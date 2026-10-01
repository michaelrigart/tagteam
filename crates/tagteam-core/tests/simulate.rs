//! §11.5: multi-day synthetic traces driven through `decide` the way the engine drives it.
//! Every property runs a fixed seed and a pinned number of cases, so a run is deterministic.

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::{Config, RngAlgorithm, RngSeed, TestCaseError, TestRunner};
use tagteam_core::autoswitch::{
    self, AccountSnapshot, AutoConfig, AutoState, Decision, Live, NoSwitchReason, Outcome, Phase,
    Snapshot, Trigger, decide, departure, most_severe, next_delay, once_exit_code,
};
use tagteam_core::{AccountId, Window, WindowKind};

const START: i64 = 1_900_000_000;
const HOUR: i64 = 3_600;
const DAY: i64 = 86_400;
const SHORT_PERIOD_S: i64 = 18_000;
const LONG_PERIOD_S: i64 = 604_800;
const DAYS: i64 = 3;
const SEED: u64 = 0x7461_6774_6561_6d00;

/// A fixed seed and `cases` cases; failures are not persisted, since the seed replays them.
fn pinned(cases: u32) -> Config {
    Config {
        cases,
        rng_algorithm: RngAlgorithm::ChaCha,
        rng_seed: RngSeed::Fixed(SEED),
        failure_persistence: None,
        ..Config::default()
    }
}

