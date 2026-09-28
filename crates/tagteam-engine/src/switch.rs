use tagteam_core::{
    AccountId, OracleVerdict, OutgoingAction, OutgoingClass, OutgoingFacts, ProviderId,
    decide_outgoing, next_in_rotation,
};
use tagteam_provider::{
    Credential, Identity, LiveAuth, LiveLocks, ProcessStamp, Provenance, Provider, ProviderError,
    Read, StoredLogin, Undo,
};

use crate::account_lock::AccountLock;
use crate::displace::displace;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::lifecycle::KIND_API_KEY;
use crate::oracle::verdict;
use crate::store::{AccountRow, EventRow, JournalRow, Store};

#[derive(Debug, Clone)]
pub enum SwitchTarget {
    Rotation,
    Account(AccountId),
}

#[derive(Debug, Clone)]
pub struct SwitchRequest {
    pub provider: ProviderId,
    pub target: SwitchTarget,
    pub force: bool,
    pub source: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchReason {
    Switched,
    AlreadyActive,
    Activated,
    UnmanagedAccount,
    OnlyOneAccount,
    NoValidTarget,
}

impl SwitchReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SwitchReason::Switched => "switched",
            SwitchReason::AlreadyActive => "already-active",
            SwitchReason::Activated => "activated",
            SwitchReason::UnmanagedAccount => "unmanaged-account",
            SwitchReason::OnlyOneAccount => "only-one-account",
            SwitchReason::NoValidTarget => "no-valid-target",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SwitchOutcome {
    pub switched: bool,
    pub from: Option<AccountRow>,
    pub to: Option<AccountRow>,
    pub strategy: &'static str,
    pub reason: SwitchReason,
    pub message: String,
    pub warnings: Vec<String>,
    pub file_store: bool,
    pub unmanaged_email: Option<String>,
}

/// §9.4 step 1: after this many lock acquisitions that each found the plan outdated, abort.
const ATTEMPTS: usize = 3;

/// §9.4 step 3: never back up an empty value, because a Keychain timeout can look empty.
const EMPTY_LIVE_READ: &str = "the live credential read back empty; a Keychain timeout can look empty, so nothing was changed";

fn unmanaged_message(email: &str) -> String {
    format!("the live login ({email}) is not managed by tagteam; add it first, or use --force")
}

fn login_email(i: &Identity) -> String {
    i.email.clone().unwrap_or_else(|| i.label.clone())
}

fn strategy_of(target: &SwitchTarget) -> &'static str {
    match target {
        SwitchTarget::Rotation => "rotation",
        SwitchTarget::Account(_) => "direct",
    }
}

fn noop(
    strategy: &'static str,
    reason: SwitchReason,
    message: String,
    from: Option<AccountRow>,
    unmanaged_email: Option<String>,
) -> SwitchOutcome {
    SwitchOutcome {
        switched: false,
        from,
        to: None,
        strategy,
        reason,
        message,
        warnings: vec![],
        file_store: false,
        unmanaged_email,
    }
}

/// The live secret on `kind`'s auth axis: the managed key for an API-key account, the
/// credential entry otherwise. A degraded read is not a secret to act on (§4.3).
fn live_secret(kind: &str, auth: &LiveAuth) -> Option<Vec<u8>> {
    if kind == KIND_API_KEY {
        auth.managed_key.as_ref().present().cloned()
    } else {
        auth.credential
            .as_ref()
            .present()
            .filter(|c| c.provenance() == Provenance::Fresh)
            .map(|c| c.bytes().to_vec())
    }
}

/// What a row says about a login, for noticing that it changed: a finished replacement
/// (§12.5) keeps the ID but may change the kind or the identity.
fn login_of(row: Option<&AccountRow>) -> Option<(&AccountId, &str, &str)> {
    row.map(|r| (&r.id, r.kind.as_str(), r.identity_key.as_str()))
}

/// The pre-lock oracle answer (§9.4 "Before locking"). It is about exact live bytes, not
/// about an account: it counts only while the live secret is still those bytes (§9.4 step 4),
/// and is attributed to an account only through `verdict` (§7.6).
struct OracleHint {
    bytes: Vec<u8>,
    resolved: Option<Identity>,
}

