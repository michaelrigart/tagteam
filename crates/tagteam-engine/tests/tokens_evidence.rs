//! Spec §6: the evidence that places a session in a home: sightings from the scan, from `run`'s
//! poll and from the statusline, and `run`'s launches. Session records are judged by the
//! fixture's `FakeProcessProbe`, never by a real pid, and times come from the fixture's clock.
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::{Fx, LSTART};
use serde_json::json;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::store::{LaunchRow, SightingRow, SightingSource};
use tagteam_engine::tokens::{LaunchId, LaunchPoll, Tenure};
use tagteam_provider::{
    FakeProcess, FlockGuard, LAUNCH_DIR, PENDING_LAUNCH_DIR, PendingLaunch, parse_lstart,
};

const NOW: i64 = 1_790_000_000_000;

/// `pid` runs, started at `LSTART`, under `ppid`.
fn running(fx: &Fx, pid: u32, ppid: Option<u32>) {
    fx.process.set(
        pid,
        FakeProcess {
            exists: Some(true),
            start_time_s: parse_lstart(LSTART),
            ppid,
            ..FakeProcess::default()
        },
    );
}

/// `pid` has exited.
fn gone(fx: &Fx, pid: u32) {
    fx.process.set(
        pid,
        FakeProcess {
            exists: Some(false),
            ..FakeProcess::default()
        },
    );
}

/// `pid`'s session record of `kind` in `home` (the default home's config directory, or a
/// profile), naming `session`, started at `started_at`.
fn record(fx: &Fx, home: &Path, pid: u32, session: &str, started_at: i64, kind: &str) -> PathBuf {
    let record = json!({
        "pid": pid,
        "procStart": LSTART,
        "startedAt": started_at,
        "kind": kind,
        "sessionId": session
    });
    fx.plant_record(home, &pid.to_string(), record.to_string().as_bytes())
}

fn sighting(
    session: &str,
    home: &str,
    started_at: i64,
    first_at: i64,
    last_at: i64,
    source: SightingSource,
    recorded_at: i64,
) -> SightingRow {
    SightingRow {
        session_id: session.into(),
        home: home.into(),
        started_at,
        first_at,
        last_at,
        source,
        recorded_at,
    }
}

/// Claude Code's sightings, by home, session and start.
fn sightings(fx: &Fx) -> Vec<SightingRow> {
    let mut all = fx
        .engine
        .store()
        .unwrap()
        .sightings(&fx.provider())
        .unwrap();
    all.sort_by(|a, b| {
        (&a.home, &a.session_id, a.first_at).cmp(&(&b.home, &b.session_id, b.first_at))
    });
    all
}

/// Claude Code's launches, by account and start.
fn launches(fx: &Fx) -> Vec<LaunchRow> {
    let mut all = fx.engine.store().unwrap().launches(&fx.provider()).unwrap();
    all.sort_by(|a, b| (&a.account_id, a.started_at).cmp(&(&b.account_id, b.started_at)));
    all
}

/// When each launch ended, by account and start.
fn ended(fx: &Fx) -> Vec<(String, Option<i64>)> {
    launches(fx)
        .into_iter()
        .map(|l| (l.account_id, l.ended_at))
        .collect()
}

fn sight(fx: &Fx) -> u64 {
    fx.engine.sight_sessions(&fx.provider()).unwrap()
}

/// Sets `path`'s mtime to `secs` after the epoch.
fn set_mtime(path: &Path, secs: u64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap();
}

/// One tick of `run`'s poller that is also a 2 s write, for the child 777: the check of its
/// record, then the store write (spec §6.1).
fn poll(fx: &Fx, id: &LaunchId, profile: &Path, state: &mut LaunchPoll) {
    check(fx, id, profile, state);
    fx.engine.poll_launch(id, state).unwrap();
}

/// One 50 ms check of the child 777's record, with no store write (spec §6.1).
fn check(fx: &Fx, id: &LaunchId, profile: &Path, state: &mut LaunchPoll) {
    fx.engine.observe_launch(id, profile, 777, state).unwrap();
}

#[test]
fn a_scan_sights_live_records_in_every_home() {
    // Spec §6.2: every live record in the default home and in each profile, its execution keyed
    // by its `startedAt`, sighted from the scan's read and extended at most once a minute (spec
    // §6.4).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    running(&fx, 101, None);
    record(
        &fx,
        &fx.paths().config_home,
        101,
        "s-default",
        NOW - 5_000,
        "interactive",
    );
    running(&fx, 202, None);
    record(&fx, &profile, 202, "s-profile", NOW - 9_000, "interactive");

    assert_eq!(sight(&fx), 2);
    let scan = SightingSource::Scan;
    assert_eq!(
        sightings(&fx),
        [
            sighting("s-default", "", NOW - 5_000, NOW, NOW, scan, NOW),
            sighting("s-profile", a.as_str(), NOW - 9_000, NOW, NOW, scan, NOW),
        ]
    );

    fx.clock.advance_ms(30_000);
    assert_eq!(sight(&fx), 0, "within the minute");
    fx.clock.advance_ms(31_000);
    assert_eq!(sight(&fx), 2);
    let later = NOW + 61_000;
    assert_eq!(
        sightings(&fx),
        [
            sighting("s-default", "", NOW - 5_000, NOW, later, scan, later),
            sighting(
                "s-profile",
                a.as_str(),
                NOW - 9_000,
                NOW,
                later,
                scan,
                later
            ),
        ]
    );
}

#[test]
fn a_scan_starts_a_tenure_at_its_read_and_continues_the_latest() {
    // Spec §6.2, §6.4: a record's `startedAt` is its execution, not when its session entered it.
    // A first scan of an execution an hour old sights the session from the scan on; later scans
    // continue that tenure. A session the record names after `/clear` starts its own tenure at
    // the scan that reads it, and a return to the first one (`/resume`), once the execution was
    // seen naming another, starts a new tenure rather than bridging the gap.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let home = fx.paths().config_home;
    let started = NOW - 3_600_000;
    running(&fx, 101, None);
    record(&fx, &home, 101, "s-1", started, "interactive");
    let scan = SightingSource::Scan;
    let seen = |session: &str, first: i64, last: i64| {
        sighting(session, "", started, first, last, scan, last)
    };

    assert_eq!(sight(&fx), 1);
    assert_eq!(sightings(&fx), [seen("s-1", NOW, NOW)]);
    fx.clock.advance_ms(61_000);
    assert_eq!(sight(&fx), 1);
    assert_eq!(sightings(&fx), [seen("s-1", NOW, NOW + 61_000)]);

    record(&fx, &home, 101, "s-2", started, "interactive");
    fx.clock.advance_ms(10_000);
    assert_eq!(sight(&fx), 1);
    let cleared = NOW + 71_000;
    assert_eq!(
        sightings(&fx),
        [
            seen("s-1", NOW, NOW + 61_000),
            seen("s-2", cleared, cleared)
        ]
    );

    record(&fx, &home, 101, "s-1", started, "interactive");
    fx.clock.advance_ms(60_000);
    assert_eq!(sight(&fx), 1);
    let back = NOW + 131_000;
    assert_eq!(
        sightings(&fx),
        [
            seen("s-1", NOW, NOW + 61_000),
            seen("s-1", back, back),
            seen("s-2", cleared, cleared)
        ]
    );
}

