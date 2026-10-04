//! §12.5 "Launch": a session's start under `MutationGuard` and the account lock (Task 10),
//! §12.3's per-launch login check, and §12.5's exit handling once `claude` exits (Task 11).

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use tagteam_provider::profile::{LAUNCH_DIR, Seed, profile_path};
use tagteam_provider::provider::{MergeReport, SessionEnv, Validity};
use tagteam_provider::reservation::{LaunchReservation, remove_dead_reservations};
use tagteam_provider::{MutationGuard, Provider, Read, ReadError};

use crate::account_lock::AccountLock;
use crate::bootstrap::{Trigger, refusal, refuse_linked_profile};
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

/// What the freshen before the locks hands the launch under them (§12.3 step 1).
#[derive(Default)]
struct Freshened {
    warnings: Vec<String>,
    /// The gate's refusal of the stored login, exactly as §12.3 step 1 and §7.2 word it: the
    /// vault's generation is consumed, or the profile conflicts with it. The gate gets that far
    /// only for an account no session owns (§7.3 step 2), but a session can start before this
    /// launch takes its locks. So it refuses a quiescent launch there, before anything is
    /// written, and a join, which touches no credential, goes on (§12.5 step 3; fix round 2).
    refusal: Option<EngineError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchEnd {
    /// `claude` ran and exited with this status code.
    Exited(i32),
    /// The launch was refused after its reservation existed (exit 1).
    Refused,
}

/// §12.4 step 3: one summary line when keys changed on both sides since the baseline, the
/// default's values kept. Each key is named in the log only. The line is logged at warn as soon
/// as it is made, so a launch that aborts after its merge-back still reports it.
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
        let summary = format!(
            "{n} {} of position {}'s session config changed on both sides while it ran; the default home's values were kept",
            if n == 1 { "key" } else { "keys" },
            row.position
        );
        tracing::warn!(position = row.position, account = %row.id, "{summary}");
        summary
    })
}

