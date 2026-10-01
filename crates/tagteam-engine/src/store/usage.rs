//! The usage tables (§6.1): each account's usage state, the hourly request budget, samples,
//! the `usage:<id>` leases and the live-identity cache. Usage columns hold epoch seconds, and
//! lease expiries hold epoch milliseconds (Decision 1).

use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use serde_json::Value;
use tagteam_core::backoff::{CAP_429_S, failure_backoff_s};
use tagteam_core::poll::budget_next_free;
use tagteam_core::trust::{FUTURE_STAMP_SLACK_S, is_future_stamped};
use tagteam_core::usage::{windows_from_json, windows_to_json};
use tagteam_core::{AccountId, PollBudget, PollPlan, ProviderId, Sample, Window};

use super::{AccountRow, Store, StoreError};

/// A usage lease lives 90 s (§8.3).
const USAGE_LEASE_MS: i64 = 90_000;

/// Whether a stored `backoff_until` is further ahead of `now_s` than any legal backoff
/// (`MAX_BACKOFF_S`) plus the slack: written under a clock that ran ahead (§8.4). Shared by
/// `reserve_usage` and the views, so what reserving ignores is never shown as a retry time.
pub(crate) fn backoff_is_skewed(until: i64, now_s: i64) -> bool {
    until.saturating_sub(now_s) > MAX_BACKOFF_S + FUTURE_STAMP_SLACK_S
}

/// Whether a stored `next_poll_at` is further ahead of `now_s` than any legal plan plus the
/// slack (§8.4), as `backoff_is_skewed` for the schedule. The longest legal plan is an
/// over-budget `next_free_at` (`count_window_s`) or a post-429 interval with jitter
/// (`post_429_max_s` plus `jitter_frac`); the bound is only sound while the first covers the
/// second.
pub(crate) fn plan_is_skewed(at: i64, now_s: i64, budget: &PollBudget) -> bool {
    debug_assert!(
        budget.count_window_s as f64 >= budget.post_429_max_s as f64 * (1.0 + budget.jitter_frac),
        "the next_poll_at skew bound must cover the longest legal plan"
    );
    at.saturating_sub(now_s) > budget.count_window_s + FUTURE_STAMP_SLACK_S
}

/// The lease row that spaces retention prunes at least a day apart (Decision 7).
const PRUNE_LEASE: &str = "prune:usage_samples";

const DAY_S: i64 = 86_400;

/// The longest backoff any failure legally sets: §8.5's 429 cap. A `backoff_until` further
/// ahead than this plus `FUTURE_STAMP_SLACK_S` was written under a clock that ran ahead, so no
/// legal schedule reaches it: reserving ignores it (§8.4).
const MAX_BACKOFF_S: i64 = CAP_429_S;

/// §8.3's `last_error` token for a request the hourly budget refused (§8.6).
const OVER_BUDGET: &str = "over-budget";

/// §6.1's lease statement. The lease is held when it changes one row: it was free, or its
/// holder's expiry (`?4`, now in ms) has passed.
const TAKE_LEASE_SQL: &str = "INSERT INTO leases (name, holder, expires_at) VALUES (?1, ?2, ?3) \
    ON CONFLICT(name) DO UPDATE SET holder = excluded.holder, expires_at = excluded.expires_at \
    WHERE leases.expires_at <= ?4";

/// One account's `usage_state` row. Times are epoch seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageStateRow {
    pub account_id: AccountId,
    /// The last successful reading. `None` when there is none, when the reading had no
    /// windows (§8.2, including a stored `[]`), or when the stored JSON is corrupt: a bad row
    /// reads as no reading.
    pub last_good: Option<Vec<Window>>,
    /// Set by success only.
    pub fetched_at: Option<i64>,
    pub last_attempt_at: Option<i64>,
    pub consecutive_failures: u32,
    /// A kind token (§6.1, §8.3, Decision 10).
    pub last_error: Option<String>,
    pub backoff_until: Option<i64>,
    pub next_poll_at: Option<i64>,
    pub poll_interval_s: Option<i64>,
    /// When the last 429's backoff lifts (Decision 2). Success never clears it.
    pub last_429_at: Option<i64>,
    /// The access-token fingerprint a 401 refused (§8.1).
    pub rejected_fp: Option<String>,
}

