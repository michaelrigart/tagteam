//! D5: the daemon's socket. Start-up binds it once `daemon.lock` is held (D4.1 step 5). It
//! serves protocol 1 to at most sixteen connections of this user: the greeting, `status` from
//! the board, and every event to the subscribers (D5.2–D5.5). One thread accepts, and each
//! connection has a thread that reads its requests and writes its replies and events
//! (Decision 2). The threads block in `poll` on their descriptors and wake pipes (H8, which
//! supersedes Decision 2's 100 ms read): the accept thread on the listener and the hub's pipe,
//! a connection on its socket, the server-stop pipe and, once subscribed, its own pipe, which
//! the hub writes for each event it queues. The scheduler only hands events to the hub, which
//! never waits on a client (D4). The hub closes the socket when `stopping` passes through it,
//! while the daemon still holds `daemon.lock` (D4.6, Decision 9).

use std::fs;
use std::io::{self, PipeReader, PipeWriter, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tagteam_provider::atomic::ensure_private_dir;
use tagteam_provider::{Cancel, Clock, SystemClock};

use crate::events::{DaemonEvent, DaemonSink};
use crate::protocol::{
    MAX_LINE, Op, Parsed, Request, daemon_event_json, error_json, hello_json, ok_json,
    parse_request, status_json,
};
use crate::status::StatusBoard;

/// D5.1: the longest socket path `bind` takes, in bytes: `sun_path` less its NUL. The engine
/// owns it, because `doctor` checks the same bound (plan A Decision 2).
pub use tagteam_engine::daemon::MAX_SOCKET_PATH;

/// D5.1: at most sixteen connections; the next is closed before the greeting.
pub const MAX_CONNECTIONS: usize = 16;
/// D5.5: the events a subscriber may have waiting before it is disconnected.
pub const QUEUE: usize = 256;
/// D5.2: how long a connection that has not subscribed may send nothing.
pub const IDLE: Duration = Duration::from_secs(30);
/// How long one write to a client may block. A client that takes nothing for that long is not
/// reading, and its connection is closed, so `shutdown` never waits on it for longer.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// How long the accept thread waits after a failed poll or accept before it tries again: a
/// listener with a connection it cannot accept stays readable, so an unbounded poll would spin.
const RETRY: Duration = Duration::from_millis(100);

/// The umask is the process's: binds in one process take turns, so one never puts back a mask
/// another narrowed.
static UMASK: Mutex<()> = Mutex::new(());

/// A server's timeouts. `Default` is D5's; tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// D5.2's idle timeout, before a subscribe.
    pub idle: Duration,
    /// How long a write to a client waits for room in its socket (`WRITE_TIMEOUT`).
    pub write_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            idle: IDLE,
            write_timeout: WRITE_TIMEOUT,
        }
    }
}

/// Why the socket was not bound. The daemon logs it at ERROR and runs without a socket (D5.1).
#[derive(Debug, thiserror::Error)]
pub enum BindError {
    #[error(
        "the socket path {} is longer than the {} bytes a Unix socket address holds",
        .0.display(),
        MAX_SOCKET_PATH
    )]
    TooLong(PathBuf),
    #[error("the socket could not be bound: {0}")]
    Io(#[from] io::Error),
}

