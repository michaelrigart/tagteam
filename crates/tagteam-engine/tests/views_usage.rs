//! The usage views (§8.4, §8.7, §13.2, §13.4, §13.5): each account's status and last good
//! reading with pace and trust, `history`, and `statusline`'s cached live identity. Readings
//! are recorded through the store's own reserve-and-record calls, as the collector records
//! them, at times chosen by the test.

mod common;

use std::fs;

use common::Fx;
use serde_json::json;
use tagteam_core::pace::pace;
use tagteam_core::{
    AccountId, Pace, PollBudget, PollPlan, ProjectionMethod, Sample, Window, WindowKind,
};
use tagteam_engine::Engine;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::{LiveIdentityCacheRow, Reservation, Reserve};
use tagteam_engine::views::{
    HistoryView, HistoryWindow, NO_DATA, StatusView, StatuslineView, UsageStatus, UsageView,
};

/// The fixture clock's start (`Fx`), in seconds.
const T0: i64 = 1_790_000_000;

fn window(key: &str, label: &str, kind: WindowKind, pct: f64, resets_at: i64) -> Window {
    Window {
        key: key.into(),
        label: label.into(),
        kind,
        pct,
        resets_at: Some(resets_at),
        period_s: match kind {
            WindowKind::Short => Some(18_000),
            _ => Some(604_800),
        },
        detail: None,
    }
}

/// A Claude Code reading taken at `at`: 5h at `five` (resetting 2h40m30s later), 7d at `seven`
/// and Fable at 0 (both resetting at T0 + 3d09h00m30s, the same window instance for every
/// reading), and €0 of €20 spend.
fn reading(at: i64, five: f64, seven: f64) -> Vec<Window> {
    vec![
        window("5h", "5h", WindowKind::Short, five, at + 9_630),
        window("7d", "7d", WindowKind::Long, seven, T0 + 291_630),
        window(
            "scoped:Fable",
            "Fable",
            WindowKind::Scoped,
            0.0,
            T0 + 291_630,
        ),
        Window {
            key: "spend".into(),
            label: "spend".into(),
            kind: WindowKind::Spend,
            pct: 0.0,
            resets_at: None,
            period_s: None,
            detail: Some(json!({"used": 0.0, "limit": 20.0, "currency": "EUR"})),
        },
    ]
}

/// A slot and the `usage:<id>` lease at `at`, as the collector's phase 1 takes them.
fn reserve(fx: &Fx, id: &AccountId, at: i64) -> Reservation {
    let store = fx.engine.store().unwrap();
    let row = store.account(id).unwrap().unwrap();
    match store
        .reserve_usage(&row, at * 1000, false, &PollBudget::STANDARD)
        .unwrap()
    {
        Reserve::Reserved(r) => r,
        other => panic!("not reserved at {at}: {other:?}"),
    }
}

/// Records `windows` as `id`'s reading at `at`, with its next poll planned at `next_poll_at`.
fn record(fx: &Fx, id: &AccountId, windows: &[Window], at: i64, next_poll_at: i64) {
    let r = reserve(fx, id, at);
    let plan = PollPlan {
        interval_s: next_poll_at - at,
        next_poll_at,
    };
    let store = fx.engine.store().unwrap();
    assert!(store.record_usage(&r, windows, at, &plan, 180).unwrap());
}

/// Moves `id`'s next poll to `next_poll_at`, as a re-plan after a switch does (§8.6).
fn replan(fx: &Fx, id: &AccountId, next_poll_at: i64) {
    let plan = PollPlan {
        interval_s: 300,
        next_poll_at,
    };
    fx.engine.store().unwrap().set_poll_plan(id, &plan).unwrap();
}

/// Records a failed fetch of `kind` at `at`, backing off until `backoff_until`.
fn fail(
    fx: &Fx,
    id: &AccountId,
    kind: &str,
    at: i64,
    backoff_until: i64,
    last_429_at: Option<i64>,
) {
    let r = reserve(fx, id, at);
    let store = fx.engine.store().unwrap();
    assert!(
        store
            .record_usage_failure(&r, kind, at, backoff_until, last_429_at, None)
            .unwrap()
    );
}

fn at(fx: &Fx, s: i64) {
    fx.clock.set(s * 1000);
}

/// `id`'s usage as `list` shows it.
fn usage_of(engine: &Engine, id: &AccountId) -> UsageView {
    engine
        .accounts(None)
        .unwrap()
        .into_iter()
        .flat_map(|l| l.accounts)
        .find(|v| &v.row.id == id)
        .unwrap()
        .usage
}

/// §8.7's pace of `w` read at `fetched_at`, from exactly these `(fetched_at, pct)` samples of
/// its window instance.
fn expected(w: &Window, fetched_at: i64, samples: &[(i64, f64)]) -> Pace {
    let samples: Vec<Sample> = samples
        .iter()
        .map(|&(t, pct)| Sample {
            fetched_at: t,
            pct,
            resets_at: w.resets_at,
        })
        .collect();
    pace(w, fetched_at, &samples)
}

