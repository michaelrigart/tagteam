//! The token report's two scheduler jobs (D13; token report spec §12). `sessions-watch`
//! records the sessions every home is running, every 5 s (§6.2). `token-index` reads new
//! transcript lines into the index every 60 s, in slices of about a second (§5.2). After each
//! call, each job reads the index's state, and a `changed_at_ms` that moved since the last
//! broadcast becomes one `token-index` event (D5.5), so the TUI re-queries. Both take only the
//! token report's own lock (§5.1), never an engine lock or the mutation lock.
//!
//! The event carries `token_index_state` exactly as the engine stores it: the daemon accumulates
//! nothing, and the state's `skipped_lines` is the engine's count over every call of the last
//! completed pass. `token-index` polls: it does not watch `projects/` for changes (token report
//! §12's filesystem trigger is out of scope), it just runs again in 60 s, or at once while a
//! refresh is incomplete.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tagteam_core::ProviderId;
use tagteam_engine::tokens::{RefreshOutcome, Wait};
use tagteam_engine::{Engine, EngineError};

use crate::events::{DaemonEvent, DaemonSink, StopReason};
use crate::scheduler::{Job, JobCx, Ran};
use crate::status::{JobStatus, StatusBoard};

pub const SESSIONS_WATCH: &str = "sessions-watch";
pub const TOKEN_INDEX: &str = "token-index";
/// Token report §12: sightings "about every 5 s".
pub const SESSIONS_EVERY_MS: i64 = 5_000;
/// Token report §12: a refresh "about every 60 s", and at once after an incomplete one.
pub const INDEX_EVERY_MS: i64 = 60_000;
/// One refresh slice (§5.2's budget): the scheduler runs one job at a time.
pub const INDEX_SLICE: Duration = Duration::from_secs(1);

/// What both jobs share: the providers they serve, the board, and per provider the
/// `changed_at_ms` its last `token-index` event carried.
struct Common {
    providers: Vec<ProviderId>,
    board: Arc<StatusBoard>,
    schema_newer: bool,
    broadcast: Mutex<BTreeMap<ProviderId, i64>>,
    logged: Mutex<Logged>,
}

/// The failures the jobs last warned of, by message, as the store watch remembers its own
/// (`watch.rs`): an error that repeats is logged at DEBUG, and a call that succeeds forgets its
/// failure, so a later one is news again. The message is only compared, never logged (§14.2).
#[derive(Default)]
struct Logged {
    /// The store could not be read: one failure for both jobs.
    store: Option<String>,
    /// A call that failed, per call and provider.
    calls: BTreeMap<(&'static str, ProviderId), String>,
}

impl Common {
    /// The providers whose index this daemon keeps: those with transcripts (token report
    /// §3.2). None while there is no store (the daemon never creates one, D4.1 step 4), or
    /// while its schema is newer than this build (the upgrade watch ends that, D4.5).
    fn serving(&self, engine: &Engine) -> Result<Vec<ProviderId>, EngineError> {
        if self.schema_newer || engine.existing_store()?.is_none() {
            return Ok(Vec::new());
        }
        Ok(self
            .providers
            .iter()
            .filter(|p| {
                engine
                    .provider(p)
                    .is_ok_and(|provider| provider.capabilities().transcripts)
            })
            .cloned()
            .collect())
    }

    /// `serving`, for a run: a failure to read the store is not one provider's, so it is the
    /// run's `lastError` (its own text already says "store error", so it is not prefixed again);
    /// the run then serves no provider and keeps its cadence. The log has the error's kind only
    /// (§14.2), at WARN when the failure is a new one and at DEBUG when it repeats.
    fn serving_or_note(&self, engine: &Engine, error: &mut Option<String>) -> Vec<ProviderId> {
        match self.serving(engine) {
            Ok(providers) => {
                self.lock_logged().store = None;
                providers
            }
            Err(e) => {
                let message = e.to_string();
                let new =
                    self.lock_logged().store.replace(message.clone()).as_ref() != Some(&message);
                if new {
                    tracing::warn!(kind = e.kind(), "cannot read the store for the token jobs");
                } else {
                    tracing::debug!(
                        kind = e.kind(),
                        "the store is still unreadable for the token jobs"
                    );
                }
                error.get_or_insert_with(|| match e {
                    EngineError::Store(_) => message,
                    _ => format!("store: {message}"),
                });
                Vec::new()
            }
        }
    }

