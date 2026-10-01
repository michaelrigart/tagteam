//! §6.1, §8.3, §8.6: the store's usage tables. Usage times are epoch seconds and lease
//! expiries epoch milliseconds (Decision 1).

mod common;

use std::path::{Path, PathBuf};
use std::sync::Barrier;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::time::Duration;

use common::{add, cc, identity};
use rusqlite::{OptionalExtension, params};
use serde_json::json;
use tagteam_core::{AccountId, PollBudget, PollPlan, ProviderId, Sample, Window, WindowKind};
use tagteam_engine::store::{
    Ineligible, LiveIdentityCacheRow, Reservation, Reserve, SendGrant, Slot, Store, StoreError,
    UsageStateRow,
};

/// Now, in epoch seconds; `T_MS` is the same instant in milliseconds.
const T: i64 = 1_790_000_000;
const T_MS: i64 = T * 1000;
const B: PollBudget = PollBudget::STANDARD;

fn open() -> (tempfile::TempDir, PathBuf, Store) {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    (d, path, s)
}

/// A second, independent connection, to arrange and inspect rows the API has no call for.
fn raw(path: &Path) -> rusqlite::Connection {
    let c = rusqlite::Connection::open(path).unwrap();
    c.busy_timeout(Duration::from_secs(5)).unwrap();
    c
}

/// Arranges an account's `usage_state` schedule directly.
fn arrange(
    path: &Path,
    id: &AccountId,
    fetched_at: Option<i64>,
    backoff_until: Option<i64>,
    next_poll_at: Option<i64>,
) {
    raw(path)
        .execute(
            "INSERT INTO usage_state (account_id, fetched_at, backoff_until, next_poll_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params![id.as_str(), fetched_at, backoff_until, next_poll_at],
        )
        .unwrap();
}

fn reserve(s: &Store, id: &AccountId, now_ms: i64, on_demand: bool) -> Reserve {
    let row = s.account(id).unwrap().unwrap();
    s.reserve_usage(&row, now_ms, on_demand, &B).unwrap()
}

fn reserved(s: &Store, id: &AccountId, now_ms: i64) -> Reservation {
    match reserve(s, id, now_ms, true) {
        Reserve::Reserved(r) => r,
        other => panic!("expected a reservation, got {other:?}"),
    }
}

/// The first slot, the one `reserve_usage` took with the reservation.
fn slot_of(r: &Reservation) -> Slot {
    Slot {
        slot: r.slot,
        slot_at: r.slot_at,
    }
}

/// A further slot under `r` at `now_ms`, as the 401 retry asks for one (no slot held, no
/// token fingerprint).
fn another_slot(s: &Store, r: &Reservation, now_ms: i64) -> SendGrant {
    s.authorize_send(r, None, None, now_ms, &B).unwrap()
}

/// Takes the rest of the identity's hourly budget under `r`, at `now_ms`.
fn spend_the_hour(s: &Store, r: &Reservation, now_ms: i64) {
    while matches!(another_slot(s, r, now_ms), SendGrant::Send(_)) {}
}