#[test]
fn a_fresh_reading_is_ok_decision_grade_and_carries_pace() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let windows = reading(T0, 9.0, 77.0);
    record(&fx, &a, &windows, T0, T0 + 180);

    let u = usage_of(&fx.engine, &a);
    assert_eq!(
        (u.status, u.decision_grade, u.fetched_at, u.age_s),
        (UsageStatus::Ok, true, Some(T0), Some(0))
    );
    assert_eq!((u.error, u.retry_at), (None, None));
    let shown = u.windows.unwrap();
    assert_eq!(shown.len(), windows.len());
    for ((w, p), stored) in shown.iter().zip(&windows) {
        assert_eq!(w, stored);
        assert_eq!(*p, expected(w, T0, &[(T0, w.pct)]), "{}", w.key);
    }
    // 77 % three and a half days into the week is ahead of pace (§8.7); a Short window has no
    // pace to be ahead of.
    assert_eq!(
        (shown[1].1.ahead, shown[1].1.method),
        (Some(true), Some(ProjectionMethod::Average))
    );
    assert_eq!(shown[0].1.ahead, None);

    // 301 s on, with no failure, no plan in force and no lease, the reading is still shown but
    // no longer decision-grade (§8.4).
    at(&fx, T0 + 301);
    let u = usage_of(&fx.engine, &a);
    assert_eq!(
        (u.status, u.decision_grade, u.age_s),
        (UsageStatus::Ok, false, Some(301))
    );
    assert!(u.windows.is_some());
}

#[test]
fn trust_extends_while_a_plan_is_in_force_or_a_fetch_is_in_flight() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    record(&fx, &a, &reading(T0, 9.0, 40.0), T0, T0 + 1_200);
    record(&fx, &b, &reading(T0, 9.0, 40.0), T0, T0 + 180);
    at(&fx, T0 + 1_000);
    assert!(usage_of(&fx.engine, &a).decision_grade, "a plan in force");
    assert!(!usage_of(&fx.engine, &b).decision_grade);
    // Another process holds b's lease: its fetch is in flight.
    reserve(&fx, &b, T0 + 1_100);
    at(&fx, T0 + 1_150);
    assert!(usage_of(&fx.engine, &b).decision_grade, "a live lease");
    at(&fx, T0 + 1_201);
    assert!(!usage_of(&fx.engine, &a).decision_grade);
    assert!(!usage_of(&fx.engine, &b).decision_grade);
}

#[test]
fn three_samples_over_two_hours_project_by_regression() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    for (i, seven) in [60.0, 70.0, 77.0].into_iter().enumerate() {
        let t = T0 + 3_600 * i as i64;
        record(&fx, &a, &reading(t, 9.0, seven), t, t + 180);
    }
    let last = T0 + 7_200;
    at(&fx, last);
    let windows = usage_of(&fx.engine, &a).windows.unwrap();
    let (w, p) = &windows[1];
    assert_eq!(
        *p,
        expected(w, last, &[(T0, 60.0), (T0 + 3_600, 70.0), (last, 77.0)])
    );
    assert_eq!(p.method, Some(ProjectionMethod::Regression));
}

#[test]
fn each_status_reaches_the_view() {
    let fx = Fx::new();
    let key = fx.add_api_key("sk-ant-api03-key");
    let quarantined = fx.add("q@x.co", "rt-q");
    let locked = fx.add("k@x.co", "rt-k");
    let tokenless = fx.add("n@x.co", "rt-n");
    let fresh = fx.add("f@x.co", "rt-f");
    fx.quarantine(&quarantined, "invalid_grant", "sha256:0");
    fail(&fx, &locked, "keychain-unavailable", T0, T0 + 30, None);
    fail(&fx, &tokenless, "no-access-token", T0, T0 + 30, None);
    let status = |id: &AccountId| usage_of(&fx.engine, id).status;
    assert_eq!(status(&key), UsageStatus::ApiKey);
    assert_eq!(status(&quarantined), UsageStatus::ReloginRequired);
    assert_eq!(status(&locked), UsageStatus::KeychainUnavailable);
    assert_eq!(status(&tokenless), UsageStatus::NoCredentials);
    // Never read and never failed: unavailable, for want of data (§13.2's `no-data`).
    let u = usage_of(&fx.engine, &fresh);
    assert_eq!(
        (u.status, u.error.as_deref(), u.retry_at, u.windows),
        (UsageStatus::Unavailable, Some(NO_DATA), None, None)
    );
}

