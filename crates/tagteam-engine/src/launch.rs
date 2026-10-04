//! §12.5 "Launch": a session's start under `MutationGuard` and the account lock (Task 10).
//! Task 11 adds the per-launch login check and the exit handling.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use tagteam_provider::profile::profile_path;
use tagteam_provider::provider::{MergeReport, SessionEnv};
use tagteam_provider::reservation::{LaunchReservation, remove_dead_reservations};
use tagteam_provider::{MutationGuard, Provider, ReadError};

use crate::account_lock::AccountLock;
use crate::bootstrap::Trigger;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::provenance::ProfileCheck;
use crate::refresh::{GateOutcome, OwnedBy};
use crate::run::no_sessions;
use crate::session::SessionState;
use crate::store::AccountRow;
use crate::switch::{cannot_refresh, needs_relogin, works_until_expiry};

pub struct Launched {
    pub account: AccountRow,
    pub profile: PathBuf,
    pub spelling: String,
    pub reservation: LaunchReservation,
    pub env: SessionEnv,
    /// This launch bootstrapped, so its login check already ran (§12.3).
    pub bootstrapped: bool,
    /// Every warning to print, §12.5's environment warnings last (Decision 18).
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchEnd {
    /// `claude` ran and exited with this status code.
    Exited(i32),
    /// The launch was refused after its reservation existed (exit 1).
    Refused,
}

/// §12.4 step 3: one summary line when keys changed on both sides since the baseline, the
/// default's values kept. Each key is named in the log only.
pub(crate) fn merge_summary(row: &AccountRow, report: &MergeReport) -> Option<String> {
    for key in &report.conflicts {
        tracing::info!(
            position = row.position,
            account = %row.id,
            key = %key,
            "kept the default home's value of a key the session changed too"
        );
    }
    tracing::debug!(
        position = row.position,
        account = %row.id,
        applied = report.applied,
        "merged the session's config back"
    );
    let n = report.conflicts.len();
    (n > 0).then(|| {
        format!(
            "{n} {} of position {}'s session config changed on both sides while it ran; the default home's values were kept",
            if n == 1 { "key" } else { "keys" },
            row.position
        )
    })
}

impl Engine {
    /// §12.5 steps 1–5 (the gate refresh of step 1 included).
    ///
    /// `account` is `plan_run`'s `Session` target. Before any lock, an interrupted switch is
    /// settled (§9.6), and a vault credential about to expire is refreshed through the gate
    /// (§12.3 step 1); a target that was removed by then is `TargetChanged`. Then, under
    /// `MutationGuard` (30 s) and the account lock:
    /// - the decision is made again (B.47): a target that was removed, or became the live
    ///   login, is `TargetChanged`, for the CLI to plan again;
    /// - dead reservations go, and a quiescent profile's pending merge-back runs, from the
    ///   baseline in its actual directory (Decision 22; a failure aborts);
    /// - the marker and the links are brought up to date;
    /// - a quiescent profile gets its provenance, then a bootstrap or the seed, and a running
    ///   one is joined as it is;
    /// - the reservation is created last; a live one already under this pid refuses
    ///   (`launch-unreachable`).
    ///
    /// The locks are released when this returns. `program` is the launch command `plan_run`
    /// resolved, which a bootstrap's validation spawns (Decision 20). `Launched.warnings` holds
    /// every warning, §12.5's environment warnings last (Decision 18).
    pub fn launch(
        &self,
        account: &AccountRow,
        program: &Path,
        cwd: &Path,
    ) -> Result<Launched, EngineError> {
        let p = self.provider(&account.provider)?;
        let p = p.as_ref();
        if !p.capabilities().sessions {
            return Err(no_sessions(p));
        }
        self.settle_or_refuse(&account.provider)?;
        hooks::point(self, "launch-before-freshen")?;
        let mut warnings = self.freshen_for_launch(p, account)?;
        hooks::point(self, "launch-before-locks")?;
        let guard =
            self.guard_or_refuse_within(&account.provider, MutationGuard::BOOTSTRAP_TIMEOUT)?;
        let lock = self.lock_account(&account.id)?;
        // 1.
        let row = self.relocked_target(p, account)?;
        self.locked_login(p, &row, &mut warnings)?;
        hooks::point(self, "launch-locked")?;
        let profile = profile_path(&self.env, &row.id);
        // 2.
        if profile.is_dir() {
            for dead in remove_dead_reservations(&profile)? {
                tracing::debug!(
                    position = row.position,
                    account = %row.id,
                    reservation = %dead.display(),
                    "removed a dead launch reservation"
                );
            }
        }
        let state = self.session_state(p, &row)?;
        let joining = state.owned();
        if let SessionState::Unreadable { detail, .. } = &state {
            warnings.push(format!(
                "a session of position {} may be running ({detail}), so this launch joins it as it is",
                row.position
            ));
        }
        if !joining {
            // §12.4: a killed session's baseline is in the profile's actual directory, which
            // the marker's spelling no longer names once the data directory has moved
            // (Decision 22). A marker naming another account refuses first.
            if self.own_marker(&row, &profile)?.is_some() && p.has_baseline(&profile) {
                let report = p.merge_back(&self.env, &profile, self.cancel())?;
                warnings.extend(merge_summary(&row, &report));
            }
            self.mark_profile(p, &row, &profile)?;
        }
        // 3.
        warnings.extend(self.sync_profile_links(p, &profile, joining)?.warnings);
        let bootstrapped =
            !joining && self.prepare_quiescent(p, &row, &profile, cwd, program, &guard, &lock)?;
        // 4.
        let spelling = self.recorded_spelling(&row, &profile)?;
        let reservation = LaunchReservation::create(&profile).map_err(reservation_refused)?;
        // 5.
        drop(lock);
        drop(guard);
        let env = p.session_env(&spelling);
        // §12.5 "Environment", Decision 18: the scrubbed variables this process has set are in
        // `Env.vars`, which the CLI records at its boundary; the home variable is the outer
        // home's, inside a run shell too.
        let home = p.session_dir_var().zip(p.session_dir(&self.env));
        warnings.extend(environment_warnings(
            &env,
            &|name| name.to_str().is_some_and(|n| self.env.var(n).is_some()),
            home.as_ref().map(|(var, dir)| (*var, dir.as_path())),
        ));
        Ok(Launched {
            env,
            account: row,
            profile,
            spelling,
            reservation,
            bootstrapped,
            warnings,
        })
    }