#[test]
#[cfg(feature = "test-hooks")]
fn a_scan_dates_each_sighting_at_its_own_read() {
    // Spec §6.2, §6.4: a sighting begins and ends at reads. The scan dates each record at its
    // own read: not at the end of the home's, nor after the liveness check and the writes that
    // follow, which take time in which the session may leave. Here 5 s pass right after each
    // record's read: 101's is read first, then 202's.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let home = fx.paths().config_home;
    running(&fx, 101, None);
    record(&fx, &home, 101, "s-1", NOW - 60_000, "interactive");
    running(&fx, 202, None);
    record(&fx, &home, 202, "s-2", NOW - 60_000, "interactive");
    let clock = fx.clock.clone();
    fx.engine.on_point(
        "scan-record-read",
        Box::new(move || clock.advance_ms(5_000)),
    );

    assert_eq!(sight(&fx), 2);
    let dated: Vec<(String, i64, i64)> = sightings(&fx)
        .into_iter()
        .map(|s| (s.session_id, s.first_at, s.last_at))
        .collect();
    assert_eq!(
        dated,
        [
            ("s-1".to_owned(), NOW, NOW),
            ("s-2".to_owned(), NOW + 5_000, NOW + 5_000)
        ]
    );
}

#[test]
fn a_scan_skips_dead_and_unreadable_records() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let home = fx.paths().config_home;
    // A record whose process is gone.
    record(&fx, &home, 303, "s-dead", NOW, "interactive");
    // One caught part-way through its write.
    fx.plant_record(&home, "404", b"{\"pid\": 404, \"sessionId\"");
    // One without a `sessionId`, and one without a `startedAt`.
    running(&fx, 505, None);
    let no_session = json!({"pid": 505, "procStart": LSTART, "startedAt": NOW});
    fx.plant_record(&home, "505", no_session.to_string().as_bytes());
    running(&fx, 606, None);
    let no_start = json!({"pid": 606, "procStart": LSTART, "sessionId": "s-6"});
    fx.plant_record(&home, "606", no_start.to_string().as_bytes());

    assert_eq!(sight(&fx), 0);
    assert!(sightings(&fx).is_empty());
}

#[test]
fn a_scan_closes_launches_of_idle_profiles_only() {
    // Spec §6.1: a launch is closed, at the moment it is found so, once its profile is no
    // longer in use; that covers a `run` killed while its session lived on.
    let fx = Fx::new();
    let cc = fx.provider();
    let (a, b, c) = (
        fx.add("a@x.co", "rt-a"),
        fx.add("b@x.co", "rt-b"),
        fx.add("c@x.co", "rt-c"),
    );
    let pa = fx.make_profile(&a);
    fx.make_profile(&b);
    for id in [&a, &b, &c] {
        fx.engine.open_launch(&cc, id).unwrap();
    }
    // a's session still runs; b's profile is idle; c has no profile left.
    running(&fx, 101, None);
    record(&fx, &pa, 101, "s-a", NOW, "interactive");
    fx.clock.advance_ms(10_000);

    assert_eq!(
        sight(&fx),
        3,
        "a's sighting, and b's and c's launches closed"
    );
    let mut want = vec![
        (a.to_string(), None),
        (b.to_string(), Some(NOW + 10_000)),
        (c.to_string(), Some(NOW + 10_000)),
    ];
    want.sort();
    assert_eq!(ended(&fx), want);

    gone(&fx, 101);
    fx.clock.advance_ms(1_000);
    assert_eq!(sight(&fx), 1, "a's launch closes once its session is gone");
    assert!(ended(&fx).iter().all(|(_, end)| end.is_some()));
}

#[test]
#[cfg(feature = "test-hooks")]
fn a_scan_closes_a_launch_at_its_idle_check_not_at_its_start() {
    // Spec §6.1: a launch closes at the moment its profile is found idle. A child of a killed
    // `run` that answers once more after the scan began, then exits before the scan checks its
    // profile, keeps that response inside its launch: the launch ends at the check.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    fx.engine.open_launch(&cc, &a).unwrap();
    running(&fx, 101, None);
    record(&fx, &profile, 101, "s-a", NOW, "interactive");
    // Between the scan's sightings and the check of the launch's profile, 5 s pass, and the
    // child answers and exits.
    let (clock, process) = (fx.clock.clone(), fx.process.clone());
    fx.engine.on_point(
        "scan-launch-check",
        Box::new(move || {
            clock.advance_ms(5_000);
            process.set(
                101,
                FakeProcess {
                    exists: Some(false),
                    ..FakeProcess::default()
                },
            );
        }),
    );

    assert_eq!(sight(&fx), 2, "s-a's sighting, and the launch closed");
    let scan = SightingSource::Scan;
    assert_eq!(
        sightings(&fx),
        [sighting("s-a", a.as_str(), NOW, NOW, NOW, scan, NOW)],
        "the scan saw the child live as it began"
    );
    assert_eq!(ended(&fx), [(a.to_string(), Some(NOW + 5_000))]);
}

#[test]
fn a_launch_is_open_before_the_spawn_and_attached_after() {
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    assert_eq!(
        id,
        LaunchId {
            provider: cc.clone(),
            account: a.clone(),
            started_at_ms: NOW
        }
    );
    let open = LaunchRow {
        account_id: a.to_string(),
        started_at: NOW,
        ended_at: None,
        pid: None,
        recorded_at: NOW,
    };
    assert_eq!(launches(&fx), [open.clone()]);

    fx.clock.advance_ms(50);
    fx.engine.attach_launch_pid(&id, 4242).unwrap();
    assert_eq!(
        launches(&fx),
        [LaunchRow {
            pid: Some(4242),
            recorded_at: NOW + 50,
            ..open
        }]
    );
}

#[test]
fn a_planned_launch_is_recorded_late_with_its_own_start_and_recording_is_idempotent() {
    // Spec §6.1: `run` plans the launch before the spawn and records it as often as it takes.
    // A record made late keeps the planned start, and a record of a launch already there
    // changes nothing: not its pid, its end, nor when it was written.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let id = fx.engine.plan_launch(&cc, &a);
    assert_eq!(id.started_at_ms, NOW);
    assert!(launches(&fx).is_empty(), "planning writes nothing");

    fx.clock.advance_ms(7_000);
    fx.engine.record_launch(&id).unwrap();
    let open = LaunchRow {
        account_id: a.to_string(),
        started_at: NOW,
        ended_at: None,
        pid: None,
        recorded_at: NOW + 7_000,
    };
    assert_eq!(launches(&fx), [open.clone()]);

    fx.clock.advance_ms(1_000);
    fx.engine.attach_launch_pid(&id, 4242).unwrap();
    let attached = LaunchRow {
        pid: Some(4242),
        recorded_at: NOW + 8_000,
        ..open
    };
    fx.clock.advance_ms(1_000);
    fx.engine.record_launch(&id).unwrap();
    assert_eq!(
        launches(&fx),
        [attached.clone()],
        "a second record is a no-op"
    );

    fx.engine
        .close_launch(
            &id,
            &fx.make_profile(&a),
            &LaunchPoll::default(),
            NOW + 9_000,
            true,
        )
        .stored
        .unwrap();
    let closed = launches(&fx);
    assert_eq!(closed[0].ended_at, Some(NOW + 9_000), "{closed:?}");
    fx.engine.record_launch(&id).unwrap();
    assert_eq!(
        launches(&fx),
        closed,
        "recording never reopens a closed launch"
    );
}