/// The oracle's answer about `bytes`, if it was asked about exactly these bytes.
fn answer_for<'h>(hint: Option<&'h OracleHint>, bytes: &[u8]) -> Option<&'h Identity> {
    hint.filter(|h| h.bytes == bytes)
        .and_then(|h| h.resolved.as_ref())
}

/// Where planning gets its oracle answer.
#[allow(clippy::large_enum_variant)]
enum Ask {
    /// Before the mutation lock: the one point where the oracle, and so the network, may be
    /// used (§9.4 "Before locking").
    Oracle,
    /// Under the mutation lock no network is used (§4.3, §9.4), so a re-plan keeps the answer
    /// an earlier attempt got; it still counts only for the bytes it was asked about.
    Reuse(Option<OracleHint>),
}

struct Plan {
    target: AccountRow,
    strategy: &'static str,
    self_switch: bool,
    hint: Option<OracleHint>,
}

#[allow(clippy::large_enum_variant)]
enum Planned {
    Done(SwitchOutcome),
    Go(Plan),
}

/// A bare `switch` (§9.2, §9.3): where the rotation goes, or why it stays.
#[allow(clippy::large_enum_variant)]
enum Rotation {
    To(AccountRow),
    Stay(SwitchReason, &'static str),
}

/// The live login and the rows the plan was made for, re-read under every lock.
struct Locked {
    live_identity: Option<Identity>,
    target: AccountRow,
    outgoing: Option<AccountRow>,
}

/// §9.4 step 10: puts back what the switch wrote, in reverse order, then the journal row's
/// `prior` if it carried one, or no row at all. It runs on an error, and through `Drop` on a
/// panic; it is disarmed once the switch commits or has rolled back. It borrows the live
/// locks its undos run under, so it cannot outlive them (§9.4: "held throughout").
struct Rollback<'a, 'g> {
    undos: Vec<Box<dyn Undo + 'a>>,
    locks: &'a LiveLocks<'g>,
    store: &'a Store,
    provider: &'a ProviderId,
    prior: Option<Box<JournalRow>>,
    /// A provider write that started and has not returned. Such a write puts its own partial
    /// state back when it fails or unwinds, but after a panic the engine cannot see whether
    /// that worked, so the journal row stays for recovery (§9.6) to confirm.
    in_flight: bool,
    armed: bool,
}

impl<'a> Rollback<'a, '_> {
    /// Runs one provider write and keeps its undo.
    fn write(
        &mut self,
        write: impl FnOnce() -> Result<Box<dyn Undo + 'a>, ProviderError>,
    ) -> Result<(), ProviderError> {
        self.in_flight = true;
        let undo = write();
        self.in_flight = false;
        self.undos.push(undo?);
        Ok(())
    }

    fn disarm(mut self) {
        self.armed = false;
    }

    /// Rolls back after `cause`, and says whether that worked.
    fn fail(mut self, cause: EngineError) -> EngineError {
        let partial = match &cause {
            EngineError::Provider(ProviderError::RestoreFailed { restore, .. }) => {
                vec![format!("restoring a partial write: {restore}")]
            }
            _ => vec![],
        };
        let failed = self.roll_back(partial);
        if failed.is_empty() {
            EngineError::RolledBack(cause.to_string())
        } else {
            EngineError::RollbackFailed {
                cause: cause.to_string(),
                failed: failed.join("; "),
            }
        }
    }

    /// Returns what could not be restored, `failed` included. The journal row is put back
    /// only when nothing failed: otherwise it stays for recovery (§9.6).
    fn roll_back(&mut self, mut failed: Vec<String>) -> Vec<String> {
        self.armed = false;
        if self.in_flight {
            failed.push("a live write interrupted part-way".into());
        }
        for undo in self.undos.drain(..).rev() {
            let what = undo.what();
            if let Err(e) = undo.undo(self.locks) {
                failed.push(format!("{what}: {e}"));
            }
        }
        if failed.is_empty() {
            let journal = match &self.prior {
                Some(row) => self.store.insert_journal(row),
                None => self.store.delete_journal(self.provider),
            };
            if let Err(e) = journal {
                failed.push(format!("the switch journal: {e}"));
            }
        }
        failed
    }
}

