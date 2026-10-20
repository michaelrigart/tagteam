//! Worker lanes (spec §4.3): one thread each, serving its requests in order. A lane builds its
//! own state inside its thread (R4), merges a queued request with a newer one of the same kind,
//! and stops taking requests once it is closed or has died.

use std::collections::VecDeque;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tagteam_core::ProviderId;
use tagteam_daemon::protocol::Status;
use tagteam_provider::Cancel;

use crate::ask::{Answers, ChannelAsk};
use crate::host::Host;
use crate::live::{LIVE_QUEUE, LiveQueue};
use crate::msg::{ActionsReq, DaemonReq, IndexReq, Msg, ReportReq, ViewsReq};
use crate::runtime::WAKE;
use crate::source::{Source, SourceFactory};

/// Whether a queued request is replaced by a newer one (spec §4.3, "Coalescing").
pub trait Coalesce {
    fn same_as(&self, other: &Self) -> bool;
}

/// The requests not yet started, and whether the lane still takes any.
struct Queue<Req> {
    items: VecDeque<Req>,
    closed: bool,
}

struct Shared<Req> {
    queue: Mutex<Queue<Req>>,
    ready: Condvar,
}

impl<Req> Shared<Req> {
    /// The queue, whatever a panic left it as: it holds only requests and a flag.
    fn lock(&self) -> MutexGuard<'_, Queue<Req>> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn close(&self) {
        let mut q = self.lock();
        q.closed = true;
        q.items.clear();
        drop(q);
        self.ready.notify_all();
    }

    /// The next request, waiting for one; `None` once the lane is closed.
    fn next(&self) -> Option<Req> {
        let mut q = self.lock();
        loop {
            if q.closed {
                return None;
            }
            if let Some(req) = q.items.pop_front() {
                return Some(req);
            }
            q = self.ready.wait(q).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// Runs `on_death` if the lane's thread unwinds, after closing the lane so that nothing more
/// is queued for a thread that will never take it.
struct Death<Req, F: FnOnce()> {
    shared: Arc<Shared<Req>>,
    on_death: Option<F>,
}

impl<Req, F: FnOnce()> Drop for Death<Req, F> {
    fn drop(&mut self) {
        if thread::panicking() {
            self.shared.close();
            if let Some(f) = self.on_death.take() {
                f();
            }
        }
    }
}

pub struct Lane<Req: Coalesce + Send + 'static> {
    shared: Arc<Shared<Req>>,
    handle: Option<JoinHandle<()>>,
}

impl<Req: Coalesce + Send + 'static> Lane<Req> {
    /// Spawns the lane's thread. `init` runs first, inside the thread (R4). `work` runs each
    /// request in order. `on_death` runs once if the thread unwinds from a panic.
    pub fn spawn<S: 'static>(
        name: &'static str,
        init: impl FnOnce() -> S + Send + 'static,
        mut work: impl FnMut(&mut S, Req) + Send + 'static,
        on_death: impl FnOnce() + Send + 'static,
    ) -> Lane<Req> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                items: VecDeque::new(),
                closed: false,
            }),
            ready: Condvar::new(),
        });
        let inner = Arc::clone(&shared);
        let handle = thread::Builder::new()
            .name(format!("tagteam-tui-{name}"))
            .spawn(move || {
                let _death = Death {
                    shared: Arc::clone(&inner),
                    on_death: Some(on_death),
                };
                let mut state = init();
                while let Some(req) = inner.next() {
                    work(&mut state, req);
                }
            })
            .expect("the system could not start a thread for a TUI lane");
        Lane {
            shared,
            handle: Some(handle),
        }
    }

    /// Queues `req`. A queued request that `same_as` it is replaced in place (the newest wins).
    /// False once the lane is closed.
    pub fn submit(&self, req: Req) -> bool {
        let mut q = self.shared.lock();
        if q.closed {
            return false;
        }
        match q.items.iter_mut().find(|queued| queued.same_as(&req)) {
            Some(queued) => *queued = req,
            None => q.items.push_back(req),
        }
        drop(q);
        self.shared.ready.notify_one();
        true
    }

    /// Drops every queued request and wakes the thread, which returns after its current one.
    pub fn close(&self) {
        self.shared.close();
    }

    /// Joins the thread; Err when it panicked. It closes the lane first, so an idle lane
    /// returns at once and a busy one after its current request.
    pub fn join(mut self) -> std::thread::Result<()> {
        self.close();
        match self.handle.take() {
            Some(handle) => handle.join(),
            None => Ok(()),
        }
    }
}

