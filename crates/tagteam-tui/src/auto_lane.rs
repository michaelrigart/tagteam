//! The auto lane (TUI auto-switch spec §3.1): a thread that runs the daemon's jobs while no
//! daemon runs, and says what they do as the daemon's protocol encodes it, so the state machine
//! reads it as it reads a socket's. Its engine is built inside the thread by the binary's
//! factory, under a cancel token of the lane's own: Ctrl-Z and the shutdown set it, and the
//! scheduler wakes at once from a sleep and returns once the job running, if any, has finished,
//! releasing every lock.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tagteam_core::ProviderId;
use tagteam_daemon::events::{DaemonEvent, DaemonSink};
use tagteam_daemon::hosted::{HostedEnd, HostedOptions, run_hosted};
use tagteam_daemon::protocol::{Status, daemon_event_json, parse_event, status_json};
use tagteam_daemon::scheduler::{Sleeper, uniform_jitter};
use tagteam_daemon::status::StatusSnapshot;
use tagteam_engine::Engine;
use tagteam_provider::Cancel;
use tagteam_provider::keychain::ReadCounts;

use crate::msg::{LaneReport, Msg};
use crate::source::SourceError;

/// What the lane runs the daemon's jobs on: the engine, and the Keychain's read counts that
/// its jobs' backoff compares (D4.1).
pub struct LaneEngine {
    pub engine: Engine,
    pub keychain_counts: Vec<Arc<ReadCounts>>,
}

/// Builds the lane's engine inside its thread, under the token given (S§3.1).
pub type AutoFactory = Arc<dyn Fn(Cancel) -> Result<LaneEngine, SourceError> + Send + Sync>;

/// The lane's sleep: it ends early when the lane's thread is unparked, which `suspend`, `close`
/// and `join` do after they set the token. The scheduler looks at its token and the clock after
/// every sleep, so a wake-up for any other reason is harmless.
struct ParkSleeper;

impl Sleeper for ParkSleeper {
    fn sleep(&self, d: Duration) {
        thread::park_timeout(d);
    }
}

/// The lane's current run: the token that ends it and its thread.
struct Run {
    cancel: Cancel,
    handle: Option<JoinHandle<()>>,
    /// An earlier run panicked, and `suspend` or `resume` joined it: `join` still says so.
    died: bool,
    /// The shutdown began: no run starts again.
    closed: bool,
}

pub(crate) struct AutoLane {
    factory: AutoFactory,
    provider: Option<ProviderId>,
    tx: Sender<Msg>,
    upgraded: Arc<AtomicBool>,
    run: Mutex<Run>,
}

impl AutoLane {
    /// Starts the first run. `upgraded` is `Lanes.upgraded`: the lane sets it when it finds
    /// the store newer than this build.
    pub(crate) fn spawn(
        factory: AutoFactory,
        provider: Option<ProviderId>,
        tx: &Sender<Msg>,
        upgraded: Arc<AtomicBool>,
    ) -> AutoLane {
        let lane = AutoLane {
            factory,
            provider,
            tx: tx.clone(),
            upgraded,
            run: Mutex::new(Run {
                cancel: Cancel::new(),
                handle: None,
                died: false,
                closed: false,
            }),
        };
        {
            let mut run = lane.lock();
            lane.start(&mut run);
        }
        lane
    }

    /// The run, whatever a panic left it as: it holds a token, a handle and two flags.
    fn lock(&self) -> MutexGuard<'_, Run> {
        self.run.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A run under a fresh token, on a thread of its own.
    fn start(&self, run: &mut Run) {
        run.cancel = Cancel::new();
        let (factory, provider) = (Arc::clone(&self.factory), self.provider.clone());
        let (tx, upgraded, cancel) = (
            self.tx.clone(),
            Arc::clone(&self.upgraded),
            run.cancel.clone(),
        );
        let handle = thread::Builder::new()
            .name("tagteam-tui-auto".into())
            .spawn(move || serve(&factory, provider, &tx, &upgraded, cancel))
            .expect("the system could not start a thread for the TUI's auto lane");
        run.handle = Some(handle);
    }