#[test]
fn checking_reads_the_record_only_when_its_mtime_moved() {
    // Spec §6.1: each check `stat`s the child's record, and reads it only when its mtime moved
    // since the read that named the current tenure's session; unchanged, it confirms that
    // tenure. The write extends the stored tenure to its last confirmation, at most once a
    // minute (spec §6.4).
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    let run = SightingSource::Run;

    poll(&fx, &id, &profile, &mut state);
    assert_eq!(state, LaunchPoll::default(), "no record yet");
    assert!(sightings(&fx).is_empty());

    // The child, spawned once the launch began, writes its record.
    let started = NOW + 1_000;
    fx.clock.advance_ms(2_000);
    let polled = NOW + 2_000;
    running(&fx, 777, None);
    let path = record(&fx, &profile, 777, "s-a", started, "interactive");
    set_mtime(&path, 1_790_000_000);
    poll(&fx, &id, &profile, &mut state);
    assert_eq!(
        state.tenure,
        Some(Tenure {
            session_id: "s-a".into(),
            started_at: started,
            first_at: polled,
            confirmed_at: polled
        })
    );
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-a",
            a.as_str(),
            started,
            polled,
            polled,
            run,
            polled
        )]
    );

    // Rewritten under the same mtime: not read again, so the check confirms s-a.
    record(&fx, &profile, 777, "s-b", started, "interactive");
    set_mtime(&path, 1_790_000_000);
    fx.clock.advance_ms(61_000);
    let later = polled + 61_000;
    poll(&fx, &id, &profile, &mut state);
    assert_eq!(
        state
            .tenure
            .as_ref()
            .map(|t| (t.session_id.as_str(), t.confirmed_at)),
        Some(("s-a", later))
    );
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-a",
            a.as_str(),
            started,
            polled,
            later,
            run,
            later
        )]
    );

    // Its mtime moves: read again. `/clear` rewrites the record this way, with a new
    // `sessionId` under the same `startedAt` (spec §17 T1): s-b is sighted from this read, and
    // s-a's tenure ends at its last confirmation.
    set_mtime(&path, 1_790_000_001);
    fx.clock.advance_ms(50);
    let read = later + 50;
    poll(&fx, &id, &profile, &mut state);
    assert_eq!(
        state
            .tenure
            .as_ref()
            .map(|t| (t.session_id.as_str(), t.first_at)),
        Some(("s-b", read))
    );
    assert_eq!(
        sightings(&fx),
        [
            sighting("s-a", a.as_str(), started, polled, later, run, later),
            sighting("s-b", a.as_str(), started, read, read, run, read),
        ]
    );
}

#[test]
fn a_session_that_enters_a_running_process_is_sighted_from_the_read_that_first_named_it() {
    // Spec §6.1, §6.4: the child's record names s-a, and later, under the same `startedAt`,
    // s-b (`/clear` or `/resume`, spec §17 T1). s-b entered a process that was already running:
    // it is sighted from the read that first named it, not from the execution's start, and
    // s-a's tenure ends at the last check that confirmed it.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    let run = SightingSource::Run;
    let started = NOW + 500;
    running(&fx, 777, None);
    let path = record(&fx, &profile, 777, "s-a", started, "interactive");
    set_mtime(&path, 1_790_000_000);
    fx.clock.advance_ms(1_000);
    poll(&fx, &id, &profile, &mut state);
    fx.clock.advance_ms(30_000);
    poll(&fx, &id, &profile, &mut state);
    let confirmed = NOW + 31_000;

    record(&fx, &profile, 777, "s-b", started, "interactive");
    set_mtime(&path, 1_790_000_001);
    fx.clock.advance_ms(9_000);
    poll(&fx, &id, &profile, &mut state);
    let switched = NOW + 40_000;
    assert_eq!(
        sightings(&fx),
        [
            sighting(
                "s-a",
                a.as_str(),
                started,
                NOW + 1_000,
                confirmed,
                run,
                switched
            ),
            sighting(
                "s-b",
                a.as_str(),
                started,
                switched,
                switched,
                run,
                switched
            ),
        ]
    );
}

#[test]
#[cfg(feature = "test-hooks")]
fn a_check_dates_a_tenure_at_its_read() {
    // Spec §6.1: a tenure begins at the read that named its session, and a read confirms it at
    // the read, not after the liveness check that follows. Here 5 s pass right after each read.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    running(&fx, 777, None);
    let path = record(&fx, &profile, 777, "s-a", NOW + 500, "interactive");
    set_mtime(&path, 1_790_000_000);
    let clock = fx.clock.clone();
    fx.engine.on_point(
        "launch-record-read",
        Box::new(move || clock.advance_ms(5_000)),
    );
    fx.clock.set(NOW + 1_000);
    check(&fx, &id, &profile, &mut state);
    let t = state.tenure.clone().unwrap();
    assert_eq!((t.first_at, t.confirmed_at), (NOW + 1_000, NOW + 1_000));

    set_mtime(&path, 1_790_000_001);
    fx.clock.set(NOW + 9_000);
    check(&fx, &id, &profile, &mut state);
    assert_eq!(
        state.tenure.as_ref().map(|t| (t.first_at, t.confirmed_at)),
        Some((NOW + 1_000, NOW + 9_000))
    );
}

#[test]
fn a_return_to_a_session_the_child_left_starts_a_new_tenure() {
    // Spec §6.1, the `/resume` case: the child leaves s-a for s-b and comes back. Both switches
    // fall between two writes, 50 ms checks apart; the write ends each tenure exactly at its
    // last confirmation, and the return to s-a is a tenure of its own. The time s-a spent away,
    // where it may have run elsewhere, is never bridged.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    let started = NOW + 500;
    running(&fx, 777, None);
    let path = record(&fx, &profile, 777, "s-a", started, "interactive");
    set_mtime(&path, 1_790_000_000);
    fx.clock.advance_ms(1_000);
    poll(&fx, &id, &profile, &mut state);
    for (at, session, mtime) in [
        (1_050, None, None),
        (1_100, Some("s-b"), Some(1_790_000_001)),
        (1_150, None, None),
        (1_200, Some("s-a"), Some(1_790_000_002)),
        (1_250, None, None),
    ] {
        if let (Some(session), Some(mtime)) = (session, mtime) {
            record(&fx, &profile, 777, session, started, "interactive");
            set_mtime(&path, mtime);
        }
        fx.clock.set(NOW + at);
        check(&fx, &id, &profile, &mut state);
    }
    assert_eq!(state.ended.len(), 2, "{state:?}");

    fx.engine.poll_launch(&id, &mut state).unwrap();
    assert!(state.ended.is_empty(), "{state:?}");
    let run = |session: &str, first: i64, last: i64| {
        sighting(
            session,
            a.as_str(),
            started,
            NOW + first,
            NOW + last,
            SightingSource::Run,
            NOW + 1_250,
        )
    };
    assert_eq!(
        sightings(&fx),
        [
            run("s-a", 1_000, 1_050),
            run("s-a", 1_200, 1_250),
            run("s-b", 1_100, 1_150)
        ]
    );
}

#[test]
fn a_poll_that_sees_no_session_extends_none() {
    // Spec §6.1: only a check that sees the tenure's session confirms it. A record rewritten
    // into one that does not read as this child's, or one that is gone, shows no session: the
    // child may have left the one it saw last (`/clear`), so that tenure is not extended, by the
    // checks or by the writes after them. Such a record is read again at the next check.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    let run = SightingSource::Run;
    running(&fx, 777, None);
    let started = NOW + 1_000;
    let path = record(&fx, &profile, 777, "s-a", started, "interactive");
    set_mtime(&path, 1_790_000_000);
    fx.clock.advance_ms(2_000);
    let polled = NOW + 2_000;
    poll(&fx, &id, &profile, &mut state);
    let seen = vec![sighting(
        "s-a",
        a.as_str(),
        started,
        polled,
        polled,
        run,
        polled,
    )];
    assert_eq!(sightings(&fx), seen);
    let confirmed = state.clone();

    fs::write(&path, b"{\"pid\": 777, \"sessionId\":").unwrap();
    set_mtime(&path, 1_790_000_001);
    fx.clock.advance_ms(61_000);
    poll(&fx, &id, &profile, &mut state);
    check(&fx, &id, &profile, &mut state);
    assert_eq!(state, confirmed, "a record that does not read");
    assert_eq!(sightings(&fx), seen, "a record that does not read");

    fs::remove_file(&path).unwrap();
    fx.clock.advance_ms(61_000);
    poll(&fx, &id, &profile, &mut state);
    assert_eq!(state, confirmed, "a record that is gone");
    assert_eq!(sightings(&fx), seen, "a record that is gone");
}

