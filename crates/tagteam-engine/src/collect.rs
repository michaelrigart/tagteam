//! The usage collector (§8.3): reserve, fetch and record, one thread per account. An inactive
//! account's token comes from the vault, through the refresh gate (§7.3) when it needs one; the
//! active account's comes from the live store and is never refreshed by a fetch (§8.1). Nothing
//! is sent without the store's authorization right before the request: the lease still held,
//! the token not refused, and a slot in the identity's hourly budget (§8.3, §8.6).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::thread;

use tagteam_core::backoff::failure_backoff_s;
use tagteam_core::poll::plan_after_fetch;
use tagteam_core::usage::{earliest_relevant_reset, max_relevant_pct};
use tagteam_core::{AccountId, PollBudget, PollInputs, PollPlan, ProviderId, Window};
use tagteam_provider::provider::UsageResult;
use tagteam_provider::{Credential, LockError, Provenance, Provider, Read, TransientKind};

use crate::active::{ActiveOutcome, ActiveTrigger};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::refresh::{GateOutcome, expired};
use crate::store::{
    AccountRow, Ineligible, Reservation, Reserve, SendGrant, Slot, Store, StoreError, UsageStateRow,
};

/// Who asked for a collection, and so which accounts are collected. M3 adds `Scheduled`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectMode {
    /// `list` and `status` (§8.3): the listed accounts, each only if its reading is older than
    /// the 180 s floor and a poll is due or none is planned.
    OnDemand { accounts: Vec<AccountId> },
}

/// What one account's collection did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Collected {
    /// A reading was recorded (§8.3 phase 3).
    Recorded,
    /// Not eligible now (§8.3 phase 1): nothing was reserved or sent.
    Ineligible(Ineligible),
    /// The identity's hourly budget was spent when reserving: nothing was sent, and the store
    /// recorded the refusal as an `over-budget` failure, backing off and moving the next poll
    /// to `next_free_at` (§8.6). A refusal at the send itself is `Failed { "over-budget" }`.
    OverBudget { next_free_at: i64 },
    /// Recorded as a failure; `kind` is its `last_error` token (§8.3, Decision 10). The kind
    /// `error` is the exception: the collection ended with an error (the store or a hook), so
    /// nothing was recorded, and the report's warnings say why.
    Failed { kind: String },
    /// Nothing was recorded: another process took the lease before this fetch sent or
    /// recorded (the fence failed, §8.3), or the live login moved to another account while
    /// this one's token was being read.
    Dropped,
    /// Nothing to collect: the provider lacks the `usage` capability, or the account is a
    /// managed API key, which has no usage (§13.2 `api_key`).
    Unsupported,
}

#[derive(Debug, Clone, Default)]
pub struct CollectReport {
    /// One entry per listed account that exists, in the order listed.
    pub outcomes: Vec<(AccountId, Collected)>,
    /// Lines for stderr, each naming an account and never a token: a successor lost while
    /// collecting (§8.3), a refresh that failed with an error or, for the live token (§8.1),
    /// with any outcome but Dead, or an account whose collection ended with an error.
    pub warnings: Vec<String>,
}

/// One account's collection: what it did, and its warnings.
type Outcome = (Collected, Vec<String>);

/// A jitter draw for the poll policy, uniform in [-1, 1) (Decision 6).
pub(crate) fn jitter() -> f64 {
    fastrand::f64() * 2.0 - 1.0
}

