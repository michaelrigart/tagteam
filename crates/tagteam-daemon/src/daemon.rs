//! D4.1 and D4.6: the daemon's start-up and its run. `Daemon::start` covers everything that
//! can refuse: the run shell, `daemon.lock`, and a store this build cannot open. `Started::serve`
//! runs the scheduler until a signal or a newer tagteam ends it, then lets everything go in
//! D4.6's order (Decision 13). Plan B binds the socket between the two.

use std::fs::OpenOptions;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use tagteam_core::ProviderId;
use tagteam_engine::daemon::{daemon_lock_path, read_daemon_record, write_daemon_record};
use tagteam_engine::settings::Settings;
use tagteam_engine::store::{SCHEMA_VERSION, StoreError};
use tagteam_engine::{Engine, EngineError};
use tagteam_provider::keychain::ReadCounts;
use tagteam_provider::{FlockGuard, Read, RunShell};

use crate::events::{DaemonEvent, DaemonSink, Fanout, StopReason};
use crate::notify::{Backend, NoticeQueue, Notifier, NotifySink};
use crate::provider_job::{ProviderJob, ProviderJobCtx};
use crate::scheduler::{Job, JobCx, Ran, Scheduler, Sleeper};
use crate::status::{DaemonInfo, NotifyInfo, StatusBoard};
use crate::watch::{ProgramStamp, StoreBaseline, StoreWatch, UpgradeWatch};

/// D4.1 step 2: how often a daemon that finds `daemon.lock` held tries it again.
const LOCK_RETRY: Duration = Duration::from_secs(5);
/// How often that wait looks at the cancel token (§14.1).
const WAIT_SLICE: Duration = Duration::from_millis(100);
/// How often the settings job republishes the board's notifications when no reload told it to
/// sooner.
const SETTINGS_JOB_MS: i64 = 3_600_000;

/// What the caller decides, from its command line and environment (D8.1).
pub struct DaemonOptions {
    /// `--provider`: only this provider's job (D8). Otherwise every registered provider.
    pub provider: Option<ProviderId>,
    /// This build's version, recorded in `daemon.lock` (D4.1) and shown by `status` (D5.4).
    pub version: String,
    /// The path the upgrade watch follows, and the file it resolved to when the caller stamped
    /// it, first thing, before anything that can wait (D4.5, Decision 17).
    pub program: ProgramStamp,
    /// The upgrade watch's period: 60 000 ms, and shorter only in plan B's tests.
    pub upgrade_watch_ms: i64,
    /// Decision 7: the read counts the keychain backoff compares (D4.1); empty on Linux.
    pub keychain_counts: Vec<Arc<ReadCounts>>,
    /// Where notices are shown; `None` when there is no backend (D6.2).
    pub backend: Option<Backend>,
}

/// How a run ended (D8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonExit {
    /// A signal stopped it: exit 0 (D4.6).
    Signal,
    /// A newer binary, or a store a newer tagteam migrated, must take over: exit 75 (D4.5).
    Restart,
}

/// Why the daemon could not start (D8.1's exit 1).
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error(transparent)]
    Engine(#[from] EngineError),
}

pub struct Daemon;

