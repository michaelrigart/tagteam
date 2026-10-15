//! D4.3: one job per registered provider. Each run evaluates the provider's mode from D4.3's
//! table (`mode_for`), then does that mode's work when its deadline has come: an `auto` tick,
//! exactly as `tagteam auto` ticks, or a `collect` round through the collector (§8.3, §8.6).
//! `idle` and `waiting` do nothing. The mode is evaluated at least every 60 s, after each
//! settings reload (`settings_changed`), and whenever the store watch wakes the job (D4.4). A
//! mode change is a `mode` event and an INFO line. After each run the job writes its line of
//! the status board (Decision 12, D5.4).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use tagteam_core::autoswitch::{
    AutoConfig, Decision, NoSwitchReason, Trigger, announces_sleep, next_delay,
};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::auto::{AutoEngine, AutoEvent, EventSink, QuarantineWatch, TickOutcome};
use tagteam_engine::collect::{CollectMode, Collected};
use tagteam_engine::settings::Settings;
use tagteam_engine::store::AccountRow;
use tagteam_engine::{Engine, EngineError};
use tagteam_provider::Read;
use tagteam_provider::keychain::ReadCounts;
use tagteam_provider::process::Liveness;

use crate::auto::{AutoFlags, auto_config, no_switch};
use crate::events::{AutoEvents, DaemonEvent, DaemonSink, Mode, StopReason, WaitingFor};
use crate::scheduler::{Job, JobCx, Ran};
use crate::status::{LastOutcome, LastSwitch, OutcomeKind, ProviderStatus, StatusBoard};

/// Every provider job's name starts with this; D5.4's `jobs` leaves them out, and the store
/// watch wakes them by it (D4.4).
pub const JOB_PREFIX: &str = "provider:";
/// D4.3: a provider's mode is evaluated again at least this often.
const RECHECK_MS: i64 = 60_000;
/// D4.3: a `collect` round's delay is never shorter.
const COLLECT_FLOOR_S: i64 = 60;
/// §11.4's jitter: a delay of `interval × U(0.9, 1.1)`.
const JITTER_FRAC: f64 = 0.1;

/// What a provider job is given besides its engine and its provider.
pub struct ProviderJobCtx {
    /// Where the job writes its line after each run (Decision 12).
    pub board: Arc<StatusBoard>,
    /// The Keychain read counts the keychain backoff compares around each run (D4.1,
    /// Decision 7); empty where there is no Keychain.
    pub keychain_counts: Vec<Arc<ReadCounts>>,
    /// D4.1 step 4: the store's schema was newer than this build at start-up.
    pub schema_newer: bool,
}

/// What D4.3's table decides from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModeInputs {
    /// The store's schema is newer than this build's (D4.1 step 4).
    pub schema_newer: bool,
    pub has_store: bool,
    /// The provider's switchable accounts: enabled, with an identity, as `auto` counts them
    /// (§11.1). A quarantine does not count against an account here.
    pub switchable: usize,
    /// `daemon.autoswitch` for the provider (D7).
    pub autoswitch: bool,
    /// The pid the engine lock's record names, when that process is live (§13.6).
    pub holder_elsewhere: Option<u32>,
    /// This job tried the engine lock just now, and another process held it.
    pub refused: bool,
    /// This job holds the provider's engine lock.
    pub holds_lock: bool,
}

/// D4.3's table, its rows tried in order and the first that applies winning. `holds_lock` is
/// taken as given: entering `auto` is the job's own try of the lock, which a refusal reports
/// back through `refused`.
pub fn mode_for(i: &ModeInputs) -> (Mode, Option<WaitingFor>) {
    if i.schema_newer {
        return (Mode::Waiting, Some(WaitingFor::Schema));
    }
    if !i.has_store || i.switchable == 0 {
        return (Mode::Idle, None);
    }
    if i.refused || i.holder_elsewhere.is_some() {
        return (Mode::Waiting, Some(WaitingFor::EngineLock));
    }
    if i.autoswitch && i.switchable >= 2 && i.holds_lock {
        return (Mode::Auto, None);
    }
    (Mode::Collect, None)
}

/// D4.3: a `collect` round's delay in seconds: `interval × U(0.9, 1.1)` (`jitter` in
/// [-1, 1)), shortened to the earliest planned poll, and never under 60 s.
fn collect_delay_s(
    interval_s: i64,
    now: i64,
    earliest_next_poll_at: Option<i64>,
    jitter: f64,
) -> i64 {
    let j = if jitter.is_finite() {
        jitter.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let mut delay = (interval_s as f64 * (1.0 + j * JITTER_FRAC)).round() as i64;
    if let Some(at) = earliest_next_poll_at {
        delay = delay.min(at.saturating_sub(now));
    }
    delay.max(COLLECT_FLOOR_S)
}

/// What a `mode` event last stated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shown {
    mode: Mode,
    waiting_for: Option<WaitingFor>,
    holder_pid: Option<u32>,
}

/// A run's work, done: its own next delay, and whether it made a switch.
struct Worked {
    delay_ms: i64,
    switched: bool,
}

/// A tick's events, passed on to the daemon's sink as they come (`AutoEvents`). The switch the
/// tick made and the error it reported are kept for the status board.
struct Watching<'a> {
    inner: AutoEvents<'a>,
    switched: RefCell<Option<(u32, u32, Trigger)>>,
    error: RefCell<Option<String>>,
}

impl<'a> Watching<'a> {
    fn new(sink: &'a dyn DaemonSink) -> Self {
        Watching {
            inner: AutoEvents(sink),
            switched: RefCell::new(None),
            error: RefCell::new(None),
        }
    }
}

