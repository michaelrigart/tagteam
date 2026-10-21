//! Spec §5: the incremental token index. FakeAgent's transcripts (Task 10) and Claude Code's
//! fixture tree, temp homes, and the fixture's clock.
mod common;

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::{FakeFx, Fx};
use serde_json::json;
use tagteam_core::ProviderId;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::{RefreshStats, TokenFileRow, UsageRow, usage_key};
use tagteam_engine::tokens::{IndexProgress, IndexReport, RefreshOutcome, Wait, tokens_lock_path};
use tagteam_engine::vault::{KeychainVault, Vault};
use tagteam_engine::{Engine, EngineConfig, EventSource};
use tagteam_fake::{FakeAgent, FakeUsage, append_transcript, usage_line_json};
use tagteam_provider::{Cancel, Clock, FlockGuard, RunShell};

const NOW: i64 = 1_790_000_000_000;
const DAY: i64 = 86_400_000;

/// One transcript line for `u`, newline included.
fn line(u: &FakeUsage) -> String {
    format!("{}\n", usage_line_json(u))
}

/// Response `id` of session `s1`: its output `out`, from a final line or a stale snapshot.
fn response(id: &str, out: u64, final_line: bool) -> FakeUsage {
    let mut u = FakeUsage {
        message_id: id.into(),
        request_id: format!("r-{id}"),
        final_line,
        ..FakeUsage::default()
    };
    u.counts.output = out;
    u
}

/// A line of another type, carrying `pad` bytes.
fn other(pad: usize) -> String {
    format!("{}\n", json!({"t": "other", "pad": "p".repeat(pad)}))
}

fn key(p: &ProviderId, id: &str) -> i64 {
    usage_key(p, id, &format!("r-{id}"))
}

fn refresh(engine: &Engine, p: &ProviderId) -> IndexReport {
    match engine
        .refresh_tokens(
            p,
            Wait::Wait,
            None,
            &Cancel::new(),
            &mut |_: &IndexProgress| {},
        )
        .unwrap()
    {
        RefreshOutcome::Done(report) => report,
        RefreshOutcome::Busy => panic!("nothing else refreshes"),
    }
}

fn rows(engine: &Engine, p: &ProviderId) -> Vec<UsageRow> {
    engine
        .store()
        .unwrap()
        .token_usage(p, None, None, None)
        .unwrap()
}

fn row(engine: &Engine, p: &ProviderId, id: &str) -> UsageRow {
    let k = key(p, id);
    rows(engine, p)
        .into_iter()
        .find(|r| r.key == k)
        .unwrap_or_else(|| panic!("{id} is indexed"))
}

fn files(engine: &Engine, p: &ProviderId) -> Vec<TokenFileRow> {
    engine.store().unwrap().token_files(p).unwrap()
}

/// The stored row of the transcript at `path`.
fn file<'a>(files: &'a [TokenFileRow], path: &Path) -> &'a TokenFileRow {
    let path = path.to_string_lossy();
    files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path} has a row"))
}

