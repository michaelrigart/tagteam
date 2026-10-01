//! §11: auto-switch decisions. Pure: no clock, no I/O.

use std::cmp::Ordering;

use crate::ids::AccountId;
use crate::rank::{binding_window, blocked_until, span};
use crate::usage::{Window, headroom};

/// `autoswitch.strategy` (§6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Best,
    ConsumeFirst,
}

impl Strategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Strategy::Best => "best",
            Strategy::ConsumeFirst => "consume-first",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "best" => Some(Strategy::Best),
            "consume-first" => Some(Strategy::ConsumeFirst),
            _ => None,
        }
    }
}

/// Why an automatic switch moves (§11.2 step 5), spelled as `events.trigger` and the `switch`
/// event spell it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Proactive,
    AtLimit,
    Failover,
    ConsumeFirst,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::Proactive => "proactive",
            Trigger::AtLimit => "at-limit",
            Trigger::Failover => "failover",
            Trigger::ConsumeFirst => "consume-first",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "proactive" => Some(Trigger::Proactive),
            "at-limit" => Some(Trigger::AtLimit),
            "failover" => Some(Trigger::Failover),
            "consume-first" => Some(Trigger::ConsumeFirst),
            _ => None,
        }
    }

    /// `at-limit` and `failover` must move: they bypass the cooldown and every anti-flap gate
    /// (§11.2 steps 6 and 8).
    fn must_move(self) -> bool {
        matches!(self, Trigger::AtLimit | Trigger::Failover)
    }
}

/// The settings one engine decides with (§6.4, flags applied), and its provider's long window.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoConfig {
    pub threshold: f64,
    pub hysteresis_pct: f64,
    pub cooldown_s: i64,
    pub interval_s: i64,
    pub unhealthy_ticks: u32,
    /// As configured.
    pub strategy: Strategy,
    pub include_api_key_accounts: bool,
    /// §8.2 relevance (`autoswitch.models`).
    pub models: Vec<String>,
    /// `Provider::primary_long_window`; `None` runs `best` for a consume-first strategy (§4.5).
    pub long_window: Option<String>,
}

impl AutoConfig {
    /// The strategy that actually runs: `Best` when consume-first has no long window.
    pub fn effective_strategy(&self) -> Strategy {
        match (self.strategy, &self.long_window) {
            (Strategy::ConsumeFirst, None) => Strategy::Best,
            (strategy, _) => strategy,
        }
    }
}

/// One account as a tick sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountSnapshot {
    pub id: AccountId,
    pub position: u32,
    /// A managed-key kind (§7.1).
    pub api_key: bool,
    /// Vault credential and identity, not disabled (§9.3).
    pub switchable: bool,
    pub quarantined: bool,
    /// `false` until M4.
    pub session_owned: bool,
    /// The decision-grade reading's windows (§8.4); `None` when there is none. Headroom,
    /// the binding window and the recovery time come from `usage::headroom`,
    /// `rank::binding_window` and `rank::blocked_until` over `cfg.models`.
    pub windows: Option<Vec<Window>>,
    pub fetched_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Live {
    /// No live login.
    None,
    /// A live login tagteam does not manage.
    Unmanaged,
    /// The live account.
    Managed(AccountId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub now: i64,
    pub live: Live,
    pub accounts: Vec<AccountSnapshot>,
}

/// `autoswitch_state` (§6.1), as read for this tick.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AutoState {
    pub last_switch_at: Option<i64>,
    pub last_switch_from: Option<AccountId>,
    pub last_switch_to: Option<AccountId>,
    pub left_headroom: Option<f64>,
    pub left_recovery_at: Option<i64>,
    pub left_trigger: Option<Trigger>,
    pub unhealthy_ticks: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Initial,
    Rechecked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    NoAction,
    Blocked,
}

/// Every `no-switch` reason (§11.4), kebab-case via `as_str`. `decide` returns the first
/// group; the engine reports the second (§11.1, §11.2 steps 2, 11, 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoSwitchReason {
    UnmanagedActiveAccount,
    NoActiveAccount,
    ActiveApiKey,
    BelowThreshold,
    ActiveUsageUnknown,
    Cooldown,
    NoCandidates,
    NoComparison,
    ResetUnknown,
    AlreadyConsumingSoonest,
    NoQualifyingCandidate,
    StaleUsage,
    AllExhausted,
    // engine-reported
    EngineRunning,
    InterruptedSwitch,
    LiveChanged,
    NoViableTarget,
}