impl EventSink for Watching<'_> {
    fn emit(&self, e: &AutoEvent) {
        match e {
            AutoEvent::Switch {
                from,
                to,
                trigger,
                dry_run: false,
                ..
            } => *self.switched.borrow_mut() = Some((*from, *to, *trigger)),
            AutoEvent::Error { message, .. } => *self.error.borrow_mut() = Some(message.clone()),
            _ => {}
        }
        self.inner.emit(e);
    }

    fn tick_done(&self, provider: &ProviderId) {
        self.inner.tick_done(provider);
    }
}

/// D4.3: one provider's job.
pub struct ProviderJob<'e> {
    engine: &'e Engine,
    provider: ProviderId,
    ctx: ProviderJobCtx,
    /// The provider's long window (§4.5), which its `AutoConfig` carries.
    long_window: Option<String>,
    /// The provider's settings as last read (Decision 8).
    settings: Settings,
    /// The warnings of the read `new` made, emitted as `config-warning` events at the start of
    /// the first run; emptied then, or by a reload before it, which reports its own read.
    first_warnings: Vec<String>,
    cfg: AutoConfig,
    /// The auto-switch engine while in `auto`. It holds the engine lock (§11.1).
    auto: Option<AutoEngine<'e>>,
    /// `collect`'s quarantine report (Decision 6), started afresh on each entry into
    /// `collect`: a tick reported everything before it.
    quarantines: QuarantineWatch,
    /// `None` before the first evaluation.
    mode: Option<Mode>,
    waiting_for: Option<WaitingFor>,
    holder_pid: Option<u32>,
    shown: Option<Shown>,
    /// The work's wall-clock deadline (the next tick or round), epoch ms, and the delay it
    /// was set for, which the wait never exceeds (D4.2).
    work_due_ms: i64,
    work_delay_ms: i64,
    last_tick_at: Option<i64>,
    last_outcome: Option<LastOutcome>,
    last_switch: Option<LastSwitch>,
}

impl<'e> ProviderJob<'e> {
    /// `provider`'s job, with its settings as `config.toml` holds them now. The read's warnings
    /// are the job's to report, as `config-warning` events at the start of its first run: the
    /// command reported its own read's, but the file may have changed since (during an unlock
    /// offer, say), and no mtime change would then come to report it (Decision 8). It emits
    /// nothing and takes no lock before its first run.
    pub fn new(engine: &'e Engine, provider: ProviderId, ctx: ProviderJobCtx) -> Self {
        let long_window = engine
            .provider(&provider)
            .ok()
            .and_then(|p| p.primary_long_window().map(str::to_owned));
        let (settings, first_warnings) = Settings::load(engine.env(), &provider);
        let cfg = auto_config(&settings, &AutoFlags::default(), long_window.as_deref());
        ProviderJob {
            engine,
            provider,
            ctx,
            long_window,
            settings,
            first_warnings,
            cfg,
            auto: None,
            quarantines: QuarantineWatch::new(),
            mode: None,
            waiting_for: None,
            holder_pid: None,
            shown: None,
            work_due_ms: 0,
            work_delay_ms: 0,
            last_tick_at: None,
            last_outcome: None,
            last_switch: None,
        }
    }

    fn now_s(&self) -> i64 {
        self.engine.now_ms().div_euclid(1000)
    }

    /// The pid the engine lock's record names, when that process is live by an exact pid and
    /// start-time match, judged as `doctor` judges it (§13.6): a start time that cannot be
    /// read counts as live. The lock itself is never tried here, since a starting `auto`
    /// would then find it taken (D4.3).
    fn live_holder(&self) -> Option<u32> {
        match self.engine.engine_holder(&self.provider) {
            Read::Present(stamp) => match stamp.liveness() {
                Liveness::Live | Liveness::Unknown(_) => Some(stamp.pid),
                Liveness::Dead => None,
            },
            Read::Absent | Read::Unreadable(_) => None,
        }
    }

    /// D4.3's inputs as they stand. A store whose schema is newer is never opened.
    fn inputs(&self) -> Result<ModeInputs, EngineError> {
        if self.ctx.schema_newer {
            return Ok(ModeInputs {
                schema_newer: true,
                ..ModeInputs::default()
            });
        }
        let store = self.engine.existing_store()?;
        let switchable = match &store {
            Some(s) => s
                .accounts(&self.provider)?
                .iter()
                .filter(|r| !r.disabled && r.identity_json.is_object())
                .count(),
            None => 0,
        };
        let holds_lock = self.auto.is_some();
        Ok(ModeInputs {
            schema_newer: false,
            has_store: store.is_some(),
            switchable,
            autoswitch: self.settings.daemon_autoswitch,
            // Holding the lock, the record is this job's own.
            holder_elsewhere: if holds_lock { None } else { self.live_holder() },
            refused: false,
            holds_lock,
        })
    }

    /// D4.3's table, applied. Leaving `auto` drops the engine, which empties its lock record
    /// and then frees the lock (D3 change 3). A provider that would be in `auto` but for the
    /// lock tries it once; a refusal is `waiting`, naming the record's live holder if any.
    fn evaluate(&mut self, cx: &JobCx<'_>) -> Result<(), EngineError> {
        let mut inputs = self.inputs()?;
        let (mut mode, mut waiting_for) = mode_for(&inputs);
        if mode != Mode::Auto {
            self.auto = None;
        }
        let with_lock = mode_for(&ModeInputs {
            holds_lock: true,
            ..inputs
        });
        if mode != Mode::Auto && with_lock.0 == Mode::Auto {
            match self.engine.auto(&self.provider, self.cfg.clone(), false)? {
                Some(auto) => {
                    self.auto = Some(auto.with_source("daemon"));
                    inputs.holds_lock = true;
                }
                None => {
                    inputs.refused = true;
                    inputs.holder_elsewhere = self.live_holder();
                }
            }
            (mode, waiting_for) = mode_for(&inputs);
        }
        let holder_pid = match waiting_for {
            Some(WaitingFor::EngineLock) => inputs.holder_elsewhere,
            _ => None,
        };
        self.set_mode(mode, waiting_for, holder_pid, cx);
        Ok(())
    }