/// The reservation times `usage_requests` holds for one identity, ascending.
fn slot_times(path: &Path, provider: &ProviderId, key: &str) -> Vec<i64> {
    let c = raw(path);
    let mut stmt = c
        .prepare(
            "SELECT at FROM usage_requests WHERE provider = ?1 AND identity_key = ?2 ORDER BY at",
        )
        .unwrap();
    stmt.query_map(params![provider.as_str(), key], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn all_slots(path: &Path) -> i64 {
    raw(path)
        .query_row("SELECT COUNT(*) FROM usage_requests", [], |r| r.get(0))
        .unwrap()
}

/// A lease row's holder and expiry (epoch ms).
fn lease(path: &Path, name: &str) -> Option<(String, i64)> {
    raw(path)
        .query_row(
            "SELECT holder, expires_at FROM leases WHERE name = ?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .unwrap()
}

fn window(key: &str, kind: WindowKind, pct: f64, resets_at: Option<i64>) -> Window {
    Window {
        key: key.into(),
        label: key.into(),
        kind,
        pct,
        resets_at,
        period_s: None,
        detail: None,
    }
}

fn windows() -> Vec<Window> {
    vec![
        Window {
            period_s: Some(18_000),
            ..window("5h", WindowKind::Short, 9.0, Some(T + 3_600))
        },
        Window {
            period_s: Some(604_800),
            ..window("7d", WindowKind::Long, 77.0, Some(T + 86_400))
        },
        Window {
            detail: Some(json!({"used": 3.5, "limit": 20.0, "currency": "EUR"})),
            ..window("spend", WindowKind::Spend, 17.5, None)
        },
    ]
}

fn plan() -> PollPlan {
    PollPlan {
        interval_s: 300,
        next_poll_at: T + 300,
    }
}

#[test]
fn reserving_takes_the_lease_and_one_slot() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let now_ms = T_MS + 400;
    let r = reserved(&s, &a, now_ms);
    assert_eq!(r.account_id, a);
    assert_eq!(r.provider, cc());
    assert_eq!(r.identity_key, "a@x.co\n");
    assert_eq!(r.slot_at, T, "slots are whole epoch seconds");
    assert_eq!(
        uuid::Uuid::parse_str(&r.holder).unwrap().get_version_num(),
        7
    );
    assert_eq!(
        lease(&path, "usage:a"),
        Some((r.holder.clone(), now_ms + 90_000)),
        "§6.1: the lease expiry is in milliseconds"
    );
    assert_eq!(slot_times(&path, &cc(), "a@x.co\n"), vec![T]);
    let rowid: i64 = raw(&path)
        .query_row("SELECT rowid FROM usage_requests", [], |row| row.get(0))
        .unwrap();
    assert_eq!(r.slot, rowid);
    assert!(s.usage_lease_live(&a, now_ms + 89_999).unwrap());
    assert!(!s.usage_lease_live(&a, now_ms + 90_000).unwrap());
    assert_eq!(
        s.usage_state(&a).unwrap(),
        None,
        "reserving records nothing"
    );
}

#[test]
fn a_quarantined_account_is_never_reserved() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    s.set_quarantine(&a, "invalid_grant", "sha256:sent", 1)
        .unwrap();
    for on_demand in [true, false] {
        assert_eq!(
            reserve(&s, &a, T_MS, on_demand),
            Reserve::Ineligible(Ineligible::Quarantined)
        );
    }
    assert_eq!(all_slots(&path), 0);
    assert_eq!(lease(&path, "usage:a"), None);
}

#[test]
fn backoff_holds_until_it_lifts() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    arrange(&path, &a, None, Some(T + 1), None);
    assert_eq!(
        reserve(&s, &a, T_MS, true),
        Reserve::Ineligible(Ineligible::Backoff)
    );
    assert_eq!(all_slots(&path), 0);
    assert!(matches!(
        reserve(&s, &a, T_MS + 1_000, true),
        Reserve::Reserved(_)
    ));
}

#[test]
fn a_live_lease_blocks_and_an_expired_one_is_taken_over() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    raw(&path)
        .execute(
            "INSERT INTO leases (name, holder, expires_at) VALUES ('usage:a', 'other', ?1)",
            [T_MS + 1],
        )
        .unwrap();
    assert_eq!(
        reserve(&s, &a, T_MS, true),
        Reserve::Ineligible(Ineligible::Leased)
    );
    assert_eq!(lease(&path, "usage:a"), Some(("other".into(), T_MS + 1)));
    assert_eq!(all_slots(&path), 0);
    let r = reserved(&s, &a, T_MS + 1);
    assert_eq!(lease(&path, "usage:a"), Some((r.holder, T_MS + 1 + 90_000)));
}

#[test]
fn on_demand_needs_an_old_reading_and_a_due_plan() {
    let (_d, path, s) = open();
    let cases = [
        // (fetched_at, next_poll_at, expected)
        (Some(T - 180), None, Some(Ineligible::NotDue)),
        (Some(T - 181), None, None),
        (Some(T - 1_000), Some(T + 1), Some(Ineligible::NotDue)),
        (Some(T - 1_000), Some(T), None),
        // A plan in force holds even without a reading, as an over-budget plan must (§8.6).
        (None, Some(T + 100), Some(Ineligible::NotDue)),
        (None, None, None),
    ];
    for (i, (fetched_at, next_poll_at, expected)) in cases.into_iter().enumerate() {
        let n = i as u32 + 1;
        let id = add(&s, &cc(), &format!("a{n}"), &format!("a{n}@x.co"), n);
        arrange(&path, &id, fetched_at, None, next_poll_at);
        let got = reserve(&s, &id, T_MS, true);
        match expected {
            Some(why) => assert_eq!(got, Reserve::Ineligible(why), "case {i}"),
            None => assert!(matches!(got, Reserve::Reserved(_)), "case {i}: {got:?}"),
        }
    }
}