impl NoSwitchReason {
    pub fn as_str(self) -> &'static str {
        match self {
            NoSwitchReason::UnmanagedActiveAccount => "unmanaged-active-account",
            NoSwitchReason::NoActiveAccount => "no-active-account",
            NoSwitchReason::ActiveApiKey => "active-api-key",
            NoSwitchReason::BelowThreshold => "below-threshold",
            NoSwitchReason::ActiveUsageUnknown => "active-usage-unknown",
            NoSwitchReason::Cooldown => "cooldown",
            NoSwitchReason::NoCandidates => "no-candidates",
            NoSwitchReason::NoComparison => "no-comparison",
            NoSwitchReason::ResetUnknown => "reset-unknown",
            NoSwitchReason::AlreadyConsumingSoonest => "already-consuming-soonest",
            NoSwitchReason::NoQualifyingCandidate => "no-qualifying-candidate",
            NoSwitchReason::StaleUsage => "stale-usage",
            NoSwitchReason::AllExhausted => "all-exhausted",
            NoSwitchReason::EngineRunning => "engine-running",
            NoSwitchReason::InterruptedSwitch => "interrupted-switch",
            NoSwitchReason::LiveChanged => "live-changed",
            NoSwitchReason::NoViableTarget => "no-viable-target",
        }
    }

    /// §11.2 and §11.4: what each reason does to `--once`'s exit code, the engine-reported
    /// reasons included. A healthy account below the threshold is never BLOCKED (Appendix B #26).
    pub fn outcome(self) -> Outcome {
        match self {
            NoSwitchReason::NoCandidates
            | NoSwitchReason::NoComparison
            | NoSwitchReason::NoQualifyingCandidate
            | NoSwitchReason::AllExhausted
            | NoSwitchReason::InterruptedSwitch
            | NoSwitchReason::NoViableTarget => Outcome::Blocked,
            NoSwitchReason::UnmanagedActiveAccount
            | NoSwitchReason::NoActiveAccount
            | NoSwitchReason::ActiveApiKey
            | NoSwitchReason::BelowThreshold
            | NoSwitchReason::ActiveUsageUnknown
            | NoSwitchReason::Cooldown
            | NoSwitchReason::ResetUnknown
            | NoSwitchReason::AlreadyConsumingSoonest
            | NoSwitchReason::StaleUsage
            | NoSwitchReason::EngineRunning
            | NoSwitchReason::LiveChanged => Outcome::NoAction,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    NoSwitch {
        reason: NoSwitchReason,
        outcome: Outcome,
        /// `"2/3"` for active-usage-unknown, the time left for cooldown (`4m`), the time until
        /// the earliest recovery for all-exhausted; `""` otherwise.
        detail: String,
        /// all-exhausted: the earliest recovery (`earliestResetAt`).
        earliest_reset: Option<i64>,
    },
    Switch {
        trigger: Trigger,
        /// Ranked OAuth targets, then (at-limit/failover only) API-key candidates in position
        /// order (§11.2 steps 9-10).
        targets: Vec<AccountId>,
        /// Consume-first, `Phase::Initial` only (Decision 2).
        recheck: bool,
    },
}

/// `unhealthy_ticks`: the counter after this tick (§11.2 step 5): 0 on known active headroom,
/// +1 on unknown, unchanged when step 5 is not reached or the active account is quarantined.
#[derive(Debug, Clone, PartialEq)]
pub struct Decided {
    pub decision: Decision,
    pub unhealthy_ticks: u32,
}

/// The departure snapshot a switch records for the account it leaves (§11.2 step 11, §11.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Departure {
    pub left_headroom: Option<f64>,
    pub left_recovery_at: Option<i64>,
    pub left_trigger: Trigger,
}

/// §11.4: an all-exhausted sleep ends this long after the earliest recovery.
const RESET_SLACK_S: i64 = 60;
/// §11.4: the longest sleep toward a known reset.
const MAX_SLEEP_S: i64 = 600;
/// §11.4: the shortest sleep after a BLOCKED outcome with no known reset.
const BLOCKED_SLEEP_S: i64 = 300;
/// §11.4: the active account's poll plan never shortens a sleep below this.
const PLAN_FLOOR_S: i64 = 60;
/// §11.4: `interval × U(0.9, 1.1)`.
const JITTER_FRAC: f64 = 0.1;
/// §11.4: a delay longer than this many intervals earns a `sleep` event.
const SLEEP_EVENT_INTERVALS: f64 = 1.5;

/// One account's figures for this tick, read once from its decision-grade windows over
/// `cfg.models`.
struct Rated<'s> {
    account: &'s AccountSnapshot,
    /// §8.2; `None` when unknown.
    headroom: Option<f64>,
    /// When the binding window resets (§11.2 step 8: the binding window first, then its
    /// reset); `None` when unknown or already past.
    recovery_at: Option<i64>,
    /// When `cfg.long_window` resets (consume-first); `None` when unknown or already past.
    long_reset: Option<i64>,
    /// When every relevant window at its limit has reset (`rank::blocked_until`).
    back_at: Option<i64>,
}

impl<'s> Rated<'s> {
    fn of(account: &'s AccountSnapshot, cfg: &AutoConfig, now: i64) -> Self {
        let windows = account.windows.as_deref();
        let future = |at: Option<i64>| at.filter(|&at| at > now);
        let long = windows
            .zip(cfg.long_window.as_deref())
            .and_then(|(w, key)| w.iter().find(|w| w.key == key));
        Self {
            account,
            headroom: windows
                .and_then(|w| headroom(w, &cfg.models))
                .filter(|h| !h.is_nan()),
            recovery_at: future(
                windows
                    .and_then(|w| binding_window(w, &cfg.models))
                    .and_then(|w| w.resets_at),
            ),
            long_reset: future(long.and_then(|w| w.resets_at)),
            back_at: windows.and_then(|w| blocked_until(w, &cfg.models)),
        }
    }
}

/// §11.2 step 5: the active account's usage (`100 − headroom`) is below the threshold.
fn below_threshold(headroom: f64, threshold: f64) -> bool {
    100.0 - headroom < threshold
}

/// §8.2: headroom ≤ 0 is at the limit.
fn at_limit(headroom: f64) -> bool {
    headroom <= 0.0
}

/// §11.2 step 5: `n` unknown ticks in a row have reached `autoswitch.unhealthy_ticks`.
fn unhealthy_limit_reached(n: u32, cfg: &AutoConfig) -> bool {
    n >= cfg.unhealthy_ticks
}

/// §11.2 step 6: the seconds of cooldown left after the last switch, if any are.
fn cooldown_left(st: &AutoState, cfg: &AutoConfig, now: i64) -> Option<i64> {
    let ends = st.last_switch_at?.saturating_add(cfg.cooldown_s);
    (now < ends).then(|| ends - now)
}

/// §11.2 step 7: switchable, not the active account, not quarantined, not session-owned; an
/// API key only when they are included, and never for `consume-first`.
fn is_candidate(
    a: &AccountSnapshot,
    active: &AccountId,
    cfg: &AutoConfig,
    trigger: Trigger,
) -> bool {
    a.id != *active
        && a.switchable
        && !a.quarantined
        && !a.session_owned
        && (!a.api_key || (cfg.include_api_key_accounts && trigger != Trigger::ConsumeFirst))
}

/// §11.2 step 9: every candidate is known to be at its limit (and there is one).
fn every_candidate_exhausted(oauth: &[Rated]) -> bool {
    !oauth.is_empty() && oauth.iter().all(|c| c.headroom.is_some_and(at_limit))
}

/// Steps 4 and 5's verdict on the active account.
struct Triggered {
    trigger: Trigger,
    unhealthy_ticks: u32,
}

fn no_switch(reason: NoSwitchReason, detail: String, unhealthy_ticks: u32) -> Decided {
    Decided {
        decision: Decision::NoSwitch {
            reason,
            outcome: reason.outcome(),
            detail,
            earliest_reset: None,
        },
        unhealthy_ticks,
    }
}