/// D5's socket server: its accept thread, its connections and their hub.
pub struct Server {
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

/// What the accept and connection threads share.
struct Shared {
    board: Arc<StatusBoard>,
    hub: Hub,
    /// The home `status` writes paths under as `~/…` (D5.4).
    home: PathBuf,
    limits: Limits,
    /// Set by `shutdown`: each connection writes what it holds, then closes.
    stop: AtomicBool,
    /// Written once `stop` is set, and never drained, so every connection's `poll` wakes.
    stop_wake: Wake,
    /// Connections being served, for D5.1's limit.
    open: AtomicUsize,
    /// The connection threads; finished ones are let go at each accept.
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl Server {
    /// D5.1: removes a leftover file, binds 0600, starts the accept thread. The caller holds
    /// `daemon.lock`. `home` is the one `status` writes paths under as `~/…` (D5.4).
    pub fn bind(
        path: &Path,
        home: &Path,
        board: Arc<StatusBoard>,
        cancel: Cancel,
    ) -> Result<Server, BindError> {
        Self::bind_with(path, home, board, cancel, Limits::default())
    }

    /// `bind` with other timeouts.
    pub fn bind_with(
        path: &Path,
        home: &Path,
        board: Arc<StatusBoard>,
        cancel: Cancel,
        limits: Limits,
    ) -> Result<Server, BindError> {
        if path.as_os_str().as_bytes().len() > MAX_SOCKET_PATH {
            return Err(BindError::TooLong(path.to_path_buf()));
        }
        // Made before the bind, so a failure leaves no socket file.
        let stop_wake = Wake::new()?;
        let closed_wake = Wake::new()?;
        let listener = bind_private(path)?;
        let bound = identity(path).map(|identity| Bound {
            path: path.to_path_buf(),
            identity,
        });
        let shared = Arc::new(Shared {
            hub: Hub::new(Arc::clone(&board), bound, closed_wake),
            board,
            home: home.to_path_buf(),
            limits,
            stop: AtomicBool::new(false),
            stop_wake,
            open: AtomicUsize::new(0),
            threads: Mutex::new(Vec::new()),
        });
        let accepting = Arc::clone(&shared);
        let accept = listener
            .set_nonblocking(true)
            .and_then(|()| {
                thread::Builder::new()
                    .name("tagteam-accept".into())
                    .spawn(move || accept_loop(listener, &accepting, &cancel))
            })
            .inspect_err(|_| {
                let _ = fs::remove_file(path);
            })?;
        tracing::debug!(path = %path.display(), "the socket listens");
        Ok(Server {
            shared,
            accept: Some(accept),
        })
    }

    /// Where the scheduler's events go (D5.5).
    pub fn hub(&self) -> &Hub {
        &self.shared.hub
    }

    /// D4.6: closes every connection once it has written what it holds (the `stopping` event
    /// among it), and joins the threads. The socket file was removed when `stopping` went out;
    /// for a hub that never saw it, this removes the file now, if it is still the one this
    /// server bound.
    pub fn shutdown(mut self) {
        self.close();
    }

    fn close(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.stop_wake.wake();
        self.shared.hub.close_socket();
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
        let threads = std::mem::take(&mut *lock(&self.shared.threads));
        for thread in threads {
            let _ = thread.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.close();
    }
}

/// A poisoned lock's data is still whole here: no holder leaves it half-changed.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// D5.1: the parent made 0700 when it is missing, a leftover file removed, and the socket bound
/// with the umask narrowed to 0177, so the file is created 0600 and never chmod'ed. When the
/// daemon binds (D4.1 step 5), the scheduler has not started and the notifier has nothing to
/// send, so no thread creates a file or starts a process while the mask is narrowed. Binds in
/// one process take turns (`UMASK`), the parent's creation included.
fn bind_private(path: &Path) -> io::Result<UnixListener> {
    let _turn = lock(&UMASK);
    if let Some(dir) = path.parent() {
        ensure_private_dir(dir)?;
    }
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    // SAFETY: umask(2) only swaps the process's file mode creation mask; it cannot fail.
    let found = unsafe { libc::umask(0o177) };
    let bound = UnixListener::bind(path);
    // SAFETY: as above; this puts back the mask that was in force.
    unsafe { libc::umask(found) };
    bound
}

/// A file's device and inode, as `Hub::close_socket` compares them.
fn identity(path: &Path) -> Option<(u64, u64)> {
    fs::symlink_metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

/// D5.1, D4.6, H8: accepts until `stopping` or `shutdown` wakes it through the hub's pipe, or
/// until its first wake after a signal: a connection, which it admits, or `stopping`, which the
/// scheduler emits within its 1 s slice. Returning drops the listener, so a later connect is
/// refused at once: a client finds no daemon, never one that does not greet (Review Focus 1).
fn accept_loop(listener: UnixListener, shared: &Arc<Shared>, cancel: &Cancel) {
    while !shared.stop.load(Ordering::SeqCst)
        && shared.hub.accepting()
        && cancel.requested().is_none()
    {
        let mut fds = [
            pollfd(listener.as_raw_fd(), libc::POLLIN),
            pollfd(shared.hub.closed_wake.fd(), libc::POLLIN),
        ];
        match poll(&mut fds, None) {
            Ok(_) if ready(&fds[1]) => {}
            Ok(_) if ready(&fds[0]) => accept_pending(&listener, shared),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                tracing::debug!(error = %e, "the socket could not be polled");
                thread::sleep(RETRY);
            }
        }
    }
}

/// Every connection waiting now; the listener does not block.
fn accept_pending(listener: &UnixListener, shared: &Arc<Shared>) {
    loop {
        match listener.accept() {
            Ok((stream, _)) => admit(stream, shared),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                // Out of descriptors, say: the next poll tries again.
                tracing::debug!(error = %e, "a connection could not be accepted");
                thread::sleep(RETRY);
                return;
            }
        }
    }
}

/// H8: a pipe that wakes a thread waiting in `poll` on its read end. Both ends are
/// non-blocking and close-on-exec. Each subscriber has one, beside the server's two latches
/// (the stop and the hub's close): two descriptors each, so at most 36 for the pipes.
struct Wake {
    read: PipeReader,
    write: PipeWriter,
}

impl Wake {
    fn new() -> io::Result<Wake> {
        let (read, write) = io::pipe()?;
        set_nonblocking(read.as_fd())?;
        set_nonblocking(write.as_fd())?;
        Ok(Wake { read, write })
    }