/// A reservation (§8.3 phase 1): the `usage:<id>` lease and the first budget slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub account_id: AccountId,
    /// The account's provider: with `identity_key`, the budget's key (§8.6), and part of every
    /// slot's full identity.
    pub provider: ProviderId,
    /// The account's identity when it was reserved; the record is fenced by it.
    pub identity_key: String,
    /// Random per acquisition (UUIDv7): the lease row names it while the lease is ours.
    pub holder: String,
    /// The `usage_requests` rowid of the first slot.
    pub slot: i64,
    /// When the first slot was reserved, in epoch seconds.
    pub slot_at: i64,
}

/// Which §8.3 caller reserves, and so when an account's reading may be fetched again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eligibility {
    /// `list`, `status` and `switch`: the reading is older than the floor (180 s), and a poll
    /// is due, where no plan counts as due.
    OnDemand,
    /// An auto tick (§8.6): a poll is due, or there is no reading yet.
    Scheduled,
    /// Consume-first's re-check (§11.2 step 8): the reading is older than the floor, whatever
    /// the plan says.
    Recheck,
}

impl Eligibility {
    /// The schedule half of §8.3's eligibility, from the stored `fetched_at` and `next_poll_at`.
    /// A reading stamped more than `FUTURE_STAMP_SLACK_S` ahead of now has no usable age
    /// (§8.4): it counts as unread, so it cannot lock the account out until the clock catches
    /// up. The same skew leaves a `next_poll_at` further ahead than any legal plan, which
    /// counts as due.
    pub(crate) fn allows(
        self,
        fetched_at: Option<i64>,
        next_poll_at: Option<i64>,
        now_s: i64,
        budget: &PollBudget,
    ) -> bool {
        let fetched_at = fetched_at.filter(|t| !is_future_stamped(*t, now_s));
        let due = next_poll_at.is_none_or(|t| t <= now_s || plan_is_skewed(t, now_s, budget));
        let older_than_floor = fetched_at.is_none_or(|t| now_s - t > budget.floor_s);
        match self {
            Eligibility::OnDemand => due && older_than_floor,
            Eligibility::Scheduled => due || fetched_at.is_none(),
            Eligibility::Recheck => older_than_floor,
        }
    }
}

/// Whether a stored `backoff_until` still holds at `now_s`. A failure recorded while the clock
/// ran ahead leaves a backoff no legal schedule reaches (`backoff_is_skewed`), which must not
/// lock the account out until the clock catches up (§8.4).
pub(crate) fn backoff_holds(until: Option<i64>, now_s: i64) -> bool {
    until.is_some_and(|t| t > now_s && !backoff_is_skewed(t, now_s))
}

/// Why an account was not reserved (§8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ineligible {
    Quarantined,
    Backoff,
    Leased,
    NotDue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reserve {
    Reserved(Reservation),
    Ineligible(Ineligible),
    /// The identity has spent its hourly budget (§8.6): no lease was taken, the refusal is
    /// recorded as an `over-budget` failure, and the account's `next_poll_at` now says when a
    /// slot frees.
    OverBudget {
        next_free_at: i64,
    },
}

/// A budget slot under a held reservation. Its full identity is its rowid and time plus the
/// reservation's `provider` and `identity_key`: SQLite reuses a pruned row's rowid, so the rowid
/// alone does not name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// The `usage_requests` rowid.
    pub slot: i64,
    /// When it was reserved, in epoch seconds (the row's `at`).
    pub slot_at: i64,
}

