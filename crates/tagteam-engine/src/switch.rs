use tagteam_core::{
    AccountId, OracleVerdict, OutgoingAction, OutgoingClass, OutgoingFacts, ProviderId,
    decide_outgoing, next_in_rotation,
};
use tagteam_provider::{
    Credential, Identity, LiveAuth, LiveLocks, ProcessStamp, Provenance, Provider, ProviderError,
    Read, ReadError, SecretStore, StoredLogin, Undo,
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
    /// Where this switch's credential write put the secret; `None` when it wrote none.
    pub stored_in: Option<SecretStore>,
    pub unmanaged_email: Option<String>,
}

/// §9.4 step 1: after this many lock acquisitions that each found the plan outdated, abort.
const ATTEMPTS: usize = 3;

/// §9.4 step 3: never back up an empty value, because a Keychain timeout can look empty. It is
/// reported as unreadable, on either axis: the value could not be read with confidence.
fn empty_live_read(what: &str) -> EngineError {
    EngineError::Unreadable(ReadError::new(
        what,
        "it read back empty, which a Keychain timeout can cause; the switch was not attempted",
    ))
}

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
        stored_in: None,
        unmanaged_email,
    }
}

/// The two auth axes a live login can be on (§9.4 step 7): the credential entry, or the
/// managed API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Axis {
    Entry,
    ManagedKey,
}

impl Axis {
    pub(crate) const BOTH: [Axis; 2] = [Axis::Entry, Axis::ManagedKey];

    pub(crate) fn of(kind: &str) -> Self {
        if kind == KIND_API_KEY {
            Axis::ManagedKey
        } else {
            Axis::Entry
        }
    }

    pub(crate) fn other(self) -> Self {
        match self {
            Axis::Entry => Axis::ManagedKey,
            Axis::ManagedKey => Axis::Entry,
        }
    }

    /// The live secret on this axis. A degraded read is not a secret to act on (§4.3).
    pub(crate) fn live_secret(self, auth: &LiveAuth) -> Option<Vec<u8>> {
        match self {
            Axis::ManagedKey => auth.managed_key.as_ref().present().cloned(),
            Axis::Entry => auth
                .credential
                .as_ref()
                .present()
                .filter(|c| c.provenance() == Provenance::Fresh)
                .map(|c| c.bytes().to_vec()),
        }
    }
}

/// §9.4 step 3, with or without --force: what could not be read fresh is never overwritten,
/// on either axis. An empty value is never backed up, because a Keychain timeout can look
/// empty; a degraded entry may hide a newer generation.
pub(crate) fn refuse_unsafe_live_reads(live: &LiveAuth) -> Result<(), EngineError> {
    match &live.credential {
        Read::Unreadable(e) => return Err(EngineError::Unreadable(e.clone())),
        Read::Present(c) if c.provenance() == Provenance::Degraded => {
            return Err(EngineError::DegradedRead);
        }
        Read::Present(c) if c.is_empty() => return Err(empty_live_read("the live credential")),
        _ => {}
    }
    match &live.managed_key {
        Read::Unreadable(e) => Err(EngineError::Unreadable(e.clone())),
        Read::Present(k) if k.is_empty() => Err(empty_live_read("the live API key")),
        _ => Ok(()),
    }
}

/// Two secrets of one generation: equal bytes, or the same fingerprint.
fn same_generation(p: &dyn Provider, a: &[u8], b: &[u8]) -> bool {
    let fp = p.fingerprint(a);
    a == b || (fp.is_some() && fp == p.fingerprint(b))
}