#[test]
fn after_a_429_the_reading_stays_trusted_until_the_earliest_relevant_reset() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let mut windows = reading(T0, 9.0, 40.0);
    windows[0].resets_at = Some(T0 + 6_000); // 5h: the earliest relevant reset
    windows[2].resets_at = Some(T0 + 200); // Fable: not relevant under the default models
    windows[3].resets_at = Some(T0 + 100); // spend: never relevant
    record(&fx, &a, &windows, T0, T0 + 180);
    let t1 = T0 + 400;
    fail(&fx, &a, "http-429", t1, t1 + 330, Some(t1 + 330));

    at(&fx, t1);
    let u = usage_of(&fx.engine, &a);
    assert_eq!(
        (u.status, u.error.as_deref(), u.retry_at),
        (UsageStatus::Unavailable, Some("http-429"), Some(t1 + 330))
    );
    assert_eq!((u.fetched_at, u.age_s), (Some(T0), Some(400)));
    assert!(
        u.windows.is_some(),
        "a failure never touches the last good reading"
    );
    // Past the hour of extended trust, the 429 rule alone keeps the reading (§8.4) …
    at(&fx, T0 + 5_000);
    assert!(usage_of(&fx.engine, &a).decision_grade);
    // … until the earliest relevant reset: not Fable's, not spend's.
    at(&fx, T0 + 6_001);
    assert!(!usage_of(&fx.engine, &a).decision_grade);
}

#[test]
fn a_quarantined_account_gets_no_extended_trust() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    for id in [&a, &b] {
        record(&fx, id, &reading(T0, 9.0, 40.0), T0, T0 + 180);
        // A refresh that failed for good: a failure on record, as the quarantine leaves it.
        fail(&fx, id, "refresh-failed", T0 + 400, T0 + 430, None);
    }
    fx.quarantine(&a, "invalid_grant", "sha256:0");
    // Inside the hour of extended trust (§8.4), past the five-minute rule.
    at(&fx, T0 + 3_000);
    let (qa, ub) = (usage_of(&fx.engine, &a), usage_of(&fx.engine, &b));
    assert_eq!(qa.status, UsageStatus::ReloginRequired);
    assert!(!qa.decision_grade, "never retried, so never extended");
    assert!(
        ub.decision_grade,
        "the same state, still retried, is extended"
    );
}

#[test]
fn every_failing_status_says_why_and_when_it_is_retried_while_the_retry_is_ahead() {
    // §13.2: not only `unavailable`: any status that comes of failures being retried.
    let fx = Fx::new();
    let cases = [
        ("keychain-unavailable", UsageStatus::KeychainUnavailable),
        ("no-access-token", UsageStatus::NoCredentials),
        ("token-expired", UsageStatus::TokenExpired),
        ("foreign-credential", UsageStatus::ForeignCredential),
        ("http-500", UsageStatus::Unavailable),
    ];
    let ids: Vec<AccountId> = cases
        .iter()
        .enumerate()
        .map(|(i, (kind, _))| {
            let id = fx.add(&format!("f{i}@x.co"), &format!("rt-{i}"));
            fail(&fx, &id, kind, T0, T0 + 30, None);
            id
        })
        .collect();
    at(&fx, T0 + 10);
    for (id, (kind, status)) in ids.iter().zip(cases) {
        let u = usage_of(&fx.engine, id);
        assert_eq!(
            (u.status, u.error.as_deref(), u.retry_at),
            (status, Some(kind), Some(T0 + 30)),
            "{kind}"
        );
    }
    // Once the retry is due there is nothing ahead to say; why it failed still stands.
    at(&fx, T0 + 31);
    for (id, (kind, status)) in ids.iter().zip(cases) {
        let u = usage_of(&fx.engine, id);
        assert_eq!(
            (u.status, u.error.as_deref(), u.retry_at),
            (status, Some(kind), None),
            "{kind}"
        );
    }
}

#[test]
fn an_unread_account_over_its_budget_is_retried_when_a_slot_frees() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fail(&fx, &a, "over-budget", T0, T0 + 600, None);
    at(&fx, T0 + 60);
    let u = usage_of(&fx.engine, &a);
    assert_eq!(
        (u.status, u.error.as_deref(), u.windows, u.retry_at),
        (
            UsageStatus::Unavailable,
            Some("over-budget"),
            None,
            Some(T0 + 600)
        )
    );
}

#[test]
fn only_a_status_that_is_retried_has_a_retry_time() {
    let fx = Fx::new();
    let ok = fx.add("o@x.co", "rt-o");
    let quarantined = fx.add("q@x.co", "rt-q");
    // A plan is in force for both, and a failure is on record for the quarantined one.
    record(&fx, &ok, &reading(T0, 9.0, 40.0), T0, T0 + 180);
    replan(&fx, &ok, T0 + 1_200);
    record(&fx, &quarantined, &reading(T0, 9.0, 40.0), T0, T0 + 180);
    fail(
        &fx,
        &quarantined,
        "refresh-failed",
        T0 + 200,
        T0 + 260,
        None,
    );
    replan(&fx, &quarantined, T0 + 1_200);
    fx.quarantine(&quarantined, "invalid_grant", "sha256:0");
    at(&fx, T0 + 220);
    let u = usage_of(&fx.engine, &ok);
    assert_eq!(
        (u.status, u.error, u.retry_at),
        (UsageStatus::Ok, None, None)
    );
    let u = usage_of(&fx.engine, &quarantined);
    assert_eq!(
        (u.status, u.retry_at),
        (UsageStatus::ReloginRequired, None),
        "never retried, so never a retry time"
    );
}