impl Daemon {
    /// D4.1 steps 1, 2 and 4:
    /// - read `config.toml`'s mtime, before the settings the daemon serves with (Decision 9)
    /// - refuse inside a run shell
    /// - read the engine's settings again, for the provider the command acts on (Decision 3)
    /// - hold `daemon.lock`, waiting while another daemon holds it, and say so once through
    ///   `notice`
    /// - record this daemon in the lock
    /// - take the store watch's baseline on a fresh connection, and probe the store
    ///   (Decision 11)
    /// - build the status board, and start the notifier when there is a backend
    ///
    /// Root is refused before any engine exists (the CLI's `run_command`). The Keychain check
    /// is the CLI's (Decision 14), and so is the program's stamp in `opts`, taken before
    /// anything that can wait (Decision 17).
    pub fn start<'e>(
        engine: &'e Engine,
        opts: DaemonOptions,
        notice: &mut dyn FnMut(&str),
    ) -> Result<Started<'e>, DaemonError> {
        Self::start_with(engine, opts, notice, LOCK_RETRY)
    }

    /// `start`, with the lock's retry period a parameter so the tests need not wait 5 s.
    fn start_with<'e>(
        engine: &'e Engine,
        opts: DaemonOptions,
        notice: &mut dyn FnMut(&str),
        retry: Duration,
    ) -> Result<Started<'e>, DaemonError> {
        // Before the `daemon.lock` wait and every settings read the daemon serves with, so a
        // write landing meanwhile is a change at the scheduler's first check (D4.2).
        let settings_mtime = Settings::mtime(engine.env());
        if matches!(engine.run_shell(), RunShell::Inside { .. }) {
            return Err(EngineError::InsideRunShell.into());
        }
        // Decision 3 and 9: the engine read its settings when the command built it, before the
        // mtime and before any unlock offer. Read them again, for the provider the command read
        // them for. The warnings are dropped: each provider job emits its own read's at its
        // first run (Decision 8), and that provider's job is among them.
        let settings_provider = opts
            .provider
            .clone()
            .unwrap_or_else(|| engine.default_provider().clone());
        let _warnings = engine.reload_settings(&settings_provider);
        let lock = DaemonLock(wait_for_lock(engine, notice, retry)?);
        write_daemon_record(&lock.0, &opts.version)?;
        // Decision 11: the store watch's baseline, on a fresh connection, before any provider
        // job is built. A failure to open the store, other than a schema this build refuses, is
        // a start failure (D8.1).
        let store = StoreBaseline::take(engine)?;
        let schema_newer = match engine.existing_store() {
            Ok(_) => false,
            Err(EngineError::Store(StoreError::UnsupportedSchema(found))) => {
                tracing::error!(
                    found,
                    supported = SCHEMA_VERSION,
                    "the store's schema is newer than this build: every provider waits until tagteam is upgraded"
                );
                true
            }
            Err(e) => return Err(e.into()),
        };
        let settings = engine.settings();
        let board = StatusBoard::new(
            DaemonInfo {
                pid: std::process::id(),
                version: opts.version.clone(),
                started_at: engine.now_ms().div_euclid(1000),
                socket: None,
                log: engine.env().log_file(),
            },
            NotifyInfo {
                enabled: settings.notify_enabled,
                backend: opts.backend.as_ref().map(Backend::name),
                events: settings.notify_events.clone(),
            },
        );
        let notifier = opts
            .backend
            .clone()
            .map(|backend| Notifier::start(backend, engine.spawner()));
        tracing::info!(
            pid = std::process::id(),
            version = %opts.version,
            "the daemon started"
        );
        Ok(Started {
            engine,
            lock,
            board,
            opts,
            schema_newer,
            settings_mtime,
            settings_provider,
            store,
            notifier,
        })
    }
}

/// D4.1 step 2: `daemon.lock`, tried at once and then every `retry` while another process
/// holds it. The first failed try is said once, naming the holder by its record's pid. The
/// wait is a cancellation point (§14.1).
fn wait_for_lock(
    engine: &Engine,
    notice: &mut dyn FnMut(&str),
    retry: Duration,
) -> Result<FlockGuard, EngineError> {
    let path = daemon_lock_path(engine.env());
    let mut told = false;
    loop {
        if let Some(signal) = engine.cancel().requested() {
            return Err(EngineError::Interrupted(signal));
        }
        if let Some(lock) = FlockGuard::try_lock(&path)? {
            return Ok(lock);
        }
        if !told {
            told = true;
            let holder = match read_daemon_record(&path) {
                Read::Present(r) => Some(r.pid),
                Read::Absent | Read::Unreadable(_) => None,
            };
            tracing::info!(
                holder_pid = holder,
                "another daemon holds daemon.lock; waiting for it to stop"
            );
            notice(&match holder {
                Some(pid) => format!("a daemon already runs (pid {pid}); waiting for it to stop"),
                None => "a daemon already runs in another process; waiting for it to stop".into(),
            });
        }
        let until = Instant::now() + retry;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            if let Some(signal) = engine.cancel().requested() {
                return Err(EngineError::Interrupted(signal));
            }
            thread::sleep(left.min(WAIT_SLICE));
        }
    }
}

/// `daemon.lock`, held for the daemon's life. Its record is emptied before the lock goes, as
/// Decision 5 does for engine locks, so `doctor` never names a daemon that stopped (D10). An
/// I/O error is logged and otherwise ignored.
struct DaemonLock(FlockGuard);

impl Drop for DaemonLock {
    fn drop(&mut self) {
        let emptied = OpenOptions::new()
            .write(true)
            .open(self.0.path())
            .and_then(|file| file.set_len(0));
        if let Err(e) = emptied {
            tracing::warn!(kind = %e.kind(), "could not empty the daemon lock's record");
        }
    }
}

