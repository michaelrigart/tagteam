//! The daemon's jobs, run by a process that is not the daemon (TUI auto-switch spec §3.1 and
//! §3.2): the settings job, the store watch, the schema half of the upgrade watch and one
//! provider job per provider, on one scheduler. The jobs run only while no daemon does. A
//! gate looks at `daemon.lock`'s record every `SLICE_MS`, and every provider job looks before
//! each run and before it hears of a settings change, so no provider work starts on a stale
//! look, even for a job overdue behind a long one. While a daemon's record is live the
//! provider jobs are dropped, which releases their engine locks; once it has been dead for
//! `GRACE_MS` of forward time they are built again.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use tagteam_core::ProviderId;
use tagteam_engine::Engine;
use tagteam_engine::daemon::live_daemon_pid;
use tagteam_engine::settings::Settings;
use tagteam_provider::RunShell;
use tagteam_provider::keychain::ReadCounts;

use crate::daemon::SettingsJob;
use crate::events::{DaemonSink, StopReason};
use crate::provider_job::{JOB_PREFIX, ProviderJob, ProviderJobCtx};
use crate::scheduler::{Job, JobCx, Ran, SLICE_MS, Scheduler, Sleeper, Wake};
use crate::status::{DaemonInfo, NotifyInfo, ProviderStatus, StatusBoard, StatusSnapshot};
use crate::watch::{StoreBaseline, StoreWatch, UpgradeWatch, schema_newer};

/// How long `daemon.lock`'s record must have been dead, in forward time on the engine's clock,
/// before the jobs start after a daemon: an upgrade restart finds its locks free within it.
pub const GRACE_MS: i64 = 30_000;
/// How long a provider job that is stopped waits before it looks again, when nothing wakes it.
const STOPPED_RECHECK_MS: i64 = 60_000;

/// What the caller decides.
pub struct HostedOptions {
    /// Whose tables the settings job reads the engine's settings for (Decision 3).
    pub settings_provider: ProviderId,
    /// Decision 7: the read counts the keychain backoff compares (D4.1); empty on Linux.
    pub keychain_counts: Vec<Arc<ReadCounts>>,
}

/// Why `run_hosted` returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostedEnd {
    /// The engine's cancel token ended the scheduler. Every lock has been released.
    Signal,
    /// Nothing ran: the store could not be opened.
    Off,
    /// The store's schema is newer than this build's, at start or later. Every lock has been
    /// released.
    Schema,
}

/// Whether the jobs run: the pure rule. It keeps the forward time `daemon.lock`'s record has
/// been dead, which a step adds to only when this look and the one before both found no live
/// record, and only by time that moved forward. A clock set back adds nothing, so it never
/// stretches the grace, and a stretch of time no look saw (a suspend) counts at its first.
#[derive(Debug)]
struct Gate {
    runs: bool,
    dead_ms: i64,
    last_ms: i64,
    last_live: bool,
}

impl Gate {
    /// The first look: the jobs run unless a daemon's record is live.
    fn at_start(daemon_live: bool, now_ms: i64) -> Self {
        Gate {
            runs: !daemon_live,
            dead_ms: 0,
            last_ms: now_ms,
            last_live: daemon_live,
        }
    }

    /// One more look, at `now_ms` on the engine's clock. Whether the jobs' running changed.
    fn step(&mut self, daemon_live: bool, now_ms: i64) -> bool {
        if daemon_live {
            self.dead_ms = 0;
        } else if !self.last_live {
            self.dead_ms = self.dead_ms.saturating_add((now_ms - self.last_ms).max(0));
        }
        self.last_ms = now_ms;
        self.last_live = daemon_live;
        let runs = !daemon_live && (self.runs || self.dead_ms >= GRACE_MS);
        std::mem::replace(&mut self.runs, runs) != runs
    }

    fn runs(&self) -> bool {
        self.runs
    }
}

/// What the gate job and every provider job share: the gate, the board and the report.
struct Shared<'a> {
    board: Arc<StatusBoard>,
    gate: RefCell<Gate>,
    wake: Wake,
    on_status: &'a dyn Fn(&StatusSnapshot),
    /// The provider lines last passed to `on_status`.
    reported: RefCell<Option<Vec<ProviderStatus>>>,
}

impl Shared<'_> {
    /// One look at `daemon.lock`'s record: steps the gate, and wakes every provider job when
    /// it flips, so each drops or builds its job at the next pass. Whether the jobs run.
    fn look(&self, engine: &Engine) -> bool {
        let daemon_live = live_daemon_pid(engine.env()).is_some();
        let mut gate = self.gate.borrow_mut();
        if gate.step(daemon_live, engine.now_ms()) {
            tracing::info!(
                jobs_run = gate.runs(),
                "the auto jobs started or stopped, by whether a daemon runs"
            );
            self.wake.wake(JOB_PREFIX);
        }
        gate.runs()
    }

    /// Passes the board to `on_status` when its provider lines differ from the last passed.
    /// While the jobs run, a board with no line is one no provider job has reported to yet.
    fn report(&self) {
        let snapshot = self.board.snapshot();
        if snapshot.providers.is_empty() && self.gate.borrow().runs() {
            return;
        }
        {
            let mut reported = self.reported.borrow_mut();
            if reported.as_ref() == Some(&snapshot.providers) {
                return;
            }
            *reported = Some(snapshot.providers.clone());
        }
        (self.on_status)(&snapshot);
    }
}

