//! §11.2's tick and §11.4's events, against Claude Code and the test-only FakeAgent (§15.2).
//! FakeAgent names no long window, so its consume-first setting runs `best`. Readings are
//! recorded through the store's own reserve and record, with a plan in force, so a tick's
//! scheduled collection finds nothing due unless a test says otherwise.
mod common;

use std::collections::BTreeMap;
use std::fs;
use std::sync::Mutex;

use common::{
    API_KEY, FakeFx, Fx, Recorded, crashed_switch, record_reading, usage_requests, usage_window,
    vault_fp, write_target_credential,
};
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_core::autoswitch::{
    AutoConfig, AutoState, Decision, NoSwitchReason, Outcome, Strategy, Trigger,
};
use tagteam_core::{AccountId, ProviderId, Window, WindowKind};
use tagteam_engine::Engine;
use tagteam_engine::auto::{AutoEvent, EventSink, TickOutcome};
use tagteam_engine::vault::SERVICE;
use tagteam_fake::FakePaths;
use tagteam_provider::{Keychain, Provider};

/// The fixture clock's start (`Fx`), in seconds.
const T0: i64 = 1_790_000_000;
/// When every reading's short window resets.
const SHORT_RESETS: i64 = T0 + 9_630;
/// When every reading's long window resets, unless a test says otherwise.
const LONG_RESETS: i64 = T0 + 291_630;

fn cfg(p: &dyn Provider) -> AutoConfig {
    AutoConfig {
        threshold: 90.0,
        hysteresis_pct: 10.0,
        cooldown_s: 300,
        interval_s: 60,
        unhealthy_ticks: 3,
        strategy: Strategy::Best,
        include_api_key_accounts: false,
        models: Vec::new(),
        long_window: p.primary_long_window().map(str::to_owned),
    }
}

/// A reading of `short` and `long` percent in the provider's own windows: Claude Code's 5h
/// and 7d, or FakeAgent's daily and monthly.
fn reading(fake: bool, short: f64, long: f64) -> Vec<Window> {
    let (s, l) = if fake {
        ("daily", "monthly")
    } else {
        ("5h", "7d")
    };
    vec![
        usage_window(s, WindowKind::Short, short, SHORT_RESETS),
        usage_window(l, WindowKind::Long, long, LONG_RESETS),
    ]
}

/// A reading taken now with its next poll five minutes out: decision-grade, and not due.
fn read(engine: &Engine, id: &AccountId, windows: &[Window]) {
    record_reading(engine, id, windows, T0, T0 + 300);
}

/// The windows of a reading as `poll`'s `windowsPct` gives them.
fn pcts(windows: &[Window]) -> BTreeMap<String, f64> {
    windows.iter().map(|w| (w.key.clone(), w.pct)).collect()
}

/// `a` at position 1 and `b` at position 2, which is live, on Claude Code or on FakeAgent;
/// with the provider and each account's email (its label, for FakeAgent).
struct Two {
    provider: ProviderId,
    a: AccountId,
    b: AccountId,
    a_email: &'static str,
    b_email: &'static str,
    config: AutoConfig,
}

fn two(ffx: &FakeFx, fake: bool) -> Two {
    if fake {
        Two {
            provider: ffx.fake_provider(),
            a: ffx.fake_add("alice", "tok-a", "renew-a"),
            b: ffx.fake_add("bob", "tok-b", "renew-b"),
            a_email: "alice@ws",
            b_email: "bob@ws",
            config: cfg(ffx.fake.as_ref()),
        }
    } else {
        Two {
            provider: ffx.fx.provider(),
            a: ffx.fx.add("a@x.co", "rt-a"),
            b: ffx.fx.add("b@x.co", "rt-b"),
            a_email: "a@x.co",
            b_email: "b@x.co",
            config: cfg(ffx.fx.cc.as_ref()),
        }
    }
}

/// The live login's email (its label, for FakeAgent).
fn live(ffx: &FakeFx, fake: bool) -> Option<String> {
    if fake {
        ffx.fake_live_label()
    } else {
        ffx.fx.live_email()
    }
}

fn no_switch(provider: &ProviderId, reason: &str, detail: &str) -> AutoEvent {
    AutoEvent::NoSwitch {
        provider: provider.clone(),
        reason: reason.into(),
        detail: detail.into(),
    }
}