/// B.5: a failed displacement aborts the switch, except under --force, where it is a warning.
fn unless_forced(
    saved: Result<(), EngineError>,
    force: bool,
    warnings: &mut Vec<String>,
) -> Result<(), EngineError> {
    match saved {
        Err(e) if force => {
            warnings.push(format!("could not save the previous live credential: {e}"));
            Ok(())
        }
        other => other,
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
pub(crate) struct OracleHint {
    pub(crate) bytes: Vec<u8>,
    pub(crate) resolved: Option<Identity>,
}

/// The oracle's answer about `bytes`, if it was asked about exactly these bytes.
pub(crate) fn answer_for<'h>(hint: Option<&'h OracleHint>, bytes: &[u8]) -> Option<&'h Identity> {
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

/// What `rederive` found under the locks.
enum Rederived {
    /// Nothing the plan rests on moved.
    Go(Locked),
    /// Something moved: plan again.
    Replan,
    /// Another process already did this command's work.
    Done(SwitchOutcome),
}

fn already_active(target: &AccountRow) -> String {
    format!("{} is already active", target.label)
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
    /// Runs one provider write and keeps its undo, handing back what else the write reported.
    fn write<T>(
        &mut self,
        write: impl FnOnce() -> Result<(Box<dyn Undo + 'a>, T), ProviderError>,
    ) -> Result<T, ProviderError> {
        self.in_flight = true;
        let written = write();
        self.in_flight = false;
        let (undo, value) = written?;
        self.undos.push(undo);
        Ok(value)
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
    /// An identity and a non-empty vault credential. A vault that cannot be read is reported,
    /// never taken for a missing credential (§4.3).
    fn has_login(&self, row: &AccountRow) -> Result<bool, EngineError> {
        if !row.identity_json.is_object() {
            return Ok(false);
        }
        match self.vault.read(&row.id) {
            Read::Present(b) => Ok(!b.is_empty()),
            Read::Absent => Ok(false),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
        }
    }

    /// "Switchable": a vault credential and an identity, and not disabled (§9.3).
    fn is_switchable(&self, row: &AccountRow) -> Result<bool, EngineError> {
        Ok(!row.disabled && self.has_login(row)?)
    }

    fn matches_vault(&self, p: &dyn Provider, row: &AccountRow, live: &[u8]) -> bool {
        match self.vault.read(&row.id) {
            Read::Present(v) => same_generation(p, &v, live),
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

    /// §9.3 rotation: the next switchable position after the live account when it is managed
    /// (`live_row`), even if the store's active account disagrees (§6.1: the live identity
    /// wins). With no live login, or an unmanaged one, the store's active account if
    /// switchable, else the first. §9.2: fewer than two switchable accounts stay put.
    fn rotation(
        &self,
        store: &Store,
        provider: &ProviderId,
        live_row: Option<&AccountRow>,
    ) -> Result<Rotation, EngineError> {
        let accounts = store.accounts(provider)?;
        let slots = accounts
            .iter()
            .map(|a| Ok((a.position, self.is_switchable(a)?)))
            .collect::<Result<Vec<(u32, bool)>, EngineError>>()?;
        if live_row.is_some() && slots.iter().filter(|(_, s)| *s).count() < 2 {
            return Ok(Rotation::Stay(
                SwitchReason::OnlyOneAccount,
                "there is only one switchable account",
            ));
        }
        let position = match live_row {
            Some(live) => next_in_rotation(&slots, Some(live.position)),
            None => store
                .active(provider)?
                .and_then(|id| accounts.iter().find(|a| a.id == id))
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
        let bytes = Axis::of(&out.kind).live_secret(&p.read_live_auth(&self.env))?;
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
        if !self.has_login(&target)? {
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
            let reconcile = Axis::of(&target.kind)
                .live_secret(&p.read_live_auth(&self.env))
                .is_some_and(|live| {
                    !self.matches_vault(p, &target, &live)
                        && verdict(answer_for(hint.as_ref(), &live), &target)
                            == OracleVerdict::ThisAccount
                });
            if !reconcile {
                return Ok(done(SwitchReason::AlreadyActive, already_active(&target)));
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
        let guard = if req.force {
            self.mutation_guard()?
        } else {
            self.guard_or_refuse(&req.provider)?
        };
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
            // Re-decided here rather than under CC's locks: a rotation reads every account's
            // vault entry, which must not lengthen CC's wait. The mutation lock keeps the
            // roster still, and `rederive` checks under CC's locks that the live row it is
            // anchored on has not moved.
            let rotation = match req.target {
                SwitchTarget::Rotation => {
                    Some(self.rotation(&store, &req.provider, outgoing.as_ref())?)
                }
                SwitchTarget::Account(_) => None,
            };
            let locks = p.lock_live(&self.env, &guard)?;
            match self.rederive(p, &store, &req, &plan, outgoing.as_ref(), rotation)? {
                Rederived::Go(locked) => {
                    return self.transact(p, &store, &plan, locked, &accounts, &locks, &req);
                }
                Rederived::Done(outcome) => return Ok(outcome),
                Rederived::Replan => {}
            }
            // `locks`, then `accounts`, are released here; the mutation lock is kept.
        }
        Err(EngineError::LiveMoved)
    }

    /// §9.4 step 1: the live account, the target and the self-switch decision, all re-read
    /// under the locks. `Replan` when any of them moved, or the account locks held are no
    /// longer the outgoing account's; planning again then reaches the right outcome.
    fn rederive(
        &self,
        p: &dyn Provider,
        store: &Store,
        req: &SwitchRequest,
        plan: &Plan,
        outgoing: Option<&AccountRow>,
        rotation: Option<Rotation>,
    ) -> Result<Rederived, EngineError> {
        let (live_identity, again) = self.live_row(p, store, &req.provider)?;
        // A login that became unmanaged is §9.2's no-op; a target removed meanwhile is
        // replaced (rotation) or reported (direct).
        if live_identity.is_some() && again.is_none() && !req.force {
            return Ok(Rederived::Replan);
        }
        let Some(target) = store.account(&plan.target.id)? else {
            return Ok(Rederived::Replan);
        };
        let self_switch = again.as_ref().is_some_and(|r| r.id == target.id);
        // Review Focus 3: another process landed exactly this rotation's target while this one
        // waited for the mutation lock (a double-fired `switch`). That was this command's work;
        // rotating on from there would switch twice. A direct target needs no such rule: planning
        // again finds the self-switch no-op.
        if matches!(req.target, SwitchTarget::Rotation) && self_switch && !plan.self_switch {
            return Ok(Rederived::Done(noop(
                plan.strategy,
                SwitchReason::AlreadyActive,
                already_active(&target),
                again,
                None,
            )));
        }
        // The whole rotation decision, recomputed from the current roster and anchor (§9.2,
        // §9.3), including the fewer-than-two case. Its anchor is `outgoing`, which is `again`
        // when anything proceeds.
        let same_pick = match rotation {
            None => true,
            Some(Rotation::To(r)) => r.id == target.id,
            Some(Rotation::Stay(..)) => false,
        };
        // Account-lock acquisition may have finished a pending replacement (§12.5), changing
        // the outgoing account's kind or identity: compare the rows, not just their IDs.
        let unchanged = login_of(again.as_ref()) == login_of(outgoing)
            && target.kind == plan.target.kind
            && target.identity_key == plan.target.identity_key
            && self_switch == plan.self_switch
            && same_pick;
        Ok(if unchanged {
            Rederived::Go(Locked {
                live_identity,
                target,
                outgoing: again,
            })
        } else {
            Rederived::Replan
        })
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
        let target_identity = p.parse_identity(&target.identity_json)?;
        let live = p.read_live_auth(&self.env);
        refuse_unsafe_live_reads(&live)?;
        // Step 3 holds for the entry the effective one shadows too, since step 7 overwrites it.
        if let Read::Unreadable(e) = &live.shadowed {
            return Err(EngineError::Unreadable(e.clone()));
        }

        let mut warnings = Vec::new();
        let target_secret = match outgoing.as_ref().filter(|_| !req.force) {
            // Step 2, the direct branch: displace whatever is live on either axis unless it is
            // byte-identical to the target.
            None => {
                let secret = self.read_target(&target)?;
                let reason = if req.force {
                    "forced-activation"
                } else {
                    "displaced-live-login"
                };
                for axis in Axis::BOTH {
                    if let Some(bytes) = axis.live_secret(&live).filter(|b| *b != secret) {
                        let saved = self.displace_live(
                            p,
                            provider,
                            &bytes,
                            reason,
                            live_identity.as_ref(),
                            &mut warnings,
                        );
                        unless_forced(saved, req.force, &mut warnings)?;
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
                let secret = self.read_target(&target)?;
                // Step 7 clears or overwrites the axis the outgoing account is not on, which
                // step 4 never classified. Never forced here, so a failed displacement aborts.
                self.displace_unless_target(
                    p,
                    provider,
                    Axis::of(&out.kind).other(),
                    &live,
                    &secret,
                    live_identity.as_ref(),
                    &mut warnings,
                )?;
                secret
            }
        };
        let saved = self.displace_shadowed(
            p,
            provider,
            &live,
            &target_secret,
            live_identity.as_ref(),
            &mut warnings,
        );
        unless_forced(saved, req.force, &mut warnings)?;

        // Step 6.
        let from_secret = outgoing
            .as_ref()
            .and_then(|o| Axis::of(&o.kind).live_secret(&live))
            .or_else(|| Axis::Entry.live_secret(&live));
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
        let stored_in = match self.apply(
            p,
            &target,
            &target_login,
            &live,
            outgoing.as_ref(),
            req,
            &mut tx,
        ) {
            Ok(stored_in) => {
                tx.disarm();
                stored_in
            }
            Err(cause) => return Err(tx.fail(cause)),
        };

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
            stored_in: Some(stored_in),
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
        let Some(bytes) = Axis::of(&out.kind).live_secret(live) else {
            return Ok(());
        };
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
            tokenless: fp_live.is_none(),
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
                        "the live credential did not belong to position {}; it was saved as displaced/{id}.json",
                        out.position
                    )
                } else {
                    format!(
                        "the live credential had no refresh token, so it did not replace position {}'s stored one; it was saved as displaced/{id}.json",
                        out.position
                    )
                });
            }
        }
        Ok(())
    }

    /// Saves a live secret the switch is about to overwrite or clear (§6.3, B.5), unless it
    /// carries nothing account-scoped: an entry holding only machine-shared keys loses nothing,
    /// since those are carried over. The error is the failed save; whether it aborts is the
    /// caller's call (B.5: it does, except under --force).
    fn displace_live(
        &self,
        p: &dyn Provider,
        provider: &ProviderId,
        bytes: &[u8],
        reason: &str,
        live_identity: Option<&Identity>,
        warnings: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        let Some(fp) = p.fingerprint(bytes) else {
            return Ok(());
        };
        let id = displace(
            self,
            provider,
            bytes,
            Some(&fp),
            reason,
            live_identity.map(|i| &i.raw),
        )?;
        warnings.push(format!(
            "the previous live credential was saved as displaced/{id}.json"
        ));
        Ok(())
    }

    /// §9.4 step 7's off-axis rule: an account-scoped secret live on `axis`, which is about to
    /// be cleared or overwritten, is saved first unless it is the target's generation
    /// (`target_secret`). Shared by the switch and by §9.6 recovery finishing forward, so a
    /// stray secret written between the journal row and a crash is never lost either way.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn displace_unless_target(
        &self,
        p: &dyn Provider,
        provider: &ProviderId,
        axis: Axis,
        live: &LiveAuth,
        target_secret: &[u8],
        live_identity: Option<&Identity>,
        warnings: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        match axis
            .live_secret(live)
            .filter(|b| !same_generation(p, b, target_secret))
        {
            Some(bytes) => self.displace_live(
                p,
                provider,
                &bytes,
                "displaced-live-login",
                live_identity,
                warnings,
            ),
            None => Ok(()),
        }
    }

    /// Step 7 overwrites the entry the effective credential shadows as well
    /// (`LiveAuth::shadowed`), and nothing restores it once the switch commits. So a generation
    /// there that is neither the effective credential's nor the target's is saved first, like
    /// the off-axis rule ("never lose a secret"); one with nothing account-scoped in it is not
    /// (`displace_live`).
    fn displace_shadowed(
        &self,
        p: &dyn Provider,
        provider: &ProviderId,
        live: &LiveAuth,
        target_secret: &[u8],
        live_identity: Option<&Identity>,
        warnings: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        let Read::Present(bytes) = &live.shadowed else {
            return Ok(());
        };
        let known = |other: &[u8]| same_generation(p, bytes, other);
        if bytes.is_empty()
            || known(target_secret)
            || Axis::Entry.live_secret(live).is_some_and(|e| known(&e))
        {
            return Ok(());
        }
        self.displace_live(
            p,
            provider,
            bytes,
            "displaced-live-login",
            live_identity,
            warnings,
        )
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
    ) -> Result<SecretStore, EngineError> {
        let locks = tx.locks;
        hooks::point(self, "after-journal")?;
        let stored_in = tx.write(|| {
            p.write_credential(&self.env, locks, target_login, live)
                .map(|w| (w.undo, w.stored_in))
        })?;
        hooks::point(self, "after-credential")?;
        tx.write(|| {
            p.write_identity(&self.env, locks, Some(&target_login.identity))
                .map(|undo| (undo, ()))
        })?;
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
        Ok(stored_in)
    }
}
