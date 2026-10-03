//! §11.4: the `auto` loop, a thin driver over `AutoEngine::tick` (Decision 7). It runs one
//! engine per provider on independent schedules, sleeps toward each one's wall-clock deadline
//! in slices of at most a second that check the cancel token, re-reads `config.toml` before a
//! tick when its mtime changed (Decision 9), and stops with exit 0 on a signal (Decision 8).
//! `--once` runs one tick per provider instead.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::ops::RangeInclusive;
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_cc::usage::format_iso8601;

use tagteam_core::ProviderId;
use tagteam_core::autoswitch::{
    AutoConfig, Decision, NoSwitchReason, Strategy, announces_sleep, most_severe, next_delay,
    once_exit_code,
};
use tagteam_engine::auto::{AutoEngine, AutoEvent, EventSink, TickOutcome};
use tagteam_engine::settings::{
    COOLDOWN_SECONDS_RANGE, INTERVAL_SECONDS_RANGE, Settings, THRESHOLD_RANGE,
};
use tagteam_engine::{Engine, EngineError};
use tagteam_provider::Cancel;

use crate::render::{MISSING, RESET, duration, severity};

/// §11.4: the loop checks the cancel token and re-reads the wall clock at least this often.
const SLICE_MS: i64 = 1_000;
/// `--once`'s code for a tick that failed (§11.4).
const ONCE_ERROR: i32 = 1;

/// How the loop waits between its checks. `ThreadSleeper` in the binary; tests advance a fake
/// wall clock instead, and jump it to model a suspend.
pub trait Sleeper {
    fn sleep(&self, d: Duration);
}

/// The thread's own sleep. Its clock may stop while the machine is suspended, so the loop never
/// trusts it for the deadline: each slice is at most a second, and the wall clock is read again
/// after it (§11.4).
pub struct ThreadSleeper;

impl Sleeper for ThreadSleeper {
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// The loop's jitter draw: uniform in [-1, 1), which `next_delay` maps to `U(0.9, 1.1)`.
pub fn uniform_jitter() -> f64 {
    fastrand::f64() * 2.0 - 1.0
}

/// One invocation's flags (§11.4). Each overrides its setting for every provider, clamped to
/// the setting's range (§6.4), and still wins after a reload (Decision 9).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AutoFlags {
    pub threshold: Option<f64>,
    pub interval_s: Option<i64>,
    pub cooldown_s: Option<i64>,
    pub strategy: Option<Strategy>,
    pub models: Option<Vec<String>>,
    pub include_api_key_accounts: Option<bool>,
}

/// What one provider's engine decides with: its settings, the flags over them, and the
/// provider's long window (§4.5).
pub fn auto_config(s: &Settings, flags: &AutoFlags, long_window: Option<&str>) -> AutoConfig {
    AutoConfig {
        threshold: flags
            .threshold
            .map_or(s.threshold, |v| clamped(v, THRESHOLD_RANGE)),
        hysteresis_pct: s.hysteresis_pct,
        cooldown_s: flags
            .cooldown_s
            .map_or(s.cooldown_seconds, |v| clamped(v, COOLDOWN_SECONDS_RANGE)),
        interval_s: flags
            .interval_s
            .map_or(s.interval_seconds, |v| clamped(v, INTERVAL_SECONDS_RANGE)),
        unhealthy_ticks: s.unhealthy_ticks,
        strategy: flags.strategy.unwrap_or(s.strategy),
        include_api_key_accounts: flags
            .include_api_key_accounts
            .unwrap_or(s.include_api_key_accounts),
        models: flags.models.clone().unwrap_or_else(|| s.models.clone()),
        long_window: long_window.map(str::to_owned),
    }
}

/// `v` clamped into `range`, as §6.4 clamps a flag.
fn clamped<T: PartialOrd + Copy>(v: T, range: RangeInclusive<T>) -> T {
    let (lo, hi) = (*range.start(), *range.end());
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// What one `tagteam auto` invocation asks for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AutoRun {
    /// `--provider`: only this one. Otherwise every provider with two switchable accounts.
    pub provider: Option<ProviderId>,
    pub flags: AutoFlags,
    pub once: bool,
    pub dry_run: bool,
}

/// How `auto` ends as a command error rather than with its own exit code.
#[derive(Debug)]
pub enum AutoError {
    /// Every provider's engine lock is held by another process (§11.1).
    AlreadyRuns(Vec<ProviderId>),
    /// No provider has two switchable accounts to switch between.
    NothingToSwitch,
    /// `--strategy consume-first` with `--provider` naming a provider that has no long window to
    /// rank by (§4.5): a usage error.
    NoLongWindow(ProviderId),
    /// An interrupted `--once` (128 + n, §14.1), or a failure before any tick.
    Engine(EngineError),
}

impl From<EngineError> for AutoError {
    fn from(e: EngineError) -> Self {
        AutoError::Engine(e)
    }
}

/// §11.1: `only`, or every registered provider with at least two switchable accounts, in
/// provider order. Switchable is read from the store, as the tick reads it: enabled, with an
/// identity. A quarantine does not count against an account here: failing over from it, or
/// releasing it, is the tick's work.
pub fn providers(
    engine: &Engine,
    only: Option<&ProviderId>,
) -> Result<Vec<ProviderId>, EngineError> {
    if let Some(p) = only {
        engine.provider(p)?;
        return Ok(vec![p.clone()]);
    }
    let Some(store) = engine.existing_store()? else {
        return Ok(Vec::new());
    };
    let mut switchable: BTreeMap<ProviderId, usize> = BTreeMap::new();
    for row in store.all_accounts()? {
        if !row.disabled && row.identity_json.is_object() && engine.provider(&row.provider).is_ok()
        {
            *switchable.entry(row.provider).or_default() += 1;
        }
    }
    Ok(switchable
        .into_iter()
        .filter(|(_, n)| *n >= 2)
        .map(|(p, _)| p)
        .collect())
}

/// `tagteam auto`: `--once`'s exit code (§11.1), or the loop's 0 once a signal stops it (§11.4).
/// A loop skips a provider whose engine runs elsewhere, with a `no-switch engine-running`
/// event, and fails with `AlreadyRuns` when that leaves it none. Consume-first named for a
/// provider without a long window fails with `NoLongWindow` before any engine starts (§4.5).
/// `jitter` draws in [-1, 1).
pub fn run_loop(
    engine: &Engine,
    run: &AutoRun,
    sink: &dyn EventSink,
    sleeper: &dyn Sleeper,
    jitter: &mut dyn FnMut() -> f64,
) -> Result<i32, AutoError> {
    let providers = providers(engine, run.provider.as_ref())?;
    if providers.is_empty() {
        return Err(AutoError::NothingToSwitch);
    }
    // §4.5: from the settings, or with no `--provider`, such a provider's engine runs `best`
    // with a `config-warning` instead.
    if let (Some(p), Some(Strategy::ConsumeFirst)) = (&run.provider, run.flags.strategy) {
        if engine.provider(p)?.primary_long_window().is_none() {
            return Err(AutoError::NoLongWindow(p.clone()));
        }
    }
    // Before the settings are read, so a write landing meanwhile still counts as a change.
    let mtime = Settings::mtime(engine.env());
    if run.once {
        return once(engine, &providers, run, sink);
    }
    let mut slots = Vec::new();
    let mut held = Vec::new();
    for provider in providers {
        let (cfg, long_window) = configured(engine, &provider, &run.flags)?;
        match engine.auto(&provider, cfg.clone(), run.dry_run)? {
            Some(auto) => slots.push(Slot {
                provider,
                long_window,
                auto,
                cfg,
                due_ms: engine.now_ms(),
                delay_ms: 0,
            }),
            None => held.push(provider),
        }
    }
    if slots.is_empty() {
        return Err(AutoError::AlreadyRuns(held));
    }
    for provider in held {
        sink.emit(&engine_running(provider));
    }
    let mut driver = Driver {
        engine,
        flags: &run.flags,
        sink,
        mtime,
    };
    Ok(driver.forever(&mut slots, sleeper, jitter))
}

