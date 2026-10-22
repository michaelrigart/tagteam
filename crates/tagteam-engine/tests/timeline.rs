//! `Engine::timeline` (§12.3): the History screen's usage per column and who was in use,
//! read from the store only. Accounts are added through the store; active-account changes and
//! samples are planted by raw SQL on `tagteam.db`, since SQLite stamps `active_history` with
//! the wall clock and a reading is recorded at the fixture's.

mod common;

use std::fs;

use common::{Fx, add};
use serde_json::json;
use tagteam_core::AccountId;
use tagteam_engine::event::AUTO_LINE_KINDS;
use tagteam_engine::timeline::{AccountRef, EventView, IN_USE, Lane, Scope, Series, TimelineView};
use tagteam_engine::{EngineError, EventKind, EventSource};

/// The fixture clock's start (`Fx`), in seconds.
const T0: i64 = 1_790_000_000;
/// Seven days before `T0`: over seven columns, column `c` is day `c`.
const SINCE: i64 = T0 - 7 * 86_400;
const COLUMNS: usize = 7;

/// Hour `h` of day `c` of the range, in seconds.
fn day(c: i64, h: i64) -> i64 {
    SINCE + c * 86_400 + h * 3_600
}

/// The store with `a` (1 `alex@example.com`), `b` (2 `sam@corp.example`, alias `work`) and `c`
/// (3 `alex@personal.example`, alias `spare`).
fn fixture() -> (Fx, AccountId, AccountId, AccountId) {
    let fx = Fx::new();
    let store = fx.engine.store().unwrap();
    let p = fx.provider();
    let a = add(&store, &p, "a", "alex@example.com", 1);
    let b = add(&store, &p, "b", "sam@corp.example", 2);
    let c = add(&store, &p, "c", "alex@personal.example", 3);
    store.set_alias(&b, Some("work")).unwrap();
    store.set_alias(&c, Some("spare")).unwrap();
    (fx, a, b, c)
}

fn raw(fx: &Fx) -> rusqlite::Connection {
    rusqlite::Connection::open(fx.engine.store_file()).unwrap()
}

/// `id` became the provider's active account at `at_s`.
fn in_use(fx: &Fx, id: &AccountId, at_s: i64) {
    in_use_ms(fx, id, at_s * 1000);
}

/// `id` became the provider's active account at `at_ms`, as SQLite stamps it.
fn in_use_ms(fx: &Fx, id: &AccountId, at_ms: i64) {
    raw(fx)
        .execute(
            "INSERT INTO active_history (provider, at, account_id, recorded_at) \
             VALUES (?1, ?2, ?3, ?2)",
            rusqlite::params![fx.provider().as_str(), at_ms, id.as_str()],
        )
        .unwrap();
}

/// No account was active from `at_s`.
fn nobody(fx: &Fx, at_s: i64) {
    raw(fx)
        .execute(
            "INSERT INTO active_history (provider, at, account_id, recorded_at) \
             VALUES (?1, ?2, NULL, ?2)",
            rusqlite::params![fx.provider().as_str(), at_s * 1000],
        )
        .unwrap();
}

/// `id`'s window `key` was at `pct` at `at_s`.
fn sample(fx: &Fx, id: &AccountId, key: &str, at_s: i64, pct: f64) {
    raw(fx)
        .execute(
            "INSERT INTO usage_samples (account_id, window, fetched_at, pct, resets_at) \
             VALUES (?1, ?2, ?3, ?4, NULL)",
            rusqlite::params![id.as_str(), key, at_s, pct],
        )
        .unwrap();
}

fn timeline(fx: &Fx, scope: Scope) -> TimelineView {
    fx.engine
        .timeline(&fx.provider(), scope, SINCE, COLUMNS)
        .unwrap()
}

fn write_config(fx: &Fx, toml: &str) {
    fs::create_dir_all(fx.env.config_dir()).unwrap();
    fs::write(fx.env.config_dir().join("config.toml"), toml).unwrap();
}

/// Seven columns' peaks: `Some(pct)` where given, `None` elsewhere.
fn peaks(at: &[(usize, f64)]) -> Vec<Option<f64>> {
    let mut v = vec![None; COLUMNS];
    for &(c, pct) in at {
        v[c] = Some(pct);
    }
    v
}