/// An engine over `ffx`'s home whose only provider is `fake`, with `settings`.
fn fake_engine(ffx: &FakeFx, fake: FakeAgent, settings: Settings) -> Engine {
    Engine::new(EngineConfig {
        env: ffx.fx.env.clone(),
        registry: ProviderRegistry::new().with(Arc::new(fake)),
        vault: Vault::new(Box::new(KeychainVault::new(ffx.fx.kc.clone()))),
        oracle: ffx.fx.oracle.clone(),
        clock: ffx.fx.clock.clone(),
        http: ffx.fx.http.clone(),
        default_provider: ffx.fake_provider(),
        settings,
        process: ffx.fx.process.clone(),
        spawner: ffx.fx.spawner.clone(),
        run_shell: RunShell::Outside,
        source: EventSource::Cli,
    })
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

#[test]
fn a_cold_refresh_indexes_every_response_once() {
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    append_transcript(
        env,
        "a.jsonl",
        &[
            line(&response("m1", 5, false)),
            line(&response("m1", 40, true)),
            line(&response("m2", 7, true)),
        ]
        .concat(),
    );
    append_transcript(env, "b.jsonl", &line(&response("m3", 9, true)));

    let report = refresh(&ffx.engine, &p);
    assert_eq!(
        report,
        IndexReport {
            complete: true,
            files_read: 2,
            responses_added: 3,
            responses_updated: 0,
            skipped_lines: 0,
            evidence_changed: 0,
            took_ms: 0,
        }
    );
    assert_eq!(rows(&ffx.engine, &p).len(), 3);
    let m1 = row(&ffx.engine, &p, "m1");
    assert_eq!((m1.counts.output, m1.finalized), (40, true), "spec §2.2");
    let state = ffx
        .engine
        .store()
        .unwrap()
        .token_state(&p)
        .unwrap()
        .unwrap();
    assert_eq!(state.refreshed_at, Some(NOW));
    assert_eq!(
        state.stats,
        RefreshStats {
            files: 2,
            responses: 3,
            skipped_lines: 0,
            unreadable_files: 0
        }
    );
}

#[test]
fn a_second_refresh_reads_nothing_new() {
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    append_transcript(env, "a.jsonl", &line(&response("m1", 2, true)));
    refresh(&ffx.engine, &p);
    let before = files(&ffx.engine, &p);

    let report = refresh(&ffx.engine, &p);
    assert!(report.complete);
    assert_eq!(
        (
            report.files_read,
            report.responses_added,
            report.responses_updated
        ),
        (0, 0, 0)
    );
    assert_eq!(files(&ffx.engine, &p), before);
}

#[test]
fn refresh_logs_at_info_only_while_something_changes() {
    const MSG: &str = "refreshed the token index";
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    append_transcript(
        env,
        "a.jsonl",
        &[
            line(&response("m1", 2, true)),
            line(&response("m2", 3, true)),
        ]
        .concat(),
    );
    let with = |logs: &[String], level: &str| {
        logs.iter()
            .filter(|l| l.starts_with(level) && l.contains(MSG))
            .count()
    };

    let (_, logs) = common::capture_logs(|| refresh(&ffx.engine, &p));
    assert_eq!(with(&logs, " INFO"), 1, "{logs:?}");

    let (_, logs) = common::capture_logs(|| refresh(&ffx.engine, &p));
    assert_eq!(with(&logs, " INFO"), 0, "{logs:?}");
    assert_eq!(with(&logs, "DEBUG"), 1, "{logs:?}");

    append_transcript(env, "a.jsonl", &line(&response("m3", 4, true)));
    let (_, logs) = common::capture_logs(|| refresh(&ffx.engine, &p));
    assert_eq!(with(&logs, " INFO"), 1, "{logs:?}");
}

#[test]
fn appended_lines_are_picked_up() {
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    append_transcript(env, "a.jsonl", &line(&response("m1", 5, false)));
    refresh(&ffx.engine, &p);
    // The final line of m1, and a new response.
    let path = append_transcript(
        env,
        "a.jsonl",
        &[
            line(&response("m1", 40, true)),
            line(&response("m2", 3, true)),
        ]
        .concat(),
    );

    let report = refresh(&ffx.engine, &p);
    assert_eq!(
        (
            report.files_read,
            report.responses_added,
            report.responses_updated
        ),
        (1, 1, 1)
    );
    let m1 = row(&ffx.engine, &p, "m1");
    assert_eq!((m1.counts.output, m1.finalized), (40, true));
    let len = fs::metadata(&path).unwrap().len();
    let f = file(&files(&ffx.engine, &p), &path).clone();
    assert_eq!((f.offset, f.size), (len, len as i64));
}

#[test]
fn the_same_response_across_files_is_one_row() {
    // Spec §2.2: a resumed session copies history into its new file.
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    let early = FakeUsage {
        at_ms: NOW - 7_200_000,
        ..response("m1", 5, false)
    };
    let late = FakeUsage {
        at_ms: NOW - 3_600_000,
        ..response("m1", 40, true)
    };
    append_transcript(env, "a.jsonl", &line(&early));
    append_transcript(env, "b.jsonl", &line(&late));

    let report = refresh(&ffx.engine, &p);
    assert_eq!(report.responses_added, 1);
    let all = rows(&ffx.engine, &p);
    assert_eq!(all.len(), 1);
    assert_eq!(
        (all[0].at_ms, all[0].counts.output, all[0].finalized),
        (NOW - 7_200_000, 40, true),
        "the earliest time, each count's maximum"
    );
}

#[test]
fn lines_outside_the_retention_window_are_not_inserted() {
    // Spec §4 Retention, Review Focus 3: a line older than the window is not inserted; a
    // future one is.
    let ffx = FakeFx::new();
    let settings = Settings {
        tokens_retention_days: 30,
        ..Settings::default()
    };
    let engine = fake_engine(&ffx, FakeAgent::new(), settings);
    let p = ffx.fake_provider();
    let at = |id: &str, at_ms: i64| {
        line(&FakeUsage {
            at_ms,
            ..response(id, 1, true)
        })
    };
    append_transcript(
        &ffx.fx.env,
        "a.jsonl",
        &[
            at("old", NOW - 31 * DAY),
            at("kept", NOW - 29 * DAY),
            at("future", NOW + DAY),
        ]
        .concat(),
    );

    let report = refresh(&engine, &p);
    assert!(report.complete);
    let keys: BTreeSet<i64> = rows(&engine, &p).iter().map(|r| r.key).collect();
    assert_eq!(keys, BTreeSet::from([key(&p, "kept"), key(&p, "future")]));
}

#[test]
#[cfg(feature = "test-hooks")]
fn a_waiting_refresh_never_passes_the_lock_while_another_holds_it() {
    // Spec §5.1: `Wait::Wait` waits for another refresh. While the test holds the lock, the
    // refresh reaches the point before its wait and never the one after it: only the cancel
    // token ends that wait. Were the lock not taken, nothing between the two points could be
    // interrupted. Once the lock is free, the next refresh passes that point holding it.
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;

    let ffx = FakeFx::new();
    let p = ffx.fake_provider();
    append_transcript(&ffx.fx.env, "a.jsonl", &line(&response("m1", 2, true)));
    let path = tokens_lock_path(&ffx.fx.env, &p);
    let other = FlockGuard::try_lock(&path).unwrap().unwrap();
    let (at_lock, reached) = mpsc::channel();
    ffx.engine.on_point(
        "refresh-tokens-lock",
        Box::new(move || at_lock.send(()).unwrap()),
    );
    let passed = Arc::new(AtomicBool::new(false));
    let free_there = Arc::new(AtomicBool::new(true));
    let (passed_cb, free_cb, path_cb) = (passed.clone(), free_there.clone(), path.clone());
    ffx.engine.also_on_point(
        "refresh-tokens-locked",
        Box::new(move || {
            passed_cb.store(true, Ordering::SeqCst);
            let free = FlockGuard::try_lock(&path_cb).unwrap().is_some();
            free_cb.store(free, Ordering::SeqCst);
        }),
    );

    let cancel = Cancel::new();
    let first = thread::scope(|s| {
        let waiting = s.spawn(|| {
            ffx.engine
                .refresh_tokens(&p, Wait::Wait, None, &cancel, &mut |_: &IndexProgress| {})
        });
        reached.recv().unwrap();
        cancel.request(2);
        waiting.join().unwrap()
    });
    assert_eq!(first.unwrap_err().signal(), Some(2));
    assert!(
        !passed.load(Ordering::SeqCst),
        "it passed the lock while another refresh held it"
    );
    assert!(rows(&ffx.engine, &p).is_empty());

    drop(other);
    let report = refresh(&ffx.engine, &p);
    assert!(report.complete);
    assert_eq!(report.responses_added, 1);
    assert!(passed.load(Ordering::SeqCst));
    assert!(
        !free_there.load(Ordering::SeqCst),
        "past that point the refresh holds the lock"
    );
}

#[test]
fn try_returns_busy_while_another_holds_the_lock() {
    let ffx = FakeFx::new();
    let p = ffx.fake_provider();
    append_transcript(&ffx.fx.env, "a.jsonl", &line(&response("m1", 2, true)));
    let other = FlockGuard::try_lock(&tokens_lock_path(&ffx.fx.env, &p))
        .unwrap()
        .unwrap();

    let outcome = ffx
        .engine
        .refresh_tokens(
            &p,
            Wait::Try,
            None,
            &Cancel::new(),
            &mut |_: &IndexProgress| {},
        )
        .unwrap();
    assert_eq!(outcome, RefreshOutcome::Busy);
    assert!(rows(&ffx.engine, &p).is_empty());

    drop(other);
    let RefreshOutcome::Done(report) = ffx
        .engine
        .refresh_tokens(
            &p,
            Wait::Try,
            None,
            &Cancel::new(),
            &mut |_: &IndexProgress| {},
        )
        .unwrap()
    else {
        panic!("the lock is free");
    };
    assert!(report.complete);
    assert_eq!(report.responses_added, 1);
}

#[test]
fn a_budgeted_refresh_resumes_where_it_stopped() {
    // Spec §5.2 Budget: past its deadline a refresh stops at the next line boundary and commits
    // what it read; the next call goes on from there. A file left part-way is never skipped, and
    // only the call that completes sets `refreshed_at`, which moves `changed_at` (spec §14).
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    let store = ffx.engine.store().unwrap();
    let a: String = (0..5)
        .map(|i| line(&response(&format!("a{i}"), 2, true)))
        .collect();
    let b: String = (0..3)
        .map(|i| line(&response(&format!("b{i}"), 2, true)))
        .collect();
    let paths: Vec<(PathBuf, u64)> = vec![
        (append_transcript(env, "a.jsonl", &a), a.len() as u64),
        (append_transcript(env, "b.jsonl", &b), b.len() as u64),
    ];
    // Bytes consumed plus files read to their end: it grows on every call that does not complete.
    let progress_of = |files: &[TokenFileRow]| -> u64 {
        files
            .iter()
            .map(|f| f.offset + u64::from(f.size >= 0))
            .sum()
    };

    let mut seen = 0;
    let mut calls = 0;
    let report = loop {
        calls += 1;
        assert!(calls <= 12, "every call makes progress");
        ffx.fx.clock.advance_ms(1_000);
        let RefreshOutcome::Done(report) = ffx
            .engine
            .refresh_tokens(
                &p,
                Wait::Wait,
                Some(Duration::ZERO),
                &Cancel::new(),
                &mut |_: &IndexProgress| {},
            )
            .unwrap()
        else {
            panic!("nothing else refreshes");
        };
        if report.complete {
            break report;
        }
        let stored = store.token_files(&p).unwrap();
        let now = progress_of(&stored);
        assert!(now > seen, "call {calls} made no progress");
        seen = now;
        for (path, len) in &paths {
            if let Some(f) = stored.iter().find(|f| f.path == path.to_string_lossy()) {
                if f.offset < *len {
                    assert_eq!(f.size, -1, "{} is read part-way", f.path);
                }
            }
        }
        assert_eq!(store.token_state(&p).unwrap().unwrap().refreshed_at, None);
    };

    assert_eq!(
        report.responses_added, 0,
        "the completing call found only the files' ends"
    );
    assert_eq!(rows(&ffx.engine, &p).len(), 8);
    let stored = store.token_files(&p).unwrap();
    for (path, len) in &paths {
        let f = file(&stored, path);
        assert_eq!((f.offset, f.size), (*len, *len as i64));
    }
    let now = ffx.fx.clock.now_ms();
    let state = store.token_state(&p).unwrap().unwrap();
    assert_eq!(
        (state.refreshed_at, state.changed_at),
        (Some(now), Some(now)),
        "the first completion moves changed_at"
    );
}

#[test]
fn cancel_commits_what_was_parsed_and_reports_interrupted() {
    // Spec §5.2 step 4: a cancelled refresh stops at the next chunk, commits what it parsed, and
    // loses nothing. Six 1 MiB chunks; the token is set once the writer has the first one, so
    // the worker, at most two chunks ahead of it on the bounded channel, stops part-way.
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    let mut text = String::new();
    let mut ends = Vec::new();
    for i in 0..6_000 {
        if i % 100 == 0 {
            let id = format!("m{i}");
            text.push_str(&line(&response(&id, 1, true)));
            ends.push((text.len() as u64, id));
        } else {
            text.push_str(&other(1_000));
        }
    }
    append_transcript(env, "big.jsonl", &text);

    let cancel = Cancel::new();
    let err = ffx
        .engine
        .refresh_tokens(&p, Wait::Wait, None, &cancel, &mut |at: &IndexProgress| {
            if at.bytes_done > 0 {
                cancel.request(2);
            }
        })
        .unwrap_err();
    assert_eq!((err.signal(), err.kind()), (Some(2), "interrupted"));

    let stored = files(&ffx.engine, &p);
    assert_eq!(stored.len(), 1);
    let offset = stored[0].offset;
    assert!(offset > 0 && offset < text.len() as u64, "{offset}");
    assert_eq!(stored[0].size, -1, "a cancelled read is never skipped");
    let indexed: BTreeSet<i64> = rows(&ffx.engine, &p).iter().map(|r| r.key).collect();
    let before: BTreeSet<i64> = ends
        .iter()
        .filter(|(end, _)| *end <= offset)
        .map(|(_, id)| key(&p, id))
        .collect();
    assert!(!before.is_empty());
    assert_eq!(indexed, before, "exactly the lines before its checkpoint");
    assert_eq!(
        ffx.engine
            .store()
            .unwrap()
            .token_state(&p)
            .unwrap()
            .unwrap()
            .refreshed_at,
        None
    );

    let report = refresh(&ffx.engine, &p);
    assert!(report.complete);
    assert_eq!(rows(&ffx.engine, &p).len(), 60, "nothing lost");
}

#[test]
fn a_provider_without_transcripts_is_refused() {
    let ffx = FakeFx::new();
    let engine = fake_engine(
        &ffx,
        FakeAgent::new().without_transcripts(),
        Settings::default(),
    );
    let p = ffx.fake_provider();
    let err = engine
        .refresh_tokens(
            &p,
            Wait::Wait,
            None,
            &Cancel::new(),
            &mut |_: &IndexProgress| {},
        )
        .unwrap_err();
    assert_eq!(err.kind(), "invalid-input");
    assert_eq!(err.to_string(), "FakeAgent has no transcripts");
    assert!(
        !tokens_lock_path(&ffx.fx.env, &p).exists(),
        "refused before the lock"
    );
}

#[test]
fn a_pass_over_several_budgeted_calls_stores_the_skips_of_all_of_them() {
    // Spec §5.4: the stored count is the last completed refresh's, and a pass spread over
    // budgeted calls is one refresh. A skipped line is consumed, so no later call sees it again.
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    let bad = "{\"t\":\"usage\",\"id\":7}\n";
    let a = format!(
        "{bad}{bad}{}{bad}{}",
        line(&response("a0", 2, true)),
        line(&response("a1", 2, true))
    );
    let b: String = (0..3)
        .map(|i| line(&response(&format!("b{i}"), 2, true)))
        .collect();
    append_transcript(env, "a.jsonl", &a);
    append_transcript(env, "b.jsonl", &b);

    let mut reports = Vec::new();
    let last = loop {
        assert!(reports.len() < 20, "every call makes progress");
        ffx.fx.clock.advance_ms(1_000);
        let RefreshOutcome::Done(report) = ffx
            .engine
            .refresh_tokens(
                &p,
                Wait::Wait,
                Some(Duration::ZERO),
                &Cancel::new(),
                &mut |_: &IndexProgress| {},
            )
            .unwrap()
        else {
            panic!("nothing else refreshes");
        };
        reports.push(report);
        if report.complete {
            break report;
        }
    };

    assert!(reports.len() > 1, "the pass took several calls");
    assert_eq!(last.skipped_lines, 0, "the completing call skipped none");
    let sum: u64 = reports.iter().map(|r| r.skipped_lines).sum();
    assert!(sum >= 3, "the calls skipped {sum} of 3 bad lines");
    let state = ffx
        .engine
        .store()
        .unwrap()
        .token_state(&p)
        .unwrap()
        .unwrap();
    assert_eq!(state.stats.skipped_lines, sum);
    assert_eq!(ffx.engine.token_index_state(&p).unwrap().skipped_lines, sum);

    // The next pass starts from nothing.
    ffx.fx.clock.advance_ms(1_000);
    assert!(refresh(&ffx.engine, &p).complete);
    let state = ffx
        .engine
        .store()
        .unwrap()
        .token_state(&p)
        .unwrap()
        .unwrap();
    assert_eq!(state.stats.skipped_lines, 0);
}

#[test]
#[cfg(feature = "test-hooks")]
fn a_commit_inside_a_chunk_does_not_carry_the_chunks_skips() {
    // Spec §5.2 step 4: a chunk's skips go with its checkpoint. A chunk of more than 2,000 new
    // responses commits once mid-chunk, before its checkpoint is in the batch; a failure after
    // that commit has the file read again from the start, so the skips must not be stored yet.
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    let bad = "{\"t\":\"usage\",\"id\":7}\n";
    let mut text = format!("{bad}{bad}");
    for i in 0..2_100 {
        text.push_str(&line(&response(&format!("m{i}"), 2, true)));
        if i == 1_000 {
            text.push_str(bad);
        }
    }
    assert!(text.len() < 1 << 20, "the file is one chunk");
    append_transcript(env, "a.jsonl", &text);

    ffx.engine.fail_at(Some("refresh-tokens-committed"));
    let err = ffx
        .engine
        .refresh_tokens(
            &p,
            Wait::Wait,
            None,
            &Cancel::new(),
            &mut |_: &IndexProgress| {},
        )
        .unwrap_err();
    assert!(err.to_string().contains("injected failure"), "{err}");
    assert!(
        !rows(&ffx.engine, &p).is_empty() && files(&ffx.engine, &p).is_empty(),
        "the first commit held rows and no checkpoint"
    );

    ffx.engine.fail_at(None);
    ffx.fx.clock.advance_ms(1_000);
    assert!(refresh(&ffx.engine, &p).complete);
    let state = ffx
        .engine
        .store()
        .unwrap()
        .token_state(&p)
        .unwrap()
        .unwrap();
    assert_eq!(state.stats.skipped_lines, 3);
}

#[test]
fn skipped_lines_are_counted_in_the_state() {
    // Spec §5.4: a line that does not parse is skipped and counted; it never fails the refresh.
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    append_transcript(
        env,
        "a.jsonl",
        &[
            line(&response("m1", 2, true)),
            "{\"t\":\"usage\",\"id\":\"m2\"\n".to_owned(),
            "{\"t\":\"usage\",\"id\":7}\n".to_owned(),
            line(&response("m3", 2, true)),
        ]
        .concat(),
    );

    let report = refresh(&ffx.engine, &p);
    assert!(report.complete);
    assert_eq!((report.responses_added, report.skipped_lines), (2, 2));
    let state = ffx
        .engine
        .store()
        .unwrap()
        .token_state(&p)
        .unwrap()
        .unwrap();
    assert_eq!(
        state.stats,
        RefreshStats {
            files: 1,
            responses: 2,
            skipped_lines: 2,
            unreadable_files: 0
        }
    );
}

#[test]
fn an_unreadable_file_is_counted_and_retried() {
    // Spec §5.4: a file that cannot be read is skipped with a warning, counted, and read again on
    // a later refresh; it never fails the refresh.
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    append_transcript(env, "a.jsonl", &line(&response("m1", 2, true)));
    let locked = append_transcript(env, "b.jsonl", &line(&response("m2", 2, true)));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&locked).is_ok() {
        // A privileged run (root) reads a file of mode 000: no file can be made unreadable here,
        // so there is nothing to test.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }

    let report = refresh(&ffx.engine, &p);
    assert!(report.complete);
    assert_eq!(report.responses_added, 1);
    let stats = ffx
        .engine
        .store()
        .unwrap()
        .token_state(&p)
        .unwrap()
        .unwrap()
        .stats;
    assert_eq!((stats.files, stats.unreadable_files), (1, 1));

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o600)).unwrap();
    let report = refresh(&ffx.engine, &p);
    assert_eq!(report.responses_added, 1, "the unreadable file is retried");
}

