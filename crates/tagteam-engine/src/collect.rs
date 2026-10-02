//! The usage collector (§8.3): reserve, fetch and record, one thread per account. An inactive
//! account's token comes from the vault, through the refresh gate (§7.3) when it needs one; the
//! active account's comes from the live store and is never refreshed by a fetch (§8.1); a
//! session-owned account's comes from its profile, read without a lock and never refreshed,
//! written or retried, because the agent in the session owns it (§8.1, §12.5). Nothing is sent
//! without the store's authorization right before the request: the lease still held, the token
//! not refused, and a slot in the identity's hourly budget (§8.3, §8.6).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::thread;

use tagteam_core::backoff::failure_backoff_s;
use tagteam_core::poll::{DueCandidate, escalates, plan_after_fetch, scheduled_pick};
use tagteam_core::trust::is_future_stamped;
use tagteam_core::usage::{earliest_relevant_reset, max_relevant_pct};
use tagteam_core::{AccountId, PollBudget, PollInputs, PollPlan, ProviderId, Window};
use tagteam_provider::profile::ProfileMarker;
use tagteam_provider::provider::UsageResult;
use tagteam_provider::{Credential, LockError, Provenance, Provider, Read, TransientKind};

use crate::active::{ActiveOutcome, ActiveTrigger};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::provenance::identity_drifted;
use crate::refresh::{GateOutcome, expired};
use crate::store::{
    AccountRow, Eligibility, Ineligible, Reservation, Reserve, SendGrant, Slot, Store, StoreError,
    UsageStateRow, backoff_holds,
};

/// Who asked for a collection, and so which accounts are collected and when each is due (§8.3).
#[derive(Debug, Clone, PartialEq)]
pub enum CollectMode {
    /// `list`, `status` and `switch` (§8.3): the listed accounts, each only if its reading is
    /// older than the 180 s floor and a poll is due or none is planned. Plans follow the
    /// settings' threshold and models.
    OnDemand { accounts: Vec<AccountId> },
    /// An auto tick (§8.6). Phase 1: the provider's live account, when it is managed and a
    /// poll is due or it has no reading. Phase 2, from the store as phase 1 left it: one pick
    /// (`scheduled_pick`) among the provider's other switchable accounts, or every due one when
    /// the live account's decision-grade max relevant pct under `models` is within the
    /// provider's escalation margin of `threshold`, or is unknown. `threshold` and `models` are
    /// the tick's (flags over the file), and every plan recorded after a fetch follows them.
    Scheduled {
        provider: ProviderId,
        threshold: f64,
        models: Vec<String>,
    },
    /// Consume-first's re-check (§8.3, §11.2 step 8): each listed account whose reading is older
    /// than the 180 s floor, whatever its plan. Plans follow the tick's `threshold` and
    /// `models`, as `Scheduled`'s do.
    Recheck {
        accounts: Vec<AccountId>,
        threshold: f64,
        models: Vec<String>,
    },
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
    /// One entry per account collected. `OnDemand` and `Recheck`: each listed account that
    /// exists, in the order listed. `Scheduled`: the live account first, when it is managed,
    /// then each picked candidate in pick order.
    pub outcomes: Vec<(AccountId, Collected)>,
    /// Lines for stderr, each naming an account and never a token: a successor lost while
    /// collecting (§8.3), a refresh that failed with an error or, for the live token (§8.1),
    /// with any outcome but Dead, or an account whose collection ended with an error.
    pub warnings: Vec<String>,
    /// The accounts quarantined while their collection ran, in `outcomes` order (§7.4: a Dead
    /// verdict or an identity conflict in the gate or §7.5, or a lost successor). Only an
    /// account this collection reserved can be named, since a quarantined one is never
    /// reserved; a quarantine another process set meanwhile names it too.
    pub quarantined: Vec<AccountId>,
}