impl Engine {
    /// §12.5 steps 1–5 (the gate refresh of step 1 included).
    ///
    /// `account` is `plan_run`'s `Session` target. An unreadable run-shell marker refuses first
    /// (§12.8). Before any lock, an interrupted switch is settled (§9.6), and a vault credential
    /// about to expire is refreshed through the gate (§12.3 step 1); a target that was removed
    /// by then is `TargetChanged`. Then, under `MutationGuard` (30 s) and the account lock:
    /// - the decision is made again (B.47): a target that was removed, or became the live
    ///   login, is `TargetChanged`, for the CLI to plan again;
    /// - a profile path that is not a real directory refuses before anything touches it;
    /// - for a quiescent profile, before anything is written, the gate's refusal before the
    ///   locks is returned, a target with no stored credential is refused, and a quarantined
    ///   one follows §7.2; a join uses no stored credential and goes on;
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
        // §12.8, as `plan_run` refuses: under a run-shell marker that cannot be read, `env` is
        // still the run shell's, so its profile would be taken for the default home.
        self.refuse_unreadable_run_shell()?;
        let p = self.provider(&account.provider)?;
        let p = p.as_ref();
        if !p.capabilities().sessions {
            return Err(no_sessions(p));
        }
        self.settle_or_refuse(&account.provider)?;
        hooks::point(self, "launch-before-freshen")?;
        let Freshened {
            mut warnings,
            refusal,
        } = self.freshen_for_launch(p, account)?;
        hooks::point(self, "launch-before-locks")?;
        let guard =
            self.guard_or_refuse_within(&account.provider, MutationGuard::BOOTSTRAP_TIMEOUT)?;
        let lock = self.lock_account(&account.id)?;
        // 1.
        let row = self.relocked_target(p, account)?;
        hooks::point(self, "launch-locked")?;
        let profile = profile_path(&self.env, &row.id);
        // Through a link at the profile path, everything below would land at its target: the
        // reservations, the merge-back, the marker, the links and the seed (controller ruling).
        refuse_linked_profile(&profile)?;
        // A dead (free) reservation never counts (§12.5), so the state is read before step 2
        // removes them, and a quiescent launch refused for its stored login writes nothing.
        let state = self.session_state(p, &row)?;
        let joining = state.owned();
        if !joining {
            // The stored login this launch starts the profile from, which a join never uses.
            if let Some(refusal) = refusal {
                return Err(refusal);
            }
            warnings.extend(self.locked_login(p, &row)?);
        }
        let reservations = profile.join(LAUNCH_DIR);
        // 2.
        if profile.is_dir() {
            let removed =
                remove_dead_reservations(&profile).map_err(|e| reservation_io(&reservations, e))?;
            // Counted, never named (§14.2, B.69): a file of any name ending `.lock` counts as a
            // reservation there, and not every one is tagteam's `<pid>.lock`.
            if !removed.is_empty() {
                tracing::debug!(
                    position = row.position,
                    account = %row.id,
                    removed = removed.len(),
                    "removed dead launch reservations"
                );
            }
        }
        if let SessionState::Unreadable { detail, .. } = &state {
            warnings.push(format!(
                "a session of position {} may be running ({detail}), so this launch joins it as it is",
                row.position
            ));
        }
        // A marker naming another account refuses first: that profile is neither merged back
        // nor marked, and a join makes no link in it.
        let marker = self.own_marker(&row, &profile)?;
        if !joining {
            // §12.4: a killed session's baseline is in the profile's actual directory, which
            // the marker's spelling no longer names once the data directory has moved
            // (Decision 22).
            if marker.is_some() && p.has_baseline(&profile) {
                let report = p.merge_back(&self.env, &profile, self.cancel())?;
                warnings.extend(merge_summary(&row, &report));
            }
            self.mark_profile(p, &row, &profile)?;
        }
        // 3.
        warnings.extend(self.sync_profile_links(p, &profile, joining)?.warnings);
        let bootstrapped = if joining {
            false
        } else {
            let (bootstrapped, summary) =
                self.prepare_quiescent(p, &row, &profile, cwd, program, &guard, &lock)?;
            // A baseline that had no marker beside it is merged back by the seed or the
            // bootstrap, and its summary is this launch's too.
            warnings.extend(summary);
            bootstrapped
        };
        // 4.
        let spelling = self.recorded_spelling(&row, &profile)?;
        let reservation = LaunchReservation::create(&profile)
            .map_err(|e| reservation_refused(&reservations, e))?;
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
    /// as §7.2's direct-target column. Returns the warnings to show, and the gate's refusal to
    /// carry across the locks. A vault read that fails because the account was removed since
    /// `plan_run` is `TargetChanged` (B.47).
    ///
    /// Nothing about the stored login refuses here: whether the launch joins a running session
    /// is known only under the locks, and a join uses no stored credential (§12.5 step 3). A
    /// quarantined account is never refreshed (§7.4), and one whose stored credential cannot be
    /// read has nothing to refresh: `locked_login` judges both for a quiescent launch. The
    /// gate's refusals are `Freshened.refusal`.
    fn freshen_for_launch(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
    ) -> Result<Freshened, EngineError> {
        if !p.kind_traits(&row.kind).refreshable || row.quarantine_reason.is_some() {
            return Ok(Freshened::default());
        }
        let vault = match self.vault_generation(row) {
            Ok(vault) => vault,
            Err(failure) => {
                return match self.removed_or(row, failure) {
                    removed @ EngineError::TargetChanged { .. } => Err(removed),
                    _ => Ok(Freshened::default()),
                };
            }
        };
        if !self.due(p, &vault) {
            return Ok(Freshened::default());
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
        let warned = |warning: String| Freshened {
            warnings: vec![warning],
            refusal: None,
        };
        let refused = |refusal: EngineError| Freshened {
            warnings: Vec::new(),
            refusal: Some(refusal),
        };
        Ok(match self.refresh_stored(p, &row.id, &vault)? {
            // Busy: the account lock this launch waits for, and the rescue settlement under it,
            // pick up the other process's refresh.
            GateOutcome::Refreshed(_) | GateOutcome::AlreadyFresh(_) | GateOutcome::Busy => {
                Freshened::default()
            }
            // The live login: the re-check under the locks decides. A session: this launch
            // joins it, and its `claude` refreshes the token.
            GateOutcome::Owned(OwnedBy::Live | OwnedBy::Session) => Freshened::default(),
            // The mutation lock decides, as it does for a switch (§7.2).
            GateOutcome::Owned(OwnedBy::Journal) => warned(cannot_refresh(
                app,
                &row.label,
                "an unfinished switch names it",
            )),
            // The vault's generation is dead, and the gate has quarantined the account. It
            // answers a quarantine before it asks who owns the account, so this may be a
            // join's: `locked_login` refuses a quiescent launch on the row it reads under the
            // locks (fix round 1).
            GateOutcome::Dead(_) => Freshened::default(),
            // Spent, its successor lost (§7.3 step 6). The gate's `successor_lost` quarantine is
            // best effort, so the refusal itself is carried (fix round 2).
            GateOutcome::Unpersisted => refused(needs_relogin(row)),
            GateOutcome::Transient { rescued: true, .. } => refused(pending(
                "the refresh succeeded, but the vault could not be written; the new token is in rescue/",
            )),
            GateOutcome::Transient { kind, .. } if kind == "rescue-unreadable" => {
                refused(pending("a pending rescue could not be adopted"))
            }
            // Nothing was spent: the stored credential goes on (§12.3 step 1).
            GateOutcome::Transient { kind, .. } => warned(cannot_refresh(app, &row.label, &kind)),
            GateOutcome::Systemic(detail) => warned(cannot_refresh(app, &row.label, &detail)),
            GateOutcome::Conflict => refused(EngineError::ProfileConflict {
                position: row.position,
                label: row.label.clone(),
            }),
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

    /// The stored login a quiescent launch bootstraps or seeds from, judged under the locks on
    /// the row read there: `plan_run` reads neither the vault nor the quarantine, and a refresh
    /// that finished meanwhile may have quarantined the account (as §9.4 step 1 says for a
    /// switch). An account with no stored credential, or one that cannot be read, is refused. A
    /// quarantined one is never refreshed (§7.4): once its access token is due it is refused as
    /// `Dead` is, and otherwise this returns the warning that it needs a new login (§7.2).
    ///
    /// Only for a quiescent launch (controller ruling): a join seeds nothing and touches no
    /// credential, since the running session's agent owns the token (§12.5 step 3).
    fn locked_login(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
    ) -> Result<Option<String>, EngineError> {
        let vault = self.vault_generation(row)?;
        if row.quarantine_reason.is_none() || !p.kind_traits(&row.kind).refreshable {
            return Ok(None);
        }
        if self.due(p, &vault) {
            return Err(needs_relogin(row));
        }
        Ok(Some(works_until_expiry(row)))
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
    /// Returns whether it bootstrapped, and the summary of a merge-back the seed or the bootstrap
    /// ran (§12.4 step 3). A bootstrap validates in `cwd`, spawning `program`.
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
    ) -> Result<(bool, Option<String>), EngineError> {
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
            return Ok((false, self.seed_of(p, row, profile)?));
        };
        tracing::info!(
            position = row.position,
            account = %row.id,
            why,
            "bootstrapping the session profile"
        );
        let summary = self.bootstrap_profile(p, row, profile, cwd, program, guard, lock)?;
        Ok((true, summary))
    }

    /// §12.3 "Every launch is checked", run after the launch's locks are released, with its
    /// reservation held, in `cwd`, the directory `claude` will run in, spawning `program`, the
    /// launch command `plan_run` resolved (Decision 20). A launch that bootstrapped was checked
    /// under its locks, and skips it. The check's wait is a cancellation point: a signal
    /// recorded by its end interrupts the launch (§12.5 "Signals"), as
    /// `Err(EngineError::Interrupted(n))`, read through the token and never through the
    /// `Validity` text. `valid` launches; `overridden`, `drifted`, `unknown` and `unreachable`
    /// refuse and keep the profile. `invalid` refuses too, keeps it, and records that the next
    /// launch bootstraps it. After a refusal the caller runs
    /// `finish_run(.., LaunchEnd::Refused)`.
    pub fn check_login(
        &self,
        launched: &Launched,
        program: &Path,
        cwd: &Path,
    ) -> Result<(), EngineError> {
        if launched.bootstrapped {
            return Ok(());
        }
        let row = &launched.account;
        let p = self.provider(&row.provider)?;
        let identity = p.parse_identity(&row.identity_json)?;
        let validity = p.validate_profile(
            &self.env,
            &launched.spelling,
            cwd,
            program,
            &identity,
            self.spawner.as_ref(),
            self.cancel(),
        );
        if let Some(signal) = self.cancel().requested() {
            return Err(EngineError::Interrupted(signal));
        }
        if matches!(validity, Validity::Invalid(_)) {
            self.mark_needs_bootstrap(row, &launched.profile);
        }
        refusal(row, validity).map_or(Ok(()), Err)
    }

    /// §12.3: an `invalid` login outside a bootstrap keeps the profile and marks it for the
    /// next launch to bootstrap. The reservation this launch holds keeps every other seed writer
    /// away (each needs a quiescent profile), so no lock is taken. With no seed, the next launch
    /// bootstraps anyway. A seed that cannot be written is logged: the next check finds the
    /// same `invalid`.
    fn mark_needs_bootstrap(&self, row: &AccountRow, profile: &Path) {
        let result = match Seed::read(profile) {
            Read::Present(seed) => Seed {
                needs_bootstrap: true,
                ..seed
            }
            .write(profile)
            .map_err(EngineError::from),
            Read::Absent => Ok(()),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
        };
        if let Err(e) = result {
            // §14.2, B.69: by its kind. An error's message may name the account's label.
            tracing::warn!(
                position = row.position,
                account = %row.id,
                kind = e.kind(),
                profile = %profile.display(),
                "could not mark the session profile for a bootstrap"
            );
        }
    }

    /// §12.5 "When the child exits": capture and merge-back when last out, then the unlink.
    /// Returns the notices to print; never changes the exit code (B.63).
    ///
    /// It runs under `MutationGuard` (30 s, as a launch) and the account lock, whose waits are
    /// its cancellation points. `end` changes nothing: a launch refused after its reservation
    /// exists is handled as if `claude` had exited at once (§12.3). A failed or cancelled step
    /// gives one notice (`exit_notice`). What it left is completed by lazy capture and the next
    /// launch, which loses nothing (§12.5 "Signals"), unless the profile's provenance stopped it.
    /// The log names the failure by its kind only (§14.2, B.69).
    pub fn finish_run(&self, launched: Launched, end: LaunchEnd) -> Vec<String> {
        let (position, id) = (launched.account.position, launched.account.id.clone());
        tracing::debug!(position, account = %id, ?end, "the session ended; exit handling starts");
        let mut notices = Vec::new();
        if let Err(e) = self.finish_locked(launched, &mut notices) {
            tracing::warn!(
                position,
                account = %id,
                kind = e.kind(),
                "exit handling did not finish"
            );
            notices.push(exit_notice(position, &e));
        }
        notices
    }

    /// `finish_run`'s work. Without the locks, the reservation is left as it is: it dies with
    /// this process, and the next launch removes it (§12.5 step 2). Once both are held, every
    /// step runs that can, the unlink last whatever failed before it (R11.2), and the first
    /// failure is returned. An unlink that fails after an earlier failure is logged.
    fn finish_locked(
        &self,
        launched: Launched,
        notices: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        let Launched {
            account,
            profile,
            reservation,
            ..
        } = launched;
        let p = self.provider(&account.provider)?;
        let p = p.as_ref();
        hooks::point(self, "exit-before-locks")?;
        let guard = MutationGuard::acquire(&self.env, MutationGuard::BOOTSTRAP_TIMEOUT)?;
        let lock = self.lock_account(&account.id)?;
        let own = reservation.path().to_path_buf();
        let failed = self
            .exit_under_locks(p, &account, &profile, &own, &lock, notices)
            .err();
        // Last, in either case.
        let unlinked = reservation.unlink().map_err(|e| reservation_io(&own, e));
        drop(lock);
        drop(guard);
        match (failed, unlinked) {
            (Some(first), Err(unlink)) => {
                tracing::warn!(
                    position = account.position,
                    account = %account.id,
                    kind = unlink.kind(),
                    reservation = %own.display(),
                    "the launch reservation could not be unlinked either"
                );
                Err(first)
            }
            (Some(first), Ok(())) => Err(first),
            (None, unlinked) => unlinked,
        }
    }

    /// Exit handling's steps under both locks, before the unlink. When the profile is quiescent
    /// apart from `own`, this process's reservation, it captures a rotation through the
    /// profile's provenance, under the account lock with no network (§6.2), then merges the
    /// profile's config back from a waiting baseline. Otherwise another session still runs (a
    /// `run`, or a `bg` or `daemon` record, §12.6), and nothing is done. A failed capture does
    /// not stop the merge-back; the first failure is returned, and a merge-back failure after
    /// it is logged (§14). A provenance `Conflict` or `Unreadable` counts as one: nothing is
    /// captured, and the next launch refuses or aborts.
    fn exit_under_locks(
        &self,
        p: &dyn Provider,
        account: &AccountRow,
        profile: &Path,
        own: &Path,
        lock: &AccountLock,
        notices: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        hooks::point(self, "exit-locked")?;
        // A session-owned account is never removed (§10.3), so this is a defence only.
        let Some(row) = self.store()?.account(&account.id)? else {
            return Ok(());
        };
        match self.session_state_apart_from(p, &row, own)? {
            SessionState::Quiescent { .. } => {}
            // §14.2, B.69: the state only. Its detail names the file, and a record's name is not
            // tagteam's to choose; the next launch's warning names it to the user.
            SessionState::Unreadable { .. } => {
                tracing::warn!(
                    position = row.position,
                    account = %row.id,
                    state = "unreadable",
                    "a session reservation or record could not be read; the profile counts as in use, so capture and merge-back wait"
                );
                return Ok(());
            }
            SessionState::Owned { .. } | SessionState::NoProfile => return Ok(()),
        }
        let mut failed: Option<EngineError> = None;
        // 1. Capture: provenance under the account lock, with no network (§6.2).
        match self.apply_provenance_apart_from(p, &row, lock, own) {
            Ok(ProfileCheck::Conflict) => {
                failed = Some(EngineError::ProfileConflict {
                    position: row.position,
                    label: row.label.clone(),
                });
            }
            Ok(ProfileCheck::Unreadable(detail)) => {
                failed = Some(EngineError::Unreadable(ReadError::new(
                    profile.display().to_string(),
                    detail,
                )));
            }
            Ok(_) => {}
            Err(e) => failed = Some(e),
        }
        // 2. Merge back, with the default home's config lock taken alone (§4.3), from the
        //    baseline in the profile's actual directory (Decision 22). A session that joined
        //    wrote none, and the last one out may have joined.
        if p.has_baseline(profile) {
            match p.merge_back(&self.env, profile, self.cancel()) {
                Ok(report) => notices.extend(merge_summary(&row, &report)),
                Err(e) => {
                    let e = EngineError::from(e);
                    if failed.is_some() {
                        tracing::warn!(
                            position = row.position,
                            account = %row.id,
                            kind = e.kind(),
                            "the session's config could not be merged back either"
                        );
                    }
                    failed.get_or_insert(e);
                }
            }
        }
        failed.map_or(Ok(()), Err)
    }
}

/// Exit handling's notice when it did not finish (Decision 12). Most failures leave work that
/// lazy capture and the next launch complete. A provenance conflict, or a profile that cannot be
/// read, stops the next launch as well (§12.5 step 3), so the notice ends with the error itself,
/// which says what is wrong and, for a conflict, what resolves it.
fn exit_notice(position: u32, e: &EngineError) -> String {
    match e {
        EngineError::ProfileConflict { .. } | EngineError::Unreadable(_) => {
            format!("exit handling for position {position} did not finish: {e}")
        }
        _ => format!(
            "exit handling for position {position} did not finish ({e}); nothing is lost: the next launch, switch or refresh of the account completes it"
        ),
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

/// The reservation calls report I/O errors without a path (Task 4), so the refusal names the
/// reservation directory, `dir`.
fn reservation_io(dir: &Path, e: io::Error) -> EngineError {
    EngineError::Io(io::Error::new(e.kind(), format!("{}: {e}", dir.display())))
}

/// Interface Contract: `create` refuses a `<pid>.lock` that is live, the reservation of an
/// orphaned `claude` of an earlier process with this pid (Task 4). Replacing it would hide a
/// running session, so the launch is refused, naming the file, as `launch-unreachable`; a later
/// `run` gets a new pid. Once `dir` is a directory only that check reports `AlreadyExists`, and
/// its message names the file. Any other failure, a file where `dir` belongs included, names
/// `dir`.
fn reservation_refused(dir: &Path, e: io::Error) -> EngineError {
    if e.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() {
        EngineError::ReservationHeld {
            detail: e.to_string(),
        }
    } else {
        reservation_io(dir, e)
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
