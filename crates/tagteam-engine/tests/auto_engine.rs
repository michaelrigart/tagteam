//! §11.1's engine: one per provider per machine, held by a try-only lock that records its
//! holder; dry-run holds no lock and writes no auto-switch state; the live account's next
//! fetchable time for the loop.
mod common;

use std::fs;

use common::{FakeFx, Fx, Recorded, record_reading, usage_window};
use serde_json::Value;
use tagteam_core::autoswitch::{AutoConfig, AutoState, Decision, NoSwitchReason, Strategy};
use tagteam_core::{ProviderId, WindowKind};
use tagteam_engine::EngineError;
use tagteam_engine::auto::{AutoEvent, TickOutcome};
use tagteam_provider::{ProcessStamp, Provider};

const T0: i64 = 1_790_000_000;

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

fn lock_path(fx: &Fx, provider: &ProviderId) -> std::path::PathBuf {
    fx.env
        .data_dir()
        .join("locks")
        .join(format!("autoswitch-{provider}.lock"))
}

/// Review Focus 3, the engine half: a second engine for one provider, in this process or
/// another, gets `None` while the first lives, and the lock once it is dropped. Another
/// provider's engine is independent.
#[test]
fn a_second_engine_for_one_provider_gets_none_while_the_first_lives() {
    let ffx = FakeFx::new();
    let cc = ffx.fx.provider();
    let config = cfg(ffx.fx.cc.as_ref());
    let first = ffx.engine.auto(&cc, config.clone(), false).unwrap();
    assert!(first.is_some());
    assert!(
        ffx.engine
            .auto(&cc, config.clone(), false)
            .unwrap()
            .is_none()
    );
    let elsewhere = ffx.fx.engine_with_env(ffx.fx.env.clone());
    assert!(
        elsewhere
            .auto(&cc, config.clone(), false)
            .unwrap()
            .is_none()
    );
    let fake = ffx.fake_provider();
    let fake_engine = ffx
        .engine
        .auto(&fake, cfg(ffx.fake.as_ref()), false)
        .unwrap();
    assert!(fake_engine.is_some(), "another provider has its own lock");
    drop(first);
    assert!(elsewhere.auto(&cc, config, false).unwrap().is_some());
}

#[test]
fn the_engine_lock_records_its_holder_as_one_json_line() {
    // M5's amendment (`86882e3`): pid and start time, the start taken as for the journal's
    // holder (§12.6). A longer record left by a dead engine is replaced whole.
    let fx = Fx::new();
    let path = lock_path(&fx, &fx.provider());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "{\"pid\":4294967295,\"start\":18446744073709551615,\"note\":\"a dead engine's\"}\n",
    )
    .unwrap();
    let engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let me = ProcessStamp::current().unwrap();
    let bytes = fs::read(&path).unwrap();
    assert_eq!(
        String::from_utf8(bytes.clone()).unwrap(),
        format!("{{\"pid\":{},\"start\":{}}}\n", me.pid, me.start)
    );
    let record: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(record["pid"].as_u64(), Some(u64::from(me.pid)));
    assert_eq!(record["start"].as_u64(), Some(me.start));
    drop(engine);
}

#[test]
fn a_real_engine_refuses_inside_a_run_shell_and_a_dry_run_does_not() {
    // §11.1: like every command that changes the live login (§9.2).
    let fx = Fx::new();
    let mut env = fx.env.clone();
    env.claude_config_dir = Some(fx.env.data_dir().join("sessions/x").into_os_string());
    let engine = fx.engine_with_env(env);
    assert!(matches!(
        engine.auto(&fx.provider(), cfg(fx.cc.as_ref()), false),
        Err(EngineError::InsideRunShell)
    ));
    assert!(!lock_path(&fx, &fx.provider()).exists());
    let dry = engine.auto(&fx.provider(), cfg(fx.cc.as_ref()), true);
    assert!(dry.unwrap().is_some());
}