impl<Req: Coalesce + Send + 'static> Drop for Lane<Req> {
    /// A lane dropped without `join` is closed, so its thread ends after its current request.
    fn drop(&mut self) {
        self.close();
    }
}

/// The index lane passes on a refresh's progress at most this often: the loop's own pace.
const PROGRESS_EVERY: Duration = WAKE;

/// What fetches the daemon's `status` on the daemon lane: `live::fetch_status` on its socket.
pub(crate) type StatusFetch = Arc<dyn Fn() -> Result<Status, String> + Send + Sync>;

/// The lanes (§4.3). The engine lanes: views (`accounts`, `history`, store-served), report
/// (the token report, which reads the index only), index (the token index's catch-up) and
/// actions (the collector and every `Host` action, one at a time). And the daemon lane, which
/// fetches the daemon's `status` and has no engine.
pub(crate) struct Lanes {
    pub(crate) views: Lane<ViewsReq>,
    pub(crate) report: Lane<ReportReq>,
    pub(crate) index: Lane<IndexReq>,
    pub(crate) actions: Lane<ActionsReq>,
    /// When the TUI follows a daemon.
    pub(crate) daemon: Option<Lane<DaemonReq>>,
    /// The threads that are let go rather than joined, the daemon lane and the live thread,
    /// that died: named in the report as a joined lane that panicked is (§10.4).
    strays: Arc<Mutex<Vec<&'static str>>>,
    /// What the live thread passes on, bounded; a `Msg::LiveReady` says it has something
    /// (C-R17).
    pub(crate) live: Arc<LiveQueue>,
}

impl Lanes {
    /// Spawns them. Each engine lane builds its own `Source` in its own thread (R4), and a lane
    /// that panics says so on `tx` (§4.3, §10.4). The daemon lane is spawned when `status` is
    /// given.
    pub(crate) fn spawn(
        provider: Option<ProviderId>,
        sources: &SourceFactory,
        host: &Arc<dyn Host>,
        tx: &Sender<Msg>,
        shutdown: &Cancel,
        answers: &Answers,
        status: Option<StatusFetch>,
    ) -> Lanes {
        let strays = Arc::default();
        let views = {
            let (make, out, died) = (Arc::clone(sources), tx.clone(), tx.clone());
            Lane::spawn(
                "views",
                move || make(),
                move |source: &mut Box<dyn Source>, req: ViewsReq| {
                    let msg = match req {
                        ViewsReq::Accounts { r#gen } => Msg::Accounts {
                            r#gen,
                            result: source.accounts(provider.as_ref()),
                        },
                        ViewsReq::History {
                            r#gen,
                            account,
                            since_s,
                        } => Msg::History {
                            r#gen,
                            result: source.history(&account, since_s),
                        },
                    };
                    let _ = out.send(msg);
                },
                move || {
                    let _ = died.send(Msg::LaneDied { lane: "views" });
                },
            )
        };
        let report = {
            let (make, out, died) = (Arc::clone(sources), tx.clone(), tx.clone());
            Lane::spawn(
                "report",
                move || make(),
                move |source: &mut Box<dyn Source>, req: ReportReq| {
                    let msg = match req {
                        ReportReq::Report { r#gen, query } => Msg::TokenReport {
                            r#gen,
                            result: source.token_report(&query),
                        },
                        ReportReq::IndexState { r#gen, provider } => Msg::IndexState {
                            r#gen,
                            result: source.token_index_state(&provider),
                        },
                        ReportReq::Inventory { r#gen, query } => Msg::Inventory {
                            r#gen,
                            result: source.token_report(&query),
                        },
                    };
                    let _ = out.send(msg);
                },
                move || {
                    let _ = died.send(Msg::LaneDied { lane: "report" });
                },
            )
        };
        let index = {
            let (make, out, died) = (Arc::clone(sources), tx.clone(), tx.clone());
            Lane::spawn(
                "index",
                move || make(),
                move |source: &mut Box<dyn Source>, req: IndexReq| {
                    let IndexReq::Refresh { provider } = req;
                    // The engine reports progress per chunk; the loop is told at its own pace.
                    let mut told: Option<Instant> = None;
                    let result = source.refresh_tokens(&provider, &mut |progress| {
                        if told.is_none_or(|at| at.elapsed() >= PROGRESS_EVERY) {
                            told = Some(Instant::now());
                            let _ = out.send(Msg::IndexProgress(*progress));
                        }
                    });
                    let _ = out.send(Msg::Refreshed { result });
                },
                move || {
                    let _ = died.send(Msg::LaneDied { lane: "index" });
                },
            )
        };
        let actions = {
            let (make, out, died) = (Arc::clone(sources), tx.clone(), tx.clone());
            let (host, shutdown, answers) = (Arc::clone(host), shutdown.clone(), answers.clone());
            Lane::spawn(
                "actions",
                move || make(),
                move |source: &mut Box<dyn Source>, req: ActionsReq| {
                    let msg = match req {
                        ActionsReq::Collect { r#gen, accounts } => Msg::Collected {
                            r#gen,
                            result: source.collect_due(&accounts),
                        },
                        ActionsReq::Act { action } => {
                            // Its prompts become dialogs, declined once the shutdown begins
                            // (§7.2, §10.2 step 3).
                            let mut ask =
                                ChannelAsk::new(out.clone(), shutdown.clone(), answers.clone());
                            let outcome = host.act(&action, &mut ask);
                            Msg::ActionDone { action, outcome }
                        }
                    };
                    let _ = out.send(msg);
                },
                move || {
                    let _ = died.send(Msg::LaneDied { lane: "actions" });
                },
            )
        };
        let daemon = status.map(|fetch| {
            let out = tx.clone();
            let died = stray_death(&strays, "daemon", tx);
            Lane::spawn(
                "daemon",
                || (),
                move |_: &mut (), req: DaemonReq| {
                    let DaemonReq::Status { link } = req;
                    let result = fetch();
                    let _ = out.send(Msg::DaemonStatus { link, result });
                },
                died,
            )
        });
        let wake = tx.clone();
        let live = Arc::new(LiveQueue::new(LIVE_QUEUE, move || {
            wake.send(Msg::LiveReady).is_ok()
        }));
        Lanes {
            views,
            report,
            index,
            actions,
            daemon,
            strays,
            live,
        }
    }