/// A daemon that holds `daemon.lock` and has not run yet.
pub struct Started<'e> {
    engine: &'e Engine,
    lock: DaemonLock,
    board: Arc<StatusBoard>,
    opts: DaemonOptions,
    /// D4.1 step 4: the store's schema was already newer than this build at start.
    schema_newer: bool,
    /// `config.toml`'s mtime, read before the settings the daemon serves with (Decision 9).
    settings_mtime: Option<SystemTime>,
    /// The provider whose tables the engine's own settings are read for (Decision 3).
    settings_provider: ProviderId,
    /// What the store watch's first run compares with (Decision 11).
    store: StoreBaseline,
    notifier: Option<(Notifier, NoticeQueue)>,
}

impl<'e> Started<'e> {
    /// The snapshot plan B's server answers `status` from (Decision 12).
    pub fn board(&self) -> Arc<StatusBoard> {
        Arc::clone(&self.board)
    }

    /// Runs the scheduler (D4.2) with these jobs, added in this order:
    /// - the settings job: the engine's own settings and the board's notifications (Decision 3)
    /// - the store watch, which wakes the provider jobs after `changed` (D4.4), and the upgrade
    ///   watch (D4.5)
    /// - one provider job per provider (D4.3)
    ///
    /// Every job is first due now, and the scheduler runs a tie in the order the jobs were
    /// added (Task 7), so the first pass checks the store and the program before any provider
    /// job ticks or collects: a store replaced since `start` is forgotten, and a newer binary
    /// or schema stops the daemon, before any provider work touches the store.
    ///
    /// Every event goes to `sink`, and to the notifier's `NotifySink`, which announces each
    /// notice back to `sink`. When the run ends, `stopping` is emitted. Then the jobs drop
    /// (their `AutoEngine`s empty and release the engine locks), the notifier stops, and
    /// `daemon.lock` goes, record first (D4.6).
    pub fn serve(
        self,
        sink: &dyn DaemonSink,
        sleeper: &dyn Sleeper,
        jitter: &mut dyn FnMut() -> f64,
    ) -> DaemonExit {
        let Started {
            engine,
            lock,
            board,
            opts,
            schema_newer,
            settings_mtime,
            settings_provider,
            store: baseline,
            notifier,
        } = self;
        let queue = notifier.as_ref().map(|(_, q)| q.clone());
        let providers = match &opts.provider {
            Some(p) => vec![p.clone()],
            None => engine.provider_ids(),
        };
        let now = engine.now_ms();
        let mut scheduler = Scheduler::new().with_settings_mtime(settings_mtime);
        // First, so the engine's own settings are read again before any provider job hears of
        // a change.
        let settings = SettingsJob {
            board: Arc::clone(&board),
            backend: opts.backend.as_ref().map(Backend::name),
            provider: settings_provider,
        };
        scheduler.add(Box::new(settings), now);
        // The watches before every provider job (Decision 11's invariant): a tie runs in the
        // order added, so the first pass's store and program checks precede any provider work.
        // The store watch's first run compares with start's baseline, and after any `changed`
        // every provider job re-evaluates its mode at once (D4.3).
        let store = StoreWatch::new(Arc::clone(&board), baseline).waking(scheduler.wake());
        scheduler.add(Box::new(store), now);
        // Every run compares with the caller's stamp, taken before start (Decision 17).
        let upgrade = UpgradeWatch::new(opts.program.clone(), schema_newer, Arc::clone(&board))
            .with_period(opts.upgrade_watch_ms);
        scheduler.add(Box::new(upgrade), now);
        for provider in providers {
            let ctx = ProviderJobCtx {
                board: Arc::clone(&board),
                notices: queue.clone(),
                keychain_counts: opts.keychain_counts.clone(),
                schema_newer,
            };
            scheduler.add(Box::new(ProviderJob::new(engine, provider, ctx)), now);
        }
        let notify = queue.map(|q| NotifySink::new(engine, q).echoing(sink));
        let mut sinks: Vec<&dyn DaemonSink> = vec![sink];
        if let Some(n) = &notify {
            sinks.push(n);
        }
        let all = Fanout(sinks);
        let reason = scheduler.run(
            &mut JobCx {
                engine,
                sink: &all,
                jitter,
            },
            sleeper,
        );
        all.emit(&DaemonEvent::Stopping { reason });
        tracing::info!(reason = reason.as_str(), "the daemon stopped");
        drop(scheduler);
        drop(all);
        drop(notify);
        if let Some((notifier, queue)) = notifier {
            drop(queue);
            notifier.stop();
        }
        drop(lock);
        match reason {
            StopReason::Signal => DaemonExit::Signal,
            StopReason::Upgrade | StopReason::Schema => DaemonExit::Restart,
        }
    }
}