#[test]
fn a_stale_record_with_a_reused_pid_is_ignored() {
    // A crashed execution left `777.json` behind, and the OS gave its pid to this child. The
    // record names an older execution, started before this launch began: it is never sighted,
    // nor remembered. Neither is a record of another pid under that name, nor one whose process
    // is gone. The child's own record, once it writes one, is.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    running(&fx, 777, None);
    let path = record(&fx, &profile, 777, "s-old", NOW - 3_600_000, "interactive");
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    poll(&fx, &id, &profile, &mut state);
    assert_eq!(state, LaunchPoll::default(), "not remembered");
    assert!(sightings(&fx).is_empty(), "not sighted");

    let other_pid = json!({
        "pid": 778,
        "procStart": LSTART,
        "startedAt": NOW + 10,
        "kind": "interactive",
        "sessionId": "s-x"
    });
    fs::write(&path, other_pid.to_string()).unwrap();
    poll(&fx, &id, &profile, &mut state);
    gone(&fx, 777);
    record(&fx, &profile, 777, "s-dead", NOW + 10, "interactive");
    poll(&fx, &id, &profile, &mut state);
    assert_eq!(state, LaunchPoll::default());
    assert!(sightings(&fx).is_empty());

    running(&fx, 777, None);
    fx.clock.advance_ms(1_000);
    record(&fx, &profile, 777, "s-new", NOW + 500, "interactive");
    poll(&fx, &id, &profile, &mut state);
    assert_eq!(
        state.tenure,
        Some(Tenure {
            session_id: "s-new".into(),
            started_at: NOW + 500,
            first_at: NOW + 1_000,
            confirmed_at: NOW + 1_000
        })
    );
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-new",
            a.as_str(),
            NOW + 500,
            NOW + 1_000,
            NOW + 1_000,
            SightingSource::Run,
            NOW + 1_000
        )]
    );
}

#[test]
fn a_write_that_fails_keeps_what_it_did_not_store() {
    // Spec §6.1: a failed write loses none of the tenures the checks saw. They stay queued, and
    // the next write ends each exactly at its last confirmation.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    let started = NOW + 500;
    running(&fx, 777, None);
    let path = record(&fx, &profile, 777, "s-a", started, "interactive");
    set_mtime(&path, 1_790_000_000);
    fx.clock.advance_ms(1_000);
    check(&fx, &id, &profile, &mut state);
    record(&fx, &profile, 777, "s-b", started, "interactive");
    set_mtime(&path, 1_790_000_001);
    fx.clock.advance_ms(50);
    check(&fx, &id, &profile, &mut state);
    let raw = rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db")).unwrap();
    raw.execute_batch("ALTER TABLE session_sightings RENAME TO hidden;")
        .unwrap();
    assert!(fx.engine.poll_launch(&id, &mut state).is_err());
    assert_eq!(state.ended.len(), 1, "{state:?}");

    raw.execute_batch("ALTER TABLE hidden RENAME TO session_sightings;")
        .unwrap();
    fx.clock.advance_ms(2_000);
    fx.engine.poll_launch(&id, &mut state).unwrap();
    assert!(state.ended.is_empty(), "{state:?}");
    let written = NOW + 3_050;
    let run = SightingSource::Run;
    assert_eq!(
        sightings(&fx),
        [
            sighting(
                "s-a",
                a.as_str(),
                started,
                NOW + 1_000,
                NOW + 1_000,
                run,
                written
            ),
            sighting(
                "s-b",
                a.as_str(),
                started,
                NOW + 1_050,
                NOW + 1_050,
                run,
                written
            ),
        ]
    );
}

/// Fails every insert of `session`'s sightings while it exists: a store write of one tenure
/// that fails, the others going through.
fn fail_writes_of(fx: &Fx, session: &str) -> rusqlite::Connection {
    let raw = rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db")).unwrap();
    raw.execute_batch(&format!(
        "CREATE TRIGGER injected BEFORE INSERT ON session_sightings \
         WHEN NEW.session_id = '{session}' BEGIN SELECT RAISE(ABORT, 'injected'); END;"
    ))
    .unwrap();
    raw
}