/// `n` responses older than any retention window, as an index a year old holds them.
fn plant_expired(engine: &Engine, p: &ProviderId, n: usize) {
    let rows: Vec<UsageRow> = (0..n)
        .map(|i| UsageRow {
            key: key(p, &format!("old{i}")),
            session_id: "s-old".into(),
            project: None,
            at_ms: NOW - 400 * DAY,
            model: "fake-1".into(),
            sidechain: false,
            counts: Default::default(),
            speed: None,
            finalized: true,
        })
        .collect();
    let store = engine.store().unwrap();
    let epoch = store.begin_token_refresh(p, NOW).unwrap();
    store
        .commit_token_batch(p, epoch, &rows, &[], &[], 0, NOW - 400 * DAY)
        .unwrap();
}

#[test]
#[cfg(feature = "test-hooks")]
fn a_purge_mid_refresh_aborts_the_refresh_without_rewriting() {
    // Spec §5.1, §13: a full purge locks the providers with token rows when it starts. A refresh
    // of one that had none then may still be running. Once the purge has emptied the index, the
    // refresh's next write is refused, as written under a purge generation that is gone, and the
    // refresh ends without rewriting a row. A refresh that begins a new generation after it is
    // refused the same way by a second purge. The next one starts clean.
    use std::sync::atomic::{AtomicBool, Ordering};
    use tagteam_engine::EngineError;
    use tagteam_engine::store::StoreError;

    let ffx = FakeFx::new();
    let p = ffx.fake_provider();
    // More responses than one batch holds, so at least two batches commit.
    let text: String = (0..2_500)
        .map(|i| line(&response(&format!("m{i}"), 1, true)))
        .collect();
    append_transcript(&ffx.fx.env, "a.jsonl", &text);
    // The purge empties the store through another engine's connection, as another process
    // would, right after the refresh's first batch commits.
    let other = ffx.fx.engine.store().unwrap();
    let purged = Arc::new(AtomicBool::new(false));
    let purged_cb = purged.clone();
    ffx.engine.on_point(
        "refresh-tokens-committed",
        Box::new(move || {
            if !purged_cb.swap(true, Ordering::SeqCst) {
                other.empty_all().unwrap();
            }
        }),
    );

    let err = ffx
        .engine
        .refresh_tokens(
            &p,
            Wait::Wait,
            None,
            &Cancel::new(),
            &mut |_: &IndexProgress| {},
        )
        .unwrap_err();
    assert!(
        matches!(err, EngineError::Store(StoreError::TokenIndexPurged)),
        "{err:?}"
    );
    assert!(purged.load(Ordering::SeqCst));
    let store = ffx.engine.store().unwrap();
    assert!(
        rows(&ffx.engine, &p).is_empty(),
        "no row written after the purge"
    );
    assert!(store.token_files(&p).unwrap().is_empty());
    assert_eq!(store.token_state(&p).unwrap(), None);

    // The next refresh begins a new generation, and a second purge lands before its first batch:
    // that batch is refused too.
    let other = ffx.fx.engine.store().unwrap();
    let again = Arc::new(AtomicBool::new(false));
    let again_cb = again.clone();
    ffx.engine.also_on_point(
        "refresh-tokens-locked",
        Box::new(move || {
            if !again_cb.swap(true, Ordering::SeqCst) {
                other.empty_all().unwrap();
            }
        }),
    );
    let err = ffx
        .engine
        .refresh_tokens(
            &p,
            Wait::Wait,
            None,
            &Cancel::new(),
            &mut |_: &IndexProgress| {},
        )
        .unwrap_err();
    assert!(
        matches!(err, EngineError::Store(StoreError::TokenIndexPurged)),
        "{err:?}"
    );
    assert!(again.load(Ordering::SeqCst));
    assert!(
        rows(&ffx.engine, &p).is_empty(),
        "no row written after the second purge"
    );
    assert!(store.token_files(&p).unwrap().is_empty());
    assert_eq!(store.token_state(&p).unwrap(), None);

    let report = refresh(&ffx.engine, &p);
    assert!(report.complete);
    assert_eq!(
        report.responses_added, 2_500,
        "the next refresh starts clean"
    );
}