    fn set_mode(
        &mut self,
        mode: Mode,
        waiting_for: Option<WaitingFor>,
        holder_pid: Option<u32>,
        cx: &JobCx<'_>,
    ) {
        if self.mode != Some(mode) {
            if mode == Mode::Collect {
                self.quarantines = QuarantineWatch::new();
            }
            // A mode that works starts with its work.
            self.work_due_ms = self.engine.now_ms();
            self.work_delay_ms = 0;
        }
        self.mode = Some(mode);
        self.waiting_for = waiting_for;
        self.holder_pid = holder_pid;
        self.announce(cx);
    }

    /// D4.3: a mode change is a `mode` event (D5.5) and an INFO line (D11).
    fn announce(&mut self, cx: &JobCx<'_>) {
        let Some(mode) = self.mode else {
            return;
        };
        let shown = Shown {
            mode,
            waiting_for: self.waiting_for,
            holder_pid: self.holder_pid,
        };
        if self.shown == Some(shown) {
            return;
        }
        self.shown = Some(shown);
        tracing::info!(
            provider = %self.provider,
            mode = mode.as_str(),
            waiting_for = self.waiting_for.map(|w| w.as_str()),
            holder_pid = self.holder_pid,
            "the provider's daemon mode changed"
        );
        cx.sink.emit(&DaemonEvent::Mode {
            provider: self.provider.clone(),
            mode,
            waiting_for: self.waiting_for,
            holder_pid: self.holder_pid,
            backoff: None,
        });
    }

    /// D4.3: a run that fails is an `error` event, and the job keeps its cadence.
    fn failed(&mut self, cx: &JobCx<'_>, e: &EngineError) {
        let message = e.to_string();
        cx.sink.emit(&DaemonEvent::Auto(AutoEvent::Error {
            provider: self.provider.clone(),
            message: message.clone(),
            transient: e.kind() == "lock-timeout",
        }));
        self.last_outcome = Some(LastOutcome {
            kind: OutcomeKind::Error,
            reason: Some(message),
            at: self.now_s(),
        });
    }

    /// One run: the mode first, then its work when the work's deadline has come.
    fn step(&mut self, cx: &mut JobCx<'_>) -> Ran {
        let now = self.engine.now_ms();
        // D4.2: a wall clock set back never stretches the wait past the delay it was set for.
        self.work_due_ms = self.work_due_ms.min(now.saturating_add(self.work_delay_ms));
        if let Err(e) = self.evaluate(cx) {
            if e.signal().is_some() {
                return Ran::Stop(StopReason::Signal);
            }
            self.failed(cx, &e);
            return Ran::After(RECHECK_MS);
        }
        if !matches!(self.mode, Some(Mode::Auto | Mode::Collect)) {
            return Ran::After(RECHECK_MS);
        }
        let now = self.engine.now_ms();
        if now < self.work_due_ms {
            // D4.3: evaluated again at least every 60 s, while the work waits its deadline.
            return Ran::After((self.work_due_ms - now).clamp(1, RECHECK_MS));
        }
        self.last_tick_at = Some(now.div_euclid(1000));
        let worked = if self.mode == Some(Mode::Auto) {
            self.tick(cx)
        } else {
            self.collect(cx)
        };
        let Some(worked) = worked else {
            return Ran::Stop(StopReason::Signal);
        };
        self.after_run_notices(cx, worked.switched);
        let now = self.engine.now_ms();
        self.work_delay_ms = worked.delay_ms;
        self.work_due_ms = now + worked.delay_ms;
        Ran::After(worked.delay_ms.clamp(1, RECHECK_MS))
    }

    /// D4.3 `auto`: one §11.2 tick, settled as `tagteam auto` settles one (§11.4), its delay
    /// from `next_delay` with a `sleep` event when it is long. A tick that ended before its
    /// collection because the live login is unmanaged or absent is followed, in the same run,
    /// by `collect`'s round. `None`: a signal interrupted it (§14.1).
    fn tick(&mut self, cx: &mut JobCx<'_>) -> Option<Worked> {
        let watching = Watching::new(cx.sink);
        let auto = self.auto.as_mut().expect("a job in auto holds its engine");
        let ticked = auto.tick(&watching);
        let at = self.now_s();
        let (outcome, decision) = match ticked {
            Ok(done) => done,
            Err(e) if e.signal().is_some() => return None,
            Err(e) => {
                // A store that failed: the engine reports every other failure itself (§11.4).
                watching.emit(&AutoEvent::Error {
                    provider: self.provider.clone(),
                    message: e.to_string(),
                    transient: false,
                });
                (
                    TickOutcome::Error,
                    no_switch(NoSwitchReason::NoActiveAccount),
                )
            }
        };
        if let Some((from, to, trigger)) = watching.switched.take() {
            self.last_switch = Some(LastSwitch {
                at,
                from,
                to,
                trigger: trigger.as_str().to_owned(),
            });
        }
        let (kind, reason) = match (outcome, &decision) {
            (TickOutcome::Error, _) => (OutcomeKind::Error, watching.error.take()),
            (_, Decision::NoSwitch { reason, .. }) => {
                (OutcomeKind::NoSwitch, Some(reason.as_str().to_owned()))
            }
            (_, Decision::Switch { .. }) => (OutcomeKind::Switch, None),
        };
        self.last_outcome = Some(LastOutcome { kind, reason, at });
        let unmanaged = outcome != TickOutcome::Error
            && matches!(
                &decision,
                Decision::NoSwitch {
                    reason: NoSwitchReason::UnmanagedActiveAccount
                        | NoSwitchReason::NoActiveAccount,
                    ..
                }
            );
        if unmanaged {
            match self.collect_round(cx, false) {
                Ok(()) => {}
                Err(e) if e.signal().is_some() => return None,
                Err(e) => self.failed(cx, &e),
            }
        }
        let now = self.now_s();
        let next_poll = self.auto.as_ref().and_then(|a| a.active_next_poll_at());
        let delay = next_delay(&decision, &self.cfg, now, next_poll, (cx.jitter)());
        if announces_sleep(delay, &self.cfg) {
            cx.sink.emit(&DaemonEvent::Auto(AutoEvent::Sleep {
                provider: self.provider.clone(),
                seconds: delay as f64,
                until: now + delay,
            }));
        }
        Some(Worked {
            delay_ms: delay * 1000,
            switched: outcome == TickOutcome::Switched,
        })
    }

