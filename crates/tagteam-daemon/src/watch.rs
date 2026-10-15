//! D4.4 and D4.5: the jobs that watch the store and the program file. The store watch tells
//! subscribers that another process changed the store, and wakes every provider job to
//! re-evaluate its mode; the upgrade watch stops the daemon to be restarted on a new binary,
//! or past a newer tagteam's store migration (exit 75).

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tagteam_engine::store::{SCHEMA_VERSION, StoreError};
use tagteam_engine::{Engine, EngineError};

use crate::events::{DaemonEvent, StopReason};
use crate::provider_job::JOB_PREFIX;
use crate::scheduler::{Job, JobCx, Ran, Wake};
use crate::status::{JobStatus, StatusBoard};

/// D4.4: the store watch's period.
pub const STORE_WATCH_MS: i64 = 2_000;
/// D4.5: the upgrade watch's period.
pub const UPGRADE_WATCH_MS: i64 = 60_000;

/// A file's device and inode, following links; `None` when nothing is there.
fn file_identity(path: &Path) -> io::Result<Option<(u64, u64)>> {
    match fs::metadata(path) {
        Ok(m) => Ok(Some((m.dev(), m.ino()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// D4.1 step 4 and D4.5: whether the store's schema is newer than this build's (§6.1), read
/// through the engine's connection, or from an open that refuses it. No store is not newer,
/// and neither is a store that cannot be read for another reason.
pub fn schema_newer(engine: &Engine) -> bool {
    match engine.existing_store() {
        Ok(Some(store)) => store.schema_version().is_ok_and(|v| v > SCHEMA_VERSION),
        Ok(None) => false,
        Err(EngineError::Store(StoreError::UnsupportedSchema(_))) => true,
        Err(_) => false,
    }
}

/// D4.5 and D9.1: the program path the upgrade watch watches. `argv0` when it is absolute (a
/// service unit passes it so). Otherwise the first `tagteam` in an absolute `PATH` entry that
/// is the running binary (`current_exe`, by device and inode), so a package manager's stable
/// link survives an upgrade. Otherwise `current_exe` itself.
pub fn program_path(argv0: &OsStr, path_var: Option<&OsStr>, current_exe: &Path) -> PathBuf {
    let argv0 = Path::new(argv0);
    if argv0.is_absolute() {
        return argv0.to_path_buf();
    }
    let running = file_identity(current_exe).ok().flatten();
    if let (Some(running), Some(path_var)) = (running, path_var) {
        for dir in std::env::split_paths(path_var).filter(|d| d.is_absolute()) {
            let candidate = dir.join("tagteam");
            if file_identity(&candidate).ok().flatten() == Some(running) {
                return candidate;
            }
        }
    }
    current_exe.to_path_buf()
}

/// How many times `StoreBaseline::take` opens the store while another process keeps
/// replacing it.
const BASELINE_TRIES: u32 = 3;

/// Decision 11: what the store watch's first run compares with: the store file's device and
/// inode, and `data_version` on the engine's connection to that file. `Daemon::start` takes it
/// once `daemon.lock` is held and before any provider job uses the store, so a store replaced
/// or committed to before the watch first runs is a change at that run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreBaseline {
    /// The store file's device and inode; `None` when there is no store.
    file: Option<(u64, u64)>,
    /// `data_version` on the engine's connection to that file; `None` without a connection.
    version: Option<i64>,
}

impl StoreBaseline {
    /// Forgets the engine's connection and opens the store again, between two looks at the
    /// store file's device and inode, so the identity recorded is the file the new connection
    /// has open. When the looks differ, another process replaced the store meanwhile, and it
    /// tries again, up to three times. A third try whose looks still differ keeps the look
    /// taken before its open: a file other than that one at the watch's first run is a move,
    /// so a race costs at most one `changed` too many, never a connection left on an unlinked
    /// file.
    ///
    /// A store this build refuses (`UnsupportedSchema`, D4.1 step 4) has no connection, so no
    /// version; the watch reports the refusal on its status at each run. Any other failure to
    /// open the store, or to read its metadata, is the error.
    pub fn take(engine: &Engine) -> Result<StoreBaseline, EngineError> {
        let path = engine.store_file();
        let mut tries = 0;
        loop {
            tries += 1;
            let before = file_identity(&path)?;
            engine.forget_store();
            let version = match engine.existing_store() {
                Ok(Some(store)) => Some(store.data_version()?),
                Ok(None) | Err(EngineError::Store(StoreError::UnsupportedSchema(_))) => None,
                Err(e) => return Err(e),
            };
            if file_identity(&path)? == before || tries == BASELINE_TRIES {
                return Ok(StoreBaseline {
                    file: before,
                    version,
                });
            }
        }
    }
}

/// D4.4 and Decision 11: every 2 s, whether another process committed to the store (the
/// engine connection's `data_version`), and whether the store file is still the one the
/// engine opened (its device and inode). Either is one `changed` event, after which every
/// provider job is woken to re-evaluate its mode (D4.3). A store replaced or removed is
/// forgotten, so the engine's next use opens the file the path names now.
pub struct StoreWatch {
    board: Arc<StatusBoard>,
    period_ms: i64,
    wake: Option<Wake>,
    /// The store file's device and inode at the last run, or at the baseline before the
    /// first; `None` when there was none.
    file: Option<(u64, u64)>,
    /// The engine connection's `data_version` at the last run, or at the baseline.
    version: Option<i64>,
    /// The last run's failure, so it is logged once until it changes.
    failing: Option<String>,
}

impl StoreWatch {
    /// A watch whose first run compares with `baseline` (`StoreBaseline::take`), as every
    /// later run compares with the run before.
    pub fn new(board: Arc<StatusBoard>, baseline: StoreBaseline) -> Self {
        StoreWatch {
            board,
            period_ms: STORE_WATCH_MS,
            wake: None,
            file: baseline.file,
            version: baseline.version,
            failing: None,
        }
    }

    pub fn with_period(mut self, ms: i64) -> Self {
        self.period_ms = ms;
        self
    }

    /// After each `changed`, the provider jobs are woken through `wake` (D4.3).
    pub fn waking(mut self, wake: Wake) -> Self {
        self.wake = Some(wake);
        self
    }

    /// One look: whether the store changed since the last look, or since the baseline at the
    /// first, and what could not be read.
    fn check(&mut self, engine: &Engine) -> (bool, Option<EngineError>) {
        let file = match file_identity(&engine.store_file()) {
            Ok(file) => file,
            Err(e) => return (false, Some(e.into())),
        };
        let moved = self.file != file;
        self.file = file;
        if moved {
            // Decision 11: the cached connection may be to an unlinked file. A new connection's
            // `data_version` compares with nothing before it.
            engine.forget_store();
            self.version = None;
        }
        let version = match file {
            Some(_) => match engine.data_version() {
                Ok(v) => v,
                Err(e) => return (moved, Some(e)),
            },
            None => None,
        };
        let before = std::mem::replace(&mut self.version, version);
        let committed = matches!((before, version), (Some(b), Some(v)) if b != v);
        (moved || committed, None)
    }
}

impl Job for StoreWatch {
    fn name(&self) -> String {
        "store-watch".to_owned()
    }

    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
        let now_ms = cx.engine.now_ms();
        let (changed, failure) = self.check(cx.engine);
        if changed {
            tracing::debug!("another process changed the store");
            cx.sink.emit(&DaemonEvent::Changed);
            if let Some(wake) = &self.wake {
                wake.wake(JOB_PREFIX);
            }
        }
        let last_error = failure.map(|e| {
            let message = e.to_string();
            if self.failing.as_ref() != Some(&message) {
                // §14.2: the error's kind, never its text, which may quote a stored value.
                tracing::warn!(kind = e.kind(), "the store watch could not read the store");
            }
            message
        });
        self.failing = last_error.clone();
        self.board.set_job(JobStatus {
            name: self.name(),
            last_run_at: Some(now_ms.div_euclid(1000)),
            next_run_at: Some((now_ms + self.period_ms).div_euclid(1000)),
            last_error,
        });
        Ran::After(self.period_ms)
    }
}

/// The file a program path resolves to: its device, inode, size and mtime (D4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
}

/// The file `path` resolves to, following links; `None` when nothing is there.
fn file_stamp(path: &Path) -> io::Result<Option<FileStamp>> {
    match fs::metadata(path) {
        Ok(m) => Ok(Some(FileStamp {
            dev: m.dev(),
            ino: m.ino(),
            size: m.size(),
            mtime: m.mtime(),
            mtime_nsec: m.mtime_nsec(),
        })),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// D4.5 and Decision 17: a program path, and the file it resolved to when the stamp was taken.
/// `tagteam daemon` takes it first thing, before anything that can wait (an unlock offer, the
/// `daemon.lock` wait), and the upgrade watch compares every run with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramStamp {
    path: PathBuf,
    /// The file `path` resolved to; `None` when it resolved to nothing, or its metadata could
    /// not be read.
    start: Option<FileStamp>,
}

impl ProgramStamp {
    /// Records the file `path` resolves to now, following links.
    pub fn take(path: PathBuf) -> Self {
        let start = file_stamp(&path).ok().flatten();
        ProgramStamp { path, start }
    }

    /// The path the watch follows.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// D4.5: every 60 s, whether the program file was replaced, or a newer tagteam migrated the
/// store past this build's schema while the daemon ran. Either stops the scheduler, so the
/// daemon exits 75 and a service restarts it on the new binary (D9).
pub struct UpgradeWatch {
    /// The program path, and the file it resolved to at the stamp (Decision 17).
    program: ProgramStamp,
    /// D4.1 step 4: a store already newer at start-up waits for the binary instead.
    schema_newer_at_start: bool,
    board: Arc<StatusBoard>,
    period_ms: i64,
}

impl UpgradeWatch {
    /// A watch that compares every run, its first included, with `program`'s stamp. It never
    /// stats the file itself here: a binary replaced between the stamp and now is an upgrade.
    pub fn new(
        program: ProgramStamp,
        schema_newer_at_start: bool,
        board: Arc<StatusBoard>,
    ) -> Self {
        UpgradeWatch {
            program,
            schema_newer_at_start,
            board,
            period_ms: UPGRADE_WATCH_MS,
        }
    }

    pub fn with_period(mut self, ms: i64) -> Self {
        self.period_ms = ms;
        self
    }

    /// Why the daemon must restart, if it must. A program file whose metadata cannot be read
    /// is the error, and no reason to restart.
    fn check(&self, engine: &Engine) -> io::Result<Option<StopReason>> {
        if file_stamp(&self.program.path)? != self.program.start {
            return Ok(Some(StopReason::Upgrade));
        }
        if !self.schema_newer_at_start && schema_newer(engine) {
            return Ok(Some(StopReason::Schema));
        }
        Ok(None)
    }
}

impl Job for UpgradeWatch {
    fn name(&self) -> String {
        "upgrade-watch".to_owned()
    }

    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
        let now_ms = cx.engine.now_ms();
        let (stop, last_error) = match self.check(cx.engine) {
            Ok(stop) => (stop, None),
            Err(e) => {
                tracing::warn!(kind = %e.kind(), "the upgrade watch could not read the program file");
                (None, Some(format!("{}: {e}", self.program.path.display())))
            }
        };
        self.board.set_job(JobStatus {
            name: self.name(),
            last_run_at: Some(now_ms.div_euclid(1000)),
            next_run_at: stop
                .is_none()
                .then(|| (now_ms + self.period_ms).div_euclid(1000)),
            last_error,
        });
        match stop {
            Some(reason) => {
                tracing::info!(
                    reason = reason.as_str(),
                    "tagteam was upgraded, so the daemon stops to be restarted"
                );
                Ran::Stop(reason)
            }
            None => Ran::After(self.period_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::{Duration, SystemTime};

    use tagteam_provider::{Clock, FakeClock};

    use super::*;
    use crate::events::Mode;
    use crate::provider_job::ProviderJob;
    use crate::scheduler::Scheduler;
    use crate::testutil::{Emitted, Fx, Slices, T0, board, cc, job_ctx, two};

    /// How many `changed` events `events` holds.
    fn changed(events: &Emitted<'_>) -> usize {
        events
            .events()
            .iter()
            .filter(|(_, e)| *e == DaemonEvent::Changed)
            .count()
    }

    /// The store watch's baseline as `Daemon::start` takes it (Task 12).
    fn baseline(fx: &Fx) -> StoreBaseline {
        StoreBaseline::take(&fx.engine).unwrap()
    }

    /// `PRAGMA user_version` past this build's, as a newer tagteam's migration leaves it.
    fn migrate_past(fx: &Fx) {
        let conn = rusqlite::Connection::open(fx.engine.store_file()).unwrap();
        conn.execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION + 1))
            .unwrap();
    }

    fn set_mtime(p: &Path, secs: u64) {
        fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    /// A program file holding `bytes`, with an mtime `secs` after the epoch.
    fn program(dir: &Path, name: &str, bytes: &[u8], secs: u64) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, bytes).unwrap();
        set_mtime(&p, secs);
        p
    }

    /// An executable file at `rel` under `root`, its directories created.
    fn executable(root: &Path, rel: &str) -> PathBuf {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// The upgrade watch's first run after `change` touched its program file, which starts as
    /// nine bytes with an mtime of 1000 s.
    fn after(fx: &Fx, change: impl FnOnce(&Path)) -> Ran {
        let dir = tempfile::tempdir().unwrap();
        let exe = program(dir.path(), "tagteam", b"binary v1", 1_000);
        let mut watch = UpgradeWatch::new(ProgramStamp::take(exe.clone()), false, board());
        change(&exe);
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        watch.run(&mut cx)
    }

    /// A job that records, by name, when each of its runs began, in seconds after `T0`.
    struct Counted<'a, J> {
        job: J,
        clock: &'a FakeClock,
        runs: &'a RefCell<Vec<(String, i64)>>,
    }

    impl<J: Job> Job for Counted<'_, J> {
        fn name(&self) -> String {
            self.job.name()
        }

        fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
            let at = self.clock.now_ms().div_euclid(1000) - T0;
            self.runs.borrow_mut().push((self.job.name(), at));
            self.job.run(cx)
        }

        fn settings_changed(&mut self, cx: &mut JobCx<'_>) {
            self.job.settings_changed(cx);
        }
    }

    #[test]
    fn another_process_s_commits_are_one_changed_and_the_engine_s_own_writes_none() {
        // D4.4: two commits by another connection within one period give one `changed`;
        // this daemon's own writes give none (they come as their own events).
        let fx = Fx::new();
        fx.add("a@x.co");
        let other = fx.other();
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut watch = StoreWatch::new(board.clone(), baseline(&fx));
        assert_eq!(watch.run(&mut cx), Ran::After(STORE_WATCH_MS));
        assert_eq!(changed(&events), 0, "nothing changed since the baseline");
        let theirs = other.store().unwrap();
        theirs.set_unhealthy_ticks(&cc(), 1).unwrap();
        theirs.set_unhealthy_ticks(&cc(), 2).unwrap();
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1);
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1, "nothing since");
        fx.engine
            .store()
            .unwrap()
            .set_unhealthy_ticks(&cc(), 3)
            .unwrap();
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1, "the engine's own write");
        assert_eq!(
            board.snapshot().jobs,
            [JobStatus {
                name: "store-watch".into(),
                last_run_at: Some(T0),
                next_run_at: Some(T0 + 2),
                last_error: None,
            }]
        );
    }

    #[test]
    fn a_replaced_store_file_is_reopened_and_reported_as_changed() {
        // Review Focus 2, Decision 11. The store this daemon has open is removed, and another
        // process creates a new one at the same path (the store deleted by hand, then
        // `tagteam add`). The watch reports `changed` once, and the engine reopens the path:
        // it reads the new store, its writes land there, and the watch follows that file's
        // commits. Nothing is written into the unlinked one.
        let fx = Fx::new();
        two(&fx);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut watch = StoreWatch::new(board.clone(), baseline(&fx));
        watch.run(&mut cx);
        fx.remove_store();
        let other = fx.other();
        fx.add_on(&other, "c@x.co");
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1);
        // Decision 11: the new connection's first `data_version` was a baseline. Compared with
        // the unlinked file's, it would read as a commit here.
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1, "nothing committed since the reopen");
        let emails: Vec<Option<String>> = fx
            .engine
            .existing_store()
            .unwrap()
            .unwrap()
            .accounts(&cc())
            .unwrap()
            .into_iter()
            .map(|r| r.email)
            .collect();
        assert_eq!(emails, [Some("c@x.co".to_owned())]);
        other
            .store()
            .unwrap()
            .set_unhealthy_ticks(&cc(), 1)
            .unwrap();
        watch.run(&mut cx);
        assert_eq!(
            changed(&events),
            2,
            "another process's commit to the new file"
        );
        fx.engine
            .store()
            .unwrap()
            .set_unhealthy_ticks(&cc(), 2)
            .unwrap();
        watch.run(&mut cx);
        assert_eq!(changed(&events), 2, "the engine's own write");
        let state = other.store().unwrap().autoswitch_state(&cc()).unwrap();
        assert_eq!(
            state.unhealthy_ticks, 2,
            "the engine wrote into the file the path names"
        );
    }

    #[test]
    fn a_store_that_vanished_is_forgotten_and_reported_once() {
        let fx = Fx::new();
        fx.add("a@x.co");
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut watch = StoreWatch::new(board.clone(), baseline(&fx));
        watch.run(&mut cx);
        fx.remove_store();
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1);
        assert!(
            fx.engine.existing_store().unwrap().is_none(),
            "the unlinked store is never used again"
        );
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1);
    }

    #[test]
    fn the_first_run_compares_with_the_baseline() {
        // Decision 11: `Daemon::start` takes the baseline, and serving begins later (plan B
        // binds its socket in between). With nothing changed since, the first run reports
        // nothing. Another process's commit since is reported at the first run, which a first
        // run that only recorded would miss.
        let fx = Fx::new();
        fx.add("a@x.co");
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut quiet = StoreWatch::new(board(), baseline(&fx));
        quiet.run(&mut cx);
        assert_eq!(changed(&events), 0, "nothing changed since the baseline");
        let taken = baseline(&fx);
        fx.other()
            .store()
            .unwrap()
            .set_unhealthy_ticks(&cc(), 1)
            .unwrap();
        let mut watch = StoreWatch::new(board(), taken);
        watch.run(&mut cx);
        assert_eq!(
            changed(&events),
            1,
            "a commit between the baseline and the first run"
        );
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1, "nothing since");
    }

    #[test]
    fn a_store_replaced_after_the_baseline_is_reopened_at_the_first_run() {
        // Decision 11: the baseline names the file the engine's connection has open. The store
        // is deleted, and another process creates a new one, after the baseline and before the
        // watch first runs (between `Daemon::start` and serving). That first run sees another
        // file: it forgets the engine's connection and reports `changed`, and the engine reads
        // the new store from then on. A first run that only recorded would take the new file
        // for the baseline and leave the engine on the unlinked one.
        let fx = Fx::new();
        two(&fx);
        let taken = baseline(&fx);
        fx.remove_store();
        let other = fx.other();
        fx.add_on(&other, "c@x.co");
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut watch = StoreWatch::new(board(), taken);
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1);
        let emails: Vec<Option<String>> = fx
            .engine
            .existing_store()
            .unwrap()
            .unwrap()
            .accounts(&cc())
            .unwrap()
            .into_iter()
            .map(|r| r.email)
            .collect();
        assert_eq!(
            emails,
            [Some("c@x.co".to_owned())],
            "the engine forgot its connection to the unlinked store"
        );
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1, "nothing committed since the reopen");
    }

    #[test]
    fn a_store_that_appears_later_moves_the_provider_out_of_idle() {
        // Review Focus 3: a fresh machine. The daemon starts before any store exists, so the
        // provider is `idle`. A first `tagteam add` from another process creates the store:
        // the watch's next run reports `changed`, and the provider job, woken by it, moves to
        // `collect` before the scheduler sleeps again.
        let fx = Fx::new();
        assert!(!fx.engine.store_file().exists());
        let other = fx.other();
        let board = board();
        let cancel = fx.engine.cancel().clone();
        let slices = Slices::new(&fx.clock, |n| match n {
            2 => {
                fx.add_on(&other, "a@x.co");
            }
            _ if n >= 4 => cancel.request(libc::SIGTERM),
            _ => {}
        });
        let mut scheduler = Scheduler::new();
        let wake = scheduler.wake();
        let now = fx.engine.now_ms();
        scheduler.add(
            Box::new(ProviderJob::new(&fx.engine, cc(), job_ctx(&board))),
            now,
        );
        let store = StoreWatch::new(board.clone(), baseline(&fx)).waking(wake);
        scheduler.add(Box::new(store), now);
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        assert_eq!(scheduler.run(&mut cx, &slices), StopReason::Signal);
        assert_eq!(
            events.modes(&cc()),
            [(0, Mode::Idle, None, None), (4, Mode::Collect, None, None)]
        );
        let order: Vec<(i64, &str)> = events
            .events()
            .into_iter()
            .filter_map(|(at, e)| match e {
                DaemonEvent::Changed => Some((at, "changed")),
                DaemonEvent::Mode {
                    mode: Mode::Collect,
                    ..
                } => Some((at, "collect")),
                _ => None,
            })
            .collect();
        assert_eq!(order, [(4, "changed"), (4, "collect")]);
    }

    #[test]
    fn a_suspend_runs_each_overdue_job_once_on_waking() {
        // Review Focus 4. The machine sleeps for an hour during the third slice. Every
        // deadline is wall clock, so on waking each job that fell due runs once, at once:
        // never once per period it slept through. The store watch's 2 s cadence resumes from
        // there.
        let fx = Fx::new();
        let dir = tempfile::tempdir().unwrap();
        let exe = program(dir.path(), "tagteam", b"binary v1", 1_000);
        let board = board();
        let runs = RefCell::new(Vec::new());
        let (clock, cancel) = (fx.clock.clone(), fx.engine.cancel().clone());
        let slices = Slices::new(&fx.clock, move |n| match n {
            2 => clock.advance_ms(3_600_000),
            _ if n >= 5 => cancel.request(libc::SIGTERM),
            _ => {}
        });
        let mut scheduler = Scheduler::new();
        let now = fx.engine.now_ms();
        let provider = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        scheduler.add(
            Box::new(Counted {
                job: provider,
                clock: &fx.clock,
                runs: &runs,
            }),
            now,
        );
        scheduler.add(
            Box::new(Counted {
                job: StoreWatch::new(board.clone(), baseline(&fx)),
                clock: &fx.clock,
                runs: &runs,
            }),
            now,
        );
        scheduler.add(
            Box::new(Counted {
                job: UpgradeWatch::new(ProgramStamp::take(exe), false, board.clone()),
                clock: &fx.clock,
                runs: &runs,
            }),
            now,
        );
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        assert_eq!(scheduler.run(&mut cx, &slices), StopReason::Signal);
        let of = |name: &str| -> Vec<i64> {
            runs.borrow()
                .iter()
                .filter(|(n, _)| n == name)
                .map(|(_, at)| *at)
                .collect()
        };
        assert_eq!(of("provider:claude-code"), [0, 3_603]);
        assert_eq!(of("store-watch"), [0, 2, 3_603, 3_605]);
        assert_eq!(of("upgrade-watch"), [0, 3_603]);
    }

    #[test]
    fn an_unchanged_program_file_keeps_the_daemon_running() {
        let fx = Fx::new();
        let dir = tempfile::tempdir().unwrap();
        let exe = program(dir.path(), "tagteam", b"binary v1", 1_000);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let stamp = ProgramStamp::take(exe.clone());
        assert_eq!(stamp.path(), exe.as_path());
        let mut watch = UpgradeWatch::new(stamp.clone(), false, board.clone());
        assert_eq!(watch.run(&mut cx), Ran::After(UPGRADE_WATCH_MS));
        assert_eq!(watch.run(&mut cx), Ran::After(UPGRADE_WATCH_MS));
        assert_eq!(
            board.snapshot().jobs,
            [JobStatus {
                name: "upgrade-watch".into(),
                last_run_at: Some(T0),
                next_run_at: Some(T0 + 60),
                last_error: None,
            }]
        );
        let mut fast = UpgradeWatch::new(stamp, false, board).with_period(500);
        assert_eq!(fast.run(&mut cx), Ran::After(500));
        assert!(events.events().is_empty());
    }

    #[test]
    fn the_watch_compares_with_the_stamp_it_is_given_never_with_the_file_when_built() {
        // D4.5, Decision 17: `tagteam daemon` stamps its program first thing, and builds the
        // watch only after the unlock offer and the `daemon.lock` wait (Task 12). A tagteam
        // installed in between is an upgrade at the watch's first run, never the stamp of a
        // daemon that still runs the old binary.
        let fx = Fx::new();
        let dir = tempfile::tempdir().unwrap();
        let exe = program(dir.path(), "tagteam", b"binary v1", 1_000);
        let stamp = ProgramStamp::take(exe.clone());
        let next = exe.with_file_name("tagteam.new");
        fs::write(&next, b"binary v2").unwrap();
        set_mtime(&next, 1_000);
        fs::rename(&next, &exe).unwrap();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut watch = UpgradeWatch::new(stamp, false, board());
        assert_eq!(watch.run(&mut cx), Ran::Stop(StopReason::Upgrade));
    }

    #[test]
    fn another_mtime_size_or_file_or_none_at_the_path_is_an_upgrade() {
        // D4.5: the file the program path resolves to, by device, inode, size and mtime.
        let fx = Fx::new();
        let upgrade = Ran::Stop(StopReason::Upgrade);
        assert_eq!(after(&fx, |_| {}), Ran::After(UPGRADE_WATCH_MS));
        assert_eq!(
            after(&fx, |p| set_mtime(p, 2_000)),
            upgrade,
            "another mtime"
        );
        let longer = |p: &Path| {
            fs::write(p, b"binary v1.1").unwrap();
            set_mtime(p, 1_000);
        };
        assert_eq!(after(&fx, longer), upgrade, "another size, the same mtime");
        let renamed = |p: &Path| {
            let next = p.with_file_name("tagteam.new");
            fs::write(&next, b"binary v2").unwrap();
            set_mtime(&next, 1_000);
            fs::rename(&next, p).unwrap();
        };
        assert_eq!(
            after(&fx, renamed),
            upgrade,
            "another file of the same size and mtime"
        );
        let removed = |p: &Path| fs::remove_file(p).unwrap();
        assert_eq!(
            after(&fx, removed),
            upgrade,
            "nothing: tagteam was uninstalled"
        );
    }

    #[test]
    fn a_stable_link_moved_to_a_new_binary_is_an_upgrade() {
        // D-R4: Homebrew's `bin/tagteam` links into a versioned directory. The watch follows
        // the link, so moving it to the new version is an upgrade while the old binary stays.
        let fx = Fx::new();
        let dir = tempfile::tempdir().unwrap();
        let old = program(dir.path(), "tagteam-1", b"binary v1", 1_000);
        let new = program(dir.path(), "tagteam-2", b"binary v2", 1_000);
        let link = dir.path().join("tagteam");
        symlink(&old, &link).unwrap();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut watch = UpgradeWatch::new(ProgramStamp::take(link.clone()), false, board());
        assert_eq!(watch.run(&mut cx), Ran::After(UPGRADE_WATCH_MS));
        fs::remove_file(&link).unwrap();
        symlink(&new, &link).unwrap();
        assert_eq!(watch.run(&mut cx), Ran::Stop(StopReason::Upgrade));
        assert!(old.exists());
    }

    #[test]
    fn a_store_migrated_past_this_build_while_running_stops_the_daemon() {
        // D4.5: a newer tagteam migrated the store while the daemon ran. It shows through the
        // engine's open connection, and, once the engine reopens, as the open's refusal.
        let fx = Fx::new();
        fx.add("a@x.co");
        let dir = tempfile::tempdir().unwrap();
        let exe = program(dir.path(), "tagteam", b"binary v1", 1_000);
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let stamp = ProgramStamp::take(exe);
        let mut watch = UpgradeWatch::new(stamp.clone(), false, board());
        assert_eq!(watch.run(&mut cx), Ran::After(UPGRADE_WATCH_MS));
        migrate_past(&fx);
        assert_eq!(watch.run(&mut cx), Ran::Stop(StopReason::Schema));
        fx.engine.forget_store();
        let mut reopened = UpgradeWatch::new(stamp, false, board());
        assert_eq!(reopened.run(&mut cx), Ran::Stop(StopReason::Schema));
    }

    #[test]
    fn a_store_already_newer_at_start_waits_for_the_binary() {
        // D4.1 step 4: the daemon started on a store this build cannot open. A restart would
        // bring the same binary back to the same store, so only a new binary ends the wait.
        let fx = Fx::new();
        fx.add("a@x.co");
        migrate_past(&fx);
        fx.engine.forget_store();
        assert!(schema_newer(&fx.engine));
        let dir = tempfile::tempdir().unwrap();
        let exe = program(dir.path(), "tagteam", b"binary v1", 1_000);
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut watch = UpgradeWatch::new(ProgramStamp::take(exe.clone()), true, board());
        assert_eq!(watch.run(&mut cx), Ran::After(UPGRADE_WATCH_MS));
        set_mtime(&exe, 2_000);
        assert_eq!(watch.run(&mut cx), Ran::Stop(StopReason::Upgrade));
    }

    #[test]
    fn schema_newer_reads_the_store_s_version_or_its_refusal() {
        let fx = Fx::new();
        assert!(!schema_newer(&fx.engine), "no store");
        fx.add("a@x.co");
        assert!(!schema_newer(&fx.engine), "this build's schema");
        migrate_past(&fx);
        assert!(schema_newer(&fx.engine), "through the open connection");
        fx.engine.forget_store();
        assert!(schema_newer(&fx.engine), "an open that refuses it");
    }

    #[test]
    fn the_program_path_is_argv0_when_absolute_else_the_tagteam_on_path_that_runs() {
        // D4.5, D9.1. A service passes its absolute program path. From a shell argv[0] is a
        // bare name, so the path is the first `tagteam` on PATH that is the running binary (a
        // stable link survives upgrades), passing over one that only shadows it; otherwise
        // the running binary's own path.
        let dir = tempfile::tempdir().unwrap();
        let exe = executable(dir.path(), "Cellar/tagteam/1/bin/tagteam");
        executable(dir.path(), "shadow/tagteam");
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        symlink(&exe, bin.join("tagteam")).unwrap();
        let path = |dirs: &[PathBuf]| std::env::join_paths(dirs).unwrap();
        assert_eq!(
            program_path(OsStr::new("/opt/tagteam/bin/tagteam"), None, &exe),
            PathBuf::from("/opt/tagteam/bin/tagteam"),
            "absolute: as given, even when nothing is there"
        );
        let on_path = path(&[
            dir.path().join("missing"),
            dir.path().join("shadow"),
            bin.clone(),
        ]);
        assert_eq!(
            program_path(OsStr::new("tagteam"), Some(on_path.as_os_str()), &exe),
            bin.join("tagteam")
        );
        let shadowed = path(&[dir.path().join("shadow")]);
        assert_eq!(
            program_path(OsStr::new("tagteam"), Some(shadowed.as_os_str()), &exe),
            exe
        );
        assert_eq!(program_path(OsStr::new("./tagteam"), None, &exe), exe);
        assert_eq!(
            program_path(OsStr::new("tagteam"), Some(OsStr::new("bin")), &exe),
            exe,
            "a relative PATH entry names no stable path"
        );
    }
}