#[test]
fn a_budgeted_prune_goes_on_over_several_calls() {
    // Spec §4 Retention, §5.2 Budget: the prune that ends a refresh stops with the budget, as
    // reading does. The refresh is then incomplete, and the next call goes on with the prune.
    // Even under a zero budget every call deletes some or completes: the store asks `stop` only
    // after a chunk that deleted rows. 12,001 rows are more than one of the store's prune chunks.
    let ffx = FakeFx::new();
    let p = ffx.fake_provider();
    plant_expired(&ffx.engine, &p, 12_001);
    let mut left = rows(&ffx.engine, &p).len();
    let mut calls = 0;
    loop {
        calls += 1;
        assert!(calls <= 10, "every call deletes some or completes");
        let RefreshOutcome::Done(report) = ffx
            .engine
            .refresh_tokens(
                &p,
                Wait::Wait,
                Some(Duration::ZERO),
                &Cancel::new(),
                &mut |_: &IndexProgress| {},
            )
            .unwrap()
        else {
            panic!("nothing else refreshes");
        };
        let now = rows(&ffx.engine, &p).len();
        if report.complete {
            assert_eq!(now, 0);
            break;
        }
        // Responses while any are left; after them the cleanup phases (the sessions the
        // responses left empty), which these rows do not show.
        assert!(now < left || left == 0, "{now} of {left} left");
        let state = ffx
            .engine
            .store()
            .unwrap()
            .token_state(&p)
            .unwrap()
            .unwrap();
        assert_eq!(state.refreshed_at, None);
        left = now;
    }
    assert!(
        calls >= 2,
        "a prune of more than one chunk takes more than one budgeted call"
    );
}

