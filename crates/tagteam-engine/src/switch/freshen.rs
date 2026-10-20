use tagteam_core::AccountId;
use tagteam_provider::{Provider, Read};

use crate::collect::CollectMode;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::provenance::ProfileCheck;
use crate::refresh::{GateOutcome, OwnedBy};
use crate::rescue::RescueFile;
use crate::store::{AccountRow, Store};

use super::plan::{Ask, Plan, Planned};
use super::support::{
    cannot_refresh, needs_relogin, profile_conflict, session_owned, works_until_expiry,
};
use super::{SwitchRequest, SwitchTarget};

/// §7.2: a target whose access token expires within this many milliseconds is refreshed
/// before it is activated. Twice CC's own 5-minute buffer.
const FRESHEN_WINDOW_MS: i64 = 10 * 60 * 1000;

/// What freshening a plan's target decided (§7.2, the manual-switch table).
enum Freshened {
    /// Go ahead, with these warnings for the outcome.
    Go(Vec<String>),
    /// A rotation's or a usage strategy's pick turned out dead and is quarantined now, or a
    /// session took it since planning: plan again; the walk skips it (§9.3).
    Replan,
}

/// What freshening an automatic switch's target decided (§11.2 step 10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AutoFreshened {
    /// Perform the switch.
    Ready,
    /// A Dead verdict, an identity conflict or a lost successor: the gate has quarantined it
    /// (§7.4). Try the next target.
    Quarantined,
    /// Transient or systemic, with what went wrong. Try the next target.
    Failed(String),
    /// Not a candidate after all: no stored credential, owned by a session, or a session
    /// profile that conflicts with the vault (§12.5). Try the next target.
    Skip,
}

impl Engine {
    /// §7.2: refreshes the plan's target through the gate when its access token is about to
    /// expire. This runs before the mutation lock is taken (§4.3). A rotation whose pick turns
    /// out dead, or that a session took, plans again from the current roster, which now skips
    /// it; every round takes one more account out of it, so the rounds are bounded by the
    /// roster.
    pub(super) fn freshen_plan(
        &self,
        p: &dyn Provider,
        store: &Store,
        req: &SwitchRequest,
        mut plan: Plan,
    ) -> Result<Planned, EngineError> {
        for _ in 0..=store.accounts(&req.provider)?.len() {
            match self.freshen(p, req, &plan)? {
                Freshened::Go(warnings) => {
                    plan.warnings.extend(warnings);
                    return Ok(Planned::Go(plan));
                }
                Freshened::Replan => {
                    plan = match self.plan(p, store, req, Ask::Reuse(plan.hint.take()))? {
                        Planned::Done(outcome) => return Ok(Planned::Done(outcome)),
                        Planned::Go(next) => next,
                    };
                }
            }
        }
        Err(EngineError::InvalidInput(
            "no account in the rotation could be refreshed".into(),
        ))
    }

    /// §7.2: whether `vault`'s access token expires within the freshen window. An unknown or
    /// non-numeric expiry is never due.
    pub(crate) fn due(&self, p: &dyn Provider, vault: &[u8]) -> bool {
        p.access_expires_at(vault)
            .is_some_and(|at| self.now_ms() + FRESHEN_WINDOW_MS >= at)
    }

