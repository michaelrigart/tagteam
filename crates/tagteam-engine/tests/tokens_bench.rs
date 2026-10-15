//! Token report spec §1.1's budgets for the index and the report, over synthetic transcripts in
//! Claude Code's format (spec §15 "Budgets"): a cold index of 500 MB, scaled from 10 s for this
//! machine's 3.42 GB; a refresh that finds nothing new, 100 ms; and a 30-day report of a
//! month's responses, 200 ms. Timings mean something only in an optimized build on an idle
//! machine, so this file exists only in release builds and its tests are ignored by default:
//! `cargo test --release -p tagteam-engine --test tokens_bench -- --ignored --nocapture`
//! (`--nocapture` prints each timing).
#![cfg(not(debug_assertions))]

mod common;

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use common::Fx;
use tagteam_core::time::format_iso8601;
use tagteam_engine::store::{SightingRow, SightingSource};
use tagteam_engine::tokens::{
    IndexReport, RefreshOutcome, TokenGrouping, TokenPeriod, TokenQuery, Wait,
};

const MIB: u64 = 1 << 20;
const DAY_MS: i64 = 86_400_000;
/// §1.1: 10 s for 3.42 GB is 1.46 s for 500 MB.
const COLD: Duration = Duration::from_millis(1_500);
const NOTHING_NEW: Duration = Duration::from_millis(100);
const QUERY: Duration = Duration::from_millis(200);
/// Spec §4 "Size": this machine's responses in 30 days, what a 30-day report reads.
const MONTH_RESPONSES: u64 = 127_000;
const MODELS: [&str; 4] = [
    "claude-opus-5-5",
    "claude-sonnet-5-5",
    "claude-haiku-4-5-20251001",
    "claude-fable-5-1",
];

/// The tests measure wall time, so they run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A deterministic generator (xorshift64*), so every run builds the same tree.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// What a generated tree holds.
#[derive(Debug, Default)]
struct Tree {
    files: u64,
    bytes: u64,
    responses: u64,
}

/// `at_ms` as Claude Code stamps a line: ISO 8601 UTC, to the millisecond.
fn timestamp(at_ms: i64) -> String {
    let seconds = format_iso8601(at_ms.div_euclid(1000));
    format!(
        "{}.{:03}Z",
        seconds.trim_end_matches('Z'),
        at_ms.rem_euclid(1000)
    )
}

/// The session id of generated transcript `file`; twenty share a project.
fn session_of(file: u64) -> String {
    format!("{:08x}-0000-4000-8000-{file:012x}", file / 20)
}

/// One generated transcript, as Claude Code lays them out (spec §2.1): a session's, or for one
/// file in ten a subagent's, under `subagents/`, whose responses never get a final line
/// (spec §2.3).
struct Transcript<'a> {
    out: BufWriter<File>,
    file: u64,
    session: String,
    subagent: bool,
    pad: &'a str,
    responses: u64,
    bytes: u64,
}

