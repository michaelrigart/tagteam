//! The daemon's socket from the TUI's side (spec §8): the live thread, which subscribes and
//! passes on what it reads through a bounded queue, and the `status` fetch that the daemon
//! lane makes over a connection of its own. Neither touches the store, and neither is joined
//! (§4.3, §10.2): each ends with the process, or once the TUI no longer listens.

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tagteam_daemon::client::{ClientError, Closer, Connect, Events, connect_at};
use tagteam_daemon::protocol::{Event, Status};

/// What the live thread passes on (spec §8).
#[derive(Debug, Clone, PartialEq)]
pub enum Live {
    /// Subscribed: every event the daemon emits from now on follows, until the next `Down`.
    /// `link` names this connection, 1 for the thread's first, then 2 and on (C-R21).
    Up { link: u64 },
    /// No link: no daemon answered, the connection failed, or it ended (`stopping`, a full
    /// queue, an error).
    Down,
    /// The daemon speaks protocol `server`, which this TUI does not (D5.6).
    Mismatch { server: u32 },
    /// One event, as the daemon sent it.
    Event(Event),
}

/// How long the live thread waits before it connects again (spec §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    /// The first wait, after a failure or a link that came down. Each failure doubles it.
    pub first: Duration,
    /// The longest wait. A link that stayed up at least this long starts the waits over.
    pub most: Duration,
    /// The wait after a daemon that speaks another protocol.
    pub mismatch: Duration,
}

impl Default for Backoff {
    /// Spec §8: from 1 s up to 30 s, and every 60 s after a mismatch.
    fn default() -> Self {
        Backoff {
            first: Duration::from_secs(1),
            most: Duration::from_secs(30),
            mismatch: Duration::from_secs(60),
        }
    }
}

/// What `fetch_status` says when no daemon answers.
pub const NO_DAEMON: &str = "no daemon answers";
/// C-R17: the most the live thread's queue holds, as the daemon's own queue for a subscriber
/// (D5.5).
pub const LIVE_QUEUE: usize = 256;

/// What `LiveQueue::push` did with what the live thread passed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// It waits for the UI thread.
    Taken,
    /// The queue was full: what it held was dropped for one `Down`, and the live thread drops
    /// its link.
    Full,
    /// The TUI no longer listens.
    Gone,
}

/// C-R17: what the live thread passes on, on its way to the UI thread. At most `cap` things
/// wait, so a UI thread that takes none (a suspension, spec §7.3) holds a bounded backlog. A
/// push into a full queue drops what it holds for a single `Down`, and the live thread drops its
/// link: the UI goes to timer mode, and the next link's snapshot (`status` and every view
/// shown, C-R4) restores the meaning of what was dropped. `wake` tells the UI thread there is
/// something to take each time the queue goes from empty to not, and says false once nobody
/// listens. The queue also holds the link's closer, by the link's name, so that the UI can end a
/// link that no longer works, and only that one (C-R19, C-R21).
pub struct LiveQueue {
    inner: Mutex<Inner>,
    cap: usize,
    closed: AtomicBool,
    wake: Box<dyn Fn() -> bool + Send + Sync>,
}

struct Inner {
    items: VecDeque<Live>,
    link: Option<(u64, Closer)>,
}

impl LiveQueue {
    pub fn new(cap: usize, wake: impl Fn() -> bool + Send + Sync + 'static) -> LiveQueue {
        LiveQueue {
            inner: Mutex::new(Inner {
                items: VecDeque::new(),
                link: None,
            }),
            cap: cap.max(1),
            closed: AtomicBool::new(false),
            wake: Box::new(wake),
        }
    }

    /// The queue, whatever a panic left it as: it holds only messages and a handle.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues `live` for the UI thread, or, when the queue is full, drops what it holds for a
    /// single `Down`.
    pub fn push(&self, live: Live) -> Delivery {
        if self.closed.load(Ordering::SeqCst) {
            return Delivery::Gone;
        }
        let mut inner = self.lock();
        let woken = inner.items.is_empty();
        let delivery = if inner.items.len() >= self.cap {
            inner.items.clear();
            inner.items.push_back(Live::Down);
            Delivery::Full
        } else {
            inner.items.push_back(live);
            Delivery::Taken
        };
        drop(inner);
        if woken && !(self.wake)() {
            self.close();
            return Delivery::Gone;
        }
        delivery
    }