#[test]
fn a_cancel_that_meets_a_refresh_with_nothing_to_do_reports_interrupted() {
    // Nothing to read and nothing to prune: the refresh completes, and it records that, but a
    // cancel it met is still reported, so `tokens` stops instead of printing a report.
    let ffx = FakeFx::new();
    let p = ffx.fake_provider();
    let cancel = Cancel::new();
    let err = ffx
        .engine
        .refresh_tokens(&p, Wait::Wait, None, &cancel, &mut |_: &IndexProgress| {
            cancel.request(2)
        })
        .unwrap_err();
    assert_eq!((err.signal(), err.kind()), (Some(2), "interrupted"));
    let state = ffx
        .engine
        .store()
        .unwrap()
        .token_state(&p)
        .unwrap()
        .unwrap();
    assert!(state.refreshed_at.is_some(), "what it completed is kept");
}

#[test]
#[cfg(feature = "test-hooks")]
fn a_scan_that_fails_loses_its_evidence_and_the_refresh_goes_on() {
    // Step 1's scan is evidence: when it cannot write (a store busy past its timeout, injected
    // here at its launch check), the refresh indexes and completes without it.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    fx.engine.open_launch(&cc, &a).unwrap();
    fx.engine.fail_at(Some("scan-launch-check"));
    let report = refresh(&fx.engine, &cc);
    assert!(report.complete);
    assert_eq!(report.evidence_changed, 0);
}

