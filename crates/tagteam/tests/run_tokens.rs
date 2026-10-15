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
    FakeCall, cmd, fake_claude, fake_claude_calls, hold_launch, path_with, std_cmd,
    two_fresh_accounts,
};
use serde_json::Value;
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::store::{LaunchRow, SightingRow, SightingSource, Store};
use tagteam_provider::{Env, PENDING_LAUNCH_DIR, PendingEntry, PendingLaunch, profile_path};

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

    /// The launches, or `None` while the table does not exist.
    fn launches_if_any(&self) -> Option<Vec<LaunchRow>> {
        self.store()
            .launches(&ProviderId::new(CLAUDE_CODE))
            .ok()
            .filter(|l| !l.is_empty())
    }

    /// The pending launch files in the profile (spec §6.1), parsed.
    fn pending(&self) -> Vec<PendingLaunch> {
        let entries = PendingLaunch::read_all(&self.profile()).present().unwrap();
        entries
            .into_iter()
            .map(|e| match e {
                PendingEntry::Launch(_, launch) => launch,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    /// `run_launches`'s schema, and `DROP TABLE` for it: the store then refuses every launch
    /// write, as one that cannot be written would.
    fn drop_launches(&self) -> String {
        let db = Env::for_test(self.root()).data_dir().join("tagteam.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        let schema: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'run_launches'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute_batch("DROP TABLE run_launches").unwrap();
        schema
    }

    fn restore_launches(&self, schema: &str) {
        let db = Env::for_test(self.root()).data_dir().join("tagteam.db");
        rusqlite::Connection::open(&db)
            .unwrap()
            .execute_batch(schema)
            .unwrap();
    }

    /// The log file's text (spec §14), empty when there is none.
    fn log(&self) -> String {
        let dir = Env::for_test(self.root()).state_dir();
        ["tagteam.log.1", "tagteam.log"]
            .iter()
            .map(|n| fs::read_to_string(dir.join(n)).unwrap_or_default())
            .collect()
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

/// The lines of `out`'s stderr that are warnings.
fn warnings(out: &Output) -> Vec<String> {
    stderr(out)
        .lines()
        .filter(|l| l.starts_with("warning:"))
        .map(str::to_owned)
        .collect()
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
    // Spec §6.1: while the child runs, `run` checks its record every 50 ms and writes every
    // 2 s: the session it names is sighted in the profile's home, in the execution of the
    // record's `startedAt`, from the read that first named it. The sighting ends at the last
    // check that confirmed it, never past the child's exit; the launch closes after exit
    // handling. The run is parked between the two (`after-exit`), so a read of the clock at the
    // pause bounds the end whatever anything took.
    let home = Home::new();
    let out = home.out("sighting");
    let hold = home.root().join("hold");
    let pause = home.root().join("pause");
    let confirmed = home.root().join("confirmed");
    fs::create_dir_all(&pause).unwrap();
    let _release = Release(hold.clone());
    let _resume = Release(pause.join("resume"));
    let child = home
        .run_a(&out)
        .env("FAKE_CLAUDE_HOLD", &hold)
        .env("FAKE_CLAUDE_RECORD", "interactive")
        .env("TAGTEAM_TEST_PAUSE_AT", "after-exit")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause)
        .env("TAGTEAM_TEST_CONFIRMED_AT", &confirmed)
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
    let started_at = record["startedAt"].as_i64().unwrap();
    assert_eq!(
        (seen.home.as_str(), seen.source, seen.started_at),
        (home.a.as_str(), SightingSource::Run, started_at),
        "{seen:?}"
    );
    assert!(
        seen.first_at >= started_at,
        "sighted from a read of the record, once it was written: {seen:?}"
    );
    let launches = home.launches();
    assert_eq!(launches.len(), 1, "{launches:?}");
    assert!(
        launches[0].started_at <= record["startedAt"].as_i64().unwrap(),
        "the launch is recorded before the spawn: {launches:?}, record {record}"
    );
    // The checks go on confirming the session while it runs; a write extends the sighting at
    // most once a minute, so only the close can carry its end past this first write's. The
    // child is let go once a check after that write has confirmed it.
    wait_for("a check after the first write", || {
        fs::read_to_string(&confirmed)
            .ok()?
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|&at| at > seen.last_at)
    });
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
    assert_eq!(after.len(), 1, "one session, one tenure: {after:?}");
    assert_eq!(after[0].first_at, seen.first_at);
    let ended = home.launches()[0]
        .ended_at
        .expect("closed after exit handling");
    assert!(
        seen.last_at < after[0].last_at && after[0].last_at <= parked_at,
        "the close ends the sighting at the last check that confirmed it, after the first \
         write's end {} and before the pause at {parked_at}: {after:?}",
        seen.last_at
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
    // the run exits with the session's code. A failed poll goes to the log. A launch the store
    // refuses is kept in a pending file, so nothing is warned of.
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
    home.drop_launches();
    let out = home.out("no-launches");
    let output = home
        .run_a(&out)
        .env("FAKE_CLAUDE_EXIT", "7")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7), "{}", stderr(&output));
    assert_eq!(
        warnings(&output),
        Vec::<String>::new(),
        "{}",
        stderr(&output)
    );
    assert_eq!(home.pending().len(), 1, "the launch is kept in its profile");
    let calls = fake_claude_calls(&out);
    assert!(
        calls
            .iter()
            .any(|c| c.mode == "session" && c.exit == Some(7)),
        "the session ran: {calls:?}"
    );
}

#[test]
fn a_launch_the_store_refused_throughout_is_kept_in_a_pending_file_until_a_scan_records_it() {
    // Spec §6.1, §6.2: the store refuses every launch write, from before the spawn to the
    // close. The launch is kept in the profile's pending file, open while the child runs and
    // closed at the close's time. Nothing is warned of, and the exit code is the child's. A
    // scan then records the launch from its planned start and removes the file.
    let home = Home::new();
    let schema = home.drop_launches();
    let out = home.out("refused");
    let planned_after = now_ms();
    let output = home
        .run_a(&out)
        .env("FAKE_CLAUDE_EXIT", "7")
        .env("FAKE_CLAUDE_SLEEP", "2.5")
        .env("TAGTEAM_LOG", "debug")
        .output()
        .unwrap();
    let closed_before = now_ms();
    assert_eq!(output.status.code(), Some(7), "{}", stderr(&output));
    assert_eq!(
        warnings(&output),
        Vec::<String>::new(),
        "{}",
        stderr(&output)
    );
    let log = home.log();
    assert!(
        log.contains("the store refused the session's launch; it is kept in a pending file"),
        "debug: {log}"
    );
    assert!(
        !log.contains("could not record the session's launch"),
        "no warning in the log either: {log}"
    );
    let files = PendingLaunch::read_all(&home.profile()).present().unwrap();
    let [PendingEntry::Launch(file, kept)] = files.as_slice() else {
        panic!("{files:?}");
    };
    assert!(
        file.path
            .starts_with(home.profile().join(PENDING_LAUNCH_DIR)),
        "{file:?}"
    );
    let kept = kept.clone();
    assert_eq!(
        (kept.provider.as_str(), kept.account_id.as_str()),
        (CLAUDE_CODE, home.a.as_str())
    );
    assert!(
        (planned_after..closed_before).contains(&kept.started_at_ms),
        "{kept:?}: planned between {planned_after} and {closed_before}"
    );
    let ended = kept.ended_at_ms.expect("the profile was idle at the close");
    assert!(
        kept.started_at_ms + 2_000 <= ended && ended <= closed_before,
        "closed after the child ran: {kept:?}"
    );

    home.restore_launches(&schema);
    cmd(home.root())
        .args(["tokens", "--json"])
        .assert()
        .success();
    let launches = home.launches();
    assert_eq!(launches.len(), 1, "{launches:?}");
    assert_eq!(
        (
            launches[0].account_id.as_str(),
            launches[0].started_at,
            launches[0].ended_at
        ),
        (home.a.as_str(), kept.started_at_ms, Some(ended))
    );
    assert!(home.pending().is_empty(), "the scan removed the file");
}

#[test]
fn a_launch_neither_the_store_nor_the_profile_keeps_is_warned_of_twice() {
    // Spec §6.1, §7.4: the store refuses the launch and the profile cannot hold the pending
    // file (a file stands where its directory goes): one warning before the spawn and one at
    // the close, once the child has run.
    let home = Home::new();
    home.drop_launches();
    let out = home.out("lost");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _resume = Release(pause.join("resume"));
    let child = home
        .run_a(&out)
        .env("FAKE_CLAUDE_EXIT", "7")
        .env("TAGTEAM_TEST_PAUSE_AT", "before-spawn")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause)
        .spawn()
        .unwrap();
    wait_for("the spawn", || pause.join("paused").exists().then_some(()));
    fs::write(home.profile().join(PENDING_LAUNCH_DIR), b"").unwrap();
    fs::write(pause.join("resume"), b"").unwrap();
    let output = finish(child);
    assert_eq!(output.status.code(), Some(7), "{}", stderr(&output));
    assert!(
        home.log().contains(
            "WARN tagteam::run: could not record the session's launch for the token report"
        ),
        "warned in the log when nothing holds the launch: {}",
        home.log()
    );
    let w = warnings(&output);
    assert_eq!(w.len(), 2, "{}", stderr(&output));
    assert!(
        w[0].starts_with(
            "warning: the token report could not record this session's launch, in the store (store error: no such table: run_launches) or in its profile ("
        ) && w[0].ends_with(
            "). It will try again while the session runs. If it never can, this session's usage may be credited to the default login's account."
        ),
        "{}",
        w[0]
    );
    assert_eq!(
        w[1],
        "warning: the token report could not record this session's launch, in the store or in its profile, so its usage may be credited to the default login's account"
    );
}

#[test]
fn a_launch_that_failed_to_record_is_recorded_later_with_its_original_start() {
    // Spec §6.1: a launch that could not be recorded before the spawn is retried by the poller
    // while the child runs. It keeps the start planned before the spawn, so attribution, which
    // reads launches at query time, covers the whole session. The table is gone when the run
    // plans its launch, which is kept in a pending file meanwhile; the poller parks right after
    // its first retry failed (`launch-retry-failed`), the table comes back, and a later retry
    // records the launch and removes the file.
    let home = Home::new();
    let db = Env::for_test(home.root()).data_dir().join("tagteam.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    let schema: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'run_launches'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute_batch("DROP TABLE run_launches").unwrap();
    let out = home.out("late-record");
    let hold = home.root().join("hold");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _release = Release(hold.clone());
    let _resume = Release(pause.join("resume"));
    let planned_after = now_ms();
    let child = home
        .run_a(&out)
        .env("FAKE_CLAUDE_HOLD", &hold)
        .env("TAGTEAM_TEST_PAUSE_AT", "launch-retry-failed")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause)
        .spawn()
        .unwrap();
    running(&out);
    wait_for("the first retry to fail", || {
        pause.join("paused").exists().then_some(())
    });
    assert!(
        home.launches_if_any().is_none(),
        "the first retry recorded nothing"
    );
    let kept = home.pending();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0].ended_at_ms, None, "open while the child runs");
    let restored_at = now_ms();
    conn.execute_batch(&schema).unwrap();
    fs::write(pause.join("resume"), b"").unwrap();
    let late = wait_for("a later retry to record the launch", || {
        home.launches().into_iter().find(|l| l.pid.is_some())
    });
    assert!(
        (planned_after..restored_at).contains(&late.started_at),
        "the launch keeps the start planned before the spawn, not the retry's: {late:?}, planned after {planned_after}, restored at {restored_at}"
    );
    assert_eq!(late.account_id, home.a);
    assert_eq!(kept[0].started_at_ms, late.started_at, "the same launch");
    // The poller removes the file while the child is still held; the close would remove it too.
    wait_for("the poller to remove the pending file", || {
        home.pending().is_empty().then_some(())
    });
    fs::write(&hold, b"").unwrap();
    let output = finish(child);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        warnings(&output),
        Vec::<String>::new(),
        "the profile kept the launch meanwhile: {}",
        stderr(&output)
    );
    assert!(home.pending().is_empty(), "the close leaves none");
    let closed = home.launches();
    assert_eq!(closed.len(), 1, "{closed:?}");
    assert_eq!(closed[0].started_at, late.started_at);
    assert!(
        closed[0].ended_at.is_some(),
        "closed at the exit: {closed:?}"
    );
}

