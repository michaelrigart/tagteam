//! §1.1's timing targets through the real binary: `statusline` within 10 ms p95, and `list`
//! within 50 ms p95 when no usage fetch is due. Timings mean something only in an optimized
//! build on an idle machine, so this file exists only in release builds and its tests are
//! ignored by default:
//! `cargo test --release -p tagteam --features test-support --test perf -- --ignored`
//! (add `--nocapture` to see each measured p95).
#![cfg(all(feature = "test-support", not(debug_assertions)))]

mod common;

use std::fs::File;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use common::{
    bloat_claude_json, now_epoch_s, record_history, std_cmd, two_fresh_accounts, usage_window,
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
/// longer than a timing run takes.
fn nothing_due(root: &Path) {
    let (a, b) = two_fresh_accounts(root);
    let now = now_epoch_s();
    let windows = windows_at(now);
    for id in [&a, &b] {
        record_history(root, id, now, READINGS, SPACING_S, &windows);
    }
    bloat_claude_json(root, CLAUDE_JSON_BYTES);
}

/// The wall time of each of `RUNS` runs of `args`, after `WARM_UP` untimed ones, with every
/// endpoint pointed at `base`. `before(n)` runs untimed ahead of run `n`.
fn timings(root: &Path, base: &str, args: &[&str], before: &dyn Fn(usize)) -> Vec<Duration> {
    let run = |n: usize| {
        before(n);
        let started = Instant::now();
        let out = std_cmd(root)
            .env("TAGTEAM_TEST_API_BASE", base)
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

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn statusline_p95_is_within_10_ms() {
    let _one_at_a_time = serial();
    let d = tempfile::tempdir().unwrap();
    nothing_due(d.path());
    let server = MockServer::start();
    let runs = timings(d.path(), &server.base_url(), &["statusline"], &|_| {});
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
    let runs = timings(d.path(), &server.base_url(), &["statusline"], &touch);
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
    let runs = timings(d.path(), &server.base_url(), &["list"], &|_| {});
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
