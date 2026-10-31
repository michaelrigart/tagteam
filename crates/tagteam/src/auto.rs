//! §11.4's output for `tagteam auto`: human lines or JSONL, from the events the loop in
//! `tagteam_daemon::auto` emits (D2). The loop itself, its flags and its errors live there.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{self, Write};

use tagteam_core::ProviderId;
use tagteam_core::autoswitch::NoSwitchReason;
use tagteam_daemon::protocol::event_json;
use tagteam_engine::auto::{AutoEvent, EventSink};
use tagteam_engine::settings::DEFAULT_THRESHOLD;
use tagteam_engine::words::{self, MISSING, NoSwitchFacts, duration};
use tagteam_provider::Cancel;

use crate::render::{RESET, severity_code};

/// Rust ignores SIGPIPE, so a reader that has gone away shows as `BrokenPipe` on a write. The
/// command stops as SIGPIPE would have stopped it, through the cancel token (§14.1), so the loop
/// ends at its next cancellation point and never inside a critical span. Any other write error
/// is ignored, as is every write without a token.
pub(crate) fn written(result: io::Result<()>, cancel: &Option<Cancel>) {
    if let (Err(e), Some(cancel)) = (result, cancel) {
        if e.kind() == io::ErrorKind::BrokenPipe {
            cancel.request(libc::SIGPIPE);
        }
    }
}

/// `--json` (§11.4): each event as one object on a line of its own.
pub struct JsonSink<'a> {
    out: RefCell<&'a mut dyn Write>,
    /// The engine's wall clock, epoch milliseconds: each event's `ts`.
    now_ms: &'a dyn Fn() -> i64,
    /// What a `BrokenPipe` on `out` stops the command through.
    cancel: Option<Cancel>,
}

impl<'a> JsonSink<'a> {
    pub fn new(out: &'a mut dyn Write, now_ms: &'a dyn Fn() -> i64) -> Self {
        JsonSink {
            out: RefCell::new(out),
            now_ms,
            cancel: None,
        }
    }

    /// A write that fails with `BrokenPipe` (the reader is gone) records SIGPIPE in `cancel`.
    pub fn stopping(mut self, cancel: &Cancel) -> Self {
        self.cancel = Some(cancel.clone());
        self
    }
}

impl EventSink for JsonSink<'_> {
    fn emit(&self, e: &AutoEvent) {
        let line = event_json(e, (self.now_ms)().div_euclid(1000));
        let mut out = self.out.borrow_mut();
        written(writeln!(out, "{line}"), &self.cancel);
    }
}

/// §11.4's human output. Each tick is one stdout line: the local time, the live account by
/// position and email, its relevant usage (the highest relevant percentage, coloured as `list`
/// colours it), and the outcome. Quarantine changes and long sleeps get lines of their own;
/// errors and warnings go to stderr. With several providers, each line names its provider
/// (§13.1).
pub struct HumanSink<'a> {
    out: RefCell<&'a mut dyn Write>,
    err: RefCell<&'a mut dyn Write>,
    /// The engine's wall clock, epoch milliseconds.
    now_ms: &'a dyn Fn() -> i64,
    /// Epoch seconds as a wall-clock time of day.
    clock: fn(i64) -> String,
    /// A provider's display name.
    names: &'a dyn Fn(&ProviderId) -> String,
    color: bool,
    several: bool,
    /// Each provider's poll in this tick, until its outcome line uses it or the tick ends.
    polled: RefCell<BTreeMap<ProviderId, Polled>>,
    /// What a `BrokenPipe` on `out` or `err` stops the command through.
    cancel: Option<Cancel>,
}

/// What a tick's line says about the live account, from its `poll`.
struct Polled {
    position: u32,
    email: String,
    /// The highest relevant percentage (100 − headroom); `None` when unknown.
    used: Option<f64>,
    /// Its windows, the one auto counts marked (`words::windows_said`).
    windows: Option<String>,
    /// The switch threshold, percent: what the poll decided by.
    threshold: f64,
}