    fn lock_logged(&self) -> std::sync::MutexGuard<'_, Logged> {
        self.logged.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What a run does with one call's result: an interruption stops the scheduler (D4.6);
    /// anything else is logged and, if it is the run's first, kept for `lastError`. The log has
    /// the error's kind only (§14.2; the full message is in `lastError`, which goes over the 0600
    /// socket), at WARN when it differs from the last one logged for this call and provider and
    /// at DEBUG when it repeats. A success forgets the last failure.
    fn note(
        &self,
        result: Result<(), EngineError>,
        provider: &ProviderId,
        what: &'static str,
        error: &mut Option<String>,
    ) -> Option<Ran> {
        let key = (what, provider.clone());
        match result {
            Ok(()) => {
                self.lock_logged().calls.remove(&key);
                None
            }
            Err(EngineError::Interrupted(_)) => Some(Ran::Stop(StopReason::Signal)),
            Err(e) => {
                let message = e.to_string();
                let new = self
                    .lock_logged()
                    .calls
                    .insert(key, message.clone())
                    .as_ref()
                    != Some(&message);
                if new {
                    tracing::warn!(provider = %provider, kind = e.kind(), "{what} failed");
                } else {
                    tracing::debug!(provider = %provider, kind = e.kind(), "{what} failed again");
                }
                error.get_or_insert_with(|| format!("{provider}: {message}"));
                None
            }
        }
    }

    /// Token report §12: reads `provider`'s index state, and emits `token-index` when its
    /// `changed_at_ms` moved since the last event for it. An index that never changed is not
    /// broadcast.
    fn after_call(
        &self,
        engine: &Engine,
        provider: &ProviderId,
        sink: &dyn DaemonSink,
    ) -> Result<(), EngineError> {
        let state = engine.token_index_state(provider)?;
        let Some(changed) = state.changed_at_ms else {
            return Ok(());
        };
        {
            let mut last = self
                .broadcast
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if last.get(provider) == Some(&changed) {
                return Ok(());
            }
            last.insert(provider.clone(), changed);
        }
        sink.emit(&DaemonEvent::TokenIndex {
            provider: provider.clone(),
            state,
        });
        Ok(())
    }

    /// D5.4's `jobs` entry for one run.
    fn report(&self, name: &str, engine: &Engine, delay_ms: i64, error: Option<String>) {
        let now_s = engine.now_ms().div_euclid(1000);
        self.board.set_job(JobStatus {
            name: name.to_owned(),
            last_run_at: Some(now_s),
            next_run_at: Some(now_s + delay_ms.div_euclid(1000)),
            last_error: error,
        });
    }
}

/// Both jobs, sharing one record of what was broadcast.
pub fn token_jobs(
    providers: Vec<ProviderId>,
    board: Arc<StatusBoard>,
    schema_newer: bool,
) -> (SessionsWatch, TokenIndexJob) {
    let common = Arc::new(Common {
        providers,
        board,
        schema_newer,
        broadcast: Mutex::new(BTreeMap::new()),
        logged: Mutex::default(),
    });
    (
        SessionsWatch {
            common: Arc::clone(&common),
        },
        TokenIndexJob {
            common,
            slice: INDEX_SLICE,
        },
    )
}

/// `sessions-watch`: `sight_sessions` for each provider, every 5 s (token report §6.2, §12).
pub struct SessionsWatch {
    common: Arc<Common>,
}

impl Job for SessionsWatch {
    fn name(&self) -> String {
        SESSIONS_WATCH.to_owned()
    }

    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
        let (engine, sink) = (cx.engine, cx.sink);
        let mut error = None;
        let providers = self.common.serving_or_note(engine, &mut error);
        for p in &providers {
            let called = engine
                .sight_sessions(p)
                .and_then(|_| self.common.after_call(engine, p, sink));
            if let Some(stop) = self.common.note(called, p, "sighting sessions", &mut error) {
                return stop;
            }
        }
        self.common
            .report(SESSIONS_WATCH, engine, SESSIONS_EVERY_MS, error);
        Ran::After(SESSIONS_EVERY_MS)
    }
}

