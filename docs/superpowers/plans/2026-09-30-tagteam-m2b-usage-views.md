# tagteam M2b — Usage and Views Implementation Plan

**Status:** Approved

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** tagteam knows how much of each account's plan is used. `list` shows every account's
5-hour, 7-day, spend and per-model usage with pace markers, `history` shows the burn rate and
when a window runs out, and `statusline` puts the live account's usage in Claude Code's status
bar. Fetching stays within the usage endpoint's budget across all tagteam processes, and the
live token is refreshed by tagteam only through §7.5, when a usage fetch finds it expired or
rejected.

**Architecture:**
- **Pure policy in `tagteam-core`.** Generic windows, headroom, failure backoff, trust, poll
  planning, the budget arithmetic and pace/projection are pure functions of their inputs, with
  time passed in (spec §4.1).
- **The provider owns the wire.** `Provider::fetch_usage` builds the request and normalizes the
  response into generic windows; `Provider::render_usage` turns windows back into the provider's
  JSON shape (§4.5, §8.2, §13.2). Claude Code's implementation lives in `tagteam-cc`, and
  `FakeAgent` gets its own.
- **The engine owns the collector.** Reserve, fetch and record (§8.3) run over the store's
  usage tables. Tokens come from the refresh gate (§7.3) for inactive accounts and from
  active-token refresh (§7.5) for the live one, which M2a built and this plan wires in.
- **The CLI renders.** `list`, `history` and `statusline` read engine views. `statusline` never
  builds the HTTP client: the engine constructs it lazily, on first use.

**Tech Stack:** Rust (edition 2024), rusqlite (bundled), serde_json, `toml_edit` (new, read-only
settings), clap 4, `ureq` 3 through the existing `Http` port, thiserror, tracing; tests with
tempfile, assert_cmd, `ScriptedHttp` and the recorded `usage-200.json` fixture.

**Spec:** `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`. Read §1.1, §4.1,
§4.5, §6.1, §6.4, §7.3–§7.5, §8, §13.1, §13.2, §13.4, §13.5, §15 and Appendix A.5 before
starting any task. Section numbers below refer to that spec. The amendments this plan
implements landed in commits `992cdfe`, `3db6dba` and `c3a735e` (§8.7: projections are measured
from the reading's `fetched_at`, and pace covers `Scoped` windows with a known period).

## Execution notes

- When execution starts, set this plan's `**Status:**` to `In progress` in one commit. The spec
  is already `In progress` and stays so until M5.
- **Run this plan in its own worktree** (`wt m2b-usage-views`, created by Michael), not in the
  primary checkout, which holds M2a's branch and the release binary its live acceptance uses.
- Task 18 is a human step: the live acceptance run is the last step before the merge request.
- Feature flags used by tests (unchanged from M2a): `tagteam-provider/file-keychain`,
  `tagteam-provider/mock-server`, `tagteam-engine/test-hooks`, `tagteam-cc/test-hooks`, and
  `tagteam/test-support`, which enables all of them.
- Tests never reach the real network. Engine tests use `ScriptedHttp`; the binary's test harness
  points every endpoint at `http://127.0.0.1:9` unless a test starts a `MockServer`.
- Clippy must pass both with `--features tagteam/test-support` and with no features (M2a's lint
  ruling): hook-only test code is gated on `test-hooks`.

## Milestones

| Milestone | Scope |
|---|---|
| M1, M2a | Implemented (`6c80ff7`; M2a on `m2a-network-credentials`) |
| **M2b (this plan)** | Read-only settings, generic usage windows, backoff, trust, poll policy, budget, pace and projection, Claude Code's usage normalization, `FakeAgent` usage, the store's usage tables, the collector with the §7.5 triggers, the post-switch poll re-plan, the lazy HTTP client, `list` with usage, `history`, `statusline`, three M1 carry-overs |
| M3 | Auto-switch (`decide()`, simulations, `auto`, scheduled collection), `best` / `next-available`, `primary_long_window`, the SIGINT handler (release blocker) |
| M4 | `tagteam run`; `statusline`'s profile lookup |
| M5 | Export/import, `doctor`, `config` (writes), `displaced`, `purge`, `completions`, logging to file, `cargo xtask compat`, CI and release |

**Deliberately absent from M2b, and why that is safe:**
- **No scheduled collection.** Only `list` and `status` collect (§8.3). `CollectMode` has one
  variant, `OnDemand`; M3 adds `Scheduled`. The poll plan is still computed and stored on every
  record, so M3 starts from real plans.
- **No `config` command.** M2b reads `config.toml` (forgivingly, §6.4) but never writes it.
- **`switch` never collects.** Rotation and direct switches do not rank by usage (§8.3).

## Decisions

Rulings made while planning. Each names what it would cost if wrong.

1. **Units.** `usage_state`, `usage_samples` and `usage_requests` hold epoch **seconds**, and
   `leases.expires_at` holds epoch **milliseconds** (§6.1's column comments). Every core
   function takes seconds unless its parameter name ends in `_ms`. Cost if wrong: a unit bug,
   which the store tests pin.
2. **`last_429_at` holds when the 429's backoff lifts**, not when the 429 arrived. §8.6 anchors
   "recent 429" on the lift, success must not clear it (§6.1), and §8.4's "after a 429" is
   `last_429_at > fetched_at` either way. Cost if wrong: a column meaning, documented in the
   store.
3. **`tagteam-core` gains `serde_json`** so generic windows carry §4.5's provider-owned `detail`
   JSON and serialize themselves for `last_good`. Core stays free of I/O. Cost: one dependency
   edge.
4. **Settings are read-only in M2b**, with `toml_edit` (the crate §6.4 names for M5's writes).
   Only the keys M2b consumes are read: `autoswitch.threshold`, `autoswitch.models`,
   `usage.history_retention_days`, `statusline.format`, `ui.color`. Cost: M5 extends the same
   module.
5. **ISO 8601 `resets_at` parsing is hand-written** in `tagteam-cc` (no date crate). It accepts
   `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)` and rejects anything else, which drops that
   window's `resets_at` rather than the window. Cost: an exotic format reads as "no reset time".
6. **Jitter** is drawn by the engine (`fastrand`) and passed into core as a fraction in
   `[-1, 1]`, so the policy stays deterministic under test.
7. **Pruning `usage_samples` at most once a day** (§6.1) is tracked by a lease row named
   `prune:usage_samples` whose `expires_at` is the next allowed prune. Cost: none; leases are
   exactly that kind of harmless-overlap marker.
8. **The reserve transaction is `IMMEDIATE`**, so two processes cannot both count the budget
   before either inserts its slot. Existing store transactions stay `DEFERRED`.
9. **Poll-plan clamps.** A reset time at or before `now` is ignored. When the reset cap and the
   floor disagree, the floor wins: `next_poll_at = max(now + floor, min(now + interval,
   reset + 60))`. Cost if wrong: a poll up to 120 s later than the reset slack would allow.
10. **`usageStatus` is derived** from the kind, quarantine, capabilities and `last_error`
    (Task 13's table). New `last_error` tokens beyond §6.1's list: `no-access-token`,
    `vault-absent`, `keychain-unavailable`, `token-expired`, `foreign-credential`.
11. **Setup tokens are fetched like OAuth tokens.** Whether the usage endpoint accepts a
    `user:inference`-only token is unrecorded; a refusal is an ordinary failure with backoff.
    Cost if wrong: one request per backoff period for each setup-token account.
12. **M1 carry-overs** taken here (Task 17): L343 (`ProcessRunner` kill and join timeout, before
    the collector's threads call `security`), L342 (the fake Keychain models `security`'s rc
    0/44), L397 (the `ShadowingItem` wording). L311 stays with M5.

## Global Constraints

Every task's requirements include these. Values are copied from the spec.

- Platforms: macOS and Linux only (§1.2). Rust edition 2024, MSRV ≥ 1.85, toolchain pinned by
  `rust-toolchain.toml` (§16). `#![forbid(unsafe_code)]` stays in core, engine and fake.
- Usage request: `GET https://api.anthropic.com/api/oauth/usage` with
  `Authorization: Bearer <accessToken>`, `anthropic-beta: oauth-2025-04-20`,
  `User-Agent: tagteam/<version>`, and a 5 s timeout (§8.1, Appendix A.5).
- **Budget:** no identity is sent more than **20** usage requests in any rolling hour, counted in
  `usage_requests` across processes, keyed by `(provider, identity_key)`. A slot is valid for
  60 s; the count covers the last 3660 s; rows older than 3660 s are pruned on insert (§8.6).
- Every request counts: first fetches, the retry after a 401, and nothing else is ever sent
  without a slot (§8.6).
- A usage fetch never refreshes the active account's token itself; only §7.5 does. An inactive
  account's token is refreshed only through the gate (§7.3, §8.1).
- No lock is held during a usage fetch, other than the ones the gate or §7.5 take for their own
  refresh (§8.3). The active path takes tagteam's mutation lock only around its read of the live
  identity and credential, and drops it before any request (Task 11). The oracle is never called
  under any lock (§7.6).
- Leases: `usage:<id>`, 90 s TTL, taken with §6.1's single statement; a result is recorded, a
  refusal stamped, and a request sent only if the lease row still shows the same holder and the
  account's identity is unchanged (§8.3; Task 8's `authorize_send`).
- Failure never touches `last_good` or `fetched_at` (§8.3).
- Poll constants, trust, backoff and pace: exactly §8.4–§8.7 (as amended in `c3a735e`).
- Secrets never reach `Debug`, logs or error messages. Log lines identify accounts by position
  and ID, never by email (§4.4).
- `--json` stdout is exactly one JSON object; warnings and notices go to stderr (§13.2).
- `statusline`: no network, no Keychain access, never constructs the `Http` adapter; ≤ 10 ms
  p95. `list` ≤ 50 ms p95 when no usage fetch is due (§1.1, §13.5).
- History retention: samples older than `usage.history_retention_days` (default 180, valid
  1–3650) are pruned at most once a day, on the write path (§6.1).
- Tests never touch the real HOME, the login keychain, or the network (§15.1).
- Commits: small, imperative mood, no license headers, no agent attribution of any kind.

## Review Focus

Inputs and conditions the spec implies but no feature test would naturally hit. Each has a
pinning test in the task named.

1. **Two `tagteam list` processes at once** (two terminals, or a script in a loop). Expected:
   each account is fetched at most once per lease, and the budget counts each request exactly
   once. → Task 8 (two stores on one file) and Task 10 (two engines on one data dir).
2. **A process suspended between reserving and sending** (laptop lid closed). Expected: a slot
   older than 60 s is discarded and a fresh one reserved before sending; a slot handed back after
   more than the count window deletes nothing, even when its rowid was reused; a sender that lost
   its lease sends nothing, and a result recorded after another process took the lease is
   dropped. → Task 8 (slot age, rowid reuse, `authorize_send`) and Task 10 (late send, late
   record).
3. **A hostile or odd `Retry-After`** (`0`, `1e400`, `inf`, `-5`, an HTTP date). Expected:
   seconds form only; non-finite values clamp to the cap and are never stored; a date is
   ignored. → Task 3.
4. **Removing and re-adding an account within the hour.** Expected: its usage rows are gone
   (cascade), but its budget count is not reset, because `usage_requests` is keyed by identity.
   → Task 8.
5. **`~/.claude.json` missing, garbled, or rewritten between two `statusline` runs.** Expected:
   missing prints nothing; garbled prints nothing and exits 0; a changed mtime or size re-parses
   and updates `live_identity_cache`. → Task 16.

---

## File Structure

```
Cargo.toml                                  + toml_edit workspace dependency
crates/tagteam-core/
  Cargo.toml                                + serde_json
  src/lib.rs                     MOD        usage, backoff, trust, poll, pace modules
  src/usage.rs                   NEW        Window, WindowKind, JSON form, relevance, headroom (Task 2)
  src/backoff.rs                 NEW        failure backoff, Retry-After parsing (Task 3)
  src/trust.rs                   NEW        decision-grade trust (Task 3)
  src/poll.rs                    NEW        PollBudget, plan_after_fetch, budget arithmetic (Task 4)
  src/pace.rs                    NEW        regression and average pace, projections (Task 5)
crates/tagteam-provider/
  src/provider.rs                MOD        UsageResult; fetch_usage, poll_budget, render_usage,
                                            live_identity_source (Task 7)
  src/lib.rs                     MOD        re-exports
  src/security.rs                MOD        ProcessRunner kill and join timeout (Task 17, L343)
  src/keychain.rs                MOD        FakeKeychain models rc 0/44 (Task 17, L342)
crates/tagteam-cc/
  src/usage.rs                   NEW        usage request, ISO 8601, normalization, rendering (Task 6)
  src/lib.rs                     MOD        usage module
  src/provider.rs                MOD        the Task 7 trait methods; ShadowingItem wording (Task 17)
  tests/usage.rs                 NEW        fixture-driven normalization tests (Task 6)
crates/tagteam-fake/
  src/usage.rs                   NEW        FakeAgent's usage endpoint shape (Task 7)
  src/provider.rs, src/lib.rs    MOD        capabilities.usage = true; the Task 7 methods
  tests/provider.rs              MOD
crates/tagteam-engine/
  Cargo.toml                                + toml_edit
  src/settings.rs                NEW        read-only config.toml (Task 1)
  src/lazy_http.rs               NEW        LazyHttp (Task 9)
  src/store/usage.rs             NEW        usage_state, reserve/record, samples, leases, cache (Task 8)
  src/store/mod.rs               MOD        `mod usage;`, re-exports
  src/collect.rs                 NEW        the collector (Tasks 10, 11)
  src/views.rs                   MOD        UsageView, UsageStatus, history and statusline views (Task 13)
  src/switch.rs                  MOD        post-switch poll re-plan (Task 12)
  src/engine.rs                  MOD        settings in EngineConfig and Engine (Task 9)
  src/lib.rs                     MOD        new modules
  tests/common/mod.rs            MOD        script_usage, usage helpers (Task 10)
  tests/{settings,store_usage,collect,collect_active,views_usage}.rs NEW
crates/tagteam/
  src/cli.rs                     MOD        History, Statusline commands (Tasks 15, 16)
  src/app.rs                     MOD        LazyHttp, settings, collect before list/status (Tasks 9, 14)
  src/render.rs                  MOD        usage JSON, list table, colours (Task 14)
  src/history.rs                 NEW        history rendering (Task 15)
  src/statusline.rs              NEW        format, --print-config (Task 16)
  tests/{usage_cli,history,statusline}.rs NEW; tests/perf.rs NEW (ignored timing tests, Task 16)
```

---

## Interface Contract

Every task implements exactly these names and signatures. A task that finds one unworkable
stops and reports it rather than inventing a variant, because other tasks' code is written
against this list. Doc comments are abbreviated here; the tasks carry the full ones. All times
are epoch **seconds** unless the name ends in `_ms` (Decision 1).

### `tagteam-core`

The types below are re-exported at the crate root (`tagteam_core::Window`, …); the free
functions are reached through their modules (`tagteam_core::usage::headroom`, …).

**`src/usage.rs`** (Task 2):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowKind { Short, Long, Spend, Scoped }
impl WindowKind {
    pub fn as_str(self) -> &'static str;              // "short" | "long" | "spend" | "scoped"
    pub fn parse(s: &str) -> Option<Self>;
    pub fn has_pace(self) -> bool;                    // Long | Scoped (with a period; §8.7)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub key: String,              // provider window key; CC: "5h" | "7d" | "spend" | "scoped:<name>"
    pub label: String,            // CC: "5h" | "7d" | "spend" | "<name>"
    pub kind: WindowKind,
    pub pct: f64,                 // always finite
    pub resets_at: Option<i64>,
    pub period_s: Option<i64>,
    pub detail: Option<serde_json::Value>,   // provider-owned (CC spend: {used, limit, currency})
}

/// `last_good`'s stored form: `[{"key","label","kind","pct","resetsAt"?,"periodS"?,"detail"?}]`.
pub fn windows_to_json(windows: &[Window]) -> serde_json::Value;
/// `None` when the value is not that shape, or any `pct` is not finite (a corrupt row reads
/// as "no reading", never as an error).
pub fn windows_from_json(v: &serde_json::Value) -> Option<Vec<Window>>;

/// §8.2 relevance: Short and Long always; Scoped when `models` names it (case-insensitive) or
/// contains "all"; Spend never.
pub fn is_relevant(w: &Window, models: &[String]) -> bool;
/// `100 − max(relevant pct)`; `None` when no window is relevant (unknown headroom).
pub fn headroom(windows: &[Window], models: &[String]) -> Option<f64>;
/// `max(relevant pct)`; `None` when no window is relevant.
pub fn max_relevant_pct(windows: &[Window], models: &[String]) -> Option<f64>;
/// The earliest `resets_at` among relevant windows, if any.
pub fn earliest_relevant_reset(windows: &[Window], models: &[String]) -> Option<i64>;
```

**`src/backoff.rs`** (Task 3):

```rust
/// Retry-After in its seconds form only (§8.1): an integer or decimal, possibly huge or `inf`.
/// Negative, empty or HTTP-date values give `None`.
pub fn parse_retry_after(value: &str) -> Option<f64>;
/// §8.5. `consecutive_failures` counts this failure (≥ 1). Non-finite `retry_after_s` is
/// clamped to the cap. Returns whole seconds, never negative.
pub fn failure_backoff_s(consecutive_failures: u32, is_429: bool, retry_after_s: Option<f64>) -> i64;
```

**`src/trust.rs`** (Task 3):

```rust
pub const STALE_OK_S: i64 = 300;
pub const TRUST_MAX_AGE_S: i64 = 3600;
pub const POST_429_TRUST_CAP_S: i64 = 7200;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrustInputs {
    pub now_s: i64,
    pub fetched_at: Option<i64>,
    pub consecutive_failures: u32,
    pub plan_in_force: bool,                   // next_poll_at > now_s
    pub live_lease: bool,
    pub last_429_at: Option<i64>,              // Decision 2
    pub earliest_relevant_reset: Option<i64>,
}
/// §8.4: whether the reading may drive a decision (and be shown as `usage`, §13.2).
pub fn decision_grade(t: &TrustInputs) -> bool;
```

**`src/poll.rs`** (Task 4):

```rust
/// §8.6's constants. Providers return it from `Provider::poll_budget`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PollBudget {
    pub hourly_requests: u32,        // 20
    pub slot_valid_s: i64,           // 60
    pub count_window_s: i64,         // 3660
    pub floor_s: i64,                // 180 (also the serve TTL)
    pub urgent_s: i64,               // 60
    pub active_max_s: i64,           // 300
    pub candidate_default_s: i64,    // 300
    pub candidate_max_s: i64,        // 600
    pub exhausted_s: i64,            // 600
    pub movement_delta: f64,         // 1.0
    pub jitter_frac: f64,            // 0.10
    pub post_429_min_s: i64,         // 360
    pub recent_429_window_s: i64,    // 3600
    pub post_429_mult: f64,          // 1.5
    pub post_429_max_s: i64,         // 1800
    pub escalation_margin: f64,      // 15.0
    pub reset_slack_s: i64,          // 60
}
impl PollBudget { pub const STANDARD: PollBudget; }       // the values above

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PollInputs {
    pub now_s: i64,
    pub active: bool,
    pub pct: Option<f64>,              // max relevant pct of the new reading
    pub prev_pct: Option<f64>,         // max relevant pct of the previous reading
    pub prev_interval_s: Option<i64>,  // usage_state.poll_interval_s
    pub threshold: f64,                // autoswitch.threshold
    pub last_429_at: Option<i64>,      // Decision 2
    pub next_relevant_reset: Option<i64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollPlan { pub interval_s: i64, pub next_poll_at: i64 }

/// §8.6 `plan_after_fetch`, with Decision 9's clamps. `jitter` is in [-1, 1].
pub fn plan_after_fetch(b: &PollBudget, i: &PollInputs, jitter: f64) -> PollPlan;
/// The default plan for an account whose role changed without a fetch (§8.3's post-switch
/// re-plan): the role's default interval, jittered, from `now_s`.
pub fn replan_for_role(b: &PollBudget, active: bool, now_s: i64, jitter: f64) -> PollPlan;
/// When a request may next be sent, given the reservation times counted in the window
/// (ascending): `None` if a slot is free now.
pub fn budget_next_free(b: &PollBudget, counted_at: &[i64], now_s: i64) -> Option<i64>;
```

**`src/pace.rs`** (Task 5):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionMethod { Regression, Average }
impl ProjectionMethod { pub fn as_str(self) -> &'static str; }   // "regression" | "average"

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample { pub fetched_at: i64, pub pct: f64, pub resets_at: Option<i64> }

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pace {
    pub expected_pct: Option<f64>,
    pub ahead: Option<bool>,
    pub rate_per_hour: Option<f64>,          // points per hour
    pub method: Option<ProjectionMethod>,
    pub exhaustion_at: Option<i64>,
    pub will_last_to_reset: Option<bool>,
}

/// Points per second over the current window instance (same `resets_at` ± 60 s, last 48 h of
/// samples before `fetched_at`); ≥ 3 samples spanning ≥ 2 h and a positive slope, else `None`.
pub fn regression_rate(samples: &[Sample], current_reset: Option<i64>, fetched_at: i64) -> Option<f64>;
/// §8.7 for one window as read at `fetched_at`. Regression first, the average fallback for
/// `has_pace` kinds with a `period_s`; `expected_pct` and `ahead` only for those kinds.
pub fn pace(w: &Window, fetched_at: i64, samples: &[Sample]) -> Pace;
```

### `tagteam-provider`

**`src/provider.rs`** — `UsageResult` lands in Task 6, because Claude Code's parser returns it;
Task 7 adds the four trait methods:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum UsageResult {
    /// 200 with a recognised body. Empty means "no usage" (§8.2).
    Windows(Vec<Window>),
    /// The credential has no access token; no request was sent (§8.1).
    NoAccessToken,
    /// 401 (§8.1: the caller decides between the gate and §7.5).
    Unauthorized,
    /// Anything else: `Http(429)` carries `retry_after_s`.
    Failed { kind: TransientKind, retry_after_s: Option<f64> },
}

// New trait methods (every provider implements them; no defaults):
fn fetch_usage(&self, http: &dyn Http, cred: &Credential) -> UsageResult;
fn poll_budget(&self) -> PollBudget;
/// §13.2: the provider's JSON for a row's `usage`/`lastGoodUsage`, from windows and their pace.
fn render_usage(&self, windows: &[(Window, Pace)]) -> serde_json::Value;
/// The file whose mtime and size key `live_identity_cache` (§13.5). CC: `~/.claude.json`.
fn live_identity_source(&self, env: &Env) -> Option<PathBuf>;
```

`Window`, `Pace` and `PollBudget` are re-exported from `tagteam_core` at `tagteam_provider`'s
root.

### `tagteam-cc`

**`src/usage.rs`** (Task 6):

```rust
pub const USAGE_TIMEOUT: Duration = Duration::from_secs(5);
pub const USAGE_BETA: &str = "oauth-2025-04-20";
pub fn usage_request(e: &Endpoints, access_token: &str) -> HttpRequest;
/// Epoch seconds from `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)` (Decision 5).
pub fn parse_iso8601(s: &str) -> Option<i64>;
/// §8.2's table. `Err(())` is `bad-response`: not an object, or a known key of the wrong type.
pub fn normalize(body: &Value) -> Result<Vec<Window>, ()>;
pub fn parse_usage(reply: Result<HttpResponse, HttpError>) -> UsageResult;
/// §13.2's cswap shape: `fiveHour`, `sevenDay`, `spend`, `scoped[]`.
pub fn render(windows: &[(Window, Pace)]) -> Value;
/// `YYYY-MM-DDTHH:MM:SSZ` (UTC) for rendering times.
pub fn format_iso8601(epoch_s: i64) -> String;
```

### `tagteam-engine`

**`src/settings.rs`** (Task 1):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode { Auto, Always, Never }

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub threshold: f64,                 // autoswitch.threshold, 90.0, 50–99.9
    pub models: Vec<String>,            // autoswitch.models, [], per-provider override first
    pub history_retention_days: u32,    // usage.history_retention_days, 180, 1–3650
    pub statusline_format: String,      // statusline.format, per-provider override first
    pub color: ColorMode,               // ui.color, auto
}
impl Default for Settings { /* the §6.4 defaults */ }
impl Settings {
    /// `env.config_dir()/config.toml`. Forgiving (§6.4): a missing file gives the defaults
    /// silently; a corrupt file or an invalid value gives its default plus one warning each.
    pub fn load(env: &Env, provider: &ProviderId) -> (Settings, Vec<String>);
}
```

**`src/lazy_http.rs`** (Task 9):

```rust
/// Builds the real adapter on the first `send`, never before (§13.5).
pub struct LazyHttp { /* OnceLock<Arc<dyn Http>>, factory */ }
impl LazyHttp {
    pub fn new(factory: impl Fn() -> Arc<dyn Http> + Send + Sync + 'static) -> Self;
    pub fn is_built(&self) -> bool;
}
impl Http for LazyHttp { /* builds once, then delegates */ }
```

**`EngineConfig` / `Engine`** (Task 9): `EngineConfig` gains `pub settings: Settings`;
`Engine::settings(&self) -> &Settings`. Every existing constructor call site passes
`Settings::default()` unless it loads them.

**`src/store/usage.rs`** (Task 8), all on `impl Store`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct UsageStateRow {
    pub account_id: AccountId,
    pub last_good: Option<Vec<Window>>,
    pub fetched_at: Option<i64>,
    pub last_attempt_at: Option<i64>,
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
    pub backoff_until: Option<i64>,
    pub next_poll_at: Option<i64>,
    pub poll_interval_s: Option<i64>,
    pub last_429_at: Option<i64>,
    pub rejected_fp: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub account_id: AccountId,
    pub provider: ProviderId,     // the budget's key, with identity_key
    pub identity_key: String,
    pub holder: String,           // random per acquisition (uuid v7)
    pub slot: i64,                // the first slot's usage_requests rowid
    pub slot_at: i64,             // when the first slot was reserved
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ineligible { Quarantined, Backoff, Leased, NotDue }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reserve { Reserved(Reservation), Ineligible(Ineligible), OverBudget { next_free_at: i64 } }

/// A budget slot: its `usage_requests` rowid and reservation time. Its full identity is these
/// plus the reservation's `provider` and `identity_key`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot { pub slot: i64, pub slot_at: i64 }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendGrant { Send(Slot), Rejected, LeaseLost, OverBudget { next_free_at: i64 } }

impl Store {
    pub fn usage_state(&self, id: &AccountId) -> Result<Option<UsageStateRow>, StoreError>;
    /// Phase 1 (§8.3), one IMMEDIATE transaction: eligibility (the on-demand rule when
    /// `on_demand`), the `usage:<id>` lease, pruning and counting `usage_requests`, and the
    /// first slot (`Reservation.slot`, `slot_at`). An over-budget result sets `next_poll_at` to
    /// `next_free_at` and takes no lease.
    pub fn reserve_usage(&self, account: &AccountRow, now_ms: i64, on_demand: bool,
                         budget: &PollBudget) -> Result<Reserve, StoreError>;
    /// Right before each request (§8.3, §8.6), in one IMMEDIATE transaction: `r` must still hold
    /// the `usage:<id>` lease (its row names `r.holder`, as the records' fence reads it) and the
    /// account's identity key must be unchanged (else `LeaseLost`); `access_fp` must not equal
    /// the durable `rejected_fp` (else `Rejected`). Then a slot to send
    /// under: `slot` itself while it is at most `slot_valid_s` old; otherwise a fresh one, giving
    /// the stale one back by its full identity; a fresh one when `slot` is None (the 401 retry).
    /// `OverBudget` when no fresh slot is free (the stale one is still given back). `LeaseLost`
    /// and `Rejected` write nothing.
    pub fn authorize_send(&self, r: &Reservation, slot: Option<&Slot>, access_fp: Option<&str>,
                          now_ms: i64, budget: &PollBudget) -> Result<SendGrant, StoreError>;
    /// Gives a slot back (a fetch that ended before sending, §8.3), in one statement matching
    /// its full identity: `rowid`, `provider`, `identity_key` and `at`. A slot whose row was
    /// pruned deletes nothing, even when SQLite has since reused its rowid.
    pub fn release_slot(&self, r: &Reservation, slot: &Slot) -> Result<(), StoreError>;
    /// Phase 3, success. Fenced by the lease holder and the account's identity key; `Ok(false)`
    /// when the fence failed (nothing written). Writes `last_good`, `fetched_at`,
    /// `last_attempt_at`, resets the failure fields and `rejected_fp`, stores the plan, inserts
    /// one `usage_samples` row per window, and prunes old samples at most once a day
    /// (Decision 7).
    pub fn record_usage(&self, r: &Reservation, windows: &[Window], now_s: i64, plan: &PollPlan,
                        retention_days: u32) -> Result<bool, StoreError>;
    /// Phase 3, failure. Same fence. Never touches `last_good` or `fetched_at`. Increments
    /// `consecutive_failures`, sets `last_error`, `last_attempt_at`, `backoff_until`, and
    /// `last_429_at` when given; deletes `release` (the unused slot) by its full identity in the
    /// same transaction.
    pub fn record_usage_failure(&self, r: &Reservation, kind: &str, now_s: i64,
                                backoff_until: i64, last_429_at: Option<i64>,
                                release: Option<&Slot>) -> Result<bool, StoreError>;
    /// Same fence: only while `r` holds the lease; `Ok(false)` and nothing written otherwise.
    /// Creates the row if missing (a 401 can come on an account's first fetch).
    /// `record_usage_failure` leaves `rejected_fp` alone; `record_usage` clears it.
    pub fn set_rejected_fp(&self, r: &Reservation, fp: Option<&str>) -> Result<bool, StoreError>;
    /// §8.3's post-switch re-plan. Task 12 calls it only for an account that has a reading,
    /// so an unread account keeps no plan and stays eligible on demand.
    pub fn set_poll_plan(&self, id: &AccountId, plan: &PollPlan) -> Result<(), StoreError>;
    /// Ascending by `fetched_at`; `window = None` means every window.
    pub fn usage_samples(&self, id: &AccountId, window: Option<&str>, since_s: i64)
        -> Result<Vec<(String, Sample)>, StoreError>;
    pub fn usage_lease_live(&self, id: &AccountId, now_ms: i64) -> Result<bool, StoreError>;
    pub fn live_identity_cache(&self, provider: &ProviderId)
        -> Result<Option<LiveIdentityCacheRow>, StoreError>;
    pub fn put_live_identity_cache(&self, row: &LiveIdentityCacheRow) -> Result<(), StoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveIdentityCacheRow {
    pub provider: ProviderId,
    pub path: String,
    pub mtime_ns: i64,
    pub size: i64,
    pub identity_key: Option<String>,   // None: the file held no live login
    pub label: Option<String>,
    pub account_uuid: Option<String>,
}
```

**`src/collect.rs`** (Tasks 10, 11):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectMode { OnDemand { accounts: Vec<AccountId> } }   // M3 adds Scheduled

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Collected {
    Recorded,
    Ineligible(Ineligible),
    OverBudget { next_free_at: i64 },
    Failed { kind: String },          // recorded as a failure
    Dropped,                          // the fence failed; nothing recorded
    Unsupported,                      // the provider lacks the `usage` capability
}

#[derive(Debug, Clone, Default)]
pub struct CollectReport {
    pub outcomes: Vec<(AccountId, Collected)>,
    pub warnings: Vec<String>,        // stderr lines: §7.5 errors, Unpersisted (§8.3)
}

impl Engine {
    /// §8.3 on demand: every listed account on its own thread; waits for all of them.
    pub fn collect_usage(&self, mode: CollectMode) -> Result<CollectReport, EngineError>;
}
```

**`src/views.rs`** (Task 13):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageStatus { Ok, TokenExpired, ApiKey, KeychainUnavailable, ReloginRequired,
                       ForeignCredential, NoCredentials, Unavailable, Unsupported }
impl UsageStatus { pub fn as_str(self) -> &'static str; }   // §13.2's snake_case values

#[derive(Debug, Clone, PartialEq)]
pub struct UsageView {
    pub status: UsageStatus,
    pub windows: Option<Vec<(Window, Pace)>>,   // the last good reading, with pace
    pub decision_grade: bool,                   // §8.4
    pub fetched_at: Option<i64>,
    pub age_s: Option<i64>,
    pub error: Option<String>,                  // last_error when status is Unavailable
    pub retry_at: Option<i64>,                  // backoff_until / next_poll_at when Unavailable
}
// AccountView gains `pub usage: UsageView`.

pub struct HistoryWindow { pub window: Window, pub samples: Vec<Sample>, pub pace: Pace }
pub struct HistoryView { pub account: AccountView, pub windows: Vec<HistoryWindow> }

pub enum StatuslineView {
    NoLogin,
    Unmanaged { email: String },
    Managed { account: AccountView },
}

impl Engine {
    /// Reads only; `window` filters by key or label (case-insensitive); `since_s` bounds samples.
    pub fn history(&self, account: &AccountId, window: Option<&str>, since_s: i64)
        -> Result<HistoryView, EngineError>;
    /// No network, no Keychain: `live_identity_cache` keyed by `live_identity_source`'s mtime
    /// and size, re-parsing through `Provider::live_identity` only when they change (§13.5).
    pub fn statusline(&self, provider: &ProviderId) -> Result<StatuslineView, EngineError>;
}
```

### `tagteam` (CLI)

```rust
// cli.rs (Tasks 15, 16)
History { account: Option<String>, #[arg(long)] window: Option<String>,
          #[arg(long, default_value = "7d")] since: String, #[arg(long)] csv: bool },
Statusline { #[arg(long = "print-config")] print_config: bool },
```

---

## Tasks

| # | Task | Human? |
|---|---|---|
| 1 | Read-only settings (`config.toml`) | |
| 2 | Generic usage windows, relevance and headroom (core) | |
| 3 | Failure backoff, `Retry-After`, trust (core) | |
| 4 | Poll budget and `plan_after_fetch` (core) | |
| 5 | Pace and projection (core) | |
| 6 | Claude Code usage request, normalization and rendering | |
| 7 | Provider trait growth; Claude Code and `FakeAgent` implementations | |
| 8 | Store: usage state, reserve and record, samples, cache | |
| 9 | Lazy HTTP client and settings in the engine | |
| 10 | Collector: reserve, token, fetch, record (inactive accounts) | |
| 11 | Collector: the active account and the §7.5 triggers | |
| 12 | Post-switch poll re-plan | |
| 13 | Usage views, history and statusline views | |
| 14 | `list` and `status` with usage | |
| 15 | `history` | |
| 16 | `statusline` and the timing tests | |
| 17 | M1 carry-overs (L343, L342, L397) | |
| 18 | Final verification and live acceptance | Steps 4–6 |

---

### Task 1: Read-only settings (`config.toml`)

**Files:**
- Modify: `Cargo.toml` (workspace dependency), `crates/tagteam-engine/Cargo.toml`, `Cargo.lock`
- Create: `crates/tagteam-engine/src/settings.rs`
- Modify: `crates/tagteam-engine/src/lib.rs`
- Test: `crates/tagteam-engine/tests/settings.rs`

**Interfaces:**
- Consumes: `Env::config_dir()` (`tagteam_provider`), `ProviderId` (`tagteam_core`).
- Produces (all in `tagteam_engine::settings`):
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode { Auto, Always, Never }

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub threshold: f64,                 // autoswitch.threshold, 90.0, 50–99.9
    pub models: Vec<String>,            // autoswitch.models, [], per-provider override first
    pub history_retention_days: u32,    // usage.history_retention_days, 180, 1–3650
    pub statusline_format: String,      // statusline.format, per-provider override first
    pub color: ColorMode,               // ui.color, auto
}
impl Default for Settings { /* the §6.4 defaults */ }
impl Settings {
    pub fn load(env: &Env, provider: &ProviderId) -> (Settings, Vec<String>);
}
pub const DEFAULT_THRESHOLD: f64;                 // 90.0
pub const DEFAULT_HISTORY_RETENTION_DAYS: u32;    // 180
pub const DEFAULT_STATUSLINE_FORMAT: &str;        // "{account} · 5h {5h}% · 7d {7d}%{stale}"
```

Decision 4 applies: M2b reads only the five keys it consumes, and never writes. The other §6.4
keys (`default_provider`, `autoswitch.interval_seconds`, …) are ignored silently until M5's
`config` command gives them a reader. `toml_edit` is the crate §6.4 names for M5's writes, so the
same module grows then.

Judgement calls, flagged rather than decided silently:
- A warning is one line per invalid key, of the form
  ``config.toml: `autoswitch.threshold` must be a number from 50 to 99.9 (ignored)``. A key that is both provider-scoped and global (`models`, `format`) warns under the name
  that was invalid, and then the next table is tried: an invalid provider override falls back
  to the global value, not straight to the default.
- A segment that should be a table but is not (`autoswitch = 5`) warns once and reads as absent.
- A corrupt file's warning carries no parser message: `toml_edit`'s error text quotes the file's
  lines, and a warning goes to stderr unfiltered.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-engine/tests/settings.rs`:
```rust
use std::fs;

use tagteam_core::ProviderId;
use tagteam_engine::settings::{ColorMode, Settings};
use tagteam_provider::Env;

const PROVIDER: &str = "claude-code";

/// Loads `text` as the `config.toml` of a fresh environment.
fn load_as(text: &str, provider: &str) -> (Settings, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    fs::create_dir_all(env.config_dir()).unwrap();
    fs::write(env.config_dir().join("config.toml"), text).unwrap();
    Settings::load(&env, &ProviderId::new(provider))
}

fn load(text: &str) -> (Settings, Vec<String>) {
    load_as(text, PROVIDER)
}

fn models(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_defaults_are_the_specs_table() {
    let d = Settings::default();
    assert_eq!(d.threshold, 90.0);
    assert_eq!(d.models, Vec::<String>::new());
    assert_eq!(d.history_retention_days, 180);
    assert_eq!(
        d.statusline_format,
        "{account} · 5h {5h}% · 7d {7d}%{stale}"
    );
    assert_eq!(d.color, ColorMode::Auto);
}

#[test]
fn a_missing_file_gives_the_defaults_without_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    let (settings, warnings) = Settings::load(&env, &ProviderId::new(PROVIDER));
    assert_eq!(settings, Settings::default());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(
        !env.config_dir().exists(),
        "loading never creates the config directory"
    );
}

#[test]
fn an_empty_file_gives_the_defaults_without_a_warning() {
    let (settings, warnings) = load("");
    assert_eq!(settings, Settings::default());
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn every_key_is_read_from_a_full_file() {
    let (settings, warnings) = load(
        r#"
[autoswitch]
threshold = 75.5
models = ["Fable", "Opus"]

[usage]
history_retention_days = 30

[statusline]
format = "{5h}%"

[ui]
color = "never"
"#,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        settings,
        Settings {
            threshold: 75.5,
            models: models(&["Fable", "Opus"]),
            history_retention_days: 30,
            statusline_format: "{5h}%".to_owned(),
            color: ColorMode::Never,
        }
    );
}

#[test]
fn keys_written_with_dotted_names_are_read_too() {
    let (settings, warnings) = load("autoswitch.threshold = 80\nui.color = \"always\"\n");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(settings.threshold, 80.0);
    assert_eq!(settings.color, ColorMode::Always);
}

#[test]
fn keys_this_milestone_does_not_read_are_ignored_without_a_warning() {
    let (settings, warnings) = load(
        "default_provider = \"claude-code\"\n[autoswitch]\ninterval_seconds = 120\nstrategy = \"best\"\nfuture = true\n",
    );
    assert_eq!(settings, Settings::default());
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn the_threshold_accepts_its_whole_range_as_an_integer_or_a_float() {
    for (text, expected) in [
        ("50", 50.0),
        ("50.0", 50.0),
        ("75", 75.0),
        ("99.9", 99.9),
        ("90.25", 90.25),
    ] {
        let (settings, warnings) = load(&format!("[autoswitch]\nthreshold = {text}\n"));
        assert_eq!(settings.threshold, expected, "{text}");
        assert!(warnings.is_empty(), "{text}: {warnings:?}");
    }
}

#[test]
fn an_invalid_threshold_falls_back_to_ninety_with_one_warning_naming_the_key() {
    for text in [
        "49.9", "49", "99.95", "100", "0", "-90", "\"90\"", "true", "nan", "inf", "[90]",
    ] {
        let (settings, warnings) = load(&format!("[autoswitch]\nthreshold = {text}\n"));
        assert_eq!(settings.threshold, 90.0, "{text}");
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(
            warnings[0].contains("`autoswitch.threshold`"),
            "{warnings:?}"
        );
    }
}

#[test]
fn the_history_retention_accepts_one_to_thirty_six_fifty() {
    for days in [1u32, 180, 3650] {
        let (settings, warnings) = load(&format!("[usage]\nhistory_retention_days = {days}\n"));
        assert_eq!(settings.history_retention_days, days);
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}

#[test]
fn an_invalid_history_retention_falls_back_to_one_eighty_with_a_warning() {
    for text in ["0", "3651", "-5", "1.5", "\"30\"", "true", "4294967297"] {
        let (settings, warnings) = load(&format!("[usage]\nhistory_retention_days = {text}\n"));
        assert_eq!(settings.history_retention_days, 180, "{text}");
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(
            warnings[0].contains("`usage.history_retention_days`"),
            "{warnings:?}"
        );
    }
}

#[test]
fn models_take_a_list_or_a_single_name() {
    let (settings, warnings) = load("[autoswitch]\nmodels = \"Fable\"\n");
    assert_eq!(settings.models, models(&["Fable"]));
    assert!(warnings.is_empty(), "{warnings:?}");

    let (settings, _) = load("[autoswitch]\nmodels = [\"Fable\", \" Opus \"]\n");
    assert_eq!(
        settings.models,
        models(&["Fable", "Opus"]),
        "names are trimmed"
    );

    let (settings, _) = load("[autoswitch]\nmodels = [\"all\"]\n");
    assert_eq!(settings.models, models(&["all"]));

    let (settings, warnings) = load("[autoswitch]\nmodels = []\n");
    assert_eq!(settings.models, Vec::<String>::new());
    assert!(warnings.is_empty(), "an empty list is valid: {warnings:?}");
}

#[test]
fn an_invalid_models_value_falls_back_to_no_models_with_a_warning() {
    for text in [
        "[\"Fable\", 3]",
        "[[\"Fable\"]]",
        "[\"\"]",
        "\"\"",
        "5",
        "true",
    ] {
        let (settings, warnings) = load(&format!("[autoswitch]\nmodels = {text}\n"));
        assert_eq!(settings.models, Vec::<String>::new(), "{text}");
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(warnings[0].contains("`autoswitch.models`"), "{warnings:?}");
    }
}

#[test]
fn a_providers_own_models_override_the_global_ones() {
    let (settings, warnings) = load(
        "[autoswitch]\nmodels = [\"Opus\"]\n\n[provider.claude-code.autoswitch]\nmodels = [\"Fable\"]\n",
    );
    assert_eq!(settings.models, models(&["Fable"]));
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn another_providers_override_does_not_apply() {
    let text = "[autoswitch]\nmodels = [\"Opus\"]\n\n[provider.fake-agent.autoswitch]\nmodels = [\"Fable\"]\n";
    assert_eq!(load(text).0.models, models(&["Opus"]));
    assert_eq!(load_as(text, "fake-agent").0.models, models(&["Fable"]));
}

#[test]
fn a_providers_override_without_the_key_leaves_the_global_value() {
    let (settings, _) = load(
        "[autoswitch]\nmodels = [\"Opus\"]\n\n[provider.claude-code.autoswitch]\nthreshold = 70\n",
    );
    assert_eq!(settings.models, models(&["Opus"]));
    assert_eq!(settings.threshold, 90.0, "threshold is not provider-scoped");
}

#[test]
fn an_invalid_provider_override_warns_and_the_global_value_applies() {
    let (settings, warnings) = load(
        "[autoswitch]\nmodels = [\"Opus\"]\n\n[provider.claude-code.autoswitch]\nmodels = 7\n",
    );
    assert_eq!(settings.models, models(&["Opus"]));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("`provider.claude-code.autoswitch.models`"),
        "{warnings:?}"
    );
}

#[test]
fn the_statusline_format_prefers_the_providers_table() {
    let (settings, warnings) = load(
        "[statusline]\nformat = \"{5h}\"\n\n[provider.claude-code.statusline]\nformat = \"{7d}\"\n",
    );
    assert_eq!(settings.statusline_format, "{7d}");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        load("[statusline]\nformat = \"{5h}\"\n")
            .0
            .statusline_format,
        "{5h}"
    );
}

#[test]
fn an_empty_or_blank_statusline_format_falls_back_with_a_warning() {
    for text in ["\"\"", "\"   \"", "5", "[\"x\"]"] {
        let (settings, warnings) = load(&format!("[statusline]\nformat = {text}\n"));
        assert_eq!(
            settings.statusline_format, "{account} · 5h {5h}% · 7d {7d}%{stale}",
            "{text}"
        );
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(warnings[0].contains("`statusline.format`"), "{warnings:?}");
    }
}

#[test]
fn the_color_mode_reads_its_three_values() {
    for (text, mode) in [
        ("auto", ColorMode::Auto),
        ("always", ColorMode::Always),
        ("never", ColorMode::Never),
    ] {
        let (settings, warnings) = load(&format!("[ui]\ncolor = \"{text}\"\n"));
        assert_eq!(settings.color, mode);
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}

#[test]
fn an_invalid_color_falls_back_to_auto_with_a_warning() {
    for text in ["\"rainbow\"", "\"Always\"", "true", "1"] {
        let (settings, warnings) = load(&format!("[ui]\ncolor = {text}\n"));
        assert_eq!(settings.color, ColorMode::Auto, "{text}");
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(warnings[0].contains("`ui.color`"), "{warnings:?}");
    }
}

#[test]
fn each_invalid_value_warns_once_and_the_valid_ones_still_apply() {
    let (settings, warnings) = load(
        r#"
[autoswitch]
threshold = 120
models = ["Fable"]

[usage]
history_retention_days = 0

[ui]
color = "always"
"#,
    );
    assert_eq!(settings.threshold, 90.0);
    assert_eq!(settings.models, models(&["Fable"]));
    assert_eq!(settings.history_retention_days, 180);
    assert_eq!(settings.color, ColorMode::Always);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("`autoswitch.threshold`"))
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("`usage.history_retention_days`"))
    );
}

#[test]
fn a_table_that_is_not_a_table_warns_once_and_reads_as_absent() {
    let (settings, warnings) = load("autoswitch = 5\n");
    assert_eq!(settings, Settings::default());
    assert_eq!(
        warnings.len(),
        1,
        "threshold and models share one warning: {warnings:?}"
    );
    assert!(
        warnings[0].contains("`autoswitch` must be a table"),
        "{warnings:?}"
    );
}

#[test]
fn a_corrupt_file_gives_the_defaults_and_one_warning_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    fs::create_dir_all(env.config_dir()).unwrap();
    let path = env.config_dir().join("config.toml");
    fs::write(&path, "[autoswitch\nthreshold = = 3\n").unwrap();
    let (settings, warnings) = Settings::load(&env, &ProviderId::new(PROVIDER));
    assert_eq!(settings, Settings::default());
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains(&path.display().to_string()),
        "{warnings:?}"
    );
    assert!(warnings[0].contains("not valid TOML"), "{warnings:?}");
}

#[test]
fn an_unreadable_file_gives_the_defaults_and_one_warning_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    // A directory where the file should be: reading it fails, and it is not "missing".
    let path = env.config_dir().join("config.toml");
    fs::create_dir_all(&path).unwrap();
    let (settings, warnings) = Settings::load(&env, &ProviderId::new(PROVIDER));
    assert_eq!(settings, Settings::default());
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains(&path.display().to_string()),
        "{warnings:?}"
    );
    assert!(warnings[0].contains("cannot read"), "{warnings:?}");
}

#[test]
fn a_file_that_is_not_utf8_reads_as_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    fs::create_dir_all(env.config_dir()).unwrap();
    fs::write(env.config_dir().join("config.toml"), [0xff, 0xfe, 0x00]).unwrap();
    let (settings, warnings) = Settings::load(&env, &ProviderId::new(PROVIDER));
    assert_eq!(settings, Settings::default());
    assert_eq!(warnings.len(), 1, "{warnings:?}");
}

#[test]
fn a_comment_heavy_file_with_inline_tables_reads_normally() {
    let (settings, warnings) =
        load("# my settings\nautoswitch = { threshold = 60, models = [\"all\"] } # inline\n");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(settings.threshold, 60.0);
    assert_eq!(settings.models, models(&["all"]));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-engine --test settings`
Expected: FAIL to compile: ``error[E0432]: unresolved import `tagteam_engine::settings` `` (there
is no such module yet).

- [ ] **Step 3: Add the `toml_edit` dependency**

In the root `Cargo.toml`, insert in `[workspace.dependencies]` after the `thiserror = "2"` line:
```toml
# config.toml (§6.4): read in M2b, written in M5; preserves comments and formatting.
toml_edit = "0.22"
```
In `crates/tagteam-engine/Cargo.toml`, insert in `[dependencies]` after `thiserror.workspace = true`:
```toml
toml_edit.workspace = true
```
(Resolving the new crate needs the crates.io index. If `cargo` fails with "Operation not
permitted" writing under `~/.cargo`, that is the sandbox; retry that one command unsandboxed.)

- [ ] **Step 4: Write `settings.rs` and register the module**

In `crates/tagteam-engine/src/lib.rs`, insert `pub mod settings;` between `mod rescue;` and
`pub mod store;`.

Create `crates/tagteam-engine/src/settings.rs`:
```rust
//! Read-only `config.toml` (§6.4): the keys M2b consumes. `tagteam config` and its writes land
//! in M5; this module only reads, and never fails: a missing file, a corrupt file and an invalid
//! value each fall back to the default, the last two with a warning for the caller to print.

use std::io::ErrorKind;

use tagteam_core::ProviderId;
use tagteam_provider::Env;
use toml_edit::{DocumentMut, Item, TableLike};

pub const DEFAULT_THRESHOLD: f64 = 90.0;
pub const DEFAULT_HISTORY_RETENTION_DAYS: u32 = 180;
pub const DEFAULT_STATUSLINE_FORMAT: &str = "{account} · 5h {5h}% · 7d {7d}%{stale}";

/// `ui.color`. `NO_COLOR`, `FORCE_COLOR` and `--no-color` are the CLI's to apply on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// `autoswitch.threshold`: 50–99.9.
    pub threshold: f64,
    /// `autoswitch.models`: model display names, or `all`. The provider's own table first.
    pub models: Vec<String>,
    /// `usage.history_retention_days`: 1–3650.
    pub history_retention_days: u32,
    /// `statusline.format`. The provider's own table first.
    pub statusline_format: String,
    /// `ui.color`.
    pub color: ColorMode,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            threshold: DEFAULT_THRESHOLD,
            models: Vec::new(),
            history_retention_days: DEFAULT_HISTORY_RETENTION_DAYS,
            statusline_format: DEFAULT_STATUSLINE_FORMAT.to_owned(),
            color: ColorMode::Auto,
        }
    }
}

impl Settings {
    /// `env.config_dir()/config.toml`. Forgiving (§6.4): a missing file gives the defaults
    /// silently; a corrupt or unreadable file gives the defaults and one warning naming the
    /// path; an invalid value gives its default and one warning naming the key. The rest of the
    /// file still applies.
    pub fn load(env: &Env, provider: &ProviderId) -> (Settings, Vec<String>) {
        let path = env.config_dir().join("config.toml");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => return (Settings::default(), Vec::new()),
            Err(e) => {
                let warning = format!(
                    "{}: cannot read the settings file ({}); using the defaults",
                    path.display(),
                    e.kind()
                );
                return (Settings::default(), vec![warning]);
            }
        };
        match text.parse::<DocumentMut>() {
            Ok(doc) => from_document(&doc, provider),
            Err(_) => {
                let warning = format!(
                    "{}: the settings file is not valid TOML; using the defaults",
                    path.display()
                );
                (Settings::default(), vec![warning])
            }
        }
    }
}

struct Reader<'a> {
    doc: &'a DocumentMut,
    warnings: Vec<String>,
}

impl<'a> Reader<'a> {
    fn warn(&mut self, message: String) {
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    /// The table at `path`, or `None` when it is absent. A segment that exists but is not a
    /// table warns and reads as absent.
    fn table(&mut self, path: &[&str]) -> Option<&'a dyn TableLike> {
        let doc: &'a DocumentMut = self.doc;
        let mut current: &'a dyn TableLike = doc.as_table();
        for (depth, segment) in path.iter().enumerate() {
            let item = current.get(segment)?;
            match item.as_table_like() {
                Some(table) => current = table,
                None => {
                    let dotted = path[..=depth].join(".");
                    self.warn(format!("config.toml: `{dotted}` must be a table (ignored)"));
                    return None;
                }
            }
        }
        Some(current)
    }

    /// The first valid value of `key` across `tables`, most specific first. A present value
    /// that `parse` rejects warns, naming the key, and the next table is tried.
    fn read<T>(
        &mut self,
        tables: &[&[&str]],
        key: &str,
        expect: &str,
        parse: impl Fn(&Item) -> Option<T>,
    ) -> Option<T> {
        for path in tables {
            let Some(table) = self.table(path) else {
                continue;
            };
            let Some(item) = table.get(key) else {
                continue;
            };
            match parse(item) {
                Some(value) => return Some(value),
                None => {
                    let dotted = path
                        .iter()
                        .copied()
                        .chain([key])
                        .collect::<Vec<_>>()
                        .join(".");
                    self.warn(format!("config.toml: `{dotted}` {expect} (ignored)"));
                }
            }
        }
        None
    }
}

fn from_document(doc: &DocumentMut, provider: &ProviderId) -> (Settings, Vec<String>) {
    let defaults = Settings::default();
    let mut reader = Reader {
        doc,
        warnings: Vec::new(),
    };
    let global_autoswitch: &[&str] = &["autoswitch"];
    let global_statusline: &[&str] = &["statusline"];
    let provider_autoswitch = ["provider", provider.as_str(), "autoswitch"];
    let provider_statusline = ["provider", provider.as_str(), "statusline"];

    let threshold = reader
        .read(
            &[global_autoswitch],
            "threshold",
            "must be a number from 50 to 99.9",
            parse_threshold,
        )
        .unwrap_or(defaults.threshold);
    let models = reader
        .read(
            &[&provider_autoswitch[..], global_autoswitch],
            "models",
            "must be a model name or a list of model names",
            parse_models,
        )
        .unwrap_or(defaults.models);
    let history_retention_days = reader
        .read(
            &[&["usage"]],
            "history_retention_days",
            "must be a whole number of days from 1 to 3650",
            parse_retention,
        )
        .unwrap_or(defaults.history_retention_days);
    let statusline_format = reader
        .read(
            &[&provider_statusline[..], global_statusline],
            "format",
            "must be a non-empty string",
            parse_format,
        )
        .unwrap_or(defaults.statusline_format);
    let color = reader
        .read(
            &[&["ui"]],
            "color",
            "must be \"auto\", \"always\" or \"never\"",
            parse_color,
        )
        .unwrap_or(defaults.color);

    let settings = Settings {
        threshold,
        models,
        history_retention_days,
        statusline_format,
        color,
    };
    (settings, reader.warnings)
}

fn parse_threshold(item: &Item) -> Option<f64> {
    let value = item
        .as_float()
        .or_else(|| item.as_integer().map(|i| i as f64))?;
    (50.0..=99.9).contains(&value).then_some(value)
}

fn parse_retention(item: &Item) -> Option<u32> {
    u32::try_from(item.as_integer()?)
        .ok()
        .filter(|days| (1..=3650).contains(days))
}

fn parse_models(item: &Item) -> Option<Vec<String>> {
    if let Some(one) = item.as_str() {
        return name(one).map(|n| vec![n]);
    }
    item.as_array()?
        .iter()
        .map(|v| v.as_str().and_then(name))
        .collect()
}

fn name(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn parse_format(item: &Item) -> Option<String> {
    let s = item.as_str()?;
    (!s.trim().is_empty()).then(|| s.to_owned())
}

fn parse_color(item: &Item) -> Option<ColorMode> {
    match item.as_str()? {
        "auto" => Some(ColorMode::Auto),
        "always" => Some(ColorMode::Always),
        "never" => Some(ColorMode::Never),
        _ => None,
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --test settings`
Expected: PASS, 26 tests, including
`an_invalid_threshold_falls_back_to_ninety_with_one_warning_naming_the_key`,
`a_corrupt_file_gives_the_defaults_and_one_warning_naming_the_file` and
`a_providers_own_models_override_the_global_ones`.

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

```
git add Cargo.toml Cargo.lock crates/tagteam-engine/Cargo.toml crates/tagteam-engine/src/lib.rs \
  crates/tagteam-engine/src/settings.rs crates/tagteam-engine/tests/settings.rs
git commit -m "Read config.toml settings forgivingly and read-only"
```

---

### Task 2: Generic usage windows, relevance and headroom (core)

**Files:**
- Modify: `crates/tagteam-core/Cargo.toml`, `Cargo.lock`
- Create: `crates/tagteam-core/src/usage.rs`
- Modify: `crates/tagteam-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces (`tagteam_core::usage`; `Window` and `WindowKind` are also re-exported at the crate root):
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowKind { Short, Long, Spend, Scoped }
impl WindowKind {
    pub fn as_str(self) -> &'static str;              // "short" | "long" | "spend" | "scoped"
    pub fn parse(s: &str) -> Option<Self>;
    pub fn has_pace(self) -> bool;                    // Long | Scoped (with a period; §8.7)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub key: String,              // provider window key; CC: "5h" | "7d" | "spend" | "scoped:<name>"
    pub label: String,            // CC: "5h" | "7d" | "spend" | "<name>"
    pub kind: WindowKind,
    pub pct: f64,                 // always finite
    pub resets_at: Option<i64>,
    pub period_s: Option<i64>,
    pub detail: Option<serde_json::Value>,   // provider-owned (CC spend: {used, limit, currency})
}

pub fn windows_to_json(windows: &[Window]) -> serde_json::Value;
pub fn windows_from_json(v: &serde_json::Value) -> Option<Vec<Window>>;
pub fn is_relevant(w: &Window, models: &[String]) -> bool;
pub fn headroom(windows: &[Window], models: &[String]) -> Option<f64>;
pub fn max_relevant_pct(windows: &[Window], models: &[String]) -> Option<f64>;
pub fn earliest_relevant_reset(windows: &[Window], models: &[String]) -> Option<i64>;
```

Decision 3: `tagteam-core` gains `serde_json` so a window can carry a provider's `detail` and
serialize itself for `usage_state.last_good`. Core stays free of I/O. Times are epoch seconds.

Judgement call: a scoped window matches `autoswitch.models` by its `label` (Claude Code's
`<name>`), not its `key`, so `models = ["Fable"]` matches the window keyed `scoped:Fable`.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-core/src/usage.rs` holding only the test module:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn win(key: &str, label: &str, kind: WindowKind, pct: f64) -> Window {
        Window {
            key: key.into(),
            label: label.into(),
            kind,
            pct,
            resets_at: None,
            period_s: None,
            detail: None,
        }
    }

    fn models(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn sample_windows() -> Vec<Window> {
        vec![
            Window {
                resets_at: Some(1_893_000_000),
                period_s: Some(18_000),
                ..win("5h", "5h", WindowKind::Short, 9.0)
            },
            Window {
                resets_at: Some(1_893_400_000),
                period_s: Some(604_800),
                ..win("7d", "7d", WindowKind::Long, 77.5)
            },
            Window {
                resets_at: Some(1_893_400_000),
                period_s: Some(604_800),
                ..win("scoped:Fable", "Fable", WindowKind::Scoped, 0.0)
            },
            Window {
                detail: Some(json!({"used": 12.5, "limit": 20.0, "currency": "EUR"})),
                ..win("spend", "spend", WindowKind::Spend, 62.5)
            },
        ]
    }

    #[test]
    fn kind_names_round_trip_and_unknown_names_parse_to_none() {
        for (kind, name) in [
            (WindowKind::Short, "short"),
            (WindowKind::Long, "long"),
            (WindowKind::Spend, "spend"),
            (WindowKind::Scoped, "scoped"),
        ] {
            assert_eq!(kind.as_str(), name);
            assert_eq!(WindowKind::parse(name), Some(kind));
        }
        assert_eq!(WindowKind::parse("Short"), None);
        assert_eq!(WindowKind::parse(""), None);
    }

    #[test]
    fn only_long_and_scoped_windows_have_pace() {
        assert!(!WindowKind::Short.has_pace());
        assert!(WindowKind::Long.has_pace());
        assert!(WindowKind::Scoped.has_pace());
        assert!(!WindowKind::Spend.has_pace());
    }

    #[test]
    fn windows_round_trip_through_json_with_detail() {
        let windows = sample_windows();
        let json = windows_to_json(&windows);
        assert_eq!(windows_from_json(&json), Some(windows));
    }

    #[test]
    fn the_stored_form_is_exactly_the_documented_one() {
        let json = windows_to_json(&sample_windows());
        assert_eq!(
            json,
            json!([
                {"key": "5h", "label": "5h", "kind": "short", "pct": 9.0,
                 "resetsAt": 1_893_000_000i64, "periodS": 18_000i64},
                {"key": "7d", "label": "7d", "kind": "long", "pct": 77.5,
                 "resetsAt": 1_893_400_000i64, "periodS": 604_800i64},
                {"key": "scoped:Fable", "label": "Fable", "kind": "scoped", "pct": 0.0,
                 "resetsAt": 1_893_400_000i64, "periodS": 604_800i64},
                {"key": "spend", "label": "spend", "kind": "spend", "pct": 62.5,
                 "detail": {"used": 12.5, "limit": 20.0, "currency": "EUR"}}
            ])
        );
    }

    #[test]
    fn optional_fields_are_omitted_when_absent() {
        let json = windows_to_json(&[win("5h", "5h", WindowKind::Short, 1.0)]);
        let o = json[0].as_object().unwrap();
        assert!(!o.contains_key("resetsAt"));
        assert!(!o.contains_key("periodS"));
        assert!(!o.contains_key("detail"));
    }

    #[test]
    fn an_empty_list_round_trips() {
        assert_eq!(windows_to_json(&[]), json!([]));
        assert_eq!(windows_from_json(&json!([])), Some(Vec::new()));
    }

    #[test]
    fn null_optional_fields_read_as_absent() {
        let v = json!([{"key": "5h", "label": "5h", "kind": "short", "pct": 3.0,
                        "resetsAt": null, "periodS": null, "detail": null}]);
        assert_eq!(
            windows_from_json(&v),
            Some(vec![win("5h", "5h", WindowKind::Short, 3.0)])
        );
    }

    #[test]
    fn an_integer_pct_reads_as_a_float() {
        let v = json!([{"key": "7d", "label": "7d", "kind": "long", "pct": 77}]);
        assert_eq!(windows_from_json(&v).unwrap()[0].pct, 77.0);
    }

    #[test]
    fn a_corrupt_value_reads_as_no_reading() {
        let good = json!({"key": "5h", "label": "5h", "kind": "short", "pct": 1.0});
        let cases = [
            ("not an array", json!({"key": "5h"})),
            ("a string", json!("5h")),
            ("null", json!(null)),
            ("an item that is not an object", json!([1])),
            (
                "missing key",
                json!([{"label": "5h", "kind": "short", "pct": 1.0}]),
            ),
            (
                "missing label",
                json!([{"key": "5h", "kind": "short", "pct": 1.0}]),
            ),
            (
                "missing kind",
                json!([{"key": "5h", "label": "5h", "pct": 1.0}]),
            ),
            (
                "missing pct",
                json!([{"key": "5h", "label": "5h", "kind": "short"}]),
            ),
            (
                "unknown kind",
                json!([{"key": "5h", "label": "5h", "kind": "monthly", "pct": 1.0}]),
            ),
            (
                "pct as a string",
                json!([{"key": "5h", "label": "5h", "kind": "short", "pct": "1.0"}]),
            ),
            (
                "pct null",
                json!([{"key": "5h", "label": "5h", "kind": "short", "pct": null}]),
            ),
            (
                "resetsAt as a string",
                json!([{"key": "5h", "label": "5h", "kind": "short", "pct": 1.0,
                        "resetsAt": "2030-01-01T00:00:00Z"}]),
            ),
            (
                "periodS as a fraction",
                json!([{"key": "5h", "label": "5h", "kind": "short", "pct": 1.0,
                        "periodS": 1.5}]),
            ),
        ];
        for (name, v) in cases {
            assert_eq!(windows_from_json(&v), None, "{name}");
        }
        // One bad item poisons the whole list; a good item alone is fine.
        assert_eq!(windows_from_json(&json!([good.clone(), 7])), None);
        assert!(windows_from_json(&json!([good])).is_some());
    }

    #[test]
    fn a_non_finite_pct_reads_as_no_reading() {
        // `1e999` is valid JSON that overflows an f64 (serde_json keeps the digits).
        let v: Value = serde_json::from_str(
            r#"[{"key": "5h", "label": "5h", "kind": "short", "pct": 1e999}]"#,
        )
        .unwrap();
        assert_eq!(windows_from_json(&v), None);
    }

    #[test]
    fn short_and_long_windows_are_always_relevant() {
        assert!(is_relevant(&win("5h", "5h", WindowKind::Short, 1.0), &[]));
        assert!(is_relevant(&win("7d", "7d", WindowKind::Long, 1.0), &[]));
        assert!(is_relevant(
            &win("7d", "7d", WindowKind::Long, 1.0),
            &models(&["Fable"])
        ));
    }

    #[test]
    fn a_spend_window_is_never_relevant() {
        let spend = win("spend", "spend", WindowKind::Spend, 99.0);
        assert!(!is_relevant(&spend, &[]));
        assert!(!is_relevant(&spend, &models(&["all"])));
        assert!(!is_relevant(&spend, &models(&["spend"])));
    }

    #[test]
    fn a_scoped_window_is_relevant_when_named_case_insensitively() {
        let fable = win("scoped:Fable", "Fable", WindowKind::Scoped, 10.0);
        assert!(!is_relevant(&fable, &[]));
        assert!(!is_relevant(&fable, &models(&["Opus"])));
        assert!(is_relevant(&fable, &models(&["Fable"])));
        assert!(is_relevant(&fable, &models(&["fable"])));
        assert!(is_relevant(&fable, &models(&["Opus", "FABLE"])));
    }

    #[test]
    fn all_matches_every_scoped_window_in_any_case() {
        let fable = win("scoped:Fable", "Fable", WindowKind::Scoped, 10.0);
        let opus = win("scoped:Opus", "Opus", WindowKind::Scoped, 10.0);
        for m in ["all", "All", "ALL"] {
            assert!(is_relevant(&fable, &models(&[m])), "{m}");
            assert!(is_relevant(&opus, &models(&[m])), "{m}");
        }
    }

    #[test]
    fn headroom_is_one_hundred_minus_the_highest_relevant_pct() {
        let windows = sample_windows();
        // 5h 9.0 and 7d 77.5 are relevant; Fable (0.0) is not named; spend never counts.
        assert_eq!(max_relevant_pct(&windows, &[]), Some(77.5));
        assert_eq!(headroom(&windows, &[]), Some(22.5));
    }

    #[test]
    fn a_named_scoped_window_can_set_the_headroom() {
        let mut windows = sample_windows();
        windows[2].pct = 91.0;
        assert_eq!(headroom(&windows, &[]), Some(22.5));
        assert_eq!(headroom(&windows, &models(&["fable"])), Some(9.0));
        assert_eq!(max_relevant_pct(&windows, &models(&["all"])), Some(91.0));
    }

    #[test]
    fn a_spend_window_never_sets_the_headroom() {
        let windows = vec![
            win("5h", "5h", WindowKind::Short, 10.0),
            win("spend", "spend", WindowKind::Spend, 100.0),
        ];
        assert_eq!(headroom(&windows, &models(&["all"])), Some(90.0));
    }

    #[test]
    fn headroom_is_unknown_when_no_window_is_relevant() {
        assert_eq!(headroom(&[], &[]), None);
        assert_eq!(max_relevant_pct(&[], &[]), None);
        let only_spend = vec![win("spend", "spend", WindowKind::Spend, 50.0)];
        assert_eq!(headroom(&only_spend, &models(&["all"])), None);
        let only_unnamed = vec![win("scoped:Fable", "Fable", WindowKind::Scoped, 50.0)];
        assert_eq!(headroom(&only_unnamed, &[]), None);
    }

    #[test]
    fn headroom_goes_negative_above_one_hundred() {
        let windows = vec![win("7d", "7d", WindowKind::Long, 104.0)];
        assert_eq!(headroom(&windows, &[]), Some(-4.0));
    }

    #[test]
    fn the_earliest_reset_ignores_spend_and_unnamed_scoped_windows() {
        let mut windows = sample_windows();
        // Spend resets earliest of all, and an unnamed scoped window earlier still.
        windows[3].resets_at = Some(1_000);
        windows[2].resets_at = Some(2_000);
        assert_eq!(
            earliest_relevant_reset(&windows, &[]),
            Some(1_893_000_000),
            "spend and the unnamed scoped window are ignored"
        );
        assert_eq!(
            earliest_relevant_reset(&windows, &models(&["Fable"])),
            Some(2_000)
        );
    }

    #[test]
    fn the_earliest_reset_skips_windows_without_one() {
        let windows = vec![
            win("5h", "5h", WindowKind::Short, 1.0),
            Window {
                resets_at: Some(500),
                ..win("7d", "7d", WindowKind::Long, 1.0)
            },
        ];
        assert_eq!(earliest_relevant_reset(&windows, &[]), Some(500));
        assert_eq!(earliest_relevant_reset(&[], &[]), None);
        assert_eq!(
            earliest_relevant_reset(&[win("5h", "5h", WindowKind::Short, 1.0)], &[]),
            None
        );
    }
}
```

In `crates/tagteam-core/Cargo.toml`, insert in `[dependencies]` after `hex.workspace = true`:
```toml
serde_json.workspace = true
```
In `crates/tagteam-core/src/lib.rs`, insert `pub mod usage;` between `pub mod rotation;` and
`pub mod validate;`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-core --lib usage::`
Expected: FAIL to compile: ``cannot find type `Window` in this scope`` (and likewise
`WindowKind`, `windows_to_json`, `headroom`, …).

- [ ] **Step 3: Write the implementation**

Insert the following at the top of `crates/tagteam-core/src/usage.rs`, above `#[cfg(test)]`
(leave one blank line between the two parts):
```rust
//! Generic usage windows (§4.5, §8.2): what a provider's usage response is normalized into,
//! how a window is stored in `usage_state.last_good`, and which windows count for decisions.
//!
//! Pure data and functions; no I/O. Times are epoch seconds.

use serde_json::{Map, Value};

/// What a window measures, which decides how the rest of the system treats it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowKind {
    /// A short rolling window (Claude Code's 5 h).
    Short,
    /// The long window that gates a week or a month (Claude Code's 7 d).
    Long,
    /// Money spent against a limit. Never relevant to a decision.
    Spend,
    /// A window scoped to one model or product (Claude Code's weekly per-model windows).
    Scoped,
}

impl WindowKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WindowKind::Short => "short",
            WindowKind::Long => "long",
            WindowKind::Spend => "spend",
            WindowKind::Scoped => "scoped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "short" => Some(WindowKind::Short),
            "long" => Some(WindowKind::Long),
            "spend" => Some(WindowKind::Spend),
            "scoped" => Some(WindowKind::Scoped),
            _ => None,
        }
    }

    /// Whether §8.7's average-pace fallback and `aheadOfPace` cover this kind (when the window
    /// also has a period).
    pub fn has_pace(self) -> bool {
        matches!(self, WindowKind::Long | WindowKind::Scoped)
    }
}

/// One usage window, in the provider-neutral form `usage_state.last_good` stores.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// The provider's window key. Claude Code: `5h`, `7d`, `spend`, `scoped:<name>`.
    pub key: String,
    /// What a person sees. Claude Code: `5h`, `7d`, `spend`, `<name>`.
    pub label: String,
    pub kind: WindowKind,
    /// Percent of the window used. Always finite; above 100 is kept (§8.2).
    pub pct: f64,
    /// When the window resets, epoch seconds.
    pub resets_at: Option<i64>,
    /// The window's length in seconds, when the provider states it.
    pub period_s: Option<i64>,
    /// Provider-owned extras (Claude Code's spend: `{used, limit, currency}`).
    pub detail: Option<Value>,
}

/// `last_good`'s stored form: `[{"key","label","kind","pct","resetsAt"?,"periodS"?,"detail"?}]`.
pub fn windows_to_json(windows: &[Window]) -> Value {
    Value::Array(
        windows
            .iter()
            .map(|w| {
                let mut o = Map::new();
                o.insert("key".into(), Value::from(w.key.as_str()));
                o.insert("label".into(), Value::from(w.label.as_str()));
                o.insert("kind".into(), Value::from(w.kind.as_str()));
                o.insert("pct".into(), Value::from(w.pct));
                if let Some(r) = w.resets_at {
                    o.insert("resetsAt".into(), Value::from(r));
                }
                if let Some(p) = w.period_s {
                    o.insert("periodS".into(), Value::from(p));
                }
                if let Some(d) = &w.detail {
                    o.insert("detail".into(), d.clone());
                }
                Value::Object(o)
            })
            .collect(),
    )
}

/// The inverse of [`windows_to_json`]. `None` when the value is not that shape, or any `pct` is
/// not finite: a corrupt row reads as "no reading", never as an error.
pub fn windows_from_json(v: &Value) -> Option<Vec<Window>> {
    v.as_array()?.iter().map(window_from_json).collect()
}

fn window_from_json(v: &Value) -> Option<Window> {
    let o = v.as_object()?;
    let pct = o.get("pct")?.as_f64().filter(|p| p.is_finite())?;
    Some(Window {
        key: o.get("key")?.as_str()?.to_owned(),
        label: o.get("label")?.as_str()?.to_owned(),
        kind: WindowKind::parse(o.get("kind")?.as_str()?)?,
        pct,
        resets_at: optional_i64(o, "resetsAt")?,
        period_s: optional_i64(o, "periodS")?,
        detail: o.get("detail").filter(|d| !d.is_null()).cloned(),
    })
}

/// `Some(None)`: absent or null. `Some(Some(n))`: an integer. `None`: present but not one.
fn optional_i64(o: &Map<String, Value>, key: &str) -> Option<Option<i64>> {
    match o.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(v) => v.as_i64().map(Some),
    }
}

/// §8.2 relevance: `Short` and `Long` always; `Scoped` when `models` names it (case-insensitive)
/// or contains `all`; `Spend` never.
pub fn is_relevant(w: &Window, models: &[String]) -> bool {
    match w.kind {
        WindowKind::Short | WindowKind::Long => true,
        WindowKind::Spend => false,
        WindowKind::Scoped => {
            let label = w.label.to_lowercase();
            models.iter().any(|m| {
                let m = m.to_lowercase();
                m == "all" || m == label
            })
        }
    }
}

fn relevant<'a>(
    windows: &'a [Window],
    models: &'a [String],
) -> impl Iterator<Item = &'a Window> + 'a {
    windows.iter().filter(move |w| is_relevant(w, models))
}

/// `max(relevant pct)`; `None` when no window is relevant.
pub fn max_relevant_pct(windows: &[Window], models: &[String]) -> Option<f64> {
    relevant(windows, models).map(|w| w.pct).reduce(f64::max)
}

/// §8.2: `100 − max(relevant pct)`. `None` is unknown headroom, which is never auto-skipped.
/// Zero or negative means at the limit.
pub fn headroom(windows: &[Window], models: &[String]) -> Option<f64> {
    max_relevant_pct(windows, models).map(|p| 100.0 - p)
}

/// The earliest `resets_at` among relevant windows, if any has one.
pub fn earliest_relevant_reset(windows: &[Window], models: &[String]) -> Option<i64> {
    relevant(windows, models).filter_map(|w| w.resets_at).min()
}
```

In `crates/tagteam-core/src/lib.rs`, add the crate-root re-export (after the other
`pub use` lines; `cargo fmt` orders them):
```rust
pub use usage::{Window, WindowKind};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-core --lib usage::`
Expected: PASS, 21 tests, including `the_stored_form_is_exactly_the_documented_one`,
`a_corrupt_value_reads_as_no_reading` and `headroom_goes_negative_above_one_hundred`.

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

```
git add Cargo.lock crates/tagteam-core/Cargo.toml crates/tagteam-core/src/lib.rs \
  crates/tagteam-core/src/usage.rs
git commit -m "Add generic usage windows with relevance and headroom"
```

---

### Task 3: Failure backoff, `Retry-After`, trust (core)

**Files:**
- Create: `crates/tagteam-core/src/backoff.rs`, `crates/tagteam-core/src/trust.rs`
- Modify: `crates/tagteam-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks (trust takes plain integers, not `Window`s).
- Produces:
```rust
// tagteam_core::backoff
pub fn parse_retry_after(value: &str) -> Option<f64>;
pub fn failure_backoff_s(consecutive_failures: u32, is_429: bool, retry_after_s: Option<f64>) -> i64;

// tagteam_core::trust (TrustInputs is also re-exported at the crate root)
pub const STALE_OK_S: i64 = 300;
pub const TRUST_MAX_AGE_S: i64 = 3600;
pub const POST_429_TRUST_CAP_S: i64 = 7200;
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrustInputs {
    pub now_s: i64,
    pub fetched_at: Option<i64>,
    pub consecutive_failures: u32,
    pub plan_in_force: bool,                   // next_poll_at > now_s
    pub live_lease: bool,
    pub last_429_at: Option<i64>,              // Decision 2: when the 429's backoff lifts
    pub earliest_relevant_reset: Option<i64>,
}
pub fn decision_grade(t: &TrustInputs) -> bool;
```

The numbers are §8.5 (backoff) and §8.4 (trust), verbatim. This task carries Review Focus 3: a
hostile or odd `Retry-After` (`0`, `1e400`, `inf`, `-5`, an HTTP date, empty, `12.5`) must never
store a non-finite or oversized wait. `parse_retry_after` keeps a huge or infinite value and
leaves the clamping to `failure_backoff_s`, so the test
`hostile_retry_after_values_never_store_a_non_finite_or_oversized_wait` walks both together.

Judgement calls, flagged rather than decided silently:
- A 429 with **no** `Retry-After` uses the base alone (30 s for the first). §8.5 only adds the
  300 s minimum for `Retry-After: 0`; the 360 s post-429 minimum is the poll policy's (Task 4).
- `Retry-After: 0` on a non-429 status has no 300 s minimum (§8.5 names 429s only).
- A fractional `Retry-After` rounds up to whole seconds.
- A negative or NaN value passed straight to `failure_backoff_s` is ignored like `None`
  (the parser never produces one).
- Trust at exactly age 300 s and 3600 s is inclusive (§8.4 says "≤"); the post-429 bound is
  inclusive too.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-core/src/backoff.rs` holding only the test module:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_accepts_the_seconds_form() {
        assert_eq!(parse_retry_after("0"), Some(0.0));
        assert_eq!(parse_retry_after("30"), Some(30.0));
        assert_eq!(parse_retry_after("12.5"), Some(12.5));
        assert_eq!(parse_retry_after(" 45 "), Some(45.0));
    }

    #[test]
    fn retry_after_keeps_huge_and_infinite_values_for_the_caller_to_clamp() {
        assert_eq!(parse_retry_after("1e400"), Some(f64::INFINITY));
        assert_eq!(parse_retry_after("inf"), Some(f64::INFINITY));
        assert_eq!(parse_retry_after("1e300"), Some(1e300));
    }

    #[test]
    fn retry_after_rejects_everything_else() {
        for s in [
            "",
            "   ",
            "-5",
            "-0",
            "nan",
            "Wed, 21 Oct 2015 07:28:00 GMT",
            "soon",
            "0x10",
            "5 seconds",
        ] {
            assert_eq!(parse_retry_after(s), None, "{s:?}");
        }
    }

    #[test]
    fn the_base_doubles_from_thirty_seconds_up_to_six_hundred() {
        for (n, expected) in [
            (1, 30),
            (2, 60),
            (3, 120),
            (4, 240),
            (5, 480),
            (6, 600),
            (7, 600),
        ] {
            assert_eq!(failure_backoff_s(n, false, None), expected, "n = {n}");
        }
    }

    #[test]
    fn the_exponent_is_clamped_so_huge_counts_do_not_overflow() {
        assert_eq!(failure_backoff_s(33, false, None), 600);
        assert_eq!(failure_backoff_s(1_000, false, None), 600);
        assert_eq!(failure_backoff_s(u32::MAX, true, None), 600);
    }

    #[test]
    fn a_zero_count_is_treated_as_the_first_failure() {
        assert_eq!(failure_backoff_s(0, false, None), 30);
    }

    #[test]
    fn a_429_without_retry_after_uses_the_base() {
        assert_eq!(failure_backoff_s(1, true, None), 30);
        assert_eq!(failure_backoff_s(3, true, None), 120);
    }

    #[test]
    fn a_429_with_retry_after_zero_waits_at_least_three_hundred_seconds() {
        assert_eq!(failure_backoff_s(1, true, Some(0.0)), 300);
        assert_eq!(failure_backoff_s(5, true, Some(0.0)), 480);
        assert_eq!(failure_backoff_s(6, true, Some(0.0)), 600);
    }

    #[test]
    fn retry_after_zero_on_another_status_has_no_minimum() {
        assert_eq!(failure_backoff_s(1, false, Some(0.0)), 30);
    }

    #[test]
    fn a_429_honours_a_retry_after_up_to_six_hundred_seconds_as_asked() {
        assert_eq!(failure_backoff_s(1, true, Some(120.0)), 120);
        assert_eq!(failure_backoff_s(1, true, Some(600.0)), 600);
    }

    #[test]
    fn a_429_asking_for_more_than_six_hundred_adds_nine_hundred_of_margin() {
        assert_eq!(failure_backoff_s(1, true, Some(601.0)), 1501);
        assert_eq!(failure_backoff_s(1, true, Some(1000.0)), 1900);
        assert_eq!(failure_backoff_s(1, true, Some(3000.0)), 3900);
    }

    #[test]
    fn a_429_is_capped_at_forty_five_hundred_seconds() {
        assert_eq!(
            failure_backoff_s(1, true, Some(4000.0)),
            4500,
            "4900 capped"
        );
        assert_eq!(failure_backoff_s(1, true, Some(1e300)), 4500);
    }

    #[test]
    fn other_statuses_are_capped_at_thirty_six_hundred_seconds() {
        assert_eq!(failure_backoff_s(1, false, Some(45.0)), 45);
        assert_eq!(failure_backoff_s(1, false, Some(3600.0)), 3600);
        assert_eq!(failure_backoff_s(1, false, Some(7200.0)), 3600);
        assert_eq!(
            failure_backoff_s(1, false, Some(700.0)),
            700,
            "no margin off a 429"
        );
    }

    #[test]
    fn a_fractional_retry_after_rounds_up_to_whole_seconds() {
        assert_eq!(failure_backoff_s(1, false, Some(50.5)), 51);
        assert_eq!(
            failure_backoff_s(1, true, Some(12.5)),
            30,
            "the base is larger"
        );
    }

    #[test]
    fn a_retry_after_shorter_than_the_base_loses_to_the_base() {
        assert_eq!(failure_backoff_s(4, false, Some(10.0)), 240);
        assert_eq!(failure_backoff_s(7, true, Some(100.0)), 600);
    }

    #[test]
    fn a_non_finite_retry_after_is_clamped_to_the_cap() {
        assert_eq!(failure_backoff_s(1, true, Some(f64::INFINITY)), 4500);
        assert_eq!(failure_backoff_s(1, false, Some(f64::INFINITY)), 3600);
    }

    #[test]
    fn a_negative_or_nan_retry_after_is_ignored() {
        assert_eq!(failure_backoff_s(1, true, Some(-5.0)), 30);
        assert_eq!(failure_backoff_s(1, true, Some(f64::NAN)), 30);
        assert_eq!(failure_backoff_s(2, false, Some(-1.0)), 60);
    }

    #[test]
    fn hostile_retry_after_values_never_store_a_non_finite_or_oversized_wait() {
        // Review Focus 3: each header value, parsed then turned into a stored backoff.
        for (header, expected_429, expected_other) in [
            ("0", 300, 30),
            ("1e400", 4500, 3600),
            ("inf", 4500, 3600),
            ("-5", 30, 30),
            ("Wed, 21 Oct 2015 07:28:00 GMT", 30, 30),
            ("", 30, 30),
            ("12.5", 30, 30),
        ] {
            let parsed = parse_retry_after(header);
            assert_eq!(
                failure_backoff_s(1, true, parsed),
                expected_429,
                "429 with {header:?}"
            );
            assert_eq!(
                failure_backoff_s(1, false, parsed),
                expected_other,
                "other with {header:?}"
            );
        }
    }
}
```

Create `crates/tagteam-core/src/trust.rs` holding only the test module:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    const FETCHED: i64 = 1_000_000;

    /// A reading `age_s` old, with nothing that extends trust.
    fn at_age(age_s: i64) -> TrustInputs {
        TrustInputs {
            now_s: FETCHED + age_s,
            fetched_at: Some(FETCHED),
            consecutive_failures: 0,
            plan_in_force: false,
            live_lease: false,
            last_429_at: None,
            earliest_relevant_reset: None,
        }
    }

    #[test]
    fn the_constants_are_the_specs() {
        assert_eq!(STALE_OK_S, 300);
        assert_eq!(TRUST_MAX_AGE_S, 3600);
        assert_eq!(POST_429_TRUST_CAP_S, 7200);
    }

    #[test]
    fn a_reading_never_taken_is_not_trusted() {
        let t = TrustInputs {
            fetched_at: None,
            plan_in_force: true,
            live_lease: true,
            consecutive_failures: 3,
            ..at_age(0)
        };
        assert!(!decision_grade(&t));
    }

    #[test]
    fn a_reading_up_to_three_hundred_seconds_old_is_trusted() {
        assert!(decision_grade(&at_age(0)));
        assert!(decision_grade(&at_age(300)));
        assert!(!decision_grade(&at_age(301)));
    }

    #[test]
    fn a_reading_from_the_future_counts_as_fresh() {
        assert!(decision_grade(&at_age(-50)));
    }

    #[test]
    fn retried_failures_extend_trust_to_an_hour() {
        let t = |age| TrustInputs {
            consecutive_failures: 1,
            ..at_age(age)
        };
        assert!(decision_grade(&t(301)));
        assert!(decision_grade(&t(3600)));
        assert!(!decision_grade(&t(3601)));
    }

    #[test]
    fn a_plan_in_force_extends_trust_to_an_hour() {
        let t = |age| TrustInputs {
            plan_in_force: true,
            ..at_age(age)
        };
        assert!(decision_grade(&t(3600)));
        assert!(!decision_grade(&t(3601)));
    }

    #[test]
    fn a_live_lease_extends_trust_to_an_hour() {
        let t = |age| TrustInputs {
            live_lease: true,
            ..at_age(age)
        };
        assert!(decision_grade(&t(3600)));
        assert!(!decision_grade(&t(3601)));
    }

    #[test]
    fn a_stale_reading_with_nothing_extending_it_is_not_trusted() {
        assert!(!decision_grade(&at_age(1800)));
    }

    #[test]
    fn after_a_429_trust_lasts_until_the_earliest_relevant_reset() {
        let t = |age| TrustInputs {
            last_429_at: Some(FETCHED + 10),
            earliest_relevant_reset: Some(FETCHED + 5000),
            ..at_age(age)
        };
        assert!(decision_grade(&t(4000)));
        assert!(decision_grade(&t(5000)), "trusted until the reset itself");
        assert!(!decision_grade(&t(5001)));
    }

    #[test]
    fn after_a_429_the_reset_cannot_extend_trust_past_seven_thousand_two_hundred_seconds() {
        let t = |age| TrustInputs {
            last_429_at: Some(FETCHED + 10),
            earliest_relevant_reset: Some(FETCHED + 10_000),
            ..at_age(age)
        };
        assert!(decision_grade(&t(7200)));
        assert!(!decision_grade(&t(7201)));
    }

    #[test]
    fn after_a_429_with_no_known_reset_only_the_cap_applies() {
        let t = |age| TrustInputs {
            last_429_at: Some(FETCHED + 10),
            ..at_age(age)
        };
        assert!(decision_grade(&t(7200)));
        assert!(!decision_grade(&t(7201)));
    }

    #[test]
    fn a_429_that_predates_the_reading_does_not_extend_trust() {
        for last_429_at in [FETCHED - 100, FETCHED] {
            let t = TrustInputs {
                last_429_at: Some(last_429_at),
                earliest_relevant_reset: Some(FETCHED + 5000),
                ..at_age(4000)
            };
            assert!(!decision_grade(&t), "last_429_at = {last_429_at}");
        }
    }

    #[test]
    fn a_reset_already_behind_the_reading_leaves_no_post_429_trust() {
        let t = TrustInputs {
            last_429_at: Some(FETCHED + 10),
            earliest_relevant_reset: Some(FETCHED - 1),
            ..at_age(4000)
        };
        assert!(!decision_grade(&t));
    }

    #[test]
    fn the_post_429_rule_never_shortens_the_other_extensions() {
        let t = TrustInputs {
            last_429_at: Some(FETCHED + 10),
            earliest_relevant_reset: Some(FETCHED + 1000),
            consecutive_failures: 2,
            ..at_age(3000)
        };
        assert!(decision_grade(&t), "failures still extend to an hour");
    }
}
```

In `crates/tagteam-core/src/lib.rs`, insert `pub mod backoff;` before `pub mod classify;` and
`pub mod trust;` between `pub mod rotation;` and `pub mod usage;`.

- [ ] **Step 2: Run them to verify they fail**

Run:
```
cargo test -p tagteam-core --lib backoff::
cargo test -p tagteam-core --lib trust::
```
Expected: FAIL to compile: ``cannot find function `parse_retry_after` in this scope``
(and likewise `failure_backoff_s`, `TrustInputs` and `decision_grade`).

- [ ] **Step 3: Write `backoff.rs`**

Insert the following at the top of `crates/tagteam-core/src/backoff.rs`, above `#[cfg(test)]`
(leave one blank line between the two parts):
```rust
//! Failure backoff (§8.5) and `Retry-After` parsing (§8.1). Pure; nothing here touches a clock.

/// `min(asked, …)` ceilings (§8.5).
const CAP_429_S: f64 = 4500.0;
const CAP_OTHER_S: f64 = 3600.0;
/// `min(30 · 2^(n−1), 600)`, with the exponent clamped at 32.
const BASE_S: i64 = 30;
const BASE_MAX_S: i64 = 600;
const MAX_EXPONENT: u32 = 32;
/// A 429 that says `Retry-After: 0` still waits this long.
const ZERO_RETRY_AFTER_FLOOR_S: f64 = 300.0;
/// A 429 that asks for more than this gets [`LONG_RETRY_AFTER_MARGIN_S`] on top, because
/// retrying at the deadline re-arms the block.
const LONG_RETRY_AFTER_S: f64 = 600.0;
const LONG_RETRY_AFTER_MARGIN_S: f64 = 900.0;

/// `Retry-After` in its seconds form only (§8.1): an integer or decimal, possibly huge or `inf`.
/// Negative, empty, unparseable and HTTP-date values give `None`.
///
/// A value that overflows to infinity (`1e400`) is returned as infinity; the caller's
/// [`failure_backoff_s`] clamps it, so it is never stored.
pub fn parse_retry_after(value: &str) -> Option<f64> {
    let s = value.trim();
    if s.is_empty() || s.starts_with('-') {
        return None;
    }
    let seconds: f64 = s.parse().ok()?;
    (!seconds.is_nan()).then_some(seconds)
}

/// §8.5. `consecutive_failures` counts this failure (≥ 1; 0 is treated as 1). A non-finite
/// `retry_after_s` is clamped to the cap; a negative or NaN one is ignored. Returns whole
/// seconds, never negative.
pub fn failure_backoff_s(
    consecutive_failures: u32,
    is_429: bool,
    retry_after_s: Option<f64>,
) -> i64 {
    let exponent = consecutive_failures.saturating_sub(1).min(MAX_EXPONENT);
    let computed = (BASE_S << exponent).min(BASE_MAX_S);
    let cap = if is_429 { CAP_429_S } else { CAP_OTHER_S };

    let asked = match retry_after_s {
        Some(r) if r.is_nan() || r < 0.0 => 0.0,
        Some(r) if !r.is_finite() => cap,
        Some(r) if is_429 && r == 0.0 => ZERO_RETRY_AFTER_FLOOR_S,
        Some(r) if is_429 && r > LONG_RETRY_AFTER_S => r + LONG_RETRY_AFTER_MARGIN_S,
        Some(r) => r,
        None => 0.0,
    };
    let asked = asked.ceil().min(cap) as i64;
    asked.max(computed)
}
```

- [ ] **Step 4: Write `trust.rs`**

Insert the following at the top of `crates/tagteam-core/src/trust.rs`, above `#[cfg(test)]`
(one blank line between the two parts), and add `pub use trust::TrustInputs;` to the
`pub use` lines in `crates/tagteam-core/src/lib.rs`:
```rust
//! Decision-grade trust (§8.4): whether a stored reading may drive a decision or be shown as
//! current usage. Pure; `now_s` is passed in.

/// A reading this young is always trusted.
pub const STALE_OK_S: i64 = 300;
/// Trust extends to this age while failures are being retried, a scheduled plan is in force, or
/// a live lease exists.
pub const TRUST_MAX_AGE_S: i64 = 3600;
/// After a 429, `last_good` is trusted until the earliest relevant reset, but never past
/// `fetched_at` plus this.
pub const POST_429_TRUST_CAP_S: i64 = 7200;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrustInputs {
    pub now_s: i64,
    /// When the reading was taken; `None` when the account has never been read.
    pub fetched_at: Option<i64>,
    pub consecutive_failures: u32,
    /// `next_poll_at > now_s`: a scheduled plan is in force.
    pub plan_in_force: bool,
    /// Another process holds `usage:<id>` right now.
    pub live_lease: bool,
    /// When the last 429's backoff lifts (Decision 2), not when it arrived.
    pub last_429_at: Option<i64>,
    pub earliest_relevant_reset: Option<i64>,
}

/// §8.4: whether the reading may drive a decision (and be shown as `usage`, §13.2).
///
/// - Age ≤ 300 s: trusted. A reading stamped in the future (clock skew) counts as fresh.
/// - Age ≤ 3600 s: trusted while failures are retried, a plan is in force, or a lease is live.
/// - After a 429 (`last_429_at > fetched_at`): trusted until the earliest relevant reset, capped
///   at `fetched_at + 7200 s`, because usage only rises within a window and the old reading is a
///   valid lower bound. With no known reset the cap alone applies.
pub fn decision_grade(t: &TrustInputs) -> bool {
    let Some(fetched_at) = t.fetched_at else {
        return false;
    };
    let age = t.now_s.saturating_sub(fetched_at);
    if age <= STALE_OK_S {
        return true;
    }
    let extended = t.consecutive_failures > 0 || t.plan_in_force || t.live_lease;
    if extended && age <= TRUST_MAX_AGE_S {
        return true;
    }
    if t.last_429_at.is_some_and(|at| at > fetched_at) {
        let cap = fetched_at.saturating_add(POST_429_TRUST_CAP_S);
        let until = t
            .earliest_relevant_reset
            .map_or(cap, |reset| reset.min(cap));
        return t.now_s <= until;
    }
    false
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run:
```
cargo test -p tagteam-core --lib backoff::
cargo test -p tagteam-core --lib trust::
```
Expected: PASS, 18 and 14 tests, including
`hostile_retry_after_values_never_store_a_non_finite_or_oversized_wait`,
`after_a_429_the_reset_cannot_extend_trust_past_seven_thousand_two_hundred_seconds` and
`a_429_that_predates_the_reading_does_not_extend_trust`.

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

```
git add crates/tagteam-core/src/lib.rs crates/tagteam-core/src/backoff.rs \
  crates/tagteam-core/src/trust.rs
git commit -m "Add failure backoff, Retry-After parsing and decision-grade trust"
```

---

### Task 4: Poll budget and `plan_after_fetch` (core)

**Files:**
- Create: `crates/tagteam-core/src/poll.rs`
- Modify: `crates/tagteam-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces (`tagteam_core::poll`; the three types are also re-exported at the crate root):
```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PollBudget {
    pub hourly_requests: u32,        // 20
    pub slot_valid_s: i64,           // 60
    pub count_window_s: i64,         // 3660
    pub floor_s: i64,                // 180 (also the serve TTL)
    pub urgent_s: i64,               // 60
    pub active_max_s: i64,           // 300
    pub candidate_default_s: i64,    // 300
    pub candidate_max_s: i64,        // 600
    pub exhausted_s: i64,            // 600
    pub movement_delta: f64,         // 1.0
    pub jitter_frac: f64,            // 0.10
    pub post_429_min_s: i64,         // 360
    pub recent_429_window_s: i64,    // 3600
    pub post_429_mult: f64,          // 1.5
    pub post_429_max_s: i64,         // 1800
    pub escalation_margin: f64,      // 15.0
    pub reset_slack_s: i64,          // 60
}
impl PollBudget { pub const STANDARD: PollBudget; }

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PollInputs {
    pub now_s: i64,
    pub active: bool,
    pub pct: Option<f64>,
    pub prev_pct: Option<f64>,
    pub prev_interval_s: Option<i64>,
    pub threshold: f64,
    pub last_429_at: Option<i64>,
    pub next_relevant_reset: Option<i64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollPlan { pub interval_s: i64, pub next_poll_at: i64 }

pub fn plan_after_fetch(b: &PollBudget, i: &PollInputs, jitter: f64) -> PollPlan;
pub fn replan_for_role(b: &PollBudget, active: bool, now_s: i64, jitter: f64) -> PollPlan;
pub fn budget_next_free(b: &PollBudget, counted_at: &[i64], now_s: i64) -> Option<i64>;
```

§8.6 is the whole specification: the constants table, the `plan_after_fetch` rules and their
order (unknown pct, movement, urgent, recent 429, exhausted, then jitter and the clamps), and
Decision 9's clamp `next_poll_at = max(now + floor, min(now + interval, reset + 60))`. Every
branch has a test. Time and jitter are arguments, so nothing here reads a clock or a random
source.

Judgement calls, flagged rather than decided silently:
- **Movement is absolute.** `|pct − prev_pct| ≥ 1` counts, so a window reset (a drop) polls
  soon after, as a rise does. §8.6 says "movement ≥ 1 point" without a sign.
- **The recent-429 rule applies with an unknown pct too.** It protects the endpoint, not the
  reading; skipping it for a reading that failed to parse would poll a rate-limited identity
  faster than one that parsed.
- **`interval_s` is the policy interval before jitter and clamping.** It is what the next call
  receives as `prev_interval_s`, so jitter and a nearby reset do not compound into the
  schedule. `next_poll_at` carries the jitter and the clamps.
- **The active account's default interval is the floor (180 s)**, matching §8.6's "Unknown
  pct" line; `active_max_s` (300 s) is only the no-movement ceiling.
- **The budget window is strict.** A `usage_requests` row counts while `now − at <
  count_window_s`, so it leaves the hour exactly 3660 s after it was reserved, and
  `budget_next_free` returns that moment. Task 8's SQL should count `at > now − 3660`; a store
  that counts `at >= now − 3660` differs from this only in the one second a row sits exactly on
  the edge, where the caller reschedules once more. A row stamped in the future (clock skew)
  counts.
- **`budget_next_free` with more than 20 counted rows** returns when enough of them leave for
  a slot to be free (the `count − 20`th oldest leaving), not just the oldest. The store never
  lets the count exceed 20, so this only matters after a clock jump.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-core/src/poll.rs` holding only the test module:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    const B: PollBudget = PollBudget::STANDARD;
    const NOW: i64 = 1_000_000;

    /// A candidate account that has not moved: the baseline each test adjusts.
    fn inputs() -> PollInputs {
        PollInputs {
            now_s: NOW,
            active: false,
            pct: Some(50.0),
            prev_pct: Some(50.0),
            prev_interval_s: Some(300),
            threshold: 90.0,
            last_429_at: None,
            next_relevant_reset: None,
        }
    }

    fn active() -> PollInputs {
        PollInputs {
            active: true,
            ..inputs()
        }
    }

    /// The plan's interval and the seconds until the next poll, with no jitter.
    fn plan(i: PollInputs) -> (i64, i64) {
        let p = plan_after_fetch(&B, &i, 0.0);
        (p.interval_s, p.next_poll_at - NOW)
    }

    #[test]
    fn the_standard_budget_is_the_specs_table() {
        assert_eq!(
            B,
            PollBudget {
                hourly_requests: 20,
                slot_valid_s: 60,
                count_window_s: 3660,
                floor_s: 180,
                urgent_s: 60,
                active_max_s: 300,
                candidate_default_s: 300,
                candidate_max_s: 600,
                exhausted_s: 600,
                movement_delta: 1.0,
                jitter_frac: 0.10,
                post_429_min_s: 360,
                recent_429_window_s: 3600,
                post_429_mult: 1.5,
                post_429_max_s: 1800,
                escalation_margin: 15.0,
                reset_slack_s: 60,
            }
        );
    }

    #[test]
    fn an_unknown_pct_uses_the_roles_default() {
        let unknown = |i: PollInputs| PollInputs {
            pct: None,
            prev_pct: None,
            ..i
        };
        assert_eq!(plan(unknown(active())), (180, 180));
        assert_eq!(plan(unknown(inputs())), (300, 300));
        // The previous interval is not consulted.
        let with_prev = PollInputs {
            prev_interval_s: Some(600),
            ..unknown(inputs())
        };
        assert_eq!(plan(with_prev), (300, 300));
    }

    #[test]
    fn movement_of_a_point_halves_the_interval_down_to_the_floor() {
        let moved = |pct| PollInputs {
            pct: Some(pct),
            prev_pct: Some(50.0),
            ..inputs()
        };
        assert_eq!(plan(moved(52.0)), (180, 180), "max(180, 300/2)");
        assert_eq!(plan(moved(51.0)), (180, 180), "exactly one point moves");
        let slow = PollInputs {
            prev_interval_s: Some(600),
            ..moved(51.0)
        };
        assert_eq!(plan(slow), (300, 300), "max(180, 600/2)");
    }

    #[test]
    fn less_than_a_point_is_not_movement() {
        let i = PollInputs {
            pct: Some(50.9),
            ..inputs()
        };
        assert_eq!(plan(i), (450, 450), "no movement: min(600, max(180, 450))");
    }

    #[test]
    fn a_drop_of_a_point_or_more_counts_as_movement() {
        let i = PollInputs {
            pct: Some(10.0),
            prev_pct: Some(50.0),
            ..inputs()
        };
        assert_eq!(plan(i), (180, 180));
    }

    #[test]
    fn without_movement_the_interval_grows_by_half_up_to_the_ceiling() {
        let cand = |prev| PollInputs {
            prev_interval_s: prev,
            ..inputs()
        };
        assert_eq!(plan(cand(Some(300))), (450, 450));
        assert_eq!(plan(cand(Some(450))), (600, 600), "675 capped at 600");
        assert_eq!(plan(cand(None)), (450, 450), "base is the default, 300");
        let act = |prev| PollInputs {
            prev_interval_s: prev,
            ..active()
        };
        assert_eq!(plan(act(Some(180))), (270, 270));
        assert_eq!(plan(act(Some(270))), (300, 300), "405 capped at 300");
        assert_eq!(plan(act(None)), (270, 270), "base is the default, 180");
    }

    #[test]
    fn the_interval_never_falls_below_the_floor() {
        let i = PollInputs {
            prev_interval_s: Some(100),
            ..active()
        };
        assert_eq!(plan(i), (180, 180), "max(180, 150), below the 300 ceiling");
        let moving = PollInputs {
            pct: Some(52.0),
            prev_interval_s: Some(100),
            ..inputs()
        };
        assert_eq!(plan(moving), (180, 180), "max(180, 50)");
    }

    #[test]
    fn a_first_reading_has_no_movement_to_measure() {
        let i = PollInputs {
            prev_pct: None,
            ..inputs()
        };
        assert_eq!(plan(i), (450, 450));
    }

    #[test]
    fn an_active_account_moving_within_fifteen_points_of_the_threshold_is_urgent() {
        let near = |pct, prev| PollInputs {
            pct: Some(pct),
            prev_pct: Some(prev),
            ..active()
        };
        assert_eq!(plan(near(76.0, 74.0)), (60, 60));
        assert_eq!(
            plan(near(75.0, 73.0)),
            (60, 60),
            "threshold − 15 is inclusive"
        );
        assert_eq!(
            plan(near(74.9, 73.5)),
            (180, 180),
            "just out of reach: max(180, 150)"
        );
    }

    #[test]
    fn urgency_needs_movement_an_active_account_and_no_recent_429() {
        let still = PollInputs {
            pct: Some(80.0),
            prev_pct: Some(80.0),
            prev_interval_s: Some(180),
            ..active()
        };
        assert_eq!(plan(still), (270, 270), "not moving");
        let candidate = PollInputs {
            pct: Some(80.0),
            prev_pct: Some(78.0),
            ..inputs()
        };
        assert_eq!(plan(candidate), (180, 180), "a candidate is never urgent");
        let after_429 = PollInputs {
            pct: Some(80.0),
            prev_pct: Some(78.0),
            last_429_at: Some(NOW - 100),
            ..active()
        };
        assert_eq!(plan(after_429), (450, 450), "max(180, max(450, 360))");
    }

    #[test]
    fn urgency_follows_the_threshold_setting() {
        let i = PollInputs {
            pct: Some(40.0),
            prev_pct: Some(38.0),
            threshold: 50.0,
            ..active()
        };
        assert_eq!(plan(i), (60, 60), "40 ≥ 50 − 15");
    }

    #[test]
    fn an_urgent_poll_may_land_below_the_normal_floor() {
        let i = PollInputs {
            pct: Some(80.0),
            prev_pct: Some(78.0),
            ..active()
        };
        let next = |jitter| plan_after_fetch(&B, &i, jitter).next_poll_at - NOW;
        assert_eq!(next(1.0), 66);
        assert_eq!(next(-1.0), 60, "54 is clamped to the urgent floor");
    }

    #[test]
    fn after_a_recent_429_the_interval_grows_to_at_least_the_post_429_minimum() {
        let after = |i: PollInputs| PollInputs {
            last_429_at: Some(NOW - 100),
            ..i
        };
        let prev180 = PollInputs {
            prev_interval_s: Some(180),
            ..active()
        };
        assert_eq!(plan(after(prev180)), (360, 360), "max(270, max(270, 360))");
        assert_eq!(
            plan(after(active())),
            (450, 450),
            "prev 300: max(450, max(450, 360))"
        );
        let prev600 = PollInputs {
            prev_interval_s: Some(600),
            ..inputs()
        };
        assert_eq!(plan(after(prev600)), (900, 900), "max(600, max(900, 360))");
    }

    #[test]
    fn the_post_429_interval_is_capped_at_eighteen_hundred_seconds() {
        let i = PollInputs {
            prev_interval_s: Some(1500),
            last_429_at: Some(NOW - 10),
            ..inputs()
        };
        assert_eq!(plan(i), (1800, 1800));
    }

    #[test]
    fn a_429_counts_as_recent_until_an_hour_after_its_backoff_lifts() {
        let with = |at| PollInputs {
            last_429_at: Some(at),
            prev_interval_s: Some(600),
            ..inputs()
        };
        assert_eq!(plan(with(NOW + 200)).0, 900, "not lifted yet");
        assert_eq!(plan(with(NOW)).0, 900, "just lifted");
        assert_eq!(plan(with(NOW - 3600)).0, 900, "exactly an hour ago");
        assert_eq!(plan(with(NOW - 3601)).0, 600, "no longer recent");
    }

    #[test]
    fn a_recent_429_applies_even_when_the_pct_is_unknown() {
        let i = PollInputs {
            pct: None,
            prev_pct: None,
            last_429_at: Some(NOW - 10),
            ..inputs()
        };
        assert_eq!(plan(i), (450, 450), "max(300, max(450, 360))");
    }

    #[test]
    fn an_exhausted_window_polls_at_most_every_six_hundred_seconds() {
        let at_limit = |i: PollInputs, pct| PollInputs {
            pct: Some(pct),
            prev_pct: Some(pct),
            ..i
        };
        assert_eq!(
            plan(at_limit(inputs(), 100.0)),
            (600, 600),
            "450 raised to 600"
        );
        assert_eq!(
            plan(at_limit(active(), 100.0)),
            (600, 600),
            "270 raised to 600"
        );
        assert_eq!(plan(at_limit(inputs(), 105.0)), (600, 600));
        let just_below = at_limit(inputs(), 99.9);
        assert_eq!(plan(just_below), (450, 450));
    }

    #[test]
    fn an_exhausted_window_that_just_moved_is_not_urgent_for_long() {
        let i = PollInputs {
            pct: Some(100.0),
            prev_pct: Some(95.0),
            ..active()
        };
        assert_eq!(plan(i), (600, 600), "urgent 60, then raised to 600");
    }

    #[test]
    fn exhaustion_does_not_shorten_a_longer_post_429_interval() {
        let i = PollInputs {
            pct: Some(100.0),
            prev_pct: Some(100.0),
            prev_interval_s: Some(600),
            last_429_at: Some(NOW - 10),
            ..inputs()
        };
        assert_eq!(plan(i), (900, 900));
    }

    #[test]
    fn jitter_scales_the_interval_by_up_to_ten_percent() {
        let i = PollInputs {
            pct: None,
            prev_pct: None,
            ..inputs()
        };
        let next = |jitter| plan_after_fetch(&B, &i, jitter);
        assert_eq!(next(1.0).next_poll_at - NOW, 330);
        assert_eq!(next(-1.0).next_poll_at - NOW, 270);
        assert_eq!(next(0.5).next_poll_at - NOW, 315);
        assert_eq!(next(0.37).next_poll_at - NOW, 311, "300 · 1.037 = 311.1");
        assert_eq!(
            next(0.37).interval_s,
            300,
            "the stored interval is pre-jitter"
        );
    }

    #[test]
    fn jitter_outside_minus_one_to_one_is_clamped_and_nan_is_ignored() {
        let i = PollInputs {
            pct: None,
            prev_pct: None,
            ..inputs()
        };
        let next = |jitter| plan_after_fetch(&B, &i, jitter).next_poll_at - NOW;
        assert_eq!(next(5.0), 330);
        assert_eq!(next(-5.0), 270);
        assert_eq!(next(f64::NAN), 300);
        assert_eq!(next(f64::INFINITY), 300);
    }

    #[test]
    fn jitter_never_takes_the_poll_below_the_floor() {
        let unknown = PollInputs {
            pct: None,
            prev_pct: None,
            ..active()
        };
        assert_eq!(plan_after_fetch(&B, &unknown, -1.0).next_poll_at - NOW, 180);
        let moving = PollInputs {
            pct: Some(52.0),
            ..active()
        };
        assert_eq!(plan_after_fetch(&B, &moving, -1.0).next_poll_at - NOW, 180);
    }

    #[test]
    fn the_next_poll_is_no_later_than_a_minute_after_the_next_reset() {
        let i = PollInputs {
            next_relevant_reset: Some(NOW + 200),
            ..inputs()
        };
        assert_eq!(plan(i), (450, 260), "min(450, 200 + 60)");
        let exhausted = PollInputs {
            pct: Some(100.0),
            prev_pct: Some(100.0),
            next_relevant_reset: Some(NOW + 300),
            ..inputs()
        };
        assert_eq!(plan(exhausted), (600, 360));
    }

    #[test]
    fn a_reset_beyond_the_interval_changes_nothing() {
        let i = PollInputs {
            next_relevant_reset: Some(NOW + 1000),
            ..inputs()
        };
        assert_eq!(plan(i), (450, 450));
    }

    #[test]
    fn a_reset_at_or_before_now_is_ignored() {
        for reset in [NOW, NOW - 10] {
            let i = PollInputs {
                next_relevant_reset: Some(reset),
                ..inputs()
            };
            assert_eq!(plan(i), (450, 450), "reset = now {:+}", reset - NOW);
        }
    }

    #[test]
    fn when_the_floor_and_the_reset_cap_disagree_the_floor_wins() {
        let i = PollInputs {
            next_relevant_reset: Some(NOW + 50),
            ..inputs()
        };
        assert_eq!(
            plan(i),
            (450, 180),
            "reset + 60 = 110 is below the 180 floor"
        );
    }

    #[test]
    fn an_urgent_poll_keeps_the_reset_cap_above_its_lower_floor() {
        let i = PollInputs {
            pct: Some(80.0),
            prev_pct: Some(78.0),
            next_relevant_reset: Some(NOW + 1),
            ..active()
        };
        assert_eq!(plan(i), (60, 60), "min(60, 61), floor 60");
    }

    #[test]
    fn a_replan_uses_the_roles_default_interval() {
        let p = replan_for_role(&B, true, NOW, 0.0);
        assert_eq!((p.interval_s, p.next_poll_at), (180, NOW + 180));
        let p = replan_for_role(&B, false, NOW, 0.0);
        assert_eq!((p.interval_s, p.next_poll_at), (300, NOW + 300));
    }

    #[test]
    fn a_replan_is_jittered_and_floored() {
        assert_eq!(replan_for_role(&B, true, NOW, 1.0).next_poll_at, NOW + 198);
        assert_eq!(replan_for_role(&B, false, NOW, 1.0).next_poll_at, NOW + 330);
        assert_eq!(replan_for_role(&B, true, NOW, -1.0).next_poll_at, NOW + 180);
        assert_eq!(
            replan_for_role(&B, false, NOW, -1.0).next_poll_at,
            NOW + 270
        );
    }

    fn ascending(count: i64, newest: i64) -> Vec<i64> {
        (0..count).map(|i| newest - (count - 1 - i) * 10).collect()
    }

    #[test]
    fn a_free_slot_means_the_request_may_go_now() {
        assert_eq!(budget_next_free(&B, &[], NOW), None);
        assert_eq!(budget_next_free(&B, &ascending(19, NOW - 5), NOW), None);
    }

    #[test]
    fn at_twenty_counted_requests_the_next_slot_is_when_the_oldest_leaves() {
        let counted = ascending(20, NOW - 5);
        let oldest = counted[0];
        assert_eq!(budget_next_free(&B, &counted, NOW), Some(oldest + 3660));
    }

    #[test]
    fn a_row_a_full_window_old_no_longer_counts() {
        let mut counted = ascending(19, NOW - 5);
        counted.insert(0, NOW - 3660);
        assert_eq!(
            budget_next_free(&B, &counted, NOW),
            None,
            "NOW − 3660 has left"
        );
        counted[0] = NOW - 3659;
        assert_eq!(
            budget_next_free(&B, &counted, NOW),
            Some(NOW + 1),
            "one second younger still counts"
        );
    }

    #[test]
    fn more_than_twenty_rows_wait_for_enough_of_them_to_leave() {
        let counted = ascending(22, NOW - 5);
        // Three must leave before a slot is free: the third-oldest's exit.
        assert_eq!(budget_next_free(&B, &counted, NOW), Some(counted[2] + 3660));
    }

    #[test]
    fn the_counted_times_may_arrive_in_any_order() {
        let mut counted = ascending(20, NOW - 5);
        let oldest = counted[0];
        counted.reverse();
        assert_eq!(budget_next_free(&B, &counted, NOW), Some(oldest + 3660));
    }

    #[test]
    fn a_row_stamped_in_the_future_still_counts() {
        let mut counted = ascending(19, NOW - 5);
        counted.push(NOW + 100);
        assert_eq!(budget_next_free(&B, &counted, NOW), Some(counted[0] + 3660));
    }
}
```

In `crates/tagteam-core/src/lib.rs`, insert `pub mod poll;` between `pub mod ids;` and
`pub mod rotation;`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-core --lib poll::`
Expected: FAIL to compile: ``cannot find type `PollBudget` in this scope`` (and likewise
`PollInputs`, `plan_after_fetch`, `replan_for_role`, `budget_next_free`).

- [ ] **Step 3: Write the implementation**

Insert the following at the top of `crates/tagteam-core/src/poll.rs`, above `#[cfg(test)]`
(leave one blank line between the two parts):
```rust
//! Poll policy (§8.6): when the next usage request for an account should be sent, and whether
//! the hourly budget lets one go out. Ported from cswap's `poll_policy.py`; the hourly budget
//! and the post-jitter floor are tagteam's. Pure: time and jitter are passed in.

/// Growth applied to the previous interval while a reading is not moving (§8.6: `base · 1.5`).
const IDLE_GROWTH: f64 = 1.5;

/// §8.6's constants. Providers return it from `Provider::poll_budget`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PollBudget {
    /// No identity is sent more than this many usage requests in any rolling hour.
    pub hourly_requests: u32,
    /// How long a reserved slot stays valid for sending.
    pub slot_valid_s: i64,
    /// The span of `usage_requests` rows that count (the hour plus the slot validity).
    pub count_window_s: i64,
    /// The shortest interval after a fetch, and the serve TTL of a reading.
    pub floor_s: i64,
    /// The active account's interval when it is moving close to the threshold.
    pub urgent_s: i64,
    /// The active account's longest interval; its default is the floor.
    pub active_max_s: i64,
    pub candidate_default_s: i64,
    pub candidate_max_s: i64,
    /// The shortest interval once a window is at its limit.
    pub exhausted_s: i64,
    /// Percentage points that count as movement.
    pub movement_delta: f64,
    /// Jitter as a fraction of the interval (±).
    pub jitter_frac: f64,
    /// The shortest interval after a recent 429.
    pub post_429_min_s: i64,
    /// A 429 is "recent" from when its backoff lifts until this long afterwards.
    pub recent_429_window_s: i64,
    pub post_429_mult: f64,
    pub post_429_max_s: i64,
    /// "Within reach of the threshold" for the urgent interval, in points.
    pub escalation_margin: f64,
    /// How long after a reset the next poll may land.
    pub reset_slack_s: i64,
}

impl PollBudget {
    pub const STANDARD: PollBudget = PollBudget {
        hourly_requests: 20,
        slot_valid_s: 60,
        count_window_s: 3660,
        floor_s: 180,
        urgent_s: 60,
        active_max_s: 300,
        candidate_default_s: 300,
        candidate_max_s: 600,
        exhausted_s: 600,
        movement_delta: 1.0,
        jitter_frac: 0.10,
        post_429_min_s: 360,
        recent_429_window_s: 3600,
        post_429_mult: 1.5,
        post_429_max_s: 1800,
        escalation_margin: 15.0,
        reset_slack_s: 60,
    };

    /// The interval used for an account with no reading to learn from.
    fn default_interval_s(&self, active: bool) -> i64 {
        if active {
            self.floor_s
        } else {
            self.candidate_default_s
        }
    }

    fn ceiling_s(&self, active: bool) -> i64 {
        if active {
            self.active_max_s
        } else {
            self.candidate_max_s
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PollInputs {
    pub now_s: i64,
    /// Whether the account is the live one.
    pub active: bool,
    /// Max relevant pct of the new reading.
    pub pct: Option<f64>,
    /// Max relevant pct of the previous reading.
    pub prev_pct: Option<f64>,
    /// `usage_state.poll_interval_s`.
    pub prev_interval_s: Option<i64>,
    /// `autoswitch.threshold`.
    pub threshold: f64,
    /// When the last 429's backoff lifts (Decision 2).
    pub last_429_at: Option<i64>,
    pub next_relevant_reset: Option<i64>,
}

/// `interval_s` is the policy interval before jitter and clamping, which is what the next plan
/// starts from (`usage_state.poll_interval_s`). `next_poll_at` has jitter and clamps applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollPlan {
    pub interval_s: i64,
    pub next_poll_at: i64,
}

fn grow(base_s: i64, mult: f64) -> i64 {
    (base_s as f64 * mult).round() as i64
}

fn jittered(interval_s: i64, jitter: f64, frac: f64) -> i64 {
    let j = if jitter.is_finite() {
        jitter.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    (interval_s as f64 * (1.0 + j * frac)).round() as i64
}

/// §8.6 `plan_after_fetch`, with Decision 9's clamps. `jitter` is in [-1, 1].
///
/// 1. Unknown pct: the role's default (180 s active, 300 s candidate). Otherwise movement of at
///    least `movement_delta` points (up or down; a drop is a window reset) gives
///    `max(180, base/2)`, and no movement gives `min(ceiling, max(180, base · 1.5))`, where
///    `base` is the previous interval or the role's default.
/// 2. Urgent (active, moving, within `escalation_margin` of the threshold, no recent 429): 60 s.
/// 3. Recent 429: `min(1800, max(interval, max(base · 1.5, 360)))`. A 429 is recent from when
///    its backoff lifts (or while it has not lifted yet) for `recent_429_window_s`. This applies
///    with an unknown pct too: the rule protects the endpoint, not the reading.
/// 4. Exhausted (pct ≥ 100): at least 600 s.
/// 5. Jitter, then the floor (180 s; 60 s when urgent), then the reset cap: the next poll is
///    `max(now + floor, min(now + interval, reset + 60))`, and a reset at or before `now` is
///    ignored. When the cap and the floor disagree, the floor wins.
pub fn plan_after_fetch(b: &PollBudget, i: &PollInputs, jitter: f64) -> PollPlan {
    let default_s = b.default_interval_s(i.active);
    let ceiling_s = b.ceiling_s(i.active);
    let base = i.prev_interval_s.unwrap_or(default_s);
    let moving = match (i.pct, i.prev_pct) {
        (Some(p), Some(q)) => (p - q).abs() >= b.movement_delta,
        _ => false,
    };
    let recent_429 = i
        .last_429_at
        .is_some_and(|at| at > i.now_s || i.now_s - at <= b.recent_429_window_s);

    let mut urgent = false;
    let mut interval = match i.pct {
        None => default_s,
        Some(p) => {
            let mut v = if moving {
                (base / 2).max(b.floor_s)
            } else {
                grow(base, IDLE_GROWTH).max(b.floor_s).min(ceiling_s)
            };
            if i.active && moving && !recent_429 && p >= i.threshold - b.escalation_margin {
                urgent = true;
                v = b.urgent_s;
            }
            v
        }
    };
    if recent_429 {
        let grown = grow(base, b.post_429_mult).max(b.post_429_min_s);
        interval = interval.max(grown).min(b.post_429_max_s);
    }
    if i.pct.is_some_and(|p| p >= 100.0) {
        interval = interval.max(b.exhausted_s);
    }

    let floor_s = if urgent { b.urgent_s } else { b.floor_s };
    let mut next = i
        .now_s
        .saturating_add(jittered(interval, jitter, b.jitter_frac));
    if let Some(reset) = i.next_relevant_reset.filter(|r| *r > i.now_s) {
        next = next.min(reset.saturating_add(b.reset_slack_s));
    }
    next = next.max(i.now_s.saturating_add(floor_s));
    PollPlan {
        interval_s: interval,
        next_poll_at: next,
    }
}

/// The default plan for an account whose role changed without a fetch (§8.3's post-switch
/// re-plan): the role's default interval, jittered, from `now_s`, and never below the floor.
pub fn replan_for_role(b: &PollBudget, active: bool, now_s: i64, jitter: f64) -> PollPlan {
    let interval = b.default_interval_s(active);
    let wait = jittered(interval, jitter, b.jitter_frac).max(b.floor_s);
    PollPlan {
        interval_s: interval,
        next_poll_at: now_s.saturating_add(wait),
    }
}

/// When a request may next be sent, given the reservation times in `counted_at` (any order):
/// `None` if a slot is free now.
///
/// A row counts while `now_s − at < count_window_s`, so it leaves the hour exactly
/// `count_window_s` after it was reserved, and the answer is that moment for the row whose
/// leaving brings the count under `hourly_requests`. A row stamped in the future (clock skew)
/// still counts.
pub fn budget_next_free(b: &PollBudget, counted_at: &[i64], now_s: i64) -> Option<i64> {
    let mut counted: Vec<i64> = counted_at
        .iter()
        .copied()
        .filter(|at| now_s.saturating_sub(*at) < b.count_window_s)
        .collect();
    let limit = b.hourly_requests as usize;
    if counted.len() < limit {
        return None;
    }
    counted.sort_unstable();
    Some(counted[counted.len() - limit].saturating_add(b.count_window_s))
}
```

In `crates/tagteam-core/src/lib.rs`, add the crate-root re-export:
```rust
pub use poll::{PollBudget, PollInputs, PollPlan};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-core --lib poll::`
Expected: PASS, 35 tests, including `the_standard_budget_is_the_specs_table`,
`an_active_account_moving_within_fifteen_points_of_the_threshold_is_urgent`,
`a_429_counts_as_recent_until_an_hour_after_its_backoff_lifts`,
`when_the_floor_and_the_reset_cap_disagree_the_floor_wins` and
`at_twenty_counted_requests_the_next_slot_is_when_the_oldest_leaves`.

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

```
git add crates/tagteam-core/src/lib.rs crates/tagteam-core/src/poll.rs
git commit -m "Add the poll policy and the hourly budget arithmetic"
```

---

### Task 5: Pace and projection (core)

**Files:**
- Create: `crates/tagteam-core/src/pace.rs`
- Modify: `crates/tagteam-core/src/lib.rs`

**Interfaces:**
- Consumes: `Window` and `WindowKind::has_pace` from Task 2.
- Produces (`tagteam_core::pace`; `Pace`, `ProjectionMethod` and `Sample` are also re-exported at
  the crate root):
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionMethod { Regression, Average }
impl ProjectionMethod { pub fn as_str(self) -> &'static str; }   // "regression" | "average"

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample { pub fetched_at: i64, pub pct: f64, pub resets_at: Option<i64> }

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pace {
    pub expected_pct: Option<f64>,
    pub ahead: Option<bool>,
    pub rate_per_hour: Option<f64>,          // points per hour
    pub method: Option<ProjectionMethod>,
    pub exhaustion_at: Option<i64>,
    pub will_last_to_reset: Option<bool>,
}

pub fn regression_rate(samples: &[Sample], current_reset: Option<i64>, fetched_at: i64) -> Option<f64>;
pub fn pace(w: &Window, fetched_at: i64, samples: &[Sample]) -> Pace;
```

§8.7 as amended in `c3a735e`: every projection is measured from the reading's `fetched_at`
(`pct` is as of then, not as of now), and the average fallback and `aheadOfPace` cover `Long`
and `Scoped` windows that have a period. The tests pin the numbers with hand-worked cases: a
week-long window three days in at 45 % has `expected = 300/7`, a rate of 0.625 points an hour,
and runs out 316 800 s after the reading.

Judgement calls, flagged rather than decided silently:
- **Average elapsed time.** `elapsed = period − (remaining mod period)` for `remaining > 0`,
  with a `remaining` that is a whole number of periods (the window just started, or a bogus
  reset) giving `elapsed = 0`, which the 24 h suppression then removes. A reset at or before
  the reading gives no average: the window the reading describes is over.
- **Regression for every kind.** The regression rate has no kind restriction in §8.7, so a
  `Short` or `Spend` window with enough samples gets a rate and an ETA; only `expected_pct` and
  `ahead` are limited to the windows the average covers. A `Spend` window never gets the
  average.
- **Samples up to and including `fetched_at`** are used, since `record_usage` inserts the
  current reading's own sample before anything computes pace.
- **`exhaustion_at` and `will_last_to_reset` need a rate** (regression or average). A window
  at 100 % with no rate still reports none; `list` shows "at the limit" from the pct itself.
  With a rate of zero (an average on a 0 % window) `exhaustion_at` is `None` and the window
  will last.
- Rounding: `exhaustion_at` rounds to the nearest second.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-core/src/pace.rs` holding only the test module:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{Window, WindowKind};

    const F: i64 = 1_800_000_000;
    const HOUR: i64 = 3600;
    const DAY: i64 = 86_400;
    const WEEK: i64 = 604_800;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn window(kind: WindowKind, pct: f64, resets_at: Option<i64>, period_s: Option<i64>) -> Window {
        Window {
            key: "w".into(),
            label: "w".into(),
            kind,
            pct,
            resets_at,
            period_s,
            detail: None,
        }
    }

    fn weekly(pct: f64, remaining_s: i64) -> Window {
        window(WindowKind::Long, pct, Some(F + remaining_s), Some(WEEK))
    }

    fn sample(offset_s: i64, pct: f64, resets_at: Option<i64>) -> Sample {
        Sample {
            fetched_at: F + offset_s,
            pct,
            resets_at,
        }
    }

    /// Four hourly samples ending at `F`, rising by `step` points an hour.
    fn rising(end_pct: f64, step: f64, resets_at: Option<i64>) -> Vec<Sample> {
        (0..4)
            .map(|i| sample(-(3 - i) * HOUR, end_pct - step * (3 - i) as f64, resets_at))
            .collect()
    }

    #[test]
    fn method_names_are_the_json_values() {
        assert_eq!(ProjectionMethod::Regression.as_str(), "regression");
        assert_eq!(ProjectionMethod::Average.as_str(), "average");
    }

    #[test]
    fn the_regression_rate_is_the_least_squares_slope_in_points_per_second() {
        let samples = rising(16.0, 2.0, None);
        let rate = regression_rate(&samples, None, F).unwrap();
        assert!(close(rate, 2.0 / 3600.0), "{rate}");
    }

    #[test]
    fn the_slope_is_a_fit_not_the_two_end_points() {
        // 10, 12, 13, 20 over three hours: slope = 15.5 / 5 = 3.1 pts/h by least squares (the end points alone give 3.33).
        let samples = [
            sample(-3 * HOUR, 10.0, None),
            sample(-2 * HOUR, 12.0, None),
            sample(-HOUR, 13.0, None),
            sample(0, 20.0, None),
        ];
        let rate = regression_rate(&samples, None, F).unwrap();
        assert!(close(rate * 3600.0, 3.1), "{}", rate * 3600.0);
    }

    #[test]
    fn it_needs_three_samples() {
        let samples = [sample(-3 * HOUR, 10.0, None), sample(0, 16.0, None)];
        assert_eq!(regression_rate(&samples, None, F), None);
    }

    #[test]
    fn it_needs_the_samples_to_span_two_hours() {
        let short = [
            sample(-7199, 10.0, None),
            sample(-3600, 12.0, None),
            sample(0, 14.0, None),
        ];
        assert_eq!(regression_rate(&short, None, F), None);
        let enough = [
            sample(-7200, 10.0, None),
            sample(-3600, 12.0, None),
            sample(0, 14.0, None),
        ];
        assert!(regression_rate(&enough, None, F).is_some());
    }

    #[test]
    fn it_needs_a_positive_slope() {
        let flat = [
            sample(-7200, 10.0, None),
            sample(-3600, 10.0, None),
            sample(0, 10.0, None),
        ];
        assert_eq!(regression_rate(&flat, None, F), None);
        let falling = [
            sample(-7200, 14.0, None),
            sample(-3600, 12.0, None),
            sample(0, 10.0, None),
        ];
        assert_eq!(regression_rate(&falling, None, F), None);
    }

    #[test]
    fn only_samples_of_the_current_window_instance_count() {
        let reset = Some(F + 3 * DAY);
        let old_instance = Some(F - 4 * DAY);
        let mut samples = rising(16.0, 2.0, reset);
        // A previous instance with a steep climb that would wreck the fit if it leaked in.
        samples.extend([
            sample(-3 * HOUR, 90.0, old_instance),
            sample(-2 * HOUR, 95.0, old_instance),
            sample(-HOUR, 99.0, old_instance),
        ]);
        let rate = regression_rate(&samples, reset, F).unwrap();
        assert!(close(rate, 2.0 / 3600.0), "{rate}");
    }

    #[test]
    fn a_reset_within_sixty_seconds_is_the_same_instance() {
        let reset = F + 3 * DAY;
        let jittery = [
            sample(-7200, 10.0, Some(reset + 60)),
            sample(-3600, 12.0, Some(reset - 60)),
            sample(0, 14.0, Some(reset)),
        ];
        assert!(regression_rate(&jittery, Some(reset), F).is_some());
        let apart = [
            sample(-7200, 10.0, Some(reset + 61)),
            sample(-3600, 12.0, Some(reset)),
            sample(0, 14.0, Some(reset)),
        ];
        assert_eq!(
            regression_rate(&apart, Some(reset), F),
            None,
            "61 s apart is a different instance, leaving two samples"
        );
    }

    #[test]
    fn windows_without_a_reset_group_with_samples_that_have_none() {
        let mut samples = rising(16.0, 2.0, None);
        samples.push(sample(-HOUR, 99.0, Some(F + DAY)));
        assert!(close(
            regression_rate(&samples, None, F).unwrap(),
            2.0 / 3600.0
        ));
        assert_eq!(regression_rate(&samples, Some(F + DAY), F), None);
    }

    #[test]
    fn only_the_last_forty_eight_hours_before_the_reading_count() {
        let samples = [
            sample(-49 * HOUR, 1.0, None),
            sample(-48 * HOUR - 1, 2.0, None),
            sample(-HOUR, 50.0, None),
            sample(0, 52.0, None),
        ];
        assert_eq!(
            regression_rate(&samples, None, F),
            None,
            "two samples remain, and they span an hour"
        );
        let edge = [
            sample(-48 * HOUR, 10.0, None),
            sample(-24 * HOUR, 20.0, None),
            sample(0, 30.0, None),
        ];
        assert!(close(
            regression_rate(&edge, None, F).unwrap() * 3600.0,
            10.0 / 24.0
        ));
    }

    #[test]
    fn samples_after_the_reading_are_ignored() {
        let mut samples = rising(16.0, 2.0, None);
        samples.push(sample(HOUR, 80.0, None));
        assert!(close(
            regression_rate(&samples, None, F).unwrap(),
            2.0 / 3600.0
        ));
    }

    #[test]
    fn the_average_fallback_follows_cswaps_formulas() {
        // 3 days into a week: elapsed 259 200 s.
        let p = pace(&weekly(45.0, 4 * DAY), F, &[]);
        assert!(
            close(p.expected_pct.unwrap(), 300.0 / 7.0),
            "{:?}",
            p.expected_pct
        );
        assert_eq!(p.ahead, Some(false), "45 − 42.86 < 15");
        assert_eq!(p.method, Some(ProjectionMethod::Average));
        assert!(
            close(p.rate_per_hour.unwrap(), 45.0 / 72.0),
            "{:?}",
            p.rate_per_hour
        );
        // 45 + rate · 4 days = 45 + 60 = 105 > 100: it will not last.
        assert_eq!(p.will_last_to_reset, Some(false));
        // (100 − 45) / rate = 316 800 s.
        assert_eq!(p.exhaustion_at, Some(F + 316_800));
    }

    #[test]
    fn a_window_fifteen_points_ahead_of_its_expected_pct_is_ahead_of_pace() {
        // Halfway through the week the expected pct is exactly 50.
        let half = 302_400;
        assert_eq!(
            pace(&weekly(65.0, half), F, &[]).ahead,
            Some(true),
            "exactly 15"
        );
        assert_eq!(pace(&weekly(64.9, half), F, &[]).ahead, Some(false));
        assert_eq!(
            pace(&weekly(60.0, 4 * DAY), F, &[]).ahead,
            Some(true),
            "60 − 42.86"
        );
    }

    #[test]
    fn the_average_is_suppressed_until_a_day_of_the_period_has_elapsed() {
        let just_under = pace(&weekly(10.0, WEEK - DAY + 1), F, &[]);
        assert_eq!(just_under, Pace::default());
        let exactly = pace(&weekly(10.0, WEEK - DAY), F, &[]);
        assert!(close(exactly.expected_pct.unwrap(), 100.0 / 7.0));
    }

    #[test]
    fn a_window_that_has_just_started_or_just_ended_has_no_average() {
        assert_eq!(
            pace(&weekly(10.0, WEEK), F, &[]),
            Pace::default(),
            "a full period left"
        );
        assert_eq!(pace(&weekly(10.0, 2 * WEEK), F, &[]), Pace::default());
        assert_eq!(
            pace(&weekly(10.0, 0), F, &[]),
            Pace::default(),
            "reset is now"
        );
        assert_eq!(
            pace(&weekly(10.0, -60), F, &[]),
            Pace::default(),
            "reset is past"
        );
    }

    #[test]
    fn expected_pct_tops_out_at_one_hundred() {
        let p = pace(&weekly(100.0, 1), F, &[]);
        assert!(p.expected_pct.unwrap() <= 100.0);
        assert!(p.expected_pct.unwrap() > 99.99);
    }

    #[test]
    fn a_scoped_window_with_a_period_has_the_average_too() {
        let w = window(WindowKind::Scoped, 45.0, Some(F + 4 * DAY), Some(WEEK));
        let p = pace(&w, F, &[]);
        assert_eq!(p.method, Some(ProjectionMethod::Average));
        assert_eq!(p.ahead, Some(false));
    }

    #[test]
    fn no_period_or_no_reset_means_no_average() {
        let scoped_no_period = window(WindowKind::Scoped, 45.0, Some(F + 4 * DAY), None);
        assert_eq!(pace(&scoped_no_period, F, &[]), Pace::default());
        let long_no_reset = window(WindowKind::Long, 45.0, None, Some(WEEK));
        assert_eq!(pace(&long_no_reset, F, &[]), Pace::default());
    }

    #[test]
    fn a_short_window_has_no_average_and_no_expected_pct() {
        let w = window(WindowKind::Short, 40.0, Some(F + 2 * HOUR), Some(18_000));
        assert_eq!(pace(&w, F, &[]), Pace::default());
    }

    #[test]
    fn a_spend_window_has_no_average() {
        let w = window(WindowKind::Spend, 40.0, Some(F + 10 * DAY), Some(30 * DAY));
        assert_eq!(pace(&w, F, &[]), Pace::default());
    }

    #[test]
    fn the_regression_rate_comes_first_and_projections_start_at_the_reading() {
        let reset = Some(F + 90_000);
        let w = window(WindowKind::Short, 40.0, reset, Some(18_000));
        let p = pace(&w, F, &rising(40.0, 2.0, reset));
        assert_eq!(p.method, Some(ProjectionMethod::Regression));
        assert!(
            close(p.rate_per_hour.unwrap(), 2.0),
            "{:?}",
            p.rate_per_hour
        );
        // (100 − 40) points at 2 a point-hour: 30 h after the reading.
        assert_eq!(p.exhaustion_at, Some(F + 30 * HOUR));
        // 40 + 2/h · 25 h = 90 ≤ 100.
        assert_eq!(p.will_last_to_reset, Some(true));
        assert_eq!(
            (p.expected_pct, p.ahead),
            (None, None),
            "never for a short window"
        );
    }

    #[test]
    fn a_window_that_outruns_its_reset_will_not_last() {
        let reset = Some(F + 200_000);
        let w = window(WindowKind::Short, 40.0, reset, Some(18_000));
        let p = pace(&w, F, &rising(40.0, 2.0, reset));
        assert_eq!(p.will_last_to_reset, Some(false), "40 + 2/h · 55.6 h > 100");
    }

    #[test]
    fn a_long_window_gets_the_regression_rate_and_the_average_expected_pct() {
        let reset = Some(F + 4 * DAY);
        let w = weekly(45.0, 4 * DAY);
        let p = pace(&w, F, &rising(45.0, 1.0, reset));
        assert_eq!(p.method, Some(ProjectionMethod::Regression));
        assert!(
            close(p.rate_per_hour.unwrap(), 1.0),
            "{:?}",
            p.rate_per_hour
        );
        assert!(close(p.expected_pct.unwrap(), 300.0 / 7.0));
        assert_eq!(p.ahead, Some(false));
        // 55 points at 1 a point-hour.
        assert_eq!(p.exhaustion_at, Some(F + 55 * HOUR));
        // 45 + 96 h · 1/h = 141 > 100.
        assert_eq!(p.will_last_to_reset, Some(false));
    }

    #[test]
    fn a_window_at_its_limit_is_exhausted_at_the_reading() {
        let reset = Some(F + DAY);
        let w = window(WindowKind::Short, 100.0, reset, Some(18_000));
        let p = pace(&w, F, &rising(100.0, 2.0, reset));
        assert_eq!(p.exhaustion_at, Some(F));
        assert_eq!(p.will_last_to_reset, Some(false));
    }

    #[test]
    fn without_a_reset_there_is_no_will_last_but_there_is_an_exhaustion_time() {
        let w = window(WindowKind::Short, 40.0, None, None);
        let p = pace(&w, F, &rising(40.0, 2.0, None));
        assert_eq!(p.will_last_to_reset, None);
        assert_eq!(p.exhaustion_at, Some(F + 30 * HOUR));
    }

    #[test]
    fn a_window_at_zero_will_last_and_never_runs_out_at_the_average_rate() {
        let p = pace(&weekly(0.0, 4 * DAY), F, &[]);
        assert_eq!(p.method, Some(ProjectionMethod::Average));
        assert_eq!(p.rate_per_hour, Some(0.0));
        assert_eq!(p.exhaustion_at, None, "a zero rate never gets there");
        assert_eq!(p.will_last_to_reset, Some(true));
    }

    #[test]
    fn nothing_to_project_from_gives_an_empty_pace() {
        let w = window(WindowKind::Short, 40.0, Some(F + HOUR), Some(18_000));
        assert_eq!(pace(&w, F, &[]), Pace::default());
        assert_eq!(
            pace(&w, F, &rising(40.0, 0.0, Some(F + HOUR))),
            Pace::default()
        );
    }
}
```

In `crates/tagteam-core/src/lib.rs`, insert `pub mod pace;` between `pub mod ids;` and
`pub mod poll;`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-core --lib pace::`
Expected: FAIL to compile: ``cannot find type `Sample` in this scope`` (and likewise
`ProjectionMethod`, `Pace`, `regression_rate`, `pace`).

- [ ] **Step 3: Write the implementation**

Insert the following at the top of `crates/tagteam-core/src/pace.rs`, above `#[cfg(test)]`
(leave one blank line between the two parts):
```rust
//! Pace and projection (§8.7): how fast a window is being used and when it runs out. Pure; every
//! projection is measured from the reading's own `fetched_at`, because `pct` is as of then.

use crate::usage::Window;

/// Samples older than this before the reading are not used for the rate.
const REGRESSION_LOOKBACK_S: i64 = 48 * 3600;
/// The regression needs this many samples of the current window instance...
const REGRESSION_MIN_SAMPLES: usize = 3;
/// ...spanning at least this long.
const REGRESSION_MIN_SPAN_S: i64 = 2 * 3600;
/// Two samples belong to the same window instance when their `resets_at` differ by at most this.
const INSTANCE_SLACK_S: i64 = 60;
/// The average-pace fallback is suppressed until this much of the period has elapsed.
const AVERAGE_MIN_ELAPSED_S: i64 = 86_400;
/// `aheadOfPace` is `pct − expected ≥` this many points.
const AHEAD_MARGIN: f64 = 15.0;

/// How a rate was measured (`projectionMethod` in JSON).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionMethod {
    Regression,
    Average,
}

impl ProjectionMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectionMethod::Regression => "regression",
            ProjectionMethod::Average => "average",
        }
    }
}

/// One stored reading of one window (`usage_samples`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub fetched_at: i64,
    pub pct: f64,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pace {
    /// Where the window would be if usage were spread evenly over its period.
    pub expected_pct: Option<f64>,
    /// `pct − expected ≥ 15`. Never for windows the average fallback does not cover.
    pub ahead: Option<bool>,
    /// Points per hour.
    pub rate_per_hour: Option<f64>,
    pub method: Option<ProjectionMethod>,
    /// When the window reaches 100 % at that rate, epoch seconds.
    pub exhaustion_at: Option<i64>,
    /// Whether the window reaches its reset without reaching 100 %. `None` without a reset time.
    pub will_last_to_reset: Option<bool>,
}

fn same_instance(sample_reset: Option<i64>, current_reset: Option<i64>) -> bool {
    match (sample_reset, current_reset) {
        (Some(a), Some(b)) => (a - b).abs() <= INSTANCE_SLACK_S,
        (None, None) => true,
        _ => false,
    }
}

/// Points per second over the current window instance (same `resets_at` ± 60 s, or both absent;
/// samples from the 48 h up to and including `fetched_at`): the least-squares slope. `None`
/// unless there are ≥ 3 samples spanning ≥ 2 h and the slope is positive.
pub fn regression_rate(
    samples: &[Sample],
    current_reset: Option<i64>,
    fetched_at: i64,
) -> Option<f64> {
    let since = fetched_at.saturating_sub(REGRESSION_LOOKBACK_S);
    let points: Vec<(i64, f64)> = samples
        .iter()
        .filter(|s| s.fetched_at >= since && s.fetched_at <= fetched_at)
        .filter(|s| same_instance(s.resets_at, current_reset))
        .map(|s| (s.fetched_at, s.pct))
        .collect();
    if points.len() < REGRESSION_MIN_SAMPLES {
        return None;
    }
    let first = points.iter().map(|p| p.0).min()?;
    let last = points.iter().map(|p| p.0).max()?;
    if last - first < REGRESSION_MIN_SPAN_S {
        return None;
    }
    let n = points.len() as f64;
    let xs: Vec<f64> = points.iter().map(|p| (p.0 - first) as f64).collect();
    let mean_x = xs.iter().sum::<f64>() / n;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = xs.iter().map(|x| (x - mean_x).powi(2)).sum();
    let sxy: f64 = xs
        .iter()
        .zip(&points)
        .map(|(x, p)| (x - mean_x) * (p.1 - mean_y))
        .sum();
    let slope = sxy / sxx;
    (slope.is_finite() && slope > 0.0).then_some(slope)
}

/// Seconds of the period already used up: `period − ((reset − fetched_at) mod period)`.
/// `None` (suppressed) when the reset is not ahead of the reading, when a whole number of periods
/// remain (the window has just started), or when less than 24 h have elapsed.
fn elapsed_s(period_s: i64, reset: i64, fetched_at: i64) -> Option<i64> {
    if period_s <= 0 {
        return None;
    }
    let remaining = reset.checked_sub(fetched_at)?;
    if remaining <= 0 {
        return None;
    }
    let into_next = remaining % period_s;
    let elapsed = if into_next == 0 {
        0
    } else {
        period_s - into_next
    };
    (elapsed >= AVERAGE_MIN_ELAPSED_S).then_some(elapsed)
}

/// cswap's average pace for one window, for the kinds and windows it covers: the expected pct
/// and the average rate in points per second.
fn average(w: &Window, fetched_at: i64) -> Option<(f64, f64)> {
    if !w.kind.has_pace() {
        return None;
    }
    let period = w.period_s?;
    let elapsed = elapsed_s(period, w.resets_at?, fetched_at)?;
    let expected = (elapsed as f64 / period as f64 * 100.0).min(100.0);
    Some((expected, w.pct / elapsed as f64))
}

/// §8.7 for one window as read at `fetched_at`. The regression rate comes first, and the average
/// fallback covers `has_pace` kinds with a `period_s`; `expected_pct` and `ahead` only exist for
/// those kinds (never `Short`), even when the rate itself came from the regression.
pub fn pace(w: &Window, fetched_at: i64, samples: &[Sample]) -> Pace {
    let mut out = Pace::default();
    let avg = average(w, fetched_at);
    if let Some((expected, _)) = avg {
        out.expected_pct = Some(expected);
        out.ahead = Some(w.pct - expected >= AHEAD_MARGIN);
    }
    let (rate, method) = match regression_rate(samples, w.resets_at, fetched_at) {
        Some(r) => (r, ProjectionMethod::Regression),
        None => match avg {
            Some((_, r)) => (r, ProjectionMethod::Average),
            None => return out,
        },
    };
    out.rate_per_hour = Some(rate * 3600.0);
    out.method = Some(method);
    out.exhaustion_at = if w.pct >= 100.0 {
        Some(fetched_at)
    } else if rate > 0.0 {
        let seconds = ((100.0 - w.pct) / rate).round() as i64;
        Some(fetched_at.saturating_add(seconds))
    } else {
        None
    };
    out.will_last_to_reset = w.resets_at.map(|reset| {
        let remaining = reset.saturating_sub(fetched_at).max(0) as f64;
        w.pct <= 0.0 || w.pct + rate * remaining <= 100.0
    });
    out
}
```

In `crates/tagteam-core/src/lib.rs`, add the crate-root re-export:
```rust
pub use pace::{Pace, ProjectionMethod, Sample};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-core --lib pace::`
Expected: PASS, 27 tests, including `the_average_fallback_follows_cswaps_formulas`,
`a_window_fifteen_points_ahead_of_its_expected_pct_is_ahead_of_pace`,
`the_regression_rate_comes_first_and_projections_start_at_the_reading` and
`the_average_is_suppressed_until_a_day_of_the_period_has_elapsed`.

- [ ] **Step 5: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.
and the crate's whole suite once more, since this is the last core task of the group:
`cargo test -p tagteam-core` — Expected: PASS, every test in the crate.

- [ ] **Step 6: Commit**

```
git add crates/tagteam-core/src/lib.rs crates/tagteam-core/src/pace.rs
git commit -m "Add pace and projection from the reading's fetch time"
```

---

### Task 6: Claude Code usage request, normalization and rendering

Claude Code's half of §8.1 and §8.2, as pure functions in a new `crates/tagteam-cc/src/usage.rs`:
the request, the ISO 8601 reader (Decision 5), §8.2's normalization table, the verdict on one
reply, and §13.2's rendering back into cswap's shape. Nothing here sends a request, holds a lock
or writes anything. Task 7 connects `fetch_usage` to these functions, and the collector
(Tasks 10–11) owns everything around them. The task also adds `UsageResult` to
`tagteam-provider`: the contract lists it under Task 7, but `parse_usage` already returns it.

The recorded reply (`usage-200.json`, M2a Task 1) is the main fixture. Its `resets_at` values in
epoch seconds, which the tests pin:

| Recorded `resets_at` | Epoch seconds |
|---|---|
| `five_hour`: `2030-01-04T17:20:00.000000+00:00` | `1_893_777_600` |
| `seven_day`: `2030-01-08T00:00:00.000000+00:00` | `1_894_060_800` |
| the Fable limit: `2030-01-08T00:00:01+00:00` | `1_894_060_801` |

It normalizes to three windows: `5h` (9.0), `7d` (77.0) and `scoped:Fable` (0.0, weekly). There is
no spend window: `spend.enabled` is false, and `extra_usage` is not read because a `spend` object
exists.

Rulings this task makes where §8.2 and §13.2 leave room. A test pins each one, except the lint,
which Step 5's clippy runs check.
- **Window order.** `5h`, `7d`, `spend`, then the scoped windows in `limits[]` order: §13.1's
  column order.
- **Which keys are "known" for `bad-response`.** `five_hour`, `seven_day`, `spend` and
  `extra_usage` must be objects, `limits` an array, and `utilization` and `resets_at` inside
  `five_hour`/`seven_day` a number and a string, each when present and not null. Inside
  `limits[]`, `spend` and `extra_usage`, a missing or mistyped field only leaves that window out,
  because §8.2 defines those windows by what an item carries.
- **Two `limits[]` items for one model** keep the higher pct. That is the more constraining
  reading, and it gives one window per key, which `usage_samples`' primary key
  `(account_id, window, fetched_at)` needs (Task 8's insert would fail otherwise). The recorded
  item carries a `scope.surface`, so per-surface duplicates are plausible.
- **Spend amounts.** `exponent` defaults to 2, as `extra_usage`'s `decimal_places` does. An
  exponent outside 0–18 leaves the window out rather than showing a wrong amount. The currency is
  `limit.currency`, else `used.currency`. Spend windows have no `resets_at` (§8.2 reads none).
- **Rendering.** A window the reading lacks is `null` (`scoped` is `[]`). Times are ISO 8601 UTC
  (`2030-01-04T17:20:00Z`, `format_iso8601`). `fiveHour` carries no pace fields, and
  `rate_per_hour` is not part of §13.2's shape (it is `history`'s, Task 15).
- **`User-Agent`.** The request does not set it. The production adapter sets it on every request
  and drops a provider-supplied one (§4.4), as for the profile request.
- **Lint.** The contract fixes `normalize(&Value) -> Result<Vec<Window>, ()>`. Clippy's
  `result_unit_err` fires on a public `Result<_, ()>`, so the function carries
  `#[allow(clippy::result_unit_err)]`, as the engine already allows `too_many_arguments`.

**Files:**
- Create: `crates/tagteam-cc/src/usage.rs`, `crates/tagteam-cc/tests/usage.rs`
- Modify: `crates/tagteam-provider/src/provider.rs` (`UsageResult`),
  `crates/tagteam-provider/src/lib.rs` (re-export), `crates/tagteam-cc/src/lib.rs` (`pub mod usage;`)
- Read: `crates/tagteam-cc/tests/fixtures/endpoints/usage-200.json` (unchanged)

**Interfaces:**
- Consumes:
  - Task 2: `tagteam_core::usage::{Window, WindowKind}` (a `pub mod`; `Window`'s seven public
    fields; `WindowKind: Copy + PartialEq`).
  - Task 3: `tagteam_core::backoff::parse_retry_after(value: &str) -> Option<f64>` (seconds form
    only; `None` for an HTTP date).
  - Task 5: `tagteam_core::pace::{Pace, ProjectionMethod}` (`Pace: Copy + Default` with its six
    `Option` fields; `ProjectionMethod::as_str`).
  - M2a: `tagteam_provider::http::{HttpRequest, HttpResponse, HttpError, Method}`
    (`HttpRequest::get(..).bearer(..).header(..)`, `HttpResponse::{json, header}`),
    `tagteam_provider::provider::TransientKind`, `tagteam_cc::endpoints::Endpoints` (its `usage`
    URL), and the recorded fixture.
- Produces:
  - `tagteam_provider::provider::UsageResult`, as the contract spells it, re-exported as
    `tagteam_provider::UsageResult`.
  - `tagteam_cc::usage::{USAGE_TIMEOUT, USAGE_BETA, usage_request, parse_iso8601, normalize,
    parse_usage, render}`, with the contract's signatures, plus
    `pub fn format_iso8601(epoch_s: i64) -> String` (`YYYY-MM-DDTHH:MM:SSZ`), which later tasks
    may use for any rendered time.
  - Claude Code's window keys and labels: `5h`/`5h` (Short, 18 000 s), `7d`/`7d` (Long,
    604 800 s), `spend`/`spend` (Spend, `detail {used, limit, currency}`), and
    `scoped:<name>`/`<name>` (Scoped, 604 800 s when `group` is `weekly`).

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-cc/tests/usage.rs`:

```rust
//! §8.1, §8.2 and §13.2 for Claude Code: the usage request, the recorded reply's windows,
//! every normalization rule, the verdicts, and the rendered JSON.

use std::time::Duration;

use serde_json::{Value, json};
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::usage::{
    USAGE_BETA, USAGE_TIMEOUT, format_iso8601, normalize, parse_iso8601, parse_usage, render,
    usage_request,
};
use tagteam_core::pace::{Pace, ProjectionMethod};
use tagteam_core::usage::{Window, WindowKind};
use tagteam_provider::http::{HttpError, HttpResponse, Method};
use tagteam_provider::provider::{TransientKind, UsageResult};

/// 2030-01-04T17:20:00Z: the recorded `five_hour.resets_at`.
const FIVE_H_RESET: i64 = 1_893_777_600;
/// 2030-01-08T00:00:00Z: the recorded `seven_day.resets_at`.
const SEVEN_D_RESET: i64 = 1_894_060_800;
/// 2030-01-08T00:00:01Z: the recorded Fable limit's `resets_at`.
const FABLE_RESET: i64 = 1_894_060_801;

/// The recorded reply's body (`usage-200.json`, Appendix A.5).
fn recorded() -> Value {
    let v: Value = serde_json::from_str(include_str!("fixtures/endpoints/usage-200.json")).unwrap();
    v["body"].clone()
}

fn window(
    key: &str,
    label: &str,
    kind: WindowKind,
    pct: f64,
    resets_at: Option<i64>,
    period_s: Option<i64>,
) -> Window {
    Window {
        key: key.into(),
        label: label.into(),
        kind,
        pct,
        resets_at,
        period_s,
        detail: None,
    }
}

fn recorded_windows() -> Vec<Window> {
    vec![
        window(
            "5h",
            "5h",
            WindowKind::Short,
            9.0,
            Some(FIVE_H_RESET),
            Some(18_000),
        ),
        window(
            "7d",
            "7d",
            WindowKind::Long,
            77.0,
            Some(SEVEN_D_RESET),
            Some(604_800),
        ),
        window(
            "scoped:Fable",
            "Fable",
            WindowKind::Scoped,
            0.0,
            Some(FABLE_RESET),
            Some(604_800),
        ),
    ]
}

fn spend(pct: f64, used: f64, limit: f64, currency: &str) -> Window {
    Window {
        detail: Some(json!({"used": used, "limit": limit, "currency": currency})),
        ..window("spend", "spend", WindowKind::Spend, pct, None, None)
    }
}

fn keys(windows: &[Window]) -> Vec<&str> {
    windows.iter().map(|w| w.key.as_str()).collect()
}

fn reply(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Result<HttpResponse, HttpError> {
    Ok(HttpResponse {
        status,
        headers: headers
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        body: body.to_vec(),
    })
}

fn failed(kind: TransientKind, retry_after_s: Option<f64>) -> UsageResult {
    UsageResult::Failed {
        kind,
        retry_after_s,
    }
}

#[test]
fn the_usage_request_follows_section_8_1() {
    let req = usage_request(&Endpoints::production(), "at-secret");
    assert_eq!(req.method, Method::Get);
    assert_eq!(req.url, "https://api.anthropic.com/api/oauth/usage");
    assert_eq!(req.timeout, Duration::from_secs(5));
    assert_eq!(USAGE_TIMEOUT, Duration::from_secs(5));
    assert!(req.body.is_none());
    assert_eq!(USAGE_BETA, "oauth-2025-04-20");
    assert_eq!(
        req.headers,
        vec![
            ("authorization", "Bearer at-secret".to_owned()),
            ("anthropic-beta", "oauth-2025-04-20".to_owned()),
        ],
        "User-Agent is the adapter's (§4.4)"
    );
    assert!(
        !format!("{req:?}").contains("at-secret"),
        "the bearer never reaches Debug"
    );
    let local = usage_request(&Endpoints::with_base("http://127.0.0.1:9"), "t");
    assert_eq!(local.url, "http://127.0.0.1:9/api/oauth/usage");
}

#[test]
fn iso8601_reads_the_forms_the_endpoint_writes_and_nothing_else() {
    for (s, want) in [
        ("2030-01-04T17:20:00.000000+00:00", 1_893_777_600),
        ("2030-01-08T00:00:01+00:00", 1_894_060_801),
        ("2030-01-04T17:20:00Z", 1_893_777_600),
        ("2030-01-04T17:20:00.999Z", 1_893_777_600),
        ("2026-09-30T12:34:56.789-05:30", 1_790_791_496),
        ("2000-03-01T00:00:00+14:00", 951_818_400),
        ("2024-02-29T23:59:59+00:00", 1_709_251_199),
        ("1970-01-01T00:00:00Z", 0),
        ("1969-12-31T23:59:59Z", -1),
    ] {
        assert_eq!(parse_iso8601(s), Some(want), "{s}");
    }
    for s in [
        "2030-01-04T17:20:00",
        "2030-01-04T17:20:00.000000",
        "2030-13-01T00:00:00Z",
        "2030-00-01T00:00:00Z",
        "2030-02-29T00:00:00Z",
        "2030-04-31T00:00:00Z",
        "2030-01-00T00:00:00Z",
        "2030-01-04 17:20:00Z",
        "2030-01-04t17:20:00Z",
        "2030-01-04T17:20:00z",
        "2030-01-04T24:00:00Z",
        "2030-01-04T17:60:00Z",
        "2030-01-04T17:20:60Z",
        "2030-01-04T17:20:00.Z",
        "2030-01-04T17:20:00+0000",
        "2030-01-04T17:20:00+24:00",
        "2030-01-04T17:20:00+05:60",
        "2030-01-04T17:20:00Zjunk",
        "2030-1-04T17:20:00Z",
        "+2030-01-04T17:20:00Z",
        "tomorrow",
        "",
    ] {
        assert_eq!(parse_iso8601(s), None, "{s}");
    }
}

#[test]
fn iso8601_formatting_is_utc_and_round_trips() {
    assert_eq!(format_iso8601(FIVE_H_RESET), "2030-01-04T17:20:00Z");
    assert_eq!(format_iso8601(0), "1970-01-01T00:00:00Z");
    assert_eq!(format_iso8601(-1), "1969-12-31T23:59:59Z");
    assert_eq!(format_iso8601(1_709_251_199), "2024-02-29T23:59:59Z");
    assert_eq!(format_iso8601(253_402_300_799), "9999-12-31T23:59:59Z");
    let mut t = 0;
    while t <= 253_402_300_799 {
        assert_eq!(parse_iso8601(&format_iso8601(t)), Some(t), "{t}");
        t += 7_777_777;
    }
}

#[test]
fn the_recorded_reply_normalizes_to_its_three_windows() {
    // `spend.enabled` is false, and `extra_usage` is not read because a `spend` object exists.
    // The session and weekly_all limits have no model, and the code-named windows and
    // `seven_day_breakdown` are ignored (§8.2).
    assert_eq!(normalize(&recorded()), Ok(recorded_windows()));
}

#[test]
fn missing_null_and_non_finite_sources_leave_their_window_out() {
    let mut body = recorded();
    body["five_hour"] = Value::Null;
    body.as_object_mut().unwrap().shift_remove("limits");
    assert_eq!(keys(&normalize(&body).unwrap()), ["7d"]);

    let mut body = recorded();
    body["seven_day"]["utilization"] = Value::Null;
    assert_eq!(keys(&normalize(&body).unwrap()), ["5h", "scoped:Fable"]);

    // 1e400 parses (arbitrary precision) but is not finite: that window alone is dropped.
    let text = r#"{"five_hour": {"utilization": 1e400, "resets_at": null},
                   "seven_day": {"utilization": 5.5, "resets_at": null},
                   "limits": [{"percent": 1e400, "group": "weekly",
                               "scope": {"model": {"display_name": "Fable"}}}]}"#;
    let body: Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        normalize(&body),
        Ok(vec![window(
            "7d",
            "7d",
            WindowKind::Long,
            5.5,
            None,
            Some(604_800)
        )])
    );

    assert_eq!(normalize(&json!({})), Ok(vec![]), "no windows: no usage");
}

#[test]
fn a_pct_above_100_is_kept_and_an_unreadable_reset_drops_only_the_reset() {
    let mut body = recorded();
    body["five_hour"]["utilization"] = json!(104.5);
    body["seven_day"]["resets_at"] = json!("next tuesday");
    let w = normalize(&body).unwrap();
    assert_eq!(w[0].pct, 104.5);
    assert_eq!((w[1].key.as_str(), w[1].resets_at), ("7d", None));
}

#[test]
fn only_limits_with_a_model_name_and_a_numeric_percent_are_scoped_windows() {
    let mut body = recorded();
    body["limits"] = json!([
        {"kind": "weekly_scoped", "group": "weekly", "percent": 12,
         "resets_at": "2030-01-08T00:00:01+00:00", "scope": {"model": {"display_name": "Opus"}}},
        {"kind": "daily_scoped", "group": "daily", "percent": 3.5,
         "scope": {"model": {"display_name": "Sonnet"}}},
        {"percent": 50, "scope": {"model": {"display_name": ""}}},
        {"percent": "50", "scope": {"model": {"display_name": "Haiku"}}},
        {"percent": 50, "scope": {"model": null}},
        {"percent": 50, "scope": null},
        "not an object",
        7
    ]);
    let w = normalize(&body).unwrap();
    assert_eq!(
        w[2..],
        [
            window(
                "scoped:Opus",
                "Opus",
                WindowKind::Scoped,
                12.0,
                Some(FABLE_RESET),
                Some(604_800)
            ),
            window(
                "scoped:Sonnet",
                "Sonnet",
                WindowKind::Scoped,
                3.5,
                None,
                None
            ),
        ]
    );
}

#[test]
fn two_limits_for_one_model_keep_the_higher_pct() {
    let mut body = recorded();
    let item = |pct: f64, surface: &str| {
        json!({"group": "weekly", "percent": pct, "resets_at": null,
               "scope": {"model": {"display_name": "Fable"}, "surface": surface}})
    };
    body["limits"] = json!([item(20.0, "chat"), item(60.0, "code"), item(40.0, "cowork")]);
    let w = normalize(&body).unwrap();
    assert_eq!(keys(&w), ["5h", "7d", "scoped:Fable"]);
    assert_eq!(w[2].pct, 60.0);
}

#[test]
fn an_enabled_spend_object_is_the_spend_window() {
    let mut body = recorded();
    body["spend"]["enabled"] = json!(true);
    body["spend"]["used"]["amount_minor"] = json!(500);
    let w = normalize(&body).unwrap();
    assert_eq!(keys(&w), ["5h", "7d", "spend", "scoped:Fable"]);
    assert_eq!(w[2], spend(25.0, 5.0, 20.0, "EUR"));

    // The exponent defaults to 2; an absurd one leaves the window out.
    let mut no_exp = body.clone();
    no_exp["spend"]["used"]
        .as_object_mut()
        .unwrap()
        .shift_remove("exponent");
    assert_eq!(
        normalize(&no_exp).unwrap()[2],
        spend(25.0, 5.0, 20.0, "EUR")
    );
    let mut used_currency = body.clone();
    used_currency["spend"]["limit"]
        .as_object_mut()
        .unwrap()
        .shift_remove("currency");
    used_currency["spend"]["used"]["currency"] = json!("USD");
    assert_eq!(
        normalize(&used_currency).unwrap()[2],
        spend(25.0, 5.0, 20.0, "USD"),
        "the limit's currency, else the used amount's"
    );
    let mut absurd = body.clone();
    absurd["spend"]["limit"]["exponent"] = json!(400);
    assert_eq!(
        keys(&normalize(&absurd).unwrap()),
        ["5h", "7d", "scoped:Fable"]
    );

    let mut zero = body.clone();
    zero["spend"]["limit"]["amount_minor"] = json!(0);
    assert_eq!(
        keys(&normalize(&zero).unwrap()),
        ["5h", "7d", "scoped:Fable"],
        "a zero limit leaves the window out"
    );
    let mut no_amount = body.clone();
    no_amount["spend"]["used"]["amount_minor"] = Value::Null;
    assert_eq!(
        keys(&normalize(&no_amount).unwrap()),
        ["5h", "7d", "scoped:Fable"]
    );
}

#[test]
fn extra_usage_is_read_only_without_a_spend_object() {
    let mut body = recorded();
    body["extra_usage"]["is_enabled"] = json!(true);
    body["extra_usage"]["used_credits"] = json!(1500.0);
    assert_eq!(
        keys(&normalize(&body).unwrap()),
        ["5h", "7d", "scoped:Fable"],
        "a spend object exists, even disabled"
    );

    body.as_object_mut().unwrap().shift_remove("spend");
    assert_eq!(normalize(&body).unwrap()[2], spend(75.0, 15.0, 20.0, "EUR"));
    let mut null_spend = recorded();
    null_spend["spend"] = Value::Null;
    null_spend["extra_usage"] = body["extra_usage"].clone();
    null_spend["extra_usage"]["decimal_places"] = json!(0);
    assert_eq!(
        normalize(&null_spend).unwrap()[2],
        spend(75.0, 1500.0, 2000.0, "EUR"),
        "a null spend is no spend object; decimal_places scales both amounts"
    );

    for (field, value) in [
        ("is_enabled", json!(false)),
        ("used_credits", Value::Null),
        ("monthly_limit", Value::Null),
        ("currency", Value::Null),
        ("monthly_limit", json!(0)),
    ] {
        let mut b = body.clone();
        b["extra_usage"][field] = value;
        assert_eq!(
            keys(&normalize(&b).unwrap()),
            ["5h", "7d", "scoped:Fable"],
            "{field}"
        );
    }
}

#[test]
fn a_known_key_of_the_wrong_type_is_a_bad_response() {
    assert_eq!(normalize(&json!([])), Err(()));
    assert_eq!(normalize(&json!("usage")), Err(()));
    for (key, value) in [
        ("five_hour", json!("9%")),
        ("seven_day", json!(77)),
        ("limits", json!({"kind": "session"})),
        ("spend", json!(true)),
        ("extra_usage", json!([])),
    ] {
        let mut body = recorded();
        body[key] = value;
        assert_eq!(normalize(&body), Err(()), "{key}");
    }
    let mut body = recorded();
    body["five_hour"]["utilization"] = json!("9.0");
    assert_eq!(normalize(&body), Err(()));
    let mut body = recorded();
    body["seven_day"]["resets_at"] = json!(1_894_060_800);
    assert_eq!(normalize(&body), Err(()));
}

#[test]
fn every_row_of_the_usage_verdict_table() {
    let body = serde_json::to_vec(&recorded()).unwrap();
    assert_eq!(
        parse_usage(reply(200, &[], &body)),
        UsageResult::Windows(recorded_windows())
    );
    assert_eq!(
        parse_usage(reply(200, &[], b"{}")),
        UsageResult::Windows(vec![])
    );
    assert_eq!(
        parse_usage(reply(200, &[], b"<html>captive portal</html>")),
        failed(TransientKind::BadResponse, None)
    );
    assert_eq!(
        parse_usage(reply(200, &[], br#"{"five_hour": "9%"}"#)),
        failed(TransientKind::BadResponse, None)
    );
    assert_eq!(
        parse_usage(reply(401, &[("retry-after", "30")], b"{}")),
        UsageResult::Unauthorized
    );
    assert_eq!(
        parse_usage(reply(429, &[("retry-after", "120")], b"{}")),
        failed(TransientKind::Http(429), Some(120.0))
    );
    assert_eq!(
        parse_usage(reply(429, &[], b"")),
        failed(TransientKind::Http(429), None)
    );
    assert_eq!(
        parse_usage(reply(
            429,
            &[("retry-after", "Wed, 21 Oct 2015 07:28:00 GMT")],
            b""
        )),
        failed(TransientKind::Http(429), None),
        "only the seconds form is read"
    );
    assert_eq!(
        parse_usage(reply(503, &[("retry-after", "30")], b"")),
        failed(TransientKind::Http(503), Some(30.0))
    );
    assert_eq!(
        parse_usage(reply(403, &[], b"{}")),
        failed(TransientKind::Http(403), None)
    );
    assert_eq!(
        parse_usage(Err(HttpError::PreSend("dns".into()))),
        failed(TransientKind::PreSend, None)
    );
    assert_eq!(
        parse_usage(Err(HttpError::Ambiguous("reset".into()))),
        failed(TransientKind::Ambiguous, None)
    );
}

#[test]
fn the_recorded_windows_render_as_cswap_s_shape() {
    let windows: Vec<(Window, Pace)> = recorded_windows()
        .into_iter()
        .map(|w| (w, Pace::default()))
        .collect();
    assert_eq!(
        render(&windows),
        json!({
            "fiveHour": {"pct": 9.0, "resetsAt": "2030-01-04T17:20:00Z"},
            "sevenDay": {"pct": 77.0, "resetsAt": "2030-01-08T00:00:00Z"},
            "spend": null,
            "scoped": [{"name": "Fable", "pct": 0.0, "resetsAt": "2030-01-08T00:00:01Z"}]
        })
    );
    assert_eq!(
        render(&[]),
        json!({"fiveHour": null, "sevenDay": null, "spend": null, "scoped": []})
    );
}

#[test]
fn pace_fields_render_on_seven_day_and_scoped_windows_only() {
    let full = Pace {
        expected_pct: Some(50.0),
        ahead: Some(true),
        rate_per_hour: Some(1.5),
        method: Some(ProjectionMethod::Average),
        exhaustion_at: Some(1_893_900_000),
        will_last_to_reset: Some(false),
    };
    let regression = Pace {
        rate_per_hour: Some(4.0),
        method: Some(ProjectionMethod::Regression),
        exhaustion_at: Some(1_894_000_000),
        will_last_to_reset: Some(true),
        ..Pace::default()
    };
    let w = recorded_windows();
    let windows = vec![
        (w[0].clone(), regression),
        (w[1].clone(), full),
        (spend(25.0, 5.0, 20.0, "EUR"), Pace::default()),
        (w[2].clone(), regression),
    ];
    assert_eq!(
        render(&windows),
        json!({
            "fiveHour": {"pct": 9.0, "resetsAt": "2030-01-04T17:20:00Z"},
            "sevenDay": {
                "pct": 77.0,
                "resetsAt": "2030-01-08T00:00:00Z",
                "expectedPct": 50.0,
                "aheadOfPace": true,
                "projectedExhaustionAt": "2030-01-06T03:20:00Z",
                "willLastToReset": false,
                "projectionMethod": "average"
            },
            "spend": {"used": 5.0, "limit": 20.0, "pct": 25.0, "currency": "EUR"},
            "scoped": [{
                "name": "Fable",
                "pct": 0.0,
                "resetsAt": "2030-01-08T00:00:01Z",
                "projectedExhaustionAt": "2030-01-07T07:06:40Z",
                "willLastToReset": true,
                "projectionMethod": "regression"
            }]
        })
    );
}
```

Run: `cargo test -p tagteam-cc --test usage`
Expected: FAIL to compile, with `unresolved import tagteam_cc::usage` and
`unresolved import tagteam_provider::provider::UsageResult`. If `tagteam_core::usage`,
`tagteam_core::pace` or `tagteam_core::backoff` is unresolved too, stop: Tasks 2, 3 and 5 are
not done.

- [ ] **Step 2: Add `UsageResult` to the provider crate**

In `crates/tagteam-provider/src/provider.rs`, add this import directly above
`use tagteam_core::{Fingerprint, IdentityKey, ProviderId};`:

```rust
use tagteam_core::usage::Window;
```

Then add, directly before `pub trait Provider: Send + Sync {`:

```rust
/// The provider's verdict on one usage request (§8.1). The engine owns the budget, the lease,
/// the token and what is recorded (§8.3).
#[derive(Debug, Clone, PartialEq)]
pub enum UsageResult {
    /// 200 with a recognised body. Empty means "no usage" (§8.2).
    Windows(Vec<Window>),
    /// The credential has no access token; no request was sent (§8.1).
    NoAccessToken,
    /// 401 (§8.1: the caller decides between the gate and §7.5).
    Unauthorized,
    /// Anything else: `Http(429)` carries `retry_after_s`.
    Failed {
        kind: TransientKind,
        retry_after_s: Option<f64>,
    },
}
```

`Debug` is derived: a window carries no secret.

In `crates/tagteam-provider/src/lib.rs`, replace the `pub use provider::{…};` block with:

```rust
pub use provider::{
    BeforeFallback, Capabilities, CapturedLogin, CredLocks, DeadReason, DoomedEntry, Identity,
    IdentitySurface, KindTraits, LiveAuth, LiveChange, LiveLockSet, LiveLocks, Provider,
    ProviderError, RefreshResult, SecretStore, StoredLogin, TransientKind, Undo, UsageResult,
    Written,
};
```

- [ ] **Step 3: Implement the usage module**

Create `crates/tagteam-cc/src/usage.rs`:

```rust
//! §8.1 and §8.2, Claude Code's half: the usage request, what its reply means, the generic
//! windows it normalizes to, and cswap's JSON shape rendered back from them (§13.2). The engine
//! owns the budget, the lease, the token and every write (§8.3).

use std::time::Duration;

use serde_json::{Map, Value, json};
use tagteam_core::backoff::parse_retry_after;
use tagteam_core::pace::Pace;
use tagteam_core::usage::{Window, WindowKind};
use tagteam_provider::http::{HttpError, HttpRequest, HttpResponse};
use tagteam_provider::provider::{TransientKind, UsageResult};

use crate::endpoints::Endpoints;

/// §8.1, Appendix A.5.
pub const USAGE_TIMEOUT: Duration = Duration::from_secs(5);

/// The `anthropic-beta` value the usage endpoint requires (§8.1).
pub const USAGE_BETA: &str = "oauth-2025-04-20";

/// Claude Code's window keys (§8.2). A scoped window's key is `scoped:<name>`.
const FIVE_HOUR: &str = "5h";
const SEVEN_DAY: &str = "7d";
const SPEND: &str = "spend";
const SCOPED_PREFIX: &str = "scoped:";

const FIVE_HOUR_S: i64 = 18_000;
const WEEK_S: i64 = 604_800;

/// The largest `exponent` or `decimal_places` an amount may carry; beyond it the amount is
/// nonsense and its window is left out.
const MAX_EXPONENT: i64 = 18;

/// `GET /api/oauth/usage`, with the access token as the bearer and the beta header. The
/// adapter sets `User-Agent` itself (§4.4).
pub fn usage_request(e: &Endpoints, access_token: &str) -> HttpRequest {
    HttpRequest::get(e.usage.clone(), USAGE_TIMEOUT)
        .bearer(access_token)
        .header("anthropic-beta", USAGE_BETA)
}

/// The value of `n` ASCII digits, or `None` if any byte is not one.
fn digits(b: &[u8]) -> Option<i64> {
    b.iter().try_fold(0i64, |n, c| {
        c.is_ascii_digit().then(|| n * 10 + i64::from(c - b'0'))
    })
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date `days` after 1970-01-01, as `(year, month, day)` (`civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Epoch seconds from `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)` (Decision 5). The fraction
/// is dropped. Anything else, a missing offset or an impossible date included, is `None`.
pub fn parse_iso8601(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (digits(&b[0..4])?, digits(&b[5..7])?, digits(&b[8..10])?);
    let (hour, minute, second) = (
        digits(&b[11..13])?,
        digits(&b[14..16])?,
        digits(&b[17..19])?,
    );
    let mut rest = &b[19..];
    if let Some(fraction) = rest.strip_prefix(b".") {
        let n = fraction.iter().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 {
            return None;
        }
        rest = &fraction[n..];
    }
    let offset = match rest {
        b"Z" => 0,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let (h, m) = (digits(&[*h1, *h2])?, digits(&[*m1, *m2])?);
            if h > 23 || m > 59 {
                return None;
            }
            if *sign == b'-' {
                -(h * 3600 + m * 60)
            } else {
                h * 3600 + m * 60
            }
        }
        _ => return None,
    };
    if !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// `YYYY-MM-DDTHH:MM:SSZ` for epoch seconds: how rendered JSON spells a time (§13.2).
pub fn format_iso8601(epoch_s: i64) -> String {
    let (year, month, day) = civil_from_days(epoch_s.div_euclid(86_400));
    let secs = epoch_s.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// `five_hour` or `seven_day`: an object of `utilization` (a percentage) and `resets_at`.
/// Null, or a null or non-finite `utilization`, leaves the window out; a field of the wrong
/// type is `bad-response`.
fn fixed(v: &Value, key: &str, kind: WindowKind, period_s: i64) -> Result<Option<Window>, ()> {
    if v.is_null() {
        return Ok(None);
    }
    let resets_at = match &v["resets_at"] {
        Value::Null => None,
        Value::String(s) => parse_iso8601(s),
        _ => return Err(()),
    };
    let pct = match &v["utilization"] {
        Value::Null => return Ok(None),
        Value::Number(n) => n.as_f64(),
        _ => return Err(()),
    };
    Ok(pct.map(|pct| Window {
        key: key.to_owned(),
        label: key.to_owned(),
        kind,
        pct,
        resets_at,
        period_s: Some(period_s),
        detail: None,
    }))
}

/// `amount / 10^exponent`; `None` for an exponent outside `0..=MAX_EXPONENT`.
fn scaled(amount: f64, exponent: i64) -> Option<f64> {
    (0..=MAX_EXPONENT)
        .contains(&exponent)
        .then(|| amount / 10f64.powi(exponent as i32))
}

/// A `{amount_minor, currency, exponent}` amount. The exponent defaults to 2, as
/// `extra_usage`'s `decimal_places` does (§8.2).
fn money(v: &Value) -> Option<f64> {
    scaled(
        v["amount_minor"].as_f64()?,
        v["exponent"].as_i64().unwrap_or(2),
    )
}

/// The spend window: `pct = used / limit · 100`. A zero limit, or a pct that is not finite,
/// leaves it out (§8.2).
fn spend_window(used: f64, limit: f64, currency: Option<&str>) -> Option<Window> {
    if limit == 0.0 {
        return None;
    }
    let pct = used / limit * 100.0;
    pct.is_finite().then(|| Window {
        key: SPEND.to_owned(),
        label: SPEND.to_owned(),
        kind: WindowKind::Spend,
        pct,
        resets_at: None,
        period_s: None,
        detail: Some(json!({"used": used, "limit": limit, "currency": currency})),
    })
}

/// §8.2's two spend rows: `spend` when it is enabled and both amounts carry `amount_minor`;
/// `extra_usage` only when there is no `spend` object at all.
fn spend(body: &Value) -> Option<Window> {
    let s = &body["spend"];
    if s.is_object() {
        if s["enabled"].as_bool() != Some(true) {
            return None;
        }
        let currency = s["limit"]["currency"]
            .as_str()
            .or_else(|| s["used"]["currency"].as_str());
        return spend_window(money(&s["used"])?, money(&s["limit"])?, currency);
    }
    let e = &body["extra_usage"];
    if e["is_enabled"].as_bool() != Some(true) {
        return None;
    }
    let places = e["decimal_places"].as_i64().unwrap_or(2);
    let used = scaled(e["used_credits"].as_f64()?, places)?;
    let limit = scaled(e["monthly_limit"].as_f64()?, places)?;
    spend_window(used, limit, Some(e["currency"].as_str()?))
}

/// Each `limits[]` item with a `scope.model.display_name` and a numeric `percent` (§8.2).
/// Two items naming the same model keep the higher pct: the more constraining reading, and one
/// window per key, as `usage_samples`' primary key needs.
fn scoped(limits: &Value) -> Vec<Window> {
    let mut out: Vec<Window> = Vec::new();
    for item in limits.as_array().into_iter().flatten() {
        let Some(name) = item["scope"]["model"]["display_name"]
            .as_str()
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Some(pct) = item["percent"].as_f64() else {
            continue;
        };
        let w = Window {
            key: format!("{SCOPED_PREFIX}{name}"),
            label: name.to_owned(),
            kind: WindowKind::Scoped,
            pct,
            resets_at: item["resets_at"].as_str().and_then(parse_iso8601),
            period_s: (item["group"].as_str() == Some("weekly")).then_some(WEEK_S),
            detail: None,
        };
        match out.iter_mut().find(|o| o.key == w.key) {
            Some(o) if o.pct < w.pct => *o = w,
            Some(_) => {}
            None => out.push(w),
        }
    }
    out
}

/// §8.2's table, in the order `5h`, `7d`, `spend`, then the scoped windows as listed. Every
/// other field is ignored. `Err(())` is `bad-response`: the body is not an object, or a key
/// tagteam reads has the wrong type (`five_hour`, `seven_day`, `spend` and `extra_usage` must
/// be objects, `limits` an array, `utilization` a number and `resets_at` a string, each when
/// present and not null). Inside `limits[]`, `spend` and `extra_usage`, a field that is missing
/// or of another type only leaves that window out.
#[allow(clippy::result_unit_err)]
pub fn normalize(body: &Value) -> Result<Vec<Window>, ()> {
    if !body.is_object() {
        return Err(());
    }
    let object_or_null = ["five_hour", "seven_day", "spend", "extra_usage"];
    if object_or_null
        .iter()
        .any(|k| !(body[*k].is_null() || body[*k].is_object()))
        || !(body["limits"].is_null() || body["limits"].is_array())
    {
        return Err(());
    }
    let mut out = Vec::new();
    out.extend(fixed(
        &body["five_hour"],
        FIVE_HOUR,
        WindowKind::Short,
        FIVE_HOUR_S,
    )?);
    out.extend(fixed(
        &body["seven_day"],
        SEVEN_DAY,
        WindowKind::Long,
        WEEK_S,
    )?);
    out.extend(spend(body));
    out.extend(scoped(&body["limits"]));
    Ok(out)
}

/// §8.1's verdict on one usage reply. A 200 whose body normalizes is `Windows`, empty when it
/// names no window (§8.2); a 200 that is not JSON, or does not normalize, is `bad-response`.
/// A 401 is the caller's to handle (§8.1). Any other status is `Http(status)` with
/// `Retry-After` in its seconds form, when there is one (§8.1, §8.5).
pub fn parse_usage(reply: Result<HttpResponse, HttpError>) -> UsageResult {
    let failed = |kind| UsageResult::Failed {
        kind,
        retry_after_s: None,
    };
    let resp = match reply {
        Ok(r) => r,
        Err(HttpError::PreSend(_)) => return failed(TransientKind::PreSend),
        Err(HttpError::Ambiguous(_)) => return failed(TransientKind::Ambiguous),
    };
    match resp.status {
        200 => match resp.json().map(|body| normalize(&body)) {
            Some(Ok(windows)) => UsageResult::Windows(windows),
            _ => failed(TransientKind::BadResponse),
        },
        401 => UsageResult::Unauthorized,
        status => UsageResult::Failed {
            kind: TransientKind::Http(status),
            retry_after_s: resp.header("retry-after").and_then(parse_retry_after),
        },
    }
}

/// `pct` and `resetsAt?`, appended to `o`.
fn pct_and_reset(mut o: Map<String, Value>, w: &Window) -> Map<String, Value> {
    o.insert("pct".into(), json!(w.pct));
    if let Some(at) = w.resets_at {
        o.insert("resetsAt".into(), json!(format_iso8601(at)));
    }
    o
}

/// `sevenDay`'s fields: the window's, then whichever pace fields §8.7 produced.
fn paced(o: Map<String, Value>, w: &Window, p: &Pace) -> Value {
    let mut o = pct_and_reset(o, w);
    if let Some(e) = p.expected_pct {
        o.insert("expectedPct".into(), json!(e));
    }
    if let Some(a) = p.ahead {
        o.insert("aheadOfPace".into(), json!(a));
    }
    if let Some(at) = p.exhaustion_at {
        o.insert("projectedExhaustionAt".into(), json!(format_iso8601(at)));
    }
    if let Some(l) = p.will_last_to_reset {
        o.insert("willLastToReset".into(), json!(l));
    }
    if let Some(m) = p.method {
        o.insert("projectionMethod".into(), json!(m.as_str()));
    }
    Value::Object(o)
}

/// `{used, limit, pct, currency, resetsAt?}`, the amounts from the window's `detail`.
fn spend_json(w: &Window) -> Value {
    let d = w.detail.as_ref().unwrap_or(&Value::Null);
    let mut o = Map::new();
    o.insert("used".into(), d["used"].clone());
    o.insert("limit".into(), d["limit"].clone());
    o.insert("pct".into(), json!(w.pct));
    o.insert("currency".into(), d["currency"].clone());
    if let Some(at) = w.resets_at {
        o.insert("resetsAt".into(), json!(format_iso8601(at)));
    }
    Value::Object(o)
}

/// §13.2's cswap shape: `fiveHour {pct, resetsAt?}`, `sevenDay {pct, resetsAt?, expectedPct?,
/// aheadOfPace?, projectedExhaustionAt?, willLastToReset?, projectionMethod?}`, `spend {used,
/// limit, pct, currency, resetsAt?}` and `scoped [{name, …sevenDay's fields}]`. A window the
/// reading does not have is `null` (`scoped` is then `[]`). Times are ISO 8601 UTC; a short
/// window's pace is not part of the shape.
pub fn render(windows: &[(Window, Pace)]) -> Value {
    let find = |key: &str| windows.iter().find(|(w, _)| w.key == key);
    let five_hour = find(FIVE_HOUR).map_or(Value::Null, |(w, _)| {
        Value::Object(pct_and_reset(Map::new(), w))
    });
    let seven_day = find(SEVEN_DAY).map_or(Value::Null, |(w, p)| paced(Map::new(), w, p));
    let spend = find(SPEND).map_or(Value::Null, |(w, _)| spend_json(w));
    let scoped: Vec<Value> = windows
        .iter()
        .filter(|(w, _)| w.kind == WindowKind::Scoped)
        .map(|(w, p)| {
            let mut o = Map::new();
            o.insert("name".into(), json!(w.label));
            paced(o, w, p)
        })
        .collect();
    json!({"fiveHour": five_hour, "sevenDay": seven_day, "spend": spend, "scoped": scoped})
}
```

Notes for the implementer:
- `serde_json::Value`'s `Index` returns `Null` for a missing key, or when the value is not an
  object, so `body["five_hour"]` and `item["scope"]["model"]` never panic. Null and missing are
  the same case throughout.
- The workspace builds `serde_json` with `arbitrary_precision`. `1e400` then parses as a
  number, and `Number::as_f64` returns `None` for it because it is not finite. That is how a
  non-finite pct drops its window (§8.2) without a separate check. The test builds that body with
  `serde_json::from_str`, because a Rust literal `1e400` does not compile.
- `days_from_civil` and `civil_from_days` are Howard Hinnant's proleptic Gregorian algorithms.
  The round-trip test covers 1970 to 9999, the range the four-digit year format can express.

In `crates/tagteam-cc/src/lib.rs`, add `pub mod usage;` after `pub mod shape;`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p tagteam-cc --test usage`
Expected: PASS, 14 tests.

Run: `cargo test -p tagteam-cc && cargo test -p tagteam-provider`
Expected: PASS. Nothing else changed behaviour.

- [ ] **Step 5: Check formatting and lints**

Run:
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt`, and both clippy runs finish without warnings. If clippy reports
`result_unit_err` on `normalize`, the `#[allow]` was dropped: restore it, because the contract
fixes the signature.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider/src/provider.rs crates/tagteam-provider/src/lib.rs \
  crates/tagteam-cc/src/usage.rs crates/tagteam-cc/src/lib.rs crates/tagteam-cc/tests/usage.rs
git commit -m "Normalize Claude Code's usage reply into windows and render them back"
```

---

### Task 7: Provider trait growth; Claude Code and `FakeAgent` implementations

The `Provider` trait gains §4.5's usage methods: `fetch_usage`, `poll_budget`, `render_usage` and
`live_identity_source`. All four are required, with no default. Both implementors get them in
this task, so the workspace compiles at the commit. `rg -n 'impl.*Provider for' crates` finds
exactly `ClaudeCode` and `FakeAgent`, and no test declares an implementor of its own. `FakeAgent`
gains the `usage` capability and a usage endpoint deliberately unlike Claude Code's (§15.2), so
the collector (Tasks 10–11) and the views (Task 13) cannot quietly take on Claude Code's keys,
units or JSON.

Rules this task pins, each with a test:
- **`fetch_usage` never refreshes, and never checks expiry.** It sends whatever access token the
  credential holds, even an expired one. The engine decides first, through the gate for an
  inactive account and through §7.5 for the live one (§8.1). The pinning test sends a token that
  expired in 1970 and asserts that exactly one request left, and that it was the usage request.
- **No access token means no request** (`NoAccessToken`): an API key, a refresh-only blob, a wiped
  blob, bytes that are not JSON.
- **A setup token is fetched** like any access token (Decision 11). A refusal is an ordinary
  verdict.
- **`live_identity_source`** is `CcPaths::resolve(env).global_config`: the same file
  `live_identity` reads, so it follows `CLAUDE_CONFIG_DIR` and the legacy `.config.json` exactly
  as that does. The test pins the default path and the `CLAUDE_CONFIG_DIR` one.
- **Both budgets are `PollBudget::STANDARD`.** The engine must still ask the provider, never use
  the constant (§4.5).

`FakeAgent`'s usage endpoint, which Tasks 10 and 13 script through `FakeFx`:

| | `FakeAgent` | Claude Code |
|---|---|---|
| Request | `GET {base}/usage` (`usage_url()`), `authorization: Fake <fa.token>` | `GET …/api/oauth/usage`, `Bearer`, `anthropic-beta` |
| Reply | `{"meters": [{"id", "used", "renews"}]}`; `used` is a fraction of 1, `renews` epoch seconds | percentages; ISO 8601 resets |
| Windows | `daily` (Short, 86 400 s), `monthly` (Long, 2 592 000 s), `pct = used · 100`; other ids ignored | `5h`, `7d`, `spend`, `scoped:<name>` |
| Verdicts | as Claude Code's: 401 `Unauthorized`, other statuses `Http(code)` with `Retry-After`, no `meters` array `bad-response` | §8.1 |
| `render_usage` | `{"meters": {"<key>": <pct>}}` | §13.2's cswap shape |
| `live_identity_source` | `FakePaths::identity` (`~/.fakeagent/identity.json`) | `~/.claude.json` |

The task scopes suggested an `x-fake-token` header. The token rides in `authorization` under
FakeAgent's own `Fake` scheme instead: `HttpRequest`'s `Debug` redacts only `authorization`, so a
token in any other header would reach `Debug` and break the Global Constraints' rule that secrets
never do. The test asserts the recorded request's `Debug` does not show it.

**Files:**
- Create: `crates/tagteam-fake/src/usage.rs`
- Modify: `crates/tagteam-provider/src/provider.rs` (the trait), `crates/tagteam-provider/src/lib.rs`
  (re-exports), `crates/tagteam-cc/src/provider.rs`, `crates/tagteam-cc/tests/provider.rs`,
  `crates/tagteam-fake/src/lib.rs`, `crates/tagteam-fake/src/provider.rs`,
  `crates/tagteam-fake/tests/provider.rs`

**Interfaces:**
- Consumes:
  - Task 6: `tagteam_provider::UsageResult`; `tagteam_cc::usage::{usage_request, parse_usage,
    render, normalize}`.
  - Task 4: `tagteam_core::poll::PollBudget` (`PollBudget::STANDARD`; `Debug + PartialEq`).
  - Tasks 2, 3 and 5: `tagteam_core::usage::{Window, WindowKind}`,
    `tagteam_core::backoff::parse_retry_after`, `tagteam_core::pace::{Pace, ProjectionMethod}`.
  - M2a: `tagteam_cc::shape::{access_token, setup_token_credential}`, `CcPaths::resolve`,
    `FakeAgent`'s `shape::token`, `FakePaths`, `tagteam_fake::credential_json`, `ScriptedHttp`.
- Produces:
  - On `pub trait Provider` (the contract's signatures):
    `fn fetch_usage(&self, http: &dyn Http, cred: &Credential) -> UsageResult;`,
    `fn poll_budget(&self) -> PollBudget;`,
    `fn render_usage(&self, windows: &[(Window, Pace)]) -> serde_json::Value;`,
    `fn live_identity_source(&self, env: &Env) -> Option<PathBuf>;`.
  - `tagteam_provider::{Window, Pace, PollBudget}`, re-exported from `tagteam_core`.
  - `FakeAgent::usage_url(&self) -> String` (`{base}/usage`), and
    `FakeAgent::capabilities().usage == true`.
  - `FakeAgent`'s usage shape, as in the table above.

- [ ] **Step 1: Write the failing Claude Code tests**

In `crates/tagteam-cc/tests/provider.rs`, replace the `use serde_json::…;` line through the end of
the `use tagteam_provider::{…};` block with:

```rust
use serde_json::{Value, json};
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::provider::ClaudeCode;
use tagteam_cc::usage;
use tagteam_cc::{CcPaths, ItemKind, keychain_account, keychain_service, read_services};
use tagteam_core::Fingerprint;
use tagteam_provider::http::{HttpResponse, Method, ScriptedHttp};
use tagteam_provider::provider::TransientKind;
use tagteam_provider::{
    Capabilities, Credential, Env, FakeKeychain, KindTraits, LockError, MutationGuard, Pace,
    PollBudget, Provider, ProviderError, Read, SecretStore, StoredLogin, UsageResult,
};
```

and append:

```rust
/// The recorded usage reply's body (`usage-200.json`, Appendix A.5).
fn usage_body() -> Value {
    let v: Value = serde_json::from_str(include_str!("fixtures/endpoints/usage-200.json")).unwrap();
    v["body"].clone()
}

/// An OAuth credential whose access token expired long ago: expiry is the engine's to act on.
fn with_access(at: &str) -> Credential {
    Credential::fresh(
        json!({"claudeAiOauth": {"accessToken": at, "refreshToken": "rt", "expiresAt": 1}})
            .to_string()
            .into_bytes(),
    )
}

#[test]
fn fetch_usage_sends_the_access_token_as_it_is_and_never_refreshes() {
    let f = fx();
    let http = ScriptedHttp::new();
    let e = Endpoints::production();
    http.push_json(Method::Get, &e.usage, 200, usage_body());
    assert_eq!(
        f.cc.fetch_usage(&http, &with_access("at-usage")),
        UsageResult::Windows(usage::normalize(&usage_body()).unwrap())
    );
    let sent = http.requests();
    assert_eq!(sent.len(), 1, "one usage request and no token request");
    assert_eq!(
        (sent[0].method, sent[0].url.as_str()),
        (Method::Get, e.usage.as_str())
    );
    assert_eq!(
        sent[0].headers,
        vec![
            ("authorization".to_owned(), "Bearer at-usage".to_owned()),
            ("anthropic-beta".to_owned(), "oauth-2025-04-20".to_owned()),
        ]
    );
}

#[test]
fn fetch_usage_sends_nothing_without_an_access_token() {
    let f = fx();
    let http = ScriptedHttp::new();
    for bytes in [
        b"sk-ant-api03-key".to_vec(),
        json!({"claudeAiOauth": {"refreshToken": "rt"}})
            .to_string()
            .into_bytes(),
        json!({"claudeAiOauth": {"accessToken": "", "refreshToken": ""}})
            .to_string()
            .into_bytes(),
        b"not json".to_vec(),
    ] {
        assert_eq!(
            f.cc.fetch_usage(&http, &Credential::fresh(bytes)),
            UsageResult::NoAccessToken
        );
    }
    assert!(http.requests().is_empty());
}

#[test]
fn a_setup_token_is_fetched_like_any_access_token() {
    // Decision 11: whether the endpoint accepts a `user:inference`-only token is unrecorded,
    // so it is asked, and a refusal is an ordinary verdict.
    let f = fx();
    let http = ScriptedHttp::new();
    http.push_json(Method::Get, &Endpoints::production().usage, 401, json!({}));
    let setup = Credential::fresh(tagteam_cc::shape::setup_token_credential(
        "sk-ant-oat01-setup",
    ));
    assert_eq!(f.cc.fetch_usage(&http, &setup), UsageResult::Unauthorized);
    assert_eq!(
        http.requests()[0].headers[0],
        (
            "authorization".to_owned(),
            "Bearer sk-ant-oat01-setup".to_owned()
        )
    );
}

#[test]
fn a_rate_limited_fetch_carries_its_retry_after() {
    let f = fx();
    let http = ScriptedHttp::new();
    http.push(
        Method::Get,
        &Endpoints::production().usage,
        Ok(HttpResponse {
            status: 429,
            headers: vec![("retry-after".into(), "90".into())],
            body: vec![],
        }),
    );
    assert_eq!(
        f.cc.fetch_usage(&http, &with_access("at")),
        UsageResult::Failed {
            kind: TransientKind::Http(429),
            retry_after_s: Some(90.0)
        }
    );
}

#[test]
fn claude_code_s_budget_rendering_and_live_identity_source() {
    let f = fx();
    assert_eq!(f.cc.poll_budget(), PollBudget::STANDARD);
    assert_eq!(
        f.cc.live_identity_source(&f.env),
        Some(f.env.home.join(".claude.json"))
    );
    let mut env = f.env.clone();
    let dir = f.env.home.join("cfg");
    env.claude_config_dir = Some(dir.clone().into_os_string());
    assert_eq!(
        f.cc.live_identity_source(&env),
        Some(CcPaths::resolve(&env).global_config),
        "the file live_identity reads"
    );
    assert_eq!(
        f.cc.live_identity_source(&env),
        Some(dir.join(".claude.json"))
    );
    let windows: Vec<_> = usage::normalize(&usage_body())
        .unwrap()
        .into_iter()
        .map(|w| (w, Pace::default()))
        .collect();
    assert_eq!(f.cc.render_usage(&windows), usage::render(&windows));
    assert_eq!(f.cc.render_usage(&windows)["sevenDay"]["pct"], json!(77.0));
}
```

- [ ] **Step 2: Write the failing `FakeAgent` tests**

In `crates/tagteam-fake/tests/provider.rs`, replace the `use serde_json::…;` line through the end
of the `use tagteam_provider::{…};` block with:

```rust
use serde_json::{Value, json};
use tagteam_core::Fingerprint;
use tagteam_core::pace::ProjectionMethod;
use tagteam_core::usage::WindowKind;
use tagteam_fake::{
    FAKE_AGENT, FakeAgent, FakePaths, KIND_STATIC, KIND_TOKEN, LOGIN_EXPIRES, credential_json,
    identity_json, login,
};
use tagteam_provider::http::{HttpError, HttpResponse, Method, RecordedRequest, ScriptedHttp};
use tagteam_provider::provider::TransientKind;
use tagteam_provider::{
    Capabilities, Credential, Env, LiveChange, LockError, MutationGuard, Pace, PollBudget,
    Provider, ProviderError, Read, SecretStore, StoredLogin, UsageResult, Window, Written,
};
```

In `kinds_capabilities_endpoints_and_surface`, change the capabilities assertion to:

```rust
    assert_eq!(
        f.fake.capabilities(),
        Capabilities {
            usage: true,
            refresh: true,
            ..Capabilities::default()
        }
    );
```

and add, directly after the `renew_url` assertion:

```rust
    assert_eq!(f.fake.usage_url(), "https://fake-agent.invalid/usage");
```

Then append:

```rust
/// A day and 30 days after 1_790_000_000 s: FakeAgent's `renews` are epoch seconds.
const RENEWS_DAY: i64 = 1_790_086_400;
const RENEWS_MONTH: i64 = 1_792_592_000;

fn meter_window(key: &str, kind: WindowKind, pct: f64, renews: i64, period_s: i64) -> Window {
    Window {
        key: key.into(),
        label: key.into(),
        kind,
        pct,
        resets_at: Some(renews),
        period_s: Some(period_s),
        detail: None,
    }
}

/// One usage fetch with `fa-tok`, against a fresh `ScriptedHttp` that answers with `reply`.
fn fetch_once(reply: Result<HttpResponse, HttpError>) -> (UsageResult, Vec<RecordedRequest>) {
    let fake = FakeAgent::new();
    let http = ScriptedHttp::new();
    http.push(Method::Get, &fake.usage_url(), reply);
    let token = Credential::fresh(
        credential_json("fa-tok", Some("fa-renew"), None)
            .to_string()
            .into_bytes(),
    );
    (fake.fetch_usage(&http, &token), http.requests())
}

#[test]
fn fake_agent_meters_are_fractions_under_its_own_keys() {
    let body = json!({"meters": [
        {"id": "daily", "used": 0.42, "renews": RENEWS_DAY},
        {"id": "monthly", "used": 0.1, "renews": RENEWS_MONTH},
        {"id": "hourly", "used": 0.9, "renews": RENEWS_DAY}
    ]});
    let (r, sent) = fetch_once(Ok(HttpResponse::json_body(200, &body)));
    assert_eq!(
        r,
        UsageResult::Windows(vec![
            meter_window("daily", WindowKind::Short, 42.0, RENEWS_DAY, 86_400),
            meter_window("monthly", WindowKind::Long, 10.0, RENEWS_MONTH, 2_592_000),
        ]),
        "an unknown meter is ignored"
    );
    assert_eq!(sent.len(), 1);
    assert_eq!(
        (sent[0].method, sent[0].url.as_str()),
        (Method::Get, "https://fake-agent.invalid/usage")
    );
    assert_eq!(
        sent[0].headers,
        vec![("authorization".to_owned(), "Fake fa-tok".to_owned())]
    );
    assert!(
        !format!("{:?}", sent[0]).contains("fa-tok"),
        "the token never reaches Debug"
    );
}

#[test]
fn fake_agent_usage_verdicts() {
    let failed = |kind, retry_after_s| UsageResult::Failed {
        kind,
        retry_after_s,
    };
    assert_eq!(
        fetch_once(Ok(HttpResponse::json_body(401, &json!({})))).0,
        UsageResult::Unauthorized
    );
    let limited = HttpResponse {
        status: 429,
        headers: vec![("retry-after".into(), "60".into())],
        body: vec![],
    };
    assert_eq!(
        fetch_once(Ok(limited)).0,
        failed(TransientKind::Http(429), Some(60.0))
    );
    assert_eq!(
        fetch_once(Ok(HttpResponse::json_body(200, &json!({"meters": "none"})))).0,
        failed(TransientKind::BadResponse, None)
    );
    assert_eq!(
        fetch_once(Ok(HttpResponse::json_body(200, &json!({"meters": []})))).0,
        UsageResult::Windows(vec![])
    );
    assert_eq!(
        fetch_once(Err(HttpError::Ambiguous("reset".into()))).0,
        failed(TransientKind::Ambiguous, None)
    );
}

#[test]
fn fake_agent_without_a_token_sends_nothing() {
    let fake = FakeAgent::new();
    let http = ScriptedHttp::new();
    let renew_only = Credential::fresh(br#"{"fa": {"renew": "r"}}"#.to_vec());
    assert_eq!(
        fake.fetch_usage(&http, &renew_only),
        UsageResult::NoAccessToken
    );
    assert!(http.requests().is_empty());
}

#[test]
fn fake_agent_renders_its_own_shape_and_names_its_identity_file() {
    let f = fx();
    let paced = Pace {
        rate_per_hour: Some(2.0),
        method: Some(ProjectionMethod::Regression),
        ..Pace::default()
    };
    let windows = vec![
        (
            meter_window("daily", WindowKind::Short, 42.0, RENEWS_DAY, 86_400),
            Pace::default(),
        ),
        (
            meter_window("monthly", WindowKind::Long, 10.0, RENEWS_MONTH, 2_592_000),
            paced,
        ),
    ];
    assert_eq!(
        f.fake.render_usage(&windows),
        json!({"meters": {"daily": 42.0, "monthly": 10.0}})
    );
    assert_eq!(f.fake.poll_budget(), PollBudget::STANDARD);
    assert_eq!(
        f.fake.live_identity_source(&f.env),
        Some(FakePaths::resolve(&f.env).identity)
    );
}
```

`0.42 · 100` and `0.1 · 100` are exactly `42.0` and `10.0` in `f64`, so the equality assertions
are exact.

- [ ] **Step 3: Run the new tests to see them fail**

Run: `cargo test -p tagteam-cc --test provider`
Expected: FAIL to compile, with `unresolved imports tagteam_provider::Pace,
tagteam_provider::PollBudget` and `no method named fetch_usage found for struct ClaudeCode`.

Run: `cargo test -p tagteam-fake --test provider`
Expected: FAIL to compile, with `no method named usage_url found for struct FakeAgent` and the
same unresolved imports (`Window` among them).

- [ ] **Step 4: Grow the trait**

In `crates/tagteam-provider/src/provider.rs`, replace Task 6's `use tagteam_core::usage::Window;`
with:

```rust
use tagteam_core::pace::Pace;
use tagteam_core::poll::PollBudget;
use tagteam_core::usage::Window;
```

(`PathBuf`, `Value`, `Http`, `Credential` and `Env` are already imported there.) Add at the end
of `pub trait Provider`, after `refresh` and a blank line:

```rust
    /// §8.1, the provider's half: sends one usage request with the credential's access token
    /// and classifies the reply into generic windows (§8.2). `NoAccessToken`, with nothing
    /// sent, when the credential has none. It never refreshes and never checks expiry: the
    /// engine owns the gate, §7.5, the budget slot and the lease (§8.3).
    fn fetch_usage(&self, http: &dyn Http, cred: &Credential) -> UsageResult;
    /// §8.6's constants and the hourly request cap of this provider's usage endpoint.
    fn poll_budget(&self) -> PollBudget;
    /// §13.2: the provider's JSON for a row's `usage`/`lastGoodUsage`, from windows and their
    /// pace.
    fn render_usage(&self, windows: &[(Window, Pace)]) -> Value;
    /// The file whose mtime and size key `live_identity_cache` (§13.5): the one `live_identity`
    /// reads. CC: `~/.claude.json`. `None` when no single file backs the live identity.
    fn live_identity_source(&self, env: &Env) -> Option<PathBuf>;
```

(`Value` is `serde_json::Value`, as the contract writes it.) In
`crates/tagteam-provider/src/lib.rs`, add after `pub use read::{Read, ReadError};`:

```rust
pub use tagteam_core::pace::Pace;
pub use tagteam_core::poll::PollBudget;
pub use tagteam_core::usage::Window;
```

The workspace does not compile again until Steps 5 and 6 give both implementors the methods.

- [ ] **Step 5: Implement the methods for Claude Code**

In `crates/tagteam-cc/src/provider.rs`, add `use std::path::PathBuf;` above
`use std::sync::Arc;`, replace the `use tagteam_provider::{…};` block with:

```rust
use tagteam_provider::{
    BeforeFallback, Capabilities, CredLocks, Credential, DoomedEntry, Env, FreshCredential,
    Identity, IdentitySurface, Keychain, KindTraits, LiveAuth, LiveChange, LiveLocks,
    MutationGuard, Pace, PollBudget, Provider, ProviderError, Read, StoredLogin, Undo, UsageResult,
    Window, Written,
};
```

and add `use crate::usage;` after `use crate::shape::{self, KIND_API_KEY, KINDS, MACHINE_SHARED_KEYS};`.
Then add to `impl Provider for ClaudeCode`, after `refresh`:

```rust
    fn fetch_usage(&self, http: &dyn Http, cred: &Credential) -> UsageResult {
        // An API key has no access token, and neither has a wiped or refresh-only blob (§8.1).
        // A setup token is sent like any OAuth access token (Decision 11).
        let Some(token) = shape::access_token(cred.bytes()) else {
            return UsageResult::NoAccessToken;
        };
        usage::parse_usage(http.send(&usage::usage_request(&self.endpoints, &token)))
    }

    fn poll_budget(&self) -> PollBudget {
        PollBudget::STANDARD
    }

    fn render_usage(&self, windows: &[(Window, Pace)]) -> Value {
        usage::render(windows)
    }

    fn live_identity_source(&self, env: &Env) -> Option<PathBuf> {
        Some(CcPaths::resolve(env).global_config)
    }
```

- [ ] **Step 6: Give `FakeAgent` its usage endpoint**

Create `crates/tagteam-fake/src/usage.rs`:

```rust
//! FakeAgent's usage endpoint, deliberately unlike Claude Code's (§15.2): its meters report a
//! fraction of 1 rather than a percentage, under ids that are not Claude Code's window keys,
//! with different periods, and it renders its own JSON.

use std::time::Duration;

use serde_json::{Map, Value, json};
use tagteam_core::backoff::parse_retry_after;
use tagteam_core::pace::Pace;
use tagteam_core::usage::{Window, WindowKind};
use tagteam_provider::http::{HttpError, HttpRequest, HttpResponse};
use tagteam_provider::provider::{TransientKind, UsageResult};

const USAGE_TIMEOUT: Duration = Duration::from_secs(5);
const DAY_S: i64 = 86_400;
const MONTH_S: i64 = 2_592_000;

/// `GET <url>` with the token under FakeAgent's own scheme. It rides in `authorization`, so
/// `HttpRequest`'s `Debug` redacts it like any token (§4.4).
pub(crate) fn usage_request(url: String, token: &str) -> HttpRequest {
    HttpRequest::get(url, USAGE_TIMEOUT).header("authorization", format!("Fake {token}"))
}

/// `{"meters": [{"id", "used", "renews"}]}`: `used` is a fraction of 1 and `renews` epoch
/// seconds. `daily` is a Short window of a day and `monthly` a Long one of 30 days; any other
/// meter, or one without a finite `used`, is ignored. `None` without a `meters` array.
fn normalize(body: &Value) -> Option<Vec<Window>> {
    let mut out = Vec::new();
    for m in body.get("meters")?.as_array()? {
        let (id, kind, period_s) = match m["id"].as_str() {
            Some(id @ "daily") => (id, WindowKind::Short, DAY_S),
            Some(id @ "monthly") => (id, WindowKind::Long, MONTH_S),
            _ => continue,
        };
        let Some(pct) = m["used"]
            .as_f64()
            .map(|used| used * 100.0)
            .filter(|p| p.is_finite())
        else {
            continue;
        };
        out.push(Window {
            key: id.to_owned(),
            label: id.to_owned(),
            kind,
            pct,
            resets_at: m["renews"].as_i64(),
            period_s: Some(period_s),
            detail: None,
        });
    }
    Some(out)
}

/// The same verdicts as Claude Code's (§8.1): the engine reads only `UsageResult`.
pub(crate) fn parse_usage(reply: Result<HttpResponse, HttpError>) -> UsageResult {
    let failed = |kind| UsageResult::Failed {
        kind,
        retry_after_s: None,
    };
    let resp = match reply {
        Ok(r) => r,
        Err(HttpError::PreSend(_)) => return failed(TransientKind::PreSend),
        Err(HttpError::Ambiguous(_)) => return failed(TransientKind::Ambiguous),
    };
    match resp.status {
        200 => match resp.json().as_ref().and_then(normalize) {
            Some(windows) => UsageResult::Windows(windows),
            None => failed(TransientKind::BadResponse),
        },
        401 => UsageResult::Unauthorized,
        status => UsageResult::Failed {
            kind: TransientKind::Http(status),
            retry_after_s: resp.header("retry-after").and_then(parse_retry_after),
        },
    }
}

/// `{"meters": {"<key>": <pct>}}`. FakeAgent's JSON carries no pace.
pub(crate) fn render(windows: &[(Window, Pace)]) -> Value {
    let meters: Map<String, Value> = windows
        .iter()
        .map(|(w, _)| (w.key.clone(), json!(w.pct)))
        .collect();
    json!({ "meters": meters })
}
```

In `crates/tagteam-fake/src/lib.rs`, add `mod usage;` after `mod shape;`.

In `crates/tagteam-fake/src/provider.rs`, replace the `use tagteam_provider::{…};` block with:

```rust
use tagteam_provider::{
    BeforeFallback, Capabilities, CredLocks, Credential, DoomedEntry, Env, FreshCredential,
    Identity, IdentitySurface, KindTraits, LiveAuth, LiveChange, LiveLockSet, LiveLocks, LockError,
    MkdirLock, MkdirLockSpec, MutationGuard, Pace, PollBudget, Provider, ProviderError, Read,
    ReadError, SecretStore, StoredLogin, Undo, UsageResult, Window, Written,
};
```

add `use crate::usage;` after `use crate::shape::{self, DEVICE, KIND_STATIC, KINDS};`, and add to
`impl FakeAgent`, after `whoami_url` and a blank line:

```rust
    pub fn usage_url(&self) -> String {
        format!("{}/usage", self.base)
    }
```

In `impl Provider for FakeAgent`, change `capabilities` to:

```rust
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            usage: true,
            refresh: true,
            ..Capabilities::default()
        }
    }
```

and add after `refresh`:

```rust
    fn fetch_usage(&self, http: &dyn Http, cred: &Credential) -> UsageResult {
        let Some(token) = shape::token(cred.bytes()) else {
            return UsageResult::NoAccessToken;
        };
        usage::parse_usage(http.send(&usage::usage_request(self.usage_url(), &token)))
    }

    fn poll_budget(&self) -> PollBudget {
        PollBudget::STANDARD
    }

    fn render_usage(&self, windows: &[(Window, Pace)]) -> Value {
        usage::render(windows)
    }

    fn live_identity_source(&self, env: &Env) -> Option<PathBuf> {
        Some(FakePaths::resolve(env).identity)
    }
```

(`PathBuf`, `Value` and `Http` are already imported in that file.)

- [ ] **Step 7: Confirm every implementor, then run everything this task touched**

Run: `rg -n 'impl.*Provider for' crates`
Expected: exactly two lines, `crates/tagteam-cc/src/provider.rs` (`ClaudeCode`) and
`crates/tagteam-fake/src/provider.rs` (`FakeAgent`). A third implementor would need the four
methods in this task too.

Run:
```bash
cargo test -p tagteam-cc
cargo test -p tagteam-fake
cargo test -p tagteam-provider
cargo test -p tagteam-engine --features test-hooks
cargo build --workspace --all-targets --features tagteam/test-support
```
Expected: PASS, including the five new Claude Code tests and the four new `FakeAgent` tests, and
the workspace builds. The engine suite runs `FakeAgent` beside Claude Code through `FakeFx`. The
capability flip changes none of its results, because nothing outside tests reads
`Capabilities` before Task 10.

- [ ] **Step 8: Check formatting and lints**

Run:
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt`, and both clippy runs finish without warnings.

- [ ] **Step 9: Commit**

```bash
git add crates/tagteam-provider/src/provider.rs crates/tagteam-provider/src/lib.rs \
  crates/tagteam-cc/src/provider.rs crates/tagteam-cc/tests/provider.rs \
  crates/tagteam-fake/src/usage.rs crates/tagteam-fake/src/lib.rs \
  crates/tagteam-fake/src/provider.rs crates/tagteam-fake/tests/provider.rs
git commit -m "Add usage fetching, poll budgets and rendering to every provider"
```

---

### Task 8: Store: usage state, reserve and record, samples, cache

The usage tables have been in `schema.sql` since M1, but nothing reads or writes them. This task
gives the store every accessor the collector (Tasks 10 and 11), the post-switch re-plan (Task 12)
and the views (Task 13) need. It lives in a child module of `store`, so it can use `Store`'s
private `lock()`. No migration: every column already exists.

Rulings made here, each pinned by a test:
- **Every new transaction that reads before it writes is `IMMEDIATE`**, not only the reserve:
  Decision 8's reasoning applies equally to the fence in `record_usage`,
  `record_usage_failure` and `set_rejected_fp`, and to the fence and count in
  `authorize_send`. In WAL mode, a `DEFERRED` transaction whose read is overtaken by another
  connection's commit fails at once with `SQLITE_BUSY` instead of waiting out
  `busy_timeout`. With `DEFERRED` transactions, both
  `two_stores_racing_*` tests fail (checked while writing this plan). Existing store
  transactions are unchanged.
- **Eligibility is checked in §8.3's order:** `Quarantined`, `Backoff`, `Leased`, then the
  schedule (`NotDue`), then the budget. The account row is re-read inside the transaction.
- **The on-demand rule is applied literally.** The reading must be older than `floor_s`
  (180 s) *and* a poll must be due; no plan counts as due. So an account with no reading but a
  plan in force is `NotDue` until the plan comes due. That covers an over-budget plan. (Task 12
  re-plans only accounts that have a reading, so it never parks an unread account.)
- **Scheduled eligibility (M3's)** reads §8.3's "either due or stale" as "a poll is due, or
  there is no reading yet". M2b never calls it, and a test pins it so M3 starts from a stated
  rule.
- **The budget window is Task 4's.** A row counts while `now − at < 3660`, so the store counts
  `at > now − 3660` and prunes `at <= now − 3660`. The prune covers every identity, on each
  reservation.
- **A reading with no windows** (§8.2: "an empty result normalizes to `None`") stores
  `last_good = NULL` but still sets `fetched_at`: it is a fresh reading of nothing, not a
  missing reading.
- **Recording leaves the lease to expire** (§8.3). A failed fence writes nothing, including
  the slot release, so an unsent slot stays counted for its hour. That only errs toward
  sending less.
- **A slot is deleted only by its full identity**: `rowid`, `provider`, `identity_key` and `at`,
  in one statement (`release_slot`, `record_usage_failure`'s `release`, and `authorize_send`'s
  stale slot). `usage_requests` has no `AUTOINCREMENT`, so SQLite gives a pruned row's rowid to
  a later insert, and a process that slept past the count window would otherwise delete another
  process's slot. `Reservation` carries `provider` for this and for the budget key.
- **Nothing is sent without `authorize_send`**, one `IMMEDIATE` transaction right before each
  request, checked in this order: the lease and identity fence (else `LeaseLost`), the durable
  `rejected_fp` (else `Rejected`), then the slot. `LeaseLost` and `Rejected` write nothing: a
  lost holder's unsent slot stays counted, as after any failed fence, and a refused token's
  slot is given back by the caller's failure record. The fence is the records' own: the lease
  row still names the holder, expired or not (§6.1: a result is recorded "only if the lease row
  still shows the same holder"). An expired lease that no one took over is harmless to send
  under, and the durable `rejected_fp` check covers what a takeover could have stamped.
- **`set_rejected_fp` is fenced like the records**, so only the lease's holder stamps or clears
  a refusal; it still creates a missing `usage_state` row.
- **A `live_identity_cache` row without `path`, `mtime_ns` or `size`** reads as no row (a cache
  miss), since the struct's key fields are not optional.

Notes for later tasks:
- Task 10: call `authorize_send` immediately before every request, the first and the 401
  retry, with the access-token fingerprint of the exact bytes about to be sent. Pass the held
  slot for the first request (the reservation's, as a `Slot`) and `None` for the retry, whose
  first slot was spent on the refused request.
- Task 10: an account removed or re-logged mid-fetch makes `authorize_send` return `LeaseLost`,
  and `set_rejected_fp` and both record calls return `Ok(false)`, because the identity fence
  fails.
- Task 13: `fetched_at: Some(_)` with `last_good: None` is a successful reading with no
  windows.

**Files:**
- Create: `crates/tagteam-engine/src/store/usage.rs`, `crates/tagteam-engine/tests/store_usage.rs`
- Modify: `crates/tagteam-engine/src/store/mod.rs`

**Interfaces:**
- Consumes:
  - Task 2: `tagteam_core::{Window, WindowKind}` and
    `tagteam_core::usage::{windows_to_json, windows_from_json}`.
  - Task 4: `tagteam_core::{PollBudget, PollPlan}` and `tagteam_core::poll::budget_next_free`.
    A row counts while `now_s − at < count_window_s`, and `next_free_at` is the moment enough
    rows have left.
  - Task 5: `tagteam_core::Sample`.
  - Existing: `Store`'s private `lock()`, `AccountRow`,
    `StoreError::NoSuchAccount`, `uuid::Uuid::now_v7` (already an engine dependency).
- Produces (the contract's signatures, re-exported from `tagteam_engine::store`):
```rust
#[derive(Debug, Clone, PartialEq)]
pub struct UsageStateRow {
    pub account_id: AccountId,
    pub last_good: Option<Vec<Window>>,
    pub fetched_at: Option<i64>,
    pub last_attempt_at: Option<i64>,
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
    pub backoff_until: Option<i64>,
    pub next_poll_at: Option<i64>,
    pub poll_interval_s: Option<i64>,
    pub last_429_at: Option<i64>,
    pub rejected_fp: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub account_id: AccountId,
    pub provider: ProviderId,     // the budget's key, with identity_key
    pub identity_key: String,
    pub holder: String,           // random per acquisition (uuid v7)
    pub slot: i64,                // the first slot's usage_requests rowid
    pub slot_at: i64,             // when the first slot was reserved
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ineligible { Quarantined, Backoff, Leased, NotDue }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reserve { Reserved(Reservation), Ineligible(Ineligible), OverBudget { next_free_at: i64 } }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot { pub slot: i64, pub slot_at: i64 }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendGrant { Send(Slot), Rejected, LeaseLost, OverBudget { next_free_at: i64 } }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveIdentityCacheRow {
    pub provider: ProviderId,
    pub path: String,
    pub mtime_ns: i64,
    pub size: i64,
    pub identity_key: Option<String>,   // None: the file held no live login
    pub label: Option<String>,
    pub account_uuid: Option<String>,
}

impl Store {
    pub fn usage_state(&self, id: &AccountId) -> Result<Option<UsageStateRow>, StoreError>;
    pub fn reserve_usage(&self, account: &AccountRow, now_ms: i64, on_demand: bool,
                         budget: &PollBudget) -> Result<Reserve, StoreError>;
    pub fn authorize_send(&self, r: &Reservation, slot: Option<&Slot>, access_fp: Option<&str>,
                          now_ms: i64, budget: &PollBudget) -> Result<SendGrant, StoreError>;
    pub fn release_slot(&self, r: &Reservation, slot: &Slot) -> Result<(), StoreError>;
    pub fn record_usage(&self, r: &Reservation, windows: &[Window], now_s: i64, plan: &PollPlan,
                        retention_days: u32) -> Result<bool, StoreError>;
    pub fn record_usage_failure(&self, r: &Reservation, kind: &str, now_s: i64,
                                backoff_until: i64, last_429_at: Option<i64>,
                                release: Option<&Slot>) -> Result<bool, StoreError>;
    pub fn set_rejected_fp(&self, r: &Reservation, fp: Option<&str>) -> Result<bool, StoreError>;
    pub fn set_poll_plan(&self, id: &AccountId, plan: &PollPlan) -> Result<(), StoreError>;
    pub fn usage_samples(&self, id: &AccountId, window: Option<&str>, since_s: i64)
        -> Result<Vec<(String, Sample)>, StoreError>;
    pub fn usage_lease_live(&self, id: &AccountId, now_ms: i64) -> Result<bool, StoreError>;
    pub fn live_identity_cache(&self, provider: &ProviderId)
        -> Result<Option<LiveIdentityCacheRow>, StoreError>;
    pub fn put_live_identity_cache(&self, row: &LiveIdentityCacheRow) -> Result<(), StoreError>;
}
```

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-engine/tests/store_usage.rs`:
```rust
//! §6.1, §8.3, §8.6: the store's usage tables. Usage times are epoch seconds and lease
//! expiries epoch milliseconds (Decision 1).

use std::path::{Path, PathBuf};
use std::sync::Barrier;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::time::Duration;

use rusqlite::{OptionalExtension, params};
use serde_json::json;
use tagteam_core::{AccountId, PollBudget, PollPlan, ProviderId, Sample, Window, WindowKind};
use tagteam_engine::store::{
    Ineligible, LiveIdentityCacheRow, NewAccount, Reservation, Reserve, SendGrant, Slot, Store,
    StoreError, UsageStateRow,
};
use tagteam_provider::Identity;

/// Now, in epoch seconds; `T_MS` is the same instant in milliseconds.
const T: i64 = 1_790_000_000;
const T_MS: i64 = T * 1000;
const B: PollBudget = PollBudget::STANDARD;

fn identity(email: &str) -> Identity {
    Identity {
        label: email.into(),
        email: Some(email.into()),
        org_uuid: String::new(),
        org_name: None,
        account_uuid: None,
        raw: json!({"emailAddress": email}),
    }
}

/// An account whose identity key is `"<email>\n"`.
fn add(s: &Store, p: &ProviderId, id: &str, email: &str, pos: u32) -> AccountId {
    let aid = AccountId::from_string(id);
    let key = format!("{email}\n");
    s.insert_account(&NewAccount {
        id: &aid,
        provider: p,
        position: pos,
        identity_key: &key,
        identity: &identity(email),
        kind: "oauth",
        alias: None,
        login_expires_at: None,
        added_at: 1,
    })
    .unwrap();
    aid
}

fn cc() -> ProviderId {
    ProviderId::new("claude-code")
}

fn open() -> (tempfile::TempDir, PathBuf, Store) {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    (d, path, s)
}

/// A second, independent connection, to arrange and inspect rows the API has no call for.
fn raw(path: &Path) -> rusqlite::Connection {
    let c = rusqlite::Connection::open(path).unwrap();
    c.busy_timeout(Duration::from_secs(5)).unwrap();
    c
}

/// Arranges an account's `usage_state` schedule directly.
fn arrange(
    path: &Path,
    id: &AccountId,
    fetched_at: Option<i64>,
    backoff_until: Option<i64>,
    next_poll_at: Option<i64>,
) {
    raw(path)
        .execute(
            "INSERT INTO usage_state (account_id, fetched_at, backoff_until, next_poll_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params![id.as_str(), fetched_at, backoff_until, next_poll_at],
        )
        .unwrap();
}

fn reserve(s: &Store, id: &AccountId, now_ms: i64, on_demand: bool) -> Reserve {
    let row = s.account(id).unwrap().unwrap();
    s.reserve_usage(&row, now_ms, on_demand, &B).unwrap()
}

fn reserved(s: &Store, id: &AccountId, now_ms: i64) -> Reservation {
    match reserve(s, id, now_ms, true) {
        Reserve::Reserved(r) => r,
        other => panic!("expected a reservation, got {other:?}"),
    }
}

/// The first slot, the one `reserve_usage` took with the reservation.
fn slot_of(r: &Reservation) -> Slot {
    Slot {
        slot: r.slot,
        slot_at: r.slot_at,
    }
}

/// A further slot under `r` at `now_ms`, as the 401 retry asks for one (no slot held, no
/// token fingerprint).
fn another_slot(s: &Store, r: &Reservation, now_ms: i64) -> SendGrant {
    s.authorize_send(r, None, None, now_ms, &B).unwrap()
}

/// Takes the rest of the identity's hourly budget under `r`, at `now_ms`.
fn spend_the_hour(s: &Store, r: &Reservation, now_ms: i64) {
    while matches!(another_slot(s, r, now_ms), SendGrant::Send(_)) {}
}

/// The reservation times `usage_requests` holds for one identity, ascending.
fn slot_times(path: &Path, provider: &ProviderId, key: &str) -> Vec<i64> {
    let c = raw(path);
    let mut stmt = c
        .prepare(
            "SELECT at FROM usage_requests WHERE provider = ?1 AND identity_key = ?2 ORDER BY at",
        )
        .unwrap();
    stmt.query_map(params![provider.as_str(), key], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn all_slots(path: &Path) -> i64 {
    raw(path)
        .query_row("SELECT COUNT(*) FROM usage_requests", [], |r| r.get(0))
        .unwrap()
}

/// A lease row's holder and expiry (epoch ms).
fn lease(path: &Path, name: &str) -> Option<(String, i64)> {
    raw(path)
        .query_row(
            "SELECT holder, expires_at FROM leases WHERE name = ?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .unwrap()
}

fn window(key: &str, kind: WindowKind, pct: f64, resets_at: Option<i64>) -> Window {
    Window {
        key: key.into(),
        label: key.into(),
        kind,
        pct,
        resets_at,
        period_s: None,
        detail: None,
    }
}

fn windows() -> Vec<Window> {
    vec![
        Window {
            period_s: Some(18_000),
            ..window("5h", WindowKind::Short, 9.0, Some(T + 3_600))
        },
        Window {
            period_s: Some(604_800),
            ..window("7d", WindowKind::Long, 77.0, Some(T + 86_400))
        },
        Window {
            detail: Some(json!({"used": 3.5, "limit": 20.0, "currency": "EUR"})),
            ..window("spend", WindowKind::Spend, 17.5, None)
        },
    ]
}

fn plan() -> PollPlan {
    PollPlan {
        interval_s: 300,
        next_poll_at: T + 300,
    }
}

#[test]
fn reserving_takes_the_lease_and_one_slot() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let now_ms = T_MS + 400;
    let r = reserved(&s, &a, now_ms);
    assert_eq!(r.account_id, a);
    assert_eq!(r.provider, cc());
    assert_eq!(r.identity_key, "a@x.co\n");
    assert_eq!(r.slot_at, T, "slots are whole epoch seconds");
    assert_eq!(
        uuid::Uuid::parse_str(&r.holder).unwrap().get_version_num(),
        7
    );
    assert_eq!(
        lease(&path, "usage:a"),
        Some((r.holder.clone(), now_ms + 90_000)),
        "§6.1: the lease expiry is in milliseconds"
    );
    assert_eq!(slot_times(&path, &cc(), "a@x.co\n"), vec![T]);
    let rowid: i64 = raw(&path)
        .query_row("SELECT rowid FROM usage_requests", [], |row| row.get(0))
        .unwrap();
    assert_eq!(r.slot, rowid);
    assert!(s.usage_lease_live(&a, now_ms + 89_999).unwrap());
    assert!(!s.usage_lease_live(&a, now_ms + 90_000).unwrap());
    assert_eq!(
        s.usage_state(&a).unwrap(),
        None,
        "reserving records nothing"
    );
}

#[test]
fn a_quarantined_account_is_never_reserved() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    s.set_quarantine(&a, "invalid_grant", "sha256:sent", 1)
        .unwrap();
    for on_demand in [true, false] {
        assert_eq!(
            reserve(&s, &a, T_MS, on_demand),
            Reserve::Ineligible(Ineligible::Quarantined)
        );
    }
    assert_eq!(all_slots(&path), 0);
    assert_eq!(lease(&path, "usage:a"), None);
}

#[test]
fn backoff_holds_until_it_lifts() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    arrange(&path, &a, None, Some(T + 1), None);
    assert_eq!(
        reserve(&s, &a, T_MS, true),
        Reserve::Ineligible(Ineligible::Backoff)
    );
    assert_eq!(all_slots(&path), 0);
    assert!(matches!(
        reserve(&s, &a, T_MS + 1_000, true),
        Reserve::Reserved(_)
    ));
}

#[test]
fn a_live_lease_blocks_and_an_expired_one_is_taken_over() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    raw(&path)
        .execute(
            "INSERT INTO leases (name, holder, expires_at) VALUES ('usage:a', 'other', ?1)",
            [T_MS + 1],
        )
        .unwrap();
    assert_eq!(
        reserve(&s, &a, T_MS, true),
        Reserve::Ineligible(Ineligible::Leased)
    );
    assert_eq!(lease(&path, "usage:a"), Some(("other".into(), T_MS + 1)));
    assert_eq!(all_slots(&path), 0);
    let r = reserved(&s, &a, T_MS + 1);
    assert_eq!(lease(&path, "usage:a"), Some((r.holder, T_MS + 1 + 90_000)));
}

#[test]
fn on_demand_needs_an_old_reading_and_a_due_plan() {
    let (_d, path, s) = open();
    let cases = [
        // (fetched_at, next_poll_at, expected)
        (Some(T - 180), None, Some(Ineligible::NotDue)),
        (Some(T - 181), None, None),
        (Some(T - 1_000), Some(T + 1), Some(Ineligible::NotDue)),
        (Some(T - 1_000), Some(T), None),
        // A plan in force holds even without a reading, as an over-budget plan must (§8.6).
        (None, Some(T + 100), Some(Ineligible::NotDue)),
        (None, None, None),
    ];
    for (i, (fetched_at, next_poll_at, expected)) in cases.into_iter().enumerate() {
        let n = i as u32 + 1;
        let id = add(&s, &cc(), &format!("a{n}"), &format!("a{n}@x.co"), n);
        arrange(&path, &id, fetched_at, None, next_poll_at);
        let got = reserve(&s, &id, T_MS, true);
        match expected {
            Some(why) => assert_eq!(got, Reserve::Ineligible(why), "case {i}"),
            None => assert!(matches!(got, Reserve::Reserved(_)), "case {i}: {got:?}"),
        }
    }
}

#[test]
fn scheduled_collection_takes_a_due_plan_or_a_missing_reading() {
    let (_d, path, s) = open();
    let cases = [
        // (fetched_at, next_poll_at, expected)
        (Some(T - 10), Some(T), None),
        (Some(T - 10), Some(T + 1), Some(Ineligible::NotDue)),
        (Some(T - 10), None, None),
        (None, Some(T + 100), None),
    ];
    for (i, (fetched_at, next_poll_at, expected)) in cases.into_iter().enumerate() {
        let n = i as u32 + 1;
        let id = add(&s, &cc(), &format!("a{n}"), &format!("a{n}@x.co"), n);
        arrange(&path, &id, fetched_at, None, next_poll_at);
        let got = reserve(&s, &id, T_MS, false);
        match expected {
            Some(why) => assert_eq!(got, Reserve::Ineligible(why), "case {i}"),
            None => assert!(matches!(got, Reserve::Reserved(_)), "case {i}: {got:?}"),
        }
    }
}

#[test]
fn over_budget_moves_the_plan_and_takes_no_lease() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    spend_the_hour(&s, &r, T_MS);
    assert_eq!(slot_times(&path, &cc(), "a@x.co\n"), vec![T; 20]);
    let free = T + B.count_window_s;
    assert_eq!(
        another_slot(&s, &r, T_MS),
        SendGrant::OverBudget { next_free_at: free }
    );

    // The lease has expired and the account is due, but its identity has spent the hour.
    let later = T_MS + 91_000;
    assert_eq!(
        reserve(&s, &a, later, true),
        Reserve::OverBudget { next_free_at: free }
    );
    assert_eq!(s.usage_state(&a).unwrap().unwrap().next_poll_at, Some(free));
    assert_eq!(
        lease(&path, "usage:a"),
        Some((r.holder.clone(), T_MS + 90_000)),
        "no lease taken"
    );
    assert_eq!(all_slots(&path), 20);
    assert_eq!(
        reserve(&s, &a, later + 1_000, true),
        Reserve::Ineligible(Ineligible::NotDue),
        "the moved plan holds on-demand callers off until a slot frees"
    );

    // The budget is per (provider, identity key).
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    assert!(matches!(reserve(&s, &b, later, true), Reserve::Reserved(_)));
    let f = add(&s, &ProviderId::new("fake-agent"), "f", "a@x.co", 1);
    assert!(matches!(reserve(&s, &f, later, true), Reserve::Reserved(_)));
}

#[test]
fn a_row_leaves_the_count_exactly_one_window_after_it_was_reserved() {
    // §8.6 with Task 4's strict window: a row counts while `now − at < count_window_s`.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let r = reserved(&s, &a, T_MS);
    spend_the_hour(&s, &r, T_MS);
    reserved(&s, &b, T_MS);
    assert_eq!(all_slots(&path), 21);
    let edge = T + B.count_window_s;
    assert_eq!(
        another_slot(&s, &r, (edge - 1) * 1000),
        SendGrant::OverBudget { next_free_at: edge },
        "one second before, every row still counts"
    );
    let SendGrant::Send(slot) = another_slot(&s, &r, edge * 1000) else {
        panic!("a slot frees exactly one window on");
    };
    assert_eq!(slot.slot_at, edge);
    assert_eq!(
        slot_times(&path, &cc(), "a@x.co\n"),
        vec![edge],
        "rows that left are pruned on insert"
    );
    assert_eq!(all_slots(&path), 1, "every identity's, not just this one's");
}

#[test]
fn a_slot_still_valid_is_the_one_sent_under() {
    // §8.6: a slot is valid for `slot_valid_s`; within it, the send takes no second slot.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    let last_valid_ms = (T + B.slot_valid_s) * 1000;
    assert_eq!(
        s.authorize_send(&r, Some(&slot_of(&r)), Some("sha256:ok"), last_valid_ms, &B)
            .unwrap(),
        SendGrant::Send(slot_of(&r))
    );
    assert_eq!(slot_times(&path, &cc(), "a@x.co\n"), vec![T]);
}

#[test]
fn a_stale_slot_is_given_back_and_a_fresh_one_reserved() {
    // Review Focus 2: a sender suspended past `slot_valid_s` discards its slot and reserves
    // again, in the one authorization, so the budget counts the request once, at its fresh
    // time.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    let resumed = T + B.slot_valid_s + 1;
    assert!(resumed - r.slot_at > B.slot_valid_s);
    let SendGrant::Send(fresh) = s
        .authorize_send(&r, Some(&slot_of(&r)), Some("sha256:ok"), resumed * 1000, &B)
        .unwrap()
    else {
        panic!("a stale slot is replaced");
    };
    assert_eq!(fresh.slot_at, resumed);
    assert_ne!(fresh, slot_of(&r));
    assert_eq!(slot_times(&path, &cc(), "a@x.co\n"), vec![resumed], "the stale row is gone");
}

#[test]
fn over_budget_a_stale_slot_is_still_given_back() {
    // The stale slot goes back before the count, and stays gone when no fresh slot is free.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    let c = raw(&path);
    for _ in 0..B.hourly_requests {
        c.execute(
            "INSERT INTO usage_requests (provider, identity_key, at) VALUES ('claude-code', ?1, ?2)",
            params!["a@x.co\n", T + 1],
        )
        .unwrap();
    }
    let resumed = T + B.slot_valid_s + 1;
    assert_eq!(
        s.authorize_send(&r, Some(&slot_of(&r)), None, resumed * 1000, &B)
            .unwrap(),
        SendGrant::OverBudget {
            next_free_at: T + 1 + B.count_window_s
        }
    );
    assert_eq!(
        slot_times(&path, &cc(), "a@x.co\n"),
        vec![T + 1; B.hourly_requests as usize],
        "the stale slot went back and no fresh one was taken"
    );
}

#[test]
fn a_holder_that_lost_the_lease_or_the_identity_is_not_authorized() {
    // §8.3's fence, before every request: a sender suspended past its lease, or whose account
    // was re-logged meanwhile, sends nothing, and nothing is written: no slot is inserted and
    // the held one is left counted, as after any failed fence.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let first = reserved(&s, &a, T_MS);
    let second = reserved(&s, &a, T_MS + 90_000);
    let later = T_MS + 91_000;
    for slot in [Some(slot_of(&first)), None] {
        assert_eq!(
            s.authorize_send(&first, slot.as_ref(), Some("sha256:ok"), later, &B)
                .unwrap(),
            SendGrant::LeaseLost
        );
    }
    assert_eq!(all_slots(&path), 2, "no slot inserted, none given back");

    s.update_login(&a, "z@x.co\n", &identity("z@x.co"), "oauth", None)
        .unwrap();
    assert_eq!(
        s.authorize_send(&second, Some(&slot_of(&second)), None, later, &B)
            .unwrap(),
        SendGrant::LeaseLost,
        "the identity fence"
    );
    assert_eq!(all_slots(&path), 2);
}

#[test]
fn a_token_the_server_refused_is_not_authorized() {
    // §8.1: the durable `rejected_fp`, not the sender's own copy, decides, so a token refused
    // since the sender read its state is not sent.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    assert!(s.set_rejected_fp(&r, Some("sha256:refused")).unwrap());
    assert_eq!(
        s.authorize_send(&r, Some(&slot_of(&r)), Some("sha256:refused"), T_MS, &B)
            .unwrap(),
        SendGrant::Rejected
    );
    assert_eq!(
        s.authorize_send(&r, None, Some("sha256:refused"), T_MS, &B)
            .unwrap(),
        SendGrant::Rejected
    );
    assert_eq!(all_slots(&path), 1, "nothing written");
    for fp in [Some("sha256:other"), None] {
        assert_eq!(
            s.authorize_send(&r, Some(&slot_of(&r)), fp, T_MS, &B)
                .unwrap(),
            SendGrant::Send(slot_of(&r))
        );
    }
}

#[test]
fn a_released_slot_returns_to_the_budget() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    spend_the_hour(&s, &r, T_MS);
    assert!(matches!(
        another_slot(&s, &r, T_MS),
        SendGrant::OverBudget { .. }
    ));
    s.release_slot(&r, &slot_of(&r)).unwrap();
    assert_eq!(all_slots(&path), 19);
    assert!(matches!(another_slot(&s, &r, T_MS), SendGrant::Send(_)));
    assert_eq!(all_slots(&path), 20);
}

#[test]
fn a_stale_release_never_deletes_the_slot_that_reused_its_rowid() {
    // `usage_requests` has no AUTOINCREMENT: once a slot's row is pruned, SQLite gives its
    // rowid to the next insert. A process that slept past the count window and then hands its
    // slot back must not delete that other process's slot.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let r = reserved(&s, &a, T_MS);
    let later = T + B.count_window_s;
    let other = reserved(&s, &b, later * 1000);
    assert_eq!(other.slot, r.slot, "the pruned row's rowid was reused");

    s.release_slot(&r, &slot_of(&r)).unwrap();
    assert!(
        s.record_usage_failure(&r, "pre-send", later, later + 30, None, Some(&slot_of(&r)))
            .unwrap()
    );
    assert_eq!(
        slot_times(&path, &cc(), "b@x.co\n"),
        vec![later],
        "b's slot still counts"
    );
    assert_eq!(all_slots(&path), 1);
}

#[test]
fn a_successful_record_writes_the_reading_the_plan_and_samples() {
    let (_d, _path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    assert!(
        s.record_usage_failure(&r, "http-429", T, T + 400, Some(T + 400), None)
            .unwrap()
    );
    let failed = s.usage_state(&a).unwrap().unwrap();
    assert_eq!(
        (failed.consecutive_failures, failed.fetched_at),
        (1, None),
        "a first failure creates the row"
    );
    assert!(s.set_rejected_fp(&r, Some("sha256:rejected")).unwrap());

    assert!(s.record_usage(&r, &windows(), T + 5, &plan(), 180).unwrap());
    assert_eq!(
        s.usage_state(&a).unwrap().unwrap(),
        UsageStateRow {
            account_id: a.clone(),
            last_good: Some(windows()),
            fetched_at: Some(T + 5),
            last_attempt_at: Some(T + 5),
            consecutive_failures: 0,
            last_error: None,
            backoff_until: None,
            next_poll_at: Some(T + 300),
            poll_interval_s: Some(300),
            last_429_at: Some(T + 400),
            rejected_fp: None,
        },
        "success resets the failure fields and rejected_fp, and never clears last_429_at"
    );
    assert_eq!(
        s.usage_samples(&a, None, 0).unwrap(),
        vec![
            (
                "5h".to_owned(),
                Sample {
                    fetched_at: T + 5,
                    pct: 9.0,
                    resets_at: Some(T + 3_600)
                }
            ),
            (
                "7d".to_owned(),
                Sample {
                    fetched_at: T + 5,
                    pct: 77.0,
                    resets_at: Some(T + 86_400)
                }
            ),
            (
                "spend".to_owned(),
                Sample {
                    fetched_at: T + 5,
                    pct: 17.5,
                    resets_at: None
                }
            ),
        ]
    );
}

#[test]
fn samples_filter_by_window_and_time() {
    let (_d, _path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    assert!(s.record_usage(&r, &windows(), T, &plan(), 180).unwrap());
    assert!(
        s.record_usage(&r, &windows(), T + 600, &plan(), 180)
            .unwrap()
    );
    let times = |w: Option<&str>, since: i64| -> Vec<(String, i64)> {
        s.usage_samples(&a, w, since)
            .unwrap()
            .into_iter()
            .map(|(k, x)| (k, x.fetched_at))
            .collect()
    };
    assert_eq!(
        times(Some("7d"), 0),
        vec![("7d".into(), T), ("7d".into(), T + 600)]
    );
    assert_eq!(
        times(None, T + 600),
        vec![
            ("5h".into(), T + 600),
            ("7d".into(), T + 600),
            ("spend".into(), T + 600)
        ]
    );
    assert!(times(Some("scoped:Fable"), 0).is_empty());
}

#[test]
fn an_empty_reading_is_stored_as_no_windows_with_its_fetch_time() {
    let (_d, _path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    assert!(s.record_usage(&r, &[], T, &plan(), 180).unwrap());
    let st = s.usage_state(&a).unwrap().unwrap();
    assert_eq!((st.last_good, st.fetched_at), (None, Some(T)));
    assert!(s.usage_samples(&a, None, 0).unwrap().is_empty());
}

#[test]
fn a_corrupt_reading_reads_as_none() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    assert!(s.record_usage(&r, &windows(), T, &plan(), 180).unwrap());
    raw(&path)
        .execute("UPDATE usage_state SET last_good = '{not json'", [])
        .unwrap();
    let st = s.usage_state(&a).unwrap().unwrap();
    assert_eq!((st.last_good, st.fetched_at), (None, Some(T)));
}

#[test]
fn a_record_after_another_holder_took_the_lease_is_dropped() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let first = reserved(&s, &a, T_MS);
    let second = reserved(&s, &a, T_MS + 90_000);
    assert_ne!(first.holder, second.holder);
    assert!(
        !s.record_usage(&first, &windows(), T + 91, &plan(), 180)
            .unwrap()
    );
    assert!(
        !s.record_usage_failure(&first, "http-500", T + 91, T + 121, None, Some(&slot_of(&first)))
            .unwrap()
    );
    assert!(
        !s.set_rejected_fp(&first, Some("sha256:late")).unwrap(),
        "a lost holder stamps no refusal"
    );
    assert_eq!(s.usage_state(&a).unwrap(), None, "nothing written");
    assert!(s.usage_samples(&a, None, 0).unwrap().is_empty());
    assert_eq!(lease(&path, "prune:usage_samples"), None);
    assert_eq!(all_slots(&path), 2, "a failed fence gives no slot back");
    assert!(
        s.record_usage(&second, &windows(), T + 91, &plan(), 180)
            .unwrap()
    );
}

#[test]
fn a_record_for_a_changed_identity_is_dropped() {
    let (_d, _path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    s.update_login(&a, "z@x.co\n", &identity("z@x.co"), "oauth", None)
        .unwrap();
    assert!(!s.record_usage(&r, &windows(), T, &plan(), 180).unwrap());
    assert!(
        !s.record_usage_failure(&r, "http-500", T, T + 30, None, None)
            .unwrap()
    );
    assert!(!s.set_rejected_fp(&r, Some("sha256:x")).unwrap());
    assert_eq!(s.usage_state(&a).unwrap(), None);
}

#[test]
fn a_failure_never_touches_the_last_good_reading() {
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    assert!(s.record_usage(&r, &windows(), T, &plan(), 180).unwrap());
    let SendGrant::Send(unsent) = another_slot(&s, &r, T_MS + 1_000) else {
        panic!("a slot is free");
    };
    assert!(
        s.record_usage_failure(&r, "http-429", T + 1, T + 301, Some(T + 301), None)
            .unwrap()
    );
    assert!(
        s.record_usage_failure(&r, "pre-send", T + 2, T + 62, None, Some(&unsent))
            .unwrap()
    );
    let st = s.usage_state(&a).unwrap().unwrap();
    assert_eq!(st.last_good, Some(windows()));
    assert_eq!(st.fetched_at, Some(T));
    assert_eq!(st.last_attempt_at, Some(T + 2));
    assert_eq!(st.consecutive_failures, 2);
    assert_eq!(st.last_error.as_deref(), Some("pre-send"));
    assert_eq!(st.backoff_until, Some(T + 62));
    assert_eq!(st.last_429_at, Some(T + 301), "kept when not given");
    assert_eq!(st.next_poll_at, Some(T + 300), "the plan is untouched");
    assert_eq!(
        slot_times(&path, &cc(), "a@x.co\n"),
        vec![T],
        "the unsent slot is given back; the sent one still counts"
    );
}

#[test]
fn samples_are_pruned_at_most_once_a_day() {
    // Decision 7: the `prune:usage_samples` lease row spaces prunes a day apart.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    let plant = || {
        raw(&path)
            .execute(
                "INSERT INTO usage_samples (account_id, window, fetched_at, pct) \
                 VALUES ('a', 'old', ?1, 1.0)",
                [T - 2 * 86_400],
            )
            .unwrap();
    };
    let old = || s.usage_samples(&a, Some("old"), 0).unwrap().len();

    plant();
    assert!(s.record_usage(&r, &windows(), T, &plan(), 1).unwrap());
    assert_eq!(old(), 0, "the first record prunes");
    assert_eq!(
        lease(&path, "prune:usage_samples").map(|(_, at)| at),
        Some((T + 86_400) * 1000)
    );

    plant();
    assert!(
        s.record_usage(&r, &windows(), T + 3_600, &plan(), 1)
            .unwrap()
    );
    assert_eq!(old(), 1, "within the day, no second prune");

    assert!(
        s.record_usage(&r, &windows(), T + 86_400, &plan(), 1)
            .unwrap()
    );
    assert_eq!(old(), 0, "a day on, pruned again");
    let kept: Vec<i64> = s
        .usage_samples(&a, Some("7d"), 0)
        .unwrap()
        .into_iter()
        .map(|(_, x)| x.fetched_at)
        .collect();
    assert_eq!(
        kept,
        vec![T, T + 3_600, T + 86_400],
        "a sample exactly retention_days old is kept"
    );
}

#[test]
fn removing_and_re_adding_keeps_the_hours_count() {
    // Review Focus 4: usage rows cascade with the account, but the budget is keyed by
    // identity, so a re-added account cannot reset it.
    let (_d, path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s, &a, T_MS);
    assert!(s.record_usage(&r, &windows(), T, &plan(), 180).unwrap());
    spend_the_hour(&s, &r, T_MS);
    s.delete_account(&a).unwrap();
    assert_eq!(s.usage_state(&a).unwrap(), None);
    assert!(s.usage_samples(&a, None, 0).unwrap().is_empty());

    let again = add(&s, &cc(), "a2", "a@x.co", 2);
    assert_eq!(
        reserve(&s, &again, T_MS + 91_000, true),
        Reserve::OverBudget {
            next_free_at: T + B.count_window_s
        }
    );
    assert_eq!(slot_times(&path, &cc(), "a@x.co\n").len(), 20);
}

#[test]
fn two_stores_racing_for_slots_never_exceed_the_budget() {
    // Review Focus 1: two processes (two `Store` handles on one file) reserve at once.
    let (_d, path, s1) = open();
    let s2 = Store::open(&path).unwrap();
    let a = add(&s1, &cc(), "a", "a@x.co", 1);
    let r = reserved(&s1, &a, T_MS);
    let barrier = Barrier::new(16);
    let (granted, refused) = (AtomicUsize::new(0), AtomicUsize::new(0));
    std::thread::scope(|scope| {
        for i in 0..16 {
            let s = if i % 2 == 0 { &s1 } else { &s2 };
            let (r, barrier, granted, refused) = (&r, &barrier, &granted, &refused);
            scope.spawn(move || {
                barrier.wait();
                for _ in 0..5 {
                    match another_slot(s, r, T_MS) {
                        SendGrant::Send(_) => granted.fetch_add(1, SeqCst),
                        SendGrant::OverBudget { next_free_at } => {
                            assert_eq!(next_free_at, T + B.count_window_s);
                            refused.fetch_add(1, SeqCst)
                        }
                        other => panic!("unexpected {other:?}"),
                    };
                }
            });
        }
    });
    assert_eq!(granted.load(SeqCst), 19, "with the reservation's own, 20");
    assert_eq!(refused.load(SeqCst), 80 - 19);
    assert_eq!(all_slots(&path), 20);
}

#[test]
fn two_stores_racing_for_one_account_reserve_it_once() {
    // Review Focus 1: each account is fetched at most once per lease.
    let (_d, path, s1) = open();
    let s2 = Store::open(&path).unwrap();
    let a = add(&s1, &cc(), "a", "a@x.co", 1);
    let row = s1.account(&a).unwrap().unwrap();
    let barrier = Barrier::new(8);
    let (won, leased) = (AtomicUsize::new(0), AtomicUsize::new(0));
    std::thread::scope(|scope| {
        for i in 0..8 {
            let s = if i % 2 == 0 { &s1 } else { &s2 };
            let (row, barrier, won, leased) = (&row, &barrier, &won, &leased);
            scope.spawn(move || {
                barrier.wait();
                match s.reserve_usage(row, T_MS, true, &B).unwrap() {
                    Reserve::Reserved(_) => won.fetch_add(1, SeqCst),
                    Reserve::Ineligible(Ineligible::Leased) => leased.fetch_add(1, SeqCst),
                    other => panic!("unexpected {other:?}"),
                };
            });
        }
    });
    assert_eq!((won.load(SeqCst), leased.load(SeqCst)), (1, 7));
    assert_eq!(all_slots(&path), 1);
}

#[test]
fn set_poll_plan_sets_the_plan_alone_and_creates_the_row() {
    let (_d, _path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let active = PollPlan {
        interval_s: 180,
        next_poll_at: T + 180,
    };
    s.set_poll_plan(&a, &active).unwrap();
    assert_eq!(
        s.usage_state(&a).unwrap().unwrap(),
        UsageStateRow {
            account_id: a.clone(),
            last_good: None,
            fetched_at: None,
            last_attempt_at: None,
            consecutive_failures: 0,
            last_error: None,
            backoff_until: None,
            next_poll_at: Some(T + 180),
            poll_interval_s: Some(180),
            last_429_at: None,
            rejected_fp: None,
        }
    );

    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let r = reserved(&s, &b, T_MS);
    assert!(s.record_usage(&r, &windows(), T, &plan(), 180).unwrap());
    let candidate = PollPlan {
        interval_s: 600,
        next_poll_at: T + 600,
    };
    s.set_poll_plan(&b, &candidate).unwrap();
    let st = s.usage_state(&b).unwrap().unwrap();
    assert_eq!(
        (
            st.last_good,
            st.fetched_at,
            st.next_poll_at,
            st.poll_interval_s
        ),
        (Some(windows()), Some(T), Some(T + 600), Some(600))
    );

    assert!(matches!(
        s.set_poll_plan(&AccountId::from_string("nobody"), &active),
        Err(StoreError::NoSuchAccount)
    ));
}

#[test]
fn rejected_fp_is_stamped_and_cleared_only_by_the_lease_holder() {
    let (_d, _path, s) = open();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let first = reserved(&s, &a, T_MS);
    assert!(s.set_rejected_fp(&first, Some("sha256:refused")).unwrap());
    assert_eq!(
        s.usage_state(&a).unwrap().unwrap().rejected_fp.as_deref(),
        Some("sha256:refused"),
        "a first stamp creates the row"
    );
    assert!(s.set_rejected_fp(&first, None).unwrap());
    assert_eq!(s.usage_state(&a).unwrap().unwrap().rejected_fp, None);

    let second = reserved(&s, &a, T_MS + 90_000);
    assert!(
        !s.set_rejected_fp(&first, Some("sha256:late")).unwrap(),
        "the lease was taken over"
    );
    assert_eq!(s.usage_state(&a).unwrap().unwrap().rejected_fp, None);

    s.delete_account(&a).unwrap();
    assert!(
        !s.set_rejected_fp(&second, Some("sha256:x")).unwrap(),
        "a removed account fails the identity fence"
    );
}

#[test]
fn the_live_identity_cache_round_trips_per_provider() {
    let (_d, path, s) = open();
    assert_eq!(s.live_identity_cache(&cc()).unwrap(), None);
    let row = LiveIdentityCacheRow {
        provider: cc(),
        path: "/home/t/.claude.json".into(),
        mtime_ns: 1_790_000_000_123_456_789,
        size: 4_096,
        identity_key: Some("a@x.co\n".into()),
        label: Some("a@x.co".into()),
        account_uuid: Some("uuid-a".into()),
    };
    s.put_live_identity_cache(&row).unwrap();
    assert_eq!(s.live_identity_cache(&cc()).unwrap(), Some(row.clone()));

    let logged_out = LiveIdentityCacheRow {
        mtime_ns: row.mtime_ns + 1,
        size: 2,
        identity_key: None,
        label: None,
        account_uuid: None,
        ..row
    };
    s.put_live_identity_cache(&logged_out).unwrap();
    assert_eq!(s.live_identity_cache(&cc()).unwrap(), Some(logged_out));

    let fake = ProviderId::new("fake-agent");
    assert_eq!(s.live_identity_cache(&fake).unwrap(), None);
    raw(&path)
        .execute(
            "INSERT INTO live_identity_cache (provider) VALUES ('fake-agent')",
            [],
        )
        .unwrap();
    assert_eq!(
        s.live_identity_cache(&fake).unwrap(),
        None,
        "a row without its file key is a miss"
    );
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-engine --test store_usage`
Expected: FAIL to compile: ``unresolved imports `tagteam_engine::store::Ineligible`,
`tagteam_engine::store::LiveIdentityCacheRow`, `tagteam_engine::store::Reservation`,
`tagteam_engine::store::Reserve`, `tagteam_engine::store::SendGrant`,
`tagteam_engine::store::Slot`, `tagteam_engine::store::UsageStateRow` `` and ``no method
named `reserve_usage` found for reference `&tagteam_engine::store::Store` ``.

- [ ] **Step 3: Write the implementation**

Create `crates/tagteam-engine/src/store/usage.rs`:
```rust
//! The usage tables (§6.1): each account's usage state, the hourly request budget, samples,
//! the `usage:<id>` leases and the live-identity cache. Usage columns hold epoch seconds, and
//! lease expiries hold epoch milliseconds (Decision 1).

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use serde_json::Value;
use tagteam_core::poll::budget_next_free;
use tagteam_core::usage::{windows_from_json, windows_to_json};
use tagteam_core::{AccountId, PollBudget, PollPlan, ProviderId, Sample, Window};

use super::{AccountRow, Store, StoreError};

/// A usage lease lives 90 s (§8.3).
const USAGE_LEASE_MS: i64 = 90_000;

/// The lease row that spaces retention prunes at least a day apart (Decision 7).
const PRUNE_LEASE: &str = "prune:usage_samples";

const DAY_S: i64 = 86_400;

/// §6.1's lease statement. The lease is held when it changes one row: it was free, or its
/// holder's expiry (`?4`, now in ms) has passed.
const TAKE_LEASE_SQL: &str = "INSERT INTO leases (name, holder, expires_at) VALUES (?1, ?2, ?3) \
    ON CONFLICT(name) DO UPDATE SET holder = excluded.holder, expires_at = excluded.expires_at \
    WHERE leases.expires_at <= ?4";

/// One account's `usage_state` row. Times are epoch seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageStateRow {
    pub account_id: AccountId,
    /// The last successful reading. `None` when there is none, when the reading had no
    /// windows (§8.2), or when the stored JSON is corrupt: a bad row reads as no reading.
    pub last_good: Option<Vec<Window>>,
    /// Set by success only.
    pub fetched_at: Option<i64>,
    pub last_attempt_at: Option<i64>,
    pub consecutive_failures: u32,
    /// A kind token (§6.1, §8.3, Decision 10).
    pub last_error: Option<String>,
    pub backoff_until: Option<i64>,
    pub next_poll_at: Option<i64>,
    pub poll_interval_s: Option<i64>,
    /// When the last 429's backoff lifts (Decision 2). Success never clears it.
    pub last_429_at: Option<i64>,
    /// The access-token fingerprint a 401 refused (§8.1).
    pub rejected_fp: Option<String>,
}

/// A reservation (§8.3 phase 1): the `usage:<id>` lease and the first budget slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub account_id: AccountId,
    /// The account's provider: with `identity_key`, the budget's key (§8.6), and part of every
    /// slot's full identity.
    pub provider: ProviderId,
    /// The account's identity when it was reserved; the record is fenced by it.
    pub identity_key: String,
    /// Random per acquisition (UUIDv7): the lease row names it while the lease is ours.
    pub holder: String,
    /// The `usage_requests` rowid of the first slot.
    pub slot: i64,
    /// When the first slot was reserved, in epoch seconds.
    pub slot_at: i64,
}

/// Why an account was not reserved (§8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ineligible {
    Quarantined,
    Backoff,
    Leased,
    NotDue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reserve {
    Reserved(Reservation),
    Ineligible(Ineligible),
    /// The identity has spent its hourly budget (§8.6): no lease was taken, and the
    /// account's `next_poll_at` now says when a slot frees.
    OverBudget {
        next_free_at: i64,
    },
}

/// A budget slot under a held reservation. Its full identity is its rowid and time plus the
/// reservation's `provider` and `identity_key`: SQLite reuses a pruned row's rowid, so the rowid
/// alone does not name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// The `usage_requests` rowid.
    pub slot: i64,
    /// When it was reserved, in epoch seconds (the row's `at`).
    pub slot_at: i64,
}

/// What `authorize_send` allows right before a request (§8.3, §8.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendGrant {
    /// Send now, under this slot.
    Send(Slot),
    /// The token about to be sent is the one the server refused (`rejected_fp`, §8.1).
    Rejected,
    /// The lease row no longer names this holder, or the account's identity changed: send
    /// nothing, record nothing.
    LeaseLost,
    /// No slot is free in the identity's hourly budget until `next_free_at`.
    OverBudget { next_free_at: i64 },
}

/// The live identity as of one version of the provider's source file (§13.5). The file wins
/// if they disagree: a row whose `path`, `mtime_ns` or `size` differs from the file is stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveIdentityCacheRow {
    pub provider: ProviderId,
    pub path: String,
    pub mtime_ns: i64,
    pub size: i64,
    /// `None`: that version of the file held no live login.
    pub identity_key: Option<String>,
    pub label: Option<String>,
    pub account_uuid: Option<String>,
}

fn lease_name(id: &AccountId) -> String {
    format!("usage:{id}")
}

fn state_from_row(r: &Row<'_>) -> rusqlite::Result<UsageStateRow> {
    let last_good: Option<String> = r.get("last_good")?;
    let failures: i64 = r.get("consecutive_failures")?;
    Ok(UsageStateRow {
        account_id: AccountId::from_string(r.get::<_, String>("account_id")?),
        last_good: last_good
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| windows_from_json(&v)),
        fetched_at: r.get("fetched_at")?,
        last_attempt_at: r.get("last_attempt_at")?,
        consecutive_failures: u32::try_from(failures.max(0)).unwrap_or(u32::MAX),
        last_error: r.get("last_error")?,
        backoff_until: r.get("backoff_until")?,
        next_poll_at: r.get("next_poll_at")?,
        poll_interval_s: r.get("poll_interval_s")?,
        last_429_at: r.get("last_429_at")?,
        rejected_fp: r.get("rejected_fp")?,
    })
}

/// Creates the account's `usage_state` row when it has none. `NoSuchAccount` when the account
/// itself is gone.
fn ensure_state(c: &Connection, id: &AccountId) -> Result<(), StoreError> {
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = ?1)",
        [id.as_str()],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(StoreError::NoSuchAccount);
    }
    c.execute(
        "INSERT OR IGNORE INTO usage_state (account_id) VALUES (?1)",
        [id.as_str()],
    )?;
    Ok(())
}

/// Prunes every `usage_requests` row that has left the count window (§8.6), then says when the
/// identity may next send: `None` when a slot is free now. A row counts while
/// `now_s − at < count_window_s`, as `budget_next_free` counts it.
fn next_free_at(
    c: &Connection,
    provider: &str,
    identity_key: &str,
    now_s: i64,
    budget: &PollBudget,
) -> rusqlite::Result<Option<i64>> {
    let left = now_s - budget.count_window_s;
    c.execute("DELETE FROM usage_requests WHERE at <= ?1", [left])?;
    let mut stmt = c.prepare(
        "SELECT at FROM usage_requests WHERE provider = ?1 AND identity_key = ?2 AND at > ?3 \
         ORDER BY at",
    )?;
    let counted = stmt
        .query_map(params![provider, identity_key, left], |r| {
            r.get::<_, i64>(0)
        })?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(budget_next_free(budget, &counted, now_s))
}

fn insert_slot(
    c: &Connection,
    provider: &str,
    identity_key: &str,
    now_s: i64,
) -> rusqlite::Result<Slot> {
    c.execute(
        "INSERT INTO usage_requests (provider, identity_key, at) VALUES (?1, ?2, ?3)",
        params![provider, identity_key, now_s],
    )?;
    Ok(Slot {
        slot: c.last_insert_rowid(),
        slot_at: now_s,
    })
}

/// Gives a slot back by its full identity, in one statement. `usage_requests` has no
/// `AUTOINCREMENT`, so SQLite gives a pruned row's rowid to a later insert; matching the
/// provider, identity key and reservation time as well means a stale handle deletes nothing
/// rather than another process's slot.
fn delete_slot(c: &Connection, r: &Reservation, slot: &Slot) -> rusqlite::Result<usize> {
    c.execute(
        "DELETE FROM usage_requests \
         WHERE rowid = ?1 AND provider = ?2 AND identity_key = ?3 AND at = ?4",
        params![slot.slot, r.provider.as_str(), r.identity_key, slot.slot_at],
    )
}

/// §8.3's fence: the lease row still names this holder (expired or not, §6.1), and the account
/// still has the identity it was reserved with.
fn fenced(c: &Connection, r: &Reservation) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM leases WHERE name = ?1 AND holder = ?2) \
         AND EXISTS(SELECT 1 FROM accounts WHERE id = ?3 AND identity_key = ?4)",
        params![
            lease_name(&r.account_id),
            r.holder,
            r.account_id.as_str(),
            r.identity_key
        ],
        |row| row.get(0),
    )
}

/// History retention (§6.1): samples older than `retention_days` go, at most once a day
/// (Decision 7).
fn prune_samples(
    c: &Connection,
    holder: &str,
    now_s: i64,
    retention_days: u32,
) -> rusqlite::Result<()> {
    let taken = c.execute(
        TAKE_LEASE_SQL,
        params![PRUNE_LEASE, holder, (now_s + DAY_S) * 1000, now_s * 1000],
    )?;
    if taken == 1 {
        c.execute(
            "DELETE FROM usage_samples WHERE fetched_at < ?1",
            [now_s - i64::from(retention_days) * DAY_S],
        )?;
    }
    Ok(())
}

// Every transaction here reads before it writes, so each is `IMMEDIATE` (Decision 8): it
// takes the write lock at `BEGIN`, waiting out `busy_timeout`, so two processes can never
// both read a count or a lease row before either writes. A `DEFERRED` one would also fail at
// once with `SQLITE_BUSY` when another connection committed between its read and its write,
// since WAL mode cannot wait that conflict out.
impl Store {
    /// The account's usage state; `None` until something records one.
    pub fn usage_state(&self, id: &AccountId) -> Result<Option<UsageStateRow>, StoreError> {
        Ok(self
            .lock()
            .query_row(
                "SELECT * FROM usage_state WHERE account_id = ?1",
                [id.as_str()],
                state_from_row,
            )
            .optional()?)
    }

    /// Phase 1 (§8.3), one `IMMEDIATE` transaction. The account is re-read inside it.
    ///
    /// Eligibility, in this order: not quarantined, not in backoff, no live lease, then the
    /// schedule. An on-demand caller (`list`, `status`) needs the reading to be older than
    /// `floor_s` and a poll to be due, where no plan counts as due. A scheduled caller (M3)
    /// needs a poll to be due or no reading at all (§8.3's "due or stale"). An eligible
    /// account then needs a free slot in its identity's hourly budget (§8.6): over budget,
    /// its `next_poll_at` moves to `next_free_at` and no lease is taken. Otherwise the lease
    /// (§6.1's statement, 90 s) and one `usage_requests` slot are taken together.
    pub fn reserve_usage(
        &self,
        account: &AccountRow,
        now_ms: i64,
        on_demand: bool,
        budget: &PollBudget,
    ) -> Result<Reserve, StoreError> {
        let now_s = now_ms.div_euclid(1000);
        let id = &account.id;
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (provider, identity_key, quarantined): (String, String, bool) = tx
            .query_row(
                "SELECT provider, identity_key, quarantine_reason IS NOT NULL FROM accounts \
                 WHERE id = ?1",
                [id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or(StoreError::NoSuchAccount)?;
        if quarantined {
            return Ok(Reserve::Ineligible(Ineligible::Quarantined));
        }
        let (fetched_at, backoff_until, next_poll_at): (Option<i64>, Option<i64>, Option<i64>) = tx
            .query_row(
                "SELECT fetched_at, backoff_until, next_poll_at FROM usage_state \
                 WHERE account_id = ?1",
                [id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .unwrap_or_default();
        if backoff_until.is_some_and(|t| t > now_s) {
            return Ok(Reserve::Ineligible(Ineligible::Backoff));
        }
        let name = lease_name(id);
        let leased: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM leases WHERE name = ?1 AND expires_at > ?2)",
            params![name, now_ms],
            |r| r.get(0),
        )?;
        if leased {
            return Ok(Reserve::Ineligible(Ineligible::Leased));
        }
        let due = next_poll_at.is_none_or(|t| t <= now_s);
        let eligible = if on_demand {
            due && fetched_at.is_none_or(|t| now_s - t > budget.floor_s)
        } else {
            due || fetched_at.is_none()
        };
        if !eligible {
            return Ok(Reserve::Ineligible(Ineligible::NotDue));
        }
        if let Some(next_free_at) = next_free_at(&tx, &provider, &identity_key, now_s, budget)? {
            ensure_state(&tx, id)?;
            tx.execute(
                "UPDATE usage_state SET next_poll_at = ?2 WHERE account_id = ?1",
                params![id.as_str(), next_free_at],
            )?;
            tx.commit()?;
            return Ok(Reserve::OverBudget { next_free_at });
        }
        let holder = uuid::Uuid::now_v7().to_string();
        let taken = tx.execute(
            TAKE_LEASE_SQL,
            params![name, holder, now_ms + USAGE_LEASE_MS, now_ms],
        )?;
        if taken != 1 {
            return Ok(Reserve::Ineligible(Ineligible::Leased));
        }
        let slot = insert_slot(&tx, &provider, &identity_key, now_s)?;
        tx.commit()?;
        Ok(Reserve::Reserved(Reservation {
            account_id: id.clone(),
            provider: ProviderId::new(provider),
            identity_key,
            holder,
            slot: slot.slot,
            slot_at: slot.slot_at,
        }))
    }

    /// Right before each request (§8.3, §8.6), in one `IMMEDIATE` transaction, so nothing is
    /// sent on a stale view of the store. In order:
    /// - `r` must still hold the `usage:<id>` lease (its row names `r.holder`) and the account
    ///   must still have `r.identity_key`; otherwise `LeaseLost`, and nothing is written.
    /// - `access_fp`, the fingerprint of the exact bytes about to be sent, must not equal the
    ///   durable `rejected_fp` (§8.1), which another holder may have stamped since the caller
    ///   read it; otherwise `Rejected`, and nothing is written.
    /// - Then the slot to send under: `slot` itself while it is at most `slot_valid_s` old;
    ///   otherwise a fresh one, after giving the stale one back by its full identity; a fresh
    ///   one when `slot` is `None` (the 401 retry, whose first slot was sent). Counted against
    ///   the reservation's `(provider, identity_key)`. `OverBudget` when no fresh slot is free;
    ///   the stale one has still been given back.
    pub fn authorize_send(
        &self,
        r: &Reservation,
        slot: Option<&Slot>,
        access_fp: Option<&str>,
        now_ms: i64,
        budget: &PollBudget,
    ) -> Result<SendGrant, StoreError> {
        let now_s = now_ms.div_euclid(1000);
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !fenced(&tx, r)? {
            return Ok(SendGrant::LeaseLost);
        }
        if let Some(fp) = access_fp {
            let rejected: Option<String> = tx
                .query_row(
                    "SELECT rejected_fp FROM usage_state WHERE account_id = ?1",
                    [r.account_id.as_str()],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            if rejected.as_deref() == Some(fp) {
                return Ok(SendGrant::Rejected);
            }
        }
        if let Some(held) = slot {
            if now_s - held.slot_at <= budget.slot_valid_s {
                return Ok(SendGrant::Send(held.clone()));
            }
            delete_slot(&tx, r, held)?;
        }
        let provider = r.provider.as_str();
        let grant = match next_free_at(&tx, provider, &r.identity_key, now_s, budget)? {
            Some(next_free_at) => SendGrant::OverBudget { next_free_at },
            None => SendGrant::Send(insert_slot(&tx, provider, &r.identity_key, now_s)?),
        };
        tx.commit()?;
        Ok(grant)
    }

    /// Gives a slot back (a fetch that ended before sending, §8.3) by its full identity, in one
    /// statement: a slot whose row was pruned deletes nothing, even when SQLite has since given
    /// its rowid to another slot.
    pub fn release_slot(&self, r: &Reservation, slot: &Slot) -> Result<(), StoreError> {
        delete_slot(&self.lock(), r, slot)?;
        Ok(())
    }

    /// Phase 3, success (§8.3). Fenced by the lease holder and the account's identity key:
    /// `Ok(false)` when the fence fails, and nothing is written. Writes the reading (a reading
    /// with no windows is stored as none, §8.2) and its `fetched_at` and `last_attempt_at`,
    /// resets the failure fields and `rejected_fp`, stores `plan`, inserts one sample per
    /// window, and prunes samples older than `retention_days` at most once a day (Decision 7).
    /// `last_429_at` is kept. The lease is left to expire.
    pub fn record_usage(
        &self,
        r: &Reservation,
        windows: &[Window],
        now_s: i64,
        plan: &PollPlan,
        retention_days: u32,
    ) -> Result<bool, StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !fenced(&tx, r)? {
            return Ok(false);
        }
        let id = r.account_id.as_str();
        let last_good = (!windows.is_empty()).then(|| windows_to_json(windows).to_string());
        ensure_state(&tx, &r.account_id)?;
        tx.execute(
            "UPDATE usage_state SET last_good = ?2, fetched_at = ?3, last_attempt_at = ?3, \
             consecutive_failures = 0, last_error = NULL, backoff_until = NULL, \
             next_poll_at = ?4, poll_interval_s = ?5, rejected_fp = NULL WHERE account_id = ?1",
            params![id, last_good, now_s, plan.next_poll_at, plan.interval_s],
        )?;
        for w in windows {
            tx.execute(
                "INSERT OR REPLACE INTO usage_samples (account_id, window, fetched_at, pct, resets_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, w.key, now_s, w.pct, w.resets_at],
            )?;
        }
        prune_samples(&tx, &r.holder, now_s, retention_days)?;
        tx.commit()?;
        Ok(true)
    }

    /// Phase 3, failure (§8.3), under the same fence. Never touches `last_good` or
    /// `fetched_at`. Counts the failure and sets `last_error` (a kind token),
    /// `last_attempt_at` and `backoff_until`, and `last_429_at` when given (Decision 2: when
    /// that 429's backoff lifts). `release`, the slot of a request that was never sent, is
    /// given back by its full identity in the same transaction. The plan is left as it was.
    pub fn record_usage_failure(
        &self,
        r: &Reservation,
        kind: &str,
        now_s: i64,
        backoff_until: i64,
        last_429_at: Option<i64>,
        release: Option<&Slot>,
    ) -> Result<bool, StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !fenced(&tx, r)? {
            return Ok(false);
        }
        ensure_state(&tx, &r.account_id)?;
        tx.execute(
            "UPDATE usage_state SET consecutive_failures = consecutive_failures + 1, \
             last_error = ?2, last_attempt_at = ?3, backoff_until = ?4, \
             last_429_at = COALESCE(?5, last_429_at) WHERE account_id = ?1",
            params![
                r.account_id.as_str(),
                kind,
                now_s,
                backoff_until,
                last_429_at
            ],
        )?;
        if let Some(slot) = release {
            delete_slot(&tx, r, slot)?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// Stamps (or with `None`, clears) the access-token fingerprint a 401 refused (§8.1), only
    /// while `r` holds the lease, under the records' fence: `Ok(false)` and nothing written
    /// otherwise, so a holder that lost its lease cannot overwrite the new holder's stamp.
    /// Creates the account's row if missing (a 401 can come on its first fetch).
    pub fn set_rejected_fp(&self, r: &Reservation, fp: Option<&str>) -> Result<bool, StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !fenced(&tx, r)? {
            return Ok(false);
        }
        ensure_state(&tx, &r.account_id)?;
        tx.execute(
            "UPDATE usage_state SET rejected_fp = ?2 WHERE account_id = ?1",
            params![r.account_id.as_str(), fp],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// §8.3's post-switch re-plan: sets the plan alone, creating the row if missing.
    pub fn set_poll_plan(&self, id: &AccountId, plan: &PollPlan) -> Result<(), StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_state(&tx, id)?;
        tx.execute(
            "UPDATE usage_state SET next_poll_at = ?2, poll_interval_s = ?3 WHERE account_id = ?1",
            params![id.as_str(), plan.next_poll_at, plan.interval_s],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The account's samples fetched at or after `since_s`, ascending by `fetched_at` (then
    /// window key); `window = None` means every window.
    pub fn usage_samples(
        &self,
        id: &AccountId,
        window: Option<&str>,
        since_s: i64,
    ) -> Result<Vec<(String, Sample)>, StoreError> {
        let c = self.lock();
        let mut stmt = c.prepare(
            "SELECT window, fetched_at, pct, resets_at FROM usage_samples \
             WHERE account_id = ?1 AND fetched_at >= ?2 AND (?3 IS NULL OR window = ?3) \
             ORDER BY fetched_at, window",
        )?;
        let rows = stmt
            .query_map(params![id.as_str(), since_s, window], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    Sample {
                        fetched_at: r.get(1)?,
                        pct: r.get(2)?,
                        resets_at: r.get(3)?,
                    },
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Whether another fetch holds the account's lease at `now_ms` (§8.4's extended trust).
    pub fn usage_lease_live(&self, id: &AccountId, now_ms: i64) -> Result<bool, StoreError> {
        Ok(self.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM leases WHERE name = ?1 AND expires_at > ?2)",
            params![lease_name(id), now_ms],
            |r| r.get(0),
        )?)
    }

    /// The provider's cached live identity. A row missing its path, mtime or size reads as
    /// no row: the caller re-parses the file.
    pub fn live_identity_cache(
        &self,
        provider: &ProviderId,
    ) -> Result<Option<LiveIdentityCacheRow>, StoreError> {
        let row = self
            .lock()
            .query_row(
                "SELECT path, mtime_ns, size, identity_key, label, account_uuid \
                 FROM live_identity_cache WHERE provider = ?1",
                [provider.as_str()],
                |r| {
                    let key: (Option<String>, Option<i64>, Option<i64>) =
                        (r.get(0)?, r.get(1)?, r.get(2)?);
                    Ok((key, r.get(3)?, r.get(4)?, r.get(5)?))
                },
            )
            .optional()?;
        Ok(match row {
            Some(((Some(path), Some(mtime_ns), Some(size)), identity_key, label, account_uuid)) => {
                Some(LiveIdentityCacheRow {
                    provider: provider.clone(),
                    path,
                    mtime_ns,
                    size,
                    identity_key,
                    label,
                    account_uuid,
                })
            }
            _ => None,
        })
    }

    pub fn put_live_identity_cache(&self, row: &LiveIdentityCacheRow) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT INTO live_identity_cache \
             (provider, path, mtime_ns, size, identity_key, label, account_uuid) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(provider) DO UPDATE SET path = excluded.path, \
             mtime_ns = excluded.mtime_ns, size = excluded.size, \
             identity_key = excluded.identity_key, label = excluded.label, \
             account_uuid = excluded.account_uuid",
            params![
                row.provider.as_str(),
                row.path,
                row.mtime_ns,
                row.size,
                row.identity_key,
                row.label,
                row.account_uuid
            ],
        )?;
        Ok(())
    }
}
```

In `crates/tagteam-engine/src/store/mod.rs`, insert between the
`use tagteam_provider::{Identity, ProcessStamp};` line and `const SCHEMA_V1` (one blank line on
each side):
```rust
mod usage;

pub use usage::{
    Ineligible, LiveIdentityCacheRow, Reservation, Reserve, SendGrant, Slot, UsageStateRow,
};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run:
```
cargo test -p tagteam-engine --test store_usage
cargo test -p tagteam-engine
```
Expected: PASS, 29 tests in `store_usage`, including
`two_stores_racing_for_slots_never_exceed_the_budget` and
`two_stores_racing_for_one_account_reserve_it_once` (Review Focus 1),
`a_stale_slot_is_given_back_and_a_fresh_one_reserved`,
`a_holder_that_lost_the_lease_or_the_identity_is_not_authorized` and
`a_stale_release_never_deletes_the_slot_that_reused_its_rowid` (Review Focus 2), and
`removing_and_re_adding_keeps_the_hours_count` (Review Focus 4). Every other engine suite still
passes.

Optionally, confirm that the race tests pin Decision 8. Change every
`TransactionBehavior::Immediate` in `usage.rs` to `TransactionBehavior::Deferred` and run
`cargo test -p tagteam-engine --test store_usage two_stores`. Both tests fail: a racing
thread's `unwrap` meets `SQLITE_BUSY`. Then change them back.

- [ ] **Step 5: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.
Every new item is `pub` on `Store`, so nothing is dead code before Task 10 calls it.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/store/mod.rs crates/tagteam-engine/src/store/usage.rs \
  crates/tagteam-engine/tests/store_usage.rs
git commit -m "Add usage reservations, fenced send authorization, records, samples and the live-identity cache to the store"
```

---

### Task 9: Lazy HTTP client and settings in the engine

§13.5 requires that `statusline` never construct the `Http` adapter. Today `build_engine` builds
`UreqHttp` eagerly on every run and hands it to both the engine and the oracle. `LazyHttp`
keeps the factory behind a `OnceLock`: the first `send` builds the adapter, exactly once, even
when several first sends race (the collector's threads, Task 10). The engine also gains the
`Settings` that Task 1 reads, and the CLI loads them once per run.

Rulings:
- **One shared lazy client.** The engine port and `HttpOracle` get the same `Arc<LazyHttp>`.
  A command that asks the oracle and then fetches usage builds one adapter, as it does today.
- **Settings are loaded for the default provider** (`claude-code`), as this task's scope
  says. It is the only provider the binary registers, so in M2b `--provider` has no other
  override table it could select.
- **`build_engine` returns the warnings, and `run` prints them** to stderr as `warning: {w}`
  before dispatch, for every command. Task 16's `statusline` fast path must stay silent: it
  either bypasses `build_engine` or drops the warnings.
- **`EngineConfig` has no `Default`**, so every literal gains `settings: Settings::default()`.
  `engine.rs`'s unit tests get a `test_config` helper, so a test can override one field.

**Files:**
- Create: `crates/tagteam-engine/src/lazy_http.rs`
- Modify: `crates/tagteam-engine/src/lib.rs`, `crates/tagteam-engine/src/engine.rs`,
  `crates/tagteam-engine/src/testutil.rs`, `crates/tagteam-engine/tests/common/mod.rs`,
  `crates/tagteam-engine/tests/oracle.rs`, `crates/tagteam/src/app.rs`,
  `crates/tagteam/tests/app.rs`

**Interfaces:**
- Consumes:
  - Task 1: `tagteam_engine::settings::Settings`, its `Default` (§6.4's defaults) and
    `Settings::load(env: &Env, provider: &ProviderId) -> (Settings, Vec<String>)`.
  - Existing: `tagteam_provider::http::{Http, HttpRequest, HttpResponse, HttpError}`,
    `UreqHttp::{new, direct}`, `HttpOracle::new`, `CachingOracle::new`.
- Produces (the contract's signatures):
```rust
// tagteam_engine::lazy_http
pub struct LazyHttp { /* OnceLock<Arc<dyn Http>>, factory */ }
impl LazyHttp {
    pub fn new(factory: impl Fn() -> Arc<dyn Http> + Send + Sync + 'static) -> Self;
    pub fn is_built(&self) -> bool;
}
impl Http for LazyHttp { /* builds once, then delegates */ }

// tagteam_engine::engine
pub struct EngineConfig { /* … */ pub settings: Settings }
impl Engine { pub fn settings(&self) -> &Settings; }
```
  The CLI's private `fn build_engine(ctx: Context) -> (Engine, Vec<String>)` also changes; `run`
  prints each warning on stderr as `warning: {w}`.

- [ ] **Step 1: Write the failing engine tests**

Create `crates/tagteam-engine/src/lazy_http.rs` holding only the test module:
```rust
#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use serde_json::json;
    use tagteam_cc::ClaudeCode;
    use tagteam_cc::live::{LiveStore, Platform};
    use tagteam_core::{CLAUDE_CODE, ProviderId};
    use tagteam_provider::http::{Method, NoHttp, ScriptedHttp};
    use tagteam_provider::{Clock, Env, FakeClock, FakeKeychain};

    use super::*;
    use crate::engine::{Engine, EngineConfig};
    use crate::oracle::{CachingOracle, HttpOracle};
    use crate::registry::ProviderRegistry;
    use crate::settings::Settings;
    use crate::vault::{KeychainVault, Vault};
    use crate::views::StatusView;

    const URL: &str = "https://usage.invalid/ping";

    /// A lazy client whose factory hands out `inner`, and how many times it has run.
    fn counting(inner: Arc<dyn Http>) -> (Arc<LazyHttp>, Arc<AtomicUsize>) {
        let builds = Arc::new(AtomicUsize::new(0));
        let counter = builds.clone();
        let lazy = LazyHttp::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            inner.clone()
        });
        (Arc::new(lazy), builds)
    }

    fn ping() -> HttpRequest {
        HttpRequest::get(URL, Duration::from_secs(5))
    }

    #[test]
    fn the_adapter_is_built_on_the_first_send_and_only_then() {
        let scripted = Arc::new(ScriptedHttp::new());
        scripted.push_json(Method::Get, URL, 200, json!({"ok": true}));
        let (lazy, builds) = counting(scripted.clone());
        assert!(!lazy.is_built());
        assert_eq!(builds.load(Ordering::SeqCst), 0);

        assert_eq!(lazy.send(&ping()).unwrap().status, 200);
        assert!(lazy.is_built());
        assert_eq!(lazy.send(&ping()).unwrap().status, 200);
        assert_eq!(builds.load(Ordering::SeqCst), 1, "built once, then reused");
        assert_eq!(
            scripted.count(Method::Get, URL),
            2,
            "every send reaches the adapter"
        );
    }

    #[test]
    fn concurrent_first_sends_build_it_once() {
        let builds = Arc::new(AtomicUsize::new(0));
        let counter = builds.clone();
        let lazy = LazyHttp::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            // Widens the window in which a second builder could start.
            std::thread::sleep(Duration::from_millis(20));
            Arc::new(NoHttp) as Arc<dyn Http>
        });
        let barrier = Barrier::new(8);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    barrier.wait();
                    assert!(lazy.send(&ping()).is_err(), "NoHttp refuses every send");
                });
            }
        });
        assert_eq!(builds.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_engine_over_a_lazy_client_does_not_build_it() {
        // §13.5: building the engine, its oracle included, and reading a view send nothing,
        // so the adapter is never constructed.
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let kc = Arc::new(FakeKeychain::new());
        let (lazy, builds) = counting(Arc::new(NoHttp));
        let http: Arc<dyn Http> = lazy.clone();
        let clock: Arc<dyn Clock> = Arc::new(FakeClock::new(1_790_000_000_000));
        let engine = Engine::new(EngineConfig {
            env,
            registry: ProviderRegistry::new().with(Arc::new(ClaudeCode::with_store(
                LiveStore::new(kc.clone(), Platform::MacOs),
            ))),
            vault: Vault::new(Box::new(KeychainVault::new(kc))),
            oracle: Arc::new(CachingOracle::new(HttpOracle::new(
                http.clone(),
                clock.clone(),
            ))),
            clock,
            http,
            default_provider: ProviderId::new(CLAUDE_CODE),
            settings: Settings::default(),
        });
        assert!(matches!(
            engine.status(&ProviderId::new(CLAUDE_CODE)).unwrap(),
            StatusView::NoLogin
        ));
        assert!(!lazy.is_built());
        assert_eq!(builds.load(Ordering::SeqCst), 0);
    }
}
```

In `crates/tagteam-engine/src/lib.rs`, insert `pub mod lazy_http;` between `mod hooks;` and
`pub mod lifecycle;`.

In `crates/tagteam-engine/src/engine.rs`'s `mod tests`, replace `fn test_engine` with this
config helper, the engine built from it, and a test of the accessor:
```rust
    fn test_config(env: Env) -> EngineConfig {
        EngineConfig {
            env,
            registry: ProviderRegistry::new(),
            vault: Vault::new(Box::new(KeychainVault::new(Arc::new(FakeKeychain::new())))),
            oracle: Arc::new(NoOracle),
            clock: Arc::new(tagteam_provider::SystemClock),
            http: Arc::new(tagteam_provider::NoHttp),
            default_provider: ProviderId::new("p"),
            settings: Settings::default(),
        }
    }

    fn test_engine(env: Env) -> Engine {
        Engine::new(test_config(env))
    }

    #[test]
    fn the_engine_keeps_the_settings_it_was_built_with() {
        let d = tempfile::tempdir().unwrap();
        let settings = Settings {
            threshold: 75.0,
            models: vec!["Fable".into()],
            ..Settings::default()
        };
        let engine = Engine::new(EngineConfig {
            settings: settings.clone(),
            ..test_config(Env::for_test(d.path()))
        });
        assert_eq!(engine.settings(), &settings);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-engine --lib -- lazy_http the_engine_keeps_the_settings`
Expected: FAIL to compile: ``cannot find type `LazyHttp` in this scope`` (along with `Arc` and
`Http`, which the implementation's imports bring in), ``struct `EngineConfig` has no field
named `settings` `` and ``no method named `settings` found``.

- [ ] **Step 3: Write `LazyHttp`**

Insert the following at the top of `crates/tagteam-engine/src/lazy_http.rs`, above
`#[cfg(test)]` (leave one blank line between the two parts):
```rust
//! The `Http` port, built on first use (§13.5): a command that sends nothing, `statusline`
//! above all, never pays for constructing the TLS client.

use std::sync::{Arc, OnceLock};

use tagteam_provider::http::{Http, HttpError, HttpRequest, HttpResponse};

/// Builds the real adapter on the first `send`, never before (§13.5). Concurrent first sends
/// build it once; every send after that goes to the same adapter.
pub struct LazyHttp {
    built: OnceLock<Arc<dyn Http>>,
    factory: Box<dyn Fn() -> Arc<dyn Http> + Send + Sync>,
}

impl LazyHttp {
    pub fn new(factory: impl Fn() -> Arc<dyn Http> + Send + Sync + 'static) -> Self {
        Self {
            built: OnceLock::new(),
            factory: Box::new(factory),
        }
    }

    /// Whether a send has built the adapter yet.
    pub fn is_built(&self) -> bool {
        self.built.get().is_some()
    }
}

impl Http for LazyHttp {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self.built.get_or_init(|| (self.factory)()).send(req)
    }
}
```

- [ ] **Step 4: Give the engine its settings**

In `crates/tagteam-engine/src/engine.rs`:

1. Add `use crate::settings::Settings;` after `use crate::registry::ProviderRegistry;`.
2. In `EngineConfig`, after `pub default_provider: ProviderId,`, add:
```rust
    /// `config.toml` as read for this command (§6.4).
    pub settings: Settings,
```
3. In `Engine`, after `pub(crate) default_provider: ProviderId,`, add:
```rust
    pub(crate) settings: Settings,
```
4. In `Engine::new`, after `default_provider: cfg.default_provider,`, add:
```rust
            settings: cfg.settings,
```
5. After `Engine::now_ms`, add:
```rust
    /// The settings this engine was built with (§6.4).
    pub fn settings(&self) -> &Settings {
        &self.settings
    }
```

- [ ] **Step 5: Pass `Settings::default()` at every other `EngineConfig` literal**

Run: `grep -rn 'EngineConfig {' crates/`
Besides `engine.rs` (Steps 1 and 4) and `lazy_http.rs` (Step 1), the hits when this plan was
written are:
- `crates/tagteam-engine/src/testutil.rs` (`T::new`)
- `crates/tagteam-engine/tests/common/mod.rs`: `Fx::build`, `Fx::engine_over` and
  `FakeFx::new`
- `crates/tagteam-engine/tests/oracle.rs` (the hung-profile test's engine)
- `crates/tagteam/src/app.rs` (`build_engine`; Step 9 replaces it)

In each, add `settings: Settings::default(),` as the literal's last field, and import
`Settings`:
- `testutil.rs`: `use crate::settings::Settings;` after `use crate::registry::ProviderRegistry;`
- `tests/common/mod.rs`, `tests/oracle.rs` and `crates/tagteam/src/app.rs`:
  `use tagteam_engine::settings::Settings;` after `use tagteam_engine::registry::ProviderRegistry;`

If the grep shows any other hit (a literal added since), give it the same field.

- [ ] **Step 6: Run the engine tests to verify they pass**

Run:
```
cargo test -p tagteam-engine --lib -- lazy_http the_engine_keeps_the_settings
cargo test --workspace
```
Expected: PASS: `the_adapter_is_built_on_the_first_send_and_only_then`,
`concurrent_first_sends_build_it_once`, `an_engine_over_a_lazy_client_does_not_build_it` and
`the_engine_keeps_the_settings_it_was_built_with`. Every other suite still passes.

- [ ] **Step 7: Run the checks and commit**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

```bash
git add crates/tagteam-engine/src/lazy_http.rs crates/tagteam-engine/src/lib.rs \
  crates/tagteam-engine/src/engine.rs crates/tagteam-engine/src/testutil.rs \
  crates/tagteam-engine/tests/common/mod.rs crates/tagteam-engine/tests/oracle.rs \
  crates/tagteam/src/app.rs
git commit -m "Add a lazily built HTTP client and give the engine its settings"
```

- [ ] **Step 8: Write the failing CLI test**

In `crates/tagteam/tests/app.rs`, after
`use tagteam_cc::{CcPaths, ItemKind, keychain_account, keychain_service};`, add:
```rust
use tagteam_core::{CLAUDE_CODE, ProviderId};
use tagteam_engine::settings::Settings;
```
and append:
```rust
#[test]
fn settings_warnings_go_to_stderr_and_never_fail_the_command() {
    // §6.4: reads are forgiving. Each warning is one stderr line; stdout is unchanged, and
    // `--json` still prints exactly one object (§13.2).
    let h = H::new();
    let (code, _, err) = h.run(&["list"], &mut Scripted::none());
    assert_eq!((code, err.as_str()), (0, ""), "no config.toml, no warning");

    let dir = h.env.config_dir();
    std::fs::create_dir_all(&dir).unwrap();
    for text in ["[autoswitch]\nthreshold = 5\n", "[autoswitch\n"] {
        std::fs::write(dir.join("config.toml"), text).unwrap();
        let (_, warnings) = Settings::load(&h.env, &ProviderId::new(CLAUDE_CODE));
        assert_eq!(warnings.len(), 1, "{text:?}: {warnings:?}");
        let expected: String = warnings.iter().map(|w| format!("warning: {w}\n")).collect();
        let (code, _, err) = h.run(&["list"], &mut Scripted::none());
        assert_eq!((code, err.as_str()), (0, expected.as_str()), "{text:?}");
        let (code, out, err) = h.run(&["--json", "list"], &mut Scripted::none());
        assert_eq!((code, err.as_str()), (0, expected.as_str()), "{text:?}");
        serde_json::from_str::<Value>(&out).expect("stdout stays one JSON object");
    }
}
```

Run: `cargo test -p tagteam --test app settings_warnings`
Expected: FAIL: ``assertion `left == right` failed: "[autoswitch]\nthreshold = 5\n"``, with
`left: (0, "")` and ``right: (0, "warning: config.toml: `autoswitch.threshold` must be a number
from 50 to 99.9 (ignored)\n")``. Nothing prints the warnings yet. The expected line is built
from `Settings::load`, so the test follows Task 1's wording.

- [ ] **Step 9: Build the engine lazily and print the warnings**

In `crates/tagteam/src/app.rs`, add `use tagteam_engine::lazy_http::LazyHttp;` before
`use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};`, and replace `build_engine`
(its doc comment is new):
```rust
/// The engine for one command, and the settings warnings for the caller to print (§6.4). The
/// HTTP adapter is built on its first request, never before (§13.5), and the oracle sends
/// through the same one.
fn build_engine(ctx: Context) -> (Engine, Vec<String>) {
    let mut cc = ClaudeCode::new(ctx.keychain.clone(), ctx.platform);
    if let Some(base) = &ctx.api_base {
        cc = cc.with_endpoints(Endpoints::with_base(base));
    }
    let vault = match ctx.platform {
        Platform::MacOs => Vault::new(Box::new(KeychainVault::new(ctx.keychain))),
        Platform::Linux => Vault::new(Box::new(FileVault::new(ctx.env.data_dir().join("vault")))),
    };
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    // `api_base` only ever carries a test base: it is sent to directly, never through whatever
    // proxy the machine's environment names, so test traffic cannot leave the machine.
    let direct = ctx.api_base.is_some();
    let http: Arc<dyn Http> = Arc::new(LazyHttp::new(move || -> Arc<dyn Http> {
        Arc::new(if direct {
            UreqHttp::direct()
        } else {
            UreqHttp::new()
        })
    }));
    let default_provider = ProviderId::new(CLAUDE_CODE);
    let (settings, warnings) = Settings::load(&ctx.env, &default_provider);
    let engine = Engine::new(EngineConfig {
        env: ctx.env,
        registry: ProviderRegistry::new().with(Arc::new(cc)),
        vault,
        // §7.6: asked at most once per credential within one command.
        oracle: Arc::new(CachingOracle::new(HttpOracle::new(
            http.clone(),
            clock.clone(),
        ))),
        clock,
        http,
        default_provider,
        settings,
    });
    (engine, warnings)
}
```

In `run`, replace:
```rust
    let mut app = App {
        engine: build_engine(ctx),
```
with:
```rust
    let (engine, warnings) = build_engine(ctx);
    for w in &warnings {
        let _ = writeln!(io.err, "warning: {w}");
    }
    let mut app = App {
        engine,
```

- [ ] **Step 10: Run the tests to verify they pass**

Run:
```
cargo test -p tagteam --test app
cargo test --workspace --features tagteam/test-support
```
Expected: PASS, including `settings_warnings_go_to_stderr_and_never_fail_the_command`. Every
existing CLI test still passes: with no `config.toml`, no command prints anything new.

- [ ] **Step 11: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 12: Commit**

```bash
git add crates/tagteam/src/app.rs crates/tagteam/tests/app.rs
git commit -m "Load settings and build the HTTP client on first use in the CLI"
```

---

### Task 10: Collector: reserve, token, fetch, record (inactive accounts)

§8.3's collector for the on-demand callers (`list`, `status`): every listed account on its own
scoped thread, three phases each. Phase 1 is Task 8's `reserve_usage`: eligibility, the
`usage:<id>` lease and the budget slot, in one IMMEDIATE transaction. Phase 2 gets a token and
sends, holding no lock but those the gate takes for its own refresh; every request is first
authorized by Task 8's fenced `authorize_send`, so a sender that lost its lease sends nothing.
Phase 3 records through Task 8's fenced writes, so a late or superseded result is dropped. This
task builds the whole collector and the inactive account's token path (§8.1): the vault's token,
refreshed through the gate (§7.3) first when it has expired or was refused, and one gate refresh
plus one retry after a 401. The active account is fetched here with its live token, read fresh
and never refreshed; Task 11 hands its expired or refused token to §7.5, and reads it under the
mutation lock.

**Readings of the spec this task commits to:**
- **Every failure backs off (§8.5)**, including a fetch that ended before sending
  (`vault-absent`, `keychain-unavailable`, `no-access-token`, `token-expired`,
  `refresh-failed`, `pre-send`): the next attempt waits `failure_backoff_s(n, …)` like any
  other. A 429's `last_429_at` is its `backoff_until` (Decision 2). A slot whose request never
  left is given back in the same fenced transaction (§8.3).
- **`rejected_fp` applies to both roles.** §8.1 states it for the live token; its rule, "the same
  bytes are not sent again until they change", costs nothing for an inactive account. A 401
  stamps the token that was sent, and a stamped vault token goes to the gate before any
  request, so a gate that fails after a 401 never leads to the refused token being sent again
  next time. `record_usage` clears the stamp on success (Task 8). The stamp is fenced like the
  records (`set_rejected_fp`): a holder that lost its lease stamps nothing and stops, `Dropped`.
- **No token that has expired or was refused is ever sent.** Every request goes through one
  `send`, which refuses such a token as `token-expired` and gives its slot back. A token the
  collector cannot refresh (a kind that does not refresh, or no refresh token) ends there too.
- **Every request is authorized right before it is sent**, first send and 401 retry alike:
  `send` calls Task 8's `authorize_send` with the access-token fingerprint of the exact bytes
  it is about to send, and the slot it holds (`None` for the retry). `send`'s own refusal
  checks the collector's copy of `rejected_fp`, read once after reserving; the store's check
  is the durable one, which catches a collector that paused past its 90 s lease while another
  process took the lease over, was refused this very token (401) and stamped it. On
  `LeaseLost` the collector sends nothing and records nothing (`Dropped`); its unsent slot
  stays counted, as after any failed fence (Task 8). On `Rejected` it records the failure
  `token-expired` for the active account (whose refusal is §7.5's to handle, Task 11) or
  `http-401` for an inactive one, and gives the slot back. On `OverBudget` it records
  `over-budget`.
- **Budget outcomes.** Over budget in phase 1 is `Collected::OverBudget`: the store moved
  `next_poll_at` and took no lease, and nothing else is recorded (§8.6: the existing reading
  keeps whatever trust §8.4 gives it). Over budget later, for the 401 retry or for a slot
  re-reserved after it went stale, happens under a held lease, so it is recorded as the
  failure `over-budget`, backing off at least until a slot frees up.
- **Managed API keys are `Unsupported` before reserving.** They have no usage (`api_key`,
  §13.2), so they take no slot and record nothing. The contract's doc comments for
  `Unsupported` and `Dropped` are widened to say so (the shapes are the contract's).
- **The active account** is the one each provider's live identity names, read once before the
  threads start. An absent or unreadable live login names none: every account then takes the
  inactive path, where the gate itself refuses anything that might be live (§7.3 step 2,
  `Owned(Live)`). The gate is the backstop; the classification only picks the path.
- **A live login that moved** to another account before its token was read (a switch finished
  meanwhile) records nothing, since that token is not this account's: the slot goes back and
  the outcome is `Dropped`. It is the live token's counterpart to the store's identity fence.
- **A slot is given back by its full identity** (Task 8: `rowid`, provider, identity key and
  time, in one statement), never by rowid alone. `usage_requests` has no `AUTOINCREMENT`, so
  once a slot's row has been pruned (older than 3660 s, after a long suspend), SQLite may have
  given its rowid to another process's slot; the full match deletes nothing then, so the
  collector gives a slot back without checking its age.
- **Errors.** A store or test-hook error is `collect_usage`'s `Err`. A refresh that returns an
  error is a recorded `refresh-failed` with a warning, never a command error (§8.3). A panic in
  a collector thread resumes on the caller.
- **Warnings**, word for word. They name the account by label and position, never a token:
  - `<label> (position <N>) needs a new login: a refreshed token was lost while collecting usage`
    — a lost successor (§8.3, the gate's `Unpersisted`; Task 11 adds §7.5's).
  - `could not refresh <label> (position <N>) to read its usage: <detail>` — a refresh that
    returned an error (Task 11 adds §7.5's `Systemic` and `Transient`).

**Design, and what was rejected.** One small struct, `Collection`, carries an account from its
reservation to its record: the reservation, the one slot reserved for the next request while
that request is unsent, the refused fingerprint as stamped so far, and the warnings. Every
request goes through `send`, which checks the token, has the store authorize it and hand over
the slot to send under (the held one, or a fresh one if it went stale, §8.6), and puts the slot
back only if the request never left. A slot still held at the record is exactly the one to give
back, so "nothing is sent without a slot and a held lease" and "an unsent slot is returned" are
each enforced in one place. Rejected: a typestate slot threaded through every step (more types
for an invariant that `send` already holds), a general retry loop (§8.1 allows exactly one
retry, after a 401), renewing the lease before the retry (§6.1: leases bound only harmless
overlap; the fence drops a late result), and a lease check in its own transaction beside the
slot call (two transactions leave a window between the check and the slot, which one IMMEDIATE
`authorize_send` does not).

**Files:**
- Create: `crates/tagteam-engine/src/collect.rs`
- Modify: `crates/tagteam-engine/src/lib.rs` (`pub mod collect;`)
- Modify: `crates/tagteam-engine/tests/common/mod.rs` (usage helpers)
- Create: `crates/tagteam-engine/tests/collect.rs`

**Interfaces:**
- Consumes:
  - Task 2: `tagteam_core::{Window, WindowKind}`,
    `tagteam_core::usage::{max_relevant_pct, earliest_relevant_reset}`
  - Task 3: `tagteam_core::backoff::failure_backoff_s(consecutive_failures: u32, is_429: bool, retry_after_s: Option<f64>) -> i64`
  - Task 4: `tagteam_core::{PollBudget, PollInputs, PollPlan}`,
    `tagteam_core::poll::plan_after_fetch(b: &PollBudget, i: &PollInputs, jitter: f64) -> PollPlan`
  - Task 6 (tests only): `tagteam_cc::usage::normalize(body: &Value) -> Result<Vec<Window>, ()>`
  - Task 7: `tagteam_provider::provider::UsageResult` (`Windows`, `NoAccessToken`,
    `Unauthorized`, `Failed { kind: TransientKind, retry_after_s }`),
    `Provider::fetch_usage(&self, http: &dyn Http, cred: &Credential) -> UsageResult`,
    `Provider::poll_budget(&self) -> PollBudget`, `Capabilities.usage` (true for Claude Code and
    FakeAgent); `FakeAgent::usage_url()`, its `authorization: Fake <token>` request header and
    its `{"meters": [{"id", "used", "renews"}]}` reply (windows `daily` Short, `monthly` Long)
  - Task 8, on `Store`: `reserve_usage(&self, account: &AccountRow, now_ms: i64, on_demand: bool, budget: &PollBudget) -> Result<Reserve, StoreError>`,
    `authorize_send(&self, r: &Reservation, slot: Option<&Slot>, access_fp: Option<&str>, now_ms: i64, budget: &PollBudget) -> Result<SendGrant, StoreError>`,
    `release_slot(&self, r: &Reservation, slot: &Slot) -> Result<(), StoreError>`,
    `record_usage(&self, r: &Reservation, windows: &[Window], now_s: i64, plan: &PollPlan, retention_days: u32) -> Result<bool, StoreError>`,
    `record_usage_failure(&self, r: &Reservation, kind: &str, now_s: i64, backoff_until: i64, last_429_at: Option<i64>, release: Option<&Slot>) -> Result<bool, StoreError>`,
    `set_rejected_fp(&self, r: &Reservation, fp: Option<&str>) -> Result<bool, StoreError>` (fenced
    like the records, `Ok(false)` when the lease is lost; creates a missing row; see the
    Interface Contract), `usage_state(&self, id) -> Result<Option<UsageStateRow>, StoreError>`,
    `usage_samples(&self, id, window: Option<&str>, since_s: i64) -> Result<Vec<(String, Sample)>, StoreError>`;
    `Reserve`, `Reservation { provider, identity_key, holder, slot, slot_at, .. }`,
    `Slot { slot, slot_at }`, `SendGrant::{Send(Slot), Rejected, LeaseLost, OverBudget { next_free_at }}`,
    `Ineligible`, `UsageStateRow`, all re-exported from `crate::store`
  - Task 9: `Engine::settings(&self) -> &Settings` (`threshold`, `models`,
    `history_retention_days`); `EngineConfig.settings`; `tagteam_engine::settings::Settings`
  - M2a: `Engine::refresh_stored(&self, p: &dyn Provider, id: &AccountId, snapshot: &[u8]) -> Result<GateOutcome, EngineError>`,
    `GateOutcome`, `crate::refresh::expired(p, bytes, now_ms) -> bool`, `hooks::point`,
    `Engine::on_point`, `Provider::{access_fingerprint, has_refresh_token, kind_traits, live_identity, read_live_auth, identity_key}`
  - Fixture: `Fx::{add, add_api_key, expire_access, script_refresh, script_token_error, put_vault, live_item, paths, login, engine_with_env, vault_bytes, vault_refresh_token, live_refresh_token, live_credential, set_live_credential, snapshot, assert_only_surface_changed_for, oauth_account}`,
    `FakeFx`, and the free helpers `two_accounts`, `due`, `token_requests`, `quarantine_of`,
    `block_rescue`, `unblock_rescue`, `credential`, `splice_oauth_account`
- Produces:
  - `crates/tagteam-engine/src/collect.rs`, exactly the contract: `CollectMode`, `Collected`,
    `CollectReport`, `Engine::collect_usage(&self, mode: CollectMode) -> Result<CollectReport, EngineError>`
  - `pub(crate) fn jitter() -> f64` in `crate::collect`, uniform in [-1, 1) (Decision 6; Task 12
    uses it)
  - Test-hook points `usage-reserved` (after phase 1), `usage-before-send` (in `send`, after its
    own token check and before `authorize_send`) and `usage-before-record` (before phase 3)
  - `last_error` tokens written: `http-<code>`, `pre-send`, `ambiguous`, `bad-response`,
    `refresh-failed`, `over-budget`, `no-access-token`, `vault-absent`,
    `keychain-unavailable`, `token-expired` (Task 11 adds `foreign-credential`)
  - The two warning lines above, word for word
  - In `tests/common/mod.rs`: `Fx::{script_usage(&self, status: u16, body: Value), script_usage_429(&self, retry_after: &str), collect(&self, ids: &[&AccountId]) -> CollectReport, usage_state(&self, id: &AccountId) -> Option<UsageStateRow>, engine_with_http(&self, http: Arc<dyn Http>) -> Engine}`
    and the free `usage_fixture() -> Value`, `usage_bearers(&Fx) -> Vec<String>`,
    `slot_times(&Fx) -> Vec<i64>`, `usage_requests(&Fx) -> usize`,
    `access_fp(&Fx, secret: &[u8]) -> String` (Tasks 11 and 12 reuse them)

- [ ] **Step 1: Add the shared usage helpers to the test fixture**

Append to the end of `crates/tagteam-engine/tests/common/mod.rs` (paths are written out in full
so the existing `use` lines stay as they are):

```rust
/// Usage collection (§8), shared by `collect.rs`, `collect_active.rs` and `switch.rs`.
impl Fx {
    /// Queues a reply from the usage endpoint (§8.1): `status` with a JSON `body`.
    pub fn script_usage(&self, status: u16, body: Value) {
        self.http
            .push_json(Method::Get, &Self::endpoints().usage, status, body);
    }

    /// Queues a 429 whose `Retry-After` header is `retry_after`, verbatim (§8.1, §8.5).
    pub fn script_usage_429(&self, retry_after: &str) {
        let mut reply = tagteam_provider::HttpResponse::json_body(
            429,
            &json!({"type": "error", "error": {"type": "rate_limit_error", "message": "Rate limited"}}),
        );
        reply
            .headers
            .push(("retry-after".to_owned(), retry_after.to_owned()));
        self.http
            .push(Method::Get, &Self::endpoints().usage, Ok(reply));
    }

    /// `list`'s on-demand collection of `ids` through the fixture's engine (§8.3).
    pub fn collect(&self, ids: &[&AccountId]) -> tagteam_engine::collect::CollectReport {
        self.engine
            .collect_usage(tagteam_engine::collect::CollectMode::OnDemand {
                accounts: ids.iter().map(|id| (*id).clone()).collect(),
            })
            .unwrap()
    }

    /// `id`'s `usage_state` row, if it has one.
    pub fn usage_state(&self, id: &AccountId) -> Option<tagteam_engine::store::UsageStateRow> {
        self.engine.store().unwrap().usage_state(id).unwrap()
    }

    /// An engine over this fixture's Env, Keychain, oracle and clock whose requests go to
    /// `http` rather than to the fixture's scripted port.
    pub fn engine_with_http(&self, http: Arc<dyn tagteam_provider::Http>) -> Engine {
        Engine::new(EngineConfig {
            env: self.env.clone(),
            registry: ProviderRegistry::new().with(self.cc.clone()),
            vault: self.keychain_vault(),
            oracle: self.oracle.clone(),
            clock: self.clock.clone(),
            http,
            default_provider: ProviderId::new(CLAUDE_CODE),
            settings: tagteam_engine::settings::Settings::default(),
        })
    }
}

/// The usage body recorded from the live endpoint (Appendix A.5).
pub fn usage_fixture() -> Value {
    let recorded: Value = serde_json::from_str(include_str!(
        "../../../tagteam-cc/tests/fixtures/endpoints/usage-200.json"
    ))
    .unwrap();
    recorded["body"].clone()
}

/// The access tokens Claude Code's usage requests carried, in the order they were sent.
pub fn usage_bearers(fx: &Fx) -> Vec<String> {
    let usage = Fx::endpoints().usage;
    fx.http
        .requests()
        .iter()
        .filter(|r| r.method == Method::Get && r.url == usage)
        .filter_map(|r| {
            r.headers
                .iter()
                .find(|(name, _)| name == "authorization")
                .and_then(|(_, value)| value.strip_prefix("Bearer "))
                .map(str::to_owned)
        })
        .collect()
}

/// When each slot the hourly budget counts was reserved, in epoch seconds, oldest first (§8.6).
pub fn slot_times(fx: &Fx) -> Vec<i64> {
    let conn = rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db")).unwrap();
    let mut rows = conn
        .prepare("SELECT at FROM usage_requests ORDER BY at")
        .unwrap();
    rows.query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// How many slots the hourly budget counts: every request sent, and any reserved but unsent.
pub fn usage_requests(fx: &Fx) -> usize {
    slot_times(fx).len()
}

/// The access-token fingerprint `rejected_fp` holds for `secret` (§8.1).
pub fn access_fp(fx: &Fx, secret: &[u8]) -> String {
    fx.cc
        .access_fingerprint(secret)
        .unwrap()
        .as_str()
        .to_owned()
}
```

- [ ] **Step 2: Write the failing tests**

Create `crates/tagteam-engine/tests/collect.rs`:

```rust
//! §8.1 and §8.3: the collector's reserve, token, fetch and record phases for inactive
//! accounts (and the live token, read but never refreshed by a fetch), the hourly budget across
//! processes (§8.6, Review Focus 1 and 2), and provider neutrality (§15.2).
mod common;

use std::collections::BTreeSet;
use std::fs;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use common::{
    API_KEY, FakeFx, Fx, access_fp, block_rescue, credential, due, quarantine_of, token_requests,
    two_accounts, unblock_rescue, usage_bearers, usage_fixture, usage_requests,
};
use serde_json::{Value, json};
use tagteam_cc::ItemKind;
use tagteam_cc::usage::normalize;
use tagteam_core::backoff::failure_backoff_s;
use tagteam_core::{AccountId, WindowKind};
use tagteam_engine::Engine;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::collect::{CollectMode, Collected};
use tagteam_engine::store::{Ineligible, UsageStateRow};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::{
    Http, HttpError, HttpRequest, HttpResponse, Keychain, Method, Provider, ScriptedHttp,
};

/// The fixture clock's start, in the seconds the usage tables hold (Decision 1).
const NOW_S: i64 = 1_790_000_000;

fn failed(kind: &str) -> Collected {
    Collected::Failed { kind: kind.into() }
}

fn state(fx: &Fx, id: &AccountId) -> UsageStateRow {
    fx.usage_state(id).expect("the collection wrote a usage_state row")
}

/// The usage endpoint refusing the token.
fn refused() -> Value {
    json!({"type": "error", "error": {"type": "authentication_error", "message": "Invalid bearer token"}})
}

fn methods(fx: &Fx) -> Vec<Method> {
    fx.http.requests().iter().map(|r| r.method).collect()
}

/// Another process's on-demand collection of `id`.
fn collect_on(engine: &Engine, id: &AccountId) -> Vec<(AccountId, Collected)> {
    engine
        .collect_usage(CollectMode::OnDemand {
            accounts: vec![id.clone()],
        })
        .unwrap()
        .outcomes
}

/// Spends `n` of the hourly budget of `id`'s identity, reserved `ago` seconds before now.
fn spend_budget(fx: &Fx, id: &AccountId, n: usize, ago: i64) {
    let key = fx
        .engine
        .store()
        .unwrap()
        .account(id)
        .unwrap()
        .unwrap()
        .identity_key;
    let conn = rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db")).unwrap();
    for _ in 0..n {
        conn.execute(
            "INSERT INTO usage_requests(provider, identity_key, at) VALUES (?1, ?2, ?3)",
            rusqlite::params![fx.provider().as_str(), key, NOW_S - ago],
        )
        .unwrap();
    }
}

#[test]
fn an_inactive_account_is_fetched_with_its_stored_token_and_recorded() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(usage_bearers(&fx), ["at-rt-a"]);
    let requests = fx.http.requests();
    assert!(
        requests[0]
            .headers
            .iter()
            .any(|(k, v)| k == "anthropic-beta" && v == "oauth-2025-04-20"),
        "{:?}",
        requests[0]
    );
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(usage_requests(&fx), 1, "the one request holds the one slot");
    let s = state(&fx, &a);
    assert_eq!(s.last_good, Some(normalize(&usage_fixture()).unwrap()));
    assert_eq!(s.fetched_at, Some(NOW_S));
    assert_eq!((s.consecutive_failures, s.last_error), (0, None));
    let next = s.next_poll_at.unwrap();
    assert!((NOW_S + 180..=NOW_S + 660).contains(&next), "{next}");
    let samples = fx.engine.store().unwrap().usage_samples(&a, None, 0).unwrap();
    let keys: BTreeSet<&str> = samples.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, BTreeSet::from(["5h", "7d", "scoped:Fable"]));
    assert!(samples.iter().all(|(_, s)| s.fetched_at == NOW_S));
}

#[test]
fn an_expired_stored_token_is_refreshed_through_the_gate_before_the_fetch() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert_eq!(methods(&fx), [Method::Post, Method::Get], "the gate first");
    assert_eq!(usage_bearers(&fx), ["at-rt-a2"]);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(
        fx.live_refresh_token().as_deref(),
        Some("rt-b"),
        "the live login is b's and untouched"
    );
    assert_eq!(usage_requests(&fx), 1);
}

#[test]
fn a_dead_refresh_quarantines_the_account_and_gives_its_slot_back() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_token_error(400, "invalid_grant");

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(
        usage_bearers(&fx).is_empty(),
        "a Dead verdict never reaches the usage endpoint (§8.1)"
    );
    assert_eq!(usage_requests(&fx), 0, "a fetch that ended before sending gives its slot back");
    assert_eq!(quarantine_of(&fx, &a).0.as_deref(), Some("invalid_grant"));
    let s = state(&fx, &a);
    assert_eq!(s.last_error.as_deref(), Some("refresh-failed"));
    assert_eq!(s.consecutive_failures, 1);
    assert_eq!(s.backoff_until, Some(NOW_S + failure_backoff_s(1, false, None)));
    assert_eq!((s.last_good, s.fetched_at), (None, None));

    // Past the 90 s lease and the backoff, only the quarantine stands (§7.4).
    fx.clock.advance_ms(91_000);
    let report = fx.collect(&[&a]);
    assert_eq!(
        report.outcomes,
        [(a.clone(), Collected::Ineligible(Ineligible::Quarantined))]
    );
    assert_eq!(token_requests(&fx), 1);
    assert!(usage_bearers(&fx).is_empty());
}

#[test]
fn every_other_gate_refusal_is_a_failure_that_sends_nothing() {
    for case in ["busy", "pre-send", "systemic"] {
        let fx = Fx::new();
        let a = due(&fx);
        let held = (case == "busy")
            .then(|| AccountLock::acquire(&fx.env, &a, Duration::from_secs(1)).unwrap());
        if case == "systemic" {
            fx.script_token_error(400, "invalid_client");
        }
        // "pre-send": nothing is scripted for the token endpoint, so its request never leaves.

        let report = fx.collect(&[&a]);
        drop(held);

        assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))], "{case}");
        assert!(usage_bearers(&fx).is_empty(), "{case}");
        assert_eq!(usage_requests(&fx), 0, "{case}");
        assert_eq!(quarantine_of(&fx, &a).0, None, "never a strike: {case}");
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"), "{case}");
    }
}

#[test]
fn a_lost_successor_is_a_warning_that_names_the_account_and_no_token() {
    // §8.3: the gate spent rt-a and could store rt-a2 nowhere (§7.3 step 6, `Unpersisted`).
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    fx.kc.set_fail_write(SERVICE, true);
    block_rescue(&fx);

    let report = fx.collect(&[&a]);
    unblock_rescue(&fx);
    fx.kc.set_fail_write(SERVICE, false);

    assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))]);
    assert_eq!(
        report.warnings,
        ["a@x.co (position 1) needs a new login: a refreshed token was lost while collecting usage"]
    );
    assert!(!report.warnings[0].contains("rt-a") && !report.warnings[0].contains("at-rt"));
    assert_eq!(quarantine_of(&fx, &a).0.as_deref(), Some("successor_lost"));
    assert!(usage_bearers(&fx).is_empty());
    assert_eq!(usage_requests(&fx), 0);
}

#[test]
fn a_missing_token_or_an_unreadable_vault_is_recorded_without_sending() {
    let cases: [(&str, fn(&Fx, &AccountId)); 3] = [
        ("no-access-token", |fx, a| {
            fx.put_vault(
                a,
                json!({"claudeAiOauth": {"refreshToken": "rt-a"}})
                    .to_string()
                    .as_bytes(),
            )
        }),
        ("vault-absent", |fx, a| fx.kc.delete(SERVICE, a.as_str()).unwrap()),
        ("keychain-unavailable", |fx, a| {
            fx.kc.set_unreadable(SERVICE, a.as_str(), true)
        }),
    ];
    for (kind, break_it) in cases {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        break_it(&fx, &a);
        fx.script_usage(200, usage_fixture());

        let report = fx.collect(&[&a]);

        assert_eq!(report.outcomes, [(a.clone(), failed(kind))], "{kind}");
        assert!(fx.http.requests().is_empty(), "nothing was sent: {kind}");
        assert_eq!(usage_requests(&fx), 0, "{kind}");
        assert_eq!(state(&fx, &a).last_error.as_deref(), Some(kind));
    }
}

#[test]
fn a_managed_api_key_has_no_usage_and_reserves_nothing() {
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b");
    let k = fx.add_api_key(API_KEY);

    let report = fx.collect(&[&k]);

    assert_eq!(report.outcomes, [(k.clone(), Collected::Unsupported)]);
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 0);
    assert_eq!(fx.usage_state(&k), None);
}

#[test]
fn a_401_refreshes_once_through_the_gate_and_retries_under_its_own_slot() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.script_usage(401, refused());
    fx.script_usage(200, usage_fixture());
    fx.script_refresh(Some("rt-a2"));

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert_eq!(methods(&fx), [Method::Get, Method::Post, Method::Get]);
    assert_eq!(usage_bearers(&fx), ["at-rt-a", "at-rt-a2"]);
    assert_eq!(usage_requests(&fx), 2, "the retry is a request like any other (§8.6)");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(state(&fx, &a).rejected_fp, None, "success clears it");
}

#[test]
fn a_second_401_is_recorded_and_the_refused_token_is_never_sent_again() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.script_usage(401, refused()); // the only reply: every usage request is refused
    fx.script_refresh(Some("rt-a2"));

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("http-401"))]);
    assert_eq!(usage_bearers(&fx), ["at-rt-a", "at-rt-a2"], "one retry, no more");
    assert_eq!(usage_requests(&fx), 2);
    let refused_fp = access_fp(&fx, &fx.vault_bytes(&a).unwrap());
    assert_eq!(state(&fx, &a).rejected_fp.as_deref(), Some(refused_fp.as_str()));

    // Next time, the refused token goes to the gate before any usage request.
    fx.http.clear();
    fx.clock.advance_ms(91_000);
    fx.script_refresh(Some("rt-a3"));
    fx.script_usage(200, usage_fixture());
    let report = fx.collect(&[&a]);
    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert_eq!(methods(&fx), [Method::Post, Method::Get]);
    assert_eq!(usage_bearers(&fx), ["at-rt-a3"], "at-rt-a2 was never sent again");
    assert_eq!(state(&fx, &a).rejected_fp, None);
}

#[test]
fn a_429_backs_off_as_retry_after_asks_and_records_when_the_block_lifts() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.script_usage_429("120");

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("http-429"))]);
    let lift = NOW_S + failure_backoff_s(1, true, Some(120.0));
    let s = state(&fx, &a);
    assert_eq!(s.backoff_until, Some(lift));
    assert_eq!(s.last_429_at, Some(lift), "Decision 2: when the backoff lifts");
    assert_eq!(usage_requests(&fx), 1, "the request was sent, so its slot stays counted");

    // Past the lease, not past the backoff.
    fx.clock.advance_ms(91_000);
    let report = fx.collect(&[&a]);
    assert_eq!(
        report.outcomes,
        [(a.clone(), Collected::Ineligible(Ineligible::Backoff))]
    );
    assert_eq!(usage_bearers(&fx).len(), 1);
}

#[test]
fn failures_never_touch_the_last_good_reading() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.script_usage(200, usage_fixture());
    assert_eq!(fx.collect(&[&a]).outcomes, [(a.clone(), Collected::Recorded)]);
    let good = state(&fx, &a);

    // Past the reading's plan (at most 660 s) and its lease.
    fx.http.clear();
    fx.clock.advance_ms(700_000);
    fx.script_usage(503, json!({"type": "error", "error": {"type": "overloaded_error"}}));
    assert_eq!(fx.collect(&[&a]).outcomes, [(a.clone(), failed("http-503"))]);

    fx.http.clear();
    fx.clock.advance_ms(91_000);
    fx.http.push(
        Method::Get,
        &Fx::endpoints().usage,
        Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: b"<html>busy</html>".to_vec(),
        }),
    );
    assert_eq!(fx.collect(&[&a]).outcomes, [(a.clone(), failed("bad-response"))]);

    let s = state(&fx, &a);
    assert_eq!(
        (s.last_good, s.fetched_at),
        (good.last_good, good.fetched_at),
        "§8.3: failure never touches the last good reading"
    );
    assert_eq!(s.consecutive_failures, 2);
    assert_eq!(s.last_error.as_deref(), Some("bad-response"));
}

#[test]
fn a_request_that_never_left_gives_its_slot_back() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.http.push(
        Method::Get,
        &Fx::endpoints().usage,
        Err(HttpError::PreSend("dns lookup failed".into())),
    );

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("pre-send"))]);
    assert_eq!(usage_requests(&fx), 0, "the request never left, so its slot is given back");
}

#[test]
fn an_identity_over_its_hourly_budget_is_sent_nothing() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    spend_budget(&fx, &a, 20, 100);
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(
        report.outcomes,
        [(a.clone(), Collected::OverBudget { next_free_at: NOW_S - 100 + 3660 })]
    );
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 20);
}

#[test]
fn the_retry_after_a_401_needs_a_slot_of_its_own() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    spend_budget(&fx, &a, 19, 100);
    fx.script_usage(401, refused());
    fx.script_refresh(Some("rt-a2"));

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("over-budget"))]);
    assert_eq!(usage_bearers(&fx), ["at-rt-a"], "the retry is never sent without a slot");
    assert_eq!(token_requests(&fx), 1);
    assert_eq!(usage_requests(&fx), 20);
    assert!(
        state(&fx, &a).backoff_until.unwrap() >= NOW_S - 100 + 3660,
        "no retry before a slot frees up"
    );
}

#[test]
fn the_live_account_is_fetched_with_the_live_token() {
    // CC refreshed its access token on its own; the vault still holds the older one.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let mut live = fx.live_credential().unwrap();
    live["claudeAiOauth"]["accessToken"] = json!("at-live");
    fx.set_live_credential(live.to_string().as_bytes());
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert_eq!(usage_bearers(&fx), ["at-live"]);
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_degraded_live_read_is_a_keychain_failure_that_sends_nothing() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let (svc, acct) = fx.live_item(ItemKind::OAuth);
    fx.kc.set_unreadable(&svc, &acct, true);
    fs::write(fx.paths().credentials_file, credential("a@x.co", "rt-a")).unwrap();
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("keychain-unavailable"))]);
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 0);
}

/// Holds every request until `want` are in flight at once (or 5 s pass), then answers from
/// `inner`: the collector's requests overlap only if it collects the accounts in parallel.
struct Rendezvous {
    inner: Arc<ScriptedHttp>,
    want: usize,
    /// In flight now, and the most ever in flight at once.
    counts: Mutex<(usize, usize)>,
    arrived: Condvar,
}

impl Http for Rendezvous {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut counts = self.counts.lock().unwrap();
        counts.0 += 1;
        counts.1 = counts.1.max(counts.0);
        self.arrived.notify_all();
        while counts.1 < self.want {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            counts = self.arrived.wait_timeout(counts, left).unwrap().0;
        }
        counts.0 -= 1;
        drop(counts);
        self.inner.send(req)
    }
}

#[test]
fn every_account_is_collected_on_its_own_thread() {
    // §8.3: one thread each, and the command waits for all of them.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c"); // live
    fx.script_usage(200, usage_fixture());
    let probe = Arc::new(Rendezvous {
        inner: fx.http.clone(),
        want: 3,
        counts: Mutex::new((0, 0)),
        arrived: Condvar::new(),
    });
    let engine = fx.engine_with_http(probe.clone());

    let report = engine
        .collect_usage(CollectMode::OnDemand {
            accounts: vec![a.clone(), b.clone(), c.clone()],
        })
        .unwrap();

    assert_eq!(
        report.outcomes,
        [
            (a, Collected::Recorded),
            (b, Collected::Recorded),
            (c, Collected::Recorded)
        ]
    );
    assert_eq!(
        probe.counts.lock().unwrap().1,
        3,
        "all three requests were in flight at once"
    );
    let bearers: BTreeSet<String> = usage_bearers(&fx).into_iter().collect();
    assert_eq!(
        bearers,
        BTreeSet::from(["at-rt-a".to_owned(), "at-rt-b".into(), "at-rt-c".into()])
    );
    assert_eq!(usage_requests(&fx), 3);
}

#[test]
fn two_processes_collecting_at_once_send_one_request() {
    // Review Focus 1: two `tagteam list` at once. Whichever reserves second finds the lease
    // taken or the reading already fresh; the budget counts the one request once.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let other = fx.engine_with_env(fx.env.clone());
    fx.script_usage(200, usage_fixture());

    let outcomes: Vec<Collected> = thread::scope(|s| {
        let mine = s.spawn(|| fx.collect(&[&a]).outcomes);
        let theirs = s.spawn(|| collect_on(&other, &a));
        [mine.join().unwrap(), theirs.join().unwrap()]
            .into_iter()
            .map(|o| o[0].1.clone())
            .collect()
    });

    assert_eq!(
        outcomes.iter().filter(|o| **o == Collected::Recorded).count(),
        1,
        "{outcomes:?}"
    );
    assert!(
        outcomes.iter().all(|o| matches!(
            o,
            Collected::Recorded | Collected::Ineligible(Ineligible::Leased | Ineligible::NotDue)
        )),
        "{outcomes:?}"
    );
    assert_eq!(usage_bearers(&fx).len(), 1);
    assert_eq!(usage_requests(&fx), 1);
}

/// `id`'s stored FakeAgent access token, moved inside §7.2's expiry buffer.
fn make_due(ffx: &FakeFx, id: &AccountId) {
    let mut v: Value = serde_json::from_slice(&ffx.fx.vault_bytes(id).unwrap()).unwrap();
    v["fa"]["expires"] = json!(NOW_S * 1000 + 60_000);
    ffx.fx.put_vault(id, v.to_string().as_bytes());
}

#[test]
fn fake_agent_usage_is_collected_through_the_same_engine_and_claude_code_is_untouched() {
    // §15.2: FakeAgent's usage goes through the same collector, gate and store, with its own
    // endpoint and shapes. The parked M2a check: Claude Code's stored login, row and usage
    // stay exactly as they were.
    let ffx = FakeFx::new();
    let cc = ffx.fx.add("cc@b.co", "rt-cc");
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let bob = ffx.fake_add("bob", "tok-b", "renew-b"); // live
    make_due(&ffx, &alice);
    ffx.fx.http.push_json(
        Method::Post,
        &ffx.fake.renew_url(),
        200,
        json!({"token": "tok-a-2", "renew": "renew-a-2", "expires_in": 3600}),
    );
    ffx.fx.http.push_json(
        Method::Get,
        &ffx.fake.usage_url(),
        200,
        json!({"meters": [
            {"id": "daily", "used": 0.42, "renews": NOW_S + 3600},
            {"id": "monthly", "used": 0.1, "renews": NOW_S + 86_400}
        ]}),
    );
    let store = ffx.engine.store().unwrap();
    let (cc_row, cc_vault) = (store.account(&cc).unwrap(), ffx.fx.vault_bytes(&cc));
    let before = ffx.fx.snapshot();

    let report = ffx
        .engine
        .collect_usage(CollectMode::OnDemand {
            accounts: vec![alice.clone(), bob.clone()],
        })
        .unwrap();
    let after = ffx.fx.snapshot();

    assert_eq!(
        report.outcomes,
        [
            (alice.clone(), Collected::Recorded),
            (bob.clone(), Collected::Recorded)
        ]
    );
    assert_eq!(ffx.fx.http.count(Method::Post, &ffx.fake.renew_url()), 1);
    let sent: BTreeSet<String> = ffx
        .fx
        .http
        .requests()
        .iter()
        .filter(|r| r.url == ffx.fake.usage_url())
        .filter_map(|r| {
            r.headers
                .iter()
                .find(|(k, _)| k == "authorization")
                .and_then(|(_, v)| v.strip_prefix("Fake "))
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(
        sent,
        BTreeSet::from(["tok-a-2".to_owned(), "tok-b".to_owned()])
    );
    let windows = store.usage_state(&alice).unwrap().unwrap().last_good.unwrap();
    let shape: Vec<(&str, WindowKind)> = windows.iter().map(|w| (w.key.as_str(), w.kind)).collect();
    assert_eq!(shape, [("daily", WindowKind::Short), ("monthly", WindowKind::Long)]);
    assert!((windows[0].pct - 42.0).abs() < 1e-9, "{}", windows[0].pct);

    // Claude Code: nothing sent, nothing stored, nothing changed.
    assert_eq!(ffx.fx.http.count(Method::Get, &Fx::endpoints().usage), 0);
    assert_eq!(ffx.fx.http.count(Method::Post, &Fx::endpoints().token), 0);
    assert_eq!(store.account(&cc).unwrap(), cc_row);
    assert_eq!(ffx.fx.vault_bytes(&cc), cc_vault);
    assert_eq!(store.usage_state(&cc).unwrap(), None);
    ffx.fx.assert_only_surface_changed_for(
        &ffx.fake.identity_surface(&ffx.fx.env),
        &before,
        &after,
        "collecting FakeAgent usage",
    );
}

#[cfg(feature = "test-hooks")]
mod hooks {
    use super::*;

    #[test]
    fn a_second_process_finds_the_lease_taken_while_the_first_fetches() {
        // Review Focus 1, deterministically: the other process reserves while this one holds
        // the lease.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.script_usage(200, usage_fixture());
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let seen = Arc::new(Mutex::new(None));
        let (engine, id, record) = (other.clone(), a.clone(), seen.clone());
        fx.engine.on_point(
            "usage-reserved",
            Box::new(move || *record.lock().unwrap() = Some(collect_on(&engine, &id))),
        );

        let report = fx.collect(&[&a]);

        assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
        assert_eq!(
            *seen.lock().unwrap(),
            Some(vec![(a.clone(), Collected::Ineligible(Ineligible::Leased))])
        );
        assert_eq!(usage_bearers(&fx).len(), 1);
        assert_eq!(usage_requests(&fx), 1);
    }

    #[test]
    fn a_slot_not_sent_within_its_validity_is_replaced_before_sending() {
        // Review Focus 2: suspended for 61 s between reserving and sending (§8.6).
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.script_usage(200, usage_fixture());
        let clock = fx.clock.clone();
        fx.engine
            .on_point("usage-reserved", Box::new(move || clock.advance_ms(61_000)));

        let report = fx.collect(&[&a]);

        assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
        assert_eq!(
            common::slot_times(&fx),
            [NOW_S + 61],
            "the stale slot went back, and the request went under a fresh one"
        );
        assert_eq!(usage_bearers(&fx).len(), 1);
    }

    #[test]
    fn a_result_recorded_after_another_process_took_the_lease_is_dropped() {
        // Review Focus 2: suspended for 91 s between fetching and recording, past the lease.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.script_usage(200, usage_fixture());
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let seen = Arc::new(Mutex::new(None));
        let (clock, engine, id, record) =
            (fx.clock.clone(), other.clone(), a.clone(), seen.clone());
        fx.engine.on_point(
            "usage-before-record",
            Box::new(move || {
                clock.advance_ms(91_000);
                *record.lock().unwrap() = Some(collect_on(&engine, &id));
            }),
        );

        let report = fx.collect(&[&a]);

        assert_eq!(report.outcomes, [(a.clone(), Collected::Dropped)]);
        assert_eq!(*seen.lock().unwrap(), Some(vec![(a.clone(), Collected::Recorded)]));
        assert_eq!(
            state(&fx, &a).fetched_at,
            Some(NOW_S + 91),
            "the other process's reading stands"
        );
        let samples = fx.engine.store().unwrap().usage_samples(&a, None, 0).unwrap();
        assert!(
            samples.iter().all(|(_, s)| s.fetched_at == NOW_S + 91),
            "the late result wrote no samples"
        );
        assert_eq!(usage_bearers(&fx).len(), 2);
        assert_eq!(usage_requests(&fx), 2);
    }

    #[test]
    fn a_sender_that_lost_its_lease_never_resends_a_token_refused_meanwhile() {
        // §8.3, §8.6: suspended for 91 s just before sending, past the lease. Another process
        // takes the lease over, is refused the same token (401) and stamps rejected_fp. This
        // process's copy of rejected_fp predates the stamp; the store's authorization, fenced
        // by the lease it lost, sends nothing.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.script_usage(401, refused()); // the only usage reply; nothing for the token endpoint
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let seen = Arc::new(Mutex::new(None));
        let (clock, engine, id, record) =
            (fx.clock.clone(), other.clone(), a.clone(), seen.clone());
        fx.engine.on_point(
            "usage-before-send",
            Box::new(move || {
                clock.advance_ms(91_000);
                *record.lock().unwrap() = Some(collect_on(&engine, &id));
            }),
        );

        let report = fx.collect(&[&a]);

        assert_eq!(report.outcomes, [(a.clone(), Collected::Dropped)]);
        assert_eq!(
            *seen.lock().unwrap(),
            Some(vec![(a.clone(), failed("refresh-failed"))]),
            "the other process sent, was refused, and its gate refresh never left"
        );
        assert_eq!(
            usage_bearers(&fx),
            ["at-rt-a"],
            "the refused token went out once, from the other process"
        );
        let refused_fp = access_fp(&fx, &fx.vault_bytes(&a).unwrap());
        assert_eq!(state(&fx, &a).rejected_fp.as_deref(), Some(refused_fp.as_str()));
        assert_eq!(
            usage_requests(&fx),
            2,
            "the other's sent slot, and this one's unsent slot, left counted by the failed fence"
        );
    }

    #[test]
    fn a_live_login_that_moves_mid_collection_records_nothing() {
        // By the time b's live token is read, the live login names a: that token is not b's.
        let fx = Fx::new();
        fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        fx.script_usage(200, usage_fixture());
        let config = fx.paths().global_config;
        fx.engine.on_point(
            "usage-reserved",
            Box::new(move || {
                common::splice_oauth_account(&config, &Fx::oauth_account("a@x.co"))
            }),
        );

        let report = fx.collect(&[&b]);

        assert_eq!(report.outcomes, [(b.clone(), Collected::Dropped)]);
        assert!(fx.http.requests().is_empty());
        assert_eq!(usage_requests(&fx), 0, "the slot went back");
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test collect`
Expected: FAIL to compile: `error[E0432]: unresolved import tagteam_engine::collect` (in
`tests/common/mod.rs` and in `tests/collect.rs`), and `no method named collect_usage found for
struct Engine`.

- [ ] **Step 4: Write the collector**

In `crates/tagteam-engine/src/lib.rs`, insert `pub mod collect;` directly after `pub mod active;`.

Create `crates/tagteam-engine/src/collect.rs`:

```rust
//! The usage collector (§8.3): reserve, fetch and record, one thread per account. An inactive
//! account's token comes from the vault, through the refresh gate (§7.3) when it needs one; the
//! active account's comes from the live store and is never refreshed by a fetch (§8.1). Nothing
//! is sent without the store's authorization right before the request: the lease still held,
//! the token not refused, and a slot in the identity's hourly budget (§8.3, §8.6).

use std::collections::HashSet;
use std::fmt;
use std::thread;

use tagteam_core::backoff::failure_backoff_s;
use tagteam_core::poll::plan_after_fetch;
use tagteam_core::usage::{earliest_relevant_reset, max_relevant_pct};
use tagteam_core::{AccountId, PollBudget, PollInputs, PollPlan, ProviderId, Window};
use tagteam_provider::provider::UsageResult;
use tagteam_provider::{Credential, Provenance, Provider, Read, TransientKind};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::refresh::{GateOutcome, expired};
use crate::store::{
    AccountRow, Ineligible, Reservation, Reserve, SendGrant, Slot, Store, StoreError,
    UsageStateRow,
};

/// Who asked for a collection, and so which accounts are collected. M3 adds `Scheduled`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectMode {
    /// `list` and `status` (§8.3): the listed accounts, each only if its reading is older than
    /// the 180 s floor and a poll is due or none is planned.
    OnDemand { accounts: Vec<AccountId> },
}

/// What one account's collection did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Collected {
    /// A reading was recorded (§8.3 phase 3).
    Recorded,
    /// Not eligible now (§8.3 phase 1): nothing was reserved or sent.
    Ineligible(Ineligible),
    /// The identity's hourly budget is spent: nothing was sent, and the store moved the next
    /// poll to `next_free_at` (§8.6).
    OverBudget { next_free_at: i64 },
    /// Recorded as a failure; `kind` is its `last_error` token (§8.3, Decision 10).
    Failed { kind: String },
    /// Nothing was recorded: another process took the lease before this fetch sent or
    /// recorded (the fence failed, §8.3), or the live login moved to another account while
    /// this one's token was being read.
    Dropped,
    /// Nothing to collect: the provider lacks the `usage` capability, or the account is a
    /// managed API key, which has no usage (§13.2 `api_key`).
    Unsupported,
}

#[derive(Debug, Clone, Default)]
pub struct CollectReport {
    /// One entry per listed account that exists, in the order listed.
    pub outcomes: Vec<(AccountId, Collected)>,
    /// Lines for stderr, each naming an account and never a token: a successor lost while
    /// collecting (§8.3), or a refresh that failed with an error or, for the live token (§8.1),
    /// with any outcome but Dead.
    pub warnings: Vec<String>,
}

/// One account's collection: what it did, and its warnings.
type Outcome = (Collected, Vec<String>);

/// A jitter draw for the poll policy, uniform in [-1, 1) (Decision 6).
pub(crate) fn jitter() -> f64 {
    fastrand::f64() * 2.0 - 1.0
}

impl Engine {
    /// §8.3 on demand: every listed account on its own thread, and the call waits for them
    /// all. A usage failure is never an error here: it is recorded, and reported in the
    /// report's outcomes and warnings. `Err` means the store itself failed. IDs that name no
    /// account are skipped. Never creates the store.
    pub fn collect_usage(&self, mode: CollectMode) -> Result<CollectReport, EngineError> {
        let CollectMode::OnDemand { accounts } = mode;
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
        let live = self.live_accounts(&rows);
        let results: Vec<Result<Outcome, EngineError>> = thread::scope(|s| {
            let running: Vec<_> = rows
                .iter()
                .map(|row| {
                    let active = live.contains(&row.id);
                    s.spawn(move || self.collect_one(store, row, active))
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
        let mut report = CollectReport::default();
        for (row, result) in rows.iter().zip(results) {
            let (collected, warnings) = result?;
            report.outcomes.push((row.id.clone(), collected));
            report.warnings.extend(warnings);
        }
        Ok(report)
    }

    /// The accounts the providers' live logins name (§8.1's active accounts), read once,
    /// before the threads start. A live login that is absent or unreadable names none: its
    /// provider's accounts all take the inactive path, where the gate refuses any account that
    /// might be live (§7.3 step 2).
    fn live_accounts(&self, rows: &[AccountRow]) -> HashSet<AccountId> {
        let mut live = HashSet::new();
        let mut seen: Vec<&ProviderId> = Vec::new();
        for row in rows {
            if seen.contains(&&row.provider) {
                continue;
            }
            seen.push(&row.provider);
            let Some(p) = self.registry.get(&row.provider) else {
                continue;
            };
            let Read::Present(identity) = p.live_identity(&self.env) else {
                continue;
            };
            let key = p.identity_key(&identity);
            live.extend(
                rows.iter()
                    .filter(|r| r.provider == row.provider && r.identity_key == key.as_str())
                    .map(|r| r.id.clone()),
            );
        }
        live
    }

    /// One account through §8.3's three phases.
    fn collect_one(
        &self,
        store: &Store,
        row: &AccountRow,
        active: bool,
    ) -> Result<Outcome, EngineError> {
        let Some(provider) = self.registry.get(&row.provider) else {
            return Ok((Collected::Unsupported, Vec::new()));
        };
        let p = provider.as_ref();
        if !p.capabilities().usage || p.kind_traits(&row.kind).managed_key_axis {
            return Ok((Collected::Unsupported, Vec::new()));
        }
        let budget = p.poll_budget();
        // Phase 1: eligibility, the lease and the slot, in one transaction.
        let reservation = match store.reserve_usage(row, self.now_ms(), true, &budget)? {
            Reserve::Reserved(r) => r,
            Reserve::Ineligible(why) => return Ok((Collected::Ineligible(why), Vec::new())),
            Reserve::OverBudget { next_free_at } => {
                return Ok((Collected::OverBudget { next_free_at }, Vec::new()));
            }
        };
        hooks::point(self, "usage-reserved")?;
        let state = store.usage_state(&row.id)?;
        let mut run = Collection {
            engine: self,
            store,
            p,
            row,
            active,
            budget,
            slot: Some(Slot {
                slot: reservation.slot,
                slot_at: reservation.slot_at,
            }),
            rejected: state.as_ref().and_then(|s| s.rejected_fp.clone()),
            state,
            reservation,
            warnings: Vec::new(),
        };
        // Phase 2, holding no lock but those the gate or §7.5 take for their own refresh.
        let fetched = if active { run.active() } else { run.inactive() };
        // Phase 3.
        run.record(fetched)
    }
}

/// One account's fetch, from its reservation to its record.
struct Collection<'a> {
    engine: &'a Engine,
    store: &'a Store,
    p: &'a dyn Provider,
    row: &'a AccountRow,
    /// Whether the live login names this account (§8.1's active account).
    active: bool,
    budget: PollBudget,
    reservation: Reservation,
    /// The slot reserved for the next request, while that request is unsent. `send` hands it
    /// to `authorize_send` and puts it back only if the request never left; one still here at
    /// the record is given back (§8.3).
    slot: Option<Slot>,
    /// The state read after reserving: the previous reading, the failure count, the plan.
    state: Option<UsageStateRow>,
    /// The access-token fingerprint the server refused (`rejected_fp`, §8.1), as read after
    /// reserving and stamped since. The store's copy is the one `authorize_send` checks.
    rejected: Option<String>,
    warnings: Vec<String>,
}

/// A failure for phase 3 to record, with what its backoff needs (§8.5).
struct Failure {
    kind: String,
    is_429: bool,
    retry_after_s: Option<f64>,
    /// No retry before this: when an over-budget identity's oldest counted request leaves the
    /// hour (§8.6).
    not_before: Option<i64>,
}

impl Failure {
    fn new(kind: &str) -> Self {
        Failure {
            kind: kind.to_owned(),
            is_429: false,
            retry_after_s: None,
            not_before: None,
        }
    }
}

/// Why a fetch ended without windows.
enum Stop {
    /// Recorded as a failure.
    Failed(Failure),
    /// The live login moved to another account: nothing is recorded, and the unsent slot goes
    /// back.
    Moved,
    /// The lease was taken over, or the account's identity changed (§8.3's fence failed at
    /// `authorize_send` or at the `rejected_fp` stamp): nothing is sent or recorded, and the
    /// unsent slot stays counted, as a failed fence writes nothing.
    LeaseLost,
    /// The store or a test hook failed: returned, never recorded.
    Error(EngineError),
}

impl From<EngineError> for Stop {
    fn from(e: EngineError) -> Self {
        Stop::Error(e)
    }
}

impl From<StoreError> for Stop {
    fn from(e: StoreError) -> Self {
        Stop::Error(e.into())
    }
}

fn failed(kind: &str) -> Stop {
    Stop::Failed(Failure::new(kind))
}

/// The fetch's windows, or the failure its result records (§8.3, §6.1's tokens).
fn windows(result: UsageResult) -> Result<Vec<Window>, Stop> {
    match result {
        UsageResult::Windows(windows) => Ok(windows),
        UsageResult::NoAccessToken => Err(failed("no-access-token")),
        UsageResult::Unauthorized => Err(failed("http-401")),
        UsageResult::Failed {
            kind,
            retry_after_s,
        } => Err(Stop::Failed(Failure {
            is_429: kind == TransientKind::Http(429),
            kind: kind.token(),
            retry_after_s,
            not_before: None,
        })),
    }
}

impl Collection<'_> {
    fn now_ms(&self) -> i64 {
        self.engine.now_ms()
    }

    fn now_s(&self) -> i64 {
        self.now_ms().div_euclid(1000)
    }

    /// §8.1 for an inactive account: the vault's token, refreshed through the gate (§7.3)
    /// first when it has expired or the server refused it. After a 401: one refresh through
    /// the gate and one retry, under a slot of its own.
    fn inactive(&mut self) -> Result<Vec<Window>, Stop> {
        let sent = self.stored_token()?;
        let first = self.send(&sent)?;
        if !matches!(first, UsageResult::Unauthorized) {
            return windows(first);
        }
        self.reject(&sent)?;
        if !self.refreshable(&sent) {
            return Err(failed("http-401"));
        }
        let next = self.gate(&sent)?;
        self.retry(&next)
    }

    /// §8.1 for the active account: the live token, read fresh, and never refreshed by a usage
    /// fetch. One that has expired or was refused is not sent (`send`); a 401 stamps it.
    fn active(&mut self) -> Result<Vec<Window>, Stop> {
        let live = self.live_bytes()?;
        let first = self.send(&live)?;
        if matches!(first, UsageResult::Unauthorized) {
            self.reject(&live)?;
        }
        windows(first)
    }

    /// The vault's token for an inactive account, refreshed through the gate first when it
    /// has expired or the server refused it. One that cannot be refreshed ends the fetch.
    fn stored_token(&mut self) -> Result<Vec<u8>, Stop> {
        let bytes = match self.engine.vault.read(&self.row.id) {
            Read::Present(b) if !b.is_empty() => b,
            Read::Present(_) | Read::Absent => return Err(failed("vault-absent")),
            Read::Unreadable(_) => return Err(failed("keychain-unavailable")),
        };
        if self.usable(&bytes) {
            return Ok(bytes);
        }
        if !self.refreshable(&bytes) {
            return Err(failed("token-expired"));
        }
        self.gate(&bytes)
    }

    fn refreshable(&self, bytes: &[u8]) -> bool {
        self.p.kind_traits(&self.row.kind).refreshable && self.p.has_refresh_token(bytes)
    }

    /// §7.3 for an inactive account, `snapshot` being the bytes the collector decided on. A
    /// Dead verdict (the gate has quarantined the account) and every deterministic refusal end
    /// the fetch before anything is sent (§8.1).
    fn gate(&mut self, snapshot: &[u8]) -> Result<Vec<u8>, Stop> {
        match self.engine.refresh_stored(self.p, &self.row.id, snapshot) {
            Ok(GateOutcome::Refreshed(bytes) | GateOutcome::AlreadyFresh(bytes)) => Ok(bytes),
            Ok(GateOutcome::Unpersisted) => {
                self.warn_lost();
                Err(failed("refresh-failed"))
            }
            Ok(
                GateOutcome::Dead(_)
                | GateOutcome::Busy
                | GateOutcome::Owned(_)
                | GateOutcome::Conflict
                | GateOutcome::Systemic(_)
                | GateOutcome::Transient { .. },
            ) => Err(failed("refresh-failed")),
            Err(e) => {
                self.warn_refresh(&e);
                Err(failed("refresh-failed"))
            }
        }
    }

    /// The live credential, read fresh, while the live login still names this account. A live
    /// login that moved stops the fetch: its token is not this account's.
    fn live_bytes(&self) -> Result<Vec<u8>, Stop> {
        let env = &self.engine.env;
        match self.p.live_identity(env) {
            Read::Present(i) if self.p.identity_key(&i).as_str() == self.row.identity_key => {}
            _ => return Err(Stop::Moved),
        }
        match self.p.read_live_auth(env).credential {
            Read::Present(c) if c.provenance() == Provenance::Degraded => {
                Err(failed("keychain-unavailable"))
            }
            Read::Present(c) if !c.is_empty() => Ok(c.bytes().to_vec()),
            Read::Present(_) | Read::Absent => Err(failed("no-access-token")),
            Read::Unreadable(_) => Err(failed("keychain-unavailable")),
        }
    }

    fn access_fp(&self, bytes: &[u8]) -> Option<String> {
        self.p
            .access_fingerprint(bytes)
            .map(|f| f.as_str().to_owned())
    }

    fn is_rejected(&self, bytes: &[u8]) -> bool {
        self.rejected.is_some() && self.access_fp(bytes) == self.rejected
    }

    /// Neither expired (§7.2) nor refused by the server (§8.1): the only kind of token sent.
    fn usable(&self, bytes: &[u8]) -> bool {
        !self.is_rejected(bytes) && !expired(self.p, bytes, self.now_ms())
    }

    /// Stamps `rejected_fp` with the token the server just refused, so it is never sent again
    /// until it changes (§8.1). The stamp is fenced by the lease (Task 8): a holder that lost
    /// it stops here, recording nothing.
    fn reject(&mut self, bytes: &[u8]) -> Result<(), Stop> {
        if let Some(fp) = self.access_fp(bytes) {
            if !self
                .store
                .set_rejected_fp(&self.reservation, Some(fp.as_str()))?
            {
                return Err(Stop::LeaseLost);
            }
            self.rejected = Some(fp);
        }
        Ok(())
    }

    /// The one request after a 401, under a slot of its own (§8.1, §8.6): the first slot went
    /// with the refused request, so `send` has the store reserve a fresh one.
    fn retry(&mut self, bytes: &[u8]) -> Result<Vec<Window>, Stop> {
        let second = self.send(bytes)?;
        if matches!(second, UsageResult::Unauthorized) {
            self.reject(bytes)?;
        }
        windows(second)
    }

    /// One usage request with `bytes`' access token. A token that has expired or was refused
    /// is never sent (§8.1): the slot stays unsent. Right before the request, the store
    /// authorizes it and hands over the slot to send under (`authorize`). A request that never
    /// left (no access token, or a pre-send failure) puts its slot back.
    fn send(&mut self, bytes: &[u8]) -> Result<UsageResult, Stop> {
        if !self.usable(bytes) {
            return Err(failed("token-expired"));
        }
        hooks::point(self.engine, "usage-before-send")?;
        let slot = self.authorize(bytes)?;
        let result = self
            .p
            .fetch_usage(self.engine.http(), &Credential::fresh(bytes.to_vec()));
        let unsent = matches!(
            result,
            UsageResult::NoAccessToken
                | UsageResult::Failed {
                    kind: TransientKind::PreSend,
                    ..
                }
        );
        self.slot = unsent.then_some(slot);
        Ok(result)
    }

    /// §8.3, §8.6: the store's one fenced authorization, immediately before the request, with
    /// the fingerprint of the exact bytes about to be sent and the slot held (`None` for the
    /// retry). It re-checks the lease and the account's identity, the durable `rejected_fp`
    /// (another process may have been refused this token since this one read its state), and
    /// the slot's validity, replacing a stale slot (a suspend or a slow refresh) or reserving
    /// one for the retry.
    /// - `LeaseLost`: nothing is sent or recorded, and the unsent slot stays counted.
    /// - `Rejected`: recorded as `token-expired` for the active account (its refusal is §7.5's
    ///   to handle, Task 11) or `http-401` for an inactive one; the slot goes back.
    /// - `OverBudget`: recorded as `over-budget`, backing off until a slot frees up; a stale
    ///   slot has already gone back.
    fn authorize(&mut self, bytes: &[u8]) -> Result<Slot, Stop> {
        let fp = self.access_fp(bytes);
        let held = self.slot.take();
        let grant = self.store.authorize_send(
            &self.reservation,
            held.as_ref(),
            fp.as_deref(),
            self.now_ms(),
            &self.budget,
        )?;
        match grant {
            SendGrant::Send(slot) => Ok(slot),
            SendGrant::LeaseLost => Err(Stop::LeaseLost),
            SendGrant::Rejected => {
                self.slot = held;
                self.rejected = fp;
                Err(failed(if self.active {
                    "token-expired"
                } else {
                    "http-401"
                }))
            }
            SendGrant::OverBudget { next_free_at } => Err(Stop::Failed(Failure {
                not_before: Some(next_free_at),
                ..Failure::new("over-budget")
            })),
        }
    }

    /// §8.3: a successor lost while collecting. Names the account by label and position, never
    /// a token; the refresh has quarantined it (`successor_lost`, `relogin_required`).
    fn warn_lost(&mut self) {
        self.warnings.push(format!(
            "{} (position {}) needs a new login: a refreshed token was lost while collecting usage",
            self.row.label, self.row.position
        ));
    }

    fn warn_refresh(&mut self, detail: &dyn fmt::Display) {
        self.warnings.push(format!(
            "could not refresh {} (position {}) to read its usage: {detail}",
            self.row.label, self.row.position
        ));
    }

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

    /// Phase 3 (§8.3), in a transaction fenced by the lease holder and the account's identity,
    /// so a late or superseded result is dropped. Success stores the reading, its samples and
    /// the next plan (§8.6). Failure never touches the last good reading, backs off (§8.5),
    /// and gives back, by its full identity, a slot whose request was never sent.
    fn record(mut self, fetched: Result<Vec<Window>, Stop>) -> Result<Outcome, EngineError> {
        hooks::point(self.engine, "usage-before-record")?;
        let now_s = self.now_s();
        let collected = match fetched {
            Ok(windows) => {
                let plan = self.plan(&windows, self.active, now_s);
                let retention = self.engine.settings().history_retention_days;
                if self
                    .store
                    .record_usage(&self.reservation, &windows, now_s, &plan, retention)?
                {
                    Collected::Recorded
                } else {
                    Collected::Dropped
                }
            }
            Err(Stop::Failed(f)) => {
                let n = self
                    .state
                    .as_ref()
                    .map_or(0, |s| s.consecutive_failures)
                    .saturating_add(1);
                let mut until = now_s + failure_backoff_s(n, f.is_429, f.retry_after_s);
                if let Some(not_before) = f.not_before {
                    until = until.max(not_before);
                }
                let recorded = self.store.record_usage_failure(
                    &self.reservation,
                    &f.kind,
                    now_s,
                    until,
                    f.is_429.then_some(until),
                    self.slot.as_ref(),
                )?;
                if recorded {
                    Collected::Failed { kind: f.kind }
                } else {
                    Collected::Dropped
                }
            }
            Err(Stop::Moved) => {
                if let Some(slot) = self.slot.take() {
                    self.store.release_slot(&self.reservation, &slot)?;
                }
                Collected::Dropped
            }
            Err(Stop::LeaseLost) => Collected::Dropped,
            Err(Stop::Error(e)) => return Err(e),
        };
        Ok((collected, self.warnings))
    }
}

#[cfg(test)]
mod tests {
    use super::jitter;

    #[test]
    fn jitter_is_a_fraction_in_the_policy_s_range() {
        for _ in 0..10_000 {
            let j = jitter();
            assert!((-1.0..=1.0).contains(&j), "{j}");
        }
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --lib collect`
Expected: PASS (1 test, `jitter_is_a_fraction_in_the_policy_s_range`).

Run: `cargo test -p tagteam-engine --test collect`
Expected: PASS (19 tests).

Run: `cargo test -p tagteam-engine --features test-hooks --test collect`
Expected: PASS (24 tests, including the five in `hooks`).

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS. No other suite collects usage, so no other test gains a request.

- [ ] **Step 6: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings. The second
run compiles `tests/collect.rs` without `test-hooks`: every import at its top is used outside
`mod hooks`.

- [ ] **Step 7: Commit**

```bash
git add crates/tagteam-engine/src/collect.rs crates/tagteam-engine/src/lib.rs \
  crates/tagteam-engine/tests/common/mod.rs crates/tagteam-engine/tests/collect.rs
git commit -m "Collect usage for inactive accounts, authorizing each request under the lease and the hourly budget"
```

---

### Task 11: Collector: the active account and the §7.5 triggers

§8.1 as amended: a usage fetch never refreshes the live token; only active-token refresh (§7.5)
does. Task 10's active path reads the live token and refuses to send it when it has expired or
was refused. This task hands both cases to §7.5 instead, and adds the retry:

- **Expired** (§7.2): §7.5 `Expired`, before the fetch.
- **Refused before** (the live access token's fingerprint equals the stored `rejected_fp`, the
  path Codex found): §7.5 `Rejected`, before any request. The refused token is never sent again.
- **401 on a token still valid locally** (the only kind `send` sends): stamp `rejected_fp` with
  it, then §7.5 `Rejected`, then one retry under its own slot. The stamp comes first, so a §7.5
  that fails still leaves it, and the next collection goes to §7.5 before any request.

The fetch goes on with the live token, read fresh after §7.5, and only if it is neither expired
nor the refused one; Task 10's `send` still refuses anything else as `token-expired`, giving its
slot back.

This task also makes the live read consistent. Task 10's `live_bytes` reads the live identity
and then the live credential as two separate reads. §9.4 writes the target's credential (step 7)
before its identity (step 8), so a tagteam switch finishing between the reads would have this
account's reservation send another account's token and record that account's usage as this
one's. `live_bytes` now reads under tagteam's mutation lock, which a switch holds for its whole
transaction: identity, credential, identity again (still this account's), then the lock is
dropped before anything is sent.

| §7.5 result | Recorded as | Warning |
|---|---|---|
| `Refreshed`, `PersistedNotPublished`, `PublishedOnly`, `NotNeeded` | the fetch goes on with the live token read fresh; `token-expired` if that is still expired or refused | none |
| `Dead` | `refresh-failed`; §7.5 quarantined the account, so it shows `relogin_required` | none |
| `Unpersisted` | `refresh-failed`; quarantined `successor_lost` | the lost-successor line |
| `Systemic`, `Transient` | `refresh-failed` (`unavailable`) | the refresh line |
| `Err(ForeignLiveCredential)` | `foreign-credential` | none |
| any other `Err` | `refresh-failed` (`unavailable`) | the refresh line, with the error |

**Readings of the spec this task commits to:**
- **An `Err` from §7.5 is never a command error** (M2a Task 16's carry-over): whether it came
  before any request or after a successor was stored (here, `abandon` keeping it in `rescue/`),
  it is recorded as `refresh-failed` with a warning, and `collect_usage` returns `Ok`.
- **`PersistedNotPublished` leaves the old token live.** §8.1 says the fetch goes on "with the
  live token, read fresh", so when the live store still holds the expired or refused token, the
  fetch reports `token-expired` rather than sending the vault's successor. The next collection
  reconciles through §7.5 (its step 3).
- **A kind that does not refresh never reaches §7.5.** §7.5 refuses it with an error
  (`active_login`), which would warn on every `list`. A live setup token that was refused is
  reported `token-expired`, with no request and no warning.
- **A live login that moves during §7.5** (it may wait up to 10 s for the mutation lock and 15 s
  for the account lock) is caught when the live token is read again: Task 10's `Dropped`.
- **The live identity and credential are read under the mutation lock** (`Engine::mutation_guard`,
  the plain one: it recovers a dead switch first like every caller, but never refuses; the
  collector is not a mutation, and `guard_or_refuse` would turn an unresolved switch into a
  command error). The lock is held only for the reads, never across a request (§8.3's "no lock
  held") and never across §7.5, which takes it itself; `live_bytes` drops it before returning.
  A lock that cannot be had within its 10 s timeout (a switch or another mutation holding it) is
  `Dropped`, like a live login that moved: nothing is sent or recorded, and the slot goes back.
  So is a provider whose `switch_journal` row is still there once the guard is held: the guard
  returns successfully even when its recovery of a dead switch failed (it could not take the
  provider's live locks), and a switch that died after writing the target's credential but
  before its identity passes both identity checks (still this account) while the live token is
  the target's. Any other error from it is `collect_usage`'s `Err`, like any store error. Residual: a login
  changed by Claude Code itself, outside tagteam's lock, between the credential read and the
  second identity read (its credential written, its `oauthAccount` not yet) still passes both
  identity checks, and can misattribute that one reading.
- A usage fetch never calls `refresh_stored` for the active account: the active path has no
  call to it, and `a_usage_fetch_refreshes_the_live_generation_never_the_vault_s` pins it (the
  gate would refuse the live account and could only ever send the vault's generation, which CC
  has already spent).

**Files:**
- Modify: `crates/tagteam-engine/src/collect.rs` (`active` and `live_bytes`, new `trigger`,
  `refresh_live` and `live_names_this_account`)
- Create: `crates/tagteam-engine/tests/collect_active.rs`

**Interfaces:**
- Consumes:
  - M2a Task 16: `Engine::refresh_active(&self, provider: &ProviderId, trigger: ActiveTrigger) -> Result<ActiveOutcome, EngineError>`,
    `ActiveTrigger::{Expired, Rejected { access_fp: String }}`,
    `ActiveOutcome::{NotNeeded { reconciled }, Refreshed, PersistedNotPublished, PublishedOnly, Dead(_), Systemic(String), Transient { kind: String }, Unpersisted}`,
    `EngineError::ForeignLiveCredential { position }`; its hook points `active-after-response`
  - M1: `Engine::mutation_guard(&self) -> Result<MutationGuard, EngineError>` (the lock a switch
    holds for its whole transaction, §9.4; 10 s timeout, `EngineError::Lock(_)` when it
    expires), `Engine::switch(&self, req: SwitchRequest) -> Result<SwitchOutcome, EngineError>`
    and `SwitchRequest: Clone` (tests)
  - Task 10: `Collection::{live_bytes, send, retry, reject, usable, is_rejected, access_fp, warn_lost, warn_refresh}`,
    `failed`, `windows`, `Stop` (its `Moved` variant); the fixture helpers
    `Fx::{script_usage, collect, usage_state}`, `usage_fixture`, `usage_bearers`,
    `usage_requests`, `access_fp`
  - Tests stamp `rejected_fp` through a raw `rusqlite` connection, as an earlier collection's
    401 would have: Task 8's `Store::set_rejected_fp` is fenced by a reservation the tests do
    not hold.
  - Fixture: `Fx::{with_lock_timeout, add, add_token_options, switch_to, switch_request, engine_with_env, live_email, script_refresh, script_token_error, rotate_live, live_credential, set_live_credential, live_refresh_token, vault_refresh_token, oauth_account, paths}`,
    `Fx.oracle`, `Fx.env`, and the free `token_requests`, `quarantine_of`, `credential`,
    `rescue_files`, `block_rescue`, `unblock_rescue`, and, for the unresolved-journal test,
    the M2a helpers `crashed_switch`, `write_target_credential` and `journal` (all `pub` in
    `tests/common/mod.rs`, as `tests/recover.rs` uses them)
  - M1: `Store::journal(&self, provider: &ProviderId) -> Result<Option<JournalRow>, StoreError>`
    (the switch journal row; one per provider, present while a switch is unresolved)
- Produces: the active account's collection per §8.1 as amended; no new public names. The
  `last_error` token `foreign-credential` (Decision 10). The test-hook point
  `usage-live-identity-read` (in `live_bytes`, under the mutation lock, between the first
  live-identity read and the live-credential read).

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-engine/tests/collect_active.rs`:

```rust
//! §8.1's active account: a usage fetch never refreshes the live token itself. An expired or
//! refused live token goes to active-token refresh (§7.5) first, and the fetch goes on only with
//! a usable, different token that §7.5 leaves in the live store.
mod common;

use std::fs;
use std::time::Duration;

use common::{
    Fx, access_fp, crashed_switch, credential, journal, quarantine_of, token_requests,
    usage_bearers, usage_fixture, usage_requests, write_target_credential,
};
use serde_json::{Value, json};
use tagteam_engine::collect::Collected;
use tagteam_engine::vault::SERVICE;
use tagteam_provider::{Clock, Method, Provider};

fn failed(kind: &str) -> Collected {
    Collected::Failed { kind: kind.into() }
}

/// The live access token as §7.2 counts it expired. Only `expiresAt` changes, so the live
/// generation is still the vault's.
fn expire_live(fx: &Fx) {
    let mut v = fx.live_credential().unwrap();
    v["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms());
    fx.set_live_credential(v.to_string().as_bytes());
}

/// The usage endpoint refusing the token.
fn refused() -> Value {
    json!({"type": "error", "error": {"type": "authentication_error", "message": "OAuth token has been revoked"}})
}

fn methods(fx: &Fx) -> Vec<Method> {
    fx.http.requests().iter().map(|r| r.method).collect()
}

/// The body of the first token-endpoint request.
fn token_request(fx: &Fx) -> Value {
    let sent = fx
        .http
        .requests()
        .into_iter()
        .find(|r| r.method == Method::Post)
        .unwrap();
    serde_json::from_slice(sent.body.as_deref().unwrap()).unwrap()
}

/// Stamps `id`'s `rejected_fp` with `fp` directly, as an earlier collection's 401 would have.
/// `Store::set_rejected_fp` is fenced by a reservation (Task 8), which these tests do not hold.
fn stamp_rejected(fx: &Fx, id: &tagteam_core::AccountId, fp: &str) {
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "INSERT INTO usage_state (account_id, rejected_fp) VALUES (?1, ?2) \
             ON CONFLICT(account_id) DO UPDATE SET rejected_fp = excluded.rejected_fp",
            rusqlite::params![id.as_str(), fp],
        )
        .unwrap();
}

/// Stamps `id`'s `rejected_fp` with `rt`'s access token, as an earlier 401 would have.
fn refuse(fx: &Fx, id: &tagteam_core::AccountId, rt: &str) {
    stamp_rejected(fx, id, &access_fp(fx, &credential("a@x.co", rt)));
}

#[test]
fn an_expired_live_token_is_refreshed_by_active_token_refresh_before_the_fetch() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    fx.script_refresh(Some("rt-a2"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(methods(&fx), [Method::Post, Method::Get], "§7.5 first, then the fetch");
    assert_eq!(
        fx.live_refresh_token().as_deref(),
        Some("rt-a2"),
        "published to the live store, which only §7.5 does"
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(usage_bearers(&fx), ["at-rt-a2"], "the live token, read fresh");
    assert_eq!(usage_requests(&fx), 1);
}

#[test]
fn a_usage_fetch_refreshes_the_live_generation_never_the_vault_s() {
    // CC rotated the live token to rt-c; the vault still holds rt-a, which CC has spent. The
    // gate refuses the live account (§7.3 step 2) and could only ever send the vault's rt-a;
    // §7.5 adopts rt-c first and refreshes that.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.rotate_live("rt-c");
    expire_live(&fx);
    fx.script_refresh(Some("rt-c2"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert_eq!(token_requests(&fx), 1);
    assert_eq!(token_request(&fx)["refresh_token"], "rt-c");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-c2"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-c2"));
    assert_eq!(usage_bearers(&fx), ["at-rt-c2"]);
}

#[test]
fn a_401_on_a_token_still_valid_locally_goes_to_active_refresh_and_retries_once() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.script_usage(401, refused());
    fx.script_usage(200, usage_fixture());
    fx.script_refresh(Some("rt-a2"));

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert_eq!(methods(&fx), [Method::Get, Method::Post, Method::Get]);
    assert_eq!(usage_bearers(&fx), ["at-rt-a", "at-rt-a2"]);
    assert_eq!(usage_requests(&fx), 2, "the retry has a slot of its own");
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
    assert_eq!(fx.usage_state(&a).unwrap().rejected_fp, None, "success clears it");
}

#[test]
fn a_stuck_rejected_fp_goes_to_active_refresh_before_any_request() {
    // The path Codex found: a 401 stamps the token, and §7.5 cannot replace it this time (its
    // request never leaves). The next collection must not send the refused token first.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.script_usage(401, refused()); // the only usage reply; nothing for the token endpoint

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))]);
    assert_eq!(
        report.warnings,
        ["could not refresh a@x.co (position 1) to read its usage: pre-send"]
    );
    assert_eq!(usage_bearers(&fx), ["at-rt-a"]);
    let stamped = access_fp(&fx, &credential("a@x.co", "rt-a"));
    assert_eq!(
        fx.usage_state(&a).unwrap().rejected_fp.as_deref(),
        Some(stamped.as_str())
    );

    // Past the lease and the backoff.
    fx.http.clear();
    fx.clock.advance_ms(91_000);
    fx.script_refresh(Some("rt-a2"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert_eq!(methods(&fx), [Method::Post, Method::Get], "§7.5 before any request");
    assert_eq!(usage_bearers(&fx), ["at-rt-a2"], "at-rt-a was never sent again");
    assert_eq!(fx.usage_state(&a).unwrap().rejected_fp, None);
}

#[test]
fn a_refused_token_that_active_refresh_left_live_is_never_sent() {
    // §7.5 refreshes the refused token but cannot publish the successor (CC holds its config
    // lock): the live store still holds the refused token, so nothing is sent.
    let fx = Fx::with_lock_timeout(Duration::from_millis(300));
    let a = fx.add("a@x.co", "rt-a");
    refuse(&fx, &a, "rt-a");
    fx.script_refresh(Some("rt-a2"));
    fx.script_usage(200, usage_fixture());
    fs::create_dir(fx.paths().config_lock).unwrap();

    let report = fx.collect(&[&a]);
    fs::remove_dir(fx.paths().config_lock).unwrap();

    assert_eq!(report.outcomes, [(a.clone(), failed("token-expired"))]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(token_requests(&fx), 1);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"), "persisted");
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"), "not published");
    assert!(usage_bearers(&fx).is_empty(), "the refused token is never sent again");
    assert_eq!(usage_requests(&fx), 0, "its unsent slot went back");
}

#[test]
fn a_dead_active_refresh_records_a_failure_and_sends_no_usage_request() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    fx.script_token_error(400, "invalid_grant");

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))]);
    assert!(
        report.warnings.is_empty(),
        "Dead shows as relogin_required, not a warning: {:?}",
        report.warnings
    );
    assert_eq!(quarantine_of(&fx, &a).0.as_deref(), Some("invalid_grant"));
    assert!(usage_bearers(&fx).is_empty());
    assert_eq!(usage_requests(&fx), 0);
}

#[test]
fn a_systemic_or_unsent_active_refresh_is_a_failure_with_a_warning() {
    let prefix = "could not refresh a@x.co (position 1) to read its usage: ";
    for systemic in [true, false] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        expire_live(&fx);
        if systemic {
            fx.script_token_error(400, "invalid_client");
        } // otherwise nothing is scripted: the request never leaves (pre-send)

        let report = fx.collect(&[&a]);

        assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))], "systemic: {systemic}");
        assert_eq!(report.warnings.len(), 1, "systemic: {systemic}");
        assert!(report.warnings[0].starts_with(prefix), "{:?}", report.warnings);
        if !systemic {
            assert_eq!(report.warnings[0], format!("{prefix}pre-send"));
        }
        assert_eq!(quarantine_of(&fx, &a).0, None, "never a strike: {systemic}");
        assert!(usage_bearers(&fx).is_empty());
        assert_eq!(usage_requests(&fx), 0);
    }
}

#[test]
fn an_error_from_active_refresh_is_a_failure_and_a_warning_never_a_command_error() {
    // §7.5 refuses before any request: live rt-a, vault rt-b, and an unreadable `.prev` that
    // may be rt-a (Task 16's tri-state rule).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.kc.put(SERVICE, a.as_str(), &credential("a@x.co", "rt-b"));
    fx.kc
        .put(SERVICE, &format!("{a}.prev"), &credential("a@x.co", "rt-a"));
    fx.kc.set_unreadable(SERVICE, &format!("{a}.prev"), true);
    expire_live(&fx);

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))]);
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].starts_with("could not refresh a@x.co (position 1) to read its usage: "),
        "{:?}",
        report.warnings
    );
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 0);
}

#[test]
fn a_live_credential_the_oracle_gives_to_someone_else_is_recorded_as_foreign() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    refuse(&fx, &a, "rt-a");
    fx.oracle.set(Some(
        fx.cc.parse_identity(&Fx::oauth_account("z@x.co")).unwrap(),
    ));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("foreign-credential"))]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 0);
}

#[test]
fn a_refused_live_setup_token_is_reported_expired_and_never_refreshed() {
    // A setup token does not refresh (§7.1): §7.5 would refuse it with an error, so its
    // refused token is reported expired, with no request and no warning.
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b");
    let s = fx
        .engine
        .add_token(fx.add_token_options("sk-ant-oat01-setup"))
        .unwrap()
        .account
        .id;
    fx.switch_to(&s, false).unwrap();
    let live = fx.live_credential().unwrap().to_string().into_bytes();
    stamp_rejected(&fx, &s, &access_fp(&fx, &live));
    fx.http.clear();

    let report = fx.collect(&[&s]);

    assert_eq!(report.outcomes, [(s.clone(), failed("token-expired"))]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(fx.http.requests().is_empty(), "no refresh and no usage request");
    assert_eq!(usage_requests(&fx), 0);
}

#[test]
fn an_unresolved_switch_journal_stops_the_active_collection_before_any_request() {
    // A switch from b to a died after writing a's credential (step 7) but before a's identity
    // (step 8): the live login still names b, the live credential is a's. Recovery cannot take
    // CC's refresh lock, so `mutation_guard` returns anyway and the journal row stays. Both
    // identity checks pass (still b), so without the journal check b's reservation would send
    // a's token and record a's usage as b's.
    let fx = Fx::with_lock_timeout(Duration::from_millis(300));
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fs::create_dir(fx.paths().refresh_lock).unwrap();
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&b]);
    fs::remove_dir(fx.paths().refresh_lock).unwrap();

    assert!(journal(&fx).is_some(), "recovery could not finish, so the row is still there");
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(report.outcomes, [(b.clone(), Collected::Dropped)]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(fx.http.requests().is_empty(), "a's token was not sent as b's");
    assert_eq!(usage_requests(&fx), 0, "the slot went back");
    assert_eq!(fx.usage_state(&b).and_then(|s| s.fetched_at), None, "no reading for b");
    assert_eq!(fx.usage_state(&a).and_then(|s| s.fetched_at), None, "nor for a");
}

#[cfg(feature = "test-hooks")]
mod hooks {
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Instant, SystemTime};

    use tagteam_engine::collect::CollectMode;

    use super::*;

    #[test]
    fn an_error_after_active_refresh_kept_its_successor_is_still_only_a_failure() {
        // M2a Task 16's carry-over: §7.5 returns an error after the successor was received and
        // kept (in rescue/). The collection records a failure and warns; the command goes on.
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        fx.engine.fail_at(Some("active-after-response"));

        let result = fx.engine.collect_usage(CollectMode::OnDemand {
            accounts: vec![a.clone()],
        });
        fx.engine.fail_at(None);

        let report = result.expect("a usage failure is never a command error (§8.3)");
        assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))]);
        assert_eq!(
            report.warnings,
            ["could not refresh a@x.co (position 1) to read its usage: injected failure at active-after-response"]
        );
        assert_eq!(
            common::rescue_files(&fx),
            1,
            "the successor was kept before the error returned"
        );
        assert!(usage_bearers(&fx).is_empty());
        assert_eq!(usage_requests(&fx), 0);
    }

    #[test]
    fn a_live_successor_held_nowhere_is_a_warning_naming_the_account() {
        // The vault and rescue/ both fail, and a takeover of CC's lock stops the live write:
        // §7.5's `Unpersisted`, quarantined `successor_lost` (§7.5 step 5).
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        fx.kc.set_fail_write(SERVICE, true);
        common::block_rescue(&fx);
        let lock_dir = fx.paths().refresh_lock;
        fx.engine.on_point(
            "active-after-response",
            Box::new(move || {
                // A takeover rewrites the lock directory's mtime (§9.1).
                fs::File::open(&lock_dir)
                    .unwrap()
                    .set_modified(SystemTime::now() + Duration::from_secs(60))
                    .unwrap();
            }),
        );

        let report = fx.collect(&[&a]);
        common::unblock_rescue(&fx);
        fx.kc.set_fail_write(SERVICE, false);

        assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))]);
        assert_eq!(
            report.warnings,
            ["a@x.co (position 1) needs a new login: a refreshed token was lost while collecting usage"]
        );
        assert!(!report.warnings[0].contains("rt-a"));
        assert_eq!(quarantine_of(&fx, &a).0.as_deref(), Some("successor_lost"));
        assert!(usage_bearers(&fx).is_empty());
    }

    #[test]
    fn a_switch_waits_for_the_live_read_so_the_account_sends_its_own_token() {
        // §9.4 writes a's credential before a's identity. Were the collector's reads of b's
        // live identity and credential not under the mutation lock, a switch to a finishing
        // between them would have b's reservation send a's token and record a's usage as b's.
        // The hook runs a switch on another thread between the two reads and gives it up to
        // 2 s to finish; under the lock it cannot, and it goes ahead once the reads are done.
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b"); // live
        fx.script_usage(200, usage_fixture());
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let request = fx.switch_request(&a, false);
        let switching: Arc<Mutex<Option<JoinHandle<bool>>>> = Arc::default();
        let handle_slot = switching.clone();
        fx.engine.on_point(
            "usage-live-identity-read",
            Box::new(move || {
                let (engine, req) = (other.clone(), request.clone());
                let switch = thread::spawn(move || engine.switch(req).is_ok());
                let deadline = Instant::now() + Duration::from_secs(2);
                while !switch.is_finished() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(20));
                }
                *handle_slot.lock().unwrap() = Some(switch);
            }),
        );

        let report = fx.collect(&[&b]);
        let switch = switching.lock().unwrap().take().expect("the hook ran");
        assert!(switch.join().unwrap(), "the switch went ahead once the reads were done");

        assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
        assert_eq!(report.outcomes, [(b.clone(), Collected::Recorded)]);
        assert_eq!(
            usage_bearers(&fx),
            ["at-rt-b"],
            "b's reservation sent b's own token"
        );
        assert!(
            fx.usage_state(&b).is_some_and(|s| s.fetched_at.is_some()),
            "the reading is recorded as b's"
        );
        assert_eq!(
            fx.usage_state(&a).and_then(|s| s.fetched_at),
            None,
            "nothing is recorded as a's"
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test collect_active`
Expected: FAIL: 13 of 14 tests. Task 10's active path refuses the expired or refused token
without calling §7.5, so the expired, rotated, Dead, systemic, unreadable-`.prev` and two §7.5
hook tests see `Failed { kind: "token-expired" }` where they expect `Recorded` or
`refresh-failed`; the 401 tests see `http-401`; the foreign one sees `token-expired`;
`a_refused_token_that_active_refresh_left_live_is_never_sent` fails on `token_requests == 1`
(it is 0); and `a_switch_waits_for_the_live_read_so_the_account_sends_its_own_token` panics on
`the hook ran`, since Task 10's `live_bytes` has no `usage-live-identity-read` point.
`an_unresolved_switch_journal_stops_the_active_collection_before_any_request` sees
`Recorded` where it expects `Dropped`, and a usage request where it expects none: Task 10's
`live_bytes` does not look at the journal, so b's reservation sends a's token.
`a_refused_live_setup_token_is_reported_expired_and_never_refreshed` already passes: Task 10's
`send` refuses a refused token, and this task must keep it passing.

- [ ] **Step 3: Hand the live token to §7.5, and read it under the mutation lock**

In `crates/tagteam-engine/src/collect.rs`, add to the `use crate::…` lines:

```rust
use crate::active::{ActiveOutcome, ActiveTrigger};
```

Replace the whole of `fn active` (its doc comment included) with:

```rust
    /// §8.1 for the active account. Only §7.5 refreshes the live token, never a usage fetch: an
    /// expired or refused live token is handed to it (`Expired`, `Rejected`) before anything is
    /// sent, and the fetch goes on with the live token, read fresh, only if §7.5 left one that
    /// is usable and different. A 401 on a token still valid locally (the only kind `send`
    /// sends) stamps `rejected_fp` first, then goes to §7.5 and retries once.
    fn active(&mut self) -> Result<Vec<Window>, Stop> {
        let mut live = self.live_bytes()?;
        if let Some(trigger) = self.trigger(&live) {
            live = self.refresh_live(trigger)?;
        }
        let first = self.send(&live)?;
        if !matches!(first, UsageResult::Unauthorized) {
            return windows(first);
        }
        self.reject(&live)?;
        let Some(trigger) = self.trigger(&live) else {
            return Err(failed("http-401"));
        };
        let next = self.refresh_live(trigger)?;
        self.retry(&next)
    }

    /// Why §7.5 must see the live token before it is sent: the server refused it
    /// (`rejected_fp`), or it has expired (§7.2).
    fn trigger(&self, live: &[u8]) -> Option<ActiveTrigger> {
        if self.is_rejected(live) {
            return self
                .access_fp(live)
                .map(|access_fp| ActiveTrigger::Rejected { access_fp });
        }
        expired(self.p, live, self.now_ms()).then_some(ActiveTrigger::Expired)
    }

    /// §7.5, then the live token it leaves, read fresh. `Refreshed`, `PersistedNotPublished`,
    /// `PublishedOnly` and `NotNeeded` go on; `send` still refuses a token that is expired or
    /// refused. `Dead` has quarantined the account (`relogin_required`). Any other outcome, and
    /// any error, is a failure with a warning, never a command error (M2a Task 16's
    /// carry-over), except a live credential the oracle gives to another identity, which has a
    /// status of its own. A kind that does not refresh never reaches §7.5, which would refuse
    /// it: its refused token is reported expired.
    fn refresh_live(&mut self, trigger: ActiveTrigger) -> Result<Vec<u8>, Stop> {
        if !self.p.kind_traits(&self.row.kind).refreshable {
            return Err(failed("token-expired"));
        }
        match self.engine.refresh_active(&self.row.provider, trigger) {
            Ok(
                ActiveOutcome::NotNeeded { .. }
                | ActiveOutcome::Refreshed
                | ActiveOutcome::PersistedNotPublished
                | ActiveOutcome::PublishedOnly,
            ) => self.live_bytes(),
            Ok(ActiveOutcome::Dead(_)) => Err(failed("refresh-failed")),
            Ok(ActiveOutcome::Unpersisted) => {
                self.warn_lost();
                Err(failed("refresh-failed"))
            }
            Ok(ActiveOutcome::Systemic(detail)) => {
                self.warn_refresh(&detail);
                Err(failed("refresh-failed"))
            }
            Ok(ActiveOutcome::Transient { kind }) => {
                self.warn_refresh(&kind);
                Err(failed("refresh-failed"))
            }
            Err(EngineError::ForeignLiveCredential { .. }) => Err(failed("foreign-credential")),
            Err(e) => {
                self.warn_refresh(&e);
                Err(failed("refresh-failed"))
            }
        }
    }
```

Replace the whole of `fn live_bytes` (its doc comment included) with the consistent read, and
its identity check:

```rust
    /// The live credential, read while the live login names this account, under tagteam's
    /// mutation lock. A switch holds that lock for its whole transaction and writes the
    /// target's credential before its identity (§9.4), so without it a switch finishing
    /// between the reads would have this reservation send, and record, another account's token.
    /// Under the lock: the live identity, the live credential, then the identity again, which
    /// must still name this account. The lock is dropped before returning, so it is never held
    /// across a request (§8.3) or across §7.5, which takes it itself.
    ///
    /// A live login that moved stops the fetch (`Moved`): its token is not this account's. So
    /// does a lock that cannot be had within its timeout (a switch or another mutation holding
    /// it. So does a switch journal row still present once the guard is held: the guard returns
    /// even when its recovery of a dead switch failed, and that switch may have written another
    /// account's credential before its identity, which both identity checks would pass.
    /// Residual: a login changed by Claude Code itself, outside tagteam's lock, between the
    /// credential read and the second identity read (its credential written, its `oauthAccount`
    /// not yet) passes both checks and can misattribute that one reading.
    fn live_bytes(&self) -> Result<Vec<u8>, Stop> {
        let guard = match self.engine.mutation_guard() {
            Ok(guard) => guard,
            Err(EngineError::Lock(_)) => return Err(Stop::Moved),
            Err(e) => return Err(e.into()),
        };
        if self.store.journal(&self.row.provider)?.is_some() {
            drop(guard);
            return Err(Stop::Moved);
        }
        self.live_names_this_account()?;
        hooks::point(self.engine, "usage-live-identity-read")?;
        let credential = self.p.read_live_auth(&self.engine.env).credential;
        self.live_names_this_account()?;
        drop(guard);
        match credential {
            Read::Present(c) if c.provenance() == Provenance::Degraded => {
                Err(failed("keychain-unavailable"))
            }
            Read::Present(c) if !c.is_empty() => Ok(c.bytes().to_vec()),
            Read::Present(_) | Read::Absent => Err(failed("no-access-token")),
            Read::Unreadable(_) => Err(failed("keychain-unavailable")),
        }
    }

    /// The live login still names this account; `Moved` otherwise.
    fn live_names_this_account(&self) -> Result<(), Stop> {
        match self.p.live_identity(&self.engine.env) {
            Read::Present(i) if self.p.identity_key(&i).as_str() == self.row.identity_key => Ok(()),
            _ => Err(Stop::Moved),
        }
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --features test-hooks --test collect_active`
Expected: PASS (14 tests).

Run: `cargo test -p tagteam-engine --test collect_active`
Expected: PASS (11 tests; the three in `hooks` need `test-hooks`).

Run: `cargo test -p tagteam-engine --features test-hooks --test collect --test active`
Expected: PASS. Task 10's tests are unchanged by this task (none of them sends an expired or
refused live token, and none holds the mutation lock while an active account is collected, so
the lock `live_bytes` now takes is always free to them), and §7.5's own suite is untouched.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS.

- [ ] **Step 5: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.
`tests/collect_active.rs`'s top-level imports are all used outside `mod hooks`, which imports
`Arc`, `Mutex`, `thread`, `JoinHandle`, `Instant`, `SystemTime` and `CollectMode` itself.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/collect.rs crates/tagteam-engine/tests/collect_active.rs
git commit -m "Hand an expired or refused live token to active-token refresh, and read the live login under the mutation lock, stopping while a switch is unresolved"
```

---

### Task 12: Post-switch poll re-plan

§8.3: "After any switch that activated an account, it re-plans polls from the last readings
without fetching. The incoming account's `next_poll_at` gets the active-account policy, and the
outgoing account's gets the candidate policy (§8.6)." This is M1's carried item L459. Without
it, the account that just became live keeps the candidate's slower plan, and the one that left
keeps the active account's faster one, until each is next fetched.

An account with no reading is the exception, and it is deliberate. §8.3's on-demand rule is
"older than 180 s *and* (a poll is due *or* no plan exists)", so a plan in force makes an unread
account ineligible until the plan comes due. Planning it here would make `list` right after a
switch skip the account it most needs to read.

**Readings of the spec this task commits to:**
- **"From the last readings" means the readings stay as they are.** The re-plan never fetches
  and never touches `last_good`, `fetched_at` or the failure fields. It changes only the plan, to
  the role's default through Task 4's `replan_for_role`: 180 s for the incoming account and
  300 s for the outgoing one, jittered ±10 % and never under the 180 s floor. It skips an
  account that has no reading (next item).
- **Only an account that has a reading is re-planned.** A reading is a `usage_state` row with a
  `fetched_at`. An account without one (never fetched, or fetched only to fail, which never sets
  `fetched_at`) is left exactly as it is, with no plan created, so the next on-demand collect
  finds it eligible and fetches it.
- **Which switches re-plan:** those that reach the transaction and commit, `Switched` and
  `Activated`. A self-switch (`Activated`, from and to the same account) re-plans that account
  once, as the active one. No-ops (`AlreadyActive`, `OnlyOneAccount`, `UnmanagedAccount`,
  `NoValidTarget`) and refusals re-plan nothing. An outgoing login that was unmanaged has no
  row to re-plan.
- **Best effort, after the locks.** The switch has committed, so a store failure here, reading
  the row or writing the plan, is logged at WARN (position and ID) and never fails it. The
  re-plan runs after the account and live locks are released and before the mutation lock is,
  since it needs none of them.
- **Providers without the `usage` capability** get no re-plan: they have no usage rows to plan.
- **Recovery** (§9.6) that completes an interrupted switch does not re-plan: it is not a switch
  command activating an account, and the next collection plans from its own fetch.

**Files:**
- Modify: `crates/tagteam-engine/src/switch.rs` (`switch`, new `replan_polls`)
- Modify: `crates/tagteam-engine/tests/switch.rs`

**Interfaces:**
- Consumes:
  - Task 4: `tagteam_core::poll::replan_for_role(b: &PollBudget, active: bool, now_s: i64, jitter: f64) -> PollPlan`
    (`next_poll_at` carries the jitter and the floor)
  - Task 7: `Provider::poll_budget(&self) -> PollBudget`, `Capabilities.usage`
  - Task 8: `Store::usage_state(&self, id: &AccountId) -> Result<Option<UsageStateRow>, StoreError>`
    (`fetched_at` says whether the account has a reading) and
    `Store::set_poll_plan(&self, id: &AccountId, plan: &PollPlan) -> Result<(), StoreError>`
    (creates the row if missing, so this task never calls it for an account without a reading)
  - Task 10: `crate::collect::jitter() -> f64`; the fixture helpers `Fx::{usage_state, collect, script_usage}`
    and `usage_fixture`
  - M2a: `Engine::switch`, `SwitchOutcome { from, to, reason, .. }`, `Engine::transact`
- Produces: `fn replan_polls(&self, p: &dyn Provider, store: &Store, outcome: &SwitchOutcome)`
  (private to `switch.rs`), called after every switch that commits. It re-plans the incoming
  and outgoing accounts that have a reading, and leaves the others without a plan.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/switch.rs`, change the `common` import to:

```rust
use common::{API_KEY, Fx, OTHER_API_KEY, STRAY_API_KEY, mutation_lock_free, usage_fixture};
use tagteam_core::poll::PollPlan;
```

and append:

```rust
/// When `id`'s next poll is planned, in seconds from now.
fn planned_in(fx: &Fx, id: &AccountId) -> i64 {
    fx.usage_state(id).unwrap().next_poll_at.unwrap() - fx.engine.now_ms() / 1000
}

/// Gives each of `ids` a reading (and so a plan), then forgets the requests that took.
fn read_each(fx: &Fx, ids: &[&AccountId]) {
    for _ in ids {
        fx.script_usage(200, usage_fixture());
    }
    fx.collect(ids);
    for id in ids {
        assert!(fx.usage_state(id).unwrap().fetched_at.is_some());
    }
    fx.http.clear();
}

#[test]
fn a_switch_re_plans_both_accounts_polls_without_fetching() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    read_each(&fx, &[&a, &b]);
    let (a_read, b_read) = (fx.usage_state(&a).unwrap(), fx.usage_state(&b).unwrap());

    let out = switch(&fx, to(&a), false).unwrap();

    assert_eq!(out.reason, SwitchReason::Switched);
    // §8.6's defaults, jittered ±10%, never under the 180 s floor.
    let incoming = planned_in(&fx, &a);
    assert!((180..=198).contains(&incoming), "the active account's policy: {incoming}");
    let outgoing = planned_in(&fx, &b);
    assert!((270..=330).contains(&outgoing), "the candidate policy: {outgoing}");
    for (id, before) in [(&a, a_read), (&b, b_read)] {
        let after = fx.usage_state(id).unwrap();
        assert_eq!(
            (&after.last_good, after.fetched_at),
            (&before.last_good, before.fetched_at),
            "the reading stays"
        );
    }
    assert!(fx.http.requests().is_empty(), "a re-plan never fetches");
}

#[test]
fn a_re_plan_keeps_the_last_reading() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.script_usage(200, usage_fixture());
    fx.collect(&[&a]);
    let before = fx.usage_state(&a).unwrap();
    assert!(before.last_good.is_some());
    fx.http.clear();

    switch(&fx, to(&a), false).unwrap();

    let after = fx.usage_state(&a).unwrap();
    assert_eq!(
        (&after.last_good, after.fetched_at, after.consecutive_failures),
        (&before.last_good, before.fetched_at, 0)
    );
    assert!((180..=198).contains(&planned_in(&fx, &a)));
    assert!(fx.http.requests().is_empty());
}

#[test]
fn an_account_with_no_reading_is_not_re_planned_and_is_fetched_on_demand() {
    // §8.3's on-demand rule treats a plan in force as "not due", so a plan here would keep
    // `list` from reading the account it most needs to read.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    read_each(&fx, &[&b]);

    let out = switch(&fx, to(&a), false).unwrap();

    assert_eq!(out.reason, SwitchReason::Switched);
    assert_eq!(fx.usage_state(&a), None, "the incoming account has no reading: no plan");
    let outgoing = planned_in(&fx, &b);
    assert!((270..=330).contains(&outgoing), "b has one: the candidate policy: {outgoing}");

    fx.script_usage(200, usage_fixture());
    let report = fx.collect(&[&a]);

    assert_eq!(fx.http.requests().len(), 1, "the next on-demand collect fetches it");
    assert!(fx.usage_state(&a).unwrap().fetched_at.is_some(), "{:?}", report.outcomes);
}

#[test]
fn the_outgoing_account_without_a_reading_is_left_unplanned_too() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    read_each(&fx, &[&a]);

    switch(&fx, to(&a), false).unwrap();

    assert!((180..=198).contains(&planned_in(&fx, &a)));
    assert_eq!(fx.usage_state(&b), None, "b was never read: no plan");
}

#[test]
fn a_forced_self_switch_plans_only_that_account_as_the_active_one() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    read_each(&fx, &[&b]);
    // Park b's plan far out, so only a re-plan can bring it back to the active policy.
    fx.engine
        .store()
        .unwrap()
        .set_poll_plan(
            &b,
            &PollPlan { interval_s: 3_600, next_poll_at: fx.engine.now_ms() / 1000 + 3_600 },
        )
        .unwrap();

    let out = switch(&fx, to(&b), true).unwrap();

    assert_eq!(out.reason, SwitchReason::Activated);
    assert!((180..=198).contains(&planned_in(&fx, &b)));
    assert_eq!(fx.usage_state(&a), None, "a was not part of this switch");
}

#[test]
fn a_switch_that_activates_nothing_re_plans_nothing() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");

    let out = switch(&fx, to(&b), false).unwrap();

    assert_eq!(out.reason, SwitchReason::AlreadyActive);
    assert_eq!(fx.usage_state(&a), None);
    assert_eq!(fx.usage_state(&b), None);
}

#[test]
fn a_re_plan_that_cannot_be_stored_never_fails_the_switch() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    read_each(&fx, &[&a]);
    let before = fx.usage_state(&a).unwrap().next_poll_at;
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER no_insert BEFORE INSERT ON usage_state \
               BEGIN SELECT RAISE(ABORT, 'usage_state is read-only'); END;
             CREATE TRIGGER no_update BEFORE UPDATE ON usage_state \
               BEGIN SELECT RAISE(ABORT, 'usage_state is read-only'); END;",
        )
        .unwrap();

    let out = switch(&fx, to(&a), false).unwrap();

    assert!(out.switched, "{}", out.message);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(fx.usage_state(&a).unwrap().next_poll_at, before, "the plan is as it was");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --test switch`
Expected: FAIL, five tests, each on a range assertion, because no switch re-plans anything yet
and every collected reading's own plan is still in place:
- `a_switch_re_plans_both_accounts_polls_without_fetching`: a's plan is a candidate's,
  405–495 s out, not 180–198 s.
- `a_forced_self_switch_plans_only_that_account_as_the_active_one`: b's parked plan, an hour
  out, is still in place.
- `a_re_plan_keeps_the_last_reading`: a's plan is a candidate's, 405–495 s out.
- `an_account_with_no_reading_is_not_re_planned_and_is_fetched_on_demand`: b was read as the
  live account, so its plan is the active policy's, under 270 s, not 270–330 s.
- `the_outgoing_account_without_a_reading_is_left_unplanned_too`: a was read as a candidate, so
  its plan is 405–495 s out, not 180–198 s.

The two tests that assert an account stays unplanned hold with no re-plan at all, and must keep
holding once one exists. `a_switch_that_activates_nothing_re_plans_nothing` and
`a_re_plan_that_cannot_be_stored_never_fails_the_switch` already pass, and must keep passing.

- [ ] **Step 3: Re-plan after the commit**

In `crates/tagteam-engine/src/switch.rs`, add below the `use tagteam_core::{…};` block:

```rust
use tagteam_core::poll::replan_for_role;
```

and to the `use crate::…` lines, after `use crate::account_lock::AccountLock;`:

```rust
use crate::collect::jitter;
```

In `Engine::switch`, replace:

```rust
                Rederived::Go(locked) => {
                    return self.transact(p, &store, &plan, locked, &accounts, &locks, &req);
                }
```

with:

```rust
                Rederived::Go(locked) => {
                    let outcome =
                        self.transact(p, &store, &plan, locked, &accounts, &locks, &req)?;
                    // The re-plan needs no lock and never fetches (§8.3).
                    drop(locks);
                    drop(accounts);
                    self.replan_polls(p, &store, &outcome);
                    return Ok(outcome);
                }
```

Then add this method to the same `impl Engine` block, directly after `fn switch`:

```rust
    /// §8.3: after a switch that activated an account, both accounts' polls are re-planned
    /// for their new roles, without fetching: the incoming account gets the active account's
    /// policy and the outgoing one the candidate policy (§8.6). The readings stay as they are.
    /// An account with no reading is skipped, so it keeps no plan and stays eligible on demand
    /// (§8.3: a plan in force would make it not due). The switch has committed, so this is best
    /// effort: a failure is logged at WARN and never fails it.
    fn replan_polls(&self, p: &dyn Provider, store: &Store, outcome: &SwitchOutcome) {
        let Some(to) = &outcome.to else {
            return;
        };
        if !p.capabilities().usage {
            return;
        }
        let budget = p.poll_budget();
        let now_s = self.now_ms().div_euclid(1000);
        let outgoing = outcome.from.as_ref().filter(|from| from.id != to.id);
        for (row, active) in [(Some(to), true), (outgoing, false)] {
            let Some(row) = row else {
                continue;
            };
            let result = store.usage_state(&row.id).and_then(|state| {
                if state.is_none_or(|s| s.fetched_at.is_none()) {
                    return Ok(());
                }
                store.set_poll_plan(&row.id, &replan_for_role(&budget, active, now_s, jitter()))
            });
            if let Err(e) = result {
                tracing::warn!(
                    position = row.position,
                    account = %row.id,
                    "could not re-plan usage polls after the switch: {e}"
                );
            }
        }
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --test switch`
Expected: PASS, including the seven new tests.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS. The re-plan writes only `usage_state` rows, in the store, which the §15.3
snapshot walk excludes (it skips tagteam's data dir), so no invariant or rollback test moves.

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
git add crates/tagteam-engine/src/switch.rs crates/tagteam-engine/tests/switch.rs
git commit -m "Re-plan usage polls after a switch for accounts that have a reading"
```

---

### Task 13: Usage views, history and statusline views

The engine's views learn usage. Every `AccountView` carries a `UsageView`: the account's
`usageStatus` (Decision 10's table below), its last good reading with §8.7's pace for each window,
and whether that reading is decision-grade (§8.4). `history` reads a window's samples (§13.4), and
`statusline` finds the live login through `live_identity_cache` (§13.5). Everything here reads the
store; nothing fetches, and `statusline` also never touches the Keychain and never creates the
store.

Decision 10's table. The first row that matches wins:

| # | Condition | `UsageStatus` |
|---|---|---|
| 1 | The provider lacks the `usage` capability, or is not registered | `Unsupported` |
| 2 | The kind is on the managed-key axis | `ApiKey` |
| 3 | The account is quarantined | `ReloginRequired` |
| 4 | Failing (`consecutive_failures > 0`) with `last_error` `foreign-credential` | `ForeignCredential` |
| 5 | Failing with `keychain-unavailable` | `KeychainUnavailable` |
| 6 | Failing with `no-access-token` or `vault-absent` | `NoCredentials` |
| 7 | Failing with `token-expired` | `TokenExpired` |
| 8 | Failing with any other `last_error` | `Unavailable`, `error` = `last_error` |
| 9 | A reading (`fetched_at` is set) | `Ok` |
| 10 | No reading and no failure | `Unavailable`, `error` = `no-data` |

`last_error` counts only while `consecutive_failures > 0`, so a token left behind by a failure
that a later success reset never shows. An account has a reading when its `fetched_at` is set:
Task 8 stores a reading with no windows as a null `last_good`, which the view shows as a reading
of no windows (`Some(vec![])`). Row 10 keeps M1's `usageError: "no-data"` for an account that was
never read. For `Unavailable`, `retry_at` is `max(backoff_until, next_poll_at)`, the earliest
time an on-demand collect may fetch again (§8.3).

Judgement calls, flagged rather than decided silently:
- **Pace uses one indexed query per window.** `usage_samples(id, Some(key), fetched_at − 48 h)`
  walks the samples' primary key (account, window, time). A single query for every window would
  scan all of an account's samples with no time bound on the key, which 180 days of retention
  makes too slow for `list`'s 50 ms and `statusline`'s 10 ms (§13.5).
- **The cache is stamped by a stat taken before the parse.** A rewrite between the two leaves
  the old stamp on the new identity, which the next run's stat notices. The other order could
  cache an old identity under a new stamp for as long as the file stays unchanged.
- **An unreadable or garbled `~/.claude.json` is not cached.** It reads as `NoLogin` and is
  parsed again on the next run. A missing file is `NoLogin` without a parse.
- **Without a store, nothing is cached.** `statusline` parses the source on every run until a
  command that writes has created the store; it never creates it.
- `account_view` stays infallible. A store that cannot be read leaves the row unread, with that
  failure's kind as its `error`, logged by position and ID: its callers report a change that has
  already committed.
- `history` with no `window` shows the relevant windows (§13.4) under the loaded
  `autoswitch.models`. With one, it shows any window whose key or label matches, spend included.

**Files:**
- Modify: `crates/tagteam-engine/src/views.rs`
- Modify: `crates/tagteam/src/render.rs` (its test helper only: `AccountView` gains `usage`)
- Modify: `crates/tagteam-engine/tests/invariant.rs` (`statusline` joins the pinned walk)
- Create: `crates/tagteam-engine/tests/views_usage.rs`

**Interfaces:**
- Consumes:
  - Task 2: `tagteam_core::{Window, WindowKind}` and
    `tagteam_core::usage::{earliest_relevant_reset, is_relevant}`.
  - Task 3: `tagteam_core::TrustInputs` and `tagteam_core::trust::decision_grade`.
  - Task 4: `tagteam_core::{PollBudget, PollPlan}` (`PollBudget::STANDARD`, in the tests).
  - Task 5: `tagteam_core::{Pace, ProjectionMethod, Sample}` and `tagteam_core::pace::pace`.
  - Task 7: `Provider::capabilities().usage` and `Provider::live_identity_source`.
  - Task 8, re-exported from `tagteam_engine::store`: `Store::{usage_state, usage_samples,
    usage_lease_live, live_identity_cache, put_live_identity_cache}`, `UsageStateRow` and
    `LiveIdentityCacheRow`; the tests also record readings through `Store::{reserve_usage,
    record_usage, record_usage_failure}`, `Reserve` and `Reservation`, as the collector does.
  - Task 9: `Engine::settings()` (its `models`) and `EngineConfig.settings`; Task 1's
    `tagteam_engine::settings::Settings`.
  - M2a's `Fx`: `add`, `add_api_key`, `add_options`, `quarantine`, `login`, `paths`, and its
    `clock`, `http`, `kc`, `cc`, `oracle`; `common::CLAUDE_JSON`.
- Produces: the Interface Contract's `views.rs` exactly: `UsageStatus` and `as_str`,
  `UsageView`, `AccountView.usage`, `HistoryWindow`, `HistoryView`, `StatuslineView`,
  `Engine::history` and `Engine::statusline`. Also `pub const NO_DATA: &str = "no-data"`, the
  `error` of an account never read, which Task 14 renders.

- [ ] **Step 1: Write the failing unit tests**

Append to the end of `crates/tagteam-engine/src/views.rs`:
```rust
#[cfg(test)]
mod tests {
    use serde_json::json;
    use tagteam_core::{CLAUDE_CODE, WindowKind};

    use super::*;

    const OAUTH: KindTraits = KindTraits {
        refreshable: true,
        managed_key_axis: false,
        default_email_prefix: None,
        display: None,
    };

    const API_KEY: KindTraits = KindTraits {
        refreshable: false,
        managed_key_axis: true,
        default_email_prefix: Some("api-key"),
        display: Some("api key"),
    };

    fn row(quarantined: bool) -> AccountRow {
        AccountRow {
            id: AccountId::from_string("0192"),
            provider: ProviderId::new(CLAUDE_CODE),
            position: 1,
            identity_key: "a@x.co\n".into(),
            label: "a@x.co".into(),
            email: Some("a@x.co".into()),
            org_uuid: String::new(),
            org_name: None,
            account_uuid: None,
            kind: "oauth".into(),
            alias: None,
            disabled: false,
            identity_json: json!({}),
            login_expires_at: None,
            login_epoch: 0,
            replacing_fp: None,
            quarantine_reason: quarantined.then(|| "invalid_grant".into()),
            quarantine_fp: None,
            quarantine_at: None,
            added_at: 1,
        }
    }

    /// A `usage_state` row with `failures` and `last_error`, and a reading when `read`.
    fn state(failures: u32, last_error: Option<&str>, read: bool) -> UsageStateRow {
        UsageStateRow {
            account_id: AccountId::from_string("0192"),
            last_good: read.then(|| {
                vec![Window {
                    key: "5h".into(),
                    label: "5h".into(),
                    kind: WindowKind::Short,
                    pct: 9.0,
                    resets_at: None,
                    period_s: None,
                    detail: None,
                }]
            }),
            fetched_at: read.then_some(1),
            last_attempt_at: None,
            consecutive_failures: failures,
            last_error: last_error.map(str::to_owned),
            backoff_until: None,
            next_poll_at: None,
            poll_interval_s: None,
            last_429_at: None,
            rejected_fp: None,
        }
    }

    #[test]
    fn usage_status_follows_the_table_row_by_row() {
        use UsageStatus::*;
        let status =
            |supported: bool, kind: KindTraits, quarantined: bool, state: Option<UsageStateRow>| {
                usage_status(supported, &kind, &row(quarantined), state.as_ref())
            };
        let failing = |error: &str| Some(state(1, Some(error), true));
        // The first row that matches wins.
        assert_eq!(
            status(false, API_KEY, true, failing("http-429")),
            Unsupported
        );
        assert_eq!(status(true, API_KEY, true, failing("http-429")), ApiKey);
        assert_eq!(
            status(true, OAUTH, true, failing("foreign-credential")),
            ReloginRequired
        );
        for (error, want) in [
            ("foreign-credential", ForeignCredential),
            ("keychain-unavailable", KeychainUnavailable),
            ("no-access-token", NoCredentials),
            ("vault-absent", NoCredentials),
            ("token-expired", TokenExpired),
            ("http-429", Unavailable),
            ("refresh-failed", Unavailable),
        ] {
            assert_eq!(status(true, OAUTH, false, failing(error)), want, "{error}");
        }
        // A last_error left from before a success no longer counts.
        let succeeded = Some(state(0, Some("http-429"), true));
        assert_eq!(status(true, OAUTH, false, succeeded), Ok);
        assert_eq!(status(true, OAUTH, false, Some(state(0, None, true))), Ok);
        // A reading of no windows is still a reading.
        let empty = UsageStateRow {
            last_good: None,
            ..state(0, None, true)
        };
        assert_eq!(status(true, OAUTH, false, Some(empty)), Ok);
        // Never read and never failed: no data yet.
        assert_eq!(
            status(true, OAUTH, false, Some(state(0, None, false))),
            Unavailable
        );
        assert_eq!(status(true, OAUTH, false, None), Unavailable);
    }

    #[test]
    fn usage_status_strings_are_pinned() {
        use UsageStatus::*;
        let all = [
            (Ok, "ok"),
            (TokenExpired, "token_expired"),
            (ApiKey, "api_key"),
            (KeychainUnavailable, "keychain_unavailable"),
            (ReloginRequired, "relogin_required"),
            (ForeignCredential, "foreign_credential"),
            (NoCredentials, "no_credentials"),
            (Unavailable, "unavailable"),
            (Unsupported, "unsupported"),
        ];
        for (status, text) in all {
            assert_eq!(status.as_str(), text);
        }
    }

    #[test]
    fn a_view_without_a_reading_says_why_only_when_unavailable() {
        let none = UsageView::unread(UsageStatus::Unavailable, None);
        assert_eq!(none.error.as_deref(), Some(NO_DATA));
        let store = UsageView::unread(UsageStatus::Unavailable, Some("store".into()));
        assert_eq!(store.error.as_deref(), Some("store"));
        let key = UsageView::unread(UsageStatus::ApiKey, Some("store".into()));
        assert_eq!(
            (key.error, key.windows, key.decision_grade),
            (None, None, false)
        );
    }
}
```

- [ ] **Step 2: Write the failing engine tests**

Create `crates/tagteam-engine/tests/views_usage.rs`:
```rust
//! The usage views (§8.4, §8.7, §13.2, §13.4, §13.5): each account's status and last good
//! reading with pace and trust, `history`, and `statusline`'s cached live identity. Readings
//! are recorded through the store's own reserve-and-record calls, as the collector records
//! them, at times chosen by the test.

mod common;

use common::Fx;
use serde_json::json;
use tagteam_core::pace::pace;
use tagteam_core::{
    AccountId, Pace, PollBudget, PollPlan, ProjectionMethod, Sample, Window, WindowKind,
};
use tagteam_engine::Engine;
use tagteam_engine::store::{Reservation, Reserve};
use tagteam_engine::views::{NO_DATA, StatusView, UsageStatus, UsageView};

/// The fixture clock's start (`Fx`), in seconds.
const T0: i64 = 1_790_000_000;

fn window(key: &str, label: &str, kind: WindowKind, pct: f64, resets_at: i64) -> Window {
    Window {
        key: key.into(),
        label: label.into(),
        kind,
        pct,
        resets_at: Some(resets_at),
        period_s: match kind {
            WindowKind::Short => Some(18_000),
            _ => Some(604_800),
        },
        detail: None,
    }
}

/// A Claude Code reading taken at `at`: 5h at `five` (resetting 2h40m30s later), 7d at `seven`
/// and Fable at 0 (both resetting at T0 + 3d09h00m30s, the same window instance for every
/// reading), and €0 of €20 spend.
fn reading(at: i64, five: f64, seven: f64) -> Vec<Window> {
    vec![
        window("5h", "5h", WindowKind::Short, five, at + 9_630),
        window("7d", "7d", WindowKind::Long, seven, T0 + 291_630),
        window(
            "scoped:Fable",
            "Fable",
            WindowKind::Scoped,
            0.0,
            T0 + 291_630,
        ),
        Window {
            key: "spend".into(),
            label: "spend".into(),
            kind: WindowKind::Spend,
            pct: 0.0,
            resets_at: None,
            period_s: None,
            detail: Some(json!({"used": 0.0, "limit": 20.0, "currency": "EUR"})),
        },
    ]
}

/// A slot and the `usage:<id>` lease at `at`, as the collector's phase 1 takes them.
fn reserve(fx: &Fx, id: &AccountId, at: i64) -> Reservation {
    let store = fx.engine.store().unwrap();
    let row = store.account(id).unwrap().unwrap();
    match store
        .reserve_usage(&row, at * 1000, false, &PollBudget::STANDARD)
        .unwrap()
    {
        Reserve::Reserved(r) => r,
        other => panic!("not reserved at {at}: {other:?}"),
    }
}

/// Records `windows` as `id`'s reading at `at`, with its next poll planned at `next_poll_at`.
fn record(fx: &Fx, id: &AccountId, windows: &[Window], at: i64, next_poll_at: i64) {
    let r = reserve(fx, id, at);
    let plan = PollPlan {
        interval_s: next_poll_at - at,
        next_poll_at,
    };
    let store = fx.engine.store().unwrap();
    assert!(store.record_usage(&r, windows, at, &plan, 180).unwrap());
}

/// Records a failed fetch of `kind` at `at`, backing off until `backoff_until`.
fn fail(
    fx: &Fx,
    id: &AccountId,
    kind: &str,
    at: i64,
    backoff_until: i64,
    last_429_at: Option<i64>,
) {
    let r = reserve(fx, id, at);
    let store = fx.engine.store().unwrap();
    assert!(
        store
            .record_usage_failure(&r, kind, at, backoff_until, last_429_at, None)
            .unwrap()
    );
}

fn at(fx: &Fx, s: i64) {
    fx.clock.set(s * 1000);
}

/// `id`'s usage as `list` shows it.
fn usage_of(engine: &Engine, id: &AccountId) -> UsageView {
    engine
        .accounts(None)
        .unwrap()
        .into_iter()
        .flat_map(|l| l.accounts)
        .find(|v| &v.row.id == id)
        .unwrap()
        .usage
}

/// §8.7's pace of `w` read at `fetched_at`, from exactly these `(fetched_at, pct)` samples of
/// its window instance.
fn expected(w: &Window, fetched_at: i64, samples: &[(i64, f64)]) -> Pace {
    let samples: Vec<Sample> = samples
        .iter()
        .map(|&(t, pct)| Sample {
            fetched_at: t,
            pct,
            resets_at: w.resets_at,
        })
        .collect();
    pace(w, fetched_at, &samples)
}

#[test]
fn a_fresh_reading_is_ok_decision_grade_and_carries_pace() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let windows = reading(T0, 9.0, 77.0);
    record(&fx, &a, &windows, T0, T0 + 180);

    let u = usage_of(&fx.engine, &a);
    assert_eq!(
        (u.status, u.decision_grade, u.fetched_at, u.age_s),
        (UsageStatus::Ok, true, Some(T0), Some(0))
    );
    assert_eq!((u.error, u.retry_at), (None, None));
    let shown = u.windows.unwrap();
    assert_eq!(shown.len(), windows.len());
    for ((w, p), stored) in shown.iter().zip(&windows) {
        assert_eq!(w, stored);
        assert_eq!(*p, expected(w, T0, &[(T0, w.pct)]), "{}", w.key);
    }
    // 77 % three and a half days into the week is ahead of pace (§8.7); a Short window has no
    // pace to be ahead of.
    assert_eq!(
        (shown[1].1.ahead, shown[1].1.method),
        (Some(true), Some(ProjectionMethod::Average))
    );
    assert_eq!(shown[0].1.ahead, None);

    // 301 s on, with no failure, no plan in force and no lease, the reading is still shown but
    // no longer decision-grade (§8.4).
    at(&fx, T0 + 301);
    let u = usage_of(&fx.engine, &a);
    assert_eq!(
        (u.status, u.decision_grade, u.age_s),
        (UsageStatus::Ok, false, Some(301))
    );
    assert!(u.windows.is_some());
}

#[test]
fn trust_extends_while_a_plan_is_in_force_or_a_fetch_is_in_flight() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    record(&fx, &a, &reading(T0, 9.0, 40.0), T0, T0 + 1_200);
    record(&fx, &b, &reading(T0, 9.0, 40.0), T0, T0 + 180);
    at(&fx, T0 + 1_000);
    assert!(usage_of(&fx.engine, &a).decision_grade, "a plan in force");
    assert!(!usage_of(&fx.engine, &b).decision_grade);
    // Another process holds b's lease: its fetch is in flight.
    reserve(&fx, &b, T0 + 1_100);
    at(&fx, T0 + 1_150);
    assert!(usage_of(&fx.engine, &b).decision_grade, "a live lease");
    at(&fx, T0 + 1_201);
    assert!(!usage_of(&fx.engine, &a).decision_grade);
    assert!(!usage_of(&fx.engine, &b).decision_grade);
}

#[test]
fn three_samples_over_two_hours_project_by_regression() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    for (i, seven) in [60.0, 70.0, 77.0].into_iter().enumerate() {
        let t = T0 + 3_600 * i as i64;
        record(&fx, &a, &reading(t, 9.0, seven), t, t + 180);
    }
    let last = T0 + 7_200;
    at(&fx, last);
    let windows = usage_of(&fx.engine, &a).windows.unwrap();
    let (w, p) = &windows[1];
    assert_eq!(
        *p,
        expected(w, last, &[(T0, 60.0), (T0 + 3_600, 70.0), (last, 77.0)])
    );
    assert_eq!(p.method, Some(ProjectionMethod::Regression));
}

#[test]
fn each_status_reaches_the_view() {
    let fx = Fx::new();
    let key = fx.add_api_key("sk-ant-api03-key");
    let quarantined = fx.add("q@x.co", "rt-q");
    let locked = fx.add("k@x.co", "rt-k");
    let tokenless = fx.add("n@x.co", "rt-n");
    let fresh = fx.add("f@x.co", "rt-f");
    fx.quarantine(&quarantined, "invalid_grant", "sha256:0");
    fail(&fx, &locked, "keychain-unavailable", T0, T0 + 30, None);
    fail(&fx, &tokenless, "no-access-token", T0, T0 + 30, None);
    let status = |id: &AccountId| usage_of(&fx.engine, id).status;
    assert_eq!(status(&key), UsageStatus::ApiKey);
    assert_eq!(status(&quarantined), UsageStatus::ReloginRequired);
    assert_eq!(status(&locked), UsageStatus::KeychainUnavailable);
    assert_eq!(status(&tokenless), UsageStatus::NoCredentials);
    // Never read and never failed: unavailable, for want of data (§13.2's `no-data`).
    let u = usage_of(&fx.engine, &fresh);
    assert_eq!(
        (u.status, u.error.as_deref(), u.retry_at, u.windows),
        (UsageStatus::Unavailable, Some(NO_DATA), None, None)
    );
}

#[test]
fn after_a_429_the_reading_stays_trusted_until_the_earliest_relevant_reset() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let mut windows = reading(T0, 9.0, 40.0);
    windows[0].resets_at = Some(T0 + 6_000); // 5h: the earliest relevant reset
    windows[2].resets_at = Some(T0 + 200); // Fable: not relevant under the default models
    windows[3].resets_at = Some(T0 + 100); // spend: never relevant
    record(&fx, &a, &windows, T0, T0 + 180);
    let t1 = T0 + 400;
    fail(&fx, &a, "http-429", t1, t1 + 330, Some(t1 + 330));

    at(&fx, t1);
    let u = usage_of(&fx.engine, &a);
    assert_eq!(
        (u.status, u.error.as_deref(), u.retry_at),
        (UsageStatus::Unavailable, Some("http-429"), Some(t1 + 330))
    );
    assert_eq!((u.fetched_at, u.age_s), (Some(T0), Some(400)));
    assert!(
        u.windows.is_some(),
        "a failure never touches the last good reading"
    );
    // Past the hour of extended trust, the 429 rule alone keeps the reading (§8.4) …
    at(&fx, T0 + 5_000);
    assert!(usage_of(&fx.engine, &a).decision_grade);
    // … until the earliest relevant reset: not Fable's, not spend's.
    at(&fx, T0 + 6_001);
    assert!(!usage_of(&fx.engine, &a).decision_grade);
}

#[test]
fn status_and_account_view_carry_the_listed_usage() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    record(&fx, &a, &reading(T0, 9.0, 77.0), T0, T0 + 180);
    let listed = usage_of(&fx.engine, &a);
    let StatusView::Managed { account, .. } = fx.engine.status(&fx.provider()).unwrap() else {
        panic!("a is live");
    };
    assert_eq!(account.usage, listed);
    let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
    assert_eq!(fx.engine.account_view(row, true).usage, listed);
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --lib views && cargo test -p tagteam-engine --test views_usage`
Expected: FAIL to compile. `usage_status`, `UsageView`, `UsageStatus` and `NO_DATA` do not exist
in `views`, and `AccountView` has no field `usage`.

- [ ] **Step 4: Derive the usage view**

Replace everything in `crates/tagteam-engine/src/views.rs` above the `#[cfg(test)]` module with:
```rust
use tagteam_core::pace::pace;
use tagteam_core::trust::decision_grade;
use tagteam_core::usage::earliest_relevant_reset;
use tagteam_core::{AccountId, Pace, ProviderId, Sample, TrustInputs, Window};
use tagteam_provider::{KindTraits, Read};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::{AccountRow, Store, UsageStateRow};

/// §8.7: pace and projections read the samples of the 48 h before a reading.
const PACE_LOOKBACK_S: i64 = 48 * 3600;

/// `usageError` for an account that has neither a reading nor a failure yet (§13.2).
pub const NO_DATA: &str = "no-data";

/// §13.2's `usageStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageStatus {
    Ok,
    TokenExpired,
    ApiKey,
    KeychainUnavailable,
    ReloginRequired,
    ForeignCredential,
    NoCredentials,
    Unavailable,
    Unsupported,
}

impl UsageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            UsageStatus::Ok => "ok",
            UsageStatus::TokenExpired => "token_expired",
            UsageStatus::ApiKey => "api_key",
            UsageStatus::KeychainUnavailable => "keychain_unavailable",
            UsageStatus::ReloginRequired => "relogin_required",
            UsageStatus::ForeignCredential => "foreign_credential",
            UsageStatus::NoCredentials => "no_credentials",
            UsageStatus::Unavailable => "unavailable",
            UsageStatus::Unsupported => "unsupported",
        }
    }
}

/// An account's usage as the views show it (§8.4, §13.2).
#[derive(Debug, Clone, PartialEq)]
pub struct UsageView {
    pub status: UsageStatus,
    /// The last good reading, each window with its pace (§8.7).
    pub windows: Option<Vec<(Window, Pace)>>,
    /// Whether the reading may drive a decision and be shown as `usage` (§8.4).
    pub decision_grade: bool,
    pub fetched_at: Option<i64>,
    pub age_s: Option<i64>,
    /// `last_error` (or `no-data`) when the status is `Unavailable`.
    pub error: Option<String>,
    /// When the next fetch may happen, `max(backoff_until, next_poll_at)`, when `Unavailable`.
    pub retry_at: Option<i64>,
}

impl UsageView {
    /// No reading to show: `status`, and for `Unavailable` why (`no-data` unless `error`).
    fn unread(status: UsageStatus, error: Option<String>) -> Self {
        let unavailable = status == UsageStatus::Unavailable;
        UsageView {
            status,
            windows: None,
            decision_grade: false,
            fetched_at: None,
            age_s: None,
            error: unavailable.then(|| error.unwrap_or_else(|| NO_DATA.to_owned())),
            retry_at: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AccountView {
    pub row: AccountRow,
    /// The live identity wins over the store's active account.
    pub active: bool,
    /// The row's credential kind, as its provider describes it (§4.5).
    pub kind: KindTraits,
    /// Its usage, from the store alone (§13.2).
    pub usage: UsageView,
}

/// The kind traits of a row whose provider this build does not register: nothing special.
const UNREGISTERED: KindTraits = KindTraits {
    refreshable: false,
    managed_key_axis: false,
    default_email_prefix: None,
    display: None,
};

#[derive(Debug, Clone)]
pub struct ProviderAccounts {
    pub provider: ProviderId,
    pub active_position: Option<u32>,
    pub accounts: Vec<AccountView>,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum StatusView {
    NoLogin,
    Unmanaged { email: String },
    Managed { account: AccountView, total: usize },
}

/// Decision 10's table; the first row that matches wins. `state` is the account's
/// `usage_state` row, if any. Its `last_error` counts only while failures are being retried:
/// a success resets them. A reading is a `fetched_at`: Task 8 stores a reading with no windows
/// as a null `last_good`.
fn usage_status(
    supported: bool,
    kind: &KindTraits,
    row: &AccountRow,
    state: Option<&UsageStateRow>,
) -> UsageStatus {
    if !supported {
        return UsageStatus::Unsupported;
    }
    if kind.managed_key_axis {
        return UsageStatus::ApiKey;
    }
    if row.quarantine_reason.is_some() {
        return UsageStatus::ReloginRequired;
    }
    let failing = state
        .filter(|s| s.consecutive_failures > 0)
        .and_then(|s| s.last_error.as_deref());
    match failing {
        Some("foreign-credential") => UsageStatus::ForeignCredential,
        Some("keychain-unavailable") => UsageStatus::KeychainUnavailable,
        Some("no-access-token" | "vault-absent") => UsageStatus::NoCredentials,
        Some("token-expired") => UsageStatus::TokenExpired,
        Some(_) => UsageStatus::Unavailable,
        None if state.is_some_and(|s| s.fetched_at.is_some()) => UsageStatus::Ok,
        None => UsageStatus::Unavailable,
    }
}

/// The reading's windows, each with §8.7's pace from its own samples of the 48 h before the
/// reading: one query per window, along the samples' key (account, window, time).
fn with_pace(
    store: &Store,
    id: &AccountId,
    windows: &[Window],
    fetched_at: i64,
) -> Result<Vec<(Window, Pace)>, EngineError> {
    windows
        .iter()
        .map(|w| {
            let samples: Vec<Sample> = store
                .usage_samples(id, Some(&w.key), fetched_at - PACE_LOOKBACK_S)?
                .into_iter()
                .map(|(_, s)| s)
                .filter(|s| s.fetched_at <= fetched_at)
                .collect();
            Ok((w.clone(), pace(w, fetched_at, &samples)))
        })
        .collect()
}

impl Engine {
    /// §13.2's usage for one account: its status (Decision 10), its last good reading with
    /// pace, and whether that reading is decision-grade (§8.4). Reads the store only.
    fn usage_view(
        &self,
        store: Option<&Store>,
        row: &AccountRow,
        kind: &KindTraits,
        supported: bool,
    ) -> Result<UsageView, EngineError> {
        let state = match store {
            Some(s) if supported => s.usage_state(&row.id)?,
            _ => None,
        };
        let status = usage_status(supported, kind, row, state.as_ref());
        let (Some(store), Some(state)) = (store, state) else {
            return Ok(UsageView::unread(status, None));
        };
        let now_ms = self.now_ms();
        let now_s = now_ms.div_euclid(1000);
        let windows = match state.fetched_at {
            Some(at) => {
                let read = state.last_good.as_deref().unwrap_or_default();
                Some(with_pace(store, &row.id, read, at)?)
            }
            None => None,
        };
        let reset = state
            .last_good
            .as_deref()
            .and_then(|w| earliest_relevant_reset(w, &self.settings().models));
        let trusted = windows.is_some()
            && decision_grade(&TrustInputs {
                now_s,
                fetched_at: state.fetched_at,
                consecutive_failures: state.consecutive_failures,
                plan_in_force: state.next_poll_at.is_some_and(|at| at > now_s),
                live_lease: store.usage_lease_live(&row.id, now_ms)?,
                last_429_at: state.last_429_at,
                earliest_relevant_reset: reset,
            });
        let unavailable = status == UsageStatus::Unavailable;
        let error = unavailable.then(|| {
            state
                .last_error
                .clone()
                .filter(|_| state.consecutive_failures > 0)
                .unwrap_or_else(|| NO_DATA.to_owned())
        });
        Ok(UsageView {
            status,
            windows,
            decision_grade: trusted,
            fetched_at: state.fetched_at,
            age_s: state.fetched_at.map(|at| (now_s - at).max(0)),
            error,
            retry_at: if unavailable {
                state.backoff_until.max(state.next_poll_at)
            } else {
                None
            },
        })
    }

    fn view(&self, provider: &ProviderId) -> Result<(ProviderAccounts, Read<String>), EngineError> {
        let p = self.provider(provider)?;
        let store = self.existing_store()?;
        let rows = match &store {
            Some(s) => s.accounts(provider)?,
            None => vec![],
        };
        let live = p.live_identity(&self.env);
        let live_key = live.as_ref().map(|i| p.identity_key(i).as_str().to_owned());
        let stored_active = match (&live_key, &store) {
            (Read::Unreadable(_), Some(s)) => s.active(provider)?,
            _ => None,
        };
        let supported = p.capabilities().usage;
        let accounts = rows
            .into_iter()
            .map(|row| {
                let active = match &live_key {
                    Read::Present(k) => &row.identity_key == k,
                    Read::Absent => false,
                    Read::Unreadable(_) => stored_active.as_ref() == Some(&row.id),
                };
                let kind = p.kind_traits(&row.kind);
                let usage = self.usage_view(store.as_deref(), &row, &kind, supported)?;
                Ok(AccountView {
                    kind,
                    row,
                    active,
                    usage,
                })
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        let active_position = accounts.iter().find(|v| v.active).map(|v| v.row.position);
        let live_label = live.map(|i| i.email.unwrap_or(i.label));
        Ok((
            ProviderAccounts {
                provider: provider.clone(),
                active_position,
                accounts,
            },
            live_label,
        ))
    }

    /// A row as the views show it, for a caller that already knows whether it is active. Its
    /// usage comes from the store; a store that cannot be read leaves it unread, logged, since
    /// the callers report a change that has already happened.
    pub fn account_view(&self, row: AccountRow, active: bool) -> AccountView {
        let provider = self.registry.get(&row.provider);
        let kind = provider
            .as_ref()
            .map_or(UNREGISTERED, |p| p.kind_traits(&row.kind));
        let supported = provider.is_some_and(|p| p.capabilities().usage);
        let usage = self
            .existing_store()
            .and_then(|s| self.usage_view(s.as_deref(), &row, &kind, supported))
            .unwrap_or_else(|e| {
                tracing::warn!(
                    position = row.position,
                    id = %row.id,
                    kind = e.kind(),
                    "could not read the account's usage"
                );
                UsageView::unread(
                    usage_status(supported, &kind, &row, None),
                    Some(e.kind().to_owned()),
                )
            });
        AccountView {
            row,
            active,
            kind,
            usage,
        }
    }

    /// Every provider that has accounts, plus the default provider; or just `provider`.
    pub fn accounts(
        &self,
        provider: Option<&ProviderId>,
    ) -> Result<Vec<ProviderAccounts>, EngineError> {
        let ids: Vec<ProviderId> = match provider {
            Some(p) => vec![p.clone()],
            None => {
                let mut ids = vec![self.default_provider.clone()];
                if let Some(s) = self.existing_store()? {
                    for row in s.all_accounts()? {
                        if !ids.contains(&row.provider)
                            && self.registry.get(&row.provider).is_some()
                        {
                            ids.push(row.provider);
                        }
                    }
                }
                ids
            }
        };
        ids.iter().map(|id| self.view(id).map(|(v, _)| v)).collect()
    }

    pub fn status(&self, provider: &ProviderId) -> Result<StatusView, EngineError> {
        let (list, live) = self.view(provider)?;
        let total = list.accounts.len();
        if let Some(active) = list.accounts.into_iter().find(|v| v.active) {
            return Ok(StatusView::Managed {
                account: active,
                total,
            });
        }
        match live {
            Read::Present(email) => Ok(StatusView::Unmanaged { email }),
            Read::Absent => Ok(StatusView::NoLogin),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
        }
    }
}
```

- [ ] **Step 5: Give the CLI's render test helper its usage**

`AccountView` has a new field, so the one literal outside the engine, `a_view` in the test
module of `crates/tagteam/src/render.rs`, needs it too. Task 14 rewrites that module; until then
the row keeps M1's status. Add `use tagteam_engine::views::{UsageStatus, UsageView};` to that test
module's imports, and in `a_view`, directly after its `kind: KindTraits { … },` field, add:
```rust
            usage: UsageView {
                status: if quarantined {
                    UsageStatus::ReloginRequired
                } else {
                    UsageStatus::Unavailable
                },
                windows: None,
                decision_grade: false,
                fetched_at: None,
                age_s: None,
                error: (!quarantined).then(|| "no-data".into()),
                retry_at: None,
            },
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --lib views && cargo test -p tagteam-engine --test views_usage && cargo test -p tagteam --lib render`
Expected: PASS: the 3 unit tests, the 6 engine tests, and the render tests.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. Every earlier test that lists accounts now also reads `usage_state`; an account
never read is `unavailable` with `no-data`, as M1 rendered it, and the CLI does not collect yet.

- [ ] **Step 7: Lint and format**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings && cargo clippy --workspace --all-targets -- -D warnings`
Expected: no output from `fmt`; both clippy runs finish without warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/tagteam-engine/src/views.rs crates/tagteam-engine/tests/views_usage.rs \
        crates/tagteam/src/render.rs
git commit -m "Derive each account's usage status, reading, pace and trust in the views"
```

- [ ] **Step 9: Write the failing history and statusline tests**

In `crates/tagteam-engine/tests/views_usage.rs`, replace everything from `mod common;` through
the last `use` line with:
```rust
mod common;

use std::fs;

use common::Fx;
use serde_json::json;
use tagteam_core::pace::pace;
use tagteam_core::{
    AccountId, CLAUDE_CODE, Pace, PollBudget, PollPlan, ProjectionMethod, ProviderId, Sample,
    Window, WindowKind,
};
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::{LiveIdentityCacheRow, Reservation, Reserve};
use tagteam_engine::vault::{KeychainVault, Vault};
use tagteam_engine::views::{
    HistoryView, NO_DATA, StatusView, StatuslineView, UsageStatus, UsageView,
};
use tagteam_engine::{Engine, EngineConfig};
```

and append to the end of the file:
```rust
/// An engine over `fx`'s Env, Keychain, clock and HTTP port with `settings`, as the CLI builds
/// one after reading `config.toml`.
fn engine_with_settings(fx: &Fx, settings: Settings) -> Engine {
    Engine::new(EngineConfig {
        env: fx.env.clone(),
        registry: ProviderRegistry::new().with(fx.cc.clone()),
        vault: Vault::new(Box::new(KeychainVault::new(fx.kc.clone()))),
        oracle: fx.oracle.clone(),
        clock: fx.clock.clone(),
        http: fx.http.clone(),
        default_provider: ProviderId::new(CLAUDE_CODE),
        settings,
    })
}

/// `a`, live, read at T0, T0 + 1 h and T0 + 2 h (7d at 60, 70 and 77 %); the clock at T0 + 2 h.
fn with_history(fx: &Fx) -> AccountId {
    let a = fx.add("a@x.co", "rt-a");
    for (i, seven) in [60.0, 70.0, 77.0].into_iter().enumerate() {
        let t = T0 + 3_600 * i as i64;
        record(fx, &a, &reading(t, 9.0 + i as f64, seven), t, t + 180);
    }
    at(fx, T0 + 7_200);
    a
}

fn keys(h: &HistoryView) -> Vec<&str> {
    h.windows.iter().map(|w| w.window.key.as_str()).collect()
}

#[test]
fn history_defaults_to_the_relevant_windows_and_bounds_samples_by_since() {
    let fx = Fx::new();
    let a = with_history(&fx);
    let h = fx.engine.history(&a, None, T0 + 1).unwrap();
    assert_eq!(
        (h.account.row.id.clone(), h.account.active),
        (a.clone(), true)
    );
    assert_eq!(keys(&h), ["5h", "7d"]);
    let seven = &h.windows[1];
    let shown: Vec<(i64, f64)> = seven
        .samples
        .iter()
        .map(|s| (s.fetched_at, s.pct))
        .collect();
    assert_eq!(shown, [(T0 + 3_600, 70.0), (T0 + 7_200, 77.0)]);
    // Pace reads the 48 h before the reading, whatever `since` shows: the list's own.
    let listed = usage_of(&fx.engine, &a).windows.unwrap();
    assert_eq!(seven.pace, listed[1].1);
    assert_eq!(seven.pace.method, Some(ProjectionMethod::Regression));
    assert!(
        fx.http.requests().is_empty(),
        "history never fetches (§13.4)"
    );
}

#[test]
fn history_filters_by_key_or_label_ignoring_case() {
    let fx = Fx::new();
    let a = with_history(&fx);
    for name in ["FABLE", "scoped:fable"] {
        let h = fx.engine.history(&a, Some(name), 0).unwrap();
        assert_eq!(keys(&h), ["scoped:Fable"], "{name}");
        assert_eq!(h.windows[0].samples.len(), 3, "{name}");
    }
    assert_eq!(
        keys(&fx.engine.history(&a, Some("Spend"), 0).unwrap()),
        ["spend"]
    );
    assert!(
        fx.engine
            .history(&a, Some("nope"), 0)
            .unwrap()
            .windows
            .is_empty()
    );
}

#[test]
fn history_follows_the_configured_models() {
    let fx = Fx::new();
    let a = with_history(&fx);
    let settings = Settings {
        models: vec!["fable".into()],
        ..Settings::default()
    };
    let engine = engine_with_settings(&fx, settings);
    assert_eq!(
        keys(&engine.history(&a, None, 0).unwrap()),
        ["5h", "7d", "scoped:Fable"]
    );
}

#[test]
fn history_of_an_account_never_read_is_empty_and_of_an_unknown_one_an_error() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    assert!(fx.engine.history(&a, None, 0).unwrap().windows.is_empty());
    let unknown = fx.engine.history(&AccountId::from_string("nope"), None, 0);
    assert_eq!(unknown.unwrap_err().kind(), "no-such-account");
}

fn cache(fx: &Fx) -> Option<LiveIdentityCacheRow> {
    fx.engine
        .store()
        .unwrap()
        .live_identity_cache(&fx.provider())
        .unwrap()
}

fn managed(v: StatuslineView) -> AccountId {
    match v {
        StatuslineView::Managed { account } => account.row.id,
        other => panic!("not managed: {other:?}"),
    }
}

#[test]
fn statusline_shows_the_live_account_with_its_usage_or_an_unmanaged_email() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    record(&fx, &b, &reading(T0, 9.0, 77.0), T0, T0 + 180);
    match fx.engine.statusline(&fx.provider()).unwrap() {
        StatuslineView::Managed { account } => {
            assert_eq!((account.row.id.clone(), account.active), (b.clone(), true));
            assert_eq!(account.usage, usage_of(&fx.engine, &b));
        }
        other => panic!("{other:?}"),
    }
    fx.login("stranger@x.co", "rt-s");
    assert!(matches!(
        fx.engine.statusline(&fx.provider()).unwrap(),
        StatuslineView::Unmanaged { email } if email == "stranger@x.co"
    ));
}

#[test]
fn a_missing_garbled_or_rewritten_claude_json_is_read_as_it_is_now() {
    // Review Focus 5, at the engine level.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let config = fx.paths().global_config;
    assert_eq!(managed(fx.engine.statusline(&fx.provider()).unwrap()), a);
    let key_a = fx
        .engine
        .store()
        .unwrap()
        .account(&a)
        .unwrap()
        .unwrap()
        .identity_key;
    let cached = cache(&fx).unwrap();
    assert_eq!(
        (cached.path.as_str(), cached.identity_key.as_deref()),
        (config.to_str().unwrap(), Some(key_a.as_str()))
    );
    // Rewritten, by a login of another length, so the stamp differs even where mtimes are
    // coarse: parsed again, and the cache follows.
    fx.login("bobby@x.co", "rt-bobby");
    assert!(matches!(
        fx.engine.statusline(&fx.provider()).unwrap(),
        StatuslineView::Unmanaged { email } if email == "bobby@x.co"
    ));
    assert_eq!(cache(&fx).unwrap().label.as_deref(), Some("bobby@x.co"));
    // Garbled: nothing to show, and no error.
    fs::write(&config, "{ \"oauthAccount\": ").unwrap();
    assert!(matches!(
        fx.engine.statusline(&fx.provider()).unwrap(),
        StatuslineView::NoLogin
    ));
    // Missing: nothing to show.
    fs::remove_file(&config).unwrap();
    assert!(matches!(
        fx.engine.statusline(&fx.provider()).unwrap(),
        StatuslineView::NoLogin
    ));
    // Written again: read again.
    fs::write(&config, common::CLAUDE_JSON).unwrap();
    fx.login("a@x.co", "rt-a");
    assert_eq!(managed(fx.engine.statusline(&fx.provider()).unwrap()), a);
}

#[test]
fn an_unchanged_claude_json_is_not_parsed_again() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    assert_eq!(managed(fx.engine.statusline(&fx.provider()).unwrap()), b);
    // A cache row under the file's current stamp that names a instead: only a lookup that
    // skipped the parse can answer a.
    let store = fx.engine.store().unwrap();
    let key_a = store.account(&a).unwrap().unwrap().identity_key;
    let planted = LiveIdentityCacheRow {
        identity_key: Some(key_a),
        label: Some("a@x.co".into()),
        ..cache(&fx).unwrap()
    };
    store.put_live_identity_cache(&planted).unwrap();
    assert_eq!(managed(fx.engine.statusline(&fx.provider()).unwrap()), a);
}

#[test]
fn statusline_needs_no_keychain_no_network_and_creates_no_store() {
    let fx = Fx::new();
    fx.login("a@x.co", "rt-a");
    fx.kc.set_locked(true);
    assert!(matches!(
        fx.engine.statusline(&fx.provider()).unwrap(),
        StatuslineView::Unmanaged { email } if email == "a@x.co"
    ));
    assert!(
        !fx.env.data_dir().join("tagteam.db").exists(),
        "§13.5: never creates the store"
    );

    fx.kc.set_locked(false);
    let a = fx.engine.add_live(fx.add_options()).unwrap().account.id;
    record(&fx, &a, &reading(T0, 9.0, 77.0), T0, T0 + 180);
    fx.http.clear();
    fx.kc.set_locked(true);
    match fx.engine.statusline(&fx.provider()).unwrap() {
        StatuslineView::Managed { account } => {
            assert_eq!(account.usage.status, UsageStatus::Ok);
            assert!(account.usage.windows.is_some());
        }
        other => panic!("{other:?}"),
    }
    assert!(fx.http.requests().is_empty(), "no request");
    assert_eq!(fx.kc.unlock_attempts(), 0);
}
```

In `crates/tagteam-engine/tests/invariant.rs`, in `run_every_command_on`, directly after the
`check(fx, "status", …)` call, add (`statusline` writes only tagteam's store, never the identity
surface):
```rust
    check(fx, "statusline", || {
        drop(fx.engine.statusline(&fx.provider()).unwrap())
    });
```

- [ ] **Step 10: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --test views_usage --test invariant`
Expected: FAIL to compile: `HistoryView` and `StatuslineView` do not exist, and `Engine` has no
method `history` or `statusline`.

- [ ] **Step 11: Add the history and statusline views**

In `crates/tagteam-engine/src/views.rs`, replace the `use` lines at the top with:
```rust
use std::os::unix::fs::MetadataExt;

use tagteam_core::pace::pace;
use tagteam_core::trust::decision_grade;
use tagteam_core::usage::{earliest_relevant_reset, is_relevant};
use tagteam_core::{AccountId, Pace, ProviderId, Sample, TrustInputs, Window};
use tagteam_provider::{Identity, KindTraits, Provider, Read};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::{AccountRow, LiveIdentityCacheRow, Store, UsageStateRow};
```

Insert directly above `impl Engine {`:
```rust
/// One window of `tagteam history` (§13.4): its definition from the last good reading, its
/// samples since the requested time, and §8.7's pace as of that reading.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryWindow {
    pub window: Window,
    pub samples: Vec<Sample>,
    pub pace: Pace,
}

#[derive(Debug, Clone)]
pub struct HistoryView {
    pub account: AccountView,
    pub windows: Vec<HistoryWindow>,
}

/// What `statusline` shows (§13.5).
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum StatuslineView {
    NoLogin,
    Unmanaged { email: String },
    Managed { account: AccountView },
}

/// The live login as `statusline` needs it: its identity key and its label (the email when it
/// has one).
struct LiveLogin {
    key: String,
    label: String,
    account_uuid: Option<String>,
}

impl LiveLogin {
    fn of(p: &dyn Provider, i: &Identity) -> Self {
        LiveLogin {
            key: p.identity_key(i).as_str().to_owned(),
            label: i.email.clone().unwrap_or_else(|| i.label.clone()),
            account_uuid: i.account_uuid.clone(),
        }
    }
}
```

And add at the end of `impl Engine`, after `status`:
```rust
    /// §13.4, reading only: the windows of `account`'s last good reading (its relevant ones,
    /// §8.2, or those whose key or label is `window`, ignoring case), each with its samples
    /// fetched at or after `since_s` and its pace as of the reading. An account never read has
    /// no windows.
    pub fn history(
        &self,
        account: &AccountId,
        window: Option<&str>,
        since_s: i64,
    ) -> Result<HistoryView, EngineError> {
        let missing = || EngineError::NoSuchAccount(account.to_string());
        let store = self.existing_store()?.ok_or_else(missing)?;
        let row = store.account(account)?.ok_or_else(missing)?;
        let view = self
            .accounts(Some(&row.provider))?
            .into_iter()
            .flat_map(|l| l.accounts)
            .find(|v| v.row.id == row.id)
            .ok_or_else(missing)?;
        let state = store.usage_state(&row.id)?;
        let reading = state.and_then(|s| Some((s.last_good.unwrap_or_default(), s.fetched_at?)));
        let Some((last_good, fetched_at)) = reading else {
            return Ok(HistoryView {
                account: view,
                windows: Vec::new(),
            });
        };
        let models = &self.settings().models;
        let lookback = fetched_at - PACE_LOOKBACK_S;
        let windows = last_good
            .into_iter()
            .filter(|w| match window {
                Some(name) => {
                    w.key.eq_ignore_ascii_case(name) || w.label.eq_ignore_ascii_case(name)
                }
                None => is_relevant(w, models),
            })
            .map(|w| {
                let all: Vec<Sample> = store
                    .usage_samples(&row.id, Some(&w.key), since_s.min(lookback))?
                    .into_iter()
                    .map(|(_, s)| s)
                    .collect();
                let recent: Vec<Sample> = all
                    .iter()
                    .copied()
                    .filter(|s| (lookback..=fetched_at).contains(&s.fetched_at))
                    .collect();
                let pace = pace(&w, fetched_at, &recent);
                let samples = all
                    .into_iter()
                    .filter(|s| s.fetched_at >= since_s)
                    .collect();
                Ok(HistoryWindow {
                    window: w,
                    samples,
                    pace,
                })
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        Ok(HistoryView {
            account: view,
            windows,
        })
    }

    /// §13.5: the live login and, when tagteam manages it, its usage. No network, no Keychain,
    /// and the store is never created. The live identity comes from `live_identity_cache`
    /// while `Provider::live_identity_source`'s mtime and size are unchanged, and is re-parsed
    /// only when they change. A missing, unreadable or garbled source is `NoLogin`, never an
    /// error.
    pub fn statusline(&self, provider: &ProviderId) -> Result<StatuslineView, EngineError> {
        let p = self.provider(provider)?;
        let store = self.existing_store()?;
        let Some(login) = self.live_login(p.as_ref(), store.as_deref())? else {
            return Ok(StatuslineView::NoLogin);
        };
        let row = match &store {
            Some(s) => s.find_by_identity_key(provider, &login.key)?,
            None => None,
        };
        Ok(match row {
            Some(row) => StatuslineView::Managed {
                account: self.account_view(row, true),
            },
            None => StatuslineView::Unmanaged { email: login.label },
        })
    }

    /// The live login, through `live_identity_cache` (§13.5). Without a store nothing is
    /// cached and the source is parsed every time.
    fn live_login(
        &self,
        p: &dyn Provider,
        store: Option<&Store>,
    ) -> Result<Option<LiveLogin>, EngineError> {
        let parse = || match p.live_identity(&self.env) {
            Read::Present(i) => Some(Some(LiveLogin::of(p, &i))),
            Read::Absent => Some(None),
            Read::Unreadable(_) => None,
        };
        let (Some(path), Some(store)) = (p.live_identity_source(&self.env), store) else {
            return Ok(parse().flatten());
        };
        // Stat before parsing: a rewrite in between leaves the old stamp on the new identity,
        // which the next run's stat sees and parses again; never a new stamp on an old one.
        let Ok(meta) = std::fs::metadata(&path) else {
            return Ok(None);
        };
        let stamp = LiveIdentityCacheRow {
            provider: p.id(),
            path: path.to_string_lossy().into_owned(),
            mtime_ns: meta
                .mtime()
                .saturating_mul(1_000_000_000)
                .saturating_add(meta.mtime_nsec()),
            size: i64::try_from(meta.len()).unwrap_or(i64::MAX),
            identity_key: None,
            label: None,
            account_uuid: None,
        };
        if let Some(c) = store.live_identity_cache(&stamp.provider)? {
            if c.path == stamp.path && c.mtime_ns == stamp.mtime_ns && c.size == stamp.size {
                return Ok(c.identity_key.map(|key| LiveLogin {
                    key,
                    label: c.label.unwrap_or_default(),
                    account_uuid: c.account_uuid,
                }));
            }
        }
        // An unreadable or garbled file is not cached: the next run parses it again.
        let Some(login) = parse() else {
            return Ok(None);
        };
        let row = LiveIdentityCacheRow {
            identity_key: login.as_ref().map(|l| l.key.clone()),
            label: login.as_ref().map(|l| l.label.clone()),
            account_uuid: login.as_ref().and_then(|l| l.account_uuid.clone()),
            ..stamp
        };
        // Only a cache: a failed write costs the next run a parse, never this one its line.
        if let Err(e) = store.put_live_identity_cache(&row) {
            tracing::debug!(error = %e, "the live identity cache was not written");
        }
        Ok(login)
    }
```

- [ ] **Step 12: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --test views_usage --test invariant && cargo test -p tagteam-engine --lib views`
Expected: PASS: the 14 `views_usage` tests, the invariant walks with their new `statusline` step,
and the 3 unit tests.

- [ ] **Step 13: Lint and format**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings && cargo clippy --workspace --all-targets -- -D warnings`
Expected: no output from `fmt`; both clippy runs finish without warnings.

- [ ] **Step 14: Commit**

```bash
git add crates/tagteam-engine/src/views.rs crates/tagteam-engine/tests/views_usage.rs \
        crates/tagteam-engine/tests/invariant.rs
git commit -m "Add the history and statusline views over the cached live identity"
```

---

### Task 14: `list` and `status` with usage

`list` and `status` collect usage on demand and show it. `list` offers every account it lists
to the collector (`CollectMode::OnDemand`), which fetches only those that are due (§8.3);
`status` offers only the live account. The command waits for the collector, then reads the views
again. JSON rows follow §13.2's presence rules, the text follows §13.1's table, and `status`
gains a usage line.

Rulings, flagged rather than decided silently:
- **Times in a row are ISO 8601 UTC** (`usageFetchedAt`, `lastGoodFetchedAt`, `usageRetryAt`),
  through Task 6's `tagteam_cc::usage::format_iso8601`, like the provider's own `resetsAt`. Ages
  are seconds. M1's `loginExpiresAt` stays epoch milliseconds, as M1 pinned it.
- **Table columns.** One per window key that some reading has, in kind order (Short, Long,
  Spend, Scoped), headed by its label in capitals; for Claude Code, `5H 7D SPEND <MODEL>…`.
  `SPEND` is always a column once any account has a reading, showing `—` throughout when no
  account has spend, because §13.1 lets only scoped columns come and go. `AGE` closes the row.
  While no account has a reading, there are no window columns at all.
- **A row without a reading** shows its status in words in place of the columns. A row with a
  reading whose status is not `ok` keeps its windows and age and adds the words after them: a
  stale reading is still shown (§13.1). The words are `relogin required`, `api key`,
  `keychain unavailable`, `no credentials`, `token expired`, `foreign credential`,
  `usage unsupported`, `no data yet`, `over budget (retry 4m)` and
  `unavailable (http-429, retry 5m)`. A retry time that has already passed is left out. The
  kind's label (`setup token`) and `disabled` follow as notes, and an organization name joins the
  account as `[Org]`.
- **Countdowns and ages** read `3d09h`, `2h40m`, `45m` or `<1m`, rounding down. A reset that
  has already passed reads `reset`.
- **Colour** marks only the percentage, red at ≥ 90 and yellow at ≥ 70. `--no-color` and
  `NO_COLOR` turn it off. Otherwise `FORCE_COLOR` turns it on. Otherwise `ui.color` decides,
  where `auto` means "stdout is a terminal". Both variables count when set at all, as the
  existing `NO_COLOR` check does and as Task 16's `statusline` reads them.
- **`status`'s text** gains a second line with the reading's windows and age, or the status in
  words. A quarantined account with no reading keeps M2a's single line.
- **`list` and `status` run no lock check.** They now read Keychain items to collect usage, but
  a locked keychain shows as each row's `keychain_unavailable` (§13.2), never as a prompt or an
  error. `Command::touches_keychain` is unchanged.
- **With nothing to list, nothing is collected**, so `list` on a fresh machine still creates no
  store (§5). If the collector fails as a whole, that is one `warning:` line on stderr, never a
  non-zero exit (§8.3).
- The in-process harness always passes `--no-color`, because the test process's own stdout may
  be a terminal.

This changes existing expectations in `crates/tagteam/tests/`:
- Every in-process `list` now collects with every endpoint offline (`http://127.0.0.1:9`). An
  OAuth or setup-token row therefore reads `unavailable (pre-send, retry <1m)`: the request failed
  before it was sent, and is retried after §8.5's 30 s.
- After a `switch`, Task 12 re-plans only an account that has a reading. Every account here has
  failed its fetches and has none, so the switch plans nothing and the next `list` or `status`
  treats each as it did before: one still inside §8.5's 30 s backoff shows its recorded failure
  with `retry <1m`, and one with no row at all is fetched, and fails the same way.
- `status_and_switch_json`'s last `status --json` follows Task 12 too. The incoming account has
  no reading, so the switch left it without a plan, and `status` fetches it on demand. Offline,
  that fails before it is sent, so the row is `a_row_offline`: `pre-send`, retried later.

**Files:**
- Modify: `crates/tagteam/src/render.rs` (rewritten)
- Modify: `crates/tagteam/src/app.rs`
- Modify: `crates/tagteam/tests/app.rs`, `crates/tagteam/tests/cli.rs`
- Create: `crates/tagteam/tests/usage_cli.rs`

**Interfaces:**
- Consumes:
  - Task 13: `tagteam_engine::views::{AccountView, ProviderAccounts, StatusView, UsageStatus,
    UsageView, NO_DATA}` and `AccountView.usage`.
  - Tasks 10 and 11: `Engine::collect_usage`, `tagteam_engine::collect::CollectMode::OnDemand
    { accounts }` and `CollectReport.warnings`; their failure tokens `pre-send`, `http-429` and
    `keychain-unavailable`.
  - Task 9: `Engine::settings()`, loaded from `config.toml` by `build_engine`. Task 1:
    `tagteam_engine::settings::ColorMode` and `Settings.color` (`ui.color`).
  - Task 7: `Provider::render_usage`. Task 6: `tagteam_cc::usage::format_iso8601`, Claude Code's
    rendered shape (`fiveHour`, `sevenDay`, `scoped[]`), and its normalization of the recorded
    fixture (`5h`, `7d`, `scoped:Fable`, periods, no spend while `spend.enabled` is false).
  - Tasks 2 and 5: `tagteam_core::{Window, WindowKind, Pace}`.
  - Task 12: the post-switch re-plan, which only touches an account that has a reading, so an
    unread account is fetched by the next `list` or `status`.
  - M2a: `MockServer` and `MockReply` (`hits`, `requests`, and `Raw` for a `Retry-After`
    header), and `common::{cmd, login, seed_home}`.
- Produces, crate-private in `render.rs`:
  - `pub type RenderUsage<'a> = &'a dyn Fn(&ProviderId, &[(Window, Pace)]) -> Value;`
  - `row_json(v, usage)`, `list_json(lists, provider, usage)`, `status_json(s, provider, usage)`
    and `account_json(account, created, usage)`
  - `list_human(lists, display_names, now_s, color)` and `status_human(s, now_s, color)`
  - `pub(crate) fn duration(secs: i64) -> String` (`3d09h`, `2h40m`, `45m`, `<1m`; a time
    already past clamps to `<1m`), reached as `crate::render::duration`. It is the one countdown
    and age formatter: Tasks 15 and 16 call it for `history` and `statusline` instead of
    defining their own.

- [ ] **Step 1: Write the failing render tests**

Replace the `#[cfg(test)] mod tests { … }` block at the end of `crates/tagteam/src/render.rs`
with:
```rust
#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tagteam_core::{AccountId, CLAUDE_CODE};
    use tagteam_engine::switch::SwitchReason;
    use tagteam_provider::KindTraits;

    use super::*;

    fn stored(stored_in: Option<SecretStore>) -> SwitchOutcome {
        SwitchOutcome {
            switched: true,
            from: None,
            to: None,
            strategy: "direct",
            reason: SwitchReason::Switched,
            message: String::new(),
            warnings: vec![],
            stored_in,
            unmanaged_email: None,
        }
    }

    #[test]
    fn only_a_fallback_is_a_notice_and_either_file_is_a_file() {
        let path = PathBuf::from("/home/u/.claude.json");
        let linux = stored(Some(SecretStore::File(path.clone())));
        assert_eq!(
            (credential_store(&linux), fallback_notice(&linux)),
            (Some("file"), None)
        );
        let fell_back = stored(Some(SecretStore::Fallback(path)));
        assert_eq!(credential_store(&fell_back), Some("file"));
        assert_eq!(
            fallback_notice(&fell_back).as_deref(),
            Some(
                "the Keychain refused the write, so the credential was stored in /home/u/.claude.json instead"
            )
        );
        let keychain = stored(Some(SecretStore::Keychain));
        assert_eq!(
            (credential_store(&keychain), fallback_notice(&keychain)),
            (Some("keychain"), None)
        );
        let none = stored(None);
        assert_eq!(
            (credential_store(&none), fallback_notice(&none)),
            (None, None)
        );
    }

    /// Any fixed instant: every reading and reset below is placed relative to it.
    const NOW: i64 = 1_790_000_000;

    const OAUTH: KindTraits = KindTraits {
        refreshable: true,
        managed_key_axis: false,
        default_email_prefix: None,
        display: None,
    };

    const API_KEY: KindTraits = KindTraits {
        refreshable: false,
        managed_key_axis: true,
        default_email_prefix: Some("api-key"),
        display: Some("api key"),
    };

    const SETUP_TOKEN: KindTraits = KindTraits {
        refreshable: false,
        managed_key_axis: false,
        default_email_prefix: Some("setup-token"),
        display: Some("setup token"),
    };

    /// A renderer that only says how many windows it was handed.
    fn count(_: &ProviderId, windows: &[(Window, Pace)]) -> Value {
        json!({"windows": windows.len()})
    }

    fn accounts(provider: &str, active: Option<u32>) -> ProviderAccounts {
        ProviderAccounts {
            provider: ProviderId::new(provider),
            active_position: active,
            accounts: vec![],
        }
    }

    #[test]
    fn active_account_number_is_the_queried_providers_wherever_it_is_listed() {
        let lists = [accounts("other", Some(5)), accounts(CLAUDE_CODE, Some(2))];
        let v = list_json(&lists, &ProviderId::new(CLAUDE_CODE), &count);
        assert_eq!(v["activeAccountNumber"], 2);
        assert_eq!(v["activeByProvider"], json!({"other": 5, CLAUDE_CODE: 2}));
        let v = list_json(&lists, &ProviderId::new("other"), &count);
        assert_eq!(v["activeAccountNumber"], 5);
        let v = list_json(&lists, &ProviderId::new("absent"), &count);
        assert_eq!(v["activeAccountNumber"], Value::Null);
    }

    /// `email`'s inactive row at `position`, of `kind`, with `usage`.
    fn view(position: u32, email: &str, kind: KindTraits, usage: UsageView) -> AccountView {
        AccountView {
            row: AccountRow {
                id: AccountId::from_string(format!("id-{position}")),
                provider: ProviderId::new(CLAUDE_CODE),
                position,
                identity_key: format!("{email}\n"),
                label: email.into(),
                email: Some(email.into()),
                org_uuid: String::new(),
                org_name: None,
                account_uuid: None,
                kind: "oauth".into(),
                alias: None,
                disabled: false,
                identity_json: json!({}),
                login_expires_at: None,
                login_epoch: 0,
                replacing_fp: None,
                quarantine_reason: None,
                quarantine_fp: None,
                quarantine_at: None,
                added_at: 1,
            },
            active: false,
            kind,
            usage,
        }
    }

    /// No reading: `status`, with `error` and a retry `retry_in` seconds from now.
    fn unread(status: UsageStatus, error: Option<&str>, retry_in: Option<i64>) -> UsageView {
        UsageView {
            status,
            windows: None,
            decision_grade: false,
            fetched_at: None,
            age_s: None,
            error: error.map(str::to_owned),
            retry_at: retry_in.map(|s| NOW + s),
        }
    }

    fn window(
        key: &str,
        label: &str,
        kind: WindowKind,
        pct: f64,
        resets_in: Option<i64>,
    ) -> Window {
        Window {
            key: key.into(),
            label: label.into(),
            kind,
            pct,
            resets_at: resets_in.map(|s| NOW + s),
            period_s: None,
            detail: None,
        }
    }

    /// Claude Code's spend window: `used` of `limit` euros.
    fn spend(used: f64, limit: f64) -> Window {
        Window {
            detail: Some(json!({"used": used, "limit": limit, "currency": "EUR"})),
            ..window(
                "spend",
                "spend",
                WindowKind::Spend,
                used / limit * 100.0,
                None,
            )
        }
    }

    fn fable(pct: f64) -> Window {
        window(
            "scoped:Fable",
            "Fable",
            WindowKind::Scoped,
            pct,
            Some(291_630),
        )
    }

    /// A good reading `age_s` old: 5h resetting in 2h40m30s and 7d in 3d09h00m30s (ahead of pace
    /// when `ahead`), then `extra`. Decision-grade while at most 300 s old.
    fn read(age_s: i64, five: f64, seven: f64, ahead: bool, extra: Vec<Window>) -> UsageView {
        let mut windows = vec![
            (
                window("5h", "5h", WindowKind::Short, five, Some(9_630)),
                Pace::default(),
            ),
            (
                window("7d", "7d", WindowKind::Long, seven, Some(291_630)),
                Pace {
                    ahead: Some(ahead),
                    ..Pace::default()
                },
            ),
        ];
        windows.extend(extra.into_iter().map(|w| (w, Pace::default())));
        UsageView {
            status: UsageStatus::Ok,
            windows: Some(windows),
            decision_grade: age_s <= 300,
            fetched_at: Some(NOW - age_s),
            age_s: Some(age_s),
            error: None,
            retry_at: None,
        }
    }

    fn one(accounts: Vec<AccountView>) -> [ProviderAccounts; 1] {
        [ProviderAccounts {
            provider: ProviderId::new(CLAUDE_CODE),
            active_position: None,
            accounts,
        }]
    }

    fn names(id: &str) -> String {
        id.to_owned()
    }

    #[test]
    fn durations_read_as_days_hours_or_minutes() {
        for (secs, text) in [
            (-5, "<1m"),
            (0, "<1m"),
            (59, "<1m"),
            (60, "1m"),
            (3_599, "59m"),
            (3_600, "1h00m"),
            (9_630, "2h40m"),
            (86_399, "23h59m"),
            (86_400, "1d00h"),
            (291_630, "3d09h"),
        ] {
            assert_eq!(duration(secs), text, "{secs}");
        }
        assert_eq!(countdown(NOW + 60, NOW), "1m");
        assert_eq!(countdown(NOW, NOW), "reset");
    }

    #[test]
    fn money_uses_the_symbol_or_the_code() {
        assert_eq!(money(0.0, "EUR"), "€0");
        assert_eq!(money(20.0, "eur"), "€20");
        assert_eq!(money(12.5, "USD"), "$12.50");
        assert_eq!(money(3.0, "GBP"), "£3");
        assert_eq!(money(7.25, "CHF"), "7.25 CHF");
    }

    #[test]
    fn the_list_is_a_table_of_windows_countdowns_pace_and_age() {
        // §13.1's layout: a column per window some reading has (SPEND always), `▲ pace`, the
        // reading's age, and the status in words for a row with no reading.
        let mut live = view(
            1,
            "michael@example.com",
            OAUTH,
            read(120, 9.0, 77.0, true, vec![spend(0.0, 20.0), fable(0.0)]),
        );
        live.active = true;
        let spare = view(
            2,
            "spare@example.com",
            OAUTH,
            read(840, 31.0, 12.0, false, vec![]),
        );
        let mut work = view(
            3,
            "w@corp.com",
            OAUTH,
            unread(UsageStatus::ReloginRequired, None, None),
        );
        work.row.alias = Some("work".into());
        work.row.quarantine_reason = Some("invalid_grant".into());
        let key = view(
            4,
            "api-key-4@token.local",
            API_KEY,
            unread(UsageStatus::ApiKey, None, None),
        );
        assert_eq!(
            list_human(&one(vec![live, spare, work, key]), &names, NOW, false),
            concat!(
                "    #  ACCOUNT                5H           7D                   SPEND      FABLE        AGE\n",
                " *  1  michael@example.com      9%  2h40m   77%  3d09h  ▲ pace  €0 of €20    0%  3d09h  2m\n",
                "    2  spare@example.com       31%  2h40m   12%  3d09h          —             —         14m\n",
                "    3  work (w@corp.com)      relogin required\n",
                "    4  api-key-4@token.local  api key\n",
            )
        );
    }

    #[test]
    fn a_row_says_in_words_what_keeps_its_reading_from_being_current() {
        // A stale reading is still shown, with its age and why it is not refreshed; a window whose
        // reset has passed says so. Rows with no reading say why instead of the columns.
        let mut stale = read(4_000, 50.0, 60.0, false, vec![]);
        stale.status = UsageStatus::Unavailable;
        stale.error = Some("http-429".into());
        stale.retry_at = Some(NOW + 330);
        stale.windows.as_mut().unwrap()[0].0.resets_at = Some(NOW - 10);
        let fresh = unread(UsageStatus::Unavailable, Some(NO_DATA), None);
        let over = unread(UsageStatus::Unavailable, Some("over-budget"), Some(600));
        let mut setup = view(
            4,
            "setup-token-4@token.local",
            SETUP_TOKEN,
            unread(UsageStatus::Unavailable, Some("pre-send"), Some(30)),
        );
        setup.row.disabled = true;
        let rows = vec![
            view(1, "a@x.co", OAUTH, stale),
            view(2, "b@x.co", OAUTH, fresh),
            view(3, "c@x.co", OAUTH, over),
            setup,
        ];
        assert_eq!(
            list_human(&one(rows), &names, NOW, false),
            concat!(
                "    #  ACCOUNT                    5H           7D           SPEND  AGE\n",
                "    1  a@x.co                      50%  reset   60%  3d09h  —      1h06m  unavailable (http-429, retry 5m)\n",
                "    2  b@x.co                     no data yet\n",
                "    3  c@x.co                     over budget (retry 10m)\n",
                "    4  setup-token-4@token.local  unavailable (pre-send, retry <1m)  setup token  disabled\n",
            )
        );
    }

    #[test]
    fn percentages_are_coloured_by_severity_only_when_asked() {
        // §13.5's severities: ≥ 90 red, ≥ 70 yellow, and the columns still line up.
        let rows = || {
            one(vec![view(
                1,
                "a@x.co",
                OAUTH,
                read(0, 95.0, 77.0, false, vec![]),
            )])
        };
        assert_eq!(
            list_human(&rows(), &names, NOW, true),
            concat!(
                "    #  ACCOUNT  5H           7D           SPEND  AGE\n",
                "    1  a@x.co    \x1b[31m95%\x1b[0m  2h40m   \x1b[33m77%\x1b[0m  3d09h  —      <1m\n",
            )
        );
        assert!(!list_human(&rows(), &names, NOW, false).contains('\x1b'));
    }

    #[test]
    fn status_shows_the_reading_or_the_trouble_on_a_second_line() {
        let mut live = view(
            1,
            "a@x.co",
            OAUTH,
            read(120, 9.0, 77.0, true, vec![spend(0.0, 20.0), fable(0.0)]),
        );
        live.active = true;
        assert_eq!(
            status_human(
                &StatusView::Managed {
                    account: live,
                    total: 1
                },
                NOW,
                false
            ),
            "Live: a@x.co (position 1 of 1)\n  5h 9% (2h40m) · 7d 77% (3d09h) ▲ pace · spend €0 of €20 · Fable 0% (3d09h) · 2m old\n"
        );
        let failing = view(
            1,
            "a@x.co",
            OAUTH,
            unread(UsageStatus::Unavailable, Some("pre-send"), Some(30)),
        );
        assert_eq!(
            status_human(
                &StatusView::Managed {
                    account: failing,
                    total: 2
                },
                NOW,
                false
            ),
            "Live: a@x.co (position 1 of 2)\n  unavailable (pre-send, retry <1m)\n"
        );
        assert_eq!(
            status_human(&StatusView::NoLogin, NOW, false),
            "No live login.\n"
        );
        assert_eq!(
            status_human(
                &StatusView::Unmanaged {
                    email: "s@x.co".into()
                },
                NOW,
                false
            ),
            "Live: s@x.co (not managed by tagteam)\n"
        );
    }

    #[test]
    fn a_quarantined_account_is_marked_for_a_new_login() {
        let mut a = view(
            1,
            "a@x.co",
            OAUTH,
            unread(UsageStatus::ReloginRequired, None, None),
        );
        a.active = true;
        a.row.quarantine_reason = Some("invalid_grant".into());
        assert_eq!(
            list_human(&one(vec![a.clone()]), &names, NOW, false),
            "    #  ACCOUNT\n *  1  a@x.co   relogin required\n"
        );
        assert_eq!(
            status_human(
                &StatusView::Managed {
                    account: a,
                    total: 1
                },
                NOW,
                false
            ),
            "Live: a@x.co (position 1 of 1), relogin required\n"
        );
    }

    /// The row's keys, in order.
    fn keys(v: &Value) -> Vec<&str> {
        v.as_object().unwrap().keys().map(String::as_str).collect()
    }

    const ALWAYS: [&str; 11] = [
        "number",
        "position",
        "id",
        "provider",
        "email",
        "organizationName",
        "organizationUuid",
        "isOrganization",
        "active",
        "usageStatus",
        "usage",
    ];

    #[test]
    fn row_json_carries_usage_only_when_decision_grade() {
        // §13.2: `usage` and its fetch time and age when it is decision-grade; otherwise the last
        // good reading's, and why and when for an `unavailable` row.
        let current = row_json(
            &view(1, "a@x.co", OAUTH, read(120, 9.0, 77.0, true, vec![])),
            &count,
        );
        assert_eq!(
            keys(&current),
            [&ALWAYS[..], &["usageFetchedAt", "usageAgeSeconds"]].concat()
        );
        assert_eq!(
            (&current["usageStatus"], &current["usage"]),
            (&json!("ok"), &json!({"windows": 2}))
        );
        assert_eq!(
            (&current["usageFetchedAt"], &current["usageAgeSeconds"]),
            (&json!(format_iso8601(NOW - 120)), &json!(120))
        );

        let stale = row_json(
            &view(1, "a@x.co", OAUTH, read(840, 9.0, 77.0, true, vec![])),
            &count,
        );
        let last_good = ["lastGoodUsage", "lastGoodFetchedAt", "lastGoodAgeSeconds"];
        assert_eq!(keys(&stale), [&ALWAYS[..], &last_good].concat());
        assert_eq!(
            (
                &stale["usage"],
                &stale["lastGoodUsage"],
                &stale["lastGoodAgeSeconds"]
            ),
            (&Value::Null, &json!({"windows": 2}), &json!(840))
        );
        assert_eq!(stale["lastGoodFetchedAt"], json!(format_iso8601(NOW - 840)));

        let failing = row_json(
            &view(
                1,
                "a@x.co",
                OAUTH,
                unread(UsageStatus::Unavailable, Some("http-429"), Some(330)),
            ),
            &count,
        );
        assert_eq!(
            keys(&failing),
            [&ALWAYS[..], &last_good, &["usageError", "usageRetryAt"]].concat()
        );
        assert_eq!(
            (
                &failing["usageStatus"],
                &failing["lastGoodUsage"],
                &failing["lastGoodFetchedAt"]
            ),
            (&json!("unavailable"), &Value::Null, &Value::Null)
        );
        assert_eq!(
            (&failing["usageError"], &failing["usageRetryAt"]),
            (&json!("http-429"), &json!(format_iso8601(NOW + 330)))
        );

        let key = row_json(
            &view(
                2,
                "k@x.co",
                API_KEY,
                unread(UsageStatus::ApiKey, None, None),
            ),
            &count,
        );
        assert_eq!(keys(&key), [&ALWAYS[..], &last_good].concat());
        assert_eq!(key["usageStatus"], "api_key");
    }
}
```

- [ ] **Step 2: Write the binary tests**

Create `crates/tagteam/tests/usage_cli.rs`:
```rust
//! `list` and `status` with usage, through the real binary against a `MockServer` that serves
//! the recorded usage reply (§8.3, §13.1, §13.2). Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use common::{cmd, login, seed_home};
use serde_json::{Value, json};
use tagteam_cc::usage::format_iso8601;
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{Env, FileKeychain};

const API_BASE: &str = "TAGTEAM_TEST_API_BASE";
const USAGE: &str = "/api/oauth/usage";

fn now_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// The recorded usage reply (`tagteam-cc`'s fixture) with its windows' resets moved to fixed
/// distances from `now`: 5h in 2h40m30s, and 7d and Fable in 3d09h00m30s. The 30 s keep each
/// countdown on its minute while the binary runs, and 77 % three and a half days into the week
/// is ahead of pace (§8.7).
fn usage_body(now: i64) -> Value {
    let recorded: Value = serde_json::from_str(include_str!(
        "../../tagteam-cc/tests/fixtures/endpoints/usage-200.json"
    ))
    .unwrap();
    let mut body = recorded["body"].clone();
    body["five_hour"]["resets_at"] = json!(format_iso8601(now + 9_630));
    body["seven_day"]["resets_at"] = json!(format_iso8601(now + 291_630));
    body["limits"][2]["resets_at"] = json!(format_iso8601(now + 291_630));
    body
}

/// A server whose usage endpoint answers `reply`, every time.
fn serving(reply: MockReply) -> MockServer {
    let server = MockServer::start();
    server.on("GET", USAGE, reply);
    server
}

fn recorded_reply(now: i64) -> MockReply {
    MockReply::Json {
        status: 200,
        body: usage_body(now),
    }
}

/// Logs each email in and stores it with `add`, which sends no usage request (only `list` and
/// `status` collect); the last one stays live.
fn accounts(root: &Path, emails: &[&str]) {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    for (i, email) in emails.iter().enumerate() {
        login(&env, &kc, email, "", &format!("rt-{i}"));
        cmd(root).arg("add").assert().success();
    }
}

/// `tagteam <args>` against `server`: it must succeed. Returns stdout and stderr.
fn run(root: &Path, server: &MockServer, args: &[&str]) -> (String, String) {
    let out = cmd(root)
        .env(API_BASE, server.base_url())
        .args(args)
        .assert()
        .success()
        .get_output()
        .clone();
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn json_of(root: &Path, server: &MockServer, args: &[&str]) -> Value {
    serde_json::from_str(&run(root, server, args).0).unwrap()
}

const TABLE: &str = concat!(
    "    #  ACCOUNT  5H           7D                   SPEND  FABLE        AGE\n",
    "    1  a@x.co     9%  2h40m   77%  3d09h  ▲ pace  —        0%  3d09h  <1m\n",
    " *  2  b@x.co     9%  2h40m   77%  3d09h  ▲ pace  —        0%  3d09h  <1m\n",
);

#[test]
fn list_fetches_each_account_and_shows_its_windows() {
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co", "b@x.co"]);
    let server = serving(recorded_reply(now_s()));
    let (out, err) = run(d.path(), &server, &["list"]);
    assert_eq!((out.as_str(), err.as_str()), (TABLE, ""));
    assert_eq!(server.hits("GET", USAGE), 2);
    // §8.1's request: the account's bearer token, the beta header, tagteam's User-Agent.
    for req in server.requests() {
        let header = |name: &str| {
            req.headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(header("authorization").as_deref(), Some("Bearer at"));
        assert_eq!(
            header("anthropic-beta").as_deref(),
            Some("oauth-2025-04-20")
        );
        assert!(header("user-agent").unwrap().starts_with("tagteam/"));
    }
}

#[test]
fn a_second_list_within_the_floor_sends_no_request() {
    // §8.3's on-demand rule: a reading younger than 180 s is served, not fetched again.
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co", "b@x.co"]);
    let server = serving(recorded_reply(now_s()));
    run(d.path(), &server, &["list"]);
    assert_eq!(run(d.path(), &server, &["list"]).0, TABLE);
    let v = json_of(d.path(), &server, &["list", "--json"]);
    assert_eq!(server.hits("GET", USAGE), 2, "one request per account");
    for row in v["accounts"].as_array().unwrap() {
        assert_eq!(row["usageStatus"], "ok");
        assert!(row["usage"].is_object(), "decision-grade: {row}");
    }
}

#[test]
fn list_json_rows_carry_usage_as_section_13_2_says() {
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co"]);
    let server = serving(recorded_reply(now_s()));
    let before = now_s();
    let v = json_of(d.path(), &server, &["list", "--json"]);
    let after = now_s();
    let row = &v["accounts"][0];
    let keys: Vec<&str> = row
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "number",
            "position",
            "id",
            "provider",
            "email",
            "organizationName",
            "organizationUuid",
            "isOrganization",
            "active",
            "usageStatus",
            "usage",
            "usageFetchedAt",
            "usageAgeSeconds",
            "loginExpiresAt",
        ]
    );
    assert_eq!(row["usageStatus"], "ok");
    let usage = &row["usage"];
    assert_eq!(usage["fiveHour"]["pct"].as_f64(), Some(9.0));
    assert_eq!(usage["sevenDay"]["pct"].as_f64(), Some(77.0));
    assert_eq!(usage["sevenDay"]["aheadOfPace"], true);
    assert_eq!(usage["scoped"][0]["name"], "Fable");
    let fetched = row["usageFetchedAt"].as_str().unwrap();
    assert!(
        (format_iso8601(before).as_str()..=format_iso8601(after).as_str()).contains(&fetched),
        "{fetched}"
    );
    assert!(row["usageAgeSeconds"].as_i64().unwrap() <= after - before);

    // `status` carries the same row, managed; the reading is fresh, so nothing is fetched.
    let s = json_of(d.path(), &server, &["status", "--json"]);
    assert_eq!(
        (&s["active"]["managed"], &s["active"]["usage"]),
        (&json!(true), usage)
    );
    assert_eq!(server.hits("GET", USAGE), 1);
}

#[test]
fn a_429_is_a_row_that_says_when_it_is_retried_never_a_command_error() {
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co"]);
    let server = serving(MockReply::Raw {
        status: 429,
        headers: vec![("retry-after".into(), "330".into())],
        body: b"{}".to_vec(),
    });
    let before = now_s();
    let (out, _) = run(d.path(), &server, &["list"]);
    assert_eq!(
        out,
        "    #  ACCOUNT\n *  1  a@x.co   unavailable (http-429, retry 5m)\n"
    );
    // In backoff: the next list sends nothing and reports the same.
    let v = json_of(d.path(), &server, &["list", "--json"]);
    let after = now_s();
    assert_eq!(server.hits("GET", USAGE), 1);
    let row = &v["accounts"][0];
    assert_eq!(
        (&row["usageStatus"], &row["usage"], &row["lastGoodUsage"]),
        (&json!("unavailable"), &Value::Null, &Value::Null)
    );
    assert_eq!(
        (&row["lastGoodFetchedAt"], &row["lastGoodAgeSeconds"]),
        (&Value::Null, &Value::Null)
    );
    assert_eq!(row["usageError"], "http-429");
    let retry = row["usageRetryAt"].as_str().unwrap();
    let (earliest, latest) = (format_iso8601(before + 330), format_iso8601(after + 330));
    assert!(
        (earliest.as_str()..=latest.as_str()).contains(&retry),
        "{retry}"
    );
}

#[test]
fn status_collects_the_live_account_only_and_shows_its_usage() {
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co", "b@x.co"]);
    let server = serving(recorded_reply(now_s()));
    let (out, _) = run(d.path(), &server, &["status"]);
    assert_eq!(
        out,
        "Live: b@x.co (position 2 of 2)\n  5h 9% (2h40m) · 7d 77% (3d09h) ▲ pace · Fable 0% (3d09h) · <1m old\n"
    );
    assert_eq!(server.hits("GET", USAGE), 1, "b only");
    run(d.path(), &server, &["list"]);
    assert_eq!(server.hits("GET", USAGE), 2, "then a, b being fresh");
}

#[test]
fn colour_follows_the_setting_and_the_environment() {
    // §13.1: a 77 % window is a warning (yellow). Off on a pipe by default; `ui.color` and
    // `FORCE_COLOR` turn it on; `NO_COLOR` and `--no-color` win over both.
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co"]);
    let server = serving(recorded_reply(now_s()));
    const YELLOW_77: &str = "\x1b[33m77%\x1b[0m";
    let list = |env: &[(&str, &str)], args: &[&str]| {
        let mut c = cmd(d.path());
        c.env(API_BASE, server.base_url()).arg("list").args(args);
        for (k, v) in env {
            c.env(k, v);
        }
        String::from_utf8(c.assert().success().get_output().stdout.clone()).unwrap()
    };
    assert!(!list(&[], &[]).contains('\x1b'));
    assert!(list(&[("FORCE_COLOR", "1")], &[]).contains(YELLOW_77));
    let config = Env::for_test(d.path()).config_dir();
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("config.toml"), "[ui]\ncolor = \"always\"\n").unwrap();
    assert!(list(&[], &[]).contains(YELLOW_77));
    assert!(!list(&[("NO_COLOR", "1")], &[]).contains('\x1b'));
    assert!(!list(&[("FORCE_COLOR", "1")], &["--no-color"]).contains('\x1b'));
    assert_eq!(server.hits("GET", USAGE), 1);
}
```

- [ ] **Step 3: Update the expectations the new behaviour changes**

In `crates/tagteam/tests/app.rs`:

(a) In `impl H`, replace the start of `fn run` (its signature and the `let cli = …;` statement)
with:
```rust
    /// Runs `tagteam <args>` in-process. `--no-color` is always added: whether this test
    /// process's own stdout is a terminal, or `FORCE_COLOR` is set around it, must not colour
    /// the output the tests compare.
    fn run(&self, args: &[&str], prompter: &mut Scripted) -> (i32, String, String) {
        let argv = std::iter::once("tagteam")
            .chain(args.iter().copied())
            .chain(["--no-color"]);
        let cli = Cli::try_parse_from(argv).unwrap();
```

(b) In `fn json`, replace `normalize_ids(&mut v);` with `normalize(&mut v);`.

(c) Replace the whole `fn normalize_ids` with:
```rust
/// Ids, and the retry time a collection records, as placeholders, so rows compare exactly.
fn normalize(v: &mut Value) {
    match v {
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                match k.as_str() {
                    "id" if x.is_string() => *x = json!("[id]"),
                    "usageRetryAt" if x.is_string() => *x = json!("[time]"),
                    _ => normalize(x),
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(normalize),
        _ => {}
    }
}

/// `a@x.co`'s row once `list` or `status` has tried to collect it with every endpoint offline
/// (§8.3): the request never left, so it is `unavailable` for `pre-send`, retried later.
fn a_row_offline(position: u32, active: bool) -> Value {
    with(
        a_row(position, active),
        json!({"usageError": "pre-send", "usageRetryAt": "[time]"}),
    )
}
```

(d) In `add_list_status_and_switch_read_like_this`, replace:
```rust
    assert_eq!(h.ok(&["list"]), "  1  work (a@x.co)\n* 2  b@x.co\n");
    assert_eq!(h.ok(&["status"]), "Live: b@x.co (position 2 of 2)\n");
```
with:
```rust
    // Every endpoint is offline: each account's fetch fails before sending (§8.3).
    assert_eq!(
        h.ok(&["list"]),
        concat!(
            "    #  ACCOUNT\n",
            "    1  work (a@x.co)  unavailable (pre-send, retry <1m)\n",
            " *  2  b@x.co         unavailable (pre-send, retry <1m)\n",
        )
    );
    assert_eq!(
        h.ok(&["status"]),
        "Live: b@x.co (position 2 of 2)\n  unavailable (pre-send, retry <1m)\n"
    );
```
and replace:
```rust
    assert_eq!(h.ok(&["ls"]), "* 1  work (a@x.co)\n  2  b@x.co\n");
```
with:
```rust
    // The switch re-planned nothing: neither account has a reading, so both stay as the
    // `list` above left them, inside the 30 s backoff of their failed fetch.
    assert_eq!(
        h.ok(&["ls"]),
        concat!(
            "    #  ACCOUNT\n",
            " *  1  work (a@x.co)  unavailable (pre-send, retry <1m)\n",
            "    2  b@x.co         unavailable (pre-send, retry <1m)\n",
        )
    );
```

(e) In `list_json_is_cswap_compatible`, replace `a_row(1, true)` in the `accounts` array with
`a_row_offline(1, true)`.

(f) In `status_and_switch_json`, replace the last assertion with:
```rust
    // a has no reading, so the switch left it without a plan (§8.3) and `status` fetches it on
    // demand: offline, that fails before it is sent.
    assert_eq!(
        h.json(&["status", "--json"]),
        json!({"schemaVersion": 1, "provider": "claude-code",
               "active": with(a_row_offline(1, true), json!({"managed": true})),
               "totalManagedAccounts": 2})
    );
```

(g) In `declining_the_unmanaged_login_offer_cancels_and_adds_nothing`, replace
`assert_eq!(h.ok(&["list"]), "  1  a@x.co\n");` with:
```rust
    assert_eq!(
        h.ok(&["list"]),
        "    #  ACCOUNT\n    1  a@x.co   unavailable (pre-send, retry <1m)\n"
    );
```

(h) In `prompts_never_block_a_non_interactive_caller`, replace
`assert_eq!(h.ok(&["list"]), "* 1  stranger@x.co\n");` with:
```rust
    assert_eq!(
        h.ok(&["list"]),
        "    #  ACCOUNT\n *  1  stranger@x.co  unavailable (pre-send, retry <1m)\n"
    );
```

(i) Directly after `const REPLACE_A: &str = …;`, add:
```rust

/// `list` with `a@x.co` alone and live, its fetch failed offline.
const LIVE_A_OFFLINE: &str = "    #  ACCOUNT\n *  1  a@x.co   unavailable (pre-send, retry <1m)\n";
```

(j) In `add_token_over_an_occupied_position_asks_on_a_terminal_and_keeps_the_token`, replace
`assert_eq!(h.ok(&["list"]), "  1  api-key-1@token.local  api key\n");` with:
```rust
    assert_eq!(
        h.ok(&["list"]),
        "    #  ACCOUNT\n    1  api-key-1@token.local  api key\n"
    );
```

(k) In `declining_add_token_over_an_occupied_position_cancels_and_keeps_the_occupant`,
`add_token_over_an_occupied_position_never_asks_off_a_terminal` and
`a_locked_keychain_is_offered_for_unlocking_on_a_terminal`, replace
`assert_eq!(h.ok(&["list"]), "* 1  a@x.co\n");` with
`assert_eq!(h.ok(&["list"]), LIVE_A_OFFLINE);`.

(l) At the end of `commands_that_touch_no_keychain_item_run_no_check`, after
`assert_eq!(h.kc.unlock_attempts(), 0);`, add:
```rust
    // `list` and `status` do read Keychain items, to collect usage (§8.3), but run no lock
    // check: a locked keychain is the row's `keychain_unavailable`, never a prompt or an error.
    let v = h.json(&["list", "--json"]);
    let a = v["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["email"] == "a@x.co")
        .unwrap();
    assert_eq!(a["usageStatus"], "keychain_unavailable");
```

(m) In `token_accounts_list_their_kind_from_the_provider`, replace the text `list` assertion with:
```rust
    assert_eq!(
        h.ok(&["list"]),
        concat!(
            "    #  ACCOUNT\n",
            "    1  setup-token-1@token.local  unavailable (pre-send, retry <1m)  setup token\n",
            "    2  api-key-2@token.local      api key\n",
        )
    );
```

In `crates/tagteam/tests/cli.rs`, in `list_and_status_mark_quarantined_accounts`, replace the
`list` assertion with:
```rust
    offline(d.path()).arg("list").assert().success().stdout(
        "    #  ACCOUNT\n    1  a@x.co   relogin required\n *  2  b@x.co   relogin required\n",
    );
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p tagteam --lib render`
Expected: FAIL to compile: `list_human`, `row_json` and the others still take their M1
arguments, and `duration`, `countdown`, `money` and `format_iso8601` are not in scope.

Run: `cargo test -p tagteam --features test-support --test usage_cli --test app --test cli`
Expected: FAIL. Every `usage_cli` test fails, because `list` prints M1's lines and sends no usage
request (`hits` is 0). The `app` tests changed in Step 3 fail on the old layout, and so does
`list_and_status_mark_quarantined_accounts`.

- [ ] **Step 5: Rewrite the rendering**

Replace everything in `crates/tagteam/src/render.rs` above the test module with:
```rust
use serde_json::{Value, json};
use tagteam_cc::usage::format_iso8601;
use tagteam_core::{Pace, ProviderId, Window, WindowKind};
use tagteam_engine::store::AccountRow;
use tagteam_engine::switch::SwitchOutcome;
use tagteam_engine::views::{
    AccountView, NO_DATA, ProviderAccounts, StatusView, UsageStatus, UsageView,
};
use tagteam_provider::SecretStore;

const NO_ACCOUNTS: &str = "No accounts yet. Log in with `claude`, then run `tagteam add`.\n";
/// Claude Code reloads a credentials-file change on its next message (Appendix A.3).
const FILE_STORE_HINT: &str = "Active on your next message.";
/// Claude Code caches Keychain reads for 30 s (Appendix A.3).
const KEYCHAIN_HINT: &str = "Claude Code picks this up within about 30 s; restart it to apply now.";

/// §13.5's severities, which `list` and `status` share: ≥ 90 critical, ≥ 70 warning.
const CRITICAL: &str = "\x1b[31m";
const WARNING: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";
/// A window a reading does not have, or an age that is not known.
const MISSING: &str = "—";
/// §8.7's marker for a window ahead of pace.
const AHEAD: &str = "▲ pace";
/// `list` always has a spend column once any account has a reading (§13.1).
const SPEND_HEAD: &str = "SPEND";

/// A row's windows in its provider's JSON shape: `Provider::render_usage` (§13.2).
pub type RenderUsage<'a> = &'a dyn Fn(&ProviderId, &[(Window, Pace)]) -> Value;

/// The account's email (or label when it has none).
pub fn email(r: &AccountRow) -> String {
    r.email.clone().unwrap_or_else(|| r.label.clone())
}

pub fn name(r: &AccountRow) -> String {
    let email = email(r);
    match &r.alias {
        Some(a) => format!("{a} ({email})"),
        None => email,
    }
}

/// One `list` row (§13.2). `usage` is decision-grade only (§8.4), with its fetch time and age.
/// Otherwise `usage` is null and the last good reading, if any, is `lastGoodUsage`; an
/// `unavailable` row also says why and when it is retried. Times are ISO 8601 UTC, as the
/// provider's own `resetsAt`.
pub fn row_json(v: &AccountView, usage: RenderUsage<'_>) -> Value {
    let r = &v.row;
    let u = &v.usage;
    let mut o = json!({
        "number": r.position,
        "position": r.position,
        "id": r.id.as_str(),
        "provider": r.provider.as_str(),
        "email": email(r),
        "organizationName": r.org_name,
        "organizationUuid": r.org_uuid,
        "isOrganization": !r.org_uuid.is_empty(),
        "active": v.active,
        "usageStatus": u.status.as_str(),
    });
    let rendered = u.windows.as_deref().map(|w| usage(&r.provider, w));
    let fetched_at = u.fetched_at.map(format_iso8601);
    match rendered {
        Some(current) if u.decision_grade => {
            o["usage"] = current;
            o["usageFetchedAt"] = json!(fetched_at);
            o["usageAgeSeconds"] = json!(u.age_s);
        }
        last_good => {
            o["usage"] = Value::Null;
            o["lastGoodUsage"] = last_good.unwrap_or(Value::Null);
            o["lastGoodFetchedAt"] = json!(fetched_at);
            o["lastGoodAgeSeconds"] = json!(u.age_s);
            if u.status == UsageStatus::Unavailable {
                o["usageError"] = json!(u.error);
                o["usageRetryAt"] = json!(u.retry_at.map(format_iso8601));
            }
        }
    }
    if let Some(a) = &r.alias {
        o["alias"] = json!(a);
    }
    if r.disabled {
        o["disabled"] = json!(true);
    }
    if let Some(e) = r.login_expires_at {
        o["loginExpiresAt"] = json!(e);
    }
    o
}

/// §13.2. `activeAccountNumber` is `provider`'s: the one the command queried, else the default
/// provider (for cswap compatibility), wherever it falls among `lists`.
pub fn list_json(
    lists: &[ProviderAccounts],
    provider: &ProviderId,
    usage: RenderUsage<'_>,
) -> Value {
    let active_by: serde_json::Map<String, Value> = lists
        .iter()
        .map(|l| (l.provider.to_string(), json!(l.active_position)))
        .collect();
    let rows: Vec<Value> = lists
        .iter()
        .flat_map(|l| l.accounts.iter().map(|v| row_json(v, usage)))
        .collect();
    let active = lists
        .iter()
        .find(|l| &l.provider == provider)
        .and_then(|l| l.active_position);
    json!({
        "schemaVersion": 1,
        "activeAccountNumber": active,
        "activeByProvider": active_by,
        "accounts": rows,
    })
}

/// A span of time as `list` and `status` show it: `3d09h`, `2h40m`, `45m`, or `<1m`.
pub(crate) fn duration(secs: i64) -> String {
    let s = secs.max(0);
    let (days, hours, minutes) = (s / 86_400, s % 86_400 / 3_600, s % 3_600 / 60);
    if days > 0 {
        format!("{days}d{hours:02}h")
    } else if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        "<1m".into()
    }
}

/// The time left until `at`, or `reset` once it has passed: the reading predates the window's
/// reset, so its pct no longer applies.
fn countdown(at: i64, now_s: i64) -> String {
    if at <= now_s {
        "reset".into()
    } else {
        duration(at - now_s)
    }
}

fn severity(pct: i64) -> Option<&'static str> {
    if pct >= 90 {
        Some(CRITICAL)
    } else if pct >= 70 {
        Some(WARNING)
    } else {
        None
    }
}

/// An amount with its currency's symbol (€, $, £), or its code for any other; a whole amount
/// without cents.
fn money(amount: f64, currency: &str) -> String {
    let n = if amount.fract() == 0.0 {
        format!("{amount:.0}")
    } else {
        format!("{amount:.2}")
    };
    match currency.to_ascii_uppercase().as_str() {
        "EUR" => format!("€{n}"),
        "USD" => format!("${n}"),
        "GBP" => format!("£{n}"),
        code => format!("{n} {code}"),
    }
}

/// `€0 of €20`, from a spend window's `detail {used, limit, currency}` (§8.2).
fn spend_text(w: &Window) -> Option<String> {
    let d = w.detail.as_ref().filter(|_| w.kind == WindowKind::Spend)?;
    let currency = d["currency"].as_str()?;
    Some(format!(
        "{} of {}",
        money(d["used"].as_f64()?, currency),
        money(d["limit"].as_f64()?, currency)
    ))
}

fn width(s: &str) -> usize {
    s.chars().count()
}

fn pad(s: &str, w: usize) -> String {
    format!("{s}{}", " ".repeat(w.saturating_sub(width(s))))
}

/// A table cell: its text, and the text as printed, with its percentage perhaps coloured.
struct Cell {
    plain: String,
    shown: String,
}

impl Cell {
    fn text(s: impl Into<String>) -> Self {
        let plain = s.into();
        Cell {
            shown: plain.clone(),
            plain,
        }
    }

    fn push(&mut self, s: &str) {
        self.plain.push_str(s);
        self.shown.push_str(s);
    }

    fn width(&self) -> usize {
        width(&self.plain)
    }
}

/// A pct, rounded and right-aligned in four columns, coloured by severity when `color`.
fn pct_cell(pct: f64, color: bool) -> Cell {
    let n = pct.round() as i64;
    let digits = format!("{n}%");
    let pad = " ".repeat(4usize.saturating_sub(digits.len()));
    let shown = match severity(n) {
        Some(c) if color => format!("{pad}{c}{digits}{RESET}"),
        _ => format!("{pad}{digits}"),
    };
    Cell {
        plain: format!("{pad}{digits}"),
        shown,
    }
}

/// A window as `list` shows it: its pct (a spend window's amounts), the countdown to its
/// reset, and `▲ pace` when it is ahead of pace.
fn window_cell(w: &Window, p: &Pace, now_s: i64, color: bool) -> Cell {
    let mut c = match spend_text(w) {
        Some(t) => Cell::text(t),
        None => pct_cell(w.pct, color),
    };
    if let Some(at) = w.resets_at {
        c.push(&format!("  {}", countdown(at, now_s)));
    }
    if p.ahead == Some(true) {
        c.push(&format!("  {AHEAD}"));
    }
    c
}

/// A usage status in words (§13.1): in place of a row's windows when it has no reading, and
/// beside them when its status is not `ok`.
fn words(u: &UsageView, now_s: i64) -> String {
    let retry = u
        .retry_at
        .filter(|&at| at > now_s)
        .map(|at| format!("retry {}", duration(at - now_s)));
    match u.status {
        UsageStatus::Ok => "no usage reported".into(),
        UsageStatus::TokenExpired => "token expired".into(),
        UsageStatus::ApiKey => "api key".into(),
        UsageStatus::KeychainUnavailable => "keychain unavailable".into(),
        UsageStatus::ReloginRequired => "relogin required".into(),
        UsageStatus::ForeignCredential => "foreign credential".into(),
        UsageStatus::NoCredentials => "no credentials".into(),
        UsageStatus::Unsupported => "usage unsupported".into(),
        UsageStatus::Unavailable => match (u.error.as_deref(), retry) {
            (None | Some(NO_DATA), _) => "no data yet".into(),
            (Some("over-budget"), Some(r)) => format!("over budget ({r})"),
            (Some("over-budget"), None) => "over budget".into(),
            (Some(e), Some(r)) => format!("unavailable ({e}, {r})"),
            (Some(e), None) => format!("unavailable ({e})"),
        },
    }
}

/// One of `list`'s window columns: the window key it shows (`None` for the spend column when
/// no account has spend), headed by the window's label in capitals.
struct Column {
    key: Option<String>,
    kind: WindowKind,
    head: String,
}

impl Column {
    /// The cell of a row whose reading lacks this window: `—`, under the `%` of a percentage
    /// column and at the start of the spend column.
    fn missing(&self) -> Cell {
        match self.kind {
            WindowKind::Spend => Cell::text(MISSING),
            _ => Cell::text(format!("{MISSING:>4}")),
        }
    }
}

/// §13.1's window columns for one provider's accounts: every window key some reading has, in
/// kind order (Short, Long, Spend, Scoped) and then by first appearance. SPEND is always a
/// column, `—` throughout when no account has spend: only scoped windows come and go. No
/// columns at all while no account has a reading.
fn columns(accounts: &[AccountView]) -> Vec<Column> {
    let windows: Vec<&Window> = accounts
        .iter()
        .filter_map(|v| v.usage.windows.as_ref())
        .flatten()
        .map(|(w, _)| w)
        .collect();
    let mut cols: Vec<Column> = Vec::new();
    if windows.is_empty() {
        return cols;
    }
    for kind in [
        WindowKind::Short,
        WindowKind::Long,
        WindowKind::Spend,
        WindowKind::Scoped,
    ] {
        for w in windows.iter().filter(|w| w.kind == kind) {
            if !cols
                .iter()
                .any(|c| c.key.as_deref() == Some(w.key.as_str()))
            {
                cols.push(Column {
                    key: Some(w.key.clone()),
                    kind,
                    head: w.label.to_uppercase(),
                });
            }
        }
        if kind == WindowKind::Spend && !windows.iter().any(|w| w.kind == WindowKind::Spend) {
            cols.push(Column {
                key: None,
                kind,
                head: SPEND_HEAD.into(),
            });
        }
    }
    cols
}

/// One account's line before alignment: its cells (the window columns and AGE) when it has a
/// reading, and the notes that follow: its status in words, its kind, `disabled`.
struct Row {
    marker: char,
    position: u32,
    account: String,
    cells: Option<Vec<Cell>>,
    notes: Vec<String>,
}

fn row(v: &AccountView, cols: &[Column], now_s: i64, color: bool) -> Row {
    let u = &v.usage;
    let mut notes = Vec::new();
    let cells = match u.windows.as_deref() {
        Some(ws) if !ws.is_empty() => {
            let mut cells: Vec<Cell> = cols
                .iter()
                .map(|c| {
                    ws.iter()
                        .find(|(w, _)| c.key.as_deref() == Some(w.key.as_str()))
                        .map_or_else(|| c.missing(), |(w, p)| window_cell(w, p, now_s, color))
                })
                .collect();
            cells.push(Cell::text(
                u.age_s.map_or_else(|| MISSING.to_owned(), duration),
            ));
            if u.status != UsageStatus::Ok {
                notes.push(words(u, now_s));
            }
            Some(cells)
        }
        _ => {
            notes.push(words(u, now_s));
            None
        }
    };
    if let Some(kind) = v.kind.display {
        if !notes.iter().any(|n| n == kind) {
            notes.push(kind.to_owned());
        }
    }
    if v.row.disabled {
        notes.push("disabled".into());
    }
    let account = match &v.row.org_name {
        Some(org) => format!("{} [{org}]", name(&v.row)),
        None => name(&v.row),
    };
    Row {
        marker: if v.active { '*' } else { ' ' },
        position: v.row.position,
        account,
        cells,
        notes,
    }
}

/// §13.1's table for one provider's accounts.
fn table(accounts: &[AccountView], now_s: i64, color: bool) -> String {
    let cols = columns(accounts);
    let rows: Vec<Row> = accounts
        .iter()
        .map(|v| row(v, &cols, now_s, color))
        .collect();
    let account_w = rows
        .iter()
        .map(|r| width(&r.account))
        .fold(width("ACCOUNT"), usize::max);
    let heads: Vec<&str> = cols
        .iter()
        .map(|c| c.head.as_str())
        .chain((!cols.is_empty()).then_some("AGE"))
        .collect();
    let widths: Vec<usize> = heads
        .iter()
        .enumerate()
        .map(|(i, head)| {
            rows.iter()
                .filter_map(|r| r.cells.as_ref())
                .map(|cells| cells[i].width())
                .fold(width(head), usize::max)
        })
        .collect();
    let mut header = format!("    #  {}", pad("ACCOUNT", account_w));
    for (head, w) in heads.iter().zip(&widths) {
        header.push_str("  ");
        header.push_str(&pad(head, *w));
    }
    let mut out = format!("{}\n", header.trim_end());
    for r in &rows {
        let mut line = format!(
            " {} {:>2}  {}",
            r.marker,
            r.position,
            pad(&r.account, account_w)
        );
        for (c, w) in r.cells.iter().flatten().zip(&widths) {
            line.push_str("  ");
            line.push_str(&c.shown);
            line.push_str(&" ".repeat(w.saturating_sub(c.width())));
        }
        for n in &r.notes {
            line.push_str("  ");
            line.push_str(n);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// §13.1's `list`: one table per provider that has accounts, headed by its name when there
/// is more than one. `now_s` measures the countdowns; `color` colours percentages.
pub fn list_human(
    lists: &[ProviderAccounts],
    display_names: &dyn Fn(&str) -> String,
    now_s: i64,
    color: bool,
) -> String {
    if lists.iter().all(|l| l.accounts.is_empty()) {
        return NO_ACCOUNTS.into();
    }
    let shown: Vec<&ProviderAccounts> = lists.iter().filter(|l| !l.accounts.is_empty()).collect();
    let mut s = String::new();
    for l in &shown {
        if shown.len() > 1 {
            s.push_str(&format!("{}\n", display_names(l.provider.as_str())));
        }
        s.push_str(&table(&l.accounts, now_s, color));
    }
    s
}

/// §13.2. Every shape names `provider` (the one the command ran against) at the top level, and
/// again in `active` wherever there is one: a managed row carries it already.
pub fn status_json(s: &StatusView, provider: &str, usage: RenderUsage<'_>) -> Value {
    match s {
        StatusView::NoLogin => {
            json!({"schemaVersion": 1, "provider": provider, "active": null})
        }
        StatusView::Unmanaged { email } => json!({
            "schemaVersion": 1,
            "provider": provider,
            "active": {"email": email, "provider": provider, "managed": false},
        }),
        StatusView::Managed { account, total } => {
            let mut row = row_json(account, usage);
            row["managed"] = json!(true);
            json!({"schemaVersion": 1, "provider": provider, "active": row, "totalManagedAccounts": total})
        }
    }
}

/// `status`'s usage line: each window of the reading with its countdown and pace, the
/// reading's age, and a status other than `ok` in words. A quarantined account without a
/// reading has none: the line above already says it.
fn usage_line(u: &UsageView, now_s: i64, color: bool) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(ws) = u.windows.as_deref().filter(|ws| !ws.is_empty()) {
        for (w, p) in ws {
            let mut part = match spend_text(w) {
                Some(t) => format!("{} {t}", w.label),
                None => format!("{} {}", w.label, pct_cell(w.pct, color).shown.trim_start()),
            };
            if let Some(at) = w.resets_at {
                part.push_str(&format!(" ({})", countdown(at, now_s)));
            }
            if p.ahead == Some(true) {
                part.push_str(&format!(" {AHEAD}"));
            }
            parts.push(part);
        }
        parts.push(format!(
            "{} old",
            u.age_s.map_or_else(|| MISSING.to_owned(), duration)
        ));
    }
    if u.status != UsageStatus::Ok && u.status != UsageStatus::ReloginRequired {
        parts.push(words(u, now_s));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

pub fn status_human(s: &StatusView, now_s: i64, color: bool) -> String {
    match s {
        StatusView::NoLogin => "No live login.\n".into(),
        StatusView::Unmanaged { email } => format!("Live: {email} (not managed by tagteam)\n"),
        StatusView::Managed { account, total } => {
            let marker = if account.row.quarantine_reason.is_some() {
                ", relogin required"
            } else {
                ""
            };
            let mut out = format!(
                "Live: {} (position {} of {total}){marker}\n",
                name(&account.row),
                account.row.position
            );
            if let Some(line) = usage_line(&account.usage, now_s, color) {
                out.push_str(&format!("  {line}\n"));
            }
            out
        }
    }
}

pub fn switch_json(o: &SwitchOutcome, provider: &str) -> Value {
    json!({
        "schemaVersion": 1,
        "provider": provider,
        "switched": o.switched,
        "from": o.from.as_ref().map(|r| r.position),
        "to": o.to.as_ref().map(|r| r.position),
        "strategy": o.strategy,
        "reason": o.reason.as_str(),
        "message": o.message,
        "credentialStore": credential_store(o),
        "warnings": o.warnings,
    })
}

/// §13.2: where the switch stored the credential, as the engine reports it; `None` when it
/// wrote none (a no-op).
fn credential_store(o: &SwitchOutcome) -> Option<&'static str> {
    o.stored_in.as_ref().map(|s| match s {
        SecretStore::Keychain => "keychain",
        SecretStore::File(_) | SecretStore::Fallback(_) => "file",
    })
}

/// The stderr notice for a write the keychain refused, naming where the secret went.
pub fn fallback_notice(o: &SwitchOutcome) -> Option<String> {
    match &o.stored_in {
        Some(SecretStore::Fallback(path)) => Some(format!(
            "the Keychain refused the write, so the credential was stored in {} instead",
            path.display()
        )),
        _ => None,
    }
}

pub fn switch_human(o: &SwitchOutcome) -> String {
    match (&o.to, o.switched) {
        (Some(to), true) => {
            let hint = match o.stored_in {
                Some(SecretStore::File(_) | SecretStore::Fallback(_)) => FILE_STORE_HINT,
                Some(SecretStore::Keychain) | None => KEYCHAIN_HINT,
            };
            format!(
                "Switched to {} (position {}).\n{hint}\n",
                name(to),
                to.position
            )
        }
        _ => format!("{}\n", o.message),
    }
}

/// An account command's result: the row as `list` shows it, `active` and usage included.
pub fn account_json(account: &AccountView, created: Option<bool>, usage: RenderUsage<'_>) -> Value {
    let mut v = json!({"schemaVersion": 1, "ok": true, "account": row_json(account, usage)});
    if let Some(c) = created {
        v["created"] = json!(c);
    }
    v
}
```

- [ ] **Step 6: Collect before `list` and `status`, and hand the renderer each provider**

In `crates/tagteam/src/app.rs`:

(a) Imports: change `use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};` to
`use tagteam_core::{AccountId, CLAUDE_CODE, Pace, ProviderId, Window};`, change Task 9's
`use tagteam_engine::settings::Settings;` to `use tagteam_engine::settings::{ColorMode, Settings};`,
change `use tagteam_engine::views::AccountView;` to
`use tagteam_engine::views::{AccountView, StatusView};`, and add
`use tagteam_engine::collect::CollectMode;` before `use tagteam_engine::lazy_http::LazyHttp;`.

(b) Directly above `/// §13.2: the one object `--json` prints for any error.`, add:
```rust
/// `Provider::render_usage` for a row's provider (§13.2); null for a provider this build does
/// not register, whose rows no view lists.
fn render_usage(engine: &Engine) -> impl Fn(&ProviderId, &[(Window, Pace)]) -> Value + '_ {
    move |provider, windows| {
        engine
            .provider(provider)
            .map_or(Value::Null, |p| p.render_usage(windows))
    }
}

/// §13.1 and §6.4: `--no-color` and `NO_COLOR` always turn colour off, `FORCE_COLOR` turns it
/// on, and otherwise `ui.color` decides, `auto` meaning "stdout is a terminal".
fn color_enabled(
    flag_off: bool,
    no_color: bool,
    force_color: bool,
    setting: ColorMode,
    stdout_terminal: bool,
) -> bool {
    if flag_off || no_color {
        return false;
    }
    if force_color {
        return true;
    }
    match setting {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => stdout_terminal,
    }
}

```

(c) In `struct App`, directly after `json: bool,`, add:
```rust
    /// `--no-color`.
    no_color: bool,
```
and in `run`, add `no_color: cli.no_color,` to the `App { … }` literal, directly after `json,`.

(d) In `impl App`, directly after `fn notices`, add:
```rust

    /// §8.3's on-demand collection, which the command waits for. A usage failure is never a
    /// command error: it shows in the account's row. The collector's warnings, and its error
    /// should collecting fail as a whole, go to stderr. With nothing to collect nothing is
    /// opened, so `list` on a fresh machine still creates nothing (§5).
    fn collect(&mut self, accounts: Vec<AccountId>) {
        if accounts.is_empty() {
            return;
        }
        let warnings = match self
            .engine
            .collect_usage(CollectMode::OnDemand { accounts })
        {
            Ok(report) => report.warnings,
            Err(e) => vec![format!("usage was not collected: {e}")],
        };
        for w in warnings {
            let _ = writeln!(self.io.err, "warning: {w}");
        }
    }

    /// Now, in epoch seconds, by the engine's clock: what countdowns and ages count from.
    fn now_s(&self) -> i64 {
        self.engine.now_ms().div_euclid(1000)
    }

    /// Whether `list` and `status` colour their percentages (§13.1).
    fn color(&self) -> bool {
        color_enabled(
            self.no_color,
            std::env::var_os("NO_COLOR").is_some(),
            std::env::var_os("FORCE_COLOR").is_some(),
            self.engine.settings().color,
            std::io::stdout().is_terminal(),
        )
    }
```

(e) In `dispatch`, replace the `Command::List` and `Command::Status` arms with:
```rust
            Command::List => {
                // §8.3: every listed account is offered to the collector, which fetches only
                // those that are due; the views are then read with whatever it recorded.
                let listed = self.engine.accounts(self.provider_flag.as_ref())?;
                let ids: Vec<AccountId> = listed
                    .iter()
                    .flat_map(|l| l.accounts.iter().map(|v| v.row.id.clone()))
                    .collect();
                self.collect(ids);
                let lists = self.engine.accounts(self.provider_flag.as_ref())?;
                let (now_s, color) = (self.now_s(), self.color());
                let engine = &self.engine;
                let names = |id: &str| {
                    engine
                        .provider(&ProviderId::new(id))
                        .map_or_else(|_| id.to_owned(), |p| p.display_name().to_owned())
                };
                let human = render::list_human(&lists, &names, now_s, color);
                let json = render::list_json(&lists, &self.provider(), &render_usage(engine));
                self.print(&human, json);
            }
            Command::Status => {
                let provider = self.provider();
                // §8.3: `status` collects the live account only.
                if let StatusView::Managed { account, .. } = self.engine.status(&provider)? {
                    self.collect(vec![account.row.id]);
                }
                let s = self.engine.status(&provider)?;
                let (now_s, color) = (self.now_s(), self.color());
                let human = render::status_human(&s, now_s, color);
                let json = render::status_json(&s, provider.as_str(), &render_usage(&self.engine));
                self.print(&human, json);
            }
```

(f) Replace `fn print_view` with:
```rust
    fn print_view(&mut self, human: &str, view: AccountView, created: Option<bool>) {
        let json = render::account_json(&view, created, &render_usage(&self.engine));
        self.print(human, json);
    }
```

(g) In the test module, directly above
`fn an_unreadable_active_flag_after_a_commit_is_inactive_not_an_error`'s `#[test]`, add:
```rust
    #[test]
    fn colour_is_off_under_no_color_forced_by_force_color_and_else_the_settings() {
        use ColorMode::{Always, Auto, Never};
        // (--no-color, NO_COLOR, FORCE_COLOR, ui.color, stdout is a terminal) → colour
        let cases = [
            ((false, false, false, Auto, true), true),
            ((false, false, false, Auto, false), false),
            ((false, false, false, Always, false), true),
            ((false, false, false, Never, true), false),
            ((false, false, true, Never, false), true),
            ((false, true, true, Always, true), false),
            ((true, false, true, Always, true), false),
        ];
        for ((flag, no, force, setting, terminal), want) in cases {
            assert_eq!(
                color_enabled(flag, no, force, setting, terminal),
                want,
                "{flag} {no} {force} {setting:?} {terminal}"
            );
        }
    }

```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p tagteam --lib`
Expected: PASS, the ten render tests and the new colour test among them.

Run: `cargo test -p tagteam --features test-support --test usage_cli --test app --test cli`
Expected: PASS. `usage_cli`'s six tests pin §13.1's table against the recorded reply, one request
per account and none for a second `list` within 180 s, §13.2's row shape, a 429 row, `status`'s
line, and colour.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS.

- [ ] **Step 8: Lint and format**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings && cargo clippy --workspace --all-targets -- -D warnings`
Expected: no output from `fmt`; both clippy runs finish without warnings.

- [ ] **Step 9: Commit**

```bash
git add crates/tagteam/src/render.rs crates/tagteam/src/app.rs crates/tagteam/tests/app.rs \
        crates/tagteam/tests/cli.rs crates/tagteam/tests/usage_cli.rs
git commit -m "Show usage in list and status, collected on demand"
```

---

### Task 15: `history`

`tagteam history [ACCOUNT] [--window W] [--since 7d] [--csv]` (§13.4) renders Task 13's history
view in three forms: per-window text with a sparkline, the burn rate and the projection; `--csv`,
one row per sample; and `--json`. It reads `usage_samples` through the engine and never
collects, so it never sends a request.

Judgement calls, flagged rather than decided silently:
- **The `--json` shape.** §13.2 leaves it to `--help` and a snapshot test. It is
  `{schemaVersion, provider, account: {number, id, email}, windows: [{key, label, kind, pct,
  resetsAt, samples: [{fetchedAt, pct, resetsAt}], ratePerHour, expectedPct, aheadOfPace,
  projectedExhaustionAt, willLastToReset, projectionMethod}]}`. It has a top-level `provider`
  because `status` and `switch` carry one. Each window has every §8.7 field ("JSON carries all
  fields") plus the current `pct`, `resetsAt` and `kind`. Times are ISO 8601 UTC, as in
  `usage.resetsAt` (Task 6).
- **Default windows.** Without `--window`, the CLI keeps the windows `is_relevant` accepts under
  `autoswitch.models` (§13.4's "its relevant windows"). With `--window`, whatever the engine
  matches is shown, so `--window spend` works although spend is never relevant.
- **`--csv` with `--json`** is a usage error (exit 2). Two formats cannot share one stdout.
- **`since_s`** in `Engine::history` is read as the epoch-second lower bound on `fetched_at`,
  per Decision 1: a time, not a span. The CLI passes `now − span`.
- **One countdown helper.** Task 14's `crate::render::duration` formats every countdown and
  age (`3d09h`, `2h40m`, `45m`, `<1m`), so `history` and Task 16's `statusline` call it rather
  than define a second one. Below a minute it reads `<1m`, and a time already past clamps to
  `<1m` too.
- **Seeding.** Tests record readings through the store's own write path (`reserve_usage` then
  `record_usage`), the path a fetch takes. `two_accounts` cannot be used for the setup: it runs
  `list`, which collects usage (Task 14), and offline that records a failed fetch and a backoff
  that would refuse the seeded reservations. The new `two_fresh_accounts` reads the ids from the
  store instead.
- Exact text is pinned by unit tests with a fixed `now`. The binary tests seed relative to the
  wall clock and keep every countdown at least 30 s from a unit boundary.

**Files:**
- Create: `crates/tagteam/src/history.rs`, `crates/tagteam/tests/history.rs`
- Modify: `crates/tagteam/src/lib.rs`, `crates/tagteam/src/cli.rs`, `crates/tagteam/src/app.rs`,
  `crates/tagteam/tests/common/mod.rs`

**Interfaces:**
- Consumes:
  - Task 1: `tagteam_engine::settings::Settings` (`models`), read through Task 9's
    `Engine::settings()`; a `config.toml` of `[autoswitch]\nmodels = ["Fable"]` sets it.
  - Task 2: `tagteam_core::{Window, WindowKind}` (crate-root re-exports),
    `tagteam_core::usage::is_relevant(&Window, &[String]) -> bool`.
  - Task 4 (tests): `tagteam_core::{PollBudget, PollPlan}`, `PollBudget::STANDARD`.
  - Task 5: `tagteam_core::{Pace, ProjectionMethod, Sample}` and `ProjectionMethod::as_str`;
    `tagteam_core::pace::pace` in a binary test, as the expected value.
  - Task 6: `tagteam_cc::usage::format_iso8601(epoch_s: i64) -> String`
    (`"2030-01-04T17:20:00Z"`).
  - Task 8 (tests): `Store::{account, accounts, reserve_usage, record_usage}` and
    `tagteam_engine::store::Reserve`.
  - Task 9: `build_engine` loads the settings for every command but `statusline`.
  - Task 13: `Engine::history(&self, account: &AccountId, window: Option<&str>, since_s: i64)
    -> Result<HistoryView, EngineError>`;
    `tagteam_engine::views::{HistoryView { account, windows }, HistoryWindow { window, samples,
    pace }, UsageView, UsageStatus}`; `AccountView.usage`; `Engine::status` (read-only) for the
    default account.
  - Task 14: `crate::render::duration(secs: i64) -> String` (`3d09h`, `2h40m`, `45m`, `<1m`;
    a time already past is `<1m`).
  - Existing: `render::{email, name}`, `App::{resolve, print, provider}`, `Failure`,
    `KIND_UNMANAGED_ACCOUNT`, `common::{cmd, seed_home, login}`.
- Produces:
  - `Command::History { account: Option<String>, window: Option<String>, since: String,
    csv: bool }` (the contract's variant), and `App::history` and `App::live_row` in `app.rs`.
  - `crate::history::{parse_since, keep_relevant, sparkline, human, csv, json}`.
  - CLI error kinds: `no-live-login` (raised by the CLI, with the engine's kind string and its
    own message), `unmanaged-account`, and `usage`.
  - Test helpers in `crates/tagteam/tests/common/mod.rs`, which Task 16 uses:
    `now_epoch_s() -> i64`, `two_fresh_accounts(root) -> (String, String)`,
    `usage_window(key, label, kind, pct, resets_at, period_s) -> Window`, and
    `record_reading(root, id, at_s, windows)`.

- [ ] **Step 1: Write the failing rendering tests**

In `crates/tagteam/src/lib.rs`, replace the module list:

```rust
pub mod app;
pub mod cli;
pub mod prompt;
mod render;
mod root_guard;
```

with:

```rust
pub mod app;
pub mod cli;
mod history;
pub mod prompt;
mod render;
mod root_guard;
```

Create `crates/tagteam/src/history.rs` holding only its tests for now:

```rust
#[cfg(test)]
mod tests {
    use serde_json::json;
    use tagteam_core::{
        AccountId, CLAUDE_CODE, Pace, ProjectionMethod, ProviderId, Sample, Window, WindowKind,
    };
    use tagteam_engine::store::AccountRow;
    use tagteam_engine::views::{AccountView, HistoryView, HistoryWindow, UsageStatus, UsageView};
    use tagteam_provider::KindTraits;

    use super::*;

    /// 2026-09-21T14:13:20Z.
    const NOW: i64 = 1_790_000_000;
    const HOUR: i64 = 3_600;

    /// `b@x.co` at position 2, live.
    fn account() -> AccountView {
        AccountView {
            row: AccountRow {
                id: AccountId::from_string("0192b"),
                provider: ProviderId::new(CLAUDE_CODE),
                position: 2,
                identity_key: "b@x.co\n".into(),
                label: "b@x.co".into(),
                email: Some("b@x.co".into()),
                org_uuid: String::new(),
                org_name: None,
                account_uuid: None,
                kind: "oauth".into(),
                alias: None,
                disabled: false,
                identity_json: json!({}),
                login_expires_at: None,
                login_epoch: 0,
                replacing_fp: None,
                quarantine_reason: None,
                quarantine_fp: None,
                quarantine_at: None,
                added_at: 1,
            },
            active: true,
            kind: KindTraits {
                refreshable: true,
                managed_key_axis: false,
                default_email_prefix: None,
                display: None,
            },
            usage: UsageView {
                status: UsageStatus::Ok,
                windows: None,
                decision_grade: false,
                fetched_at: None,
                age_s: None,
                error: None,
                retry_at: None,
            },
        }
    }

    fn window(key: &str, label: &str, kind: WindowKind, pct: f64, resets_at: Option<i64>) -> Window {
        Window {
            key: key.into(),
            label: label.into(),
            kind,
            pct,
            resets_at,
            period_s: None,
            detail: None,
        }
    }

    /// Readings of one window instance 6, 4 and 2 hours before `NOW`.
    fn samples(pcts: [f64; 3], resets_at: i64) -> Vec<Sample> {
        [6, 4, 2]
            .into_iter()
            .zip(pcts)
            .map(|(hours, pct)| Sample {
                fetched_at: NOW - hours * HOUR,
                pct,
                resets_at: Some(resets_at),
            })
            .collect()
    }

    fn history_window(window: Window, samples: Vec<Sample>, pace: Pace) -> HistoryWindow {
        HistoryWindow {
            window,
            samples,
            pace,
        }
    }

    /// 5h with no rate yet; 7d running out 12 hours on by regression; Fable, never sampled,
    /// lasting to its reset at the average pace.
    fn view() -> HistoryView {
        let (r5, r7) = (NOW + 9_600, NOW + 266_400);
        HistoryView {
            account: account(),
            windows: vec![
                history_window(
                    window("5h", "5h", WindowKind::Short, 9.0, Some(r5)),
                    samples([5.0, 7.0, 9.0], r5),
                    Pace::default(),
                ),
                history_window(
                    window("7d", "7d", WindowKind::Long, 30.0, Some(r7)),
                    samples([10.0, 20.0, 30.0], r7),
                    Pace {
                        rate_per_hour: Some(5.0),
                        method: Some(ProjectionMethod::Regression),
                        exhaustion_at: Some(NOW + 43_200),
                        will_last_to_reset: Some(false),
                        ..Pace::default()
                    },
                ),
                history_window(
                    window("scoped:Fable", "Fable", WindowKind::Scoped, 2.0, Some(r7)),
                    vec![],
                    Pace {
                        expected_pct: Some(44.0),
                        ahead: Some(false),
                        rate_per_hour: Some(0.3),
                        method: Some(ProjectionMethod::Average),
                        exhaustion_at: Some(NOW + 1_000_000),
                        will_last_to_reset: Some(true),
                    },
                ),
            ],
        }
    }

    #[test]
    fn since_takes_whole_days_hours_or_minutes() {
        assert_eq!(parse_since("7d"), Some(604_800));
        assert_eq!(parse_since("12h"), Some(43_200));
        assert_eq!(parse_since("30m"), Some(1_800));
        for bad in [
            "",
            "d",
            "0d",
            "7",
            "7w",
            "1.5h",
            "+7d",
            " 7d",
            "-1d",
            "999999999999999d",
            "99999999999999999999d",
        ] {
            assert_eq!(parse_since(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_sparkline_scales_zero_to_one_hundred() {
        assert_eq!(sparkline(&[]), "");
        assert_eq!(
            sparkline(&[0.0, 14.3, 28.6, 42.9, 57.1, 71.4, 85.7, 100.0]),
            "▁▂▃▄▅▆▇█"
        );
        assert_eq!(sparkline(&[-5.0, 140.0]), "▁█", "clamped to 0–100");
    }

    #[test]
    fn a_long_history_is_bucketed_by_its_peaks() {
        let spike: Vec<f64> = (0..120).map(|i| if i == 7 { 100.0 } else { 0.0 }).collect();
        let line = sparkline(&spike);
        assert_eq!(line.chars().count(), 60);
        assert_eq!(line.chars().nth(3), Some('█'), "a bucket shows its peak: {line}");
        assert_eq!(sparkline(&[0.0; 61]).chars().count(), 31);
    }

    #[test]
    fn the_text_shows_each_windows_rate_and_projection() {
        assert_eq!(
            human(&view(), "7d", NOW),
            concat!(
                "b@x.co (position 2), last 7d\n",
                "\n",
                "5h  9%  resets in 2h40m\n",
                "  ▁▁▂  3 samples\n",
                "  no rate yet\n",
                "\n",
                "7d  30%  resets in 3d02h\n",
                "  ▂▂▃  3 samples\n",
                "  +5.0 pts/h · runs out in 12h00m (regression)\n",
                "\n",
                "Fable  2%  resets in 3d02h\n",
                "  no samples\n",
                "  +0.3 pts/h · lasts to reset (average)\n",
            )
        );
    }

    #[test]
    fn a_projection_lasts_runs_out_or_is_missing() {
        let with = |pace: Pace| {
            let w = window("7d", "7d", WindowKind::Long, 30.0, None);
            projection(&history_window(w, vec![], pace), NOW)
        };
        assert_eq!(with(Pace::default()), "no rate yet");
        let past = Pace {
            rate_per_hour: Some(5.0),
            method: Some(ProjectionMethod::Regression),
            exhaustion_at: Some(NOW - 60),
            ..Pace::default()
        };
        assert_eq!(with(past), "+5.0 pts/h · runs out in <1m (regression)");
        let flat = Pace {
            rate_per_hour: Some(0.0),
            method: Some(ProjectionMethod::Average),
            ..Pace::default()
        };
        assert_eq!(with(flat), "+0.0 pts/h · no projection (average)");
    }

    #[test]
    fn an_account_without_windows_says_so() {
        let v = HistoryView {
            account: account(),
            windows: vec![],
        };
        assert_eq!(human(&v, "7d", NOW), "No usage history for b@x.co.\n");
        assert_eq!(csv(&v), "window,fetched_at,pct,resets_at\n");
    }

    #[test]
    fn csv_is_one_row_per_sample_in_iso_8601() {
        assert_eq!(
            csv(&view()),
            concat!(
                "window,fetched_at,pct,resets_at\n",
                "5h,2026-09-21T08:13:20Z,5,2026-09-21T16:53:20Z\n",
                "5h,2026-09-21T10:13:20Z,7,2026-09-21T16:53:20Z\n",
                "5h,2026-09-21T12:13:20Z,9,2026-09-21T16:53:20Z\n",
                "7d,2026-09-21T08:13:20Z,10,2026-09-24T16:13:20Z\n",
                "7d,2026-09-21T10:13:20Z,20,2026-09-24T16:13:20Z\n",
                "7d,2026-09-21T12:13:20Z,30,2026-09-24T16:13:20Z\n",
            )
        );
    }

    #[test]
    fn a_csv_field_is_quoted_only_when_it_must_be() {
        assert_eq!(csv_field("scoped:Fable"), "scoped:Fable");
        assert_eq!(csv_field("scoped:a,b"), "\"scoped:a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        let unreset = HistoryView {
            account: account(),
            windows: vec![history_window(
                window("spend", "spend", WindowKind::Spend, 0.0, None),
                vec![Sample {
                    fetched_at: NOW,
                    pct: 0.0,
                    resets_at: None,
                }],
                Pace::default(),
            )],
        };
        assert_eq!(
            csv(&unreset),
            "window,fetched_at,pct,resets_at\nspend,2026-09-21T14:13:20Z,0,\n"
        );
    }

    #[test]
    fn json_carries_every_sample_and_every_projection_field() {
        let v = json(&view(), "claude-code");
        assert_eq!(v["schemaVersion"], 1);
        assert_eq!(v["provider"], "claude-code");
        assert_eq!(
            v["account"],
            json!({"number": 2, "id": "0192b", "email": "b@x.co"})
        );
        assert_eq!(
            v["windows"][1],
            json!({
                "key": "7d", "label": "7d", "kind": "long", "pct": 30.0,
                "resetsAt": "2026-09-24T16:13:20Z",
                "samples": [
                    {"fetchedAt": "2026-09-21T08:13:20Z", "pct": 10.0, "resetsAt": "2026-09-24T16:13:20Z"},
                    {"fetchedAt": "2026-09-21T10:13:20Z", "pct": 20.0, "resetsAt": "2026-09-24T16:13:20Z"},
                    {"fetchedAt": "2026-09-21T12:13:20Z", "pct": 30.0, "resetsAt": "2026-09-24T16:13:20Z"}
                ],
                "ratePerHour": 5.0, "expectedPct": null, "aheadOfPace": null,
                "projectedExhaustionAt": "2026-09-22T02:13:20Z", "willLastToReset": false,
                "projectionMethod": "regression"
            })
        );
        let fable = &v["windows"][2];
        assert_eq!(
            (
                fable["expectedPct"].clone(),
                fable["aheadOfPace"].clone(),
                fable["projectionMethod"].clone(),
                fable["samples"].clone()
            ),
            (json!(44.0), json!(false), json!("average"), json!([]))
        );
        let short = &v["windows"][0];
        assert_eq!(
            (
                short["ratePerHour"].clone(),
                short["projectionMethod"].clone(),
                short["projectedExhaustionAt"].clone()
            ),
            (Value::Null, Value::Null, Value::Null)
        );
    }

    #[test]
    fn without_a_window_flag_only_relevant_windows_remain() {
        let keys = |models: &[&str]| {
            let mut v = view();
            v.windows.push(history_window(
                window("spend", "spend", WindowKind::Spend, 0.0, None),
                vec![],
                Pace::default(),
            ));
            let models: Vec<String> = models.iter().map(|m| (*m).to_owned()).collect();
            keep_relevant(&mut v, &models);
            v.windows
                .iter()
                .map(|w| w.window.key.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(&[]), ["5h", "7d"]);
        assert_eq!(keys(&["FABLE"]), ["5h", "7d", "scoped:Fable"]);
        assert_eq!(keys(&["all"]), ["5h", "7d", "scoped:Fable"]);
    }
}
```

- [ ] **Step 2: Run the rendering tests to verify they fail**

Run: `cargo test -p tagteam --lib -- history::`
Expected: FAIL to compile. error E0425, "cannot find function parse_since in this scope", and
likewise for `sparkline`, `human`, `projection`, `csv`, `csv_field`, `json` and
`keep_relevant`.

- [ ] **Step 3: Implement the history rendering**

In `crates/tagteam/src/history.rs`, above the test module, add:

```rust
//! `tagteam history` (§13.4): the stored samples as sparklines, burn rates and projections, or
//! raw as CSV and JSON. Reads only.

use serde_json::{Value, json};
use tagteam_cc::usage::format_iso8601;
use tagteam_core::ProjectionMethod;
use tagteam_core::usage::is_relevant;
use tagteam_engine::views::{HistoryView, HistoryWindow};

use crate::render;

const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// The widest sparkline. A longer history is bucketed, and each bucket shows its peak.
const SPARK_WIDTH: usize = 60;

/// `--since`: `<n>d`, `<n>h` or `<n>m`, `n` a positive whole number. Seconds.
pub(crate) fn parse_since(s: &str) -> Option<i64> {
    let unit = match s.as_bytes().last()? {
        b'd' => 86_400,
        b'h' => 3_600,
        b'm' => 60,
        _ => return None,
    };
    let digits = &s[..s.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0)?
        .checked_mul(unit)
}

/// §13.4's default: without `--window`, only the windows that count for switching (§8.2).
pub(crate) fn keep_relevant(view: &mut HistoryView, models: &[String]) {
    view.windows.retain(|w| is_relevant(&w.window, models));
}

/// Eight levels from 0 to 100 %. Bucketing keeps a short spike visible: a bucket shows its
/// peak, never an average.
pub(crate) fn sparkline(pcts: &[f64]) -> String {
    if pcts.is_empty() {
        return String::new();
    }
    pcts.chunks(pcts.len().div_ceil(SPARK_WIDTH))
        .map(|bucket| {
            let peak = bucket.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            SPARK[(peak.clamp(0.0, 100.0) / 100.0 * 7.0).round() as usize]
        })
        .collect()
}

/// §13.4's text: per window, its reading and reset, a sparkline of its samples, and its burn
/// rate with the projection and the method behind it. `now_s` dates the countdowns.
pub(crate) fn human(view: &HistoryView, since: &str, now_s: i64) -> String {
    let row = &view.account.row;
    if view.windows.is_empty() {
        return format!("No usage history for {}.\n", render::name(row));
    }
    let mut s = format!(
        "{} (position {}), last {since}\n",
        render::name(row),
        row.position
    );
    for w in &view.windows {
        s.push('\n');
        s.push_str(&block(w, now_s));
    }
    s
}

fn block(hw: &HistoryWindow, now_s: i64) -> String {
    let w = &hw.window;
    let mut head = format!("{}  {:.0}%", w.label, w.pct.round());
    if let Some(at) = w.resets_at {
        head.push_str(&format!("  resets in {}", render::duration(at - now_s)));
    }
    let pcts: Vec<f64> = hw.samples.iter().map(|x| x.pct).collect();
    let samples = match pcts.len() {
        0 => "no samples".to_owned(),
        1 => format!("{}  1 sample", sparkline(&pcts)),
        n => format!("{}  {n} samples", sparkline(&pcts)),
    };
    format!("{head}\n  {samples}\n  {}\n", projection(hw, now_s))
}

/// The burn rate and where it leads (§8.7): `lasts to reset` when the window will, else when it
/// runs out, with the method that measured the rate.
fn projection(hw: &HistoryWindow, now_s: i64) -> String {
    let p = &hw.pace;
    let Some(rate) = p.rate_per_hour else {
        return "no rate yet".to_owned();
    };
    let eta = match (p.will_last_to_reset, p.exhaustion_at) {
        (Some(true), _) => "lasts to reset".to_owned(),
        (_, Some(at)) => format!("runs out in {}", render::duration(at - now_s)),
        _ => "no projection".to_owned(),
    };
    let method = p
        .method
        .map_or_else(String::new, |m| format!(" ({})", m.as_str()));
    format!("{rate:+.1} pts/h · {eta}{method}")
}

/// `--csv`: `window,fetched_at,pct,resets_at`, one row per sample, times in ISO 8601 UTC.
pub(crate) fn csv(view: &HistoryView) -> String {
    let mut s = String::from("window,fetched_at,pct,resets_at\n");
    for hw in &view.windows {
        let key = csv_field(&hw.window.key);
        for x in &hw.samples {
            let resets = x.resets_at.map(format_iso8601).unwrap_or_default();
            s.push_str(&format!(
                "{key},{},{},{resets}\n",
                format_iso8601(x.fetched_at),
                x.pct
            ));
        }
    }
    s
}

/// RFC 4180: a field holding a comma, a quote or a line break is quoted, its quotes doubled.
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// `--json`: the shape `history --help` documents.
pub(crate) fn json(view: &HistoryView, provider: &str) -> Value {
    let row = &view.account.row;
    let windows: Vec<Value> = view.windows.iter().map(window_json).collect();
    json!({
        "schemaVersion": 1,
        "provider": provider,
        "account": {"number": row.position, "id": row.id.as_str(), "email": render::email(row)},
        "windows": windows,
    })
}

fn window_json(hw: &HistoryWindow) -> Value {
    let (w, p) = (&hw.window, &hw.pace);
    let samples: Vec<Value> = hw
        .samples
        .iter()
        .map(|x| {
            json!({
                "fetchedAt": format_iso8601(x.fetched_at),
                "pct": x.pct,
                "resetsAt": x.resets_at.map(format_iso8601),
            })
        })
        .collect();
    json!({
        "key": w.key,
        "label": w.label,
        "kind": w.kind.as_str(),
        "pct": w.pct,
        "resetsAt": w.resets_at.map(format_iso8601),
        "samples": samples,
        "ratePerHour": p.rate_per_hour,
        "expectedPct": p.expected_pct,
        "aheadOfPace": p.ahead,
        "projectedExhaustionAt": p.exhaustion_at.map(format_iso8601),
        "willLastToReset": p.will_last_to_reset,
        "projectionMethod": p.method.map(ProjectionMethod::as_str),
    })
}
```

- [ ] **Step 4: Run the rendering tests to verify they pass**

Run: `cargo test -p tagteam --lib -- history::`
Expected: PASS, 10 tests. Among them are `the_text_shows_each_windows_rate_and_projection`,
`json_carries_every_sample_and_every_projection_field` and
`without_a_window_flag_only_relevant_windows_remain`.

- [ ] **Step 5: Add the shared test helpers and write the failing binary tests**

In `crates/tagteam/tests/common/mod.rs`, add the following imports. If Task 14 already imports
from `tagteam_core` or `tagteam_engine::store`, merge these names into those lines.

```rust
use tagteam_core::{AccountId, CLAUDE_CODE, PollBudget, PollPlan, ProviderId, Window, WindowKind};
use tagteam_engine::store::{Reserve, Store};
```

Then append:

```rust
/// Now, in epoch seconds, as the binary's clock reads it.
pub fn now_epoch_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// `a@x.co` at position 1 and `b@x.co` at position 2 (live), set up as `two_accounts` does, but
/// with the ids read from the store rather than from `list`. `list` collects usage, and with
/// every endpoint offline it would record a failed fetch, and its backoff, before the test
/// seeds its readings.
pub fn two_fresh_accounts(root: &Path) -> (String, String) {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    cmd(root).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    cmd(root).arg("add").assert().success();
    let store = Store::open_existing(&env.data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    let rows = store.accounts(&ProviderId::new(CLAUDE_CODE)).unwrap();
    (
        rows[0].id.as_str().to_owned(),
        rows[1].id.as_str().to_owned(),
    )
}

/// A window as a provider reports it, without provider detail.
pub fn usage_window(
    key: &str,
    label: &str,
    kind: WindowKind,
    pct: f64,
    resets_at: Option<i64>,
    period_s: Option<i64>,
) -> Window {
    Window {
        key: key.to_owned(),
        label: label.to_owned(),
        kind,
        pct,
        resets_at,
        period_s,
        detail: None,
    }
}

/// Records `windows` as account `id`'s reading taken at `at_s`, through the store's own write
/// path, as a fetch would: reserve (§8.3 phase 1), then record (phase 3), which writes
/// `usage_state` and one `usage_samples` row per window. One account's readings go in time
/// order, at least 180 s apart (the on-demand rule's minimum age).
pub fn record_reading(root: &Path, id: &str, at_s: i64, windows: &[Window]) {
    let store = Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    let row = store.account(&AccountId::from_string(id)).unwrap().unwrap();
    let reservation = match store
        .reserve_usage(&row, at_s * 1000, true, &PollBudget::STANDARD)
        .unwrap()
    {
        Reserve::Reserved(r) => r,
        other => panic!("no reservation for a reading at {at_s}: {other:?}"),
    };
    let plan = PollPlan {
        interval_s: 300,
        next_poll_at: at_s + 300,
    };
    assert!(
        store
            .record_usage(&reservation, windows, at_s, &plan, 180)
            .unwrap(),
        "the record was fenced out"
    );
}
```

Create `crates/tagteam/tests/history.rs`:

```rust
//! `tagteam history` through the binary (§13.4). Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::path::Path;

use common::{
    cmd, login, now_epoch_s, record_reading, seed_home, two_fresh_accounts, usage_window,
};
use serde_json::{Value, json};
use tagteam_cc::usage::format_iso8601 as iso;
use tagteam_core::pace::pace;
use tagteam_core::{ProjectionMethod, Sample, WindowKind};
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{Env, FileKeychain};

const HOUR: i64 = 3_600;
const WEEK: i64 = 604_800;

/// `b` (live) read three times, two hours apart, the last two hours ago, with each window on one
/// instance throughout. `a` is never read.
struct Seeded {
    dir: tempfile::TempDir,
    b: String,
    at: [i64; 3],
    r7: i64,
}

impl Seeded {
    fn root(&self) -> &Path {
        self.dir.path()
    }
}

fn seeded() -> Seeded {
    let dir = tempfile::tempdir().unwrap();
    let (_, b) = two_fresh_accounts(dir.path());
    let now = now_epoch_s();
    let at = [now - 6 * HOUR, now - 4 * HOUR, now - 2 * HOUR];
    // Well clear of a unit boundary, so a countdown read seconds later prints the same text.
    let (r5, r7) = (now + 2 * HOUR + 40 * 60 + 30, now + 3 * 86_400 + 30 * 60);
    for (i, t) in at.into_iter().enumerate() {
        let i = i as f64;
        record_reading(
            dir.path(),
            &b,
            t,
            &[
                usage_window("5h", "5h", WindowKind::Short, 5.0 + 2.0 * i, Some(r5), Some(5 * HOUR)),
                usage_window("7d", "7d", WindowKind::Long, 10.0 + 10.0 * i, Some(r7), Some(WEEK)),
                usage_window("spend", "spend", WindowKind::Spend, 0.0, None, None),
                usage_window("scoped:Fable", "Fable", WindowKind::Scoped, i, Some(r7), Some(WEEK)),
            ],
        );
    }
    Seeded { dir, b, at, r7 }
}

fn history_json(root: &Path, args: &[&str]) -> Value {
    let out = cmd(root)
        .arg("history")
        .args(args)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&out).unwrap()
}

fn keys(v: &Value) -> Vec<String> {
    v["windows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["key"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn history_defaults_to_the_live_account_and_its_relevant_windows() {
    let s = seeded();
    let v = history_json(s.root(), &[]);
    assert_eq!(
        (v["schemaVersion"].clone(), v["provider"].clone()),
        (json!(1), json!("claude-code"))
    );
    assert_eq!(
        v["account"],
        json!({"number": 2, "id": s.b, "email": "b@x.co"})
    );
    // Spend never counts, and no model is configured (§8.2).
    assert_eq!(keys(&v), ["5h", "7d"]);
    // The 7d window as stored, with the pace of its last reading: the engine calls the same
    // pure function over the same stored values.
    let samples: Vec<Sample> = s
        .at
        .iter()
        .zip([10.0, 20.0, 30.0])
        .map(|(&fetched_at, pct)| Sample {
            fetched_at,
            pct,
            resets_at: Some(s.r7),
        })
        .collect();
    let current = usage_window("7d", "7d", WindowKind::Long, 30.0, Some(s.r7), Some(WEEK));
    let p = pace(&current, s.at[2], &samples);
    assert_eq!(p.method, Some(ProjectionMethod::Regression), "{p:?}");
    assert!((p.rate_per_hour.unwrap() - 5.0).abs() < 1e-9, "{p:?}");
    let stored: Vec<Value> = samples
        .iter()
        .map(|x| json!({"fetchedAt": iso(x.fetched_at), "pct": x.pct, "resetsAt": iso(s.r7)}))
        .collect();
    assert_eq!(
        v["windows"][1],
        json!({
            "key": "7d", "label": "7d", "kind": "long", "pct": 30.0, "resetsAt": iso(s.r7),
            "samples": stored,
            "ratePerHour": p.rate_per_hour, "expectedPct": p.expected_pct, "aheadOfPace": p.ahead,
            "projectedExhaustionAt": p.exhaustion_at.map(iso),
            "willLastToReset": p.will_last_to_reset, "projectionMethod": "regression",
        })
    );
}

#[test]
fn a_configured_model_is_relevant_by_default() {
    let s = seeded();
    let config = Env::for_test(s.root()).config_dir();
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("config.toml"), "[autoswitch]\nmodels = [\"Fable\"]\n").unwrap();
    assert_eq!(
        keys(&history_json(s.root(), &[])),
        ["5h", "7d", "scoped:Fable"]
    );
}

#[test]
fn window_picks_one_window_by_key_or_label_and_since_bounds_the_samples() {
    let s = seeded();
    assert_eq!(
        keys(&history_json(s.root(), &["--window", "fable"])),
        ["scoped:Fable"]
    );
    assert_eq!(
        keys(&history_json(s.root(), &["--window", "SPEND"])),
        ["spend"]
    );
    let v = history_json(s.root(), &["--window", "7d", "--since", "3h"]);
    let fetched: Vec<&str> = v["windows"][0]["samples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["fetchedAt"].as_str().unwrap())
        .collect();
    assert_eq!(fetched, [iso(s.at[2])]);
}

#[test]
fn csv_prints_the_raw_samples() {
    let s = seeded();
    let out = cmd(s.root())
        .args(["history", "--window", "7d", "--csv"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let mut expected = String::from("window,fetched_at,pct,resets_at\n");
    for (t, pct) in s.at.iter().zip(["10", "20", "30"]) {
        expected.push_str(&format!("7d,{},{pct},{}\n", iso(*t), iso(s.r7)));
    }
    assert_eq!(String::from_utf8(out).unwrap(), expected);
}

#[test]
fn the_text_shows_sparklines_rates_and_projections() {
    let s = seeded();
    let out = cmd(s.root())
        .arg("history")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    // 5h climbs a point an hour and lasts to its reset; 7d climbs five and runs out about 12
    // hours after its last reading, a countdown whose exact minute depends on the wall clock.
    assert!(
        text.starts_with(concat!(
            "b@x.co (position 2), last 7d\n\n",
            "5h  9%  resets in 2h40m\n  ▁▁▂  3 samples\n  +1.0 pts/h · lasts to reset (regression)\n\n",
            "7d  30%  resets in 3d00h\n  ▂▂▃  3 samples\n  +5.0 pts/h · runs out in ",
        )),
        "{text}"
    );
    assert!(text.ends_with(" (regression)\n"), "{text}");
    assert_eq!(text.matches(" samples\n").count(), 2, "only relevant windows: {text}");
}

#[test]
fn an_account_without_samples_says_so() {
    let s = seeded();
    cmd(s.root())
        .args(["history", "1"])
        .assert()
        .success()
        .stdout("No usage history for a@x.co.\n");
    assert_eq!(history_json(s.root(), &["1"])["windows"], json!([]));
}

#[test]
fn without_a_live_login_history_needs_an_account() {
    const NO_LOGIN: &str = "there is no live login; name an account, or log in with `claude` first";
    let s = seeded();
    let env = Env::for_test(s.root());
    seed_home(&env);
    cmd(s.root())
        .arg("history")
        .assert()
        .code(1)
        .stdout("")
        .stderr(format!("tagteam: {NO_LOGIN}\n"));
    let out = cmd(s.root())
        .args(["history", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "no-live-login", "message": NO_LOGIN}})
    );
    login(&env, &FileKeychain::new(s.root().join("keychain")), "c@x.co", "", "rt-c");
    cmd(s.root()).arg("history").assert().code(1).stderr(
        "tagteam: the live login (c@x.co) is not managed by tagteam; name an account, or run `tagteam add` first\n",
    );
    // Naming the account needs no live login.
    assert_eq!(history_json(s.root(), &["2"])["account"]["email"], "b@x.co");
}

#[test]
fn history_never_fetches() {
    // Both accounts are due for a fetch: `b`'s reading is two hours old and `a` has none.
    let s = seeded();
    let server = MockServer::start();
    server.on(
        "GET",
        "/api/oauth/usage",
        MockReply::Json {
            status: 200,
            body: json!({}),
        },
    );
    let cases: [&[&str]; 3] = [
        &["history"],
        &["history", "--json"],
        &["history", "--csv"],
    ];
    for args in cases {
        cmd(s.root())
            .env("TAGTEAM_TEST_API_BASE", server.base_url())
            .args(args)
            .assert()
            .success();
    }
    assert_eq!(server.requests().len(), 0);
}

#[test]
fn bad_flags_are_usage_errors() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("home")).unwrap();
    for since in ["7w", "0d", "d", "1.5h", "7"] {
        cmd(d.path())
            .args(["history", "--since", since])
            .assert()
            .code(2)
            .stdout("")
            .stderr("tagteam: --since takes a span like 14d, 12h or 30m\n");
    }
    let out = cmd(d.path())
        .args(["history", "--csv", "--json"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "usage",
               "message": "--csv and --json are two output formats; pass one"}})
    );
}

#[test]
fn help_documents_the_json_shape() {
    // §13.2: `history`'s `--json` shape is documented in `--help`.
    let d = tempfile::tempdir().unwrap();
    let out = cmd(d.path())
        .args(["history", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help = String::from_utf8(out).unwrap();
    for word in ["never fetches", "fetchedAt", "ratePerHour", "projectionMethod"] {
        assert!(help.contains(word), "{word}: {help}");
    }
}
```

- [ ] **Step 6: Run the binary tests to verify they fail**

Run: `cargo test -p tagteam --features test-support --test history`
Expected: FAIL, all 10 tests. `history` is not a subcommand yet, so every run exits 2 with
clap's `unrecognized subcommand 'history'`.

- [ ] **Step 7: Add the command**

In `crates/tagteam/src/cli.rs`, add this variant after `Move { account: String, position: u32 },`,
the last variant of `Command`:

```rust
    /// Usage history: burn rate, and when each window runs out
    ///
    /// Reads the stored samples only; it never fetches. With no ACCOUNT it shows the live
    /// login's account, and with no --window the windows that count for switching. --json
    /// prints {schemaVersion, provider, account: {number, id, email}, windows: [{key, label,
    /// kind, pct, resetsAt, samples: [{fetchedAt, pct, resetsAt}], ratePerHour, expectedPct,
    /// aheadOfPace, projectedExhaustionAt, willLastToReset, projectionMethod}]}, with times in
    /// ISO 8601 UTC.
    History {
        account: Option<String>,
        /// One window: 5h, 7d, spend, or a model name
        #[arg(long)]
        window: Option<String>,
        /// How far back: 14d, 12h or 30m
        #[arg(long, default_value = "7d")]
        since: String,
        /// Print the raw samples as CSV
        #[arg(long)]
        csv: bool,
    },
```

`touches_keychain` stays as it is: `history` reads the store and `~/.claude.json` only.

In `crates/tagteam/src/app.rs`:

1. Directly after `const KIND_UNMANAGED_ACCOUNT: &str = "unmanaged-account";` add:

```rust
/// The engine's kind for the same condition; the CLI raises it with its own message.
const KIND_NO_LIVE_LOGIN: &str = "no-live-login";
```

2. Directly after `const ALIAS_USAGE: &str = "alias takes ACCOUNT NAME, ACCOUNT --unset, or no arguments";`
   add:

```rust
const NO_LIVE_LOGIN: &str =
    "there is no live login; name an account, or log in with `claude` first";
const CSV_AND_JSON: &str = "--csv and --json are two output formats; pass one";
const BAD_SINCE: &str = "--since takes a span like 14d, 12h or 30m";
```

3. Add `StatusView` to the `use tagteam_engine::views::{…}` import, and `history` to the
   `use crate::{…}` import (`use crate::{history, render, root_guard};`).

4. In `dispatch`, after the `Command::Move { account, position }` arm (the last arm of the
   match), add:

```rust
            Command::History {
                account,
                window,
                since,
                csv,
            } => self.history(account, window, &since, csv)?,
```

5. At the end of `impl App<'_, '_>`, add:

```rust
    /// §13.4: reads `usage_samples` only, so it never fetches. ACCOUNT defaults to the live
    /// login's account; without `--window`, only the windows that count for switching show.
    fn history(
        &mut self,
        account: Option<String>,
        window: Option<String>,
        since: &str,
        csv: bool,
    ) -> Result<(), Failure> {
        if csv && self.json {
            return Err(Failure::Usage(CSV_AND_JSON.into()));
        }
        let span = history::parse_since(since).ok_or_else(|| Failure::Usage(BAD_SINCE.into()))?;
        let row = match &account {
            Some(a) => self.resolve(a)?,
            None => self.live_row()?,
        };
        let now_s = self.engine.now_ms() / 1000;
        let mut view =
            self.engine
                .history(&row.id, window.as_deref(), now_s.saturating_sub(span))?;
        if window.is_none() {
            history::keep_relevant(&mut view, &self.engine.settings().models);
        }
        if csv {
            let _ = write!(self.io.out, "{}", history::csv(&view));
        } else {
            self.print(
                &history::human(&view, since, now_s),
                history::json(&view, row.provider.as_str()),
            );
        }
        Ok(())
    }

    /// The live login's account, for a command whose ACCOUNT defaults to it.
    fn live_row(&self) -> Result<AccountRow, Failure> {
        match self.engine.status(&self.provider())? {
            StatusView::Managed { account, .. } => Ok(account.row),
            StatusView::Unmanaged { email } => Err(Failure::Message(
                KIND_UNMANAGED_ACCOUNT,
                format!(
                    "the live login ({email}) is not managed by tagteam; name an account, or run `tagteam add` first"
                ),
            )),
            StatusView::NoLogin => Err(Failure::Message(
                KIND_NO_LIVE_LOGIN,
                NO_LIVE_LOGIN.into(),
            )),
        }
    }
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p tagteam --features test-support --test history`
Expected: PASS, 10 tests.

Run: `cargo test -p tagteam --features test-support`
Expected: PASS. The other suites (`cli`, `app`, `kill`, `gate_race`, `cli_smoke`, and whatever
Task 14 added) are unchanged by a new subcommand.

- [ ] **Step 9: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no diff after the first command, and no warnings. `cargo fmt --all` may reflow the
long lines in the test files; that is the only change it should make.

- [ ] **Step 10: Commit**

```bash
git add crates/tagteam/src/history.rs crates/tagteam/src/lib.rs crates/tagteam/src/cli.rs crates/tagteam/src/app.rs crates/tagteam/tests/common/mod.rs crates/tagteam/tests/history.rs
git commit -m "Add the history command"
```

---

### Task 16: `statusline` and the timing tests

`tagteam statusline [--print-config]` (§13.5) prints one line for Claude Code's `statusLine`
command, built from the stored reading. It never fetches, never touches the Keychain, and
never builds the HTTP adapter. This task also adds §1.1's timing tests: `statusline` within
10 ms p95, and `list` within 50 ms p95 when nothing is due.

**Design.** The command takes a fast path in `app::run`: after the root check, before
`build_engine`, and so before the lock check and before Task 9's settings warnings. The path
runs on its own engine, `statusline::engine`, which is built with walls:
- a `NoKeychain` that refuses every call and counts them, used for the provider and for the vault
- `NoOracle`
- a `LazyHttp` whose factory builds `NoHttp`

Even a future code path that reached for the Keychain or the network could not do it. The walls
are returned alongside the engine, so an in-process test can prove nothing reached them:
`calls() == 0` and `!is_built()`. The rejected alternative was to reuse `build_engine`, which is
lazy after Task 9, and rely on tests alone. That engine holds the real Keychain and an adapter
that can send, so the guarantee would last only as long as every future path through
`Engine::statusline` stayed clean. The walls cost about 40 lines. They also keep this task
independent of how Task 9 shaped `build_engine`'s signature.

Other rulings:
- **stdin is drained in `main_with_args`, not in `run`.** In-process tests drive `run`, and a
  drain there would read the test runner's stdin. The drain reads at most 64 KiB, and never a
  terminal: someone running the command by hand would otherwise have to press Ctrl-D.
- **Colour.** `--no-color` and `NO_COLOR` turn colour off, and `FORCE_COLOR` turns it on;
  otherwise only `ui.color = never` turns it off. Unlike `list`, `auto` colours even though
  stdout is not a terminal, because the line goes to Claude Code, which renders ANSI and is
  never a terminal. The severity check uses the percentage as shown (89.6 shows as `90`, in
  red). `list`'s colours are Task 14's. This module's are its own.
- **Placeholders map by window kind**, so they are provider-neutral: `{5h}` is the Short window,
  `{7d}` the Long one, `{spend}` the Spend one, and `{model:<name>}` the Scoped window whose label
  matches, ignoring case. A missing window shows `–`. `{spend}` shows money (`€3.50 of €20`)
  when the provider's detail carries `used`, `limit` and `currency`, and otherwise `18%`.
- **`--print-config`** prints the snippet alone on stdout, so it can be redirected into a file,
  and a one-line hint that names `settings.json` on stderr.
- **`--json` is refused** (usage error, exit 2): the command prints a line of text, and
  `--json` promises one JSON object.
- **The timing tests exist only in release builds** (`not(debug_assertions)`), so Task 18's
  debug `-- --ignored` run compiles them out instead of failing them or skipping them silently.
  The standard clippy runs cannot see them, so this task lints them once in release.

**Files:**
- Create: `crates/tagteam/src/statusline.rs`, `crates/tagteam/tests/statusline.rs`,
  `crates/tagteam/tests/perf.rs`
- Modify: `crates/tagteam/src/lib.rs`, `crates/tagteam/src/cli.rs`, `crates/tagteam/src/app.rs`

**Interfaces:**
- Consumes:
  - Task 1: `tagteam_engine::settings::{Settings, ColorMode, DEFAULT_STATUSLINE_FORMAT}`,
    `Settings::load(&Env, &ProviderId) -> (Settings, Vec<String>)`, and the fields
    `statusline_format`, `color` and `models`.
  - Task 2: `tagteam_core::{Window, WindowKind}`.
  - Task 4 (tests): `tagteam_core::{PollBudget, PollPlan}`.
  - Task 5: `tagteam_core::Pace` (unit tests build views with `Pace::default()`).
  - Task 7: `Capabilities.statusline` (Claude Code sets it); `ClaudeCode`'s
    `live_identity_source`, which Task 13 uses.
  - Task 8: `Store::live_identity_cache(&ProviderId) -> Result<Option<LiveIdentityCacheRow>, _>`
    (`identity_key`, `mtime_ns`, `size`), plus `reserve_usage` and `record_usage` in tests.
  - Task 9: `tagteam_engine::lazy_http::LazyHttp::{new, is_built}`, `EngineConfig.settings`, and
    `Engine::settings()`.
  - Task 13: `Engine::statusline(&ProviderId) -> Result<StatuslineView, EngineError>`;
    `StatuslineView::{NoLogin, Unmanaged { email }, Managed { account }}`,
    `AccountView.usage: UsageView { status, windows, decision_grade, fetched_at, age_s, error,
    retry_at }`, and `UsageStatus`. The engine returns `NoLogin` without an error when
    `~/.claude.json` is missing or garbled, and never creates the store.
  - Task 14: `crate::render::duration(secs: i64) -> String` (`3d09h`, `2h40m`, `45m`, `<1m`),
    the one countdown and age formatter.
  - Task 15: `common::{now_epoch_s, two_fresh_accounts,
    usage_window, record_reading}`; the `KIND_NO_LIVE_LOGIN`, `NO_LIVE_LOGIN`, `CSV_AND_JSON`
    and `BAD_SINCE` constants and the `Command::History` arm in `app.rs`, which anchor this
    task's edits.
  - Existing: `tagteam_provider::{NoHttp, SystemClock, Keychain, KeychainError, LockState, Read,
    ReadError, Capabilities}`, `tagteam_engine::oracle::NoOracle`,
    `tagteam_engine::vault::{KeychainVault, Vault}`, `tagteam_engine::registry::ProviderRegistry`,
    `tagteam_cc::{ClaudeCode, CcPaths}`, `render::email`, and `app::{Context, fail, Failure}`.
- Produces:
  - `Command::Statusline { print_config: bool }` (the contract's variant).
  - `crate::statusline::{drain, colour, unsupported, line, config_snippet, config_hint, engine,
    NoKeychain}`: the pure rendering, and the walled engine.
  - `app::run`'s fast path, with `run_statusline` and `statusline_supported`.
  - A new CLI error kind, `unsupported`: a provider without the `statusline` capability.
  - `crates/tagteam/tests/statusline.rs` (Review Focus 5 through the binary) and
    `crates/tagteam/tests/perf.rs` (§1.1, release-only and ignored). Task 18 runs the latter with
    `cargo test --release -p tagteam --features test-support --test perf -- --ignored`.

- [ ] **Step 1: Write the failing unit tests**

In `crates/tagteam/src/lib.rs`, add `mod statusline;` after `mod root_guard;`.

Create `crates/tagteam/src/statusline.rs` holding only its tests for now:

```rust
#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use clap::Parser;
    use tagteam_cc::live::Platform;
    use tagteam_cc::{ItemKind, keychain_account, keychain_service};
    use tagteam_core::{AccountId, Pace, PollBudget, PollPlan};
    use tagteam_engine::settings::DEFAULT_STATUSLINE_FORMAT;
    use tagteam_engine::store::{AccountRow, Reserve, Store};
    use tagteam_engine::views::{UsageStatus, UsageView};
    use tagteam_provider::{FakeKeychain, KindTraits};

    use super::*;
    use crate::app::{Io, run};
    use crate::cli::Cli;
    use crate::prompt::Prompter;

    /// 2026-09-21T14:13:20Z.
    const NOW: i64 = 1_790_000_000;

    fn window(key: &str, label: &str, kind: WindowKind, pct: f64, resets_in: i64) -> Window {
        Window {
            key: key.into(),
            label: label.into(),
            kind,
            pct,
            resets_at: Some(NOW + resets_in),
            period_s: None,
            detail: None,
        }
    }

    /// 5h at 9 % resetting in 2h40m, 7d at 77 % in 3d09h, €3.50 of €20 spent (17.5 %), and
    /// Fable at 0 %.
    fn windows() -> Vec<Window> {
        let mut spend = window("spend", "spend", WindowKind::Spend, 17.5, 10 * 86_400);
        spend.detail = Some(json!({"used": 3.5, "limit": 20, "currency": "EUR"}));
        vec![
            window("5h", "5h", WindowKind::Short, 9.0, 2 * 3_600 + 40 * 60),
            window("7d", "7d", WindowKind::Long, 77.0, 3 * 86_400 + 9 * 3_600),
            spend,
            window("scoped:Fable", "Fable", WindowKind::Scoped, 0.0, 3 * 86_400 + 9 * 3_600),
        ]
    }

    /// `b@x.co` at position 2 is the live login, with `windows` read at `fetched_at`.
    fn managed(
        alias: Option<&str>,
        windows: Option<Vec<Window>>,
        fetched_at: Option<i64>,
    ) -> StatuslineView {
        StatuslineView::Managed {
            account: AccountView {
                row: AccountRow {
                    id: AccountId::from_string("0192b"),
                    provider: ProviderId::new(CLAUDE_CODE),
                    position: 2,
                    identity_key: "b@x.co\n".into(),
                    label: "b@x.co".into(),
                    email: Some("b@x.co".into()),
                    org_uuid: String::new(),
                    org_name: None,
                    account_uuid: None,
                    kind: "oauth".into(),
                    alias: alias.map(str::to_owned),
                    disabled: false,
                    identity_json: json!({}),
                    login_expires_at: None,
                    login_epoch: 0,
                    replacing_fp: None,
                    quarantine_reason: None,
                    quarantine_fp: None,
                    quarantine_at: None,
                    added_at: 1,
                },
                active: true,
                kind: KindTraits {
                    refreshable: true,
                    managed_key_axis: false,
                    default_email_prefix: None,
                    display: None,
                },
                usage: UsageView {
                    status: UsageStatus::Ok,
                    windows: windows
                        .map(|ws| ws.into_iter().map(|w| (w, Pace::default())).collect()),
                    decision_grade: true,
                    fetched_at,
                    age_s: fetched_at.map(|at| NOW - at),
                    error: None,
                    retry_at: None,
                },
            },
        }
    }

    fn plain(view: &StatuslineView, format: &str) -> String {
        line(view, format, NOW, false)
    }

    #[test]
    fn the_default_format_names_the_account_and_its_two_main_windows() {
        let v = managed(None, Some(windows()), Some(NOW));
        assert_eq!(plain(&v, DEFAULT_STATUSLINE_FORMAT), "b · 5h 9% · 7d 77%\n");
        let v = managed(Some("work"), Some(windows()), Some(NOW));
        assert_eq!(
            plain(&v, DEFAULT_STATUSLINE_FORMAT),
            "work · 5h 9% · 7d 77%\n"
        );
    }

    #[test]
    fn every_placeholder_is_filled_from_the_row_and_the_reading() {
        let v = managed(None, Some(windows()), Some(NOW));
        assert_eq!(
            plain(
                &v,
                "{position}|{email}|{5h_reset}|{7d_reset}|{spend}|{model:FABLE}"
            ),
            "2|b@x.co|2h40m|3d09h|€3.50 of €20|0\n"
        );
    }

    #[test]
    fn a_window_the_reading_lacks_shows_a_dash() {
        let v = managed(None, None, None);
        assert_eq!(
            plain(
                &v,
                "{5h}|{7d}|{5h_reset}|{7d_reset}|{spend}|{model:Fable}|{stale}"
            ),
            "–|–|–|–|–|–|\n"
        );
    }

    #[test]
    fn an_unknown_or_unclosed_placeholder_stays_as_written() {
        let v = managed(None, Some(windows()), Some(NOW));
        assert_eq!(
            plain(&v, "{nope} {model} {} {5h"),
            "{nope} {model} {} {5h\n"
        );
    }

    #[test]
    fn stale_reports_a_reading_older_than_fifteen_minutes() {
        let at = |age: i64| plain(&managed(None, Some(windows()), Some(NOW - age)), "{stale}");
        assert_eq!(at(15 * 60), "\n");
        assert_eq!(at(15 * 60 + 1), " · 15m old\n");
        assert_eq!(at(3 * 3_600 + 5 * 60), " · 3h05m old\n");
    }

    #[test]
    fn percentages_are_coloured_by_severity_as_shown() {
        let mut ws = windows();
        ws[0].pct = 95.0;
        ws[1].pct = 70.0;
        ws[3].pct = 89.6;
        let v = managed(None, Some(ws.clone()), Some(NOW));
        let format = "{5h} {7d} {model:Fable} {spend}";
        assert_eq!(
            line(&v, format, NOW, true),
            "\u{1b}[31m95\u{1b}[0m \u{1b}[33m70\u{1b}[0m \u{1b}[31m90\u{1b}[0m €3.50 of €20\n"
        );
        assert_eq!(line(&v, format, NOW, false), "95 70 90 €3.50 of €20\n");
        ws[1].pct = 69.4;
        let v = managed(None, Some(ws), Some(NOW));
        assert_eq!(line(&v, "{7d}", NOW, true), "69\n");
    }

    #[test]
    fn spend_is_money_when_the_provider_says_how_much() {
        assert_eq!(money(0.0, "eur"), "€0");
        assert_eq!(money(20.0, "USD"), "$20");
        assert_eq!(money(3.5, "GBP"), "£3.50");
        assert_eq!(money(12.0, "CHF"), "CHF 12");
        let mut ws = windows();
        ws[2].detail = None;
        assert_eq!(
            plain(&managed(None, Some(ws), Some(NOW)), "{spend}"),
            "18%\n"
        );
    }

    #[test]
    fn an_unmanaged_login_is_its_email_and_no_login_is_nothing() {
        let unmanaged = StatuslineView::Unmanaged {
            email: "c@x.co".into(),
        };
        assert_eq!(
            line(&unmanaged, DEFAULT_STATUSLINE_FORMAT, NOW, true),
            "c@x.co\n"
        );
        assert_eq!(
            line(&StatuslineView::NoLogin, DEFAULT_STATUSLINE_FORMAT, NOW, true),
            ""
        );
    }

    #[test]
    fn colour_follows_the_flag_the_environment_then_the_setting() {
        assert!(colour(false, false, false, ColorMode::Auto), "auto colours");
        assert!(colour(false, false, false, ColorMode::Always));
        assert!(!colour(false, false, false, ColorMode::Never));
        assert!(!colour(true, false, true, ColorMode::Always), "--no-color wins");
        assert!(!colour(false, true, true, ColorMode::Always), "NO_COLOR wins");
        assert!(colour(false, false, true, ColorMode::Never), "FORCE_COLOR outranks never");
    }

    /// A reader that fails the test if it is ever read.
    struct Untouchable;

    impl std::io::Read for Untouchable {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            panic!("a terminal is never read");
        }
    }

    #[test]
    fn stdin_is_drained_up_to_64_kib_and_a_terminal_is_never_read() {
        assert_eq!(
            drain(std::io::Cursor::new(vec![b'x'; 100 * 1024]), false),
            64 * 1024
        );
        assert_eq!(drain(std::io::Cursor::new(b"{}".to_vec()), false), 2);
        assert_eq!(drain(Untouchable, true), 0);
    }

    #[test]
    fn print_config_is_claude_codes_settings_snippet() {
        assert_eq!(
            config_snippet(),
            "{\n  \"statusLine\": {\n    \"type\": \"command\",\n    \"command\": \"tagteam statusline\"\n  }\n}\n"
        );
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            config_hint(&Env::for_test(dir.path())),
            format!(
                "Add this to {}; tagteam never edits Claude Code's settings.",
                dir.path().join("home/.claude/settings.json").display()
            )
        );
    }

    #[test]
    fn a_provider_without_the_capability_is_refused() {
        assert_eq!(
            unsupported(Capabilities::default(), "Fake Agent").as_deref(),
            Some("Fake Agent has no statusline")
        );
        let capable = Capabilities {
            statusline: true,
            ..Capabilities::default()
        };
        assert_eq!(unsupported(capable, "Claude Code"), None);
    }

    #[test]
    fn the_no_keychain_refuses_and_counts_every_call() {
        let k = NoKeychain::default();
        assert!(matches!(k.find("s", "a"), Read::Unreadable(_)));
        assert!(matches!(k.exists("s", "a"), Read::Unreadable(_)));
        assert!(k.upsert("s", "a", b"x").is_err());
        assert!(k.delete("s", "a").is_err());
        assert_eq!(k.lock_state(), LockState::Unknown);
        assert!(!k.unlock());
        assert_eq!(k.calls(), 6);
    }

    /// Answers no prompt: the `add` below asks none.
    struct NoPrompts;

    impl Prompter for NoPrompts {
        fn interactive(&self) -> bool {
            false
        }
        fn confirm(&mut self, question: &str, _default_yes: bool) -> bool {
            panic!("unexpected prompt: {question}");
        }
        fn choose(&mut self, question: &str, _options: &[String]) -> Option<usize> {
            panic!("unexpected prompt: {question}");
        }
        fn secret(&mut self, question: &str) -> Option<String> {
            panic!("unexpected prompt: {question}");
        }
    }

    /// `a@x.co` logged in and added through `run`, with one reading recorded now: 5h at 9 %,
    /// 7d at 77 %.
    fn managed_home() -> (tempfile::TempDir, Env) {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        std::fs::create_dir_all(env.home.join(".claude")).unwrap();
        let config = json!({"oauthAccount": {"emailAddress": "a@x.co", "organizationUuid": "", "accountUuid": "uuid-a"}});
        std::fs::write(env.home.join(".claude.json"), config.to_string()).unwrap();
        let kc = Arc::new(FakeKeychain::new());
        let credential = json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": "rt-a", "refreshTokenExpiresAt": 1_797_000_000_000i64}});
        kc.put(
            &keychain_service(&env, ItemKind::OAuth),
            &keychain_account(&env),
            credential.to_string().as_bytes(),
        );
        let ctx = Context {
            env: env.clone(),
            keychain: kc,
            platform: Platform::MacOs,
            api_base: Some("http://127.0.0.1:9".into()),
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(
            Cli::try_parse_from(["tagteam", "add"]).unwrap(),
            ctx,
            &mut Io {
                out: &mut out,
                err: &mut err,
                prompter: &mut NoPrompts,
            },
        );
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
        let store = Store::open_existing(&env.data_dir().join("tagteam.db"))
            .unwrap()
            .unwrap();
        let row = store
            .accounts(&ProviderId::new(CLAUDE_CODE))
            .unwrap()
            .remove(0);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let Reserve::Reserved(reservation) = store
            .reserve_usage(&row, now * 1000, true, &PollBudget::STANDARD)
            .unwrap()
        else {
            panic!("no reservation for the reading");
        };
        let reading = [
            Window {
                key: "5h".into(),
                label: "5h".into(),
                kind: WindowKind::Short,
                pct: 9.0,
                resets_at: None,
                period_s: None,
                detail: None,
            },
            Window {
                key: "7d".into(),
                label: "7d".into(),
                kind: WindowKind::Long,
                pct: 77.0,
                resets_at: None,
                period_s: None,
                detail: None,
            },
        ];
        let plan = PollPlan {
            interval_s: 300,
            next_poll_at: now + 300,
        };
        assert!(
            store
                .record_usage(&reservation, &reading, now, &plan, 180)
                .unwrap()
        );
        (dir, env)
    }

    #[test]
    fn the_engine_reaches_neither_the_keychain_nor_the_network() {
        // §13.5: the walls count what reaches them, so none of this may.
        let (_dir, env) = managed_home();
        let provider = ProviderId::new(CLAUDE_CODE);
        let ctx = Context {
            env,
            keychain: Arc::new(FakeKeychain::new()),
            platform: Platform::MacOs,
            api_base: None,
        };
        let (built, http, keychain) = engine(ctx, &provider);
        let view = built.statusline(&provider).unwrap();
        let text = line(
            &view,
            &built.settings().statusline_format,
            built.now_ms() / 1000,
            false,
        );
        assert_eq!(text, "a · 5h 9% · 7d 77%\n");
        assert!(!http.is_built(), "the HTTP adapter was built");
        assert_eq!(keychain.calls(), 0, "the Keychain was asked");
    }
}
```

- [ ] **Step 2: Run the unit tests to verify they fail**

Run: `cargo test -p tagteam --lib -- statusline::`
Expected: FAIL to compile. error E0425, "cannot find function line in this scope", and likewise for
`colour`, `drain`, `money`, `config_snippet`, `config_hint`, `unsupported` and `engine`, plus
error E0412, "cannot find type NoKeychain". The names the tests take from `super::*` (`json`, `Arc`,
`Env`, `ProviderId`, `CLAUDE_CODE`, `Window`, `WindowKind`, `AccountView`, `StatuslineView`,
`ColorMode`, `Capabilities`, `LockState`, `Read`, `Context`) are unresolved too, because the
module has no imports yet.

- [ ] **Step 3: Implement the rendering and the walled engine**

In `crates/tagteam/src/statusline.rs`, above the test module, add:

```rust
//! `tagteam statusline` (§13.5): one line for Claude Code's status bar, from the stored
//! reading. It never fetches, never touches the Keychain, and never builds the HTTP adapter.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use tagteam_cc::{CcPaths, ClaudeCode};
use tagteam_core::{CLAUDE_CODE, ProviderId, Window, WindowKind};
use tagteam_engine::lazy_http::LazyHttp;
use tagteam_engine::oracle::NoOracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::{ColorMode, Settings};
use tagteam_engine::vault::{KeychainVault, Vault};
use tagteam_engine::views::{AccountView, StatuslineView};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_provider::{
    Capabilities, Env, Http, Keychain, KeychainError, LockState, NoHttp, Read, ReadError,
    SystemClock,
};

use crate::app::Context;
use crate::render;

/// §13.5: at most this much piped stdin is read, and none of it is used.
const STDIN_CAP: u64 = 64 * 1024;
/// A reading older than this says how old it is (`{stale}`).
const STALE_AFTER_S: i64 = 15 * 60;
/// §13.5's severities, on the percentage as shown.
const CRITICAL_PCT: f64 = 90.0;
const WARNING_PCT: f64 = 70.0;
const RED: &str = "\u{1b}[31m";
const YELLOW: &str = "\u{1b}[33m";
const RESET: &str = "\u{1b}[0m";
/// What a placeholder shows when the reading has no such window.
const MISSING: &str = "–";
const REFUSED: &str = "the statusline never uses the Keychain";
/// What Claude Code's `statusLine` setting runs.
const COMMAND: &str = "tagteam statusline";

/// Reads and discards up to `STDIN_CAP` bytes, so Claude Code's write of its session JSON never
/// blocks. A terminal is never read: someone running the command by hand would otherwise have
/// to press Ctrl-D. Returns how much was read.
pub(crate) fn drain(input: impl std::io::Read, terminal: bool) -> u64 {
    if terminal {
        return 0;
    }
    std::io::copy(&mut input.take(STDIN_CAP), &mut std::io::sink()).unwrap_or(0)
}

/// Whether the line is coloured. `--no-color` and `NO_COLOR` switch colour off, and
/// `FORCE_COLOR` switches it on. Otherwise only `ui.color = never` switches it off: the line
/// goes to Claude Code, which renders ANSI colour but is never a terminal, so `auto`'s terminal
/// test would turn colour off exactly where it is shown.
pub(crate) fn colour(
    no_color_flag: bool,
    no_color_env: bool,
    force_color_env: bool,
    setting: ColorMode,
) -> bool {
    if no_color_flag || no_color_env {
        return false;
    }
    force_color_env || setting != ColorMode::Never
}

/// §4.5: the command refuses for a provider without the `statusline` capability.
pub(crate) fn unsupported(caps: Capabilities, provider_name: &str) -> Option<String> {
    (!caps.statusline).then(|| format!("{provider_name} has no statusline"))
}

/// §13.5: the managed live login's line in `format`, an unmanaged login's email alone, or
/// nothing without a live login. `now_s` dates the countdowns and `{stale}`.
pub(crate) fn line(view: &StatuslineView, format: &str, now_s: i64, colour: bool) -> String {
    match view {
        StatuslineView::NoLogin => String::new(),
        StatuslineView::Unmanaged { email } => format!("{email}\n"),
        StatuslineView::Managed { account } => {
            format!("{}\n", expand(format, account, now_s, colour))
        }
    }
}

/// Fills each known `{placeholder}`. An unknown or unclosed one stays as written.
fn expand(format: &str, account: &AccountView, now_s: i64, colour: bool) -> String {
    let windows: Vec<&Window> = account
        .usage
        .windows
        .iter()
        .flatten()
        .map(|(w, _)| w)
        .collect();
    let mut out = String::with_capacity(format.len());
    let mut rest = format;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|c| open + c) else {
            break;
        };
        out.push_str(&rest[..open]);
        match placeholder(&rest[open + 1..close], account, &windows, now_s, colour) {
            Some(text) => out.push_str(&text),
            None => out.push_str(&rest[open..=close]),
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// One placeholder's text, or `None` for a name §13.5 does not define. The window placeholders
/// go by kind, so they mean the same for every provider.
fn placeholder(
    name: &str,
    account: &AccountView,
    windows: &[&Window],
    now_s: i64,
    colour: bool,
) -> Option<String> {
    let row = &account.row;
    let of_kind = |kind: WindowKind| windows.iter().copied().find(|w| w.kind == kind);
    let text = match name {
        "account" => row
            .alias
            .clone()
            .unwrap_or_else(|| local_part(&render::email(row)).to_owned()),
        "position" => row.position.to_string(),
        "email" => render::email(row),
        "5h" => pct(of_kind(WindowKind::Short), colour),
        "7d" => pct(of_kind(WindowKind::Long), colour),
        "5h_reset" => reset(of_kind(WindowKind::Short), now_s),
        "7d_reset" => reset(of_kind(WindowKind::Long), now_s),
        "spend" => spend(of_kind(WindowKind::Spend), colour),
        "stale" => stale(account.usage.fetched_at, now_s),
        _ => {
            let model = name.strip_prefix("model:")?;
            let scoped = windows
                .iter()
                .copied()
                .find(|w| w.kind == WindowKind::Scoped && w.label.eq_ignore_ascii_case(model));
            pct(scoped, colour)
        }
    };
    Some(text)
}

/// The part of an email before `@`; a label without one is itself.
fn local_part(email: &str) -> &str {
    email.split_once('@').map_or(email, |(local, _)| local)
}

/// A window's percentage as shown, whole, coloured by the severity of what is shown.
fn pct(w: Option<&Window>, colour: bool) -> String {
    w.map_or_else(
        || MISSING.to_owned(),
        |w| {
            let shown = w.pct.round();
            paint(&format!("{shown:.0}"), shown, colour)
        },
    )
}

fn paint(text: &str, shown_pct: f64, colour: bool) -> String {
    let code = if !colour {
        None
    } else if shown_pct >= CRITICAL_PCT {
        Some(RED)
    } else if shown_pct >= WARNING_PCT {
        Some(YELLOW)
    } else {
        None
    };
    match code {
        Some(c) => format!("{c}{text}{RESET}"),
        None => text.to_owned(),
    }
}

fn reset(w: Option<&Window>, now_s: i64) -> String {
    w.and_then(|w| w.resets_at)
        .map_or_else(|| MISSING.to_owned(), |at| render::duration(at - now_s))
}

/// The spend window as money (`€3.50 of €20`) when the provider's detail says how much, else
/// as a percentage.
fn spend(w: Option<&Window>, colour: bool) -> String {
    let Some(w) = w else {
        return MISSING.to_owned();
    };
    let shown = w.pct.round();
    let detail = w.detail.as_ref();
    let used = detail.and_then(|d| d.get("used")).and_then(Value::as_f64);
    let limit = detail.and_then(|d| d.get("limit")).and_then(Value::as_f64);
    let currency = detail.and_then(|d| d.get("currency")).and_then(Value::as_str);
    let text = match (used, limit, currency) {
        (Some(used), Some(limit), Some(currency)) => {
            format!("{} of {}", money(used, currency), money(limit, currency))
        }
        _ => format!("{shown:.0}%"),
    };
    paint(&text, shown, colour)
}

/// `€20`, `$3.50`, `£0`; any other currency by its code (`CHF 12`).
fn money(amount: f64, currency: &str) -> String {
    let n = if amount.fract() == 0.0 {
        format!("{amount:.0}")
    } else {
        format!("{amount:.2}")
    };
    match currency.to_ascii_uppercase().as_str() {
        "EUR" => format!("€{n}"),
        "USD" => format!("${n}"),
        "GBP" => format!("£{n}"),
        code => format!("{code} {n}"),
    }
}

fn stale(fetched_at: Option<i64>, now_s: i64) -> String {
    match fetched_at {
        Some(at) if now_s - at > STALE_AFTER_S => format!(" · {} old", render::duration(now_s - at)),
        _ => String::new(),
    }
}

/// `--print-config`'s snippet for Claude Code's `settings.json`. tagteam never edits that file
/// (§3); the user pastes this into it.
pub(crate) fn config_snippet() -> String {
    let v = json!({"statusLine": {"type": "command", "command": COMMAND}});
    format!(
        "{}\n",
        serde_json::to_string_pretty(&v).expect("a JSON value always serializes")
    )
}

/// Where the snippet goes. It is printed on stderr, so stdout stays the snippet alone.
pub(crate) fn config_hint(env: &Env) -> String {
    let path = CcPaths::resolve(env).config_home.join("settings.json");
    format!(
        "Add this to {}; tagteam never edits Claude Code's settings.",
        path.display()
    )
}

/// The engine the fast path runs on, built with walls rather than trust (§13.5): a Keychain that
/// refuses every call, no profile oracle, and a lazy HTTP port whose adapter could send nothing
/// even if it were built. The settings' warnings are dropped, since a status bar has nowhere to
/// show them. The walls are returned so a test can prove nothing reached them.
pub(crate) fn engine(
    ctx: Context,
    provider: &ProviderId,
) -> (Engine, Arc<LazyHttp>, Arc<NoKeychain>) {
    let keychain = Arc::new(NoKeychain::default());
    let http = Arc::new(LazyHttp::new(|| Arc::new(NoHttp) as Arc<dyn Http>));
    let (settings, _warnings) = Settings::load(&ctx.env, provider);
    let engine = Engine::new(EngineConfig {
        registry: ProviderRegistry::new().with(Arc::new(ClaudeCode::new(
            keychain.clone(),
            ctx.platform,
        ))),
        vault: Vault::new(Box::new(KeychainVault::new(keychain.clone()))),
        oracle: Arc::new(NoOracle),
        clock: Arc::new(SystemClock),
        http: http.clone(),
        default_provider: ProviderId::new(CLAUDE_CODE),
        settings,
        env: ctx.env,
    });
    (engine, http, keychain)
}

/// The statusline engine's Keychain: every call is refused, and counted.
#[derive(Default)]
pub(crate) struct NoKeychain {
    calls: AtomicUsize,
}

impl NoKeychain {
    fn refuse(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Keychain for NoKeychain {
    fn find(&self, _service: &str, _account: &str) -> Read<Vec<u8>> {
        self.refuse();
        Read::Unreadable(ReadError::new("keychain", REFUSED))
    }

    fn exists(&self, _service: &str, _account: &str) -> Read<()> {
        self.refuse();
        Read::Unreadable(ReadError::new("keychain", REFUSED))
    }

    fn upsert(&self, _service: &str, _account: &str, _data: &[u8]) -> Result<(), KeychainError> {
        self.refuse();
        Err(KeychainError {
            rc: None,
            detail: REFUSED.into(),
        })
    }

    fn delete(&self, _service: &str, _account: &str) -> Result<(), KeychainError> {
        self.refuse();
        Err(KeychainError {
            rc: None,
            detail: REFUSED.into(),
        })
    }

    fn lock_state(&self) -> LockState {
        self.refuse();
        LockState::Unknown
    }

    fn unlock(&self) -> bool {
        self.refuse();
        false
    }
}
```

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: `cargo test -p tagteam --lib -- statusline::`
Expected: PASS, 14 tests. Among them are
`the_engine_reaches_neither_the_keychain_nor_the_network`,
`percentages_are_coloured_by_severity_as_shown` and
`stdin_is_drained_up_to_64_kib_and_a_terminal_is_never_read`.

- [ ] **Step 5: Write the failing binary tests**

Create `crates/tagteam/tests/statusline.rs`:

```rust
//! `tagteam statusline` through the binary (§13.5), with Review Focus 5. Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{
    cmd, login, now_epoch_s, record_reading, seed_home, two_fresh_accounts, usage_window,
};
use serde_json::{Value, json};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId, WindowKind};
use tagteam_engine::store::Store;
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{Env, FileKeychain};

const HOUR: i64 = 3_600;
const WEEK: i64 = 604_800;
const YELLOW_77: &str = "\u{1b}[33m77\u{1b}[0m";

/// `a` at position 1 and `b` at position 2 (live), with one reading of `b` taken `age_s` ago:
/// 5h at 9 % resetting in 2h40m, 7d at 77 % in 3d09h, and Fable at 0 %. Each reset is 30 s past
/// its minute, so a countdown read seconds later prints the same text.
fn managed(age_s: i64) -> (tempfile::TempDir, String, String) {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = two_fresh_accounts(d.path());
    let now = now_epoch_s();
    let (r5, r7) = (now + 2 * HOUR + 40 * 60 + 30, now + 3 * 86_400 + 9 * HOUR + 30);
    record_reading(
        d.path(),
        &b,
        now - age_s,
        &[
            usage_window("5h", "5h", WindowKind::Short, 9.0, Some(r5), Some(5 * HOUR)),
            usage_window("7d", "7d", WindowKind::Long, 77.0, Some(r7), Some(WEEK)),
            usage_window("scoped:Fable", "Fable", WindowKind::Scoped, 0.0, Some(r7), Some(WEEK)),
        ],
    );
    (d, a, b)
}

fn statusline(root: &Path) -> assert_cmd::Command {
    let mut c = cmd(root);
    c.arg("statusline");
    c
}

fn write_config(root: &Path, text: &str) {
    let dir = Env::for_test(root).config_dir();
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), text).unwrap();
}

fn store(root: &Path) -> Store {
    Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap()
}

/// `(mtime_ns, size)`, the pair that keys `live_identity_cache` (§13.5).
fn stat(path: &Path) -> (i64, i64) {
    let m = fs::metadata(path).unwrap();
    let mtime = m.modified().unwrap().duration_since(UNIX_EPOCH).unwrap();
    (mtime.as_nanos() as i64, m.len() as i64)
}

fn set_mtime(path: &Path, at: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(at)
        .unwrap();
}

/// Moves the mtime two seconds on, so a rewrite is visible even on a filesystem whose clock is
/// coarser than the test.
fn bump_mtime(path: &Path) {
    let at = fs::metadata(path).unwrap().modified().unwrap();
    set_mtime(path, at + Duration::from_secs(2));
}

#[test]
fn a_managed_login_prints_its_line_from_the_stored_reading() {
    let (d, _, _) = managed(0);
    statusline(d.path())
        .assert()
        .success()
        .stdout(format!("b · 5h 9% · 7d {YELLOW_77}%\n"))
        .stderr("");
    statusline(d.path())
        .arg("--no-color")
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77%\n");
    statusline(d.path())
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77%\n");
}

#[test]
fn the_format_and_colour_come_from_config_toml() {
    let (d, _, _) = managed(0);
    write_config(
        d.path(),
        "[statusline]\nformat = \"{position} {email} {5h_reset} {7d_reset} {model:fable} {spend} {7d} {nope} {5h\"\n\n[ui]\ncolor = \"never\"\n",
    );
    statusline(d.path())
        .assert()
        .success()
        .stdout("2 b@x.co 2h40m 3d09h 0 – 77 {nope} {5h\n");
    statusline(d.path())
        .env("FORCE_COLOR", "1")
        .assert()
        .success()
        .stdout(format!(
            "2 b@x.co 2h40m 3d09h 0 – {YELLOW_77} {{nope}} {{5h\n"
        ));
}

#[test]
fn a_stale_reading_says_how_old_it_is() {
    let (d, _, _) = managed(20 * 60);
    statusline(d.path())
        .arg("--no-color")
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77% · 20m old\n");
    let (d, _, _) = managed(14 * 60);
    statusline(d.path())
        .arg("--no-color")
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77%\n");
}

#[test]
fn an_unmanaged_login_prints_its_email_and_no_login_prints_nothing() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    fs::create_dir_all(&env.home).unwrap();
    // No `~/.claude.json` at all.
    statusline(d.path()).assert().success().stdout("").stderr("");
    seed_home(&env);
    // Claude Code has run, but nobody is logged in.
    statusline(d.path()).assert().success().stdout("").stderr("");
    login(&env, &FileKeychain::new(d.path().join("keychain")), "c@x.co", "", "rt-c");
    statusline(d.path())
        .assert()
        .success()
        .stdout("c@x.co\n")
        .stderr("");
    assert!(!env.data_dir().exists(), "statusline never creates the store");
}

#[test]
fn claude_json_missing_garbled_or_rewritten() {
    // Review Focus 5.
    let (d, a, b) = managed(0);
    let root = d.path();
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    let path = env.home.join(".claude.json");
    let shows = |expected: &str| {
        statusline(root)
            .arg("--no-color")
            .assert()
            .success()
            .stdout(expected.to_owned())
            .stderr("");
    };
    let key_of = |id: &str| {
        store(root)
            .account(&AccountId::from_string(id))
            .unwrap()
            .unwrap()
            .identity_key
    };
    let cached = || {
        store(root)
            .live_identity_cache(&ProviderId::new(CLAUDE_CODE))
            .unwrap()
            .unwrap()
    };
    shows("b · 5h 9% · 7d 77%\n");
    assert_eq!(cached().identity_key, Some(key_of(&b)));

    // Rewritten between two runs: the new mtime makes the next run re-parse, and re-key the
    // cache to the file as it is now.
    login(&env, &kc, "a@x.co", "", "rt-a");
    bump_mtime(&path);
    shows("a · 5h –% · 7d –%\n");
    let row = cached();
    assert_eq!(row.identity_key, Some(key_of(&a)));
    assert_eq!((row.mtime_ns, row.size), stat(&path));

    // The cache is what keeps the line within budget: a rewrite that keeps both the size and
    // the mtime is not re-parsed, and any change of mtime is.
    let (mtime, size) = (fs::metadata(&path).unwrap().modified().unwrap(), stat(&path).1);
    login(&env, &kc, "b@x.co", "", "rt-b");
    assert_eq!(stat(&path).1, size, "a@x.co and b@x.co splice to the same size");
    set_mtime(&path, mtime);
    shows("a · 5h –% · 7d –%\n");
    bump_mtime(&path);
    shows("b · 5h 9% · 7d 77%\n");

    // Garbled: nothing, and still success. Missing: nothing.
    fs::write(&path, "{ \"oauthAccount\": ").unwrap();
    shows("");
    fs::remove_file(&path).unwrap();
    shows("");
}

#[test]
fn no_network_no_lock_check_and_no_settings_warnings() {
    // Nothing is recorded, so both accounts are due for a fetch; the Keychain is locked, so a
    // lock check would refuse; and the settings file is corrupt, so any other command warns.
    let d = tempfile::tempdir().unwrap();
    two_fresh_accounts(d.path());
    fs::write(d.path().join("keychain/LOCKED"), "").unwrap();
    write_config(d.path(), "this is [not toml\n");
    let server = MockServer::start();
    server.on(
        "GET",
        "/api/oauth/usage",
        MockReply::Json {
            status: 200,
            body: json!({}),
        },
    );
    cmd(d.path())
        .env("TAGTEAM_TEST_API_BASE", server.base_url())
        .args(["statusline", "--no-color"])
        .assert()
        .success()
        .stdout("b · 5h –% · 7d –%\n")
        .stderr("");
    assert_eq!(server.requests().len(), 0, "statusline sent a request");
}

#[test]
fn a_large_stdin_is_drained_and_ignored() {
    let (d, _, _) = managed(0);
    statusline(d.path())
        .arg("--no-color")
        .write_stdin(vec![b'x'; 100 * 1024])
        .timeout(Duration::from_secs(10))
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77%\n");
    statusline(d.path())
        .arg("--no-color")
        .write_stdin(r#"{"session_id":"s","model":{"display_name":"Fable"}}"#)
        .timeout(Duration::from_secs(10))
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77%\n");
}

#[test]
fn print_config_prints_the_snippet_and_json_is_refused() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    statusline(d.path())
        .arg("--print-config")
        .assert()
        .success()
        .stdout("{\n  \"statusLine\": {\n    \"type\": \"command\",\n    \"command\": \"tagteam statusline\"\n  }\n}\n")
        .stderr(format!(
            "Add this to {}; tagteam never edits Claude Code's settings.\n",
            d.path().join("home/.claude/settings.json").display()
        ));
    let cases: [&[&str]; 2] = [
        &["statusline", "--json"],
        &["statusline", "--print-config", "--json"],
    ];
    for args in cases {
        let out = cmd(d.path())
            .args(args)
            .assert()
            .code(2)
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            json!({"schemaVersion": 1, "error": {"type": "usage",
                   "message": "statusline prints a line of text; run it without --json"}}),
            "{args:?}"
        );
    }
}

#[test]
fn an_unknown_provider_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    statusline(d.path())
        .args(["--provider", "nope"])
        .assert()
        .code(1)
        .stdout("")
        .stderr("tagteam: unknown provider \"nope\"\n");
}
```

- [ ] **Step 6: Run the binary tests to verify they fail**

Run: `cargo test -p tagteam --features test-support --test statusline`
Expected: FAIL, all 9 tests. `statusline` is not a subcommand yet, so every run exits 2 with
clap's `unrecognized subcommand 'statusline'`.

- [ ] **Step 7: Add the command and its fast path**

In `crates/tagteam/src/cli.rs`, add this variant after the `History { … }` variant (Task 15):

```rust
    /// One line for Claude Code's status bar
    Statusline {
        /// Print the settings.json snippet that sets it up
        #[arg(long = "print-config")]
        print_config: bool,
    },
```

In `crates/tagteam/src/lib.rs`, change `use std::io::Write;` to
`use std::io::{IsTerminal, Write};`, then directly before `let mut prompter = prompt::TtyPrompter;`
add:

```rust
    // §13.5: Claude Code pipes its session JSON into the status bar command. It is drained
    // here, at the process boundary, rather than in `app::run`: in-process tests drive `run`,
    // and must never read the test runner's stdin.
    if matches!(cli.command, Some(cli::Command::Statusline { .. })) {
        let stdin = std::io::stdin();
        statusline::drain(stdin.lock(), stdin.is_terminal());
    }
```

In `crates/tagteam/src/app.rs`:

1. Directly after `const KIND_NO_LIVE_LOGIN: &str = "no-live-login";` (Task 15) add:

```rust
/// A provider without the capability a command needs (§4.5).
const KIND_UNSUPPORTED: &str = "unsupported";
```

2. Directly after `const BAD_SINCE: &str = "--since takes a span like 14d, 12h or 30m";`
   (Task 15) add:

```rust
const STATUSLINE_UNDER_JSON: &str = "statusline prints a line of text; run it without --json";
```

3. Add `statusline` to the `use crate::{…}` import
   (`use crate::{history, render, root_guard, statusline};`).

4. In `run`, directly after:

```rust
    if let Err(msg) = root_guard::refuse_root() {
        return fail(io, json, KIND_ROOT, &msg);
    }
```

add:

```rust
    // §13.5: the status bar's fast path, before anything else is built.
    if let Some(Command::Statusline { print_config }) = &cli.command {
        return run_statusline(ctx, io, json, cli.no_color, cli.provider, *print_config);
    }
```

5. Directly after the `fail` function, add:

```rust
/// §13.5's fast path, taken before `build_engine`: no lock check, no settings warnings (a status
/// bar has nowhere to show them), and an engine walled off from the Keychain and the network
/// (`statusline::engine`). `main_with_args` has already drained stdin.
fn run_statusline(
    ctx: Context,
    io: &mut Io<'_>,
    json: bool,
    no_color: bool,
    provider: Option<String>,
    print_config: bool,
) -> i32 {
    if json {
        fail(io, true, KIND_USAGE, STATUSLINE_UNDER_JSON);
        return EXIT_USAGE;
    }
    let provider = provider.map_or_else(|| ProviderId::new(CLAUDE_CODE), ProviderId::new);
    let (engine, _http, _keychain) = statusline::engine(ctx, &provider);
    let result = statusline_supported(&engine, &provider).and_then(|()| {
        if print_config {
            let _ = writeln!(io.err, "{}", statusline::config_hint(engine.env()));
            return Ok(statusline::config_snippet());
        }
        let view = engine.statusline(&provider)?;
        let settings = engine.settings();
        let colour = statusline::colour(
            no_color,
            std::env::var_os("NO_COLOR").is_some(),
            std::env::var_os("FORCE_COLOR").is_some(),
            settings.color,
        );
        Ok(statusline::line(
            &view,
            &settings.statusline_format,
            engine.now_ms() / 1000,
            colour,
        ))
    });
    match result {
        Ok(text) => {
            let _ = write!(io.out, "{text}");
            0
        }
        Err(Failure::Engine(e)) => fail(io, false, e.kind(), &e.to_string()),
        Err(Failure::Message(kind, m)) => fail(io, false, kind, &m),
        Err(Failure::Usage(m)) => {
            fail(io, false, KIND_USAGE, &m);
            EXIT_USAGE
        }
    }
}

/// §4.5: `statusline` refuses for a provider without the capability.
fn statusline_supported(engine: &Engine, provider: &ProviderId) -> Result<(), Failure> {
    let p = engine.provider(provider)?;
    match statusline::unsupported(p.capabilities(), p.display_name()) {
        Some(message) => Err(Failure::Message(KIND_UNSUPPORTED, message)),
        None => Ok(()),
    }
}
```

6. In `dispatch`, after the `Command::History { … } => self.history(account, window, &since, csv)?,`
   arm (Task 15), add:

```rust
            Command::Statusline { .. } => unreachable!("run answers statusline before dispatch"),
```

`touches_keychain` stays as it is: `statusline` never reaches `lock_check`.

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p tagteam --features test-support --test statusline`
Expected: PASS, 9 tests. The locked Keychain in `no_network_no_lock_check_and_no_settings_warnings`
shows through the binary that no lock check ran. The stronger claims, that nothing reaches the
Keychain at all and that the HTTP adapter is never built, are proved in-process by Step 4's
`the_engine_reaches_neither_the_keychain_nor_the_network`: its walls count every call.

Run: `cargo test -p tagteam --features test-support && cargo test -p tagteam --lib`
Expected: PASS. The second run has no features, so it also covers the release branch of the
test-override checks.

- [ ] **Step 9: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no diff after the first command, and no warnings.

- [ ] **Step 10: Commit**

```bash
git add crates/tagteam/src/statusline.rs crates/tagteam/src/lib.rs crates/tagteam/src/cli.rs crates/tagteam/src/app.rs crates/tagteam/tests/statusline.rs
git commit -m "Add the statusline command on a Keychain-free, network-free fast path"
```

- [ ] **Step 11: Write the timing tests**

Create `crates/tagteam/tests/perf.rs`:

```rust
//! §1.1's timing targets through the real binary: `statusline` within 10 ms p95, and `list`
//! within 50 ms p95 when no usage fetch is due. Timings mean something only in an optimized
//! build on an idle machine, so this file exists only in release builds and its tests are
//! ignored by default:
//! `cargo test --release -p tagteam --features test-support --test perf -- --ignored`
//! (add `--nocapture` to see each measured p95).
#![cfg(all(feature = "test-support", not(debug_assertions)))]

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{now_epoch_s, record_reading, std_cmd, two_fresh_accounts, usage_window};
use tagteam_core::WindowKind;
use tagteam_provider::mock_server::MockServer;

const RUNS: usize = 50;
const WARM_UP: usize = 3;

/// Two accounts, `b` live, each with a reading taken just now. Nothing is due for 180 s (§8.3's
/// on-demand rule), far longer than a timing run takes.
fn nothing_due(root: &Path) {
    let (a, b) = two_fresh_accounts(root);
    let now = now_epoch_s();
    for id in [&a, &b] {
        record_reading(
            root,
            id,
            now,
            &[
                usage_window("5h", "5h", WindowKind::Short, 9.0, Some(now + 9_600), Some(18_000)),
                usage_window("7d", "7d", WindowKind::Long, 77.0, Some(now + 300_000), Some(604_800)),
            ],
        );
    }
}

/// The wall time of each of `RUNS` runs of `args`, after `WARM_UP` untimed ones, with every
/// endpoint pointed at `base`.
fn timings(root: &Path, base: &str, args: &[&str]) -> Vec<Duration> {
    let run = || {
        let started = Instant::now();
        let out = std_cmd(root)
            .env("TAGTEAM_TEST_API_BASE", base)
            .args(args)
            .output()
            .unwrap();
        let took = started.elapsed();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        took
    };
    for _ in 0..WARM_UP {
        run();
    }
    (0..RUNS).map(|_| run()).collect()
}

/// The 95th percentile, by nearest rank.
fn p95(runs: &[Duration]) -> Duration {
    let mut sorted = runs.to_vec();
    sorted.sort();
    sorted[(sorted.len() * 95).div_ceil(100) - 1]
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn statusline_p95_is_within_10_ms() {
    let d = tempfile::tempdir().unwrap();
    nothing_due(d.path());
    let server = MockServer::start();
    let runs = timings(d.path(), &server.base_url(), &["statusline"]);
    let p = p95(&runs);
    eprintln!("statusline p95 {p:?} over {RUNS} runs");
    assert_eq!(server.requests().len(), 0, "statusline sent a request");
    assert!(
        p <= Duration::from_millis(10),
        "statusline p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn list_p95_is_within_50_ms_when_nothing_is_due() {
    let d = tempfile::tempdir().unwrap();
    nothing_due(d.path());
    let server = MockServer::start();
    let runs = timings(d.path(), &server.base_url(), &["list"]);
    let p = p95(&runs);
    eprintln!("list p95 {p:?} over {RUNS} runs");
    assert_eq!(
        server.requests().len(),
        0,
        "a fetch was due, so this measured the wrong path"
    );
    assert!(
        p <= Duration::from_millis(50),
        "list p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}
```

- [ ] **Step 12: Run the timing tests**

Run: `cargo test -p tagteam --features test-support --test perf`
Expected: `running 0 tests`. A debug build compiles the file out.

Run: `cargo test --release -p tagteam --features test-support --test perf -- --ignored --nocapture`
Expected: PASS, 2 tests. The printed p95s are within 10 ms (`statusline`) and 50 ms (`list`),
and neither run sent a request. Run this on an otherwise idle machine. After one miss, re-run
once. A second miss is a real miss of §1.1's budget: stop and report both measured p95s.
Do not loosen a threshold.

- [ ] **Step 13: Run the checks, with a release lint for the timing file**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --release -p tagteam --all-targets --features test-support -- -D warnings
```
Expected: no diff and no warnings. The last command is the only one that lints `perf.rs`,
because the debug clippy runs compile it out.

- [ ] **Step 14: Commit**

```bash
git add crates/tagteam/tests/perf.rs
git commit -m "Measure statusline and list against their timing targets"
```

---

### Task 17: M1 carry-overs (L343, L342, L397)

Three small items from the M1 final review's triage (Decision 12). Each is independent: one
failing test, one fix, one commit apiece. The review file is local and git-ignored, so the
findings are quoted here:

- **L343** (T9): "`ProcessRunner` has no kill on `try_wait` error and no join timeout" — CAN
  WAIT "(M2, before the daemon makes a hang costly)". M2b's collector calls `security` from one
  thread per account, so a hung `security` would now hang a `list`.
- **L342** (T9): "fake `exists` is Unreadable when locked, unlike real `security`" — CAN WAIT
  "(M2: make the fakes model rc 0/44 so engine tests follow reality)". Real
  `security find-generic-password` without `-w` or `-g` reads attributes only, so it answers
  rc 0 (present) or rc 44 (absent) whether or not the keychain is locked.
- **L397** (T14): "`remove_items` and the other edge cases" — CAN WAIT "(M2/M5). The
  `ShadowingItem` wording for managed-key removal is worth fixing with M2." Today's message
  says "the credential was written to the file, but the Keychain item … that shadows it could
  not be verified gone", which is false when `clear_managed_key` removes a managed-key item
  and no file was written at all. L311 stays with M5.

**Files:**
- Modify: `crates/tagteam-provider/src/security.rs` (L343, and its `mod tests`)
- Modify: `crates/tagteam-provider/src/keychain.rs` (L342, and its `mod tests`)
- Modify: `crates/tagteam-provider/src/provider.rs` (L397, the `ShadowingItem` message)
- Modify: `crates/tagteam-cc/tests/live_store.rs` (L397's test)

**Interfaces:**
- Consumes: `Runner`, `RunResult`, `ProcessRunner` (`tagteam-provider`), `FakeKeychain` and
  `FileKeychain`, `LiveStore::clear_managed_key` (`tagteam-cc`).
- Produces: no public name changes. `ProcessRunner` stays a unit struct, so every
  `Box::new(ProcessRunner)` call site is untouched. New private items in `security.rs`:
  `DRAIN_GRACE`, `Waitable`, `Waited`, `wait_for`, `drain`, `collect` and
  `ProcessRunner::run_bounded`.

**Decision inside this task (flag it in review):** when the child has exited but its output
pipes do not close within the grace period (a grandchild inherited them), `run` returns
`RunResult::TimedOut`, never `Exited` with truncated output. A caller must not parse half a
secret as a whole one. `SecurityCli` already maps `TimedOut` to an unreadable result.

- [ ] **Step 1: L343 — write the failing tests**

In `crates/tagteam-provider/src/security.rs`, inside `mod tests`, add these imports next to the
existing ones:

```rust
    use std::io;
    use std::os::unix::process::ExitStatusExt;
```

and add these tests at the end of the module:

```rust
    /// A child whose `try_wait` answers are scripted, counting the kills and reaps.
    struct FakeChild {
        polls: VecDeque<io::Result<Option<ExitStatus>>>,
        kills: usize,
        waits: usize,
    }

    impl FakeChild {
        fn new(polls: Vec<io::Result<Option<ExitStatus>>>) -> Self {
            Self {
                polls: polls.into(),
                kills: 0,
                waits: 0,
            }
        }
    }

    impl Waitable for FakeChild {
        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            self.polls.pop_front().expect("unexpected extra poll")
        }
        fn kill(&mut self) -> io::Result<()> {
            self.kills += 1;
            Ok(())
        }
        fn wait(&mut self) -> io::Result<ExitStatus> {
            self.waits += 1;
            Ok(ExitStatus::from_raw(0))
        }
    }

    #[test]
    fn a_failed_try_wait_kills_and_reaps_the_child() {
        // L343: the child used to be left running, and never reaped.
        let mut child = FakeChild::new(vec![Err(io::Error::other("boom"))]);
        let waited = wait_for(&mut child, Instant::now() + Duration::from_secs(60));
        assert!(matches!(&waited, Waited::Failed(m) if m.contains("boom")), "{waited:?}");
        assert_eq!((child.kills, child.waits), (1, 1));
    }

    #[test]
    fn a_child_past_its_deadline_is_killed_and_reaped() {
        let mut child = FakeChild::new(vec![Ok(None)]);
        assert_eq!(wait_for(&mut child, Instant::now()), Waited::TimedOut);
        assert_eq!((child.kills, child.waits), (1, 1));
    }

    #[test]
    fn a_child_that_exits_in_time_is_left_alone() {
        let mut child = FakeChild::new(vec![Ok(None), Ok(Some(ExitStatus::from_raw(3 << 8)))]);
        let waited = wait_for(&mut child, Instant::now() + Duration::from_secs(60));
        assert_eq!(waited, Waited::Exited(3));
        assert_eq!((child.kills, child.waits), (0, 0));
    }

    #[test]
    fn a_real_process_reports_its_code_and_both_streams() {
        let r = ProcessRunner::run_bounded(
            "/bin/sh",
            &s(&["-c", "printf out; printf err >&2; exit 3"]),
            None,
            Duration::from_secs(5),
            Duration::from_secs(1),
        );
        match r {
            RunResult::Exited {
                code,
                stdout,
                stderr,
            } => {
                assert_eq!(code, 3);
                assert_eq!(stdout, b"out");
                assert_eq!(stderr, b"err");
            }
            other => panic!("expected Exited, got {other:?}"),
        }
    }

    #[test]
    fn a_grandchild_holding_the_pipes_cannot_hang_run() {
        // L343: `sleep` inherits the pipes and outlives the shell, so the readers never see
        // EOF. `run` used to join them and wait the full 8 s.
        let started = Instant::now();
        let r = ProcessRunner::run_bounded(
            "/bin/sh",
            &s(&["-c", "sleep 8 & echo done"]),
            None,
            Duration::from_secs(5),
            Duration::from_millis(200),
        );
        assert!(matches!(r, RunResult::TimedOut), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-provider --lib security::tests`
Expected: FAIL to compile: `cannot find trait Waitable`, `cannot find function wait_for`,
`cannot find type Waited` and no function `run_bounded` on `ProcessRunner`. (The existing tests
are unchanged and would still pass.)

- [ ] **Step 3: Bound the drain and reap on every failed wait**

In `crates/tagteam-provider/src/security.rs`, change the imports at the top:

```rust
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
```

Then replace everything from `pub struct ProcessRunner;` through the closing brace of
`fn run` (the `    }` that precedes `fn run_attached`) with the following. `run_attached` and the
closing brace of the `impl` stay as they are.

```rust
/// How long `run` waits for the output pipes to close once the child has exited. A grandchild
/// that inherited a pipe keeps it open after the child is gone; without a bound, `run` would
/// wait for that grandchild too (L343).
const DRAIN_GRACE: Duration = Duration::from_secs(2);

pub struct ProcessRunner;

/// What `wait_for` needs from a child process, so its failure paths are testable.
trait Waitable {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>>;
    fn kill(&mut self) -> std::io::Result<()>;
    fn wait(&mut self) -> std::io::Result<ExitStatus>;
}

impl Waitable for Child {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        Child::try_wait(self)
    }
    fn kill(&mut self) -> std::io::Result<()> {
        Child::kill(self)
    }
    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        Child::wait(self)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Waited {
    Exited(i32),
    TimedOut,
    Failed(String),
}

/// Polls until the child exits or `deadline` passes. A timeout and a failed poll both kill and
/// reap the child, so neither leaves a process running (L343).
fn wait_for(child: &mut dyn Waitable, deadline: Instant) -> Waited {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Waited::Exited(status.code().unwrap_or(-1)),
            Ok(None) if Instant::now() >= deadline => {
                reap(child);
                return Waited::TimedOut;
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                reap(child);
                return Waited::Failed(e.to_string());
            }
        }
    }
}

fn reap(child: &mut dyn Waitable) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Reads a pipe to its end on a thread and sends what it read. The thread is detached, so a
/// pipe that never closes costs one parked thread, not a hang.
fn drain<R: std::io::Read + Send + 'static>(pipe: Option<R>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut buf);
        }
        let _ = tx.send(buf);
    });
    rx
}

/// The drained bytes, or `None` when the pipe had not closed by `until`.
fn collect(rx: &mpsc::Receiver<Vec<u8>>, until: Instant) -> Option<Vec<u8>> {
    rx.recv_timeout(until.saturating_duration_since(Instant::now()))
        .ok()
}

impl ProcessRunner {
    fn run_bounded(
        program: &str,
        args: &[String],
        stdin: Option<&[u8]>,
        timeout: Duration,
        grace: Duration,
    ) -> RunResult {
        let mut child = match Command::new(program)
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return RunResult::SpawnFailed(e.to_string()),
        };
        if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
            let data = data.to_vec();
            thread::spawn(move || {
                let _ = pipe.write_all(&data);
            });
        }
        let out = drain(child.stdout.take());
        let err = drain(child.stderr.take());
        match wait_for(&mut child, Instant::now() + timeout) {
            Waited::Exited(code) => {
                let until = Instant::now() + grace;
                match (collect(&out, until), collect(&err, until)) {
                    (Some(stdout), Some(stderr)) => RunResult::Exited {
                        code,
                        stdout,
                        stderr,
                    },
                    // Never hand back half an output as a whole one.
                    _ => RunResult::TimedOut,
                }
            }
            Waited::TimedOut => RunResult::TimedOut,
            Waited::Failed(e) => RunResult::SpawnFailed(e),
        }
    }
}

impl Runner for ProcessRunner {
    fn run(
        &self,
        program: &str,
        args: &[String],
        stdin: Option<&[u8]>,
        timeout: Duration,
    ) -> RunResult {
        Self::run_bounded(program, args, stdin, timeout, DRAIN_GRACE)
    }
```

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p tagteam-provider --lib security::tests`
Expected: PASS, including the five new tests and every pre-existing `security` test.

On a Mac also run `cargo test -p tagteam-provider --features real_keychain --test real_keychain`.
Expected: PASS with no GUI prompt: the real `security` binary goes through the new `run`.

- [ ] **Step 5: Format, lint and commit**

Run:
```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no diff and no warnings.

```bash
git add crates/tagteam-provider/src/security.rs
git commit -m "Bound ProcessRunner's pipe drain and reap the child when waiting fails"
```

- [ ] **Step 6: L342 — write the failing tests**

In `crates/tagteam-provider/src/keychain.rs`, inside `mod tests`, change the tail of
`fake_keychain_models_absent_locked_and_failures`. Replace

```rust
        k.set_locked(true);
        assert!(matches!(k.exists("s", "a"), Read::Unreadable(_)));
        k.set_locked(false);
```

with

```rust
        k.set_locked(true);
        // L342: `exists` reads attributes only, which `security` answers without an unlock.
        assert!(k.exists("s", "a").is_present());
        assert!(matches!(k.find("s", "a"), Read::Unreadable(_)));
        k.set_locked(false);
```

(At that point the item still holds `v`: the failed write and failed delete above changed
nothing.) Add a new test after it:

```rust
    #[test]
    fn fake_exists_answers_present_or_absent_even_when_locked() {
        // L342: real `security find-generic-password` (no -w, no -g) gives rc 0 or rc 44
        // whether or not the keychain is locked; engine tests must follow reality.
        let k = FakeKeychain::new();
        k.put("s", "a", b"v");
        k.set_locked(true);
        assert!(k.exists("s", "a").is_present());
        assert!(matches!(k.exists("s", "missing"), Read::Absent));
        assert!(matches!(k.find("s", "a"), Read::Unreadable(_)), "the secret still needs it");
        // An item marked unreadable is still the way a test injects a failing `exists`.
        k.set_locked(false);
        k.set_unreadable("s", "a", true);
        assert!(matches!(k.exists("s", "a"), Read::Unreadable(_)));
    }
```

In the same module, at the end of `file_keychain_persists_across_instances`, after
`assert!(!k.unlock());` and before the closing brace, add:

```rust
        // L342: attributes need no unlock.
        assert!(
            k.exists("Claude Code-credentials", "me").is_present(),
            "present while locked"
        );
        assert!(matches!(k.exists("nope", "me"), Read::Absent));
```

- [ ] **Step 7: Run them to verify they fail**

Run: `cargo test -p tagteam-provider --features file-keychain --lib keychain::tests`
Expected: FAIL in `fake_keychain_models_absent_locked_and_failures`,
`fake_exists_answers_present_or_absent_even_when_locked` and `file_keychain_persists_across_instances`,
each at the `exists` assertion made while locked (`is_present()` is false, because `exists` is
`Unreadable`).

- [ ] **Step 8: Model rc 0 / 44 in both fakes**

In `crates/tagteam-provider/src/keychain.rs`, replace `FakeKeychain`'s `exists`:

```rust
    /// Attributes only, as `security find-generic-password` without `-w` or `-g` reads them:
    /// rc 0 when the item is there and rc 44 when it is not, locked or not (L342). Only an
    /// item marked unreadable fails, which is how a test injects a failing `exists`.
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        if self.unreadable.lock().unwrap().contains(&key(s, a)) {
            return Read::Unreadable(ReadError::new(
                "keychain",
                "rc 1: injected failure reading the item's attributes",
            ));
        }
        match self.get(s, a) {
            Some(_) => Read::Present(()),
            None => Read::Absent,
        }
    }
```

and `FileKeychain`'s `exists`:

```rust
    /// Attributes only, locked or not, like `FakeKeychain::exists` (L342).
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        match std::fs::metadata(self.path(s, a)) {
            Ok(_) => Read::Present(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Read::Absent,
            Err(e) => Read::Unreadable(ReadError::new("keychain", e.to_string())),
        }
    }
```

- [ ] **Step 9: Run them to verify they pass, then the whole workspace**

Run: `cargo test -p tagteam-provider --features file-keychain --lib keychain::tests`
Expected: PASS.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. `exists` has one production caller, `LiveStore::remove_items` in
`tagteam-cc`, which runs `delete` (ignoring its result) and then `exists`. Under a locked fake
an item that really is gone now reads `Absent` instead of `Unreadable`, which is what `security`
does. If any test fails because it relied on the old locked `exists`, that test encoded the
fake's bug: change its expectation to the real `security` behaviour, and say which test in the
commit body.

- [ ] **Step 10: Format, lint and commit**

Run:
```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no diff and no warnings.

```bash
git add crates/tagteam-provider/src/keychain.rs
git commit -m "Make the fake Keychain's exists answer like security when locked"
```

- [ ] **Step 11: L397 — write the failing test**

In `crates/tagteam-cc/tests/live_store.rs`, add after
`managed_keys_record_approval_and_never_leave_a_shadowing_item`:

```rust
#[test]
fn a_managed_key_item_that_will_not_delete_is_reported_as_a_removal_not_a_file_write() {
    // L397: `clear_managed_key` writes no file, so the message must not say one was written.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let svc = keychain_service(&f.env, ItemKind::ManagedKey);
    let acct = keychain_account(&f.env);
    f.kc.put(&svc, &acct, b"sk-ant-api03-stale");
    f.kc.set_fail_delete(&svc, true);
    let err = s
        .clear_managed_key(&f.env, &f.paths, &open)
        .expect_err("the item cannot be verified gone");
    let ProviderError::ShadowingItem(name) = &err else {
        panic!("expected ShadowingItem, got {err:?}");
    };
    let shown = err.to_string();
    assert!(shown.contains(name.as_str()), "{shown}");
    assert!(shown.contains("could not be verified gone"), "{shown}");
    assert!(!shown.contains("written to the file"), "{shown}");
}
```

- [ ] **Step 12: Run it to verify it fails**

Run: `cargo test -p tagteam-cc --test live_store a_managed_key_item_that_will_not_delete`
Expected: FAIL at the `written to the file` assertion, with the current message in the output.

- [ ] **Step 13: Reword the message**

In `crates/tagteam-provider/src/provider.rs`, replace

```rust
    #[error(
        "the credential was written to the file, but the Keychain item {0} that shadows it could not be verified gone"
    )]
    ShadowingItem(String),
```

with

```rust
    /// A Keychain item `remove_items` could not verify gone: after a file fallback, after a
    /// managed-key fallback, or when a managed key is removed. It may still be read instead
    /// of what tagteam wrote (L397).
    #[error(
        "the Keychain item {0} could not be verified gone, so it may still be read instead of what tagteam wrote"
    )]
    ShadowingItem(String),
```

- [ ] **Step 14: Run it to verify it passes, then the affected suites**

Run: `cargo test -p tagteam-cc --test live_store`
Expected: PASS, including `file_fallback_requires_the_shadowing_item_to_be_gone` and
`file_fallback_verifies_every_fallback_item_is_gone_including_the_plain_one` (they match the
variant, not its text).

Run: `rg -n 'that shadows it' crates`
Expected: no output (nothing else quoted the old wording).

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS.

- [ ] **Step 15: Format, lint and commit**

Run:
```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no diff and no warnings.

```bash
git add crates/tagteam-provider/src/provider.rs crates/tagteam-cc/tests/live_store.rs
git commit -m "Reword the shadowing-item error so it fits managed-key removal"
```

---

### Task 18: Final verification and live acceptance

**Human steps (Step 3 is Michael's call; Steps 4–6 are the acceptance).** Steps 1–2 are the implementer's. The live acceptance run sends real
usage requests with real tokens, and may refresh a real live token through §7.5, so Michael
runs it, and the merge waits for it. An agent never runs Steps 3–6. The whole-branch review and
the pre-merge cross-review run after Step 2 and before the pull request. They follow the global
workflow and are not steps of this plan.

**Files:**
- Modify: this plan's own file, `docs/superpowers/plans/2026-09-30-tagteam-m2b-usage-views.md`
  (the `**Status:**` line)

**Interfaces:**
- Consumes: everything Tasks 1–17 produced.
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
- No diff and no warnings, with the test features and without them (M2a's lint ruling).
- Every test passes. The `--ignored` run includes M2a's 9 s CC-lock test and `gate_race`'s 15 s
  stopped-holder test.
- `cargo test -p tagteam --lib` runs without features, so it exercises the release branch of
  the test-override checks.
- The perf run passes both timing tests: `statusline` p95 ≤ 10 ms and `list` p95 ≤ 50 ms over
  50 runs with every reading fresh (§1.1, §13.5). Run it on an otherwise idle machine. One
  failure gets one re-run; a second failure is a real miss of the budget: stop and report the
  measured p95.

On a Mac, also run
`cargo test -p tagteam-provider --features real_keychain --test real_keychain` (PASS, no GUI
prompt). It drives the real `security` binary through Task 17's `ProcessRunner`.

Run: `cargo check -p tagteam-core -p tagteam-provider -p tagteam-cc -p tagteam-fake --target x86_64-unknown-linux-gnu`
Expected: `Finished`. The engine and the binary are left out because bundled SQLite's build
script needs a Linux C cross-compiler; if this Mac has one, add `-p tagteam-engine -p tagteam`.

- [ ] **Step 2: Build the release binary and check it carries no test hooks**

Run:
```bash
cargo build --release -p tagteam
grep -a -c TAGTEAM_TEST_ target/release/tagteam ; test $? -eq 1
```
Expected: the build succeeds and `grep` prints `0` and exits 1, so the final `test` exits 0.
The keychain-directory, platform and API-base overrides exist only under `test-support`, and
M2b added no new ones.

- [ ] **Step 3: Michael — check it against the real login**

Run, from the worktree root (`./target/release/tagteam` is the binary from Step 2; put it on
`PATH` as `tagteam` for the rest of this task, or substitute the path):

```bash
./target/release/tagteam status && ./target/release/tagteam list
```
Expected:
- `status` names the live Claude Code login, with its 5-hour and 7-day usage.
- `list` shows each stored account with 5H, 7D, SPEND and AGE columns, or says there are none.
- Both exit 0.

Unlike M2a's read-only check, this **does** send requests and write the store: `list` fetches
usage for every account, and `status` for the live one (at most one request per account).
An inactive account whose token has expired is refreshed through the gate, and the live
account's token is refreshed through §7.5 only if the usage endpoint finds it expired or
rejected. Running it is Michael's call. If the first request is blocked, suspect Little
Snitch: the new binary is ad-hoc signed, so allow `api.anthropic.com` for it and re-run.

- [ ] **Step 4: Michael — prepare the acceptance run**

The goal: the numbers tagteam shows match what Claude.ai shows, the request budget holds, and
the status line and history work on a real login. No token is handled by hand.

1. Keep `claude.ai` → Settings → Usage open in a browser signed in as the live account.
2. Have at least two accounts stored (`tagteam list`), one of them live.
3. Define a helper that prints only positions, statuses and numbers (no email, no token):

```bash
tt_usage() {
  tagteam list --json | /usr/bin/python3 -c '
import json, sys
for a in json.load(sys.stdin)["accounts"]:
    u = a.get("usage") or {}
    f = u.get("fiveHour") or {}
    s = u.get("sevenDay") or {}
    print(a["position"], a["usageStatus"], "5h", f.get("pct"), f.get("resetsAt"), "7d", s.get("pct"), s.get("resetsAt"))
'
}
tt_budget() {
  sqlite3 ~/.local/share/tagteam/tagteam.db \
    "select count(*) from usage_requests where at > cast(strftime('%s','now') as integer) - 3660 group by provider, identity_key order by 1 desc limit 1"
}
```

4. Note the time and run `tt_budget` once, so later counts have a baseline `B0`. If the
   command prints nothing, `B0` is 0.
5. The history row needs samples over time: start using `tagteam list` (or let the loop in
   row 5 run) at least a day before the merge, since `history` needs a day of samples for the
   7-day average pace.

- [ ] **Step 5: Michael — run the acceptance**

| # | Command | Pass when |
|---|---|---|
| 1 | `tagteam list`, then read the live account's Usage page on claude.ai | `list` exits 0 and its live row's 5H and 7D percentages are each within 1 point of claude.ai's; the reset countdowns agree to within a minute |
| 2 | Within 3 minutes of row 1: `tagteam --debug list`, then `tt_budget` | the debug output names no usage request, the AGE column has grown instead of resetting, and `tt_budget` still equals its value after row 1 |
| 3 | `tt_usage` | the live account's line shows `ok`, numeric 5h and 7d percentages, and ISO `resetsAt` values; no account shows `relogin_required` or `token_expired` |
| 4 | `tagteam statusline --print-config`, add the printed `statusLine` block to Claude Code's `settings.json`, then send one prompt in Claude Code | the status bar shows the live account's line, its 5h and 7d figures match row 1 to within a point or the last `list` refresh, and `echo '{}' \| tagteam statusline` returns at once with no network and no Keychain prompt |
| 5 | `for i in $(seq 30); do tagteam list >/dev/null; sleep 10; done`, then `tt_budget` | the count never exceeds 20, and it grew by at most 3 over `B0` (the 180 s floor allows about two fetches per account in five minutes) |
| 6 | A day or more later: `tagteam history`, `tagteam history --json`, `tagteam history --csv`, and `sqlite3 ~/.local/share/tagteam/tagteam.db "select window, count(*) from usage_samples group by window"` | every window has samples; `history` shows a sparkline, a rate in `pts/h` and either `runs out in …` or `lasts to reset`; the JSON parses as one object; the CSV starts with `window,fetched_at,pct,resets_at` |

If `settings.json` is chezmoi-managed, edit the chezmoi source (`chezmoi edit`) and run
`chezmoi apply`, per the machine's dotfile rule, rather than editing the deployed file.

Extended, optional: run `tagteam switch <other>` and `tagteam list` once more. The switch
re-plans, without fetching, the polls of whichever account already has a reading (Task 12),
and the next `list` shows the new live account's usage.

- [ ] **Step 6: Decide and record**

- **Pass** (rows 1–6): put the table into the pull request description. Record percentages,
  counts and times relative to the run. Never a token, never an email. Then continue with
  Step 7.
- **Fail:** stop. Do not open or merge the pull request. Bring the table to Michael. A failure
  in row 1 means Appendix A.5's normalization or the reset parsing disagrees with the real
  endpoint; in row 2 or row 5 it means the on-demand rule, the lease or the budget (Tasks 8,
  10); in row 3 it means the §7.5 triggers or the status derivation (Tasks 11, 13); in row 4,
  Task 16; in row 6, Tasks 5 and 15.

- [ ] **Step 7: Mark the plan implemented**

Once implementation and review are complete and merging is the next action, set this plan's
`**Status:**` line to `Implemented — <pull request URL>`, per the design-record lifecycle. The
spec stays `In progress` until M5.

```bash
git add docs/superpowers/plans/2026-09-30-tagteam-m2b-usage-views.md
git commit -m "Mark the M2b plan implemented"
```
