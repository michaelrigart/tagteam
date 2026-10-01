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
/// §11.2 step 8: a re-checked target's reading is at most this old.
const RECHECK_FRESH_S: i64 = 180;
/// §11.3: recovery by headroom, and dominance's margin, in points.
const RECOVERED_PTS: f64 = 3.0;
/// §11.2 step 8 and §11.3: a recovery this much sooner counts.
const RECOVERY_HYSTERESIS_S: i64 = 300;
/// §11.2 step 8: headroom within this many points of none is spent.
const SPENT_HEADROOM_PCT: f64 = 3.0;
/// §11.2 step 8: a recovery within this horizon makes the recovery axis useful.
const RECOVERY_HORIZON_S: i64 = 14_400;
/// §11.2 step 8's headroom axis and §11.3's dominance: twice the active account's headroom.
const HEADROOM_RATIO: f64 = 2.0;

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

/// §11.2 step 8: a `proactive` or `consume-first` landing is below the threshold, unless every
/// account is above it.
fn landing_ok(headroom: f64, threshold: f64, every_account_above: bool) -> bool {
    every_account_above || below_threshold(headroom, threshold)
}

/// §11.2 step 8 (`best`): the candidate's headroom beats the active account's by at least
/// `hysteresis_pct`.
fn beats_by_hysteresis(headroom: f64, active_h: f64, hysteresis_pct: f64) -> bool {
    headroom - active_h >= hysteresis_pct
}

/// §11.2 step 8: the active account and every candidate whose headroom is known are at or
/// above the threshold. A candidate of unknown headroom is never ranked, so it does not count.
fn every_account_above(active_h: Option<f64>, oauth: &[Rated], threshold: f64) -> bool {
    let above = |h: f64| !below_threshold(h, threshold);
    active_h.is_some_and(above) && oauth.iter().filter_map(|c| c.headroom).all(above)
}

/// §11.2 step 8, every account above the threshold: this pair is judged on the recovery axis
/// when both are spent (headroom within 3 points of none) or either recovers within 4 h.
fn recovery_axis_useful(
    active_h: f64,
    headroom: f64,
    active_at: Option<i64>,
    at: Option<i64>,
    now: i64,
) -> bool {
    let spent = |h: f64| h <= SPENT_HEADROOM_PCT;
    let soon = |at: Option<i64>| at.is_some_and(|at| at - now <= RECOVERY_HORIZON_S);
    (spent(active_h) && spent(headroom)) || soon(active_at) || soon(at)
}

/// `at` is at least 300 s before `than`.
fn sooner_by_hysteresis(at: i64, than: i64) -> bool {
    than.saturating_sub(at) >= RECOVERY_HYSTERESIS_S
}

/// §11.2 step 8's recovery axis: the candidate's binding window recovers at least 300 s before
/// the active account's. A past or unknown recovery sorts last: the candidate's never passes,
/// and every known one is sooner than the active account's.
fn recovers_sooner(at: Option<i64>, active_at: Option<i64>) -> bool {
    match (at, active_at) {
        (Some(at), Some(active_at)) => sooner_by_hysteresis(at, active_at),
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// §11.2 step 8's headroom axis: at least twice the active account's headroom.
fn doubles_headroom(headroom: f64, active_h: f64) -> bool {
    headroom >= HEADROOM_RATIO * active_h
}

/// §11.3 dominance: more than twice the active account's headroom, plus 3.
fn dominates(headroom: f64, active_h: f64) -> bool {
    headroom > HEADROOM_RATIO * active_h + RECOVERED_PTS
}

/// §11.2 step 8 (`consume-first`): the candidate's long window resets strictly sooner than the
/// active account's. An unknown reset on either side never does.
fn resets_sooner(at: Option<i64>, active_at: Option<i64>) -> bool {
    matches!((at, active_at), (Some(at), Some(active_at)) if at < active_at)
}

/// §11.2 step 8: after the re-check, the target's reading is at most 180 s old.
fn fresh_after_recheck(fetched_at: Option<i64>, now: i64) -> bool {
    fetched_at.is_some_and(|at| now.saturating_sub(at) <= RECHECK_FRESH_S)
}

/// §11.3: no departure snapshot to judge a return by. A failover departure needs no headroom:
/// it is judged on the landing and recovery legs.
fn departure_missing(st: &AutoState) -> bool {
    match st.left_trigger {
        None => true,
        Some(Trigger::Failover) => false,
        Some(_) => st.left_headroom.is_none(),
    }
}

/// §11.3: the account a `proactive` or `consume-first` move may not return to while the
/// engine still sits on the account it switched to. A missing departure snapshot lifts the bar.
fn barred<'a>(st: &'a AutoState, active: &AccountId) -> Option<&'a AccountId> {
    if st.last_switch_to.as_ref() != Some(active) || departure_missing(st) {
        return None;
    }
    st.last_switch_from.as_ref()
}