impl Engine {
    /// §8.3 on demand: every listed account on its own thread, and the call waits for them
    /// all. A usage failure is never an error here: it is recorded, and reported in the
    /// report's outcomes and warnings. So is an error that ends one account's collection (the
    /// store failing under it): every thread is joined and kept, and that account's outcome is
    /// `Failed { kind: "error" }` with one warning naming it, so one account never costs the
    /// others' outcomes. `Err` only for an error before any thread starts (opening the store,
    /// reading the listed accounts, reading each provider's recorded active account). IDs
    /// that name no account are skipped. Never creates the store.
    pub fn collect_usage(&self, mode: CollectMode) -> Result<CollectReport, EngineError> {
        let CollectMode::OnDemand { accounts } = mode;
        hooks::point(self, "usage-collect-start")?;
        let Some(shared) = self.existing_store()? else {
            return Ok(CollectReport::default());
        };
        let store: &Store = &shared;
        let mut rows = Vec::new();
        for id in &accounts {
            if let Some(row) = store.account(id)? {
                rows.push(row);
            }
        }
        // Each provider's recorded active account is read before its live login: a switch
        // writes the live login before it commits the record, so a record read first can only
        // be older than the live role, never newer (`Collection::active_now`).
        let mut recorded: HashMap<ProviderId, Option<AccountId>> = HashMap::new();
        for row in &rows {
            if !recorded.contains_key(&row.provider) {
                recorded.insert(row.provider.clone(), store.active(&row.provider)?);
            }
        }
        hooks::point(self, "usage-roles-between-reads")?;
        let live = self.live_accounts(&rows);
        let results: Vec<Result<Outcome, EngineError>> = thread::scope(|s| {
            let running: Vec<_> = rows
                .iter()
                .map(|row| {
                    let active = live.contains(&row.id);
                    let started_with = recorded[&row.provider].clone();
                    s.spawn(move || self.collect_one(store, row, active, started_with))
                })
                .collect();
            running
                .into_iter()
                .map(|t| match t.join() {
                    Ok(result) => result,
                    Err(panic) => std::panic::resume_unwind(panic),
                })
                .collect()
        });
        let mut report = CollectReport::default();
        for (row, result) in rows.iter().zip(results) {
            let (collected, warnings) = result.unwrap_or_else(|e| {
                let warning = format!(
                    "usage for {} (position {}) was not collected: {e}",
                    row.label, row.position
                );
                (
                    Collected::Failed {
                        kind: "error".to_owned(),
                    },
                    vec![warning],
                )
            });
            report.outcomes.push((row.id.clone(), collected));
            report.warnings.extend(warnings);
        }
        Ok(report)
    }

    /// The accounts the providers' live logins name (§8.1's active accounts), read once,
    /// before the threads start. A live login that is absent or unreadable names none: its
    /// provider's accounts all take the inactive path, where the gate refuses any account that
    /// might be live (§7.3 step 2).
    fn live_accounts(&self, rows: &[AccountRow]) -> HashSet<AccountId> {
        let mut live = HashSet::new();
        let mut seen: Vec<&ProviderId> = Vec::new();
        for row in rows {
            if seen.contains(&&row.provider) {
                continue;
            }
            seen.push(&row.provider);
            let Some(p) = self.registry.get(&row.provider) else {
                continue;
            };
            let Read::Present(identity) = p.live_identity(&self.env) else {
                continue;
            };
            let key = p.identity_key(&identity);
            live.extend(
                rows.iter()
                    .filter(|r| r.provider == row.provider && r.identity_key == key.as_str())
                    .map(|r| r.id.clone()),
            );
        }
        live
    }

    /// One account through §8.3's three phases.
    fn collect_one(
        &self,
        store: &Store,
        row: &AccountRow,
        active: bool,
        recorded_active: Option<AccountId>,
    ) -> Result<Outcome, EngineError> {
        let Some(provider) = self.registry.get(&row.provider) else {
            return Ok((Collected::Unsupported, Vec::new()));
        };
        let p = provider.as_ref();
        if !p.capabilities().usage || p.kind_traits(&row.kind).managed_key_axis {
            return Ok((Collected::Unsupported, Vec::new()));
        }
        let budget = p.poll_budget();
        // Phase 1: eligibility, the lease and the slot, in one transaction.
        let reservation = match store.reserve_usage(row, self.now_ms(), true, &budget)? {
            Reserve::Reserved(r) => r,
            Reserve::Ineligible(why) => return Ok((Collected::Ineligible(why), Vec::new())),
            Reserve::OverBudget { next_free_at } => {
                return Ok((Collected::OverBudget { next_free_at }, Vec::new()));
            }
        };
        let slot = Slot {
            slot: reservation.slot,
            slot_at: reservation.slot_at,
        };
        let state = match hooks::point(self, "usage-reserved")
            .and_then(|()| Ok(store.usage_state(&row.id)?))
        {
            Ok(state) => state,
            Err(e) => {
                // Nothing was sent: give the slot back, best effort, before the error.
                let _ = store.release_slot(&reservation, &slot);
                return Err(e);
            }
        };
        let mut run = Collection {
            engine: self,
            store,
            p,
            row,
            active,
            budget,
            slot: Some(slot),
            rejected: state.as_ref().and_then(|s| s.rejected_fp.clone()),
            gated: false,
            recorded_active,
            state,
            reservation,
            warnings: Vec::new(),
        };
        // Phase 2, holding no lock but those the gate or §7.5 take for their own refresh.
        let fetched = if active { run.active() } else { run.inactive() };
        // Phase 3.
        run.record(fetched)
    }
}

