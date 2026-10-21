//! The token report's jobs (D13; token report spec §12) against a real engine over a fixture
//! home: transcripts under `~/.claude/projects`, session records under `~/.claude/sessions`, a
//! store, and the index's own lock.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tagteam_cc::CLAUDE_CODE;
use tagteam_cc::ClaudeCode;
use tagteam_cc::live::Platform;
use tagteam_core::ProviderId;
use tagteam_daemon::events::{DaemonEvent, DaemonSink, StopReason};
use tagteam_daemon::protocol::{Event, daemon_event_json, parse_event};
use tagteam_daemon::scheduler::{Job, JobCx, Ran};
use tagteam_daemon::status::{DaemonInfo, JobStatus, NotifyInfo, StatusBoard};
use tagteam_daemon::tokens::{INDEX_EVERY_MS, SESSIONS_EVERY_MS, token_jobs};
use tagteam_engine::oracle::NoOracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::tokens::{IndexProgress, IndexState, Wait, tokens_lock_path};
use tagteam_engine::vault::{KeychainVault, Vault};
use tagteam_engine::{Engine, EngineConfig, EngineError, EventSource};
use tagteam_provider::{
    Env, FakeClock, FakeKeychain, FakeProcess, FakeProcessProbe, FlockGuard, RunShell,
    ScriptedHttp, ScriptedSpawner,
};

/// 2026-10-14T20:45:00Z.
const T0_MS: i64 = 1_792_010_700_000;

fn cc() -> ProviderId {
    ProviderId::new(CLAUDE_CODE)
}

struct Fx {
    _dir: tempfile::TempDir,
    env: Env,
    clock: Arc<FakeClock>,
    process: Arc<FakeProcessProbe>,
    engine: Engine,
    board: Arc<StatusBoard>,
}

/// A macOS engine over a fresh fixture home whose Claude Code has run, with its store created
/// when `store` is true.
fn fx(store: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    let kc = Arc::new(FakeKeychain::new());
    let clock = Arc::new(FakeClock::new(T0_MS));
    let process = Arc::new(FakeProcessProbe::new());
    let engine = Engine::new(EngineConfig {
        env: env.clone(),
        registry: ProviderRegistry::new()
            .with(Arc::new(ClaudeCode::new(kc.clone(), Platform::MacOs))),
        vault: Vault::new(Box::new(KeychainVault::new(kc))),
        oracle: Arc::new(NoOracle),
        clock: clock.clone(),
        http: Arc::new(ScriptedHttp::new()),
        default_provider: cc(),
        settings: Settings::default(),
        process: process.clone(),
        spawner: Arc::new(ScriptedSpawner::new()),
        run_shell: RunShell::Outside,
        source: EventSource::Daemon,
    });
    if store {
        engine.store().unwrap();
    }
    let board = StatusBoard::new(
        DaemonInfo {
            pid: 4242,
            version: "test".into(),
            started_at: T0_MS / 1000,
            socket: None,
            log: dir.path().join("tagteam.log"),
        },
        NotifyInfo {
            enabled: false,
            backend: None,
            events: Vec::new(),
        },
    );
    Fx {
        _dir: dir,
        env,
        clock,
        process,
        engine,
        board,
    }
}

#[derive(Default)]
struct Recorded(Mutex<Vec<DaemonEvent>>);

impl DaemonSink for Recorded {
    fn emit(&self, e: &DaemonEvent) {
        self.0.lock().unwrap().push(e.clone());
    }
}

impl Recorded {
    /// The `token-index` events so far.
    fn token_index(&self) -> Vec<(ProviderId, IndexState)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                DaemonEvent::TokenIndex { provider, state } => Some((provider.clone(), *state)),
                _ => None,
            })
            .collect()
    }
}

fn run(job: &mut dyn Job, fx: &Fx, sink: &Recorded) -> Ran {
    let mut jitter = || 0.0;
    let mut cx = JobCx {
        engine: &fx.engine,
        sink,
        jitter: &mut jitter,
    };
    job.run(&mut cx)
}

fn job_status(fx: &Fx, name: &str) -> JobStatus {
    fx.board
        .snapshot()
        .jobs
        .into_iter()
        .find(|j| j.name == name)
        .unwrap_or_else(|| panic!("{name} is not on the board"))
}

/// Response `n` of `session` as one final assistant line (token report §2.1, §2.2), with its own
/// message and request IDs, `n` input tokens and 10 output tokens.
fn line(session: &str, n: u64) -> String {
    json!({
        "type": "assistant",
        "sessionId": session,
        "requestId": format!("req_{session}_{n:06}"),
        "timestamp": "2026-10-14T20:00:00.000Z",
        "cwd": "/work/app",
        "isSidechain": false,
        "message": {
            "id": format!("msg_{session}_{n:06}"),
            "model": "claude-opus-5-5",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": n,
                "output_tokens": 10,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0
            }
        }
    })
    .to_string()
}