    /// The spelling every operation on the profile's credential uses (§12.2): the marker's.
    pub(crate) fn recorded_spelling(
        &self,
        row: &AccountRow,
        profile: &Path,
    ) -> Result<String, EngineError> {
        self.own_marker(row, profile)?
            .map(|m| m.config_dir)
            .ok_or_else(|| {
                EngineError::InvalidInput(format!("{} has no profile marker", profile.display()))
            })
    }

    /// §12.3 step 1, before any lock. A vault credential about to expire is refreshed through
    /// the gate (§7.3), as a switch freshens its target (§7.2), so a bootstrap starts the
    /// profile from a fresh generation. The outcomes map as §12.3 step 1 says, and otherwise
    /// as §7.2's direct-target column. Returns the warnings to show. A vault read that fails
    /// because the account was removed since `plan_run` is `TargetChanged` (B.47).
    fn freshen_for_launch(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
    ) -> Result<Vec<String>, EngineError> {
        if !p.kind_traits(&row.kind).refreshable {
            return Ok(Vec::new());
        }
        let vault = self
            .vault_generation(row)
            .map_err(|failure| self.removed_or(row, failure))?;
        let due = self.due(p, &vault);
        if row.quarantine_reason.is_some() {
            // §7.4: never refreshed; usable only while its access token lasts.
            return if due {
                Err(needs_relogin(row))
            } else {
                Ok(vec![works_until_expiry(row)])
            };
        }
        if !due {
            return Ok(Vec::new());
        }
        let app = p.display_name();
        let pending = |detail: &str| EngineError::RescuePending {
            position: row.position,
            label: row.label.clone(),
            detail: detail.to_owned(),
        };
        // §14.1 (R10.2): nothing is locked yet, and the gate has no cancellation point of its
        // own, so a signal that has landed stops the launch before it spends the refresh token.
        self.check_cancel()?;
        Ok(match self.refresh_stored(p, &row.id, &vault)? {
            // Busy: the account lock this launch waits for, and the rescue settlement under it,
            // pick up the other process's refresh.
            GateOutcome::Refreshed(_) | GateOutcome::AlreadyFresh(_) | GateOutcome::Busy => {
                Vec::new()
            }
            // The live login: the re-check under the locks decides. A session: this launch
            // joins it, and its `claude` refreshes the token.
            GateOutcome::Owned(OwnedBy::Live | OwnedBy::Session) => Vec::new(),
            // The mutation lock decides, as it does for a switch (§7.2).
            GateOutcome::Owned(OwnedBy::Journal) => vec![cannot_refresh(
                app,
                &row.label,
                "an unfinished switch names it",
            )],
            // The vault's generation is dead, or spent with its successor lost (§7.3 step 6).
            GateOutcome::Dead(_) | GateOutcome::Unpersisted => return Err(needs_relogin(row)),
            GateOutcome::Transient { rescued: true, .. } => {
                return Err(pending(
                    "the refresh succeeded, but the vault could not be written; the new token is in rescue/",
                ));
            }
            GateOutcome::Transient { kind, .. } if kind == "rescue-unreadable" => {
                return Err(pending("a pending rescue could not be adopted"));
            }
            // Nothing was spent: the stored credential goes on (§12.3 step 1).
            GateOutcome::Transient { kind, .. } => vec![cannot_refresh(app, &row.label, &kind)],
            GateOutcome::Systemic(detail) => vec![cannot_refresh(app, &row.label, &detail)],
            GateOutcome::Conflict => {
                return Err(EngineError::ProfileConflict {
                    position: row.position,
                    label: row.label.clone(),
                });
            }
        })
    }

