//! §1.1's timing targets through the real binary: `statusline` within 10 ms p95, `list` within
//! 50 ms p95 when no usage fetch is due, alone and while a daemon runs against the same store
//! (D1.1 criterion 3, D12), and the TUI's first frame within 50 ms p95 under the same
//! conditions (TUI spec §1.1), with the token index caught up or not. Timings mean something
//! only in an optimized build on an idle machine, so this file exists only in release builds
//! and its tests are ignored by default. The TUI's opens a pseudo-terminal, so run them
//! outside a sandbox:
//! `cargo test --release -p tagteam --features test-support --test perf -- --ignored`
//! (add `--nocapture` to see each measured p95).
#![cfg(all(feature = "test-support", not(debug_assertions)))]

mod common;

use std::fs::File;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use common::pty::{Pty, Screen};
use common::{
    Guarded, Turn, bloat_claude_json, cc_profile, hold_launch, now_epoch_s, record_history,
    short_tempdir, std_cmd, turn, two_fresh_accounts, usage_window, write_transcript,
};
use tagteam_core::{Window, WindowKind};
use tagteam_provider::Env;
use tagteam_provider::mock_server::MockServer;

const RUNS: usize = 50;
const WARM_UP: usize = 3;
/// A reading about every 3 minutes for 48 hours: what the collector leaves behind, and what
/// §8.7's pace reads (the hourly budget allows 20 requests in 3 660 s, so 183 s is the closest).
const SPACING_S: i64 = 183;
const READINGS: usize = (48 * 3_600 / SPACING_S) as usize;
/// What a machine that has run Claude Code for months has in `~/.claude.json`.
const CLAUDE_JSON_BYTES: usize = 400 * 1024;
const HOUR: i64 = 3_600;

/// The tests measure wall time, so they run one at a time: two binaries running at once would
/// each measure the other.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The four windows Claude Code reports, as a reading taken at `at` with its last at `now`: 5h
/// on its own 5-hour instances, 7d and Fable rising across the 48 hours, spend at zero.
fn windows_at(now: i64) -> impl Fn(i64) -> Vec<Window> {
    let (end5, end7) = (now + 9_600, now + 300_000);
    let start = now - 48 * HOUR;
    move |at| {
        let reset5 = end5 - 18_000 * ((end5 - at) / 18_000);
        let into5 = 1.0 - (reset5 - at) as f64 / 18_000.0;
        let across = (at - start) as f64 / (48 * HOUR) as f64;
        vec![
            usage_window(
                "5h",
                "5h",
                WindowKind::Short,
                (into5 * 20.0).round(),
                Some(reset5),
                Some(18_000),
            ),
            usage_window(
                "7d",
                "7d",
                WindowKind::Long,
                (20.0 + 57.0 * across).round(),
                Some(end7),
                Some(604_800),
            ),
            usage_window(
                "scoped:Fable",
                "Fable",
                WindowKind::Scoped,
                (3.0 * across).round(),
                Some(end7),
                Some(604_800),
            ),
            usage_window("spend", "spend", WindowKind::Spend, 0.0, None, None),
        ]
    }
}

/// Two accounts, `b` live, each with 48 hours of readings, the last taken just now, and a
/// `~/.claude.json` of a few hundred KB. Nothing is due for 180 s (§8.3's on-demand rule), far
/// longer than a timing run takes. Returns the two ids, `a` first.
fn nothing_due(root: &Path) -> (String, String) {
    let (a, b) = two_fresh_accounts(root);
    let now = now_epoch_s();
    let windows = windows_at(now);
    for id in [&a, &b] {
        record_history(root, id, now, READINGS, SPACING_S, &windows);
    }
    bloat_claude_json(root, CLAUDE_JSON_BYTES);
    (a, b)
}