/// §11.3: the barred account has recovered against its departure snapshot. One leg is enough:
/// headroom at least 3 points higher, the binding window recovering at least 300 s sooner, or
/// dominance over the active account. A failover departure is judged on the landing leg (it is
/// below the threshold now) and the recovery leg only. The recovery leg needs both recoveries
/// known.
fn recovered_since_departure(
    left: &Rated,
    st: &AutoState,
    active_h: Option<f64>,
    threshold: f64,
) -> bool {
    let h = left.headroom;
    let recovery = matches!(
        (left.recovery_at, st.left_recovery_at),
        (Some(at), Some(then)) if sooner_by_hysteresis(at, then)
    );
    if st.left_trigger == Some(Trigger::Failover) {
        return recovery || h.is_some_and(|h| below_threshold(h, threshold));
    }
    let headroom = matches!(
        (h, st.left_headroom),
        (Some(h), Some(then)) if h - then >= RECOVERED_PTS
    );
    let dominance = matches!((h, active_h), (Some(h), Some(a)) if dominates(h, a));
    headroom || recovery || dominance
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

/// What step 8 compares every candidate with.
struct Ranking<'a> {
    cfg: &'a AutoConfig,
    st: &'a AutoState,
    trigger: Trigger,
    active: &'a Rated<'a>,
    now: i64,
}

impl Ranking<'_> {
    /// The active account's headroom; an API key counts as 0 (step 4).
    fn active_h(&self) -> Option<f64> {
        if self.active.account.api_key {
            Some(0.0)
        } else {
            self.active.headroom
        }
    }
}

/// §11.2 step 8 and §11.3: the OAuth targets, best first. Unknown headroom and headroom ≤ 0
/// never rank. `at-limit` and `failover` skip every anti-flap gate; the bar is lifted only when
/// the barred ranking is empty and the barred account has recovered, and then the ranking runs
/// again without it.
fn rank(r: &Ranking, oauth: &[Rated]) -> Vec<AccountId> {
    if r.trigger.must_move() {
        let usable = oauth
            .iter()
            .filter(|c| c.headroom.is_some_and(|h| !at_limit(h)));
        return ordered(usable.collect(), most_headroom);
    }
    let bar = barred(r.st, &r.active.account.id);
    let ranked = gated(r, oauth, bar);
    let lifted = |left: &AccountId| {
        oauth.iter().any(|c| {
            &c.account.id == left
                && recovered_since_departure(c, r.st, r.active_h(), r.cfg.threshold)
        })
    };
    match bar {
        Some(left) if ranked.is_empty() && lifted(left) => gated(r, oauth, None),
        _ => ranked,
    }
}

/// Step 8's gates for `proactive` and `consume-first`, with `bar` left out.
fn gated(r: &Ranking, oauth: &[Rated], bar: Option<&AccountId>) -> Vec<AccountId> {
    let Some(active_h) = r.active_h() else {
        return Vec::new();
    };
    // Step 4: a way back from an API key lands below the threshold whatever the others show.
    let all_above =
        !r.active.account.api_key && every_account_above(Some(active_h), oauth, r.cfg.threshold);
    let passing = oauth.iter().filter(|c| {
        let Some(h) = c.headroom else {
            return false;
        };
        if at_limit(h) || bar == Some(&c.account.id) || !landing_ok(h, r.cfg.threshold, all_above) {
            return false;
        }
        match r.trigger {
            Trigger::ConsumeFirst => resets_sooner(c.long_reset, r.active.long_reset),
            _ if all_above => {
                if recovery_axis_useful(active_h, h, r.active.recovery_at, c.recovery_at, r.now) {
                    recovers_sooner(c.recovery_at, r.active.recovery_at)
                } else {
                    doubles_headroom(h, active_h)
                }
            }
            _ => beats_by_hysteresis(h, active_h, r.cfg.hysteresis_pct),
        }
    });
    let order = match r.trigger {
        Trigger::ConsumeFirst => soonest_long_reset,
        _ if all_above => soonest_recovery,
        _ => most_headroom,
    };
    ordered(passing.collect(), order)
}

fn ordered(mut ranked: Vec<&Rated>, order: fn(&Rated, &Rated) -> Ordering) -> Vec<AccountId> {
    ranked.sort_by(|a, b| order(a, b));
    ranked.into_iter().map(|c| c.account.id.clone()).collect()
}