/// One account's fetch, from its reservation to its record.
struct Collection<'a> {
    engine: &'a Engine,
    store: &'a Store,
    p: &'a dyn Provider,
    row: &'a AccountRow,
    /// Whether the live login names this account (§8.1's active account).
    active: bool,
    budget: PollBudget,
    reservation: Reservation,
    /// The slot reserved for the next request, while that request is unsent. `send` hands it
    /// to `authorize_send` and puts it back only if the request never left; one still here at
    /// the record is given back (§8.3).
    slot: Option<Slot>,
    /// The state read after reserving: the previous reading, the failure count, the plan.
    state: Option<UsageStateRow>,
    /// The access-token fingerprint the server refused (`rejected_fp`, §8.1), as read after
    /// reserving and stamped since. The store's copy is the one `authorize_send` checks.
    rejected: Option<String>,
    /// Whether this collection's own gate refresh (§7.3, `Refreshed`, not `AlreadyFresh`)
    /// has produced the token in use: a 401 on it is not refreshed again (§8.3: at most a
    /// gate refresh, a fetch and one retry).
    gated: bool,
    /// The store's record of the provider's active account, read before the live login
    /// (`collect_usage`).
    recorded_active: Option<AccountId>,
    warnings: Vec<String>,
}

/// A failure for phase 3 to record, with what its backoff needs (§8.5).
struct Failure {
    kind: String,
    is_429: bool,
    retry_after_s: Option<f64>,
    /// No retry before this: when an over-budget identity's oldest counted request leaves the
    /// hour (§8.6).
    not_before: Option<i64>,
}

impl Failure {
    fn new(kind: &str) -> Self {
        Failure {
            kind: kind.to_owned(),
            is_429: false,
            retry_after_s: None,
            not_before: None,
        }
    }
}

/// Why a fetch ended without windows.
enum Stop {
    /// Recorded as a failure.
    Failed(Failure),
    /// The live login moved to another account: nothing is recorded, and the unsent slot goes
    /// back.
    Moved,
    /// The lease was taken over, the account's identity changed, or the account was
    /// quarantined (§8.3's fence failed at `authorize_send` or at the `rejected_fp` stamp):
    /// nothing is sent or recorded. A held slot that was never sent is given back, best
    /// effort, since the fence itself writes nothing.
    LeaseLost,
    /// The store or a test hook failed: returned, never recorded. The unsent slot goes back,
    /// best effort, so a lasting fault does not spend the hourly budget (§8.6); `collect_usage`
    /// turns it into a warning and a `Failed { kind: "error" }` outcome.
    Error(EngineError),
}

impl From<EngineError> for Stop {
    fn from(e: EngineError) -> Self {
        Stop::Error(e)
    }
}

impl From<StoreError> for Stop {
    fn from(e: StoreError) -> Self {
        Stop::Error(e.into())
    }
}

fn failed(kind: &str) -> Stop {
    Stop::Failed(Failure::new(kind))
}

