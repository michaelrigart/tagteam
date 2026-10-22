//! §11.1's engine: one per provider per machine, held by a try-only lock that records its
//! holder; dry-run holds no lock and writes no auto-switch state; the live account's next
//! fetchable time for the loop. The daemon's hooks (daemon plan A, Decisions 4 to 6): the
//! source an engine records, the record it empties when dropped, and the quarantine watch its
//! ticks share with the daemon's collect rounds.
mod common;

use std::fs;

use common::{
    FakeFx, Fx, Recorded, crashed_switch, record_reading, usage_window, write_target_credential,
};
use serde_json::{Value, json};
use tagteam_core::autoswitch::{AutoConfig, AutoState, AutoStrategy, Decision, NoSwitchReason};
use tagteam_core::{AccountId, ProviderId, Window, WindowKind};
use tagteam_engine::auto::{AutoEvent, QuarantineWatch, TickOutcome};
use tagteam_engine::store::AccountRow;
use tagteam_engine::{EngineError, EventKind, EventSource};
use tagteam_provider::{FlockGuard, ProcessStamp, Provider, Read};

const T0: i64 = 1_790_000_000;

fn cfg(p: &dyn Provider) -> AutoConfig {
    AutoConfig {
        threshold: 90.0,
        hysteresis_pct: 10.0,
        cooldown_s: 300,
        interval_s: 60,
        unhealthy_ticks: 3,
        strategy: AutoStrategy::Best,
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
    let a = fx.add("a@x.co", "rt-a");
    let engine = fx.engine_located(fx.shell_env(&fx.make_profile(&a)));
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

/// Claude Code's 5h and 7d windows at `short` and `long` percent, as every reading here has
/// them.
fn windows(short: f64, long: f64) -> Vec<Window> {
    vec![
        usage_window("5h", WindowKind::Short, short, T0 + 9_630),
        usage_window("7d", WindowKind::Long, long, T0 + 291_630),
    ]
}

/// A reading taken now with its next poll five minutes out: decision-grade, and not due.
fn read(fx: &Fx, id: &AccountId, w: &[Window]) {
    record_reading(&fx.engine, id, w, T0, T0 + 300);
}

/// The last event the store recorded, as `(kind, source)`.
fn last_event(fx: &Fx) -> (String, String) {
    let last = fx.engine.store().unwrap().events().unwrap().pop().unwrap();
    (
        last.kind.as_str().to_owned(),
        last.source.as_str().to_owned(),
    )
}

#[test]
fn a_daemon_engine_records_its_switches_as_the_daemon_s() {
    // Decision 4: `events.source` reserves `daemon` (§6.1); the engine's source is the daemon's.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx, &a, &windows(10.0, 20.0));
    read(&fx, &b, &windows(20.0, 95.0));
    let daemon = fx.engine_as(EventSource::Daemon);
    let mut engine = daemon
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    assert_eq!(engine.provider(), &fx.provider());
    let (outcome, _) = engine.tick(&Recorded::default()).unwrap();
    assert_eq!(outcome, TickOutcome::Switched);
    assert_eq!(last_event(&fx), ("switch".into(), "daemon".into()));
}

#[test]
fn a_daemon_engine_recovers_an_interrupted_switch_as_the_daemon() {
    // Decision 4: §11.2 step 2's recovery records the engine's source too (§9.6).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    read(&fx, &a, &windows(10.0, 20.0));
    read(&fx, &b, &windows(10.0, 20.0));
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    let daemon = fx.engine_as(EventSource::Daemon);
    let mut engine = daemon
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    engine.tick(&Recorded::default()).unwrap();
    assert_eq!(
        last_event(&fx),
        ("switch-recovered".into(), "daemon".into())
    );
}

#[test]
fn a_daemon_engine_releases_an_unbound_quarantine_as_the_daemon() {
    // Decision 4: step 1's release records the engine's source (§7.4). The vault has moved
    // past the generation the quarantine names, so it no longer binds.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx, &a, &windows(10.0, 10.0));
    read(&fx, &b, &windows(10.0, 50.0));
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    let daemon = fx.engine_as(EventSource::Daemon);
    let mut engine = daemon
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    engine.tick(&Recorded::default()).unwrap();
    assert_eq!(last_event(&fx), ("unquarantine".into(), "daemon".into()));
}

/// The provider's stored pauses and resumes, oldest first, as `(kind, source, detail)`.
fn pauses(fx: &Fx) -> Vec<(EventKind, EventSource, Option<Value>)> {
    fx.engine
        .store()
        .unwrap()
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| {
            matches!(
                e.kind,
                EventKind::AutoswitchPaused | EventKind::AutoswitchResumed
            )
        })
        .inspect(|e| assert_eq!((&e.from_id, &e.to_id), (&None, &None)))
        .map(|e| (e.kind, e.source, e.detail))
        .collect()
}

#[test]
fn a_pause_is_recorded_at_start_only_when_it_differs_from_the_store() {
    // Redesign §12.4, T4-25: at start, only a setting the provider's latest stored pause or
    // resume does not already say, none reading as resumed; a reload's change always. Never
    // without a store.
    let fx = Fx::new();
    let cc = fx.provider();
    let daemon = fx.engine_as(EventSource::Daemon);
    assert!(!daemon.record_autoswitch(&cc, false, true).unwrap());
    assert!(!daemon.record_autoswitch(&cc, false, false).unwrap());
    assert!(
        !fx.env.data_dir().join("tagteam.db").exists(),
        "no store is created"
    );
    fx.engine.store().unwrap();
    let at_start = Some(json!({"at_start": true}));
    assert!(daemon.record_autoswitch(&cc, false, true).unwrap());
    assert_eq!(
        pauses(&fx),
        [(
            EventKind::AutoswitchPaused,
            EventSource::Daemon,
            at_start.clone()
        )]
    );
    assert!(!daemon.record_autoswitch(&cc, false, true).unwrap());
    assert_eq!(pauses(&fx).len(), 1);
    assert!(daemon.record_autoswitch(&cc, true, true).unwrap());
    assert!(daemon.record_autoswitch(&cc, true, false).unwrap());
    assert_eq!(
        pauses(&fx)[1..],
        [
            (EventKind::AutoswitchResumed, EventSource::Daemon, at_start),
            (EventKind::AutoswitchResumed, EventSource::Daemon, None),
        ]
    );
}

#[test]
fn a_dropped_engine_leaves_its_lock_file_empty_and_free() {
    // Decision 5, D3 change 3: a daemon that lets a provider go lives on, so a record left
    // behind would name a live process as the provider's engine.
    let fx = Fx::new();
    let provider = fx.provider();
    let path = lock_path(&fx, &provider);
    assert!(matches!(fx.engine.engine_holder(&provider), Read::Absent));
    let engine = fx
        .engine
        .auto(&provider, cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let me = ProcessStamp::current().unwrap();
    assert!(matches!(fx.engine.engine_holder(&provider), Read::Present(s) if s == me));
    drop(engine);
    assert_eq!(
        fs::read(&path).unwrap(),
        b"",
        "the lock file stays, emptied"
    );
    assert!(matches!(fx.engine.engine_holder(&provider), Read::Absent));
    assert!(
        FlockGuard::try_lock(&path).unwrap().is_some(),
        "and the lock is free"
    );
}

#[test]
fn a_dry_run_engine_s_drop_touches_no_lock_file() {
    // Decision 5: a dry run holds no lock (§11.1), so the record belongs to someone else.
    let fx = Fx::new();
    let provider = fx.provider();
    let path = lock_path(&fx, &provider);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let record = "{\"pid\":4242,\"start\":1}\n";
    fs::write(&path, record).unwrap();
    let dry = fx
        .engine
        .auto(&provider, cfg(fx.cc.as_ref()), true)
        .unwrap()
        .unwrap();
    drop(dry);
    assert_eq!(fs::read_to_string(&path).unwrap(), record);
    assert!(matches!(
        fx.engine.engine_holder(&provider),
        Read::Present(ProcessStamp {
            pid: 4242,
            start: 1
        })
    ));
    fs::write(&path, "garbage").unwrap();
    assert!(matches!(
        fx.engine.engine_holder(&provider),
        Read::Unreadable(_)
    ));
}

/// A Claude Code account row at `position`, quarantined for `reason` when one is given.
fn row(id: &str, position: u32, reason: Option<&str>, login_epoch: i64) -> AccountRow {
    let email = format!("{id}@x.co");
    AccountRow {
        id: AccountId::from_string(id),
        provider: ProviderId::new("claude-code"),
        position,
        identity_key: format!("{email}\n"),
        label: email.clone(),
        email: Some(email),
        org_uuid: String::new(),
        org_name: None,
        account_uuid: None,
        kind: "oauth".into(),
        alias: None,
        disabled: false,
        identity_json: json!({}),
        login_expires_at: None,
        login_epoch,
        replacing_fp: None,
        quarantine_reason: reason.map(str::to_owned),
        quarantine_fp: reason.map(|_| "sha256:q".to_owned()),
        quarantine_at: reason.map(|_| 1),
        added_at: 1,
    }
}

fn quarantined(p: &ProviderId, number: u32, email: &str, reason: &str) -> AutoEvent {
    AutoEvent::AccountQuarantined {
        provider: p.clone(),
        number,
        email: email.into(),
        reason: reason.into(),
    }
}

fn unquarantined(p: &ProviderId, number: u32, email: &str, reason: &str) -> AutoEvent {
    AutoEvent::AccountUnquarantined {
        provider: p.clone(),
        number,
        email: email.into(),
        reason: reason.into(),
    }
}

#[test]
fn a_quarantine_watch_reports_only_releases_on_its_first_call() {
    // Decision 6, as the tick's step 1 did (Decision 10): with no previous call to compare
    // with, a quarantine that is already set is not news.
    let p = ProviderId::new("claude-code");
    let sink = Recorded::default();
    let mut watch = QuarantineWatch::new();
    let rows = [row("a", 1, Some("invalid_grant"), 0), row("b", 2, None, 0)];
    watch.report(&p, &rows, &[AccountId::from_string("b")], &sink);
    assert_eq!(
        sink.take(),
        [unquarantined(&p, 2, "b@x.co", "credentials-replaced")]
    );
}

#[test]
fn a_quarantine_watch_reports_what_was_set_or_cleared_since_its_previous_call() {
    let p = ProviderId::new("claude-code");
    let sink = Recorded::default();
    let mut watch = QuarantineWatch::new();
    watch.report(
        &p,
        &[row("a", 1, None, 0), row("b", 2, Some("invalid_grant"), 0)],
        &[],
        &sink,
    );
    assert!(sink.take().is_empty());
    // Set on a, cleared on b, and c added since, already quarantined.
    watch.report(
        &p,
        &[
            row("a", 1, Some("invalid_grant"), 0),
            row("b", 2, None, 0),
            row("c", 3, Some("invalid_grant"), 0),
        ],
        &[],
        &sink,
    );
    assert_eq!(
        sink.take(),
        [
            quarantined(&p, 1, "a@x.co", "invalid_grant"),
            unquarantined(&p, 2, "b@x.co", "credentials-replaced"),
            quarantined(&p, 3, "c@x.co", "invalid_grant"),
        ]
    );
    // An explicit replacement (§12.5) moved a's login epoch as it cleared the quarantine.
    let now = [
        row("a", 1, None, 1),
        row("b", 2, None, 0),
        row("c", 3, Some("invalid_grant"), 0),
    ];
    watch.report(&p, &now, &[], &sink);
    assert_eq!(
        sink.take(),
        [unquarantined(&p, 1, "a@x.co", "account-replaced")]
    );
    watch.report(&p, &now, &[], &sink);
    assert!(sink.take().is_empty(), "nothing changed since");
}

#[test]
fn a_quarantine_a_round_found_itself_is_reported_once() {
    // Decision 6: a collection's or a freshening's quarantine is news once; the next
    // comparison already knows it.
    let p = ProviderId::new("claude-code");
    let sink = Recorded::default();
    let mut watch = QuarantineWatch::new();
    let a = row("a", 1, Some("invalid_grant"), 0);
    watch.note_quarantined(&p, &a, &sink);
    watch.note_quarantined(&p, &a, &sink);
    assert_eq!(sink.take(), [quarantined(&p, 1, "a@x.co", "invalid_grant")]);
    watch.report(&p, std::slice::from_ref(&a), &[], &sink);
    assert!(sink.take().is_empty());
    watch.note_quarantined(&p, &row("b", 2, None, 0), &sink);
    assert!(
        sink.take().is_empty(),
        "an account with no quarantine is no news"
    );
}