/// Seven columns' flags from a string of `t` and `f`.
fn flags(s: &str) -> Vec<bool> {
    s.chars().map(|c| c == 't').collect()
}

fn ids(lanes: &[Lane]) -> Vec<&str> {
    lanes.iter().map(|l| l.account.id.as_str()).collect()
}

/// `a` in use from before the range, with 5h at 40 and 60 on day 0, 7d at 30 on day 1, spend
/// at 99 on day 0 and Fable at 88 on day 2.
fn first_fixture() -> (Fx, AccountId) {
    let (fx, a, _, _) = fixture();
    in_use(&fx, &a, SINCE - 3_600);
    sample(&fx, &a, "5h", day(0, 1), 40.0);
    sample(&fx, &a, "5h", day(0, 2), 60.0);
    sample(&fx, &a, "7d", day(1, 5), 30.0);
    sample(&fx, &a, "spend", day(0, 3), 99.0);
    sample(&fx, &a, "scoped:Fable", day(2, 1), 88.0);
    (fx, a)
}

#[test]
fn a_lane_is_each_column_s_peak_of_the_account_s_relevant_windows() {
    let (fx, a) = first_fixture();
    let v = timeline(&fx, Scope::All);
    assert_eq!(
        v.lanes,
        [Lane {
            account: AccountRef {
                id: a.clone(),
                position: Some(1),
                alias: None,
                email: Some("alex@example.com".into()),
                removed: false,
            },
            peaks: peaks(&[(0, 60.0), (1, 30.0)]),
            in_use: vec![true; COLUMNS],
        }]
    );
    assert_eq!(
        v.charts,
        [Series {
            label: IN_USE.into(),
            peaks: peaks(&[(0, 60.0), (1, 30.0)]),
        }]
    );
    assert!(fx.http.requests().is_empty(), "the timeline never fetches");

    write_config(&fx, "[autoswitch]\nmodels = [\"fable\"]\n");
    let v = timeline(&fx, Scope::All);
    assert_eq!(v.lanes[0].peaks, peaks(&[(0, 60.0), (1, 30.0), (2, 88.0)]));
}

#[test]
fn the_in_use_chart_takes_each_sample_only_while_its_account_was_in_use() {
    let (fx, a, b, _) = fixture();
    in_use(&fx, &a, SINCE - 3_600);
    in_use(&fx, &b, day(3, 0));
    sample(&fx, &a, "5h", day(1, 0), 50.0);
    sample(&fx, &a, "5h", day(5, 0), 70.0);
    sample(&fx, &b, "5h", day(1, 0), 80.0);
    sample(&fx, &b, "5h", day(4, 0), 20.0);
    let v = timeline(&fx, Scope::All);
    assert_eq!(
        v.charts,
        [Series {
            label: IN_USE.into(),
            peaks: peaks(&[(1, 50.0), (4, 20.0)]),
        }]
    );
    assert_eq!(ids(&v.lanes), ["a", "b"]);
    assert_eq!(v.lanes[0].peaks, peaks(&[(1, 50.0), (5, 70.0)]));
    assert_eq!(v.lanes[0].in_use, flags("tttffff"));
    assert_eq!(v.lanes[1].peaks, peaks(&[(1, 80.0), (4, 20.0)]));
    assert_eq!(v.lanes[1].in_use, flags("ffftttt"));
    assert_eq!(
        v.lanes[1].account,
        AccountRef {
            id: b,
            position: Some(2),
            alias: Some("work".into()),
            email: Some("sam@corp.example".into()),
            removed: false,
        }
    );
}

#[test]
fn a_column_spanning_a_switch_shows_its_peak() {
    let (fx, a, b, _) = fixture();
    in_use(&fx, &a, SINCE - 3_600);
    in_use(&fx, &b, day(2, 12));
    sample(&fx, &a, "5h", day(2, 11), 93.0);
    sample(&fx, &b, "5h", day(2, 13), 3.0);
    let v = timeline(&fx, Scope::All);
    assert_eq!(v.charts[0].peaks, peaks(&[(2, 93.0)]));
    assert_eq!(v.lanes[0].in_use, flags("tttffff"));
    assert_eq!(v.lanes[1].in_use, flags("ffttttt"));
}