    /// One byte. A full pipe is ignored, as is any error: its reader is already due to wake.
    fn wake(&self) {
        let _ = (&self.write).write(&[1]);
    }

    /// Takes every byte waiting, so the next `poll` waits for the next `wake`.
    fn drain(&self) {
        let mut bytes = [0u8; 64];
        while let Ok(1..) = (&self.read).read(&mut bytes) {}
    }

    /// The read end, which `poll` watches.
    fn fd(&self) -> RawFd {
        self.read.as_raw_fd()
    }
}

fn set_nonblocking(fd: BorrowedFd<'_>) -> io::Result<()> {
    let fd = fd.as_raw_fd();
    // SAFETY: fcntl(2) with F_GETFL and F_SETFL only reads and sets the status flags of a
    // descriptor that `fd` keeps open.
    let rc = unsafe {
        match libc::fcntl(fd, libc::F_GETFL) {
            -1 => -1,
            flags => libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK),
        }
    };
    if rc == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// `fd` watched for `events`. A negative `fd` is ignored by `poll`.
fn pollfd(fd: RawFd, events: libc::c_short) -> libc::pollfd {
    libc::pollfd {
        fd,
        events,
        revents: 0,
    }
}

/// poll(2) on `fds`: how many are ready. `None` waits with no bound; a timeout is rounded up
/// to a whole millisecond, so a wait never spins. `EINTR` is `ErrorKind::Interrupted`.
fn poll(fds: &mut [libc::pollfd], timeout: Option<Duration>) -> io::Result<usize> {
    let ms = timeout.map_or(-1, |t| {
        libc::c_int::try_from(t.as_nanos().div_ceil(1_000_000)).unwrap_or(libc::c_int::MAX)
    });
    let len = libc::nfds_t::try_from(fds.len()).map_err(|_| io::ErrorKind::InvalidInput)?;
    // SAFETY: `fds` is a writable slice of `pollfd`s, and the callers keep each descriptor in
    // it open for the call; a negative one is ignored.
    match unsafe { libc::poll(fds.as_mut_ptr(), len, ms) } {
        -1 => Err(io::Error::last_os_error()),
        n => Ok(usize::try_from(n).unwrap_or(0)),
    }
}

/// Whether `poll` found `fd` readable, or at its end, or in error: each makes the next read
/// return at once.
fn ready(fd: &libc::pollfd) -> bool {
    fd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
}

/// D5.1, Decision 3: another user's connection, and one past the limit, are closed before the
/// greeting. Any other gets a thread of its own.
fn admit(stream: UnixStream, shared: &Arc<Shared>) {
    match peer_uid(&stream) {
        Ok(uid) if uid == own_uid() => {}
        Ok(uid) => {
            tracing::debug!(uid, "a connection from another user was closed");
            return;
        }
        Err(e) => {
            tracing::debug!(error = %e, "a connection whose peer is unknown was closed");
            return;
        }
    }
    if shared.open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
        shared.open.fetch_sub(1, Ordering::SeqCst);
        tracing::debug!(
            limit = MAX_CONNECTIONS,
            "a connection past the limit was closed"
        );
        return;
    }
    let slot = Slot(Arc::clone(shared));
    let spawned = thread::Builder::new()
        .name("tagteam-socket".into())
        .spawn(move || serve(stream, slot));
    match spawned {
        Ok(thread) => {
            let mut threads = lock(&shared.threads);
            threads.retain(|t| !t.is_finished());
            threads.push(thread);
        }
        Err(e) => {
            tracing::debug!(error = %e, "a connection's thread could not start; it was closed");
        }
    }
}

/// One of D5.1's sixteen places, given back when its connection ends.
struct Slot(Arc<Shared>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::SeqCst);
    }
}