/// One provider the loop drives: its engine, what it decides with, and when it ticks next.
struct Slot<'e> {
    provider: ProviderId,
    long_window: Option<String>,
    auto: AutoEngine<'e>,
    cfg: AutoConfig,
    /// The wall-clock time of its next tick, epoch milliseconds.
    due_ms: i64,
    /// The delay that deadline was set for, which the wait never exceeds.
    delay_ms: i64,
}

struct Driver<'a> {
    engine: &'a Engine,
    flags: &'a AutoFlags,
    sink: &'a dyn EventSink,
    /// `config.toml`'s mtime as the settings were last read.
    mtime: Option<std::time::SystemTime>,
}

impl Driver<'_> {
    /// Each pass either ticks the provider whose deadline has passed (the earliest; on a tie, the
    /// first provider) or sleeps a slice of at most a second toward the earliest deadline. The
    /// cancel token is checked before each (§14.1: the loop's sleep and between its ticks), and
    /// the wall clock is read again after each slice: a machine suspended past a deadline ticks
    /// as soon as it wakes.
    fn forever(
        &mut self,
        slots: &mut [Slot<'_>],
        sleeper: &dyn Sleeper,
        jitter: &mut dyn FnMut() -> f64,
    ) -> i32 {
        loop {
            if self.engine.cancel().requested().is_some() {
                return 0;
            }
            let now_ms = self.engine.now_ms();
            for slot in slots.iter_mut() {
                // A wall clock set back would stretch the wait by as much: never wait longer
                // than the delay the deadline was set for.
                slot.due_ms = slot.due_ms.min(now_ms + slot.delay_ms);
            }
            let next = (0..slots.len())
                .min_by_key(|&i| slots[i].due_ms)
                .expect("the loop drives at least one provider");
            let left = slots[next].due_ms - now_ms;
            if left > 0 {
                sleeper.sleep(Duration::from_millis(left.min(SLICE_MS) as u64));
                continue;
            }
            self.reload_if_changed(slots);
            let slot = &mut slots[next];
            let ticked = slot.auto.tick(self.sink);
            match settle(&slot.provider, ticked, self.sink) {
                Ok((_, decision)) => self.schedule(slot, &decision, jitter()),
                // §11.4: a signal met inside the tick stops the loop as cleanly as one between
                // ticks.
                Err(_) => return 0,
            }
        }
    }

    /// Decision 9: before a tick, every provider's settings are read again when `config.toml`'s
    /// mtime changed, the flags applied over them. Each warning the read gives is a
    /// `config-warning`.
    fn reload_if_changed(&mut self, slots: &mut [Slot<'_>]) {
        let mtime = Settings::mtime(self.engine.env());
        if mtime == self.mtime {
            return;
        }
        self.mtime = mtime;
        for slot in slots {
            let (settings, warnings) = Settings::load(self.engine.env(), &slot.provider);
            for message in warnings {
                self.sink.emit(&AutoEvent::ConfigWarning {
                    provider: slot.provider.clone(),
                    message,
                });
            }
            slot.cfg = auto_config(&settings, self.flags, slot.long_window.as_deref());
            slot.auto.set_config(slot.cfg.clone());
        }
    }

    /// The slot's next deadline, `next_delay` after now (§11.4), with a `sleep` event when the
    /// delay is long.
    fn schedule(&self, slot: &mut Slot<'_>, decision: &Decision, jitter: f64) {
        let now_ms = self.engine.now_ms();
        let now = now_ms.div_euclid(1000);
        let delay = next_delay(
            decision,
            &slot.cfg,
            now,
            slot.auto.active_next_poll_at(),
            jitter,
        );
        if announces_sleep(delay, &slot.cfg) {
            self.sink.emit(&AutoEvent::Sleep {
                provider: slot.provider.clone(),
                seconds: delay as f64,
                until: now + delay,
            });
        }
        slot.delay_ms = delay * 1000;
        slot.due_ms = now_ms + slot.delay_ms;
    }
}

/// `--once` (§11.1): one tick per provider, in order, each holding its engine lock only for its
/// tick. A provider whose engine runs elsewhere reports `engine-running`. The exit code is the
/// most severe. A signal ends it at the next tick's opening cancellation point (§14.1).
fn once(
    engine: &Engine,
    providers: &[ProviderId],
    run: &AutoRun,
    sink: &dyn EventSink,
) -> Result<i32, AutoError> {
    let mut codes = Vec::new();
    for provider in providers {
        let (cfg, _) = configured(engine, provider, &run.flags)?;
        let code = match engine.auto(provider, cfg, run.dry_run)? {
            Some(mut auto) => settle(provider, auto.tick(sink), sink)?.0,
            None => {
                sink.emit(&engine_running(provider.clone()));
                once_exit_code(&no_switch(NoSwitchReason::EngineRunning))
            }
        };
        codes.push(code);
    }
    Ok(most_severe(&codes))
}

/// A provider's settings, flags over them, and its long window. The command printed the
/// settings file's warnings when it started, so they are not repeated here.
fn configured(
    engine: &Engine,
    provider: &ProviderId,
    flags: &AutoFlags,
) -> Result<(AutoConfig, Option<String>), EngineError> {
    let long_window = engine
        .provider(provider)?
        .primary_long_window()
        .map(str::to_owned);
    let (settings, _) = Settings::load(engine.env(), provider);
    Ok((
        auto_config(&settings, flags, long_window.as_deref()),
        long_window,
    ))
}

/// A tick's `--once` code and the decision its delay follows. `Err` only for an interruption.
/// Any other `Err` the tick returns (a store that failed) is reported here as an `error` event,
/// since the engine reports every other failure itself, and the loop keeps its normal cadence
/// (§11.4).
fn settle(
    provider: &ProviderId,
    ticked: Result<(TickOutcome, Decision), EngineError>,
    sink: &dyn EventSink,
) -> Result<(i32, Decision), EngineError> {
    match ticked {
        Ok((TickOutcome::Error, decision)) => Ok((ONCE_ERROR, decision)),
        Ok((_, decision)) => Ok((once_exit_code(&decision), decision)),
        Err(e) if e.signal().is_some() => Err(e),
        Err(e) => {
            sink.emit(&AutoEvent::Error {
                provider: provider.clone(),
                message: e.to_string(),
                transient: false,
            });
            let normal = no_switch(NoSwitchReason::NoActiveAccount);
            Ok((ONCE_ERROR, normal))
        }
    }
}

/// A `no-switch` with no detail, as `decide` would return it.
fn no_switch(reason: NoSwitchReason) -> Decision {
    Decision::NoSwitch {
        reason,
        outcome: reason.outcome(),
        detail: String::new(),
        earliest_reset: None,
    }
}

/// §11.1: the `no-switch` a provider whose engine lock another process holds reports.
fn engine_running(provider: ProviderId) -> AutoEvent {
    AutoEvent::NoSwitch {
        provider,
        reason: NoSwitchReason::EngineRunning.as_str().to_owned(),
        detail: String::new(),
    }
}

/// Rust ignores SIGPIPE, so a reader that has gone away shows as `BrokenPipe` on a write. The
/// command stops as SIGPIPE would have stopped it, through the cancel token (§14.1), so the loop
/// ends at its next cancellation point and never inside a critical span. Any other write error
/// is ignored, as is every write without a token.
fn written(result: io::Result<()>, cancel: &Option<Cancel>) {
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

/// One event in §11.4's shape: the envelope `{"schemaVersion":1,"event":<kind>,"ts":"…Z"}`,
/// the additive `provider` (§13.2), then the kind's fields in cswap's spelling. Accounts are
/// keyed by position; times are ISO 8601 UTC. `fetchErrors` and `windowsPct` appear only when
/// they hold something.
pub fn event_json(e: &AutoEvent, now_s: i64) -> Value {
    let (kind, provider, fields) = match e {
        AutoEvent::Poll {
            provider,
            active,
            headroom_pct,
            threshold,
            fetch_errors,
            windows_pct,
        } => {
            let mut f = json!({
                "active": active.as_ref().map(|(number, email)| json!({"number": number, "email": email})),
                "headroomPct": by_position(headroom_pct, |h| json!(h)),
                "threshold": threshold,
            });
            if !fetch_errors.is_empty() {
                f["fetchErrors"] = by_position(fetch_errors, |kind| json!(kind));
            }
            if !windows_pct.is_empty() {
                f["windowsPct"] = by_position(windows_pct, |w| json!(w));
            }
            ("poll", provider, f)
        }
        AutoEvent::Switch {
            provider,
            trigger,
            from,
            to,
            warnings,
            dry_run,
        } => (
            "switch",
            provider,
            json!({"trigger": trigger.as_str(), "from": from, "to": to, "warnings": warnings, "dryRun": dry_run}),
        ),
        AutoEvent::NoSwitch {
            provider,
            reason,
            detail,
        } => (
            "no-switch",
            provider,
            json!({"reason": reason, "detail": detail}),
        ),
        AutoEvent::AccountQuarantined {
            provider,
            number,
            email,
            reason,
        } => (
            "account-quarantined",
            provider,
            json!({"number": number, "email": email, "reason": reason}),
        ),
        AutoEvent::AccountUnquarantined {
            provider,
            number,
            email,
            reason,
        } => (
            "account-unquarantined",
            provider,
            json!({"number": number, "email": email, "reason": reason}),
        ),
        AutoEvent::AllExhausted {
            provider,
            earliest_reset_at,
        } => (
            "all-exhausted",
            provider,
            json!({"earliestResetAt": earliest_reset_at.map(format_iso8601)}),
        ),
        AutoEvent::Sleep {
            provider,
            seconds,
            until,
        } => (
            "sleep",
            provider,
            json!({"seconds": (seconds * 10.0).round() / 10.0, "until": format_iso8601(*until)}),
        ),
        AutoEvent::Error {
            provider,
            message,
            transient,
        } => (
            "error",
            provider,
            json!({"message": message, "transient": transient}),
        ),
        AutoEvent::ConfigWarning { provider, message } => {
            ("config-warning", provider, json!({"message": message}))
        }
    };
    let mut o = json!({
        "schemaVersion": 1,
        "event": kind,
        "ts": format_iso8601(now_s),
        "provider": provider.as_str(),
    });
    if let Value::Object(fields) = fields {
        for (k, v) in fields {
            o[k] = v;
        }
    }
    o
}

/// A map keyed by account position, as §11.4's objects key accounts.
fn by_position<T>(m: &BTreeMap<u32, T>, value: impl Fn(&T) -> Value) -> Value {
    Value::Object(
        m.iter()
            .map(|(position, v)| (position.to_string(), value(v)))
            .collect(),
    )
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

    /// The time and, with several providers, the provider: how every line starts.
    fn head(&self, now: i64, provider: &ProviderId) -> String {
        let time = (self.clock)(now);
        if self.several {
            format!("{time}  {}  ", (self.names)(provider))
        } else {
            format!("{time}  ")
        }
    }

    fn line(&self, now: i64, provider: &ProviderId, text: &str) {
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
    match severity(n) {
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
                ..
            } => {
                let used = headroom_pct.get(position).copied().flatten();
                let polled = Polled {
                    position: *position,
                    email: email.clone(),
                    used: used.map(|h| 100.0 - h),
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
                    &format!("{verb} to #{to} ({})", trigger.as_str()),
                );
            }
            AutoEvent::NoSwitch {
                provider,
                reason,
                detail,
            } => {
                if reason == NoSwitchReason::EngineRunning.as_str() {
                    let name = (self.names)(provider);
                    let text = format!(
                        "auto-switch already runs for {name} in another process; skipping it"
                    );
                    self.err_line(now, provider, "warning", &text);
                } else if reason != NoSwitchReason::AllExhausted.as_str() {
                    // `all-exhausted` has its line at the event that follows, with its time.
                    let text = if detail.is_empty() {
                        format!("no switch: {reason}")
                    } else {
                        format!("no switch: {reason} ({detail})")
                    };
                    self.tick(now, provider, &text);
                }
            }
            AutoEvent::AllExhausted {
                provider,
                earliest_reset_at,
            } => {
                let text = match earliest_reset_at {
                    Some(at) => format!(
                        "no switch: all-exhausted, the first back at {} ({})",
                        (self.clock)(*at),
                        duration(at - now)
                    ),
                    None => "no switch: all-exhausted".to_owned(),
                };
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

/// Epoch seconds as the local time of day, `HH:MM:SS`; UTC should the zone be unreadable.
pub fn local_time(epoch_s: i64) -> String {
    let t = epoch_s as libc::time_t;
    // SAFETY: an all-zero `tm` is valid storage for `localtime_r` to fill.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the call; `localtime_r` is reentrant and keeps
    // neither.
    let filled = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    let (h, m, s) = if filled {
        (
            i64::from(tm.tm_hour),
            i64::from(tm.tm_min),
            i64::from(tm.tm_sec),
        )
    } else {
        let s = epoch_s.rem_euclid(86_400);
        (s / 3600, s % 3600 / 60, s % 60)
    };
    format!("{h:02}:{m:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::sync::Arc;

    use serde_json::json;
    use tagteam_cc::live::Platform;
    use tagteam_cc::{ClaudeCode, ItemKind, keychain_account, keychain_service};
    use tagteam_core::{AccountId, CLAUDE_CODE, PollBudget, PollPlan, Window, WindowKind};
    use tagteam_engine::EngineConfig;
    use tagteam_engine::lifecycle::AddOptions;
    use tagteam_engine::oracle::NoOracle;
    use tagteam_engine::registry::ProviderRegistry;
    use tagteam_engine::store::{Eligibility, Reserve};
    use tagteam_engine::vault::{KeychainVault, SERVICE, Vault};
    use tagteam_fake::{FAKE_AGENT, FakeAgent};
    use tagteam_provider::liveness::SystemProcessProbe;
    use tagteam_provider::profile::RunShell;
    use tagteam_provider::{Clock, Env, FakeClock, FakeKeychain, Method, ScriptedHttp};

    use super::*;

    /// The fixture clock's start, epoch seconds.
    const T0: i64 = 1_790_000_000;
    /// An access-token expiry no test reaches, so nothing is freshened unless a test says so.
    const FAR_MS: i64 = 4_102_444_800_000;
    const CC_USAGE: &str = "https://api.anthropic.com/api/oauth/usage";

    fn cc() -> ProviderId {
        ProviderId::new(CLAUDE_CODE)
    }

    fn fake() -> ProviderId {
        ProviderId::new(FAKE_AGENT)
    }

    /// A home with Claude Code and FakeAgent registered, on a fake wall clock that starts at
    /// `T0` and a scripted HTTP port that answers nothing (every request is `PreSend`).
    struct Fx {
        _dir: tempfile::TempDir,
        env: Env,
        kc: Arc<FakeKeychain>,
        clock: Arc<FakeClock>,
        http: Arc<ScriptedHttp>,
        engine: Engine,
    }

    impl Fx {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let env = Env::for_test(dir.path());
            fs::create_dir_all(env.home.join(".claude")).unwrap();
            let kc = Arc::new(FakeKeychain::new());
            let clock = Arc::new(FakeClock::new(T0 * 1000));
            let http = Arc::new(ScriptedHttp::new());
            let engine = Engine::new(EngineConfig {
                env: env.clone(),
                registry: ProviderRegistry::new()
                    .with(Arc::new(ClaudeCode::new(kc.clone(), Platform::MacOs)))
                    .with(Arc::new(FakeAgent::new())),
                vault: Vault::new(Box::new(KeychainVault::new(kc.clone()))),
                oracle: Arc::new(NoOracle),
                clock: clock.clone(),
                http: http.clone(),
                default_provider: cc(),
                settings: Settings::default(),
                process: Arc::new(SystemProcessProbe),
                // Nothing here validates, and a scripted spawner never starts a process (§15.1).
                spawner: Arc::new(tagteam_provider::process::ScriptedSpawner::new()),
                run_shell: RunShell::Outside,
            });
            Fx {
                _dir: dir,
                env,
                kc,
                clock,
                http,
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
            self.add_live(cc())
        }

        /// A FakeAgent login as `handle`, then `tagteam add`: the login stays live.
        fn add_fake(&self, handle: &str) -> AccountId {
            let (token, renew) = (format!("tok-{handle}"), format!("renew-{handle}"));
            tagteam_fake::login(&self.env, handle, "ws", &token, &renew);
            self.add_live(fake())
        }

        fn add_live(&self, provider: ProviderId) -> AccountId {
            let opts = AddOptions {
                provider,
                position: None,
                alias: None,
                yes: false,
            };
            self.engine.add_live(opts).unwrap().account.id
        }

        /// `id`'s reading of `short` and `long` percent in its provider's two windows, taken at
        /// `at` with its next poll planned at `next_poll_at`, recorded as a fetch records one.
        fn read(&self, id: &AccountId, short: f64, long: f64, at: i64, next_poll_at: i64) {
            let store = self.engine.store().unwrap();
            let row = store.account(id).unwrap().unwrap();
            let (s, l) = if row.provider == cc() {
                ("5h", "7d")
            } else {
                ("daily", "monthly")
            };
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
                window(s, WindowKind::Short, short, T0 + 9_630, 18_000),
                window(l, WindowKind::Long, long, T0 + 291_630, 604_800),
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

        /// `config.toml` as `text`, with an mtime of its own whatever the file system's
        /// granularity.
        fn settings(&self, text: &str, mtime_s: u64) {
            let path = self.env.config_dir().join("config.toml");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            let at = std::time::UNIX_EPOCH + Duration::from_secs(mtime_s);
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(at)
                .unwrap();
        }
    }

    /// The loop's sleep on the fixture's wall clock: each slice moves the clock on by its
    /// length, then `then` runs with the slice's number (from 0), to jump the clock, rewrite the
    /// settings or send a signal.
    struct Slices<'a> {
        clock: &'a FakeClock,
        taken: RefCell<Vec<Duration>>,
        then: Box<dyn Fn(usize) + 'a>,
    }

    impl<'a> Slices<'a> {
        fn new(clock: &'a FakeClock, then: impl Fn(usize) + 'a) -> Self {
            Slices {
                clock,
                taken: RefCell::new(Vec::new()),
                then: Box::new(then),
            }
        }

        /// A sleep `--once` must never take.
        fn none(clock: &'a FakeClock) -> Self {
            Self::new(clock, |_| panic!("--once never sleeps"))
        }
    }

    impl Sleeper for Slices<'_> {
        fn sleep(&self, d: Duration) {
            let n = self.taken.borrow().len();
            self.taken.borrow_mut().push(d);
            self.clock.advance_ms(d.as_millis() as i64);
            (self.then)(n);
        }
    }

    /// Every event, with the fixture clock's time it was emitted at, in seconds after `T0`.
    struct Recorded<'a> {
        clock: &'a FakeClock,
        events: RefCell<Vec<(i64, AutoEvent)>>,
    }

    impl<'a> Recorded<'a> {
        fn new(clock: &'a FakeClock) -> Self {
            Recorded {
                clock,
                events: RefCell::new(Vec::new()),
            }
        }

        fn events(&self) -> Vec<(i64, AutoEvent)> {
            self.events.borrow().clone()
        }

        /// When `provider` polled: one per tick that reached §11.2 step 3.
        fn polls(&self, of: &ProviderId) -> Vec<i64> {
            self.events
                .borrow()
                .iter()
                .filter_map(|(at, e)| match e {
                    AutoEvent::Poll { provider, .. } if provider == of => Some(*at),
                    _ => None,
                })
                .collect()
        }

        /// Each `no-switch` as (seconds after `T0`, reason, detail).
        fn no_switches(&self) -> Vec<(i64, String, String)> {
            self.events
                .borrow()
                .iter()
                .filter_map(|(at, e)| match e {
                    AutoEvent::NoSwitch { reason, detail, .. } => {
                        Some((*at, reason.clone(), detail.clone()))
                    }
                    _ => None,
                })
                .collect()
        }
    }

    impl EventSink for Recorded<'_> {
        fn emit(&self, e: &AutoEvent) {
            let at = self.clock.now_ms().div_euclid(1000) - T0;
            self.events.borrow_mut().push((at, e.clone()));
        }
    }

    fn looping() -> AutoRun {
        AutoRun::default()
    }

    /// `b` at position 2 is the live login, `a` at position 1 a candidate.
    fn two(fx: &Fx) -> (AccountId, AccountId) {
        (fx.add("a@x.co"), fx.add("b@x.co"))
    }

    #[test]
    fn flags_override_the_settings_clamped_to_their_ranges() {
        let settings = Settings {
            threshold: 80.0,
            interval_seconds: 120,
            cooldown_seconds: 600,
            hysteresis_pct: 5.0,
            strategy: Strategy::ConsumeFirst,
            include_api_key_accounts: true,
            unhealthy_ticks: 7,
            models: vec!["Fable".into()],
            ..Settings::default()
        };
        let from_file = auto_config(&settings, &AutoFlags::default(), Some("7d"));
        assert_eq!(
            from_file,
            AutoConfig {
                threshold: 80.0,
                hysteresis_pct: 5.0,
                cooldown_s: 600,
                interval_s: 120,
                unhealthy_ticks: 7,
                strategy: Strategy::ConsumeFirst,
                include_api_key_accounts: true,
                models: vec!["Fable".into()],
                long_window: Some("7d".into()),
            }
        );
        let flags = AutoFlags {
            threshold: Some(120.0),
            interval_s: Some(5),
            cooldown_s: Some(-1),
            strategy: Some(Strategy::Best),
            models: Some(Vec::new()),
            include_api_key_accounts: Some(false),
        };
        let flagged = auto_config(&settings, &flags, None);
        assert_eq!(
            flagged,
            AutoConfig {
                threshold: 99.9,
                cooldown_s: 0,
                interval_s: 15,
                strategy: Strategy::Best,
                include_api_key_accounts: false,
                models: Vec::new(),
                long_window: None,
                ..from_file
            }
        );
        let high = AutoFlags {
            threshold: Some(10.0),
            interval_s: Some(7_200),
            cooldown_s: Some(100_000),
            ..AutoFlags::default()
        };
        let c = auto_config(&settings, &high, None);
        assert_eq!(
            (c.threshold, c.interval_s, c.cooldown_s),
            (50.0, 3_600, 86_400)
        );
    }

    #[test]
    fn auto_runs_every_provider_with_two_switchable_accounts_or_the_one_named() {
        let fx = Fx::new();
        assert_eq!(
            providers(&fx.engine, None).unwrap(),
            Vec::<ProviderId>::new()
        );
        let (a, _b) = two(&fx);
        fx.add_fake("alice");
        assert_eq!(providers(&fx.engine, None).unwrap(), [cc()]);
        let bob = fx.add_fake("bob");
        assert_eq!(providers(&fx.engine, None).unwrap(), [cc(), fake()]);
        // A quarantine is the tick's to fail over from or release; a disabled account is out.
        let store = fx.engine.store().unwrap();
        store
            .set_quarantine(&a, "invalid_grant", "sha256:x", 1)
            .unwrap();
        store.set_disabled(&bob, true).unwrap();
        assert_eq!(providers(&fx.engine, None).unwrap(), [cc()]);
        assert_eq!(providers(&fx.engine, Some(&fake())).unwrap(), [fake()]);
        assert!(matches!(
            providers(&fx.engine, Some(&ProviderId::new("nope"))),
            Err(EngineError::UnknownProvider(_))
        ));
    }

    #[test]
    fn with_no_provider_to_switch_on_auto_fails_before_any_tick() {
        let fx = Fx::new();
        fx.add("a@x.co");
        let sink = Recorded::new(&fx.clock);
        let none = Slices::none(&fx.clock);
        for run in [
            looping(),
            AutoRun {
                once: true,
                ..looping()
            },
        ] {
            let ended = run_loop(&fx.engine, &run, &sink, &none, &mut || 0.0);
            assert!(
                matches!(ended, Err(AutoError::NothingToSwitch)),
                "{ended:?}"
            );
        }
        assert!(sink.events().is_empty());
    }

    #[test]
    fn each_provider_ticks_on_its_own_wall_clock_schedule_in_slices_of_a_second() {
        // Claude Code keeps the default 60 s; FakeAgent's own table says 20 s. The jitter is
        // pinned at +1, so the delays are 66 s and 22 s (§11.4: interval × 1.1).
        let fx = Fx::new();
        let (a, b) = two(&fx);
        let (alice, bob) = (fx.add_fake("alice"), fx.add_fake("bob"));
        for (id, long) in [(&a, 10.0), (&b, 50.0), (&alice, 10.0), (&bob, 50.0)] {
            fx.read(id, 10.0, long, T0, T0 + 300);
        }
        fx.settings(
            "[provider.fake-agent.autoswitch]\ninterval_seconds = 20\n",
            1,
        );
        let cancel = fx.engine.cancel().clone();
        let clock = fx.clock.clone();
        let slices = Slices::new(&fx.clock, move |_| {
            if clock.now_ms() >= (T0 + 120) * 1000 {
                cancel.request(libc::SIGTERM);
            }
        });
        let sink = Recorded::new(&fx.clock);
        let code = run_loop(&fx.engine, &looping(), &sink, &slices, &mut || 1.0).unwrap();
        assert_eq!(code, 0, "a signal stops the loop cleanly");
        assert_eq!(sink.polls(&cc()), [0, 66]);
        assert_eq!(sink.polls(&fake()), [0, 22, 44, 66, 88, 110]);
        let taken = slices.taken.borrow();
        assert!(taken.iter().all(|d| *d <= Duration::from_secs(1)));
        assert_eq!(taken.iter().sum::<Duration>(), Duration::from_secs(120));
        assert!(fx.http.requests().is_empty(), "nothing was due");
    }

    #[test]
    fn the_next_tick_comes_no_later_than_the_live_account_s_next_poll() {
        // §11.4: a 300 s interval is shortened to the live account's next poll, 100 s out.
        let fx = Fx::new();
        let (a, b) = two(&fx);
        fx.read(&a, 10.0, 10.0, T0, T0 + 300);
        fx.read(&b, 20.0, 50.0, T0, T0 + 100);
        let (clock, cancel) = (fx.clock.clone(), fx.engine.cancel().clone());
        let slices = Slices::new(&fx.clock, move |_| {
            if clock.now_ms() > (T0 + 100) * 1000 {
                cancel.request(libc::SIGTERM);
            }
        });
        let run = AutoRun {
            flags: AutoFlags {
                interval_s: Some(300),
                ..AutoFlags::default()
            },
            ..looping()
        };
        let sink = Recorded::new(&fx.clock);
        assert_eq!(
            run_loop(&fx.engine, &run, &sink, &slices, &mut || 0.0).unwrap(),
            0
        );
        assert_eq!(sink.polls(&cc()), [0, 100]);
    }

    #[test]
    fn a_long_delay_is_announced_with_a_sleep_event() {
        // Review Focus 4, the loop's half: every candidate exhausted sleeps until the earliest
        // recovery plus 60 s, capped at 600 s, and says so.
        let fx = Fx::new();
        let (a, b) = two(&fx);
        fx.read(&a, 100.0, 50.0, T0, T0 + 300);
        fx.read(&b, 95.0, 50.0, T0, T0 + 300);
        let cancel = fx.engine.cancel().clone();
        let slices = Slices::new(&fx.clock, move |_| cancel.request(libc::SIGINT));
        let sink = Recorded::new(&fx.clock);
        let code = run_loop(&fx.engine, &looping(), &sink, &slices, &mut || 0.0).unwrap();
        assert_eq!(code, 0);
        let events = sink.events();
        assert_eq!(
            events[events.len() - 2..],
            [
                (
                    0,
                    AutoEvent::AllExhausted {
                        provider: cc(),
                        earliest_reset_at: Some(T0 + 9_630),
                    }
                ),
                (
                    0,
                    AutoEvent::Sleep {
                        provider: cc(),
                        seconds: 600.0,
                        until: T0 + 600,
                    }
                ),
            ]
        );
    }

    #[test]
    fn a_suspend_past_the_deadline_ticks_on_waking_and_never_on_the_old_readings() {
        // Review Focus 2. The laptop sleeps for an hour during the first slice after a tick.
        // The loop's deadline is wall clock, so it ticks the moment it wakes, not 59 s of
        // awake time later. The readings are then over an hour old: past even §8.4's extended
        // trust. The tick fetches each account once, finds nothing (every request fails here),
        // and reports the usage unknown rather than deciding on the old readings.
        let fx = Fx::new();
        let (a, b) = two(&fx);
        fx.read(&a, 10.0, 10.0, T0 - 60, T0 + 240);
        fx.read(&b, 20.0, 50.0, T0 - 60, T0 + 240);
        let (clock, cancel) = (fx.clock.clone(), fx.engine.cancel().clone());
        let slices = Slices::new(&fx.clock, move |n| match n {
            0 => clock.advance_ms(3_600_000),
            _ => cancel.request(libc::SIGTERM),
        });
        let sink = Recorded::new(&fx.clock);
        let code = run_loop(&fx.engine, &looping(), &sink, &slices, &mut || 0.0).unwrap();
        assert_eq!(code, 0);
        assert_eq!(sink.polls(&cc()), [0, 3_601], "one slice, then the tick");
        assert_eq!(slices.taken.borrow().len(), 2);
        let headroom: Vec<BTreeMap<u32, Option<f64>>> = sink
            .events()
            .into_iter()
            .filter_map(|(_, e)| match e {
                AutoEvent::Poll { headroom_pct, .. } => Some(headroom_pct),
                _ => None,
            })
            .collect();
        assert_eq!(
            headroom,
            [
                BTreeMap::from([(1, Some(90.0)), (2, Some(50.0))]),
                BTreeMap::from([(1, None), (2, None)]),
            ]
        );
        assert_eq!(
            sink.no_switches(),
            [
                (0, "below-threshold".into(), String::new()),
                (3_601, "active-usage-unknown".into(), "1/3".into()),
            ]
        );
        // No burst: one usage request per account, all of them after waking.
        let requests = fx.http.requests();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert!(
            requests
                .iter()
                .all(|r| r.method == Method::Get && r.url == CC_USAGE)
        );
    }

    #[test]
    fn a_wall_clock_set_back_never_stretches_the_wait_past_the_delay() {
        // The clock is set back an hour during the first slice after a tick. The next tick
        // still comes the 60 s delay after that, by the clock as it now reads, not an hour
        // later.
        let fx = Fx::new();
        let (a, b) = two(&fx);
        fx.read(&a, 10.0, 10.0, T0, T0 + 300);
        fx.read(&b, 20.0, 50.0, T0, T0 + 300);
        let (clock, cancel) = (fx.clock.clone(), fx.engine.cancel().clone());
        let slices = Slices::new(&fx.clock, move |n| {
            if n == 0 {
                clock.set(clock.now_ms() - 3_600_000);
            } else if clock.now_ms() > (T0 - 3_539) * 1000 {
                cancel.request(libc::SIGTERM);
            }
        });
        let sink = Recorded::new(&fx.clock);
        let code = run_loop(&fx.engine, &looping(), &sink, &slices, &mut || 0.0).unwrap();
        assert_eq!(code, 0);
        assert_eq!(sink.polls(&cc()), [0, -3_539]);
        let taken = slices.taken.borrow();
        assert_eq!(
            taken[..61].iter().sum::<Duration>(),
            Duration::from_secs(61)
        );
    }

    #[test]
    fn changed_settings_are_read_again_before_the_next_tick_and_flags_still_win() {
        // b at 70 % stays below the default 90. During the first sleep the file lowers the
        // threshold to 60 (and gives an interval out of range, which warns and keeps the
        // default). The next tick runs with 60 and switches, unless `--threshold 95` holds.
        for (flag, threshold) in [(None, 60.0), (Some(95.0), 95.0)] {
            let fx = Fx::new();
            let (a, b) = two(&fx);
            fx.read(&a, 10.0, 10.0, T0, T0 + 300);
            fx.read(&b, 20.0, 70.0, T0, T0 + 300);
            let env = fx.env.clone();
            let file = env.config_dir().join("config.toml");
            let (clock, cancel) = (fx.clock.clone(), fx.engine.cancel().clone());
            let slices = Slices::new(&fx.clock, |n| {
                if n == 0 {
                    fx.settings("[autoswitch]\nthreshold = 60\ninterval_seconds = 5\n", 2);
                }
                if clock.now_ms() > (T0 + 60) * 1000 {
                    cancel.request(libc::SIGTERM);
                }
            });
            let sink = Recorded::new(&fx.clock);
            let run = AutoRun {
                flags: AutoFlags {
                    threshold: flag,
                    ..AutoFlags::default()
                },
                ..looping()
            };
            assert_eq!(
                run_loop(&fx.engine, &run, &sink, &slices, &mut || 0.0).unwrap(),
                0
            );
            let thresholds: Vec<f64> = sink
                .events()
                .into_iter()
                .filter_map(|(_, e)| match e {
                    AutoEvent::Poll { threshold, .. } => Some(threshold),
                    _ => None,
                })
                .collect();
            assert_eq!(thresholds, [flag.unwrap_or(90.0), threshold], "{flag:?}");
            let warning = format!(
                "{}: `autoswitch.interval_seconds` must be a whole number of seconds from 15 to 3600 (ignored)",
                file.display()
            );
            assert!(sink.events().contains(&(
                60,
                AutoEvent::ConfigWarning {
                    provider: cc(),
                    message: warning,
                }
            )));
            let switched = sink
                .events()
                .iter()
                .any(|(_, e)| matches!(e, AutoEvent::Switch { from: 2, to: 1, .. }));
            assert_eq!(switched, flag.is_none(), "{flag:?}");
        }
    }

    #[test]
    fn a_failed_tick_is_reported_and_the_loop_keeps_its_normal_cadence() {
        // b at 95 % decides to switch to a, whose access token needs a refresh that cannot be
        // sent: §11.2 step 12's error. The next tick comes after the normal 60 s, not after a
        // blocked tick's 300 s, and no sleep is announced.
        let fx = Fx::new();
        let (a, b) = two(&fx);
        fx.read(&a, 10.0, 10.0, T0, T0 + 300);
        fx.read(&b, 20.0, 95.0, T0, T0 + 300);
        let near = json!({"claudeAiOauth": {"accessToken": "at-a", "refreshToken": "rt-a", "expiresAt": T0 * 1000 + 60_000, "refreshTokenExpiresAt": FAR_MS}});
        fx.kc.put(SERVICE, a.as_str(), near.to_string().as_bytes());
        let (clock, cancel) = (fx.clock.clone(), fx.engine.cancel().clone());
        let slices = Slices::new(&fx.clock, move |_| {
            if clock.now_ms() > (T0 + 60) * 1000 {
                cancel.request(libc::SIGTERM);
            }
        });
        let sink = Recorded::new(&fx.clock);
        let code = run_loop(&fx.engine, &looping(), &sink, &slices, &mut || 0.0).unwrap();
        assert_eq!(code, 0);
        assert_eq!(sink.polls(&cc()), [0, 60]);
        let errors: Vec<(i64, bool)> = sink
            .events()
            .into_iter()
            .filter_map(|(at, e)| match e {
                AutoEvent::Error { transient, .. } => Some((at, transient)),
                _ => None,
            })
            .collect();
        assert_eq!(errors, [(0, true), (60, true)]);
        assert!(
            !sink
                .events()
                .iter()
                .any(|(_, e)| matches!(e, AutoEvent::Sleep { .. }))
        );
    }

    #[test]
    fn a_tick_that_fails_with_an_error_is_an_error_event_and_an_interruption_is_not() {
        // The engine reports every failure but a store's (and an interruption) itself.
        let fx = Fx::new();
        let sink = Recorded::new(&fx.clock);
        let failed = Err(EngineError::Io(std::io::Error::other("disk I/O error")));
        let (code, decision) = settle(&cc(), failed, &sink).unwrap();
        assert_eq!(code, 1);
        let cfg = auto_config(&Settings::default(), &AutoFlags::default(), None);
        assert_eq!(
            next_delay(&decision, &cfg, T0, None, 0.0),
            60,
            "the normal cadence"
        );
        assert_eq!(
            sink.events(),
            [(
                0,
                AutoEvent::Error {
                    provider: cc(),
                    message: "disk I/O error".into(),
                    transient: false,
                }
            )]
        );
        let interrupted = settle(&cc(), Err(EngineError::Interrupted(15)), &sink);
        assert_eq!(interrupted.unwrap_err().signal(), Some(15));
        assert_eq!(sink.events().len(), 1);
    }

    /// Decision 8: a signal that lands inside a tick, at its collection's cancellation point,
    /// ends the loop with exit 0 as one between ticks does.
    #[cfg(feature = "test-support")]
    #[test]
    fn a_signal_met_inside_a_tick_stops_the_loop_with_exit_0() {
        let fx = Fx::new();
        two(&fx); // b is live and never read: the tick's collection reserves a fetch for it
        let cancel = fx.engine.cancel().clone();
        fx.engine.on_point(
            "usage-reserved",
            Box::new(move || cancel.request(libc::SIGINT)),
        );
        let sink = Recorded::new(&fx.clock);
        let none = Slices::new(&fx.clock, |_| panic!("the loop slept after the signal"));
        let code = run_loop(&fx.engine, &looping(), &sink, &none, &mut || 0.0).unwrap();
        assert_eq!(code, 0);
        assert!(sink.events().is_empty(), "the tick stopped before its poll");
        assert!(fx.http.requests().is_empty());
    }

    #[test]
    fn a_loop_skips_a_provider_whose_engine_runs_elsewhere_and_fails_when_none_is_left() {
        // Review Focus 3, the loop's half (§11.1).
        let fx = Fx::new();
        let (a, b) = two(&fx);
        let (alice, bob) = (fx.add_fake("alice"), fx.add_fake("bob"));
        for id in [&a, &b, &alice, &bob] {
            fx.read(id, 10.0, 10.0, T0, T0 + 300);
        }
        let cfg = |p: &ProviderId| configured(&fx.engine, p, &AutoFlags::default()).unwrap().0;
        let held_cc = fx.engine.auto(&cc(), cfg(&cc()), false).unwrap().unwrap();
        let cancel = fx.engine.cancel().clone();
        let slices = Slices::new(&fx.clock, move |_| cancel.request(libc::SIGTERM));
        let sink = Recorded::new(&fx.clock);
        let code = run_loop(&fx.engine, &looping(), &sink, &slices, &mut || 0.0).unwrap();
        assert_eq!(code, 0);
        let events = sink.events();
        assert_eq!(
            events[0],
            (
                0,
                AutoEvent::NoSwitch {
                    provider: cc(),
                    reason: "engine-running".into(),
                    detail: String::new(),
                }
            )
        );
        assert!(sink.polls(&cc()).is_empty());
        assert_eq!(sink.polls(&fake()), [0]);
        fx.engine
            .cancel()
            .cell()
            .store(0, std::sync::atomic::Ordering::SeqCst);
        let held_fake = fx
            .engine
            .auto(&fake(), cfg(&fake()), false)
            .unwrap()
            .unwrap();
        let quiet = Recorded::new(&fx.clock);
        let ended = run_loop(
            &fx.engine,
            &looping(),
            &quiet,
            &Slices::none(&fx.clock),
            &mut || 0.0,
        );
        assert!(
            matches!(&ended, Err(AutoError::AlreadyRuns(p)) if *p == [cc(), fake()]),
            "{ended:?}"
        );
        assert!(quiet.events().is_empty());
        drop((held_cc, held_fake));
    }

    #[test]
    fn once_ticks_each_provider_once_and_exits_with_the_most_severe_code() {
        // Claude Code switches (0); FakeAgent has every candidate exhausted (3). With Claude
        // Code's engine running elsewhere, it reports `engine-running` (2) instead.
        let fx = Fx::new();
        let (a, b) = two(&fx);
        let (alice, bob) = (fx.add_fake("alice"), fx.add_fake("bob"));
        fx.read(&a, 10.0, 10.0, T0, T0 + 300);
        fx.read(&b, 20.0, 95.0, T0, T0 + 300);
        fx.read(&alice, 100.0, 50.0, T0, T0 + 300);
        fx.read(&bob, 95.0, 50.0, T0, T0 + 300);
        let once = AutoRun {
            once: true,
            ..looping()
        };
        let none = Slices::none(&fx.clock);
        let cfg = configured(&fx.engine, &cc(), &AutoFlags::default())
            .unwrap()
            .0;
        let held = fx.engine.auto(&cc(), cfg, false).unwrap().unwrap();
        let sink = Recorded::new(&fx.clock);
        assert_eq!(
            run_loop(&fx.engine, &once, &sink, &none, &mut || 0.0).unwrap(),
            3
        );
        assert_eq!(
            sink.no_switches(),
            [
                (0, "engine-running".into(), String::new()),
                (0, "all-exhausted".into(), "2h40m".into()),
            ]
        );
        drop(held);
        let sink = Recorded::new(&fx.clock);
        assert_eq!(
            run_loop(&fx.engine, &once, &sink, &none, &mut || 0.0).unwrap(),
            0
        );
        assert!(
            sink.events()
                .iter()
                .any(|(_, e)| matches!(e, AutoEvent::Switch { provider, from: 2, to: 1, .. } if *provider == cc()))
        );
        assert!(
            sink.events()
                .iter()
                .all(|(_, e)| !matches!(e, AutoEvent::Sleep { .. }))
        );
    }

    #[test]
    fn an_interrupted_once_is_the_command_s_interruption() {
        // §14.1: `--once` exits 128 + n, unlike the loop.
        let fx = Fx::new();
        two(&fx);
        fx.engine.cancel().request(libc::SIGTERM);
        let once = AutoRun {
            once: true,
            ..looping()
        };
        let sink = Recorded::new(&fx.clock);
        let ended = run_loop(
            &fx.engine,
            &once,
            &sink,
            &Slices::none(&fx.clock),
            &mut || 0.0,
        );
        assert!(
            matches!(&ended, Err(AutoError::Engine(e)) if e.signal() == Some(libc::SIGTERM)),
            "{ended:?}"
        );
        assert!(sink.events().is_empty());
    }

    #[test]
    fn consume_first_named_for_a_provider_without_a_long_window_is_refused_before_any_engine() {
        // §4.5: `--strategy consume-first` with `--provider` naming FakeAgent, which names no
        // long window. Neither a loop nor `--once` ticks, and no engine lock is taken.
        let fx = Fx::new();
        fx.add_fake("alice");
        fx.add_fake("bob");
        let named = AutoRun {
            provider: Some(fake()),
            flags: AutoFlags {
                strategy: Some(Strategy::ConsumeFirst),
                ..AutoFlags::default()
            },
            ..looping()
        };
        for once in [false, true] {
            let run = AutoRun {
                once,
                ..named.clone()
            };
            let sink = Recorded::new(&fx.clock);
            let ended = run_loop(
                &fx.engine,
                &run,
                &sink,
                &Slices::none(&fx.clock),
                &mut || 0.0,
            );
            assert!(
                matches!(&ended, Err(AutoError::NoLongWindow(p)) if *p == fake()),
                "{ended:?}"
            );
            assert!(sink.events().is_empty());
        }
        let lock = fx
            .env
            .data_dir()
            .join(format!("locks/autoswitch-{}.lock", fake()));
        assert!(!lock.exists());
    }

    #[test]
    fn consume_first_from_the_settings_or_without_provider_runs_best_with_one_warning() {
        // §4.5's two fallbacks, neither an error: `autoswitch.strategy` with `--provider` naming
        // FakeAgent, and `--strategy consume-first` with no `--provider`. FakeAgent's engine
        // decides as `best` (below the threshold, it stays) and says so once.
        let fx = Fx::new();
        let (alice, bob) = (fx.add_fake("alice"), fx.add_fake("bob"));
        fx.read(&alice, 10.0, 10.0, T0, T0 + 300);
        fx.read(&bob, 20.0, 50.0, T0, T0 + 300);
        let once = AutoRun {
            once: true,
            ..looping()
        };
        let best = |sink: &Recorded| {
            let warnings: Vec<AutoEvent> = sink
                .events()
                .into_iter()
                .filter(|(_, e)| matches!(e, AutoEvent::ConfigWarning { .. }))
                .map(|(_, e)| e)
                .collect();
            assert_eq!(
                warnings,
                [AutoEvent::ConfigWarning {
                    provider: fake(),
                    message:
                        "FakeAgent has no long usage window to rank by, so consume-first runs best"
                            .into(),
                }]
            );
            assert_eq!(
                sink.no_switches(),
                [(0, "below-threshold".into(), String::new())]
            );
        };
        fx.settings("[autoswitch]\nstrategy = \"consume-first\"\n", 1);
        let from_settings = AutoRun {
            provider: Some(fake()),
            ..once.clone()
        };
        let sink = Recorded::new(&fx.clock);
        let none = Slices::none(&fx.clock);
        assert_eq!(
            run_loop(&fx.engine, &from_settings, &sink, &none, &mut || 0.0).unwrap(),
            2
        );
        best(&sink);
        fx.settings("", 2);
        let flagged = AutoRun {
            flags: AutoFlags {
                strategy: Some(Strategy::ConsumeFirst),
                ..AutoFlags::default()
            },
            ..once
        };
        let sink = Recorded::new(&fx.clock);
        assert_eq!(
            run_loop(&fx.engine, &flagged, &sink, &none, &mut || 0.0).unwrap(),
            2
        );
        best(&sink);
    }

    #[test]
    fn the_jitter_draw_stays_within_its_range() {
        for _ in 0..1_000 {
            let j = uniform_jitter();
            assert!((-1.0..1.0).contains(&j), "{j}");
        }
    }

    fn poll(active: u32, headroom: &[(u32, Option<f64>)]) -> AutoEvent {
        AutoEvent::Poll {
            provider: cc(),
            active: Some((active, format!("{}@x.co", ["", "a", "b"][active as usize]))),
            headroom_pct: headroom.iter().copied().collect(),
            threshold: 90.0,
            fetch_errors: BTreeMap::new(),
            windows_pct: BTreeMap::new(),
        }
    }

    fn no_switch_event(reason: &str, detail: &str) -> AutoEvent {
        AutoEvent::NoSwitch {
            provider: cc(),
            reason: reason.into(),
            detail: detail.into(),
        }
    }

    #[test]
    fn every_event_is_one_object_in_section_11_4_s_shape() {
        let poll = AutoEvent::Poll {
            provider: cc(),
            active: Some((2, "b@x.co".into())),
            headroom_pct: BTreeMap::from([(1, Some(80.0)), (2, None)]),
            threshold: 90.0,
            fetch_errors: BTreeMap::from([(2, "http-429".into())]),
            windows_pct: BTreeMap::from([(1, BTreeMap::from([("5h".into(), 20.0)]))]),
        };
        let quiet = AutoEvent::Poll {
            provider: cc(),
            active: None,
            headroom_pct: BTreeMap::new(),
            threshold: 90.0,
            fetch_errors: BTreeMap::new(),
            windows_pct: BTreeMap::new(),
        };
        let cases = [
            (
                poll,
                json!({"event": "poll", "active": {"number": 2, "email": "b@x.co"},
                       "headroomPct": {"1": 80.0, "2": null}, "threshold": 90.0,
                       "fetchErrors": {"2": "http-429"}, "windowsPct": {"1": {"5h": 20.0}}}),
            ),
            (
                quiet,
                json!({"event": "poll", "active": null, "headroomPct": {}, "threshold": 90.0}),
            ),
            (
                AutoEvent::Switch {
                    provider: cc(),
                    trigger: tagteam_core::autoswitch::Trigger::Failover,
                    from: 2,
                    to: 1,
                    warnings: vec!["w".into()],
                    dry_run: true,
                },
                json!({"event": "switch", "trigger": "failover", "from": 2, "to": 1,
                       "warnings": ["w"], "dryRun": true}),
            ),
            (
                no_switch_event("cooldown", "3m"),
                json!({"event": "no-switch", "reason": "cooldown", "detail": "3m"}),
            ),
            (
                AutoEvent::AccountQuarantined {
                    provider: cc(),
                    number: 3,
                    email: "w@corp.com".into(),
                    reason: "invalid_grant".into(),
                },
                json!({"event": "account-quarantined", "number": 3, "email": "w@corp.com",
                       "reason": "invalid_grant"}),
            ),
            (
                AutoEvent::AccountUnquarantined {
                    provider: cc(),
                    number: 3,
                    email: "w@corp.com".into(),
                    reason: "account-replaced".into(),
                },
                json!({"event": "account-unquarantined", "number": 3, "email": "w@corp.com",
                       "reason": "account-replaced"}),
            ),
            (
                AutoEvent::AllExhausted {
                    provider: cc(),
                    earliest_reset_at: Some(T0 + 9_630),
                },
                json!({"event": "all-exhausted", "earliestResetAt": "2026-09-21T16:53:50Z"}),
            ),
            (
                AutoEvent::AllExhausted {
                    provider: cc(),
                    earliest_reset_at: None,
                },
                json!({"event": "all-exhausted", "earliestResetAt": null}),
            ),
            (
                AutoEvent::Sleep {
                    provider: cc(),
                    seconds: 600.0,
                    until: T0 + 600,
                },
                json!({"event": "sleep", "seconds": 600.0, "until": "2026-09-21T14:23:20Z"}),
            ),
            (
                AutoEvent::Error {
                    provider: cc(),
                    message: "m".into(),
                    transient: true,
                },
                json!({"event": "error", "message": "m", "transient": true}),
            ),
            (
                AutoEvent::ConfigWarning {
                    provider: cc(),
                    message: "m".into(),
                },
                json!({"event": "config-warning", "message": "m"}),
            ),
        ];
        for (event, fields) in cases {
            let mut want = json!({"schemaVersion": 1, "event": fields["event"],
                                  "ts": "2026-09-21T14:13:20Z", "provider": "claude-code"});
            for (k, v) in fields.as_object().unwrap() {
                want[k] = v.clone();
            }
            assert_eq!(event_json(&event, T0), want);
        }
        let line = event_json(&no_switch_event("cooldown", ""), T0).to_string();
        assert_eq!(
            line,
            r#"{"schemaVersion":1,"event":"no-switch","ts":"2026-09-21T14:13:20Z","provider":"claude-code","reason":"cooldown","detail":""}"#
        );
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
             t+0  #2 b@x.co  95%  switched to #1 (proactive)\n\
             t+0  #1 a@x.co  —  no switch: active-usage-unknown (1/3)\n\
             t+0  no switch: no-active-account\n\
             t+0  #2 b@x.co  95%  no switch: all-exhausted, the first back at t+9630 (2h40m)\n\
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
            "t+60  no switch: no-active-account\n"
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
            "t+0  <claude-code>  #2 b@x.co  \x1b[31m95%\x1b[0m  no switch: below-threshold\n\
             t+0  <claude-code>  #2 b@x.co  \x1b[33m75%\x1b[0m  no switch: below-threshold\n\
             t+0  <claude-code>  #2 b@x.co  50%  no switch: below-threshold\n"
        );
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