    /// One row of §7.2's manual-switch table, for the plan's target.
    fn freshen(
        &self,
        p: &dyn Provider,
        req: &SwitchRequest,
        plan: &Plan,
    ) -> Result<Freshened, EngineError> {
        let target = &plan.target;
        // A self-switch activates what is already live: only CC, or §7.5, refreshes that token.
        // A forced one re-activates the vault's generation (§9.2), so it freshens like any other.
        if (plan.self_switch && !req.force) || !p.kind_traits(&target.kind).refreshable {
            return Ok(Freshened::Go(vec![]));
        }
        // §9.2 lazy capture before the freshen decision: a quiescent profile that rotated is
        // adopted first, so `due` judges the generation this switch will activate, which may
        // expire sooner than the vault's older one. The target's account lock is taken alone
        // and released before the switch takes `MutationGuard` (§4.3: no lock is taken while
        // a later one is held), and `transact` settles the profile again under every lock.
        // The row is read again once the lock is held: a replacement that landed since
        // planning moved `login_epoch`, and the profile's stale mark is judged against it.
        hooks::point(self, "freshen-before-lock")?;
        let reread;
        let target = {
            let lock = self.lock_account(&target.id)?;
            let current = |store: &Store| -> Result<AccountRow, EngineError> {
                store
                    .account(&target.id)?
                    .ok_or_else(|| EngineError::NoSuchAccount(target.label.clone()))
            };
            let store = self.store()?;
            let row = current(&store)?;
            self.settle_rescues(p, &row, &lock)?;
            self.settle_profile(p, &row, &lock)?;
            reread = current(&store)?;
            &reread
        };
        let vault = self.read_target(target)?;
        let due = self.due(p, &vault);
        let chosen = req.target.chosen();
        if target.quarantine_reason.is_some() {
            // Never refreshed (§7.4): usable only while its current access token lasts.
            return match (due, chosen) {
                (false, _) => Ok(Freshened::Go(vec![works_until_expiry(target)])),
                (true, true) => Ok(Freshened::Replan),
                (true, false) => Err(needs_relogin(p, target)),
            };
        }
        if !due {
            return Ok(Freshened::Go(vec![]));
        }
        let label = target.label.as_str();
        let pending = |detail: String| EngineError::RescuePending {
            position: target.position,
            label: target.label.clone(),
            detail,
        };
        // §14.1: nothing is locked yet, so a signal that has landed stops the switch before it
        // spends the target's refresh token.
        self.check_cancel()?;
        Ok(match self.refresh_stored(p, &target.id, &vault)? {
            // Busy: another process is refreshing it now. The account lock this switch waits
            // for, and the pending-rescue settle under it (§6.2), pick up that refresh.
            GateOutcome::Refreshed(_) | GateOutcome::AlreadyFresh(_) | GateOutcome::Busy => {
                Freshened::Go(vec![])
            }
            GateOutcome::Dead(_) if chosen => Freshened::Replan,
            GateOutcome::Dead(_) => return Err(needs_relogin(p, target)),
            GateOutcome::Transient { rescued: true, .. } => {
                return Err(pending(
                    "the refresh succeeded, but the vault could not be written; the new token is in rescue/".into(),
                ));
            }
            GateOutcome::Transient { kind, .. } if kind == "rescue-unreadable" => {
                // The gate reports both an unreadable rescue file and a failed adoption of a
                // readable one under this kind; only unreadable files can be named. The error's
                // own text adds "retry once the vault can be written".
                let damaged: Vec<String> = self
                    .rescues_for(&target.id)
                    .into_iter()
                    .filter_map(|r| match r {
                        RescueFile::Unreadable { path, .. } => Some(path.display().to_string()),
                        RescueFile::Entry(_) => None,
                    })
                    .collect();
                let detail = if damaged.is_empty() {
                    "a pending rescue could not be adopted".to_owned()
                } else {
                    format!("{} cannot be read", damaged.join(", "))
                };
                return Err(pending(detail));
            }
            // Nothing was spent, or what was spent is lost either way; once the account is
            // live, the gate leaves its refresh to CC (§7.3 step 2). A profile that cannot be
            // read (`profile-unreadable`) lands here too, and the transaction refuses it.
            GateOutcome::Transient { kind, .. } => {
                Freshened::Go(vec![cannot_refresh(p.display_name(), label, &kind)])
            }
            GateOutcome::Systemic(detail) => {
                Freshened::Go(vec![cannot_refresh(p.display_name(), label, &detail)])
            }
            // The successor is lost and the vault's generation is spent (§7.3 step 6): no
            // retry helps, and activating would hand CC a used refresh token.
            GateOutcome::Unpersisted => return Err(needs_relogin(p, target)),
            // A journal row names it. The gate cannot tell a switch still in progress from an
            // interrupted one, so this does not refuse: with `--force` the switch supersedes
            // the row, and without it `guard` decides under the mutation lock (it waits for a
            // live holder, and recovers or refuses a dead one with the right message).
            GateOutcome::Owned(OwnedBy::Journal) => Freshened::Go(vec![cannot_refresh(
                p.display_name(),
                label,
                "an unfinished switch names it",
            )]),
            // The gate found it live where planning did not (it became the live login, or the
            // live identity could not be read): never refreshed here. `rederive` plans the
            // self-switch again, and the warning is carried into its outcome.
            GateOutcome::Owned(OwnedBy::Live) => Freshened::Go(vec![cannot_refresh(
                p.display_name(),
                label,
                "it may be the live login",
            )]),
            // §7.2's last row, §9.2's refusals. A session that started since planning owns the
            // target: a rotation or a usage strategy plans again, and its walk now skips it
            // (§9.3); a direct target is refused. A conflicting profile refuses either way
            // (§12.5).
            GateOutcome::Owned(OwnedBy::Session) if chosen => Freshened::Replan,
            // The gate answers only "a session"; the state is read again to name a daemon.
            GateOutcome::Owned(OwnedBy::Session) => {
                let state = self.session_state(p, target)?;
                return Err(session_owned(p, target, &state));
            }
            GateOutcome::Conflict => return Err(profile_conflict(target)),
        })
    }