#[test]
fn scheduled_collection_takes_a_due_plan_or_a_missing_reading() {
    let (_d, path, s) = open();
    let cases = [
        // (fetched_at, next_poll_at, expected)
        (Some(T - 10), Some(T), None),
        (Some(T - 10), Some(T + 1), Some(Ineligible::NotDue)),
        (Some(T - 10), None, None),
        (None, Some(T + 100), None),
    ];
    for (i, (fetched_at, next_poll_at, expected)) in cases.into_iter().enumerate() {
        let n = i as u32 + 1;
        let id = add(&s, &cc(), &format!("a{n}"), &format!("a{n}@x.co"), n);
        arrange(&path, &id, fetched_at, None, next_poll_at);
        let got = reserve(&s, &id, T_MS, false);
        match expected {
            Some(why) => assert_eq!(got, Reserve::Ineligible(why), "case {i}"),
            None => assert!(matches!(got, Reserve::Reserved(_)), "case {i}: {got:?}"),
        }
    }
}

#[test]
fn over_budget_moves_the_plan_and_takes_no_lease() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    spend_the_hour(&s, &r, T_MS);
    assert_eq!(slot_times(&path, &cc(), "a@x.co\n"), vec![T; 20]);
    let free = T + B.count_window_s;
    assert_eq!(
        another_slot(&s, &r, T_MS),
        SendGrant::OverBudget { next_free_at: free }
    );

    // The lease has expired and the account is due, but its identity has spent the hour.
    let later = T_MS + 91_000;
    assert_eq!(
        reserve(&s, &a, later, true),
        Reserve::OverBudget { next_free_at: free }
    );
    assert_eq!(s.usage_state(&a).unwrap().unwrap().next_poll_at, Some(free));
    assert_eq!(
        lease(&path, "usage:a"),
        Some((r.holder.clone(), T_MS + 90_000)),
        "no lease taken"
    );
    assert_eq!(all_slots(&path), 20);
    assert_eq!(
        reserve(&s, &a, later + 1_000, true),
        Reserve::Ineligible(Ineligible::NotDue),
        "the moved plan holds on-demand callers off until a slot frees"
    );

    // The budget is per (provider, identity key).
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    assert!(matches!(reserve(&s, &b, later, true), Reserve::Reserved(_)));
    let f = add(&s, &ProviderId::new("fake-agent"), "f", "a@x.co", 1);
    assert!(matches!(reserve(&s, &f, later, true), Reserve::Reserved(_)));
}

#[test]
fn a_row_leaves_the_count_exactly_one_window_after_it_was_reserved() {
    // §8.6 with Task 4's strict window: a row counts while `now − at < count_window_s`.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let r = reserved(&s, &a, T_MS);
    spend_the_hour(&s, &r, T_MS);
    reserved(&s, &b, T_MS);
    assert_eq!(all_slots(&path), 21);
    let edge = T + B.count_window_s;
    assert_eq!(
        another_slot(&s, &r, (edge - 1) * 1000),
        SendGrant::OverBudget { next_free_at: edge },
        "one second before, every row still counts"
    );
    let SendGrant::Send(slot) = another_slot(&s, &r, edge * 1000) else {
        panic!("a slot frees exactly one window on");
    };
    assert_eq!(slot.slot_at, edge);
    assert_eq!(
        slot_times(&path, &cc(), "a@x.co\n"),
        vec![edge],
        "rows that left are pruned on insert"
    );
    assert_eq!(all_slots(&path), 1, "every identity's, not just this one's");
}

#[test]
fn a_slot_still_valid_is_the_one_sent_under() {
    // §8.6: a slot is valid for `slot_valid_s`; within it, the send takes no second slot.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    let last_valid_ms = (T + B.slot_valid_s) * 1000;
    assert_eq!(
        s.authorize_send(&r, Some(&slot_of(&r)), Some("sha256:ok"), last_valid_ms, &B)
            .unwrap(),
        SendGrant::Send(slot_of(&r))
    );
    assert_eq!(slot_times(&path, &cc(), "a@x.co\n"), vec![T]);
}