/// One account's usage, as the simulation moves it on.
#[derive(Debug, Clone)]
struct Account {
    api_key: bool,
    /// Points per hour the 5h and the 7d window gain while the account is live.
    short_rate: f64,
    long_rate: f64,
    /// Points per hour both gain while another account is live (a session elsewhere).
    idle_rate: f64,
    short_pct: f64,
    long_pct: f64,
    short_reset: i64,
    long_reset: i64,
    /// Seconds between scheduled readings.
    poll_s: i64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Fault {
    /// A 429: no new reading, and the last stays trusted until its earliest reset, at most
    /// 2 h (§8.4).
    RateLimited,
    /// Every fetch fails and nothing is trusted: usage unknown.
    Unreadable,
    /// A dead refresh token: quarantined (§7.4) until the login is replaced.
    Dead,
}

#[derive(Debug, Clone)]
struct Incident {
    account: usize,
    fault: Fault,
    from: i64,
    until: i64,
}

#[derive(Debug, Clone)]
struct Trace {
    accounts: Vec<Account>,
    incidents: Vec<Incident>,
    cfg: AutoConfig,
    days: i64,
    jitter_seed: u64,
}

fn account(api_key: bool) -> impl Strategy<Value = Account> {
    (
        5.0..60.0f64,
        0.5..6.0f64,
        prop_oneof![3 => Just(0.0), 1 => 0.1..3.0f64],
        0.0..100.0f64,
        0.0..100.0f64,
        1..SHORT_PERIOD_S,
        1..LONG_PERIOD_S,
        prop_oneof![Just(60i64), Just(180), Just(300), Just(600)],
    )
        .prop_map(
            move |(
                short_rate,
                long_rate,
                idle_rate,
                short_pct,
                long_pct,
                short_in,
                long_in,
                poll_s,
            )| {
                Account {
                    api_key,
                    short_rate,
                    long_rate,
                    idle_rate,
                    short_pct,
                    long_pct,
                    short_reset: START + short_in,
                    long_reset: START + long_in,
                    poll_s,
                }
            },
        )
}

/// 2–6 accounts. The first, live at the start, is OAuth; each other is an API key one time in
/// five.
fn accounts() -> impl Strategy<Value = Vec<Account>> {
    proptest::collection::vec(prop::bool::weighted(0.2), 1..=5).prop_flat_map(|keys| {
        let mut all = vec![account(false).boxed()];
        all.extend(keys.into_iter().map(|k| account(k).boxed()));
        all
    })
}

fn incidents(n: usize) -> impl Strategy<Value = Vec<Incident>> {
    let fault = prop_oneof![
        Just(Fault::RateLimited),
        Just(Fault::Unreadable),
        Just(Fault::Dead)
    ];
    let incident =
        (0..n, fault, 0..DAYS * DAY, 300..8 * HOUR).prop_map(|(account, fault, from, len)| {
            Incident {
                account,
                fault,
                from: START + from,
                until: START + from + len,
            }
        });
    proptest::collection::vec(incident, 0..8)
}

/// Settings across their valid ranges (§6.4), and a provider with or without a long window.
fn settings() -> impl Strategy<Value = AutoConfig> {
    (
        prop_oneof![2 => Just(90.0), 1 => 50.0..99.9f64],
        prop_oneof![Just(0.0), Just(10.0), 0.0..50.0f64],
        prop_oneof![Just(0i64), Just(300), Just(900), Just(3_600)],
        prop_oneof![Just(15i64), Just(60), Just(300)],
        1u32..=5,
        prop_oneof![
            Just(autoswitch::Strategy::Best),
            Just(autoswitch::Strategy::ConsumeFirst)
        ],
        any::<bool>(),
        prop_oneof![4 => Just(Some("7d".to_owned())), 1 => Just(None)],
    )
        .prop_map(
            |(
                threshold,
                hysteresis_pct,
                cooldown_s,
                interval_s,
                unhealthy_ticks,
                strategy,
                include_api_key_accounts,
                long_window,
            )| AutoConfig {
                threshold,
                hysteresis_pct,
                cooldown_s,
                interval_s,
                unhealthy_ticks,
                strategy,
                include_api_key_accounts,
                models: vec![],
                long_window,
            },
        )
}

fn trace() -> impl Strategy<Value = Trace> {
    (accounts(), settings(), any::<u64>()).prop_flat_map(|(accounts, cfg, jitter_seed)| {
        incidents(accounts.len()).prop_map(move |incidents| Trace {
            accounts: accounts.clone(),
            incidents,
            cfg: cfg.clone(),
            days: DAYS,
            jitter_seed,
        })
    })
}

/// Review Focus 4: every account's weekly window spent, resetting one to three days in; no
/// API keys, faults or sessions elsewhere.
fn spent_week() -> impl Strategy<Value = Trace> {
    let spent =
        (account(false), DAY..3 * DAY, 0.0..90.0f64).prop_map(|(a, long_in, short_pct)| Account {
            long_pct: 100.0,
            long_reset: START + long_in,
            short_pct,
            idle_rate: 0.0,
            ..a
        });
    (proptest::collection::vec(spent, 2..=6), settings()).prop_map(|(accounts, cfg)| Trace {
        accounts,
        incidents: vec![],
        cfg,
        days: 4,
        jitter_seed: 7,
    })
}

fn id(i: usize) -> AccountId {
    AccountId::from_string(format!("acct-{i}"))
}

fn index(id: &AccountId) -> usize {
    id.as_str()["acct-".len()..].parse().unwrap()
}

fn window(key: &str, kind: WindowKind, pct: f64, resets_at: i64) -> Window {
    Window {
        key: key.into(),
        label: key.into(),
        kind,
        pct,
        resets_at: Some(resets_at),
        period_s: None,
        detail: None,
    }
}

/// A window past its reset starts over at 0%.
fn roll(pct: &mut f64, reset: &mut i64, period: i64, now: i64) {
    if now >= *reset {
        *pct = 0.0;
        while *reset <= now {
            *reset += period;
        }
    }
}

#[derive(Debug, Clone)]
struct Reading {
    windows: Vec<Window>,
    fetched_at: i64,
}

/// One tick as the log keeps it: when, the live account after it, and the decision.
type Tick = (i64, usize, Decision);

/// The engine's loop around `decide`, over a model of the accounts' usage.
struct Sim<'t> {
    trace: &'t Trace,
    accounts: Vec<Account>,
    readings: Vec<Option<Reading>>,
    live: usize,
    st: AutoState,
    now: i64,
    rng: u64,
}