#[test]
fn a_change_at_the_instant_of_the_one_before_marks_nothing_for_it() {
    // `a` then `b` at one `at`: `a`'s span is empty.
    let (fx, a, b, _) = fixture();
    in_use(&fx, &a, day(2, 0));
    in_use(&fx, &b, day(2, 0));
    sample(&fx, &a, "5h", day(2, 0), 60.0);
    sample(&fx, &b, "5h", day(2, 1), 10.0);
    let v = timeline(&fx, Scope::All);
    assert_eq!(ids(&v.lanes), ["b"]);
    assert_eq!(v.lanes[0].in_use, flags("ffttttt"));
    assert_eq!(v.charts[0].peaks, peaks(&[(2, 10.0)]));
}

#[test]
fn a_sample_at_a_switch_counts_for_the_new_account() {
    let (fx, a, b, _) = fixture();
    in_use(&fx, &a, SINCE - 3_600);
    in_use(&fx, &b, day(3, 0));
    sample(&fx, &a, "5h", day(3, 0), 90.0);
    sample(&fx, &b, "5h", day(3, 0), 15.0);
    let v = timeline(&fx, Scope::All);
    assert_eq!(v.charts[0].peaks, peaks(&[(3, 15.0)]));
    assert_eq!(v.lanes[0].in_use, flags("tttffff"));
    assert_eq!(v.lanes[0].peaks, peaks(&[(3, 90.0)]));
}

#[test]
fn no_account_in_use_ends_the_span_before_it() {
    let (fx, a, _, _) = fixture();
    in_use(&fx, &a, SINCE - 3_600);
    nobody(&fx, day(2, 0));
    sample(&fx, &a, "5h", day(1, 0), 40.0);
    sample(&fx, &a, "5h", day(4, 0), 70.0);
    let v = timeline(&fx, Scope::All);
    assert_eq!(ids(&v.lanes), ["a"]);
    assert_eq!(v.lanes[0].in_use, flags("ttfffff"));
    assert_eq!(v.lanes[0].peaks, peaks(&[(1, 40.0), (4, 70.0)]));
    assert_eq!(v.charts[0].peaks, peaks(&[(1, 40.0)]));
}

#[test]
fn removed_accounts_follow_the_current_ones_in_order_of_first_use() {
    let (fx, a, b, c) = fixture();
    in_use(&fx, &c, SINCE - 3_600);
    in_use(&fx, &b, day(1, 0));
    in_use(&fx, &a, day(2, 0));
    let store = fx.engine.store().unwrap();
    store.delete_account(&b).unwrap();
    store.delete_account(&c).unwrap();
    let v = timeline(&fx, Scope::All);
    assert_eq!(ids(&v.lanes), ["a", "c", "b"]);
    let removed: Vec<bool> = v.lanes.iter().map(|l| l.account.removed).collect();
    assert_eq!(removed, [false, true, true]);
    assert_eq!(v.lanes[1].in_use, flags("tffffff"));
    assert_eq!(v.lanes[2].in_use, flags("ftfffff"));
}

#[test]
fn accounts_never_in_use_in_the_range_have_no_lane() {
    let (fx, a, _, c) = fixture();
    in_use(&fx, &c, SINCE - 7_200);
    in_use(&fx, &a, SINCE - 3_600);
    sample(&fx, &c, "5h", day(1, 0), 77.0);
    let v = timeline(&fx, Scope::All);
    assert_eq!(ids(&v.lanes), ["a"]);
    assert_eq!(v.charts[0].peaks, peaks(&[]), "c's samples are not in use");
}

#[test]
fn one_account_has_a_chart_per_relevant_window_and_its_in_use_lane() {
    let (fx, a) = first_fixture();
    let v = timeline(&fx, Scope::Account(a.clone()));
    let labels: Vec<&str> = v.charts.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["5h", "7d"]);
    assert_eq!(v.charts[0].peaks, peaks(&[(0, 60.0)]));
    assert_eq!(v.charts[1].peaks, peaks(&[(1, 30.0)]));
    assert_eq!(ids(&v.lanes), ["a"]);
    assert_eq!(v.lanes[0].in_use, vec![true; COLUMNS]);

    let unknown = fx.engine.timeline(
        &fx.provider(),
        Scope::Account(AccountId::from_string("nope")),
        SINCE,
        COLUMNS,
    );
    assert!(
        matches!(unknown, Err(EngineError::NoSuchAccount(_))),
        "{unknown:?}"
    );
}