fn row_count(fx: &Fx, table: &str) -> i64 {
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn auto_state(fx: &Fx) -> AutoState {
    fx.engine
        .store()
        .unwrap()
        .autoswitch_state(&fx.provider())
        .unwrap()
}

#[test]
fn a_tick_below_the_threshold_polls_and_stays() {
    for fake in [false, true] {
        let ffx = FakeFx::new();
        let t = two(&ffx, fake);
        let (low, high) = (reading(fake, 10.0, 10.0), reading(fake, 20.0, 50.0));
        read(&ffx.engine, &t.a, &low);
        read(&ffx.engine, &t.b, &high);
        let sink = Recorded::default();
        let mut engine = ffx
            .engine
            .auto(&t.provider, t.config, false)
            .unwrap()
            .unwrap();
        let (outcome, decision) = engine.tick(&sink).unwrap();
        assert_eq!(outcome, TickOutcome::NoAction, "fake={fake}");
        assert_eq!(
            decision,
            Decision::NoSwitch {
                reason: NoSwitchReason::BelowThreshold,
                outcome: Outcome::NoAction,
                detail: String::new(),
                earliest_reset: None,
            }
        );
        assert_eq!(
            sink.take(),
            [
                AutoEvent::Poll {
                    provider: t.provider.clone(),
                    active: Some((2, t.b_email.into())),
                    headroom_pct: BTreeMap::from([(1, Some(90.0)), (2, Some(50.0))]),
                    threshold: 90.0,
                    fetch_errors: BTreeMap::new(),
                    windows_pct: BTreeMap::from([(1, pcts(&low)), (2, pcts(&high))]),
                },
                no_switch(&t.provider, "below-threshold", ""),
            ],
            "fake={fake}"
        );
        assert!(ffx.fx.http.requests().is_empty(), "nothing was due");
        assert_eq!(
            row_count(&ffx.fx, "autoswitch_state"),
            0,
            "the count stayed 0"
        );
    }
}

#[test]
fn a_proactive_tick_switches_and_records_its_departure() {
    for fake in [false, true] {
        let ffx = FakeFx::new();
        let t = two(&ffx, fake);
        read(&ffx.engine, &t.a, &reading(fake, 10.0, 20.0));
        read(&ffx.engine, &t.b, &reading(fake, 20.0, 95.0));
        let sink = Recorded::default();
        let mut engine = ffx
            .engine
            .auto(&t.provider, t.config, false)
            .unwrap()
            .unwrap();
        let (outcome, decision) = engine.tick(&sink).unwrap();
        assert_eq!(outcome, TickOutcome::Switched, "fake={fake}");
        assert_eq!(
            decision,
            Decision::Switch {
                trigger: Trigger::Proactive,
                targets: vec![t.a.clone()],
                recheck: false,
            }
        );
        let events = sink.take();
        assert_eq!(
            events[1..],
            [AutoEvent::Switch {
                provider: t.provider.clone(),
                trigger: Trigger::Proactive,
                from: 2,
                to: 1,
                warnings: Vec::new(),
                dry_run: false,
            }],
            "fake={fake}"
        );
        assert_eq!(live(&ffx, fake).as_deref(), Some(t.a_email));
        let store = ffx.engine.store().unwrap();
        assert_eq!(
            store.autoswitch_state(&t.provider).unwrap(),
            AutoState {
                last_switch_at: Some(T0),
                last_switch_from: Some(t.b.clone()),
                last_switch_to: Some(t.a.clone()),
                left_headroom: Some(5.0),
                left_recovery_at: Some(LONG_RESETS),
                left_trigger: Some(Trigger::Proactive),
                unhealthy_ticks: 0,
            }
        );
        let last = store.events().unwrap().pop().unwrap();
        assert_eq!(
            (
                last.kind.as_str(),
                last.trigger.as_deref(),
                last.source.as_str()
            ),
            ("switch", Some("proactive"), "auto")
        );
    }
}

/// Review Focus 5: a live login tagteam does not manage is never acted on, and the tick writes
/// nothing, every tick: it stops at step 2, before collecting.
#[test]
fn an_unmanaged_live_login_is_never_acted_on_and_nothing_is_written() {
    for fake in [false, true] {
        let ffx = FakeFx::new();
        let t = two(&ffx, fake);
        if fake {
            ffx.fake_login("stranger", "tok-s", "renew-s");
        } else {
            ffx.fx.login("stranger@x.co", "rt-s");
        }
        let (events, requests) = (row_count(&ffx.fx, "events"), usage_requests(&ffx.fx));
        let keychain = ffx.fx.kc.items();
        let sink = Recorded::default();
        let mut engine = ffx
            .engine
            .auto(&t.provider, t.config, false)
            .unwrap()
            .unwrap();
        for _ in 0..2 {
            let (outcome, decision) = engine.tick(&sink).unwrap();
            assert_eq!(outcome, TickOutcome::NoAction);
            assert!(matches!(
                decision,
                Decision::NoSwitch {
                    reason: NoSwitchReason::UnmanagedActiveAccount,
                    ..
                }
            ));
            assert_eq!(
                sink.take(),
                [no_switch(&t.provider, "unmanaged-active-account", "")]
            );
        }
        assert_eq!(row_count(&ffx.fx, "events"), events);
        assert_eq!(row_count(&ffx.fx, "autoswitch_state"), 0);
        assert_eq!(usage_requests(&ffx.fx), requests);
        assert!(ffx.fx.http.requests().is_empty());
        assert_eq!(ffx.fx.kc.items(), keychain);
        let stranger = if fake { "stranger@ws" } else { "stranger@x.co" };
        assert_eq!(live(&ffx, fake).as_deref(), Some(stranger));
    }
}

#[test]
fn no_live_login_is_no_active_account() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fs::write(fx.paths().global_config, common::CLAUDE_JSON).unwrap();
    fx.kc
        .delete(
            &keychain_service(&fx.env, ItemKind::OAuth),
            &keychain_account(&fx.env),
        )
        .unwrap();
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    assert_eq!(engine.tick(&sink).unwrap().0, TickOutcome::NoAction);
    assert_eq!(
        sink.take(),
        [no_switch(&fx.provider(), "no-active-account", "")]
    );
}