/// What `authorize_send` allows right before a request (§8.3, §8.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendGrant {
    /// Send now, under this slot.
    Send(Slot),
    /// The token about to be sent is the one the server refused (`rejected_fp`, §8.1).
    Rejected,
    /// The lease row no longer names this holder, the account's identity changed, or the
    /// account is quarantined: send nothing, record nothing.
    LeaseLost,
    /// No slot is free in the identity's hourly budget until `next_free_at`.
    OverBudget { next_free_at: i64 },
}

/// The live identity as of one version of the provider's source file (§13.5). The file wins
/// if they disagree: a row whose `path`, `mtime_ns` or `size` differs from the file is stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveIdentityCacheRow {
    pub provider: ProviderId,
    pub path: String,
    pub mtime_ns: i64,
    pub size: i64,
    /// `None`: that version of the file held no live login.
    pub identity_key: Option<String>,
    pub label: Option<String>,
    pub account_uuid: Option<String>,
}

/// One window's samples since a time: a range on the primary key (account, window, time), so
/// no scan and no sort.
const SAMPLES_OF_WINDOW: &str = "SELECT window, fetched_at, pct, resets_at FROM usage_samples \
     WHERE account_id = ?1 AND window = ?2 AND fetched_at >= ?3 ORDER BY fetched_at";

/// Every window's samples since a time.
const SAMPLES_OF_ALL: &str = "SELECT window, fetched_at, pct, resets_at FROM usage_samples \
     WHERE account_id = ?1 AND fetched_at >= ?2 ORDER BY fetched_at, window";

pub(super) fn lease_name(id: &AccountId) -> String {
    format!("usage:{id}")
}

fn state_from_row(r: &Row<'_>) -> rusqlite::Result<UsageStateRow> {
    let last_good: Option<String> = r.get("last_good")?;
    let failures: i64 = r.get("consecutive_failures")?;
    Ok(UsageStateRow {
        account_id: AccountId::from_string(r.get::<_, String>("account_id")?),
        last_good: last_good
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| windows_from_json(&v))
            .filter(|w| !w.is_empty()),
        fetched_at: r.get("fetched_at")?,
        last_attempt_at: r.get("last_attempt_at")?,
        consecutive_failures: u32::try_from(failures.max(0)).unwrap_or(u32::MAX),
        last_error: r.get("last_error")?,
        backoff_until: r.get("backoff_until")?,
        next_poll_at: r.get("next_poll_at")?,
        poll_interval_s: r.get("poll_interval_s")?,
        last_429_at: r.get("last_429_at")?,
        rejected_fp: r.get("rejected_fp")?,
    })
}

/// Creates the account's `usage_state` row when it has none. `NoSuchAccount` when the account
/// itself is gone.
fn ensure_state(c: &Connection, id: &AccountId) -> Result<(), StoreError> {
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = ?1)",
        [id.as_str()],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(StoreError::NoSuchAccount);
    }
    c.execute(
        "INSERT OR IGNORE INTO usage_state (account_id) VALUES (?1)",
        [id.as_str()],
    )?;
    Ok(())
}

/// Prunes every `usage_requests` row that has left the count window (§8.6), then says when the
/// identity may next send: `None` when a slot is free now. A row counts while
/// `now_s − at < count_window_s`, as `budget_next_free` counts it.
fn next_free_at(
    c: &Connection,
    provider: &str,
    identity_key: &str,
    now_s: i64,
    budget: &PollBudget,
) -> rusqlite::Result<Option<i64>> {
    let left = now_s - budget.count_window_s;
    c.execute("DELETE FROM usage_requests WHERE at <= ?1", [left])?;
    let mut stmt = c.prepare(
        "SELECT at FROM usage_requests WHERE provider = ?1 AND identity_key = ?2 AND at > ?3 \
         ORDER BY at",
    )?;
    let counted = stmt
        .query_map(params![provider, identity_key, left], |r| {
            r.get::<_, i64>(0)
        })?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(budget_next_free(budget, &counted, now_s))
}

