//! The UI thread (TUI spec §4.3): the lanes it drives, the loop that runs `update` and draws
//! `view`, the suspension that hands the terminal to a foreground action (§7.3), and the one
//! shutdown every way out goes through (§10.2).

use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jiff::tz::TimeZone;
use tagteam_core::ProviderId;
use tagteam_engine::views::ProviderNames;
use tagteam_provider::Cancel;

use crate::ask::Answers;
use crate::host::{Action, ActionOutcome, Host};
use crate::input::{Input, Pause};
use crate::lanes::{Lanes, StatusFetch};
use crate::live::{self, Backoff};
use crate::msg::{ActionsReq, Cmd, DaemonReq, IndexReq, Msg, ReportReq, ViewsReq};
use crate::screens::tokens::TokensState;
use crate::source::SourceFactory;
use crate::state::{Screen, Startup, State};
use crate::term::{self, RealTty, Tty};
use crate::theme::Theme;
use crate::update::{drawn, update};
use crate::view::view;

/// The loop wakes at least this often, to look at the signal token and to tick (§4.3).
pub(crate) const WAKE: Duration = Duration::from_millis(250);
/// How long the first frame waits for the first `accounts` (§6.1).
const FIRST_FRAME: Duration = Duration::from_millis(100);
/// C-R17: the most messages a wake applies before the loop draws, so a burst is one frame and
/// the frames still come under a stream that never ends.
const BATCH: usize = 256;
/// Why a hand-over of the terminal was refused (§7.3 step 1): the input thread could still read
/// keys typed for the terminal's next owner.
const NOT_PAUSED: &str = "the TUI's key reader did not pause in time; try again";