#[test]
fn a_dry_run_tick_writes_nothing_and_says_what_it_would_switch() {
    for fake in [false, true] {
        let ffx = FakeFx::new();
        let t = two(&ffx, fake);
        read(&ffx.engine, &t.a, &reading(fake, 10.0, 20.0));
        read(&ffx.engine, &t.b, &reading(fake, 20.0, 95.0));
        let (events, requests) = (row_count(&ffx.fx, "events"), usage_requests(&ffx.fx));
        let keychain = ffx.fx.kc.items();
        let config = fs::read(ffx.fx.paths().global_config).unwrap();
        let fake_files = FakePaths::resolve(&ffx.fx.env);
        let fake_live = (
            fs::read(&fake_files.identity).ok(),
            fs::read(&fake_files.credential).ok(),
        );
        let sink = Recorded::default();
        let mut engine = ffx
            .engine
            .auto(&t.provider, t.config, true)
            .unwrap()
            .unwrap();
        let (outcome, decision) = engine.tick(&sink).unwrap();
        assert_eq!(outcome, TickOutcome::Switched, "fake={fake}");
        assert!(matches!(
            decision,
            Decision::Switch {
                trigger: Trigger::Proactive,
                ..
            }
        ));
        assert_eq!(
            sink.take()[1..],
            [AutoEvent::Switch {
                provider: t.provider.clone(),
                trigger: Trigger::Proactive,
                from: 2,
                to: 1,
                warnings: Vec::new(),
                dry_run: true,
            }]
        );
        assert_eq!(row_count(&ffx.fx, "events"), events);
        assert_eq!(row_count(&ffx.fx, "autoswitch_state"), 0);
        assert_eq!(usage_requests(&ffx.fx), requests, "nothing was due");
        assert_eq!(ffx.fx.kc.items(), keychain, "the vault and the live items");
        assert_eq!(fs::read(ffx.fx.paths().global_config).unwrap(), config);
        assert_eq!(
            (
                fs::read(&fake_files.identity).ok(),
                fs::read(&fake_files.credential).ok()
            ),
            fake_live
        );
        assert_eq!(live(&ffx, fake).as_deref(), Some(t.b_email));
    }
}

#[test]
fn consume_first_runs_best_on_a_provider_without_a_long_window() {
    // §4.5 and §11.5: FakeAgent names no long window, so a consume-first setting decides as
    // `best` (below the threshold, it stays) and says so once.
    let ffx = FakeFx::new();
    let t = two(&ffx, true);
    read(&ffx.engine, &t.a, &reading(true, 10.0, 10.0));
    read(&ffx.engine, &t.b, &reading(true, 20.0, 50.0));
    let config = AutoConfig {
        strategy: Strategy::ConsumeFirst,
        ..t.config
    };
    assert_eq!(config.long_window, None);
    let sink = Recorded::default();
    let mut engine = ffx
        .engine
        .auto(&t.provider, config, false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::NoAction);
    assert!(matches!(
        decision,
        Decision::NoSwitch {
            reason: NoSwitchReason::BelowThreshold,
            ..
        }
    ));
    let events = sink.take();
    assert_eq!(
        events[1..],
        [
            AutoEvent::ConfigWarning {
                provider: t.provider.clone(),
                message:
                    "FakeAgent has no long usage window to rank by, so consume-first runs best"
                        .into(),
            },
            no_switch(&t.provider, "below-threshold", ""),
        ]
    );
    engine.tick(&sink).unwrap();
    assert_eq!(
        sink.take()[1..],
        [no_switch(&t.provider, "below-threshold", "")]
    );
}

#[test]
fn consume_first_switches_to_the_sooner_long_reset_after_a_re_check() {
    // Decision 2: the re-check finds every reading at most 180 s old, so it fetches nothing,
    // and the re-checked decision switches.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let soon = vec![
        usage_window("5h", WindowKind::Short, 10.0, SHORT_RESETS),
        usage_window("7d", WindowKind::Long, 30.0, T0 + 86_400),
    ];
    read(&fx.engine, &a, &soon);
    read(&fx.engine, &b, &reading(false, 10.0, 50.0));
    let config = AutoConfig {
        strategy: Strategy::ConsumeFirst,
        ..cfg(fx.cc.as_ref())
    };
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), config, false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Switched);
    assert_eq!(
        decision,
        Decision::Switch {
            trigger: Trigger::ConsumeFirst,
            targets: vec![a.clone()],
            recheck: false,
        }
    );
    assert!(matches!(
        sink.take()[1],
        AutoEvent::Switch {
            trigger: Trigger::ConsumeFirst,
            from: 2,
            to: 1,
            ..
        }
    ));
    assert!(fx.http.requests().is_empty());
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
}