/// Looks at the gate every `SLICE_MS`, so a daemon that comes up while the scheduler sleeps is
/// seen within a second.
struct GateJob<'a> {
    shared: Rc<Shared<'a>>,
}

impl Job for GateJob<'_> {
    fn name(&self) -> String {
        "daemon-gate".into()
    }

    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
        self.shared.look(cx.engine);
        self.shared.report();
        Ran::After(SLICE_MS)
    }
}

/// One provider's job behind the gate: built while the jobs run, dropped (and with it the
/// engine lock) while they do not. It looks before each run and each settings change, since
/// the scheduler offers a change to every job before it picks one, and a provider job that
/// hears of one writes the store and may take its lock. It looks once more after a run, so a
/// daemon that came up meanwhile has the lock as the run returns.
struct HostedJob<'e, 'a> {
    engine: &'e Engine,
    provider: ProviderId,
    keychain_counts: Vec<Arc<ReadCounts>>,
    shared: Rc<Shared<'a>>,
    job: Option<ProviderJob<'e>>,
}

impl HostedJob<'_, '_> {
    /// A daemon runs: the job is dropped, which releases the lock, and its line goes.
    fn stand_down(&mut self) {
        self.job = None;
        self.shared.board.remove_provider(&self.provider);
    }
}

impl Job for HostedJob<'_, '_> {
    fn name(&self) -> String {
        format!("{JOB_PREFIX}{}", self.provider)
    }

    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
        let ran = if self.shared.look(self.engine) {
            let job = self.job.get_or_insert_with(|| {
                let ctx = ProviderJobCtx {
                    board: Arc::clone(&self.shared.board),
                    notices: None,
                    keychain_counts: self.keychain_counts.clone(),
                    schema_newer: false,
                };
                ProviderJob::new(self.engine, self.provider.clone(), ctx)
            });
            match job.run(cx) {
                // A daemon came up while the job ran: one more look, so the lock goes as the job
                // returns (S§3.2) and not at the next look.
                Ran::After(_) if !self.shared.look(self.engine) => None,
                ran => Some(ran),
            }
        } else {
            None
        };
        let ran = ran.unwrap_or_else(|| {
            self.stand_down();
            Ran::After(STOPPED_RECHECK_MS)
        });
        self.shared.report();
        ran
    }

    fn settings_changed(&mut self, cx: &mut JobCx<'_>) {
        if self.shared.look(self.engine) {
            if let Some(job) = self.job.as_mut() {
                job.settings_changed(cx);
            }
        } else {
            self.stand_down();
        }
    }
}

/// Runs the daemon's jobs on `engine` until its cancel token is set or a newer schema ends
/// them, with these jobs, added in this order:
/// - the gate
/// - the settings job: the engine's own settings (Decision 3)
/// - the store watch, which wakes the provider jobs after a change (D4.4)
/// - the schema half of the upgrade watch
/// - one provider job per provider (D4.3), with no notifier
///
/// Every job is first due now, and a tie runs in the order added. `on_status` is called with
/// the board whenever its provider lines differ from the last passed: after every provider
/// job's run and every look of the gate, and with no lines once no provider job runs. It is not
/// called while the jobs run and no provider job has a line yet. Every event goes to `sink`. The
/// jobs drop, and release their locks, before this returns.
///
/// Inside a `tagteam run` shell the engine refuses to switch (S§3.1), so no job runs but the
/// schema half of the upgrade watch (S§3.7): `on_off` is called once, at start, and the run
/// goes on to end `Schema` when the store's schema is newer than this build's, or `Signal` when
/// the token is set. It takes no lock and calls neither `on_status` nor `sink`.
pub fn run_hosted(
    engine: &Engine,
    opts: HostedOptions,
    sink: &dyn DaemonSink,
    on_status: &dyn Fn(&StatusSnapshot),
    on_off: &dyn Fn(),
    sleeper: &dyn Sleeper,
    jitter: &mut dyn FnMut() -> f64,
) -> HostedEnd {
    if matches!(engine.run_shell(), RunShell::Inside { .. }) {
        on_off();
        return schema_watch_alone(engine, sink, sleeper, jitter);
    }
    if schema_newer(engine) {
        return HostedEnd::Schema;
    }
    // Before the first read of the settings the jobs serve with, as the daemon's start does,
    // so a write landing meanwhile is a change at the scheduler's first check.
    let settings_mtime = Settings::mtime(engine.env());
    let board = hosted_board(engine);
    let baseline = match StoreBaseline::take(engine) {
        Ok(baseline) => baseline,
        Err(e) => {
            tracing::warn!(kind = e.kind(), "the auto jobs could not open the store");
            return HostedEnd::Off;
        }
    };
    let now = engine.now_ms();
    let mut scheduler = Scheduler::new().with_settings_mtime(settings_mtime);
    let shared = Rc::new(Shared {
        board: Arc::clone(&board),
        gate: RefCell::new(Gate::at_start(live_daemon_pid(engine.env()).is_some(), now)),
        wake: scheduler.wake(),
        on_status,
        reported: RefCell::new(None),
    });
    scheduler.add(
        Box::new(GateJob {
            shared: Rc::clone(&shared),
        }),
        now,
    );
    let settings = SettingsJob::new(Arc::clone(&board), None, opts.settings_provider);
    scheduler.add(Box::new(settings), now);
    let store = StoreWatch::new(Arc::clone(&board), baseline).waking(scheduler.wake());
    scheduler.add(Box::new(store), now);
    scheduler.add(Box::new(UpgradeWatch::schema_only(Arc::clone(&board))), now);
    for provider in engine.provider_ids() {
        let job = HostedJob {
            engine,
            provider,
            keychain_counts: opts.keychain_counts.clone(),
            shared: Rc::clone(&shared),
            job: None,
        };
        scheduler.add(Box::new(job), now);
    }
    let reason = scheduler.run(
        &mut JobCx {
            engine,
            sink,
            jitter,
        },
        sleeper,
    );
    drop(scheduler);
    match reason {
        StopReason::Signal => HostedEnd::Signal,
        StopReason::Upgrade | StopReason::Schema => HostedEnd::Schema,
    }
}