#[test]
fn the_retry_is_the_later_of_the_backoff_and_the_next_planned_poll() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    record(&fx, &a, &reading(T0, 9.0, 40.0), T0, T0 + 180);
    fail(&fx, &a, "http-500", T0 + 400, T0 + 430, None);
    // A switch re-plans the account's poll beyond the backoff.
    replan(&fx, &a, T0 + 1_200);
    let state = fx.engine.store().unwrap().usage_state(&a).unwrap().unwrap();
    assert_eq!(
        (state.backoff_until, state.next_poll_at),
        (Some(T0 + 430), Some(T0 + 1_200)),
        "the plan is the later of the two"
    );
    at(&fx, T0 + 410);
    assert_eq!(usage_of(&fx.engine, &a).retry_at, Some(T0 + 1_200));
}

#[test]
fn a_clock_skewed_plan_is_not_a_plan_in_force_for_trust() {
    // A 600 s-old reading with no failure, no lease and no recent 429 is decision-grade only
    // while a poll is planned. A `next_poll_at` a day ahead (a clock that ran ahead, then was
    // corrected) is skew, as `reserve_usage` treats it (§8.4), so it extends nothing; a legal
    // plan still does.
    let fx = Fx::new();
    let legal = fx.add("a@x.co", "rt-a");
    let skewed = fx.add("b@x.co", "rt-b");
    record(&fx, &legal, &reading(T0, 9.0, 40.0), T0, T0 + 1_200);
    record(&fx, &skewed, &reading(T0, 9.0, 40.0), T0, T0 + 86_400);
    at(&fx, T0 + 600);
    assert!(usage_of(&fx.engine, &legal).decision_grade, "a legal plan");
    assert!(
        !usage_of(&fx.engine, &skewed).decision_grade,
        "a skewed plan is no plan"
    );
}

#[test]
fn a_retry_time_the_reserve_rule_ignores_as_clock_skew_is_not_shown() {
    // A `next_poll_at` a day ahead (a clock that ran ahead, then was corrected) is ignored by
    // `reserve_usage` (§8.4), so it is no retry time either: the backoff 20 s ahead is.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    record(&fx, &a, &reading(T0, 9.0, 40.0), T0, T0 + 180);
    fail(&fx, &a, "http-500", T0 + 400, T0 + 430, None);
    replan(&fx, &a, T0 + 410 + 86_400);
    at(&fx, T0 + 410);
    let u = usage_of(&fx.engine, &a);
    assert_eq!(u.status, UsageStatus::Unavailable);
    assert_eq!(u.retry_at, Some(T0 + 430));
    // The bound is the reserve rule's: a plan one count window out is still a retry time.
    replan(&fx, &a, T0 + 410 + 3_660);
    assert_eq!(usage_of(&fx.engine, &a).retry_at, Some(T0 + 410 + 3_660));
}

#[test]
fn a_plan_in_force_never_extends_a_quarantined_accounts_trust() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    for id in [&a, &b] {
        record(&fx, id, &reading(T0, 9.0, 40.0), T0, T0 + 1_200);
    }
    fx.quarantine(&a, "invalid_grant", "sha256:0");
    // Past the five-minute rule, inside the planned poll: no failure on record at all.
    at(&fx, T0 + 1_000);
    assert!(usage_of(&fx.engine, &b).decision_grade, "a plan in force");
    assert!(
        !usage_of(&fx.engine, &a).decision_grade,
        "a quarantined account is never polled, so a plan means nothing"
    );
}

#[test]
fn status_and_account_view_carry_the_listed_usage() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    record(&fx, &a, &reading(T0, 9.0, 77.0), T0, T0 + 180);
    let listed = usage_of(&fx.engine, &a);
    let StatusView::Managed { account, .. } = fx.engine.status(&fx.provider()).unwrap() else {
        panic!("a is live");
    };
    assert_eq!(account.usage, listed);
    let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
    assert_eq!(fx.engine.account_view(row, true).usage, listed);
}

/// `a`, live, read at T0, T0 + 1 h and T0 + 2 h (7d at 60, 70 and 77 %); the clock at T0 + 2 h.
fn with_history(fx: &Fx) -> AccountId {
    let a = fx.add("a@x.co", "rt-a");
    for (i, seven) in [60.0, 70.0, 77.0].into_iter().enumerate() {
        let t = T0 + 3_600 * i as i64;
        record(fx, &a, &reading(t, 9.0 + i as f64, seven), t, t + 180);
    }
    at(fx, T0 + 7_200);
    a
}