    /// `failure`, from a read of `planned` before the locks, unless the account is gone. A
    /// `remove` that completed after `plan_run` is then `TargetChanged`, as the re-check under
    /// the locks would say (Decision 14, B.47). Only a present account propagates `failure`; a
    /// re-read that fails itself leaves `failure` as it is.
    fn removed_or(&self, planned: &AccountRow, failure: EngineError) -> EngineError {
        match self
            .store()
            .and_then(|s| s.account(&planned.id).map_err(EngineError::from))
        {
            Ok(None) => target_removed(planned),
            Ok(Some(_)) | Err(_) => failure,
        }
    }

    /// §12.5 launch step 1 and B.47: the target as it stands under the locks. If it was removed
    /// (Decision 7), or became the live default login, this returns `TargetChanged`, for the
    /// CLI to plan again. A login replaced by an API key since `plan_run` is refused, as
    /// `plan_run` would refuse it.
    fn relocked_target(
        &self,
        p: &dyn Provider,
        planned: &AccountRow,
    ) -> Result<AccountRow, EngineError> {
        let Some(row) = self.store()?.account(&planned.id)? else {
            return Err(target_removed(planned));
        };
        if p.kind_traits(&row.kind).managed_key_axis {
            return Err(EngineError::ApiKeyAccount {
                position: row.position,
            });
        }
        if self.is_live_login(p, &row)? {
            return Err(EngineError::TargetChanged {
                why: format!(
                    "position {} became the live login while `tagteam run` started it",
                    row.position
                ),
            });
        }
        Ok(row)
    }

    /// §12.5 launch step 1 for the login itself, on the row read under the locks: `plan_run`
    /// reads neither the vault nor the quarantine, and the freshen before the locks may be out
    /// of date (a refresh that finished meanwhile may have quarantined the account, as §9.4 step
    /// 1 says for a switch). An account with no stored credential, or one that cannot be read,
    /// is refused. A quarantined one is never refreshed (§7.4): once its access token is due it
    /// is refused as `Dead` is, and otherwise it launches with the warning that it needs a new
    /// login, unless the freshen gave that warning already (§7.2).
    fn locked_login(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        warnings: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        let vault = self.vault_generation(row)?;
        if row.quarantine_reason.is_none() || !p.kind_traits(&row.kind).refreshable {
            return Ok(());
        }
        if self.due(p, &vault) {
            return Err(needs_relogin(row));
        }
        let warning = works_until_expiry(row);
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
        Ok(())
    }

