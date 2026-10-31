//! The input thread (TUI spec §4.3): the terminal's events as `Msg`s. It blocks until the
//! terminal has bytes, SIGWINCH arrives or the UI thread wakes it, so it makes no wake-up while
//! idle. It can pause, and acknowledge the pause, before the terminal is handed to a foreground
//! action (§7.3 step 1), and it reads nothing once the TUI has let the terminal go.

use std::io::{self, ErrorKind};
use std::mem;
use std::os::fd::RawFd;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use mio::unix::SourceFd;
use mio::{Interest, Poll, Token};
use signal_hook_mio::v1_0::Signals;
use tagteam_provider::term::{bytes_waiting, read_some};

use crate::decode::{InternalEvent, Parser, TTY_BUFFER_SIZE};
use crate::detect::{self, Control as Typed};
use crate::msg::Msg;
use crate::replies::ReplyFilter;
use crate::term;

/// How long the thread rests after the terminal fails it, before it tries again.
const SLICE: Duration = Duration::from_millis(50);
/// How long `pause` and dropping wait for the thread to say so. A thread in its wait is woken
/// and says so at once: one that takes longer is held up somewhere between its look at the
/// control and its call into the terminal, or gone.
const ACK: Duration = Duration::from_secs(1);

/// What `Wait` registers its fd, its SIGWINCH and its waker under.
const FD: Token = Token(0);
const RESIZE: Token = Token(1);
const WAKE: Token = Token(2);

/// What woke the input thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ready {
    Input,
    Resized,
    Woken,
    Closed,
}

/// What one step of reading the terminal came to.
pub(crate) enum Next {
    Event(Event),
    /// The step read bytes that decoded to no event, or a signal cut its read short: the
    /// terminal may hold more.
    Pending,
    Empty,
    Closed,
}

/// Where terminal events come from: the process's terminal in the TUI, a script in the tests.
pub(crate) trait Events: Send + 'static {
    /// Blocks until the terminal has bytes (`Input`), SIGWINCH arrived (`Resized`), the
    /// waker was called or a signal interrupted the wait (`Woken`), or the terminal hung up
    /// (`Closed`).
    fn wait(&mut self) -> io::Result<Ready>;
    /// One bounded step, never blocking: at most one read of the terminal and at most one
    /// event. `Pending` when the step produced no event and the terminal may hold more, and
    /// `Empty` only once nothing waits in the terminal and the decoder holds no event. `Closed`
    /// once the terminal has hung up, and then nothing is read.
    fn next(&mut self) -> io::Result<Next>;
    /// The terminal's size now, for a resize.
    fn size(&mut self) -> io::Result<(u16, u16)>;
    /// Wakes the current or the next `wait`, from any thread.
    fn waker(&self) -> Arc<dyn Fn() + Send + Sync>;
}

/// The wait itself, on any fd, so tests can drive it with a socket pair.
pub(crate) struct Wait {
    poll: Poll,
    events: mio::Events,
    signals: Signals,
    waker: Arc<mio::Waker>,
    /// The fd has hung up. Its event comes once, so it is remembered.
    closed: bool,
    /// A SIGWINCH, bytes and a wake `closed` took, for the next `wait` to report: each event
    /// comes once.
    resized: bool,
    input: bool,
    woken: bool,
}

impl Wait {
    /// Waits on `fd`, on SIGWINCH and on the waker. The SIGWINCH registration is this one's
    /// own: a resize wakes the wait whatever the thread has read.
    pub(crate) fn on(fd: RawFd) -> io::Result<Wait> {
        let poll = Poll::new()?;
        let registry = poll.registry();
        registry.register(&mut SourceFd(&fd), FD, Interest::READABLE)?;
        let mut signals = Signals::new([libc::SIGWINCH])?;
        registry.register(&mut signals, RESIZE, Interest::READABLE)?;
        let waker = Arc::new(mio::Waker::new(registry, WAKE)?);
        Ok(Wait {
            poll,
            events: mio::Events::with_capacity(3),
            signals,
            waker,
            closed: false,
            resized: false,
            input: false,
            woken: false,
        })
    }

