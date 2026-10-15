//! The terminal (TUI spec §10.1): raw mode, the alternate screen and a hidden cursor while the
//! TUI holds it, all three undone on every way out, a panic's included. One lock serializes
//! drawing with every change of the terminal's owner, a panic hook's restore included, so no
//! frame is ever written to the main screen. `Tty` is what the loop needs of a terminal, so the
//! loop's tests can use a `TestBackend` instead.

use std::io;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, Once, PoisonError};

use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::Rect;
use ratatui::{Frame, Terminal};

/// The process's terminal.
static TERMINAL: Ownership = Ownership::new();
static HOOK: Once = Once::new();

thread_local! {
    /// This thread's key for `Ownership::holder`, never 0.
    static KEY: u64 = {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    };
}

/// This thread's key, or 0 once its thread-locals are gone (it holds no lock by then).
fn thread_key() -> u64 {
    KEY.try_with(|key| *key).unwrap_or(0)
}

/// Whether the TUI holds a terminal, and the lock that every change of that, and every frame,
/// is made under (§10.1).
pub(crate) struct Ownership {
    lock: Mutex<()>,
    /// Set by an enter, cleared by the restore that gives the terminal back. Changed only under
    /// `lock`.
    entered: AtomicBool,
    /// The key of the thread inside `lock` now, or 0.
    holder: AtomicU64,
    /// How many threads wait for `lock` now: what the tests watch to see a hook held up.
    waiting: AtomicUsize,
}

/// `lock`, held. Its holder is known until it is let go.
struct Held<'a> {
    ownership: &'a Ownership,
    _guard: MutexGuard<'a, ()>,
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.ownership.holder.store(0, Ordering::SeqCst);
    }
}

impl Ownership {
    pub(crate) const fn new() -> Ownership {
        Ownership {
            lock: Mutex::new(()),
            entered: AtomicBool::new(false),
            holder: AtomicU64::new(0),
            waiting: AtomicUsize::new(0),
        }
    }

    /// Takes the lock. A panic while it was held poisons it; it still serializes.
    fn hold(&self) -> Held<'_> {
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        self.waiting.fetch_sub(1, Ordering::SeqCst);
        self.holder.store(thread_key(), Ordering::SeqCst);
        Held {
            ownership: self,
            _guard: guard,
        }
    }

    /// Marks the terminal taken, then runs `take`, under the lock: a `take` that fails part way
    /// is undone by a restore too, and a panic hook restores before or after it, never during.
    fn enter_with<R>(&self, take: impl FnOnce() -> io::Result<R>) -> io::Result<R> {
        let _held = self.hold();
        self.entered.store(true, Ordering::SeqCst);
        take()
    }

    /// Runs `give_back` if, and only if, the terminal is taken, and marks it given back.
    fn restore_with(&self, give_back: impl FnOnce()) {
        let _held = self.hold();
        self.give_back(give_back);
    }

    /// The panic hook's restore. A thread that panics while it holds the lock (in `view`,
    /// inside a draw) owns the terminal already, and must not wait for itself. Any other waits
    /// for the frame in flight, so the frame lands on the alternate screen before it is left.
    fn restore_on_panic(&self, give_back: impl FnOnce()) {
        let me = thread_key();
        if me != 0 && self.holder.load(Ordering::SeqCst) == me {
            self.give_back(give_back);
        } else {
            self.restore_with(give_back);
        }
    }

    fn give_back(&self, give_back: impl FnOnce()) {
        if self.entered.swap(false, Ordering::SeqCst) {
            give_back();
        }
    }

    /// Runs `draw` if the terminal is taken, the check and the whole of `draw` under the lock;
    /// `None`, and nothing drawn, once it has been given back.
    fn while_entered<R>(&self, draw: impl FnOnce() -> R) -> Option<R> {
        let _held = self.hold();
        self.entered.load(Ordering::SeqCst).then(draw)
    }

    fn entered(&self) -> bool {
        self.entered.load(Ordering::SeqCst)
    }
}

/// Raw mode, the alternate screen, the cursor hidden.
fn take() -> io::Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, Hide)
}

/// Raw mode off first (it has more side effects, as ratatui's own restore says), then the main
/// screen and the cursor.
fn give_back() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
}

/// Gives the terminal back, once however often it is called.
pub(crate) fn restore() {
    TERMINAL.restore_with(give_back);
}

/// §10.1: a panic on any thread restores the terminal, then goes on to the hook installed
/// before: logging's, which logs it and prints it on stderr (core §14.2).
fn install_panic_hook() {
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            TERMINAL.restore_on_panic(give_back);
            previous(info);
        }));
    });
}

/// Clears the screen and forgets the last frame, so the next draw writes every cell. Never
/// `Terminal::clear`, which first asks the terminal where its cursor is, and waits for the
/// answer.
pub(crate) fn repaint<B: Backend>(terminal: &mut Terminal<B>) -> Result<(), B::Error> {
    let size = terminal.size()?;
    terminal.resize(Rect::new(0, 0, size.width, size.height))
}