    /// A new run with a fresh token, unless one runs, a run ended `Schema` (S§3.7), or the
    /// shutdown began.
    pub(crate) fn resume(&self) {
        let mut run = self.lock();
        if run.closed || self.upgraded.load(Ordering::SeqCst) {
            return;
        }
        if run.handle.as_ref().is_some_and(|h| !h.is_finished()) {
            return;
        }
        if let Some(ended) = run.handle.take() {
            run.died |= ended.join().is_err();
        }
        self.start(&mut run);
    }

    /// Sets the run's token and joins its thread (Ctrl-Z): every lock it held is free on return.
    pub(crate) fn suspend(&self) {
        let mut run = self.lock();
        run.cancel.request(libc::SIGINT);
        if let Some(handle) = run.handle.take() {
            handle.thread().unpark();
            run.died |= handle.join().is_err();
        }
    }

    /// Sets the run's token (the shutdown's step 2); no run starts again.
    pub(crate) fn close(&self) {
        let mut run = self.lock();
        run.closed = true;
        run.cancel.request(libc::SIGINT);
        if let Some(handle) = &run.handle {
            handle.thread().unpark();
        }
    }

    /// Joins the run; `Err` when it, or one joined before it, panicked.
    pub(crate) fn join(self) -> thread::Result<()> {
        let run = self
            .run
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        run.cancel.request(libc::SIGINT);
        let ended = run.handle.map_or(Ok(()), |handle| {
            handle.thread().unpark();
            handle.join()
        });
        if run.died {
            return Err(Box::new("the auto lane panicked"));
        }
        ended
    }
}

/// Tells the loop the lane's thread unwound from a panic (§10.4).
struct Death(Sender<Msg>);

impl Drop for Death {
    fn drop(&mut self) {
        if thread::panicking() {
            let _ = self.0.send(Msg::LaneDied { lane: "auto" });
        }
    }
}

/// One run: the engine, then the daemon's jobs on it until the token ends them.
fn serve(
    factory: &AutoFactory,
    provider: Option<ProviderId>,
    tx: &Sender<Msg>,
    upgraded: &AtomicBool,
    token: Cancel,
) {
    let _death = Death(tx.clone());
    let report = |report: LaneReport| {
        let _ = tx.send(Msg::Lane(report));
    };
    let LaneEngine {
        engine,
        keychain_counts,
    } = match factory(token) {
        Ok(built) => built,
        Err(_) => return report(LaneReport::Off),
    };
    // Ruling 13: it runs every provider; `--provider` only chooses whose tables the settings
    // job reads, as for the other lanes.
    let opts = HostedOptions {
        settings_provider: provider.unwrap_or_else(|| engine.default_provider().clone()),
        keychain_counts,
    };
    let home = engine.env().home.clone();
    let now = || engine.now_s();
    let sink = LaneSink::new(tx, &now);
    let on_status = |snapshot: &StatusSnapshot| send_status(tx, snapshot, &home);
    let end = run_hosted(
        &engine,
        opts,
        &sink,
        &on_status,
        &|| report(LaneReport::Off),
        &ParkSleeper,
        &mut uniform_jitter,
    );
    match end {
        HostedEnd::Signal => {}
        HostedEnd::Off => report(LaneReport::Off),
        HostedEnd::Schema => {
            // Before the report, so that a writer taken after it is refused (Ruling 5).
            upgraded.store(true, Ordering::SeqCst);
            report(LaneReport::Upgraded);
        }
    }
}

/// The board as `status` encodes it (Ruling 3), so the header reads what a socket would give.
fn send_status(tx: &Sender<Msg>, snapshot: &StatusSnapshot, home: &Path) {
    if let Ok(status) = serde_json::from_value::<Status>(status_json(snapshot, home)) {
        let _ = tx.send(Msg::Lane(LaneReport::Status(status)));
    }
}

/// Where the jobs' events go: each as `subscribe` would encode it, then read as a socket's
/// client reads it (Ruling 3).
struct LaneSink<'a> {
    tx: &'a Sender<Msg>,
    /// The engine's clock, in seconds: an event carries the time it happened.
    now_s: &'a dyn Fn() -> i64,
}

impl<'a> LaneSink<'a> {
    fn new(tx: &'a Sender<Msg>, now_s: &'a dyn Fn() -> i64) -> Self {
        LaneSink { tx, now_s }
    }
}

