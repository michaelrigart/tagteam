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

use crossterm::event::{Event, KeyEventKind};
use mio::unix::SourceFd;
use mio::{Interest, Poll, Token};
use signal_hook_mio::v1_0::Signals;

use crate::msg::Msg;

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

/// What the terminal holds now.
pub(crate) enum Next {
    Event(Event),
    Empty,
    Closed,
}

/// Where terminal events come from: the process's terminal in the TUI, a script in the tests.
pub(crate) trait Events: Send + 'static {
    /// Blocks until the terminal has bytes (`Input`), SIGWINCH arrived (`Resized`), the
    /// waker was called or a signal interrupted the wait (`Woken`), or the terminal hung up
    /// (`Closed`).
    fn wait(&mut self) -> io::Result<Ready>;
    /// An event crossterm has read and parsed, without blocking, and `Empty` once it holds
    /// none. `Closed` once the terminal has hung up, and then crossterm is not called.
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
    /// own, made before crossterm's: a resize wakes the wait whatever crossterm has seen.
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

/// How many replies to a terminal's queries crossterm can keep back from one read of it: a
/// read is at most 1024 bytes (`TTY_BUFFER_SIZE` in crossterm 0.29.0's
/// `src/event/source/unix/mio.rs`), and the shortest reply, the primary device attributes
/// `ESC [ ? c`, is 4 bytes. The workspace pins crossterm at `=0.29.0`.
const REPLIES: usize = 1024 / 4;

/// `Terminal::next` over crossterm's calls, `closed` before each one.
///
/// crossterm's zero-timeout `poll` hands out one parsed event a call. When that event is a
/// reply it keeps to itself (a cursor position, the keyboard enhancement flags, the primary
/// device attributes), it says no, though keys may wait behind it. What it holds comes from
/// one read, so a key waits behind at most `REPLIES` of them, and `REPLIES + 1` noes in a row
/// say it holds nothing more. A read of newer bytes within those noes takes bytes whose
/// arrival `Wait::closed` keeps, or the wait has yet to see: the next wait drains again.
fn next_of(
    mut closed: impl FnMut() -> io::Result<bool>,
    mut poll: impl FnMut() -> io::Result<bool>,
    read: impl FnOnce() -> io::Result<Event>,
) -> io::Result<Next> {
    for _ in 0..=REPLIES {
        // crossterm's read loops forever on a terminal that has hung up.
        if closed()? {
            return Ok(Next::Closed);
        }
        if poll()? {
            return read().map(Next::Event);
        }
    }
    Ok(Next::Empty)
}

/// The process's terminal: fd 0 through `Wait`. Its events come through crossterm
/// (`event::poll(Duration::ZERO)` and `read`), and only after `Wait::closed` has said no. Its
/// size comes from `crossterm::terminal::size()`.
pub(crate) struct Terminal {
    wait: Wait,
}

impl Terminal {
    /// fd 0: `tagteam tui` runs only with stdin a terminal, and crossterm reads that one.
    fn stdin() -> io::Result<Terminal> {
        Ok(Terminal {
            wait: Wait::on(libc::STDIN_FILENO)?,
        })
    }
}

impl Events for Terminal {
    fn wait(&mut self) -> io::Result<Ready> {
        self.wait.wait()
    }

    fn next(&mut self) -> io::Result<Next> {
        let wait = &mut self.wait;
        next_of(
            || wait.closed(),
            || crossterm::event::poll(Duration::ZERO),
            crossterm::event::read,
        )
    }

    fn size(&mut self) -> io::Result<(u16, u16)> {
        crossterm::terminal::size()
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
    /// it. crossterm's reader loops on a closed terminal and never looks at the control, so an
    /// acknowledgment would only be waited for in vain, at full speed.
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

impl Input {
    pub(crate) fn spawn(tx: Sender<Msg>) -> io::Result<Input> {
        Input::spawn_with(tx, Terminal::stdin()?)
    }

    pub(crate) fn spawn_with(tx: Sender<Msg>, events: impl Events) -> io::Result<Input> {
        let control = Arc::new(Control {
            mode: Mutex::new(Mode::Reading),
            changed: Condvar::new(),
        });
        let wake = events.waker();
        let theirs = Arc::clone(&control);
        thread::Builder::new()
            .name("tagteam-tui-input".into())
            .spawn(move || read(&theirs, events, &tx))?;
        Ok(Input { control, wake })
    }

    /// Asks the thread for mode `to`, and wakes it from its wait to look.
    fn ask(&self, mode: &mut MutexGuard<'_, Mode>, to: Mode) {
        self.control.set(mode, to);
        (self.wake)();
    }
}

/// The thread: reads while the mode is `Reading`, and between two calls into the terminal
/// acknowledges a pause or a stop.
fn read(control: &Control, mut events: impl Events, tx: &Sender<Msg>) {
    // crossterm may hold events from before.
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
                        // A pause can come between two events crossterm holds.
                        more = true;
                    }
                    Mode::StopAsked | Mode::HungUp | Mode::Stopped => {
                        control.set(&mut mode, Mode::Stopped);
                        return;
                    }
                }
            }
        }
        if !step(&mut events, tx, &mut more) {
            control.set(&mut control.lock(), Mode::Stopped);
            return;
        }
    }
}