/// §11.2 steps 4 and 5.
fn triggered(active: &Rated, st: &AutoState, cfg: &AutoConfig) -> Result<Triggered, Decided> {
    let kept = st.unhealthy_ticks;
    let go = |trigger, unhealthy_ticks| {
        Ok(Triggered {
            trigger,
            unhealthy_ticks,
        })
    };
    // Step 4: with API keys included, the tick looks for a way back to OAuth.
    if active.account.api_key {
        if !cfg.include_api_key_accounts {
            return Err(no_switch(NoSwitchReason::ActiveApiKey, String::new(), kept));
        }
        return go(Trigger::Proactive, kept);
    }
    // Neither tagteam nor CC can refresh a quarantined token, whatever its reading says.
    if active.account.quarantined {
        return go(Trigger::Failover, kept);
    }
    match active.headroom {
        Some(h) if at_limit(h) => go(Trigger::AtLimit, 0),
        Some(h) if below_threshold(h, cfg.threshold) => match cfg.effective_strategy() {
            Strategy::Best => Err(no_switch(NoSwitchReason::BelowThreshold, String::new(), 0)),
            Strategy::ConsumeFirst => go(Trigger::ConsumeFirst, 0),
        },
        Some(_) => go(Trigger::Proactive, 0),
        None => {
            let n = kept.saturating_add(1);
            if unhealthy_limit_reached(n, cfg) {
                go(Trigger::Failover, n)
            } else {
                Err(no_switch(
                    NoSwitchReason::ActiveUsageUnknown,
                    format!("{n}/{}", cfg.unhealthy_ticks),
                    n,
                ))
            }
        }
    }
}

/// §11.2 step 8's order alone, without its gates: known headroom above 0, most first, ties to
/// the lower position.
fn rank(oauth: &[Rated]) -> Vec<AccountId> {
    let mut ranked: Vec<&Rated> = oauth
        .iter()
        .filter(|c| c.headroom.is_some_and(|h| !at_limit(h)))
        .collect();
    ranked.sort_by(|a, b| most_headroom(a, b));
    ranked.into_iter().map(|c| c.account.id.clone()).collect()
}

/// Most headroom first, ties to the lower position.
fn most_headroom(a: &Rated, b: &Rated) -> Ordering {
    b.headroom
        .partial_cmp(&a.headroom)
        .unwrap_or(Ordering::Equal)
        .then(a.account.position.cmp(&b.account.position))
}

/// §11.2 step 9: why nothing ranked.
fn nothing_ranked(t: &Triggered, active: &Rated, oauth: &[Rated], now: i64) -> Decided {
    let n = t.unhealthy_ticks;
    if t.trigger == Trigger::ConsumeFirst {
        // The active account is healthy and below the threshold: never BLOCKED.
        let comparable =
            active.long_reset.is_some() && oauth.iter().any(|c| c.long_reset.is_some());
        let reason = if comparable {
            NoSwitchReason::AlreadyConsumingSoonest
        } else {
            NoSwitchReason::ResetUnknown
        };
        return no_switch(reason, String::new(), n);
    }
    if oauth.iter().all(|c| c.headroom.is_none()) {
        return no_switch(NoSwitchReason::NoComparison, String::new(), n);
    }
    if !every_candidate_exhausted(oauth) {
        return no_switch(NoSwitchReason::NoQualifyingCandidate, String::new(), n);
    }
    let earliest = oauth.iter().filter_map(|c| c.back_at).min();
    Decided {
        decision: Decision::NoSwitch {
            reason: NoSwitchReason::AllExhausted,
            outcome: Outcome::Blocked,
            detail: earliest.map_or_else(String::new, |at| span(at - now)),
            earliest_reset: earliest,
        },
        unhealthy_ticks: n,
    }
}

/// §11.2 steps 2 and 4–9 (the engine owns steps 1, 3, 10–12). Pure.
pub fn decide(s: &Snapshot, st: &AutoState, cfg: &AutoConfig, _phase: Phase) -> Decided {
    let kept = st.unhealthy_ticks;
    // Step 2: tagteam never acts on a login it does not manage.
    let active = match &s.live {
        Live::None => return no_switch(NoSwitchReason::NoActiveAccount, String::new(), kept),
        Live::Unmanaged => {
            return no_switch(NoSwitchReason::UnmanagedActiveAccount, String::new(), kept);
        }
        Live::Managed(id) => match s.accounts.iter().find(|a| &a.id == id) {
            Some(a) => Rated::of(a, cfg, s.now),
            None => {
                return no_switch(NoSwitchReason::UnmanagedActiveAccount, String::new(), kept);
            }
        },
    };
    let t = match triggered(&active, st, cfg) {
        Ok(t) => t,
        Err(stop) => return stop,
    };
    if !t.trigger.must_move() {
        if let Some(left) = cooldown_left(st, cfg, s.now) {
            return no_switch(NoSwitchReason::Cooldown, span(left), t.unhealthy_ticks);
        }
    }
    let candidates: Vec<&AccountSnapshot> = s
        .accounts
        .iter()
        .filter(|a| is_candidate(a, &active.account.id, cfg, t.trigger))
        .collect();
    if candidates.is_empty() {
        let reason = match t.trigger {
            Trigger::ConsumeFirst => NoSwitchReason::BelowThreshold,
            _ => NoSwitchReason::NoCandidates,
        };
        return no_switch(reason, String::new(), t.unhealthy_ticks);
    }
    let oauth: Vec<Rated> = candidates
        .iter()
        .filter(|a| !a.api_key)
        .map(|a| Rated::of(a, cfg, s.now))
        .collect();
    let targets = rank(&oauth);
    if targets.is_empty() {
        return nothing_ranked(&t, &active, &oauth, s.now);
    }
    Decided {
        decision: Decision::Switch {
            trigger: t.trigger,
            targets,
            recheck: false,
        },
        unhealthy_ticks: t.unhealthy_ticks,
    }
}

/// The departure snapshot a switch records for `from` (§11.2 step 11, §11.3): its headroom and
/// binding-window recovery as this tick saw them.
pub fn departure(s: &Snapshot, cfg: &AutoConfig, from: &AccountId, trigger: Trigger) -> Departure {
    let left = s
        .accounts
        .iter()
        .find(|a| &a.id == from)
        .map(|a| Rated::of(a, cfg, s.now));
    Departure {
        left_headroom: left.as_ref().and_then(|r| r.headroom),
        left_recovery_at: left.as_ref().and_then(|r| r.recovery_at),
        left_trigger: trigger,
    }
}