    /// The TUI no longer listens: every later push says `Gone`, whatever the queue holds, and the
    /// live thread returns.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    /// Everything waiting, oldest first; the UI thread takes it at each wake.
    pub fn take(&self) -> Vec<Live> {
        self.lock().items.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.lock().items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// C-R19, C-R21: ends link `link`, if the live thread still holds it; it then goes down and
    /// connects again with backoff. The UI calls it for a link that no longer works. A link the
    /// thread has since replaced is gone already, and its successor is left alone.
    pub fn end_link(&self, link: u64) {
        if let Some((held, closer)) = &self.lock().link {
            if *held == link {
                closer.close();
            }
        }
    }

    /// The link the live thread holds now, by its name, if any.
    fn hold(&self, link: Option<(u64, Closer)>) {
        self.lock().link = link;
    }
}

/// Starts the live thread on the daemon at `path`. What it learns goes into `queue`; it returns
/// once nobody listens. `on_death` runs if the thread unwinds from a panic.
pub fn spawn(
    path: PathBuf,
    backoff: Backoff,
    queue: Arc<LiveQueue>,
    on_death: impl FnOnce() + Send + 'static,
) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("tagteam-tui-live".into())
        .spawn(move || {
            let _death = OnPanic(Some(on_death));
            run(&path, backoff, &queue);
        })
}

/// Runs its closure if the thread unwinds from a panic.
struct OnPanic<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Drop for OnPanic<F> {
    fn drop(&mut self) {
        if thread::panicking() {
            if let Some(f) = self.0.take() {
                f();
            }
        }
    }
}

/// What one try at a link came to.
enum Attempt {
    /// It came up, and went down after `lasted`.
    Lost { lasted: Duration },
    /// No link came up.
    Failed,
    /// The daemon speaks another protocol.
    Mismatch,
    /// The TUI no longer listens.
    Gone,
}

/// Spec §8: connects and subscribes, passes on every event, and once the link is down tries
/// again, waiting `first`, then twice as long after each failure, up to `most`; after a
/// mismatch it waits `mismatch`. A link that went down within `most` of coming up keeps the
/// longer waits, so a daemon that drops it at once is not reconnected to in a loop.
fn run(path: &Path, backoff: Backoff, queue: &LiveQueue) {
    let mut wait = backoff.first;
    let mut links = 0;
    loop {
        let tried = attempt(path, queue, &mut links);
        if matches!(tried, Attempt::Gone) {
            return;
        }
        thread::sleep(pause(&mut wait, &tried, backoff));
    }
}

/// How long to wait after `tried`, and `wait` moved on for the one after: a mismatch waits
/// `mismatch` and leaves `wait` alone; a link that lasted at least `most` waits `first` and
/// starts the doubling over; any other failure waits `wait` and doubles it, up to `most`.
fn pause(wait: &mut Duration, tried: &Attempt, backoff: Backoff) -> Duration {
    match tried {
        Attempt::Mismatch => backoff.mismatch,
        Attempt::Lost { lasted } if *lasted >= backoff.most => {
            *wait = (backoff.first * 2).min(backoff.most);
            backoff.first
        }
        Attempt::Lost { .. } | Attempt::Failed | Attempt::Gone => {
            let now = *wait;
            *wait = (now * 2).min(backoff.most);
            now
        }
    }
}

/// One try: the connection, the greeting and the subscription, then the events until the link
/// ends. `links` counts the links made, and names this one if it comes up.
fn attempt(path: &Path, queue: &LiveQueue, links: &mut u64) -> Attempt {
    let (said, attempt) = match connect_at(path) {
        Ok(Connect::Connected(client)) => match client.subscribe() {
            Ok(events) => {
                *links += 1;
                let link = *links;
                let Ok(closer) = events.closer() else {
                    *links -= 1;
                    return match queue.push(Live::Down) {
                        Delivery::Gone => Attempt::Gone,
                        Delivery::Taken | Delivery::Full => Attempt::Failed,
                    };
                };
                queue.hold(Some((link, closer)));
                let attempt = linked(events, link, queue);
                queue.hold(None);
                return attempt;
            }
            Err(_) => (Live::Down, Attempt::Failed),
        },
        Ok(Connect::Mismatch { server, .. }) => (Live::Mismatch { server }, Attempt::Mismatch),
        Ok(Connect::NoDaemon) | Err(_) => (Live::Down, Attempt::Failed),
    };
    match queue.push(said) {
        Delivery::Gone => Attempt::Gone,
        Delivery::Taken | Delivery::Full => attempt,
    }
}