#[test]
fn a_write_stores_the_current_tenure_before_it_ends_any() {
    // Spec §6.1, §6.4: a tenure leaves run's queue only once every later one is stored. Here
    // the write of the current tenure, s-b, fails, so s-a's ended tenure stays queued. When a
    // scan later continues s-a's tenure over the time s-a was away (s-b, which would have
    // stopped it, is still not stored), the next write still ends it exactly.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    let started = NOW + 500;
    let at = |ms: i64| NOW + ms;
    running(&fx, 777, None);
    let path = record(&fx, &profile, 777, "s-a", started, "interactive");
    set_mtime(&path, 1_790_000_000);
    fx.clock.set(at(1_000));
    poll(&fx, &id, &profile, &mut state);
    fx.clock.set(at(1_050));
    check(&fx, &id, &profile, &mut state);
    record(&fx, &profile, 777, "s-b", started, "interactive");
    set_mtime(&path, 1_790_000_001);
    fx.clock.set(at(1_100));
    check(&fx, &id, &profile, &mut state);

    let raw = fail_writes_of(&fx, "s-b");
    assert!(fx.engine.poll_launch(&id, &mut state).is_err());
    let queued = |state: &LaunchPoll| {
        state
            .ended
            .iter()
            .map(|t| t.session_id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(queued(&state), ["s-a"], "s-a's end waits for s-b");
    let run = SightingSource::Run;
    let first = sighting(
        "s-a",
        a.as_str(),
        started,
        at(1_000),
        at(1_000),
        run,
        at(1_000),
    );
    assert_eq!(sightings(&fx), [first]);

    record(&fx, &profile, 777, "s-a", started, "interactive");
    set_mtime(&path, 1_790_000_002);
    fx.clock.set(at(1_200));
    check(&fx, &id, &profile, &mut state);
    fx.clock.set(at(62_000));
    sight(&fx);
    raw.execute_batch("DROP TRIGGER injected;").unwrap();
    fx.clock.set(at(62_050));
    poll(&fx, &id, &profile, &mut state);
    let written = at(62_050);
    assert_eq!(
        sightings(&fx),
        [
            sighting(
                "s-a",
                a.as_str(),
                started,
                at(1_000),
                at(1_050),
                run,
                written
            ),
            sighting("s-a", a.as_str(), started, at(1_200), written, run, written),
            sighting(
                "s-b",
                a.as_str(),
                started,
                at(1_100),
                at(1_100),
                run,
                written
            ),
        ]
    );
}

#[test]
fn a_write_ends_the_newest_tenure_first() {
    // Spec §6.1: the tenures that ended since the last write are ended newest first, so an
    // older one leaves the queue only after the ones that followed it. Here s-b's end fails:
    // s-a's tenure, older, stays queued and its stored end untouched.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    let started = NOW + 500;
    let at = |ms: i64| NOW + ms;
    running(&fx, 777, None);
    let path = record(&fx, &profile, 777, "s-a", started, "interactive");
    set_mtime(&path, 1_790_000_000);
    fx.clock.set(at(1_000));
    poll(&fx, &id, &profile, &mut state);
    for (when, session, mtime) in [
        (1_050, None, None),
        (1_100, Some("s-b"), Some(1_790_000_001)),
        (1_150, None, None),
        (1_200, Some("s-c"), Some(1_790_000_002)),
    ] {
        if let (Some(session), Some(mtime)) = (session, mtime) {
            record(&fx, &profile, 777, session, started, "interactive");
            set_mtime(&path, mtime);
        }
        fx.clock.set(at(when));
        check(&fx, &id, &profile, &mut state);
    }

    let _raw = fail_writes_of(&fx, "s-b");
    assert!(fx.engine.poll_launch(&id, &mut state).is_err());
    assert_eq!(
        state
            .ended
            .iter()
            .map(|t| t.session_id.as_str())
            .collect::<Vec<_>>(),
        ["s-a", "s-b"]
    );
    let run = SightingSource::Run;
    assert_eq!(
        sightings(&fx),
        [
            sighting(
                "s-a",
                a.as_str(),
                started,
                at(1_000),
                at(1_000),
                run,
                at(1_000)
            ),
            sighting(
                "s-c",
                a.as_str(),
                started,
                at(1_200),
                at(1_200),
                run,
                at(1_200)
            ),
        ]
    );
}

#[test]
fn a_scan_after_the_child_returns_to_a_session_never_bridges_its_absence() {
    // Spec §6.4: the child left s-a for s-b for over a minute, then came back. A scan that reads
    // the record after the return, before the poller's next write, finds s-b's tenure stored
    // since s-a's ended, so it starts a tenure of its own rather than stretching s-a's first one
    // over the time s-a was away. The poller's next write stores its own tenure beside it.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    let started = NOW + 500;
    running(&fx, 777, None);
    let path = record(&fx, &profile, 777, "s-a", started, "interactive");
    set_mtime(&path, 1_790_000_000);
    fx.clock.advance_ms(1_000);
    poll(&fx, &id, &profile, &mut state);
    record(&fx, &profile, 777, "s-b", started, "interactive");
    set_mtime(&path, 1_790_000_001);
    fx.clock.advance_ms(50);
    poll(&fx, &id, &profile, &mut state);
    fx.clock.advance_ms(90_000);
    poll(&fx, &id, &profile, &mut state);

    record(&fx, &profile, 777, "s-a", started, "interactive");
    set_mtime(&path, 1_790_000_002);
    fx.clock.advance_ms(50);
    sight(&fx);
    let (run, scan) = (SightingSource::Run, SightingSource::Scan);
    let (left, away, back) = (NOW + 1_000, NOW + 91_050, NOW + 91_100);
    let first = sighting("s-a", a.as_str(), started, left, left, run, left);
    let other = sighting("s-b", a.as_str(), started, left + 50, away, run, away);
    let scanned = sighting("s-a", a.as_str(), started, back, back, scan, back);
    assert_eq!(
        sightings(&fx),
        [first.clone(), scanned.clone(), other.clone()]
    );

    fx.clock.advance_ms(50);
    poll(&fx, &id, &profile, &mut state);
    let polled = sighting(
        "s-a",
        a.as_str(),
        started,
        back + 50,
        back + 50,
        run,
        back + 50,
    );
    assert_eq!(sightings(&fx), [first, scanned, polled, other]);
}

#[test]
fn ending_a_tenure_clamps_a_tenure_bridged_over_the_session_s_absence() {
    // Spec §6.4, criterion 2: run's checks saw s-a leave for s-b at +100 s and come back at
    // +101 s, inside one 2 s write. Before that write stored s-b, the statusline read s-a and
    // continued its own tenure of s-a, the latest there, over the absence. When run ends its
    // tenure of s-a at its last confirmation, every other tenure of s-a in that execution that
    // began by then is clamped to it, so no sighting covers the time s-a was away.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    let started = NOW + 500;
    // The statusline runs under the child, 777.
    running(&fx, std::process::id(), Some(777));
    running(&fx, 777, None);
    let path = record(&fx, &profile, 777, "s-a", started, "interactive");
    set_mtime(&path, 1_790_000_000);
    let at = |ms: i64| NOW + ms;
    let line = |when: i64| {
        fx.clock.set(when);
        fx.engine
            .statusline_sighting(&cc, "s-a", Some(&profile))
            .unwrap();
    };
    fx.clock.set(at(1_000));
    check(&fx, &id, &profile, &mut state);
    line(at(1_010));
    fx.clock.set(at(1_050));
    fx.engine.poll_launch(&id, &mut state).unwrap();
    for (when, session, mtime) in [
        (99_950, None, None),
        (100_000, Some("s-b"), Some(1_790_000_001)),
        (100_950, None, None),
        (101_000, Some("s-a"), Some(1_790_000_002)),
    ] {
        if let (Some(session), Some(mtime)) = (session, mtime) {
            record(&fx, &profile, 777, session, started, "interactive");
            set_mtime(&path, mtime);
        }
        fx.clock.set(at(when));
        check(&fx, &id, &profile, &mut state);
    }
    line(at(101_100));
    assert!(
        sightings(&fx).iter().any(|s| s.last_at == at(101_100)),
        "the statusline bridged its tenure: {:?}",
        sightings(&fx)
    );

    fx.clock.set(at(101_500));
    poll(&fx, &id, &profile, &mut state);
    let (run, statusline) = (SightingSource::Run, SightingSource::Statusline);
    let written = at(101_500);
    assert_eq!(
        sightings(&fx),
        [
            sighting(
                "s-a",
                a.as_str(),
                started,
                at(1_000),
                at(99_950),
                run,
                written
            ),
            sighting(
                "s-a",
                a.as_str(),
                started,
                at(1_010),
                at(99_950),
                statusline,
                written
            ),
            sighting(
                "s-a",
                a.as_str(),
                started,
                at(101_000),
                written,
                run,
                written
            ),
            sighting(
                "s-b",
                a.as_str(),
                started,
                at(100_000),
                at(100_950),
                run,
                written
            ),
        ]
    );
}

#[test]
fn close_leaves_the_launch_open_while_a_background_session_runs() {
    // Spec §6.1: at the child's exit its tenure ends at its last confirmation, and the launch
    // closes only once the profile is no longer in use. A background session the child started
    // keeps it open; the next scan that finds the profile idle closes it.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    running(&fx, 777, None);
    let child = record(&fx, &profile, 777, "s-a", NOW, "interactive");
    poll(&fx, &id, &profile, &mut state);
    // The child exits and its record goes; a background session it started lives on.
    fs::remove_file(&child).unwrap();
    gone(&fx, 777);
    running(&fx, 888, None);
    let background = record(&fx, &profile, 888, "s-bg", NOW, "bg");
    fx.clock.advance_ms(5_000);

    let exit = fx.engine.now_ms();
    fx.engine
        .close_launch(&id, &profile, &state, exit, true)
        .stored
        .unwrap();
    assert!(
        sightings(&fx).contains(&sighting(
            "s-a",
            a.as_str(),
            NOW,
            NOW,
            NOW,
            SightingSource::Run,
            NOW
        )),
        "{:?}",
        sightings(&fx)
    );
    assert_eq!(ended(&fx), [(a.to_string(), None)]);

    gone(&fx, 888);
    fs::remove_file(&background).unwrap();
    fx.clock.advance_ms(1_000);
    sight(&fx);
    assert_eq!(ended(&fx), [(a.to_string(), Some(NOW + 6_000))]);
}

#[test]
fn close_leaves_the_launch_open_while_another_launcher_holds_the_profile() {
    // Another process's held launch reservation (spec §12.5) is the profile in use: the launch
    // stays open until a close or a scan finds the profile idle.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let other = FlockGuard::try_lock(&profile.join(LAUNCH_DIR).join("4242.lock"))
        .unwrap()
        .unwrap();
    let first = fx.engine.open_launch(&cc, &a).unwrap();
    fx.engine
        .close_launch(&first, &profile, &LaunchPoll::default(), NOW, true)
        .stored
        .unwrap();
    assert_eq!(
        ended(&fx),
        [(a.to_string(), None)],
        "another launcher holds the profile"
    );

    drop(other);
    fx.clock.advance_ms(1_000);
    let second = fx.engine.open_launch(&cc, &a).unwrap();
    fx.engine
        .close_launch(&second, &profile, &LaunchPoll::default(), NOW + 1_000, true)
        .stored
        .unwrap();
    assert_eq!(
        ended(&fx),
        [(a.to_string(), None), (a.to_string(), Some(NOW + 1_000))]
    );
}

#[test]
fn close_ends_the_tenure_at_its_last_read_and_never_moves_it_back() {
    // Spec §6.1: at the child's exit its tenure ends at its last confirming read, not at the
    // exit: the child may have left its session after that read. Every stored end is a read, so
    // a later one stands: here a scan's read between run's last check and the exit, which the
    // close keeps. The launch still closes at its idle check, after exit handling.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    let run = SightingSource::Run;
    let started = NOW + 500;
    running(&fx, 777, None);
    let child = record(&fx, &profile, 777, "s-a", started, "interactive");
    set_mtime(&child, 1_790_000_000);
    fx.clock.advance_ms(1_000);
    poll(&fx, &id, &profile, &mut state);
    fx.clock.advance_ms(30_000);
    check(&fx, &id, &profile, &mut state);
    let confirmed = NOW + 31_000;
    fx.clock.advance_ms(61_000);
    sight(&fx);
    let scanned = NOW + 92_000;
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-a",
            a.as_str(),
            started,
            NOW + 1_000,
            scanned,
            run,
            scanned
        )],
        "the scan continued run's tenure"
    );
    fs::remove_file(&child).unwrap();
    gone(&fx, 777);
    fx.clock.advance_ms(10);
    let exit = fx.engine.now_ms();
    fx.clock.advance_ms(45_000);
    let closed = fx.engine.now_ms();

    fx.engine
        .close_launch(&id, &profile, &state, exit, true)
        .stored
        .unwrap();
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-a",
            a.as_str(),
            started,
            NOW + 1_000,
            scanned,
            run,
            scanned
        )],
        "the scan's read stands"
    );
    assert_eq!(ended(&fx), [(a.to_string(), Some(closed))]);

    // Without that read, the close moves the end forward to run's last confirmation.
    let fx2 = Fx::new();
    let a2 = fx2.add("a@x.co", "rt-a");
    let profile2 = fx2.make_profile(&a2);
    let id2 = fx2.engine.open_launch(&fx2.provider(), &a2).unwrap();
    let mut state2 = LaunchPoll::default();
    running(&fx2, 777, None);
    let child2 = record(&fx2, &profile2, 777, "s-a", started, "interactive");
    set_mtime(&child2, 1_790_000_000);
    fx2.clock.advance_ms(1_000);
    poll(&fx2, &id2, &profile2, &mut state2);
    fx2.clock.advance_ms(30_000);
    check(&fx2, &id2, &profile2, &mut state2);
    fx2.clock.advance_ms(5_000);
    let exit2 = fx2.engine.now_ms();
    fx2.engine
        .close_launch(&id2, &profile2, &state2, exit2, true)
        .stored
        .unwrap();
    assert_eq!(
        sightings(&fx2),
        [sighting(
            "s-a",
            a2.as_str(),
            started,
            NOW + 1_000,
            confirmed,
            run,
            exit2
        )]
    );
}