#[test]
fn a_stale_slot_is_given_back_and_a_fresh_one_reserved() {
    // Review Focus 2: a sender suspended past `slot_valid_s` discards its slot and reserves
    // again, in the one authorization, so the budget counts the request once, at its fresh
    // time.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    let resumed = T + B.slot_valid_s + 1;
    assert!(resumed - r.slot_at > B.slot_valid_s);
    let SendGrant::Send(fresh) = s
        .authorize_send(
            &r,
            Some(&slot_of(&r)),
            Some("sha256:ok"),
            resumed * 1000,
            &B,
        )
        .unwrap()
    else {
        panic!("a stale slot is replaced");
    };
    assert_eq!(fresh.slot_at, resumed);
    assert_ne!(fresh, slot_of(&r));
    assert_eq!(
        slot_times(&path, &cc(), "a@x.co\n"),
        vec![resumed],
        "the stale row is gone"
    );
}

#[test]
fn over_budget_a_stale_slot_is_still_given_back() {
    // The stale slot goes back before the count, and stays gone when no fresh slot is free.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    let c = raw(&path);
    for _ in 0..B.hourly_requests {
        c.execute(
            "INSERT INTO usage_requests (provider, identity_key, at) VALUES ('claude-code', ?1, ?2)",
            params!["a@x.co\n", T + 1],
        )
        .unwrap();
    }
    let resumed = T + B.slot_valid_s + 1;
    assert_eq!(
        s.authorize_send(&r, Some(&slot_of(&r)), None, resumed * 1000, &B)
            .unwrap(),
        SendGrant::OverBudget {
            next_free_at: T + 1 + B.count_window_s
        }
    );
    assert_eq!(
        slot_times(&path, &cc(), "a@x.co\n"),
        vec![T + 1; B.hourly_requests as usize],
        "the stale slot went back and no fresh one was taken"
    );
}

#[test]
fn a_holder_that_lost_the_lease_or_the_identity_is_not_authorized() {
    // §8.3's fence, before every request: a sender suspended past its lease, or whose account
    // was re-logged meanwhile, sends nothing, and nothing is written: no slot is inserted and
    // the held one is left counted, as after any failed fence.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let first = reserved(&s, &a, T_MS);
    let second = reserved(&s, &a, T_MS + 90_000);
    let later = T_MS + 91_000;
    for slot in [Some(slot_of(&first)), None] {
        assert_eq!(
            s.authorize_send(&first, slot.as_ref(), Some("sha256:ok"), later, &B)
                .unwrap(),
            SendGrant::LeaseLost
        );
    }
    assert_eq!(all_slots(&path), 2, "no slot inserted, none given back");

    s.update_login(&a, "z@x.co\n", &identity("z@x.co"), "oauth", None)
        .unwrap();
    assert_eq!(
        s.authorize_send(&second, Some(&slot_of(&second)), None, later, &B)
            .unwrap(),
        SendGrant::LeaseLost,
        "the identity fence"
    );
    assert_eq!(all_slots(&path), 2);
}

#[test]
fn a_token_the_server_refused_is_not_authorized() {
    // §8.1: the durable `rejected_fp`, not the sender's own copy, decides, so a token refused
    // since the sender read its state is not sent.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    assert!(s.set_rejected_fp(&r, Some("sha256:refused")).unwrap());
    assert_eq!(
        s.authorize_send(&r, Some(&slot_of(&r)), Some("sha256:refused"), T_MS, &B)
            .unwrap(),
        SendGrant::Rejected
    );
    assert_eq!(
        s.authorize_send(&r, None, Some("sha256:refused"), T_MS, &B)
            .unwrap(),
        SendGrant::Rejected
    );
    assert_eq!(all_slots(&path), 1, "nothing written");
    for fp in [Some("sha256:other"), None] {
        assert_eq!(
            s.authorize_send(&r, Some(&slot_of(&r)), fp, T_MS, &B)
                .unwrap(),
            SendGrant::Send(slot_of(&r))
        );
    }
}