fn insert_slot(
    c: &Connection,
    provider: &str,
    identity_key: &str,
    now_s: i64,
) -> rusqlite::Result<Slot> {
    c.execute(
        "INSERT INTO usage_requests (provider, identity_key, at) VALUES (?1, ?2, ?3)",
        params![provider, identity_key, now_s],
    )?;
    Ok(Slot {
        slot: c.last_insert_rowid(),
        slot_at: now_s,
    })
}

/// Gives a slot back by its full identity, in one statement. `usage_requests` has no
/// `AUTOINCREMENT`, so SQLite gives a pruned row's rowid to a later insert; matching the
/// provider, identity key and reservation time as well means a stale handle deletes nothing
/// rather than another process's slot.
fn delete_slot(c: &Connection, r: &Reservation, slot: &Slot) -> rusqlite::Result<usize> {
    c.execute(
        "DELETE FROM usage_requests \
         WHERE rowid = ?1 AND provider = ?2 AND identity_key = ?3 AND at = ?4",
        params![slot.slot, r.provider.as_str(), r.identity_key, slot.slot_at],
    )
}

/// §8.3's fence: the lease row still names this holder (expired or not, §6.1), and the account
/// still has the identity it was reserved with.
fn fenced(c: &Connection, r: &Reservation) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM leases WHERE name = ?1 AND holder = ?2) \
         AND EXISTS(SELECT 1 FROM accounts WHERE id = ?3 AND identity_key = ?4)",
        params![
            lease_name(&r.account_id),
            r.holder,
            r.account_id.as_str(),
            r.identity_key
        ],
        |row| row.get(0),
    )
}

/// History retention (§6.1): samples older than `retention_days` go, at most once a day
/// (Decision 7).
fn prune_samples(
    c: &Connection,
    holder: &str,
    now_s: i64,
    retention_days: u32,
) -> rusqlite::Result<()> {
    let taken = c.execute(
        TAKE_LEASE_SQL,
        params![PRUNE_LEASE, holder, (now_s + DAY_S) * 1000, now_s * 1000],
    )?;
    if taken == 1 {
        c.execute(
            "DELETE FROM usage_samples WHERE fetched_at < ?1",
            [now_s - i64::from(retention_days) * DAY_S],
        )?;
    }
    Ok(())
}

// Every transaction here reads before it writes, so each is `IMMEDIATE` (Decision 8): it
// takes the write lock at `BEGIN`, waiting out `busy_timeout`, so two processes can never
// both read a count or a lease row before either writes. A `DEFERRED` one would also fail at
// once with `SQLITE_BUSY` when another connection committed between its read and its write,
// since WAL mode cannot wait that conflict out.
impl Store {
    /// The account's usage state; `None` until something records one.
    pub fn usage_state(&self, id: &AccountId) -> Result<Option<UsageStateRow>, StoreError> {
        Ok(self
            .lock()
            .query_row(
                "SELECT * FROM usage_state WHERE account_id = ?1",
                [id.as_str()],
                state_from_row,
            )
            .optional()?)
    }