    /// §11.2 step 10 for one target, before its switch takes `MutationGuard` (§7.2): a stored
    /// token that expires within the freshen window is refreshed through the gate (§7.3),
    /// and the gate's outcome is read by the auto-switch table. A kind that does not refresh
    /// passes as it is, so API-key targets pass. The vault is read here, lazily (§9.3), once
    /// the target's rescues and profile are settled, as `freshen` settles them.
    pub(crate) fn freshen_auto(
        &self,
        p: &dyn Provider,
        target: &AccountRow,
    ) -> Result<AutoFreshened, EngineError> {
        if !p.kind_traits(&target.kind).refreshable {
            return Ok(AutoFreshened::Ready);
        }
        // §6.2 and §9.2 lazy capture before the due check, as in `freshen`: a quiescent profile
        // that rotated is adopted first, so `due` judges the generation this switch will
        // activate, which may expire sooner than the vault's older one (§7.2). The account lock
        // is taken alone, outside `MutationGuard`, nothing under it uses the network, and it is
        // released before the gate tries it. The row is read again under it, for the stale
        // mark. Each outcome is read by the auto-switch table: a conflict passes the target over
        // as the gate's `Conflict` does, and what cannot be read fails it as the gate's
        // unreadable outcomes do. `transact` settles both again under every lock.
        {
            let lock = self.lock_account(&target.id)?;
            let Some(row) = self.store()?.account(&target.id)? else {
                return Ok(AutoFreshened::Skip);
            };
            match self.settle_rescues(p, &row, &lock) {
                Ok(()) => {}
                // Fixed phrases (§14.2): the details name a path, and may quote a file or an
                // error that names the account.
                Err(EngineError::RescuePending { .. }) => {
                    tracing::warn!(
                        target: "tagteam_engine::switch",
                        position = row.position,
                        account = %row.id,
                        "a pending rescue could not be settled; the target is passed over"
                    );
                    return Ok(AutoFreshened::Failed("rescue-unreadable".into()));
                }
                Err(EngineError::Unreadable(e)) => return Ok(AutoFreshened::Failed(e.to_string())),
                Err(e) => return Err(e),
            }
            match self.apply_provenance(p, &row, &lock) {
                Ok(ProfileCheck::Conflict) => return Ok(AutoFreshened::Skip),
                Ok(ProfileCheck::Unreadable(_)) => {
                    tracing::warn!(
                        target: "tagteam_engine::switch",
                        position = row.position,
                        account = %row.id,
                        "the session profile could not be read; the target is passed over"
                    );
                    return Ok(AutoFreshened::Failed("profile-unreadable".into()));
                }
                Ok(_) => {}
                Err(EngineError::Unreadable(e)) => return Ok(AutoFreshened::Failed(e.to_string())),
                Err(e) => return Err(e),
            }
        }
        let vault = match self.vault.read(&target.id) {
            Read::Present(b) if !b.is_empty() => b,
            Read::Unreadable(e) => return Ok(AutoFreshened::Failed(e.to_string())),
            Read::Present(_) | Read::Absent => return Ok(AutoFreshened::Skip),
        };
        if !self.due(p, &vault) {
            return Ok(AutoFreshened::Ready);
        }
        // §14.1: nothing is locked now, so a signal that has landed stops the tick before it
        // spends the target's refresh token.
        self.check_cancel()?;
        Ok(match self.refresh_stored(p, &target.id, &vault)? {
            // Busy: another process is refreshing it now; the switch's account lock and its
            // pending-rescue settle pick that refresh up, as for a manual switch (§7.2).
            GateOutcome::Refreshed(_) | GateOutcome::AlreadyFresh(_) | GateOutcome::Busy => {
                AutoFreshened::Ready
            }
            // It became the live login, or an unfinished switch names it: the switch's own
            // checks under its locks decide (§9.4 step 1, §9.6).
            GateOutcome::Owned(OwnedBy::Live | OwnedBy::Journal) => AutoFreshened::Ready,
            GateOutcome::Dead(_) | GateOutcome::Unpersisted => AutoFreshened::Quarantined,
            // The vault's generation is spent and its successor waits in `rescue/`: activating
            // it would hand the agent a used refresh token. A later tick adopts the rescue.
            GateOutcome::Transient { rescued: true, .. } => AutoFreshened::Failed(
                "its refreshed token is in rescue/, not yet in the vault".into(),
            ),
            GateOutcome::Transient { kind, .. } => AutoFreshened::Failed(kind),
            GateOutcome::Systemic(detail) => AutoFreshened::Failed(detail),
            GateOutcome::Owned(OwnedBy::Session) | GateOutcome::Conflict => AutoFreshened::Skip,
        })
    }