#[test]
fn a_released_slot_returns_to_the_budget() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    spend_the_hour(&s, &r, T_MS);
    assert!(matches!(
        another_slot(&s, &r, T_MS),
        SendGrant::OverBudget { .. }
    ));
    s.release_slot(&r, &slot_of(&r)).unwrap();
    assert_eq!(all_slots(&path), 19);
    assert!(matches!(another_slot(&s, &r, T_MS), SendGrant::Send(_)));
    assert_eq!(all_slots(&path), 20);
}

#[test]
fn a_stale_release_never_deletes_the_slot_that_reused_its_rowid() {
    // `usage_requests` has no AUTOINCREMENT: once a slot's row is pruned, SQLite gives its
    // rowid to the next insert. A process that slept past the count window and then hands its
    // slot back must not delete that other process's slot.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let r = reserved(&s, &a, T_MS);
    let later = T + B.count_window_s;
    let other = reserved(&s, &b, later * 1000);
    assert_eq!(other.slot, r.slot, "the pruned row's rowid was reused");

    s.release_slot(&r, &slot_of(&r)).unwrap();
    assert!(
        s.record_usage_failure(&r, "pre-send", later, later + 30, None, Some(&slot_of(&r)))
            .unwrap()
    );
    assert_eq!(
        slot_times(&path, &cc(), "b@x.co\n"),
        vec![later],
        "b's slot still counts"
    );
    assert_eq!(all_slots(&path), 1);
}