    /// Phase 1 (§8.3), one `IMMEDIATE` transaction. The account is re-read inside it.
    ///
    /// Eligibility, in this order: not quarantined, not in backoff, no live lease, then the
    /// schedule, as `eligibility` reads it (`Eligibility::allows`). A clock that ran ahead when
    /// a record was written leaves times no legal schedule reaches, which count as clock skew
    /// and are ignored: a `fetched_at` more than `FUTURE_STAMP_SLACK_S` ahead counts as no
    /// reading, a `next_poll_at` more than `count_window_s` plus the slack ahead counts as due,
    /// and a `backoff_until` more than `MAX_BACKOFF_S` plus the slack ahead is no backoff
    /// (§8.4). An eligible account then needs a free slot in its identity's hourly budget
    /// (§8.6). Over budget, the fetch reports `over-budget`: the refusal is recorded as the
    /// collector records `authorize_send`'s, one more consecutive failure with
    /// `last_error = over-budget`, `last_attempt_at` now and a backoff until
    /// `max(now + §8.5's base, next_free_at)`, so an account with no reading shows why; its
    /// `next_poll_at` moves to `next_free_at`, the reading is never touched, and no lease is
    /// taken. The backoff is checked before the budget, so a refusal is recorded once per
    /// budget period. Otherwise the lease (§6.1's statement, 90 s) and one `usage_requests`
    /// slot are taken together.
    pub fn reserve_usage(
        &self,
        account: &AccountRow,
        now_ms: i64,
        eligibility: Eligibility,
        budget: &PollBudget,
    ) -> Result<Reserve, StoreError> {
        let now_s = now_ms.div_euclid(1000);
        let id = &account.id;
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (provider, identity_key, quarantined): (String, String, bool) = tx
            .query_row(
                "SELECT provider, identity_key, quarantine_reason IS NOT NULL FROM accounts \
                 WHERE id = ?1",
                [id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or(StoreError::NoSuchAccount)?;
        if quarantined {
            return Ok(Reserve::Ineligible(Ineligible::Quarantined));
        }
        type Schedule = (Option<i64>, Option<i64>, Option<i64>, i64);
        let (fetched_at, backoff_until, next_poll_at, failures): Schedule = tx
            .query_row(
                "SELECT fetched_at, backoff_until, next_poll_at, consecutive_failures \
                 FROM usage_state WHERE account_id = ?1",
                [id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?
            .unwrap_or_default();
        if backoff_holds(backoff_until, now_s) {
            return Ok(Reserve::Ineligible(Ineligible::Backoff));
        }
        let name = lease_name(id);
        let leased: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM leases WHERE name = ?1 AND expires_at > ?2)",
            params![name, now_ms],
            |r| r.get(0),
        )?;
        if leased {
            return Ok(Reserve::Ineligible(Ineligible::Leased));
        }
        if !eligibility.allows(fetched_at, next_poll_at, now_s, budget) {
            return Ok(Reserve::Ineligible(Ineligible::NotDue));
        }
        if let Some(next_free_at) = next_free_at(&tx, &provider, &identity_key, now_s, budget)? {
            let n = u32::try_from(failures.max(0))
                .unwrap_or(u32::MAX)
                .saturating_add(1);
            let until = (now_s + failure_backoff_s(n, false, None)).max(next_free_at);
            ensure_state(&tx, id)?;
            tx.execute(
                "UPDATE usage_state SET consecutive_failures = consecutive_failures + 1, \
                 last_error = ?2, last_attempt_at = ?3, backoff_until = ?4, next_poll_at = ?5 \
                 WHERE account_id = ?1",
                params![id.as_str(), OVER_BUDGET, now_s, until, next_free_at],
            )?;
            tx.commit()?;
            return Ok(Reserve::OverBudget { next_free_at });
        }
        let holder = uuid::Uuid::now_v7().to_string();
        let taken = tx.execute(
            TAKE_LEASE_SQL,
            params![name, holder, now_ms + USAGE_LEASE_MS, now_ms],
        )?;
        if taken != 1 {
            return Ok(Reserve::Ineligible(Ineligible::Leased));
        }
        let slot = insert_slot(&tx, &provider, &identity_key, now_s)?;
        tx.commit()?;
        Ok(Reserve::Reserved(Reservation {
            account_id: id.clone(),
            provider: ProviderId::new(provider),
            identity_key,
            holder,
            slot: slot.slot,
            slot_at: slot.slot_at,
        }))
    }

    /// Right before each request (§8.3, §8.6), in one `IMMEDIATE` transaction, so nothing is
    /// sent on a stale view of the store. In order:
    /// - `r` must still hold the `usage:<id>` lease (its row names `r.holder`) and the account
    ///   must still have `r.identity_key` and not be quarantined; otherwise `LeaseLost`, and
    ///   nothing is written.
    /// - `access_fp`, the fingerprint of the exact bytes about to be sent, must not equal the
    ///   durable `rejected_fp` (§8.1), which another holder may have stamped since the caller
    ///   read it; otherwise `Rejected`, and nothing is written.
    /// - Then the slot to send under: `slot` itself while it is less than `slot_valid_s` old
    ///   in whole seconds, so the request leaves less than `slot_valid_s` after the instant it
    ///   was reserved, and stays counted for at least the hour after it is sent (§8.6);
    ///   otherwise a fresh one, after giving the stale one back by its full identity; a fresh
    ///   one when `slot` is `None` (the 401 retry, whose first slot was sent). Counted against
    ///   the reservation's `(provider, identity_key)`. `OverBudget` when no fresh slot is free;
    ///   the stale one has still been given back.
    pub fn authorize_send(
        &self,
        r: &Reservation,
        slot: Option<&Slot>,
        access_fp: Option<&str>,
        now_ms: i64,
        budget: &PollBudget,
    ) -> Result<SendGrant, StoreError> {
        let now_s = now_ms.div_euclid(1000);
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let quarantined: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = ?1 AND quarantine_reason IS NOT NULL)",
            [r.account_id.as_str()],
            |row| row.get(0),
        )?;
        if quarantined || !fenced(&tx, r)? {
            return Ok(SendGrant::LeaseLost);
        }
        if let Some(fp) = access_fp {
            let rejected: Option<String> = tx
                .query_row(
                    "SELECT rejected_fp FROM usage_state WHERE account_id = ?1",
                    [r.account_id.as_str()],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            if rejected.as_deref() == Some(fp) {
                return Ok(SendGrant::Rejected);
            }
        }
        if let Some(held) = slot {
            if now_s - held.slot_at < budget.slot_valid_s {
                return Ok(SendGrant::Send(held.clone()));
            }
            delete_slot(&tx, r, held)?;
        }
        let provider = r.provider.as_str();
        let grant = match next_free_at(&tx, provider, &r.identity_key, now_s, budget)? {
            Some(next_free_at) => SendGrant::OverBudget { next_free_at },
            None => SendGrant::Send(insert_slot(&tx, provider, &r.identity_key, now_s)?),
        };
        tx.commit()?;
        Ok(grant)
    }

    /// Gives a slot back (a fetch that ended before sending, §8.3) by its full identity, in one
    /// statement: a slot whose row was pruned deletes nothing, even when SQLite has since given
    /// its rowid to another slot.
    pub fn release_slot(&self, r: &Reservation, slot: &Slot) -> Result<(), StoreError> {
        delete_slot(&self.lock(), r, slot)?;
        Ok(())
    }

    /// Phase 3, success (§8.3). Fenced by the lease holder and the account's identity key:
    /// `Ok(false)` when the fence fails, and nothing is written. Writes the reading (a reading
    /// with no windows is stored as none, §8.2) and its `fetched_at` and `last_attempt_at`,
    /// resets the failure fields and `rejected_fp`, stores `plan`, inserts one sample per
    /// window, and prunes samples older than `retention_days` at most once a day (Decision 7).
    /// `last_429_at` is kept. The lease is left to expire.
    pub fn record_usage(
        &self,
        r: &Reservation,
        windows: &[Window],
        now_s: i64,
        plan: &PollPlan,
        retention_days: u32,
    ) -> Result<bool, StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !fenced(&tx, r)? {
            return Ok(false);
        }
        let id = r.account_id.as_str();
        let last_good = (!windows.is_empty()).then(|| windows_to_json(windows).to_string());
        ensure_state(&tx, &r.account_id)?;
        tx.execute(
            "UPDATE usage_state SET last_good = ?2, fetched_at = ?3, last_attempt_at = ?3, \
             consecutive_failures = 0, last_error = NULL, backoff_until = NULL, \
             next_poll_at = ?4, poll_interval_s = ?5, rejected_fp = NULL WHERE account_id = ?1",
            params![id, last_good, now_s, plan.next_poll_at, plan.interval_s],
        )?;
        for w in windows {
            tx.execute(
                "INSERT OR REPLACE INTO usage_samples (account_id, window, fetched_at, pct, resets_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, w.key, now_s, w.pct, w.resets_at],
            )?;
        }
        prune_samples(&tx, &r.holder, now_s, retention_days)?;
        tx.commit()?;
        Ok(true)
    }

