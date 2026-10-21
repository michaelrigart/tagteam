//! `refresh_tokens` (spec §5): the incremental index of a provider's transcripts. At most four
//! scoped workers read one file at a time, each holding one chunk; one writer, on the calling
//! thread, commits their lines in batches, with the offsets just past them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::num::NonZeroUsize;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use tagteam_core::ProviderId;
use tagteam_provider::{Cancel, FlockGuard, LockError, Provider, UsageLine};
use tracing::Level;

use super::reader::{Checkpoint, Limits, LineReader, Step, Stop, open_at, past, unchanged};
use super::{lock_until_free, tokens_lock_path};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::store::{Store, TokenFileRow, UsageRow, merge_line, usage_key};

/// At most this many files are read at once (spec §5.2 step 4).
const MAX_WORKERS: usize = 4;
/// A batch commits at most this many responses, so its transaction holds the write lock for
/// about 50 ms at most (spec §5.2 step 4) ...
const BATCH_ROWS: usize = 2_000;
/// ... and commits once its first line has waited this long.
const BATCH_WAIT: Duration = Duration::from_millis(50);
const DAY_MS: i64 = 86_400_000;

/// How a refresh takes the provider's tokens lock (spec §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// Waits for another refresh to finish, cancellably: `tagteam tokens`.
    Wait,
    /// Returns `Busy` at once while another refresh holds it: the TUI's timer and the daemon.
    Try,
}

/// What a refresh came to (spec §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    Done(IndexReport),
    /// Another process is refreshing (`Wait::Try`).
    Busy,
}

/// How far a refresh has come (spec §5): the files and bytes it has read of those it found to
/// read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexProgress {
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// One refresh (spec §5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexReport {
    /// False when the budget ran out first, or the prune it ends with was stopped by it; call
    /// again to go on. (A cancelled refresh returns the interruption instead of a report.)
    pub complete: bool,
    pub files_read: u64,
    pub responses_added: u64,
    pub responses_updated: u64,
    pub skipped_lines: u64,
    /// Evidence rows its `sight_sessions` added, extended or closed.
    pub evidence_changed: u64,
    pub took_ms: u64,
}

/// A file step 2 found changed (spec §5.2).
struct Job {
    path: PathBuf,
    stored: Option<TokenFileRow>,
    /// The bytes it likely holds past its stored offset, for progress.
    bytes: u64,
}

/// How a file's read ended in this refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    /// Read to its last newline, or found rewritten, so its next read starts over.
    Done,
    /// Stopped by the budget or the cancel token; the next refresh goes on from its offset.
    Stopped,
}

/// How a worker's turn at a file came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Turn {
    /// The file was read, as far as `End` says.
    Read(End),
    /// The file was unreadable or gone: skipped, and not the worker's one guaranteed file under
    /// a spent budget, since it read nothing.
    Skipped,
}

/// What a worker sends the writer.
enum Msg {
    /// One chunk's rows and the checkpoint just past them; `end` with a file's last chunk.
    Chunk {
        rows: Vec<UsageRow>,
        skipped: u64,
        file: TokenFileRow,
        read: u64,
        end: Option<End>,
    },
    /// A file that could not be opened or read: skipped, and tried again next time (spec §5.4).
    Unreadable,
    /// A file deleted between the listing and the open: nothing to read, nothing to retry.
    Gone,
}

/// What the workers share.
struct Work<'a> {
    p: &'a dyn Provider,
    provider: &'a ProviderId,
    jobs: &'a [Job],
    next: AtomicUsize,
    limits: Limits,
    deadline: Option<Instant>,
    cancel: &'a Cancel,
    /// Set when the writer fails: the workers stop.
    halt: AtomicBool,
    /// Lines older than this are not inserted (spec §4 Retention).
    cutoff_ms: i64,
    /// The most bytes any reader held at once.
    peak: AtomicUsize,
}