    /// What a thread that is let go rather than joined runs if it panics: it is named in the
    /// report, and the loop is told, which ends the TUI (§10.4).
    pub(crate) fn stray_death(
        &self,
        name: &'static str,
        tx: &Sender<Msg>,
    ) -> impl FnOnce() + Send + 'static {
        stray_death(&self.strays, name, tx)
    }

    /// §10.2 step 2: nothing more is taken, what is queued is dropped, and an idle lane wakes
    /// and returns. A refresh the index lane runs stops at its next chunk: its engine takes
    /// the shutdown token, which step 1 set (§9).
    pub(crate) fn close(&self) {
        self.views.close();
        self.report.close();
        self.index.close();
        self.actions.close();
        if let Some(daemon) = &self.daemon {
            daemon.close();
        }
        // The loop stops listening to the live thread: its next push says `Gone`, whatever the
        // queue holds, and it returns.
        self.live.close();
    }

    /// §10.2 step 5: joins every engine lane, lets the daemon lane go (§4.3: it touches no
    /// store, and a `status` it waits for may take 2 s), and names the lanes that panicked.
    pub(crate) fn join(self) -> Vec<&'static str> {
        let mut died = Vec::new();
        if self.views.join().is_err() {
            died.push("views");
        }
        if self.report.join().is_err() {
            died.push("report");
        }
        if self.index.join().is_err() {
            died.push("index");
        }
        if self.actions.join().is_err() {
            died.push("actions");
        }
        drop(self.daemon);
        died.extend(lock(&self.strays).iter());
        died
    }
}

/// `stray_death` over the list it adds to.
fn stray_death(
    strays: &Arc<Mutex<Vec<&'static str>>>,
    name: &'static str,
    tx: &Sender<Msg>,
) -> impl FnOnce() + Send + 'static {
    let (strays, tx) = (Arc::clone(strays), tx.clone());
    move || {
        lock(&strays).push(name);
        let _ = tx.send(Msg::LaneDied { lane: name });
    }
}