/// `token-index`: one budgeted refresh slice per provider, every 60 s, and again at once while
/// a refresh is incomplete (token report §5.2, §12).
pub struct TokenIndexJob {
    common: Arc<Common>,
    slice: Duration,
}

impl TokenIndexJob {
    /// A refresh slice other than `INDEX_SLICE`. Under `Duration::ZERO` the budget is spent from
    /// the start, so each call stops about one line past its deadline (token report §5.2) and
    /// reports `complete: false` until the transcripts are read.
    pub fn with_slice(mut self, slice: Duration) -> Self {
        self.slice = slice;
        self
    }
}

impl Job for TokenIndexJob {
    fn name(&self) -> String {
        TOKEN_INDEX.to_owned()
    }

    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
        let (engine, sink) = (cx.engine, cx.sink);
        let mut error = None;
        let mut delay = INDEX_EVERY_MS;
        let providers = self.common.serving_or_note(engine, &mut error);
        for p in &providers {
            let refreshed = engine
                .refresh_tokens(p, Wait::Try, Some(self.slice), engine.cancel(), &mut |_| {})
                .map(|outcome| {
                    if matches!(outcome, RefreshOutcome::Done(ref report) if !report.complete) {
                        delay = 0;
                    }
                });
            if let Some(stop) =
                self.common
                    .note(refreshed, p, "the token index refresh", &mut error)
            {
                return stop;
            }
            let read = self.common.after_call(engine, p, sink);
            if let Some(stop) = self
                .common
                .note(read, p, "reading the token index", &mut error)
            {
                return stop;
            }
        }
        self.common.report(TOKEN_INDEX, engine, delay, error);
        Ran::After(delay)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::status::StatusBoard;
    use crate::testutil::{Emitted, Fx, board, capture_logs, cc};

    const MARKER: &str = "secret-marker";

    fn run(job: &mut dyn Job, fx: &Fx, sink: &Emitted<'_>) -> Ran {
        let mut jitter = || 0.0;
        job.run(&mut JobCx {
            engine: &fx.engine,
            sink,
            jitter: &mut jitter,
        })
    }

    fn last_error(board: &StatusBoard, name: &str) -> Option<String> {
        let jobs = board.snapshot().jobs;
        jobs.into_iter()
            .find(|j| j.name == name)
            .unwrap_or_else(|| panic!("{name} is not on the board"))
            .last_error
    }

    /// The lines at WARN, trimmed.
    fn warns(lines: &[String]) -> Vec<&str> {
        lines
            .iter()
            .map(|l| l.trim_start())
            .filter(|l| l.starts_with("WARN"))
            .collect()
    }

    /// The repeats the jobs logged: the DEBUG lines about a failure, not the engine's own.
    fn repeats(lines: &[String]) -> usize {
        lines
            .iter()
            .map(|l| l.trim_start())
            .filter(|l| l.starts_with("DEBUG") && l.contains("kind="))
            .count()
    }

    /// A store file that is not a database, so the store cannot be read.
    fn corrupt_store(fx: &Fx) {
        fx.remove_store();
        fs::create_dir_all(fx.env.data_dir()).unwrap();
        fs::write(fx.engine.store_file(), "not a database".repeat(100)).unwrap();
    }

    /// A directory where the store file should be: SQLite's error quotes the file's path.
    fn store_path_is_a_directory(fx: &Fx) {
        fx.remove_store();
        fs::create_dir_all(fx.engine.store_file()).unwrap();
    }

    /// A `locks` path that is a regular file, so the tokens lock cannot be opened.
    fn block_locks(fx: &Fx) {
        let locks = fx.env.data_dir().join("locks");
        let _ = fs::remove_dir_all(&locks);
        fs::write(&locks, "").unwrap();
    }