impl Work<'_> {
    /// One worker: the files in turn, one at a time, until none is left, the deadline has
    /// passed once it has read one, the token is set, or the writer is gone.
    fn run(&self, tx: &SyncSender<Msg>) {
        let mut first = true;
        loop {
            let spent = !first && past(self.deadline);
            if spent || self.halt.load(Ordering::Relaxed) || self.cancel.requested().is_some() {
                return;
            }
            let Some(job) = self.jobs.get(self.next.fetch_add(1, Ordering::Relaxed)) else {
                return;
            };
            match self.read(job, tx) {
                Some(Turn::Read(End::Done)) => first = false,
                Some(Turn::Skipped) => {}
                Some(Turn::Read(End::Stopped)) | None => return,
            }
        }
    }

    /// One file, from where `open_at` starts it, a message per chunk. `None` when the writer is
    /// gone.
    fn read(&self, job: &Job, tx: &SyncSender<Msg>) -> Option<Turn> {
        let open = match open_at(&job.path, job.stored.as_ref()) {
            Ok(open) => open,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                tx.send(Msg::Gone).ok()?;
                return Some(Turn::Skipped);
            }
            Err(e) => return self.unreadable(job, &e, tx),
        };
        let mut reader = LineReader::new(&open, self.limits);
        let (mut rows, mut skipped, mut long) = (Vec::new(), 0, 0);
        let mut sent = open.start();
        loop {
            let step = reader.step(self.deadline, self.cancel, &mut |line: &[u8]| {
                match self.p.parse_usage_line(line) {
                    Ok(Some(u)) if u.at_ms >= self.cutoff_ms => rows.push(row_of(self.provider, u)),
                    // Another type, a line with no usage, or one older than the window.
                    Ok(_) => {}
                    // Spec §5.4: skipped and counted, never a failure.
                    Err(_) => skipped += 1,
                }
            });
            let step = match step {
                Ok(step) => step,
                Err(e) => return self.unreadable(job, &e, tx),
            };
            skipped += reader.long_lines() - long;
            long = reader.long_lines();
            let mut end = match step {
                Step::More => None,
                Step::Stop(Stop::Eof) => Some(End::Done),
                Step::Stop(Stop::Deadline | Stop::Cancelled) => Some(End::Stopped),
            };
            // Inside one long line: nothing to commit yet.
            if end.is_none() && rows.is_empty() && reader.consumed() == sent {
                continue;
            }
            let file = match reader.checkpoint(step == Step::Stop(Stop::Eof)) {
                Ok(Checkpoint::Valid(file)) => file,
                Ok(Checkpoint::Rewritten(file)) => {
                    tracing::debug!(
                        path = %job.path.display(),
                        "a transcript changed while it was read; the next refresh reads it again"
                    );
                    end = Some(End::Done);
                    file
                }
                Err(e) => return self.unreadable(job, &e, tx),
            };
            self.peak.fetch_max(reader.peak(), Ordering::Relaxed);
            let read = reader.consumed() - sent;
            sent = reader.consumed();
            let msg = Msg::Chunk {
                rows: std::mem::take(&mut rows),
                skipped: std::mem::take(&mut skipped),
                file,
                read,
                end,
            };
            tx.send(msg).ok()?;
            if let Some(end) = end {
                return Some(Turn::Read(end));
            }
        }
    }

    /// Spec §5.4: a file that cannot be opened or read is skipped with a warning (its path at
    /// debug only, §13), and tried again next time; its checkpoint stays where it was.
    fn unreadable(&self, job: &Job, e: &io::Error, tx: &SyncSender<Msg>) -> Option<Turn> {
        tracing::warn!(kind = ?e.kind(), "a transcript could not be read; the next refresh tries it again");
        tracing::debug!(path = %job.path.display(), error = %e, "the transcript could not be read");
        tx.send(Msg::Unreadable).ok()?;
        Some(Turn::Skipped)
    }
}

/// A usage line as the index stores it, keyed by its response (spec §2.2, §4).
fn row_of(provider: &ProviderId, u: UsageLine) -> UsageRow {
    UsageRow {
        key: usage_key(provider, &u.message_id, &u.request_id),
        session_id: u.session_id,
        project: u.project,
        at_ms: u.at_ms,
        model: u.model,
        sidechain: u.sidechain,
        counts: u.counts,
        speed: u.speed,
        finalized: u.final_line,
    }
}