fn own_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Decision 3: the effective uid of the process at the other end of `stream`, as the kernel
/// recorded it when that process connected.
#[cfg(target_os = "linux")]
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` is writable for `len` bytes, which `getsockopt` updates; the descriptor
    // stays open while `stream` lives.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &mut len,
        )
    };
    if rc == 0 {
        Ok(cred.uid)
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Decision 3: the effective uid of the process at the other end of `stream`, as the kernel
/// recorded it when that process connected.
#[cfg(not(target_os = "linux"))]
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let (mut uid, mut gid): (libc::uid_t, libc::gid_t) = (0, 0);
    // SAFETY: `uid` and `gid` are writable; the descriptor stays open while `stream` lives.
    let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if rc == 0 {
        Ok(uid)
    } else {
        Err(io::Error::last_os_error())
    }
}

/// D5.5's fan-out to the subscribers. `emit` only `try_send`s, so the scheduler never waits on
/// a client. A subscriber whose queue is full is dropped and its socket shut down, which ends
/// its connection even while its thread is blocked writing; the client re-reads what it shows
/// and subscribes again. The hub keeps the board's `subscribers` current. On `stopping` it
/// closes the socket (D4.6, Decision 9).
pub struct Hub {
    board: Arc<StatusBoard>,
    subscribers: Mutex<Vec<Subscriber>>,
    next: AtomicU64,
    /// The socket file as `bind` created it, until `close_socket` has dealt with it.
    file: Mutex<Option<Bound>>,
    /// Set by `close_socket`: the accept thread returns, dropping the listener.
    closed: AtomicBool,
    /// Written once `closed` is set, and never drained, so the accept thread's `poll` wakes.
    closed_wake: Wake,
}

/// The socket file `bind` created: its path, and its device and inode then.
struct Bound {
    path: PathBuf,
    identity: (u64, u64),
}

struct Subscriber {
    id: u64,
    queue: SyncSender<Arc<str>>,
    /// Its connection's socket, shut down when the hub drops the subscriber.
    socket: UnixStream,
    /// Written after each event queued, so its connection's `poll` wakes to write it.
    wake: Arc<Wake>,
}

impl Hub {
    fn new(board: Arc<StatusBoard>, file: Option<Bound>, closed_wake: Wake) -> Self {
        Self {
            board,
            subscribers: Mutex::new(Vec::new()),
            next: AtomicU64::new(0),
            file: Mutex::new(file),
            closed: AtomicBool::new(false),
            closed_wake,
        }
    }

    /// Whether the accept thread still takes new connections.
    fn accepting(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
    }

    /// D4.6, Decision 9: no new connection is accepted, and the socket file is removed while
    /// its device and inode are still the ones `bind` created. `stopping` calls this while the
    /// daemon still holds `daemon.lock`; `shutdown` calls it again, after the lock has gone,
    /// when a successor may have bound its own socket at the path, which then stays. Only the
    /// first call removes anything.
    fn close_socket(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.closed_wake.wake();
        let Some(bound) = lock(&self.file).take() else {
            return;
        };
        if identity(&bound.path) != Some(bound.identity) {
            return;
        }
        match fs::remove_file(&bound.path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                tracing::warn!(error = %e, path = %bound.path.display(), "the socket file could not be removed");
            }
            _ => tracing::debug!(path = %bound.path.display(), "the socket file was removed"),
        }
    }

    /// `e`, serialized once and stamped with the wall clock, for every subscriber.
    fn queue(&self, e: &DaemonEvent) {
        let mut subscribers = lock(&self.subscribers);
        if subscribers.is_empty() {
            return;
        }
        let now_s = SystemClock.now_ms().div_euclid(1000);
        let line: Arc<str> = format!("{}\n", daemon_event_json(e, now_s)).into();
        let before = subscribers.len();
        subscribers.retain(|s| match s.queue.try_send(Arc::clone(&line)) {
            Ok(()) => {
                s.wake.wake();
                true
            }
            Err(TrySendError::Full(_)) => {
                tracing::debug!(
                    queue = QUEUE,
                    "a subscriber fell behind and was disconnected"
                );
                let _ = s.socket.shutdown(Shutdown::Both);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        });
        if subscribers.len() != before {
            self.board.set_subscribers(subscribers.len());
        }
    }

    /// A new subscriber on `socket`'s connection, woken through `wake`: the id `unsubscribe`
    /// takes, and its queue.
    fn subscribe(&self, socket: UnixStream, wake: Arc<Wake>) -> (u64, Receiver<Arc<str>>) {
        let (queue, events) = sync_channel(QUEUE);
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let mut subscribers = lock(&self.subscribers);
        subscribers.push(Subscriber {
            id,
            queue,
            socket,
            wake,
        });
        self.board.set_subscribers(subscribers.len());
        (id, events)
    }

    fn unsubscribe(&self, id: u64) {
        let mut subscribers = lock(&self.subscribers);
        let before = subscribers.len();
        subscribers.retain(|s| s.id != id);
        if subscribers.len() != before {
            self.board.set_subscribers(subscribers.len());
        }
    }
}

impl DaemonSink for Hub {
    /// Each event is queued for every subscriber. `stopping` is queued like any other, then the
    /// socket closes to new connections and its file goes (D4.6, Decision 9).
    fn emit(&self, e: &DaemonEvent) {
        self.queue(e);
        if matches!(e, DaemonEvent::Stopping { .. }) {
            self.close_socket();
        }
    }
}

/// A connection's thread (Decision 2): the greeting, then requests answered and queued events
/// written in the order they came. It ends when the client leaves, breaks the framing, falls
/// behind, or idles, or when the server shuts down.
fn serve(stream: UnixStream, slot: Slot) {
    let Ok(mut connection) = Connection::new(stream, &slot.0) else {
        return;
    };
    tracing::debug!("a socket connection opened");
    let why = connection.run();
    tracing::debug!(why, "a socket connection closed");
}

struct Connection<'s> {
    shared: &'s Shared,
    stream: UnixStream,
    /// Bytes received that are not yet a whole line.
    pending: Vec<u8>,
    /// When the client last sent anything (D5.2's idle timeout).
    heard: Instant,
    /// Its id at the hub, its queue and the pipe the hub wakes it through, once it has
    /// subscribed.
    subscription: Option<(u64, Receiver<Arc<str>>, Arc<Wake>)>,
}

impl<'s> Connection<'s> {
    fn new(stream: UnixStream, shared: &'s Shared) -> io::Result<Self> {
        // The socket is non-blocking (on macOS an accepted one inherits the listener's
        // O_NONBLOCK, on Linux not), so the thread waits only in `poll`: a read finds what
        // `poll` saw or nothing, and a write waits there for room (H8).
        stream.set_nonblocking(true)?;
        Ok(Self {
            shared,
            stream,
            pending: Vec::new(),
            heard: Instant::now(),
            subscription: None,
        })
    }

    /// Serves the connection, and says why it ended, for the DEBUG line (D11).
    fn run(&mut self) -> &'static str {
        if self
            .send(&hello_json(&self.shared.board.snapshot().daemon))
            .is_err()
        {
            return "the greeting could not be written";
        }
        let mut chunk = [0u8; 8192];
        loop {
            // Read before the queue is written: whatever was queued before `shutdown`, the
            // `stopping` event among it, is written before the connection closes (D4.6).
            let stopping = self.shared.stop.load(Ordering::SeqCst);
            if self.flush().is_err() {
                return "an event could not be written";
            }
            if stopping {
                return "the daemon stopped";
            }
            // D5.2's idle timeout bounds the wait before a subscribe; after it, nothing does.
            let timeout = if self.subscription.is_some() {
                None
            } else {
                let left = self.shared.limits.idle.saturating_sub(self.heard.elapsed());
                if left.is_zero() {
                    return "it was idle";
                }
                Some(left)
            };
            let wake = self.subscription.as_ref().map(|(_, _, wake)| wake);
            let mut fds = [
                pollfd(self.stream.as_raw_fd(), libc::POLLIN),
                pollfd(self.shared.stop_wake.fd(), libc::POLLIN),
                pollfd(wake.map_or(-1, |wake| wake.fd()), libc::POLLIN),
            ];
            match poll(&mut fds, timeout) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return "its socket could not be polled",
            }
            // Drained before the next `flush`, so an event queued after this leaves a byte for
            // the next `poll`, and no wake is lost.
            if let Some(wake) = wake.filter(|_| ready(&fds[2])) {
                wake.drain();
            }
            if !ready(&fds[0]) {
                continue;
            }
            match self.stream.read(&mut chunk) {
                Ok(0) => return "the client closed it",
                Ok(n) => {
                    self.heard = Instant::now();
                    self.pending.extend_from_slice(&chunk[..n]);
                    if let Err(why) = self.answer() {
                        return why;
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return "a read failed",
            }
        }
    }

    /// Answers each whole line received (D5.2, D5.3, Decision 1). `Err` closes the connection.
    fn answer(&mut self) -> Result<(), &'static str> {
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            match parse_request(&line[..end]) {
                Parsed::Close => return Err("it broke the framing"),
                Parsed::Refuse(e) => self.reply(&error_json(&e))?,
                Parsed::Request(r) => self.request(r)?,
            }
        }
        if self.pending.len() > MAX_LINE {
            return Err("it sent a line over 64 KiB");
        }
        Ok(())
    }

    fn request(&mut self, r: Request) -> Result<(), &'static str> {
        match r.op {
            Op::Status => {
                let status = status_json(&self.shared.board.snapshot(), &self.shared.home);
                self.reply(&ok_json(&r.id, status))
            }
            Op::Subscribe if self.subscription.is_some() => self.reply(&ok_json(&r.id, json!({}))),
            Op::Subscribe => {
                let socket = self
                    .stream
                    .try_clone()
                    .map_err(|_| "its socket could not be shared with the hub")?;
                let wake = Arc::new(Wake::new().map_err(|_| "its wake pipe could not be made")?);
                let (id, events) = self.shared.hub.subscribe(socket, Arc::clone(&wake));
                self.subscription = Some((id, events, wake));
                // D5.5: the `ok` comes before any event, so it goes ahead of the new queue.
                self.send(&ok_json(&r.id, json!({})))
                    .map_err(|_| "a reply could not be written")
            }
        }
    }

    /// A reply, after every event queued before it (D5.3's order).
    fn reply(&self, v: &Value) -> Result<(), &'static str> {
        self.flush().map_err(|_| "an event could not be written")?;
        self.send(v).map_err(|_| "a reply could not be written")
    }

    /// Writes every event queued for this subscriber.
    fn flush(&self) -> io::Result<()> {
        if let Some((_, queue, _)) = &self.subscription {
            while let Ok(line) = queue.try_recv() {
                self.write_all(line.as_bytes())?;
            }
        }
        Ok(())
    }

    /// One object, as one line.
    fn send(&self, v: &Value) -> io::Result<()> {
        let mut line = v.to_string();
        line.push('\n');
        self.write_all(line.as_bytes())
    }

    /// `bytes`, whole. Whenever the socket has no room, the write waits in `poll` for at most
    /// the write timeout from then: `TimedOut` after it. It never waits on the stop pipe, so a
    /// connection writes what it holds before it closes (Decision 9). A subscriber the hub
    /// dropped ends sooner: the hub's shutdown of its socket wakes the `poll`, and the write
    /// then fails.
    fn write_all(&self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            match (&self.stream).write(bytes) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => bytes = &bytes[n..],
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    let (started, timeout) = (Instant::now(), self.shared.limits.write_timeout);
                    loop {
                        let mut fds = [pollfd(self.stream.as_raw_fd(), libc::POLLOUT)];
                        match poll(&mut fds, Some(timeout.saturating_sub(started.elapsed()))) {
                            Ok(0) => return Err(io::ErrorKind::TimedOut.into()),
                            Ok(_) => break,
                            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                            Err(e) => return Err(e),
                        }
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

impl Drop for Connection<'_> {
    fn drop(&mut self) {
        if let Some((id, _, _)) = self.subscription.take() {
            self.shared.hub.unsubscribe(id);
        }
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}