/// What the loop needs of the terminal (§7.3, §10.1).
pub(crate) trait Tty {
    type B: Backend;
    /// Draws a frame with `render`, unless the terminal has been given back: then `Ok(false)`,
    /// and nothing is written. The check, the frame and its flush hold the terminal's lock, so
    /// a panic hook on another thread gives the terminal back before the frame or after it.
    fn draw(
        &mut self,
        render: impl FnOnce(&mut Frame),
    ) -> Result<bool, <Self::B as Backend>::Error>;
    /// Its size now, in columns and rows.
    fn size(&mut self) -> io::Result<(u16, u16)>;
    /// Whether the TUI holds it: whether there is anything to give back. A panic hook may give
    /// it back just after; `release` is then a no-op.
    fn active(&self) -> bool;
    /// Gives it back: raw mode off, the main screen, the cursor shown (§7.3 step 2).
    fn release(&mut self);
    /// Takes it again, and repaints (§7.3 step 4).
    fn reclaim(&mut self) -> io::Result<()>;
    /// Ctrl-Z: gives it back, stops the process, and takes it again once continued (§10.1).
    fn stop(&mut self) -> io::Result<()>;
}

/// The process's terminal, on stdout.
///
/// ratatui's `Terminal` is never dropped: its drop shows the cursor outside our lock and
/// `eprintln!`s when that write fails, which panics on a hung-up terminal and would turn a
/// SIGHUP's 129 into 1. Restoring (`give_back`) shows the cursor already, under the lock.
pub(crate) struct RealTty {
    terminal: ManuallyDrop<Terminal<CrosstermBackend<io::Stdout>>>,
}

impl RealTty {
    /// Takes the terminal (§10.1) and clears the alternate screen. The panic hook is in place
    /// before, so a panic from here on finds the terminal restored first.
    pub(crate) fn enter() -> io::Result<RealTty> {
        install_panic_hook();
        TERMINAL.enter_with(|| {
            take()?;
            let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
            repaint(&mut terminal)?;
            Ok(RealTty {
                terminal: ManuallyDrop::new(terminal),
            })
        })
    }
}

impl Tty for RealTty {
    type B = CrosstermBackend<io::Stdout>;

    fn draw(&mut self, render: impl FnOnce(&mut Frame)) -> io::Result<bool> {
        let terminal = &mut *self.terminal;
        TERMINAL
            .while_entered(|| terminal.draw(render).map(|_| ()))
            .map_or(Ok(false), |drawn| drawn.map(|()| true))
    }

    fn size(&mut self) -> io::Result<(u16, u16)> {
        crossterm::terminal::size()
    }

    fn active(&self) -> bool {
        TERMINAL.entered()
    }

    fn release(&mut self) {
        restore();
    }

    fn reclaim(&mut self) -> io::Result<()> {
        let terminal = &mut *self.terminal;
        TERMINAL.enter_with(|| {
            take()?;
            repaint(terminal)
        })
    }