impl<'a> Transcript<'a> {
    fn create(config_home: &Path, file: u64, pad: &'a str) -> Self {
        let session = session_of(file);
        let subagent = file % 10 == 9;
        let dir = config_home.join(format!("projects/-work-p{}", file / 20));
        let path = if subagent {
            dir.join(&session)
                .join("subagents")
                .join(format!("agent-{file:016x}.jsonl"))
        } else {
            dir.join(format!("{session}.jsonl"))
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        Transcript {
            out: BufWriter::new(File::create(&path).unwrap()),
            file,
            session,
            subagent,
            pad,
            responses: 0,
            bytes: 0,
        }
    }

    /// One response at `at_ms`: a user line with `user_len` bytes of text, then its 1 to 9
    /// assistant lines, one per content block with up to `text_len` bytes each, all with the
    /// same ids and growing output (spec §2.2). The last is final unless this is a subagent's.
    fn response(&mut self, at_ms: i64, rng: &mut Rng, user_len: usize, text_len: usize) {
        let (file, n, session) = (self.file, self.responses, self.session.as_str());
        let ts = timestamp(at_ms);
        let mut lines = format!(
            "{{\"type\":\"user\",\"sessionId\":\"{session}\",\"timestamp\":\"{ts}\",\"message\":{{\"role\":\"user\",\"content\":\"{}\"}}}}\n",
            &self.pad[..user_len]
        );
        let model = MODELS[rng.below(MODELS.len() as u64) as usize];
        let blocks = if rng.below(10) < 2 {
            2 + rng.below(8)
        } else {
            1
        };
        let (input, write, read) = (rng.below(5_000), rng.below(20_000), rng.below(200_000));
        for block in 0..blocks {
            let stop = if block + 1 == blocks && !self.subagent {
                "\"end_turn\""
            } else {
                "null"
            };
            let text = &self.pad[..rng.below(text_len as u64 + 1) as usize];
            lines.push_str(&format!(
                "{{\"parentUuid\":null,\"isSidechain\":{side},\"type\":\"assistant\",\"requestId\":\"req_{file}_{n}\",\"sessionId\":\"{session}\",\"timestamp\":\"{ts}\",\"cwd\":\"/work/p\",\"message\":{{\"id\":\"msg_{file}_{n}\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"{model}\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}],\"stop_reason\":{stop},\"usage\":{{\"input_tokens\":{input},\"output_tokens\":{output},\"cache_creation_input_tokens\":{write},\"cache_creation\":{{\"ephemeral_5m_input_tokens\":{write},\"ephemeral_1h_input_tokens\":0}},\"cache_read_input_tokens\":{read},\"server_tool_use\":{{\"web_search_requests\":0}}}}}}}}\n",
                side = self.subagent,
                output = (block + 1) * 40,
            ));
        }
        self.out.write_all(lines.as_bytes()).unwrap();
        self.bytes += lines.len() as u64;
        self.responses += 1;
    }
}

impl Tree {
    fn add(&mut self, mut t: Transcript<'_>) {
        t.out.flush().unwrap();
        self.files += 1;
        self.bytes += t.bytes;
        self.responses += t.responses;
    }
}

/// 2,500 transcripts, 500 MB in all: four of 50 MiB, the rest about 116 KB each, at times in
/// the 30 days before `now_ms`. User lines of up to 32 KiB carry most of the bytes, as real
/// ones do, so a response averages about 20 KB (this machine: 3.42 GB for 127k responses a
/// month, 27 KB).
fn five_hundred_mb(config_home: &Path, now_ms: i64) -> Tree {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let pad = "x".repeat(32 * 1024);
    let (files, big, total) = (2_500u64, 4u64, 500_000_000u64);
    let small = (total - big * 50 * MIB) / (files - big);
    let mut tree = Tree::default();
    for file in 0..files {
        let target = if file < big {
            50 * MIB
        } else {
            small / 2 + rng.below(small)
        };
        let mut t = Transcript::create(config_home, file, &pad);
        while t.bytes < target {
            let at = now_ms - 1 - rng.below(30 * DAY_MS as u64) as i64;
            let user = rng.below(pad.len() as u64 + 1) as usize;
            t.response(at, &mut rng, user, 2_048);
        }
        tree.add(t);
    }
    tree
}

/// `MONTH_RESPONSES` light responses over the 30 days before `now_ms`, in 1,000 transcripts.
fn a_month_of_responses(config_home: &Path, now_ms: i64) -> Tree {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let files = 1_000u64;
    let mut tree = Tree::default();
    for file in 0..files {
        let mut t = Transcript::create(config_home, file, "");
        for _ in 0..MONTH_RESPONSES / files {
            let at = now_ms - 1 - rng.below(30 * DAY_MS as u64) as i64;
            t.response(at, &mut rng, 0, 0);
        }
        tree.add(t);
    }
    tree
}

