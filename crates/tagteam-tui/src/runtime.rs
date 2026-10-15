//! The UI thread (TUI spec §4.3): the lanes it drives, the loop that runs `update` and draws
//! `view`, the suspension that hands the terminal to a foreground action (§7.3), and the one
//! shutdown every way out goes through (§10.2).

use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tagteam_core::ProviderId;
use tagteam_provider::Cancel;

use crate::ask::ChannelAsk;
use crate::host::{Action, ActionOutcome, Host};
use crate::input::{Input, Pause};
use crate::lanes::Lane;
use crate::msg::{ActionsReq, Cmd, Msg, ViewsReq};
use crate::source::{Source, SourceFactory};
use crate::state::{Screen, State};
use crate::term::{self, RealTty, Tty};
use crate::update::{drawn, update};
use crate::view::view;

/// The loop wakes at least this often, to look at the signal token and to tick (§4.3).
const WAKE: Duration = Duration::from_millis(250);
/// How long the first frame waits for the first `accounts` (§6.1).
const FIRST_FRAME: Duration = Duration::from_millis(100);
/// Why a hand-over of the terminal was refused (§7.3 step 1): the input thread could still read
/// keys typed for the terminal's next owner.
const NOT_PAUSED: &str = "the TUI's key reader did not pause in time; try again";

/// How the TUI runs.
pub struct TuiOptions {
    pub screen: Screen,
    pub color: bool,
    pub provider: Option<ProviderId>,
    /// The process's signal token: the CLI's handlers record SIGINT, SIGTERM and SIGHUP in it.
    pub signal: Cancel,
    /// The shutdown token. The binary has already built every lane's engine, and the host's
    /// in-TUI context, with it.
    pub shutdown: Cancel,
}

/// How the TUI ended.
pub struct TuiExit {
    pub code: i32,
    /// Printed on stderr after the terminal is restored (spec §10.2 step 6).
    pub report: Option<String>,
}

/// Runs the TUI on the process's terminal until it is quit, signalled, or fails. It returns
/// once every lane has been joined and the terminal restored (§10.2).
pub fn run(opts: TuiOptions, sources: SourceFactory, host: Arc<dyn Host>) -> TuiExit {
    run_with(
        opts,
        sources,
        host,
        RealTty::enter,
        Input::spawn,
        term::restore,
    )
}

/// `run`, over the terminal `enter` sets up and the input thread `input` starts.
pub(crate) fn run_with<T: Tty, P: Pause>(
    opts: TuiOptions,
    sources: SourceFactory,
    host: Arc<dyn Host>,
    enter: impl FnOnce() -> io::Result<T>,
    input: impl FnOnce(Sender<Msg>) -> io::Result<P>,
    restore: fn(),
) -> TuiExit {
    let _restored = Restore(restore);
    let (tx, rx) = mpsc::channel();
    // The lanes build their engines while the terminal is set up (§6.1).
    let lanes = Lanes::spawn(opts.provider.clone(), &sources, &host, &tx, &opts.shutdown);
    let driven = panic::catch_unwind(AssertUnwindSafe(|| {
        let tty = match enter() {
            Ok(tty) => tty,
            Err(e) => {
                return Ending::fatal(format!("tagteam: the terminal could not be set up: {e}"));
            }
        };
        let input = match input(tx.clone()) {
            Ok(input) => input,
            Err(e) => {
                return Ending::fatal(format!(
                    "tagteam: the terminal's keys could not be read: {e}"
                ));
            }
        };
        let mut ui = Ui::new(&opts, tty, input, &rx, &lanes, host.as_ref());
        ui.start();
        ui.run()
    }));
    // A panic on the UI thread: the hook has restored the terminal and logged it (§10.1), and
    // the lanes still drain (§10.2).
    let ending = driven.unwrap_or(Ending {
        code: 1,
        fatal: None,
        report: String::new(),
    });
    finish(lanes, &opts.shutdown, &rx, ending, restore)
}

/// §10.2 steps 1, 2, 5 and 6, whatever ended the loop. The token is set and the lanes closed,
/// which a panic on the UI thread skipped; every lane is joined; the terminal is restored; and
/// the report is made: what an action still running did, in the CLI's words, then the error
/// that ended the TUI.
fn finish(
    lanes: Lanes,
    shutdown: &Cancel,
    rx: &Receiver<Msg>,
    ending: Ending,
    restore: fn(),
) -> TuiExit {
    if shutdown.requested().is_none() {
        shutdown.request(libc::SIGINT);
    }
    lanes.close();
    let died = lanes.join();
    restore();
    let mut report = ending.report;
    while let Ok(msg) = rx.try_recv() {
        report.push_str(&reported(msg));
    }
    let mut code = ending.code;
    let mut errors: Vec<String> = ending.fatal.into_iter().collect();
    for lane in died {
        errors.push(format!(
            "tagteam: the TUI's {lane} lane panicked; the log has the details"
        ));
        if code == 0 {
            code = 1;
        }
    }
    for error in errors {
        report.push_str(&error);
        report.push('\n');
    }
    TuiExit {
        code,
        report: (!report.is_empty()).then_some(report),
    }
}