#[test]
fn a_removed_account_in_use_in_the_range_keeps_its_lane() {
    let (fx, a) = first_fixture();
    fx.engine.store().unwrap().delete_account(&a).unwrap();
    let v = timeline(&fx, Scope::All);
    assert_eq!(
        v.lanes,
        [Lane {
            account: AccountRef {
                id: a,
                position: None,
                alias: None,
                email: None,
                removed: true,
            },
            peaks: vec![None; COLUMNS],
            in_use: vec![true; COLUMNS],
        }]
    );
}

#[test]
fn a_change_later_in_the_current_second_is_in_the_last_column() {
    // The view's grid ends at `now_s`; SQLite stamps the row in ms, 300 ms after it.
    let (fx, a, b, _) = fixture();
    in_use(&fx, &a, day(5, 0));
    in_use_ms(&fx, &b, T0 * 1000 + 300);
    let v = timeline(&fx, Scope::All);
    assert_eq!(ids(&v.lanes), ["a", "b"]);
    assert_eq!(v.lanes[0].in_use, flags("ffffftt"));
    assert_eq!(v.lanes[1].in_use, flags("fffffft"));
    // `a`'s span ends where `b`'s starts: at `now`, so a sample there is `b`'s.
    sample(&fx, &a, "5h", T0, 80.0);
    sample(&fx, &b, "5h", T0, 5.0);
    let v = timeline(&fx, Scope::All);
    assert_eq!(v.charts[0].peaks, peaks(&[(6, 5.0)]));
}

#[test]
fn a_change_from_the_next_second_on_is_in_no_column() {
    let (fx, a, b, c) = fixture();
    in_use(&fx, &a, day(5, 0));
    in_use_ms(&fx, &b, T0 * 1000 + 300);
    in_use_ms(&fx, &c, (T0 + 1) * 1000);
    let v = timeline(&fx, Scope::All);
    assert_eq!(ids(&v.lanes), ["a", "b"], "c is not in use yet");
    assert_eq!(v.lanes[1].in_use, flags("fffffft"));
}

#[test]
fn no_store_is_an_empty_timeline_and_creates_nothing() {
    let fx = Fx::new();
    let v = timeline(&fx, Scope::All);
    assert_eq!(
        v.charts,
        [Series {
            label: IN_USE.into(),
            peaks: vec![None; COLUMNS],
        }]
    );
    assert!(v.lanes.is_empty());
    assert_eq!(v.threshold, 90.0);
    assert_eq!((v.since_s, v.now_s, v.columns), (SINCE, T0, COLUMNS));
    let one = fx.engine.timeline(
        &fx.provider(),
        Scope::Account(AccountId::from_string("a")),
        SINCE,
        COLUMNS,
    );
    assert!(matches!(one, Err(EngineError::NoSuchAccount(_))), "{one:?}");
    assert!(!fx.env.data_dir().exists(), "no data directory");
}

#[test]
fn the_threshold_is_the_provider_s_effective_setting() {
    let fx = Fx::new();
    write_config(&fx, "[autoswitch]\nthreshold = 85\n");
    assert_eq!(timeline(&fx, Scope::All).threshold, 85.0);
}

/// An `events` row as written raw: by `auto`, at `at_ms`, with nothing else set.
#[derive(Clone, Copy)]
struct Raw<'a> {
    at_ms: i64,
    kind: &'a str,
    source: &'a str,
    from: Option<&'a str>,
    to: Option<&'a str>,
    trigger: Option<&'a str>,
    detail: Option<&'a str>,
}

/// A `kind` row by `auto` at `at_s`.
fn raw_event(at_s: i64, kind: &str) -> Raw<'_> {
    Raw {
        at_ms: at_s * 1000,
        kind,
        source: "auto",
        from: None,
        to: None,
        trigger: None,
        detail: None,
    }
}

/// Writes `e` past the store's typing, so a test sets its spellings itself.
fn put(fx: &Fx, e: Raw<'_>) {
    raw(fx)
        .execute(
            "INSERT INTO events (at, provider, kind, from_id, to_id, trigger, source, detail) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                e.at_ms,
                fx.provider().as_str(),
                e.kind,
                e.from,
                e.to,
                e.trigger,
                e.source,
                e.detail
            ],
        )
        .unwrap();
}