/// One complete refresh, waiting for nothing: no other process indexes here.
fn refresh(fx: &Fx) -> IndexReport {
    let outcome = fx
        .engine
        .refresh_tokens(
            &fx.provider(),
            Wait::Wait,
            None,
            fx.engine.cancel(),
            &mut |_| {},
        )
        .unwrap();
    match outcome {
        RefreshOutcome::Done(report) => report,
        RefreshOutcome::Busy => panic!("nothing else refreshes this index"),
    }
}

fn median(mut runs: Vec<Duration>) -> Duration {
    runs.sort();
    runs[runs.len() / 2]
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / MIB as f64
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn a_cold_index_of_500_mb_is_within_1_5_s() {
    let _one_at_a_time = serial();
    let fx = Fx::new();
    let tree = five_hundred_mb(&fx.paths().config_home, fx.engine.now_ms());
    let started = Instant::now();
    let report = refresh(&fx);
    let took = started.elapsed();
    eprintln!(
        "cold index: {took:?} for {} files, {:.0} MiB, {} responses ({:.0} MiB/s)",
        tree.files,
        mib(tree.bytes),
        tree.responses,
        mib(tree.bytes) / took.as_secs_f64()
    );
    assert!(report.complete, "{report:?}");
    assert_eq!(report.responses_added, tree.responses, "{report:?}");
    assert!(took <= COLD, "a cold index of 500 MB took {took:?}");
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn a_refresh_with_nothing_new_is_within_100_ms() {
    let _one_at_a_time = serial();
    let fx = Fx::new();
    let tree = five_hundred_mb(&fx.paths().config_home, fx.engine.now_ms());
    assert_eq!(refresh(&fx).responses_added, tree.responses);
    let runs: Vec<Duration> = (0..10)
        .map(|_| {
            let started = Instant::now();
            let report = refresh(&fx);
            let took = started.elapsed();
            assert_eq!(
                (report.responses_added, report.responses_updated),
                (0, 0),
                "{report:?}"
            );
            took
        })
        .collect();
    let took = median(runs.clone());
    eprintln!(
        "nothing new: median {took:?} over {} files: {runs:?}",
        tree.files
    );
    assert!(
        took <= NOTHING_NEW,
        "a refresh with nothing new took {took:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn a_30_day_report_query_is_within_200_ms() {
    let _one_at_a_time = serial();
    let fx = Fx::new();
    let now = fx.engine.now_ms();
    let tree = a_month_of_responses(&fx.paths().config_home, now);
    assert_eq!(refresh(&fx).responses_added, tree.responses);
    // A profile's sightings, one session in ten, so attribution reads evidence too.
    let store = fx.engine.store().unwrap();
    for file in (0..1_000).step_by(10) {
        let sighting = SightingRow {
            session_id: session_of(file),
            home: "0192-bench-profile".into(),
            started_at: now - 30 * DAY_MS,
            first_at: now - 30 * DAY_MS,
            last_at: now,
            source: SightingSource::Run,
            recorded_at: now,
        };
        store
            .record_sighting(&fx.provider(), &sighting, 60_000)
            .unwrap();
    }
    let periods = [
        ("day", TokenPeriod::Day),
        ("month", TokenPeriod::Month),
        ("session", TokenPeriod::Session),
        ("block", TokenPeriod::Block),
    ];
    for (name, period) in periods {
        let query = || TokenQuery {
            provider: fx.provider(),
            period,
            grouping: TokenGrouping::Account,
            since_ms: Some(now - 30 * DAY_MS),
            until_ms: None,
            accounts: None,
            models: None,
            session: None,
            tz: jiff::tz::TimeZone::UTC,
        };
        let report = fx.engine.token_report(&query()).unwrap();
        assert_eq!(report.total.responses, tree.responses, "{name}");
        let runs: Vec<Duration> = (0..5)
            .map(|_| {
                let started = Instant::now();
                fx.engine.token_report(&query()).unwrap();
                started.elapsed()
            })
            .collect();
        let took = median(runs.clone());
        eprintln!(
            "30-day {name} report: median {took:?} over {} responses, {} rows: {runs:?}",
            tree.responses,
            report.rows.len()
        );
        assert!(took <= QUERY, "a 30-day {name} report took {took:?}");
    }
}