/// A subscribed link, `link`: `Up`, each event as it arrives, then `Down` once the daemon closes
/// it, it fails, or it is ended (`LiveQueue::end_link`). `stopping` ends it at once, whether or
/// not the daemon goes on to close its socket (C-R18); a full queue ends it too (C-R17).
/// Returning drops the connection.
fn linked(events: Events, link: u64, queue: &LiveQueue) -> Attempt {
    let up = Instant::now();
    let lost = |up: Instant| Attempt::Lost {
        lasted: up.elapsed(),
    };
    match queue.push(Live::Up { link }) {
        Delivery::Taken => {}
        Delivery::Full => return lost(up),
        Delivery::Gone => return Attempt::Gone,
    }
    for event in events {
        let Ok(event) = event else {
            break;
        };
        let stopping = matches!(event, Event::Stopping { .. });
        match queue.push(Live::Event(event)) {
            Delivery::Taken if stopping => break,
            Delivery::Taken => {}
            Delivery::Full => return lost(up),
            Delivery::Gone => return Attempt::Gone,
        }
    }
    match queue.push(Live::Down) {
        Delivery::Gone => Attempt::Gone,
        Delivery::Taken | Delivery::Full => lost(up),
    }
}

/// Why the daemon's `status` could not be read (CC-O-23c).
#[derive(Debug)]
pub enum StatusError {
    /// Nothing answers on the socket.
    NoDaemon,
    /// The daemon speaks protocol `server`, which this client does not.
    Mismatch { server: u32 },
    /// The connection or the reply failed.
    Client(ClientError),
}

impl std::fmt::Display for StatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatusError::NoDaemon => f.write_str(NO_DAEMON),
            StatusError::Mismatch { server } => write!(f, "the daemon speaks protocol {server}"),
            StatusError::Client(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for StatusError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StatusError::Client(e) => Some(e),
            StatusError::NoDaemon | StatusError::Mismatch { .. } => None,
        }
    }
}

/// Spec §8: the daemon's `status`, over a short-lived connection of its own, since the live
/// thread's is the event stream's. It waits 250 ms at most for the greeting and 2 s for the
/// reply (D5.7).
pub fn fetch_status(path: &Path) -> Result<Status, StatusError> {
    match connect_at(path) {
        Ok(Connect::Connected(mut client)) => client.status().map_err(StatusError::Client),
        Ok(Connect::NoDaemon) => Err(StatusError::NoDaemon),
        Ok(Connect::Mismatch { server, .. }) => Err(StatusError::Mismatch { server }),
        Err(e) => Err(StatusError::Client(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn backoff() -> Backoff {
        Backoff {
            first: ms(20),
            most: ms(160),
            mismatch: ms(1_000),
        }
    }

    #[test]
    fn failures_wait_first_then_twice_as_long_up_to_most() {
        let b = backoff();
        let mut wait = b.first;
        let waits: Vec<Duration> = (0..7)
            .map(|_| pause(&mut wait, &Attempt::Failed, b))
            .collect();
        assert_eq!(waits, [20, 40, 80, 160, 160, 160, 160].map(ms));
    }

    #[test]
    fn a_link_that_goes_down_within_most_doubles_the_wait_like_a_failure() {
        let b = backoff();
        let mut wait = b.first;
        let lost = Attempt::Lost { lasted: ms(159) };
        let waits: Vec<Duration> = (0..4).map(|_| pause(&mut wait, &lost, b)).collect();
        assert_eq!(waits, [20, 40, 80, 160].map(ms));
    }

    #[test]
    fn a_link_that_lasted_most_waits_first_and_starts_the_doubling_over() {
        let b = backoff();
        let mut wait = b.first;
        for _ in 0..5 {
            pause(&mut wait, &Attempt::Failed, b);
        }
        assert_eq!(wait, b.most);
        let lasted = Attempt::Lost { lasted: b.most };
        assert_eq!(pause(&mut wait, &lasted, b), b.first);
        assert_eq!(pause(&mut wait, &Attempt::Failed, b), ms(40));
        assert_eq!(pause(&mut wait, &Attempt::Failed, b), ms(80));
    }

    #[test]
    fn a_mismatch_waits_its_own_time_and_leaves_the_wait_as_it_was() {
        let b = backoff();
        let mut wait = ms(80);
        assert_eq!(pause(&mut wait, &Attempt::Mismatch, b), b.mismatch);
        assert_eq!(wait, ms(80));
        assert_eq!(pause(&mut wait, &Attempt::Failed, b), ms(80));
    }

    #[test]
    fn the_doubling_never_passes_most_when_first_is_longer_than_half_of_it() {
        let b = Backoff {
            first: ms(100),
            most: ms(150),
            mismatch: ms(1),
        };
        let mut wait = b.first;
        assert_eq!(pause(&mut wait, &Attempt::Failed, b), ms(100));
        assert_eq!(pause(&mut wait, &Attempt::Failed, b), ms(150));
        let lasted = Attempt::Lost { lasted: b.most };
        assert_eq!(pause(&mut wait, &lasted, b), ms(100));
        assert_eq!(wait, ms(150));
    }
}