/// `sessions` transcripts of `turns` Claude Code responses each, a minute apart, back from now:
/// with the defaults below, some 7 MB that a cold token index reads.
fn transcripts(root: &Path, sessions: usize, turns: usize) {
    let now_ms = now_epoch_s() * 1000;
    for s in 0..sessions {
        let all: Vec<Turn> = (0..turns)
            .map(|t| {
                let ago = (s * turns + t) as i64 * 60_000;
                turn("claude-opus-5-5", now_ms - ago, [100, 20, 30, 40, 1_000])
            })
            .collect();
        write_transcript(root, &format!("s-{s}"), &all);
    }
}

/// The wall time of each of `RUNS` runs of `args`, after `WARM_UP` untimed ones, with every
/// endpoint pointed at `base` and `envs` set. `before(n)` runs untimed ahead of run `n`.
fn timings(
    root: &Path,
    base: &str,
    args: &[&str],
    envs: &[(&str, &str)],
    before: &dyn Fn(usize),
) -> Vec<Duration> {
    let run = |n: usize| {
        before(n);
        let started = Instant::now();
        let out = std_cmd(root)
            .env("TAGTEAM_TEST_API_BASE", base)
            .envs(envs.iter().copied())
            .args(args)
            .output()
            .unwrap();
        let took = started.elapsed();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        took
    };
    for n in 0..WARM_UP {
        run(n);
    }
    (0..RUNS).map(|n| run(WARM_UP + n)).collect()
}

/// The 95th percentile, by nearest rank.
fn p95(runs: &[Duration]) -> Duration {
    let mut sorted = runs.to_vec();
    sorted.sort();
    sorted[(sorted.len() * 95).div_ceil(100) - 1]
}