/// The board the jobs report to: this process, with no socket and no notifier.
fn hosted_board(engine: &Engine) -> Arc<StatusBoard> {
    StatusBoard::new(
        DaemonInfo {
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").into(),
            started_at: engine.now_s(),
            socket: None,
            log: engine.env().log_file(),
        },
        NotifyInfo {
            enabled: false,
            backend: None,
            events: Vec::new(),
        },
    )
}

/// The only job inside a `tagteam run` shell: the schema half of the upgrade watch, due at
/// once, so a store already newer ends the run before a sleep.
fn schema_watch_alone(
    engine: &Engine,
    sink: &dyn DaemonSink,
    sleeper: &dyn Sleeper,
    jitter: &mut dyn FnMut() -> f64,
) -> HostedEnd {
    let board = hosted_board(engine);
    let mut scheduler = Scheduler::new();
    scheduler.add(Box::new(UpgradeWatch::schema_only(board)), engine.now_ms());
    let reason = scheduler.run(
        &mut JobCx {
            engine,
            sink,
            jitter,
        },
        sleeper,
    );
    match reason {
        StopReason::Signal => HostedEnd::Signal,
        StopReason::Upgrade | StopReason::Schema => HostedEnd::Schema,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::sync::{Arc, Mutex};

    use serde_json::json;
    use tagteam_core::{AccountId, ProviderId};
    use tagteam_engine::auto::{AutoEngine, AutoEvent, HolderKind};
    use tagteam_engine::daemon::daemon_lock_path;
    use tagteam_engine::store::SCHEMA_VERSION;
    use tagteam_engine::{Engine, EventKind, EventSource};
    use tagteam_provider::http::{HttpError, HttpRequest, HttpResponse};
    use tagteam_provider::profile::ProfileMarker;
    use tagteam_provider::{Clock, Env, Http, ProcessStamp, Read, RunShell, ScriptedHttp};

    use super::*;
    use crate::auto::{AutoFlags, auto_config};
    use crate::events::{DaemonEvent, DaemonSink, Mode, WaitingFor};
    use crate::scheduler::Sleeper;
    use crate::status::{ProviderStatus, StatusSnapshot};
    use crate::testutil::{CC_USAGE, Emitted, Fx, Slices, T0, cc, fake, two};

    /// Two Claude Code accounts, `b` the live login at `b_long` percent of its long window and
    /// `a` a candidate at 10 %, both read at `T0` with their next poll five minutes out.
    fn read_two(fx: &Fx, b_long: f64) -> (AccountId, AccountId) {
        let (a, b) = two(fx);
        fx.read(&a, 10.0, 10.0, T0, T0 + 300);
        fx.read(&b, 20.0, b_long, T0, T0 + 300);
        (a, b)
    }

    /// `daemon.lock` as a running daemon leaves it: this process's pid and start time.
    fn daemon_up(env: &Env) {
        let path = daemon_lock_path(env);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let stamp = ProcessStamp::current().unwrap();
        let line = format!(
            "{{\"pid\":{},\"start\":{},\"version\":\"test\"}}\n",
            stamp.pid, stamp.start
        );
        fs::write(path, line).unwrap();
    }

    /// `daemon.lock` as a daemon that stopped leaves it: emptied.
    fn daemon_down(env: &Env) {
        let path = daemon_lock_path(env);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    /// Whether `of`'s engine lock has no record: nobody holds it.
    fn absent(fx: &Fx, of: &ProviderId) -> bool {
        matches!(fx.engine.engine_holder(of), Read::Absent)
    }

    /// The kind of holder `of`'s engine lock record names; `None` when it has no record, and
    /// `Some(None)` for a record that names no kind.
    fn holder(fx: &Fx, of: &ProviderId) -> Option<Option<HolderKind>> {
        match fx.engine.engine_holder(of) {
            Read::Present(h) => Some(h.kind),
            Read::Absent | Read::Unreadable(_) => None,
        }
    }

    /// The `on_status` calls of one run.
    type Calls = RefCell<Vec<StatusSnapshot>>;

    fn options() -> HostedOptions {
        HostedOptions {
            settings_provider: cc(),
            keychain_counts: Vec::new(),
        }
    }

    fn host(
        engine: &Engine,
        sink: &dyn DaemonSink,
        sleeper: &dyn Sleeper,
        calls: &Calls,
    ) -> HostedEnd {
        let on_status = |s: &StatusSnapshot| calls.borrow_mut().push(s.clone());
        let mut jitter = || 0.0;
        run_hosted(
            engine,
            options(),
            sink,
            &on_status,
            &|| panic!("only a run shell is off at start"),
            sleeper,
            &mut jitter,
        )
    }

    /// Runs the jobs on `engine` as `host` does, and counts how often `on_off` was called.
    fn host_counting_off(
        engine: &Engine,
        sink: &dyn DaemonSink,
        sleeper: &dyn Sleeper,
        calls: &Calls,
        off: &Cell<usize>,
    ) -> HostedEnd {
        let on_status = |s: &StatusSnapshot| calls.borrow_mut().push(s.clone());
        let on_off = || off.set(off.get() + 1);
        let mut jitter = || 0.0;
        run_hosted(
            engine,
            options(),
            sink,
            &on_status,
            &on_off,
            sleeper,
            &mut jitter,
        )
    }

    /// `PRAGMA user_version` past this build's, as a newer tagteam's migration leaves it.
    fn migrate_past(fx: &Fx) {
        let conn = rusqlite::Connection::open(fx.engine.store_file()).unwrap();
        conn.execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION + 1))
            .unwrap();
    }

    fn run_shell_fx() -> Fx {
        let profile = std::path::PathBuf::from("/nonexistent/profile");
        Fx::with_run_shell(RunShell::Inside {
            profile: profile.clone(),
            marker: ProfileMarker {
                provider: cc(),
                account_id: AccountId::from_string("a"),
                config_dir: profile.display().to_string(),
                outer: json!({}),
            },
        })
    }

    /// The provider lines of the last `on_status` call.
    fn last_lines(calls: &Calls) -> Option<Vec<ProviderStatus>> {
        calls.borrow().last().map(|s| s.providers.clone())
    }

    fn line(lines: &[ProviderStatus], of: &ProviderId) -> ProviderStatus {
        lines
            .iter()
            .find(|p| &p.provider == of)
            .expect("the provider has a line")
            .clone()
    }

    /// A usage port that acts while the request is held open: it notes the URL, runs `during`,
    /// and times out. Everything else is `ScriptedHttp`'s.
    struct HeldOpen {
        urls: Mutex<Vec<String>>,
        during: Box<dyn Fn() + Send + Sync>,
        others: ScriptedHttp,
    }

    impl HeldOpen {
        fn new(during: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
            Arc::new(HeldOpen {
                urls: Mutex::new(Vec::new()),
                during: Box::new(during),
                others: ScriptedHttp::new(),
            })
        }

        fn usage_urls(&self) -> Vec<String> {
            self.urls.lock().unwrap().clone()
        }
    }

    impl Http for HeldOpen {
        fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError> {
            if !req.url.ends_with("/usage") {
                return self.others.send(req);
            }
            self.urls.lock().unwrap().push(req.url.clone());
            (self.during)();
            Err(HttpError::Ambiguous("timed out".into()))
        }
    }

    fn replay(start_live: bool, steps: &[(bool, i64)]) -> Vec<(bool, bool)> {
        let mut gate = Gate::at_start(start_live, 0);
        steps
            .iter()
            .map(|&(live, now)| {
                let changed = gate.step(live, now);
                (changed, gate.runs())
            })
            .collect()
    }

    #[test]
    fn gate_rule() {
        // S§3.2: the jobs run at start unless a daemon's record is live; after a live record
        // they start again once it has been dead for `GRACE_MS` of forward time, counted
        // between looks that both found it dead.
        const T: bool = true;
        const F: bool = false;
        let back = 21_000 - 3_600_000;
        let rows = [
            ("no daemon at start", F, vec![(F, 1_000)], vec![(F, T)]),
            (
                "a daemon at start; 29 999 ms after its record died, still stopped; at 30 000, started",
                T,
                vec![(F, 5_000), (F, 34_999), (F, 35_000)],
                vec![(F, F), (F, F), (T, T)],
            ),
            (
                "a record back during the grace stops at once, and the grace restarts",
                T,
                vec![
                    (F, 1_000),
                    (F, 21_000),
                    (T, 22_000),
                    (F, 23_000),
                    (F, 52_999),
                    (F, 53_000),
                ],
                vec![(F, F), (F, F), (F, F), (F, F), (F, F), (T, T)],
            ),
            (
                "a record appearing while the jobs run stops them",
                F,
                vec![(F, 1_000), (T, 2_000), (T, 3_000)],
                vec![(F, T), (T, F), (F, F)],
            ),
            (
                "a clock set back an hour after 20 s of grace adds nothing; 10 s more starts it",
                T,
                vec![
                    (F, 1_000),
                    (F, 21_000),
                    (F, back),
                    (F, back + 9_999),
                    (F, back + 10_000),
                ],
                vec![(F, F), (F, F), (F, F), (F, F), (T, T)],
            ),
            (
                "a started gate keeps running while no record is live",
                T,
                vec![(F, 1_000), (F, 31_000), (F, 91_000), (F, 92_000)],
                vec![(F, F), (T, T), (F, T), (F, T)],
            ),
            (
                "a record that stays live never changes a stopped gate",
                T,
                vec![(T, 1_000), (T, 100_000)],
                vec![(F, F), (F, F)],
            ),
        ];
        for (what, start_live, steps, want) in rows {
            assert_eq!(replay(start_live, &steps), want, "{what}");
        }
    }

    #[test]
    fn with_no_daemon_the_jobs_take_the_lock_and_store_a_switch_as_auto() {
        let fx = Fx::new();
        read_two(&fx, 95.0);
        let engine = fx.tui_engine();
        let held = Cell::new(None);
        let cancel = engine.cancel().clone();
        let slices = Slices::new(&fx.clock, |n| match n {
            0 => held.set(Some(holder(&fx, &cc()))),
            _ => cancel.request(libc::SIGTERM),
        });
        let events = Emitted::new(&fx.clock);
        let calls = Calls::default();
        assert_eq!(host(&engine, &events, &slices, &calls), HostedEnd::Signal);
        assert_eq!(held.get(), Some(Some(Some(HolderKind::Tui))));
        assert!(
            events.events().iter().any(|(_, e)| matches!(
                e,
                DaemonEvent::Auto(AutoEvent::Switch {
                    from: 2,
                    to: 1,
                    dry_run: false,
                    ..
                })
            )),
            "the tick switched from 2 to 1"
        );
        let stored = fx
            .engine
            .store()
            .unwrap()
            .latest_event(&cc(), &[EventKind::Switch], None, None)
            .unwrap()
            .expect("the switch is stored");
        assert_eq!(stored.source, EventSource::Auto);
        let lines = last_lines(&calls).expect("on_status was called");
        assert_eq!(line(&lines, &cc()).mode, Mode::Auto);
        assert!(absent(&fx, &cc()), "returning released the lock");
    }

    #[test]
    fn a_live_daemon_record_frees_the_locks_at_the_next_passes() {
        let fx = Fx::new();
        read_two(&fx, 50.0);
        let engine = fx.tui_engine();
        let cancel = engine.cancel().clone();
        let seen = RefCell::new(Vec::new());
        let slices = Slices::new(&fx.clock, |n| match n {
            1 => {
                seen.borrow_mut().push(("held", !absent(&fx, &cc())));
                daemon_up(&fx.env);
            }
            2 => seen.borrow_mut().push(("freed", absent(&fx, &cc()))),
            3 => {
                seen.borrow_mut()
                    .push(("notify on", engine.settings().notify_enabled));
                fx.settings("[notify]\nenabled = false\n", 5);
            }
            5 => {
                seen.borrow_mut()
                    .push(("notify off", !engine.settings().notify_enabled));
                cancel.request(libc::SIGTERM);
            }
            _ => {}
        });
        let events = Emitted::new(&fx.clock);
        let calls = Calls::default();
        assert_eq!(host(&engine, &events, &slices, &calls), HostedEnd::Signal);
        assert_eq!(
            *seen.borrow(),
            [
                ("held", true),
                ("freed", true),
                ("notify on", true),
                ("notify off", true)
            ]
        );
        assert_eq!(last_lines(&calls), Some(Vec::new()));
    }

    #[test]
    fn a_record_that_appears_during_a_held_open_request_frees_the_lock_once_it_returns() {
        // No readings, so the first tick asks for usage. A daemon comes up while that request
        // is open: the lock stays held until the job returns, and is released before the next
        // tick.
        let fx = Fx::new();
        two(&fx);
        let (env, lock) = (
            fx.env.clone(),
            fx.env.data_dir().join("locks/autoswitch-claude-code.lock"),
        );
        let http = HeldOpen::new(move || {
            assert!(
                !fs::read(&lock).unwrap().is_empty(),
                "the lock's record is still there during the request"
            );
            daemon_up(&env);
        });
        let engine = fx.tui_engine_on(http.clone());
        let cancel = engine.cancel().clone();
        let when_it_returned = Cell::new(None);
        let slices = Slices::new(&fx.clock, |n| match n {
            0 => when_it_returned.set(Some((absent(&fx, &cc()), http.usage_urls()))),
            _ => cancel.request(libc::SIGTERM),
        });
        let events = Emitted::new(&fx.clock);
        let calls = Calls::default();
        assert_eq!(host(&engine, &events, &slices, &calls), HostedEnd::Signal);
        let (freed, during) = when_it_returned.take().unwrap();
        assert!(freed, "the lock was released before the next tick");
        assert!(!during.is_empty(), "the tick asked for usage");
        assert_eq!(http.usage_urls(), during, "and asked for nothing after it");
    }

    #[test]
    fn the_lock_goes_as_the_job_returns_and_not_at_the_next_look_of_the_gate() {
        // S§3.2: "once the job in flight returns". The daemon comes up while the request is
        // open; the first report after the run, which comes before any other job looks, finds
        // the lock free and the provider's line gone. Without the look after the run, the lock
        // would still be held then, and until the gate's next look a second later.
        let fx = Fx::new();
        two(&fx);
        let env = fx.env.clone();
        let http = HeldOpen::new(move || daemon_up(&env));
        let engine = fx.tui_engine_on(http);
        let cancel = engine.cancel().clone();
        let first = RefCell::new(None);
        let on_status = |s: &StatusSnapshot| {
            first
                .borrow_mut()
                .get_or_insert_with(|| (absent(&fx, &cc()), s.providers.len()));
        };
        let slices = Slices::new(&fx.clock, |_| cancel.request(libc::SIGTERM));
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let end = run_hosted(
            &engine,
            options(),
            &events,
            &on_status,
            &|| panic!("only a run shell is off at start"),
            &slices,
            &mut jitter,
        );
        assert_eq!(end, HostedEnd::Signal);
        assert_eq!(
            *first.borrow(),
            Some((true, 0)),
            "free, and no line, at the first report after the run"
        );
    }

    #[test]
    fn the_jobs_start_again_30_s_after_the_record_dies_and_not_before() {
        let fx = Fx::new();
        read_two(&fx, 50.0);
        daemon_up(&fx.env);
        let engine = fx.tui_engine();
        let cancel = engine.cancel().clone();
        let at = |s: i64| (T0 + s) * 1000;
        let seen = RefCell::new(Vec::new());
        let slices = Slices::new(&fx.clock, |_| {
            let now = fx.clock.now_ms();
            if now == at(5) {
                daemon_down(&fx.env);
            }
            if now == at(35) {
                seen.borrow_mut().push(("before 35 s", holder(&fx, &cc())));
            }
            if now == at(36) {
                seen.borrow_mut().push(("by 36 s", holder(&fx, &cc())));
                cancel.request(libc::SIGTERM);
            }
        });
        let events = Emitted::new(&fx.clock);
        let calls = Calls::default();
        assert_eq!(host(&engine, &events, &slices, &calls), HostedEnd::Signal);
        assert_eq!(
            *seen.borrow(),
            [
                ("before 35 s", None),
                ("by 36 s", Some(Some(HolderKind::Tui)))
            ]
        );
    }

    #[test]
    fn a_clock_set_back_during_the_grace_never_stretches_it() {
        let fx = Fx::new();
        read_two(&fx, 50.0);
        daemon_up(&fx.env);
        let engine = fx.tui_engine();
        let cancel = engine.cancel().clone();
        let jumped = Cell::new(None);
        let seen = RefCell::new(Vec::new());
        let slices = Slices::new(&fx.clock, |n| {
            if n == 0 {
                daemon_down(&fx.env);
            }
            if n == 21 {
                fx.clock.advance_ms(-3_600_000);
                jumped.set(Some(fx.clock.now_ms()));
            }
            if let Some(from) = jumped.get() {
                match fx.clock.now_ms() - from {
                    9_000 => seen.borrow_mut().push(("9 s after", holder(&fx, &cc()))),
                    12_000 => {
                        seen.borrow_mut().push(("12 s after", holder(&fx, &cc())));
                        cancel.request(libc::SIGTERM);
                    }
                    _ => {}
                }
            }
        });
        let events = Emitted::new(&fx.clock);
        let calls = Calls::default();
        assert_eq!(host(&engine, &events, &slices, &calls), HostedEnd::Signal);
        assert_eq!(
            *seen.borrow(),
            [
                ("9 s after", None),
                ("12 s after", Some(Some(HolderKind::Tui)))
            ]
        );
    }

    #[test]
    fn an_overdue_provider_never_starts_work_after_a_daemon_appears() {
        // Both providers would poll at once. Claude Code's request is open when a daemon comes
        // up and the clock moves on, so FakeAgent's job is overdue behind it: its own look
        // before its run stops it, whatever the scheduler picks next.
        let fx = Fx::new();
        two(&fx);
        fx.add_fake("alice");
        fx.add_fake("bob");
        let (env, clock) = (fx.env.clone(), fx.clock.clone());
        let http = HeldOpen::new(move || {
            daemon_up(&env);
            clock.advance_ms(500);
        });
        let engine = fx.tui_engine_on(http.clone());
        let cancel = engine.cancel().clone();
        let by_then = Cell::new(None);
        let slices = Slices::new(&fx.clock, |n| match n {
            3 => by_then.set(Some((absent(&fx, &cc()), absent(&fx, &fake())))),
            n if n > 3 => cancel.request(libc::SIGTERM),
            _ => {}
        });
        let events = Emitted::new(&fx.clock);
        let calls = Calls::default();
        assert_eq!(host(&engine, &events, &slices, &calls), HostedEnd::Signal);
        assert_eq!(by_then.get(), Some((true, true)));
        let urls = http.usage_urls();
        assert!(!urls.is_empty(), "Claude Code's tick had asked for usage");
        assert!(
            urls.iter().all(|u| u == CC_USAGE),
            "FakeAgent, overdue behind it, asked for nothing: {urls:?}"
        );
    }

    #[test]
    fn a_daemon_live_at_start_keeps_every_provider_job_from_running() {
        let fx = Fx::new();
        read_two(&fx, 95.0);
        daemon_up(&fx.env);
        let engine = fx.tui_engine();
        let cancel = engine.cancel().clone();
        let slices = Slices::new(&fx.clock, |n| {
            if n >= 3 {
                cancel.request(libc::SIGTERM);
            }
        });
        let events = Emitted::new(&fx.clock);
        let calls = Calls::default();
        assert_eq!(host(&engine, &events, &slices, &calls), HostedEnd::Signal);
        assert!(absent(&fx, &cc()) && absent(&fx, &fake()));
        assert!(events.modes(&cc()).is_empty() && events.modes(&fake()).is_empty());
        assert_eq!(last_lines(&calls), Some(Vec::new()));
    }

    #[test]
    fn a_settings_reload_as_a_daemon_appears_takes_no_lock_and_records_nothing() {
        // The provider is paused, so it holds no lock. A daemon comes up and the setting is
        // turned back on in the same slice: the reload must neither take the lock nor store
        // an `autoswitch-resumed`.
        let fx = Fx::new();
        read_two(&fx, 50.0);
        fx.settings("[provider.claude-code.daemon]\nautoswitch = false\n", 1);
        let engine = fx.tui_engine();
        let cancel = engine.cancel().clone();
        let slices = Slices::new(&fx.clock, |n| match n {
            1 => {
                assert!(absent(&fx, &cc()), "paused: no lock");
                daemon_up(&fx.env);
                fx.settings("[provider.claude-code.daemon]\nautoswitch = true\n", 2);
            }
            n if n >= 3 => cancel.request(libc::SIGTERM),
            _ => {}
        });
        let events = Emitted::new(&fx.clock);
        let calls = Calls::default();
        assert_eq!(host(&engine, &events, &slices, &calls), HostedEnd::Signal);
        assert!(absent(&fx, &cc()));
        let store = fx.engine.store().unwrap();
        let latest = |kind| store.latest_event(&cc(), &[kind], None, None).unwrap();
        assert!(latest(EventKind::AutoswitchPaused).is_some());
        assert!(latest(EventKind::AutoswitchResumed).is_none());
        assert_eq!(last_lines(&calls), Some(Vec::new()));
    }

    #[test]
    fn a_newer_schema_ends_the_run_with_schema_and_frees_the_lock() {
        let fx = Fx::new();
        read_two(&fx, 50.0);
        let migrate = |fx: &Fx| {
            let conn = rusqlite::Connection::open(fx.engine.store_file()).unwrap();
            conn.execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION + 1))
                .unwrap();
        };
        // Midway: after the watch's period.
        let engine = fx.tui_engine();
        let cancel = engine.cancel().clone();
        let held = Cell::new(None);
        let slices = Slices::new(&fx.clock, |n| match n {
            10 => {
                held.set(Some(holder(&fx, &cc())));
                migrate(&fx);
            }
            n if n > 100 => cancel.request(libc::SIGTERM),
            _ => {}
        });
        let events = Emitted::new(&fx.clock);
        let calls = Calls::default();
        assert_eq!(host(&engine, &events, &slices, &calls), HostedEnd::Schema);
        assert_eq!(held.get(), Some(Some(Some(HolderKind::Tui))));
        assert!(absent(&fx, &cc()));
        // At start: before any job runs.
        let engine = fx.tui_engine();
        let events = Emitted::new(&fx.clock);
        let calls = Calls::default();
        let slices = Slices::none(&fx.clock);
        assert_eq!(host(&engine, &events, &slices, &calls), HostedEnd::Schema);
        assert!(absent(&fx, &cc()));
        assert!(calls.borrow().is_empty() && events.events().is_empty());
    }

    #[test]
    fn a_settings_change_reaches_the_engine_and_the_jobs() {
        let fx = Fx::new();
        read_two(&fx, 50.0);
        let engine = fx.tui_engine();
        let cancel = engine.cancel().clone();
        let seen = RefCell::new(Vec::new());
        let paused =
            "[notify]\nenabled = false\n\n[provider.claude-code.daemon]\nautoswitch = false\n";
        let slices = Slices::new(&fx.clock, |n| match n {
            2 => fx.settings("[notify]\nenabled = false\n", 5),
            4 => {
                seen.borrow_mut()
                    .push(("notify off", !engine.settings().notify_enabled));
                seen.borrow_mut()
                    .push(("held", holder(&fx, &cc()) == Some(Some(HolderKind::Tui))));
                fx.settings(paused, 6);
            }
            6 => {
                seen.borrow_mut().push(("paused", absent(&fx, &cc())));
                fx.settings("[notify]\nenabled = false\n", 7);
            }
            8 => {
                seen.borrow_mut()
                    .push(("resumed", holder(&fx, &cc()) == Some(Some(HolderKind::Tui))));
                cancel.request(libc::SIGTERM);
            }
            _ => {}
        });
        let events = Emitted::new(&fx.clock);
        let calls = Calls::default();
        assert_eq!(host(&engine, &events, &slices, &calls), HostedEnd::Signal);
        assert_eq!(
            *seen.borrow(),
            [
                ("notify off", true),
                ("held", true),
                ("paused", true),
                ("resumed", true)
            ]
        );
        let store = fx.engine.store().unwrap();
        for kind in [EventKind::AutoswitchPaused, EventKind::AutoswitchResumed] {
            let stored = store
                .latest_event(&cc(), &[kind], None, None)
                .unwrap()
                .unwrap_or_else(|| panic!("{kind:?} is stored"));
            assert_eq!(stored.source, EventSource::Auto, "{kind:?}");
        }
    }

    #[test]
    fn inside_a_run_shell_only_the_schema_check_runs_and_a_newer_store_ends_it() {
        let fx = run_shell_fx();
        fx.engine.store().unwrap();
        let off = Cell::new(0);
        let events = Emitted::new(&fx.clock);
        let calls = Calls::default();
        let held = Cell::new(false);
        let slices = Slices::new(&fx.clock, |n| {
            assert_eq!(off.get(), 1, "off is told before the first sleep");
            held.set(held.get() || !absent(&fx, &cc()));
            match n {
                3 => migrate_past(&fx),
                n if n > 200 => panic!("the schema check never ended the run"),
                _ => {}
            }
        });
        let end = host_counting_off(&fx.engine, &events, &slices, &calls, &off);
        assert_eq!(end, HostedEnd::Schema);
        assert_eq!(off.get(), 1);
        assert!(!held.get(), "no engine lock was taken");
        assert!(absent(&fx, &cc()));
        assert!(calls.borrow().is_empty() && events.events().is_empty());
        let taken = slices.taken.borrow().len();
        assert!(
            (4..=4 + 60).contains(&taken),
            "ended within the watch's period of the migration: {taken} slices"
        );
    }

    #[test]
    fn inside_a_run_shell_a_newer_store_at_start_ends_at_once_and_a_signal_ends_it() {
        let fx = run_shell_fx();
        fx.engine.store().unwrap();
        migrate_past(&fx);
        let off = Cell::new(0);
        let events = Emitted::new(&fx.clock);
        let calls = Calls::default();
        let slices = Slices::none(&fx.clock);
        let end = host_counting_off(&fx.engine, &events, &slices, &calls, &off);
        assert_eq!((end, off.get()), (HostedEnd::Schema, 1));

        let fx = run_shell_fx();
        let off = Cell::new(0);
        let slices = Slices::new(&fx.clock, |n| {
            if n == 2 {
                fx.engine.cancel().request(libc::SIGTERM);
            }
        });
        let end = host_counting_off(&fx.engine, &events, &slices, &calls, &off);
        assert_eq!((end, off.get()), (HostedEnd::Signal, 1));
    }

    #[test]
    fn two_runs_on_one_home_the_second_waits_and_takes_over() {
        let fx = Fx::new();
        read_two(&fx, 50.0);
        let first = fx.tui_engine();
        let cfg = auto_config(
            &tagteam_engine::settings::Settings::default(),
            &AutoFlags::default(),
            Some("7d"),
        );
        let held: RefCell<Option<AutoEngine<'_>>> =
            RefCell::new(Some(first.auto(&cc(), cfg, false).unwrap().unwrap()));
        let engine = fx.tui_engine();
        let cancel = engine.cancel().clone();
        let calls = Calls::default();
        let waiting = RefCell::new(None);
        let took = Cell::new(None);
        let slices = Slices::new(&fx.clock, |n| match n {
            1 => {
                let lines = last_lines(&calls).unwrap();
                *waiting.borrow_mut() = Some(line(&lines, &cc()));
                held.borrow_mut().take();
            }
            62 => {
                took.set(Some(holder(&fx, &cc())));
                cancel.request(libc::SIGTERM);
            }
            _ => {}
        });
        let events = Emitted::new(&fx.clock);
        assert_eq!(host(&engine, &events, &slices, &calls), HostedEnd::Signal);
        let waiting = waiting.borrow().clone().unwrap();
        assert_eq!(waiting.mode, Mode::Waiting);
        assert_eq!(waiting.waiting_for, Some(WaitingFor::EngineLock));
        assert_eq!(waiting.holder_pid, Some(std::process::id()));
        assert_eq!(
            took.get(),
            Some(Some(Some(HolderKind::Tui))),
            "once the first let go, the second took the lock within 60 s"
        );
    }

    #[test]
    fn a_signal_ends_the_run_and_frees_every_lock() {
        let fx = Fx::new();
        read_two(&fx, 50.0);
        let engine = fx.tui_engine();
        let held = Cell::new(None);
        let slices = Slices::new(&fx.clock, |n| {
            if n == 2 {
                held.set(Some(holder(&fx, &cc())));
                engine.cancel().request(15);
            }
        });
        let events = Emitted::new(&fx.clock);
        let calls = Calls::default();
        assert_eq!(host(&engine, &events, &slices, &calls), HostedEnd::Signal);
        assert_eq!(held.get(), Some(Some(Some(HolderKind::Tui))));
        assert!(absent(&fx, &cc()));
    }
}