#[test]
fn a_re_check_that_cannot_refresh_a_reading_is_stale_usage() {
    // Readings 200 s old are decision-grade, so the first decision is a consume-first switch;
    // the re-check sends for both accounts, gets no reply, and the target is still too old.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let soon = vec![
        usage_window("5h", WindowKind::Short, 10.0, SHORT_RESETS),
        usage_window("7d", WindowKind::Long, 30.0, T0 + 86_400),
    ];
    record_reading(&fx.engine, &a, &soon, T0 - 200, T0 + 100);
    record_reading(
        &fx.engine,
        &b,
        &reading(false, 10.0, 50.0),
        T0 - 200,
        T0 + 100,
    );
    let config = AutoConfig {
        strategy: Strategy::ConsumeFirst,
        ..cfg(fx.cc.as_ref())
    };
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), config, false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::NoAction);
    assert!(matches!(
        decision,
        Decision::NoSwitch {
            reason: NoSwitchReason::StaleUsage,
            ..
        }
    ));
    assert_eq!(
        sink.take().last(),
        Some(&no_switch(&fx.provider(), "stale-usage", ""))
    );
    let mut sent = common::usage_bearers(&fx);
    sent.sort();
    assert_eq!(
        sent,
        ["at-rt-a", "at-rt-b"],
        "one thread per account (§8.3)"
    );
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

/// Decision 2: the re-check skips a reading at most 180 s old when it begins, so the decision
/// after it judges freshness from then. The re-check below takes 10 s on the fixture clock, in
/// which the target's 175 s old reading turns 185 s old.
#[cfg(feature = "test-hooks")]
#[test]
fn a_reading_the_re_check_skipped_stays_fresh_for_the_decision_after_it() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let soon = vec![
        usage_window("5h", WindowKind::Short, 10.0, SHORT_RESETS),
        usage_window("7d", WindowKind::Long, 30.0, T0 + 86_400),
    ];
    record_reading(&fx.engine, &a, &soon, T0 - 175, T0 + 100);
    record_reading(
        &fx.engine,
        &b,
        &reading(false, 10.0, 50.0),
        T0 - 200,
        T0 + 100,
    );
    fx.script_usage(200, common::usage_fixture());
    // Only b is re-fetched, so only b's reservation passes the hook. The pause lets a's
    // thread, which is not re-fetched, check its reading before the clock moves.
    let clock = fx.clock.clone();
    fx.engine.on_point(
        "usage-reserved",
        Box::new(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            clock.advance_ms(10_000);
        }),
    );
    let config = AutoConfig {
        strategy: Strategy::ConsumeFirst,
        ..cfg(fx.cc.as_ref())
    };
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), config, false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(
        common::usage_bearers(&fx),
        ["at-rt-b"],
        "only b was re-fetched"
    );
    assert_eq!(outcome, TickOutcome::Switched, "{decision:?}");
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
}

#[test]
fn a_target_the_re_check_left_stale_is_never_switched_to() {
    // Decision 2: a was read just now, b 200 s ago. The re-check sends for b alone and gets no
    // reply, so b's reading stays stale and b leaves the targets. a's access token needs a
    // refresh that cannot be sent: the tick ends there instead of falling back to b.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c"); // live
    let long_resets = |at| {
        vec![
            usage_window("5h", WindowKind::Short, 10.0, SHORT_RESETS),
            usage_window("7d", WindowKind::Long, 30.0, at),
        ]
    };
    read(&fx.engine, &a, &long_resets(T0 + 86_400));
    record_reading(
        &fx.engine,
        &b,
        &long_resets(T0 + 172_800),
        T0 - 200,
        T0 + 100,
    );
    read(&fx.engine, &c, &reading(false, 10.0, 50.0));
    fx.expire_access(&a);
    let config = AutoConfig {
        strategy: Strategy::ConsumeFirst,
        ..cfg(fx.cc.as_ref())
    };
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), config, false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(
        decision,
        Decision::Switch {
            trigger: Trigger::ConsumeFirst,
            targets: vec![a.clone()],
            recheck: false,
        }
    );
    assert_eq!(outcome, TickOutcome::Error);
    assert_eq!(
        sink.take().last(),
        Some(&AutoEvent::Error {
            provider: fx.provider(),
            message: "could not freshen a@x.co (position 1): pre-send".into(),
            transient: true,
        })
    );
    assert_eq!(common::usage_bearers(&fx), ["at-rt-b"]);
    assert_eq!(fx.live_email().as_deref(), Some("c@x.co"));
}

/// Review Focus 1, the tick half: a manual switch between two ticks. The next tick decides
/// afresh from the account switched to, and never switches back on its own initiative.
#[test]
fn a_manual_switch_between_ticks_is_decided_afresh_from_the_new_live_account() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c"); // live
    for id in [&a, &b, &c] {
        read(&fx.engine, id, &reading(false, 10.0, 50.0));
    }
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    engine.tick(&sink).unwrap();
    assert!(matches!(
        &sink.take()[0],
        AutoEvent::Poll {
            active: Some((3, _)),
            ..
        }
    ));
    assert!(fx.switch_to(&b, false).unwrap().switched);
    let (outcome, _) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::NoAction);
    let events = sink.take();
    assert!(
        matches!(&events[0], AutoEvent::Poll { active: Some((2, email)), .. } if email == "b@x.co"),
        "{events:?}"
    );
    assert_eq!(events[1], no_switch(&fx.provider(), "below-threshold", ""));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