#[test]
fn a_cancelled_prune_reports_interrupted() {
    // A cancel that meets the prune follows the refresh's cancel path: the prune stops before
    // its next chunk, what was pruned stays pruned, and the refresh is interrupted, not complete.
    let ffx = FakeFx::new();
    let p = ffx.fake_provider();
    plant_expired(&ffx.engine, &p, 12_001);
    let cancel = Cancel::new();
    // Nothing to read: the token is set as the refresh starts, and the prune is what meets it,
    // after its first chunk (the store asks `stop` only once a chunk has deleted rows).
    let err = ffx
        .engine
        .refresh_tokens(&p, Wait::Wait, None, &cancel, &mut |_: &IndexProgress| {
            cancel.request(2)
        })
        .unwrap_err();
    assert_eq!(err.signal(), Some(2));
    let left = rows(&ffx.engine, &p).len();
    assert!(left > 0, "the prune stopped short of the end");
    let state = ffx
        .engine
        .store()
        .unwrap()
        .token_state(&p)
        .unwrap()
        .unwrap();
    assert_eq!(state.refreshed_at, None);
}

#[test]
fn the_claude_code_fixture_tree_is_indexed() {
    // Task 8's synthetic tree: 4 files, 4 responses over 8 usage lines, 2 unreadable lines,
    // subagents included. The longest retention keeps the fixtures' dates in the window.
    let fx = Fx::new();
    let engine = fx.engine_with_settings(Settings {
        tokens_retention_days: 3_650,
        ..Settings::default()
    });
    let cc = fx.provider();
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tagteam-cc/tests/fixtures/transcripts/projects"),
        &fx.paths().config_home.join("projects"),
    );

    let report = refresh(&engine, &cc);
    assert!(report.complete);
    assert_eq!(
        (
            report.files_read,
            report.responses_added,
            report.skipped_lines
        ),
        (4, 4, 2)
    );
    assert_eq!(rows(&engine, &cc).len(), 4);
    assert_eq!(files(&engine, &cc).len(), 4);
    assert_eq!(
        refresh(&engine, &cc).files_read,
        0,
        "and nothing on the next"
    );
}