    /// `Events::wait` on `fd`. Of several at once, a hang-up is reported first, then a resize,
    /// after which the thread reads the terminal anyway, then bytes, then a wake.
    pub(crate) fn wait(&mut self) -> io::Result<Ready> {
        if !(self.closed || self.resized || self.input || self.woken) {
            match self.poll.poll(&mut self.events, None) {
                Ok(()) => self.take(),
                // A signal handled on this thread: the thread looks at its control again.
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        if self.closed {
            return Ok(Ready::Closed);
        }
        if mem::take(&mut self.resized) {
            self.signals.pending().for_each(drop);
            return Ok(Ready::Resized);
        }
        // A wake that comes with bytes is spent too: the thread looks at its control before
        // every call into the terminal.
        self.woken = false;
        if mem::take(&mut self.input) {
            Ok(Ready::Input)
        } else {
            Ok(Ready::Woken)
        }
    }

    /// Without blocking: whether `fd` has hung up. Once seen it stays true. Whatever else this
    /// poll takes, a SIGWINCH, bytes or a wake, is kept, and the next `wait` returns it at once.
    pub(crate) fn closed(&mut self) -> io::Result<bool> {
        if !self.closed {
            loop {
                match self.poll.poll(&mut self.events, Some(Duration::ZERO)) {
                    Ok(()) => break,
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
            self.take();
        }
        Ok(self.closed)
    }

    /// Wakes the current or the next `wait`, from any thread.
    pub(crate) fn waker(&self) -> Arc<dyn Fn() + Send + Sync> {
        let waker = Arc::clone(&self.waker);
        Arc::new(move || {
            // One that fails leaves the thread to its next event, and a pause that asked
            // says it was not acknowledged.
            let _ = waker.wake();
        })
    }

    /// Keeps what the events a poll took say: a hang-up, a SIGWINCH, bytes and a wake.
    fn take(&mut self) {
        for event in &self.events {
            match event.token() {
                FD if event.is_read_closed() || event.is_error() => self.closed = true,
                FD => self.input = true,
                RESIZE => self.resized = true,
                WAKE => self.woken = true,
                _ => {}
            }
        }
    }
}

/// A terminal's fd, fd 0 in the TUI, through `Wait`, read and decoded by the thread itself. Each
/// `next` is one bounded step (TUI input reader spec §3.2): a read of at most `TTY_BUFFER_SIZE`
/// bytes and no more than `FIONREAD` says the terminal holds, so it cannot block and the fd is
/// never made non-blocking. The bytes go through crossterm 0.29.0's decoder, vendored in
/// `decode`. The size comes from `crossterm::terminal::size()`.
pub(crate) struct Terminal {
    wait: Wait,
    fd: RawFd,
    decoder: Parser,
    buffer: Box<[u8; TTY_BUFFER_SIZE]>,
    size: fn() -> io::Result<(u16, u16)>,
}

impl Terminal {
    /// Reads `fd`, and asks `size` for the terminal's size after a resize.
    pub(crate) fn on(fd: RawFd, size: fn() -> io::Result<(u16, u16)>) -> io::Result<Terminal> {
        Ok(Terminal {
            wait: Wait::on(fd)?,
            fd,
            decoder: Parser::default(),
            buffer: Box::new([0; TTY_BUFFER_SIZE]),
            size,
        })
    }

    /// fd 0: `tagteam tui` runs only with stdin a terminal.
    fn stdin() -> io::Result<Terminal> {
        Terminal::on(libc::STDIN_FILENO, crossterm::terminal::size)
    }

    /// The first event the decoder holds. The replies to queries it holds are dropped: nothing
    /// in tagteam asks for one.
    fn take(&mut self) -> Option<Event> {
        self.decoder.find_map(|item| match item {
            InternalEvent::Event(event) => Some(event),
            InternalEvent::CursorPosition(..)
            | InternalEvent::KeyboardEnhancementFlags(_)
            | InternalEvent::PrimaryDeviceAttributes => None,
        })
    }
}

/// A terminal that has hung up reports `EIO` on its slave side.
fn hung_up(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::EIO)
}

impl Events for Terminal {
    fn wait(&mut self) -> io::Result<Ready> {
        self.wait.wait()
    }

    fn next(&mut self) -> io::Result<Next> {
        if let Some(event) = self.take() {
            return Ok(Next::Event(event));
        }
        if self.wait.closed()? {
            return Ok(Next::Closed);
        }
        let waiting = match bytes_waiting(self.fd) {
            Ok(0) => return Ok(Next::Empty),
            Ok(waiting) => waiting,
            Err(e) if hung_up(&e) => return Ok(Next::Closed),
            Err(e) => return Err(e),
        };
        // Only what is there: a read of more could block, whatever the terminal's VMIN.
        let read = match read_some(self.fd, &mut self.buffer[..waiting.min(TTY_BUFFER_SIZE)]) {
            Ok(0) => return Ok(Next::Closed),
            Ok(read) => read,
            Err(e) if hung_up(&e) => return Ok(Next::Closed),
            Err(e) if e.kind() == ErrorKind::Interrupted => return Ok(Next::Pending),
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(Next::Empty),
            Err(e) => return Err(e),
        };
        // crossterm's rule for whether more input follows: a read that filled the buffer.
        self.decoder
            .advance(&self.buffer[..read], read == TTY_BUFFER_SIZE);
        Ok(self.take().map_or(Next::Pending, Next::Event))
    }

    fn size(&mut self) -> io::Result<(u16, u16)> {
        (self.size)()
    }

    fn waker(&self) -> Arc<dyn Fn() + Send + Sync> {
        self.wait.waker()
    }
}

/// What the UI thread asks of the input thread (§7.3 step 1).
pub(crate) trait Pause {
    /// True once the thread reads no more: it is between two calls into the terminal, and
    /// makes the next only after `resume`. False when it has not said so within `ACK`: it may
    /// still read keys meant for the terminal's next owner, so the terminal must not be handed
    /// over. The pause stays asked for until `resume`.
    fn pause(&self) -> bool;
    fn resume(&self);
    /// The terminal has hung up and is gone: the thread is asked to stop, and nothing waits for
    /// it: a terminal that is gone has no next owner for a late read to take keys from, and an
    /// acknowledgment is not worth waiting for.
    fn hang_up(&self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Reading,
    PauseAsked,
    Paused,
    StopAsked,
    /// A stop asked for after a hang-up: nothing waits for it, now or on drop.
    HungUp,
    Stopped,
}

struct Control {
    mode: Mutex<Mode>,
    changed: Condvar,
}

impl Control {
    fn lock(&self) -> MutexGuard<'_, Mode> {
        self.mode.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set(&self, mode: &mut MutexGuard<'_, Mode>, to: Mode) {
        **mode = to;
        self.changed.notify_all();
    }

    /// Waits while the mode is `from`, for at most `ACK`. False when it still is.
    fn wait_while(&self, mut mode: MutexGuard<'_, Mode>, from: Mode) -> bool {
        let deadline = Instant::now() + ACK;
        while *mode == from {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            mode = self
                .changed
                .wait_timeout(mode, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

/// The input thread's handle. Dropping it stops the thread once the thread has said so; the
/// thread is never joined (§10.2).
pub(crate) struct Input {
    control: Arc<Control>,
    /// Wakes the thread from its wait, to look at the control.
    wake: Arc<dyn Fn() + Send + Sync>,
}

/// How the input thread asks for the background (spec §4.2): `ask` writes the query, and the
/// replies come on `fd`.
pub(crate) struct Detection {
    pub(crate) ask: Box<dyn FnOnce() -> bool + Send>,
    pub(crate) fd: RawFd,
}

impl Input {
    /// With `detect`, the thread asks the terminal its background before its first read, unless
    /// the terminal is not the one this process controls in the foreground (§4.2).
    pub(crate) fn spawn(tx: Sender<Msg>, detect: bool) -> io::Result<Input> {
        let detection = (detect && tagteam_provider::term::stdin_is_our_foreground_terminal())
            .then(|| Detection {
                ask: Box::new(|| term::query(detect::QUERY)),
                fd: libc::STDIN_FILENO,
            });
        Input::spawn_with(tx, Terminal::stdin()?, detection)
    }

    pub(crate) fn spawn_with(
        tx: Sender<Msg>,
        events: impl Events,
        detection: Option<Detection>,
    ) -> io::Result<Input> {
        let control = Arc::new(Control {
            mode: Mutex::new(Mode::Reading),
            changed: Condvar::new(),
        });
        let wake = events.waker();
        let theirs = Arc::clone(&control);
        thread::Builder::new()
            .name("tagteam-tui-input".into())
            .spawn(move || read(&theirs, events, &tx, detection))?;
        Ok(Input { control, wake })
    }

    /// Asks the thread for mode `to`, and wakes it from its wait to look.
    fn ask(&self, mode: &mut MutexGuard<'_, Mode>, to: Mode) {
        self.control.set(mode, to);
        (self.wake)();
    }
}

/// Asks the terminal its background, reads what it says before the thread's first read, and
/// sends it as `Detected` (§4.2). A Ctrl-C or Ctrl-Z that ended the reading is sent after it, as
/// the key the decoder would have made. The filter for the keys that follow is calm when no query
/// was asked or its DA1 answer was read, and otherwise wary, from where the parser stood however
/// the reading ended (a quiet second, the limit, a stop or pause, Ctrl-C or Ctrl-Z): detection is
/// not started again. `None` once the UI thread is gone.
fn ask_background(
    control: &Control,
    tx: &Sender<Msg>,
    detection: Detection,
) -> Option<ReplyFilter> {
    let Detection { ask, fd } = detection;
    let stop = || *control.lock() != Mode::Reading;
    let replies = ask().then(|| detect::read(fd, Instant::now(), &stop));
    let background = replies.as_ref().and_then(detect::Replies::background);
    tx.send(Msg::Detected { background }).ok()?;
    let filter = match &replies {
        Some(replies) if !replies.answered() => ReplyFilter::wary(replies.handoff()),
        _ => ReplyFilter::calm(),
    };
    let key = match replies.and_then(|replies| replies.control()) {
        Some(Typed::Interrupt) => 'c',
        Some(Typed::Suspend) => 'z',
        None => return Some(filter),
    };
    let key = KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL);
    tx.send(Msg::Key(key)).ok()?;
    Some(filter)
}

/// The keys' way to the UI thread: through the reply filter, which tells the UI thread once when
/// it starts to drop every key and once when it stops (§4.2).
struct Keys {
    filter: ReplyFilter,
    /// `KeysHeld { held: true }` was sent and `false` has not been.
    told: bool,
}

impl Keys {
    /// Sends what `event` is to the UI thread. False once it is gone.
    fn send(&mut self, event: Event, tx: &Sender<Msg>) -> bool {
        match message(event) {
            Some(Msg::Key(key)) => {
                let pass = self.filter.key(key);
                if self.told && !self.filter.holding() {
                    self.told = false;
                    if tx.send(Msg::KeysHeld { held: false }).is_err() {
                        return false;
                    }
                }
                pass.into_iter().all(|key| tx.send(Msg::Key(key)).is_ok())
            }
            Some(msg) => tx.send(msg).is_ok(),
            None => true,
        }
    }

    /// A drain has ended: the keys the terminal holds are all handled. False once the UI thread
    /// is gone.
    fn drained(&mut self, tx: &Sender<Msg>) -> bool {
        if self.filter.holding() && !self.told {
            self.told = true;
            return tx.send(Msg::KeysHeld { held: true }).is_ok();
        }
        true
    }
}

/// The thread: reads while the mode is `Reading`, and between two calls into the terminal
/// acknowledges a pause or a stop.
fn read(
    control: &Control,
    mut events: impl Events,
    tx: &Sender<Msg>,
    detection: Option<Detection>,
) {
    let filter = match detection {
        Some(detection) => match ask_background(control, tx, detection) {
            Some(filter) => filter,
            None => {
                control.set(&mut control.lock(), Mode::Stopped);
                return;
            }
        },
        None => ReplyFilter::calm(),
    };
    let mut keys = Keys {
        filter,
        told: false,
    };
    // Bytes may already wait in the terminal: one step looks before the first wait, at no cost.
    let mut more = true;
    loop {
        {
            let mut mode = control.lock();
            loop {
                let now = *mode;
                match now {
                    Mode::Reading => break,
                    Mode::PauseAsked => control.set(&mut mode, Mode::Paused),
                    Mode::Paused => {
                        mode = control
                            .changed
                            .wait(mode)
                            .unwrap_or_else(PoisonError::into_inner);
                        // A pause can come between two events the decoder holds.
                        more = true;
                    }
                    Mode::StopAsked | Mode::HungUp | Mode::Stopped => {
                        control.set(&mut mode, Mode::Stopped);
                        return;
                    }
                }
            }
        }
        if !step(&mut events, tx, &mut more, &mut keys) {
            control.set(&mut control.lock(), Mode::Stopped);
            return;
        }
    }
}

/// One call into the terminal, so the mode is looked at between any two: one `next` step while
/// there may be `more`, else a wait. False once the thread is to stop:
/// the terminal has hung up, or the UI thread is gone and nothing reads these any more.
fn step(events: &mut impl Events, tx: &Sender<Msg>, more: &mut bool, keys: &mut Keys) -> bool {
    if *more {
        return match events.next() {
            Ok(Next::Event(event)) => keys.send(event, tx),
            // Bytes read and no event yet: the terminal may hold more, and `more` stays set.
            Ok(Next::Pending) => true,
            Ok(Next::Empty) => {
                *more = false;
                keys.drained(tx)
            }
            Ok(Next::Closed) => false,
            // A terminal that cannot be read now: a hang-up arrives as SIGHUP. No spinning.
            Err(_) => {
                *more = false;
                thread::sleep(SLICE);
                true
            }
        };
    }
    match events.wait() {
        Ok(Ready::Input) => {
            *more = true;
            true
        }
        // The size is read after the thread's own SIGWINCH (R-8).
        Ok(Ready::Resized) => {
            *more = true;
            events
                .size()
                .ok()
                .is_none_or(|(width, height)| tx.send(Msg::Resize { width, height }).is_ok())
        }
        Ok(Ready::Woken) => true,
        Ok(Ready::Closed) => false,
        Err(_) => {
            thread::sleep(SLICE);
            true
        }
    }
}

/// The events the TUI acts on: key presses (and repeats). Resizes come from the thread's own
/// SIGWINCH, and a `Resize` event is dropped (R-8).
fn message(event: Event) -> Option<Msg> {
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => Some(Msg::Key(key)),
        _ => None,
    }
}

impl Pause for Input {
    fn pause(&self) -> bool {
        let mut mode = self.control.lock();
        if *mode == Mode::Reading {
            self.ask(&mut mode, Mode::PauseAsked);
        }
        self.control.wait_while(mode, Mode::PauseAsked)
    }

    fn resume(&self) {
        let mut mode = self.control.lock();
        if matches!(*mode, Mode::PauseAsked | Mode::Paused) {
            self.ask(&mut mode, Mode::Reading);
        }
    }

    fn hang_up(&self) {
        let mut mode = self.control.lock();
        if *mode != Mode::Stopped {
            self.ask(&mut mode, Mode::HungUp);
        }
    }
}

impl Drop for Input {
    /// Stops the thread before the terminal is given back, so it never reads the shell's
    /// input. The TUI is ending, so nothing waits beyond `ACK`: a thread held up longer stops
    /// at its next look. After a hang-up there is no terminal to read, and nothing waits.
    fn drop(&mut self) {
        let mut mode = self.control.lock();
        if matches!(*mode, Mode::HungUp | Mode::Stopped) {
            return;
        }
        self.ask(&mut mode, Mode::StopAsked);
        let _ = self.control.wait_while(mode, Mode::StopAsked);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs::File;
    use std::io::Write;
    use std::os::fd::{AsRawFd, OwnedFd};
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tagteam_provider::term::{open_raw_pty, status_flags};

    use super::*;

    /// What a script holds for the thread.
    #[derive(Default)]
    struct Held {
        /// What `next` returns, in order, and `Empty` once it runs out. After a `Closed`, or a
        /// `hang_up`, `next` and `wait` both return `Closed` for good.
        queue: VecDeque<Next>,
        closed: bool,
        /// A SIGWINCH no `wait` has reported yet, and the size `size` reports.
        resized: bool,
        size: (u16, u16),
        /// The waker was called since a `wait` last reported it.
        woken: bool,
        /// Every `next` fails, or every `wait` does.
        next_fails: bool,
        wait_fails: bool,
    }

    /// A terminal in a script. Its `wait` blocks until something is queued, a resize arrives,
    /// the waker is called or the terminal hangs up. `reading` is set while the thread is
    /// inside `next` or `wait`, and `nexts` and `waits` count its calls.
    #[derive(Clone, Default)]
    struct Script {
        held: Arc<(Mutex<Held>, Condvar)>,
        reading: Arc<AtomicBool>,
        nexts: Arc<AtomicUsize>,
        waits: Arc<AtomicUsize>,
    }

    impl Script {
        fn hold(&self) -> MutexGuard<'_, Held> {
            self.held.0.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// Queues `items` all at once, as one read of the terminal brings them.
        fn queue(&self, items: impl IntoIterator<Item = Next>) {
            self.hold().queue.extend(items);
            self.held.1.notify_all();
        }

        fn push(&self, event: Event) {
            self.queue([Next::Event(event)]);
        }

        /// A SIGWINCH, the terminal now `width`×`height`.
        fn resize(&self, width: u16, height: u16) {
            let mut held = self.hold();
            held.size = (width, height);
            held.resized = true;
            drop(held);
            self.held.1.notify_all();
        }

        /// The terminal hangs up, seen by the `wait` the thread may be in.
        fn hang_up(&self) {
            self.hold().closed = true;
            self.held.1.notify_all();
        }

        /// From now on every `next` fails.
        fn fail_next(&self) {
            self.hold().next_fails = true;
        }

        /// From now on every `wait` fails.
        fn fail_wait(&self) {
            self.hold().wait_fails = true;
            self.held.1.notify_all();
        }

        /// Its `next`s and `wait`s so far.
        fn calls(&self) -> usize {
            self.nexts.load(Ordering::SeqCst) + self.waits.load(Ordering::SeqCst)
        }

        /// Returns once the thread is inside its `n`th `wait`.
        fn idle(&self, n: usize) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.waits.load(Ordering::SeqCst) != n || !self.reading.load(Ordering::SeqCst) {
                assert!(Instant::now() < deadline, "the thread never waited");
                thread::yield_now();
            }
        }
    }

    impl Events for Script {
        fn wait(&mut self) -> io::Result<Ready> {
            self.waits.fetch_add(1, Ordering::SeqCst);
            self.reading.store(true, Ordering::SeqCst);
            let mut held = self.hold();
            let ready = loop {
                if held.wait_fails {
                    break Err(io::Error::other("the wait fails"));
                }
                if held.closed {
                    break Ok(Ready::Closed);
                }
                if mem::take(&mut held.resized) {
                    break Ok(Ready::Resized);
                }
                if !held.queue.is_empty() {
                    break Ok(Ready::Input);
                }
                if mem::take(&mut held.woken) {
                    break Ok(Ready::Woken);
                }
                held = self
                    .held
                    .1
                    .wait(held)
                    .unwrap_or_else(PoisonError::into_inner);
            };
            drop(held);
            self.reading.store(false, Ordering::SeqCst);
            ready
        }

        fn next(&mut self) -> io::Result<Next> {
            self.nexts.fetch_add(1, Ordering::SeqCst);
            self.reading.store(true, Ordering::SeqCst);
            let mut held = self.hold();
            let next = if held.next_fails {
                Err(io::Error::other("the read fails"))
            } else if held.closed {
                Ok(Next::Closed)
            } else {
                Ok(held.queue.pop_front().unwrap_or(Next::Empty))
            };
            held.closed |= matches!(next, Ok(Next::Closed));
            drop(held);
            self.reading.store(false, Ordering::SeqCst);
            next
        }

        fn size(&mut self) -> io::Result<(u16, u16)> {
            Ok(self.hold().size)
        }

        fn waker(&self) -> Arc<dyn Fn() + Send + Sync> {
            let held = Arc::clone(&self.held);
            Arc::new(move || {
                held.0.lock().unwrap_or_else(PoisonError::into_inner).woken = true;
                held.1.notify_all();
            })
        }
    }

    /// A terminal whose every call waits at a gate first: the thread is past its look at the
    /// control and has not called into the terminal yet, as if it were descheduled there. Once
    /// opened, it is `Script`.
    #[derive(Clone, Default)]
    struct Gate {
        script: Script,
        open: Arc<(Mutex<bool>, Condvar)>,
        waiting: Arc<AtomicBool>,
    }

    impl Gate {
        fn open(&self) {
            *self.open.0.lock().unwrap() = true;
            self.open.1.notify_all();
        }

        fn pass(&self) {
            self.waiting.store(true, Ordering::SeqCst);
            let (open, opened) = &*self.open;
            let mut open = open.lock().unwrap();
            while !*open {
                open = opened.wait(open).unwrap();
            }
        }
    }

    impl Events for Gate {
        fn wait(&mut self) -> io::Result<Ready> {
            self.pass();
            self.script.wait()
        }

        fn next(&mut self) -> io::Result<Next> {
            self.pass();
            self.script.next()
        }

        fn size(&mut self) -> io::Result<(u16, u16)> {
            self.script.size()
        }

        fn waker(&self) -> Arc<dyn Fn() + Send + Sync> {
            self.script.waker()
        }
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn key_presses_and_resizes_become_messages_and_nothing_else_does() {
        let script = Script::default();
        let mut release = key('x');
        release.kind = KeyEventKind::Release;
        for event in [
            Event::Key(release),
            Event::FocusGained,
            Event::Key(key('q')),
            // Not one the decoder makes, but dropped all the same: the SIGWINCH below makes the
            // resize (R-8).
            Event::Resize(80, 24),
        ] {
            script.push(event);
        }
        script.resize(100, 30);
        let (tx, rx) = mpsc::channel();
        let _input = Input::spawn_with(tx, script.clone(), None).unwrap();
        let within = Duration::from_secs(2);
        assert!(matches!(
            rx.recv_timeout(within),
            Ok(Msg::Key(k)) if k.code == KeyCode::Char('q')
        ));
        assert!(matches!(
            rx.recv_timeout(within),
            Ok(Msg::Resize {
                width: 100,
                height: 30
            })
        ));
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn a_pause_returns_once_the_thread_reads_no_more_and_resume_reads_again() {
        let script = Script::default();
        let (tx, rx) = mpsc::channel();
        let input = Input::spawn_with(tx, script.clone(), None).unwrap();
        thread::sleep(Duration::from_millis(30));
        assert!(input.pause());
        assert!(!script.reading.load(Ordering::SeqCst), "between two calls");
        let calls = script.calls();
        script.push(Event::Key(key('x')));
        thread::sleep(Duration::from_millis(200));
        assert_eq!(script.calls(), calls, "no call while paused");
        assert!(rx.try_recv().is_err(), "nothing read while paused");
        input.resume();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)),
            Ok(Msg::Key(k)) if k.code == KeyCode::Char('x')
        ));
    }

    #[test]
    fn a_pause_the_thread_does_not_acknowledge_says_so() {
        // The thread has looked at the control (`Reading`) and is held up before its call. A
        // pause that returned anyway would hand the terminal over while it can still read.
        let gate = Gate::default();
        gate.script.push(Event::Key(key('x')));
        let (tx, rx) = mpsc::channel();
        let input = Input::spawn_with(tx, gate.clone(), None).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !gate.waiting.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "the thread never reached its call"
            );
            thread::yield_now();
        }
        let started = Instant::now();
        assert!(!input.pause(), "not acknowledged");
        assert!(started.elapsed() >= ACK);
        // What a hand-over would have lost: the thread reads the next owner's key.
        gate.open();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)),
            Ok(Msg::Key(k)) if k.code == KeyCode::Char('x')
        ));
        // Back at its look, it acknowledges the pause still asked for.
        assert!(input.pause());
        input.resume();
    }

    #[test]
    fn dropping_the_input_stops_its_thread_before_it_returns() {
        let script = Script::default();
        let (tx, _rx) = mpsc::channel();
        let input = Input::spawn_with(tx, script.clone(), None).unwrap();
        thread::sleep(Duration::from_millis(30));
        drop(input);
        let calls = script.calls();
        thread::sleep(Duration::from_millis(200));
        assert_eq!(script.calls(), calls);
        assert!(!script.reading.load(Ordering::SeqCst));
    }

    /// A thread held up before its call, as a call into the terminal that does not return.
    fn stuck_input() -> (Input, Gate) {
        let gate = Gate::default();
        let (tx, _rx) = mpsc::channel();
        let input = Input::spawn_with(tx, gate.clone(), None).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !gate.waiting.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "the thread never reached its call"
            );
            thread::yield_now();
        }
        (input, gate)
    }

    #[test]
    fn a_hang_up_waits_for_no_acknowledgment_and_neither_does_the_drop_after_it() {
        let (input, gate) = stuck_input();
        let started = Instant::now();
        input.hang_up();
        drop(input);
        assert!(
            started.elapsed() < ACK / 4,
            "no acknowledgment was waited for: {:?}",
            started.elapsed()
        );
        gate.open();
    }

    #[test]
    fn dropping_a_thread_held_up_waits_for_it_up_to_the_acknowledgment() {
        let (input, gate) = stuck_input();
        let started = Instant::now();
        drop(input);
        assert!(started.elapsed() >= ACK);
        gate.open();
    }

    #[test]
    fn with_the_ui_gone_the_thread_stops_and_a_pause_never_hangs() {
        let script = Script::default();
        let (tx, rx) = mpsc::channel();
        let input = Input::spawn_with(tx, script.clone(), None).unwrap();
        drop(rx);
        script.push(Event::Key(key('q')));
        thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        assert!(input.pause(), "a thread that has stopped reads no more");
        input.resume();
        drop(input);
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn an_idle_thread_waits_without_waking() {
        let script = Script::default();
        let (tx, _rx) = mpsc::channel();
        let _input = Input::spawn_with(tx, script.clone(), None).unwrap();
        thread::sleep(Duration::from_millis(300));
        assert_eq!(
            script.nexts.load(Ordering::SeqCst),
            1,
            "the drain at startup"
        );
        assert_eq!(
            script.waits.load(Ordering::SeqCst),
            1,
            "one wait, not yet over"
        );
    }

    #[test]
    fn a_pause_is_acknowledged_within_50_ms_while_idle() {
        let script = Script::default();
        let (tx, _rx) = mpsc::channel();
        let input = Input::spawn_with(tx, script.clone(), None).unwrap();
        script.idle(1);
        let started = Instant::now();
        assert!(input.pause());
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "{:?}",
            started.elapsed()
        );
        input.resume();
    }

    /// The thread has stopped at its `nexts`th `next` and `waits`th `wait`: it makes no call
    /// after them, paused or resumed, and a pause is acknowledged at once.
    fn assert_stopped(input: &Input, script: &Script, nexts: usize, waits: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while script.nexts.load(Ordering::SeqCst) < nexts {
            assert!(Instant::now() < deadline, "the thread never got there");
            thread::yield_now();
        }
        let counts = || {
            (
                script.nexts.load(Ordering::SeqCst),
                script.waits.load(Ordering::SeqCst),
            )
        };
        thread::sleep(Duration::from_millis(200));
        assert_eq!(counts(), (nexts, waits), "no call after the hang-up");
        let started = Instant::now();
        assert!(input.pause(), "a thread that has stopped reads no more");
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "{:?}",
            started.elapsed()
        );
        input.resume();
        thread::sleep(Duration::from_millis(100));
        assert_eq!(counts(), (nexts, waits), "none after a resume either");
    }

    #[test]
    fn a_closed_terminal_stops_the_thread_wherever_it_shows() {
        // The drain at startup.
        let script = Script::default();
        script.queue([Next::Closed]);
        let (tx, _rx) = mpsc::channel();
        let input = Input::spawn_with(tx, script.clone(), None).unwrap();
        assert_stopped(&input, &script, 1, 0);

        // The drain after a resume.
        let script = Script::default();
        let (tx, _rx) = mpsc::channel();
        let input = Input::spawn_with(tx, script.clone(), None).unwrap();
        script.idle(1);
        assert!(input.pause());
        script.queue([Next::Closed]);
        input.resume();
        assert_stopped(&input, &script, 2, 1);

        // A drain after a wait, two events in.
        let script = Script::default();
        let (tx, rx) = mpsc::channel();
        let input = Input::spawn_with(tx, script.clone(), None).unwrap();
        script.idle(1);
        script.queue([
            Next::Event(Event::Key(key('a'))),
            Next::Event(Event::Key(key('b'))),
            Next::Closed,
        ]);
        for c in ['a', 'b'] {
            assert!(matches!(
                rx.recv_timeout(Duration::from_secs(2)),
                Ok(Msg::Key(k)) if k.code == KeyCode::Char(c)
            ));
        }
        assert_stopped(&input, &script, 4, 1);

        // The wait, idle.
        let script = Script::default();
        let (tx, _rx) = mpsc::channel();
        let input = Input::spawn_with(tx, script.clone(), None).unwrap();
        script.idle(1);
        script.hang_up();
        assert_stopped(&input, &script, 1, 1);
    }

    /// The failing call `calls` counts is tried again, a slice after the last try at the
    /// soonest: since `started`, no more tries than slices, with two to spare. The bound is the
    /// clock's, so a late runner cannot break it. With no back-off the tries come by the
    /// hundred thousand.
    fn assert_tried_a_slice_apart(calls: &AtomicUsize, started: Instant, what: &str) {
        let deadline = started + Duration::from_secs(5);
        while calls.load(Ordering::SeqCst) < 2 {
            assert!(Instant::now() < deadline, "{what}: never tried again");
            thread::yield_now();
        }
        thread::sleep(Duration::from_millis(200));
        let tries = calls.load(Ordering::SeqCst) as u128;
        let elapsed = started.elapsed();
        let most = elapsed.as_millis() / SLICE.as_millis() + 2;
        assert!(
            tries <= most,
            "{what}: {tries} in {elapsed:?}, at most {most}"
        );
    }

    #[test]
    fn a_terminal_that_fails_is_tried_again_a_slice_later() {
        // Reading fails, while the terminal says it has bytes.
        let script = Script::default();
        script.fail_next();
        script.push(Event::Key(key('x')));
        let (tx, _rx) = mpsc::channel();
        let started = Instant::now();
        let input = Input::spawn_with(tx, script.clone(), None).unwrap();
        assert_tried_a_slice_apart(&script.nexts, started, "reads");
        drop(input);

        // The wait fails.
        let script = Script::default();
        script.fail_wait();
        let (tx, _rx) = mpsc::channel();
        let started = Instant::now();
        let _input = Input::spawn_with(tx, script.clone(), None).unwrap();
        assert_tried_a_slice_apart(&script.waits, started, "waits");
        assert_eq!(
            script.nexts.load(Ordering::SeqCst),
            1,
            "the drain at startup"
        );
    }

    #[test]
    fn a_resize_is_sent_from_the_thread_s_own_signal() {
        // The thread's own SIGWINCH woke it: no event from the terminal is needed.
        let script = Script::default();
        let (tx, rx) = mpsc::channel();
        let _input = Input::spawn_with(tx, script.clone(), None).unwrap();
        script.idle(1);
        script.resize(100, 30);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)),
            Ok(Msg::Resize {
                width: 100,
                height: 30
            })
        ));
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    }

    /// A detection over a socket pair: `ask` records that it ran and says the query was
    /// written. The peer's end is what the terminal answers on; `ours` must outlive the thread.
    fn detection() -> (Detection, UnixStream, UnixStream, Arc<AtomicBool>) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let asked = Arc::new(AtomicBool::new(false));
        let detection = Detection {
            ask: Box::new({
                let asked = Arc::clone(&asked);
                move || {
                    asked.store(true, Ordering::SeqCst);
                    true
                }
            }),
            fd: ours.as_raw_fd(),
        };
        (detection, ours, theirs, asked)
    }

    #[test]
    fn detection_runs_before_the_first_terminal_read() {
        let script = Script::default();
        let (detection, _ours, mut theirs, asked) = detection();
        let (tx, rx) = mpsc::channel();
        let _input = Input::spawn_with(tx, script.clone(), Some(detection)).unwrap();
        script.push(Event::Key(key('q')));
        thread::sleep(Duration::from_millis(200));
        assert!(asked.load(Ordering::SeqCst), "the query was written");
        assert_eq!(script.calls(), 0, "no call into the terminal before DA1");
        assert!(rx.try_recv().is_err(), "nothing sent before DA1");
        theirs.write_all(b"\x1b[?62;22c").unwrap();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)),
            Ok(Msg::Detected { background: None })
        ));
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)),
            Ok(Msg::Key(k)) if k.code == KeyCode::Char('q')
        ));
    }

    #[test]
    fn a_ctrl_c_during_detection_reaches_update_right_after_it() {
        let gate = Gate::default();
        let (detection, _ours, mut theirs, _asked) = detection();
        let (tx, rx) = mpsc::channel();
        let input = Input::spawn_with(tx, gate.clone(), Some(detection)).unwrap();
        thread::sleep(Duration::from_millis(100));
        theirs.write_all(b"\x03").unwrap();
        let within = Duration::from_secs(2);
        assert!(matches!(
            rx.recv_timeout(within),
            Ok(Msg::Detected { background: None })
        ));
        assert!(matches!(
            rx.recv_timeout(within),
            Ok(Msg::Key(k)) if k.code == KeyCode::Char('c') && k.modifiers == KeyModifiers::CONTROL
        ));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !gate.waiting.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "the thread never read on");
            thread::yield_now();
        }
        assert_eq!(
            gate.script.calls(),
            0,
            "the keys came before the first call"
        );
        gate.open();
        drop(input);
    }

    #[test]
    fn a_pause_during_detection_is_acknowledged_within_a_slice() {
        let script = Script::default();
        let (detection, _ours, _theirs, _asked) = detection();
        let (tx, _rx) = mpsc::channel();
        let spawned = Instant::now();
        let input = Input::spawn_with(tx, script.clone(), Some(detection)).unwrap();
        thread::sleep(Duration::from_millis(100));
        assert!(input.pause());
        // Acknowledged by the pause, not by detection's quiet second running out.
        assert!(
            spawned.elapsed() < detect::BEFORE_QUIET,
            "{:?}",
            spawned.elapsed()
        );
        input.resume();
    }

    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    fn ctrl_g() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)
    }

    #[test]
    fn a_held_reply_is_told_once_and_its_end_once() {
        let script = Script::default();
        let (detection, _ours, mut theirs, _asked) = detection();
        // The terminal began its answer and said no more: detection hands off in an OSC.
        theirs.write_all(b"\x1b]11;rgb:").unwrap();
        script.queue([Next::Event(Event::Key(key('j'))), Next::Empty]);
        let (tx, rx) = mpsc::channel();
        let _input = Input::spawn_with(tx, script.clone(), Some(detection)).unwrap();
        let within = Duration::from_secs(3);
        assert!(matches!(
            rx.recv_timeout(within),
            Ok(Msg::Detected { background: None })
        ));
        assert!(matches!(
            rx.recv_timeout(within),
            Ok(Msg::KeysHeld { held: true })
        ));
        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "neither `j` nor a second telling"
        );
        script.queue([
            Next::Event(Event::Key(ctrl_g())),
            Next::Event(Event::Key(key('k'))),
        ]);
        assert!(matches!(
            rx.recv_timeout(within),
            Ok(Msg::KeysHeld { held: false })
        ));
        assert!(matches!(
            rx.recv_timeout(within),
            Ok(Msg::Key(k)) if k.code == KeyCode::Char('k')
        ));
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[test]
    fn after_an_answered_query_esc_is_sent_at_once() {
        let script = Script::default();
        let (detection, _ours, mut theirs, _asked) = detection();
        theirs.write_all(b"\x1b[?62;22c").unwrap();
        script.queue([Next::Event(Event::Key(esc())), Next::Empty]);
        let (tx, rx) = mpsc::channel();
        let _input = Input::spawn_with(tx, script.clone(), Some(detection)).unwrap();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)),
            Ok(Msg::Detected { background: None })
        ));
        assert!(matches!(
            rx.recv_timeout(detect::BEFORE_QUIET),
            Ok(Msg::Key(k)) if k.code == KeyCode::Esc
        ));
    }

    #[test]
    fn a_pause_that_ended_detection_leaves_the_filter_wary() {
        // The query was asked and no DA1 answer was read: after the pause, detection does not
        // start again, and a lone Esc waits for the next key.
        let script = Script::default();
        let (detection, _ours, _theirs, _asked) = detection();
        let (tx, rx) = mpsc::channel();
        let input = Input::spawn_with(tx, script.clone(), Some(detection)).unwrap();
        thread::sleep(Duration::from_millis(100));
        assert!(input.pause());
        script.queue([Next::Event(Event::Key(esc()))]);
        input.resume();
        let within = Duration::from_secs(2);
        assert!(matches!(
            rx.recv_timeout(within),
            Ok(Msg::Detected { background: None })
        ));
        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "the Esc is held"
        );
        script.push(Event::Key(key('j')));
        assert!(matches!(
            rx.recv_timeout(within),
            Ok(Msg::Key(k)) if k.code == KeyCode::Esc
        ));
        assert!(matches!(
            rx.recv_timeout(within),
            Ok(Msg::Key(k)) if k.code == KeyCode::Char('j')
        ));
    }

    /// Held by each test that makes a `Wait`. A SIGWINCH reaches every `Wait` in the process,
    /// so one test's would end another's wait.
    static WAITS: Mutex<()> = Mutex::new(());

    fn alone() -> MutexGuard<'static, ()> {
        WAITS.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[test]
    fn the_wait_blocks_until_bytes_its_waker_or_a_resize() {
        let _alone = alone();
        let (ours, mut theirs) = UnixStream::pair().unwrap();
        let mut wait = Wait::on(ours.as_raw_fd()).unwrap();
        let wake = wait.waker();
        let (tx, rx) = mpsc::channel();
        let waiting = thread::spawn(move || {
            for _ in 0..3 {
                tx.send(wait.wait().unwrap()).unwrap();
            }
        });
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "nothing has happened"
        );
        wake();
        assert_eq!(rx.recv_timeout(Duration::from_millis(50)), Ok(Ready::Woken));
        signal_hook::low_level::raise(libc::SIGWINCH).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(Ready::Resized));
        theirs.write_all(b"x").unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(Ready::Input));
        waiting.join().unwrap();
        drop(ours);
    }

    #[test]
    fn a_closed_terminal_is_seen_without_blocking() {
        let _alone = alone();
        let (ours, theirs) = UnixStream::pair().unwrap();
        let mut wait = Wait::on(ours.as_raw_fd()).unwrap();
        drop(theirs);
        let started = Instant::now();
        assert!(wait.closed().unwrap());
        assert!(started.elapsed() < Duration::from_millis(50));
        assert!(wait.closed().unwrap(), "once seen, it stays");
        // The poll above took the hang-up: `wait` returns from what it remembers.
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(wait.wait().unwrap());
            drop(ours);
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(Ready::Closed));
    }

    #[test]
    fn a_resize_the_hang_up_check_takes_is_kept_for_the_wait() {
        let _alone = alone();
        let (ours, _theirs) = UnixStream::pair().unwrap();
        let mut wait = Wait::on(ours.as_raw_fd()).unwrap();
        signal_hook::low_level::raise(libc::SIGWINCH).unwrap();
        assert!(!wait.closed().unwrap());
        // The poll above took the SIGWINCH: `wait` returns from what it kept.
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(wait.wait().unwrap());
            drop(ours);
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(Ready::Resized));
    }

    #[test]
    fn a_wake_the_hang_up_check_takes_is_kept_for_the_wait() {
        let _alone = alone();
        let (ours, _theirs) = UnixStream::pair().unwrap();
        let mut wait = Wait::on(ours.as_raw_fd()).unwrap();
        wait.waker()();
        assert!(!wait.closed().unwrap());
        // The poll above took the wake: `wait` returns from what it kept.
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(wait.wait().unwrap());
            drop(ours);
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(Ready::Woken));
    }

    #[test]
    fn bytes_the_hang_up_check_takes_are_kept_for_the_wait() {
        let _alone = alone();
        let (ours, mut theirs) = UnixStream::pair().unwrap();
        let mut wait = Wait::on(ours.as_raw_fd()).unwrap();
        theirs.write_all(b"x").unwrap();
        assert!(!wait.closed().unwrap());
        // The poll above took the bytes' event, and nothing read them: `wait` returns from
        // what it kept, once.
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for _ in 0..2 {
                if tx.send(wait.wait().unwrap()).is_err() {
                    break;
                }
            }
            drop(ours);
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(Ready::Input));
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "the byte is still unread and nothing else came"
        );
        // The hang-up ends the second wait.
        drop(theirs);
    }

    /// The in-process pty the reader tests use: the non-blocking master as a file, and the raw
    /// slave `Terminal::on` reads.
    fn pty() -> (File, OwnedFd) {
        let (master, slave) = open_raw_pty().unwrap();
        (File::from(master), slave)
    }

    fn terminal_on(slave: &OwnedFd) -> Terminal {
        Terminal::on(slave.as_raw_fd(), || Ok((100, 30))).unwrap()
    }

    /// Returns once the slave holds `n` bytes unread: bytes reach a Linux slave asynchronously.
    fn landed(slave: &OwnedFd, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while bytes_waiting(slave.as_raw_fd()).unwrap() != n {
            assert!(Instant::now() < deadline, "{n} bytes never landed");
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Writes all of `bytes` to the master, keeping its offset across partial writes and trying
    /// a full master again every millisecond. False only once `abort` is set, for the clean-up of
    /// a test after which no key is expected. No write blocks, so every writer can be joined.
    fn write_whole(mut master: &File, bytes: &[u8], abort: &AtomicBool) -> bool {
        let mut done = 0;
        while done < bytes.len() {
            match master.write(&bytes[done..]) {
                Ok(n) => done += n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if abort.load(Ordering::SeqCst) {
                        return false;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => panic!("the master write failed: {e}"),
            }
        }
        true
    }

    /// Writes as much of `bytes` as the master takes, and how much that was.
    fn write_what_fits(mut master: &File, bytes: &[u8]) -> usize {
        let mut done = 0;
        while done < bytes.len() {
            match master.write(&bytes[done..]) {
                Ok(n) => done += n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => panic!("the master write failed: {e}"),
            }
        }
        done
    }

    #[cfg(target_os = "linux")]
    fn is_key(next: &Next, c: char) -> bool {
        matches!(next, Next::Event(Event::Key(k)) if k.code == KeyCode::Char(c))
    }

    #[test]
    fn a_key_split_across_two_reads_is_one_key() {
        let _alone = alone();
        let (master, slave) = pty();
        let mut terminal = terminal_on(&slave);
        write_whole(&master, b"\x1b[", &AtomicBool::new(false));
        landed(&slave, 2);
        assert!(matches!(terminal.next().unwrap(), Next::Pending));
        write_whole(&master, b"A", &AtomicBool::new(false));
        landed(&slave, 1);
        assert!(matches!(
            terminal.next().unwrap(),
            Next::Event(Event::Key(k)) if k.code == KeyCode::Up
        ));
        assert!(matches!(terminal.next().unwrap(), Next::Empty));
    }

    #[test]
    fn an_esc_alone_at_the_end_of_a_short_read_is_esc_at_once() {
        let _alone = alone();
        let (master, slave) = pty();
        let mut terminal = terminal_on(&slave);
        write_whole(&master, b"\x1b", &AtomicBool::new(false));
        landed(&slave, 1);
        assert!(matches!(
            terminal.next().unwrap(),
            Next::Event(Event::Key(k)) if k.code == KeyCode::Esc
        ));
    }

    #[test]
    fn replies_in_one_read_are_dropped_and_the_key_behind_them_comes_out() {
        let _alone = alone();
        let (master, slave) = pty();
        let mut terminal = terminal_on(&slave);
        let bytes = b"\x1b[?62;22c\x1b[1;1R\x1b[?1uq";
        write_whole(&master, bytes, &AtomicBool::new(false));
        landed(&slave, bytes.len());
        assert!(matches!(
            terminal.next().unwrap(),
            Next::Event(Event::Key(k)) if k.code == KeyCode::Char('q')
        ));
        assert!(matches!(terminal.next().unwrap(), Next::Empty));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_step_that_yields_no_event_reads_once() {
        let _alone = alone();
        let (master, slave) = pty();
        let mut terminal = terminal_on(&slave);
        let replies = b"\x1b[?62;22c".repeat(300);
        assert_eq!(replies.len(), 2700);
        assert!(write_whole(&master, &replies, &AtomicBool::new(false)));
        landed(&slave, 2700);
        assert!(matches!(terminal.next().unwrap(), Next::Pending));
        assert_eq!(
            bytes_waiting(slave.as_raw_fd()).unwrap(),
            2700 - TTY_BUFFER_SIZE
        );
    }

    #[test]
    fn more_than_a_read_s_worth_comes_out_whole_and_in_order() {
        let _alone = alone();
        const TOTAL: usize = 3000;
        let (master, slave) = pty();
        let mut terminal = terminal_on(&slave);
        let bytes: Vec<u8> = (0..TOTAL).map(|i| b'a' + (i % 26) as u8).collect();
        let mut sent = write_what_fits(&master, &bytes);
        assert!(sent > 0);
        landed(&slave, sent);
        let mut keys = Vec::new();
        // Linux holds the lot, and one read takes 1024 of it. macOS holds about 1 KiB, so the
        // rest is written as the terminal empties.
        #[cfg(target_os = "linux")]
        {
            assert_eq!(sent, TOTAL);
            assert!(is_key(&terminal.next().unwrap(), 'a'));
            assert_eq!(
                bytes_waiting(slave.as_raw_fd()).unwrap(),
                TOTAL - TTY_BUFFER_SIZE
            );
            keys.push(KeyCode::Char('a'));
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while keys.len() < TOTAL {
            assert!(Instant::now() < deadline, "only {} keys came", keys.len());
            match terminal.next().unwrap() {
                Next::Event(Event::Key(k)) => keys.push(k.code),
                Next::Pending => {}
                Next::Empty => {
                    assert_eq!(bytes_waiting(slave.as_raw_fd()).unwrap(), 0);
                    let more = write_what_fits(&master, &bytes[sent..]);
                    sent += more;
                    landed(&slave, more);
                }
                _ => panic!("a key, a pending step or an empty terminal"),
            }
        }
        let expected: Vec<KeyCode> = bytes.iter().map(|b| KeyCode::Char(*b as char)).collect();
        assert_eq!(keys, expected);
    }

    #[test]
    fn a_hang_up_ends_in_closed_without_blocking() {
        let _alone = alone();
        let (master, slave) = pty();
        let mut terminal = terminal_on(&slave);
        write_whole(&master, b"abc", &AtomicBool::new(false));
        landed(&slave, 3);
        drop(master);
        let mut closed = false;
        for _ in 0..10 {
            let started = Instant::now();
            let next = terminal.next().unwrap();
            assert!(started.elapsed() < Duration::from_millis(50));
            if matches!(next, Next::Closed) {
                closed = true;
                break;
            }
        }
        assert!(closed, "no Closed within 10 calls");
        for _ in 0..2 {
            let started = Instant::now();
            assert!(matches!(terminal.next().unwrap(), Next::Closed));
            assert!(started.elapsed() < Duration::from_millis(50));
        }
    }

    #[test]
    fn a_resize_and_a_key_pending_together_both_arrive() {
        let _alone = alone();
        let (master, slave) = pty();
        let terminal = terminal_on(&slave);
        signal_hook::low_level::raise(libc::SIGWINCH).unwrap();
        write_whole(&master, b"q", &AtomicBool::new(false));
        landed(&slave, 1);
        let (tx, rx) = mpsc::channel();
        let input = Input::spawn_with(tx, terminal, None).unwrap();
        let (mut resized, mut typed) = (false, false);
        for _ in 0..2 {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Msg::Resize {
                    width: 100,
                    height: 30,
                }) => resized = true,
                Ok(Msg::Key(k)) if k.code == KeyCode::Char('q') => typed = true,
                _ => panic!("the resize and the key, in either order"),
            }
        }
        assert!(resized && typed);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        drop(input);
    }

    /// Sets `stop` and `abort` when dropped, so a test that fails never leaves its writer
    /// spinning.
    struct Halt(Arc<AtomicBool>, Arc<AtomicBool>);

    impl Drop for Halt {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
            self.1.store(true, Ordering::SeqCst);
        }
    }

    /// Answers to a DA1 query inside an unfinished bracketed paste, until `stop` is set, and then
    /// `last` if there is one. The paste's start is written once into the empty pty, so it lands
    /// whole; the decoder holds every byte after it until the paste's end, whatever `more` says.
    /// A write the full pty cuts could otherwise end a short read on a lone ESC, which decodes to
    /// the Esc key and, after it, to keys. `stop` ends the stream only between two answers.
    fn stream_replies(
        master: &Arc<File>,
        slave: &OwnedFd,
        stop: &Arc<AtomicBool>,
        abort: &Arc<AtomicBool>,
        last: Option<&'static [u8]>,
    ) -> thread::JoinHandle<()> {
        const PASTE: &[u8] = b"\x1b[200~";
        assert!(write_whole(master, PASTE, abort));
        landed(slave, PASTE.len());
        let (master, stop, abort) = (Arc::clone(master), Arc::clone(stop), Arc::clone(abort));
        thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                if !write_whole(&master, b"\x1b[?62;22c", &abort) {
                    return;
                }
            }
            if let Some(last) = last {
                write_whole(&master, last, &abort);
            }
        })
    }

    #[test]
    fn a_pause_and_a_stop_are_acknowledged_while_replies_stream_in() {
        let _alone = alone();
        // A pause: acknowledged while replies keep coming, and the thread reads nothing until
        // the resume, the key behind the stream included.
        let (master, slave) = pty();
        let (master, stop, abort) = (
            Arc::new(master),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        let writer = stream_replies(&master, &slave, &stop, &abort, Some(b"\x1b[201~q"));
        let _halt = Halt(Arc::clone(&stop), Arc::clone(&abort));
        let (tx, rx) = mpsc::channel();
        let input = Input::spawn_with(tx, terminal_on(&slave), None).unwrap();
        thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        assert!(input.pause(), "acknowledged within ACK");
        assert!(started.elapsed() < ACK);
        stop.store(true, Ordering::SeqCst);
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "a paused thread reads nothing"
        );
        input.resume();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)),
            Ok(Msg::Key(k)) if k.code == KeyCode::Char('q')
        ));
        writer.join().unwrap();
        drop(input);
        drop(slave);

        // A stop: the drop is acknowledged while replies keep coming.
        let (master, slave) = pty();
        let (master, stop, abort) = (
            Arc::new(master),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        let writer = stream_replies(&master, &slave, &stop, &abort, None);
        let _halt = Halt(Arc::clone(&stop), Arc::clone(&abort));
        let (tx, _rx) = mpsc::channel();
        let input = Input::spawn_with(tx, terminal_on(&slave), None).unwrap();
        thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        drop(input);
        assert!(started.elapsed() < ACK, "acknowledged within ACK");
        stop.store(true, Ordering::SeqCst);
        abort.store(true, Ordering::SeqCst);
        writer.join().unwrap();
    }

    #[test]
    fn with_nothing_pending_next_is_empty_at_once() {
        let _alone = alone();
        let (_master, slave) = pty();
        let mut terminal = terminal_on(&slave);
        let started = Instant::now();
        assert!(matches!(terminal.next().unwrap(), Next::Empty));
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn the_terminal_s_file_status_flags_never_change() {
        let _alone = alone();
        let (master, slave) = pty();
        let mut terminal = terminal_on(&slave);
        let before = status_flags(slave.as_raw_fd()).unwrap();
        write_whole(&master, b"jk", &AtomicBool::new(false));
        landed(&slave, 2);
        let empty = (0..10).any(|_| matches!(terminal.next().unwrap(), Next::Empty));
        assert!(empty, "the keys were read and the terminal was empty");
        drop(master);
        let closed = (0..10).any(|_| matches!(terminal.next().unwrap(), Next::Closed));
        assert!(closed, "the hang-up was seen");
        let after = status_flags(slave.as_raw_fd()).unwrap();
        assert_eq!(before, after);
        assert_eq!(after & libc::O_NONBLOCK, 0);
    }

    #[test]
    fn a_pending_step_reads_on_without_waiting() {
        let script = Script::default();
        script.queue([
            Next::Pending,
            Next::Pending,
            Next::Event(Event::Key(key('q'))),
        ]);
        let (tx, rx) = mpsc::channel();
        let _input = Input::spawn_with(tx, script.clone(), None).unwrap();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)),
            Ok(Msg::Key(k)) if k.code == KeyCode::Char('q')
        ));
        script.idle(1);
        // Three steps, then `Empty`, then one wait. `Pending` taken for `Empty` waits at each.
        assert_eq!(
            (
                script.nexts.load(Ordering::SeqCst),
                script.waits.load(Ordering::SeqCst)
            ),
            (4, 1)
        );
    }
}
