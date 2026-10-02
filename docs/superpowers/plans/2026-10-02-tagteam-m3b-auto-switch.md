# tagteam M3b — Auto-switch Implementation Plan

**Status:** In progress

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `tagteam auto` switches Claude Code's live login before a rate limit, on its own and
safely. It polls within the usage budget, decides with cswap's anti-flap rules (threshold,
hysteresis, cooldown, no-return, recovery axes, consume-first), never fights a manual switch,
and runs as exactly one engine per provider per machine. It also prints a line per tick, or
JSONL with `--json`, and stops cleanly on a signal.

**Architecture:**
- **The decision is pure.** `tagteam-core::autoswitch::decide` maps a snapshot (every
  account's decision-grade headroom, resets and flags), the persisted auto-switch state, the
  config and `now` to a `Decision`. It is built from named, unit-tested predicates and
  simulated over multi-day traces with proptest (§11.1, §11.5).
- **The engine runs one tick at a time.** `AutoEngine` (tagteam-engine) holds the engine lock.
  Each tick:
  1. releases quarantines that no longer bind;
  2. collects as scheduled;
  3. calls `decide`;
  4. freshens the ranked targets;
  5. performs the switch through the ordinary §9.4 transaction, carrying auto preconditions
     that the switch re-checks under its locks; the commit records the auto-switch state in
     the same transaction.
- **The loop is the CLI's.** `tagteam auto` drives one `AutoEngine` per provider on independent
  schedules. It sleeps toward wall-clock deadlines in ≤ 1 s slices that check the cancel
  token, re-reads `config.toml` when its mtime changes, and renders events as human lines or
  JSONL. The daemon (sub-project 2) will drive `AutoEngine` with its own loop.

**Tech Stack:** Rust (edition 2024), `proptest` (new dev-dependency of `tagteam-core`),
rusqlite, clap 4, serde_json, thiserror, tracing; tests with tempfile, assert_cmd,
`ScriptedHttp`, the engine's `test-hooks`, and M3a's binary signal-test helpers.

**Spec:** `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md` on `main` (`7ee733b`):
§4.2, §4.3, §4.5, §6.1, §6.4, §8.2–§8.6, §9.3, §9.4, §11 (all of it), §13.1, §13.2, §14.1,
§15.2 and Appendix B (#23–#27, #55, #56). M5's signed-off amendment `86882e3` (branch
`m5-admin`, not yet on `main`) adds one rule to §11.1, quoted in Task 8: once the engine holds
its lock, it writes its pid and start time into the lock file. Section numbers below refer to
the spec.

## Execution notes

- Run this plan in the worktree `~/Code/tagteam-m3-auto-switch` on branch `m3b-auto-switch`
  (cut from `main` at `7ee733b`, M3a merged). Before Task 1, rebase onto the current `main`
  if it has moved and run the full suite.
- When execution starts, set this plan's `**Status:**` to `In progress` in one commit.
- Task 11 is a human step, the live acceptance, run in throwaway Claude Code config
  directories so running Claude Code sessions are never touched (M3a's isolated method).
- Task 6 adds `proptest` (1.11.0) as a dev-dependency. The sandbox keeps `~/.cargo`
  read-only, so the first build that needs it fails on the registry write. Run
  `cargo fetch` once outside the sandbox after that failure, then continue sandboxed.
- Feature flags used by tests (unchanged): `tagteam-provider/file-keychain`,
  `tagteam-provider/mock-server`, `tagteam-engine/test-hooks`, `tagteam-cc/test-hooks`, and
  `tagteam/test-support`, which enables all of them.
- Clippy must pass both with `--features tagteam/test-support` and with no features. Every task
  runs `cargo fmt --all` before `cargo fmt --all --check`.
- The tagteam lib's pseudo-terminal prompt tests need a real pty: run them outside Claude Code's
  sandbox (it blocks `/dev/ptmx`). The macOS `real_keychain` test runs in CI.

## Milestones

| Milestone | Scope |
|---|---|
| M1, M2a, M2b, M3a | Implemented (M3a: PR #4, `main` `7ee733b`) |
| **M3b (this plan)** | Auto-switch settings, `primary_long_window`, scheduled and re-check collection, `decide()` and its simulations, the auto preconditions and commit-time record in the switch, `AutoEngine` (engine lock, tick, events, dry-run), the loop and `tagteam auto` |
| M4 | `tagteam run`; session-owned accounts (which §11.2 step 7 skips) |
| M5 | Export/import, `doctor` (reads the engine lock's pid/start record), `config` writes, CI and release |

**Deliberately absent from M3b, and why that is safe:**
- **No session-owned candidates.** M4 creates sessions; until then the gate's `session_owned`
  is `false` and `decide` receives `session_owned: false` for every account.
- **No `config set`.** M5 writes `config.toml`; `auto` only re-reads it when its mtime changes.

## Decisions

Rulings made while planning. Each names what it would cost if wrong.

1. **`decide` is one pure function over explicit types** (`tagteam-core/src/autoswitch.rs`,
   Interface Contract). The engine builds the `Snapshot` from the store, its decision-grade
   windows (M3a's `decision_windows`) and the live login. Nothing in `decide` reads a clock,
   the store or a provider. Cost if wrong: a field missing from the snapshot needs a contract
   change, caught by the simulations.
2. **Consume-first's two phases are two calls.** `decide(…, Phase::Initial)` ranks on stored
   readings and returns `Decision::Switch { recheck: true, .. }`. The engine then re-checks the
   current account and every candidate (`CollectMode::Recheck`, §8.3), rebuilds the snapshot,
   and calls `decide(…, Phase::Rechecked)`. That call re-ranks and, when it still decides a
   `consume-first` switch, requires the first target's reading to be ≤ 180 s old, else
   `no-switch stale-usage` (§11.2 step 8). It drops every later target whose reading is older,
   so each target the tick tries is fresh. A re-checked tick that now finds `at-limit` or
   `failover` switches without the freshness check: those triggers skip every gate.
3. **The engine lock is a try-only `flock`** on `locks/autoswitch-<provider>.lock`
   (`FlockGuard::try_lock`), held by `AutoEngine` for its life. After taking it, the engine
   truncates the file and writes `{"pid":…,"start":…}` from `ProcessStamp::current()` (M5's
   `86882e3`). The record is never read for exclusion. Cost if wrong: `doctor` (M5) parses
   the format; it is pinned by a test.
4. **`autoswitch_state` has one writer, the engine.** It updates `unhealthy_ticks` itself after
   each tick that does not switch. The departure snapshot (`last_switch_*`, `left_headroom`,
   `left_recovery_at`, `left_trigger`) is written by the switch's commit transaction (§9.4
   step 9), never separately, and that transaction also resets `unhealthy_ticks` to 0: the
   count belongs to the account it judged, and a stale count would fail over again from the
   new account at its first unknown tick.
5. **Auto preconditions travel in the `SwitchRequest`** (`auto: Option<AutoPerform>`), and the
   switch checks them in `rederive` under its locks (§9.4 step 1, §11.2 step 11):
   - the live account is still `expected_from`, otherwise `SwitchReason::LiveChanged`;
   - for `proactive` / `consume-first`, the cooldown read from `autoswitch_state` under the lock
     still allows the switch, otherwise `SwitchReason::Cooldown`;
   - the target is still a candidate, otherwise `SwitchReason::NotCandidate`, and the tick tries
     its next target.

   A refused precondition writes nothing.
6. **Events go through a trait.** `EventSink::emit(&self, e: &AutoEvent)` is in the engine;
   `AutoEvent` mirrors §11.4's table. The CLI implements a JSONL sink and a human sink. Cost if
   wrong: a field the daemon needs later is added to `AutoEvent`. `EventSink::tick_done`, a
   no-op by default, marks the end of every tick on every path, so a sink that keeps something
   for a tick's line (the human sink's poll) lets it go even when the tick ends in an error.
7. **The loop lives in the CLI** (`crates/tagteam/src/auto.rs`). It is a thin driver over
   `AutoEngine::tick` and `tagteam_core::autoswitch::next_delay`, with a `Sleeper` seam so tests
   can drive it with a fake clock (wall-clock jumps model a suspend). The daemon writes its own
   driver.
8. **An interruption ends the loop with exit 0** (§11.4). Inside a tick it surfaces as the
   tick's `Err(Interrupted)` at a cancellation point, or as the late-signal case after a
   committed switch. `--once` interrupted exits 128 + n like any command (§14.1).
9. **Settings are re-read between ticks** when `config.toml`'s mtime changed. Flags still win.
   A reload that changes `autoswitch.models` re-runs the model-name check (§11.2 step 3).
10. **Quarantine changes made by other processes are reported by diff.** The engine keeps the
    previous tick's quarantine map in memory. The first tick reports only its own releases.
    `account-unquarantined`'s reason is `account-replaced` when the account's `login_epoch`
    moved, else `credentials-replaced` (§11.4).
11. **`proptest` is a dev-dependency of `tagteam-core` only.** Simulations use a fixed seed per
    test (deterministic, §11.5), with the case count pinned.
12. **M3a carry-overs folded in:**
    - the untested gate-entry and `refresh_live` cancellation points (now reused by the tick's
      collection), in Task 3;
    - the binary-test pipe-draining helper (M3a's `--debug` tests read output only after exit),
      in Task 10;
    - the misleading `usage-unavailable` message when a known candidate was dropped as
      exhausted beside a credential-less healthy one (parked in M3a's final review), in Task 7.

13. **A recovered switch records no auto-switch state** (§9.6, §11.2 step 2). The journal names
    neither the switch's source nor its trigger, and a departure snapshot would be a dead
    tick's view of usage. Cost if wrong: after a crashed automatic switch, the next tick has no
    cooldown and no bar; recording one needs journal columns (a migration).
14. **The unknown-usage count belongs to the account it judged.** The engine resets it when
    the live account differs from the one its previous deciding tick judged; a fresh engine
    trusts the stored count. Cost if wrong: a `--once` run right after a manual switch inherits
    the old account's count once.

## Global Constraints

Every task's requirements include these. Values are copied from the spec.

- Platforms: macOS and Linux (§1.2). Rust edition 2024, toolchain pinned by
  `rust-toolchain.toml` (1.88.0). `#![forbid(unsafe_code)]` stays in `tagteam-core`,
  `tagteam-engine` and `tagteam-fake`.
- Settings (§6.4), clamped as listed:
  - `autoswitch.threshold` 90.0 (50–99.9)
  - `interval_seconds` 60 (15–3600)
  - `cooldown_seconds` 300 (0–86400)
  - `hysteresis_pct` 10.0 (0–50)
  - `strategy` `best` | `consume-first`
  - `include_api_key_accounts` false
  - `unhealthy_ticks` 3 (1–100)
  - `models` `[]` (names or `["all"]`)

  CLI flags override for one invocation, clamped the same.
- §11 constants:
  - the 180 s re-check freshness;
  - no-return recovery: +3 points headroom, a reset ≥ 300 s sooner, or dominance `> 2 × active + 3`;
  - all-above-threshold axis: within 3 points of headroom, or a reset within 4 h (14 400 s);
    recovery-axis gap ≥ 300 s; headroom-axis ratio ≥ 2 ×;
  - `all-exhausted` sleep = earliest recovery + 60 s, capped at 600 s.
- Loop delay (§11.4):
  - BLOCKED with a known reset: `min(max(until − now, interval), 600)`;
  - BLOCKED otherwise: `max(interval, 300)`;
  - anything else: `interval × U(0.9, 1.1)`, shortened (never lengthened) to the active
    account's `next_poll_at`, floored at 60 s.

  A `sleep` event when the delay > 1.5 × interval.
- `--once` exit codes: `0` switched · `1` error · `2` no action · `3` blocked; with several
  providers the most severe (1 > 0 > 3 > 2) (§11.1).
- JSONL envelope `{"schemaVersion":1,"event":<kind>,"ts":"…Z",…}` with an additive `provider`.
  Every event kind and field is listed in §11.4.
- Budget and trust unchanged: ≤ 20 usage requests per identity per rolling hour across
  processes (§8.6); decisions use decision-grade readings only (§8.4).
- No cancellation point inside a critical span (§14.1). The auto switch goes through `apply`,
  so its step-7 write waits on `Engine::critical_env` (M3a Decision 13).
- Secrets never reach `Debug`, logs or error messages; log lines name accounts by position and
  ID (§4.4).
- Tests never touch the real HOME, the login keychain or the network (§15.1).
- Commits: small, imperative mood, no license headers, no agent attribution of any kind.

## Review Focus

Inputs and conditions the spec implies but no feature test would naturally hit. Each has a
pinning test in the task named.

1. **You switch by hand while `auto` runs** (a manual `tagteam switch 3` between two ticks, or
   during a tick's freshen). Expected: auto never switches you back on its own initiative;
   the tick that decided on the old live account reports `live-changed`, and the next tick
   decides afresh from account 3. → Task 7 (precondition) and Task 8 (tick).
2. **A laptop sleeps for an hour with `auto` running.** Expected: on wake the loop ticks
   promptly (its deadline is wall clock), stale readings are not trusted, and no burst of
   requests exceeds the budget. → Task 9.
3. **Two `tagteam auto` loops started by mistake** (two terminals, or a cron `--once` while a
   loop runs). Expected: the second loop skips the provider with a warning (exit 1 if it holds
   no provider); `--once` reports `engine-running`, exit 2; nothing is counted twice. → Task 8
   and Task 10.
4. **Every account near or at its limit for days** (the weekly window spent everywhere).
   Expected: no flapping between exhausted accounts; `all-exhausted` with the earliest reset,
   sleeping ≤ 600 s; a switch happens only once an account recovers past the gates. → Task 5
   and Task 6 (simulations).
5. **The live login is not one tagteam manages** (you ran `claude /login` with a new account).
   Expected: `no-switch unmanaged-active-account`, NO_ACTION, every tick; auto never
   overwrites it. → Task 4 and Task 8.

---

## File Structure

```
Cargo.toml, Cargo.lock           MOD        proptest (Task 6); fastrand, tagteam-fake (Task 9)
crates/tagteam-core/
  Cargo.toml                                + proptest (dev)
  src/autoswitch.rs              NEW        Strategy (Task 1); Snapshot, AutoState, AutoConfig,
                                            Decision, decide, next_delay, the named predicates
                                            (Tasks 4, 5)
  src/lib.rs                     MOD        `pub mod autoswitch` (Task 1)
  src/poll.rs                    MOD        scheduled_pick (Task 3)
  tests/simulate.rs              NEW        §11.5 proptest traces (Task 6)
crates/tagteam-provider/
  src/provider.rs                MOD        `Provider::primary_long_window` (Task 2)
crates/tagteam-cc/
  src/provider.rs                MOD        CC: Some("7d") (Task 2)
crates/tagteam-fake/
  src/provider.rs                MOD        FakeAgent: None (Task 2)
crates/tagteam-engine/
  src/settings.rs                MOD        the remaining autoswitch keys; mtime-aware reload (Task 1)
  src/collect.rs                 MOD        CollectMode::{Scheduled, Recheck} (Task 3)
  src/store/usage.rs             MOD        Eligibility: scheduled / re-check (Task 3)
  src/switch.rs                  MOD        is_candidate pub(crate) (Task 3)
  tests/{collect,collect_active,store_usage,views_usage}.rs MOD (Task 3)
  src/store/mod.rs               MOD        autoswitch_state access; commit_switch records it (Task 7)
  src/switch.rs                  MOD        AutoPerform preconditions; new reasons; best_pick message (Task 7)
  src/auto.rs                    NEW        AutoEngine, EventSink, AutoEvent, the tick (Task 8)
  src/lib.rs                     MOD        `pub mod auto` (Task 8)
  src/switch.rs, src/engine.rs   MOD        freshen_auto; recovery takes the event source (Task 8)
  src/recover.rs                 MOD        commit_switch(None) (Task 7); source (Task 8)
  src/store/usage.rs             MOD        usage_lease_expires_at (Task 8)
  tests/common/mod.rs            MOD        Recorded sink, usage_window, record_reading (Task 8)
  tests/{auto_tick,auto_engine}.rs NEW      (Task 8)
  tests/{collect,switch,strategy,settings}.rs MOD
crates/tagteam/
  Cargo.toml                     MOD        fastrand; tagteam-fake (dev) (Task 9)
  src/auto.rs                    NEW        the loop driver, Sleeper, sinks (Tasks 9, 10)
  src/cli.rs, src/app.rs, src/lib.rs MOD    `auto` command, flags, exit codes (Task 10)
  tests/auto_cli.rs              NEW        (Task 10)
  tests/app.rs, tests/signals.rs MOD        auto checks; the drain helper (Task 10)
  tests/common/mod.rs            MOD        reserve_usage's Eligibility (Task 3); pipe-draining
                                            spawn helper (Task 10)
  tests/usage_cli.rs, src/statusline.rs MOD reserve_usage's Eligibility (Task 3)
```

---

## Interface Contract

Every task implements exactly these names and signatures. A task that finds one unworkable
stops and reports it rather than inventing a variant.

### `tagteam-core`

**`src/autoswitch.rs`** (Task 1 creates it with `Strategy` alone, for the settings; Tasks 4
and 5 add the rest):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy { Best, ConsumeFirst }
impl Strategy { pub fn as_str(self) -> &'static str; pub fn parse(s: &str) -> Option<Self>; } // "best" | "consume-first"

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger { Proactive, AtLimit, Failover, ConsumeFirst }
impl Trigger { pub fn as_str(self) -> &'static str; pub fn parse(s: &str) -> Option<Self>; } // "proactive" | "at-limit" | "failover" | "consume-first"

#[derive(Debug, Clone, PartialEq)]
pub struct AutoConfig {
    pub threshold: f64,
    pub hysteresis_pct: f64,
    pub cooldown_s: i64,
    pub interval_s: i64,
    pub unhealthy_ticks: u32,
    pub strategy: Strategy,               // as configured
    pub include_api_key_accounts: bool,
    pub models: Vec<String>,              // §8.2 relevance (`autoswitch.models`)
    pub long_window: Option<String>,      // Provider::primary_long_window; None runs `best` for a
                                          // consume-first strategy (§4.5)
}
impl AutoConfig {
    /// The strategy that actually runs: `Best` when consume-first has no long window.
    pub fn effective_strategy(&self) -> Strategy;
}

/// One account as a tick sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountSnapshot {
    pub id: AccountId,
    pub position: u32,
    pub api_key: bool,                // a managed-key kind (§7.1)
    pub switchable: bool,             // vault credential + identity, not disabled (§9.3)
    pub quarantined: bool,
    pub session_owned: bool,          // false until M4
    /// The decision-grade reading's windows (§8.4); `None` when there is none. Headroom,
    /// the binding window and the recovery time come from `usage::headroom`,
    /// `rank::binding_window` and `rank::blocked_until` over `cfg.models`.
    pub windows: Option<Vec<Window>>,
    pub fetched_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Live {
    None,                             // no live login
    Unmanaged,                        // a live login tagteam does not manage
    Managed(AccountId),               // the live account
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot { pub now: i64, pub live: Live, pub accounts: Vec<AccountSnapshot> }

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
pub enum Phase { Initial, Rechecked }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome { NoAction, Blocked }

/// Every `no-switch` reason (§11.4), kebab-case via `as_str`. `decide` returns the first
/// group; the engine reports the second (§11.1, §11.2 steps 2, 11, 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoSwitchReason {
    UnmanagedActiveAccount, NoActiveAccount, ActiveApiKey, BelowThreshold,
    ActiveUsageUnknown, Cooldown, NoCandidates, NoComparison, ResetUnknown,
    AlreadyConsumingSoonest, NoQualifyingCandidate, StaleUsage, AllExhausted,
    // engine-reported
    EngineRunning, InterruptedSwitch, LiveChanged, NoViableTarget,
}
impl NoSwitchReason {
    pub fn as_str(self) -> &'static str;
    /// NO_ACTION or BLOCKED for every reason, the engine-reported ones too: `engine-running`
    /// and `live-changed` are NO_ACTION; `interrupted-switch` and `no-viable-target` BLOCKED.
    pub fn outcome(self) -> Outcome;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    NoSwitch {
        reason: NoSwitchReason,
        outcome: Outcome,
        detail: String,               // "2/3" for active-usage-unknown, the time left ("3m") for
                                      // cooldown, the span to the earliest recovery ("2h00m")
                                      // for all-exhausted; "" when none
        earliest_reset: Option<i64>,  // all-exhausted: the earliest recovery (`earliestResetAt`)
    },
    Switch {
        trigger: Trigger,
        targets: Vec<AccountId>,      // ranked OAuth targets, then (at-limit/failover only) API-key
                                      // candidates in position order (§11.2 steps 9-10)
        recheck: bool,                // consume-first, Phase::Initial only (Decision 2)
    },
}

/// `unhealthy_ticks`: the counter after this tick (§11.2 step 5): 0 on known active headroom,
/// +1 on unknown, unchanged when step 5 is not reached or the active account is quarantined.
#[derive(Debug, Clone, PartialEq)]
pub struct Decided { pub decision: Decision, pub unhealthy_ticks: u32 }

/// §11.2 steps 2 and 4–9 (the engine owns steps 1, 3, 10–12). Pure.
pub fn decide(s: &Snapshot, st: &AutoState, cfg: &AutoConfig, phase: Phase) -> Decided;

/// The departure snapshot a switch records for `from` (§11.2 step 11, §11.3).
pub fn departure(s: &Snapshot, cfg: &AutoConfig, from: &AccountId, trigger: Trigger) -> Departure;
#[derive(Debug, Clone, PartialEq)]
pub struct Departure { pub left_headroom: Option<f64>, pub left_recovery_at: Option<i64>, pub left_trigger: Trigger }

/// §11.4 loop delay, seconds. `jitter` in [-1, 1] maps to U(0.9, 1.1).
/// - `all-exhausted` with an `earliest_reset` t: `min(max(t + 60 − now, interval), 600)`;
/// - any other BLOCKED outcome except `no-qualifying-candidate` (normal cadence, §11.2 step 9):
///   `max(interval, 300)`;
/// - everything else (switched, NO_ACTION, error, `no-qualifying-candidate`): the jittered
///   interval, shortened to `active_next_poll_at − now` when sooner; the 60 s floor bounds
///   only that shortening, so a 15 s interval stays 15 s (Appendix B #27).
pub fn next_delay(d: &Decision, cfg: &AutoConfig, now: i64, active_next_poll_at: Option<i64>, jitter: f64) -> i64;
/// Whether a delay earns a `sleep` event (> 1.5 × interval).
pub fn announces_sleep(delay_s: i64, cfg: &AutoConfig) -> bool;
/// `--once` (§11.1): 0 switched, 2 NO_ACTION, 3 BLOCKED.
pub fn once_exit_code(d: &Decision) -> i32;
/// Several providers: the most severe, 1 > 0 > 3 > 2; empty is 2.
pub fn most_severe(codes: &[i32]) -> i32;
```

**`src/poll.rs`** (Task 3):

```rust
/// One candidate as scheduling sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DueCandidate { pub position: u32, pub due: bool, pub fetched_at: Option<i64> }
/// §8.6 phase 2: the single stalest due candidate (never fetched first, then oldest
/// `fetched_at`, ties to the lower position), or every due candidate when `escalate`.
pub fn scheduled_pick(cands: &[DueCandidate], escalate: bool) -> Vec<u32>;
/// §8.6: escalate when the active max relevant pct ≥ threshold − 15, or its headroom is
/// still unknown after phase 1.
pub fn escalates(active_max_pct: Option<f64>, threshold: f64, margin: f64) -> bool;
```

### `tagteam-provider`

```rust
// Provider trait (Task 2): the window key consume-first ranks on (§4.5). CC: Some("7d").
fn primary_long_window(&self) -> Option<&'static str>;
```

### `tagteam-engine`

**`src/settings.rs`** (Task 1): `Settings` gains `interval_seconds: i64`,
`cooldown_seconds: i64`, `hysteresis_pct: f64`, `strategy: Strategy`,
`include_api_key_accounts: bool`, `unhealthy_ticks: u32` (provider table first, then global,
as `threshold`). `Settings::mtime(env: &Env) -> Option<SystemTime>` reports `config.toml`'s
mtime for the loop's reload check. For the CLI flags: `DEFAULT_*` and `*_RANGE` constants for
each numeric key, and `settings::parse_bool(&str) -> Option<bool>`.

**`src/collect.rs`** (Task 3):

```rust
pub enum CollectMode {
    OnDemand { accounts: Vec<AccountId> },
    /// §8.6: phase 1 the active account if due; phase 2 the pick (`scheduled_pick`).
    Scheduled { provider: ProviderId, threshold: f64, models: Vec<String> },
    /// §8.3 re-check: each listed account whose reading is older than 180 s, ignoring its plan.
    Recheck { accounts: Vec<AccountId>, threshold: f64, models: Vec<String> },
}
```

`Scheduled` and `Recheck` plan the next poll (`plan_after_fetch`) from the mode's threshold and
models, so a tick's flags and reloaded settings drive its whole collection; `OnDemand` keeps
`Settings`. `CollectReport` gains `quarantined: Vec<AccountId>` (accounts quarantined while
their collection ran). `Store::reserve_usage` takes `eligibility: Eligibility`
(`pub enum store::Eligibility { OnDemand, Scheduled, Recheck }`) instead of `on_demand: bool`.

**`src/store/mod.rs`** (Task 7):

```rust
impl Store {
    pub fn autoswitch_state(&self, provider: &ProviderId) -> Result<AutoState, StoreError>;
    pub fn set_unhealthy_ticks(&self, provider: &ProviderId, n: u32) -> Result<(), StoreError>;
    /// §9.4 step 9: `record` is written in the same transaction as the switch's commit.
    pub fn commit_switch(&self, provider: &ProviderId, to: &AccountId, event: &EventRow,
                         record: Option<&AutoRecord>) -> Result<(), StoreError>;
}
#[derive(Debug, Clone, PartialEq)]
pub struct AutoRecord { pub at: i64, pub from: AccountId, pub to: AccountId, pub departure: Departure }
```

**`src/switch.rs`** (Task 7):

```rust
pub struct SwitchRequest { /* … */ pub auto: Option<AutoPerform> }
#[derive(Debug, Clone)]
pub struct AutoPerform { pub expected_from: AccountId, pub trigger: Trigger, pub cooldown_s: i64, pub departure: Departure }
pub enum SwitchReason { /* … */ LiveChanged, Cooldown, NotCandidate }   // "live-changed" | "cooldown" | "not-candidate"
```

**`src/auto.rs`** (Task 8):

```rust
pub trait EventSink {
    fn emit(&self, e: &AutoEvent);
    /// The end of every tick: `AutoEngine::tick` calls it exactly once, last, on every path,
    /// `Ok` and `Err` alike. The human sink drops the tick's poll here.
    fn tick_done(&self, _provider: &ProviderId) {}
}

#[derive(Debug, Clone, PartialEq)]
pub enum AutoEvent {   // §11.4; every one carries its provider
    Poll { provider: ProviderId, active: Option<(u32, String)>, headroom_pct: BTreeMap<u32, Option<f64>>,
           threshold: f64, fetch_errors: BTreeMap<u32, String>, windows_pct: BTreeMap<u32, BTreeMap<String, f64>> },
    Switch { provider: ProviderId, trigger: Trigger, from: u32, to: u32, warnings: Vec<String>, dry_run: bool },
    NoSwitch { provider: ProviderId, reason: String, detail: String },
    AccountQuarantined { provider: ProviderId, number: u32, email: String, reason: String },
    AccountUnquarantined { provider: ProviderId, number: u32, email: String, reason: String },
    AllExhausted { provider: ProviderId, earliest_reset_at: Option<i64> },
    Sleep { provider: ProviderId, seconds: f64, until: i64 },
    Error { provider: ProviderId, message: String, transient: bool },
    ConfigWarning { provider: ProviderId, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickOutcome { Switched, NoAction, Blocked, Error }

pub struct AutoEngine<'e> { /* engine, provider, config, lock (None in dry-run), in-memory state */ }

impl Engine {
    /// §11.1: takes the engine lock (try-only) and writes its pid/start record; `Ok(None)` when
    /// another process holds it (`engine-running`). Dry-run takes no lock. A real engine inside
    /// a `tagteam run` shell is `Err(InsideRunShell)`.
    pub fn auto(&self, provider: &ProviderId, cfg: AutoConfig, dry_run: bool)
        -> Result<Option<AutoEngine<'_>>, EngineError>;
}
impl AutoEngine<'_> {
    /// One §11.2 tick. `Err` only for an interruption or a store failure; every other failure
    /// is an `error` event and `TickOutcome::Error`. Every path ends with `sink.tick_done`.
    pub fn tick(&mut self, sink: &dyn EventSink) -> Result<(TickOutcome, Decision), EngineError>;
    pub fn set_config(&mut self, cfg: AutoConfig);   // a settings reload (Decision 9)
    /// The later of the live account's `next_poll_at` and its usage lease's expiry (§8.3: a
    /// lease outlives its record), so the loop never wakes a tick that cannot fetch.
    pub fn active_next_poll_at(&self) -> Option<i64>;
}
// store/usage.rs (Task 8)
impl Store { pub fn usage_lease_expires_at(&self, id: &AccountId) -> Result<Option<i64>, StoreError>; } // epoch ms
```

### `tagteam` (CLI)

```rust
// cli.rs (Task 10)
Auto {
    #[arg(long)] once: bool,
    #[arg(long = "dry-run")] dry_run: bool,
    #[arg(long)] threshold: Option<f64>,
    #[arg(long)] interval: Option<i64>,
    #[arg(long)] cooldown: Option<i64>,
    #[arg(long, value_enum)] strategy: Option<AutoStrategyArg>,   // best | consume-first
    #[arg(long)] model: Option<String>,
    #[arg(long = "include-api-key-accounts", value_name = "BOOL")] include_api_key_accounts: Option<String>,
},

// auto.rs (Tasks 9, 10)
pub trait Sleeper { fn sleep(&self, d: Duration); }
/// Exit code 0/1/2/3 for `--once` (most severe across providers), 0 for a loop ended by a
/// signal. An interrupted `--once`, or a loop that cannot start, is a command error.
pub fn run_loop(engine: &Engine, run: &AutoRun, sink: &dyn EventSink, sleeper: &dyn Sleeper,
                jitter: &mut dyn FnMut() -> f64) -> Result<i32, AutoError>;
pub enum AutoError { AlreadyRuns(Vec<ProviderId>), NothingToSwitch, NoLongWindow(ProviderId),
                     Engine(EngineError) }
// NoLongWindow: `--strategy consume-first` with `--provider` naming a provider that has no
// `primary_long_window` (§4.5), refused before any engine starts.
// also pub: ThreadSleeper, uniform_jitter, AutoFlags, auto_config, AutoRun, providers,
// JsonSink, HumanSink, event_json, local_time (Tasks 9, 10)

// app.rs (Task 10): `dispatch` returns `Result<i32, Failure>`; new error kinds
// `engine-running` and `no-candidates` (exit 1); `NoLongWindow` is a usage error (exit 2).
```

---

## Tasks

| # | Task | Human? |
|---|---|---|
| 1 | Auto-switch settings | |
| 2 | `primary_long_window` | |
| 3 | Scheduled and re-check collection | |
| 4 | `decide()`: triggers, candidates and outcomes | |
| 5 | `decide()`: ranking, the no-return rule and the API-key fallback | |
| 6 | Simulation tests | |
| 7 | Auto preconditions and the commit-time record in the switch | |
| 8 | `AutoEngine`: the engine lock, the tick and its events | |
| 9 | The loop: delays, wall-clock sleep, settings reload, several providers | |
| 10 | `tagteam auto` | |
| 11 | Final verification and live acceptance | Yes |

### Task 1: Auto-switch settings

§6.4 gives every `autoswitch.*` key a default and a valid range, and says: "Reads are forgiving.
A corrupt file or an out-of-range value falls back to the default, with a warning." It adds that
the keys are read "from that provider's table first, then from the global table", and that
"Booleans parse only `true/false/1/0/yes/no`". `settings.rs` reads two of the eight keys today,
`threshold` and `models`. This task reads the other six the same way, and adds `Settings::mtime`
for the loop's reload check (§11.4: "Settings are re-read before a tick whenever `config.toml`'s
mtime has changed"). It also creates `tagteam-core`'s `autoswitch` module, holding only
`Strategy` for now; Tasks 4 and 5 add the rest.

**Readings of the spec this task commits to:**
- **An invalid value in the file falls back; a flag is clamped.** A value of the wrong type or
  outside its range warns, naming the key, exactly as `threshold` does today: an invalid
  provider value warns and the global value applies; an invalid global value gives the default.
  The ranges are public constants, so Task 10 clamps `--threshold`, `--interval` and
  `--cooldown` into the same ranges (§6.4: "clamped to the same ranges").
- **Seconds and ticks are whole numbers.** `interval_seconds`, `cooldown_seconds` and
  `unhealthy_ticks` take TOML integers only, as `usage.history_retention_days` does; `60.0` is
  invalid. `hysteresis_pct` takes an integer or a float, as `threshold` does.
- **Booleans.** In the file a boolean is a TOML boolean, the integer `1` or `0`, or one of the
  six words as a string, in lower case: `"Yes"`, `"on"`, `2` and `1.0` are invalid.
  `parse_bool` is the words alone, public for Task 10's `--include-api-key-accounts <BOOL>`.
- **The strategy is its lower-case name**, `best` or `consume-first` (`Strategy::parse`).
- **`Settings::mtime` is `None` when the file is missing or its metadata cannot be read.** The
  loop (Task 9) reads it before `load`, so a write landing between the two shows as one more
  change at the next check rather than going unseen, and compares mtimes for inequality, not
  order.
- **Every command reads every key.** `list`, `status`, `switch` and the statusline load the same
  `Settings`, so an invalid auto-switch key warns there too, as an invalid `threshold` does
  today (`settings_warnings_go_to_stderr_and_never_fail_the_command`); the statusline drops its
  warnings, as before. Nothing reads the new fields until Task 8.

**Files:**
- Create: `crates/tagteam-core/src/autoswitch.rs`
- Modify: `crates/tagteam-core/src/lib.rs`
- Modify: `crates/tagteam-engine/src/settings.rs`
- Modify: `crates/tagteam-engine/tests/settings.rs`

Every other `Settings { … }` in the workspace ends in `..Settings::default()` (`engine.rs`,
`tests/views_usage.rs`, `tests/strategy.rs`), so it compiles unchanged.

**Interfaces:**
- Consumes: `Reader::read` in `settings.rs` (the provider's table first, then the global one;
  one deduplicated warning per invalid key), `Env::config_dir`.
- Produces:
  - `tagteam_core::autoswitch::Strategy { Best, ConsumeFirst }`, with
    `Strategy::as_str(self) -> &'static str` and `Strategy::parse(s: &str) -> Option<Self>`
    (`"best"`, `"consume-first"`)
  - `Settings` fields `interval_seconds: i64`, `cooldown_seconds: i64`, `hysteresis_pct: f64`,
    `strategy: Strategy`, `include_api_key_accounts: bool`, `unhealthy_ticks: u32`
  - `settings::{DEFAULT_INTERVAL_SECONDS, DEFAULT_COOLDOWN_SECONDS, DEFAULT_HYSTERESIS_PCT,
    DEFAULT_UNHEALTHY_TICKS}` and the ranges `settings::{THRESHOLD_RANGE,
    INTERVAL_SECONDS_RANGE, COOLDOWN_SECONDS_RANGE, HYSTERESIS_PCT_RANGE,
    UNHEALTHY_TICKS_RANGE}` (`RangeInclusive`)
  - `settings::parse_bool(s: &str) -> Option<bool>`
  - `Settings::mtime(env: &Env) -> Option<SystemTime>`

- [ ] **Step 1: Create the core module and write the failing tests**

Create `crates/tagteam-core/src/autoswitch.rs` with exactly this content; Tasks 4 and 5 add the
rest of the module:

```rust
//! §11: auto-switch decisions. Pure: no clock, no I/O.

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
```

In `crates/tagteam-core/src/lib.rs`, replace:

```rust
#![forbid(unsafe_code)]

pub mod backoff;
```

with:

```rust
#![forbid(unsafe_code)]

pub mod autoswitch;
pub mod backoff;
```

In `crates/tagteam-engine/tests/settings.rs` (the imports), replace:

```rust
use std::fs;

use tagteam_core::ProviderId;
use tagteam_engine::settings::{
    ColorMode, STATUSLINE_PLACEHOLDERS, Settings, is_statusline_placeholder,
};
use tagteam_provider::Env;
```

with:

```rust
use std::fs;
use std::time::{Duration, SystemTime};

use tagteam_core::ProviderId;
use tagteam_core::autoswitch::Strategy;
use tagteam_engine::settings::{
    COOLDOWN_SECONDS_RANGE, ColorMode, HYSTERESIS_PCT_RANGE, INTERVAL_SECONDS_RANGE,
    STATUSLINE_PLACEHOLDERS, Settings, THRESHOLD_RANGE, UNHEALTHY_TICKS_RANGE,
    is_statusline_placeholder, parse_bool,
};
use tagteam_provider::Env;
```

In `crates/tagteam-engine/tests/settings.rs`, in `the_defaults_are_the_specs_table`, replace:

```rust
    let d = Settings::default();
    assert_eq!(d.threshold, 90.0);
    assert_eq!(d.models, Vec::<String>::new());
```

with:

```rust
    let d = Settings::default();
    assert_eq!(d.threshold, 90.0);
    assert_eq!(d.interval_seconds, 60);
    assert_eq!(d.cooldown_seconds, 300);
    assert_eq!(d.hysteresis_pct, 10.0);
    assert_eq!(d.strategy, Strategy::Best);
    assert!(!d.include_api_key_accounts);
    assert_eq!(d.unhealthy_ticks, 3);
    assert_eq!(d.models, Vec::<String>::new());
```

In `crates/tagteam-engine/tests/settings.rs`, in `every_key_is_read_from_a_full_file`, the
file's first table, replace:

```rust
[autoswitch]
threshold = 75.5
models = ["Fable", "Opus"]
```

with:

```rust
[autoswitch]
threshold = 75.5
interval_seconds = 120
cooldown_seconds = 600
hysteresis_pct = 12.5
strategy = "consume-first"
include_api_key_accounts = true
unhealthy_ticks = 5
models = ["Fable", "Opus"]
```

In `crates/tagteam-engine/tests/settings.rs`, in the same test, the start of the expected
`Settings`, replace:

```rust
            threshold: 75.5,
            models: models(&["Fable", "Opus"]),
```

with:

```rust
            threshold: 75.5,
            interval_seconds: 120,
            cooldown_seconds: 600,
            hysteresis_pct: 12.5,
            strategy: Strategy::ConsumeFirst,
            include_api_key_accounts: true,
            unhealthy_ticks: 5,
            models: models(&["Fable", "Opus"]),
```

In `crates/tagteam-engine/tests/settings.rs`, in
`keys_this_milestone_does_not_read_are_ignored_without_a_warning` (two of its keys are read
now), replace:

```rust
        "default_provider = \"claude-code\"\n[autoswitch]\ninterval_seconds = 120\nstrategy = \"best\"\nfuture = true\n",
```

with:

```rust
        "default_provider = \"claude-code\"\n[autoswitch]\nfuture = true\n[run]\nshare_extra = [\"x\"]\n",
```

Append to the end of `crates/tagteam-engine/tests/settings.rs`:

```rust
#[test]
fn the_autoswitch_ranges_are_the_specs_table() {
    // §6.4. A file value outside its range falls back to the default (with a warning); a CLI
    // flag is clamped into the same range.
    assert_eq!(THRESHOLD_RANGE, 50.0..=99.9);
    assert_eq!(INTERVAL_SECONDS_RANGE, 15..=3600);
    assert_eq!(COOLDOWN_SECONDS_RANGE, 0..=86_400);
    assert_eq!(HYSTERESIS_PCT_RANGE, 0.0..=50.0);
    assert_eq!(UNHEALTHY_TICKS_RANGE, 1..=100);
}

#[test]
fn the_autoswitch_numbers_accept_the_ends_of_their_ranges() {
    for (text, seconds) in [("15", 15), ("3600", 3600), ("120", 120)] {
        let (s, w) = load(&format!("[autoswitch]\ninterval_seconds = {text}\n"));
        assert_eq!((s.interval_seconds, w.len()), (seconds, 0), "{text}: {w:?}");
    }
    for (text, seconds) in [("0", 0), ("86400", 86_400)] {
        let (s, w) = load(&format!("[autoswitch]\ncooldown_seconds = {text}\n"));
        assert_eq!((s.cooldown_seconds, w.len()), (seconds, 0), "{text}: {w:?}");
    }
    for (text, pct) in [("0", 0.0), ("0.0", 0.0), ("50", 50.0), ("12.5", 12.5)] {
        let (s, w) = load(&format!("[autoswitch]\nhysteresis_pct = {text}\n"));
        assert_eq!((s.hysteresis_pct, w.len()), (pct, 0), "{text}: {w:?}");
    }
    for (text, ticks) in [("1", 1), ("100", 100)] {
        let (s, w) = load(&format!("[autoswitch]\nunhealthy_ticks = {text}\n"));
        assert_eq!((s.unhealthy_ticks, w.len()), (ticks, 0), "{text}: {w:?}");
    }
}

#[test]
fn an_invalid_autoswitch_value_falls_back_to_its_default_with_one_warning_naming_the_key() {
    // §6.4: reads are forgiving. Whole seconds and ticks are integers only, as
    // `usage.history_retention_days` is; the percentage takes an integer or a float, as
    // `autoswitch.threshold` does.
    let cases: &[(&str, &[&str])] = &[
        (
            "interval_seconds",
            &["14", "3601", "0", "-60", "60.0", "\"60\"", "true", "nan"],
        ),
        (
            "cooldown_seconds",
            &["-1", "86401", "300.5", "\"300\"", "false"],
        ),
        (
            "hysteresis_pct",
            &["-0.1", "50.1", "100", "nan", "inf", "\"10\"", "true"],
        ),
        (
            "strategy",
            &[
                "\"Best\"",
                "\"consume_first\"",
                "\"consumefirst\"",
                "\"\"",
                "1",
                "true",
                "[\"best\"]",
            ],
        ),
        (
            "include_api_key_accounts",
            &[
                "\"Yes\"", "\"TRUE\"", "\"on\"", "\"y\"", "2", "-1", "1.0", "\"\"",
            ],
        ),
        (
            "unhealthy_ticks",
            &["0", "101", "-3", "3.0", "\"3\"", "4294967299"],
        ),
    ];
    for (key, texts) in cases {
        for text in *texts {
            let (settings, warnings) = load(&format!("[autoswitch]\n{key} = {text}\n"));
            assert_eq!(settings, Settings::default(), "{key} = {text}");
            assert_eq!(warnings.len(), 1, "{key} = {text}: {warnings:?}");
            assert!(
                warnings[0].contains(&format!("`autoswitch.{key}`")),
                "{warnings:?}"
            );
        }
    }
}

#[test]
fn each_autoswitch_warning_says_what_its_key_accepts() {
    for (line, expected) in [
        (
            "interval_seconds = 5",
            "`autoswitch.interval_seconds` must be a whole number of seconds from 15 to 3600 (ignored)",
        ),
        (
            "cooldown_seconds = -5",
            "`autoswitch.cooldown_seconds` must be a whole number of seconds from 0 to 86400 (ignored)",
        ),
        (
            "hysteresis_pct = 60",
            "`autoswitch.hysteresis_pct` must be a number from 0 to 50 (ignored)",
        ),
        (
            "strategy = \"worst\"",
            "`autoswitch.strategy` must be \"best\" or \"consume-first\" (ignored)",
        ),
        (
            "include_api_key_accounts = \"maybe\"",
            "`autoswitch.include_api_key_accounts` must be true, false, 1, 0, yes or no (ignored)",
        ),
        (
            "unhealthy_ticks = 0",
            "`autoswitch.unhealthy_ticks` must be a whole number of ticks from 1 to 100 (ignored)",
        ),
    ] {
        let (_, warnings) = load(&format!("[autoswitch]\n{line}\n"));
        assert_eq!(warnings.len(), 1, "{line}: {warnings:?}");
        assert!(warnings[0].ends_with(expected), "{line}: {warnings:?}");
    }
}

#[test]
fn a_boolean_reads_only_true_false_one_zero_yes_and_no() {
    // §6.4. In the file: a TOML boolean, the integers 1 and 0, or one of the six words as a
    // string, in lower case. `parse_bool` is the words alone, for a flag's value.
    for (text, expected) in [
        ("true", true),
        ("false", false),
        ("1", true),
        ("0", false),
        ("\"true\"", true),
        ("\"false\"", false),
        ("\"1\"", true),
        ("\"0\"", false),
        ("\"yes\"", true),
        ("\"no\"", false),
    ] {
        let (settings, warnings) = load(&format!(
            "[autoswitch]\ninclude_api_key_accounts = {text}\n"
        ));
        assert_eq!(settings.include_api_key_accounts, expected, "{text}");
        assert!(warnings.is_empty(), "{text}: {warnings:?}");
    }
    for (word, expected) in [
        ("true", Some(true)),
        ("false", Some(false)),
        ("1", Some(true)),
        ("0", Some(false)),
        ("yes", Some(true)),
        ("no", Some(false)),
        ("Yes", None),
        ("TRUE", None),
        ("on", None),
        ("off", None),
        ("y", None),
        ("", None),
        (" yes", None),
    ] {
        assert_eq!(parse_bool(word), expected, "{word:?}");
    }
}

#[test]
fn the_strategy_reads_best_and_consume_first_by_their_names() {
    assert_eq!(Strategy::Best.as_str(), "best");
    assert_eq!(Strategy::ConsumeFirst.as_str(), "consume-first");
    for strategy in [Strategy::Best, Strategy::ConsumeFirst] {
        assert_eq!(Strategy::parse(strategy.as_str()), Some(strategy));
        let (settings, warnings) = load(&format!(
            "[autoswitch]\nstrategy = \"{}\"\n",
            strategy.as_str()
        ));
        assert_eq!(settings.strategy, strategy);
        assert!(warnings.is_empty(), "{warnings:?}");
    }
    for name in ["Best", "consume_first", "", " best"] {
        assert_eq!(Strategy::parse(name), None, "{name:?}");
    }
}

#[test]
fn a_providers_autoswitch_table_comes_first_for_every_key() {
    let global = "[autoswitch]\ninterval_seconds = 120\ncooldown_seconds = 600\nhysteresis_pct = 5\nstrategy = \"consume-first\"\ninclude_api_key_accounts = true\nunhealthy_ticks = 5\n";
    let (settings, warnings) = load(&format!(
        "{global}\n[provider.claude-code.autoswitch]\ninterval_seconds = 30\ncooldown_seconds = 0\nhysteresis_pct = 20\nstrategy = \"best\"\ninclude_api_key_accounts = false\nunhealthy_ticks = 1\n"
    ));
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        (
            settings.interval_seconds,
            settings.cooldown_seconds,
            settings.hysteresis_pct,
            settings.strategy,
            settings.include_api_key_accounts,
            settings.unhealthy_ticks
        ),
        (30, 0, 20.0, Strategy::Best, false, 1)
    );

    // An invalid override warns, naming the provider's key, and the global value applies.
    let text = format!(
        "{global}\n[provider.claude-code.autoswitch]\ninterval_seconds = 1\nstrategy = \"worst\"\n"
    );
    let (settings, warnings) = load(&text);
    assert_eq!(
        (settings.interval_seconds, settings.strategy),
        (120, Strategy::ConsumeFirst)
    );
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    for key in ["interval_seconds", "strategy"] {
        let named = format!("`provider.claude-code.autoswitch.{key}`");
        assert!(warnings.iter().any(|w| w.contains(&named)), "{warnings:?}");
    }
    // Another provider reads only the global table.
    let (other, warnings) = load_as(&text, "fake-agent");
    assert_eq!(
        (other.interval_seconds, other.strategy),
        (120, Strategy::ConsumeFirst)
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn the_mtime_is_the_settings_files_and_none_without_one() {
    // §11.4: a running `auto` re-reads the file whenever this changes.
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    assert_eq!(Settings::mtime(&env), None, "no file");
    fs::create_dir_all(env.config_dir()).unwrap();
    let path = env.config_dir().join("config.toml");
    fs::write(&path, "[autoswitch]\nthreshold = 80\n").unwrap();
    let first = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000);
    let touch = |at: SystemTime| {
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(at)
            .unwrap()
    };
    touch(first);
    assert_eq!(Settings::mtime(&env), Some(first));
    touch(first + Duration::from_secs(1));
    assert_eq!(
        Settings::mtime(&env),
        Some(first + Duration::from_secs(1)),
        "a write moves it"
    );
    fs::remove_file(&path).unwrap();
    assert_eq!(Settings::mtime(&env), None, "a removed file has none");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --test settings`
Expected: FAIL to compile. Among the errors: `unresolved imports
tagteam_engine::settings::COOLDOWN_SECONDS_RANGE` (and the other four ranges and `parse_bool`),
`struct Settings has no field named interval_seconds` (and `cooldown_seconds`, `hysteresis_pct`,
`strategy`, `include_api_key_accounts`, `unhealthy_ticks`), `no field interval_seconds on type
Settings`, and `no function or associated item named mtime found for struct Settings`.

- [ ] **Step 3: Read the six keys and the mtime**

In `crates/tagteam-engine/src/settings.rs` (the module doc, the imports and the constants),
replace:

```rust
//! Read-only `config.toml` (§6.4): the keys M2b consumes. `tagteam config` and its writes land
//! in M5; this module only reads, and never fails: a missing file, a corrupt file and an invalid
//! value each fall back to the default, the last two with a warning for the caller to print.
//! Every warning names the full path of the settings file.

use std::io::ErrorKind;

use tagteam_core::ProviderId;
use tagteam_provider::Env;
use toml_edit::{DocumentMut, Item, TableLike};

pub const DEFAULT_THRESHOLD: f64 = 90.0;
pub const DEFAULT_HISTORY_RETENTION_DAYS: u32 = 180;
pub const DEFAULT_STATUSLINE_FORMAT: &str = "{account} · 5h {5h}% · 7d {7d}%{stale}";
```

with:

```rust
//! Read-only `config.toml` (§6.4): the keys M2b and M3 consume. `tagteam config` and its writes
//! land in M5; this module only reads, and never fails: a missing file, a corrupt file and an
//! invalid value each fall back to the default, the last two with a warning for the caller to
//! print. Every warning names the full path of the settings file.

use std::io::ErrorKind;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::time::SystemTime;

use tagteam_core::ProviderId;
use tagteam_core::autoswitch::Strategy;
use tagteam_provider::Env;
use toml_edit::{DocumentMut, Item, TableLike};

pub const DEFAULT_THRESHOLD: f64 = 90.0;
pub const DEFAULT_INTERVAL_SECONDS: i64 = 60;
pub const DEFAULT_COOLDOWN_SECONDS: i64 = 300;
pub const DEFAULT_HYSTERESIS_PCT: f64 = 10.0;
pub const DEFAULT_UNHEALTHY_TICKS: u32 = 3;
pub const DEFAULT_HISTORY_RETENTION_DAYS: u32 = 180;
pub const DEFAULT_STATUSLINE_FORMAT: &str = "{account} · 5h {5h}% · 7d {7d}%{stale}";

/// §6.4's valid ranges. A value in the file outside its range falls back to the default, with a
/// warning; a CLI flag is clamped into it.
pub const THRESHOLD_RANGE: RangeInclusive<f64> = 50.0..=99.9;
pub const INTERVAL_SECONDS_RANGE: RangeInclusive<i64> = 15..=3600;
pub const COOLDOWN_SECONDS_RANGE: RangeInclusive<i64> = 0..=86_400;
pub const HYSTERESIS_PCT_RANGE: RangeInclusive<f64> = 0.0..=50.0;
pub const UNHEALTHY_TICKS_RANGE: RangeInclusive<u32> = 1..=100;
```

In `crates/tagteam-engine/src/settings.rs` (the head of `Settings`), replace:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// `autoswitch.threshold`: 50–99.9. The provider's own table first.
    pub threshold: f64,
    /// `autoswitch.models`: model display names, or `all`. The provider's own table first.
    pub models: Vec<String>,
```

with:

```rust
/// Every `autoswitch.*` key is read from the provider's own table first, then from
/// `[autoswitch]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// `autoswitch.threshold`: 50–99.9.
    pub threshold: f64,
    /// `autoswitch.interval_seconds`: 15–3600.
    pub interval_seconds: i64,
    /// `autoswitch.cooldown_seconds`: 0–86400.
    pub cooldown_seconds: i64,
    /// `autoswitch.hysteresis_pct`: 0–50.
    pub hysteresis_pct: f64,
    /// `autoswitch.strategy`: `best` or `consume-first`.
    pub strategy: Strategy,
    /// `autoswitch.include_api_key_accounts`.
    pub include_api_key_accounts: bool,
    /// `autoswitch.unhealthy_ticks`: 1–100.
    pub unhealthy_ticks: u32,
    /// `autoswitch.models`: model display names, or `all`.
    pub models: Vec<String>,
```

In `crates/tagteam-engine/src/settings.rs`, in `impl Default for Settings`, replace:

```rust
            threshold: DEFAULT_THRESHOLD,
            models: Vec::new(),
```

with:

```rust
            threshold: DEFAULT_THRESHOLD,
            interval_seconds: DEFAULT_INTERVAL_SECONDS,
            cooldown_seconds: DEFAULT_COOLDOWN_SECONDS,
            hysteresis_pct: DEFAULT_HYSTERESIS_PCT,
            strategy: Strategy::Best,
            include_api_key_accounts: false,
            unhealthy_ticks: DEFAULT_UNHEALTHY_TICKS,
            models: Vec::new(),
```

In `crates/tagteam-engine/src/settings.rs`, in `Settings::load`, replace:

```rust
        let path = env.config_dir().join("config.toml");
```

with:

```rust
        let path = path(env);
```

In `crates/tagteam-engine/src/settings.rs`, at the end of `Settings::load` and its `impl` block,
replace:

```rust
                (Settings::default(), vec![warning])
            }
        }
    }
}
```

with:

```rust
                (Settings::default(), vec![warning])
            }
        }
    }

    /// `config.toml`'s modification time; `None` when the file is missing or its metadata
    /// cannot be read. A running `auto` re-reads the settings before a tick whenever this
    /// changes (§11.4). Read it before `load`, so a write landing between the two counts as
    /// one more change at the next check rather than going unseen.
    pub fn mtime(env: &Env) -> Option<SystemTime> {
        std::fs::metadata(path(env)).and_then(|m| m.modified()).ok()
    }
}

fn path(env: &Env) -> PathBuf {
    env.config_dir().join("config.toml")
}

/// §6.4's booleans: `true`, `false`, `1`, `0`, `yes` and `no`, exactly. The file also takes a
/// TOML boolean and the integers 1 and 0; a flag's value is the words alone.
pub fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}
```

In `crates/tagteam-engine/src/settings.rs`, in `from_document`, the reads of `threshold` and
`models`, replace:

```rust
    let threshold = reader
        .read(
            &[&provider_autoswitch[..], global_autoswitch],
            "threshold",
            "must be a number from 50 to 99.9",
            parse_threshold,
        )
        .unwrap_or(defaults.threshold);
    let models = reader
        .read(
            &[&provider_autoswitch[..], global_autoswitch],
```

with:

```rust
    let autoswitch: &[&[&str]] = &[&provider_autoswitch[..], global_autoswitch];

    let threshold = reader
        .read(
            autoswitch,
            "threshold",
            "must be a number from 50 to 99.9",
            |item| number(item).filter(|v| THRESHOLD_RANGE.contains(v)),
        )
        .unwrap_or(defaults.threshold);
    let interval_seconds = reader
        .read(
            autoswitch,
            "interval_seconds",
            "must be a whole number of seconds from 15 to 3600",
            |item| {
                item.as_integer()
                    .filter(|v| INTERVAL_SECONDS_RANGE.contains(v))
            },
        )
        .unwrap_or(defaults.interval_seconds);
    let cooldown_seconds = reader
        .read(
            autoswitch,
            "cooldown_seconds",
            "must be a whole number of seconds from 0 to 86400",
            |item| {
                item.as_integer()
                    .filter(|v| COOLDOWN_SECONDS_RANGE.contains(v))
            },
        )
        .unwrap_or(defaults.cooldown_seconds);
    let hysteresis_pct = reader
        .read(
            autoswitch,
            "hysteresis_pct",
            "must be a number from 0 to 50",
            |item| number(item).filter(|v| HYSTERESIS_PCT_RANGE.contains(v)),
        )
        .unwrap_or(defaults.hysteresis_pct);
    let strategy = reader
        .read(
            autoswitch,
            "strategy",
            "must be \"best\" or \"consume-first\"",
            |item| Strategy::parse(item.as_str()?),
        )
        .unwrap_or(defaults.strategy);
    let include_api_key_accounts = reader
        .read(
            autoswitch,
            "include_api_key_accounts",
            "must be true, false, 1, 0, yes or no",
            parse_bool_item,
        )
        .unwrap_or(defaults.include_api_key_accounts);
    let unhealthy_ticks = reader
        .read(
            autoswitch,
            "unhealthy_ticks",
            "must be a whole number of ticks from 1 to 100",
            |item| {
                u32::try_from(item.as_integer()?)
                    .ok()
                    .filter(|n| UNHEALTHY_TICKS_RANGE.contains(n))
            },
        )
        .unwrap_or(defaults.unhealthy_ticks);
    let models = reader
        .read(
            autoswitch,
```

In `crates/tagteam-engine/src/settings.rs`, at the end of `from_document`, replace:

```rust
    let settings = Settings {
        threshold,
        models,
```

with:

```rust
    let settings = Settings {
        threshold,
        interval_seconds,
        cooldown_seconds,
        hysteresis_pct,
        strategy,
        include_api_key_accounts,
        unhealthy_ticks,
        models,
```

In `crates/tagteam-engine/src/settings.rs` (`parse_threshold`, whose range check moved into the
read), replace:

```rust
fn parse_threshold(item: &Item) -> Option<f64> {
    let value = item
        .as_float()
        .or_else(|| item.as_integer().map(|i| i as f64))?;
    (50.0..=99.9).contains(&value).then_some(value)
}
```

with:

```rust
/// A float or an integer. A non-finite float is never in a range, so it is rejected there.
fn number(item: &Item) -> Option<f64> {
    item.as_float()
        .or_else(|| item.as_integer().map(|i| i as f64))
}

/// A TOML boolean, the integer 1 or 0, or one of `parse_bool`'s words.
fn parse_bool_item(item: &Item) -> Option<bool> {
    if let Some(b) = item.as_bool() {
        return Some(b);
    }
    match item.as_integer() {
        Some(1) => Some(true),
        Some(0) => Some(false),
        Some(_) => None,
        None => parse_bool(item.as_str()?),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --test settings`
Expected: PASS, 47 tests: the 8 new ones and the 3 changed ones among them.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. The tagteam lib's pseudo-terminal prompt tests need a real pty (Execution notes).

- [ ] **Step 5: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-core/src/autoswitch.rs crates/tagteam-core/src/lib.rs \
  crates/tagteam-engine/src/settings.rs crates/tagteam-engine/tests/settings.rs
git commit -m "Read every auto-switch setting and report the settings file's mtime"
```

---

### Task 2: `primary_long_window`

§4.5's trait sketch has `fn primary_long_window(&self) -> Option<&'static str>; // a window key;
CC: "7d"; ranks consume-first (§11.2)`, and: "Without a `primary_long_window`, consume-first is
unavailable for that provider", whose engine "run[s] `best` instead, with one
`config-warning`". §11.5: "`FakeAgent` has no `Long` window, so a `consume-first` setting runs
`best` there." This task adds the method; Tasks 4, 5 and 8 consume it through
`AutoConfig::long_window`.

**Readings of the spec this task commits to:**
- **A required method, not a default.** The sketch gives it no body, and §15.2 says "every trait
  method lands with its `FakeAgent` implementation". A default `None` would let a future
  provider lose consume-first without anyone deciding it. `rg -n 'impl Provider for' crates/`
  lists the only two implementations, `ClaudeCode` and `FakeAgent`; no test double implements
  the trait.
- **Claude Code's key is its normalizer's.** `usage::SEVEN_DAY` becomes `pub(crate)` and CC
  returns it. A test pins it to the one window `normalize` gives as `Long` and to
  `describe_window`'s description, so the key and the window kind cannot drift apart.
- **FakeAgent returns `None`, though its `monthly` meter is a `Long` window.** §15.2 says
  FakeAgent has "no `Long` window", but its normalizer has described `monthly` as `Long` since
  M2b, which gives it pace (§8.7). This task leaves that alone and offers no window for ranking,
  so FakeAgent still exercises §4.5's `best` fallback, which §11.5 relies on.

**Files:**
- Modify: `crates/tagteam-provider/src/provider.rs` (the trait)
- Modify: `crates/tagteam-cc/src/usage.rs` (`SEVEN_DAY` becomes `pub(crate)`)
- Modify: `crates/tagteam-cc/src/provider.rs`
- Modify: `crates/tagteam-fake/src/provider.rs`
- Modify: `crates/tagteam-cc/tests/provider.rs`
- Modify: `crates/tagteam-fake/tests/provider.rs`

**Interfaces:**
- Consumes: `tagteam_cc::usage::normalize`, `Provider::describe_window`, the recorded
  `usage-200.json` reply (`usage_body()` in CC's provider tests).
- Produces: `Provider::primary_long_window(&self) -> Option<&'static str>`, required. Claude
  Code: `Some("7d")`. FakeAgent: `None`.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-cc/tests/provider.rs` (a new test before the `keychain_login` helper),
replace:

```rust
/// A live OAuth login in the Keychain, as CC leaves it; returns the item's (service, account).
```

with:

```rust
#[test]
fn consume_first_ranks_on_the_key_claude_code_normalizes_as_its_long_window() {
    // §4.5: CC's primary long window is "7d". It is the key §8.2's normalization gives the
    // `Long` window, and the key `describe_window` describes as one, so the two cannot drift.
    let f = fx();
    assert_eq!(f.cc.primary_long_window(), Some("7d"));
    let key = f.cc.primary_long_window().unwrap();
    let long: Vec<String> = usage::normalize(&usage_body())
        .unwrap()
        .into_iter()
        .filter(|w| w.kind == WindowKind::Long)
        .map(|w| w.key)
        .collect();
    assert_eq!(long, [key]);
    assert_eq!(
        f.cc.describe_window(key).map(|w| (w.kind, w.period_s)),
        Some((WindowKind::Long, Some(604_800)))
    );
}

/// A live OAuth login in the Keychain, as CC leaves it; returns the item's (service, account).
```

Append to the end of `crates/tagteam-fake/tests/provider.rs`:

```rust
#[test]
fn fake_agent_offers_consume_first_no_long_window() {
    // §4.5, §11.5: without a primary long window a consume-first strategy runs `best`, and
    // FakeAgent is the provider that exercises that path. Its `monthly` meter is a `Long`
    // window for pace (§8.7), but it is not offered for ranking.
    let f = fx();
    assert_eq!(f.fake.primary_long_window(), None);
    assert_eq!(
        f.fake.describe_window("monthly").map(|w| w.kind),
        Some(WindowKind::Long)
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-cc --test provider`
Expected: FAIL to compile: `no method named primary_long_window found for struct ClaudeCode`
(twice).

Run: `cargo test -p tagteam-fake --test provider`
Expected: FAIL to compile: `no method named primary_long_window found for struct FakeAgent`.

- [ ] **Step 3: Add the method and both implementations**

In `crates/tagteam-provider/src/provider.rs`, in `pub trait Provider`, replace:

```rust
    fn kind_traits(&self, kind: &str) -> KindTraits;
    fn parse_identity(&self, raw: &Value) -> Result<Identity, ProviderError>;
```

with:

```rust
    fn kind_traits(&self, kind: &str) -> KindTraits;
    /// The window consume-first ranks on (§4.5, §11.2 step 8): the key of a `Long` window this
    /// provider's normalization produces (§8.2). `None` when it offers none; a consume-first
    /// strategy then runs `best` for this provider.
    fn primary_long_window(&self) -> Option<&'static str>;
    fn parse_identity(&self, raw: &Value) -> Result<Identity, ProviderError>;
```

In `crates/tagteam-cc/src/usage.rs`, replace:

```rust
const FIVE_HOUR: &str = "5h";
const SEVEN_DAY: &str = "7d";
```

with:

```rust
const FIVE_HOUR: &str = "5h";
/// The `Long` window, which consume-first ranks on (`Provider::primary_long_window`).
pub(crate) const SEVEN_DAY: &str = "7d";
```

In `crates/tagteam-cc/src/provider.rs`, in `impl Provider for ClaudeCode`, replace:

```rust
    fn kind_traits(&self, kind: &str) -> KindTraits {
        shape::kind_traits(kind)
    }

    fn parse_identity(&self, raw: &Value) -> Result<Identity, ProviderError> {
        shape::identity_from_oauth_account(raw).ok_or_else(|| {
```

with:

```rust
    fn kind_traits(&self, kind: &str) -> KindTraits {
        shape::kind_traits(kind)
    }

    fn primary_long_window(&self) -> Option<&'static str> {
        Some(usage::SEVEN_DAY)
    }

    fn parse_identity(&self, raw: &Value) -> Result<Identity, ProviderError> {
        shape::identity_from_oauth_account(raw).ok_or_else(|| {
```

In `crates/tagteam-fake/src/provider.rs`, in `impl Provider for FakeAgent`, replace:

```rust
    fn kind_traits(&self, kind: &str) -> KindTraits {
        shape::kind_traits(kind)
    }

    fn parse_identity(&self, raw: &Value) -> Result<Identity, ProviderError> {
        shape::identity_from(raw).ok_or_else(|| {
```

with:

```rust
    fn kind_traits(&self, kind: &str) -> KindTraits {
        shape::kind_traits(kind)
    }

    /// None on purpose: FakeAgent is the provider whose consume-first setting runs `best`
    /// (§4.5, §11.5). Its `monthly` meter is still a `Long` window for pace (§8.7).
    fn primary_long_window(&self) -> Option<&'static str> {
        None
    }

    fn parse_identity(&self, raw: &Value) -> Result<Identity, ProviderError> {
        shape::identity_from(raw).ok_or_else(|| {
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-cc --test provider`
Expected: PASS (35 tests), including
`consume_first_ranks_on_the_key_claude_code_normalizes_as_its_long_window`.

Run: `cargo test -p tagteam-fake --test provider`
Expected: PASS (14 tests), including `fake_agent_offers_consume_first_no_long_window`.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. The tagteam lib's pseudo-terminal prompt tests need a real pty.

- [ ] **Step 5: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider/src/provider.rs crates/tagteam-cc/src/usage.rs \
  crates/tagteam-cc/src/provider.rs crates/tagteam-fake/src/provider.rs \
  crates/tagteam-cc/tests/provider.rs crates/tagteam-fake/tests/provider.rs
git commit -m "Name the window consume-first ranks on in the provider trait"
```

---

### Task 3: Scheduled and re-check collection

§8.3's reserve has a rule for each caller: on demand ("older than 180 s *and* either a poll to be
due or no plan to exist"), scheduled ("a poll to be due, or no reading yet") and re-check ("only
the reading to be older than 180 s; it ignores the plan. Quarantine, backoff, the lease and the
budget still apply"). Only `CollectMode::OnDemand` exists today; `reserve_usage(…, on_demand:
bool, …)` already has the scheduled rule as its `false` arm, used by test helpers alone. §8.6's
"Auto-switch scheduling" is O(1) per tick, in two phases: "**The active account**, if it is
due", then "**Candidates**, chosen from the store as phase 1 left it: the single stalest due
candidate (never fetched first, then the oldest `fetched_at`, ties to the lower position). The
tick **escalates** to every due candidate when the active account's max relevant pct is within
15 points of the threshold (≥ threshold − 15), or its headroom is still unknown (§8.4)." §8.3
"Who collects": "An auto tick collects as scheduled (§8.6), and consume-first re-checks before
switching (§11.2 step 8). Its fetches run in parallel like `list`'s."

This task adds the pure pick and escalation to `tagteam-core`, `CollectMode::Scheduled` and
`CollectMode::Recheck` to the collector, and what the tick (Task 8) needs from a collection to
report `account-quarantined`. It also pins M3a's two untested cancellation points.

**Readings of the spec this task commits to:**
- **The reserve's rule is an enum.** `reserve_usage` takes `Eligibility::{OnDemand, Scheduled,
  Recheck}` instead of `on_demand: bool`, and `Eligibility::allows` holds the schedule half of
  all three, so the phase 2 pick and the reservation cannot disagree about what is due.
  Quarantine, backoff and the lease are checked first, and the budget last, for every caller,
  as today.
- **A re-check reading must be older than 180 s**: `now − fetched_at > floor_s`, as on demand,
  so a reading exactly 180 s old is not re-checked. A reading stamped more than 60 s ahead
  counts as none (§8.4).
- **"Due" for the pick is the reservation's answer without the lease and the budget**: not in
  backoff, and a poll due or no reading yet. The backoff must be part of it: otherwise a
  never-read account in backoff is the stalest at every tick and starves every other candidate
  for as long as 4500 s. The lease (at most 90 s, while another process reads the account) and
  the budget (a refusal backs the account off and moves its `next_poll_at`, so it is not due
  at the next tick) are left to the reservation.
- **The candidates** are the provider's accounts other than the live one that the switch's own
  `is_candidate` accepts (enabled, not quarantined, with an identity: §9.3's switchable, whose
  vault is read only when collected) and whose kind has usage. A managed key has none (§13.2)
  and would otherwise be a never-read candidate forever.
- **Escalation reads the live account's decision-grade reading** (`Engine::decision_windows`,
  §8.4) under the tick's `models`, after phase 1 has recorded. A reading that is not
  decision-grade, or a reading with no relevant window, is unknown headroom and escalates. With
  no managed live login there is no headroom either, so phase 2 escalates; a tick never
  collects then (§11.2 step 2 stops it first), and a test pins it anyway.
- **The tick's `threshold` and `models` drive everything its collection does.** Both
  `Scheduled` and `Recheck` carry them (flags over the file, reloaded by Task 9): they decide
  escalation, and the plan recorded after each fetch (`plan_after_fetch`'s urgent band and
  relevant windows) is made for them. `OnDemand` keeps the engine's `Settings`. A private
  `Policy` carries the rule, threshold and models into every account's collection;
  `a_ticks_collection_plans_for_the_ticks_threshold_not_the_settings` pins both modes against
  the settings' 90.
- **The report's order:** the live account first, then the picks in pick order; escalated picks
  come stalest first.
- **Quarantines during a collection are reported.** `collect_one` reads the account again after
  its record: a quarantined account is never reserved, so a quarantine set by then was set
  while this collection ran (the gate's or §7.5's Dead verdict or identity conflict, or a lost
  successor). One that another process set in that window is named too; Task 8 merges these
  with its quarantine diff (Decision 10) so nothing is reported twice.
- **A signal during phase 1 starts no phase 2** (§14.1: nothing is reserved once the token is
  set).
- **A lease outlives its fetch.** A lease is left to expire after the record (§8.3), so a tick
  less than 90 s after an account's fetch finds it `Ineligible(Leased)`, with a reading seconds
  old. Tests that need `NotDue` leave 100 s between ticks.
- **The two cancellation points M3a left untested** (Decision 12). The gate takes its account
  lock with a try, which never looks at the token, so the check at `gate`'s entry is all that
  stops a token request after a signal. The check at `refresh_live`'s entry is all that stops
  two things: a live kind that cannot refresh, whose expired token would otherwise be recorded
  as a `token-expired` failure, and a refused live token still valid locally, for which §7.5
  first asks the profile oracle (§7.6), a request. For a refreshable expired token the mutation
  lock's wait in §7.5 already stops it, so that case pins nothing. Step 8 shows each test fails
  once its check is deleted.

**Files:**
- Modify: `crates/tagteam-core/src/poll.rs` (`DueCandidate`, `scheduled_pick`, `escalates` and
  their tests)
- Modify: `crates/tagteam-engine/src/store/usage.rs` (`Eligibility`, `backoff_holds`,
  `reserve_usage`)
- Modify: `crates/tagteam-engine/src/store/mod.rs` (re-exports)
- Modify: `crates/tagteam-engine/src/collect.rs`
- Modify: `crates/tagteam-engine/src/switch.rs` (`is_candidate` becomes `pub(crate)`)
- Modify: `crates/tagteam-engine/tests/store_usage.rs`, `tests/collect.rs`,
  `tests/collect_active.rs`
- Modify, for the new `reserve_usage` argument (`rg -n 'reserve_usage\(' crates/` lists every
  caller): `crates/tagteam-engine/tests/strategy.rs`, `crates/tagteam-engine/tests/views_usage.rs`,
  `crates/tagteam/src/statusline.rs` (its tests), `crates/tagteam/tests/common/mod.rs`,
  `crates/tagteam/tests/usage_cli.rs`

**Interfaces:**
- Consumes: `Store::{reserve_usage, usage_state, account, accounts, active}`,
  `Engine::{decision_windows, provider, settings, check_cancel, live_accounts}`,
  `switch::is_candidate`,
  `store::{backoff_is_skewed, plan_is_skewed}`, `trust::is_future_stamped`,
  `usage::max_relevant_pct`, `PollBudget::{floor_s, escalation_margin}`; in tests the engine
  fixtures `Fx`, `due`, `spend_budget`, `Rendezvous`, `signal_at`, `live_setup_token`, `refuse`
  and `oracle::HttpOracle`.
- Produces:
  - `tagteam_core::poll::{DueCandidate, scheduled_pick, escalates}`, as the Interface Contract
    states; `scheduled_pick` returns escalated picks stalest first.
  - `CollectMode::{OnDemand { accounts }, Scheduled { provider, threshold, models }, Recheck {
    accounts, threshold, models }}`. `Recheck` carries the tick's threshold and models too
    (the Interface Contract's `Recheck { accounts }` amended). It derives `Debug, Clone,
    PartialEq`: the `f64` rules out `Eq`.
  - `CollectReport.quarantined: Vec<AccountId>` (new): the accounts quarantined while their
    collection ran, in `outcomes` order. `outcomes` for `Scheduled`: the live account first,
    when it is managed, then each pick in pick order. For `poll`'s `fetchErrors` (Task 8):
    `Collected::Failed { kind }` is a recorded failure whose `last_error` token is `kind`,
    except `kind == "error"` (the collection itself failed, nothing was recorded, and a warning
    says why); `Collected::OverBudget { .. }` was recorded as `over-budget`.
  - `tagteam_engine::store::Eligibility::{OnDemand, Scheduled, Recheck}`, and
    `Store::reserve_usage(&self, account: &AccountRow, now_ms: i64, eligibility: Eligibility,
    budget: &PollBudget) -> Result<Reserve, StoreError>` (it took `on_demand: bool`)
  - crate-private: `Eligibility::allows`, `store::backoff_holds`, `switch::is_candidate`

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-core/src/poll.rs`, at the end of `mod tests`, replace:

```rust
    #[test]
    fn a_row_stamped_in_the_future_still_counts() {
        let mut counted = ascending(19, NOW - 5);
        counted.push(NOW + 100);
        assert_eq!(budget_next_free(&B, &counted, NOW), Some(counted[0] + 3660));
    }
}
```

with:

```rust
    #[test]
    fn a_row_stamped_in_the_future_still_counts() {
        let mut counted = ascending(19, NOW - 5);
        counted.push(NOW + 100);
        assert_eq!(budget_next_free(&B, &counted, NOW), Some(counted[0] + 3660));
    }

    fn cand(position: u32, due: bool, fetched_at: Option<i64>) -> DueCandidate {
        DueCandidate {
            position,
            due,
            fetched_at,
        }
    }

    #[test]
    fn the_scheduled_pick_takes_a_candidate_never_fetched_first() {
        let cands = [
            cand(1, true, Some(NOW - 5_000)),
            cand(2, true, None),
            cand(3, true, Some(NOW - 9_000)),
        ];
        assert_eq!(scheduled_pick(&cands, false), [2]);
    }

    #[test]
    fn the_scheduled_pick_takes_the_oldest_reading_and_a_tie_goes_to_the_lower_position() {
        let cands = [
            cand(3, true, Some(NOW - 900)),
            cand(1, true, Some(NOW - 600)),
            cand(2, true, Some(NOW - 900)),
        ];
        assert_eq!(scheduled_pick(&cands, false), [2]);
        let never = [cand(4, true, None), cand(2, true, None)];
        assert_eq!(scheduled_pick(&never, false), [2], "never fetched ties too");
    }

    #[test]
    fn a_candidate_that_is_not_due_is_never_picked() {
        let cands = [
            cand(1, false, None),
            cand(2, false, Some(NOW - 9_000)),
            cand(3, true, Some(NOW - 10)),
        ];
        assert_eq!(scheduled_pick(&cands, false), [3]);
        for escalate in [false, true] {
            assert!(scheduled_pick(&cands[..2], escalate).is_empty());
            assert!(scheduled_pick(&[], escalate).is_empty());
        }
    }

    #[test]
    fn escalation_takes_every_due_candidate_stalest_first() {
        let cands = [
            cand(1, true, Some(NOW - 600)),
            cand(2, false, None),
            cand(3, true, None),
            cand(4, true, Some(NOW - 900)),
        ];
        assert_eq!(scheduled_pick(&cands, true), [3, 4, 1]);
    }

    #[test]
    fn escalation_starts_at_exactly_the_margin_below_the_threshold() {
        // §8.6: within 15 points of the threshold, that is ≥ threshold − 15.
        let m = B.escalation_margin;
        assert_eq!(m, 15.0);
        assert!(escalates(Some(75.0), 90.0, m));
        assert!(!escalates(Some(74.99), 90.0, m));
        assert!(escalates(Some(77.0), 92.0, m));
        assert!(!escalates(Some(77.0), 92.5, m));
        assert!(escalates(Some(120.0), 90.0, m), "past the limit");
        assert!(!escalates(Some(0.0), 50.0, m));
    }

    #[test]
    fn an_unknown_active_headroom_escalates() {
        // §8.6: "or its headroom is still unknown". A non-finite pct is unknown (§8.2).
        for pct in [
            None,
            Some(f64::NAN),
            Some(f64::INFINITY),
            Some(f64::NEG_INFINITY),
        ] {
            assert!(escalates(pct, 90.0, 15.0), "{pct:?}");
        }
    }
}
```

In `crates/tagteam-engine/tests/store_usage.rs`, replace:

```rust
use tagteam_engine::store::{
    Ineligible, LiveIdentityCacheRow, Reservation, Reserve, SendGrant, Slot, Store, StoreError,
    UsageStateRow,
};
```

with:

```rust
use tagteam_engine::store::{
    Eligibility, Ineligible, LiveIdentityCacheRow, Reservation, Reserve, SendGrant, Slot, Store,
    StoreError, UsageStateRow,
};
```

In `crates/tagteam-engine/tests/store_usage.rs` (the `reserve` helper, which now names the
rule), replace:

```rust
fn reserve(s: &Store, id: &AccountId, now_ms: i64, on_demand: bool) -> Reserve {
    let row = s.account(id).unwrap().unwrap();
    s.reserve_usage(&row, now_ms, on_demand, &B).unwrap()
}
```

with:

```rust
fn reserve(s: &Store, id: &AccountId, now_ms: i64, on_demand: bool) -> Reserve {
    let eligibility = if on_demand {
        Eligibility::OnDemand
    } else {
        Eligibility::Scheduled
    };
    reserve_as(s, id, now_ms, eligibility)
}

fn reserve_as(s: &Store, id: &AccountId, now_ms: i64, eligibility: Eligibility) -> Reserve {
    let row = s.account(id).unwrap().unwrap();
    s.reserve_usage(&row, now_ms, eligibility, &B).unwrap()
}
```

In `crates/tagteam-engine/tests/store_usage.rs`, in
`two_stores_racing_for_one_account_reserve_it_once`, replace:

```rust
                match s.reserve_usage(row, T_MS, true, &B).unwrap() {
```

with:

```rust
                match s
                    .reserve_usage(row, T_MS, Eligibility::OnDemand, &B)
                    .unwrap()
                {
```

In `crates/tagteam-engine/tests/store_usage.rs` (two new tests before
`over_budget_moves_the_plan_and_takes_no_lease`), replace:

```rust
#[test]
fn over_budget_moves_the_plan_and_takes_no_lease() {
```

with:

```rust
#[test]
fn a_recheck_needs_only_a_reading_older_than_the_floor() {
    // §8.3: consume-first's re-check (§11.2 step 8) ignores the plan. 180 s is not older than
    // 180 s, and a reading stamped more than 60 s ahead counts as none (§8.4).
    let (_d, path, s) = open();
    let cases = [
        // (fetched_at, next_poll_at, eligible)
        (Some(T - 180), Some(T + 500), false),
        (Some(T - 181), Some(T + 500), true),
        (None, Some(T + 500), true),
        (Some(T + 60), None, false),
        (Some(T + 61), Some(T + 500), true),
    ];
    for (i, (fetched_at, next_poll_at, eligible)) in cases.into_iter().enumerate() {
        let n = i as u32 + 1;
        let id = add(&s, &cc(), &format!("a{n}"), &format!("a{n}@x.co"), n);
        arrange(&path, &id, fetched_at, None, next_poll_at);
        let got = reserve_as(&s, &id, T_MS, Eligibility::Recheck);
        if eligible {
            assert!(matches!(got, Reserve::Reserved(_)), "case {i}: {got:?}");
        } else {
            assert_eq!(got, Reserve::Ineligible(Ineligible::NotDue), "case {i}");
        }
    }
}

#[test]
fn a_recheck_still_honours_quarantine_backoff_the_lease_and_the_budget() {
    // §8.3: "Quarantine, backoff, the lease and the budget still apply." Each account's
    // reading is old and it has no plan, so only the named rule can refuse it.
    let (_d, path, s) = open();
    let old = Some(T - 1_000);
    let q = add(&s, &cc(), "q", "q@x.co", 1);
    arrange(&path, &q, old, None, None);
    s.set_quarantine(&q, "invalid_grant", "sha256:sent", 1)
        .unwrap();
    assert_eq!(
        reserve_as(&s, &q, T_MS, Eligibility::Recheck),
        Reserve::Ineligible(Ineligible::Quarantined)
    );
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    arrange(&path, &b, old, Some(T + 1), None);
    assert_eq!(
        reserve_as(&s, &b, T_MS, Eligibility::Recheck),
        Reserve::Ineligible(Ineligible::Backoff)
    );
    let l = add(&s, &cc(), "l", "l@x.co", 3);
    arrange(&path, &l, old, None, None);
    raw(&path)
        .execute(
            "INSERT INTO leases (name, holder, expires_at) VALUES ('usage:l', 'other', ?1)",
            [T_MS + 1],
        )
        .unwrap();
    assert_eq!(
        reserve_as(&s, &l, T_MS, Eligibility::Recheck),
        Reserve::Ineligible(Ineligible::Leased)
    );
    let o = add(&s, &cc(), "o", "o@x.co", 4);
    arrange(&path, &o, old, None, None);
    for _ in 0..20 {
        raw(&path)
            .execute(
                "INSERT INTO usage_requests (provider, identity_key, at) VALUES (?1, ?2, ?3)",
                params![cc().as_str(), "o@x.co\n", T - 100],
            )
            .unwrap();
    }
    assert_eq!(
        reserve_as(&s, &o, T_MS, Eligibility::Recheck),
        Reserve::OverBudget {
            next_free_at: T - 100 + B.count_window_s
        }
    );
}

#[test]
fn over_budget_moves_the_plan_and_takes_no_lease() {
```

In `crates/tagteam-engine/tests/collect.rs` (the module doc), replace:

```rust
//! §8.1 and §8.3: the collector's reserve, token, fetch and record phases for inactive
//! accounts (and the live token, read but never refreshed by a fetch), the hourly budget across
//! processes (§8.6, Review Focus 1 and 2), and provider neutrality (§15.2).
```

with:

```rust
//! §8.1 and §8.3: the collector's reserve, token, fetch and record phases for inactive
//! accounts (and the live token, read but never refreshed by a fetch), the hourly budget across
//! processes (§8.6, Review Focus 1 and 2), provider neutrality (§15.2), and an auto tick's
//! scheduled collection and consume-first's re-check (§8.3, §8.6).
```

In `crates/tagteam-engine/tests/collect.rs`, replace:

```rust
use tagteam_core::{AccountId, WindowKind};
```

with:

```rust
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId, WindowKind};
```

In `crates/tagteam-engine/tests/collect.rs` (new helpers before the first test), replace:

```rust
#[test]
fn an_inactive_account_is_fetched_with_its_stored_token_and_recorded() {
```

with:

```rust
/// An auto tick's scheduled collection (§8.6) of the fixture's provider through `engine`, at
/// `threshold` with `models` relevant.
fn scheduled_on(engine: &Engine, threshold: f64, models: &[&str]) -> CollectReport {
    engine
        .collect_usage(CollectMode::Scheduled {
            provider: ProviderId::new(CLAUDE_CODE),
            threshold,
            models: models.iter().map(|m| (*m).to_owned()).collect(),
        })
        .unwrap()
}

/// `scheduled_on` the fixture's engine, with no model's window relevant.
fn scheduled(fx: &Fx, threshold: f64) -> CollectReport {
    scheduled_on(&fx.engine, threshold, &[])
}

/// `a`, `b` and `c` at positions 1 to 3; `c` is the live login.
fn three_accounts(fx: &Fx) -> [AccountId; 3] {
    [
        fx.add("a@x.co", "rt-a"),
        fx.add("b@x.co", "rt-b"),
        fx.add("c@x.co", "rt-c"),
    ]
}

/// Backs `id` off until `until`, directly, as a recorded failure would.
fn back_off(fx: &Fx, id: &AccountId, until: i64) {
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "INSERT INTO usage_state (account_id, backoff_until) VALUES (?1, ?2) \
             ON CONFLICT(account_id) DO UPDATE SET backoff_until = excluded.backoff_until",
            rusqlite::params![id.as_str(), until],
        )
        .unwrap();
}

#[test]
fn an_inactive_account_is_fetched_with_its_stored_token_and_recorded() {
```

In `crates/tagteam-engine/tests/collect.rs` (new tests before `mod hooks`), replace:

```rust
#[cfg(feature = "test-hooks")]
mod hooks {
```

with:

```rust
#[test]
fn a_scheduled_tick_reads_the_due_live_account_then_the_stalest_due_candidate() {
    // §8.6: phase 1 the live account if it is due; phase 2, from the store as phase 1 left it,
    // the single stalest due candidate. The live account's 77 % is below 99.9 − 15, so once
    // phase 1 has read it the tick does not escalate.
    let fx = Fx::new();
    let [a, b, c] = three_accounts(&fx);
    fx.script_usage(200, usage_fixture());

    let report = scheduled(&fx, 99.9);

    assert_eq!(
        report.outcomes,
        [
            (c.clone(), Collected::Recorded),
            (a.clone(), Collected::Recorded)
        ]
    );
    assert_eq!(
        usage_bearers(&fx),
        ["at-rt-c", "at-rt-a"],
        "phase 1, then phase 2"
    );
    assert_eq!(fx.usage_state(&b), None, "one candidate a tick");
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(report.quarantined.is_empty());

    // 100 s on, the lease of the live account's fetch (90 s) has expired but its plan (243 s
    // at the soonest) is not due, and its reading still decides; b, never read, is the
    // stalest.
    fx.clock.advance_ms(100_000);
    let report = scheduled(&fx, 99.9);
    assert_eq!(
        report.outcomes,
        [
            (c.clone(), Collected::Ineligible(Ineligible::NotDue)),
            (b.clone(), Collected::Recorded)
        ]
    );

    // 1000 s in, every plan has come due, and a's reading is older than b's.
    fx.clock.advance_ms(900_000);
    let report = scheduled(&fx, 99.9);
    assert_eq!(
        report.outcomes,
        [(c, Collected::Recorded), (a, Collected::Recorded)]
    );
}

#[test]
fn a_candidate_in_backoff_is_not_due_so_it_cannot_starve_the_others() {
    // §8.3: a scheduled reservation refuses an account in backoff. Were it due for the pick, a
    // never-read account in backoff would be the stalest at every tick until its backoff
    // lifted, and no other candidate would be read meanwhile.
    let fx = Fx::new();
    let [a, b, c] = three_accounts(&fx);
    back_off(&fx, &a, NOW_S + 600);
    fx.script_usage(200, usage_fixture());

    let report = scheduled(&fx, 99.9);

    assert_eq!(
        report.outcomes,
        [(c, Collected::Recorded), (b, Collected::Recorded)]
    );
}

#[test]
fn only_switchable_accounts_with_usage_are_candidates() {
    // §8.6: the candidates are the provider's switchable accounts other than the live one
    // (§9.3), never a quarantined or disabled one; a managed API key has no usage (§13.2). The
    // tick escalates (77 % ≥ 50 − 15), so it reads every candidate that is due.
    let fx = Fx::new();
    let q = fx.add("q@x.co", "rt-q");
    let d = fx.add("d@x.co", "rt-d");
    let ok = fx.add("ok@x.co", "rt-ok");
    let c = fx.add("c@x.co", "rt-c"); // live
    fx.add_api_key(API_KEY);
    fx.quarantine(&q, "invalid_grant", "sha256:sent");
    fx.engine.set_disabled(&d, true).unwrap();
    fx.script_usage(200, usage_fixture());

    let report = scheduled(&fx, 50.0);

    assert_eq!(
        report.outcomes,
        [(c, Collected::Recorded), (ok, Collected::Recorded)]
    );
    assert_eq!(usage_bearers(&fx), ["at-rt-c", "at-rt-ok"]);
}

#[test]
fn a_tick_escalates_to_every_due_candidate_from_exactly_fifteen_points_below_its_threshold() {
    // §8.6: the live account's 77 % is exactly 92 − 15, and below 92.5 − 15. The tick's own
    // threshold decides, not the settings' 90.
    for (threshold, escalated) in [(92.0, true), (92.5, false)] {
        let fx = Fx::new();
        let [a, b, c] = three_accounts(&fx);
        fx.script_usage(200, usage_fixture());

        let report = scheduled(&fx, threshold);

        let mut expected = vec![(c, Collected::Recorded), (a, Collected::Recorded)];
        if escalated {
            expected.push((b, Collected::Recorded));
        }
        assert_eq!(report.outcomes, expected, "{threshold}");
    }
}

#[test]
fn an_unknown_live_headroom_escalates() {
    // §8.6: the live account's fetch failed, so after phase 1 its headroom is still unknown.
    let fx = Fx::new();
    let [a, b, c] = three_accounts(&fx);
    fx.script_usage(500, json!({}));
    fx.script_usage(200, usage_fixture());

    let report = scheduled(&fx, 99.9);

    assert_eq!(
        report.outcomes,
        [
            (c, failed("http-500")),
            (a, Collected::Recorded),
            (b, Collected::Recorded)
        ]
    );
}

#[test]
fn the_ticks_own_models_decide_the_live_headroom() {
    // §8.2: a scoped window counts only when the models name it. Fable's limit at 95 %
    // escalates a tick at 99.9 only when the tick's models make it relevant.
    let mut body = usage_fixture();
    body["limits"][2]["percent"] = json!(95);
    let none: &[&str] = &[];
    for (models, escalated) in [(none, false), (&["Fable"][..], true)] {
        let fx = Fx::new();
        let [a, b, c] = three_accounts(&fx);
        fx.script_usage(200, body.clone());

        let report = scheduled_on(&fx.engine, 99.9, models);

        let mut expected = vec![(c, Collected::Recorded), (a, Collected::Recorded)];
        if escalated {
            expected.push((b, Collected::Recorded));
        }
        assert_eq!(report.outcomes, expected, "{models:?}");
    }
}

#[test]
fn escalated_candidates_are_read_in_parallel() {
    // §8.3 "Who collects": a tick's fetches run in parallel like `list`'s. The live account was
    // read 100 s ago and is not due, so phase 1 sends nothing, and phase 2's two requests meet.
    let fx = Fx::new();
    let [a, b, c] = three_accounts(&fx);
    fx.script_usage(200, usage_fixture());
    fx.collect(&[&c]);
    fx.clock.advance_ms(100_000);
    let probe = Arc::new(Rendezvous {
        inner: fx.http.clone(),
        want: 2,
        counts: Mutex::new((0, 0)),
        arrived: Condvar::new(),
    });
    let engine = fx.engine_with_http(probe.clone());

    let report = scheduled_on(&engine, 92.0, &[]);

    assert_eq!(
        report.outcomes,
        [
            (c, Collected::Ineligible(Ineligible::NotDue)),
            (a, Collected::Recorded),
            (b, Collected::Recorded)
        ]
    );
    assert_eq!(
        probe.counts.lock().unwrap().1,
        2,
        "both requests were in flight at once"
    );
}

#[test]
fn without_a_managed_live_login_every_due_candidate_is_read() {
    // No live account for phase 1, so no headroom: phase 2 escalates (§8.6). An auto tick never
    // collects here (§11.2 step 2 stops it first); this pins what the mode does on its own.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.login("x@x.co", "rt-x"); // a login tagteam does not manage
    fx.script_usage(200, usage_fixture());

    let report = scheduled(&fx, 99.9);

    assert_eq!(
        report.outcomes,
        [(a, Collected::Recorded), (b, Collected::Recorded)]
    );
    let sent: BTreeSet<String> = usage_bearers(&fx).into_iter().collect();
    assert_eq!(
        sent,
        BTreeSet::from(["at-rt-a".to_owned(), "at-rt-b".to_owned()])
    );
}

#[test]
fn an_account_quarantined_while_it_is_collected_is_reported() {
    // §7.4: the gate's Dead verdict quarantines a during the tick, and the report names it for
    // the tick's `account-quarantined` event (§11.4). An account already quarantined when the
    // collection starts is not named: it was never reserved.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    fx.expire_access(&a);
    fx.script_token_error(400, "invalid_grant");
    fx.script_usage(200, usage_fixture());

    let report = scheduled(&fx, 99.9);

    assert_eq!(
        report.outcomes,
        [
            (b, Collected::Recorded),
            (a.clone(), failed("refresh-failed"))
        ]
    );
    assert_eq!(report.quarantined, [a.clone()]);
    assert_eq!(quarantine_of(&fx, &a).0.as_deref(), Some("invalid_grant"));

    let report = fx.collect(&[&a]);
    assert_eq!(
        report.outcomes,
        [(a, Collected::Ineligible(Ineligible::Quarantined))]
    );
    assert!(report.quarantined.is_empty());
}

#[test]
fn a_scheduled_tick_sends_nothing_for_an_identity_whose_hour_is_spent() {
    // §8.6, Review Focus 2: the budget decides whether a request may be sent, for a tick as for
    // any caller. The live account's hour is spent, so its headroom stays unknown and the tick
    // escalates; a's hour is spent too, and only b is read.
    let fx = Fx::new();
    let [a, b, c] = three_accounts(&fx);
    spend_budget(&fx, &c, 20, 100);
    spend_budget(&fx, &a, 20, 100);
    fx.script_usage(200, usage_fixture());

    let report = scheduled(&fx, 99.9);

    let free = NOW_S - 100 + 3660;
    assert_eq!(
        report.outcomes,
        [
            (c, Collected::OverBudget { next_free_at: free }),
            (a, Collected::OverBudget { next_free_at: free }),
            (b, Collected::Recorded)
        ]
    );
    assert_eq!(usage_bearers(&fx), ["at-rt-b"]);
    assert_eq!(usage_requests(&fx), 41);
}

#[test]
fn a_tick_after_a_long_suspend_sends_at_most_one_request_per_identity() {
    // Review Focus 2: on wake every reading is stale and every plan is due. The tick reads each
    // account once and never catches up on the polls it slept through. (50 minutes keeps the
    // access tokens valid, so no refresh joins in.)
    let fx = Fx::new();
    let [a, b, c] = three_accounts(&fx);
    fx.script_usage(200, usage_fixture());
    scheduled(&fx, 90.0);
    fx.http.clear();
    fx.script_usage(200, usage_fixture());
    fx.clock.advance_ms(3_000_000);

    let report = scheduled(&fx, 90.0);

    assert_eq!(
        report.outcomes,
        [
            (c, Collected::Recorded),
            (a, Collected::Recorded),
            (b, Collected::Recorded)
        ]
    );
    let mut sent = usage_bearers(&fx);
    sent.sort();
    assert_eq!(sent, ["at-rt-a", "at-rt-b", "at-rt-c"]);
    assert_eq!(
        usage_requests(&fx),
        6,
        "one request per identity at each of the two ticks"
    );
}

#[test]
fn a_recheck_reads_each_listed_account_older_than_180_s_whatever_its_plan() {
    // §8.3: consume-first's re-check (§11.2 step 8). The plans made at the first read are at
    // least 243 s out, so on demand still waits at 181 s; a re-check does not.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    fx.script_usage(200, usage_fixture());
    fx.collect(&[&a, &b]);
    let recheck = || {
        fx.engine
            .collect_usage(CollectMode::Recheck {
                accounts: vec![a.clone(), b.clone()],
                threshold: 90.0,
                models: Vec::new(),
            })
            .unwrap()
            .outcomes
    };
    let not_due = Collected::Ineligible(Ineligible::NotDue);

    fx.clock.advance_ms(180_000);
    assert_eq!(
        recheck(),
        [(a.clone(), not_due.clone()), (b.clone(), not_due.clone())],
        "180 s is not older than 180 s"
    );
    fx.clock.advance_ms(1_000);
    assert_eq!(
        fx.collect(&[&a, &b]).outcomes,
        [(a.clone(), not_due.clone()), (b.clone(), not_due)]
    );
    assert_eq!(
        recheck(),
        [
            (a.clone(), Collected::Recorded),
            (b.clone(), Collected::Recorded)
        ]
    );
    assert_eq!(state(&fx, &a).fetched_at, Some(NOW_S + 181));
    assert_eq!(usage_bearers(&fx).len(), 4);
}

#[test]
fn a_ticks_collection_plans_for_the_ticks_threshold_not_the_settings() {
    // §8.6's urgent 60 s plan is for a live account moving within 15 points of the threshold.
    // A tick's threshold (flags over the file) makes that plan, scheduled or re-checked; the
    // engine's settings say 90. Under 80, 70 % is urgent though 70 < 90 − 15; under 99.9, 77 %
    // is not though 77 ≥ 90 − 15. Both readings move (≥ 1 point), so only the band decides:
    // urgent is 60 s, otherwise half the previous 270 s, floored at 180.
    for recheck in [false, true] {
        for (threshold, before, after, interval) in
            [(80.0, 60.0, 70.0, 60), (99.9, 66.0, 77.0, 180)]
        {
            let fx = Fx::new();
            let a = fx.add("a@x.co", "rt-a"); // live, and the only account
            let collect = |fx: &Fx| {
                let mode = if recheck {
                    CollectMode::Recheck {
                        accounts: vec![a.clone()],
                        threshold,
                        models: Vec::new(),
                    }
                } else {
                    CollectMode::Scheduled {
                        provider: fx.provider(),
                        threshold,
                        models: Vec::new(),
                    }
                };
                fx.engine.collect_usage(mode).unwrap().outcomes
            };
            let mut body = usage_fixture();
            body["seven_day"]["utilization"] = json!(before);
            fx.script_usage(200, body.clone());
            assert_eq!(collect(&fx), [(a.clone(), Collected::Recorded)]);
            assert_eq!(state(&fx, &a).poll_interval_s, Some(270), "a first reading");

            fx.http.clear();
            body["seven_day"]["utilization"] = json!(after);
            fx.script_usage(200, body);
            fx.clock.advance_ms(300_000);
            assert_eq!(collect(&fx), [(a.clone(), Collected::Recorded)]);

            let s = state(&fx, &a);
            let case = format!("recheck {recheck}, threshold {threshold}");
            assert_eq!(s.poll_interval_s, Some(interval), "{case}");
            let ahead = s.next_poll_at.unwrap() - (NOW_S + 300);
            assert!(
                (interval..=interval * 11 / 10).contains(&ahead),
                "{case}: {ahead}"
            );
        }
    }
}

#[cfg(feature = "test-hooks")]
mod hooks {
```

In `crates/tagteam-engine/tests/collect.rs`, at the end of `mod hooks` (the end of the file),
replace:

```rust
        assert_eq!(fx.usage_state(&b), None, "nothing is recorded");
        assert_eq!(usage_requests(&fx), 0, "the slot went back");
    }
}
```

with:

```rust
        assert_eq!(fx.usage_state(&b), None, "nothing is recorded");
        assert_eq!(usage_requests(&fx), 0, "the slot went back");
    }

    #[test]
    fn a_signal_at_the_gates_entry_starts_no_refresh() {
        // §14.1: no refresh starts once the token is set. The gate takes its account lock with
        // a try, which never looks at the token, so without the cancellation point at its entry
        // the token request would leave.
        let fx = Fx::new();
        let a = due(&fx);
        fx.script_refresh(Some("rt-a2"));
        fx.script_usage(200, usage_fixture());
        signal_at(&fx, "usage-before-gate");

        assert_interrupted(collect_result(&fx, &a));

        assert!(
            fx.http.requests().is_empty(),
            "no token request and no usage request"
        );
        assert_eq!(
            fx.vault_refresh_token(&a).as_deref(),
            Some("rt-a"),
            "the vault is untouched"
        );
        assert_eq!(fx.usage_state(&a), None, "nothing is recorded");
        assert_eq!(usage_requests(&fx), 0, "the slot went back");
    }

    #[test]
    fn a_signal_during_phase_one_starts_no_phase_two() {
        // §14.1: the live account's request had left, so its reading is recorded; no candidate
        // is reserved once the token is set.
        let fx = Fx::new();
        let [a, b, c] = three_accounts(&fx);
        fx.script_usage(200, usage_fixture());
        signal_at(&fx, "usage-before-record");

        assert_interrupted(fx.engine.collect_usage(CollectMode::Scheduled {
            provider: fx.provider(),
            threshold: 50.0,
            models: Vec::new(),
        }));

        assert_eq!(usage_bearers(&fx), ["at-rt-c"]);
        assert_eq!(state(&fx, &c).fetched_at, Some(NOW_S));
        assert_eq!((fx.usage_state(&a), fx.usage_state(&b)), (None, None));
        assert_eq!(usage_requests(&fx), 1);
    }
}
```

In `crates/tagteam-engine/tests/collect_active.rs`, at the end of `mod hooks` (the end of the
file), replace:

```rust
        assert_eq!(
            fx.usage_state(&a).and_then(|s| s.fetched_at),
            None,
            "nothing is recorded as a's"
        );
    }
}
```

with:

```rust
        assert_eq!(
            fx.usage_state(&a).and_then(|s| s.fetched_at),
            None,
            "nothing is recorded as a's"
        );
    }

    /// `engine`'s cancel token, set from inside the named hook as a signal handler would set it.
    fn signal_at(engine: &tagteam_engine::Engine, point: &'static str) {
        let cancel = engine.cancel().clone();
        engine.on_point(point, Box::new(move || cancel.request(libc::SIGINT)));
    }

    /// `id`'s on-demand collection through `engine` ended as SIGINT's interruption.
    fn assert_interrupted(engine: &tagteam_engine::Engine, id: &tagteam_core::AccountId) {
        let result = engine.collect_usage(CollectMode::OnDemand {
            accounts: vec![id.clone()],
        });
        assert!(
            matches!(
                result,
                Err(tagteam_engine::EngineError::Interrupted(libc::SIGINT))
            ),
            "{result:?}"
        );
    }

    #[test]
    fn a_signal_before_active_token_refresh_records_nothing_for_an_expired_live_token() {
        // §14.1: no §7.5 starts once the token is set. A live setup token cannot refresh, so
        // without the cancellation point at §7.5's entry its expired token would be recorded as
        // a `token-expired` failure, backing the account off for a fetch that never happened.
        let fx = Fx::new();
        let s = live_setup_token(&fx);
        let mut live = fx.live_credential().unwrap();
        live["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms());
        fx.set_live_credential(live.to_string().as_bytes());
        signal_at(&fx.engine, "usage-live-identity-read");

        assert_interrupted(&fx.engine, &s);

        assert_eq!(fx.usage_state(&s), None, "nothing is recorded");
        assert!(fx.http.requests().is_empty());
        assert_eq!(usage_requests(&fx), 0, "the slot went back");
    }

    #[test]
    fn a_signal_before_active_token_refresh_asks_the_oracle_nothing() {
        // §14.1: the live token was refused (`rejected_fp`) but is still valid locally, so §7.5
        // asks the profile oracle about it before taking any lock (§7.6). Once the token is set
        // that request must not leave.
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        refuse(&fx, &a, "rt-a");
        let engine = fx.engine_with_oracle(Arc::new(tagteam_engine::oracle::HttpOracle::new(
            fx.http.clone(),
            fx.clock.clone(),
        )));
        signal_at(&engine, "usage-live-identity-read");

        assert_interrupted(&engine, &a);

        assert!(
            fx.http.requests().is_empty(),
            "no profile, token or usage request"
        );
        assert_eq!(fx.usage_state(&a).and_then(|s| s.last_error), None);
        assert_eq!(usage_requests(&fx), 0, "the slot went back");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-core --lib poll`
Expected: FAIL to compile: `cannot find type DueCandidate`, `cannot find struct, variant or union
type DueCandidate`, `cannot find function scheduled_pick` and `cannot find function escalates`.

Run: `cargo test -p tagteam-engine --features test-hooks --test store_usage --test collect`
Expected: FAIL to compile: `unresolved import tagteam_engine::store::Eligibility` (store_usage),
`no variant named Scheduled found for enum CollectMode`, `no variant named Recheck found` and
`no field quarantined on type CollectReport` (collect).

Run: `cargo test -p tagteam-engine --features test-hooks --test collect_active
a_signal_before_active_token_refresh`
Expected: PASS, 2 tests. They pin a check that exists; Step 8 shows each fails without it.

- [ ] **Step 3: The pick and the escalation, in core**

In `crates/tagteam-core/src/poll.rs` (after `budget_next_free`), replace:

```rust
    counted.sort_unstable();
    Some(counted[counted.len() - limit].saturating_add(b.count_window_s))
}

#[cfg(test)]
```

with:

```rust
    counted.sort_unstable();
    Some(counted[counted.len() - limit].saturating_add(b.count_window_s))
}

/// One candidate as §8.6's scheduled collection sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DueCandidate {
    pub position: u32,
    /// A scheduled collection may reserve it now (§8.3): not in backoff, and a poll due or no
    /// reading yet.
    pub due: bool,
    /// Its reading's `fetched_at`; `None` when it has never been read, or when the stamp has no
    /// usable age (§8.4).
    pub fetched_at: Option<i64>,
}

/// §8.6 phase 2: the single stalest due candidate (never fetched first, then the oldest
/// `fetched_at`, ties to the lower position), or every due candidate, stalest first, when
/// `escalate`. Positions; empty when none is due.
pub fn scheduled_pick(cands: &[DueCandidate], escalate: bool) -> Vec<u32> {
    let mut due: Vec<&DueCandidate> = cands.iter().filter(|c| c.due).collect();
    // `None` sorts before any `Some`: never fetched first.
    due.sort_by_key(|c| (c.fetched_at, c.position));
    let take = if escalate { due.len() } else { 1 };
    due.into_iter().take(take).map(|c| c.position).collect()
}

/// §8.6: whether a tick escalates to every due candidate: the active account's max relevant
/// pct is within `margin` points of `threshold` (≥ threshold − margin), or its headroom is still
/// unknown. A non-finite pct is unknown.
pub fn escalates(active_max_pct: Option<f64>, threshold: f64, margin: f64) -> bool {
    active_max_pct
        .filter(|p| p.is_finite())
        .is_none_or(|p| p >= threshold - margin)
}

#[cfg(test)]
```

- [ ] **Step 4: The reservation's three rules**

In `crates/tagteam-engine/src/store/usage.rs` (before `Ineligible`), replace:

```rust
/// Why an account was not reserved (§8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ineligible {
```

with:

```rust
/// Which §8.3 caller reserves, and so when an account's reading may be fetched again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eligibility {
    /// `list`, `status` and `switch`: the reading is older than the floor (180 s), and a poll
    /// is due, where no plan counts as due.
    OnDemand,
    /// An auto tick (§8.6): a poll is due, or there is no reading yet.
    Scheduled,
    /// Consume-first's re-check (§11.2 step 8): the reading is older than the floor, whatever
    /// the plan says.
    Recheck,
}

impl Eligibility {
    /// The schedule half of §8.3's eligibility, from the stored `fetched_at` and `next_poll_at`.
    /// A reading stamped more than `FUTURE_STAMP_SLACK_S` ahead of now has no usable age
    /// (§8.4): it counts as unread, so it cannot lock the account out until the clock catches
    /// up. The same skew leaves a `next_poll_at` further ahead than any legal plan, which
    /// counts as due.
    pub(crate) fn allows(
        self,
        fetched_at: Option<i64>,
        next_poll_at: Option<i64>,
        now_s: i64,
        budget: &PollBudget,
    ) -> bool {
        let fetched_at = fetched_at.filter(|t| !is_future_stamped(*t, now_s));
        let due = next_poll_at.is_none_or(|t| t <= now_s || plan_is_skewed(t, now_s, budget));
        let older_than_floor = fetched_at.is_none_or(|t| now_s - t > budget.floor_s);
        match self {
            Eligibility::OnDemand => due && older_than_floor,
            Eligibility::Scheduled => due || fetched_at.is_none(),
            Eligibility::Recheck => older_than_floor,
        }
    }
}

/// Whether a stored `backoff_until` still holds at `now_s`. A failure recorded while the clock
/// ran ahead leaves a backoff no legal schedule reaches (`backoff_is_skewed`), which must not
/// lock the account out until the clock catches up (§8.4).
pub(crate) fn backoff_holds(until: Option<i64>, now_s: i64) -> bool {
    until.is_some_and(|t| t > now_s && !backoff_is_skewed(t, now_s))
}

/// Why an account was not reserved (§8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ineligible {
```

In `crates/tagteam-engine/src/store/usage.rs` (the doc and signature of `reserve_usage`, after
its first paragraph), replace:

```rust
    /// Eligibility, in this order: not quarantined, not in backoff, no live lease, then the
    /// schedule. A clock that ran ahead when a record was written leaves times no legal
    /// schedule reaches, which count as clock skew and are ignored: a `fetched_at` more than
    /// `FUTURE_STAMP_SLACK_S` ahead counts as no reading, a `next_poll_at` more than
    /// `count_window_s` plus the slack ahead counts as due, and a `backoff_until` more than
    /// `MAX_BACKOFF_S` plus the slack ahead is no backoff (§8.4). An on-demand caller
    /// (`list`, `status`) needs the reading to be older than `floor_s` and a poll to be due,
    /// where no plan counts as due. A scheduled caller (M3)
    /// needs a poll to be due or no reading at all (§8.3's "due or stale"). An eligible
    /// account then needs a free slot in its identity's hourly budget (§8.6). Over budget, the
    /// fetch reports `over-budget`: the refusal is recorded as the collector records
    /// `authorize_send`'s, one more consecutive failure with `last_error = over-budget`,
    /// `last_attempt_at` now and a backoff until `max(now + §8.5's base, next_free_at)`, so an
    /// account with no reading shows why; its `next_poll_at` moves to `next_free_at`, the
    /// reading is never touched, and no lease is taken. The backoff is checked before the
    /// budget, so a refusal is recorded once per budget period. Otherwise the lease (§6.1's
    /// statement, 90 s) and one `usage_requests` slot are taken together.
    pub fn reserve_usage(
        &self,
        account: &AccountRow,
        now_ms: i64,
        on_demand: bool,
        budget: &PollBudget,
    ) -> Result<Reserve, StoreError> {
```

with:

```rust
    /// Eligibility, in this order: not quarantined, not in backoff, no live lease, then the
    /// schedule, as `eligibility` reads it (`Eligibility::allows`). A clock that ran ahead when
    /// a record was written leaves times no legal schedule reaches, which count as clock skew
    /// and are ignored: a `fetched_at` more than `FUTURE_STAMP_SLACK_S` ahead counts as no
    /// reading, a `next_poll_at` more than `count_window_s` plus the slack ahead counts as due,
    /// and a `backoff_until` more than `MAX_BACKOFF_S` plus the slack ahead is no backoff
    /// (§8.4). An eligible account then needs a free slot in its identity's hourly budget
    /// (§8.6). Over budget, the fetch reports `over-budget`: the refusal is recorded as the
    /// collector records `authorize_send`'s, one more consecutive failure with
    /// `last_error = over-budget`, `last_attempt_at` now and a backoff until
    /// `max(now + §8.5's base, next_free_at)`, so an account with no reading shows why; its
    /// `next_poll_at` moves to `next_free_at`, the reading is never touched, and no lease is
    /// taken. The backoff is checked before the budget, so a refusal is recorded once per
    /// budget period. Otherwise the lease (§6.1's statement, 90 s) and one `usage_requests`
    /// slot are taken together.
    pub fn reserve_usage(
        &self,
        account: &AccountRow,
        now_ms: i64,
        eligibility: Eligibility,
        budget: &PollBudget,
    ) -> Result<Reserve, StoreError> {
```

In `crates/tagteam-engine/src/store/usage.rs`, in `reserve_usage`, replace:

```rust
        // Clock skew: a failure recorded while the clock ran ahead leaves a backoff no legal
        // schedule reaches (`MAX_BACKOFF_S`), which must not lock the account out until the
        // clock catches up.
        if backoff_until.is_some_and(|t| t > now_s && !backoff_is_skewed(t, now_s)) {
            return Ok(Reserve::Ineligible(Ineligible::Backoff));
        }
```

with:

```rust
        if backoff_holds(backoff_until, now_s) {
            return Ok(Reserve::Ineligible(Ineligible::Backoff));
        }
```

In `crates/tagteam-engine/src/store/usage.rs`, in the same function, replace:

```rust
        // A reading stamped more than the slack ahead of now has no usable age (§8.4): it
        // counts as unread, so it cannot lock the account out until the clock catches up.
        let fetched_at = fetched_at.filter(|t| !is_future_stamped(*t, now_s));
        // The same skew leaves a `next_poll_at` further ahead than any legal plan: it counts
        // as due.
        let due = next_poll_at.is_none_or(|t| t <= now_s || plan_is_skewed(t, now_s, budget));
        let eligible = if on_demand {
            due && fetched_at.is_none_or(|t| now_s - t > budget.floor_s)
        } else {
            due || fetched_at.is_none()
        };
        if !eligible {
            return Ok(Reserve::Ineligible(Ineligible::NotDue));
        }
```

with:

```rust
        if !eligibility.allows(fetched_at, next_poll_at, now_s, budget) {
            return Ok(Reserve::Ineligible(Ineligible::NotDue));
        }
```

In `crates/tagteam-engine/src/store/mod.rs`, replace:

```rust
pub use usage::{
    Ineligible, LiveIdentityCacheRow, Reservation, Reserve, SendGrant, Slot, UsageStateRow,
};
pub(crate) use usage::{backoff_is_skewed, plan_is_skewed};
```

with:

```rust
pub use usage::{
    Eligibility, Ineligible, LiveIdentityCacheRow, Reservation, Reserve, SendGrant, Slot,
    UsageStateRow,
};
pub(crate) use usage::{backoff_holds, backoff_is_skewed, plan_is_skewed};
```

- [ ] **Step 5: The collector's three modes**

In `crates/tagteam-engine/src/collect.rs` (the imports and `CollectMode`), replace:

```rust
use tagteam_core::backoff::failure_backoff_s;
use tagteam_core::poll::plan_after_fetch;
use tagteam_core::usage::{earliest_relevant_reset, max_relevant_pct};
use tagteam_core::{AccountId, PollBudget, PollInputs, PollPlan, ProviderId, Window};
use tagteam_provider::provider::UsageResult;
use tagteam_provider::{Credential, LockError, Provenance, Provider, Read, TransientKind};

use crate::active::{ActiveOutcome, ActiveTrigger};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::refresh::{GateOutcome, expired};
use crate::store::{
    AccountRow, Ineligible, Reservation, Reserve, SendGrant, Slot, Store, StoreError, UsageStateRow,
};

/// Who asked for a collection, and so which accounts are collected. M3 adds `Scheduled`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectMode {
    /// `list` and `status` (§8.3): the listed accounts, each only if its reading is older than
    /// the 180 s floor and a poll is due or none is planned.
    OnDemand { accounts: Vec<AccountId> },
}
```

with:

```rust
use tagteam_core::backoff::failure_backoff_s;
use tagteam_core::poll::{DueCandidate, escalates, plan_after_fetch, scheduled_pick};
use tagteam_core::trust::is_future_stamped;
use tagteam_core::usage::{earliest_relevant_reset, max_relevant_pct};
use tagteam_core::{AccountId, PollBudget, PollInputs, PollPlan, ProviderId, Window};
use tagteam_provider::provider::UsageResult;
use tagteam_provider::{Credential, LockError, Provenance, Provider, Read, TransientKind};

use crate::active::{ActiveOutcome, ActiveTrigger};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::refresh::{GateOutcome, expired};
use crate::store::{
    AccountRow, Eligibility, Ineligible, Reservation, Reserve, SendGrant, Slot, Store, StoreError,
    UsageStateRow, backoff_holds,
};
use crate::switch::is_candidate;

/// Who asked for a collection, and so which accounts are collected and when each is due (§8.3).
#[derive(Debug, Clone, PartialEq)]
pub enum CollectMode {
    /// `list`, `status` and `switch` (§8.3): the listed accounts, each only if its reading is
    /// older than the 180 s floor and a poll is due or none is planned. Plans follow the
    /// settings' threshold and models.
    OnDemand { accounts: Vec<AccountId> },
    /// An auto tick (§8.6). Phase 1: the provider's live account, when it is managed and a
    /// poll is due or it has no reading. Phase 2, from the store as phase 1 left it: one pick
    /// (`scheduled_pick`) among the provider's other switchable accounts, or every due one when
    /// the live account's decision-grade max relevant pct under `models` is within the
    /// provider's escalation margin of `threshold`, or is unknown. `threshold` and `models` are
    /// the tick's (flags over the file), and every plan recorded after a fetch follows them.
    Scheduled {
        provider: ProviderId,
        threshold: f64,
        models: Vec<String>,
    },
    /// Consume-first's re-check (§8.3, §11.2 step 8): each listed account whose reading is older
    /// than the 180 s floor, whatever its plan. Plans follow the tick's `threshold` and
    /// `models`, as `Scheduled`'s do.
    Recheck {
        accounts: Vec<AccountId>,
        threshold: f64,
        models: Vec<String>,
    },
}
```

In `crates/tagteam-engine/src/collect.rs` (`CollectReport` and `Outcome`), replace:

```rust
#[derive(Debug, Clone, Default)]
pub struct CollectReport {
    /// One entry per listed account that exists, in the order listed.
    pub outcomes: Vec<(AccountId, Collected)>,
    /// Lines for stderr, each naming an account and never a token: a successor lost while
    /// collecting (§8.3), a refresh that failed with an error or, for the live token (§8.1),
    /// with any outcome but Dead, or an account whose collection ended with an error.
    pub warnings: Vec<String>,
}

/// One account's collection: what it did, and its warnings.
type Outcome = (Collected, Vec<String>);
```

with:

```rust
#[derive(Debug, Clone, Default)]
pub struct CollectReport {
    /// One entry per account collected. `OnDemand` and `Recheck`: each listed account that
    /// exists, in the order listed. `Scheduled`: the live account first, when it is managed,
    /// then each picked candidate in pick order.
    pub outcomes: Vec<(AccountId, Collected)>,
    /// Lines for stderr, each naming an account and never a token: a successor lost while
    /// collecting (§8.3), a refresh that failed with an error or, for the live token (§8.1),
    /// with any outcome but Dead, or an account whose collection ended with an error.
    pub warnings: Vec<String>,
    /// The accounts quarantined while their collection ran, in `outcomes` order (§7.4: a Dead
    /// verdict or an identity conflict in the gate or §7.5, or a lost successor). Only an
    /// account this collection reserved can be named, since a quarantined one is never
    /// reserved; a quarantine another process set meanwhile names it too.
    pub quarantined: Vec<AccountId>,
}

impl CollectReport {
    /// Adds each account's result, in order. An error that ended one account's collection is a
    /// warning naming the account and a `Failed { kind: "error" }` outcome, so one account
    /// never costs the others theirs.
    fn add(&mut self, results: Vec<(&AccountRow, Result<Outcome, EngineError>)>) {
        for (row, result) in results {
            let outcome = result.unwrap_or_else(|e| Outcome {
                collected: Collected::Failed {
                    kind: "error".to_owned(),
                },
                warnings: vec![format!(
                    "usage for {} (position {}) was not collected: {e}",
                    row.label, row.position
                )],
                quarantined: false,
            });
            if outcome.quarantined {
                self.quarantined.push(row.id.clone());
            }
            self.outcomes.push((row.id.clone(), outcome.collected));
            self.warnings.extend(outcome.warnings);
        }
    }
}

/// One account's collection: what it did, its warnings, and whether the account was
/// quarantined while it ran.
struct Outcome {
    collected: Collected,
    warnings: Vec<String>,
    quarantined: bool,
}

impl Outcome {
    /// Nothing was reserved, so nothing was sent or recorded.
    fn unreserved(collected: Collected) -> Self {
        Outcome {
            collected,
            warnings: Vec::new(),
            quarantined: false,
        }
    }
}

/// What a collection's caller sets for every account it collects: when one is eligible
/// (§8.3), and the threshold and models its next plan is made for (§8.6: the urgent band and
/// the relevant windows). An auto tick passes its own, flags over the file; on demand passes
/// the settings'.
#[derive(Clone, Copy)]
struct Policy<'m> {
    eligibility: Eligibility,
    threshold: f64,
    models: &'m [String],
}

/// The roles every thread needs, read once before any starts (`Engine::roles`).
struct Roles {
    /// Each provider's recorded active account.
    recorded: HashMap<ProviderId, Option<AccountId>>,
    /// The accounts the providers' live logins name (§8.1's active accounts).
    live: HashSet<AccountId>,
}
```

In `crates/tagteam-engine/src/collect.rs` (`collect_usage`, now dispatching on the mode),
replace:

```rust
impl Engine {
    /// §8.3 on demand: every listed account on its own thread, and the call waits for them
    /// all. A usage failure is never an error here: it is recorded, and reported in the
    /// report's outcomes and warnings. So is an error that ends one account's collection (the
    /// store failing under it): every thread is joined and kept, and that account's outcome is
    /// `Failed { kind: "error" }` with one warning naming it, so one account never costs the
    /// others' outcomes. `Err` only for an error before any thread starts (opening the store,
    /// reading the listed accounts, reading each provider's recorded active account), or for
    /// §14.1's cancel token set during the collection: `Interrupted`, once every thread has
    /// joined and given back the slot it held unsent. IDs that name no account are skipped.
    /// Never creates the store.
    pub fn collect_usage(&self, mode: CollectMode) -> Result<CollectReport, EngineError> {
        let CollectMode::OnDemand { accounts } = mode;
        hooks::point(self, "usage-collect-start")?;
        let Some(shared) = self.existing_store()? else {
            return Ok(CollectReport::default());
        };
        let store: &Store = &shared;
        let mut rows = Vec::new();
        for id in &accounts {
            if let Some(row) = store.account(id)? {
                rows.push(row);
            }
        }
        // Each provider's recorded active account is read before its live login: a switch
        // writes the live login before it commits the record, so a record read first can only
        // be older than the live role, never newer (`Collection::active_now`).
        let mut recorded: HashMap<ProviderId, Option<AccountId>> = HashMap::new();
        for row in &rows {
            if !recorded.contains_key(&row.provider) {
                recorded.insert(row.provider.clone(), store.active(&row.provider)?);
            }
        }
        hooks::point(self, "usage-roles-between-reads")?;
        let live = self.live_accounts(&rows);
        let results: Vec<Result<Outcome, EngineError>> = thread::scope(|s| {
            let running: Vec<_> = rows
                .iter()
                .map(|row| {
                    let active = live.contains(&row.id);
                    let started_with = recorded[&row.provider].clone();
                    s.spawn(move || self.collect_one(store, row, active, started_with))
                })
                .collect();
            running
                .into_iter()
                .map(|t| match t.join() {
                    Ok(result) => result,
                    Err(panic) => std::panic::resume_unwind(panic),
                })
                .collect()
        });
        // §14.1: a collection the token was set during is the command's interruption, whatever
        // each account did. A request already sent was recorded as usual; nothing else was.
        if let Some(signal) = self.cancel().requested() {
            return Err(EngineError::Interrupted(signal));
        }
        let mut report = CollectReport::default();
        for (row, result) in rows.iter().zip(results) {
            let (collected, warnings) = result.unwrap_or_else(|e| {
                let warning = format!(
                    "usage for {} (position {}) was not collected: {e}",
                    row.label, row.position
                );
                (
                    Collected::Failed {
                        kind: "error".to_owned(),
                    },
                    vec![warning],
                )
            });
            report.outcomes.push((row.id.clone(), collected));
            report.warnings.extend(warnings);
        }
        Ok(report)
    }
```

with:

```rust
impl Engine {
    /// §8.3: the accounts `mode` selects, each on its own thread, and the call waits for them
    /// all. A usage failure is never an error here: it is recorded, and reported in the
    /// report's outcomes and warnings. So is an error that ends one account's collection (the
    /// store failing under it): every thread is joined and kept, and that account's outcome is
    /// `Failed { kind: "error" }` with one warning naming it, so one account never costs the
    /// others' outcomes. `Err` only for an error outside the threads (opening the store,
    /// reading the accounts, reading each provider's recorded active account and, for
    /// `Scheduled`, the provider and the store between the phases), or for §14.1's cancel
    /// token set during the collection: `Interrupted`, once every thread has joined and given
    /// back the slot it held unsent. A `Scheduled` collection interrupted in phase 1 starts no
    /// phase 2. IDs that name no account are skipped. Never creates the store.
    pub fn collect_usage(&self, mode: CollectMode) -> Result<CollectReport, EngineError> {
        hooks::point(self, "usage-collect-start")?;
        let Some(shared) = self.existing_store()? else {
            return Ok(CollectReport::default());
        };
        let store: &Store = &shared;
        match mode {
            CollectMode::OnDemand { accounts } => {
                let settings = self.settings();
                let policy = Policy {
                    eligibility: Eligibility::OnDemand,
                    threshold: settings.threshold,
                    models: &settings.models,
                };
                self.collect_listed(store, &accounts, policy)
            }
            CollectMode::Recheck {
                accounts,
                threshold,
                models,
            } => {
                let policy = Policy {
                    eligibility: Eligibility::Recheck,
                    threshold,
                    models: &models,
                };
                self.collect_listed(store, &accounts, policy)
            }
            CollectMode::Scheduled {
                provider,
                threshold,
                models,
            } => self.collect_scheduled(store, &provider, threshold, &models),
        }
    }

    /// `OnDemand` and `Recheck`: every listed account at once.
    fn collect_listed(
        &self,
        store: &Store,
        accounts: &[AccountId],
        policy: Policy<'_>,
    ) -> Result<CollectReport, EngineError> {
        let mut rows = Vec::new();
        for id in accounts {
            if let Some(row) = store.account(id)? {
                rows.push(row);
            }
        }
        let roles = self.roles(store, &rows)?;
        let all: Vec<&AccountRow> = rows.iter().collect();
        let results = self.collect_each(store, &all, &roles, policy);
        // §14.1: a collection the token was set during is the command's interruption, whatever
        // each account did. A request already sent was recorded as usual; nothing else was.
        self.check_cancel()?;
        let mut report = CollectReport::default();
        report.add(results);
        Ok(report)
    }

    /// `Scheduled` (§8.6): phase 1, then phase 2 once phase 1 has recorded, so the pick and
    /// the escalation read the live account's new reading. The live login is read once, before
    /// phase 1; a switch made meanwhile leaves its new live account a candidate, whose token
    /// the gate refuses to refresh while it may be live (§7.3 step 2).
    fn collect_scheduled(
        &self,
        store: &Store,
        provider: &ProviderId,
        threshold: f64,
        models: &[String],
    ) -> Result<CollectReport, EngineError> {
        let p = self.provider(provider)?;
        let policy = Policy {
            eligibility: Eligibility::Scheduled,
            threshold,
            models,
        };
        let rows = store.accounts(provider)?;
        let roles = self.roles(store, &rows)?;
        let live = rows.iter().find(|r| roles.live.contains(&r.id));
        let mut report = CollectReport::default();
        let first: Vec<&AccountRow> = live.into_iter().collect();
        let results = self.collect_each(store, &first, &roles, policy);
        self.check_cancel()?;
        report.add(results);
        let picked =
            self.scheduled_candidates(store, p.as_ref(), &rows, live, threshold, models)?;
        let results = self.collect_each(store, &picked, &roles, policy);
        self.check_cancel()?;
        report.add(results);
        Ok(report)
    }

    /// §8.6 phase 2's pick, read from the store. The candidates are the provider's switchable
    /// accounts (§9.3) other than the live one, whose kind has usage (a managed key has none,
    /// §13.2). One is due as `reserve_usage` would find it for a scheduled caller, leaving the
    /// lease and the budget to the reservation: not in backoff, and a poll due or no reading
    /// yet. Escalation reads the live account's decision-grade reading under `models` (§8.4);
    /// without a managed live account, or without such a reading, the headroom is unknown and
    /// the tick escalates.
    fn scheduled_candidates<'r>(
        &self,
        store: &Store,
        p: &dyn Provider,
        rows: &'r [AccountRow],
        live: Option<&AccountRow>,
        threshold: f64,
        models: &[String],
    ) -> Result<Vec<&'r AccountRow>, EngineError> {
        if !p.capabilities().usage {
            return Ok(Vec::new());
        }
        let budget = p.poll_budget();
        let now_s = self.now_ms().div_euclid(1000);
        let mut cands = Vec::new();
        for row in rows {
            let is_live = live.is_some_and(|l| l.id == row.id);
            if is_live || !is_candidate(row) || p.kind_traits(&row.kind).managed_key_axis {
                continue;
            }
            let state = store.usage_state(&row.id)?;
            let (fetched_at, backoff_until, next_poll_at) = state.map_or((None, None, None), |s| {
                (s.fetched_at, s.backoff_until, s.next_poll_at)
            });
            cands.push(DueCandidate {
                position: row.position,
                due: !backoff_holds(backoff_until, now_s)
                    && Eligibility::Scheduled.allows(fetched_at, next_poll_at, now_s, &budget),
                fetched_at: fetched_at.filter(|t| !is_future_stamped(*t, now_s)),
            });
        }
        let live_pct = match live {
            Some(row) => self
                .decision_windows(row, models)?
                .and_then(|w| max_relevant_pct(&w, models)),
            None => None,
        };
        let escalate = escalates(live_pct, threshold, budget.escalation_margin);
        Ok(scheduled_pick(&cands, escalate)
            .into_iter()
            .filter_map(|position| rows.iter().find(|r| r.position == position))
            .collect())
    }

    /// The roles every thread needs, read before any starts. Each provider's recorded active
    /// account is read before its live login: a switch writes the live login before it commits
    /// the record, so a record read first can only be older than the live role, never newer
    /// (`Collection::active_now`).
    fn roles(&self, store: &Store, rows: &[AccountRow]) -> Result<Roles, EngineError> {
        let mut recorded: HashMap<ProviderId, Option<AccountId>> = HashMap::new();
        for row in rows {
            if !recorded.contains_key(&row.provider) {
                recorded.insert(row.provider.clone(), store.active(&row.provider)?);
            }
        }
        hooks::point(self, "usage-roles-between-reads")?;
        Ok(Roles {
            recorded,
            live: self.live_accounts(rows),
        })
    }

    /// Each of `rows` on its own thread (§8.3), waiting for them all; each result with its row.
    fn collect_each<'r>(
        &self,
        store: &Store,
        rows: &[&'r AccountRow],
        roles: &Roles,
        policy: Policy<'_>,
    ) -> Vec<(&'r AccountRow, Result<Outcome, EngineError>)> {
        thread::scope(|s| {
            let running: Vec<_> = rows
                .iter()
                .map(|&row| {
                    let active = roles.live.contains(&row.id);
                    let started_with = roles.recorded[&row.provider].clone();
                    let thread =
                        s.spawn(move || self.collect_one(store, row, active, started_with, policy));
                    (row, thread)
                })
                .collect();
            running
                .into_iter()
                .map(|(row, t)| match t.join() {
                    Ok(result) => (row, result),
                    Err(panic) => std::panic::resume_unwind(panic),
                })
                .collect()
        })
    }
```

In `crates/tagteam-engine/src/collect.rs` (the head of `collect_one`), replace:

```rust
    /// One account through §8.3's three phases.
    fn collect_one(
        &self,
        store: &Store,
        row: &AccountRow,
        active: bool,
        recorded_active: Option<AccountId>,
    ) -> Result<Outcome, EngineError> {
        let Some(provider) = self.registry.get(&row.provider) else {
            return Ok((Collected::Unsupported, Vec::new()));
        };
        let p = provider.as_ref();
        if !p.capabilities().usage || p.kind_traits(&row.kind).managed_key_axis {
            return Ok((Collected::Unsupported, Vec::new()));
        }
```

with:

```rust
    /// One account through §8.3's three phases. Afterwards the account is read again: a
    /// quarantine now was set while this collection ran, since `reserve_usage` refuses a
    /// quarantined account.
    fn collect_one(
        &self,
        store: &Store,
        row: &AccountRow,
        active: bool,
        recorded_active: Option<AccountId>,
        policy: Policy<'_>,
    ) -> Result<Outcome, EngineError> {
        let Some(provider) = self.registry.get(&row.provider) else {
            return Ok(Outcome::unreserved(Collected::Unsupported));
        };
        let p = provider.as_ref();
        if !p.capabilities().usage || p.kind_traits(&row.kind).managed_key_axis {
            return Ok(Outcome::unreserved(Collected::Unsupported));
        }
```

In `crates/tagteam-engine/src/collect.rs`, in `collect_one`, replace:

```rust
        let reservation = match store.reserve_usage(row, self.now_ms(), true, &budget)? {
            Reserve::Reserved(r) => r,
            Reserve::Ineligible(why) => return Ok((Collected::Ineligible(why), Vec::new())),
            Reserve::OverBudget { next_free_at } => {
                return Ok((Collected::OverBudget { next_free_at }, Vec::new()));
            }
        };
```

with:

```rust
        let reservation =
            match store.reserve_usage(row, self.now_ms(), policy.eligibility, &budget)? {
                Reserve::Reserved(r) => r,
                Reserve::Ineligible(why) => {
                    return Ok(Outcome::unreserved(Collected::Ineligible(why)));
                }
                Reserve::OverBudget { next_free_at } => {
                    return Ok(Outcome::unreserved(Collected::OverBudget { next_free_at }));
                }
            };
```

In `crates/tagteam-engine/src/collect.rs`, in the same function, where `run` is built, replace:

```rust
            active,
            budget,
            slot: Some(slot),
```

with:

```rust
            active,
            budget,
            threshold: policy.threshold,
            models: policy.models,
            slot: Some(slot),
```

In `crates/tagteam-engine/src/collect.rs`, at the end of `collect_one`, replace:

```rust
        // Phase 3.
        run.record(fetched)
    }
```

with:

```rust
        // Phase 3.
        let (collected, warnings) = run.record(fetched)?;
        let quarantined = store
            .account(&row.id)?
            .is_some_and(|r| r.quarantine_reason.is_some());
        Ok(Outcome {
            collected,
            warnings,
            quarantined,
        })
    }
```

In `crates/tagteam-engine/src/collect.rs`, in `struct Collection`, replace:

```rust
    /// Whether the live login names this account (§8.1's active account).
    active: bool,
    budget: PollBudget,
```

with:

```rust
    /// Whether the live login names this account (§8.1's active account).
    active: bool,
    budget: PollBudget,
    /// The threshold and models the next plan is made for (`Policy`).
    threshold: f64,
    models: &'a [String],
```

In `crates/tagteam-engine/src/collect.rs` (`Collection::plan`, which reads the collection's
threshold and models, not the settings'), replace:

```rust
    /// §8.6's next plan after a success, from the previous reading and this one.
    fn plan(&self, windows: &[Window], active: bool, now_s: i64) -> PollPlan {
        let settings = self.engine.settings();
        let models = &settings.models;
        let prev = self.state.as_ref();
        let inputs = PollInputs {
            now_s,
            active,
            pct: max_relevant_pct(windows, models),
            prev_pct: prev
                .and_then(|s| s.last_good.as_deref())
                .and_then(|w| max_relevant_pct(w, models)),
            prev_interval_s: prev.and_then(|s| s.poll_interval_s),
            threshold: settings.threshold,
            last_429_at: prev.and_then(|s| s.last_429_at),
            next_relevant_reset: earliest_relevant_reset(windows, models),
        };
        plan_after_fetch(&self.budget, &inputs, jitter())
    }
```

with:

```rust
    /// §8.6's next plan after a success, from the previous reading and this one, under the
    /// collection's threshold and models.
    fn plan(&self, windows: &[Window], active: bool, now_s: i64) -> PollPlan {
        let models = self.models;
        let prev = self.state.as_ref();
        let inputs = PollInputs {
            now_s,
            active,
            pct: max_relevant_pct(windows, models),
            prev_pct: prev
                .and_then(|s| s.last_good.as_deref())
                .and_then(|w| max_relevant_pct(w, models)),
            prev_interval_s: prev.and_then(|s| s.poll_interval_s),
            threshold: self.threshold,
            last_429_at: prev.and_then(|s| s.last_429_at),
            next_relevant_reset: earliest_relevant_reset(windows, models),
        };
        plan_after_fetch(&self.budget, &inputs, jitter())
    }
```

In `crates/tagteam-engine/src/collect.rs` (`Collection::record`'s signature; its body is
unchanged), replace:

```rust
    fn record(mut self, fetched: Result<Vec<Window>, Stop>) -> Result<Outcome, EngineError> {
```

with:

```rust
    fn record(
        mut self,
        fetched: Result<Vec<Window>, Stop>,
    ) -> Result<(Collected, Vec<String>), EngineError> {
```

In `crates/tagteam-engine/src/switch.rs`, replace:

```rust
fn is_candidate(row: &AccountRow) -> bool {
```

with:

```rust
pub(crate) fn is_candidate(row: &AccountRow) -> bool {
```

- [ ] **Step 6: Name the rule at the other callers of `reserve_usage`**

Each passes the rule its `bool` meant: `true` is `Eligibility::OnDemand`, `false` is
`Eligibility::Scheduled`.

In `crates/tagteam-engine/tests/strategy.rs`, replace:

```rust
use tagteam_engine::store::Reserve;
```

with:

```rust
use tagteam_engine::store::{Eligibility, Reserve};
```

In `crates/tagteam-engine/tests/strategy.rs`, in `record_at`, replace:

```rust
        .reserve_usage(&row, at * 1000, false, &PollBudget::STANDARD)
```

with:

```rust
        .reserve_usage(
            &row,
            at * 1000,
            Eligibility::Scheduled,
            &PollBudget::STANDARD,
        )
```

In `crates/tagteam-engine/tests/views_usage.rs`, replace:

```rust
use tagteam_engine::store::{LiveIdentityCacheRow, Reservation, Reserve};
```

with:

```rust
use tagteam_engine::store::{Eligibility, LiveIdentityCacheRow, Reservation, Reserve};
```

In `crates/tagteam-engine/tests/views_usage.rs`, in `reserve`, replace:

```rust
        .reserve_usage(&row, at * 1000, false, &PollBudget::STANDARD)
```

with:

```rust
        .reserve_usage(
            &row,
            at * 1000,
            Eligibility::Scheduled,
            &PollBudget::STANDARD,
        )
```

In `crates/tagteam/src/statusline.rs`, in `mod tests`, replace:

```rust
    use tagteam_engine::store::{Reserve, Store};
```

with:

```rust
    use tagteam_engine::store::{Eligibility, Reserve, Store};
```

In `crates/tagteam/src/statusline.rs`, in the same module, replace:

```rust
            .reserve_usage(&row, now * 1000, true, &PollBudget::STANDARD)
```

with:

```rust
            .reserve_usage(
                &row,
                now * 1000,
                Eligibility::OnDemand,
                &PollBudget::STANDARD,
            )
```

In `crates/tagteam/tests/common/mod.rs`, replace:

```rust
use tagteam_engine::store::{Reserve, Store};
```

with:

```rust
use tagteam_engine::store::{Eligibility, Reserve, Store};
```

In `crates/tagteam/tests/common/mod.rs`, in `record_reading`, replace:

```rust
        .reserve_usage(&row, at_s * 1000, true, &PollBudget::STANDARD)
```

with:

```rust
        .reserve_usage(
            &row,
            at_s * 1000,
            Eligibility::OnDemand,
            &PollBudget::STANDARD,
        )
```

In `crates/tagteam/tests/common/mod.rs`, in `record_history`, replace:

```rust
            .reserve_usage(&row, at_s * 1000, false, &PollBudget::STANDARD)
```

with:

```rust
            .reserve_usage(
                &row,
                at_s * 1000,
                Eligibility::Scheduled,
                &PollBudget::STANDARD,
            )
```

In `crates/tagteam/tests/usage_cli.rs`, replace:

```rust
use tagteam_engine::store::{Reserve, SendGrant, Store};
```

with:

```rust
use tagteam_engine::store::{Eligibility, Reserve, SendGrant, Store};
```

In `crates/tagteam/tests/usage_cli.rs`, in
`an_unread_account_over_its_hourly_budget_reads_as_over_budget_not_no_data`, replace:

```rust
        .reserve_usage(&row, spent_at * 1000, false, &PollBudget::STANDARD)
```

with:

```rust
        .reserve_usage(
            &row,
            spent_at * 1000,
            Eligibility::Scheduled,
            &PollBudget::STANDARD,
        )
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p tagteam-core --lib poll`
Expected: PASS, 51 tests, the 6 new ones among them.

Run: `cargo test -p tagteam-engine --features test-hooks --test store_usage --test collect --test collect_active`
Expected: PASS: store_usage 39 (2 new), collect 62 (15 new), collect_active 23 and 1 ignored
(2 new). The scheduled tests' plans are jittered; each asserts only what holds across the
jitter (a live plan at least 243 s out, a candidate's at most 595 s).

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. The tagteam lib's pseudo-terminal prompt tests need a real pty.

- [ ] **Step 8: Show the carry-over tests fail without their checks**

In `crates/tagteam-engine/src/collect.rs`, in `Collection::gate`, delete the line
`self.interruption()?;` that follows `hooks::point(self.engine, "usage-before-gate")?;`.

Run: `cargo test -p tagteam-engine --features test-hooks --test collect a_signal_at_the_gates_entry`
Expected: FAIL at `no token request and no usage request`: the token request was sent.

Put the line back. In `Collection::refresh_live`, delete its first line, `self.interruption()?;`.

Run: `cargo test -p tagteam-engine --features test-hooks --test collect_active a_signal_before_active_token_refresh`
Expected: FAIL, both: `nothing is recorded` (a `token-expired` failure was recorded) and `no
profile, token or usage request` (the oracle's profile request was sent).

Put the line back, and run both commands again. Expected: PASS. `git diff
crates/tagteam-engine/src/collect.rs` shows only Step 5's changes.

- [ ] **Step 9: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 10: Commit**

```bash
git add crates/tagteam-core/src/poll.rs crates/tagteam-engine/src/store/usage.rs \
  crates/tagteam-engine/src/store/mod.rs crates/tagteam-engine/src/collect.rs \
  crates/tagteam-engine/src/switch.rs crates/tagteam-engine/tests/store_usage.rs \
  crates/tagteam-engine/tests/collect.rs crates/tagteam-engine/tests/collect_active.rs \
  crates/tagteam-engine/tests/strategy.rs crates/tagteam-engine/tests/views_usage.rs \
  crates/tagteam/src/statusline.rs crates/tagteam/tests/common/mod.rs \
  crates/tagteam/tests/usage_cli.rs
git commit -m "Collect usage as an auto tick schedules it and as consume-first re-checks"
```

### Task 4: `decide()`: triggers, candidates and outcomes

§11.1: "`decide(snapshot, state, config, now) -> Decision`. It is pure: no clock, no I/O. It is
built from named predicates (`below_threshold`, `landing_ok`, `beats_by_hysteresis`,
`recovered_since_departure`, `recovery_axis_useful`, …), each unit tested. The
scheduled-collection pick (§8.6) and the loop delay (§11.4) are pure functions there too."

This task writes every type the Interface Contract puts in `autoswitch.rs`, and the whole of
`decide` except step 8's gates:
- step 2's live checks;
- step 4's API-key active account;
- step 5's triggers and the unhealthy count;
- step 6's cooldown;
- step 7's candidates;
- step 9's outcome table.

It also writes `departure`, `next_delay` and `announces_sleep`. Step 8 ranks here without its
gates (known headroom above 0, most first, ties to the lower position), so the outcome tests
have targets. Task 5 replaces `rank` whole, and every test in this task is chosen to hold under
Task 5's gates too.

**Readings of the spec this task commits to:**
- **Below the threshold is usage strictly under it:** `100 − headroom < threshold`. An account
  at exactly 90% with the threshold at 90 is `proactive`.
- **At the limit is checked before below the threshold.** With a valid threshold (≤ 99.9) the
  two never overlap; the order only keeps an unclamped `AutoConfig` from idling at the limit.
- **A `Live::Managed` id the snapshot does not list is `unmanaged-active-account`** (step 2:
  tagteam never acts on a login it does not manage).
- **An included API-key active account (step 4) is `proactive` whatever the strategy,** and it
  leaves `unhealthy_ticks` unchanged, because step 5 is not reached. Task 5 gives it headroom 0
  and the landing rule without its exception.
- **The unhealthy count follows the contract:** a quarantined active account fails over at
  once and leaves the count unchanged; known headroom resets it to 0; unknown headroom adds 1
  and fails over once the count reaches `autoswitch.unhealthy_ticks`.
- **The cooldown runs while `now < last_switch_at + cooldown_seconds`.** Exactly
  `cooldown_seconds` later it is over. A last switch stamped after `now` (clock skew) keeps the
  cooldown: that is conservative, and at-limit and failover bypass it anyway.
- **API-key accounts are step 7 candidates, when included, for every trigger but
  consume-first.** A proactive tick whose only candidates are API keys is therefore
  `no-comparison`, not `no-candidates`. A consume-first tick with no OAuth peer is
  `below-threshold`.
- **Step 9's table reads the OAuth candidates, in its order:**
  - a consume-first trigger is never BLOCKED (Appendix B #26): `reset-unknown` when the active
    account's long-window reset, or every candidate's, is unknown, else
    `already-consuming-soonest`;
  - no candidate with a known headroom: `no-comparison`;
  - not every candidate known to be at its limit: `no-qualifying-candidate`;
  - otherwise `all-exhausted`. Its `earliest_reset` is the earliest of the candidates'
    `rank::blocked_until`, the moment each is usable again.
- **A reset at or before `now` is unknown** for the binding window and the long window (step 8:
  "a past or unknown reset sorts last"). `blocked_until` is used as stored, as `switch`'s
  `candidates-exhausted` message uses it.
- **`detail`:**
  - `n/N` for `active-usage-unknown`;
  - the cooldown's time left, as `rank::span` writes it (`3m`);
  - for `all-exhausted`, the span until the earliest recovery (`2h00m`; empty when unknown);
  - `""` for every other reason.
- **Every reason has one outcome,** given by `NoSwitchReason::outcome`, and every no-switch
  that `decide` returns takes its outcome from there. It is public, an addition to the
  Interface Contract, so the engine (Task 8) builds its own no-switches from the same mapping.
  Of the engine-reported reasons, `engine-running` and `live-changed` are NO_ACTION (§11.1,
  step 11), and `interrupted-switch` and `no-viable-target` are BLOCKED (steps 2 and 12).
- **The loop delay's 60 s floor bounds only the poll plan's shortening:**
  `min(jittered, max(active_next_poll_at − now, 60))`. An interval under 60 s is kept, so a
  sleep is never lengthened (Appendix B #27). The jittered interval is rounded to the second;
  a jitter outside [-1, 1] is clamped, and a NaN one counts as 0.
- **A departure records what this tick saw of the account it leaves:** its headroom and its
  binding window's reset, each `None` when unknown (an API key, an unread account).

**Files:**
- Modify: `crates/tagteam-core/src/autoswitch.rs` (Task 1 created it with `Strategy` alone)

**Interfaces:**
- Consumes: `autoswitch::Strategy` (Task 1); `usage::{Window, WindowKind, headroom}`;
  `rank::{binding_window, blocked_until, span}`; `ids::AccountId`.
- Produces, in `tagteam_core::autoswitch`, exactly as the Interface Contract states them:
  - `Trigger` (`as_str`, `parse`), `AutoConfig` (`effective_strategy`), `AccountSnapshot`,
    `Live`, `Snapshot`, `AutoState`, `Phase`, `Outcome`, `NoSwitchReason` (`as_str`),
    `Decision`, `Decided`, `Departure`;
  - `decide`, `departure`, `next_delay`, `announces_sleep`;
  - beyond the contract: `NoSwitchReason::outcome(self) -> Outcome`, every reason's outcome,
    the engine-reported ones included.
- Private, for Task 5:
  - `Rated::of` (`headroom`, `recovery_at`, `long_reset`, `back_at`), `Trigger::must_move`,
    `Triggered`, `triggered`, `no_switch`, `nothing_ranked`;
  - the predicates `below_threshold`, `at_limit`, `unhealthy_limit_reached`, `cooldown_left`,
    `is_candidate`, `every_candidate_exhausted`;
  - `rank(oauth)` and `most_headroom`, which Task 5 replaces.

- [ ] **Step 1: Write the failing tests**

Append to `crates/tagteam-core/src/autoswitch.rs`, after a blank line below `impl Strategy`'s
closing brace:

```rust
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
```

Review Focus 5 is pinned by `a_live_login_tagteam_does_not_manage_is_never_acted_on`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-core --lib autoswitch`
Expected: FAIL to compile with `error[E0432]: unresolved import NoSwitchReason`. The module has
only `Strategy` so far, and rustc stops at the unresolved import.

- [ ] **Step 3: Write the types, the predicates and `decide`**

In `crates/tagteam-core/src/autoswitch.rs`, below the first line
(`//! §11: auto-switch decisions. Pure: no clock, no I/O.`), insert a blank line and:

```rust
use std::cmp::Ordering;

use crate::ids::AccountId;
use crate::rank::{binding_window, blocked_until, span};
use crate::usage::{Window, headroom};
```

Then insert, between `impl Strategy`'s closing brace and `#[cfg(test)]`, with a blank line on
each side:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-core --lib autoswitch`
Expected: PASS, 31 tests.

Run: `cargo test -p tagteam-core`
Expected: PASS, 210 lib tests.

- [ ] **Step 5: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-core/src/autoswitch.rs
git commit -m "Decide the auto-switch trigger, candidates and outcome, and the loop delay"
```

---

### Task 5: `decide()`: ranking, the no-return rule and the API-key fallback

§11.2 step 8 ranks the OAuth candidates: it skips unknown headroom, headroom ≤ 0 and the
barred account; it applies the landing rule, `best`'s hysteresis or consume-first's sooner
reset; with every account above the threshold it applies the recovery and headroom axes; and
"`at-limit` and `failover` skip every anti-flap gate". §11.3 bars a proactive or consume-first
return to the account the engine left "unless A has *recovered* against its departure
snapshot". This task replaces Task 4's ungated `rank` with all of that. It also appends the
API-key fallback (steps 9 and 10) to `decide`, and adds consume-first's two phases
(Decision 2).

**Readings of the spec this task commits to:**
- **The trigger picks the ranking, not the configured strategy.**
  - A `consume-first` trigger ranks on `cfg.long_window`.
  - A `proactive` trigger ranks as `best`. That includes a consume-first account above the
    threshold and a way back from an API key.
  - `at-limit` and `failover` order by most headroom and skip every anti-flap gate: the
    landing rule, the hysteresis, the axes and the bar. They still skip unknown and exhausted
    candidates.
- **"Every account above the threshold"** is the active account plus every OAuth candidate
  with a known headroom. A barred or exhausted candidate counts; an unknown one is never
  ranked, so it does not. The mode never applies to a way back from an API key (step 4). In it,
  the axes replace the hysteresis, and the order is the soonest binding-window recovery (a
  past or unknown one last), then most headroom, then the lower position.
- **"Within 3 points of headroom" means both are spent:** each headroom ≤ 3 (cswap's
  `SPENT_HEADROOM_PCT`). **"Resets within 4 h"** means the binding window's reset is known,
  after `now`, and at most 14 400 s away.
- **The axes:**
  - the recovery axis needs a recovery at least 300 s sooner (inclusive). A candidate whose
    recovery is unknown never passes it, and any known recovery passes against an active
    account whose recovery is unknown;
  - the headroom axis needs `h ≥ 2 × active` (inclusive).
- **Consume-first** needs a long-window reset strictly sooner than the active account's, both
  known. The landing rule applies; there is no hysteresis. The order is the soonest reset, then
  most headroom, then the lower position.
- **The bar (§11.3)** holds while live is `last_switch_to` and the candidate is
  `last_switch_from`, for proactive and consume-first.
  - A departure snapshot is missing when there is no `left_trigger`, or a non-failover one
    with no `left_headroom`. A missing snapshot lifts the bar.
  - Recovery has three legs, and any one is enough: headroom (`h − left_headroom ≥ 3`);
    recovery (`left_recovery_at − recovery_at ≥ 300`, both known: an unknown recovery is not
    "sooner" here, unlike in the axis's order); dominance (`h > 2 × active + 3`, an API-key
    active account counting as 0).
  - **"A failover departure is judged on the landing and recovery legs":** the account left
    has recovered when it is below the threshold now (the landing leg), or by the recovery leg.
    Its `left_headroom` was unknown, or untrusted for a quarantined account, so the headroom
    and dominance legs do not apply.
  - The bar lifts only when the barred ranking is empty and the account left has recovered.
    The gated ranking then runs again without the bar, so that account still faces every gate.
- **The API-key fallback:** for at-limit and failover, `targets` is the ranked OAuth targets
  followed by every API-key candidate in position order. That covers steps 9 and 10 at once,
  because the engine tries the targets in order. A proactive switch never lists an API key.
- **Consume-first's two phases (Decision 2):**
  - `recheck: true` only for a consume-first switch in `Phase::Initial`.
  - In `Phase::Rechecked`, a consume-first switch whose first target was read more than 180 s
    ago is `stale-usage` (NO_ACTION). Otherwise every later target read more than 180 s ago is
    dropped from `targets`, so each target the tick tries is fresh: a re-check whose fetch for
    one candidate failed never lets the tick fall back to that candidate's old reading.
  - A re-check that turned the trigger into at-limit, failover or proactive moves as an
    initial tick would. §11.5's "never idling at the limit while a viable candidate exists"
    outranks a one-tick wait for freshness.
  - The engine passes the same `AutoState` to both calls, so the unhealthy count moves once.

**Files:**
- Modify: `crates/tagteam-core/src/autoswitch.rs` (its constants and predicates, `rank`,
  `decide`, and `mod tests`)

**Interfaces:**
- Consumes: Task 4's private `Rated`, `Triggered`, `no_switch`, `nothing_ranked`,
  `below_threshold`, `at_limit`, `Trigger::must_move`.
- Produces: no new public item. `decide` now implements step 8, §11.3, the API-key fallback
  and both phases.
- Private:
  - the predicates `landing_ok`, `beats_by_hysteresis`, `every_account_above`,
    `recovery_axis_useful`, `sooner_by_hysteresis`, `recovers_sooner`, `doubles_headroom`,
    `dominates`, `resets_sooner`, `fresh_after_recheck`, `departure_missing`, `barred`,
    `recovered_since_departure`;
  - `Ranking`, `rank(r, oauth)`, `gated`, `ordered`, `most_headroom`, `soonest_long_reset`,
    `soonest_recovery`.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-core/src/autoswitch.rs`, add these at the end of `mod tests`, after
`a_sleep_longer_than_one_and_a_half_intervals_is_announced`:

```rust
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
```

Review Focus 4 is pinned here by
`every_account_at_its_limit_for_days_never_flaps_and_sleeps_at_most_600_s`, and in Task 6 by a
simulation.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-core --lib autoswitch`
Expected: FAIL to compile, 34 errors, all `error[E0425]: cannot find function`, for
`recovery_axis_useful`, `recovers_sooner`, `resets_sooner`, `landing_ok`,
`fresh_after_recheck`, `every_account_above`, `beats_by_hysteresis`, `doubles_headroom` and
`dominates`.

- [ ] **Step 3: Rank behind the gates, and add the bar, the fallback and the phases**

In `crates/tagteam-core/src/autoswitch.rs`, after `const SLEEP_EVENT_INTERVALS: f64 = 1.5;`,
insert:

```rust
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
```

After the closing brace of `every_candidate_exhausted`, insert a blank line and:

```rust
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
```

Replace `rank` and `most_headroom`, from the line
`/// §11.2 step 8's order alone, without its gates: known headroom above 0, most first, ties to`
through the closing brace of `most_headroom`, with:

```rust
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
```

Replace `decide` whole, from its doc line
`/// §11.2 steps 2 and 4–9 (the engine owns steps 1, 3, 10–12). Pure.` through its closing
brace, with:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-core --lib autoswitch`
Expected: PASS, 62 tests: Task 4's 31, unchanged, and these 31.

Run: `cargo test -p tagteam-core`
Expected: PASS, 241 lib tests.

- [ ] **Step 5: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-core/src/autoswitch.rs
git commit -m "Rank auto-switch targets behind the anti-flap gates and the no-return bar"
```

---

### Task 6: Simulation tests

§11.5: "A proptest harness drives `decide()` through multi-day synthetic traces: 2–6 accounts,
burn rates, 5h and 7d resets, 429s, dead tokens, unknown readings, and API-key accounts." It
asserts seven properties, from "no A→B→A return within a cooldown unless A recovered" to
"deterministic results for a given seed". This task adds `proptest`, writes the harness in
`crates/tagteam-core/tests/simulate.rs`, and adds the two pure functions the "`--once` exit
codes consistent with the outcome" property needs. Task 10 uses those functions for
`tagteam auto --once`.

**Readings of the spec this task commits to:**
- **`proptest` 1.11** (rust-version 1.85, so it builds on the pinned 1.88), with its default
  features off and `std` alone: no forking, no timeouts, no regex strategies. It is a
  dev-dependency of `tagteam-core` only (Decision 11).
- **Deterministic:** every property runs ChaCha with a fixed seed (`RngSeed::Fixed`) and a
  pinned case count: 48 three-day traces, 48 four-day spent weeks and 256 exit-code vectors.
  Failure persistence is off, because the seed replays any failure. The whole file runs in
  about a second in a debug build.
- **The generator:**
  - 2–6 accounts. The first, live at the start, is OAuth; each other is an API key (no
    windows) one time in five.
  - Each OAuth account has a 5h (`Short`) and a 7d (`Long`) window with its own burn rate
    while live, an idle rate while not (a session elsewhere), starting usage, and resets that
    roll over by their period. Usage stops at 100%.
  - Up to seven incidents of three kinds: a 429 (no new reading; the last stays trusted until
    its earliest reset, at most 2 h); an unreadable account (no trusted reading: unknown
    usage); a dead token (quarantined while it lasts).
  - The settings range over §6.4's valid values, and one provider in five has no long window
    (consume-first then runs `best`, §4.5).
- **The simulator plays the engine (Tasks 3, 7 and 8):**
  - it collects as scheduled (each account when due by its own poll interval; a re-check also
    reads every reading older than 180 s);
  - it hands `decide` decision-grade windows only (an hour while a plan is in force);
  - it calls `decide` twice when consume-first asks for a re-check, with the same state;
  - it performs a switch on the first target, recording `departure` and `last_switch_*`;
  - it stores `unhealthy_ticks: 0` with a performed switch. The count belonged to the
    account it left, and Task 8's tick must do the same;
  - it sleeps `next_delay`, with the live account's next poll.
- **The oracles are restated from the spec,** not taken from `autoswitch.rs`'s private
  predicates: step 7's candidates, §8.2's headroom, the binding reset, `blocked_until`, §11.3's
  recovery legs, and each reason's outcome.
- **"No A→B→A return within a cooldown unless A recovered" is checked for proactive and
  consume-first returns.** At-limit and failover skip the anti-flap gates by design (§11.2
  step 8), so a return they make is not a flap.
- **Two public functions, additions to the Interface Contract:**
  - `once_exit_code(&Decision) -> i32` maps a switch to 0, NO_ACTION to 2 and BLOCKED to 3. A
    tick that failed has no decision; Task 10 maps it to 1.
  - `most_severe(&[i32]) -> i32` orders the codes 1 > 0 > 3 > 2 (§11.1). An empty slice is 2.
    A code outside 0–3 outranks all four, so an unexpected code is never hidden.

**Files:**
- Modify: `Cargo.toml` (the workspace's test-only dependencies)
- Modify: `crates/tagteam-core/Cargo.toml`
- Modify: `Cargo.lock` (cargo adds proptest 1.11.0 and what it pulls in: rand 0.9.5,
  rand_core 0.9.5, rand_chacha 0.9.0, rand_xorshift 0.4.0, getrandom 0.3.4, ppv-lite86,
  zerocopy and its derive, unarray, and target-only crates for WASI and UEFI)
- Modify: `crates/tagteam-core/src/autoswitch.rs` (`once_exit_code`, `most_severe`, and
  `mod tests`)
- Create: `crates/tagteam-core/tests/simulate.rs`

**Interfaces:**
- Consumes: `decide`, `departure`, `next_delay` and the types of Tasks 4 and 5;
  `tagteam_core::{AccountId, Window, WindowKind}`.
- Produces:
  - `tagteam_core::autoswitch::once_exit_code(d: &Decision) -> i32`
  - `tagteam_core::autoswitch::most_severe(codes: &[i32]) -> i32`
  - the workspace dev-dependency `proptest`

- [ ] **Step 1: Add `proptest`**

In `Cargo.toml`, replace

```toml
assert_cmd = "2"
predicates = "3"
tempfile = "3"
```

with

```toml
assert_cmd = "2"
predicates = "3"
# §11.5's simulations (tagteam-core only): std alone, without the fork and timeout features.
proptest = { version = "1.11", default-features = false, features = ["std"] }
tempfile = "3"
```

In `crates/tagteam-core/Cargo.toml`, replace

```toml
thiserror.workspace = true

[lints]
```

with

```toml
thiserror.workspace = true

[dev-dependencies]
proptest.workspace = true

[lints]
```

Run: `cargo build -p tagteam-core --tests`
Expected: cargo downloads proptest 1.11.0 and its dependencies, records them in `Cargo.lock`,
and builds.

- [ ] **Step 2: Write the failing tests**

In `crates/tagteam-core/src/autoswitch.rs`, add these at the end of `mod tests`, after
`every_account_at_its_limit_for_days_never_flaps_and_sleeps_at_most_600_s`:

```rust
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
```

Create `crates/tagteam-core/tests/simulate.rs`:

```rust
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
```

Review Focus 4's simulation is
`a_spent_week_never_flaps_and_moves_only_to_an_account_that_recovered`.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p tagteam-core`
Expected: FAIL to compile. The lib tests report `error[E0425]: cannot find function` for
`once_exit_code` (3) and `most_severe` (6); `simulate` reports
`error[E0432]: unresolved imports tagteam_core::autoswitch::most_severe, tagteam_core::autoswitch::once_exit_code`.

- [ ] **Step 4: Write `once_exit_code` and `most_severe`**

In `crates/tagteam-core/src/autoswitch.rs`, after the closing brace of `announces_sleep`,
insert a blank line and:

```rust
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
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tagteam-core --lib autoswitch`
Expected: PASS, 64 tests.

Run: `cargo test -p tagteam-core --test simulate`
Expected: PASS, 4 tests, in about a second.

Run: `cargo test -p tagteam-core`
Expected: PASS, 243 lib tests and the 4 simulations.

- [ ] **Step 6: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock crates/tagteam-core/Cargo.toml \
  crates/tagteam-core/src/autoswitch.rs crates/tagteam-core/tests/simulate.rs
git commit -m "Simulate multi-day auto-switch traces and map decisions to --once exit codes"
```

### Task 7: Auto preconditions and the commit-time record in the switch

§11.2 step 11: "Call `switch` (direct) with the tick's preconditions. The switch re-checks them
under its locks, before its first write (§9.4 step 1)", and "The switch's commit (§9.4 step 9)
records `last_switch_*`, `left_headroom`, `left_recovery_at` and `left_trigger` together with the
switch." This task makes the ordinary transaction carry an automatic switch (Decisions 4 and 5):
`SwitchRequest.auto` holds the preconditions, `rederive` checks them under every lock, and
`commit_switch` writes the departure record in the transaction that sets the active account,
inserts the event and deletes the journal row. It also gives the store the `autoswitch_state`
read and the count's write that the tick (Task 8) uses, and fixes M3a's parked `best` message.

**Readings of the spec this task commits to:**
- **The preconditions are one function, run twice.** `auto_refusal` runs while planning, before
  the oracle and the mutation lock, so a refused switch spends no request. It runs again in
  `rederive`, under every lock, where it decides (§9.4 step 1). A refusal is `Rederived::Done`,
  never `Replan`, so it can never fall through to a plan that switches elsewhere. Nothing between
  `switch()`'s entry and the locks undoes them: an automatic request is never forced, its target
  is a direct `SwitchTarget::Account`, a usage strategy's collection never runs for it, and it
  skips `freshen_plan`.
- **The order is the spec's: the live account, the cooldown, the candidate.** A cooldown ends the
  tick and a non-candidate only moves it on (Task 8), so the cooldown wins when both hold.
- **"The live account is still the one this tick decided on" covers every live change:** another
  managed account, an unmanaged login, and no login at all. Without the last two, a direct switch
  would displace the one or activate over the other.
- **The cooldown is judged as `decide` judges it (Task 4):** in epoch seconds, held while
  `now < last_switch_at + cooldown_s`, from `autoswitch_state` as read under the lock, for
  `proactive` and `consume-first` only.
- **A candidate is §11.2 step 7's:** the account still exists on the provider, is enabled, is not
  quarantined, and has an identity and a non-empty vault credential (§9.3). Session-owned is never
  true before M4. A target removed meanwhile is not a candidate either, rather than an error.
- **An automatic switch does not freshen in `switch()`.** The tick freshens each target by §11.2
  step 10's table before it calls `switch` (Task 8); `freshen_plan` would apply the manual table
  (§7.2) a second time.
- **The record.** Its `at` is the commit's time in epoch seconds, the unit `decide` reads, and its
  `from` is the outgoing account the preconditions confirmed. `commit_switch` writes it first in
  its transaction, so a later statement that fails takes it down too. It resets
  `unhealthy_ticks` to 0 (Decision 4).
- **The `switch` event row.** `trigger` is the automatic switch's trigger, else `manual`; the old
  `auto` value is not one of §6.1's triggers. `source` is the request's.
- **Ruling: recovery never records.** `finish_forward` commits with no record. The journal row
  names neither the switch's source nor its trigger, so recovery cannot tell an automatic switch
  from a manual one; the departure snapshot was the dead tick's view of usage, which cannot be
  rebuilt later; and recording a manual switch would start a cooldown and bar a return nobody
  asked for. §11.2 step 2's "its events carry `source` = `auto`" is about who recovers: Task 8
  passes `auto` when a tick recovers. Cost if wrong: after a crashed automatic switch the next
  tick has no cooldown and no bar, so it can move again at once if the hysteresis allows.
- **Ruling: a `left_trigger` this build does not know reads as `None`.** That lifts the no-return
  bar (§11.3: a missing departure snapshot lifts it) instead of failing every tick on a store
  error. Cost if wrong: one unbarred return after a downgrade.
- **M3a's parked item.** With a managed live login of unknown usage, `best` leaves out the
  candidates known to be at their limit. When no remaining candidate holds a stored credential,
  none of them is switchable (§9.3), so the result is `candidates-exhausted` naming the exhausted
  ones, as `next-available` reports the same case. It was `usage-unavailable` with "no candidate
  with a known usage reading holds a stored credential", which was false.

**Files:**
- Modify: `crates/tagteam-engine/src/store/mod.rs` (`AutoRecord`, `RECORD_SWITCH_SQL`,
  `commit_switch`, `autoswitch_state`, `set_unhealthy_ticks`)
- Modify: `crates/tagteam-engine/src/switch.rs` (`SwitchRequest`, `AutoPerform`, `SwitchReason`,
  `LIVE_CHANGED`, `best_pick`, `auto_refusal`, `plan`, `switch_planned`, `rederive`, `apply`)
- Modify: `crates/tagteam-engine/src/recover.rs` (`finish_forward`'s commit)
- Modify: every other `SwitchRequest` literal, for `auto: None`. `rg -n 'SwitchRequest \{' crates/`
  lists them: `crates/tagteam/src/app.rs` and `crates/tagteam-engine/tests/{common/mod.rs,
  fake_agent.rs, switch_rollback.rs, strategy.rs, switch.rs}`.
- Test: `crates/tagteam-engine/tests/{store.rs, switch.rs, strategy.rs, recover.rs}`

**Interfaces:**
- Consumes: `tagteam_core::autoswitch::{AutoState, Departure, Trigger}` (Tasks 1, 4 and 5),
  `Engine::has_login`, `noop`, `tagteam_core::rank::span`, `Engine::exhausted_message`; the
  fixtures `Fx`, `Fx::switch_request`, `crashed_switch`, `write_target_credential`,
  `token_requests`.
- Produces:
  - `tagteam_engine::store::AutoRecord { at: i64, from: AccountId, to: AccountId, departure:
    Departure }` (`Debug, Clone, PartialEq`)
  - `Store::autoswitch_state(&self, provider: &ProviderId) -> Result<AutoState, StoreError>`: the
    default when the provider has no row
  - `Store::set_unhealthy_ticks(&self, provider: &ProviderId, n: u32) -> Result<(), StoreError>`
  - `Store::commit_switch(&self, provider: &ProviderId, to: &AccountId, event: &EventRow,
    record: Option<&AutoRecord>) -> Result<(), StoreError>`
  - `SwitchRequest.auto: Option<AutoPerform>` and `tagteam_engine::switch::AutoPerform {
    expected_from: AccountId, trigger: Trigger, cooldown_s: i64, departure: Departure }`
    (`Debug, Clone`)
  - `SwitchReason::{LiveChanged, Cooldown, NotCandidate}`, spelled `live-changed`, `cooldown` and
    `not-candidate`. Each refusal is a no-op whose `from` is the live account as read, with the
    message:
    - `LiveChanged`: "the live login changed after this switch was decided; a switch made
      meanwhile is never overridden"
    - `Cooldown`: "the cooldown after the last automatic switch has <span> left"
    - `NotCandidate`: "<label> (position <n>) is no longer a candidate: it is disabled", "…: it
      needs a new login", "…: it has no stored credential", or "the account to switch to was
      removed"
  - crate-private: `Engine::auto_refusal`

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/store.rs`, replace

```rust
use std::os::unix::fs::PermissionsExt;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::store::{EventRow, JournalRow, LoginMeta, NewAccount, Store, StoreError};
```

with

```rust
use std::os::unix::fs::PermissionsExt;
use tagteam_core::autoswitch::{AutoState, Departure, Trigger};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::store::{
    AutoRecord, EventRow, JournalRow, LoginMeta, NewAccount, Store, StoreError,
};
```

In `crates/tagteam-engine/tests/store.rs`, replace

```rust
    s.commit_switch(&cc(), &b, &ev).unwrap();
```

with

```rust
    s.commit_switch(&cc(), &b, &ev, None).unwrap();
```

Append to the end of `crates/tagteam-engine/tests/store.rs`:

```rust
/// The `switch` event a commit from `from` to `to` inserts.
fn switch_event(from: &AccountId, to: &AccountId, trigger: &str, source: &str) -> EventRow {
    EventRow {
        at: 1_790_000_000_000,
        provider: cc(),
        kind: "switch".into(),
        from_id: Some(from.clone()),
        to_id: Some(to.clone()),
        trigger: Some(trigger.into()),
        source: source.into(),
        detail: None,
    }
}

/// What an automatic switch from `from` to `to` records (§11.2 step 11).
fn record(from: &AccountId, to: &AccountId) -> AutoRecord {
    AutoRecord {
        at: 1_790_000_000,
        from: from.clone(),
        to: to.clone(),
        departure: Departure {
            left_headroom: Some(4.5),
            left_recovery_at: Some(1_790_009_630),
            left_trigger: Trigger::Proactive,
        },
    }
}

#[test]
fn auto_switch_state_is_the_default_until_written_and_kept_per_provider() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let fake = ProviderId::new("fake-agent");
    assert_eq!(s.autoswitch_state(&cc()).unwrap(), AutoState::default());
    s.set_unhealthy_ticks(&cc(), 2).unwrap();
    s.set_unhealthy_ticks(&fake, 1).unwrap();
    assert_eq!(
        s.autoswitch_state(&cc()).unwrap(),
        AutoState {
            unhealthy_ticks: 2,
            ..AutoState::default()
        }
    );
    s.set_unhealthy_ticks(&cc(), 3).unwrap();
    assert_eq!(s.autoswitch_state(&cc()).unwrap().unhealthy_ticks, 3);
    assert_eq!(s.autoswitch_state(&fake).unwrap().unhealthy_ticks, 1);
}

#[test]
fn an_automatic_commit_records_its_departure_and_resets_the_count() {
    // §9.4 step 9 and Decision 4: the record and the reset ride the switch's own commit.
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    s.set_unhealthy_ticks(&cc(), 2).unwrap();
    let ev = switch_event(&a, &b, "proactive", "auto");
    s.commit_switch(&cc(), &b, &ev, Some(&record(&a, &b)))
        .unwrap();
    let recorded = AutoState {
        last_switch_at: Some(1_790_000_000),
        last_switch_from: Some(a.clone()),
        last_switch_to: Some(b.clone()),
        left_headroom: Some(4.5),
        left_recovery_at: Some(1_790_009_630),
        left_trigger: Some(Trigger::Proactive),
        unhealthy_ticks: 0,
    };
    assert_eq!(s.autoswitch_state(&cc()).unwrap(), recorded);
    assert_eq!(s.active(&cc()).unwrap(), Some(b.clone()));
    assert_eq!(s.events().unwrap(), vec![ev]);
    // A manual commit records nothing: the last automatic switch's record stands.
    s.set_unhealthy_ticks(&cc(), 1).unwrap();
    s.commit_switch(&cc(), &a, &switch_event(&b, &a, "manual", "cli"), None)
        .unwrap();
    assert_eq!(
        s.autoswitch_state(&cc()).unwrap(),
        AutoState {
            unhealthy_ticks: 1,
            ..recorded
        }
    );
}

#[test]
fn the_record_lands_with_the_commit_or_not_at_all() {
    // One transaction (§9.4 step 9): a commit whose active-account write fails, here on its
    // foreign key, leaves no record, and the journal row stays for recovery.
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    s.insert_journal(&JournalRow {
        provider: cc(),
        holder: ProcessStamp { pid: 1, start: 2 },
        from_id: Some(a.clone()),
        to_id: b.clone(),
        from_fp: None,
        from_identity: None,
        to_fp: "sha256:b".into(),
        started_at: 5,
        prior: None,
    })
    .unwrap();
    let ghost = AccountId::from_string("ghost");
    let ev = switch_event(&a, &ghost, "proactive", "auto");
    assert!(
        s.commit_switch(&cc(), &ghost, &ev, Some(&record(&a, &ghost)))
            .is_err()
    );
    assert_eq!(s.autoswitch_state(&cc()).unwrap(), AutoState::default());
    assert!(s.journal(&cc()).unwrap().is_some());
    assert!(s.events().unwrap().is_empty());
}

#[test]
fn a_departure_trigger_this_build_does_not_know_reads_as_none() {
    // §11.3: a missing departure snapshot lifts the no-return bar; an unknown one is missing.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let ev = switch_event(&a, &b, "proactive", "auto");
    s.commit_switch(&cc(), &b, &ev, Some(&record(&a, &b)))
        .unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("UPDATE autoswitch_state SET left_trigger = 'idle-hold'", [])
        .unwrap();
    let state = s.autoswitch_state(&cc()).unwrap();
    assert_eq!(state.left_trigger, None);
    assert_eq!(state.last_switch_from, Some(a));
}
```

In `crates/tagteam-engine/tests/switch.rs`, replace

```rust
use common::{
    API_KEY, Fx, OTHER_API_KEY, STRAY_API_KEY, capture_logs, mutation_lock_free, usage_fixture,
};
use serde_json::json;
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_core::poll::PollPlan;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::EngineError;
use tagteam_engine::lifecycle::AddTokenOptions;
use tagteam_engine::store::NewAccount;
use tagteam_engine::switch::{SwitchOutcome, SwitchReason, SwitchRequest, SwitchTarget};
```

with

```rust
use common::{
    API_KEY, Fx, OTHER_API_KEY, STRAY_API_KEY, capture_logs, mutation_lock_free, token_requests,
    usage_fixture,
};
use serde_json::json;
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_core::autoswitch::{AutoState, Departure, Trigger};
use tagteam_core::poll::PollPlan;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::EngineError;
use tagteam_engine::lifecycle::AddTokenOptions;
use tagteam_engine::store::{EventRow, NewAccount};
use tagteam_engine::switch::{
    AutoPerform, SwitchOutcome, SwitchReason, SwitchRequest, SwitchTarget,
};
```

Append to the end of `crates/tagteam-engine/tests/switch.rs`:

```rust
/// The fixture clock's start (`Fx`), in seconds.
const T0: i64 = 1_790_000_000;

/// An automatic switch from `from` to `target` with a 300 s cooldown, as a tick performs it
/// (§11.2 step 11).
fn auto_switch(fx: &Fx, from: &AccountId, target: &AccountId, trigger: Trigger) -> SwitchRequest {
    SwitchRequest {
        provider: fx.provider(),
        target: to(target),
        force: false,
        source: "auto",
        auto: Some(AutoPerform {
            expected_from: from.clone(),
            trigger,
            cooldown_s: 300,
            departure: Departure {
                left_headroom: Some(4.0),
                left_recovery_at: Some(T0 + 9_630),
                left_trigger: trigger,
            },
        }),
    }
}

fn auto_state(fx: &Fx) -> AutoState {
    fx.engine
        .store()
        .unwrap()
        .autoswitch_state(&fx.provider())
        .unwrap()
}

fn switch_events(fx: &Fx) -> Vec<EventRow> {
    let events = fx.engine.store().unwrap().events().unwrap();
    events.into_iter().filter(|e| e.kind == "switch").collect()
}

#[test]
fn the_new_reasons_carry_the_spec_s_tokens() {
    assert_eq!(SwitchReason::LiveChanged.as_str(), "live-changed");
    assert_eq!(SwitchReason::Cooldown.as_str(), "cooldown");
    assert_eq!(SwitchReason::NotCandidate.as_str(), "not-candidate");
}

#[test]
fn an_automatic_switch_records_its_departure_in_the_commit() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let store = fx.engine.store().unwrap();
    store.set_unhealthy_ticks(&fx.provider(), 2).unwrap();
    let out = fx
        .engine
        .switch(auto_switch(&fx, &b, &a, Trigger::Proactive))
        .unwrap();
    assert_eq!(
        (out.switched, out.reason, out.strategy),
        (true, SwitchReason::Switched, "direct")
    );
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(
        auto_state(&fx),
        AutoState {
            last_switch_at: Some(T0),
            last_switch_from: Some(b.clone()),
            last_switch_to: Some(a.clone()),
            left_headroom: Some(4.0),
            left_recovery_at: Some(T0 + 9_630),
            left_trigger: Some(Trigger::Proactive),
            unhealthy_ticks: 0,
        }
    );
    let last = switch_events(&fx).pop().unwrap();
    assert_eq!(
        (
            last.trigger.as_deref(),
            last.source.as_str(),
            last.from_id,
            last.to_id
        ),
        (Some("proactive"), "auto", Some(b), Some(a))
    );
}

#[test]
fn a_manual_switch_records_no_auto_state_and_its_trigger_is_manual() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    assert!(fx.switch_to(&a, false).unwrap().switched);
    assert_eq!(auto_state(&fx), AutoState::default());
    let last = switch_events(&fx).pop().unwrap();
    assert_eq!(
        (last.trigger.as_deref(), last.source.as_str()),
        (Some("manual"), "cli")
    );
}

/// Review Focus 1, the precondition half: a manual `tagteam switch` lands between the tick's
/// decision and its perform, while the automatic switch waits before the mutation lock. The
/// automatic one finds the live account moved and writes nothing; the manual result stands.
#[cfg(feature = "test-hooks")]
#[test]
fn a_manual_switch_between_the_decision_and_the_perform_is_never_overridden() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c"); // live
    let other = fx.engine_with_env(fx.env.clone());
    let manual = fx.switch_request(&b, false);
    fx.engine.on_point(
        "planned",
        Box::new(move || assert!(other.switch(manual.clone()).unwrap().switched)),
    );
    let out = fx
        .engine
        .switch(auto_switch(&fx, &c, &a, Trigger::Proactive))
        .unwrap();
    assert_eq!(
        (out.switched, out.reason, out.reason.as_str()),
        (false, SwitchReason::LiveChanged, "live-changed")
    );
    assert_eq!(out.from.map(|r| r.id), Some(b.clone()));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
    let store = fx.engine.store().unwrap();
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(b.clone()));
    let switches = switch_events(&fx);
    assert_eq!(switches.len(), 1, "{switches:?}");
    assert_eq!(
        (switches[0].source.as_str(), switches[0].to_id.clone()),
        ("cli", Some(b))
    );
    assert_eq!(auto_state(&fx), AutoState::default());
}

#[test]
fn an_automatic_switch_never_acts_on_a_live_login_it_did_not_decide_on() {
    // §11.2 step 11: an unmanaged login, or none at all, is a live change too. A direct switch
    // would displace the one and activate over the other.
    let leave: [fn(&Fx); 2] = [log_out, |fx| fx.login("stranger@x.co", "rt-s")];
    for (n, leave) in leave.into_iter().enumerate() {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        leave(&fx);
        let live = fx.live_credential();
        let out = fx
            .engine
            .switch(auto_switch(&fx, &b, &a, Trigger::Failover))
            .unwrap();
        assert_eq!(
            (out.switched, out.reason),
            (false, SwitchReason::LiveChanged),
            "case {n}"
        );
        assert_eq!(fx.live_credential(), live, "case {n}");
        assert!(fx.displaced().is_empty(), "case {n}");
        assert!(switch_events(&fx).is_empty(), "case {n}");
        assert_eq!(auto_state(&fx), AutoState::default(), "case {n}");
    }
}

#[test]
fn the_cooldown_is_judged_again_from_the_state_under_the_lock() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let first = fx
        .engine
        .switch(auto_switch(&fx, &b, &a, Trigger::Proactive))
        .unwrap();
    assert!(first.switched);
    let recorded = auto_state(&fx);
    fx.clock.advance_ms(60_000);
    for trigger in [Trigger::Proactive, Trigger::ConsumeFirst] {
        let out = fx.engine.switch(auto_switch(&fx, &a, &b, trigger)).unwrap();
        assert_eq!(
            (out.switched, out.reason, out.reason.as_str()),
            (false, SwitchReason::Cooldown, "cooldown"),
            "{trigger:?}"
        );
        assert_eq!(
            out.message,
            "the cooldown after the last automatic switch has 4m left"
        );
    }
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(auto_state(&fx), recorded);
    assert_eq!(switch_events(&fx).len(), 1);
    // §11.2 step 6: at-limit and failover bypass it.
    let out = fx
        .engine
        .switch(auto_switch(&fx, &a, &b, Trigger::AtLimit))
        .unwrap();
    assert!(out.switched, "{}", out.message);
    assert_eq!(auto_state(&fx).last_switch_at, Some(T0 + 60));
    // The cooldown ends 300 s after that switch, to the second.
    fx.clock.advance_ms(299_000);
    let out = fx
        .engine
        .switch(auto_switch(&fx, &b, &a, Trigger::Proactive))
        .unwrap();
    assert_eq!(out.reason, SwitchReason::Cooldown);
    assert_eq!(
        out.message,
        "the cooldown after the last automatic switch has <1m left"
    );
    fx.clock.advance_ms(1_000);
    let out = fx
        .engine
        .switch(auto_switch(&fx, &b, &a, Trigger::Proactive))
        .unwrap();
    assert!(out.switched, "{}", out.message);
}

/// What stops `a` being a candidate, in one case below.
type Uncandidate = fn(&Fx, &AccountId);

#[test]
fn a_target_that_is_no_longer_a_candidate_writes_nothing() {
    // §11.2 step 11: a direct switch accepts a disabled or quarantined target (§9.3, §7.2),
    // so an automatic one checks for itself, and the tick moves on to its next target. The
    // quarantined target's token is due: nothing is refreshed for it either.
    let cases: [(&str, Uncandidate, &str); 4] = [
        (
            "disabled",
            |fx, a| drop(fx.engine.set_disabled(a, true).unwrap()),
            "a@x.co (position 1) is no longer a candidate: it is disabled",
        ),
        (
            "quarantined",
            |fx, a| {
                fx.expire_access(a);
                fx.quarantine(a, "invalid_grant", "sha256:sent");
            },
            "a@x.co (position 1) is no longer a candidate: it needs a new login",
        ),
        (
            "vault-less",
            |fx, a| fx.kc.delete(SERVICE, a.as_str()).unwrap(),
            "a@x.co (position 1) is no longer a candidate: it has no stored credential",
        ),
        (
            "removed",
            |fx, a| drop(fx.engine.remove(a).unwrap()),
            "the account to switch to was removed",
        ),
    ];
    for (case, make, message) in cases {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b"); // live
        make(&fx, &a);
        let out = fx
            .engine
            .switch(auto_switch(&fx, &b, &a, Trigger::AtLimit))
            .unwrap();
        assert_eq!(
            (out.switched, out.reason, out.reason.as_str()),
            (false, SwitchReason::NotCandidate, "not-candidate"),
            "{case}"
        );
        assert_eq!(out.message, message, "{case}");
        assert_eq!(fx.live_email().as_deref(), Some("b@x.co"), "{case}");
        assert!(switch_events(&fx).is_empty(), "{case}");
        assert_eq!(token_requests(&fx), 0, "{case}");
    }
}

#[test]
fn an_automatic_switch_leaves_freshening_to_the_tick() {
    // §11.2 step 10: the tick freshens each target by its own table before it performs; the
    // switch does not freshen again.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    fx.expire_access(&a);
    let out = fx
        .engine
        .switch(auto_switch(&fx, &b, &a, Trigger::AtLimit))
        .unwrap();
    assert!(out.switched, "{}", out.message);
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
}
```

Append to the end of `crates/tagteam-engine/tests/strategy.rs`:

```rust
#[test]
fn best_names_the_exhausted_candidates_when_the_healthy_one_holds_no_credential() {
    // M3a's parked item. The live c's usage is unknown; a is at its limit and holds a
    // credential; b has headroom but an empty vault, so it is not switchable (§9.3). Nothing
    // switches, and the reason says why instead of claiming no known candidate holds one.
    let fx = Fx::new();
    let (a, b, _c) = three(&fx); // c is live and never read
    read(&fx, &a, &reading(100.0, 20.0, 0.0));
    read(&fx, &b, &reading(10.0, 40.0, 0.0));
    fx.kc.delete(SERVICE, b.as_str()).unwrap();
    let out = best(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason, out.strategy),
        (false, SwitchReason::CandidatesExhausted, "best")
    );
    assert_eq!(
        out.message,
        "every candidate is at its limit: a@x.co (5h at 100%); the earliest reset is in 2h40m"
    );
    assert_eq!(fx.live_email().as_deref(), Some("c@x.co"));
}
```

In `crates/tagteam-engine/tests/recover.rs`, replace

```rust
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
```

with

```rust
use tagteam_core::AccountId;
use tagteam_core::autoswitch::AutoState;
use tagteam_engine::EngineError;
```

In `crates/tagteam-engine/tests/recover.rs`, before

```rust
#[test]
fn forward_recovery_displaces_a_stray_secret_on_the_axis_it_clears() {
```

insert

```rust
/// Task 7's ruling: recovery records no auto-switch state, even for a switch an engine began.
/// The journal row names neither the switch's source nor its trigger, and the departure
/// snapshot was the dead tick's view of usage, which recovery cannot rebuild.
#[test]
fn a_recovered_switch_records_no_auto_switch_state() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: b
    let store = fx.engine.store().unwrap();
    store.set_unhealthy_ticks(&fx.provider(), 2).unwrap();
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    any_mutation(&fx, &a);
    assert_eq!(active(&fx), Some(a));
    assert_eq!(
        store.autoswitch_state(&fx.provider()).unwrap(),
        AutoState {
            unhealthy_ticks: 2,
            ..AutoState::default()
        }
    );
    let last = store.events().unwrap().pop().unwrap();
    assert_eq!(
        (last.kind.as_str(), last.source.as_str()),
        ("switch-recovered", "cli")
    );
}

```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test store --test switch --test strategy --test recover`
Expected: FAIL to compile: unresolved imports `tagteam_engine::store::AutoRecord` and
`tagteam_engine::switch::AutoPerform`, no methods `autoswitch_state` and `set_unhealthy_ticks`,
`SwitchRequest` has no field `auto`, no variants `LiveChanged`, `Cooldown` and `NotCandidate`, and
`commit_switch` takes 3 arguments but 4 were supplied.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/store/mod.rs`, replace

```rust
use serde_json::{Value, json};
use tagteam_core::{AccountId, ProviderId};
```

with

```rust
use serde_json::{Value, json};
use tagteam_core::autoswitch::{AutoState, Departure, Trigger};
use tagteam_core::{AccountId, ProviderId};
```

In `crates/tagteam-engine/src/store/mod.rs`, before

```rust
/// The metadata an explicit replacement installs with its credential (§12.5). It is recorded
```

insert

```rust
/// What an automatic switch's commit records for its provider (§9.4 step 9, §11.2 step 11):
/// when it switched, in epoch seconds, between which accounts, and the departure snapshot of
/// the account it left (§11.3).
#[derive(Debug, Clone, PartialEq)]
pub struct AutoRecord {
    pub at: i64,
    pub from: AccountId,
    pub to: AccountId,
    pub departure: Departure,
}

```

In `crates/tagteam-engine/src/store/mod.rs`, before

```rust
/// Moves one account to a given position: shared by both sides of the swap in `move_to`.
```

insert

```rust
/// Writes an automatic switch's record over the provider's `autoswitch_state` row, creating it
/// if needed, and resets `unhealthy_ticks`: the count judged the account the switch left
/// (Decision 4). `idle_hold_since` is never written (§6.1).
const RECORD_SWITCH_SQL: &str = "INSERT INTO autoswitch_state (provider, last_switch_at, \
    last_switch_from, last_switch_to, left_headroom, left_recovery_at, left_trigger, unhealthy_ticks) \
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0) ON CONFLICT(provider) DO UPDATE SET \
    last_switch_at = excluded.last_switch_at, last_switch_from = excluded.last_switch_from, \
    last_switch_to = excluded.last_switch_to, left_headroom = excluded.left_headroom, \
    left_recovery_at = excluded.left_recovery_at, left_trigger = excluded.left_trigger, \
    unhealthy_ticks = 0";

```

In `crates/tagteam-engine/src/store/mod.rs`, replace

```rust
    /// §9.4 step 9: the active account, the event and the journal row move together.
    pub fn commit_switch(
        &self,
        provider: &ProviderId,
        to: &AccountId,
        event: &EventRow,
    ) -> Result<(), StoreError> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        set_active_on(&tx, provider, Some(to))?;
        Self::insert_event_on(&tx, event)?;
        tx.execute(DELETE_JOURNAL_SQL, [provider.as_str()])?;
        tx.commit()?;
        Ok(())
    }

```

with

```rust
    /// §9.4 step 9: the active account, the event and the journal row move together, and so
    /// does an automatic switch's `record` (§11.2 step 11), which also resets
    /// `unhealthy_ticks`. The record is written first, so any later statement that fails
    /// takes it down too.
    pub fn commit_switch(
        &self,
        provider: &ProviderId,
        to: &AccountId,
        event: &EventRow,
        record: Option<&AutoRecord>,
    ) -> Result<(), StoreError> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        if let Some(r) = record {
            tx.execute(
                RECORD_SWITCH_SQL,
                params![
                    provider.as_str(),
                    r.at,
                    r.from.as_str(),
                    r.to.as_str(),
                    r.departure.left_headroom,
                    r.departure.left_recovery_at,
                    r.departure.left_trigger.as_str(),
                ],
            )?;
        }
        set_active_on(&tx, provider, Some(to))?;
        Self::insert_event_on(&tx, event)?;
        tx.execute(DELETE_JOURNAL_SQL, [provider.as_str()])?;
        tx.commit()?;
        Ok(())
    }

    /// The provider's auto-switch state (§6.1); a provider without a row has the default. A
    /// `left_trigger` this build does not know reads as none, which lifts the no-return bar
    /// (§11.3: a missing departure snapshot lifts it).
    pub fn autoswitch_state(&self, provider: &ProviderId) -> Result<AutoState, StoreError> {
        let c = self.lock();
        let state = c
            .query_row(
                "SELECT last_switch_at, last_switch_from, last_switch_to, left_headroom, \
                 left_recovery_at, left_trigger, unhealthy_ticks \
                 FROM autoswitch_state WHERE provider = ?1",
                [provider.as_str()],
                |r| {
                    Ok(AutoState {
                        last_switch_at: r.get(0)?,
                        last_switch_from: r
                            .get::<_, Option<String>>(1)?
                            .map(AccountId::from_string),
                        last_switch_to: r.get::<_, Option<String>>(2)?.map(AccountId::from_string),
                        left_headroom: r.get(3)?,
                        left_recovery_at: r.get(4)?,
                        left_trigger: r
                            .get::<_, Option<String>>(5)?
                            .as_deref()
                            .and_then(Trigger::parse),
                        unhealthy_ticks: r.get(6)?,
                    })
                },
            )
            .optional()?;
        Ok(state.unwrap_or_default())
    }

    /// §11.2 step 5: the engine's count of ticks in a row whose active usage was unknown. The
    /// rest of the row is left as it is.
    pub fn set_unhealthy_ticks(&self, provider: &ProviderId, n: u32) -> Result<(), StoreError> {
        self.exec(
            "INSERT INTO autoswitch_state (provider, unhealthy_ticks) VALUES (?1, ?2) \
             ON CONFLICT(provider) DO UPDATE SET unhealthy_ticks = excluded.unhealthy_ticks",
            &[&provider.as_str(), &n],
        )?;
        Ok(())
    }

```

In `crates/tagteam-engine/src/switch.rs`, replace

```rust
use tagteam_core::poll::replan_for_role;
```

with

```rust
use tagteam_core::autoswitch::{Departure, Trigger};
use tagteam_core::poll::replan_for_role;
```

In `crates/tagteam-engine/src/switch.rs`, replace

```rust
use crate::store::{AccountRow, EventRow, JournalRow, Store};
```

with

```rust
use crate::store::{AccountRow, AutoRecord, EventRow, JournalRow, Store};
```

In `crates/tagteam-engine/src/switch.rs`, replace

```rust
#[derive(Debug, Clone)]
pub struct SwitchRequest {
    pub provider: ProviderId,
    pub target: SwitchTarget,
    pub force: bool,
    pub source: &'static str,
}
```

with

```rust
#[derive(Debug, Clone)]
pub struct SwitchRequest {
    pub provider: ProviderId,
    pub target: SwitchTarget,
    pub force: bool,
    pub source: &'static str,
    /// An automatic switch's preconditions (§11.2 step 11), checked under the locks before the
    /// first write; `None` for every other switch. Its target is a `SwitchTarget::Account`.
    pub auto: Option<AutoPerform>,
}

/// What an automatic switch rests on (§11.2 step 11): the live account the tick decided on,
/// its trigger, the cooldown to judge it by, and the departure snapshot its commit records for
/// the account it leaves (§11.3).
#[derive(Debug, Clone)]
pub struct AutoPerform {
    pub expected_from: AccountId,
    pub trigger: Trigger,
    pub cooldown_s: i64,
    pub departure: Departure,
}
```

In `crates/tagteam-engine/src/switch.rs`, replace

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchReason {
    Switched,
    AlreadyActive,
    Activated,
    UnmanagedAccount,
    OnlyOneAccount,
    NoValidTarget,
    UsageUnavailable,
    AlreadyBest,
    CandidatesExhausted,
}

impl SwitchReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SwitchReason::Switched => "switched",
            SwitchReason::AlreadyActive => "already-active",
            SwitchReason::Activated => "activated",
            SwitchReason::UnmanagedAccount => "unmanaged-account",
            SwitchReason::OnlyOneAccount => "only-one-account",
            SwitchReason::NoValidTarget => "no-valid-target",
            SwitchReason::UsageUnavailable => "usage-unavailable",
            SwitchReason::AlreadyBest => "already-best",
            SwitchReason::CandidatesExhausted => "candidates-exhausted",
        }
    }
}
```

with

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchReason {
    Switched,
    AlreadyActive,
    Activated,
    UnmanagedAccount,
    OnlyOneAccount,
    NoValidTarget,
    UsageUnavailable,
    AlreadyBest,
    CandidatesExhausted,
    /// An automatic switch's live account is no longer the one its tick decided on.
    LiveChanged,
    /// An automatic `proactive` or `consume-first` switch met the cooldown under the lock.
    Cooldown,
    /// An automatic switch's target is no longer a candidate (§11.2 step 7).
    NotCandidate,
}

impl SwitchReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SwitchReason::Switched => "switched",
            SwitchReason::AlreadyActive => "already-active",
            SwitchReason::Activated => "activated",
            SwitchReason::UnmanagedAccount => "unmanaged-account",
            SwitchReason::OnlyOneAccount => "only-one-account",
            SwitchReason::NoValidTarget => "no-valid-target",
            SwitchReason::UsageUnavailable => "usage-unavailable",
            SwitchReason::AlreadyBest => "already-best",
            SwitchReason::CandidatesExhausted => "candidates-exhausted",
            SwitchReason::LiveChanged => "live-changed",
            SwitchReason::Cooldown => "cooldown",
            SwitchReason::NotCandidate => "not-candidate",
        }
    }
}
```

In `crates/tagteam-engine/src/switch.rs`, after

```rust
const NO_LIVE: &str =
    "switching to the best known candidate; there is no managed live login to compare with";
```

insert

```rust
/// §11.2 step 11: the live account is no longer the one the tick decided on.
const LIVE_CHANGED: &str = "the live login changed after this switch was decided; a switch made meanwhile is never overridden";
```

In `crates/tagteam-engine/src/switch.rs`, replace

```rust
    /// a walk that finds none to activate is `already-best` (or `usage-unavailable` when the
    /// live headroom is unknown).
```

with

```rust
    /// a walk that finds none to activate is `already-best`; with the live headroom unknown, it
    /// is `candidates-exhausted` when candidates known to be at their limit were left out, and
    /// `usage-unavailable` otherwise.
```

In `crates/tagteam-engine/src/switch.rs`, replace

```rust
                        Some(live) if live_headroom.is_some() => Ranked::Stay(
                            SwitchReason::AlreadyBest,
                            format!(
                                "no candidate with more headroom than {} holds a stored credential",
                                live.described(models)
                            ),
                            notes,
                        ),
                        _ => Ranked::Stay(
                            SwitchReason::UsageUnavailable,
```

with

```rust
                        Some(live) if live_headroom.is_some() => Ranked::Stay(
                            SwitchReason::AlreadyBest,
                            format!(
                                "no candidate with more headroom than {} holds a stored credential",
                                live.described(models)
                            ),
                            notes,
                        ),
                        // §9.3: no candidate that beat the exhausted ones holds a stored
                        // credential, so none of them is switchable; what is left is known to
                        // be at its limit, as `next-available` reports it.
                        _ if !exhausted.is_empty() => {
                            let exhausted: Vec<&Rated> = exhausted.iter().filter_map(at).collect();
                            Ranked::Stay(
                                SwitchReason::CandidatesExhausted,
                                self.exhausted_message(&exhausted, models),
                                notes,
                            )
                        }
                        _ => Ranked::Stay(
                            SwitchReason::UsageUnavailable,
```

In `crates/tagteam-engine/src/switch.rs`, before

```rust
    /// The target and the §9.2 special cases, decided from the current state.
```

insert

```rust
    /// §11.2 step 11: an automatic switch's preconditions, in the spec's order. `None` when
    /// every one holds, or for any other switch; otherwise the no-op that says which failed.
    /// `live_row` is the live account as just read. Planning runs it first, so a refused switch
    /// spends no oracle request; `rederive` runs it again under every lock, where it decides
    /// (§9.4 step 1). Nothing is written either way.
    fn auto_refusal(
        &self,
        store: &Store,
        req: &SwitchRequest,
        live_row: Option<&AccountRow>,
    ) -> Result<Option<SwitchOutcome>, EngineError> {
        let Some(auto) = &req.auto else {
            return Ok(None);
        };
        let SwitchTarget::Account(id) = &req.target else {
            return Err(EngineError::InvalidInput(
                "an automatic switch names its target".into(),
            ));
        };
        let refuse = |reason: SwitchReason, message: String| {
            Ok(Some(noop(
                "direct",
                reason,
                message,
                live_row.cloned(),
                None,
            )))
        };
        // A manual switch made meanwhile, a login made in Claude Code, or a logout.
        if live_row.map(|r| &r.id) != Some(&auto.expected_from) {
            return refuse(SwitchReason::LiveChanged, LIVE_CHANGED.into());
        }
        // §11.2 step 6, judged from the state as read now: the engine is its one writer, and
        // the check and the record are both made under `MutationGuard`.
        if matches!(auto.trigger, Trigger::Proactive | Trigger::ConsumeFirst) {
            let now_s = self.now_ms().div_euclid(1000);
            if let Some(at) = store.autoswitch_state(&req.provider)?.last_switch_at {
                let ends = at.saturating_add(auto.cooldown_s);
                if now_s < ends {
                    return refuse(
                        SwitchReason::Cooldown,
                        format!(
                            "the cooldown after the last automatic switch has {} left",
                            span(ends - now_s)
                        ),
                    );
                }
            }
        }
        // §11.2 step 7: switchable (a vault credential and an identity, not disabled), not
        // quarantined, not session-owned (never, before M4).
        let Some(target) = store.account(id)?.filter(|a| a.provider == req.provider) else {
            return refuse(
                SwitchReason::NotCandidate,
                "the account to switch to was removed".into(),
            );
        };
        let why = if target.disabled {
            Some("it is disabled")
        } else if target.quarantine_reason.is_some() {
            Some("it needs a new login")
        } else if !self.has_login(&target)? {
            Some("it has no stored credential")
        } else {
            None
        };
        match why {
            Some(why) => refuse(
                SwitchReason::NotCandidate,
                format!(
                    "{} (position {}) is no longer a candidate: {why}",
                    target.label, target.position
                ),
            ),
            None => Ok(None),
        }
    }

```

In `crates/tagteam-engine/src/switch.rs`, replace

```rust
        let (live, live_row) = self.live_row(p, store, &req.provider)?;
        let unmanaged_email = match (&live, &live_row) {
```

with

```rust
        let (live, live_row) = self.live_row(p, store, &req.provider)?;
        if let Some(refused) = self.auto_refusal(store, req, live_row.as_ref())? {
            return Ok(Planned::Done(refused));
        }
        let unmanaged_email = match (&live, &live_row) {
```

In `crates/tagteam-engine/src/switch.rs`, replace

```rust
            Planned::Done(outcome) => return Ok(outcome),
            Planned::Go(plan) => match self.freshen_plan(p, store, req, plan)? {
```

with

```rust
            Planned::Done(outcome) => return Ok(outcome),
            // §11.2 step 10: the tick has freshened an automatic switch's target by its own
            // table; freshening here again would act on the manual table instead.
            Planned::Go(plan) if req.auto.is_some() => plan,
            Planned::Go(plan) => match self.freshen_plan(p, store, req, plan)? {
```

In `crates/tagteam-engine/src/switch.rs`, replace

```rust
        let (live_identity, again) = self.live_row(p, store, &req.provider)?;
        // A login that became unmanaged is §9.2's no-op; a target removed meanwhile is
```

with

```rust
        let (live_identity, again) = self.live_row(p, store, &req.provider)?;
        // §9.4 step 1: an automatic switch re-checks its preconditions here, before anything is
        // written. A refusal ends the switch; it never falls through to a plan made elsewhere.
        if let Some(refused) = self.auto_refusal(store, req, again.as_ref())? {
            return Ok(Rederived::Done(refused));
        }
        // A login that became unmanaged is §9.2's no-op; a target removed meanwhile is
```

In `crates/tagteam-engine/src/switch.rs`, replace

```rust
        hooks::point(self, "after-identity")?;
        tx.store.commit_switch(
            &req.provider,
            &target.id,
            &EventRow {
                at: self.now_ms(),
                provider: req.provider.clone(),
                kind: "switch".into(),
                from_id: outgoing.map(|o| o.id.clone()),
                to_id: Some(target.id.clone()),
                trigger: Some(
                    if req.source == "auto" {
                        "auto"
                    } else {
                        "manual"
                    }
                    .into(),
                ),
                source: req.source.into(),
                detail: None,
            },
        )?;
        Ok(stored_in)
    }
}
```

with

```rust
        hooks::point(self, "after-identity")?;
        // §11.2 step 11: an automatic switch records its departure with the commit; its
        // preconditions have made the outgoing account the one it decided on.
        let record = req
            .auto
            .as_ref()
            .zip(outgoing)
            .map(|(auto, from)| AutoRecord {
                at: self.now_ms().div_euclid(1000),
                from: from.id.clone(),
                to: target.id.clone(),
                departure: auto.departure.clone(),
            });
        tx.store.commit_switch(
            &req.provider,
            &target.id,
            &EventRow {
                at: self.now_ms(),
                provider: req.provider.clone(),
                kind: "switch".into(),
                from_id: outgoing.map(|o| o.id.clone()),
                to_id: Some(target.id.clone()),
                trigger: Some(
                    req.auto
                        .as_ref()
                        .map_or("manual", |a| a.trigger.as_str())
                        .into(),
                ),
                source: req.source.into(),
                detail: None,
            },
            record.as_ref(),
        )?;
        Ok(stored_in)
    }
}
```

In `crates/tagteam-engine/src/recover.rs`, replace

```rust
                trigger: Some("recovery".into()),
                source: "cli".into(),
                detail: None,
            },
        )?;
```

with

```rust
                trigger: Some("recovery".into()),
                source: "cli".into(),
                detail: None,
            },
            // Never an auto-switch record (Task 7's ruling): the row names neither the
            // switch's source nor its trigger, and the departure snapshot was the dead tick's
            // view of usage, which cannot be rebuilt now.
            None,
        )?;
```

Every other `SwitchRequest` literal gets `auto: None`:

In `crates/tagteam-engine/tests/common/mod.rs`, after

```rust
            target: SwitchTarget::Account(id.clone()),
            force,
            source: "cli",
```

insert

```rust
            auto: None,
```

In `crates/tagteam-engine/tests/common/mod.rs`, after

```rust
            target: SwitchTarget::Rotation,
            force,
            source: "cli",
```

insert

```rust
            auto: None,
```

In `crates/tagteam-engine/tests/common/mod.rs`, after

```rust
                target: SwitchTarget::Account(id.clone()),
                force: false,
                source: "cli",
```

insert

```rust
                auto: None,
```

In `crates/tagteam-engine/tests/fake_agent.rs`, after

```rust
                target: SwitchTarget::Account(bob.clone()),
                force: true,
                source: "cli",
```

insert

```rust
                auto: None,
```

In `crates/tagteam-engine/tests/fake_agent.rs`, after

```rust
            target: SwitchTarget::Account(alice.clone()),
            force: false,
            source: "cli",
```

insert

```rust
            auto: None,
```

In `crates/tagteam-engine/tests/switch_rollback.rs`, after

```rust
        target,
        force,
        source: "cli",
```

insert

```rust
        auto: None,
```

In `crates/tagteam-engine/tests/strategy.rs`, after

```rust
        target,
        force: false,
        source: "cli",
```

insert

```rust
        auto: None,
```

In `crates/tagteam-engine/tests/switch.rs`, after

```rust
        target,
        force,
        source: "cli",
```

insert

```rust
        auto: None,
```

In `crates/tagteam/src/app.rs`, after

```rust
            target: target.clone(),
            force,
            source: "cli",
```

insert

```rust
            auto: None,
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --features test-hooks --test store --test switch --test strategy --test recover`
Expected: PASS. `store` 27 (4 new), `switch` 57 with 1 ignored (8 new, one of them under
`test-hooks`), `strategy` 35 (1 new), `recover` 35 (1 new). Before Step 3's new `best_pick` arm,
`best_names_the_exhausted_candidates_when_the_healthy_one_holds_no_credential` fails with
`left: (false, UsageUnavailable, "best")`; without the `rederive` call,
`a_manual_switch_between_the_decision_and_the_perform_is_never_overridden` switches
(`left: (true, Switched, "switched")`); and with `freshen_plan` left in,
`an_automatic_switch_leaves_freshening_to_the_tick` sees one token request.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS, apart from the tagteam lib's pseudo-terminal prompt tests and
`signals::ctrl_c_while_add_token_reads_a_terminal_stdin_exits_130_without_waiting_for_enter`
inside Claude Code's sandbox, which blocks `/dev/ptmx` (Execution notes).

- [ ] **Step 5: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/store/mod.rs crates/tagteam-engine/src/switch.rs \
  crates/tagteam-engine/src/recover.rs crates/tagteam/src/app.rs \
  crates/tagteam-engine/tests/common/mod.rs crates/tagteam-engine/tests/fake_agent.rs \
  crates/tagteam-engine/tests/switch_rollback.rs crates/tagteam-engine/tests/strategy.rs \
  crates/tagteam-engine/tests/switch.rs crates/tagteam-engine/tests/store.rs \
  crates/tagteam-engine/tests/recover.rs
git commit -m "Check an automatic switch's preconditions under the locks and record its departure in the commit"
```

---

### Task 8: `AutoEngine`: the engine lock, the tick and its events

§11.1: "An engine holds its provider's engine lock, `locks/autoswitch-<provider>.lock` (§5), for
its whole life. The lock is a `flock` that is only ever tried, never waited for, and that the
kernel releases when the holder dies." M5's signed-off amendment (`86882e3` on `m5-admin`) adds
one rule to §11.1:

> Once it holds the lock, the engine writes its pid and start time into the lock file, the start
> time taken as for the `switch_journal` holder (§12.6). The record is never used for exclusion.

This task builds `AutoEngine` in `crates/tagteam-engine/src/auto.rs`. `Engine::auto` takes the
lock and writes that record; `tick` runs §11.2's steps 1 to 12 in order and reports through an
`EventSink` the §11.4 events that the CLI renders (Task 10). The loop that schedules ticks is
Task 9's.

**Readings of the spec this task commits to.** The engine:
- **`Engine::auto`** checks the provider. Unless in dry-run, it refuses inside a `tagteam run`
  shell (§11.1 "Where it runs") and tries the lock once: `Ok(None)` while another process holds
  it. Holding the lock, it replaces the file's whole content through a second descriptor:
  `set_len(0)`, then `{"pid":<u32>,"start":<u64>}` and a newline from `ProcessStamp::current()`.
  A longer record a dead engine left is replaced whole. The record is written only here.
- **Dry-run** takes no lock, creates no lock file, writes no auto-switch state and releases no
  quarantine (§11.1). It reads `autoswitch_state` like a real engine, so the cooldown and the bar
  are the real ones, and keeps its own `unhealthy_ticks` in memory. Its collection is the
  ordinary one, and it may finish a recovery (§11.1).

The tick:
- **Step 1.** `release_unbound_quarantines(provider, "auto")`, except in dry-run. Then Decision
  10's comparison with the previous tick's map of each account's quarantine and `login_epoch`:
  - the tick's own releases, and every quarantine cleared since, give `account-unquarantined`:
    `account-replaced` when the epoch moved since the previous tick, else `credentials-replaced`;
  - every quarantine set since gives `account-quarantined` with the stored `quarantine_reason`;
  - the first tick has no map, so it reports only its own releases;
  - quarantines the tick's own work finds (the collection's `CollectReport.quarantined`, step
    10's freshening) are reported when found and entered in the map, so each is reported once.
- **Step 2.** `settle_or_refuse_as(provider, "auto")`: recovery under `MutationGuard`, whose
  `switch-recovered` event carries `source` `auto`. A row that stays gives `no-switch
  interrupted-switch` (BLOCKED) with recovery's refusal as its detail: `InterruptedSwitch`,
  `RecoveryBlocked` or `RecoveryMoved`, as today, or the mutation lock's timeout, since then
  recovery could not run yet either. Then the live check: no live login is `no-active-account`,
  an unmanaged one `unmanaged-active-account` (NO_ACTION). Both return before collecting, so such
  a tick writes nothing (Review Focus 5).
- **Step 3.** `CollectMode::Scheduled` with the tick's threshold and models, then `poll`. Then
  the settings checks, each once per engine and again after `set_config` changes its setting, as
  `config-warning`: a consume-first strategy on a provider without a long window (§4.5: "with one
  `config-warning`"), and each configured model name other than `all` that matches no scoped
  window of any account's stored reading, case-insensitively.
- **Steps 4 to 9.** `decide` over a snapshot built from the store: each account's decision-grade
  windows (`decision_windows` under the tick's models), its `fetched_at` as stored, and `api_key`
  from its kind's `managed_key_axis`. A consume-first `recheck` collects `CollectMode::Recheck`
  for the live account and every OAuth candidate, rebuilds the snapshot, and decides again with
  `Phase::Rechecked` and the same state (Decision 2). Its targets are those Task 5's `decide`
  keeps, so a candidate whose re-check fetch failed, its reading still stale, is never tried.
- **Steps 10 and 11.** For each target in order, dry-run emits `switch` with `dry_run` true for
  the first and stops. Otherwise `freshen_auto`, then `switch` with `source` `auto` and an
  `AutoPerform` whose departure is `autoswitch::departure` over the tick's snapshot.
  `LiveChanged` gives `no-switch live-changed`, `Cooldown` gives `no-switch cooldown` with the
  time left, and `NotCandidate` moves to the next target.
- **Step 12.** Every target failed: `error` "could not freshen <label> (position <n>): <why>; …"
  (transient) when any of them failed transiently or systemically, else `no-switch
  no-viable-target` (BLOCKED).
- **Errors.** `tick` returns `Err` only for an interruption, in any carrier
  (`EngineError::signal()`), or a store failure (`EngineError::Store`). A cancellation point opens
  each tick (§14.1: "between its ticks"). A switch that committed despite a signal inside its
  critical span is `Switched`; the token stays set for the caller's next check (M3a's rule).
- **The tick's end.** `tick` calls `sink.tick_done(provider)` exactly once, after everything
  else, on every path: a decision, an `error` tick and an `Err` alike. `run` holds the tick's
  steps; `tick` wraps it. A sink that keeps something for a tick's line lets it go there (Task
  10's human sink, its poll).

Rulings where the spec is silent (what; why; cost if wrong):
- **`switchable` comes from the store alone** (enabled, with an identity). The vault is read
  lazily at step 10, where an empty one passes the target over. Why: §9.3 reads vaults lazily,
  and a Keychain read per account per tick is a `security` spawn each. Cost: a tick reports
  `no-viable-target` where an eager read would have said `no-candidates`; both are BLOCKED.
- **`freshen_auto`'s reading of the gate (§7.3) by §11.2 step 10's table:**
  - perform on `Refreshed`, `AlreadyFresh`, `Busy` (the switch's account lock and pending-rescue
    settle pick the other refresh up, as §7.2 says for a manual switch), and on `Owned` by the
    live login or a journal row (the switch's own checks under its locks decide);
  - quarantined on `Dead` and on `Unpersisted` (`successor_lost`);
  - failed on `Transient` and `Systemic`, and on `Transient { rescued: true }`, whose spent
    generation must not be activated;
  - passed over on `Owned` by a session and on `Conflict` (M4), and on an empty vault;
  - a vault that cannot be read, or an error from the gate other than an interruption or a store
    failure, is a failure.
  Cost: a transient gate result moves on to the next target instead of activating the vault's
  generation with a warning, as a manual switch would.
- **`unhealthy_ticks` is written after the tick only when it changed**, and never after a
  switch, whose commit reset it. A fresh store gets no row from healthy ticks.
- **The count belongs to the account it judged** (Decision 4). When the live account is not the
  one the previous tick judged (a manual switch, a recovery, a login in Claude Code), the count
  starts from 0. A fresh engine trusts the stored count, so a `--once` run can still reach
  `failover`. Cost: a `--once` after a manual switch inherits the old account's count once.
- **Step 12 is an `error` when any target failed transiently or systemically.** Why: a retry at
  the normal cadence is what a transient failure needs. Cost: a tick that mixes such a failure
  with quarantined or passed-over targets reports `error`, not `no-viable-target`.
- **A collection's warnings** (a lost successor, a failed refresh) are `error` events with
  `transient` true that do not fail the tick (§8.3: a usage failure is never a command error). A
  perform error's `transient` is true only for a lock timeout.
- **A tick that fails before deciding** returns `TickOutcome::Error` with `Decision::NoSwitch {
  no-active-account, NO_ACTION, detail: the error }`. Why: the contract returns a decision, and
  NO_ACTION keeps the loop's normal cadence (§11.4: "the loop goes on after the normal delay").
  Task 10 renders the `error` event, never this decision.
- **`all-exhausted`** emits `no-switch` (with the span as its detail), then `all-exhausted`
  (with `earliestResetAt`).
- **A dry-run's would-be switch** is `TickOutcome::Switched` (so `--once --dry-run` exits 0),
  names its first target only, and records no departure: the live account has not moved, so each
  dry tick shows what auto would do now.
- **`poll`:** `headroom_pct` holds every account of the provider, `None` when unknown;
  `fetch_errors` holds each `Failed { kind }`, `error` included, and `over-budget`; `windows_pct`
  holds each decision-grade reading's windows by key. Events name an account by its email, or
  its label when it has none (FakeAgent: `alice@ws`).
- **`active_next_poll_at()`** is the later of the live account's `next_poll_at` and its usage
  lease's expiry, rounded up to the second (Task 3's controller ruling: a lease outlives its
  record). It is `None` before a tick has seen a managed live account, without a plan, or on a
  store error.

Supporting changes:
- **Recovery's event `source` is threaded through:** `recover_one` and `finish_forward` take it.
  `guard_or_refuse` and `settle_or_refuse` keep their signatures and pass `cli`; their `_as`
  variants take a source; and `switch()` recovers with its request's source.
- **`Store::usage_lease_expires_at`**, for `active_next_poll_at`.

**Files:**
- Create: `crates/tagteam-engine/src/auto.rs`
- Modify: `crates/tagteam-engine/src/lib.rs` (`pub mod auto`)
- Modify: `crates/tagteam-engine/src/engine.rs` (`guard_or_refuse_as`, `settle_or_refuse_as`,
  `guard_recovering`'s source)
- Modify: `crates/tagteam-engine/src/recover.rs` (`recover_one`, `finish_forward`)
- Modify: `crates/tagteam-engine/src/switch.rs` (`AutoFreshened`, `freshen_auto`, the switch's
  recovery source)
- Modify: `crates/tagteam-engine/src/store/usage.rs` (`usage_lease_expires_at`)
- Modify: `crates/tagteam-engine/tests/common/mod.rs` (`Recorded`, `usage_window`,
  `record_reading`)
- Create: `crates/tagteam-engine/tests/auto_tick.rs`, `crates/tagteam-engine/tests/auto_engine.rs`

**Interfaces:**
- Consumes:
  - Task 3: `CollectMode::{Scheduled, Recheck}`, `CollectReport.{outcomes, warnings,
    quarantined}`, `Collected`.
  - Tasks 4 and 5: `decide`, `departure`, `Decision`, `Phase`, `NoSwitchReason::outcome`,
    `AutoConfig::effective_strategy`, `AutoState`, `Snapshot`, `AccountSnapshot`, `Live`.
  - Task 7: `SwitchRequest.auto`, `AutoPerform`, `SwitchReason::{LiveChanged, Cooldown,
    NotCandidate}`, `Store::{autoswitch_state, set_unhealthy_ticks}`.
  - Existing: `release_unbound_quarantines`, `decision_windows`, `read_live_identity`,
    `refresh_stored`, `due`, `check_cancel`, `refuse_inside_run_shell`, `FlockGuard::try_lock`,
    `ProcessStamp::current`, `rank::span`; the fixtures `Fx`, `FakeFx`, `crashed_switch`,
    `write_target_credential`, `usage_requests`, `usage_bearers`, `vault_fp`, `API_KEY` and
    `Engine::on_point`.
- Produces:
  - `tagteam_engine::auto::{EventSink, AutoEvent, TickOutcome, AutoEngine}`, exactly as the
    Interface Contract states them: `EventSink` has `emit` and the default no-op
    `tick_done(&self, _provider: &ProviderId)`. `AutoEvent` derives `Debug, Clone, PartialEq`;
    `Sleep` is the loop's (Task 9), never a tick's.
  - `Engine::auto(&self, provider: &ProviderId, cfg: AutoConfig, dry_run: bool) ->
    Result<Option<AutoEngine<'_>>, EngineError>`; `Err(InsideRunShell)` for a real engine inside
    a run shell.
  - `AutoEngine::tick(&mut self, sink: &dyn EventSink) -> Result<(TickOutcome, Decision),
    EngineError>`, `AutoEngine::set_config(&mut self, cfg: AutoConfig)`,
    `AutoEngine::active_next_poll_at(&self) -> Option<i64>`
  - `Store::usage_lease_expires_at(&self, id: &AccountId) -> Result<Option<i64>, StoreError>`
    (epoch ms; contract addition)
  - test fixtures: `common::{Recorded, usage_window, record_reading}`
  - crate-private: `switch::AutoFreshened::{Ready, Quarantined, Failed(String), Skip}`,
    `Engine::freshen_auto`, `Engine::{guard_or_refuse_as, settle_or_refuse_as}`, and the
    `source` parameter of `recover_one` and `finish_forward`

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/common/mod.rs`, replace

```rust
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::oracle::Oracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::{JournalRow, LoginMeta, NewAccount, Store};
```

with

```rust
use tagteam_core::{AccountId, CLAUDE_CODE, PollBudget, PollPlan, ProviderId, Window, WindowKind};
use tagteam_engine::auto::{AutoEvent, EventSink};
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::oracle::Oracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::{Eligibility, JournalRow, LoginMeta, NewAccount, Reserve, Store};
```

Append to the end of `crates/tagteam-engine/tests/common/mod.rs`:

```rust
/// An auto-switch event sink that keeps every event it is given, in order (§11.4).
#[derive(Default)]
pub struct Recorded(Mutex<Vec<AutoEvent>>);

impl EventSink for Recorded {
    fn emit(&self, e: &AutoEvent) {
        self.0.lock().unwrap().push(e.clone());
    }
}

impl Recorded {
    /// The events emitted since the last call, oldest first.
    pub fn take(&self) -> Vec<AutoEvent> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

/// A usage window of `kind` at `pct` that resets at `resets_at`, labelled as Claude Code
/// labels its windows (`scoped:Fable` is `Fable`).
pub fn usage_window(key: &str, kind: WindowKind, pct: f64, resets_at: i64) -> Window {
    Window {
        key: key.into(),
        label: key.strip_prefix("scoped:").unwrap_or(key).into(),
        kind,
        pct,
        resets_at: Some(resets_at),
        period_s: match kind {
            WindowKind::Short => Some(18_000),
            WindowKind::Spend => None,
            _ => Some(604_800),
        },
        detail: None,
    }
}

/// Records `windows` as `id`'s reading taken at `at` (epoch seconds), its next poll planned at
/// `next_poll_at`, through `engine`'s store as the collector records one: reserve (§8.3 phase
/// 1, which counts a slot of the hourly budget and leaves a 90 s lease), then record (phase 3).
pub fn record_reading(
    engine: &Engine,
    id: &AccountId,
    windows: &[Window],
    at: i64,
    next_poll_at: i64,
) {
    let store = engine.store().unwrap();
    let row = store.account(id).unwrap().unwrap();
    let r = match store
        .reserve_usage(
            &row,
            at * 1000,
            Eligibility::Scheduled,
            &PollBudget::STANDARD,
        )
        .unwrap()
    {
        Reserve::Reserved(r) => r,
        other => panic!("not reserved at {at}: {other:?}"),
    };
    let plan = PollPlan {
        interval_s: next_poll_at - at,
        next_poll_at,
    };
    assert!(store.record_usage(&r, windows, at, &plan, 180).unwrap());
}
```

Create `crates/tagteam-engine/tests/auto_engine.rs`:

```rust
//! §11.1's engine: one per provider per machine, held by a try-only lock that records its
//! holder; dry-run holds no lock and writes no auto-switch state; the live account's next
//! fetchable time for the loop.
mod common;

use std::fs;

use common::{FakeFx, Fx, Recorded, record_reading, usage_window};
use serde_json::Value;
use tagteam_core::autoswitch::{AutoConfig, AutoState, Decision, NoSwitchReason, Strategy};
use tagteam_core::{ProviderId, WindowKind};
use tagteam_engine::EngineError;
use tagteam_engine::auto::{AutoEvent, TickOutcome};
use tagteam_provider::{ProcessStamp, Provider};

const T0: i64 = 1_790_000_000;

fn cfg(p: &dyn Provider) -> AutoConfig {
    AutoConfig {
        threshold: 90.0,
        hysteresis_pct: 10.0,
        cooldown_s: 300,
        interval_s: 60,
        unhealthy_ticks: 3,
        strategy: Strategy::Best,
        include_api_key_accounts: false,
        models: Vec::new(),
        long_window: p.primary_long_window().map(str::to_owned),
    }
}

fn lock_path(fx: &Fx, provider: &ProviderId) -> std::path::PathBuf {
    fx.env
        .data_dir()
        .join("locks")
        .join(format!("autoswitch-{provider}.lock"))
}

/// Review Focus 3, the engine half: a second engine for one provider, in this process or
/// another, gets `None` while the first lives, and the lock once it is dropped. Another
/// provider's engine is independent.
#[test]
fn a_second_engine_for_one_provider_gets_none_while_the_first_lives() {
    let ffx = FakeFx::new();
    let cc = ffx.fx.provider();
    let config = cfg(ffx.fx.cc.as_ref());
    let first = ffx.engine.auto(&cc, config.clone(), false).unwrap();
    assert!(first.is_some());
    assert!(
        ffx.engine
            .auto(&cc, config.clone(), false)
            .unwrap()
            .is_none()
    );
    let elsewhere = ffx.fx.engine_with_env(ffx.fx.env.clone());
    assert!(
        elsewhere
            .auto(&cc, config.clone(), false)
            .unwrap()
            .is_none()
    );
    let fake = ffx.fake_provider();
    let fake_engine = ffx
        .engine
        .auto(&fake, cfg(ffx.fake.as_ref()), false)
        .unwrap();
    assert!(fake_engine.is_some(), "another provider has its own lock");
    drop(first);
    assert!(elsewhere.auto(&cc, config, false).unwrap().is_some());
}

#[test]
fn the_engine_lock_records_its_holder_as_one_json_line() {
    // M5's amendment (`86882e3`): pid and start time, the start taken as for the journal's
    // holder (§12.6). A longer record left by a dead engine is replaced whole.
    let fx = Fx::new();
    let path = lock_path(&fx, &fx.provider());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "{\"pid\":4294967295,\"start\":18446744073709551615,\"note\":\"a dead engine's\"}\n",
    )
    .unwrap();
    let engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let me = ProcessStamp::current().unwrap();
    let bytes = fs::read(&path).unwrap();
    assert_eq!(
        String::from_utf8(bytes.clone()).unwrap(),
        format!("{{\"pid\":{},\"start\":{}}}\n", me.pid, me.start)
    );
    let record: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(record["pid"].as_u64(), Some(u64::from(me.pid)));
    assert_eq!(record["start"].as_u64(), Some(me.start));
    drop(engine);
}

#[test]
fn a_real_engine_refuses_inside_a_run_shell_and_a_dry_run_does_not() {
    // §11.1: like every command that changes the live login (§9.2).
    let fx = Fx::new();
    let mut env = fx.env.clone();
    env.claude_config_dir = Some(fx.env.data_dir().join("sessions/x").into_os_string());
    let engine = fx.engine_with_env(env);
    assert!(matches!(
        engine.auto(&fx.provider(), cfg(fx.cc.as_ref()), false),
        Err(EngineError::InsideRunShell)
    ));
    assert!(!lock_path(&fx, &fx.provider()).exists());
    let dry = engine.auto(&fx.provider(), cfg(fx.cc.as_ref()), true);
    assert!(dry.unwrap().is_some());
}

#[test]
fn dry_run_takes_no_lock_and_keeps_its_state_in_memory() {
    // §11.1: no engine lock, no auto-switch state, no quarantine released. Its count of
    // unknown ticks lives in memory.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live, never read
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    let mut dry = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), true)
        .unwrap()
        .unwrap();
    assert!(!lock_path(&fx, &fx.provider()).exists());
    let real = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap();
    assert!(
        real.is_some(),
        "a dry run holds nothing a real engine needs"
    );
    drop(real);
    let sink = Recorded::default();
    for n in 1..=2 {
        let (outcome, decision) = dry.tick(&sink).unwrap();
        assert_eq!(outcome, TickOutcome::NoAction);
        assert!(
            matches!(
                &decision,
                Decision::NoSwitch { reason: NoSwitchReason::ActiveUsageUnknown, detail, .. }
                    if *detail == format!("{n}/3")
            ),
            "{decision:?}"
        );
    }
    let store = fx.engine.store().unwrap();
    assert_eq!(
        store.autoswitch_state(&fx.provider()).unwrap(),
        AutoState::default()
    );
    let row = store.account(&a).unwrap().unwrap();
    assert_eq!(row.quarantine_reason.as_deref(), Some("invalid_grant"));
    assert!(
        !sink
            .take()
            .iter()
            .any(|e| matches!(e, AutoEvent::AccountUnquarantined { .. }))
    );
}

#[test]
fn the_next_poll_is_the_later_of_the_plan_and_the_usage_lease() {
    // Task 3's controller ruling: a §8.3 lease outlives its record, and a tick that wakes
    // inside it cannot fetch. Each reading below leaves its 90 s lease, to T0 + 90.
    let reading = |pct| {
        vec![
            usage_window("5h", WindowKind::Short, pct, T0 + 9_630),
            usage_window("7d", WindowKind::Long, pct, T0 + 291_630),
        ]
    };
    // The urgent band's 60 s plan ends inside the lease; a 300 s one outlasts it.
    for (planned, next) in [(T0 + 60, T0 + 90), (T0 + 300, T0 + 300)] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b"); // live
        record_reading(&fx.engine, &a, &reading(10.0), T0, T0 + 300);
        record_reading(&fx.engine, &b, &reading(20.0), T0, planned);
        let mut engine = fx
            .engine
            .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
            .unwrap()
            .unwrap();
        assert_eq!(
            engine.active_next_poll_at(),
            None,
            "no tick has seen the live account yet"
        );
        engine.tick(&Recorded::default()).unwrap();
        assert_eq!(
            engine.active_next_poll_at(),
            Some(next),
            "planned {planned}"
        );
    }
}
```

Create `crates/tagteam-engine/tests/auto_tick.rs`:

```rust
//! §11.2's tick and §11.4's events, against Claude Code and the test-only FakeAgent (§15.2).
//! FakeAgent names no long window, so its consume-first setting runs `best`. Readings are
//! recorded through the store's own reserve and record, with a plan in force, so a tick's
//! scheduled collection finds nothing due unless a test says otherwise.
mod common;

use std::collections::BTreeMap;
use std::fs;
use std::sync::Mutex;

use common::{
    API_KEY, FakeFx, Fx, Recorded, crashed_switch, record_reading, usage_requests, usage_window,
    vault_fp, write_target_credential,
};
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_core::autoswitch::{
    AutoConfig, AutoState, Decision, NoSwitchReason, Outcome, Strategy, Trigger,
};
use tagteam_core::{AccountId, ProviderId, Window, WindowKind};
use tagteam_engine::Engine;
use tagteam_engine::auto::{AutoEvent, EventSink, TickOutcome};
use tagteam_engine::vault::SERVICE;
use tagteam_fake::FakePaths;
use tagteam_provider::{Keychain, Provider};

/// The fixture clock's start (`Fx`), in seconds.
const T0: i64 = 1_790_000_000;
/// When every reading's short window resets.
const SHORT_RESETS: i64 = T0 + 9_630;
/// When every reading's long window resets, unless a test says otherwise.
const LONG_RESETS: i64 = T0 + 291_630;

fn cfg(p: &dyn Provider) -> AutoConfig {
    AutoConfig {
        threshold: 90.0,
        hysteresis_pct: 10.0,
        cooldown_s: 300,
        interval_s: 60,
        unhealthy_ticks: 3,
        strategy: Strategy::Best,
        include_api_key_accounts: false,
        models: Vec::new(),
        long_window: p.primary_long_window().map(str::to_owned),
    }
}

/// A reading of `short` and `long` percent in the provider's own windows: Claude Code's 5h
/// and 7d, or FakeAgent's daily and monthly.
fn reading(fake: bool, short: f64, long: f64) -> Vec<Window> {
    let (s, l) = if fake {
        ("daily", "monthly")
    } else {
        ("5h", "7d")
    };
    vec![
        usage_window(s, WindowKind::Short, short, SHORT_RESETS),
        usage_window(l, WindowKind::Long, long, LONG_RESETS),
    ]
}

/// A reading taken now with its next poll five minutes out: decision-grade, and not due.
fn read(engine: &Engine, id: &AccountId, windows: &[Window]) {
    record_reading(engine, id, windows, T0, T0 + 300);
}

/// The windows of a reading as `poll`'s `windowsPct` gives them.
fn pcts(windows: &[Window]) -> BTreeMap<String, f64> {
    windows.iter().map(|w| (w.key.clone(), w.pct)).collect()
}

/// `a` at position 1 and `b` at position 2, which is live, on Claude Code or on FakeAgent;
/// with the provider and each account's email (its label, for FakeAgent).
struct Two {
    provider: ProviderId,
    a: AccountId,
    b: AccountId,
    a_email: &'static str,
    b_email: &'static str,
    config: AutoConfig,
}

fn two(ffx: &FakeFx, fake: bool) -> Two {
    if fake {
        Two {
            provider: ffx.fake_provider(),
            a: ffx.fake_add("alice", "tok-a", "renew-a"),
            b: ffx.fake_add("bob", "tok-b", "renew-b"),
            a_email: "alice@ws",
            b_email: "bob@ws",
            config: cfg(ffx.fake.as_ref()),
        }
    } else {
        Two {
            provider: ffx.fx.provider(),
            a: ffx.fx.add("a@x.co", "rt-a"),
            b: ffx.fx.add("b@x.co", "rt-b"),
            a_email: "a@x.co",
            b_email: "b@x.co",
            config: cfg(ffx.fx.cc.as_ref()),
        }
    }
}

/// The live login's email (its label, for FakeAgent).
fn live(ffx: &FakeFx, fake: bool) -> Option<String> {
    if fake {
        ffx.fake_live_label()
    } else {
        ffx.fx.live_email()
    }
}

fn no_switch(provider: &ProviderId, reason: &str, detail: &str) -> AutoEvent {
    AutoEvent::NoSwitch {
        provider: provider.clone(),
        reason: reason.into(),
        detail: detail.into(),
    }
}

fn row_count(fx: &Fx, table: &str) -> i64 {
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn auto_state(fx: &Fx) -> AutoState {
    fx.engine
        .store()
        .unwrap()
        .autoswitch_state(&fx.provider())
        .unwrap()
}

#[test]
fn a_tick_below_the_threshold_polls_and_stays() {
    for fake in [false, true] {
        let ffx = FakeFx::new();
        let t = two(&ffx, fake);
        let (low, high) = (reading(fake, 10.0, 10.0), reading(fake, 20.0, 50.0));
        read(&ffx.engine, &t.a, &low);
        read(&ffx.engine, &t.b, &high);
        let sink = Recorded::default();
        let mut engine = ffx
            .engine
            .auto(&t.provider, t.config, false)
            .unwrap()
            .unwrap();
        let (outcome, decision) = engine.tick(&sink).unwrap();
        assert_eq!(outcome, TickOutcome::NoAction, "fake={fake}");
        assert_eq!(
            decision,
            Decision::NoSwitch {
                reason: NoSwitchReason::BelowThreshold,
                outcome: Outcome::NoAction,
                detail: String::new(),
                earliest_reset: None,
            }
        );
        assert_eq!(
            sink.take(),
            [
                AutoEvent::Poll {
                    provider: t.provider.clone(),
                    active: Some((2, t.b_email.into())),
                    headroom_pct: BTreeMap::from([(1, Some(90.0)), (2, Some(50.0))]),
                    threshold: 90.0,
                    fetch_errors: BTreeMap::new(),
                    windows_pct: BTreeMap::from([(1, pcts(&low)), (2, pcts(&high))]),
                },
                no_switch(&t.provider, "below-threshold", ""),
            ],
            "fake={fake}"
        );
        assert!(ffx.fx.http.requests().is_empty(), "nothing was due");
        assert_eq!(
            row_count(&ffx.fx, "autoswitch_state"),
            0,
            "the count stayed 0"
        );
    }
}

#[test]
fn a_proactive_tick_switches_and_records_its_departure() {
    for fake in [false, true] {
        let ffx = FakeFx::new();
        let t = two(&ffx, fake);
        read(&ffx.engine, &t.a, &reading(fake, 10.0, 20.0));
        read(&ffx.engine, &t.b, &reading(fake, 20.0, 95.0));
        let sink = Recorded::default();
        let mut engine = ffx
            .engine
            .auto(&t.provider, t.config, false)
            .unwrap()
            .unwrap();
        let (outcome, decision) = engine.tick(&sink).unwrap();
        assert_eq!(outcome, TickOutcome::Switched, "fake={fake}");
        assert_eq!(
            decision,
            Decision::Switch {
                trigger: Trigger::Proactive,
                targets: vec![t.a.clone()],
                recheck: false,
            }
        );
        let events = sink.take();
        assert_eq!(
            events[1..],
            [AutoEvent::Switch {
                provider: t.provider.clone(),
                trigger: Trigger::Proactive,
                from: 2,
                to: 1,
                warnings: Vec::new(),
                dry_run: false,
            }],
            "fake={fake}"
        );
        assert_eq!(live(&ffx, fake).as_deref(), Some(t.a_email));
        let store = ffx.engine.store().unwrap();
        assert_eq!(
            store.autoswitch_state(&t.provider).unwrap(),
            AutoState {
                last_switch_at: Some(T0),
                last_switch_from: Some(t.b.clone()),
                last_switch_to: Some(t.a.clone()),
                left_headroom: Some(5.0),
                left_recovery_at: Some(LONG_RESETS),
                left_trigger: Some(Trigger::Proactive),
                unhealthy_ticks: 0,
            }
        );
        let last = store.events().unwrap().pop().unwrap();
        assert_eq!(
            (
                last.kind.as_str(),
                last.trigger.as_deref(),
                last.source.as_str()
            ),
            ("switch", Some("proactive"), "auto")
        );
    }
}

/// Review Focus 5: a live login tagteam does not manage is never acted on, and the tick writes
/// nothing, every tick: it stops at step 2, before collecting.
#[test]
fn an_unmanaged_live_login_is_never_acted_on_and_nothing_is_written() {
    for fake in [false, true] {
        let ffx = FakeFx::new();
        let t = two(&ffx, fake);
        if fake {
            ffx.fake_login("stranger", "tok-s", "renew-s");
        } else {
            ffx.fx.login("stranger@x.co", "rt-s");
        }
        let (events, requests) = (row_count(&ffx.fx, "events"), usage_requests(&ffx.fx));
        let keychain = ffx.fx.kc.items();
        let sink = Recorded::default();
        let mut engine = ffx
            .engine
            .auto(&t.provider, t.config, false)
            .unwrap()
            .unwrap();
        for _ in 0..2 {
            let (outcome, decision) = engine.tick(&sink).unwrap();
            assert_eq!(outcome, TickOutcome::NoAction);
            assert!(matches!(
                decision,
                Decision::NoSwitch {
                    reason: NoSwitchReason::UnmanagedActiveAccount,
                    ..
                }
            ));
            assert_eq!(
                sink.take(),
                [no_switch(&t.provider, "unmanaged-active-account", "")]
            );
        }
        assert_eq!(row_count(&ffx.fx, "events"), events);
        assert_eq!(row_count(&ffx.fx, "autoswitch_state"), 0);
        assert_eq!(usage_requests(&ffx.fx), requests);
        assert!(ffx.fx.http.requests().is_empty());
        assert_eq!(ffx.fx.kc.items(), keychain);
        let stranger = if fake { "stranger@ws" } else { "stranger@x.co" };
        assert_eq!(live(&ffx, fake).as_deref(), Some(stranger));
    }
}

#[test]
fn no_live_login_is_no_active_account() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fs::write(fx.paths().global_config, common::CLAUDE_JSON).unwrap();
    fx.kc
        .delete(
            &keychain_service(&fx.env, ItemKind::OAuth),
            &keychain_account(&fx.env),
        )
        .unwrap();
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    assert_eq!(engine.tick(&sink).unwrap().0, TickOutcome::NoAction);
    assert_eq!(
        sink.take(),
        [no_switch(&fx.provider(), "no-active-account", "")]
    );
}

#[test]
fn a_dry_run_tick_writes_nothing_and_says_what_it_would_switch() {
    for fake in [false, true] {
        let ffx = FakeFx::new();
        let t = two(&ffx, fake);
        read(&ffx.engine, &t.a, &reading(fake, 10.0, 20.0));
        read(&ffx.engine, &t.b, &reading(fake, 20.0, 95.0));
        let (events, requests) = (row_count(&ffx.fx, "events"), usage_requests(&ffx.fx));
        let keychain = ffx.fx.kc.items();
        let config = fs::read(ffx.fx.paths().global_config).unwrap();
        let fake_files = FakePaths::resolve(&ffx.fx.env);
        let fake_live = (
            fs::read(&fake_files.identity).ok(),
            fs::read(&fake_files.credential).ok(),
        );
        let sink = Recorded::default();
        let mut engine = ffx
            .engine
            .auto(&t.provider, t.config, true)
            .unwrap()
            .unwrap();
        let (outcome, decision) = engine.tick(&sink).unwrap();
        assert_eq!(outcome, TickOutcome::Switched, "fake={fake}");
        assert!(matches!(
            decision,
            Decision::Switch {
                trigger: Trigger::Proactive,
                ..
            }
        ));
        assert_eq!(
            sink.take()[1..],
            [AutoEvent::Switch {
                provider: t.provider.clone(),
                trigger: Trigger::Proactive,
                from: 2,
                to: 1,
                warnings: Vec::new(),
                dry_run: true,
            }]
        );
        assert_eq!(row_count(&ffx.fx, "events"), events);
        assert_eq!(row_count(&ffx.fx, "autoswitch_state"), 0);
        assert_eq!(usage_requests(&ffx.fx), requests, "nothing was due");
        assert_eq!(ffx.fx.kc.items(), keychain, "the vault and the live items");
        assert_eq!(fs::read(ffx.fx.paths().global_config).unwrap(), config);
        assert_eq!(
            (
                fs::read(&fake_files.identity).ok(),
                fs::read(&fake_files.credential).ok()
            ),
            fake_live
        );
        assert_eq!(live(&ffx, fake).as_deref(), Some(t.b_email));
    }
}

#[test]
fn consume_first_runs_best_on_a_provider_without_a_long_window() {
    // §4.5 and §11.5: FakeAgent names no long window, so a consume-first setting decides as
    // `best` (below the threshold, it stays) and says so once.
    let ffx = FakeFx::new();
    let t = two(&ffx, true);
    read(&ffx.engine, &t.a, &reading(true, 10.0, 10.0));
    read(&ffx.engine, &t.b, &reading(true, 20.0, 50.0));
    let config = AutoConfig {
        strategy: Strategy::ConsumeFirst,
        ..t.config
    };
    assert_eq!(config.long_window, None);
    let sink = Recorded::default();
    let mut engine = ffx
        .engine
        .auto(&t.provider, config, false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::NoAction);
    assert!(matches!(
        decision,
        Decision::NoSwitch {
            reason: NoSwitchReason::BelowThreshold,
            ..
        }
    ));
    let events = sink.take();
    assert_eq!(
        events[1..],
        [
            AutoEvent::ConfigWarning {
                provider: t.provider.clone(),
                message:
                    "FakeAgent has no long usage window to rank by, so consume-first runs best"
                        .into(),
            },
            no_switch(&t.provider, "below-threshold", ""),
        ]
    );
    engine.tick(&sink).unwrap();
    assert_eq!(
        sink.take()[1..],
        [no_switch(&t.provider, "below-threshold", "")]
    );
}

#[test]
fn consume_first_switches_to_the_sooner_long_reset_after_a_re_check() {
    // Decision 2: the re-check finds every reading at most 180 s old, so it fetches nothing,
    // and the re-checked decision switches.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let soon = vec![
        usage_window("5h", WindowKind::Short, 10.0, SHORT_RESETS),
        usage_window("7d", WindowKind::Long, 30.0, T0 + 86_400),
    ];
    read(&fx.engine, &a, &soon);
    read(&fx.engine, &b, &reading(false, 10.0, 50.0));
    let config = AutoConfig {
        strategy: Strategy::ConsumeFirst,
        ..cfg(fx.cc.as_ref())
    };
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), config, false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Switched);
    assert_eq!(
        decision,
        Decision::Switch {
            trigger: Trigger::ConsumeFirst,
            targets: vec![a.clone()],
            recheck: false,
        }
    );
    assert!(matches!(
        sink.take()[1],
        AutoEvent::Switch {
            trigger: Trigger::ConsumeFirst,
            from: 2,
            to: 1,
            ..
        }
    ));
    assert!(fx.http.requests().is_empty());
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
}

#[test]
fn a_re_check_that_cannot_refresh_a_reading_is_stale_usage() {
    // Readings 200 s old are decision-grade, so the first decision is a consume-first switch;
    // the re-check sends for both accounts, gets no reply, and the target is still too old.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let soon = vec![
        usage_window("5h", WindowKind::Short, 10.0, SHORT_RESETS),
        usage_window("7d", WindowKind::Long, 30.0, T0 + 86_400),
    ];
    record_reading(&fx.engine, &a, &soon, T0 - 200, T0 + 100);
    record_reading(
        &fx.engine,
        &b,
        &reading(false, 10.0, 50.0),
        T0 - 200,
        T0 + 100,
    );
    let config = AutoConfig {
        strategy: Strategy::ConsumeFirst,
        ..cfg(fx.cc.as_ref())
    };
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), config, false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::NoAction);
    assert!(matches!(
        decision,
        Decision::NoSwitch {
            reason: NoSwitchReason::StaleUsage,
            ..
        }
    ));
    assert_eq!(
        sink.take().last(),
        Some(&no_switch(&fx.provider(), "stale-usage", ""))
    );
    let mut sent = common::usage_bearers(&fx);
    sent.sort();
    assert_eq!(
        sent,
        ["at-rt-a", "at-rt-b"],
        "one thread per account (§8.3)"
    );
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn a_target_the_re_check_left_stale_is_never_switched_to() {
    // Decision 2: a was read just now, b 200 s ago. The re-check sends for b alone and gets no
    // reply, so b's reading stays stale and b leaves the targets. a's access token needs a
    // refresh that cannot be sent: the tick ends there instead of falling back to b.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c"); // live
    let long_resets = |at| {
        vec![
            usage_window("5h", WindowKind::Short, 10.0, SHORT_RESETS),
            usage_window("7d", WindowKind::Long, 30.0, at),
        ]
    };
    read(&fx.engine, &a, &long_resets(T0 + 86_400));
    record_reading(
        &fx.engine,
        &b,
        &long_resets(T0 + 172_800),
        T0 - 200,
        T0 + 100,
    );
    read(&fx.engine, &c, &reading(false, 10.0, 50.0));
    fx.expire_access(&a);
    let config = AutoConfig {
        strategy: Strategy::ConsumeFirst,
        ..cfg(fx.cc.as_ref())
    };
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), config, false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(
        decision,
        Decision::Switch {
            trigger: Trigger::ConsumeFirst,
            targets: vec![a.clone()],
            recheck: false,
        }
    );
    assert_eq!(outcome, TickOutcome::Error);
    assert_eq!(
        sink.take().last(),
        Some(&AutoEvent::Error {
            provider: fx.provider(),
            message: "could not freshen a@x.co (position 1): pre-send".into(),
            transient: true,
        })
    );
    assert_eq!(common::usage_bearers(&fx), ["at-rt-b"]);
    assert_eq!(fx.live_email().as_deref(), Some("c@x.co"));
}

/// Review Focus 1, the tick half: a manual switch between two ticks. The next tick decides
/// afresh from the account switched to, and never switches back on its own initiative.
#[test]
fn a_manual_switch_between_ticks_is_decided_afresh_from_the_new_live_account() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c"); // live
    for id in [&a, &b, &c] {
        read(&fx.engine, id, &reading(false, 10.0, 50.0));
    }
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    engine.tick(&sink).unwrap();
    assert!(matches!(
        &sink.take()[0],
        AutoEvent::Poll {
            active: Some((3, _)),
            ..
        }
    ));
    assert!(fx.switch_to(&b, false).unwrap().switched);
    let (outcome, _) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::NoAction);
    let events = sink.take();
    assert!(
        matches!(&events[0], AutoEvent::Poll { active: Some((2, email)), .. } if email == "b@x.co"),
        "{events:?}"
    );
    assert_eq!(events[1], no_switch(&fx.provider(), "below-threshold", ""));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

/// Review Focus 1: the tick decided on c, and a manual switch lands while its switch waits
/// before the mutation lock. The tick reports `live-changed` and writes nothing.
#[cfg(feature = "test-hooks")]
#[test]
fn a_manual_switch_during_the_tick_s_perform_is_live_changed() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c"); // live
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &b, &reading(false, 10.0, 50.0));
    read(&fx.engine, &c, &reading(false, 10.0, 95.0));
    let other = fx.engine_with_env(fx.env.clone());
    let manual = fx.switch_request(&b, false);
    fx.engine.on_point(
        "planned",
        Box::new(move || assert!(other.switch(manual.clone()).unwrap().switched)),
    );
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::NoAction);
    assert!(matches!(
        decision,
        Decision::NoSwitch {
            reason: NoSwitchReason::LiveChanged,
            outcome: Outcome::NoAction,
            ..
        }
    ));
    assert_eq!(
        sink.take()[1..],
        [no_switch(&fx.provider(), "live-changed", "")]
    );
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    let switches: Vec<_> = fx
        .engine
        .store()
        .unwrap()
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "switch")
        .collect();
    assert_eq!(switches.len(), 1);
    assert_eq!(switches[0].source, "cli");
    assert_eq!(auto_state(&fx), AutoState::default());
}

#[test]
fn the_unknown_usage_count_starts_afresh_on_an_account_switched_to_by_hand() {
    // Decision 4: the count belongs to the account it judged. Carried over, 2 of c's unknown
    // ticks would fail over from b, just chosen by hand, at b's first unknown tick.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live; nothing is ever read
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    for n in 1..=2 {
        let (_, decision) = engine.tick(&sink).unwrap();
        assert!(
            matches!(&decision, Decision::NoSwitch { reason: NoSwitchReason::ActiveUsageUnknown, detail, .. } if *detail == format!("{n}/3")),
            "{decision:?}"
        );
    }
    assert_eq!(auto_state(&fx).unhealthy_ticks, 2);
    assert!(fx.switch_to(&b, false).unwrap().switched);
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::NoAction);
    assert!(
        matches!(&decision, Decision::NoSwitch { reason: NoSwitchReason::ActiveUsageUnknown, detail, .. } if detail == "1/3"),
        "{decision:?}"
    );
    assert_eq!(auto_state(&fx).unhealthy_ticks, 1);
}

#[test]
fn every_candidate_exhausted_is_blocked_until_the_earliest_recovery() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx.engine, &a, &reading(false, 100.0, 40.0));
    read(&fx.engine, &b, &reading(false, 100.0, 40.0));
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Blocked);
    assert_eq!(
        decision,
        Decision::NoSwitch {
            reason: NoSwitchReason::AllExhausted,
            outcome: Outcome::Blocked,
            detail: "2h40m".into(),
            earliest_reset: Some(SHORT_RESETS),
        }
    );
    assert_eq!(
        sink.take()[1..],
        [
            no_switch(&fx.provider(), "all-exhausted", "2h40m"),
            AutoEvent::AllExhausted {
                provider: fx.provider(),
                earliest_reset_at: Some(SHORT_RESETS),
            },
        ]
    );
}

#[test]
fn a_dead_target_is_quarantined_once_and_the_next_target_is_switched_to() {
    // §11.2 step 10: Dead quarantines the target, `account-quarantined`, next target. The
    // next tick already knows the quarantine and does not report it again.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c"); // live, at its limit
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &b, &reading(false, 10.0, 50.0));
    read(&fx.engine, &c, &reading(false, 100.0, 50.0));
    fx.expire_access(&a);
    fx.script_token_error(400, "invalid_grant");
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Switched);
    assert_eq!(
        decision,
        Decision::Switch {
            trigger: Trigger::AtLimit,
            targets: vec![a.clone(), b.clone()],
            recheck: false,
        }
    );
    assert_eq!(
        sink.take()[1..],
        [
            AutoEvent::AccountQuarantined {
                provider: fx.provider(),
                number: 1,
                email: "a@x.co".into(),
                reason: "invalid_grant".into(),
            },
            AutoEvent::Switch {
                provider: fx.provider(),
                trigger: Trigger::AtLimit,
                from: 3,
                to: 2,
                warnings: Vec::new(),
                dry_run: false,
            },
        ]
    );
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    engine.tick(&sink).unwrap();
    assert_eq!(
        sink.take()[1..],
        [no_switch(&fx.provider(), "below-threshold", "")]
    );
}

#[test]
fn a_target_that_cannot_be_freshened_for_now_is_an_error_at_the_normal_cadence() {
    // §11.2 step 12: every target failed, transiently. No token reply is scripted: the gate's
    // request never leaves (`pre-send`).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let c = fx.add("c@x.co", "rt-c"); // live, at its limit
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &c, &reading(false, 100.0, 50.0));
    fx.expire_access(&a);
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Error);
    assert!(matches!(
        decision,
        Decision::Switch {
            trigger: Trigger::AtLimit,
            ..
        }
    ));
    assert_eq!(
        sink.take()[1..],
        [AutoEvent::Error {
            provider: fx.provider(),
            message: "could not freshen a@x.co (position 1): pre-send".into(),
            transient: true,
        }]
    );
    assert_eq!(fx.live_email().as_deref(), Some("c@x.co"));
}

#[test]
fn a_target_without_a_stored_credential_leaves_no_viable_target() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let c = fx.add("c@x.co", "rt-c"); // live, at its limit
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &c, &reading(false, 100.0, 50.0));
    fx.kc.delete(SERVICE, a.as_str()).unwrap();
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Blocked);
    assert!(matches!(
        decision,
        Decision::NoSwitch {
            reason: NoSwitchReason::NoViableTarget,
            outcome: Outcome::Blocked,
            ..
        }
    ));
    assert_eq!(
        sink.take()[1..],
        [no_switch(&fx.provider(), "no-viable-target", "")]
    );
}

#[test]
fn an_api_key_target_passes_freshening() {
    // §11.2 step 10: API keys do not refresh; with no OAuth target, at-limit falls back to them.
    let fx = Fx::new();
    let c = fx.add("c@x.co", "rt-c"); // live, at its limit
    let k = fx.add_api_key(API_KEY);
    read(&fx.engine, &c, &reading(false, 100.0, 50.0));
    let config = AutoConfig {
        include_api_key_accounts: true,
        ..cfg(fx.cc.as_ref())
    };
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), config, false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Switched);
    assert_eq!(
        decision,
        Decision::Switch {
            trigger: Trigger::AtLimit,
            targets: vec![k.clone()],
            recheck: false,
        }
    );
    assert!(matches!(
        sink.take()[1],
        AutoEvent::Switch {
            trigger: Trigger::AtLimit,
            from: 1,
            to: 2,
            ..
        }
    ));
    assert_eq!(fx.managed_key().as_deref(), Some(API_KEY.as_bytes()));
}

#[test]
fn an_interrupted_switch_recovery_cannot_decide_blocks_the_tick() {
    // §11.2 step 2: CC rotated the target's credential after the crash and no oracle answers.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fx.rotate_live("rt-a-rotated-by-cc");
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Blocked);
    let Decision::NoSwitch {
        reason: NoSwitchReason::InterruptedSwitch,
        detail,
        ..
    } = &decision
    else {
        panic!("{decision:?}");
    };
    assert!(detail.contains("--force"), "{detail}");
    assert_eq!(
        sink.take(),
        [no_switch(&fx.provider(), "interrupted-switch", detail)]
    );
}

#[test]
fn a_tick_recovers_an_interrupted_switch_as_auto_and_goes_on() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &b, &reading(false, 10.0, 20.0));
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    engine.tick(&sink).unwrap();
    let recovered = fx.engine.store().unwrap().events().unwrap().pop().unwrap();
    assert_eq!(
        (recovered.kind.as_str(), recovered.source.as_str()),
        ("switch-recovered", "auto")
    );
    assert!(matches!(
        &sink.take()[0],
        AutoEvent::Poll {
            active: Some((1, _)),
            ..
        }
    ));
}

#[test]
fn quarantines_other_processes_set_or_clear_are_reported_against_the_previous_tick() {
    // Decision 10. The first tick reports only its own releases; later ones report every
    // change since the tick before, `account-replaced` when the login epoch moved.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx.engine, &a, &reading(false, 10.0, 10.0));
    read(&fx.engine, &b, &reading(false, 10.0, 50.0));
    let store = fx.engine.store().unwrap();
    let bound = vault_fp(&fx, &a);
    let quarantined = AutoEvent::AccountQuarantined {
        provider: fx.provider(),
        number: 1,
        email: "a@x.co".into(),
        reason: "invalid_grant".into(),
    };
    let unquarantined = |reason: &str| AutoEvent::AccountUnquarantined {
        provider: fx.provider(),
        number: 1,
        email: "a@x.co".into(),
        reason: reason.into(),
    };
    let sink = Recorded::default();
    // Set before the engine starts: the first tick has nothing to compare it with.
    fx.quarantine(&a, "invalid_grant", &bound);
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    engine.tick(&sink).unwrap();
    assert!(matches!(sink.take()[0], AutoEvent::Poll { .. }));
    store.clear_quarantine(&a).unwrap();
    engine.tick(&sink).unwrap();
    assert_eq!(sink.take()[0], unquarantined("credentials-replaced"));
    fx.quarantine(&a, "invalid_grant", &bound);
    engine.tick(&sink).unwrap();
    assert_eq!(sink.take()[0], quarantined);
    // An explicit replacement (§12.5) bumps the epoch and installs a new login.
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "UPDATE accounts SET login_epoch = login_epoch + 1, quarantine_reason = NULL, \
             quarantine_fp = NULL, quarantine_at = NULL WHERE id = ?1",
            [a.as_str()],
        )
        .unwrap();
    engine.tick(&sink).unwrap();
    assert_eq!(sink.take()[0], unquarantined("account-replaced"));
    // Step 1's own release: the vault has moved past the generation the quarantine names.
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    engine.tick(&sink).unwrap();
    assert_eq!(sink.take()[0], unquarantined("credentials-replaced"));
    let last = store.events().unwrap().pop().unwrap();
    assert_eq!(
        (last.kind.as_str(), last.source.as_str()),
        ("unquarantine", "auto")
    );
}

#[test]
fn a_quarantine_the_collection_set_is_reported_once() {
    // Task 3's report names the accounts quarantined while they were collected; the next
    // tick's comparison knows it already (Decision 10).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a"); // never read: the scheduled pick
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx.engine, &b, &reading(false, 10.0, 50.0));
    fx.expire_access(&a);
    fx.script_token_error(400, "invalid_grant");
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    engine.tick(&sink).unwrap();
    let quarantined = AutoEvent::AccountQuarantined {
        provider: fx.provider(),
        number: 1,
        email: "a@x.co".into(),
        reason: "invalid_grant".into(),
    };
    let events = sink.take();
    assert_eq!(
        events.iter().filter(|e| **e == quarantined).count(),
        1,
        "{events:?}"
    );
    engine.tick(&sink).unwrap();
    assert!(!sink.take().contains(&quarantined));
}

#[test]
fn unknown_model_names_are_warned_about_once_per_setting() {
    // §11.2 step 3: once per engine, and again whenever `models` changes.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let mut fable = reading(false, 10.0, 10.0);
    fable.push(usage_window(
        "scoped:Fable",
        WindowKind::Scoped,
        10.0,
        LONG_RESETS,
    ));
    read(&fx.engine, &a, &fable);
    read(&fx.engine, &b, &reading(false, 10.0, 50.0));
    let warning = |name: &str| AutoEvent::ConfigWarning {
        provider: fx.provider(),
        message: format!(
            "autoswitch.models names {name:?}, but no account's usage reports a window for that model"
        ),
    };
    let config = AutoConfig {
        models: vec!["fable".into(), "Opus".into()],
        ..cfg(fx.cc.as_ref())
    };
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), config.clone(), false)
        .unwrap()
        .unwrap();
    engine.tick(&sink).unwrap();
    assert_eq!(sink.take()[1], warning("Opus"));
    engine.tick(&sink).unwrap();
    assert!(!sink.take().contains(&warning("Opus")));
    engine.set_config(AutoConfig {
        models: vec!["opus".into()],
        ..config.clone()
    });
    engine.tick(&sink).unwrap();
    assert_eq!(sink.take()[1], warning("opus"));
    engine.set_config(AutoConfig {
        models: vec!["all".into()],
        ..config
    });
    engine.tick(&sink).unwrap();
    assert!(
        !sink
            .take()
            .iter()
            .any(|e| matches!(e, AutoEvent::ConfigWarning { .. }))
    );
}

#[test]
fn the_model_check_waits_for_a_reading_to_compare_with() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let config = AutoConfig {
        models: vec!["Opus".into()],
        ..cfg(fx.cc.as_ref())
    };
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), config, false)
        .unwrap()
        .unwrap();
    let warned = |events: &[AutoEvent]| {
        events
            .iter()
            .any(|e| matches!(e, AutoEvent::ConfigWarning { .. }))
    };
    engine.tick(&sink).unwrap();
    assert!(!warned(&sink.take()), "no account has a reading yet");
    fx.clock.advance_ms(3_600_000);
    let now = T0 + 3_600;
    record_reading(&fx.engine, &a, &reading(false, 10.0, 10.0), now, now + 300);
    record_reading(&fx.engine, &b, &reading(false, 10.0, 50.0), now, now + 300);
    engine.tick(&sink).unwrap();
    assert!(warned(&sink.take()));
}

/// §14.1: a signal at a collection cancellation point ends the tick with the interruption. The
/// slot reserved for the request that never left is given back, and nothing is recorded.
/// What a sink is given, in order: each event, and each tick's end with its provider.
#[derive(Debug, Clone, PartialEq)]
enum Given {
    Event(AutoEvent),
    TickDone(ProviderId),
}

#[derive(Default)]
struct Bounded(Mutex<Vec<Given>>);

impl EventSink for Bounded {
    fn emit(&self, e: &AutoEvent) {
        self.0.lock().unwrap().push(Given::Event(e.clone()));
    }

    fn tick_done(&self, provider: &ProviderId) {
        self.0
            .lock()
            .unwrap()
            .push(Given::TickDone(provider.clone()));
    }
}

impl Bounded {
    fn take(&self) -> Vec<Given> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

#[test]
fn every_tick_ends_at_one_boundary_whatever_it_came_to() {
    // A sink learns where each tick ends, so nothing it keeps for a tick's line outlives the
    // tick: once, last, after an `error` tick, an `Ok` one and an interrupted one.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let c = fx.add("c@x.co", "rt-c"); // live, at its limit
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &c, &reading(false, 100.0, 50.0));
    fx.expire_access(&a);
    let sink = Bounded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let done = Given::TickDone(fx.provider());
    let ends_once = |given: Vec<Given>| {
        assert_eq!(given.iter().filter(|g| **g == done).count(), 1, "{given:?}");
        assert_eq!(given.last(), Some(&done), "{given:?}");
    };
    // a's refresh cannot be sent: an `error` tick.
    assert_eq!(engine.tick(&sink).unwrap().0, TickOutcome::Error);
    ends_once(sink.take());
    // Without a's stored credential nothing is viable: an `Ok` tick, blocked.
    fx.kc.delete(SERVICE, a.as_str()).unwrap();
    assert_eq!(engine.tick(&sink).unwrap().0, TickOutcome::Blocked);
    ends_once(sink.take());
    // A signal before the tick: it stops at its first cancellation point.
    fx.engine.cancel().request(libc::SIGTERM);
    let e = engine.tick(&sink).unwrap_err();
    assert_eq!(e.signal(), Some(libc::SIGTERM));
    assert_eq!(sink.take(), [done.clone()]);
}

#[cfg(feature = "test-hooks")]
#[test]
fn an_interruption_during_collection_ends_the_tick_with_nothing_half_written() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live and never read: phase 1 collects it
    let (events, requests) = (row_count(&fx, "events"), usage_requests(&fx));
    let cancel = fx.engine.cancel().clone();
    fx.engine.on_point(
        "usage-reserved",
        Box::new(move || cancel.request(libc::SIGINT)),
    );
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let e = engine.tick(&sink).unwrap_err();
    assert_eq!(e.signal(), Some(libc::SIGINT));
    assert!(sink.take().is_empty());
    assert_eq!(usage_requests(&fx), requests, "the unsent slot went back");
    assert_eq!(fx.usage_state(&b).and_then(|s| s.fetched_at), None);
    assert!(fx.http.requests().is_empty());
    assert_eq!(row_count(&fx, "events"), events);
    assert_eq!(row_count(&fx, "autoswitch_state"), 0);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

/// §11.2 step 11: a target that stopped being a candidate while the switch waited is passed
/// over for the next one. Another process disables a before the switch takes its locks.
#[cfg(feature = "test-hooks")]
#[test]
fn a_target_that_stops_being_a_candidate_moves_the_tick_to_the_next() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c"); // live
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &b, &reading(false, 10.0, 50.0));
    read(&fx.engine, &c, &reading(false, 10.0, 95.0));
    let other = fx.engine_with_env(fx.env.clone());
    let disabled = a.clone();
    fx.engine.on_point(
        "planned",
        Box::new(move || drop(other.set_disabled(&disabled, true).unwrap())),
    );
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Switched);
    assert_eq!(
        decision,
        Decision::Switch {
            trigger: Trigger::Proactive,
            targets: vec![a, b],
            recheck: false,
        }
    );
    assert!(matches!(
        sink.take()[1],
        AutoEvent::Switch { from: 3, to: 2, .. }
    ));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

/// §11.2 step 11: the cooldown is judged again under the lock, from the state as stored then;
/// here a switch is recorded while the tick's own switch waits.
#[cfg(feature = "test-hooks")]
#[test]
fn a_cooldown_found_under_the_lock_is_no_switch_cooldown() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &b, &reading(false, 10.0, 95.0));
    let db = fx.env.data_dir().join("tagteam.db");
    fx.engine.on_point(
        "planned",
        Box::new(move || {
            rusqlite::Connection::open(&db)
                .unwrap()
                .execute(
                    "INSERT INTO autoswitch_state (provider, last_switch_at) VALUES ('claude-code', ?1)",
                    [T0 - 60],
                )
                .unwrap();
        }),
    );
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, decision) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::NoAction);
    assert_eq!(
        decision,
        Decision::NoSwitch {
            reason: NoSwitchReason::Cooldown,
            outcome: Outcome::NoAction,
            detail: "4m".into(),
            earliest_reset: None,
        }
    );
    assert_eq!(
        sink.take()[1..],
        [no_switch(&fx.provider(), "cooldown", "4m")]
    );
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

/// M3a's rule (§14.1): a signal inside the switch's critical span does not stop it. The tick
/// reports the switch it made, and the token stays set for the caller's next check.
#[cfg(feature = "test-hooks")]
#[test]
fn a_switch_that_commits_despite_a_late_signal_is_switched() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx.engine, &a, &reading(false, 10.0, 20.0));
    read(&fx.engine, &b, &reading(false, 10.0, 95.0));
    let cancel = fx.engine.cancel().clone();
    fx.engine.on_point(
        "after-journal",
        Box::new(move || cancel.request(libc::SIGTERM)),
    );
    let sink = Recorded::default();
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let (outcome, _) = engine.tick(&sink).unwrap();
    assert_eq!(outcome, TickOutcome::Switched);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(auto_state(&fx).last_switch_to, Some(a));
    assert_eq!(
        engine.tick(&sink).unwrap_err().signal(),
        Some(libc::SIGTERM)
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test auto_tick --test auto_engine`
Expected: FAIL to compile: unresolved import `tagteam_engine::auto` (in `tests/common/mod.rs`,
which every engine test binary includes).

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/engine.rs`, replace

```rust
    pub(crate) fn guard_or_refuse(
        &self,
        provider: &ProviderId,
    ) -> Result<MutationGuard, EngineError> {
        let (guard, blocked) = self.guard_recovering(true)?;
```

with

```rust
    pub(crate) fn guard_or_refuse(
        &self,
        provider: &ProviderId,
    ) -> Result<MutationGuard, EngineError> {
        self.guard_or_refuse_as(provider, "cli")
    }

    /// `guard_or_refuse`, recording each switch it recovers with `source` (§9.4 step 9: `cli`,
    /// or `auto` for an auto-switch engine, §11.2 step 2).
    pub(crate) fn guard_or_refuse_as(
        &self,
        provider: &ProviderId,
        source: &'static str,
    ) -> Result<MutationGuard, EngineError> {
        let (guard, blocked) = self.guard_recovering(true, source)?;
```

In `crates/tagteam-engine/src/engine.rs`, replace

```rust
    pub(crate) fn settle_or_refuse(&self, provider: &ProviderId) -> Result<(), EngineError> {
        if self.interrupted(provider)? {
            drop(self.guard_or_refuse(provider)?);
        }
        Ok(())
    }

```

with

```rust
    pub(crate) fn settle_or_refuse(&self, provider: &ProviderId) -> Result<(), EngineError> {
        self.settle_or_refuse_as(provider, "cli")
    }

    /// `settle_or_refuse`, recording each switch it recovers with `source`.
    pub(crate) fn settle_or_refuse_as(
        &self,
        provider: &ProviderId,
        source: &'static str,
    ) -> Result<(), EngineError> {
        if self.interrupted(provider)? {
            drop(self.guard_or_refuse_as(provider, source)?);
        }
        Ok(())
    }

```

In `crates/tagteam-engine/src/engine.rs`, replace

```rust
        Ok(self.guard_recovering(true)?.0)
```

with

```rust
        Ok(self.guard_recovering(true, "cli")?.0)
```

In `crates/tagteam-engine/src/engine.rs`, replace

```rust
        Ok(self.guard_recovering(false)?.0)
```

with

```rust
        Ok(self.guard_recovering(false, "cli")?.0)
```

In `crates/tagteam-engine/src/engine.rs`, replace

```rust
    /// interrupted at one of its lock waits ends the command with that interruption (§14.1).
    fn guard_recovering(
        &self,
        ask_oracle: bool,
    ) -> Result<(MutationGuard, Vec<(ProviderId, EngineError)>), EngineError> {
```

with

```rust
    /// interrupted at one of its lock waits ends the command with that interruption (§14.1).
    /// Each recovered switch is recorded with `source`.
    fn guard_recovering(
        &self,
        ask_oracle: bool,
        source: &'static str,
    ) -> Result<(MutationGuard, Vec<(ProviderId, EngineError)>), EngineError> {
```

In `crates/tagteam-engine/src/engine.rs`, replace

```rust
            if let Err(e) = self.recover_one(&guard, &row, hint) {
```

with

```rust
            if let Err(e) = self.recover_one(&guard, &row, hint, source) {
```

In `crates/tagteam-engine/src/recover.rs`, replace

```rust
    /// stays until `switch --force` settles it.
    pub(crate) fn recover_one(
        &self,
        guard: &MutationGuard,
        row: &JournalRow,
        hints: &[OracleHint],
    ) -> Result<(), EngineError> {
        let provider = self.provider(&row.provider)?;
```

with

```rust
    /// stays until `switch --force` settles it. A forward finish's event carries `source`.
    pub(crate) fn recover_one(
        &self,
        guard: &MutationGuard,
        row: &JournalRow,
        hints: &[OracleHint],
        source: &'static str,
    ) -> Result<(), EngineError> {
        let provider = self.provider(&row.provider)?;
```

In `crates/tagteam-engine/src/recover.rs`, replace

```rust
                self.finish_forward(p, &store, row, &accounts, &locks, &live, hints, &fp)
```

with

```rust
                self.finish_forward(p, &store, row, &accounts, &locks, &live, hints, &fp, source)
```

In `crates/tagteam-engine/src/recover.rs`, replace

```rust
        established: &str,
    ) -> Result<(), EngineError> {
        let to = store
```

with

```rust
        established: &str,
        source: &'static str,
    ) -> Result<(), EngineError> {
        let to = store
```

In `crates/tagteam-engine/src/recover.rs`, replace

```rust
                trigger: Some("recovery".into()),
                source: "cli".into(),
```

with

```rust
                trigger: Some("recovery".into()),
                source: source.into(),
```

In `crates/tagteam-engine/src/switch.rs`, before

```rust
fn needs_relogin(target: &AccountRow) -> EngineError {
```

insert

```rust
/// What freshening an automatic switch's target decided (§11.2 step 10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AutoFreshened {
    /// Perform the switch.
    Ready,
    /// A Dead verdict, an identity conflict or a lost successor: the gate has quarantined it
    /// (§7.4). Try the next target.
    Quarantined,
    /// Transient or systemic, with what went wrong. Try the next target.
    Failed(String),
    /// Not a candidate after all: no stored credential, or owned by a session (§12.5). Try the
    /// next target.
    Skip,
}

```

In `crates/tagteam-engine/src/switch.rs`, before

```rust
    /// §5: with no store there is nothing to activate, and nothing is created; an unmanaged
```

insert

```rust
    /// §11.2 step 10 for one target, before its switch takes `MutationGuard` (§7.2): a stored
    /// token that expires within the freshen window is refreshed through the gate (§7.3),
    /// and the gate's outcome is read by the auto-switch table. A kind that does not refresh
    /// passes as it is, so API-key targets pass. The vault is read here, lazily (§9.3).
    pub(crate) fn freshen_auto(
        &self,
        p: &dyn Provider,
        target: &AccountRow,
    ) -> Result<AutoFreshened, EngineError> {
        if !p.kind_traits(&target.kind).refreshable {
            return Ok(AutoFreshened::Ready);
        }
        let vault = match self.vault.read(&target.id) {
            Read::Present(b) if !b.is_empty() => b,
            Read::Unreadable(e) => return Ok(AutoFreshened::Failed(e.to_string())),
            Read::Present(_) | Read::Absent => return Ok(AutoFreshened::Skip),
        };
        if !self.due(p, &vault) {
            return Ok(AutoFreshened::Ready);
        }
        // §14.1: nothing is locked yet, so a signal that has landed stops the tick before it
        // spends the target's refresh token.
        self.check_cancel()?;
        Ok(match self.refresh_stored(p, &target.id, &vault)? {
            // Busy: another process is refreshing it now; the switch's account lock and its
            // pending-rescue settle pick that refresh up, as for a manual switch (§7.2).
            GateOutcome::Refreshed(_) | GateOutcome::AlreadyFresh(_) | GateOutcome::Busy => {
                AutoFreshened::Ready
            }
            // It became the live login, or an unfinished switch names it: the switch's own
            // checks under its locks decide (§9.4 step 1, §9.6).
            GateOutcome::Owned(OwnedBy::Live | OwnedBy::Journal) => AutoFreshened::Ready,
            GateOutcome::Dead(_) | GateOutcome::Unpersisted => AutoFreshened::Quarantined,
            // The vault's generation is spent and its successor waits in `rescue/`: activating
            // it would hand the agent a used refresh token. A later tick adopts the rescue.
            GateOutcome::Transient { rescued: true, .. } => AutoFreshened::Failed(
                "its refreshed token is in rescue/, not yet in the vault".into(),
            ),
            GateOutcome::Transient { kind, .. } => AutoFreshened::Failed(kind),
            GateOutcome::Systemic(detail) => AutoFreshened::Failed(detail),
            GateOutcome::Owned(OwnedBy::Session) | GateOutcome::Conflict => AutoFreshened::Skip,
        })
    }

```

In `crates/tagteam-engine/src/switch.rs`, replace

```rust
            self.settle_or_refuse(&req.provider)?;
```

with

```rust
            self.settle_or_refuse_as(&req.provider, req.source)?;
```

In `crates/tagteam-engine/src/switch.rs`, replace

```rust
            self.guard_or_refuse(&req.provider)?
```

with

```rust
            self.guard_or_refuse_as(&req.provider, req.source)?
```

In `crates/tagteam-engine/src/store/usage.rs`, before

```rust
    /// The provider's cached live identity. A row missing its path, mtime or size reads as
```

insert

```rust
    /// When the account's usage lease expires, in epoch ms, while its row is there: a lease
    /// outlives the record it fenced until it expires (§8.3).
    pub fn usage_lease_expires_at(&self, id: &AccountId) -> Result<Option<i64>, StoreError> {
        Ok(self
            .lock()
            .query_row(
                "SELECT expires_at FROM leases WHERE name = ?1",
                [lease_name(id)],
                |r| r.get(0),
            )
            .optional()?)
    }

```

In `crates/tagteam-engine/src/lib.rs`, replace

```rust
pub mod active;
```

with

```rust
pub mod active;
pub mod auto;
```

Create `crates/tagteam-engine/src/auto.rs`:

```rust
//! §11.1 and §11.2: the auto-switch engine for one provider. It holds the provider's engine
//! lock for its whole life and runs one tick at a time; the loop that schedules its ticks
//! belongs to its caller (Decision 7).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use tagteam_core::autoswitch::{
    AccountSnapshot, AutoConfig, AutoState, Decision, Live, NoSwitchReason, Outcome, Phase,
    Snapshot, Strategy, Trigger, decide, departure,
};
use tagteam_core::rank::span;
use tagteam_core::usage::headroom;
use tagteam_core::{AccountId, ProviderId, Window, WindowKind};
use tagteam_provider::{FlockGuard, LockError, ProcessStamp, Provider};

use crate::collect::{CollectMode, CollectReport, Collected};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::{AccountRow, Store};
use crate::switch::{AutoFreshened, AutoPerform, SwitchReason, SwitchRequest, SwitchTarget};

/// Where a tick's events go (Decision 6). The CLI renders them as human lines or as JSONL.
pub trait EventSink {
    fn emit(&self, e: &AutoEvent);
    /// The end of a tick of `provider`'s engine: `AutoEngine::tick` calls it exactly once, last,
    /// whatever the tick came to, an `Err` too. A sink that keeps something for the rest of a
    /// tick (the human line's poll) lets it go here.
    fn tick_done(&self, _provider: &ProviderId) {}
}

/// §11.4's events. Every one carries its provider. An account is named by its position, and
/// by its email (its label, for a provider whose logins have none).
#[derive(Debug, Clone, PartialEq)]
pub enum AutoEvent {
    /// After step 3's collection: the live account, every account's decision-grade headroom
    /// by position (`None`: unknown), the threshold, each failed fetch's `last_error` token by
    /// position, and each decision-grade reading's windows by position (key → pct).
    Poll {
        provider: ProviderId,
        active: Option<(u32, String)>,
        headroom_pct: BTreeMap<u32, Option<f64>>,
        threshold: f64,
        fetch_errors: BTreeMap<u32, String>,
        windows_pct: BTreeMap<u32, BTreeMap<String, f64>>,
    },
    /// A switch made, or in dry-run one that would have been made (§11.2 step 10).
    Switch {
        provider: ProviderId,
        trigger: Trigger,
        from: u32,
        to: u32,
        warnings: Vec<String>,
        dry_run: bool,
    },
    /// `reason` is a `NoSwitchReason` in kebab-case; `detail` as `Decision::NoSwitch` has it.
    NoSwitch {
        provider: ProviderId,
        reason: String,
        detail: String,
    },
    /// `reason` is the stored `quarantine_reason` (§6.1).
    AccountQuarantined {
        provider: ProviderId,
        number: u32,
        email: String,
        reason: String,
    },
    /// `reason` is `account-replaced` or `credentials-replaced` (§11.4).
    AccountUnquarantined {
        provider: ProviderId,
        number: u32,
        email: String,
        reason: String,
    },
    /// After `no-switch all-exhausted`: the earliest recovery, epoch seconds.
    AllExhausted {
        provider: ProviderId,
        earliest_reset_at: Option<i64>,
    },
    /// The loop's, before a long sleep (§11.4); a tick never emits it.
    Sleep {
        provider: ProviderId,
        seconds: f64,
        until: i64,
    },
    Error {
        provider: ProviderId,
        message: String,
        transient: bool,
    },
    ConfigWarning {
        provider: ProviderId,
        message: String,
    },
}

/// What one tick came to, for `--once`'s exit code (§11.1, §11.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickOutcome {
    Switched,
    NoAction,
    Blocked,
    Error,
}

impl TickOutcome {
    fn of(d: &Decision) -> Self {
        match d {
            Decision::Switch { .. } => TickOutcome::Switched,
            Decision::NoSwitch {
                outcome: Outcome::NoAction,
                ..
            } => TickOutcome::NoAction,
            Decision::NoSwitch {
                outcome: Outcome::Blocked,
                ..
            } => TickOutcome::Blocked,
        }
    }
}

/// An account's quarantine as a tick last saw it, with its login epoch (Decision 10).
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    reason: Option<String>,
    login_epoch: i64,
}

/// One account as a tick read it from the store: its row, its decision-grade windows under
/// the tick's models (§8.4), and when its reading was taken, as stored.
struct Account {
    row: AccountRow,
    windows: Option<Vec<Window>>,
    fetched_at: Option<i64>,
}

/// The auto-switch engine of one provider (§11.1).
pub struct AutoEngine<'e> {
    engine: &'e Engine,
    provider: ProviderId,
    cfg: AutoConfig,
    dry_run: bool,
    /// The engine lock, held for this engine's life; `None` in dry-run.
    _lock: Option<FlockGuard>,
    /// Dry-run's `unhealthy_ticks`: it writes no auto-switch state (§11.1).
    unhealthy: Option<u32>,
    /// Decision 10: each account's quarantine as the previous tick left it.
    seen: Option<BTreeMap<AccountId, Seen>>,
    /// The `models` the model-name check last ran for (§11.2 step 3).
    checked_models: Option<Vec<String>>,
    /// The strategy the long-window check last ran for (§4.5).
    checked_strategy: Option<Strategy>,
    /// The live account whose ticks `unhealthy_ticks` last counted (Decision 4).
    judged: Option<AccountId>,
    /// The managed live account as the last tick left it.
    live: Option<AccountId>,
}

/// Replaces the held engine lock file's whole content with this process's pid and start time,
/// one JSON line, through a second descriptor: the lock stays on the first. M5's `doctor` reads
/// it; nothing reads it for exclusion.
fn write_holder(path: &Path) -> Result<(), EngineError> {
    let stamp = ProcessStamp::current()?;
    let mut file = OpenOptions::new().write(true).open(path)?;
    file.set_len(0)?;
    file.write_all(format!("{{\"pid\":{},\"start\":{}}}\n", stamp.pid, stamp.start).as_bytes())?;
    Ok(())
}

impl Engine {
    /// §11.1: the auto-switch engine for `provider`. It tries the provider's engine lock,
    /// `locks/autoswitch-<provider>.lock`, once and never waits: `Ok(None)` while another
    /// process holds it (`engine-running`). M5's amendment (`86882e3`): "Once it holds the
    /// lock, the engine writes its pid and start time into the lock file, the start time taken
    /// as for the `switch_journal` holder (§12.6). The record is never used for exclusion."
    /// Dry-run takes no lock and writes nothing here. Like every command that changes the live
    /// login, a real engine refuses inside a `tagteam run` shell; dry-run does not (§11.1).
    pub fn auto(
        &self,
        provider: &ProviderId,
        cfg: AutoConfig,
        dry_run: bool,
    ) -> Result<Option<AutoEngine<'_>>, EngineError> {
        self.provider(provider)?;
        let lock = if dry_run {
            None
        } else {
            self.refuse_inside_run_shell()?;
            let path = self
                .env
                .data_dir()
                .join("locks")
                .join(format!("autoswitch-{provider}.lock"));
            let Some(lock) = FlockGuard::try_lock(&path)? else {
                return Ok(None);
            };
            write_holder(lock.path())?;
            Some(lock)
        };
        Ok(Some(AutoEngine {
            engine: self,
            provider: provider.clone(),
            cfg,
            dry_run,
            _lock: lock,
            unhealthy: None,
            seen: None,
            checked_models: None,
            checked_strategy: None,
            judged: None,
            live: None,
        }))
    }
}

/// An interruption, or a store that failed: a tick returns these as `Err`.
fn fatal(e: &EngineError) -> bool {
    e.signal().is_some() || matches!(e, EngineError::Store(_))
}

/// An error a retry at the next tick may clear by itself.
fn transient(e: &EngineError) -> bool {
    e.kind() == "lock-timeout"
}

/// The decision an `Error` tick returns when it failed before deciding: NO_ACTION, so a loop
/// keeps its normal cadence (§11.4). Its `error` event is the report; this is never rendered.
fn undecided(detail: String) -> Decision {
    Decision::NoSwitch {
        reason: NoSwitchReason::NoActiveAccount,
        outcome: Outcome::NoAction,
        detail,
        earliest_reset: None,
    }
}

/// How events name an account: its email, or its label when its provider's logins have none.
fn email_of(row: &AccountRow) -> String {
    row.email.clone().unwrap_or_else(|| row.label.clone())
}

impl AutoEngine<'_> {
    /// One §11.2 tick. `Err` only for an interruption (§14.1) or a store failure; every other
    /// failure is an `error` event and `TickOutcome::Error`. A switch that committed is
    /// `Switched` even when a signal arrived during its critical span: the token stays set for
    /// the caller's next cancellation point. Every tick ends with `sink.tick_done`.
    pub fn tick(&mut self, sink: &dyn EventSink) -> Result<(TickOutcome, Decision), EngineError> {
        let ticked = match self.run(sink) {
            Err(e) if !fatal(&e) => {
                self.error(sink, e.to_string(), transient(&e));
                Ok((TickOutcome::Error, undecided(e.to_string())))
            }
            done => done,
        };
        sink.tick_done(&self.provider);
        ticked
    }

    /// A settings reload (Decision 9): the next tick decides and collects with `cfg`, and runs
    /// the model-name check again when `models` changed.
    pub fn set_config(&mut self, cfg: AutoConfig) {
        self.cfg = cfg;
    }

    /// When the live account can next be fetched: the later of its planned poll and its usage
    /// lease's expiry, since a lease outlives its record (§8.3) and a tick that wakes inside it
    /// cannot fetch. `None` before a tick has seen a managed live account, or when it has no
    /// plan; a store that cannot be read is `None` too.
    pub fn active_next_poll_at(&self) -> Option<i64> {
        let id = self.live.as_ref()?;
        let store = self.engine.existing_store().ok()??;
        let next = store.usage_state(id).ok()??.next_poll_at?;
        let lease = store.usage_lease_expires_at(id).ok().flatten();
        Some(lease.map_or(next, |ms| next.max((ms + 999).div_euclid(1000))))
    }

    fn run(&mut self, sink: &dyn EventSink) -> Result<(TickOutcome, Decision), EngineError> {
        // §14.1: between ticks.
        self.engine.check_cancel()?;
        let p = self.engine.provider(&self.provider)?;
        let p = p.as_ref();
        // Step 1. Dry-run releases nothing (§11.1).
        let released = if self.dry_run {
            Vec::new()
        } else {
            self.engine
                .release_unbound_quarantines(&self.provider, "auto")?
        };
        let store = self.engine.existing_store()?;
        let rows = match &store {
            Some(s) => s.accounts(&self.provider)?,
            None => Vec::new(),
        };
        self.report_quarantines(&rows, &released, sink);
        // Step 2: recovery first, then the live check.
        if let Some(refusal) = self.recover()? {
            return Ok(self.no_switch(
                sink,
                NoSwitchReason::InterruptedSwitch,
                refusal.to_string(),
            ));
        }
        let live = match self.engine.read_live_identity(p)? {
            None => None,
            Some(identity) => {
                let key = p.identity_key(&identity);
                match rows.iter().find(|r| r.identity_key == key.as_str()) {
                    Some(row) => Some(row.clone()),
                    None => {
                        self.live = None;
                        return Ok(self.no_switch(
                            sink,
                            NoSwitchReason::UnmanagedActiveAccount,
                            String::new(),
                        ));
                    }
                }
            }
        };
        let (Some(store), Some(live)) = (store, live) else {
            self.live = None;
            return Ok(self.no_switch(sink, NoSwitchReason::NoActiveAccount, String::new()));
        };
        self.live = Some(live.id.clone());
        // Step 3.
        let report = self.engine.collect_usage(CollectMode::Scheduled {
            provider: self.provider.clone(),
            threshold: self.cfg.threshold,
            models: self.cfg.models.clone(),
        })?;
        let mut accounts = self.read(&store)?;
        self.report_collection(&report, &accounts, sink);
        self.poll(&report, &accounts, &live, sink);
        self.check_settings(p, &store, &accounts, sink)?;
        // Steps 4 to 9.
        let (state, stored) = self.state(&store, &live.id)?;
        let mut snap = self.snapshot(p, &accounts, &live.id);
        let mut decided = decide(&snap, &state, &self.cfg, Phase::Initial);
        if let Decision::Switch { recheck: true, .. } = &decided.decision {
            // Decision 2: re-check the current account and every candidate, then decide again
            // from what the store holds now, with the same state.
            let report = self.engine.collect_usage(CollectMode::Recheck {
                accounts: recheck_ids(&snap, &live.id),
                threshold: self.cfg.threshold,
                models: self.cfg.models.clone(),
            })?;
            accounts = self.read(&store)?;
            self.report_collection(&report, &accounts, sink);
            snap = self.snapshot(p, &accounts, &live.id);
            decided = decide(&snap, &state, &self.cfg, Phase::Rechecked);
        }
        self.judged = Some(live.id.clone());
        let ticked = Ticked {
            store: &store,
            live: &live,
            snap,
            stored,
            unhealthy: decided.unhealthy_ticks,
        };
        match decided.decision {
            Decision::Switch {
                trigger,
                ref targets,
                ..
            } => self.perform(p, &ticked, trigger, targets, &decided.decision, sink),
            decision => {
                self.count(&ticked)?;
                self.announce(sink, &decision);
                Ok((TickOutcome::of(&decision), decision))
            }
        }
    }

    /// Steps 10 to 12 for a `Switch` decision: each target in order, freshened (§7.2) and
    /// then switched to through the ordinary transaction, with the tick's preconditions.
    fn perform(
        &mut self,
        p: &dyn Provider,
        t: &Ticked<'_>,
        trigger: Trigger,
        targets: &[AccountId],
        decision: &Decision,
        sink: &dyn EventSink,
    ) -> Result<(TickOutcome, Decision), EngineError> {
        let mut failed = Vec::new();
        for id in targets {
            let Some(row) = t.store.account(id)? else {
                continue;
            };
            if self.dry_run {
                // §11.1: it freshens and switches nothing.
                self.count(t)?;
                self.emit(sink, |provider| AutoEvent::Switch {
                    provider,
                    trigger,
                    from: t.live.position,
                    to: row.position,
                    warnings: Vec::new(),
                    dry_run: true,
                });
                return Ok((TickOutcome::Switched, decision.clone()));
            }
            let fresh = match self.engine.freshen_auto(p, &row) {
                Ok(fresh) => fresh,
                Err(e) if fatal(&e) => return Err(e),
                Err(e) => AutoFreshened::Failed(e.to_string()),
            };
            match fresh {
                AutoFreshened::Ready => {}
                AutoFreshened::Quarantined => {
                    if let Some(now) = t.store.account(id)? {
                        self.note_quarantined(&now, sink);
                    }
                    continue;
                }
                AutoFreshened::Failed(why) => {
                    failed.push(format!("{} (position {}): {why}", row.label, row.position));
                    continue;
                }
                AutoFreshened::Skip => continue,
            }
            // Step 11.
            let req = SwitchRequest {
                provider: self.provider.clone(),
                target: SwitchTarget::Account(id.clone()),
                force: false,
                source: "auto",
                auto: Some(AutoPerform {
                    expected_from: t.live.id.clone(),
                    trigger,
                    cooldown_s: self.cfg.cooldown_s,
                    departure: departure(&t.snap, &self.cfg, &t.live.id, trigger),
                }),
            };
            match self.engine.switch(req) {
                Ok(out) if out.switched => {
                    // The commit reset `unhealthy_ticks` (Decision 4): nothing to count.
                    self.live = Some(id.clone());
                    self.judged = Some(id.clone());
                    self.emit(sink, |provider| AutoEvent::Switch {
                        provider,
                        trigger,
                        from: t.live.position,
                        to: row.position,
                        warnings: out.warnings,
                        dry_run: false,
                    });
                    return Ok((TickOutcome::Switched, decision.clone()));
                }
                Ok(out) => match out.reason {
                    SwitchReason::NotCandidate => continue,
                    SwitchReason::Cooldown => {
                        self.count(t)?;
                        let left = self.cooldown_left(t.store)?;
                        return Ok(self.no_switch(sink, NoSwitchReason::Cooldown, left));
                    }
                    SwitchReason::LiveChanged => {
                        self.count(t)?;
                        return Ok(self.no_switch(
                            sink,
                            NoSwitchReason::LiveChanged,
                            String::new(),
                        ));
                    }
                    // No other no-op answers a direct switch to an account other than the live
                    // one; should one ever, it is reported, never taken for a switch.
                    _ => {
                        self.count(t)?;
                        self.error(sink, out.message, false);
                        return Ok((TickOutcome::Error, decision.clone()));
                    }
                },
                Err(e) if fatal(&e) => return Err(e),
                Err(e) => {
                    self.count(t)?;
                    self.error(
                        sink,
                        format!(
                            "could not switch to {} (position {}): {e}",
                            row.label, row.position
                        ),
                        transient(&e),
                    );
                    return Ok((TickOutcome::Error, decision.clone()));
                }
            }
        }
        // Step 12.
        self.count(t)?;
        if !failed.is_empty() {
            self.error(
                sink,
                format!("could not freshen {}", failed.join("; ")),
                true,
            );
            return Ok((TickOutcome::Error, decision.clone()));
        }
        Ok(self.no_switch(sink, NoSwitchReason::NoViableTarget, String::new()))
    }

    /// §11.2 step 2: an unresolved switch journal for the provider is recovered first, under
    /// `MutationGuard`, its events carrying `source` = `auto` (§9.6). `Some` with recovery's
    /// refusal while a row stays: one recovery could not decide, could not take the live
    /// locks or found its agent writing, or the mutation lock was held past its timeout.
    fn recover(&self) -> Result<Option<EngineError>, EngineError> {
        match self.engine.settle_or_refuse_as(&self.provider, "auto") {
            Ok(()) => Ok(None),
            Err(e) if e.signal().is_some() => Err(e),
            Err(
                e @ (EngineError::InterruptedSwitch(_)
                | EngineError::RecoveryBlocked { .. }
                | EngineError::RecoveryMoved { .. }
                | EngineError::Lock(LockError::Timeout(_))),
            ) => Ok(Some(e)),
            Err(e) => Err(e),
        }
    }

    /// The provider's accounts as the store holds them now, each with its decision-grade
    /// windows under the tick's models (§8.4).
    fn read(&self, store: &Store) -> Result<Vec<Account>, EngineError> {
        store
            .accounts(&self.provider)?
            .into_iter()
            .map(|row| {
                Ok(Account {
                    windows: self.engine.decision_windows(&row, &self.cfg.models)?,
                    fetched_at: store.usage_state(&row.id)?.and_then(|s| s.fetched_at),
                    row,
                })
            })
            .collect()
    }

    /// The snapshot `decide` reads. Switchable is decided from the store alone (enabled, with
    /// an identity): the vault is read lazily, at step 10 (§9.3), where a target with no
    /// stored credential is passed over. Nothing is session-owned before M4.
    fn snapshot(&self, p: &dyn Provider, accounts: &[Account], live: &AccountId) -> Snapshot {
        Snapshot {
            now: self.engine.now_ms().div_euclid(1000),
            live: Live::Managed(live.clone()),
            accounts: accounts
                .iter()
                .map(|a| AccountSnapshot {
                    id: a.row.id.clone(),
                    position: a.row.position,
                    api_key: p.kind_traits(&a.row.kind).managed_key_axis,
                    switchable: !a.row.disabled && a.row.identity_json.is_object(),
                    quarantined: a.row.quarantine_reason.is_some(),
                    session_owned: false,
                    windows: a.windows.clone(),
                    fetched_at: a.fetched_at,
                })
                .collect(),
        }
    }

    /// The state this tick decides with, and `unhealthy_ticks` as stored. Dry-run counts in
    /// memory. The count belongs to the account it judged (Decision 4): a live account that a
    /// switch made elsewhere since the last tick starts from 0.
    fn state(&self, store: &Store, live: &AccountId) -> Result<(AutoState, u32), EngineError> {
        let mut state = store.autoswitch_state(&self.provider)?;
        if let Some(n) = self.unhealthy.filter(|_| self.dry_run) {
            state.unhealthy_ticks = n;
        }
        let stored = state.unhealthy_ticks;
        if self.judged.as_ref().is_some_and(|judged| judged != live) {
            state.unhealthy_ticks = 0;
        }
        Ok((state, stored))
    }

    /// Writes this tick's `unhealthy_ticks` when it changed (Decision 4); dry-run keeps it.
    fn count(&mut self, t: &Ticked<'_>) -> Result<(), EngineError> {
        if self.dry_run {
            self.unhealthy = Some(t.unhealthy);
        } else if t.unhealthy != t.stored {
            t.store.set_unhealthy_ticks(&self.provider, t.unhealthy)?;
        }
        Ok(())
    }

    /// The cooldown left as `no-switch cooldown` states it, from the state as stored now.
    fn cooldown_left(&self, store: &Store) -> Result<String, EngineError> {
        let now = self.engine.now_ms().div_euclid(1000);
        Ok(store
            .autoswitch_state(&self.provider)?
            .last_switch_at
            .map_or_else(String::new, |at| {
                span(at.saturating_add(self.cfg.cooldown_s) - now)
            }))
    }

    /// Step 1's quarantine report: this tick's own releases, and, against the previous tick's
    /// map, every quarantine another process set or cleared since (Decision 10). The first
    /// tick has no previous map, so it reports only its own releases.
    fn report_quarantines(
        &mut self,
        rows: &[AccountRow],
        released: &[AccountId],
        sink: &dyn EventSink,
    ) {
        let before = self.seen.take();
        for row in rows {
            let was = before.as_ref().and_then(|m| m.get(&row.id));
            let reason = || {
                match was {
                    Some(s) if s.login_epoch != row.login_epoch => "account-replaced",
                    _ => "credentials-replaced",
                }
                .to_owned()
            };
            let cleared = released.contains(&row.id)
                || (was.is_some_and(|s| s.reason.is_some()) && row.quarantine_reason.is_none());
            let set = before.is_some()
                && row.quarantine_reason.is_some()
                && was.is_none_or(|s| s.reason.is_none());
            if cleared {
                self.emit(sink, |provider| AutoEvent::AccountUnquarantined {
                    provider,
                    number: row.position,
                    email: email_of(row),
                    reason: reason(),
                });
            } else if set {
                self.emit(sink, |provider| AutoEvent::AccountQuarantined {
                    provider,
                    number: row.position,
                    email: email_of(row),
                    reason: row.quarantine_reason.clone().unwrap_or_default(),
                });
            }
        }
        self.seen = Some(
            rows.iter()
                .map(|r| {
                    let seen = Seen {
                        reason: r.quarantine_reason.clone(),
                        login_epoch: r.login_epoch,
                    };
                    (r.id.clone(), seen)
                })
                .collect(),
        );
    }

    /// A quarantine this tick's own work found (a collection's, or step 10's freshening),
    /// reported once: the next tick's comparison already knows it.
    fn note_quarantined(&mut self, row: &AccountRow, sink: &dyn EventSink) {
        let Some(reason) = row.quarantine_reason.clone() else {
            return;
        };
        let seen = self.seen.get_or_insert_with(BTreeMap::new);
        let entry = seen.entry(row.id.clone()).or_insert(Seen {
            reason: None,
            login_epoch: row.login_epoch,
        });
        if entry.reason.is_some() {
            return;
        }
        entry.reason = Some(reason.clone());
        self.emit(sink, |provider| AutoEvent::AccountQuarantined {
            provider,
            number: row.position,
            email: email_of(row),
            reason,
        });
    }

    /// A collection's findings: the accounts it saw quarantined, and its warnings, each an
    /// `error` event that does not fail the tick (§8.3: a usage failure is never an error).
    fn report_collection(
        &mut self,
        report: &CollectReport,
        accounts: &[Account],
        sink: &dyn EventSink,
    ) {
        for id in &report.quarantined {
            if let Some(a) = accounts.iter().find(|a| &a.row.id == id) {
                self.note_quarantined(&a.row, sink);
            }
        }
        for warning in &report.warnings {
            self.error(sink, warning.clone(), true);
        }
    }

    /// Step 3's `poll` event (§11.4).
    fn poll(
        &self,
        report: &CollectReport,
        accounts: &[Account],
        live: &AccountRow,
        sink: &dyn EventSink,
    ) {
        let position = |id: &AccountId| {
            accounts
                .iter()
                .find(|a| &a.row.id == id)
                .map(|a| a.row.position)
        };
        let fetch_errors = report
            .outcomes
            .iter()
            .filter_map(|(id, collected)| {
                let kind = match collected {
                    Collected::Failed { kind } => kind.clone(),
                    Collected::OverBudget { .. } => "over-budget".to_owned(),
                    _ => return None,
                };
                Some((position(id)?, kind))
            })
            .collect();
        let headroom_pct = accounts
            .iter()
            .map(|a| {
                let h = a
                    .windows
                    .as_deref()
                    .and_then(|w| headroom(w, &self.cfg.models));
                (a.row.position, h)
            })
            .collect();
        let windows_pct = accounts
            .iter()
            .filter_map(|a| {
                let windows = a.windows.as_ref()?;
                let pct = windows.iter().map(|w| (w.key.clone(), w.pct)).collect();
                Some((a.row.position, pct))
            })
            .collect();
        self.emit(sink, |provider| AutoEvent::Poll {
            provider,
            active: Some((live.position, email_of(live))),
            headroom_pct,
            threshold: self.cfg.threshold,
            fetch_errors,
            windows_pct,
        });
    }

    /// Step 3's settings checks, once per engine and again when the setting changes: a
    /// consume-first strategy on a provider without a long window runs `best` (§4.5), and
    /// each configured model name should be a scoped window some account's reading reports.
    /// The model check waits for a tick on which some account has a reading.
    fn check_settings(
        &mut self,
        p: &dyn Provider,
        store: &Store,
        accounts: &[Account],
        sink: &dyn EventSink,
    ) -> Result<(), EngineError> {
        if self.checked_strategy != Some(self.cfg.strategy) {
            if self.cfg.effective_strategy() != self.cfg.strategy {
                let message = format!(
                    "{} has no long usage window to rank by, so consume-first runs best",
                    p.display_name()
                );
                self.emit(sink, |provider| AutoEvent::ConfigWarning {
                    provider,
                    message,
                });
            }
            self.checked_strategy = Some(self.cfg.strategy);
        }
        if self.checked_models.as_ref() == Some(&self.cfg.models) {
            return Ok(());
        }
        let names: Vec<&String> = self
            .cfg
            .models
            .iter()
            .filter(|m| !m.eq_ignore_ascii_case("all"))
            .collect();
        if !names.is_empty() {
            let mut read_any = false;
            let mut scoped = BTreeSet::new();
            for a in accounts {
                if let Some(windows) = store.usage_state(&a.row.id)?.and_then(|s| s.last_good) {
                    read_any = true;
                    scoped.extend(
                        windows
                            .iter()
                            .filter(|w| w.kind == WindowKind::Scoped)
                            .map(|w| w.label.to_lowercase()),
                    );
                }
            }
            if !read_any {
                return Ok(());
            }
            for name in names {
                if !scoped.contains(&name.to_lowercase()) {
                    let message = format!(
                        "autoswitch.models names {name:?}, but no account's usage reports a window for that model"
                    );
                    self.emit(sink, |provider| AutoEvent::ConfigWarning {
                        provider,
                        message,
                    });
                }
            }
        }
        self.checked_models = Some(self.cfg.models.clone());
        Ok(())
    }

    fn emit(&self, sink: &dyn EventSink, event: impl FnOnce(ProviderId) -> AutoEvent) {
        sink.emit(&event(self.provider.clone()));
    }

    fn error(&self, sink: &dyn EventSink, message: String, transient: bool) {
        self.emit(sink, |provider| AutoEvent::Error {
            provider,
            message,
            transient,
        });
    }

    /// A `no-switch` the engine reports itself (§11.2 steps 2, 11 and 12), announced.
    fn no_switch(
        &self,
        sink: &dyn EventSink,
        reason: NoSwitchReason,
        detail: String,
    ) -> (TickOutcome, Decision) {
        let decision = Decision::NoSwitch {
            reason,
            outcome: reason.outcome(),
            detail,
            earliest_reset: None,
        };
        self.announce(sink, &decision);
        (TickOutcome::of(&decision), decision)
    }

    /// `no-switch`, then `all-exhausted` with its earliest recovery when that is the reason.
    fn announce(&self, sink: &dyn EventSink, decision: &Decision) {
        let Decision::NoSwitch {
            reason,
            detail,
            earliest_reset,
            ..
        } = decision
        else {
            return;
        };
        self.emit(sink, |provider| AutoEvent::NoSwitch {
            provider,
            reason: reason.as_str().to_owned(),
            detail: detail.clone(),
        });
        if *reason == NoSwitchReason::AllExhausted {
            self.emit(sink, |provider| AutoEvent::AllExhausted {
                provider,
                earliest_reset_at: *earliest_reset,
            });
        }
    }
}

/// What a tick has read by the time it decided.
struct Ticked<'a> {
    store: &'a Store,
    live: &'a AccountRow,
    snap: Snapshot,
    /// `unhealthy_ticks` as stored, and as this tick decided it.
    stored: u32,
    unhealthy: u32,
}

/// Consume-first's re-check (§11.2 step 8): the current account and every OAuth candidate.
fn recheck_ids(snap: &Snapshot, live: &AccountId) -> Vec<AccountId> {
    let candidates = snap.accounts.iter().filter(|a| {
        &a.id != live && a.switchable && !a.quarantined && !a.session_owned && !a.api_key
    });
    std::iter::once(live.clone())
        .chain(candidates.map(|a| a.id.clone()))
        .collect()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --features test-hooks --test auto_tick --test auto_engine`
Expected: PASS: `auto_tick` 28, `auto_engine` 5. Without `test-hooks`, the five hook tests in
`auto_tick.rs` are compiled out.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS. `recover.rs` still sees `switch-recovered` with `source` `cli` from a plain
command's recovery.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS, apart from the sandbox's pseudo-terminal failures named in Task 7's Step 4.

- [ ] **Step 5: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/auto.rs crates/tagteam-engine/src/lib.rs \
  crates/tagteam-engine/src/engine.rs crates/tagteam-engine/src/recover.rs \
  crates/tagteam-engine/src/switch.rs crates/tagteam-engine/src/store/usage.rs \
  crates/tagteam-engine/tests/common/mod.rs crates/tagteam-engine/tests/auto_tick.rs \
  crates/tagteam-engine/tests/auto_engine.rs
git commit -m "Run auto-switch ticks behind a per-provider engine lock and report them as events"
```

---

### Task 9: The loop: delays, wall-clock sleep, settings reload, several providers

§11.4: "The loop sleeps toward a wall-clock deadline, in slices of at most 1 s, and checks the
cancel token in each slice (§14.1). A machine that was suspended therefore ticks as soon as it
wakes past the deadline. macOS's monotonic clock stops during sleep, so a monotonic sleep would
add the suspended time to the delay." §11.1: "`tagteam auto` runs one independent engine per
provider that has at least two switchable accounts, or only the provider given with
`--provider`. … One process ticks its providers on independent schedules." This task writes that
driver, `crates/tagteam/src/auto.rs` (Decision 7). It takes each provider's `AutoEngine`
(Task 8), ticks whichever is due, schedules its next tick with `next_delay` (Task 4), reads
`config.toml` again when its mtime changed (Decision 9), and stops with exit 0 on a signal
(Decision 8). `--once` runs one tick per provider and exits with the most severe code (Task 6).
Nothing calls it yet; Task 10 makes it `tagteam auto`.

**Readings of the spec this task commits to:**
- **One clock.** The loop reads the engine's clock (`Engine::now_ms`), the wall clock the tick
  reads too, so a jump moves both. The binary's is `SystemClock`. The `Sleeper` only waits
  (`ThreadSleeper` is `std::thread::sleep`); no deadline is ever measured with it.
- **Deadlines.** A provider's next tick is due `next_delay(…)` seconds after the clock reads when
  its tick returned. The call gets the tick's decision, the provider's `AutoConfig`,
  `active_next_poll_at()` and one jitter draw. Each pass of the loop either ticks the provider
  whose deadline has passed (the earliest; on a tie, the first provider) or sleeps
  `min(1 s, deadline − now)`. The cancel token is checked before every pass, so before every
  slice and every tick (§14.1: "in `auto`'s sleep, and between its ticks").
- **A clock set back.** A deadline never lies further ahead than the delay it was set for:
  before each pass every deadline is clamped to `now + delay`. Without that, a wall clock set back
  an hour, by hand or by a time-sync step, would hold the next tick an hour longer. The spec names
  only the forward case (a suspend); this is its mirror.
- **Which providers.** `--provider P` gives `[P]`, even with fewer than two accounts: its ticks
  then say why nothing switches. Otherwise every registered provider with at least two enabled
  accounts that have an identity, read from the store as Task 8's snapshot reads `switchable`, in
  provider-ID order.
- **The engine locks.** A loop takes every provider's engine first and holds them all for its
  life. A provider whose lock another process holds is skipped with a `no-switch
  engine-running` event before the first tick; that event is the warning, which Task 10 prints on
  stderr. With no lock left, the loop fails with `AutoError::AlreadyRuns` before any tick
  (§11.1: "If it gets no provider's lock at all, it exits 1").
- **Settings.** Before each tick, `Settings::mtime` is compared with its value when the settings
  were last read (`run_loop` reads it before its first `Settings::load`). On a change, every
  provider's settings are read again, the invocation's flags are applied over them by
  `auto_config`, and `AutoEngine::set_config` gets the result. Each warning the read gives is a
  `config-warning`. The flags still win (§6.4, §11.4), clamped to §6.4's ranges. A change made
  during a long sleep takes effect at the next tick, as §11.4 says ("before a tick").
- **Events.** `sleep` when `announces_sleep(delay)`: `seconds` is the delay, `until` the epoch
  second it ends. Every other event is the tick's.
- **Errors.** A tick that returns `TickOutcome::Error` has emitted its own `error` event. Its
  decision is a `Switch` or a NO_ACTION `no-switch`, so `next_delay` gives the normal jittered
  interval (§11.4: "the loop goes on after the normal delay"). An `Err` that carries no signal
  (a store failure, the one other kind Task 8 returns) becomes an `error` event here, with the
  same cadence.
- **Signals.** The token seen before a pass, or an `Err` carrying a signal from inside a tick,
  ends the loop with exit 0. A switch that committed despite a late signal returns `Switched`,
  and the next pass stops the loop.
- **`--once`.** One tick per provider, in order, each engine taken just before its tick and
  dropped after it. A provider held elsewhere reports `no-switch engine-running`, code 2.
  `TickOutcome::Error` is 1, anything else `once_exit_code(&decision)`, and the result is
  `most_severe`. A signal meets the next tick's opening check and returns
  `Err(AutoError::Engine(_))` carrying it; Task 10 exits 128 + n (§14.1).
- **Consume-first named for a provider without a long window.** §4.5: "`auto --strategy
  consume-first` with `--provider` naming it is a usage error (exit 2)". `run_loop` checks that
  combination once it has the providers and before any engine starts, for a loop and `--once`
  alike, and returns `AutoError::NoLongWindow(provider)`; Task 10 makes it the usage error. The
  two other ways to ask for consume-first are not errors: from `autoswitch.strategy`, or with
  the flag and no `--provider`, that provider's engine runs `best` with one `config-warning`
  (Task 8).

**Rulings (what — why — cost if wrong):**
- **A quarantine does not count against an account when choosing providers** — §9.3's
  "switchable" excludes quarantined accounts, but the choice is made once, at start: a provider
  whose second account was quarantined then would never be driven, not even after a new login,
  and the failover from a quarantined live account (§11.2 step 5) could never run — such a
  provider is driven and reports `no-candidates` until the quarantine lifts.
- **No provider to drive is `AutoError::NothingToSwitch`, for the loop and `--once` alike**
  (Task 10: exit 1) — a loop could do nothing, and accounts added later would not join it — a
  cron `--once` on a one-account machine exits 1, not 2.
- **A loop's skip warning is the `no-switch engine-running` event** — §11.4 lists
  `engine-running` among the `no-switch` reasons, and a JSONL consumer learns which provider is
  not driven — a consumer that counts `no-switch` events as ticks counts one more per skipped
  provider.
- **The settings file's warnings at start are the command's** — every command prints the
  warnings for the provider it resolves (`build_engine`, `run_command`); the loop emits only a
  reload's — a warning from another provider's own table goes unshown until a reload.
- **A tick's store failure is an `error` event with `transient: false`** — nothing says a retry
  will clear it — none: the loop retries at the normal cadence either way.

**Files:**
- Create: `crates/tagteam/src/auto.rs`
- Modify: `crates/tagteam/src/lib.rs` (`pub mod auto`)
- Modify: `crates/tagteam/Cargo.toml`: `fastrand` for the jitter (already in the build through
  `tagteam-engine`), and `tagteam-fake` as a dev-dependency so the loop's tests drive two
  providers; `Cargo.lock` follows

**Interfaces:**
- Consumes:
  - Task 1: `Settings::{load, mtime}` and its fields, `THRESHOLD_RANGE`,
    `INTERVAL_SECONDS_RANGE`, `COOLDOWN_SECONDS_RANGE`.
  - Task 2: `Provider::primary_long_window`.
  - Tasks 4–6: `AutoConfig`, `Strategy`, `Decision`, `NoSwitchReason::{as_str, outcome}`,
    `next_delay`, `announces_sleep`, `once_exit_code`, `most_severe`.
  - Task 8: `Engine::auto`, `AutoEngine::{tick, set_config, active_next_poll_at}`, `AutoEvent`,
    `EventSink`, `TickOutcome`.
  - Existing: `Engine::{now_ms, cancel, env, existing_store, store, provider, add_live,
    on_point}`, `Store::{all_accounts, account, reserve_usage, record_usage, set_quarantine,
    set_disabled}`, `FakeClock`, `FakeKeychain`, `ScriptedHttp`, `NoOracle`,
    `tagteam_fake::{login, FakeAgent, FAKE_AGENT}`.
- Produces, in `tagteam::auto` (the contract's `Sleeper`, and `run_loop` with its signature
  settled here):
  - `pub trait Sleeper { fn sleep(&self, d: Duration); }` and `pub struct ThreadSleeper`
  - `pub fn uniform_jitter() -> f64`, uniform in [-1, 1)
  - `pub struct AutoFlags { pub threshold: Option<f64>, pub interval_s: Option<i64>,
    pub cooldown_s: Option<i64>, pub strategy: Option<Strategy>, pub models:
    Option<Vec<String>>, pub include_api_key_accounts: Option<bool> }` (`Debug, Clone, Default,
    PartialEq`)
  - `pub fn auto_config(s: &Settings, flags: &AutoFlags, long_window: Option<&str>) -> AutoConfig`
  - `pub struct AutoRun { pub provider: Option<ProviderId>, pub flags: AutoFlags, pub once: bool,
    pub dry_run: bool }` (`Debug, Clone, Default, PartialEq`)
  - `pub enum AutoError { AlreadyRuns(Vec<ProviderId>), NothingToSwitch, NoLongWindow(ProviderId),
    Engine(EngineError) }`, with `From<EngineError>`
  - `pub fn providers(engine: &Engine, only: Option<&ProviderId>) -> Result<Vec<ProviderId>,
    EngineError>`
  - `pub fn run_loop(engine: &Engine, run: &AutoRun, sink: &dyn EventSink, sleeper: &dyn
    Sleeper, jitter: &mut dyn FnMut() -> f64) -> Result<i32, AutoError>`: `Ok` with `--once`'s
    code, or the loop's 0 once a signal stops it. The contract's `-> i32` became a `Result`
    because a loop that cannot start and an interrupted `--once` are the command's errors
    (§13.2's error object, §14.1's 128 + n), not exit codes the loop chooses.

- [ ] **Step 1: Write the failing tests**

The tests drive the real `AutoEngine` (Task 8) against Claude Code and FakeAgent in one fixture
home, on a `FakeClock` that the `Slices` sleeper moves forward one slice at a time, with a
`ScriptedHttp` that answers nothing (every request fails as `PreSend`). Readings are recorded
with a plan in force, so a tick fetches only where a test makes it. What they pin:
- `each_provider_ticks_on_its_own_wall_clock_schedule_in_slices_of_a_second`: two providers on
  their own intervals (Claude Code's default 60 s, FakeAgent's 20 s from its own table), the
  jitter applied, every slice at most a second.
- `the_next_tick_comes_no_later_than_the_live_account_s_next_poll`: §11.4's shortening.
- `a_long_delay_is_announced_with_a_sleep_event`: Review Focus 4, the loop's half.
- `a_suspend_past_the_deadline_ticks_on_waking_and_never_on_the_old_readings`: Review Focus 2.
  The clock jumps an hour during one slice. The next tick comes at once; the readings are then
  past even §8.4's extended trust, so the tick fetches each account once (one request each, all
  after waking) and reports `active-usage-unknown 1/3`, never `below-threshold` from the old
  reading.
- `a_wall_clock_set_back_never_stretches_the_wait_past_the_delay`: the mirror case.
- `changed_settings_are_read_again_before_the_next_tick_and_flags_still_win`: Decision 9.
- `a_failed_tick_is_reported_and_the_loop_keeps_its_normal_cadence` and
  `a_tick_that_fails_with_an_error_is_an_error_event_and_an_interruption_is_not`: errors.
- `a_signal_met_inside_a_tick_stops_the_loop_with_exit_0` (needs the engine's test hooks).
- `a_loop_skips_a_provider_whose_engine_runs_elsewhere_and_fails_when_none_is_left`: Review
  Focus 3, the loop's half.
- `once_ticks_each_provider_once_and_exits_with_the_most_severe_code` and
  `an_interrupted_once_is_the_command_s_interruption`.
- `consume_first_named_for_a_provider_without_a_long_window_is_refused_before_any_engine`:
  §4.5's usage error, against FakeAgent: `--provider` naming it with `--strategy
  consume-first` ticks nothing and takes no engine lock, in a loop and with `--once`.
- `consume_first_from_the_settings_or_without_provider_runs_best_with_one_warning`: §4.5's two
  fallbacks, tested apart from the error. Neither is refused; FakeAgent's engine runs `best` and
  warns once.
- `auto_runs_every_provider_with_two_switchable_accounts_or_the_one_named`,
  `with_no_provider_to_switch_on_auto_fails_before_any_tick`,
  `flags_override_the_settings_clamped_to_their_ranges` and
  `the_jitter_draw_stays_within_its_range`.

In `crates/tagteam/Cargo.toml`, replace

```toml
[dev-dependencies]
tagteam-provider = { workspace = true, features = ["file-keychain", "mock-server"] }
```

with

```toml
[dev-dependencies]
tagteam-fake.workspace = true
tagteam-provider = { workspace = true, features = ["file-keychain", "mock-server"] }
```

In `crates/tagteam/src/lib.rs`, replace

```rust
pub mod app;
pub mod cli;
```

with

```rust
pub mod app;
pub mod auto;
pub mod cli;
```

Create `crates/tagteam/src/auto.rs` with only its module doc and its tests (Step 3 adds the code
between them):

```rust
//! §11.4: the `auto` loop, a thin driver over `AutoEngine::tick` (Decision 7). It runs one
//! engine per provider on independent schedules, sleeps toward each one's wall-clock deadline
//! in slices of at most a second that check the cancel token, re-reads `config.toml` before a
//! tick when its mtime changed (Decision 9), and stops with exit 0 on a signal (Decision 8).
//! `--once` runs one tick per provider instead.

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
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam --features test-support --lib auto::`
Expected: FAIL to compile: ``could not compile `tagteam` (lib test)``, with errors such as
``cannot find function `run_loop` in this scope``, ``cannot find type `AutoRun` in this scope``
and ``use of undeclared type `AutoFlags` ``.

- [ ] **Step 3: Implement**

In `crates/tagteam/Cargo.toml`, replace

```toml
clap.workspace = true
libc.workspace = true
```

with

```toml
clap.workspace = true
fastrand.workspace = true
libc.workspace = true
```

In `crates/tagteam/src/auto.rs`, insert between the module doc and the `#[cfg(test)]` line that opens `mod tests`:

```rust
use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::time::Duration;

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

```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam --features test-support --lib auto::`
Expected: PASS, 18 tests.

Run: `cargo test -p tagteam --lib auto::`
Expected: PASS, 17 tests: without the features `a_signal_met_inside_a_tick_stops_the_loop_with_exit_0`
is compiled out, since it needs the engine's `on_point` hook.

- [ ] **Step 5: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add Cargo.lock crates/tagteam/Cargo.toml crates/tagteam/src/lib.rs crates/tagteam/src/auto.rs
git commit -m "Drive each provider's auto-switch engine on its own wall-clock schedule"
```

---

### Task 10: `tagteam auto`

§11.4: "The flags `--once`, `--dry-run`, `--json`, `--threshold`, `--interval`, `--cooldown`,
`--strategy`, `--model` and `--include-api-key-accounts` override the settings." "Human output.
Each tick prints one line on stdout: the local time, the active account (position and label), its
relevant usage, and the outcome (the reason, or the switch). Quarantine changes and long sleeps
get lines of their own. Errors and config warnings go to stderr." "`--json` emits one JSON object
per line, with the envelope `{"schemaVersion":1,"event":<kind>,"ts":"…Z",…}`." §11.1: "Like every
command that changes the live login, `auto` refuses inside a `tagteam run` shell (§9.2), except
with `--dry-run`. On macOS it runs the Keychain lock check before its first tick (Appendix A.3)."
This task adds the command, its two event sinks and its exit codes, with binary tests. It first
lands M3a's carry-over (Decision 12): a spawn helper that drains a child's stdout and stderr while
it runs, since a loop prints until it is stopped.

**Readings of the spec this task commits to:**
- **The command.** `tagteam auto [--once] [--dry-run] [--threshold N] [--interval S] [--cooldown S]
  [--strategy best|consume-first] [--model M] [--include-api-key-accounts BOOL]`, with the global
  `--json` and `--provider`. The numbers are clamped to §6.4's ranges by Task 9's `auto_config`,
  as §6.4 says a flag is. A non-finite `--threshold` (clap reads `nan` and `inf` as numbers; Task
  1's ruling) and an `--include-api-key-accounts` that is not one of §6.4's six words are usage
  errors, exit 2. `--model` is `switch --model`'s comma-separated list (`model_list`).
- **Where it runs.** Inside a run shell `auto` refuses with `inside-run-shell`, exit 1, unless
  `--dry-run`, before anything else could stop it. On macOS the Keychain lock check runs before
  the first tick, for a dry run too, since every tick reads its accounts' Keychain items
  (`Command::touches_keychain`). With no store there is nothing to switch between, so no check
  runs, as for `switch` and `remove`.
- **Exit codes.** `--once` exits with Task 9's code (§11.1). The loop exits 0 once a signal stops
  it, without the "interrupted too late" notice, since a signal is how it ends (§11.4).
  `AutoError::AlreadyRuns` is the error `engine-running` ("auto-switch already runs for Claude
  Code") and `NothingToSwitch` the error `no-candidates`, both exit 1, in §13.2's one error object
  under `--json`. An interrupted `--once` exits 128 + n with the `interrupted` error, by
  `run_command`'s existing path for an engine error that carries a signal (§14.1). `dispatch` now
  returns the exit code. `AutoError::NoLongWindow` (Task 9) is a usage error, exit 2, in the
  usage error's shape, as every bad flag value is (§4.5): "--strategy consume-first needs a long
  usage window to rank by, and <provider> has none". `auto_failure` maps each `AutoError` to the
  command's failure.
- **`--json`.** One object per event and line, written as the event happens. The envelope
  `schemaVersion`, `event`, `ts` (the engine's clock, ISO 8601 UTC to the second) and `provider`
  come first, then §11.4's fields in cswap's spelling:
  - `poll`: `active` `{number, email}`, `headroomPct` `{"<position>": pct or null}`, `threshold`,
    and `fetchErrors` `{"<position>": kind}` and `windowsPct` `{"<position>": {key: pct}}` only
    when they hold something;
  - `switch`: `trigger`, `from`, `to`, `warnings`, `dryRun`;
  - `no-switch`: `reason`, `detail`;
  - `account-quarantined`, `account-unquarantined`: `number`, `email`, `reason`;
  - `all-exhausted`: `earliestResetAt`, ISO or null;
  - `sleep`: `seconds` (one decimal), `until` (ISO);
  - `error`: `message`, `transient`; `config-warning`: `message`.

  A command error (the refusal, `engine-running`, a usage error) is still §13.2's error object.
- **Human lines.** Each starts with the local time, `HH:MM:SS`, and two spaces.
  - A tick's line, on stdout: `#<position> <email>  <pct>%  <outcome>`. The percentage is the live
    account's highest relevant one (100 − headroom), or `—` when unknown, coloured by §13.5's
    severities under `list`'s rule (`App::color`).
  - Its outcome: `switched to #<n> (<trigger>)`; `would switch to #<n> (<trigger>)` in a dry run;
    `no switch: <reason>`, with ` (<detail>)` when there is one. `all-exhausted` is one line,
    `no switch: all-exhausted, the first back at HH:MM:SS (2h40m)`, printed at the
    `all-exhausted` event that follows its `no-switch`.
  - A tick that ended at §11.2 step 2 (no live account, an unmanaged one, an interrupted switch)
    has no `poll`, so its line is the outcome alone.
  - The sink keeps a tick's `poll` only until the tick ends (`tick_done`, Task 8). A tick that
    ended in an error printed no line and leaves its poll unused; a later tick that never polls
    must not show that tick's account and usage.
  - Lines of their own, on stdout: `#<n> <email> quarantined (<reason>)`,
    `#<n> <email> unquarantined (<reason>)`, `sleeping 10m until HH:MM:SS`.
  - On stderr: `error: <message>`, and `warning: <message>` for a config warning, a switch's
    warning, and `engine-running` ("auto-switch already runs for Claude Code in another process;
    skipping it").
  - With several providers, each line names its provider after the time (§13.1: with one, the
    output looks as it would without providers).
- **The drain helper.** `tests/common/mod.rs` gets `Running`. It spawns the binary with both
  pipes read on threads of their own, gives the stdout printed so far, waits for a condition on
  it, sends a signal, and finishes with the whole `Output`. Every `auto` binary test uses it, and
  so do M3a's two `--debug` tests in `tests/signals.rs`, which read their pipes only after exit.

**Rulings (what — why — cost if wrong):**
- **A tick's line gives one usage figure, the highest relevant percentage** — it is what the
  threshold is compared with, so the line reads against the decision — the line does not say
  which window binds; `--json`'s `windowsPct` does.
- **`engine-running` is a stderr warning in human output, for `--once` too** — no tick ran, so
  there is no tick line to print, and the exit code (2) carries the outcome — a script that
  reads `--once`'s stdout sees nothing for that provider.
- **A tick that ends in an error prints no stdout line**, only its `error` line on stderr (§11.4:
  "Errors … go to stderr") — a terminal shows that tick on stderr alone.
- **`windowsPct` is keyed by the provider's window keys**, as Task 8's `AutoEvent::Poll` holds
  them: Claude Code's `5h`, `7d`, `spend` and `scoped:<model>`, where cswap keyed a model window
  by its bare name — a cswap script that reads `windowsPct["Fable"]` finds nothing.
- **The flags are checked in `dispatch`, after the Keychain check** — `run_command` runs the
  check before dispatching any command that touches the Keychain — with a locked keychain on a
  terminal, a bad `--threshold` is reported after the unlock prompt.
- **Inside a run shell, the refusal comes before consume-first's usage error** — `App::auto`
  refuses before `run_loop` checks the strategy, keeping "the refusal comes first, whatever else
  would stop `auto`"; the bad flag values are checked earlier, in `dispatch` — inside a run shell,
  `--strategy consume-first` naming a provider without a long window exits 1
  (`inside-run-shell`), not 2; no shipped provider lacks a long window.

**Files:**
- Modify: `crates/tagteam/tests/common/mod.rs` (`Running`, `drain`)
- Modify: `crates/tagteam/tests/signals.rs` (the two `--debug` tests)
- Modify: `crates/tagteam/src/cli.rs` (`Command::Auto`, `AutoStrategyArg`, `touches_keychain`,
  `touches_keychain_only_with_a_store`)
- Modify: `crates/tagteam/src/app.rs` (`run`, `run_command`, `command_name`, `dispatch`,
  `App::auto`, `auto_flags`, `display_name`, `auto_failure`, and `mod tests`)
- Modify: `crates/tagteam/src/auto.rs` (`JsonSink`, `event_json`, `HumanSink`, `local_time`, and
  `mod tests`)
- Create: `crates/tagteam/tests/auto_cli.rs`
- Test: `crates/tagteam/tests/app.rs`

**Interfaces:**
- Consumes: Task 9's `run_loop`, `providers`, `AutoRun`, `AutoFlags`, `AutoError` (with
  `NoLongWindow`), `ThreadSleeper` and `uniform_jitter`; Task 8's `AutoEvent` and `EventSink`
  (`emit`, `tick_done`); `tagteam_fake::FakeAgent` in `app.rs`'s tests; Task 1's
  `parse_bool`; existing `model_list`, `App::color`, `render::{severity, duration, MISSING,
  RESET}`, `tagteam_cc::usage::format_iso8601`, `EngineError::InsideRunShell`, and the test
  helpers `std_cmd`, `cmd`, `two_fresh_accounts`, `record_reading`, `usage_window`,
  `expire_vault`, `live_email`, `now_epoch_s`, `LOCKED`, `H` and `Scripted`.
- Produces:
  - `tagteam::cli::Command::Auto { once, dry_run, threshold, interval, cooldown, strategy, model,
    include_api_key_accounts }` as the contract states it, and `pub enum AutoStrategyArg { Best,
    ConsumeFirst }` (`best | consume-first`)
  - `tagteam::auto::JsonSink::new(out: &'a mut dyn Write, now_ms: &'a dyn Fn() -> i64)` and
    `tagteam::auto::HumanSink::new(out: &'a mut dyn Write, err: &'a mut dyn Write, now_ms: &'a
    dyn Fn() -> i64, names: &'a dyn Fn(&ProviderId) -> String, color: bool, several: bool)`, both
    `EventSink`s; `HumanSink`'s `tick_done` drops the tick's poll
  - `tagteam::auto::event_json(e: &AutoEvent, now_s: i64) -> serde_json::Value` and
    `tagteam::auto::local_time(epoch_s: i64) -> String`
  - the `error.type`s `engine-running` and `no-candidates`, and `NoLongWindow` as the `usage`
    error (exit 2), through `app::auto_failure` (private)
  - the test helper `common::Running { pub child, .. }`: `spawn(cmd)`, `stdout()`,
    `wait_for(within, what, ready)`, `signal(signal)`, `finish(within) -> Output`

- [ ] **Step 1: The drain helper (M3a carry-over)**

In `crates/tagteam/tests/common/mod.rs`, replace

```rust
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
```

with

```rust
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
```

At the end of `crates/tagteam/tests/common/mod.rs`, add:

```rust

/// A running binary whose stdout and stderr are read on threads of their own while it runs, so
/// one that prints without end (an `auto` loop) never blocks on a full pipe, and what it has
/// printed so far can be read at any time.
pub struct Running {
    pub child: Child,
    out: Arc<Mutex<Vec<u8>>>,
    err: Arc<Mutex<Vec<u8>>>,
    readers: Vec<JoinHandle<()>>,
}

impl Running {
    /// Spawns `cmd` with its stdout and stderr piped, each drained on a thread of its own.
    pub fn spawn(mut cmd: std::process::Command) -> Self {
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (out, err) = (Arc::default(), Arc::default());
        let readers = vec![
            drain(child.stdout.take().unwrap(), Arc::clone(&out)),
            drain(child.stderr.take().unwrap(), Arc::clone(&err)),
        ];
        Running {
            child,
            out,
            err,
            readers,
        }
    }

    /// Everything it has printed on stdout so far.
    pub fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.out.lock().unwrap()).into_owned()
    }

    /// Polls every 10 ms until `ready` holds for its stdout so far. Fails the test if it exits
    /// first, or if `within` passes (killing it, so a hung one is not left behind).
    pub fn wait_for(&mut self, within: Duration, what: &str, ready: impl Fn(&str) -> bool) {
        let deadline = Instant::now() + within;
        while !ready(&self.stdout()) {
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!("tagteam exited ({status}) before {what}");
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                panic!("tagteam never got to {what}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Sends `signal`, as a terminal's Ctrl-C (SIGINT) or `kill` would.
    pub fn signal(&self, signal: i32) {
        // SAFETY: kill(2) reads no memory of ours. The child is one this test spawned and has
        // not reaped, so its pid names it and no other process.
        let rc = unsafe { libc::kill(self.child.id() as libc::pid_t, signal) };
        assert_eq!(rc, 0, "kill: {}", std::io::Error::last_os_error());
    }

    /// Its status and everything it printed, once it has exited. Fails the test, killing it,
    /// if it is still running after `within`.
    pub fn finish(mut self, within: Duration) -> Output {
        let deadline = Instant::now() + within;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                panic!("tagteam was still running {within:?} later");
            }
            thread::sleep(Duration::from_millis(10));
        };
        for reader in self.readers.drain(..) {
            reader.join().unwrap();
        }
        Output {
            status,
            stdout: std::mem::take(&mut *self.out.lock().unwrap()),
            stderr: std::mem::take(&mut *self.err.lock().unwrap()),
        }
    }
}

/// Reads `from` until its end into `into`, on a thread of its own.
fn drain(mut from: impl Read + Send + 'static, into: Arc<Mutex<Vec<u8>>>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = from.read(&mut buf) {
            if n == 0 {
                break;
            }
            into.lock().unwrap().extend_from_slice(&buf[..n]);
        }
    })
}
```

In `crates/tagteam/tests/signals.rs`, replace

```rust
use common::{live_email, std_cmd, two_fresh_accounts};
```

with

```rust
use common::{Running, live_email, std_cmd, two_fresh_accounts};
```

then, in `debug_logging_from_a_collector_thread_does_not_deadlock_the_command`, replace

```rust
    two_fresh_accounts(root); // both due: `list` collects with a thread per account

    let child = std_cmd(root)
        .args(["--debug", "list"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = finish(child, Duration::from_secs(10));
```

with

```rust
    two_fresh_accounts(root); // both due: `list` collects with a thread per account

    let mut cmd = std_cmd(root);
    cmd.args(["--debug", "list"]);
    let out = Running::spawn(cmd).finish(Duration::from_secs(10));
```

and in `ctrl_c_during_a_debug_collection_exits_130`, replace

```rust
    let mut child = std_cmd(root)
        .args(["--debug", "list"])
        .env("TAGTEAM_TEST_API_BASE", server.base_url())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_until(
        &mut child,
        Duration::from_secs(20),
        "a usage request in flight",
        || server.hits("GET", "/api/oauth/usage") > 0,
    );
    send(&child, libc::SIGINT);
    let out = finish(child, Duration::from_secs(8));
```

with

```rust
    let mut cmd = std_cmd(root);
    cmd.args(["--debug", "list"])
        .env("TAGTEAM_TEST_API_BASE", server.base_url());
    let mut running = Running::spawn(cmd);
    wait_until(
        &mut running.child,
        Duration::from_secs(20),
        "a usage request in flight",
        || server.hits("GET", "/api/oauth/usage") > 0,
    );
    send(&running.child, libc::SIGINT);
    let out = running.finish(Duration::from_secs(8));
```

`send`, `wait_until` and `finish` stay in `signals.rs` for its other tests.

- [ ] **Step 2: Run the moved tests, check, and commit**

Run: `cargo test -p tagteam --features test-support --test signals debug`
Expected: PASS, 2 tests.

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

```bash
git add crates/tagteam/tests/common/mod.rs crates/tagteam/tests/signals.rs
git commit -m "Drain a binary test's output on threads of its own while it runs"
```

- [ ] **Step 3: Write the failing tests**

The binary tests run the real binary on two accounts whose readings are fresh, with a plan in
force and every endpoint offline, as `strategy_cli.rs` does: b (position 2) is live. What they
pin:
- `once_exits_0_switched_1_error_2_no_action_and_3_blocked`: §11.4's codes and the tick line.
  The error case is §11.2 step 12's: the target's access token needs a refresh that cannot be
  sent.
- `json_prints_one_event_per_line_in_section_11_4_s_shape` and
  `dry_run_says_what_it_would_switch_and_writes_nothing` (§11.1: no engine lock file, no
  auto-switch state, no event row, the live login unchanged).
- `inside_a_run_shell_auto_refuses_unless_it_is_a_dry_run`: the refusal comes first, even where
  `auto` would otherwise find nothing to switch between.
- `the_loop_stops_cleanly_on_sigterm_and_sigint_after_a_tick`: exit 0, nothing on stderr, and no
  Claude Code lock directory left behind by the switch the first tick made.
- `a_second_loop_refuses_and_once_reports_engine_running_while_one_runs`: Review Focus 3, the
  CLI's half. The second loop exits 1; `--once` reports `engine-running` and exits 2; the first
  loop polled once, so nothing was ticked twice.
- `bad_flag_values_are_usage_errors`; out-of-range numbers are clamped, not refused.

Create `crates/tagteam/tests/auto_cli.rs`:

```rust
//! `tagteam auto` through the real binary (§11.1, §11.4, §14.1). Readings are recorded through
//! the store, fresh and with a plan in force, so a tick's scheduled collection sends nothing;
//! every endpoint stays offline (`std_cmd`'s default). Every run is drained while it runs
//! (`Running`), since a loop prints until it is stopped. Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::path::Path;
use std::process::Output;
use std::time::Duration;

use common::{
    Running, expire_vault, live_email, now_epoch_s, record_reading, std_cmd, two_fresh_accounts,
    usage_window,
};
use serde_json::{Value, json};
use tagteam_cc::CcPaths;
use tagteam_core::autoswitch::AutoState;
use tagteam_core::{CLAUDE_CODE, ProviderId, Window, WindowKind};
use tagteam_engine::store::Store;
use tagteam_provider::Env;

/// A reading taken at `now`: 5h at `five` and 7d at `seven`, resetting in 2h40m30s and
/// 3d09h00m30s.
fn reading(now: i64, five: f64, seven: f64) -> Vec<Window> {
    vec![
        usage_window(
            "5h",
            "5h",
            WindowKind::Short,
            five,
            Some(now + 9_630),
            Some(18_000),
        ),
        usage_window(
            "7d",
            "7d",
            WindowKind::Long,
            seven,
            Some(now + 291_630),
            Some(604_800),
        ),
    ]
}

/// `a@x.co` at position 1 read at (5h, 7d) `a`, and `b@x.co` at position 2, live, read at `b`.
fn accounts(root: &Path, a: (f64, f64), b: (f64, f64)) -> (String, String) {
    let (id_a, id_b) = two_fresh_accounts(root);
    let now = now_epoch_s();
    record_reading(root, &id_a, now, &reading(now, a.0, a.1));
    record_reading(root, &id_b, now, &reading(now, b.0, b.1));
    (id_a, id_b)
}

/// b at 95 % of its 7d window, a at 20 %: a tick switches to a (proactive).
fn switching(root: &Path) -> (String, String) {
    accounts(root, (10.0, 20.0), (10.0, 95.0))
}

/// b at 60 %: below the threshold, a tick stays.
fn staying(root: &Path) -> (String, String) {
    accounts(root, (10.0, 20.0), (10.0, 60.0))
}

fn auto_cmd(root: &Path, args: &[&str]) -> std::process::Command {
    let mut cmd = std_cmd(root);
    cmd.arg("auto").args(args);
    cmd
}

/// `tagteam auto <args>`, run to its end.
fn auto(root: &Path, args: &[&str]) -> Output {
    Running::spawn(auto_cmd(root, args)).finish(Duration::from_secs(20))
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Each stdout line after its `HH:MM:SS  ` time.
fn untimed(bytes: &[u8]) -> Vec<String> {
    text(bytes)
        .lines()
        .map(|l| {
            let (time, rest) = l.split_at(10);
            assert!(time.ends_with("  ") && time.as_bytes()[2] == b':', "{l}");
            rest.to_owned()
        })
        .collect()
}

/// Each stdout line as JSON, its `ts` checked as ISO 8601 UTC and then replaced by `[ts]`.
fn events(bytes: &[u8]) -> Vec<Value> {
    text(bytes)
        .lines()
        .map(|l| {
            let mut v: Value = serde_json::from_str(l).unwrap();
            let ts = v["ts"].as_str().unwrap();
            assert!(
                ts.len() == 20 && ts.ends_with('Z') && ts.as_bytes()[10] == b'T',
                "{ts}"
            );
            v["ts"] = json!("[ts]");
            v
        })
        .collect()
}

fn store(root: &Path) -> Store {
    Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap()
}

fn cc() -> ProviderId {
    ProviderId::new(CLAUDE_CODE)
}

#[test]
fn once_exits_0_switched_1_error_2_no_action_and_3_blocked() {
    // §11.4. One line per tick on stdout, errors on stderr.
    let d = tempfile::tempdir().unwrap();
    staying(d.path());
    let out = auto(d.path(), &["--once"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert_eq!(
        untimed(&out.stdout),
        ["#2 b@x.co  60%  no switch: below-threshold"]
    );
    assert_eq!(text(&out.stderr), "");

    let d = tempfile::tempdir().unwrap();
    switching(d.path());
    let out = auto(d.path(), &["--once"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        untimed(&out.stdout),
        ["#2 b@x.co  95%  switched to #1 (proactive)"]
    );
    assert_eq!(live_email(d.path()), "a@x.co");

    // Every candidate at its limit: blocked until the first one is back.
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), (100.0, 50.0), (10.0, 95.0));
    let out = auto(d.path(), &["--once"]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out.stderr));
    let line = &untimed(&out.stdout)[0];
    assert!(
        line.starts_with("#2 b@x.co  95%  no switch: all-exhausted, the first back at ")
            && line.ends_with(" (2h40m)"),
        "{line}"
    );

    // a's access token needs a refresh that cannot be sent: §11.2 step 12's error.
    let d = tempfile::tempdir().unwrap();
    let (a, _b) = switching(d.path());
    expire_vault(d.path(), &a, 60_000);
    let out = auto(d.path(), &["--once"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert_eq!(out.stdout, b"");
    let err = untimed(&out.stderr);
    assert!(
        err.len() == 1 && err[0].starts_with("error: could not freshen a@x.co (position 1): "),
        "{err:?}"
    );
    assert_eq!(live_email(d.path()), "b@x.co");
}

#[test]
fn json_prints_one_event_per_line_in_section_11_4_s_shape() {
    let d = tempfile::tempdir().unwrap();
    switching(d.path());
    let out = auto(d.path(), &["--once", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).starts_with(r#"{"schemaVersion":1,"event":"poll","ts":""#),
        "the envelope comes first"
    );
    assert_eq!(
        events(&out.stdout),
        [
            json!({"schemaVersion": 1, "event": "poll", "ts": "[ts]", "provider": "claude-code",
                   "active": {"number": 2, "email": "b@x.co"},
                   "headroomPct": {"1": 80.0, "2": 5.0}, "threshold": 90.0,
                   "windowsPct": {"1": {"5h": 10.0, "7d": 20.0}, "2": {"5h": 10.0, "7d": 95.0}}}),
            json!({"schemaVersion": 1, "event": "switch", "ts": "[ts]", "provider": "claude-code",
                   "trigger": "proactive", "from": 2, "to": 1, "warnings": [], "dryRun": false}),
        ]
    );
    assert_eq!(text(&out.stderr), "");
}

#[test]
fn dry_run_says_what_it_would_switch_and_writes_nothing() {
    // §11.1: no engine lock, no auto-switch state, no switch.
    let d = tempfile::tempdir().unwrap();
    switching(d.path());
    let before = store(d.path()).events().unwrap().len();
    let out = auto(d.path(), &["--once", "--dry-run", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        events(&out.stdout)[1],
        json!({"schemaVersion": 1, "event": "switch", "ts": "[ts]", "provider": "claude-code",
               "trigger": "proactive", "from": 2, "to": 1, "warnings": [], "dryRun": true})
    );
    assert_eq!(live_email(d.path()), "b@x.co");
    let s = store(d.path());
    assert_eq!(s.events().unwrap().len(), before);
    assert_eq!(s.autoswitch_state(&cc()).unwrap(), AutoState::default());
    let lock = Env::for_test(d.path())
        .data_dir()
        .join("locks/autoswitch-claude-code.lock");
    assert!(!lock.exists());
    let out = auto(d.path(), &["--once", "--dry-run"]);
    assert_eq!(
        untimed(&out.stdout),
        ["#2 b@x.co  95%  would switch to #1 (proactive)"]
    );
}

#[test]
fn inside_a_run_shell_auto_refuses_unless_it_is_a_dry_run() {
    // §11.1, like every command that changes the live login (§9.2).
    let d = tempfile::tempdir().unwrap();
    switching(d.path());
    let session = Env::for_test(d.path()).data_dir().join("sessions/x");
    let in_shell = |args: &[&str]| {
        let mut cmd = auto_cmd(d.path(), args);
        cmd.env("CLAUDE_CONFIG_DIR", &session);
        Running::spawn(cmd).finish(Duration::from_secs(20))
    };
    let out = in_shell(&["--once"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        text(&out.stderr),
        "tagteam: this command cannot run inside a `tagteam run` session\n"
    );
    let out = in_shell(&["--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "inside-run-shell",
               "message": "this command cannot run inside a `tagteam run` session"}})
    );
    // The refusal comes first, whatever else would stop `auto`.
    common::cmd(d.path())
        .args(["disable", "1"])
        .assert()
        .success();
    let out = in_shell(&["--once"]);
    assert_eq!(
        (out.status.code(), text(&out.stderr)),
        (
            Some(1),
            "tagteam: this command cannot run inside a `tagteam run` session\n".into()
        )
    );
    common::cmd(d.path())
        .args(["enable", "1"])
        .assert()
        .success();
    // A dry run is not refused: it runs a tick. What it reads there (the default home's login
    // or the session's own) is §12.8's (M4) to settle, so only that it ran is pinned.
    let out = in_shell(&["--once", "--dry-run", "--json"]);
    assert_ne!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert_ne!(
        serde_json::from_slice::<Value>(&out.stdout).ok(),
        Some(
            json!({"schemaVersion": 1, "error": {"type": "inside-run-shell",
               "message": "this command cannot run inside a `tagteam run` session"}})
        )
    );
    let outcomes: Vec<Value> = events(&out.stdout)
        .into_iter()
        .filter(|e| ["no-switch", "switch"].contains(&e["event"].as_str().unwrap()))
        .collect();
    assert_eq!(outcomes.len(), 1, "{}", text(&out.stdout));
    if outcomes[0]["event"] == "switch" {
        assert_eq!(outcomes[0]["dryRun"], json!(true));
    }
    assert_eq!(live_email(d.path()), "b@x.co");
}

#[test]
fn the_loop_stops_cleanly_on_sigterm_and_sigint_after_a_tick() {
    // §11.4: the loop exits 0, with no notice that the signal came too late, and every lock
    // its switch took is released: no Claude Code lock directory is left behind.
    for signal in [libc::SIGTERM, libc::SIGINT] {
        let d = tempfile::tempdir().unwrap();
        switching(d.path());
        let mut running = Running::spawn(auto_cmd(d.path(), &["--json"]));
        running.wait_for(Duration::from_secs(20), "the first switch", |out| {
            out.contains(r#""event":"switch""#)
        });
        running.signal(signal);
        let out = running.finish(Duration::from_secs(5));
        assert_eq!(
            out.status.code(),
            Some(0),
            "{signal}: {}",
            text(&out.stderr)
        );
        assert_eq!(text(&out.stderr), "", "{signal}");
        assert_eq!(live_email(d.path()), "a@x.co");
        let paths = CcPaths::resolve(&Env::for_test(d.path()));
        for lock in [
            paths.refresh_lock.clone(),
            paths.legacy_lock(),
            paths.config_lock.clone(),
            paths.storage_write_lock.clone(),
        ] {
            assert!(!lock.exists(), "{} was left behind", lock.display());
        }
    }
}

#[test]
fn a_second_loop_refuses_and_once_reports_engine_running_while_one_runs() {
    // Review Focus 3, the CLI's half: one engine per provider per machine (§11.1). Nothing is
    // ticked twice: the refused runs never poll.
    let d = tempfile::tempdir().unwrap();
    staying(d.path());
    let mut first = Running::spawn(auto_cmd(d.path(), &["--json"]));
    first.wait_for(Duration::from_secs(20), "its first tick", |out| {
        out.contains(r#""event":"no-switch""#)
    });

    let out = auto(d.path(), &[]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        text(&out.stderr),
        "tagteam: auto-switch already runs for Claude Code\n"
    );
    assert_eq!(out.stdout, b"");
    let out = auto(d.path(), &["--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "engine-running",
               "message": "auto-switch already runs for Claude Code"}})
    );

    let out = auto(d.path(), &["--once", "--json"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert_eq!(
        events(&out.stdout),
        [
            json!({"schemaVersion": 1, "event": "no-switch", "ts": "[ts]", "provider": "claude-code",
                   "reason": "engine-running", "detail": ""})
        ]
    );
    let out = auto(d.path(), &["--once"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(out.stdout, b"");
    assert_eq!(
        untimed(&out.stderr),
        ["warning: auto-switch already runs for Claude Code in another process; skipping it"]
    );

    first.signal(libc::SIGTERM);
    let out = first.finish(Duration::from_secs(5));
    assert_eq!(out.status.code(), Some(0));
    let polls = events(&out.stdout)
        .iter()
        .filter(|e| e["event"] == "poll")
        .count();
    assert_eq!(polls, 1);
}

#[test]
fn bad_flag_values_are_usage_errors() {
    let d = tempfile::tempdir().unwrap();
    staying(d.path());
    for (args, message) in [
        (
            &["--threshold", "nan"][..],
            "--threshold takes a number from 50 to 99.9",
        ),
        (
            &["--include-api-key-accounts", "maybe"][..],
            "--include-api-key-accounts takes true, false, 1, 0, yes or no",
        ),
    ] {
        let mut all = vec!["--once", "--json"];
        all.extend_from_slice(args);
        let out = auto(d.path(), &all);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap(),
            json!({"schemaVersion": 1, "error": {"type": "usage", "message": message}})
        );
    }
    // In range after clamping, and spelled as §6.4 spells booleans: accepted.
    let out = auto(
        d.path(),
        &[
            "--once",
            "--threshold",
            "120",
            "--interval",
            "1",
            "--include-api-key-accounts",
            "yes",
        ],
    );
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert_eq!(
        untimed(&out.stdout),
        ["#2 b@x.co  60%  no switch: below-threshold"]
    );
}
```

In `crates/tagteam/tests/app.rs`, insert immediately before `#[test]` `fn with_no_store_switch_and_remove_run_no_lock_check`:

```rust
#[test]
fn auto_checks_the_keychain_before_its_first_tick_a_dry_run_too() {
    // §11.1 and Appendix A.3. With no store there is nothing to switch between, so no check
    // runs and nothing prompts (Scripted panics on a prompt it has no answer for).
    let h = H::new();
    h.kc.set_locked(true);
    let (code, out, err) = h.run(&["auto", "--once"], &mut Scripted::answering(&[]));
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (
            1,
            "",
            "tagteam: auto-switch needs two switchable accounts on a provider; add another with `tagteam add`\n"
        )
    );
    h.kc.set_locked(false);
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    h.login("b@x.co", "rt-b");
    h.ok(&["add"]);
    h.kc.set_locked(true);
    for args in [&["auto", "--once"][..], &["auto", "--once", "--dry-run"]] {
        let (code, out, err) = h.run(args, &mut Scripted::none());
        assert_eq!(
            (code, out.as_str(), err),
            (1, "", format!("tagteam: {LOCKED}\n")),
            "{args:?}"
        );
    }
    assert_eq!(h.kc.unlock_attempts(), 0);
}

```

In `crates/tagteam/src/app.rs`, inside `mod tests`, in `the_late_notice_names_each_command_as_it_is_typed`, replace

```rust
        let cases: [&[&str]; 14] = [
```

with

```rust
        let cases: [&[&str]; 15] = [
```

and replace

```rust
            &["statusline"],
        ];
```

with

```rust
            &["statusline"],
            &["auto"],
        ];
```

and, at the end of `mod tests`, replace

```rust
    #[test]
    fn each_strategy_flag_names_its_engine_strategy() {
        assert_eq!(usage_strategy(StrategyArg::Best), UsageStrategy::Best);
        assert_eq!(
            usage_strategy(StrategyArg::NextAvailable),
            UsageStrategy::NextAvailable
        );
    }
}
```

with

```rust
    #[test]
    fn each_strategy_flag_names_its_engine_strategy() {
        assert_eq!(usage_strategy(StrategyArg::Best), UsageStrategy::Best);
        assert_eq!(
            usage_strategy(StrategyArg::NextAvailable),
            UsageStrategy::NextAvailable
        );
    }

    #[test]
    fn consume_first_named_for_a_provider_without_a_long_window_is_a_usage_error() {
        // §4.5. FakeAgent names no long window; `run_loop` refuses the combination before any
        // engine starts, and the command exits 2 in the usage error's shape.
        use tagteam_fake::FakeAgent;
        use tagteam_provider::Provider;
        let fake = FakeAgent::new();
        assert_eq!(fake.primary_long_window(), None);
        let name = |_: &ProviderId| fake.display_name().to_owned();
        let Failure::Usage(message) = auto_failure(AutoError::NoLongWindow(fake.id()), &name)
        else {
            panic!("not a usage error");
        };
        assert_eq!(
            message,
            "--strategy consume-first needs a long usage window to rank by, and FakeAgent has none"
        );
    }
}
```

`consume_first_named_for_a_provider_without_a_long_window_is_a_usage_error` pins §4.5's usage
error against FakeAgent, the provider without a long window: `AutoError::NoLongWindow` becomes
`Failure::Usage`, which `run_command` prints as the `usage` error object and exits 2 with, as it
does for every bad flag value. Task 9's tests pin the refusal itself, and its two fallbacks.

The sinks' unit tests render every event kind, pin the human lines on a fixed clock (`t+0`
stands for the local time), and check the colours and the provider names.
`a_tick_that_ends_in_an_error_leaves_its_poll_to_no_later_line` drives the real loop on Claude
Code: a tick polls b and ends in an error, the live login goes, and the next tick's line names
no account:

In `crates/tagteam/src/auto.rs`, at the end of `mod tests`, replace

```rust
    #[test]
    fn the_jitter_draw_stays_within_its_range() {
        for _ in 0..1_000 {
            let j = uniform_jitter();
            assert!((-1.0..1.0).contains(&j), "{j}");
        }
    }
}
```

with

```rust
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
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p tagteam --features test-support --lib auto::`
Expected: FAIL to compile, ``could not compile `tagteam` (lib test) due to 7 previous errors``:
``cannot find function `event_json` in this scope``, ``cannot find type `HumanSink` in this
scope`` and ``cannot find function `local_time` in this scope``, and in `app.rs`'s new test
``cannot find function `auto_failure` in this scope`` and ``use of undeclared type
`AutoError` ``.

Run: `cargo test -p tagteam --features test-support --test auto_cli`
Expected: FAIL, 0 passed and 7 failed: the binary has no `auto` command, so each run exits 2
(for example `left: Some(2)`, `right: Some(0)`, or `tagteam exited (exit status: 2) before the
first switch`).

Run: `cargo test -p tagteam --features test-support --test app auto_checks`
Expected: FAIL: `Cli::try_parse_from` panics with `InvalidSubcommand` for `auto`.

- [ ] **Step 5: Implement**

In `crates/tagteam/src/cli.rs`, insert immediately before the doc line `/// Usage history: burn rate, and when each window runs out` of `Command::History`:

```rust
    /// Switch automatically before a rate limit, until stopped (Ctrl-C)
    ///
    /// Runs one engine per provider with two switchable accounts (or only --provider's), each
    /// on its own schedule, and prints a line per tick. --json prints one event per line:
    /// {"schemaVersion":1,"event":<kind>,"ts":"…Z","provider":…, …}. --once ticks once and
    /// exits 0 switched, 1 error, 2 no action, 3 blocked.
    Auto {
        /// Tick once per provider and exit with its outcome
        #[arg(long)]
        once: bool,
        /// Decide and report, but switch nothing and write no auto-switch state
        #[arg(long = "dry-run")]
        dry_run: bool,
        /// Switch away above this percentage of usage (50–99.9)
        #[arg(long)]
        threshold: Option<f64>,
        /// Seconds between ticks (15–3600)
        #[arg(long)]
        interval: Option<i64>,
        /// Seconds after an automatic switch before another proactive one (0–86400)
        #[arg(long)]
        cooldown: Option<i64>,
        /// best or consume-first
        #[arg(long, value_enum)]
        strategy: Option<AutoStrategyArg>,
        /// Model limits that count, comma-separated, or `all`
        #[arg(long)]
        model: Option<String>,
        /// Fall back to API-key accounts at the limit: true or false
        #[arg(long = "include-api-key-accounts", value_name = "BOOL")]
        include_api_key_accounts: Option<String>,
    },
```

In `crates/tagteam/src/cli.rs`, insert immediately before `impl Command {`:

```rust
/// `auto --strategy` (§6.4's `autoswitch.strategy`).
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum AutoStrategyArg {
    /// The candidate with the most headroom, past the hysteresis
    Best,
    /// The candidate whose weekly window resets soonest, while below the threshold
    ConsumeFirst,
}

```

and replace the end of `touches_keychain`'s doc comment, through the end of the file,

```rust
    /// rest need no Keychain item. A recovery under their mutation lock (Task 21) only reads the
    /// Keychain, tri-state, and leaves what it cannot decide to the next command that checks.
    pub fn touches_keychain(&self) -> bool {
        matches!(
            self,
            Command::Add { .. }
                | Command::AddToken { .. }
                | Command::Switch { .. }
                | Command::Remove { .. }
        )
    }

    /// Of those, the ones that reach a Keychain item only through a stored account, so with
    /// no store they touch none (§5): there is nothing to activate or delete.
    pub fn touches_keychain_only_with_a_store(&self) -> bool {
        matches!(self, Command::Switch { .. } | Command::Remove { .. })
    }
}
```

with

```rust
    /// rest need no Keychain item. A recovery under their mutation lock (Task 21) only reads the
    /// Keychain, tri-state, and leaves what it cannot decide to the next command that checks.
    /// `auto` checks before its first tick, a dry run too (§11.1): every tick reads its
    /// accounts' items, and a real one switches.
    pub fn touches_keychain(&self) -> bool {
        matches!(
            self,
            Command::Add { .. }
                | Command::AddToken { .. }
                | Command::Switch { .. }
                | Command::Remove { .. }
                | Command::Auto { .. }
        )
    }

    /// Of those, the ones that reach a Keychain item only through a stored account, so with
    /// no store they touch none (§5): there is nothing to activate, delete or switch between.
    pub fn touches_keychain_only_with_a_store(&self) -> bool {
        matches!(
            self,
            Command::Switch { .. } | Command::Remove { .. } | Command::Auto { .. }
        )
    }
}
```

In `crates/tagteam/src/auto.rs`, replace the imports

```rust
use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::time::Duration;

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
```

with

```rust
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Write;
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

use crate::render::{MISSING, RESET, duration, severity};
```

In `crates/tagteam/src/auto.rs`, insert immediately before the `#[cfg(test)]` line that opens `mod tests`:

```rust
/// `--json` (§11.4): each event as one object on a line of its own.
pub struct JsonSink<'a> {
    out: RefCell<&'a mut dyn Write>,
    /// The engine's wall clock, epoch milliseconds: each event's `ts`.
    now_ms: &'a dyn Fn() -> i64,
}

impl<'a> JsonSink<'a> {
    pub fn new(out: &'a mut dyn Write, now_ms: &'a dyn Fn() -> i64) -> Self {
        JsonSink {
            out: RefCell::new(out),
            now_ms,
        }
    }
}

impl EventSink for JsonSink<'_> {
    fn emit(&self, e: &AutoEvent) {
        let line = event_json(e, (self.now_ms)().div_euclid(1000));
        let mut out = self.out.borrow_mut();
        let _ = writeln!(out, "{line}");
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
        }
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
        let _ = writeln!(out, "{}{text}", self.head(now, provider));
    }

    fn err_line(&self, now: i64, provider: &ProviderId, label: &str, text: &str) {
        let mut err = self.err.borrow_mut();
        let _ = writeln!(err, "{}{label}: {text}", self.head(now, provider));
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

```

In `crates/tagteam/src/app.rs`, replace the imports

```rust
use tagteam_core::{AccountId, CLAUDE_CODE, Pace, ProviderId, Window};
use tagteam_engine::collect::CollectMode;
use tagteam_engine::lazy_http::LazyHttp;
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::net::UreqHttp;
use tagteam_engine::oracle::{CachingOracle, HttpOracle};
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::{ColorMode, Settings};
use tagteam_engine::store::AccountRow;
use tagteam_engine::switch::{SwitchReason, SwitchRequest, SwitchTarget, UsageStrategy};
use tagteam_engine::vault::{FileVault, KeychainVault, Vault};
use tagteam_engine::views::{AccountView, StatusView};
use tagteam_engine::{Engine, EngineConfig, EngineError};
use tagteam_provider::http::Http;
use tagteam_provider::security::SecurityCli;
use tagteam_provider::{Clock, Env, Keychain, LockState, SystemClock};

use crate::cli::{Cli, Command, StrategyArg};
use crate::prompt::Prompter;
use crate::{history, prompt, render, root_guard, statusline};
```

with

```rust
use tagteam_core::autoswitch::Strategy;
use tagteam_core::{AccountId, CLAUDE_CODE, Pace, ProviderId, Window};
use tagteam_engine::collect::CollectMode;
use tagteam_engine::lazy_http::LazyHttp;
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::net::UreqHttp;
use tagteam_engine::oracle::{CachingOracle, HttpOracle};
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::{ColorMode, Settings, parse_bool};
use tagteam_engine::store::AccountRow;
use tagteam_engine::switch::{SwitchReason, SwitchRequest, SwitchTarget, UsageStrategy};
use tagteam_engine::vault::{FileVault, KeychainVault, Vault};
use tagteam_engine::views::{AccountView, StatusView};
use tagteam_engine::{Engine, EngineConfig, EngineError};
use tagteam_provider::http::Http;
use tagteam_provider::security::SecurityCli;
use tagteam_provider::{Clock, Env, Keychain, LockState, SystemClock};

use crate::auto::{
    AutoError, AutoFlags, AutoRun, HumanSink, JsonSink, ThreadSleeper, uniform_jitter,
};
use crate::cli::{AutoStrategyArg, Cli, Command, StrategyArg};
use crate::prompt::Prompter;
use crate::{auto, history, prompt, render, root_guard, statusline};
```

then replace

```rust
/// A provider without the capability a command needs (§4.5).
const KIND_UNSUPPORTED: &str = "unsupported";
```

with

```rust
/// A provider without the capability a command needs (§4.5).
const KIND_UNSUPPORTED: &str = "unsupported";
/// §11.1: every provider `auto` would drive has its engine running in another process.
const KIND_ENGINE_RUNNING: &str = "engine-running";
/// `auto` found no provider with two switchable accounts.
const KIND_NO_CANDIDATES: &str = "no-candidates";
```

then replace

```rust
const STATUSLINE_UNDER_JSON: &str = "statusline prints a line of text; run it without --json";
```

with

```rust
const STATUSLINE_UNDER_JSON: &str = "statusline prints a line of text; run it without --json";
const NOTHING_TO_SWITCH: &str =
    "auto-switch needs two switchable accounts on a provider; add another with `tagteam add`";
const BAD_THRESHOLD: &str = "--threshold takes a number from 50 to 99.9";
const BAD_INCLUDE: &str = "--include-api-key-accounts takes true, false, 1, 0, yes or no";
```

Insert immediately before `is_set`'s doc comment, the line that starts `/// Whether a colour variable`:

```rust
/// `auto`'s flags as the loop takes them (§11.4); each is clamped there (§6.4). clap reads `nan`
/// and `inf` as numbers, so `--threshold` must be finite. `--include-api-key-accounts` takes
/// §6.4's booleans.
fn auto_flags(
    threshold: Option<f64>,
    interval: Option<i64>,
    cooldown: Option<i64>,
    strategy: Option<AutoStrategyArg>,
    model: Option<String>,
    include_api_key_accounts: Option<String>,
) -> Result<AutoFlags, Failure> {
    if threshold.is_some_and(|t| !t.is_finite()) {
        return Err(Failure::Usage(BAD_THRESHOLD.into()));
    }
    let include_api_key_accounts = match include_api_key_accounts.as_deref() {
        Some(v) => Some(parse_bool(v).ok_or_else(|| Failure::Usage(BAD_INCLUDE.into()))?),
        None => None,
    };
    Ok(AutoFlags {
        threshold,
        interval_s: interval,
        cooldown_s: cooldown,
        strategy: strategy.map(|s| match s {
            AutoStrategyArg::Best => Strategy::Best,
            AutoStrategyArg::ConsumeFirst => Strategy::ConsumeFirst,
        }),
        models: model.as_deref().map(model_list),
        include_api_key_accounts,
    })
}

/// A provider's display name, or its ID for one this build does not register.
fn display_name(engine: &Engine, provider: &ProviderId) -> String {
    engine
        .provider(provider)
        .map_or_else(|_| provider.to_string(), |p| p.display_name().to_owned())
}

/// How `auto` fails as a command, each provider named by `name`. Consume-first named for a
/// provider without a long window is a usage error, exit 2, as every bad flag value is (§4.5).
fn auto_failure(e: AutoError, name: &dyn Fn(&ProviderId) -> String) -> Failure {
    match e {
        AutoError::AlreadyRuns(providers) => {
            let names: Vec<String> = providers.iter().map(name).collect();
            Failure::Message(
                KIND_ENGINE_RUNNING,
                format!("auto-switch already runs for {}", names.join(", ")),
            )
        }
        AutoError::NothingToSwitch => {
            Failure::Message(KIND_NO_CANDIDATES, NOTHING_TO_SWITCH.into())
        }
        AutoError::NoLongWindow(p) => Failure::Usage(format!(
            "--strategy consume-first needs a long usage window to rank by, and {} has none",
            name(&p)
        )),
        AutoError::Engine(e) => e.into(),
    }
}

```

In `run`, replace

```rust
    let name = cli.command.as_ref().map_or("list", command_name);
    let cancel = ctx.env.cancel.clone();
    match run_command(cli, ctx, io) {
        Ended::Interrupted(signal) => {
            fail(io, json, KIND_INTERRUPTED, INTERRUPTED);
            EXIT_SIGNAL_BASE + signal
        }
        Ended::Code(code) => {
            if cancel.requested().is_some() {
```

with

```rust
    let name = cli.command.as_ref().map_or("list", command_name);
    // §11.4: a signal is how the `auto` loop ends, so it is never too late for it.
    let stops_on_a_signal = matches!(cli.command, Some(Command::Auto { once: false, .. }));
    let cancel = ctx.env.cancel.clone();
    match run_command(cli, ctx, io) {
        Ended::Interrupted(signal) => {
            fail(io, json, KIND_INTERRUPTED, INTERRUPTED);
            EXIT_SIGNAL_BASE + signal
        }
        Ended::Code(code) => {
            if cancel.requested().is_some() && !stops_on_a_signal {
```

In `run_command`, replace

```rust
        Ok(()) => Ended::Code(0),
```

with

```rust
        Ok(code) => Ended::Code(code),
```

In `command_name`, replace

```rust
        Command::Statusline { .. } => "statusline",
    }
}
```

with

```rust
        Command::Statusline { .. } => "statusline",
        Command::Auto { .. } => "auto",
    }
}
```

Replace `dispatch`'s signature

```rust
    fn dispatch(&mut self, command: Command) -> Result<(), Failure> {
```

with

```rust
    /// Runs `command` and returns its exit code: 0, except for `auto` (§11.4).
    fn dispatch(&mut self, command: Command) -> Result<i32, Failure> {
```

and its end

```rust
            Command::Statusline { .. } => unreachable!("run answers statusline before dispatch"),
        }
        Ok(())
    }
```

with

```rust
            Command::Statusline { .. } => unreachable!("run answers statusline before dispatch"),
            Command::Auto {
                once,
                dry_run,
                threshold,
                interval,
                cooldown,
                strategy,
                model,
                include_api_key_accounts,
            } => {
                let flags = auto_flags(
                    threshold,
                    interval,
                    cooldown,
                    strategy,
                    model,
                    include_api_key_accounts,
                )?;
                return self.auto(AutoRun {
                    provider: self.provider_flag.clone(),
                    flags,
                    once,
                    dry_run,
                });
            }
        }
        Ok(0)
    }

    /// §11: `auto`. Like every command that changes the live login it refuses inside a run
    /// shell, except with `--dry-run` (§11.1); `run_command` has run the Keychain check. Events
    /// go to stdout as human lines or JSONL (§11.4), warnings and errors to stderr.
    fn auto(&mut self, run: AutoRun) -> Result<i32, Failure> {
        if !run.dry_run && self.engine.env().inside_run_shell() {
            return Err(EngineError::InsideRunShell.into());
        }
        let several = auto::providers(&self.engine, run.provider.as_ref())?.len() > 1;
        let color = self.color();
        let engine = &self.engine;
        let now_ms = || engine.now_ms();
        let names = |p: &ProviderId| display_name(engine, p);
        let Io { out, err, .. } = &mut *self.io;
        let ended = if self.json {
            let sink = JsonSink::new(&mut **out, &now_ms);
            auto::run_loop(engine, &run, &sink, &ThreadSleeper, &mut uniform_jitter)
        } else {
            let sink = HumanSink::new(&mut **out, &mut **err, &now_ms, &names, color, several);
            auto::run_loop(engine, &run, &sink, &ThreadSleeper, &mut uniform_jitter)
        };
        ended.map_err(|e| auto_failure(e, &names))
    }
```

Every other arm of `dispatch` falls through to `Ok(0)` as it fell through to `Ok(())`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p tagteam --features test-support --lib auto::`
Expected: PASS, 23 tests (Task 9's 18 and the sinks' 5).

Run: `cargo test -p tagteam --features test-support --test auto_cli`
Expected: PASS, 7 tests.

Run: `cargo test -p tagteam --features test-support --test app`
Expected: PASS, 39 tests.

Run: `cargo test -p tagteam --features test-support --lib app::`
Expected: PASS, 8 tests.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. Inside Claude Code's sandbox, the 10 tests that need a pseudo-terminal fail with
`openpty … Operation not permitted`: the tagteam lib's 9 `prompt::tests::*` and
`signals::ctrl_c_while_add_token_reads_a_terminal_stdin_exits_130_without_waiting_for_enter`. Run
`cargo test -p tagteam --features test-support --lib prompt::` and
`cargo test -p tagteam --features test-support --test signals` outside it: PASS.

- [ ] **Step 7: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/tagteam/src/cli.rs crates/tagteam/src/app.rs crates/tagteam/src/auto.rs \
  crates/tagteam/tests/app.rs crates/tagteam/tests/auto_cli.rs
git commit -m "Add tagteam auto: the loop's command, its JSONL and human lines, and its exit codes"
```

---

### Task 11: Final verification and live acceptance

**Human steps (Step 3 prepares; Steps 4–5 are the acceptance and Michael's).** Steps 1–2 are
the implementer's. The live run uses Michael's real Claude Code accounts, logged in afresh in a
throwaway Claude Code config directory with a throwaway tagteam store (M3a's isolated method:
`CLAUDE_CONFIG_DIR` and `XDG_*` point into one directory). Claude Code names its Keychain item
after its config directory (Appendix A.2), so the throwaway login has an item of its own, and
every Claude Code session already running keeps its login, its config and its lock directories
untouched. The run still sends real usage and token requests and switches real logins in the
throwaway directory, so Michael runs it, and the merge waits for it. An agent never runs Steps
3–5.

The whole-branch review and the pre-merge cross-review run after Step 2. They follow the global
workflow and are not steps of this plan. The live run is the last step before the pull request.

**Files:**
- Modify: this plan's own file, `docs/superpowers/plans/2026-10-02-tagteam-m3b-auto-switch.md`
  (the `**Status:**` line, Step 7)

**Interfaces:**
- Consumes: everything Tasks 1–10 produced. In particular:
  - `tagteam auto [--once] [--dry-run] [--json] [--threshold N] [--interval S] [--cooldown S]
    [--strategy best|consume-first] [--model M] [--include-api-key-accounts BOOL]`;
  - `--once`'s exit codes, `0` switched, `1` error, `2` no action, `3` blocked; the loop's 0 on
    SIGINT, SIGTERM or SIGHUP;
  - the human tick line `HH:MM:SS  #<position> <email>  <pct>%  <outcome>` and the JSONL
    envelope `{"schemaVersion":1,"event":<kind>,"ts":"…Z","provider":…}` (Task 10);
  - the errors `engine-running` ("auto-switch already runs for Claude Code") and
    `inside-run-shell`;
  - the engine lock file `$XDG_DATA_HOME/tagteam/locks/autoswitch-claude-code.lock` (Task 8),
    which a dry run never creates.
- Produces: nothing new.

- [ ] **Step 1: Format, lint and test everything**

Run:
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --features tagteam/test-support
cargo test --workspace --features tagteam/test-support -- --ignored
cargo test -p tagteam --lib
cargo test --release -p tagteam --features test-support --test perf -- --ignored
```

Expected:
- No diff, and no warnings with the test features or without them.
- Every test passes. Record the totals as passed/total for the pull request. Claude Code's
  sandbox blocks `/dev/ptmx`, so inside it the 10 tests that need a pseudo-terminal fail with
  `openpty … Operation not permitted`: the tagteam lib's 9 `prompt::tests::*` and
  `signals::ctrl_c_while_add_token_reads_a_terminal_stdin_exits_130_without_waiting_for_enter`.
  Run the suite, or at least
  `cargo test -p tagteam --features test-support --lib prompt::` and
  `cargo test -p tagteam --features test-support --test signals`, outside the sandbox, where they
  pass. At the plan's last commit, review round 1's fixes included, the suite was 1465/1475
  inside the sandbox (3 ignored). Those 10 passed outside it before those fixes, which touch
  neither them nor the code they test.
- The `--ignored` run passes its 3 slow tests: `collect_active`'s 10 s mutation-lock timeout,
  `switch`'s 9 s Claude Code lock and `gate_race`'s 15 s stopped holder.
- `cargo test -p tagteam --lib` runs without features, so it exercises the release branch of the
  test overrides; it compiles out `auto`'s one test that needs the engine's hooks.
- The perf run passes all three timing tests: `statusline` p95 ≤ 10 ms (also on a cache miss) and
  `list` p95 ≤ 50 ms. M3b adds nothing on their paths, so a miss here is a regression elsewhere.
  Run it on an otherwise idle machine. One failure gets one re-run; a second failure is a real
  miss of the budget, so stop and report the measured p95.

On a Mac, also run:
`cargo test -p tagteam-provider --features real_keychain --test real_keychain`

Expected: PASS, with no GUI prompt (CI runs it too).

Then run the Linux suite in Docker, as M3a's final review did: non-root, as a user with a passwd
entry (a naming test compares with `id -un`), with HOME outside `/tmp` (the harness guard trips
when a temporary directory falls under HOME), the worktree mounted read-only, and named volumes
for the target and the registry:

```
docker run --rm -v /Users/michael/Code/tagteam-m3-auto-switch:/src:ro -v tagteam-target:/target -v tagteam-cargo-registry:/usr/local/cargo/registry -e CARGO_TARGET_DIR=/target -w /src rust:1.88 bash -c 'useradd -m -u 1000 tester && chown -R tester /target /usr/local/cargo && setpriv --reuid=1000 --regid=1000 --clear-groups env HOME=/home/tester USER=tester cargo test --workspace --locked --no-fail-fast --features tagteam/test-support'
```

Run it as a Bash call of its own, with `docker` as its first word and no pipe or redirect, in the
background: only then does Claude Code run it outside its sandbox, which `docker` needs.

Expected: every test passes, the pseudo-terminal ones included. CI's `ubuntu-latest` job runs the
same suite on the pull request.

- [ ] **Step 2: Build the release binary and check it carries no test hooks**

Run:
```bash
cargo build --release -p tagteam
grep -a -c TAGTEAM_TEST_ target/release/tagteam ; test $? -eq 1
```

Expected:
- The build succeeds.
- `grep` prints `0` and exits 1, so the final `test` exits 0. The crash points and every test
  override exist only under `test-support`; M3b's loop reads the system clock and sleeps the
  thread, with no test seam in the release build.

- [ ] **Step 3: Michael — prepare the acceptance run**

The goal of the run:
- `auto` decides and switches as §11 says on real usage: a dry run, a lowered threshold,
  consume-first.
- The loop prints a line per tick at its interval, emits well-formed JSONL, and stops cleanly on
  Ctrl-C and SIGTERM, leaving the terminal intact and no Claude Code lock directory behind.
- A manual switch while the loop runs is never undone, and a second `auto` refuses.

No token is handled by hand, and no Claude Code session needs to be quit.

1. From the worktree root, put the Step 2 binary first on `PATH` for this shell:
   ```bash
   export PATH="$PWD/target/release:$PATH"
   command -v tagteam
   ```
   Expected: `<worktree>/target/release/tagteam`. The binary is new and ad-hoc signed, so if a
   request is blocked, suspect Little Snitch and allow `api.anthropic.com` and
   `platform.claude.com` for it.
2. Point this shell, and only this shell, at a throwaway Claude Code config dir and a throwaway
   tagteam store. Export each once and keep the shell open until Step 5: Claude Code hashes the
   exact `CLAUDE_CONFIG_DIR` string into its Keychain item's name.
   ```bash
   export TT="$HOME/.cache/tagteam-m3b-acceptance"
   mkdir -p "$TT/claude"
   export CLAUDE_CONFIG_DIR="$TT/claude" XDG_CONFIG_HOME="$TT/config" XDG_DATA_HOME="$TT/data" XDG_STATE_HOME="$TT/state"
   unset CLAUDE_SECURESTORAGE_CONFIG_DIR
   tagteam list
   ```
   Expected: `No accounts yet. Log in with `claude`, then run `tagteam add`.` — a store of its own.
3. Log in to at least two of your accounts there, three if you have them. For each one: run
   `claude`, finish its first-run questions, log in with `/login` as that account, leave with
   `/exit`, then run `tagteam add`. Each login is a new grant of its own; the logins of running
   sessions are untouched. Then run `tagteam list`. Note the live account's position as **P0**.
4. Define these helpers. They print positions, ids, statuses, numbers and event fields, never an
   email or a token:
   ```bash
   # The throwaway config dir's four Claude Code lock directories (§9.1, Appendix A.1).
   tt_locks() {
     local d="$CLAUDE_CONFIG_DIR"
     local n=$(ls -d "$d/.oauth_refresh.lock" "$(realpath "$d").lock" "$d/.claude.json.lock" "$d/.storage-write" 2>/dev/null | wc -l | tr -d ' ')
     echo "locks left: $n"
   }
   # Whether the auto-switch engine lock file exists (§11.1): a dry run never creates it.
   tt_engine_lock() {
     if [ -e "$XDG_DATA_HOME/tagteam/locks/autoswitch-claude-code.lock" ]; then echo "engine lock file: present"; else echo "engine lock file: absent"; fi
   }
   # The roster: (position, id, active) per account.
   tt_accounts() {
     tagteam list --json | /usr/bin/python3 -c '
   import json, sys
   print(sorted((a["position"], a["id"], a["active"]) for a in json.load(sys.stdin)["accounts"]))
   '
   }
   # Each account's decision-grade 5h and 7d percentages, and the hours until its 7d window
   # resets; "*" marks the live one.
   tt_heads() {
     tagteam list --json | /usr/bin/python3 -c '
   import json, sys, time
   from datetime import datetime
   for a in json.load(sys.stdin)["accounts"]:
       u = a.get("usage") or {}
       five, seven = u.get("fiveHour") or {}, u.get("sevenDay") or {}
       at = seven.get("resetsAt")
       h = None if at is None else round((datetime.fromisoformat(at.replace("Z", "+00:00")).timestamp() - time.time()) / 3600, 1)
       print(a["position"], "*" if a["active"] else " ", a["usageStatus"], {"5h": five.get("pct"), "7d": seven.get("pct"), "7d resets in h": h})
   '
   }
   # A JSONL file of auto events, one per line: its kind and fields, without times or emails.
   # Fails on a line that is not one envelope-shaped JSON object.
   tt_events() {
     /usr/bin/python3 -c '
   import json, sys
   for line in open(sys.argv[1]):
       e = json.loads(line)
       assert e["schemaVersion"] == 1 and e["provider"] == "claude-code" and e["ts"].endswith("Z"), line
       f = {k: v for k, v in e.items() if k not in ("schemaVersion", "event", "ts", "provider", "email")}
       if isinstance(f.get("active"), dict):
           f["active"] = f["active"]["number"]
       print(e["event"], f)
   ' "$1"
   }
   ```
5. Run `tt_locks`, `tt_engine_lock` and `tt_accounts`. Expected: `locks left: 0` and
   `engine lock file: absent`. Keep the `tt_accounts` line as **A0**.
6. From `tt_heads`, pick **B**, the account whose relevant usage **U** (its higher percentage)
   is highest, and **T**, the whole number just below U: U 63.4 gives T 63. A lowered threshold
   is never below 50 (§6.4), so rows 2 and 3 need U ≥ 51; with every account below that, mark
   them n/a and let row 7's consume-first switch stand for a real switch.

**Reading the output.** Never copy a human line into the results: it carries an email. Record
positions, outcomes, reasons and exit codes. Every human line starts with the local time,
`HH:MM:SS`.

**Working out the expectation for rows 1–3 (`best`, §11.2).** Use only rows whose status is
`ok`; any other status means unknown headroom. Headroom is `100 −` the higher of 5h and 7d.
- Below the threshold (90 by default), the live account stays: `no switch: below-threshold`,
  exit 2.
- At or above it, the target is the candidate with the most headroom, ties to the lower
  position, among those whose usage is below the threshold and whose headroom beats the live
  account's by at least 10 points: `switched to #N (proactive)`, exit 0. With none,
  `no switch: no-qualifying-candidate` (exit 3) or another reason; record it and judge it
  against §11.2 step 8.

**Working out the expectation for row 7 (`consume-first`).** With the live account below 90 %,
the target is the candidate below 90 % with headroom left whose 7d window resets strictly sooner
than the live account's: the soonest, then the most headroom. The tick first fetches again every
reading older than 180 s, so `tt_heads` right before it shows the same numbers. With none:
`no switch: already-consuming-soonest`, or `no switch: reset-unknown` when a reset is unknown;
exit 2.

**Usage requests.** A loop polls the live account at most every 60 s near the threshold and
others less often, within 20 requests per account per hour (§8.6). If a tagteam in your main
setup polls the same accounts meanwhile, the endpoint may answer 429; the tick then reports the
usage unknown and backs off. That is expected behaviour, not a failure: note it with the row.

- [ ] **Step 4: Michael — run the acceptance**

Run every row in this order, in the same shell. After each row, `tt_locks` must print
`locks left: 0`.

| # | Run | Do | Pass when |
|---|---|---|---|
| 1 | `tt_heads`, then `tagteam auto --once --dry-run; echo "exit $?"`, then `tagteam auto --once --dry-run --json > "$TT/r1.jsonl"; echo "exit $?"`, `tt_events "$TT/r1.jsonl"`, `tt_engine_lock`, `tt_accounts`, `tt_locks` | Work out the expectation from `tt_heads` first | The human run prints one line, `HH:MM:SS  #P0 …  U%  …`, whose outcome matches the expectation, with `would switch to #N (proactive)` in place of a switch, and the matching exit code. The JSON run exits the same, and `tt_events` lists `poll` (`'active': P0`, `'threshold': 90.0`, a `headroomPct` entry for every position) and then the `no-switch`, or the `switch` with `'dryRun': True`. `engine lock file: absent`. `tt_accounts` equals A0: nothing switched. `locks left: 0` |
| 2 | `tagteam switch B`, then `tt_heads`, then `tagteam auto --once --threshold T; echo "exit $?"`, `tagteam status`, `tt_engine_lock`, `tt_locks` | Work out the expectation for B at threshold T first. n/a when U < 51 | The line shows `#B` at about U % and the expected outcome; on `switched to #N (proactive)`, `exit 0` and `status` names N. `engine lock file: present`; it stays after the run, unlocked. `locks left: 0` |
| 3 | `tagteam switch B`, then `tagteam auto --interval 15 --threshold T --cooldown 0`; after the Ctrl-C, `echo "exit $?"`, then `stty -a \| tr -s ' \t' '\n' \| grep -x -e isig -e -isig -e echo -e -echo`, `tagteam status`, `tt_locks` | Watch at least three lines, then press Ctrl-C. n/a when U < 51 | The first line switches as row 2 did (`--cooldown 0`, since row 2's switch started a 5-minute cooldown). The next lines show the new live account and `no switch: below-threshold`, each 13–20 s after the one before (the delay is 15 s ±10%, the tick's own run time adds to it, and the stamps are whole seconds). After Ctrl-C the prompt is back within about a second, with nothing printed, and `exit 0`. The `stty` command is visible as you type it and prints `isig` and `echo`: the terminal is intact. `locks left: 0` |
| 4 | `tagteam auto --json --interval 15 > "$TT/r4.jsonl" & pid=$!`, then `sleep 40; kill -TERM $pid; wait $pid; echo "exit $?"`, then `tt_events "$TT/r4.jsonl"`, `tt_locks` | Nothing: SIGTERM after 40 s | `exit 0`, right after the `kill`. `tt_events` checks every line and lists two or three ticks (a tick's own fetches can push the third past 40 s), each a `poll` (`'threshold': 90.0`) followed by its `no-switch` or `switch`: §11.4's fields, `headroomPct` keyed by position. A blocked tick (`no-candidates`, `all-exhausted`, …) is followed by a `sleep` event instead of the next tick. No line other than events. `locks left: 0` |
| 5 | `tt_heads`, then `tagteam auto --json --interval 15 > "$TT/r5.jsonl" & pid=$!`, then `sleep 5; tagteam switch N; echo "exit $?"`, then `sleep 40; kill -TERM $pid; wait $pid`, `tt_events "$TT/r5.jsonl"`, `tagteam status`, `tt_locks` | From `tt_heads`, pick **N**: a stored account other than the live one, status `ok`, whose 5h and 7d are both below 90, so it is below the default threshold and not at its limit. Under the default `best` strategy every tick on N is then a `below-threshold` no-op, and no switch away from N is valid. With no such account, mark the row n/a | The manual switch exits 0. In `tt_events`, the first `poll` has the account live before (**L**) as `active`; every later `poll` has N and is followed by a `no-switch` `below-threshold` (or `active-usage-unknown` while the endpoint answers 429, as **Usage requests** says). No `switch` event has `'to': L`: the loop never switches back. If the manual switch landed inside a tick's own switch, that tick's `no-switch` is `live-changed` and the next tick decides from N; by hand this is rare. `status` names N. `locks left: 0` |
| 6 | `tagteam auto --interval 15 > /dev/null & pid=$!`, then `sleep 5`, `tagteam auto; echo "exit $?"`, `tagteam auto --json; echo "exit $?"`, `tagteam auto --once --json; echo "exit $?"`, `tagteam auto --once; echo "exit $?"`, then `kill -TERM $pid; wait $pid; echo "exit $?"`, `tt_locks` | Nothing | `tagteam auto` prints `tagteam: auto-switch already runs for Claude Code` and `exit 1`; with `--json`, `{"schemaVersion":1,"error":{"type":"engine-running","message":"auto-switch already runs for Claude Code"}}` and `exit 1`. `--once --json` prints one line, `{"schemaVersion":1,"event":"no-switch","ts":"…Z","provider":"claude-code","reason":"engine-running","detail":""}`, and `exit 2`; the human `--once` prints only `HH:MM:SS  warning: auto-switch already runs for Claude Code in another process; skipping it` on stderr, and `exit 2`. The first loop then exits 0. `locks left: 0` |
| 7 | `tt_heads`, then `tagteam auto --once --strategy consume-first --cooldown 0; echo "exit $?"`, `tagteam status`, `tt_locks` | Work out the expectation first | It matches: `switched to #N (consume-first)` and `exit 0`, and `status` names N; or `no switch: already-consuming-soonest` (or `reset-unknown`) and `exit 2`. `locks left: 0` |
| 8 | `tagteam switch P0; echo "exit $?"`, then `tt_accounts`, `tt_locks` | Nothing: this restores the starting login | `exit 0` (`already-active` if P0 is live already). `tt_accounts` equals A0. `locks left: 0` |

Optional, on a terminal (§11.1, §9.2): with `CLAUDE_CONFIG_DIR` set to
`"$XDG_DATA_HOME/tagteam/sessions/x"` for one command, `tagteam auto --once` refuses with
`tagteam: this command cannot run inside a `tagteam run` session` and `exit 1`, and
`tagteam auto --once --dry-run` runs (it is not refused); what it reports there is §12.8's to
settle with M4. The binary tests pin the refusal and that the dry run is allowed.

- [ ] **Step 5: Michael — decide, record and clean up**

- **Pass (rows 1–8, n/a rows explained):** keep the table for the pull request description, with
  exit codes, outcomes, reasons and positions, plus the Step 1 totals as passed/total. Never
  record a token or an email. Then clean up and continue with Step 6.
- **Fail:** stop. Do not open or merge the pull request. Bring the table to Michael. Where to
  look:
  - Rows 1–3 and 7, a wrong outcome: `decide` and its predicates (Tasks 4 and 5), the tick's
    collection and snapshot (Tasks 3 and 8), or the flags (Tasks 9 and 10).
  - Row 3's spacing, or a loop that never ticks again: the loop (Task 9).
  - A lock directory left behind after any row: the switch's locks through the tick (Tasks 7
    and 8), or the loop's exit (Task 9).
  - Rows 3 and 4's exit, or the terminal: the loop's signal handling (Tasks 9 and 10).
  - Row 4's lines: the JSON sink (Task 10).
  - Row 5, a switch back: the precondition under the locks (Task 7) or the tick (Task 8).
  - Row 6: the engine lock (Task 8) and the CLI's mapping (Tasks 9 and 10).

Clean up in the same shell, so the throwaway Keychain items go too:
1. Run `claude`, type `/logout`, and leave with `/exit`. Claude Code deletes the throwaway
   config dir's Keychain item.
2. Run `tagteam list`, then `tagteam remove N` for each stored position. Each removal deletes
   that account's vault item from the login keychain.
3. Check that the throwaway item is gone; the probe reads attributes only, never the secret:
   ```bash
   security find-generic-password -s "Claude Code-credentials-$(printf %s "$CLAUDE_CONFIG_DIR" | shasum -a 256 | cut -c1-8)" >/dev/null 2>&1; echo "throwaway item rc: $?"
   ```
   Expected: `throwaway item rc: 44` (absent). If it prints `0`, delete the item:
   ```bash
   security delete-generic-password -s "Claude Code-credentials-$(printf %s "$CLAUDE_CONFIG_DIR" | shasum -a 256 | cut -c1-8)" >/dev/null
   ```
4. `rm -rf "$TT"`, then
   `unset TT CLAUDE_CONFIG_DIR XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME`, or close the shell.

- [ ] **Step 6: Open the pull request**

The driver pushes the branch and opens the Draft pull request. The repository has no template,
so the description gives what, why and how to verify, with the Step 5 table and the Step 1
totals. It records each review ruling under "Rulings" (global workflow), and Tasks 7 and 8's
known limitations:
- A recovered automatic switch records no cooldown and no no-return bar: the switch journal names
  neither its source nor its trigger.
- A `--once` run right after a manual switch inherits the old account's unknown-usage count once.

This is not an agent command of this plan.

- [ ] **Step 7: Mark the plan implemented**

This is the final pre-merge commit, made once review has converged and merging is the next
action. Set line 3 of this plan from whatever it says now (`In progress`, with or without the
pull request reference) to:

```markdown
**Status:** Implemented — <the Draft pull request's URL>
```

This follows the design-record rules: the plan is Implemented and cites its pull request. The
spec stays `In progress` until M5.

```bash
git add docs/superpowers/plans/2026-10-02-tagteam-m3b-auto-switch.md
git commit -m "Mark the M3b plan implemented"
```

The driver pushes this commit to the open pull request.