fn keys(h: &HistoryView) -> Vec<&str> {
    h.windows.iter().map(|w| w.window.key.as_str()).collect()
}

#[test]
fn history_defaults_to_the_relevant_windows_and_bounds_samples_by_since() {
    let fx = Fx::new();
    let a = with_history(&fx);
    let h = fx.engine.history(&a, None, T0 + 1).unwrap();
    assert_eq!(
        (h.account.row.id.clone(), h.account.active),
        (a.clone(), true)
    );
    assert_eq!(keys(&h), ["5h", "7d"]);
    let seven = &h.windows[1];
    let shown: Vec<(i64, f64)> = seven
        .samples
        .iter()
        .map(|s| (s.fetched_at, s.pct))
        .collect();
    assert_eq!(shown, [(T0 + 3_600, 70.0), (T0 + 7_200, 77.0)]);
    // Pace reads the 48 h before the reading, whatever `since` shows: the list's own.
    let listed = usage_of(&fx.engine, &a).windows.unwrap();
    assert_eq!(seven.pace, listed[1].1);
    assert_eq!(seven.pace.method, Some(ProjectionMethod::Regression));
    assert!(
        fx.http.requests().is_empty(),
        "history never fetches (§13.4)"
    );
}

#[test]
fn history_filters_by_key_or_label_ignoring_case() {
    let fx = Fx::new();
    let a = with_history(&fx);
    for name in ["FABLE", "scoped:fable"] {
        let h = fx.engine.history(&a, Some(name), 0).unwrap();
        assert_eq!(keys(&h), ["scoped:Fable"], "{name}");
        assert_eq!(h.windows[0].samples.len(), 3, "{name}");
    }
    assert_eq!(
        keys(&fx.engine.history(&a, Some("Spend"), 0).unwrap()),
        ["spend"]
    );
    assert!(
        fx.engine
            .history(&a, Some("nope"), 0)
            .unwrap()
            .windows
            .is_empty()
    );
}

#[test]
fn history_follows_the_configured_models() {
    let fx = Fx::new();
    let a = with_history(&fx);
    let settings = Settings {
        models: vec!["fable".into()],
        ..Settings::default()
    };
    let engine = fx.engine_with_settings(settings);
    assert_eq!(
        keys(&engine.history(&a, None, 0).unwrap()),
        ["5h", "7d", "scoped:Fable"]
    );
}

/// `samples`' `(fetched_at, pct)` pairs.
fn shown(w: &HistoryWindow) -> Vec<(i64, f64)> {
    w.samples.iter().map(|s| (s.fetched_at, s.pct)).collect()
}

#[test]
fn history_shows_a_sampled_window_the_latest_reading_lacks() {
    // §13.4: `history` dumps the samples it holds. Fable was read hourly from T0 to T0 + 2 h,
    // then the endpoint stopped reporting it: its samples are still history, its window
    // described by the provider and read as of its latest sample.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let fable_samples = [(T0, 10.0), (T0 + 3_600, 20.0), (T0 + 7_200, 30.0)];
    for (t, fable) in fable_samples {
        let mut windows = reading(t, 9.0, 60.0);
        windows[2].pct = fable;
        record(&fx, &a, &windows, t, t + 180);
    }
    let last = T0 + 10_800;
    let without_fable: Vec<Window> = reading(last, 11.0, 70.0)
        .into_iter()
        .filter(|w| w.kind != WindowKind::Scoped)
        .collect();
    record(&fx, &a, &without_fable, last, last + 180);
    at(&fx, last);

    let h = fx.engine.history(&a, Some("Fable"), 0).unwrap();
    assert_eq!(keys(&h), ["scoped:Fable"]);
    let fable = &h.windows[0];
    let described = Window {
        key: "scoped:Fable".into(),
        label: "Fable".into(),
        kind: WindowKind::Scoped,
        pct: 30.0,
        resets_at: Some(T0 + 291_630),
        period_s: None,
        detail: None,
    };
    assert_eq!(fable.window, described);
    assert_eq!(shown(fable), fable_samples);
    assert_eq!(
        fable.pace,
        expected(&described, T0 + 7_200, &fable_samples),
        "the shared pace helper, as of the window's latest sample"
    );
    assert_eq!(fable.pace.method, Some(ProjectionMethod::Regression));
    assert_eq!(
        keys(&fx.engine.history(&a, None, 0).unwrap()),
        ["5h", "7d"],
        "not relevant under the default models"
    );
    let settings = Settings {
        models: vec!["fable".into()],
        ..Settings::default()
    };
    assert_eq!(
        keys(
            &fx.engine_with_settings(settings)
                .history(&a, None, 0)
                .unwrap()
        ),
        ["5h", "7d", "scoped:Fable"],
        "relevant when the settings name it (§8.2)"
    );
    assert!(fx.http.requests().is_empty(), "history never fetches");
}