impl<'t> Sim<'t> {
    fn new(trace: &'t Trace) -> Self {
        Self {
            trace,
            accounts: trace.accounts.clone(),
            readings: vec![None; trace.accounts.len()],
            live: 0,
            st: AutoState::default(),
            now: START,
            rng: trace.jitter_seed | 1,
        }
    }

    /// A jitter in [-1, 1] from a xorshift the trace seeds.
    fn jitter(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }

    fn fault(&self, i: usize) -> Option<Fault> {
        self.trace
            .incidents
            .iter()
            .find(|x| x.account == i && x.from <= self.now && self.now < x.until)
            .map(|x| x.fault)
    }

    /// Usage moves on to `to`: windows past their reset start over, then the live account burns
    /// at its rates and the others at their idle rate. Usage stops at 100%.
    fn advance(&mut self, to: i64) {
        let hours = (to - self.now) as f64 / HOUR as f64;
        for (i, a) in self.accounts.iter_mut().enumerate() {
            if a.api_key {
                continue;
            }
            roll(&mut a.short_pct, &mut a.short_reset, SHORT_PERIOD_S, to);
            roll(&mut a.long_pct, &mut a.long_reset, LONG_PERIOD_S, to);
            let (short, long) = if i == self.live {
                (a.short_rate, a.long_rate)
            } else {
                (a.idle_rate, a.idle_rate)
            };
            a.short_pct = (a.short_pct + short * hours).min(100.0);
            a.long_pct = (a.long_pct + long * hours).min(100.0);
        }
        self.now = to;
    }

    /// Scheduled collection reads every fault-free OAuth account that is due; a re-check
    /// (§8.3) also reads every one whose reading is older than 180 s.
    fn collect(&mut self, recheck: bool) {
        for i in 0..self.accounts.len() {
            let a = &self.accounts[i];
            if a.api_key || self.fault(i).is_some() {
                continue;
            }
            let age = self.readings[i].as_ref().map(|r| self.now - r.fetched_at);
            let read = age.is_none_or(|age| age >= a.poll_s || (recheck && age > 180));
            if read {
                self.readings[i] = Some(Reading {
                    windows: vec![
                        window("5h", WindowKind::Short, a.short_pct, a.short_reset),
                        window("7d", WindowKind::Long, a.long_pct, a.long_reset),
                    ],
                    fetched_at: self.now,
                });
            }
        }
    }

    /// What the engine hands `decide`: each account's decision-grade windows (§8.4). A
    /// reading with a plan in force is trusted for an hour.
    fn snapshot(&self) -> Snapshot {
        let accounts = self
            .accounts
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let fault = self.fault(i);
                let reading = self.readings[i].as_ref();
                let trusted = reading.filter(|r| match fault {
                    Some(Fault::Unreadable) => false,
                    Some(Fault::RateLimited) => {
                        let reset = r.windows.iter().filter_map(|w| w.resets_at).min();
                        let cap = r.fetched_at + 7_200;
                        self.now <= reset.map_or(cap, |reset| reset.min(cap))
                    }
                    _ => self.now - r.fetched_at <= HOUR,
                });
                AccountSnapshot {
                    id: id(i),
                    position: i as u32 + 1,
                    api_key: a.api_key,
                    switchable: true,
                    quarantined: !a.api_key && fault == Some(Fault::Dead),
                    session_owned: false,
                    windows: trusted.map(|r| r.windows.clone()),
                    fetched_at: reading.map(|r| r.fetched_at),
                }
            })
            .collect();
        Snapshot {
            now: self.now,
            live: Live::Managed(id(self.live)),
            accounts,
        }
    }

    /// One tick: collect, decide (re-checking first when consume-first asks), check the §11.5
    /// properties, switch to the first target as the engine would, and sleep `next_delay`.
    fn tick(&mut self) -> Result<Tick, TestCaseError> {
        let cfg = &self.trace.cfg;
        self.collect(false);
        let st = self.st.clone();
        let mut seen = self.snapshot();
        let mut d = decide(&seen, &st, cfg, Phase::Initial);
        if matches!(d.decision, Decision::Switch { recheck: true, .. }) {
            self.collect(true);
            seen = self.snapshot();
            d = decide(&seen, &st, cfg, Phase::Rechecked);
        }
        check(&seen, &st, cfg, &d.decision)?;
        self.st.unhealthy_ticks = d.unhealthy_ticks;
        if let Decision::Switch {
            trigger, targets, ..
        } = &d.decision
        {
            let from = id(self.live);
            let left = departure(&seen, cfg, &from, *trigger);
            self.st = AutoState {
                last_switch_at: Some(self.now),
                last_switch_from: Some(from),
                last_switch_to: Some(targets[0].clone()),
                left_headroom: left.left_headroom,
                left_recovery_at: left.left_recovery_at,
                left_trigger: Some(left.left_trigger),
                // The count was the old live account's.
                unhealthy_ticks: 0,
            };
            self.live = index(&targets[0]);
        }
        let plan = self.readings[self.live]
            .as_ref()
            .map(|r| r.fetched_at + self.accounts[self.live].poll_s);
        let jitter = self.jitter();
        let delay = next_delay(&d.decision, cfg, self.now, plan, jitter);
        check_delay(&d.decision, cfg, delay)?;
        let tick = (self.now, self.live, d.decision);
        self.advance(self.now + delay);
        Ok(tick)
    }
}