    /// D4.3 `collect`: one round, then `interval × U(0.9, 1.1)` shortened to the earliest
    /// planned poll and floored at 60 s. `None`: a signal interrupted it (§14.1).
    fn collect(&mut self, cx: &mut JobCx<'_>) -> Option<Worked> {
        match self.collect_round(cx, true) {
            Ok(()) => {
                self.last_outcome = Some(LastOutcome {
                    kind: OutcomeKind::Collect,
                    reason: None,
                    at: self.now_s(),
                });
            }
            Err(e) if e.signal().is_some() => return None,
            Err(e) => self.failed(cx, &e),
        }
        // A store that cannot be read plans nothing, and the floor applies.
        let earliest = self.earliest_next_poll().unwrap_or(None);
        let delay = collect_delay_s(self.cfg.interval_s, self.now_s(), earliest, (cx.jitter)());
        Some(Worked {
            delay_ms: delay * 1000,
            switched: false,
        })
    }

    /// The provider's accounts as the store holds them now; none without a store.
    fn rows(&self) -> Result<Vec<AccountRow>, EngineError> {
        Ok(match self.engine.existing_store()? {
            Some(s) => s.accounts(&self.provider)?,
            None => Vec::new(),
        })
    }

    /// `collect`'s round: `collect_usage` as an auto tick schedules it (§8.6), with the
    /// provider's threshold and models. Its warnings are `error` events that do not fail the
    /// run (§8.3), and a round that recorded or failed something is a `usage` event (D5.5).
    /// `report` is false after an `auto` tick: the next tick's step 1 reports what this round
    /// quarantined, so nothing is reported twice (Decision 6). Releasing is never this
    /// round's (Decision 10).
    fn collect_round(&mut self, cx: &JobCx<'_>, report: bool) -> Result<(), EngineError> {
        let sink = AutoEvents(cx.sink);
        if report {
            let rows = self.rows()?;
            self.quarantines.report(&self.provider, &rows, &[], &sink);
        }
        let collected = self.engine.collect_usage(CollectMode::Scheduled {
            provider: self.provider.clone(),
            threshold: self.cfg.threshold,
            models: self.cfg.models.clone(),
        })?;
        let rows = self.rows()?;
        if report {
            for id in &collected.quarantined {
                if let Some(row) = rows.iter().find(|r| &r.id == id) {
                    self.quarantines
                        .note_quarantined(&self.provider, row, &sink);
                }
            }
        }
        for warning in &collected.warnings {
            sink.emit(&AutoEvent::Error {
                provider: self.provider.clone(),
                message: warning.clone(),
                transient: true,
            });
        }
        let position = |id: &AccountId| rows.iter().find(|r| &r.id == id).map(|r| r.position);
        let mut updated = Vec::new();
        let mut fetch_errors = BTreeMap::new();
        for (id, outcome) in &collected.outcomes {
            let Some(at) = position(id) else {
                continue;
            };
            match outcome {
                Collected::Recorded => updated.push(at),
                Collected::Failed { kind } => {
                    fetch_errors.insert(at, kind.clone());
                }
                Collected::OverBudget { .. } => {
                    fetch_errors.insert(at, "over-budget".to_owned());
                }
                _ => {}
            }
        }
        updated.sort_unstable();
        updated.dedup();
        if !updated.is_empty() || !fetch_errors.is_empty() {
            cx.sink.emit(&DaemonEvent::Usage {
                provider: self.provider.clone(),
                updated,
                fetch_errors,
            });
        }
        Ok(())
    }

    /// The earliest `next_poll_at` among the accounts a round can collect: the switchable
    /// ones that are not quarantined (§8.6).
    fn earliest_next_poll(&self) -> Result<Option<i64>, EngineError> {
        let Some(store) = self.engine.existing_store()? else {
            return Ok(None);
        };
        let mut earliest: Option<i64> = None;
        for row in store.accounts(&self.provider)? {
            if row.disabled || !row.identity_json.is_object() || row.quarantine_reason.is_some() {
                continue;
            }
            if let Some(at) = store.usage_state(&row.id)?.and_then(|s| s.next_poll_at) {
                earliest = Some(earliest.map_or(at, |e| e.min(at)));
            }
        }
        Ok(earliest)
    }

    /// D6.1's `limit` notice is judged here, from the store, after each run that did work: in
    /// `auto` after a tick that did not switch, in `collect` after every round. Notifications
    /// are not wired yet, so it does nothing.
    fn after_run_notices(&mut self, _cx: &mut JobCx<'_>, _switched: bool) {}

    /// Decision 12: this job's line of the status board.
    fn publish(&self) {
        let Some(mode) = self.mode else {
            return;
        };
        let works = matches!(mode, Mode::Auto | Mode::Collect);
        self.ctx.board.set_provider(ProviderStatus {
            provider: self.provider.clone(),
            mode,
            autoswitch: self.settings.daemon_autoswitch,
            waiting_for: self.waiting_for,
            holder_pid: self.holder_pid,
            backoff: None,
            last_tick_at: self.last_tick_at,
            next_tick_at: works.then(|| self.work_due_ms.div_euclid(1000)),
            last_outcome: self.last_outcome.clone(),
            last_switch: self.last_switch.clone(),
        });
    }
}

impl Job for ProviderJob<'_> {
    fn name(&self) -> String {
        format!("{JOB_PREFIX}{}", self.provider)
    }