#[test]
fn history_of_an_empty_latest_reading_still_shows_the_relevant_samples() {
    // An empty reading is stored as no windows (§8.2), yet the samples before it remain.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    record(&fx, &a, &reading(T0, 9.0, 60.0), T0, T0 + 180);
    record(
        &fx,
        &a,
        &reading(T0 + 3_600, 12.0, 70.0),
        T0 + 3_600,
        T0 + 3_780,
    );
    record(&fx, &a, &[], T0 + 7_200, T0 + 7_380);
    at(&fx, T0 + 7_200);

    let h = fx.engine.history(&a, None, 0).unwrap();
    assert_eq!(keys(&h), ["5h", "7d"]);
    let five = &h.windows[0];
    assert_eq!(
        (five.window.kind, five.window.pct, five.window.period_s),
        (WindowKind::Short, 12.0, Some(18_000))
    );
    assert_eq!(shown(five), [(T0, 9.0), (T0 + 3_600, 12.0)]);
    assert_eq!(shown(&h.windows[1]), [(T0, 60.0), (T0 + 3_600, 70.0)]);
    assert_eq!(
        shown(&fx.engine.history(&a, None, T0 + 1).unwrap().windows[0]),
        [(T0 + 3_600, 12.0)],
        "since still bounds the samples"
    );
}

#[test]
fn history_says_when_a_window_filter_matched_nothing() {
    let fx = Fx::new();
    let a = with_history(&fx);
    let never = fx.add("n@x.co", "rt-n");
    let unmatched = |id: &AccountId, window: Option<&str>| {
        let h = fx.engine.history(id, window, 0).unwrap();
        (h.windows.is_empty(), h.unmatched_window)
    };
    assert_eq!(unmatched(&a, Some("nope")), (true, true));
    assert_eq!(unmatched(&a, Some("FABLE")), (false, false));
    assert_eq!(unmatched(&a, None), (false, false));
    // An account with nothing to name has nothing a filter could miss.
    assert_eq!(unmatched(&never, Some("nope")), (true, false));
}

#[test]
fn history_names_the_account_as_the_list_does() {
    let fx = Fx::new();
    let a = with_history(&fx);
    let b = fx.add("b@x.co", "rt-b");
    let agree = |state: &str| {
        for l in fx.engine.accounts(None).unwrap() {
            for v in l.accounts {
                let h = fx.engine.history(&v.row.id, None, 0).unwrap().account;
                assert_eq!(
                    (h.active, h.usage, h.row.id),
                    (v.active, v.usage, v.row.id),
                    "{state}"
                );
            }
        }
    };
    agree("b live");
    assert!(!fx.engine.history(&a, None, 0).unwrap().account.active);
    assert!(fx.engine.history(&b, None, 0).unwrap().account.active);
    // The live identity unreadable: the store's active account stands in for both.
    fs::write(fx.paths().global_config, "{ \"oauthAccount\": ").unwrap();
    agree("live login unreadable");
}

#[test]
fn history_of_an_account_never_read_is_empty_and_of_an_unknown_one_an_error() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    assert!(fx.engine.history(&a, None, 0).unwrap().windows.is_empty());
    let unknown = fx.engine.history(&AccountId::from_string("nope"), None, 0);
    assert_eq!(unknown.unwrap_err().kind(), "no-such-account");
}

fn cache(fx: &Fx) -> Option<LiveIdentityCacheRow> {
    fx.engine
        .store()
        .unwrap()
        .live_identity_cache(&fx.provider())
        .unwrap()
}

fn managed(v: StatuslineView) -> AccountId {
    match v {
        StatuslineView::Managed { account } => account.row.id,
        other => panic!("not managed: {other:?}"),
    }
}

#[test]
fn statusline_shows_the_live_account_with_its_usage_or_an_unmanaged_email() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    record(&fx, &b, &reading(T0, 9.0, 77.0), T0, T0 + 180);
    match fx.engine.statusline(&fx.provider()).unwrap() {
        StatuslineView::Managed { account } => {
            assert_eq!((account.row.id.clone(), account.active), (b.clone(), true));
            // The line never shows pace, so it is not computed (§13.5): everything else is the
            // list's own.
            let listed = usage_of(&fx.engine, &b);
            let paceless = UsageView {
                windows: listed
                    .windows
                    .clone()
                    .map(|ws| ws.into_iter().map(|(w, _)| (w, Pace::default())).collect()),
                ..listed.clone()
            };
            assert_eq!(account.usage, paceless);
            assert!(
                listed
                    .windows
                    .unwrap()
                    .iter()
                    .any(|(_, p)| *p != Pace::default()),
                "the list's own pace is not empty, or this proves nothing"
            );
        }
        other => panic!("{other:?}"),
    }
    fx.login("stranger@x.co", "rt-s");
    assert!(matches!(
        fx.engine.statusline(&fx.provider()).unwrap(),
        StatuslineView::Unmanaged { email } if email == "stranger@x.co"
    ));
}