    #[test]
    fn a_store_that_stays_unreadable_warns_once_and_again_after_a_success() {
        let fx = Fx::marked(MARKER);
        let sink = Emitted::new(&fx.clock);
        let (mut sessions, mut index) = token_jobs(vec![cc()], board(), false);
        corrupt_store(&fx);
        let ((), lines) = capture_logs(|| {
            for _ in 0..3 {
                run(&mut sessions, &fx, &sink);
                run(&mut index, &fx, &sink);
            }
        });
        let warned = warns(&lines);
        assert_eq!(warned.len(), 1, "{lines:#?}");
        assert!(warned[0].contains("kind=\"store\""), "{}", warned[0]);
        assert_eq!(repeats(&lines), 5, "{lines:#?}");

        // A run that reads the store forgets the failure: the next one is news again.
        fx.remove_store();
        let ((), lines) = capture_logs(|| {
            run(&mut sessions, &fx, &sink);
            run(&mut index, &fx, &sink);
        });
        assert!(warns(&lines).is_empty(), "{lines:#?}");
        corrupt_store(&fx);
        let ((), lines) = capture_logs(|| {
            run(&mut sessions, &fx, &sink);
            run(&mut index, &fx, &sink);
        });
        assert_eq!(warns(&lines).len(), 1, "{lines:#?}");
    }

    #[test]
    fn a_call_that_keeps_failing_warns_once_and_again_after_a_success() {
        let fx = Fx::marked(MARKER);
        let board = board();
        let sink = Emitted::new(&fx.clock);
        let (_, mut index) = token_jobs(vec![cc()], board.clone(), false);
        fx.engine.store().unwrap();
        block_locks(&fx);
        let ((), lines) = capture_logs(|| {
            for _ in 0..3 {
                run(&mut index, &fx, &sink);
            }
        });
        let warned = warns(&lines);
        assert_eq!(warned.len(), 1, "{lines:#?}");
        assert!(
            warned[0].contains("provider=claude-code") && warned[0].contains("kind="),
            "{}",
            warned[0]
        );
        assert_eq!(repeats(&lines), 2, "{lines:#?}");
        assert!(
            last_error(&board, TOKEN_INDEX).is_some_and(|e| e.starts_with("claude-code: ")),
            "status keeps the failure"
        );

        fs::remove_file(fx.env.data_dir().join("locks")).unwrap();
        let ((), lines) = capture_logs(|| {
            run(&mut index, &fx, &sink);
        });
        assert!(warns(&lines).is_empty(), "{lines:#?}");
        assert_eq!(last_error(&board, TOKEN_INDEX), None);

        block_locks(&fx);
        let ((), lines) = capture_logs(|| {
            run(&mut index, &fx, &sink);
        });
        assert_eq!(warns(&lines).len(), 1, "{lines:#?}");
    }

    #[test]
    fn no_log_line_carries_an_errors_text_or_a_path() {
        // §14.2: status keeps the message, which here names the path; the log never does.
        let fx = Fx::marked(MARKER);
        let board = board();
        let sink = Emitted::new(&fx.clock);
        let (mut sessions, mut index) = token_jobs(vec![cc()], board.clone(), false);

        store_path_is_a_directory(&fx);
        let ((), lines) = capture_logs(|| {
            run(&mut sessions, &fx, &sink);
            run(&mut index, &fx, &sink);
        });
        let status = last_error(&board, SESSIONS_WATCH).unwrap();
        assert!(status.contains(MARKER), "the premise: {status}");
        assert!(!warns(&lines).is_empty(), "{lines:#?}");
        let logged = lines.join("\n");
        assert!(
            !logged.contains(MARKER) && !logged.contains("unable to open"),
            "{logged}"
        );

        fs::remove_dir(fx.engine.store_file()).unwrap();
        fx.engine.store().unwrap();
        block_locks(&fx);
        let ((), lines) = capture_logs(|| {
            run(&mut index, &fx, &sink);
        });
        let status = last_error(&board, TOKEN_INDEX).unwrap();
        assert!(status.contains("File exists"), "the premise: {status}");
        assert!(!warns(&lines).is_empty(), "{lines:#?}");
        let logged = lines.join("\n");
        assert!(!logged.contains("File exists"), "{logged}");
    }

    #[test]
    fn the_stores_error_names_the_store_once() {
        let fx = Fx::marked(MARKER);
        let board = board();
        let sink = Emitted::new(&fx.clock);
        let (mut sessions, _) = token_jobs(vec![cc()], board.clone(), false);
        corrupt_store(&fx);
        run(&mut sessions, &fx, &sink);
        assert_eq!(
            last_error(&board, SESSIONS_WATCH).as_deref(),
            Some("store error: file is not a database")
        );
    }
}