/// What a message reports once the TUI has stopped reading (§10.2 step 6): an action's `out` and
/// `err`, in the CLI's words; nothing for any other message.
fn reported(msg: Msg) -> String {
    match msg {
        Msg::ActionDone {
            outcome: ActionOutcome::Done { out, err, .. },
            ..
        } => out + &err,
        _ => String::new(),
    }
}

/// §10.1: restores the terminal when dropped, on every way out of `run`.
struct Restore(fn());

impl Drop for Restore {
    fn drop(&mut self) {
        (self.0)();
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Part A's engine lanes (§4.3): views (`accounts`, `history`, store-served) and actions (the
/// collector and every `Host` action, one at a time).
pub(crate) struct Lanes {
    views: Lane<ViewsReq>,
    actions: Lane<ActionsReq>,
}

impl Lanes {
    /// Spawns both. Each builds its own `Source` in its own thread (R4), and a lane that panics
    /// says so on `tx` (§4.3, §10.4).
    pub(crate) fn spawn(
        provider: Option<ProviderId>,
        sources: &SourceFactory,
        host: &Arc<dyn Host>,
        tx: &Sender<Msg>,
        shutdown: &Cancel,
    ) -> Lanes {
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
        let actions = {
            let (make, out, died) = (Arc::clone(sources), tx.clone(), tx.clone());
            let (host, shutdown) = (Arc::clone(host), shutdown.clone());
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
                            let mut ask = ChannelAsk::new(out.clone(), shutdown.clone());
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
        Lanes { views, actions }
    }

    /// §10.2 step 2: nothing more is taken, what is queued is dropped, and an idle lane wakes
    /// and returns.
    fn close(&self) {
        self.views.close();
        self.actions.close();
    }

    /// §10.2 step 5: joins both lanes, and names those that panicked.
    fn join(self) -> Vec<&'static str> {
        let mut died = Vec::new();
        if self.views.join().is_err() {
            died.push("views");
        }
        if self.actions.join().is_err() {
            died.push("actions");
        }
        died
    }
}

/// How the loop ended (§10.2's table): the exit code, a fatal error's message, and what an
/// action reported in a message the shutdown overtook, for the report.
pub(crate) struct Ending {
    code: i32,
    fatal: Option<String>,
    report: String,
}

impl Ending {
    fn fatal(message: String) -> Ending {
        Ending {
            code: 1,
            fatal: Some(message),
            report: String::new(),
        }
    }
}

/// The UI thread's loop and what it owns (§4.3).
pub(crate) struct Ui<'a, T: Tty, P: Pause> {
    state: State,
    tty: T,
    input: P,
    rx: &'a Receiver<Msg>,
    lanes: &'a Lanes,
    host: &'a dyn Host,
    signal: Cancel,
    shutdown: Cancel,
    /// Set when the shutdown begins.
    ending: Option<Ending>,
    next_tick: Instant,
}

impl<'a, T: Tty, P: Pause> Ui<'a, T, P> {
    pub(crate) fn new(
        opts: &TuiOptions,
        tty: T,
        input: P,
        rx: &'a Receiver<Msg>,
        lanes: &'a Lanes,
        host: &'a dyn Host,
    ) -> Self {
        Ui {
            state: State::new(opts.screen, opts.color, opts.provider.clone(), now_ms()),
            tty,
            input,
            rx,
            lanes,
            host,
            signal: opts.signal.clone(),
            shutdown: opts.shutdown.clone(),
            ending: None,
            next_tick: Instant::now() + WAKE,
        }
    }

    /// §6.1: the size, the first loads, up to 100 ms for the first `accounts`, and the first
    /// frame, after which the first collection is asked for (`draw`). The signal token comes
    /// first here too: before the wait, and on every wake before the message (§10.2).
    pub(crate) fn start(&mut self) {
        self.resized();
        let cmds = self.state.start();
        self.execute(cmds);
        let deadline = Instant::now() + FIRST_FRAME;
        while !self.shutting_down() {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(msg) = self.rx.recv_timeout(left) else {
                break;
            };
            let accounts = matches!(msg, Msg::Accounts { .. });
            self.deliver(msg);
            if accounts {
                break;
            }
        }
        if self.ending.is_none() {
            self.state.dirty = true;
            self.draw();
        }
    }

    /// §4.3, until a shutdown begins. Each wake looks at the signal token before anything else
    /// (§10.2): before the wait, and again before the message or the tick that ended it. It
    /// redraws when the state changed, takes one message, and ticks every 250 ms whatever
    /// arrives.
    pub(crate) fn run(&mut self) -> Ending {
        loop {
            self.shutting_down();
            if let Some(ending) = self.ending.take() {
                return ending;
            }
            if self.state.dirty {
                self.draw();
            }
            let wait = self.next_tick.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(wait) {
                Ok(msg) => self.deliver(msg),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => self.begin_shutdown(
                    1,
                    libc::SIGINT,
                    Some("tagteam: the TUI lost its message channel".into()),
                ),
            }
            if self.ending.is_none() && Instant::now() >= self.next_tick {
                self.next_tick = Instant::now() + WAKE;
                self.deliver(Msg::Tick);
            }
        }
    }