/// Appends responses `from..to` of `session` to its transcript in the default home.
fn append(fx: &Fx, session: &str, from: u64, to: u64) -> PathBuf {
    let path = fx
        .env
        .home
        .join(format!(".claude/projects/-work-app/{session}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap();
    for n in from..to {
        writeln!(f, "{}", line(session, n)).unwrap();
    }
    path
}

#[test]
fn the_index_job_refreshes_and_broadcasts_each_change_once() {
    // §12: a refresh, then the index's state, broadcast when `changed_at_ms` moved.
    let fx = fx(true);
    let sink = Recorded::default();
    let (_, mut index) = token_jobs(vec![cc()], fx.board.clone(), false);
    append(&fx, "s1", 0, 2);
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    let events = sink.token_index();
    assert_eq!(events.len(), 1, "{events:?}");
    let (provider, state) = &events[0];
    assert_eq!(provider, &cc());
    assert_eq!((state.files, state.responses), (1, 2), "{state:?}");
    assert!(
        state.refreshed_at_ms.is_some() && state.changed_at_ms.is_some(),
        "{state:?}"
    );
    let job = job_status(&fx, "token-index");
    assert_eq!(job.last_error, None);
    assert!(job.last_run_at.is_some() && job.next_run_at > job.last_run_at);

    fx.clock.advance_ms(60_000);
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    assert_eq!(
        sink.token_index().len(),
        1,
        "nothing new, nothing broadcast"
    );

    fx.clock.advance_ms(60_000);
    append(&fx, "s1", 2, 3);
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    let events = sink.token_index();
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[1].1.responses, 3);
}

#[test]
fn an_incomplete_refresh_is_due_again_at_once_until_it_completes() {
    // §5.2's budget and §12: a slice that runs out first returns `complete: false`, and the
    // job is due again at once. Under a zero budget a call reads one line past the deadline, so
    // a transcript of several lines needs several calls.
    let fx = fx(true);
    let sink = Recorded::default();
    let (_, index) = token_jobs(vec![cc()], fx.board.clone(), false);
    let mut index = index.with_slice(Duration::ZERO);
    const N: u64 = 5;
    append(&fx, "big", 0, N);
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(0));
    let mut runs = 1;
    while run(&mut index, &fx, &sink) == Ran::After(0) {
        runs += 1;
        assert!(runs < 50, "the refresh never completed");
    }
    let state = fx.engine.token_index_state(&cc()).unwrap();
    assert_eq!(state.responses, N);
    assert!(state.refreshed_at_ms.is_some(), "{state:?}");
}

#[test]
fn a_refresh_another_process_holds_is_tried_again_in_a_minute() {
    // §5.1: `Wait::Try` answers `Busy` while another refresh holds the provider's tokens lock.
    // The job is due again in 60 s, with no error.
    let fx = fx(true);
    let sink = Recorded::default();
    let (_, mut index) = token_jobs(vec![cc()], fx.board.clone(), false);
    append(&fx, "s1", 0, 1);
    let held = FlockGuard::try_lock(&tokens_lock_path(&fx.env, &cc()))
        .unwrap()
        .expect("nothing else holds the tokens lock");
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    assert_eq!(
        fx.engine.token_index_state(&cc()).unwrap().responses,
        0,
        "nothing was read"
    );
    assert_eq!(job_status(&fx, "token-index").last_error, None);
    drop(held);
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    assert_eq!(fx.engine.token_index_state(&cc()).unwrap().responses, 1);
}

#[test]
fn a_failing_refresh_is_recorded_in_status_and_keeps_its_cadence() {
    // D5.4's `lastError`, and D4.3's rule that a failed run keeps its cadence. A `locks` path
    // that is a regular file makes the tokens lock impossible to open.
    let fx = fx(true);
    let sink = Recorded::default();
    let (_, mut index) = token_jobs(vec![cc()], fx.board.clone(), false);
    append(&fx, "s1", 0, 1);
    let locks = fx.env.data_dir().join("locks");
    let _ = fs::remove_dir_all(&locks);
    fs::write(&locks, "").unwrap();
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    let error = job_status(&fx, "token-index").last_error;
    assert!(
        error
            .as_deref()
            .is_some_and(|e| e.starts_with("claude-code: ")),
        "{error:?}"
    );
    fs::remove_file(&locks).unwrap();
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    assert_eq!(
        job_status(&fx, "token-index").last_error,
        None,
        "a clean run clears it"
    );
}

