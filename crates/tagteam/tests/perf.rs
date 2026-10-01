//! §1.1's timing targets through the real binary: `statusline` within 10 ms p95, and `list`
//! within 50 ms p95 when no usage fetch is due. Timings mean something only in an optimized
//! build on an idle machine, so this file exists only in release builds and its tests are
//! ignored by default:
//! `cargo test --release -p tagteam --features test-support --test perf -- --ignored`
//! (add `--nocapture` to see each measured p95).
#![cfg(all(feature = "test-support", not(debug_assertions)))]

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{now_epoch_s, record_reading, std_cmd, two_fresh_accounts, usage_window};
use tagteam_core::WindowKind;
use tagteam_provider::mock_server::MockServer;

const RUNS: usize = 50;
const WARM_UP: usize = 3;

/// Two accounts, `b` live, each with a reading taken just now. Nothing is due for 180 s (§8.3's
/// on-demand rule), far longer than a timing run takes.
fn nothing_due(root: &Path) {
    let (a, b) = two_fresh_accounts(root);
    let now = now_epoch_s();
    for id in [&a, &b] {
        record_reading(
            root,
            id,
            now,
            &[
                usage_window(
                    "5h",
                    "5h",
                    WindowKind::Short,
                    9.0,
                    Some(now + 9_600),
                    Some(18_000),
                ),
                usage_window(
                    "7d",
                    "7d",
                    WindowKind::Long,
                    77.0,
                    Some(now + 300_000),
                    Some(604_800),
                ),
            ],
        );
    }
}

/// The wall time of each of `RUNS` runs of `args`, after `WARM_UP` untimed ones, with every
/// endpoint pointed at `base`.
fn timings(root: &Path, base: &str, args: &[&str]) -> Vec<Duration> {
    let run = || {
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
    for _ in 0..WARM_UP {
        run();
    }
    (0..RUNS).map(|_| run()).collect()
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
    let d = tempfile::tempdir().unwrap();
    nothing_due(d.path());
    let server = MockServer::start();
    let runs = timings(d.path(), &server.base_url(), &["statusline"]);
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
fn list_p95_is_within_50_ms_when_nothing_is_due() {
    let d = tempfile::tempdir().unwrap();
    nothing_due(d.path());
    let server = MockServer::start();
    let runs = timings(d.path(), &server.base_url(), &["list"]);
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