#[test]
fn a_write_after_the_exit_is_capped_at_the_close() {
    // Spec §6.1: the poller can finish one more tick after the exit was stamped. On a record a
    // crash left behind, unchanged, its check confirms after the exit, and its write stores
    // that (a minute after the first, so past the gap). The close caps the tenure at the exit:
    // no sighting ends after the child did.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    let started = NOW + 500;
    running(&fx, 777, None);
    let child = record(&fx, &profile, 777, "s-a", started, "interactive");
    set_mtime(&child, 1_790_000_000);
    fx.clock.advance_ms(1_000);
    poll(&fx, &id, &profile, &mut state);
    fx.clock.advance_ms(61_000);
    let exit = fx.engine.now_ms();
    gone(&fx, 777);
    fx.clock.advance_ms(40);
    poll(&fx, &id, &profile, &mut state);
    assert_eq!(
        sightings(&fx)[0].last_at,
        exit + 40,
        "the late write stored a check after the exit"
    );

    fx.engine
        .close_launch(&id, &profile, &state, exit, true)
        .stored
        .unwrap();
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-a",
            a.as_str(),
            started,
            NOW + 1_000,
            exit,
            SightingSource::Run,
            exit + 40
        )]
    );
}

#[test]
fn a_check_after_the_exit_confirms_nothing() {
    // Spec §6.1: a crash leaves the child's record behind, unchanged, so a check after the
    // child exited still finds the mtime of the read that named its session. The close ends the
    // tenure no later than the exit.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    let started = NOW + 500;
    running(&fx, 777, None);
    let child = record(&fx, &profile, 777, "s-a", started, "interactive");
    set_mtime(&child, 1_790_000_000);
    fx.clock.advance_ms(1_000);
    poll(&fx, &id, &profile, &mut state);
    fx.clock.advance_ms(10_000);
    let exit = fx.engine.now_ms();
    gone(&fx, 777);
    fx.clock.advance_ms(40);
    check(&fx, &id, &profile, &mut state);
    assert_eq!(
        state.tenure.as_ref().map(|t| t.confirmed_at),
        Some(exit + 40)
    );

    fx.engine
        .close_launch(&id, &profile, &state, exit, true)
        .stored
        .unwrap();
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-a",
            a.as_str(),
            started,
            NOW + 1_000,
            exit,
            SightingSource::Run,
            exit + 40
        )]
    );
}

#[test]
#[cfg(feature = "test-hooks")]
fn close_ends_the_launch_at_its_idle_check() {
    // Spec §6.1: as in the scan, the launch ends when the close finds the profile idle, not when
    // the close began.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let clock = fx.clock.clone();
    fx.engine.on_point(
        "close-launch-check",
        Box::new(move || clock.advance_ms(5_000)),
    );

    fx.engine
        .close_launch(&id, &profile, &LaunchPoll::default(), NOW, true)
        .stored
        .unwrap();
    assert_eq!(ended(&fx), [(a.to_string(), Some(NOW + 5_000))]);
}

#[test]
fn the_statusline_extends_only_its_own_execution() {
    // Spec §6.3: the statusline finds the Claude Code process it runs under among its
    // ancestors, by that process's record in the home, and writes that execution's sighting.
    // An earlier execution's sighting of the same session is never extended.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    running(&fx, std::process::id(), Some(500));
    running(&fx, 500, Some(600));
    running(&fx, 600, Some(1));
    // Claude Code, two levels up, in the default home.
    record(
        &fx,
        &fx.paths().config_home,
        600,
        "s-1",
        NOW - 60_000,
        "interactive",
    );
    let scan = SightingSource::Scan;
    let earlier = sighting(
        "s-1",
        "",
        NOW - 3_600_000,
        NOW - 3_600_000,
        NOW - 3_000_000,
        scan,
        NOW - 3_000_000,
    );
    fx.engine
        .store()
        .unwrap()
        .record_sighting(&cc, &earlier, 60_000)
        .unwrap();

    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    let statusline = SightingSource::Statusline;
    let ours = sighting("s-1", "", NOW - 60_000, NOW, NOW, statusline, NOW);
    assert_eq!(sightings(&fx), [earlier.clone(), ours.clone()]);

    // Within the minute, nothing is written; past it, only its own execution's sighting moves.
    fx.clock.advance_ms(30_000);
    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    assert_eq!(sightings(&fx), [earlier.clone(), ours.clone()]);
    fx.clock.advance_ms(31_000);
    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    let later = NOW + 61_000;
    assert_eq!(
        sightings(&fx),
        [
            earlier.clone(),
            SightingRow {
                last_at: later,
                recorded_at: later,
                ..ours
            }
        ]
    );

    // In a run shell, the home is the profile, and the profile's records are searched: there,
    // 500 is the Claude Code process.
    record(&fx, &profile, 500, "s-2", NOW - 1_000, "interactive");
    fx.engine
        .statusline_sighting(&cc, "s-2", Some(&profile))
        .unwrap();
    assert!(sightings(&fx).contains(&sighting(
        "s-2",
        a.as_str(),
        NOW - 1_000,
        later,
        later,
        statusline,
        later
    )));
}