    /// Phase 3, failure (§8.3), under the same fence. Never touches `last_good` or
    /// `fetched_at`. Counts the failure and sets `last_error` (a kind token),
    /// `last_attempt_at` and `backoff_until`, and `last_429_at` when given (Decision 2: when
    /// that 429's backoff lifts). `release`, the slot of a request that was never sent, is
    /// given back by its full identity in the same transaction. The plan is left as it was.
    pub fn record_usage_failure(
        &self,
        r: &Reservation,
        kind: &str,
        now_s: i64,
        backoff_until: i64,
        last_429_at: Option<i64>,
        release: Option<&Slot>,
    ) -> Result<bool, StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !fenced(&tx, r)? {
            return Ok(false);
        }
        ensure_state(&tx, &r.account_id)?;
        tx.execute(
            "UPDATE usage_state SET consecutive_failures = consecutive_failures + 1, \
             last_error = ?2, last_attempt_at = ?3, backoff_until = ?4, \
             last_429_at = COALESCE(?5, last_429_at) WHERE account_id = ?1",
            params![
                r.account_id.as_str(),
                kind,
                now_s,
                backoff_until,
                last_429_at
            ],
        )?;
        if let Some(slot) = release {
            delete_slot(&tx, r, slot)?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// Stamps (or with `None`, clears) the access-token fingerprint a 401 refused (§8.1), only
    /// while `r` holds the lease, under the records' fence: `Ok(false)` and nothing written
    /// otherwise, so a holder that lost its lease cannot overwrite the new holder's stamp.
    /// Creates the account's row if missing (a 401 can come on its first fetch).
    pub fn set_rejected_fp(&self, r: &Reservation, fp: Option<&str>) -> Result<bool, StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !fenced(&tx, r)? {
            return Ok(false);
        }
        ensure_state(&tx, &r.account_id)?;
        tx.execute(
            "UPDATE usage_state SET rejected_fp = ?2 WHERE account_id = ?1",
            params![r.account_id.as_str(), fp],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// §8.3's post-switch re-plan: sets the plan alone, creating the row if missing.
    pub fn set_poll_plan(&self, id: &AccountId, plan: &PollPlan) -> Result<(), StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_state(&tx, id)?;
        tx.execute(
            "UPDATE usage_state SET next_poll_at = ?2, poll_interval_s = ?3 WHERE account_id = ?1",
            params![id.as_str(), plan.next_poll_at, plan.interval_s],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The account's samples fetched at or after `since_s`, ascending by `fetched_at` (then
    /// window key); `window = None` means every window. One window is read along the primary
    /// key (account, window, time), never by scanning the account's samples.
    pub fn usage_samples(
        &self,
        id: &AccountId,
        window: Option<&str>,
        since_s: i64,
    ) -> Result<Vec<(String, Sample)>, StoreError> {
        let c = self.lock();
        let map = |r: &rusqlite::Row<'_>| {
            Ok((
                r.get::<_, String>(0)?,
                Sample {
                    fetched_at: r.get(1)?,
                    pct: r.get(2)?,
                    resets_at: r.get(3)?,
                },
            ))
        };
        let rows = match window {
            Some(w) => c
                .prepare(SAMPLES_OF_WINDOW)?
                .query_map(params![id.as_str(), w, since_s], map)?
                .collect::<Result<Vec<_>, _>>()?,
            None => c
                .prepare(SAMPLES_OF_ALL)?
                .query_map(params![id.as_str(), since_s], map)?
                .collect::<Result<Vec<_>, _>>()?,
        };
        Ok(rows)
    }

    /// Whether another fetch holds the account's lease at `now_ms` (§8.4's extended trust).
    pub fn usage_lease_live(&self, id: &AccountId, now_ms: i64) -> Result<bool, StoreError> {
        Ok(self.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM leases WHERE name = ?1 AND expires_at > ?2)",
            params![lease_name(id), now_ms],
            |r| r.get(0),
        )?)
    }