/// The fetch's windows, or the failure its result records (§8.3, §6.1's tokens).
fn windows(result: UsageResult) -> Result<Vec<Window>, Stop> {
    match result {
        UsageResult::Windows(windows) => Ok(windows),
        UsageResult::NoAccessToken => Err(failed("no-access-token")),
        UsageResult::Unauthorized => Err(failed("http-401")),
        UsageResult::Failed {
            kind,
            retry_after_s,
        } => Err(Stop::Failed(Failure {
            is_429: kind == TransientKind::Http(429),
            kind: kind.token(),
            retry_after_s,
            not_before: None,
        })),
    }
}

impl Collection<'_> {
    fn now_ms(&self) -> i64 {
        self.engine.now_ms()
    }

    fn now_s(&self) -> i64 {
        self.now_ms().div_euclid(1000)
    }

    /// §8.1 for an inactive account: the vault's token, refreshed through the gate (§7.3)
    /// first when it has expired or the server refused it. After a 401: one refresh through
    /// the gate and one retry, under a slot of its own.
    fn inactive(&mut self) -> Result<Vec<Window>, Stop> {
        let sent = self.stored_token()?;
        let first = self.send(&sent)?;
        if !matches!(first, UsageResult::Unauthorized) {
            return windows(first);
        }
        self.reject(&sent)?;
        if self.gated || !self.refreshable(&sent) {
            return Err(failed("http-401"));
        }
        let next = self.gate(&sent)?;
        self.retry(&next)
    }

    /// §8.1 for the active account. Only §7.5 refreshes the live token, never a usage fetch: an
    /// expired or refused live token is handed to it (`Expired`, `Rejected`) before anything is
    /// sent, and the fetch goes on with the live token, read fresh, only if §7.5 left one that
    /// is usable and different. A 401 on a token still valid locally (the only kind `send`
    /// sends) stamps `rejected_fp` first, then goes to §7.5 and retries once.
    fn active(&mut self) -> Result<Vec<Window>, Stop> {
        let mut live = self.live_bytes()?;
        if let Some(trigger) = self.trigger(&live) {
            live = self.refresh_live(trigger)?;
        }
        let first = self.send(&live)?;
        if !matches!(first, UsageResult::Unauthorized) {
            return windows(first);
        }
        self.reject(&live)?;
        let Some(trigger) = self.trigger(&live) else {
            return Err(failed("http-401"));
        };
        let next = self.refresh_live(trigger)?;
        self.retry(&next)
    }

    /// Why §7.5 must see the live token before it is sent: the server refused it
    /// (`rejected_fp`), or it has expired (§7.2).
    fn trigger(&self, live: &[u8]) -> Option<ActiveTrigger> {
        if self.is_rejected(live) {
            return self
                .access_fp(live)
                .map(|access_fp| ActiveTrigger::Rejected { access_fp });
        }
        expired(self.p, live, self.now_ms()).then_some(ActiveTrigger::Expired)
    }

    /// §7.5, then the live token it leaves, read fresh. `Refreshed`, `PersistedNotPublished`,
    /// `PublishedOnly` and `NotNeeded` go on; `send` still refuses a token that is expired or
    /// refused. `Dead` has quarantined the account (`relogin_required`). Any other outcome, and
    /// any error, is a failure with a warning, never a command error (M2a Task 16's
    /// carry-over), except a live credential the oracle gives to another identity, which has a
    /// status of its own. A kind that does not refresh never reaches §7.5, which would refuse
    /// it: its expired token is `token-expired`, and its refused one `http-401`, as on the
    /// inactive path (Decision 11: a refusal is an ordinary failure).
    fn refresh_live(&mut self, trigger: ActiveTrigger) -> Result<Vec<u8>, Stop> {
        if !self.p.kind_traits(&self.row.kind).refreshable {
            return Err(failed(match trigger {
                ActiveTrigger::Rejected { .. } => "http-401",
                ActiveTrigger::Expired => "token-expired",
            }));
        }
        match self.engine.refresh_active(&self.row.provider, trigger) {
            Ok(
                ActiveOutcome::NotNeeded { .. }
                | ActiveOutcome::Refreshed
                | ActiveOutcome::PersistedNotPublished
                | ActiveOutcome::PublishedOnly,
            ) => self.live_bytes(),
            Ok(ActiveOutcome::Dead(_)) => Err(failed("refresh-failed")),
            Ok(ActiveOutcome::Unpersisted) => {
                self.warn_lost();
                Err(failed("refresh-failed"))
            }
            Ok(ActiveOutcome::Systemic(detail)) => {
                self.warn_refresh(&detail);
                Err(failed("refresh-failed"))
            }
            Ok(ActiveOutcome::Transient { kind }) => {
                self.warn_refresh(&kind);
                Err(failed("refresh-failed"))
            }
            Err(EngineError::ForeignLiveCredential { .. }) => Err(failed("foreign-credential")),
            Err(e) => {
                self.warn_refresh(&e);
                Err(failed("refresh-failed"))
            }
        }
    }

    /// The vault's token for an inactive account, refreshed through the gate first when it
    /// has expired or the server refused it. One that cannot be refreshed ends the fetch.
    fn stored_token(&mut self) -> Result<Vec<u8>, Stop> {
        let bytes = match self.engine.vault.read(&self.row.id) {
            Read::Present(b) if !b.is_empty() => b,
            Read::Present(_) | Read::Absent => return Err(failed("vault-absent")),
            Read::Unreadable(_) => return Err(failed("keychain-unavailable")),
        };
        if self.usable(&bytes) {
            return Ok(bytes);
        }
        if !self.refreshable(&bytes) {
            return Err(failed(if self.is_rejected(&bytes) {
                "http-401"
            } else {
                "token-expired"
            }));
        }
        self.gate(&bytes)
    }

    fn refreshable(&self, bytes: &[u8]) -> bool {
        self.p.kind_traits(&self.row.kind).refreshable && self.p.has_refresh_token(bytes)
    }

    /// §7.3 for an inactive account, `snapshot` being the bytes the collector decided on. A
    /// Dead verdict (the gate has quarantined the account) and every deterministic refusal end
    /// the fetch before anything is sent (§8.1).
    fn gate(&mut self, snapshot: &[u8]) -> Result<Vec<u8>, Stop> {
        hooks::point(self.engine, "usage-before-gate")?;
        match self.engine.refresh_stored(self.p, &self.row.id, snapshot) {
            Ok(GateOutcome::Refreshed(bytes)) => {
                self.gated = true;
                Ok(bytes)
            }
            // Another process's refresh produced it: this collection has not spent its one.
            Ok(GateOutcome::AlreadyFresh(bytes)) => Ok(bytes),
            Ok(GateOutcome::Unpersisted) => {
                self.warn_lost();
                Err(failed("refresh-failed"))
            }
            Ok(
                GateOutcome::Dead(_)
                | GateOutcome::Busy
                | GateOutcome::Owned(_)
                | GateOutcome::Conflict
                | GateOutcome::Systemic(_)
                | GateOutcome::Transient { .. },
            ) => Err(failed("refresh-failed")),
            Err(e) => {
                self.warn_refresh(&e);
                Err(failed("refresh-failed"))
            }
        }
    }

    /// The live credential, read while the live login names this account, under tagteam's
    /// mutation lock. A switch holds that lock for its whole transaction and writes the
    /// target's credential before its identity (§9.4), so without it a switch finishing
    /// between the reads would have this reservation send, and record, another account's token.
    /// Under the lock: the live identity, the live credential, then the identity again, which
    /// must still name this account. The lock is dropped before returning, so it is never held
    /// across a request (§8.3) or across §7.5, which takes it itself.
    ///
    /// A live login that moved stops the fetch (`Moved`): its token is not this account's. So
    /// does a lock that cannot be had within its timeout (a switch or another mutation holding
    /// it); any other lock error (the lock file cannot be opened) is the collection's error,
    /// never a silent drop. So does a switch journal row still present once recovery has run
    /// under the guard: that switch may have written another account's credential before its
    /// identity, which both identity checks would pass. `guard_or_refuse`'s refusal becomes a
    /// warning, since nothing else on `list` says so: a retry when recovery could not take the
    /// provider's live locks, `--force` only for a row recovery could not decide.
    /// Residual: a login changed by Claude Code itself, outside tagteam's lock, between the
    /// credential read and the second identity read (its credential written, its `oauthAccount`
    /// not yet) passes both checks and can misattribute that one reading.
    fn live_bytes(&mut self) -> Result<Vec<u8>, Stop> {
        let guard = match self.engine.guard_or_refuse(&self.row.provider) {
            Ok(guard) => guard,
            Err(EngineError::Lock(LockError::Timeout(_))) => return Err(Stop::Moved),
            Err(e @ (EngineError::InterruptedSwitch(_) | EngineError::RecoveryBlocked { .. })) => {
                self.warnings.push(format!(
                    "usage for the live {} account was not collected: {e}",
                    self.row.provider
                ));
                return Err(Stop::Moved);
            }
            Err(e) => return Err(e.into()),
        };
        self.live_names_this_account()?;
        hooks::point(self.engine, "usage-live-identity-read")?;
        let credential = self.p.read_live_auth(&self.engine.env).credential;
        self.live_names_this_account()?;
        drop(guard);
        match credential {
            Read::Present(c) if c.provenance() == Provenance::Degraded => {
                Err(failed("keychain-unavailable"))
            }
            Read::Present(c) if !c.is_empty() => Ok(c.bytes().to_vec()),
            Read::Present(_) | Read::Absent => Err(failed("no-access-token")),
            Read::Unreadable(_) => Err(failed("keychain-unavailable")),
        }
    }

    /// The live login still names this account; `Moved` otherwise.
    fn live_names_this_account(&self) -> Result<(), Stop> {
        match self.p.live_identity(&self.engine.env) {
            Read::Present(i) if self.p.identity_key(&i).as_str() == self.row.identity_key => Ok(()),
            _ => Err(Stop::Moved),
        }
    }

    fn access_fp(&self, bytes: &[u8]) -> Option<String> {
        self.p
            .access_fingerprint(bytes)
            .map(|f| f.as_str().to_owned())
    }

    fn is_rejected(&self, bytes: &[u8]) -> bool {
        self.rejected.is_some() && self.access_fp(bytes) == self.rejected
    }

    /// Neither expired (§7.2) nor refused by the server (§8.1): the only kind of token sent.
    fn usable(&self, bytes: &[u8]) -> bool {
        !self.is_rejected(bytes) && !expired(self.p, bytes, self.now_ms())
    }

    /// Stamps `rejected_fp` with the token the server just refused, so it is never sent again
    /// until it changes (§8.1). The stamp is fenced by the lease (Task 8): a holder that lost
    /// it stops here, recording nothing.
    fn reject(&mut self, bytes: &[u8]) -> Result<(), Stop> {
        if let Some(fp) = self.access_fp(bytes) {
            if !self
                .store
                .set_rejected_fp(&self.reservation, Some(fp.as_str()))?
            {
                return Err(Stop::LeaseLost);
            }
            self.rejected = Some(fp);
        }
        Ok(())
    }

    /// The one request after a 401, under a slot of its own (§8.1, §8.6): the first slot went
    /// with the refused request, so `send` has the store reserve a fresh one.
    fn retry(&mut self, bytes: &[u8]) -> Result<Vec<Window>, Stop> {
        let second = self.send(bytes)?;
        if matches!(second, UsageResult::Unauthorized) {
            self.reject(bytes)?;
        }
        windows(second)
    }

    /// One usage request with `bytes`' access token. A token that has expired or was refused
    /// is never sent (§8.1): the slot stays unsent. Right before the request, the store
    /// authorizes it and hands over the slot to send under (`authorize`). A request that never
    /// left (no access token, or a pre-send failure) puts its slot back.
    fn send(&mut self, bytes: &[u8]) -> Result<UsageResult, Stop> {
        if !self.usable(bytes) {
            return Err(failed("token-expired"));
        }
        hooks::point(self.engine, "usage-before-send")?;
        let slot = self.authorize(bytes)?;
        let result = self
            .p
            .fetch_usage(self.engine.http(), &Credential::fresh(bytes.to_vec()));
        let unsent = matches!(
            result,
            UsageResult::NoAccessToken
                | UsageResult::Failed {
                    kind: TransientKind::PreSend,
                    ..
                }
        );
        self.slot = unsent.then_some(slot);
        Ok(result)
    }

    /// §8.3, §8.6: the store's one fenced authorization, immediately before the request, with
    /// the fingerprint of the exact bytes about to be sent and the slot held (`None` for the
    /// retry). It re-checks the lease and the account's identity, the durable `rejected_fp`
    /// (another process may have been refused this token since this one read its state), and
    /// the slot's validity, replacing a stale slot (a suspend or a slow refresh) or reserving
    /// one for the retry.
    /// - `LeaseLost` (the lease was lost, the identity changed, or the account is
    ///   quarantined): nothing is sent or recorded, and the never-sent held slot goes back,
    ///   best effort, at the record.
    /// - `Rejected`: a stamp written by another process after this collection read its state
    ///   (the fence for `rejected_fp`): nothing is sent, and the slot goes back. Recorded as
    ///   `token-expired` for a refreshable active account (the next collection hands the token
    ///   to §7.5), otherwise `http-401`.
    /// - `OverBudget`: recorded as `over-budget`, backing off until a slot frees up; a stale
    ///   slot has already gone back.
    fn authorize(&mut self, bytes: &[u8]) -> Result<Slot, Stop> {
        let fp = self.access_fp(bytes);
        let held = self.slot.take();
        let grant = match self.store.authorize_send(
            &self.reservation,
            held.as_ref(),
            fp.as_deref(),
            self.now_ms(),
            &self.budget,
        ) {
            Ok(grant) => grant,
            Err(e) => {
                // The store's transaction rolled back, so the slot is still counted: keep it
                // for the record to give back.
                self.slot = held;
                return Err(e.into());
            }
        };
        match grant {
            SendGrant::Send(slot) => Ok(slot),
            SendGrant::LeaseLost => {
                self.slot = held;
                Err(Stop::LeaseLost)
            }
            SendGrant::Rejected => {
                self.slot = held;
                self.rejected = fp;
                let handed_to_active_refresh =
                    self.active && self.p.kind_traits(&self.row.kind).refreshable;
                Err(failed(if handed_to_active_refresh {
                    "token-expired"
                } else {
                    "http-401"
                }))
            }
            SendGrant::OverBudget { next_free_at } => Err(Stop::Failed(Failure {
                not_before: Some(next_free_at),
                ..Failure::new("over-budget")
            })),
        }
    }

    /// §8.3: a successor lost while collecting. Names the account by label and position, never
    /// a token; the refresh has quarantined it (`successor_lost`, `relogin_required`).
    fn warn_lost(&mut self) {
        self.warnings.push(format!(
            "{} (position {}) needs a new login: a refreshed token was lost while collecting usage",
            self.row.label, self.row.position
        ));
    }

    fn warn_refresh(&mut self, detail: &dyn fmt::Display) {
        self.warnings.push(format!(
            "could not refresh {} (position {}) to read its usage: {detail}",
            self.row.label, self.row.position
        ));
    }

    /// §8.6's next plan after a success, from the previous reading and this one.
    fn plan(&self, windows: &[Window], active: bool, now_s: i64) -> PollPlan {
        let settings = self.engine.settings();
        let models = &settings.models;
        let prev = self.state.as_ref();
        let inputs = PollInputs {
            now_s,
            active,
            pct: max_relevant_pct(windows, models),
            prev_pct: prev
                .and_then(|s| s.last_good.as_deref())
                .and_then(|w| max_relevant_pct(w, models)),
            prev_interval_s: prev.and_then(|s| s.poll_interval_s),
            threshold: settings.threshold,
            last_429_at: prev.and_then(|s| s.last_429_at),
            next_relevant_reset: earliest_relevant_reset(windows, models),
        };
        plan_after_fetch(&self.budget, &inputs, jitter())
    }

    /// Phase 3 (§8.3), in a transaction fenced by the lease holder and the account's identity,
    /// so a late or superseded result is dropped. Success stores the reading, its samples and
    /// the next plan (§8.6). Failure never touches the last good reading, backs off (§8.5),
    /// and gives back, by its full identity, a slot whose request was never sent. So does an
    /// error, from any step, best effort: a release that fails never masks it, and the lease
    /// expires as usual.
    fn record(mut self, fetched: Result<Vec<Window>, Stop>) -> Result<Outcome, EngineError> {
        match self.record_to_store(fetched) {
            Ok(collected) => Ok((collected, self.warnings)),
            Err(e) => {
                if let Some(slot) = self.slot.take() {
                    let _ = self.store.release_slot(&self.reservation, &slot);
                }
                Err(e)
            }
        }
    }

    /// Whether the account is the provider's active one when it records. The role read
    /// before the fetch (the live login's) stands, unless the store's record of the active
    /// account (which `add` and a switch's commit write, §9.4 step 9) changed since the
    /// collection started: then a switch committed while the fetch was in flight, and a plan
    /// made for the pre-switch role would overwrite the switch's re-plan for one cycle
    /// (§8.3). The start value is read in `collect_usage` before the live login: a switch
    /// writes the live login before it commits the record, so no switch can fall between the
    /// two reads unseen (one committing after the start read changes the record, and one that
    /// committed before it is already in the live read). The record is not read alone: it
    /// does not follow a login made outside tagteam (Claude Code's `/login`), so a stale one
    /// must never outrank the live login. A removed record keeps the live role. A switch
    /// committing between this read and the record is a window of a few statements, left
    /// open: the next collection plans from the new role.
    fn active_now(&self) -> Result<bool, StoreError> {
        let recorded = self.store.active(&self.row.provider)?;
        Ok(match recorded {
            Some(id) if recorded != self.recorded_active => id == self.row.id,
            _ => self.active,
        })
    }

    fn record_to_store(
        &mut self,
        fetched: Result<Vec<Window>, Stop>,
    ) -> Result<Collected, EngineError> {
        hooks::point(self.engine, "usage-before-record")?;
        let now_s = self.now_s();
        Ok(match fetched {
            Ok(windows) => {
                let plan = self.plan(&windows, self.active_now()?, now_s);
                let retention = self.engine.settings().history_retention_days;
                if self
                    .store
                    .record_usage(&self.reservation, &windows, now_s, &plan, retention)?
                {
                    Collected::Recorded
                } else {
                    Collected::Dropped
                }
            }
            Err(Stop::Failed(f)) => {
                let n = self
                    .state
                    .as_ref()
                    .map_or(0, |s| s.consecutive_failures)
                    .saturating_add(1);
                let mut until = now_s + failure_backoff_s(n, f.is_429, f.retry_after_s);
                if let Some(not_before) = f.not_before {
                    until = until.max(not_before);
                }
                let recorded = self.store.record_usage_failure(
                    &self.reservation,
                    &f.kind,
                    now_s,
                    until,
                    f.is_429.then_some(until),
                    self.slot.as_ref(),
                )?;
                if recorded {
                    Collected::Failed { kind: f.kind }
                } else {
                    // Fenced out before the record deleted it: a slot that was never sent
                    // must not hold budget for the hour. One that was sent is not held here.
                    if let Some(slot) = self.slot.take() {
                        let _ = self.store.release_slot(&self.reservation, &slot);
                    }
                    Collected::Dropped
                }
            }
            Err(Stop::Moved) => {
                if let Some(slot) = &self.slot {
                    self.store.release_slot(&self.reservation, slot)?;
                }
                Collected::Dropped
            }
            Err(Stop::LeaseLost) => {
                if let Some(slot) = self.slot.take() {
                    let _ = self.store.release_slot(&self.reservation, &slot);
                }
                Collected::Dropped
            }
            Err(Stop::Error(e)) => return Err(e),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::jitter;

    #[test]
    fn jitter_is_a_fraction_in_the_policy_s_range() {
        for _ in 0..10_000 {
            let j = jitter();
            assert!((-1.0..=1.0).contains(&j), "{j}");
        }
    }
}