#[test]
fn the_statusline_starts_a_tenure_at_its_read_and_continues_the_latest() {
    // Spec §6.3, §6.4: the ancestor's record is the execution, not when the session entered it.
    // A session the line reports is sighted from that run of the line on, however old the
    // execution, and later runs continue its tenure. A return to it (`/resume`) after the line
    // reported another session there starts a new tenure.
    let fx = Fx::new();
    let cc = fx.provider();
    fx.add("a@x.co", "rt-a");
    running(&fx, std::process::id(), Some(500));
    running(&fx, 500, Some(1));
    let home = fx.paths().config_home;
    let started = NOW - 3_600_000;
    record(&fx, &home, 500, "s-1", started, "interactive");
    let line = SightingSource::Statusline;
    let seen = |session: &str, first: i64, last: i64| {
        sighting(session, "", started, first, last, line, last)
    };

    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    assert_eq!(sightings(&fx), [seen("s-1", NOW, NOW)]);
    fx.clock.advance_ms(61_000);
    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    assert_eq!(sightings(&fx), [seen("s-1", NOW, NOW + 61_000)]);

    record(&fx, &home, 500, "s-2", started, "interactive");
    fx.clock.advance_ms(5_000);
    fx.engine.statusline_sighting(&cc, "s-2", None).unwrap();
    record(&fx, &home, 500, "s-1", started, "interactive");
    fx.clock.advance_ms(60_000);
    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    let (cleared, back) = (NOW + 66_000, NOW + 126_000);
    assert_eq!(
        sightings(&fx),
        [
            seen("s-1", NOW, NOW + 61_000),
            seen("s-1", back, back),
            seen("s-2", cleared, cleared)
        ]
    );
}

#[test]
#[cfg(feature = "test-hooks")]
fn the_statusline_dates_a_sighting_when_it_starts() {
    // Spec §6.3, §6.4: the line's session comes from its environment, fixed when Claude Code
    // spawned it; the ancestor's record proves only the execution. So the sighting is dated
    // when the line starts, before the freshness lookup and the walk, not at the record's read.
    // Here 5 s pass before that read.
    let fx = Fx::new();
    let cc = fx.provider();
    fx.add("a@x.co", "rt-a");
    running(&fx, std::process::id(), Some(500));
    running(&fx, 500, Some(1));
    record(
        &fx,
        &fx.paths().config_home,
        500,
        "s-1",
        NOW - 60_000,
        "interactive",
    );
    let clock = fx.clock.clone();
    fx.engine.on_point(
        "statusline-ancestor",
        Box::new(move || clock.advance_ms(5_000)),
    );

    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    let seen = sightings(&fx);
    assert_eq!(
        (seen[0].first_at, seen[0].last_at, seen[0].recorded_at),
        (NOW, NOW, NOW + 5_000),
        "dated when the line started, written later: {seen:?}"
    );
}

#[test]
fn the_statusline_records_nothing_without_an_ancestor_record() {
    let fx = Fx::new();
    let cc = fx.provider();
    let home = fx.paths().config_home;
    // Without a store, it creates none.
    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    assert!(!fx.env.data_dir().join("tagteam.db").exists());

    fx.add("a@x.co", "rt-a");
    // Nine ancestors; only the ninth has a record, and so does a process that is no ancestor.
    let chain: Vec<u32> = (1001..=1009).collect();
    running(&fx, std::process::id(), Some(chain[0]));
    for pair in chain.windows(2) {
        running(&fx, pair[0], Some(pair[1]));
    }
    running(&fx, chain[8], Some(1));
    record(&fx, &home, chain[8], "s-1", NOW, "interactive");
    running(&fx, 4242, Some(1));
    record(&fx, &home, 4242, "s-1", NOW, "interactive");
    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    assert!(
        sightings(&fx).is_empty(),
        "the walk stops after eight ancestors"
    );

    // The eighth is within reach.
    record(&fx, &home, chain[7], "s-1", NOW - 5_000, "interactive");
    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-1",
            "",
            NOW - 5_000,
            NOW,
            NOW,
            SightingSource::Statusline,
            NOW
        )]
    );
}

#[test]
fn the_statusline_never_waits_on_a_busy_store() {
    // Spec §6.3: the write waits at most `CACHE_WRITE_WAIT` (5 ms); a busy store skips it, and
    // the call still succeeds. How long it took is left to the `#[ignore]` perf tests.
    let fx = Fx::new();
    let cc = fx.provider();
    fx.add("a@x.co", "rt-a");
    running(&fx, std::process::id(), Some(500));
    running(&fx, 500, Some(1));
    record(&fx, &fx.paths().config_home, 500, "s-1", NOW, "interactive");
    let other = rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db")).unwrap();
    other.execute_batch("BEGIN IMMEDIATE;").unwrap();

    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    other.execute_batch("ROLLBACK;").unwrap();
    assert!(sightings(&fx).is_empty(), "a busy store skips the write");

    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-1",
            "",
            NOW,
            NOW,
            NOW,
            SightingSource::Statusline,
            NOW
        )]
    );
}

/// A pending launch file (spec §6.1) of `account` in `profile`, written as `run` writes it.
fn pending(
    fx: &Fx,
    profile: &Path,
    account: &AccountId,
    started_at_ms: i64,
    ended_at_ms: Option<i64>,
    run_pid: u32,
) -> PathBuf {
    PendingLaunch {
        provider: fx.provider(),
        account_id: account.clone(),
        started_at_ms,
        ended_at_ms,
    }
    .write(profile, run_pid)
    .unwrap();
    PendingLaunch::path(profile, started_at_ms, run_pid)
}