/// What the writer counts.
#[derive(Default)]
struct Tally {
    added: u64,
    updated: u64,
    skipped: u64,
    unreadable: u64,
    files_read: u64,
    files_done: u64,
    bytes_done: u64,
}

/// The responses waiting for their transaction, one row each, and the checkpoints whose lines
/// are all in it or in a committed one.
#[derive(Default)]
struct Batch {
    rows: Vec<UsageRow>,
    keys: HashMap<i64, usize>,
    files: BTreeMap<String, TokenFileRow>,
    removed: Vec<String>,
    /// The skipped lines of the chunks whose checkpoints are in `files`.
    skipped: u64,
    since: Option<Instant>,
}

/// The one writer (spec §5.2 step 4), on the calling thread.
struct Writer<'a> {
    engine: &'a Engine,
    store: &'a Store,
    provider: &'a ProviderId,
    /// The purge generation the refresh began: a batch written after a purge emptied the index
    /// is refused (`TokenIndexPurged`).
    epoch: i64,
    batch: Batch,
    tally: Tally,
    total: IndexProgress,
}

impl Writer<'_> {
    /// Takes the workers' messages until every worker is done, reporting progress after each,
    /// then commits what is left, the rows of files no longer found among it.
    fn run(
        &mut self,
        rx: Receiver<Msg>,
        progress: &mut dyn FnMut(&IndexProgress),
    ) -> Result<(), EngineError> {
        loop {
            let received = match self.batch.since {
                Some(since) => rx.recv_timeout(BATCH_WAIT.saturating_sub(since.elapsed())),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match received {
                Ok(msg) => {
                    self.take(msg)?;
                    if self
                        .batch
                        .since
                        .is_some_and(|since| since.elapsed() >= BATCH_WAIT)
                    {
                        self.commit()?;
                    }
                    progress(&IndexProgress {
                        files_done: self.tally.files_done,
                        bytes_done: self.tally.bytes_done,
                        ..self.total
                    });
                }
                Err(RecvTimeoutError::Timeout) => self.commit()?,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if self.batch.since.is_some() || !self.batch.removed.is_empty() {
            self.commit()?;
        }
        Ok(())
    }

    fn take(&mut self, msg: Msg) -> Result<(), EngineError> {
        let Msg::Chunk {
            rows,
            skipped,
            file,
            read,
            end,
        } = msg
        else {
            if matches!(msg, Msg::Unreadable) {
                self.tally.unreadable += 1;
            }
            self.tally.files_done += 1;
            return Ok(());
        };
        self.tally.skipped += skipped;
        self.tally.bytes_done += read;
        self.batch.since.get_or_insert_with(Instant::now);
        for row in rows {
            if let Some(&i) = self.batch.keys.get(&row.key) {
                merge_line(&mut self.batch.rows[i], &row);
                continue;
            }
            // Full before this chunk's rows are all in: its checkpoint waits for the rest.
            if self.batch.rows.len() == BATCH_ROWS {
                self.commit()?;
                self.batch.since = Some(Instant::now());
            }
            self.batch.keys.insert(row.key, self.batch.rows.len());
            self.batch.rows.push(row);
        }
        self.batch.files.insert(file.path.clone(), file);
        self.batch.skipped += skipped;
        if self.batch.rows.len() == BATCH_ROWS {
            self.commit()?;
        }
        match end {
            Some(End::Done) => {
                self.tally.files_read += 1;
                self.tally.files_done += 1;
            }
            Some(End::Stopped) => self.tally.files_read += 1,
            None => {}
        }
        Ok(())
    }

    /// One transaction: the batch's responses by the merge rule, its checkpoints, and the rows
    /// of files no longer found (spec §5.2 steps 2 and 4).
    fn commit(&mut self) -> Result<(), EngineError> {
        let files: Vec<TokenFileRow> = std::mem::take(&mut self.batch.files)
            .into_values()
            .collect();
        let outcome = self.store.commit_token_batch(
            self.provider,
            self.epoch,
            &self.batch.rows,
            &files,
            &self.batch.removed,
            self.batch.skipped,
            self.engine.now_ms(),
        )?;
        tracing::debug!(
            responses = self.batch.rows.len(),
            files = files.len(),
            added = outcome.added,
            updated = outcome.updated,
            "committed a token batch"
        );
        self.tally.added += outcome.added;
        self.tally.updated += outcome.updated;
        self.batch.rows.clear();
        self.batch.keys.clear();
        self.batch.removed.clear();
        self.batch.skipped = 0;
        self.batch.since = None;
        hooks::point(self.engine, "refresh-tokens-committed")?;
        Ok(())
    }
}

impl Engine {
    /// Spec §5: brings the provider's index up to date with its transcripts. It takes the
    /// provider's tokens lock, waiting for another refresh (`Wait::Wait`, cancellably) or
    /// returning `Busy` (`Wait::Try`). With a `budget` it stops at the next line boundary once
    /// the budget is spent, commits what it read, and returns `complete: false`; the next call
    /// goes on from there. A cancelled refresh commits what it parsed, then returns the
    /// interruption.
    pub fn refresh_tokens(
        &self,
        provider: &ProviderId,
        wait: Wait,
        budget: Option<Duration>,
        cancel: &Cancel,
        progress: &mut dyn FnMut(&IndexProgress),
    ) -> Result<RefreshOutcome, EngineError> {
        Ok(self
            .refresh_with(provider, wait, budget, cancel, progress, Limits::SPEC)?
            .0)
    }

    /// `refresh_tokens` with the reader's sizes; also returns the most bytes any reader held.
    pub(crate) fn refresh_with(
        &self,
        provider: &ProviderId,
        wait: Wait,
        budget: Option<Duration>,
        cancel: &Cancel,
        progress: &mut dyn FnMut(&IndexProgress),
        limits: Limits,
    ) -> Result<(RefreshOutcome, usize), EngineError> {
        let started_ms = self.now_ms();
        let p = self.provider(provider)?;
        if !p.capabilities().transcripts {
            return Err(EngineError::InvalidInput(format!(
                "{} has no transcripts",
                p.display_name()
            )));
        }
        // Step 1, holding no lock: sightings are written without the tokens lock (spec §5.1),
        // and so nothing is taken while it is held. A scan that fails loses its evidence, never
        // the refresh.
        let evidence_changed = match self.scan_sessions(p.as_ref()) {
            Ok(changed) => changed,
            Err(EngineError::Interrupted(signal)) => return Err(EngineError::Interrupted(signal)),
            Err(e) => {
                tracing::warn!(
                    kind = e.kind(),
                    "could not record the sessions found for the token report"
                );
                tracing::debug!(error = %e, "the session scan failed");
                0
            }
        };
        let path = tokens_lock_path(&self.env, provider);
        hooks::point(self, "refresh-tokens-lock")?;
        let _lock = match wait {
            Wait::Try => match FlockGuard::try_lock(&path)? {
                Some(lock) => lock,
                None => return Ok((RefreshOutcome::Busy, 0)),
            },
            Wait::Wait => lock_until_free(&path, cancel).map_err(|e| match e {
                LockError::Interrupted { signal, .. } => EngineError::Interrupted(signal),
                e => EngineError::Lock(e),
            })?,
        };
        let store = self.store()?;
        // The purge generation the refresh writes under (spec §13), begun anew when a purge left
        // none. A full purge that started before this provider had token rows does not wait for
        // its lock; once it has emptied the index, every later write of this refresh is refused,
        // and the next one begins a generation of its own.
        let epoch = store.begin_token_refresh(provider, self.now_ms())?;
        hooks::point(self, "refresh-tokens-locked")?;
        let retention = i64::from(self.settings().tokens_retention_days).saturating_mul(DAY_MS);
        let cutoff_ms = self.now_ms().saturating_sub(retention);
        let (jobs, removed, unreadable) =
            self.token_jobs(p.as_ref(), store.token_files(provider)?)?;
        // The budget starts here: discovery and the wait for the lock are not budgeted (spec
        // §5.2).
        let deadline = budget.and_then(|b| Instant::now().checked_add(b));
        let total = IndexProgress {
            files_done: 0,
            files_total: jobs.len() as u64,
            bytes_done: 0,
            bytes_total: jobs.iter().map(|j| j.bytes).sum(),
        };
        progress(&total);
        let work = Work {
            p: p.as_ref(),
            provider,
            jobs: &jobs,
            next: AtomicUsize::new(0),
            limits,
            deadline,
            cancel,
            halt: AtomicBool::new(false),
            cutoff_ms,
            peak: AtomicUsize::new(0),
        };
        let workers = thread::available_parallelism()
            .map_or(1, NonZeroUsize::get)
            .min(MAX_WORKERS)
            .min(jobs.len());
        let mut writer = Writer {
            engine: self,
            store: &store,
            provider,
            epoch,
            batch: Batch {
                removed,
                ..Batch::default()
            },
            tally: Tally {
                unreadable,
                ..Tally::default()
            },
            total,
        };
        thread::scope(|s| {
            let (tx, rx) = mpsc::sync_channel(workers.max(1));
            for _ in 0..workers {
                let (tx, work) = (tx.clone(), &work);
                s.spawn(move || work.run(&tx));
            }
            drop(tx);
            // Returning drops `rx`, so a worker blocked on a full channel stops too.
            let written = writer.run(rx, progress);
            if written.is_err() {
                work.halt.store(true, Ordering::Relaxed);
            }
            written
        })?;
        let tally = writer.tally;
        let mut complete = tally.files_done == jobs.len() as u64;
        let mut pruned = 0;
        if complete {
            // Step 5. The prune stops with the budget or the cancel token, as reading does, and
            // a prune left unfinished leaves the refresh incomplete, so the next call goes on.
            // The store asks `stop` only once a chunk has deleted rows, so every call deletes
            // some or completes (spec §5.2 Budget).
            let holder = uuid::Uuid::now_v7().to_string();
            let stop = || past(deadline) || cancel.requested().is_some();
            let outcome =
                store.prune_tokens(provider, epoch, cutoff_ms, &holder, self.now_ms(), &stop)?;
            complete = outcome.complete;
            pruned = outcome.deleted;
        }
        if complete {
            // The store keeps `responses` and the pass's skipped lines itself, each batch counting
            // them.
            let files = store.token_files(provider)?.len() as u64;
            store.finish_token_refresh(provider, epoch, files, tally.unreadable, self.now_ms())?;
        }
        // A cancel is reported even when there was nothing left for it to stop; what completed
        // stays recorded.
        if let Some(signal) = cancel.requested() {
            return Err(EngineError::Interrupted(signal));
        }
        let report = IndexReport {
            complete,
            files_read: tally.files_read,
            responses_added: tally.added,
            responses_updated: tally.updated,
            skipped_lines: tally.skipped,
            evidence_changed,
            took_ms: u64::try_from(self.now_ms().saturating_sub(started_ms)).unwrap_or(0),
        };
        let provider_id = self.registered_id(provider);
        if level(&report, pruned) == Level::INFO {
            tracing::info!(
                provider = provider_id,
                complete,
                files = report.files_read,
                added = report.responses_added,
                updated = report.responses_updated,
                pruned,
                skipped = report.skipped_lines,
                took_ms = report.took_ms,
                "refreshed the token index"
            );
        } else {
            tracing::debug!(
                provider = provider_id,
                complete,
                files = report.files_read,
                added = report.responses_added,
                updated = report.responses_updated,
                pruned,
                skipped = report.skipped_lines,
                took_ms = report.took_ms,
                "refreshed the token index"
            );
        }
        Ok((
            RefreshOutcome::Done(report),
            work.peak.load(Ordering::Relaxed),
        ))
    }

    /// Step 2 (spec §5.2), not budgeted: every transcript file, `stat`ed once. Returns the
    /// files to read, by path; the stored rows of files no longer found, whose usage stays;
    /// and how many could not be `stat`ed.
    fn token_jobs(
        &self,
        p: &dyn Provider,
        stored: Vec<TokenFileRow>,
    ) -> Result<(Vec<Job>, Vec<String>, u64), EngineError> {
        let mut stored: HashMap<String, TokenFileRow> =
            stored.into_iter().map(|r| (r.path.clone(), r)).collect();
        let (mut jobs, mut found, mut unreadable) = (Vec::new(), HashSet::new(), 0);
        for path in p.transcript_files(&self.env)? {
            let key = path.to_string_lossy().into_owned();
            if !found.insert(key.clone()) {
                continue;
            }
            let row = stored.remove(&key);
            match fs::metadata(&path) {
                Ok(meta) if !meta.is_file() || unchanged(&meta, row.as_ref()) => {}
                Ok(meta) => {
                    let resume = row
                        .as_ref()
                        .filter(|r| {
                            (r.dev, r.ino) == (meta.dev(), meta.ino()) && meta.len() >= r.offset
                        })
                        .map_or(0, |r| r.offset);
                    jobs.push(Job {
                        bytes: meta.len() - resume,
                        path,
                        stored: row,
                    });
                }
                // Gone since the listing: the next refresh drops its row.
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    unreadable += 1;
                    tracing::warn!(kind = ?e.kind(), "a transcript could not be read; the next refresh tries it again");
                    tracing::debug!(path = %path.display(), error = %e, "the transcript could not be read");
                }
            }
        }
        jobs.sort_by(|a, b| a.path.cmp(&b.path));
        Ok((jobs, stored.into_keys().collect(), unreadable))
    }
}

/// INFO when the pass changed something the person could see in a report (`pruned` is the rows
/// its prune deleted, which `IndexReport` does not carry); DEBUG for a pass that found nothing.
fn level(report: &IndexReport, pruned: u64) -> Level {
    if !report.complete
        || report.files_read > 0
        || report.responses_added > 0
        || report.responses_updated > 0
        || report.evidence_changed > 0
        || pruned > 0
    {
        Level::INFO
    } else {
        Level::DEBUG
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tagteam_fake::{FAKE_AGENT, FakeAgent, FakeUsage, append_transcript, usage_line_json};
    use tagteam_provider::{
        Env, FakeClock, FakeKeychain, FakeProcessProbe, NoHttp, RunShell, ScriptedSpawner,
    };

    use super::*;
    use crate::engine::EngineConfig;
    use crate::event::EventSource;
    use crate::oracle::NoOracle;
    use crate::registry::ProviderRegistry;
    use crate::settings::Settings;
    use crate::vault::{KeychainVault, Vault};

    const NOW: i64 = 1_790_000_000_000;

    fn engine(env: &Env) -> Engine {
        Engine::new(EngineConfig {
            env: env.clone(),
            registry: ProviderRegistry::new().with(Arc::new(FakeAgent::new())),
            vault: Vault::new(Box::new(KeychainVault::new(Arc::new(FakeKeychain::new())))),
            oracle: Arc::new(NoOracle),
            clock: Arc::new(FakeClock::new(NOW)),
            http: Arc::new(NoHttp),
            default_provider: ProviderId::new(FAKE_AGENT),
            settings: Settings::default(),
            process: Arc::new(FakeProcessProbe::new()),
            spawner: Arc::new(ScriptedSpawner::new()),
            run_shell: RunShell::Outside,
            source: EventSource::Cli,
        })
    }

    fn usage(id: &str) -> String {
        let u = FakeUsage {
            message_id: id.into(),
            request_id: format!("r-{id}"),
            ..FakeUsage::default()
        };
        format!("{}\n", usage_line_json(&u))
    }

    fn other(pad: &str) -> String {
        format!("{}\n", serde_json::json!({"t": "other", "pad": pad}))
    }

    #[test]
    fn a_pass_that_changed_nothing_is_debug_and_anything_else_is_info() {
        let quiet = IndexReport {
            complete: true,
            ..IndexReport::default()
        };
        assert_eq!(level(&quiet, 0), Level::DEBUG);
        assert_eq!(
            level(
                &IndexReport {
                    skipped_lines: 3,
                    took_ms: 40,
                    ..quiet
                },
                0
            ),
            Level::DEBUG
        );
        for changed in [
            IndexReport {
                complete: false,
                ..quiet
            },
            IndexReport {
                files_read: 1,
                ..quiet
            },
            IndexReport {
                responses_added: 1,
                ..quiet
            },
            IndexReport {
                responses_updated: 1,
                ..quiet
            },
            IndexReport {
                evidence_changed: 1,
                ..quiet
            },
        ] {
            assert_eq!(level(&changed, 0), Level::INFO, "{changed:?}");
        }
        assert_eq!(level(&quiet, 2), Level::INFO, "a prune that deleted rows");
    }

    #[test]
    fn memory_and_budget_hold_on_a_file_of_many_chunks() {
        // Review Focus 1: chunks of 64 bytes and a cap of 1 KiB stand in for 1 MiB and 64 MiB.
        let limits = Limits {
            chunk: 64,
            max_line: 1_024,
        };
        let p = ProviderId::new(FAKE_AGENT);

        // Memory: every worker's reader holds at most one chunk and one line up to the cap, and
        // a line over the cap is skipped, counted, and never held.
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let mut text = String::new();
        for i in 0..20 {
            text.push_str(&usage(&format!("m{i}")));
            text.push_str(&other(&"p".repeat(300)));
        }
        text.push_str(&other(&"x".repeat(5_000)));
        text.push_str(&usage("last"));
        append_transcript(&env, "big.jsonl", &text);
        let (outcome, peak) = engine(&env)
            .refresh_with(
                &p,
                Wait::Wait,
                None,
                &Cancel::new(),
                &mut |_: &IndexProgress| {},
                limits,
            )
            .unwrap();
        let RefreshOutcome::Done(report) = outcome else {
            panic!("nothing else refreshes");
        };
        assert!(report.complete);
        assert_eq!((report.responses_added, report.skipped_lines), (21, 1));
        assert!(
            peak > 0 && peak <= limits.chunk + limits.max_line,
            "a reader held {peak} bytes"
        );

        // Budget: past the deadline each call reads one more line, assembled across chunks,
        // never none and never the rest of the file, and the file is never skipped part-way.
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let lines: Vec<String> = (0..5).map(|i| usage(&format!("m{i}"))).collect();
        let path = append_transcript(&env, "slow.jsonl", &lines.concat());
        let engine = engine(&env);
        let store = engine.store().unwrap();
        let mut end = 0;
        for line in &lines {
            let (outcome, _) = engine
                .refresh_with(
                    &p,
                    Wait::Wait,
                    Some(Duration::ZERO),
                    &Cancel::new(),
                    &mut |_: &IndexProgress| {},
                    limits,
                )
                .unwrap();
            assert!(matches!(outcome, RefreshOutcome::Done(r) if !r.complete));
            end += line.len() as u64;
            let f = &store.token_files(&p).unwrap()[0];
            assert_eq!((f.offset, f.size), (end, -1));
            assert_eq!(store.token_state(&p).unwrap().unwrap().refreshed_at, None);
        }
        let (outcome, _) = engine
            .refresh_with(
                &p,
                Wait::Wait,
                Some(Duration::ZERO),
                &Cancel::new(),
                &mut |_: &IndexProgress| {},
                limits,
            )
            .unwrap();
        assert!(matches!(outcome, RefreshOutcome::Done(r) if r.complete));
        assert_eq!(store.token_usage(&p, None, None, None).unwrap().len(), 5);
        let f = &store.token_files(&p).unwrap()[0];
        let len = std::fs::metadata(&path).unwrap().len();
        assert_eq!((f.offset, f.size), (len, len as i64));
    }
}
