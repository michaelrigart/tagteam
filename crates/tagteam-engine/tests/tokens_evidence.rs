//! Spec §6: the evidence that places a session in a home: sightings from the scan, from `run`'s
//! poll and from the statusline, and `run`'s launches. Session records are judged by the
//! fixture's `FakeProcessProbe`, never by a real pid, and times come from the fixture's clock.
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::{Fx, LSTART};
use serde_json::json;
use tagteam_engine::store::{LaunchRow, SightingRow, SightingSource};
use tagteam_engine::tokens::{LaunchId, LaunchPoll};
use tagteam_provider::{FakeProcess, FlockGuard, LAUNCH_DIR, parse_lstart};

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
        )
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
fn polling_reads_the_record_only_when_it_changed() {
    // Spec §6.1: `run` reads its child's record only when its mtime moved, and extends the
    // sighting of the session it saw last, at most once a minute (spec §6.4).
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    let run = SightingSource::Run;

    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    assert_eq!(state, LaunchPoll::default(), "no record yet");
    assert!(sightings(&fx).is_empty());

    // The child, spawned once the launch began, writes its record.
    let started = NOW + 1_000;
    fx.clock.advance_ms(2_000);
    let polled = NOW + 2_000;
    running(&fx, 777, None);
    let path = record(&fx, &profile, 777, "s-a", started, "interactive");
    set_mtime(&path, 1_790_000_000);
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    assert_eq!(state.last_sighting, Some(("s-a".to_owned(), started)));
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-a",
            a.as_str(),
            started,
            started,
            polled,
            run,
            polled
        )]
    );

    // Rewritten under the same mtime: not read again, and the sighting it saw is extended.
    record(&fx, &profile, 777, "s-b", started, "interactive");
    set_mtime(&path, 1_790_000_000);
    fx.clock.advance_ms(61_000);
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    assert_eq!(state.last_sighting, Some(("s-a".to_owned(), started)));
    let later = polled + 61_000;
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-a",
            a.as_str(),
            started,
            started,
            later,
            run,
            later
        )]
    );

    // Its mtime moves: read again. `/clear` rewrites the record this way, with a new
    // `sessionId` under the same `startedAt` (spec §17 T1), and the new session is sighted too.
    set_mtime(&path, 1_790_000_001);
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    assert_eq!(state.last_sighting, Some(("s-b".to_owned(), started)));
    assert_eq!(
        sightings(&fx),
        [
            sighting("s-a", a.as_str(), started, started, later, run, later),
            sighting("s-b", a.as_str(), started, started, later, run, later),
        ]
    );
}

#[test]
fn a_poll_that_sees_no_session_extends_none() {
    // Spec §6.1: a poll extends the sighting of the session it sees. A record rewritten into
    // one that does not read as this child's, or one that is gone, shows no session: the child
    // may have left the one it saw last (`/clear`), so that sighting is not extended.
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
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    let seen = vec![sighting(
        "s-a",
        a.as_str(),
        started,
        started,
        polled,
        run,
        polled,
    )];
    assert_eq!(sightings(&fx), seen);

    fs::write(&path, b"{\"pid\": 777, \"sessionId\":").unwrap();
    set_mtime(&path, 1_790_000_001);
    fx.clock.advance_ms(61_000);
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    assert_eq!(sightings(&fx), seen, "a record that does not read");

    fs::remove_file(&path).unwrap();
    fx.clock.advance_ms(61_000);
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
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
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
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
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    gone(&fx, 777);
    record(&fx, &profile, 777, "s-dead", NOW + 10, "interactive");
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    assert_eq!(state, LaunchPoll::default());
    assert!(sightings(&fx).is_empty());

    running(&fx, 777, None);
    fx.clock.advance_ms(1_000);
    record(&fx, &profile, 777, "s-new", NOW + 500, "interactive");
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    assert_eq!(state.last_sighting, Some(("s-new".to_owned(), NOW + 500)));
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-new",
            a.as_str(),
            NOW + 500,
            NOW + 500,
            NOW + 1_000,
            SightingSource::Run,
            NOW + 1_000
        )]
    );
}

#[test]
fn close_leaves_the_launch_open_while_a_background_session_runs() {
    // Spec §6.1: at the child's exit the sighting it saw last ends then, and the launch closes
    // only once the profile is no longer in use. A background session the child started keeps
    // it open; the next scan that finds the profile idle closes it.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    running(&fx, 777, None);
    let child = record(&fx, &profile, 777, "s-a", NOW, "interactive");
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    // The child exits and its record goes; a background session it started lives on.
    fs::remove_file(&child).unwrap();
    gone(&fx, 777);
    running(&fx, 888, None);
    let background = record(&fx, &profile, 888, "s-bg", NOW, "bg");
    fx.clock.advance_ms(5_000);

    let exit = fx.engine.now_ms();
    fx.engine.close_launch(&id, &profile, &state, exit).unwrap();
    assert!(
        sightings(&fx).contains(&sighting(
            "s-a",
            a.as_str(),
            NOW,
            NOW,
            exit,
            SightingSource::Run,
            exit
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
        .close_launch(&first, &profile, &LaunchPoll::default(), NOW)
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
        .close_launch(&second, &profile, &LaunchPoll::default(), NOW + 1_000)
        .unwrap();
    assert_eq!(
        ended(&fx),
        [(a.to_string(), None), (a.to_string(), Some(NOW + 1_000))]
    );
}

#[test]
fn close_stamps_the_sighting_at_the_exit_not_at_the_close() {
    // Spec §6.1: the sighting ends when the child exited. `run`'s exit handling, which may wait
    // on locks, comes before the close; only the launch may close late. A poll in flight as the
    // child exited may have stamped the sighting later: the close moves it back to the exit.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    running(&fx, 777, None);
    let child = record(&fx, &profile, 777, "s-a", NOW + 500, "interactive");
    fx.clock.advance_ms(1_000);
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    // The child exits at t1. The poller's last round, in flight then, saw the record before it
    // went and stamps the sighting past the exit, and exit handling takes until t2.
    fx.clock.advance_ms(30_000);
    let t1 = fx.engine.now_ms();
    fx.clock.advance_ms(61_000);
    let stamped = fx.engine.now_ms();
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    fs::remove_file(&child).unwrap();
    gone(&fx, 777);
    let run = SightingSource::Run;
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-a",
            a.as_str(),
            NOW + 500,
            NOW + 500,
            stamped,
            run,
            stamped
        )],
        "the late poll stamped it past the exit"
    );
    fx.clock.advance_ms(45_000);
    let t2 = fx.engine.now_ms();

    fx.engine.close_launch(&id, &profile, &state, t1).unwrap();
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-a",
            a.as_str(),
            NOW + 500,
            NOW + 500,
            t1,
            run,
            t2
        )],
        "the close moves it back to the exit"
    );
    assert_eq!(ended(&fx), [(a.to_string(), Some(t2))]);
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
        .close_launch(&id, &profile, &LaunchPoll::default(), NOW)
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