#[test]
fn a_deleted_transcript_drops_its_file_row_and_keeps_its_usage() {
    // Spec §5.2 step 2: the rows of files no longer found are removed; their usage stays. With
    // nothing else to read, the removal is a commit of its own.
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    let a = append_transcript(env, "a.jsonl", &line(&response("m1", 2, true)));
    let b = append_transcript(env, "b.jsonl", &line(&response("m2", 3, true)));
    refresh(&ffx.engine, &p);
    assert_eq!(files(&ffx.engine, &p).len(), 2);
    fs::remove_file(&b).unwrap();

    let report = refresh(&ffx.engine, &p);
    assert!(report.complete);
    assert_eq!(
        (
            report.files_read,
            report.responses_added,
            report.skipped_lines
        ),
        (0, 0, 0)
    );
    let stored = files(&ffx.engine, &p);
    assert_eq!(stored.len(), 1);
    file(&stored, &a);
    assert_eq!(rows(&ffx.engine, &p).len(), 2, "its usage stays");
    assert_eq!(row(&ffx.engine, &p, "m2").counts.output, 3);
    let stats = ffx
        .engine
        .store()
        .unwrap()
        .token_state(&p)
        .unwrap()
        .unwrap()
        .stats;
    assert_eq!(
        (stats.files, stats.responses, stats.unreadable_files),
        (1, 2, 0)
    );
}