    /// When the account's usage lease expires, in epoch ms, while its row is there: a lease
    /// outlives the record it fenced until it expires (§8.3).
    pub fn usage_lease_expires_at(&self, id: &AccountId) -> Result<Option<i64>, StoreError> {
        Ok(self
            .lock()
            .query_row(
                "SELECT expires_at FROM leases WHERE name = ?1",
                [lease_name(id)],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The provider's cached live identity. A row missing its path, mtime or size reads as
    /// no row: the caller re-parses the file.
    pub fn live_identity_cache(
        &self,
        provider: &ProviderId,
    ) -> Result<Option<LiveIdentityCacheRow>, StoreError> {
        let row = self
            .lock()
            .query_row(
                "SELECT path, mtime_ns, size, identity_key, label, account_uuid \
                 FROM live_identity_cache WHERE provider = ?1",
                [provider.as_str()],
                |r| {
                    let key: (Option<String>, Option<i64>, Option<i64>) =
                        (r.get(0)?, r.get(1)?, r.get(2)?);
                    Ok((key, r.get(3)?, r.get(4)?, r.get(5)?))
                },
            )
            .optional()?;
        Ok(match row {
            Some(((Some(path), Some(mtime_ns), Some(size)), identity_key, label, account_uuid)) => {
                Some(LiveIdentityCacheRow {
                    provider: provider.clone(),
                    path,
                    mtime_ns,
                    size,
                    identity_key,
                    label,
                    account_uuid,
                })
            }
            _ => None,
        })
    }

    pub fn put_live_identity_cache(&self, row: &LiveIdentityCacheRow) -> Result<(), StoreError> {
        put_live_identity_cache(&self.lock(), row)
    }

    /// `put_live_identity_cache`, giving up after `wait` on another connection's write lock
    /// (`SQLITE_BUSY`) instead of the connection's own, far longer, wait: for a writer that is
    /// only a cache and must never hold its caller up.
    pub fn put_live_identity_cache_within(
        &self,
        row: &LiveIdentityCacheRow,
        wait: Duration,
    ) -> Result<(), StoreError> {
        let c = self.lock();
        c.busy_timeout(wait)?;
        let written = put_live_identity_cache(&c, row);
        c.busy_timeout(super::BUSY_TIMEOUT)?;
        written
    }
}

fn put_live_identity_cache(c: &Connection, row: &LiveIdentityCacheRow) -> Result<(), StoreError> {
    c.execute(
        "INSERT INTO live_identity_cache \
         (provider, path, mtime_ns, size, identity_key, label, account_uuid) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(provider) DO UPDATE SET path = excluded.path, \
         mtime_ns = excluded.mtime_ns, size = excluded.size, \
         identity_key = excluded.identity_key, label = excluded.label, \
         account_uuid = excluded.account_uuid",
        params![
            row.provider.as_str(),
            row.path,
            row.mtime_ns,
            row.size,
            row.identity_key,
            row.label,
            row.account_uuid
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_windows_samples_are_read_along_the_primary_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let c = store.lock();
        let mut stmt = c
            .prepare(&format!("EXPLAIN QUERY PLAN {SAMPLES_OF_WINDOW}"))
            .unwrap();
        let plan: Vec<String> = stmt
            .query_map(params!["a", "7d", 0], |r| r.get::<_, String>(3))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let plan = plan.join("\n");
        assert!(plan.contains("SEARCH usage_samples"), "{plan}");
        assert!(!plan.contains("SCAN"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    }
}