    /// §9.3, before a usage strategy plans: quarantines that no longer bind are released
    /// (§7.4, Decision 9), then the managed live account and every candidate are collected on
    /// demand (§8.3, Decision 8). Returns the collector's warnings. Other targets do neither,
    /// and neither does a usage strategy over an unmanaged live login without --force:
    /// planning reports that no-op (§9.2), and the network is not used for it.
    pub(super) fn prepare_usage(
        &self,
        p: &dyn Provider,
        store: &Store,
        req: &SwitchRequest,
    ) -> Result<Vec<String>, EngineError> {
        if !matches!(req.target, SwitchTarget::Usage { .. }) {
            return Ok(Vec::new());
        }
        let (live, live_row) = self.live_row(p, store, &req.provider)?;
        if live.is_some() && live_row.is_none() && !req.force {
            return Ok(Vec::new());
        }
        self.release_unbound_quarantines(&req.provider, req.source)?;
        // §9.3: the live account, then every switchable candidate; a session-owned account is
        // no candidate of any strategy, so it is not collected for one.
        let mut accounts: Vec<AccountId> = live_row.into_iter().map(|r| r.id).collect();
        for row in store.accounts(&req.provider)? {
            if !accounts.contains(&row.id) && self.switch_candidate(p, &row)? {
                accounts.push(row.id);
            }
        }
        // §8.3: a usage failure is never a command error. An interruption is not a usage
        // failure, and ends the command (§14.1).
        match self.collect_usage(CollectMode::OnDemand { accounts }) {
            Ok(report) => Ok(report.warnings),
            Err(e) if e.signal().is_some() => Err(e),
            Err(e) => Ok(vec![format!("usage was not collected: {e}")]),
        }
    }
}