impl CollectReport {
    /// Adds each account's result, in order. An error that ended one account's collection is a
    /// warning naming the account and a `Failed { kind: "error" }` outcome, so one account
    /// never costs the others theirs.
    fn add(&mut self, results: Vec<(&AccountRow, Result<Outcome, EngineError>)>) {
        for (row, result) in results {
            let outcome = result.unwrap_or_else(|e| Outcome {
                collected: Collected::Failed {
                    kind: "error".to_owned(),
                },
                warnings: vec![format!(
                    "usage for {} (position {}) was not collected: {e}",
                    row.label, row.position
                )],
                quarantined: false,
            });
            if outcome.quarantined {
                self.quarantined.push(row.id.clone());
            }
            self.outcomes.push((row.id.clone(), outcome.collected));
            self.warnings.extend(outcome.warnings);
        }
    }
}

/// One account's collection: what it did, its warnings, and whether the account was
/// quarantined while it ran.
struct Outcome {
    collected: Collected,
    warnings: Vec<String>,
    quarantined: bool,
}

impl Outcome {
    /// Nothing was reserved, so nothing was sent or recorded.
    fn unreserved(collected: Collected) -> Self {
        Outcome {
            collected,
            warnings: Vec::new(),
            quarantined: false,
        }
    }
}

/// What a collection's caller sets for every account it collects: when one is eligible
/// (§8.3), and the threshold and models its next plan is made for (§8.6: the urgent band and
/// the relevant windows). An auto tick passes its own, flags over the file; on demand passes
/// the settings'.
#[derive(Clone, Copy)]
struct Policy<'m> {
    eligibility: Eligibility,
    threshold: f64,
    models: &'m [String],
}

/// Which token one account's collection reads (§8.1), decided before the threads start
/// (`Engine::role_of`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Role {
    /// The live login names the account: the live token, which only §7.5 refreshes.
    Active,
    /// The vault's token, refreshed through the gate (§7.3) when it needs it.
    Inactive,
    /// A `tagteam run` session owns the account (§12.5): its profile's token, read as the agent
    /// in the session reads it, and never refreshed, written or retried.
    Session { profile: PathBuf },
}

/// The roles every thread needs, read once before any starts (`Engine::roles`).
struct Roles {
    /// Each provider's recorded active account.
    recorded: HashMap<ProviderId, Option<AccountId>>,
    /// The accounts the providers' live logins name (§8.1's active accounts).
    live: HashSet<AccountId>,
}

/// A jitter draw for the poll policy, uniform in [-1, 1) (Decision 6).
pub(crate) fn jitter() -> f64 {
    fastrand::f64() * 2.0 - 1.0
}

impl Engine {
    /// §8.3: the accounts `mode` selects, each on its own thread, and the call waits for them
    /// all. Each account's role is decided before any thread starts (§8.1, `role_of`): the
    /// account the live login names is the active one, any other that a `tagteam run` session
    /// owns takes the session branch (§12.5), and the rest are inactive. A usage failure is
    /// never an error here: it is recorded, and reported in the report's outcomes and warnings.
    /// So is an error that ends one account's collection (the store failing under it): every
    /// thread is joined and kept, and that account's outcome is `Failed { kind: "error" }` with
    /// one warning naming it, so one account never costs the others' outcomes. A profile that
    /// cannot be read is no such error: it decides a role too (`role_of`). `Err` only for an error outside the threads (opening the
    /// store, reading the accounts, reading each provider's recorded active account and, for
    /// `Scheduled`, the provider and the store between the phases), or for §14.1's cancel
    /// token set during the collection: `Interrupted`, once every thread has joined and given
    /// back the slot it held unsent. A `Scheduled` collection interrupted in phase 1 starts no
    /// phase 2. IDs that name no account are skipped. Never creates the store.
    pub fn collect_usage(&self, mode: CollectMode) -> Result<CollectReport, EngineError> {
        hooks::point(self, "usage-collect-start")?;
        let Some(shared) = self.existing_store()? else {
            return Ok(CollectReport::default());
        };
        let store: &Store = &shared;
        match mode {
            CollectMode::OnDemand { accounts } => {
                let settings = self.settings();
                let policy = Policy {
                    eligibility: Eligibility::OnDemand,
                    threshold: settings.threshold,
                    models: &settings.models,
                };
                self.collect_listed(store, &accounts, policy)
            }
            CollectMode::Recheck {
                accounts,
                threshold,
                models,
            } => {
                let policy = Policy {
                    eligibility: Eligibility::Recheck,
                    threshold,
                    models: &models,
                };
                self.collect_listed(store, &accounts, policy)
            }
            CollectMode::Scheduled {
                provider,
                threshold,
                models,
            } => self.collect_scheduled(store, &provider, threshold, &models),
        }
    }