fn simulate(trace: &Trace) -> Result<Vec<Tick>, TestCaseError> {
    let mut sim = Sim::new(trace);
    let end = START + trace.days * DAY;
    let mut log = Vec::new();
    while sim.now < end {
        log.push(sim.tick()?);
    }
    Ok(log)
}

/// §8.2: `100 − max(pct)`; every window the simulation makes is relevant.
fn headroom(a: &AccountSnapshot) -> Option<f64> {
    a.windows
        .as_ref()?
        .iter()
        .map(|w| w.pct)
        .reduce(f64::max)
        .map(|p| 100.0 - p)
}

/// The binding window's reset (the highest pct, ties to the earlier window), if after `now`.
fn binding_reset(a: &AccountSnapshot, now: i64) -> Option<i64> {
    let binding = a
        .windows
        .as_ref()?
        .iter()
        .fold(None::<&Window>, |best, w| match best {
            Some(b) if b.pct >= w.pct => Some(b),
            _ => Some(w),
        })?;
    binding.resets_at.filter(|&at| at > now)
}

/// When an exhausted account is back: the latest reset among its windows at 100%.
fn back_at(a: &AccountSnapshot) -> Option<i64> {
    a.windows
        .as_ref()?
        .iter()
        .filter(|w| w.pct >= 100.0)
        .filter_map(|w| w.resets_at)
        .max()
}

/// §11.2 and §11.4: the outcome each reason stands for.
fn outcome_of(reason: NoSwitchReason) -> Outcome {
    match reason {
        NoSwitchReason::NoCandidates
        | NoSwitchReason::NoComparison
        | NoSwitchReason::NoQualifyingCandidate
        | NoSwitchReason::AllExhausted
        | NoSwitchReason::InterruptedSwitch
        | NoSwitchReason::NoViableTarget => Outcome::Blocked,
        _ => Outcome::NoAction,
    }
}

/// §11.3, restated from the spec: the account the engine left has recovered against its
/// departure snapshot (a missing snapshot counts).
fn recovered(
    left: &AccountSnapshot,
    st: &AutoState,
    active_h: Option<f64>,
    threshold: f64,
    now: i64,
) -> bool {
    let h = headroom(left);
    let reset = matches!(
        (binding_reset(left, now), st.left_recovery_at),
        (Some(at), Some(then)) if then - at >= 300
    );
    if st.left_trigger == Some(Trigger::Failover) {
        return reset || h.is_some_and(|h| 100.0 - h < threshold);
    }
    st.left_headroom.is_none()
        || matches!((h, st.left_headroom), (Some(h), Some(then)) if h - then >= 3.0)
        || reset
        || matches!((h, active_h), (Some(h), Some(a)) if h > 2.0 * a + 3.0)
}