/// What is in `profile`'s pending directory, by name.
fn pending_files(profile: &Path) -> Vec<String> {
    let Ok(listing) = fs::read_dir(profile.join(PENDING_LAUNCH_DIR)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = listing
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_scan_records_a_pending_launch_open_and_removes_its_file() {
    // Spec §6.2: a launch `run` kept in its profile because the store refused it is recorded
    // from its own start, open while the profile is in use, and its file goes.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let _in_use = FlockGuard::try_lock(&profile.join(LAUNCH_DIR).join("4242.lock"))
        .unwrap()
        .unwrap();
    let file = pending(&fx, &profile, &a, NOW - 5_000, None, 4242);

    assert_eq!(sight(&fx), 1, "the launch recorded");
    assert_eq!(
        launches(&fx),
        [LaunchRow {
            account_id: a.to_string(),
            started_at: NOW - 5_000,
            ended_at: None,
            pid: None,
            recorded_at: NOW,
        }]
    );
    assert!(!file.exists(), "{:?}", pending_files(&profile));
    assert_eq!(sight(&fx), 0, "nothing more to record");
}

#[test]
fn a_pending_launch_with_an_end_is_recorded_closed_at_it() {
    // Spec §6.2: `endedAt` is the moment `run`'s close found the profile idle; the scan closes
    // the launch there, not when it finds the file.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let file = pending(&fx, &profile, &a, NOW - 5_000, Some(NOW - 1_000), 4242);
    fx.clock.advance_ms(10_000);

    assert_eq!(sight(&fx), 1);
    assert_eq!(ended(&fx), [(a.to_string(), Some(NOW - 1_000))]);
    assert_eq!(launches(&fx)[0].recorded_at, NOW + 10_000);
    assert!(!file.exists());
}

#[test]
fn a_pending_launch_the_store_already_holds_changes_nothing() {
    // Spec §6.2: both writes are idempotent. A launch `run` recorded late, whose file a failed
    // removal left behind, keeps its stored end, and the file goes.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    fx.engine
        .close_launch(&id, &profile, &LaunchPoll::default(), NOW, true)
        .stored
        .unwrap();
    let stored = launches(&fx);
    let file = pending(&fx, &profile, &a, id.started_at_ms, Some(NOW + 9_000), 4242);
    fx.clock.advance_ms(10_000);

    assert_eq!(sight(&fx), 0);
    assert_eq!(launches(&fx), stored);
    assert!(!file.exists());
}

#[test]
fn a_scan_leaves_a_pending_file_that_is_not_a_launch_of_its_profile_where_it_is() {
    // Spec §6.2: a file that does not parse, or names another provider or another account than
    // its profile, is skipped and kept; a writer's temp file (not `*.json`) is ignored. The
    // valid file beside them is recorded and removed all the same.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let dir = profile.join(PENDING_LAUNCH_DIR);
    let good = pending(&fx, &profile, &a, NOW - 5_000, Some(NOW - 4_000), 1);
    fs::write(dir.join("1000-2.json"), "{ torn").unwrap();
    let other_account = pending(
        &fx,
        &profile,
        &AccountId::from_string("someone-else"),
        2_000,
        None,
        3,
    );
    let other_provider = dir.join("3000-4.json");
    PendingLaunch {
        provider: ProviderId::new("other-provider"),
        account_id: a.clone(),
        started_at_ms: 3_000,
        ended_at_ms: None,
    }
    .write(&profile, 4)
    .unwrap();
    let temp = dir.join(".4000-5.json.tagteam-5-0a1b2c3d");
    fs::write(&temp, fs::read(&good).unwrap()).unwrap();

    assert_eq!(sight(&fx), 1, "only the valid launch");
    assert_eq!(ended(&fx), [(a.to_string(), Some(NOW - 4_000))]);
    assert!(!good.exists());
    for kept in [dir.join("1000-2.json"), other_account, other_provider, temp] {
        assert!(kept.exists(), "{kept:?} is left in place");
    }
}

#[test]
fn a_launch_kept_unrecorded_is_closed_with_the_close_time_and_dropped_once_recorded() {
    // Spec §6.1: at the close an unrecorded launch is kept in its pending file, with the close
    // time as `endedAt` once the profile is idle (the moment the store's close would have
    // stored) and null while it is in use. A launch the caller says is recorded loses its file.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.plan_launch(&cc, &a);
    fx.engine.keep_pending_launch(&id, &profile, None).unwrap();
    let file = PendingLaunch::path(&profile, id.started_at_ms, std::process::id());
    let read = || {
        let all = PendingLaunch::read_all(&profile);
        assert_eq!(all.len(), 1, "{all:?}");
        all.into_iter().next().unwrap().1.unwrap()
    };
    assert_eq!(read().ended_at_ms, None, "kept open while the child runs");

    let in_use = FlockGuard::try_lock(&profile.join(LAUNCH_DIR).join("4242.lock"))
        .unwrap()
        .unwrap();
    let closed = fx
        .engine
        .close_launch(&id, &profile, &LaunchPoll::default(), NOW, false);
    assert!(closed.kept, "{closed:?}");
    assert_eq!(read().ended_at_ms, None, "the profile is in use");

    drop(in_use);
    fx.clock.advance_ms(3_000);
    let closed = fx
        .engine
        .close_launch(&id, &profile, &LaunchPoll::default(), NOW, false);
    assert!(closed.kept, "{closed:?}");
    assert_eq!(read().ended_at_ms, Some(NOW + 3_000));
    assert!(launches(&fx).is_empty(), "nothing went to the store");

    let closed = fx
        .engine
        .close_launch(&id, &profile, &LaunchPoll::default(), NOW, true);
    assert!(!closed.kept, "{closed:?}");
    assert!(!file.exists(), "recorded: the file goes");
}

#[test]
fn remove_records_the_pending_launches_of_the_profile_it_deletes() {
    // Spec §13: `remove` records a profile's pending launches before it deletes the profile,
    // as the scan does; the launch outlives the account.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    pending(&fx, &profile, &a, NOW - 5_000, Some(NOW - 1_000), 4242);

    fx.engine.remove(&a).unwrap();

    assert!(!profile.exists(), "the profile is gone");
    assert_eq!(ended(&fx), [(a.to_string(), Some(NOW - 1_000))]);
}

#[test]
#[cfg(feature = "test-hooks")]
fn a_pending_file_replaced_during_the_scan_is_kept_for_the_next_one() {
    // Spec §6.2: `run`'s close renames a newer file (with `endedAt`) over its own while the
    // scan has the older one in hand. The scan records what it read and leaves the newer file,
    // which the next scan records, closing the launch.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let _in_use = FlockGuard::try_lock(&profile.join(LAUNCH_DIR).join("4242.lock"))
        .unwrap()
        .unwrap();
    let file = pending(&fx, &profile, &a, NOW - 5_000, None, 4242);
    let (account, dir) = (a.clone(), profile.clone());
    let provider = fx.provider();
    let swapped = std::sync::atomic::AtomicBool::new(false);
    fx.engine.on_point(
        "scan-pending-recorded",
        Box::new(move || {
            // Once: the second scan reads the newer file.
            if !swapped.swap(true, std::sync::atomic::Ordering::SeqCst) {
                PendingLaunch {
                    provider: provider.clone(),
                    account_id: account.clone(),
                    started_at_ms: NOW - 5_000,
                    ended_at_ms: Some(NOW - 1_000),
                }
                .write(&dir, 4242)
                .unwrap();
            }
        }),
    );

    sight(&fx);
    assert_eq!(ended(&fx), [(a.to_string(), None)], "what was read");
    assert!(file.exists(), "the newer file is kept");
    assert_eq!(
        PendingLaunch::read_all(&profile)[0]
            .1
            .as_ref()
            .unwrap()
            .ended_at_ms,
        Some(NOW - 1_000)
    );

    sight(&fx);
    assert_eq!(ended(&fx), [(a.to_string(), Some(NOW - 1_000))]);
    assert!(!file.exists());
}

#[test]
fn remove_fails_before_deleting_anything_when_the_store_refuses_the_pending_launches() {
    // Spec §13: the launches are recorded before the vault entry, the rescue files or the
    // profile go, so a store that refuses them fails the remove with the account as it was.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live
    let profile = fx.make_profile(&a);
    let file = pending(&fx, &profile, &a, NOW - 5_000, None, 4242);
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute_batch("DROP TABLE run_launches")
        .unwrap();

    let err = fx.engine.remove(&a).unwrap_err();

    assert_eq!(err.kind(), "store", "{err}");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert!(profile.exists() && file.exists(), "the profile is kept");
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_some());
}

#[test]
fn a_close_with_a_store_that_cannot_be_opened_still_keeps_the_launch_in_its_pending_file() {
    // Spec §6.1: the file needs the profile alone. An engine whose store cannot be opened (the
    // database's path is a directory) cannot end the tenures or close the launch, but the
    // unrecorded launch is kept with the close time, and a recorded one is not claimed kept.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.plan_launch(&cc, &a);
    let db = fx.env.data_dir().join("tagteam.db");
    let engine = fx.engine_with_settings(Default::default());
    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", db.display()));
    }
    fs::create_dir(&db).unwrap();
    assert!(engine.store().is_err(), "the premise: no store");

    let closed = engine.close_launch(&id, &profile, &LaunchPoll::default(), NOW, false);

    assert!(closed.stored.is_err(), "{closed:?}");
    assert!(closed.kept, "{closed:?}");
    let kept = PendingLaunch::read_all(&profile).remove(0).1.unwrap();
    assert_eq!(
        (kept.started_at_ms, kept.ended_at_ms),
        (id.started_at_ms, Some(NOW)),
        "closed at the idle check"
    );
}