/// Review Focus 1: the tick decided on c, and a manual switch lands while its switch waits
/// before the mutation lock. The tick reports `live-changed` and writes nothing.
#[cfg(feature = "test-hooks")]
#[test]
fn a_manual_switch_during_the_tick_s_perform_is_live_changed() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c"); // live
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &b, &reading(false, 10.0, 50.0));
    read(&fx.engine, &c, &reading(false, 10.0, 95.0));
    let other = fx.engine_with_env(fx.env.clone());
    let manual = fx.switch_request(&b, false);
    fx.engine.on_point(
        "planned",
        Box::new(move || assert!(other.switch(manual.clone()).unwrap().switched)),
    );
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::NoAction);
    assert!(matches!(
        decision,
        Decision::NoSwitch {
            reason: NoSwitchReason::LiveChanged,
            outcome: Outcome::NoAction,
            ..
        }
    ));
    assert_eq!(
        sink.take()[1..],
        [no_switch(&fx.provider(), "live-changed", "")]
    );
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    let switches: Vec<_> = fx
        .engine
        .store()
        .unwrap()
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "switch")
        .collect();
    assert_eq!(switches.len(), 1);
    assert_eq!(switches[0].source, "cli");
    assert_eq!(auto_state(&fx), AutoState::default());
}

#[test]
fn the_unknown_usage_count_starts_afresh_on_an_account_switched_to_by_hand() {
    // Decision 4: the count belongs to the account it judged. Carried over, 2 of c's unknown
    // ticks would fail over from b, just chosen by hand, at b's first unknown tick.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live; nothing is ever read
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    for n in 1..=2 {
        let (_, decision) = engine.tick(&sink).unwrap();
        assert!(
            matches!(&decision, Decision::NoSwitch { reason: NoSwitchReason::ActiveUsageUnknown, detail, .. } if *detail == format!("{n}/3")),
            "{decision:?}"
        );
    }
    assert_eq!(auto_state(&fx).unhealthy_ticks, 2);
    assert!(fx.switch_to(&b, false).unwrap().switched);
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::NoAction);
    assert!(
        matches!(&decision, Decision::NoSwitch { reason: NoSwitchReason::ActiveUsageUnknown, detail, .. } if detail == "1/3"),
        "{decision:?}"
    );
    assert_eq!(auto_state(&fx).unhealthy_ticks, 1);
}

#[test]
fn every_candidate_exhausted_is_blocked_until_the_earliest_recovery() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx.engine, &a, &reading(false, 100.0, 40.0));
    read(&fx.engine, &b, &reading(false, 100.0, 40.0));
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Blocked);
    assert_eq!(
        decision,
        Decision::NoSwitch {
            reason: NoSwitchReason::AllExhausted,
            outcome: Outcome::Blocked,
            detail: "2h40m".into(),
            earliest_reset: Some(SHORT_RESETS),
        }
    );
    assert_eq!(
        sink.take()[1..],
        [
            no_switch(&fx.provider(), "all-exhausted", "2h40m"),
            AutoEvent::AllExhausted {
                provider: fx.provider(),
                earliest_reset_at: Some(SHORT_RESETS),
            },
        ]
    );
}

#[test]
fn a_dead_target_is_quarantined_once_and_the_next_target_is_switched_to() {
    // §11.2 step 10: Dead quarantines the target, `account-quarantined`, next target. The
    // next tick already knows the quarantine and does not report it again.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c"); // live, at its limit
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &b, &reading(false, 10.0, 50.0));
    read(&fx.engine, &c, &reading(false, 100.0, 50.0));
    fx.expire_access(&a);
    fx.script_token_error(400, "invalid_grant");
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Switched);
    assert_eq!(
        decision,
        Decision::Switch {
            trigger: Trigger::AtLimit,
            targets: vec![a.clone(), b.clone()],
            recheck: false,
        }
    );
    assert_eq!(
        sink.take()[1..],
        [
            AutoEvent::AccountQuarantined {
                provider: fx.provider(),
                number: 1,
                email: "a@x.co".into(),
                reason: "invalid_grant".into(),
            },
            AutoEvent::Switch {
                provider: fx.provider(),
                trigger: Trigger::AtLimit,
                from: 3,
                to: 2,
                warnings: Vec::new(),
                dry_run: false,
            },
        ]
    );
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    engine.tick(&sink).unwrap();
    assert_eq!(
        sink.take()[1..],
        [no_switch(&fx.provider(), "below-threshold", "")]
    );
}