#[test]
fn a_successful_record_writes_the_reading_the_plan_and_samples() {
    let (_d, _path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    assert!(
        s.record_usage_failure(&r, "http-429", T, T + 400, Some(T + 400), None)
            .unwrap()
    );
    let failed = s.usage_state(&a).unwrap().unwrap();
    assert_eq!(
        (failed.consecutive_failures, failed.fetched_at),
        (1, None),
        "a first failure creates the row"
    );
    assert!(s.set_rejected_fp(&r, Some("sha256:rejected")).unwrap());

    assert!(s.record_usage(&r, &windows(), T + 5, &plan(), 180).unwrap());
    assert_eq!(
        s.usage_state(&a).unwrap().unwrap(),
        UsageStateRow {
            account_id: a.clone(),
            last_good: Some(windows()),
            fetched_at: Some(T + 5),
            last_attempt_at: Some(T + 5),
            consecutive_failures: 0,
            last_error: None,
            backoff_until: None,
            next_poll_at: Some(T + 300),
            poll_interval_s: Some(300),
            last_429_at: Some(T + 400),
            rejected_fp: None,
        },
        "success resets the failure fields and rejected_fp, and never clears last_429_at"
    );
    assert_eq!(
        s.usage_samples(&a, None, 0).unwrap(),
        vec![
            (
                "5h".to_owned(),
                Sample {
                    fetched_at: T + 5,
                    pct: 9.0,
                    resets_at: Some(T + 3_600)
                }
            ),
            (
                "7d".to_owned(),
                Sample {
                    fetched_at: T + 5,
                    pct: 77.0,
                    resets_at: Some(T + 86_400)
                }
            ),
            (
                "spend".to_owned(),
                Sample {
                    fetched_at: T + 5,
                    pct: 17.5,
                    resets_at: None
                }
            ),
        ]
    );
}

#[test]
fn samples_filter_by_window_and_time() {
    let (_d, _path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    assert!(s.record_usage(&r, &windows(), T, &plan(), 180).unwrap());
    assert!(
        s.record_usage(&r, &windows(), T + 600, &plan(), 180)
            .unwrap()
    );
    let times = |w: Option<&str>, since: i64| -> Vec<(String, i64)> {
        s.usage_samples(&a, w, since)
            .unwrap()
            .into_iter()
            .map(|(k, x)| (k, x.fetched_at))
            .collect()
    };
    assert_eq!(
        times(Some("7d"), 0),
        vec![("7d".into(), T), ("7d".into(), T + 600)]
    );
    assert_eq!(
        times(None, T + 600),
        vec![
            ("5h".into(), T + 600),
            ("7d".into(), T + 600),
            ("spend".into(), T + 600)
        ]
    );
    assert!(times(Some("scoped:Fable"), 0).is_empty());
}

#[test]
fn an_empty_reading_is_stored_as_no_windows_with_its_fetch_time() {
    let (_d, _path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    assert!(s.record_usage(&r, &[], T, &plan(), 180).unwrap());
    let st = s.usage_state(&a).unwrap().unwrap();
    assert_eq!((st.last_good, st.fetched_at), (None, Some(T)));
    assert!(s.usage_samples(&a, None, 0).unwrap().is_empty());
}

#[test]
fn a_corrupt_reading_reads_as_none() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    assert!(s.record_usage(&r, &windows(), T, &plan(), 180).unwrap());
    raw(&path)
        .execute("UPDATE usage_state SET last_good = '{not json'", [])
        .unwrap();
    let st = s.usage_state(&a).unwrap().unwrap();
    assert_eq!((st.last_good, st.fetched_at), (None, Some(T)));
}

#[test]
fn a_record_after_another_holder_took_the_lease_is_dropped() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let first = reserved(&s, &a, T_MS);
    let second = reserved(&s, &a, T_MS + 90_000);
    assert_ne!(first.holder, second.holder);
    assert!(
        !s.record_usage(&first, &windows(), T + 91, &plan(), 180)
            .unwrap()
    );
    assert!(
        !s.record_usage_failure(
            &first,
            "http-500",
            T + 91,
            T + 121,
            None,
            Some(&slot_of(&first))
        )
        .unwrap()
    );
    assert!(
        !s.set_rejected_fp(&first, Some("sha256:late")).unwrap(),
        "a lost holder stamps no refusal"
    );
    assert_eq!(s.usage_state(&a).unwrap(), None, "nothing written");
    assert!(s.usage_samples(&a, None, 0).unwrap().is_empty());
    assert_eq!(lease(&path, "prune:usage_samples"), None);
    assert_eq!(all_slots(&path), 2, "a failed fence gives no slot back");
    assert!(
        s.record_usage(&second, &windows(), T + 91, &plan(), 180)
            .unwrap()
    );
}

#[test]
fn a_record_for_a_changed_identity_is_dropped() {
    let (_d, _path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    s.update_login(&a, "z@x.co\n", &identity("z@x.co"), "oauth", None)
        .unwrap();
    assert!(!s.record_usage(&r, &windows(), T, &plan(), 180).unwrap());
    assert!(
        !s.record_usage_failure(&r, "http-500", T, T + 30, None, None)
            .unwrap()
    );
    assert!(!s.set_rejected_fp(&r, Some("sha256:x")).unwrap());
    assert_eq!(s.usage_state(&a).unwrap(), None);
}

#[test]
fn a_failure_never_touches_the_last_good_reading() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    assert!(s.record_usage(&r, &windows(), T, &plan(), 180).unwrap());
    let SendGrant::Send(unsent) = another_slot(&s, &r, T_MS + 1_000) else {
        panic!("a slot is free");
    };
    assert!(
        s.record_usage_failure(&r, "http-429", T + 1, T + 301, Some(T + 301), None)
            .unwrap()
    );
    assert!(
        s.record_usage_failure(&r, "pre-send", T + 2, T + 62, None, Some(&unsent))
            .unwrap()
    );
    let st = s.usage_state(&a).unwrap().unwrap();
    assert_eq!(st.last_good, Some(windows()));
    assert_eq!(st.fetched_at, Some(T));
    assert_eq!(st.last_attempt_at, Some(T + 2));
    assert_eq!(st.consecutive_failures, 2);
    assert_eq!(st.last_error.as_deref(), Some("pre-send"));
    assert_eq!(st.backoff_until, Some(T + 62));
    assert_eq!(st.last_429_at, Some(T + 301), "kept when not given");
    assert_eq!(st.next_poll_at, Some(T + 300), "the plan is untouched");
    assert_eq!(
        slot_times(&path, &cc(), "a@x.co\n"),
        vec![T],
        "the unsent slot is given back; the sent one still counts"
    );
}

#[test]
fn samples_are_pruned_at_most_once_a_day() {
    // Decision 7: the `prune:usage_samples` lease row spaces prunes a day apart.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    let plant = || {
        raw(&path)
            .execute(
                "INSERT INTO usage_samples (account_id, window, fetched_at, pct) \
                 VALUES ('a', 'old', ?1, 1.0)",
                [T - 2 * 86_400],
            )
            .unwrap();
    };
    let old = || s.usage_samples(&a, Some("old"), 0).unwrap().len();

    plant();
    assert!(s.record_usage(&r, &windows(), T, &plan(), 1).unwrap());
    assert_eq!(old(), 0, "the first record prunes");
    assert_eq!(
        lease(&path, "prune:usage_samples").map(|(_, at)| at),
        Some((T + 86_400) * 1000)
    );

    plant();
    assert!(
        s.record_usage(&r, &windows(), T + 3_600, &plan(), 1)
            .unwrap()
    );
    assert_eq!(old(), 1, "within the day, no second prune");

    assert!(
        s.record_usage(&r, &windows(), T + 86_400, &plan(), 1)
            .unwrap()
    );
    assert_eq!(old(), 0, "a day on, pruned again");
    let kept: Vec<i64> = s
        .usage_samples(&a, Some("7d"), 0)
        .unwrap()
        .into_iter()
        .map(|(_, x)| x.fetched_at)
        .collect();
    assert_eq!(
        kept,
        vec![T, T + 3_600, T + 86_400],
        "a sample exactly retention_days old is kept"
    );
}