#[test]
fn a_session_the_watch_sights_is_broadcast_as_token_index() {
    // §12: the watch's sightings move `changed_at_ms` too, so the TUI hears of evidence that no
    // refresh brought.
    let fx = fx(true);
    let sink = Recorded::default();
    let (mut sessions, _) = token_jobs(vec![cc()], fx.board.clone(), false);
    assert_eq!(
        run(&mut sessions, &fx, &sink),
        Ran::After(SESSIONS_EVERY_MS)
    );
    assert!(
        sink.token_index().is_empty(),
        "nothing sighted, nothing changed"
    );

    fx.process.set(
        4242,
        FakeProcess {
            exists: Some(true),
            ..FakeProcess::default()
        },
    );
    let records = fx.env.home.join(".claude/sessions");
    fs::create_dir_all(&records).unwrap();
    fs::write(
        records.join("4242.json"),
        json!({"pid": 4242, "sessionId": "s-live", "cwd": "/work/app",
               "startedAt": T0_MS - 60_000, "kind": "interactive"})
        .to_string(),
    )
    .unwrap();
    fx.clock.advance_ms(5_000);
    assert_eq!(
        run(&mut sessions, &fx, &sink),
        Ran::After(SESSIONS_EVERY_MS)
    );
    let events = sink.token_index();
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(events[0].1.changed_at_ms.is_some());
    assert_eq!(job_status(&fx, "sessions-watch").last_error, None);
}

/// A live session's record in the default home, as the sessions watch sights it.
fn live_session(fx: &Fx) {
    fx.process.set(
        4242,
        FakeProcess {
            exists: Some(true),
            ..FakeProcess::default()
        },
    );
    let records = fx.env.home.join(".claude/sessions");
    fs::create_dir_all(&records).unwrap();
    fs::write(
        records.join("4242.json"),
        json!({"pid": 4242, "sessionId": "s-live", "cwd": "/work/app",
               "startedAt": T0_MS - 60_000, "kind": "interactive"})
        .to_string(),
    )
    .unwrap();
}

#[test]
fn a_change_one_job_broadcast_is_not_broadcast_again_by_the_other() {
    // The two jobs share one record of what was broadcast (one `changed_at_ms` per provider), so
    // a change is announced once, whichever job saw it first.
    let fx = fx(true);
    let sink = Recorded::default();
    let (mut sessions, mut index) = token_jobs(vec![cc()], fx.board.clone(), false);
    live_session(&fx);
    assert_eq!(
        run(&mut sessions, &fx, &sink),
        Ran::After(SESSIONS_EVERY_MS)
    );
    let events = sink.token_index();
    assert_eq!(events.len(), 1, "{events:?}");
    let changed = events[0].1.changed_at_ms;
    assert!(changed.is_some());

    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    assert_eq!(
        fx.engine.token_index_state(&cc()).unwrap().changed_at_ms,
        changed,
        "the refresh found nothing new"
    );
    assert_eq!(sink.token_index().len(), 1, "announced once");
}

#[test]
#[cfg(feature = "test-hooks")]
fn a_failed_scan_still_broadcasts_a_change_the_state_shows() {
    // §12: the state is read after each call, a failed one included. The scan records the
    // live session, then fails at its launch check (injected, as a store busy past its timeout
    // would): the evidence it committed is readable, so it is announced now, not after a later
    // scan that succeeds.
    let fx = fx(true);
    let sink = Recorded::default();
    let (mut sessions, _) = token_jobs(vec![cc()], fx.board.clone(), false);
    live_session(&fx);
    fx.engine
        .open_launch(&cc(), &tagteam_core::AccountId::from_string("a@x.co"))
        .unwrap();
    fx.engine.fail_at(Some("scan-launch-check"));
    assert_eq!(
        run(&mut sessions, &fx, &sink),
        Ran::After(SESSIONS_EVERY_MS)
    );
    let error = job_status(&fx, "sessions-watch").last_error;
    assert!(
        error
            .as_deref()
            .is_some_and(|e| e.starts_with("claude-code: ")),
        "the scan's failure is the run's lastError: {error:?}"
    );
    let changed = fx.engine.token_index_state(&cc()).unwrap().changed_at_ms;
    assert!(changed.is_some(), "the premise: the scan changed the index");
    let events = sink.token_index();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].1.changed_at_ms, changed);
}

#[test]
fn an_interrupted_refresh_stops_the_scheduler_with_signal() {
    // D4.3, D4.6: a signal that reaches a call ends the run with `Stop(Signal)`, as an
    // interrupted provider run does, and the job reports nothing.
    let fx = fx(true);
    let sink = Recorded::default();
    let (_, mut index) = token_jobs(vec![cc()], fx.board.clone(), false);
    append(&fx, "s1", 0, 1);
    fx.engine.cancel().request(libc::SIGTERM);
    let call = fx.engine.refresh_tokens(
        &cc(),
        Wait::Try,
        None,
        fx.engine.cancel(),
        &mut |_: &IndexProgress| {},
    );
    assert!(
        matches!(call, Err(EngineError::Interrupted(_))),
        "the call itself ends in an interruption: {call:?}"
    );
    assert_eq!(run(&mut index, &fx, &sink), Ran::Stop(StopReason::Signal));
    assert!(sink.0.lock().unwrap().is_empty());
}