#[test]
fn a_spawn_that_fails_with_an_unrecorded_launch_keeps_it_in_a_pending_file() {
    // No session ran, so no usage can be misplaced: nothing is warned of, and the launch is
    // kept, closed, for the next scan.
    let home = Home::new();
    home.drop_launches();
    let out = home.out("gone-unrecorded");
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
    assert_eq!(
        warnings(&output),
        Vec::<String>::new(),
        "{}",
        stderr(&output)
    );
    let kept = home.pending();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(kept[0].ended_at_ms.is_some(), "{kept:?}");
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

#[test]
fn the_poller_writes_the_pending_file_late_when_the_first_write_failed() {
    // Spec §6.1: the store refuses the launch and the profile cannot take the pending file at
    // first (a file stands where its directory goes), so the pre-spawn warning is printed.
    // While the child runs the obstacle goes; the poller's next retry writes the file, open,
    // and the close sets its end. No warning is printed at the close.
    let home = Home::new();
    home.drop_launches();
    let out = home.out("late-file");
    let hold = home.root().join("hold");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _release = Release(hold.clone());
    let _resume = Release(pause.join("resume"));
    let child = home
        .run_a(&out)
        .env("FAKE_CLAUDE_HOLD", &hold)
        .env("TAGTEAM_TEST_PAUSE_AT", "before-spawn")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause)
        .spawn()
        .unwrap();
    wait_for("the spawn", || pause.join("paused").exists().then_some(()));
    let obstacle = home.profile().join(PENDING_LAUNCH_DIR);
    fs::write(&obstacle, b"").unwrap();
    fs::write(pause.join("resume"), b"").unwrap();
    running(&out);
    fs::remove_file(&obstacle).unwrap();
    let late = wait_for("the poller's pending file", || {
        home.pending().into_iter().next()
    });
    assert_eq!(
        late.ended_at_ms, None,
        "open while the child runs: {late:?}"
    );
    fs::write(&hold, b"").unwrap();
    let output = finish(child);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let w = warnings(&output);
    assert_eq!(
        w.len(),
        1,
        "the pre-spawn warning alone: {}",
        stderr(&output)
    );
    assert!(w[0].contains("or in its profile ("), "{}", w[0]);
    let kept = home.pending();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0].started_at_ms, late.started_at_ms);
    assert!(
        kept[0].ended_at_ms.is_some(),
        "closed at the exit: {kept:?}"
    );
}