#[test]
fn a_target_that_cannot_be_freshened_for_now_is_an_error_at_the_normal_cadence() {
    // §11.2 step 12: every target failed, transiently. No token reply is scripted: the gate's
    // request never leaves (`pre-send`).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let c = fx.add("c@x.co", "rt-c"); // live, at its limit
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &c, &reading(false, 100.0, 50.0));
    fx.expire_access(&a);
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Error);
    assert!(matches!(
        decision,
        Decision::Switch {
            trigger: Trigger::AtLimit,
            ..
        }
    ));
    assert_eq!(
        sink.take()[1..],
        [AutoEvent::Error {
            provider: fx.provider(),
            message: "could not freshen a@x.co (position 1): pre-send".into(),
            transient: true,
        }]
    );
    assert_eq!(fx.live_email().as_deref(), Some("c@x.co"));
}

#[test]
fn a_target_without_a_stored_credential_leaves_no_viable_target() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let c = fx.add("c@x.co", "rt-c"); // live, at its limit
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &c, &reading(false, 100.0, 50.0));
    fx.kc.delete(SERVICE, a.as_str()).unwrap();
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Blocked);
    assert!(matches!(
        decision,
        Decision::NoSwitch {
            reason: NoSwitchReason::NoViableTarget,
            outcome: Outcome::Blocked,
            ..
        }
    ));
    assert_eq!(
        sink.take()[1..],
        [no_switch(&fx.provider(), "no-viable-target", "")]
    );
}

#[test]
fn an_api_key_target_passes_freshening() {
    // §11.2 step 10: API keys do not refresh; with no OAuth target, at-limit falls back to them.
    let fx = Fx::new();
    let c = fx.add("c@x.co", "rt-c"); // live, at its limit
    let k = fx.add_api_key(API_KEY);
    read(&fx.engine, &c, &reading(false, 100.0, 50.0));
    let config = AutoConfig {
        include_api_key_accounts: true,
        ..cfg(fx.cc.as_ref())
    };
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), config, false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Switched);
    assert_eq!(
        decision,
        Decision::Switch {
            trigger: Trigger::AtLimit,
            targets: vec![k.clone()],
            recheck: false,
        }
    );
    assert!(matches!(
        sink.take()[1],
        AutoEvent::Switch {
            trigger: Trigger::AtLimit,
            from: 1,
            to: 2,
            ..
        }
    ));
    assert_eq!(fx.managed_key().as_deref(), Some(API_KEY.as_bytes()));
}

#[test]
fn an_interrupted_switch_recovery_cannot_decide_blocks_the_tick() {
    // §11.2 step 2: CC rotated the target's credential after the crash and no oracle answers.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fx.rotate_live("rt-a-rotated-by-cc");
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Blocked);
    let Decision::NoSwitch {
        reason: NoSwitchReason::InterruptedSwitch,
        detail,
        ..
    } = &decision
    else {
        panic!("{decision:?}");
    };
    assert!(detail.contains("--force"), "{detail}");
    assert_eq!(
        sink.take(),
        [no_switch(&fx.provider(), "interrupted-switch", detail)]
    );
}

#[test]
fn a_tick_recovers_an_interrupted_switch_as_auto_and_goes_on() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &b, &reading(false, 10.0, 20.0));
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    engine.tick(&sink).unwrap();
    let recovered = fx.engine.store().unwrap().events().unwrap().pop().unwrap();
    assert_eq!(
        (recovered.kind.as_str(), recovered.source.as_str()),
        ("switch-recovered", "auto")
    );
    assert!(matches!(
        &sink.take()[0],
        AutoEvent::Poll {
            active: Some((1, _)),
            ..
        }
    ));
}

#[test]
fn quarantines_other_processes_set_or_clear_are_reported_against_the_previous_tick() {
    // Decision 10. The first tick reports only its own releases; later ones report every
    // change since the tick before, `account-replaced` when the login epoch moved.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx.engine, &a, &reading(false, 10.0, 10.0));
    read(&fx.engine, &b, &reading(false, 10.0, 50.0));
    let store = fx.engine.store().unwrap();
    let bound = vault_fp(&fx, &a);
    let quarantined = AutoEvent::AccountQuarantined {
        provider: fx.provider(),
        number: 1,
        email: "a@x.co".into(),
        reason: "invalid_grant".into(),
    };
    let unquarantined = |reason: &str| AutoEvent::AccountUnquarantined {
        provider: fx.provider(),
        number: 1,
        email: "a@x.co".into(),
        reason: reason.into(),
    };
    let sink = Recorded::default();
    // Set before the engine starts: the first tick has nothing to compare it with.
    fx.quarantine(&a, "invalid_grant", &bound);
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    engine.tick(&sink).unwrap();
    assert!(matches!(sink.take()[0], AutoEvent::Poll { .. }));
    store.clear_quarantine(&a).unwrap();
    engine.tick(&sink).unwrap();
    assert_eq!(sink.take()[0], unquarantined("credentials-replaced"));
    fx.quarantine(&a, "invalid_grant", &bound);
    engine.tick(&sink).unwrap();
    assert_eq!(sink.take()[0], quarantined);
    // An explicit replacement (§12.5) bumps the epoch and installs a new login.
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "UPDATE accounts SET login_epoch = login_epoch + 1, quarantine_reason = NULL, \
             quarantine_fp = NULL, quarantine_at = NULL WHERE id = ?1",
            [a.as_str()],
        )
        .unwrap();
    engine.tick(&sink).unwrap();
    assert_eq!(sink.take()[0], unquarantined("account-replaced"));
    // Step 1's own release: the vault has moved past the generation the quarantine names.
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    engine.tick(&sink).unwrap();
    assert_eq!(sink.take()[0], unquarantined("credentials-replaced"));
    let last = store.events().unwrap().pop().unwrap();
    assert_eq!(
        (last.kind.as_str(), last.source.as_str()),
        ("unquarantine", "auto")
    );
}