impl Drop for Rollback<'_, '_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let failed = self.roll_back(Vec::new());
        if failed.is_empty() {
            tracing::warn!("an unwinding switch was rolled back");
        } else {
            tracing::error!(
                "an unwinding switch was not fully rolled back; its journal row stays for recovery: {}",
                failed.join("; ")
            );
        }
    }
}

impl Engine {
    fn has_login(&self, row: &AccountRow) -> bool {
        row.identity_json.is_object()
            && matches!(self.vault.read(&row.id), Read::Present(b) if !b.is_empty())
    }

    /// "Switchable": a vault credential and an identity, and not disabled (§9.3).
    fn is_switchable(&self, row: &AccountRow) -> bool {
        !row.disabled && self.has_login(row)
    }

    fn matches_vault(&self, p: &dyn Provider, row: &AccountRow, live: &[u8]) -> bool {
        match self.vault.read(&row.id) {
            Read::Present(v) => {
                let fp = p.fingerprint(&v);
                v == live || (fp.is_some() && fp == p.fingerprint(live))
            }
            _ => false,
        }
    }

    fn read_live_identity(&self, p: &dyn Provider) -> Result<Option<Identity>, EngineError> {
        match p.live_identity(&self.env) {
            Read::Present(i) => Ok(Some(i)),
            Read::Absent => Ok(None),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
        }
    }

    /// The live identity and the stored account it names, if any.
    fn live_row(
        &self,
        p: &dyn Provider,
        store: &Store,
        provider: &ProviderId,
    ) -> Result<(Option<Identity>, Option<AccountRow>), EngineError> {
        let live = self.read_live_identity(p)?;
        let row = match &live {
            Some(i) => store.find_by_identity_key(provider, p.identity_key(i).as_str())?,
            None => None,
        };
        Ok((live, row))
    }

    /// §9.3 rotation: the next switchable position after the store's active account; on a
    /// fresh machine, the store's active account if switchable, else the first. §9.2: fewer
    /// than two switchable accounts stay put.
    fn rotation(
        &self,
        store: &Store,
        provider: &ProviderId,
        live_row: Option<&AccountRow>,
    ) -> Result<Rotation, EngineError> {
        let accounts = store.accounts(provider)?;
        let slots: Vec<(u32, bool)> = accounts
            .iter()
            .map(|a| (a.position, self.is_switchable(a)))
            .collect();
        if live_row.is_some() && slots.iter().filter(|(_, s)| *s).count() < 2 {
            return Ok(Rotation::Stay(
                SwitchReason::OnlyOneAccount,
                "there is only one switchable account",
            ));
        }
        let stored_active = store
            .active(provider)?
            .and_then(|id| accounts.iter().find(|a| a.id == id));
        let position = match live_row {
            Some(live) => next_in_rotation(&slots, Some(stored_active.unwrap_or(live).position)),
            None => stored_active
                .map(|a| a.position)
                .filter(|pos| slots.contains(&(*pos, true)))
                .or_else(|| next_in_rotation(&slots, None)),
        };
        Ok(
            match position.and_then(|pos| accounts.into_iter().find(|a| a.position == pos)) {
                Some(a) => Rotation::To(a),
                None => Rotation::Stay(SwitchReason::NoValidTarget, "no account can be activated"),
            },
        )
    }

    /// §9.4 "Before locking": asks the oracle about the outgoing live secret when it is not
    /// that account's vault generation. Never called under the mutation lock.
    fn oracle_hint(&self, p: &dyn Provider, out: &AccountRow) -> Option<OracleHint> {
        let bytes = live_secret(&out.kind, &p.read_live_auth(&self.env))?;
        if bytes.is_empty() || self.matches_vault(p, out, &bytes) {
            return None;
        }
        let resolved = self.oracle.resolve(p, &Credential::fresh(bytes.clone()));
        Some(OracleHint { bytes, resolved })
    }