#[test]
fn two_runs_on_one_profile_keep_two_pending_files_and_a_scan_records_both() {
    // Spec §6.1: a second `run` joins a profile in use. Each keeps a file of its own, named by
    // its start and its pid. A scan then records both, and closes whichever is still open once
    // the profile is idle.
    let home = Home::new();
    let schema = home.drop_launches();
    let hold = home.root().join("hold");
    let _release = Release(hold.clone());
    let out_1 = home.out("join-1");
    let first = home
        .run_a(&out_1)
        .env("FAKE_CLAUDE_HOLD", &hold)
        .spawn()
        .unwrap();
    running(&out_1);
    let out_2 = home.out("join-2");
    let second = home
        .run_a(&out_2)
        .env("FAKE_CLAUDE_HOLD", &hold)
        .spawn()
        .unwrap();
    running(&out_2);
    let both = wait_for("both pending files", || {
        Some(home.pending()).filter(|p| p.len() == 2)
    });
    assert_ne!(both[0].started_at_ms, both[1].started_at_ms);
    fs::write(&hold, b"").unwrap();
    for child in [first, second] {
        let output = finish(child);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        assert_eq!(
            warnings(&output),
            Vec::<String>::new(),
            "{}",
            stderr(&output)
        );
    }
    assert_eq!(home.pending().len(), 2);

    home.restore_launches(&schema);
    cmd(home.root())
        .args(["tokens", "--json"])
        .assert()
        .success();
    let mut stored: Vec<i64> = home.launches().iter().map(|l| l.started_at).collect();
    stored.sort_unstable();
    let mut kept: Vec<i64> = both.iter().map(|l| l.started_at_ms).collect();
    kept.sort_unstable();
    assert_eq!(stored, kept);
    assert!(
        home.launches().iter().all(|l| l.ended_at.is_some()),
        "{:?}",
        home.launches()
    );
    assert!(home.pending().is_empty());
}