impl DaemonSink for LaneSink<'_> {
    fn emit(&self, e: &DaemonEvent) {
        if let Some(event) = parse_event(&daemon_event_json(e, (self.now_s)())) {
            let _ = self.tx.send(Msg::Lane(LaneReport::Event(event)));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use tagteam_core::autoswitch::Trigger;
    use tagteam_daemon::events::Mode;
    use tagteam_daemon::protocol::Event;
    use tagteam_daemon::status::{DaemonInfo, NotifyInfo, ProviderStatus};
    use tagteam_engine::auto::AutoEvent;
    use tagteam_provider::FakeClock;

    use super::*;
    use crate::testutil::LaneHome;

    fn fake() -> ProviderId {
        ProviderId::new("fake-agent")
    }

    fn board() -> StatusSnapshot {
        StatusSnapshot {
            daemon: DaemonInfo {
                pid: 77,
                version: "20261111.0.0".into(),
                started_at: 1_790_000_000,
                socket: None,
                log: "/home/x/.local/state/tagteam/tagteam.log".into(),
            },
            notifications: NotifyInfo {
                enabled: false,
                backend: None,
                events: vec![],
            },
            providers: vec![ProviderStatus {
                provider: fake(),
                mode: Mode::Auto,
                autoswitch: true,
                waiting_for: None,
                holder_pid: None,
                backoff: None,
                last_tick_at: Some(1_790_000_005),
                next_tick_at: Some(1_790_000_065),
                last_outcome: None,
                last_switch: None,
            }],
            jobs: vec![],
            subscribers: 0,
        }
    }

    #[test]
    fn a_lane_in_a_run_shell_says_off_and_then_upgraded_when_the_store_turns_newer() {
        let home = Arc::new(LaneHome::new());
        let clock = Arc::new(FakeClock::new(1_790_000_000_000));
        let (tx, rx) = mpsc::channel();
        let upgraded = Arc::new(AtomicBool::new(false));
        let lane = AutoLane::spawn(
            home.in_a_run_shell(Arc::clone(&clock)),
            None,
            &tx,
            Arc::clone(&upgraded),
        );
        let first = rx.recv_timeout(Duration::from_secs(5));
        assert!(
            matches!(first, Ok(Msg::Lane(LaneReport::Off))),
            "the lane did not say off first"
        );
        assert!(!upgraded.load(Ordering::SeqCst));
        // A newer tagteam migrates the store, and the watch's period passes on the lane's clock.
        home.newer_store();
        clock.advance_ms(61_000);
        let second = rx.recv_timeout(Duration::from_secs(5));
        assert!(
            matches!(second, Ok(Msg::Lane(LaneReport::Upgraded))),
            "the lane did not say upgraded after off"
        );
        assert!(upgraded.load(Ordering::SeqCst));
        assert!(lane.join().is_ok());
    }

    fn switch() -> DaemonEvent {
        DaemonEvent::Auto(AutoEvent::Switch {
            provider: fake(),
            trigger: Trigger::Proactive,
            from: 2,
            to: 1,
            warnings: vec![],
            dry_run: false,
        })
    }

    #[test]
    fn the_lane_s_board_and_events_reach_the_ui_as_a_socket_gives_them() {
        let (tx, rx) = mpsc::channel();
        let home = Path::new("/home/x");
        let snapshot = board();
        send_status(&tx, &snapshot, home);
        let Ok(Msg::Lane(LaneReport::Status(status))) = rx.recv_timeout(Duration::from_secs(5))
        else {
            panic!("the board did not arrive as a lane's status");
        };
        let as_a_socket = serde_json::from_value::<Status>(status_json(&snapshot, home)).unwrap();
        assert_eq!(status, as_a_socket);
        assert_eq!(status.providers.len(), 1);
        assert_eq!(status.providers[0].provider, "fake-agent");
        assert_eq!(status.providers[0].mode.known(), Some(Mode::Auto));
        assert!(status.providers[0].autoswitch);

        let event = switch();
        LaneSink::new(&tx, &|| 1_790_000_100).emit(&event);
        let Ok(Msg::Lane(LaneReport::Event(got))) = rx.recv_timeout(Duration::from_secs(5)) else {
            panic!("the event did not arrive as a lane's event");
        };
        let as_a_socket = parse_event(&daemon_event_json(&event, 1_790_000_100)).unwrap();
        assert_eq!(got, as_a_socket);
        assert!(
            matches!(&got, Event::Auto { kind, provider, .. } if kind == "switch" && provider == "fake-agent"),
            "{got:?}"
        );
    }
}