/// The §11.5 properties for one decision, judged on the snapshot it was made from.
fn check(
    s: &Snapshot,
    st: &AutoState,
    cfg: &AutoConfig,
    d: &Decision,
) -> Result<(), TestCaseError> {
    let Live::Managed(live_id) = &s.live else {
        unreachable!("the simulation always has a managed live account")
    };
    let find = |id: &AccountId| s.accounts.iter().find(|a| &a.id == id).unwrap();
    let live = find(live_id);
    let active_h = if live.api_key {
        Some(0.0)
    } else {
        headroom(live)
    };
    // Step 7's candidates, read straight from the spec.
    let candidates: Vec<&AccountSnapshot> = s
        .accounts
        .iter()
        .filter(|a| {
            a.id != live.id
                && a.switchable
                && !a.quarantined
                && !a.session_owned
                && (!a.api_key || cfg.include_api_key_accounts)
        })
        .collect();
    let has_room = |a: &AccountSnapshot| headroom(a).is_some_and(|h| h > 0.0);
    let viable = candidates.iter().any(|c| c.api_key || has_room(c));

    // `--once` exit codes are consistent with the outcome.
    let code = match d {
        Decision::Switch { .. } => 0,
        Decision::NoSwitch {
            reason, outcome, ..
        } => {
            prop_assert_eq!(*outcome, outcome_of(*reason), "{:?}", reason);
            if *outcome == Outcome::NoAction { 2 } else { 3 }
        }
    };
    prop_assert_eq!(once_exit_code(d), code);

    // A quarantined active account fails over on the first tick that has a viable candidate.
    if live.quarantined && viable {
        prop_assert!(
            matches!(
                d,
                Decision::Switch {
                    trigger: Trigger::Failover,
                    ..
                }
            ),
            "quarantined at {} with a viable candidate: {:?}",
            s.now,
            d
        );
    }
    // Never idling at the limit while a viable candidate exists.
    if !live.api_key && !live.quarantined && headroom(live).is_some_and(|h| h <= 0.0) && viable {
        prop_assert!(
            matches!(
                d,
                Decision::Switch {
                    trigger: Trigger::AtLimit,
                    ..
                }
            ),
            "at the limit at {} with a viable candidate: {:?}",
            s.now,
            d
        );
    }
    match d {
        Decision::Switch {
            trigger, targets, ..
        } => {
            let to = find(&targets[0]);
            // Never landing on an account with headroom ≤ 0.
            prop_assert!(
                to.api_key || has_room(to),
                "landed on {:?} at {}",
                to,
                s.now
            );
            // Never landing on an API key unless at-limit or failover had no OAuth target.
            if to.api_key {
                prop_assert!(matches!(trigger, Trigger::AtLimit | Trigger::Failover));
                prop_assert!(!candidates.iter().any(|c| !c.api_key && has_room(c)));
            }
            // A return from an API key always lands below the threshold.
            if live.api_key {
                prop_assert!(
                    headroom(to).is_some_and(|h| 100.0 - h < cfg.threshold),
                    "{:?}",
                    to
                );
            }
            // No A→B→A return within a cooldown unless A recovered.
            let returning = st.last_switch_to.as_ref() == Some(&live.id)
                && st.last_switch_from.as_ref() == Some(&to.id);
            if returning && matches!(trigger, Trigger::Proactive | Trigger::ConsumeFirst) {
                let since = s.now - st.last_switch_at.unwrap();
                prop_assert!(since >= cfg.cooldown_s, "returned after {} s", since);
                prop_assert!(
                    recovered(to, st, active_h, cfg.threshold, s.now),
                    "returned to {:?} unrecovered from {:?}",
                    to,
                    st
                );
            }
        }
        Decision::NoSwitch {
            reason: NoSwitchReason::AllExhausted,
            earliest_reset,
            ..
        } => {
            // Every candidate is known to be exhausted, and the earliest recovery is reported.
            let oauth: Vec<&&AccountSnapshot> = candidates.iter().filter(|c| !c.api_key).collect();
            prop_assert!(oauth.iter().all(|c| headroom(c).is_some_and(|h| h <= 0.0)));
            prop_assert_eq!(
                *earliest_reset,
                oauth.iter().filter_map(|c| back_at(c)).min()
            );
        }
        Decision::NoSwitch { .. } => {}
    }
    Ok(())
}