    /// §12.5 step 3 for a quiescent profile, after its sync. Pending rescues go first (§6.2),
    /// then the profile's provenance (§12.5):
    /// - a rotation is captured;
    /// - a conflict refuses;
    /// - a profile that cannot be read aborts, since tagteam never overwrites what it could not
    ///   read;
    /// - a vault that moved on, or a stale-marked profile, bootstraps.
    ///
    /// Any other §12.3 trigger bootstraps too; otherwise the profile is seeded (§12.4).
    /// Returns whether it bootstrapped. A bootstrap validates in `cwd`, spawning `program`.
    #[allow(clippy::too_many_arguments)]
    fn prepare_quiescent(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        profile: &Path,
        cwd: &Path,
        program: &Path,
        guard: &MutationGuard,
        lock: &AccountLock,
    ) -> Result<bool, EngineError> {
        self.settle_rescues(p, row, lock)?;
        let why = match self.apply_provenance(p, row, lock)? {
            ProfileCheck::Conflict => {
                return Err(EngineError::ProfileConflict {
                    position: row.position,
                    label: row.label.clone(),
                });
            }
            ProfileCheck::Unreadable(detail) => {
                return Err(EngineError::Unreadable(ReadError::new(
                    profile.display().to_string(),
                    detail,
                )));
            }
            ProfileCheck::VaultMovedOn => Some("vault-moved-on"),
            ProfileCheck::ReplacementWins => Some("replacement-wins"),
            ProfileCheck::InStep | ProfileCheck::Captured | ProfileCheck::NotApplicable => self
                .bootstrap_trigger(p, row, profile)?
                .map(Trigger::as_str),
        };
        let Some(why) = why else {
            self.seed_of(p, row, profile)?;
            return Ok(false);
        };
        tracing::info!(
            position = row.position,
            account = %row.id,
            why,
            "bootstrapping the session profile"
        );
        self.bootstrap_profile(p, row, profile, cwd, program, guard, lock)?;
        Ok(true)
    }
}

/// §12.5 "Environment" (Decision 18): one warning per scrubbed variable this process has set,
/// which the session would otherwise inherit, and one for the provider's home variable when the
/// outer home names another directory than the profile. `is_set` answers for this process's
/// environment; `home` is the provider's variable and the directory the outer home gives it.
pub(crate) fn environment_warnings(
    session: &SessionEnv,
    is_set: &dyn Fn(&OsStr) -> bool,
    home: Option<(&str, &Path)>,
) -> Vec<String> {
    let mut warnings: Vec<String> = session
        .remove
        .iter()
        .filter(|name| is_set(name.as_os_str()))
        .map(|name| {
            format!(
                "{} is set here; the session runs without it, so it cannot change which login the session uses",
                name.to_string_lossy()
            )
        })
        .collect();
    if let Some((var, dir)) = home {
        let overridden = session
            .set
            .iter()
            .any(|(name, value)| name.as_os_str() == OsStr::new(var) && Path::new(value) != dir);
        if overridden {
            warnings.push(format!(
                "{var} is set to {}; the session runs in its own profile instead",
                dir.display()
            ));
        }
    }
    warnings
}

/// Decision 14: `planned` was removed while `tagteam run` started it, and its mapping with it
/// (Decision 7). Before the locks and under them alike.
fn target_removed(planned: &AccountRow) -> EngineError {
    EngineError::TargetChanged {
        why: format!(
            "position {} was removed while `tagteam run` started it",
            planned.position
        ),
    }
}

/// Interface Contract: `create` refuses a `<pid>.lock` that is live, the reservation of an
/// orphaned `claude` of an earlier process with this pid (Task 4). Replacing it would hide a
/// running session, so the launch is refused, naming the file; a later `run` gets a new pid.
fn reservation_refused(e: io::Error) -> EngineError {
    if e.kind() == io::ErrorKind::AlreadyExists {
        EngineError::LaunchUnreachable {
            detail: format!("{e}; run it again"),
        }
    } else {
        e.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_env() -> SessionEnv {
        SessionEnv {
            set: vec![("CLAUDE_CONFIG_DIR".into(), "/data/sessions/a".into())],
            remove: vec![
                "ANTHROPIC_API_KEY".into(),
                "CLAUDE_CODE_OAUTH_TOKEN".into(),
                "CLAUDE_SECURESTORAGE_CONFIG_DIR".into(),
            ],
        }
    }

    #[test]
    fn each_scrubbed_variable_that_is_set_is_named_once() {
        let set = |n: &OsStr| {
            n == OsStr::new("ANTHROPIC_API_KEY")
                || n == OsStr::new("CLAUDE_SECURESTORAGE_CONFIG_DIR")
        };
        let w = environment_warnings(&session_env(), &set, None);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[0].starts_with("ANTHROPIC_API_KEY "), "{w:?}");
        assert!(
            w[1].starts_with("CLAUDE_SECURESTORAGE_CONFIG_DIR "),
            "{w:?}"
        );
        assert!(environment_warnings(&session_env(), &|_| false, None).is_empty());
    }

    #[test]
    fn a_home_variable_the_session_overrides_is_named_unless_it_names_the_profile() {
        let none = |_: &OsStr| false;
        let elsewhere = Path::new("/Users/me/.claude-work");
        let w = environment_warnings(
            &session_env(),
            &none,
            Some(("CLAUDE_CONFIG_DIR", elsewhere)),
        );
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].starts_with("CLAUDE_CONFIG_DIR ") && w[0].contains("/Users/me/.claude-work"),
            "{w:?}"
        );
        let same = Path::new("/data/sessions/a");
        assert!(
            environment_warnings(&session_env(), &none, Some(("CLAUDE_CONFIG_DIR", same)))
                .is_empty()
        );
        assert!(
            environment_warnings(&session_env(), &none, Some(("FAKEAGENT_HOME", elsewhere)))
                .is_empty(),
            "a variable the session does not set is not overridden"
        );
    }
}
