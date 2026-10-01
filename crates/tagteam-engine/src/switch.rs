use tagteam_core::poll::replan_for_role;
use tagteam_core::rank::{
    BestOrder, Candidate, NextAvailable, best_order, binding_window, next_available, span,
};
use tagteam_core::usage::headroom;
use tagteam_core::{
    AccountId, OracleVerdict, OutgoingAction, OutgoingClass, OutgoingFacts, ProviderId, Window,
    decide_outgoing, rotation_order,
};
use tagteam_provider::{
    BeforeFallback, Credential, DoomedEntry, Identity, LiveAuth, LiveChange, LiveLocks,
    ProcessStamp, Provenance, Provider, ProviderError, Read, ReadError, SecretStore, StoredLogin,
    Undo,
};

use crate::account_lock::AccountLock;
use crate::collect::{CollectMode, jitter};
use crate::displace::displace;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::oracle::verdict;
use crate::refresh::{GateOutcome, OwnedBy};
use crate::rescue::RescueFile;
use crate::store::{AccountRow, EventRow, JournalRow, Store};

/// §9.3's strategies that rank by usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageStrategy {
    Best,
    NextAvailable,
}

impl UsageStrategy {
    /// The strategy as `switch --json` names it (§13.2).
    pub fn as_str(self) -> &'static str {
        match self {
            UsageStrategy::Best => "best",
            UsageStrategy::NextAvailable => "next-available",
        }
    }
}

#[derive(Debug, Clone)]
pub enum SwitchTarget {
    Rotation,
    Account(AccountId),
    /// §9.3: `models` overrides `autoswitch.models` for this switch (`--model`).
    Usage {
        strategy: UsageStrategy,
        models: Option<Vec<String>>,
    },
}

impl SwitchTarget {
    /// Rotation or a usage strategy: the engine chooses the account, so a pick that turns out
    /// dead or quarantined is replaced by planning again, where a named target is refused or
    /// warned about (§7.2, §9.3, §9.4 step 1).
    fn chosen(&self) -> bool {
        !matches!(self, SwitchTarget::Account(_))
    }
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
    UsageUnavailable,
    AlreadyBest,
    CandidatesExhausted,
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
            SwitchReason::UsageUnavailable => "usage-unavailable",
            SwitchReason::AlreadyBest => "already-best",
            SwitchReason::CandidatesExhausted => "candidates-exhausted",
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

/// §9.2: fewer than two candidates, for a rotation or a usage strategy.
const ONLY_ONE: &str = "there is only one switchable account";
/// §9.3: no live login to anchor on, and nothing in the store to activate.
const NO_VALID: &str = "no account can be activated";
/// §9.3 `best`: no candidate's reading can drive a decision (§8.4).
const USAGE_UNAVAILABLE: &str = "no candidate has a usage reading recent enough to rank by";
const ONE_UNRANKED: &str =
    "1 candidate has no usage reading recent enough to rank by; it was not considered";
const LIVE_UNKNOWN: &str =
    "switching to the best known candidate; the live account's usage is unknown";
const NO_LIVE: &str =
    "switching to the best known candidate; there is no managed live login to compare with";

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
        SwitchTarget::Usage { strategy, .. } => strategy.as_str(),
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