impl<'a> HumanSink<'a> {
    pub fn new(
        out: &'a mut dyn Write,
        err: &'a mut dyn Write,
        now_ms: &'a dyn Fn() -> i64,
        names: &'a dyn Fn(&ProviderId) -> String,
        color: bool,
        several: bool,
    ) -> Self {
        HumanSink {
            out: RefCell::new(out),
            err: RefCell::new(err),
            now_ms,
            clock: local_time,
            names,
            color,
            several,
            polled: RefCell::new(BTreeMap::new()),
            cancel: None,
        }
    }

    /// A write that fails with `BrokenPipe` (the reader is gone) records SIGPIPE in `cancel`.
    pub fn stopping(mut self, cancel: &Cancel) -> Self {
        self.cancel = Some(cancel.clone());
        self
    }

    /// The engine's wall clock, epoch seconds: what `tagteam daemon`'s own lines are timed by.
    pub(crate) fn now_s(&self) -> i64 {
        (self.now_ms)().div_euclid(1000)
    }

    /// Epoch seconds as this sink writes a time of day.
    pub(crate) fn time_of(&self, epoch_s: i64) -> String {
        (self.clock)(epoch_s)
    }

    /// A line about no provider in particular: the time, then `text` (D8.1's start and stop).
    pub(crate) fn plain(&self, now: i64, text: &str) {
        let mut out = self.out.borrow_mut();
        written(writeln!(out, "{}  {text}", (self.clock)(now)), &self.cancel);
    }

    /// This sink on `clock` instead of the local time of day, for tests outside this module.
    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: fn(i64) -> String) -> Self {
        self.clock = clock;
        self
    }

    /// The time and, with several providers, the provider: how every line starts.
    fn head(&self, now: i64, provider: &ProviderId) -> String {
        let time = (self.clock)(now);
        if self.several {
            format!("{time}  {}  ", (self.names)(provider))
        } else {
            format!("{time}  ")
        }
    }

    pub(crate) fn line(&self, now: i64, provider: &ProviderId, text: &str) {
        let mut out = self.out.borrow_mut();
        written(
            writeln!(out, "{}{text}", self.head(now, provider)),
            &self.cancel,
        );
    }

    fn err_line(&self, now: i64, provider: &ProviderId, label: &str, text: &str) {
        let mut err = self.err.borrow_mut();
        written(
            writeln!(err, "{}{label}: {text}", self.head(now, provider)),
            &self.cancel,
        );
    }

    /// A tick's line: the live account and its usage from this tick's poll, then `outcome`.
    /// A tick that ended before collecting (§11.2 step 2) has no poll, and says only why.
    fn tick(&self, now: i64, provider: &ProviderId, outcome: &str) {
        match self.polled.borrow_mut().remove(provider) {
            Some(p) => {
                let used = match p.used {
                    Some(u) => pct(u, self.color),
                    None => MISSING.to_owned(),
                };
                let text = format!("#{} {}  {used}  {outcome}", p.position, p.email);
                self.line(now, provider, &text);
            }
            None => self.line(now, provider, outcome),
        }
    }
}

/// A percentage rounded as `list` rounds it, coloured by §13.5's severities when `color`.
fn pct(used: f64, color: bool) -> String {
    let n = used.round() as i64;
    match severity_code(n) {
        Some(code) if color => format!("{code}{n}%{RESET}"),
        _ => format!("{n}%"),
    }
}