    /// The target and the §9.2 special cases, decided from the current state.
    fn plan(
        &self,
        p: &dyn Provider,
        store: &Store,
        req: &SwitchRequest,
        ask: Ask,
    ) -> Result<Planned, EngineError> {
        let strategy = strategy_of(&req.target);
        let (live, live_row) = self.live_row(p, store, &req.provider)?;
        let unmanaged_email = match (&live, &live_row) {
            (Some(i), None) => Some(login_email(i)),
            _ => None,
        };
        let done = |reason: SwitchReason, message: String| {
            Planned::Done(noop(
                strategy,
                reason,
                message,
                live_row.clone(),
                unmanaged_email.clone(),
            ))
        };
        if let (Some(email), false) = (&unmanaged_email, req.force) {
            return Ok(done(
                SwitchReason::UnmanagedAccount,
                unmanaged_message(email),
            ));
        }
        let target = match &req.target {
            // A switch never crosses providers (§9.3).
            SwitchTarget::Account(id) => store
                .account(id)?
                .filter(|a| a.provider == req.provider)
                .ok_or_else(|| EngineError::NoSuchAccount(id.to_string()))?,
            SwitchTarget::Rotation => {
                match self.rotation(store, &req.provider, live_row.as_ref())? {
                    Rotation::To(a) => a,
                    Rotation::Stay(reason, message) => return Ok(done(reason, message.into())),
                }
            }
        };
        if !self.has_login(&target) {
            return Err(EngineError::InvalidInput(format!(
                "{} cannot be activated: it has no stored credential; log in and run `tagteam add` again",
                target.label
            )));
        }
        // The direct branch displaces whatever is live, so it has nothing to ask about.
        let hint = match ask {
            _ if req.force => None,
            Ask::Oracle => live_row.as_ref().and_then(|out| self.oracle_hint(p, out)),
            Ask::Reuse(hint) => hint,
        };
        let self_switch = live_row.as_ref().is_some_and(|r| r.id == target.id);
        if self_switch && !req.force {
            // A no-op unless the live credential diverged from the vault and the oracle
            // attributed it to this very account; then a full switch reconciles it (§9.2).
            let reconcile =
                live_secret(&target.kind, &p.read_live_auth(&self.env)).is_some_and(|live| {
                    !self.matches_vault(p, &target, &live)
                        && verdict(answer_for(hint.as_ref(), &live), &target)
                            == OracleVerdict::ThisAccount
                });
            if !reconcile {
                return Ok(done(
                    SwitchReason::AlreadyActive,
                    format!("{} is already active", target.label),
                ));
            }
        }
        Ok(Planned::Go(Plan {
            target,
            strategy,
            self_switch,
            hint,
        }))
    }

    /// §5: with no store there is nothing to activate, and nothing is created; an unmanaged
    /// live login is still reported as one (§9.2).
    fn without_store(
        &self,
        p: &dyn Provider,
        req: &SwitchRequest,
    ) -> Result<SwitchOutcome, EngineError> {
        let unmanaged = self.read_live_identity(p)?.as_ref().map(login_email);
        let (reason, message) = match (&unmanaged, req.force) {
            (Some(email), false) => (SwitchReason::UnmanagedAccount, unmanaged_message(email)),
            _ => (
                SwitchReason::NoValidTarget,
                "there are no stored accounts; add one with `tagteam add`".into(),
            ),
        };
        Ok(noop(
            strategy_of(&req.target),
            reason,
            message,
            None,
            unmanaged,
        ))
    }

    /// §9: plan and ask the oracle without any lock; then take the mutation lock once, and
    /// under it the account locks and the live locks, and re-derive every decision. Anything
    /// that moved while this command waited releases every lock but the mutation lock and
    /// plans again, without the network, for at most `ATTEMPTS` lock acquisitions.
    pub fn switch(&self, req: SwitchRequest) -> Result<SwitchOutcome, EngineError> {
        self.refuse_inside_run_shell()?;
        let provider = self.provider(&req.provider)?;
        let p = provider.as_ref();
        if !req.force {
            self.settle_or_refuse(&req.provider)?;
        }
        let Some(store) = self.existing_store()? else {
            return self.without_store(p, &req);
        };
        let mut plan = match self.plan(p, &store, &req, Ask::Oracle)? {
            Planned::Done(outcome) => return Ok(outcome),
            Planned::Go(plan) => plan,
        };
        // Once, before the mutation lock: a test callback here may take that lock itself.
        hooks::point(self, "planned")?;
        let guard = self.mutation_guard()?;
        if !req.force {
            self.refuse_if_interrupted(&req.provider)?;
        }
        for attempt in 1..=ATTEMPTS {
            if attempt > 1 {
                plan = match self.plan(p, &store, &req, Ask::Reuse(plan.hint.take()))? {
                    Planned::Done(outcome) => return Ok(outcome),
                    Planned::Go(plan) => plan,
                };
            }
            let (_, outgoing) = self.live_row(p, &store, &req.provider)?;
            let mut ids = vec![&plan.target.id];
            if let Some(o) = &outgoing {
                ids.push(&o.id);
            }
            let accounts = self.lock_accounts(&ids)?;
            let locks = p.lock_live(&self.env, &guard)?;
            if let Some(locked) = self.rederive(p, &store, &req, &plan, outgoing.as_ref())? {
                return self.transact(p, &store, &plan, locked, &accounts, &locks, &req);
            }
            // `locks`, then `accounts`, are released here; the mutation lock is kept.
        }
        Err(EngineError::LiveMoved)
    }