#[test]
fn removing_and_re_adding_keeps_the_hours_count() {
    // Review Focus 4: usage rows cascade with the account, but the budget is keyed by
    // identity, so a re-added account cannot reset it.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    assert!(s.record_usage(&r, &windows(), T, &plan(), 180).unwrap());
    spend_the_hour(&s, &r, T_MS);
    s.delete_account(&a).unwrap();
    assert_eq!(s.usage_state(&a).unwrap(), None);
    assert!(s.usage_samples(&a, None, 0).unwrap().is_empty());

    let again = add(&s, &cc(), "a2", "a@x.co", 2);
    assert_eq!(
        reserve(&s, &again, T_MS + 91_000, true),
        Reserve::OverBudget {
            next_free_at: T + B.count_window_s
        }
    );
    assert_eq!(slot_times(&path, &cc(), "a@x.co\n").len(), 20);
}

#[test]
fn two_stores_racing_for_slots_never_exceed_the_budget() {
    // Review Focus 1: two processes (two `Store` handles on one file) reserve at once.
    let (_d, path, s1) = open();
    let s2 = Store::open(&path).unwrap();
    let a = add(&s1, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s1, &a, T_MS);
    let barrier = Barrier::new(16);
    let (granted, refused) = (AtomicUsize::new(0), AtomicUsize::new(0));
    std::thread::scope(|scope| {
        for i in 0..16 {
            let s = if i % 2 == 0 { &s1 } else { &s2 };
            let (r, barrier, granted, refused) = (&r, &barrier, &granted, &refused);
            scope.spawn(move || {
                barrier.wait();
                for _ in 0..5 {
                    match another_slot(s, r, T_MS) {
                        SendGrant::Send(_) => granted.fetch_add(1, SeqCst),
                        SendGrant::OverBudget { next_free_at } => {
                            assert_eq!(next_free_at, T + B.count_window_s);
                            refused.fetch_add(1, SeqCst)
                        }
                        other => panic!("unexpected {other:?}"),
                    };
                }
            });
        }
    });
    assert_eq!(granted.load(SeqCst), 19, "with the reservation's own, 20");
    assert_eq!(refused.load(SeqCst), 80 - 19);
    assert_eq!(all_slots(&path), 20);
}

#[test]
fn two_stores_racing_for_one_account_reserve_it_once() {
    // Review Focus 1: each account is fetched at most once per lease.
    let (_d, path, s1) = open();
    let s2 = Store::open(&path).unwrap();
    let a = add(&s1, &cc(), "a", "a@x.co", 1);
    let row = s1.account(&a).unwrap().unwrap();
    let barrier = Barrier::new(8);
    let (won, leased) = (AtomicUsize::new(0), AtomicUsize::new(0));
    std::thread::scope(|scope| {
        for i in 0..8 {
            let s = if i % 2 == 0 { &s1 } else { &s2 };
            let (row, barrier, won, leased) = (&row, &barrier, &won, &leased);
            scope.spawn(move || {
                barrier.wait();
                match s.reserve_usage(row, T_MS, true, &B).unwrap() {
                    Reserve::Reserved(_) => won.fetch_add(1, SeqCst),
                    Reserve::Ineligible(Ineligible::Leased) => leased.fetch_add(1, SeqCst),
                    other => panic!("unexpected {other:?}"),
                };
            });
        }
    });
    assert_eq!((won.load(SeqCst), leased.load(SeqCst)), (1, 7));
    assert_eq!(all_slots(&path), 1);
}