    /// Decision 8: the first run starts with the warnings of the job's first settings read,
    /// before its mode is evaluated, as a reload's come before its evaluation.
    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
        for message in std::mem::take(&mut self.first_warnings) {
            cx.sink.emit(&DaemonEvent::Auto(AutoEvent::ConfigWarning {
                provider: self.provider.clone(),
                message,
            }));
        }
        let ran = self.step(cx);
        self.publish();
        ran
    }

    /// D4.2, Decision 8: the provider's settings read again, each warning a `config-warning`
    /// event before anything else happens, the engine's configuration replaced, and the mode
    /// evaluated at once. Pausing (`daemon.autoswitch` false) therefore drops the engine lock
    /// here, before the scheduler's next job or sleep. A file that cannot be read gives the
    /// defaults, so switching resumes, and its warning comes first (Review Focus 1). A reload
    /// before the first run supersedes the first read, whose warnings are then never emitted.
    fn settings_changed(&mut self, cx: &mut JobCx<'_>) {
        self.first_warnings.clear();
        let (settings, warnings) = Settings::load(self.engine.env(), &self.provider);
        for message in warnings {
            cx.sink.emit(&DaemonEvent::Auto(AutoEvent::ConfigWarning {
                provider: self.provider.clone(),
                message,
            }));
        }
        self.cfg = auto_config(
            &settings,
            &AutoFlags::default(),
            self.long_window.as_deref(),
        );
        self.settings = settings;
        if let Some(auto) = self.auto.as_mut() {
            auto.set_config(self.cfg.clone());
        }
        if let Err(e) = self.evaluate(cx) {
            // An interruption is the scheduler's, at its next cancellation point.
            if e.signal().is_none() {
                self.failed(cx, &e);
            }
        }
        self.publish();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use serde_json::{Value, json};
    use tagteam_engine::settings::config_path;
    use tagteam_provider::{FlockGuard, Method};

    use super::*;
    use crate::scheduler::Scheduler;
    use crate::testutil::{
        CC_USAGE, Emitted, Fx, Slices, T0, board, cc, fake, job_ctx, status, two,
    };

    /// Claude Code's configuration under the default settings.
    fn cfg() -> AutoConfig {
        auto_config(&Settings::default(), &AutoFlags::default(), Some("7d"))
    }

    /// A usage reply of 12 % and 30 % in Claude Code's two windows (§8.1).
    fn usage_body() -> Value {
        json!({
            "five_hour": {"utilization": 12.0, "resets_at": "2030-01-04T17:20:00.000000+00:00"},
            "seven_day": {"utilization": 30.0, "resets_at": "2030-01-08T00:00:00.000000+00:00"}
        })
    }

    /// `a` at 10 % and the live `b` at `b_long` % of the long window, both read at `T0` and
    /// next due at `T0 + 300`: no tick or round fetches anything for five minutes.
    fn read_two(fx: &Fx, b_long: f64) -> (AccountId, AccountId) {
        let (a, b) = two(fx);
        fx.read(&a, 10.0, 10.0, T0, T0 + 300);
        fx.read(&b, 20.0, b_long, T0, T0 + 300);
        (a, b)
    }

    fn mode_event(provider: ProviderId, mode: Mode) -> DaemonEvent {
        DaemonEvent::Mode {
            provider,
            mode,
            waiting_for: None,
            holder_pid: None,
            backoff: None,
        }
    }

    #[test]
    fn the_mode_table_s_rows_apply_in_order() {
        // D4.3: the rows are tried in order, and the first that applies wins.
        let full = ModeInputs {
            has_store: true,
            switchable: 2,
            autoswitch: true,
            holds_lock: true,
            ..ModeInputs::default()
        };
        let engine_lock = (Mode::Waiting, Some(WaitingFor::EngineLock));
        // A newer schema, before anything else; the store is not even opened.
        let schema = (Mode::Waiting, Some(WaitingFor::Schema));
        assert_eq!(
            mode_for(&ModeInputs {
                schema_newer: true,
                ..full
            }),
            schema
        );
        assert_eq!(
            mode_for(&ModeInputs {
                schema_newer: true,
                ..ModeInputs::default()
            }),
            schema
        );
        // No store, or no switchable account, even with another engine running.
        assert_eq!(
            mode_for(&ModeInputs {
                has_store: false,
                ..full
            }),
            (Mode::Idle, None)
        );
        assert_eq!(
            mode_for(&ModeInputs {
                switchable: 0,
                holder_elsewhere: Some(7),
                holds_lock: false,
                ..full
            }),
            (Mode::Idle, None)
        );
        // Another live engine, paused or not, with one account or two; or a refused try.
        for i in [
            ModeInputs {
                holder_elsewhere: Some(7),
                holds_lock: false,
                ..full
            },
            ModeInputs {
                holder_elsewhere: Some(7),
                holds_lock: false,
                autoswitch: false,
                ..full
            },
            ModeInputs {
                holder_elsewhere: Some(7),
                holds_lock: false,
                switchable: 1,
                ..full
            },
            ModeInputs {
                refused: true,
                holds_lock: false,
                ..full
            },
            ModeInputs {
                holder_elsewhere: Some(7),
                ..full
            },
        ] {
            assert_eq!(mode_for(&i), engine_lock, "{i:?}");
        }
        // Switching on, two switchable accounts, and the lock held.
        assert_eq!(mode_for(&full), (Mode::Auto, None));
        // Otherwise. Entering `auto` is the job's try of the lock, not the table's.
        for i in [
            ModeInputs {
                autoswitch: false,
                holds_lock: false,
                ..full
            },
            ModeInputs {
                autoswitch: false,
                ..full
            },
            ModeInputs {
                switchable: 1,
                holds_lock: false,
                ..full
            },
            ModeInputs {
                holds_lock: false,
                ..full
            },
        ] {
            assert_eq!(mode_for(&i), (Mode::Collect, None), "{i:?}");
        }
    }

    #[test]
    fn a_collect_round_waits_its_interval_shortened_to_the_earliest_poll_and_floored() {
        // D4.3: interval × U(0.9, 1.1), shortened to the earliest `next_poll_at`, at least 60 s.
        assert_eq!(collect_delay_s(300, T0, None, 0.0), 300);
        assert_eq!(collect_delay_s(300, T0, None, 1.0), 330);
        assert_eq!(collect_delay_s(300, T0, None, -1.0), 270);
        assert_eq!(collect_delay_s(300, T0, None, f64::NAN), 300);
        assert_eq!(collect_delay_s(300, T0, Some(T0 + 100), 0.0), 100);
        assert_eq!(collect_delay_s(300, T0, Some(T0 + 10), 0.0), 60);
        assert_eq!(collect_delay_s(300, T0, Some(T0 - 500), 0.0), 60);
        assert_eq!(collect_delay_s(15, T0, None, 1.0), 60);
    }

    #[test]
    fn accounts_added_move_the_provider_from_idle_to_collect_to_auto_and_back() {
        // D4.3 as `add`, a second `add` and a disable change the store: no store is `idle`,
        // one switchable account `collect`, two `auto` with the engine lock, and one again
        // `collect`, the lock let go and its record emptied. A mode that works starts with
        // its work at once.
        let fx = Fx::new();
        fx.http.push_json(Method::Get, CC_USAGE, 200, usage_body());
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        assert_eq!(job.run(&mut cx), Ran::After(60_000));
        let a = fx.add("a@x.co");
        job.run(&mut cx);
        fx.add("b@x.co");
        job.run(&mut cx);
        assert!(
            fx.engine.auto(&cc(), cfg(), false).unwrap().is_none(),
            "the job holds the engine lock"
        );
        fx.engine.store().unwrap().set_disabled(&a, true).unwrap();
        job.run(&mut cx);
        assert_eq!(
            events.modes(&cc()),
            [
                (0, Mode::Idle, None, None),
                (0, Mode::Collect, None, None),
                (0, Mode::Auto, None, None),
                (0, Mode::Collect, None, None),
            ]
        );
        assert!(matches!(fx.engine.engine_holder(&cc()), Read::Absent));
        assert!(fx.engine.auto(&cc(), cfg(), false).unwrap().is_some());
        // The first `collect` round recorded a's first reading.
        let first = DaemonEvent::Usage {
            provider: cc(),
            updated: vec![1],
            fetch_errors: BTreeMap::new(),
        };
        assert!(events.events().iter().any(|(_, e)| *e == first));
    }

    #[test]
    fn a_lock_another_engine_holds_is_waited_for_and_taken_once_it_is_dropped() {
        // D4.3: entering `auto` tries the lock once. Another engine holds it, and its record
        // names a live process (this one's, as a `tagteam auto` would name its own), so the
        // job waits and names that pid. It takes the lock at its next look once it is free.
        let fx = Fx::new();
        read_two(&fx, 50.0);
        let held = fx.engine.auto(&cc(), cfg(), false).unwrap().unwrap();
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        assert_eq!(job.run(&mut cx), Ran::After(60_000));
        let pid = Some(std::process::id());
        assert_eq!(
            events.modes(&cc()),
            [(0, Mode::Waiting, Some(WaitingFor::EngineLock), pid)]
        );
        let s = status(&board, &cc());
        assert_eq!(
            (s.mode, s.waiting_for, s.holder_pid, s.next_tick_at),
            (Mode::Waiting, Some(WaitingFor::EngineLock), pid, None)
        );
        drop(held);
        fx.clock.advance_ms(60_000);
        job.run(&mut cx);
        assert_eq!(events.modes(&cc())[1..], [(60, Mode::Auto, None, None)]);
        assert!(fx.engine.auto(&cc(), cfg(), false).unwrap().is_none());
    }

    #[test]
    fn a_lock_held_without_a_record_is_waited_for_with_no_pid() {
        // A process that has taken the lock but not yet written its record (§11.1): the try
        // fails, so the job waits, and has no pid to name.
        let fx = Fx::new();
        read_two(&fx, 50.0);
        let path = fx.env.data_dir().join("locks/autoswitch-claude-code.lock");
        let guard = FlockGuard::try_lock(&path).unwrap().unwrap();
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        assert_eq!(job.run(&mut cx), Ran::After(60_000));
        assert_eq!(
            events.modes(&cc()),
            [(0, Mode::Waiting, Some(WaitingFor::EngineLock), None)]
        );
        drop(guard);
        fx.clock.advance_ms(60_000);
        job.run(&mut cx);
        assert_eq!(events.modes(&cc())[1..], [(60, Mode::Auto, None, None)]);
    }

    #[test]
    fn a_store_schema_newer_than_this_build_keeps_the_provider_waiting() {
        // D4.1 step 4: every provider job waits for the upgrade watch, and tries no lock.
        let fx = Fx::new();
        read_two(&fx, 50.0);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let ctx = ProviderJobCtx {
            schema_newer: true,
            ..job_ctx(&board)
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), ctx);
        assert_eq!(job.run(&mut cx), Ran::After(60_000));
        assert_eq!(
            events.modes(&cc()),
            [(0, Mode::Waiting, Some(WaitingFor::Schema), None)]
        );
        assert!(
            !fx.env
                .data_dir()
                .join("locks/autoswitch-claude-code.lock")
                .exists()
        );
    }

    #[test]
    fn pausing_releases_the_engine_lock_within_one_slice_and_empties_its_record() {
        // D4.2, D4.3: `daemon.autoswitch` set false while the scheduler sleeps. The reload in
        // the next slice moves the provider to `collect`, which drops the engine: by the slice
        // after the write, the record is empty and a `tagteam auto` could take the lock.
        let fx = Fx::new();
        read_two(&fx, 50.0);
        let board = board();
        let freed = Cell::new(None);
        let cancel = fx.engine.cancel().clone();
        let slices = Slices::new(&fx.clock, |n| match n {
            2 => fx.settings("[daemon]\nautoswitch = false\n", 2),
            3 => {
                let empty = matches!(fx.engine.engine_holder(&cc()), Read::Absent);
                let free = fx.engine.auto(&cc(), cfg(), false).unwrap().is_some();
                freed.set(Some((empty, free)));
            }
            _ if n >= 4 => cancel.request(libc::SIGTERM),
            _ => {}
        });
        let mut scheduler = Scheduler::new();
        scheduler.add(
            Box::new(ProviderJob::new(&fx.engine, cc(), job_ctx(&board))),
            fx.engine.now_ms(),
        );
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        assert_eq!(scheduler.run(&mut cx, &slices), StopReason::Signal);
        assert_eq!(freed.get(), Some((true, true)));
        assert_eq!(
            events.modes(&cc()),
            [(0, Mode::Auto, None, None), (3, Mode::Collect, None, None)]
        );
    }

    #[test]
    fn a_corrupt_settings_file_resumes_switching_with_a_warning() {
        // Review Focus 1. Paused, the provider collects. A `config.toml` that is no longer
        // TOML reads as the defaults (§6.4), so `daemon.autoswitch` is true again and the job
        // goes back to `auto`. The person is told why, before the mode changes.
        let fx = Fx::new();
        read_two(&fx, 50.0);
        fx.settings("[daemon]\nautoswitch = false\n", 1);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        job.run(&mut cx);
        assert!(!status(&board, &cc()).autoswitch);
        fx.settings("[daemon\nautoswitch = false\n", 2);
        job.settings_changed(&mut cx);
        let warning = format!(
            "{}: the settings file is not valid TOML; using the defaults",
            config_path(&fx.env).display()
        );
        let seen: Vec<DaemonEvent> = events.events().into_iter().map(|(_, e)| e).collect();
        assert_eq!(
            seen,
            [
                mode_event(cc(), Mode::Collect),
                DaemonEvent::Auto(AutoEvent::ConfigWarning {
                    provider: cc(),
                    message: warning,
                }),
                mode_event(cc(), Mode::Auto),
            ]
        );
        assert!(status(&board, &cc()).autoswitch);
        assert!(
            fx.engine.auto(&cc(), cfg(), false).unwrap().is_none(),
            "it holds the engine lock again"
        );
    }

    #[test]
    fn a_settings_file_corrupt_when_the_job_is_built_is_warned_at_its_first_run() {
        // Decision 8. The job's first read comes after the command printed its own, and the
        // file may have been corrupted in between: no mtime change will come to report it. Its
        // warning is the first run's first event, before the mode (`auto`: the defaults switch
        // again), and is never repeated.
        let fx = Fx::new();
        read_two(&fx, 50.0);
        fx.settings("[daemon\nautoswitch = false\n", 1);
        let warning = DaemonEvent::Auto(AutoEvent::ConfigWarning {
            provider: cc(),
            message: format!(
                "{}: the settings file is not valid TOML; using the defaults",
                config_path(&fx.env).display()
            ),
        });
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        assert!(events.events().is_empty(), "building the job emits nothing");
        job.run(&mut cx);
        job.run(&mut cx);
        let seen: Vec<DaemonEvent> = events.events().into_iter().map(|(_, e)| e).collect();
        assert_eq!(
            seen[..2],
            [warning.clone(), mode_event(cc(), Mode::Auto)],
            "{seen:?}"
        );
        assert_eq!(
            seen.iter().filter(|e| **e == warning).count(),
            1,
            "{seen:?}"
        );

        // A reload before the first run reads the file again and reports what it finds then.
        // The first read's warning no longer describes the file, so it is never emitted.
        let fx = Fx::new();
        read_two(&fx, 50.0);
        fx.settings("[daemon\nautoswitch = false\n", 1);
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        fx.settings("[daemon]\nautoswitch = false\n", 2);
        job.settings_changed(&mut cx);
        job.run(&mut cx);
        assert_eq!(events.modes(&cc()), [(0, Mode::Collect, None, None)]);
        assert!(
            !events
                .events()
                .iter()
                .any(|(_, e)| matches!(e, DaemonEvent::Auto(AutoEvent::ConfigWarning { .. }))),
            "{:?}",
            events.events()
        );
    }

    #[test]
    fn a_provider_table_pauses_only_its_provider() {
        // Review Focus 5: `provider.fake-agent.daemon.autoswitch false` pauses FakeAgent
        // alone. Claude Code keeps its engine lock and ticks on.
        let fx = Fx::new();
        let (a, b) = two(&fx);
        let (alice, bob) = (fx.add_fake("alice"), fx.add_fake("bob"));
        for (id, long) in [(&a, 10.0), (&b, 50.0), (&alice, 10.0), (&bob, 50.0)] {
            fx.read(id, 10.0, long, T0, T0 + 300);
        }
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut claude = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        let mut agent = ProviderJob::new(&fx.engine, fake(), job_ctx(&board));
        claude.run(&mut cx);
        agent.run(&mut cx);
        fx.settings("[provider.fake-agent.daemon]\nautoswitch = false\n", 1);
        claude.settings_changed(&mut cx);
        agent.settings_changed(&mut cx);
        assert_eq!(events.modes(&cc()), [(0, Mode::Auto, None, None)]);
        assert_eq!(
            events.modes(&fake()),
            [(0, Mode::Auto, None, None), (0, Mode::Collect, None, None)]
        );
        let fake_cfg = auto_config(&Settings::default(), &AutoFlags::default(), None);
        assert!(fx.engine.auto(&fake(), fake_cfg, false).unwrap().is_some());
        assert!(fx.engine.auto(&cc(), cfg(), false).unwrap().is_none());
        let shown: Vec<(ProviderId, Mode, bool)> = board
            .snapshot()
            .providers
            .into_iter()
            .map(|p| (p.provider, p.mode, p.autoswitch))
            .collect();
        assert_eq!(
            shown,
            [(cc(), Mode::Auto, true), (fake(), Mode::Collect, false)]
        );
        fx.clock.advance_ms(60_000);
        claude.run(&mut cx);
        agent.run(&mut cx);
        assert_eq!(events.polls(&cc()), [0, 60]);
        assert_eq!(events.polls(&fake()), [0]);
        assert_eq!(
            status(&board, &fake()).last_outcome.map(|o| o.kind),
            Some(OutcomeKind::Collect)
        );
    }

    #[test]
    fn an_unmanaged_live_login_in_auto_still_has_its_candidates_collected() {
        // D4.3, D1.1 criterion 1: the tick stops at §11.2 step 2, since tagteam does not
        // manage the live login, so it collects nothing itself. The job runs `collect`'s
        // round after it: with no managed live account the round escalates (§8.6) and
        // fetches every due candidate.
        let fx = Fx::new();
        two(&fx);
        fx.login("c@x.co");
        fx.http.push_json(Method::Get, CC_USAGE, 200, usage_body());
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        job.run(&mut cx);
        let seen: Vec<DaemonEvent> = events.events().into_iter().map(|(_, e)| e).collect();
        assert_eq!(
            seen,
            [
                mode_event(cc(), Mode::Auto),
                DaemonEvent::Auto(AutoEvent::NoSwitch {
                    provider: cc(),
                    reason: "unmanaged-active-account".into(),
                    detail: String::new(),
                }),
                DaemonEvent::Usage {
                    provider: cc(),
                    updated: vec![1, 2],
                    fetch_errors: BTreeMap::new(),
                },
            ]
        );
        assert_eq!(fx.http.count(Method::Get, CC_USAGE), 2);
        assert_eq!(
            status(&board, &cc()).last_outcome,
            Some(LastOutcome {
                kind: OutcomeKind::NoSwitch,
                reason: Some("unmanaged-active-account".into()),
                at: T0,
            })
        );
    }

    #[test]
    fn collect_reports_a_quarantine_another_process_set_and_never_tries_the_lock() {
        // D4.3: each round compares the provider's accounts with the round before, as a
        // tick's step 1 does, and reports what another process quarantined or released.
        // Paused, the job never tries the engine lock: its file is never even created.
        let fx = Fx::new();
        let (a, _) = read_two(&fx, 50.0);
        fx.settings("[daemon]\nautoswitch = false\n", 1);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        job.run(&mut cx);
        let store = fx.engine.store().unwrap();
        store
            .set_quarantine(&a, "invalid_grant", "sha256:x", 1)
            .unwrap();
        fx.clock.advance_ms(60_000);
        job.run(&mut cx);
        store
            .clear_quarantine(&a, "credentials-replaced", "cli", (T0 + 60) * 1000)
            .unwrap();
        fx.clock.advance_ms(60_000);
        job.run(&mut cx);
        let reported: Vec<(i64, AutoEvent)> = events
            .events()
            .into_iter()
            .filter_map(|(at, e)| match e {
                DaemonEvent::Auto(e) => Some((at, e)),
                _ => None,
            })
            .collect();
        assert_eq!(
            reported,
            [
                (
                    60,
                    AutoEvent::AccountQuarantined {
                        provider: cc(),
                        number: 1,
                        email: "a@x.co".into(),
                        reason: "invalid_grant".into(),
                    }
                ),
                (
                    120,
                    AutoEvent::AccountUnquarantined {
                        provider: cc(),
                        number: 1,
                        email: "a@x.co".into(),
                        reason: "credentials-replaced".into(),
                    }
                ),
            ]
        );
        assert_eq!(events.modes(&cc()), [(0, Mode::Collect, None, None)]);
        assert!(
            !fx.env
                .data_dir()
                .join("locks/autoswitch-claude-code.lock")
                .exists()
        );
    }

    #[test]
    fn the_status_board_shows_a_tick_that_stayed_and_when_the_next_is_due() {
        // D5.4: `no-switch` with its reason; the next tick on §11.4's delay (60 s here).
        let fx = Fx::new();
        read_two(&fx, 50.0);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        assert_eq!(job.run(&mut cx), Ran::After(60_000));
        assert_eq!(
            status(&board, &cc()),
            ProviderStatus {
                provider: cc(),
                mode: Mode::Auto,
                autoswitch: true,
                waiting_for: None,
                holder_pid: None,
                backoff: None,
                last_tick_at: Some(T0),
                next_tick_at: Some(T0 + 60),
                last_outcome: Some(LastOutcome {
                    kind: OutcomeKind::NoSwitch,
                    reason: Some("below-threshold".into()),
                    at: T0,
                }),
                last_switch: None,
            }
        );
    }

    #[test]
    fn the_status_board_shows_a_switch_and_keeps_it() {
        // D5.4: `switch`, and the switch itself: positions and trigger, as its event said.
        let fx = Fx::new();
        read_two(&fx, 95.0);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        job.run(&mut cx);
        let trigger = events
            .events()
            .into_iter()
            .find_map(|(_, e)| match e {
                DaemonEvent::Auto(AutoEvent::Switch {
                    from: 2,
                    to: 1,
                    trigger,
                    dry_run: false,
                    ..
                }) => Some(trigger),
                _ => None,
            })
            .expect("the tick switched from 2 to 1");
        let s = status(&board, &cc());
        assert_eq!(
            s.last_outcome,
            Some(LastOutcome {
                kind: OutcomeKind::Switch,
                reason: None,
                at: T0,
            })
        );
        assert_eq!(
            s.last_switch,
            Some(LastSwitch {
                at: T0,
                from: 2,
                to: 1,
                trigger: trigger.as_str().to_owned(),
            })
        );
    }
}
