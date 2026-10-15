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

    /// `serving`, for a run: a failure to read the store is not one provider's, so it is logged
    /// at WARN with the error's kind only (never its text, §14.2) and is the run's `lastError`,
    /// as `store: <message>`; the run then serves no provider and keeps its cadence.
    fn serving_or_note(&self, engine: &Engine, error: &mut Option<String>) -> Vec<ProviderId> {
        self.serving(engine).unwrap_or_else(|e| {
            tracing::warn!(kind = e.kind(), "cannot read the store for the token jobs");
            error.get_or_insert_with(|| format!("store: {e}"));
            Vec::new()
        })
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

/// What a run does with one call's error: an interruption stops the scheduler (D4.6); anything
/// else is logged at WARN and, if it is the run's first, kept for `lastError`.
fn note(
    result: Result<(), EngineError>,
    provider: &ProviderId,
    what: &str,
    error: &mut Option<String>,
) -> Option<Ran> {
    match result {
        Ok(()) => None,
        Err(EngineError::Interrupted(_)) => Some(Ran::Stop(StopReason::Signal)),
        Err(e) => {
            tracing::warn!(provider = %provider, error = %e, "{what} failed");
            error.get_or_insert_with(|| format!("{provider}: {e}"));
            None
        }
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
            if let Some(stop) = note(called, p, "sighting sessions", &mut error) {
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
            if let Some(stop) = note(refreshed, p, "the token index refresh", &mut error) {
                return stop;
            }
            let read = self.common.after_call(engine, p, sink);
            if let Some(stop) = note(read, p, "reading the token index", &mut error) {
                return stop;
            }
        }
        self.common.report(TOKEN_INDEX, engine, delay, error);
        Ran::After(delay)
    }
}