/// The ticks a daemon's `--json` output has finished: each ends in one `switch` or `no-switch`
/// event (§11.4).
fn ticks(out: &str) -> usize {
    out.matches(r#""event":"no-switch""#).count() + out.matches(r#""event":"switch""#).count()
}

/// Untimed, before each run: 350 ms, so the 53 runs span about 19 s, more than the longest tick
/// delay (15 s × 1.1), and at least one tick lands among them. It spins rather than sleeps: an
/// idle core clocks down, and a run started after 350 ms asleep measures the wake-up (about
/// 19 ms for `statusline` with no daemon at all), not the binary.
fn spread(_: usize) {
    let until = Instant::now() + Duration::from_millis(350);
    while Instant::now() < until {
        std::hint::spin_loop();
    }
}

/// D12, D14: a daemon on the fixture while the runs are timed. It is in `auto` mode, with the
/// shortest interval §6.4 allows (15 s), so it ticks while the runs go on. Its scheduler also
/// wakes every second, and its store watch reads the store every 2 s. Notifications are off,
/// so it starts nothing. Returns once its first tick is in.
fn start_daemon(root: &Path, base: &str) -> Guarded {
    for (key, value) in [
        ("autoswitch.interval_seconds", "15"),
        ("notify.enabled", "false"),
    ] {
        let out = std_cmd(root)
            .args(["config", "set", key, value])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "config set {key}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let mut cmd = std_cmd(root);
    cmd.env("TAGTEAM_TEST_API_BASE", base)
        .args(["daemon", "--json"])
        .stdin(Stdio::null());
    let mut daemon = Guarded::spawn(cmd);
    daemon.wait_for(Duration::from_secs(30), "the daemon's first tick", |out| {
        ticks(out) >= 1
    });
    daemon
}

/// SIGTERM, and the daemon's exit 0 (D4.6).
fn stop(daemon: Guarded) {
    daemon.signal(libc::SIGTERM);
    let out = daemon.finish(Duration::from_secs(10));
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn statusline_p95_is_within_10_ms() {
    let _one_at_a_time = serial();
    let d = tempfile::tempdir().unwrap();
    nothing_due(d.path());
    let server = MockServer::start();
    let runs = timings(d.path(), &server.base_url(), &["statusline"], &[], &|_| {});
    let p = p95(&runs);
    eprintln!("statusline p95 {p:?} over {RUNS} runs");
    assert_eq!(server.requests().len(), 0, "statusline sent a request");
    assert!(
        p <= Duration::from_millis(10),
        "statusline p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn statusline_p95_on_a_cache_miss_is_within_10_ms() {
    // The identity cache is keyed on the file's mtime and size (§13.5): a new mtime before each
    // run makes it parse the whole file and write the cache, as after every Claude Code write.
    let _one_at_a_time = serial();
    let d = tempfile::tempdir().unwrap();
    nothing_due(d.path());
    let path = Env::for_test(d.path()).home.join(".claude.json");
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size >= CLAUDE_JSON_BYTES as u64, "{size} bytes");
    let base = std::fs::metadata(&path).unwrap().modified().unwrap();
    let touch = |n: usize| {
        let at: SystemTime = base + Duration::from_secs(1 + n as u64);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    };
    let server = MockServer::start();
    let runs = timings(d.path(), &server.base_url(), &["statusline"], &[], &touch);
    let p = p95(&runs);
    eprintln!("statusline p95 on a cache miss {p:?} over {RUNS} runs ({size} byte claude.json)");
    assert_eq!(server.requests().len(), 0, "statusline sent a request");
    assert!(
        p <= Duration::from_millis(10),
        "statusline p95 on a cache miss {p:?} over {RUNS} runs: {runs:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn list_p95_is_within_50_ms_when_nothing_is_due() {
    let _one_at_a_time = serial();
    let d = tempfile::tempdir().unwrap();
    nothing_due(d.path());
    let server = MockServer::start();
    let runs = timings(d.path(), &server.base_url(), &["list"], &[], &|_| {});
    let p = p95(&runs);
    eprintln!("list p95 {p:?} over {RUNS} runs");
    assert_eq!(
        server.requests().len(),
        0,
        "a fetch was due, so this measured the wrong path"
    );
    assert!(
        p <= Duration::from_millis(50),
        "list p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn tui_first_frame_p95_is_within_50_ms_when_nothing_is_due() {
    // TUI spec §1.1 criterion 2, §11: from the spawn to the arrival of the bytes that first put
    // the accounts table on the screen, through a pty, under `list`'s conditions (core §1.1).
    // The first frame never waits on a fetch (§6.1), and nothing is due, so the collection the
    // TUI starts after it sends nothing either.
    let _one_at_a_time = serial();
    let d = tempfile::tempdir().unwrap();
    nothing_due(d.path());
    let server = MockServer::start();
    let shown = |s: &Screen| s.on_alt() && s.contains("a@x.co");
    let run = || {
        let mut c = std_cmd(d.path());
        c.env("TAGTEAM_TEST_API_BASE", server.base_url()).arg("tui");
        let mut tui = Pty::spawn(c, true);
        tui.wait_for(Duration::from_secs(10), "the accounts table", shown);
        let took = tui.first_shown(shown).unwrap() - tui.started;
        tui.type_keys(b"q");
        let f = tui.finish(Duration::from_secs(10));
        assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
        took
    };
    for _ in 0..WARM_UP {
        run();
    }
    let runs: Vec<Duration> = (0..RUNS).map(|_| run()).collect();
    let p = p95(&runs);
    eprintln!("tui first frame p95 {p:?} over {RUNS} runs");
    assert_eq!(
        server.requests().len(),
        0,
        "a fetch was due, so this measured the wrong path"
    );
    assert!(
        p <= Duration::from_millis(50),
        "tui first frame p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn tui_first_frame_p95_is_within_50_ms_while_the_index_catches_up() {
    // TUI spec §1.1 criterion 2 with a cold token index: its catch-up starts after the first
    // frame (B5), so the transcripts it reads never delay it. Before each run the index forgets
    // every file it has read, so each run catches up anew; `q` stops the catch-up (§9).
    let _one_at_a_time = serial();
    let d = tempfile::tempdir().unwrap();
    nothing_due(d.path());
    transcripts(d.path(), 50, 200);
    let db = Env::for_test(d.path()).data_dir().join("tagteam.db");
    let server = MockServer::start();
    let shown = |s: &Screen| s.on_alt() && s.contains("a@x.co");
    let run = || {
        let forget = rusqlite::Connection::open(&db).unwrap();
        forget.execute("DELETE FROM token_files", []).unwrap();
        drop(forget);
        let mut c = std_cmd(d.path());
        c.env("TAGTEAM_TEST_API_BASE", server.base_url()).arg("tui");
        let mut tui = Pty::spawn(c, true);
        tui.wait_for(Duration::from_secs(10), "the accounts table", shown);
        let took = tui.first_shown(shown).unwrap() - tui.started;
        tui.type_keys(b"q");
        let f = tui.finish(Duration::from_secs(10));
        assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
        took
    };
    for _ in 0..WARM_UP {
        run();
    }
    let runs: Vec<Duration> = (0..RUNS).map(|_| run()).collect();
    let p = p95(&runs);
    eprintln!("tui first frame while the index catches up p95 {p:?} over {RUNS} runs");
    assert_eq!(server.requests().len(), 0, "a fetch was due");
    let read: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT count(*) FROM token_files", [], |r| r.get(0))
        .unwrap();
    assert!(read > 0, "the catch-up never read a transcript");
    assert!(
        p <= Duration::from_millis(50),
        "tui first frame while the index catches up p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn statusline_in_a_run_shell_p95_is_within_10_ms() {
    // §12.8, §13.5: in a run shell the line names the marker's account. The marker is one
    // small file and the 400 KB `~/.claude.json` is never parsed. The line computes no session
    // state (Decision 17), so the reservation held below is not part of the timed cost. It
    // makes the timed run a real session's, and would put a regression that made `statusline`
    // read the profile's session state (a directory listing and a non-blocking `flock`) inside
    // the budget.
    let _one_at_a_time = serial();
    let d = tempfile::tempdir().unwrap();
    let (a, _b) = nothing_due(d.path());
    let (profile, shell) = cc_profile(d.path(), &a);
    let _session = hold_launch(&profile);
    let server = MockServer::start();
    let envs = [("CLAUDE_CONFIG_DIR", shell.as_str())];
    // The timed path is the run-shell one: the line is the session's account, a, not b.
    let out = std_cmd(d.path())
        .env("TAGTEAM_TEST_API_BASE", server.base_url())
        .envs(envs)
        .args(["statusline", "--no-color"])
        .output()
        .unwrap();
    let line = String::from_utf8(out.stdout).unwrap();
    assert!(line.starts_with("a · "), "{line:?}");
    let runs = timings(
        d.path(),
        &server.base_url(),
        &["statusline"],
        &envs,
        &|_| {},
    );
    let p = p95(&runs);
    eprintln!("statusline in a run shell p95 {p:?} over {RUNS} runs");
    assert_eq!(server.requests().len(), 0, "statusline sent a request");
    assert!(
        p <= Duration::from_millis(10),
        "statusline in a run shell p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}

/// The session id the timed `statusline` runs carry, as Claude Code passes it (token report
/// spec §6.3).
const SESSION: &str = "perf-session";

/// Claude Code's session record for this test process, every timed run's parent, in the
/// default home: the ancestor a sighting is keyed by (token report spec §6.3).
fn record_this_process(root: &Path) {
    let dir = Env::for_test(root).home.join(".claude/sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let pid = std::process::id();
    let record = format!(
        "{{\"pid\":{pid},\"sessionId\":\"{SESSION}\",\"cwd\":\"/w\",\"startedAt\":{},\"kind\":\"interactive\"}}",
        now_epoch_s() * 1000
    );
    std::fs::write(dir.join(format!("{pid}.json")), record).unwrap();
}

/// How many sightings the store holds, and the latest end of one.
fn sightings(root: &Path) -> (i64, Option<i64>) {
    rusqlite::Connection::open(Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .query_row(
            "SELECT count(*), max(last_at) FROM session_sightings",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn statusline_with_a_fresh_sighting_p95_is_within_10_ms() {
    // Token report spec §6.3, §15 "Budgets": the first warm-up run records the session, and
    // every timed run finds that sighting less than a minute old: one indexed lookup.
    let _one_at_a_time = serial();
    let d = tempfile::tempdir().unwrap();
    nothing_due(d.path());
    record_this_process(d.path());
    let server = MockServer::start();
    let envs = [("CLAUDE_CODE_SESSION_ID", SESSION)];
    let runs = timings(
        d.path(),
        &server.base_url(),
        &["statusline"],
        &envs,
        &|_| {},
    );
    let p = p95(&runs);
    eprintln!("statusline with a fresh sighting p95 {p:?} over {RUNS} runs");
    assert_eq!(sightings(d.path()).0, 1, "the session was recorded");
    assert_eq!(server.requests().len(), 0, "statusline sent a request");
    assert!(
        p <= Duration::from_millis(10),
        "statusline with a fresh sighting p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn statusline_due_a_sighting_extension_p95_is_within_10_ms() {
    // Before each run the sighting is moved a minute and more into the past, so every timed
    // run extends it: the lookup, the walk to the ancestor with a record, and one write.
    let _one_at_a_time = serial();
    let d = tempfile::tempdir().unwrap();
    nothing_due(d.path());
    record_this_process(d.path());
    let server = MockServer::start();
    let envs = [("CLAUDE_CODE_SESSION_ID", SESSION)];
    let db = Env::for_test(d.path()).data_dir().join("tagteam.db");
    let age = |_: usize| {
        rusqlite::Connection::open(&db)
            .unwrap()
            .execute(
                "UPDATE session_sightings SET first_at = first_at - 120000, \
                 last_at = last_at - 120000",
                [],
            )
            .unwrap();
    };
    let started = now_epoch_s() * 1000;
    let runs = timings(d.path(), &server.base_url(), &["statusline"], &envs, &age);
    let p = p95(&runs);
    eprintln!("statusline due a sighting extension p95 {p:?} over {RUNS} runs");
    let (count, last) = sightings(d.path());
    assert_eq!(count, 1, "one execution, one sighting");
    assert!(
        last.is_some_and(|at| at >= started),
        "the last run extended the sighting: {last:?}"
    );
    assert_eq!(server.requests().len(), 0, "statusline sent a request");
    assert!(
        p <= Duration::from_millis(10),
        "statusline due a sighting extension p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn statusline_p95_is_within_10_ms_while_a_daemon_runs() {
    let _one_at_a_time = serial();
    let d = short_tempdir();
    nothing_due(d.path());
    let server = MockServer::start();
    let daemon = start_daemon(d.path(), &server.base_url());
    let before = ticks(&daemon.stdout());
    let runs = timings(d.path(), &server.base_url(), &["statusline"], &[], &spread);
    let ticked = ticks(&daemon.stdout()) - before;
    stop(daemon);
    let p = p95(&runs);
    eprintln!(
        "statusline p95 while a daemon runs {p:?} over {RUNS} runs, {ticked} ticks among them"
    );
    assert!(
        ticked >= 1,
        "the daemon never ticked while the runs went on"
    );
    assert_eq!(server.requests().len(), 0, "a request was sent");
    assert!(
        p <= Duration::from_millis(10),
        "statusline p95 while a daemon runs {p:?} over {RUNS} runs: {runs:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn list_p95_is_within_50_ms_when_nothing_is_due_while_a_daemon_runs() {
    let _one_at_a_time = serial();
    let d = short_tempdir();
    nothing_due(d.path());
    let server = MockServer::start();
    let daemon = start_daemon(d.path(), &server.base_url());
    let before = ticks(&daemon.stdout());
    let runs = timings(d.path(), &server.base_url(), &["list"], &[], &spread);
    let ticked = ticks(&daemon.stdout()) - before;
    stop(daemon);
    let p = p95(&runs);
    eprintln!("list p95 while a daemon runs {p:?} over {RUNS} runs, {ticked} ticks among them");
    assert!(
        ticked >= 1,
        "the daemon never ticked while the runs went on"
    );
    assert_eq!(
        server.requests().len(),
        0,
        "a fetch was due, so this measured the wrong path"
    );
    assert!(
        p <= Duration::from_millis(50),
        "list p95 while a daemon runs {p:?} over {RUNS} runs: {runs:?}"
    );
}