/// Most headroom first, ties to the lower position.
fn most_headroom(a: &Rated, b: &Rated) -> Ordering {
    b.headroom
        .partial_cmp(&a.headroom)
        .unwrap_or(Ordering::Equal)
        .then(a.account.position.cmp(&b.account.position))
}

/// `consume-first`: the soonest long-window reset first, then most headroom.
fn soonest_long_reset(a: &Rated, b: &Rated) -> Ordering {
    a.long_reset
        .cmp(&b.long_reset)
        .then_with(|| most_headroom(a, b))
}

/// Every account above the threshold: the soonest binding-window recovery first, a past or
/// unknown one last, then most headroom.
fn soonest_recovery(a: &Rated, b: &Rated) -> Ordering {
    let key = |c: &Rated| c.recovery_at.map_or((1, 0), |at| (0, at));
    key(a).cmp(&key(b)).then_with(|| most_headroom(a, b))
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
pub fn decide(s: &Snapshot, st: &AutoState, cfg: &AutoConfig, phase: Phase) -> Decided {
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
    let ranking = Ranking {
        cfg,
        st,
        trigger: t.trigger,
        active: &active,
        now: s.now,
    };
    let mut targets = rank(&ranking, &oauth);
    // Steps 9 and 10: at-limit and failover fall back to the API keys, in position order.
    if t.trigger.must_move() {
        let mut keys: Vec<&AccountSnapshot> =
            candidates.iter().copied().filter(|a| a.api_key).collect();
        keys.sort_by_key(|a| a.position);
        targets.extend(keys.into_iter().map(|a| a.id.clone()));
    }
    if targets.is_empty() {
        return nothing_ranked(&t, &active, &oauth, s.now);
    }
    // Decision 2: after its re-check, a consume-first switch tries only freshly read targets. A
    // stale first target is `stale-usage`; a stale later one is dropped. A trigger the re-check
    // turned into another one moves as it would have without it.
    if phase == Phase::Rechecked && t.trigger == Trigger::ConsumeFirst {
        let stale = |id: &AccountId| {
            oauth
                .iter()
                .any(|c| &c.account.id == id && !fresh_after_recheck(c.account.fetched_at, s.now))
        };
        if stale(&targets[0]) {
            return no_switch(NoSwitchReason::StaleUsage, String::new(), t.unhealthy_ticks);
        }
        targets.retain(|id| !stale(id));
    }
    Decided {
        decision: Decision::Switch {
            trigger: t.trigger,
            targets,
            recheck: phase == Phase::Initial && t.trigger == Trigger::ConsumeFirst,
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

/// §11.4 `--once`: `0` switched, `2` no action, `3` blocked. A tick that failed has no
/// decision; it exits `1`.
pub fn once_exit_code(d: &Decision) -> i32 {
    match d {
        Decision::Switch { .. } => 0,
        Decision::NoSwitch {
            outcome: Outcome::NoAction,
            ..
        } => 2,
        Decision::NoSwitch {
            outcome: Outcome::Blocked,
            ..
        } => 3,
    }
}

/// §11.1: `--once` with several providers exits with the most severe code, `1` (error) over `0`
/// (switched) over `3` (blocked) over `2` (no action). Any other code outranks all four; no
/// code at all is no action.
pub fn most_severe(codes: &[i32]) -> i32 {
    let severity = |code: i32| match code {
        2 => 0,
        3 => 1,
        0 => 2,
        1 => 3,
        _ => 4,
    };
    codes
        .iter()
        .copied()
        .max_by_key(|&code| severity(code))
        .unwrap_or(2)
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

    /// The engine switched `from` → `to` an hour ago (past the cooldown), leaving `from` with
    /// this departure snapshot.
    fn left(
        from: u32,
        to: u32,
        trigger: Option<Trigger>,
        headroom: Option<f64>,
        recovery_at: Option<i64>,
    ) -> AutoState {
        AutoState {
            last_switch_at: Some(NOW - 3_600),
            last_switch_from: Some(id(from)),
            last_switch_to: Some(id(to)),
            left_headroom: headroom,
            left_recovery_at: recovery_at,
            left_trigger: trigger,
            unhealthy_ticks: 0,
        }
    }

    /// `cfg()` with a 5-point hysteresis, so the bar, not the hysteresis, decides.
    fn loose() -> AutoConfig {
        AutoConfig {
            hysteresis_pct: 5.0,
            ..cfg()
        }
    }

    fn rated(a: &AccountSnapshot) -> Rated<'_> {
        Rated::of(a, &cfg(), NOW)
    }

    #[test]
    fn a_landing_is_below_the_threshold_unless_every_account_is_above_it() {
        assert!(landing_ok(11.0, 90.0, false));
        assert!(!landing_ok(10.0, 90.0, false));
        assert!(landing_ok(10.0, 90.0, true));
        assert!(landing_ok(1.0, 90.0, true));
    }

    #[test]
    fn hysteresis_is_the_candidate_minus_the_active_account_at_least_the_setting() {
        assert!(beats_by_hysteresis(15.0, 5.0, 10.0));
        assert!(!beats_by_hysteresis(14.9, 5.0, 10.0));
        assert!(
            beats_by_hysteresis(5.0, 5.0, 0.0),
            "a zero hysteresis lets a tie through"
        );
    }

    #[test]
    fn every_account_above_counts_the_active_account_and_every_known_candidate() {
        let (a, b, low, u) = (at(2, 92.0), at(3, 100.0), at(4, 80.0), unknown(5));
        assert!(every_account_above(
            Some(5.0),
            &[rated(&a), rated(&b), rated(&u)],
            90.0
        ));
        assert!(!every_account_above(
            Some(5.0),
            &[rated(&a), rated(&low)],
            90.0
        ));
        assert!(!every_account_above(Some(20.0), &[rated(&a)], 90.0));
        assert!(!every_account_above(None, &[rated(&a)], 90.0));
    }

    #[test]
    fn the_recovery_axis_is_useful_when_both_are_spent_or_either_recovers_within_4_h() {
        let far = Some(NOW + 86_400);
        assert!(recovery_axis_useful(3.0, 2.0, far, far, NOW));
        assert!(!recovery_axis_useful(3.1, 2.0, far, far, NOW));
        assert!(!recovery_axis_useful(2.0, 3.1, far, far, NOW));
        assert!(recovery_axis_useful(8.0, 9.0, Some(NOW + 14_400), far, NOW));
        assert!(recovery_axis_useful(8.0, 9.0, far, Some(NOW + 14_400), NOW));
        assert!(!recovery_axis_useful(
            8.0,
            9.0,
            Some(NOW + 14_401),
            None,
            NOW
        ));
    }

    #[test]
    fn the_recovery_axis_needs_a_recovery_300_s_sooner_and_unknown_sorts_last() {
        assert!(recovers_sooner(Some(NOW + 100), Some(NOW + 400)));
        assert!(!recovers_sooner(Some(NOW + 101), Some(NOW + 400)));
        assert!(recovers_sooner(Some(NOW + 100), None));
        assert!(!recovers_sooner(None, Some(NOW + 400)));
        assert!(!recovers_sooner(None, None));
    }

    #[test]
    fn the_headroom_axis_needs_twice_the_active_headroom_and_dominance_three_more() {
        assert!(doubles_headroom(8.0, 4.0));
        assert!(!doubles_headroom(7.9, 4.0));
        assert!(dominates(11.1, 4.0));
        assert!(!dominates(11.0, 4.0), "more than 2 × 4 + 3");
    }

    #[test]
    fn consume_first_needs_a_strictly_sooner_known_long_reset() {
        assert!(resets_sooner(Some(NOW + 10), Some(NOW + 11)));
        assert!(!resets_sooner(Some(NOW + 11), Some(NOW + 11)));
        assert!(!resets_sooner(None, Some(NOW + 11)));
        assert!(!resets_sooner(Some(NOW + 10), None));
    }

    #[test]
    fn a_rechecked_reading_is_fresh_for_180_s() {
        assert!(fresh_after_recheck(Some(NOW - 180), NOW));
        assert!(!fresh_after_recheck(Some(NOW - 181), NOW));
        assert!(fresh_after_recheck(Some(NOW + 30), NOW));
        assert!(!fresh_after_recheck(None, NOW));
    }

    #[test]
    fn best_skips_unknown_exhausted_and_short_of_hysteresis_candidates() {
        let s = snap(
            1,
            vec![
                at(1, 95.0),
                unknown(2),
                at(3, 100.0),
                at(4, 86.0),
                at(5, 85.0),
                at(6, 70.0),
                at(7, 70.0),
            ],
        );
        let d = run(&s, &AutoState::default(), &cfg());
        assert_eq!(switched(&d), (Trigger::Proactive, vec![6, 7, 5]));
    }

    #[test]
    fn a_proactive_landing_is_below_the_threshold_while_any_account_is() {
        let none = AutoConfig {
            hysteresis_pct: 0.0,
            ..cfg()
        };
        let s = snap(1, vec![at(1, 96.0), at(2, 92.0), at(3, 89.0)]);
        assert_eq!(
            switched(&run(&s, &AutoState::default(), &none)),
            (Trigger::Proactive, vec![3])
        );
    }

    #[test]
    fn with_every_account_above_spent_accounts_move_only_for_a_recovery_300_s_sooner() {
        let st = AutoState::default();
        let s = |reset_at| {
            snap(
                1,
                vec![at(1, 98.0), reset(at(2, 97.5), "7d", Some(reset_at))],
            )
        };
        assert_eq!(
            switched(&run(&s(NOW + 86_100), &st, &cfg())),
            (Trigger::Proactive, vec![2]),
            "no hysteresis on this axis"
        );
        assert_eq!(
            stopped(&run(&s(NOW + 86_101), &st, &cfg())),
            (NoQualifyingCandidate, Outcome::Blocked, "")
        );
    }

    #[test]
    fn with_every_account_above_and_recoveries_far_off_a_candidate_needs_twice_the_headroom() {
        let st = AutoState::default();
        let s = |pct| snap(1, vec![at(1, 96.0), at(2, pct)]);
        assert_eq!(
            switched(&run(&s(92.0), &st, &cfg())),
            (Trigger::Proactive, vec![2])
        );
        assert_eq!(
            stopped(&run(&s(92.1), &st, &cfg())).0,
            NoQualifyingCandidate
        );
    }

    #[test]
    fn the_binding_window_is_selected_before_its_reset() {
        // 2's 5h (91%) resets within 4 h, but its 7d (93%) binds and resets in a day: the
        // headroom axis judges it, and 7 is not twice 4.
        let s = snap(1, vec![at(1, 96.0), oauth(2, 91.0, 93.0)]);
        assert_eq!(
            stopped(&run(&s, &AutoState::default(), &cfg())).0,
            NoQualifyingCandidate
        );
    }

    #[test]
    fn with_every_account_above_the_soonest_recovery_ranks_first_and_an_unknown_one_last() {
        let s = snap(
            1,
            vec![
                at(1, 98.0),
                reset(at(2, 97.0), "7d", Some(NOW + 50_000)),
                reset(at(3, 98.0), "7d", Some(NOW + 40_000)),
                reset(at(4, 97.5), "7d", Some(NOW + 40_000)),
                reset(at(5, 91.0), "7d", None),
            ],
        );
        assert_eq!(
            switched(&run(&s, &AutoState::default(), &cfg())),
            (Trigger::Proactive, vec![4, 3, 2, 5])
        );
    }

    #[test]
    fn consume_first_moves_only_to_a_sooner_long_reset_soonest_first() {
        let s = snap(
            1,
            vec![
                at(1, 40.0),
                reset(at(2, 10.0), "7d", Some(NOW + 40_000)),
                reset(at(3, 30.0), "7d", Some(NOW + 20_000)),
                reset(at(4, 50.0), "7d", Some(NOW + 20_000)),
                at(5, 10.0),
                reset(at(6, 10.0), "7d", None),
                reset(at(7, 95.0), "7d", Some(NOW + 1_000)),
            ],
        );
        let d = run(&s, &AutoState::default(), &consume_first());
        assert_eq!(
            d.decision,
            Decision::Switch {
                trigger: Trigger::ConsumeFirst,
                targets: vec![id(3), id(4), id(2)],
                recheck: true,
            },
            "no hysteresis; an equal, unknown or above-threshold reset is skipped"
        );
    }

    #[test]
    fn at_limit_and_failover_skip_every_anti_flap_gate() {
        let bar = left(2, 1, Some(Trigger::Proactive), Some(5.0), None);
        let s = snap(1, vec![at(1, 100.0), at(2, 95.0), at(3, 99.0), unknown(4)]);
        assert_eq!(
            switched(&run(&s, &bar, &cfg())),
            (Trigger::AtLimit, vec![2, 3])
        );
        let mut dead = at(1, 50.0);
        dead.quarantined = true;
        let s = snap(1, vec![dead, at(2, 95.0), at(3, 99.0), unknown(4)]);
        assert_eq!(
            switched(&run(&s, &bar, &cfg())),
            (Trigger::Failover, vec![2, 3])
        );
    }

    #[test]
    fn the_no_return_bar_holds_a_proactive_return_until_the_left_account_recovers() {
        // The engine left 2 at 90% for 1. 1 is at 95% now.
        let bar = left(
            2,
            1,
            Some(Trigger::Proactive),
            Some(10.0),
            Some(NOW + 86_400),
        );
        let s = |pct| snap(1, vec![at(1, 95.0), at(2, pct)]);
        assert_eq!(
            stopped(&run(&s(87.1), &bar, &loose())),
            (NoQualifyingCandidate, Outcome::Blocked, ""),
            "2.9 points better is not recovered"
        );
        assert_eq!(
            switched(&run(&s(87.0), &bar, &loose())),
            (Trigger::Proactive, vec![2]),
            "3 points better is"
        );
    }

    #[test]
    fn dominance_over_the_active_account_lifts_the_bar() {
        // 1 is at 99%: more than 2 × 1 + 3 = 5 points dominates. Every account is above the
        // threshold, so the headroom axis then judges the return.
        let bar = left(
            2,
            1,
            Some(Trigger::Proactive),
            Some(10.0),
            Some(NOW + 86_400),
        );
        let s = |pct| snap(1, vec![at(1, 99.0), at(2, pct)]);
        assert_eq!(
            switched(&run(&s(94.0), &bar, &loose())),
            (Trigger::Proactive, vec![2])
        );
        assert_eq!(
            stopped(&run(&s(95.1), &bar, &loose())).0,
            NoQualifyingCandidate
        );
    }

    #[test]
    fn a_binding_recovery_300_s_sooner_lifts_the_bar() {
        let s = snap(1, vec![at(1, 95.0), at(2, 88.0)]);
        let bar = |then| left(2, 1, Some(Trigger::Proactive), Some(10.0), then);
        assert_eq!(
            switched(&run(&s, &bar(Some(NOW + 86_700)), &loose())).1,
            vec![2]
        );
        assert_eq!(
            stopped(&run(&s, &bar(Some(NOW + 86_699)), &loose())).0,
            NoQualifyingCandidate
        );
        assert_eq!(
            stopped(&run(&s, &bar(None), &loose())).0,
            NoQualifyingCandidate,
            "the recovery leg needs both recoveries known"
        );
    }

    #[test]
    fn the_bar_lifts_only_when_the_barred_ranking_is_empty() {
        let bar = left(
            2,
            1,
            Some(Trigger::Proactive),
            Some(10.0),
            Some(NOW + 86_400),
        );
        let s = snap(1, vec![at(1, 95.0), at(2, 70.0), at(3, 80.0)]);
        assert_eq!(
            switched(&run(&s, &bar, &loose())),
            (Trigger::Proactive, vec![3]),
            "2 has recovered and has more room, but 3 qualifies"
        );
    }

    #[test]
    fn the_bar_holds_only_while_the_engine_sits_where_it_switched_to() {
        // You switched to 3 by hand: 2 is no longer barred.
        let bar = left(
            2,
            1,
            Some(Trigger::Proactive),
            Some(10.0),
            Some(NOW + 86_400),
        );
        let s = snap(3, vec![at(1, 50.0), at(2, 87.1), at(3, 95.0)]);
        assert_eq!(
            switched(&run(&s, &bar, &loose())),
            (Trigger::Proactive, vec![1, 2])
        );
    }

    #[test]
    fn a_missing_departure_snapshot_lifts_the_bar() {
        let s = snap(1, vec![at(1, 95.0), at(2, 87.1), at(3, 88.0)]);
        for bar in [
            left(2, 1, None, Some(10.0), Some(NOW + 86_400)),
            left(2, 1, Some(Trigger::Proactive), None, Some(NOW + 86_400)),
        ] {
            assert_eq!(
                switched(&run(&s, &bar, &loose())),
                (Trigger::Proactive, vec![2, 3]),
                "{bar:?}"
            );
        }
    }

    #[test]
    fn a_failover_departure_is_judged_on_the_landing_and_recovery_legs() {
        let bar = |headroom, then| left(2, 1, Some(Trigger::Failover), headroom, then);
        let below = snap(1, vec![at(1, 95.0), at(2, 87.1)]);
        assert_eq!(
            switched(&run(&below, &bar(None, None), &loose())).1,
            vec![2],
            "the landing leg: 2 is below the threshold now"
        );
        // 2 at 94% dominates 1 at 99% and is 5 points above a remembered 99%, but neither leg
        // counts for a failover departure.
        let above = snap(1, vec![at(1, 99.0), at(2, 94.0)]);
        assert_eq!(
            stopped(&run(&above, &bar(Some(1.0), None), &loose())).0,
            NoQualifyingCandidate
        );
        assert_eq!(
            switched(&run(&above, &bar(None, Some(NOW + 86_700)), &loose())).1,
            vec![2],
            "the recovery leg"
        );
    }

    #[test]
    fn the_bar_holds_a_consume_first_return_too() {
        let bar = left(
            2,
            1,
            Some(Trigger::ConsumeFirst),
            Some(68.0),
            Some(NOW + 20_000),
        );
        let s = |pct| {
            snap(
                1,
                vec![at(1, 40.0), reset(at(2, pct), "7d", Some(NOW + 20_000))],
            )
        };
        assert_eq!(
            stopped(&run(&s(30.0), &bar, &consume_first())),
            (AlreadyConsumingSoonest, Outcome::NoAction, "")
        );
        assert_eq!(
            switched(&run(&s(29.0), &bar, &consume_first())),
            (Trigger::ConsumeFirst, vec![2])
        );
    }

    #[test]
    fn at_limit_or_failover_with_no_oauth_target_falls_back_to_api_keys_in_position_order() {
        let s = snap(
            1,
            vec![
                at(1, 100.0),
                unknown(2),
                api_key(5),
                api_key(3),
                at(4, 100.0),
            ],
        );
        assert_eq!(
            switched(&run(&s, &AutoState::default(), &with_keys())),
            (Trigger::AtLimit, vec![3, 5])
        );
        let mut dead = at(1, 40.0);
        dead.quarantined = true;
        let s = snap(1, vec![dead, api_key(3), unknown(2)]);
        assert_eq!(
            switched(&run(&s, &AutoState::default(), &with_keys())),
            (Trigger::Failover, vec![3])
        );
    }

    #[test]
    fn api_keys_follow_every_oauth_target_and_never_a_proactive_one() {
        let s = |active| snap(1, vec![active, api_key(3), at(2, 50.0)]);
        assert_eq!(
            switched(&run(&s(at(1, 100.0)), &AutoState::default(), &with_keys())),
            (Trigger::AtLimit, vec![2, 3])
        );
        assert_eq!(
            switched(&run(&s(at(1, 95.0)), &AutoState::default(), &with_keys())),
            (Trigger::Proactive, vec![2])
        );
    }

    #[test]
    fn a_way_back_from_an_api_key_lands_below_the_threshold_even_when_every_account_is_above() {
        let s = snap(1, vec![api_key(1), at(2, 95.0), at(3, 92.0)]);
        assert_eq!(
            stopped(&run(&s, &AutoState::default(), &with_keys())).0,
            NoQualifyingCandidate
        );
        let s = snap(1, vec![api_key(1), at(2, 95.0), at(3, 92.0), at(4, 80.0)]);
        assert_eq!(
            switched(&run(&s, &AutoState::default(), &with_keys())),
            (Trigger::Proactive, vec![4])
        );
    }

    #[test]
    fn consume_first_asks_for_a_recheck_only_in_the_initial_phase() {
        let s = snap(
            1,
            vec![at(1, 40.0), reset(at(2, 10.0), "7d", Some(NOW + 600))],
        );
        let st = AutoState::default();
        let recheck = |d: Decided| match d.decision {
            Decision::Switch { recheck, .. } => recheck,
            other => panic!("{other:?}"),
        };
        assert!(recheck(decide(&s, &st, &consume_first(), Phase::Initial)));
        assert!(!recheck(decide(
            &s,
            &st,
            &consume_first(),
            Phase::Rechecked
        )));
        let proactive = snap(1, vec![at(1, 95.0), at(2, 10.0)]);
        assert!(!recheck(decide(&proactive, &st, &cfg(), Phase::Initial)));
    }

    #[test]
    fn after_a_recheck_a_consume_first_target_must_have_been_read_within_180_s() {
        let read = |a: AccountSnapshot, at| AccountSnapshot {
            fetched_at: Some(at),
            ..a
        };
        let s = |first_read| {
            snap(
                1,
                vec![
                    at(1, 40.0),
                    read(reset(at(2, 10.0), "7d", Some(NOW + 600)), first_read),
                    reset(at(3, 10.0), "7d", Some(NOW + 900)),
                ],
            )
        };
        let st = AutoState::default();
        let d = decide(&s(NOW - 180), &st, &consume_first(), Phase::Rechecked);
        assert_eq!(switched(&d), (Trigger::ConsumeFirst, vec![2, 3]));
        let d = decide(&s(NOW - 181), &st, &consume_first(), Phase::Rechecked);
        assert_eq!(stopped(&d), (StaleUsage, Outcome::NoAction, ""));
        assert_eq!(d.unhealthy_ticks, 0);
        assert_eq!(
            switched(&decide(
                &s(NOW - 181),
                &st,
                &consume_first(),
                Phase::Initial
            ))
            .0,
            Trigger::ConsumeFirst,
            "the initial phase ranks on stored readings"
        );
        // The re-check found the active account at its limit: at-limit moves at once.
        let stale = |a: AccountSnapshot| read(a, NOW - 4_000);
        let s = snap(1, vec![at(1, 100.0), stale(at(2, 10.0))]);
        assert_eq!(
            switched(&decide(&s, &st, &consume_first(), Phase::Rechecked)),
            (Trigger::AtLimit, vec![2])
        );
    }

    #[test]
    fn after_a_recheck_a_stale_later_consume_first_target_is_dropped() {
        // The re-check could not refresh 3's reading; 2's and 4's were taken within 180 s. A
        // tick that cannot switch to 2 tries 4 next, never 3.
        let stale = |a: AccountSnapshot| AccountSnapshot {
            fetched_at: Some(NOW - 181),
            ..a
        };
        let s = snap(
            1,
            vec![
                at(1, 40.0),
                reset(at(2, 10.0), "7d", Some(NOW + 600)),
                stale(reset(at(3, 10.0), "7d", Some(NOW + 900))),
                reset(at(4, 10.0), "7d", Some(NOW + 1_200)),
            ],
        );
        let st = AutoState::default();
        assert_eq!(
            switched(&decide(&s, &st, &consume_first(), Phase::Rechecked)),
            (Trigger::ConsumeFirst, vec![2, 4])
        );
        assert_eq!(
            switched(&run(&s, &st, &consume_first())).1,
            vec![2, 3, 4],
            "the initial phase ranks on stored readings"
        );
    }

    #[test]
    fn every_account_at_its_limit_for_days_never_flaps_and_sleeps_at_most_600_s() {
        // Review Focus 4: the weekly window is spent everywhere. 2 recovers first, in a day.
        let spent = |p, back| reset(at(p, 100.0), "7d", Some(back));
        let day = 86_400;
        let accounts = vec![
            spent(1, NOW + 2 * day),
            spent(2, NOW + day),
            spent(3, NOW + 3 * day),
        ];
        let st = AutoState::default();
        let mut now = NOW;
        let mut ticks = 0;
        while now < NOW + day {
            let s = Snapshot {
                now,
                live: Live::Managed(id(1)),
                accounts: accounts.clone(),
            };
            let d = run(&s, &st, &cfg());
            assert_eq!(
                d.decision,
                Decision::NoSwitch {
                    reason: AllExhausted,
                    outcome: Outcome::Blocked,
                    detail: span(NOW + day - now),
                    earliest_reset: Some(NOW + day),
                },
                "no switch between exhausted accounts"
            );
            let delay = next_delay(&d.decision, &cfg(), now, None, 0.0);
            assert!((60..=600).contains(&delay), "{delay}");
            now += delay;
            ticks += 1;
        }
        assert!(ticks >= 144, "a day at no more than 600 s a tick: {ticks}");
        // 2's week resets: the next tick moves there at once, and only there.
        let mut back = accounts.clone();
        back[1] = reset(at(2, 0.0), "7d", Some(NOW + 8 * day));
        let s = Snapshot {
            now,
            live: Live::Managed(id(1)),
            accounts: back.clone(),
        };
        assert_eq!(switched(&run(&s, &st, &cfg())), (Trigger::AtLimit, vec![2]));
        // On 2, nothing pulls it back to an exhausted account.
        let s = Snapshot {
            now: now + 60,
            live: Live::Managed(id(2)),
            accounts: back,
        };
        assert_eq!(stopped(&run(&s, &st, &cfg())).0, BelowThreshold);
    }

    #[test]
    fn once_exits_0_on_a_switch_2_on_no_action_and_3_when_blocked() {
        assert_eq!(once_exit_code(&a_switch()), 0);
        for reason in [
            BelowThreshold,
            Cooldown,
            EngineRunning,
            LiveChanged,
            StaleUsage,
        ] {
            assert_eq!(once_exit_code(&no_action(reason)), 2, "{reason:?}");
        }
        for reason in [
            NoCandidates,
            AllExhausted,
            NoViableTarget,
            InterruptedSwitch,
        ] {
            assert_eq!(once_exit_code(&blocked(reason, None)), 3, "{reason:?}");
        }
    }

    #[test]
    fn several_providers_exit_with_the_most_severe_code() {
        assert_eq!(most_severe(&[2, 3, 0, 1]), 1);
        assert_eq!(most_severe(&[2, 3, 0]), 0);
        assert_eq!(most_severe(&[2, 3, 2]), 3);
        assert_eq!(most_severe(&[2]), 2);
        assert_eq!(most_severe(&[]), 2, "no provider ticked");
        assert_eq!(
            most_severe(&[1, 130, 0]),
            130,
            "an unexpected code is never hidden"
        );
    }
}