impl EventSink for HumanSink<'_> {
    fn emit(&self, e: &AutoEvent) {
        let now = (self.now_ms)().div_euclid(1000);
        match e {
            AutoEvent::Poll {
                provider,
                active: Some((position, email)),
                headroom_pct,
                threshold,
                windows_pct,
                counting_window,
                ..
            } => {
                let used = headroom_pct
                    .get(position)
                    .copied()
                    .flatten()
                    .map(|h| 100.0 - h);
                let windows = windows_pct.get(position).and_then(|w| {
                    let counting = counting_window.get(position).map(String::as_str);
                    words::windows_said(w.iter().map(|(k, pct)| (k.as_str(), *pct)), counting)
                });
                let polled = Polled {
                    position: *position,
                    email: email.clone(),
                    used,
                    windows,
                    threshold: *threshold,
                };
                self.polled.borrow_mut().insert(provider.clone(), polled);
            }
            AutoEvent::Poll { .. } => {}
            AutoEvent::Switch {
                provider,
                trigger,
                to,
                warnings,
                dry_run,
                ..
            } => {
                for w in warnings {
                    self.err_line(now, provider, "warning", w);
                }
                let verb = if *dry_run { "would switch" } else { "switched" };
                self.tick(
                    now,
                    provider,
                    &format!("{verb} to #{to} ({})", words::trigger(*trigger)),
                );
            }
            AutoEvent::NoSwitch {
                provider,
                reason,
                detail,
            } => {
                match NoSwitchReason::parse(reason) {
                    Some(NoSwitchReason::EngineRunning) => {
                        let name = (self.names)(provider);
                        let text = format!(
                            "auto-switch already runs for {name} in another process; skipping it"
                        );
                        self.err_line(now, provider, "warning", &text);
                    }
                    // `all-exhausted` has its line at the event that follows, with its time.
                    Some(NoSwitchReason::AllExhausted) => {}
                    Some(parsed) => {
                        let (used_pct, windows, threshold) = self
                            .polled
                            .borrow()
                            .get(provider)
                            .map_or((None, None, DEFAULT_THRESHOLD), |p| {
                                (p.used, p.windows.clone(), p.threshold)
                            });
                        let display_name = (self.names)(provider);
                        let facts = NoSwitchFacts {
                            detail,
                            used_pct,
                            windows: windows.as_deref(),
                            threshold,
                            frees_at: None,
                            display_name: &display_name,
                        };
                        self.tick(now, provider, &words::no_switch(parsed, &facts));
                    }
                    None => {
                        let text = if detail.is_empty() {
                            format!("no switch: {reason}")
                        } else {
                            format!("no switch: {reason} ({detail})")
                        };
                        self.tick(now, provider, &text);
                    }
                }
            }
            AutoEvent::AllExhausted {
                provider,
                earliest_reset_at,
            } => {
                let frees_at = earliest_reset_at
                    .map(|at| format!("{} ({})", (self.clock)(at), duration(at - now)));
                let display_name = (self.names)(provider);
                let facts = NoSwitchFacts {
                    detail: "",
                    used_pct: None,
                    windows: None,
                    threshold: DEFAULT_THRESHOLD,
                    frees_at: frees_at.as_deref(),
                    display_name: &display_name,
                };
                let text = words::no_switch(NoSwitchReason::AllExhausted, &facts);
                self.tick(now, provider, &text);
            }
            AutoEvent::AccountQuarantined {
                provider,
                number,
                email,
                reason,
            } => self.line(
                now,
                provider,
                &format!("#{number} {email} quarantined ({reason})"),
            ),
            AutoEvent::AccountUnquarantined {
                provider,
                number,
                email,
                reason,
            } => self.line(
                now,
                provider,
                &format!("#{number} {email} unquarantined ({reason})"),
            ),
            AutoEvent::Sleep {
                provider,
                seconds,
                until,
            } => {
                let text = format!(
                    "sleeping {} until {}",
                    duration(*seconds as i64),
                    (self.clock)(*until)
                );
                self.line(now, provider, &text);
            }
            AutoEvent::Error {
                provider, message, ..
            } => self.err_line(now, provider, "error", message),
            AutoEvent::ConfigWarning { provider, message } => {
                self.err_line(now, provider, "warning", message)
            }
        }
    }

    /// A tick that ended in an error printed no outcome line: its poll goes with it, so a later
    /// tick that never polls never shows that tick's account and usage.
    fn tick_done(&self, provider: &ProviderId) {
        self.polled.borrow_mut().remove(provider);
    }
}