    /// `OnDemand` and `Recheck`: every listed account at once.
    fn collect_listed(
        &self,
        store: &Store,
        accounts: &[AccountId],
        policy: Policy<'_>,
    ) -> Result<CollectReport, EngineError> {
        let mut rows = Vec::new();
        for id in accounts {
            if let Some(row) = store.account(id)? {
                rows.push(row);
            }
        }
        let roles = self.roles(store, &rows)?;
        let all: Vec<&AccountRow> = rows.iter().collect();
        let results = self.collect_each(store, &all, &roles, policy);
        // §14.1: a collection the token was set during is the command's interruption, whatever
        // each account did. A request already sent was recorded as usual; nothing else was.
        self.check_cancel()?;
        let mut report = CollectReport::default();
        report.add(results);
        Ok(report)
    }

    /// `Scheduled` (§8.6): phase 1, then phase 2 once phase 1 has recorded, so the pick and
    /// the escalation read the live account's new reading. The live login is read once, before
    /// phase 1; a switch made meanwhile leaves its new live account a candidate, whose token
    /// the gate refuses to refresh while it may be live (§7.3 step 2).
    fn collect_scheduled(
        &self,
        store: &Store,
        provider: &ProviderId,
        threshold: f64,
        models: &[String],
    ) -> Result<CollectReport, EngineError> {
        let p = self.provider(provider)?;
        let policy = Policy {
            eligibility: Eligibility::Scheduled,
            threshold,
            models,
        };
        let rows = store.accounts(provider)?;
        let roles = self.roles(store, &rows)?;
        let live = rows.iter().find(|r| roles.live.contains(&r.id));
        let mut report = CollectReport::default();
        let first: Vec<&AccountRow> = live.into_iter().collect();
        let results = self.collect_each(store, &first, &roles, policy);
        self.check_cancel()?;
        report.add(results);
        let picked =
            self.scheduled_candidates(store, p.as_ref(), &rows, live, threshold, models)?;
        let results = self.collect_each(store, &picked, &roles, policy);
        self.check_cancel()?;
        report.add(results);
        Ok(report)
    }