/// One call into the terminal, so the mode is looked at between any two: the next event
/// crossterm holds while there may be `more`, else a wait. False once the thread is to stop:
/// the terminal has hung up, or the UI thread is gone and nothing reads these any more.
fn step(events: &mut impl Events, tx: &Sender<Msg>, more: &mut bool) -> bool {
    if *more {
        return match events.next() {
            Ok(Next::Event(event)) => message(event).is_none_or(|msg| tx.send(msg).is_ok()),
            Ok(Next::Empty) => {
                *more = false;
                true
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
        // The size is read after the thread's own SIGWINCH, so crossterm's `Resize` is not
        // waited for (R-8).
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
/// SIGWINCH, and crossterm's `Resize` is dropped (R-8).
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
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
            // crossterm's own, which the SIGWINCH below makes too: dropped (R-8).
            Event::Resize(80, 24),
        ] {
            script.push(event);
        }
        script.resize(100, 30);
        let (tx, rx) = mpsc::channel();
        let _input = Input::spawn_with(tx, script.clone()).unwrap();
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
        let input = Input::spawn_with(tx, script.clone()).unwrap();
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
        let input = Input::spawn_with(tx, gate.clone()).unwrap();
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
        let input = Input::spawn_with(tx, script.clone()).unwrap();
        thread::sleep(Duration::from_millis(30));
        drop(input);
        let calls = script.calls();
        thread::sleep(Duration::from_millis(200));
        assert_eq!(script.calls(), calls);
        assert!(!script.reading.load(Ordering::SeqCst));
    }

    /// A thread held up before its call, as a closed terminal holds crossterm's reader.
    fn stuck_input() -> (Input, Gate) {
        let gate = Gate::default();
        let (tx, _rx) = mpsc::channel();
        let input = Input::spawn_with(tx, gate.clone()).unwrap();
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
        let input = Input::spawn_with(tx, script.clone()).unwrap();
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
        let _input = Input::spawn_with(tx, script.clone()).unwrap();
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
        let input = Input::spawn_with(tx, script.clone()).unwrap();
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
        let input = Input::spawn_with(tx, script.clone()).unwrap();
        assert_stopped(&input, &script, 1, 0);

        // The drain after a resume.
        let script = Script::default();
        let (tx, _rx) = mpsc::channel();
        let input = Input::spawn_with(tx, script.clone()).unwrap();
        script.idle(1);
        assert!(input.pause());
        script.queue([Next::Closed]);
        input.resume();
        assert_stopped(&input, &script, 2, 1);

        // A drain after a wait, two events in.
        let script = Script::default();
        let (tx, rx) = mpsc::channel();
        let input = Input::spawn_with(tx, script.clone()).unwrap();
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
        let input = Input::spawn_with(tx, script.clone()).unwrap();
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
        let input = Input::spawn_with(tx, script.clone()).unwrap();
        assert_tried_a_slice_apart(&script.nexts, started, "reads");
        drop(input);

        // The wait fails.
        let script = Script::default();
        script.fail_wait();
        let (tx, _rx) = mpsc::channel();
        let started = Instant::now();
        let _input = Input::spawn_with(tx, script.clone()).unwrap();
        assert_tried_a_slice_apart(&script.waits, started, "waits");
        assert_eq!(
            script.nexts.load(Ordering::SeqCst),
            1,
            "the drain at startup"
        );
    }

    #[test]
    fn a_resize_is_sent_from_the_outer_signal_even_before_crossterm_has_it() {
        // The thread's own SIGWINCH woke it, and crossterm has no `Resize` to read yet.
        let script = Script::default();
        let (tx, rx) = mpsc::channel();
        let _input = Input::spawn_with(tx, script.clone()).unwrap();
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
        // what it kept.
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(wait.wait().unwrap());
            drop(ours);
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(Ready::Input));
        drop(theirs);
    }

    /// `next_of` over a crossterm that holds `replies` it keeps to itself and then `q`, or
    /// nothing after them: what it returns, and how many `closed`s and `poll`s it made.
    fn next_behind(replies: usize, then_q: bool, closes_at: Option<usize>) -> (Next, usize, usize) {
        let (mut closeds, mut polls) = (0, 0);
        let next = next_of(
            || {
                closeds += 1;
                Ok(closes_at == Some(closeds))
            },
            || {
                polls += 1;
                Ok(then_q && polls > replies)
            },
            || Ok(Event::Key(key('q'))),
        )
        .unwrap();
        (next, closeds, polls)
    }

    #[test]
    fn a_key_behind_the_replies_crossterm_keeps_is_read() {
        // Up to as many as one read holds.
        for replies in [0, 1, 2, REPLIES] {
            let (next, closeds, polls) = next_behind(replies, true, None);
            assert!(
                matches!(next, Next::Event(Event::Key(k)) if k.code == KeyCode::Char('q')),
                "behind {replies} replies"
            );
            assert_eq!(
                (closeds, polls),
                (replies + 1, replies + 1),
                "a hang-up check each"
            );
        }
    }

    #[test]
    fn only_enough_noes_in_a_row_say_crossterm_holds_nothing() {
        let (next, closeds, polls) = next_behind(REPLIES + 1, false, None);
        assert!(matches!(next, Next::Empty));
        assert_eq!((closeds, polls), (REPLIES + 1, REPLIES + 1));
    }

    #[test]
    fn a_hang_up_between_the_noes_ends_them_before_crossterm_is_called_again() {
        let (next, closeds, polls) = next_behind(REPLIES, true, Some(5));
        assert!(matches!(next, Next::Closed));
        assert_eq!((closeds, polls), (5, 4));
    }
}