#[test]
fn dry_run_takes_no_lock_and_keeps_its_state_in_memory() {
    // §11.1: no engine lock, no auto-switch state, no quarantine released. Its count of
    // unknown ticks lives in memory.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live, never read
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    let mut dry = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), true)
        .unwrap()
        .unwrap();
    assert!(!lock_path(&fx, &fx.provider()).exists());
    let real = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap();
    assert!(
        real.is_some(),
        "a dry run holds nothing a real engine needs"
    );
    drop(real);
    let sink = Recorded::default();
    for n in 1..=2 {
        let (outcome, decision) = dry.tick(&sink).unwrap();
        assert_eq!(outcome, TickOutcome::NoAction);
        assert!(
            matches!(
                &decision,
                Decision::NoSwitch { reason: NoSwitchReason::ActiveUsageUnknown, detail, .. }
                    if *detail == format!("{n}/3")
            ),
            "{decision:?}"
        );
    }
    let store = fx.engine.store().unwrap();
    assert_eq!(
        store.autoswitch_state(&fx.provider()).unwrap(),
        AutoState::default()
    );
    let row = store.account(&a).unwrap().unwrap();
    assert_eq!(row.quarantine_reason.as_deref(), Some("invalid_grant"));
    assert!(
        !sink
            .take()
            .iter()
            .any(|e| matches!(e, AutoEvent::AccountUnquarantined { .. }))
    );
}

#[test]
fn the_next_poll_is_the_later_of_the_plan_and_the_usage_lease() {
    // Task 3's controller ruling: a §8.3 lease outlives its record, and a tick that wakes
    // inside it cannot fetch. Each reading below leaves its 90 s lease, to T0 + 90.
    let reading = |pct| {
        vec![
            usage_window("5h", WindowKind::Short, pct, T0 + 9_630),
            usage_window("7d", WindowKind::Long, pct, T0 + 291_630),
        ]
    };
    // The urgent band's 60 s plan ends inside the lease; a 300 s one outlasts it.
    for (planned, next) in [(T0 + 60, T0 + 90), (T0 + 300, T0 + 300)] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b"); // live
        record_reading(&fx.engine, &a, &reading(10.0), T0, T0 + 300);
        record_reading(&fx.engine, &b, &reading(20.0), T0, planned);
        let mut engine = fx
            .engine
            .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
            .unwrap()
            .unwrap();
        assert_eq!(
            engine.active_next_poll_at(),
            None,
            "no tick has seen the live account yet"
        );
        engine.tick(&Recorded::default()).unwrap();
        assert_eq!(
            engine.active_next_poll_at(),
            Some(next),
            "planned {planned}"
        );
    }
}

#[test]
fn the_next_poll_waits_out_the_live_accounts_backoff() {
    // A failed fetch leaves the plan in the past but backs the account off: a tick that wakes
    // inside the backoff cannot fetch (§8.3). A backoff no legal schedule reaches is ignored
    // (§8.4).
    let reading = |pct| {
        vec![
            usage_window("5h", WindowKind::Short, pct, T0 + 9_630),
            usage_window("7d", WindowKind::Long, pct, T0 + 291_630),
        ]
    };
    for (backoff, next) in [
        (T0 + 3_600, T0 + 3_600),
        (T0 + 30, T0 + 90),
        (T0 + 1_000_000, T0 + 90),
    ] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b"); // live
        record_reading(&fx.engine, &a, &reading(10.0), T0, T0 + 300);
        record_reading(&fx.engine, &b, &reading(20.0), T0, T0 + 60);
        let mut engine = fx
            .engine
            .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
            .unwrap()
            .unwrap();
        engine.tick(&Recorded::default()).unwrap();
        rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
            .unwrap()
            .execute(
                "UPDATE usage_state SET backoff_until = ?2 WHERE account_id = ?1",
                rusqlite::params![b.as_str(), backoff],
            )
            .unwrap();
        assert_eq!(
            engine.active_next_poll_at(),
            Some(next),
            "backoff until {backoff}"
        );
    }
}