    fn stop(&mut self) -> io::Result<()> {
        restore();
        // Without job control the signal is discarded, and the TUI simply carries on.
        let _ = tagteam_provider::term::stop_self();
        self.reclaim()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::panic::{self, AssertUnwindSafe};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use ratatui::backend::TestBackend;

    use super::*;

    #[test]
    fn restore_gives_the_terminal_back_once_and_only_after_an_enter() {
        let own = Ownership::new();
        let given = Cell::new(0);
        own.restore_with(|| given.set(given.get() + 1));
        assert_eq!(given.get(), 0, "nothing to give back before an enter");
        own.enter_with(|| Ok(())).unwrap();
        own.restore_with(|| given.set(given.get() + 1));
        own.restore_on_panic(|| given.set(given.get() + 1));
        assert_eq!(
            given.get(),
            1,
            "the guard, the hook and the loop may all restore"
        );
        own.enter_with(|| Ok(())).unwrap();
        own.restore_with(|| given.set(given.get() + 1));
        assert_eq!(
            given.get(),
            2,
            "a suspension enters again, and is restored again"
        );
    }

    #[test]
    fn an_enter_that_fails_part_way_is_still_undone() {
        // Raw mode may be on when the alternate screen fails.
        let own = Ownership::new();
        assert!(
            own.enter_with(|| Err::<(), _>(io::Error::other("no alternate screen")))
                .is_err()
        );
        let given = Cell::new(false);
        own.restore_with(|| given.set(true));
        assert!(given.get());
    }

    #[test]
    fn nothing_is_drawn_once_the_terminal_is_given_back() {
        let own = Ownership::new();
        assert_eq!(own.while_entered(|| "frame"), None);
        own.enter_with(|| Ok(())).unwrap();
        assert_eq!(own.while_entered(|| "frame"), Some("frame"));
        own.restore_on_panic(|| {});
        assert_eq!(own.while_entered(|| "frame"), None);
    }

    /// The events of one interleaving, in order.
    #[derive(Clone, Default)]
    struct Events(Arc<Mutex<Vec<&'static str>>>);

    impl Events {
        fn push(&self, what: &'static str) {
            self.0.lock().unwrap().push(what);
        }
        fn seen(&self) -> Vec<&'static str> {
            self.0.lock().unwrap().clone()
        }
    }

    #[test]
    fn a_panic_on_another_thread_gives_the_terminal_back_after_the_frame_in_flight() {
        // The UI thread has checked that the TUI holds the terminal and is writing a frame when
        // a lane panics. Its hook must wait for the frame, or the frame lands on the main screen
        // once the hook has left the alternate one.
        let own = Arc::new(Ownership::new());
        own.enter_with(|| Ok(())).unwrap();
        let events = Events::default();
        let (checked, mid_frame) = mpsc::channel();
        let (flush, flushed) = mpsc::channel::<()>();
        let ui = thread::spawn({
            let (own, events) = (Arc::clone(&own), events.clone());
            move || {
                own.while_entered(|| {
                    checked.send(()).unwrap();
                    flushed.recv().unwrap();
                    events.push("frame");
                })
            }
        });
        mid_frame.recv().unwrap();
        let lane = thread::spawn({
            let (own, events) = (Arc::clone(&own), events.clone());
            move || own.restore_on_panic(|| events.push("given back"))
        });
        // The hook either waits for the lock (fixed) or has already given the terminal back.
        let deadline = Instant::now() + Duration::from_secs(5);
        while own.waiting.load(Ordering::SeqCst) == 0 && events.seen().is_empty() {
            assert!(Instant::now() < deadline, "the hook neither waited nor ran");
            thread::yield_now();
        }
        assert_eq!(
            events.seen(),
            Vec::<&str>::new(),
            "nothing is given back under a frame"
        );
        flush.send(()).unwrap();
        assert_eq!(ui.join().unwrap(), Some(()));
        lane.join().unwrap();
        assert_eq!(events.seen(), ["frame", "given back"]);
        assert_eq!(
            own.while_entered(|| events.push("late frame")),
            None,
            "the loop draws nothing once the terminal is given back"
        );
    }

    #[test]
    fn a_panic_inside_a_frame_gives_the_terminal_back_without_waiting_for_itself() {
        // `view` panics on the UI thread, which holds the lock: the hook runs on that thread.
        let own = Arc::new(Ownership::new());
        own.enter_with(|| Ok(())).unwrap();
        let (done, finished) = mpsc::channel();
        thread::spawn({
            let own = Arc::clone(&own);
            move || {
                let given = Cell::new(false);
                own.while_entered(|| own.restore_on_panic(|| given.set(true)));
                done.send(given.get()).unwrap();
            }
        });
        assert_eq!(
            finished.recv_timeout(Duration::from_secs(5)),
            Ok(true),
            "the hook gave the terminal back, and did not wait for its own thread"
        );
    }

    #[test]
    fn a_panic_that_unwinds_out_of_a_frame_leaves_the_lock_usable() {
        let own = Ownership::new();
        own.enter_with(|| Ok(())).unwrap();
        let unwound = panic::catch_unwind(AssertUnwindSafe(|| {
            own.while_entered(|| panic!("the view panics"));
        }));
        assert!(unwound.is_err());
        assert_eq!(own.holder.load(Ordering::SeqCst), 0, "let go as it unwound");
        let given = Cell::new(false);
        own.restore_with(|| given.set(true));
        assert!(given.get(), "the poisoned lock still serializes");
    }

    #[test]
    fn dropping_a_real_tty_runs_no_drop_so_ratatuis_cursor_write_never_happens() {
        // ratatui's `Terminal::drop` retries a failed `show_cursor` and `eprintln!`s, which
        // panics on a hung-up terminal; `RealTty` has no drop glue at all.
        assert!(!std::mem::needs_drop::<RealTty>());
    }

    #[test]
    fn a_repaint_clears_the_screen_and_forgets_the_last_frame() {
        let mut terminal = Terminal::new(TestBackend::new(20, 2)).unwrap();
        terminal
            .draw(|frame| frame.render_widget("tagteam", frame.area()))
            .unwrap();
        repaint(&mut terminal).unwrap();
        assert_eq!(
            terminal.backend().to_string(),
            "\"                    \"\n\"                    \"\n"
        );
        terminal
            .draw(|frame| frame.render_widget("tagteam", frame.area()))
            .unwrap();
        assert!(
            terminal.backend().to_string().starts_with("\"tagteam "),
            "the same frame is written again in full"
        );
    }
}