    /// §9.4 step 1: the live account, the target and the self-switch decision, all re-read
    /// under the locks. `None` when any of them moved, or the account locks held are no longer
    /// the outgoing account's.
    fn rederive(
        &self,
        p: &dyn Provider,
        store: &Store,
        req: &SwitchRequest,
        plan: &Plan,
        outgoing: Option<&AccountRow>,
    ) -> Result<Option<Locked>, EngineError> {
        let (live_identity, again) = self.live_row(p, store, &req.provider)?;
        let target = store
            .account(&plan.target.id)?
            .ok_or_else(|| EngineError::NoSuchAccount(plan.target.id.to_string()))?;
        let self_switch = again.as_ref().is_some_and(|r| r.id == target.id);
        let same_pick = match req.target {
            SwitchTarget::Account(_) => true,
            // The whole rotation decision is recomputed from the current roster and anchor
            // (§9.2, §9.3), including the fewer-than-two case.
            SwitchTarget::Rotation => matches!(
                self.rotation(store, &req.provider, again.as_ref())?,
                Rotation::To(r) if r.id == target.id
            ),
        };
        // Account-lock acquisition may have finished a pending replacement (§12.5), changing
        // the outgoing account's kind or identity: compare the rows, not just their IDs.
        let unchanged = login_of(again.as_ref()) == login_of(outgoing)
            && target.kind == plan.target.kind
            && target.identity_key == plan.target.identity_key
            && self_switch == plan.self_switch
            && same_pick;
        Ok(unchanged.then_some(Locked {
            live_identity,
            target,
            outgoing: again,
        }))
    }

    fn read_target(&self, target: &AccountRow) -> Result<Vec<u8>, EngineError> {
        match self.vault.read(&target.id) {
            Read::Present(b) if !b.is_empty() => Ok(b),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
            _ => Err(EngineError::InvalidInput(format!(
                "{} has no stored credential",
                target.label
            ))),
        }
    }