#[test]
fn a_truncated_or_rewritten_transcript_is_read_again_from_the_start() {
    // Spec §5.2 step 2: a file that shrank, or whose head no longer matches, is not resumed at
    // its old offset: it is read from 0, and each response counts once.
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    let path = append_transcript(
        env,
        "a.jsonl",
        &[
            line(&response("m1", 5, false)),
            line(&response("m2", 7, true)),
            line(&response("m3", 9, true)),
        ]
        .concat(),
    );
    refresh(&ffx.engine, &p);

    // Truncated to a shorter file with other content.
    let shorter = [
        line(&response("m1", 40, true)),
        line(&response("m4", 1, true)),
    ]
    .concat();
    fs::write(&path, &shorter).unwrap();
    let report = refresh(&ffx.engine, &p);
    assert!(report.complete);
    assert_eq!(
        (
            report.files_read,
            report.responses_added,
            report.responses_updated
        ),
        (1, 1, 1)
    );
    assert_eq!(rows(&ffx.engine, &p).len(), 4, "m1 to m4, each once");
    assert_eq!(row(&ffx.engine, &p, "m1").counts.output, 40);
    let len = fs::metadata(&path).unwrap().len();
    let f = file(&files(&ffx.engine, &p), &path).clone();
    assert_eq!((f.offset, f.size), (len, len as i64));

    // Rewritten longer than before, with a new head.
    let longer: String = (5..15)
        .map(|i| line(&response(&format!("m{i}"), 1, true)))
        .collect();
    fs::write(&path, &longer).unwrap();
    let report = refresh(&ffx.engine, &p);
    assert_eq!((report.files_read, report.responses_added), (1, 10));
    assert_eq!(rows(&ffx.engine, &p).len(), 14);
    let len = fs::metadata(&path).unwrap().len();
    let f = file(&files(&ffx.engine, &p), &path).clone();
    assert_eq!((f.offset, f.size), (len, len as i64));
}

#[test]
fn unreadable_files_do_not_use_up_a_spent_budget() {
    // More unreadable files, sorted first, than workers: under a zero budget a worker's one
    // guaranteed file is one it reads, so the readable file after them is still reached.
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    let mut locked = Vec::new();
    for i in 0..6 {
        let path = append_transcript(env, &format!("a{i}.jsonl"), &line(&response("x", 1, true)));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        locked.push(path);
    }
    append_transcript(env, "z.jsonl", &line(&response("m1", 2, true)));
    let restore = |locked: &[PathBuf]| {
        for path in locked {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    };
    if fs::File::open(&locked[0]).is_ok() {
        // A privileged run reads files of mode 000: none can be made unreadable here.
        restore(&locked);
        return;
    }

    let outcome = ffx
        .engine
        .refresh_tokens(
            &p,
            Wait::Wait,
            Some(Duration::ZERO),
            &Cancel::new(),
            &mut |_: &IndexProgress| {},
        )
        .unwrap();
    restore(&locked);
    let RefreshOutcome::Done(report) = outcome else {
        panic!("nothing else refreshes");
    };
    assert_eq!(report.responses_added, 1, "z.jsonl was read");
    assert_eq!(rows(&ffx.engine, &p).len(), 1);
}