/// Epoch seconds as the local time of day, `HH:MM:SS`; UTC should the zone be unreadable or the
/// time lie outside the years jiff reads (about ±9999).
pub fn local_time(epoch_s: i64) -> String {
    let zoned = jiff::Timestamp::from_second(epoch_s)
        .map(|ts| ts.to_zoned(jiff::tz::TimeZone::try_system().unwrap_or(jiff::tz::TimeZone::UTC)));
    let (h, m, s) = match zoned {
        Ok(z) => (
            i64::from(z.hour()),
            i64::from(z.minute()),
            i64::from(z.second()),
        ),
        Err(_) => {
            let s = epoch_s.rem_euclid(86_400);
            (s / 3600, s % 3600 / 60, s % 60)
        }
    };
    format!("{h:02}:{m:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::json;
    use tagteam_cc::CLAUDE_CODE;
    use tagteam_cc::live::Platform;
    use tagteam_cc::{ClaudeCode, ItemKind, keychain_account, keychain_service};
    use tagteam_core::{AccountId, PollBudget, PollPlan, Window, WindowKind};
    use tagteam_daemon::auto::{AutoRun, run_loop};
    use tagteam_daemon::scheduler::Sleeper;
    use tagteam_engine::lifecycle::AddOptions;
    use tagteam_engine::oracle::NoOracle;
    use tagteam_engine::registry::ProviderRegistry;
    use tagteam_engine::settings::Settings;
    use tagteam_engine::store::{Eligibility, Reserve};
    use tagteam_engine::vault::{KeychainVault, SERVICE, Vault};
    use tagteam_engine::{Engine, EngineConfig, EventSource};
    use tagteam_provider::liveness::SystemProcessProbe;
    use tagteam_provider::profile::RunShell;
    use tagteam_provider::{Clock, Env, FakeClock, FakeKeychain, ScriptedHttp};

    use super::*;

    /// The fixture clock's start, epoch seconds.
    const T0: i64 = 1_790_000_000;
    /// An access-token expiry no test reaches, so nothing is freshened unless a test says so.
    const FAR_MS: i64 = 4_102_444_800_000;

    fn cc() -> ProviderId {
        ProviderId::new(CLAUDE_CODE)
    }

    /// A home with Claude Code registered, on a fake wall clock that starts at `T0` and a
    /// scripted HTTP port that answers nothing: what the one loop test here needs. The loop's
    /// own tests, and their fuller fixture, are in `tagteam_daemon::auto`.
    struct Fx {
        _dir: tempfile::TempDir,
        env: Env,
        kc: Arc<FakeKeychain>,
        clock: Arc<FakeClock>,
        engine: Engine,
    }

    impl Fx {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let env = Env::for_test(dir.path());
            fs::create_dir_all(env.home.join(".claude")).unwrap();
            let kc = Arc::new(FakeKeychain::new());
            let clock = Arc::new(FakeClock::new(T0 * 1000));
            let engine = Engine::new(EngineConfig {
                env: env.clone(),
                registry: ProviderRegistry::new()
                    .with(Arc::new(ClaudeCode::new(kc.clone(), Platform::MacOs))),
                vault: Vault::new(Box::new(KeychainVault::new(kc.clone()))),
                oracle: Arc::new(NoOracle),
                clock: clock.clone(),
                http: Arc::new(ScriptedHttp::new()),
                default_provider: cc(),
                settings: Settings::default(),
                process: Arc::new(SystemProcessProbe),
                // Nothing here validates, and a scripted spawner never starts a process (§15.1).
                spawner: Arc::new(tagteam_provider::process::ScriptedSpawner::new()),
                run_shell: RunShell::Outside,
                source: EventSource::Auto,
            });
            Fx {
                _dir: dir,
                env,
                kc,
                clock,
                engine,
            }
        }

        /// `claude /login` as `email`, then `tagteam add`: the login stays live.
        fn add(&self, email: &str) -> AccountId {
            let account = json!({"emailAddress": email, "organizationUuid": "", "accountUuid": format!("uuid-{email}")});
            fs::write(
                self.env.home.join(".claude.json"),
                json!({"oauthAccount": account}).to_string(),
            )
            .unwrap();
            let credential = json!({"claudeAiOauth": {"accessToken": format!("at-{email}"), "refreshToken": format!("rt-{email}"), "expiresAt": FAR_MS, "refreshTokenExpiresAt": FAR_MS}});
            self.kc.put(
                &keychain_service(&self.env, ItemKind::OAuth),
                &keychain_account(&self.env),
                credential.to_string().as_bytes(),
            );
            let opts = AddOptions {
                provider: cc(),
                position: None,
                alias: None,
                yes: false,
            };
            self.engine.add_live(opts).unwrap().account.id
        }

        /// `id`'s reading of `short` and `long` percent in its two windows, taken at `at` with
        /// its next poll planned at `next_poll_at`, recorded as a fetch records one.
        fn read(&self, id: &AccountId, short: f64, long: f64, at: i64, next_poll_at: i64) {
            let store = self.engine.store().unwrap();
            let row = store.account(id).unwrap().unwrap();
            let window = |key: &str, kind, pct, resets_at, period_s| Window {
                key: key.into(),
                label: key.into(),
                kind,
                pct,
                resets_at: Some(resets_at),
                period_s: Some(period_s),
                detail: None,
            };
            let windows = [
                window("5h", WindowKind::Short, short, T0 + 9_630, 18_000),
                window("7d", WindowKind::Long, long, T0 + 291_630, 604_800),
            ];
            let budget = &PollBudget::STANDARD;
            let Reserve::Reserved(r) = store
                .reserve_usage(&row, at * 1000, Eligibility::Scheduled, budget)
                .unwrap()
            else {
                panic!("no reservation at {at}");
            };
            let plan = PollPlan {
                interval_s: next_poll_at - at,
                next_poll_at,
            };
            assert!(store.record_usage(&r, &windows, at, &plan, 180).unwrap());
        }
    }

    /// The loop's sleep on the fixture's wall clock: each slice moves the clock on by its
    /// length, then `then` runs with the slice's number (from 0).
    struct Slices<'a> {
        clock: &'a FakeClock,
        taken: Cell<usize>,
        then: Box<dyn Fn(usize) + 'a>,
    }

    impl<'a> Slices<'a> {
        fn new(clock: &'a FakeClock, then: impl Fn(usize) + 'a) -> Self {
            Slices {
                clock,
                taken: Cell::new(0),
                then: Box::new(then),
            }
        }
    }

    impl Sleeper for Slices<'_> {
        fn sleep(&self, d: Duration) {
            let n = self.taken.replace(self.taken.get() + 1);
            self.clock.advance_ms(d.as_millis() as i64);
            (self.then)(n);
        }
    }

    fn looping() -> AutoRun {
        AutoRun::default()
    }

    /// `b` at position 2 is the live login, `a` at position 1 a candidate.
    fn two(fx: &Fx) -> (AccountId, AccountId) {
        (fx.add("a@x.co"), fx.add("b@x.co"))
    }

    fn poll(active: u32, headroom: &[(u32, Option<f64>)]) -> AutoEvent {
        AutoEvent::Poll {
            provider: cc(),
            active: Some((active, format!("{}@x.co", ["", "a", "b"][active as usize]))),
            headroom_pct: headroom.iter().copied().collect(),
            threshold: 90.0,
            fetch_errors: BTreeMap::new(),
            windows_pct: BTreeMap::new(),
            counting_window: BTreeMap::new(),
        }
    }

    fn no_switch_event(reason: &str, detail: &str) -> AutoEvent {
        AutoEvent::NoSwitch {
            provider: cc(),
            reason: reason.into(),
            detail: detail.into(),
        }
    }

    /// The human sink at `T0` on a fixed clock, whose times read as seconds after `T0`.
    fn human<'a>(
        out: &'a mut Vec<u8>,
        err: &'a mut Vec<u8>,
        now: &'a dyn Fn() -> i64,
        names: &'a dyn Fn(&ProviderId) -> String,
        color: bool,
        several: bool,
    ) -> HumanSink<'a> {
        let mut sink = HumanSink::new(out, err, now, names, color, several);
        sink.clock = |s| format!("t+{}", s - T0);
        sink
    }

    #[test]
    fn a_tick_is_one_line_and_quarantines_sleeps_errors_and_warnings_are_lines_of_their_own() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let now = || T0 * 1000;
        let names = |p: &ProviderId| format!("<{p}>");
        let sink = human(&mut out, &mut err, &now, &names, false, false);
        let events = [
            AutoEvent::AccountUnquarantined {
                provider: cc(),
                number: 1,
                email: "a@x.co".into(),
                reason: "credentials-replaced".into(),
            },
            poll(2, &[(1, Some(80.0)), (2, Some(4.6))]),
            AutoEvent::Switch {
                provider: cc(),
                trigger: tagteam_core::autoswitch::Trigger::Proactive,
                from: 2,
                to: 1,
                warnings: vec!["the Keychain refused the write".into()],
                dry_run: false,
            },
            poll(1, &[(1, None), (2, Some(4.6))]),
            AutoEvent::Error {
                provider: cc(),
                message: "usage was not collected".into(),
                transient: true,
            },
            no_switch_event("active-usage-unknown", "1/3"),
            no_switch_event("no-active-account", ""),
            poll(2, &[(1, Some(0.0)), (2, Some(5.0))]),
            no_switch_event("all-exhausted", "2h40m"),
            AutoEvent::AllExhausted {
                provider: cc(),
                earliest_reset_at: Some(T0 + 9_630),
            },
            AutoEvent::Sleep {
                provider: cc(),
                seconds: 600.0,
                until: T0 + 600,
            },
            AutoEvent::AccountQuarantined {
                provider: cc(),
                number: 1,
                email: "a@x.co".into(),
                reason: "invalid_grant".into(),
            },
            AutoEvent::ConfigWarning {
                provider: cc(),
                message: "autoswitch.models names \"Fabel\"".into(),
            },
            no_switch_event("engine-running", ""),
        ];
        for e in &events {
            sink.emit(e);
        }
        drop(sink);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "t+0  #1 a@x.co unquarantined (credentials-replaced)\n\
             t+0  #2 b@x.co  95%  switched to #1 (before it reached the limit)\n\
             t+0  #1 a@x.co  —  can't read the account in use (1 of 3 tries)\n\
             t+0  <claude-code> isn't logged in\n\
             t+0  #2 b@x.co  95%  every account is near its limit; the first frees up at t+9630 (2h40m)\n\
             t+0  sleeping 10m until t+600\n\
             t+0  #1 a@x.co quarantined (invalid_grant)\n"
        );
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "t+0  warning: the Keychain refused the write\n\
             t+0  error: usage was not collected\n\
             t+0  warning: autoswitch.models names \"Fabel\"\n\
             t+0  warning: auto-switch already runs for <claude-code> in another process; skipping it\n"
        );
    }

    #[test]
    fn the_no_switch_line_marks_the_window_auto_counts() {
        // `models = ["Opus"]`: Fable and Opus tie at 53 and Fable is first in the key order, but
        // the poll names Opus as the one auto counts (headroom 47). The line marks Opus.
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let now = || T0 * 1000;
        let names = |p: &ProviderId| format!("<{p}>");
        let sink = human(&mut out, &mut err, &now, &names, false, false);
        let mut p = poll(2, &[(1, Some(80.0)), (2, Some(47.0))]);
        if let AutoEvent::Poll {
            windows_pct,
            counting_window,
            ..
        } = &mut p
        {
            let windows = [
                ("5h", 9.0),
                ("7d", 20.0),
                ("scoped:Fable", 53.0),
                ("scoped:Opus", 53.0),
            ];
            windows_pct.insert(
                2,
                windows.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
            );
            counting_window.insert(2, "scoped:Opus".into());
        }
        sink.emit(&p);
        sink.emit(&no_switch_event("below-threshold", ""));
        drop(sink);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "t+0  #2 b@x.co  53%  no switch needed (5h 9% · 7d 20% · Fable 53% · Opus 53%◂, switches at 90%)\n"
        );
    }

    #[test]
    fn a_tick_that_ends_in_an_error_leaves_its_poll_to_no_later_line() {
        // b at 95 % polls, then a cannot be freshened: the tick ends in an error, with no line
        // on stdout. The live login is gone before the next tick, which has no poll: its line
        // names no account, not b.
        let fx = Fx::new();
        let (a, b) = two(&fx);
        fx.read(&a, 10.0, 10.0, T0, T0 + 300);
        fx.read(&b, 20.0, 95.0, T0, T0 + 300);
        let near = json!({"claudeAiOauth": {"accessToken": "at-a", "refreshToken": "rt-a", "expiresAt": T0 * 1000 + 60_000, "refreshTokenExpiresAt": FAR_MS}});
        fx.kc.put(SERVICE, a.as_str(), near.to_string().as_bytes());
        let live = fx.env.home.join(".claude.json");
        let (clock, cancel) = (fx.clock.clone(), fx.engine.cancel().clone());
        let slices = Slices::new(&fx.clock, move |_| {
            let _ = fs::remove_file(&live);
            if clock.now_ms() > (T0 + 60) * 1000 {
                cancel.request(libc::SIGTERM);
            }
        });
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let now = || fx.clock.now_ms();
        let names = |p: &ProviderId| format!("<{p}>");
        let sink = human(&mut out, &mut err, &now, &names, false, false);
        let code = run_loop(&fx.engine, &looping(), &sink, &slices, &mut || 0.0).unwrap();
        drop(sink);
        assert_eq!(code, 0);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "t+60  <claude-code> isn't logged in\n"
        );
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "t+0  error: could not freshen a@x.co (position 1): pre-send\n"
        );
    }

    #[test]
    fn usage_is_coloured_as_list_colours_it_and_several_providers_are_named() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let now = || T0 * 1000;
        let names = |p: &ProviderId| format!("<{p}>");
        let sink = human(&mut out, &mut err, &now, &names, true, true);
        for (active, headroom) in [(2, 5.0), (2, 25.0), (2, 50.0)] {
            sink.emit(&poll(active, &[(active, Some(headroom))]));
            sink.emit(&no_switch_event("below-threshold", ""));
        }
        drop(sink);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "t+0  <claude-code>  #2 b@x.co  \x1b[31m95%\x1b[0m  no switch needed (95%, switches at 90%)\n\
             t+0  <claude-code>  #2 b@x.co  \x1b[33m75%\x1b[0m  no switch needed (75%, switches at 90%)\n\
             t+0  <claude-code>  #2 b@x.co  50%  no switch needed (50%, switches at 90%)\n"
        );
    }

    /// One line of stdout for `events`, on a fixed clock with provider names `<id>`.
    fn lines_for(events: &[AutoEvent]) -> String {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let now = || T0 * 1000;
        let names = |p: &ProviderId| format!("<{p}>");
        let sink = human(&mut out, &mut err, &now, &names, false, false);
        for e in events {
            sink.emit(e);
        }
        drop(sink);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn each_no_switch_line_is_its_plain_sentence() {
        assert_eq!(
            lines_for(&[no_switch_event("cooldown", "4m")]),
            "t+0  waiting 4m after the last switch\n"
        );
        assert_eq!(
            lines_for(&[
                poll(2, &[(2, Some(23.0))]),
                no_switch_event("below-threshold", "")
            ]),
            "t+0  #2 b@x.co  77%  no switch needed (77%, switches at 90%)\n"
        );
        assert_eq!(
            lines_for(&[no_switch_event("active-usage-unknown", "2/3")]),
            "t+0  can't read the account in use (2 of 3 tries)\n"
        );
        assert_eq!(
            lines_for(&[no_switch_event("no-active-account", "")]),
            "t+0  <claude-code> isn't logged in\n"
        );
    }

    #[test]
    fn a_below_threshold_line_without_a_poll_says_the_default_threshold() {
        assert_eq!(
            lines_for(&[no_switch_event("below-threshold", "")]),
            "t+0  no switch needed (switches at 90%)\n"
        );
    }

    #[test]
    fn a_below_threshold_line_says_the_threshold_of_its_poll() {
        let mut p = poll(2, &[(2, Some(23.0))]);
        if let AutoEvent::Poll { threshold, .. } = &mut p {
            *threshold = 85.0;
        }
        assert_eq!(
            lines_for(&[p, no_switch_event("below-threshold", "")]),
            "t+0  #2 b@x.co  77%  no switch needed (77%, switches at 85%)\n"
        );
    }

    #[test]
    fn all_exhausted_says_when_the_first_frees_up_and_how_long_that_is() {
        let exhausted = |earliest_reset_at| AutoEvent::AllExhausted {
            provider: cc(),
            earliest_reset_at,
        };
        assert_eq!(
            lines_for(&[
                poll(2, &[(2, Some(5.0))]),
                no_switch_event("all-exhausted", "2h40m"),
                exhausted(Some(T0 + 9_630)),
            ]),
            "t+0  #2 b@x.co  95%  every account is near its limit; the first frees up at t+9630 (2h40m)\n"
        );
        assert_eq!(
            lines_for(&[
                poll(2, &[(2, Some(5.0))]),
                no_switch_event("all-exhausted", ""),
                exhausted(None),
            ]),
            "t+0  #2 b@x.co  95%  every account is near its limit\n"
        );
    }

    #[test]
    fn a_switch_line_says_its_trigger_in_words() {
        let switch = |dry_run| AutoEvent::Switch {
            provider: cc(),
            trigger: tagteam_core::autoswitch::Trigger::Proactive,
            from: 1,
            to: 2,
            warnings: Vec::new(),
            dry_run,
        };
        assert_eq!(
            lines_for(&[switch(false)]),
            "t+0  switched to #2 (before it reached the limit)\n"
        );
        let at_limit = AutoEvent::Switch {
            provider: cc(),
            trigger: tagteam_core::autoswitch::Trigger::AtLimit,
            from: 1,
            to: 2,
            warnings: Vec::new(),
            dry_run: true,
        };
        assert_eq!(
            lines_for(&[at_limit]),
            "t+0  would switch to #2 (at the limit)\n"
        );
    }

    #[test]
    fn an_unknown_no_switch_reason_is_still_printed() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let now = || T0 * 1000;
        let names = |p: &ProviderId| format!("<{p}>");
        let sink = human(&mut out, &mut err, &now, &names, false, false);
        sink.emit(&no_switch_event("some-new-reason", ""));
        drop(sink);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "t+0  no switch: some-new-reason\n"
        );
        assert_eq!(String::from_utf8(err).unwrap(), "");
    }

    #[test]
    fn an_unknown_no_switch_reason_is_printed_with_its_detail() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let now = || T0 * 1000;
        let names = |p: &ProviderId| format!("<{p}>");
        let sink = human(&mut out, &mut err, &now, &names, false, false);
        sink.emit(&no_switch_event("some-new-reason", "2/3"));
        drop(sink);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "t+0  no switch: some-new-reason (2/3)\n"
        );
        assert_eq!(String::from_utf8(err).unwrap(), "");
    }

    #[test]
    fn local_time_of_an_out_of_range_epoch_still_reads_as_a_time() {
        // Out of jiff's range the zone is never read, so the UTC time of day is exact.
        assert_eq!(local_time(i64::MAX), "15:30:07");
        assert_eq!(local_time(i64::MIN), "08:29:52");
    }

    #[test]
    fn local_time_is_a_time_of_day() {
        let t = local_time(T0);
        assert_eq!(t.len(), 8, "{t}");
        assert_eq!(&t[2..3], ":");
        assert_eq!(
            &t[5..],
            ":20",
            "zones differ by whole minutes; T0 is :20 past"
        );
    }

    /// A writer that fails every write with `kind`.
    struct Failing(io::ErrorKind);

    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(self.0.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_broken_pipe_on_either_sink_stops_the_command_as_sigpipe_would() {
        let now = || T0 * 1000;
        let names = |p: &ProviderId| format!("<{p}>");
        let engine_running = no_switch_event("engine-running", "");
        let quiet = no_switch_event("cooldown", "");
        let cases: [(&str, bool, &AutoEvent); 3] = [
            ("json", false, &quiet),
            ("human stdout", false, &quiet),
            ("human stderr", true, &engine_running),
        ];
        for (name, on_err, event) in cases {
            for (kind, want) in [
                (io::ErrorKind::BrokenPipe, Some(libc::SIGPIPE)),
                (io::ErrorKind::PermissionDenied, None),
                (io::ErrorKind::WouldBlock, None),
            ] {
                let cancel = Cancel::new();
                let mut broken = Failing(kind);
                let mut fine = Vec::new();
                match name {
                    "json" => JsonSink::new(&mut broken, &now)
                        .stopping(&cancel)
                        .emit(event),
                    _ if on_err => {
                        HumanSink::new(&mut fine, &mut broken, &now, &names, false, false)
                            .stopping(&cancel)
                            .emit(event)
                    }
                    _ => HumanSink::new(&mut broken, &mut fine, &now, &names, false, false)
                        .stopping(&cancel)
                        .emit(event),
                }
                assert_eq!(cancel.requested(), want, "{name}: {kind:?}");
            }
        }
    }
}