    /// §8.6 phase 2's pick, read from the store. The candidates are the provider's switchable
    /// accounts (§9.3, `switch_candidate`: never a session-owned one, which no switch may
    /// target, §11.2 step 7) other than the live one, whose kind has usage (a managed key has
    /// none, §13.2). One is due as `reserve_usage` would find it for a scheduled caller, leaving
    /// the lease and the budget to the reservation: not in backoff, and a poll due or no
    /// reading yet. Escalation reads the live account's decision-grade reading under `models`
    /// (§8.4); without a managed live account, or without such a reading, the headroom is
    /// unknown and the tick escalates.
    fn scheduled_candidates<'r>(
        &self,
        store: &Store,
        p: &dyn Provider,
        rows: &'r [AccountRow],
        live: Option<&AccountRow>,
        threshold: f64,
        models: &[String],
    ) -> Result<Vec<&'r AccountRow>, EngineError> {
        if !p.capabilities().usage {
            return Ok(Vec::new());
        }
        let budget = p.poll_budget();
        let now_s = self.now_ms().div_euclid(1000);
        let mut cands = Vec::new();
        for row in rows {
            let is_live = live.is_some_and(|l| l.id == row.id);
            if is_live
                || p.kind_traits(&row.kind).managed_key_axis
                || !self.switch_candidate(p, row)?
            {
                continue;
            }
            let state = store.usage_state(&row.id)?;
            let (fetched_at, backoff_until, next_poll_at) = state.map_or((None, None, None), |s| {
                (s.fetched_at, s.backoff_until, s.next_poll_at)
            });
            cands.push(DueCandidate {
                position: row.position,
                due: !backoff_holds(backoff_until, now_s)
                    && Eligibility::Scheduled.allows(fetched_at, next_poll_at, now_s, &budget),
                fetched_at: fetched_at.filter(|t| !is_future_stamped(*t, now_s)),
            });
        }
        let live_pct = match live {
            Some(row) => self
                .decision_windows(row, models)?
                .and_then(|w| max_relevant_pct(&w, models)),
            None => None,
        };
        let escalate = escalates(live_pct, threshold, budget.escalation_margin);
        Ok(scheduled_pick(&cands, escalate)
            .into_iter()
            .filter_map(|position| rows.iter().find(|r| r.position == position))
            .collect())
    }

    /// The roles every thread needs, read before any starts. Each provider's recorded active
    /// account is read before its live login: a switch writes the live login before it commits
    /// the record, so a record read first can only be older than the live role, never newer
    /// (`Collection::active_now`).
    fn roles(&self, store: &Store, rows: &[AccountRow]) -> Result<Roles, EngineError> {
        let mut recorded: HashMap<ProviderId, Option<AccountId>> = HashMap::new();
        for row in rows {
            if !recorded.contains_key(&row.provider) {
                recorded.insert(row.provider.clone(), store.active(&row.provider)?);
            }
        }
        hooks::point(self, "usage-roles-between-reads")?;
        Ok(Roles {
            recorded,
            live: self.live_accounts(rows),
        })
    }

    /// Each of `rows` on its own thread (§8.3), waiting for them all; each result with its row.
    /// Every row's role (`role_of`) is decided before the first thread starts. A profile read
    /// never fails it: `session_state` makes every I/O failure a state, and one that cannot be
    /// read counts as session-owned (§12.6). Should `role_of` ever fail, that is the account's
    /// error.
    fn collect_each<'r>(
        &self,
        store: &Store,
        rows: &[&'r AccountRow],
        roles: &Roles,
        policy: Policy<'_>,
    ) -> Vec<(&'r AccountRow, Result<Outcome, EngineError>)> {
        let roles_of: Vec<Result<Role, EngineError>> = rows
            .iter()
            .map(|row| self.role_of(row, roles.live.contains(&row.id)))
            .collect();
        thread::scope(|s| {
            let running: Vec<_> = rows
                .iter()
                .zip(roles_of)
                .map(|(&row, role)| {
                    let started_with = roles.recorded[&row.provider].clone();
                    let thread = s.spawn(move || {
                        role.and_then(|role| {
                            self.collect_one(store, row, role, started_with, policy)
                        })
                    });
                    (row, thread)
                })
                .collect();
            running
                .into_iter()
                .map(|(row, t)| match t.join() {
                    Ok(result) => (row, result),
                    Err(panic) => std::panic::resume_unwind(panic),
                })
                .collect()
        })
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

    /// §8.1's role for `row`. The account the live login names is the active one, inside a run
    /// shell too, since the live login is always the default home's (§12.8). Any other account
    /// that a session owns, or may own (a reservation or record that cannot be read, §12.6),
    /// takes the session branch with its profile; the rest are inactive. An account that
    /// `collect_one` reports as unsupported needs no session state. The state is computed for
    /// this call and never cached (Decision 8).
    fn role_of(&self, row: &AccountRow, live: bool) -> Result<Role, EngineError> {
        if live {
            return Ok(Role::Active);
        }
        let Some(p) = self.registry.get(&row.provider) else {
            return Ok(Role::Inactive);
        };
        if !p.capabilities().usage || p.kind_traits(&row.kind).managed_key_axis {
            return Ok(Role::Inactive);
        }
        let state = self.session_state(p.as_ref(), row)?;
        Ok(match state.profile() {
            Some(profile) if state.owned() => Role::Session {
                profile: profile.to_path_buf(),
            },
            _ => Role::Inactive,
        })
    }

    /// One account through §8.3's three phases, in the role `collect_each` decided. Afterwards
    /// the account is read again: a quarantine now was set while this collection ran, since
    /// `reserve_usage` refuses a quarantined account.
    fn collect_one(
        &self,
        store: &Store,
        row: &AccountRow,
        role: Role,
        recorded_active: Option<AccountId>,
        policy: Policy<'_>,
    ) -> Result<Outcome, EngineError> {
        let Some(provider) = self.registry.get(&row.provider) else {
            return Ok(Outcome::unreserved(Collected::Unsupported));
        };
        let p = provider.as_ref();
        if !p.capabilities().usage || p.kind_traits(&row.kind).managed_key_axis {
            return Ok(Outcome::unreserved(Collected::Unsupported));
        }
        let budget = p.poll_budget();
        // §14.1's first cancellation point: once the token is set, nothing is reserved.
        if let Some(signal) = self.cancel().requested() {
            return Err(EngineError::Interrupted(signal));
        }
        // Phase 1: eligibility, the lease and the slot, in one transaction.
        let reservation =
            match store.reserve_usage(row, self.now_ms(), policy.eligibility, &budget)? {
                Reserve::Reserved(r) => r,
                Reserve::Ineligible(why) => {
                    return Ok(Outcome::unreserved(Collected::Ineligible(why)));
                }
                Reserve::OverBudget { next_free_at } => {
                    return Ok(Outcome::unreserved(Collected::OverBudget { next_free_at }));
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
            role,
            budget,
            threshold: policy.threshold,
            models: policy.models,
            slot: Some(slot),
            rejected: state.as_ref().and_then(|s| s.rejected_fp.clone()),
            gated: false,
            recorded_active,
            state,
            reservation,
            warnings: Vec::new(),
        };
        // Phase 2, holding no lock but those the gate or §7.5 take for their own refresh; the
        // session branch takes none at all (§8.1).
        let fetched = match run.role.clone() {
            Role::Active => run.active(),
            Role::Inactive => run.inactive(),
            Role::Session { profile } => run.session(&profile),
        };
        // Phase 3.
        let (collected, warnings) = run.record(fetched)?;
        let quarantined = store
            .account(&row.id)?
            .is_some_and(|r| r.quarantine_reason.is_some());
        Ok(Outcome {
            collected,
            warnings,
            quarantined,
        })
    }
}

/// One account's fetch, from its reservation to its record.
struct Collection<'a> {
    engine: &'a Engine,
    store: &'a Store,
    p: &'a dyn Provider,
    row: &'a AccountRow,
    /// Which token this fetch reads (§8.1), as `collect_each` decided it.
    role: Role,
    budget: PollBudget,
    /// The threshold and models the next plan is made for (`Policy`).
    threshold: f64,
    models: &'a [String],
    reservation: Reservation,
    /// The slot reserved for the next request, while that request is unsent.
    /// `send_credential` hands it to the store's authorization (`authorize`) and puts it back
    /// only if the request never left; one still here at the record is given back (§8.3).
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
    /// §14.1: the cancel token was set at a cancellation point, or a lock wait on the way met
    /// it. Not a usage failure: nothing is recorded (no failure count, no backoff), and the
    /// unsent slot goes back. The lease is left to expire, as after any record. `collect_usage`
    /// reports the whole collection as interrupted, never this account as failed.
    Interrupted(i32),
}

