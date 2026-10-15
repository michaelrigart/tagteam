//! The input thread (TUI spec §4.3): the terminal's events as `Msg`s. It polls in 50 ms slices,
//! so it can pause, and acknowledge the pause, before the terminal is handed to a foreground
//! action (§7.3 step 1), and it reads nothing once the TUI has let the terminal go.

use std::io;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyEventKind};

use crate::msg::Msg;

/// How long one poll waits before the thread looks at its control again.
const SLICE: Duration = Duration::from_millis(50);
/// How long `pause` and dropping wait for the thread to say so. Far more than a slice: a thread
/// that takes longer is held up somewhere between its look at the control and its poll, or
/// gone.
const ACK: Duration = Duration::from_secs(1);

/// Where terminal events come from: crossterm in the TUI, a script in the tests.
pub(crate) trait Events: Send + 'static {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool>;
    fn read(&mut self) -> io::Result<Event>;
}

/// The process's terminal, through crossterm's reader.
pub(crate) struct Crossterm;

impl Events for Crossterm {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        crossterm::event::poll(timeout)
    }

    fn read(&mut self) -> io::Result<Event> {
        crossterm::event::read()
    }
}

/// What the UI thread asks of the input thread (§7.3 step 1).
pub(crate) trait Pause {
    /// True once the thread reads no more: it is between two polls, and polls again only after
    /// `resume`. False when it has not said so within `ACK`: it may still poll, and read keys
    /// meant for the terminal's next owner, so the terminal must not be handed over. The pause
    /// stays asked for until `resume`.
    fn pause(&self) -> bool;
    fn resume(&self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Reading,
    PauseAsked,
    Paused,
    StopAsked,
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
}

impl Input {
    pub(crate) fn spawn(tx: Sender<Msg>) -> io::Result<Input> {
        Input::spawn_with(tx, Crossterm)
    }

    pub(crate) fn spawn_with(tx: Sender<Msg>, events: impl Events) -> io::Result<Input> {
        let control = Arc::new(Control {
            mode: Mutex::new(Mode::Reading),
            changed: Condvar::new(),
        });
        let theirs = Arc::clone(&control);
        thread::Builder::new()
            .name("tagteam-tui-input".into())
            .spawn(move || read(&theirs, events, &tx))?;
        Ok(Input { control })
    }
}

/// The thread: reads while the mode is `Reading`, and between two polls acknowledges a pause or
/// a stop.
fn read(control: &Control, mut events: impl Events, tx: &Sender<Msg>) {
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
                    }
                    Mode::StopAsked | Mode::Stopped => {
                        control.set(&mut mode, Mode::Stopped);
                        return;
                    }
                }
            }
        }
        match events.poll(SLICE) {
            Ok(true) => match events.read() {
                Ok(event) => {
                    let sent = message(event).is_none_or(|msg| tx.send(msg).is_ok());
                    if !sent {
                        // The UI thread is gone: nothing reads these any more.
                        control.set(&mut control.lock(), Mode::Stopped);
                        return;
                    }
                }
                Err(_) => thread::sleep(SLICE),
            },
            Ok(false) => {}
            // A terminal that cannot be read now: a hang-up arrives as SIGHUP. No spinning.
            Err(_) => thread::sleep(SLICE),
        }
    }
}

/// The events the TUI acts on: key presses (and repeats) and resizes.
fn message(event: Event) -> Option<Msg> {
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => Some(Msg::Key(key)),
        Event::Resize(width, height) => Some(Msg::Resize { width, height }),
        _ => None,
    }
}

impl Pause for Input {
    fn pause(&self) -> bool {
        let mut mode = self.control.lock();
        if *mode == Mode::Reading {
            self.control.set(&mut mode, Mode::PauseAsked);
        }
        self.control.wait_while(mode, Mode::PauseAsked)
    }

    fn resume(&self) {
        let mut mode = self.control.lock();
        if matches!(*mode, Mode::PauseAsked | Mode::Paused) {
            self.control.set(&mut mode, Mode::Reading);
        }
    }
}

impl Drop for Input {
    /// Stops the thread before the terminal is given back, so it never reads the shell's
    /// input. The TUI is ending, so nothing waits beyond `ACK`: a thread held up longer stops
    /// at its next look.
    fn drop(&mut self) {
        let mut mode = self.control.lock();
        if *mode != Mode::Stopped {
            self.control.set(&mut mode, Mode::StopAsked);
        }
        let _ = self.control.wait_while(mode, Mode::StopAsked);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;

    /// Scripted events. `reading` is set while the thread is inside `poll` or `read`, and
    /// `polls` counts its polls.
    #[derive(Clone, Default)]
    struct Script {
        queue: Arc<Mutex<VecDeque<Event>>>,
        reading: Arc<AtomicBool>,
        polls: Arc<AtomicUsize>,
    }

    impl Script {
        fn push(&self, event: Event) {
            self.queue.lock().unwrap().push_back(event);
        }
    }

    impl Events for Script {
        fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
            self.reading.store(true, Ordering::SeqCst);
            self.polls.fetch_add(1, Ordering::SeqCst);
            let ready = !self.queue.lock().unwrap().is_empty();
            if !ready {
                thread::sleep(timeout);
            }
            self.reading.store(false, Ordering::SeqCst);
            Ok(ready)
        }

        fn read(&mut self) -> io::Result<Event> {
            Ok(self
                .queue
                .lock()
                .unwrap()
                .pop_front()
                .expect("poll said ready"))
        }
    }

    /// Events whose poll waits at a gate before it looks: the thread is past its look at the
    /// control and has not polled yet, as if it were descheduled there. Once opened, it polls
    /// as `Script` does.
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
    }

    impl Events for Gate {
        fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
            self.waiting.store(true, Ordering::SeqCst);
            let (open, opened) = &*self.open;
            let mut open = open.lock().unwrap();
            while !*open {
                open = opened.wait(open).unwrap();
            }
            drop(open);
            self.script.poll(timeout)
        }

        fn read(&mut self) -> io::Result<Event> {
            self.script.read()
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
            Event::Resize(100, 30),
        ] {
            script.push(event);
        }
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
        assert!(!script.reading.load(Ordering::SeqCst), "between two polls");
        let polls = script.polls.load(Ordering::SeqCst);
        script.push(Event::Key(key('x')));
        thread::sleep(Duration::from_millis(200));
        assert_eq!(
            script.polls.load(Ordering::SeqCst),
            polls,
            "no poll while paused"
        );
        assert!(rx.try_recv().is_err(), "nothing read while paused");
        input.resume();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)),
            Ok(Msg::Key(k)) if k.code == KeyCode::Char('x')
        ));
    }

    #[test]
    fn a_pause_the_thread_does_not_acknowledge_says_so() {
        // The thread has looked at the control (`Reading`) and is held up before its poll. A
        // pause that returned anyway would hand the terminal over while it can still read.
        let gate = Gate::default();
        gate.script.push(Event::Key(key('x')));
        let (tx, rx) = mpsc::channel();
        let input = Input::spawn_with(tx, gate.clone()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !gate.waiting.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "the thread never reached its poll"
            );
            thread::yield_now();
        }
        let started = Instant::now();
        assert!(!input.pause(), "not acknowledged");
        assert!(started.elapsed() >= ACK);
        // What a hand-over would have lost: the thread polls, and reads the next owner's key.
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
        let polls = script.polls.load(Ordering::SeqCst);
        thread::sleep(Duration::from_millis(200));
        assert_eq!(script.polls.load(Ordering::SeqCst), polls);
        assert!(!script.reading.load(Ordering::SeqCst));
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
}