#[test]
fn a_missing_garbled_or_rewritten_claude_json_is_read_as_it_is_now() {
    // Review Focus 5, at the engine level.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let config = fx.paths().global_config;
    assert_eq!(managed(fx.engine.statusline(&fx.provider()).unwrap()), a);
    let key_a = fx
        .engine
        .store()
        .unwrap()
        .account(&a)
        .unwrap()
        .unwrap()
        .identity_key;
    let cached = cache(&fx).unwrap();
    assert_eq!(
        (cached.path.as_str(), cached.identity_key.as_deref()),
        (config.to_str().unwrap(), Some(key_a.as_str()))
    );
    // Rewritten, by a login of another length, so the stamp differs even where mtimes are
    // coarse: parsed again, and the cache follows.
    fx.login("bobby@x.co", "rt-bobby");
    assert!(matches!(
        fx.engine.statusline(&fx.provider()).unwrap(),
        StatuslineView::Unmanaged { email } if email == "bobby@x.co"
    ));
    assert_eq!(cache(&fx).unwrap().label.as_deref(), Some("bobby@x.co"));
    // Garbled: nothing to show, and no error. It is not cached: the row still holds the
    // identity and the stamp of the last file that parsed, so the next run parses again.
    let before = cache(&fx).unwrap();
    fs::write(&config, "{ \"oauthAccount\": ").unwrap();
    assert!(matches!(
        fx.engine.statusline(&fx.provider()).unwrap(),
        StatuslineView::NoLogin
    ));
    let after = cache(&fx).unwrap();
    assert_eq!(after, before, "a garbled file is not cached");
    assert_eq!(after.label.as_deref(), Some("bobby@x.co"));
    // Missing: nothing to show.
    fs::remove_file(&config).unwrap();
    assert!(matches!(
        fx.engine.statusline(&fx.provider()).unwrap(),
        StatuslineView::NoLogin
    ));
    // Written again: read again.
    fs::write(&config, common::CLAUDE_JSON).unwrap();
    fx.login("a@x.co", "rt-a");
    assert_eq!(managed(fx.engine.statusline(&fx.provider()).unwrap()), a);
}

#[test]
fn an_unchanged_claude_json_is_not_parsed_again() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    assert_eq!(managed(fx.engine.statusline(&fx.provider()).unwrap()), b);
    // A cache row under the file's current stamp that names a instead: only a lookup that
    // skipped the parse can answer a.
    let store = fx.engine.store().unwrap();
    let key_a = store.account(&a).unwrap().unwrap().identity_key;
    let planted = LiveIdentityCacheRow {
        identity_key: Some(key_a),
        label: Some("a@x.co".into()),
        ..cache(&fx).unwrap()
    };
    store.put_live_identity_cache(&planted).unwrap();
    assert_eq!(managed(fx.engine.statusline(&fx.provider()).unwrap()), a);
}

#[test]
fn a_new_mtime_alone_makes_the_next_run_parse_again() {
    // §13.5 keys the cache on mtime and size together: the same size under a new mtime is a
    // different version of the file.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let config = fx.paths().global_config;
    assert_eq!(managed(fx.engine.statusline(&fx.provider()).unwrap()), b);
    let store = fx.engine.store().unwrap();
    let key_a = store.account(&a).unwrap().unwrap().identity_key;
    let key_b = store.account(&b).unwrap().unwrap().identity_key;
    let planted = LiveIdentityCacheRow {
        identity_key: Some(key_a),
        label: Some("a@x.co".into()),
        ..cache(&fx).unwrap()
    };
    store.put_live_identity_cache(&planted).unwrap();
    // Unchanged stamp: the planted row is believed.
    assert_eq!(managed(fx.engine.statusline(&fx.provider()).unwrap()), a);
    let size = fs::metadata(&config).unwrap().len();
    let mtime = fs::metadata(&config).unwrap().modified().unwrap();
    fs::File::options()
        .write(true)
        .open(&config)
        .unwrap()
        .set_modified(mtime + std::time::Duration::from_secs(2))
        .unwrap();
    assert_eq!(fs::metadata(&config).unwrap().len(), size);
    assert_eq!(managed(fx.engine.statusline(&fx.provider()).unwrap()), b);
    let row = cache(&fx).unwrap();
    assert_eq!(row.identity_key, Some(key_b));
    assert_ne!(
        row.mtime_ns, planted.mtime_ns,
        "the cache follows the new stamp"
    );
}

