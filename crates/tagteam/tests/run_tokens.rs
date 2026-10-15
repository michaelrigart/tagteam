//! `tagteam run`'s evidence for the token report (token report spec §6.1), through the binary
//! and the fake `claude`: the launch it records before the spawn and closes at the exit, and
//! the session it sights while the child runs. Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use common::{
    FakeCall, fake_claude, fake_claude_calls, hold_launch, path_with, std_cmd, two_fresh_accounts,
};
use serde_json::Value;
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::store::{LaunchRow, SightingRow, SightingSource, Store};
use tagteam_provider::{Env, profile_path};

/// How long one step may take before it counts as stuck.
const LONG: Duration = Duration::from_secs(20);

/// `a@x.co` at position 1 and `b@x.co` at position 2, the live login, with the fake `claude`
/// in `<root>/bin` and a working directory `<root>/work`: `run 1` launches a session.
struct Home {
    dir: tempfile::TempDir,
    /// Account `a`'s id.
    a: String,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (a, _b) = two_fresh_accounts(dir.path());
        fake_claude(dir.path());
        fs::create_dir_all(dir.path().join("work")).unwrap();
        Home { dir, a }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// A fresh `FAKE_CLAUDE_OUT` file for one run of the fake `claude`.
    fn out(&self, name: &str) -> PathBuf {
        let dir = self.root().join("calls");
        fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// `tagteam run 1 -- x` in `<root>/work`, the fake `claude` first on `PATH`, recording its
    /// runs in `out`, with stdout and stderr piped.
    fn run_a(&self, out: &Path) -> Command {
        let mut c = std_cmd(self.root());
        c.env("PATH", path_with(&self.root().join("bin")))
            .env("FAKE_CLAUDE_OUT", out)
            .current_dir(self.root().join("work"))
            .args(["run", "1", "--", "x"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }

    /// Account `a`'s profile (core §12.2).
    fn profile(&self) -> PathBuf {
        profile_path(
            &Env::for_test(self.root()),
            &AccountId::from_string(&self.a),
        )
    }

    fn store(&self) -> Store {
        Store::open_existing(&Env::for_test(self.root()).data_dir().join("tagteam.db"))
            .unwrap()
            .unwrap()
    }

    fn launches(&self) -> Vec<LaunchRow> {
        self.store()
            .launches(&ProviderId::new(CLAUDE_CODE))
            .unwrap()
    }

    fn sightings(&self) -> Vec<SightingRow> {
        self.store()
            .sightings(&ProviderId::new(CLAUDE_CODE))
            .unwrap()
    }
}

/// Creates the hold file when dropped, so a failing test never leaves a fake `claude` running
/// out its 30 s.
struct Release(PathBuf);

impl Drop for Release {
    fn drop(&mut self) {
        let _ = fs::write(&self.0, b"");
    }
}

/// Polls `ready` every 20 ms until it gives a value; fails after `LONG`.
fn wait_for<T>(what: &str, ready: impl Fn() -> Option<T>) -> T {
    let deadline = Instant::now() + LONG;
    loop {
        if let Some(v) = ready() {
            return v;
        }
        assert!(Instant::now() < deadline, "never got to {what}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// The session the fake `claude` recorded in `out`, once it runs.
fn running(out: &Path) -> FakeCall {
    wait_for("the session", || {
        fake_claude_calls(out)
            .into_iter()
            .find(|c| c.mode == "session" && c.ready)
    })
}

/// `child`'s output once it has exited; kills it and fails if it still runs after `LONG`.
fn finish(mut child: Child) -> Output {
    let deadline = Instant::now() + LONG;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("tagteam was still running {LONG:?} later");
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The system clock in epoch ms, the clock the binary stamps with.
fn now_ms() -> i64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    i64::try_from(since.as_millis()).unwrap()
}

#[test]
fn run_records_a_launch_and_closes_it() {
    // Spec §6.1: the launch is open from before the spawn, holds the child's pid once it
    // exists, and is closed at the exit, the profile being in use no more.
    let home = Home::new();
    assert!(home.launches().is_empty(), "no profile existed to seed one");
    let out = home.out("launch");
    let hold = home.root().join("hold");
    let _release = Release(hold.clone());
    let child = home
        .run_a(&out)
        .env("FAKE_CLAUDE_HOLD", &hold)
        .spawn()
        .unwrap();
    let session = running(&out);
    let open = wait_for("the launch's pid", || {
        home.launches().into_iter().find(|l| l.pid.is_some())
    });
    assert_eq!(
        (open.account_id.as_str(), open.pid, open.ended_at),
        (home.a.as_str(), Some(session.pid), None),
        "{open:?}"
    );
    assert_eq!(home.launches().len(), 1);
    let released_at = now_ms();
    fs::write(&hold, b"").unwrap();
    let output = finish(child);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let closed = home.launches();
    assert_eq!(closed.len(), 1, "{closed:?}");
    assert_eq!(closed[0].started_at, open.started_at, "the same launch");
    let ended = closed[0].ended_at.expect("closed at the exit");
    assert!(
        ended >= released_at,
        "closed at the exit, after the child was let go at {released_at}: {closed:?}"
    );
}

#[test]
fn run_sights_the_child_session() {
    // Spec §6.1: while the child runs, `run` reads its record every 2 s and sights the session
    // it names in the profile's home, keyed by the record's `startedAt`. The child's exit,
    // stamped as soon as the wait returns, is where that sighting ends; the launch closes after
    // exit handling. The run is parked between the two (`after-exit`), so two reads of the
    // clock, as the child is let go and at the pause, bracket the stamp whatever anything took.
    let home = Home::new();
    let out = home.out("sighting");
    let hold = home.root().join("hold");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _release = Release(hold.clone());
    let _resume = Release(pause.join("resume"));
    let child = home
        .run_a(&out)
        .env("FAKE_CLAUDE_HOLD", &hold)
        .env("FAKE_CLAUDE_RECORD", "interactive")
        .env("TAGTEAM_TEST_PAUSE_AT", "after-exit")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause)
        .spawn()
        .unwrap();
    let session = running(&out);
    let session_id = format!("fake-{}", session.pid);
    let seen = wait_for("the poll's sighting", || {
        home.sightings()
            .into_iter()
            .find(|s| s.session_id == session_id)
    });
    let record: Value = serde_json::from_slice(
        &fs::read(
            home.profile()
                .join("sessions")
                .join(format!("{}.json", session.pid)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (seen.home.as_str(), seen.source, seen.first_at),
        (
            home.a.as_str(),
            SightingSource::Run,
            record["startedAt"].as_i64().unwrap()
        ),
        "{seen:?}"
    );
    let launches = home.launches();
    assert_eq!(launches.len(), 1, "{launches:?}");
    assert!(
        launches[0].started_at <= record["startedAt"].as_i64().unwrap(),
        "the launch is recorded before the spawn: {launches:?}, record {record}"
    );
    let released_at = now_ms();
    fs::write(&hold, b"").unwrap();
    wait_for("the run's exit handling", || {
        pause.join("paused").exists().then_some(())
    });
    let parked_at = now_ms();
    fs::write(pause.join("resume"), b"").unwrap();
    let output = finish(child);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let after: Vec<SightingRow> = home
        .sightings()
        .into_iter()
        .filter(|s| s.session_id == session_id)
        .collect();
    assert_eq!(after.len(), 1, "one execution, one sighting: {after:?}");
    assert_eq!(after[0].first_at, seen.first_at);
    let ended = home.launches()[0]
        .ended_at
        .expect("closed after exit handling");
    // A poll writes at most once a minute, so only `close_launch`, which sets the end to the
    // stamp exactly, can have put it in this bracket.
    assert!(
        (released_at..=parked_at).contains(&after[0].last_at),
        "the sighting ends at the exit stamp, between {released_at} and {parked_at}: {after:?}"
    );
    assert!(
        parked_at <= ended,
        "the launch closes after exit handling: ended {ended}, parked at {parked_at}"
    );
}

#[test]
fn a_launch_stays_open_while_the_profile_is_in_use() {
    // Spec §6.1: a profile still in use at the exit (here another reservation, as a second
    // `run` or a background session holds one) keeps the launch open; the next
    // `sight_sessions` that finds it idle closes it.
    let home = Home::new();
    let out = home.out("in-use");
    let hold = home.root().join("hold");
    let _release = Release(hold.clone());
    let child = home
        .run_a(&out)
        .env("FAKE_CLAUDE_HOLD", &hold)
        .spawn()
        .unwrap();
    running(&out);
    let _other = hold_launch(&home.profile());
    fs::write(&hold, b"").unwrap();
    let output = finish(child);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let launches = home.launches();
    assert_eq!(launches.len(), 1, "{launches:?}");
    assert_eq!(launches[0].ended_at, None, "{launches:?}");
}

#[test]
fn evidence_failures_never_change_the_exit_code() {
    // Spec §6.1: evidence is never worth a session. With the sightings table gone every poll
    // fails, and with the launches table gone too the launch cannot be recorded; either way
    // the run exits with the session's code. A failed poll goes to the log; a launch that
    // could not be recorded is a warning, before the session starts.
    let home = Home::new();
    let db = Env::for_test(home.root()).data_dir().join("tagteam.db");
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TABLE session_sightings")
        .unwrap();
    let out = home.out("no-sightings");
    let output = home
        .run_a(&out)
        .env("FAKE_CLAUDE_EXIT", "7")
        .env("FAKE_CLAUDE_RECORD", "interactive")
        .env("FAKE_CLAUDE_SLEEP", "2.5")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7), "{}", stderr(&output));
    assert!(
        !stderr(&output).contains("could not read the session's record"),
        "a failed poll goes to the log, never to claude's stderr: {}",
        stderr(&output)
    );
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TABLE run_launches")
        .unwrap();
    let out = home.out("no-launches");
    let output = home
        .run_a(&out)
        .env("FAKE_CLAUDE_EXIT", "7")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7), "{}", stderr(&output));
    assert!(
        stderr(&output).contains(
            "warning: the token report could not record this session's launch, so its usage may show as unattributed"
        ),
        "{}",
        stderr(&output)
    );
    let calls = fake_claude_calls(&out);
    assert!(
        calls
            .iter()
            .any(|c| c.mode == "session" && c.exit == Some(7)),
        "the session ran: {calls:?}"
    );
}

#[test]
fn a_spawn_that_fails_closes_its_launch_with_no_sighting() {
    // Spec §6.1: the launch is recorded before the spawn; when no child ever ran it closes
    // after exit handling, with nothing sighted.
    let home = Home::new();
    let out = home.out("gone");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _resume = Release(pause.join("resume"));
    let child = home
        .run_a(&out)
        .env("TAGTEAM_TEST_PAUSE_AT", "before-spawn")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause)
        .spawn()
        .unwrap();
    wait_for("the spawn", || pause.join("paused").exists().then_some(()));
    fs::remove_file(home.root().join("bin/claude")).unwrap();
    fs::write(pause.join("resume"), b"").unwrap();
    let output = finish(child);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let launches = home.launches();
    assert_eq!(launches.len(), 1, "{launches:?}");
    assert!(launches[0].ended_at.is_some(), "{launches:?}");
    assert_eq!(launches[0].pid, None, "no child ever ran: {launches:?}");
    assert!(home.sightings().is_empty(), "{:?}", home.sightings());
}