#[test]
fn a_store_that_cannot_be_read_is_recorded_in_status_and_keeps_the_cadence() {
    // `serving` is no provider's error: `lastError` is the store's own message (it says "store
    // error" once), and both jobs keep their cadence and emit nothing (the log has the kind).
    let fx = fx(false);
    let sink = Recorded::default();
    let (mut sessions, mut index) = token_jobs(vec![cc()], fx.board.clone(), false);
    fs::create_dir_all(fx.env.data_dir()).unwrap();
    fs::write(
        fx.env.data_dir().join("tagteam.db"),
        "not a database".repeat(100),
    )
    .unwrap();
    assert_eq!(
        run(&mut sessions, &fx, &sink),
        Ran::After(SESSIONS_EVERY_MS)
    );
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    for name in ["sessions-watch", "token-index"] {
        let error = job_status(&fx, name).last_error;
        assert!(
            error
                .as_deref()
                .is_some_and(|e| e.starts_with("store error: ") && !e.contains("store: store")),
            "{name}: {error:?}"
        );
    }
    assert!(sink.0.lock().unwrap().is_empty());
}

#[test]
fn without_a_store_the_jobs_do_nothing_and_create_nothing() {
    // D4.1 step 4: the daemon never creates the store; the token jobs wait for one, as the
    // provider jobs do.
    let fx = fx(false);
    let sink = Recorded::default();
    let (mut sessions, mut index) = token_jobs(vec![cc()], fx.board.clone(), false);
    append(&fx, "s1", 0, 1);
    assert_eq!(
        run(&mut sessions, &fx, &sink),
        Ran::After(SESSIONS_EVERY_MS)
    );
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    assert!(!fx.env.data_dir().exists(), "no data directory was created");
    assert!(sink.0.lock().unwrap().is_empty());
}

#[test]
fn with_a_newer_schema_the_jobs_wait_for_the_upgrade() {
    // D4.1 step 4: every job waits, and the upgrade watch ends it with exit 75 (D4.5).
    let fx = fx(true);
    let sink = Recorded::default();
    let (mut sessions, mut index) = token_jobs(vec![cc()], fx.board.clone(), true);
    append(&fx, "s1", 0, 1);
    assert_eq!(
        run(&mut sessions, &fx, &sink),
        Ran::After(SESSIONS_EVERY_MS)
    );
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    assert_eq!(fx.engine.token_index_state(&cc()).unwrap().responses, 0);
    assert!(sink.0.lock().unwrap().is_empty());
}

#[test]
fn token_index_is_the_index_state_in_camel_case_and_the_client_reads_it() {
    // D5.5's reserved kind, with token report §12's fields; times in ISO 8601 UTC.
    let state = IndexState {
        refreshed_at_ms: Some(1_792_010_640_000),
        changed_at_ms: Some(1_792_010_670_500),
        files: 3,
        responses: 120,
        skipped_lines: 1,
        unreadable_files: 0,
        oldest_ms: Some(1_790_841_600_000),
        newest_ms: Some(1_792_010_640_000),
    };
    let v = daemon_event_json(
        &DaemonEvent::TokenIndex {
            provider: cc(),
            state,
        },
        T0_MS / 1000,
    );
    assert_eq!(
        v,
        json!({"schemaVersion": 1, "event": "token-index", "ts": "2026-10-14T20:45:00Z",
               "provider": "claude-code", "refreshedAt": "2026-10-14T20:44:00Z",
               "changedAt": "2026-10-14T20:44:30Z", "files": 3, "responses": 120,
               "skippedLines": 1, "unreadableFiles": 0, "oldest": "2026-10-01T08:00:00Z",
               "newest": "2026-10-14T20:44:00Z"})
    );
    assert!(
        v.to_string()
            .starts_with(r#"{"schemaVersion":1,"event":"token-index","ts":"#),
        "the envelope comes first"
    );
    assert_eq!(
        parse_event(&v),
        Some(Event::TokenIndex {
            provider: "claude-code".into(),
            raw: v.clone(),
        })
    );

    let empty = IndexState {
        refreshed_at_ms: None,
        changed_at_ms: None,
        files: 0,
        responses: 0,
        skipped_lines: 0,
        unreadable_files: 0,
        oldest_ms: None,
        newest_ms: None,
    };
    let v = daemon_event_json(
        &DaemonEvent::TokenIndex {
            provider: cc(),
            state: empty,
        },
        T0_MS / 1000,
    );
    for key in ["refreshedAt", "changedAt", "oldest", "newest"] {
        assert!(v[key].is_null(), "{key}: {v}");
    }
}