#[test]
fn a_quarantine_the_collection_set_is_reported_once() {
    // Task 3's report names the accounts quarantined while they were collected; the next
    // tick's comparison knows it already (Decision 10).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a"); // never read: the scheduled pick
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx.engine, &b, &reading(false, 10.0, 50.0));
    fx.expire_access(&a);
    fx.script_token_error(400, "invalid_grant");
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    engine.tick(&sink).unwrap();
    let quarantined = AutoEvent::AccountQuarantined {
        provider: fx.provider(),
        number: 1,
        email: "a@x.co".into(),
        reason: "invalid_grant".into(),
    };
    let events = sink.take();
    assert_eq!(
        events.iter().filter(|e| **e == quarantined).count(),
        1,
        "{events:?}"
    );
    engine.tick(&sink).unwrap();
    assert!(!sink.take().contains(&quarantined));
}

#[test]
fn unknown_model_names_are_warned_about_once_per_setting() {
    // §11.2 step 3: once per engine, and again whenever `models` changes.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let mut fable = reading(false, 10.0, 10.0);
    fable.push(usage_window(
        "scoped:Fable",
        WindowKind::Scoped,
        10.0,
        LONG_RESETS,
    ));
    read(&fx.engine, &a, &fable);
    read(&fx.engine, &b, &reading(false, 10.0, 50.0));
    let warning = |name: &str| AutoEvent::ConfigWarning {
        provider: fx.provider(),
        message: format!(
            "autoswitch.models names {name:?}, but no account's usage reports a window for that model"
        ),
    };
    let config = AutoConfig {
        models: vec!["fable".into(), "Opus".into()],
        ..cfg(fx.cc.as_ref())
    };
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), config.clone(), false)
        .unwrap()
        .unwrap();
    engine.tick(&sink).unwrap();
    assert_eq!(sink.take()[1], warning("Opus"));
    engine.tick(&sink).unwrap();
    assert!(!sink.take().contains(&warning("Opus")));
    engine.set_config(AutoConfig {
        models: vec!["opus".into()],
        ..config.clone()
    });
    engine.tick(&sink).unwrap();
    assert_eq!(sink.take()[1], warning("opus"));
    engine.set_config(AutoConfig {
        models: vec!["all".into()],
        ..config
    });
    engine.tick(&sink).unwrap();
    assert!(
        !sink
            .take()
            .iter()
            .any(|e| matches!(e, AutoEvent::ConfigWarning { .. }))
    );
}

#[test]
fn the_model_check_waits_for_a_reading_to_compare_with() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let config = AutoConfig {
        models: vec!["Opus".into()],
        ..cfg(fx.cc.as_ref())
    };
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), config, false)
        .unwrap()
        .unwrap();
    let warned = |events: &[AutoEvent]| {
        events
            .iter()
            .any(|e| matches!(e, AutoEvent::ConfigWarning { .. }))
    };
    engine.tick(&sink).unwrap();
    assert!(!warned(&sink.take()), "no account has a reading yet");
    fx.clock.advance_ms(3_600_000);
    let now = T0 + 3_600;
    record_reading(&fx.engine, &a, &reading(false, 10.0, 10.0), now, now + 300);
    record_reading(&fx.engine, &b, &reading(false, 10.0, 50.0), now, now + 300);
    engine.tick(&sink).unwrap();
    assert!(warned(&sink.take()));
}

/// What a sink is given, in order: each event, and each tick's end with its provider.
#[derive(Debug, Clone, PartialEq)]
enum Given {
    Event(AutoEvent),
    TickDone(ProviderId),
}

#[derive(Default)]
struct Bounded(Mutex<Vec<Given>>);

impl EventSink for Bounded {
    fn emit(&self, e: &AutoEvent) {
        self.0.lock().unwrap().push(Given::Event(e.clone()));
    }

    fn tick_done(&self, provider: &ProviderId) {
        self.0
            .lock()
            .unwrap()
            .push(Given::TickDone(provider.clone()));
    }
}