#[test]
fn a_busy_store_costs_the_statusline_neither_its_line_nor_its_time() {
    // §13.5: the identity cache is only a cache. Another process holds the write lock, as a
    // collector recording a reading does; the line is still right and is not kept waiting.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    assert_eq!(managed(fx.engine.statusline(&fx.provider()).unwrap()), b);
    // A login that changes the file: the next run must parse it and would write the cache.
    fx.login("a@x.co", "rt-a");
    let db = fx.env.data_dir().join("tagteam.db");
    let other = rusqlite::Connection::open(&db).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = std::time::Instant::now();
    let view = fx.engine.statusline(&fx.provider()).unwrap();
    let took = started.elapsed();
    other.execute_batch("ROLLBACK").unwrap();
    assert_eq!(managed(view), a);
    assert!(
        took < std::time::Duration::from_millis(100),
        "the statusline waited {took:?} on the write lock"
    );
    // The contended write was skipped: the cache still holds b, the last file that was cached.
    let key_b = fx
        .engine
        .store()
        .unwrap()
        .account(&b)
        .unwrap()
        .unwrap()
        .identity_key;
    assert_eq!(cache(&fx).unwrap().identity_key, Some(key_b));
    // The next, uncontended run parses again and caches a.
    assert_eq!(managed(fx.engine.statusline(&fx.provider()).unwrap()), a);
    let key_a = fx
        .engine
        .store()
        .unwrap()
        .account(&a)
        .unwrap()
        .unwrap()
        .identity_key;
    assert_eq!(cache(&fx).unwrap().identity_key, Some(key_a));
}

#[test]
fn statusline_needs_no_keychain_no_network_and_creates_no_store() {
    let fx = Fx::new();
    fx.login("a@x.co", "rt-a");
    fx.kc.set_locked(true);
    assert!(matches!(
        fx.engine.statusline(&fx.provider()).unwrap(),
        StatuslineView::Unmanaged { email } if email == "a@x.co"
    ));
    assert!(
        !fx.env.data_dir().join("tagteam.db").exists(),
        "§13.5: never creates the store"
    );

    fx.kc.set_locked(false);
    let a = fx.engine.add_live(fx.add_options()).unwrap().account.id;
    record(&fx, &a, &reading(T0, 9.0, 77.0), T0, T0 + 180);
    fx.http.clear();
    fx.kc.set_locked(true);
    match fx.engine.statusline(&fx.provider()).unwrap() {
        StatuslineView::Managed { account } => {
            assert_eq!(account.usage.status, UsageStatus::Ok);
            assert!(account.usage.windows.is_some());
        }
        other => panic!("{other:?}"),
    }
    assert!(fx.http.requests().is_empty(), "no request");
    assert_eq!(fx.kc.unlock_attempts(), 0);
}

#[test]
fn decision_windows_are_the_reading_while_it_is_decision_grade_under_the_models_given() {
    // §8.4 after a 429: trusted until the earliest relevant reset, capped at two hours. Fable
    // resets first here, so it shortens that trust only when it counts (§8.2).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let mut windows = reading(T0, 9.0, 40.0);
    windows[2].resets_at = Some(T0 + 1_000);
    record(&fx, &a, &windows, T0, T0 + 180);
    fail(&fx, &a, "http-429", T0 + 200, T0 + 5_000, Some(T0 + 5_000));
    let store = fx.engine.store().unwrap();
    let a_row = store.account(&a).unwrap().unwrap();
    let b_row = store.account(&b).unwrap().unwrap();
    let fable = ["Fable".to_owned()];

    at(&fx, T0 + 100);
    assert_eq!(
        fx.engine.decision_windows(&a_row, &fable).unwrap(),
        Some(windows.clone()),
        "a young reading counts under any models"
    );
    assert_eq!(
        fx.engine.decision_windows(&b_row, &[]).unwrap(),
        None,
        "never read"
    );

    at(&fx, T0 + 4_000);
    assert_eq!(
        fx.engine.decision_windows(&a_row, &[]).unwrap(),
        Some(windows),
        "trusted until the 5h reset, capped at T0 + 7200"
    );
    assert_eq!(
        fx.engine.decision_windows(&a_row, &fable).unwrap(),
        None,
        "Fable's reset, at T0 + 1000, has passed"
    );
}

#[test]
fn decision_windows_take_a_clock_skewed_plan_for_no_plan() {
    // As the views do (§8.4): past the five-minute rule, a legal plan keeps a reading
    // decision-grade, and a `next_poll_at` a day ahead (clock skew) does not.
    let fx = Fx::new();
    let legal = fx.add("a@x.co", "rt-a");
    let skewed = fx.add("b@x.co", "rt-b");
    record(&fx, &legal, &reading(T0, 9.0, 40.0), T0, T0 + 1_200);
    record(&fx, &skewed, &reading(T0, 9.0, 40.0), T0, T0 + 86_400);
    at(&fx, T0 + 600);
    let store = fx.engine.store().unwrap();
    for (id, grade) in [(&legal, true), (&skewed, false)] {
        let row = store.account(id).unwrap().unwrap();
        assert_eq!(
            fx.engine.decision_windows(&row, &[]).unwrap().is_some(),
            grade,
            "{}",
            row.label
        );
    }
}