    /// Whether `fp` is held already, without recording it.
    pub(crate) fn contains(&self, fp: &str) -> bool {
        self.0.iter().any(|h| h == fp)
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
    /// The live account the plan was made against: a usage strategy's pick stands under the
    /// locks only while it still is (Decision 11).
    anchor: Option<AccountId>,
    /// A rotation's accounts the walk read and passed over before its pick (§9.3). Empty for a
    /// direct target and a usage strategy.
    walked: Vec<AccountId>,
    /// What a usage strategy's ranking tells the user (§9.3): accounts it skipped, candidates
    /// it could not rank, an unknown live headroom. Made afresh with every plan.
    notes: Vec<String>,
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

/// What a usage strategy decided (§9.3).
#[allow(clippy::large_enum_variant)]
enum Ranked {
    /// The pick, and what the ranking tells the user.
    To(AccountRow, Vec<String>),
    /// A no-op: its reason, its message, and what the ranking tells the user.
    Stay(SwitchReason, String, Vec<String>),
}

/// A candidate as a usage strategy ranks it: its row and its decision-grade windows (`None`:
/// its reading cannot drive a decision, §8.4).
struct Rated {
    row: AccountRow,
    windows: Option<Vec<Window>>,
}

impl Rated {
    fn headroom(&self, models: &[String]) -> Option<f64> {
        self.windows.as_deref().and_then(|w| headroom(w, models))
    }

    fn candidate(&self, models: &[String]) -> Candidate {
        Candidate {
            position: self.row.position,
            headroom: self.headroom(models),
        }
    }

    /// Its binding window as a message names it: `7d at 77%`.
    fn binding(&self, models: &[String]) -> String {
        binding_text(self.windows.as_deref().unwrap_or_default(), models)
    }

    /// `a@x.co (7d at 77%)`.
    fn described(&self, models: &[String]) -> String {
        format!("{} ({})", self.row.label, self.binding(models))
    }
}

/// §8.2's binding window, as a strategy's message names it: its label and its pct, rounded as
/// `list` rounds it.
fn binding_text(windows: &[Window], models: &[String]) -> String {
    match binding_window(windows, models) {
        Some(w) => format!("{} at {}%", w.label, w.pct.round() as i64),
        None => "usage unknown".to_owned(),
    }
}

/// A walk that found no account to activate ends as a rotation's does (§9.3).
fn nothing_to_activate(live_row: Option<&AccountRow>) -> Ranked {
    match live_row {
        Some(_) => Ranked::Stay(SwitchReason::OnlyOneAccount, ONLY_ONE.into(), Vec::new()),
        None => Ranked::Stay(SwitchReason::NoValidTarget, NO_VALID.into(), Vec::new()),
    }
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
    /// A rotation's or a usage strategy's pick turned out dead and is quarantined now: plan
    /// again; the walk skips it (§9.3).
    Replan,
}

fn needs_relogin(target: &AccountRow) -> EngineError {
    EngineError::NeedsRelogin {
        position: target.position,
        label: target.label.clone(),
    }
}

fn cannot_refresh(app: &str, label: &str, why: &str) -> String {
    format!("could not refresh {label} first ({why}); {app} will refresh it when it is online")
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

    /// §9.3's lazy walk, shared by rotation and the usage strategies: reads each vault in
    /// `order` only until one holds a credential, and returns that pick with the accounts it
    /// passed over. An unreadable vault before the pick could have been the pick, so the walk
    /// fails naming the account; no vault after the pick is read.
    fn walk(
        &self,
        order: impl IntoIterator<Item = AccountRow>,
    ) -> Result<Option<(AccountRow, Vec<AccountId>)>, EngineError> {
        let mut walked = Vec::new();
        for row in order {
            if self.vault_holds_login(&row)? {
                return Ok(Some((row, walked)));
            }
            walked.push(row.id);
        }
        Ok(None)
    }

    /// §9.3 rotation, reading the vault lazily (`walk`). The candidates are counted from the
    /// store: with a managed live anchor and fewer than two of them, it stays put (§9.2).
    fn rotation(
        &self,
        store: &Store,
        provider: &ProviderId,
        live_row: Option<&AccountRow>,
    ) -> Result<Rotation, EngineError> {
        let Some(order) = self.candidate_order(store, provider, live_row)? else {
            return Ok(Rotation::Stay(SwitchReason::OnlyOneAccount, ONLY_ONE));
        };
        if let Some((row, walked)) = self.walk(order)? {
            return Ok(Rotation::To(row, walked));
        }
        Ok(match live_row {
            Some(_) => Rotation::Stay(SwitchReason::OnlyOneAccount, ONLY_ONE),
            None => Rotation::Stay(SwitchReason::NoValidTarget, NO_VALID),
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

    /// §9.3's usage strategies, planned from the store alone: the candidates are a rotation's
    /// (`candidate_order`), each ranked by its decision-grade headroom under `models` (§8.2,
    /// §8.4), and their vaults are read lazily in the strategy's own order (`walk`).
    fn usage_pick(
        &self,
        store: &Store,
        provider: &ProviderId,
        live_row: Option<&AccountRow>,
        strategy: UsageStrategy,
        models: &[String],
    ) -> Result<Ranked, EngineError> {
        let Some(order) = self.candidate_order(store, provider, live_row)? else {
            return Ok(Ranked::Stay(
                SwitchReason::OnlyOneAccount,
                ONLY_ONE.into(),
                Vec::new(),
            ));
        };
        // Only without a managed live login: nothing in the store can be a candidate.
        if order.is_empty() {
            return Ok(Ranked::Stay(
                SwitchReason::NoValidTarget,
                NO_VALID.into(),
                Vec::new(),
            ));
        }
        let rated = order
            .into_iter()
            .map(|row| {
                Ok(Rated {
                    windows: self.decision_windows(&row, models)?,
                    row,
                })
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        match strategy {
            UsageStrategy::Best => self.best_pick(&rated, live_row, models),
            UsageStrategy::NextAvailable => self.next_available_pick(&rated, live_row, models),
        }
    }

    /// §9.3 `best`: the known candidate with the most headroom, if it beats the live
    /// account's; every known one, with a warning, when the live headroom is unknown or there
    /// is no managed live login. A candidate whose headroom is unknown is never picked, and a
    /// warning counts them. A better candidate whose vault holds nothing is not switchable, so
    /// a walk that finds none to activate is `already-best` (or `usage-unavailable` when the
    /// live headroom is unknown).
    fn best_pick(
        &self,
        rated: &[Rated],
        live_row: Option<&AccountRow>,
        models: &[String],
    ) -> Result<Ranked, EngineError> {
        let candidates: Vec<Candidate> = rated.iter().map(|r| r.candidate(models)).collect();
        let at = |position: &u32| rated.iter().find(|r| r.row.position == *position);
        let live = match live_row {
            Some(row) => Some(Rated {
                windows: self.decision_windows(row, models)?,
                row: row.clone(),
            }),
            None => None,
        };
        let live_headroom = live.as_ref().and_then(|l| l.headroom(models));
        let mut notes = match candidates.iter().filter(|c| c.headroom.is_none()).count() {
            0 => Vec::new(),
            1 => vec![ONE_UNRANKED.to_owned()],
            n => vec![format!(
                "{n} candidates have no usage reading recent enough to rank by; they were not considered"
            )],
        };
        Ok(match best_order(live_headroom, &candidates) {
            BestOrder::UsageUnavailable => Ranked::Stay(
                SwitchReason::UsageUnavailable,
                USAGE_UNAVAILABLE.into(),
                Vec::new(),
            ),
            BestOrder::AlreadyBest => {
                let leader = match best_order(None, &candidates) {
                    BestOrder::Try(order) => order.first().and_then(at),
                    _ => None,
                };
                let message = match (&live, leader) {
                    (Some(live), Some(leader)) => format!(
                        "{} already has the most headroom ({}); the best candidate is {}",
                        live.row.label,
                        live.binding(models),
                        leader.described(models)
                    ),
                    _ => "the live account already has the most headroom".to_owned(),
                };
                Ranked::Stay(SwitchReason::AlreadyBest, message, notes)
            }
            BestOrder::Try(order) => {
                match self.walk(order.iter().filter_map(at).map(|r| r.row.clone()))? {
                    Some((pick, _)) => {
                        if live_headroom.is_none() {
                            let why = if live_row.is_some() {
                                LIVE_UNKNOWN
                            } else {
                                NO_LIVE
                            };
                            notes.push(why.to_owned());
                        }
                        Ranked::To(pick, notes)
                    }
                    None => match &live {
                        Some(live) if live_headroom.is_some() => Ranked::Stay(
                            SwitchReason::AlreadyBest,
                            format!(
                                "no candidate with more headroom than {} holds a stored credential",
                                live.described(models)
                            ),
                            notes,
                        ),
                        _ => Ranked::Stay(
                            SwitchReason::UsageUnavailable,
                            "no candidate with a known usage reading holds a stored credential"
                                .into(),
                            notes,
                        ),
                    },
                }
            }
        })
    }

    /// §9.3 `next-available`: the rotation's walk without the candidates known to be at their
    /// limit (§8.2: unknown headroom is never skipped). Each skipped account is named, with its
    /// binding window, in a warning. If every candidate is skipped, `candidates-exhausted`
    /// names them all and when the first of those windows resets.
    fn next_available_pick(
        &self,
        rated: &[Rated],
        live_row: Option<&AccountRow>,
        models: &[String],
    ) -> Result<Ranked, EngineError> {
        let walk: Vec<Candidate> = rated.iter().map(|r| r.candidate(models)).collect();
        let at = |position: &u32| rated.iter().find(|r| r.row.position == *position);
        let (order, skipped) = match next_available(&walk) {
            NextAvailable::Exhausted => {
                let all: Vec<&Rated> = rated.iter().collect();
                return Ok(Ranked::Stay(
                    SwitchReason::CandidatesExhausted,
                    self.exhausted_message(&all, models),
                    Vec::new(),
                ));
            }
            NextAvailable::Try { order, skipped } => (order, skipped),
        };
        let skipped: Vec<&Rated> = skipped.iter().filter_map(at).collect();
        let notes = skipped
            .iter()
            .map(|r| {
                format!(
                    "skipped {} (position {}): at its limit ({})",
                    r.row.label,
                    r.row.position,
                    r.binding(models)
                )
            })
            .collect();
        Ok(
            match self.walk(order.iter().filter_map(at).map(|r| r.row.clone()))? {
                Some((pick, _)) => Ranked::To(pick, notes),
                None if skipped.is_empty() => nothing_to_activate(live_row),
                // §9.3: an account whose vault holds nothing is not switchable, so every
                // switchable candidate was skipped.
                None => Ranked::Stay(
                    SwitchReason::CandidatesExhausted,
                    self.exhausted_message(&skipped, models),
                    Vec::new(),
                ),
            },
        )
    }

    /// `candidates-exhausted`'s message: each candidate with its binding window, then how long
    /// until the earliest of those windows resets (§9.3; §11.2 step 8: the binding window
    /// first, then its reset). No reset is named when none of them has one.
    fn exhausted_message(&self, rated: &[&Rated], models: &[String]) -> String {
        let named: Vec<String> = rated.iter().map(|r| r.described(models)).collect();
        let reset = rated
            .iter()
            .filter_map(|r| {
                binding_window(r.windows.as_deref().unwrap_or_default(), models)?.resets_at
            })
            .min();
        let now_s = self.now_ms().div_euclid(1000);
        let when = reset.map_or_else(String::new, |at| {
            format!("; the earliest reset is in {}", span(at - now_s))
        });
        format!(
            "every candidate is at its limit: {}{when}",
            named.join(", ")
        )
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
        let done = |reason: SwitchReason, message: String, warnings: Vec<String>| {
            let mut outcome = noop(
                strategy,
                reason,
                message,
                live_row.clone(),
                unmanaged_email.clone(),
            );
            outcome.warnings = warnings;
            Planned::Done(outcome)
        };
        if let (Some(email), false) = (&unmanaged_email, req.force) {
            return Ok(done(
                SwitchReason::UnmanagedAccount,
                unmanaged_message(email),
                Vec::new(),
            ));
        }
        let mut walked = Vec::new();
        let mut notes = Vec::new();
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
                    Rotation::Stay(reason, message) => {
                        return Ok(done(reason, message.into(), Vec::new()));
                    }
                }
            }
            // §9.3: `--model` replaces `autoswitch.models` for this switch, when given.
            SwitchTarget::Usage {
                strategy: by,
                models,
            } => {
                let models = models
                    .clone()
                    .unwrap_or_else(|| self.settings().models.clone());
                match self.usage_pick(store, &req.provider, live_row.as_ref(), *by, &models)? {
                    Ranked::To(a, said) => {
                        notes = said;
                        a
                    }
                    Ranked::Stay(reason, message, said) => {
                        return Ok(done(reason, message, said));
                    }
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
                return Ok(done(
                    SwitchReason::AlreadyActive,
                    already_active(&target),
                    Vec::new(),
                ));
            }
        }
        Ok(Planned::Go(Plan {
            anchor: live_row.as_ref().map(|r| r.id.clone()),
            target,
            strategy,
            self_switch,
            hint,
            walked,
            notes,
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
        // A forced one re-activates the vault's generation (§9.2), so it freshens like any other.
        if (plan.self_switch && !req.force) || !p.kind_traits(&target.kind).refreshable {
            return Ok(Freshened::Go(vec![]));
        }
        let vault = self.read_target(target)?;
        let due = self.due(p, &vault);
        let chosen = req.target.chosen();
        if target.quarantine_reason.is_some() {
            // Never refreshed (§7.4): usable only while its current access token lasts.
            return match (due, chosen) {
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
            GateOutcome::Dead(_) if chosen => Freshened::Replan,
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
                Freshened::Go(vec![cannot_refresh(p.display_name(), label, &kind)])
            }
            GateOutcome::Systemic(detail) => {
                Freshened::Go(vec![cannot_refresh(p.display_name(), label, &detail)])
            }
            // The successor is lost and the vault's generation is spent (§7.3 step 6): no
            // retry helps, and activating would hand CC a used refresh token.
            GateOutcome::Unpersisted => return Err(needs_relogin(target)),
            // A journal row names it. The gate cannot tell a switch still in progress from an
            // interrupted one, so this does not refuse: with `--force` the switch supersedes
            // the row, and without it `guard_or_refuse` decides under the mutation lock (it
            // waits for a live holder, and recovers or refuses a dead one with the right
            // message).
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

    /// §9.3, before a usage strategy plans: quarantines that no longer bind are released
    /// (§7.4, Decision 9), then the managed live account and every candidate are collected on
    /// demand (§8.3, Decision 8). Returns the collector's warnings. Other targets do neither,
    /// and neither does a usage strategy over an unmanaged live login without --force:
    /// planning reports that no-op (§9.2), and the network is not used for it.
    fn prepare_usage(
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
        let mut accounts: Vec<AccountId> = live_row.into_iter().map(|r| r.id).collect();
        for row in store.accounts(&req.provider)? {
            if is_candidate(&row) && !accounts.contains(&row.id) {
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

    /// §9: plan and ask the oracle without any lock; then take the mutation lock once, and
    /// under it the account locks and the live locks, and re-derive every decision. Anything
    /// that moved while this command waited releases every lock but the mutation lock and
    /// plans again, without the network, for at most `ATTEMPTS` lock acquisitions. A usage
    /// strategy first releases quarantines that no longer bind and collects (§9.3); the
    /// collector's warnings lead the outcome's.
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
        let mut warnings = self.prepare_usage(p, &store, &req)?;
        let mut outcome = self.switch_planned(p, &store, &req)?;
        warnings.append(&mut outcome.warnings);
        outcome.warnings = warnings;
        Ok(outcome)
    }

    /// `switch` from its first plan on.
    fn switch_planned(
        &self,
        p: &dyn Provider,
        store: &Store,
        req: &SwitchRequest,
    ) -> Result<SwitchOutcome, EngineError> {
        // §7.2: before the mutation lock, the only place a manual switch may use the network.
        let mut plan = match self.plan(p, store, req, Ask::Oracle)? {
            Planned::Done(outcome) => return Ok(outcome),
            Planned::Go(plan) => match self.freshen_plan(p, store, req, plan)? {
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
                plan = match self.plan(p, store, req, Ask::Reuse(plan.hint.take()))? {
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
            let (_, outgoing) = self.live_row(p, store, &req.provider)?;
            let mut ids = vec![&plan.target.id];
            if let Some(o) = &outgoing {
                ids.push(&o.id);
            }
            let accounts = self.lock_accounts(&ids)?;
            let locks = p.lock_live(&self.env, &guard)?;
            match self.rederive(p, store, req, &plan, outgoing.as_ref())? {
                Rederived::Go(locked) => {
                    let outcome = self.transact(p, store, &plan, locked, &accounts, &locks, req)?;
                    // The re-plan needs no lock and never fetches (§8.3).
                    drop(locks);
                    drop(accounts);
                    self.replan_polls(p, store, &outcome);
                    return Ok(outcome);
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

    /// §8.3: after a switch that activated an account, both accounts' polls are re-planned
    /// for their new roles, without fetching: the incoming account gets §9.4's plan (180 s
    /// after its reading, due at once when that is past) and the outgoing one the candidate
    /// policy (§8.6). The readings stay as they are. An account with no reading is skipped,
    /// so it keeps no plan and stays eligible on demand (§8.3: a plan in force would make it
    /// not due). The switch has committed, so this is contained (§14): a failure is logged at
    /// ERROR and never fails it.
    fn replan_polls(&self, p: &dyn Provider, store: &Store, outcome: &SwitchOutcome) {
        let Some(to) = &outcome.to else {
            return;
        };
        if !p.capabilities().usage {
            return;
        }
        let budget = p.poll_budget();
        let now_s = self.now_ms().div_euclid(1000);
        let outgoing = outcome.from.as_ref().filter(|from| from.id != to.id);
        for (row, active) in [(Some(to), true), (outgoing, false)] {
            let Some(row) = row else {
                continue;
            };
            let result = store.usage_state(&row.id).and_then(|state| {
                let Some(fetched_at) = state.and_then(|s| s.fetched_at) else {
                    return Ok(());
                };
                let plan = replan_for_role(&budget, active, fetched_at, now_s, jitter());
                store.set_poll_plan(&row.id, &plan)
            });
            if let Err(e) = result {
                tracing::error!(
                    position = row.position,
                    account = %row.id,
                    error = %e,
                    "could not re-plan usage polls after the switch"
                );
            }
        }
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
        // M1's Review Focus 3, and M3a's Review Focus 4 for a usage strategy: another process
        // landed exactly this plan's pick while this one waited for the mutation lock (a
        // double-fired `switch`). That was this command's work; moving on from there would
        // switch twice. A direct target needs no such rule: planning again finds the
        // self-switch no-op.
        if req.target.chosen() && self_switch && !plan.self_switch {
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
        // activated with the warning (unless freshening already gave it). An unforced
        // self-switch activates the live generation, which no refresh has spent, so the rule
        // does not apply to it (§7.2: only CC refreshes a live token); a forced one re-activates
        // the vault's generation (§9.2), so it does.
        let mut warnings = Vec::new();
        if target.quarantine_reason.is_some() {
            if req.target.chosen() {
                return Ok(Rederived::Replan);
            }
            if (!self_switch || req.force) && p.kind_traits(&target.kind).refreshable {
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
            // Decision 11: no network under the locks, so the ranking is not recomputed. The
            // pick stands while it is still a candidate and the live login is the account it
            // was ranked against; otherwise planning again ranks from the store's readings.
            SwitchTarget::Usage { .. } => {
                is_candidate(&target) && again.as_ref().map(|r| &r.id) == plan.anchor.as_ref()
            }
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

        let mut warnings = plan.notes.clone();
        warnings.extend(plan.warnings.iter().cloned());
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
        let ours = vault
            .as_deref()
            .is_some_and(|v| same_generation(p, v, &bytes));
        // §9.4 step 4 `Superseded`: the vault's `.prev` is this generation, so an active-token
        // refresh stored a newer one it could not publish (§7.5). Read only when the live
        // credential is not the vault's own, and tri-state: an unreadable `.prev` may be
        // exactly that generation, and capturing over the newer one would lose it (step 3).
        let superseded = !ours
            && match self.vault.read_prev(&out.id) {
                Read::Present(prev) => same_generation(p, &prev, &bytes),
                Read::Absent => false,
                Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
            };
        let facts = OutgoingFacts {
            bytes_equal_vault: vault.as_deref() == Some(bytes.as_slice()),
            fp_equal_vault: fp_live.is_some()
                && vault.as_deref().and_then(|v| p.fingerprint(v)) == fp_live,
            equals_vault_prev: superseded,
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