    /// Steps 2–10, under every lock. Everything here uses the rows read under the locks.
    #[allow(clippy::too_many_arguments)]
    fn transact(
        &self,
        p: &dyn Provider,
        store: &Store,
        plan: &Plan,
        locked: Locked,
        account_locks: &[AccountLock],
        locks: &LiveLocks<'_>,
        req: &SwitchRequest,
    ) -> Result<SwitchOutcome, EngineError> {
        let Locked {
            live_identity,
            target,
            outgoing,
        } = locked;
        let provider = &req.provider;
        if live_identity.is_some() && outgoing.is_none() && !req.force {
            return Err(EngineError::LiveMoved); // became unmanaged since planning
        }
        let target_identity = p.parse_identity(&target.identity_json)?;
        let live = p.read_live_auth(&self.env);

        // Step 3 read rules, with or without --force: never overwrite what could not be read.
        let live_cred: Option<Credential> = match &live.credential {
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e.clone())),
            Read::Present(c) if c.provenance() == Provenance::Degraded => {
                return Err(EngineError::DegradedRead);
            }
            Read::Present(c) if c.is_empty() => {
                return Err(EngineError::InvalidInput(EMPTY_LIVE_READ.into()));
            }
            Read::Present(c) => Some(c.clone()),
            Read::Absent => None,
        };
        if let Read::Unreadable(e) = &live.managed_key {
            return Err(EngineError::Unreadable(e.clone()));
        }

        let mut warnings = Vec::new();
        let target_secret = match outgoing.as_ref().filter(|_| !req.force) {
            // Step 2, the direct branch: displace whatever is live unless it is byte-identical
            // to the target.
            None => {
                let secret = self.read_target(&target)?;
                let reason = if req.force {
                    "forced-activation"
                } else {
                    "displaced-live-login"
                };
                let live_bytes = [
                    live_cred.as_ref().map(|c| c.bytes().to_vec()),
                    live.managed_key.as_ref().present().cloned(),
                ];
                for bytes in live_bytes.into_iter().flatten().filter(|b| *b != secret) {
                    let identity = live_identity.as_ref().map(|i| &i.raw);
                    match displace(
                        self,
                        provider,
                        &bytes,
                        p.fingerprint(&bytes).as_ref(),
                        reason,
                        identity,
                    ) {
                        Ok(id) => warnings.push(format!(
                            "the previous live credential was saved as displaced/{id}"
                        )),
                        Err(e) if req.force => warnings
                            .push(format!("could not save the previous live credential: {e}")),
                        Err(e) => return Err(e),
                    }
                }
                secret
            }
            Some(out) => {
                self.settle_outgoing(
                    p,
                    store,
                    out,
                    &live,
                    plan.hint.as_ref(),
                    account_locks,
                    live_identity.as_ref(),
                    &mut warnings,
                )?;
                // Read only now: settling a self-switch may have captured a newer live
                // generation into this very account, and that is the one to activate.
                self.read_target(&target)?
            }
        };

        // Step 6.
        let from_secret = outgoing
            .as_ref()
            .and_then(|o| live_secret(&o.kind, &live))
            .or_else(|| live_cred.as_ref().map(|c| c.bytes().to_vec()));
        // A forced switch may supersede an undecidable row; it is carried along, so a forced
        // switch that never lands puts it back instead of forgetting it (§9.6).
        let prior = if req.force {
            store.journal(provider)?.map(Box::new)
        } else {
            None
        };
        store.insert_journal(&JournalRow {
            provider: provider.clone(),
            holder: ProcessStamp::current()?,
            from_id: outgoing.as_ref().map(|o| o.id.clone()),
            to_id: target.id.clone(),
            from_fp: from_secret
                .as_deref()
                .and_then(|b| p.fingerprint(b))
                .map(|f| f.as_str().to_owned()),
            from_identity: live_identity.as_ref().map(|i| i.raw.clone()),
            to_fp: p
                .fingerprint(&target_secret)
                .map(|f| f.as_str().to_owned())
                .unwrap_or_default(),
            started_at: self.now_ms(),
            prior: prior.clone(),
        })?;

        // Steps 7–10.
        let target_login = StoredLogin {
            kind: target.kind.clone(),
            secret: target_secret,
            identity: target_identity,
        };
        let mut tx = Rollback {
            undos: Vec::new(),
            locks,
            store,
            provider,
            prior,
            in_flight: false,
            armed: true,
        };
        match self.apply(
            p,
            &target,
            &target_login,
            &live,
            outgoing.as_ref(),
            req,
            &mut tx,
        ) {
            Ok(()) => tx.disarm(),
            Err(cause) => return Err(tx.fail(cause)),
        }

        let (reason, switched) = if plan.self_switch {
            (SwitchReason::Activated, true)
        } else {
            (
                SwitchReason::Switched,
                outgoing.as_ref().map(|o| &o.id) != Some(&target.id),
            )
        };
        let message = format!(
            "Switched to {}",
            target.alias.as_deref().unwrap_or(&target.label)
        );
        Ok(SwitchOutcome {
            switched,
            from: outgoing,
            to: Some(target),
            strategy: plan.strategy,
            reason,
            message,
            warnings,
            file_store: p.uses_file_store(&self.env),
            unmanaged_email: None,
        })
    }

    /// Step 4: classify the outgoing credential and act on it.
    #[allow(clippy::too_many_arguments)]
    fn settle_outgoing(
        &self,
        p: &dyn Provider,
        store: &Store,
        out: &AccountRow,
        live: &LiveAuth,
        hint: Option<&OracleHint>,
        account_locks: &[AccountLock],
        live_identity: Option<&Identity>,
        warnings: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        let Some(bytes) = live_secret(&out.kind, live) else {
            return Ok(());
        };
        if bytes.is_empty() {
            // Step 3's rule, on the managed-key axis too: capturing an empty read would leave
            // this account with nothing to activate.
            return Err(EngineError::InvalidInput(EMPTY_LIVE_READ.into()));
        }
        let vault = match self.vault.read(&out.id) {
            Read::Present(v) => Some(v),
            Read::Absent => None,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        let fp_live = p.fingerprint(&bytes);
        let resolved = answer_for(hint, &bytes);
        let facts = OutgoingFacts {
            bytes_equal_vault: vault.as_deref() == Some(bytes.as_slice()),
            fp_equal_vault: fp_live.is_some()
                && vault.as_deref().and_then(|v| p.fingerprint(v)) == fp_live,
            wiped: p.is_wiped(&bytes),
            oracle: verdict(resolved, out),
            lacks_refresh_over_complete: !p.has_refresh_token(&bytes)
                && vault.as_deref().is_some_and(|v| p.has_refresh_token(v)),
        };
        let (class, action) = decide_outgoing(&facts);
        match action {
            OutgoingAction::Nothing => {}
            OutgoingAction::CaptureToVault { backfill_uuid } => {
                let lock = account_locks
                    .iter()
                    .find(|l| l.id() == &out.id)
                    .expect("the outgoing account is locked");
                self.vault.store(lock, &bytes, &|b| p.fingerprint(b))?;
                let identity = p.parse_identity(&out.identity_json)?;
                store.update_login(
                    &out.id,
                    &out.identity_key,
                    &identity,
                    &out.kind,
                    p.login_expires_at(&bytes),
                )?;
                if backfill_uuid {
                    if let Some(uuid) = resolved.and_then(|i| i.account_uuid.as_deref()) {
                        store.backfill_account_uuid(&out.id, uuid)?;
                    }
                }
                if class == OutgoingClass::Unresolved {
                    tracing::warn!(
                        position = out.position,
                        "captured an unverified live credential into the vault; .prev keeps the previous generation"
                    );
                }
            }
            OutgoingAction::Displace => {
                let identity = resolved.or(live_identity).map(|i| &i.raw);
                let id = displace(
                    self,
                    &out.provider,
                    &bytes,
                    fp_live.as_ref(),
                    "displaced-live-login",
                    identity,
                )?;
                warnings.push(if class == OutgoingClass::Foreign {
                    format!(
                        "the live credential did not belong to position {}; it was saved as displaced/{id}",
                        out.position
                    )
                } else {
                    format!(
                        "the live credential had no refresh token, so it did not replace position {}'s stored one; it was saved as displaced/{id}",
                        out.position
                    )
                });
            }
        }
        Ok(())
    }

    /// Steps 7–9, keeping an undo for each write.
    #[allow(clippy::too_many_arguments)]
    fn apply<'a>(
        &self,
        p: &dyn Provider,
        target: &AccountRow,
        target_login: &StoredLogin,
        live: &LiveAuth,
        outgoing: Option<&AccountRow>,
        req: &SwitchRequest,
        tx: &mut Rollback<'a, '_>,
    ) -> Result<(), EngineError> {
        let locks = tx.locks;
        hooks::point(self, "after-journal")?;
        tx.write(|| p.write_credential(&self.env, locks, target_login, live))?;
        hooks::point(self, "after-credential")?;
        tx.write(|| p.write_identity(&self.env, locks, Some(&target_login.identity)))?;
        hooks::point(self, "after-identity")?;
        tx.store.commit_switch(
            &req.provider,
            &target.id,
            &EventRow {
                at: self.now_ms(),
                provider: req.provider.clone(),
                kind: "switch".into(),
                from_id: outgoing.map(|o| o.id.clone()),
                to_id: Some(target.id.clone()),
                trigger: Some(
                    if req.source == "auto" {
                        "auto"
                    } else {
                        "manual"
                    }
                    .into(),
                ),
                source: req.source.into(),
                detail: None,
            },
        )?;
        Ok(())
    }
}
