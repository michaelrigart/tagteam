use tagteam_core::{
    AccountId, OracleVerdict, OutgoingAction, OutgoingClass, OutgoingFacts, ProviderId,
    decide_outgoing, rotation_order,
};
use tagteam_provider::{
    BeforeFallback, Credential, DoomedEntry, Identity, LiveAuth, LiveChange, LiveLocks,
    ProcessStamp, Provenance, Provider, ProviderError, Read, ReadError, SecretStore, StoredLogin,
    Undo,
};

use crate::account_lock::AccountLock;
use crate::displace::displace;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::oracle::verdict;
use crate::refresh::{GateOutcome, OwnedBy};
use crate::rescue::RescueFile;
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

/// §7.2: a target whose access token expires within this many milliseconds is refreshed
/// before it is activated. Twice CC's own 5-minute buffer.
const FRESHEN_WINDOW_MS: i64 = 10 * 60 * 1000;

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

    /// The axis `p` keeps a credential of `kind` on (§9.4 step 7).
    pub(crate) fn of(p: &dyn Provider, kind: &str) -> Self {
        if p.kind_traits(kind).managed_key_axis {
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

/// Generations already kept somewhere, by fingerprint (§9.4 step 7): destroying a live entry
/// of one loses nothing. An entry with no fingerprint holds nothing account-scoped to lose.
#[derive(Default)]
pub(crate) struct Held(Vec<String>);

impl Held {
    /// Records `fp`; false when it was already held.
    pub(crate) fn insert(&mut self, fp: &str) -> bool {
        if self.0.iter().any(|h| h == fp) {
            return false;
        }
        self.0.push(fp.to_owned());
        true
    }

    /// Records the generation of `secret`, if it has one.
    pub(crate) fn hold(&mut self, p: &dyn Provider, secret: &[u8]) {
        if let Some(fp) = p.fingerprint(secret) {
            self.insert(fp.as_str());
        }
    }
}

/// §9.4 step 3, for every entry a change destroys: one that could not be read is never
/// overwritten or deleted, with or without --force.
pub(crate) fn refuse_unreadable(doomed: &[DoomedEntry]) -> Result<(), EngineError> {
    match doomed.iter().find_map(|d| match &d.bytes {
        Read::Unreadable(e) => Some(e),
        _ => None,
    }) {
        Some(e) => Err(EngineError::Unreadable(e.clone())),
        None => Ok(()),
    }
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

/// A rotation candidate by the store alone (§9.3 "Reading the vault lazily"): enabled, not
/// quarantined, and with an identity. Whether its vault holds a credential is read only when
/// the walk reaches it.
fn is_candidate(row: &AccountRow) -> bool {
    !row.disabled && row.quarantine_reason.is_none() && row.identity_json.is_object()
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
    /// A rotation's accounts the walk read and passed over before its pick (§9.3). Empty for a
    /// direct target.
    walked: Vec<AccountId>,
    /// What freshening the target decided to tell the user (§7.2), carried into the outcome.
    warnings: Vec<String>,
}

#[allow(clippy::large_enum_variant)]
enum Planned {
    Done(SwitchOutcome),
    Go(Plan),
}

/// A bare `switch` (§9.2, §9.3): where the rotation goes, or why it stays.
#[allow(clippy::large_enum_variant)]
enum Rotation {
    /// The pick, and the accounts the walk read and passed over before it.
    To(AccountRow, Vec<AccountId>),
    Stay(SwitchReason, &'static str),
}

/// The live login and the rows the plan was made for, re-read under every lock.
struct Locked {
    live_identity: Option<Identity>,
    target: AccountRow,
    outgoing: Option<AccountRow>,
    /// What the locked re-read decided to tell the user (§9.4 step 1).
    warnings: Vec<String>,
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

/// What freshening a plan's target decided (§7.2, the manual-switch table).
enum Freshened {
    /// Go ahead, with these warnings for the outcome.
    Go(Vec<String>),
    /// A rotation's pick turned out dead and is quarantined now: plan again; the walk skips
    /// it (§9.3).
    Replan,
}

fn needs_relogin(target: &AccountRow) -> EngineError {
    EngineError::NeedsRelogin {
        position: target.position,
        label: target.label.clone(),
    }
}

fn cannot_refresh(label: &str, why: &str) -> String {
    format!(
        "could not refresh {label} first ({why}); Claude Code will refresh it when it is online"
    )
}

fn works_until_expiry(target: &AccountRow) -> String {
    format!(
        "{} (position {}) needs a new login: its stored refresh token can no longer be used; it works only until its current access token expires",
        target.label, target.position
    )
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
    /// Whether `row`'s vault holds a credential. A vault that cannot be read is reported,
    /// naming the account, never taken for a missing credential (§4.3, §9.3).
    fn vault_holds_login(&self, row: &AccountRow) -> Result<bool, EngineError> {
        match self.vault.read(&row.id) {
            Read::Present(b) => Ok(!b.is_empty()),
            Read::Absent => Ok(false),
            Read::Unreadable(source) => Err(EngineError::UnreadableAccount {
                position: row.position,
                label: row.label.clone(),
                source,
            }),
        }
    }

    /// An identity and a non-empty vault credential.
    fn has_login(&self, row: &AccountRow) -> Result<bool, EngineError> {
        Ok(row.identity_json.is_object() && self.vault_holds_login(row)?)
    }

    fn matches_vault(&self, p: &dyn Provider, row: &AccountRow, live: &[u8]) -> bool {
        match self.vault.read(&row.id) {
            Read::Present(v) => same_generation(p, &v, live),
            _ => false,
        }
    }

    /// The live identity, `None` when there is no live login; an unreadable one is an error,
    /// never taken for an absent one.
    pub(crate) fn read_live_identity(
        &self,
        p: &dyn Provider,
    ) -> Result<Option<Identity>, EngineError> {
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

    /// §9.3: the rotation's candidates in walk order, from the store alone. `None` when the
    /// live anchor is managed and fewer than two accounts qualify (§9.2). The walk starts after
    /// the live account when it is managed (`live_row`), even if the store's active account
    /// disagrees (§6.1: the live identity wins). With no live login, or an unmanaged one, it
    /// starts at the store's active account if that is a candidate, then goes on from the first
    /// position.
    fn candidate_order(
        &self,
        store: &Store,
        provider: &ProviderId,
        live_row: Option<&AccountRow>,
    ) -> Result<Option<Vec<AccountRow>>, EngineError> {
        let accounts = store.accounts(provider)?;
        let candidates: Vec<AccountRow> = accounts.into_iter().filter(is_candidate).collect();
        if live_row.is_some() && candidates.len() < 2 {
            return Ok(None);
        }
        let positions: Vec<u32> = candidates.iter().map(|a| a.position).collect();
        let order: Vec<u32> = match live_row {
            Some(live) => rotation_order(&positions, Some(live.position)),
            None => {
                let rest = rotation_order(&positions, None);
                let active = store
                    .active(provider)?
                    .and_then(|id| candidates.iter().find(|a| a.id == id))
                    .map(|a| a.position);
                match active {
                    Some(first) => std::iter::once(first)
                        .chain(rest.into_iter().filter(|p| *p != first))
                        .collect(),
                    None => rest,
                }
            }
        };
        Ok(Some(
            order
                .into_iter()
                .filter_map(|pos| candidates.iter().find(|a| a.position == pos).cloned())
                .collect(),
        ))
    }

    /// §9.3 rotation, reading the vault lazily.
    ///
    /// - The candidates are counted from the store: with a managed live anchor and fewer than
    ///   two of them, it stays put (§9.2).
    /// - Each vault is read only when the walk reaches it, and the walk stops at the first one
    ///   that holds a credential. An unreadable one before that could have been the pick, so
    ///   it fails naming the account; no account after the pick is ever read.
    fn rotation(
        &self,
        store: &Store,
        provider: &ProviderId,
        live_row: Option<&AccountRow>,
    ) -> Result<Rotation, EngineError> {
        const ONLY_ONE: &str = "there is only one switchable account";
        let Some(order) = self.candidate_order(store, provider, live_row)? else {
            return Ok(Rotation::Stay(SwitchReason::OnlyOneAccount, ONLY_ONE));
        };
        let mut walked = Vec::new();
        for row in order {
            if self.vault_holds_login(&row)? {
                return Ok(Rotation::To(row, walked));
            }
            walked.push(row.id);
        }
        Ok(match live_row {
            Some(_) => Rotation::Stay(SwitchReason::OnlyOneAccount, ONLY_ONE),
            None => Rotation::Stay(SwitchReason::NoValidTarget, "no account can be activated"),
        })
    }

    /// Under the locks: whether the plan's rotation pick still stands, decided from the store
    /// alone (§9.3 "under the locks only the chosen account is read again"). It does when the
    /// pick is still a candidate and every candidate the new walk order puts before it is one
    /// the planning walk already read and passed over.
    fn rotation_pick_stands(
        &self,
        store: &Store,
        plan: &Plan,
        anchor: Option<&AccountRow>,
    ) -> Result<bool, EngineError> {
        let Some(order) = self.candidate_order(store, &plan.target.provider, anchor)? else {
            return Ok(false);
        };
        Ok(order
            .iter()
            .position(|r| r.id == plan.target.id)
            .is_some_and(|at| order[..at].iter().all(|r| plan.walked.contains(&r.id))))
    }

    /// §9.4 "Before locking": asks the oracle about the outgoing live secret when it is not
    /// that account's vault generation. Never called under the mutation lock.
    fn oracle_hint(&self, p: &dyn Provider, out: &AccountRow) -> Option<OracleHint> {
        let bytes = Axis::of(p, &out.kind).live_secret(&p.read_live_auth(&self.env))?;
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
        let mut walked = Vec::new();
        let target = match &req.target {
            // A switch never crosses providers (§9.3).
            SwitchTarget::Account(id) => store
                .account(id)?
                .filter(|a| a.provider == req.provider)
                .ok_or_else(|| EngineError::NoSuchAccount(id.to_string()))?,
            SwitchTarget::Rotation => {
                match self.rotation(store, &req.provider, live_row.as_ref())? {
                    Rotation::To(a, passed) => {
                        walked = passed;
                        a
                    }
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
            let reconcile = Axis::of(p, &target.kind)
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
            walked,
            warnings: vec![],
        }))
    }

    /// §7.2: refreshes the plan's target through the gate when its access token is about to
    /// expire. This runs before any lock is taken (§4.3). A rotation whose pick turns out dead
    /// plans again from the current roster, which now skips it; every round quarantines one
    /// more account, so the rounds are bounded by the roster.
    fn freshen_plan(
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
    fn due(&self, p: &dyn Provider, vault: &[u8]) -> bool {
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
        if plan.self_switch || !p.kind_traits(&target.kind).refreshable {
            return Ok(Freshened::Go(vec![]));
        }
        let vault = self.read_target(target)?;
        let due = self.due(p, &vault);
        let rotation = matches!(req.target, SwitchTarget::Rotation);
        if target.quarantine_reason.is_some() {
            // Never refreshed (§7.4): usable only while its current access token lasts.
            return match (due, rotation) {
                (false, _) => Ok(Freshened::Go(vec![works_until_expiry(target)])),
                (true, true) => Ok(Freshened::Replan),
                (true, false) => Err(needs_relogin(target)),
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
        Ok(match self.refresh_stored(p, &target.id, &vault)? {
            // Busy: another process is refreshing it now. The account lock this switch waits
            // for, and the pending-rescue settle under it (§6.2), pick up that refresh.
            GateOutcome::Refreshed(_) | GateOutcome::AlreadyFresh(_) | GateOutcome::Busy => {
                Freshened::Go(vec![])
            }
            GateOutcome::Dead(_) if rotation => Freshened::Replan,
            GateOutcome::Dead(_) => return Err(needs_relogin(target)),
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
            // live, the gate leaves its refresh to CC (§7.3 step 2).
            GateOutcome::Transient { kind, .. } => {
                Freshened::Go(vec![cannot_refresh(label, &kind)])
            }
            GateOutcome::Systemic(detail) => Freshened::Go(vec![cannot_refresh(label, &detail)]),
            // The successor is lost and the vault's generation is spent (§7.3 step 6): no
            // retry helps, and activating would hand CC a used refresh token.
            GateOutcome::Unpersisted => return Err(needs_relogin(target)),
            // A journal row names it. The gate cannot tell a switch still in progress from an
            // interrupted one, so this does not refuse: with `--force` the switch supersedes
            // the row, and without it `guard_or_refuse` decides under the mutation lock (it
            // waits for a live holder, and recovers or refuses a dead one with the right
            // message).
            GateOutcome::Owned(OwnedBy::Journal) => {
                Freshened::Go(vec![cannot_refresh(label, "an unfinished switch names it")])
            }
            // The gate found it live where planning did not (it became the live login, or the
            // live identity could not be read): never refreshed here. `rederive` plans the
            // self-switch again, and the warning is carried into its outcome.
            GateOutcome::Owned(OwnedBy::Live) => {
                Freshened::Go(vec![cannot_refresh(label, "it may be the live login")])
            }
            // Unreachable before M4, which introduces sessions and provenance. M4 gives these
            // two the spec's `session-owned` and `profile-conflict` kinds (§7.2's table); until
            // then they refuse with `invalid-input`.
            GateOutcome::Owned(OwnedBy::Session) => {
                return Err(EngineError::InvalidInput(format!(
                    "{label} is in use by a `tagteam run` session; exit it first"
                )));
            }
            GateOutcome::Conflict => {
                return Err(EngineError::InvalidInput(format!(
                    "{label}'s session profile holds a login that conflicts with the vault; refusing to activate it"
                )));
            }
        })
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
        // §7.2: before the mutation lock, the only place a manual switch may use the network.
        let mut plan = match self.plan(p, &store, &req, Ask::Oracle)? {
            Planned::Done(outcome) => return Ok(outcome),
            Planned::Go(plan) => match self.freshen_plan(p, &store, &req, plan)? {
                Planned::Done(outcome) => return Ok(outcome),
                Planned::Go(plan) => plan,
            },
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
                // No freshen here: this runs under the mutation lock, where no network is
                // allowed (§4.3). A pick that changed is activated with the vault's
                // generation, and CC refreshes it.
                let warnings = std::mem::take(&mut plan.warnings);
                plan = match self.plan(p, &store, &req, Ask::Reuse(plan.hint.take()))? {
                    Planned::Done(mut outcome) => {
                        outcome.warnings.extend(warnings);
                        return Ok(outcome);
                    }
                    Planned::Go(mut next) => {
                        next.warnings = warnings;
                        next
                    }
                };
            }
            let (_, outgoing) = self.live_row(p, &store, &req.provider)?;
            let mut ids = vec![&plan.target.id];
            if let Some(o) = &outgoing {
                ids.push(&o.id);
            }
            let accounts = self.lock_accounts(&ids)?;
            let locks = p.lock_live(&self.env, &guard)?;
            match self.rederive(p, &store, &req, &plan, outgoing.as_ref())? {
                Rederived::Go(locked) => {
                    return self.transact(p, &store, &plan, locked, &accounts, &locks, &req);
                }
                Rederived::Done(mut outcome) => {
                    outcome.warnings.extend(plan.warnings.clone());
                    return Ok(outcome);
                }
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
        // The rotation decision, recomputed from the store alone (§9.2, §9.3), including the
        // fewer-than-two case; no vault but the target's is read here. Its anchor is `again`,
        // which is `outgoing` when anything proceeds.
        // Only the target's vault is read here (§9.3): if it emptied while this command waited,
        // a rotation plans again and rotates on, and a direct switch reports it.
        if !self.has_login(&target)? {
            return Ok(Rederived::Replan);
        }
        // §9.4 step 1 (amended): a refresh that finished while this switch waited for the
        // target's account lock may have quarantined it (§7.4 `successor_lost`). This is
        // decided from the row just read, never from what the plan saw: a plan made on an
        // earlier attempt may already hold the quarantine, yet nothing else applies §7.2's
        // quarantined-target rule under the locks. A rotation plans again, and the walk skips
        // the row. A direct target is refused when its access token is due, and otherwise
        // activated with the warning (unless freshening already gave it). A self-switch
        // activates the live generation, which no refresh has spent, so the rule does not
        // apply to it (§7.2: only CC refreshes a live token).
        let mut warnings = Vec::new();
        if target.quarantine_reason.is_some() {
            if matches!(req.target, SwitchTarget::Rotation) {
                return Ok(Rederived::Replan);
            }
            if !self_switch && p.kind_traits(&target.kind).refreshable {
                let vault = self.read_target(&target)?;
                if self.due(p, &vault) {
                    return Err(needs_relogin(&target));
                }
                let warning = works_until_expiry(&target);
                if !plan.warnings.contains(&warning) {
                    warnings.push(warning);
                }
            }
        }
        let same_pick = match req.target {
            SwitchTarget::Account(_) => true,
            SwitchTarget::Rotation => self.rotation_pick_stands(store, plan, again.as_ref())?,
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
                warnings,
            })
        } else {
            Rederived::Replan
        })
    }

    fn read_target(&self, target: &AccountRow) -> Result<Vec<u8>, EngineError> {
        match self.vault.read(&target.id) {
            Read::Present(b) if !b.is_empty() => Ok(b),
            Read::Unreadable(source) => Err(EngineError::UnreadableAccount {
                position: target.position,
                label: target.label.clone(),
                source,
            }),
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
            warnings: locked_warnings,
        } = locked;
        let provider = &req.provider;
        let target_identity = p.parse_identity(&target.identity_json)?;
        let live = p.read_live_auth(&self.env);
        refuse_unsafe_live_reads(&live)?;
        let doomed = p.doomed(&self.env, locks, LiveChange::Write(&target.kind));
        refuse_unreadable(&doomed)?;

        // §6.2 "Pending rescues before activation": a rescue has already spent the vault's
        // generation. It is settled here, under the target's account lock and before anything
        // is written, so neither branch below reads the spent generation (§9.4 steps 2 and 5).
        // A rescue that cannot be read or adopted refuses the switch before its journal row
        // exists, so there is nothing to roll back.
        let target_lock = account_locks
            .iter()
            .find(|l| l.id() == &target.id)
            .expect("the target is locked");
        self.settle_rescues(p, &target, target_lock)?;

        let mut warnings = plan.warnings.clone();
        warnings.extend(locked_warnings);
        // What steps 2 and 4 settle, and the vaults below: step 7's rule never saves it again.
        let mut held = Held::default();
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
                        held.hold(p, &bytes);
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
                // Step 4 settled the outgoing generation: kept, captured or displaced.
                if let Some(bytes) = Axis::of(p, &out.kind).live_secret(&live) {
                    held.hold(p, &bytes);
                }
                // Read only now: settling a self-switch may have captured a newer live
                // generation into this very account, and that is the one to activate.
                self.read_target(&target)?
            }
        };
        // Step 7's rule: every entry the write surely destroys is saved first, unless a vault
        // of either account, or steps 2 and 4, already hold its generation. What only a
        // Keychain-refusal fallback destroys is saved by `before_fallback`, if it happens.
        for id in outgoing.iter().map(|o| &o.id).chain([&target.id]) {
            self.hold_vault(p, &mut held, id);
        }
        for entry in doomed.iter().filter(|d| !d.on_fallback) {
            if let Read::Present(bytes) = &entry.bytes {
                self.save_unheld(
                    p,
                    provider,
                    bytes,
                    &mut held,
                    req.force,
                    live_identity.as_ref(),
                    &mut warnings,
                )?;
            }
        }

        // Step 6.
        let from_secret = outgoing
            .as_ref()
            .and_then(|o| Axis::of(p, &o.kind).live_secret(&live))
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
        let mut before_fallback = |bytes: &[u8]| {
            self.save_unheld(
                p,
                provider,
                bytes,
                &mut held,
                req.force,
                live_identity.as_ref(),
                &mut warnings,
            )
            .map_err(|e| {
                ProviderError::Invalid(format!(
                    "could not save a credential the Keychain fallback would delete: {e}"
                ))
            })
        };
        let stored_in = match self.apply(
            p,
            &target,
            &target_login,
            outgoing.as_ref(),
            req,
            &mut tx,
            &mut before_fallback,
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
        let Some(bytes) = Axis::of(p, &out.kind).live_secret(live) else {
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

    /// Records the generations `id`'s vault holds, current and `.prev`. One that cannot be read
    /// is left out, so what it may hold is saved rather than assumed kept.
    pub(crate) fn hold_vault(&self, p: &dyn Provider, held: &mut Held, id: &AccountId) {
        for bytes in [self.vault.read(id), self.vault.read_prev(id)]
            .into_iter()
            .filter_map(Read::present)
        {
            held.hold(p, &bytes);
        }
    }

    /// §9.4 step 7: saves `bytes`, a live entry a change is about to overwrite or delete, unless
    /// it holds nothing account-scoped or a generation `held` already keeps; either way the
    /// generation is held from then on, so no entry of it is saved twice. A failed save aborts,
    /// except under `force` (B.5).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn save_unheld(
        &self,
        p: &dyn Provider,
        provider: &ProviderId,
        bytes: &[u8],
        held: &mut Held,
        force: bool,
        live_identity: Option<&Identity>,
        warnings: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        match p.fingerprint(bytes) {
            Some(fp) if held.insert(fp.as_str()) => {
                let saved = self.displace_live(
                    p,
                    provider,
                    bytes,
                    "displaced-live-login",
                    live_identity,
                    warnings,
                );
                unless_forced(saved, force, warnings)
            }
            _ => Ok(()),
        }
    }

    /// Steps 7–9, keeping an undo for each write.
    #[allow(clippy::too_many_arguments)]
    fn apply<'a>(
        &self,
        p: &dyn Provider,
        target: &AccountRow,
        target_login: &StoredLogin,
        outgoing: Option<&AccountRow>,
        req: &SwitchRequest,
        tx: &mut Rollback<'a, '_>,
        before_fallback: BeforeFallback<'_>,
    ) -> Result<SecretStore, EngineError> {
        let locks = tx.locks;
        hooks::point(self, "after-journal")?;
        let stored_in = tx.write(|| {
            p.write_credential(&self.env, locks, target_login, before_fallback)
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