/// How the TUI runs.
pub struct TuiOptions {
    pub screen: Screen,
    pub color: bool,
    /// `COLORTERM` says the terminal draws 24-bit colour: with `color`, detection may take the
    /// theme to tier 2 (spec §4.2).
    pub truecolor: bool,
    pub provider: Option<ProviderId>,
    /// The provider Tokens reports on: `--provider`'s, else the default (spec §5.3).
    pub tokens_provider: ProviderId,
    /// The zone of Tokens' days and months: the system zone (spec §5.3), read by the binary
    /// before the terminal is touched, so the UI thread reads no zone file (B6).
    pub tz: TimeZone,
    /// The process's signal token: the CLI's handlers record SIGINT, SIGTERM and SIGHUP in it.
    pub signal: Cancel,
    /// The shutdown token. The binary has already built every lane's engine, and the host's
    /// in-TUI context, with it.
    pub shutdown: Cancel,
    /// The socket of the daemon of the store the TUI reads (`daemon_socket_path`, D5.1), which
    /// it follows (spec §8); `None` for timer mode alone.
    pub daemon: Option<PathBuf>,
    /// The default provider's names (redesign §12.2): the screens' words for a provider the
    /// accounts view does not list, as before its first read.
    pub names: ProviderNames,
    /// The settings warnings the CLI printed before the alternate screen (redesign §5.6): shown
    /// as a banner until `Esc` on Accounts, or the next config change.
    pub warnings: Vec<String>,
    /// `config.toml`'s modification time, read before the settings were loaded
    /// (`Settings::mtime`): an accounts read that sees another one clears `warnings`.
    pub config_mtime: Option<SystemTime>,
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
    let detect = opts.color && opts.truecolor;
    run_with(
        opts,
        sources,
        host,
        RealTty::enter,
        move |tx| Input::spawn(tx, detect),
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
    // The lanes build their engines while the terminal is set up (§6.1), and the live thread
    // connects meanwhile. Neither is waited for: the TUI starts in timer mode (§8).
    let status = opts
        .daemon
        .clone()
        .map(|path| -> StatusFetch { Arc::new(move || live::fetch_status(&path)) });
    let answers = Answers::default();
    let lanes = Lanes::spawn(
        opts.provider.clone(),
        &sources,
        &host,
        &tx,
        &opts.shutdown,
        &answers,
        status,
    );
    if let Some(path) = opts.daemon.clone() {
        let died = lanes.stray_death("live", &tx);
        // A thread that cannot start leaves the TUI in timer mode.
        let _ = live::spawn(path, Backoff::default(), Arc::clone(&lanes.live), died);
    }
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
        let mut ui = Ui::new(&opts, tty, input, &rx, &lanes, host.as_ref(), &answers);
        ui.start();
        ui.run()
    }));
    // A panic on the UI thread: the hook has restored the terminal and logged it (§10.1), and
    // the lanes still drain (§10.2). `begin_shutdown` never ran, so a prompt still waiting is
    // declined here (§10.2 step 3); on every other path it already was.
    let ending = driven.unwrap_or_else(|_| {
        answers.decline_all();
        Ending {
            code: 1,
            fatal: None,
            report: String::new(),
        }
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

/// What a test runs once inside `Ui::start`'s first wait, after the token was looked at and
/// before the wait begins (§6.1).
#[cfg(test)]
type FirstWaitHook = Box<dyn FnOnce()>;

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
    /// The senders of the CLI prompts still waiting; `State` holds only their ids.
    answers: Answers,
    /// Set when the shutdown begins.
    ending: Option<Ending>,
    next_tick: Instant,
    /// Run once, in the first wait, so that a test puts a signal exactly between the token's
    /// look and the wait.
    #[cfg(test)]
    before_first_wait: Option<FirstWaitHook>,
}

impl<'a, T: Tty, P: Pause> Ui<'a, T, P> {
    pub(crate) fn new(
        opts: &TuiOptions,
        tty: T,
        input: P,
        rx: &'a Receiver<Msg>,
        lanes: &'a Lanes,
        host: &'a dyn Host,
        answers: &Answers,
    ) -> Self {
        let tokens = TokensState::new(opts.tokens_provider.clone(), opts.tz.clone());
        let now = now_ms();
        let mut state = State::new(
            opts.screen,
            Theme::new(opts.color),
            opts.provider.clone(),
            tokens,
            opts.names,
            Startup {
                warnings: opts.warnings.clone(),
                mtime: opts.config_mtime,
            },
            now,
        );
        if opts.daemon.is_some() {
            // C-R20: the first collection waits to know who collects.
            state.daemon.await_daemon(now);
        }
        Ui {
            state,
            tty,
            input,
            rx,
            lanes,
            host,
            signal: opts.signal.clone(),
            shutdown: opts.shutdown.clone(),
            answers: answers.clone(),
            ending: None,
            next_tick: Instant::now() + WAKE,
            #[cfg(test)]
            before_first_wait: None,
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
            #[cfg(test)]
            if let Some(hook) = self.before_first_wait.take() {
                hook();
            }
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
        // A wait that ran out looks at the token too, before the first frame (R17).
        self.shutting_down();
        if self.ending.is_none() {
            self.state.dirty = true;
            self.draw();
        }
    }

    /// §4.3, until a shutdown begins. Each wake looks at the signal token before anything else
    /// (§10.2): before the wait, and again before each message or the tick that ended it. It
    /// redraws when the state changed, applies every message waiting (`BATCH` at most, C-R17),
    /// and ticks every 250 ms whatever arrives.
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
                Ok(msg) => {
                    self.deliver(msg);
                    self.drain();
                }
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

    /// C-R17: the messages already waiting, after the one that woke the loop, up to `BATCH` in
    /// all, so that a burst is drawn once and a key behind it waits for no frame of its own.
    fn drain(&mut self) {
        for _ in 1..BATCH {
            if self.ending.is_some() {
                return;
            }
            match self.rx.try_recv() {
                Ok(msg) => self.deliver(msg),
                Err(_) => return,
            }
        }
    }

    /// Applies `msg`, after a look at the signal token: a signal that arrived with it comes
    /// first, and so does a shutdown already begun. Then `msg` is not applied; an action's
    /// outcome in it still goes into the report (§10.2 step 6). `LiveReady` applies everything
    /// in the live queue, each thing as a message of its own (C-R17).
    fn deliver(&mut self, msg: Msg) {
        if let Msg::LiveReady = msg {
            for live in self.lanes.live.take() {
                self.deliver(Msg::Live(live));
            }
            return;
        }
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
                Cmd::LoadCard {
                    r#gen,
                    account,
                    since_s,
                } => {
                    let req = ViewsReq::Card {
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
                Cmd::LoadReport { r#gen, query } => {
                    let _ = self.lanes.report.submit(ReportReq::Report { r#gen, query });
                }
                Cmd::LoadIndexState { r#gen, provider } => {
                    let req = ReportReq::IndexState { r#gen, provider };
                    let _ = self.lanes.report.submit(req);
                }
                Cmd::RefreshIndex { provider } => {
                    let _ = self.lanes.index.submit(IndexReq::Refresh { provider });
                }
                Cmd::LoadInventory { r#gen, query } => {
                    let req = ReportReq::Inventory { r#gen, query };
                    let _ = self.lanes.report.submit(req);
                }
                Cmd::Act(action) => {
                    let _ = self.lanes.actions.submit(ActionsReq::Act { action });
                }
                Cmd::Suspend(action) => self.suspend(action),
                Cmd::Quit { code } => self.begin_shutdown(code, libc::SIGINT, None),
                Cmd::Stop => self.stop(),
                Cmd::FetchStatus { link } => {
                    if let Some(daemon) = &self.lanes.daemon {
                        let _ = daemon.submit(DaemonReq::Status { link });
                    }
                }
                Cmd::DropLink { link } => self.lanes.live.end_link(link),
                Cmd::Answer { to, answer } => self.answers.answer(to, answer),
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
            // begun there is nothing more to do about it. A hang-up is what made it fail, and
            // exits 128 + the signal: the token is looked at first.
            Err(e) => {
                if !self.shutting_down() {
                    self.begin_shutdown(
                        1,
                        libc::SIGINT,
                        Some(format!("tagteam: the terminal could not be written: {e}")),
                    );
                }
            }
        }
    }

    /// §10.2 steps 1 to 4. A second trigger, a second `q` among them, changes nothing.
    fn begin_shutdown(&mut self, code: i32, signal: i32, fatal: Option<String>) {
        if self.ending.is_some() {
            return;
        }
        self.shutdown.request(signal);
        self.answers.decline_all();
        self.lanes.close();
        self.apply(Msg::ShutdownBegan { code });
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
            self.hand_back(signal);
        }
    }

    /// §10.2 step 4 for a signal or a fatal error. The input thread is asked to pause, and the
    /// terminal is given back whether it says so or not: the process is exiting. After a SIGHUP
    /// the terminal is gone, its reader may never answer, and nothing waits for it.
    fn hand_back(&mut self, signal: i32) {
        if self.tty.active() {
            if signal == libc::SIGHUP {
                self.input.hang_up();
            } else {
                let _ = self.input.pause();
            }
            self.tty.release();
        }
    }

    /// §7.3 step 1, before the terminal is handed over: true once the input thread has said it
    /// reads no more. Otherwise the pause is withdrawn, the terminal kept, and the banner says
    /// that `action` did not start, or, for `None`, that Ctrl-Z did not suspend, and why.
    fn paused(&mut self, action: Option<&Action>) -> bool {
        if self.input.pause() {
            return true;
        }
        self.input.resume();
        self.apply(Msg::SuspendRefused {
            action: action.cloned(),
            why: NOT_PAUSED,
        });
        false
    }

    /// §7.3: runs `action` with the terminal handed back, then takes the terminal again. When
    /// the input thread does not pause, nothing runs and the TUI keeps the terminal.
    fn suspend(&mut self, action: Action) {
        if !self.paused(Some(&action)) {
            return;
        }
        self.tty.release();
        let outcome = self.host.act_foreground(&action);
        // A SIGINT recorded by now was the action's: Ctrl-C at its prompt or in its session
        // (§7.3). Only a SIGINT is spent, in one step, so a SIGTERM or SIGHUP that replaced it
        // stays recorded.
        self.signal.take_if(libc::SIGINT);
        if let Some(signal) = self.signal.requested() {
            // SIGTERM or SIGHUP: the shutdown, with the terminal as the action left it. The
            // outcome the shutdown overtakes goes into the report (R17).
            self.begin_shutdown(128 + signal, signal, None);
            self.report(action, outcome);
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
            self.report(action, outcome);
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

    /// Adds an outcome the shutdown overtook to the report (§10.2 step 6).
    fn report(&mut self, action: Action, outcome: ActionOutcome) {
        if let Some(ending) = &mut self.ending {
            ending
                .report
                .push_str(&reported(Msg::ActionDone { action, outcome }));
        }
    }

    /// Ctrl-Z (§10.1): the terminal is given back while the process is stopped, and taken
    /// again, and redrawn, once it is continued. Refused, as a suspension is, when the input
    /// thread does not pause.
    fn stop(&mut self) {
        if !self.paused(None) {
            return;
        }
        match self.tty.stop() {
            Ok(()) => {
                // A signal that came while the process was stopped comes first (R17).
                if self.shutting_down() {
                    return;
                }
                self.input.resume();
                self.resized();
                self.state.dirty = true;
            }
            // A window closed while stopped is a SIGHUP with the SIGCONT: the token first, so
            // it exits with the signal, as a failed write does (`draw`).
            Err(e) => {
                if !self.shutting_down() {
                    self.begin_shutdown(
                        1,
                        libc::SIGINT,
                        Some(format!(
                            "tagteam: the terminal could not be set up again: {e}"
                        )),
                    );
                }
            }
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
    use tagteam_daemon::client::ClientError;
    use tagteam_engine::tokens::{
        IndexProgress, IndexState, RefreshOutcome, TokenPeriod, TokenQuery, TokenReport,
    };
    use tagteam_engine::views::HistoryView;

    use super::*;
    use crate::host::{AccountRef, Ask};
    use crate::live::StatusError;
    use crate::live::{LIVE_QUEUE, Live, LiveQueue};
    use crate::source::{AccountsSnapshot, CollectSummary, Source, SourceError};
    use crate::state::BannerKind;
    use crate::term::repaint;
    use crate::testutil::CC_NAMES;
    use crate::theme::Background;

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
        fn hang_up(&self) {
            self.log.push("hang_up");
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
                in_run_shell: false,
                read_at_s: 0,
                auto_holders: vec![],
                config_mtime: None,
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
                updated: 0,
                rate_limited: vec![],
            })
        }

        fn refresh_tokens(
            &self,
            _provider: &ProviderId,
            _progress: &mut dyn FnMut(&IndexProgress),
        ) -> Result<RefreshOutcome, SourceError> {
            Ok(RefreshOutcome::Busy)
        }

        fn token_report(&self, _query: &TokenQuery) -> Result<TokenReport, SourceError> {
            Err(no_tokens())
        }

        fn token_index_state(&self, _provider: &ProviderId) -> Result<IndexState, SourceError> {
            Ok(IndexState::default())
        }
    }

    /// What a test source with no token report answers a report query with.
    fn no_tokens() -> SourceError {
        SourceError {
            kind: "test".into(),
            message: "no token report in this test".into(),
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
                updated: 0,
                rate_limited: vec![],
            })
        }

        fn refresh_tokens(
            &self,
            _provider: &ProviderId,
            _progress: &mut dyn FnMut(&IndexProgress),
        ) -> Result<RefreshOutcome, SourceError> {
            Ok(RefreshOutcome::Busy)
        }

        fn token_report(&self, _query: &TokenQuery) -> Result<TokenReport, SourceError> {
            Err(no_tokens())
        }

        fn token_index_state(&self, _provider: &ProviderId) -> Result<IndexState, SourceError> {
            Ok(IndexState::default())
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

    /// A source with one account whose `history` read has a sample of each window every 20
    /// minutes over the last 7 days, by the real clock, at the reading's percentage.
    struct Carded;

    /// How many samples `Carded` gives each window.
    const CARD_SAMPLES: usize = 7 * 72;

    impl Source for Carded {
        fn accounts(
            &self,
            _provider: Option<&ProviderId>,
        ) -> Result<AccountsSnapshot, SourceError> {
            Ok(crate::testutil::snapshot(crate::testutil::many(1)))
        }

        fn history(&self, account: &AccountId, _since_s: i64) -> Result<HistoryView, SourceError> {
            let v = crate::testutil::many(1).remove(0);
            assert_eq!(&v.row.id, account, "the card's account");
            let now_s = now_ms().div_euclid(1000);
            let windows = v
                .usage
                .windows
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|(w, pace)| {
                    let samples = (0..CARD_SAMPLES as i64)
                        .map(|i| tagteam_core::Sample {
                            fetched_at: now_s - i * 1200 - 600,
                            pct: w.pct,
                            resets_at: None,
                        })
                        .collect();
                    crate::testutil::history_window(w, samples, pace)
                })
                .collect();
            Ok(crate::testutil::history_of(v, windows))
        }

        fn collect_due(&self, _accounts: &[AccountId]) -> Result<CollectSummary, SourceError> {
            Ok(CollectSummary {
                fetched: 0,
                warnings: Vec::new(),
                updated: 0,
                rate_limited: vec![],
            })
        }

        fn refresh_tokens(
            &self,
            _provider: &ProviderId,
            _progress: &mut dyn FnMut(&IndexProgress),
        ) -> Result<RefreshOutcome, SourceError> {
            Ok(RefreshOutcome::Busy)
        }

        fn token_report(&self, _query: &TokenQuery) -> Result<TokenReport, SourceError> {
            Err(no_tokens())
        }

        fn token_index_state(&self, _provider: &ProviderId) -> Result<IndexState, SourceError> {
            Ok(IndexState::default())
        }
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
                fn refresh_tokens(
                    &self,
                    _: &ProviderId,
                    _: &mut dyn FnMut(&IndexProgress),
                ) -> Result<RefreshOutcome, SourceError> {
                    unreachable!("the token index is refreshed after the first frame")
                }
                fn token_report(&self, _: &TokenQuery) -> Result<TokenReport, SourceError> {
                    unreachable!("no token report is read")
                }
                fn token_index_state(&self, _: &ProviderId) -> Result<IndexState, SourceError> {
                    unreachable!("no token index state is read")
                }
            }
            Box::new(Signalled(signal.clone())) as Box<dyn Source>
        })
    }

    /// A source with one account whose index refresh runs until the shutdown token is set,
    /// telling its progress every 5 ms and logging `refresh` and `stopped` in `log`, while its
    /// report answers at once with no rows.
    struct CatchingUp {
        shutdown: Cancel,
        log: Log,
    }

    impl Source for CatchingUp {
        fn accounts(&self, _: Option<&ProviderId>) -> Result<AccountsSnapshot, SourceError> {
            Ok(crate::testutil::snapshot(crate::testutil::many(1)))
        }
        fn history(&self, _: &AccountId, _: i64) -> Result<HistoryView, SourceError> {
            Err(no_tokens())
        }
        fn collect_due(&self, _: &[AccountId]) -> Result<CollectSummary, SourceError> {
            Ok(CollectSummary {
                fetched: 0,
                warnings: Vec::new(),
                updated: 0,
                rate_limited: vec![],
            })
        }
        fn refresh_tokens(
            &self,
            _: &ProviderId,
            progress: &mut dyn FnMut(&IndexProgress),
        ) -> Result<RefreshOutcome, SourceError> {
            self.log.push("refresh");
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.shutdown.requested().is_none() && Instant::now() < deadline {
                progress(&IndexProgress::default());
                thread::sleep(Duration::from_millis(5));
            }
            self.log.push("stopped");
            Err(SourceError {
                kind: "interrupted".into(),
                message: "interrupted".into(),
            })
        }
        fn token_report(&self, query: &TokenQuery) -> Result<TokenReport, SourceError> {
            Ok(crate::testutil::token_report(query.clone(), vec![]))
        }
        fn token_index_state(&self, _: &ProviderId) -> Result<IndexState, SourceError> {
            Ok(IndexState::default())
        }
    }

    fn catching_up(shutdown: &Cancel, log: &Log) -> SourceFactory {
        let (shutdown, log) = (shutdown.clone(), log.clone());
        Arc::new(move || {
            Box::new(CatchingUp {
                shutdown: shutdown.clone(),
                log: log.clone(),
            }) as Box<dyn Source>
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
            truecolor: false,
            provider: None,
            tokens_provider: ProviderId::new("claude-code"),
            tz: TimeZone::UTC,
            signal: Cancel::new(),
            shutdown: Cancel::new(),
            daemon: None,
            names: CC_NAMES,
            warnings: vec![],
            config_mtime: None,
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
        answers: Answers,
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
            Fixture::build(opts, sources, host, None)
        }

        /// A TUI that follows a daemon (`TuiOptions::daemon`), with `host`, and a daemon lane
        /// whose `status` is `status`'s answer (spec §8). No live thread runs: a test puts what
        /// it would pass on in the lanes' live queue.
        fn with_daemon(
            sources: SourceFactory,
            host: impl FnOnce(&TuiOptions) -> Scripted,
            status: StatusFetch,
        ) -> Fixture {
            let opts = TuiOptions {
                daemon: Some(PathBuf::from("/nonexistent/daemon.sock")),
                ..options()
            };
            Fixture::build(opts, sources, host, Some(status))
        }

        fn build(
            opts: TuiOptions,
            sources: SourceFactory,
            host: impl FnOnce(&TuiOptions) -> Scripted,
            status: Option<StatusFetch>,
        ) -> Fixture {
            let host: Arc<dyn Host> = Arc::new(host(&opts));
            let (tx, rx) = mpsc::channel();
            let answers = Answers::default();
            let lanes = Lanes::spawn(None, &sources, &host, &tx, &opts.shutdown, &answers, status);
            let log = Log::default();
            Fixture {
                opts,
                tx,
                rx,
                lanes,
                host,
                answers,
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
                &self.answers,
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
    fn the_first_frame_draws_before_detection_and_the_next_in_tier_2() {
        let opts = TuiOptions {
            color: true,
            ..options()
        };
        let fx = Fixture::with(opts, quick(), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            assert_eq!(fx.frames.seen(), ["draw"], "the first frame, undetected");
            assert_eq!(ui.state.theme, Theme::new(true));
            let background = Some(Background::Dark);
            fx.tx.send(Msg::Detected { background }).unwrap();
            // What the lanes sent first is applied; none of it draws.
            while ui.state.theme == Theme::new(true) {
                let msg = fx.rx.recv_timeout(Duration::from_secs(5));
                ui.deliver(msg.expect("the detection arrived"));
            }
            assert_eq!(ui.state.theme, Theme::new(true).truecolor(Background::Dark));
            assert!(ui.state.dirty, "the next frame is due");
            let drawn = fx.frames.seen().len();
            let (tx, q) = (
                fx.tx.clone(),
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            );
            ui.tty.on_draw = Some(Box::new(move || tx.send(Msg::Key(q)).unwrap()));
            let ending = ui.run();
            // The frame in tier 2, and the one `q` ends on (§10.2 step 1).
            assert_eq!(fx.frames.seen().len(), drawn + 2);
            ending
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
            assert!(seen_within(&fx.frames, "collect"), "never collected");
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
            4,
            "every lane ended before the TUI returned"
        );
        assert_eq!(
            shutdown.requested(),
            Some(libc::SIGINT),
            "§10.2 step 1: SIGINT for q"
        );
    }

    #[test]
    fn the_card_s_history_read_comes_back_from_the_views_lane_and_draws_its_sparklines() {
        // T3-21: `Cmd::LoadCard` goes to the views lane, whose `Msg::Card` carries the source's
        // `history` answer for the selected account; the card draws its samples.
        let sources: SourceFactory = Arc::new(|| Box::new(Carded) as Box<dyn Source>);
        let fx = Fixture::new(sources, |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            let account = ui.state.accounts.selected().expect("an account").id;
            let since = now_ms().div_euclid(1000) - crate::screens::accounts::card::CARD_SINCE_S;
            let load = ui.state.load_card(account.clone(), since);
            ui.execute(vec![load]);
            loop {
                let msg = fx.rx.recv_timeout(Duration::from_secs(5));
                let msg = msg.expect("the views lane answered");
                if let Msg::Card {
                    account: of,
                    result,
                    ..
                } = &msg
                {
                    assert_eq!(of, &account);
                    let view = result.as_ref().expect("the source's history");
                    assert_eq!(view.windows.len(), 2);
                    assert!(view.windows.iter().all(|w| w.samples.len() == CARD_SAMPLES));
                    ui.deliver(msg);
                    break;
                }
                ui.deliver(msg);
            }
            ui.draw();
            let text = ui.tty.text();
            let rows: Vec<&str> = text
                .lines()
                .filter(|l| l.contains(" 24h ") || l.contains(" 7d "))
                .collect();
            assert_eq!(rows.len(), 2, "{text}");
            for row in rows {
                assert!(
                    row.chars().any(|c| ('▁'..='█').contains(&c)),
                    "a sparkline: {text}"
                );
            }
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn tokens_reads_its_report_on_the_report_lane() {
        let opts = TuiOptions {
            screen: Screen::Tokens,
            ..options()
        };
        let fx = Fixture::with(opts, quick(), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            // The first frame waited for the accounts only; the report may come after it.
            while ui.state.tokens.error().is_none() {
                let msg = fx.rx.recv_timeout(Duration::from_secs(5));
                ui.deliver(msg.expect("the report lane answered"));
            }
            assert_eq!(
                ui.state.tokens.error(),
                Some("couldn't read the token index (no token report in this test) · r retries")
            );
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn a_report_asked_for_while_the_index_catches_up_is_answered_and_q_stops_the_catch_up() {
        // Spec §4.3, §11: the index has a lane of its own, so a report asked for while a
        // catch-up runs is answered before the catch-up ends; the shutdown token then stops the
        // catch-up, and its lane is joined (§9, §10.2).
        let opts = TuiOptions {
            screen: Screen::Tokens,
            ..options()
        };
        let shutdown = opts.shutdown.clone();
        let log = Log::default();
        let fx = Fixture::with(opts, catching_up(&shutdown, &log), |_| idle());
        let started = Instant::now();
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            assert!(seen_within(&log, "refresh"), "the first frame started it");
            // Asked for only now that the catch-up runs: `p` reads the months.
            ui.apply(Msg::Key(KeyEvent::new(
                KeyCode::Char('p'),
                KeyModifiers::NONE,
            )));
            let asked = ui.state.gens.report;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                let msg = fx.rx.recv_timeout(left);
                let msg = msg.expect("the report lane answered while the catch-up ran");
                let answer = matches!(&msg, Msg::TokenReport { r#gen, .. } if *r#gen == asked);
                ui.deliver(msg);
                if answer {
                    break;
                }
            }
            assert_eq!(
                ui.state.tokens.report().map(|r| r.query.period),
                Some(TokenPeriod::Month)
            );
            assert_eq!(log.seen(), ["refresh"], "the catch-up still runs");
            assert!(ui.state.tokens.watch().refreshing().is_some());
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
        assert_eq!(log.seen(), ["refresh", "stopped"]);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_refresh_tells_the_loop_its_progress_at_most_every_250_ms() {
        let opts = options();
        let shutdown = opts.shutdown.clone();
        let log = Log::default();
        let fx = Fixture::with(opts, catching_up(&shutdown, &log), |_| idle());
        let provider = ProviderId::new("claude-code");
        assert!(fx.lanes.index.submit(IndexReq::Refresh { provider }));
        let started = Instant::now();
        let mut told = 0;
        while started.elapsed() < Duration::from_millis(600) {
            if let Ok(Msg::IndexProgress(_)) = fx.rx.recv_timeout(Duration::from_millis(10)) {
                told += 1;
            }
        }
        assert!(
            (2..=4).contains(&told),
            "{told} in 600 ms, of a call every 5 ms"
        );
        shutdown.request(libc::SIGINT);
        let ending = Ending {
            code: 0,
            fatal: None,
            report: String::new(),
        };
        assert_eq!(fx.finish(ending).code, 0);
        assert_eq!(log.seen(), ["refresh", "stopped"]);
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
                ui.state.banner_text(),
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
    fn a_hang_up_gives_the_terminal_back_without_waiting_for_the_keys() {
        let fx = Fixture::new(quick(), |_| idle());
        fx.keys.stuck.store(true, Ordering::SeqCst);
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            fx.opts.signal.request(libc::SIGHUP);
            ui.run()
        };
        assert_eq!(fx.log.take(), ["hang_up", "release"]);
        assert_eq!(fx.finish(ending).code, 129);
    }

    /// A terminal whose writes fail, as a hung-up one's do; the hang-up has been recorded by
    /// the time the write fails when `hangs_up` holds the token.
    struct Unwritable {
        log: Log,
        active: bool,
        hangs_up: Option<Cancel>,
    }

    impl Tty for Unwritable {
        type B = ratatui::backend::CrosstermBackend<io::Sink>;

        fn draw(&mut self, _render: impl FnOnce(&mut ratatui::Frame)) -> io::Result<bool> {
            if let Some(signal) = &self.hangs_up {
                signal.request(libc::SIGHUP);
            }
            Err(io::Error::from_raw_os_error(libc::EIO))
        }

        fn size(&mut self) -> io::Result<(u16, u16)> {
            Ok((120, 40))
        }

        fn active(&self) -> bool {
            self.active
        }

        fn release(&mut self) {
            self.log.push("release");
            self.active = false;
        }

        fn reclaim(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn stop(&mut self) -> io::Result<()> {
            // A window closed while stopped: SIGHUP, and the terminal cannot be taken again.
            if let Some(signal) = &self.hangs_up {
                signal.request(libc::SIGHUP);
            }
            Err(io::Error::from_raw_os_error(libc::EIO))
        }
    }

    /// The ending a failed frame write leads to.
    fn write_fails(fx: &Fixture, hangs_up: bool) -> Ending {
        let tty = Unwritable {
            log: fx.log.clone(),
            active: true,
            hangs_up: hangs_up.then(|| fx.opts.signal.clone()),
        };
        let mut ui = Ui::new(
            &fx.opts,
            tty,
            fx.keys.clone(),
            &fx.rx,
            &fx.lanes,
            fx.host.as_ref(),
            &fx.answers,
        );
        ui.state.dirty = true;
        ui.draw();
        ui.ending.take().expect("the failed write began a shutdown")
    }

    #[test]
    fn a_write_that_fails_because_of_a_hang_up_exits_with_the_signal_not_1() {
        let fx = Fixture::new(quick(), |_| idle());
        let ending = write_fails(&fx, true);
        let log = fx.log.clone();
        let exit = fx.finish(ending);
        assert_eq!((exit.code, exit.report), (129, None));
        assert_eq!(log.take(), ["hang_up", "release"]);
    }

    #[test]
    fn a_write_that_fails_alone_is_fatal_and_says_so() {
        let fx = Fixture::new(quick(), |_| idle());
        let ending = write_fails(&fx, false);
        let log = fx.log.clone();
        let exit = fx.finish(ending);
        assert_eq!(exit.code, 1);
        let report = exit.report.unwrap();
        assert!(
            report.starts_with("tagteam: the terminal could not be written:"),
            "{report}"
        );
        assert_eq!(log.take(), ["pause", "release"]);
    }

    /// The ending Ctrl-Z leads to on a terminal that cannot be taken again.
    fn stop_fails(fx: &Fixture, hangs_up: bool) -> Ending {
        let tty = Unwritable {
            log: fx.log.clone(),
            active: true,
            hangs_up: hangs_up.then(|| fx.opts.signal.clone()),
        };
        let mut ui = Ui::new(
            &fx.opts,
            tty,
            fx.keys.clone(),
            &fx.rx,
            &fx.lanes,
            fx.host.as_ref(),
            &fx.answers,
        );
        ui.execute(vec![Cmd::Stop]);
        ui.ending.take().expect("the failed stop began a shutdown")
    }

    #[test]
    fn a_shutdown_declines_a_waiting_prompt_at_once() {
        let fx = Fixture::new(quick(), |_| idle());
        let (tx, rx) = mpsc::channel();
        let ending = {
            let mut ui = fx.ui();
            ui.answers.keep(tx);
            ui.execute(vec![Cmd::Quit { code: 0 }]);
            assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected));
            ui.ending.take().expect("q began the shutdown")
        };
        fx.finish(ending);

        let fx = Fixture::new(quick(), |_| idle());
        let (tx, rx) = mpsc::channel();
        let ending = {
            let mut ui = fx.ui();
            ui.answers.keep(tx);
            ui.begin_shutdown(143, libc::SIGTERM, None);
            assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected));
            ui.ending.take().expect("a signal began the shutdown")
        };
        fx.finish(ending);
    }

    #[test]
    fn a_hang_up_while_stopped_exits_with_the_signal_not_1() {
        let fx = Fixture::new(quick(), |_| idle());
        let ending = stop_fails(&fx, true);
        let log = fx.log.clone();
        let exit = fx.finish(ending);
        assert_eq!((exit.code, exit.report), (129, None));
        assert_eq!(log.take(), ["pause", "hang_up", "release"]);
    }

    #[test]
    fn a_terminal_that_cannot_be_taken_again_after_ctrl_z_is_fatal_and_says_so() {
        let fx = Fixture::new(quick(), |_| idle());
        let ending = stop_fails(&fx, false);
        let exit = fx.finish(ending);
        assert_eq!(exit.code, 1);
        let report = exit.report.unwrap();
        assert!(
            report.starts_with("tagteam: the terminal could not be set up again:"),
            "{report}"
        );
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
    fn a_signal_during_a_first_wait_that_runs_out_draws_no_frame() {
        let fx = Fixture::new(
            sources(Duration::from_millis(600), false, &Arc::default()),
            |_| idle(),
        );
        let signal = fx.opts.signal.clone();
        let ending = {
            let mut ui = fx.ui();
            ui.before_first_wait = Some(Box::new(move || signal.request(libc::SIGTERM)));
            ui.start();
            assert!(ui.ending.is_some(), "the timeout looked at the token");
            ui.run()
        };
        assert_eq!(fx.frames.seen(), Vec::<&str>::new(), "no frame was drawn");
        assert_eq!(fx.finish(ending).code, 143);
    }

    #[test]
    fn the_outcome_of_a_suspension_a_sigterm_overtook_is_reported() {
        let fx = Fixture::new(quick(), |opts| {
            let signal = opts.signal.clone();
            Scripted {
                act: never(),
                foreground: Box::new(move |_| {
                    signal.request(libc::SIGTERM);
                    done(143, "out line\n", "err line\n")
                }),
            }
        });
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            ui.execute(vec![Cmd::Suspend(Action::Run(account_a()))]);
            ui.run()
        };
        let exit = fx.finish(ending);
        assert_eq!(
            (exit.code, exit.report.as_deref()),
            (143, Some("out line\nerr line\n"))
        );
    }

    #[test]
    fn a_refusal_banner_is_no_lane_death_and_the_next_good_read_clears_it() {
        let fx = Fixture::new(quick(), |_| idle());
        fx.keys.stuck.store(true, Ordering::SeqCst);
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            ui.state
                .raise_banner(BannerKind::Collect, "usage was not collected: earlier");
            ui.execute(vec![Cmd::Suspend(Action::Run(account_a()))]);
            assert!(ui.state.banner.is_some());
            assert_eq!(
                ui.state.banner.as_ref().map(|b| b.kind),
                Some(BannerKind::Refusal),
                "it owns no read's lifetime"
            );
            let r#gen = ui.state.gens.accounts;
            let snapshot = crate::testutil::snapshot(crate::testutil::many(1));
            ui.apply(Msg::Accounts {
                r#gen,
                result: Ok(snapshot),
            });
            assert!(ui.state.banner.is_none(), "a good read clears it (R18)");
            assert!(ui.ending.is_none() && ui.state.quitting.is_none());
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn a_foreground_action_that_asks_for_the_terminal_again_is_a_failure_not_a_loop() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let fx = Fixture::new(quick(), move |_| Scripted {
            act: never(),
            foreground: Box::new(move |_| {
                counted.fetch_add(1, Ordering::SeqCst);
                ActionOutcome::NeedsTerminal
            }),
        });
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            let run = Action::Run(account_a());
            ui.state.running = Some(run.clone());
            ui.execute(vec![Cmd::Suspend(run)]);
            assert_eq!(calls.load(Ordering::SeqCst), 1, "run once, not again");
            assert!(ui.state.running.is_none() && ui.ending.is_none());
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
        assert_eq!(drops.load(Ordering::SeqCst), 4, "every lane was joined");
        assert_eq!(shutdown.requested(), Some(libc::SIGINT));
    }

    /// Claude Code's job switching accounts, after `wait`; `started` is set when it begins.
    fn slow_status(wait: Duration, started: &Arc<AtomicBool>) -> StatusFetch {
        let started = Arc::clone(started);
        Arc::new(move || {
            started.store(true, Ordering::SeqCst);
            thread::sleep(wait);
            Ok(crate::testutil::daemon_status(
                vec![crate::testutil::daemon_job("claude-code", "auto", true)],
                vec![],
            ))
        })
    }

    /// Waits up to 5 s for `flag`.
    fn raised(flag: &AtomicBool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !flag.load(Ordering::SeqCst) {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
        true
    }

    #[test]
    fn the_first_frame_never_waits_for_the_daemon_and_its_status_starts_live_mode() {
        // Spec §6.1, §8: the TUI starts in timer mode and switches once the first `status`
        // arrives, however long the daemon takes.
        let started = Arc::new(AtomicBool::new(false));
        let status = slow_status(Duration::from_millis(400), &started);
        let fx = Fixture::with_daemon(quick(), |_| idle(), status);
        // The live thread subscribed before the TUI's first read returned.
        fx.tx.send(Msg::Live(Live::Up { link: 1 })).unwrap();
        let ending = {
            let mut ui = fx.ui();
            let begun = Instant::now();
            ui.start();
            assert!(
                begun.elapsed() < Duration::from_millis(300),
                "{:?}",
                begun.elapsed()
            );
            assert!(
                ui.state.accounts.snapshot().is_some(),
                "drawn with the accounts"
            );
            assert!(raised(&started), "the status was asked for at once");
            assert!(!ui.state.daemon.live(), "timer mode until it comes");
            while !ui.state.daemon.live() {
                let msg = fx.rx.recv_timeout(Duration::from_secs(5));
                ui.deliver(msg.expect("the daemon lane answered"));
            }
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn q_never_waits_for_a_status_the_daemon_lane_still_waits_for() {
        // Spec §4.3, §10.2: the daemon lane is not joined. Its `status` from a daemon that
        // never answers takes 2 s (D5.7); the shutdown does not wait for it.
        let started = Arc::new(AtomicBool::new(false));
        let status = slow_status(Duration::from_secs(3), &started);
        let fx = Fixture::with_daemon(quick(), |_| idle(), status);
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            ui.execute(vec![Cmd::FetchStatus { link: 1 }]);
            assert!(raised(&started), "the fetch began");
            fx.key('q');
            ui.run()
        };
        let joining = Instant::now();
        let exit = fx.finish(ending);
        assert!(
            joining.elapsed() < Duration::from_millis(500),
            "{:?}",
            joining.elapsed()
        );
        assert_eq!((exit.code, exit.report), (0, None));
    }

    #[test]
    fn a_daemon_lane_that_panics_ends_the_tui_with_1_and_says_so() {
        let status: StatusFetch = Arc::new(|| panic!("the daemon lane's fetch panicked"));
        let fx = Fixture::with_daemon(quick(), |_| idle(), status);
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            ui.execute(vec![Cmd::FetchStatus { link: 1 }]);
            ui.run()
        };
        let exit = fx.finish(ending);
        assert_eq!(exit.code, 1);
        assert_eq!(
            exit.report.as_deref(),
            Some("tagteam: the TUI's daemon lane panicked; the log has the details\n")
        );
    }

    /// The messages waiting on `rx`, applied as the loop applies them, until `done` holds; at
    /// most 5 s. How many of them woke the loop for the live queue.
    fn pump(
        ui: &mut Ui<'_, FakeTty, Keys>,
        rx: &Receiver<Msg>,
        done: impl Fn(&State) -> bool,
    ) -> usize {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut wakes = 0;
        while !done(&ui.state) {
            let left = deadline.saturating_duration_since(Instant::now());
            let msg = rx
                .recv_timeout(left)
                .expect("a message before the deadline");
            wakes += usize::from(matches!(msg, Msg::LiveReady));
            ui.deliver(msg);
        }
        wakes
    }

    #[test]
    fn a_flood_during_a_suspension_stays_within_the_queue_and_a_snapshot_recovers_it() {
        // C-R17: while `R`'s session has the terminal the loop takes no messages, and the live
        // thread goes on. Its backlog never passes `LIVE_QUEUE`, and it wakes the loop once. The
        // overflow is a disconnect; the next link's snapshot brings live mode back.
        let queue: Arc<std::sync::OnceLock<Arc<LiveQueue>>> = Arc::default();
        let flooding = Arc::clone(&queue);
        let started = Arc::new(AtomicBool::new(false));
        let status = slow_status(Duration::ZERO, &started);
        let fx = Fixture::with_daemon(
            quick(),
            move |_| Scripted {
                act: never(),
                // The live thread, while the session runs: a flood until the queue is full, which
                // drops the link; then the next link and its first events.
                foreground: Box::new(move |_| {
                    let q = flooding.get().expect("the queue is set");
                    let changed = || Live::Event(tagteam_daemon::protocol::Event::Changed);
                    let mut pushed = 0;
                    while q.push(changed()) == crate::live::Delivery::Taken {
                        pushed += 1;
                        assert!(q.len() <= LIVE_QUEUE, "{}", q.len());
                    }
                    assert_eq!(pushed, LIVE_QUEUE);
                    let up = q.push(Live::Up { link: 2 });
                    assert_eq!(up, crate::live::Delivery::Taken);
                    for _ in 0..10 {
                        assert_eq!(q.push(changed()), crate::live::Delivery::Taken);
                    }
                    assert!(q.len() <= LIVE_QUEUE);
                    done(0, "", "")
                }),
            },
            status,
        );
        queue.set(Arc::clone(&fx.lanes.live)).ok();
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            let up = fx.lanes.live.push(Live::Up { link: 1 });
            assert_eq!(up, crate::live::Delivery::Taken);
            pump(&mut ui, &fx.rx, |s| s.daemon.live());
            let run = Action::Run(account_a());
            ui.state.running = Some(run.clone());
            ui.execute(vec![Cmd::Suspend(run)]);
            assert_eq!(
                fx.lanes.live.len(),
                12,
                "a Down, then the next link's Up and 10 events"
            );
            assert!(
                ui.state.daemon.live(),
                "no message was taken while suspended"
            );
            let wakes = pump(&mut ui, &fx.rx, |s| !s.daemon.live());
            assert_eq!(wakes, 1, "one wake for the whole flood");
            assert!(fx.lanes.live.is_empty(), "taken whole");
            pump(&mut ui, &fx.rx, |s| s.daemon.live());
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    /// A daemon on `path` that greets each connection and takes its subscription, and counts
    /// them. It closes the first when `close_first` is sent to, and holds the others open.
    fn daemon_at(path: &std::path::Path) -> (Sender<()>, Arc<AtomicUsize>) {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;
        let listener = UnixListener::bind(path).unwrap();
        let (close_first, closing) = mpsc::channel::<()>();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&accepted);
        thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let hello = "{\"schemaVersion\":1,\"event\":\"hello\",\"protocol\":1,\"version\":\"20261014.2045.0\",\"pid\":4242,\"startedAt\":\"2026-09-21T14:13:20Z\"}\n";
                stream.write_all(hello.as_bytes()).unwrap();
                let mut request = String::new();
                BufReader::new(&stream).read_line(&mut request).unwrap();
                let ok = "{\"schemaVersion\":1,\"id\":1,\"ok\":true,\"result\":{}}\n";
                stream.write_all(ok.as_bytes()).unwrap();
                if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                    let _ = closing.recv();
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                } else {
                    held.push(stream);
                }
            }
        });
        (close_first, accepted)
    }

    #[test]
    fn a_failure_judged_after_the_live_thread_replaced_its_link_leaves_the_new_link_alone() {
        // C-R21, in the order a suspension (§7.3) leaves it: while the loop takes no messages,
        // link 1's `status` fails, and the live thread then loses link 1 and brings link 2 up.
        // On resume the failure ends link 1 by name; the live thread holds link 2, which carries
        // on: no third connection, and no Down.
        let dir = tempfile::Builder::new()
            .prefix("tt")
            .tempdir_in("/tmp")
            .or_else(|_| tempfile::Builder::new().prefix("tt").tempdir())
            .unwrap();
        let path = dir.path().join("daemon.sock");
        let (close_first, accepted) = daemon_at(&path);
        // The daemon lane: the second `status` fails once `fail` is sent to; the others answer.
        let calls = Arc::new(AtomicUsize::new(0));
        let (fail, failing) = mpsc::channel::<()>();
        let failing = Mutex::new(failing);
        let counted = Arc::clone(&calls);
        let status: StatusFetch = Arc::new(move || {
            if counted.fetch_add(1, Ordering::SeqCst) == 1 {
                let _ = failing.lock().unwrap().recv();
                return Err(StatusError::Client(ClientError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the daemon closed the connection",
                ))));
            }
            Ok(crate::testutil::daemon_status(
                vec![crate::testutil::daemon_job("claude-code", "auto", true)],
                vec![],
            ))
        });
        let fx = Fixture::with_daemon(quick(), |_| idle(), status);
        let backoff = Backoff {
            first: Duration::from_millis(20),
            most: Duration::from_millis(100),
            mismatch: Duration::from_secs(1),
        };
        crate::live::spawn(path.clone(), backoff, Arc::clone(&fx.lanes.live), || {}).unwrap();
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            pump(&mut ui, &fx.rx, |s| s.daemon.live());
            // An event asks for `status` again, and the lane takes it.
            let mode = tagteam_daemon::protocol::Event::Mode {
                provider: "claude-code".into(),
                mode: tagteam_daemon::protocol::Wire::from_wire("collect"),
                waiting_for: None,
                holder_pid: None,
                backoff: None,
            };
            fx.lanes.live.push(Live::Event(mode));
            let deadline = Instant::now() + Duration::from_secs(5);
            while calls.load(Ordering::SeqCst) < 2 {
                assert!(Instant::now() < deadline, "the second status was asked for");
                if let Ok(msg) = fx.rx.recv_timeout(Duration::from_millis(10)) {
                    ui.deliver(msg);
                }
            }
            // Suspended: the failure is queued, then link 1 goes and link 2 comes.
            fail.send(()).unwrap();
            let mut held = Vec::new();
            loop {
                let msg = fx
                    .rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("the failure");
                let failed = matches!(msg, Msg::DaemonStatus { result: Err(_), .. });
                held.push(msg);
                if failed {
                    break;
                }
            }
            close_first.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while fx.lanes.live.len() < 2 {
                assert!(Instant::now() < deadline, "link 2 came up");
                thread::sleep(Duration::from_millis(5));
            }
            // Resumed, in that order.
            for msg in held {
                ui.deliver(msg);
            }
            pump(&mut ui, &fx.rx, |s| s.daemon.live());
            thread::sleep(Duration::from_millis(300));
            assert_eq!(accepted.load(Ordering::SeqCst), 2, "link 2 carried on");
            assert!(fx.lanes.live.is_empty(), "no Down");
            assert!(ui.state.daemon.live());
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn the_live_thread_ends_after_the_shutdown_though_its_queue_still_holds_events() {
        // The shutdown stops listening to the live queue, which then says `Gone` whatever it
        // holds, so the live thread returns instead of waiting on a queue nobody takes.
        let path = PathBuf::from("/nonexistent/daemon.sock");
        let fx = Fixture::with_daemon(quick(), |_| idle(), Arc::new(|| Err(StatusError::NoDaemon)));
        let backoff = Backoff {
            first: Duration::from_millis(10),
            most: Duration::from_millis(20),
            mismatch: Duration::from_secs(1),
        };
        let handle = crate::live::spawn(path, backoff, Arc::clone(&fx.lanes.live), || {}).unwrap();
        let live = Arc::clone(&fx.lanes.live);
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            let deadline = Instant::now() + Duration::from_secs(5);
            while live.is_empty() {
                assert!(Instant::now() < deadline, "the thread pushed a Down");
                thread::sleep(Duration::from_millis(5));
            }
            fx.key('q');
            ui.run()
        };
        // The loop's receiver is still there: only the queue's own closing can end the thread.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !handle.is_finished() {
            assert!(Instant::now() < deadline, "the live thread ended");
            thread::sleep(Duration::from_millis(5));
        }
        handle.join().unwrap();
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn a_key_queued_behind_a_burst_is_handled_within_one_frame() {
        // C-R17: the loop applies every message waiting before it draws, so a burst costs one
        // frame, not one a message, and a key behind it waits for none of them.
        let fx = Fixture::new(quick(), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.frames.take();
            for width in 0..200 {
                let resize = Msg::Resize {
                    width: 100 + width % 20,
                    height: 40,
                };
                fx.tx.send(resize).unwrap();
            }
            fx.key('q');
            ui.run()
        };
        assert!(
            fx.frames.seen().len() <= 1,
            "{} frames for 201 messages",
            fx.frames.seen().len()
        );
        assert_eq!(fx.finish(ending).code, 0);
    }
}