/// The list of dead strays, whatever a panic left it as: it only holds names.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::{Duration, Instant};

    use super::*;

    const WAIT: Duration = Duration::from_secs(5);

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Req {
        /// Never coalesces.
        Keep(u32),
        /// Coalesces with any other `Merge`.
        Merge(u32),
        /// Waits for the gate before it is reported.
        Gate(u32),
        /// Panics.
        Boom,
    }

    impl Coalesce for Req {
        fn same_as(&self, other: &Self) -> bool {
            matches!((self, other), (Req::Merge(_), Req::Merge(_)))
        }
    }

    /// A lane that reports every request it ran on `done`; a `Gate` waits on `gate` first.
    fn lane(name: &'static str) -> (Lane<Req>, Sender<()>, Receiver<Req>) {
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) = mpsc::channel::<Req>();
        let lane = Lane::spawn(
            name,
            move || (gate_rx, done_tx),
            |(gate, done): &mut (Receiver<()>, Sender<Req>), req: Req| {
                match req {
                    Req::Gate(_) => gate.recv_timeout(WAIT).expect("the test opens the gate"),
                    Req::Boom => panic!("the lane's work panicked"),
                    Req::Keep(_) | Req::Merge(_) => {}
                }
                let _ = done.send(req);
            },
            || {},
        );
        (lane, gate_tx, done_rx)
    }

    fn take(done: &Receiver<Req>, n: usize) -> Vec<Req> {
        (0..n)
            .map(|_| done.recv_timeout(WAIT).expect("the lane ran the request"))
            .collect()
    }

    #[test]
    fn requests_run_in_the_order_they_were_submitted() {
        let (lane, _gate, done) = lane("order");
        for n in [1, 2, 3] {
            assert!(lane.submit(Req::Keep(n)));
        }
        assert_eq!(take(&done, 3), [Req::Keep(1), Req::Keep(2), Req::Keep(3)]);
        lane.join().unwrap();
    }

    #[test]
    fn a_newer_request_replaces_a_queued_one_in_place_and_others_are_left_alone() {
        let (lane, gate, done) = lane("coalesce");
        assert!(lane.submit(Req::Gate(0)));
        assert!(lane.submit(Req::Merge(1)));
        assert!(lane.submit(Req::Keep(2)));
        assert!(lane.submit(Req::Merge(3)));
        assert!(lane.submit(Req::Keep(2)));
        gate.send(()).unwrap();
        assert_eq!(
            take(&done, 4),
            [Req::Gate(0), Req::Merge(3), Req::Keep(2), Req::Keep(2)]
        );
        lane.join().unwrap();
    }

    #[test]
    fn close_drops_what_is_queued_and_lets_the_current_request_finish() {
        let (lane, gate, done) = lane("close");
        assert!(lane.submit(Req::Gate(0)));
        // The gate request is taken before the others are queued behind it.
        let deadline = Instant::now() + WAIT;
        while !lane.shared.lock().items.is_empty() {
            assert!(
                Instant::now() < deadline,
                "the lane never took its first request"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert!(lane.submit(Req::Keep(1)));
        assert!(lane.submit(Req::Keep(2)));
        lane.close();
        gate.send(()).unwrap();
        lane.join().unwrap();
        assert_eq!(done.try_iter().collect::<Vec<_>>(), [Req::Gate(0)]);
    }

    #[test]
    fn an_idle_lane_joins_once_closed() {
        let (lane, _gate, _done) = lane("idle");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            lane.close();
            let _ = tx.send(lane.join().is_ok());
        });
        assert_eq!(rx.recv_timeout(WAIT), Ok(true));
    }

    #[test]
    fn a_closed_lane_takes_no_request() {
        let (lane, _gate, _done) = lane("closed");
        lane.close();
        assert!(!lane.submit(Req::Keep(1)));
        lane.join().unwrap();
    }

    #[test]
    fn init_runs_on_the_lanes_own_named_thread() {
        let (tx, rx) = mpsc::channel();
        let lane: Lane<Req> = Lane::spawn(
            "views",
            move || {
                let me = thread::current();
                tx.send((me.name().map(str::to_owned), me.id())).unwrap();
            },
            |_: &mut (), _: Req| {},
            || {},
        );
        let (name, id) = rx.recv_timeout(WAIT).unwrap();
        assert_eq!(name.as_deref(), Some("tagteam-tui-views"));
        assert_ne!(id, thread::current().id());
        lane.join().unwrap();
    }

    #[test]
    fn a_panicking_request_calls_on_death_once_and_join_says_so() {
        let deaths = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&deaths);
        let lane: Lane<Req> = Lane::spawn(
            "doomed",
            || (),
            |_: &mut (), req: Req| {
                if req == Req::Boom {
                    panic!("the lane's work panicked");
                }
            },
            move || {
                counted.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert!(lane.submit(Req::Boom));
        let deadline = Instant::now() + WAIT;
        while deaths.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "on_death never ran");
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!lane.submit(Req::Keep(1)), "a dead lane is closed");
        assert!(lane.join().is_err());
        assert_eq!(deaths.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_panicking_init_calls_on_death_too() {
        let deaths = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&deaths);
        let lane: Lane<Req> = Lane::spawn(
            "stillborn",
            || panic!("the lane's state could not be built"),
            |_: &mut (), _: Req| {},
            move || {
                counted.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert!(lane.join().is_err());
        assert_eq!(deaths.load(Ordering::SeqCst), 1);
    }
}