/// The daemon's settings job, with two duties on every settings reload (D4.2):
/// - the engine's own settings are read again for `provider`, the provider `start` read them
///   for (D3 change 1, Decision 3), so the collector and the views read what the file says now
/// - the board's `notifications` follow `notify.enabled` and that provider's `notify.events`
///   (D5.4); it also sets them at start
///
/// The reload's warnings are dropped: each provider job emits its own provider's as
/// `config-warning` events (Decision 8), so a second emitter would print every warning twice.
/// It writes no `JobStatus`, so it never shows in `status.jobs`.
struct SettingsJob {
    board: Arc<StatusBoard>,
    backend: Option<&'static str>,
    /// `--provider`'s, else the default provider: whose tables the command read (Decision 3).
    provider: ProviderId,
}

impl SettingsJob {
    /// The board's `notifications`, from the engine's settings as they now stand.
    fn publish(&self, engine: &Engine) {
        let settings = engine.settings();
        self.board.set_notifications(NotifyInfo {
            enabled: settings.notify_enabled,
            backend: self.backend,
            events: settings.notify_events.clone(),
        });
    }
}

impl Job for SettingsJob {
    fn name(&self) -> String {
        "settings".into()
    }

    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
        self.publish(cx.engine);
        Ran::After(SETTINGS_JOB_MS)
    }

    fn settings_changed(&mut self, cx: &mut JobCx<'_>) {
        let _warnings = cx.engine.reload_settings(&self.provider);
        self.publish(cx.engine);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use serde_json::json;
    use tagteam_core::AccountId;
    use tagteam_engine::auto::AutoEvent;
    use tagteam_engine::daemon::DaemonRecord;
    use tagteam_engine::settings::{NotifyEvent, config_path};
    use tagteam_provider::process::Captured;
    use tagteam_provider::profile::ProfileMarker;

    // `super::*` already brings `Instant`, `NotifyInfo`, `FlockGuard`, `Read`, `RunShell`,
    // `ProgramStamp` and the lock-record functions.
    use super::*;
    use crate::events::{Mode, WaitingFor};
    use crate::notify::Notice;
    use crate::testutil::{Emitted, Fx, Slices, T0, capture_logs, cc, fake, windows};

    const VERSION: &str = "20261014.2045.0";
    const SIGTERM: i32 = 15;

    /// What `tagteam daemon` passes, with a program file of the fixture's own, outside HOME,
    /// stamped now as `App::daemon` stamps it first (Decision 17). Unless a test replaces it,
    /// it never changes, so the upgrade watch never stops a run (D4.5).
    fn options(home: &Fx) -> DaemonOptions {
        let program = home.env.home.parent().unwrap().join("tagteam-bin");
        if !program.exists() {
            fs::write(&program, "a tagteam binary").unwrap();
        }
        DaemonOptions {
            provider: None,
            version: VERSION.into(),
            program: ProgramStamp::take(program),
            upgrade_watch_ms: 60_000,
            keychain_counts: Vec::new(),
            backend: None,
        }
    }

    /// `daemon.lock`'s content.
    fn record(home: &Fx) -> Vec<u8> {
        fs::read(daemon_lock_path(&home.env)).unwrap()
    }

    /// The scheduler's sleep on the fixture clock (`testutil::Slices`), with SIGTERM in the
    /// cancel token after slice `last`, counted from 1, as a `kill` would leave it.
    fn stop_after(home: &Fx, last: usize) -> Slices<'_> {
        stop_after_with(home, last, |_| {})
    }

    /// `stop_after`, running `on_slice` with each slice's number, from 1, before the signal.
    fn stop_after_with<'a>(home: &'a Fx, last: usize, on_slice: impl Fn(usize) + 'a) -> Slices<'a> {
        let cancel = home.engine.cancel().clone();
        Slices::new(&home.clock, move |n| {
            on_slice(n + 1);
            if n + 1 == last {
                cancel.request(SIGTERM);
            }
        })
    }

    /// `work` at position 1 at 20 % of its 7d window, and a (alias `alias_a`) at 2, live, at
    /// `a_long` percent.
    fn two_at(a_long: f64, alias_a: Option<&str>) -> Fx {
        let home = Fx::new();
        let w = home.add_as("w@b.example", "Acme", Some("work"));
        let a = home.add_as("a@b.example", "Acme", alias_a);
        home.read_windows(&w, &windows(10.0, 20.0), T0, T0 + 300);
        home.read_windows(&a, &windows(10.0, a_long), T0, T0 + 300);
        home
    }

    #[test]
    fn a_held_daemon_lock_is_announced_once_and_taken_over_when_it_is_released() {
        // D4.1 step 2: the holder is named by its record's pid, once, and the lock is tried
        // again until it is free. The new holder's record replaces the old one.
        let home = Fx::new();
        let path = daemon_lock_path(&home.env);
        let first = FlockGuard::try_lock(&path).unwrap().unwrap();
        write_daemon_record(&first, "first").unwrap();
        let release = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            drop(first);
        });
        let mut notices = Vec::new();
        let started = Daemon::start_with(
            &home.engine,
            options(&home),
            &mut |n| notices.push(n.to_owned()),
            Duration::from_millis(50),
        )
        .unwrap();
        release.join().unwrap();
        assert_eq!(
            notices,
            [format!(
                "a daemon already runs (pid {}); waiting for it to stop",
                std::process::id()
            )]
        );
        match read_daemon_record(&path) {
            Read::Present(DaemonRecord { pid, version, .. }) => {
                assert_eq!((pid, version.as_str()), (std::process::id(), VERSION));
            }
            other => panic!("no record: {other:?}"),
        }
        // A start that never serves still empties its record as it lets the lock go.
        drop(started);
        assert!(record(&home).is_empty());
        assert!(FlockGuard::try_lock(&path).unwrap().is_some());
    }

    #[test]
    fn a_program_replaced_while_waiting_for_daemon_lock_stops_at_the_first_upgrade_watch_run() {
        // D4.5, Decision 17: the stamp is the caller's, taken before the wait. A tagteam
        // installed while this daemon waits for another to stop (the other holder lets go only
        // after the install) is an upgrade at once: serving ends at the upgrade watch's first
        // run, before the scheduler ever sleeps and before any provider job runs (Decision 11's
        // order), and the daemon exits to be restarted (75).
        let home = Fx::new();
        let opts = options(&home);
        let program = opts.program.path().to_path_buf();
        let path = daemon_lock_path(&home.env);
        let first = FlockGuard::try_lock(&path).unwrap().unwrap();
        write_daemon_record(&first, "first").unwrap();
        let install = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            let next = program.with_file_name("tagteam-bin.new");
            fs::write(&next, "a newer tagteam binary").unwrap();
            fs::rename(&next, &program).unwrap();
            drop(first);
        });
        let started =
            Daemon::start_with(&home.engine, opts, &mut |_| {}, Duration::from_millis(50)).unwrap();
        install.join().unwrap();
        let sink = Emitted::new(&home.clock);
        let exit = started.serve(&sink, &Slices::none(&home.clock), &mut || 0.0);
        assert_eq!(exit, DaemonExit::Restart);
        assert_eq!(
            sink.events(),
            [(
                0,
                DaemonEvent::Stopping {
                    reason: StopReason::Upgrade
                }
            )],
            "no provider job ran, so no `mode` event came first"
        );
    }

    #[test]
    fn a_signal_while_waiting_for_daemon_lock_ends_start_interrupted() {
        // §14.1: the wait is a cancellation point. A lock whose record is not written yet is
        // held by "another process".
        let home = Fx::new();
        let _held = FlockGuard::try_lock(&daemon_lock_path(&home.env))
            .unwrap()
            .unwrap();
        let cancel = home.engine.cancel().clone();
        let signal = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            cancel.request(SIGTERM);
        });
        let mut notices = Vec::new();
        let err = Daemon::start_with(
            &home.engine,
            options(&home),
            &mut |n| notices.push(n.to_owned()),
            Duration::from_secs(5),
        )
        .err()
        .unwrap();
        signal.join().unwrap();
        let DaemonError::Engine(e) = err;
        assert_eq!(e.signal(), Some(SIGTERM));
        assert_eq!(
            notices,
            ["a daemon already runs in another process; waiting for it to stop"]
        );
    }

    #[test]
    fn inside_a_run_shell_start_refuses_before_daemon_lock_is_tried() {
        // D4.1 step 1, as `auto` refuses (§12.8).
        let profile = PathBuf::from("/nonexistent/profile");
        let home = Fx::with_run_shell(RunShell::Inside {
            profile: profile.clone(),
            marker: ProfileMarker {
                provider: cc(),
                account_id: AccountId::from_string("a"),
                config_dir: profile.display().to_string(),
                outer: json!({}),
            },
        });
        let err = Daemon::start(&home.engine, options(&home), &mut |n| panic!("{n}"))
            .err()
            .unwrap();
        let DaemonError::Engine(e) = err;
        assert!(matches!(e, EngineError::InsideRunShell), "{e}");
        assert!(!daemon_lock_path(&home.env).exists());
    }

    #[test]
    fn a_store_from_a_newer_tagteam_is_logged_once_and_every_provider_waits_for_the_schema() {
        // D4.1 step 4: not an exit, since a service would restart into the same refusal.
        let home = Fx::new();
        home.add_as("a@b.example", "Acme", None);
        rusqlite::Connection::open(home.env.data_dir().join("tagteam.db"))
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        let engine = home.other();
        let (started, lines) =
            capture_logs(|| Daemon::start(&engine, options(&home), &mut |_| {}).unwrap());
        let errors: Vec<&str> = lines
            .iter()
            .map(|l| l.trim_start())
            .filter(|l| l.starts_with("ERROR"))
            .collect();
        assert_eq!(errors.len(), 1, "{lines:#?}");
        assert!(
            errors[0].contains("newer than this build") && errors[0].contains("found=99"),
            "{}",
            errors[0]
        );
        let board = started.board();
        let exit = started.serve(
            &Emitted::new(&home.clock),
            &stop_after(&home, 2),
            &mut || 0.0,
        );
        assert_eq!(exit, DaemonExit::Signal);
        let provider = board
            .snapshot()
            .providers
            .into_iter()
            .find(|p| p.provider == cc())
            .unwrap();
        assert_eq!(
            (provider.mode, provider.waiting_for),
            (Mode::Waiting, Some(WaitingFor::Schema))
        );
    }

    #[test]
    fn a_signal_ends_serving_with_stopping_last_and_every_lock_record_emptied() {
        // D4.6. Two switchable accounts are `auto`, so the provider job held the engine lock
        // and ticked. The run ends at the slice after SIGTERM, and `stopping` is the last
        // event. Once `serve` returns, both lock files are empty and free.
        let home = two_at(60.0, None);
        let started = Daemon::start(&home.engine, options(&home), &mut |n| panic!("{n}")).unwrap();
        let board = started.board();
        let sink = Emitted::new(&home.clock);
        let exit = started.serve(&sink, &stop_after(&home, 3), &mut || 0.0);
        assert_eq!(exit, DaemonExit::Signal);
        let events: Vec<DaemonEvent> = sink.events().into_iter().map(|(_, e)| e).collect();
        assert_eq!(
            events.last(),
            Some(&DaemonEvent::Stopping {
                reason: StopReason::Signal
            })
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, DaemonEvent::Stopping { .. }))
                .count(),
            1
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, DaemonEvent::Auto(AutoEvent::NoSwitch { .. }))),
            "it ticked: {events:?}"
        );
        assert!(
            !events.contains(&DaemonEvent::Changed),
            "nothing changed since start's baseline, and the tick's own writes are no change"
        );
        let provider = board
            .snapshot()
            .providers
            .into_iter()
            .find(|p| p.provider == cc())
            .unwrap();
        assert_eq!(provider.mode, Mode::Auto, "it held the engine lock");
        assert!(record(&home).is_empty());
        assert!(
            FlockGuard::try_lock(&daemon_lock_path(&home.env))
                .unwrap()
                .is_some()
        );
        assert!(matches!(home.engine.engine_holder(&cc()), Read::Absent));
        let engine_lock = home
            .env
            .data_dir()
            .join("locks/autoswitch-claude-code.lock");
        assert!(FlockGuard::try_lock(&engine_lock).unwrap().is_some());
    }

    #[test]
    fn a_store_created_while_serving_wakes_the_provider_jobs_at_once() {
        // D4.3, D4.4: `serve` builds its store watch waking the provider jobs (Task 10). A
        // first `tagteam add` from another process creates the store. The watch's next run
        // reports `changed`, and Claude Code's job leaves `idle` before the scheduler sleeps
        // again, not at its next 60 s look.
        let home = Fx::new();
        let other = home.other();
        let started = Daemon::start(&home.engine, options(&home), &mut |n| panic!("{n}")).unwrap();
        let sink = Emitted::new(&home.clock);
        let slices = stop_after_with(&home, 3, |n| {
            if n == 2 {
                home.add_on(&other, "a@x.co");
            }
        });
        assert_eq!(
            started.serve(&sink, &slices, &mut || 0.0),
            DaemonExit::Signal
        );
        assert_eq!(
            sink.modes(&cc()),
            [(0, Mode::Idle, None, None), (2, Mode::Collect, None, None)]
        );
    }

    #[test]
    fn a_store_replaced_between_start_and_serving_is_reopened_before_any_provider_work() {
        // Decision 11: `start` takes the store watch's baseline on a fresh connection, after
        // `daemon.lock`. The store is deleted and another process creates a new one before
        // `serve` (plan B binds its socket in between). The watch's first run sees another
        // file: `changed`, once, at once, and the engine reads the new store from then on.
        // Its invariant: that check precedes all provider work. A provider job's first act in
        // a run is its mode evaluation, which is a `mode` event the first time, and a tick or
        // round only follows it. So `changed` being the run's first event means no provider
        // job evaluated, ticked or collected before the engine's connection to the deleted
        // store was gone: nothing was written into it.
        let home = Fx::new();
        home.add_as("a@b.example", "Acme", None);
        let started = Daemon::start(&home.engine, options(&home), &mut |n| panic!("{n}")).unwrap();
        home.remove_store();
        let other = home.other();
        home.add_on(&other, "c@x.co");
        let sink = Emitted::new(&home.clock);
        assert_eq!(
            started.serve(&sink, &stop_after(&home, 1), &mut || 0.0),
            DaemonExit::Signal
        );
        let seen: Vec<(i64, DaemonEvent)> = sink.events();
        assert_eq!(seen.first(), Some(&(0, DaemonEvent::Changed)), "{seen:?}");
        let changed: Vec<i64> = seen
            .iter()
            .filter(|(_, e)| *e == DaemonEvent::Changed)
            .map(|(at, _)| *at)
            .collect();
        assert_eq!(changed, [0]);
        assert_eq!(
            sink.modes(&cc()),
            [(0, Mode::Collect, None, None)],
            "one evaluation, of the new store's one account"
        );
        let emails: Vec<Option<String>> = home
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
    }

    #[test]
    fn settings_written_after_the_engine_was_built_are_the_ones_start_serves_with() {
        // Decision 9, Decision 3: the engine read `config.toml` when the command built it,
        // before an unlock offer that can wait on a person. `start` reads the mtime, then the
        // engine's settings again, for the provider the command acts on, so a write in between
        // is neither lost nor a change the scheduler never sees.
        let home = Fx::new();
        home.settings(
            "[notify]\nenabled = false\n\n[provider.fake-agent.notify]\nevents = [\"limit\"]\n",
            1,
        );
        let started = Daemon::start(&home.engine, options(&home), &mut |n| panic!("{n}")).unwrap();
        let notifications = started.board().snapshot().notifications;
        assert!(!notifications.enabled);
        assert_eq!(
            notifications.events,
            NotifyEvent::ALL,
            "the default provider's"
        );
        assert!(!home.engine.settings().notify_enabled);
        drop(started);
        let opts = DaemonOptions {
            provider: Some(fake()),
            ..options(&home)
        };
        let started = Daemon::start(&home.engine, opts, &mut |n| panic!("{n}")).unwrap();
        assert_eq!(
            started.board().snapshot().notifications.events,
            [NotifyEvent::Limit],
            "--provider's own table first (§6.4)"
        );
    }

    #[test]
    fn a_settings_file_corrupted_before_start_is_warned_before_any_provider_work() {
        // Decision 8. The file pauses switching and is valid when the engine is built, then is
        // corrupted before `Daemon::start` (during an unlock offer, say). Start's mtime is read
        // after the write, so no reload will ever report it. Each provider job's first run
        // emits its first read's warning before its mode is evaluated, and so before any tick
        // or round; the defaults then switch again (`auto`).
        let home = two_at(60.0, None);
        home.settings("[daemon]\nautoswitch = false\n", 1);
        home.settings("[daemon\nautoswitch = false\n", 2);
        let started = Daemon::start(&home.engine, options(&home), &mut |n| panic!("{n}")).unwrap();
        let sink = Emitted::new(&home.clock);
        assert_eq!(
            started.serve(&sink, &stop_after(&home, 1), &mut || 0.0),
            DaemonExit::Signal
        );
        let seen: Vec<DaemonEvent> = sink.events().into_iter().map(|(_, e)| e).collect();
        let first = |want: fn(&DaemonEvent) -> bool| {
            seen.iter()
                .position(want)
                .unwrap_or_else(|| panic!("{seen:?}"))
        };
        let warned = first(
            |e| matches!(e, DaemonEvent::Auto(AutoEvent::ConfigWarning { provider, .. }) if *provider == cc()),
        );
        let evaluated =
            first(|e| matches!(e, DaemonEvent::Mode { provider, .. } if *provider == cc()));
        let ticked = first(|e| {
            matches!(
                e,
                DaemonEvent::Auto(AutoEvent::Poll { .. } | AutoEvent::NoSwitch { .. })
            )
        });
        assert!(warned < evaluated && evaluated < ticked, "{seen:?}");
        assert_eq!(
            seen[warned],
            DaemonEvent::Auto(AutoEvent::ConfigWarning {
                provider: cc(),
                message: format!(
                    "{}: the settings file is not valid TOML; using the defaults",
                    config_path(&home.env).display()
                ),
            })
        );
        assert_eq!(sink.modes(&cc()), [(0, Mode::Auto, None, None)]);
    }

    #[test]
    fn a_switch_while_serving_is_announced_and_delivered_through_the_notifier() {
        // D6.1, D6.2, D8.1. The daemon's own switch is queued for the notifier, which runs
        // the backend through the engine's spawner. It is also announced to the caller's sink.
        let home = two_at(95.0, Some("spare"));
        home.spawner.push(Captured::Exited {
            code: Some(0),
            signal: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
        });
        let program = PathBuf::from("/usr/bin/notify-send");
        let opts = DaemonOptions {
            backend: Some(Backend::NotifySend(program.clone())),
            ..options(&home)
        };
        let started = Daemon::start(&home.engine, opts, &mut |n| panic!("{n}")).unwrap();
        let sink = Emitted::new(&home.clock);
        let delivered = || {
            home.spawner
                .specs()
                .into_iter()
                .filter(|s| s.program == program)
                .collect::<Vec<_>>()
        };
        let slices = stop_after_with(&home, 2, |n| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while n == 2 && delivered().is_empty() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
        });
        assert_eq!(
            started.serve(&sink, &slices, &mut || 0.0),
            DaemonExit::Signal
        );
        let body = "Switched 2 · spare → 1 · work (7d at 95%)";
        assert_eq!(
            sink.announced(),
            [(
                cc(),
                Notice {
                    title: "tagteam".into(),
                    body: body.into()
                }
            )]
        );
        let specs = delivered();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].args.last().unwrap(), body);
    }

    #[test]
    fn the_board_s_notifications_and_the_engine_s_settings_follow_the_file_while_serving() {
        // D5.4 through D4.2: `notify.enabled` set false while the daemon runs shows on the
        // board within a slice or two. The settings job reloads the engine's own settings too
        // (Decision 3), which the collector and the views read.
        let home = Fx::new();
        let opts = DaemonOptions {
            backend: Some(Backend::Osascript),
            ..options(&home)
        };
        let started = Daemon::start(&home.engine, opts, &mut |n| panic!("{n}")).unwrap();
        let board = started.board();
        assert_eq!(
            board.snapshot().notifications,
            NotifyInfo {
                enabled: true,
                backend: Some("osascript"),
                events: vec![
                    NotifyEvent::Switch,
                    NotifyEvent::Quarantine,
                    NotifyEvent::Limit
                ],
            }
        );
        let slices = stop_after_with(&home, 3, |n| {
            if n == 1 {
                home.settings("[notify]\nenabled = false\n", 1);
            }
        });
        started.serve(&Emitted::new(&home.clock), &slices, &mut || 0.0);
        assert!(!board.snapshot().notifications.enabled);
        assert!(
            !home.engine.settings().notify_enabled,
            "the engine's settings were read again"
        );
    }
}