/// An error that carries a signal is the fetch's interruption; any other is an error.
impl From<EngineError> for Stop {
    fn from(e: EngineError) -> Self {
        match e.signal() {
            Some(signal) => Stop::Interrupted(signal),
            None => Stop::Error(e),
        }
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

    /// §14.1's cancellation point before a usage request, or before a refresh started for one.
    fn interruption(&self) -> Result<(), Stop> {
        match self.engine.cancel().requested() {
            Some(signal) => Err(Stop::Interrupted(signal)),
            None => Ok(()),
        }
    }

    /// A refresh's error as the fetch's stop. A lock wait inside the refresh that met the token
    /// is the fetch's interruption; any other error is a failure, with a warning.
    fn refresh_error(&mut self, e: EngineError) -> Stop {
        match e.signal() {
            Some(signal) => Stop::Interrupted(signal),
            None => {
                self.warn_refresh(&e);
                failed("refresh-failed")
            }
        }
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

    /// §8.1 for a session-owned account (§12.5). The agent in the session owns the profile's
    /// token, so this reads it without a lock (`profile_credential`), and never refreshes,
    /// writes or retries it. An expired token is `token-expired` with no request, and a refused
    /// one stays refused until its bytes change. Neither leaves the machine, and the slot goes
    /// back (§8.3). A 401 stamps `rejected_fp` and ends the fetch.
    fn session(&mut self, profile: &Path) -> Result<Vec<Window>, Stop> {
        let credential = self.profile_credential(profile)?;
        if self.is_rejected(credential.bytes()) {
            return Err(failed(self.refusal_kind()));
        }
        let first = self.send_credential(&credential)?;
        if !matches!(first, UsageResult::Unauthorized) {
            return windows(first);
        }
        self.reject(credential.bytes())?;
        Err(failed(self.refusal_kind()))
    }

    /// The profile's credential as the agent in the session reads it (§8.1, §12.2), with no
    /// lock: its hashed item under the spelling the marker records, never one derived again,
    /// and its files in `profile`, where it is now (Decision 19).
    /// - A marker that is missing, unreadable, or another account's names no spelling, so the
    ///   credential cannot be read: `keychain-unavailable` (Decision 9).
    /// - The identity comes first: the credential of a profile whose login is not the
    ///   account's is never read (§12.5 "Identity drift"). An identity that is absent or
    ///   cannot be read cannot be confirmed, so it counts as drifted: `profile-drifted`.
    /// - A degraded read is used, since a usage request consumes nothing. An unreadable one is
    ///   `keychain-unavailable`, and an absent or empty one `no-access-token`.
    fn profile_credential(&self, profile: &Path) -> Result<Credential, Stop> {
        let env = &self.engine.env;
        let spelling = match ProfileMarker::read(profile) {
            Read::Present(m) if m.provider == self.row.provider && m.account_id == self.row.id => {
                m.config_dir
            }
            Read::Present(_) | Read::Absent | Read::Unreadable(_) => {
                return Err(failed("keychain-unavailable"));
            }
        };
        match self.p.profile_identity(env, profile) {
            Read::Present(identity) if !identity_drifted(&identity, self.row) => {}
            Read::Present(_) | Read::Absent | Read::Unreadable(_) => {
                return Err(failed("profile-drifted"));
            }
        }
        match self.p.read_profile_credential(env, profile, &spelling) {
            Read::Present(c) if !c.is_empty() => Ok(c),
            Read::Present(_) | Read::Absent => Err(failed("no-access-token")),
            Read::Unreadable(_) => Err(failed("keychain-unavailable")),
        }
    }

    /// The failure a refused token records wherever this collection may not refresh it (§8.1):
    /// `token-expired` when its kind refreshes and someone else refreshes it (§7.5 for the
    /// active account, the agent in the session for a session-owned one), otherwise `http-401`
    /// (Decision 11: a setup token never refreshes).
    fn refusal_kind(&self) -> &'static str {
        let refreshed_elsewhere = matches!(self.role, Role::Active | Role::Session { .. });
        if refreshed_elsewhere && self.p.kind_traits(&self.row.kind).refreshable {
            "token-expired"
        } else {
            "http-401"
        }
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
    /// refused. `Dead` has quarantined the account (`relogin_required`). `Replaced` is
    /// `live-replaced` (Decision 10: `unavailable`), with a warning naming the forced switch
    /// that activates the replacement (§7.5 step 2). Any other outcome, and any error, is a failure with a warning, never a command error (M2a Task 16's
    /// carry-over), except a live credential the oracle gives to another identity, which has a
    /// status of its own, and §14.1's interruption: no refresh starts once the token is set,
    /// and a lock wait inside §7.5 that meets it stops the fetch without a record. A kind that
    /// does not refresh never reaches §7.5, which would refuse it: its expired token is
    /// `token-expired`, and its refused one `http-401`, as on the inactive path (Decision 11:
    /// a refusal is an ordinary failure).
    fn refresh_live(&mut self, trigger: ActiveTrigger) -> Result<Vec<u8>, Stop> {
        self.interruption()?;
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
            Ok(ActiveOutcome::Replaced) => {
                self.warn_replaced();
                Err(failed("live-replaced"))
            }
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
            Err(e) => Err(self.refresh_error(e)),
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
    /// the fetch before anything is sent (§8.1). No refresh starts once the cancel token is set
    /// (§14.1); one already started runs to its end, its successor persisted.
    fn gate(&mut self, snapshot: &[u8]) -> Result<Vec<u8>, Stop> {
        hooks::point(self.engine, "usage-before-gate")?;
        self.interruption()?;
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
            Err(e) => Err(self.refresh_error(e)),
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
            Err(
                e @ (EngineError::InterruptedSwitch(_)
                | EngineError::RecoveryBlocked { .. }
                | EngineError::RecoveryMoved { .. }),
            ) => {
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

    /// One usage request with `bytes`, which a fresh read gave: the vault, the gate or the live
    /// store (`live_bytes` refuses a degraded one). See `send_credential`.
    fn send(&mut self, bytes: &[u8]) -> Result<UsageResult, Stop> {
        self.send_credential(&Credential::fresh(bytes.to_vec()))
    }

    /// One usage request with `credential`'s access token, its provenance kept: only a
    /// session's profile read may be degraded, and a usage request consumes nothing (§8.1). A
    /// token that has expired or was refused is never sent: the slot stays unsent. Right before
    /// the request, the store authorizes it and hands over the slot to send under
    /// (`authorize`). A request that never left (no access token, or a pre-send failure) puts
    /// its slot back.
    fn send_credential(&mut self, credential: &Credential) -> Result<UsageResult, Stop> {
        let bytes = credential.bytes();
        // §14.1: every request, the 401 retry included, is preceded by a cancellation point.
        self.interruption()?;
        if !self.usable(bytes) {
            return Err(failed("token-expired"));
        }
        hooks::point(self.engine, "usage-before-send")?;
        let slot = self.authorize(bytes)?;
        // §14.1: the last cancellation point, after the store's authorization (which may have
        // waited on SQLite, and passes the `usage-before-send` hook's window). The slot was
        // never sent: it stays held for `record` to give back.
        if let Err(stop) = self.interruption() {
            self.slot = Some(slot);
            return Err(stop);
        }
        let result = self.p.fetch_usage(self.engine.http(), credential);
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
    ///   `refusal_kind` says: `token-expired` for a refreshable active or session-owned account
    ///   (§7.5, or the agent in the session, refreshes it), otherwise `http-401`.
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
                Err(failed(self.refusal_kind()))
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

    /// §7.5 step 2: the live store holds a lineage an explicit replacement superseded. Names
    /// the account by label and position, never a token, and the command that activates the
    /// replacement.
    fn warn_replaced(&mut self) {
        self.warnings.push(format!(
            "{label} (position {n})'s login was replaced while Claude Code kept the old one; run `tagteam switch {n} --force` to activate the replacement",
            label = self.row.label,
            n = self.row.position
        ));
    }

    fn warn_refresh(&mut self, detail: &dyn fmt::Display) {
        self.warnings.push(format!(
            "could not refresh {} (position {}) to read its usage: {detail}",
            self.row.label, self.row.position
        ));
    }

    /// §8.6's next plan after a success, from the previous reading and this one, under the
    /// collection's threshold and models.
    fn plan(&self, windows: &[Window], active: bool, now_s: i64) -> PollPlan {
        let models = self.models;
        let prev = self.state.as_ref();
        let inputs = PollInputs {
            now_s,
            active,
            pct: max_relevant_pct(windows, models),
            prev_pct: prev
                .and_then(|s| s.last_good.as_deref())
                .and_then(|w| max_relevant_pct(w, models)),
            prev_interval_s: prev.and_then(|s| s.poll_interval_s),
            threshold: self.threshold,
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
    fn record(
        mut self,
        fetched: Result<Vec<Window>, Stop>,
    ) -> Result<(Collected, Vec<String>), EngineError> {
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
            _ => self.role == Role::Active,
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
            // §14.1: no record at all. As after an error, `record` gives the slot of the
            // request that never left back, best effort: a release that fails never masks the
            // interruption.
            Err(Stop::Interrupted(signal)) => return Err(EngineError::Interrupted(signal)),
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