impl Bounded {
    fn take(&self) -> Vec<Given> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

#[test]
fn every_tick_ends_at_one_boundary_whatever_it_came_to() {
    // A sink learns where each tick ends, so nothing it keeps for a tick's line outlives the
    // tick: once, last, after an `error` tick, an `Ok` one and an interrupted one.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let c = fx.add("c@x.co", "rt-c"); // live, at its limit
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &c, &reading(false, 100.0, 50.0));
    fx.expire_access(&a);
    let sink = Bounded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let done = Given::TickDone(fx.provider());
    let ends_once = |given: Vec<Given>| {
        assert_eq!(given.iter().filter(|g| **g == done).count(), 1, "{given:?}");
        assert_eq!(given.last(), Some(&done), "{given:?}");
    };
    // a's refresh cannot be sent: an `error` tick.
    assert_eq!(engine.tick(&sink).unwrap().0, TickOutcome::Error);
    ends_once(sink.take());
    // Without a's stored credential nothing is viable: an `Ok` tick, blocked.
    fx.kc.delete(SERVICE, a.as_str()).unwrap();
    assert_eq!(engine.tick(&sink).unwrap().0, TickOutcome::Blocked);
    ends_once(sink.take());
    // A signal before the tick: it stops at its first cancellation point.
    fx.engine.cancel().request(libc::SIGTERM);
    let e = engine.tick(&sink).unwrap_err();
    assert_eq!(e.signal(), Some(libc::SIGTERM));
    assert_eq!(sink.take(), [done.clone()]);
}

/// §14.1: a signal at a collection cancellation point ends the tick with the interruption. The
/// slot reserved for the request that never left is given back, and nothing is recorded.
#[cfg(feature = "test-hooks")]
#[test]
fn an_interruption_during_collection_ends_the_tick_with_nothing_half_written() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live and never read: phase 1 collects it
    let (events, requests) = (row_count(&fx, "events"), usage_requests(&fx));
    let cancel = fx.engine.cancel().clone();
    fx.engine.on_point(
        "usage-reserved",
        Box::new(move || cancel.request(libc::SIGINT)),
    );
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let e = engine.tick(&sink).unwrap_err();
    assert_eq!(e.signal(), Some(libc::SIGINT));
    assert!(sink.take().is_empty());
    assert_eq!(usage_requests(&fx), requests, "the unsent slot went back");
    assert_eq!(fx.usage_state(&b).and_then(|s| s.fetched_at), None);
    assert!(fx.http.requests().is_empty());
    assert_eq!(row_count(&fx, "events"), events);
    assert_eq!(row_count(&fx, "autoswitch_state"), 0);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

/// §11.2 step 11: a target that stopped being a candidate while the switch waited is passed
/// over for the next one. Another process disables a before the switch takes its locks.
#[cfg(feature = "test-hooks")]
#[test]
fn a_target_that_stops_being_a_candidate_moves_the_tick_to_the_next() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c"); // live
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &b, &reading(false, 10.0, 50.0));
    read(&fx.engine, &c, &reading(false, 10.0, 95.0));
    let other = fx.engine_with_env(fx.env.clone());
    let disabled = a.clone();
    fx.engine.on_point(
        "planned",
        Box::new(move || drop(other.set_disabled(&disabled, true).unwrap())),
    );
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Switched);
    assert_eq!(
        decision,
        Decision::Switch {
            trigger: Trigger::Proactive,
            targets: vec![a, b],
            recheck: false,
        }
    );
    assert!(matches!(
        sink.take()[1],
        AutoEvent::Switch { from: 3, to: 2, .. }
    ));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

/// §11.2 step 11: the cooldown is judged again under the lock, from the state as stored then;
/// here a switch is recorded while the tick's own switch waits.
#[cfg(feature = "test-hooks")]
#[test]
fn a_cooldown_found_under_the_lock_is_no_switch_cooldown() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &b, &reading(false, 10.0, 95.0));
    let db = fx.env.data_dir().join("tagteam.db");
    fx.engine.on_point(
        "planned",
        Box::new(move || {
            rusqlite::Connection::open(&db)
                .unwrap()
                .execute(
                    "INSERT INTO autoswitch_state (provider, last_switch_at) VALUES ('claude-code', ?1)",
                    [T0 - 60],
                )
                .unwrap();
        }),
    );
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::NoAction);
    assert_eq!(
        decision,
        Decision::NoSwitch {
            reason: NoSwitchReason::Cooldown,
            outcome: Outcome::NoAction,
            detail: "4m".into(),
            earliest_reset: None,
        }
    );
    assert_eq!(
        sink.take()[1..],
        [no_switch(&fx.provider(), "cooldown", "4m")]
    );
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

/// M3a's rule (§14.1): a signal inside the switch's critical span does not stop it. The tick
/// reports the switch it made, and the token stays set for the caller's next check.
#[cfg(feature = "test-hooks")]
#[test]
fn a_switch_that_commits_despite_a_late_signal_is_switched() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &b, &reading(false, 10.0, 95.0));
    let cancel = fx.engine.cancel().clone();
    fx.engine.on_point(
        "after-journal",
        Box::new(move || cancel.request(libc::SIGTERM)),
    );
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, _) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Switched);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(auto_state(&fx).last_switch_to, Some(a));
    assert_eq!(
        engine.tick(&sink).unwrap_err().signal(),
        Some(libc::SIGTERM)
    );
}