#[test]
fn set_poll_plan_sets_the_plan_alone_and_creates_the_row() {
    let (_d, _path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let active = PollPlan {
        interval_s: 180,
        next_poll_at: T + 180,
    };
    s.set_poll_plan(&a, &active).unwrap();
    assert_eq!(
        s.usage_state(&a).unwrap().unwrap(),
        UsageStateRow {
            account_id: a.clone(),
            last_good: None,
            fetched_at: None,
            last_attempt_at: None,
            consecutive_failures: 0,
            last_error: None,
            backoff_until: None,
            next_poll_at: Some(T + 180),
            poll_interval_s: Some(180),
            last_429_at: None,
            rejected_fp: None,
        }
    );

    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let r = reserved(&s, &b, T_MS);
    assert!(s.record_usage(&r, &windows(), T, &plan(), 180).unwrap());
    let candidate = PollPlan {
        interval_s: 600,
        next_poll_at: T + 600,
    };
    s.set_poll_plan(&b, &candidate).unwrap();
    let st = s.usage_state(&b).unwrap().unwrap();
    assert_eq!(
        (
            st.last_good,
            st.fetched_at,
            st.next_poll_at,
            st.poll_interval_s
        ),
        (Some(windows()), Some(T), Some(T + 600), Some(600))
    );

    assert!(matches!(
        s.set_poll_plan(&AccountId::from_string("nobody"), &active),
        Err(StoreError::NoSuchAccount)
    ));
}

#[test]
fn rejected_fp_is_stamped_and_cleared_only_by_the_lease_holder() {
    let (_d, _path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let first = reserved(&s, &a, T_MS);
    assert!(s.set_rejected_fp(&first, Some("sha256:refused")).unwrap());
    assert_eq!(
        s.usage_state(&a).unwrap().unwrap().rejected_fp.as_deref(),
        Some("sha256:refused"),
        "a first stamp creates the row"
    );
    assert!(s.set_rejected_fp(&first, None).unwrap());
    assert_eq!(s.usage_state(&a).unwrap().unwrap().rejected_fp, None);

    let second = reserved(&s, &a, T_MS + 90_000);
    assert!(
        !s.set_rejected_fp(&first, Some("sha256:late")).unwrap(),
        "the lease was taken over"
    );
    assert_eq!(s.usage_state(&a).unwrap().unwrap().rejected_fp, None);

    s.delete_account(&a).unwrap();
    assert!(
        !s.set_rejected_fp(&second, Some("sha256:x")).unwrap(),
        "a removed account fails the identity fence"
    );
}

#[test]
fn the_live_identity_cache_round_trips_per_provider() {
    let (_d, path, s) = open();
    assert_eq!(s.live_identity_cache(&cc()).unwrap(), None);
    let row = LiveIdentityCacheRow {
        provider: cc(),
        path: "/home/t/.claude.json".into(),
        mtime_ns: 1_790_000_000_123_456_789,
        size: 4_096,
        identity_key: Some("a@x.co\n".into()),
        label: Some("a@x.co".into()),
        account_uuid: Some("uuid-a".into()),
    };
    s.put_live_identity_cache(&row).unwrap();
    assert_eq!(s.live_identity_cache(&cc()).unwrap(), Some(row.clone()));

    let logged_out = LiveIdentityCacheRow {
        mtime_ns: row.mtime_ns + 1,
        size: 2,
        identity_key: None,
        label: None,
        account_uuid: None,
        ..row
    };
    s.put_live_identity_cache(&logged_out).unwrap();
    assert_eq!(s.live_identity_cache(&cc()).unwrap(), Some(logged_out));

    let fake = ProviderId::new("fake-agent");
    assert_eq!(s.live_identity_cache(&fake).unwrap(), None);
    raw(&path)
        .execute(
            "INSERT INTO live_identity_cache (provider) VALUES ('fake-agent')",
            [],
        )
        .unwrap();
    assert_eq!(
        s.live_identity_cache(&fake).unwrap(),
        None,
        "a row without its file key is a miss"
    );
}