/// §11.4 loop delay, seconds. `jitter` in [-1, 1] maps to U(0.9, 1.1).
/// - `all-exhausted` with an `earliest_reset` t: `min(max(t + 60 − now, interval), 600)`;
/// - any other BLOCKED outcome except `no-qualifying-candidate` (normal cadence, §11.2 step 9):
///   `max(interval, 300)`;
/// - everything else (switched, NO_ACTION, error, `no-qualifying-candidate`): the jittered
///   interval, shortened to `active_next_poll_at − now` when sooner, floored at 60.
///
/// The 60 s floor bounds only the poll plan's shortening, so a sleep is never lengthened
/// (Appendix B #27): an interval below 60 s stays as it is.
pub fn next_delay(
    d: &Decision,
    cfg: &AutoConfig,
    now: i64,
    active_next_poll_at: Option<i64>,
    jitter: f64,
) -> i64 {
    let interval = cfg.interval_s.max(1);
    match d {
        Decision::NoSwitch {
            reason: NoSwitchReason::AllExhausted,
            earliest_reset: Some(at),
            ..
        } => at
            .saturating_add(RESET_SLACK_S)
            .saturating_sub(now)
            .max(interval)
            .min(MAX_SLEEP_S),
        Decision::NoSwitch {
            outcome: Outcome::Blocked,
            reason,
            ..
        } if *reason != NoSwitchReason::NoQualifyingCandidate => interval.max(BLOCKED_SLEEP_S),
        _ => {
            let j = if jitter.is_finite() {
                jitter.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            let delay = ((interval as f64) * (1.0 + j * JITTER_FRAC)).round() as i64;
            let delay = delay.max(1);
            match active_next_poll_at {
                Some(at) => delay.min(at.saturating_sub(now).max(PLAN_FLOOR_S)),
                None => delay,
            }
        }
    }
}

/// Whether a delay earns a `sleep` event (> 1.5 × interval).
pub fn announces_sleep(delay_s: i64, cfg: &AutoConfig) -> bool {
    delay_s as f64 > SLEEP_EVENT_INTERVALS * cfg.interval_s as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::WindowKind;
    use NoSwitchReason::*;

    const NOW: i64 = 1_900_000_000;

    fn id(position: u32) -> AccountId {
        AccountId::from_string(format!("acct-{position}"))
    }

    fn position(id: &AccountId) -> u32 {
        id.as_str().trim_start_matches("acct-").parse().unwrap()
    }

    fn window(key: &str, kind: WindowKind, pct: f64, resets_at: Option<i64>) -> Window {
        Window {
            key: key.into(),
            label: key.trim_start_matches("scoped:").into(),
            kind,
            pct,
            resets_at,
            period_s: None,
            detail: None,
        }
    }

    /// An OAuth account read 30 s ago: 5h at `pct5` (resets in an hour), 7d at `pct7` (resets
    /// in a day).
    fn oauth(position: u32, pct5: f64, pct7: f64) -> AccountSnapshot {
        AccountSnapshot {
            id: id(position),
            position,
            api_key: false,
            switchable: true,
            quarantined: false,
            session_owned: false,
            windows: Some(vec![
                window("5h", WindowKind::Short, pct5, Some(NOW + 3_600)),
                window("7d", WindowKind::Long, pct7, Some(NOW + 86_400)),
            ]),
            fetched_at: Some(NOW - 30),
        }
    }

    /// An OAuth account whose usage is `pct`, set by its 7d window.
    fn at(position: u32, pct: f64) -> AccountSnapshot {
        oauth(position, 0.0, pct)
    }

    fn unknown(position: u32) -> AccountSnapshot {
        AccountSnapshot {
            windows: None,
            fetched_at: None,
            ..oauth(position, 0.0, 0.0)
        }
    }

    fn api_key(position: u32) -> AccountSnapshot {
        AccountSnapshot {
            api_key: true,
            ..unknown(position)
        }
    }

    /// `a` with window `key` resetting at `at` instead.
    fn reset(mut a: AccountSnapshot, key: &str, at: Option<i64>) -> AccountSnapshot {
        for w in a.windows.iter_mut().flatten() {
            if w.key == key {
                w.resets_at = at;
            }
        }
        a
    }

    fn snap(live: u32, accounts: Vec<AccountSnapshot>) -> Snapshot {
        Snapshot {
            now: NOW,
            live: Live::Managed(id(live)),
            accounts,
        }
    }

    fn cfg() -> AutoConfig {
        AutoConfig {
            threshold: 90.0,
            hysteresis_pct: 10.0,
            cooldown_s: 300,
            interval_s: 60,
            unhealthy_ticks: 3,
            strategy: Strategy::Best,
            include_api_key_accounts: false,
            models: vec![],
            long_window: Some("7d".into()),
        }
    }

    fn consume_first() -> AutoConfig {
        AutoConfig {
            strategy: Strategy::ConsumeFirst,
            ..cfg()
        }
    }

    fn with_keys() -> AutoConfig {
        AutoConfig {
            include_api_key_accounts: true,
            ..cfg()
        }
    }

    fn switched_at(at: i64) -> AutoState {
        AutoState {
            last_switch_at: Some(at),
            ..AutoState::default()
        }
    }

    fn run(s: &Snapshot, st: &AutoState, cfg: &AutoConfig) -> Decided {
        decide(s, st, cfg, Phase::Initial)
    }

    /// A no-switch's reason, outcome and detail; panics on a switch.
    fn stopped(d: &Decided) -> (NoSwitchReason, Outcome, &str) {
        match &d.decision {
            Decision::NoSwitch {
                reason,
                outcome,
                detail,
                ..
            } => (*reason, *outcome, detail.as_str()),
            other => panic!("expected a no-switch, got {other:?}"),
        }
    }

    /// A switch's trigger and its targets' positions; panics on a no-switch.
    fn switched(d: &Decided) -> (Trigger, Vec<u32>) {
        match &d.decision {
            Decision::Switch {
                trigger, targets, ..
            } => (*trigger, targets.iter().map(position).collect()),
            other => panic!("expected a switch, got {other:?}"),
        }
    }

    #[test]
    fn triggers_spell_their_event_names_and_parse_back() {
        for (t, s) in [
            (Trigger::Proactive, "proactive"),
            (Trigger::AtLimit, "at-limit"),
            (Trigger::Failover, "failover"),
            (Trigger::ConsumeFirst, "consume-first"),
        ] {
            assert_eq!(t.as_str(), s);
            assert_eq!(Trigger::parse(s), Some(t));
        }
        assert_eq!(
            Trigger::parse("manual"),
            None,
            "a manual switch is no auto trigger"
        );
        assert_eq!(Trigger::parse("At-Limit"), None);
    }

    #[test]
    fn every_no_switch_reason_is_spelled_in_kebab_case() {
        let spelled: Vec<&str> = [
            UnmanagedActiveAccount,
            NoActiveAccount,
            ActiveApiKey,
            BelowThreshold,
            ActiveUsageUnknown,
            Cooldown,
            NoCandidates,
            NoComparison,
            ResetUnknown,
            AlreadyConsumingSoonest,
            NoQualifyingCandidate,
            StaleUsage,
            AllExhausted,
            EngineRunning,
            InterruptedSwitch,
            LiveChanged,
            NoViableTarget,
        ]
        .into_iter()
        .map(NoSwitchReason::as_str)
        .collect();
        assert_eq!(
            spelled,
            [
                "unmanaged-active-account",
                "no-active-account",
                "active-api-key",
                "below-threshold",
                "active-usage-unknown",
                "cooldown",
                "no-candidates",
                "no-comparison",
                "reset-unknown",
                "already-consuming-soonest",
                "no-qualifying-candidate",
                "stale-usage",
                "all-exhausted",
                "engine-running",
                "interrupted-switch",
                "live-changed",
                "no-viable-target",
            ]
        );
    }

    #[test]
    fn only_the_reasons_that_cannot_move_are_blocked() {
        for r in [
            NoCandidates,
            NoComparison,
            NoQualifyingCandidate,
            AllExhausted,
            InterruptedSwitch,
            NoViableTarget,
        ] {
            assert_eq!(r.outcome(), Outcome::Blocked, "{r:?}");
        }
        // Appendix B #26: a healthy account below the threshold is never BLOCKED; §11.1 and
        // §11.2 step 11: another engine and a manual switch are no action.
        for r in [
            UnmanagedActiveAccount,
            NoActiveAccount,
            ActiveApiKey,
            BelowThreshold,
            ActiveUsageUnknown,
            Cooldown,
            ResetUnknown,
            AlreadyConsumingSoonest,
            StaleUsage,
            EngineRunning,
            LiveChanged,
        ] {
            assert_eq!(r.outcome(), Outcome::NoAction, "{r:?}");
        }
    }

    #[test]
    fn consume_first_without_a_long_window_runs_best() {
        assert_eq!(cfg().effective_strategy(), Strategy::Best);
        assert_eq!(consume_first().effective_strategy(), Strategy::ConsumeFirst);
        let no_long = AutoConfig {
            long_window: None,
            ..consume_first()
        };
        assert_eq!(no_long.effective_strategy(), Strategy::Best);
        assert_eq!(
            no_long.strategy,
            Strategy::ConsumeFirst,
            "the setting is kept"
        );
        let s = snap(1, vec![at(1, 40.0), at(2, 10.0)]);
        assert_eq!(
            stopped(&run(&s, &AutoState::default(), &no_long)),
            (BelowThreshold, Outcome::NoAction, ""),
            "a healthy account is left alone, as best leaves it"
        );
    }

    #[test]
    fn below_the_threshold_means_usage_strictly_under_it() {
        assert!(below_threshold(10.5, 90.0));
        assert!(
            !below_threshold(10.0, 90.0),
            "90% is at the threshold, not below it"
        );
        assert!(!below_threshold(4.0, 90.0));
        assert!(below_threshold(0.2, 99.9));
    }

    #[test]
    fn at_the_limit_is_headroom_zero_or_less() {
        assert!(at_limit(0.0));
        assert!(at_limit(-4.0));
        assert!(!at_limit(0.01));
    }

    #[test]
    fn a_live_login_tagteam_does_not_manage_is_never_acted_on() {
        // Review Focus 5: `claude /login` with a new account. Whatever the state, every tick
        // stops at step 2 and leaves the counter alone.
        let accounts = vec![at(1, 100.0), at(2, 20.0)];
        let unmanaged = Snapshot {
            now: NOW,
            live: Live::Unmanaged,
            accounts: accounts.clone(),
        };
        for st in [
            AutoState::default(),
            AutoState {
                unhealthy_ticks: 2,
                last_switch_to: Some(id(1)),
                ..switched_at(NOW - 10_000)
            },
        ] {
            let d = run(&unmanaged, &st, &cfg());
            assert_eq!(stopped(&d), (UnmanagedActiveAccount, Outcome::NoAction, ""));
            assert_eq!(d.unhealthy_ticks, st.unhealthy_ticks);
        }
        // A live account the snapshot does not list is not managed either.
        assert_eq!(
            stopped(&run(&snap(9, accounts), &AutoState::default(), &cfg())).0,
            UnmanagedActiveAccount
        );
    }

    #[test]
    fn no_live_login_is_no_active_account() {
        let s = Snapshot {
            now: NOW,
            live: Live::None,
            accounts: vec![at(1, 50.0), at(2, 20.0)],
        };
        assert_eq!(
            stopped(&run(&s, &AutoState::default(), &cfg())),
            (NoActiveAccount, Outcome::NoAction, "")
        );
    }

    #[test]
    fn an_active_api_key_is_left_alone_unless_api_keys_are_included() {
        let s = snap(1, vec![api_key(1), at(2, 10.0)]);
        let st = AutoState {
            unhealthy_ticks: 1,
            ..AutoState::default()
        };
        let d = run(&s, &st, &cfg());
        assert_eq!(stopped(&d), (ActiveApiKey, Outcome::NoAction, ""));
        assert_eq!(d.unhealthy_ticks, 1);
    }

    #[test]
    fn an_included_api_key_looks_for_a_way_back_to_oauth_as_proactive() {
        let s = snap(1, vec![api_key(1), at(2, 10.0)]);
        let st = AutoState {
            unhealthy_ticks: 1,
            ..AutoState::default()
        };
        let d = run(&s, &st, &with_keys());
        assert_eq!(switched(&d), (Trigger::Proactive, vec![2]));
        assert_eq!(d.unhealthy_ticks, 1, "step 5 is not reached");
        let consuming = AutoConfig {
            strategy: Strategy::ConsumeFirst,
            ..with_keys()
        };
        assert_eq!(switched(&run(&s, &st, &consuming)).0, Trigger::Proactive);
        assert_eq!(
            stopped(&run(&s, &switched_at(NOW - 10), &with_keys())).0,
            Cooldown,
            "proactive, so the cooldown holds it"
        );
    }

    #[test]
    fn a_quarantined_active_account_fails_over_at_once_whatever_its_reading() {
        let mut active = at(1, 20.0);
        active.quarantined = true;
        let s = snap(1, vec![active, at(2, 30.0)]);
        let st = AutoState {
            unhealthy_ticks: 2,
            ..switched_at(NOW - 10)
        };
        let d = run(&s, &st, &cfg());
        assert_eq!(
            switched(&d),
            (Trigger::Failover, vec![2]),
            "below the threshold and within the cooldown"
        );
        assert_eq!(d.unhealthy_ticks, 2, "the counter is left alone");
    }

    #[test]
    fn unknown_active_usage_counts_ticks_up_to_failover() {
        let s = snap(1, vec![unknown(1), at(2, 30.0)]);
        let mut st = switched_at(NOW - 10);
        for n in 1..=2 {
            let d = run(&s, &st, &cfg());
            assert_eq!(
                stopped(&d),
                (
                    ActiveUsageUnknown,
                    Outcome::NoAction,
                    format!("{n}/3").as_str()
                )
            );
            assert_eq!(d.unhealthy_ticks, n);
            st.unhealthy_ticks = d.unhealthy_ticks;
        }
        let d = run(&s, &st, &cfg());
        assert_eq!(
            switched(&d),
            (Trigger::Failover, vec![2]),
            "within the cooldown too"
        );
        assert_eq!(d.unhealthy_ticks, 3);
        let one = AutoConfig {
            unhealthy_ticks: 1,
            ..cfg()
        };
        assert_eq!(
            switched(&run(&s, &AutoState::default(), &one)).0,
            Trigger::Failover,
            "at 1 the first unknown tick fails over"
        );
        assert!(unhealthy_limit_reached(3, &cfg()));
        assert!(!unhealthy_limit_reached(2, &cfg()));
    }

    #[test]
    fn known_active_headroom_resets_the_unhealthy_count() {
        let st = AutoState {
            unhealthy_ticks: 2,
            ..AutoState::default()
        };
        for pct in [40.0, 95.0, 100.0] {
            let d = run(&snap(1, vec![at(1, pct), at(2, 10.0)]), &st, &cfg());
            assert_eq!(d.unhealthy_ticks, 0, "{pct}");
        }
    }

    #[test]
    fn the_trigger_follows_the_active_headroom_and_the_strategy() {
        let st = AutoState::default();
        let with = |active| snap(1, vec![active, reset(at(2, 10.0), "7d", Some(NOW + 600))]);
        assert_eq!(
            stopped(&run(&with(at(1, 89.9)), &st, &cfg())),
            (BelowThreshold, Outcome::NoAction, "")
        );
        assert_eq!(
            switched(&run(&with(at(1, 90.0)), &st, &cfg())).0,
            Trigger::Proactive
        );
        assert_eq!(
            switched(&run(&with(at(1, 100.0)), &st, &cfg())).0,
            Trigger::AtLimit
        );
        assert_eq!(
            switched(&run(&with(at(1, 104.0)), &st, &cfg())).0,
            Trigger::AtLimit,
            "above 100 is kept (§8.2)"
        );
        assert_eq!(
            switched(&run(&with(at(1, 40.0)), &st, &consume_first())).0,
            Trigger::ConsumeFirst
        );
        assert_eq!(
            switched(&run(&with(at(1, 95.0)), &st, &consume_first())).0,
            Trigger::Proactive,
            "above the threshold consume-first moves on proactively"
        );
    }

    #[test]
    fn the_cooldown_holds_proactive_and_consume_first_but_never_at_limit_or_failover() {
        let recent = switched_at(NOW - 100);
        let second = |active| snap(1, vec![active, reset(at(2, 10.0), "7d", Some(NOW + 600))]);
        assert_eq!(
            stopped(&run(&second(at(1, 95.0)), &recent, &cfg())),
            (Cooldown, Outcome::NoAction, "3m")
        );
        assert_eq!(
            stopped(&run(&second(at(1, 40.0)), &recent, &consume_first())).0,
            Cooldown
        );
        assert_eq!(
            switched(&run(&second(at(1, 100.0)), &recent, &cfg())).0,
            Trigger::AtLimit
        );
        assert_eq!(
            switched(&run(&second(at(1, 95.0)), &switched_at(NOW - 300), &cfg())).0,
            Trigger::Proactive,
            "over exactly cooldown_seconds after the switch"
        );
        let none = AutoConfig {
            cooldown_s: 0,
            ..cfg()
        };
        assert_eq!(
            switched(&run(&second(at(1, 95.0)), &switched_at(NOW), &none)).0,
            Trigger::Proactive
        );
    }

    #[test]
    fn the_cooldown_reports_the_seconds_it_has_left() {
        assert_eq!(
            cooldown_left(&switched_at(NOW - 100), &cfg(), NOW),
            Some(200)
        );
        assert_eq!(cooldown_left(&switched_at(NOW - 300), &cfg(), NOW), None);
        assert_eq!(cooldown_left(&AutoState::default(), &cfg(), NOW), None);
        assert_eq!(
            cooldown_left(&switched_at(NOW + 50), &cfg(), NOW),
            Some(350),
            "a switch stamped ahead of now (clock skew) keeps the cooldown"
        );
    }

    #[test]
    fn a_candidate_is_switchable_not_active_not_quarantined_and_not_session_owned() {
        let c = cfg();
        let active = id(1);
        assert!(is_candidate(&at(2, 10.0), &active, &c, Trigger::Proactive));
        assert!(!is_candidate(&at(1, 10.0), &active, &c, Trigger::AtLimit));
        let mut off = at(2, 10.0);
        off.switchable = false;
        let mut dead = at(3, 10.0);
        dead.quarantined = true;
        let mut owned = at(4, 10.0);
        owned.session_owned = true;
        for a in [&off, &dead, &owned] {
            assert!(!is_candidate(a, &active, &c, Trigger::Failover), "{a:?}");
        }
        assert!(!is_candidate(&api_key(5), &active, &c, Trigger::AtLimit));
        for t in [Trigger::Proactive, Trigger::AtLimit, Trigger::Failover] {
            assert!(is_candidate(&api_key(5), &active, &with_keys(), t));
        }
        assert!(!is_candidate(
            &api_key(5),
            &active,
            &with_keys(),
            Trigger::ConsumeFirst
        ));
    }

    #[test]
    fn no_candidates_is_blocked_except_for_a_healthy_consume_first_account() {
        let mut off = at(2, 10.0);
        off.switchable = false;
        let mut dead = at(3, 10.0);
        dead.quarantined = true;
        let mut owned = at(4, 10.0);
        owned.session_owned = true;
        let others = vec![off, dead, owned, api_key(5)];
        let st = AutoState::default();
        let mut accounts = vec![at(1, 95.0)];
        accounts.extend(others.clone());
        assert_eq!(
            stopped(&run(&snap(1, accounts), &st, &cfg())),
            (NoCandidates, Outcome::Blocked, "")
        );
        let mut accounts = vec![at(1, 40.0)];
        accounts.extend(others);
        let consuming = AutoConfig {
            include_api_key_accounts: true,
            ..consume_first()
        };
        assert_eq!(
            stopped(&run(&snap(1, accounts), &st, &consuming)),
            (BelowThreshold, Outcome::NoAction, ""),
            "an API key never counts for consume-first"
        );
    }

    #[test]
    fn an_included_api_key_is_a_candidate_but_never_a_proactive_target() {
        let s = snap(1, vec![at(1, 95.0), api_key(2)]);
        assert_eq!(
            stopped(&run(&s, &AutoState::default(), &with_keys())),
            (NoComparison, Outcome::Blocked, "")
        );
    }

    #[test]
    fn with_no_readable_candidate_nothing_compares() {
        let s = snap(1, vec![at(1, 95.0), unknown(2), unknown(3)]);
        assert_eq!(
            stopped(&run(&s, &AutoState::default(), &cfg())),
            (NoComparison, Outcome::Blocked, "")
        );
    }

    #[test]
    fn candidates_not_all_known_to_be_exhausted_are_no_qualifying_candidate() {
        let s = snap(1, vec![at(1, 100.0), unknown(2), at(3, 100.0)]);
        assert_eq!(
            stopped(&run(&s, &AutoState::default(), &cfg())),
            (NoQualifyingCandidate, Outcome::Blocked, "")
        );
        assert!(!every_candidate_exhausted(&[]));
    }

    #[test]
    fn every_candidate_exhausted_is_all_exhausted_with_the_earliest_recovery() {
        // 2 is back once both its windows have reset, in a day; 3 once its 5h has, in 2 h.
        let two = oauth(2, 100.0, 100.0);
        let three = reset(oauth(3, 100.0, 60.0), "5h", Some(NOW + 7_200));
        let d = run(
            &snap(1, vec![at(1, 100.0), two, three]),
            &AutoState::default(),
            &cfg(),
        );
        assert_eq!(
            d.decision,
            Decision::NoSwitch {
                reason: AllExhausted,
                outcome: Outcome::Blocked,
                detail: "2h00m".into(),
                earliest_reset: Some(NOW + 7_200),
            }
        );
        let no_reset = reset(at(2, 100.0), "7d", None);
        let d = run(
            &snap(1, vec![at(1, 95.0), no_reset]),
            &AutoState::default(),
            &cfg(),
        );
        assert_eq!(
            d.decision,
            Decision::NoSwitch {
                reason: AllExhausted,
                outcome: Outcome::Blocked,
                detail: String::new(),
                earliest_reset: None,
            }
        );
    }

    #[test]
    fn consume_first_finding_nothing_is_never_blocked() {
        // Appendix B #26: a healthy below-threshold tick is NO_ACTION.
        let st = AutoState::default();
        let run_cf = |accounts| run(&snap(1, accounts), &st, &consume_first());
        assert_eq!(
            stopped(&run_cf(vec![at(1, 40.0), unknown(2)])),
            (ResetUnknown, Outcome::NoAction, "")
        );
        assert_eq!(
            stopped(&run_cf(vec![at(1, 40.0), at(2, 100.0)])),
            (AlreadyConsumingSoonest, Outcome::NoAction, "")
        );
        assert_eq!(
            stopped(&run_cf(vec![reset(at(1, 40.0), "7d", None), at(2, 100.0)])),
            (ResetUnknown, Outcome::NoAction, ""),
            "the active account's own reset is unknown"
        );
    }

    #[test]
    fn a_switch_lists_its_targets_most_headroom_first() {
        let s = snap(1, vec![at(1, 95.0), at(2, 40.0), at(3, 20.0), at(4, 40.0)]);
        let st = AutoState {
            unhealthy_ticks: 1,
            ..AutoState::default()
        };
        let d = run(&s, &st, &cfg());
        assert_eq!(
            d,
            Decided {
                decision: Decision::Switch {
                    trigger: Trigger::Proactive,
                    targets: vec![id(3), id(2), id(4)],
                    recheck: false,
                },
                unhealthy_ticks: 0,
            }
        );
    }

    #[test]
    fn a_departure_records_the_left_accounts_headroom_and_binding_recovery() {
        // 1's 5h binds (95% over the 7d's 60%), so it recovers when the 5h resets.
        let s = snap(1, vec![oauth(1, 95.0, 60.0), at(2, 10.0)]);
        assert_eq!(
            departure(&s, &cfg(), &id(1), Trigger::Proactive),
            Departure {
                left_headroom: Some(5.0),
                left_recovery_at: Some(NOW + 3_600),
                left_trigger: Trigger::Proactive,
            }
        );
        assert_eq!(
            departure(
                &snap(1, vec![unknown(1)]),
                &cfg(),
                &id(1),
                Trigger::Failover
            ),
            Departure {
                left_headroom: None,
                left_recovery_at: None,
                left_trigger: Trigger::Failover,
            }
        );
        let past = snap(1, vec![reset(oauth(1, 95.0, 60.0), "5h", Some(NOW))]);
        assert_eq!(
            departure(&past, &cfg(), &id(1), Trigger::AtLimit).left_recovery_at,
            None,
            "a reset that is not after now is unknown"
        );
        assert_eq!(
            departure(&s, &cfg(), &id(7), Trigger::AtLimit).left_headroom,
            None
        );
    }

    #[test]
    fn the_figures_count_only_the_relevant_windows() {
        let mut a = oauth(1, 40.0, 60.0);
        a.windows.as_mut().unwrap().extend([
            window("scoped:Fable", WindowKind::Scoped, 99.0, Some(NOW + 500)),
            window("spend", WindowKind::Spend, 100.0, Some(NOW + 100)),
        ]);
        let plain = Rated::of(&a, &cfg(), NOW);
        assert_eq!(plain.headroom, Some(40.0));
        assert_eq!(plain.recovery_at, Some(NOW + 86_400), "the 7d binds");
        assert_eq!(plain.long_reset, Some(NOW + 86_400));
        assert_eq!(plain.back_at, None, "spend never counts");
        let fable = AutoConfig {
            models: vec!["fable".into()],
            ..cfg()
        };
        let scoped = Rated::of(&a, &fable, NOW);
        assert!((scoped.headroom.unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(scoped.recovery_at, Some(NOW + 500));
        let no_long = AutoConfig {
            long_window: None,
            ..cfg()
        };
        assert_eq!(Rated::of(&a, &no_long, NOW).long_reset, None);
    }

    fn no_action(reason: NoSwitchReason) -> Decision {
        Decision::NoSwitch {
            reason,
            outcome: Outcome::NoAction,
            detail: String::new(),
            earliest_reset: None,
        }
    }

    fn blocked(reason: NoSwitchReason, earliest_reset: Option<i64>) -> Decision {
        Decision::NoSwitch {
            reason,
            outcome: Outcome::Blocked,
            detail: String::new(),
            earliest_reset,
        }
    }

    fn a_switch() -> Decision {
        Decision::Switch {
            trigger: Trigger::Proactive,
            targets: vec![id(2)],
            recheck: false,
        }
    }

    #[test]
    fn all_exhausted_sleeps_until_a_minute_after_the_earliest_recovery_at_most_600_s() {
        let c = cfg();
        let exhausted = |at| blocked(AllExhausted, Some(at));
        assert_eq!(next_delay(&exhausted(NOW + 100), &c, NOW, None, 0.0), 160);
        assert_eq!(next_delay(&exhausted(NOW + 7_200), &c, NOW, None, 0.0), 600);
        assert_eq!(
            next_delay(&exhausted(NOW - 500), &c, NOW, None, 0.0),
            60,
            "never shorter than the interval"
        );
        assert_eq!(
            next_delay(&exhausted(NOW + 100), &c, NOW, Some(NOW + 10), 1.0),
            160,
            "no jitter and no poll plan"
        );
        let slow = AutoConfig {
            interval_s: 900,
            ..cfg()
        };
        assert_eq!(
            next_delay(&exhausted(NOW + 100), &slow, NOW, None, 0.0),
            600,
            "the cap beats a longer interval"
        );
    }

    #[test]
    fn other_blocked_outcomes_sleep_at_least_300_s() {
        for reason in [
            NoCandidates,
            NoComparison,
            InterruptedSwitch,
            NoViableTarget,
        ] {
            assert_eq!(
                next_delay(&blocked(reason, None), &cfg(), NOW, Some(NOW + 10), 1.0),
                300,
                "{reason:?}"
            );
        }
        assert_eq!(
            next_delay(&blocked(AllExhausted, None), &cfg(), NOW, None, 0.0),
            300,
            "no known reset"
        );
        let slow = AutoConfig {
            interval_s: 900,
            ..cfg()
        };
        assert_eq!(
            next_delay(&blocked(NoCandidates, None), &slow, NOW, None, 0.0),
            900
        );
    }

    #[test]
    fn everything_else_sleeps_the_jittered_interval() {
        for d in [
            a_switch(),
            no_action(BelowThreshold),
            no_action(LiveChanged),
            blocked(NoQualifyingCandidate, None),
        ] {
            assert_eq!(next_delay(&d, &cfg(), NOW, None, 0.0), 60, "{d:?}");
            assert_eq!(next_delay(&d, &cfg(), NOW, None, 1.0), 66, "{d:?}");
            assert_eq!(next_delay(&d, &cfg(), NOW, None, -1.0), 54, "{d:?}");
        }
        assert_eq!(
            next_delay(&a_switch(), &cfg(), NOW, None, 7.0),
            66,
            "clamped"
        );
        assert_eq!(next_delay(&a_switch(), &cfg(), NOW, None, f64::NAN), 60);
    }

    #[test]
    fn the_poll_plan_shortens_a_sleep_to_no_less_than_60_s_and_never_lengthens_it() {
        let c = AutoConfig {
            interval_s: 300,
            ..cfg()
        };
        let d = no_action(BelowThreshold);
        assert_eq!(next_delay(&d, &c, NOW, Some(NOW + 200), 0.0), 200);
        assert_eq!(next_delay(&d, &c, NOW, Some(NOW + 20), 0.0), 60);
        assert_eq!(
            next_delay(&d, &c, NOW, Some(NOW - 90), 0.0),
            60,
            "an overdue poll"
        );
        assert_eq!(next_delay(&d, &c, NOW, Some(NOW + 500), 0.0), 300);
        // Appendix B #27: the floor bounds the plan's shortening, never the interval.
        let fast = AutoConfig {
            interval_s: 15,
            ..cfg()
        };
        assert_eq!(next_delay(&d, &fast, NOW, Some(NOW + 5), 0.0), 15);
        assert_eq!(next_delay(&d, &fast, NOW, None, 0.0), 15);
    }

    #[test]
    fn a_sleep_longer_than_one_and_a_half_intervals_is_announced() {
        let c = cfg();
        assert!(!announces_sleep(66, &c));
        assert!(!announces_sleep(90, &c));
        assert!(announces_sleep(91, &c));
        assert!(announces_sleep(600, &c));
    }
}