    /// Begins the shutdown for a signal in the token: SIGTERM, SIGHUP, or a SIGINT sent to the
    /// process (in raw mode Ctrl-C is a key). True once a shutdown has begun, for any reason.
    fn shutting_down(&mut self) -> bool {
        if self.ending.is_none() {
            if let Some(signal) = self.signal.requested() {
                self.begin_shutdown(128 + signal, signal, None);
            }
        }
        self.ending.is_some()
    }

    /// Applies `msg`, after a look at the signal token: a signal that arrived with it comes
    /// first, and so does a shutdown already begun. Then `msg` is not applied; an action's
    /// outcome in it still goes into the report (§10.2 step 6).
    fn deliver(&mut self, msg: Msg) {
        if !self.shutting_down() {
            self.apply(msg);
        } else if let Some(ending) = &mut self.ending {
            ending.report.push_str(&reported(msg));
        }
    }

    fn apply(&mut self, msg: Msg) {
        let cmds = update(&mut self.state, msg, now_ms());
        self.execute(cmds);
    }

    fn execute(&mut self, cmds: Vec<Cmd>) {
        for cmd in cmds {
            // Once the shutdown has begun nothing new starts (§10.2 step 2).
            if self.ending.is_some() {
                return;
            }
            match cmd {
                Cmd::LoadAccounts { r#gen } => {
                    let _ = self.lanes.views.submit(ViewsReq::Accounts { r#gen });
                }
                Cmd::LoadHistory {
                    r#gen,
                    account,
                    since_s,
                } => {
                    let req = ViewsReq::History {
                        r#gen,
                        account,
                        since_s,
                    };
                    let _ = self.lanes.views.submit(req);
                }
                Cmd::CollectDue { r#gen, accounts } => {
                    let _ = self
                        .lanes
                        .actions
                        .submit(ActionsReq::Collect { r#gen, accounts });
                }
                Cmd::Act(action) => {
                    let _ = self.lanes.actions.submit(ActionsReq::Act { action });
                }
                Cmd::Suspend(action) => self.suspend(action),
                Cmd::Quit { code } => self.begin_shutdown(code, libc::SIGINT, None),
                Cmd::Stop => self.stop(),
            }
        }
    }

    /// Draws the state, unless the terminal has been given back: a panic hook may have done so
    /// from another thread, and `Tty::draw` checks under the lock the hook takes. After a frame,
    /// the first that shows accounts asks for the first collection (§6.1).
    fn draw(&mut self) {
        let now = now_ms();
        let state = &self.state;
        let result = self
            .tty
            .draw(|frame| view(state, frame, now))
            .map_err(|e| e.to_string());
        match result {
            Ok(true) => {
                self.state.dirty = false;
                let cmds = drawn(&mut self.state);
                self.execute(cmds);
            }
            Ok(false) => {}
            // §10.4: a terminal that can no longer be written is fatal. Once the shutdown has
            // begun there is nothing more to do about it.
            Err(e) if self.ending.is_none() => self.begin_shutdown(
                1,
                libc::SIGINT,
                Some(format!("tagteam: the terminal could not be written: {e}")),
            ),
            Err(_) => {}
        }
    }

    /// §10.2 steps 1 to 4. A second trigger, a second `q` among them, changes nothing.
    fn begin_shutdown(&mut self, code: i32, signal: i32, fatal: Option<String>) {
        if self.ending.is_some() {
            return;
        }
        self.shutdown.request(signal);
        self.lanes.close();
        self.state.quitting = Some(code);
        self.state.dirty = true;
        self.ending = Some(Ending {
            code,
            fatal,
            report: String::new(),
        });
        if code == 0 {
            // `q` or Ctrl-C: `finishing <action>…` stays up while the lanes drain.
            self.draw();
        } else {
            // A signal or a fatal error: the terminal is restored first.
            self.hand_back();
        }
    }

    /// §10.2 step 4 for a signal or a fatal error. The input thread is asked to pause, and the
    /// terminal is given back whether it says so or not: the process is exiting.
    fn hand_back(&mut self) {
        if self.tty.active() {
            let _ = self.input.pause();
            self.tty.release();
        }
    }

    /// §7.3 step 1, before the terminal is handed over: true once the input thread has said it
    /// reads no more. Otherwise the pause is withdrawn, the terminal kept, and the banner says
    /// that `refused`, and why.
    fn paused(&mut self, refused: &str) -> bool {
        if self.input.pause() {
            return true;
        }
        self.input.resume();
        self.state.banner = Some(format!("{refused}: {NOT_PAUSED}"));
        self.state.banner_from = None;
        self.state.dirty = true;
        false
    }

    /// §7.3: runs `action` with the terminal handed back, then takes the terminal again. When
    /// the input thread does not pause, nothing runs and the TUI keeps the terminal.
    fn suspend(&mut self, action: Action) {
        if !self.paused(&format!("{} did not start", action.name())) {
            self.state.running = None;
            return;
        }
        self.tty.release();
        let outcome = self.host.act_foreground(&action);
        // A SIGINT recorded by now was the action's: Ctrl-C at its prompt or in its session
        // (§7.3). Only a SIGINT is spent, in one step, so a SIGTERM or SIGHUP that replaced it
        // stays recorded.
        self.signal.take_if(libc::SIGINT);
        if let Some(signal) = self.signal.requested() {
            // SIGTERM or SIGHUP: the shutdown, with the terminal as the action left it.
            self.begin_shutdown(128 + signal, signal, None);
            return;
        }
        if let Err(e) = self.tty.reclaim() {
            self.begin_shutdown(
                1,
                libc::SIGINT,
                Some(format!(
                    "tagteam: the terminal could not be set up again: {e}"
                )),
            );
            return;
        }
        self.input.resume();
        self.resized();
        let outcome = match outcome {
            // A foreground action has the terminal; one that asks for it again would loop.
            ActionOutcome::NeedsTerminal => ActionOutcome::Done {
                exit: 1,
                out: String::new(),
                err: String::new(),
            },
            done => done,
        };
        self.deliver(Msg::ActionDone { action, outcome });
        self.state.dirty = true;
    }

    /// Ctrl-Z (§10.1): the terminal is given back while the process is stopped, and taken
    /// again, and redrawn, once it is continued. Refused, as a suspension is, when the input
    /// thread does not pause.
    fn stop(&mut self) {
        if !self.paused("Ctrl-Z did not suspend") {
            return;
        }
        match self.tty.stop() {
            Ok(()) => {
                self.input.resume();
                self.resized();
                self.state.dirty = true;
            }
            Err(e) => self.begin_shutdown(
                1,
                libc::SIGINT,
                Some(format!(
                    "tagteam: the terminal could not be set up again: {e}"
                )),
            ),
        }
    }

    /// The terminal's size, as `update` takes it.
    fn resized(&mut self) {
        if let Ok((width, height)) = self.tty.size() {
            self.apply(Msg::Resize { width, height });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tagteam_core::AccountId;
    use tagteam_engine::views::HistoryView;

    use super::*;
    use crate::host::{AccountRef, Ask};
    use crate::source::{AccountsSnapshot, CollectSummary, SourceError};
    use crate::term::repaint;

    /// What the fake terminal and input were asked to do, in order.
    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<&'static str>>>);

    impl Log {
        fn push(&self, what: &'static str) {
            self.0.lock().unwrap().push(what);
        }
        fn take(&self) -> Vec<&'static str> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
        fn seen(&self) -> Vec<&'static str> {
            self.0.lock().unwrap().clone()
        }
    }

    /// The fake input thread: it logs to `log`, and acknowledges a pause unless `stuck`.
    #[derive(Clone, Default)]
    struct Keys {
        log: Log,
        stuck: Arc<AtomicBool>,
    }

    impl Pause for Keys {
        fn pause(&self) -> bool {
            self.log.push("pause");
            !self.stuck.load(Ordering::SeqCst)
        }
        fn resume(&self) {
            self.log.push("resume");
        }
    }

    /// A 120×40 `TestBackend` that records being handed over in `log`, and each frame in
    /// `frames`. With `panics`, drawing panics; `on_draw` runs once, after the next frame.
    struct FakeTty {
        terminal: Terminal<TestBackend>,
        log: Log,
        frames: Log,
        active: bool,
        panics: bool,
        on_draw: Option<Box<dyn FnOnce()>>,
    }

    impl FakeTty {
        fn new(log: Log, frames: Log) -> FakeTty {
            FakeTty {
                terminal: Terminal::new(TestBackend::new(120, 40)).unwrap(),
                log,
                frames,
                active: true,
                panics: false,
                on_draw: None,
            }
        }

        fn text(&self) -> String {
            self.terminal.backend().to_string()
        }
    }

    impl Tty for FakeTty {
        type B = TestBackend;

        fn draw(&mut self, render: impl FnOnce(&mut ratatui::Frame)) -> Result<bool, Infallible> {
            assert!(!self.panics, "the UI thread panics while it draws");
            if !self.active {
                return Ok(false);
            }
            self.terminal.draw(render)?;
            self.frames.push("draw");
            if let Some(then) = self.on_draw.take() {
                then();
            }
            Ok(true)
        }

        fn size(&mut self) -> io::Result<(u16, u16)> {
            let size = self.terminal.size().unwrap();
            Ok((size.width, size.height))
        }

        fn active(&self) -> bool {
            self.active
        }

        fn release(&mut self) {
            self.log.push("release");
            self.active = false;
        }

        fn reclaim(&mut self) -> io::Result<()> {
            self.log.push("reclaim");
            self.active = true;
            repaint(&mut self.terminal).map_err(|never| match never {})
        }

        fn stop(&mut self) -> io::Result<()> {
            self.log.push("stop");
            Ok(())
        }
    }

    /// A source with no accounts. `accounts` takes `delay`, or panics with `panics`. `drops`
    /// counts the sources dropped: a lane drops its own when its thread ends.
    struct Empty {
        delay: Duration,
        panics: bool,
        drops: Arc<AtomicUsize>,
    }

    impl Drop for Empty {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl Source for Empty {
        fn accounts(
            &self,
            _provider: Option<&ProviderId>,
        ) -> Result<AccountsSnapshot, SourceError> {
            assert!(!self.panics, "the views lane's read panics");
            thread::sleep(self.delay);
            Ok(AccountsSnapshot {
                providers: Vec::new(),
                this: None,
                unmanaged_live: None,
                in_run_shell: false,
                read_at_s: 0,
            })
        }

        fn history(&self, _account: &AccountId, _since_s: i64) -> Result<HistoryView, SourceError> {
            Err(SourceError {
                kind: "test".into(),
                message: "no history in this test".into(),
            })
        }

        fn collect_due(&self, _accounts: &[AccountId]) -> Result<CollectSummary, SourceError> {
            Ok(CollectSummary {
                fetched: 0,
                warnings: Vec::new(),
            })
        }
    }

    fn sources(delay: Duration, panics: bool, drops: &Arc<AtomicUsize>) -> SourceFactory {
        let drops = Arc::clone(drops);
        Arc::new(move || {
            Box::new(Empty {
                delay,
                panics,
                drops: Arc::clone(&drops),
            }) as Box<dyn Source>
        })
    }

    fn quick() -> SourceFactory {
        sources(Duration::ZERO, false, &Arc::default())
    }

    /// A source with one account, read after `delay`. Each collection is logged in `frames`,
    /// beside the fake terminal's frames.
    struct OneAccount {
        delay: Duration,
        frames: Log,
    }

    impl Source for OneAccount {
        fn accounts(
            &self,
            _provider: Option<&ProviderId>,
        ) -> Result<AccountsSnapshot, SourceError> {
            thread::sleep(self.delay);
            Ok(crate::testutil::snapshot(crate::testutil::many(1)))
        }

        fn history(&self, _account: &AccountId, _since_s: i64) -> Result<HistoryView, SourceError> {
            Err(SourceError {
                kind: "test".into(),
                message: "no history in this test".into(),
            })
        }

        fn collect_due(&self, _accounts: &[AccountId]) -> Result<CollectSummary, SourceError> {
            self.frames.push("collect");
            Ok(CollectSummary {
                fetched: 0,
                warnings: Vec::new(),
            })
        }
    }

    fn one_account(delay: Duration, frames: &Log) -> SourceFactory {
        let frames = frames.clone();
        Arc::new(move || {
            Box::new(OneAccount {
                delay,
                frames: frames.clone(),
            }) as Box<dyn Source>
        })
    }

    /// A source whose `accounts` read finds SIGTERM recorded in `signal` by the time it returns:
    /// the signal arrives while the first frame waits for it.
    fn signalled_during_the_first_read(signal: &Cancel) -> SourceFactory {
        let signal = signal.clone();
        Arc::new(move || {
            struct Signalled(Cancel);
            impl Source for Signalled {
                fn accounts(
                    &self,
                    _provider: Option<&ProviderId>,
                ) -> Result<AccountsSnapshot, SourceError> {
                    self.0.request(libc::SIGTERM);
                    Ok(crate::testutil::snapshot(crate::testutil::many(1)))
                }
                fn history(&self, _: &AccountId, _: i64) -> Result<HistoryView, SourceError> {
                    unreachable!("no history is read")
                }
                fn collect_due(&self, _: &[AccountId]) -> Result<CollectSummary, SourceError> {
                    unreachable!("nothing is collected")
                }
            }
            Box::new(Signalled(signal.clone())) as Box<dyn Source>
        })
    }

    /// Waits up to 5 s for `frames` to see `what`; whether it did.
    fn seen_within(frames: &Log, what: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !frames.seen().contains(&what) {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
        true
    }

    type Behaviour = Box<dyn Fn(&Action) -> ActionOutcome + Send + Sync>;

    /// A host whose actions are closures.
    struct Scripted {
        act: Behaviour,
        foreground: Behaviour,
    }

    impl Host for Scripted {
        fn act(&self, action: &Action, _ask: &mut dyn Ask) -> ActionOutcome {
            (self.act)(action)
        }
        fn act_foreground(&self, action: &Action) -> ActionOutcome {
            (self.foreground)(action)
        }
    }

    fn never() -> Behaviour {
        Box::new(|action| panic!("{action:?} was not expected to run"))
    }

    fn idle() -> Scripted {
        Scripted {
            act: never(),
            foreground: never(),
        }
    }

    fn done(exit: i32, out: &str, err: &str) -> ActionOutcome {
        ActionOutcome::Done {
            exit,
            out: out.into(),
            err: err.into(),
        }
    }

    fn options() -> TuiOptions {
        TuiOptions {
            screen: Screen::Accounts,
            color: false,
            provider: None,
            signal: Cancel::new(),
            shutdown: Cancel::new(),
        }
    }

    fn account_a() -> AccountRef {
        AccountRef {
            id: AccountId::from_string("0192-a"),
            provider: ProviderId::new("claude-code"),
            position: 1,
        }
    }

    /// The runtime's wiring around a fake terminal and input, with real lanes. `log` has what
    /// the terminal and the input were asked to do; `frames` has each frame, and what the
    /// sources record.
    struct Fixture {
        opts: TuiOptions,
        tx: Sender<Msg>,
        rx: Receiver<Msg>,
        lanes: Lanes,
        host: Arc<dyn Host>,
        log: Log,
        frames: Log,
        keys: Keys,
    }

    impl Fixture {
        fn new(sources: SourceFactory, host: impl FnOnce(&TuiOptions) -> Scripted) -> Fixture {
            Fixture::with(options(), sources, host)
        }

        fn with(
            opts: TuiOptions,
            sources: SourceFactory,
            host: impl FnOnce(&TuiOptions) -> Scripted,
        ) -> Fixture {
            let host: Arc<dyn Host> = Arc::new(host(&opts));
            let (tx, rx) = mpsc::channel();
            let lanes = Lanes::spawn(None, &sources, &host, &tx, &opts.shutdown);
            let log = Log::default();
            Fixture {
                opts,
                tx,
                rx,
                lanes,
                host,
                keys: Keys {
                    log: log.clone(),
                    stuck: Arc::default(),
                },
                log,
                frames: Log::default(),
            }
        }

        fn ui(&self) -> Ui<'_, FakeTty, Keys> {
            Ui::new(
                &self.opts,
                FakeTty::new(self.log.clone(), self.frames.clone()),
                self.keys.clone(),
                &self.rx,
                &self.lanes,
                self.host.as_ref(),
            )
        }

        fn key(&self, c: char) {
            let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
            self.tx.send(Msg::Key(key)).unwrap();
        }

        fn finish(self, ending: Ending) -> TuiExit {
            finish(self.lanes, &self.opts.shutdown, &self.rx, ending, || {})
        }
    }

    #[test]
    fn the_first_frame_waits_for_the_first_accounts_read() {
        let fx = Fixture::new(quick(), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            assert!(
                ui.state.accounts.snapshot().is_some(),
                "drawn with the accounts, not `loading…`"
            );
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn a_slow_first_read_is_drawn_after_100_ms_without_it() {
        let fx = Fixture::new(
            sources(Duration::from_millis(600), false, &Arc::default()),
            |_| idle(),
        );
        let ending = {
            let mut ui = fx.ui();
            let started = Instant::now();
            ui.start();
            let took = started.elapsed();
            assert!(
                took >= FIRST_FRAME && took < Duration::from_millis(500),
                "{took:?}"
            );
            assert!(ui.state.accounts.snapshot().is_none());
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn the_first_collection_is_asked_for_after_the_first_frame() {
        // Spec §6.1: the TUI collects after drawing, not a timer's 30 s later.
        let frames = Log::default();
        let mut fx = Fixture::new(one_account(Duration::ZERO, &frames), |_| idle());
        fx.frames = frames;
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            seen_within(&fx.frames, "collect");
            fx.key('q');
            ui.run()
        };
        let seen = fx.frames.seen();
        assert_eq!(seen[..2], ["draw", "collect"], "{seen:?}");
        assert_eq!(seen.iter().filter(|&&e| e == "collect").count(), 1);
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn accounts_that_come_after_the_loading_frame_are_collected_after_their_own_frame() {
        let frames = Log::default();
        let mut fx = Fixture::new(one_account(Duration::from_millis(300), &frames), |_| idle());
        fx.frames = frames;
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            assert_eq!(fx.frames.seen(), ["draw"], "`loading…`, and no collection");
            let (frames, tx) = (fx.frames.clone(), fx.tx.clone());
            // `q` once the collection came, or once it is clear it will not: the loop ends.
            let quit = thread::spawn(move || {
                seen_within(&frames, "collect");
                let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
                tx.send(Msg::Key(q)).unwrap();
            });
            let ending = ui.run();
            quit.join().unwrap();
            ending
        };
        let seen = fx.frames.seen();
        let collect = seen.iter().position(|&e| e == "collect");
        let collect = collect.unwrap_or_else(|| panic!("nothing was collected: {seen:?}"));
        assert!(
            seen[..collect].iter().filter(|&&e| e == "draw").count() >= 2,
            "the loading frame, then the accounts': {seen:?}"
        );
        assert_eq!(seen.iter().filter(|&&e| e == "collect").count(), 1);
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn q_with_idle_lanes_joins_them_promptly_and_exits_0() {
        let drops = Arc::default();
        let fx = Fixture::new(sources(Duration::ZERO, false, &drops), |_| idle());
        let shutdown = fx.opts.shutdown.clone();
        let started = Instant::now();
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.key('q');
            ui.run()
        };
        let exit = fx.finish(ending);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!((exit.code, exit.report), (0, None));
        assert_eq!(
            drops.load(Ordering::SeqCst),
            2,
            "both lanes ended before the TUI returned"
        );
        assert_eq!(
            shutdown.requested(),
            Some(libc::SIGINT),
            "§10.2 step 1: SIGINT for q"
        );
    }

    #[test]
    fn q_while_an_action_runs_waits_for_it_and_reports_what_it_did() {
        const SPAN: Duration = Duration::from_millis(300);
        const SWITCHED: &str = "Switched to a@x.co (position 1).\n";
        const TOO_LATE: &str =
            "tagteam: interrupted too late to stop: switch had already finished\n";
        let started = Arc::new(AtomicBool::new(false));
        let running = Arc::clone(&started);
        let fx = Fixture::new(quick(), move |opts| {
            let shutdown = opts.shutdown.clone();
            Scripted {
                // Inside its critical span when the shutdown begins, so it runs to the end
                // (core §14.1) and reports itself switched.
                act: Box::new(move |_| {
                    running.store(true, Ordering::SeqCst);
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while shutdown.requested().is_none() && Instant::now() < deadline {
                        thread::sleep(Duration::from_millis(5));
                    }
                    thread::sleep(SPAN);
                    done(0, SWITCHED, TOO_LATE)
                }),
                foreground: never(),
            }
        });
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            let switch = Action::Switch(account_a());
            ui.state.running = Some(switch.clone());
            ui.execute(vec![Cmd::Act(switch)]);
            let deadline = Instant::now() + Duration::from_secs(5);
            while !started.load(Ordering::SeqCst) {
                assert!(Instant::now() < deadline, "the action never started");
                thread::sleep(Duration::from_millis(5));
            }
            fx.key('q');
            let ending = ui.run();
            assert!(
                ui.tty.text().contains("finishing switch"),
                "{}",
                ui.tty.text()
            );
            ending
        };
        let joining = Instant::now();
        let exit = fx.finish(ending);
        assert!(
            joining.elapsed() >= SPAN - Duration::from_millis(50),
            "the actions lane was joined: {:?}",
            joining.elapsed()
        );
        assert_eq!(exit.code, 0);
        assert_eq!(
            exit.report.as_deref(),
            Some(format!("{SWITCHED}{TOO_LATE}").as_str())
        );
    }

    #[test]
    fn a_sigterm_is_seen_within_one_wake_and_ends_with_143() {
        let fx = Fixture::new(quick(), |_| idle());
        let (signal, shutdown) = (fx.opts.signal.clone(), fx.opts.shutdown.clone());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            let sender = thread::spawn(move || {
                thread::sleep(Duration::from_millis(50));
                signal.request(libc::SIGTERM);
            });
            let started = Instant::now();
            let ending = ui.run();
            sender.join().unwrap();
            assert!(
                started.elapsed() < Duration::from_millis(600),
                "{:?}",
                started.elapsed()
            );
            ending
        };
        assert_eq!(
            fx.log.take(),
            ["pause", "release"],
            "a signal restores the terminal first (§10.2 step 4)"
        );
        let exit = fx.finish(ending);
        assert_eq!((exit.code, exit.report), (143, None));
        assert_eq!(shutdown.requested(), Some(libc::SIGTERM));
    }

    #[test]
    fn a_signal_that_arrives_during_the_wait_comes_before_the_key_that_ends_it() {
        // SIGTERM is recorded while the loop waits, then `q` wakes it: the shutdown is the
        // signal's (143, the terminal restored first), not `q`'s.
        let fx = Fixture::new(quick(), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            // After the loop's look at the token and its frame, before its wait ends.
            let (signal, tx) = (fx.opts.signal.clone(), fx.tx.clone());
            ui.tty.on_draw = Some(Box::new(move || {
                signal.request(libc::SIGTERM);
                let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
                tx.send(Msg::Key(q)).unwrap();
            }));
            ui.state.dirty = true;
            ui.run()
        };
        assert_eq!(fx.log.take(), ["pause", "release"]);
        let exit = fx.finish(ending);
        assert_eq!((exit.code, exit.report), (143, None));
    }

    #[test]
    fn an_outcome_that_arrives_with_a_signal_is_reported_after_the_tui() {
        const SWITCHED: &str = "Switched to a@x.co (position 1).\n";
        let fx = Fixture::new(quick(), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            ui.state.running = Some(Action::Switch(account_a()));
            let (signal, tx) = (fx.opts.signal.clone(), fx.tx.clone());
            ui.tty.on_draw = Some(Box::new(move || {
                signal.request(libc::SIGTERM);
                tx.send(Msg::ActionDone {
                    action: Action::Switch(account_a()),
                    outcome: done(0, SWITCHED, ""),
                })
                .unwrap();
            }));
            ui.state.dirty = true;
            ui.run()
        };
        let exit = fx.finish(ending);
        assert_eq!((exit.code, exit.report.as_deref()), (143, Some(SWITCHED)));
    }

    #[test]
    fn a_signal_during_the_first_frames_wait_comes_before_the_first_read() {
        let opts = options();
        let sources = signalled_during_the_first_read(&opts.signal);
        let fx = Fixture::with(opts, sources, |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            assert!(
                ui.state.accounts.snapshot().is_none(),
                "the read is not applied"
            );
            ui.run()
        };
        assert_eq!(fx.frames.seen(), Vec::<&str>::new(), "no frame was drawn");
        assert_eq!(fx.log.take(), ["pause", "release"]);
        assert_eq!(fx.finish(ending).code, 143);
    }

    #[test]
    fn a_sigint_sent_to_the_process_ends_with_130() {
        // In raw mode Ctrl-C is a key, so a SIGINT came from `kill -INT` (§10.2).
        let fx = Fixture::new(quick(), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.opts.signal.request(libc::SIGINT);
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 130);
    }

    #[test]
    fn a_sigint_left_by_a_suspended_action_is_taken_and_the_tui_carries_on() {
        let fx = Fixture::new(quick(), |opts| {
            let signal = opts.signal.clone();
            Scripted {
                act: never(),
                // Ctrl-C in the session: the terminal sent it to the TUI as well (§7.3).
                foreground: Box::new(move |_| {
                    signal.request(libc::SIGINT);
                    done(130, "", "")
                }),
            }
        });
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            let run = Action::Run(account_a());
            ui.state.running = Some(run.clone());
            ui.execute(vec![Cmd::Suspend(run)]);
            assert_eq!(
                fx.log.take(),
                ["pause", "release", "reclaim", "resume"],
                "§7.3 steps 1, 2 and 4"
            );
            assert_eq!(
                fx.opts.signal.requested(),
                None,
                "the action's SIGINT is spent"
            );
            assert!(ui.ending.is_none(), "the TUI carries on");
            assert!(
                ui.state.overlay.is_none() && ui.state.running.is_none(),
                "R12: the outcome shows nothing, so `q` is not an overlay's key (R9)"
            );
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn a_sigterm_during_a_suspension_shuts_down_without_taking_the_terminal_back() {
        let fx = Fixture::new(quick(), |opts| {
            let signal = opts.signal.clone();
            Scripted {
                act: never(),
                foreground: Box::new(move |_| {
                    signal.request(libc::SIGTERM);
                    done(143, "", "")
                }),
            }
        });
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            ui.execute(vec![Cmd::Suspend(Action::Run(account_a()))]);
            assert_eq!(fx.log.take(), ["pause", "release"]);
            ui.run()
        };
        let exit = fx.finish(ending);
        assert_eq!((exit.code, exit.report), (143, None));
    }

    #[test]
    fn a_sigterm_that_replaced_the_actions_sigint_still_ends_the_tui_with_143() {
        let fx = Fixture::new(quick(), |opts| {
            let signal = opts.signal.clone();
            Scripted {
                act: never(),
                // Ctrl-C in the session, then a `kill` of the TUI before it looks.
                foreground: Box::new(move |_| {
                    signal.request(libc::SIGINT);
                    signal.request(libc::SIGTERM);
                    done(130, "", "")
                }),
            }
        });
        let signal = fx.opts.signal.clone();
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            ui.execute(vec![Cmd::Suspend(Action::Run(account_a()))]);
            assert_eq!(fx.log.take(), ["pause", "release"]);
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 143);
        assert_eq!(signal.requested(), Some(libc::SIGTERM), "not spent");
    }

    #[test]
    fn a_suspension_is_refused_while_the_keys_do_not_pause() {
        // The input thread may still read: the action does not run (`never` would panic), and
        // the TUI keeps the terminal and says why.
        let fx = Fixture::new(quick(), |_| idle());
        fx.keys.stuck.store(true, Ordering::SeqCst);
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            let run = Action::Run(account_a());
            ui.state.running = Some(run.clone());
            ui.execute(vec![Cmd::Suspend(run)]);
            assert_eq!(fx.log.take(), ["pause", "resume"], "never handed over");
            assert!(ui.ending.is_none() && ui.state.running.is_none());
            ui.draw();
            assert!(
                ui.tty.text().contains(
                    "run did not start: the TUI's key reader did not pause in time; try again"
                ),
                "{}",
                ui.tty.text()
            );
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn ctrl_z_is_refused_while_the_keys_do_not_pause() {
        let fx = Fixture::new(quick(), |_| idle());
        fx.keys.stuck.store(true, Ordering::SeqCst);
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            ui.execute(vec![Cmd::Stop]);
            assert_eq!(fx.log.take(), ["pause", "resume"], "never stopped");
            assert_eq!(
                ui.state.banner.as_deref(),
                Some(
                    "Ctrl-Z did not suspend: the TUI's key reader did not pause in time; try again"
                )
            );
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn a_signal_gives_the_terminal_back_even_while_the_keys_do_not_pause() {
        // The process is exiting: the terminal is restored all the same.
        let fx = Fixture::new(quick(), |_| idle());
        fx.keys.stuck.store(true, Ordering::SeqCst);
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            fx.opts.signal.request(libc::SIGTERM);
            ui.run()
        };
        assert_eq!(fx.log.take(), ["pause", "release"]);
        assert_eq!(fx.finish(ending).code, 143);
    }

    #[test]
    fn ctrl_z_hands_the_terminal_back_and_takes_it_again() {
        let fx = Fixture::new(quick(), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            ui.execute(vec![Cmd::Stop]);
            assert_eq!(fx.log.take(), ["pause", "stop", "resume"]);
            assert!(
                ui.ending.is_none() && ui.state.dirty,
                "redrawn, and carrying on"
            );
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn a_lane_that_dies_ends_the_tui_with_exit_1_and_says_so() {
        let fx = Fixture::new(sources(Duration::ZERO, true, &Arc::default()), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            ui.run()
        };
        let exit = fx.finish(ending);
        assert_eq!(exit.code, 1);
        assert_eq!(
            exit.report.as_deref(),
            Some("tagteam: the TUI's views lane panicked; the log has the details\n")
        );
    }

    #[test]
    fn a_panic_on_the_ui_thread_still_joins_every_lane_and_exits_1() {
        let drops = Arc::default();
        let opts = options();
        let shutdown = opts.shutdown.clone();
        let exit = run_with(
            opts,
            sources(Duration::ZERO, false, &drops),
            Arc::new(idle()),
            || {
                Ok(FakeTty {
                    panics: true,
                    ..FakeTty::new(Log::default(), Log::default())
                })
            },
            |_tx| Ok(Keys::default()),
            || {},
        );
        assert_eq!((exit.code, exit.report), (1, None));
        assert_eq!(drops.load(Ordering::SeqCst), 2, "both lanes were joined");
        assert_eq!(shutdown.requested(), Some(libc::SIGINT));
    }
}