/// §11.4's loop delay, bounded per outcome.
fn check_delay(d: &Decision, cfg: &AutoConfig, delay: i64) -> Result<(), TestCaseError> {
    let interval = cfg.interval_s;
    match d {
        Decision::NoSwitch {
            reason: NoSwitchReason::AllExhausted,
            earliest_reset: Some(_),
            ..
        } => prop_assert!((interval.min(600)..=600).contains(&delay), "{}", delay),
        Decision::NoSwitch {
            outcome: Outcome::Blocked,
            reason,
            ..
        } if *reason != NoSwitchReason::NoQualifyingCandidate => {
            prop_assert_eq!(delay, interval.max(300))
        }
        // A sleep may be shortened by the poll plan, never lengthened (Appendix B #27).
        _ => prop_assert!(
            delay >= 1 && delay as f64 <= (interval as f64 * 1.1).round(),
            "{}",
            delay
        ),
    }
    Ok(())
}

proptest! {
    #![proptest_config(pinned(48))]

    #[test]
    fn multi_day_traces_keep_every_auto_switch_property(trace in trace()) {
        simulate(&trace)?;
    }

    #[test]
    fn a_spent_week_never_flaps_and_moves_only_to_an_account_that_recovered(
        trace in spent_week()
    ) {
        // Review Focus 4. Before the first weekly reset every account is exhausted: each tick
        // is all-exhausted (its earliest reset and its sleep of at most 600 s are checked per
        // tick), and nothing switches. Every switch lands on an account whose week has reset.
        let first_reset = trace.accounts.iter().map(|a| a.long_reset).min().unwrap();
        let log = simulate(&trace)?;
        for (now, live, d) in &log {
            if *now < first_reset {
                prop_assert!(
                    matches!(
                        d,
                        Decision::NoSwitch { reason: NoSwitchReason::AllExhausted, .. }
                    ),
                    "at {}: {:?}",
                    now,
                    d
                );
            }
            if matches!(d, Decision::Switch { .. }) {
                prop_assert!(
                    trace.accounts[*live].long_reset <= *now,
                    "switched at {} to {} before its week reset",
                    now,
                    live
                );
            }
        }
        // When a candidate recovers first, the engine is on it within two polls and a sleep.
        let first = trace.accounts.iter().position(|a| a.long_reset == first_reset).unwrap();
        if first != 0 {
            let moved = log
                .iter()
                .find(|(_, _, d)| matches!(d, Decision::Switch { .. }))
                .map(|(now, _, _)| *now);
            prop_assert!(
                moved.is_some_and(|at| at <= first_reset + 1_800),
                "first switch at {:?}, first reset at {}",
                moved,
                first_reset
            );
        }
    }
}

proptest! {
    #![proptest_config(pinned(256))]

    #[test]
    fn several_providers_exit_with_the_most_severe_code(
        codes in proptest::collection::vec(0..=3i32, 1..6)
    ) {
        let expected = [1, 0, 3, 2].into_iter().find(|c| codes.contains(c)).unwrap();
        prop_assert_eq!(most_severe(&codes), expected);
    }
}

#[test]
fn a_seed_replays_the_same_traces_and_the_same_decisions() {
    let run = || {
        let mut runner = TestRunner::new(pinned(8));
        (0..8)
            .map(|_| {
                let trace = trace().new_tree(&mut runner).unwrap().current();
                simulate(&trace).unwrap()
            })
            .collect::<Vec<_>>()
    };
    let first = run();
    assert!(first.iter().all(|log| !log.is_empty()));
    assert_eq!(first, run());
}