/// A current account as the timeline names it.
fn named(id: &AccountId, position: u32, alias: Option<&str>, email: &str) -> AccountRef {
    AccountRef {
        id: id.clone(),
        position: Some(position),
        alias: alias.map(str::to_owned),
        email: Some(email.to_owned()),
        removed: false,
    }
}

fn kinds(events: &[EventView]) -> Vec<EventKind> {
    events.iter().map(|e| e.kind).collect()
}

/// The fixture's events: a switch `a → b` just before the range and on day 0, a quarantine of
/// `c` on day 2, and a pause and a resume on days 4 and 5.
fn events_fixture() -> (Fx, AccountId, AccountId, AccountId) {
    let (fx, a, b, c) = fixture();
    let switch = Raw {
        from: Some("a"),
        to: Some("b"),
        trigger: Some("proactive"),
        ..raw_event(SINCE - 1, "switch")
    };
    put(&fx, switch);
    put(
        &fx,
        Raw {
            at_ms: day(0, 1) * 1000,
            ..switch
        },
    );
    put(
        &fx,
        Raw {
            to: Some("c"),
            detail: Some(r#"{"reason":"invalid_grant"}"#),
            ..raw_event(day(2, 0), "quarantine")
        },
    );
    put(&fx, raw_event(day(4, 0), "autoswitch-paused"));
    put(&fx, raw_event(day(5, 0), "autoswitch-resumed"));
    (fx, a, b, c)
}

#[test]
fn the_timeline_lists_the_scope_s_events_in_the_range_oldest_first() {
    let (fx, a, b, c) = events_fixture();
    let v = timeline(&fx, Scope::All);
    assert_eq!(
        v.events,
        [
            EventView {
                at_ms: day(0, 1) * 1000,
                kind: EventKind::Switch,
                source: EventSource::Auto,
                trigger: Some("proactive".into()),
                from: Some(AccountRef {
                    id: a.clone(),
                    position: Some(1),
                    alias: None,
                    email: Some("alex@example.com".into()),
                    removed: false,
                }),
                to: Some(named(&b, 2, Some("work"), "sam@corp.example")),
                detail: None,
                before_pct: None,
                after_pct: None,
            },
            EventView {
                at_ms: day(2, 0) * 1000,
                kind: EventKind::Quarantine,
                source: EventSource::Auto,
                trigger: None,
                from: None,
                to: Some(named(&c, 3, Some("spare"), "alex@personal.example")),
                detail: Some(json!({"reason": "invalid_grant"})),
                before_pct: None,
                after_pct: None,
            },
            EventView {
                at_ms: day(4, 0) * 1000,
                kind: EventKind::AutoswitchPaused,
                source: EventSource::Auto,
                trigger: None,
                from: None,
                to: None,
                detail: None,
                before_pct: None,
                after_pct: None,
            },
            EventView {
                at_ms: day(5, 0) * 1000,
                kind: EventKind::AutoswitchResumed,
                source: EventSource::Auto,
                trigger: None,
                from: None,
                to: None,
                detail: None,
                before_pct: None,
                after_pct: None,
            },
        ]
    );
    assert!(!v.paused_at_start);

    let pause_and_resume = [EventKind::AutoswitchPaused, EventKind::AutoswitchResumed];
    let v = timeline(&fx, Scope::Account(c));
    assert_eq!(
        kinds(&v.events),
        [&[EventKind::Quarantine][..], &pause_and_resume].concat()
    );
    let v = timeline(&fx, Scope::Account(a));
    assert_eq!(
        kinds(&v.events),
        [&[EventKind::Switch][..], &pause_and_resume].concat()
    );
    assert_eq!(v.events[0].at_ms, day(0, 1) * 1000);
}

#[test]
fn a_pause_that_began_before_the_range_draws_from_its_first_column() {
    let (fx, ..) = fixture();
    put(&fx, raw_event(SINCE - 3_600, "autoswitch-paused"));
    let v = timeline(&fx, Scope::All);
    assert!(v.paused_at_start);
    assert!(v.events.is_empty(), "{:?}", v.events);

    put(&fx, raw_event(SINCE - 60, "autoswitch-resumed"));
    let v = timeline(&fx, Scope::All);
    assert!(!v.paused_at_start);
    assert!(v.events.is_empty(), "{:?}", v.events);
}

#[test]
fn an_event_later_in_the_current_second_is_in_the_last_column() {
    let (fx, ..) = fixture();
    put(
        &fx,
        Raw {
            at_ms: T0 * 1000 + 300,
            ..raw_event(T0, "autoswitch-paused")
        },
    );
    put(
        &fx,
        Raw {
            at_ms: (T0 + 1) * 1000,
            ..raw_event(T0, "autoswitch-resumed")
        },
    );
    let v = timeline(&fx, Scope::All);
    assert_eq!(kinds(&v.events), [EventKind::AutoswitchPaused]);
    assert_eq!(v.grid().column_of(v.events[0].at_ms), Some(COLUMNS - 1));
}

#[test]
fn a_switch_carries_the_usage_just_before_and_just_after_it() {
    let (fx, a, b, c) = fixture();
    in_use(&fx, &a, SINCE - 3_600);
    in_use(&fx, &b, day(2, 12));
    sample(&fx, &a, "5h", day(2, 10), 91.0);
    sample(&fx, &a, "7d", day(2, 10), 40.0);
    sample(&fx, &a, "5h", day(2, 11), 93.0);
    sample(&fx, &a, "7d", day(2, 11), 41.0);
    sample(&fx, &b, "5h", day(2, 13), 3.0);
    sample(&fx, &b, "7d", day(2, 13), 2.0);
    put(
        &fx,
        Raw {
            from: Some("a"),
            to: Some("b"),
            ..raw_event(day(2, 12), "switch")
        },
    );
    put(
        &fx,
        Raw {
            to: Some("a"),
            ..raw_event(day(3, 0), "quarantine")
        },
    );
    // `b`'s only reading is after it, and `c` has none.
    put(
        &fx,
        Raw {
            from: Some("b"),
            to: Some("c"),
            ..raw_event(day(1, 0), "switch")
        },
    );
    let v = timeline(&fx, Scope::All);
    let at = |at_s: i64| {
        let e = v.events.iter().find(|e| e.at_ms == at_s * 1000).unwrap();
        (e.kind, e.before_pct, e.after_pct)
    };
    assert_eq!(at(day(2, 12)), (EventKind::Switch, Some(93.0), Some(3.0)));
    assert_eq!(at(day(3, 0)), (EventKind::Quarantine, None, None));
    assert_eq!(at(day(1, 0)), (EventKind::Switch, None, None));

    // At the range's edge, `from`'s last reading is from before it.
    sample(&fx, &c, "5h", SINCE - 60, 91.0);
    put(
        &fx,
        Raw {
            from: Some("c"),
            to: Some("a"),
            ..raw_event(SINCE + 60, "switch")
        },
    );
    let v = timeline(&fx, Scope::All);
    let edge = v
        .events
        .iter()
        .find(|e| e.at_ms == (SINCE + 60) * 1000)
        .unwrap();
    assert_eq!((edge.before_pct, edge.after_pct), (Some(91.0), Some(91.0)));
}

#[test]
fn a_removed_account_is_named_by_the_label_its_remove_event_stored() {
    let (fx, a, _, c) = fixture();
    in_use(&fx, &a, SINCE - 3_600);
    in_use(&fx, &c, day(1, 0));
    put(
        &fx,
        Raw {
            from: Some("a"),
            to: Some("c"),
            ..raw_event(day(1, 0), "switch")
        },
    );
    put(
        &fx,
        Raw {
            from: Some("c"),
            detail: Some(r#"{"label":"alex@personal.example"}"#),
            source: "cli",
            ..raw_event(day(2, 0), "remove")
        },
    );
    fx.engine.store().unwrap().delete_account(&c).unwrap();
    let removed = AccountRef {
        id: c.clone(),
        position: None,
        alias: None,
        email: Some("alex@personal.example".into()),
        removed: true,
    };
    let v = timeline(&fx, Scope::All);
    assert_eq!(kinds(&v.events), [EventKind::Switch, EventKind::Remove]);
    assert_eq!(v.events[0].to.as_ref(), Some(&removed));
    assert_eq!(v.events[1].from.as_ref(), Some(&removed));
    assert_eq!(ids(&v.lanes), ["a", "c"]);
    assert_eq!(v.lanes[1].account, removed);

    let (fx, _, _, c) = fixture();
    put(
        &fx,
        Raw {
            from: Some("a"),
            to: Some("c"),
            ..raw_event(day(1, 0), "switch")
        },
    );
    put(
        &fx,
        Raw {
            from: Some("c"),
            ..raw_event(day(2, 0), "remove")
        },
    );
    fx.engine.store().unwrap().delete_account(&c).unwrap();
    let v = timeline(&fx, Scope::All);
    assert_eq!(
        v.events[0].to,
        Some(AccountRef {
            id: c,
            position: None,
            alias: None,
            email: None,
            removed: true,
        })
    );
}

/// The provider's last auto-line event of the 30 days to `T0`.
fn last_auto(fx: &Fx) -> Option<EventView> {
    fx.engine
        .last_auto_event(&fx.provider(), (T0 - 30 * 86_400) * 1000)
        .unwrap()
}

#[test]
fn rows_a_newer_build_wrote_are_left_out_of_the_timeline_and_the_auto_line() {
    let (fx, ..) = fixture();
    put(
        &fx,
        Raw {
            to: Some("c"),
            ..raw_event(T0 - 3_600, "quarantine")
        },
    );
    put(
        &fx,
        Raw {
            source: "cli",
            ..raw_event(T0 - 60, "x-new-kind")
        },
    );
    put(
        &fx,
        Raw {
            source: "menubar",
            ..raw_event(T0 - 120, "switch")
        },
    );
    let v = timeline(&fx, Scope::All);
    assert_eq!(kinds(&v.events), [EventKind::Quarantine]);
    let last = last_auto(&fx).unwrap();
    assert_eq!(
        (last.kind, last.at_ms),
        (EventKind::Quarantine, (T0 - 3_600) * 1000)
    );
}

#[test]
fn last_auto_event_is_the_newest_auto_line_kind_in_its_bound() {
    let fx = Fx::new();
    assert_eq!(last_auto(&fx), None, "no store");
    assert!(!fx.env.data_dir().exists(), "no data directory");

    let (fx, ..) = fixture();
    let clear = || raw(&fx).execute("DELETE FROM events", []).unwrap();
    for kind in AUTO_LINE_KINDS {
        clear();
        put(&fx, raw_event(T0 - 60, kind.as_str()));
        let last = last_auto(&fx).unwrap();
        assert_eq!(
            (last.kind, last.at_ms),
            (kind, (T0 - 60) * 1000),
            "{kind:?}"
        );
    }
    for kind in ["add", "import", "remove"] {
        clear();
        put(&fx, raw_event(T0 - 60, kind));
        assert_eq!(last_auto(&fx), None, "{kind}");
    }

    clear();
    put(&fx, raw_event(T0 - 31 * 86_400, "switch"));
    assert_eq!(last_auto(&fx), None, "older than its bound");

    clear();
    put(&fx, raw_event(T0 - 120, "switch"));
    put(&fx, raw_event(T0 - 60, "add"));
    assert_eq!(last_auto(&fx).unwrap().kind, EventKind::Switch);

    put(&fx, raw_event(T0 - 30, "quarantine"));
    put(&fx, raw_event(T0 - 30, "unquarantine"));
    assert_eq!(last_auto(&fx).unwrap().kind, EventKind::Unquarantine);
}

#[test]
fn last_auto_event_names_its_accounts_as_they_read_now() {
    let (fx, a, b, _) = fixture();
    put(
        &fx,
        Raw {
            from: Some("a"),
            to: Some("b"),
            trigger: Some("proactive"),
            detail: Some(r#"{"window":"5h","label":"5h","pct":93.0,"threshold":90.0}"#),
            ..raw_event(T0 - 60, "switch")
        },
    );
    let store = fx.engine.store().unwrap();
    store.set_alias(&b, None).unwrap();
    let last = last_auto(&fx).unwrap();
    assert_eq!(last.to, Some(named(&b, 2, None, "sam@corp.example")));

    store.set_alias(&b, Some("work")).unwrap();
    store.move_to(&b, 5).unwrap();
    assert_eq!(
        last_auto(&fx).unwrap(),
        EventView {
            at_ms: (T0 - 60) * 1000,
            kind: EventKind::Switch,
            source: EventSource::Auto,
            trigger: Some("proactive".into()),
            from: Some(named(&a, 1, None, "alex@example.com")),
            to: Some(named(&b, 5, Some("work"), "sam@corp.example")),
            detail: Some(json!({"window": "5h", "label": "5h", "pct": 93.0, "threshold": 90.0})),
            before_pct: None,
            after_pct: None,
        }
    );
}
