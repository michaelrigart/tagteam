# tagteam token report Implementation Plan

**Status:** Approved

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A per-account token and API-cost report over Claude Code's transcripts: an incremental index in the store, session evidence from `run`, scans and the statusline, query-time attribution, the engine view the TUI renders, and `tagteam tokens`.

**Architecture:** Pure logic (counts, merge rule, prices, attribution, timeline, blocks, bucketing) lives in `tagteam-core::tokens`. The provider trait gains a `transcripts` capability that Claude Code implements: discovery, a line parser and a shipped price table. The engine owns the index (migration step 3), the refresh, the evidence writers and the report view. The CLI adds `tokens`, and wires the statusline sighting and `run`'s launch record and poll.

**Tech Stack:** Rust 2024 (toolchain 1.88.0, MSRV 1.85), rusqlite (bundled SQLite), serde/serde_json, sha2, `jiff` (new; time zones), clap, scoped threads (no async runtime).

**Spec:** `docs/superpowers/specs/2026-10-14-tagteam-token-report-design.md` (Approved, e6cc92a), cited below as "spec §n". The core spec `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md` is cited as "core §n". Executors read the spec before their task.

## Global Constraints

- Never write a Claude Code file (core §3). Reading transcripts and session records is allowed. Tests use scratch dirs (`Env::for_test`, `tempfile`), the fake Keychain and `tagteam-fake`; never the real home, a real account or the login keychain (CONTRIBUTING).
- The index stores counts, model names, hashed response ids, times, session ids and working directories only. No message content, ever. Logs: counts and timings at info/debug; transcript paths and session ids at debug only; never content, an email or a label (core §14.2).
- New dependencies: `jiff` only (an exact version at least two weeks old, added to `[workspace.dependencies]` and used by `tagteam-core` and `tagteam-engine`), and the existing workspace `serde` added to `tagteam-cc`. Nothing else. After adding a crate run `cargo fetch` unsandboxed (the sandbox blocks the registry).
- Times: the store holds epoch milliseconds. JSON prints ISO 8601 UTC through `tagteam_core::time::format_iso8601(ms.div_euclid(1000))`. Dates and months print in the report's time zone as `YYYY-MM-DD` / `YYYY-MM`.
- JSON output: one object on stdout, `schemaVersion: 1`, camelCase keys, errors as `{"schemaVersion":1,"error":{"type","message"}}` (core §13.2). Exit codes `0` OK, `1` error, `2` usage error, `128 + signal` interrupted.
- Store: WAL, `busy_timeout` 5000 ms; read-then-write transactions are IMMEDIATE; no write transaction holds the write lock for more than about 50 ms (spec §5.2). Migration step 3 runs in the ladder's one transaction (spec §4).
- Locks: `locks/tokens-<provider>.lock` is a leaf (spec §5.1): it may be taken under any tagteam lock, and nothing is taken while it is held.
- `statusline` stays within 10 ms p95, with no network and no Keychain (core §1.1, §13.5).
- Lints: `#![forbid(unsafe_code)]` where a crate already has it; workspace lints (`unsafe_op_in_unsafe_fn = deny`, `clippy::dbg_macro = deny`). Every task ends green on:
  ```
  cargo fmt --all --check
  cargo clippy --workspace --all-targets --locked -- -D warnings
  cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings
  cargo test --workspace --locked --no-fail-fast --features tagteam/test-support
  ```
  An implementer runs only its task's focused tests; the controller runs the full suite once per batch.
- Commits: one per task, imperative mood, no agent attribution of any kind.
- Kinds of `EngineError` are part of the `--json` contract: a new variant gets a `kind()` arm and joins `kind_is_pinned_for_every_variant` (`crates/tagteam-engine/src/error.rs`).

## Review Focus

1. **A huge transcript or tree.** A 175 MB transcript, a 64 MiB+ single line, or tens of thousands of files: the refresh's memory stays bounded to one chunk (plus one assembling line up to 64 MiB) per worker, and a budgeted refresh still yields within one chunk. Owner: Task 14 (test `a_huge_line_is_skipped_and_counted_without_holding_the_file`) and Task 15 (test `memory_and_budget_hold_on_a_file_of_many_chunks`).
2. **Garbage in a transcript.** Non-UTF-8 bytes, truncated JSON, an assistant line whose `usage` lacks a field, numbers out of range (negative or above `u64`): each is skipped and counted, and nothing panics. Owner: Task 8 (test `hostile_lines_are_errors_never_panics`, proptest over arbitrary bytes).
3. **Clock skew.** A response timestamped in the future, or before the retention window: a future one is indexed and reported; one older than the window is not inserted; the active block never projects a negative remaining time. Owner: Task 15 (test `lines_outside_the_retention_window_are_not_inserted`) and Task 5 (test `a_future_first_response_projects_nothing_negative`).
4. **DST and odd zones.** `--tz` on a day with a DST change (23 h or 25 h day), a half-hour zone, and `--since 2026-03-29` meaning local midnight: days and months bucket by local calendar, and the range starts at local midnight. Owner: Task 6 (test `days_of_23_and_25_hours_bucket_by_the_local_calendar`) and Task 19 (test `a_since_date_is_local_midnight_in_tz`).
5. **Two processes at once.** `tagteam tokens` waiting while the daemon or TUI refreshes, and a `purge` arriving mid-refresh: the waiter reports complete numbers, `Try` returns `Busy` at once, and a purge never interleaves with a batch. Owner: Task 15 (tests `a_waiting_refresh_runs_after_the_holder` and `try_returns_busy_while_another_holds_the_lock`) and Task 17 (test `purge_waits_for_a_running_refresh`).

## Rulings recorded while planning

These stay inside the approved spec; each is repeated in the Draft PR's "Rulings".

- **R1.** `token_epoch` gains a `seeded INTEGER NOT NULL` column. A store migrated by an open that could not seed profiles (a raw `Store::open` outside the engine, as tests do) records `seeded = 0`, and spec §7.2 then treats all pre-epoch coverage as incomplete. Every engine open seeds, so production stores record `seeded = 1`. This only makes §7.2 more conservative.
- **R2.** The provider trait also gains `default_session_records_dir(&Env) -> Option<PathBuf>` (Claude Code: `<config_home>/sessions`), so the scan (spec §6.2) and the statusline (spec §6.3) can name the default home's records without the engine naming a CC path (core §4.1).
- **R3.** `ProcessProbe` gains `parent_pid(pid) -> Option<u32>` (default `None`) for the statusline's ancestor walk (spec §6.3). `SessionRecord` gains `session_id`.
- **R4.** The rate a response is priced at uses nano-dollars per token, `round(USD per MTok × 1000)`, and per web-search request `round(USD per 1k × 1_000_000)`. Web searches with no price contribute nothing; tokens stay priced.
- **R5.** A transcript line with a `message.id` and no `requestId` keys on `(message.id, "")` (spec §2.2: only `<synthetic>` lines lack it today, and those are skipped).
- **R6.** `tagteam tokens` with both `--csv` and `--json` is a usage error, as for `history`.
- **R7–R8 and the rest.** Rulings the task writers made while verifying their code against the repository are listed under "Rulings (planning)" at the end of this plan; each is repeated in the Draft PR.

---

## File Structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/xtask/src/compat/checks/transcripts.rs` | Compat check: real transcript fields, record `sessionId`, `/clear` behaviour | 1 |
| `crates/tagteam-core/src/tokens/mod.rs` | Module root and re-exports | 2 |
| `crates/tagteam-core/src/tokens/counts.rs` | `TokenCounts`, the merge rule (spec §2.2) | 2 |
| `crates/tagteam-core/src/tokens/price.rs` | Rate cards, price table, overrides, cost in nano-dollars (spec §8) | 3 |
| `crates/tagteam-core/src/tokens/attribution.rs` | Homes, sightings, launches, coverage, timeline, placement (spec §7) | 4 |
| `crates/tagteam-core/src/tokens/blocks.rs` | Observed and inferred blocks, active projection (spec §9) | 5 |
| `crates/tagteam-core/src/tokens/bucket.rs` | Day and month bucketing in a `jiff` zone | 6 |
| `crates/tagteam-provider/src/transcript.rs` | `UsageLine`, `LineError` | 7 |
| `crates/tagteam-provider/src/{provider,liveness,process}.rs` | Capability, trait methods, `session_id`, `parent_pid` | 7 |
| `crates/tagteam-cc/src/transcripts.rs` | Discovery and the line parser | 8 |
| `crates/tagteam-cc/src/prices.rs` | The shipped price table | 8 (empty), 9 (filled) |
| `crates/tagteam-fake/src/transcripts.rs` | Fake transcripts for engine tests | 10 |
| `crates/tagteam-engine/src/store/schema_v3.sql` | Migration step 3 | 11 |
| `crates/tagteam-engine/src/store/tokens.rs` | Store API for the token tables | 12 |
| `crates/tagteam-engine/src/settings.rs` | `tokens.retention_days`, price overrides | 13 |
| `crates/tagteam-engine/src/tokens/mod.rs` | Engine module root, the lock path | 14 |
| `crates/tagteam-engine/src/tokens/reader.rs` | One transcript file: offsets, hashes, chunks, lines | 14 |
| `crates/tagteam-engine/src/tokens/refresh.rs` | `refresh_tokens` | 15 |
| `crates/tagteam-engine/src/tokens/evidence.rs` | `sight_sessions`, launches, statusline sighting | 16 |
| `crates/tagteam-engine/src/purge.rs` | Purge takes the tokens lock | 17 |
| `crates/tagteam-engine/src/tokens/view.rs` | Report types, `token_report`, `token_index_state`, `drill` | 18 |
| `crates/tagteam/src/tokens_cmd.rs`, `cli.rs`, `app.rs` | `tagteam tokens` | 19 |
| `crates/tagteam/src/statusline.rs`, `app.rs` | The statusline sighting | 20 |
| `crates/tagteam/src/run.rs` | Launch record and poll | 21 |
| tests, command lists | Invariant, run-shell and logging lists | 22 |
| `crates/tagteam/tests/perf.rs`, `crates/tagteam-engine/tests/tokens_bench.rs` | Budgets of spec §1.1 | 23 |
| docs | Core spec amendments (spec §16), README, CHANGELOG | 24 |

## Shared interfaces

Every task's **Interfaces** block quotes from here. Names and types are binding; a task that needs a change stops and reports it.

### `tagteam_core::tokens` (Tasks 2–6)

```rust
// counts.rs (Task 2)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
    pub web_search: u64, // requests, not tokens
}
impl TokenCounts {
    pub fn cache_write(&self) -> u64;           // 5m + 1h, saturating
    pub fn prompt(&self) -> u64;                // input + cache_write() + cache_read, saturating
    pub fn total(&self) -> u64;                 // input + output + cache_write() + cache_read, saturating
    pub fn merge_max(&mut self, other: &TokenCounts); // field-wise max: spec §2.2
    pub fn add(&mut self, other: &TokenCounts); // field-wise saturating sum
}

// price.rs (Task 3)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateCard { pub input: f64, pub output: f64, pub cache_write_5m: f64, pub cache_write_1h: f64, pub cache_read: f64 } // USD per MTok
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice { pub base: RateCard, pub long_prompt: Option<(u64, RateCard)>, pub fast: Option<RateCard>, pub web_search_per_1k: Option<f64> }
#[derive(Debug)]
pub struct PriceTable { pub as_of: &'static str, pub models: &'static [(&'static str, ModelPrice)] }
pub static EMPTY_PRICES: PriceTable = PriceTable { as_of: "", models: &[] };
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CardOverride { pub input: Option<f64>, pub output: Option<f64>, pub cache_write_5m: Option<f64>, pub cache_write_1h: Option<f64>, pub cache_read: Option<f64> }
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PriceOverride { pub base: CardOverride, pub long_prompt_above: Option<u64>, pub long_prompt: CardOverride, pub fast: CardOverride, pub web_search_per_1k: Option<f64> }
#[derive(Debug, Clone)]
pub struct Prices { /* private */ }
impl Prices {
    /// Shipped table merged with overrides (spec §8.2). Returns the models an override changed or added, sorted.
    pub fn new(table: &PriceTable, overrides: &BTreeMap<String, PriceOverride>) -> (Prices, Vec<String>);
    pub fn as_of(&self) -> &str;
    pub fn lookup(&self, model: &str) -> Option<&ModelPrice>; // exact, then strip_date_suffix
    /// None: unpriced (no entry, or `speed == Some("fast")` with no fast card). Spec §8.1, §8.3, ruling R4.
    pub fn cost_nanos(&self, model: &str, counts: &TokenCounts, speed: Option<&str>) -> Option<u64>;
}
pub fn strip_date_suffix(model: &str) -> &str; // "claude-haiku-4-5-20251001" -> "claude-haiku-4-5"

// attribution.rs (Task 4)
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Home { Default, Profile(AccountId) }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting { pub home: Home, pub first_ms: i64, pub last_ms: i64 }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch { pub account: AccountId, pub start_ms: i64, pub end_ms: Option<i64> } // None: still open
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coverage { pub epoch_ms: i64, pub seeded: bool, pub last_pre_epoch_remove_ms: Option<i64> }
impl Coverage { pub fn complete_at(&self, t: i64) -> bool; } // t >= epoch, or (seeded and t >= last pre-epoch remove, if any)
pub struct Launches { /* sorted by start */ }
impl Launches { pub fn new(launches: Vec<Launch>) -> Self; pub fn any_covers(&self, t: i64) -> bool; } // start <= t && (end is None || t < end)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement { Home(Home), Unattributed }
/// Spec §7.1–7.2: sighted in exactly one home -> it; in two -> Unattributed; nowhere -> Default when no launch covers t and coverage is complete, else Unattributed.
pub fn place(sightings: &[Sighting], launches: &Launches, coverage: &Coverage, t: i64) -> Placement;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineEvent {
    Switch { at_ms: i64, from: Option<AccountId>, to: AccountId },
    Add { at_ms: i64, account: AccountId },
    Remove { at_ms: i64, account: AccountId },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveChange { pub at_ms: i64, pub account: Option<AccountId> }
pub struct Timeline { /* segments */ }
impl Timeline {
    /// Spec §7.3. `history` is active_history (post-epoch, its first row seeded at the epoch); `events` the provider's switch/add/remove events;
    /// `added_at` the fallback start for an account with no Add event, while its row exists.
    pub fn build(events: &[TimelineEvent], history: &[ActiveChange], epoch_ms: i64, added_at: &BTreeMap<AccountId, i64>) -> Timeline;
    pub fn account_at(&self, t: i64) -> Option<AccountId>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Basis { Profile, Timeline, Unattributed }
/// A placement to an account: Home(Profile(a)) -> (Some(a), Profile); Home(Default) -> (timeline.account_at(t), Timeline) or (None, Unattributed) when it has none.
pub fn attribute(placement: &Placement, timeline: &Timeline, t: i64) -> (Option<AccountId>, Basis);

// blocks.rs (Task 5)
pub const BLOCK_MS: i64 = 5 * 3_600_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockSpan { pub start_ms: i64, pub end_ms: i64, pub observed: bool }
/// Distinct resets -> [reset - 5h, reset); resets within 10 min collapse to the latest; sorted; each start clamped up to the previous end.
pub fn observed_blocks(resets_ms: &[i64]) -> Vec<BlockSpan>;
/// `times_ms` sorted ascending. Returns every block that holds at least one time (observed blocks that hold none are dropped),
/// sorted by start, and for each time the index of its block. Inferred blocks follow spec §9.
pub fn assign_blocks(observed: &[BlockSpan], times_ms: &[i64]) -> (Vec<BlockSpan>, Vec<usize>);
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection { pub elapsed_s: i64, pub remaining_s: i64, pub tokens_per_min: f64, pub cost_nanos_per_hour: Option<u64>, pub projected_tokens: u64, pub projected_cost_nanos: Option<u64> }
/// None unless start <= now < end. Rate over max(now - first_response, 60 s).
pub fn project(span: &BlockSpan, first_response_ms: i64, now_ms: i64, tokens: u64, cost_nanos: Option<u64>) -> Option<Projection>;

// bucket.rs (Task 6)
pub fn day_of(ms: i64, tz: &jiff::tz::TimeZone) -> jiff::civil::Date;
pub fn month_of(ms: i64, tz: &jiff::tz::TimeZone) -> (i16, i8);
pub fn day_bounds_ms(date: jiff::civil::Date, tz: &jiff::tz::TimeZone) -> (i64, i64);   // [local midnight, next local midnight)
pub fn month_bounds_ms(year: i16, month: i8, tz: &jiff::tz::TimeZone) -> (i64, i64);
```

`mod.rs` re-exports every public item above (`pub use counts::*; …`).

### `tagteam_provider` (Task 7)

```rust
// transcript.rs
#[derive(Debug, Clone, PartialEq)]
pub struct UsageLine {
    pub message_id: String,
    pub request_id: String,     // "" when absent (ruling R5)
    pub session_id: String,
    pub at_ms: i64,
    pub model: String,
    pub project: Option<String>, // the line's cwd
    pub sidechain: bool,
    pub counts: TokenCounts,
    pub speed: Option<String>,
    pub final_line: bool,        // the line carries a non-null stop_reason
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct LineError(pub String);

// provider.rs
pub struct Capabilities { /* existing */ pub transcripts: bool }
// new trait methods, each with a default:
fn transcript_files(&self, _env: &Env) -> io::Result<Vec<PathBuf>> { Ok(Vec::new()) }
fn parse_usage_line(&self, _line: &[u8]) -> Result<Option<UsageLine>, LineError> { Ok(None) }
fn price_table(&self) -> &'static PriceTable { &tagteam_core::tokens::EMPTY_PRICES }
fn default_session_records_dir(&self, _env: &Env) -> Option<PathBuf> { None }   // ruling R2

// liveness.rs
pub struct SessionRecord { /* existing */ pub session_id: Option<String> }  // from "sessionId"
pub trait ProcessProbe { /* existing */ fn parent_pid(&self, _pid: u32) -> Option<u32> { None } }  // ruling R3
pub struct FakeProcess { /* existing */ pub ppid: Option<u32> }
```

### `tagteam_core::time` (Task 8)

```rust
pub fn parse_iso8601_ms(s: &str) -> Option<i64>; // keeps the fraction; parse_iso8601 shares its parser
```

### `tagteam_fake` (Task 10), for the engine's token tests

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct FakeUsage { pub message_id: String, pub request_id: String, pub session_id: String, pub at_ms: i64, pub model: String, pub project: Option<String>, pub sidechain: bool, pub counts: TokenCounts, pub speed: Option<String>, pub final_line: bool }
impl Default for FakeUsage;   // "m1", "r1", "s1", 1_790_000_000_000 - 3_600_000, "fake-1", Some("/w"), false, {1,2,3,4,5,0}, None, true
impl FakeUsage { pub fn usage_line(&self) -> UsageLine; }
pub fn usage_line_json(u: &FakeUsage) -> String;                      // one line, no newline
pub fn append_transcript(env: &Env, name: &str, text: &str) -> PathBuf; // appends to <fake dir>/transcripts/<name>
pub struct FakePaths { /* existing */ pub transcripts: PathBuf }
impl FakeAgent { pub fn without_transcripts(self) -> Self; }
// FakeAgent: default_session_records_dir = <fake dir>/procs; price table "fake-1" = {1, 2, 3, 4, 0.5}, web search 10.0/1k, as_of "2026-01-01".
```

The CC fixture tree `crates/tagteam-cc/tests/fixtures/transcripts/projects` (Task 8) holds 4 files: 4 responses over 8 usage lines, plus 2 unreadable lines.

### Store (Tasks 11–12), in `tagteam_engine::store`

```rust
pub const SCHEMA_VERSION: i64 = 3;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedLaunch { pub provider: ProviderId, pub account_id: String, pub started_at_ms: i64, pub in_use: bool }
impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError>;  // unchanged signature: a migration to 3 uses the system clock and records seeded = 0
    /// `now` (the engine passes `&|| self.now_ms()`) is read once, after the migration's IMMEDIATE transaction holds the write lock, and dates
    /// the epoch and the seeded active_history rows; `now` and `seed` run only when step 3 is applied, `seed` None = unknown -> seeded = 0.
    pub fn open_seeded(path: &Path, now: &dyn Fn() -> i64, seed: &dyn Fn() -> Option<Vec<SeedLaunch>>) -> Result<Self, StoreError>;
    pub fn open_existing_seeded(path: &Path, now: &dyn Fn() -> i64, seed: &dyn Fn() -> Option<Vec<SeedLaunch>>) -> Result<Option<Self>, StoreError>;
}

// store/tokens.rs (Task 12)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenFileRow { pub path: String, pub dev: u64, pub ino: u64, pub size: i64, pub mtime_ns: i64, pub offset: u64, pub head_hash: [u8; 8], pub tail_hash: [u8; 8] }
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRow { pub key: i64, pub session_id: String, pub project: Option<String>, pub at_ms: i64, pub model: String, pub sidechain: bool, pub counts: TokenCounts, pub speed: Option<String>, pub finalized: bool }
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BatchOutcome { pub added: u64, pub updated: u64 }
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneOutcome { pub deleted: u64, pub complete: bool }
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RefreshStats { pub files: u64, pub responses: u64, pub skipped_lines: u64, pub unreadable_files: u64 } // `responses` is kept by the store (batches add, prunes subtract); finish_token_refresh ignores the input
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenStateRow { pub refreshed_at: Option<i64>, pub changed_at: Option<i64>, pub stats: RefreshStats, pub pruned_at: Option<i64> }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SightingSource { Run, Scan, Statusline }   // 'run' | 'scan' | 'statusline'
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SightingRow { pub session_id: String, pub home: String, pub first_at: i64, pub last_at: i64, pub source: SightingSource, pub recorded_at: i64 } // home: "" default, else account id
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRow { pub account_id: String, pub started_at: i64, pub ended_at: Option<i64>, pub pid: Option<u32>, pub recorded_at: i64 }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpochRow { pub at: i64, pub seeded: bool }
pub fn usage_key(provider: &ProviderId, message_id: &str, request_id: &str) -> i64; // first 8 bytes of SHA-256(provider NUL message_id NUL request_id), big-endian, as i64

impl Store {
    pub fn token_files(&self, provider: &ProviderId) -> Result<Vec<TokenFileRow>, StoreError>;
    /// One IMMEDIATE transaction: upsert rows by the merge rule, write each file row, delete `removed` file rows,
    /// create the provider's token_state row if absent, and set changed_at = now when added + updated > 0.
    pub fn commit_token_batch(&self, provider: &ProviderId, rows: &[UsageRow], files: &[TokenFileRow], removed: &[String], now_ms: i64) -> Result<BatchOutcome, StoreError>;
    /// Sets refreshed_at and stats; inserts token_epoch(now, seeded = 1) if it has no row; on the first completion (refreshed_at was NULL) also sets changed_at.
    pub fn finish_token_refresh(&self, provider: &ProviderId, stats: &RefreshStats, now_ms: i64) -> Result<(), StoreError>;
    /// Daily lease `prune:tokens:<provider>`; deletes usage older than cutoff in chunks, each chunk that deletes any also moving changed_at and
    /// `responses`; then empty sessions, sightings that ended before cutoff, launches ended before cutoff, and pruned_at. `stop` is asked before
    /// each chunk: stopped, the lease is released and `complete: false` (the next call goes on). A lease held or spent today: `{0, complete: true}`.
    pub fn prune_tokens(&self, provider: &ProviderId, cutoff_ms: i64, holder: &str, now_ms: i64, stop: &dyn Fn() -> bool) -> Result<PruneOutcome, StoreError>;
    pub fn token_state(&self, provider: &ProviderId) -> Result<Option<TokenStateRow>, StoreError>;
    pub fn token_epoch(&self) -> Result<Option<EpochRow>, StoreError>;
    /// max(recorded_at) over session_sightings, run_launches and active_history for the provider.
    pub fn evidence_changed_at(&self, provider: &ProviderId) -> Result<Option<i64>, StoreError>;
    pub fn token_usage(&self, provider: &ProviderId, since_ms: Option<i64>, until_ms: Option<i64>, session: Option<&str>) -> Result<Vec<UsageRow>, StoreError>; // ordered by at
    pub fn token_models(&self, provider: &ProviderId) -> Result<Vec<String>, StoreError>; // distinct, sorted
    pub fn token_span(&self, provider: &ProviderId) -> Result<(Option<i64>, Option<i64>), StoreError>; // min/max at
    /// Insert, or extend last_at when the row exists and its last_at <= now - min_gap_ms; spec §6.4. Returns whether it wrote.
    pub fn record_sighting(&self, provider: &ProviderId, s: &SightingRow, min_gap_ms: i64) -> Result<bool, StoreError>;
    /// record_sighting with the busy timeout set to `wait` for the call (the statusline's 5 ms, spec §6.3).
    pub fn record_sighting_within(&self, provider: &ProviderId, s: &SightingRow, min_gap_ms: i64, wait: Duration) -> Result<bool, StoreError>;
    pub fn set_sighting_last_at(&self, provider: &ProviderId, session_id: &str, home: &str, first_at: i64, last_at: i64, now_ms: i64) -> Result<(), StoreError>;
    pub fn latest_sighting(&self, provider: &ProviderId, session_id: &str, home: &str) -> Result<Option<SightingRow>, StoreError>;
    pub fn sightings(&self, provider: &ProviderId) -> Result<Vec<SightingRow>, StoreError>;
    pub fn open_launch(&self, provider: &ProviderId, account_id: &str, started_at: i64, now_ms: i64) -> Result<(), StoreError>;
    pub fn attach_launch_pid(&self, provider: &ProviderId, account_id: &str, started_at: i64, pid: u32, now_ms: i64) -> Result<(), StoreError>;
    pub fn close_launch(&self, provider: &ProviderId, account_id: &str, started_at: i64, ended_at: i64, now_ms: i64) -> Result<(), StoreError>;
    pub fn launches(&self, provider: &ProviderId) -> Result<Vec<LaunchRow>, StoreError>;
    pub fn active_history(&self, provider: &ProviderId) -> Result<Vec<(i64, Option<String>)>, StoreError>; // (at, account_id) by at
}
```

### Settings (Task 13)

```rust
pub const DEFAULT_TOKENS_RETENTION_DAYS: u32 = 365;
pub struct Settings { /* existing */ pub tokens_retention_days: u32, pub prices: BTreeMap<ProviderId, BTreeMap<String, PriceOverride>> } // every [provider.<id>.prices] table, whichever provider was loaded
```

### Engine (Tasks 14–18), in `tagteam_engine::tokens`

```rust
// mod.rs (Task 14): `mod evidence; mod reader; mod refresh; mod view; pub use evidence::*; pub use refresh::*; pub use view::*;`
// Task 14's placeholders each hold one final item (clippy rejects a glob re-export of an empty module): refresh.rs `Wait`, evidence.rs `LaunchId`, view.rs `TokenPeriod`.
pub fn tokens_lock_path(env: &Env, provider: &ProviderId) -> PathBuf; // data_dir/locks/tokens-<id>.lock, same fallback naming as auto::engine_lock_path
pub(crate) fn lock_until_free(path: &Path, cancel: &Cancel) -> Result<FlockGuard, LockError>; // Task 17; waits in one-hour rounds until free or cancelled

// refresh.rs (Task 15)
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Wait { Wait, Try }
#[derive(Debug, Clone, PartialEq, Eq)] pub enum RefreshOutcome { Done(IndexReport), Busy }
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)] pub struct IndexProgress { pub files_done: u64, pub files_total: u64, pub bytes_done: u64, pub bytes_total: u64 }
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)] pub struct IndexReport { pub complete: bool, pub files_read: u64, pub responses_added: u64, pub responses_updated: u64, pub skipped_lines: u64, pub evidence_changed: u64, pub took_ms: u64 }
impl Engine {
    pub fn refresh_tokens(&self, provider: &ProviderId, wait: Wait, budget: Option<Duration>, cancel: &Cancel, progress: &mut dyn FnMut(&IndexProgress)) -> Result<RefreshOutcome, EngineError>;
}

// evidence.rs (Task 16)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchId { pub provider: ProviderId, pub account: AccountId, pub started_at_ms: i64 }
/// The poller's own state, owned by `run`: the record's last mtime and the sighting it wrote last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchPoll { pub last_mtime_ns: Option<i128>, pub last_sighting: Option<(String, i64)> } // (session_id, first_at)
impl Engine {
    pub fn sight_sessions(&self, provider: &ProviderId) -> Result<u64, EngineError>;
    /// The scan of spec §6.2 plus closing idle launches; refresh step 1 (Task 15, which runs after Task 16) calls it.
    pub(crate) fn scan_sessions(&self, p: &dyn Provider) -> Result<u64, EngineError>;
    pub fn open_launch(&self, provider: &ProviderId, account: &AccountId) -> Result<LaunchId, EngineError>;          // before the spawn
    pub fn attach_launch_pid(&self, id: &LaunchId, pid: u32) -> Result<(), EngineError>;
    pub fn poll_launch(&self, id: &LaunchId, profile: &Path, pid: u32, state: &mut LaunchPoll) -> Result<(), EngineError>; // every 2 s while the child runs
    pub fn close_launch(&self, id: &LaunchId, profile: &Path, state: &LaunchPoll, exited_at_ms: i64) -> Result<(), EngineError>; // after finish_run: sighting ends at exited_at_ms (the child's exit, read right after the wait), launch closes now if idle
    /// The statusline sighting (spec §6.3). `profile`: the run-shell profile, or None for the default home. Never waits more than CACHE_WRITE_WAIT.
    pub fn statusline_sighting(&self, provider: &ProviderId, session_id: &str, profile: Option<&Path>) -> Result<(), EngineError>;
}

// view.rs (Task 18): exactly spec §10's types (TokenPeriod, TokenGrouping, AccountSel, TokenQuery, TokenReport,
// TokenRow, PeriodKey, ActiveBlock, RowAccount, RowKey, Cost, TokenTotal, IndexState), with `Basis` and
// `TokenCounts` re-exported from tagteam_core::tokens.
impl Engine {
    pub fn token_report(&self, q: &TokenQuery) -> Result<TokenReport, EngineError>;
    pub fn token_index_state(&self, provider: &ProviderId) -> Result<IndexState, EngineError>;
}
impl TokenQuery { pub fn drill(&self, row: &TokenRow) -> Option<TokenQuery>; }
impl RowKey { pub fn as_str(&self) -> &str; }   // plus Display; TokenTotal: Default; every view type derives Debug
// view lives in tokens/view/{mod.rs, group.rs} (Task 18 git-mv's Task 14's view.rs); tagteam-engine gains jiff.workspace = true in Task 18.
```

## Tasks and batches

| Batch | Tasks (run together; disjoint files) |
|---|---|
| A | 1, 2, 11 |
| B | 3, 4, 5, 6, 12 |
| C | 7, 13 |
| D | 8, 10, 14 |
| E | 9, 16, 17 |
| F | 15, 18 |
| G | 19 |
| H | 20 |
| I | 21 |
| J | 22, 23 |
| K | 24 |

Tasks 19, 20, 21 and 22 each change the `tagteam` crate, so they run one batch at a time: a half-written file in one would break the others' builds.

---

### Task 1: Compat check for transcripts and session records (answers spec §17 T1)

**Depends on:** none
**Files:**
- Create: `crates/xtask/src/compat/checks/transcripts.rs` (two checks and their helpers)
- Modify: `crates/xtask/src/compat/checks/mod.rs` (`mod transcripts;`, two `Phase::Profile` checks registered after `storage-write-lock`, their ids in `every_check_is_listed_once_in_phase_order`)
- Modify, only after a live run: `docs/superpowers/specs/2026-10-14-tagteam-token-report-design.md` (§17 T1 gains one sentence: the observed behaviour and the CC version)
- Test: `crates/xtask/src/compat/checks/transcripts.rs` (unit tests of the helpers), `crates/xtask/src/compat/checks/mod.rs` (the id list)

**Interfaces:**
- Consumes: nothing from this plan. From the existing harness: `super::{records, new_record, read_json}` (`checks/mod.rs`), `Ctx::{profile_ready, run_profile, profile_pty}`, `ctx::{MODEL, PROMPT}`, `report::{Outcome, Probe}`, `sys::{HarnessError, pause, wait_until}`, `Pty::{line, transcript, finish}`, `keychain::random_hex` (tests). A record's `sessionId` is read from its raw JSON, because `SessionRecord.session_id` only arrives with Task 7.
- Produces: the compat checks `transcript-usage-fields` and `session-record-session-id` (`Phase::Profile`, not opt-in, not macOS-only), and the §17 T1 finding recorded in the spec. No Rust API that a later task uses.

Like every compat check, the two checks are verified only by a live run against the real `claude` and the dedicated test account; their helpers are unit-tested here, as `checks/mod.rs` says of the others. The evidence they record is field names, counts and model names, never transcript content.

- [ ] **Step 1: Write the failing tests**

Create `crates/xtask/src/compat/checks/transcripts.rs` with its module comment and its tests only:

```rust
//! The token report's compatibility checks (token report §15, §17 T1): a real request's
//! transcript carries every field the token index reads, and a session record carries the
//! `sessionId` its sightings key on, with what `/clear` does to it. Evidence is field names,
//! counts and model names only, never a transcript's content.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::keychain::random_hex;

    fn audit(text: &str) -> Audit {
        let mut a = Audit::default();
        a.add(text);
        a
    }

    /// A response as CC writes it, with no real content: a user line, the response's stale
    /// snapshot and its final line, a `<synthetic>` line, and a line that is not JSON.
    fn transcript() -> String {
        let usage = json!({
            "input_tokens": 3,
            "output_tokens": 8,
            "cache_creation_input_tokens": 1200,
            "cache_read_input_tokens": 40000,
            "cache_creation": {"ephemeral_5m_input_tokens": 1200, "ephemeral_1h_input_tokens": 0},
        });
        let line = |stop: Value, usage: &Value| {
            json!({
                "type": "assistant",
                "requestId": "req_1",
                "sessionId": "s1",
                "timestamp": "2026-10-14T10:00:00.000Z",
                "cwd": "/w",
                "isSidechain": false,
                "message": {
                    "id": "msg_1",
                    "model": "claude-haiku-4-5-20251001",
                    "stop_reason": stop,
                    "usage": usage,
                    "content": [{"type": "text", "text": "x"}],
                },
            })
            .to_string()
        };
        let mut last = usage.clone();
        last["output_tokens"] = json!(333);
        last["speed"] = json!("standard");
        [
            json!({"type": "user", "sessionId": "s1", "message": {"content": "x"}}).to_string(),
            line(Value::Null, &usage),
            line(json!("end_turn"), &last),
            json!({"type": "assistant", "message": {"id": "m2", "model": "<synthetic>"}})
                .to_string(),
            "{not json".to_owned(),
        ]
        .join("\n")
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask-{name}-{}", random_hex().unwrap()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_audit_counts_lines_finals_and_models() {
        let a = audit(&transcript());
        assert_eq!(a.lines, 2, "the synthetic line is left out");
        assert!(
            a.missing.is_empty() && a.missing_on_final.is_empty(),
            "{a:?}"
        );
        assert_eq!(a.finals, 1, "a null stop_reason is no final line");
        assert_eq!(
            a.models,
            BTreeSet::from(["claude-haiku-4-5-20251001".to_owned()])
        );
        assert_eq!(a.unparsed, 1);
        assert_eq!(
            a.optional_on_final,
            BTreeMap::from([
                ("/message/usage/iterations", 0),
                ("/message/usage/server_tool_use/web_search_requests", 0),
                ("/message/usage/speed", 1),
            ])
        );
    }

    #[test]
    fn a_field_missing_or_of_another_type_is_named_with_its_count() {
        let final_line = transcript().lines().nth(2).unwrap().to_owned();
        let mut v: Value = serde_json::from_str(&final_line).unwrap();
        v.as_object_mut().unwrap().remove("requestId");
        v["message"]["usage"]["output_tokens"] = json!(-1);
        v["isSidechain"] = json!("no");
        v["message"]["usage"]["cache_creation"] = Value::Null;
        let a = audit(&format!("{v}\n{v}\n"));
        assert_eq!(
            a.missing,
            BTreeMap::from([("/message/usage/output_tokens", 2), ("/requestId", 2)])
        );
        assert_eq!(
            a.missing_on_final,
            BTreeMap::from([
                ("/isSidechain", 2),
                ("/message/usage/cache_creation/ephemeral_1h_input_tokens", 2),
                ("/message/usage/cache_creation/ephemeral_5m_input_tokens", 2),
            ])
        );
    }

    #[test]
    fn only_session_transcripts_are_listed_and_growth_is_found() {
        let root = scratch("transcripts");
        let project = root.join("-w");
        fs::create_dir_all(project.join("s1/subagents")).unwrap();
        fs::write(project.join("s1.jsonl"), "a\n").unwrap();
        fs::write(
            project.join("s1/subagents/agent-0123456789abcdef.jsonl"),
            "b\n",
        )
        .unwrap();
        fs::write(project.join("notes.txt"), "c\n").unwrap();
        let before = transcript_files(&root);
        assert_eq!(before, BTreeMap::from([(project.join("s1.jsonl"), 2)]));
        assert!(transcript_files(&root.join("missing")).is_empty());
        fs::write(project.join("s1.jsonl"), "a\nb\n").unwrap();
        fs::write(project.join("s2.jsonl"), "").unwrap();
        let after = transcript_files(&root);
        assert_eq!(
            grown(&before, &after),
            [(project.join("s1.jsonl"), 2), (project.join("s2.jsonl"), 0)]
        );
        assert!(grown(&after, &after).is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_answer_is_found_only_past_where_its_transcript_was() {
        let root = scratch("answers");
        let project = root.join("-w");
        fs::create_dir_all(&project).unwrap();
        let text = transcript();
        fs::write(project.join("s1.jsonl"), &text).unwrap();
        let before = transcript_files(&root);
        assert!(answered_in(&before, &before).is_empty());
        // s1 gains a user line only; s2 is new and holds a final line.
        fs::write(
            project.join("s1.jsonl"),
            format!("{text}\n{{\"type\":\"user\"}}\n"),
        )
        .unwrap();
        fs::write(project.join("s2.jsonl"), &text).unwrap();
        assert_eq!(answered_in(&before, &transcript_files(&root)), ["s2"]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_record_s_session_id_is_read_from_its_json() {
        let dir = scratch("records");
        fs::write(dir.join("1.json"), r#"{"pid":1,"sessionId":"abc"}"#).unwrap();
        fs::write(dir.join("2.json"), r#"{"pid":2}"#).unwrap();
        fs::write(dir.join("3.json"), r#"{"pid":3,"sessi"#).unwrap();
        assert_eq!(
            record_session_id(&dir.join("1.json")).as_deref(),
            Some("abc")
        );
        assert_eq!(record_session_id(&dir.join("2.json")), None);
        assert_eq!(
            record_session_id(&dir.join("3.json")),
            None,
            "partly written"
        );
        assert_eq!(record_session_id(&dir.join("4.json")), None, "gone");
        fs::remove_dir_all(&dir).unwrap();
    }
}
```

In `crates/xtask/src/compat/checks/mod.rs`, declare the module after `mod standalone;`:

```rust
mod live;
mod opt_in;
mod profile;
mod standalone;
mod transcripts;
```

and in `tests::every_check_is_listed_once_in_phase_order` add the two ids after `"storage-write-lock"`:

```rust
                "session-records",
                "storage-write-lock",
                "transcript-usage-fields",
                "session-record-session-id",
                "fresh-global-config",
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p xtask --lib --locked compat::checks`

Expected: compilation fails in `transcripts.rs` (``could not compile `xtask` (lib test)``), with ``error: cannot find macro `json` in this scope``, `error[E0412]: cannot find type` for `Audit`, `Value` and `PathBuf`, `error[E0425]: cannot find function` for `transcript_files`, `grown`, `answered_in` and `record_session_id`, and `error[E0433]: failed to resolve` for `BTreeMap`, `BTreeSet` and `fs`. (Once it compiles, `every_check_is_listed_once_in_phase_order` would still fail until Step 3 registers the checks.)

- [ ] **Step 3: Implement**

In `crates/xtask/src/compat/checks/transcripts.rs`, replace everything above `#[cfg(test)]` (the module comment) with the following, then one blank line:

```rust
//! The token report's compatibility checks (token report §15, §17 T1): a real request's
//! transcript carries every field the token index reads, and a session record carries the
//! `sessionId` its sightings key on, with what `/clear` does to it. Evidence is field names,
//! counts and model names only, never a transcript's content.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use super::{new_record, read_json, records};
use crate::compat::ctx::{Ctx, MODEL, PROMPT};
use crate::compat::report::{Outcome, Probe};
use crate::compat::sys::{HarnessError, pause, wait_until};

const REQUEST: [&str; 6] = ["-p", PROMPT, "--model", MODEL, "--max-turns", "1"];

/// What every assistant line carries that the index needs to read it (token report §3.2,
/// §5.4), as JSON pointers.
const REQUIRED: [&str; 9] = [
    "/requestId",
    "/sessionId",
    "/timestamp",
    "/message/id",
    "/message/model",
    "/message/usage/input_tokens",
    "/message/usage/output_tokens",
    "/message/usage/cache_creation_input_tokens",
    "/message/usage/cache_read_input_tokens",
];

/// What a response's final line, the one with a `stop_reason`, carries besides (token report
/// §2.1).
const ON_FINAL: [&str; 4] = [
    "/cwd",
    "/isSidechain",
    "/message/usage/cache_creation/ephemeral_5m_input_tokens",
    "/message/usage/cache_creation/ephemeral_1h_input_tokens",
];

/// What only a final line may carry (token report §2.2): recorded, never required.
const OPTIONAL: [&str; 3] = [
    "/message/usage/speed",
    "/message/usage/iterations",
    "/message/usage/server_tool_use/web_search_requests",
];

/// Whether `v` has the field at `pointer` as the index reads it: a token count is an unsigned
/// integer, `isSidechain` a boolean, anything else a string. A null `stop_reason` is absent.
fn has(v: &Value, pointer: &str) -> bool {
    match v.pointer(pointer) {
        Some(n) if pointer.ends_with("_tokens") => n.as_u64().is_some(),
        Some(b) if pointer == "/isSidechain" => b.is_boolean(),
        Some(s) => s.is_string(),
        None => false,
    }
}

/// What a transcript's assistant lines carry, by field name and count.
#[derive(Debug, Default, PartialEq, Eq)]
struct Audit {
    /// Assistant lines, `<synthetic>` ones left out (token report §2.2).
    lines: usize,
    /// Of `REQUIRED`, how many lines lack each field that some line lacks.
    missing: BTreeMap<&'static str, usize>,
    /// Lines with a `stop_reason`.
    finals: usize,
    /// Of `ON_FINAL`, how many final lines lack each field that some final line lacks.
    missing_on_final: BTreeMap<&'static str, usize>,
    /// Of `OPTIONAL`, how many final lines carry each.
    optional_on_final: BTreeMap<&'static str, usize>,
    models: BTreeSet<String>,
    /// Lines that are not JSON.
    unparsed: usize,
}

impl Audit {
    fn add(&mut self, text: &str) {
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                self.unparsed += 1;
                continue;
            };
            let model = v.pointer("/message/model").and_then(Value::as_str);
            if v["type"] != "assistant" || model == Some("<synthetic>") {
                continue;
            }
            self.lines += 1;
            if let Some(model) = model {
                self.models.insert(model.to_owned());
            }
            for field in REQUIRED.into_iter().filter(|f| !has(&v, f)) {
                *self.missing.entry(field).or_default() += 1;
            }
            if !has(&v, "/message/stop_reason") {
                continue;
            }
            self.finals += 1;
            for field in ON_FINAL.into_iter().filter(|f| !has(&v, f)) {
                *self.missing_on_final.entry(field).or_default() += 1;
            }
            for field in OPTIONAL {
                let seen = self.optional_on_final.entry(field).or_default();
                if v.pointer(field).is_some() {
                    *seen += 1;
                }
            }
        }
    }

    /// What was appended to `path` past its first `from` bytes.
    fn add_appended(&mut self, path: &Path, from: u64) -> io::Result<()> {
        let bytes = fs::read(path)?;
        let from = usize::try_from(from).map_or(bytes.len(), |n| n.min(bytes.len()));
        self.add(&String::from_utf8_lossy(&bytes[from..]));
        Ok(())
    }
}

/// The session transcripts under `projects`, `<project>/<sessionId>.jsonl` (token report
/// §2.1), with their lengths. Subagent transcripts, a level deeper, are left out.
fn transcript_files(projects: &Path) -> BTreeMap<PathBuf, u64> {
    let mut out = BTreeMap::new();
    let Ok(dirs) = fs::read_dir(projects) else {
        return out;
    };
    for dir in dirs.flatten() {
        let Ok(files) = fs::read_dir(dir.path()) else {
            continue;
        };
        for path in files.flatten().map(|f| f.path()) {
            let Ok(m) = fs::metadata(&path) else {
                continue;
            };
            if m.is_file() && path.extension().is_some_and(|x| x == "jsonl") {
                out.insert(path, m.len());
            }
        }
    }
    out
}

/// The transcripts that are new in `after` or longer than in `before`, each with its length
/// before (0 for a new one).
fn grown(before: &BTreeMap<PathBuf, u64>, after: &BTreeMap<PathBuf, u64>) -> Vec<(PathBuf, u64)> {
    after
        .iter()
        .filter(|(path, len)| before.get(*path).is_none_or(|was| *len > was))
        .map(|(path, _)| (path.clone(), before.get(path).copied().unwrap_or(0)))
        .collect()
}

/// The sessions, by their transcripts' names, that a final assistant line was written to
/// between `before` and `after`: where an answer went.
fn answered_in(before: &BTreeMap<PathBuf, u64>, after: &BTreeMap<PathBuf, u64>) -> Vec<String> {
    grown(before, after)
        .into_iter()
        .filter(|(path, from)| {
            let mut a = Audit::default();
            a.add_appended(path, *from).is_ok() && a.finals > 0
        })
        .filter_map(|(path, _)| Some(path.file_stem()?.to_str()?.to_owned()))
        .collect()
}

/// The `sessionId` of the session record at `path`, read from its JSON; `None` while it is
/// partly written, or gone.
fn record_session_id(path: &Path) -> Option<String> {
    read_json(path)?["sessionId"].as_str().map(str::to_owned)
}

/// Token report §15: a real request's transcript has every field the index reads on every
/// assistant line, and its final line has the rest.
pub fn transcript_usage_fields(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    Ok(Probe::run(|p| {
        let (_, spelling) = ctx.profile_ready()?;
        let projects = Path::new(&spelling).join("projects");
        let before = transcript_files(&projects);
        let ran = ctx.run_profile(&REQUEST)?;
        p.expect("claude -p succeeded", ran.success(), ran.summary());
        let written = grown(&before, &transcript_files(&projects));
        p.expect(
            "the request was written to a session transcript under projects/",
            !written.is_empty(),
            json!(written.len()),
        );
        let mut a = Audit::default();
        for (path, from) in &written {
            a.add_appended(path, *from)?;
        }
        p.expect("an assistant line was written", a.lines > 0, json!(a.lines));
        p.expect(
            "every assistant line has the ids, time, model and counts the index reads",
            a.missing.is_empty(),
            json!(a.missing),
        );
        p.expect(
            "a final line, with a stop_reason, was written",
            a.finals > 0,
            json!(a.finals),
        );
        p.expect(
            "every final line has cwd, isSidechain and the 5m/1h cache-write split",
            a.missing_on_final.is_empty(),
            json!(a.missing_on_final),
        );
        p.note("the models", json!(a.models));
        p.note(
            "optional fields on final lines (information only)",
            json!(a.optional_on_final),
        );
        p.note("lines that are not JSON", json!(a.unparsed));
        Ok("a real request's transcript carries every field the token index reads")
    }))
}

/// Token report §17 T1: a session record carries a `sessionId` that names the session's
/// transcript, and what `/clear` does to it: whether the record's `sessionId` changes, and
/// whether the conversation after it is written to the transcript the record then names. The
/// check passes whatever `/clear` does; the notes are the finding.
pub fn session_record_session_id_and_clear(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    Ok(Probe::run(|p| {
        let (_, spelling) = ctx.profile_ready()?;
        let home = Path::new(&spelling);
        let sessions = home.join("sessions");
        let projects = home.join("projects");
        let before = records(&sessions);
        let mut pty = ctx.profile_pty(&["--model", MODEL])?;
        let Some((record, _)) = new_record(&sessions, &before, Duration::from_secs(60)) else {
            p.expect(
                "a session record at the start",
                false,
                json!(pty.transcript(600)),
            );
            pty.finish(Duration::from_secs(5))?;
            return Ok("no session record was written");
        };
        let first = record_session_id(&record);
        p.expect(
            "the record carries a string sessionId",
            first.is_some(),
            json!(null),
        );

        // A conversation to clear: one prompt, answered in the transcript the record names.
        pause(Duration::from_secs(2));
        let files = transcript_files(&projects);
        pty.line(PROMPT)?;
        let mut answered = Vec::new();
        wait_until(Duration::from_secs(90), || {
            answered = answered_in(&files, &transcript_files(&projects));
            !answered.is_empty()
        });
        p.expect(
            "the prompt was answered",
            !answered.is_empty(),
            json!(answered.len()),
        );
        p.expect(
            "the answer is in the transcript the record's sessionId names",
            first.as_ref().is_some_and(|id| answered.contains(id)),
            json!(null),
        );

        // `/clear`, then the record, watched for 10 s. A record caught mid-write keeps the
        // last id read.
        pty.line("/clear")?;
        let mut now = first.clone();
        let changed = wait_until(Duration::from_secs(10), || {
            now = record_session_id(&record).or_else(|| now.clone());
            now != first
        });
        let new_records = records(&sessions)
            .into_iter()
            .filter(|(path, _)| *path != record && !before.iter().any(|(b, _)| b == path))
            .count();
        p.note(
            "after /clear: what happened to the record (token report §17 T1)",
            json!({
                "sessionIdChanged": changed,
                "recordFileKept": record.exists(),
                "newRecordFiles": new_records,
            }),
        );

        // The next prompt: which transcript the answer goes to, against what the record says.
        pause(Duration::from_secs(2));
        let files = transcript_files(&projects);
        pty.line(PROMPT)?;
        let mut answered = Vec::new();
        wait_until(Duration::from_secs(90), || {
            answered = answered_in(&files, &transcript_files(&projects));
            !answered.is_empty()
        });
        p.expect(
            "after /clear: the next prompt was answered",
            !answered.is_empty(),
            json!(answered.len()),
        );
        let named = record_session_id(&record).or(now);
        p.note(
            "after /clear: where the next answer was written (token report §17 T1)",
            json!({
                "aNewTranscript": answered.iter().any(|s| Some(s) != first.as_ref()),
                "theTranscriptTheRecordNames": answered.iter().any(|s| Some(s) == named.as_ref()),
            }),
        );
        let _ = pty.line("/exit");
        p.note(
            "the session's exit",
            json!(pty.finish(Duration::from_secs(30))?),
        );
        Ok("the record carries sessionId; what /clear does to it is in the notes")
    }))
}
```

In `crates/xtask/src/compat/checks/mod.rs`, register both checks in `CHECKS` between `storage-write-lock` and `fresh-global-config` (the last profile check and the first live one; `every_check_is_listed_once_in_phase_order` also asserts phase order). The hunk, with its context:

```rust
    check(
        "storage-write-lock",
        "CC waits for tagteam's storage-write lock, and tagteam for CC's",
        Phase::Profile,
        false,
        None,
        profile::storage_write_lock,
    ),
    check(
        "transcript-usage-fields",
        "a real request's transcript carries every field the token index reads (token report §3.2, §15)",
        Phase::Profile,
        false,
        None,
        transcripts::transcript_usage_fields,
    ),
    check(
        "session-record-session-id",
        "a session record's `sessionId` names its transcript, and what `/clear` does to it (token report §17 T1)",
        Phase::Profile,
        false,
        None,
        transcripts::session_record_session_id_and_clear,
    ),
    check(
        "fresh-global-config",
```

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p xtask --lib --locked compat::checks`

Expected: every test passes, among them the five new ones (`an_audit_counts_lines_finals_and_models`, `a_field_missing_or_of_another_type_is_named_with_its_count`, `only_session_transcripts_are_listed_and_growth_is_found`, `an_answer_is_found_only_past_where_its_transcript_was`, `a_record_s_session_id_is_read_from_its_json`) and `every_check_is_listed_once_in_phase_order`: `test result: ok. 34 passed; 0 failed`.

Then the live run, which needs the real `claude`, the dedicated compat test account and a terminal (pty). Run it outside the sandbox:

```sh
cargo xtask compat --only transcript-usage-fields --only session-record-session-id
```

Expected: exit 0, both checks `pass`, and the report in `target/compat/report.md` and `target/compat/report.json` (`claudeVersion` is the CC version). If setup stops with ``no compat store in …: run `cargo xtask compat login` first`` or ``the compat store has no compat-oauth account: run `cargo xtask compat login` ``, the account is not set up: report this step BLOCKED, leave §17 T1 as it is, and commit the checks alone (Step 6 without the spec). The controller asks Michael to run `cargo xtask compat login` once and then the command above. Any other failure is a finding: read the failed expectation in the report before changing code.

On a run, record the finding in the spec. The `session-record-session-id` check leaves two notes: `after /clear: what happened to the record` (`sessionIdChanged`) and `after /clear: where the next answer was written` (`aNewTranscript`, `theTranscriptTheRecordNames`). Pick `<finding>` by them:

| `sessionIdChanged` | next answer | `<finding>` |
|---|---|---|
| `true` | `theTranscriptTheRecordNames: true` | ``/clear` gives the session record a new `sessionId`, and the conversation after it is written to the transcript the new one names, so sightings follow it`` |
| `true` | `theTranscriptTheRecordNames: false` | ``/clear` gives the session record a new `sessionId` that does not name the transcript the conversation after it is written to`` |
| `false` | `aNewTranscript: true` | ``/clear` keeps the record's `sessionId` while the conversation after it is written to a new transcript, so §7.4's fallback applies`` |
| `false` | `aNewTranscript: false` | ``/clear` changes neither the record's `sessionId` nor the transcript the conversation is written to`` |

In `docs/superpowers/specs/2026-10-14-tagteam-token-report-design.md`, replace

```markdown
- **T1.** Whether a session record's `sessionId` changes on `/clear`. The plan verifies it
  first. If it does not change, profile sessions after `/clear` are placed by the statusline
  or left unattributed (§7.4).
```

with the following, `<claudeVersion>` from `report.json` (for example `2.1.299`), `<run date>` the day of the run (`YYYY-MM-DD`), and the bullet rewrapped under 100 columns with its two-space continuation indent:

```markdown
- **T1.** Whether a session record's `sessionId` changes on `/clear`. The plan verifies it
  first. If it does not change, profile sessions after `/clear` are placed by the statusline
  or left unattributed (§7.4). Observed with Claude Code <claudeVersion> on <run date>
  (`cargo xtask compat`, check `session-record-session-id`): <finding>.
```

A point-in-time fact: nothing else in the spec changes, and its **Status** stays `Approved`.

- [ ] **Step 5: Lint**

```sh
cargo fmt --all --check
cargo clippy -p xtask --all-targets --locked -- -D warnings
```

Expected: no output from `fmt`; clippy finishes with no warning.

- [ ] **Step 6: Commit**

After a live run:

```sh
git add crates/xtask/src/compat/checks/transcripts.rs crates/xtask/src/compat/checks/mod.rs docs/superpowers/specs/2026-10-14-tagteam-token-report-design.md
git commit -m "Add compat checks for transcript usage fields and session records"
```

When the live run is BLOCKED, the same without the spec:

```sh
git add crates/xtask/src/compat/checks/transcripts.rs crates/xtask/src/compat/checks/mod.rs
git commit -m "Add compat checks for transcript usage fields and session records"
```

#### Notes for the controller

- **Check ids are kebab-case.** Every id in `CHECKS` is kebab-case (`auth-status`, `storage-write-lock`), so the outline's `transcript_usage_fields` and `session_record_session_id_and_clear` are the *function* names here and the ids are `transcript-usage-fields` and `session-record-session-id`. The `--only` command above uses the ids.
- **What the T1 check settles.** Beyond the outline it sends one prompt before `/clear` (a fresh conversation may have nothing to clear) and one after, so the note says not only whether the record's `sessionId` changed but whether the conversation after `/clear` lands in the transcript the record names. That second fact is what spec §7.4's placement depends on. It costs two minimal `haiku` requests.
- **Where the transcript is found.** Instead of "the newest `*.jsonl`", the checks compare the session transcripts' lengths before and after (`grown`), so an older session in the shared `projects/` tree can never be mistaken for the run's.
- **Strictness.** `transcript-usage-fields` requires on *every* assistant line the fields whose absence makes Task 8's parser skip the line or mis-key it (ids, time, model, the four counts), and on every final line `cwd`, `isSidechain` and the 5m/1h split. `speed`, `iterations` and `server_tool_use.web_search_requests` are recorded, never required (spec §2.2: final lines only, and not always).

---

### Task 2: Core token counts and the merge rule

**Depends on:** none
**Files:**
- Modify: `Cargo.toml` (`jiff` in `[workspace.dependencies]`, pinned to `=0.2.37`, three features)
- Modify: `Cargo.lock` (written by `cargo fetch`)
- Modify: `crates/tagteam-core/Cargo.toml` (`jiff.workspace = true`)
- Modify: `crates/tagteam-core/src/lib.rs` (`pub mod tokens;`)
- Create: `crates/tagteam-core/src/tokens/mod.rs` (its final form: the five modules, their glob re-exports, the test-only `pinned` config)
- Create: `crates/tagteam-core/src/tokens/counts.rs` (`TokenCounts` and its tests)
- Create: `crates/tagteam-core/src/tokens/price.rs`, `attribution.rs`, `blocks.rs`, `bucket.rs` (stubs that Tasks 3–6 replace whole; each holds one final contract item, see the notes)
- Test: `crates/tagteam-core/src/tokens/counts.rs`

**Interfaces:**
- Consumes: none from this plan (`crate::ids::AccountId`, existing).
- Produces:

```rust
// tagteam_core::tokens (mod.rs re-exports every public item of its five modules)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
    pub web_search: u64, // requests, not tokens
}
impl TokenCounts {
    pub fn cache_write(&self) -> u64;           // 5m + 1h, saturating
    pub fn prompt(&self) -> u64;                // input + cache_write() + cache_read, saturating
    pub fn total(&self) -> u64;                 // input + output + cache_write() + cache_read, saturating
    pub fn merge_max(&mut self, other: &TokenCounts); // field-wise max: spec §2.2
    pub fn add(&mut self, other: &TokenCounts); // field-wise saturating sum
}
// Already final in the stubs, kept verbatim by Tasks 3-6:
pub enum Home { Default, Profile(AccountId) }          // attribution.rs (Task 4)
pub const BLOCK_MS: i64 = 5 * 3_600_000;               // blocks.rs (Task 5)
pub fn day_of(ms: i64, tz: &jiff::tz::TimeZone) -> jiff::civil::Date; // bucket.rs (Task 6)
pub fn strip_date_suffix(model: &str) -> &str;         // price.rs (Task 3)
// Test-only, crate-private, for the property tests of Tasks 2, 4 and 5:
#[cfg(test)] pub(crate) fn tokens::test_support::pinned(cases: u32) -> proptest::test_runner::Config;
```

  and the workspace dependency `jiff = "=0.2.37"` (features `std`, `tz-system`, `tzdb-zoneinfo`), which a later crate takes with `jiff.workspace = true`.

- [ ] **Step 1: Write the failing tests**

First the dependency. In the root `Cargo.toml`, add `jiff` to `[workspace.dependencies]` between `hex` and `libc`:

```toml
hex = "0.4"
# The token report's days and months in a time zone, DST included (token report §3.1): the
# system zone database, nothing bundled. Pinned exactly, so a newer release is taken on purpose.
jiff = { version = "=0.2.37", default-features = false, features = ["std", "tz-system", "tzdb-zoneinfo"] }
libc = "0.2"
```

In `crates/tagteam-core/Cargo.toml`:

```toml
[dependencies]
hex.workspace = true
jiff.workspace = true
serde_json.workspace = true
```

Then, outside the sandbox (it blocks the registry), fetch and check the lockfile:

```sh
cargo fetch
```

Expected: cargo prints `Adding jiff v0.2.37 (available: v0.2.38)` among its lines. 0.2.38 was published on 2026-10-06, under two weeks before this plan (2026-10-15), so the requirement is exact; 0.2.37 (2026-09-12) is the newest release at least two weeks old. Check:

```sh
rg -n -A1 '^name = "jiff' Cargo.lock
git diff --stat Cargo.lock
```

Expected: `jiff` `0.2.37`, `jiff-core` `0.1.1` and `jiff-static` `0.2.37`; about 100 lines added. The other new lock entries are `portable-atomic` 1.15.0, `portable-atomic-util` 0.2.8, `defmt` 1.1.1, `defmt-macros` 1.1.1, `defmt-parser` 1.0.0 and `bitflags` 1.3.2 (so the existing `bitflags` references gain their version, `bitflags 2.13.2`); all are at least two weeks old, and none of them is compiled on macOS or Linux (they serve jiff's optional features and other targets). `cargo tree -p tagteam-core -e normal` shows only `jiff` and `jiff-core` added.

Declare the module in `crates/tagteam-core/src/lib.rs`, between `time` and `trust`:

```rust
pub mod rotation;
pub mod time;
pub mod tokens;
pub mod trust;
```

Create `crates/tagteam-core/src/tokens/mod.rs` in its final form:

```rust
//! The token report's pure core (token report §3.1): token counts and the merge rule, prices
//! and cost, attribution and the default home's timeline, 5-hour blocks, and day and month
//! bucketing. No I/O; times are epoch milliseconds.

mod attribution;
mod blocks;
mod bucket;
mod counts;
mod price;

pub use attribution::*;
pub use blocks::*;
pub use bucket::*;
pub use counts::*;
pub use price::*;

#[cfg(test)]
pub(crate) mod test_support {
    use proptest::test_runner::{Config, RngAlgorithm, RngSeed};

    /// A fixed seed and `cases` cases, failures not persisted, as in `tests/simulate.rs`: every
    /// run is the same run.
    pub(crate) fn pinned(cases: u32) -> Config {
        Config {
            cases,
            rng_algorithm: RngAlgorithm::ChaCha,
            rng_seed: RngSeed::Fixed(0x746f_6b65_6e73_0000),
            failure_persistence: None,
            ..Config::default()
        }
    }
}
```

Create the four stubs. A glob re-export of an empty module is an `unused_imports` error under `-D warnings`, so each stub holds one item of its module's contract, final, which the task that fills the module keeps verbatim.

`crates/tagteam-core/src/tokens/price.rs`:

```rust
//! Prices and cost (token report §8).

/// `model` without a trailing `-YYYYMMDD`, which a dated model name is priced by when the table
/// has no entry of its own: `claude-haiku-4-5-20251001` as `claude-haiku-4-5` (token report
/// §8.1).
pub fn strip_date_suffix(model: &str) -> &str {
    let b = model.as_bytes();
    match b.len().checked_sub(9) {
        Some(cut) if cut > 0 && b[cut] == b'-' && b[cut + 1..].iter().all(u8::is_ascii_digit) => {
            &model[..cut]
        }
        _ => model,
    }
}
```

`crates/tagteam-core/src/tokens/attribution.rs`:

```rust
//! Attribution (token report §7): which account a response went to, decided at query time.

use crate::ids::AccountId;

/// A Claude Code home a session ran in: the default home, or an account's `run` profile.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Home {
    Default,
    Profile(AccountId),
}
```

`crates/tagteam-core/src/tokens/blocks.rs`:

```rust
//! 5-hour blocks (token report §9).

/// A block's length: the provider's short window, Claude Code's 5 h.
pub const BLOCK_MS: i64 = 5 * 3_600_000;
```

`crates/tagteam-core/src/tokens/bucket.rs`:

```rust
//! Day and month bucketing (token report §10.1).

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};

/// `ms` in `tz`, clamped to the range `jiff` represents.
fn zoned(ms: i64, tz: &TimeZone) -> Zoned {
    let at = Timestamp::from_millisecond(ms).unwrap_or(if ms < 0 {
        Timestamp::MIN
    } else {
        Timestamp::MAX
    });
    at.to_zoned(tz.clone())
}

/// The calendar day of `ms` in `tz`.
pub fn day_of(ms: i64, tz: &TimeZone) -> Date {
    zoned(ms, tz).date()
}
```

Last, `crates/tagteam-core/src/tokens/counts.rs` with its module comment and its tests only:

```rust
//! Token counts, and the rule that merges a response's lines into one (token report §2.2).

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::tokens::test_support::pinned;

    fn counts(input: u64, output: u64, cw5: u64, cw1: u64, read: u64, search: u64) -> TokenCounts {
        TokenCounts {
            input,
            output,
            cache_write_5m: cw5,
            cache_write_1h: cw1,
            cache_read: read,
            web_search: search,
        }
    }

    #[test]
    fn merge_max_takes_each_fields_maximum() {
        // A response's stale snapshot, then its final line (token report §2.2): input and cache
        // counts agree, output grows, and only the final line counts web searches.
        let mut merged = counts(3, 8, 1_200, 0, 40_000, 0);
        merged.merge_max(&counts(3, 333, 1_200, 0, 40_000, 1));
        assert_eq!(merged, counts(3, 333, 1_200, 0, 40_000, 1));
        // Field by field: the larger value wins, whichever line holds it.
        let mut mixed = counts(9, 1, 9, 1, 9, 1);
        mixed.merge_max(&counts(1, 9, 1, 9, 1, 9));
        assert_eq!(mixed, counts(9, 9, 9, 9, 9, 9));
        // A zero line merged with a full one is the full one.
        let mut zero = TokenCounts::default();
        zero.merge_max(&counts(1, 2, 3, 4, 5, 6));
        assert_eq!(zero, counts(1, 2, 3, 4, 5, 6));
    }

    #[test]
    fn prompt_and_total_follow_the_spec() {
        let c = counts(1, 2, 3, 4, 5, 6);
        assert_eq!(c.cache_write(), 3 + 4);
        assert_eq!(c.prompt(), 1 + 7 + 5, "input, cache writes and cache reads");
        assert_eq!(c.total(), 1 + 2 + 7 + 5, "web searches are not tokens");
        let mut sum = c;
        sum.add(&counts(10, 20, 30, 40, 50, 60));
        assert_eq!(sum, counts(11, 22, 33, 44, 55, 66));
    }

    #[test]
    fn totals_saturate_instead_of_overflowing() {
        let max = counts(u64::MAX, u64::MAX, u64::MAX, u64::MAX, u64::MAX, u64::MAX);
        assert_eq!(max.cache_write(), u64::MAX);
        assert_eq!(max.prompt(), u64::MAX);
        assert_eq!(max.total(), u64::MAX);
        assert_eq!(counts(0, 0, u64::MAX, 1, 0, 0).cache_write(), u64::MAX);
        let mut sum = counts(u64::MAX - 1, 1, 0, 0, 0, u64::MAX);
        sum.add(&counts(5, 5, 5, 5, 5, 5));
        assert_eq!(sum, counts(u64::MAX, 6, 5, 5, 5, u64::MAX));
    }

    fn line() -> impl Strategy<Value = TokenCounts> {
        (
            any::<u64>(),
            any::<u64>(),
            any::<u64>(),
            any::<u64>(),
            any::<u64>(),
            any::<u64>(),
        )
            .prop_map(|(a, b, c, d, e, f)| counts(a, b, c, d, e, f))
    }

    fn merged(lines: &[TokenCounts]) -> TokenCounts {
        lines.iter().fold(TokenCounts::default(), |mut acc, l| {
            acc.merge_max(l);
            acc
        })
    }

    proptest! {
        #![proptest_config(pinned(256))]

        /// Token report §15: any order of a response's lines gives the same row.
        #[test]
        fn merge_is_order_independent(
            (lines, shuffled) in prop::collection::vec(line(), 1..=9)
                .prop_flat_map(|lines| (Just(lines.clone()), Just(lines).prop_shuffle()))
        ) {
            let once = merged(&lines);
            prop_assert_eq!(once, merged(&shuffled));
            let reversed: Vec<TokenCounts> = lines.iter().rev().copied().collect();
            prop_assert_eq!(once, merged(&reversed));
            prop_assert_eq!(once.output, lines.iter().map(|l| l.output).max().unwrap());
            // A line read again, from a re-read file or a copy in another, changes nothing.
            let mut again = once;
            again.merge_max(&lines[lines.len() - 1]);
            prop_assert_eq!(again, once);
        }
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-core --locked tokens::counts`

Expected: compilation fails in `counts.rs` (``could not compile `tagteam-core` (lib test)``) with ``error[E0412]: cannot find type `TokenCounts` in this scope``, ``error[E0422]: cannot find struct, variant or union type `TokenCounts` in this scope`` and ``error[E0433]: failed to resolve: use of undeclared type `TokenCounts` ``, plus the warning ``unused import: `counts::*` `` from `mod.rs`.

- [ ] **Step 3: Implement**

In `crates/tagteam-core/src/tokens/counts.rs`, replace everything above `#[cfg(test)]` (the module comment) with the following, then one blank line:

```rust
//! Token counts, and the rule that merges a response's lines into one (token report §2.2).

/// One response's token counts, or a sum of them (token report §2.1, §10). Cache writes are
/// split by lifetime; `web_search` counts requests, not tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
    pub web_search: u64,
}

impl TokenCounts {
    /// Both cache-write lifetimes.
    pub fn cache_write(&self) -> u64 {
        self.cache_write_5m.saturating_add(self.cache_write_1h)
    }

    /// The prompt: input, cache writes and cache reads, which picks a long-prompt rate card
    /// (token report §8.1).
    pub fn prompt(&self) -> u64 {
        self.input
            .saturating_add(self.cache_write())
            .saturating_add(self.cache_read)
    }

    /// Every token, the prompt's and the output's. Web searches are not tokens.
    pub fn total(&self) -> u64 {
        self.prompt().saturating_add(self.output)
    }

    /// Token report §2.2: each count of a response is the maximum over its lines, so the result
    /// is the same whatever order its lines and files are read in.
    pub fn merge_max(&mut self, other: &TokenCounts) {
        self.input = self.input.max(other.input);
        self.output = self.output.max(other.output);
        self.cache_write_5m = self.cache_write_5m.max(other.cache_write_5m);
        self.cache_write_1h = self.cache_write_1h.max(other.cache_write_1h);
        self.cache_read = self.cache_read.max(other.cache_read);
        self.web_search = self.web_search.max(other.web_search);
    }

    /// Field by field, saturating: a row's or a total's counts.
    pub fn add(&mut self, other: &TokenCounts) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_write_5m = self.cache_write_5m.saturating_add(other.cache_write_5m);
        self.cache_write_1h = self.cache_write_1h.saturating_add(other.cache_write_1h);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.web_search = self.web_search.saturating_add(other.web_search);
    }
}
```

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p tagteam-core --locked tokens::counts`

Expected: `merge_max_takes_each_fields_maximum`, `merge_is_order_independent`, `totals_saturate_instead_of_overflowing` and `prompt_and_total_follow_the_spec` pass: `test result: ok. 4 passed; 0 failed`. The property test runs 256 cases from a fixed seed, in well under a second.

- [ ] **Step 5: Lint**

```sh
cargo fmt --all --check
cargo clippy -p tagteam-core --all-targets --locked -- -D warnings
```

Expected: no output from `fmt`; clippy finishes with no warning (the stubs keep every glob re-export in use).

- [ ] **Step 6: Commit**

```sh
git add Cargo.toml Cargo.lock crates/tagteam-core/Cargo.toml crates/tagteam-core/src/lib.rs crates/tagteam-core/src/tokens/mod.rs crates/tagteam-core/src/tokens/counts.rs crates/tagteam-core/src/tokens/price.rs crates/tagteam-core/src/tokens/attribution.rs crates/tagteam-core/src/tokens/blocks.rs crates/tagteam-core/src/tokens/bucket.rs
git commit -m "Add token counts and the response merge rule"
```

#### Notes for the controller

- **Stubs are not `//!`-only (outline deviation, contract unchanged).** With `mod.rs` in its final form, `pub use price::*;` over a module with no public item fails `cargo clippy … -D warnings` with ``unused import: `price::*` `` (checked on the pinned 1.88.0 toolchain). So each stub carries one final contract item (`strip_date_suffix`, `Home`, `BLOCK_MS`, `day_of`), and Tasks 3–6 still own and replace their files whole, keeping that item verbatim; `mod.rs` is never edited again, so batch B stays file-disjoint. **The same trap is in Task 14:** an engine `tokens/mod.rs` with `pub use refresh::*; pub use view::*;` over `//!`-only files, or `pub use evidence::*;` over a file whose only item is a `pub(crate)` method in an `impl Engine` block (an impl is not importable), does not pass clippy (checked: ``unused import: `evidence::*` ``, and the uncalled `pub(crate)` stub is a second error, ``method `scan_sessions` is never used``, until Task 15 calls it). Task 14 needs a public item in each, or the re-exports added by the tasks that fill them, and its stub called or allowed.
- **jiff 0.2.37, pinned exactly.** The newest 0.2.x at least two weeks old at planning time, 2026-10-15 (0.2.38 is from 2026-10-06). A caret `0.2.37` would resolve to 0.2.38 on the first `cargo fetch`, and `cargo update --precise` cannot be run before jiff is in the lock, so the requirement is `=0.2.37`. Its compiled footprint is `jiff` and `jiff-core` (both MIT or Unlicense). Every API Tasks 2 and 6 use was checked against the 0.2.37 source and compiled: `Timestamp::{from_millisecond, as_millisecond, to_zoned, MIN, MAX}`, `Zoned::{date, timestamp}`, `civil::{Date, date}` with `Date::{new, year, month, tomorrow, last_of_month, to_zoned, MIN, MAX}`, and `tz::TimeZone::{get, system, UTC}`.
- **The zone database.** With these features jiff reads the system's `/usr/share/zoneinfo`; Task 6's tests name zones (`Europe/Brussels`, `Asia/Kolkata`, `America/St_Johns`, `Pacific/Kiritimati`). macOS, the Ubuntu CI runners and the `rust:1.88` Docker image have it. An image without `tzdata` (the release-smoke Alpine container) has no zone names: there `TimeZone::system()` is UTC and `--tz <name>` fails (Task 19 should make that a clear usage error). Bundling a database would add `jiff-tzdb`, a second new crate, which the head's rule excludes.
- **Later crates.** Only `tagteam-core` takes `jiff` here. `tagteam-engine` needs `jiff.workspace = true` in the task that first names a jiff type there (Task 18, `TokenQuery.tz`), and so does `tagteam` for `--tz` (Task 19), unless they go through a `tagteam_core` re-export.
- **A performance lever, if needed.** jiff's `tz-fat` feature (no new crate) precomputes transitions past a zone file's last explicit one; enable it only if Task 23's 30-day query budget shows bucketing cost.

---

### Task 3: Prices and cost

**Depends on:** 2
**Files:**
- Modify: `crates/tagteam-core/src/tokens/price.rs` (replaces Task 2's stub whole, keeping its item verbatim)
- Test: `crates/tagteam-core/src/tokens/price.rs`

**Interfaces:**
- Consumes: `TokenCounts` and `TokenCounts::prompt` (Task 2, via `super::counts::TokenCounts`); `strip_date_suffix` (Task 2's stub, kept verbatim).
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateCard { pub input: f64, pub output: f64, pub cache_write_5m: f64, pub cache_write_1h: f64, pub cache_read: f64 } // USD per MTok
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice { pub base: RateCard, pub long_prompt: Option<(u64, RateCard)>, pub fast: Option<RateCard>, pub web_search_per_1k: Option<f64> }
#[derive(Debug)]
pub struct PriceTable { pub as_of: &'static str, pub models: &'static [(&'static str, ModelPrice)] }
pub static EMPTY_PRICES: PriceTable = PriceTable { as_of: "", models: &[] };
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CardOverride { pub input: Option<f64>, pub output: Option<f64>, pub cache_write_5m: Option<f64>, pub cache_write_1h: Option<f64>, pub cache_read: Option<f64> }
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PriceOverride { pub base: CardOverride, pub long_prompt_above: Option<u64>, pub long_prompt: CardOverride, pub fast: CardOverride, pub web_search_per_1k: Option<f64> }
#[derive(Debug, Clone)]
pub struct Prices { /* private */ }
impl Prices {
    /// Shipped table merged with overrides (spec §8.2). Returns the models an override changed or added, sorted.
    pub fn new(table: &PriceTable, overrides: &BTreeMap<String, PriceOverride>) -> (Prices, Vec<String>);
    pub fn as_of(&self) -> &str;
    pub fn lookup(&self, model: &str) -> Option<&ModelPrice>; // exact, then strip_date_suffix
    /// None: unpriced (no entry, or `speed == Some("fast")` with no fast card). Spec §8.1, §8.3, ruling R4.
    pub fn cost_nanos(&self, model: &str, counts: &TokenCounts, speed: Option<&str>) -> Option<u64>;
}
pub fn strip_date_suffix(model: &str) -> &str; // "claude-haiku-4-5-20251001" -> "claude-haiku-4-5"
```

- [ ] **Step 1: Write the failing tests**

Append to the end of `crates/tagteam-core/src/tokens/price.rs`, after Task 2's stub, one blank line between them:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const OPUS: RateCard = RateCard {
        input: 4.0,
        output: 20.0,
        cache_write_5m: 5.0,
        cache_write_1h: 8.0,
        cache_read: 0.20,
    };
    const OPUS_FAST: RateCard = RateCard {
        input: 8.0,
        output: 40.0,
        cache_write_5m: 10.0,
        cache_write_1h: 16.0,
        cache_read: 0.40,
    };
    const HAIKU: RateCard = RateCard {
        input: 0.10,
        output: 0.50,
        cache_write_5m: 0.125,
        cache_write_1h: 0.20,
        cache_read: 0.01,
    };
    const HAIKU_LONG: RateCard = RateCard {
        input: 0.50,
        output: 2.50,
        cache_write_5m: 0.625,
        cache_write_1h: 1.00,
        cache_read: 0.05,
    };
    const OLD: RateCard = RateCard {
        input: 1.0,
        output: 5.0,
        cache_write_5m: 1.25,
        cache_write_1h: 2.0,
        cache_read: 0.10,
    };

    static TABLE: PriceTable = PriceTable {
        as_of: "2026-10-14",
        models: &[
            (
                "claude-opus-5-5",
                ModelPrice {
                    base: OPUS,
                    long_prompt: None,
                    fast: Some(OPUS_FAST),
                    web_search_per_1k: Some(10.0),
                },
            ),
            (
                "claude-haiku-5-5",
                ModelPrice {
                    base: HAIKU,
                    long_prompt: Some((100_000, HAIKU_LONG)),
                    fast: None,
                    web_search_per_1k: Some(10.0),
                },
            ),
            (
                "claude-haiku-4-5",
                ModelPrice {
                    base: OLD,
                    long_prompt: None,
                    fast: None,
                    web_search_per_1k: None,
                },
            ),
            (
                "claude-haiku-4-5-20240101",
                ModelPrice {
                    base: HAIKU,
                    long_prompt: None,
                    fast: None,
                    web_search_per_1k: None,
                },
            ),
            // No model has both today (token report §8.1); the rule is tested all the same.
            (
                "claude-both",
                ModelPrice {
                    base: OLD,
                    long_prompt: Some((10, HAIKU_LONG)),
                    fast: Some(OPUS_FAST),
                    web_search_per_1k: None,
                },
            ),
        ],
    };

    fn shipped() -> Prices {
        Prices::new(&TABLE, &BTreeMap::new()).0
    }

    fn tokens(input: u64, output: u64, cw5: u64, cw1: u64, read: u64) -> TokenCounts {
        TokenCounts {
            input,
            output,
            cache_write_5m: cw5,
            cache_write_1h: cw1,
            cache_read: read,
            web_search: 0,
        }
    }

    fn card(input: f64, output: f64, cw5: f64, cw1: f64, read: f64) -> CardOverride {
        CardOverride {
            input: Some(input),
            output: Some(output),
            cache_write_5m: Some(cw5),
            cache_write_1h: Some(cw1),
            cache_read: Some(read),
        }
    }

    fn input_only(input: f64) -> CardOverride {
        CardOverride {
            input: Some(input),
            ..CardOverride::default()
        }
    }

    #[test]
    fn exact_then_date_stripped_lookup() {
        let prices = shipped();
        assert_eq!(prices.as_of(), "2026-10-14");
        let base = |model: &str| prices.lookup(model).map(|p| p.base);
        assert_eq!(base("claude-haiku-4-5"), Some(OLD));
        assert_eq!(
            base("claude-haiku-4-5-20251001"),
            Some(OLD),
            "the date is stripped"
        );
        assert_eq!(
            base("claude-haiku-4-5-20240101"),
            Some(HAIKU),
            "an exact name wins"
        );
        assert_eq!(
            base("claude-haiku-4-5-2025100"),
            None,
            "seven digits are no date"
        );
        assert_eq!(base("claude-opus-5-5[1m]"), None, "only a date is stripped");
        assert_eq!(
            strip_date_suffix("claude-haiku-4-5-20251001"),
            "claude-haiku-4-5"
        );
        assert_eq!(strip_date_suffix("claude-opus-5-5"), "claude-opus-5-5");
        assert_eq!(
            strip_date_suffix("-20251001"),
            "-20251001",
            "nothing would be left"
        );
        assert_eq!(strip_date_suffix("model-2025100a"), "model-2025100a");
        assert_eq!(strip_date_suffix("model_20251001"), "model_20251001");
        assert_eq!(strip_date_suffix(""), "");
    }

    #[test]
    fn cost_is_integer_nanos_per_token() {
        let prices = shipped();
        // $4, $20, $5, $8 and $0.20 per million tokens are 4,000, 20,000, 5,000, 8,000 and 200
        // nano-dollars per token.
        let c = tokens(1_000, 500, 2_000, 100, 10_000);
        let want = 1_000 * 4_000 + 500 * 20_000 + 2_000 * 5_000 + 100 * 8_000 + 10_000 * 200;
        assert_eq!(prices.cost_nanos("claude-opus-5-5", &c, None), Some(want));
        assert_eq!(want, 26_800_000);
        let million = tokens(1_000_000, 0, 0, 0, 0);
        assert_eq!(
            prices.cost_nanos("claude-opus-5-5", &million, None),
            Some(4_000_000_000),
            "a million input tokens at $4 is $4 exactly"
        );
        assert_eq!(
            prices.cost_nanos("claude-opus-5-5", &TokenCounts::default(), None),
            Some(0),
            "no tokens: priced, at nothing"
        );
        // Each rate rounds to whole nano-dollars per token: $0.0004 is 0, $0.0006 is 1, and
        // $0.30 is 300 although 0.30 × 1000 is not exactly 300 in binary.
        let (odd, _) = Prices::new(
            &EMPTY_PRICES,
            &BTreeMap::from([(
                "m".to_owned(),
                PriceOverride {
                    base: card(0.0004, 0.0006, 0.30, 0.125, 0.01),
                    ..PriceOverride::default()
                },
            )]),
        );
        assert_eq!(
            odd.cost_nanos("m", &tokens(1, 1, 1, 1, 1), None),
            Some(1 + 300 + 125 + 10)
        );
        // Sums saturate instead of overflowing.
        let huge = tokens(u64::MAX, u64::MAX, 0, 0, 0);
        assert_eq!(
            prices.cost_nanos("claude-opus-5-5", &huge, None),
            Some(u64::MAX)
        );
    }

    #[test]
    fn fast_uses_its_card_and_without_one_is_unpriced() {
        let prices = shipped();
        let c = tokens(1_000, 100, 0, 0, 0);
        let base = 1_000 * 4_000 + 100 * 20_000;
        let fast = 1_000 * 8_000 + 100 * 40_000;
        assert_eq!(prices.cost_nanos("claude-opus-5-5", &c, None), Some(base));
        assert_eq!(
            prices.cost_nanos("claude-opus-5-5", &c, Some("standard")),
            Some(base)
        );
        assert_eq!(
            prices.cost_nanos("claude-opus-5-5", &c, Some("fast")),
            Some(fast)
        );
        assert_eq!(
            prices.cost_nanos("claude-haiku-4-5", &c, Some("fast")),
            None,
            "no fast card: unpriced"
        );
        // A fast response keeps its fast card above a long-prompt threshold.
        assert!(c.prompt() > 10);
        assert_eq!(
            prices.cost_nanos("claude-both", &c, Some("fast")),
            Some(fast)
        );
    }

    #[test]
    fn long_prompt_switches_card_above_the_threshold_only() {
        let prices = shipped();
        // The prompt is input, cache writes and cache reads; exactly at the threshold is not
        // above it.
        let at = tokens(40_000, 1_000, 30_000, 10_000, 20_000);
        assert_eq!(at.prompt(), 100_000);
        let base = 40_000 * 100 + 1_000 * 500 + 30_000 * 125 + 10_000 * 200 + 20_000 * 10;
        assert_eq!(prices.cost_nanos("claude-haiku-5-5", &at, None), Some(base));
        let above = tokens(40_001, 1_000, 30_000, 10_000, 20_000);
        let long = 40_001 * 500 + 1_000 * 2_500 + 30_000 * 625 + 10_000 * 1_000 + 20_000 * 50;
        assert_eq!(
            prices.cost_nanos("claude-haiku-5-5", &above, None),
            Some(long)
        );
        // Output never counts toward the threshold.
        let talkative = tokens(10, 1_000_000, 0, 0, 0);
        assert_eq!(
            prices.cost_nanos("claude-haiku-5-5", &talkative, None),
            Some(10 * 100 + 1_000_000 * 500)
        );
    }

    #[test]
    fn web_search_is_priced_per_request() {
        let prices = shipped();
        let searches = TokenCounts {
            web_search: 3,
            ..TokenCounts::default()
        };
        assert_eq!(
            prices.cost_nanos("claude-opus-5-5", &searches, None),
            Some(30_000_000),
            "$10 per thousand: a cent each"
        );
        let mixed = TokenCounts {
            input: 1_000,
            web_search: 2,
            ..TokenCounts::default()
        };
        assert_eq!(
            prices.cost_nanos("claude-haiku-4-5", &mixed, None),
            Some(1_000 * 1_000),
            "no search price: its tokens are priced, its searches add nothing"
        );
    }

    #[test]
    fn overrides_merge_field_by_field_and_complete_base_adds_a_model() {
        let overrides = BTreeMap::from([
            (
                "claude-opus-5-5".to_owned(),
                PriceOverride {
                    base: input_only(3.0),
                    fast: CardOverride {
                        output: Some(30.0),
                        ..CardOverride::default()
                    },
                    ..PriceOverride::default()
                },
            ),
            (
                "claude-new".to_owned(),
                PriceOverride {
                    base: card(1.0, 2.0, 3.0, 4.0, 0.5),
                    web_search_per_1k: Some(5.0),
                    ..PriceOverride::default()
                },
            ),
            (
                "claude-partial".to_owned(),
                PriceOverride {
                    base: input_only(1.0),
                    ..PriceOverride::default()
                },
            ),
            (
                "claude-haiku-4-5".to_owned(),
                PriceOverride {
                    base: input_only(OLD.input),
                    ..PriceOverride::default()
                },
            ),
        ]);
        let (prices, changed) = Prices::new(&TABLE, &overrides);
        assert_eq!(
            changed,
            ["claude-new", "claude-opus-5-5"],
            "an override that matches the shipped price changes nothing"
        );
        assert_eq!(prices.as_of(), "2026-10-14");
        let opus = prices.lookup("claude-opus-5-5").unwrap();
        assert_eq!(opus.base, RateCard { input: 3.0, ..OPUS });
        assert_eq!(
            opus.fast,
            Some(RateCard {
                output: 30.0,
                ..OPUS_FAST
            })
        );
        assert_eq!(opus.web_search_per_1k, Some(10.0));
        let new = prices.lookup("claude-new").unwrap();
        assert_eq!(
            *new,
            ModelPrice {
                base: RateCard {
                    input: 1.0,
                    output: 2.0,
                    cache_write_5m: 3.0,
                    cache_write_1h: 4.0,
                    cache_read: 0.5,
                },
                long_prompt: None,
                fast: None,
                web_search_per_1k: Some(5.0),
            }
        );
        assert_eq!(
            prices.lookup("claude-partial"),
            None,
            "an incomplete base adds nothing"
        );
        assert_eq!(prices.lookup("claude-haiku-4-5").map(|p| p.base), Some(OLD));
    }

    #[test]
    fn a_card_the_entry_lacks_needs_every_rate_and_bad_rates_are_ignored() {
        let overrides = BTreeMap::from([
            // A fast card for a model with none is added only whole.
            (
                "claude-haiku-4-5".to_owned(),
                PriceOverride {
                    fast: input_only(2.0),
                    ..PriceOverride::default()
                },
            ),
            // A new threshold keeps the shipped long-prompt card; a rate that is not finite is
            // ignored.
            (
                "claude-haiku-5-5".to_owned(),
                PriceOverride {
                    base: input_only(f64::NAN),
                    long_prompt_above: Some(50_000),
                    ..PriceOverride::default()
                },
            ),
            // A long-prompt card with no threshold has nothing to switch at.
            (
                "claude-opus-5-5".to_owned(),
                PriceOverride {
                    long_prompt: card(1.0, 1.0, 1.0, 1.0, 1.0),
                    ..PriceOverride::default()
                },
            ),
            // A dated name gets an entry of its own, from the undated one.
            (
                "claude-opus-5-5-20261001".to_owned(),
                PriceOverride {
                    long_prompt_above: Some(200_000),
                    long_prompt: card(1.0, 1.0, 1.0, 1.0, 1.0),
                    ..PriceOverride::default()
                },
            ),
            // A negative rate leaves the base incomplete, so no model is added.
            (
                "claude-negative".to_owned(),
                PriceOverride {
                    base: card(-1.0, 1.0, 1.0, 1.0, 1.0),
                    ..PriceOverride::default()
                },
            ),
        ]);
        let (prices, changed) = Prices::new(&TABLE, &overrides);
        assert_eq!(changed, ["claude-haiku-5-5", "claude-opus-5-5-20261001"]);
        assert_eq!(prices.lookup("claude-haiku-4-5").unwrap().fast, None);
        let haiku = prices.lookup("claude-haiku-5-5").unwrap();
        assert_eq!(haiku.base, HAIKU);
        assert_eq!(haiku.long_prompt, Some((50_000, HAIKU_LONG)));
        assert_eq!(prices.lookup("claude-opus-5-5").unwrap().long_prompt, None);
        let dated = prices.lookup("claude-opus-5-5-20261001").unwrap();
        assert_eq!(dated.base, OPUS);
        assert_eq!(dated.fast, Some(OPUS_FAST));
        assert_eq!(dated.long_prompt.map(|(above, _)| above), Some(200_000));
        assert_eq!(
            prices
                .lookup("claude-opus-5-5-20261002")
                .unwrap()
                .long_prompt,
            None,
            "another date still finds the shipped entry"
        );
        assert_eq!(prices.lookup("claude-negative"), None);
    }

    #[test]
    fn a_dated_override_equal_to_shipped_still_pins_its_entry() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "claude-opus-5-5".to_owned(),
            PriceOverride {
                base: input_only(3.0),
                ..PriceOverride::default()
            },
        );
        overrides.insert(
            "claude-opus-5-5-20261001".to_owned(),
            PriceOverride {
                base: input_only(4.0),
                ..PriceOverride::default()
            },
        );
        let (prices, changed) = Prices::new(&TABLE, &overrides);
        assert_eq!(changed, vec!["claude-opus-5-5".to_owned()]);
        let input = |model: &str| prices.lookup(model).map(|p| p.base.input);
        assert_eq!(input("claude-opus-5-5"), Some(3.0));
        assert_eq!(input("claude-opus-5-5-20261001"), Some(4.0));
        assert_eq!(input("claude-opus-5-5-20270101"), Some(3.0));
    }

    #[test]
    fn an_unknown_model_is_unpriced() {
        let prices = shipped();
        let c = tokens(1, 1, 1, 1, 1);
        assert_eq!(prices.lookup("gpt-x"), None);
        assert_eq!(prices.cost_nanos("gpt-x", &c, None), None);
        assert_eq!(prices.cost_nanos("gpt-x", &c, Some("fast")), None);
        let (empty, changed) = Prices::new(&EMPTY_PRICES, &BTreeMap::new());
        assert!(changed.is_empty());
        assert_eq!(empty.as_of(), "");
        assert_eq!(empty.cost_nanos("claude-opus-5-5", &c, None), None);
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-core --locked tokens::price`

Expected: compilation fails in `price.rs` (``could not compile `tagteam-core` (lib test)``) with `error[E0412]: cannot find type` for `RateCard`, `PriceTable`, `Prices`, `CardOverride` and `TokenCounts`, `error[E0422]: cannot find struct, variant or union type` for `ModelPrice`, `PriceOverride` and the others, ``error[E0425]: cannot find value `EMPTY_PRICES` in this scope``, and ``error[E0433]: failed to resolve: use of undeclared type `BTreeMap` ``.

- [ ] **Step 3: Implement**

In `crates/tagteam-core/src/tokens/price.rs`, replace everything above `#[cfg(test)]` (Task 2's stub) with the following, then one blank line:

```rust
//! Prices and cost (token report §8): rate cards in USD per million tokens, the shipped table
//! with the user's overrides merged in, and a response's API cost in integer nano-dollars.

use std::collections::BTreeMap;

use super::counts::TokenCounts;

/// USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateCard {
    pub input: f64,
    pub output: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
}

/// One model's prices (token report §8.1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    pub base: RateCard,
    /// The card for a response whose prompt is above the threshold, in tokens.
    pub long_prompt: Option<(u64, RateCard)>,
    /// The card for a `speed: "fast"` response.
    pub fast: Option<RateCard>,
    /// USD per thousand web search requests.
    pub web_search_per_1k: Option<f64>,
}

/// A provider's shipped prices, and the date they were taken.
#[derive(Debug)]
pub struct PriceTable {
    pub as_of: &'static str,
    pub models: &'static [(&'static str, ModelPrice)],
}

/// The table of a provider that ships none.
pub static EMPTY_PRICES: PriceTable = PriceTable {
    as_of: "",
    models: &[],
};

/// A rate card's fields as a price table in the config sets them (token report §8.2).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CardOverride {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_write_5m: Option<f64>,
    pub cache_write_1h: Option<f64>,
    pub cache_read: Option<f64>,
}

/// One `[provider.<id>.prices."<model>"]` table (token report §8.2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PriceOverride {
    pub base: CardOverride,
    pub long_prompt_above: Option<u64>,
    pub long_prompt: CardOverride,
    pub fast: CardOverride,
    pub web_search_per_1k: Option<f64>,
}

/// A rate as given, when it is finite and not negative.
fn valid(rate: Option<f64>) -> Option<f64> {
    rate.filter(|r| r.is_finite() && *r >= 0.0)
}

impl CardOverride {
    /// `card` with every rate this sets replaced.
    fn onto(&self, card: RateCard) -> RateCard {
        RateCard {
            input: valid(self.input).unwrap_or(card.input),
            output: valid(self.output).unwrap_or(card.output),
            cache_write_5m: valid(self.cache_write_5m).unwrap_or(card.cache_write_5m),
            cache_write_1h: valid(self.cache_write_1h).unwrap_or(card.cache_write_1h),
            cache_read: valid(self.cache_read).unwrap_or(card.cache_read),
        }
    }

    /// A card of these rates, when all five are set.
    fn whole(&self) -> Option<RateCard> {
        Some(RateCard {
            input: valid(self.input)?,
            output: valid(self.output)?,
            cache_write_5m: valid(self.cache_write_5m)?,
            cache_write_1h: valid(self.cache_write_1h)?,
            cache_read: valid(self.cache_read)?,
        })
    }

    /// `card` overridden, or, for a card the entry lacks, one only when this is whole.
    fn apply(&self, card: Option<RateCard>) -> Option<RateCard> {
        match card {
            Some(card) => Some(self.onto(card)),
            None => self.whole(),
        }
    }
}

impl PriceOverride {
    /// `price` with this override merged in, field by field. A long-prompt card the entry
    /// lacks needs a threshold too.
    fn onto(&self, price: ModelPrice) -> ModelPrice {
        let long_prompt = match (price.long_prompt, self.long_prompt_above) {
            (Some((above, card)), threshold) => {
                Some((threshold.unwrap_or(above), self.long_prompt.onto(card)))
            }
            (None, Some(threshold)) => self.long_prompt.whole().map(|card| (threshold, card)),
            (None, None) => None,
        };
        ModelPrice {
            base: self.base.onto(price.base),
            long_prompt,
            fast: self.fast.apply(price.fast),
            web_search_per_1k: valid(self.web_search_per_1k).or(price.web_search_per_1k),
        }
    }
}

/// The shipped table with the user's overrides merged in: what cost is computed from.
#[derive(Debug, Clone)]
pub struct Prices {
    as_of: &'static str,
    models: BTreeMap<String, ModelPrice>,
}

impl Prices {
    /// The shipped table with `overrides` merged in (token report §8.2), and the models an
    /// override changed or added, sorted. An override applies field by field to the entry its
    /// name finds as `lookup` does, and is kept under its own name; one for a model the table
    /// lacks adds it only with a whole base. A rate that is negative or not finite is ignored.
    pub fn new(
        table: &PriceTable,
        overrides: &BTreeMap<String, PriceOverride>,
    ) -> (Prices, Vec<String>) {
        let shipped = Prices {
            as_of: table.as_of,
            models: table
                .models
                .iter()
                .map(|(name, price)| ((*name).to_owned(), *price))
                .collect(),
        };
        let mut models = shipped.models.clone();
        let mut changed = Vec::new();
        for (name, over) in overrides {
            let was = shipped.lookup(name).copied();
            let now = match was {
                Some(price) => Some(over.onto(price)),
                None => over.base.whole().map(|base| {
                    over.onto(ModelPrice {
                        base,
                        long_prompt: None,
                        fast: None,
                        web_search_per_1k: None,
                    })
                }),
            };
            if let Some(price) = now {
                // Kept under its own name even when it equals what the name found, so a dated
                // override pins its entry against an override of the undated name.
                if Some(price) != was {
                    changed.push(name.clone());
                }
                models.insert(name.clone(), price);
            }
        }
        (
            Prices {
                as_of: table.as_of,
                models,
            },
            changed,
        )
    }

    /// The date the shipped prices were taken.
    pub fn as_of(&self) -> &str {
        self.as_of
    }

    /// A model's prices: by its exact name, then without a trailing date (token report §8.1).
    pub fn lookup(&self, model: &str) -> Option<&ModelPrice> {
        self.models
            .get(model)
            .or_else(|| self.models.get(strip_date_suffix(model)))
    }

    /// A response's API cost in nano-dollars (token report §8.1, §8.3, ruling R4): the fast
    /// card for `speed: "fast"`, else the long-prompt card for a prompt above its threshold,
    /// else the base card. A rate is rounded to whole nano-dollars per token, a web search to
    /// whole nano-dollars per request, and a model with no web search price prices its tokens
    /// only. `None` is unpriced: a model with no entry, or a fast response with no fast card.
    pub fn cost_nanos(
        &self,
        model: &str,
        counts: &TokenCounts,
        speed: Option<&str>,
    ) -> Option<u64> {
        let price = self.lookup(model)?;
        let card = match (speed, price.long_prompt) {
            (Some("fast"), _) => price.fast?,
            (_, Some((above, long))) if counts.prompt() > above => long,
            _ => price.base,
        };
        let tokens = [
            (counts.input, card.input),
            (counts.output, card.output),
            (counts.cache_write_5m, card.cache_write_5m),
            (counts.cache_write_1h, card.cache_write_1h),
            (counts.cache_read, card.cache_read),
        ]
        .into_iter()
        .fold(0u64, |sum, (n, rate)| {
            sum.saturating_add(n.saturating_mul(nanos(rate, 1_000.0)))
        });
        let searches = price.web_search_per_1k.map_or(0, |per_1k| {
            counts.web_search.saturating_mul(nanos(per_1k, 1_000_000.0))
        });
        Some(tokens.saturating_add(searches))
    }
}

/// `usd` in whole nano-dollars per unit: `scale` is 1,000 for USD per million tokens, and
/// 1,000,000 for USD per thousand requests.
fn nanos(usd: f64, scale: f64) -> u64 {
    (usd * scale).round() as u64
}

/// `model` without a trailing `-YYYYMMDD`, which a dated model name is priced by when the table
/// has no entry of its own: `claude-haiku-4-5-20251001` as `claude-haiku-4-5` (token report
/// §8.1).
pub fn strip_date_suffix(model: &str) -> &str {
    let b = model.as_bytes();
    match b.len().checked_sub(9) {
        Some(cut) if cut > 0 && b[cut] == b'-' && b[cut + 1..].iter().all(u8::is_ascii_digit) => {
            &model[..cut]
        }
        _ => model,
    }
}
```

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p tagteam-core --locked tokens::price`

Expected: `exact_then_date_stripped_lookup`, `cost_is_integer_nanos_per_token`, `fast_uses_its_card_and_without_one_is_unpriced`, `long_prompt_switches_card_above_the_threshold_only`, `web_search_is_priced_per_request`, `overrides_merge_field_by_field_and_complete_base_adds_a_model`, `a_card_the_entry_lacks_needs_every_rate_and_bad_rates_are_ignored` and `an_unknown_model_is_unpriced` pass: `test result: ok. 8 passed; 0 failed`.

- [ ] **Step 5: Lint**

```sh
cargo fmt --all --check
cargo clippy -p tagteam-core --all-targets --locked -- -D warnings
```

Expected: no output from `fmt`; clippy finishes with no warning.

- [ ] **Step 6: Commit**

```sh
git add crates/tagteam-core/src/tokens/price.rs
git commit -m "Add rate cards, price overrides and cost in nano-dollars"
```

#### Notes for the controller

- **Rulings inside spec §8.2, for the PR's "Rulings".** (a) An override keyed by a dated name (`claude-opus-5-5-20261001`) applies to the entry that name finds, by `lookup`'s rule, and is kept under its own name, so it never reprices the other dates. (b) `price_overrides` lists only models whose effective price differs from the shipped one: an override that repeats the shipped value, or that is ignored, is not "a model whose price config changed". (c) A card the entry lacks (`fast`, `long_prompt`) is added only from all five rates, and a long-prompt card also needs `long_prompt_above`; a lone threshold re-targets an existing long-prompt card. (d) A rate that is negative or not finite is ignored here too; Task 13 warns about it when it reads the config, so this is defence in depth.
- **Contract, unchanged.** `Prices::as_of` returns `&str` per the contract although it stores the table's `&'static str`; Task 18 can fill `TokenReport.prices_as_of: &'static str` from `p.price_table().as_of`.
- **Ruling R4 as built.** A rate is rounded once, per token (`$0.30` per MTok is 300 nano-dollars per token although `0.30 × 1000` is not exact in binary), and sums saturate. A model with no web-search price still prices its tokens; its searches add nothing.
- **An extra test** (`a_card_the_entry_lacks_needs_every_rate_and_bad_rates_are_ignored`) pins rulings (a), (c) and (d).

---

### Task 4: Attribution and the default home's timeline

**Depends on:** 2
**Files:**
- Modify: `crates/tagteam-core/src/tokens/attribution.rs` (replaces Task 2's stub whole, keeping its item verbatim)
- Test: `crates/tagteam-core/src/tokens/attribution.rs`

**Interfaces:**
- Consumes: `crate::ids::AccountId` (existing); `Home` (Task 2's stub, kept verbatim); `crate::tokens::test_support::pinned` (Task 2, tests only).
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Home { Default, Profile(AccountId) }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting { pub home: Home, pub first_ms: i64, pub last_ms: i64 }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch { pub account: AccountId, pub start_ms: i64, pub end_ms: Option<i64> } // None: still open
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coverage { pub epoch_ms: i64, pub seeded: bool, pub last_pre_epoch_remove_ms: Option<i64> }
impl Coverage { pub fn complete_at(&self, t: i64) -> bool; } // t >= epoch, or (seeded and t >= last pre-epoch remove, if any)
#[derive(Debug, Clone, Default, PartialEq, Eq)] // derives beyond the contract's, harmless
pub struct Launches { /* sorted, merged spans */ }
impl Launches { pub fn new(launches: Vec<Launch>) -> Self; pub fn any_covers(&self, t: i64) -> bool; } // start <= t && (end is None || t < end)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement { Home(Home), Unattributed }
/// Spec §7.1–7.2: sighted in exactly one home -> it; in two -> Unattributed; nowhere -> Default when no launch covers t and coverage is complete, else Unattributed.
pub fn place(sightings: &[Sighting], launches: &Launches, coverage: &Coverage, t: i64) -> Placement;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineEvent {
    Switch { at_ms: i64, from: Option<AccountId>, to: AccountId },
    Add { at_ms: i64, account: AccountId },
    Remove { at_ms: i64, account: AccountId },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveChange { pub at_ms: i64, pub account: Option<AccountId> }
#[derive(Debug, Clone, Default, PartialEq, Eq)] // derives beyond the contract's, harmless
pub struct Timeline { /* segments */ }
impl Timeline {
    pub fn build(events: &[TimelineEvent], history: &[ActiveChange], epoch_ms: i64, added_at: &BTreeMap<AccountId, i64>) -> Timeline;
    pub fn account_at(&self, t: i64) -> Option<AccountId>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Basis { Profile, Timeline, Unattributed }
/// A placement to an account: Home(Profile(a)) -> (Some(a), Profile); Home(Default) -> (timeline.account_at(t), Timeline) or (None, Unattributed) when it has none.
pub fn attribute(placement: &Placement, timeline: &Timeline, t: i64) -> (Option<AccountId>, Basis);
```

- [ ] **Step 1: Write the failing tests**

Append to the end of `crates/tagteam-core/src/tokens/attribution.rs`, after Task 2's stub, one blank line between them:

```rust
#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::tokens::test_support::pinned;

    const H: i64 = 3_600_000;
    const EPOCH: i64 = 1_000 * H;

    fn id(s: &str) -> AccountId {
        AccountId::from_string(s)
    }

    fn profile(s: &str) -> Home {
        Home::Profile(id(s))
    }

    fn seen(home: Home, first_ms: i64, last_ms: i64) -> Sighting {
        Sighting {
            home,
            first_ms,
            last_ms,
        }
    }

    fn launch(account: &str, start_ms: i64, end_ms: Option<i64>) -> Launch {
        Launch {
            account: id(account),
            start_ms,
            end_ms,
        }
    }

    fn none() -> Launches {
        Launches::new(Vec::new())
    }

    fn complete() -> Coverage {
        Coverage {
            epoch_ms: 0,
            seeded: true,
            last_pre_epoch_remove_ms: None,
        }
    }

    fn switch(at_ms: i64, from: Option<&str>, to: &str) -> TimelineEvent {
        TimelineEvent::Switch {
            at_ms,
            from: from.map(id),
            to: id(to),
        }
    }

    fn add(at_ms: i64, account: &str) -> TimelineEvent {
        TimelineEvent::Add {
            at_ms,
            account: id(account),
        }
    }

    fn remove(at_ms: i64, account: &str) -> TimelineEvent {
        TimelineEvent::Remove {
            at_ms,
            account: id(account),
        }
    }

    fn active(at_ms: i64, account: Option<&str>) -> ActiveChange {
        ActiveChange {
            at_ms,
            account: account.map(id),
        }
    }

    #[test]
    fn one_home_places_two_homes_conflict() {
        let a = seen(profile("a"), 10, 20);
        assert_eq!(
            place(&[a.clone()], &none(), &complete(), 15),
            Placement::Home(profile("a"))
        );
        // Two executions in the same home still name one home.
        let twice = [a.clone(), seen(profile("a"), 12, 30)];
        assert_eq!(
            place(&twice, &none(), &complete(), 15),
            Placement::Home(profile("a"))
        );
        // Open in a profile and the default home at once: unattributed, never guessed.
        let both = [a.clone(), seen(Home::Default, 14, 16)];
        assert_eq!(
            place(&both, &none(), &complete(), 15),
            Placement::Unattributed
        );
        assert_eq!(
            place(&both, &none(), &complete(), 17),
            Placement::Home(profile("a")),
            "outside the overlap, one home again"
        );
        // A sighting places a response whatever launches cover it.
        let other = Launches::new(vec![launch("b", 0, None)]);
        assert_eq!(
            place(&[a], &other, &complete(), 15),
            Placement::Home(profile("a"))
        );
    }

    #[test]
    fn a_sighting_never_covers_before_its_first_or_after_its_last() {
        let a = [seen(profile("a"), 10, 20)];
        let incomplete = Coverage {
            epoch_ms: 1_000,
            seeded: false,
            last_pre_epoch_remove_ms: None,
        };
        for t in [9, 21] {
            assert_eq!(
                place(&a, &none(), &incomplete, t),
                Placement::Unattributed,
                "{t}"
            );
        }
        for t in [10, 20] {
            assert_eq!(
                place(&a, &none(), &incomplete, t),
                Placement::Home(profile("a")),
                "{t}: both ends are inclusive"
            );
        }
        assert_eq!(
            place(&a, &none(), &complete(), 21),
            Placement::Home(Home::Default),
            "after it, with nothing else running, the default home"
        );
    }

    #[test]
    fn unsighted_is_default_only_without_a_covering_launch() {
        let launches = Launches::new(vec![launch("a", 100, Some(200))]);
        assert_eq!(
            place(&[], &launches, &complete(), 99),
            Placement::Home(Home::Default)
        );
        assert_eq!(
            place(&[], &launches, &complete(), 100),
            Placement::Unattributed,
            "a launch covers its start"
        );
        assert_eq!(
            place(&[], &launches, &complete(), 199),
            Placement::Unattributed
        );
        assert_eq!(
            place(&[], &launches, &complete(), 200),
            Placement::Home(Home::Default),
            "and not its end"
        );
    }

    #[test]
    fn an_open_launch_covers_until_closed() {
        let open = Launches::new(vec![launch("a", 100, None)]);
        assert!(!open.any_covers(99));
        assert!(open.any_covers(100) && open.any_covers(i64::MAX - 1));
        let closed = Launches::new(vec![launch("a", 100, Some(150))]);
        assert!(closed.any_covers(149) && !closed.any_covers(150));
        // Overlapping, nested, touching and empty launches cover what they cover together.
        let many = Launches::new(vec![
            launch("b", 300, Some(400)),
            launch("a", 100, Some(200)),
            launch("c", 150, Some(180)),
            launch("d", 200, Some(250)),
            launch("e", 500, Some(500)),
        ]);
        for (t, covered) in [
            (99, false),
            (100, true),
            (199, true),
            (200, true),
            (249, true),
            (250, false),
            (300, true),
            (399, true),
            (400, false),
            (500, false),
        ] {
            assert_eq!(many.any_covers(t), covered, "{t}");
        }
        assert!(!none().any_covers(0));
    }

    #[test]
    fn pre_epoch_coverage_needs_seeding_and_follows_the_last_remove() {
        let unseeded = Coverage {
            epoch_ms: EPOCH,
            seeded: false,
            last_pre_epoch_remove_ms: None,
        };
        assert!(!unseeded.complete_at(EPOCH - 1));
        assert!(
            unseeded.complete_at(EPOCH),
            "from the epoch on, run records every launch"
        );
        let seeded = Coverage {
            seeded: true,
            ..unseeded
        };
        assert!(seeded.complete_at(0));
        let removed = Coverage {
            last_pre_epoch_remove_ms: Some(EPOCH - 10 * H),
            ..seeded
        };
        assert!(
            !removed.complete_at(EPOCH - 10 * H - 1),
            "a removed account may have had a profile that left no trace"
        );
        assert!(removed.complete_at(EPOCH - 10 * H));
        assert_eq!(
            place(&[], &none(), &removed, EPOCH - 11 * H),
            Placement::Unattributed
        );
        assert_eq!(
            place(&[], &none(), &removed, EPOCH - 9 * H),
            Placement::Home(Home::Default)
        );
    }

    #[test]
    fn timeline_follows_switches_then_history() {
        let events = [
            add(H, "a"),
            add(H, "b"),
            switch(10 * H, Some("a"), "b"),
            switch(20 * H, Some("b"), "a"),
        ];
        // The epoch at 30 h, seeded with a; b from 40 h; no active account from 50 h.
        let history = [
            active(30 * H, Some("a")),
            active(40 * H, Some("b")),
            active(50 * H, None),
        ];
        let t = Timeline::build(&events, &history, 30 * H, &BTreeMap::new());
        assert_eq!(t.account_at(10 * H), Some(id("b")), "a switch's own time");
        assert_eq!(t.account_at(15 * H), Some(id("b")));
        assert_eq!(t.account_at(25 * H), Some(id("a")));
        assert_eq!(t.account_at(30 * H), Some(id("a")), "the seeded row");
        assert_eq!(t.account_at(45 * H), Some(id("b")));
        assert_eq!(t.account_at(55 * H), None, "no active account");
        // From the epoch on, only the history counts: a switch event there is not read.
        let late = [add(H, "a"), add(H, "c"), switch(35 * H, Some("a"), "c")];
        let t = Timeline::build(&late, &history, 30 * H, &BTreeMap::new());
        assert_eq!(t.account_at(36 * H), Some(id("a")));
        assert_eq!(t.account_at(5 * H), Some(id("a")), "before it, its from");
        // From the epoch on, a time before the first history row has no account.
        let t = Timeline::build(
            &events,
            &[active(31 * H, Some("a"))],
            30 * H,
            &BTreeMap::new(),
        );
        assert_eq!(t.account_at(30 * H), None);
    }

    #[test]
    fn timeline_before_the_first_switch_uses_from_since_its_add() {
        let history = [active(30 * H, Some("b"))];
        let events = [
            add(5 * H, "a"),
            add(6 * H, "b"),
            switch(10 * H, Some("a"), "b"),
        ];
        let t = Timeline::build(&events, &history, 30 * H, &BTreeMap::new());
        assert_eq!(t.account_at(5 * H - 1), None, "before a was added");
        assert_eq!(t.account_at(5 * H), Some(id("a")));
        assert_eq!(t.account_at(10 * H - 1), Some(id("a")));
        assert_eq!(t.account_at(10 * H), Some(id("b")));
        // With no add event, the account row's added_at stands in.
        let bare = [switch(10 * H, Some("a"), "b")];
        let t = Timeline::build(&bare, &history, 30 * H, &BTreeMap::from([(id("a"), 7 * H)]));
        assert_eq!(t.account_at(7 * H - 1), None);
        assert_eq!(t.account_at(8 * H), Some(id("a")));
        // With neither, nothing before the first switch is placed.
        let t = Timeline::build(&bare, &history, 30 * H, &BTreeMap::new());
        assert_eq!(t.account_at(8 * H), None);
        assert_eq!(t.account_at(11 * H), Some(id("b")));
        // A first switch from no account leaves the time before it unplaced.
        let from_none = [add(H, "b"), switch(10 * H, None, "b")];
        let t = Timeline::build(&from_none, &history, 30 * H, &BTreeMap::new());
        assert_eq!(t.account_at(5 * H), None);
    }

    #[test]
    fn timeline_without_switches_uses_the_seeded_account() {
        let history = [active(30 * H, Some("a")), active(40 * H, Some("b"))];
        let t = Timeline::build(&[add(5 * H, "a")], &history, 30 * H, &BTreeMap::new());
        assert_eq!(t.account_at(4 * H), None, "before its add");
        assert_eq!(t.account_at(5 * H), Some(id("a")));
        assert_eq!(t.account_at(29 * H), Some(id("a")));
        assert_eq!(t.account_at(41 * H), Some(id("b")));
        // Seeded with no active account: nothing before the epoch.
        let t = Timeline::build(
            &[add(5 * H, "a")],
            &[active(30 * H, None)],
            30 * H,
            &BTreeMap::new(),
        );
        assert_eq!(t.account_at(10 * H), None);
        // With no add event left, the account row's added_at stands in.
        let t = Timeline::build(&[], &history, 30 * H, &BTreeMap::from([(id("a"), 2 * H)]));
        assert_eq!(t.account_at(3 * H), Some(id("a")));
    }

    #[test]
    fn a_missing_seed_never_backdates_a_later_activation() {
        // No account was active at the epoch, so the migration seeded no row; `b` became
        // active later without a switch.
        let t = Timeline::build(
            &[add(5 * H, "b")],
            &[active(40 * H, Some("b"))],
            30 * H,
            &BTreeMap::new(),
        );
        assert_eq!(t.account_at(10 * H), None);
        assert_eq!(t.account_at(35 * H), None);
        assert_eq!(t.account_at(41 * H), Some(id("b")));
    }

    #[test]
    fn remove_ends_a_segment() {
        let events = [
            add(H, "a"),
            add(H, "b"),
            add(H, "c"),
            switch(10 * H, Some("a"), "b"),
            remove(12 * H, "b"),
            switch(15 * H, None, "c"),
            remove(18 * H, "a"),
        ];
        let t = Timeline::build(
            &events,
            &[active(30 * H, Some("c"))],
            30 * H,
            &BTreeMap::new(),
        );
        assert_eq!(
            t.account_at(5 * H),
            Some(id("a")),
            "a removed account keeps its past"
        );
        assert_eq!(t.account_at(11 * H), Some(id("b")));
        assert_eq!(t.account_at(12 * H), None, "b's remove ends its segment");
        assert_eq!(t.account_at(14 * H), None, "until the next switch");
        assert_eq!(t.account_at(15 * H), Some(id("c")));
        assert_eq!(
            t.account_at(19 * H),
            Some(id("c")),
            "another account's remove ends nothing"
        );
    }

    #[test]
    fn attribute_maps_homes_to_accounts_and_bases() {
        let t = Timeline::build(&[], &[active(0, Some("a"))], 0, &BTreeMap::new());
        assert_eq!(
            attribute(&Placement::Home(profile("p")), &t, 5),
            (Some(id("p")), Basis::Profile)
        );
        assert_eq!(
            attribute(&Placement::Home(Home::Default), &t, 5),
            (Some(id("a")), Basis::Timeline)
        );
        assert_eq!(
            attribute(&Placement::Home(Home::Default), &t, -1),
            (None, Basis::Unattributed),
            "the default home with no account then"
        );
        assert_eq!(
            attribute(&Placement::Unattributed, &t, 5),
            (None, Basis::Unattributed)
        );
    }

    fn home() -> impl Strategy<Value = Home> {
        prop_oneof![Just(Home::Default), Just(profile("a")), Just(profile("b"))]
    }

    fn sighting() -> impl Strategy<Value = Sighting> {
        (home(), 0..100i64, 0..30i64).prop_map(|(home, first, len)| seen(home, first, first + len))
    }

    fn launches() -> impl Strategy<Value = Vec<Launch>> {
        prop::collection::vec(
            (
                prop_oneof![Just("a"), Just("b")],
                0..100i64,
                prop::option::of(0..40i64),
            )
                .prop_map(|(account, start, len)| launch(account, start, len.map(|l| start + l))),
            0..4,
        )
    }

    proptest! {
        #![proptest_config(pinned(512))]

        /// Token report §7.1–7.2: a profile is only ever a sighting's home, and the default
        /// home without one only as the sole candidate.
        #[test]
        fn place_is_never_a_profile_without_a_sighting(
            sightings in prop::collection::vec(sighting(), 0..4),
            launches in launches(),
            epoch_ms in 0..120i64,
            seeded in any::<bool>(),
            last_remove in prop::option::of(0..120i64),
            t in 0..130i64,
        ) {
            let coverage = Coverage { epoch_ms, seeded, last_pre_epoch_remove_ms: last_remove };
            let launches = Launches::new(launches);
            let homes: Vec<&Home> = sightings
                .iter()
                .filter(|s| s.first_ms <= t && t <= s.last_ms)
                .map(|s| &s.home)
                .collect();
            let alone = !launches.any_covers(t) && coverage.complete_at(t);
            match place(&sightings, &launches, &coverage, t) {
                Placement::Home(home) if homes.is_empty() => {
                    prop_assert_eq!(home, Home::Default);
                    prop_assert!(alone);
                }
                Placement::Home(home) => prop_assert!(homes.iter().all(|h| **h == home)),
                Placement::Unattributed => prop_assert!(
                    homes.iter().any(|h| *h != homes[0]) || (homes.is_empty() && !alone)
                ),
            }
        }

        /// Merging launches into spans changes nothing they cover.
        #[test]
        fn merged_launches_cover_exactly_what_their_launches_cover(
            list in launches(),
            t in 0..150i64,
        ) {
            let one_by_one = list
                .iter()
                .any(|l| l.start_ms <= t && l.end_ms.is_none_or(|e| t < e));
            prop_assert_eq!(Launches::new(list).any_covers(t), one_by_one);
        }
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-core --locked tokens::attribution`

Expected: compilation fails in `attribution.rs` (``could not compile `tagteam-core` (lib test)``) with `error[E0412]: cannot find type` for `Sighting`, `Launch`, `Launches`, `Coverage`, `TimelineEvent` and `ActiveChange`, `error[E0425]: cannot find function` for `place` and `attribute`, and `error[E0433]: failed to resolve: use of undeclared type` for `Placement`, `Timeline`, `Basis` and `BTreeMap`.

- [ ] **Step 3: Implement**

In `crates/tagteam-core/src/tokens/attribution.rs`, replace everything above `#[cfg(test)]` (Task 2's stub) with the following, then one blank line:

```rust
//! Attribution (token report §7): which account a response went to, decided at query time from
//! the evidence of where its session ran. A response is placed only where the evidence allows
//! one home; anything else is unattributed, never guessed.

use std::collections::BTreeMap;

use crate::ids::AccountId;

/// A Claude Code home a session ran in: the default home, or an account's `run` profile.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Home {
    Default,
    Profile(AccountId),
}

/// One execution of a session, seen alive in `home` from `first_ms` to `last_ms` (token report
/// §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    pub home: Home,
    pub first_ms: i64,
    pub last_ms: i64,
}

/// A `run` launch: a profile session may have run for `account` from `start_ms` until `end_ms`,
/// or still runs (`None`) (token report §6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub account: AccountId,
    pub start_ms: i64,
    pub end_ms: Option<i64>,
}

/// Whether every profile session at a time has its launch recorded (token report §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coverage {
    /// When the store began recording launches and sightings (`token_epoch.at`).
    pub epoch_ms: i64,
    /// Whether the migration seeded the profiles that existed then (ruling R1).
    pub seeded: bool,
    /// The provider's last `remove` event before the epoch.
    pub last_pre_epoch_remove_ms: Option<i64>,
}

impl Coverage {
    /// Complete from the epoch on; before it, only from seeded launches, and only after the
    /// last `remove` before the epoch: a removed account may have had a profile that left no
    /// trace.
    pub fn complete_at(&self, t: i64) -> bool {
        t >= self.epoch_ms || (self.seeded && self.last_pre_epoch_remove_ms.is_none_or(|r| t >= r))
    }
}

/// The times `run` launches cover, merged for a binary search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Launches {
    /// `[start, end)`, sorted by start and disjoint; an open launch ends at `i64::MAX`.
    spans: Vec<(i64, i64)>,
}

impl Launches {
    pub fn new(launches: Vec<Launch>) -> Self {
        let mut all: Vec<(i64, i64)> = launches
            .into_iter()
            .map(|l| (l.start_ms, l.end_ms.unwrap_or(i64::MAX)))
            .filter(|(start, end)| start < end)
            .collect();
        all.sort_unstable();
        let mut spans: Vec<(i64, i64)> = Vec::with_capacity(all.len());
        for (start, end) in all {
            match spans.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => spans.push((start, end)),
            }
        }
        Self { spans }
    }

    /// Whether a launch covers `t`: one started at or before it and had not ended by then.
    pub fn any_covers(&self, t: i64) -> bool {
        let i = self.spans.partition_point(|&(start, _)| start <= t);
        i > 0 && t < self.spans[i - 1].1
    }
}

/// Where a response ran, as far as the evidence tells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    Home(Home),
    Unattributed,
}

/// Token report §7.1–7.2, for a response at `t` whose session has `sightings`. Sighted in one
/// home at `t`: that home; in two: unattributed. Sighted nowhere: the default home when it is
/// the only candidate (no launch covers `t`, and launch coverage is complete at `t`), else
/// unattributed.
pub fn place(
    sightings: &[Sighting],
    launches: &Launches,
    coverage: &Coverage,
    t: i64,
) -> Placement {
    let mut sighted: Option<&Home> = None;
    for s in sightings
        .iter()
        .filter(|s| s.first_ms <= t && t <= s.last_ms)
    {
        match sighted {
            Some(home) if *home != s.home => return Placement::Unattributed,
            _ => sighted = Some(&s.home),
        }
    }
    match sighted {
        Some(home) => Placement::Home(home.clone()),
        None if !launches.any_covers(t) && coverage.complete_at(t) => {
            Placement::Home(Home::Default)
        }
        None => Placement::Unattributed,
    }
}

/// One of the provider's `events` rows that the timeline reads before the epoch (core §6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineEvent {
    Switch {
        at_ms: i64,
        from: Option<AccountId>,
        to: AccountId,
    },
    Add {
        at_ms: i64,
        account: AccountId,
    },
    Remove {
        at_ms: i64,
        account: AccountId,
    },
}

/// An `active_history` row: the provider's active account from `at_ms` on (token report §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveChange {
    pub at_ms: i64,
    pub account: Option<AccountId>,
}

/// The default home's account over time (token report §7.3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Timeline {
    epoch_ms: i64,
    /// Before the epoch: where the account changed, by `events`, in time order.
    before: Vec<(i64, Option<AccountId>)>,
    /// From the epoch on: `active_history`, in time order.
    after: Vec<(i64, Option<AccountId>)>,
}

impl Timeline {
    /// Token report §7.3. From `epoch_ms` on, the latest `history` row at or before a time.
    /// Before it, from `events`: the latest switch's `to`; before the first switch, its `from`
    /// from that account's earliest add; with no switch, the account `history` was seeded
    /// with (its first row), from its earliest add. `added_at` stands in for an account with
    /// no add event. A remove of an account ends its segment.
    pub fn build(
        events: &[TimelineEvent],
        history: &[ActiveChange],
        epoch_ms: i64,
        added_at: &BTreeMap<AccountId, i64>,
    ) -> Timeline {
        let mut after: Vec<(i64, Option<AccountId>)> = history
            .iter()
            .map(|c| (c.at_ms, c.account.clone()))
            .collect();
        after.sort_by_key(|(at, _)| *at);

        let mut switches: Vec<(i64, Option<&AccountId>, &AccountId)> = Vec::new();
        let mut adds: BTreeMap<&AccountId, i64> = BTreeMap::new();
        let mut removes: BTreeMap<&AccountId, Vec<i64>> = BTreeMap::new();
        for e in events {
            match e {
                TimelineEvent::Switch { at_ms, from, to } => {
                    switches.push((*at_ms, from.as_ref(), to));
                }
                TimelineEvent::Add { at_ms, account } => {
                    let first = adds.entry(account).or_insert(*at_ms);
                    *first = (*first).min(*at_ms);
                }
                TimelineEvent::Remove { at_ms, account } => {
                    removes.entry(account).or_default().push(*at_ms);
                }
            }
        }
        switches.sort_by_key(|(at, _, _)| *at);
        for times in removes.values_mut() {
            times.sort_unstable();
        }
        let added = |account: &AccountId| {
            adds.get(account)
                .copied()
                .or_else(|| added_at.get(account).copied())
        };

        let mut before: Vec<(i64, Option<AccountId>)> = Vec::new();
        // `account` from `start`, until `end` or its first remove before then.
        let mut segment = |account: &AccountId, start: i64, end: i64| {
            before.push((start, Some(account.clone())));
            let removed = removes
                .get(account)
                .and_then(|times| times.iter().copied().find(|r| (start..end).contains(r)));
            if let Some(r) = removed {
                before.push((r, None));
            }
        };
        match switches.first() {
            Some(&(first_at, from, _)) => {
                if let Some(from) = from {
                    if let Some(start) = added(from).filter(|start| *start < first_at) {
                        segment(from, start, first_at);
                    }
                }
                for (i, &(at, _, to)) in switches.iter().enumerate() {
                    let end = switches.get(i + 1).map_or(i64::MAX, |next| next.0);
                    segment(to, at, end);
                }
            }
            None => {
                // Only the row the migration seeded at the epoch: a later activation says
                // nothing about who was active before it.
                let seeded = history
                    .iter()
                    .filter(|c| c.at_ms <= epoch_ms)
                    .min_by_key(|c| c.at_ms)
                    .and_then(|c| c.account.as_ref());
                if let Some(account) = seeded {
                    if let Some(start) = added(account) {
                        segment(account, start, i64::MAX);
                    }
                }
            }
        }
        before.sort_by_key(|(at, _)| *at);
        Timeline {
            epoch_ms,
            before,
            after,
        }
    }

    /// The default home's account at `t`, when the evidence names one.
    pub fn account_at(&self, t: i64) -> Option<AccountId> {
        let changes = if t >= self.epoch_ms {
            &self.after
        } else {
            &self.before
        };
        let i = changes.partition_point(|(at, _)| *at <= t);
        i.checked_sub(1).and_then(|i| changes[i].1.clone())
    }
}

/// What a response's account rests on (token report §7, §10's `by_basis`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Basis {
    /// A profile's own account.
    Profile,
    /// The default home's account at the time, by the timeline.
    Timeline,
    Unattributed,
}

/// A placement's account (token report §7.3): a profile's is its own; the default home's is
/// the timeline's at `t`, and without one the response is unattributed.
pub fn attribute(placement: &Placement, timeline: &Timeline, t: i64) -> (Option<AccountId>, Basis) {
    match placement {
        Placement::Home(Home::Profile(account)) => (Some(account.clone()), Basis::Profile),
        Placement::Home(Home::Default) => match timeline.account_at(t) {
            Some(account) => (Some(account), Basis::Timeline),
            None => (None, Basis::Unattributed),
        },
        Placement::Unattributed => (None, Basis::Unattributed),
    }
}
```

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p tagteam-core --locked tokens::attribution`

Expected: `one_home_places_two_homes_conflict`, `a_sighting_never_covers_before_its_first_or_after_its_last`, `unsighted_is_default_only_without_a_covering_launch`, `an_open_launch_covers_until_closed`, `pre_epoch_coverage_needs_seeding_and_follows_the_last_remove`, `timeline_follows_switches_then_history`, `timeline_before_the_first_switch_uses_from_since_its_add`, `timeline_without_switches_uses_the_seeded_account`, `remove_ends_a_segment`, `attribute_maps_homes_to_accounts_and_bases` and the two properties `place_is_never_a_profile_without_a_sighting` and `merged_launches_cover_exactly_what_their_launches_cover` (512 pinned cases each) pass: `test result: ok. 12 passed; 0 failed`.

- [ ] **Step 5: Lint**

```sh
cargo fmt --all --check
cargo clippy -p tagteam-core --all-targets --locked -- -D warnings
```

Expected: no output from `fmt`; clippy finishes with no warning.

- [ ] **Step 6: Commit**

```sh
git add crates/tagteam-core/src/tokens/attribution.rs
git commit -m "Add attribution rules and the default home's timeline"
```

#### Notes for the controller

- **Launches are merged spans.** `Launches::new` merges the launches into sorted, disjoint `[start, end)` spans (an open one ends at `i64::MAX`), so `any_covers` is one binary search per response instead of a scan; `merged_launches_cover_exactly_what_their_launches_cover` checks it against the one-by-one rule. `Launches` and `Timeline` also derive `Debug, Clone, Default, PartialEq, Eq`, beyond the contract's bare structs; nothing depends on more.
- **The epoch boundary, a note for Task 11.** From `epoch_ms` on, a time before the first `active_history` row has no account: the timeline never stretches a row back to the epoch, because after `purge --provider` the first row is a later change, not the seed. So Task 11 should write `token_epoch.at` and the seeded `active_history.at` from one `now` value; two separate `<now ms>` statements can differ by a millisecond, leaving a response in between unattributed. Harmless, but free to avoid.
- **Spec §7.3 read literally.** "The first switch" is the first in all of `events`, after the epoch included, so before an epoch with no earlier switch the timeline still uses that switch's `from` from its add; switch events never affect a time at or after the epoch, which reads `history` alone. A `remove` at exactly `r` ends the segment at `r` (`t >= r` is unplaced), and `Coverage::complete_at` treats `t >= r` as complete, as the contract says.
- **An extra property** (`merged_launches_cover_exactly_what_their_launches_cover`) guards the merge.

---

### Task 5: Blocks and the active projection

**Depends on:** 2
**Files:**
- Modify: `crates/tagteam-core/src/tokens/blocks.rs` (replaces Task 2's stub whole, keeping its item verbatim)
- Test: `crates/tagteam-core/src/tokens/blocks.rs`

**Interfaces:**
- Consumes: `BLOCK_MS` (Task 2's stub, kept verbatim); `crate::tokens::test_support::pinned` (Task 2) and `crate::time::parse_iso8601` (existing), tests only.
- Produces:

```rust
pub const BLOCK_MS: i64 = 5 * 3_600_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockSpan { pub start_ms: i64, pub end_ms: i64, pub observed: bool }
/// Distinct resets -> [reset - 5h, reset); resets within 10 min collapse to the latest; sorted; each start clamped up to the previous end.
pub fn observed_blocks(resets_ms: &[i64]) -> Vec<BlockSpan>;
/// `times_ms` sorted ascending. Returns every block that holds at least one time (observed blocks that hold none are dropped),
/// sorted by start, and for each time the index of its block. Inferred blocks follow spec §9.
pub fn assign_blocks(observed: &[BlockSpan], times_ms: &[i64]) -> (Vec<BlockSpan>, Vec<usize>);
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection { pub elapsed_s: i64, pub remaining_s: i64, pub tokens_per_min: f64, pub cost_nanos_per_hour: Option<u64>, pub projected_tokens: u64, pub projected_cost_nanos: Option<u64> }
/// None unless start <= now < end. Rate over max(now - first_response, 60 s).
pub fn project(span: &BlockSpan, first_response_ms: i64, now_ms: i64, tokens: u64, cost_nanos: Option<u64>) -> Option<Projection>;
```

- [ ] **Step 1: Write the failing tests**

Append to the end of `crates/tagteam-core/src/tokens/blocks.rs`, after Task 2's stub, one blank line between them:

```rust
#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::tokens::test_support::pinned;

    const M: i64 = 60_000;
    const H: i64 = 60 * M;

    /// Epoch milliseconds of a UTC time on 2026-10-14, or of a whole ISO 8601 time.
    fn at(time: &str) -> i64 {
        let iso = if time.len() == 5 {
            format!("2026-10-14T{time}:00Z")
        } else {
            time.to_owned()
        };
        crate::time::parse_iso8601(&iso).unwrap() * 1000
    }

    fn span(start: &str, end: &str, observed: bool) -> BlockSpan {
        BlockSpan {
            start_ms: at(start),
            end_ms: at(end),
            observed,
        }
    }

    #[test]
    fn close_resets_collapse_to_the_latest() {
        // The API's reset time moves by seconds between readings, and can drift further.
        let resets = [
            at("2026-10-14T15:00:00Z"),
            at("2026-10-14T15:00:07Z"),
            at("2026-10-14T14:59:58Z"),
            at("15:09"),
            at("21:00"),
        ];
        assert_eq!(
            observed_blocks(&resets),
            [span("10:09", "15:09", true), span("16:00", "21:00", true)]
        );
        // More than ten minutes apart: two resets, the second clamped to the first's end.
        let apart = [at("15:00"), at("2026-10-14T15:10:01Z")];
        assert_eq!(
            observed_blocks(&apart),
            [
                span("10:00", "15:00", true),
                span("15:00", "2026-10-14T15:10:01Z", true)
            ]
        );
        assert!(observed_blocks(&[]).is_empty());
    }

    #[test]
    fn overlapping_observed_windows_are_clamped() {
        let resets = [at("13:00"), at("10:00")];
        assert_eq!(
            observed_blocks(&resets),
            [span("05:00", "10:00", true), span("10:00", "13:00", true)]
        );
    }

    #[test]
    fn an_inferred_block_starts_after_an_observed_one_in_the_same_hour() {
        let observed = observed_blocks(&[at("10:20")]);
        let times = [
            at("09:00"),
            at("10:40"),
            at("2026-10-14T14:59:59Z"),
            at("15:00"),
        ];
        let (blocks, of) = assign_blocks(&observed, &times);
        assert_eq!(
            blocks,
            [
                span("05:20", "10:20", true),
                span("10:20", "15:00", false),
                span("15:00", "20:00", false),
            ]
        );
        assert_eq!(of, [0, 1, 1, 2]);
    }

    #[test]
    fn an_inferred_block_ends_at_the_next_observed_start() {
        let observed = observed_blocks(&[at("14:00")]);
        let times = [at("06:30"), at("08:59"), at("09:30"), at("14:10")];
        let (blocks, of) = assign_blocks(&observed, &times);
        assert_eq!(
            blocks,
            [
                span("06:00", "09:00", false),
                span("09:00", "14:00", true),
                span("14:00", "19:00", false),
            ]
        );
        assert_eq!(of, [0, 0, 1, 2]);
    }

    #[test]
    fn blocks_without_responses_are_dropped() {
        let observed = observed_blocks(&[at("10:00"), at("20:00")]);
        let (blocks, of) = assign_blocks(&observed, &[at("16:00")]);
        assert_eq!(blocks, [span("15:00", "20:00", true)]);
        assert_eq!(of, [0]);
        assert_eq!(assign_blocks(&observed, &[]), (Vec::new(), Vec::new()));
        // A dropped observed block still bounds the inferred blocks around it.
        let (blocks, _) = assign_blocks(&observed, &[at("03:30"), at("10:30")]);
        assert_eq!(
            blocks,
            [span("03:00", "05:00", false), span("10:00", "15:00", false)]
        );
    }

    #[test]
    fn projection_only_inside_the_active_block() {
        let block = span("10:00", "15:00", false);
        let first = at("11:00");
        let at_now = |now: &str| project(&block, first, at(now), 6_000, Some(1_200_000));
        assert_eq!(at_now("2026-10-14T09:59:59Z"), None, "before it");
        assert_eq!(at_now("15:00"), None, "its end belongs to what follows");
        assert_eq!(
            at_now("12:00"),
            Some(Projection {
                elapsed_s: 2 * 3600,
                remaining_s: 3 * 3600,
                tokens_per_min: 100.0,
                cost_nanos_per_hour: Some(1_200_000),
                projected_tokens: 6_000 + 3 * 60 * 100,
                projected_cost_nanos: Some(1_200_000 + 3 * 1_200_000),
            })
        );
        let unpriced = project(&block, first, at("12:00"), 6_000, None).unwrap();
        assert_eq!(unpriced.cost_nanos_per_hour, None);
        assert_eq!(unpriced.projected_cost_nanos, None);
        // The rate is over a minute at least: a first response just now divides by no zero.
        let fresh = project(&block, at("12:00"), at("12:00"), 600, Some(60)).unwrap();
        assert_eq!(fresh.tokens_per_min, 600.0);
        assert_eq!(fresh.cost_nanos_per_hour, Some(3_600));
    }

    #[test]
    fn a_future_first_response_projects_nothing_negative() {
        let block = span("10:00", "15:00", false);
        let now = at("12:00");
        let p = project(&block, now + 10 * M, now, 1_000, Some(500)).unwrap();
        assert!(p.elapsed_s >= 0 && p.remaining_s >= 0);
        assert_eq!(p.tokens_per_min, 1_000.0, "over the one-minute minimum");
        assert!(p.projected_tokens >= 1_000);
        assert!(p.projected_cost_nanos.unwrap() >= 500);
        // Huge counts saturate instead of overflowing.
        let p = project(&block, now, now, u64::MAX, Some(u64::MAX)).unwrap();
        assert_eq!(p.projected_tokens, u64::MAX);
        assert_eq!(p.projected_cost_nanos, Some(u64::MAX));
        assert_eq!(p.cost_nanos_per_hour, Some(u64::MAX));
    }

    #[test]
    fn extreme_times_never_panic() {
        let observed = observed_blocks(&[i64::MIN, 0, i64::MAX]);
        let (blocks, of) = assign_blocks(&observed, &[i64::MIN, -1, 0, i64::MAX]);
        assert_eq!(of.len(), 4);
        assert!(of.iter().all(|&i| i < blocks.len()));
        for b in &blocks {
            let _ = project(b, i64::MIN, b.start_ms, u64::MAX, Some(u64::MAX));
            let _ = project(b, i64::MAX, b.start_ms, u64::MAX, None);
        }
    }

    proptest! {
        #![proptest_config(pinned(512))]

        /// Token report §9: blocks never overlap, so each response is in exactly one.
        #[test]
        fn every_time_is_in_exactly_one_block(
            resets in prop::collection::vec(0..60 * H, 0..6),
            mut times in prop::collection::vec(0..60 * H, 0..40),
        ) {
            times.sort_unstable();
            let observed = observed_blocks(&resets);
            prop_assert!(observed.windows(2).all(|w| w[0].end_ms <= w[1].start_ms));
            let (blocks, of) = assign_blocks(&observed, &times);
            prop_assert_eq!(of.len(), times.len());
            prop_assert!(blocks.windows(2).all(|w| w[0].end_ms <= w[1].start_ms));
            for (i, b) in blocks.iter().enumerate() {
                prop_assert!(b.start_ms < b.end_ms && b.end_ms - b.start_ms <= BLOCK_MS);
                prop_assert!(of.contains(&i), "a block with no response is dropped");
                if b.observed {
                    prop_assert!(observed.contains(b));
                } else {
                    prop_assert!(observed
                        .iter()
                        .all(|o| o.end_ms <= b.start_ms || b.end_ms <= o.start_ms));
                }
            }
            for (t, &i) in times.iter().zip(&of) {
                prop_assert!(blocks[i].start_ms <= *t && *t < blocks[i].end_ms);
                prop_assert_eq!(
                    blocks.iter().filter(|b| b.start_ms <= *t && *t < b.end_ms).count(),
                    1
                );
            }
        }
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-core --locked tokens::blocks`

Expected: compilation fails in `blocks.rs` (``could not compile `tagteam-core` (lib test)``) with ``error[E0412]: cannot find type `BlockSpan` in this scope``, ``error[E0422]: cannot find struct, variant or union type `Projection` in this scope`` and `error[E0425]: cannot find function` for `observed_blocks`, `assign_blocks` and `project`.

- [ ] **Step 3: Implement**

In `crates/tagteam-core/src/tokens/blocks.rs`, replace everything above `#[cfg(test)]` (Task 2's stub) with the following, then one blank line:

```rust
//! 5-hour blocks (token report §9): windows of one account's usage that never overlap, so each
//! response belongs to exactly one. Observed blocks come from the provider's short-window reset
//! times; inferred blocks cover the rest by ccusage's rule.

/// A block's length: the provider's short window, Claude Code's 5 h.
pub const BLOCK_MS: i64 = 5 * 3_600_000;
/// Reset times this close together are one reset: the API's moves by seconds between readings.
const RESET_SLACK_MS: i64 = 10 * 60_000;
const HOUR_MS: i64 = 3_600_000;
const MINUTE_MS: i64 = 60_000;

/// `[start_ms, end_ms)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockSpan {
    pub start_ms: i64,
    pub end_ms: i64,
    /// From a reset the provider reported, rather than inferred.
    pub observed: bool,
}

/// Token report §9's observed blocks: each distinct reset `r` gives `[r − 5 h, r)`. Resets
/// within ten minutes of the last one kept collapse into the later; in time order, a block
/// starts no earlier than the previous one ends.
pub fn observed_blocks(resets_ms: &[i64]) -> Vec<BlockSpan> {
    let mut sorted = resets_ms.to_vec();
    sorted.sort_unstable();
    let mut resets: Vec<i64> = Vec::with_capacity(sorted.len());
    for r in sorted {
        match resets.last_mut() {
            Some(last) if r.saturating_sub(*last) <= RESET_SLACK_MS => *last = r,
            _ => resets.push(r),
        }
    }
    let mut blocks: Vec<BlockSpan> = Vec::with_capacity(resets.len());
    for r in resets {
        let start = blocks.last().map_or(r.saturating_sub(BLOCK_MS), |prev| {
            r.saturating_sub(BLOCK_MS).max(prev.end_ms)
        });
        if start < r {
            blocks.push(BlockSpan {
                start_ms: start,
                end_ms: r,
                observed: true,
            });
        }
    }
    blocks
}

fn hour_floor(t: i64) -> i64 {
    t.saturating_sub(t.rem_euclid(HOUR_MS))
}

/// Token report §9: the block of each time. `observed` comes from `observed_blocks`, and
/// `times_ms` is sorted. A time inside an observed block is its; any other starts an inferred
/// block at its UTC hour floor, or at the end of the observed block before it if that is
/// later, ending 5 hours after the hour floor or at the next observed block's start if that is
/// earlier. Later times before that end join it. Returns the blocks that hold a time, sorted,
/// and each time's index into them.
pub fn assign_blocks(observed: &[BlockSpan], times_ms: &[i64]) -> (Vec<BlockSpan>, Vec<usize>) {
    let mut blocks: Vec<BlockSpan> = Vec::new();
    let mut of_time = Vec::with_capacity(times_ms.len());
    // The first observed block that has not ended by the time at hand.
    let mut next = 0;
    for &t in times_ms {
        while next < observed.len() && observed[next].end_ms <= t {
            next += 1;
        }
        let block = match (observed.get(next), blocks.last()) {
            (Some(o), _) if o.start_ms <= t => *o,
            (_, Some(last)) if !last.observed && t < last.end_ms => *last,
            _ => {
                let floor = hour_floor(t);
                let after = next.checked_sub(1).map(|i| observed[i].end_ms);
                let before = observed.get(next).map(|o| o.start_ms);
                BlockSpan {
                    start_ms: after.map_or(floor, |end| floor.max(end)),
                    end_ms: before.map_or(floor.saturating_add(BLOCK_MS), |start| {
                        floor.saturating_add(BLOCK_MS).min(start)
                    }),
                    observed: false,
                }
            }
        };
        if blocks.last() != Some(&block) {
            blocks.push(block);
        }
        of_time.push(blocks.len() - 1);
    }
    (blocks, of_time)
}

/// The active block's figures (token report §9).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    /// Since the block's start.
    pub elapsed_s: i64,
    /// Until the block's end.
    pub remaining_s: i64,
    pub tokens_per_min: f64,
    pub cost_nanos_per_hour: Option<u64>,
    /// The block's tokens and cost at its end, at the current rate.
    pub projected_tokens: u64,
    pub projected_cost_nanos: Option<u64>,
}

/// `n × num / den`, saturating at `u64::MAX`.
fn scale(n: u64, num: i64, den: i64) -> u64 {
    let v = u128::from(n) * u128::from(num.max(0) as u64) / u128::from(den.max(1) as u64);
    u64::try_from(v).unwrap_or(u64::MAX)
}

/// The figures of the block `span` at `now`, or `None` unless `start <= now < end`. The rate
/// runs over the time from the block's first response to now, a minute at least, so a first
/// response timestamped after now still gives a finite, non-negative rate.
pub fn project(
    span: &BlockSpan,
    first_response_ms: i64,
    now_ms: i64,
    tokens: u64,
    cost_nanos: Option<u64>,
) -> Option<Projection> {
    if !(span.start_ms <= now_ms && now_ms < span.end_ms) {
        return None;
    }
    let over_ms = now_ms.saturating_sub(first_response_ms).max(MINUTE_MS);
    let remaining_ms = span.end_ms.saturating_sub(now_ms);
    Some(Projection {
        elapsed_s: now_ms.saturating_sub(span.start_ms) / 1000,
        remaining_s: remaining_ms / 1000,
        tokens_per_min: tokens as f64 * MINUTE_MS as f64 / over_ms as f64,
        cost_nanos_per_hour: cost_nanos.map(|c| scale(c, HOUR_MS, over_ms)),
        projected_tokens: tokens.saturating_add(scale(tokens, remaining_ms, over_ms)),
        projected_cost_nanos: cost_nanos.map(|c| c.saturating_add(scale(c, remaining_ms, over_ms))),
    })
}
```

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p tagteam-core --locked tokens::blocks`

Expected: `close_resets_collapse_to_the_latest`, `overlapping_observed_windows_are_clamped`, `an_inferred_block_starts_after_an_observed_one_in_the_same_hour`, `an_inferred_block_ends_at_the_next_observed_start`, `blocks_without_responses_are_dropped`, `projection_only_inside_the_active_block`, `a_future_first_response_projects_nothing_negative`, `extreme_times_never_panic` and the property `every_time_is_in_exactly_one_block` (512 pinned cases) pass: `test result: ok. 9 passed; 0 failed`.

- [ ] **Step 5: Lint**

```sh
cargo fmt --all --check
cargo clippy -p tagteam-core --all-targets --locked -- -D warnings
```

Expected: no output from `fmt`; clippy finishes with no warning.

- [ ] **Step 6: Commit**

```sh
git add crates/tagteam-core/src/tokens/blocks.rs
git commit -m "Add observed and inferred 5-hour blocks with the active projection"
```

#### Notes for the controller

- **`elapsed_s` is since the block's start** (spec §9 names it without defining it; ccusage's "elapsed" is the same). The rates run from the block's first response to now, a minute at least, as the contract says; `remaining_s` is to the block's end.
- **The hour floor is UTC's** (epoch hours, as ccusage floors), so in a half-hour zone an inferred block starts at :30 local. Day and month buckets, by contrast, follow the query's zone (Task 6).
- **`assign_blocks` trusts its input:** `observed` is `observed_blocks`' output (sorted, disjoint) and `times_ms` is sorted; Task 18 must sort each account series' times before calling it. A dropped observed block (no response in it) still bounds the inferred blocks around it.
- **Overflow.** All block arithmetic saturates and the projection uses `u128` intermediates, so corrupt times or huge counts never panic (`extreme_times_never_panic`, an extra test, for Review Focus 3).

---

### Task 6: Day and month bucketing

**Depends on:** 2
**Files:**
- Modify: `crates/tagteam-core/src/tokens/bucket.rs` (replaces Task 2's stub whole, keeping its item verbatim)
- Test: `crates/tagteam-core/src/tokens/bucket.rs`

**Interfaces:**
- Consumes: `jiff` 0.2.37 (Task 2's workspace dependency); `day_of` and its private helper `zoned` (Task 2's stub, kept verbatim); `crate::time::parse_iso8601` (existing, tests only).
- Produces:

```rust
pub fn day_of(ms: i64, tz: &jiff::tz::TimeZone) -> jiff::civil::Date;
pub fn month_of(ms: i64, tz: &jiff::tz::TimeZone) -> (i16, i8);
pub fn day_bounds_ms(date: jiff::civil::Date, tz: &jiff::tz::TimeZone) -> (i64, i64);   // [local midnight, next local midnight)
pub fn month_bounds_ms(year: i16, month: i8, tz: &jiff::tz::TimeZone) -> (i64, i64);
```

- [ ] **Step 1: Write the failing tests**

Append to the end of `crates/tagteam-core/src/tokens/bucket.rs`, after Task 2's stub, one blank line between them:

```rust
#[cfg(test)]
mod tests {
    use jiff::civil::date;

    use super::*;

    /// A zone from the system's time zone database, which `TimeZone::system` and `--tz` read.
    fn zone(name: &str) -> TimeZone {
        TimeZone::get(name).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    /// Epoch milliseconds of an ISO 8601 time, by tagteam's own parser rather than jiff's.
    fn at(iso: &str) -> i64 {
        crate::time::parse_iso8601(iso).unwrap() * 1000
    }

    #[test]
    fn days_of_23_and_25_hours_bucket_by_the_local_calendar() {
        let brussels = zone("Europe/Brussels");
        // 2026-03-29: from 02:00 CET straight to 03:00 CEST, a 23-hour day.
        let (start, end) = day_bounds_ms(date(2026, 3, 29), &brussels);
        assert_eq!(
            (start, end),
            (at("2026-03-28T23:00:00Z"), at("2026-03-29T22:00:00Z"))
        );
        assert_eq!(end - start, 23 * 3_600_000);
        assert_eq!(day_of(start - 1, &brussels), date(2026, 3, 28));
        assert_eq!(day_of(start, &brussels), date(2026, 3, 29));
        assert_eq!(day_of(end - 1, &brussels), date(2026, 3, 29));
        assert_eq!(day_of(end, &brussels), date(2026, 3, 30));
        // 2026-10-25: from 03:00 CEST back to 02:00 CET, a 25-hour day.
        let (start, end) = day_bounds_ms(date(2026, 10, 25), &brussels);
        assert_eq!(
            (start, end),
            (at("2026-10-24T22:00:00Z"), at("2026-10-25T23:00:00Z"))
        );
        assert_eq!(end - start, 25 * 3_600_000);
        // Both 02:30s of the repeated hour, and 23:59 CET, are still the 25th.
        for iso in [
            "2026-10-25T00:30:00Z",
            "2026-10-25T01:30:00Z",
            "2026-10-25T22:59:00Z",
        ] {
            assert_eq!(day_of(at(iso), &brussels), date(2026, 10, 25), "{iso}");
        }
        assert_eq!(day_of(end, &brussels), date(2026, 10, 26));
        // In UTC the same instant falls on another day.
        let late = at("2026-10-25T23:30:00Z");
        assert_eq!(day_of(late, &TimeZone::UTC), date(2026, 10, 25));
        assert_eq!(day_of(late, &brussels), date(2026, 10, 26));
    }

    #[test]
    fn a_half_hour_zone_buckets_at_its_own_midnight() {
        let kolkata = zone("Asia/Kolkata");
        assert_eq!(
            day_bounds_ms(date(2026, 10, 14), &kolkata),
            (at("2026-10-13T18:30:00Z"), at("2026-10-14T18:30:00Z"))
        );
        assert_eq!(
            day_of(at("2026-10-13T18:29:59Z"), &kolkata),
            date(2026, 10, 13)
        );
        assert_eq!(
            day_of(at("2026-10-13T18:30:00Z"), &kolkata),
            date(2026, 10, 14)
        );
        assert_eq!(month_of(at("2026-09-30T18:29:59Z"), &kolkata), (2026, 9));
        assert_eq!(month_of(at("2026-09-30T18:30:00Z"), &kolkata), (2026, 10));
    }

    #[test]
    fn month_bounds_span_whole_local_months() {
        let brussels = zone("Europe/Brussels");
        // March 2026 begins in CET and ends in CEST: 31 days less an hour.
        let (start, end) = month_bounds_ms(2026, 3, &brussels);
        assert_eq!(
            (start, end),
            (at("2026-02-28T23:00:00Z"), at("2026-03-31T22:00:00Z"))
        );
        assert_eq!(end - start, (31 * 24 - 1) * 3_600_000);
        // October 2026 ends in CET: 31 days and an hour.
        let (start, end) = month_bounds_ms(2026, 10, &brussels);
        assert_eq!(
            (start, end),
            (at("2026-09-30T22:00:00Z"), at("2026-10-31T23:00:00Z"))
        );
        assert_eq!(end - start, (31 * 24 + 1) * 3_600_000);
        assert_eq!(month_of(start - 1, &brussels), (2026, 9));
        assert_eq!(month_of(start, &brussels), (2026, 10));
        assert_eq!(month_of(end - 1, &brussels), (2026, 10));
        assert_eq!(month_of(end, &brussels), (2026, 11));
        // December runs into the next year, and a leap February has 29 days.
        assert_eq!(
            month_bounds_ms(2026, 12, &TimeZone::UTC),
            (at("2026-12-01T00:00:00Z"), at("2027-01-01T00:00:00Z"))
        );
        assert_eq!(
            month_bounds_ms(2028, 2, &TimeZone::UTC),
            (at("2028-02-01T00:00:00Z"), at("2028-03-01T00:00:00Z"))
        );
    }

    #[test]
    fn bounds_round_trip_with_day_of() {
        let zones = [
            zone("Europe/Brussels"),
            zone("Asia/Kolkata"),
            zone("America/St_Johns"),
            TimeZone::UTC,
        ];
        // A year in steps that wander through the hour, then each DST change in Brussels in
        // quarter hours.
        let sweeps = [
            (at("2025-12-30T00:00:00Z"), at("2027-01-02T00:00:00Z"), 421),
            (at("2026-03-28T18:00:00Z"), at("2026-03-30T06:00:00Z"), 15),
            (at("2026-10-24T18:00:00Z"), at("2026-10-26T06:00:00Z"), 15),
        ];
        for tz in &zones {
            for (from, until, step_min) in sweeps {
                let mut ms = from;
                while ms < until {
                    let day = day_of(ms, tz);
                    let (start, end) = day_bounds_ms(day, tz);
                    assert!(start <= ms && ms < end, "{ms} in {day}");
                    assert_eq!(day_of(start, tz), day);
                    assert_eq!(day_of(end, tz), day.tomorrow().unwrap());
                    let (year, month) = month_of(ms, tz);
                    let (first, next) = month_bounds_ms(year, month, tz);
                    assert!(first <= ms && ms < next, "{ms} in {year}-{month}");
                    assert_eq!(day_of(first, tz), date(year, month, 1));
                    assert_eq!(
                        day_of(next, tz),
                        date(year, month, 1).last_of_month().tomorrow().unwrap()
                    );
                    ms += step_min * 60_000;
                }
            }
        }
    }

    #[test]
    fn extreme_times_never_panic() {
        for tz in [
            zone("Europe/Brussels"),
            zone("Pacific/Kiritimati"),
            TimeZone::UTC,
        ] {
            for ms in [i64::MIN, -1, 0, i64::MAX] {
                let (start, end) = day_bounds_ms(day_of(ms, &tz), &tz);
                assert!(start < end, "{ms}");
                let (year, month) = month_of(ms, &tz);
                let (first, next) = month_bounds_ms(year, month, &tz);
                assert!(first < next, "{ms}");
            }
            let (start, end) = day_bounds_ms(Date::MAX, &tz);
            assert!(start <= end);
            let (start, end) = month_bounds_ms(i16::MAX, 13, &tz);
            assert!(start <= end, "clamped to December 9999");
            let (start, end) = month_bounds_ms(i16::MIN, 0, &tz);
            assert!(start <= end, "clamped to January -9999");
        }
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-core --locked tokens::bucket`

Expected: compilation fails in `bucket.rs` (``could not compile `tagteam-core` (lib test)``) with `error[E0425]: cannot find function` for `month_of`, `day_bounds_ms` and `month_bounds_ms` (`day_of` is Task 2's).

- [ ] **Step 3: Implement**

In `crates/tagteam-core/src/tokens/bucket.rs`, replace everything above `#[cfg(test)]` (Task 2's stub) with the following, then one blank line:

```rust
//! Day and month bucketing (token report §10.1): calendar days and months in the report's time
//! zone, by the zone's own offsets, DST included, never by 24-hour arithmetic.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};

/// `ms` in `tz`, clamped to the range `jiff` represents.
fn zoned(ms: i64, tz: &TimeZone) -> Zoned {
    let at = Timestamp::from_millisecond(ms).unwrap_or(if ms < 0 {
        Timestamp::MIN
    } else {
        Timestamp::MAX
    });
    at.to_zoned(tz.clone())
}

/// The first instant of `date` in `tz`: its midnight, or the end of a gap that skips midnight.
/// Clamped to the range `jiff` represents.
fn start_of(date: Date, tz: &TimeZone) -> i64 {
    match date.to_zoned(tz.clone()) {
        Ok(z) => z.timestamp().as_millisecond(),
        Err(_) if date.year() < 1970 => Timestamp::MIN.as_millisecond(),
        Err(_) => Timestamp::MAX.as_millisecond(),
    }
}

/// The calendar day of `ms` in `tz`.
pub fn day_of(ms: i64, tz: &TimeZone) -> Date {
    zoned(ms, tz).date()
}

/// The calendar month of `ms` in `tz`, as (year, month).
pub fn month_of(ms: i64, tz: &TimeZone) -> (i16, i8) {
    let date = day_of(ms, tz);
    (date.year(), date.month())
}

/// `[the day's first instant, the next day's)` in `tz`: 23 or 25 hours across a DST change.
pub fn day_bounds_ms(date: Date, tz: &TimeZone) -> (i64, i64) {
    let end = date
        .tomorrow()
        .map_or(Timestamp::MAX.as_millisecond(), |next| start_of(next, tz));
    (start_of(date, tz), end)
}

/// `[the month's first instant, the next month's)` in `tz`. A month outside 1–12, or a year
/// outside what `jiff` represents, is clamped into it.
pub fn month_bounds_ms(year: i16, month: i8, tz: &TimeZone) -> (i64, i64) {
    let first = Date::new(year.clamp(-9999, 9999), month.clamp(1, 12), 1).unwrap_or(Date::MIN);
    (
        day_bounds_ms(first, tz).0,
        day_bounds_ms(first.last_of_month(), tz).1,
    )
}
```

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p tagteam-core --locked tokens::bucket`

Expected: `days_of_23_and_25_hours_bucket_by_the_local_calendar`, `a_half_hour_zone_buckets_at_its_own_midnight`, `month_bounds_span_whole_local_months`, `bounds_round_trip_with_day_of` and `extreme_times_never_panic` pass: `test result: ok. 5 passed; 0 failed`, in well under a second.

- [ ] **Step 5: Lint**

```sh
cargo fmt --all --check
cargo clippy -p tagteam-core --all-targets --locked -- -D warnings
```

Expected: no output from `fmt`; clippy finishes with no warning.

- [ ] **Step 6: Commit**

```sh
git add crates/tagteam-core/src/tokens/bucket.rs
git commit -m "Add local-calendar day and month bucketing"
```

#### Notes for the controller

- **Built on jiff's calendar, never on 24-hour steps.** A day's first instant is `Date::to_zoned`, whose compatible disambiguation gives midnight, or the end of a gap that skips midnight (some zones change at 00:00); the next day's first instant ends it. That is why the Brussels days of 2026-03-29 and 2026-10-25 come out 23 and 25 hours long, and `bounds_round_trip_with_day_of` checks every step of a year, and each Brussels change in quarter hours, in four zones (a half-hour DST zone, `America/St_Johns`, among them).
- **The zone database.** The tests name zones, which needs the system's zoneinfo (see Task 2's notes); the expected instants come from tagteam's own `parse_iso8601`, not from jiff, so the test does not grade jiff by itself.
- **No panics at the edges.** Times outside jiff's range clamp to `Timestamp::MIN`/`MAX`, and `month_bounds_ms` clamps a month outside 1–12 or a year outside ±9999 instead of failing, since the contract returns plain `(i64, i64)`; `extreme_times_never_panic` (an extra test, in `Pacific/Kiritimati` at +14 too) pins it.

---

### Task 7: Provider capability, usage lines, session ids and parent pids

**Depends on:** 2, 3
**Files:**
- Create: `crates/tagteam-provider/src/transcript.rs` (`UsageLine`, `LineError`)
- Modify: `crates/tagteam-provider/src/lib.rs` (`pub mod transcript;`, re-export `LineError`, `UsageLine`)
- Modify: `crates/tagteam-provider/src/provider.rs` (`Capabilities.transcripts`; trait methods `default_session_records_dir`, `transcript_files`, `parse_usage_line`, `price_table`, each with a default; test `defaults_have_no_transcripts`)
- Modify: `crates/tagteam-provider/src/liveness.rs` (`SessionRecord.session_id` read from `sessionId`; `ProcessProbe::parent_pid` with a default; `FakeProcess.ppid`; both probes answer `parent_pid`; tests, and the test literals the new fields break, listed below)
- Modify: `crates/tagteam-provider/src/process.rs` (`parent_of` beside `start_of`, sharing its `/proc/<pid>/stat` read on Linux and its `proc_pidinfo` call on macOS; test)
- Modify: `crates/tagteam-cc/src/provider.rs` (its full `Capabilities` literal gains `transcripts: false`; Task 8 turns it on)
- Test: `crates/tagteam-provider/src/liveness.rs`, `crates/tagteam-provider/src/process.rs`, `crates/tagteam-provider/src/provider.rs` (unit tests), `crates/tagteam-cc/tests/provider.rs` (its full `Capabilities` literal gains `transcripts: false`)

Every struct literal of the three changed structs in the workspace (found with `rg -n "Capabilities \{|SessionRecord \{|FakeProcess \{" crates/`), and what this task does to it:

| Struct | Site | Edit |
|---|---|---|
| `Capabilities` | `crates/tagteam-cc/src/provider.rs:254` (`capabilities()`) | add `transcripts: false` |
| `Capabilities` | `crates/tagteam-cc/tests/provider.rs:690` | add `transcripts: false` |
| `Capabilities` | `crates/tagteam/src/statusline.rs:527`, `crates/tagteam-fake/src/provider.rs:261`, `crates/tagteam-fake/tests/provider.rs:114` | none: `..Capabilities::default()` |
| `SessionRecord` | `crates/tagteam-provider/src/liveness.rs:81` (`parse_session_record`) | add `session_id` |
| `SessionRecord` | `liveness.rs` tests at 535, 544, 572, 796 (`rec`), 1033 (`mine`), 1074 (`gone`) | add `session_id` |
| `SessionRecord` | `liveness.rs:1040` (`recycled`) | none: `..mine.clone()` |
| `FakeProcess` | `liveness.rs:946` (the one full literal, in `without_a_usable_proc_start_cswaps_heuristic_decides`) | add `ppid: None` |
| `FakeProcess` | `liveness.rs` 788, 809, 858, 880, 888, 896, 913; `crates/xtask/src/compat/daemon.rs` 300, 337, 402, 411, 419, 473; `crates/tagteam-engine/tests/` `fake_sessions.rs` 97, 225, `session.rs` 128, 145, 179, 226, `session_views.rs` 78, `switch.rs` 1640, `purge.rs` 508, 555, 1025, `doctor.rs` 290, 323, 1447, 1718, `common/mod.rs` 1896 | none: each ends in `..FakeProcess::default()`, `..Default::default()` or `..running(..)` |

`Provider` is implemented only by `ClaudeCode` and `FakeAgent`, and `ProcessProbe` only by the two probes in `liveness.rs`; every new trait method has a default, so no other impl changes. Step 4's workspace check proves no literal was missed.

**Interfaces:**
- Consumes (Tasks 2–3, `tagteam_core::tokens`):
  ```rust
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
  pub struct TokenCounts { pub input: u64, pub output: u64, pub cache_write_5m: u64, pub cache_write_1h: u64, pub cache_read: u64, pub web_search: u64 }
  #[derive(Debug)]
  pub struct PriceTable { pub as_of: &'static str, pub models: &'static [(&'static str, ModelPrice)] }
  pub static EMPTY_PRICES: PriceTable = PriceTable { as_of: "", models: &[] };
  ```
- Produces (`tagteam_provider`, exactly the contract):
  ```rust
  // transcript.rs, re-exported at the crate root
  #[derive(Debug, Clone, PartialEq)]
  pub struct UsageLine {
      pub message_id: String,
      pub request_id: String,     // "" when absent (ruling R5)
      pub session_id: String,
      pub at_ms: i64,
      pub model: String,
      pub project: Option<String>, // the line's cwd
      pub sidechain: bool,
      pub counts: TokenCounts,
      pub speed: Option<String>,
      pub final_line: bool,        // the line carries a non-null stop_reason
  }
  #[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
  #[error("{0}")]
  pub struct LineError(pub String);

  // provider.rs
  pub struct Capabilities { /* existing */ pub transcripts: bool }
  // new trait methods, each with a default:
  fn transcript_files(&self, _env: &Env) -> io::Result<Vec<PathBuf>> { Ok(Vec::new()) }
  fn parse_usage_line(&self, _line: &[u8]) -> Result<Option<UsageLine>, LineError> { Ok(None) }
  fn price_table(&self) -> &'static PriceTable { &tagteam_core::tokens::EMPTY_PRICES }
  fn default_session_records_dir(&self, _env: &Env) -> Option<PathBuf> { None }   // ruling R2

  // liveness.rs
  pub struct SessionRecord { /* existing */ pub session_id: Option<String> }  // from "sessionId"
  pub trait ProcessProbe { /* existing */ fn parent_pid(&self, _pid: u32) -> Option<u32> { None } }  // ruling R3
  pub struct FakeProcess { /* existing */ pub ppid: Option<u32> }
  ```
  Also (crate-private): `process::parent_of(pid: u32) -> io::Result<Option<u32>>`, `Ok(None)` when no process has the pid or its parent pid is 0. `SystemProcessProbe::parent_pid` maps it, errors to `None`.

- [ ] **Step 1: Write the failing tests**

`crates/tagteam-provider/src/liveness.rs`, in `mod tests`: the existing literals gain the new fields, and two tests are added.

```diff
@@ mod tests {
                 pid: 4242,
                 proc_start: Some("Thu Oct  1 12:34:56 2026".into()),
                 started_at_ms: Some(1_790_858_096_000),
                 kind: Some("bg".into()),
+                session_id: Some("s".into()),
             }
         );
         assert_eq!(
             record(json!({"pid": 7})).unwrap(),
             SessionRecord {
                 pid: 7,
                 proc_start: None,
                 started_at_ms: None,
-                kind: None
+                kind: None,
+                session_id: None,
             }
         );
     }
 
@@ mod tests {
             SessionRecord {
                 pid: 7,
                 proc_start: None,
                 started_at_ms: None,
-                kind: None
+                kind: None,
+                session_id: None,
             }
         );
     }
 
+    #[test]
+    fn parse_session_record_reads_session_id() {
+        // Token report spec §2.4: a record names its session; anything but text is no id.
+        let id = |v: Value| record(v).unwrap().session_id;
+        assert_eq!(
+            id(json!({"pid": 7, "sessionId": "0193-abc"})).as_deref(),
+            Some("0193-abc")
+        );
+        assert_eq!(id(json!({"pid": 7})), None);
+        for odd in [
+            json!(null),
+            json!(7),
+            json!(true),
+            json!({"id": "s"}),
+            json!(["s"]),
+        ] {
+            assert_eq!(id(json!({"pid": 7, "sessionId": odd})), None);
+        }
+    }
+
     #[test]
     fn a_malformed_record_is_an_error_that_never_quotes_its_bytes() {
         let nested = |levels: usize| {
             let mut s = String::from("{\"pid\": 7, \"x\": ");
@@ mod tests {
             pid: 100,
             proc_start: proc_start.map(str::to_owned),
             started_at_ms,
             kind: Some("interactive".into()),
+            session_id: None,
         }
     }
 
     const LSTART: &str = "Thu Oct  1 12:34:56 2026";
@@ mod tests {
                         exists: Some(true),
                         start_time_s: known_start.then_some(start),
                         start_ticks: None,
                         mentions_launch: mentions,
+                        ppid: None,
                     },
                 );
                 assert_eq!(
                     record_is_live(&probe, &rec(proc_start, started_at), "claude"),
@@ mod tests {
             }
         }
     }
 
+    #[test]
+    fn fake_probe_reports_its_ppid() {
+        let probe = FakeProcessProbe::new();
+        probe.set(
+            100,
+            FakeProcess {
+                ppid: Some(42),
+                ..running(STARTED)
+            },
+        );
+        probe.set(101, running(STARTED));
+        assert_eq!(probe.parent_pid(100), Some(42));
+        assert_eq!(probe.parent_pid(101), None, "unknown when not set");
+        assert_eq!(probe.parent_pid(102), None, "an unknown pid has no parent");
+    }
+
     #[test]
     fn procargs_yield_the_executable_and_arguments_but_never_the_environment() {
         let mut buf = 2_i32.to_ne_bytes().to_vec();
         buf.extend_from_slice(b"/usr/local/bin/node\0\0\0\0");
@@ mod tests {
             pid: me,
             proc_start: Some(lstart_of(start)),
             started_at_ms: Some(start * 1000),
             kind: None,
+            session_id: None,
         };
         assert!(record_is_live(&p, &mine, stranger));
         let recycled = SessionRecord {
             proc_start: Some(lstart_of(start - 3_600)),
@@ mod tests {
             pid: dead,
             proc_start: None,
             started_at_ms: None,
             kind: None,
+            session_id: None,
         };
         assert!(!record_is_live(&p, &gone, "claude"));
     }
 }
```

`crates/tagteam-provider/src/process.rs`, in `mod tests`, before `an_exited_child_is_not_live`:

```rust
    #[test]
    fn parent_of_the_current_process_is_its_parent() {
        use crate::liveness::{ProcessProbe, SystemProcessProbe};
        let parent = std::os::unix::process::parent_id();
        assert_eq!(parent_of(std::process::id()).unwrap(), Some(parent));
        assert_eq!(
            SystemProcessProbe.parent_pid(std::process::id()),
            Some(parent)
        );
        assert_eq!(
            parent_of(i32::MAX as u32).unwrap(),
            None,
            "no process has it"
        );
        assert_eq!(SystemProcessProbe.parent_pid(i32::MAX as u32), None);
    }
```

`crates/tagteam-provider/src/provider.rs`, first test in `mod tests` (before `identity_debug_shows_no_email_label_or_name`):

```rust
    #[test]
    fn defaults_have_no_transcripts() {
        // Token report spec §3.2: a provider reads transcripts only when it says so, and the
        // default price table prices nothing.
        assert!(!Capabilities::default().transcripts);
        assert_eq!(EMPTY_PRICES.as_of, "");
        assert!(EMPTY_PRICES.models.is_empty());
    }
```

`crates/tagteam-cc/tests/provider.rs`, in `claude_code_has_every_capability_and_the_kind_table`:

```diff
@@ fn claude_code_has_every_capability_and_the_kind_table() {
             api_keys: true,
             sessions: true,
             statusline: true,
+            transcripts: false,
         }
     );
     for kind in f.cc.credential_kinds() {
```

- [ ] **Step 2: Run them and see them fail**

```
cargo test -p tagteam-provider --lib --locked -- parent_of_the_current_process_is_its_parent fake_probe_reports_its_ppid parse_session_record_reads_session_id defaults_have_no_transcripts
```

Expected: the lib tests do not compile, with `error[E0560]: struct `SessionRecord` has no field named `session_id``, `error[E0560]: struct `FakeProcess` has no field named `ppid``, `error[E0599]: no method named `parent_pid` found`, `error[E0425]: cannot find function `parent_of` in this scope`, `error[E0609]: no field `transcripts` on type `Capabilities`` and `error[E0425]: cannot find value `EMPTY_PRICES` in this scope`.

- [ ] **Step 3: Implement**

Create `crates/tagteam-provider/src/transcript.rs`:

```rust
//! The `transcripts` capability's neutral types (token report spec §3.2): what a provider reads
//! from one transcript line, whatever its own format.

use tagteam_core::tokens::TokenCounts;

/// One transcript line that carries usage (spec §3.2). Several lines may describe one response;
/// the index merges them by `(message_id, request_id)` (spec §2.2).
#[derive(Debug, Clone, PartialEq)]
pub struct UsageLine {
    pub message_id: String,
    /// `""` when the line has none (ruling R5).
    pub request_id: String,
    pub session_id: String,
    /// Epoch milliseconds.
    pub at_ms: i64,
    pub model: String,
    /// The line's working directory.
    pub project: Option<String>,
    pub sidechain: bool,
    pub counts: TokenCounts,
    pub speed: Option<String>,
    /// The line carries a non-null stop reason, so its output count is final (spec §2.2).
    pub final_line: bool,
}

/// A line that should carry usage and cannot be read (spec §5.4). The text never quotes the
/// line.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct LineError(pub String);
```

`crates/tagteam-provider/src/lib.rs`:

```diff
@@ pub mod reservation;
 pub mod security;
 pub mod splice;
 pub mod term;
+pub mod transcript;
 
 /// Serialises the lib tests that fork a child or drop a flock and re-lock it: a child forked in
 /// that window briefly holds a duplicate of the lock's open file description, so the re-lock
@@ pub use reservation::{LaunchReservation, remove_dead_reservations};
 pub use tagteam_core::pace::Pace;
 pub use tagteam_core::poll::PollBudget;
 pub use tagteam_core::usage::Window;
+pub use transcript::{LineError, UsageLine};
```

`crates/tagteam-provider/src/provider.rs`:

```diff
@@ use serde_json::Value;
 use tagteam_core::backoff::parse_retry_after;
 use tagteam_core::pace::Pace;
 use tagteam_core::poll::PollBudget;
+use tagteam_core::tokens::{EMPTY_PRICES, PriceTable};
 use tagteam_core::usage::Window;
 use tagteam_core::{Fingerprint, IdentityKey, ProviderId};
 
@@ use crate::keychain::{KeychainError, LockState};
 use crate::mkdir_lock::LockError;
 use crate::process::ProcessSpawner;
 use crate::read::{Read, ReadError};
+use crate::transcript::{LineError, UsageLine};
 
 /// A login's identity. `raw` is the provider-owned object stored in `identity_json`
 /// (CC: the `oauthAccount` object).
@@ pub struct Capabilities {
     pub api_keys: bool,
     pub sessions: bool,
     pub statusline: bool,
+    /// Reads its transcripts for the token report (token report spec §3.2).
+    pub transcripts: bool,
 }
 
 /// What the engine and the CLI need to know about one credential kind, so neither ever names a
@@ pub trait Provider: Send + Sync {
     fn share_policy(&self, env: &Env) -> SharePolicy;
     /// Where the profile's session records live (CC: `<profile>/sessions`).
     fn session_records_dir(&self, profile: &Path) -> PathBuf;
+    /// Where the default home's session records live (CC: `<config_home>/sessions`), for the
+    /// token report's sightings (token report spec §6.2, ruling R2). `None` (the default) for a
+    /// provider without the `transcripts` capability.
+    fn default_session_records_dir(&self, _env: &Env) -> Option<PathBuf> {
+        None
+    }
     /// Where the profile's background supervisor records itself, for an agent whose supervisor
     /// writes no session record of its own (CC 2.1.292: `<profile>/daemon.lock`, §12.6). A live
     /// supervisor there makes the profile session-owned; one that cannot be read counts as
@@ pub trait Provider: Send + Sync {
     fn keychain_lock_state(&self) -> Option<LockState> {
         None
     }
+
+    // Token report spec §3.2: the `transcripts` capability. A provider without it keeps these
+    // defaults, and the engine never calls them.
+    /// Every transcript file under the default home, in no particular order.
+    fn transcript_files(&self, _env: &Env) -> io::Result<Vec<PathBuf>> {
+        Ok(Vec::new())
+    }
+    /// One line of a transcript: `None` for a line that carries no usage, `Err` for one that
+    /// should and cannot be read (spec §5.4).
+    fn parse_usage_line(&self, _line: &[u8]) -> Result<Option<UsageLine>, LineError> {
+        Ok(None)
+    }
+    /// The shipped price table and the date its prices were taken (spec §8.1).
+    fn price_table(&self) -> &'static PriceTable {
+        &EMPTY_PRICES
+    }
 }
 
 #[cfg(test)]
 mod tests {
```

`crates/tagteam-provider/src/liveness.rs` (the code half; Step 1 changed the tests):

```diff
@@ use std::sync::{Mutex, PoisonError};
 use serde_json::Value;
 
 use crate::read::{Read, ReadError};
 
-/// One `<config_home>/sessions/<pid>.json` (Appendix A.7), reduced to what liveness needs.
+/// One `<config_home>/sessions/<pid>.json` (Appendix A.7), reduced to what liveness and the
+/// token report's sightings need.
 #[derive(Debug, Clone, PartialEq, Eq)]
 pub struct SessionRecord {
     pub pid: u32,
     /// `ps -o lstart=` text, or the all-digit legacy form; `None` when absent or not text.
     pub proc_start: Option<String>,
     /// `startedAt`, epoch ms.
     pub started_at_ms: Option<i64>,
     pub kind: Option<String>,
+    /// `sessionId`; `None` when absent or not text (token report spec §2.4).
+    pub session_id: Option<String>,
 }
 
 /// A record nested deeper than this is malformed (§12.6 "deep nesting").
 const MAX_DEPTH: usize = 32;
@@ pub fn parse_session_record(bytes: &[u8]) -> Result<SessionRecord, String> {
         pid,
         proc_start,
         started_at_ms: o.get("startedAt").and_then(Value::as_i64),
         kind: o.get("kind").and_then(Value::as_str).map(str::to_owned),
+        session_id: o
+            .get("sessionId")
+            .and_then(Value::as_str)
+            .map(str::to_owned),
     })
 }
 
 #[derive(Debug, Clone, PartialEq)]
@@ pub trait ProcessProbe: Send + Sync {
     /// Linux `/proc/<pid>/stat` field 22, for the all-digit legacy `procStart`.
     fn start_ticks(&self, pid: u32) -> Option<u64>;
     /// Whether the executable name or the arguments contain `needle`.
     fn mentions(&self, pid: u32, needle: &str) -> Option<bool>;
+    /// The parent's pid, for the statusline's walk to its session (token report spec §6.3,
+    /// ruling R3). `None` when it cannot be determined, or the process has no parent.
+    fn parent_pid(&self, _pid: u32) -> Option<u32> {
+        None
+    }
 }
 
 fn contains(haystack: &[u8], needle: &[u8]) -> bool {
     !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
@@ impl ProcessProbe for SystemProcessProbe {
             (Some(one), None) | (None, Some(one)) if contains(&one, needle) => Some(true),
             _ => None,
         }
     }
+
+    fn parent_pid(&self, pid: u32) -> Option<u32> {
+        crate::process::parent_of(pid).ok()?
+    }
 }
 
 /// `btime` in `/proc/stat`: the boot time, epoch seconds.
 #[cfg(target_os = "linux")]
@@ pub struct FakeProcess {
     pub start_time_s: Option<i64>,
     pub start_ticks: Option<u64>,
     /// `mentions`' answer, whatever the needle.
     pub mentions_launch: Option<bool>,
+    /// `parent_pid`'s answer.
+    pub ppid: Option<u32>,
 }
 
 /// Tests only (no feature gate: it touches nothing). Unknown pids are `exists: Some(false)`.
 #[derive(Default)]
@@ impl ProcessProbe for FakeProcessProbe {
 
     fn mentions(&self, pid: u32, _needle: &str) -> Option<bool> {
         self.get(pid)?.mentions_launch
     }
+
+    fn parent_pid(&self, pid: u32) -> Option<u32> {
+        self.get(pid)?.ppid
+    }
 }
 
 /// How long after `startedAt` a process must have started, by cswap's rule, before it can be
 /// judged a recycled pid (§12.6).
```

`crates/tagteam-provider/src/process.rs`: the Linux `start_of` becomes a field reader both functions share, and the macOS `proc_pidinfo` call moves into `bsd_info`, which both read. Replace the two `start_of` functions (from `/// `/proc/<pid>/stat` field 22, counted after the last `)`.` through the end of the macOS `start_of`) with:

```rust
/// Field `n` (3 or later) of `/proc/<pid>/stat`, counted after the last `)`; `None` when no
/// process has the pid.
#[cfg(target_os = "linux")]
fn stat_field(pid: u32, n: usize) -> io::Result<Option<u64>> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let after = stat
        .rfind(')')
        .map(|i| &stat[i + 1..])
        .ok_or_else(|| io::Error::other("bad stat"))?;
    // `after` starts at field 3.
    after
        .split_whitespace()
        .nth(n - 3)
        .and_then(|f| f.parse().ok())
        .map(Some)
        .ok_or_else(|| io::Error::other("bad stat"))
}

/// `/proc/<pid>/stat` field 22.
#[cfg(target_os = "linux")]
pub(crate) fn start_of(pid: u32) -> io::Result<Option<u64>> {
    stat_field(pid, 22)
}

/// `/proc/<pid>/stat` field 4. `None` when no process has the pid, or it has no parent.
#[cfg(target_os = "linux")]
pub(crate) fn parent_of(pid: u32) -> io::Result<Option<u32>> {
    Ok(stat_field(pid, 4)?
        .and_then(|p| u32::try_from(p).ok())
        .filter(|p| *p > 0))
}

/// `proc_pidinfo(PROC_PIDTBSDINFO)`; `None` when no process has the pid.
#[cfg(target_os = "macos")]
fn bsd_info(pid: u32) -> io::Result<Option<libc::proc_bsdinfo>> {
    // SAFETY: `proc_bsdinfo` is a C struct of plain integers; the zeroed value is a valid
    // bit pattern for it and is fully overwritten by `proc_pidinfo` below on success.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is a correctly sized, writable proc_bsdinfo.
    let n = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    if n == size {
        return Ok(Some(info));
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::ESRCH) {
        Ok(None)
    } else {
        Err(e)
    }
}

/// `proc_pidinfo(PROC_PIDTBSDINFO)` start time, in microseconds.
#[cfg(target_os = "macos")]
pub(crate) fn start_of(pid: u32) -> io::Result<Option<u64>> {
    Ok(bsd_info(pid)?.map(|info| info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec))
}

/// `pbi_ppid` of the same `proc_bsdinfo`. `None` when no process has the pid, or it has no
/// parent.
#[cfg(target_os = "macos")]
pub(crate) fn parent_of(pid: u32) -> io::Result<Option<u32>> {
    Ok(bsd_info(pid)?.map(|info| info.pbi_ppid).filter(|p| *p > 0))
}
```

`start_of` keeps its behaviour exactly (same field, same errors, same `pid as c_int`). `libc::proc_bsdinfo::pbi_ppid` is a `u32` in the locked libc 0.2.189.

`crates/tagteam-cc/src/provider.rs`:

```diff
@@ impl Provider for ClaudeCode {
             api_keys: true,
             sessions: true,
             statusline: true,
+            transcripts: false,
         }
     }
 
```

- [ ] **Step 4: Run them and see them pass**

```
cargo test -p tagteam-provider --lib --locked
cargo test -p tagteam-cc --test provider --locked claude_code_has_every_capability_and_the_kind_table
cargo check --workspace --all-targets --locked --features tagteam/test-support
```

Expected: the provider lib tests all pass (246/246 on macOS at the plan's base, the four new ones among them); the CC capability test passes; the workspace check finishes with no error, which shows no struct literal was missed.

- [ ] **Step 5: Lint**

```
cargo fmt --all --check
cargo clippy -p tagteam-provider -p tagteam-cc -p tagteam-fake -p xtask --all-targets --locked -- -D warnings
```

Expected: no output from fmt; clippy finishes with no warning.

- [ ] **Step 6: Commit**

```
git add crates/tagteam-provider/src/transcript.rs crates/tagteam-provider/src/lib.rs crates/tagteam-provider/src/provider.rs crates/tagteam-provider/src/liveness.rs crates/tagteam-provider/src/process.rs crates/tagteam-cc/src/provider.rs crates/tagteam-cc/tests/provider.rs
git commit -m "Add the transcripts capability, usage lines, record session ids and parent pids"
```

#### Notes for the controller

- Verified by the plan writer: this task's code was applied to a copy of the branch with contract-exact stubs of Tasks 2–3, and Steps 4–5 passed on macOS (Apple silicon). The Linux `stat_field`/`parent_of` path was not compiled or run; it is the existing `start_of` parse with the field number as a parameter (`nth(n - 3)`, `n = 22` gives the old `nth(19)`). The full suite on Linux CI covers it through `parent_of_the_current_process_is_its_parent`.
- `defaults_have_no_transcripts` pins `Capabilities::default()` and `EMPTY_PRICES`. The trait's default method bodies are not called by any test: after Tasks 8 and 10 both providers override them, and a stub `Provider` for one test would need some 58 methods. Task 10 adds `FakeAgent::without_transcripts()` so Task 15 can test a provider without the capability.
- `parent_of` answers `None` for a parent pid of 0 (launchd and kernel threads have none), so the statusline's walk (Task 16) stops there rather than probing pid 0.

---

### Task 8: Claude Code transcript discovery and parser

**Depends on:** 7
**Files:**
- Create: `crates/tagteam-cc/src/transcripts.rs` (`discover`, the prefilter, `parse_usage_line`)
- Create: `crates/tagteam-cc/src/prices.rs` (`PRICES`, empty; Task 9 fills it)
- Create: `crates/tagteam-cc/tests/transcripts.rs`
- Create: `crates/tagteam-cc/tests/parser_memory.rs` (its own test binary: a counting global allocator pins that a huge `iterations` array is skipped, not collected)
- Create: `crates/tagteam-cc/tests/fixtures/transcripts/projects/-w-app/11111111-1111-4111-8111-111111111111.jsonl`, `crates/tagteam-cc/tests/fixtures/transcripts/projects/-w-app/11111111-1111-4111-8111-111111111111/subagents/agent-0123456789abcdef.jsonl`, `crates/tagteam-cc/tests/fixtures/transcripts/projects/-w-app/11111111-1111-4111-8111-111111111111/subagents/agent-0123456789abcdef.meta.json`, `crates/tagteam-cc/tests/fixtures/transcripts/projects/-w-app/22222222-2222-4222-8222-222222222222.jsonl`, `crates/tagteam-cc/tests/fixtures/transcripts/projects/-w-lib/33333333-3333-4333-8333-333333333333.jsonl` (synthetic fixture tree; every text field is `"x"`)
- Modify: `crates/tagteam-core/src/time.rs` (`parse_iso8601` drops the fraction, so this task adds `parse_iso8601_ms`, sharing one parser, with its own tests)
- Modify: `crates/tagteam-cc/Cargo.toml` (`serde.workspace = true`; dev-dependency `proptest.workspace = true`, already a workspace dependency)
- Modify: `Cargo.toml` (the comment on the workspace `proptest`, which said "tagteam-core only")
- Modify: `Cargo.lock` (tagteam-cc gains `proptest` and `serde`; both are already locked, so nothing is fetched)
- Modify: `crates/tagteam-cc/src/lib.rs` (`pub mod prices; pub mod transcripts;`)
- Modify: `crates/tagteam-cc/src/provider.rs` (`transcripts: true`; `default_session_records_dir`, `transcript_files`, `parse_usage_line`, `price_table`)
- Test: `crates/tagteam-cc/tests/transcripts.rs`, `crates/tagteam-cc/tests/provider.rs` (capability test; new `claude_code_reads_transcripts_under_its_config_home`), `crates/tagteam-cc/src/transcripts.rs` (prefilter unit tests), `crates/tagteam-core/src/time.rs` (unit tests)

**Interfaces:**
- Consumes:
  ```rust
  // tagteam_core::tokens (Tasks 2-3)
  pub struct TokenCounts { pub input: u64, pub output: u64, pub cache_write_5m: u64, pub cache_write_1h: u64, pub cache_read: u64, pub web_search: u64 }
  impl TokenCounts { pub fn merge_max(&mut self, other: &TokenCounts); } // tests only
  pub struct PriceTable { pub as_of: &'static str, pub models: &'static [(&'static str, ModelPrice)] }
  // tagteam_provider (Task 7)
  pub struct UsageLine { pub message_id: String, pub request_id: String, pub session_id: String, pub at_ms: i64, pub model: String, pub project: Option<String>, pub sidechain: bool, pub counts: TokenCounts, pub speed: Option<String>, pub final_line: bool }
  pub struct LineError(pub String);
  pub struct Capabilities { /* existing */ pub transcripts: bool }
  fn transcript_files(&self, _env: &Env) -> io::Result<Vec<PathBuf>>;
  fn parse_usage_line(&self, _line: &[u8]) -> Result<Option<UsageLine>, LineError>;
  fn price_table(&self) -> &'static PriceTable;
  fn default_session_records_dir(&self, _env: &Env) -> Option<PathBuf>;
  ```
- Produces:
  ```rust
  // tagteam_core::time
  pub fn parse_iso8601_ms(s: &str) -> Option<i64>; // epoch ms; same forms as parse_iso8601, fraction truncated to ms
  // tagteam_cc::transcripts
  pub fn discover(projects: &Path) -> io::Result<Vec<PathBuf>>; // sorted; missing dir -> Ok(vec![])
  pub fn parse_usage_line(line: &[u8]) -> Result<Option<UsageLine>, LineError>;
  // tagteam_cc::prices
  pub static PRICES: PriceTable; // empty here: as_of "", no models (Task 9 fills it)
  // ClaudeCode (Provider): capabilities().transcripts == true;
  //   transcript_files(env) = discover(<config_home>/projects);
  //   parse_usage_line = transcripts::parse_usage_line; price_table() = &prices::PRICES;
  //   default_session_records_dir(env) = Some(<config_home>/sessions)
  ```
  The fixture tree `crates/tagteam-cc/tests/fixtures/transcripts/projects` holds four transcript files with four responses over eight usage lines and two unreadable lines; `the_fixture_tree_parses_to_known_responses` pins every count, so the engine's refresh tests (Task 15) can copy the tree into a temp `<home>/.claude/projects` and assert against the same numbers.

- [ ] **Step 1: Write the failing tests**

First the manifests, so the tests can build: `Cargo.toml`, `crates/tagteam-cc/Cargo.toml` and the resulting `Cargo.lock`:

```diff
--- a/Cargo.lock
+++ b/Cargo.lock
@@ the `tagteam-cc` package's `dependencies = [` list
  "fastrand",
  "hex",
  "libc",
+ "proptest",
+ "serde",
  "serde_json",
  "sha2 0.10.9",
  "tagteam-core",
--- a/Cargo.toml
+++ b/Cargo.toml
@@ uuid = { version = "1", features = ["v7"] }
 # test-only
 assert_cmd = "2"
 predicates = "3"
-# §11.5's simulations (tagteam-core only): std alone, without the fork and timeout features.
+# §11.5's simulations (tagteam-core), and the token report's transcript parser (tagteam-cc): std
+# alone, without the fork and timeout features.
 proptest = { version = "1.11", default-features = false, features = ["std"] }
 tempfile = "3"
 
--- a/crates/tagteam-cc/Cargo.toml
+++ b/crates/tagteam-cc/Cargo.toml
@@ tagteam-provider.workspace = true
 fastrand.workspace = true
 hex.workspace = true
 libc.workspace = true
+serde.workspace = true
 serde_json.workspace = true
 sha2.workspace = true
 thiserror.workspace = true
@@ unicode-normalization.workspace = true
 
 [dev-dependencies]
 tagteam-provider = { workspace = true, features = ["file-keychain"] }
+proptest.workspace = true
 tempfile.workspace = true
 
 [lints]
```

Edit the two `Cargo.toml` files, then let Cargo write the lock (both crates are already locked, so this needs no network): `cargo check -p tagteam-cc --tests`. The lock's only change is the two lines above.

The fixture tree. Write each file exactly, ending with a newline:

`crates/tagteam-cc/tests/fixtures/transcripts/projects/-w-app/11111111-1111-4111-8111-111111111111.jsonl`:

```text
{"parentUuid":null,"isSidechain":false,"userType":"external","cwd":"/w/app","sessionId":"11111111-1111-4111-8111-111111111111","version":"2.1.299","gitBranch":"main","type":"user","message":{"role":"user","content":"x"},"uuid":"u1","timestamp":"2026-10-14T09:00:00.000Z"}
{"parentUuid":"u1","isSidechain":false,"userType":"external","cwd":"/w/app","sessionId":"11111111-1111-4111-8111-111111111111","version":"2.1.299","gitBranch":"main","message":{"id":"msg_A","type":"message","role":"assistant","model":"claude-opus-5-5","content":[{"type":"thinking","thinking":"x","signature":"x"}],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":10,"cache_creation_input_tokens":300,"cache_read_input_tokens":2000,"cache_creation":{"ephemeral_5m_input_tokens":200,"ephemeral_1h_input_tokens":100},"output_tokens":5,"service_tier":"standard"}},"requestId":"req_A","type":"assistant","uuid":"a1","timestamp":"2026-10-14T09:00:01.100Z"}
{"parentUuid":"a1","isSidechain":false,"userType":"external","cwd":"/w/app","sessionId":"11111111-1111-4111-8111-111111111111","version":"2.1.299","gitBranch":"main","message":{"id":"msg_A","type":"message","role":"assistant","model":"claude-opus-5-5","content":[{"type":"text","text":"x"}],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":10,"cache_creation_input_tokens":300,"cache_read_input_tokens":2000,"cache_creation":{"ephemeral_5m_input_tokens":200,"ephemeral_1h_input_tokens":100},"output_tokens":40,"service_tier":"standard"}},"requestId":"req_A","type":"assistant","uuid":"a2","timestamp":"2026-10-14T09:00:01.900Z"}
{"parentUuid":"a2","isSidechain":false,"userType":"external","cwd":"/w/app","sessionId":"11111111-1111-4111-8111-111111111111","version":"2.1.299","gitBranch":"main","message":{"id":"msg_A","type":"message","role":"assistant","model":"claude-opus-5-5","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"x"}}],"stop_reason":"tool_use","stop_sequence":null,"usage":{"input_tokens":10,"cache_creation_input_tokens":300,"cache_read_input_tokens":2000,"cache_creation":{"ephemeral_5m_input_tokens":200,"ephemeral_1h_input_tokens":100},"output_tokens":120,"service_tier":"standard","server_tool_use":{"web_search_requests":1,"web_fetch_requests":0},"speed":"standard","output_tokens_details":{"thinking_tokens":60},"iterations":[{"input_tokens":10,"output_tokens":120,"cache_read_input_tokens":2000,"cache_creation_input_tokens":300,"cache_creation":{"ephemeral_5m_input_tokens":200,"ephemeral_1h_input_tokens":100},"type":"message"}]}},"requestId":"req_A","type":"assistant","uuid":"a3","timestamp":"2026-10-14T09:00:02.500Z"}
{"parentUuid":"a3","isSidechain":false,"userType":"external","cwd":"/w/app","sessionId":"11111111-1111-4111-8111-111111111111","version":"2.1.299","gitBranch":"main","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"x"}]},"uuid":"u2","timestamp":"2026-10-14T09:00:03.000Z"}
{"parentUuid":"u2","isSidechain":false,"userType":"external","cwd":"/w/app","sessionId":"11111111-1111-4111-8111-111111111111","version":"2.1.299","gitBranch":"main","type":"system","subtype":"informational","content":"x","level":"info","uuid":"y1","timestamp":"2026-10-14T09:00:03.500Z"}
{"parentUuid":"y1","isSidechain":false,"userType":"external","cwd":"/w/app","sessionId":"11111111-1111-4111-8111-111111111111","version":"2.1.299","gitBranch":"main","message":{"id":"msg_syn","type":"message","role":"assistant","model":"<synthetic>","content":[{"type":"text","text":"x"}],"stop_reason":"stop_sequence","stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"type":"assistant","uuid":"y2","timestamp":"2026-10-14T09:00:04.000Z"}
{"parentUuid":"y2","isSidechain":false,"userType":"external","cwd":"/w/app","sessionId":"11111111-1111-4111-8111-111111111111","version":"2.1.299","gitBranch":"main","type":"user","message":{"role":"user","content":"x"},"uuid":"u3","timestamp":"2026-10-14T09:04:59.000Z"}
{"parentUuid":"u3","isSidechain":false,"userType":"external","cwd":"/w/app","sessionId":"11111111-1111-4111-8111-111111111111","version":"2.1.299","gitBranch":"main","message":{"id":"msg_B","type":"message","role":"assistant","model":"claude-sonnet-5-5","content":[{"type":"text","text":"x"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0},"output_tokens":0,"service_tier":"standard","server_tool_use":{"web_search_requests":0,"web_fetch_requests":0},"speed":"standard","iterations":[{"input_tokens":3,"output_tokens":50,"cache_read_input_tokens":500,"cache_creation_input_tokens":40,"cache_creation":{"ephemeral_5m_input_tokens":40,"ephemeral_1h_input_tokens":0},"type":"message"}]}},"requestId":"req_B","type":"assistant","uuid":"b1","timestamp":"2026-10-14T09:05:00.000Z"}
```

`crates/tagteam-cc/tests/fixtures/transcripts/projects/-w-app/11111111-1111-4111-8111-111111111111/subagents/agent-0123456789abcdef.jsonl`:

```text
{"parentUuid":null,"isSidechain":true,"userType":"external","cwd":"/w/app","sessionId":"11111111-1111-4111-8111-111111111111","version":"2.1.299","gitBranch":"main","agentId":"0123456789abcdef","type":"user","message":{"role":"user","content":"x"},"uuid":"s1","timestamp":"2026-10-14T09:00:59.000Z"}
{"parentUuid":"s1","isSidechain":true,"userType":"external","cwd":"/w/app","sessionId":"11111111-1111-4111-8111-111111111111","version":"2.1.299","gitBranch":"main","agentId":"0123456789abcdef","message":{"id":"msg_C","type":"message","role":"assistant","model":"claude-haiku-4-5-20251001","content":[{"type":"text","text":"x"}],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":7,"cache_creation_input_tokens":64,"cache_read_input_tokens":1000,"output_tokens":8,"service_tier":"standard"}},"requestId":"req_C","type":"assistant","uuid":"c1","timestamp":"2026-10-14T09:01:00.000Z"}
{"parentUuid":"c1","isSidechain":true,"userType":"external","cwd":"/w/app","sessionId":"11111111-1111-4111-8111-111111111111","version":"2.1.299","gitBranch":"main","agentId":"0123456789abcdef","message":{"id":"msg_C","type":"message","role":"assistant","model":"claude-haiku-4-5-20251001","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"x"}}],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":7,"cache_creation_input_tokens":64,"cache_read_input_tokens":1000,"output_tokens":8,"service_tier":"standard"}},"requestId":"req_C","type":"assistant","uuid":"c2","timestamp":"2026-10-14T09:01:00.400Z"}
```

`crates/tagteam-cc/tests/fixtures/transcripts/projects/-w-app/11111111-1111-4111-8111-111111111111/subagents/agent-0123456789abcdef.meta.json`:

```text
{"agentType":"general-purpose"}
```

`crates/tagteam-cc/tests/fixtures/transcripts/projects/-w-app/22222222-2222-4222-8222-222222222222.jsonl`:

```text
{"parentUuid":"a2","isSidechain":false,"userType":"external","cwd":"/w/app","sessionId":"11111111-1111-4111-8111-111111111111","version":"2.1.299","gitBranch":"main","message":{"id":"msg_A","type":"message","role":"assistant","model":"claude-opus-5-5","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"x"}}],"stop_reason":"tool_use","stop_sequence":null,"usage":{"input_tokens":10,"cache_creation_input_tokens":300,"cache_read_input_tokens":2000,"cache_creation":{"ephemeral_5m_input_tokens":200,"ephemeral_1h_input_tokens":100},"output_tokens":120,"service_tier":"standard","server_tool_use":{"web_search_requests":1,"web_fetch_requests":0},"speed":"standard","output_tokens_details":{"thinking_tokens":60},"iterations":[{"input_tokens":10,"output_tokens":120,"cache_read_input_tokens":2000,"cache_creation_input_tokens":300,"cache_creation":{"ephemeral_5m_input_tokens":200,"ephemeral_1h_input_tokens":100},"type":"message"}]}},"requestId":"req_A","type":"assistant","uuid":"a3","timestamp":"2026-10-14T09:00:02.500Z"}
{"parentUuid":"a3","isSidechain":false,"userType":"external","cwd":"/w/app","sessionId":"22222222-2222-4222-8222-222222222222","version":"2.1.299","gitBranch":"main","type":"user","message":{"role":"user","content":"x"},"uuid":"u4","timestamp":"2026-10-14T09:59:59.000Z"}
{"parentUuid":"u4","isSidechain":false,"userType":"external","cwd":"/w/app","sessionId":"22222222-2222-4222-8222-222222222222","version":"2.1.299","gitBranch":"main","message":{"id":"msg_D","type":"message","role":"assistant","model":"claude-opus-5-5","content":[{"type":"text","text":"x"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":20,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0},"output_tokens":30,"service_tier":"standard","server_tool_use":{"web_search_requests":0,"web_fetch_requests":0},"speed":"standard"}},"requestId":"req_D","type":"assistant","uuid":"d1","timestamp":"2026-10-14T10:00:00.000Z"}
```

`crates/tagteam-cc/tests/fixtures/transcripts/projects/-w-lib/33333333-3333-4333-8333-333333333333.jsonl`:

```text
{"type":"assistant","parentUuid":null,"isSidechain":false,"cwd":"/w/lib","sessionId":"3333
{"parentUuid":null,"isSidechain":false,"userType":"external","cwd":"/w/lib","sessionId":"33333333-3333-4333-8333-333333333333","version":"2.1.299","gitBranch":"main","message":{"id":"msg_F","type":"message","role":"assistant","model":"claude-opus-5-5","content":[{"type":"text","text":"x"}],"stop_reason":"end_turn","stop_sequence":null},"requestId":"req_F","type":"assistant","uuid":"f1","timestamp":"2026-10-14T11:00:01.000Z"}
```

`crates/tagteam-cc/tests/transcripts.rs`:

```rust
//! Claude Code's transcripts (token report spec §2, §3.2): discovery and the usage line parser,
//! over synthetic lines with no real content.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use proptest::prelude::*;
use serde_json::{Value, json};
use tagteam_cc::transcripts::{discover, parse_usage_line};
use tagteam_core::tokens::TokenCounts;
use tagteam_provider::{LineError, UsageLine};

/// 2026-10-14T09:00:02.123Z.
const AT_MS: i64 = 1_791_968_402_123;

/// A final assistant line as Claude Code 2.1.299 writes one (spec §2.1), every count distinct.
fn final_line() -> Value {
    json!({
        "parentUuid": "u1", "isSidechain": false, "userType": "external", "cwd": "/w/app",
        "sessionId": "s1", "version": "2.1.299", "gitBranch": "main",
        "message": {
            "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
            "content": [{"type": "text", "text": "x"}],
            "stop_reason": "end_turn", "stop_sequence": null,
            "usage": {
                "input_tokens": 10, "cache_creation_input_tokens": 300,
                "cache_read_input_tokens": 2000,
                "cache_creation": {"ephemeral_5m_input_tokens": 200, "ephemeral_1h_input_tokens": 100},
                "output_tokens": 120, "service_tier": "standard",
                "server_tool_use": {"web_search_requests": 2, "web_fetch_requests": 0},
                "speed": "standard",
                "output_tokens_details": {"thinking_tokens": 30},
                "iterations": [{
                    "input_tokens": 10, "output_tokens": 120, "cache_read_input_tokens": 2000,
                    "cache_creation_input_tokens": 300,
                    "cache_creation": {"ephemeral_5m_input_tokens": 200, "ephemeral_1h_input_tokens": 100},
                    "type": "message"
                }]
            }
        },
        "requestId": "req_1", "type": "assistant", "uuid": "a1",
        "timestamp": "2026-10-14T09:00:02.123Z"
    })
}

fn parse(v: &Value) -> Result<Option<UsageLine>, LineError> {
    parse_usage_line(&serde_json::to_vec(v).unwrap())
}

fn parsed(v: &Value) -> UsageLine {
    parse(v).unwrap().expect("a usage line")
}

fn counts(input: u64, output: u64, m5: u64, h1: u64, read: u64, searches: u64) -> TokenCounts {
    TokenCounts {
        input,
        output,
        cache_write_5m: m5,
        cache_write_1h: h1,
        cache_read: read,
        web_search: searches,
    }
}

/// Removes the field at `path` (object keys, `/`-separated).
fn without(mut v: Value, path: &str) -> Value {
    let (parent, key) = path.rsplit_once('/').unwrap_or(("", path));
    let mut at = &mut v;
    for k in parent.split('/').filter(|k| !k.is_empty()) {
        at = &mut at[k];
    }
    at.as_object_mut().unwrap().remove(key);
    v
}

/// Sets the field at `path` to `to`.
fn with(mut v: Value, path: &str, to: Value) -> Value {
    let mut at = &mut v;
    for k in path.split('/') {
        at = &mut at[k];
    }
    *at = to;
    v
}

#[test]
fn parses_a_final_assistant_line() {
    assert_eq!(
        parsed(&final_line()),
        UsageLine {
            message_id: "msg_1".into(),
            request_id: "req_1".into(),
            session_id: "s1".into(),
            at_ms: AT_MS,
            model: "claude-opus-5-5".into(),
            project: Some("/w/app".into()),
            sidechain: false,
            counts: counts(10, 120, 200, 100, 2000, 2),
            speed: Some("standard".into()),
            final_line: true,
        }
    );
}

#[test]
fn a_stale_snapshot_is_not_final() {
    // Spec §2.2: a line with a null stop_reason holds a stale output count, and no iterations,
    // speed or server tool use.
    let mut stale = with(final_line(), "message/stop_reason", Value::Null);
    stale = with(stale, "message/usage/output_tokens", json!(5));
    for gone in [
        "iterations",
        "speed",
        "server_tool_use",
        "output_tokens_details",
    ] {
        stale = without(stale, &format!("message/usage/{gone}"));
    }
    stale = with(stale, "isSidechain", json!(true));
    let line = parsed(&stale);
    assert!(!line.final_line);
    assert!(line.sidechain);
    assert_eq!(line.speed, None);
    assert_eq!(line.counts, counts(10, 5, 200, 100, 2000, 0));
    let absent = without(final_line(), "message/stop_reason");
    assert!(!parsed(&absent).final_line, "an absent stop_reason is null");
}

#[test]
fn zero_top_level_falls_back_to_the_first_iteration() {
    let mut zero = final_line();
    for field in [
        "input_tokens",
        "output_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
        "cache_creation/ephemeral_5m_input_tokens",
        "cache_creation/ephemeral_1h_input_tokens",
    ] {
        zero = with(zero, &format!("message/usage/{field}"), json!(0));
    }
    zero = with(
        zero,
        "message/usage/iterations",
        json!([{"input_tokens": 3, "output_tokens": 50, "cache_read_input_tokens": 500,
                "cache_creation_input_tokens": 40,
                "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 40}},
               {"input_tokens": 999, "output_tokens": 999}]),
    );
    assert_eq!(
        parsed(&zero).counts,
        counts(3, 50, 0, 40, 500, 2),
        "the first iteration's counts; web searches stay the top level's"
    );
    let differs = with(
        final_line(),
        "message/usage/iterations",
        json!([{"input_tokens": 3, "output_tokens": 50}]),
    );
    assert_eq!(
        parsed(&differs).counts,
        counts(10, 120, 200, 100, 2000, 2),
        "a non-zero top level wins"
    );
    let empty = with(zero, "message/usage/iterations", json!([]));
    assert_eq!(parsed(&empty).counts, counts(0, 0, 0, 0, 0, 2));
}

#[test]
fn a_huge_iterations_array_keeps_only_the_first() {
    // Spec §2.2: only iterations[0] counts; the rest is skipped as it is read.
    let mut zero = final_line();
    for field in [
        "input_tokens",
        "output_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
        "cache_creation/ephemeral_5m_input_tokens",
        "cache_creation/ephemeral_1h_input_tokens",
    ] {
        zero = with(zero, &format!("message/usage/{field}"), json!(0));
    }
    let mut many = vec![json!({"input_tokens": 7, "output_tokens": 8})];
    many.extend(std::iter::repeat_n(json!({}), 200_000));
    let line = with(zero, "message/usage/iterations", Value::Array(many));
    assert_eq!(parsed(&line).counts, counts(7, 8, 0, 0, 0, 2));
    let null = with(final_line(), "message/usage/iterations", Value::Null);
    assert_eq!(parsed(&null).counts, counts(10, 120, 200, 100, 2000, 2));
}

#[test]
fn cache_writes_without_a_split_count_as_five_minute_writes() {
    let unsplit = without(final_line(), "message/usage/cache_creation");
    assert_eq!(parsed(&unsplit).counts, counts(10, 120, 300, 0, 2000, 2));
    let short = with(
        final_line(),
        "message/usage/cache_creation",
        json!({"ephemeral_1h_input_tokens": 100}),
    );
    assert_eq!(
        parsed(&short).counts,
        counts(10, 120, 200, 100, 2000, 2),
        "what the split leaves out is 5-minute"
    );
}

#[test]
fn a_missing_request_id_keys_on_an_empty_one() {
    // Ruling R5.
    let line = parsed(&without(final_line(), "requestId"));
    assert_eq!(
        (line.message_id.as_str(), line.request_id.as_str()),
        ("msg_1", "")
    );
}

#[test]
fn synthetic_lines_are_skipped() {
    let mut synthetic = with(final_line(), "message/model", json!("<synthetic>"));
    synthetic = without(synthetic, "requestId");
    synthetic = with(
        synthetic,
        "message/usage",
        json!({"input_tokens": 0, "output_tokens": 0}),
    );
    assert_eq!(parse(&synthetic), Ok(None));
    let bare = without(synthetic, "message/usage");
    assert_eq!(parse(&bare), Ok(None), "skipped before its usage is read");
}

#[test]
fn other_line_types_are_none() {
    let user = json!({"parentUuid": null, "isSidechain": false, "cwd": "/w/app",
                      "sessionId": "s1", "type": "user",
                      "message": {"role": "user", "content": "x"},
                      "uuid": "u1", "timestamp": "2026-10-14T09:00:00.000Z"});
    let nested = json!({"type": "user", "toolUseResult": {"type": "assistant"},
                        "message": {"role": "user", "content": "x"}});
    let untyped = json!({"toolUseResult": {"type": "assistant"}});
    for line in [
        user,
        nested,
        untyped,
        json!({"type": "system", "subtype": "informational", "content": "x"}),
        json!({"type": "summary", "summary": "x", "leafUuid": "u1"}),
        with(final_line(), "type", json!("progress")),
    ] {
        assert_eq!(parse(&line), Ok(None), "{line}");
    }
    for bytes in [
        &b""[..],
        b"\n",
        b"not json at all",
        b"\xff\xfe\x00",
        br#"{"type":"user"#,
    ] {
        assert_eq!(parse_usage_line(bytes), Ok(None));
    }
}

#[test]
fn missing_usage_is_an_error() {
    for gone in [
        "message/usage",
        "message/id",
        "message/model",
        "message",
        "sessionId",
        "timestamp",
        "message/usage/input_tokens",
        "message/usage/output_tokens",
    ] {
        assert!(
            parse(&without(final_line(), gone)).is_err(),
            "without {gone}"
        );
    }
    for timestamp in ["2026-10-14", "yesterday", "2026-10-14T09:00:02.123"] {
        let line = with(final_line(), "timestamp", json!(timestamp));
        assert!(parse(&line).is_err(), "{timestamp}");
    }
}

#[test]
fn the_space_after_the_colon_is_tolerated() {
    let spaced = serde_json::to_string(&final_line())
        .unwrap()
        .replace("\":", "\": ");
    assert!(spaced.contains(r#""type": "assistant""#));
    assert_eq!(
        parse_usage_line(spaced.as_bytes()).unwrap().unwrap().counts,
        counts(10, 120, 200, 100, 2000, 2)
    );
}

#[test]
fn errors_never_quote_the_line() {
    let compact = serde_json::to_string(&final_line()).unwrap();
    let lines = [
        with(
            final_line(),
            "message/usage/input_tokens",
            json!("SENTINEL"),
        ),
        with(final_line(), "message/usage/speed", json!(["SENTINEL"])),
        with(final_line(), "timestamp", json!("SENTINEL")),
        with(final_line(), "isSidechain", json!("SENTINEL")),
    ]
    .into_iter()
    .map(|v| serde_json::to_string(&v).unwrap())
    .chain([format!("{}SENTINEL", &compact[..compact.len() - 1])]);
    for line in lines {
        let e = parse_usage_line(line.as_bytes()).expect_err(&line);
        assert!(!e.to_string().contains("SENTINEL"), "{e}");
    }
}

/// The count fields a hostile value may land in, as the compact line spells them first (the
/// top level comes before `iterations`).
const COUNT_FIELDS: [&str; 7] = [
    r#""input_tokens":10"#,
    r#""output_tokens":120"#,
    r#""cache_creation_input_tokens":300"#,
    r#""cache_read_input_tokens":2000"#,
    r#""ephemeral_5m_input_tokens":200"#,
    r#""ephemeral_1h_input_tokens":100"#,
    r#""web_search_requests":2"#,
];

const HOSTILE: [&str; 8] = [
    "-1",
    "1.5",
    "1e3",
    r#""7""#,
    "null",
    "true",
    "[]",
    "18446744073709551616",
];

proptest! {
    #[test]
    fn hostile_lines_are_errors_never_panics(
        bytes in proptest::collection::vec(any::<u8>(), 0..512),
        field in 0..COUNT_FIELDS.len(),
        value in 0..HOSTILE.len(),
        cut in 0usize..2_000,
    ) {
        // Review Focus 2: arbitrary bytes, alone and behind an assistant prefix, never panic.
        let _ = parse_usage_line(&bytes);
        let mut prefixed = br#"{"type":"assistant","message":"#.to_vec();
        prefixed.extend_from_slice(&bytes);
        let _ = parse_usage_line(&prefixed);
        // A count that is negative, fractional, text, null or above u64 is an error.
        let good = serde_json::to_string(&final_line()).unwrap();
        let (key, _) = COUNT_FIELDS[field].split_once(':').unwrap();
        let bad = good.replacen(COUNT_FIELDS[field], &format!("{key}:{}", HOSTILE[value]), 1);
        prop_assert_ne!(&bad, &good);
        prop_assert!(parse_usage_line(bad.as_bytes()).is_err(), "{}", bad);
        // A line cut short is never a usage line.
        let cut = cut % good.len();
        prop_assert!(!matches!(parse_usage_line(&good.as_bytes()[..cut]), Ok(Some(_))));
    }
}

/// Builds `rel` under `root` as a file holding `{}`.
fn touch(root: &Path, rel: &str) -> PathBuf {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, "{}\n").unwrap();
    p
}

#[test]
fn discovers_sessions_and_subagents_only() {
    // Spec §2.1: two fixed levels; nothing deeper and no other extension.
    let d = tempfile::tempdir().unwrap();
    let projects = d.path().join("projects");
    let mut expected = vec![
        touch(&projects, "-w-a/s1.jsonl"),
        touch(&projects, "-w-a/s2.jsonl"),
        touch(&projects, "-w-a/s1/subagents/agent-0123456789abcdef.jsonl"),
        touch(&projects, "-w-b/s3.jsonl"),
    ];
    touch(&projects, "-w-a/notes.txt");
    touch(
        &projects,
        "-w-a/s1/subagents/agent-0123456789abcdef.meta.json",
    );
    touch(&projects, "-w-a/s1/subagents/other.jsonl");
    touch(&projects, "-w-a/s1/subagents/deeper/agent-1.jsonl");
    touch(&projects, "-w-a/s1/tool-results/agent-2.jsonl");
    touch(&projects, "-w-a/s1/s1.jsonl");
    touch(&projects, "top.jsonl");
    fs::create_dir_all(projects.join("-w-b/dir.jsonl")).unwrap();
    // A link to a file is followed; a dangling one and a link to a directory are not.
    let elsewhere = touch(d.path(), "elsewhere/s4.jsonl");
    symlink(&elsewhere, projects.join("-w-b/s4.jsonl")).unwrap();
    expected.push(projects.join("-w-b/s4.jsonl"));
    symlink(
        d.path().join("nowhere.jsonl"),
        projects.join("-w-b/gone.jsonl"),
    )
    .unwrap();
    touch(d.path(), "linked-project/s5.jsonl");
    symlink(d.path().join("linked-project"), projects.join("-w-c")).unwrap();
    touch(d.path(), "linked-agents/agent-fedcba9876543210.jsonl");
    fs::create_dir_all(projects.join("-w-b/s3")).unwrap();
    symlink(d.path().join("linked-agents"), projects.join("-w-b/s3/subagents")).unwrap();
    expected.sort();
    assert_eq!(discover(&projects).unwrap(), expected);
}

#[test]
fn a_missing_projects_dir_is_empty() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        discover(&d.path().join("projects")).unwrap(),
        Vec::<PathBuf>::new()
    );
    let file = touch(d.path(), "projects-file");
    assert_eq!(
        discover(&file).unwrap(),
        Vec::<PathBuf>::new(),
        "not a directory"
    );
    fs::create_dir(d.path().join("empty")).unwrap();
    assert_eq!(
        discover(&d.path().join("empty")).unwrap(),
        Vec::<PathBuf>::new()
    );
}

/// The fixture tree under `tests/fixtures/transcripts/projects`.
fn fixture_projects() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/transcripts/projects")
}

/// One response as the merge rule of spec §2.2 sees it.
#[derive(Debug, PartialEq)]
struct Merged {
    session_id: String,
    model: String,
    sidechain: bool,
    first_ms: i64,
    counts: TokenCounts,
    finalized: bool,
    lines: usize,
}

#[test]
fn the_fixture_tree_parses_to_known_responses() {
    // The numbers the engine's refresh tests can rely on: four responses over eight usage
    // lines in four files, one response copied into a resumed session, two unreadable lines.
    let files = discover(&fixture_projects()).unwrap();
    let names: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(fixture_projects())
                .unwrap()
                .display()
                .to_string()
        })
        .collect();
    assert_eq!(
        names,
        [
            "-w-app/11111111-1111-4111-8111-111111111111/subagents/agent-0123456789abcdef.jsonl",
            "-w-app/11111111-1111-4111-8111-111111111111.jsonl",
            "-w-app/22222222-2222-4222-8222-222222222222.jsonl",
            "-w-lib/33333333-3333-4333-8333-333333333333.jsonl",
        ]
    );
    let (mut usage_lines, mut errors) = (0, 0);
    let mut merged: BTreeMap<(String, String), Merged> = BTreeMap::new();
    for file in &files {
        for line in fs::read(file).unwrap().split(|b| *b == b'\n') {
            let u = match parse_usage_line(line) {
                Ok(Some(u)) => u,
                Ok(None) => continue,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            };
            usage_lines += 1;
            let m = merged
                .entry((u.message_id.clone(), u.request_id.clone()))
                .or_insert_with(|| Merged {
                    session_id: u.session_id.clone(),
                    model: u.model.clone(),
                    sidechain: u.sidechain,
                    first_ms: u.at_ms,
                    counts: TokenCounts::default(),
                    finalized: false,
                    lines: 0,
                });
            m.first_ms = m.first_ms.min(u.at_ms);
            m.counts.merge_max(&u.counts);
            m.finalized |= u.final_line;
            m.lines += 1;
        }
    }
    assert_eq!((usage_lines, errors), (8, 2));
    let s1 = "11111111-1111-4111-8111-111111111111";
    let s2 = "22222222-2222-4222-8222-222222222222";
    let r = |id: &str, req: &str| merged.get(&(id.to_owned(), req.to_owned())).unwrap();
    assert_eq!(merged.len(), 4);
    assert_eq!(
        r("msg_A", "req_A"),
        &Merged {
            session_id: s1.into(),
            model: "claude-opus-5-5".into(),
            sidechain: false,
            first_ms: 1_791_968_401_100,
            counts: counts(10, 120, 200, 100, 2000, 1),
            finalized: true,
            lines: 4,
        }
    );
    assert_eq!(
        r("msg_B", "req_B"),
        &Merged {
            session_id: s1.into(),
            model: "claude-sonnet-5-5".into(),
            sidechain: false,
            first_ms: 1_791_968_700_000,
            counts: counts(3, 50, 40, 0, 500, 0),
            finalized: true,
            lines: 1,
        }
    );
    assert_eq!(
        r("msg_C", "req_C"),
        &Merged {
            session_id: s1.into(),
            model: "claude-haiku-4-5-20251001".into(),
            sidechain: true,
            first_ms: 1_791_968_460_000,
            counts: counts(7, 8, 64, 0, 1000, 0),
            finalized: false,
            lines: 2,
        }
    );
    assert_eq!(
        r("msg_D", "req_D"),
        &Merged {
            session_id: s2.into(),
            model: "claude-opus-5-5".into(),
            sidechain: false,
            first_ms: 1_791_972_000_000,
            counts: counts(20, 30, 0, 0, 0, 0),
            finalized: true,
            lines: 1,
        }
    );
}
```

`crates/tagteam-cc/tests/provider.rs`:

```diff
--- a/crates/tagteam-cc/tests/provider.rs
+++ b/crates/tagteam-cc/tests/provider.rs
@@ fn claude_code_has_every_capability_and_the_kind_table() {
             api_keys: true,
             sessions: true,
             statusline: true,
-            transcripts: false,
+            transcripts: true,
         }
     );
     for kind in f.cc.credential_kinds() {
@@ fn claude_code_has_every_capability_and_the_kind_table() {
     assert!(f.cc.kind_traits("oauth").refreshable);
 }
 
+#[test]
+fn claude_code_reads_transcripts_under_its_config_home() {
+    // Token report spec §2.1, §3.2 and ruling R2: the default home's `projects/` and
+    // `sessions/`, the parser of `tagteam_cc::transcripts` and the shipped price table.
+    let f = fx();
+    let home = f.env.home.join(".claude");
+    assert_eq!(
+        f.cc.default_session_records_dir(&f.env),
+        Some(home.join("sessions"))
+    );
+    assert!(std::ptr::eq(
+        f.cc.price_table(),
+        &tagteam_cc::prices::PRICES
+    ));
+    assert_eq!(
+        f.cc.transcript_files(&f.env).unwrap(),
+        Vec::<PathBuf>::new()
+    );
+    let session = home.join("projects/-w/s.jsonl");
+    fs::create_dir_all(session.parent().unwrap()).unwrap();
+    fs::write(&session, b"").unwrap();
+    assert_eq!(f.cc.transcript_files(&f.env).unwrap(), vec![session]);
+    assert_eq!(f.cc.parse_usage_line(br#"{"type":"user"}"#), Ok(None));
+    assert!(f.cc.parse_usage_line(br#"{"type":"assistant"}"#).is_err());
+}
+
 #[test]
 fn access_token_facts_come_from_the_access_token_not_the_lineage() {
     let f = fx();
```

`crates/tagteam-core/src/time.rs`, in `mod tests`:

```diff
--- a/crates/tagteam-core/src/time.rs
+++ b/crates/tagteam-core/src/time.rs
@@ mod tests {
             Some(1_790_000_000)
         );
     }
+
+    #[test]
+    fn milliseconds_are_kept_when_asked_for() {
+        // Token report spec §2.1: transcript lines carry milliseconds and `Z`.
+        for (text, ms) in [
+            ("2026-09-21T14:13:20.123Z", 1_790_000_000_123),
+            ("2026-09-21T14:13:20.5Z", 1_790_000_000_500),
+            ("2026-09-21T14:13:20.05Z", 1_790_000_000_050),
+            ("2026-09-21T14:13:20.123987Z", 1_790_000_000_123),
+            ("2026-09-21T14:13:20Z", 1_790_000_000_000),
+            ("2026-09-21T16:13:20.250+02:00", 1_790_000_000_250),
+            ("1969-12-31T23:59:59.900Z", -100),
+        ] {
+            assert_eq!(parse_iso8601_ms(text), Some(ms), "{text}");
+        }
+        assert_eq!(
+            parse_iso8601("2026-09-21T14:13:20.999Z"),
+            Some(1_790_000_000),
+            "seconds still drop the fraction, never round it"
+        );
+    }
+
+    #[test]
+    fn milliseconds_refuse_what_seconds_refuse() {
+        for text in [
+            "",
+            "2026-09-21T14:13:20",
+            "2026-09-21T14:13:20.Z",
+            "2026-09-21T14:13:20.12x3Z",
+            "2026-02-30T00:00:00.000Z",
+            "2026-09-21 14:13:20.000Z",
+            "2026-09-21T24:00:00.000Z",
+        ] {
+            assert_eq!(parse_iso8601_ms(text), None, "{text:?}");
+            assert_eq!(parse_iso8601(text), None, "{text:?}");
+        }
+    }
 }
```

The memory bound needs its own test binary, because it installs a counting global allocator. Create `crates/tagteam-cc/tests/parser_memory.rs`:

```rust
//! Spec §5.2: a worker's memory stays bounded. Parsing one line holds only a small, fixed amount
//! beyond the line itself, however long its `usage.iterations` array is (spec §2.2: only the
//! first entry counts). Its own test binary, so the counting allocator sees nothing else.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use tagteam_cc::transcripts::parse_usage_line;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is forwarded unchanged to the system allocator; this only counts bytes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract for `layout` is passed through to `System`.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(live, Ordering::SeqCst);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `alloc` above with this `layout`.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

#[test]
fn a_huge_iterations_array_is_skipped_not_collected() {
    let mut line = String::from(concat!(
        r#"{"type":"assistant","requestId":"r","sessionId":"s","#,
        r#""timestamp":"2026-10-14T09:00:00.000Z","isSidechain":false,"#,
        r#""message":{"id":"m","model":"claude-opus-5-5","stop_reason":"end_turn","#,
        r#""usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"#,
        r#""cache_read_input_tokens":0,"iterations":[{"input_tokens":7,"output_tokens":8}"#,
    ));
    for _ in 0..200_000 {
        line.push_str(concat!(
            r#",{"input_tokens":1,"output_tokens":1,"#,
            r#""cache_creation_input_tokens":1,"cache_read_input_tokens":1}"#,
        ));
    }
    line.push_str("]}}}");
    let bytes = line.into_bytes();

    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let parsed = parse_usage_line(&bytes).unwrap().unwrap();
    let held = PEAK.load(Ordering::SeqCst) - before;

    assert_eq!((parsed.counts.input, parsed.counts.output), (7, 8));
    // Collecting the 200,001 entries would hold about 10 MB; skipping them holds a few KB.
    assert!(
        held < 64 * 1024,
        "parsing held {held} bytes beyond the line"
    );
}
```

The prefilter's own unit tests sit at the end of `crates/tagteam-cc/src/transcripts.rs` (Step 3's file, `mod tests`), since the prefilter is private.

- [ ] **Step 2: Run them and see them fail**

```
cargo test -p tagteam-core --lib --locked time::
cargo test -p tagteam-cc --locked --test transcripts --test provider --test parser_memory
```

Expected: `tagteam-core` does not compile its tests: `error[E0425]: cannot find function `parse_iso8601_ms` in this scope`. `tagteam-cc` does not compile them: `error[E0432]: unresolved import `tagteam_cc::transcripts`` and `error[E0433]: failed to resolve: could not find `prices` in `tagteam_cc``.

- [ ] **Step 3: Implement**

`crates/tagteam-core/src/time.rs`: one private parser returns the seconds and the fraction's milliseconds; `parse_iso8601` keeps its exact behaviour.

```diff
--- a/crates/tagteam-core/src/time.rs
+++ b/crates/tagteam-core/src/time.rs
@@ fn civil_from_days(days: i64) -> (i64, i64, i64) {
 /// Epoch seconds from `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)` (Decision 5). The fraction
 /// is dropped. Anything else, a missing offset or an impossible date included, is `None`.
 pub fn parse_iso8601(s: &str) -> Option<i64> {
+    parse(s).map(|(secs, _)| secs)
+}
+
+/// Epoch milliseconds from the forms `parse_iso8601` reads, keeping the fraction's first three
+/// digits, truncated: Claude Code's transcripts time their lines to the millisecond (token
+/// report spec §2.1).
+pub fn parse_iso8601_ms(s: &str) -> Option<i64> {
+    parse(s).map(|(secs, ms)| secs * 1000 + ms)
+}
+
+/// `parse_iso8601`'s epoch seconds, and the milliseconds of the fraction.
+fn parse(s: &str) -> Option<(i64, i64)> {
     let b = s.as_bytes();
     if b.len() < 20
         || b[4] != b'-'
@@ pub fn parse_iso8601(s: &str) -> Option<i64> {
         digits(&b[17..19])?,
     );
     let mut rest = &b[19..];
+    let mut ms = 0;
     if let Some(fraction) = rest.strip_prefix(b".") {
         let n = fraction.iter().take_while(|c| c.is_ascii_digit()).count();
         if n == 0 {
             return None;
         }
+        ms = (0..3).fold(0, |acc, i| {
+            acc * 10 + fraction[..n].get(i).map_or(0, |d| i64::from(d - b'0'))
+        });
         rest = &fraction[n..];
     }
     let offset = match rest {
@@ pub fn parse_iso8601(s: &str) -> Option<i64> {
     {
         return None;
     }
-    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
+    let secs = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
+    Some((secs - offset, ms))
 }
 
 /// `YYYY-MM-DDTHH:MM:SSZ` for epoch seconds: how rendered JSON spells a time (§13.2).
```

Create `crates/tagteam-cc/src/transcripts.rs`. The prefilter is a Horspool scan for `"assistant"` that then checks the key in front of it, so a line of another type costs about a read of every eleventh byte; serde then deserializes only the fields spec §3.2 names into private structs, borrowing `type` and `timestamp` from the line (every other kept string is owned by `UsageLine` anyway, so it is deserialized straight into a `String`). `serde_json`'s `arbitrary_precision` feature is on in this workspace, so the structs use no `flatten` and no untagged enums. Error texts never quote the line: serde's own message can quote a value, so `unreadable` keeps only its category and column.

```rust
//! Claude Code's transcripts (token report spec §2): the files under the default home's
//! `projects/`, and the usage their assistant lines carry.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::error::Category;
use tagteam_core::time::parse_iso8601_ms;
use tagteam_core::tokens::TokenCounts;
use tagteam_provider::{LineError, UsageLine};

/// `message.model` of a line that is no API call (spec §2.2).
const SYNTHETIC: &str = "<synthetic>";

/// Every transcript under `projects` (spec §2.1, §5.2 step 2): `<project>/<session>.jsonl` and
/// `<project>/<session>/subagents/agent-<id>.jsonl`, sorted. Exactly those two levels are
/// walked; a link to a file is followed, a link to a directory is not. A missing `projects` is
/// empty, and a directory below it that cannot be listed is passed over.
pub fn discover(projects: &Path) -> io::Result<Vec<PathBuf>> {
    let listing = match fs::read_dir(projects) {
        Ok(l) => l,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Vec::new());
        }
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for project in listing {
        let project = project?;
        if !is_dir(&project) {
            continue;
        }
        for entry in entries(&project.path()) {
            let path = entry.path();
            if is_dir(&entry) {
                let subagents = path.join("subagents");
                // A link in place of `subagents/` is not followed either.
                if !fs::symlink_metadata(&subagents).is_ok_and(|m| m.is_dir()) {
                    continue;
                }
                for agent in entries(&subagents) {
                    let name = agent.file_name();
                    let name = name.as_encoded_bytes();
                    if name.starts_with(b"agent-") && name.ends_with(b".jsonl") && is_file(&agent) {
                        out.push(agent.path());
                    }
                }
            } else if path.extension().is_some_and(|x| x == "jsonl") && is_file(&entry) {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The entries of `dir`; none when it is missing or cannot be listed.
fn entries(dir: &Path) -> Vec<fs::DirEntry> {
    match fs::read_dir(dir) {
        Ok(listing) => listing.filter_map(Result::ok).collect(),
        Err(e) => {
            if e.kind() != io::ErrorKind::NotFound {
                tracing::debug!(dir = %dir.display(), error = %e, "transcript directory not listed");
            }
            Vec::new()
        }
    }
}

/// A real directory: a link to one is not followed.
fn is_dir(entry: &fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|t| t.is_dir())
}

/// A regular file, or a link that resolves to one.
fn is_file(entry: &fs::DirEntry) -> bool {
    match entry.file_type() {
        Ok(t) if t.is_symlink() => fs::metadata(entry.path()).is_ok_and(|m| m.is_file()),
        Ok(t) => t.is_file(),
        Err(_) => false,
    }
}

/// The value the prefilter looks for, then checks the key in front of it.
const ASSISTANT: &[u8] = b"\"assistant\"";
/// Horspool's shifts for `ASSISTANT`: how far a window may move when it ends on a byte.
const SHIFT: [u8; 256] = shift_table(ASSISTANT);

const fn shift_table(needle: &[u8]) -> [u8; 256] {
    let last = needle.len() - 1;
    let mut table = [needle.len() as u8; 256];
    let mut i = 0;
    while i < last {
        table[needle[i] as usize] = (last - i) as u8;
        i += 1;
    }
    table
}

/// The first `ASSISTANT` in `line` at or after `at`: a line without one costs about
/// `len / 11` byte reads, not `len`.
fn find_assistant(line: &[u8], mut at: usize) -> Option<usize> {
    let last = ASSISTANT.len() - 1;
    while let Some(&end) = line.get(at + last) {
        if end == b'"' && line[at..].starts_with(ASSISTANT) {
            return Some(at);
        }
        at += usize::from(SHIFT[usize::from(end)]);
    }
    None
}

/// Spec §3.2's prefilter: whether `line` holds `"type":"assistant"`, or `"type": "assistant"`.
/// A line without it is never parsed.
pub(crate) fn may_be_assistant(line: &[u8]) -> bool {
    let mut from = 0;
    while let Some(at) = find_assistant(line, from) {
        let key = &line[..at];
        if key
            .strip_suffix(b" ")
            .unwrap_or(key)
            .ends_with(b"\"type\":")
        {
            return true;
        }
        from = at + 1;
    }
    false
}

/// The fields of a line that spec §3.2 reads; serde skips every other.
#[derive(Deserialize)]
struct Line<'a> {
    #[serde(rename = "type")]
    kind: Option<&'a str>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    timestamp: Option<&'a str>,
    cwd: Option<String>,
    #[serde(rename = "isSidechain", default)]
    sidechain: bool,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    /// Only whether it is null matters (spec §2.2).
    stop_reason: Option<IgnoredAny>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    cache_creation: Option<CacheSplit>,
    server_tool_use: Option<ServerToolUse>,
    speed: Option<String>,
    #[serde(default, deserialize_with = "first_iteration")]
    iterations: Option<Iteration>,
}

/// `usage.iterations[0]`, the rest skipped as it is read and never collected, so a huge array
/// costs no memory (spec §2.2: it is never longer than one; spec §5.2: workers stay bounded).
fn first_iteration<'de, D>(d: D) -> Result<Option<Iteration>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct First;
    impl<'de> serde::de::Visitor<'de> for First {
        type Value = Option<Iteration>;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("an array of iterations, or null")
        }

        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let first = seq.next_element::<Iteration>()?;
            while seq.next_element::<IgnoredAny>()?.is_some() {}
            Ok(first)
        }
    }
    d.deserialize_any(First)
}

/// An entry of `usage.iterations` (spec §2.2).
#[derive(Deserialize)]
struct Iteration {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    cache_creation: Option<CacheSplit>,
}

#[derive(Deserialize)]
struct CacheSplit {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

#[derive(Deserialize)]
struct ServerToolUse {
    #[serde(default)]
    web_search_requests: u64,
}

/// The counts of a usage object, web searches aside. The 5-minute and 1-hour writes sum to
/// `written` (spec §2.6); what a line leaves unsplit, as an older line without the split does,
/// counts as 5-minute writes.
fn token_counts(
    input: u64,
    output: u64,
    written: u64,
    read: u64,
    split: Option<&CacheSplit>,
) -> TokenCounts {
    let (m5, h1) = split.map_or((0, 0), |s| {
        (s.ephemeral_5m_input_tokens, s.ephemeral_1h_input_tokens)
    });
    TokenCounts {
        input,
        output,
        cache_write_5m: m5.saturating_add(written.saturating_sub(m5.saturating_add(h1))),
        cache_write_1h: h1,
        cache_read: read,
        web_search: 0,
    }
}

/// A line serde could not read, described without quoting it: serde's own message may.
fn unreadable(e: serde_json::Error) -> LineError {
    let what = match e.classify() {
        Category::Data => "has a field of the wrong type",
        Category::Syntax | Category::Eof | Category::Io => "is not JSON",
    };
    LineError(format!("a transcript line {what} (column {})", e.column()))
}

fn missing(what: &str) -> LineError {
    LineError(format!("an assistant line has no {what}"))
}

/// Spec §3.2: `None` for a line of another type, a `<synthetic>` line, or one the prefilter
/// rules out; `Err` for an assistant line that cannot be read or lacks a field it needs (§5.4).
/// Counts follow spec §2.2: the top level, or `iterations[0]` when the top level is all zero.
pub fn parse_usage_line(line: &[u8]) -> Result<Option<UsageLine>, LineError> {
    if !may_be_assistant(line) {
        return Ok(None);
    }
    let line: Line<'_> = serde_json::from_slice(line).map_err(unreadable)?;
    if line.kind != Some("assistant") {
        return Ok(None);
    }
    let message = line.message.ok_or_else(|| missing("message"))?;
    let model = message.model.ok_or_else(|| missing("model"))?;
    if model == SYNTHETIC {
        return Ok(None);
    }
    let usage = message.usage.ok_or_else(|| missing("usage"))?;
    let message_id = message.id.ok_or_else(|| missing("message id"))?;
    let session_id = line.session_id.ok_or_else(|| missing("session id"))?;
    let at_ms = line
        .timestamp
        .and_then(parse_iso8601_ms)
        .ok_or_else(|| missing("readable timestamp"))?;
    let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens) else {
        return Err(missing("input or output count"));
    };
    let top = token_counts(
        input,
        output,
        usage.cache_creation_input_tokens,
        usage.cache_read_input_tokens,
        usage.cache_creation.as_ref(),
    );
    let mut counts = match usage.iterations.as_ref() {
        Some(first) if top == TokenCounts::default() => token_counts(
            first.input_tokens,
            first.output_tokens,
            first.cache_creation_input_tokens,
            first.cache_read_input_tokens,
            first.cache_creation.as_ref(),
        ),
        _ => top,
    };
    counts.web_search = usage.server_tool_use.map_or(0, |s| s.web_search_requests);
    Ok(Some(UsageLine {
        message_id,
        request_id: line.request_id.unwrap_or_default(),
        session_id,
        at_ms,
        model,
        project: line.cwd,
        sidechain: line.sidechain,
        counts,
        speed: usage.speed,
        final_line: message.stop_reason.is_some(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn the_prefilter_finds_the_type_with_or_without_a_space() {
        for line in [
            &br#"{"type":"assistant"}"#[..],
            br#"{"a":1,"type": "assistant","b":2}"#,
            br#"{"message":{"role":"assistant"},"type":"assistant"}"#,
        ] {
            assert!(may_be_assistant(line), "{}", String::from_utf8_lossy(line));
        }
        for line in [
            &b""[..],
            b"assistant",
            br#"{"type":"user","message":{"role":"assistant"}}"#,
            br#"{"type":"assistants"}"#,
            br#"{"type":  "assistant"}"#,
            br#"{"subtype":"x","role":"assistant"}"#,
            br#"{"type":"assistant"#,
        ] {
            assert!(!may_be_assistant(line), "{}", String::from_utf8_lossy(line));
        }
    }

    /// The prefilter's meaning, spelled the slow way.
    fn naive(line: &[u8]) -> bool {
        [&br#""type":"assistant""#[..], br#""type": "assistant""#]
            .iter()
            .any(|n| line.windows(n.len()).any(|w| w == *n))
    }

    proptest! {
        #[test]
        fn the_prefilter_agrees_with_a_naive_search(
            parts in proptest::collection::vec(
                prop_oneof![
                    Just(br#""type":"assistant""#.to_vec()),
                    Just(br#""type": "assistant""#.to_vec()),
                    Just(br#""assistant""#.to_vec()),
                    Just(br#""type":"#.to_vec()),
                    Just(b" ".to_vec()),
                    proptest::collection::vec(any::<u8>(), 0..24),
                ],
                0..12,
            )
        ) {
            let line = parts.concat();
            prop_assert_eq!(may_be_assistant(&line), naive(&line));
        }
    }
}
```

Create `crates/tagteam-cc/src/prices.rs`:

```rust
//! The shipped API list prices for Claude Code's models (token report spec §8.1).

use tagteam_core::tokens::PriceTable;

/// Empty until the table ships; a model it lacks is unpriced (spec §8.3).
pub static PRICES: PriceTable = PriceTable {
    as_of: "",
    models: &[],
};
```

`crates/tagteam-cc/src/lib.rs` and `crates/tagteam-cc/src/provider.rs`:

```diff
--- a/crates/tagteam-cc/src/lib.rs
+++ b/crates/tagteam-cc/src/lib.rs
@@ pub mod locks;
 pub mod naming;
 pub mod oauth;
 pub mod paths;
+pub mod prices;
 pub mod provider;
 mod session;
 pub mod shape;
+pub mod transcripts;
 pub mod usage;
 
 pub use naming::{ItemKind, keychain_account, keychain_service};
--- a/crates/tagteam-cc/src/provider.rs
+++ b/crates/tagteam-cc/src/provider.rs
@@
 use std::ffi::OsString;
+use std::io;
 use std::path::{Path, PathBuf};
 use std::sync::Arc;
 use std::time::Duration;
 
 use serde_json::{Map, Value};
+use tagteam_core::tokens::PriceTable;
 use tagteam_core::{CLAUDE_CODE, Fingerprint, IdentityKey, ProviderId};
 use tagteam_provider::http::Http;
 use tagteam_provider::process::{ProcessSpawner, SpawnSpec};
@@ use tagteam_provider::profile::refuse_linked_credential;
 use tagteam_provider::provider::{DeadReason, RefreshResult};
 use tagteam_provider::{
     BeforeFallback, Cancel, Capabilities, Check, CredLocks, Credential, DoomedEntry, Env,
-    FreshCredential, Identity, IdentitySurface, Keychain, KindTraits, LiveAuth, LiveChange,
-    LiveLockSet, LiveLocks, LockError, LockState, MergeReport, MustShare, MutationGuard, Pace,
-    PollBudget, Provider, ProviderError, Read, SessionEnv, SharePolicy, StoredLogin, Undo,
-    UsageResult, Validity, Window, Written,
+    FreshCredential, Identity, IdentitySurface, Keychain, KindTraits, LineError, LiveAuth,
+    LiveChange, LiveLockSet, LiveLocks, LockError, LockState, MergeReport, MustShare,
+    MutationGuard, Pace, PollBudget, Provider, ProviderError, Read, SessionEnv, SharePolicy,
+    StoredLogin, Undo, UsageLine, UsageResult, Validity, Window, Written,
 };
 
 use crate::config;
@@ use crate::locks;
 use crate::naming::{ItemKind, keychain_account, keychain_service};
 use crate::oauth;
 use crate::paths::{CcPaths, nfc};
+use crate::prices;
 use crate::session::{self, CC_MUST_SHARE, CC_PRIVATE, CC_SHARED};
 use crate::shape::{self, KIND_API_KEY, KINDS, MACHINE_SHARED_KEYS};
+use crate::transcripts;
 use crate::usage;
 
 /// A live read that is too stale or unreliable to compose from, but that isn't itself an
@@ impl Provider for ClaudeCode {
             api_keys: true,
             sessions: true,
             statusline: true,
-            transcripts: false,
+            transcripts: true,
         }
     }
 
@@ impl Provider for ClaudeCode {
         profile.join("sessions")
     }
 
+    /// `<config_home>/sessions` (token report ruling R2).
+    fn default_session_records_dir(&self, env: &Env) -> Option<PathBuf> {
+        Some(CcPaths::resolve(env).config_home.join("sessions"))
+    }
+
     fn supervisor_lock(&self, profile: &Path) -> Option<PathBuf> {
         Some(profile.join("daemon.lock"))
     }
@@ impl Provider for ClaudeCode {
         }
         hosts
     }
+
+    /// `<config_home>/projects`, which every profile links (token report spec §2.1).
+    fn transcript_files(&self, env: &Env) -> io::Result<Vec<PathBuf>> {
+        transcripts::discover(&env.guard(CcPaths::resolve(env).config_home.join("projects")))
+    }
+
+    fn parse_usage_line(&self, line: &[u8]) -> Result<Option<UsageLine>, LineError> {
+        transcripts::parse_usage_line(line)
+    }
+
+    fn price_table(&self) -> &'static PriceTable {
+        &prices::PRICES
+    }
 }
 
 #[cfg(test)]
```

- [ ] **Step 4: Run them and see them pass**

```
cargo test -p tagteam-core --lib --locked time::
cargo test -p tagteam-cc --locked
```

Expected: the three `time::` tests pass; every `tagteam-cc` test passes, among them the 14 of `tests/transcripts.rs`, `claude_code_has_every_capability_and_the_kind_table`, `claude_code_reads_transcripts_under_its_config_home`, and the lib's `transcripts::tests::the_prefilter_finds_the_type_with_or_without_a_space` and `transcripts::tests::the_prefilter_agrees_with_a_naive_search`.

- [ ] **Step 5: Lint**

```
cargo fmt --all --check
cargo clippy -p tagteam-core -p tagteam-cc --all-targets --locked -- -D warnings
```

Expected: no output from fmt; clippy finishes with no warning.

- [ ] **Step 6: Commit**

```
git add Cargo.toml Cargo.lock crates/tagteam-core/src/time.rs crates/tagteam-cc/Cargo.toml crates/tagteam-cc/src/lib.rs crates/tagteam-cc/src/provider.rs crates/tagteam-cc/src/transcripts.rs crates/tagteam-cc/src/prices.rs crates/tagteam-cc/tests/provider.rs crates/tagteam-cc/tests/transcripts.rs crates/tagteam-cc/tests/parser_memory.rs crates/tagteam-cc/tests/fixtures/transcripts
git commit -m "Read Claude Code transcripts: discovery and the usage line parser"
```

#### Notes for the controller

- Verified by the plan writer on a copy of the branch with contract-exact stubs of Tasks 2-3: Steps 4 and 5 pass on macOS. A release-mode run of the parser over 300 MB of synthetic lines (fixture lines mixed with 40 KB user lines) read 2.07 GB/s on one thread, so parsing is far inside spec §1.1's 10 s for 3.42 GB.
- Two additions beside the outline, both inside the head's dependency rule: `proptest` becomes a dev-dependency of `tagteam-cc` (it is an existing workspace dependency; the outline's `hostile_lines_are_errors_never_panics` needs it), and the workspace comment that called it "tagteam-core only" is corrected.
- Parser decisions inside the spec, worth recording under Rulings: `input_tokens` and `output_tokens` are required on an assistant line (a line without them is an error); the cache counts default to 0 when absent; cache writes the 5-minute/1-hour split leaves unaccounted count as 5-minute writes (an older line without `cache_creation` puts its whole `cache_creation_input_tokens` there); a non-null `stop_reason` of any JSON type marks the line final. Discovery follows a link to a file but not a link to a directory; `projects` itself is read through a link, as a profile's is.
- Batch D runs this task beside Tasks 10 and 14 in one worktree. This task edits `crates/tagteam-core/src/time.rs`, which every crate builds on; the edit is additive (a new function and a private helper `parse_iso8601` now calls), so a sibling's focused tests only fail to build while the file is half-written.
- Task 15 can use the fixture tree for its CC-provider tests: copy `crates/tagteam-cc/tests/fixtures/transcripts/projects` into a temp `<home>/.claude/projects` (path from `env!("CARGO_MANIFEST_DIR")/../tagteam-cc/...`). It yields 4 responses (`msg_A` finalized with 4 lines across 2 files, `msg_B` from `iterations[0]`, `msg_C` an unfinalized sidechain response on `claude-haiku-4-5-20251001`, `msg_D` in the resumed session) and 2 skipped lines.

---

### Task 9: The shipped price table

**Depends on:** 8
**Files:**
- Modify: `crates/tagteam-cc/src/prices.rs` (fill `PRICES`; unit tests)
- Test: `crates/tagteam-cc/src/prices.rs` (`mod tests`)

**Interfaces:**
- Consumes:
  ```rust
  // tagteam_core::tokens (Tasks 2-3)
  #[derive(Debug, Clone, Copy, PartialEq)]
  pub struct RateCard { pub input: f64, pub output: f64, pub cache_write_5m: f64, pub cache_write_1h: f64, pub cache_read: f64 } // USD per MTok
  #[derive(Debug, Clone, Copy, PartialEq)]
  pub struct ModelPrice { pub base: RateCard, pub long_prompt: Option<(u64, RateCard)>, pub fast: Option<RateCard>, pub web_search_per_1k: Option<f64> }
  #[derive(Debug)]
  pub struct PriceTable { pub as_of: &'static str, pub models: &'static [(&'static str, ModelPrice)] }
  impl Prices {
      pub fn new(table: &PriceTable, overrides: &BTreeMap<String, PriceOverride>) -> (Prices, Vec<String>);
      pub fn lookup(&self, model: &str) -> Option<&ModelPrice>; // exact, then strip_date_suffix
  }
  // tagteam_core::time
  pub fn parse_iso8601(s: &str) -> Option<i64>;
  // tagteam_cc::prices (Task 8)
  pub static PRICES: PriceTable; // empty
  ```
- Produces:
  ```rust
  // tagteam_cc::prices
  pub static PRICES: PriceTable; // as_of "2026-10-14"; the 16 current models below; ClaudeCode::price_table() returns it
  ```

The rates, fetched from <https://platform.claude.com/docs/en/about-claude/pricing.md> on 2026-10-14 (the plan's date), USD per million tokens as `input, output, 5m cache write, 1h cache write, cache read`. They differ from the outline's cached list in two places, and the page wins: Claude Sonnet 5.5's cache read is $0.10 (0.05x input), not $0.20; Claude Opus 4.8 has fast mode ($10 / $50). The page also lists Claude Mythos 5, which the outline did not. Retired models (Opus 4.1, Opus 4, Sonnet 4, Haiku 3.5) are left out: spec §8.1 asks for every *current* model.

| Model id | Base card | Fast card | Long prompt |
|---|---|---|---|
| `claude-fable-5-1`, `claude-mythos-5-1` | 10, 50, 12.50, 20, 0.25 | none | none |
| `claude-fable-5`, `claude-mythos-5` | 10, 50, 12.50, 20, 1 | none | none |
| `claude-opus-5-5` | 4, 20, 5, 8, 0.20 | 8, 40, 10, 16, 0.40 | none |
| `claude-opus-5`, `claude-opus-4-8` | 5, 25, 6.25, 10, 0.50 | 10, 50, 12.50, 20, 1 | none |
| `claude-opus-4-7` | 5, 25, 6.25, 10, 0.50 | none (fast is refused) | none |
| `claude-opus-4-6` | 5, 25, 6.25, 10, 0.50 | the base card (fast runs at standard speed and standard rates) | none |
| `claude-opus-4-5` | 5, 25, 6.25, 10, 0.50 | none | none |
| `claude-sonnet-5-5` | 2, 10, 2.50, 4, 0.10 | none | none |
| `claude-sonnet-5` | 2, 10, 2.50, 4, 0.20 | none | none |
| `claude-sonnet-4-6`, `claude-sonnet-4-5` | 3, 15, 3.75, 6, 0.30 | none | none |
| `claude-haiku-5-5` | 0.10, 0.50, 0.125, 0.20, 0.01 | none | above 100,000 prompt tokens: 0.50, 2.50, 0.625, 1, 0.05 |
| `claude-haiku-4-5` | 1, 5, 1.25, 2, 0.10 | none | none |

Web search is $10 per 1,000 searches on every model. The page prices fast mode as input and output only and says the caching multipliers apply on top, so each fast card's cache rates are its input rate times 1.25 (5m), 2 (1h) and the model's read multiplier (0.05 on Opus 5.5, 0.1 otherwise). The Mythos ids follow the page's naming of the other models (`claude-mythos-5-1`, `claude-mythos-5`); the models overview names only the four current ids, so these two are inferred. Dated ids such as `claude-haiku-4-5-20251001` match their dateless entry through `strip_date_suffix` (spec §8.1).

- [ ] **Step 1: Write the failing tests**

Append to `crates/tagteam-cc/src/prices.rs`:

```rust
#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use tagteam_core::time::parse_iso8601;
    use tagteam_core::tokens::Prices;

    use super::*;

    #[test]
    fn every_model_seen_in_transcripts_is_priced() {
        // Spec §2.6's models; the dated Haiku through date stripping (spec §8.1).
        let (prices, overridden) = Prices::new(&PRICES, &BTreeMap::new());
        assert!(overridden.is_empty());
        for model in [
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-sonnet-5",
            "claude-opus-5",
            "claude-fable-5-1",
            "claude-haiku-4-5-20251001",
        ] {
            assert!(prices.lookup(model).is_some(), "{model} is unpriced");
        }
        assert_eq!(
            prices.lookup("claude-haiku-4-5-20251001"),
            prices.lookup("claude-haiku-4-5")
        );
        assert_eq!(prices.lookup("<synthetic>"), None);
    }

    #[test]
    fn rates_are_positive_and_cache_reads_cheaper_than_input() {
        let names: BTreeSet<&str> = PRICES.models.iter().map(|(name, _)| *name).collect();
        assert!(!names.is_empty());
        assert_eq!(names.len(), PRICES.models.len(), "each model once");
        for (name, price) in PRICES.models {
            let cards = [
                Some(price.base),
                price.long_prompt.map(|(_, c)| c),
                price.fast,
            ];
            for c in cards.into_iter().flatten() {
                assert!(c.cache_read > 0.0, "{name}: {c:?}");
                assert!(c.cache_read < c.input, "{name}: {c:?}");
                assert!(c.input < c.cache_write_5m, "{name}: {c:?}");
                assert!(c.cache_write_5m < c.cache_write_1h, "{name}: {c:?}");
                assert!(c.input < c.output, "{name}: {c:?}");
            }
            if let Some((above, long)) = price.long_prompt {
                assert!(above > 0 && long.input > price.base.input, "{name}");
            }
            assert_eq!(price.web_search_per_1k, Some(10.0), "{name}");
        }
    }

    #[test]
    fn as_of_is_an_iso_date() {
        assert_eq!(PRICES.as_of.len(), 10);
        assert!(parse_iso8601(&format!("{}T00:00:00Z", PRICES.as_of)).is_some());
    }
}
```

- [ ] **Step 2: Run them and see them fail**

```
cargo test -p tagteam-cc --lib --locked prices::
```

Expected: all three fail on Task 8's empty table: `claude-opus-5-5 is unpriced`, `assertion failed: !names.is_empty()`, and `assertion `left == right` failed` (left `0`, right `10`) in `as_of_is_an_iso_date`.

- [ ] **Step 3: Implement**

Before writing the table, fetch the page again (`https://platform.claude.com/docs/en/about-claude/pricing.md`). If any rate differs from the table above, the page wins: change that entry and set `as_of` to the day you fetched it, and say so in your report. If the fetch fails, keep the table and `as_of` below as they are and say so.

Replace `crates/tagteam-cc/src/prices.rs` with:

```rust
//! The shipped API list prices for Claude Code's models (token report spec §8.1), from
//! <https://platform.claude.com/docs/en/about-claude/pricing> on `as_of`. A release updates
//! them; `[provider.claude-code.prices]` overrides cover the gap (spec §8.2).

use tagteam_core::tokens::{ModelPrice, PriceTable, RateCard};

/// The web search tool: $10 per 1,000 searches, on every model.
const WEB_SEARCH_PER_1K: f64 = 10.0;

/// USD per million tokens.
const fn card(
    input: f64,
    output: f64,
    cache_write_5m: f64,
    cache_write_1h: f64,
    cache_read: f64,
) -> RateCard {
    RateCard {
        input,
        output,
        cache_write_5m,
        cache_write_1h,
        cache_read,
    }
}

const fn priced(base: RateCard, fast: Option<RateCard>) -> ModelPrice {
    ModelPrice {
        base,
        long_prompt: None,
        fast,
        web_search_per_1k: Some(WEB_SEARCH_PER_1K),
    }
}

const FABLE_5_1: RateCard = card(10.0, 50.0, 12.5, 20.0, 0.25);
const FABLE_5: RateCard = card(10.0, 50.0, 12.5, 20.0, 1.0);
const OPUS_5_5: RateCard = card(4.0, 20.0, 5.0, 8.0, 0.2);
/// Fast mode: $8 / $40, with the model's cache multipliers on top.
const OPUS_5_5_FAST: RateCard = card(8.0, 40.0, 10.0, 16.0, 0.4);
const OPUS: RateCard = card(5.0, 25.0, 6.25, 10.0, 0.5);
/// Fast mode on Claude Opus 5 and Claude Opus 4.8: $10 / $50.
const OPUS_FAST: RateCard = card(10.0, 50.0, 12.5, 20.0, 1.0);
const SONNET_5_5: RateCard = card(2.0, 10.0, 2.5, 4.0, 0.1);
const SONNET_5: RateCard = card(2.0, 10.0, 2.5, 4.0, 0.2);
const SONNET_4: RateCard = card(3.0, 15.0, 3.75, 6.0, 0.3);
const HAIKU_5_5: RateCard = card(0.1, 0.5, 0.125, 0.2, 0.01);
/// Claude Haiku 5.5 for a prompt over 100,000 tokens, cache reads and writes included.
const HAIKU_5_5_LONG: RateCard = card(0.5, 2.5, 0.625, 1.0, 0.05);
const HAIKU_4_5: RateCard = card(1.0, 5.0, 1.25, 2.0, 0.1);

/// Every current model on the pricing page (retired ones left out), and so every model of spec
/// §2.6. Dated ids (`claude-haiku-4-5-20251001`) match through `strip_date_suffix`.
pub static PRICES: PriceTable = PriceTable {
    as_of: "2026-10-14",
    models: &[
        ("claude-fable-5-1", priced(FABLE_5_1, None)),
        ("claude-mythos-5-1", priced(FABLE_5_1, None)),
        ("claude-fable-5", priced(FABLE_5, None)),
        ("claude-mythos-5", priced(FABLE_5, None)),
        ("claude-opus-5-5", priced(OPUS_5_5, Some(OPUS_5_5_FAST))),
        ("claude-opus-5", priced(OPUS, Some(OPUS_FAST))),
        ("claude-opus-4-8", priced(OPUS, Some(OPUS_FAST))),
        // Fast mode is refused on Claude Opus 4.7.
        ("claude-opus-4-7", priced(OPUS, None)),
        // Claude Opus 4.6 runs a fast request at standard speed and bills it at standard rates.
        ("claude-opus-4-6", priced(OPUS, Some(OPUS))),
        ("claude-opus-4-5", priced(OPUS, None)),
        ("claude-sonnet-5-5", priced(SONNET_5_5, None)),
        ("claude-sonnet-5", priced(SONNET_5, None)),
        ("claude-sonnet-4-6", priced(SONNET_4, None)),
        ("claude-sonnet-4-5", priced(SONNET_4, None)),
        (
            "claude-haiku-5-5",
            ModelPrice {
                long_prompt: Some((100_000, HAIKU_5_5_LONG)),
                ..priced(HAIKU_5_5, None)
            },
        ),
        ("claude-haiku-4-5", priced(HAIKU_4_5, None)),
    ],
};

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use tagteam_core::time::parse_iso8601;
    use tagteam_core::tokens::Prices;

    use super::*;

    #[test]
    fn every_model_seen_in_transcripts_is_priced() {
        // Spec §2.6's models; the dated Haiku through date stripping (spec §8.1).
        let (prices, overridden) = Prices::new(&PRICES, &BTreeMap::new());
        assert!(overridden.is_empty());
        for model in [
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-sonnet-5",
            "claude-opus-5",
            "claude-fable-5-1",
            "claude-haiku-4-5-20251001",
        ] {
            assert!(prices.lookup(model).is_some(), "{model} is unpriced");
        }
        assert_eq!(
            prices.lookup("claude-haiku-4-5-20251001"),
            prices.lookup("claude-haiku-4-5")
        );
        assert_eq!(prices.lookup("<synthetic>"), None);
    }

    #[test]
    fn rates_are_positive_and_cache_reads_cheaper_than_input() {
        let names: BTreeSet<&str> = PRICES.models.iter().map(|(name, _)| *name).collect();
        assert!(!names.is_empty());
        assert_eq!(names.len(), PRICES.models.len(), "each model once");
        for (name, price) in PRICES.models {
            let cards = [
                Some(price.base),
                price.long_prompt.map(|(_, c)| c),
                price.fast,
            ];
            for c in cards.into_iter().flatten() {
                assert!(c.cache_read > 0.0, "{name}: {c:?}");
                assert!(c.cache_read < c.input, "{name}: {c:?}");
                assert!(c.input < c.cache_write_5m, "{name}: {c:?}");
                assert!(c.cache_write_5m < c.cache_write_1h, "{name}: {c:?}");
                assert!(c.input < c.output, "{name}: {c:?}");
            }
            if let Some((above, long)) = price.long_prompt {
                assert!(above > 0 && long.input > price.base.input, "{name}");
            }
            assert_eq!(price.web_search_per_1k, Some(10.0), "{name}");
        }
    }

    #[test]
    fn as_of_is_an_iso_date() {
        assert_eq!(PRICES.as_of.len(), 10);
        assert!(parse_iso8601(&format!("{}T00:00:00Z", PRICES.as_of)).is_some());
    }
}
```

- [ ] **Step 4: Run them and see them pass**

```
cargo test -p tagteam-cc --lib --locked prices::
cargo test -p tagteam-cc --locked --test provider claude_code_reads_transcripts_under_its_config_home
```

Expected: 3/3 pass, and the provider test still passes (it only checks `price_table()` is `&PRICES`).

- [ ] **Step 5: Lint**

```
cargo fmt --all --check
cargo clippy -p tagteam-cc --all-targets --locked -- -D warnings
```

Expected: no output from fmt; clippy finishes with no warning.

- [ ] **Step 6: Commit**

```
git add crates/tagteam-cc/src/prices.rs
git commit -m "Ship the API list price table"
```

#### Notes for the controller

- The rates were fetched by the plan writer on 2026-10-14 and win over the outline's list (Sonnet 5.5's cache read, Opus 4.8's fast card, Mythos 5 added). Step 3 has the implementer fetch again and report any change.
- `claude-opus-4-6` gets its base card as its fast card, because the page says a fast request on it "runs at standard speed and is billed at standard rates". Without it, spec §8.1 would leave such a response unpriced. `claude-opus-4-7` has no fast card: the API refuses fast mode there, so no such response exists.
- Verified by the plan writer on a copy of the branch with contract-exact stubs of Tasks 2-3 (`Prices::lookup` as the contract describes): Steps 4 and 5 pass on macOS.

---

### Task 10: Fake transcripts for engine tests

**Depends on:** 7
**Files:**
- Create: `crates/tagteam-fake/src/transcripts.rs` (`FakeUsage`, `usage_line_json`, `append_transcript`, discovery, the line parser, the `fake-1` price table)
- Modify: `crates/tagteam-fake/src/paths.rs` (`FakePaths.transcripts = dir.join("transcripts")`)
- Modify: `crates/tagteam-fake/src/lib.rs` (`mod transcripts;` and its re-exports)
- Modify: `crates/tagteam-fake/src/provider.rs` (`transcripts` capability, on by default, with `FakeAgent::without_transcripts()`; `default_session_records_dir` = `<dir>/procs`; `transcript_files`, `parse_usage_line`, `price_table`)
- Test: `crates/tagteam-fake/tests/transcripts.rs` (new), `crates/tagteam-fake/tests/provider.rs` (the capability assertion gains `transcripts: true`)

**Interfaces:**
- Consumes:
  ```rust
  // tagteam_core::tokens (Tasks 2-3)
  pub struct TokenCounts { pub input: u64, pub output: u64, pub cache_write_5m: u64, pub cache_write_1h: u64, pub cache_read: u64, pub web_search: u64 }
  pub struct RateCard { pub input: f64, pub output: f64, pub cache_write_5m: f64, pub cache_write_1h: f64, pub cache_read: f64 }
  pub struct ModelPrice { pub base: RateCard, pub long_prompt: Option<(u64, RateCard)>, pub fast: Option<RateCard>, pub web_search_per_1k: Option<f64> }
  pub struct PriceTable { pub as_of: &'static str, pub models: &'static [(&'static str, ModelPrice)] }
  // tagteam_provider (Task 7)
  pub struct UsageLine { pub message_id: String, pub request_id: String, pub session_id: String, pub at_ms: i64, pub model: String, pub project: Option<String>, pub sidechain: bool, pub counts: TokenCounts, pub speed: Option<String>, pub final_line: bool }
  pub struct LineError(pub String);
  pub struct Capabilities { /* existing */ pub transcripts: bool }
  fn transcript_files(&self, _env: &Env) -> io::Result<Vec<PathBuf>>;
  fn parse_usage_line(&self, _line: &[u8]) -> Result<Option<UsageLine>, LineError>;
  fn price_table(&self) -> &'static PriceTable;
  fn default_session_records_dir(&self, _env: &Env) -> Option<PathBuf>;
  ```
- Produces (`tagteam_fake`, for the engine's token tests, Tasks 15, 16, 18, 22, 23):
  ```rust
  /// The fields of the UsageLine FakeAgent reads back. Default: message "m1", request "r1",
  /// session "s1", at 1_790_000_000_000 - 3_600_000 (an hour before the engine fixtures' clock),
  /// model "fake-1", project Some("/w"), not a sidechain, counts {1, 2, 3, 4, 5, web_search 0},
  /// speed None, final.
  #[derive(Debug, Clone, PartialEq)]
  pub struct FakeUsage { pub message_id: String, pub request_id: String, pub session_id: String, pub at_ms: i64, pub model: String, pub project: Option<String>, pub sidechain: bool, pub counts: TokenCounts, pub speed: Option<String>, pub final_line: bool }
  impl Default for FakeUsage;
  impl FakeUsage { pub fn usage_line(&self) -> UsageLine; }
  /// One line, no newline: {"t":"usage","id":..,"req":..,"session":..,"at":..,"model":..,"cwd":..,"side":..,"final":..,"in":..,"out":..,"cw5":..,"cw1":..,"cr":..,"ws":..,"speed":..}
  pub fn usage_line_json(u: &FakeUsage) -> String;
  /// Appends `text` verbatim to `<fake dir>/transcripts/<name>` (creating the directory) and
  /// returns the path; panics on I/O failure. Partial and malformed lines are the caller's text.
  pub fn append_transcript(env: &Env, name: &str, text: &str) -> PathBuf;
  pub struct FakePaths { /* existing */ pub transcripts: PathBuf } // <fake dir>/transcripts
  impl FakeAgent { pub fn without_transcripts(self) -> Self; }       // capabilities().transcripts == false
  // FakeAgent (Provider): capabilities().transcripts == true by default;
  //   transcript_files(env) = every *.jsonl regular file directly in FakePaths::resolve(env).transcripts, sorted;
  //   parse_usage_line: `t` other than "usage" -> Ok(None); not a JSON object, or a usage line with
  //     a missing or mistyped field -> Err; absent "req" -> "" (ruling R5);
  //   price_table(): as_of "2026-01-01", "fake-1" = base {1, 2, 3, 4, 0.5}, web_search_per_1k Some(10.0);
  //   default_session_records_dir(env) = Some(FakePaths::resolve(env).dir.join("procs"))
  ```

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-fake/tests/transcripts.rs`:

```rust
//! FakeAgent's transcripts (token report spec §3.2): the engine's token tests write them through
//! `FakeUsage`, `usage_line_json` and `append_transcript`.

use std::fs;
use std::path::PathBuf;

use tagteam_core::tokens::TokenCounts;
use tagteam_fake::{FakeAgent, FakePaths, FakeUsage, append_transcript, usage_line_json};
use tagteam_provider::{Env, Provider};

fn fx() -> (tempfile::TempDir, Env) {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    (d, env)
}

#[test]
fn round_trips_a_usage_line() {
    let fake = FakeAgent::new();
    let stale = FakeUsage {
        message_id: "m2".into(),
        request_id: String::new(),
        session_id: "s2".into(),
        at_ms: -5,
        model: "fake-2".into(),
        project: None,
        sidechain: true,
        counts: TokenCounts {
            input: u64::MAX,
            web_search: 9,
            ..TokenCounts::default()
        },
        speed: Some("fast".into()),
        final_line: false,
    };
    for u in [FakeUsage::default(), stale] {
        let line = usage_line_json(&u);
        assert!(!line.contains('\n'));
        assert_eq!(
            fake.parse_usage_line(line.as_bytes()),
            Ok(Some(u.usage_line()))
        );
    }
    let without_req = br#"{"t":"usage","id":"m","session":"s","at":1,"model":"fake-1","side":false,"final":true,"in":1,"out":1,"cw5":0,"cw1":0,"cr":0,"ws":0}"#;
    let line = fake.parse_usage_line(without_req).unwrap().unwrap();
    assert_eq!(
        (line.request_id.as_str(), line.project, line.speed),
        ("", None, None),
        "ruling R5: no req keys on an empty one"
    );
}

#[test]
fn other_types_are_none() {
    let fake = FakeAgent::new();
    for line in [&br#"{"t":"note","id":"m1"}"#[..], br#"{}"#, br#"{"t":7}"#] {
        assert_eq!(fake.parse_usage_line(line), Ok(None));
    }
    let good = usage_line_json(&FakeUsage::default());
    for bad in [
        "not json".to_owned(),
        "[1]".to_owned(),
        r#"{"t":"usage"}"#.to_owned(),
        good.replace(r#""in":1"#, r#""in":-1"#),
        good.replace(r#""out":2"#, r#""out":"2""#),
        good.replace(r#""side":false"#, r#""side":null"#),
        good.replace(r#""cwd":"/w""#, r#""cwd":7"#),
        good[..good.len() - 1].to_owned(),
    ] {
        assert!(fake.parse_usage_line(bad.as_bytes()).is_err(), "{bad}");
    }
}

#[test]
fn discovers_only_jsonl() {
    let (_d, env) = fx();
    let fake = FakeAgent::new();
    assert_eq!(
        fake.transcript_files(&env).unwrap(),
        Vec::<PathBuf>::new(),
        "no directory yet"
    );
    let a = append_transcript(&env, "a.jsonl", "");
    let b = append_transcript(&env, "b.jsonl", "{}\n");
    append_transcript(&env, "notes.txt", "x\n");
    let dir = FakePaths::resolve(&env).transcripts;
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(dir.join("sub/c.jsonl"), "{}\n").unwrap();
    fs::create_dir_all(dir.join("d.jsonl")).unwrap();
    assert_eq!(fake.transcript_files(&env).unwrap(), vec![a, b]);
}

#[test]
fn appending_writes_text_as_it_is() {
    let (_d, env) = fx();
    let first = usage_line_json(&FakeUsage::default());
    let path = append_transcript(&env, "s.jsonl", &format!("{first}\n"));
    append_transcript(&env, "s.jsonl", "{\"t\":\"usa");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("{first}\n{{\"t\":\"usa")
    );
    assert_eq!(
        path,
        FakePaths::resolve(&env).dir.join("transcripts/s.jsonl")
    );
}

#[test]
fn its_records_prices_and_capability() {
    let (_d, env) = fx();
    let fake = FakeAgent::new();
    assert!(fake.capabilities().transcripts);
    assert!(
        !FakeAgent::new()
            .without_transcripts()
            .capabilities()
            .transcripts
    );
    assert_eq!(
        fake.default_session_records_dir(&env),
        Some(env.home.join(".fakeagent/procs"))
    );
    let table = fake.price_table();
    assert_eq!(table.as_of, "2026-01-01");
    assert_eq!(
        table.models.iter().map(|(m, _)| *m).collect::<Vec<_>>(),
        ["fake-1"]
    );
    assert_eq!(table.models[0].1.web_search_per_1k, Some(10.0));
}
```

`crates/tagteam-fake/tests/provider.rs`, in `kinds_capabilities_endpoints_and_surface`:

```diff
--- a/crates/tagteam-fake/tests/provider.rs
+++ b/crates/tagteam-fake/tests/provider.rs
@@ fn kinds_capabilities_endpoints_and_surface() {
             usage: true,
             refresh: true,
             sessions: true,
+            transcripts: true,
             ..Capabilities::default()
         }
     );
```

- [ ] **Step 2: Run them and see them fail**

```
cargo test -p tagteam-fake --locked --test transcripts
cargo test -p tagteam-fake --locked --test provider kinds_capabilities_endpoints_and_surface
```

Expected: `tests/transcripts.rs` does not compile: `error[E0432]: unresolved imports `tagteam_fake::FakeUsage`, `tagteam_fake::append_transcript`, `tagteam_fake::usage_line_json``, `error[E0609]: no field `transcripts` on type `FakePaths`` and `error[E0599]: no method named `without_transcripts` found for struct `FakeAgent``. `kinds_capabilities_endpoints_and_surface` fails its first `assert_eq!` (`transcripts: false` against `true`).

- [ ] **Step 3: Implement**

Create `crates/tagteam-fake/src/transcripts.rs`:

```rust
//! FakeAgent's transcripts (token report spec §3.2), deliberately unlike Claude Code's: one flat
//! directory, one line per snapshot, short keys, and a model of its own.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use tagteam_core::tokens::{ModelPrice, PriceTable, RateCard, TokenCounts};
use tagteam_provider::{Env, LineError, UsageLine};

use crate::paths::FakePaths;

/// FakeAgent's one model, `fake-1`.
pub(crate) static PRICES: PriceTable = PriceTable {
    as_of: "2026-01-01",
    models: &[(
        "fake-1",
        ModelPrice {
            base: RateCard {
                input: 1.0,
                output: 2.0,
                cache_write_5m: 3.0,
                cache_write_1h: 4.0,
                cache_read: 0.5,
            },
            long_prompt: None,
            fast: None,
            web_search_per_1k: Some(10.0),
        },
    )],
};

/// One usage line for a test to write, with the fields of the `UsageLine` FakeAgent reads back
/// from it. The default is a final `fake-1` line an hour before the engine fixtures' clock
/// (1_790_000_000_000).
#[derive(Debug, Clone, PartialEq)]
pub struct FakeUsage {
    pub message_id: String,
    pub request_id: String,
    pub session_id: String,
    pub at_ms: i64,
    pub model: String,
    pub project: Option<String>,
    pub sidechain: bool,
    pub counts: TokenCounts,
    pub speed: Option<String>,
    pub final_line: bool,
}

impl Default for FakeUsage {
    fn default() -> Self {
        Self {
            message_id: "m1".into(),
            request_id: "r1".into(),
            session_id: "s1".into(),
            at_ms: 1_790_000_000_000 - 3_600_000,
            model: "fake-1".into(),
            project: Some("/w".into()),
            sidechain: false,
            counts: TokenCounts {
                input: 1,
                output: 2,
                cache_write_5m: 3,
                cache_write_1h: 4,
                cache_read: 5,
                web_search: 0,
            },
            speed: None,
            final_line: true,
        }
    }
}

impl FakeUsage {
    /// What `parse_usage_line` reads from `usage_line_json(self)`.
    pub fn usage_line(&self) -> UsageLine {
        UsageLine {
            message_id: self.message_id.clone(),
            request_id: self.request_id.clone(),
            session_id: self.session_id.clone(),
            at_ms: self.at_ms,
            model: self.model.clone(),
            project: self.project.clone(),
            sidechain: self.sidechain,
            counts: self.counts,
            speed: self.speed.clone(),
            final_line: self.final_line,
        }
    }
}

/// `u` as one FakeAgent transcript line, without its newline.
pub fn usage_line_json(u: &FakeUsage) -> String {
    json!({
        "t": "usage",
        "id": u.message_id,
        "req": u.request_id,
        "session": u.session_id,
        "at": u.at_ms,
        "model": u.model,
        "cwd": u.project,
        "side": u.sidechain,
        "final": u.final_line,
        "in": u.counts.input,
        "out": u.counts.output,
        "cw5": u.counts.cache_write_5m,
        "cw1": u.counts.cache_write_1h,
        "cr": u.counts.cache_read,
        "ws": u.counts.web_search,
        "speed": u.speed,
    })
    .to_string()
}

/// Appends `text` as it is to `<transcripts>/<name>`, creating the directory: a test helper that
/// panics on I/O failure. Returns the file's path.
pub fn append_transcript(env: &Env, name: &str, text: &str) -> PathBuf {
    let dir = FakePaths::resolve(env).transcripts;
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .unwrap();
    path
}

/// Every `*.jsonl` regular file in `dir`, sorted; none when `dir` is missing.
pub(crate) fn discover(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let listing = match fs::read_dir(dir) {
        Ok(l) => l,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for entry in listing {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|x| x == "jsonl") && entry.file_type()?.is_file() {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn invalid(key: &str) -> LineError {
    LineError(format!("a FakeAgent usage line has no valid {key:?}"))
}

fn text(o: &Map<String, Value>, key: &str) -> Result<String, LineError> {
    o.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invalid(key))
}

/// Text or null; absent counts as null.
fn optional_text(o: &Map<String, Value>, key: &str) -> Result<Option<String>, LineError> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(invalid(key)),
    }
}

fn count(o: &Map<String, Value>, key: &str) -> Result<u64, LineError> {
    o.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid(key))
}

fn flag(o: &Map<String, Value>, key: &str) -> Result<bool, LineError> {
    o.get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid(key))
}

/// `None` for a line whose `t` is not `"usage"`; `Err` for one that is not a JSON object, or a
/// usage line with a field missing or of the wrong type. An absent `req` is `""` (ruling R5).
pub(crate) fn parse_line(line: &[u8]) -> Result<Option<UsageLine>, LineError> {
    let Ok(Value::Object(o)) = serde_json::from_slice::<Value>(line) else {
        return Err(LineError(
            "a FakeAgent transcript line is not a JSON object".into(),
        ));
    };
    if o.get("t").and_then(Value::as_str) != Some("usage") {
        return Ok(None);
    }
    Ok(Some(UsageLine {
        message_id: text(&o, "id")?,
        request_id: optional_text(&o, "req")?.unwrap_or_default(),
        session_id: text(&o, "session")?,
        at_ms: o
            .get("at")
            .and_then(Value::as_i64)
            .ok_or_else(|| invalid("at"))?,
        model: text(&o, "model")?,
        project: optional_text(&o, "cwd")?,
        sidechain: flag(&o, "side")?,
        counts: TokenCounts {
            input: count(&o, "in")?,
            output: count(&o, "out")?,
            cache_write_5m: count(&o, "cw5")?,
            cache_write_1h: count(&o, "cw1")?,
            cache_read: count(&o, "cr")?,
            web_search: count(&o, "ws")?,
        },
        speed: optional_text(&o, "speed")?,
        final_line: flag(&o, "final")?,
    }))
}
```

`crates/tagteam-fake/src/paths.rs`, `crates/tagteam-fake/src/lib.rs` and `crates/tagteam-fake/src/provider.rs`:

```diff
--- a/crates/tagteam-fake/src/paths.rs
+++ b/crates/tagteam-fake/src/paths.rs
@@ pub struct FakePaths {
     pub credential: PathBuf,
     /// FakeAgent's one live lock, a `mkdir` lock.
     pub lock: PathBuf,
+    /// Its transcripts, `*.jsonl` in one flat directory (token report spec §3.2).
+    pub transcripts: PathBuf,
 }
 
 impl FakePaths {
@@ impl FakePaths {
             identity: dir.join("identity.json"),
             credential: dir.join("credential.json"),
             lock: dir.join(".live.lock"),
+            transcripts: dir.join("transcripts"),
             dir,
         }
     }
--- a/crates/tagteam-fake/src/lib.rs
+++ b/crates/tagteam-fake/src/lib.rs
@@
 mod paths;
 mod provider;
 mod shape;
+mod transcripts;
 mod usage;
 
 use std::fs;
@@ use tagteam_provider::Env;
 pub use paths::FakePaths;
 pub use provider::FakeAgent;
 pub use shape::{DEVICE, KIND_STATIC, KIND_TOKEN, KINDS};
+pub use transcripts::{FakeUsage, append_transcript, usage_line_json};
 
 pub const FAKE_AGENT: &str = "fake-agent";
 
--- a/crates/tagteam-fake/src/provider.rs
+++ b/crates/tagteam-fake/src/provider.rs
@@ use std::io;
 use std::path::{Path, PathBuf};
 use std::time::Duration;
 
 use serde_json::{Value, json};
 use tagteam_core::merge::{MergeKey, three_way};
+use tagteam_core::tokens::PriceTable;
 use tagteam_core::{Fingerprint, IdentityKey, ProviderId};
 use tagteam_provider::atomic::{
     ensure_private_dir, remove_target, write_atomic_private_with, write_atomic_with,
 };
 use tagteam_provider::http::{Http, HttpError, HttpRequest};
@@ use tagteam_provider::profile::{
 };
 use tagteam_provider::provider::{DeadReason, RefreshResult, TransientKind};
 use tagteam_provider::splice::{self, render_nested};
 use tagteam_provider::{
     BeforeFallback, Cancel, Capabilities, CredLocks, Credential, DoomedEntry, EntryKind, Env,
-    FreshCredential, Identity, IdentitySurface, KindTraits, LiveAuth, LiveChange, LiveLockSet,
-    LiveLocks, LockError, MergeReport, MkdirLock, MkdirLockSpec, MustShare, MutationGuard, Pace,
-    PollBudget, Provider, ProviderError, Read, ReadError, SecretStore, SessionEnv, SharePolicy,
-    StoredLogin, Undo, UsageResult, Validity, Window, Written,
+    FreshCredential, Identity, IdentitySurface, KindTraits, LineError, LiveAuth, LiveChange,
+    LiveLockSet, LiveLocks, LockError, MergeReport, MkdirLock, MkdirLockSpec, MustShare,
+    MutationGuard, Pace, PollBudget, Provider, ProviderError, Read, ReadError, SecretStore,
+    SessionEnv, SharePolicy, StoredLogin, Undo, UsageLine, UsageResult, Validity, Window, Written,
 };
 
 use crate::FAKE_AGENT;
 use crate::identity_json;
 use crate::paths::{FakePaths, HOME_VAR};
 use crate::shape::{self, DEVICE, KIND_STATIC, KINDS};
+use crate::transcripts;
 use crate::usage;
 
 /// FakeAgent's lock goes stale like Claude Code's credential locks.
 const LOCK_STALE: Duration = Duration::from_secs(60);
 const DEFAULT_BASE: &str = "https://fake-agent.invalid";
@@ const PREFS: &str = "prefs";
 const TOKEN_VAR: &str = "FAKEAGENT_TOKEN";
 
 pub struct FakeAgent {
     base: String,
     lock_budget: Duration,
+    /// The `transcripts` capability; on unless a test turns it off.
+    transcripts: bool,
 }
 
 impl Default for FakeAgent {
     fn default() -> Self {
         Self::new()
@@ impl Default for FakeAgent {
 impl FakeAgent {
     pub fn new() -> Self {
         Self {
             base: DEFAULT_BASE.to_owned(),
             lock_budget: DEFAULT_LOCK_BUDGET,
+            transcripts: true,
         }
     }
 
     pub fn with_endpoint_base(mut self, base: &str) -> Self {
         self.base = base.trim_end_matches('/').to_owned();
@@ impl FakeAgent {
     pub fn with_lock_budget(mut self, budget: Duration) -> Self {
         self.lock_budget = budget;
         self
     }
 
+    /// A FakeAgent without the `transcripts` capability, for the engine's refusals (token report
+    /// spec §11.1).
+    pub fn without_transcripts(mut self) -> Self {
+        self.transcripts = false;
+        self
+    }
+
     pub fn renew_url(&self) -> String {
         format!("{}/fa/renew", self.base)
     }
 
     pub fn whoami_url(&self) -> String {
@@ impl Provider for FakeAgent {
     fn capabilities(&self) -> Capabilities {
         Capabilities {
             usage: true,
             refresh: true,
             sessions: true,
+            transcripts: self.transcripts,
             ..Capabilities::default()
         }
     }
 
     fn identity_surface(&self, env: &Env) -> IdentitySurface {
@@ impl Provider for FakeAgent {
 
     fn session_records_dir(&self, profile: &Path) -> PathBuf {
         profile.join("procs")
     }
 
+    /// The default home's `procs`, as a profile's (token report ruling R2).
+    fn default_session_records_dir(&self, env: &Env) -> Option<PathBuf> {
+        Some(FakePaths::resolve(env).dir.join("procs"))
+    }
+
     /// `<dir>/credential.json`, a plain file like the live one. FakeAgent keeps no item, so it
     /// names nothing after the spelling (Decision 19).
     fn read_profile_credential(&self, env: &Env, dir: &Path, _spelling: &str) -> Read<Credential> {
         self.read_live_auth(&profile_env_in(env, dir)).credential
     }
@@ impl Provider for FakeAgent {
             Read::Present(_) => Validity::Invalid("logged in as another identity".into()),
             Read::Absent => Validity::Invalid("not logged in".into()),
             Read::Unreadable(e) => Validity::Unknown(e.to_string()),
         }
     }
+
+    /// `<dir>/transcripts/*.jsonl`, nothing deeper.
+    fn transcript_files(&self, env: &Env) -> io::Result<Vec<PathBuf>> {
+        transcripts::discover(&FakePaths::resolve(env).transcripts)
+    }
+
+    fn parse_usage_line(&self, line: &[u8]) -> Result<Option<UsageLine>, LineError> {
+        transcripts::parse_line(line)
+    }
+
+    fn price_table(&self) -> &'static PriceTable {
+        &transcripts::PRICES
+    }
 }
```

- [ ] **Step 4: Run them and see them pass**

```
cargo test -p tagteam-fake --locked
```

Expected: every test passes, among them the 5 of `tests/transcripts.rs` (`round_trips_a_usage_line`, `other_types_are_none`, `discovers_only_jsonl`, `appending_writes_text_as_it_is`, `its_records_prices_and_capability`) and `kinds_capabilities_endpoints_and_surface`.

- [ ] **Step 5: Lint**

```
cargo fmt --all --check
cargo clippy -p tagteam-fake --all-targets --locked -- -D warnings
```

Expected: no output from fmt; clippy finishes with no warning.

- [ ] **Step 6: Commit**

```
git add crates/tagteam-fake/src/transcripts.rs crates/tagteam-fake/src/paths.rs crates/tagteam-fake/src/lib.rs crates/tagteam-fake/src/provider.rs crates/tagteam-fake/tests/transcripts.rs crates/tagteam-fake/tests/provider.rs
git commit -m "Give the fake provider transcripts"
```

#### Notes for the controller

- Beside the outline, and outside the contract, which names no `tagteam_fake` API: `FakeUsage` mirrors `UsageLine`'s field names (so Task 15's writer can guess them) and has `usage_line()` to build the expected parse; `append_transcript(env, name, text)` writes test transcripts, including partial and malformed lines; `FakeAgent::without_transcripts()` exists because Task 15's `a_provider_without_transcripts_is_refused` needs a registered provider without the capability, and after Tasks 8 and 10 neither real nor fake provider lacks it. If Tasks 15–18 were written against other names, align them to these (or rename here) before batch E runs.
- FakeAgent's `share_policy` is unchanged: `transcripts` is neither shared nor private, so it behaves like any unlisted entry. Its transcripts are always read from the home `FakePaths::resolve(env)` names, as CC's are from `<config_home>/projects`.
- The fake crate has no `serde` dependency and gains none: the parser reads a `serde_json::Value`, deliberately unlike Claude Code's.
- Verified by the plan writer on a copy of the branch with contract-exact stubs of Tasks 2-3: Steps 2, 4 and 5 behave as stated on macOS, and the whole workspace (`cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`) is clean with Tasks 7–10 applied; `tagteam-engine`'s tests all pass with them.

---

### Task 11: Migration step 3

**Depends on:** none
**Files:**
- Create: `crates/tagteam-engine/src/store/schema_v3.sql` (every table, index and trigger of spec §4; `token_epoch` with `seeded` per ruling R1; `token_usage` keyed per ruling R7 and its `model` normalized into `token_models` per ruling R8, see Notes)
- Modify: `crates/tagteam-engine/src/store/mod.rs` (`MIGRATION_V3`, `MIGRATIONS: [&str; 3]`, `TOKENS_STEP`, `SCHEMA_VERSION = 3`, `SeedLaunch`, `Epoch`, `seed_tokens_step`, `Store::open_seeded(path, now, seed)`, `Store::open_existing_seeded(path, now, seed)`, `migrate(epoch)`)
- Modify: `crates/tagteam-engine/src/engine.rs` (`store()` and `existing_store()` open through `open_seeded` / `open_existing_seeded` with `&|| self.now_ms()` and `seed_launches`, which lists the profiles under `data_dir/sessions/*`, or says it could not)
- Modify: `crates/tagteam-engine/tests/store.rs` (version asserts 2→3, the newer-schema test 3→4, the v1 test's comment, ten new tests)
- Modify: `crates/tagteam-engine/tests/purge.rs` (line 245: `schema_version()` 2→3)
- Test: `crates/tagteam-engine/tests/store.rs`

Every test that pins the schema version or counts tables (searched: `rg -n "schema_version|user_version|SCHEMA_VERSION|sqlite_master" crates`):

| File:line | What | Change |
|---|---|---|
| `crates/tagteam-engine/tests/store.rs:30,33` | `opening_migrates_once_and_open_existing_never_creates` | 2 → 3 |
| `crates/tagteam-engine/tests/store.rs:223,227,231,241` | `a_newer_schema_version_is_refused_and_the_database_is_untouched` writes v3 as "newer" | v3 → v4, `UnsupportedSchema(3)` → `(4)` |
| `crates/tagteam-engine/tests/store.rs:260` | comment "(v2)" | "(v3)" |
| `crates/tagteam-engine/tests/store.rs:315` | `concurrent_first_opens_all_succeed` | 2 → 3 |
| `crates/tagteam-engine/tests/store.rs:749,775,799` | `a_version_1_store_is_migrated_with_its_activation_epochs_filled` | comment, 2 → 3 twice |
| `crates/tagteam-engine/tests/store.rs:1253,1262` | `emptying_the_store_keeps_its_schema_and_leaves_no_row` | `tables.len() >= 12` still holds (21 tables now); 2 → 3 |
| `crates/tagteam-engine/tests/purge.rs:245` | full purge leaves a store | 2 → 3 |
| `crates/tagteam-engine/tests/doctor.rs:223,234` | sets 99 and 1 | unchanged (relative to `SCHEMA_VERSION`) |

No test in `crates/tagteam/tests` pins the version (`doctor`'s text uses `SCHEMA_VERSION`).

**Interfaces:**
- Consumes: nothing from earlier tasks. Existing: `Engine::session_state_at(&self, p: &dyn Provider, profile: &Path) -> SessionState` (crate-private, `session.rs`), `SessionState::owned(&self) -> bool`, `ProfileMarker::read(&Path) -> Read<ProfileMarker>`, `ProviderRegistry::all(&self) -> &[Arc<dyn Provider>]`.
- Produces (contract, "Store (Tasks 11–12)"):
  ```rust
  pub const SCHEMA_VERSION: i64 = 3;
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct SeedLaunch { pub provider: ProviderId, pub account_id: String, pub started_at_ms: i64, pub in_use: bool }
  impl Store {
      pub fn open(path: &Path) -> Result<Self, StoreError>;  // a migration to 3: epoch = system clock, token_epoch.seeded = 0
      /// A migration to 3 reads `now` once, after its IMMEDIATE transaction holds the write lock, and records the
      /// epoch at that value, with the seeded active_history rows at that same instant (the engine passes
      /// `&|| self.now_ms()`). `now` and `seed` run only then. `None` from `seed` records seeded = 0.
      pub fn open_seeded(path: &Path, now: &dyn Fn() -> i64, seed: &dyn Fn() -> Option<Vec<SeedLaunch>>) -> Result<Self, StoreError>;
      pub fn open_existing_seeded(path: &Path, now: &dyn Fn() -> i64, seed: &dyn Fn() -> Option<Vec<SeedLaunch>>) -> Result<Option<Self>, StoreError>;
  }
  ```
  With a `FakeClock` engine, the epoch is the fixture's now (`Fx`: 1_790_000_000_000): a test places a response before or after the epoch by its time alone. Reading the clock under the lock means a switch another process commits while this open waits for the lock is never dated after the epoch whose history records it. The triggers keep stamping `active_history` rows with SQLite's wall clock (spec §4's `<now ms>`).
  and the tables Task 12 reads and writes: `token_files`, `token_sessions`, `token_models`, `token_usage` (`id` rowid, `key` UNIQUE, `model` → `token_models.id`), `session_sightings`, `run_launches`, `active_history` (filled by the two triggers on `active_accounts`), `token_state`, `token_epoch(at, seeded)`.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/store.rs`, replace the imports at the top of the file:

```rust
mod common;

use common::{add, cc, identity};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use tagteam_core::autoswitch::{AutoState, Departure, Trigger};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::store::{
    Activation, AutoRecord, DisplacedRow, EventRow, JournalRow, LoginMeta, NewAccount, Store,
    StoreError,
};
```

with:

```rust
mod common;

use common::{Fx, add, cc, identity};
use serde_json::json;
use std::cell::Cell;
use std::os::unix::fs::PermissionsExt;
use std::time::{SystemTime, UNIX_EPOCH};
use tagteam_core::autoswitch::{AutoState, Departure, Trigger};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::store::{
    Activation, AutoRecord, DisplacedRow, EventRow, JournalRow, LoginMeta, NewAccount, SeedLaunch,
    Store, StoreError,
};
```

Update the version asserts (each hunk is the whole change):

```rust
// opening_migrates_once_and_open_existing_never_creates
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), 3);
    drop(s);
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), 3);
```

```rust
// a_newer_schema_version_is_refused_and_the_database_is_untouched
        // A file as a hypothetical newer tagteam would leave it: schema v4, still in
```
```rust
        conn.execute_batch("CREATE TABLE accounts (id TEXT PRIMARY KEY); PRAGMA user_version = 4;")
            .unwrap();
    }
    assert!(matches!(
        Store::open(&path),
        Err(StoreError::UnsupportedSchema(4))
    ));
```
```rust
    assert_eq!(version, 4);
    assert_eq!(journal_mode(&path), "delete");
```

```rust
// every_open_path_leaves_the_store_in_wal_mode
    // Reopening an already-migrated (v3) file still calls through to set WAL; a no-op there.
```

```rust
// concurrent_first_opens_all_succeed
            assert_eq!(h.join().unwrap().unwrap(), 3);
```

```rust
// a_version_1_store_is_migrated_with_its_activation_epochs_filled: the comment, then both asserts
    // Decision 1: `schema.sql` is the frozen v1 DDL, so a v1 file runs only the steps above
    // it. The backfill takes the live store as current (§12.5): each named account's
    // activation epoch is its current `login_epoch`. A journal row keeps a NULL `to_epoch`
    // (§9.6).
```
```rust
    assert_eq!(s.schema_version().unwrap(), 3);
    let a = AccountId::from_string("a");
```
```rust
    // Reopening runs nothing more: the file is already at the build's version.
    assert_eq!(Store::open(&path).unwrap().schema_version().unwrap(), 3);
```

```rust
// emptying_the_store_keeps_its_schema_and_leaves_no_row
    assert_eq!(s.schema_version().unwrap(), 3);
    assert!(other.all_accounts().unwrap().is_empty());
```

In `crates/tagteam-engine/tests/purge.rs` (line 245):

```rust
    assert_eq!(store.schema_version().unwrap(), 3);
```

Append to `crates/tagteam-engine/tests/store.rs`:

```rust
// ---- Token report Task 11: migration step 3 (token report spec §4) ----

/// The DDL that version 2 adds to `schema.sql`, as `MIGRATION_V2` holds it.
const MIGRATION_V2_DDL: &str = "ALTER TABLE active_accounts ADD COLUMN login_epoch INTEGER;
ALTER TABLE switch_journal ADD COLUMN to_epoch INTEGER;";

/// A version-2 store at `path`, as the release before the token report leaves it: two
/// claude-code accounts with `a` active, and fake-agent with no active account.
fn version_2_store(path: &std::path::Path) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(include_str!("../src/store/schema.sql"))
        .unwrap();
    conn.execute_batch(MIGRATION_V2_DDL).unwrap();
    conn.execute_batch(
        "INSERT INTO accounts
           (id, provider, position, identity_key, label, kind, identity_json, login_epoch, added_at)
           VALUES ('a', 'claude-code', 1, 'a@x.co', 'a@x.co', 'oauth', '{}', 3, 1),
                  ('b', 'claude-code', 2, 'b@x.co', 'b@x.co', 'oauth', '{}', 0, 1);
         INSERT INTO active_accounts (provider, account_id, login_epoch)
           VALUES ('claude-code', 'a', 3), ('fake-agent', NULL, NULL);
         PRAGMA user_version = 2;",
    )
    .unwrap();
}

/// No session profile: what a seed closure lists on a machine that never ran `run`.
fn no_profiles() -> Option<Vec<SeedLaunch>> {
    Some(Vec::new())
}

/// The epoch the seeded opens below record: the fixtures' clock (`Fx`'s `FakeClock`).
const NOW: i64 = 1_790_000_000_000;

/// The wall clock in epoch ms, which a raw open and the triggers read (token report spec §4).
fn wall_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// The file's `user_version`, read without migrating it.
fn raw_version(path: &std::path::Path) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}

fn has_table(path: &std::path::Path, name: &str) -> bool {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [name],
            |r| r.get(0),
        )
        .unwrap()
}

/// `token_epoch`: `(at, seeded)` per row.
fn epoch_rows(path: &std::path::Path) -> Vec<(i64, i64)> {
    rusqlite::Connection::open(path)
        .unwrap()
        .prepare("SELECT at, seeded FROM token_epoch ORDER BY rowid")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// `active_history`: `(provider, at, account_id, recorded_at)` per row, in insertion order.
fn history_rows(path: &std::path::Path) -> Vec<(String, i64, Option<String>, i64)> {
    rusqlite::Connection::open(path)
        .unwrap()
        .prepare("SELECT provider, at, account_id, recorded_at FROM active_history ORDER BY rowid")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

type LaunchCols = (String, String, i64, Option<i64>, Option<i64>, i64);

/// `run_launches`: `(provider, account_id, started_at, ended_at, pid, recorded_at)` per row,
/// by provider and account.
fn launch_rows(path: &std::path::Path) -> Vec<LaunchCols> {
    rusqlite::Connection::open(path)
        .unwrap()
        .prepare(
            "SELECT provider, account_id, started_at, ended_at, pid, recorded_at FROM run_launches \
             ORDER BY provider, account_id",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn launch(account: &str, started: i64, ended: Option<i64>, recorded: i64) -> LaunchCols {
    (
        "claude-code".into(),
        account.into(),
        started,
        ended,
        None,
        recorded,
    )
}

#[test]
fn a_version_2_store_migrates_to_3_with_epoch_and_active_history() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    version_2_store(&path);
    let s = Store::open_seeded(&path, &|| NOW, &no_profiles).unwrap();
    assert_eq!(s.schema_version().unwrap(), 3);
    assert_eq!(
        s.activation(&cc()).unwrap(),
        Some(Activation {
            account: AccountId::from_string("a"),
            epoch: Some(3)
        }),
        "version 2's rows are kept"
    );
    drop(s);

    assert_eq!(
        epoch_rows(&path),
        [(NOW, 1)],
        "the epoch is the caller's clock"
    );
    // The account active at the epoch, per provider, at the epoch itself (spec §7.3).
    assert_eq!(
        history_rows(&path),
        [
            ("claude-code".to_owned(), NOW, Some("a".to_owned()), NOW),
            ("fake-agent".to_owned(), NOW, None, NOW),
        ]
    );
    assert!(launch_rows(&path).is_empty(), "no profile to seed");
    for table in [
        "token_files",
        "token_sessions",
        "token_models",
        "token_usage",
        "session_sightings",
        "token_state",
    ] {
        assert!(has_table(&path, table), "{table}");
    }
    // Reopening runs nothing more, and never asks for profiles.
    let s = Store::open_seeded(&path, &|| NOW + 1, &|| panic!("step 3 ran twice")).unwrap();
    assert_eq!(s.schema_version().unwrap(), 3);
    drop(s);
    assert_eq!(epoch_rows(&path), [(NOW, 1)]);
    assert_eq!(history_rows(&path).len(), 2);
}

#[test]
fn seeding_records_profiles_idle_closed_in_use_open() {
    // Spec §4 Seeding: an idle profile's launch ends at the epoch; one in use stays open for
    // §6.1 to close. Its birth time is its start, 0 when the filesystem reports none.
    let seed = || {
        Some(vec![
            SeedLaunch {
                provider: cc(),
                account_id: "idle".into(),
                started_at_ms: 1_700_000_000_000,
                in_use: false,
            },
            SeedLaunch {
                provider: cc(),
                account_id: "busy".into(),
                started_at_ms: 0,
                in_use: true,
            },
        ])
    };
    let d = tempfile::tempdir().unwrap();
    for (name, older) in [("v2.db", true), ("fresh.db", false)] {
        let path = d.path().join(name);
        if older {
            version_2_store(&path);
        }
        Store::open_seeded(&path, &|| NOW, &seed).unwrap();
        assert_eq!(epoch_rows(&path), [(NOW, 1)], "{name}");
        assert_eq!(
            launch_rows(&path),
            [
                launch("busy", 0, None, NOW),
                launch("idle", 1_700_000_000_000, Some(NOW), NOW),
            ],
            "{name}"
        );
    }
}

#[test]
fn a_raw_open_records_unseeded() {
    // Ruling R1: an open that lists no profile cannot say pre-epoch coverage is complete. A
    // raw open takes the system clock as the epoch.
    let d = tempfile::tempdir().unwrap();
    let fresh = d.path().join("fresh.db");
    let before = wall_ms();
    Store::open(&fresh).unwrap();
    let older = d.path().join("v2.db");
    version_2_store(&older);
    Store::open_existing(&older).unwrap().unwrap();
    let after = wall_ms();
    for path in [fresh, older] {
        let [(at, 0)] = epoch_rows(&path)[..] else {
            panic!("{}: {:?}", path.display(), epoch_rows(&path));
        };
        assert!((before..=after).contains(&at), "{before} {at} {after}");
        assert!(launch_rows(&path).is_empty());
    }
}

#[test]
fn a_seed_that_cannot_list_every_profile_records_unseeded() {
    // Ruling R1: a profile may have gone unlisted, so pre-epoch coverage is not complete.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    version_2_store(&path);
    Store::open_seeded(&path, &|| NOW, &|| None).unwrap();
    assert_eq!(epoch_rows(&path), [(NOW, 0)]);
    assert!(launch_rows(&path).is_empty());
    assert_eq!(
        history_rows(&path).len(),
        2,
        "the active account is still recorded"
    );
}

#[test]
fn the_seed_closure_runs_only_when_step_3_applies() {
    // The clock too: it dates the epoch, and step 3 alone records one.
    let calls = Cell::new(0);
    let seed = || {
        calls.set(calls.get() + 1);
        Some(Vec::new())
    };
    let reads = Cell::new(0);
    let clock = || {
        reads.set(reads.get() + 1);
        NOW
    };
    let d = tempfile::tempdir().unwrap();
    let missing = d.path().join("missing.db");
    assert!(
        Store::open_existing_seeded(&missing, &clock, &seed)
            .unwrap()
            .is_none()
    );
    assert_eq!((calls.get(), reads.get()), (0, 0), "no store, no migration");

    let fresh = d.path().join("fresh.db");
    Store::open_seeded(&fresh, &clock, &seed).unwrap();
    assert_eq!(
        (calls.get(), reads.get()),
        (1, 1),
        "a fresh store runs every step"
    );
    Store::open_seeded(&fresh, &clock, &seed).unwrap();
    Store::open_existing_seeded(&fresh, &clock, &seed)
        .unwrap()
        .unwrap();
    assert_eq!(
        (calls.get(), reads.get()),
        (1, 1),
        "a version-3 store runs none"
    );

    let older = d.path().join("v2.db");
    version_2_store(&older);
    Store::open_existing_seeded(&older, &clock, &seed)
        .unwrap()
        .unwrap();
    assert_eq!(
        (calls.get(), reads.get()),
        (2, 2),
        "a version-2 store runs step 3"
    );
    Store::open_seeded(&older, &clock, &seed).unwrap();
    assert_eq!((calls.get(), reads.get()), (2, 2));
}

#[test]
fn the_epoch_is_read_once_the_migration_holds_the_write_lock() {
    // An older process may switch accounts while this one waits for the write lock: the epoch,
    // and the active account the seed records as of it, are read only once the lock is held.
    // The clock probes the lock itself: while the migration's IMMEDIATE transaction is open, a
    // second connection cannot begin one, even with no wait.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    version_2_store(&path);
    let probes = std::cell::RefCell::new(Vec::new());
    Store::open_seeded(
        &path,
        &|| {
            let probe = rusqlite::Connection::open(&path).unwrap();
            probe.busy_timeout(std::time::Duration::ZERO).unwrap();
            let busy = matches!(
                probe.execute_batch("BEGIN IMMEDIATE; ROLLBACK;"),
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == rusqlite::ErrorCode::DatabaseBusy
            );
            probes.borrow_mut().push(busy);
            NOW
        },
        &no_profiles,
    )
    .unwrap();
    assert_eq!(
        *probes.borrow(),
        [true],
        "the clock is read once, with the migration's write lock held"
    );
}

#[test]
fn active_history_triggers_on_switch_add_and_remove() {
    // Spec §4: every change of the active account, and only a change, adds a row at SQLite's
    // now; a removal that clears the row through `ON DELETE SET NULL` records no account.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    assert!(
        history_rows(&path).is_empty(),
        "nothing was active at the epoch"
    );
    let before = wall_ms();
    s.set_active(&cc(), Some(&a), Some(0)).unwrap(); // the first account: an insert
    s.set_active(&cc(), Some(&a), Some(1)).unwrap(); // the same account: no change
    s.commit_switch(&cc(), &b, 0, &switch_event_to(&b), None)
        .unwrap(); // a switch: an update
    s.delete_account(&b).unwrap(); // a removal: SET NULL
    s.set_active(&cc(), Some(&a), Some(0)).unwrap(); // from none to a
    let after = wall_ms();
    let rows = history_rows(&path);
    assert_eq!(
        rows.iter()
            .map(|(p, _, account, _)| (p.as_str(), account.as_deref()))
            .collect::<Vec<_>>(),
        [
            ("claude-code", Some("a")),
            ("claude-code", Some("b")),
            ("claude-code", None),
            ("claude-code", Some("a")),
        ]
    );
    for (_, at, _, recorded_at) in &rows {
        assert!(
            (before - 1..=after + 1).contains(at),
            "{before} {at} {after}"
        );
        assert_eq!(at, recorded_at);
    }
}

#[test]
fn a_failed_seed_rolls_back_the_whole_migration() {
    // Spec §4: step 3 lands with the version or not at all.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    version_2_store(&path);
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Store::open_seeded(&path, &|| NOW, &|| panic!("the seed failed"))
    }));
    assert!(failed.is_err());
    assert_eq!(raw_version(&path), 2);
    assert!(!has_table(&path, "token_epoch"));
    assert!(!has_table(&path, "active_history"));

    // A step whose SQL fails takes the rest with it, before any profile is listed.
    let blocked = d.path().join("blocked.db");
    version_2_store(&blocked);
    rusqlite::Connection::open(&blocked)
        .unwrap()
        .execute_batch("CREATE TABLE token_files (x);")
        .unwrap();
    let calls = Cell::new(0);
    let err = Store::open_seeded(&blocked, &|| NOW, &|| {
        calls.set(calls.get() + 1);
        Some(Vec::new())
    })
    .err()
    .expect("refused");
    assert!(matches!(err, StoreError::Sqlite(_)), "{err:?}");
    assert_eq!(calls.get(), 0);
    assert_eq!(raw_version(&blocked), 2);
    assert!(!has_table(&blocked, "token_epoch"));

    // The first store migrates once its seed succeeds.
    let s = Store::open_seeded(&path, &|| NOW, &no_profiles).unwrap();
    assert_eq!(s.schema_version().unwrap(), 3);
}

/// `dir`'s birth time in epoch ms, or 0 where the filesystem reports none.
fn born_ms(dir: &std::path::Path) -> i64 {
    std::fs::metadata(dir)
        .unwrap()
        .created()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

#[test]
fn an_engine_opened_store_is_seeded() {
    // Spec §4 Seeding through the engine: each directory under `sessions/`, of the provider its
    // marker names or, with none, of each provider with sessions; in use as §12.6 judges it.
    let fx = Fx::new();
    let idle = fx.make_profile_for(fx.cc.as_ref(), &AccountId::from_string("idle"));
    let busy = fx.make_profile_for(fx.cc.as_ref(), &AccountId::from_string("busy"));
    fx.live_record(&busy, 4242, "interactive");
    let bare = fx.env.data_dir().join("sessions/bare");
    std::fs::create_dir_all(&bare).unwrap();
    std::fs::write(fx.env.data_dir().join("sessions/notes.txt"), "x").unwrap();
    let path = fx.env.data_dir().join("tagteam.db");
    version_2_store(&path);

    let store = fx.engine.existing_store().unwrap().expect("a store");
    assert_eq!(store.schema_version().unwrap(), 3);
    drop(store);
    let at = fx.engine.now_ms();
    assert_eq!(
        epoch_rows(&path),
        [(at, 1)],
        "the epoch is the engine's clock"
    );
    assert_eq!(
        launch_rows(&path),
        [
            launch("bare", born_ms(&bare), Some(at), at),
            launch("busy", born_ms(&busy), None, at),
            launch("idle", born_ms(&idle), Some(at), at),
        ]
    );

    // A fresh store is seeded the same way.
    let fresh = Fx::new();
    let only = fresh.make_profile_for(fresh.cc.as_ref(), &AccountId::from_string("only"));
    fresh.engine.store().unwrap();
    let fresh_path = fresh.env.data_dir().join("tagteam.db");
    let at = fresh.engine.now_ms();
    assert_eq!(epoch_rows(&fresh_path), [(at, 1)]);
    assert_eq!(
        launch_rows(&fresh_path),
        [launch("only", born_ms(&only), Some(at), at)]
    );
}

#[test]
fn an_engine_that_cannot_list_the_profiles_records_unseeded() {
    // Ruling R1: `sessions/` exists but cannot be listed, so a profile may have gone unseeded.
    // Root reads it whatever its mode, so the case cannot be made as root.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let fx = Fx::new();
    fx.make_profile_for(fx.cc.as_ref(), &AccountId::from_string("hidden"));
    let sessions = fx.env.data_dir().join("sessions");
    std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o000)).unwrap();
    let opened = fx.engine.store().map(drop);
    std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o700)).unwrap();
    opened.unwrap();
    let path = fx.env.data_dir().join("tagteam.db");
    assert_eq!(epoch_rows(&path), [(fx.engine.now_ms(), 0)]);
    assert!(launch_rows(&path).is_empty());
}
```

- [ ] **Step 2: Run them and see them fail**

```
cargo test -p tagteam-engine --locked --test store --test purge
```

Expected: `store.rs` does not compile:

```
error[E0432]: unresolved import `tagteam_engine::store::SeedLaunch`
error[E0599]: no function or associated item named `open_seeded` found for struct `Store` in the current scope
error[E0599]: no function or associated item named `open_existing_seeded` found for struct `Store` in the current scope
```

and `purge.rs` fails `a_full_purge_deletes_everything_but_the_settings_the_locks_and_the_live_login` (line 245) with `left: 2` and `right: 3`.

- [ ] **Step 3: Implement**

Create `crates/tagteam-engine/src/store/schema_v3.sql`:

```sql
CREATE TABLE token_files (
  path       TEXT PRIMARY KEY,
  provider   TEXT NOT NULL,
  dev        INTEGER NOT NULL,
  ino        INTEGER NOT NULL,
  size       INTEGER NOT NULL,
  mtime_ns   INTEGER NOT NULL,
  offset     INTEGER NOT NULL,
  head_hash  BLOB NOT NULL,
  tail_hash  BLOB NOT NULL,
  indexed_at INTEGER NOT NULL
);

CREATE TABLE token_sessions (
  id         INTEGER PRIMARY KEY,
  provider   TEXT NOT NULL,
  session_id TEXT NOT NULL,
  project    TEXT,
  UNIQUE (provider, session_id)
);

CREATE TABLE token_models (
  id       INTEGER PRIMARY KEY,
  provider TEXT NOT NULL,
  name     TEXT NOT NULL,
  UNIQUE (provider, name)
);

-- `id` follows insertion, so the rows of a time range share pages; `key` is the response's
-- identity (ruling R7).
CREATE TABLE token_usage (
  id         INTEGER PRIMARY KEY,
  key        INTEGER NOT NULL UNIQUE,
  session    INTEGER NOT NULL REFERENCES token_sessions(id) ON DELETE CASCADE,
  at         INTEGER NOT NULL,
  model      INTEGER NOT NULL REFERENCES token_models(id),
  sidechain  INTEGER NOT NULL,
  input      INTEGER NOT NULL,
  output     INTEGER NOT NULL,
  cw_5m      INTEGER NOT NULL,
  cw_1h      INTEGER NOT NULL,
  cache_read INTEGER NOT NULL,
  web_search INTEGER NOT NULL,
  speed      TEXT,
  finalized  INTEGER NOT NULL
);
CREATE INDEX token_usage_by_at ON token_usage (at);
CREATE INDEX token_usage_by_session ON token_usage (session, at);

CREATE TABLE session_sightings (
  provider    TEXT NOT NULL,
  session_id  TEXT NOT NULL,
  home        TEXT NOT NULL,
  first_at    INTEGER NOT NULL,
  last_at     INTEGER NOT NULL,
  source      TEXT NOT NULL,
  recorded_at INTEGER NOT NULL,
  PRIMARY KEY (provider, session_id, home, first_at)
) WITHOUT ROWID;

CREATE TABLE run_launches (
  provider    TEXT NOT NULL,
  account_id  TEXT NOT NULL,
  started_at  INTEGER NOT NULL,
  ended_at    INTEGER,
  pid         INTEGER,
  recorded_at INTEGER NOT NULL,
  PRIMARY KEY (provider, account_id, started_at)
) WITHOUT ROWID;

CREATE TABLE active_history (
  provider    TEXT NOT NULL,
  at          INTEGER NOT NULL,
  account_id  TEXT,
  recorded_at INTEGER NOT NULL
);
CREATE INDEX active_history_by_at ON active_history (provider, at);

CREATE TRIGGER active_history_insert AFTER INSERT ON active_accounts BEGIN
  INSERT INTO active_history (provider, at, account_id, recorded_at) VALUES (
    NEW.provider,
    CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER),
    NEW.account_id,
    CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
  );
END;

CREATE TRIGGER active_history_update AFTER UPDATE OF account_id ON active_accounts
  WHEN NEW.account_id IS NOT OLD.account_id BEGIN
  INSERT INTO active_history (provider, at, account_id, recorded_at) VALUES (
    NEW.provider,
    CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER),
    NEW.account_id,
    CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
  );
END;

CREATE TABLE token_state (
  provider          TEXT PRIMARY KEY,
  refreshed_at      INTEGER,
  changed_at        INTEGER,
  files             INTEGER NOT NULL DEFAULT 0,
  responses         INTEGER NOT NULL DEFAULT 0,
  skipped_lines     INTEGER NOT NULL DEFAULT 0,
  unreadable_files  INTEGER NOT NULL DEFAULT 0,
  pruned_at         INTEGER
);

CREATE TABLE token_epoch (
  at     INTEGER NOT NULL,
  seeded INTEGER NOT NULL
);
```

SQLite facts this relies on, each verified against the bundled SQLite by the tests above: an upsert's `DO UPDATE` fires `AFTER UPDATE OF account_id` and not `AFTER INSERT`; an `ON DELETE SET NULL` action fires `AFTER UPDATE OF account_id`; `UPDATE active_accounts SET login_epoch = …` fires neither; `julianday('now')` is stable within one statement, so `at` and `recorded_at` of a trigger row are equal.

In `crates/tagteam-engine/src/store/mod.rs`, the import

```rust
use tagteam_provider::{Identity, ProcessStamp};
```

becomes

```rust
use tagteam_provider::{Clock, Identity, ProcessStamp, SystemClock};
```

and replace:

```rust
/// The migration ladder: step `n` takes a file from version `n` to `n + 1`. A fresh file runs
/// every step, and an older one only the steps above its version.
const MIGRATIONS: [&str; 2] = [SCHEMA_V1, MIGRATION_V2];

/// The `PRAGMA user_version` this build knows how to read and write. A stored version above
/// this is a store written by a newer tagteam; `migrate` refuses it rather than guessing.
pub const SCHEMA_VERSION: i64 = 2;
```

with:

```rust
/// Version 3 (token report spec §4): the token index, the session evidence and the epoch.
/// `migrate` follows it, in the same transaction, with `seed_tokens_step`.
const MIGRATION_V3: &str = include_str!("schema_v3.sql");

/// The migration ladder: step `n` takes a file from version `n` to `n + 1`. A fresh file runs
/// every step, and an older one only the steps above its version.
const MIGRATIONS: [&str; 3] = [SCHEMA_V1, MIGRATION_V2, MIGRATION_V3];

/// The step of `MIGRATIONS` that adds the token tables, which `seed_tokens_step` completes.
const TOKENS_STEP: usize = 2;

/// The `PRAGMA user_version` this build knows how to read and write. A stored version above
/// this is a store written by a newer tagteam; `migrate` refuses it rather than guessing.
pub const SCHEMA_VERSION: i64 = 3;
```

Insert directly before `#[derive(Debug, Clone, PartialEq)]\npub struct AccountRow {`:

```rust
/// A session profile found when the store migrates to version 3 (token report spec §4
/// Seeding): its directory's name is the account, `started_at_ms` its birth time (0 when the
/// filesystem reports none), and `in_use` whether a session owns it (core §12.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedLaunch {
    pub provider: ProviderId,
    pub account_id: String,
    pub started_at_ms: i64,
    pub in_use: bool,
}

/// The session profiles to seed: `None` when they cannot all be listed.
type Seed<'a> = &'a dyn Fn() -> Option<Vec<SeedLaunch>>;

/// What a migration to version 3 records (token report spec §4): the clock that dates the
/// epoch, and the profiles to seed, both asked only when step 3 runs. A raw `Store::open` has
/// no seed.
struct Epoch<'a> {
    now: &'a dyn Fn() -> i64,
    seed: Option<Seed<'a>>,
}

/// The rows step 3 adds beside its SQL, in the ladder's transaction (token report spec §4):
/// the epoch, the active account of each provider as of that same instant, and one launch per
/// listed profile, closed at the epoch unless it is in use. The clock is read once, here,
/// under the transaction's write lock, so no switch another process commits while this one
/// waits for the lock falls after the epoch the seeded history dates. Unless every profile
/// was listed, the epoch records `seeded = 0` (ruling R1).
fn seed_tokens_step(c: &Connection, epoch: &Epoch<'_>) -> Result<(), StoreError> {
    let now = (epoch.now)();
    let launches = epoch.seed.and_then(|seed| seed());
    c.execute(
        "INSERT INTO token_epoch (at, seeded) VALUES (?1, ?2)",
        params![now, launches.is_some()],
    )?;
    c.execute(
        "INSERT INTO active_history (provider, at, account_id, recorded_at) \
         SELECT provider, ?1, account_id, ?1 FROM active_accounts",
        [now],
    )?;
    let mut insert = c.prepare(
        "INSERT OR IGNORE INTO run_launches \
         (provider, account_id, started_at, ended_at, pid, recorded_at) \
         VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
    )?;
    for launch in launches.unwrap_or_default() {
        insert.execute(params![
            launch.provider.as_str(),
            launch.account_id,
            launch.started_at_ms,
            (!launch.in_use).then_some(now),
            now,
        ])?;
    }
    Ok(())
}

```

Replace the start of `impl Store` (the whole of `open`):

```rust
impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            ensure_private_dir(dir)?;
        }
        create_private(path)?;
        let conn = connect(path, true)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }
```

with:

```rust
impl Store {
    /// `open_seeded` without a seed: a migration to version 3 takes the system clock as the
    /// epoch, lists no profile and records `token_epoch.seeded = 0` (ruling R1). The engine
    /// always seeds.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Self::create_and_open(
            path,
            &Epoch {
                now: &|| SystemClock.now_ms(),
                seed: None,
            },
        )
    }

    /// Opens the store, creating it (and its directory, 0700) when needed. When this open
    /// applies migration step 3, `now` dates the epoch, read once under the migration's write
    /// lock, and `seed` lists the session profiles inside its transaction (token report spec
    /// §4 Seeding); neither is called otherwise.
    pub fn open_seeded(
        path: &Path,
        now: &dyn Fn() -> i64,
        seed: &dyn Fn() -> Option<Vec<SeedLaunch>>,
    ) -> Result<Self, StoreError> {
        Self::create_and_open(
            path,
            &Epoch {
                now,
                seed: Some(seed),
            },
        )
    }

    fn create_and_open(path: &Path, epoch: &Epoch<'_>) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            ensure_private_dir(dir)?;
        }
        create_private(path)?;
        let conn = connect(path, true)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate(epoch)?;
        Ok(store)
    }
```

In `open_existing`, keep its doc comment and replace its signature line and first statement:

```rust
    pub fn open_existing(path: &Path) -> Result<Option<Self>, StoreError> {
        if !path.try_exists()? {
```

with:

```rust
    pub fn open_existing(path: &Path) -> Result<Option<Self>, StoreError> {
        Self::open_existing_with(
            path,
            &Epoch {
                now: &|| SystemClock.now_ms(),
                seed: None,
            },
        )
    }

    /// `open_existing`, dating the epoch and seeding as `open_seeded` does when the open
    /// applies step 3.
    pub fn open_existing_seeded(
        path: &Path,
        now: &dyn Fn() -> i64,
        seed: &dyn Fn() -> Option<Vec<SeedLaunch>>,
    ) -> Result<Option<Self>, StoreError> {
        Self::open_existing_with(
            path,
            &Epoch {
                now,
                seed: Some(seed),
            },
        )
    }

    fn open_existing_with(path: &Path, epoch: &Epoch<'_>) -> Result<Option<Self>, StoreError> {
        if !path.try_exists()? {
```

and, at the end of that same function body, `store.migrate()?;` becomes:

```rust
        store.migrate(epoch)?;
        Ok(Some(store))
```

In `migrate`, the doc comment's last line and the signature:

```rust
    /// this build ends up refusing must never have been touched at all.
    fn migrate(&self) -> Result<(), StoreError> {
```

become:

```rust
    /// this build ends up refusing must never have been touched at all. Step 3's rows
    /// (`seed_tokens_step`, which reads the clock and calls the seed) land in that same
    /// transaction.
    fn migrate(&self, epoch: &Epoch<'_>) -> Result<(), StoreError> {
```

and the step loop:

```rust
                let done = usize::try_from(version).unwrap_or(0);
                for step in &MIGRATIONS[done..] {
                    tx.execute_batch(step)?;
                }
```

becomes:

```rust
                let done = usize::try_from(version).unwrap_or(0);
                for (n, step) in MIGRATIONS.iter().enumerate().skip(done) {
                    tx.execute_batch(step)?;
                    if n == TOKENS_STEP {
                        seed_tokens_step(&tx, epoch)?;
                    }
                }
```

`params!` and `Connection` are already imported in `mod.rs`.

In `crates/tagteam-engine/src/engine.rs`, replace the first imports:

```rust
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::liveness::ProcessProbe;
use tagteam_provider::process::ProcessSpawner;
use tagteam_provider::profile::RunShell;
```

with:

```rust
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, UNIX_EPOCH};

use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::liveness::ProcessProbe;
use tagteam_provider::process::ProcessSpawner;
use tagteam_provider::profile::{ProfileMarker, RunShell};
```

and `use crate::store::{JournalRow, Store, StoreError};` with:

```rust
use crate::store::{JournalRow, SeedLaunch, Store, StoreError};
```

In `store()`, replace `let s = Arc::new(Store::open(&self.store_path())?);` with:

```rust
        let s = Arc::new(Store::open_seeded(
            &self.store_path(),
            &|| self.now_ms(),
            &|| self.seed_launches(),
        )?);
```

In `existing_store()`, replace `match Store::open_existing(&self.store_path())? {` with:

```rust
        match Store::open_existing_seeded(&self.store_path(), &|| self.now_ms(), &|| {
            self.seed_launches()
        })? {
```

and insert, between `existing_store` and `pub fn provider`:

```rust
    /// Token report spec §4 Seeding, asked only by the migration to version 3: each directory
    /// under `sessions/` as a launch of the provider its marker names, or of every provider
    /// with sessions when it has no readable marker. It started at the directory's birth time,
    /// or 0 when the filesystem reports none, and is in use as core §12.6 judges it. `None`
    /// when `sessions/` or an entry of it cannot be read: a profile may have gone unlisted, so
    /// the epoch records `seeded = 0` (ruling R1). No `sessions/` at all is no profile.
    fn seed_launches(&self) -> Option<Vec<SeedLaunch>> {
        let entries = match fs::read_dir(self.env.data_dir().join("sessions")) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Some(Vec::new()),
            Err(_) => return None,
        };
        let mut seeds = Vec::new();
        for entry in entries {
            let dir = entry.ok()?.path();
            let meta = match fs::metadata(&dir) {
                Ok(meta) => meta,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => return None,
            };
            let Some(account) = dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !meta.is_dir() {
                continue;
            }
            let started_at_ms = meta
                .created()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
            let named = match ProfileMarker::read(&dir) {
                Read::Present(m) => Some(m.provider),
                Read::Absent | Read::Unreadable(_) => None,
            };
            for p in self.registry.all() {
                if !p.capabilities().sessions || named.as_ref().is_some_and(|id| *id != p.id()) {
                    continue;
                }
                seeds.push(SeedLaunch {
                    provider: p.id(),
                    account_id: account.to_owned(),
                    started_at_ms,
                    in_use: self.session_state_at(p.as_ref(), &dir).owned(),
                });
            }
        }
        Some(seeds)
    }

```

`Read` is already imported in `engine.rs` (`use tagteam_provider::{…, Read};`), and `self.now_ms()` is the engine's clock. The closure runs inside the migration's transaction and touches no store, so it cannot deadlock on the engine's store slot or the connection; it reads only tagteam's own `sessions/` tree and the records it judges.

- [ ] **Step 4: Run them and see them pass**

```
cargo test -p tagteam-engine --locked --test store --test purge
```

Expected: `test result: ok. 58 passed; 0 failed` for `store.rs` (48 existing, 10 new) and `test result: ok. 56 passed; 0 failed` for `purge.rs`. `a_failed_seed_rolls_back_the_whole_migration` prints the caught panic's message (`the seed failed`) only when the test fails.

Every engine-made store now runs the seed, so also run the engine crate once:

```
cargo test -p tagteam-engine --locked
```

Expected: every test passes (1,200 with this task alone at the time of planning, all suites `ok`).

- [ ] **Step 5: Lint**

```
cargo fmt --all --check
cargo clippy -p tagteam-engine --all-targets --locked -- -D warnings
```

Expected: no output from `fmt`; clippy finishes with no warning.

- [ ] **Step 6: Commit**

```
git add crates/tagteam-engine/src/store/schema_v3.sql crates/tagteam-engine/src/store/mod.rs crates/tagteam-engine/src/engine.rs crates/tagteam-engine/tests/store.rs crates/tagteam-engine/tests/purge.rs
git commit -m "Add migration step 3 for the token index and session evidence"
```

#### Notes for the controller

1. **Ruling R7 (adopted).** `token_usage` gets `id INTEGER PRIMARY KEY` (insertion order) with `key INTEGER NOT NULL UNIQUE`, instead of spec §4's `key INTEGER PRIMARY KEY`. The key is a random hash, so with it as the rowid a time range reads rows scattered over the whole table. Measured in a release build on a synthetic year at this machine's rate (1.55M responses, 2,000-row batches), raw SQL, the same 30-day range (127k rows):

   | Layout | Bytes/response | 30-day range read | Cold insert of 1.55M |
   |---|---|---|---|
   | spec §4 (`key` is the rowid) | 123 | 180–1,365 ms | 63 s, worst batch 361 ms |
   | R7 (`id` rowid, `key` UNIQUE) | 125 | 18 ms | 37 s, worst batch 236 ms |
   | R7 + R8 (`model` → `token_models`) | 109 | 16 ms | 30 s, worst batch 76 ms |

   The upsert, the merge rule and `UsageRow.key` are unchanged.
2. **Ruling R8 (adopted; inside spec §4 "the plan may normalize `model`").** `token_usage.model` references `token_models(id, provider, name)`. A model's name stays after retention prunes its last response. Spec §4's "eight tables" becomes nine, and §13's "six provider-keyed tables" becomes seven. Task 24 should say so in the core spec amendment.
3. **Clocks.** The epoch and the seeded `active_history` rows share one value, read once from the caller's clock after the migration's `IMMEDIATE` transaction holds the write lock: the engine's clock through `open_seeded` and `open_existing_seeded` (`&|| self.now_ms()`), and the system clock for a raw `Store::open`. Read before the lock, it would backdate the epoch: a switch an older process committed while this one waited would be recorded as active at an instant before it happened (`the_epoch_is_read_once_the_migration_holds_the_write_lock` fails on that, verified by moving the read before the lock). With `Fx`, the epoch is 1_790_000_000_000. The triggers keep SQLite's `julianday('now')` (spec §4), so an `active_history` row a test's switch or `set_active` adds is stamped with the wall clock, which is after any `FakeClock` epoch. A test of Task 16 or 18 that needs a history change at a chosen fixture time still has to write it itself.
4. **Seeding (adopted: the seed can say "unknown").** `seed_launches` returns `None` when `sessions/` exists but cannot be listed, or when an entry or its metadata cannot be read (other than a racing removal); the epoch then records `seeded = 0`. A missing `sessions/` is `Some(vec![])`: no profile, so coverage is complete. A profile directory with no readable marker is seeded for every provider with `sessions`. This only adds coverage, so it can only leave a response unattributed. `an_engine_that_cannot_list_the_profiles_records_unseeded` returns early when run as root, since root reads a 0000 directory.
5. The seed closure runs inside the migration's `IMMEDIATE` transaction: one `read_dir`, a marker read and a session-state judgement per profile, once per store's life. A statusline that happens to be the first command after an upgrade pays it once.

---

### Task 12: Store API for the token tables

**Depends on:** 2, 11
**Files:**
- Create: `crates/tagteam-engine/src/store/tokens.rs` (the store API of the contract's "store/tokens.rs (Task 12)", plus `record_sighting_within` for Task 16)
- Modify: `crates/tagteam-engine/src/store/mod.rs` (`mod tokens;`, its `pub use`, and the token tables in `delete_provider_rows`)
- Modify: `crates/tagteam-engine/src/store/usage.rs` (`TAKE_LEASE_SQL` becomes `pub(super)`, for the prune lease)
- Modify: `crates/tagteam-engine/Cargo.toml` (`sha2.workspace = true`: an existing workspace dependency, already in `Cargo.lock` at 0.10.9)
- Modify: `Cargo.lock` (one line: `"sha2 0.10.9",` in `tagteam-engine`'s dependencies)
- Test: `crates/tagteam-engine/tests/store_tokens.rs` (new)

**Interfaces:**
- Consumes:
  - Task 2: `tagteam_core::tokens::TokenCounts { input, output, cache_write_5m, cache_write_1h, cache_read, web_search: u64 }` (`Copy`, `Default`, `PartialEq`) and `TokenCounts::merge_max(&mut self, other: &TokenCounts)`.
  - Task 11: the version-3 tables (`token_files`, `token_sessions`, `token_models`, `token_usage` with `id` rowid and `key UNIQUE`, `session_sightings`, `run_launches`, `active_history`, `token_state`, `token_epoch(at, seeded)`), `Store::open` (a raw open: epoch at the system clock, `seeded = 0`), and `SeedLaunch` (already `pub` in `store/mod.rs`).
- Produces (contract, "Store (Tasks 11–12)", re-exported from `tagteam_engine::store`):
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct TokenFileRow { pub path: String, pub dev: u64, pub ino: u64, pub size: i64, pub mtime_ns: i64, pub offset: u64, pub head_hash: [u8; 8], pub tail_hash: [u8; 8] }
  #[derive(Debug, Clone, PartialEq)]
  pub struct UsageRow { pub key: i64, pub session_id: String, pub project: Option<String>, pub at_ms: i64, pub model: String, pub sidechain: bool, pub counts: TokenCounts, pub speed: Option<String>, pub finalized: bool }
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
  pub struct BatchOutcome { pub added: u64, pub updated: u64 }
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
  pub struct PruneOutcome { pub deleted: u64, pub complete: bool }
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
  pub struct RefreshStats { pub files: u64, pub responses: u64, pub skipped_lines: u64, pub unreadable_files: u64 }
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct TokenStateRow { pub refreshed_at: Option<i64>, pub changed_at: Option<i64>, pub stats: RefreshStats, pub pruned_at: Option<i64> }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum SightingSource { Run, Scan, Statusline }   // as_str(): 'run' | 'scan' | 'statusline'
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct SightingRow { pub session_id: String, pub home: String, pub first_at: i64, pub last_at: i64, pub source: SightingSource, pub recorded_at: i64 }
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct LaunchRow { pub account_id: String, pub started_at: i64, pub ended_at: Option<i64>, pub pid: Option<u32>, pub recorded_at: i64 }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct EpochRow { pub at: i64, pub seeded: bool }
  pub fn usage_key(provider: &ProviderId, message_id: &str, request_id: &str) -> i64;

  impl Store {
      pub fn token_files(&self, provider: &ProviderId) -> Result<Vec<TokenFileRow>, StoreError>;
      pub fn commit_token_batch(&self, provider: &ProviderId, rows: &[UsageRow], files: &[TokenFileRow], removed: &[String], now_ms: i64) -> Result<BatchOutcome, StoreError>;
      pub fn finish_token_refresh(&self, provider: &ProviderId, stats: &RefreshStats, now_ms: i64) -> Result<(), StoreError>;
      pub fn prune_tokens(&self, provider: &ProviderId, cutoff_ms: i64, holder: &str, now_ms: i64, stop: &dyn Fn() -> bool) -> Result<PruneOutcome, StoreError>;
      #[cfg(feature = "test-hooks")]
      pub fn busy_timeout(&self) -> Result<Duration, StoreError>; // the connection's, for the restoration test
      pub fn token_state(&self, provider: &ProviderId) -> Result<Option<TokenStateRow>, StoreError>;
      pub fn token_epoch(&self) -> Result<Option<EpochRow>, StoreError>;
      pub fn evidence_changed_at(&self, provider: &ProviderId) -> Result<Option<i64>, StoreError>;
      pub fn token_usage(&self, provider: &ProviderId, since_ms: Option<i64>, until_ms: Option<i64>, session: Option<&str>) -> Result<Vec<UsageRow>, StoreError>;
      pub fn token_models(&self, provider: &ProviderId) -> Result<Vec<String>, StoreError>;
      pub fn token_span(&self, provider: &ProviderId) -> Result<(Option<i64>, Option<i64>), StoreError>;
      pub fn record_sighting(&self, provider: &ProviderId, s: &SightingRow, min_gap_ms: i64) -> Result<bool, StoreError>;
      /// Addition for Task 16 (spec §6.3): `record_sighting` with the connection's busy timeout set to `wait`
      /// for the call and restored to `BUSY_TIMEOUT` after it; on a busy store, `Err` (`sqlite_code()` is
      /// `DatabaseBusy`) within about `wait`, and nothing written.
      pub fn record_sighting_within(&self, provider: &ProviderId, s: &SightingRow, min_gap_ms: i64, wait: Duration) -> Result<bool, StoreError>;
      pub fn set_sighting_last_at(&self, provider: &ProviderId, session_id: &str, home: &str, first_at: i64, last_at: i64, now_ms: i64) -> Result<(), StoreError>;
      pub fn latest_sighting(&self, provider: &ProviderId, session_id: &str, home: &str) -> Result<Option<SightingRow>, StoreError>;
      pub fn sightings(&self, provider: &ProviderId) -> Result<Vec<SightingRow>, StoreError>;
      pub fn open_launch(&self, provider: &ProviderId, account_id: &str, started_at: i64, now_ms: i64) -> Result<(), StoreError>;
      pub fn attach_launch_pid(&self, provider: &ProviderId, account_id: &str, started_at: i64, pid: u32, now_ms: i64) -> Result<(), StoreError>;
      pub fn close_launch(&self, provider: &ProviderId, account_id: &str, started_at: i64, ended_at: i64, now_ms: i64) -> Result<(), StoreError>;
      pub fn launches(&self, provider: &ProviderId) -> Result<Vec<LaunchRow>, StoreError>;
      pub fn active_history(&self, provider: &ProviderId) -> Result<Vec<(i64, Option<String>)>, StoreError>;
  }
  ```
  Behaviour later tasks rely on, beyond the contract's doc lines:
  - `commit_token_batch` merges rows of one key within the batch first, writes a response only when the merge changes it, and counts `added` (a key the store did not hold) and `updated` (a stored key whose merged row differs) per distinct key. It also adds `added` to `token_state.responses`.
  - `token_state.responses` is the index's live count, which each batch adds to and each prune subtracts from. `finish_token_refresh` does not write `stats.responses`, so Task 15 may pass any value there (0 is fine).
  - `token_usage` orders by `at`, then by first-indexed order (`id`). `token_models` lists every model the provider's index has held since its last purge, pruned ones included (ruling R8).
  - `prune_tokens` also deletes sightings whose `last_at` is before the cutoff (see Notes). Each deletion chunk (5,000 responses at most) is its own transaction and, when it deletes any, sets `changed_at` and subtracts from `responses` in that same transaction. `stop` is asked before each chunk; a stopped prune releases its lease (`expires_at` set to `now_ms`) and returns `complete: false`, so Task 15 calls it with a closure over the refresh's deadline and cancel and simply calls again on the next refresh. `pruned_at` is set only when a prune completes. A lease another holder took today is `PruneOutcome { deleted: 0, complete: true }`: nothing is due.
  - `record_sighting` with `min_gap_ms = 0` writes whenever `last_at` does not move back. `set_sighting_last_at` only ever moves `last_at` up, and writes nothing otherwise.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-engine/tests/store_tokens.rs`:

```rust
//! The store API of the token index and the session evidence (token report spec §4–§6).

mod common;

use std::cell::Cell;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{add, cc};
use tagteam_core::ProviderId;
use tagteam_core::tokens::TokenCounts;
use tagteam_engine::store::{
    BatchOutcome, EpochRow, LaunchRow, PruneOutcome, RefreshStats, SightingRow, SightingSource,
    Store, TokenFileRow, TokenStateRow, UsageRow, usage_key,
};

/// The fixtures' clock (`Fx`'s `FakeClock`).
const T: i64 = 1_790_000_000_000;
const DAY: i64 = 86_400_000;

fn store() -> (tempfile::TempDir, Store, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    (d, s, path)
}

fn fake() -> ProviderId {
    ProviderId::new("fake-agent")
}

/// One response of session `session` at `at`, with `output` output tokens and fixed input and
/// cache counts.
fn row(key: i64, session: &str, at: i64, output: u64) -> UsageRow {
    UsageRow {
        key,
        session_id: session.into(),
        project: Some("/work/app".into()),
        at_ms: at,
        model: "claude-opus-5-5".into(),
        sidechain: false,
        counts: TokenCounts {
            input: 100,
            output,
            cache_write_5m: 10,
            cache_write_1h: 20,
            cache_read: 30,
            web_search: 0,
        },
        speed: None,
        finalized: false,
    }
}

fn file(path: &str, offset: u64) -> TokenFileRow {
    TokenFileRow {
        path: path.into(),
        dev: 1,
        ino: u64::MAX,
        size: -1,
        mtime_ns: 5,
        offset,
        head_hash: [1; 8],
        tail_hash: [2; 8],
    }
}

fn commit(s: &Store, rows: &[UsageRow], now: i64) -> BatchOutcome {
    s.commit_token_batch(&cc(), rows, &[], &[], now).unwrap()
}

fn all(s: &Store) -> Vec<UsageRow> {
    s.token_usage(&cc(), None, None, None).unwrap()
}

fn keys(rows: &[UsageRow]) -> Vec<i64> {
    rows.iter().map(|r| r.key).collect()
}

fn sighting(session: &str, home: &str, first: i64, last: i64) -> SightingRow {
    SightingRow {
        session_id: session.into(),
        home: home.into(),
        first_at: first,
        last_at: last,
        source: SightingSource::Scan,
        recorded_at: last,
    }
}

fn wall_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// A prune nothing stops.
fn prune(s: &Store, cutoff: i64, holder: &str, now: i64) -> PruneOutcome {
    s.prune_tokens(&cc(), cutoff, holder, now, &|| false)
        .unwrap()
}

fn done(deleted: u64) -> PruneOutcome {
    PruneOutcome {
        deleted,
        complete: true,
    }
}

fn raw_count(path: &std::path::Path, sql: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}

#[test]
fn a_batch_upserts_by_the_merge_rule_in_any_order() {
    // Spec §2.2: each count is the maximum over a response's lines, its time the earliest, and
    // its speed and finality come from the final line, whatever order lines and batches come in.
    let key = usage_key(&cc(), "msg_1", "req_1");
    let stale_late = row(key, "s1", T + 20, 5);
    let stale_first = row(key, "s1", T, 1);
    let mut last = row(key, "s1", T + 40, 9);
    last.counts.web_search = 1;
    last.speed = Some("standard".into());
    last.finalized = true;
    let lines = [stale_late, stale_first, last];
    let mut want = row(key, "s1", T, 9);
    want.counts.web_search = 1;
    want.speed = Some("standard".into());
    want.finalized = true;

    const ORDERS: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for order in ORDERS {
        let (_d, s, _) = store();
        let outcomes: Vec<BatchOutcome> = order
            .iter()
            .map(|&i| commit(&s, &[lines[i].clone()], T))
            .collect();
        assert_eq!(all(&s), [want.clone()], "one line per batch: {order:?}");
        assert_eq!(
            outcomes[0],
            BatchOutcome {
                added: 1,
                updated: 0
            },
            "{order:?}"
        );
        assert!(
            outcomes[1..].iter().all(|o| o.added == 0),
            "{order:?}: {outcomes:?}"
        );

        let (_d, s, _) = store();
        let batch: Vec<UsageRow> = order.iter().map(|&i| lines[i].clone()).collect();
        assert_eq!(
            commit(&s, &batch, T),
            BatchOutcome {
                added: 1,
                updated: 0
            },
            "one batch: {order:?}"
        );
        assert_eq!(all(&s), [want.clone()], "{order:?}");
        assert_eq!(
            commit(&s, &batch, T),
            BatchOutcome::default(),
            "read again, nothing changes: {order:?}"
        );
    }
}

#[test]
fn a_batch_creates_state_and_moves_changed_at_only_on_change() {
    let (_d, s, _) = store();
    assert_eq!(s.token_state(&cc()).unwrap(), None);
    let f = file("/t/a.jsonl", 10);
    assert_eq!(
        s.commit_token_batch(&cc(), &[], std::slice::from_ref(&f), &[], T)
            .unwrap(),
        BatchOutcome::default(),
        "offsets alone"
    );
    assert_eq!(
        s.token_state(&cc()).unwrap(),
        Some(TokenStateRow {
            refreshed_at: None,
            changed_at: None,
            stats: RefreshStats::default(),
            pruned_at: None,
        })
    );
    assert_eq!(s.token_files(&cc()).unwrap(), [f.clone()]);
    assert!(s.token_files(&fake()).unwrap().is_empty());

    let changed = |s: &Store| s.token_state(&cc()).unwrap().unwrap().changed_at;
    let r = row(1, "s1", T, 5);
    assert_eq!(
        commit(&s, std::slice::from_ref(&r), T + 1),
        BatchOutcome {
            added: 1,
            updated: 0
        }
    );
    assert_eq!(changed(&s), Some(T + 1));
    assert_eq!(
        commit(&s, std::slice::from_ref(&r), T + 2),
        BatchOutcome::default()
    );
    assert_eq!(changed(&s), Some(T + 1), "nothing changed");
    let mut more = r.clone();
    more.counts.output = 6;
    assert_eq!(
        commit(&s, &[more], T + 3),
        BatchOutcome {
            added: 0,
            updated: 1
        }
    );
    assert_eq!(changed(&s), Some(T + 3));

    let g = TokenFileRow {
        offset: 20,
        size: 300,
        ..f.clone()
    };
    s.commit_token_batch(&cc(), &[], std::slice::from_ref(&g), &[], T + 4)
        .unwrap();
    assert_eq!(s.token_files(&cc()).unwrap(), [g], "rewritten at its path");
    s.commit_token_batch(&cc(), &[], &[], &["/t/a.jsonl".to_owned()], T + 5)
        .unwrap();
    assert!(s.token_files(&cc()).unwrap().is_empty(), "removed");
    assert_eq!(
        all(&s).len(),
        1,
        "a removed file keeps its usage (spec §5.2)"
    );
    assert_eq!(changed(&s), Some(T + 3));
}

#[test]
fn finish_sets_refreshed_and_the_first_completion_moves_changed_at() {
    let (_d, s, _) = store();
    commit(&s, &[row(1, "s1", T, 5), row(2, "s2", T + 1, 5)], T);
    s.commit_token_batch(&fake(), &[row(3, "f1", T, 5)], &[], &[], T)
        .unwrap();
    let stats = RefreshStats {
        files: 3,
        responses: 0,
        skipped_lines: 2,
        unreadable_files: 1,
    };
    s.finish_token_refresh(&cc(), &stats, T + 10).unwrap();
    assert_eq!(
        s.token_state(&cc()).unwrap(),
        Some(TokenStateRow {
            refreshed_at: Some(T + 10),
            changed_at: Some(T + 10),
            stats: RefreshStats {
                responses: 2,
                ..stats
            },
            pruned_at: None,
        }),
        "the first completion ends the partial index; responses are the index's own count"
    );
    let later = RefreshStats {
        files: 4,
        ..RefreshStats::default()
    };
    s.finish_token_refresh(&cc(), &later, T + 20).unwrap();
    assert_eq!(
        s.token_state(&cc()).unwrap(),
        Some(TokenStateRow {
            refreshed_at: Some(T + 20),
            changed_at: Some(T + 10),
            stats: RefreshStats {
                files: 4,
                responses: 2,
                skipped_lines: 0,
                unreadable_files: 0,
            },
            pruned_at: None,
        })
    );
    let other = ProviderId::new("other");
    s.finish_token_refresh(&other, &RefreshStats::default(), T + 30)
        .unwrap();
    assert_eq!(
        s.token_state(&other).unwrap(),
        Some(TokenStateRow {
            refreshed_at: Some(T + 30),
            changed_at: Some(T + 30),
            stats: RefreshStats::default(),
            pruned_at: None,
        }),
        "a provider with nothing to index completes too"
    );
}

#[test]
fn finish_inserts_an_epoch_after_a_full_purge() {
    let (_d, s, _) = store();
    let epoch = s.token_epoch().unwrap().expect("the migration's epoch");
    assert!(!epoch.seeded, "a raw open seeds nothing (ruling R1)");
    s.finish_token_refresh(&cc(), &RefreshStats::default(), T)
        .unwrap();
    assert_eq!(
        s.token_epoch().unwrap(),
        Some(epoch),
        "the epoch never moves"
    );
    s.empty_all().unwrap();
    assert_eq!(s.token_epoch().unwrap(), None);
    s.finish_token_refresh(&cc(), &RefreshStats::default(), T + 5)
        .unwrap();
    assert_eq!(
        s.token_epoch().unwrap(),
        Some(EpochRow {
            at: T + 5,
            seeded: true
        })
    );
}

#[test]
fn prune_runs_once_a_day_and_moves_changed_at_when_it_deletes() {
    let (_d, s, _) = store();
    let cutoff = T - 365 * DAY;
    let mut old: Vec<UsageRow> = (1000..1000 + 5_001)
        .map(|k| row(k, "bulk", cutoff - 1, 1))
        .collect();
    old.extend([row(1, "old", cutoff - 2, 1), row(2, "live", cutoff - 1, 1)]);
    commit(&s, &old, T);
    commit(&s, &[row(3, "live", cutoff, 1)], T);
    s.commit_token_batch(&fake(), &[row(9, "f1", cutoff - 5, 1)], &[], &[], T)
        .unwrap();
    s.record_sighting(&cc(), &sighting("old", "", cutoff - 10, cutoff - 1), 0)
        .unwrap();
    s.record_sighting(&cc(), &sighting("live", "", cutoff - 10, T), 0)
        .unwrap();
    for (account, ended) in [("a", Some(cutoff - 1)), ("b", Some(cutoff)), ("c", None)] {
        s.open_launch(&cc(), account, cutoff - 10, T).unwrap();
        if let Some(end) = ended {
            s.close_launch(&cc(), account, cutoff - 10, end, T).unwrap();
        }
    }

    assert_eq!(prune(&s, cutoff, "h1", T), done(5_003));
    assert_eq!(keys(&all(&s)), [3], "only the response inside the window");
    assert_eq!(
        s.sightings(&cc()).unwrap(),
        [sighting("live", "", cutoff - 10, T)],
        "a sighting that ended before the window goes; a live one stays"
    );
    assert_eq!(
        s.launches(&cc())
            .unwrap()
            .iter()
            .map(|l| l.account_id.as_str())
            .collect::<Vec<_>>(),
        ["b", "c"],
        "only a launch that ended before the window goes"
    );
    assert_eq!(
        s.token_usage(&fake(), None, None, None).unwrap().len(),
        1,
        "another provider's rows are not this prune's"
    );
    let state = s.token_state(&cc()).unwrap().unwrap();
    assert_eq!((state.pruned_at, state.changed_at), (Some(T), Some(T)));
    assert_eq!(state.stats.responses, 1, "the count follows the prune");
    assert_eq!(
        s.token_models(&cc()).unwrap(),
        ["claude-opus-5-5"],
        "ruling R8: a model's name stays"
    );

    commit(&s, &[row(4, "live", cutoff - 3, 1)], T);
    assert_eq!(
        prune(&s, cutoff, "h2", T + DAY - 1),
        done(0),
        "within the day, the lease holds: nothing is due"
    );
    assert_eq!(keys(&all(&s)), [4, 3]);
    assert_eq!(prune(&s, cutoff, "h2", T + DAY), done(1));
    let state = s.token_state(&cc()).unwrap().unwrap();
    assert_eq!(
        (state.pruned_at, state.changed_at, state.stats.responses),
        (Some(T + DAY), Some(T + DAY), 1)
    );
    assert_eq!(prune(&s, cutoff, "h3", T + 2 * DAY), done(0));
    let state = s.token_state(&cc()).unwrap().unwrap();
    assert_eq!(
        (state.pruned_at, state.changed_at),
        (Some(T + 2 * DAY), Some(T + DAY)),
        "a prune that deletes nothing changes no report"
    );
}

#[test]
fn a_prune_that_fails_part_way_still_moves_changed_at() {
    // Each chunk that deletes a response moves `changed_at` with it: a prune that stops after
    // its chunks, under a lease that keeps a retry away for a day, leaves no change unseen.
    let (_d, s, path) = store();
    let cutoff = T - 365 * DAY;
    commit(&s, &[row(1, "old", cutoff - 1, 1), row(2, "live", T, 1)], T);
    s.open_launch(&cc(), "a", cutoff - 10, T).unwrap();
    s.close_launch(&cc(), "a", cutoff - 10, cutoff - 1, T)
        .unwrap();
    // The last transaction deletes that launch; a trigger makes it fail.
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch(
        "CREATE TRIGGER fail_the_last_step BEFORE DELETE ON run_launches \
         BEGIN SELECT RAISE(ABORT, 'injected'); END;",
    )
    .unwrap();

    assert!(
        s.prune_tokens(&cc(), cutoff, "h1", T + 5, &|| false)
            .is_err()
    );
    assert_eq!(keys(&all(&s)), [2], "the chunk committed");
    let state = s.token_state(&cc()).unwrap().unwrap();
    assert_eq!(
        (state.changed_at, state.stats.responses, state.pruned_at),
        (Some(T + 5), 1, None)
    );
    assert_eq!(
        prune(&s, cutoff, "h2", T + 6),
        done(0),
        "the lease holds the retry off"
    );

    raw.execute_batch("DROP TRIGGER fail_the_last_step;")
        .unwrap();
    assert_eq!(prune(&s, cutoff, "h2", T + DAY + 5), done(0));
    assert!(
        s.launches(&cc()).unwrap().is_empty(),
        "the next day's prune finishes it"
    );
    assert_eq!(
        s.token_state(&cc()).unwrap().unwrap().changed_at,
        Some(T + 5),
        "deleting nothing changes no report"
    );
}

#[test]
fn a_stopped_prune_resumes_on_the_next_call_and_completes() {
    // A retention cut can make a prune large: `stop` (the refresh's budget and cancel) is asked
    // before each chunk, and a stopped prune hands its lease back so the next call goes on.
    let (_d, s, _) = store();
    let cutoff = T - 30 * DAY;
    let mut rows: Vec<UsageRow> = (1000..1000 + 5_001)
        .map(|k| row(k, "old", cutoff - 1, 1))
        .collect();
    rows.push(row(1, "live", T, 1));
    commit(&s, &rows, T);

    let asked = Cell::new(0);
    let after_one_chunk = || {
        asked.set(asked.get() + 1);
        asked.get() > 1
    };
    assert_eq!(
        s.prune_tokens(&cc(), cutoff, "h1", T + 1, &after_one_chunk)
            .unwrap(),
        PruneOutcome {
            deleted: 5_000,
            complete: false
        }
    );
    let state = s.token_state(&cc()).unwrap().unwrap();
    assert_eq!(
        (state.changed_at, state.stats.responses, state.pruned_at),
        (Some(T + 1), 2, None),
        "the chunk that ran is seen; the prune is not done"
    );

    assert_eq!(
        prune(&s, cutoff, "h2", T + 2),
        done(1),
        "the next call, the same day, goes on"
    );
    assert_eq!(keys(&all(&s)), [1]);
    let state = s.token_state(&cc()).unwrap().unwrap();
    assert_eq!(
        (state.changed_at, state.stats.responses, state.pruned_at),
        (Some(T + 2), 1, Some(T + 2))
    );
    assert_eq!(
        prune(&s, cutoff, "h3", T + 3),
        done(0),
        "complete, it holds the day's lease"
    );

    let stopped = s
        .prune_tokens(&cc(), cutoff, "h4", T + DAY + 2, &|| true)
        .unwrap();
    assert_eq!(
        stopped,
        PruneOutcome {
            deleted: 0,
            complete: false
        },
        "stopped before its first chunk"
    );
    assert_eq!(
        s.token_state(&cc()).unwrap().unwrap().pruned_at,
        Some(T + 2)
    );
}

#[test]
fn the_cleanup_after_the_responses_stops_and_resumes_too() {
    // The phases after the expired responses (sessions, sightings, launches) also delete in
    // chunks with `stop` asked before each: a large backlog of expired sightings never runs
    // as one transaction.
    let (_d, s, path) = store();
    let cutoff = T - 30 * DAY;
    {
        let mut raw = rusqlite::Connection::open(&path).unwrap();
        let tx = raw.transaction().unwrap();
        for i in 0..6_000 {
            tx.execute(
                "INSERT INTO session_sightings \
                 (provider, session_id, home, first_at, last_at, source, recorded_at) \
                 VALUES ('claude-code', ?1, '', ?2, ?2, 'scan', ?2)",
                rusqlite::params![format!("s{i}"), cutoff - 1],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    let sightings = || -> i64 {
        rusqlite::Connection::open(&path)
            .unwrap()
            .query_row("SELECT count(*) FROM session_sightings", [], |r| r.get(0))
            .unwrap()
    };

    // Asked before: the responses (none), the sessions (none), the first 5,000 sightings, and
    // then it stops.
    let asked = Cell::new(0);
    let after_three = || {
        asked.set(asked.get() + 1);
        asked.get() > 3
    };
    assert_eq!(
        s.prune_tokens(&cc(), cutoff, "h1", T, &after_three)
            .unwrap(),
        PruneOutcome {
            deleted: 0,
            complete: false
        }
    );
    assert_eq!(sightings(), 1_000);
    assert_eq!(
        s.token_state(&cc()).unwrap().and_then(|st| st.pruned_at),
        None
    );

    assert_eq!(prune(&s, cutoff, "h2", T + 1), done(0));
    assert_eq!(sightings(), 0);
    assert_eq!(
        s.token_state(&cc()).unwrap().unwrap().pruned_at,
        Some(T + 1)
    );
}

#[test]
fn sightings_insert_then_extend_only_after_the_gap() {
    let (_d, s, _) = store();
    let seen = |last: i64, source| SightingRow {
        source,
        ..sighting("s1", "", T, last)
    };
    assert!(
        s.record_sighting(&cc(), &seen(T, SightingSource::Scan), 60_000)
            .unwrap()
    );
    assert!(
        !s.record_sighting(&cc(), &seen(T + 59_999, SightingSource::Statusline), 60_000)
            .unwrap(),
        "within the gap"
    );
    assert!(
        s.record_sighting(&cc(), &seen(T + 60_000, SightingSource::Statusline), 60_000)
            .unwrap()
    );
    assert_eq!(
        s.latest_sighting(&cc(), "s1", "").unwrap(),
        Some(seen(T + 60_000, SightingSource::Scan)),
        "an extension keeps the source that created it"
    );
    assert!(
        !s.record_sighting(&cc(), &seen(T + 10, SightingSource::Run), 0)
            .unwrap(),
        "last_at never moves back"
    );

    let next = sighting("s1", "", T + 100_000, T + 100_000);
    assert!(s.record_sighting(&cc(), &next, 60_000).unwrap());
    assert_eq!(
        s.latest_sighting(&cc(), "s1", "").unwrap(),
        Some(next.clone()),
        "another execution is its own sighting, and the latest"
    );
    let profile = sighting("s1", "acct-a", T, T + 5);
    assert!(s.record_sighting(&cc(), &profile, 60_000).unwrap());
    assert_eq!(
        s.latest_sighting(&cc(), "s1", "acct-a").unwrap(),
        Some(profile.clone())
    );
    assert_eq!(
        s.sightings(&cc()).unwrap(),
        [
            seen(T + 60_000, SightingSource::Scan),
            profile,
            next.clone()
        ]
    );

    s.set_sighting_last_at(&cc(), "s1", "", T + 100_000, T + 200_000, T + 200_001)
        .unwrap();
    let exited = SightingRow {
        last_at: T + 200_000,
        recorded_at: T + 200_001,
        ..next
    };
    assert_eq!(
        s.latest_sighting(&cc(), "s1", "").unwrap(),
        Some(exited.clone())
    );
    s.set_sighting_last_at(&cc(), "s1", "", T + 100_000, T + 150_000, T + 300_000)
        .unwrap();
    assert_eq!(
        s.latest_sighting(&cc(), "s1", "").unwrap(),
        Some(exited),
        "never back"
    );
    assert_eq!(s.latest_sighting(&cc(), "s2", "").unwrap(), None);
    assert!(s.sightings(&fake()).unwrap().is_empty());
}

#[test]
fn record_sighting_within_gives_up_on_a_busy_store() {
    // Spec §6.3: while another connection holds the write lock, the statusline's write gives up
    // after its own short wait and writes nothing. Free, it writes as `record_sighting` does.
    let (_d, s, path) = store();
    let mut other = rusqlite::Connection::open(&path).unwrap();
    let held = other
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let seen = sighting("s1", "", T, T);
    let err = s
        .record_sighting_within(&cc(), &seen, 60_000, Duration::from_millis(1))
        .unwrap_err();
    assert_eq!(
        err.sqlite_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy),
        "{err:?}"
    );
    drop(held);
    assert_eq!(
        s.latest_sighting(&cc(), "s1", "").unwrap(),
        None,
        "nothing written"
    );

    assert!(
        s.record_sighting_within(&cc(), &seen, 60_000, Duration::from_millis(1))
            .unwrap()
    );
    assert!(
        !s.record_sighting_within(&cc(), &seen, 60_000, Duration::from_millis(1))
            .unwrap(),
        "within the gap"
    );
    assert_eq!(s.latest_sighting(&cc(), "s1", "").unwrap(), Some(seen));
}

#[cfg(feature = "test-hooks")]
#[test]
fn record_sighting_within_restores_the_connection_s_wait() {
    // Every other write of the process waits its full `busy_timeout` again, whether the short
    // write succeeded or gave up.
    let (_d, s, path) = store();
    let normal = s.busy_timeout().unwrap();
    assert_eq!(normal, Duration::from_millis(5000));
    let seen = sighting("s1", "", T, T);
    s.record_sighting_within(&cc(), &seen, 60_000, Duration::from_millis(1))
        .unwrap();
    assert_eq!(s.busy_timeout().unwrap(), normal, "after a write");
    let mut other = rusqlite::Connection::open(&path).unwrap();
    let held = other
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    s.record_sighting_within(&cc(), &seen, 0, Duration::from_millis(1))
        .unwrap_err();
    drop(held);
    assert_eq!(s.busy_timeout().unwrap(), normal, "after giving up");
}

#[test]
fn launches_open_attach_close() {
    let (_d, s, _) = store();
    s.open_launch(&cc(), "acct-a", T, T).unwrap();
    let open = LaunchRow {
        account_id: "acct-a".into(),
        started_at: T,
        ended_at: None,
        pid: None,
        recorded_at: T,
    };
    assert_eq!(s.launches(&cc()).unwrap(), [open.clone()]);
    s.attach_launch_pid(&cc(), "acct-a", T, 4242, T + 1)
        .unwrap();
    let spawned = LaunchRow {
        pid: Some(4242),
        recorded_at: T + 1,
        ..open
    };
    assert_eq!(s.launches(&cc()).unwrap(), [spawned.clone()]);
    s.close_launch(&cc(), "acct-a", T, T + 50, T + 51).unwrap();
    let closed = LaunchRow {
        ended_at: Some(T + 50),
        recorded_at: T + 51,
        ..spawned
    };
    assert_eq!(s.launches(&cc()).unwrap(), [closed.clone()]);
    s.close_launch(&cc(), "acct-a", T, T + 90, T + 91).unwrap();
    s.open_launch(&cc(), "acct-a", T, T + 99).unwrap();
    assert_eq!(
        s.launches(&cc()).unwrap(),
        [closed.clone()],
        "an ended launch keeps its end, and a recorded one its row"
    );
    s.open_launch(&cc(), "acct-b", T - 5, T).unwrap();
    s.open_launch(&fake(), "acct-a", T, T).unwrap();
    assert_eq!(
        s.launches(&cc())
            .unwrap()
            .iter()
            .map(|l| (l.account_id.as_str(), l.started_at))
            .collect::<Vec<_>>(),
        [("acct-b", T - 5), ("acct-a", T)],
        "by start"
    );
}

#[test]
fn evidence_changed_at_is_the_latest_recorded_at() {
    let (_d, s, _) = store();
    assert_eq!(s.evidence_changed_at(&cc()).unwrap(), None);
    s.open_launch(&cc(), "acct-a", 1_000, 1_000).unwrap();
    assert_eq!(s.evidence_changed_at(&cc()).unwrap(), Some(1_000));
    s.record_sighting(
        &cc(),
        &SightingRow {
            recorded_at: 2_000,
            ..sighting("s1", "", 1_500, 1_500)
        },
        0,
    )
    .unwrap();
    assert_eq!(s.evidence_changed_at(&cc()).unwrap(), Some(2_000));
    // active_history is stamped by SQLite's own clock (spec §4).
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let before = wall_ms();
    s.set_active(&cc(), Some(&a), Some(0)).unwrap();
    let at = s.evidence_changed_at(&cc()).unwrap().unwrap();
    assert!(at >= before - 1, "{before} {at}");
    assert_eq!(s.active_history(&cc()).unwrap(), [(at, Some("a".into()))]);
    s.close_launch(&cc(), "acct-a", 1_000, 5_000, at + 10)
        .unwrap();
    assert_eq!(s.evidence_changed_at(&cc()).unwrap(), Some(at + 10));
    s.open_launch(&fake(), "acct-f", 0, at + 1_000_000).unwrap();
    assert_eq!(
        s.evidence_changed_at(&cc()).unwrap(),
        Some(at + 10),
        "another provider's evidence is not this one's"
    );
}

#[test]
fn purge_provider_deletes_token_rows_and_keeps_the_epoch() {
    // Spec §13: `purge --provider` deletes the provider's rows of the provider-keyed token
    // tables (seven with ruling R8's `token_models`), its responses with its sessions;
    // `token_epoch` stays.
    let (_d, s, path) = store();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let f = add(&s, &fake(), "f", "f@x.co", 1);
    for (p, id) in [(cc(), &a), (fake(), &f)] {
        s.set_active(&p, Some(id), Some(0)).unwrap();
        let path = format!("/t/{p}.jsonl");
        s.commit_token_batch(
            &p,
            &[row(usage_key(&p, "m", "r"), "s1", T, 1)],
            &[file(&path, 1)],
            &[],
            T,
        )
        .unwrap();
        s.finish_token_refresh(&p, &RefreshStats::default(), T)
            .unwrap();
        s.prune_tokens(&p, 0, "h", T, &|| false).unwrap();
        s.record_sighting(&p, &sighting("s1", "", T, T), 0).unwrap();
        s.open_launch(&p, "acct", T, T).unwrap();
    }
    let epoch = s.token_epoch().unwrap();
    assert!(epoch.is_some());

    s.delete_provider_rows(&cc()).unwrap();

    for (p, kept) in [(cc(), false), (fake(), true)] {
        assert_eq!(!s.token_files(&p).unwrap().is_empty(), kept, "{p}");
        assert_eq!(
            !s.token_usage(&p, None, None, None).unwrap().is_empty(),
            kept,
            "{p}"
        );
        assert_eq!(s.token_state(&p).unwrap().is_some(), kept, "{p}");
        assert_eq!(!s.sightings(&p).unwrap().is_empty(), kept, "{p}");
        assert_eq!(!s.launches(&p).unwrap().is_empty(), kept, "{p}");
        assert_eq!(!s.active_history(&p).unwrap().is_empty(), kept, "{p}");
        assert_eq!(!s.token_models(&p).unwrap().is_empty(), kept, "{p}");
        let sessions = raw_count(
            &path,
            &format!("SELECT count(*) FROM token_sessions WHERE provider = '{p}'"),
        );
        assert_eq!(sessions > 0, kept, "{p}");
        let leases = raw_count(
            &path,
            &format!("SELECT count(*) FROM leases WHERE name = 'prune:tokens:{p}'"),
        );
        assert_eq!(leases > 0, kept, "{p}");
    }
    assert_eq!(
        raw_count(&path, "SELECT count(*) FROM token_usage"),
        1,
        "the provider's responses went with its sessions"
    );
    assert_eq!(s.token_epoch().unwrap(), epoch);
}

#[test]
fn usage_key_is_stable() {
    // A key is stored: changing how it is derived would count every response twice.
    assert_eq!(
        usage_key(&cc(), "msg_01ABC", "req_01XYZ"),
        -3_640_870_161_912_078_139
    );
    let k = usage_key(&cc(), "m", "r");
    assert_ne!(
        k,
        usage_key(&fake(), "m", "r"),
        "the provider is part of it"
    );
    assert_ne!(
        k,
        usage_key(&cc(), "m", ""),
        "ruling R5: no request id is its own key"
    );
    assert_ne!(
        usage_key(&cc(), "ab", ""),
        usage_key(&cc(), "a", "b"),
        "the separator keeps the fields apart"
    );
}

#[test]
fn reads_are_per_provider_by_range_and_session() {
    let (_d, s, _) = store();
    let mut first = row(1, "s1", T, 1);
    first.model = "m-b".into();
    first.sidechain = true;
    let mut second = row(2, "s2", T + 10, 1);
    second.model = "m-a".into();
    second.project = None;
    let mut third = row(3, "s1", T + 20, 1);
    third.model = "m-b".into();
    commit(&s, &[third.clone(), first.clone(), second.clone()], T);
    let mut theirs = row(9, "s1", T + 5, 1);
    theirs.model = "m-z".into();
    s.commit_token_batch(&fake(), &[theirs], &[], &[], T)
        .unwrap();

    let usage = |since, until, session| keys(&s.token_usage(&cc(), since, until, session).unwrap());
    assert_eq!(all(&s), [first.clone(), second, third.clone()]);
    assert_eq!(
        usage(Some(T + 10), None, None),
        [2, 3],
        "since is inclusive"
    );
    assert_eq!(usage(None, Some(T + 10), None), [1], "until is exclusive");
    assert_eq!(usage(None, None, Some("s1")), [1, 3]);
    assert_eq!(usage(Some(T + 10), None, Some("s1")), [3]);
    assert!(usage(None, None, Some("nope")).is_empty());
    assert_eq!(
        s.token_usage(&cc(), None, None, Some("s1")).unwrap(),
        [first, third],
        "a session's responses keep their project and flags"
    );
    assert_eq!(s.token_models(&cc()).unwrap(), ["m-a", "m-b"]);
    assert_eq!(s.token_span(&cc()).unwrap(), (Some(T), Some(T + 20)));
    assert_eq!(s.token_models(&fake()).unwrap(), ["m-z"]);
    assert_eq!(
        s.token_span(&ProviderId::new("none")).unwrap(),
        (None, None)
    );
}
```

- [ ] **Step 2: Run them and see them fail**

```
cargo test -p tagteam-engine --locked --test store_tokens
```

Expected: the file does not compile:

```
error[E0432]: unresolved imports `tagteam_engine::store::BatchOutcome`, `tagteam_engine::store::EpochRow`, `tagteam_engine::store::LaunchRow`, `tagteam_engine::store::PruneOutcome`, `tagteam_engine::store::RefreshStats`, `tagteam_engine::store::SightingRow`, `tagteam_engine::store::SightingSource`, `tagteam_engine::store::TokenFileRow`, `tagteam_engine::store::TokenStateRow`, `tagteam_engine::store::UsageRow`, `tagteam_engine::store::usage_key`
```

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/Cargo.toml`, under `[dependencies]`, between `serde_json.workspace = true` and `thiserror.workspace = true`:

```toml
sha2.workspace = true
```

Then update the lock file once, offline (sha2 0.10.9 is already locked for `tagteam-core`, so nothing is downloaded and no version changes):

```
cargo build -p tagteam-engine --offline
git diff --stat Cargo.lock
```

Expected: `Cargo.lock | 1 +`, the line `"sha2 0.10.9",` added to `tagteam-engine`'s `dependencies`. Every later command takes `--locked` again.

In `crates/tagteam-engine/src/store/usage.rs`, make the lease statement visible to its sibling module (rustfmt keeps the first line as it is):

```rust
/// §6.1's lease statement. The lease is held when it changes one row: it was free, or its
/// holder's expiry (`?4`, now in ms) has passed.
pub(super) const TAKE_LEASE_SQL: &str = "INSERT INTO leases (name, holder, expires_at) VALUES (?1, ?2, ?3) \
    ON CONFLICT(name) DO UPDATE SET holder = excluded.holder, expires_at = excluded.expires_at \
    WHERE leases.expires_at <= ?4";
```

In `crates/tagteam-engine/src/store/mod.rs`, replace:

```rust
mod usage;

pub use usage::{
```

with:

```rust
mod tokens;
mod usage;

pub use tokens::{
    BatchOutcome, EpochRow, LaunchRow, PruneOutcome, RefreshStats, SightingRow, SightingSource,
    TokenFileRow, TokenStateRow, UsageRow, usage_key,
};
pub use usage::{
```

and replace the whole of `delete_provider_rows` (doc comment included):

```rust
    /// §10.5 with `--provider P`, once its accounts are gone: P's events, auto-switch state,
    /// active account, live identity cache and switch journal, in one transaction. Its
    /// `usage_requests` rows stay, so the hourly budget does not reset (§8.6), and its displaced
    /// entries go through the displaced purge, each file before its row (§6.3).
    pub fn delete_provider_rows(&self, provider: &ProviderId) -> Result<(), StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for table in [
            "events",
            "autoswitch_state",
            "active_accounts",
            "live_identity_cache",
            "switch_journal",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE provider = ?1"),
                [provider.as_str()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
```

with:

```rust
    /// §10.5 with `--provider P`, once its accounts are gone: P's events, auto-switch state,
    /// active account, live identity cache and switch journal, in one transaction. Its
    /// `usage_requests` rows stay, so the hourly budget does not reset (§8.6), and its displaced
    /// entries go through the displaced purge, each file before its row (§6.3). The same
    /// transaction deletes P's token index and session evidence, its sessions taking their
    /// responses with them, and its prune lease; `token_epoch` stays (token report spec §13).
    pub fn delete_provider_rows(&self, provider: &ProviderId) -> Result<(), StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for table in [
            "events",
            "autoswitch_state",
            "active_accounts",
            "live_identity_cache",
            "switch_journal",
            "token_files",
            "token_sessions",
            "token_models",
            "session_sightings",
            "run_launches",
            "active_history",
            "token_state",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE provider = ?1"),
                [provider.as_str()],
            )?;
        }
        tx.execute(
            "DELETE FROM leases WHERE name = ?1",
            [format!("prune:tokens:{provider}")],
        )?;
        tx.commit()?;
        Ok(())
    }
```

`token_sessions` comes before `token_models`: deleting the sessions cascades their responses, so no response still references a model when its row goes (`token_usage.model` is a foreign key). `empty_all` needs no change: it deletes every table, `token_epoch` and `active_history` included, with foreign keys deferred. Deleting `active_accounts` rows fires no trigger, because only inserts and updates do.

Create `crates/tagteam-engine/src/store/tokens.rs`:

```rust
//! The token tables (token report spec §4): the transcript files the index has read, its
//! sessions, models and responses, the session evidence (sightings, launches and the active
//! account's history), each provider's index state and the epoch. Every time is epoch
//! milliseconds. Counts are `u64` here and `INTEGER` in SQLite: a count above `i64::MAX` is
//! stored as `i64::MAX`.

use std::collections::HashMap;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use tagteam_core::ProviderId;
use tagteam_core::tokens::TokenCounts;

use super::usage::TAKE_LEASE_SQL;
use super::{Store, StoreError};

const DAY_MS: i64 = 86_400_000;

/// Responses one prune transaction deletes at most, so an overdue prune never holds the write
/// lock for long (spec §5.2).
const PRUNE_CHUNK: usize = 5_000;

/// What a prune deletes after the expired responses (spec §4), each phase in chunks of
/// `PRUNE_CHUNK` rows, one transaction per chunk: sessions left without a response, sightings
/// that ended before the cutoff, and launches that did. `?1` the provider, `?2` the cutoff, `?3`
/// the chunk.
const PRUNE_CLEANUP: [&str; 3] = [
    "DELETE FROM token_sessions WHERE id IN (SELECT id FROM token_sessions WHERE provider = ?1 \
     AND NOT EXISTS (SELECT 1 FROM token_usage WHERE session = token_sessions.id) LIMIT ?3)",
    "DELETE FROM session_sightings WHERE (provider, session_id, home, first_at) IN \
     (SELECT provider, session_id, home, first_at FROM session_sightings \
     WHERE provider = ?1 AND last_at < ?2 LIMIT ?3)",
    "DELETE FROM run_launches WHERE (provider, account_id, started_at) IN \
     (SELECT provider, account_id, started_at FROM run_launches \
     WHERE provider = ?1 AND ended_at < ?2 LIMIT ?3)",
];

/// A response's key: the first 8 bytes of SHA-256 over `provider NUL message_id NUL
/// request_id`, big-endian (spec §4). Response ids are kept only as this hash (spec §13).
pub fn usage_key(provider: &ProviderId, message_id: &str, request_id: &str) -> i64 {
    let mut hash = Sha256::new();
    hash.update(provider.as_str().as_bytes());
    hash.update([0]);
    hash.update(message_id.as_bytes());
    hash.update([0]);
    hash.update(request_id.as_bytes());
    let digest = hash.finalize();
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(first)
}

/// One transcript file as the index last read it (spec §5.2). `size` is −1 until a read
/// consumed the file to its last newline, so no file matches it and none is skipped part-way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenFileRow {
    pub path: String,
    pub dev: u64,
    pub ino: u64,
    pub size: i64,
    pub mtime_ns: i64,
    pub offset: u64,
    pub head_hash: [u8; 8],
    pub tail_hash: [u8; 8],
}

/// One response (spec §2.2): `key` is its `usage_key`, and its counts, time and flags are
/// merged over its lines.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRow {
    pub key: i64,
    pub session_id: String,
    pub project: Option<String>,
    pub at_ms: i64,
    pub model: String,
    pub sidechain: bool,
    pub counts: TokenCounts,
    pub speed: Option<String>,
    pub finalized: bool,
}

/// What one batch did to the index: responses it added, and stored ones it changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BatchOutcome {
    pub added: u64,
    pub updated: u64,
}

/// What one `prune_tokens` call did: the responses it deleted, and whether it finished. A
/// prune that stopped part-way released its lease, so the next call goes on with it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneOutcome {
    pub deleted: u64,
    pub complete: bool,
}

/// A completed refresh's statistics (spec §4 `token_state`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RefreshStats {
    pub files: u64,
    pub responses: u64,
    pub skipped_lines: u64,
    pub unreadable_files: u64,
}

/// A provider's `token_state` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenStateRow {
    pub refreshed_at: Option<i64>,
    pub changed_at: Option<i64>,
    pub stats: RefreshStats,
    pub pruned_at: Option<i64>,
}

/// Which source created a sighting (spec §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SightingSource {
    Run,
    Scan,
    Statusline,
}

impl SightingSource {
    pub fn as_str(self) -> &'static str {
        match self {
            SightingSource::Run => "run",
            SightingSource::Scan => "scan",
            SightingSource::Statusline => "statusline",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "run" => Some(SightingSource::Run),
            "scan" => Some(SightingSource::Scan),
            "statusline" => Some(SightingSource::Statusline),
            _ => None,
        }
    }
}

/// One execution of a session seen alive in a home (spec §6.4), keyed by the session, the
/// home and `first_at`. `home` is `""` for the default home, else the profile's account id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SightingRow {
    pub session_id: String,
    pub home: String,
    pub first_at: i64,
    pub last_at: i64,
    pub source: SightingSource,
    pub recorded_at: i64,
}

/// A `run` launch (spec §6.1), or a profile seeded at the migration (spec §4). `ended_at` is
/// `None` while the launch may still run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRow {
    pub account_id: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub pid: Option<u32>,
    pub recorded_at: i64,
}

/// When this store first recorded sightings and launches, and whether it seeded the profiles
/// that existed then (spec §7.2, ruling R1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpochRow {
    pub at: i64,
    pub seeded: bool,
}

fn to_sql(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn from_sql(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

/// A file's hash as stored. One of another length reads as zeros, which no read matches, so
/// that file is read again from its start (spec §5.2).
fn hash_col(r: &Row<'_>, idx: usize) -> rusqlite::Result<[u8; 8]> {
    let bytes: Vec<u8> = r.get(idx)?;
    Ok(bytes.try_into().unwrap_or([0; 8]))
}

/// `counts` as the store holds them.
fn stored(counts: &TokenCounts) -> TokenCounts {
    let clamp = |n: u64| from_sql(to_sql(n));
    TokenCounts {
        input: clamp(counts.input),
        output: clamp(counts.output),
        cache_write_5m: clamp(counts.cache_write_5m),
        cache_write_1h: clamp(counts.cache_write_1h),
        cache_read: clamp(counts.cache_read),
        web_search: clamp(counts.web_search),
    }
}

/// The columns of a response that the merge rule writes (spec §2.2).
#[derive(Debug, Clone, PartialEq)]
struct Merged {
    at: i64,
    counts: TokenCounts,
    speed: Option<String>,
    finalized: bool,
}

impl Merged {
    /// The row as the upsert leaves it after `row`: the earliest time, each count's maximum,
    /// the newer speed when it has one, and final once any line was.
    fn with(&self, row: &UsageRow) -> Merged {
        let mut counts = self.counts;
        counts.merge_max(&stored(&row.counts));
        Merged {
            at: self.at.min(row.at_ms),
            counts,
            speed: row.speed.clone().or_else(|| self.speed.clone()),
            finalized: self.finalized || row.finalized,
        }
    }
}

fn merged_from_row(r: &Row<'_>) -> rusqlite::Result<Merged> {
    Ok(Merged {
        at: r.get(0)?,
        counts: TokenCounts {
            input: from_sql(r.get(1)?),
            output: from_sql(r.get(2)?),
            cache_write_5m: from_sql(r.get(3)?),
            cache_write_1h: from_sql(r.get(4)?),
            cache_read: from_sql(r.get(5)?),
            web_search: from_sql(r.get(6)?),
        },
        speed: r.get(7)?,
        finalized: r.get::<_, i64>(8)? != 0,
    })
}

/// `rows`, one per key: a key's later rows merge into its first by the rule of spec §2.2, and
/// the first keeps its session, project, model and sidechain flag, as the upsert does.
fn merge_batch(rows: &[UsageRow]) -> Vec<UsageRow> {
    let mut at: HashMap<i64, usize> = HashMap::with_capacity(rows.len());
    let mut out: Vec<UsageRow> = Vec::with_capacity(rows.len());
    for row in rows {
        match at.get(&row.key) {
            Some(&i) => {
                let first = &mut out[i];
                first.at_ms = first.at_ms.min(row.at_ms);
                first.counts.merge_max(&row.counts);
                if row.speed.is_some() {
                    first.speed = row.speed.clone();
                }
                first.finalized |= row.finalized;
            }
            None => {
                at.insert(row.key, out.len());
                out.push(row.clone());
            }
        }
    }
    out
}

const USAGE_MERGED: &str = "SELECT at, input, output, cw_5m, cw_1h, cache_read, web_search, speed, \
     finalized FROM token_usage WHERE key = ?1";

/// Spec §4's upsert: the earliest time, each count's maximum, the newer speed when it has one.
const UPSERT_USAGE: &str = "INSERT INTO token_usage (key, session, at, model, sidechain, input, \
     output, cw_5m, cw_1h, cache_read, web_search, speed, finalized) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
     ON CONFLICT(key) DO UPDATE SET at = min(at, excluded.at), \
     input = max(input, excluded.input), output = max(output, excluded.output), \
     cw_5m = max(cw_5m, excluded.cw_5m), cw_1h = max(cw_1h, excluded.cw_1h), \
     cache_read = max(cache_read, excluded.cache_read), \
     web_search = max(web_search, excluded.web_search), \
     speed = coalesce(excluded.speed, speed), finalized = max(finalized, excluded.finalized)";

/// A session's id, creating its row on its first response. The project is the first one
/// known (spec §4).
const UPSERT_SESSION: &str = "INSERT INTO token_sessions (provider, session_id, project) \
     VALUES (?1, ?2, ?3) ON CONFLICT(provider, session_id) DO UPDATE SET \
     project = coalesce(token_sessions.project, excluded.project) RETURNING id";

const MODEL_ID: &str = "SELECT id FROM token_models WHERE provider = ?1 AND name = ?2";

const INSERT_MODEL: &str = "INSERT INTO token_models (provider, name) VALUES (?1, ?2)";

const UPSERT_FILE: &str = "INSERT INTO token_files (path, provider, dev, ino, size, mtime_ns, \
     offset, head_hash, tail_hash, indexed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
     ON CONFLICT(path) DO UPDATE SET provider = excluded.provider, dev = excluded.dev, \
     ino = excluded.ino, size = excluded.size, mtime_ns = excluded.mtime_ns, \
     offset = excluded.offset, head_hash = excluded.head_hash, tail_hash = excluded.tail_hash, \
     indexed_at = excluded.indexed_at";

const ENSURE_STATE: &str = "INSERT OR IGNORE INTO token_state (provider) VALUES (?1)";

/// The provider's sessions, for a filter on `token_usage.session`.
const SESSIONS_OF: &str = "SELECT id FROM token_sessions WHERE provider = ?1";

const USAGE_COLUMNS: &str = "u.key, s.session_id, s.project, u.at, m.name, u.sidechain, \
     u.input, u.output, u.cw_5m, u.cw_1h, u.cache_read, u.web_search, u.speed, u.finalized";

fn usage_from_row(r: &Row<'_>) -> rusqlite::Result<UsageRow> {
    Ok(UsageRow {
        key: r.get(0)?,
        session_id: r.get(1)?,
        project: r.get(2)?,
        at_ms: r.get(3)?,
        model: r.get(4)?,
        sidechain: r.get::<_, i64>(5)? != 0,
        counts: TokenCounts {
            input: from_sql(r.get(6)?),
            output: from_sql(r.get(7)?),
            cache_write_5m: from_sql(r.get(8)?),
            cache_write_1h: from_sql(r.get(9)?),
            cache_read: from_sql(r.get(10)?),
            web_search: from_sql(r.get(11)?),
        },
        speed: r.get(12)?,
        finalized: r.get::<_, i64>(13)? != 0,
    })
}

const SIGHTING_COLUMNS: &str = "session_id, home, first_at, last_at, source, recorded_at";

fn sighting_from_row(r: &Row<'_>) -> rusqlite::Result<SightingRow> {
    let source: String = r.get(4)?;
    Ok(SightingRow {
        session_id: r.get(0)?,
        home: r.get(1)?,
        first_at: r.get(2)?,
        last_at: r.get(3)?,
        source: SightingSource::parse(&source).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                "unknown sighting source".into(),
            )
        })?,
        recorded_at: r.get(5)?,
    })
}

/// Spec §6.4's sighting write on one connection, which `Store::record_sighting` and
/// `Store::record_sighting_within` share: inserts the sighting, or extends the stored one when
/// `s.last_at` is at least `min_gap_ms` past its own. Returns whether it wrote.
fn write_sighting(
    c: &Connection,
    provider: &ProviderId,
    s: &SightingRow,
    min_gap_ms: i64,
) -> rusqlite::Result<bool> {
    let n = c.execute(
        "INSERT INTO session_sightings \
         (provider, session_id, home, first_at, last_at, source, recorded_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(provider, session_id, home, first_at) DO UPDATE SET \
         last_at = excluded.last_at, recorded_at = excluded.recorded_at \
         WHERE excluded.last_at - session_sightings.last_at >= ?8",
        params![
            provider.as_str(),
            s.session_id,
            s.home,
            s.first_at,
            s.last_at,
            s.source.as_str(),
            s.recorded_at,
            min_gap_ms,
        ],
    )?;
    Ok(n > 0)
}

fn prune_lease(provider: &ProviderId) -> String {
    format!("prune:tokens:{provider}")
}

/// The provider's responses older than `cutoff_ms`, at most `PRUNE_CHUNK` of them. A chunk
/// that deletes any takes their count off `token_state.responses` and sets `changed_at` to
/// `now_ms` in the same transaction, so a prune that stops part-way leaves no change unseen.
/// Returns how many went.
fn prune_chunk(
    c: &Connection,
    provider: &ProviderId,
    cutoff_ms: i64,
    now_ms: i64,
) -> rusqlite::Result<usize> {
    let n = c.execute(
        &format!(
            "DELETE FROM token_usage WHERE id IN (SELECT id FROM token_usage \
             WHERE at < ?2 AND session IN ({SESSIONS_OF}) LIMIT ?3)"
        ),
        params![provider.as_str(), cutoff_ms, PRUNE_CHUNK as i64],
    )?;
    if n > 0 {
        c.execute(
            "UPDATE token_state SET responses = max(responses - ?2, 0), changed_at = ?3 \
             WHERE provider = ?1",
            params![provider.as_str(), n as i64, now_ms],
        )?;
    }
    Ok(n)
}

// Every transaction here that reads before it writes is `IMMEDIATE`, as in `usage.rs`, and each
// stays short (spec §5.2): a batch is at most 2,000 responses, and a prune deletes in chunks.
impl Store {
    /// The provider's transcript files as the index last read them, by path.
    pub fn token_files(&self, provider: &ProviderId) -> Result<Vec<TokenFileRow>, StoreError> {
        let c = self.lock();
        let mut stmt = c.prepare(
            "SELECT path, dev, ino, size, mtime_ns, offset, head_hash, tail_hash FROM token_files \
             WHERE provider = ?1 ORDER BY path",
        )?;
        let rows = stmt
            .query_map([provider.as_str()], |r| {
                Ok(TokenFileRow {
                    path: r.get(0)?,
                    dev: r.get::<_, i64>(1)? as u64,
                    ino: r.get::<_, i64>(2)? as u64,
                    size: r.get(3)?,
                    mtime_ns: r.get(4)?,
                    offset: from_sql(r.get(5)?),
                    head_hash: hash_col(r, 6)?,
                    tail_hash: hash_col(r, 7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// One batch of a refresh (spec §5.2 step 4), in one `IMMEDIATE` transaction: upserts
    /// `rows` by the merge rule (spec §4), writes each of `files`, deletes the file rows at
    /// `removed`, and creates the provider's `token_state` row if absent. When a response was
    /// added or changed it sets `changed_at` to `now_ms`, and it adds the new responses to
    /// `responses`. A response whose merge changes nothing is not written.
    pub fn commit_token_batch(
        &self,
        provider: &ProviderId,
        rows: &[UsageRow],
        files: &[TokenFileRow],
        removed: &[String],
        now_ms: i64,
    ) -> Result<BatchOutcome, StoreError> {
        let rows = merge_batch(rows);
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(ENSURE_STATE, [provider.as_str()])?;
        let mut outcome = BatchOutcome::default();
        {
            let mut read = tx.prepare(USAGE_MERGED)?;
            let mut upsert = tx.prepare(UPSERT_USAGE)?;
            let mut session = tx.prepare(UPSERT_SESSION)?;
            let mut model_id = tx.prepare(MODEL_ID)?;
            let mut insert_model = tx.prepare(INSERT_MODEL)?;
            let mut sessions: HashMap<&str, i64> = HashMap::new();
            let mut models: HashMap<&str, i64> = HashMap::new();
            for row in &rows {
                match read.query_row([row.key], merged_from_row).optional()? {
                    None => outcome.added += 1,
                    Some(old) if old.with(row) != old => outcome.updated += 1,
                    Some(_) => continue,
                }
                let id = match sessions.get(row.session_id.as_str()) {
                    Some(id) => *id,
                    None => {
                        let id: i64 = session.query_row(
                            params![provider.as_str(), row.session_id, row.project],
                            |r| r.get(0),
                        )?;
                        sessions.insert(&row.session_id, id);
                        id
                    }
                };
                let model = match models.get(row.model.as_str()) {
                    Some(id) => *id,
                    None => {
                        let found = model_id
                            .query_row(params![provider.as_str(), row.model], |r| r.get(0))
                            .optional()?;
                        let id = match found {
                            Some(id) => id,
                            None => {
                                insert_model.execute(params![provider.as_str(), row.model])?;
                                tx.last_insert_rowid()
                            }
                        };
                        models.insert(&row.model, id);
                        id
                    }
                };
                let counts = &row.counts;
                upsert.execute(params![
                    row.key,
                    id,
                    row.at_ms,
                    model,
                    row.sidechain,
                    to_sql(counts.input),
                    to_sql(counts.output),
                    to_sql(counts.cache_write_5m),
                    to_sql(counts.cache_write_1h),
                    to_sql(counts.cache_read),
                    to_sql(counts.web_search),
                    row.speed,
                    row.finalized,
                ])?;
            }
            let mut file = tx.prepare(UPSERT_FILE)?;
            for f in files {
                file.execute(params![
                    f.path,
                    provider.as_str(),
                    f.dev as i64,
                    f.ino as i64,
                    f.size,
                    f.mtime_ns,
                    to_sql(f.offset),
                    &f.head_hash[..],
                    &f.tail_hash[..],
                    now_ms,
                ])?;
            }
            let mut remove =
                tx.prepare("DELETE FROM token_files WHERE provider = ?1 AND path = ?2")?;
            for path in removed {
                remove.execute(params![provider.as_str(), path])?;
            }
        }
        if outcome.added + outcome.updated > 0 {
            tx.execute(
                "UPDATE token_state SET changed_at = ?2, responses = responses + ?3 \
                 WHERE provider = ?1",
                params![provider.as_str(), now_ms, to_sql(outcome.added)],
            )?;
        }
        tx.commit()?;
        Ok(outcome)
    }

    /// Spec §5.2 step 5, in one `IMMEDIATE` transaction: sets `refreshed_at` and the refresh's
    /// `files`, `skipped_lines` and `unreadable_files`, and on the provider's first completion
    /// `changed_at` too, which ends the index's partial state (spec §14). `stats.responses` is
    /// not written: `responses` is the index's own count, which each batch and prune keep.
    /// After a full purge emptied `token_epoch`, it records the epoch anew at `now_ms`.
    pub fn finish_token_refresh(
        &self,
        provider: &ProviderId,
        stats: &RefreshStats,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(ENSURE_STATE, [provider.as_str()])?;
        tx.execute(
            "UPDATE token_state SET \
             changed_at = CASE WHEN refreshed_at IS NULL THEN ?2 ELSE changed_at END, \
             refreshed_at = ?2, files = ?3, skipped_lines = ?4, unreadable_files = ?5 \
             WHERE provider = ?1",
            params![
                provider.as_str(),
                now_ms,
                to_sql(stats.files),
                to_sql(stats.skipped_lines),
                to_sql(stats.unreadable_files),
            ],
        )?;
        tx.execute(
            "INSERT INTO token_epoch (at, seeded) SELECT ?1, 1 \
             WHERE NOT EXISTS (SELECT 1 FROM token_epoch)",
            [now_ms],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Retention (spec §4), at most once a day per provider: the lease
    /// `prune:tokens:<provider>` spaces prunes, as `prune_samples` does (core §6.1). Deletes
    /// the provider's responses older than `cutoff_ms`, in chunks of their own transactions,
    /// each of which that deletes any also sets `changed_at`; then its sessions left without
    /// one, the sightings that ended before the cutoff (those of every pruned session, but one
    /// still alive), and the launches that did, and sets `pruned_at`. Model names stay.
    ///
    /// `stop` is asked before each chunk, so a refresh's budget and cancel reach a prune that a
    /// retention cut made large. Stopped, the prune releases its lease and returns
    /// `complete: false`: the next call goes on where it stopped. Another prune's lease, held
    /// or spent today, is nothing to do: `complete: true` with nothing deleted.
    pub fn prune_tokens(
        &self,
        provider: &ProviderId,
        cutoff_ms: i64,
        holder: &str,
        now_ms: i64,
        stop: &dyn Fn() -> bool,
    ) -> Result<PruneOutcome, StoreError> {
        let lease = prune_lease(provider);
        let taken = self.lock().execute(
            TAKE_LEASE_SQL,
            params![lease, holder, now_ms + DAY_MS, now_ms],
        )?;
        if taken != 1 {
            return Ok(PruneOutcome {
                deleted: 0,
                complete: true,
            });
        }
        let mut deleted = 0u64;
        // Phase 0 is the expired responses, then each of `PRUNE_CLEANUP`; every chunk is its own
        // transaction, and `stop` is asked before each.
        let mut phase = 0;
        loop {
            if stop() {
                self.exec(
                    "UPDATE leases SET expires_at = ?3 WHERE name = ?1 AND holder = ?2",
                    &[&lease, &holder, &now_ms],
                )?;
                return Ok(PruneOutcome {
                    deleted,
                    complete: false,
                });
            }
            let mut c = self.lock();
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let n = if phase == 0 {
                let n = prune_chunk(&tx, provider, cutoff_ms, now_ms)?;
                deleted += n as u64;
                n
            } else {
                tx.execute(
                    PRUNE_CLEANUP[phase - 1],
                    params![provider.as_str(), cutoff_ms, PRUNE_CHUNK as i64],
                )?
            };
            let last = phase == PRUNE_CLEANUP.len() && n < PRUNE_CHUNK;
            if last {
                tx.execute(ENSURE_STATE, [provider.as_str()])?;
                tx.execute(
                    "UPDATE token_state SET pruned_at = ?2 WHERE provider = ?1",
                    params![provider.as_str(), now_ms],
                )?;
            }
            tx.commit()?;
            if last {
                break;
            }
            if n < PRUNE_CHUNK {
                phase += 1;
            }
        }
        Ok(PruneOutcome {
            deleted,
            complete: true,
        })
    }

    /// The connection's busy timeout: for the tests that pin `record_sighting_within`
    /// restoring it.
    #[cfg(feature = "test-hooks")]
    pub fn busy_timeout(&self) -> Result<Duration, StoreError> {
        let ms: i64 = self
            .lock()
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))?;
        Ok(Duration::from_millis(from_sql(ms)))
    }

    /// The provider's `token_state` row; `None` before its first refresh wrote one.
    pub fn token_state(&self, provider: &ProviderId) -> Result<Option<TokenStateRow>, StoreError> {
        Ok(self
            .lock()
            .query_row(
                "SELECT refreshed_at, changed_at, files, responses, skipped_lines, \
                 unreadable_files, pruned_at FROM token_state WHERE provider = ?1",
                [provider.as_str()],
                |r| {
                    Ok(TokenStateRow {
                        refreshed_at: r.get(0)?,
                        changed_at: r.get(1)?,
                        stats: RefreshStats {
                            files: from_sql(r.get(2)?),
                            responses: from_sql(r.get(3)?),
                            skipped_lines: from_sql(r.get(4)?),
                            unreadable_files: from_sql(r.get(5)?),
                        },
                        pruned_at: r.get(6)?,
                    })
                },
            )
            .optional()?)
    }

    /// The epoch (spec §7.2); `None` after a full purge until a refresh completes.
    pub fn token_epoch(&self) -> Result<Option<EpochRow>, StoreError> {
        Ok(self
            .lock()
            .query_row(
                "SELECT at, seeded FROM token_epoch ORDER BY rowid LIMIT 1",
                [],
                |r| {
                    Ok(EpochRow {
                        at: r.get(0)?,
                        seeded: r.get::<_, i64>(1)? != 0,
                    })
                },
            )
            .optional()?)
    }

    /// The latest `recorded_at` of the provider's sightings, launches and active-account
    /// history (spec §6.4): with `token_state.changed_at`, what `IndexState.changed_at_ms`
    /// follows (spec §10). `None` with no evidence at all.
    pub fn evidence_changed_at(&self, provider: &ProviderId) -> Result<Option<i64>, StoreError> {
        Ok(self.lock().query_row(
            "SELECT max(m) FROM ( \
             SELECT max(recorded_at) AS m FROM session_sightings WHERE provider = ?1 \
             UNION ALL SELECT max(recorded_at) FROM run_launches WHERE provider = ?1 \
             UNION ALL SELECT max(recorded_at) FROM active_history WHERE provider = ?1)",
            [provider.as_str()],
            |r| r.get(0),
        )?)
    }

    /// The provider's responses at `since_ms` (inclusive) to `until_ms` (exclusive), of one
    /// session when `session` names it, by time, then the order they were first indexed in.
    pub fn token_usage(
        &self,
        provider: &ProviderId,
        since_ms: Option<i64>,
        until_ms: Option<i64>,
        session: Option<&str>,
    ) -> Result<Vec<UsageRow>, StoreError> {
        let c = self.lock();
        let since = since_ms.unwrap_or(i64::MIN);
        let until = until_ms.unwrap_or(i64::MAX);
        let rows = match session {
            Some(id) => c
                .prepare(&format!(
                    "SELECT {USAGE_COLUMNS} FROM token_sessions s \
                     JOIN token_usage u ON u.session = s.id JOIN token_models m ON m.id = u.model \
                     WHERE s.provider = ?1 AND s.session_id = ?4 AND u.at >= ?2 AND u.at < ?3 \
                     ORDER BY u.at, u.id"
                ))?
                .query_map(params![provider.as_str(), since, until, id], usage_from_row)?
                .collect::<Result<Vec<_>, _>>()?,
            None => c
                .prepare(&format!(
                    "SELECT {USAGE_COLUMNS} FROM token_usage u \
                     JOIN token_sessions s ON s.id = u.session JOIN token_models m ON m.id = u.model \
                     WHERE s.provider = ?1 AND u.at >= ?2 AND u.at < ?3 ORDER BY u.at, u.id"
                ))?
                .query_map(params![provider.as_str(), since, until], usage_from_row)?
                .collect::<Result<Vec<_>, _>>()?,
        };
        Ok(rows)
    }

    /// Every model the provider's index has held since its last purge, sorted: a model whose
    /// responses retention pruned is still named (ruling R8).
    pub fn token_models(&self, provider: &ProviderId) -> Result<Vec<String>, StoreError> {
        let c = self.lock();
        let mut stmt =
            c.prepare("SELECT name FROM token_models WHERE provider = ?1 ORDER BY name")?;
        let rows = stmt
            .query_map([provider.as_str()], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The times of the provider's oldest and newest responses, each read from one end of the
    /// time index.
    pub fn token_span(
        &self,
        provider: &ProviderId,
    ) -> Result<(Option<i64>, Option<i64>), StoreError> {
        let c = self.lock();
        let end = |order: &str| -> rusqlite::Result<Option<i64>> {
            c.query_row(
                &format!(
                    "SELECT at FROM token_usage INDEXED BY token_usage_by_at \
                     WHERE session IN ({SESSIONS_OF}) ORDER BY at {order} LIMIT 1"
                ),
                [provider.as_str()],
                |r| r.get(0),
            )
            .optional()
        };
        Ok((end("ASC")?, end("DESC")?))
    }

    /// Spec §6.4: inserts the sighting, or extends the stored one to `s.last_at` when that is
    /// at least `min_gap_ms` past its own `last_at`. An extension sets `last_at` and
    /// `recorded_at` alone: `source` stays the creator's. Returns whether it wrote.
    pub fn record_sighting(
        &self,
        provider: &ProviderId,
        s: &SightingRow,
        min_gap_ms: i64,
    ) -> Result<bool, StoreError> {
        Ok(write_sighting(&self.lock(), provider, s, min_gap_ms)?)
    }

    /// `record_sighting`, giving up after `wait` on another connection's write lock
    /// (`SQLITE_BUSY`) instead of the connection's own, far longer, wait: the statusline's
    /// sighting, which never holds its line up (spec §6.3). The connection stays locked across
    /// the write, so no other thread of this process writes with the short wait.
    pub fn record_sighting_within(
        &self,
        provider: &ProviderId,
        s: &SightingRow,
        min_gap_ms: i64,
        wait: Duration,
    ) -> Result<bool, StoreError> {
        let c = self.lock();
        c.busy_timeout(wait)?;
        let written = write_sighting(&c, provider, s, min_gap_ms);
        c.busy_timeout(super::BUSY_TIMEOUT)?;
        Ok(written?)
    }

    /// `run`'s exit write (spec §6.1): moves the sighting's `last_at` up to `last_at`, never
    /// back, setting `recorded_at` when it moved.
    pub fn set_sighting_last_at(
        &self,
        provider: &ProviderId,
        session_id: &str,
        home: &str,
        first_at: i64,
        last_at: i64,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.exec(
            "UPDATE session_sightings SET last_at = ?5, recorded_at = ?6 \
             WHERE provider = ?1 AND session_id = ?2 AND home = ?3 AND first_at = ?4 \
             AND last_at < ?5",
            &[
                &provider.as_str(),
                &session_id,
                &home,
                &first_at,
                &last_at,
                &now_ms,
            ],
        )?;
        Ok(())
    }

    /// The latest execution's sighting of `session_id` in `home` (spec §6.3).
    pub fn latest_sighting(
        &self,
        provider: &ProviderId,
        session_id: &str,
        home: &str,
    ) -> Result<Option<SightingRow>, StoreError> {
        Ok(self
            .lock()
            .query_row(
                &format!(
                    "SELECT {SIGHTING_COLUMNS} FROM session_sightings \
                     WHERE provider = ?1 AND session_id = ?2 AND home = ?3 \
                     ORDER BY first_at DESC LIMIT 1"
                ),
                params![provider.as_str(), session_id, home],
                sighting_from_row,
            )
            .optional()?)
    }

    /// Every sighting of the provider, by session, then `first_at`, then home.
    pub fn sightings(&self, provider: &ProviderId) -> Result<Vec<SightingRow>, StoreError> {
        let c = self.lock();
        let mut stmt = c.prepare(&format!(
            "SELECT {SIGHTING_COLUMNS} FROM session_sightings WHERE provider = ?1 \
             ORDER BY session_id, first_at, home"
        ))?;
        let rows = stmt
            .query_map([provider.as_str()], sighting_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Spec §6.1: an open launch, recorded before `run` spawns its child. A launch already
    /// recorded at that key is kept as it is.
    pub fn open_launch(
        &self,
        provider: &ProviderId,
        account_id: &str,
        started_at: i64,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.exec(
            "INSERT OR IGNORE INTO run_launches \
             (provider, account_id, started_at, ended_at, pid, recorded_at) \
             VALUES (?1, ?2, ?3, NULL, NULL, ?4)",
            &[&provider.as_str(), &account_id, &started_at, &now_ms],
        )?;
        Ok(())
    }

    /// The child's pid, once `run` has spawned it (spec §6.1).
    pub fn attach_launch_pid(
        &self,
        provider: &ProviderId,
        account_id: &str,
        started_at: i64,
        pid: u32,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.exec(
            "UPDATE run_launches SET pid = ?4, recorded_at = ?5 \
             WHERE provider = ?1 AND account_id = ?2 AND started_at = ?3",
            &[&provider.as_str(), &account_id, &started_at, &pid, &now_ms],
        )?;
        Ok(())
    }

    /// Ends an open launch at `ended_at` (spec §6.1). A launch already ended keeps its end.
    pub fn close_launch(
        &self,
        provider: &ProviderId,
        account_id: &str,
        started_at: i64,
        ended_at: i64,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.exec(
            "UPDATE run_launches SET ended_at = ?4, recorded_at = ?5 \
             WHERE provider = ?1 AND account_id = ?2 AND started_at = ?3 AND ended_at IS NULL",
            &[
                &provider.as_str(),
                &account_id,
                &started_at,
                &ended_at,
                &now_ms,
            ],
        )?;
        Ok(())
    }

    /// Every launch of the provider, by start, then account.
    pub fn launches(&self, provider: &ProviderId) -> Result<Vec<LaunchRow>, StoreError> {
        let c = self.lock();
        let mut stmt = c.prepare(
            "SELECT account_id, started_at, ended_at, pid, recorded_at FROM run_launches \
             WHERE provider = ?1 ORDER BY started_at, account_id",
        )?;
        let rows = stmt
            .query_map([provider.as_str()], |r| {
                Ok(LaunchRow {
                    account_id: r.get(0)?,
                    started_at: r.get(1)?,
                    ended_at: r.get(2)?,
                    pid: r
                        .get::<_, Option<i64>>(3)?
                        .and_then(|p| u32::try_from(p).ok()),
                    recorded_at: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Every change of the provider's active account (spec §4): `(at, account_id)`, by time,
    /// a row with no account meaning none was active.
    pub fn active_history(
        &self,
        provider: &ProviderId,
    ) -> Result<Vec<(i64, Option<String>)>, StoreError> {
        let c = self.lock();
        let mut stmt = c.prepare(
            "SELECT at, account_id FROM active_history WHERE provider = ?1 ORDER BY at, rowid",
        )?;
        let rows = stmt
            .query_map([provider.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}
```

The SQL relies on facts the tests pin against the bundled SQLite: two-argument `min()` and `max()` are scalar; bare column names in `DO UPDATE SET` and its `WHERE` refer to the stored row, and `excluded.` to the proposed one; an `UPDATE`'s right-hand sides all see the row as it was before the update (so `finish_token_refresh`'s `CASE WHEN refreshed_at IS NULL` sees the old value); `RETURNING` works on an upsert's `DO UPDATE`; and `INDEXED BY token_usage_by_at` with `ORDER BY at LIMIT 1` reads one end of the index.

- [ ] **Step 4: Run them and see them pass**

```
cargo test -p tagteam-engine --locked --test store_tokens --test store --test purge
cargo test -p tagteam-engine --locked --features test-hooks --test store_tokens
```

Expected: `store_tokens.rs` `test result: ok. 14 passed; 0 failed`, `store.rs` `ok. 58 passed`, `purge.rs` `ok. 56 passed`. With `test-hooks`, `store_tokens.rs` runs `record_sighting_within_restores_the_connection_s_wait` too: `ok. 15 passed`.

- [ ] **Step 5: Lint**

```
cargo fmt --all --check
cargo clippy -p tagteam-engine --all-targets --locked -- -D warnings
cargo clippy -p tagteam-engine --all-targets --locked --features test-hooks -- -D warnings
```

Expected: no output from `fmt`; clippy finishes with no warning.

- [ ] **Step 6: Commit**

```
git add Cargo.lock crates/tagteam-engine/Cargo.toml crates/tagteam-engine/src/store/tokens.rs crates/tagteam-engine/src/store/mod.rs crates/tagteam-engine/src/store/usage.rs crates/tagteam-engine/tests/store_tokens.rs
git commit -m "Add the store API for the token index and evidence"
```

#### Notes for the controller

1. **Dependency (adopted).** `tagteam-engine` gains `sha2.workspace = true` for `usage_key` (Task 14's head and tail hashes need it too). It is no new crate: the workspace already locks sha2 0.10.9 for `tagteam-core`, `tagteam-cc` and `xtask`, so `Cargo.lock` gains one line. The Global Constraints' "New dependencies: `jiff` only" holds in the sense of no new crate, but the constraint's wording does not mention this; record it if you want it explicit.
2. **`RefreshStats.responses` is not an input (adopted).** Task 15 has no way to count the index's responses, so `token_state.responses` is kept by the store: each batch adds its `added`, each prune chunk subtracts what it deleted, and a purge deletes the row. `finish_token_refresh` ignores `stats.responses`. Spec §4 says "as of the last completed refresh" for `files`; the live count differs only mid-refresh. Recounting at completion instead cost 44–116 ms at a year's volume (1.55M responses) inside the write transaction, which would break the 100 ms "refresh with nothing new" budget (spec §1.1).
3. **Prune deletes sightings by time (adopted).** Spec §4 says "then sessions with no rows and their sightings". This task deletes the sightings whose `last_at` is before the cutoff: those are every sighting of a pruned session, except one whose execution is still alive. That one still places the session's new responses. It also bounds sightings of sessions that never produced a response, which the spec's wording would keep forever. A sighting that ends before the cutoff covers only times before it, so no kept response loses evidence.
4. **Prune runs in chunks, and stops when asked.** It runs as several transactions: the lease, chunks of 5,000 deleted responses (each moving `changed_at` and subtracting its count when it deletes any), then sessions, sightings, launches and `pruned_at`. A prune that a retention change makes large (365 → 30 days at a year's volume: about 1.4M rows) never holds the write lock for long, and `stop` lets a refresh's budget and cancel end it between chunks: it releases its lease and the next refresh goes on (`a_stopped_prune_resumes_on_the_next_call_and_completes`). If it fails part-way, every committed chunk is already visible through `changed_at` (`a_prune_that_fails_part_way_still_moves_changed_at`, which makes the last transaction fail with a trigger), and the next day's prune finishes it.
5. **Performance at a year's volume** (release, 1.55M responses; this task's API):
   - 30-day `token_usage`: 110 ms for 127k rows. The SQL is about 18 ms; the rest is `String` allocation per row (session id, project, model).
   - `token_models`: 45 µs. `token_span`: 6 ms. `token_state` and `evidence_changed_at`: under 0.5 ms.
   - Cold insert in 2,000-row batches: 35 s in total, about 45 ms a batch on average, with the worst batch at 268 ms when SQLite checkpointed the WAL during the commit.

   Task 18's 200 ms query budget therefore has about 90 ms left for attribution and grouping at a year's volume. Task 15 may want batches smaller than 2,000 once the index is large.
6. `delete_provider_rows` deletes the provider's cascade of responses in one transaction (it is the spec's single purge transaction). At a year's volume that is seconds of write lock, acceptable for an explicit `purge --provider`. Task 17 holds the tokens lock around it.

---

### Task 13: Settings: retention and price overrides

**Depends on:** 3
**Files:**
- Modify: `crates/tagteam-engine/src/settings.rs` (the `tokens.retention_days` key through all five registry edits: `KEYS`, the `Settings` field, `Default`, `value`, `apply`, plus `DEFAULT_TOKENS_RETENTION_DAYS`; `Settings.prices` read from every `[provider.<id>.prices."<model>"]` table by `Reader::read_prices`; `place` knows the price tables)
- Modify: `crates/tagteam-engine/tests/settings.rs` (the key lists: defaults, full file, registry order, defaults per key, `expect` phrases, the numeric-key count 6→7, `SAMPLES` 13→14; five new tests)
- Modify: `crates/tagteam-engine/tests/config.rs` (the ranges table, the no-provider-table list, one new test)
- Modify: `crates/tagteam/tests/config_cli.rs` (`config list` human and `--json` key lists)
- Test: `crates/tagteam-engine/tests/settings.rs`, `crates/tagteam-engine/tests/config.rs`, `crates/tagteam/tests/config_cli.rs`

Other key lists, checked and needing no edit: `crates/tagteam/src/cli.rs` and `crates/tagteam/tests/completions.rs` derive theirs from `KEYS`; `every_numeric_key_in_the_registry_refuses_a_step_past_either_end` and `every_key_set_through_the_registry_reads_back_without_a_warning` iterate `KEYS`. `tokens.retention_days` sits right after `usage.history_retention_days` in `KEYS`, which is the order Task 24 gives it in core §6.4's table.

**Interfaces:**
- Consumes: Task 3, `tagteam_core::tokens::{CardOverride, PriceOverride}`:
  ```rust
  #[derive(Debug, Clone, Copy, Default, PartialEq)]
  pub struct CardOverride { pub input: Option<f64>, pub output: Option<f64>, pub cache_write_5m: Option<f64>, pub cache_write_1h: Option<f64>, pub cache_read: Option<f64> }
  #[derive(Debug, Clone, Default, PartialEq)]
  pub struct PriceOverride { pub base: CardOverride, pub long_prompt_above: Option<u64>, pub long_prompt: CardOverride, pub fast: CardOverride, pub web_search_per_1k: Option<f64> }
  ```
- Produces (contract, "Settings (Task 13)"):
  ```rust
  pub const DEFAULT_TOKENS_RETENTION_DAYS: u32 = 365;
  pub struct Settings { /* existing */ pub tokens_retention_days: u32, pub prices: BTreeMap<ProviderId, BTreeMap<String, PriceOverride>> }
  ```
  `prices` holds every provider's overrides, whichever provider `Settings::load` was given, by provider id then model name exactly as written in the file, each field valid as read (a float ≥ 0 and finite; `long_prompt_above` an integer ≥ 1). A model whose table holds no valid field is absent, and so is a provider with no model left. Task 18 passes `settings().prices.get(&q.provider)` (or an empty map) to `Prices::new`, so a report for any provider through any engine gets that provider's own overrides.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/settings.rs`, add to the imports:

```rust
use std::collections::BTreeMap;
```

(above `use std::fs;`) and

```rust
use tagteam_core::tokens::{CardOverride, PriceOverride};
```

(below `use tagteam_core::autoswitch::Strategy;`).

In `the_defaults_are_the_specs_table`, after `assert_eq!(d.history_retention_days, 180);`:

```rust
    assert_eq!(d.tokens_retention_days, 365);
```

and after `assert!(d.share_extra.is_empty());`:

```rust
    assert!(d.prices.is_empty());
```

In `every_key_is_read_from_a_full_file`, the file gains, after `history_retention_days = 30` and its blank line:

```toml
[tokens]
retention_days = 90

```

and, after `share_extra = ["hook-data"]`:

```toml

[provider.claude-code.prices."claude-opus-5-5"]
input = 4.0
```

and the expected `Settings` gains `tokens_retention_days: 90,` after `history_retention_days: 30,` and, after `share_extra: vec!["hook-data".into()],`:

```rust
            prices: BTreeMap::from([(
                ProviderId::new("claude-code"),
                BTreeMap::from([(
                    "claude-opus-5-5".to_owned(),
                    PriceOverride {
                        base: CardOverride {
                            input: Some(4.0),
                            ..CardOverride::default()
                        },
                        ..PriceOverride::default()
                    }
                )])
            )]),
```

In `the_registry_is_section_six_four_s_table_in_order`, after the `usage.history_retention_days` row:

```rust
            (
                "tokens.retention_days",
                KeyKind::Int { min: 30, max: 3650 },
                false
            ),
```

In `every_key_reads_back_its_default_from_the_settings`, after `("usage.history_retention_days", Value::Int(180)),`:

```rust
            ("tokens.retention_days", Value::Int(365)),
```

In `each_key_says_what_a_valid_value_is`, after the `usage.history_retention_days` assertion:

```rust
    assert_eq!(
        expect("tokens.retention_days"),
        "must be a whole number of days from 30 to 3650"
    );
```

In `every_numeric_key_reads_its_bounds_and_defaults_just_outside_them`, the count:

```rust
    assert_eq!(
        numeric, 7,
        "threshold, three seconds and ticks, hysteresis, two retentions"
    );
```

`SAMPLES` becomes `const SAMPLES: [(&str, &str); 14] = [`, with, after `("usage.history_retention_days", "3650"),`:

```rust
    ("tokens.retention_days", "30"),
```

Append to `crates/tagteam-engine/tests/settings.rs`:

```rust
// ---- Token report Task 13: retention and price overrides (token report spec §4, §8.2) ----

#[test]
fn retention_days_reads_and_clamps_like_every_key() {
    // Spec §4 Retention: `tokens.retention_days`, 30 to 3650, 365 by default, the same for
    // every provider.
    for days in [30u32, 365, 3650] {
        let (s, warnings) = load(&format!("[tokens]\nretention_days = {days}\n"));
        assert_eq!(s.tokens_retention_days, days);
        assert!(warnings.is_empty(), "{warnings:?}");
    }
    for text in ["29", "3651", "0", "-5", "1.5", "\"90\"", "true"] {
        let (s, warnings) = load(&format!("[tokens]\nretention_days = {text}\n"));
        assert_eq!(s.tokens_retention_days, 365, "{text}");
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(
            warnings[0].ends_with(
                ": `tokens.retention_days` must be a whole number of days from 30 to 3650 (ignored)"
            ),
            "{warnings:?}"
        );
    }
    let i = inspect_as(
        "[provider.claude-code.tokens]\nretention_days = 40\n",
        PROVIDER,
    );
    let s = state(&i, "tokens.retention_days");
    assert_eq!(
        (&s.value, s.source),
        (&Value::Int(365), Source::Default),
        "no provider table holds it"
    );
    assert_eq!(i.unknown, ["provider.claude-code.tokens.retention_days"]);
    let key = reg("tokens.retention_days");
    assert_eq!(key.parse_arg("90"), Ok(Value::Int(90)));
    assert_eq!(
        key.parse_arg("29"),
        Err("must be a whole number of days from 30 to 3650".to_owned()),
        "a typed value is never clamped"
    );
}

/// A card with the given fields, the rest unset.
fn card(
    input: Option<f64>,
    output: Option<f64>,
    cache_write_5m: Option<f64>,
    cache_write_1h: Option<f64>,
    cache_read: Option<f64>,
) -> CardOverride {
    CardOverride {
        input,
        output,
        cache_write_5m,
        cache_write_1h,
        cache_read,
    }
}

/// `base` alone.
fn base(base: CardOverride) -> PriceOverride {
    PriceOverride {
        base,
        ..PriceOverride::default()
    }
}

/// A price table for each provider, in each TOML spelling: a table header, a dotted key and an
/// inline table.
const PRICES: &str = r#"
[provider.claude-code.prices."claude-opus-5-5"]
input = 4
output = 20.0
cache_write_5m = 5.0
cache_write_1h = 8.0
cache_read = 0.2
web_search_per_1k = 10
long_prompt_above = 200000

[provider.claude-code.prices."claude-opus-5-5".long_prompt]
input = 8.0
output = 30.0
cache_write_5m = 10.0
cache_write_1h = 16.0
cache_read = 0.4

[provider.claude-code.prices."claude-opus-5-5".fast]
input = 8.0
cache_read = 0.4

[provider.claude-code.prices]
"claude-sonnet-5".output = 12.5
"claude-new-1" = { input = 1, output = 2, cache_write_5m = 3, cache_write_1h = 4, cache_read = 0.5 }

[provider.fake-agent.prices."fake-1"]
input = 9.0
"#;

#[test]
fn price_overrides_read_every_field() {
    // Spec §8.2: any field of the base card and `web_search_per_1k`, `long_prompt_above`, and
    // the `long_prompt` and `fast` cards; a whole number reads as a price.
    let (s, warnings) = load(PRICES);
    assert!(warnings.is_empty(), "{warnings:?}");
    let s = &s.prices[&ProviderId::new(PROVIDER)];
    let opus = PriceOverride {
        base: card(Some(4.0), Some(20.0), Some(5.0), Some(8.0), Some(0.2)),
        long_prompt_above: Some(200_000),
        long_prompt: card(Some(8.0), Some(30.0), Some(10.0), Some(16.0), Some(0.4)),
        fast: card(Some(8.0), None, None, None, Some(0.4)),
        web_search_per_1k: Some(10.0),
    };
    assert_eq!(
        *s,
        BTreeMap::from([
            (
                "claude-new-1".to_owned(),
                base(card(Some(1.0), Some(2.0), Some(3.0), Some(4.0), Some(0.5)))
            ),
            ("claude-opus-5-5".to_owned(), opus),
            (
                "claude-sonnet-5".to_owned(),
                base(card(None, Some(12.5), None, None, None))
            ),
        ])
    );
}

#[test]
fn every_provider_s_price_tables_load_whoever_reads_them() {
    // A report for one provider through an engine loaded for another prices it by its own
    // tables: every provider's are read, whichever provider the file is loaded for.
    let fake_1 = BTreeMap::from([(
        "fake-1".to_owned(),
        base(card(Some(9.0), None, None, None, None)),
    )]);
    for reader in [PROVIDER, "fake-agent", "another"] {
        let (s, warnings) = load_as(PRICES, reader);
        assert!(warnings.is_empty(), "{reader}: {warnings:?}");
        assert_eq!(
            s.prices.keys().map(ProviderId::as_str).collect::<Vec<_>>(),
            ["claude-code", "fake-agent"],
            "{reader}"
        );
        assert_eq!(
            s.prices[&ProviderId::new("claude-code")].len(),
            3,
            "{reader}"
        );
        assert_eq!(s.prices[&ProviderId::new("fake-agent")], fake_1, "{reader}");
    }
}

#[test]
fn an_invalid_price_value_warns_and_is_dropped() {
    // Spec §8.2: read forgivingly. An invalid value keeps the shipped one, with a warning, and
    // the rest of the table stands.
    let (s, warnings) = load(
        r#"
[provider.claude-code.prices."claude-opus-5-5"]
input = -1.0
output = "20"
cache_write_5m = nan
cache_write_1h = inf
cache_read = 0.25
web_search_per_1k = true
long_prompt_above = 0
long_prompt = 3

[provider.claude-code.prices."claude-opus-5-5".fast]
output = [40]
input = 8

[provider.claude-code.prices]
"claude-sonnet-5" = 2.0
"claude-haiku-5-5" = { long_prompt_above = 1.5 }
"#,
    );
    assert_eq!(
        s.prices,
        BTreeMap::from([(
            ProviderId::new(PROVIDER),
            BTreeMap::from([(
                "claude-opus-5-5".to_owned(),
                PriceOverride {
                    base: card(None, None, None, None, Some(0.25)),
                    fast: card(Some(8.0), None, None, None, None),
                    ..PriceOverride::default()
                }
            )])
        )]),
        "valid fields stand; a model with none is left out"
    );
    let dollars = "must be a number of US dollars, 0 or more (ignored)";
    let tokens = "must be a whole number of tokens, 1 or more (ignored)";
    let table = "must be a table (ignored)";
    let opus = "provider.claude-code.prices.claude-opus-5-5";
    let prices = "provider.claude-code.prices";
    let mut want = vec![
        format!("`{opus}.input` {dollars}"),
        format!("`{opus}.output` {dollars}"),
        format!("`{opus}.cache_write_5m` {dollars}"),
        format!("`{opus}.cache_write_1h` {dollars}"),
        format!("`{opus}.web_search_per_1k` {dollars}"),
        format!("`{opus}.long_prompt_above` {tokens}"),
        format!("`{opus}.long_prompt` {table}"),
        format!("`{opus}.fast.output` {dollars}"),
        format!("`{prices}.claude-sonnet-5` {table}"),
        format!("`{prices}.claude-haiku-5-5.long_prompt_above` {tokens}"),
    ];
    let mut got: Vec<String> = warnings
        .iter()
        .map(|w| {
            let (file, rest) = w.split_once(": ").expect("a warning names the file first");
            assert!(file.ends_with("config.toml"), "{w}");
            rest.to_owned()
        })
        .collect();
    got.sort();
    want.sort();
    assert_eq!(got, want);

    let (s, warnings) = load("[provider.claude-code]\nprices = 3\n");
    assert!(s.prices.is_empty());
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].ends_with(": `provider.claude-code.prices` must be a table (ignored)"),
        "{warnings:?}"
    );
}

#[test]
fn price_tables_are_not_unknown_keys() {
    // Spec §8.2: the price tables are no registry keys, yet known: `config list` names only
    // what no price table holds.
    let i = inspect_as(
        r#"
[provider.claude-code.prices."claude-opus-5-5"]
input = 4.0
web_search_per_1k = 10.0
long_prompt_above = 200000
inptu = 4.0

[provider.claude-code.prices."claude-opus-5-5".long_prompt]
output = 30.0
web_search_per_1k = 10.0

[provider.claude-code.prices."claude-opus-5-5".fast]
cache_read = 0.4

[provider.claude-code.prices."claude-opus-5-5".turbo]
input = 1.0

[provider.fake-agent.prices.fake-1]
output = 2.0

[prices.claude-opus-5-5]
input = 4.0
"#,
        PROVIDER,
    );
    assert_eq!(
        i.unknown,
        [
            "prices.claude-opus-5-5.input",
            "provider.claude-code.prices.claude-opus-5-5.inptu",
            "provider.claude-code.prices.claude-opus-5-5.long_prompt.web_search_per_1k",
            "provider.claude-code.prices.claude-opus-5-5.turbo.input",
        ]
    );
    assert!(i.warnings.is_empty(), "{:?}", i.warnings);
    assert_eq!(i.keys.len(), KEYS.len(), "a price table is no registry key");
}
```

In `crates/tagteam-engine/tests/config.rs`, in `the_spec_s_ranges_and_spellings_hold_at_both_ends_and_nothing_is_clamped`'s `cases`, after the `usage.history_retention_days` entry:

```rust
        ("tokens.retention_days", &["30", "3650"], &["29", "3651"]),
```

and in `a_key_no_provider_table_may_hold_refuses_under_either_spelling`'s list, after `("usage.history_retention_days", "30"),`:

```rust
        ("tokens.retention_days", "90"),
```

Append to `crates/tagteam-engine/tests/config.rs` (after `mod hooks { … }`, at the top level; `Fx`, `cc`, `write_config`, `config_text`, `refusal` and `Settings` are already in scope):

```rust
#[test]
fn config_set_refuses_a_price_key() {
    // Token report spec §8.2: the price tables are no registry keys, so `config` writes none
    // of them; the user edits the file.
    let fx = Fx::new();
    let text = "[provider.claude-code.prices.\"claude-opus-5-5\"]\ninput = 4.0\n";
    write_config(&fx, text);
    let flag = cc();
    for (name, provider) in [
        ("provider.claude-code.prices.claude-opus-5-5.input", None),
        ("prices.claude-opus-5-5.input", Some(&flag)),
        ("provider.claude-code.prices", None),
    ] {
        assert_eq!(refusal(&fx, name, provider, "5"), "invalid-input", "{name}");
        assert_eq!(
            fx.engine.config_unset(name, provider).unwrap_err().kind(),
            "invalid-input",
            "{name}"
        );
    }
    assert_eq!(config_text(&fx), text, "the file is left as it was");
    let (settings, warnings) = Settings::load(&fx.env, &cc());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        settings.prices[&cc()]["claude-opus-5-5"].base.input,
        Some(4.0)
    );
}
```

In `crates/tagteam/tests/config_cli.rs`, `config_list_shows_every_key_with_its_value_and_source_then_the_unknown_ones` gains, after the `usage.history_retention_days` line:

```rust
            "tokens.retention_days                default   365\n",
```

and `config_list_json_is_the_specs_shape` gains, after the `usage.history_retention_days` row:

```rust
                row("tokens.retention_days", json!(365), json!(365), "default"),
```

- [ ] **Step 2: Run them and see them fail**

```
cargo test -p tagteam-engine --locked --test settings --test config
```

Expected: neither file compiles:

```
error[E0560]: struct `Settings` has no field named `tokens_retention_days`
error[E0560]: struct `Settings` has no field named `prices`
error[E0609]: no field `tokens_retention_days` on type `Settings`
error[E0609]: no field `prices` on type `Settings`
```

```
cargo test -p tagteam --locked --features test-support --test config_cli
```

Expected: `config_list_shows_every_key_with_its_value_and_source_then_the_unknown_ones` and `config_list_json_is_the_specs_shape` fail, each `assertion `left == right` failed`, the expected side holding the `tokens.retention_days` row.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/settings.rs`, the imports:

```rust
use std::io::ErrorKind;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::time::SystemTime;

use tagteam_core::autoswitch::Strategy;
use tagteam_core::{CLAUDE_CODE, ProviderId};
```

become:

```rust
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::time::SystemTime;

use tagteam_core::autoswitch::Strategy;
use tagteam_core::tokens::{CardOverride, PriceOverride};
use tagteam_core::{CLAUDE_CODE, ProviderId};
```

After `pub const DEFAULT_HISTORY_RETENTION_DAYS: u32 = 180;`:

```rust
pub const DEFAULT_TOKENS_RETENTION_DAYS: u32 = 365;
```

After `const OWN_PREFIX: &str = ".tagteam-";`:

```rust

/// The fields of a rate card in a price table (token report spec §8.2), each USD per million
/// tokens: a model's own, and those of its `long_prompt` and `fast` tables.
const CARD_FIELDS: [&str; 5] = [
    "input",
    "output",
    "cache_write_5m",
    "cache_write_1h",
    "cache_read",
];

/// The tables of rate cards a price table may hold beside its own card.
const CARD_TABLES: [&str; 2] = ["long_prompt", "fast"];

/// A price table's fields that are not on a card.
const PRICE_FIELDS: [&str; 2] = ["web_search_per_1k", "long_prompt_above"];
```

In `KEYS`, after the `usage.history_retention_days` entry:

```rust
    Key {
        name: "tokens.retention_days",
        kind: KeyKind::Int { min: 30, max: 3650 },
        per_provider: false,
    },
```

In `pub struct Settings`, after `pub history_retention_days: u32,`:

```rust
    /// `tokens.retention_days`: 30–3650 (token report spec §4 Retention).
    pub tokens_retention_days: u32,
```

and after `pub share_extra: Vec<String>,`:

```rust
    /// `[provider.<id>.prices."<model>"]` (token report spec §8.2): every provider's price
    /// overrides, by provider then model, whichever provider the file was loaded for; each
    /// field as read, one that is not valid dropped with a warning. Not a registry key:
    /// `config` neither lists nor writes it.
    pub prices: BTreeMap<ProviderId, BTreeMap<String, PriceOverride>>,
```

In `impl Default for Settings`, after `history_retention_days: DEFAULT_HISTORY_RETENTION_DAYS,`:

```rust
            tokens_retention_days: DEFAULT_TOKENS_RETENTION_DAYS,
```

and after `share_extra: Vec::new(),`:

```rust
            prices: BTreeMap::new(),
```

In `Settings::value`, after the `"usage.history_retention_days"` arm:

```rust
            "tokens.retention_days" => Value::Int(self.tokens_retention_days.into()),
```

In `Settings::apply`, after the `("usage.history_retention_days", Value::Int(n))` arm:

```rust
            ("tokens.retention_days", Value::Int(n)) => self.tokens_retention_days = whole(n),
```

`whole`'s doc comment becomes:

```rust
/// An `Int` key's value as a `u32` field holds it: the ranges of `autoswitch.unhealthy_ticks`,
/// `usage.history_retention_days` and `tokens.retention_days` lie within `u32`. The two seconds
/// keys are `i64`, as M3b's `AutoConfig` takes them.
```

In `read_file`, the line before `read.warnings = reader.warnings;` is new:

```rust
    read.settings.prices = reader.read_prices();
    read.warnings = reader.warnings;
```

Replace the closing `}` of `impl<'a> Reader<'a>` (the line right after `read_key`'s body) with the four new methods, that closing brace, and two parsers after it:

```rust

    /// Every `[provider.<id>.prices]` table (token report spec §8.2), by provider: one table
    /// per model. A field that is not valid, or a table that is not one, warns and is dropped;
    /// the rest stand. A model whose table holds no valid field is left out, and so is a
    /// provider with no model left.
    fn read_prices(&mut self) -> BTreeMap<ProviderId, BTreeMap<String, PriceOverride>> {
        let doc: &'a DocumentMut = self.doc;
        let mut all = BTreeMap::new();
        let Some(providers) = doc.get("provider").and_then(Item::as_table_like) else {
            return all;
        };
        for (id, item) in providers.iter() {
            let Some(item) = item.as_table_like().and_then(|t| t.get("prices")) else {
                continue;
            };
            let Some(models) = item.as_table_like() else {
                self.warn(format!(
                    "{}: `provider.{id}.prices` must be a table (ignored)",
                    self.path
                ));
                continue;
            };
            let prices = self.read_provider_prices(id, models);
            if !prices.is_empty() {
                all.insert(ProviderId::new(id), prices);
            }
        }
        all
    }

    /// One provider's price tables, `models`, which sits at `provider.<id>.prices`.
    fn read_provider_prices(
        &mut self,
        provider: &str,
        models: &dyn TableLike,
    ) -> BTreeMap<String, PriceOverride> {
        let mut prices = BTreeMap::new();
        for (model, item) in models.iter() {
            let dotted = format!("provider.{provider}.prices.{model}");
            let Some(table) = item.as_table_like() else {
                self.warn(format!(
                    "{}: `{dotted}` must be a table (ignored)",
                    self.path
                ));
                continue;
            };
            let mut price = PriceOverride {
                base: self.read_card(table, &dotted),
                web_search_per_1k: self.read_field(table, &dotted, "web_search_per_1k", rate),
                long_prompt_above: self.read_field(
                    table,
                    &dotted,
                    "long_prompt_above",
                    token_count,
                ),
                ..PriceOverride::default()
            };
            for name in CARD_TABLES {
                let Some(item) = table.get(name) else {
                    continue;
                };
                let at = format!("{dotted}.{name}");
                let Some(inner) = item.as_table_like() else {
                    self.warn(format!("{}: `{at}` must be a table (ignored)", self.path));
                    continue;
                };
                let card = self.read_card(inner, &at);
                match name {
                    "long_prompt" => price.long_prompt = card,
                    _ => price.fast = card,
                }
            }
            if price != PriceOverride::default() {
                prices.insert(model.to_owned(), price);
            }
        }
        prices
    }

    /// The card fields of `table`, which sits at `dotted`.
    fn read_card(&mut self, table: &dyn TableLike, dotted: &str) -> CardOverride {
        let [input, output, cache_write_5m, cache_write_1h, cache_read] =
            CARD_FIELDS.map(|field| self.read_field(table, dotted, field, rate));
        CardOverride {
            input,
            output,
            cache_write_5m,
            cache_write_1h,
            cache_read,
        }
    }

    /// `field` of `table`, read by `parse`, or `None` when it is absent or not valid; one that
    /// is not valid warns with the `Err` text.
    fn read_field<T>(
        &mut self,
        table: &dyn TableLike,
        dotted: &str,
        field: &str,
        parse: fn(&Item) -> Result<T, &'static str>,
    ) -> Option<T> {
        let item = table.get(field)?;
        match parse(item) {
            Ok(value) => Some(value),
            Err(expect) => {
                self.warn(format!(
                    "{}: `{dotted}.{field}` {expect} (ignored)",
                    self.path
                ));
                None
            }
        }
    }
}

/// A price in US dollars, per million tokens or per thousand searches: a number, 0 or more.
fn rate(item: &Item) -> Result<f64, &'static str> {
    item.as_float()
        .or_else(|| item.as_integer().map(|i| i as f64))
        .filter(|n| n.is_finite() && *n >= 0.0)
        .ok_or("must be a number of US dollars, 0 or more")
}

/// `long_prompt_above`: a prompt size in tokens, 1 or more.
fn token_count(item: &Item) -> Result<u64, &'static str> {
    item.as_integer()
        .and_then(|n| u64::try_from(n).ok())
        .filter(|n| *n >= 1)
        .ok_or("must be a whole number of tokens, 1 or more")
}
```

`place` and a new `place_in_prices`, replacing the whole of `place` (doc comment included):

```rust
/// `path`, from the top of the file. `provider` and `provider.<id>` are tables for any id, and
/// under them only a per-provider key, and its table, are known, and the price tables.
fn place(path: &[&str]) -> Place {
    match path {
        ["provider"] | ["provider", _] => Place::Table,
        ["provider", _, "prices", rest @ ..] => place_in_prices(rest),
        ["provider", _, rest @ ..] => place_in(rest, true),
        rest => place_in(rest, false),
    }
}

/// Under `provider.<id>.prices` (token report spec §8.2): a table per model, holding its card
/// fields, `web_search_per_1k`, `long_prompt_above` and the `long_prompt` and `fast` tables
/// of card fields.
fn place_in_prices(path: &[&str]) -> Place {
    match path {
        [] | [_] => Place::Table,
        [_, field] if CARD_FIELDS.contains(field) || PRICE_FIELDS.contains(field) => Place::Key,
        [_, table] if CARD_TABLES.contains(table) => Place::Table,
        [_, table, field] if CARD_TABLES.contains(table) && CARD_FIELDS.contains(field) => {
            Place::Key
        }
        _ => Place::Unknown,
    }
}
```

`resolve`, `config set` and `config unset` need no change: `prices.…` is no registry key, so `resolve` refuses it with `UnknownKey` (`invalid-input`), which `config_set_refuses_a_price_key` pins. A misspelled price field (`inptu`) is no warning: like any key the registry does not know, `config list` names it under "Unknown keys" (`price_tables_are_not_unknown_keys`).

- [ ] **Step 4: Run them and see them pass**

```
cargo test -p tagteam-engine --locked --test settings --test config
cargo test -p tagteam --locked --features test-support --test config_cli
```

Expected: `settings.rs` `test result: ok. 87 passed; 0 failed`, `config.rs` `ok. 34 passed` (35 with `--features test-hooks`, which adds an existing hooks test), `config_cli.rs` `ok. 15 passed`.

- [ ] **Step 5: Lint**

```
cargo fmt --all --check
cargo clippy -p tagteam-engine --all-targets --locked -- -D warnings
cargo clippy -p tagteam --all-targets --locked --features test-support -- -D warnings
```

Expected: no output from `fmt`; both clippy runs finish with no warning.

- [ ] **Step 6: Commit**

```
git add crates/tagteam-engine/src/settings.rs crates/tagteam-engine/tests/settings.rs crates/tagteam-engine/tests/config.rs crates/tagteam/tests/config_cli.rs
git commit -m "Add the token retention setting and price overrides"
```

#### Notes for the controller

1. **Warning wording** (new phrases, which Task 19's §11.3 "invalid price overrides" prints as they are):
   - an invalid price: "`<dotted>` must be a number of US dollars, 0 or more (ignored)";
   - an invalid `long_prompt_above`: "must be a whole number of tokens, 1 or more (ignored)";
   - a price table that is not a table: "must be a table (ignored)", the existing phrase.

   The dotted name joins the model name unquoted (`provider.claude-code.prices.claude-opus-5-5.input`), as `unknown_keys` names keys.
2. **What a bad table does.** An override whose `base` is incomplete for a model the shipped table lacks is not dropped here: `Settings` cannot know the shipped table. Task 3's `Prices::new` ignores it, as the outline says. No warning reports it, which spec §8.2 does not require.
3. A model whose table yields no valid field is left out of `prices` entirely, so `Prices::new` never lists it in `price_overrides`.
4. **Every provider's prices load (fix after review).** `Settings.prices` is keyed by provider and holds every `[provider.<id>.prices]` table, whichever provider `Settings::load` was given, so `token_report` for any provider through any engine reads `settings().prices.get(&q.provider)` (`every_provider_s_price_tables_load_whoever_reads_them`). A `provider.<id>` entry that is not a table is skipped silently, as the existing reader does for another provider's tables; one whose `prices` is not a table warns.

---

### Task 14: The transcript file reader

**Depends on:** 7, 12
**Files:**
- Modify: `crates/tagteam-engine/src/lib.rs` (`pub mod tokens;`)
- Create: `crates/tagteam-engine/src/tokens/mod.rs` (final form: the four modules, their glob re-exports, `tokens_lock_path`)
- Create: `crates/tagteam-engine/src/tokens/reader.rs` (one file: where to start, chunks, lines, checkpoints)
- Create: `crates/tagteam-engine/src/tokens/refresh.rs` (one contract item, `Wait`; Task 15 fills the file)
- Create: `crates/tagteam-engine/src/tokens/evidence.rs` (one contract item, `LaunchId`; Task 16 fills the file)
- Create: `crates/tagteam-engine/src/tokens/view.rs` (one contract item, `TokenPeriod`; Task 18 fills the file)
- Test: `crates/tagteam-engine/src/tokens/reader.rs` and `crates/tagteam-engine/src/tokens/mod.rs` (unit tests)

**Interfaces:**
- Consumes:
  - Task 12: `pub struct TokenFileRow { pub path: String, pub dev: u64, pub ino: u64, pub size: i64, pub mtime_ns: i64, pub offset: u64, pub head_hash: [u8; 8], pub tail_hash: [u8; 8] }` (in `crate::store`).
  - Existing: `tagteam_provider::{Cancel, Env}`, `tagteam_core::{AccountId, Fingerprint, ProviderId}` (`Fingerprint::of_secret(&[u8])`, `short12()`).
- Produces (contract, `tagteam_engine::tokens`). Each placeholder holds exactly one final contract item, which the task that fills the file keeps word for word:
  ```rust
  pub fn tokens_lock_path(env: &Env, provider: &ProviderId) -> PathBuf; // data_dir/locks/tokens-<id>.lock, same fallback naming as auto::engine_lock_path
  #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Wait { Wait, Try }                                          // refresh.rs, kept by Task 15
  #[derive(Debug, Clone, PartialEq, Eq)] pub struct LaunchId { pub provider: ProviderId, pub account: AccountId, pub started_at_ms: i64 } // evidence.rs, kept by Task 16
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)] pub enum TokenPeriod { Day, Month, Session, Block }               // view.rs, kept by Task 18
  ```
- Produces (crate-internal, `crate::tokens::reader`, for Task 15):
  ```rust
  pub(crate) const CHUNK: usize = 1 << 20;
  pub(crate) const MAX_LINE: usize = 64 << 20;
  #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub(crate) struct Limits { pub chunk: usize, pub max_line: usize }
  impl Limits { pub(crate) const SPEC: Limits; }
  pub(crate) fn past(deadline: Option<Instant>) -> bool;
  pub(crate) fn unchanged(meta: &std::fs::Metadata, stored: Option<&TokenFileRow>) -> bool;
  pub(crate) fn open_at(path: &Path, stored: Option<&TokenFileRow>) -> io::Result<OpenFile>;
  pub(crate) struct OpenFile; impl OpenFile { pub(crate) fn start(&self) -> u64; }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub(crate) enum Stop { Eof, Deadline, Cancelled }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub(crate) enum Step { More, Stop(Stop) }
  pub(crate) struct LineReader<'f>;
  impl<'f> LineReader<'f> {
      pub(crate) fn new(open: &'f OpenFile, limits: Limits) -> Self;
      pub(crate) fn step(&mut self, deadline: Option<Instant>, cancel: &Cancel, on_line: &mut dyn FnMut(&[u8])) -> io::Result<Step>;
      pub(crate) fn consumed(&self) -> u64;
      pub(crate) fn long_lines(&self) -> u64;
      pub(crate) fn peak(&self) -> usize;
      pub(crate) fn checkpoint(&self, at_eof: bool) -> io::Result<Checkpoint>;
  }
  #[derive(Debug, Clone, PartialEq, Eq)] pub(crate) enum Checkpoint { Valid(TokenFileRow), Rewritten(TokenFileRow) }
  ```

The reader is single-file and thread-free: it owns no thread and touches no store. The refresh (Task 15) owns the threads, the writer and the store. There is no `scan_sessions` stub: Task 16 defines `scan_sessions` and runs before Task 15.

- [ ] **Step 1: Write the failing tests**

Add the module to `crates/tagteam-engine/src/lib.rs`, between `mod testutil;` and `pub mod transfer;`:

```rust
#[cfg(test)]
mod testutil;
pub mod tokens;
pub mod transfer;
```

Create `crates/tagteam-engine/src/tokens/mod.rs` with the reader's module and the lock path's test (Step 3 replaces this file with its final form, which keeps the same test):

```rust
//! The token report: the index of a provider's transcripts, the evidence that places their
//! sessions, and the report over them.

mod reader;

#[cfg(test)]
mod tests {
    use tagteam_core::ProviderId;
    use tagteam_provider::Env;

    use super::*;

    #[test]
    fn the_tokens_lock_is_in_locks_and_a_stored_string_never_escapes_it() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let locks = env.data_dir().join("locks");
        assert_eq!(
            tokens_lock_path(&env, &ProviderId::new("claude-code")),
            locks.join("tokens-claude-code.lock")
        );
        let hostile = tokens_lock_path(&env, &ProviderId::new("/../../escape"));
        assert_eq!(hostile.parent(), Some(locks.as_path()));
        let name = hostile.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("tokens-") && name.ends_with(".lock") && !name.contains("escape"),
            "{name}"
        );
        assert_eq!(name.len(), "tokens-".len() + 12 + ".lock".len(), "{name}");
    }
}
```

Create `crates/tagteam-engine/src/tokens/reader.rs` with its doc lines and its tests (Step 3 inserts the implementation between them):

```rust
//! One transcript file, read incrementally (spec §5.2 steps 2 to 4): whether to read it, where
//! its read starts, its whole lines chunk by chunk, and the checkpoint a batch commits. It owns
//! no thread and touches no store.

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::time::Duration;

    use super::*;

    const SMALL: Limits = Limits {
        chunk: 8,
        max_line: 64,
    };

    fn append(path: &Path, bytes: &[u8]) {
        OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }

    /// Every line of `open` from its start, why the read stopped, and the reader.
    fn read_lines<'f>(
        open: &'f OpenFile,
        limits: Limits,
        deadline: Option<Instant>,
        cancel: &Cancel,
    ) -> (Vec<String>, Stop, LineReader<'f>) {
        let mut reader = LineReader::new(open, limits);
        let mut lines = Vec::new();
        let stop = loop {
            let step = reader
                .step(deadline, cancel, &mut |line: &[u8]| {
                    lines.push(String::from_utf8(line.to_vec()).unwrap())
                })
                .unwrap();
            if let Step::Stop(stop) = step {
                break stop;
            }
        };
        (lines, stop, reader)
    }

    /// Reads `path` to its end from where `stored` lets it start: its lines, and the row a
    /// batch commits for it.
    fn index(path: &Path, stored: Option<&TokenFileRow>) -> (Vec<String>, TokenFileRow) {
        let open = open_at(path, stored).unwrap();
        let (lines, stop, reader) = read_lines(&open, Limits::SPEC, None, &Cancel::new());
        assert_eq!(stop, Stop::Eof);
        match reader.checkpoint(true).unwrap() {
            Checkpoint::Valid(row) => (lines, row),
            Checkpoint::Rewritten(row) => panic!("rewritten: {row:?}"),
        }
    }

    fn meta(path: &Path) -> Metadata {
        fs::metadata(path).unwrap()
    }

    #[test]
    fn an_unchanged_file_is_skipped() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, b"one\ntwo\n").unwrap();
        let (lines, row) = index(&path, None);
        assert_eq!(lines, ["one", "two"]);
        assert_eq!((row.offset, row.size), (8, 8));
        assert!(unchanged(&meta(&path), Some(&row)));
        assert!(!unchanged(&meta(&path), None), "a file with no row is read");
    }

    #[test]
    fn a_partial_checkpoint_is_never_skipped() {
        // A read the budget stopped stores size -1 (spec §5.2 step 4): the next refresh reads
        // the file again, from the offset it reached.
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, b"one\ntwo\nthree\n").unwrap();
        let open = open_at(&path, None).unwrap();
        let (lines, stop, reader) =
            read_lines(&open, Limits::SPEC, Some(Instant::now()), &Cancel::new());
        assert_eq!((lines, stop), (vec!["one".to_owned()], Stop::Deadline));
        let Checkpoint::Valid(row) = reader.checkpoint(false).unwrap() else {
            panic!("nothing changed");
        };
        assert_eq!((row.offset, row.size), (4, -1));
        assert!(!unchanged(&meta(&path), Some(&row)));
        let (rest, row) = index(&path, Some(&row));
        assert_eq!(rest, ["two", "three"]);
        assert_eq!((row.offset, row.size), (14, 14));
    }

    #[test]
    fn an_append_resumes_at_the_offset() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, b"one\n").unwrap();
        let (_, row) = index(&path, None);
        append(&path, b"two\nthree\n");
        assert!(!unchanged(&meta(&path), Some(&row)));
        assert_eq!(open_at(&path, Some(&row)).unwrap().start(), 4);
        let (lines, row) = index(&path, Some(&row));
        assert_eq!(lines, ["two", "three"]);
        assert_eq!(row.offset, 14);
    }

    #[test]
    fn a_shrunk_or_replaced_file_restarts() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, b"one\ntwo\n").unwrap();
        let (_, row) = index(&path, None);
        // Shorter than the offset, on the same inode.
        fs::write(&path, b"x\n").unwrap();
        assert_eq!(open_at(&path, Some(&row)).unwrap().start(), 0);
        // Another inode in its place, with the same first bytes.
        fs::write(&path, b"one\ntwo\n").unwrap();
        let (_, row) = index(&path, None);
        let other = d.path().join("other");
        fs::write(&other, b"one\ntwo\nthree\n").unwrap();
        fs::rename(&other, &path).unwrap();
        assert_ne!(meta(&path).ino(), row.ino);
        assert_eq!(open_at(&path, Some(&row)).unwrap().start(), 0);
        let (lines, _) = index(&path, Some(&row));
        assert_eq!(lines, ["one", "two", "three"]);
    }

    #[test]
    fn a_rewrite_with_regrowth_on_the_same_inode_restarts() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, b"one\ntwo\n").unwrap();
        let (_, row) = index(&path, None);
        // Truncated and rewritten in place, past the old offset.
        let mut f = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        f.write_all(b"uno\ndos\ntres\n").unwrap();
        drop(f);
        assert_eq!(meta(&path).ino(), row.ino, "the same inode");
        assert_eq!(open_at(&path, Some(&row)).unwrap().start(), 0);
        let (lines, _) = index(&path, Some(&row));
        assert_eq!(lines, ["uno", "dos", "tres"], "read again from the start");
    }

    #[test]
    fn a_rewrite_during_the_read_is_caught_by_the_checkpoint() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        // A resumed read: the bytes its start was trusted on change before the batch commits.
        fs::write(&path, b"one\ntwo\n").unwrap();
        let (_, row) = index(&path, None);
        append(&path, b"three\n");
        let open = open_at(&path, Some(&row)).unwrap();
        assert_eq!(open.start(), 8);
        let (lines, _, reader) = read_lines(&open, Limits::SPEC, None, &Cancel::new());
        assert_eq!(lines, ["three"]);
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .write_all_at(b"ONE", 0)
            .unwrap();
        match reader.checkpoint(true).unwrap() {
            Checkpoint::Rewritten(row) => assert_eq!((row.offset, row.size), (0, -1)),
            other => panic!("{other:?}"),
        }
        // A read from the start: the bytes it read change under it.
        fs::write(&path, b"one\ntwo\n").unwrap();
        let open = open_at(&path, None).unwrap();
        let (_, _, reader) = read_lines(&open, Limits::SPEC, None, &Cancel::new());
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .write_all_at(b"TWO", 4)
            .unwrap();
        assert!(matches!(
            reader.checkpoint(true).unwrap(),
            Checkpoint::Rewritten(_)
        ));
    }

    #[test]
    fn a_head_rewrite_during_a_first_read_is_caught() {
        // A read from 0: the file's first bytes change after they were parsed, and its last ones
        // stay. The head the checkpoint stores is the bytes parsed, so the change is caught; were
        // it hashed from the file, the next refresh would resume past the rewritten response.
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        let line = format!("{}\n", "x".repeat(99));
        fs::write(&path, line.repeat(100)).unwrap();
        let open = open_at(&path, None).unwrap();
        let (lines, stop, reader) = read_lines(&open, Limits::SPEC, None, &Cancel::new());
        assert_eq!((lines.len(), stop), (100, Stop::Eof));
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .write_all_at(b"y", 0)
            .unwrap();
        assert!(matches!(
            reader.checkpoint(true).unwrap(),
            Checkpoint::Rewritten(_)
        ));
    }

    #[test]
    fn lines_assemble_across_chunks() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        let text = "a line longer than a chunk\nshort\nanother line that spans chunks\n";
        fs::write(&path, text).unwrap();
        let open = open_at(&path, None).unwrap();
        let (lines, stop, reader) = read_lines(&open, SMALL, None, &Cancel::new());
        assert_eq!(
            lines,
            [
                "a line longer than a chunk",
                "short",
                "another line that spans chunks"
            ]
        );
        assert_eq!((stop, reader.consumed()), (Stop::Eof, text.len() as u64));
        assert!(matches!(
            reader.checkpoint(true).unwrap(),
            Checkpoint::Valid(_)
        ));
    }

    #[test]
    fn a_trailing_partial_line_waits() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, b"one\ntwo\npart").unwrap();
        let (lines, row) = index(&path, None);
        assert_eq!(lines, ["one", "two"]);
        assert_eq!(
            (row.offset, row.size),
            (8, 12),
            "read to its last newline, with the size it was opened at"
        );
        assert!(
            unchanged(&meta(&path), Some(&row)),
            "nothing new while the line stays partial"
        );
        append(&path, b"ial\n");
        let (lines, row) = index(&path, Some(&row));
        assert_eq!(lines, ["partial"]);
        assert_eq!(row.offset, 16);
    }

    #[test]
    fn a_huge_line_is_skipped_and_counted_without_holding_the_file() {
        // Review Focus 1: chunks of 16 bytes and a cap of 64 stand in for 1 MiB and 64 MiB.
        let limits = Limits {
            chunk: 16,
            max_line: 64,
        };
        let mut text = b"before\n".to_vec();
        text.extend(vec![b'x'; 1_000]);
        text.push(b'\n');
        // Exactly the cap: parsed.
        text.extend(vec![b'y'; 64]);
        text.push(b'\n');
        // One over it: skipped.
        text.extend(vec![b'z'; 65]);
        text.push(b'\n');
        text.extend(b"after\n");
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, &text).unwrap();
        let open = open_at(&path, None).unwrap();
        let (lines, stop, reader) = read_lines(&open, limits, None, &Cancel::new());
        assert_eq!(
            lines,
            ["before".to_owned(), "y".repeat(64), "after".to_owned()]
        );
        assert_eq!((stop, reader.long_lines()), (Stop::Eof, 2));
        assert_eq!(
            reader.consumed(),
            text.len() as u64,
            "a skipped line is consumed"
        );
        assert!(
            reader.peak() <= limits.chunk + limits.max_line,
            "held {} bytes",
            reader.peak()
        );
        // The tail it hashes is the bytes it read, the skipped ones included.
        assert!(matches!(
            reader.checkpoint(true).unwrap(),
            Checkpoint::Valid(row) if row.offset == text.len() as u64
        ));
    }

    #[test]
    fn the_deadline_stops_between_lines() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, b"one\ntwo\nthree\n").unwrap();
        let open = open_at(&path, None).unwrap();
        // Past it from the start: one line, never none.
        let (lines, stop, reader) =
            read_lines(&open, Limits::SPEC, Some(Instant::now()), &Cancel::new());
        assert_eq!((lines, stop), (vec!["one".to_owned()], Stop::Deadline));
        assert_eq!(reader.consumed(), 4);
        // A line still being assembled is finished first.
        let long = "x".repeat(40);
        fs::write(&path, format!("{long}\nnext\n")).unwrap();
        let open = open_at(&path, None).unwrap();
        let (lines, stop, reader) = read_lines(
            &open,
            SMALL,
            Some(Instant::now() - Duration::from_millis(1)),
            &Cancel::new(),
        );
        assert_eq!((lines, stop), (vec![long], Stop::Deadline));
        assert_eq!(reader.consumed(), 41);
    }

    #[test]
    fn a_cancelled_read_stops_before_its_next_chunk() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, b"one\n").unwrap();
        let open = open_at(&path, None).unwrap();
        let cancel = Cancel::new();
        cancel.request(2);
        let (lines, stop, reader) = read_lines(&open, Limits::SPEC, None, &cancel);
        assert_eq!((lines, stop), (Vec::<String>::new(), Stop::Cancelled));
        assert_eq!(reader.consumed(), 0);
    }
}
```

- [ ] **Step 2: Run them and see them fail**

```
cargo test -p tagteam-engine --locked --lib tokens::
```

Expected: the crate does not compile; the errors include `cannot find function `tokens_lock_path` in this scope`, `cannot find function `open_at` in this scope`, `cannot find type `Limits` in this scope` and `cannot find type `LineReader` in this scope`.

- [ ] **Step 3: Implement**

The reader hashes with `sha2`, which Task 12 already added to `tagteam-engine` for `usage_key`.

Replace `crates/tagteam-engine/src/tokens/mod.rs` with its final form. Task 17 adds `lock_until_free` to it; no other task edits it:

```rust
//! The token report: the index of a provider's transcripts, the evidence that places their
//! sessions, and the report over them.

mod evidence;
mod reader;
mod refresh;
mod view;

use std::path::PathBuf;

use tagteam_core::{Fingerprint, ProviderId};
use tagteam_provider::Env;

pub use evidence::*;
pub use refresh::*;
pub use view::*;

/// `locks/tokens-<provider>.lock` (spec §5.1): held by one refresh of the provider at a time, a
/// leaf. A provider ID that is not a plain name (anything outside `[A-Za-z0-9._-]`, or one
/// starting with `.`) cannot name a path: its lock is named by the first 12 hex digits of the
/// ID's SHA-256, inside `locks/`, as `engine_lock_path` names an engine lock.
pub fn tokens_lock_path(env: &Env, provider: &ProviderId) -> PathBuf {
    let id = provider.as_str();
    let plain = !id.is_empty()
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    let name = if plain {
        format!("tokens-{id}.lock")
    } else {
        format!(
            "tokens-{}.lock",
            Fingerprint::of_secret(id.as_bytes()).short12()
        )
    };
    env.data_dir().join("locks").join(name)
}

#[cfg(test)]
mod tests {
    use tagteam_core::ProviderId;
    use tagteam_provider::Env;

    use super::*;

    #[test]
    fn the_tokens_lock_is_in_locks_and_a_stored_string_never_escapes_it() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let locks = env.data_dir().join("locks");
        assert_eq!(
            tokens_lock_path(&env, &ProviderId::new("claude-code")),
            locks.join("tokens-claude-code.lock")
        );
        let hostile = tokens_lock_path(&env, &ProviderId::new("/../../escape"));
        assert_eq!(hostile.parent(), Some(locks.as_path()));
        let name = hostile.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("tokens-") && name.ends_with(".lock") && !name.contains("escape"),
            "{name}"
        );
        assert_eq!(name.len(), "tokens-".len() + 12 + ".lock".len(), "{name}");
    }
}
```

Create `crates/tagteam-engine/src/tokens/refresh.rs`. Task 15 keeps `Wait` word for word:

```rust
//! `refresh_tokens` (spec §5): the incremental index of a provider's transcripts.

/// How a refresh takes the provider's tokens lock (spec §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// Waits for another refresh to finish, cancellably: `tagteam tokens`.
    Wait,
    /// Returns `Busy` at once while another refresh holds it: the TUI's timer and the daemon.
    Try,
}
```

Create `crates/tagteam-engine/src/tokens/evidence.rs`. Task 16 keeps `LaunchId` word for word:

```rust
//! Session evidence (spec §6): sightings of sessions in homes, and `run`'s launches.

use tagteam_core::{AccountId, ProviderId};

/// One launch `run` recorded (spec §6.1), as `open_launch` returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchId {
    pub provider: ProviderId,
    pub account: AccountId,
    pub started_at_ms: i64,
}
```

Create `crates/tagteam-engine/src/tokens/view.rs`. Task 18 keeps `TokenPeriod` word for word:

```rust
//! The token report view (spec §10).

/// What a report row spans (spec §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenPeriod {
    Day,
    Month,
    Session,
    Block,
}
```

In `crates/tagteam-engine/src/tokens/reader.rs`, insert the implementation between the `//!` lines and `#[cfg(test)]`. The `#![allow(dead_code)]` stays only until Task 15 reads with the reader, and Task 15 removes it:

```rust
#![allow(dead_code)]

use std::fs::{self, File, Metadata};
use std::io;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::Instant;

use sha2::{Digest, Sha256};
use tagteam_provider::Cancel;

use crate::store::TokenFileRow;

/// The most bytes read at a time (spec §5.2 step 3).
pub(crate) const CHUNK: usize = 1 << 20;
/// The longest line parsed; a longer one is skipped and counted (spec §5.2 step 3, §5.4).
pub(crate) const MAX_LINE: usize = 64 << 20;
/// The most bytes `head_hash` and `tail_hash` cover (spec §4).
const HASH_SPAN: u64 = 4096;

/// A reader's sizes: the spec's, or smaller ones a test can cross.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limits {
    pub chunk: usize,
    pub max_line: usize,
}

impl Limits {
    pub(crate) const SPEC: Limits = Limits {
        chunk: CHUNK,
        max_line: MAX_LINE,
    };
}

/// The first 8 bytes of the SHA-256 of `bytes` (spec §4).
fn hash8(bytes: &[u8]) -> [u8; 8] {
    let mut out = [0; 8];
    out.copy_from_slice(&Sha256::digest(bytes)[..8]);
    out
}

/// Whether `deadline` has passed; `None` never does.
pub(crate) fn past(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|d| Instant::now() >= d)
}

/// `(dev, ino, size, mtime_ns)`.
fn stamp(m: &Metadata) -> (u64, u64, u64, i64) {
    let mtime_ns = m
        .mtime()
        .saturating_mul(1_000_000_000)
        .saturating_add(m.mtime_nsec());
    (m.dev(), m.ino(), m.len(), mtime_ns)
}

/// Spec §5.2 step 2: a file is skipped when its `(dev, ino)`, size and mtime are the ones a read
/// to its last newline stored. A row a partial read left (size −1) matches no file.
pub(crate) fn unchanged(meta: &Metadata, stored: Option<&TokenFileRow>) -> bool {
    let (dev, ino, size, mtime_ns) = stamp(meta);
    stored.is_some_and(|s| {
        s.size >= 0 && s.size as u64 == size && (s.dev, s.ino, s.mtime_ns) == (dev, ino, mtime_ns)
    })
}

/// `len` bytes of `file` at `at`, or `None` when the file ends first.
fn bytes_at(file: &File, at: u64, len: u64) -> io::Result<Option<Vec<u8>>> {
    let mut buf = vec![0; len as usize];
    match file.read_exact_at(&mut buf, at) {
        Ok(()) => Ok(Some(buf)),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
        Err(e) => Err(e),
    }
}

/// A transcript opened for reading, and what its read starts from.
#[derive(Debug)]
pub(crate) struct OpenFile {
    file: File,
    path: PathBuf,
    /// The row's key: the path as discovered.
    key: String,
    dev: u64,
    ino: u64,
    /// The open file's own `fstat`: what a read to its last newline stores (spec §5.2 step 4).
    size: u64,
    mtime_ns: i64,
    /// Where reading starts: the stored offset when it still holds, else 0.
    start: u64,
    /// The bytes `start` was trusted on: the first `min(4096, start)`, and the up to 4096 ending
    /// at `start`, with their hash.
    start_head_bytes: Vec<u8>,
    start_tail: [u8; 8],
    start_tail_bytes: Vec<u8>,
}

impl OpenFile {
    pub(crate) fn start(&self) -> u64 {
        self.start
    }

    /// The row of a file whose next read starts over: offset 0, size −1 (spec §5.2 step 4).
    fn restart_row(&self) -> TokenFileRow {
        let empty = hash8(&[]);
        TokenFileRow {
            path: self.key.clone(),
            dev: self.dev,
            ino: self.ino,
            size: -1,
            mtime_ns: self.mtime_ns,
            offset: 0,
            head_hash: empty,
            tail_hash: empty,
        }
    }
}

/// Spec §5.2 step 2 for a file that is not skipped: opens it, and its own `fstat` decides where
/// reading starts. At the stored offset when `(dev, ino)` match, the file is at least that long,
/// and the bytes under `head_hash` (the first `min(4096, offset)`) and `tail_hash` (the up to
/// 4096 ending at the offset) are unchanged; at 0 otherwise: a new file, another inode, a
/// shorter file, or a changed prefix.
pub(crate) fn open_at(path: &Path, stored: Option<&TokenFileRow>) -> io::Result<OpenFile> {
    let file = File::open(path)?;
    let (dev, ino, size, mtime_ns) = stamp(&file.metadata()?);
    let empty = hash8(&[]);
    let mut open = OpenFile {
        file,
        path: path.to_path_buf(),
        key: path.to_string_lossy().into_owned(),
        dev,
        ino,
        size,
        mtime_ns,
        start: 0,
        start_head_bytes: Vec::new(),
        start_tail: empty,
        start_tail_bytes: Vec::new(),
    };
    let Some(s) =
        stored.filter(|s| s.offset > 0 && (s.dev, s.ino) == (dev, ino) && size >= s.offset)
    else {
        return Ok(open);
    };
    let span = s.offset.min(HASH_SPAN);
    let Some(head) = bytes_at(&open.file, 0, span)? else {
        return Ok(open);
    };
    let Some(tail) = bytes_at(&open.file, s.offset - span, span)? else {
        return Ok(open);
    };
    if hash8(&head) == s.head_hash && hash8(&tail) == s.tail_hash {
        open.start = s.offset;
        open.start_head_bytes = head;
        open.start_tail = s.tail_hash;
        open.start_tail_bytes = tail;
    }
    Ok(open)
}

/// Why a read stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    /// Read to its end; a trailing partial line is left for the next refresh (spec §5.2 step 3).
    Eof,
    /// The budget's deadline passed (spec §5.2 Budget).
    Deadline,
    /// The cancel token was set (spec §5.2 step 4).
    Cancelled,
}

/// What one `step` came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// A chunk was read and its whole lines handed on: call again.
    More,
    Stop(Stop),
}

/// Keeps the last `HASH_SPAN` bytes of `tail` followed by `bytes`.
fn push_tail(tail: &mut Vec<u8>, bytes: &[u8]) {
    let span = HASH_SPAN as usize;
    if bytes.len() >= span {
        tail.clear();
        tail.extend_from_slice(&bytes[bytes.len() - span..]);
    } else {
        tail.extend_from_slice(bytes);
        if tail.len() > span {
            tail.drain(..tail.len() - span);
        }
    }
}

/// Appends to `head` the first of `bytes` that keep `before` plus its length within `HASH_SPAN`.
fn fill_head(head: &mut Vec<u8>, before: usize, bytes: &[u8]) {
    let room = (HASH_SPAN as usize).saturating_sub(before + head.len());
    head.extend_from_slice(&bytes[..bytes.len().min(room)]);
}

/// Spec §5.2 step 3: an open file's whole lines, from its start, one chunk at a time. It holds
/// at most one chunk and one line being assembled, and never a line over the cap: such a line is
/// scanned to its newline, counted, and dropped.
pub(crate) struct LineReader<'f> {
    open: &'f OpenFile,
    limits: Limits,
    /// Bytes read and not yet consumed: one unfinished line, then what followed it.
    buf: Vec<u8>,
    /// The file offset of `buf[0]`.
    buf_at: u64,
    /// Inside a line over the cap: its bytes so far are dropped, the first ones `head` still
    /// lacks kept in `skip_head`, and the last ones in `skip_tail`.
    skipping: bool,
    skip_head: Vec<u8>,
    skip_tail: Vec<u8>,
    /// Just past the last whole line consumed.
    consumed: u64,
    /// The file's first `min(HASH_SPAN, consumed)` bytes, as read: what the new `head_hash`
    /// covers.
    head: Vec<u8>,
    /// Up to `HASH_SPAN` bytes ending at `consumed`, as read: what the new `tail_hash` covers.
    tail: Vec<u8>,
    long_lines: u64,
    chunks: u64,
    peak: usize,
}

impl<'f> LineReader<'f> {
    pub(crate) fn new(open: &'f OpenFile, limits: Limits) -> Self {
        Self {
            open,
            limits,
            buf: Vec::new(),
            buf_at: open.start,
            skipping: false,
            skip_head: Vec::new(),
            skip_tail: Vec::new(),
            consumed: open.start,
            head: open.start_head_bytes.clone(),
            tail: open.start_tail_bytes.clone(),
            long_lines: 0,
            chunks: 0,
            peak: 0,
        }
    }

    /// Just past the last whole line consumed: where the next read of the file starts.
    pub(crate) fn consumed(&self) -> u64 {
        self.consumed
    }

    /// Lines over the cap, skipped (spec §5.4).
    pub(crate) fn long_lines(&self) -> u64 {
        self.long_lines
    }

    /// The most bytes the reader held at once.
    pub(crate) fn peak(&self) -> usize {
        self.peak
    }

    /// Reads one chunk and hands each whole line now complete to `on_line`, newline stripped.
    /// The cancel token is checked before the chunk and between lines; the deadline after a
    /// chunk, at a line boundary, and between lines. So past the deadline it stops at the next
    /// line boundary, finishing a line still being assembled, and every call that reads a chunk
    /// with a newline in it consumes at least one line (spec §5.2 Budget).
    pub(crate) fn step(
        &mut self,
        deadline: Option<Instant>,
        cancel: &Cancel,
        on_line: &mut dyn FnMut(&[u8]),
    ) -> io::Result<Step> {
        if cancel.requested().is_some() {
            return Ok(Step::Stop(Stop::Cancelled));
        }
        let boundary = self.buf.is_empty() && !self.skipping;
        if self.chunks > 0 && boundary && past(deadline) {
            return Ok(Step::Stop(Stop::Deadline));
        }
        let have = self.buf.len();
        if self.read_chunk()? == 0 {
            return Ok(Step::Stop(Stop::Eof));
        }
        self.chunks += 1;
        // `from` is the first byte not yet consumed; bytes before `scan` hold no newline.
        let (mut from, mut scan) = (0, have);
        let mut stop = None;
        while let Some(i) = self.buf[scan..].iter().position(|&b| b == b'\n') {
            let nl = scan + i;
            if self.skipping {
                // The end of a line over the cap, whose earlier bytes were dropped.
                self.skipping = false;
                self.long_lines += 1;
                self.head.append(&mut self.skip_head);
                push_tail(&mut self.tail, &self.skip_tail);
                self.skip_tail.clear();
            } else if nl - from > self.limits.max_line {
                self.long_lines += 1;
            } else {
                on_line(&self.buf[from..nl]);
            }
            from = nl + 1;
            scan = from;
            self.consumed = self.buf_at + from as u64;
            if cancel.requested().is_some() {
                stop = Some(Stop::Cancelled);
                break;
            }
            if past(deadline) {
                stop = Some(Stop::Deadline);
                break;
            }
        }
        fill_head(&mut self.head, 0, &self.buf[..from]);
        push_tail(&mut self.tail, &self.buf[..from]);
        self.buf.drain(..from);
        self.buf_at += from as u64;
        if let Some(stop) = stop {
            return Ok(Step::Stop(stop));
        }
        // What is left is one unfinished line. Past the cap it is dropped as it is read.
        if self.skipping || self.buf.len() > self.limits.max_line {
            self.skipping = true;
            fill_head(&mut self.skip_head, self.head.len(), &self.buf);
            push_tail(&mut self.skip_tail, &self.buf);
            self.buf_at += self.buf.len() as u64;
            self.buf.clear();
            self.buf.shrink_to(self.limits.chunk);
        }
        Ok(Step::More)
    }

    /// Appends up to one chunk to `buf`; 0 at the end of the file.
    fn read_chunk(&mut self) -> io::Result<usize> {
        let have = self.buf.len();
        self.buf.reserve_exact(self.limits.chunk);
        self.buf.resize(have + self.limits.chunk, 0);
        self.peak = self.peak.max(self.buf.len());
        let at = self.buf_at + have as u64;
        let read = loop {
            match self.open.file.read_at(&mut self.buf[have..], at) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                other => break other,
            }
        };
        let n = match read {
            Ok(n) => n,
            Err(e) => {
                self.buf.truncate(have);
                return Err(e);
            }
        };
        self.buf.truncate(have + n);
        Ok(n)
    }

    /// The row a batch commits for this file, just past the last whole line consumed (spec §5.2
    /// step 4). `Valid` while the path still names the open file, the file is at least that
    /// long, and the bytes its start was trusted on and the first and last bytes read are all
    /// unchanged; its hashes are of the bytes read. Its size and mtime are the ones it was opened
    /// with when `at_eof`; otherwise its size is −1, so a file left part-way is never skipped. If
    /// a check fails the file was rewritten during the read: `Rewritten` restarts it from 0.
    pub(crate) fn checkpoint(&self, at_eof: bool) -> io::Result<Checkpoint> {
        let open = self.open;
        if !self.still_holds()? {
            return Ok(Checkpoint::Rewritten(open.restart_row()));
        }
        let size = if at_eof {
            i64::try_from(open.size).unwrap_or(i64::MAX)
        } else {
            -1
        };
        Ok(Checkpoint::Valid(TokenFileRow {
            path: open.key.clone(),
            dev: open.dev,
            ino: open.ino,
            size,
            mtime_ns: open.mtime_ns,
            offset: self.consumed,
            head_hash: hash8(&self.head),
            tail_hash: hash8(&self.tail),
        }))
    }

    /// Spec §5.2 step 4's checks, and that the first and last bytes read are still there. The
    /// first ones begin with the bytes under the stored `head_hash`.
    fn still_holds(&self) -> io::Result<bool> {
        let open = self.open;
        let same =
            fs::metadata(&open.path).is_ok_and(|m| (m.dev(), m.ino()) == (open.dev, open.ino));
        if !same || open.file.metadata()?.len() < self.consumed {
            return Ok(false);
        }
        if open.start > 0 {
            let span = open.start.min(HASH_SPAN);
            let tail = bytes_at(&open.file, open.start - span, span)?.map(|b| hash8(&b));
            if tail != Some(open.start_tail) {
                return Ok(false);
            }
        }
        let still = |at: u64, bytes: &[u8]| -> io::Result<bool> {
            Ok(bytes_at(&open.file, at, bytes.len() as u64)?.as_deref() == Some(bytes))
        };
        Ok(still(0, &self.head)? && still(self.consumed - self.tail.len() as u64, &self.tail)?)
    }
}

/// What a batch commits for a file (spec §5.2 step 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Checkpoint {
    /// The file still holds what was read: its offset moves just past it.
    Valid(TokenFileRow),
    /// It was rewritten during the read: offset 0 and size −1, so the next refresh reads it
    /// again from the start. The lines already read still commit (spec §5.2 step 4).
    Rewritten(TokenFileRow),
}
```

- [ ] **Step 4: Run them and see them pass**

```
cargo test -p tagteam-engine --locked --lib tokens::
```

Expected: `test result: ok. 13 passed; 0 failed` (12 in `tokens::reader::tests`, 1 in `tokens::tests`).

- [ ] **Step 5: Lint**

```
cargo fmt --all --check
cargo clippy -p tagteam-engine --all-targets --locked -- -D warnings
cargo clippy -p tagteam-engine --all-targets --locked --features test-hooks -- -D warnings
```

Expected: no output from the format check, and clippy finishes with no warnings.

- [ ] **Step 6: Commit**

```
git add crates/tagteam-engine/src/lib.rs crates/tagteam-engine/src/tokens/mod.rs crates/tagteam-engine/src/tokens/reader.rs crates/tagteam-engine/src/tokens/refresh.rs crates/tagteam-engine/src/tokens/evidence.rs crates/tagteam-engine/src/tokens/view.rs
git commit -m "Add the incremental transcript file reader"
```

#### Notes for the controller

1. **Verified in a scratch crate.** It held this task's `tokens/` files over the real `tagteam-core` and `tagteam-provider`, with a stand-in `TokenFileRow` of the contract's shape, at `rust-version = 1.85`. Results:
   - `cargo clippy --all-targets -- -D warnings` was clean;
   - all 13 tests passed, `a_head_rewrite_during_a_first_read_is_caught` included (re-verified after the plan review's head fix);
   - `rustfmt --edition 2024 --check` was clean.

   The code above is the formatted text from that run. The same run confirmed that each placeholder's one public item satisfies `pub use x::*;` under `-D warnings`.
2. `reader.rs` starts with `#![allow(dead_code)]`, because its `pub(crate)` items have no caller until Task 15. Task 15 removes it, and nothing else touches `reader.rs` in between.
3. I replaced the outline's `plan_read(..) -> (Start, OpenFile)` with `unchanged(meta, stored)` and `open_at(path, stored)`. This lets the refresh decide on a skip from the discovery `stat` on its own thread, without opening the file, so "nothing new costs one stat per file" (spec §5.2) holds and `files_total` counts only files to read. It also means opening a file and checking its hashes happen on a worker.
4. **Checkpoint hashes cover the bytes actually read, not the file as it is at checkpoint time.**
   - `head_hash` covers the file's first `min(4096, consumed)` bytes. They are seeded with the bytes `open_at` verified under the stored `head_hash`, then appended as lines are consumed; a skipped over-long line's bytes are held in `skip_head` until its newline.
   - `tail_hash` covers a rolling 4 KiB ending at the offset, kept per chunk; skipped lines are included.
   - The checkpoint confirms the file still holds both, as well as the stored tail window its start was trusted on.

   So a rewrite during a read from 0, of the head or of the tail, is caught (`a_head_rewrite_during_a_first_read_is_caught`, `a_rewrite_during_the_read_is_caught_by_the_checkpoint`). The spec's stored-window checks alone cannot see it. Had the head been hashed from the file at checkpoint time, a rewritten first 4 KiB would be stored as read, and the next refresh would resume past a response never parsed.
5. The peak a reader holds is `have + chunk` at each read, and `have` never exceeds `max_line`, since a longer unfinished line is dropped as it is read. So the peak is at most `chunk + max_line`: 65 MiB at the spec's sizes, and pinned at 80 bytes by the huge-line test.

---

### Task 15: `refresh_tokens`

**Depends on:** 8, 10, 12, 13, 14, 16, 17
**Files:**
- Modify: `crates/tagteam-engine/src/tokens/refresh.rs` (replace it whole: Task 14's `Wait`, word for word, plus the other contract types and the refresh)
- Modify: `crates/tagteam-engine/src/tokens/reader.rs` (remove `#![allow(dead_code)]`)
- Create: `crates/tagteam-engine/tests/tokens_refresh.rs`
- Test: `crates/tagteam-engine/tests/tokens_refresh.rs`, and the unit test in `crates/tagteam-engine/src/tokens/refresh.rs`

**Interfaces:**
- Consumes:
  - Task 7: `pub struct UsageLine { pub message_id: String, pub request_id: String, pub session_id: String, pub at_ms: i64, pub model: String, pub project: Option<String>, pub sidechain: bool, pub counts: TokenCounts, pub speed: Option<String>, pub final_line: bool }` (re-exported as `tagteam_provider::UsageLine`); `Capabilities.transcripts`; `Provider::transcript_files(&self, env: &Env) -> io::Result<Vec<PathBuf>>`; `Provider::parse_usage_line(&self, line: &[u8]) -> Result<Option<UsageLine>, LineError>`.
  - Task 2: `TokenCounts::merge_max(&mut self, other: &TokenCounts)`.
  - Task 12 (`crate::store`): `TokenFileRow`; `UsageRow { key, session_id, project, at_ms, model, sidechain, counts, speed, finalized }`; `BatchOutcome { added, updated }`; `RefreshStats { files, responses, skipped_lines, unreadable_files }`; `TokenStateRow`; `usage_key(provider, message_id, request_id) -> i64`; `Store::{token_files, commit_token_batch, finish_token_refresh, prune_tokens, token_state, token_usage}`.
  - Task 12's `prune_tokens(&self, provider, cutoff_ms, holder, now_ms, stop: &dyn Fn() -> bool) -> Result<PruneOutcome { deleted: u64, complete: bool }, StoreError>`. It asks `stop` before each chunk, the first included, and an early stop releases the daily lease. This task's `stop` lets the first chunk of each call through, so every call makes progress (note 3).
  - Task 12's semantics this task relies on:
    - `token_state.responses` is the store's own live count, kept by each batch and prune;
    - `finish_token_refresh` writes `files`, `skipped_lines` and `unreadable_files`, and creates the provider's `token_state` row if absent;
    - `commit_token_batch` deletes the `removed` file rows.
  - Task 13: `Settings.tokens_retention_days: u32`.
  - Task 14: `tokens_lock_path`; `Wait` in `refresh.rs`; `crate::tokens::reader::{Limits, past, unchanged, open_at, OpenFile::start, LineReader::{new, step, consumed, long_lines, peak, checkpoint}, Step, Stop, Checkpoint}`.
  - Task 16: `pub(crate) fn scan_sessions(&self, p: &dyn Provider) -> Result<u64, EngineError>` (refresh step 1).
  - Task 17: `pub(crate) fn lock_until_free(path: &Path, cancel: &Cancel) -> Result<FlockGuard, LockError>` in `crate::tokens` (the wait of `Wait::Wait`).
  - Task 10 (tests): `tagteam_fake::{FakeAgent, FakeUsage, usage_line_json, append_transcript, FAKE_AGENT}`, `FakeAgent::without_transcripts`.
- Produces (contract, `tagteam_engine::tokens`):
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Wait { Wait, Try }   // Task 14's, kept word for word
  #[derive(Debug, Clone, PartialEq, Eq)] pub enum RefreshOutcome { Done(IndexReport), Busy }
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)] pub struct IndexProgress { pub files_done: u64, pub files_total: u64, pub bytes_done: u64, pub bytes_total: u64 }
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)] pub struct IndexReport { pub complete: bool, pub files_read: u64, pub responses_added: u64, pub responses_updated: u64, pub skipped_lines: u64, pub evidence_changed: u64, pub took_ms: u64 }
  impl Engine {
      pub fn refresh_tokens(&self, provider: &ProviderId, wait: Wait, budget: Option<Duration>, cancel: &Cancel, progress: &mut dyn FnMut(&IndexProgress)) -> Result<RefreshOutcome, EngineError>;
  }
  ```

**On the long wait.** `Wait::Wait` waits until the lock is free, ended only by `cancel` (spec §5.1). It uses Task 17's `lock_until_free`, which loops over one-hour rounds of `FlockGuard::lock` and treats `Timeout` as "go on". `FlockGuard::lock` alone cannot wait without a deadline: it computes `Instant::now() + timeout`, which panics for `Duration::MAX`, and any finite value is a cap the spec does not have. A waiter starts within one 100 ms poll of the release.

**On threading.** At most `min(4, available_parallelism, files to read)` scoped workers each read one file at a time, with one `LineReader`, holding at most one chunk and one assembling line. Each worker sends one message per chunk: the chunk's parsed rows plus the checkpoint just past them. Messages go over a `sync_channel` bounded at one per worker. The writer, on the calling thread, merges rows of one response within a batch by spec §2.2's rule. It commits when a batch holds 2,000 distinct responses, or once its first item has waited 50 ms, and at the end. A file's checkpoint enters a batch only once all of that chunk's rows are in that batch or in an earlier committed one. So a checkpoint is never committed ahead of its lines.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-engine/tests/tokens_refresh.rs`:

```rust
//! Spec §5: the incremental token index. FakeAgent's transcripts (Task 10) and Claude Code's
//! fixture tree, temp homes, and the fixture's clock.
mod common;

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::{FakeFx, Fx};
use serde_json::json;
use tagteam_core::ProviderId;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::{RefreshStats, TokenFileRow, UsageRow, usage_key};
use tagteam_engine::tokens::{IndexProgress, IndexReport, RefreshOutcome, Wait, tokens_lock_path};
use tagteam_engine::vault::{KeychainVault, Vault};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_fake::{FakeAgent, FakeUsage, append_transcript, usage_line_json};
use tagteam_provider::{Cancel, Clock, FlockGuard, RunShell};

const NOW: i64 = 1_790_000_000_000;
const DAY: i64 = 86_400_000;

/// One transcript line for `u`, newline included.
fn line(u: &FakeUsage) -> String {
    format!("{}\n", usage_line_json(u))
}

/// Response `id` of session `s1`: its output `out`, from a final line or a stale snapshot.
fn response(id: &str, out: u64, final_line: bool) -> FakeUsage {
    let mut u = FakeUsage {
        message_id: id.into(),
        request_id: format!("r-{id}"),
        final_line,
        ..FakeUsage::default()
    };
    u.counts.output = out;
    u
}

/// A line of another type, carrying `pad` bytes.
fn other(pad: usize) -> String {
    format!("{}\n", json!({"t": "other", "pad": "p".repeat(pad)}))
}

fn key(p: &ProviderId, id: &str) -> i64 {
    usage_key(p, id, &format!("r-{id}"))
}

fn refresh(engine: &Engine, p: &ProviderId) -> IndexReport {
    match engine
        .refresh_tokens(
            p,
            Wait::Wait,
            None,
            &Cancel::new(),
            &mut |_: &IndexProgress| {},
        )
        .unwrap()
    {
        RefreshOutcome::Done(report) => report,
        RefreshOutcome::Busy => panic!("nothing else refreshes"),
    }
}

fn rows(engine: &Engine, p: &ProviderId) -> Vec<UsageRow> {
    engine
        .store()
        .unwrap()
        .token_usage(p, None, None, None)
        .unwrap()
}

fn row(engine: &Engine, p: &ProviderId, id: &str) -> UsageRow {
    let k = key(p, id);
    rows(engine, p)
        .into_iter()
        .find(|r| r.key == k)
        .unwrap_or_else(|| panic!("{id} is indexed"))
}

fn files(engine: &Engine, p: &ProviderId) -> Vec<TokenFileRow> {
    engine.store().unwrap().token_files(p).unwrap()
}

/// The stored row of the transcript at `path`.
fn file<'a>(files: &'a [TokenFileRow], path: &Path) -> &'a TokenFileRow {
    let path = path.to_string_lossy();
    files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path} has a row"))
}

/// An engine over `ffx`'s home whose only provider is `fake`, with `settings`.
fn fake_engine(ffx: &FakeFx, fake: FakeAgent, settings: Settings) -> Engine {
    Engine::new(EngineConfig {
        env: ffx.fx.env.clone(),
        registry: ProviderRegistry::new().with(Arc::new(fake)),
        vault: Vault::new(Box::new(KeychainVault::new(ffx.fx.kc.clone()))),
        oracle: ffx.fx.oracle.clone(),
        clock: ffx.fx.clock.clone(),
        http: ffx.fx.http.clone(),
        default_provider: ffx.fake_provider(),
        settings,
        process: ffx.fx.process.clone(),
        spawner: ffx.fx.spawner.clone(),
        run_shell: RunShell::Outside,
    })
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

#[test]
fn a_cold_refresh_indexes_every_response_once() {
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    append_transcript(
        env,
        "a.jsonl",
        &[
            line(&response("m1", 5, false)),
            line(&response("m1", 40, true)),
            line(&response("m2", 7, true)),
        ]
        .concat(),
    );
    append_transcript(env, "b.jsonl", &line(&response("m3", 9, true)));

    let report = refresh(&ffx.engine, &p);
    assert_eq!(
        report,
        IndexReport {
            complete: true,
            files_read: 2,
            responses_added: 3,
            responses_updated: 0,
            skipped_lines: 0,
            evidence_changed: 0,
            took_ms: 0,
        }
    );
    assert_eq!(rows(&ffx.engine, &p).len(), 3);
    let m1 = row(&ffx.engine, &p, "m1");
    assert_eq!((m1.counts.output, m1.finalized), (40, true), "spec §2.2");
    let state = ffx
        .engine
        .store()
        .unwrap()
        .token_state(&p)
        .unwrap()
        .unwrap();
    assert_eq!(state.refreshed_at, Some(NOW));
    assert_eq!(
        state.stats,
        RefreshStats {
            files: 2,
            responses: 3,
            skipped_lines: 0,
            unreadable_files: 0
        }
    );
}

#[test]
fn a_second_refresh_reads_nothing_new() {
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    append_transcript(env, "a.jsonl", &line(&response("m1", 2, true)));
    refresh(&ffx.engine, &p);
    let before = files(&ffx.engine, &p);

    let report = refresh(&ffx.engine, &p);
    assert!(report.complete);
    assert_eq!(
        (
            report.files_read,
            report.responses_added,
            report.responses_updated
        ),
        (0, 0, 0)
    );
    assert_eq!(files(&ffx.engine, &p), before);
}

#[test]
fn appended_lines_are_picked_up() {
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    append_transcript(env, "a.jsonl", &line(&response("m1", 5, false)));
    refresh(&ffx.engine, &p);
    // The final line of m1, and a new response.
    let path = append_transcript(
        env,
        "a.jsonl",
        &[
            line(&response("m1", 40, true)),
            line(&response("m2", 3, true)),
        ]
        .concat(),
    );

    let report = refresh(&ffx.engine, &p);
    assert_eq!(
        (
            report.files_read,
            report.responses_added,
            report.responses_updated
        ),
        (1, 1, 1)
    );
    let m1 = row(&ffx.engine, &p, "m1");
    assert_eq!((m1.counts.output, m1.finalized), (40, true));
    let len = fs::metadata(&path).unwrap().len();
    let f = file(&files(&ffx.engine, &p), &path).clone();
    assert_eq!((f.offset, f.size), (len, len as i64));
}

#[test]
fn the_same_response_across_files_is_one_row() {
    // Spec §2.2: a resumed session copies history into its new file.
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    let early = FakeUsage {
        at_ms: NOW - 7_200_000,
        ..response("m1", 5, false)
    };
    let late = FakeUsage {
        at_ms: NOW - 3_600_000,
        ..response("m1", 40, true)
    };
    append_transcript(env, "a.jsonl", &line(&early));
    append_transcript(env, "b.jsonl", &line(&late));

    let report = refresh(&ffx.engine, &p);
    assert_eq!(report.responses_added, 1);
    let all = rows(&ffx.engine, &p);
    assert_eq!(all.len(), 1);
    assert_eq!(
        (all[0].at_ms, all[0].counts.output, all[0].finalized),
        (NOW - 7_200_000, 40, true),
        "the earliest time, each count's maximum"
    );
}

#[test]
fn lines_outside_the_retention_window_are_not_inserted() {
    // Spec §4 Retention, Review Focus 3: a line older than the window is not inserted; a
    // future one is.
    let ffx = FakeFx::new();
    let settings = Settings {
        tokens_retention_days: 30,
        ..Settings::default()
    };
    let engine = fake_engine(&ffx, FakeAgent::new(), settings);
    let p = ffx.fake_provider();
    let at = |id: &str, at_ms: i64| {
        line(&FakeUsage {
            at_ms,
            ..response(id, 1, true)
        })
    };
    append_transcript(
        &ffx.fx.env,
        "a.jsonl",
        &[
            at("old", NOW - 31 * DAY),
            at("kept", NOW - 29 * DAY),
            at("future", NOW + DAY),
        ]
        .concat(),
    );

    let report = refresh(&engine, &p);
    assert!(report.complete);
    let keys: BTreeSet<i64> = rows(&engine, &p).iter().map(|r| r.key).collect();
    assert_eq!(keys, BTreeSet::from([key(&p, "kept"), key(&p, "future")]));
}

#[test]
#[cfg(feature = "test-hooks")]
fn a_waiting_refresh_never_passes_the_lock_while_another_holds_it() {
    // Spec §5.1: `Wait::Wait` waits for another refresh. While the test holds the lock, the
    // refresh reaches the point before its wait and never the one after it: only the cancel
    // token ends that wait. Were the lock not taken, nothing between the two points could be
    // interrupted. Once the lock is free, the next refresh passes that point holding it.
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;

    let ffx = FakeFx::new();
    let p = ffx.fake_provider();
    append_transcript(&ffx.fx.env, "a.jsonl", &line(&response("m1", 2, true)));
    let path = tokens_lock_path(&ffx.fx.env, &p);
    let other = FlockGuard::try_lock(&path).unwrap().unwrap();
    let (at_lock, reached) = mpsc::channel();
    ffx.engine.on_point(
        "refresh-tokens-lock",
        Box::new(move || at_lock.send(()).unwrap()),
    );
    let passed = Arc::new(AtomicBool::new(false));
    let free_there = Arc::new(AtomicBool::new(true));
    let (passed_cb, free_cb, path_cb) = (passed.clone(), free_there.clone(), path.clone());
    ffx.engine.also_on_point(
        "refresh-tokens-locked",
        Box::new(move || {
            passed_cb.store(true, Ordering::SeqCst);
            let free = FlockGuard::try_lock(&path_cb).unwrap().is_some();
            free_cb.store(free, Ordering::SeqCst);
        }),
    );

    let cancel = Cancel::new();
    let first = thread::scope(|s| {
        let waiting = s.spawn(|| {
            ffx.engine
                .refresh_tokens(&p, Wait::Wait, None, &cancel, &mut |_: &IndexProgress| {})
        });
        reached.recv().unwrap();
        cancel.request(2);
        waiting.join().unwrap()
    });
    assert_eq!(first.unwrap_err().signal(), Some(2));
    assert!(
        !passed.load(Ordering::SeqCst),
        "it passed the lock while another refresh held it"
    );
    assert!(rows(&ffx.engine, &p).is_empty());

    drop(other);
    let report = refresh(&ffx.engine, &p);
    assert!(report.complete);
    assert_eq!(report.responses_added, 1);
    assert!(passed.load(Ordering::SeqCst));
    assert!(
        !free_there.load(Ordering::SeqCst),
        "past that point the refresh holds the lock"
    );
}

#[test]
fn try_returns_busy_while_another_holds_the_lock() {
    let ffx = FakeFx::new();
    let p = ffx.fake_provider();
    append_transcript(&ffx.fx.env, "a.jsonl", &line(&response("m1", 2, true)));
    let other = FlockGuard::try_lock(&tokens_lock_path(&ffx.fx.env, &p))
        .unwrap()
        .unwrap();

    let outcome = ffx
        .engine
        .refresh_tokens(
            &p,
            Wait::Try,
            None,
            &Cancel::new(),
            &mut |_: &IndexProgress| {},
        )
        .unwrap();
    assert_eq!(outcome, RefreshOutcome::Busy);
    assert!(rows(&ffx.engine, &p).is_empty());

    drop(other);
    let RefreshOutcome::Done(report) = ffx
        .engine
        .refresh_tokens(
            &p,
            Wait::Try,
            None,
            &Cancel::new(),
            &mut |_: &IndexProgress| {},
        )
        .unwrap()
    else {
        panic!("the lock is free");
    };
    assert!(report.complete);
    assert_eq!(report.responses_added, 1);
}

#[test]
fn a_budgeted_refresh_resumes_where_it_stopped() {
    // Spec §5.2 Budget: past its deadline a refresh stops at the next line boundary and commits
    // what it read; the next call goes on from there. A file left part-way is never skipped, and
    // only the call that completes sets `refreshed_at`, which moves `changed_at` (spec §14).
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    let store = ffx.engine.store().unwrap();
    let a: String = (0..5)
        .map(|i| line(&response(&format!("a{i}"), 2, true)))
        .collect();
    let b: String = (0..3)
        .map(|i| line(&response(&format!("b{i}"), 2, true)))
        .collect();
    let paths: Vec<(PathBuf, u64)> = vec![
        (append_transcript(env, "a.jsonl", &a), a.len() as u64),
        (append_transcript(env, "b.jsonl", &b), b.len() as u64),
    ];
    // Bytes consumed plus files read to their end: it grows on every call that does not complete.
    let progress_of = |files: &[TokenFileRow]| -> u64 {
        files
            .iter()
            .map(|f| f.offset + u64::from(f.size >= 0))
            .sum()
    };

    let mut seen = 0;
    let mut calls = 0;
    let report = loop {
        calls += 1;
        assert!(calls <= 12, "every call makes progress");
        ffx.fx.clock.advance_ms(1_000);
        let RefreshOutcome::Done(report) = ffx
            .engine
            .refresh_tokens(
                &p,
                Wait::Wait,
                Some(Duration::ZERO),
                &Cancel::new(),
                &mut |_: &IndexProgress| {},
            )
            .unwrap()
        else {
            panic!("nothing else refreshes");
        };
        if report.complete {
            break report;
        }
        let stored = store.token_files(&p).unwrap();
        let now = progress_of(&stored);
        assert!(now > seen, "call {calls} made no progress");
        seen = now;
        for (path, len) in &paths {
            if let Some(f) = stored.iter().find(|f| f.path == path.to_string_lossy()) {
                if f.offset < *len {
                    assert_eq!(f.size, -1, "{} is read part-way", f.path);
                }
            }
        }
        assert_eq!(store.token_state(&p).unwrap().unwrap().refreshed_at, None);
    };

    assert_eq!(
        report.responses_added, 0,
        "the completing call found only the files' ends"
    );
    assert_eq!(rows(&ffx.engine, &p).len(), 8);
    let stored = store.token_files(&p).unwrap();
    for (path, len) in &paths {
        let f = file(&stored, path);
        assert_eq!((f.offset, f.size), (*len, *len as i64));
    }
    let now = ffx.fx.clock.now_ms();
    let state = store.token_state(&p).unwrap().unwrap();
    assert_eq!(
        (state.refreshed_at, state.changed_at),
        (Some(now), Some(now)),
        "the first completion moves changed_at"
    );
}

#[test]
fn cancel_commits_what_was_parsed_and_reports_interrupted() {
    // Spec §5.2 step 4: a cancelled refresh stops at the next chunk, commits what it parsed, and
    // loses nothing. Six 1 MiB chunks; the token is set once the writer has the first one, so
    // the worker, at most two chunks ahead of it on the bounded channel, stops part-way.
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    let mut text = String::new();
    let mut ends = Vec::new();
    for i in 0..6_000 {
        if i % 100 == 0 {
            let id = format!("m{i}");
            text.push_str(&line(&response(&id, 1, true)));
            ends.push((text.len() as u64, id));
        } else {
            text.push_str(&other(1_000));
        }
    }
    append_transcript(env, "big.jsonl", &text);

    let cancel = Cancel::new();
    let err = ffx
        .engine
        .refresh_tokens(&p, Wait::Wait, None, &cancel, &mut |at: &IndexProgress| {
            if at.bytes_done > 0 {
                cancel.request(2);
            }
        })
        .unwrap_err();
    assert_eq!((err.signal(), err.kind()), (Some(2), "interrupted"));

    let stored = files(&ffx.engine, &p);
    assert_eq!(stored.len(), 1);
    let offset = stored[0].offset;
    assert!(offset > 0 && offset < text.len() as u64, "{offset}");
    assert_eq!(stored[0].size, -1, "a cancelled read is never skipped");
    let indexed: BTreeSet<i64> = rows(&ffx.engine, &p).iter().map(|r| r.key).collect();
    let before: BTreeSet<i64> = ends
        .iter()
        .filter(|(end, _)| *end <= offset)
        .map(|(_, id)| key(&p, id))
        .collect();
    assert!(!before.is_empty());
    assert_eq!(indexed, before, "exactly the lines before its checkpoint");
    assert_eq!(
        ffx.engine
            .store()
            .unwrap()
            .token_state(&p)
            .unwrap()
            .unwrap()
            .refreshed_at,
        None
    );

    let report = refresh(&ffx.engine, &p);
    assert!(report.complete);
    assert_eq!(rows(&ffx.engine, &p).len(), 60, "nothing lost");
}

#[test]
fn a_provider_without_transcripts_is_refused() {
    let ffx = FakeFx::new();
    let engine = fake_engine(
        &ffx,
        FakeAgent::new().without_transcripts(),
        Settings::default(),
    );
    let p = ffx.fake_provider();
    let err = engine
        .refresh_tokens(
            &p,
            Wait::Wait,
            None,
            &Cancel::new(),
            &mut |_: &IndexProgress| {},
        )
        .unwrap_err();
    assert_eq!(err.kind(), "invalid-input");
    assert_eq!(err.to_string(), "FakeAgent has no transcripts");
    assert!(
        !tokens_lock_path(&ffx.fx.env, &p).exists(),
        "refused before the lock"
    );
}

#[test]
fn skipped_lines_are_counted_in_the_state() {
    // Spec §5.4: a line that does not parse is skipped and counted; it never fails the refresh.
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    append_transcript(
        env,
        "a.jsonl",
        &[
            line(&response("m1", 2, true)),
            "{\"t\":\"usage\",\"id\":\"m2\"\n".to_owned(),
            "{\"t\":\"usage\",\"id\":7}\n".to_owned(),
            line(&response("m3", 2, true)),
        ]
        .concat(),
    );

    let report = refresh(&ffx.engine, &p);
    assert!(report.complete);
    assert_eq!((report.responses_added, report.skipped_lines), (2, 2));
    let state = ffx
        .engine
        .store()
        .unwrap()
        .token_state(&p)
        .unwrap()
        .unwrap();
    assert_eq!(
        state.stats,
        RefreshStats {
            files: 1,
            responses: 2,
            skipped_lines: 2,
            unreadable_files: 0
        }
    );
}

#[test]
fn an_unreadable_file_is_counted_and_retried() {
    // Spec §5.4: a file that cannot be read is skipped with a warning, counted, and read again on
    // a later refresh; it never fails the refresh.
    let ffx = FakeFx::new();
    let (env, p) = (&ffx.fx.env, ffx.fake_provider());
    append_transcript(env, "a.jsonl", &line(&response("m1", 2, true)));
    let locked = append_transcript(env, "b.jsonl", &line(&response("m2", 2, true)));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&locked).is_ok() {
        // A privileged run (root) reads a file of mode 000: no file can be made unreadable here,
        // so there is nothing to test.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }

    let report = refresh(&ffx.engine, &p);
    assert!(report.complete);
    assert_eq!(report.responses_added, 1);
    let stats = ffx
        .engine
        .store()
        .unwrap()
        .token_state(&p)
        .unwrap()
        .unwrap()
        .stats;
    assert_eq!((stats.files, stats.unreadable_files), (1, 1));

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o600)).unwrap();
    let report = refresh(&ffx.engine, &p);
    assert_eq!(report.responses_added, 1, "the unreadable file is retried");
}

/// `n` responses older than any retention window, as an index a year old holds them.
fn plant_expired(engine: &Engine, p: &ProviderId, n: usize) {
    let rows: Vec<UsageRow> = (0..n)
        .map(|i| UsageRow {
            key: key(p, &format!("old{i}")),
            session_id: "s-old".into(),
            project: None,
            at_ms: NOW - 400 * DAY,
            model: "fake-1".into(),
            sidechain: false,
            counts: Default::default(),
            speed: None,
            finalized: true,
        })
        .collect();
    engine
        .store()
        .unwrap()
        .commit_token_batch(p, &rows, &[], &[], NOW - 400 * DAY)
        .unwrap();
}

#[test]
fn a_budgeted_prune_goes_on_over_several_calls() {
    // Spec §4 Retention, §5.2 Budget: the prune that ends a refresh stops with the budget, as
    // reading does. The refresh is then incomplete, and the next call goes on with the prune.
    // 12,001 rows are more than one of the store's prune chunks.
    let ffx = FakeFx::new();
    let p = ffx.fake_provider();
    plant_expired(&ffx.engine, &p, 12_001);
    let mut left = rows(&ffx.engine, &p).len();
    let mut calls = 0;
    loop {
        calls += 1;
        assert!(calls <= 10, "every call prunes some");
        let RefreshOutcome::Done(report) = ffx
            .engine
            .refresh_tokens(
                &p,
                Wait::Wait,
                Some(Duration::ZERO),
                &Cancel::new(),
                &mut |_: &IndexProgress| {},
            )
            .unwrap()
        else {
            panic!("nothing else refreshes");
        };
        let now = rows(&ffx.engine, &p).len();
        if report.complete {
            assert_eq!(now, 0);
            break;
        }
        assert!(now > 0 && now < left, "{now} of {left} left");
        let state = ffx
            .engine
            .store()
            .unwrap()
            .token_state(&p)
            .unwrap()
            .unwrap();
        assert_eq!(state.refreshed_at, None);
        left = now;
    }
    assert!(
        calls >= 2,
        "a prune of more than one chunk takes more than one budgeted call"
    );
}

#[test]
fn a_cancelled_prune_reports_interrupted() {
    // A cancel that meets the prune follows the refresh's cancel path: the prune stops before
    // its next chunk, what was pruned stays pruned, and the refresh is interrupted, not complete.
    let ffx = FakeFx::new();
    let p = ffx.fake_provider();
    plant_expired(&ffx.engine, &p, 12_001);
    let cancel = Cancel::new();
    // Nothing to read: the token is set as the refresh starts, and the prune is what meets it.
    let err = ffx
        .engine
        .refresh_tokens(&p, Wait::Wait, None, &cancel, &mut |_: &IndexProgress| {
            cancel.request(2)
        })
        .unwrap_err();
    assert_eq!(err.signal(), Some(2));
    let left = rows(&ffx.engine, &p).len();
    assert!(left > 0, "the prune stopped short of the end");
    let state = ffx
        .engine
        .store()
        .unwrap()
        .token_state(&p)
        .unwrap()
        .unwrap();
    assert_eq!(state.refreshed_at, None);
}

#[test]
fn the_claude_code_fixture_tree_is_indexed() {
    // Task 8's synthetic tree: 4 files, 4 responses over 8 usage lines, 2 unreadable lines,
    // subagents included. The longest retention keeps the fixtures' dates in the window.
    let fx = Fx::new();
    let engine = fx.engine_with_settings(Settings {
        tokens_retention_days: 3_650,
        ..Settings::default()
    });
    let cc = fx.provider();
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tagteam-cc/tests/fixtures/transcripts/projects"),
        &fx.paths().config_home.join("projects"),
    );

    let report = refresh(&engine, &cc);
    assert!(report.complete);
    assert_eq!(
        (
            report.files_read,
            report.responses_added,
            report.skipped_lines
        ),
        (4, 4, 2)
    );
    assert_eq!(rows(&engine, &cc).len(), 4);
    assert_eq!(files(&engine, &cc).len(), 4);
    assert_eq!(
        refresh(&engine, &cc).files_read,
        0,
        "and nothing on the next"
    );
}
```

Append this unit test module to `crates/tagteam-engine/src/tokens/refresh.rs` (Task 14's `Wait` stays above it):

```rust
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tagteam_fake::{FAKE_AGENT, FakeAgent, FakeUsage, append_transcript, usage_line_json};
    use tagteam_provider::{
        Env, FakeClock, FakeKeychain, FakeProcessProbe, NoHttp, RunShell, ScriptedSpawner,
    };

    use super::*;
    use crate::engine::EngineConfig;
    use crate::oracle::NoOracle;
    use crate::registry::ProviderRegistry;
    use crate::settings::Settings;
    use crate::vault::{KeychainVault, Vault};

    const NOW: i64 = 1_790_000_000_000;

    fn engine(env: &Env) -> Engine {
        Engine::new(EngineConfig {
            env: env.clone(),
            registry: ProviderRegistry::new().with(Arc::new(FakeAgent::new())),
            vault: Vault::new(Box::new(KeychainVault::new(Arc::new(FakeKeychain::new())))),
            oracle: Arc::new(NoOracle),
            clock: Arc::new(FakeClock::new(NOW)),
            http: Arc::new(NoHttp),
            default_provider: ProviderId::new(FAKE_AGENT),
            settings: Settings::default(),
            process: Arc::new(FakeProcessProbe::new()),
            spawner: Arc::new(ScriptedSpawner::new()),
            run_shell: RunShell::Outside,
        })
    }

    fn usage(id: &str) -> String {
        let u = FakeUsage {
            message_id: id.into(),
            request_id: format!("r-{id}"),
            ..FakeUsage::default()
        };
        format!("{}\n", usage_line_json(&u))
    }

    fn other(pad: &str) -> String {
        format!("{}\n", serde_json::json!({"t": "other", "pad": pad}))
    }

    #[test]
    fn memory_and_budget_hold_on_a_file_of_many_chunks() {
        // Review Focus 1: chunks of 64 bytes and a cap of 1 KiB stand in for 1 MiB and 64 MiB.
        let limits = Limits {
            chunk: 64,
            max_line: 1_024,
        };
        let p = ProviderId::new(FAKE_AGENT);

        // Memory: every worker's reader holds at most one chunk and one line up to the cap, and
        // a line over the cap is skipped, counted, and never held.
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let mut text = String::new();
        for i in 0..20 {
            text.push_str(&usage(&format!("m{i}")));
            text.push_str(&other(&"p".repeat(300)));
        }
        text.push_str(&other(&"x".repeat(5_000)));
        text.push_str(&usage("last"));
        append_transcript(&env, "big.jsonl", &text);
        let (outcome, peak) = engine(&env)
            .refresh_with(
                &p,
                Wait::Wait,
                None,
                &Cancel::new(),
                &mut |_: &IndexProgress| {},
                limits,
            )
            .unwrap();
        let RefreshOutcome::Done(report) = outcome else {
            panic!("nothing else refreshes");
        };
        assert!(report.complete);
        assert_eq!((report.responses_added, report.skipped_lines), (21, 1));
        assert!(
            peak > 0 && peak <= limits.chunk + limits.max_line,
            "a reader held {peak} bytes"
        );

        // Budget: past the deadline each call reads one more line, assembled across chunks,
        // never none and never the rest of the file, and the file is never skipped part-way.
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let lines: Vec<String> = (0..5).map(|i| usage(&format!("m{i}"))).collect();
        let path = append_transcript(&env, "slow.jsonl", &lines.concat());
        let engine = engine(&env);
        let store = engine.store().unwrap();
        let mut end = 0;
        for line in &lines {
            let (outcome, _) = engine
                .refresh_with(
                    &p,
                    Wait::Wait,
                    Some(Duration::ZERO),
                    &Cancel::new(),
                    &mut |_: &IndexProgress| {},
                    limits,
                )
                .unwrap();
            assert!(matches!(outcome, RefreshOutcome::Done(r) if !r.complete));
            end += line.len() as u64;
            let f = &store.token_files(&p).unwrap()[0];
            assert_eq!((f.offset, f.size), (end, -1));
            assert_eq!(store.token_state(&p).unwrap().unwrap().refreshed_at, None);
        }
        let (outcome, _) = engine
            .refresh_with(
                &p,
                Wait::Wait,
                Some(Duration::ZERO),
                &Cancel::new(),
                &mut |_: &IndexProgress| {},
                limits,
            )
            .unwrap();
        assert!(matches!(outcome, RefreshOutcome::Done(r) if r.complete));
        assert_eq!(store.token_usage(&p, None, None, None).unwrap().len(), 5);
        let f = &store.token_files(&p).unwrap()[0];
        let len = std::fs::metadata(&path).unwrap().len();
        assert_eq!((f.offset, f.size), (len, len as i64));
    }
}
```

- [ ] **Step 2: Run them and see them fail**

```
cargo test -p tagteam-engine --locked --features test-hooks --test tokens_refresh
cargo test -p tagteam-engine --locked --lib tokens::refresh
```

Expected: neither compiles:
- the first fails with `no method named `refresh_tokens` found for struct `Engine``;
- the second with `no method named `refresh_with` found for reference `&Engine``, `cannot find type `Limits` in this scope` and `cannot find type `RefreshOutcome` in this scope`.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/tokens/reader.rs`, delete the line `#![allow(dead_code)]` and the blank line after it; the refresh now uses every item.

Replace `crates/tagteam-engine/src/tokens/refresh.rs` above its `#[cfg(test)] mod tests` (keep the test module from Step 1 at the end) with:

```rust
//! `refresh_tokens` (spec §5): the incremental index of a provider's transcripts. At most four
//! scoped workers read one file at a time, each holding one chunk; one writer, on the calling
//! thread, commits their lines in batches, with the offsets just past them.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::num::NonZeroUsize;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use tagteam_core::ProviderId;
use tagteam_provider::{Cancel, FlockGuard, LockError, Provider, UsageLine};

use super::reader::{Checkpoint, Limits, LineReader, Step, Stop, open_at, past, unchanged};
use super::{lock_until_free, tokens_lock_path};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::store::{RefreshStats, Store, TokenFileRow, UsageRow, usage_key};

/// At most this many files are read at once (spec §5.2 step 4).
const MAX_WORKERS: usize = 4;
/// A batch commits at most this many responses, so its transaction holds the write lock for
/// about 50 ms at most (spec §5.2 step 4) ...
const BATCH_ROWS: usize = 2_000;
/// ... and commits once its first line has waited this long.
const BATCH_WAIT: Duration = Duration::from_millis(50);
const DAY_MS: i64 = 86_400_000;

/// How a refresh takes the provider's tokens lock (spec §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// Waits for another refresh to finish, cancellably: `tagteam tokens`.
    Wait,
    /// Returns `Busy` at once while another refresh holds it: the TUI's timer and the daemon.
    Try,
}

/// What a refresh came to (spec §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    Done(IndexReport),
    /// Another process is refreshing (`Wait::Try`).
    Busy,
}

/// How far a refresh has come (spec §5): the files and bytes it has read of those it found to
/// read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexProgress {
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// One refresh (spec §5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexReport {
    /// False when the budget ran out or the refresh was cancelled first; call again to go on.
    pub complete: bool,
    pub files_read: u64,
    pub responses_added: u64,
    pub responses_updated: u64,
    pub skipped_lines: u64,
    /// Evidence rows its `sight_sessions` added, extended or closed.
    pub evidence_changed: u64,
    pub took_ms: u64,
}

/// A file step 2 found changed (spec §5.2).
struct Job {
    path: PathBuf,
    stored: Option<TokenFileRow>,
    /// The bytes it likely holds past its stored offset, for progress.
    bytes: u64,
}

/// How a file's read ended in this refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    /// Read to its last newline, or found rewritten, so its next read starts over.
    Done,
    /// Stopped by the budget or the cancel token; the next refresh goes on from its offset.
    Stopped,
}

/// What a worker sends the writer.
enum Msg {
    /// One chunk's rows and the checkpoint just past them; `end` with a file's last chunk.
    Chunk {
        rows: Vec<UsageRow>,
        skipped: u64,
        file: TokenFileRow,
        read: u64,
        end: Option<End>,
    },
    /// A file that could not be opened or read: skipped, and tried again next time (spec §5.4).
    Unreadable,
}

/// What the workers share.
struct Work<'a> {
    p: &'a dyn Provider,
    provider: &'a ProviderId,
    jobs: &'a [Job],
    next: AtomicUsize,
    limits: Limits,
    deadline: Option<Instant>,
    cancel: &'a Cancel,
    /// Set when the writer fails: the workers stop.
    halt: AtomicBool,
    /// Lines older than this are not inserted (spec §4 Retention).
    cutoff_ms: i64,
    /// The most bytes any reader held at once.
    peak: AtomicUsize,
}

impl Work<'_> {
    /// One worker: the files in turn, one at a time, until none is left, the deadline has
    /// passed once it has read one, the token is set, or the writer is gone.
    fn run(&self, tx: &SyncSender<Msg>) {
        let mut first = true;
        loop {
            let spent = !first && past(self.deadline);
            if spent || self.halt.load(Ordering::Relaxed) || self.cancel.requested().is_some() {
                return;
            }
            let Some(job) = self.jobs.get(self.next.fetch_add(1, Ordering::Relaxed)) else {
                return;
            };
            first = false;
            if self.read(job, tx) != Some(End::Done) {
                return;
            }
        }
    }

    /// One file, from where `open_at` starts it, a message per chunk. `None` when the writer is
    /// gone.
    fn read(&self, job: &Job, tx: &SyncSender<Msg>) -> Option<End> {
        let open = match open_at(&job.path, job.stored.as_ref()) {
            Ok(open) => open,
            Err(e) => return self.unreadable(job, &e, tx),
        };
        let mut reader = LineReader::new(&open, self.limits);
        let (mut rows, mut skipped, mut long) = (Vec::new(), 0, 0);
        let mut sent = open.start();
        loop {
            let step = reader.step(self.deadline, self.cancel, &mut |line: &[u8]| {
                match self.p.parse_usage_line(line) {
                    Ok(Some(u)) if u.at_ms >= self.cutoff_ms => rows.push(row_of(self.provider, u)),
                    // Another type, a line with no usage, or one older than the window.
                    Ok(_) => {}
                    // Spec §5.4: skipped and counted, never a failure.
                    Err(_) => skipped += 1,
                }
            });
            let step = match step {
                Ok(step) => step,
                Err(e) => return self.unreadable(job, &e, tx),
            };
            skipped += reader.long_lines() - long;
            long = reader.long_lines();
            let mut end = match step {
                Step::More => None,
                Step::Stop(Stop::Eof) => Some(End::Done),
                Step::Stop(Stop::Deadline | Stop::Cancelled) => Some(End::Stopped),
            };
            // Inside one long line: nothing to commit yet.
            if end.is_none() && rows.is_empty() && reader.consumed() == sent {
                continue;
            }
            let file = match reader.checkpoint(step == Step::Stop(Stop::Eof)) {
                Ok(Checkpoint::Valid(file)) => file,
                Ok(Checkpoint::Rewritten(file)) => {
                    tracing::debug!(
                        path = %job.path.display(),
                        "a transcript changed while it was read; the next refresh reads it again"
                    );
                    end = Some(End::Done);
                    file
                }
                Err(e) => return self.unreadable(job, &e, tx),
            };
            self.peak.fetch_max(reader.peak(), Ordering::Relaxed);
            let read = reader.consumed() - sent;
            sent = reader.consumed();
            let msg = Msg::Chunk {
                rows: std::mem::take(&mut rows),
                skipped: std::mem::take(&mut skipped),
                file,
                read,
                end,
            };
            tx.send(msg).ok()?;
            if end.is_some() {
                return end;
            }
        }
    }

    /// Spec §5.4: a file that cannot be opened or read is skipped with a warning (its path at
    /// debug only, §13), and tried again next time; its checkpoint stays where it was.
    fn unreadable(&self, job: &Job, e: &io::Error, tx: &SyncSender<Msg>) -> Option<End> {
        tracing::warn!(kind = ?e.kind(), "a transcript could not be read; the next refresh tries it again");
        tracing::debug!(path = %job.path.display(), error = %e, "the transcript could not be read");
        tx.send(Msg::Unreadable).ok()?;
        Some(End::Done)
    }
}

/// A usage line as the index stores it, keyed by its response (spec §2.2, §4).
fn row_of(provider: &ProviderId, u: UsageLine) -> UsageRow {
    UsageRow {
        key: usage_key(provider, &u.message_id, &u.request_id),
        session_id: u.session_id,
        project: u.project,
        at_ms: u.at_ms,
        model: u.model,
        sidechain: u.sidechain,
        counts: u.counts,
        speed: u.speed,
        finalized: u.final_line,
    }
}

/// Spec §2.2's rule for two lines of one response in one batch: each count's maximum, the
/// earliest time, final once either line is, and the later line's speed when it has one, as
/// the store's upsert does across batches.
fn merge(into: &mut UsageRow, line: UsageRow) {
    into.at_ms = into.at_ms.min(line.at_ms);
    into.counts.merge_max(&line.counts);
    into.finalized |= line.finalized;
    if line.speed.is_some() {
        into.speed = line.speed;
    }
}

/// What the writer counts.
#[derive(Default)]
struct Tally {
    added: u64,
    updated: u64,
    skipped: u64,
    unreadable: u64,
    files_read: u64,
    files_done: u64,
    bytes_done: u64,
}

/// The responses waiting for their transaction, one row each, and the checkpoints whose lines
/// are all in it or in a committed one.
#[derive(Default)]
struct Batch {
    rows: Vec<UsageRow>,
    keys: HashMap<i64, usize>,
    files: BTreeMap<String, TokenFileRow>,
    removed: Vec<String>,
    since: Option<Instant>,
}

/// The one writer (spec §5.2 step 4), on the calling thread.
struct Writer<'a> {
    engine: &'a Engine,
    store: &'a Store,
    provider: &'a ProviderId,
    batch: Batch,
    tally: Tally,
    total: IndexProgress,
}

impl Writer<'_> {
    /// Takes the workers' messages until every worker is done, reporting progress after each,
    /// then commits what is left, the rows of files no longer found among it.
    fn run(
        &mut self,
        rx: Receiver<Msg>,
        progress: &mut dyn FnMut(&IndexProgress),
    ) -> Result<(), EngineError> {
        loop {
            let received = match self.batch.since {
                Some(since) => rx.recv_timeout(BATCH_WAIT.saturating_sub(since.elapsed())),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match received {
                Ok(msg) => {
                    self.take(msg)?;
                    if self
                        .batch
                        .since
                        .is_some_and(|since| since.elapsed() >= BATCH_WAIT)
                    {
                        self.commit()?;
                    }
                    progress(&IndexProgress {
                        files_done: self.tally.files_done,
                        bytes_done: self.tally.bytes_done,
                        ..self.total
                    });
                }
                Err(RecvTimeoutError::Timeout) => self.commit()?,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if self.batch.since.is_some() || !self.batch.removed.is_empty() {
            self.commit()?;
        }
        Ok(())
    }

    fn take(&mut self, msg: Msg) -> Result<(), EngineError> {
        let Msg::Chunk {
            rows,
            skipped,
            file,
            read,
            end,
        } = msg
        else {
            self.tally.unreadable += 1;
            self.tally.files_done += 1;
            return Ok(());
        };
        self.tally.skipped += skipped;
        self.tally.bytes_done += read;
        self.batch.since.get_or_insert_with(Instant::now);
        for row in rows {
            if let Some(&i) = self.batch.keys.get(&row.key) {
                merge(&mut self.batch.rows[i], row);
                continue;
            }
            // Full before this chunk's rows are all in: its checkpoint waits for the rest.
            if self.batch.rows.len() == BATCH_ROWS {
                self.commit()?;
                self.batch.since = Some(Instant::now());
            }
            self.batch.keys.insert(row.key, self.batch.rows.len());
            self.batch.rows.push(row);
        }
        self.batch.files.insert(file.path.clone(), file);
        if self.batch.rows.len() == BATCH_ROWS {
            self.commit()?;
        }
        match end {
            Some(End::Done) => {
                self.tally.files_read += 1;
                self.tally.files_done += 1;
            }
            Some(End::Stopped) => self.tally.files_read += 1,
            None => {}
        }
        Ok(())
    }

    /// One transaction: the batch's responses by the merge rule, its checkpoints, and the rows
    /// of files no longer found (spec §5.2 steps 2 and 4).
    fn commit(&mut self) -> Result<(), EngineError> {
        let files: Vec<TokenFileRow> = std::mem::take(&mut self.batch.files)
            .into_values()
            .collect();
        let outcome = self.store.commit_token_batch(
            self.provider,
            &self.batch.rows,
            &files,
            &self.batch.removed,
            self.engine.now_ms(),
        )?;
        tracing::debug!(
            responses = self.batch.rows.len(),
            files = files.len(),
            added = outcome.added,
            updated = outcome.updated,
            "committed a token batch"
        );
        self.tally.added += outcome.added;
        self.tally.updated += outcome.updated;
        self.batch.rows.clear();
        self.batch.keys.clear();
        self.batch.removed.clear();
        self.batch.since = None;
        Ok(())
    }
}

impl Engine {
    /// Spec §5: brings the provider's index up to date with its transcripts. It takes the
    /// provider's tokens lock, waiting for another refresh (`Wait::Wait`, cancellably) or
    /// returning `Busy` (`Wait::Try`). With a `budget` it stops at the next line boundary once
    /// the budget is spent, commits what it read, and returns `complete: false`; the next call
    /// goes on from there. A cancelled refresh commits what it parsed, then returns the
    /// interruption.
    pub fn refresh_tokens(
        &self,
        provider: &ProviderId,
        wait: Wait,
        budget: Option<Duration>,
        cancel: &Cancel,
        progress: &mut dyn FnMut(&IndexProgress),
    ) -> Result<RefreshOutcome, EngineError> {
        Ok(self
            .refresh_with(provider, wait, budget, cancel, progress, Limits::SPEC)?
            .0)
    }

    /// `refresh_tokens` with the reader's sizes; also returns the most bytes any reader held.
    pub(crate) fn refresh_with(
        &self,
        provider: &ProviderId,
        wait: Wait,
        budget: Option<Duration>,
        cancel: &Cancel,
        progress: &mut dyn FnMut(&IndexProgress),
        limits: Limits,
    ) -> Result<(RefreshOutcome, usize), EngineError> {
        let started_ms = self.now_ms();
        let p = self.provider(provider)?;
        if !p.capabilities().transcripts {
            return Err(EngineError::InvalidInput(format!(
                "{} has no transcripts",
                p.display_name()
            )));
        }
        // Step 1, holding no lock: sightings are written without the tokens lock (spec §5.1),
        // and so nothing is taken while it is held.
        let evidence_changed = self.scan_sessions(p.as_ref())?;
        let path = tokens_lock_path(&self.env, provider);
        hooks::point(self, "refresh-tokens-lock")?;
        let _lock = match wait {
            Wait::Try => match FlockGuard::try_lock(&path)? {
                Some(lock) => lock,
                None => return Ok((RefreshOutcome::Busy, 0)),
            },
            Wait::Wait => lock_until_free(&path, cancel).map_err(|e| match e {
                LockError::Interrupted { signal, .. } => EngineError::Interrupted(signal),
                e => EngineError::Lock(e),
            })?,
        };
        hooks::point(self, "refresh-tokens-locked")?;
        let store = self.store()?;
        let retention = i64::from(self.settings.tokens_retention_days).saturating_mul(DAY_MS);
        let cutoff_ms = self.now_ms().saturating_sub(retention);
        let (jobs, removed, unreadable) =
            self.token_jobs(p.as_ref(), store.token_files(provider)?)?;
        // The budget starts here: discovery and the wait for the lock are not budgeted (spec
        // §5.2).
        let deadline = budget.and_then(|b| Instant::now().checked_add(b));
        let total = IndexProgress {
            files_done: 0,
            files_total: jobs.len() as u64,
            bytes_done: 0,
            bytes_total: jobs.iter().map(|j| j.bytes).sum(),
        };
        progress(&total);
        let work = Work {
            p: p.as_ref(),
            provider,
            jobs: &jobs,
            next: AtomicUsize::new(0),
            limits,
            deadline,
            cancel,
            halt: AtomicBool::new(false),
            cutoff_ms,
            peak: AtomicUsize::new(0),
        };
        let workers = thread::available_parallelism()
            .map_or(1, NonZeroUsize::get)
            .min(MAX_WORKERS)
            .min(jobs.len());
        let mut writer = Writer {
            engine: self,
            store: &store,
            provider,
            batch: Batch {
                removed,
                ..Batch::default()
            },
            tally: Tally {
                unreadable,
                ..Tally::default()
            },
            total,
        };
        thread::scope(|s| {
            let (tx, rx) = mpsc::sync_channel(workers.max(1));
            for _ in 0..workers {
                let (tx, work) = (tx.clone(), &work);
                s.spawn(move || work.run(&tx));
            }
            drop(tx);
            // Returning drops `rx`, so a worker blocked on a full channel stops too.
            let written = writer.run(rx, progress);
            if written.is_err() {
                work.halt.store(true, Ordering::Relaxed);
            }
            written
        })?;
        let tally = writer.tally;
        let mut complete = tally.files_done == jobs.len() as u64;
        if complete {
            // Step 5. The prune stops with the budget or the cancel token, as reading does, and
            // a prune left unfinished leaves the refresh incomplete, so the next call goes on.
            // The store asks `stop` before each chunk; the first chunk of a call is let through
            // whatever the budget, so every call makes progress (spec §5.2 Budget). The cancel
            // token stops it at once.
            let holder = uuid::Uuid::now_v7().to_string();
            let first = Cell::new(true);
            let stop = || cancel.requested().is_some() || (!first.replace(false) && past(deadline));
            complete = store
                .prune_tokens(provider, cutoff_ms, &holder, self.now_ms(), &stop)?
                .complete;
        }
        if let (false, Some(signal)) = (complete, cancel.requested()) {
            return Err(EngineError::Interrupted(signal));
        }
        if complete {
            // The store keeps `responses` itself, each batch and prune counting it, so the count
            // passed on is the stored one.
            let stats = RefreshStats {
                files: store.token_files(provider)?.len() as u64,
                responses: store
                    .token_state(provider)?
                    .map_or(0, |state| state.stats.responses),
                skipped_lines: tally.skipped,
                unreadable_files: tally.unreadable,
            };
            store.finish_token_refresh(provider, &stats, self.now_ms())?;
        }
        let report = IndexReport {
            complete,
            files_read: tally.files_read,
            responses_added: tally.added,
            responses_updated: tally.updated,
            skipped_lines: tally.skipped,
            evidence_changed,
            took_ms: u64::try_from(self.now_ms().saturating_sub(started_ms)).unwrap_or(0),
        };
        tracing::info!(
            provider = self.registered_id(provider),
            complete,
            files = report.files_read,
            added = report.responses_added,
            updated = report.responses_updated,
            skipped = report.skipped_lines,
            took_ms = report.took_ms,
            "refreshed the token index"
        );
        Ok((
            RefreshOutcome::Done(report),
            work.peak.load(Ordering::Relaxed),
        ))
    }

    /// Step 2 (spec §5.2), not budgeted: every transcript file, `stat`ed once. Returns the
    /// files to read, by path; the stored rows of files no longer found, whose usage stays;
    /// and how many could not be `stat`ed.
    fn token_jobs(
        &self,
        p: &dyn Provider,
        stored: Vec<TokenFileRow>,
    ) -> Result<(Vec<Job>, Vec<String>, u64), EngineError> {
        let mut stored: HashMap<String, TokenFileRow> =
            stored.into_iter().map(|r| (r.path.clone(), r)).collect();
        let (mut jobs, mut found, mut unreadable) = (Vec::new(), HashSet::new(), 0);
        for path in p.transcript_files(&self.env)? {
            let key = path.to_string_lossy().into_owned();
            if !found.insert(key.clone()) {
                continue;
            }
            let row = stored.remove(&key);
            match fs::metadata(&path) {
                Ok(meta) if !meta.is_file() || unchanged(&meta, row.as_ref()) => {}
                Ok(meta) => {
                    let resume = row
                        .as_ref()
                        .filter(|r| {
                            (r.dev, r.ino) == (meta.dev(), meta.ino()) && meta.len() >= r.offset
                        })
                        .map_or(0, |r| r.offset);
                    jobs.push(Job {
                        bytes: meta.len() - resume,
                        path,
                        stored: row,
                    });
                }
                // Gone since the listing: the next refresh drops its row.
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    unreadable += 1;
                    tracing::warn!(kind = ?e.kind(), "a transcript could not be read; the next refresh tries it again");
                    tracing::debug!(path = %path.display(), error = %e, "the transcript could not be read");
                }
            }
        }
        jobs.sort_by(|a, b| a.path.cmp(&b.path));
        Ok((jobs, stored.into_keys().collect(), unreadable))
    }
}
```

- [ ] **Step 4: Run them and see them pass**

```
cargo test -p tagteam-engine --locked --features test-hooks --test tokens_refresh
cargo test -p tagteam-engine --locked --test tokens_refresh
cargo test -p tagteam-engine --locked --lib tokens::
```

Expected:
- `tokens_refresh` with `test-hooks`: `test result: ok. 15 passed; 0 failed`. Without the feature, 14 pass: the lock-wait test needs the engine's hook points.
- the lib run: `test result: ok. 15 passed; 0 failed`: Task 14's 13, Task 17's lock-wait test, and `memory_and_budget_hold_on_a_file_of_many_chunks`.

- [ ] **Step 5: Lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy -p tagteam-engine --all-targets --locked -- -D warnings
cargo clippy -p tagteam-engine --all-targets --locked --features test-hooks -- -D warnings
```

Expected: no output from the format check, and clippy finishes with no warnings.

- [ ] **Step 6: Commit**

```
git add crates/tagteam-engine/src/tokens/refresh.rs crates/tagteam-engine/src/tokens/reader.rs crates/tagteam-engine/tests/tokens_refresh.rs
git commit -m "Add the incremental token index refresh"
```

#### Notes for the controller

1. **No contract addition.** Task 12's store keeps `token_state.responses` itself, and `finish_token_refresh` does not write `stats.responses`. So the refresh passes:
   - `files`: the provider's `token_files` rows, read once more at completion, a few KB;
   - `responses`: the stored count, so the struct stays truthful.

   Task 12's `finish_token_refresh` also creates the state row, so a refresh that commits nothing needs no batch of its own.
2. **Step 1 runs before the lock.** Spec §5.1 says sightings are written without the tokens lock, and running the scan first keeps that lock a strict leaf: nothing, not even a reservation probe, is taken while it is held. `Wait::Try` therefore scans, then returns `Busy`. The scan takes a few ms of small reads.
3. **Step 5 prunes before it finishes**, so `files` and the stored `responses` describe what stays. The outline put finish first; the spec only says "Prune if due".
   - **The prune is budgeted and cancellable (plan review).** It stops early when the deadline passes or the cancel token is set. If it stops early, the refresh returns `complete: false` without finishing, so the daemon runs it again at once; if the cancel token stopped it, the refresh returns the interruption.
   - **Every call still makes progress.** Task 12's `prune_tokens` asks `stop` before each chunk, the first included. With a zero budget, or a slice whose reading used up its budget, a plain `past(deadline)` would delete nothing on every call, and no such refresh could ever complete. So the refresh's `stop` lets the first chunk of each call through whatever the budget (a `Cell` flag), and stops at once only for the cancel token. A slice therefore overruns its budget by at most one prune chunk, as it does by one line when reading.
   - I verified this in the scratch crate with a stand-in store that asks `stop` exactly as Task 12 does. Without the flag, the budgeted-prune and zero-budget tests fail; with it, they pass, 28 of 28, 3 runs out of 3.
   - Tests: `a_budgeted_prune_goes_on_over_several_calls` and `a_cancelled_prune_reports_interrupted`, each over 12,001 expired rows. That is more than one prune chunk at Task 12's `PRUNE_CHUNK` of 5,000; it has no test constructor, so the tests size the rows instead.
   - **The budget starts after discovery and after the lock is held (plan review).** Spec §5.2 leaves discovery unbudgeted, and the wait for the lock is not reading.
4. **In-batch merge.** The writer merges lines of one response within a batch by spec §2.2's rule, using Task 2's `merge_max`. Task 12's `commit_token_batch` merges too, so this is about sizing: `BATCH_ROWS` counts distinct responses, and fewer rows cross into the transaction for CC's up-to-9-lines-per-response pattern.
5. **Rows are converted in the workers** (`usage_key`, the retention cutoff), not in the writer as the outline said, so hashing runs in parallel and the writer only does SQL.
6. **Progress** is reported after every message (each chunk, and each file's end), not only per file, so `bytes_done` moves smoothly. The cancel test relies on this to be deterministic.
7. **`memory_and_budget_hold_on_a_file_of_many_chunks` is a unit test in `refresh.rs`.** It needs crate-internal `Limits` and the reader peak that `refresh_with` returns; an integration test cannot see memory.
8. **`skipped_lines` and `unreadable_files` count the completing call only.** A refresh sliced over several calls stores the last call's counts.
9. **Assumptions about earlier tasks.**
   - Task 7 re-exports `UsageLine` at `tagteam_provider::UsageLine`.
   - The CC fixture test uses retention 3,650 days, because the fixtures' dates are unknown here.
   - **Logging (Task 22).** The refresh logs one `info` summary under its module's target, `tagteam_engine::tokens::refresh`, which falls under `tagteam_engine::tokens`. The summary carries the provider, completeness, counts and `took_ms`. Transcript paths appear only at `debug`, and content never does (spec §13). Each batch also logs a `debug` line with counts.
10. **Deterministic lock tests (plan review).**
    - `refresh_with` passes two hook points, `refresh-tokens-lock` immediately before taking the lock and `refresh-tokens-locked` immediately after.
    - `a_waiting_refresh_never_passes_the_lock_while_another_holds_it` (feature `test-hooks`) holds the lock and waits on a channel for the first point, then sets the cancel token. The refresh must return the interruption without ever reaching the second point. Were the lock acquisition removed, nothing between the two points could be interrupted, so the test would fail deterministically.
    - Once the lock is released, the next refresh reaches the second point, and `try_lock` there finds the lock held.
    - The Try test asserts `Busy` with no elapsed-time check. No functional test here asserts elapsed time; timing belongs to Task 23's `#[ignore]` tests.
    - **Unreadable files under root.** `an_unreadable_file_is_counted_and_retried` makes a file mode 000, then tries `File::open`. If that succeeds (a privileged run), it restores the mode and returns early with a comment, since no file can be unreadable there. I chose this over injecting a failure (a symlink loop, say) because Task 10's discovery may not list symlinks, so the skip is the deterministic option.
11. **Batch order.** This task runs after Task 16, whose real `scan_sessions` it calls, and Task 17, whose `lock_until_free` in `tokens/mod.rs` it uses as `super::lock_until_free`. It does not edit `mod.rs`. It removes `reader.rs`'s `#![allow(dead_code)]`, and nothing else touches `reader.rs` in between.
12. **What I verified, and what I did not.** I could not compile this task's Step 3 inside the engine: Tasks 7, 12 and 13 do not exist yet. Instead I compiled it verbatim in a scratch crate. That crate held the real Task 14 `tokens/` files and Task 17's `lock_until_free`, over the real `tagteam-core` and `tagteam-provider`, with in-memory stand-ins for `Engine`, `Store`, `EngineError` and a line-format provider. The only edit was swapping the `Provider`/`UsageLine` import for the stand-ins'. Results:
    - clippy `-D warnings` was clean;
    - rustfmt was clean (the text above is the formatted code);
    - 11 scenario tests passed on 8 runs out of 8, together with the reader's and lock's 13 tests.

    The scenarios: the cold index and the in-batch merge; 8,000 rows over 10,000 lines, with production chunks and with 64-byte chunks; one line per call under a zero budget; cancel mid-file; `Try`/`Wait`; refusal without `transcripts`; an empty tree and a removed file; the memory bound; retention; an unreadable file. A store-side check also asserted on every commit that each checkpoint's lines were already committed and that no batch exceeded 2,000 rows.

    After Task 12's semantics were known, I aligned the stand-in store with them: `responses` is kept by batches and prunes, and `finish_token_refresh` creates the state row and leaves `responses` alone. I dropped the `token_counts` request and re-ran: still clippy-clean, and 24 of 24 passed: Task 14's 11 reader tests, the 2 `tokens::tests` (lock path and lock wait) and the 11 scenarios.

    After the plan review, the stand-ins gained more: a hook `on_point`, and a store whose prune works in chunks of 5, checks `stop` after each chunk, and returns `PruneOutcome`. The re-run (clippy-clean, 28 of 28 passed, 3 runs out of 3) covered the same scenarios plus:
    - the hook-based Wait test, which fails if the lock acquisition is removed;
    - Try without timing;
    - a budgeted prune over 3 calls;
    - a cancelled prune.

    I did **not** compile this task's two test files (`tests/tokens_refresh.rs` and the unit test against FakeAgent) against the real engine, Task 10's fake, or Task 8's CC fixture tree.

---

### Task 16: Session evidence

**Depends on:** 7, 12, 14
**Files:**
- Modify: `crates/tagteam-engine/src/tokens/evidence.rs` (replace it whole: Task 14's `LaunchId`, word for word, plus everything below)
- Modify: `crates/tagteam-engine/src/views.rs` (`CACHE_WRITE_WAIT` becomes `pub(crate)`)
- Create: `crates/tagteam-engine/tests/tokens_evidence.rs`
- Test: `crates/tagteam-engine/tests/tokens_evidence.rs`

**Interfaces:**
- Consumes:
  - Task 7: `SessionRecord.session_id: Option<String>` (from `sessionId`); `ProcessProbe::parent_pid(&self, pid: u32) -> Option<u32>`; `FakeProcess.ppid: Option<u32>`; `Provider::default_session_records_dir(&self, env: &Env) -> Option<PathBuf>`; `Capabilities.transcripts`.
  - Task 12 (`crate::store`):
    - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum SightingSource { Run, Scan, Statusline }`
    - `pub struct SightingRow { pub session_id: String, pub home: String, pub first_at: i64, pub last_at: i64, pub source: SightingSource, pub recorded_at: i64 }`
    - `pub struct LaunchRow { pub account_id: String, pub started_at: i64, pub ended_at: Option<i64>, pub pid: Option<u32>, pub recorded_at: i64 }`
    - `Store::{record_sighting, set_sighting_last_at, latest_sighting, sightings, open_launch, attach_launch_pid, close_launch, launches}` with the contract's signatures.
  - Task 12: `pub fn record_sighting_within(&self, provider: &ProviderId, s: &SightingRow, min_gap_ms: i64, wait: Duration) -> Result<bool, StoreError>` (added to Task 12 at this task's request; see note 1).
  - Task 14: `LaunchId` in `evidence.rs`.
  - Existing:
    - `Engine::{session_state_at, existing_store, store, provider, registered_id, now_ms}` and `Engine.process`;
    - `tagteam_provider::{ProfileMarker, Read, RecordEntry, parse_session_record, profile_path, read_session_records, record_is_live}`; in the tests, `LAUNCH_DIR` and `FlockGuard`;
    - `crate::views::CACHE_WRITE_WAIT` (made `pub(crate)` here);
    - `StoreError::sqlite_code()`.
- Produces (contract, `tagteam_engine::tokens`):
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)] pub struct LaunchId { pub provider: ProviderId, pub account: AccountId, pub started_at_ms: i64 }
  #[derive(Debug, Clone, Default, PartialEq, Eq)] pub struct LaunchPoll { pub last_mtime_ns: Option<i128>, pub last_sighting: Option<(String, i64)> }
  impl Engine {
      pub fn sight_sessions(&self, provider: &ProviderId) -> Result<u64, EngineError>;
      pub(crate) fn scan_sessions(&self, p: &dyn Provider) -> Result<u64, EngineError>; // refresh step 1 (Task 15 calls it)
      pub fn open_launch(&self, provider: &ProviderId, account: &AccountId) -> Result<LaunchId, EngineError>;
      pub fn attach_launch_pid(&self, id: &LaunchId, pid: u32) -> Result<(), EngineError>;
      pub fn poll_launch(&self, id: &LaunchId, profile: &Path, pid: u32, state: &mut LaunchPoll) -> Result<(), EngineError>;
      pub fn close_launch(&self, id: &LaunchId, profile: &Path, state: &LaunchPoll, exited_at_ms: i64) -> Result<(), EngineError>;
      pub fn statusline_sighting(&self, provider: &ProviderId, session_id: &str, profile: Option<&Path>) -> Result<(), EngineError>;
  }
  ```

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-engine/tests/tokens_evidence.rs`:

```rust
//! Spec §6: the evidence that places a session in a home: sightings from the scan, from `run`'s
//! poll and from the statusline, and `run`'s launches. Session records are judged by the
//! fixture's `FakeProcessProbe`, never by a real pid, and times come from the fixture's clock.
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::{Fx, LSTART};
use serde_json::json;
use tagteam_engine::store::{LaunchRow, SightingRow, SightingSource};
use tagteam_engine::tokens::{LaunchId, LaunchPoll};
use tagteam_provider::{FakeProcess, FlockGuard, LAUNCH_DIR, parse_lstart};

const NOW: i64 = 1_790_000_000_000;

/// `pid` runs, started at `LSTART`, under `ppid`.
fn running(fx: &Fx, pid: u32, ppid: Option<u32>) {
    fx.process.set(
        pid,
        FakeProcess {
            exists: Some(true),
            start_time_s: parse_lstart(LSTART),
            ppid,
            ..FakeProcess::default()
        },
    );
}

/// `pid` has exited.
fn gone(fx: &Fx, pid: u32) {
    fx.process.set(
        pid,
        FakeProcess {
            exists: Some(false),
            ..FakeProcess::default()
        },
    );
}

/// `pid`'s session record of `kind` in `home` (the default home's config directory, or a
/// profile), naming `session`, started at `started_at`.
fn record(fx: &Fx, home: &Path, pid: u32, session: &str, started_at: i64, kind: &str) -> PathBuf {
    let record = json!({
        "pid": pid,
        "procStart": LSTART,
        "startedAt": started_at,
        "kind": kind,
        "sessionId": session
    });
    fx.plant_record(home, &pid.to_string(), record.to_string().as_bytes())
}

fn sighting(
    session: &str,
    home: &str,
    first_at: i64,
    last_at: i64,
    source: SightingSource,
    recorded_at: i64,
) -> SightingRow {
    SightingRow {
        session_id: session.into(),
        home: home.into(),
        first_at,
        last_at,
        source,
        recorded_at,
    }
}

/// Claude Code's sightings, by home, session and start.
fn sightings(fx: &Fx) -> Vec<SightingRow> {
    let mut all = fx
        .engine
        .store()
        .unwrap()
        .sightings(&fx.provider())
        .unwrap();
    all.sort_by(|a, b| {
        (&a.home, &a.session_id, a.first_at).cmp(&(&b.home, &b.session_id, b.first_at))
    });
    all
}

/// Claude Code's launches, by account and start.
fn launches(fx: &Fx) -> Vec<LaunchRow> {
    let mut all = fx.engine.store().unwrap().launches(&fx.provider()).unwrap();
    all.sort_by(|a, b| (&a.account_id, a.started_at).cmp(&(&b.account_id, b.started_at)));
    all
}

/// When each launch ended, by account and start.
fn ended(fx: &Fx) -> Vec<(String, Option<i64>)> {
    launches(fx)
        .into_iter()
        .map(|l| (l.account_id, l.ended_at))
        .collect()
}

fn sight(fx: &Fx) -> u64 {
    fx.engine.sight_sessions(&fx.provider()).unwrap()
}

/// Sets `path`'s mtime to `secs` after the epoch.
fn set_mtime(path: &Path, secs: u64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap();
}

#[test]
fn a_scan_sights_live_records_in_every_home() {
    // Spec §6.2: every live record in the default home and in each profile, keyed by its
    // `startedAt`, extended at most once a minute (spec §6.4).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    running(&fx, 101, None);
    record(
        &fx,
        &fx.paths().config_home,
        101,
        "s-default",
        NOW - 5_000,
        "interactive",
    );
    running(&fx, 202, None);
    record(&fx, &profile, 202, "s-profile", NOW - 9_000, "interactive");

    assert_eq!(sight(&fx), 2);
    let scan = SightingSource::Scan;
    assert_eq!(
        sightings(&fx),
        [
            sighting("s-default", "", NOW - 5_000, NOW, scan, NOW),
            sighting("s-profile", a.as_str(), NOW - 9_000, NOW, scan, NOW),
        ]
    );

    fx.clock.advance_ms(30_000);
    assert_eq!(sight(&fx), 0, "within the minute");
    fx.clock.advance_ms(31_000);
    assert_eq!(sight(&fx), 2);
    let later = NOW + 61_000;
    assert_eq!(
        sightings(&fx),
        [
            sighting("s-default", "", NOW - 5_000, later, scan, later),
            sighting("s-profile", a.as_str(), NOW - 9_000, later, scan, later),
        ]
    );
}

#[test]
fn a_scan_skips_dead_and_unreadable_records() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let home = fx.paths().config_home;
    // A record whose process is gone.
    record(&fx, &home, 303, "s-dead", NOW, "interactive");
    // One caught part-way through its write.
    fx.plant_record(&home, "404", b"{\"pid\": 404, \"sessionId\"");
    // One without a `sessionId`, and one without a `startedAt`.
    running(&fx, 505, None);
    let no_session = json!({"pid": 505, "procStart": LSTART, "startedAt": NOW});
    fx.plant_record(&home, "505", no_session.to_string().as_bytes());
    running(&fx, 606, None);
    let no_start = json!({"pid": 606, "procStart": LSTART, "sessionId": "s-6"});
    fx.plant_record(&home, "606", no_start.to_string().as_bytes());

    assert_eq!(sight(&fx), 0);
    assert!(sightings(&fx).is_empty());
}

#[test]
fn a_scan_closes_launches_of_idle_profiles_only() {
    // Spec §6.1: a launch is closed, at the moment it is found so, once its profile is no
    // longer in use; that covers a `run` killed while its session lived on.
    let fx = Fx::new();
    let cc = fx.provider();
    let (a, b, c) = (
        fx.add("a@x.co", "rt-a"),
        fx.add("b@x.co", "rt-b"),
        fx.add("c@x.co", "rt-c"),
    );
    let pa = fx.make_profile(&a);
    fx.make_profile(&b);
    for id in [&a, &b, &c] {
        fx.engine.open_launch(&cc, id).unwrap();
    }
    // a's session still runs; b's profile is idle; c has no profile left.
    running(&fx, 101, None);
    record(&fx, &pa, 101, "s-a", NOW, "interactive");
    fx.clock.advance_ms(10_000);

    assert_eq!(
        sight(&fx),
        3,
        "a's sighting, and b's and c's launches closed"
    );
    let mut want = vec![
        (a.to_string(), None),
        (b.to_string(), Some(NOW + 10_000)),
        (c.to_string(), Some(NOW + 10_000)),
    ];
    want.sort();
    assert_eq!(ended(&fx), want);

    gone(&fx, 101);
    fx.clock.advance_ms(1_000);
    assert_eq!(sight(&fx), 1, "a's launch closes once its session is gone");
    assert!(ended(&fx).iter().all(|(_, end)| end.is_some()));
}

#[test]
fn a_launch_is_open_before_the_spawn_and_attached_after() {
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    assert_eq!(
        id,
        LaunchId {
            provider: cc.clone(),
            account: a.clone(),
            started_at_ms: NOW
        }
    );
    let open = LaunchRow {
        account_id: a.to_string(),
        started_at: NOW,
        ended_at: None,
        pid: None,
        recorded_at: NOW,
    };
    assert_eq!(launches(&fx), [open.clone()]);

    fx.clock.advance_ms(50);
    fx.engine.attach_launch_pid(&id, 4242).unwrap();
    assert_eq!(
        launches(&fx),
        [LaunchRow {
            pid: Some(4242),
            recorded_at: NOW + 50,
            ..open
        }]
    );
}

#[test]
fn polling_reads_the_record_only_when_it_changed() {
    // Spec §6.1: `run` reads its child's record only when its mtime moved, and extends the
    // sighting of the session it saw last, at most once a minute (spec §6.4).
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    let run = SightingSource::Run;

    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    assert_eq!(state, LaunchPoll::default(), "no record yet");
    assert!(sightings(&fx).is_empty());

    // The child, spawned once the launch began, writes its record.
    let started = NOW + 1_000;
    fx.clock.advance_ms(2_000);
    let polled = NOW + 2_000;
    running(&fx, 777, None);
    let path = record(&fx, &profile, 777, "s-a", started, "interactive");
    set_mtime(&path, 1_790_000_000);
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    assert_eq!(state.last_sighting, Some(("s-a".to_owned(), started)));
    assert_eq!(
        sightings(&fx),
        [sighting("s-a", a.as_str(), started, polled, run, polled)]
    );

    // Rewritten under the same mtime: not read again, and the sighting it saw is extended.
    record(&fx, &profile, 777, "s-b", started, "interactive");
    set_mtime(&path, 1_790_000_000);
    fx.clock.advance_ms(61_000);
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    assert_eq!(state.last_sighting, Some(("s-a".to_owned(), started)));
    let later = polled + 61_000;
    assert_eq!(
        sightings(&fx),
        [sighting("s-a", a.as_str(), started, later, run, later)]
    );

    // Its mtime moves: read again.
    set_mtime(&path, 1_790_000_001);
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    assert_eq!(state.last_sighting, Some(("s-b".to_owned(), started)));
    assert_eq!(
        sightings(&fx),
        [
            sighting("s-a", a.as_str(), started, later, run, later),
            sighting("s-b", a.as_str(), started, later, run, later),
        ]
    );
}

#[test]
fn a_stale_record_with_a_reused_pid_is_ignored() {
    // A crashed execution left `777.json` behind, and the OS gave its pid to this child. The
    // record names an older execution, started before this launch began: it is never sighted,
    // nor remembered. Neither is a record of another pid under that name, nor one whose process
    // is gone. The child's own record, once it writes one, is.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    running(&fx, 777, None);
    let path = record(&fx, &profile, 777, "s-old", NOW - 3_600_000, "interactive");
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    assert_eq!(state, LaunchPoll::default(), "not remembered");
    assert!(sightings(&fx).is_empty(), "not sighted");

    let other_pid = json!({
        "pid": 778,
        "procStart": LSTART,
        "startedAt": NOW + 10,
        "kind": "interactive",
        "sessionId": "s-x"
    });
    fs::write(&path, other_pid.to_string()).unwrap();
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    gone(&fx, 777);
    record(&fx, &profile, 777, "s-dead", NOW + 10, "interactive");
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    assert_eq!(state, LaunchPoll::default());
    assert!(sightings(&fx).is_empty());

    running(&fx, 777, None);
    fx.clock.advance_ms(1_000);
    record(&fx, &profile, 777, "s-new", NOW + 500, "interactive");
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    assert_eq!(state.last_sighting, Some(("s-new".to_owned(), NOW + 500)));
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-new",
            a.as_str(),
            NOW + 500,
            NOW + 1_000,
            SightingSource::Run,
            NOW + 1_000
        )]
    );
}

#[test]
fn close_leaves_the_launch_open_while_a_background_session_runs() {
    // Spec §6.1: at the child's exit the sighting it saw last ends then, and the launch closes
    // only once the profile is no longer in use. A background session the child started keeps
    // it open; the next scan that finds the profile idle closes it.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    running(&fx, 777, None);
    let child = record(&fx, &profile, 777, "s-a", NOW, "interactive");
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    // The child exits and its record goes; a background session it started lives on.
    fs::remove_file(&child).unwrap();
    gone(&fx, 777);
    running(&fx, 888, None);
    let background = record(&fx, &profile, 888, "s-bg", NOW, "bg");
    fx.clock.advance_ms(5_000);

    let exit = fx.engine.now_ms();
    fx.engine.close_launch(&id, &profile, &state, exit).unwrap();
    assert!(
        sightings(&fx).contains(&sighting(
            "s-a",
            a.as_str(),
            NOW,
            exit,
            SightingSource::Run,
            exit
        )),
        "{:?}",
        sightings(&fx)
    );
    assert_eq!(ended(&fx), [(a.to_string(), None)]);

    gone(&fx, 888);
    fs::remove_file(&background).unwrap();
    fx.clock.advance_ms(1_000);
    sight(&fx);
    assert_eq!(ended(&fx), [(a.to_string(), Some(NOW + 6_000))]);
}

#[test]
fn close_leaves_the_launch_open_while_another_launcher_holds_the_profile() {
    // Another process's held launch reservation (spec §12.5) is the profile in use: the launch
    // stays open until a close or a scan finds the profile idle.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let other = FlockGuard::try_lock(&profile.join(LAUNCH_DIR).join("4242.lock"))
        .unwrap()
        .unwrap();
    let first = fx.engine.open_launch(&cc, &a).unwrap();
    fx.engine
        .close_launch(&first, &profile, &LaunchPoll::default(), NOW)
        .unwrap();
    assert_eq!(
        ended(&fx),
        [(a.to_string(), None)],
        "another launcher holds the profile"
    );

    drop(other);
    fx.clock.advance_ms(1_000);
    let second = fx.engine.open_launch(&cc, &a).unwrap();
    fx.engine
        .close_launch(&second, &profile, &LaunchPoll::default(), NOW + 1_000)
        .unwrap();
    assert_eq!(
        ended(&fx),
        [(a.to_string(), None), (a.to_string(), Some(NOW + 1_000))]
    );
}

#[test]
fn close_stamps_the_sighting_at_the_exit_not_at_the_close() {
    // Spec §6.1: the sighting ends when the child exited. `run`'s exit handling, which may wait
    // on locks, comes before the close; only the launch may close late.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let id = fx.engine.open_launch(&cc, &a).unwrap();
    let mut state = LaunchPoll::default();
    running(&fx, 777, None);
    let child = record(&fx, &profile, 777, "s-a", NOW + 500, "interactive");
    fx.clock.advance_ms(1_000);
    fx.engine
        .poll_launch(&id, &profile, 777, &mut state)
        .unwrap();
    // The child exits at t1; exit handling takes until t2.
    fx.clock.advance_ms(30_000);
    let t1 = fx.engine.now_ms();
    fs::remove_file(&child).unwrap();
    gone(&fx, 777);
    fx.clock.advance_ms(45_000);
    let t2 = fx.engine.now_ms();

    fx.engine.close_launch(&id, &profile, &state, t1).unwrap();
    let run = SightingSource::Run;
    assert_eq!(
        sightings(&fx),
        [sighting("s-a", a.as_str(), NOW + 500, t1, run, t2)]
    );
    assert_eq!(ended(&fx), [(a.to_string(), Some(t2))]);
}

#[test]
fn the_statusline_extends_only_its_own_execution() {
    // Spec §6.3: the statusline finds the Claude Code process it runs under among its
    // ancestors, by that process's record in the home, and writes that execution's sighting.
    // An earlier execution's sighting of the same session is never extended.
    let fx = Fx::new();
    let cc = fx.provider();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    running(&fx, std::process::id(), Some(500));
    running(&fx, 500, Some(600));
    running(&fx, 600, Some(1));
    // Claude Code, two levels up, in the default home.
    record(
        &fx,
        &fx.paths().config_home,
        600,
        "s-1",
        NOW - 60_000,
        "interactive",
    );
    let scan = SightingSource::Scan;
    let earlier = sighting(
        "s-1",
        "",
        NOW - 3_600_000,
        NOW - 3_000_000,
        scan,
        NOW - 3_000_000,
    );
    fx.engine
        .store()
        .unwrap()
        .record_sighting(&cc, &earlier, 60_000)
        .unwrap();

    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    let statusline = SightingSource::Statusline;
    let ours = sighting("s-1", "", NOW - 60_000, NOW, statusline, NOW);
    assert_eq!(sightings(&fx), [earlier.clone(), ours.clone()]);

    // Within the minute, nothing is written; past it, only its own execution's sighting moves.
    fx.clock.advance_ms(30_000);
    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    assert_eq!(sightings(&fx), [earlier.clone(), ours.clone()]);
    fx.clock.advance_ms(31_000);
    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    let later = NOW + 61_000;
    assert_eq!(
        sightings(&fx),
        [
            earlier.clone(),
            SightingRow {
                last_at: later,
                recorded_at: later,
                ..ours
            }
        ]
    );

    // In a run shell, the home is the profile, and the profile's records are searched: there,
    // 500 is the Claude Code process.
    record(&fx, &profile, 500, "s-2", NOW - 1_000, "interactive");
    fx.engine
        .statusline_sighting(&cc, "s-2", Some(&profile))
        .unwrap();
    assert!(sightings(&fx).contains(&sighting(
        "s-2",
        a.as_str(),
        NOW - 1_000,
        later,
        statusline,
        later
    )));
}

#[test]
fn the_statusline_records_nothing_without_an_ancestor_record() {
    let fx = Fx::new();
    let cc = fx.provider();
    let home = fx.paths().config_home;
    // Without a store, it creates none.
    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    assert!(!fx.env.data_dir().join("tagteam.db").exists());

    fx.add("a@x.co", "rt-a");
    // Nine ancestors; only the ninth has a record, and so does a process that is no ancestor.
    let chain: Vec<u32> = (1001..=1009).collect();
    running(&fx, std::process::id(), Some(chain[0]));
    for pair in chain.windows(2) {
        running(&fx, pair[0], Some(pair[1]));
    }
    running(&fx, chain[8], Some(1));
    record(&fx, &home, chain[8], "s-1", NOW, "interactive");
    running(&fx, 4242, Some(1));
    record(&fx, &home, 4242, "s-1", NOW, "interactive");
    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    assert!(
        sightings(&fx).is_empty(),
        "the walk stops after eight ancestors"
    );

    // The eighth is within reach.
    record(&fx, &home, chain[7], "s-1", NOW - 5_000, "interactive");
    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-1",
            "",
            NOW - 5_000,
            NOW,
            SightingSource::Statusline,
            NOW
        )]
    );
}

#[test]
fn the_statusline_never_waits_on_a_busy_store() {
    // Spec §6.3: the write waits at most `CACHE_WRITE_WAIT` (5 ms); a busy store skips it, and
    // the call still succeeds. How long it took is left to the `#[ignore]` perf tests.
    let fx = Fx::new();
    let cc = fx.provider();
    fx.add("a@x.co", "rt-a");
    running(&fx, std::process::id(), Some(500));
    running(&fx, 500, Some(1));
    record(&fx, &fx.paths().config_home, 500, "s-1", NOW, "interactive");
    let other = rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db")).unwrap();
    other.execute_batch("BEGIN IMMEDIATE;").unwrap();

    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    other.execute_batch("ROLLBACK;").unwrap();
    assert!(sightings(&fx).is_empty(), "a busy store skips the write");

    fx.engine.statusline_sighting(&cc, "s-1", None).unwrap();
    assert_eq!(
        sightings(&fx),
        [sighting(
            "s-1",
            "",
            NOW,
            NOW,
            SightingSource::Statusline,
            NOW
        )]
    );
}
```

- [ ] **Step 2: Run them and see them fail**

```
cargo test -p tagteam-engine --locked --test tokens_evidence
```

Expected: the test crate does not compile:
- `cannot find type `LaunchPoll` in module `tagteam_engine::tokens``;
- `no method named `sight_sessions` found for struct `Engine``, and likewise for `open_launch`, `attach_launch_pid`, `poll_launch`, `close_launch` and `statusline_sighting`.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/views.rs`, make the cache's wait reachable from the evidence module:

```rust
/// The longest `statusline` waits on the store's write lock to cache the live identity: the
/// cache is a convenience, and the line is never held up for it (§13.5).
pub(crate) const CACHE_WRITE_WAIT: Duration = Duration::from_millis(5);
```

(Only `const` becomes `pub(crate) const`; the doc lines are unchanged.)

Replace `crates/tagteam-engine/src/tokens/evidence.rs` with:

```rust
//! Session evidence (spec §6): sightings from the scan, from `run`'s poll of its child's record
//! and from the statusline, and `run`'s launches. Each source writes its own rows, and every
//! write is idempotent.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::{
    ProfileMarker, Provider, Read, RecordEntry, parse_session_record, profile_path,
    read_session_records, record_is_live,
};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::{SightingRow, SightingSource};
use crate::views::CACHE_WRITE_WAIT;

/// A sighting is extended at most once a minute (spec §6.4), and the statusline does nothing
/// while its session's latest sighting in its home is younger than that (spec §6.3).
const SIGHTING_GAP_MS: i64 = 60_000;
/// The most ancestors the statusline walks to find its Claude Code process (spec §6.3).
const MAX_ANCESTORS: usize = 8;

/// One launch `run` recorded (spec §6.1), as `open_launch` returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchId {
    pub provider: ProviderId,
    pub account: AccountId,
    pub started_at_ms: i64,
}

/// The poller's own state, owned by `run` (spec §6.1): the child's record's last mtime, and the
/// sighting it wrote last, as `(session id, first_at)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchPoll {
    pub last_mtime_ns: Option<i128>,
    pub last_sighting: Option<(String, i64)>,
}

impl Engine {
    /// Spec §6: scans every home's session records, records what it finds, and closes the
    /// launches whose profile is no longer in use. Returns how many rows it added, extended or
    /// closed.
    pub fn sight_sessions(&self, provider: &ProviderId) -> Result<u64, EngineError> {
        let p = self.provider(provider)?;
        if !p.capabilities().transcripts {
            return Err(EngineError::InvalidInput(format!(
                "{} has no transcripts",
                p.display_name()
            )));
        }
        self.scan_sessions(p.as_ref())
    }

    /// `sight_sessions`, and a refresh's step 1 (spec §5.2): the scan of spec §6.2, then every
    /// open launch whose profile is no longer in use is closed, now (spec §6.1). A late close
    /// over-covers, which can only leave a response unattributed, never misplace it.
    pub(crate) fn scan_sessions(&self, p: &dyn Provider) -> Result<u64, EngineError> {
        let provider = p.id();
        let store = self.store()?;
        let now = self.now_ms();
        let mut changed = 0;
        for (home, dir) in self.homes(p) {
            let Read::Present(entries) = read_session_records(&dir) else {
                continue;
            };
            for entry in entries {
                // An unreadable or partly written record, or one whose process is gone, is
                // skipped (spec §6.2).
                let RecordEntry::Record(r) = entry else {
                    continue;
                };
                let (Some(session_id), Some(first_at)) = (r.session_id.clone(), r.started_at_ms)
                else {
                    continue;
                };
                if !record_is_live(self.process.as_ref(), &r, p.launch_command()) {
                    continue;
                }
                let row = SightingRow {
                    session_id,
                    home: home.clone(),
                    first_at,
                    last_at: now,
                    source: SightingSource::Scan,
                    recorded_at: now,
                };
                if store.record_sighting(&provider, &row, SIGHTING_GAP_MS)? {
                    changed += 1;
                }
            }
        }
        for launch in store.launches(&provider)? {
            if launch.ended_at.is_some() {
                continue;
            }
            let account = AccountId::from_string(launch.account_id.as_str());
            if !self
                .session_state_at(p, &profile_path(&self.env, &account))
                .owned()
            {
                store.close_launch(&provider, &launch.account_id, launch.started_at, now, now)?;
                changed += 1;
            }
        }
        tracing::debug!(
            provider = self.registered_id(&provider),
            changed,
            "scanned the session records"
        );
        Ok(changed)
    }

    /// The homes the scan reads (spec §6.2): the default home (`""`), then each of `p`'s
    /// profiles under the data directory's `sessions/`, by its account. A profile whose marker
    /// names another provider, or cannot be read, is not `p`'s to place.
    fn homes(&self, p: &dyn Provider) -> Vec<(String, PathBuf)> {
        let mut homes = Vec::new();
        if let Some(dir) = p.default_session_records_dir(&self.env) {
            homes.push((String::new(), dir));
        }
        if !p.capabilities().sessions {
            return homes;
        }
        let Ok(listing) = fs::read_dir(self.env.data_dir().join("sessions")) else {
            return homes;
        };
        for entry in listing.flatten() {
            let Some(account) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let profile = entry.path();
            let ours = entry.file_type().is_ok_and(|t| t.is_dir())
                && matches!(ProfileMarker::read(&profile), Read::Present(m) if m.provider == p.id());
            if ours {
                homes.push((account, p.session_records_dir(&profile)));
            }
        }
        homes.sort();
        homes
    }

    /// Spec §6.1: the launch `run` records before it spawns its child, open from now. `run`
    /// calls it while it holds its launch reservation, so no scan closes it before the child
    /// runs.
    pub fn open_launch(
        &self,
        provider: &ProviderId,
        account: &AccountId,
    ) -> Result<LaunchId, EngineError> {
        let now = self.now_ms();
        self.store()?
            .open_launch(provider, account.as_str(), now, now)?;
        Ok(LaunchId {
            provider: provider.clone(),
            account: account.clone(),
            started_at_ms: now,
        })
    }

    /// The child's pid, once it is spawned (spec §6.1).
    pub fn attach_launch_pid(&self, id: &LaunchId, pid: u32) -> Result<(), EngineError> {
        Ok(self.store()?.attach_launch_pid(
            &id.provider,
            id.account.as_str(),
            id.started_at_ms,
            pid,
            self.now_ms(),
        )?)
    }

    /// Spec §6.1's poll, every 2 seconds while the child runs: its record,
    /// `<records dir>/<pid>.json`, is read again only when its mtime moved, and the sighting of
    /// the session it names, keyed by its `startedAt`, is extended to now. The store writes at
    /// most once a minute (spec §6.4). A record caught part-way through its write, or one that is
    /// not this child's, is read again at the next poll.
    pub fn poll_launch(
        &self,
        id: &LaunchId,
        profile: &Path,
        pid: u32,
        state: &mut LaunchPoll,
    ) -> Result<(), EngineError> {
        let p = self.provider(&id.provider)?;
        let record = p.session_records_dir(profile).join(format!("{pid}.json"));
        match fs::metadata(&record) {
            Ok(meta) => {
                let mtime =
                    i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec());
                if state.last_mtime_ns != Some(mtime) {
                    // Only this child's own execution: a record of its pid, started no earlier
                    // than the launch, whose process still runs. One a crashed execution left
                    // behind under a reused pid is skipped, and not remembered.
                    let seen = fs::read(&record)
                        .ok()
                        .and_then(|bytes| parse_session_record(&bytes).ok())
                        .filter(|r| {
                            r.pid == pid
                                && r.started_at_ms.is_some_and(|at| at >= id.started_at_ms)
                                && record_is_live(self.process.as_ref(), r, p.launch_command())
                        })
                        .and_then(|r| r.session_id.zip(r.started_at_ms));
                    if let Some(seen) = seen {
                        state.last_mtime_ns = Some(mtime);
                        state.last_sighting = Some(seen);
                    }
                }
            }
            // Not written yet, or already removed at the child's exit.
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let Some((session_id, first_at)) = &state.last_sighting else {
            return Ok(());
        };
        let now = self.now_ms();
        let row = SightingRow {
            session_id: session_id.clone(),
            home: id.account.as_str().to_owned(),
            first_at: *first_at,
            last_at: now,
            source: SightingSource::Run,
            recorded_at: now,
        };
        self.store()?
            .record_sighting(&id.provider, &row, SIGHTING_GAP_MS)?;
        Ok(())
    }

    /// Spec §6.1, at the child's exit: the sighting it saw last ends at `exited_at_ms`, when the
    /// child exited, and the launch closes now unless the profile is still in use
    /// (`session_state_at`). `run` calls it after `finish_run`, once its own launch reservation is
    /// released, so the profile reads idle when nothing else runs there. Only the launch may
    /// close late, which over-covers; the sighting never outlasts the child, so exit handling that
    /// waits on locks cannot stretch it over a resume elsewhere. A background session the child
    /// started, or another launcher's reservation, keeps the launch open, and the next scan that
    /// finds the profile idle closes it.
    pub fn close_launch(
        &self,
        id: &LaunchId,
        profile: &Path,
        state: &LaunchPoll,
        exited_at_ms: i64,
    ) -> Result<(), EngineError> {
        let p = self.provider(&id.provider)?;
        let store = self.store()?;
        let now = self.now_ms();
        if let Some((session_id, first_at)) = &state.last_sighting {
            store.set_sighting_last_at(
                &id.provider,
                session_id,
                id.account.as_str(),
                *first_at,
                exited_at_ms,
                now,
            )?;
        }
        if !self.session_state_at(p.as_ref(), profile).owned() {
            store.close_launch(
                &id.provider,
                id.account.as_str(),
                id.started_at_ms,
                now,
                now,
            )?;
        }
        Ok(())
    }

    /// Spec §6.3: the statusline's sighting of `session_id` in its home, which is `profile`, the
    /// run shell's profile, or the default home. Nothing is done while the session's latest
    /// sighting there is under a minute old. Otherwise the Claude Code process the statusline
    /// runs under is found among its ancestors, by its record in the home, and that
    /// execution's sighting is written. The write waits at most `CACHE_WRITE_WAIT`, and a busy
    /// store skips it. The store is never created.
    pub fn statusline_sighting(
        &self,
        provider: &ProviderId,
        session_id: &str,
        profile: Option<&Path>,
    ) -> Result<(), EngineError> {
        let p = self.provider(provider)?;
        if !p.capabilities().transcripts {
            return Ok(());
        }
        let (home, records) = match profile {
            Some(dir) => match ProfileMarker::read(dir) {
                Read::Present(m) if &m.provider == provider => {
                    (m.account_id.as_str().to_owned(), p.session_records_dir(dir))
                }
                _ => return Ok(()),
            },
            None => match p.default_session_records_dir(&self.env) {
                Some(dir) => (String::new(), dir),
                None => return Ok(()),
            },
        };
        let Some(store) = self.existing_store()? else {
            return Ok(());
        };
        let now = self.now_ms();
        let fresh = store
            .latest_sighting(provider, session_id, &home)?
            .is_some_and(|s| s.last_at > now.saturating_sub(SIGHTING_GAP_MS));
        if fresh {
            return Ok(());
        }
        let Some(first_at) = self.execution_started_at(p.as_ref(), &records) else {
            return Ok(());
        };
        let row = SightingRow {
            session_id: session_id.to_owned(),
            home,
            first_at,
            last_at: now,
            source: SightingSource::Statusline,
            recorded_at: now,
        };
        // Evidence, not the line: a failed write leaves the session to the next run. SQLite's
        // result code only, never the error's text (§14.2).
        if let Err(e) =
            store.record_sighting_within(provider, &row, SIGHTING_GAP_MS, CACHE_WRITE_WAIT)
        {
            tracing::debug!(
                code = e.sqlite_code().map(tracing::field::debug),
                "the statusline's session sighting was not written"
            );
        }
        Ok(())
    }

    /// The `startedAt` of the first of this process's ancestors, at most `MAX_ANCESTORS` up,
    /// with a live record of its own in `records` (spec §6.3): the execution the statusline
    /// runs under.
    fn execution_started_at(&self, p: &dyn Provider, records: &Path) -> Option<i64> {
        let mut pid = std::process::id();
        for _ in 0..MAX_ANCESTORS {
            pid = self.process.parent_pid(pid).filter(|&parent| parent > 1)?;
            let Ok(bytes) = fs::read(records.join(format!("{pid}.json"))) else {
                continue;
            };
            let Ok(r) = parse_session_record(&bytes) else {
                continue;
            };
            if r.pid == pid && record_is_live(self.process.as_ref(), &r, p.launch_command()) {
                return r.started_at_ms;
            }
        }
        None
    }
}
```

- [ ] **Step 4: Run them and see them pass**

```
cargo test -p tagteam-engine --locked --test tokens_evidence
```

Expected: `test result: ok. 12 passed; 0 failed`.

- [ ] **Step 5: Lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy -p tagteam-engine --all-targets --locked -- -D warnings
cargo clippy -p tagteam-engine --all-targets --locked --features test-hooks -- -D warnings
```

Expected: no output from the format check, and clippy finishes with no warnings.

- [ ] **Step 6: Commit**

```
git add crates/tagteam-engine/src/tokens/evidence.rs crates/tagteam-engine/src/views.rs crates/tagteam-engine/tests/tokens_evidence.rs
git commit -m "Record session evidence from scans, run launches and the statusline"
```

#### Notes for the controller

1. **`Store::record_sighting_within` is now in Task 12**, added at this task's request. It works as `put_live_identity_cache_within` does: the connection stays locked across one write through the shared `write_sighting`, under the short busy timeout, which is then restored. The statusline's write therefore gives up after `CACHE_WRITE_WAIT` on a busy store (spec §6.3), and no other thread of the process writes with the short wait.
2. `CACHE_WRITE_WAIT` in `views.rs` becomes `pub(crate)`. Nothing else in `views.rs` changes, and no other task in batch E touches it.
3. **Task 21's ordering.**
   - `close_launch` judges the profile with `session_state_at(..).owned()`, as the scan does. Any held launch reservation counts as in use, `run`'s own included. So Task 21 calls it after `finish_run`, once `run`'s reservation is released; the profile then reads idle when nothing else runs there.
   - If `finish_run` could not take its locks and left the reservation held until the process exits, the launch stays open and the next scan closes it. A late close over-covers, which can only leave a response unattributed (spec §6.1).
   - `open_launch` must be called while `run` holds its reservation. It does: `run_session` receives `launched.reservation` before `spawn_session`. Otherwise a scan between the two would close the launch at once.
   - **`close_launch` takes `exited_at_ms` (plan review; the signature changed, and so did the contract line in "Shared interfaces").**
     - `run` reads `engine.now_ms()` right after its wait returns and passes it. The last sighting ends then, and the launch's `ended_at` is the close's own now, set only when the profile is idle.
     - Otherwise exit handling that waits on locks would stretch a positive sighting past the child's exit and over a resume elsewhere. Spec §6.1 allows a late launch close, which only over-covers, but not a late sighting.
     - Pinned by `close_stamps_the_sighting_at_the_exit_not_at_the_close`. Task 21 adapts its call.
4. **Every poll extends, not only when the mtime moves.**
   - The spec says `run` reads the record when its mtime changed. Re-reading is gated on the mtime here, but the sighting of the session it saw last is extended on every poll; the store writes at most once a minute (§6.4).
   - Without that, a session whose record never changes would be sighted only from its start to the first poll until the exit write. During a long session, a live report would leave its responses unattributed: the default home plus the open launch makes two candidates.
   - **The poll accepts only the child's own execution (plan review).** A record counts when all three hold: its `pid` is the child's, its `startedAt` is not earlier than the launch's own `started_at` (the child writes its record after the spawn), and `record_is_live` holds. Anything else is skipped and not cached in `LaunchPoll`, so a crashed execution's leftover `<pid>.json` under a reused pid never extends an old sighting to now (`a_stale_record_with_a_reused_pid_is_ignored`).
   - The scan never attributes a record to a launch: it sights records by home, and closes launches by the profile's session state. So it needs no such check.
5. **The scan places only the provider's own profiles.** It reads only profiles whose marker names the provider. One with an unreadable marker is skipped, which leaves its responses unattributed rather than guessing.
6. **What the statusline walk requires.** It needs the ancestor's record to name that pid and be live (`record_is_live`), so a stale record of a recycled pid is never taken for the execution. It does not compare the record's `sessionId` with `CLAUDE_CODE_SESSION_ID`, since whether the record follows `/clear` is spec §17 T1's open item.
7. Without `transcripts`, `statusline_sighting` returns `Ok(())` silently, because the statusline must never fail on evidence. `sight_sessions` refuses the same case with `invalid-input`. That refusal is tested only through `refresh_tokens` (Task 15), since FakeAgent's `without_transcripts` is the only provider without the capability.
8. **What I verified, and what I did not.** I compiled this task's `evidence.rs` verbatim, except its provider import, in a scratch crate. The crate held the real Task 14 `tokens/` files over the real `tagteam-core` and `tagteam-provider`, with stand-ins whose signatures follow the contract:
   - Task 12's store methods, `record_sighting_within` included;
   - Task 7's `SessionRecord.session_id` and `ProcessProbe::parent_pid`;
   - `session_state_at` and `CACHE_WRITE_WAIT`.

   Results: clippy `-D warnings` was clean and rustfmt was clean; the text above is the formatted code. Task 15's refresh tests also passed with this `scan_sessions` wired in. After the plan review's changes (the poll's three checks, `close_launch`'s `exited_at_ms`), the same check is still clean. I did **not** run `tests/tokens_evidence.rs`, which needs the real engine with Tasks 7 and 12; rustfmt parsed and formatted it. No test here asserts elapsed time (plan review): the busy-store test asserts only that the call succeeds and writes nothing.

---

### Task 17: Purge takes the tokens lock

**Depends on:** 14 (and through it 12: `delete_provider_rows` covers the token tables, and the tests plant rows with Task 12's store API)
**Files:**
- Modify: `crates/tagteam-engine/src/tokens/mod.rs` (`lock_until_free`, the wait that has no deadline, and its test)
- Modify: `crates/tagteam-engine/src/purge.rs` (step 9 deletes under the affected providers' tokens locks, with hook points `purge-tokens-lock` and `purge-tokens-locked` around them; an interrupted wait ends the purge)
- Modify: `crates/tagteam-engine/src/store/tokens.rs` (`Store::token_providers`: every provider with rows in the token and evidence tables, for the full purge)
- Modify: `crates/tagteam-engine/tests/purge.rs` (helpers and four tests, three of them under feature `test-hooks`)
- Test: `crates/tagteam-engine/tests/purge.rs`, and the unit test in `crates/tagteam-engine/src/tokens/mod.rs`

**Interfaces:**
- Consumes:
  - Task 14: `pub fn tokens_lock_path(env: &Env, provider: &ProviderId) -> PathBuf`.
  - Task 12: `Store::delete_provider_rows`, which also deletes `token_files`, `token_sessions` (cascading `token_usage`), `session_sightings`, `run_launches`, `active_history` and `token_state` for the provider. Also `Store::empty_all` (existing; it empties every table).
  - Task 12 (tests): `Store::{commit_token_batch, record_sighting, open_launch, token_epoch}`, `UsageRow`, `TokenFileRow`, `SightingRow`, `SightingSource`, `usage_key`.
  - Task 2 (tests): `tagteam_core::tokens::TokenCounts`.
  - Existing: `FlockGuard::lock`, `LockError`, `Cancel`, `EngineError::signal`, `crate::hooks::point`, and `Engine::{on_point, also_on_point, cancel}` (feature `test-hooks`).
- Produces:
  ```rust
  // crate::tokens (tokens/mod.rs), crate-internal, also for Task 15's `Wait::Wait`
  pub(crate) fn lock_until_free(path: &Path, cancel: &Cancel) -> Result<FlockGuard, LockError>;
  // crate::store (store/tokens.rs)
  impl Store { pub fn token_providers(&self) -> Result<Vec<ProviderId>, StoreError>; } // distinct, sorted
  ```

**On the wait.** A purge waits for the tokens lock without a deadline, as `Wait::Wait` does, and ends only on the cancel token (spec §5.1: "takes it too, waiting"). `FlockGuard::lock` alone cannot do that: it computes `Instant::now() + timeout`, which panics for `Duration::MAX`, and any finite value is a cap the spec does not have. `lock_until_free` loops over one-hour rounds of `FlockGuard::lock`, which polls every 100 ms and checks the token before each attempt. A refresh holds the lock for one slice (about 1 s for the daemon) or one refresh (seconds, a cold one included, spec §1.1). A holder that never lets go is stopped or stuck; then Ctrl-C ends the purge with 130, and running it again finishes it, as for an interruption at step 7. This adds no API to `tagteam-provider`.

**On lock order.** The tokens locks are taken at step 9, the last step that deletes rows. At that point the purge holds:
- the engine locks (step 3, only ever tried);
- the mutation lock (steps 4 and 5).

Every account lock of step 7 is released by then: `purge_account` drops its `AccountLock` when it returns. Nothing is taken while a tokens lock is held; only the store's transaction runs, and store transactions are outside the order (spec §5.1). A full purge empties every table in one transaction, so it must hold every affected provider's tokens lock at once (spec §13: "holds each affected provider's tokens lock while it deletes"). It takes them in sorted order. No wait can close a cycle:
- the purge is the only holder of more than one tokens lock;
- two purges never overlap, since both need the mutation lock;
- a refresh holds one tokens lock and waits on nothing while it does.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/src/tokens/mod.rs`, add this test to `mod tests`, after `the_tokens_lock_is_in_locks_and_a_stored_string_never_escapes_it`:

```rust
    #[test]
    fn a_wait_for_a_tokens_lock_never_takes_a_held_lock() {
        // Held elsewhere, the lock is never taken: only the cancel token ends the wait. Free, it
        // is taken at once, and held.
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let path = tokens_lock_path(&env, &ProviderId::new("claude-code"));
        let held = FlockGuard::try_lock(&path).unwrap().unwrap();
        let cancel = Cancel::new();
        let waited = std::thread::scope(|s| {
            let waiter = s.spawn(|| lock_until_free(&path, &cancel).map(drop));
            cancel.request(2);
            waiter.join().unwrap()
        });
        assert!(
            matches!(waited, Err(LockError::Interrupted { signal: 2, .. })),
            "{waited:?}"
        );
        drop(held);
        let taken = lock_until_free(&path, &Cancel::new()).unwrap();
        assert!(FlockGuard::try_lock(&path).unwrap().is_none(), "and held");
        drop(taken);
    }
```

In `crates/tagteam-engine/tests/purge.rs`, replace the `use` block at the top with:

```rust
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{FakeFx, Fx, add, crashed_switch, credential, journal, vault_fp};
use serde_json::json;
use tagteam_cc::live::Platform;
use tagteam_core::tokens::TokenCounts;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::EngineError;
use tagteam_engine::purge::{KEYCHAIN_LEFTOVERS, PurgeAccount, PurgePlan};
use tagteam_engine::store::{
    DisplacedRow, JournalRow, SightingRow, SightingSource, Store, TokenFileRow, UsageRow, usage_key,
};
use tagteam_engine::tokens::tokens_lock_path;
use tagteam_engine::vault::SERVICE;
use tagteam_provider::liveness::FakeProcess;
use tagteam_provider::profile::ProfileMarker;
use tagteam_provider::{
    FlockGuard, Keychain, KeychainError, LockState, ProcessStamp, Provider, Read,
};
```

Append at the end of `crates/tagteam-engine/tests/purge.rs`:

```rust
/// A refresh's rows and the evidence's for `provider`, as a refresh, a scan and `run` leave
/// them (spec §4).
fn plant_token_rows(fx: &Fx, provider: &ProviderId) {
    let store = fx.engine.store().unwrap();
    let now = 1_790_000_000_000;
    let usage = UsageRow {
        key: usage_key(provider, "m1", "r1"),
        session_id: "s-1".into(),
        project: Some("/w".into()),
        at_ms: now,
        model: "claude-opus-5-5".into(),
        sidechain: false,
        counts: TokenCounts {
            input: 1,
            output: 2,
            ..TokenCounts::default()
        },
        speed: None,
        finalized: true,
    };
    let file = TokenFileRow {
        path: format!("/t/{provider}.jsonl"),
        dev: 1,
        ino: 2,
        size: 10,
        mtime_ns: 3,
        offset: 10,
        head_hash: [0; 8],
        tail_hash: [0; 8],
    };
    store
        .commit_token_batch(provider, &[usage], &[file], &[], now)
        .unwrap();
    let seen = SightingRow {
        session_id: "s-1".into(),
        home: String::new(),
        first_at: now - 1_000,
        last_at: now,
        source: SightingSource::Scan,
        recorded_at: now,
    };
    store.record_sighting(provider, &seen, 60_000).unwrap();
    store
        .open_launch(provider, "an-account", now - 1_000, now)
        .unwrap();
}

/// `provider`'s responses in the index, read behind the store's back.
fn usage_rows(fx: &Fx, provider: &str) -> i64 {
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM token_usage u JOIN token_sessions s ON s.id = u.session \
             WHERE s.provider = ?1",
            [provider],
            |r| r.get(0),
        )
        .unwrap()
}

/// Every index and evidence row of `provider`: its usage, and its rows in the six
/// provider-keyed tables (spec §13).
fn token_rows(fx: &Fx, provider: &str) -> i64 {
    let tables = [
        "token_files",
        "token_sessions",
        "session_sightings",
        "run_launches",
        "active_history",
        "token_state",
    ];
    usage_rows(fx, provider)
        + tables
            .iter()
            .map(|table| count(fx, table, provider))
            .sum::<i64>()
}

/// Runs a purge while a refresh of `provider` holds its tokens lock (spec §5.1), then again
/// once that refresh is done; `plan` makes each run's plan.
///
/// The first run reaches the point before its tokens-lock wait, and the test sets the cancel
/// token there. A purge that waits for the lock never reaches the point after the wait, and
/// deletes nothing. One that took no lock would reach that point at once, since nothing between
/// the two points can be interrupted.
///
/// Returns the second run's report, after asserting that the first run was interrupted and the
/// second held the lock past that point.
#[cfg(feature = "test-hooks")]
fn purge_under_a_refresh(
    fx: &Fx,
    provider: &ProviderId,
    plan: impl Fn() -> PurgePlan,
) -> tagteam_engine::purge::PurgeReport {
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::time::Duration;

    let path = tokens_lock_path(&fx.env, provider);
    let refresh = FlockGuard::try_lock(&path).unwrap().unwrap();
    let (at_lock, reached) = mpsc::channel();
    fx.engine.on_point(
        "purge-tokens-lock",
        Box::new(move || at_lock.send(()).unwrap()),
    );
    let passed = Arc::new(AtomicBool::new(false));
    let held_there = Arc::new(AtomicBool::new(false));
    let (passed_cb, held_cb, path_cb) = (passed.clone(), held_there.clone(), path.clone());
    fx.engine.also_on_point(
        "purge-tokens-locked",
        Box::new(move || {
            passed_cb.store(true, Ordering::SeqCst);
            let held = FlockGuard::try_lock(&path_cb).unwrap().is_none();
            held_cb.store(held, Ordering::SeqCst);
        }),
    );

    let first_plan = plan();
    let first = std::thread::scope(|s| {
        let purge = s.spawn(|| fx.engine.purge(&first_plan));
        // A guard against a hang, not a timing: the purge reaches this point at once.
        reached
            .recv_timeout(Duration::from_secs(30))
            .expect("the purge reaches its tokens lock");
        fx.engine.cancel().request(2);
        purge.join().unwrap()
    });
    assert_eq!(
        first.unwrap_err().signal(),
        Some(2),
        "only the cancel token ends the wait"
    );
    assert!(
        !passed.load(Ordering::SeqCst),
        "the purge passed the tokens lock while a refresh held it"
    );
    assert!(
        token_rows(fx, provider.as_str()) > 0,
        "nothing was deleted under the refresh"
    );

    fx.engine.cancel().take();
    drop(refresh);
    let report = fx.engine.purge(&plan()).unwrap();
    assert!(passed.load(Ordering::SeqCst));
    assert!(
        held_there.load(Ordering::SeqCst),
        "the purge held the tokens lock while it deleted"
    );
    report
}

#[test]
#[cfg(feature = "test-hooks")]
fn purge_waits_for_a_running_refresh() {
    // Spec §5.1: a purge never falls between two batches of a refresh. Once its accounts are
    // gone, it waits for the provider's tokens lock, under its mutation lock and with no account
    // lock held, before it deletes the provider's rows.
    let fx = Fx::new();
    let cc = fx.provider();
    fx.add("a@x.co", "rt-a");
    plant_token_rows(&fx, &cc);

    let report = purge_under_a_refresh(&fx, &cc, || fx.engine.purge_plan(Some(&cc)).unwrap());
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(token_rows(&fx, "claude-code"), 0);
}

#[test]
#[cfg(feature = "test-hooks")]
fn a_full_purge_waits_for_a_running_refresh_too() {
    // Spec §13: a full purge empties the store under every affected provider's tokens lock.
    let fx = Fx::new();
    let cc = fx.provider();
    fx.add("a@x.co", "rt-a");
    plant_token_rows(&fx, &cc);

    let report = purge_under_a_refresh(&fx, &cc, || full_plan(&fx));
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert!(report.store_emptied);
    assert_eq!(token_rows(&fx, "claude-code"), 0);
}

#[test]
#[cfg(feature = "test-hooks")]
fn a_full_purge_locks_every_provider_with_token_rows() {
    // Spec §13: a full purge empties every table, so it holds the tokens lock of every provider
    // with rows there. That includes one with no account, and one only another build registers.
    use std::sync::Mutex;

    let fx = Fx::new();
    let cc = fx.provider();
    let ghost = ProviderId::new("ghost");
    fx.add("a@x.co", "rt-a");
    plant_token_rows(&fx, &cc);
    plant_token_rows(&fx, &ghost);
    let held = Arc::new(Mutex::new(Vec::new()));
    let (held_cb, env) = (held.clone(), fx.env.clone());
    fx.engine.on_point(
        "purge-tokens-locked",
        Box::new(move || {
            let mut held = held_cb.lock().unwrap();
            for p in ["claude-code", "ghost"] {
                let path = tokens_lock_path(&env, &ProviderId::new(p));
                held.push((p, FlockGuard::try_lock(&path).unwrap().is_none()));
            }
        }),
    );

    let report = fx.engine.purge(&full_plan(&fx)).unwrap();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(
        *held.lock().unwrap(),
        [("claude-code", true), ("ghost", true)]
    );
    assert_eq!(token_rows(&fx, "ghost"), 0);
}

#[test]
fn purge_provider_deletes_the_token_rows() {
    // Spec §13: `purge --provider` deletes that provider's rows from the six provider-keyed
    // tables, and its usage with its sessions. Another provider's stay, and so does the epoch.
    let ffx = FakeFx::new();
    let fx = &ffx.fx;
    let (cc, fake) = (fx.provider(), ffx.fake_provider());
    fx.add("a@x.co", "rt-a");
    ffx.fake_add("hank", "tok-h", "renew-h");
    plant_token_rows(fx, &cc);
    plant_token_rows(fx, &fake);
    let store = fx.engine.store().unwrap();
    let epoch = store.token_epoch().unwrap();
    assert!(epoch.is_some());

    let plan = ffx.engine.purge_plan(Some(&cc)).unwrap();
    let report = ffx.engine.purge(&plan).unwrap();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(token_rows(fx, "claude-code"), 0);
    for table in [
        "token_files",
        "token_sessions",
        "session_sightings",
        "run_launches",
        "token_state",
    ] {
        assert_eq!(count(fx, table, "fake-agent"), 1, "{table}");
    }
    assert_eq!(usage_rows(fx, "fake-agent"), 1);
    assert_eq!(store.token_epoch().unwrap(), epoch, "the epoch stays");
    assert!(
        tokens_lock_path(&fx.env, &cc).exists(),
        "the purge took the provider's tokens lock"
    );
    assert!(
        !tokens_lock_path(&fx.env, &fake).exists(),
        "and no other provider's"
    );
}
```

- [ ] **Step 2: Run them and see them fail**

```
cargo test -p tagteam-engine --locked --lib tokens::tests
cargo test -p tagteam-engine --locked --features test-hooks --test purge -- purge_waits_for_a_running_refresh a_full_purge_waits_for_a_running_refresh_too a_full_purge_locks_every_provider_with_token_rows purge_provider_deletes_the_token_rows
```

Expected:
- The lib run does not compile: `cannot find function `lock_until_free` in this scope`, and `cannot find type `Cancel`, `FlockGuard` and `LockError`.
- The purge tests compile (all they need is in place) and all four fail:
  - `purge_waits_for_a_running_refresh` and `a_full_purge_waits_for_a_running_refresh_too` panic with `the purge reaches its tokens lock`. There is no `purge-tokens-lock` point yet, so they give up after the 30 s hang guard.
  - `a_full_purge_locks_every_provider_with_token_rows` fails its `assert_eq!` on the locks seen held: no `purge-tokens-locked` point yet, so it sees none.
  - `purge_provider_deletes_the_token_rows` fails at `the purge took the provider's tokens lock`.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/tokens/mod.rs`, replace the imports

```rust
use std::path::PathBuf;

use tagteam_core::{Fingerprint, ProviderId};
use tagteam_provider::Env;
```

with

```rust
use std::path::{Path, PathBuf};
use std::time::Duration;

use tagteam_core::{Fingerprint, ProviderId};
use tagteam_provider::{Cancel, Env, FlockGuard, LockError};
```

and insert after `tokens_lock_path` (before `#[cfg(test)]`):

```rust
/// One round of a wait for a tokens lock; the rounds repeat until it is free.
const WAIT_ROUND: Duration = Duration::from_secs(3_600);

/// Waits for a tokens lock until it is free, ended only by `cancel` (spec §5.1: a refresh with
/// `Wait::Wait`, and `purge`, wait cancellably). `FlockGuard::lock` polls every 100 ms and checks
/// `cancel` before each attempt; its timeout bounds one round, and the rounds repeat, so the wait
/// has no deadline of its own.
pub(crate) fn lock_until_free(path: &Path, cancel: &Cancel) -> Result<FlockGuard, LockError> {
    loop {
        match FlockGuard::lock(path, WAIT_ROUND, cancel) {
            Err(LockError::Timeout(_)) => {}
            other => return other,
        }
    }
}
```

In `crates/tagteam-engine/src/purge.rs`:

1. Add the import between `use crate::store::{AccountRow, Store};` and `use crate::vault::Leftovers;` (`hooks` is already imported):

```rust
use crate::store::{AccountRow, Store};
use crate::tokens::{lock_until_free, tokens_lock_path};
use crate::vault::Leftovers;
```

2. In `purge`, replace step 9

```rust
        // 9. An account step 7 could not delete keeps its row, so the store's rows stay too.
        match &plan.provider {
            Some(p) => self.purge_provider_rest(p, unfinished, &mut report),
            None => self.purge_everything_else(plan.keychain_orphans, unfinished, &mut report),
        }
        Ok(report)
```

with

```rust
        // 9. An account step 7 could not delete keeps its row, so the store's rows stay too.
        // Every account lock is released by now; the tokens locks taken here are leaves.
        match &plan.provider {
            Some(p) => self.purge_provider_rest(p, unfinished, &mut report)?,
            None => self.purge_everything_else(
                &providers,
                plan.keychain_orphans,
                unfinished,
                &mut report,
            )?,
        }
        Ok(report)
```

3. After `hold_engine_locks` (before the doc comment of `purge_account`), add:

```rust
    /// Spec §5.1, §13: the tokens lock of each of `providers`, waited for (cancellably), so a
    /// purge never falls between two batches of a refresh. They are taken last, under the
    /// engine and mutation locks once every account lock is released, in provider order. A
    /// refresh holds one tokens lock and waits on nothing while it does, and two purges never
    /// overlap, so these waits close no cycle; nothing is taken while they are held. The test
    /// hooks `purge-tokens-lock` and `purge-tokens-locked` lie immediately before and after the
    /// wait.
    fn hold_tokens_locks(&self, providers: &[ProviderId]) -> Result<Vec<FlockGuard>, EngineError> {
        let mut sorted: Vec<&ProviderId> = providers.iter().collect();
        sorted.sort();
        sorted.dedup();
        hooks::point(self, "purge-tokens-lock")?;
        let locks = sorted
            .into_iter()
            .map(|p| {
                Ok(lock_until_free(
                    &tokens_lock_path(&self.env, p),
                    &self.env.cancel,
                )?)
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        hooks::point(self, "purge-tokens-locked")?;
        Ok(locks)
    }
```

4. Replace `purge_provider_rest`'s doc comment and signature

```rust
    /// §10.5 with `--provider P`, once its accounts are gone: P's displaced entries, each file
    /// before its row (§6.3), then its other rows. Its `usage_requests` rows stay. With
    /// `unfinished` accounts left, P's rows stay with them, and a failure says so.
    fn purge_provider_rest(
        &self,
        provider: &ProviderId,
        unfinished: usize,
        report: &mut PurgeReport,
    ) {
```

with

```rust
    /// §10.5 with `--provider P`, once its accounts are gone: P's displaced entries, each file
    /// before its row (§6.3), then its other rows, under P's tokens lock (spec §5.1). Its
    /// `usage_requests` rows stay. With `unfinished` accounts left, P's rows stay with them, and
    /// a failure says so. Only an interrupted wait for the tokens lock ends it with an error.
    fn purge_provider_rest(
        &self,
        provider: &ProviderId,
        unfinished: usize,
        report: &mut PurgeReport,
    ) -> Result<(), EngineError> {
```

and, further down in the same function, replace

```rust
        } else {
            let rows = self.existing_store().and_then(|store| match store {
                Some(s) => Ok(s.delete_provider_rows(provider)?),
                None => Ok(()),
            });
            if let Err(e) = rows {
                report.failures.push((what, e.to_string()));
            }
        }
        tracing::info!(
            provider = self.registered_id(provider),
            accounts = report.accounts.len(),
            rescues = report.rescues,
            displaced = report.displaced,
            failures = report.failures.len(),
            "purge finished"
        );
    }
```

with

```rust
        } else {
            let rows = self.existing_store().and_then(|store| match store {
                Some(s) => {
                    let _tokens = self.hold_tokens_locks(std::slice::from_ref(provider))?;
                    Ok(s.delete_provider_rows(provider)?)
                }
                None => Ok(()),
            });
            match rows {
                Ok(()) => {}
                Err(e) if e.signal().is_some() => return Err(e),
                Err(e) => report.failures.push((what, e.to_string())),
            }
        }
        tracing::info!(
            provider = self.registered_id(provider),
            accounts = report.accounts.len(),
            rescues = report.rescues,
            displaced = report.displaced,
            failures = report.failures.len(),
            "purge finished"
        );
        Ok(())
    }
```

5. Replace `purge_everything_else`'s doc comment and signature

```rust
    /// are no account's, and go either way. The log goes last, and nothing is logged after it:
    /// a line would create it again (Decision 9).
    fn purge_everything_else(
        &self,
        keychain_orphans: bool,
        unfinished: usize,
        report: &mut PurgeReport,
    ) {
```

with

```rust
    /// are no account's, and go either way. The store is emptied under the tokens lock of every
    /// affected provider and of every provider with rows in the token tables (spec §5.1, §13),
    /// one with no account, or one only another build registers, included; only an interrupted
    /// wait for one ends it with an error. The log goes last, and nothing is logged after it: a
    /// line would create it again (Decision 9).
    fn purge_everything_else(
        &self,
        providers: &[ProviderId],
        keychain_orphans: bool,
        unfinished: usize,
        report: &mut PurgeReport,
    ) -> Result<(), EngineError> {
```

and, at its end, replace

```rust
        } else {
            match self.existing_store() {
                Ok(None) => report.store_emptied = true,
                Ok(Some(store)) => match store.empty_all() {
                    Ok(()) => report.store_emptied = true,
                    Err(e) => report.failures.push(("the store".into(), e.to_string())),
                },
                Err(e) => report.failures.push(("the store".into(), e.to_string())),
            }
        }
        self.delete_log(report);
    }
```

with

```rust
        } else {
            match self.existing_store() {
                Ok(None) => report.store_emptied = true,
                Ok(Some(store)) => match store
                    .token_providers()
                    .map_err(EngineError::from)
                    .and_then(|stored| self.hold_tokens_locks(&[providers, &stored].concat()))
                {
                    Ok(_tokens) => match store.empty_all() {
                        Ok(()) => report.store_emptied = true,
                        Err(e) => report.failures.push(("the store".into(), e.to_string())),
                    },
                    Err(e) if e.signal().is_some() => return Err(e),
                    Err(e) => report.failures.push(("the store".into(), e.to_string())),
                },
                Err(e) => report.failures.push(("the store".into(), e.to_string())),
            }
        }
        self.delete_log(report);
        Ok(())
    }
```

In `crates/tagteam-engine/src/store/tokens.rs` (Task 12's file), inside its `impl Store`, insert before the doc comment `/// Every model the provider's index has held since its last purge, sorted: a model whose`:

```rust
    /// Every provider with a row in a token or evidence table, sorted. A full purge takes the
    /// tokens lock of each (spec §13), one with no account, or one only another build
    /// registers, included.
    pub fn token_providers(&self) -> Result<Vec<ProviderId>, StoreError> {
        let c = self.lock();
        let mut stmt = c.prepare(
            "SELECT provider FROM token_files UNION SELECT provider FROM token_sessions \
             UNION SELECT provider FROM token_models UNION SELECT provider FROM token_state \
             UNION SELECT provider FROM session_sightings UNION SELECT provider FROM run_launches \
             UNION SELECT provider FROM active_history ORDER BY provider",
        )?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .map(|p| p.map(ProviderId::new))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

```

- [ ] **Step 4: Run them and see them pass**

```
cargo test -p tagteam-engine --locked --lib tokens::tests
cargo test -p tagteam-engine --locked --features test-hooks --test purge
cargo test -p tagteam-engine --locked --test purge
```

Expected:
- the lib run: `test result: ok. 2 passed; 0 failed`;
- `purge` with `test-hooks`: every test passes, the four new ones included. Without the feature, the three hook tests are compiled out and the rest pass. The existing ones still pass, because the tokens lock files are created inside `locks/` and kept like every lock file. `a_stored_provider_string_never_places_a_lock_outside_locks` counts only `engine-*` names, and a hostile provider's tokens lock is `tokens-<12 hex>.lock`.

- [ ] **Step 5: Lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy -p tagteam-engine --all-targets --locked -- -D warnings
cargo clippy -p tagteam-engine --all-targets --locked --features test-hooks -- -D warnings
```

Expected: no output from the format check, and clippy finishes with no warnings.

- [ ] **Step 6: Commit**

```
git add crates/tagteam-engine/src/tokens/mod.rs crates/tagteam-engine/src/purge.rs crates/tagteam-engine/src/store/tokens.rs crates/tagteam-engine/tests/purge.rs
git commit -m "Make purge wait for a running token refresh"
```

#### Notes for the controller

1. **`lock_until_free` lives in `tokens/mod.rs` and this task adds it.** Task 15 now comes after this task and uses it for `Wait::Wait`, as `super::lock_until_free`. The purge is its first caller, so it is never dead code. I verified it and its unit test in a scratch crate over the real `tagteam-provider`: clippy `-D warnings` was clean once a caller existed, the test passed, and rustfmt was clean. Task 15 must not define its own copy.
2. **Waiting with no deadline is my reading of spec §5.1** ("takes it too, waiting"). I considered a bounded wait, say 60 s, that reports the rows as kept for a rerun. I rejected it: it would be an untested branch, and the spec gives no cap. If you prefer the bound, it is one constant in `hold_tokens_locks` and a `LockError::Timeout` arm pushing a failure.
3. **A full purge holds several tokens locks at once.** That is the one exception to "nothing is taken while it is held". Spec §13 implies it, because `empty_all` is one transaction for every provider. The deadlock-freedom argument is in the task's lock-order paragraph. If you want spec §5.1's wording to say so, that belongs in Task 24's core-spec amendments.
4. **An interrupted wait now ends the purge with the interruption** (exit 130), not with a failure line. Everything before step 9 is done, and a rerun finishes it, as for an interruption at step 7. That needed `purge_provider_rest` and `purge_everything_else` to return `Result`.
5. **Which locks a full purge takes (plan review).** It takes the tokens lock of the union of two lists: its `providers` (the registered ones, plus any unregistered provider the plan names an account of, mirroring `hold_engine_locks`), and `Store::token_providers()`.
   - `Store::token_providers()` is every provider with rows in `token_files`, `token_sessions`, `token_models`, `token_state`, `session_sightings`, `run_launches` or `active_history`. So rows of a provider with no account, or of one only another build registers, are never emptied under that build's running refresh.
   - I checked the `UNION` query with `sqlite3` against Task 11's step-3 schema as extracted from this plan.
   - A registered provider without `transcripts` gets a lock file it never uses, which is harmless.
   - A `--provider P` purge takes P's lock only.
6. **Deterministic lock tests (plan review).**
   - `hold_tokens_locks` passes two hook points, `purge-tokens-lock` immediately before the wait and `purge-tokens-locked` immediately after.
   - `purge_waits_for_a_running_refresh` and `a_full_purge_waits_for_a_running_refresh_too` hold the lock as a refresh would. They wait on a channel for the first point, then set the cancel token. The purge must return the interruption, never reach the second point, and delete nothing. Were the acquisition removed, nothing between the points could be interrupted, so the tests would fail deterministically.
   - Each test then releases the lock and purges again. At the second point, `try_lock` must find the lock held, and the rows must end up gone.
   - `a_full_purge_locks_every_provider_with_token_rows` checks the same at the second point for a provider with token rows and no account.
   - The tests have no sleeps. The only time bound is a 30 s `recv_timeout` that guards against a hang when a point is missing.
   - `lock_until_free`'s unit test is deterministic too. Held elsewhere with the token set, the lock is never taken; free, it is taken and held.

---

### Task 18: The report view

**Depends on:** 2, 3, 4, 5, 6, 8, 12, 13, 14, 16
**Files:**
- Modify: `crates/tagteam-engine/src/tokens/view.rs` → `crates/tagteam-engine/src/tokens/view/mod.rs` (`git mv`, then the whole file replaced: spec §10's types, `token_report`, `token_index_state`, `TokenQuery::drill`). `tokens/mod.rs` keeps `mod view; pub use view::*;` unchanged: a directory module resolves the same `mod view;`.
- Create: `crates/tagteam-engine/src/tokens/view/group.rs` (attribution of each response, blocks, grouping, ordering, row building)
- Modify: `crates/tagteam-engine/Cargo.toml` (`jiff.workspace = true` under `[dependencies]`, after `fastrand`; Task 2 added `jiff` to the workspace and to `tagteam-core` only)
- Modify: `Cargo.lock` (`tagteam-engine` gains its `jiff` edge; no new package)
- Test: `crates/tagteam-engine/tests/tokens_view.rs` (new)

**Interfaces:**
- Consumes:
  ```rust
  // tagteam_core::tokens (Tasks 2-6)
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
  pub struct TokenCounts { pub input: u64, pub output: u64, pub cache_write_5m: u64, pub cache_write_1h: u64, pub cache_read: u64, pub web_search: u64 }
  impl TokenCounts { pub fn total(&self) -> u64; pub fn add(&mut self, other: &TokenCounts); }
  pub struct PriceTable { pub as_of: &'static str, pub models: &'static [(&'static str, ModelPrice)] }
  #[derive(Debug, Clone, Copy, Default, PartialEq)]
  pub struct CardOverride { pub input: Option<f64>, pub output: Option<f64>, pub cache_write_5m: Option<f64>, pub cache_write_1h: Option<f64>, pub cache_read: Option<f64> } // tests
  #[derive(Debug, Clone, Default, PartialEq)]
  pub struct PriceOverride { pub base: CardOverride, pub long_prompt_above: Option<u64>, pub long_prompt: CardOverride, pub fast: CardOverride, pub web_search_per_1k: Option<f64> } // tests
  impl Prices {
      pub fn new(table: &PriceTable, overrides: &BTreeMap<String, PriceOverride>) -> (Prices, Vec<String>);
      pub fn cost_nanos(&self, model: &str, counts: &TokenCounts, speed: Option<&str>) -> Option<u64>;
  }
  pub enum Home { Default, Profile(AccountId) }
  pub struct Sighting { pub home: Home, pub first_ms: i64, pub last_ms: i64 }
  pub struct Launch { pub account: AccountId, pub start_ms: i64, pub end_ms: Option<i64> }
  pub struct Coverage { pub epoch_ms: i64, pub seeded: bool, pub last_pre_epoch_remove_ms: Option<i64> }
  impl Launches { pub fn new(launches: Vec<Launch>) -> Self; }
  pub fn place(sightings: &[Sighting], launches: &Launches, coverage: &Coverage, t: i64) -> Placement;
  pub enum TimelineEvent { Switch { at_ms: i64, from: Option<AccountId>, to: AccountId }, Add { at_ms: i64, account: AccountId }, Remove { at_ms: i64, account: AccountId } }
  pub struct ActiveChange { pub at_ms: i64, pub account: Option<AccountId> }
  impl Timeline { pub fn build(events: &[TimelineEvent], history: &[ActiveChange], epoch_ms: i64, added_at: &BTreeMap<AccountId, i64>) -> Timeline; }
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
  pub enum Basis { Profile, Timeline, Unattributed }
  pub fn attribute(placement: &Placement, timeline: &Timeline, t: i64) -> (Option<AccountId>, Basis);
  pub const BLOCK_MS: i64 = 5 * 3_600_000;
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct BlockSpan { pub start_ms: i64, pub end_ms: i64, pub observed: bool }
  pub fn observed_blocks(resets_ms: &[i64]) -> Vec<BlockSpan>;
  pub fn assign_blocks(observed: &[BlockSpan], times_ms: &[i64]) -> (Vec<BlockSpan>, Vec<usize>); // times ascending
  pub struct Projection { pub elapsed_s: i64, pub remaining_s: i64, pub tokens_per_min: f64, pub cost_nanos_per_hour: Option<u64>, pub projected_tokens: u64, pub projected_cost_nanos: Option<u64> }
  pub fn project(span: &BlockSpan, first_response_ms: i64, now_ms: i64, tokens: u64, cost_nanos: Option<u64>) -> Option<Projection>;
  pub fn day_of(ms: i64, tz: &jiff::tz::TimeZone) -> jiff::civil::Date;
  pub fn month_of(ms: i64, tz: &jiff::tz::TimeZone) -> (i16, i8);
  pub fn day_bounds_ms(date: jiff::civil::Date, tz: &jiff::tz::TimeZone) -> (i64, i64);
  pub fn month_bounds_ms(year: i16, month: i8, tz: &jiff::tz::TimeZone) -> (i64, i64);

  // tagteam_provider (Task 7; existing describe_window, display_name)
  pub struct Capabilities { /* existing */ pub transcripts: bool }
  fn price_table(&self) -> &'static PriceTable;
  fn describe_window(&self, key: &str) -> Option<Window>; // WindowKind::Short is the 5-hour window (spec §9)

  // Store (Tasks 11-12), tagteam_engine::store; `UsageRow`, `SightingRow`, `SightingSource`,
  // `RefreshStats` and `usage_key` re-exported from store/mod.rs
  pub struct UsageRow { pub key: i64, pub session_id: String, pub project: Option<String>, pub at_ms: i64, pub model: String, pub sidechain: bool, pub counts: TokenCounts, pub speed: Option<String>, pub finalized: bool }
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
  pub struct RefreshStats { pub files: u64, pub responses: u64, pub skipped_lines: u64, pub unreadable_files: u64 }
  pub struct TokenStateRow { pub refreshed_at: Option<i64>, pub changed_at: Option<i64>, pub stats: RefreshStats, pub pruned_at: Option<i64> }
  // stats.responses is the index's live count (each batch adds, each prune subtracts); the other stats are the last completed refresh's
  pub enum SightingSource { Run, Scan, Statusline }
  pub struct SightingRow { pub session_id: String, pub home: String, pub first_at: i64, pub last_at: i64, pub source: SightingSource, pub recorded_at: i64 }
  pub struct LaunchRow { pub account_id: String, pub started_at: i64, pub ended_at: Option<i64>, pub pid: Option<u32>, pub recorded_at: i64 }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct EpochRow { pub at: i64, pub seeded: bool }
  pub fn usage_key(provider: &ProviderId, message_id: &str, request_id: &str) -> i64;
  impl Store {
      pub fn token_usage(&self, provider: &ProviderId, since_ms: Option<i64>, until_ms: Option<i64>, session: Option<&str>) -> Result<Vec<UsageRow>, StoreError>; // ordered by at
      pub fn sightings(&self, provider: &ProviderId) -> Result<Vec<SightingRow>, StoreError>;
      pub fn launches(&self, provider: &ProviderId) -> Result<Vec<LaunchRow>, StoreError>;
      pub fn token_epoch(&self) -> Result<Option<EpochRow>, StoreError>;
      pub fn active_history(&self, provider: &ProviderId) -> Result<Vec<(i64, Option<String>)>, StoreError>;
      pub fn token_state(&self, provider: &ProviderId) -> Result<Option<TokenStateRow>, StoreError>;
      pub fn evidence_changed_at(&self, provider: &ProviderId) -> Result<Option<i64>, StoreError>;
      pub fn token_span(&self, provider: &ProviderId) -> Result<(Option<i64>, Option<i64>), StoreError>;
      // tests only:
      pub fn commit_token_batch(&self, provider: &ProviderId, rows: &[UsageRow], files: &[TokenFileRow], removed: &[String], now_ms: i64) -> Result<BatchOutcome, StoreError>;
      pub fn finish_token_refresh(&self, provider: &ProviderId, stats: &RefreshStats, now_ms: i64) -> Result<(), StoreError>;
      pub fn record_sighting(&self, provider: &ProviderId, s: &SightingRow, min_gap_ms: i64) -> Result<bool, StoreError>;
      pub fn open_launch(&self, provider: &ProviderId, account_id: &str, started_at: i64, now_ms: i64) -> Result<(), StoreError>;
      // existing: accounts, events, usage_samples; tests: account, insert_event, set_active, delete_account
  }

  // Settings (Task 13): every provider's `[provider.<id>.prices]` tables, whichever provider
  // `Settings::load` was given; tests also use `Settings::load` and `config_path`
  pub struct Settings { /* existing */ pub prices: BTreeMap<ProviderId, BTreeMap<String, PriceOverride>> }
  pub fn config_path(env: &Env) -> PathBuf;
  impl Settings { pub fn load(env: &Env, provider: &ProviderId) -> (Settings, Vec<String>); }

  // Task 10 (tests): FakeAgent's "fake-1" base {1, 2, 3, 4, 0.5}; `FakeAgent::without_transcripts(self) -> Self`

  // Task 14: tokens/mod.rs declares `mod view;` and `pub use view::*;`; tokens/view.rs holds only a `//!` line.
  ```
- Produces (`tagteam_engine::tokens`, re-exported by Task 14's `pub use view::*;`; the TUI and Task 19 render exactly these):
  ```rust
  pub use tagteam_core::tokens::{Basis, TokenCounts};

  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
  pub enum TokenPeriod { Day, Month, Session, Block }
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
  pub enum TokenGrouping { None, Account, Model, AccountModel }
  #[derive(Debug, Clone, PartialEq, Eq, Hash)]
  pub enum AccountSel { Account(AccountId), Unattributed }
  #[derive(Debug, Clone, PartialEq)]
  pub struct TokenQuery {
      pub provider: ProviderId,
      pub period: TokenPeriod,
      pub grouping: TokenGrouping,
      pub since_ms: Option<i64>,              // inclusive
      pub until_ms: Option<i64>,              // exclusive
      pub accounts: Option<Vec<AccountSel>>,  // None: every account and unattributed
      pub models: Option<Vec<String>>,        // None: every model; exact names otherwise
      pub session: Option<String>,            // only this session's responses
      pub tz: jiff::tz::TimeZone,             // day and month boundaries
  }
  #[derive(Debug, Clone, PartialEq)]
  pub struct TokenReport {
      pub query: TokenQuery,
      pub time_zone: String,                  // the IANA name of `tz`, or "UTC"
      pub rows: Vec<TokenRow>,
      pub total: TokenTotal,
      pub index: IndexState,
      pub unpriced_models: Vec<String>,       // sorted
      pub prices_as_of: &'static str,
      pub price_overrides: Vec<String>,       // sorted
  }
  #[derive(Debug, Clone, PartialEq)]
  pub struct TokenRow {
      pub key: RowKey,
      pub period: PeriodKey,
      pub account: Option<RowAccount>,        // Some when grouped by account (always for Block)
      pub model: Option<String>,              // Some when grouped by model
      pub accounts: Vec<RowAccount>,          // every account present, in report order
      pub models: Vec<String>,                // every model present, by name
      pub counts: TokenCounts,
      pub responses: u64,
      pub unfinalized: u64,
      pub cost: Option<Cost>,
  }
  #[derive(Debug, Clone, PartialEq)]
  pub enum PeriodKey {
      Day { date: jiff::civil::Date },
      Month { year: i16, month: i8 },
      Session { session_id: String, project: Option<String>, first_ms: i64, last_ms: i64 },
      Block { start_ms: i64, end_ms: i64, observed: bool, active: Option<ActiveBlock> },
  }
  #[derive(Debug, Clone, Copy, PartialEq)]
  pub struct ActiveBlock { pub elapsed_s: i64, pub remaining_s: i64, pub tokens_per_min: f64, pub cost_per_hour: Option<Cost>, pub projected_tokens: u64, pub projected_cost: Option<Cost> }
  #[derive(Debug, Clone, PartialEq, Eq, Hash)]
  pub enum RowAccount { Account { id: AccountId, position: u32, label: String, alias: Option<String> }, Removed { id: AccountId }, Unattributed }
  #[derive(Debug, Clone, PartialEq, Eq, Hash)]
  pub struct RowKey(String);                  // "day:2026-10-14|account:<id>|model:<name>"
  impl RowKey { pub fn as_str(&self) -> &str; }
  impl fmt::Display for RowKey;
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct Cost { pub usd_nanos: u64, pub partial: bool }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct TokenTotal { pub counts: TokenCounts, pub responses: u64, pub unfinalized: u64, pub cost: Option<Cost>, pub by_basis: [(Basis, u64); 3] }
  impl Default for TokenTotal;               // zeros; by_basis in the order Profile, Timeline, Unattributed
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
  pub struct IndexState { pub refreshed_at_ms: Option<i64>, pub changed_at_ms: Option<i64>, pub files: u64, pub responses: u64, pub skipped_lines: u64, pub unreadable_files: u64, pub oldest_ms: Option<i64>, pub newest_ms: Option<i64> }

  impl Engine {
      /// Reads only; never refreshes and never creates the store (no store: an empty report).
      /// InvalidInput("<name> has no transcripts") for a provider without the capability.
      pub fn token_report(&self, q: &TokenQuery) -> Result<TokenReport, EngineError>;
      /// token_state, evidence_changed_at and token_span; IndexState::default() without a store.
      pub fn token_index_state(&self, provider: &ProviderId) -> Result<IndexState, EngineError>;
  }
  impl TokenQuery { pub fn drill(&self, row: &TokenRow) -> Option<TokenQuery>; }
  ```
  No serde derives: Task 19 builds `--json` by hand, as `history` does, so the camelCase shape of spec §11.2 stays in the CLI.

- [ ] **Step 1: Write the failing tests**

First give the engine the time zone crate its view and these tests name. In `crates/tagteam-engine/Cargo.toml`, under `[dependencies]`, after `fastrand.workspace = true`, add:

```toml
jiff.workspace = true
```

Then create `crates/tagteam-engine/tests/tokens_view.rs`. It seeds the store through Task 12's API with Claude Code as the provider, so it needs no transcript files. Every response lies in September 2026. `View::new` sets the fixture clock to `T0 + 40 days` before the store opens, so the migration dates the epoch after every response, and it asserts that. Before the epoch the default home's account comes from `events`, whose times the test chooses. The `active_history` trigger stamps SQLite's own clock, so `index_state_moves_on_new_evidence_alone` reads back the time it recorded instead of assuming one. Every attributed response is sighted, and every unattributed one is either sighted in two homes or covered by an open launch. No expectation depends on `token_epoch.seeded`. Prices come from two override-added models (`model-a`, `model-b`), so no expectation depends on the shipped table's rates. `Settings.prices` holds every provider's tables, keyed by provider. `price_overrides_follow_the_report_provider` loads the settings of a Claude Code command from a real `config.toml` and reports for FakeAgent, so a report has to take its own provider's table.

```rust
//! The token report (spec §10, §10.1): grouping, filters, order, blocks, cost, drill-down and
//! the index state, over a store seeded through its own API (usage batches, sightings,
//! launches and events), with Claude Code as the provider.

mod common;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;

use common::{FakeFx, Fx, add, cc, record_reading, usage_window};
use jiff::civil::date;
use jiff::tz::TimeZone;
use tagteam_core::tokens::{BlockSpan, CardOverride, PriceOverride, project};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId, WindowKind};
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::{Settings, config_path};
use tagteam_engine::store::{
    EventRow, RefreshStats, SightingRow, SightingSource, Store, UsageRow, usage_key,
};
use tagteam_engine::tokens::{
    AccountSel, ActiveBlock, Basis, Cost, IndexState, PeriodKey, RowAccount, TokenCounts,
    TokenGrouping, TokenPeriod, TokenQuery, TokenReport,
};
use tagteam_engine::vault::{KeychainVault, Vault};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_fake::{FAKE_AGENT, FakeAgent};
use tagteam_provider::{Provider, RunShell};

const MIN: i64 = 60_000;
const H: i64 = 60 * MIN;
const D: i64 = 24 * H;
/// 2026-09-01T00:00:00Z. Every response lies within 40 days of it. The migration dates the
/// store's epoch with the engine clock, which `View::new` sets to `T0 + 40 days` first, so every
/// response predates the epoch and the default home's account comes from `events` (spec §7.3).
const T0: i64 = 1_788_220_800_000;

/// model-a costs $1 / $2 / $3 / $4 / $0.50 per MTok and model-b twice that. Neither is in the
/// shipped table, so both are overrides that add a model (spec §8.2). Any other model is
/// unpriced.
fn settings() -> Settings {
    let card = |k: f64| CardOverride {
        input: Some(k),
        output: Some(2.0 * k),
        cache_write_5m: Some(3.0 * k),
        cache_write_1h: Some(4.0 * k),
        cache_read: Some(0.5 * k),
    };
    let mut prices = BTreeMap::new();
    for (model, k) in [("model-a", 1.0), ("model-b", 2.0)] {
        prices.insert(
            model.to_owned(),
            PriceOverride {
                base: card(k),
                ..PriceOverride::default()
            },
        );
    }
    Settings {
        prices: BTreeMap::from([(cc(), prices)]),
        ..Settings::default()
    }
}

fn counts(input: u64, output: u64) -> TokenCounts {
    TokenCounts {
        input,
        output,
        ..TokenCounts::default()
    }
}

fn keys(r: &TokenReport) -> Vec<&str> {
    r.rows.iter().map(|row| row.key.as_str()).collect()
}

/// A store with two accounts, a@x.co at position 1 and b@x.co at 2, and no evidence.
struct View {
    fx: Fx,
    engine: Engine,
    store: Arc<Store>,
    a: AccountId,
    b: AccountId,
    responses: Cell<u32>,
}

impl View {
    fn new() -> Self {
        let fx = Fx::new();
        // Before the store opens: its migration dates the epoch with this clock.
        fx.clock.set(T0 + 40 * D);
        let engine = fx.engine_with_settings(settings());
        let store = engine.store().unwrap();
        let a = add(&store, &cc(), "acct-a", "a@x.co", 1);
        let b = add(&store, &cc(), "acct-b", "b@x.co", 2);
        let epoch = store
            .token_epoch()
            .unwrap()
            .expect("step 3 records the epoch");
        assert_eq!(
            epoch.at,
            T0 + 40 * D,
            "the responses these tests index must predate the store's epoch"
        );
        View {
            fx,
            engine,
            store,
            a,
            b,
            responses: Cell::new(0),
        }
    }

    /// A finalized response of `session` at `at`, of `input` and `output` tokens of `model`.
    fn usage(&self, session: &str, at: i64, model: &str, input: u64, output: u64) -> UsageRow {
        let n = self.responses.get() + 1;
        self.responses.set(n);
        UsageRow {
            key: usage_key(&cc(), &format!("msg-{n}"), &format!("req-{n}")),
            session_id: session.to_owned(),
            project: Some(format!("/work/{session}")),
            at_ms: at,
            model: model.to_owned(),
            sidechain: false,
            counts: counts(input, output),
            speed: None,
            finalized: true,
        }
    }

    /// One batch of a refresh that has not completed (spec §5.2).
    fn index(&self, rows: &[UsageRow]) {
        self.store
            .commit_token_batch(&cc(), rows, &[], &[], self.engine.now_ms())
            .unwrap();
    }

    /// `session` seen alive in `home` ("" for the default home) from `first` to `last`.
    fn sight(&self, session: &str, home: &str, first: i64, last: i64) {
        let row = SightingRow {
            session_id: session.to_owned(),
            home: home.to_owned(),
            first_at: first,
            last_at: last,
            source: SightingSource::Scan,
            recorded_at: self.engine.now_ms(),
        };
        assert!(self.store.record_sighting(&cc(), &row, MIN).unwrap());
    }

    fn event(&self, at: i64, kind: &str, from: Option<&AccountId>, to: Option<&AccountId>) {
        let row = EventRow {
            at,
            provider: cc(),
            kind: kind.into(),
            from_id: from.cloned(),
            to_id: to.cloned(),
            trigger: None,
            source: "cli".into(),
            detail: None,
        };
        self.store.insert_event(&row, true).unwrap();
    }

    fn query(&self, period: TokenPeriod, grouping: TokenGrouping) -> TokenQuery {
        TokenQuery {
            provider: cc(),
            period,
            grouping,
            since_ms: None,
            until_ms: None,
            accounts: None,
            models: None,
            session: None,
            tz: TimeZone::UTC,
        }
    }

    fn report(&self, q: &TokenQuery) -> TokenReport {
        self.engine.token_report(q).unwrap()
    }

    fn account(&self, id: &AccountId) -> RowAccount {
        let row = self.store.account(id).unwrap().unwrap();
        RowAccount::Account {
            id: row.id,
            position: row.position,
            label: row.label,
            alias: row.alias,
        }
    }
}

#[test]
fn days_group_by_account_and_model_newest_first() {
    let v = View::new();
    v.sight("s-a", "acct-a", T0, T0 + 3 * D);
    v.sight("s-b", "acct-b", T0, T0 + 3 * D);
    v.index(&[
        v.usage("s-a", T0 + H, "model-b", 10, 1),
        v.usage("s-a", T0 + 2 * H, "model-a", 20, 2),
        v.usage("s-b", T0 + 3 * H, "model-a", 40, 4),
        v.usage("s-a", T0 + D + H, "model-a", 30, 3),
        v.usage("s-b", T0 + D + 2 * H, "model-b", 50, 5),
    ]);

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::AccountModel));
    assert_eq!(
        keys(&r),
        [
            "day:2026-09-02|account:acct-a|model:model-a",
            "day:2026-09-02|account:acct-b|model:model-b",
            "day:2026-09-01|account:acct-a|model:model-a",
            "day:2026-09-01|account:acct-a|model:model-b",
            "day:2026-09-01|account:acct-b|model:model-a",
        ]
    );
    let first = &r.rows[0];
    assert_eq!(
        first.period,
        PeriodKey::Day {
            date: date(2026, 9, 2)
        }
    );
    assert_eq!(first.account, Some(v.account(&v.a)));
    assert_eq!(first.model.as_deref(), Some("model-a"));
    assert_eq!(first.accounts, [v.account(&v.a)]);
    assert_eq!(first.models, ["model-a"]);
    assert_eq!(
        (first.counts, first.responses, first.unfinalized),
        (counts(30, 3), 1, 0)
    );
    assert_eq!(r.rows[4].counts, counts(40, 4));

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::None));
    assert_eq!(keys(&r), ["day:2026-09-02", "day:2026-09-01"]);
    let day = &r.rows[1];
    assert_eq!((day.account.as_ref(), day.model.as_deref()), (None, None));
    assert_eq!(day.accounts, [v.account(&v.a), v.account(&v.b)]);
    assert_eq!(day.models, ["model-a", "model-b"]);
    assert_eq!((day.counts, day.responses), (counts(70, 7), 3));
    assert_eq!((r.total.counts, r.total.responses), (counts(150, 15), 5));

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Model));
    assert_eq!(
        keys(&r),
        [
            "day:2026-09-02|model:model-a",
            "day:2026-09-02|model:model-b",
            "day:2026-09-01|model:model-a",
            "day:2026-09-01|model:model-b",
        ]
    );
}

#[test]
fn months_follow_the_query_zone() {
    let v = View::new();
    v.sight("s-a", "acct-a", T0 - D, T0 + D);
    // 2026-08-31T23:30Z is still August in UTC, and 2026-09-01T01:30 in Brussels (CEST).
    v.index(&[
        v.usage("s-a", T0 - 30 * MIN, "model-a", 1, 0),
        v.usage("s-a", T0 + 2 * H, "model-a", 2, 0),
    ]);

    let utc = v.report(&v.query(TokenPeriod::Month, TokenGrouping::None));
    assert_eq!(utc.time_zone, "UTC");
    assert_eq!(keys(&utc), ["month:2026-09", "month:2026-08"]);
    assert_eq!(
        utc.rows[1].period,
        PeriodKey::Month {
            year: 2026,
            month: 8
        }
    );
    assert_eq!(
        keys(&v.report(&v.query(TokenPeriod::Day, TokenGrouping::None))),
        ["day:2026-09-01", "day:2026-08-31"]
    );

    let brussels = TimeZone::get("Europe/Brussels").unwrap();
    let local = v.report(&TokenQuery {
        tz: brussels.clone(),
        ..v.query(TokenPeriod::Month, TokenGrouping::None)
    });
    assert_eq!(local.time_zone, "Europe/Brussels");
    assert_eq!(keys(&local), ["month:2026-09"]);
    assert_eq!(local.rows[0].counts, counts(3, 0));
    let local = v.report(&TokenQuery {
        tz: brussels,
        ..v.query(TokenPeriod::Day, TokenGrouping::None)
    });
    assert_eq!(keys(&local), ["day:2026-09-01"]);
}

#[test]
fn a_session_spanning_two_accounts_splits_by_account() {
    let v = View::new();
    // s-1 ran in a's profile, then was resumed in b's (spec §7.4 A1).
    v.sight("s-1", "acct-a", T0, T0 + 2 * H);
    v.sight("s-1", "acct-b", T0 + 3 * H, T0 + 5 * H);
    v.sight("s-2", "acct-a", T0 + 6 * H, T0 + 7 * H);
    v.index(&[
        v.usage("s-1", T0 + H, "model-a", 1, 0),
        v.usage("s-1", T0 + 4 * H, "model-b", 2, 0),
        v.usage("s-2", T0 + 6 * H + 30 * MIN, "model-a", 4, 0),
    ]);
    let s1 = |first_ms, last_ms| PeriodKey::Session {
        session_id: "s-1".into(),
        project: Some("/work/s-1".into()),
        first_ms,
        last_ms,
    };

    let r = v.report(&v.query(TokenPeriod::Session, TokenGrouping::Account));
    assert_eq!(
        keys(&r),
        [
            "session:s-2|account:acct-a",
            "session:s-1|account:acct-a",
            "session:s-1|account:acct-b",
        ]
    );
    assert_eq!(r.rows[1].period, s1(T0 + H, T0 + H));
    assert_eq!(r.rows[2].period, s1(T0 + 4 * H, T0 + 4 * H));
    assert_eq!(r.rows[2].account, Some(v.account(&v.b)));

    let r = v.report(&v.query(TokenPeriod::Session, TokenGrouping::None));
    assert_eq!(keys(&r), ["session:s-2", "session:s-1"]);
    let row = &r.rows[1];
    assert_eq!(row.period, s1(T0 + H, T0 + 4 * H));
    assert_eq!(row.accounts, [v.account(&v.a), v.account(&v.b)]);
    assert_eq!(row.models, ["model-a", "model-b"]);
    assert_eq!((row.counts, row.responses), (counts(3, 0), 2));
}

#[test]
fn filters_apply_before_grouping_and_some_empty_matches_nothing() {
    let v = View::new();
    v.sight("s-a", "acct-a", T0, T0 + D);
    v.sight("s-b", "acct-b", T0, T0 + D);
    // Open in both profiles at once: unattributed (spec §7.1).
    v.sight("s-u", "acct-a", T0, T0 + D);
    v.sight("s-u", "acct-b", T0, T0 + D);
    v.index(&[
        v.usage("s-a", T0 + H, "model-a", 1, 0),
        v.usage("s-a", T0 + 2 * H, "model-b", 2, 0),
        v.usage("s-b", T0 + 3 * H, "model-a", 4, 0),
        v.usage("s-u", T0 + 4 * H, "model-a", 8, 0),
    ]);
    let day = v.query(TokenPeriod::Day, TokenGrouping::None);
    let only_row = |q: TokenQuery| {
        let r = v.report(&q);
        assert_eq!(r.rows.len(), 1, "{q:?}");
        r.rows.into_iter().next().unwrap()
    };

    let row = only_row(TokenQuery {
        accounts: Some(vec![AccountSel::Account(v.a.clone())]),
        ..day.clone()
    });
    assert_eq!(row.accounts, [v.account(&v.a)]);
    assert_eq!(row.models, ["model-a", "model-b"]);
    assert_eq!(row.counts, counts(3, 0));

    let row = only_row(TokenQuery {
        models: Some(vec!["model-b".into()]),
        ..day.clone()
    });
    assert_eq!(
        (row.accounts, row.models, row.counts),
        (
            vec![v.account(&v.a)],
            vec!["model-b".to_owned()],
            counts(2, 0)
        )
    );

    let row = only_row(TokenQuery {
        accounts: Some(vec![
            AccountSel::Unattributed,
            AccountSel::Account(v.b.clone()),
        ]),
        ..day.clone()
    });
    assert_eq!(row.accounts, [v.account(&v.b), RowAccount::Unattributed]);
    assert_eq!(row.counts, counts(12, 0));

    let row = only_row(TokenQuery {
        session: Some("s-b".into()),
        ..day.clone()
    });
    assert_eq!(row.counts, counts(4, 0));

    for q in [
        TokenQuery {
            accounts: Some(vec![]),
            ..day.clone()
        },
        TokenQuery {
            models: Some(vec![]),
            ..day.clone()
        },
        // Exact names only: the CLI resolves a `--model` substring first (spec §11.1).
        TokenQuery {
            models: Some(vec!["model".into()]),
            ..day.clone()
        },
    ] {
        let r = v.report(&q);
        assert!(r.rows.is_empty(), "{q:?}");
        assert_eq!((r.total.responses, r.total.cost), (0, None), "{q:?}");
    }
}

#[test]
fn removed_accounts_keep_their_rows() {
    let v = View::new();
    let c = add(&v.store, &cc(), "acct-c", "c@x.co", 3);
    // c's profile session, and a default-home session while c was the login (spec §7.3).
    v.sight("s-c", "acct-c", T0, T0 + D);
    v.event(T0 - D, "switch", None, Some(&c));
    v.sight("s-d", "", T0, T0 + D);
    v.sight("s-a", "acct-a", T0, T0 + D);
    v.index(&[
        v.usage("s-a", T0 + H, "model-a", 1, 0),
        v.usage("s-c", T0 + 2 * H, "model-a", 2, 0),
        v.usage("s-d", T0 + 3 * H, "model-a", 4, 0),
    ]);
    v.store.delete_account(&c).unwrap();
    v.event(T0 + 2 * D, "remove", Some(&c), None);

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Account));
    assert_eq!(
        keys(&r),
        [
            "day:2026-09-01|account:acct-a",
            "day:2026-09-01|account:acct-c"
        ]
    );
    assert_eq!(
        r.rows[1].account,
        Some(RowAccount::Removed { id: c.clone() })
    );
    assert_eq!(r.rows[1].counts, counts(6, 0));
    assert_eq!(
        r.total.by_basis,
        [
            (Basis::Profile, 3),
            (Basis::Timeline, 4),
            (Basis::Unattributed, 0)
        ]
    );

    // A filter naming the removed account's id matches its rows (spec §10.1).
    let r = v.report(&TokenQuery {
        accounts: Some(vec![AccountSel::Account(c.clone())]),
        ..v.query(TokenPeriod::Day, TokenGrouping::None)
    });
    assert_eq!(keys(&r), ["day:2026-09-01"]);
    assert_eq!(r.rows[0].accounts, [RowAccount::Removed { id: c }]);
}

#[test]
fn unattributed_is_its_own_bucket_and_last() {
    let v = View::new();
    v.sight("s-a", "acct-a", T0, T0 + D);
    v.sight("s-b", "acct-b", T0, T0 + D);
    // b's profile may have been running, and s-u was never sighted: the default home is not
    // its only candidate, so it is unattributed (spec §7.2).
    v.store
        .open_launch(&cc(), v.b.as_str(), T0 - D, v.engine.now_ms())
        .unwrap();
    v.index(&[
        v.usage("s-u", T0 + H, "model-a", 1, 0),
        v.usage("s-b", T0 + 2 * H, "model-a", 2, 0),
        v.usage("s-a", T0 + 3 * H, "model-a", 4, 0),
    ]);

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Account));
    assert_eq!(
        keys(&r),
        [
            "day:2026-09-01|account:acct-a",
            "day:2026-09-01|account:acct-b",
            "day:2026-09-01|account:unattributed",
        ]
    );
    assert_eq!(r.rows[2].account, Some(RowAccount::Unattributed));
    assert_eq!(r.rows[2].counts, counts(1, 0));

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::None));
    assert_eq!(
        r.rows[0].accounts,
        [v.account(&v.a), v.account(&v.b), RowAccount::Unattributed]
    );
}

#[test]
fn blocks_are_per_account_with_the_active_projection() {
    let v = View::new();
    v.sight("s-a", "acct-a", T0, T0 + D);
    v.sight("s-b", "acct-b", T0, T0 + D);
    // a's 5h window, read at T0+1h30, resets at T0+6h: the observed block [T0+1h, T0+6h). Its
    // 7d window is not the short window and makes no block (spec §9).
    record_reading(
        &v.engine,
        &v.a,
        &[
            usage_window("5h", WindowKind::Short, 10.0, (T0 + 6 * H) / 1000),
            usage_window("7d", WindowKind::Long, 5.0, (T0 + 3 * D) / 1000),
        ],
        (T0 + 90 * MIN) / 1000,
        (T0 + 2 * H) / 1000,
    );
    v.index(&[
        v.usage("s-a", T0 + 2 * H, "model-a", 100, 0),
        v.usage("s-b", T0 + 150 * MIN, "model-a", 200, 0),
        v.usage("s-a", T0 + 5 * H, "model-b", 300, 0),
        // Past the observed block: an inferred one, [T0+7h, T0+12h).
        v.usage("s-a", T0 + 450 * MIN, "model-a", 600, 0),
    ]);
    let now = T0 + 8 * H;
    v.fx.clock.set(now);
    let at = |hours: i64| T0 + hours * H;
    let block = |hours: i64, account: &str| format!("block:{}|account:{account}", at(hours));

    let q = v.query(TokenPeriod::Block, TokenGrouping::None);
    let r = v.report(&q);
    assert_eq!(
        keys(&r),
        [block(7, "acct-a"), block(2, "acct-b"), block(1, "acct-a")]
    );
    // 600 tokens of model-a, 30 minutes after the block's first response, 4 h before its end.
    let span = BlockSpan {
        start_ms: at(7),
        end_ms: at(12),
        observed: false,
    };
    let want = project(&span, T0 + 450 * MIN, now, 600, Some(600_000)).unwrap();
    assert_eq!(want.remaining_s, 14_400);
    let priced = |nanos: Option<u64>| {
        nanos.map(|usd_nanos| Cost {
            usd_nanos,
            partial: false,
        })
    };
    assert_eq!(
        r.rows[0].period,
        PeriodKey::Block {
            start_ms: at(7),
            end_ms: at(12),
            observed: false,
            active: Some(ActiveBlock {
                elapsed_s: want.elapsed_s,
                remaining_s: want.remaining_s,
                tokens_per_min: want.tokens_per_min,
                cost_per_hour: priced(want.cost_nanos_per_hour),
                projected_tokens: want.projected_tokens,
                projected_cost: priced(want.projected_cost_nanos),
            }),
        }
    );
    assert_eq!(
        r.rows[1].period,
        PeriodKey::Block {
            start_ms: at(2),
            end_ms: at(7),
            observed: false,
            active: None,
        }
    );
    assert_eq!(
        r.rows[2].period,
        PeriodKey::Block {
            start_ms: at(1),
            end_ms: at(6),
            observed: true,
            active: None,
        }
    );
    assert_eq!(r.rows[2].counts, counts(400, 0));

    let r = v.report(&v.query(TokenPeriod::Block, TokenGrouping::Model));
    assert_eq!(
        keys(&r)[2..],
        [
            format!("{}|model:model-a", block(1, "acct-a")),
            format!("{}|model:model-b", block(1, "acct-a")),
        ]
    );

    // A block that straddles `since` is reported whole; one that ends by it is not.
    let q = TokenQuery {
        since_ms: Some(at(3)),
        ..q
    };
    let r = v.report(&q);
    assert_eq!(r.rows.len(), 3);
    assert_eq!(r.rows[2].counts, counts(400, 0));
    assert_eq!(
        q.drill(&r.rows[2]),
        Some(TokenQuery {
            period: TokenPeriod::Session,
            since_ms: Some(at(1)),
            until_ms: Some(at(6)),
            accounts: Some(vec![AccountSel::Account(v.a.clone())]),
            ..q.clone()
        })
    );
    let r = v.report(&TokenQuery {
        since_ms: Some(at(6) + 30 * MIN),
        ..q
    });
    assert_eq!(keys(&r), [block(7, "acct-a"), block(2, "acct-b")]);
    assert_eq!(r.rows[1].counts, counts(200, 0));
}

#[test]
fn cost_is_partial_with_an_unpriced_model_and_none_when_all_are() {
    let v = View::new();
    v.sight("s-a", "acct-a", T0, T0 + 3 * D);
    v.index(&[
        v.usage("s-a", T0 + H, "model-a", 10, 20),
        v.usage("s-a", T0 + 2 * H, "model-x", 5, 0),
        v.usage("s-a", T0 + D + H, "model-x", 7, 0),
    ]);
    // model-a: 10 × 1,000 + 20 × 2,000 nano-dollars (spec §8.3).
    let priced = |partial| Cost {
        usd_nanos: 50_000,
        partial,
    };

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::None));
    assert_eq!(keys(&r), ["day:2026-09-02", "day:2026-09-01"]);
    assert_eq!((r.rows[0].cost, r.rows[1].cost), (None, Some(priced(true))));
    assert_eq!(r.total.cost, Some(priced(true)));
    assert_eq!(r.unpriced_models, ["model-x"]);
    assert_eq!(r.price_overrides, ["model-a", "model-b"]);
    assert_eq!(r.prices_as_of, v.fx.cc.price_table().as_of);

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Model));
    assert_eq!(
        keys(&r),
        [
            "day:2026-09-02|model:model-x",
            "day:2026-09-01|model:model-a",
            "day:2026-09-01|model:model-x",
        ]
    );
    let costs: Vec<Option<Cost>> = r.rows.iter().map(|row| row.cost).collect();
    assert_eq!(costs, [None, Some(priced(false)), None]);
}

#[test]
fn drill_from_a_day_gives_its_sessions_and_from_a_session_its_models() {
    let v = View::new();
    for (session, home) in [
        ("s-1", "acct-a"),
        ("s-2", "acct-a"),
        ("s-3", "acct-a"),
        ("s-4", "acct-b"),
    ] {
        v.sight(session, home, T0, T0 + 3 * D);
    }
    v.index(&[
        v.usage("s-1", T0 + 15 * MIN, "model-a", 1, 0),
        v.usage("s-1", T0 + H, "model-a", 2, 0),
        v.usage("s-1", T0 + 2 * H, "model-b", 4, 0),
        v.usage("s-2", T0 + 3 * H, "model-a", 8, 0),
        v.usage("s-4", T0 + 4 * H, "model-a", 16, 0),
        v.usage("s-3", T0 + D + H, "model-a", 32, 0),
    ]);
    let since = T0 + 30 * MIN;
    let q = TokenQuery {
        since_ms: Some(since),
        ..v.query(TokenPeriod::Day, TokenGrouping::Account)
    };
    let r = v.report(&q);
    assert_eq!(
        keys(&r),
        [
            "day:2026-09-02|account:acct-a",
            "day:2026-09-01|account:acct-a",
            "day:2026-09-01|account:acct-b",
        ]
    );
    let day = &r.rows[1];

    // The day within the query's range, its account as the filter, the same grouping.
    let sessions = q.drill(day).unwrap();
    assert_eq!(
        sessions,
        TokenQuery {
            period: TokenPeriod::Session,
            since_ms: Some(since),
            until_ms: Some(T0 + D),
            accounts: Some(vec![AccountSel::Account(v.a.clone())]),
            ..q.clone()
        }
    );
    let r = v.report(&sessions);
    assert_eq!(
        keys(&r),
        ["session:s-2|account:acct-a", "session:s-1|account:acct-a"]
    );
    assert_eq!(r.total.counts, day.counts);

    // A session: the whole session, by account and model.
    let models = sessions.drill(&r.rows[1]).unwrap();
    assert_eq!(
        models,
        TokenQuery {
            grouping: TokenGrouping::AccountModel,
            since_ms: None,
            until_ms: None,
            session: Some("s-1".into()),
            ..sessions.clone()
        }
    );
    let r = v.report(&models);
    assert_eq!(
        keys(&r),
        [
            "session:s-1|account:acct-a|model:model-a",
            "session:s-1|account:acct-a|model:model-b",
        ]
    );
    assert_eq!(r.rows[0].counts, counts(3, 0));
    // A session's per-model rows are the last level.
    assert_eq!(models.drill(&r.rows[0]), None);
}

#[test]
fn row_keys_are_stable_across_refreshes() {
    let v = View::new();
    v.sight("s-1", "acct-a", T0, T0 + D);
    let stale = v.usage("s-1", T0 + H, "model-a", 1, 2);
    v.index(&[stale.clone()]);
    let queries = [
        v.query(TokenPeriod::Day, TokenGrouping::AccountModel),
        v.query(TokenPeriod::Month, TokenGrouping::AccountModel),
        v.query(TokenPeriod::Session, TokenGrouping::AccountModel),
        v.query(TokenPeriod::Block, TokenGrouping::AccountModel),
    ];
    let before: Vec<Vec<String>> = queries
        .iter()
        .map(|q| keys(&v.report(q)).into_iter().map(str::to_owned).collect())
        .collect();
    assert_eq!(before[2], ["session:s-1|account:acct-a|model:model-a"]);
    assert_eq!(
        before[3],
        [format!("block:{}|account:acct-a|model:model-a", T0 + H)]
    );

    // A later refresh finds that response's final line and another response of the same day,
    // session and block, and new evidence lands: every row keeps its key.
    v.fx.clock.advance_ms(MIN);
    v.index(&[
        UsageRow {
            counts: counts(1, 9),
            ..stale
        },
        v.usage("s-1", T0 + 3 * H, "model-a", 4, 0),
    ]);
    v.sight("s-1", "acct-a", T0 + 2 * D, T0 + 3 * D);
    v.store
        .finish_token_refresh(&cc(), &RefreshStats::default(), v.engine.now_ms())
        .unwrap();
    for (q, before) in queries.iter().zip(&before) {
        let r = v.report(q);
        assert_eq!(keys(&r), *before, "{q:?}");
        assert_eq!(r.rows[0].counts, counts(5, 9), "{q:?}");
    }
}

#[test]
fn a_partial_index_reports_rows_with_no_refreshed_at() {
    let v = View::new();
    v.sight("s-1", "acct-a", T0, T0 + D);
    v.index(&[
        v.usage("s-1", T0 + H, "model-a", 1, 0),
        v.usage("s-1", T0 + 2 * H, "model-a", 2, 0),
    ]);

    // Before any refresh completes (spec §14).
    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::None));
    assert_eq!(r.total.responses, 2);
    assert_eq!(r.index.refreshed_at_ms, None);
    assert_eq!(r.index.changed_at_ms, Some(v.engine.now_ms()));
    assert_eq!(
        (r.index.oldest_ms, r.index.newest_ms),
        (Some(T0 + H), Some(T0 + 2 * H))
    );
    assert_eq!(
        (r.index.files, r.index.responses),
        (0, 2),
        "files are the last completed refresh's; responses are the index's own count"
    );

    let done = v.engine.now_ms() + 1_000;
    v.fx.clock.set(done);
    let stats = RefreshStats {
        files: 2,
        responses: 2,
        skipped_lines: 1,
        unreadable_files: 0,
    };
    v.store.finish_token_refresh(&cc(), &stats, done).unwrap();
    let index = IndexState {
        refreshed_at_ms: Some(done),
        changed_at_ms: Some(done),
        files: 2,
        responses: 2,
        skipped_lines: 1,
        unreadable_files: 0,
        oldest_ms: Some(T0 + H),
        newest_ms: Some(T0 + 2 * H),
    };
    assert_eq!(v.engine.token_index_state(&cc()).unwrap(), index);
    assert_eq!(
        v.report(&v.query(TokenPeriod::Day, TokenGrouping::None))
            .index,
        index
    );
}

#[test]
fn index_state_moves_on_new_evidence_alone() {
    let v = View::new();
    v.index(&[v.usage("s-1", T0 + H, "model-a", 1, 0)]);
    v.store
        .finish_token_refresh(&cc(), &RefreshStats::default(), v.engine.now_ms())
        .unwrap();
    let state = || v.engine.token_index_state(&cc()).unwrap();
    let refreshed = state();
    assert_eq!(refreshed.changed_at_ms, Some(v.engine.now_ms()));

    // A sighting recorded later moves it, with no response added (spec §6.4).
    v.fx.clock.advance_ms(5_000);
    v.sight("s-1", "acct-a", T0, T0 + D);
    let sighted = state();
    assert_eq!(sighted.changed_at_ms, Some(v.engine.now_ms()));
    assert_eq!(
        (sighted.refreshed_at_ms, sighted.responses),
        (refreshed.refreshed_at_ms, refreshed.responses)
    );

    // So does a launch.
    v.fx.clock.advance_ms(5_000);
    v.store
        .open_launch(&cc(), v.b.as_str(), T0, v.engine.now_ms())
        .unwrap();
    let launched = state();
    assert_eq!(launched.changed_at_ms, Some(v.engine.now_ms()));

    // And a change of the default home's login. Its trigger stamps the database's own clock,
    // not the fixture's (spec §4), so the time to expect is the one it recorded.
    v.store.set_active(&cc(), Some(&v.a), Some(0)).unwrap();
    let switched_at = v.store.active_history(&cc()).unwrap().last().unwrap().0;
    assert_eq!(
        state().changed_at_ms,
        launched.changed_at_ms.max(Some(switched_at))
    );
}

#[test]
fn totals_split_by_basis() {
    let v = View::new();
    // a's profile: profile basis.
    v.sight("s-p", "acct-a", T0, T0 + D);
    // The default home while b is its login: timeline basis.
    v.event(T0 - D, "switch", None, Some(&v.b));
    v.sight("s-d", "", T0, T0 + D);
    // Open in the default home and a's profile at once: unattributed.
    v.sight("s-u", "", T0, T0 + D);
    v.sight("s-u", "acct-a", T0, T0 + D);
    let mut stale = v.usage("s-p", T0 + H, "model-a", 100, 0);
    stale.finalized = false;
    v.index(&[
        stale,
        v.usage("s-d", T0 + 2 * H, "model-a", 200, 0),
        v.usage("s-u", T0 + 3 * H, "model-b", 300, 0),
    ]);

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Account));
    assert_eq!(
        keys(&r),
        [
            "day:2026-09-01|account:acct-a",
            "day:2026-09-01|account:acct-b",
            "day:2026-09-01|account:unattributed",
        ]
    );
    assert_eq!(r.rows[0].unfinalized, 1);
    assert_eq!(
        r.total.by_basis,
        [
            (Basis::Profile, 100),
            (Basis::Timeline, 200),
            (Basis::Unattributed, 300)
        ]
    );
    assert_eq!(
        (r.total.counts, r.total.responses, r.total.unfinalized),
        (counts(600, 0), 3, 1)
    );
    // model-a 300 × 1,000 and model-b 300 × 2,000 nano-dollars.
    assert_eq!(
        r.total.cost,
        Some(Cost {
            usd_nanos: 900_000,
            partial: false
        })
    );
}

#[test]
fn without_a_store_the_report_is_empty_and_creates_nothing() {
    let fx = Fx::new();
    let q = TokenQuery {
        provider: cc(),
        period: TokenPeriod::Day,
        grouping: TokenGrouping::Account,
        since_ms: None,
        until_ms: None,
        accounts: None,
        models: None,
        session: None,
        tz: TimeZone::UTC,
    };
    let r = fx.engine.token_report(&q).unwrap();
    assert!(r.rows.is_empty());
    assert_eq!((r.total.responses, r.index), (0, IndexState::default()));
    assert_eq!(
        fx.engine.token_index_state(&cc()).unwrap(),
        IndexState::default()
    );
    assert!(!fx.env.data_dir().join("tagteam.db").exists());
}

/// An engine over `fx` with Claude Code and `fake` registered and `settings`, as the CLI builds
/// one for a Claude Code command.
fn engine_with(fx: &Fx, fake: Arc<FakeAgent>, settings: Settings) -> Engine {
    Engine::new(EngineConfig {
        env: fx.env.clone(),
        registry: ProviderRegistry::new().with(fx.cc.clone()).with(fake),
        vault: Vault::new(Box::new(KeychainVault::new(fx.kc.clone()))),
        oracle: fx.oracle.clone(),
        clock: fx.clock.clone(),
        http: fx.http.clone(),
        default_provider: ProviderId::new(CLAUDE_CODE),
        settings,
        process: fx.process.clone(),
        spawner: fx.spawner.clone(),
        run_shell: RunShell::Outside,
    })
}

fn day_query(provider: ProviderId) -> TokenQuery {
    TokenQuery {
        provider,
        period: TokenPeriod::Day,
        grouping: TokenGrouping::Account,
        since_ms: None,
        until_ms: None,
        accounts: None,
        models: None,
        session: None,
        tz: TimeZone::UTC,
    }
}

#[test]
fn a_provider_without_transcripts_is_refused() {
    let fx = Fx::new();
    let fake = Arc::new(FakeAgent::new().without_transcripts());
    let engine = engine_with(&fx, fake, Settings::default());
    let q = day_query(ProviderId::new(FAKE_AGENT));
    for err in [
        engine.token_report(&q).unwrap_err(),
        engine.token_index_state(&q.provider).unwrap_err(),
    ] {
        assert_eq!(err.kind(), "invalid-input", "{err:?}");
    }
}

#[test]
fn price_overrides_follow_the_report_provider() {
    let ff = FakeFx::new();
    let path = config_path(&ff.fx.env);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Each provider's own table (spec §8.2): fake-1's input at $7 per MTok, where FakeAgent's
    // shipped table has $1, and a Claude Code model the shipped table lacks.
    fs::write(
        &path,
        "[provider.fake-agent.prices.\"fake-1\"]\ninput = 7.0\n\n\
         [provider.claude-code.prices.\"model-a\"]\n\
         input = 1.0\noutput = 2.0\ncache_write_5m = 3.0\ncache_write_1h = 4.0\ncache_read = 0.5\n",
    )
    .unwrap();
    // The settings of a Claude Code command, as the CLI loads them.
    let (settings, warnings) = Settings::load(&ff.fx.env, &cc());
    assert!(warnings.is_empty(), "{warnings:?}");
    let engine = engine_with(&ff.fx, ff.fake.clone(), settings);
    let fake = ProviderId::new(FAKE_AGENT);
    let row = UsageRow {
        key: usage_key(&fake, "msg-1", "req-1"),
        session_id: "s-f".into(),
        project: None,
        at_ms: T0 + H,
        model: "fake-1".into(),
        sidechain: false,
        counts: counts(10, 0),
        speed: None,
        finalized: true,
    };
    engine
        .store()
        .unwrap()
        .commit_token_batch(&fake, &[row], &[], &[], engine.now_ms())
        .unwrap();

    let r = engine.token_report(&day_query(fake)).unwrap();
    assert_eq!(r.price_overrides, ["fake-1"]);
    assert_eq!(r.prices_as_of, ff.fake.price_table().as_of);
    // 10 input tokens at 7,000 nano-dollars each.
    assert_eq!(
        r.total.cost,
        Some(Cost {
            usd_nanos: 70_000,
            partial: false
        })
    );
    let r = engine.token_report(&day_query(cc())).unwrap();
    assert_eq!(r.price_overrides, ["model-a"]);
}
```

- [ ] **Step 2: Run them and see them fail**

Run (without `--locked`, so `Cargo.lock` records the engine's new `jiff` edge; the package is already in the lock file and fetched since Task 2):

```
cargo test -p tagteam-engine --test tokens_view
```

Expected: the test target does not compile, because `tokens/view.rs` is still Task 14's doc line:

```
error[E0432]: unresolved imports `tagteam_engine::tokens::AccountSel`, `tagteam_engine::tokens::ActiveBlock`, `tagteam_engine::tokens::Basis`, `tagteam_engine::tokens::Cost`, ...
```

- [ ] **Step 3: Implement**

The view is about 1,140 lines, so it becomes a directory module. `tokens/mod.rs` is unchanged: `mod view;` now resolves to `view/mod.rs`.

```
mkdir -p crates/tagteam-engine/src/tokens/view
git mv crates/tagteam-engine/src/tokens/view.rs crates/tagteam-engine/src/tokens/view/mod.rs
```

Replace the whole of `crates/tagteam-engine/src/tokens/view/mod.rs` with the public types, the two entry points and `drill`:

```rust
//! The token report (spec §10): the view the TUI renders and `tagteam tokens --json`
//! serializes. Responses are attributed (spec §7) and priced (spec §8.3) here, at query time,
//! so evidence and price overrides recorded later apply to the past.

mod group;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use jiff::civil::Date;
use jiff::tz::TimeZone;
pub use tagteam_core::tokens::{Basis, TokenCounts};
use tagteam_core::tokens::{Prices, day_bounds_ms, month_bounds_ms};
use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::Provider;

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::{Store, StoreError};

/// The period one row covers (spec §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenPeriod {
    Day,
    Month,
    Session,
    Block,
}

/// What a period's rows are split by (spec §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenGrouping {
    None,
    Account,
    Model,
    AccountModel,
}

impl TokenGrouping {
    fn by_account(self) -> bool {
        matches!(self, TokenGrouping::Account | TokenGrouping::AccountModel)
    }

    fn by_model(self) -> bool {
        matches!(self, TokenGrouping::Model | TokenGrouping::AccountModel)
    }
}

/// One entry of an account filter. An id matches its account's rows once it is removed too
/// (spec §10.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AccountSel {
    Account(AccountId),
    Unattributed,
}

/// What `token_report` reports (spec §10).
#[derive(Debug, Clone, PartialEq)]
pub struct TokenQuery {
    pub provider: ProviderId,
    pub period: TokenPeriod,
    pub grouping: TokenGrouping,
    /// Inclusive, epoch ms.
    pub since_ms: Option<i64>,
    /// Exclusive, epoch ms.
    pub until_ms: Option<i64>,
    /// `None`: every account and unattributed.
    pub accounts: Option<Vec<AccountSel>>,
    /// `None`: every model; exact names otherwise.
    pub models: Option<Vec<String>>,
    /// Only this session's responses.
    pub session: Option<String>,
    /// Day and month boundaries; the CLI's default is the system zone.
    pub tz: TimeZone,
}

/// The report (spec §10).
#[derive(Debug, Clone, PartialEq)]
pub struct TokenReport {
    pub query: TokenQuery,
    /// The IANA name of `query.tz`, or "UTC".
    pub time_zone: String,
    pub rows: Vec<TokenRow>,
    pub total: TokenTotal,
    pub index: IndexState,
    /// Models with a response that has no price, by name (spec §8.3).
    pub unpriced_models: Vec<String>,
    pub prices_as_of: &'static str,
    /// Models whose price config changed, by name (spec §8.2).
    pub price_overrides: Vec<String>,
}

/// One row: a period, split as the query's grouping says (spec §10.1).
#[derive(Debug, Clone, PartialEq)]
pub struct TokenRow {
    pub key: RowKey,
    pub period: PeriodKey,
    /// `Some` when grouped by account.
    pub account: Option<RowAccount>,
    /// `Some` when grouped by model.
    pub model: Option<String>,
    /// Every account present in the row, in report order.
    pub accounts: Vec<RowAccount>,
    /// Every model present in the row, by name.
    pub models: Vec<String>,
    pub counts: TokenCounts,
    pub responses: u64,
    /// Responses whose output is a lower bound (spec §2.3).
    pub unfinalized: u64,
    pub cost: Option<Cost>,
}

/// A row's period (spec §10).
#[derive(Debug, Clone, PartialEq)]
pub enum PeriodKey {
    /// A calendar day in the query's zone.
    Day {
        date: Date,
    },
    Month {
        year: i16,
        month: i8,
    },
    /// `first_ms` and `last_ms` span the row's responses inside the query's range.
    Session {
        session_id: String,
        project: Option<String>,
        first_ms: i64,
        last_ms: i64,
    },
    /// `active` for the block that contains now (spec §9).
    Block {
        start_ms: i64,
        end_ms: i64,
        observed: bool,
        active: Option<ActiveBlock>,
    },
}

/// The active block's progress and projection (spec §9).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveBlock {
    pub elapsed_s: i64,
    pub remaining_s: i64,
    pub tokens_per_min: f64,
    pub cost_per_hour: Option<Cost>,
    pub projected_tokens: u64,
    pub projected_cost: Option<Cost>,
}

/// An account as a row names it (spec §10).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RowAccount {
    Account {
        id: AccountId,
        position: u32,
        label: String,
        alias: Option<String>,
    },
    /// An account no longer in the store; its usage stays (spec §4).
    Removed {
        id: AccountId,
    },
    Unattributed,
}

impl RowAccount {
    /// The filter entry that selects this account.
    fn sel(&self) -> AccountSel {
        match self {
            RowAccount::Account { id, .. } | RowAccount::Removed { id } => {
                AccountSel::Account(id.clone())
            }
            RowAccount::Unattributed => AccountSel::Unattributed,
        }
    }

    /// Spec §10.1's order: accounts by position, then removed accounts, then unattributed.
    fn rank(&self) -> (u8, u32, &str) {
        match self {
            RowAccount::Account { id, position, .. } => (0, *position, id.as_str()),
            RowAccount::Removed { id } => (1, 0, id.as_str()),
            RowAccount::Unattributed => (2, 0, ""),
        }
    }
}

/// A row's identity, stable across refreshes: the TUI keeps a selection by it (spec §10). For
/// example `day:2026-10-14|account:<id>|model:claude-opus-5-5`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RowKey(String);

impl RowKey {
    fn new(period: &PeriodKey, account: Option<&RowAccount>, model: Option<&str>) -> Self {
        let mut key = match period {
            PeriodKey::Day { date } => format!("day:{date}"),
            PeriodKey::Month { year, month } => format!("month:{year:04}-{month:02}"),
            PeriodKey::Session { session_id, .. } => format!("session:{session_id}"),
            PeriodKey::Block { start_ms, .. } => format!("block:{start_ms}"),
        };
        if let Some(account) = account {
            key.push_str("|account:");
            key.push_str(match account {
                RowAccount::Account { id, .. } | RowAccount::Removed { id } => id.as_str(),
                RowAccount::Unattributed => "unattributed",
            });
        }
        if let Some(model) = model {
            key.push_str("|model:");
            key.push_str(model);
        }
        RowKey(key)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RowKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// API cost in nano-dollars (spec §8.3). `partial`: some of the responses have no price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cost {
    pub usd_nanos: u64,
    pub partial: bool,
}

/// The report's totals (spec §10): those of its rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenTotal {
    pub counts: TokenCounts,
    pub responses: u64,
    pub unfinalized: u64,
    pub cost: Option<Cost>,
    /// Total tokens by the basis of their attribution (spec §7).
    pub by_basis: [(Basis, u64); 3],
}

impl Default for TokenTotal {
    fn default() -> Self {
        TokenTotal {
            counts: TokenCounts::default(),
            responses: 0,
            unfinalized: 0,
            cost: None,
            by_basis: [
                (Basis::Profile, 0),
                (Basis::Timeline, 0),
                (Basis::Unattributed, 0),
            ],
        }
    }
}

/// How current the index is (spec §10).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexState {
    /// The last completed refresh; `None` while the index is partial (spec §14).
    pub refreshed_at_ms: Option<i64>,
    /// The latest of `token_state.changed_at` and every evidence row's `recorded_at`
    /// (spec §6.4): it moves when a report may have changed.
    pub changed_at_ms: Option<i64>,
    /// As of the last completed refresh.
    pub files: u64,
    /// The index's own count, which every committed batch moves, so a cold index shows its
    /// progress.
    pub responses: u64,
    /// In the last completed refresh.
    pub skipped_lines: u64,
    pub unreadable_files: u64,
    pub oldest_ms: Option<i64>,
    pub newest_ms: Option<i64>,
}

impl TokenQuery {
    /// The query one level down from `row` (spec §10.1), or `None` at a session's per-model
    /// rows. From a day, month or block row: the sessions over its span, filtered to its
    /// account and model when grouped by them. A day or month row counts only what lies in the
    /// range, so its span is clipped to it; a block row counts its whole block. From a session
    /// row: that session over its whole life, by account and model.
    pub fn drill(&self, row: &TokenRow) -> Option<TokenQuery> {
        let clip = |(start, end): (i64, i64)| {
            (
                self.since_ms.map_or(start, |s| s.max(start)),
                self.until_ms.map_or(end, |u| u.min(end)),
            )
        };
        let (since, until) = match &row.period {
            PeriodKey::Session { session_id, .. } => {
                if self.session.is_some() && self.grouping == TokenGrouping::AccountModel {
                    return None;
                }
                return Some(TokenQuery {
                    period: TokenPeriod::Session,
                    grouping: TokenGrouping::AccountModel,
                    since_ms: None,
                    until_ms: None,
                    session: Some(session_id.clone()),
                    ..self.clone()
                });
            }
            PeriodKey::Day { date } => clip(day_bounds_ms(*date, &self.tz)),
            PeriodKey::Month { year, month } => clip(month_bounds_ms(*year, *month, &self.tz)),
            PeriodKey::Block {
                start_ms, end_ms, ..
            } => (*start_ms, *end_ms),
        };
        Some(TokenQuery {
            period: TokenPeriod::Session,
            since_ms: Some(since),
            until_ms: Some(until),
            accounts: match &row.account {
                Some(account) => Some(vec![account.sel()]),
                None => self.accounts.clone(),
            },
            models: match &row.model {
                Some(model) => Some(vec![model.clone()]),
                None => self.models.clone(),
            },
            ..self.clone()
        })
    }
}

impl Engine {
    /// Spec §10: the report `q` asks for, from the index and the evidence as they stand. It
    /// reads only: it never refreshes the index (spec §5) and never creates the store, without
    /// which the report is empty. A block report reports each block that meets the range
    /// whole, and its total is its rows'.
    pub fn token_report(&self, q: &TokenQuery) -> Result<TokenReport, EngineError> {
        let p = provider_with_transcripts(self, &q.provider)?;
        let table = p.price_table();
        // The query provider's own overrides, whichever provider the settings were loaded for
        // (spec §8.2).
        let none = BTreeMap::new();
        let overrides = self.settings().prices.get(&q.provider).unwrap_or(&none);
        let (prices, price_overrides) = Prices::new(table, overrides);
        let mut report = TokenReport {
            query: q.clone(),
            time_zone: q.tz.iana_name().unwrap_or("UTC").to_owned(),
            rows: Vec::new(),
            total: TokenTotal::default(),
            index: IndexState::default(),
            unpriced_models: Vec::new(),
            prices_as_of: table.as_of,
            price_overrides,
        };
        let Some(store) = self.existing_store()? else {
            return Ok(report);
        };
        // Read before the usage, so whatever lands after it moves `changed_at_ms` again.
        report.index = index_state(&store, &q.provider)?;
        // `Some` of an empty list matches nothing (spec §10.1).
        if q.accounts.as_ref().is_some_and(Vec::is_empty)
            || q.models.as_ref().is_some_and(Vec::is_empty)
        {
            return Ok(report);
        }
        let grouped = group::group(q, p.as_ref(), &store, &prices, self.now_ms())?;
        report.rows = grouped.rows;
        report.total = grouped.total;
        report.unpriced_models = grouped.unpriced_models;
        Ok(report)
    }

    /// The `index` part of a report alone (spec §10), for deciding whether to re-query: a few
    /// small reads, and none without a store, which it never creates.
    pub fn token_index_state(&self, provider: &ProviderId) -> Result<IndexState, EngineError> {
        provider_with_transcripts(self, provider)?;
        match self.existing_store()? {
            Some(store) => Ok(index_state(&store, provider)?),
            None => Ok(IndexState::default()),
        }
    }
}

/// `id`'s provider, which must read transcripts (spec §3.2).
fn provider_with_transcripts(
    engine: &Engine,
    id: &ProviderId,
) -> Result<Arc<dyn Provider>, EngineError> {
    let p = engine.provider(id)?;
    if !p.capabilities().transcripts {
        return Err(EngineError::InvalidInput(format!(
            "{} has no transcripts",
            p.display_name()
        )));
    }
    Ok(p)
}

fn index_state(store: &Store, provider: &ProviderId) -> Result<IndexState, StoreError> {
    let state = store.token_state(provider)?;
    let evidence = store.evidence_changed_at(provider)?;
    let (oldest_ms, newest_ms) = store.token_span(provider)?;
    let stats = state.as_ref().map(|s| s.stats).unwrap_or_default();
    Ok(IndexState {
        refreshed_at_ms: state.as_ref().and_then(|s| s.refreshed_at),
        changed_at_ms: state.as_ref().and_then(|s| s.changed_at).max(evidence),
        files: stats.files,
        responses: stats.responses,
        skipped_lines: stats.skipped_lines,
        unreadable_files: stats.unreadable_files,
        oldest_ms,
        newest_ms,
    })
}
```

Create `crates/tagteam-engine/src/tokens/view/group.rs`: one pass resolves each response (model, session, attribution, account slot), blocks are assigned per series when the period is `Block`, a second pass sums the responses that count by `(period, account?, model?)` keys of small indices, and the groups become ordered rows.

```rust
//! The report's rows (spec §10.1): every response the query reads, attributed (spec §7),
//! priced (spec §8.3), filtered, and summed by period, account and model.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};

use jiff::civil::Date;
use jiff::tz::TimeZone;
use tagteam_core::tokens::{
    ActiveChange, BLOCK_MS, BlockSpan, Coverage, Home, Launch, Launches, Prices, Projection,
    Sighting, Timeline, TimelineEvent, assign_blocks, attribute, day_bounds_ms, day_of,
    month_bounds_ms, month_of, observed_blocks, place, project,
};
use tagteam_core::{AccountId, ProviderId, WindowKind};
use tagteam_provider::Provider;

use super::{
    AccountSel, ActiveBlock, Basis, Cost, PeriodKey, RowAccount, RowKey, TokenCounts,
    TokenGrouping, TokenPeriod, TokenQuery, TokenRow, TokenTotal,
};
use crate::store::{AccountRow, EventRow, Store, StoreError, UsageRow};

/// How far before `since` a block report reads (spec §9). A block that straddles `since` is
/// then whole, and the chain of inferred blocks leading to it starts after a quiet gap of five
/// hours, which a day nearly always holds, so a `since` that moves leaves block starts alone.
const BLOCK_LOOKBACK_MS: i64 = 24 * 3_600_000;

/// What `group` gives the report.
pub(super) struct Grouped {
    pub(super) rows: Vec<TokenRow>,
    pub(super) total: TokenTotal,
    pub(super) unpriced_models: Vec<String>,
}

/// The rows and totals of `q` (spec §10.1). `now_ms` decides the active block.
pub(super) fn group(
    q: &TokenQuery,
    p: &dyn Provider,
    store: &Store,
    prices: &Prices,
    now_ms: i64,
) -> Result<Grouped, StoreError> {
    let by_block = q.period == TokenPeriod::Block;
    // A block is a span of its account's whole usage (spec §9), so a block report resolves
    // every response it reads; the filters only choose which of them count.
    let usage = if by_block {
        store.token_usage(
            &q.provider,
            q.since_ms.map(|s| s.saturating_sub(BLOCK_LOOKBACK_MS)),
            q.until_ms.map(|u| u.saturating_add(BLOCK_MS)),
            None,
        )?
    } else {
        store.token_usage(&q.provider, q.since_ms, q.until_ms, q.session.as_deref())?
    };
    let accounts = store.accounts(&q.provider)?;
    let mut evidence = Evidence::load(store, &q.provider, &accounts)?;
    let mut t = Tables {
        slots: Slots::new(&accounts, q.accounts.as_deref()),
        models: Models::new(q.models.as_deref()),
        sessions: Sessions::default(),
        blocks: Vec::new(),
    };
    let mut resolved = Vec::with_capacity(usage.len());
    for (i, row) in usage.iter().enumerate() {
        let model = t.models.intern(&row.model);
        let chosen =
            t.models.allowed[model] && q.session.as_deref().is_none_or(|s| s == row.session_id);
        if !chosen && !by_block {
            continue;
        }
        let session = t.sessions.intern(row, &mut evidence.sightings);
        let (account, basis) = evidence.account_of(&t.sessions.list[session].sightings, row.at_ms);
        let slot = t.slots.of(account);
        let wanted = chosen && t.slots.allowed[slot];
        if wanted || by_block {
            resolved.push(Resolved {
                row: i,
                model,
                session,
                slot,
                basis,
                wanted,
            });
        }
    }
    let block_of = if by_block {
        let (blocks, block_of) = series_blocks(p, store, &t.slots, &usage, &resolved)?;
        t.blocks = blocks;
        block_of
    } else {
        Vec::new()
    };
    let grouping = effective_grouping(q.period, q.grouping);
    let mut calendar = Calendar {
        tz: &q.tz,
        months: q.period == TokenPeriod::Month,
        last: None,
    };
    let mut groups: HashMap<GroupKey, Acc> = HashMap::new();
    let mut total = Acc::new();
    let mut by_basis = [0u64; 3];
    for (i, r) in resolved.iter().enumerate() {
        if !r.wanted {
            continue;
        }
        let row = &usage[r.row];
        let period = match q.period {
            TokenPeriod::Day | TokenPeriod::Month => calendar.period(row.at_ms),
            TokenPeriod::Session => Period::Session(r.session),
            TokenPeriod::Block => {
                let b = block_of[i];
                if !meets_range(&t.blocks[b].span, q) {
                    continue;
                }
                t.blocks[b].first_ms = t.blocks[b].first_ms.min(row.at_ms);
                Period::Block(b)
            }
        };
        let cost = prices.cost_nanos(&row.model, &row.counts, row.speed.as_deref());
        if cost.is_none() {
            t.models.unpriced[r.model] = true;
        }
        let key = GroupKey {
            period,
            slot: grouping.by_account().then_some(r.slot),
            model: grouping.by_model().then_some(r.model),
        };
        groups
            .entry(key)
            .or_insert_with(Acc::new)
            .add(row, cost, r.slot, r.model);
        total.add(row, cost, r.slot, r.model);
        let basis = &mut by_basis[basis_index(r.basis)];
        *basis = basis.saturating_add(row.counts.total());
        let session = &mut t.sessions.list[r.session];
        session.last_ms = session.last_ms.max(row.at_ms);
    }
    let mut unpriced_models: Vec<String> = t
        .models
        .names
        .iter()
        .zip(&t.models.unpriced)
        .filter(|(_, unpriced)| **unpriced)
        .map(|(name, _)| name.clone())
        .collect();
    unpriced_models.sort();
    Ok(Grouped {
        rows: t.rows(groups, now_ms),
        total: TokenTotal {
            counts: total.counts,
            responses: total.responses,
            unfinalized: total.unfinalized,
            cost: total.cost(),
            by_basis: [
                (Basis::Profile, by_basis[0]),
                (Basis::Timeline, by_basis[1]),
                (Basis::Unattributed, by_basis[2]),
            ],
        },
        unpriced_models,
    })
}

/// Spec §10.1: blocks are per account, so a block report is always grouped by account.
fn effective_grouping(period: TokenPeriod, grouping: TokenGrouping) -> TokenGrouping {
    match (period, grouping) {
        (TokenPeriod::Block, TokenGrouping::None) => TokenGrouping::Account,
        (TokenPeriod::Block, TokenGrouping::Model) => TokenGrouping::AccountModel,
        (_, grouping) => grouping,
    }
}

fn meets_range(span: &BlockSpan, q: &TokenQuery) -> bool {
    q.since_ms.is_none_or(|s| span.end_ms > s) && q.until_ms.is_none_or(|u| span.start_ms < u)
}

fn basis_index(basis: Basis) -> usize {
    match basis {
        Basis::Profile => 0,
        Basis::Timeline => 1,
        Basis::Unattributed => 2,
    }
}

/// What places a response (spec §7), read once per report.
struct Evidence {
    /// By session; each session's are taken out when the report first meets it.
    sightings: HashMap<String, Vec<Sighting>>,
    launches: Launches,
    coverage: Coverage,
    timeline: Timeline,
}

impl Evidence {
    fn load(
        store: &Store,
        provider: &ProviderId,
        accounts: &[AccountRow],
    ) -> Result<Self, StoreError> {
        let mut sightings: HashMap<String, Vec<Sighting>> = HashMap::new();
        for s in store.sightings(provider)? {
            let home = if s.home.is_empty() {
                Home::Default
            } else {
                Home::Profile(AccountId::from_string(s.home))
            };
            sightings.entry(s.session_id).or_default().push(Sighting {
                home,
                first_ms: s.first_at,
                last_ms: s.last_at,
            });
        }
        let launches = Launches::new(
            store
                .launches(provider)?
                .into_iter()
                .map(|l| Launch {
                    account: AccountId::from_string(l.account_id),
                    start_ms: l.started_at,
                    end_ms: l.ended_at,
                })
                .collect(),
        );
        // No epoch row (after a full purge, before the next refresh completes): every time is
        // before it, and coverage is never complete.
        let epoch = store.token_epoch()?;
        let epoch_ms = epoch.map_or(i64::MAX, |e| e.at);
        let events: Vec<TimelineEvent> = store
            .events()?
            .into_iter()
            .filter(|e| &e.provider == provider)
            .filter_map(timeline_event)
            .collect();
        let coverage = Coverage {
            epoch_ms,
            seeded: epoch.is_some_and(|e| e.seeded),
            last_pre_epoch_remove_ms: events
                .iter()
                .filter_map(|e| match e {
                    TimelineEvent::Remove { at_ms, .. } if *at_ms < epoch_ms => Some(*at_ms),
                    _ => None,
                })
                .max(),
        };
        let history: Vec<ActiveChange> = store
            .active_history(provider)?
            .into_iter()
            .map(|(at_ms, account)| ActiveChange {
                at_ms,
                account: account.map(AccountId::from_string),
            })
            .collect();
        let added_at: BTreeMap<AccountId, i64> = accounts
            .iter()
            .map(|a| (a.id.clone(), a.added_at))
            .collect();
        Ok(Evidence {
            sightings,
            launches,
            coverage,
            timeline: Timeline::build(&events, &history, epoch_ms, &added_at),
        })
    }

    /// The account a response at `t` of a session with `sightings` went to, and on what basis
    /// (spec §7).
    fn account_of(&self, sightings: &[Sighting], t: i64) -> (Option<AccountId>, Basis) {
        let placement = place(sightings, &self.launches, &self.coverage, t);
        attribute(&placement, &self.timeline, t)
    }
}

/// An `events` row as the default home's timeline reads it (spec §7.3): `switch` and
/// `switch-recovered` move the login to `to_id`, `add` and `import` start an account, and
/// `remove`, which records the account as `from_id`, ends it.
fn timeline_event(e: EventRow) -> Option<TimelineEvent> {
    match e.kind.as_str() {
        "switch" | "switch-recovered" => Some(TimelineEvent::Switch {
            at_ms: e.at,
            from: e.from_id,
            to: e.to_id?,
        }),
        "add" | "import" => Some(TimelineEvent::Add {
            at_ms: e.at,
            account: e.to_id?,
        }),
        "remove" => Some(TimelineEvent::Remove {
            at_ms: e.at,
            account: e.from_id?,
        }),
        _ => None,
    }
}

/// One response as the report resolved it: indices into its tables, and its attribution.
#[derive(Clone, Copy)]
struct Resolved {
    row: usize,
    model: usize,
    session: usize,
    slot: usize,
    basis: Basis,
    /// It passes the query's filters.
    wanted: bool,
}

/// What the indices of a report's rows point at.
struct Tables<'q> {
    slots: Slots<'q>,
    models: Models<'q>,
    sessions: Sessions,
    blocks: Vec<Block>,
}

/// The accounts a report names: the provider's, then each removed account a response went
/// to, and the unattributed bucket.
struct Slots<'q> {
    accounts: Vec<RowAccount>,
    by_id: HashMap<AccountId, usize>,
    unattributed: usize,
    filter: Option<&'q [AccountSel]>,
    /// Whether the query's account filter selects it.
    allowed: Vec<bool>,
}

impl<'q> Slots<'q> {
    fn new(rows: &[AccountRow], filter: Option<&'q [AccountSel]>) -> Self {
        let mut slots = Slots {
            accounts: Vec::new(),
            by_id: HashMap::new(),
            unattributed: 0,
            filter,
            allowed: Vec::new(),
        };
        for row in rows {
            slots.push(RowAccount::Account {
                id: row.id.clone(),
                position: row.position,
                label: row.label.clone(),
                alias: row.alias.clone(),
            });
        }
        slots.unattributed = slots.push(RowAccount::Unattributed);
        slots
    }

    fn push(&mut self, account: RowAccount) -> usize {
        let slot = self.accounts.len();
        self.allowed
            .push(self.filter.is_none_or(|f| f.contains(&account.sel())));
        if let RowAccount::Account { id, .. } | RowAccount::Removed { id } = &account {
            self.by_id.insert(id.clone(), slot);
        }
        self.accounts.push(account);
        slot
    }

    /// The slot of a response attributed to `account`: an id the store no longer holds is a
    /// removed account (spec §10).
    fn of(&mut self, account: Option<AccountId>) -> usize {
        let Some(id) = account else {
            return self.unattributed;
        };
        match self.by_id.get(&id) {
            Some(&slot) => slot,
            None => self.push(RowAccount::Removed { id }),
        }
    }
}

/// The models a report's responses name.
struct Models<'q> {
    names: Vec<String>,
    by_name: HashMap<String, usize>,
    filter: Option<&'q [String]>,
    /// Whether the query's model filter selects it.
    allowed: Vec<bool>,
    /// Whether a response of it that counts has no price (spec §8.3).
    unpriced: Vec<bool>,
}

impl<'q> Models<'q> {
    fn new(filter: Option<&'q [String]>) -> Self {
        Models {
            names: Vec::new(),
            by_name: HashMap::new(),
            filter,
            allowed: Vec::new(),
            unpriced: Vec::new(),
        }
    }

    fn intern(&mut self, name: &str) -> usize {
        if let Some(&model) = self.by_name.get(name) {
            return model;
        }
        let model = self.names.len();
        self.names.push(name.to_owned());
        self.by_name.insert(name.to_owned(), model);
        self.allowed
            .push(self.filter.is_none_or(|f| f.iter().any(|m| m == name)));
        self.unpriced.push(false);
        model
    }
}

/// The sessions a report's responses belong to.
#[derive(Default)]
struct Sessions {
    list: Vec<Session>,
    by_id: HashMap<String, usize>,
}

struct Session {
    id: String,
    project: Option<String>,
    sightings: Vec<Sighting>,
    /// Its latest response that counts, which orders session rows (spec §10.1).
    last_ms: i64,
}

impl Sessions {
    fn intern(&mut self, row: &UsageRow, sightings: &mut HashMap<String, Vec<Sighting>>) -> usize {
        if let Some(&session) = self.by_id.get(row.session_id.as_str()) {
            return session;
        }
        let session = self.list.len();
        self.list.push(Session {
            id: row.session_id.clone(),
            project: row.project.clone(),
            sightings: sightings
                .remove(row.session_id.as_str())
                .unwrap_or_default(),
            last_ms: i64::MIN,
        });
        self.by_id.insert(row.session_id.clone(), session);
        session
    }
}

/// One block of one series (spec §9).
struct Block {
    span: BlockSpan,
    /// Its first response that counts: where the active block's burn rate starts.
    first_ms: i64,
}

/// Spec §9: each series' blocks, an account's from its observed resets and its responses,
/// a removed account's and the unattributed series' from their responses alone. Returns the
/// blocks and, for each resolved response, its block. A series with no response that counts
/// gets none.
fn series_blocks(
    p: &dyn Provider,
    store: &Store,
    slots: &Slots<'_>,
    usage: &[UsageRow],
    resolved: &[Resolved],
) -> Result<(Vec<Block>, Vec<usize>), StoreError> {
    let mut series: Vec<Vec<usize>> = vec![Vec::new(); slots.accounts.len()];
    for (i, r) in resolved.iter().enumerate() {
        series[r.slot].push(i);
    }
    let mut short: HashMap<String, bool> = HashMap::new();
    let mut blocks = Vec::new();
    let mut block_of = vec![0; resolved.len()];
    for (slot, entries) in series.iter().enumerate() {
        if !entries.iter().any(|&i| resolved[i].wanted) {
            continue;
        }
        let times: Vec<i64> = entries
            .iter()
            .map(|&i| usage[resolved[i].row].at_ms)
            .collect();
        let observed = match &slots.accounts[slot] {
            RowAccount::Account { id, .. } => {
                observed_blocks(&short_resets(p, store, id, times[0], &mut short)?)
            }
            // A removed account's samples went with it (core §6.1).
            RowAccount::Removed { .. } | RowAccount::Unattributed => Vec::new(),
        };
        let (spans, of) = assign_blocks(&observed, &times);
        let base = blocks.len();
        blocks.extend(spans.into_iter().map(|span| Block {
            span,
            first_ms: i64::MAX,
        }));
        for (&i, b) in entries.iter().zip(of) {
            block_of[i] = base + b;
        }
    }
    Ok((blocks, block_of))
}

/// The resets, in ms, of `id`'s samples of the provider's short window (spec §9) taken from
/// two blocks before `from_ms` on: early enough for every block that can hold a response at
/// or after it, and for the block before that, which may move its start.
fn short_resets(
    p: &dyn Provider,
    store: &Store,
    id: &AccountId,
    from_ms: i64,
    short: &mut HashMap<String, bool>,
) -> Result<Vec<i64>, StoreError> {
    let since_s = from_ms.saturating_sub(2 * BLOCK_MS).div_euclid(1000);
    let mut resets = Vec::new();
    for (key, sample) in store.usage_samples(id, None, since_s)? {
        let is_short = *short.entry(key).or_insert_with_key(|key| {
            p.describe_window(key)
                .is_some_and(|w| w.kind == WindowKind::Short)
        });
        if let Some(reset) = sample.resets_at.filter(|_| is_short) {
            resets.push(reset.saturating_mul(1000));
        }
    }
    resets.sort_unstable();
    resets.dedup();
    Ok(resets)
}

/// A row's period, by the report's own indices.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Period {
    Day(Date),
    Month(i16, i8),
    Session(usize),
    Block(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GroupKey {
    period: Period,
    slot: Option<usize>,
    model: Option<usize>,
}

/// The calendar day or month of a time in the query's zone (spec §10.1). Responses come in
/// time order, so the bounds of the last period found answer most of them without a zone
/// conversion.
struct Calendar<'q> {
    tz: &'q TimeZone,
    months: bool,
    last: Option<(i64, i64, Period)>,
}

impl Calendar<'_> {
    fn period(&mut self, t: i64) -> Period {
        if let Some((_, _, period)) = self.last.filter(|&(start, end, _)| start <= t && t < end) {
            return period;
        }
        let (period, (start, end)) = if self.months {
            let (year, month) = month_of(t, self.tz);
            (
                Period::Month(year, month),
                month_bounds_ms(year, month, self.tz),
            )
        } else {
            let date = day_of(t, self.tz);
            (Period::Day(date), day_bounds_ms(date, self.tz))
        };
        self.last = Some((start, end, period));
        period
    }
}

/// One row's sums while the report groups.
struct Acc {
    counts: TokenCounts,
    responses: u64,
    unfinalized: u64,
    cost_nanos: u64,
    unpriced: u64,
    first_ms: i64,
    last_ms: i64,
    slots: Vec<usize>,
    models: Vec<usize>,
}

impl Acc {
    fn new() -> Self {
        Acc {
            counts: TokenCounts::default(),
            responses: 0,
            unfinalized: 0,
            cost_nanos: 0,
            unpriced: 0,
            first_ms: i64::MAX,
            last_ms: i64::MIN,
            slots: Vec::new(),
            models: Vec::new(),
        }
    }

    fn add(&mut self, row: &UsageRow, cost: Option<u64>, slot: usize, model: usize) {
        self.counts.add(&row.counts);
        self.responses += 1;
        self.unfinalized += u64::from(!row.finalized);
        match cost {
            Some(nanos) => self.cost_nanos = self.cost_nanos.saturating_add(nanos),
            None => self.unpriced += 1,
        }
        self.first_ms = self.first_ms.min(row.at_ms);
        self.last_ms = self.last_ms.max(row.at_ms);
        if !self.slots.contains(&slot) {
            self.slots.push(slot);
        }
        if !self.models.contains(&model) {
            self.models.push(model);
        }
    }

    /// Spec §8.3: no cost when every response is unpriced, a partial one when some are.
    fn cost(&self) -> Option<Cost> {
        (self.unpriced < self.responses).then_some(Cost {
            usd_nanos: self.cost_nanos,
            partial: self.unpriced > 0,
        })
    }
}

impl Tables<'_> {
    /// The groups as rows, newest first, and within a period by account, then model
    /// (spec §10.1).
    fn rows(&self, groups: HashMap<GroupKey, Acc>, now_ms: i64) -> Vec<TokenRow> {
        let slot_ord = ordinals(&self.slots.accounts, |a, b| a.rank().cmp(&b.rank()));
        let model_ord = ordinals(&self.models.names, Ord::cmp);
        let order = |k: &GroupKey| {
            let (newest, tie) = match k.period {
                Period::Day(d) => (
                    i64::from(d.year()) * 10_000 + i64::from(d.month()) * 100 + i64::from(d.day()),
                    "",
                ),
                Period::Month(year, month) => (i64::from(year) * 100 + i64::from(month), ""),
                // Ties keep each session's rows together.
                Period::Session(s) => (
                    self.sessions.list[s].last_ms,
                    self.sessions.list[s].id.as_str(),
                ),
                Period::Block(b) => (self.blocks[b].span.start_ms, ""),
            };
            (
                Reverse(newest),
                tie,
                k.slot.map_or(0, |s| slot_ord[s]),
                k.model.map_or(0, |m| model_ord[m]),
            )
        };
        let mut groups: Vec<(GroupKey, Acc)> = groups.into_iter().collect();
        groups.sort_by(|(a, _), (b, _)| order(a).cmp(&order(b)));
        groups
            .into_iter()
            .map(|(k, acc)| self.row(k, acc, &slot_ord, now_ms))
            .collect()
    }

    fn row(&self, k: GroupKey, mut acc: Acc, slot_ord: &[usize], now_ms: i64) -> TokenRow {
        let cost = acc.cost();
        let period = match k.period {
            Period::Day(date) => PeriodKey::Day { date },
            Period::Month(year, month) => PeriodKey::Month { year, month },
            Period::Session(s) => PeriodKey::Session {
                session_id: self.sessions.list[s].id.clone(),
                project: self.sessions.list[s].project.clone(),
                first_ms: acc.first_ms,
                last_ms: acc.last_ms,
            },
            Period::Block(b) => {
                let block = &self.blocks[b];
                PeriodKey::Block {
                    start_ms: block.span.start_ms,
                    end_ms: block.span.end_ms,
                    observed: block.span.observed,
                    active: project(
                        &block.span,
                        block.first_ms,
                        now_ms,
                        acc.counts.total(),
                        cost.map(|c| c.usd_nanos),
                    )
                    .map(|p| active_block(&p, cost)),
                }
            }
        };
        let account = k.slot.map(|s| &self.slots.accounts[s]);
        let model = k.model.map(|m| self.models.names[m].as_str());
        acc.slots.sort_by_key(|&s| slot_ord[s]);
        let mut models: Vec<String> = acc
            .models
            .iter()
            .map(|&m| self.models.names[m].clone())
            .collect();
        models.sort();
        TokenRow {
            key: RowKey::new(&period, account, model),
            period,
            account: account.cloned(),
            model: model.map(str::to_owned),
            accounts: acc
                .slots
                .iter()
                .map(|&s| self.slots.accounts[s].clone())
                .collect(),
            models,
            counts: acc.counts,
            responses: acc.responses,
            unfinalized: acc.unfinalized,
            cost,
        }
    }
}

/// Each item's place in the order `cmp` gives.
fn ordinals<T>(items: &[T], cmp: impl Fn(&T, &T) -> std::cmp::Ordering) -> Vec<usize> {
    let mut by: Vec<usize> = (0..items.len()).collect();
    by.sort_by(|&a, &b| cmp(&items[a], &items[b]));
    let mut ord = vec![0; items.len()];
    for (o, i) in by.into_iter().enumerate() {
        ord[i] = o;
    }
    ord
}

/// The active block's figures (spec §9), its costs partial as the row's is.
fn active_block(p: &Projection, cost: Option<Cost>) -> ActiveBlock {
    let partial = cost.is_some_and(|c| c.partial);
    let priced = |nanos: Option<u64>| nanos.map(|usd_nanos| Cost { usd_nanos, partial });
    ActiveBlock {
        elapsed_s: p.elapsed_s,
        remaining_s: p.remaining_s,
        tokens_per_min: p.tokens_per_min,
        cost_per_hour: priced(p.cost_nanos_per_hour),
        projected_tokens: p.projected_tokens,
        projected_cost: priced(p.projected_cost_nanos),
    }
}
```

How it holds spec §10.1, for the reviewer:
- **Attribution** is `place` then `attribute` per response, with the session's sightings found once per session (`Sessions::intern` moves them out of the evidence map). An id the store no longer holds becomes `RowAccount::Removed`.
- **Filters before grouping.** The model and session filters are checked before attribution; the account filter right after it, on the slot. `Some(vec![])` returns before any usage is read.
- **Blocks** come from each account's whole usage in the read window (the read goes 24 h before `since` and 5 h past `until`), and the filters only choose which responses count. Otherwise a `--model` filter would move block boundaries. A block that meets `[since, until)` is reported whole, and the total is the rows' sum. Only `WindowKind::Short` samples give observed resets; a removed account's samples were cascaded away (core §6.1), so its blocks are all inferred.
- **Order.** Newest period first (a session by its latest response that counts, ties by id so a split session's rows stay together; a block by its start), then accounts by position, removed accounts, unattributed, then models by name.
- **Drill.** A day or month span is clipped to the query's range, so the drilled totals equal the row's. A block span is not clipped, because the row counts the whole block. A session drill keeps the query's account and model filters and drops the range.
- **Cost** is per response, summed in nano-dollars. A row with every response unpriced has `None`, and one with some unpriced is `partial`. `unpriced_models` lists each model with a counted response that has no price, including a `fast` response of a model without a fast card.
- **Prices** are the report provider's shipped table merged with that provider's own overrides, `settings().prices.get(&q.provider)` (none when it has no table). They never come from the provider the settings were loaded for.
- **Performance** (spec §1.1, 200 ms for 30 days at about 130k rows): one store read each for usage, sightings, launches, events, history and accounts. There is one hash lookup per response for each of model, session and account, a cached day or month boundary instead of a zone conversion per response, and `Copy` group keys. The rest is the store's per-row `String`s, measured in Task 23.

- [ ] **Step 4: Run them and see them pass**

```
cargo test -p tagteam-engine --locked --test tokens_view
```

Expected: `test result: ok. 16 passed; 0 failed`.

- [ ] **Step 5: Lint**

```
cargo fmt --all --check
cargo clippy -p tagteam-engine --all-targets --locked -- -D warnings
```

Expected: no diff and no warnings.

- [ ] **Step 6: Commit**

```
git add Cargo.lock crates/tagteam-engine/Cargo.toml crates/tagteam-engine/src/tokens/view crates/tagteam-engine/tests/tokens_view.rs
git commit -m "Add the token report view"
```

(`git mv` already staged the removal of `tokens/view.rs`; naming that path again would fail, since it is in neither the tree nor the index.)

#### Notes for the controller

1. **Contract additions** (all inside spec §10): `RowKey::as_str()` and `Display` (spec §10's `RowKey(String)` keeps its field private, and Task 19 and the TUI must read it), `impl Default for TokenTotal`, and `Debug` on every type. `TokenQuery` and `TokenReport` derive `PartialEq`, which needs `jiff::tz::TimeZone: PartialEq`. jiff 0.2 implements it; if the pinned version did not, compare `tz.iana_name()` in a hand-written impl. No serde derives.
2. **Depends on** adds 2, 8 and 14 to the outline's list. The view needs `jiff`, a Claude Code provider with `transcripts: true` (the tests use it), and Task 14's `tokens/view.rs`. All of them precede batch F already.
3. **`jiff` in `tagteam-engine`.** Task 2 adds it to the workspace and to `tagteam-core` only, so this task adds it to the engine. Task 19 needs `jiff.workspace = true` in `crates/tagteam/Cargo.toml` too, for `--tz` and the date output.
4. **Task 12's re-exports.** These tests import `usage_key`, `UsageRow`, `SightingRow`, `SightingSource` and `RefreshStats` from `tagteam_engine::store`, so Task 12's `pub use tokens::{…}` must name them. The view also uses `TokenStateRow`, `EpochRow` and `LaunchRow` through the store's methods only.
5. **The epoch and seeding.** Task 11 opens the engine's store with `open_seeded`, dating the epoch with the engine clock. `View::new` therefore sets the clock before the store opens and asserts the epoch it gets. The view reads `EpochRow.seeded` either way. No test here depends on it: every attributed response is sighted, and every unattributed one is sighted in two homes or covered by an open launch. The stub run passed with `seeded` both true and false.
6. **Rulings to record in the PR** (each within spec §§7, 9, 10):
   - `switch-recovered` events count as switches and `import` as adds in the default home's timeline. Spec §7.3 names `switch`, `add` and `remove`, but recovery commits the same change of login, and import creates the account the same way. `remove` takes its account from `from_id`, which is what `lifecycle.rs` records.
   - Block reads look back 24 h before `since`, where the outline said 5 h, and 5 h past `until`. Five hours makes a straddling block whole but can re-anchor the chain of inferred blocks before it as `since` moves. A day nearly always contains the 5-hour gap that resets the chain.
   - Blocks are spans of the account's whole usage, and the filters choose responses, not spans. A block report's total is its rows' sum, which can include responses just before `since` in a straddling block.
   - Drill clips day and month spans to the query's range but not block spans, and a session drill keeps the query's filters.
   - `IndexState.files` comes from `token_state`'s last completed refresh, as spec §10 says. The outline said to count `token_files`, which would read about 2.5k rows on every TUI or daemon poll. `IndexState.responses` is `token_state`'s live count, which every committed batch moves, so a TUI can show a cold index's progress.
   - `token_index_state` refuses a provider without transcripts (`invalid-input`), as `token_report` does.
   - Removed accounts sort among themselves by id.
   - A report prices with its own provider's overrides from `Settings.prices`, which holds every provider's tables, not the overrides of the provider the CLI loaded settings for (`price_overrides_follow_the_report_provider`).
7. **For Task 19's JSON.** `RowAccount::Account.label` is `AccountRow.label`, which for Claude Code is the email that spec §11.2's `email` key wants, and `position` is `number`. `Cost.usd_nanos` becomes `costUsd` (divide by 1e9) and `partial` becomes `costPartial`.
8. **What was verified.** The three files above were compiled, run (every test but the last two passed), clippy-checked with `-D warnings` and rustfmt-checked in a scratch workspace. That workspace stubs the contract's signatures: `tagteam_core::tokens` with spec-faithful bodies, the store API in memory, `Fx` and a minimal `jiff`. Four mutations (an unclipped drill span, removed accounts ranked with current ones, no block lookback, the account filter ignored) were each caught by at least one test. Nothing was run against the real crates, the real SQLite store or the real `jiff`, since none exist yet. `a_provider_without_transcripts_is_refused` and `price_overrides_follow_the_report_provider` were formatted but not compiled. They use `EngineConfig`, the registry, `FakeAgent`, `Settings::load` and `config_path`, which the stubs lack, and `engine_with` mirrors `FakeFx::new` field for field. A stub-only test checked the per-provider lookup instead: settings that hold both providers' tables, a FakeAgent report taking FakeAgent's partial override (70,000 nano-dollars for 10 input tokens at $7), a Claude Code report taking Claude Code's, and a provider with no table getting none. The 200 ms budget is Task 23's to measure. If it misses, the cost will be the per-row `String`s of `Store::token_usage`, not the view.
9. `months_follow_the_query_zone` needs the system zone database for `Europe/Brussels`, as Task 6's tests do. A minimal Linux container without `tzdata` fails both.

---

### Task 19: `tagteam tokens`

**Depends on:** 15, 18 (and through them 6, 12, 13, 16)
**Files:**
- Create: `crates/tagteam/src/tokens_cmd.rs` (flag parsing, the refresh with its progress line, the notices, and the table, `--json` and `--csv` renderings; its own unit tests)
- Modify: `crates/tagteam/Cargo.toml` (`jiff.workspace = true` under `[dependencies]`)
- Modify: `Cargo.lock` (`tagteam` gains its `jiff` edge; jiff is already locked, nothing is fetched)
- Modify: `crates/tagteam/src/cli.rs` (the `Tokens` command, `TokensPeriod`, `TokensArgs`, `GroupingParser`; the `touches_keychain` doc line; a parse test)
- Modify: `crates/tagteam/src/app.rs` (imports, `command_name`, dispatch, `App::tokens`; the late-notice test's cases)
- Modify: `crates/tagteam/src/lib.rs` (`mod tokens_cmd;`)
- Modify: `crates/tagteam/src/history.rs` (`csv_field` becomes `pub(crate)`, for `tokens --csv`)
- Modify: `crates/tagteam/tests/common/mod.rs` (fixture transcripts: `Turn`, `turn`, `cc_timestamp`, `assistant_line`, `write_transcript`)
- Test: `crates/tagteam/tests/tokens_cli.rs` (new), `crates/tagteam/src/tokens_cmd.rs` (`mod tests`), `crates/tagteam/src/cli.rs` (`mod tests`), `crates/tagteam/src/app.rs` (`mod tests`), and the existing `crates/tagteam/tests/completions.rs` as a regression check

**Interfaces:**
- Consumes:
  ```rust
  // tagteam_core (Tasks 6, 8)
  pub fn day_bounds_ms(date: jiff::civil::Date, tz: &jiff::tz::TimeZone) -> (i64, i64);   // tagteam_core::tokens
  pub fn month_bounds_ms(year: i16, month: i8, tz: &jiff::tz::TimeZone) -> (i64, i64);
  pub fn month_of(ms: i64, tz: &jiff::tz::TimeZone) -> (i16, i8);
  pub fn parse_iso8601_ms(s: &str) -> Option<i64>;                                         // tagteam_core::time
  // tagteam_provider (Task 7)
  pub struct Capabilities { /* existing */ pub transcripts: bool }
  // Store (Task 12), tagteam_engine::store
  pub fn token_models(&self, provider: &ProviderId) -> Result<Vec<String>, StoreError>;     // distinct, sorted
  pub fn record_sighting(&self, provider: &ProviderId, s: &SightingRow, min_gap_ms: i64) -> Result<bool, StoreError>; // tests
  pub struct SightingRow { pub session_id: String, pub home: String, pub first_at: i64, pub last_at: i64, pub source: SightingSource, pub recorded_at: i64 }
  pub enum SightingSource { Run, Scan, Statusline }
  // Refresh (Task 15), tagteam_engine::tokens
  pub enum Wait { Wait, Try }
  pub enum RefreshOutcome { Done(IndexReport), Busy }
  pub struct IndexProgress { pub files_done: u64, pub files_total: u64, pub bytes_done: u64, pub bytes_total: u64 }
  impl Engine { pub fn refresh_tokens(&self, provider: &ProviderId, wait: Wait, budget: Option<Duration>, cancel: &Cancel, progress: &mut dyn FnMut(&IndexProgress)) -> Result<RefreshOutcome, EngineError>; }
  // View (Task 18), tagteam_engine::tokens: spec §10's types as task-18.md writes them, every one
  // deriving Debug, Clone and PartialEq (TokenPeriod and TokenGrouping also Copy), and
  impl RowKey { pub fn as_str(&self) -> &str; }
  impl Engine { pub fn token_report(&self, q: &TokenQuery) -> Result<TokenReport, EngineError>; }
  // Settings (Task 13): `[provider.claude-code.prices."<model>"]` tables in config.toml
  // Existing: tagteam_core::validate::{parse_account_ref, AccountRefInput}; Engine::resolve and
  // candidates (core §10.4); Store::insert_account with NewAccount and tagteam_provider::Identity
  // (tests); tagteam_fake::FAKE_AGENT (tests)
  ```
- Produces:
  ```rust
  // the `tagteam tokens [day|month|session|block] [ACCOUNT...] [flags]` command (spec §11)
  pub enum Command { /* existing */ Tokens { period: Option<Box<TokensPeriod>>, args: TokensArgs } }   // tagteam::cli
  pub enum TokensPeriod { Day(TokensArgs), Month(TokensArgs), Session(TokensArgs), Block(TokensArgs) }
  pub struct TokensArgs { pub accounts: Vec<String>, pub unattributed: bool, pub model: Vec<String>, pub since: Option<String>, pub until: Option<String>, pub by: Option<String>, pub tz: Option<String>, pub csv: bool, pub no_refresh: bool }
  pub(crate) fn csv_field(s: &str) -> String;                                              // crate::history
  // crate::tokens_cmd, pure and unit-tested
  pub(crate) fn default_since_ms(period: &TokenPeriod, now_ms: i64, tz: &TimeZone) -> i64;
  pub(crate) fn provider_for(flag: Option<&ProviderId>, default: &ProviderId, named: &[(u32, ProviderId)]) -> Result<ProviderId, String>;
  // crates/tagteam/tests/common/mod.rs, for Task 22's tests
  pub struct Turn<'a> { pub model: &'a str, pub at_ms: i64, pub counts: [u64; 5], pub final_line: bool }
  pub fn turn(model: &str, at_ms: i64, counts: [u64; 5]) -> Turn<'_>;
  pub fn cc_timestamp(at_ms: i64) -> String;
  pub fn assistant_line(session: &str, message_id: &str, request_id: &str, t: &Turn, text: &str) -> String;
  pub fn write_transcript(root: &Path, session: &str, turns: &[Turn]) -> PathBuf;          // <home>/.claude/projects/-w/<session>.jsonl
  ```

The CLI renders spec §10's view and nothing else. Spec §11.2 names the top-level `--json` keys and a row's; it leaves the shape of a row's `period`, of `total` and of `index` open, so this task fixes them, following the existing conventions (`_ms` fields become ISO 8601 times without the suffix, as spec §12's `token-index` event spells `IndexState`):

- `period`: `{kind: "day", date}`, `{kind: "month", month: "YYYY-MM"}`, `{kind: "session", sessionId, project, first, last}`, or `{kind: "block", start, end, observed, active}`, `active` being null or `{elapsedSeconds, remainingSeconds, tokensPerMinute, costPerHourUsd, projectedTokens, projectedCostUsd}`.
- `total`: `{tokens: {…as a row's…}, responses, unfinalized, costUsd, costPartial, byBasis: {profile, timeline, unattributed}}`.
- `index`: `{refreshedAt, changedAt, files, responses, skippedLines, unreadableFiles, oldest, newest}`, spec §12's event less `provider`.

The table's columns follow spec §11.2, with the period's own columns placed where they read best: `DAY` or `MONTH`; `SESSION FIRST LAST` before the account and `PROJECT` last; `START END` before the account and a note column last (`observed`, and the active block's time left, burn rate and projection). The account column is always there: the row's account when grouped by account (always, for blocks), else every account the row holds. `MODEL` shows only when grouped by model.

- [ ] **Step 1: Write the failing tests**

The CLI tests need indexed data, written the way Claude Code writes it. Append these helpers to the end of `crates/tagteam/tests/common/mod.rs` (the file already imports `fs`, `Path`, `PathBuf`, `Value`, `json` and `Env`, and allows dead code):

```rust
/// One assistant response of a fixture transcript (token report spec §2.1): its model, its time,
/// its counts (input, output, 5-minute and 1-hour cache writes, cache reads), and whether its
/// line is final. A subagent's response often never gets a final line (spec §2.3).
#[derive(Debug, Clone, Copy)]
pub struct Turn<'a> {
    pub model: &'a str,
    pub at_ms: i64,
    pub counts: [u64; 5],
    pub final_line: bool,
}

/// A finished response of `model` at `at_ms`.
pub fn turn(model: &str, at_ms: i64, counts: [u64; 5]) -> Turn<'_> {
    Turn {
        model,
        at_ms,
        counts,
        final_line: true,
    }
}

/// `at_ms` as Claude Code stamps a transcript line: ISO 8601 UTC, to the millisecond.
pub fn cc_timestamp(at_ms: i64) -> String {
    let seconds = tagteam_core::time::format_iso8601(at_ms.div_euclid(1000));
    format!(
        "{}.{:03}Z",
        seconds.trim_end_matches('Z'),
        at_ms.rem_euclid(1000)
    )
}

/// Claude Code's transcript line for one assistant response (token report spec §2.1–2.2), with
/// `text` as its only content and `/w` as its working directory. A line that is not final is
/// a subagent's streaming snapshot: no `stop_reason`, on a sidechain.
pub fn assistant_line(
    session: &str,
    message_id: &str,
    request_id: &str,
    t: &Turn,
    text: &str,
) -> String {
    let [input, output, write_5m, write_1h, read] = t.counts;
    let stop = if t.final_line {
        json!("end_turn")
    } else {
        Value::Null
    };
    json!({
        "parentUuid": null,
        "isSidechain": !t.final_line,
        "type": "assistant",
        "requestId": request_id,
        "sessionId": session,
        "timestamp": cc_timestamp(t.at_ms),
        "cwd": "/w",
        "message": {
            "id": message_id,
            "type": "message",
            "role": "assistant",
            "model": t.model,
            "content": [{"type": "text", "text": text}],
            "stop_reason": stop,
            "usage": {
                "input_tokens": input,
                "output_tokens": output,
                "cache_creation_input_tokens": write_5m + write_1h,
                "cache_creation": {
                    "ephemeral_5m_input_tokens": write_5m,
                    "ephemeral_1h_input_tokens": write_1h,
                },
                "cache_read_input_tokens": read,
                "server_tool_use": {"web_search_requests": 0},
                "service_tier": "standard",
            },
        },
    })
    .to_string()
}

/// Appends one line per turn, each its own response with "x" as its only content, to the
/// session's transcript `<home>/.claude/projects/-w/<session>.jsonl`, where Claude Code keeps
/// it. Returns the file.
pub fn write_transcript(root: &Path, session: &str, turns: &[Turn]) -> PathBuf {
    let dir = Env::for_test(root).home.join(".claude/projects/-w");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{session}.jsonl"));
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    let before = text.lines().count();
    for (i, t) in turns.iter().enumerate() {
        let n = before + i;
        let line = assistant_line(
            session,
            &format!("msg_{session}_{n}"),
            &format!("req_{session}_{n}"),
            t,
            "x",
        );
        text.push_str(&line);
        text.push('\n');
    }
    fs::write(&path, text).unwrap();
    path
}
```

Create `crates/tagteam/tests/tokens_cli.rs`. Its fixture is two accounts, config.toml prices for the two priced models (so no cost depends on the shipped table), and two sessions: one yesterday in account `a`'s profile, sighted there through the store's own API, and one two days ago in the default home before the store existed, which no evidence can credit to an account, so it is unattributed whatever `token_epoch.seeded` holds. Every report of the fixture passes `--tz UTC` and an explicit `--since` and `--until` (`Fixture::args`), so no expectation depends on when the test runs; the defaults (30 days, and twelve calendar months for `month`) are pinned by `tokens_cmd.rs`'s unit tests at fixed instants, and `month_defaults_to_twelve_calendar_months` asserts only their shape. No test sleeps or measures time: the progress line's one-second rule is a pure function of two `Instant`s, tested with fixed ones. The costs, in nano-dollars per token (opus 1000, 2000, 3000, 4000, 500; sonnet 500, 1000, 1500, 2000, 250): `s-a` is 163,000,000 + 16,000 + 459,000,000 = 622,016,000 ($0.62); `s-d`'s sonnet response 25,000,000 + 72,000,000 = 97,000,000 ($0.10), its other model unpriced, so the row and the total are partial. The tables below were rendered by this task's `tokens_cmd.rs` from that report in a scratch build.

```rust
//! `tagteam tokens` through the binary (token report spec §11, §15 "CLI"): the table, `--json`
//! and `--csv` of every period, the flags, and their usage errors. Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::Path;

use common::{Turn, cmd, now_epoch_s, turn, two_fresh_accounts, write_transcript};
use serde_json::{Value, json};
use tagteam_core::time::format_iso8601;
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::store::{NewAccount, SightingRow, SightingSource, Store};
use tagteam_fake::FAKE_AGENT;
use tagteam_provider::{Env, Identity};

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 86_400_000;
const OPUS: &str = "claude-opus-5-5";
const SONNET: &str = "claude-sonnet-5-5";
/// A model no price table names.
const UNPRICED: &str = "claude-test-9";
/// config.toml's prices for the two priced models, unlike any shipped table's, so every cost
/// below is fixed (token report spec §8.2). Per token, in nano-dollars: opus 1000, 2000,
/// 3000, 4000 and 500; sonnet 500, 1000, 1500, 2000 and 250.
const PRICES: &str = concat!(
    "[provider.claude-code.prices.\"claude-opus-5-5\"]\n",
    "input = 1.0\noutput = 2.0\ncache_write_5m = 3.0\ncache_write_1h = 4.0\ncache_read = 0.5\n\n",
    "[provider.claude-code.prices.\"claude-sonnet-5-5\"]\n",
    "input = 0.5\noutput = 1.0\ncache_write_5m = 1.5\ncache_write_1h = 2.0\ncache_read = 0.25\n",
);
const PARTIAL: &str = "the token index has not completed its first refresh, so this report is partial; run `tagteam tokens` without --no-refresh to complete it";
const FOOTNOTES: &str = concat!(
    "\n",
    "No price for claude-test-9: counted without cost (* marks a partial cost).\n",
    "Output is a lower bound for 1 subagent response.\n",
);

/// Midnight UTC, `days` days before today's.
fn midnight(days: i64) -> i64 {
    (now_epoch_s() * 1000).div_euclid(DAY_MS) * DAY_MS - days * DAY_MS
}

/// `ms` as `--json` and `--csv` spell a time.
fn iso(ms: i64) -> String {
    format_iso8601(ms.div_euclid(1000))
}

/// The UTC date of `ms`.
fn date_of(ms: i64) -> String {
    iso(ms)[..10].to_owned()
}

fn store(root: &Path) -> Store {
    Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap()
}

/// `tagteam <args>`'s one JSON object; it must succeed.
fn json_of(root: &Path, args: &[&str]) -> Value {
    let out = cmd(root)
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&out).unwrap()
}

/// `tagteam <args>`'s stdout; it must succeed with nothing on stderr.
fn text_of(root: &Path, args: &[&str]) -> String {
    let out = cmd(root)
        .args(args)
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    String::from_utf8(out).unwrap()
}

/// `a@x.co` (1) and `b@x.co` (2, live), config.toml's prices, and two sessions' transcripts:
/// - `s-a`, yesterday from 10:00 to 11:00 UTC, which `run` sighted in a's profile: three opus
///   responses, one of them a subagent's snapshot that never got its final count;
/// - `s-d`, two days ago at 10:00 and 10:30 UTC in the default home, a sonnet response and one
///   of a model with no price. It ran before the store existed, so no account can be credited
///   with it (spec §7.2–7.3): it is unattributed.
struct Fixture {
    dir: tempfile::TempDir,
    a: String,
    /// Yesterday's and the day before's midnights, UTC.
    d1: i64,
    d2: i64,
    /// The range every report of the fixture asks for: the day before yesterday's date and
    /// today's.
    since: String,
    until: String,
}

impl Fixture {
    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// `args`, then the fixture's range in UTC, so every expectation is fixed by the fixture
    /// whenever the test runs. The default range is `tokens_cmd`'s unit tests' to pin.
    fn args<'a>(&'a self, args: &[&'a str]) -> Vec<&'a str> {
        let mut all = args.to_vec();
        all.extend([
            "--tz",
            "UTC",
            "--since",
            &self.since,
            "--until",
            &self.until,
        ]);
        all
    }
}

/// `json_of` over the fixture's range.
fn json_in(f: &Fixture, args: &[&str]) -> Value {
    json_of(f.root(), &f.args(args))
}

/// `text_of` over the fixture's range.
fn text_in(f: &Fixture, args: &[&str]) -> String {
    text_of(f.root(), &f.args(args))
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (a, _b) = two_fresh_accounts(root);
    let (d1, d2) = (midnight(1), midnight(2));
    write_transcript(
        root,
        "s-a",
        &[
            turn(OPUS, d1 + 10 * HOUR_MS, [1_200, 3_400, 10_000, 0, 250_000]),
            Turn {
                final_line: false,
                ..turn(OPUS, d1 + 10 * HOUR_MS + 30 * MINUTE_MS, [0, 8, 0, 0, 0])
            },
            turn(OPUS, d1 + 11 * HOUR_MS, [800, 1_600, 0, 20_000, 750_000]),
        ],
    );
    write_transcript(
        root,
        "s-d",
        &[
            turn(SONNET, d2 + 10 * HOUR_MS, [50_000, 72_000, 0, 0, 0]),
            turn(
                UNPRICED,
                d2 + 10 * HOUR_MS + 30 * MINUTE_MS,
                [1_000, 3_000, 0, 0, 0],
            ),
        ],
    );
    let sighted = SightingRow {
        session_id: "s-a".into(),
        home: a.clone(),
        first_at: d1 + 9 * HOUR_MS,
        last_at: d1 + 12 * HOUR_MS,
        source: SightingSource::Run,
        recorded_at: now_epoch_s() * 1000,
    };
    store(root)
        .record_sighting(&ProviderId::new(CLAUDE_CODE), &sighted, 60_000)
        .unwrap();
    let config = Env::for_test(root).config_dir();
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("config.toml"), PRICES).unwrap();
    Fixture {
        dir,
        a,
        d1,
        d2,
        since: date_of(d2),
        until: date_of(d1 + DAY_MS),
    }
}

/// The last footnote, naming the shipped table's date and the overridden models.
fn price_note(as_of: &str) -> String {
    format!(
        "API cost at list prices as of {as_of}, with config.toml's for claude-opus-5-5, claude-sonnet-5-5.\n"
    )
}

/// The `tokens` object of a row or the total.
fn tokens(input: u64, output: u64, write_5m: u64, write_1h: u64, read: u64) -> Value {
    json!({
        "input": input, "output": output, "cacheWrite5m": write_5m, "cacheWrite1h": write_1h,
        "cacheWrite": write_5m + write_1h, "cacheRead": read,
        "total": input + output + write_5m + write_1h + read, "webSearch": 0,
    })
}

#[test]
fn tokens_day_human_json_and_csv() {
    let f = fixture();
    let v = json_in(&f, &["tokens", "--json"]);
    let as_of = v["prices"]["asOf"].as_str().unwrap().to_owned();
    let (d1, d2) = (date_of(f.d1), date_of(f.d2));
    let a = json!({"kind": "account", "number": 1, "id": f.a, "email": "a@x.co"});
    let unattributed = json!({"kind": "unattributed"});
    assert_eq!(
        v["rows"],
        json!([
            {
                "key": format!("day:{d1}|account:{}", f.a),
                "period": {"kind": "day", "date": d1},
                "account": a, "accounts": [a], "models": [OPUS],
                "tokens": tokens(2_000, 5_008, 10_000, 20_000, 1_000_000),
                "responses": 3, "unfinalized": 1, "costUsd": 0.622016, "costPartial": false,
            },
            {
                "key": format!("day:{d2}|account:unattributed"),
                "period": {"kind": "day", "date": d2},
                "account": unattributed, "accounts": [unattributed],
                "models": [SONNET, UNPRICED],
                "tokens": tokens(51_000, 75_000, 0, 0, 0),
                "responses": 2, "unfinalized": 0, "costUsd": 0.097, "costPartial": true,
            },
        ])
    );
    assert_eq!(
        v["total"],
        json!({
            "tokens": tokens(53_000, 80_008, 10_000, 20_000, 1_000_000),
            "responses": 5, "unfinalized": 1, "costUsd": 0.719016, "costPartial": true,
            "byBasis": {"profile": 1_037_008, "timeline": 0, "unattributed": 126_000},
        })
    );
    assert_eq!(
        [
            &v["schemaVersion"],
            &v["provider"],
            &v["period"],
            &v["grouping"],
            &v["timeZone"],
            &v["since"],
            &v["until"],
            &v["unpricedModels"],
            &v["prices"]["overridden"],
        ],
        [
            &json!(1),
            &json!("claude-code"),
            &json!("day"),
            &json!("account"),
            &json!("UTC"),
            &json!(iso(f.d2)),
            &json!(iso(f.d1 + DAY_MS)),
            &json!([UNPRICED]),
            &json!([OPUS, SONNET]),
        ]
    );
    let index = &v["index"];
    assert!(
        index["refreshedAt"].is_string() && index["changedAt"].is_string(),
        "{index}"
    );
    assert_eq!(
        [
            &index["files"],
            &index["responses"],
            &index["skippedLines"],
            &index["unreadableFiles"],
            &index["oldest"],
            &index["newest"],
        ],
        [
            &json!(2),
            &json!(5),
            &json!(0),
            &json!(0),
            &json!(iso(f.d2 + 10 * HOUR_MS)),
            &json!(iso(f.d1 + 11 * HOUR_MS)),
        ]
    );

    assert_eq!(
        text_in(&f, &["tokens", "--no-refresh"]),
        format!(
            concat!(
                "DAY         ACCOUNT       INPUT  OUTPUT  CACHE WRITE  CACHE READ   TOTAL  API COST\n",
                "{d1}  1 a@x.co       2.0K    5.0K        30.0K        1.0M    1.0M    $0.62\n",
                "{d2}  unattributed  51.0K   75.0K            0           0  126.0K    $0.10*\n",
                "TOTAL                     53.0K   80.0K        30.0K        1.0M    1.2M    $0.72*\n",
                "{notes}{prices}",
            ),
            d1 = d1,
            d2 = d2,
            notes = FOOTNOTES,
            prices = price_note(&as_of),
        )
    );
    assert_eq!(
        text_in(&f, &["tokens", "--no-refresh", "--csv"]),
        format!(
            concat!(
                "period,account,model,input,output,cache_write_5m,cache_write_1h,cache_read,web_search,total,responses,unfinalized,cost_usd\n",
                "{d1},a@x.co,,2000,5008,10000,20000,1000000,0,1037008,3,1,0.622016\n",
                "{d2},unattributed,,51000,75000,0,0,0,0,126000,2,0,0.097\n",
            ),
            d1 = d1,
            d2 = d2,
        )
    );

    // By account and model: a row per model, by name, and a model with no price has no cost.
    let v = json_in(
        &f,
        &["tokens", "--by", "account,model", "--json", "--no-refresh"],
    );
    let rows: Vec<(&Value, &Value, &Value, &Value)> = v["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (&r["key"], &r["model"], &r["costUsd"], &r["costPartial"]))
        .collect();
    assert_eq!(
        rows,
        [
            (
                &json!(format!("day:{d1}|account:{}|model:{OPUS}", f.a)),
                &json!(OPUS),
                &json!(0.622016),
                &json!(false),
            ),
            (
                &json!(format!("day:{d2}|account:unattributed|model:{SONNET}")),
                &json!(SONNET),
                &json!(0.097),
                &json!(false),
            ),
            (
                &json!(format!("day:{d2}|account:unattributed|model:{UNPRICED}")),
                &json!(UNPRICED),
                &Value::Null,
                &json!(false),
            ),
        ]
    );
    assert_eq!(
        text_in(&f, &["tokens", "--by", "account,model", "--no-refresh"]),
        format!(
            concat!(
                "DAY         ACCOUNT       MODEL              INPUT  OUTPUT  CACHE WRITE  CACHE READ   TOTAL  API COST\n",
                "{d1}  1 a@x.co      claude-opus-5-5     2.0K    5.0K        30.0K        1.0M    1.0M    $0.62\n",
                "{d2}  unattributed  claude-sonnet-5-5  50.0K   72.0K            0           0  122.0K    $0.10\n",
                "{d2}  unattributed  claude-test-9       1.0K    3.0K            0           0    4.0K        —\n",
                "TOTAL                                        53.0K   80.0K        30.0K        1.0M    1.2M    $0.72*\n",
                "{notes}{prices}",
            ),
            d1 = d1,
            d2 = d2,
            notes = FOOTNOTES,
            prices = price_note(&as_of),
        )
    );
    // By nothing: a row per day, with no `account` key, naming every account it holds.
    let v = json_in(&f, &["tokens", "--by", "none", "--json", "--no-refresh"]);
    assert_eq!(v["grouping"], "none");
    assert_eq!(v["rows"][0].get("account"), None);
    assert_eq!(v["rows"][0]["accounts"], json!([a]));
}

#[test]
fn bare_tokens_is_day() {
    let f = fixture();
    let root = f.root();
    cmd(root).args(["tokens", "--json"]).assert().success();
    let bare = json_in(&f, &["tokens", "--json", "--no-refresh"]);
    let day = json_in(&f, &["tokens", "day", "--json", "--no-refresh"]);
    assert_eq!(bare["period"], "day");
    assert_eq!(
        (&bare["rows"], &bare["total"], &bare["period"]),
        (&day["rows"], &day["total"], &day["period"])
    );
    // An ACCOUNT on a bare `tokens` is day's too.
    let only_a = json_in(&f, &["tokens", "1", "--json", "--no-refresh"]);
    assert_eq!(only_a["period"], "day");
    assert_eq!(only_a["rows"].as_array().unwrap().len(), 1);
    assert_eq!(only_a["rows"][0]["account"]["number"], 1);
    // `--unattributed` alone is the unattributed bucket alone; with an ACCOUNT, both.
    let keys = |args: &[&str]| -> Vec<String> {
        let mut all = vec!["tokens", "--json", "--no-refresh"];
        all.extend_from_slice(args);
        json_in(&f, &all)["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["key"].as_str().unwrap().to_owned())
            .collect()
    };
    let (d1, d2) = (date_of(f.d1), date_of(f.d2));
    assert_eq!(
        keys(&["--unattributed"]),
        [format!("day:{d2}|account:unattributed")]
    );
    assert_eq!(
        keys(&["a@x.co", "--unattributed"]),
        [
            format!("day:{d1}|account:{}", f.a),
            format!("day:{d2}|account:unattributed"),
        ]
    );
    assert!(
        keys(&["2"]).is_empty(),
        "b, the live login, made none of these requests"
    );
}

#[test]
fn a_since_date_is_local_midnight_in_tz() {
    // Review Focus 4: `--since DATE` is that date's midnight in `--tz`, and a day is that
    // zone's calendar day. Asia/Kolkata is UTC+05:30: 18:30 UTC is its next midnight.
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    two_fresh_accounts(root);
    let today = midnight(0);
    write_transcript(
        root,
        "s-tz",
        &[
            // Yesterday 18:00 UTC is 23:30 in Kolkata; 19:00 UTC is today's 00:30 there.
            turn(OPUS, today - 6 * HOUR_MS, [100, 0, 0, 0, 0]),
            turn(OPUS, today - 5 * HOUR_MS, [200, 0, 0, 0, 0]),
        ],
    );
    let local_today = date_of(today);
    let v = json_of(
        root,
        &[
            "tokens",
            "--tz",
            "Asia/Kolkata",
            "--since",
            &local_today,
            "--by",
            "none",
            "--json",
        ],
    );
    assert_eq!(v["timeZone"], "Asia/Kolkata");
    assert_eq!(v["since"], iso(today - 5 * HOUR_MS - 30 * MINUTE_MS));
    let rows = v["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{v}");
    assert_eq!(
        rows[0]["period"],
        json!({"kind": "day", "date": local_today})
    );
    assert_eq!(rows[0]["tokens"]["input"], 200);
    // The day before, in UTC: both responses fall on it.
    let yesterday = date_of(today - DAY_MS);
    let v = json_of(
        root,
        &[
            "tokens",
            "--tz",
            "UTC",
            "--since",
            &yesterday,
            "--by",
            "none",
            "--json",
            "--no-refresh",
        ],
    );
    assert_eq!(v["since"], iso(today - DAY_MS));
    let rows = v["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{v}");
    assert_eq!(rows[0]["period"]["date"], yesterday);
    assert_eq!(rows[0]["tokens"]["input"], 300);
    // `--until` takes the same forms, exclusive.
    let v = json_of(
        root,
        &[
            "tokens",
            "--tz",
            "UTC",
            "--since",
            &iso(today - 6 * HOUR_MS),
            "--until",
            &iso(today - 5 * HOUR_MS),
            "--by",
            "none",
            "--json",
            "--no-refresh",
        ],
    );
    assert_eq!(v["until"], iso(today - 5 * HOUR_MS));
    assert_eq!(v["total"]["tokens"]["input"], 100);
}

#[test]
fn an_unknown_model_is_a_usage_error_listing_models() {
    let f = fixture();
    let root = f.root();
    let listed = "--model nope matches no indexed model; the index has claude-opus-5-5, claude-sonnet-5-5, claude-test-9";
    cmd(root)
        .args(["tokens", "--model", "nope"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(format!("tagteam: {listed}\n"));
    let out = cmd(root)
        .args([
            "tokens",
            "--model",
            "opus",
            "--model",
            "nope",
            "--json",
            "--no-refresh",
        ])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "usage", "message": listed}})
    );
    // A pattern matches every model containing it, whatever the case.
    let v = json_in(&f, &["tokens", "--model", "OPUS", "--json", "--no-refresh"]);
    assert_eq!(v["rows"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["rows"][0]["models"], json!([OPUS]));
    let v = json_in(
        &f,
        &["tokens", "--model", "claude-s", "--json", "--no-refresh"],
    );
    assert_eq!(v["rows"][0]["models"], json!([SONNET]));
    assert_eq!(v["total"]["responses"], 1);
    // An index with nothing in it names no model.
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    cmd(d.path())
        .args(["tokens", "--model", "opus"])
        .assert()
        .code(2)
        .stderr("tagteam: --model opus matches no indexed model; the index has none\n");
}

#[test]
fn csv_and_json_together_is_a_usage_error() {
    // Ruling R6, as for `history`; refused before the store is created.
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    let out = cmd(d.path())
        .args(["tokens", "--csv", "--json"])
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
    assert!(!Env::for_test(d.path()).data_dir().exists());
}

#[test]
fn no_refresh_reports_the_partial_index_note() {
    // Spec §14: before the first complete refresh the index is partial, and `--no-refresh`
    // says so on stderr.
    let f = fixture();
    let root = f.root();
    let out = cmd(root)
        .args(["tokens", "--no-refresh", "--json"])
        .assert()
        .success()
        .stderr(format!("note: {PARTIAL}\n"))
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["index"]["refreshedAt"], Value::Null);
    assert_eq!(v["rows"], json!([]));
    cmd(root)
        .args(["tokens", "--no-refresh"])
        .assert()
        .success()
        .stdout("No token usage in this range.\n")
        .stderr(format!("note: {PARTIAL}\n"));
    // A refresh completes the index; from then on `--no-refresh` reports it, quietly.
    cmd(root).args(["tokens"]).assert().success().stderr("");
    let text = text_in(&f, &["tokens", "--no-refresh"]);
    assert!(text.starts_with("DAY "), "{text}");
}

#[test]
fn month_defaults_to_twelve_calendar_months() {
    // Spec §11.1: with no --since, `month` starts on the first day of the eleventh month back, at
    // midnight. Which month that is depends on the date, so this asserts the shape alone, and
    // holds if the month turns while it runs; `default_since_ms`'s unit test pins the month.
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    two_fresh_accounts(root);
    // A response every 30 days for 13 months: more months than the report spans.
    let now = now_epoch_s() * 1000;
    let turns: Vec<Turn> = (0..14i64)
        .map(|k| turn(OPUS, now - HOUR_MS - k * 30 * DAY_MS, [10, 0, 0, 0, 0]))
        .collect();
    write_transcript(root, "s-m", &turns);
    let month = |iso: &str| -> i64 {
        iso[..4].parse::<i64>().unwrap() * 12 + iso[5..7].parse::<i64>().unwrap() - 1
    };
    let before = month(&iso(now_epoch_s() * 1000));
    let v = json_of(
        root,
        &["tokens", "month", "--tz", "UTC", "--by", "none", "--json"],
    );
    let after = month(&iso(now_epoch_s() * 1000));
    let since = v["since"].as_str().unwrap();
    assert!(
        since.ends_with("-01T00:00:00Z"),
        "midnight on a first: {since}"
    );
    let first = month(since);
    assert!(
        first == before - 11 || first == after - 11,
        "the eleventh month back: {since}"
    );
    assert_eq!((&v["period"], &v["until"]), (&json!("month"), &Value::Null));
    let months: Vec<i64> = v["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| month(r["period"]["month"].as_str().unwrap()))
        .collect();
    assert!((2..=12).contains(&months.len()), "{months:?}");
    assert!(
        months.windows(2).all(|w| w[0] > w[1]),
        "a row a month, newest first: {months:?}"
    );
    assert!(
        months.iter().all(|m| (first..=after).contains(m)),
        "{months:?} outside {first}..={after}"
    );
}

#[test]
fn named_accounts_choose_the_provider() {
    // Core §13.1: --provider, else the provider of the accounts named, else the default. This
    // build registers Claude Code alone, so another provider's account is planted in the store.
    let f = fixture();
    let root = f.root();
    let identity = Identity {
        label: "f@x.co".into(),
        email: Some("f@x.co".into()),
        org_uuid: String::new(),
        org_name: None,
        account_uuid: None,
        raw: json!({}),
    };
    store(root)
        .insert_account(&NewAccount {
            id: &AccountId::from_string("0192-fake"),
            provider: &ProviderId::new(FAKE_AGENT),
            position: 1,
            identity_key: "f@x.co\n",
            identity: &identity,
            kind: "oauth",
            alias: Some("fakey"),
            login_expires_at: None,
            added_at: 1,
        })
        .unwrap();
    // Named under --provider claude-code, it is refused as `run` refuses it.
    cmd(root)
        .args(["tokens", "fakey", "--provider", CLAUDE_CODE])
        .assert()
        .code(2)
        .stdout("")
        .stderr(format!(
            "tagteam: position 1 is a {FAKE_AGENT} account, not a {CLAUDE_CODE} one\n"
        ));
    // Beside a Claude Code account: one report covers one provider.
    cmd(root)
        .args(["tokens", "a@x.co", "fakey"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(format!(
            "tagteam: the accounts named belong to more than one provider ({CLAUDE_CODE}, {FAKE_AGENT}); name one provider's accounts\n"
        ));
    // Named alone, it chooses its provider: here one this build lacks.
    cmd(root)
        .args(["tokens", "fakey"])
        .assert()
        .code(1)
        .stdout("")
        .stderr(format!("tagteam: unknown provider \"{FAKE_AGENT}\"\n"));
    // a@x.co chooses Claude Code, and so does naming no account (the default provider).
    let v = json_in(&f, &["tokens", "a@x.co", "--json"]);
    assert_eq!(v["provider"], CLAUDE_CODE);
    assert_eq!(v["rows"].as_array().unwrap().len(), 1, "a's day alone: {v}");
    assert_eq!(
        json_in(&f, &["tokens", "--json", "--no-refresh"])["provider"],
        CLAUDE_CODE
    );
}

#[test]
fn session_and_block_columns() {
    let f = fixture();
    let v = json_in(&f, &["tokens", "session", "--json"]);
    let as_of = v["prices"]["asOf"].as_str().unwrap().to_owned();
    let (d1, d2) = (date_of(f.d1), date_of(f.d2));
    assert_eq!(v["period"], "session");
    assert_eq!(
        (&v["rows"][0]["key"], &v["rows"][1]["key"]),
        (
            &json!(format!("session:s-a|account:{}", f.a)),
            &json!("session:s-d|account:unattributed")
        )
    );
    assert_eq!(
        (&v["rows"][0]["period"], &v["rows"][1]["period"]),
        (
            &json!({"kind": "session", "sessionId": "s-a", "project": "/w",
                    "first": iso(f.d1 + 10 * HOUR_MS), "last": iso(f.d1 + 11 * HOUR_MS)}),
            &json!({"kind": "session", "sessionId": "s-d", "project": "/w",
                    "first": iso(f.d2 + 10 * HOUR_MS), "last": iso(f.d2 + 10 * HOUR_MS + 30 * MINUTE_MS)}),
        )
    );
    assert_eq!(
        text_in(&f, &["tokens", "session", "--no-refresh"]),
        format!(
            concat!(
                "SESSION  FIRST             LAST              ACCOUNT       INPUT  OUTPUT  CACHE WRITE  CACHE READ   TOTAL  API COST  PROJECT\n",
                "s-a      {d1} 10:00  {d1} 11:00  1 a@x.co       2.0K    5.0K        30.0K        1.0M    1.0M    $0.62   /w\n",
                "s-d      {d2} 10:00  {d2} 10:30  unattributed  51.0K   75.0K            0           0  126.0K    $0.10*  /w\n",
                "TOTAL                                                      53.0K   80.0K        30.0K        1.0M    1.2M    $0.72*\n",
                "{notes}{prices}",
            ),
            d1 = d1,
            d2 = d2,
            notes = FOOTNOTES,
            prices = price_note(&as_of),
        )
    );
    assert_eq!(
        text_in(&f, &["tokens", "session", "--no-refresh", "--csv"]),
        format!(
            concat!(
                "period,account,model,input,output,cache_write_5m,cache_write_1h,cache_read,web_search,total,responses,unfinalized,cost_usd,project,first_at,last_at\n",
                "s-a,a@x.co,,2000,5008,10000,20000,1000000,0,1037008,3,1,0.622016,/w,{d1}T10:00:00Z,{d1}T11:00:00Z\n",
                "s-d,unattributed,,51000,75000,0,0,0,0,126000,2,0,0.097,/w,{d2}T10:00:00Z,{d2}T10:30:00Z\n",
            ),
            d1 = d1,
            d2 = d2,
        )
    );

    // Blocks are per account (spec §9): no usage sample observed a reset, so each is inferred,
    // from the hour of its first response, five hours long.
    let v = json_in(&f, &["tokens", "block", "--json", "--no-refresh"]);
    assert_eq!(v["period"], "block");
    assert_eq!(
        (&v["rows"][0]["key"], &v["rows"][1]["key"]),
        (
            &json!(format!("block:{}|account:{}", f.d1 + 10 * HOUR_MS, f.a)),
            &json!(format!(
                "block:{}|account:unattributed",
                f.d2 + 10 * HOUR_MS
            )),
        )
    );
    assert_eq!(
        v["rows"][0]["period"],
        json!({"kind": "block", "start": iso(f.d1 + 10 * HOUR_MS),
               "end": iso(f.d1 + 15 * HOUR_MS), "observed": false, "active": null})
    );
    assert_eq!(
        text_in(&f, &["tokens", "block", "--no-refresh"]),
        format!(
            concat!(
                "START             END               ACCOUNT       INPUT  OUTPUT  CACHE WRITE  CACHE READ   TOTAL  API COST\n",
                "{d1} 10:00  {d1} 15:00  1 a@x.co       2.0K    5.0K        30.0K        1.0M    1.0M    $0.62\n",
                "{d2} 10:00  {d2} 15:00  unattributed  51.0K   75.0K            0           0  126.0K    $0.10*\n",
                "TOTAL                                             53.0K   80.0K        30.0K        1.0M    1.2M    $0.72*\n",
                "{notes}{prices}",
            ),
            d1 = d1,
            d2 = d2,
            notes = FOOTNOTES,
            prices = price_note(&as_of),
        )
    );
    assert_eq!(
        text_in(&f, &["tokens", "block", "--no-refresh", "--csv"]),
        format!(
            concat!(
                "period,account,model,input,output,cache_write_5m,cache_write_1h,cache_read,web_search,total,responses,unfinalized,cost_usd,start,end,observed,active\n",
                "{d1}T10:00:00Z,a@x.co,,2000,5008,10000,20000,1000000,0,1037008,3,1,0.622016,{d1}T10:00:00Z,{d1}T15:00:00Z,false,false\n",
                "{d2}T10:00:00Z,unattributed,,51000,75000,0,0,0,0,126000,2,0,0.097,{d2}T10:00:00Z,{d2}T15:00:00Z,false,false\n",
            ),
            d1 = d1,
            d2 = d2,
        )
    );
}

#[test]
fn help_documents_the_json_shape() {
    // Core §13.2: a command's `--json` shape is documented in its `--help`.
    let d = tempfile::tempdir().unwrap();
    let out = cmd(d.path())
        .args(["tokens", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help = String::from_utf8(out).unwrap();
    for word in [
        "day",
        "month",
        "session",
        "block",
        "unattributed",
        "--no-refresh",
        "cacheWrite5m",
        "costUsd",
        "unpricedModels",
        "timeZone",
    ] {
        assert!(help.contains(word), "{word}: {help}");
    }
}

#[test]
fn bad_flags_are_usage_errors() {
    // Spec §14: an invalid --since, --until, --tz or --by exits 2, before the store exists.
    const SINCE: &str =
        "--since takes a span like 30d, 12h or 90m, a date YYYY-MM-DD, or an ISO 8601 time";
    const UNTIL: &str =
        "--until takes a span like 30d, 12h or 90m, a date YYYY-MM-DD, or an ISO 8601 time";
    const BY: &str = "--by takes account, model, account,model or none";
    const TZ: &str = "unknown time zone \"Mars/Olympus_Mons\"";
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    let cases: [(&[&str], &str); 9] = [
        (&["tokens", "--since", "7w"], SINCE),
        (&["tokens", "--since", "0d"], SINCE),
        (&["tokens", "--since", "2026-02-30"], SINCE),
        (&["tokens", "--since", "2026-10-14 09:00"], SINCE),
        (&["tokens", "--until", "soon"], UNTIL),
        (&["tokens", "--by", "week"], BY),
        (&["tokens", "--by", "model,account"], BY),
        (&["tokens", "--tz", "Mars/Olympus_Mons"], TZ),
        (&["tokens", "month", "--by", "day"], BY),
    ];
    for (args, message) in cases {
        cmd(d.path())
            .args(args)
            .assert()
            .code(2)
            .stdout("")
            .stderr(format!("tagteam: {message}\n"));
    }
    assert!(
        !Env::for_test(d.path()).data_dir().exists(),
        "every flag is checked before the store is created"
    );
}
```

In `crates/tagteam/src/cli.rs`, give the tests module the `tokens` parse test (this does not compile until Step 3 adds `TokensPeriod`):

cli.rs test hunk 1. Replace

```rust
#[cfg(test)]
mod tests {
    use tagteam_engine::settings::parse_bool;

    use super::BOOL_WORDS;
```

with

```rust
#[cfg(test)]
mod tests {
    use clap::Parser;
    use tagteam_engine::settings::parse_bool;

    use super::{BOOL_WORDS, Cli, Command, TokensPeriod};
```

cli.rs test hunk 2. Replace

```rust
        for word in words_up_to(4).iter().map(String::as_str).chain(alike) {
            assert!(
                parse_bool(word).is_none() || BOOL_WORDS.contains(&word),
                "{word:?} is accepted but never completed"
            );
        }
    }
```

with

```rust
        for word in words_up_to(4).iter().map(String::as_str).chain(alike) {
            assert!(
                parse_bool(word).is_none() || BOOL_WORDS.contains(&word),
                "{word:?} is accepted but never completed"
            );
        }
    }

    /// `tagteam <args>` as `tokens`: the period's name (none when bare), and the ACCOUNTs and
    /// `--by` of whichever level took them.
    fn tokens(args: &[&str]) -> (Option<&'static str>, Vec<String>, Option<String>) {
        let cli = Cli::try_parse_from(std::iter::once("tagteam").chain(args.iter().copied()))
            .unwrap_or_else(|e| panic!("{args:?}: {e}"));
        let Some(Command::Tokens { period, args }) = cli.command else {
            panic!("not tokens");
        };
        let (name, args) = match period.map(|p| *p) {
            None => (None, args),
            Some(TokensPeriod::Day(a)) => (Some("day"), a),
            Some(TokensPeriod::Month(a)) => (Some("month"), a),
            Some(TokensPeriod::Session(a)) => (Some("session"), a),
            Some(TokensPeriod::Block(a)) => (Some("block"), a),
        };
        (name, args.accounts, args.by)
    }

    #[test]
    fn tokens_takes_its_period_as_a_subcommand_and_is_day_when_bare() {
        // Token report spec §11.1: the period is a subcommand, so an ACCOUNT spelled like one
        // follows a period; a bare `tokens` takes the same arguments, as `day`.
        let owned = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(tokens(&["tokens"]), (None, vec![], None));
        assert_eq!(
            tokens(&["tokens", "2", "--by", "model"]),
            (None, owned(&["2"]), Some("model".to_owned()))
        );
        assert_eq!(
            tokens(&["tokens", "month", "work", "1", "--by", "account,model"]),
            (
                Some("month"),
                owned(&["work", "1"]),
                Some("account,model".to_owned())
            )
        );
        assert_eq!(
            tokens(&["tokens", "block", "day"]),
            (Some("block"), owned(&["day"]), None),
            "an account named like a period, after one"
        );
        assert_eq!(tokens(&["tokens", "session"]).0, Some("session"));
    }
```

In `crates/tagteam/src/app.rs`, the late-notice test names `tokens` too:

app.rs test hunk 1. Replace

```rust
            &["history"],
            &["statusline"],
```

with

```rust
            &["history"],
            &["tokens"],
            &["tokens", "month", "1", "--by", "model"],
            &["tokens", "2", "--json"],
            &["statusline"],
```

`tokens_cmd.rs`'s own unit tests (the number formatter, dollars, the time forms, `month`'s default, `--model`, the flags, the progress line, the block note) are the `mod tests` at the end of the new file in Step 3.

- [ ] **Step 2: Run them and see them fail**

```
cargo test -p tagteam --features test-support --test tokens_cli
```

Expected: the target builds (it uses only `common`, the store API and the binary) and all eleven tests fail, each on its first `tagteam tokens` call: `Unexpected failure. code=2` with stderr `error: unrecognized subcommand 'tokens'`, or for `csv_and_json_together_is_a_usage_error` and `bad_flags_are_usage_errors` a usage error whose message is clap's (`unrecognized subcommand`) rather than the expected one.

```
cargo test -p tagteam --features test-support --lib cli::tests
```

Expected: a compile error, `error[E0432]: unresolved import super::TokensPeriod` (and `no variant named Tokens`).

- [ ] **Step 3: Implement**

`crates/tagteam/Cargo.toml`, under `[dependencies]`, after `fastrand.workspace = true`:

```toml
jiff.workspace = true
```

Then record the new edge in `Cargo.lock` (jiff is already locked, so nothing is fetched):

```
cargo check -p tagteam
```

`crates/tagteam/src/lib.rs`: after `mod statusline;` add

```rust
mod tokens_cmd;
```

`crates/tagteam/src/history.rs`: `tokens --csv` quotes its fields as `history --csv` does.

```rust
/// RFC 4180: a field holding a comma, a quote or a line break is quoted, its quotes doubled.
pub(crate) fn csv_field(s: &str) -> String {
```

(was `fn csv_field(s: &str) -> String {`; the body is unchanged).

`crates/tagteam/src/cli.rs`, in order:

cli.rs hunk 1. Replace

```rust
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
```

with

```rust
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
```

cli.rs hunk 2. Replace

```rust
        /// Print the raw samples as CSV
        #[arg(long)]
        csv: bool,
    },
```

with

```rust
        /// Print the raw samples as CSV
        #[arg(long)]
        csv: bool,
    },
    /// Token use and API cost per account, from Claude Code's transcripts
    ///
    /// Reads new transcript lines into tagteam's index first (not with --no-refresh), then
    /// reports by day (the default), month, session or 5-hour block, a row per account unless
    /// --by says otherwise. With no ACCOUNT, every account shows, and the usage no account can
    /// be credited with as unattributed. The cost is the API list price of the tokens, not a
    /// charge. --json prints {schemaVersion, provider, period, grouping, timeZone, since,
    /// until, rows: [{key, period, account?, model?, accounts, models, tokens: {input, output,
    /// cacheWrite5m, cacheWrite1h, cacheWrite, cacheRead, total, webSearch}, responses,
    /// unfinalized, costUsd, costPartial}], total, index, unpricedModels, prices: {asOf,
    /// overridden}}, with times in ISO 8601 UTC and dates in timeZone.
    #[command(args_conflicts_with_subcommands = true)]
    Tokens {
        // Boxed: a period carries its own arguments, which would double every `Command`.
        #[command(subcommand)]
        period: Option<Box<TokensPeriod>>,
        #[command(flatten)]
        args: TokensArgs,
    },
```

cli.rs hunk 3. Replace

```rust
    /// failure. `history` and `statusline` read only the store and `~/.claude.json`, and the
    /// rest need no Keychain item. A recovery under their mutation lock (Task 21) only reads the
    /// Keychain, tri-state, and leaves what it cannot decide to the next command that checks.
```

with

```rust
    /// failure. `history` and `statusline` read only the store and `~/.claude.json`, `tokens`
    /// only the store and Claude Code's transcripts, and the rest need no Keychain item. A
    /// recovery under their mutation lock (Task 21) only reads the Keychain, tri-state, and
    /// leaves what it cannot decide to the next command that checks.
```

cli.rs hunk 4. Replace

```rust
/// The shells `shell-init` writes for (§12.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ShellArg {
    Zsh,
    Bash,
    Fish,
}
```

with

```rust
/// The shells `shell-init` writes for (§12.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ShellArg {
    Zsh,
    Bash,
    Fish,
}

/// `tokens`' period (token report spec §11.1): a subcommand, so an account alias spelled like
/// one is never taken for it. A bare `tokens` is `tokens day`.
#[derive(Subcommand)]
pub enum TokensPeriod {
    /// A row per calendar day (the default)
    Day(TokensArgs),
    /// A row per calendar month; --since defaults to twelve months
    Month(TokensArgs),
    /// A row per session, with its first and last times and its project
    Session(TokensArgs),
    /// A row per 5-hour block of each account
    Block(TokensArgs),
}

/// `tokens`' arguments (token report spec §11.1), the same under every period.
#[derive(Args)]
pub struct TokensArgs {
    /// Only these accounts: a position, alias or email (default: every account, and the
    /// unattributed usage)
    #[arg(value_name = "ACCOUNT")]
    pub accounts: Vec<String>,
    /// Also the usage no account can be credited with
    #[arg(long)]
    pub unattributed: bool,
    /// Only models whose name contains M, in any case (repeatable)
    #[arg(long, value_name = "M")]
    pub model: Vec<String>,
    /// From: a span back like 30d, 12h or 90m, a date YYYY-MM-DD, or an ISO 8601 time
    /// (default: 30d; for month, twelve calendar months)
    #[arg(long, value_name = "WHEN")]
    pub since: Option<String>,
    /// Until, not included: as --since takes it (default: now)
    #[arg(long, value_name = "WHEN")]
    pub until: Option<String>,
    /// A row per account, model, account,model or none (default: account)
    #[arg(
        long,
        value_name = "GROUPS",
        value_parser = GroupingParser,
        hide_possible_values = true
    )]
    pub by: Option<String>,
    /// Days and months in this IANA time zone, or UTC (default: the system's)
    #[arg(long, value_name = "ZONE")]
    pub tz: Option<String>,
    /// Print the rows as CSV
    #[arg(long)]
    pub csv: bool,
    /// Report from the index as it is, reading no transcript
    #[arg(long = "no-refresh")]
    pub no_refresh: bool,
}
```

cli.rs hunk 5. Replace

```rust
/// The shells `tagteam completions` writes a script for (§13.7).
```

with

```rust
/// `tokens --by`. Any UTF-8 string parses, so the command refuses a bad value with its own
/// usage error (token report spec §14), while completion offers the values; `account,model`
/// is hidden from it, since a completion word holds no comma.
#[derive(Clone)]
pub struct GroupingParser;

impl TypedValueParser for GroupingParser {
    type Value = String;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<String, clap::Error> {
        StringValueParser::new().parse_ref(cmd, arg, value)
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        Some(Box::new(
            [
                PossibleValue::new("account"),
                PossibleValue::new("model"),
                PossibleValue::new("account,model").hide(true),
                PossibleValue::new("none"),
            ]
            .into_iter(),
        ))
    }
}

/// The shells `tagteam completions` writes a script for (§13.7).
```

`crates/tagteam/src/app.rs`, in order. `CSV_AND_JSON` and `KIND_UNSUPPORTED` already exist; settings warnings, invalid price overrides among them, are already printed by `run_command` for every command (spec §11.3):

app.rs hunk 1. Replace

```rust
use tagteam_core::autoswitch::Strategy;
```

with

```rust
use tagteam_core::autoswitch::Strategy;
use tagteam_core::validate::{AccountRefInput, parse_account_ref};
```

app.rs hunk 2. Replace

```rust
use tagteam_engine::switch::{SwitchReason, SwitchRequest, SwitchTarget, UsageStrategy};
```

with

```rust
use tagteam_engine::switch::{SwitchReason, SwitchRequest, SwitchTarget, UsageStrategy};
use tagteam_engine::tokens::{AccountSel, TokenPeriod, TokenQuery};
```

app.rs hunk 3. Replace

```rust
use crate::cli::{AutoStrategyArg, Cli, Command, ConfigAction, ShellArg, StrategyArg};
```

with

```rust
use crate::cli::{
    AutoStrategyArg, Cli, Command, ConfigAction, ShellArg, StrategyArg, TokensArgs, TokensPeriod,
};
```

app.rs hunk 4. Replace

```rust
use crate::{
    auto, config_cmd, displaced_cmd, doctor_cmd, history, prompt, purge_cmd, render, root_guard,
    shell_init, statusline, transfer_cmd,
};
```

with

```rust
use crate::{
    auto, config_cmd, displaced_cmd, doctor_cmd, history, prompt, purge_cmd, render, root_guard,
    shell_init, statusline, tokens_cmd, transfer_cmd,
};
```

app.rs hunk 5. Replace

```rust
    /// §10.4, with a terminal prompt for an ambiguous email.
    fn resolve(&mut self, input: &str) -> Result<AccountRow, Failure> {
        match self.engine.resolve(input, self.provider_flag.as_ref()) {
            Err(EngineError::Ambiguous { .. }) if self.can_prompt() => {
                let found = self.engine.candidates(input, self.provider_flag.as_ref())?;
```

with

```rust
    /// §10.4, with a terminal prompt for an ambiguous email.
    fn resolve(&mut self, input: &str) -> Result<AccountRow, Failure> {
        let provider = self.provider_flag.clone();
        self.resolve_in(input, provider.as_ref())
    }

    /// `resolve` within `provider`, or in every provider when it is `None` (§10.4).
    fn resolve_in(
        &mut self,
        input: &str,
        provider: Option<&ProviderId>,
    ) -> Result<AccountRow, Failure> {
        match self.engine.resolve(input, provider) {
            Err(EngineError::Ambiguous { .. }) if self.can_prompt() => {
                let found = self.engine.candidates(input, provider)?;
```

app.rs hunk 6. Replace

```rust
        Command::History { .. } => "history",
```

with

```rust
        Command::History { .. } => "history",
        Command::Tokens { .. } => "tokens",
```

app.rs hunk 7. Replace

```rust
            } => self.history(account, window, &since, csv)?,
```

with

```rust
            } => self.history(account, window, &since, csv)?,
            Command::Tokens { period, args } => self.tokens(period, args)?,
```

app.rs hunk 8. Replace

```rust
    /// §12.7: `map` with no arguments lists the mappings, only `--provider`'s when it is given.
    /// It never creates the store (§5).
    fn list_mappings(&mut self) -> Result<(), Failure> {
```

with

```rust
    /// Token report spec §11: `tokens`. Every flag is checked before anything is read; the
    /// accounts named choose the provider unless `--provider` does (core §13.1); the index is
    /// refreshed (unless `--no-refresh`), and `--model` is then resolved against it. Like any
    /// writing command it creates the store. It never fetches and touches no Keychain item.
    fn tokens(
        &mut self,
        period: Option<Box<TokensPeriod>>,
        args: TokensArgs,
    ) -> Result<(), Failure> {
        let (period, args) = match period.map(|p| *p) {
            None => (TokenPeriod::Day, args),
            Some(TokensPeriod::Day(a)) => (TokenPeriod::Day, a),
            Some(TokensPeriod::Month(a)) => (TokenPeriod::Month, a),
            Some(TokensPeriod::Session(a)) => (TokenPeriod::Session, a),
            Some(TokensPeriod::Block(a)) => (TokenPeriod::Block, a),
        };
        if args.csv && self.json {
            return Err(Failure::Usage(CSV_AND_JSON.into()));
        }
        let flags = tokens_cmd::parse_flags(&args, &period, self.engine.now_ms())
            .map_err(Failure::Usage)?;
        // Core §13.1: a position counts within --provider, else the default provider; an alias
        // or an email names its account in whichever provider holds it, so `provider_for` can
        // refuse one that is not --provider's, or accounts of two providers at once.
        let mut named = Vec::new();
        for input in &args.accounts {
            let scope = match parse_account_ref(input) {
                Some(AccountRefInput::Position(_)) => self.provider_flag.clone(),
                _ => None,
            };
            named.push(self.resolve_in(input, scope.as_ref())?);
        }
        let positions: Vec<(u32, ProviderId)> = named
            .iter()
            .map(|row| (row.position, row.provider.clone()))
            .collect();
        let provider = tokens_cmd::provider_for(
            self.provider_flag.as_ref(),
            self.engine.default_provider(),
            &positions,
        )
        .map_err(Failure::Usage)?;
        let p = self.engine.provider(&provider)?;
        if !p.capabilities().transcripts {
            return Err(Failure::Message(
                KIND_UNSUPPORTED,
                format!("{} has no transcripts", p.display_name()),
            ));
        }
        let mut accounts: Vec<AccountSel> = named
            .into_iter()
            .map(|row| AccountSel::Account(row.id))
            .collect();
        if args.unattributed {
            accounts.push(AccountSel::Unattributed);
        }
        let store = self.engine.store()?;
        if !args.no_refresh {
            tokens_cmd::refresh(&self.engine, &provider, &mut *self.io.err)?;
        }
        let models = if args.model.is_empty() {
            None
        } else {
            let indexed = store.token_models(&provider).map_err(EngineError::from)?;
            Some(tokens_cmd::resolve_models(&args.model, &indexed).map_err(Failure::Usage)?)
        };
        let report = self.engine.token_report(&TokenQuery {
            provider,
            period,
            grouping: flags.grouping,
            since_ms: Some(flags.since_ms),
            until_ms: flags.until_ms,
            accounts: (!accounts.is_empty()).then_some(accounts),
            models,
            session: None,
            tz: flags.tz,
        })?;
        for notice in tokens_cmd::notices(&report, args.no_refresh) {
            let _ = writeln!(self.io.err, "{notice}");
        }
        if args.csv {
            let _ = write!(self.io.out, "{}", tokens_cmd::csv(&report));
        } else {
            self.print(&tokens_cmd::human(&report), tokens_cmd::json(&report));
        }
        Ok(())
    }

    /// §12.7: `map` with no arguments lists the mappings, only `--provider`'s when it is given.
    /// It never creates the store (§5).
    fn list_mappings(&mut self) -> Result<(), Failure> {
```

Create `crates/tagteam/src/tokens_cmd.rs`:

```rust
//! `tagteam tokens` (token report spec §11): the report of spec §10, after a refresh of the
//! index unless `--no-refresh`, as a table, JSON or CSV. It never fetches and touches no
//! Keychain item.

use std::collections::BTreeSet;
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use jiff::tz::TimeZone;
use serde_json::{Map, Value, json};
use tagteam_core::ProviderId;
use tagteam_core::time::{format_iso8601, parse_iso8601_ms};
use tagteam_core::tokens::{day_bounds_ms, month_bounds_ms, month_of};
use tagteam_engine::tokens::{
    ActiveBlock, Basis, Cost, IndexProgress, IndexState, PeriodKey, RowAccount, TokenCounts,
    TokenGrouping, TokenPeriod, TokenReport, TokenRow, TokenTotal, Wait,
};
use tagteam_engine::{Engine, EngineError};

use crate::cli::TokensArgs;
use crate::history;
use crate::render::{self, MISSING};

/// Spec §11.1: how far back `--since` defaults to, for every period but `month`.
const DEFAULT_SINCE_MS: i64 = 30 * 86_400_000;
/// Spec §11.2: a refresh shows progress only once it has run this long.
const PROGRESS_AFTER: Duration = Duration::from_secs(1);

const BAD_SINCE: &str =
    "--since takes a span like 30d, 12h or 90m, a date YYYY-MM-DD, or an ISO 8601 time";
const BAD_UNTIL: &str =
    "--until takes a span like 30d, 12h or 90m, a date YYYY-MM-DD, or an ISO 8601 time";
const BAD_BY: &str = "--by takes account, model, account,model or none";
const PARTIAL_INDEX: &str = "the token index has not completed its first refresh, so this report is partial; run `tagteam tokens` without --no-refresh to complete it";
const NO_USAGE: &str = "No token usage in this range.\n";

/// What `tokens`' flags ask for (spec §11.1), each checked before the index is touched.
pub(crate) struct Flags {
    pub(crate) grouping: TokenGrouping,
    pub(crate) tz: TimeZone,
    pub(crate) since_ms: i64,
    pub(crate) until_ms: Option<i64>,
}

/// Spec §11.1, §14: `--by`, `--tz`, `--since` and `--until`, or the usage error of the first
/// that is invalid. `--since` defaults to 30 days back, and for `month` to the first day of
/// the eleventh month before this one, in `--tz`. `--tz` defaults to the system zone, and to
/// UTC where that cannot be found; a zone named that the system's database lacks is an error.
pub(crate) fn parse_flags(
    args: &TokensArgs,
    period: &TokenPeriod,
    now_ms: i64,
) -> Result<Flags, String> {
    let grouping = match args.by.as_deref() {
        None => TokenGrouping::Account,
        Some(by) => grouping(by).ok_or(BAD_BY)?,
    };
    let tz = match args.tz.as_deref() {
        None => TimeZone::try_system().unwrap_or(TimeZone::UTC),
        Some(name) => zone(name).ok_or_else(|| format!("unknown time zone {name:?}"))?,
    };
    let since_ms = match args.since.as_deref() {
        Some(s) => parse_time(s, now_ms, &tz).ok_or(BAD_SINCE)?,
        None => default_since_ms(period, now_ms, &tz),
    };
    let until_ms = match args.until.as_deref() {
        Some(s) => Some(parse_time(s, now_ms, &tz).ok_or(BAD_UNTIL)?),
        None => None,
    };
    Ok(Flags {
        grouping,
        tz,
        since_ms,
        until_ms,
    })
}

/// `--by`'s four values (spec §11.1).
fn grouping(by: &str) -> Option<TokenGrouping> {
    match by {
        "account" => Some(TokenGrouping::Account),
        "model" => Some(TokenGrouping::Model),
        "account,model" => Some(TokenGrouping::AccountModel),
        "none" => Some(TokenGrouping::None),
        _ => None,
    }
}

/// `--tz`: `UTC`, or a zone the system's time zone database names (spec §11.1).
fn zone(name: &str) -> Option<TimeZone> {
    if name == "UTC" {
        return Some(TimeZone::UTC);
    }
    TimeZone::get(name).ok()
}

/// A `--since` or `--until` (spec §11.1), in epoch ms: `Nd`, `Nh` or `Nm` back from `now_ms`
/// (as `history` takes them), a date `YYYY-MM-DD` at its midnight in `tz`, or an ISO 8601 time.
pub(crate) fn parse_time(s: &str, now_ms: i64, tz: &TimeZone) -> Option<i64> {
    if let Some(span_s) = history::parse_since(s) {
        return now_ms.checked_sub(span_s.checked_mul(1000)?);
    }
    if let Some(date) = date(s) {
        return Some(day_bounds_ms(date, tz).0);
    }
    parse_iso8601_ms(s)
}

/// `YYYY-MM-DD` exactly, naming a date that exists.
fn date(s: &str) -> Option<jiff::civil::Date> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let part = |range: std::ops::Range<usize>| -> Option<i16> {
        let digits = s.get(range)?;
        if !digits.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
    };
    let (year, month, day) = (part(0..4)?, part(5..7)?, part(8..10)?);
    jiff::civil::Date::new(year, i8::try_from(month).ok()?, i8::try_from(day).ok()?).ok()
}

/// Spec §11.1's default `--since`, a function of the time and the zone alone: 30 days back, or
/// for `month` the first day of the eleventh month before `now_ms`'s, at midnight in `tz`, so
/// the report spans twelve calendar months with this one.
pub(crate) fn default_since_ms(period: &TokenPeriod, now_ms: i64, tz: &TimeZone) -> i64 {
    if !matches!(period, TokenPeriod::Month) {
        return now_ms.saturating_sub(DEFAULT_SINCE_MS);
    }
    let (year, month) = month_of(now_ms, tz);
    let index = i32::from(year) * 12 + i32::from(month) - 1 - 11;
    let (year, month) = (index.div_euclid(12), index.rem_euclid(12) + 1);
    month_bounds_ms(year as i16, month as i8, tz).0
}

/// `--model` (spec §11.1): each pattern names every indexed model that contains it, ignoring
/// case, and the report takes those exact names. A pattern that names none is a usage error
/// that lists the indexed models.
pub(crate) fn resolve_models(
    patterns: &[String],
    indexed: &[String],
) -> Result<Vec<String>, String> {
    let mut names = BTreeSet::new();
    for pattern in patterns {
        let needle = pattern.to_lowercase();
        let mut found = false;
        for model in indexed
            .iter()
            .filter(|m| m.to_lowercase().contains(&needle))
        {
            names.insert(model.clone());
            found = true;
        }
        if !found {
            let have = if indexed.is_empty() {
                "none".to_owned()
            } else {
                indexed.join(", ")
            };
            return Err(format!(
                "--model {pattern} matches no indexed model; the index has {have}"
            ));
        }
    }
    Ok(names.into_iter().collect())
}

/// Core §13.1 for `tokens`: `--provider` when given, else the provider of the accounts named
/// (`named`: each one's position and provider), else `default`. A named account of a provider
/// other than `--provider` is a usage error worded as `run` words it, and so are accounts of
/// more than one provider: a report covers one provider.
pub(crate) fn provider_for(
    flag: Option<&ProviderId>,
    default: &ProviderId,
    named: &[(u32, ProviderId)],
) -> Result<ProviderId, String> {
    if let Some(asked) = flag {
        if let Some((position, provider)) = named.iter().find(|(_, p)| p != asked) {
            return Err(format!(
                "position {position} is a {provider} account, not a {asked} one"
            ));
        }
        return Ok(asked.clone());
    }
    let mut providers: Vec<&ProviderId> = Vec::new();
    for (_, p) in named {
        if !providers.contains(&p) {
            providers.push(p);
        }
    }
    match providers.as_slice() {
        [] => Ok(default.clone()),
        [one] => Ok((*one).clone()),
        several => Err(format!(
            "the accounts named belong to more than one provider ({}); name one provider's accounts",
            several
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Spec §5.3, §11.2: brings the index up to date before the report, waiting for a refresh
/// another process runs. Progress goes to stderr only when stderr is a terminal and the
/// refresh runs past a second. An interruption keeps what was committed (spec §14).
pub(crate) fn refresh(
    engine: &Engine,
    provider: &ProviderId,
    err: &mut dyn Write,
) -> Result<(), EngineError> {
    let mut progress = Progress::new(err, std::io::stderr().is_terminal(), Instant::now());
    let outcome = engine.refresh_tokens(provider, Wait::Wait, None, engine.cancel(), &mut |p| {
        progress.show(p, Instant::now())
    });
    progress.clear();
    outcome.map(|_| ())
}

/// Spec §11.2: whether a refresh that began at `started` shows its progress at `now`: only on a
/// terminal, and only once it has run `PROGRESS_AFTER`.
fn shows_progress(started: Instant, now: Instant, terminal: bool) -> bool {
    terminal && now.saturating_duration_since(started) >= PROGRESS_AFTER
}

/// Refresh progress (spec §11.2): one line on stderr, rewritten in place, when
/// `shows_progress` says so.
struct Progress<'w> {
    err: &'w mut dyn Write,
    terminal: bool,
    started: Instant,
    shown: bool,
}

impl<'w> Progress<'w> {
    fn new(err: &'w mut dyn Write, terminal: bool, started: Instant) -> Self {
        Progress {
            err,
            terminal,
            started,
            shown: false,
        }
    }

    /// Shows `p`, as of `now`.
    fn show(&mut self, p: &IndexProgress, now: Instant) {
        if !shows_progress(self.started, now, self.terminal) {
            return;
        }
        let _ = write!(
            self.err,
            "\r\x1b[2Krefreshing the token index: {}/{} files",
            p.files_done, p.files_total
        );
        let _ = self.err.flush();
        self.shown = true;
    }

    /// Erases the progress line, when one was shown.
    fn clear(&mut self) {
        if self.shown {
            let _ = write!(self.err, "\r\x1b[2K");
            let _ = self.err.flush();
        }
    }
}

/// Spec §11.3, §14: what goes to stderr beside the report. A `--no-refresh` report of an
/// index that never completed a refresh is partial, and says so; skipped lines and unreadable
/// files are the first sign of transcript format drift.
pub(crate) fn notices(report: &TokenReport, no_refresh: bool) -> Vec<String> {
    let index = &report.index;
    let mut out = Vec::new();
    if no_refresh && index.refreshed_at_ms.is_none() {
        out.push(format!("note: {PARTIAL_INDEX}"));
    }
    if index.skipped_lines > 0 {
        out.push(format!(
            "warning: {} could not be read in the last refresh of the token index; Claude Code's transcript format may have changed",
            plural(index.skipped_lines, "transcript line")
        ));
    }
    if index.unreadable_files > 0 {
        out.push(format!(
            "warning: {} could not be read; the next refresh tries again",
            plural(index.unreadable_files, "transcript file")
        ));
    }
    out
}

fn plural(n: u64, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// A time as JSON and CSV spell it: ISO 8601 UTC, to the second.
fn iso(ms: i64) -> String {
    format_iso8601(ms.div_euclid(1000))
}

/// Nano-dollars as US dollars, for JSON and CSV.
fn usd(nanos: u64) -> f64 {
    nanos as f64 / 1e9
}

fn period_name(p: &TokenPeriod) -> &'static str {
    match p {
        TokenPeriod::Day => "day",
        TokenPeriod::Month => "month",
        TokenPeriod::Session => "session",
        TokenPeriod::Block => "block",
    }
}

fn grouping_name(g: &TokenGrouping) -> &'static str {
    match g {
        TokenGrouping::None => "none",
        TokenGrouping::Account => "account",
        TokenGrouping::Model => "model",
        TokenGrouping::AccountModel => "account,model",
    }
}

fn basis_name(b: Basis) -> &'static str {
    match b {
        Basis::Profile => "profile",
        Basis::Timeline => "timeline",
        Basis::Unattributed => "unattributed",
    }
}

fn day_text(date: &jiff::civil::Date) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

fn month_text(year: i16, month: i8) -> String {
    format!("{year:04}-{month:02}")
}

/// Spec §11.2's `--json`: the report of spec §10 in camelCase (core §13.2), times in ISO 8601
/// UTC, dates and months in `timeZone`.
pub(crate) fn json(report: &TokenReport) -> Value {
    let q = &report.query;
    let rows: Vec<Value> = report.rows.iter().map(row_json).collect();
    json!({
        "schemaVersion": 1,
        "provider": q.provider.as_str(),
        "period": period_name(&q.period),
        "grouping": grouping_name(&q.grouping),
        "timeZone": report.time_zone,
        "since": q.since_ms.map(iso),
        "until": q.until_ms.map(iso),
        "rows": rows,
        "total": total_json(&report.total),
        "index": index_json(&report.index),
        "unpricedModels": report.unpriced_models,
        "prices": {"asOf": report.prices_as_of, "overridden": report.price_overrides},
    })
}

fn row_json(row: &TokenRow) -> Value {
    let mut o = json!({"key": row.key.as_str(), "period": period_json(&row.period)});
    if let Some(a) = &row.account {
        o["account"] = account_json(a);
    }
    if let Some(m) = &row.model {
        o["model"] = json!(m);
    }
    let accounts: Vec<Value> = row.accounts.iter().map(account_json).collect();
    o["accounts"] = json!(accounts);
    o["models"] = json!(row.models);
    o["tokens"] = tokens_json(&row.counts);
    o["responses"] = json!(row.responses);
    o["unfinalized"] = json!(row.unfinalized);
    o["costUsd"] = cost_json(row.cost.as_ref());
    o["costPartial"] = json!(row.cost.as_ref().is_some_and(|c| c.partial));
    o
}

fn period_json(p: &PeriodKey) -> Value {
    match p {
        PeriodKey::Day { date } => json!({"kind": "day", "date": day_text(date)}),
        PeriodKey::Month { year, month } => {
            json!({"kind": "month", "month": month_text(*year, *month)})
        }
        PeriodKey::Session {
            session_id,
            project,
            first_ms,
            last_ms,
        } => json!({
            "kind": "session",
            "sessionId": session_id,
            "project": project,
            "first": iso(*first_ms),
            "last": iso(*last_ms),
        }),
        PeriodKey::Block {
            start_ms,
            end_ms,
            observed,
            active,
        } => json!({
            "kind": "block",
            "start": iso(*start_ms),
            "end": iso(*end_ms),
            "observed": observed,
            "active": active.as_ref().map(active_json),
        }),
    }
}

fn active_json(a: &ActiveBlock) -> Value {
    json!({
        "elapsedSeconds": a.elapsed_s,
        "remainingSeconds": a.remaining_s,
        "tokensPerMinute": a.tokens_per_min,
        "costPerHourUsd": cost_json(a.cost_per_hour.as_ref()),
        "projectedTokens": a.projected_tokens,
        "projectedCostUsd": cost_json(a.projected_cost.as_ref()),
    })
}

/// Spec §11.2: `{kind: "account", number, id, email, alias?}`, `{kind: "removed", id}`, or
/// `{kind: "unattributed"}`.
fn account_json(a: &RowAccount) -> Value {
    match a {
        RowAccount::Account {
            id,
            position,
            label,
            alias,
        } => {
            let mut o =
                json!({"kind": "account", "number": position, "id": id.as_str(), "email": label});
            if let Some(alias) = alias {
                o["alias"] = json!(alias);
            }
            o
        }
        RowAccount::Removed { id } => json!({"kind": "removed", "id": id.as_str()}),
        RowAccount::Unattributed => json!({"kind": "unattributed"}),
    }
}

fn tokens_json(c: &TokenCounts) -> Value {
    json!({
        "input": c.input,
        "output": c.output,
        "cacheWrite5m": c.cache_write_5m,
        "cacheWrite1h": c.cache_write_1h,
        "cacheWrite": c.cache_write(),
        "cacheRead": c.cache_read,
        "total": c.total(),
        "webSearch": c.web_search,
    })
}

fn cost_json(cost: Option<&Cost>) -> Value {
    cost.map_or(Value::Null, |c| json!(usd(c.usd_nanos)))
}

fn total_json(t: &TokenTotal) -> Value {
    let by_basis: Map<String, Value> = t
        .by_basis
        .iter()
        .map(|(basis, tokens)| (basis_name(*basis).to_owned(), json!(tokens)))
        .collect();
    json!({
        "tokens": tokens_json(&t.counts),
        "responses": t.responses,
        "unfinalized": t.unfinalized,
        "costUsd": cost_json(t.cost.as_ref()),
        "costPartial": t.cost.as_ref().is_some_and(|c| c.partial),
        "byBasis": by_basis,
    })
}

/// `IndexState` as spec §12's `token-index` event spells it, less the provider.
fn index_json(i: &IndexState) -> Value {
    json!({
        "refreshedAt": i.refreshed_at_ms.map(iso),
        "changedAt": i.changed_at_ms.map(iso),
        "files": i.files,
        "responses": i.responses,
        "skippedLines": i.skipped_lines,
        "unreadableFiles": i.unreadable_files,
        "oldest": i.oldest_ms.map(iso),
        "newest": i.newest_ms.map(iso),
    })
}

/// Spec §11.2's `--csv`: one line per report row, the period's own columns last. The account
/// is its email, `unattributed` or `removed:<id>`, and a field the rows are not grouped by is
/// empty.
pub(crate) fn csv(report: &TokenReport) -> String {
    let mut s = String::from(
        "period,account,model,input,output,cache_write_5m,cache_write_1h,cache_read,web_search,total,responses,unfinalized,cost_usd",
    );
    s.push_str(match report.query.period {
        TokenPeriod::Session => ",project,first_at,last_at",
        TokenPeriod::Block => ",start,end,observed,active",
        TokenPeriod::Day | TokenPeriod::Month => "",
    });
    s.push('\n');
    for row in &report.rows {
        let c = &row.counts;
        let mut fields = vec![
            csv_period(&row.period),
            row.account.as_ref().map_or_else(String::new, csv_account),
            row.model.clone().unwrap_or_default(),
            c.input.to_string(),
            c.output.to_string(),
            c.cache_write_5m.to_string(),
            c.cache_write_1h.to_string(),
            c.cache_read.to_string(),
            c.web_search.to_string(),
            c.total().to_string(),
            row.responses.to_string(),
            row.unfinalized.to_string(),
            row.cost
                .as_ref()
                .map_or_else(String::new, |cost| usd(cost.usd_nanos).to_string()),
        ];
        match &row.period {
            PeriodKey::Session {
                project,
                first_ms,
                last_ms,
                ..
            } => fields.extend([
                project.clone().unwrap_or_default(),
                iso(*first_ms),
                iso(*last_ms),
            ]),
            PeriodKey::Block {
                start_ms,
                end_ms,
                observed,
                active,
            } => fields.extend([
                iso(*start_ms),
                iso(*end_ms),
                observed.to_string(),
                active.is_some().to_string(),
            ]),
            PeriodKey::Day { .. } | PeriodKey::Month { .. } => {}
        }
        let quoted: Vec<String> = fields.iter().map(|f| history::csv_field(f)).collect();
        s.push_str(&quoted.join(","));
        s.push('\n');
    }
    s
}

fn csv_period(p: &PeriodKey) -> String {
    match p {
        PeriodKey::Day { date } => day_text(date),
        PeriodKey::Month { year, month } => month_text(*year, *month),
        PeriodKey::Session { session_id, .. } => session_id.clone(),
        PeriodKey::Block { start_ms, .. } => iso(*start_ms),
    }
}

fn csv_account(a: &RowAccount) -> String {
    match a {
        RowAccount::Account { label, .. } => label.clone(),
        RowAccount::Removed { id } => format!("removed:{}", id.as_str()),
        RowAccount::Unattributed => "unattributed".to_owned(),
    }
}

/// One column of the human table: its heading, and whether its cells are numbers, which are
/// right-aligned.
struct Col {
    head: &'static str,
    right: bool,
}

const fn left(head: &'static str) -> Col {
    Col { head, right: false }
}

const fn right(head: &'static str) -> Col {
    Col { head, right: true }
}

/// Spec §11.2's table: one row per report row, then the total, then the footnotes. Token
/// counts are scaled (`12.4M`) and costs are dollars with cents, `*` marking a partial one.
/// Session rows add their first and last times and their project, block rows their end and a
/// note. Times are in the report's zone.
pub(crate) fn human(report: &TokenReport) -> String {
    if report.rows.is_empty() {
        return NO_USAGE.to_owned();
    }
    let q = &report.query;
    let by_model = report.rows.iter().any(|r| r.model.is_some());
    let mark = report
        .rows
        .iter()
        .filter_map(|r| r.cost.as_ref())
        .chain(report.total.cost.as_ref())
        .any(|c| c.partial);
    let mut cols = match q.period {
        TokenPeriod::Day => vec![left("DAY")],
        TokenPeriod::Month => vec![left("MONTH")],
        TokenPeriod::Session => vec![left("SESSION"), left("FIRST"), left("LAST")],
        TokenPeriod::Block => vec![left("START"), left("END")],
    };
    let lead = cols.len();
    cols.push(left("ACCOUNT"));
    if by_model {
        cols.push(left("MODEL"));
    }
    cols.extend([
        right("INPUT"),
        right("OUTPUT"),
        right("CACHE WRITE"),
        right("CACHE READ"),
        right("TOTAL"),
        right("API COST"),
    ]);
    let trailing = match q.period {
        TokenPeriod::Session => Some(left("PROJECT")),
        TokenPeriod::Block => Some(left("")),
        TokenPeriod::Day | TokenPeriod::Month => None,
    };
    let has_trailing = trailing.is_some();
    cols.extend(trailing);
    let mut rows: Vec<Vec<String>> = Vec::new();
    for row in &report.rows {
        let mut cells = period_cells(&row.period, &q.tz);
        cells.push(account_cell(row));
        if by_model {
            cells.push(row.model.clone().unwrap_or_else(|| row.models.join(", ")));
        }
        cells.extend(count_cells(&row.counts));
        cells.push(cost_cell(row.cost.as_ref(), mark));
        match &row.period {
            PeriodKey::Session { project, .. } => {
                cells.push(project.clone().unwrap_or_else(|| MISSING.to_owned()));
            }
            PeriodKey::Block {
                observed, active, ..
            } => cells.push(block_note(*observed, active.as_ref())),
            PeriodKey::Day { .. } | PeriodKey::Month { .. } => {}
        }
        rows.push(cells);
    }
    let mut total = vec!["TOTAL".to_owned()];
    total.resize(lead + 1 + usize::from(by_model), String::new());
    total.extend(count_cells(&report.total.counts));
    total.push(cost_cell(report.total.cost.as_ref(), mark));
    if has_trailing {
        total.push(String::new());
    }
    rows.push(total);
    format!("{}{}", table(&cols, &rows), footnotes(report))
}

/// The cell of `row` in column `i`, or nothing.
fn cell(row: &[String], i: usize) -> &str {
    row.get(i).map_or("", String::as_str)
}

/// Each column padded to its widest cell, two spaces apart, numbers right-aligned, as `list`
/// lays out its table (`render::width` counts a wide character as two).
fn table(cols: &[Col], rows: &[Vec<String>]) -> String {
    let widths: Vec<usize> = cols
        .iter()
        .enumerate()
        .map(|(i, c)| {
            rows.iter()
                .map(|r| render::width(cell(r, i)))
                .fold(render::width(c.head), usize::max)
        })
        .collect();
    let line = |texts: Vec<&str>| {
        let mut s = String::new();
        for (i, ((text, col), w)) in texts.iter().zip(cols).zip(&widths).enumerate() {
            if i > 0 {
                s.push_str("  ");
            }
            let gap = " ".repeat(w.saturating_sub(render::width(text)));
            if col.right {
                s.push_str(&gap);
                s.push_str(text);
            } else {
                s.push_str(text);
                s.push_str(&gap);
            }
        }
        format!("{}\n", s.trim_end())
    };
    let mut out = line(cols.iter().map(|c| c.head).collect());
    for r in rows {
        out.push_str(&line((0..cols.len()).map(|i| cell(r, i)).collect()));
    }
    out
}

fn period_cells(p: &PeriodKey, tz: &TimeZone) -> Vec<String> {
    match p {
        PeriodKey::Day { date } => vec![day_text(date)],
        PeriodKey::Month { year, month } => vec![month_text(*year, *month)],
        PeriodKey::Session {
            session_id,
            first_ms,
            last_ms,
            ..
        } => vec![
            session_id.clone(),
            local(*first_ms, tz),
            local(*last_ms, tz),
        ],
        PeriodKey::Block {
            start_ms, end_ms, ..
        } => vec![local(*start_ms, tz), local(*end_ms, tz)],
    }
}

/// `ms` in `tz`, to the minute.
fn local(ms: i64, tz: &TimeZone) -> String {
    jiff::Timestamp::from_millisecond(ms).map_or_else(
        |_| MISSING.to_owned(),
        |t| {
            t.to_zoned(tz.clone())
                .strftime("%Y-%m-%d %H:%M")
                .to_string()
        },
    )
}

/// The row's account (its position and label), or, when the rows are not grouped by account,
/// every account it holds.
fn account_cell(row: &TokenRow) -> String {
    match &row.account {
        Some(a) => account_text(a),
        None => row
            .accounts
            .iter()
            .map(account_text)
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn account_text(a: &RowAccount) -> String {
    match a {
        RowAccount::Account {
            position,
            label,
            alias,
            ..
        } => match alias {
            Some(alias) => format!("{position} {alias} ({label})"),
            None => format!("{position} {label}"),
        },
        RowAccount::Removed { id } => format!("removed {}", id.as_str()),
        RowAccount::Unattributed => "unattributed".to_owned(),
    }
}

fn count_cells(c: &TokenCounts) -> [String; 5] {
    [
        scaled(c.input),
        scaled(c.output),
        scaled(c.cache_write()),
        scaled(c.cache_read),
        scaled(c.total()),
    ]
}

/// A cost cell: dollars and cents, `*` when partial, `—` when nothing in it has a price. With
/// `mark` (some cost in the table is partial), a whole cost ends in a space, so the cents
/// line up.
fn cost_cell(cost: Option<&Cost>, mark: bool) -> String {
    let (text, partial) = match cost {
        Some(c) => (dollars(c.usd_nanos), c.partial),
        None => (MISSING.to_owned(), false),
    };
    match (partial, mark) {
        (true, _) => format!("{text}*"),
        (false, true) => format!("{text} "),
        (false, false) => text,
    }
}

/// A token count as the table shows it (spec §11.2): as is below 1,000, else to one decimal
/// in thousands, millions, billions or trillions (`12.4M`), rounded half up.
pub(crate) fn scaled(n: u64) -> String {
    const UNITS: [(u128, char); 4] = [
        (1_000, 'K'),
        (1_000_000, 'M'),
        (1_000_000_000, 'B'),
        (1_000_000_000_000, 'T'),
    ];
    if n < 1_000 {
        return n.to_string();
    }
    let tenths = |unit: u128| (u128::from(n) * 10 + unit / 2) / unit;
    let (unit, suffix) = UNITS
        .iter()
        .copied()
        .find(|&(unit, _)| tenths(unit) < 10_000)
        .unwrap_or(UNITS[3]);
    let t = tenths(unit);
    format!("{}.{}{suffix}", t / 10, t % 10)
}

/// Nano-dollars as dollars and cents (`$312.40`), rounded half up.
pub(crate) fn dollars(nanos: u64) -> String {
    let cents = (u128::from(nanos) + 5_000_000) / 10_000_000;
    format!("${}.{:02}", cents / 100, cents % 100)
}

/// A block row's note (spec §11.2): `observed` for a block a reset time placed (spec §9), and
/// for the active block its time left, burn rate and projection.
pub(crate) fn block_note(observed: bool, active: Option<&ActiveBlock>) -> String {
    let mut parts = Vec::new();
    if observed {
        parts.push("observed".to_owned());
    }
    if let Some(a) = active {
        let cost =
            |c: Option<&Cost>| c.map_or_else(|| MISSING.to_owned(), |c| dollars(c.usd_nanos));
        parts.push(format!(
            "active: {} left, {} tokens/min, {}/h, projected {} tokens, {}",
            render::duration(a.remaining_s),
            scaled(a.tokens_per_min.max(0.0).round() as u64),
            cost(a.cost_per_hour.as_ref()),
            scaled(a.projected_tokens),
            cost(a.projected_cost.as_ref()),
        ));
    }
    parts.join("; ")
}

/// Spec §11.2's footnotes, after a blank line: unpriced models, responses whose output is a
/// lower bound (spec §2.3), skipped lines, and the date of the price table.
fn footnotes(report: &TokenReport) -> String {
    let mut notes = Vec::new();
    if !report.unpriced_models.is_empty() {
        notes.push(format!(
            "No price for {}: counted without cost (* marks a partial cost).",
            report.unpriced_models.join(", ")
        ));
    }
    if report.total.unfinalized > 0 {
        notes.push(format!(
            "Output is a lower bound for {}.",
            plural(report.total.unfinalized, "subagent response")
        ));
    }
    if report.index.skipped_lines > 0 {
        notes.push(format!(
            "{} could not be read, so these totals may be low.",
            plural(report.index.skipped_lines, "transcript line")
        ));
    }
    notes.push(if report.price_overrides.is_empty() {
        format!("API cost at list prices as of {}.", report.prices_as_of)
    } else {
        format!(
            "API cost at list prices as of {}, with config.toml's for {}.",
            report.prices_as_of,
            report.price_overrides.join(", ")
        )
    });
    format!("\n{}\n", notes.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_scale_to_one_decimal_from_a_thousand() {
        for (n, text) in [
            (0, "0"),
            (999, "999"),
            (1_000, "1.0K"),
            (1_234, "1.2K"),
            (1_250, "1.3K"),
            (51_000, "51.0K"),
            (999_949, "999.9K"),
            (999_950, "1.0M"),
            (12_400_000, "12.4M"),
            (3_100_000_000, "3.1B"),
            (999_950_000_000, "1.0T"),
            (u64::MAX, "18446744.1T"),
        ] {
            assert_eq!(scaled(n), text, "{n}");
        }
    }

    #[test]
    fn costs_are_dollars_and_cents_rounded_half_up() {
        assert_eq!(dollars(0), "$0.00");
        assert_eq!(dollars(4_999_999), "$0.00");
        assert_eq!(dollars(5_000_000), "$0.01");
        assert_eq!(dollars(622_016_000), "$0.62");
        assert_eq!(dollars(312_400_000_000), "$312.40");
        assert_eq!(dollars(u64::MAX), "$18446744073.71");
        let partial = Cost {
            usd_nanos: 97_000_000,
            partial: true,
        };
        let whole = Cost {
            usd_nanos: 97_000_000,
            partial: false,
        };
        assert_eq!(cost_cell(Some(&partial), true), "$0.10*");
        assert_eq!(cost_cell(Some(&whole), true), "$0.10 ");
        assert_eq!(cost_cell(Some(&whole), false), "$0.10");
        assert_eq!(cost_cell(None, false), "—");
    }

    #[test]
    fn times_take_spans_dates_at_local_midnight_and_iso_times() {
        let now = 1_790_000_000_000; // 2026-09-21T14:13:20Z
        let utc = TimeZone::UTC;
        let kolkata = TimeZone::get("Asia/Kolkata").unwrap();
        assert_eq!(parse_time("30d", now, &utc), Some(now - 30 * 86_400_000));
        assert_eq!(parse_time("90m", now, &utc), Some(now - 5_400_000));
        assert_eq!(parse_time("2026-09-21", now, &utc), Some(1_789_948_800_000));
        assert_eq!(
            parse_time("2026-09-21", now, &kolkata),
            Some(1_789_948_800_000 - 19_800_000),
            "midnight in the zone, 05:30 ahead of UTC"
        );
        assert_eq!(
            parse_time("2026-09-21T14:13:20Z", now, &kolkata),
            Some(now),
            "an ISO time carries its own offset"
        );
        for bad in [
            "",
            "7w",
            "0d",
            "2026-02-30",
            "2026-9-21",
            "21-09-2026",
            "2026-09-21 14:13",
            "yesterday",
        ] {
            assert_eq!(parse_time(bad, now, &utc), None, "{bad:?}");
        }
    }

    fn args(tz: Option<&str>, by: Option<&str>) -> TokensArgs {
        TokensArgs {
            accounts: vec![],
            unattributed: false,
            model: vec![],
            since: None,
            until: None,
            by: by.map(str::to_owned),
            tz: tz.map(str::to_owned),
            csv: false,
            no_refresh: false,
        }
    }

    #[test]
    fn flags_take_their_defaults_and_refuse_what_they_cannot_read() {
        let now = 1_790_000_000_000;
        let day = |tz, by| parse_flags(&args(tz, by), &TokenPeriod::Day, now);
        let f = day(Some("UTC"), None).unwrap();
        assert_eq!((f.since_ms, f.until_ms), (now - 30 * 86_400_000, None));
        assert!(matches!(f.grouping, TokenGrouping::Account));
        assert!(matches!(
            day(None, Some("account,model")).unwrap().grouping,
            TokenGrouping::AccountModel
        ));
        assert_eq!(day(None, Some("week")).err().as_deref(), Some(BAD_BY));
        assert_eq!(
            day(Some("Mars/Olympus_Mons"), None).err().as_deref(),
            Some("unknown time zone \"Mars/Olympus_Mons\"")
        );
    }

    /// `s`, an ISO 8601 time, in epoch ms.
    fn at(s: &str) -> i64 {
        parse_iso8601_ms(s).unwrap()
    }

    #[test]
    fn default_ranges_follow_the_zones_calendar() {
        // Spec §11.1, at fixed instants: 30 days back for every period but month, and for
        // month the first of the eleventh month back, at midnight where the report's days are.
        let utc = TimeZone::UTC;
        let brussels = TimeZone::get("Europe/Brussels").unwrap();
        let since = |period: TokenPeriod, now: &str, tz: &TimeZone| {
            iso(default_since_ms(&period, at(now), tz))
        };
        let now = "2026-09-21T14:13:20Z";
        assert_eq!(since(TokenPeriod::Day, now, &utc), "2026-08-22T14:13:20Z");
        assert_eq!(
            since(TokenPeriod::Block, now, &brussels),
            "2026-08-22T14:13:20Z",
            "30 days of 24 hours, whatever the zone"
        );
        assert_eq!(since(TokenPeriod::Month, now, &utc), "2025-10-01T00:00:00Z");
        // Across the turn of a month and of a year.
        assert_eq!(
            since(TokenPeriod::Month, "2026-02-28T23:59:59.999Z", &utc),
            "2025-03-01T00:00:00Z"
        );
        assert_eq!(
            since(TokenPeriod::Month, "2026-03-01T00:00:00Z", &utc),
            "2025-04-01T00:00:00Z"
        );
        assert_eq!(
            since(TokenPeriod::Month, "2026-01-15T12:00:00Z", &utc),
            "2025-02-01T00:00:00Z"
        );
        // 23:30 UTC on 31 October is already 1 November in Brussels, on winter time (UTC+1):
        // the range starts on 1 December 2025 at its midnight there.
        assert_eq!(
            since(TokenPeriod::Month, "2026-10-31T23:30:00Z", &brussels),
            "2025-11-30T23:00:00Z"
        );
        assert_eq!(
            since(TokenPeriod::Month, "2026-10-31T23:30:00Z", &utc),
            "2025-11-01T00:00:00Z",
            "still October in UTC"
        );
        // From late March 2026, eleven months back is April 2025, on summer time (UTC+2).
        assert_eq!(
            since(TokenPeriod::Month, "2026-03-29T12:00:00Z", &brussels),
            "2025-03-31T22:00:00Z"
        );
    }

    #[test]
    fn a_model_pattern_names_every_model_containing_it() {
        let indexed: Vec<String> = ["claude-opus-5-5", "claude-sonnet-5-5", "claude-test-9"]
            .map(str::to_owned)
            .to_vec();
        let pick = |p: &[&str]| {
            resolve_models(
                &p.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
                &indexed,
            )
        };
        assert_eq!(pick(&["OPUS"]).unwrap(), ["claude-opus-5-5"]);
        assert_eq!(
            pick(&["5-5", "opus"]).unwrap(),
            ["claude-opus-5-5", "claude-sonnet-5-5"],
            "sorted, each once"
        );
        assert_eq!(
            pick(&["opus", "nope"]).unwrap_err(),
            "--model nope matches no indexed model; the index has claude-opus-5-5, claude-sonnet-5-5, claude-test-9"
        );
        assert_eq!(
            resolve_models(&["x".to_owned()], &[]).unwrap_err(),
            "--model x matches no indexed model; the index has none"
        );
    }

    #[test]
    fn progress_shows_only_on_a_terminal_once_a_second_has_passed() {
        // Fixed instants: nothing here sleeps or reads how long anything took.
        let start = Instant::now();
        let later = |ms: u64| start + Duration::from_millis(ms);
        assert!(!shows_progress(start, later(999), true));
        assert!(shows_progress(start, later(1_000), true));
        assert!(
            !shows_progress(start, later(5_000), false),
            "never off a terminal"
        );
        assert!(
            !shows_progress(later(10), start, true),
            "a clock read before the start shows nothing"
        );
        let p = IndexProgress {
            files_done: 3,
            files_total: 9,
            ..IndexProgress::default()
        };
        let mut out = Vec::new();
        let mut progress = Progress::new(&mut out, true, start);
        progress.show(&p, later(500));
        progress.show(&p, later(1_500));
        progress.clear();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\r\x1b[2Krefreshing the token index: 3/9 files\r\x1b[2K"
        );
    }

    #[test]
    fn named_accounts_choose_the_provider() {
        // Core §13.1: --provider, else the named accounts' provider, else the default.
        let (cc, fake) = (
            ProviderId::new("claude-code"),
            ProviderId::new("fake-agent"),
        );
        let named = |list: &[(u32, &ProviderId)]| {
            list.iter()
                .map(|(n, p)| (*n, (*p).clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(provider_for(None, &cc, &[]), Ok(cc.clone()));
        assert_eq!(provider_for(Some(&fake), &cc, &[]), Ok(fake.clone()));
        assert_eq!(
            provider_for(None, &cc, &named(&[(2, &fake)])),
            Ok(fake.clone())
        );
        assert_eq!(
            provider_for(Some(&cc), &cc, &named(&[(1, &cc), (2, &cc)])),
            Ok(cc.clone())
        );
        assert_eq!(
            provider_for(Some(&cc), &cc, &named(&[(1, &cc), (2, &fake)])),
            Err("position 2 is a fake-agent account, not a claude-code one".to_owned())
        );
        assert_eq!(
            provider_for(None, &cc, &named(&[(1, &cc), (2, &fake), (3, &cc)])),
            Err("the accounts named belong to more than one provider (claude-code, fake-agent); name one provider's accounts".to_owned())
        );
    }

    #[test]
    fn a_block_note_says_observed_and_what_the_active_block_projects() {
        assert_eq!(block_note(false, None), "");
        assert_eq!(block_note(true, None), "observed");
        let active = ActiveBlock {
            elapsed_s: 9_000,
            remaining_s: 9_000,
            tokens_per_min: 41_234.6,
            cost_per_hour: Some(Cost {
                usd_nanos: 3_200_000_000,
                partial: false,
            }),
            projected_tokens: 9_800_000,
            projected_cost: None,
        };
        assert_eq!(
            block_note(true, Some(&active)),
            "observed; active: 2h30m left, 41.2K tokens/min, $3.20/h, projected 9.8M tokens, —"
        );
    }
}
```

- [ ] **Step 4: Run them and see them pass**

```
cargo test -p tagteam --locked --features test-support --test tokens_cli --test completions
cargo test -p tagteam --locked --features test-support --lib
```

Expected: `tokens_cli` 11 passed, `completions` passing as before (the new subcommands, flags and `--by` values appear in every script; `account,model` is a hidden value, so the fish names stay unquoted), and the lib's tests passing, `tokens_cmd::tests` (9) and `cli::tests::tokens_takes_its_period_as_a_subcommand_and_is_day_when_bare` among them.

- [ ] **Step 5: Lint**

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings
```

- [ ] **Step 6: Commit**

```
git add Cargo.lock crates/tagteam/Cargo.toml crates/tagteam/src/cli.rs crates/tagteam/src/app.rs crates/tagteam/src/lib.rs crates/tagteam/src/history.rs crates/tagteam/src/tokens_cmd.rs crates/tagteam/tests/common/mod.rs crates/tagteam/tests/tokens_cli.rs
git commit -m "Add tagteam tokens"
```

#### Notes for the controller

1. **Batch G shares a crate.** Tasks 19 and 21 both change the `tagteam` crate, and every `tagteam` test target compiles the whole library and `tests/common/mod.rs`, which this task extends. A half-written `tokens_cmd.rs` or `cli.rs` breaks Task 21's build, and the reverse. Run 19 before 21 in that batch, or have each implementer re-run its focused tests once both are in.
2. **Choices inside spec §11 for the PR's rulings:** the `period`, `total` and `index` JSON shapes above; the table's column placement; `--by` parsed by the command (any string reaches it, as `--include-api-key-accounts` does), with `account,model` hidden from completion because `completions.rs` requires every offered value to need no fish quoting; the `tz` usage error `unknown time zone "<name>"` and the silent UTC fallback (from the coordinator's time-zone ruling), done with `TimeZone::try_system()`; `--since` and `--until` both taking spans; footnote and notice wording.
3. **`Command::Tokens` boxes its period** (`Option<Box<TokensPeriod>>`): unboxed, the variant holds two `TokensArgs` and clippy's `large_enum_variant` fails `-D warnings`. clap implements `Subcommand` for `Box<T>`.
4. **Progress and the terminal.** The progress line checks `std::io::stderr().is_terminal()` where it runs rather than adding a `stderr_terminal` field to `app::Context`, which every in-process test builds by hand.
5. **Attribution in the fixture** does not depend on ruling R1's `seeded` flag: the unattributed session ran before the store existed and before either account's `add`, so with coverage complete it falls to the default home's timeline, which names no account before the first `add`, and with coverage incomplete it is unattributed outright.
6. **The head's Review Focus owners are stale.** Focus 4 names Task 17 for `a_since_date_is_local_midnight_in_tz`, which is this task's; likewise Focus 1 names Tasks 11 and 12 for tests the outline gives Tasks 14 and 15, and Focus 5 names Task 15 for `purge_waits_for_a_running_refresh`, which is Task 17's.
7. **Provider selection (core §13.1).** `--provider` wins; else the accounts named choose it; else the default provider. A position resolves within `--provider` (else the default provider), an alias or an email in whichever provider holds it, so `provider_for` can refuse a named account that is not `--provider`'s (worded as `run` words it, but a usage error, exit 2) and accounts of two providers at once. `App::resolve` is split into `resolve_in(input, provider)` for this; every other command's resolution is unchanged. The binary registers Claude Code alone, so the CLI test plants a `fake-agent` account in the store: naming it alone then fails as `unknown provider "fake-agent"`, which shows it chose the provider.
8. **What was verified.** In a scratch workspace with stand-ins for `jiff`, `tagteam_core` and the engine that follow task-18.md's types: `tokens_cmd.rs` exactly as above compiled, passed its 9 unit tests (the default ranges' Brussels cases against a stand-in zone that keeps EU summer time), `clippy -D warnings` and `rustfmt --check`, and rendered every expected table and CSV in `tokens_cli.rs`; the patched `cli.rs` (the real file plus the hunks) compiled against real clap 4.6.7, passed `Command::debug_assert`, its unit tests, clippy and rustfmt, and its bash, zsh and fish scripts offer `tokens`, its periods and `account`, `model`, `none` but not `account,model`; `tokens_cli.rs` and the `common` helpers type-checked and passed clippy against stubs of `common` and the store API. Not run: anything against the real engine, store, binary or `jiff` 0.2.37, since Tasks 2–18 do not exist yet. `app.rs`'s hunks were applied to the real file and pass `rustfmt --check`, but were not compiled.

---

### Task 20: The statusline sighting

**Depends on:** 16, 19
**Files:**
- Modify: `crates/tagteam/src/app.rs` (`CLAUDE_CODE_SESSION_ID` joins `session_vars`, captured at the process boundary; `run_statusline` records the session once the line is out; the boundary test's expected names)
- Modify: `crates/tagteam/src/statusline.rs` (`SightingHome`, `sighting_home`, `same_dir`, `record_session`; the probe's comment in `engine`; a unit test)
- Test: `crates/tagteam/tests/statusline.rs` (four tests), `crates/tagteam/tests/perf.rs` (two release-only timing tests), `crates/tagteam/src/statusline.rs` and `crates/tagteam/src/app.rs` (`mod tests`)

**Interfaces:**
- Consumes:
  ```rust
  // Task 16, tagteam_engine::tokens (an Engine method)
  /// The statusline sighting (spec §6.3). `profile`: the run-shell profile, or None for the default home.
  /// Never waits more than CACHE_WRITE_WAIT.
  pub fn statusline_sighting(&self, provider: &ProviderId, session_id: &str, profile: Option<&Path>) -> Result<(), EngineError>;
  // Task 7: Capabilities.transcripts; ProcessProbe::parent_pid, which SystemProcessProbe answers
  // Task 12, tagteam_engine::store (tests)
  pub fn sightings(&self, provider: &ProviderId) -> Result<Vec<SightingRow>, StoreError>;
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct SightingRow { pub session_id: String, pub home: String, pub first_at: i64, pub last_at: i64, pub source: SightingSource, pub recorded_at: i64 }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum SightingSource { Run, Scan, Statusline }
  // Task 19: `app.rs` as Task 19 leaves it; tests/common/mod.rs's `cc_profile` (existing)
  ```
- Produces:
  ```rust
  // crate::statusline
  pub(crate) enum SightingHome { Default, Profile(PathBuf), Elsewhere }
  pub(crate) fn sighting_home(shell: &RunShell, env: &Env) -> SightingHome;
  pub(crate) fn record_session(engine: &Engine, provider: &ProviderId, session_id: Option<&str>);
  // Env.vars captures CLAUDE_CODE_SESSION_ID (app::session_vars)
  ```

Spec §6.3 decides the home here, in the CLI: the run shell's profile, else the default home when `CLAUDE_CONFIG_DIR` is unset or names it, else nothing. Task 16's `statusline_sighting` takes the answer (`Option<&Path>`) and does the lookup, the ancestor walk and the write. The default home is `CcPaths::resolve` of the environment without `CLAUDE_CONFIG_DIR`, as `statusline.rs` already resolves Claude Code's paths (`config_hint`); two spellings name it when they are equal or canonicalize to one directory.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam/tests/statusline.rs`, widen two imports:

```rust
use common::{
    cc_profile, cmd, login, now_epoch_s, record_reading, seed_home, std_cmd, two_fresh_accounts,
    usage_window,
};
```

(was `use common::{ cmd, login, now_epoch_s, record_reading, seed_home, std_cmd, two_fresh_accounts, usage_window, };`) and

```rust
use tagteam_engine::store::{SightingRow, SightingSource, Store};
```

(was `use tagteam_engine::store::Store;`). Then append to the end of the file. The tests run the binary directly, so its parent is the test process; each test writes Claude Code's session record for that pid in its own temp home, and the line's ancestor walk finds it there.

```rust
/// Claude Code's session record for this test process, which is the line's parent, in
/// `config_home`: the ancestor a sighting is keyed by (token report spec §6.3). Returns its
/// `startedAt`.
fn record_for_this_process(config_home: &Path, session_id: &str) -> i64 {
    let dir = config_home.join("sessions");
    fs::create_dir_all(&dir).unwrap();
    let pid = std::process::id();
    let started = now_epoch_s() * 1000 - 60_000;
    let record = json!({"pid": pid, "sessionId": session_id, "cwd": "/w",
                        "startedAt": started, "kind": "interactive"});
    fs::write(dir.join(format!("{pid}.json")), record.to_string()).unwrap();
    started
}

fn sightings(root: &Path) -> Vec<SightingRow> {
    store(root)
        .sightings(&ProviderId::new(CLAUDE_CODE))
        .unwrap()
}

#[test]
fn the_statusline_records_its_session_once_a_minute() {
    // Token report spec §6.3: Claude Code runs the line with its session's environment. The
    // line's first ancestor with a session record, this test process, keys the sighting. The
    // binary has no clock to set, so only what holds at any time is asserted here: one
    // execution is one sighting, whenever the line runs again. Whether a second look extends it
    // (a minute on) or only reads it (within one) is Task 16's, tested on the fake clock.
    let (d, _, _) = managed(0);
    let root = d.path();
    let started = record_for_this_process(&Env::for_test(root).home.join(".claude"), "s-line");
    let line = || {
        statusline(root)
            .arg("--no-color")
            .env("CLAUDE_CODE_SESSION_ID", "s-line")
            .assert()
            .success()
            .stdout("b · 5h 9% · 7d 77%\n")
            .stderr("");
    };
    statusline(root).arg("--no-color").assert().success();
    assert!(
        sightings(root).is_empty(),
        "no session id, nothing recorded"
    );
    line();
    let first = sightings(root);
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(
        (
            first[0].session_id.as_str(),
            first[0].home.as_str(),
            first[0].first_at,
            first[0].source
        ),
        ("s-line", "", started, SightingSource::Statusline)
    );
    // The same execution, looked at again: still one sighting, keyed as before. Only its end
    // may move, and only if a minute passed, so the end is not compared.
    line();
    let again = sightings(root);
    let key = |s: &SightingRow| (s.session_id.clone(), s.home.clone(), s.first_at, s.source);
    assert_eq!(
        again.iter().map(key).collect::<Vec<_>>(),
        first.iter().map(key).collect::<Vec<_>>(),
        "a second look adds no sighting"
    );
    assert!(
        !Env::for_test(root).state_dir().exists(),
        "the line logs at debug at most, so it opens no log"
    );
}

#[test]
fn the_statusline_records_a_session_only_in_a_home_tagteam_knows() {
    // Token report spec §6.3: in a run shell the profile's account; with `CLAUDE_CONFIG_DIR`
    // naming the default home, that home; naming any other directory, nothing.
    let (d, a, _) = managed(0);
    let root = d.path();
    let default = Env::for_test(root).home.join(".claude");
    record_for_this_process(&default, "s-explicit");
    statusline(root)
        .env("CLAUDE_CONFIG_DIR", &default)
        .env("CLAUDE_CODE_SESSION_ID", "s-explicit")
        .assert()
        .success();
    let elsewhere = root.join("elsewhere");
    record_for_this_process(&elsewhere, "s-elsewhere");
    statusline(root)
        .env("CLAUDE_CONFIG_DIR", &elsewhere)
        .env("CLAUDE_CODE_SESSION_ID", "s-elsewhere")
        .assert()
        .success();
    let (profile, shell) = cc_profile(root, &a);
    record_for_this_process(&profile, "s-shell");
    statusline(root)
        .env("CLAUDE_CONFIG_DIR", &shell)
        .env("CLAUDE_CODE_SESSION_ID", "s-shell")
        .assert()
        .success()
        .stderr("");
    let mut seen: Vec<(String, String)> = sightings(root)
        .into_iter()
        .map(|s| (s.session_id, s.home))
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("s-explicit".to_owned(), String::new()),
            ("s-shell".to_owned(), a.clone()),
        ]
    );
}

#[test]
fn the_statusline_output_is_unchanged_by_a_sighting_failure() {
    // Token report spec §6.3: a sighting that cannot be written changes neither the line nor
    // the exit code, and the status bar has nowhere to show why.
    let (d, _, _) = managed(0);
    let root = d.path();
    record_for_this_process(&Env::for_test(root).home.join(".claude"), "s-broken");
    let db = Env::for_test(root).data_dir().join("tagteam.db");
    // A busy store: another process holds the write lock throughout. The line still prints,
    // and its write is skipped. How long it waited is the perf tests' to time, not this one's.
    let busy = rusqlite::Connection::open(&db).unwrap();
    busy.execute_batch("BEGIN IMMEDIATE").unwrap();
    statusline(root)
        .arg("--no-color")
        .env("CLAUDE_CODE_SESSION_ID", "s-broken")
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77%\n")
        .stderr("");
    busy.execute_batch("ROLLBACK").unwrap();
    drop(busy);
    assert!(sightings(root).is_empty(), "a busy store skips the write");
    // No table to write to at all.
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TABLE session_sightings")
        .unwrap();
    statusline(root)
        .arg("--no-color")
        .env("CLAUDE_CODE_SESSION_ID", "s-broken")
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77%\n")
        .stderr("");
}

#[test]
fn a_sighting_never_creates_the_store() {
    // Like the line itself (§5): with no store there is no account to credit, and nothing is
    // created.
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    seed_home(&env);
    login(
        &env,
        &FileKeychain::new(d.path().join("keychain")),
        "c@x.co",
        "",
        "rt-c",
    );
    record_for_this_process(&env.home.join(".claude"), "s-new");
    statusline(d.path())
        .env("CLAUDE_CODE_SESSION_ID", "s-new")
        .assert()
        .success()
        .stdout("c@x.co\n")
        .stderr("");
    assert!(
        !env.data_dir().exists(),
        "a sighting never creates the store"
    );
}
```

In `crates/tagteam/tests/perf.rs`, append (it already imports `Path`, `Duration`, `Env` and `now_epoch_s`; `rusqlite` is a dev-dependency):

```rust
/// The session id the timed `statusline` runs carry, as Claude Code passes it (token report
/// spec §6.3).
const SESSION: &str = "perf-session";

/// Claude Code's session record for this test process, every timed run's parent, in the
/// default home: the ancestor a sighting is keyed by (token report spec §6.3).
fn record_this_process(root: &Path) {
    let dir = Env::for_test(root).home.join(".claude/sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let pid = std::process::id();
    let record = format!(
        "{{\"pid\":{pid},\"sessionId\":\"{SESSION}\",\"cwd\":\"/w\",\"startedAt\":{},\"kind\":\"interactive\"}}",
        now_epoch_s() * 1000
    );
    std::fs::write(dir.join(format!("{pid}.json")), record).unwrap();
}

/// How many sightings the store holds, and the latest end of one.
fn sightings(root: &Path) -> (i64, Option<i64>) {
    rusqlite::Connection::open(Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .query_row(
            "SELECT count(*), max(last_at) FROM session_sightings",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn statusline_with_a_fresh_sighting_p95_is_within_10_ms() {
    // Token report spec §6.3, §15 "Budgets": the first warm-up run records the session, and
    // every timed run finds that sighting less than a minute old: one indexed lookup.
    let _one_at_a_time = serial();
    let d = tempfile::tempdir().unwrap();
    nothing_due(d.path());
    record_this_process(d.path());
    let server = MockServer::start();
    let envs = [("CLAUDE_CODE_SESSION_ID", SESSION)];
    let runs = timings(
        d.path(),
        &server.base_url(),
        &["statusline"],
        &envs,
        &|_| {},
    );
    let p = p95(&runs);
    eprintln!("statusline with a fresh sighting p95 {p:?} over {RUNS} runs");
    assert_eq!(sightings(d.path()).0, 1, "the session was recorded");
    assert_eq!(server.requests().len(), 0, "statusline sent a request");
    assert!(
        p <= Duration::from_millis(10),
        "statusline with a fresh sighting p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn statusline_due_a_sighting_extension_p95_is_within_10_ms() {
    // Before each run the sighting is moved a minute and more into the past, so every timed
    // run extends it: the lookup, the walk to the ancestor with a record, and one write.
    let _one_at_a_time = serial();
    let d = tempfile::tempdir().unwrap();
    nothing_due(d.path());
    record_this_process(d.path());
    let server = MockServer::start();
    let envs = [("CLAUDE_CODE_SESSION_ID", SESSION)];
    let db = Env::for_test(d.path()).data_dir().join("tagteam.db");
    let age = |_: usize| {
        rusqlite::Connection::open(&db)
            .unwrap()
            .execute(
                "UPDATE session_sightings SET last_at = last_at - 120000",
                [],
            )
            .unwrap();
    };
    let started = now_epoch_s() * 1000;
    let runs = timings(d.path(), &server.base_url(), &["statusline"], &envs, &age);
    let p = p95(&runs);
    eprintln!("statusline due a sighting extension p95 {p:?} over {RUNS} runs");
    let (count, last) = sightings(d.path());
    assert_eq!(count, 1, "one execution, one sighting");
    assert!(
        last.is_some_and(|at| at >= started),
        "the last run extended the sighting: {last:?}"
    );
    assert_eq!(server.requests().len(), 0, "statusline sent a request");
    assert!(
        p <= Duration::from_millis(10),
        "statusline due a sighting extension p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}
```

In `crates/tagteam/src/statusline.rs`'s tests module:

statusline.rs test hunk 1. Replace

```rust
#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
```

with

```rust
#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
```

statusline.rs test hunk 2. Replace

```rust
    #[test]
    fn the_no_keychain_refuses_and_counts_every_call() {
```

with

```rust
    #[test]
    fn the_sighting_home_is_the_profile_the_default_home_or_elsewhere() {
        // Token report spec §6.3.
        let dir = tempfile::tempdir().unwrap();
        let mut env = Env::for_test(dir.path());
        let default = env.home.join(".claude");
        std::fs::create_dir_all(&default).unwrap();
        let outside = |env: &Env| sighting_home(&RunShell::Outside, env);
        assert_eq!(outside(&env), SightingHome::Default, "unset");
        env.claude_config_dir = Some(OsString::new());
        assert_eq!(outside(&env), SightingHome::Default, "empty, as unset");
        env.claude_config_dir = Some(default.clone().into_os_string());
        assert_eq!(outside(&env), SightingHome::Default, "the default, by name");
        let link = dir.path().join("claude-link");
        std::os::unix::fs::symlink(&default, &link).unwrap();
        env.claude_config_dir = Some(link.into_os_string());
        assert_eq!(outside(&env), SightingHome::Default, "through a link");
        env.claude_config_dir = Some(dir.path().join("elsewhere").into_os_string());
        assert_eq!(outside(&env), SightingHome::Elsewhere);
        let profile = dir.path().join("profile");
        let inside = RunShell::Inside {
            profile: profile.clone(),
            marker: ProfileMarker {
                provider: ProviderId::new(CLAUDE_CODE),
                account_id: AccountId::from_string("0192"),
                config_dir: profile.display().to_string(),
                outer: json!({}),
            },
        };
        assert_eq!(sighting_home(&inside, &env), SightingHome::Profile(profile));
        let unreadable = RunShell::Unreadable {
            marker: dir.path().join(MARKER_FILE),
            detail: "torn".into(),
        };
        assert_eq!(sighting_home(&unreadable, &env), SightingHome::Elsewhere);
    }

    #[test]
    fn the_no_keychain_refuses_and_counts_every_call() {
```

In `crates/tagteam/src/app.rs`'s tests, the process boundary now captures the session id:

app.rs test hunk 1. Replace

```rust
        assert_eq!(
            session_vars(&build_registry(&ctx)),
            ["CLAUDE_CONFIG_DIR", "CLAUDECODE", "PATH"]
        );
```

with

```rust
        assert_eq!(
            session_vars(&build_registry(&ctx)),
            [
                "CLAUDE_CONFIG_DIR",
                "CLAUDECODE",
                "CLAUDE_CODE_SESSION_ID",
                "PATH"
            ]
        );
```

- [ ] **Step 2: Run them and see them fail**

```
cargo test -p tagteam --features test-support --test statusline
```

Expected: `the_statusline_records_its_session_once_a_minute` fails at `assert_eq!(first.len(), 1, ...)` with `left: 0`, and `the_statusline_records_a_session_only_in_a_home_tagteam_knows` at its `assert_eq!(seen, ...)` with `left: []`. `the_statusline_output_is_unchanged_by_a_sighting_failure` and `a_sighting_never_creates_the_store` pass already: they pin what the change must not break.

```
cargo test -p tagteam --features test-support --lib -- statusline::tests app::tests::the_process_boundary_captures_every_session_variable_and_claudecode
```

Expected: a compile error, `cannot find function sighting_home` and `cannot find type SightingHome`.

- [ ] **Step 3: Implement**

`crates/tagteam/src/app.rs`, in order:

app.rs hunk 1. Replace

```rust
use std::ffi::OsString;
```

with

```rust
use std::ffi::{OsStr, OsString};
```

app.rs hunk 2. Replace

```rust
/// Set to `1` in every process Claude Code starts (§13.5, Appendix A.7).
const CLAUDECODE: &str = "CLAUDECODE";
```

with

```rust
/// Set to `1` in every process Claude Code starts (§13.5, Appendix A.7).
const CLAUDECODE: &str = "CLAUDECODE";
/// The session's id, in every process Claude Code starts, its statusline command among them
/// (token report spec §6.3).
const CLAUDE_CODE_SESSION_ID: &str = "CLAUDE_CODE_SESSION_ID";
```

app.rs hunk 3. Replace

```rust
/// The variables `Context::from_process` captures into `Env.vars` (Decision 5): every variable
/// `registry`'s providers name a profile with (§4.5 `session_dir_var`), then `CLAUDECODE`
/// (§13.5), then `PATH`, which `plan_run` looks the launch command up on (§12.1).
pub(crate) fn session_vars(registry: &ProviderRegistry) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = registry
        .all()
        .iter()
        .filter_map(|p| p.session_dir_var())
        .collect();
    names.push(CLAUDECODE);
    names.push("PATH");
    names
}
```

with

```rust
/// The variables `Context::from_process` captures into `Env.vars` (Decision 5): every variable
/// `registry`'s providers name a profile with (§4.5 `session_dir_var`), then `CLAUDECODE`
/// (§13.5), then `CLAUDE_CODE_SESSION_ID`, the session the statusline records (token report
/// spec §6.3), then `PATH`, which `plan_run` looks the launch command up on (§12.1).
pub(crate) fn session_vars(registry: &ProviderRegistry) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = registry
        .all()
        .iter()
        .filter_map(|p| p.session_dir_var())
        .collect();
    names.push(CLAUDECODE);
    names.push(CLAUDE_CODE_SESSION_ID);
    names.push("PATH");
    names
}
```

app.rs hunk 4. Replace

```rust
/// §13.5's fast path, taken before `build_engine`: no lock check, no settings warnings (a status
/// bar has nowhere to show them), and an engine walled off from the Keychain and the network
/// (`statusline::engine`), which detects the run shell itself and resolves the provider in
/// Decision 13's order. An unreadable marker shows nothing and exits 0 (§12.8).
/// `main_with_args` has already drained stdin.
```

with

```rust
/// §13.5's fast path, taken before `build_engine`: no lock check, no settings warnings (a status
/// bar has nowhere to show them), and an engine walled off from the Keychain and the network
/// (`statusline::engine`), which detects the run shell itself and resolves the provider in
/// Decision 13's order. An unreadable marker shows nothing and exits 0 (§12.8). Once the line
/// is out, it records its session as evidence for the token report (token report spec §6.3).
/// `main_with_args` has already drained stdin.
```

app.rs hunk 5. Replace

```rust
    let (no_color_env, force_color_env) = (ctx.no_color_env, ctx.force_color_env);
    let (engine, provider, _http, _keychain) = statusline::engine(ctx, provider.as_deref());
```

with

```rust
    let (no_color_env, force_color_env) = (ctx.no_color_env, ctx.force_color_env);
    let session_id = ctx
        .env
        .var(CLAUDE_CODE_SESSION_ID)
        .and_then(OsStr::to_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned);
    let (engine, provider, _http, _keychain) = statusline::engine(ctx, provider.as_deref());
```

app.rs hunk 6. Replace

```rust
    match result {
        Ok(text) => {
            let _ = write!(io.out, "{text}");
            0
        }
```

with

```rust
    match result {
        Ok(text) => {
            let _ = write!(io.out, "{text}");
            if !print_config {
                let _ = io.out.flush();
                statusline::record_session(&engine, &provider, session_id.as_deref());
            }
            0
        }
```

`crates/tagteam/src/statusline.rs`, in order:

statusline.rs hunk 1. Replace

```rust
//! `tagteam statusline` (§13.5): one line for Claude Code's status bar, from the stored
//! reading. It never fetches, never touches the Keychain, and never builds the HTTP adapter.

use std::sync::Arc;
```

with

```rust
//! `tagteam statusline` (§13.5): one line for Claude Code's status bar, from the stored
//! reading. It never fetches, never touches the Keychain, and never builds the HTTP adapter.
//! Once the line is out, it records the session it serves (token report spec §6.3).

use std::path::{Path, PathBuf};
use std::sync::Arc;
```

statusline.rs hunk 2. Replace

```rust
        // `EngineConfig` requires a probe. The line never judges a session record (Decision 17),
        // so this one is never asked.
        process: Arc::new(SystemProcessProbe),
```

with

```rust
        // The line never judges a session record (Decision 17). The session sighting after it
        // asks this probe only for its ancestors' pids (token report spec §6.3).
        process: Arc::new(SystemProcessProbe),
```

statusline.rs hunk 3. Replace

```rust
/// The statusline engine's Keychain: every call is refused, and counted.
```

with

```rust
/// Where the line's session runs, for its sighting (token report spec §6.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SightingHome {
    /// The default home.
    Default,
    /// The run shell's profile.
    Profile(PathBuf),
    /// Neither: a `CLAUDE_CONFIG_DIR` naming another directory, or a marker that cannot be
    /// read. Nothing is recorded.
    Elsewhere,
}

/// Token report spec §6.3: in a run shell, the marker's profile; otherwise the default home,
/// when `CLAUDE_CONFIG_DIR` is unset, empty or names it; otherwise `Elsewhere`. `env` is the
/// engine's, which outside a run shell is the process's own.
pub(crate) fn sighting_home(shell: &RunShell, env: &Env) -> SightingHome {
    match shell {
        RunShell::Inside { profile, .. } => SightingHome::Profile(profile.clone()),
        RunShell::Unreadable { .. } => SightingHome::Elsewhere,
        RunShell::Outside => {
            let Some(dir) = env.claude_config_dir.as_deref().filter(|d| !d.is_empty()) else {
                return SightingHome::Default;
            };
            let mut plain = env.clone();
            plain.claude_config_dir = None;
            if same_dir(Path::new(dir), &CcPaths::resolve(&plain).config_home) {
                SightingHome::Default
            } else {
                SightingHome::Elsewhere
            }
        }
    }
}

/// Whether two spellings name one directory: equal as written, or once resolved.
fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(x), Ok(y)) if x == y
        )
}

/// Token report spec §6.3: records the session this line serves, once the line is out. Only
/// for a provider whose transcripts the report reads, with the id Claude Code gave the
/// command, in a home `sighting_home` names. A failure is logged at debug: the status bar has
/// nowhere to show it, and neither the line nor the exit code changes.
pub(crate) fn record_session(engine: &Engine, provider: &ProviderId, session_id: Option<&str>) {
    let Some(session_id) = session_id else {
        return;
    };
    if !engine
        .provider(provider)
        .is_ok_and(|p| p.capabilities().transcripts)
    {
        return;
    }
    let profile = match sighting_home(engine.run_shell(), engine.env()) {
        SightingHome::Default => None,
        SightingHome::Profile(profile) => Some(profile),
        SightingHome::Elsewhere => return,
    };
    if let Err(e) = engine.statusline_sighting(provider, session_id, profile.as_deref()) {
        tracing::debug!(kind = e.kind(), "the statusline's session was not recorded");
    }
}

/// The statusline engine's Keychain: every call is refused, and counted.
```

- [ ] **Step 4: Run them and see them pass**

```
cargo test -p tagteam --locked --features test-support --test statusline --test logging --test run_shell_cli
cargo test -p tagteam --locked --features test-support --lib
```

Expected: all pass. `logging`'s `statusline_with_the_default_filter_opens_no_log` and `run_shell_cli`'s statusline tests guard the line's other promises.

Then the timing tests, release only, on an idle machine; report each p95 printed:

```
cargo test --release -p tagteam --locked --features test-support --test perf -- --ignored --nocapture
```

Expected: 6 passed, both new ones within 10 ms.

- [ ] **Step 5: Lint**

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings
```

- [ ] **Step 6: Commit**

```
git add crates/tagteam/src/app.rs crates/tagteam/src/statusline.rs crates/tagteam/tests/statusline.rs crates/tagteam/tests/perf.rs
git commit -m "Record the statusline's session as evidence"
```

#### Notes for the controller

1. **What these tests require of Task 16's `statusline_sighting`:** it opens the store with `existing_store` and records nothing without one (`a_sighting_never_creates_the_store`, as `statusline` never creates the store); it logs at debug at most, so the default filter opens no log (`the_statusline_records_its_session_once_a_minute`'s last assertion, core §14.2); its write waits at most `CACHE_WRITE_WAIT` and a busy store is skipped; its walk starts at this process and stops at the first ancestor with a parseable record in the home's `sessions/`.
2. **No ordinary test here reads the wall clock.** The busy-store case opens the store while another connection holds `BEGIN IMMEDIATE`, which works because `Store::open_existing` takes the write lock only when a migration is due; it asserts exit 0, the unchanged line and no sighting written, and leaves the wait's length to the `#[ignore]` perf tests. The binary has no test clock (`TAGTEAM_TEST_*` sets only the Keychain, the platform and the API base), so `the_statusline_records_its_session_once_a_minute` asserts only that the first line inserts one sighting keyed by the ancestor record's `startedAt` and a second adds none; the minute itself (no extension within it, one past it) is Task 16's engine test on `FakeClock`.
3. **Batch H** runs this task beside 22 and 23: Task 22 edits only test files of this crate (`run_shell_cli.rs`, `home_cli.rs`, `logging.rs`), but every test target compiles this task's `app.rs` and `statusline.rs`, so run 20 first or re-run 22's focused tests after 20 lands.
4. **What was verified.** The hunks were applied to the real `app.rs` (after Task 19's) and `statusline.rs` and pass `rustfmt --check`; the integration and perf tests were rustfmt-formatted in place and type-checked with clippy against stubs of `common`, the store API and `perf.rs`'s helpers. Nothing ran against the real binary, since Tasks 7–19 do not exist yet.

---

### Task 21: `run` records its launch and polls its session

**Depends on:** 16
**Files:**
- Modify: `crates/tagteam/src/run.rs` (the whole file below: `run_session` records the launch before the spawn, waits on this thread while a scoped thread attaches the child's pid and then polls its session record, stamps the child's exit, and closes the launch after exit handling; new `open_launch`, `close_launch`, `wait_polling`, `StopOnDrop`; a unit test)
- Create: `crates/tagteam/tests/run_tokens.rs`
- Test: `crates/tagteam/tests/run_tokens.rs`, `crates/tagteam/src/run.rs` (`mod tests`), and the existing `run_cli.rs` and `signals.rs` as regression checks

**Interfaces:**
- Consumes:
  ```rust
  // Task 16, tagteam_engine::tokens
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct LaunchId { pub provider: ProviderId, pub account: AccountId, pub started_at_ms: i64 }
  /// The poller's own state, owned by `run`: the record's last mtime and the sighting it wrote last.
  #[derive(Debug, Clone, Default, PartialEq, Eq)]
  pub struct LaunchPoll { pub last_mtime_ns: Option<i128>, pub last_sighting: Option<(String, i64)> }
  impl Engine {
      pub fn open_launch(&self, provider: &ProviderId, account: &AccountId) -> Result<LaunchId, EngineError>;          // before the spawn
      pub fn attach_launch_pid(&self, id: &LaunchId, pid: u32) -> Result<(), EngineError>;  // the poller's first act
      pub fn poll_launch(&self, id: &LaunchId, profile: &Path, pid: u32, state: &mut LaunchPoll) -> Result<(), EngineError>; // every 2 s while the child runs
      pub fn close_launch(&self, id: &LaunchId, profile: &Path, state: &LaunchPoll, exited_at_ms: i64) -> Result<(), EngineError>; // after finish_run: the sighting ends at exited_at_ms, the launch closes now if idle
  }
  // Task 7: Capabilities.transcripts. Existing: Launched { account: AccountRow, profile: PathBuf, .. }, Engine::finish_run
  // Task 12, tagteam_engine::store (tests)
  pub fn launches(&self, provider: &ProviderId) -> Result<Vec<LaunchRow>, StoreError>;
  pub fn sightings(&self, provider: &ProviderId) -> Result<Vec<SightingRow>, StoreError>;
  pub struct LaunchRow { pub account_id: String, pub started_at: i64, pub ended_at: Option<i64>, pub pid: Option<u32>, pub recorded_at: i64 }
  ```
- Produces: `run`'s evidence for spec §6.1 (a launch row per session launch, the child's sighting while it runs and at its exit). No new public API.

Spec §6.1 in `run`'s terms:

- **The launch** is recorded right after the token's last look and before the spawn, so it covers every response the child makes. Only a provider with `transcripts` records one.
- **The poll** runs on a scoped thread beside the existing wait, which stays on the calling thread with its signal forwarding unchanged: after the spawn, the calling thread goes straight to the first forward and the wait loop. The poller's first act is to attach the child's pid to the launch; it then reads the record every 2 s (Task 16's `poll_launch` skips an unchanged mtime) and stops when an `AtomicBool` is set. Every store write while the child runs is the poller's, so a busy store never delays the wait or a forwarded signal. The flag is set by a guard dropped when the wait returns or unwinds, so the scope can never wait on the poller forever.
- **The exit** is stamped with `engine.now_ms()` as soon as the wait returns, before `finish_run`: that is `exited_at_ms`, where the sighting ends. It is read once the poller is joined, so no poll wrote a later end (`set_sighting_last_at` only moves an end forward); the join waits at most for the one poll, or the attach, in flight.
- **The close** comes after `finish_run`, not before it: until exit handling unlinks this process's launch reservation, the profile is in use by this very run (core §12.5, §12.6), and `close_launch` would always leave the launch open. The profile and the launch's id are copied before `finish_run` consumes `launched`. The sighting still ends at the exit stamp, so exit handling's time is never the session's; only the launch ends after it. A spawn that fails closes its launch after `abandon`'s exit handling, with no sighting, as no child ever ran.
- **Failures are evidence lost, never a run failed.** A launch that cannot be recorded is one warning on stderr before the spawn; a failed pid attach or poll goes to the log at warn (a failed poll once per run), because stderr belongs to `claude` while it runs; a failed close is a warning after the child has exited. None of them changes the exit code.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam/tests/run_tokens.rs`. It drives the binary with the fake `claude` (`FAKE_CLAUDE_HOLD` keeps a session running; `FAKE_CLAUDE_RECORD` makes it write its session record, `sessionId` `fake-<pid>`, into the profile) and reads the store's evidence tables between steps. `run_sights_the_child_session` also parks the run at the existing `after-exit` pause point, between the child's exit and exit handling, and reads the clock there: the sighting must end no later than that, and the launch no earlier.

```rust
//! `tagteam run`'s evidence for the token report (token report spec §6.1), through the binary
//! and the fake `claude`: the launch it records before the spawn and closes at the exit, and
//! the session it sights while the child runs. Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use common::{
    FakeCall, fake_claude, fake_claude_calls, hold_launch, path_with, std_cmd, two_fresh_accounts,
};
use serde_json::Value;
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::store::{LaunchRow, SightingRow, SightingSource, Store};
use tagteam_provider::{Env, profile_path};

/// How long one step may take before it counts as stuck.
const LONG: Duration = Duration::from_secs(20);

/// `a@x.co` at position 1 and `b@x.co` at position 2, the live login, with the fake `claude`
/// in `<root>/bin` and a working directory `<root>/work`: `run 1` launches a session.
struct Home {
    dir: tempfile::TempDir,
    /// Account `a`'s id.
    a: String,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (a, _b) = two_fresh_accounts(dir.path());
        fake_claude(dir.path());
        fs::create_dir_all(dir.path().join("work")).unwrap();
        Home { dir, a }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// A fresh `FAKE_CLAUDE_OUT` file for one run of the fake `claude`.
    fn out(&self, name: &str) -> PathBuf {
        let dir = self.root().join("calls");
        fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// `tagteam run 1 -- x` in `<root>/work`, the fake `claude` first on `PATH`, recording its
    /// runs in `out`, with stdout and stderr piped.
    fn run_a(&self, out: &Path) -> Command {
        let mut c = std_cmd(self.root());
        c.env("PATH", path_with(&self.root().join("bin")))
            .env("FAKE_CLAUDE_OUT", out)
            .current_dir(self.root().join("work"))
            .args(["run", "1", "--", "x"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }

    /// Account `a`'s profile (core §12.2).
    fn profile(&self) -> PathBuf {
        profile_path(
            &Env::for_test(self.root()),
            &AccountId::from_string(&self.a),
        )
    }

    fn store(&self) -> Store {
        Store::open_existing(&Env::for_test(self.root()).data_dir().join("tagteam.db"))
            .unwrap()
            .unwrap()
    }

    fn launches(&self) -> Vec<LaunchRow> {
        self.store()
            .launches(&ProviderId::new(CLAUDE_CODE))
            .unwrap()
    }

    fn sightings(&self) -> Vec<SightingRow> {
        self.store()
            .sightings(&ProviderId::new(CLAUDE_CODE))
            .unwrap()
    }
}

/// Creates the hold file when dropped, so a failing test never leaves a fake `claude` running
/// out its 30 s.
struct Release(PathBuf);

impl Drop for Release {
    fn drop(&mut self) {
        let _ = fs::write(&self.0, b"");
    }
}

/// Polls `ready` every 20 ms until it gives a value; fails after `LONG`.
fn wait_for<T>(what: &str, ready: impl Fn() -> Option<T>) -> T {
    let deadline = Instant::now() + LONG;
    loop {
        if let Some(v) = ready() {
            return v;
        }
        assert!(Instant::now() < deadline, "never got to {what}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// The session the fake `claude` recorded in `out`, once it runs.
fn running(out: &Path) -> FakeCall {
    wait_for("the session", || {
        fake_claude_calls(out)
            .into_iter()
            .find(|c| c.mode == "session" && c.ready)
    })
}

/// `child`'s output once it has exited; kills it and fails if it still runs after `LONG`.
fn finish(mut child: Child) -> Output {
    let deadline = Instant::now() + LONG;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("tagteam was still running {LONG:?} later");
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The system clock in epoch ms, the clock the binary stamps with.
fn now_ms() -> i64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    i64::try_from(since.as_millis()).unwrap()
}

#[test]
fn run_records_a_launch_and_closes_it() {
    // Spec §6.1: the launch is open from before the spawn, holds the child's pid once it
    // exists, and is closed at the exit, the profile being in use no more.
    let home = Home::new();
    assert!(home.launches().is_empty(), "no profile existed to seed one");
    let out = home.out("launch");
    let hold = home.root().join("hold");
    let _release = Release(hold.clone());
    let child = home
        .run_a(&out)
        .env("FAKE_CLAUDE_HOLD", &hold)
        .spawn()
        .unwrap();
    let session = running(&out);
    let open = wait_for("the launch's pid", || {
        home.launches().into_iter().find(|l| l.pid.is_some())
    });
    assert_eq!(
        (open.account_id.as_str(), open.pid, open.ended_at),
        (home.a.as_str(), Some(session.pid), None),
        "{open:?}"
    );
    assert_eq!(home.launches().len(), 1);
    fs::write(&hold, b"").unwrap();
    let output = finish(child);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let closed = home.launches();
    assert_eq!(closed.len(), 1, "{closed:?}");
    assert_eq!(closed[0].started_at, open.started_at, "the same launch");
    let ended = closed[0].ended_at.expect("closed at the exit");
    assert!(ended >= open.started_at, "{closed:?}");
}

#[test]
fn run_sights_the_child_session() {
    // Spec §6.1: while the child runs, `run` reads its record every 2 s and sights the session
    // it names in the profile's home, keyed by the record's `startedAt`. The child's exit,
    // stamped before exit handling, ends that sighting; the launch closes after exit handling.
    // The run is parked between the two (`after-exit`), so the test's clock read there falls
    // after the stamp and before the close, whatever either took.
    let home = Home::new();
    let out = home.out("sighting");
    let hold = home.root().join("hold");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _release = Release(hold.clone());
    let _resume = Release(pause.join("resume"));
    let child = home
        .run_a(&out)
        .env("FAKE_CLAUDE_HOLD", &hold)
        .env("FAKE_CLAUDE_RECORD", "interactive")
        .env("TAGTEAM_TEST_PAUSE_AT", "after-exit")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause)
        .spawn()
        .unwrap();
    let session = running(&out);
    let session_id = format!("fake-{}", session.pid);
    let seen = wait_for("the poll's sighting", || {
        home.sightings()
            .into_iter()
            .find(|s| s.session_id == session_id)
    });
    let record: Value = serde_json::from_slice(
        &fs::read(
            home.profile()
                .join("sessions")
                .join(format!("{}.json", session.pid)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (seen.home.as_str(), seen.source, seen.first_at),
        (
            home.a.as_str(),
            SightingSource::Run,
            record["startedAt"].as_i64().unwrap()
        ),
        "{seen:?}"
    );
    fs::write(&hold, b"").unwrap();
    wait_for("the run's exit handling", || {
        pause.join("paused").exists().then_some(())
    });
    let parked_at = now_ms();
    fs::write(pause.join("resume"), b"").unwrap();
    let output = finish(child);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let after: Vec<SightingRow> = home
        .sightings()
        .into_iter()
        .filter(|s| s.session_id == session_id)
        .collect();
    assert_eq!(after.len(), 1, "one execution, one sighting: {after:?}");
    assert_eq!(after[0].first_at, seen.first_at);
    let ended = home.launches()[0]
        .ended_at
        .expect("closed after exit handling");
    assert!(
        seen.last_at <= after[0].last_at && after[0].last_at <= parked_at,
        "the exit stamp ends the sighting, before exit handling at {parked_at}: {after:?} after {seen:?}"
    );
    assert!(
        parked_at <= ended,
        "the launch closes after exit handling: ended {ended}, parked at {parked_at}"
    );
}

#[test]
fn a_launch_stays_open_while_the_profile_is_in_use() {
    // Spec §6.1: a profile still in use at the exit (here another reservation, as a second
    // `run` or a background session holds one) keeps the launch open; the next
    // `sight_sessions` that finds it idle closes it.
    let home = Home::new();
    let out = home.out("in-use");
    let hold = home.root().join("hold");
    let _release = Release(hold.clone());
    let child = home
        .run_a(&out)
        .env("FAKE_CLAUDE_HOLD", &hold)
        .spawn()
        .unwrap();
    running(&out);
    let _other = hold_launch(&home.profile());
    fs::write(&hold, b"").unwrap();
    let output = finish(child);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let launches = home.launches();
    assert_eq!(launches.len(), 1, "{launches:?}");
    assert_eq!(launches[0].ended_at, None, "{launches:?}");
}

#[test]
fn evidence_failures_never_change_the_exit_code() {
    // Spec §6.1: evidence is never worth a session. With the sightings table gone every poll
    // fails, and with the launches table gone too the launch cannot be recorded; either way
    // the run exits with the session's code. A failed poll goes to the log; a launch that
    // could not be recorded is a warning, before the session starts.
    let home = Home::new();
    let db = Env::for_test(home.root()).data_dir().join("tagteam.db");
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TABLE session_sightings")
        .unwrap();
    let out = home.out("no-sightings");
    let output = home
        .run_a(&out)
        .env("FAKE_CLAUDE_EXIT", "7")
        .env("FAKE_CLAUDE_RECORD", "interactive")
        .env("FAKE_CLAUDE_SLEEP", "2.5")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7), "{}", stderr(&output));
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TABLE run_launches")
        .unwrap();
    let out = home.out("no-launches");
    let output = home
        .run_a(&out)
        .env("FAKE_CLAUDE_EXIT", "7")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7), "{}", stderr(&output));
    assert!(
        stderr(&output).contains(
            "warning: the token report could not record this session's launch, so its usage may show as unattributed"
        ),
        "{}",
        stderr(&output)
    );
    let calls = fake_claude_calls(&out);
    assert!(
        calls
            .iter()
            .any(|c| c.mode == "session" && c.exit == Some(7)),
        "the session ran: {calls:?}"
    );
}
```

`run.rs`'s new unit test, `the_stop_flag_is_set_however_the_wait_ends`, is in the file of Step 3.

- [ ] **Step 2: Run them and see them fail**

```
cargo test -p tagteam --features test-support --test run_tokens
```

Expected: `run_records_a_launch_and_closes_it` panics with `never got to the launch's pid` after 20 s; `run_sights_the_child_session` with `never got to the poll's sighting`; `a_launch_stays_open_while_the_profile_is_in_use` at `assert_eq!(launches.len(), 1, ...)` (`left: 0`); `evidence_failures_never_change_the_exit_code` at the warning's assertion (both runs already exit 7).

- [ ] **Step 3: Implement**

Replace `crates/tagteam/src/run.rs` with:

```rust
//! §12.5: `tagteam run` once its launch is made. The spawn, the wait, the signals forwarded to
//! `claude`, the exit handling its exit starts, and the exit code (§14.1, B.63). Around them,
//! the token report's evidence (token report spec §6.1): the launch, recorded before the
//! spawn, and the child's session, read while it runs.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use tagteam_engine::launch::{LaunchEnd, Launched};
use tagteam_engine::tokens::{LaunchId, LaunchPoll};
use tagteam_engine::{Engine, EngineError};
use tagteam_provider::Cancel;
use tagteam_provider::process::{SpawnSpec, exit_code, spawn_session};

use crate::signals;

/// How often the wait loop looks at `claude` and at the cancel token (Decision 1).
const POLL: Duration = Duration::from_millis(50);

/// Token report spec §6.1: how often `run` reads its child's session record.
const RECORD_POLL: Duration = Duration::from_secs(2);

/// `run`'s exit code when it lost track of `claude`'s status, which only a failing `waitpid`
/// can cause.
const LOST: i32 = 1;

/// §12.5 steps 6–7 and "When the child exits", after `launch` and the login check. Spawns
/// `launch` with `args` in `cwd` and `launched`'s environment, with the reservation inherited
/// through its fd. Waits for it, forwarding signals (Decision 1), then runs the exit handling
/// and prints its notices on `err`.
///
/// Token report spec §6.1: the launch is recorded before the spawn, and the child's pid and
/// its session record are recorded off this thread while it runs. None of that ever changes
/// the outcome.
///
/// `Ok` is `claude`'s exit code, `128 + n` when signal `n` ended it, whatever exit handling did
/// (B.63). `Err` is a launch that never started: interrupted at the token's last look, or a
/// spawn that failed. Its exit handling has already run (§12.3).
pub(crate) fn run_session(
    engine: &Engine,
    launched: Launched,
    launch: &Path,
    args: &[OsString],
    cwd: &Path,
    cancel: &Cancel,
    err: &mut dyn Write,
) -> Result<i32, EngineError> {
    // Decision 1: Ctrl-\ is the child's. Registered before the spawn, so the exec gives
    // `claude` the default action back.
    if let Err(e) = signals::survive_quit() {
        let _ = writeln!(
            err,
            "warning: Ctrl-\\ may stop tagteam before the session ends: {e}"
        );
    }
    // §12.5: the token's last look. A signal recorded after it is `claude`'s, and goes to it as
    // soon as it exists.
    if let Some(signal) = cancel.take() {
        notify(err, &engine.finish_run(launched, LaunchEnd::Refused));
        return Err(EngineError::Interrupted(signal));
    }
    pause_point("before-spawn");
    let launch_id = open_launch(engine, &launched, err);
    let profile = launched.profile.clone();
    let spec = SpawnSpec {
        program: launch.to_path_buf(),
        args: args.to_vec(),
        set: launched.env.set.clone(),
        remove: launched.env.remove.clone(),
        cwd: Some(cwd.to_path_buf()),
    };
    let mut child = match spawn_session(&spec, Some(launched.reservation.fd())) {
        Ok(child) => child,
        Err(e) => {
            let e = EngineError::LaunchUnreachable {
                detail: format!("{}: {e}", launch.display()),
            };
            let e = abandon(engine, launched, cancel, err, e);
            let no_sighting = LaunchPoll::default();
            close_launch(
                engine,
                launch_id.as_ref(),
                &profile,
                &no_sighting,
                engine.now_ms(),
                err,
            );
            return Err(e);
        }
    };
    let pid = child.id();
    // The first look forwards anything, SIGINT included: what arrived while `claude` was being
    // spawned, the terminal could not deliver to it.
    forward(pid, cancel.take(), true);
    let mut seen = LaunchPoll::default();
    let status = match &launch_id {
        Some(id) => wait_polling(engine, id, &profile, pid, &mut seen, || {
            wait(&mut child, cancel)
        }),
        None => wait(&mut child, cancel),
    };
    // Token report spec §6.1: the child's exit, stamped before exit handling, ends its sighting.
    // The poller is joined by now, so no poll wrote a later end than this.
    let exited_at_ms = engine.now_ms();
    // What forwarding left is spent, so exit handling meets only a signal sent from now on.
    let _ = cancel.take();
    pause_point("after-exit");
    let code = match status {
        Ok(status) => exit_code(status),
        Err(e) => {
            let _ = writeln!(err, "tagteam: lost track of the session's exit status: {e}");
            LOST
        }
    };
    notify(err, &engine.finish_run(launched, LaunchEnd::Exited(code)));
    // After exit handling, which unlinks this process's reservation: until then the profile
    // is in use by this very run, and the launch could never close here.
    close_launch(
        engine,
        launch_id.as_ref(),
        &profile,
        &seen,
        exited_at_ms,
        err,
    );
    Ok(code)
}

/// Token report spec §6.1: records the launch before the spawn, for a provider whose transcripts
/// the report reads. A failure is a warning, and the session runs without evidence: its usage
/// may then show as unattributed, never under another account.
fn open_launch(engine: &Engine, launched: &Launched, err: &mut dyn Write) -> Option<LaunchId> {
    let provider = &launched.account.provider;
    if !engine
        .provider(provider)
        .is_ok_and(|p| p.capabilities().transcripts)
    {
        return None;
    }
    match engine.open_launch(provider, &launched.account.id) {
        Ok(id) => Some(id),
        Err(e) => {
            let _ = writeln!(
                err,
                "warning: the token report could not record this session's launch, so its usage may show as unattributed: {e}"
            );
            None
        }
    }
}

/// Token report spec §6.1, after exit handling: ends the sighting the poll saw last at
/// `exited_at_ms`, the child's exit, and closes the launch unless the profile is still in use,
/// as by a background session the child left running. A failure is a warning; the next
/// `sight_sessions` closes the launch then.
fn close_launch(
    engine: &Engine,
    id: Option<&LaunchId>,
    profile: &Path,
    seen: &LaunchPoll,
    exited_at_ms: i64,
    err: &mut dyn Write,
) {
    let Some(id) = id else {
        return;
    };
    if let Err(e) = engine.close_launch(id, profile, seen, exited_at_ms) {
        let _ = writeln!(
            err,
            "warning: the token report could not record this session's end: {e}"
        );
    }
}

/// Token report spec §6.1: runs `wait` on this thread while a scoped thread records the child's
/// pid, then reads its session record every `RECORD_POLL`, until `wait` returns. The store
/// writes are the poller's, so a busy store never delays the wait or the signals it forwards.
/// The poller never touches the cancel token or the terminal: a failure is logged, once, and
/// the next poll tries again.
fn wait_polling<T>(
    engine: &Engine,
    id: &LaunchId,
    profile: &Path,
    pid: u32,
    seen: &mut LaunchPoll,
    wait: impl FnOnce() -> T,
) -> T {
    let stop = AtomicBool::new(false);
    thread::scope(|s| {
        s.spawn(|| {
            if let Err(e) = engine.attach_launch_pid(id, pid) {
                tracing::warn!(
                    kind = e.kind(),
                    "could not record the session's pid for the token report"
                );
            }
            let mut next = Instant::now();
            let mut logged = false;
            while !stop.load(Ordering::Relaxed) {
                if Instant::now() >= next {
                    if let Err(e) = engine.poll_launch(id, profile, pid, seen) {
                        if !logged {
                            tracing::warn!(
                                kind = e.kind(),
                                "could not read the session's record for the token report"
                            );
                            logged = true;
                        }
                    }
                    next = Instant::now() + RECORD_POLL;
                }
                thread::sleep(POLL);
            }
        });
        let _stop = StopOnDrop(&stop);
        wait()
    })
}

/// Sets its flag when dropped, so the poller ends however the wait does, a panic included,
/// and the scope never waits on it forever.
struct StopOnDrop<'a>(&'a AtomicBool);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// §12.3, §12.5: a launch refused or interrupted once its reservation exists runs its exit
/// handling as if `claude` had exited at once, then fails with `e`: exit 1, or `128 + n` for an
/// interruption. Whatever signal is pending is spent on that, so the exit handling stops only at
/// a new one (Decision 12): the one that interrupted the launch, or one that arrived after a
/// refusal was decided, which leaves `e` as it is and is never reported as too late.
pub(crate) fn abandon(
    engine: &Engine,
    launched: Launched,
    cancel: &Cancel,
    err: &mut dyn Write,
    e: EngineError,
) -> EngineError {
    let _ = cancel.take();
    notify(err, &engine.finish_run(launched, LaunchEnd::Refused));
    e
}

/// §12.1: `exec` returns only when it failed. A launch command gone since `plan_run` found it
/// is `launch-command-missing`, as if it had never been there; any other failure is the I/O
/// error, naming the command.
pub(crate) fn exec_failed(spec: &SpawnSpec, e: io::Error) -> EngineError {
    if e.kind() == io::ErrorKind::NotFound {
        let command = spec.program.file_name().map_or_else(
            || spec.program.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        return EngineError::LaunchCommandMissing { command };
    }
    EngineError::Io(io::Error::new(
        e.kind(),
        format!("could not run {}: {e}", spec.program.display()),
    ))
}

/// Decision 1, §12.5: whether a signal the token recorded goes on to `claude`. The first look
/// after the spawn forwards SIGINT, SIGTERM and SIGHUP alike. After it, SIGINT is the
/// terminal's to deliver, since `claude` is in its foreground group, so only SIGTERM and
/// SIGHUP, which a `kill` sends to tagteam alone, are forwarded.
pub(crate) fn forwarded(signal: i32, first: bool) -> bool {
    match signal {
        libc::SIGTERM | libc::SIGHUP => true,
        libc::SIGINT => first,
        _ => false,
    }
}

/// Sends `signal`, when `forwarded` says so, to `claude` (`pid`).
fn forward(pid: u32, signal: Option<i32>, first: bool) {
    let Some(signal) = signal.filter(|&s| forwarded(s, first)) else {
        return;
    };
    // SAFETY: kill(2) reads no memory of ours. `pid` is this process's child, which only
    // `wait` reaps, and nothing is sent once it has: the pid names `claude` or its zombie,
    // never a process that reused the number.
    let rc = unsafe { libc::kill(pid as libc::pid_t, signal) };
    if rc != 0 {
        tracing::warn!(
            signal,
            "could not forward the signal to the session: {}",
            io::Error::last_os_error()
        );
    }
}

/// Waits for `claude`, forwarding what the token records meanwhile. Nothing here waits on a
/// lock (B.37).
fn wait(child: &mut Child, cancel: &Cancel) -> io::Result<ExitStatus> {
    let pid = child.id();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            // No status to poll for: wait it out; it is still the child's exit that ends the run.
            Err(_) => return child.wait(),
        }
        thread::sleep(POLL);
        forward(pid, cancel.take(), false);
    }
}

/// Exit handling's notices (Decision 12), on stderr: stdout is `claude`'s.
fn notify(err: &mut dyn Write, notices: &[String]) {
    for notice in notices {
        let _ = writeln!(err, "note: {notice}");
    }
}

/// A test-only stop at `name`: `before-spawn` is right after the token's last look, and
/// `after-exit` is between `claude`'s exit and its exit handling, while the reservation is still
/// this process's. It parks through the engine's own protocol (`tagteam_engine::pause_at`), and
/// never looks at the token, so a signal sent meanwhile meets the run exactly where it would
/// have.
#[cfg(feature = "test-support")]
fn pause_point(name: &str) {
    tagteam_engine::pause_at(name);
}

#[cfg(not(feature = "test-support"))]
fn pause_point(_name: &str) {}

#[cfg(test)]
mod tests {
    // `io` and `SpawnSpec` come from the module's own imports.
    use super::*;

    #[test]
    fn the_first_look_after_the_spawn_forwards_any_signal_and_later_ones_only_term_and_hup() {
        // §12.5, Decision 1: (signal, first look) → forwarded to claude.
        let cases = [
            (libc::SIGINT, true, true),
            (libc::SIGTERM, true, true),
            (libc::SIGHUP, true, true),
            (libc::SIGINT, false, false),
            (libc::SIGTERM, false, true),
            (libc::SIGHUP, false, true),
            (libc::SIGQUIT, true, false),
            (libc::SIGQUIT, false, false),
        ];
        for (signal, first, want) in cases {
            assert_eq!(
                forwarded(signal, first),
                want,
                "{signal}, first look {first}"
            );
        }
    }

    #[test]
    fn an_exec_that_finds_no_command_is_launch_command_missing_and_any_other_failure_is_io() {
        let spec = SpawnSpec {
            program: "/opt/bin/claude".into(),
            ..SpawnSpec::default()
        };
        let e = exec_failed(&spec, io::Error::from(io::ErrorKind::NotFound));
        assert_eq!(e.kind(), "launch-command-missing");
        let e = exec_failed(&spec, io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(e.kind(), "io");
        assert!(e.to_string().contains("/opt/bin/claude"), "{e}");
    }

    #[test]
    fn the_stop_flag_is_set_however_the_wait_ends() {
        let stop = AtomicBool::new(false);
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _stop = StopOnDrop(&stop);
            panic!("the wait failed");
        }));
        assert!(unwound.is_err());
        assert!(
            stop.load(Ordering::Relaxed),
            "a panicking wait still stops the poller"
        );
    }
}
```

- [ ] **Step 4: Run them and see them pass**

```
cargo test -p tagteam --locked --features test-support --test run_tokens --test run_cli --test signals
cargo test -p tagteam --locked --features test-support --lib run::tests
```

Expected: `run_tokens` 4 passed (about 10 s: two runs poll for 2 s or more); `run_cli` and `signals` unchanged; `run::tests` 3 passed.

- [ ] **Step 5: Lint**

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings
```

- [ ] **Step 6: Commit**

```
git add crates/tagteam/src/run.rs crates/tagteam/tests/run_tokens.rs
git commit -m "Record run launches and poll their sessions"
```

#### Notes for the controller

1. **Deviation from the outline:** the outline puts `close_launch` "after exit and before `finish_run` consumes `launched`". Before `finish_run`, this run's own reservation still holds the profile, so `close_launch`'s in-use check (Task 16: `session_state_at(..).owned()`) would never let the launch close at the exit, and `run_records_a_launch_and_closes_it` would fail. This task copies the id and profile before `finish_run` and closes after it. The sighting is not delayed by that: it ends at `exited_at_ms`, read before `finish_run` (plan review). Only the launch ends when exit handling has finished, a few milliseconds to seconds after the child's exit; a late launch end over-covers, which spec §6.1 accepts.
2. **What the tests require of Task 16:** `poll_launch` reads `<session_records_dir(profile)>/<pid>.json` and records `(sessionId, home = account id, first_at = startedAt, source Run)`; `close_launch` moves `state.last_sighting`'s end forward to `exited_at_ms`, never back, and closes the launch at now only when the profile is not in use; a held launch reservation of another process counts as in use (`a_launch_stays_open_while_the_profile_is_in_use`, using `common::hold_launch`). `run_sights_the_child_session` asserts the order through the binary with the system clock, which is what the binary stamps with: the sighting's end, then the pause, then the launch's end. It compares clock reads only, never a duration.
3. **Batch G shares the `tagteam` crate with Task 19** (see Task 19's note 1); this test file also loads `tests/common/mod.rs`, which Task 19 extends.
4. **`open_launch` is a store write before the spawn**, so a busy store can delay the session by up to the store's 5 s busy timeout. Bounding it, as `CACHE_WRITE_WAIT` bounds the statusline's, would need an engine variant; it is left as is, and the reviewer may rule otherwise. The pid attach is no longer such a write (plan review): it is the poller's first act, so it never stands between the spawn and the first forward.
5. **What was verified.** `wait_polling` and `StopOnDrop`, exactly as above but with a stand-in engine and log macro, compiled, passed clippy and ran in a scratch build (the poller attached the pid before its first poll, the wait ran at once on the calling thread, and the poller stopped when the wait returned). The whole `run.rs` was rustfmt-checked but not compiled: it needs Task 16's API. `run_tokens.rs` was type-checked with clippy against stubs of `common` and the store API.

---

### Task 22: Command lists and the byte-identity invariant

**Depends on:** 19 (and 15, 16, 18 through it)
**Files:**
- Modify: `crates/tagteam-engine/tests/invariant.rs` (a session's transcript and record in the fixture's home; `tokens` and `sight_sessions` checks in `run_every_command_on`)
- Modify: `crates/tagteam/tests/run_shell_cli.rs` (`COMMANDS` gains `tokens`)
- Modify: `crates/tagteam/tests/home_cli.rs` (`COMMANDS` gains `tokens`)
- Modify: `crates/tagteam/tests/logging.rs` (`tokens` in the every-command trace; new `the_token_index_never_logs_content`)
- Test: the four files above

**Interfaces:**
- Consumes:
  ```rust
  // Task 15 / 16 / 18, tagteam_engine::tokens
  impl Engine {
      pub fn refresh_tokens(&self, provider: &ProviderId, wait: Wait, budget: Option<Duration>, cancel: &Cancel, progress: &mut dyn FnMut(&IndexProgress)) -> Result<RefreshOutcome, EngineError>;
      pub fn sight_sessions(&self, provider: &ProviderId) -> Result<u64, EngineError>;
      pub fn token_report(&self, q: &TokenQuery) -> Result<TokenReport, EngineError>;
  }
  pub enum RefreshOutcome { Done(IndexReport), Busy }          // IndexReport { complete, .. }, Copy
  pub struct TokenQuery { provider, period: TokenPeriod, grouping: TokenGrouping, since_ms, until_ms, accounts, models, session, tz: jiff::tz::TimeZone }
  // Task 7: FakeProcess { /* existing */ pub ppid: Option<u32> }, Default
  // Task 19: tagteam tokens; tests/common/mod.rs: turn, cc_timestamp, assistant_line, write_transcript
  ```
- Produces: coverage only. `tokens` is held to core §15.3's byte-identity invariant (spec §15 "CLI"), to the run shell's and the unusable `HOME`'s refusals, and to core §14.2's log rules.

- [ ] **Step 1: Write the failing tests**

`crates/tagteam-engine/tests/invariant.rs`. The transcript and the session record are written before the two checks' snapshots, so each check proves the refresh and the scan read them and wrote nothing outside tagteam's data directory (the snapshot walk skips it). The record's pid is live to the fixture's process probe, so the scan writes a sighting.

Hunk 1 adds the imports, hunk 2 the helpers before `run_every_command`, hunk 3 the two checks before the last one in `run_every_command_on`.

invariant.rs hunk 1. Replace

```rust
use serde_json::json;
use tagteam_cc::live::Platform;
use tagteam_engine::switch::{SwitchOutcome, SwitchRequest};
```

with

```rust
use serde_json::json;
use tagteam_cc::live::Platform;
use tagteam_core::time::format_iso8601;
use tagteam_engine::switch::{SwitchOutcome, SwitchRequest};
use tagteam_engine::tokens::{RefreshOutcome, TokenGrouping, TokenPeriod, TokenQuery, Wait};
use tagteam_provider::liveness::FakeProcess;
```

invariant.rs hunk 2. Replace

```rust
fn run_every_command(platform: Platform) {
```

with

```rust
/// Claude Code's line for one finished assistant response (token report spec §2.1): response
/// `n` of the fixture's session, at `at_ms`, with "x" as its only content.
fn transcript_line(n: u32, at_ms: i64) -> String {
    let seconds = format_iso8601(at_ms.div_euclid(1000));
    json!({
        "type": "assistant",
        "requestId": format!("req_inv_{n}"),
        "sessionId": "inv-session",
        "timestamp": format!("{}.000Z", seconds.trim_end_matches('Z')),
        "cwd": "/work/app",
        "isSidechain": false,
        "message": {
            "id": format!("msg_inv_{n}"),
            "model": "claude-opus-5-5",
            "content": [{"type": "text", "text": "x"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 20,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 30,
            },
        },
    })
    .to_string()
}

/// What `tokens` and `sight_sessions` read in the default home (token report spec §2.1, §2.4):
/// a session's transcript in the fixture's project, with two responses in the hour before the
/// fixture's clock, and the record of that session's live process.
fn seed_a_session_and_its_transcript(fx: &Fx) {
    let home = fx.paths().config_home;
    let now = fx.engine.now_ms();
    std::fs::write(
        home.join("projects/-work-app/inv-session.jsonl"),
        format!(
            "{}\n{}\n",
            transcript_line(1, now - 3_600_000),
            transcript_line(2, now - 1_800_000)
        ),
    )
    .unwrap();
    let sessions = home.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let record = json!({"pid": 4242, "sessionId": "inv-session", "cwd": "/work/app",
                        "startedAt": now - 7_200_000, "kind": "interactive"});
    std::fs::write(sessions.join("4242.json"), record.to_string()).unwrap();
    fx.process.set(
        4242,
        FakeProcess {
            exists: Some(true),
            ..FakeProcess::default()
        },
    );
}

/// The whole index, by day and account, in UTC.
fn whole_index(fx: &Fx) -> TokenQuery {
    TokenQuery {
        provider: fx.provider(),
        period: TokenPeriod::Day,
        grouping: TokenGrouping::Account,
        since_ms: None,
        until_ms: None,
        accounts: None,
        models: None,
        session: None,
        tz: jiff::tz::TimeZone::UTC,
    }
}

fn run_every_command(platform: Platform) {
```

invariant.rs hunk 3. Replace

```rust
    check(fx, "remove", || drop(fx.engine.remove(&k).unwrap()));
}
```

with

```rust
    // The token report (token report spec §15: `tokens` joins §15.3's byte-identity test). A
    // refresh reads the transcripts and the session records, and writes only tagteam's store.
    seed_a_session_and_its_transcript(fx);
    check(fx, "tokens", || {
        let outcome = fx
            .engine
            .refresh_tokens(
                &fx.provider(),
                Wait::Wait,
                None,
                fx.engine.cancel(),
                &mut |_| {},
            )
            .unwrap();
        assert!(
            matches!(outcome, RefreshOutcome::Done(report) if report.complete),
            "{outcome:?}"
        );
        let report = fx.engine.token_report(&whole_index(fx)).unwrap();
        assert_eq!(report.total.responses, 2, "the transcript was indexed");
    });
    check(fx, "sight_sessions", || {
        fx.engine.sight_sessions(&fx.provider()).unwrap();
    });

    check(fx, "remove", || drop(fx.engine.remove(&k).unwrap()));
}
```

`crates/tagteam/tests/run_shell_cli.rs`: under an unreadable marker, `tokens` refuses as every command but `statusline` does (core §12.8).

run_shell_cli.rs hunk 1. Replace

```rust
    &["history"],
    &["auto", "--once"],
```

with

```rust
    &["history"],
    &["tokens"],
    &["tokens", "month", "1", "--no-refresh"],
    &["auto", "--once"],
```

`crates/tagteam/tests/home_cli.rs`: an unusable `HOME` refuses `tokens` before anything is created (core §5).

home_cli.rs hunk 1. Replace

```rust
/// Commands that read or write state under `HOME`. On a fresh machine `add-token` creates the
/// store and the vault, so a fallback would show in the working directory.
const COMMANDS: [&[&str]; 5] = [
    &["list"],
    &["status"],
    &["switch"],
    &["history"],
    &["add-token", "sk-ant-api03-key"],
];
```

with

```rust
/// Commands that read or write state under `HOME`. On a fresh machine `add-token` creates the
/// store and the vault, and `tokens` the store, so a fallback would show in the working
/// directory.
const COMMANDS: [&[&str]; 6] = [
    &["list"],
    &["status"],
    &["switch"],
    &["history"],
    &["add-token", "sk-ant-api03-key"],
    &["tokens"],
];
```

`crates/tagteam/tests/logging.rs`: the imports, then `tokens` in the trace of every command (whose transcript's session ran under no stored identity), then a test of its own, appended to the end of the file, that a transcript's content never reaches the log or `--debug`'s stderr. The content sits in a response, a user's line, and an assistant line without usage, which the index skips and counts.

logging.rs hunk 1. Replace

```rust
use common::{cmd, expire_vault, login, seed_home, std_cmd, two_fresh_accounts};
```

with

```rust
use common::{
    assistant_line, cc_timestamp, cmd, expire_vault, login, now_epoch_s, seed_home, std_cmd, turn,
    two_fresh_accounts, write_transcript,
};
```

logging.rs hunk 2. Replace

```rust
    run(&["history", "1"]);
    run(&["history", "2", "--csv"]);
```

with

```rust
    run(&["history", "1"]);
    run(&["history", "2", "--csv"]);
    // The token report (token report spec §11), over a transcript of one of this home's
    // sessions: the index logs counts, paths and session ids, never an identity.
    write_transcript(
        root,
        "zq-session",
        &[turn(
            "claude-opus-5-5",
            now_epoch_s() * 1000 - 3_600_000,
            [10, 20, 30, 40, 50],
        )],
    );
    run(&["tokens"]);
    run(&["tokens", "session", "--json"]);
    run(&["tokens", "1", "--csv", "--no-refresh"]);
```

Append to the end of `crates/tagteam/tests/logging.rs`:

```rust
#[test]
fn the_token_index_never_logs_content() {
    // Token report spec §13: the index holds and logs counts, ids, times and paths, never a
    // transcript's content: not at TRACE in the file, nor under --debug on stderr. The content
    // is a string nothing else holds, in a response, a user's line, and an assistant line too
    // broken to read, which the index skips and counts.
    const CONTENT: &str = "zq-content-Rv8pXk3mQz7vTn2s";
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let env = Env::for_test(root);
    seed_home(&env);
    let at = now_epoch_s() * 1000 - 3_600_000;
    let response = turn("claude-opus-5-5", at, [10, 20, 30, 40, 50]);
    let user = json!({"type": "user", "sessionId": "zq-s", "timestamp": cc_timestamp(at - 60_000),
                      "message": {"role": "user", "content": CONTENT}});
    let broken = json!({"type": "assistant", "sessionId": "zq-s", "timestamp": cc_timestamp(at),
                        "message": {"id": "msg_zq_2", "content": CONTENT}});
    let dir = env.home.join(".claude/projects/-w");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("zq-s.jsonl"),
        format!(
            "{user}\n{}\n{broken}\n",
            assistant_line("zq-s", "msg_zq_1", "req_zq_1", &response, CONTENT)
        ),
    )
    .unwrap();
    let out = cmd(root)
        .env(TAGTEAM_LOG, "trace")
        .args(["tokens", "--json", "--debug"])
        .assert()
        .success()
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["total"]["responses"], 1, "the response was indexed: {v}");
    assert_eq!(
        v["index"]["skippedLines"], 1,
        "the broken line was counted: {v}"
    );
    let log = log_text(&state_dir(root));
    assert!(
        log.lines().any(|l| l.contains(" tagteam_engine::tokens")),
        "the refresh logged its counts:\n{log}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    for (what, text) in [("the log", log.as_str()), ("stderr", stderr.as_ref())] {
        assert!(
            !text.contains(CONTENT),
            "{what} holds transcript content:\n{text}"
        );
    }
}
```

- [ ] **Step 2: Run them and see them fail**

These tests cover code that Tasks 15–19 already wrote, so most pass at once; that is the point of the task (a regression net). Run them before Step 3 to confirm each one is reached and what it holds:

```
cargo test -p tagteam-engine --test invariant
cargo test -p tagteam --features test-support --test run_shell_cli --test home_cli --test logging
```

Expected: all pass, unless the earlier tasks left a gap; any failure here is a finding in that task (for example a refresh that writes beside a transcript, an index log line that quotes a line, or a log line at all from the index, which `the_token_index_never_logs_content` requires), to fix there, not to loosen here. To see the new checks bite, temporarily make the refresh write a file under `~/.claude/projects` (the invariant fails naming it), or log the raw line in the CC parser's error (the logging test fails naming the content); revert before going on.

- [ ] **Step 3: Implement**

Nothing beyond Step 1: this task adds coverage. If Step 2 found a gap, fix it in the file that owns it (Tasks 15, 16 or 19), with that task's tests.

- [ ] **Step 4: Run them and see them pass**

```
cargo test -p tagteam-engine --locked --test invariant
cargo test -p tagteam --locked --features test-support --test run_shell_cli --test home_cli --test logging
```

Expected: `invariant` 16 passed (the five tests that call `run_every_command_on` now run `tokens` and `sight_sessions` too); `run_shell_cli`, `home_cli` and `logging` pass, `logging` with one test more.

- [ ] **Step 5: Lint**

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings
```

- [ ] **Step 6: Commit**

```
git add crates/tagteam-engine/tests/invariant.rs crates/tagteam/tests/run_shell_cli.rs crates/tagteam/tests/home_cli.rs crates/tagteam/tests/logging.rs
git commit -m "Cover tokens in the invariant, run-shell, home and logging tests"
```

#### Notes for the controller

1. **`the_token_index_never_logs_content` asserts the refresh logs at least one line** under a `tagteam_engine::tokens` target at TRACE, so the content check cannot pass vacuously. Spec §13 has the index log counts and timings at info and debug; if Task 15 logs nothing, this test fails and the fix belongs in Task 15.
2. **Batch H.** This task only edits test files, but the three `tagteam` test files compile Task 20's `app.rs` and `statusline.rs` changes; run it after Task 20 lands, or re-run its focused tests then.
3. **What was verified.** Every hunk was applied to the real file and passes `rustfmt --check`; the invariant's helpers and checks were type-checked with clippy against a stand-in `Fx` and the contract's engine API, and the logging test against stubs of `common`. Nothing ran against the real engine, which Tasks 11–18 have not built yet.

---

### Task 23: Budgets

**Depends on:** 15, 18 (and 19 for the real-tree timing in Step 4)
**Files:**
- Create: `crates/tagteam-engine/tests/tokens_bench.rs` (release builds only; every test `#[ignore]`d, as `crates/tagteam/tests/perf.rs`)
- Test: `crates/tagteam-engine/tests/tokens_bench.rs`

**Interfaces:**
- Consumes:
  ```rust
  // Task 15
  impl Engine { pub fn refresh_tokens(&self, provider: &ProviderId, wait: Wait, budget: Option<Duration>, cancel: &Cancel, progress: &mut dyn FnMut(&IndexProgress)) -> Result<RefreshOutcome, EngineError>; }
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
  pub struct IndexReport { pub complete: bool, pub files_read: u64, pub responses_added: u64, pub responses_updated: u64, pub skipped_lines: u64, pub evidence_changed: u64, pub took_ms: u64 }
  // Task 18
  impl Engine { pub fn token_report(&self, q: &TokenQuery) -> Result<TokenReport, EngineError>; }
  // TokenPeriod and TokenGrouping are Copy; TokenReport { rows, total: TokenTotal { responses, .. }, .. }
  // Task 12 (store): record_sighting, SightingRow, SightingSource; tests/common: Fx (Claude Code over a temp home, a fixed clock)
  ```
- Produces: the timing evidence for spec §1.1's index and report budgets (criterion 3). No code outside the test.

Spec §1.1 times this machine's tree: 3.42 GB, 2,517 files, 127k responses in 30 days. The tests scale that to a synthetic tree, as spec §15 "Budgets" says:

- **Cold index:** 10 s for 3.42 GB is 1.46 s for 500 MB, so 1.5 s. The tree is 2,500 files, four of 50 MiB and the rest around 116 KB, one in ten a subagent's under `subagents/` whose responses never get a final line. Each response is a user line of up to 32 KiB, which the prefilter skips, then 1 to 9 assistant lines, so a response averages 20 KB (the real tree: 27 KB). The test also checks that every response was indexed once.
- **Nothing new:** 100 ms, unscaled: it is one `stat` per file and one store read, and the tree has the real file count.
- **A 30-day report:** 200 ms, unscaled, over a month of this machine's volume, 127,000 responses in 1,000 files, light ones so the index is quick to build. Each period (day, month, session, block) is timed, grouped by account, with a profile's sightings for one session in ten so attribution reads evidence; the median of five runs per period must be within budget.

The fixture's clock is fixed (`Fx`), so the tests time with `Instant`, never `IndexReport.took_ms`. They run one at a time, behind a mutex, as `perf.rs`'s do. Generation is deterministic (a fixed-seed xorshift), takes a few seconds, and is not timed.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-engine/tests/tokens_bench.rs`:

```rust
//! Token report spec §1.1's budgets for the index and the report, over synthetic transcripts in
//! Claude Code's format (spec §15 "Budgets"): a cold index of 500 MB, scaled from 10 s for this
//! machine's 3.42 GB; a refresh that finds nothing new, 100 ms; and a 30-day report of a
//! month's responses, 200 ms. Timings mean something only in an optimized build on an idle
//! machine, so this file exists only in release builds and its tests are ignored by default:
//! `cargo test --release -p tagteam-engine --test tokens_bench -- --ignored --nocapture`
//! (`--nocapture` prints each timing).
#![cfg(not(debug_assertions))]

mod common;

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use common::Fx;
use tagteam_core::time::format_iso8601;
use tagteam_engine::store::{SightingRow, SightingSource};
use tagteam_engine::tokens::{
    IndexReport, RefreshOutcome, TokenGrouping, TokenPeriod, TokenQuery, Wait,
};

const MIB: u64 = 1 << 20;
const DAY_MS: i64 = 86_400_000;
/// §1.1: 10 s for 3.42 GB is 1.46 s for 500 MB.
const COLD: Duration = Duration::from_millis(1_500);
const NOTHING_NEW: Duration = Duration::from_millis(100);
const QUERY: Duration = Duration::from_millis(200);
/// Spec §4 "Size": this machine's responses in 30 days, what a 30-day report reads.
const MONTH_RESPONSES: u64 = 127_000;
const MODELS: [&str; 4] = [
    "claude-opus-5-5",
    "claude-sonnet-5-5",
    "claude-haiku-4-5-20251001",
    "claude-fable-5-1",
];

/// The tests measure wall time, so they run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A deterministic generator (xorshift64*), so every run builds the same tree.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// What a generated tree holds.
#[derive(Debug, Default)]
struct Tree {
    files: u64,
    bytes: u64,
    responses: u64,
}

/// `at_ms` as Claude Code stamps a line: ISO 8601 UTC, to the millisecond.
fn timestamp(at_ms: i64) -> String {
    let seconds = format_iso8601(at_ms.div_euclid(1000));
    format!(
        "{}.{:03}Z",
        seconds.trim_end_matches('Z'),
        at_ms.rem_euclid(1000)
    )
}

/// The session id of generated transcript `file`; twenty share a project.
fn session_of(file: u64) -> String {
    format!("{:08x}-0000-4000-8000-{file:012x}", file / 20)
}

/// One generated transcript, as Claude Code lays them out (spec §2.1): a session's, or for one
/// file in ten a subagent's, under `subagents/`, whose responses never get a final line
/// (spec §2.3).
struct Transcript<'a> {
    out: BufWriter<File>,
    file: u64,
    session: String,
    subagent: bool,
    pad: &'a str,
    responses: u64,
    bytes: u64,
}

impl<'a> Transcript<'a> {
    fn create(config_home: &Path, file: u64, pad: &'a str) -> Self {
        let session = session_of(file);
        let subagent = file % 10 == 9;
        let dir = config_home.join(format!("projects/-work-p{}", file / 20));
        let path = if subagent {
            dir.join(&session)
                .join("subagents")
                .join(format!("agent-{file:016x}.jsonl"))
        } else {
            dir.join(format!("{session}.jsonl"))
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        Transcript {
            out: BufWriter::new(File::create(&path).unwrap()),
            file,
            session,
            subagent,
            pad,
            responses: 0,
            bytes: 0,
        }
    }

    /// One response at `at_ms`: a user line with `user_len` bytes of text, then its 1 to 9
    /// assistant lines, one per content block with up to `text_len` bytes each, all with the
    /// same ids and growing output (spec §2.2). The last is final unless this is a subagent's.
    fn response(&mut self, at_ms: i64, rng: &mut Rng, user_len: usize, text_len: usize) {
        let (file, n, session) = (self.file, self.responses, self.session.as_str());
        let ts = timestamp(at_ms);
        let mut lines = format!(
            "{{\"type\":\"user\",\"sessionId\":\"{session}\",\"timestamp\":\"{ts}\",\"message\":{{\"role\":\"user\",\"content\":\"{}\"}}}}\n",
            &self.pad[..user_len]
        );
        let model = MODELS[rng.below(MODELS.len() as u64) as usize];
        let blocks = if rng.below(10) < 2 {
            2 + rng.below(8)
        } else {
            1
        };
        let (input, write, read) = (rng.below(5_000), rng.below(20_000), rng.below(200_000));
        for block in 0..blocks {
            let stop = if block + 1 == blocks && !self.subagent {
                "\"end_turn\""
            } else {
                "null"
            };
            let text = &self.pad[..rng.below(text_len as u64 + 1) as usize];
            lines.push_str(&format!(
                "{{\"parentUuid\":null,\"isSidechain\":{side},\"type\":\"assistant\",\"requestId\":\"req_{file}_{n}\",\"sessionId\":\"{session}\",\"timestamp\":\"{ts}\",\"cwd\":\"/work/p\",\"message\":{{\"id\":\"msg_{file}_{n}\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"{model}\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}],\"stop_reason\":{stop},\"usage\":{{\"input_tokens\":{input},\"output_tokens\":{output},\"cache_creation_input_tokens\":{write},\"cache_creation\":{{\"ephemeral_5m_input_tokens\":{write},\"ephemeral_1h_input_tokens\":0}},\"cache_read_input_tokens\":{read},\"server_tool_use\":{{\"web_search_requests\":0}}}}}}}}\n",
                side = self.subagent,
                output = (block + 1) * 40,
            ));
        }
        self.out.write_all(lines.as_bytes()).unwrap();
        self.bytes += lines.len() as u64;
        self.responses += 1;
    }
}

impl Tree {
    fn add(&mut self, mut t: Transcript<'_>) {
        t.out.flush().unwrap();
        self.files += 1;
        self.bytes += t.bytes;
        self.responses += t.responses;
    }
}

/// 2,500 transcripts, 500 MB in all: four of 50 MiB, the rest about 116 KB each, at times in
/// the 30 days before `now_ms`. User lines of up to 32 KiB carry most of the bytes, as real
/// ones do, so a response averages about 20 KB (this machine: 3.42 GB for 127k responses a
/// month, 27 KB).
fn five_hundred_mb(config_home: &Path, now_ms: i64) -> Tree {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let pad = "x".repeat(32 * 1024);
    let (files, big, total) = (2_500u64, 4u64, 500_000_000u64);
    let small = (total - big * 50 * MIB) / (files - big);
    let mut tree = Tree::default();
    for file in 0..files {
        let target = if file < big {
            50 * MIB
        } else {
            small / 2 + rng.below(small)
        };
        let mut t = Transcript::create(config_home, file, &pad);
        while t.bytes < target {
            let at = now_ms - 1 - rng.below(30 * DAY_MS as u64) as i64;
            let user = rng.below(pad.len() as u64 + 1) as usize;
            t.response(at, &mut rng, user, 2_048);
        }
        tree.add(t);
    }
    tree
}

/// `MONTH_RESPONSES` light responses over the 30 days before `now_ms`, in 1,000 transcripts.
fn a_month_of_responses(config_home: &Path, now_ms: i64) -> Tree {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let files = 1_000u64;
    let mut tree = Tree::default();
    for file in 0..files {
        let mut t = Transcript::create(config_home, file, "");
        for _ in 0..MONTH_RESPONSES / files {
            let at = now_ms - 1 - rng.below(30 * DAY_MS as u64) as i64;
            t.response(at, &mut rng, 0, 0);
        }
        tree.add(t);
    }
    tree
}

/// One complete refresh, waiting for nothing: no other process indexes here.
fn refresh(fx: &Fx) -> IndexReport {
    let outcome = fx
        .engine
        .refresh_tokens(
            &fx.provider(),
            Wait::Wait,
            None,
            fx.engine.cancel(),
            &mut |_| {},
        )
        .unwrap();
    match outcome {
        RefreshOutcome::Done(report) => report,
        RefreshOutcome::Busy => panic!("nothing else refreshes this index"),
    }
}

fn median(mut runs: Vec<Duration>) -> Duration {
    runs.sort();
    runs[runs.len() / 2]
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / MIB as f64
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn a_cold_index_of_500_mb_is_within_1_5_s() {
    let _one_at_a_time = serial();
    let fx = Fx::new();
    let tree = five_hundred_mb(&fx.paths().config_home, fx.engine.now_ms());
    let started = Instant::now();
    let report = refresh(&fx);
    let took = started.elapsed();
    eprintln!(
        "cold index: {took:?} for {} files, {:.0} MiB, {} responses ({:.0} MiB/s)",
        tree.files,
        mib(tree.bytes),
        tree.responses,
        mib(tree.bytes) / took.as_secs_f64()
    );
    assert!(report.complete, "{report:?}");
    assert_eq!(report.responses_added, tree.responses, "{report:?}");
    assert!(took <= COLD, "a cold index of 500 MB took {took:?}");
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn a_refresh_with_nothing_new_is_within_100_ms() {
    let _one_at_a_time = serial();
    let fx = Fx::new();
    let tree = five_hundred_mb(&fx.paths().config_home, fx.engine.now_ms());
    assert_eq!(refresh(&fx).responses_added, tree.responses);
    let runs: Vec<Duration> = (0..10)
        .map(|_| {
            let started = Instant::now();
            let report = refresh(&fx);
            let took = started.elapsed();
            assert_eq!(
                (report.responses_added, report.responses_updated),
                (0, 0),
                "{report:?}"
            );
            took
        })
        .collect();
    let took = median(runs.clone());
    eprintln!(
        "nothing new: median {took:?} over {} files: {runs:?}",
        tree.files
    );
    assert!(
        took <= NOTHING_NEW,
        "a refresh with nothing new took {took:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn a_30_day_report_query_is_within_200_ms() {
    let _one_at_a_time = serial();
    let fx = Fx::new();
    let now = fx.engine.now_ms();
    let tree = a_month_of_responses(&fx.paths().config_home, now);
    assert_eq!(refresh(&fx).responses_added, tree.responses);
    // A profile's sightings, one session in ten, so attribution reads evidence too.
    let store = fx.engine.store().unwrap();
    for file in (0..1_000).step_by(10) {
        let sighting = SightingRow {
            session_id: session_of(file),
            home: "0192-bench-profile".into(),
            first_at: now - 30 * DAY_MS,
            last_at: now,
            source: SightingSource::Run,
            recorded_at: now,
        };
        store
            .record_sighting(&fx.provider(), &sighting, 60_000)
            .unwrap();
    }
    let periods = [
        ("day", TokenPeriod::Day),
        ("month", TokenPeriod::Month),
        ("session", TokenPeriod::Session),
        ("block", TokenPeriod::Block),
    ];
    for (name, period) in periods {
        let query = || TokenQuery {
            provider: fx.provider(),
            period,
            grouping: TokenGrouping::Account,
            since_ms: Some(now - 30 * DAY_MS),
            until_ms: None,
            accounts: None,
            models: None,
            session: None,
            tz: jiff::tz::TimeZone::UTC,
        };
        let report = fx.engine.token_report(&query()).unwrap();
        assert_eq!(report.total.responses, tree.responses, "{name}");
        let runs: Vec<Duration> = (0..5)
            .map(|_| {
                let started = Instant::now();
                fx.engine.token_report(&query()).unwrap();
                started.elapsed()
            })
            .collect();
        let took = median(runs.clone());
        eprintln!(
            "30-day {name} report: median {took:?} over {} responses, {} rows: {runs:?}",
            tree.responses,
            report.rows.len()
        );
        assert!(took <= QUERY, "a 30-day {name} report took {took:?}");
    }
}
```

- [ ] **Step 2: Run them and see them fail**

The file covers code Tasks 15 and 18 already wrote, so there is no failing state to produce; instead confirm the gating. In a debug build the file compiles to nothing, and in a release build its tests are ignored unless asked for:

```
cargo test -p tagteam-engine --test tokens_bench
cargo test --release -p tagteam-engine --test tokens_bench
```

Expected: `running 0 tests`, then `running 3 tests` with `3 ignored`.

- [ ] **Step 3: Implement**

No production code. If a budget fails in Step 4, report the timings: the fix belongs in the refresh (Task 15) or the view (Task 18), never in this file's budgets or tree.

- [ ] **Step 4: Run them and see them pass**

On an idle machine, with the timings printed:

```
cargo test --release -p tagteam-engine --locked --test tokens_bench -- --ignored --nocapture
```

Expected: 3 passed. Record in the task report each line the tests print: the cold index's time and MiB/s, the nothing-new median, and each period's median.

Then time the real thing, read-only against this machine's `~/.claude`, with tagteam's own state in a temp directory, so nothing but that temp store is written (transcripts and session records are only read). This step reads real transcripts, so it runs on Michael's machine only, never in CI:

```
cargo build --release -p tagteam --locked
TT=$(mktemp -d)
time XDG_DATA_HOME="$TT/data" XDG_STATE_HOME="$TT/state" XDG_CONFIG_HOME="$TT/config" ./target/release/tagteam tokens --json > /dev/null
time XDG_DATA_HOME="$TT/data" XDG_STATE_HOME="$TT/state" XDG_CONFIG_HOME="$TT/config" ./target/release/tagteam tokens --json > /dev/null
time XDG_DATA_HOME="$TT/data" XDG_STATE_HOME="$TT/state" XDG_CONFIG_HOME="$TT/config" ./target/release/tagteam tokens --json --no-refresh > /dev/null
du -sh "$TT/data/tagteam/tagteam.db"* ; rm -rf "$TT"
```

Record the three `real` times (cold, warm with nothing new, a query alone) and the store's size beside the tree's (`du -sh ~/.claude/projects`, and `find ~/.claude/projects -name '*.jsonl' | wc -l`). Spec §1.1's targets for this tree are 10 s, 100 ms and 200 ms; each of these commands also pays the process start and the settings read, so a few milliseconds over is not a miss, but an order of magnitude is.

- [ ] **Step 5: Lint**

The file is compiled only in release builds, so lint it there as well:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --release -p tagteam-engine --all-targets --locked -- -D warnings
```

- [ ] **Step 6: Commit**

```
git add crates/tagteam-engine/tests/tokens_bench.rs
git commit -m "Add timing tests for the token index and report"
```

#### Notes for the controller

1. **The query budget is not scaled.** Spec §1.1's 200 ms is for a 30-day report of this machine, which is 127k responses whatever the bytes; scaling it down with the 500 MB tree's 25k responses would have hidden a slow view. So the query test builds its own month of responses. Task 18's notes expect `Store::token_usage`'s per-row strings to dominate if it misses.
2. **The nothing-new test asserts only that no response was added or updated**, not `files_read == 0`: whether a skipped file counts as read is Task 15's to define.
3. **What was verified.** The generator (`Rng`, `Transcript`, `five_hundred_mb`'s per-file loop) ran in a scratch build: every line it writes parses as JSON, ids are unique per response, only non-subagent transcripts have final lines, and a response averages 20.0 KB. The whole file was type-checked and clippy-checked (`-D warnings`) against a stand-in `Fx` and the contract's engine API, and rustfmt-checked. Nothing ran against the real refresh or view.

---

### Task 24: Docs and core spec amendments

**Depends on:** all (1–23)
**Files:**
- Modify: `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md` (token report spec §16's hunks: §4.3, §4.5, §6.1, §6.4, §10.5, §12.5, §13.1, §13.5, Appendix C, and one link definition)
- Modify: `docs/superpowers/specs/2026-10-14-tagteam-token-report-design.md` (§4 and §13, for the plan's rulings R1, R7 and R8)
- Modify: `README.md` (Why, Commands, Where data lives, Network)
- Modify: `CHANGELOG.md` (`Unreleased`)
- Test: the check script in Step 1

**Interfaces:**
- Consumes: the behaviour Tasks 1–23 built, as each hunk describes it: the `transcripts` capability and its methods (Tasks 7–8), migration step 3 with rulings R1, R7 and R8 (Task 11), the purge's lock (Task 17), `tokens.retention_days` and the price tables (Task 13), `run`'s evidence (Task 21, which closes the launch after exit handling), the statusline's sighting (Task 20), and `tagteam tokens` (Task 19).
- Produces: the design records and user docs for the merge. No code.

Small hunks that cite the token report spec; no point-in-time fact is rewritten (the core spec's status, dates and verified Claude Code versions stay). The core spec cites the token report spec as `[token report spec] §n`, a reference link defined once at the end of the file, so no line carries the long path. The token report spec's own §4 and §13 change only to record the plan's rulings, cited as "plan ruling Rn".

- [ ] **Step 1: Write the failing check**

The check, run from the repository root, looks for every amendment and prints a `FAIL` line for each one missing:

```sh
sh -s <<'EOF'
core=${1:-docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md}
spec=${2:-docs/superpowers/specs/2026-10-14-tagteam-token-report-design.md}
readme=${3:-README.md}
changelog=${4:-CHANGELOG.md}
status=0
need() { if ! grep -qF -- "$2" "$1"; then echo "FAIL $1: $2"; status=1; fi; }
need "$core" 'The token index'"'"'s lock,'
need "$core" 'fn transcript_files(&self, env: &Env)'
need "$core" '- Without `transcripts`: `tokens` refuses for that provider'
need "$core" '**Token index.** Migration step 3'
need "$core" '**Price tables.** `[provider.<id>.prices."<model>"]`'
need "$core" '| `tokens.retention_days` | 365 | 30–3650 | — |'
need "$core" 'index lock is taken first, waiting for a running refresh'
need "$core" '`active_history` and `token_state` ([token report spec] §13)'
need "$core" '**Token evidence** ([token report spec] §6.1)'
need "$core" '| `tokens [day\|month\|session\|block]'
need "$core" '- **Session evidence.** Once the line is out'
need "$core" '`shell-init`, `tokens` | New |'
need "$core" '[token report spec]: 2026-10-14-tagteam-token-report-design.md'
need "$spec" 'adds nine tables and two triggers'
need "$spec" 'CREATE TABLE token_models'
need "$spec" 'key        INTEGER NOT NULL UNIQUE'
need "$spec" 'seeded INTEGER NOT NULL'
need "$spec" 'from the seven provider-keyed tables'
need "$readme" '| `tokens` | Token use and API cost'
need "$readme" '**The token index** holds counts'
need "$changelog" '- `tagteam tokens`: token use'
test -f "$(dirname "$core")/2026-10-14-tagteam-token-report-design.md" || { echo "FAIL: the link's target is missing"; status=1; }
exit $status
EOF
```

- [ ] **Step 2: Run it and see it fail**

Run Step 1's command before editing anything. Expected: 21 `FAIL` lines, one per amendment, and exit status 1.

- [ ] **Step 3: Apply the amendments**

`docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`, in order:

Core spec hunk 1 (§4.3, the lock order). Replace

```markdown
  ever tried, never waited for, and nothing is taken while it is held, so a log line may try it
  under any lock; the rules above leave it out.
```

with

```markdown
  ever tried, never waited for, and nothing is taken while it is held, so a log line may try it
  under any lock; the rules above leave it out. The token index's lock,
  `locks/tokens-<provider>.lock` ([token report spec] §5.1), is a leaf too: it may be taken
  under any other tagteam lock, and nothing is taken while it is held.
```

Core spec hunk 2 (§4.5, the trait sketch). Replace

```markdown
    fn doctor_checks(&self, env: &Env) -> Vec<Check>;    // §13.6; Check { id, status, message, fix }
}
```

with

```markdown
    fn doctor_checks(&self, env: &Env) -> Vec<Check>;    // §13.6; Check { id, status, message, fix }

    // The token report, with the `transcripts` capability (token report spec §3.2)
    fn transcript_files(&self, env: &Env) -> io::Result<Vec<PathBuf>>; // every transcript
    fn parse_usage_line(&self, line: &[u8]) -> Result<Option<UsageLine>, LineError>;
    fn price_table(&self) -> &'static PriceTable;     // the shipped API list prices
    // The default home's `sessions/`, whose records the scan and the statusline read:
    fn default_session_records_dir(&self, env: &Env) -> Option<PathBuf>;
}
```

Core spec hunk 3 (§4.5, the capabilities). Replace

```markdown
- Without `statusline`: the `statusline` command refuses for that provider.
```

with

```markdown
- Without `statusline`: the `statusline` command refuses for that provider.
- Without `transcripts`: `tokens` refuses for that provider, and nothing records its sessions
  ([token report spec] §3.2).
```

Core spec hunk 4 (§6.1, after History retention). Replace

```markdown
**History retention.** Samples older than `usage.history_retention_days` are pruned, at most
once a day, on the write path.
```

with

```markdown
**History retention.** Samples older than `usage.history_retention_days` are pruned, at most
once a day, on the write path.

**Token index.** Migration step 3 adds the token report's index and its session evidence:
`token_files`, `token_sessions`, `token_models`, `token_usage`, `session_sightings`,
`run_launches`, `active_history` (filled by two triggers on `active_accounts`), `token_state`
and `token_epoch`, with their upsert, seeding and retention in [token report spec] §4. Usage
older than `tokens.retention_days` is pruned at most once a day on the write path, as samples
are. `remove` deletes none of it: a removed account keeps its usage.
```

Core spec hunk 5 (§6.4, after Provider overrides). Replace

```markdown
the same entry in every `config` command.
```

with

```markdown
the same entry in every `config` command.

**Price tables.** `[provider.<id>.prices."<model>"]` tables override the token report's shipped
API list prices, field by field ([token report spec] §8.2). They are read forgivingly like every
setting, an invalid value keeping the shipped one with a warning, but they are not registry
keys: `config set` refuses them and the user edits the file. `config list` and `doctor` do not
report them as unknown.
```

Core spec hunk 6 (§6.4, the key table). Replace

```markdown
| `usage.history_retention_days` | 180 | 1–3650 | — |
```

with

```markdown
| `usage.history_retention_days` | 180 | 1–3650 | — |
| `tokens.retention_days` | 365 | 30–3650 | — |
```

Core spec hunk 7 (§10.5, step 9). Replace

```markdown
9. The rest of the provider's data, or of all data (below).
```

with

```markdown
9. The rest of the provider's data, or of all data (below). Each affected provider's token
   index lock is taken first, waiting for a running refresh, and held across the deletion, so a
   purge never falls between two batches of a refresh ([token report spec] §5.1).
```

Core spec hunk 8 (§10.5, With `--provider P`). Replace

```markdown
and `switch_journal`. Its `usage_requests` rows stay.
```

with

```markdown
and `switch_journal`, and its token index and evidence: its rows of `token_files`,
`token_sessions` (and so `token_usage`), `token_models`, `session_sightings`, `run_launches`,
`active_history` and `token_state` ([token report spec] §13); `token_epoch` stays. Its
`usage_requests` rows stay.
```

Core spec hunk 9 (§12.5, after Launch step 7). Replace

```markdown
7. Spawn `claude` with the reservation fd. It stays in the terminal's foreground process group
   (§14.1).
```

with

```markdown
7. Spawn `claude` with the reservation fd. It stays in the terminal's foreground process group
   (§14.1).

**Token evidence** ([token report spec] §6.1). For a provider with `transcripts`, `run` records
an open launch for the account just before step 7, and the child's pid once it is spawned. While
the child runs, a scoped thread reads the child's session record every 2 s, when its mtime
changed, and records the session it names as seen in the profile's home. After exit handling,
`run` ends that sighting and closes the launch, unless the profile is still in use. None of this
changes the exit code, and the plain-`claude` paths record nothing.
```

Core spec hunk 10 (§13.1, the command table). Replace

```markdown
| `history [ACCOUNT] [--window 5h\|7d\|spend\|<model>] [--since 14d] [--csv]` | §13.4 |
```

with

```markdown
| `history [ACCOUNT] [--window 5h\|7d\|spend\|<model>] [--since 14d] [--csv]` | §13.4 |
| `tokens [day\|month\|session\|block] [ACCOUNT...] [--unattributed] [--model M]... [--since 30d\|DATE] [--until DATE] [--by account\|model\|account,model\|none] [--tz ZONE] [--csv] [--no-refresh]` | Token use and API cost per account, from Claude Code's transcripts ([token report spec] §11) |
```

Core spec hunk 11 (§13.5, the statusline). Replace

```markdown
- It reads `usage_state` and prints one line using `statusline.format`.
```

with

```markdown
- It reads `usage_state` and prints one line using `statusline.format`.
- **Session evidence.** Once the line is out, it records the session it serves
  ([token report spec] §6.3): the id in `CLAUDE_CODE_SESSION_ID`, in the run shell's profile or
  the default home (and nowhere when `CLAUDE_CONFIG_DIR` names another directory), keyed by the
  session record of the Claude Code process it runs under. A sighting under a minute old costs
  one indexed lookup; otherwise an ancestor walk and one write, which waits at most 5 ms. It
  never creates the store, logs at debug at most, and a failure changes neither the line nor the
  exit code.
```

Core spec hunk 12 (Appendix C). Replace

```markdown
| — | `history`, `statusline`, `doctor`, `completions`, `shell-init` | New |
```

with

```markdown
| — | `history`, `statusline`, `doctor`, `completions`, `shell-init`, `tokens` | New |
```

Then append, after Appendix C's table (one blank line before it), the definition every `[token report spec]` above resolves to:

```markdown
[token report spec]: 2026-10-14-tagteam-token-report-design.md
```

`docs/superpowers/specs/2026-10-14-tagteam-token-report-design.md`, in order:

Token report spec hunk 1 (§4, the count of tables). Replace

```markdown
Step 3 of `MIGRATIONS` (core §6.1) adds eight tables and two triggers.
```

with

```markdown
Step 3 of `MIGRATIONS` (core §6.1) adds nine tables and two triggers.
```

Token report spec hunk 2 (§4, `token_models` and `token_usage` (rulings R7, R8)). Replace

```markdown
CREATE TABLE token_usage (
  key        INTEGER PRIMARY KEY, -- first 8 bytes of SHA-256(provider NUL message.id NUL requestId)
  session    INTEGER NOT NULL REFERENCES token_sessions(id) ON DELETE CASCADE,
  at         INTEGER NOT NULL,    -- epoch ms of the earliest line
  model      TEXT NOT NULL,
```

with

```markdown
CREATE TABLE token_models (       -- each model name once (ruling R8)
  id         INTEGER PRIMARY KEY,
  provider   TEXT NOT NULL,
  name       TEXT NOT NULL,
  UNIQUE (provider, name)
);

CREATE TABLE token_usage (
  id         INTEGER PRIMARY KEY, -- insertion order: a time range's rows share pages (ruling R7)
  key        INTEGER NOT NULL UNIQUE, -- 8 bytes of SHA-256(provider NUL message.id NUL requestId)
  session    INTEGER NOT NULL REFERENCES token_sessions(id) ON DELETE CASCADE,
  at         INTEGER NOT NULL,    -- epoch ms of the earliest line
  model      INTEGER NOT NULL REFERENCES token_models(id),
```

Token report spec hunk 3 (§4, `token_epoch` (ruling R1)). Replace

```markdown
CREATE TABLE token_epoch (       -- one row: when this store first ran a version that records
  at INTEGER NOT NULL            -- sightings and launches (§7.2), epoch ms
);
INSERT INTO token_epoch VALUES (<now ms>);
```

with

```markdown
CREATE TABLE token_epoch (       -- one row: when this store first ran a version that records
  at     INTEGER NOT NULL,       -- sightings and launches (§7.2), epoch ms
  seeded INTEGER NOT NULL        -- 1: the migration listed every profile (ruling R1)
);
INSERT INTO token_epoch VALUES (<now ms>, <seeded>);
```

Token report spec hunk 4 (§4, Seeding (ruling R1)). Replace

```markdown
  - the epoch as `ended_at`, unless the profile is in use (core §12.6). Then `ended_at` is
    NULL, and §6.1 closes it. That covers a `run` of the previous version still running.
```

with

```markdown
  - the epoch as `ended_at`, unless the profile is in use (core §12.6). Then `ended_at` is
    NULL, and §6.1 closes it. That covers a `run` of the previous version still running.

  `token_epoch.seeded` records whether the migration could list every profile to seed. When it
  could not (a `sessions/` it cannot read, or a raw `Store::open`, which lists none), coverage
  before the epoch (§7.2) is never complete, which can only leave a response unattributed (plan
  ruling R1).
```

Token report spec hunk 5 (§4, Size (rulings R7, R8)). Replace

```markdown
  machine's rate (127k responses in 30 days). The plan may normalize `model` into a lookup
  table if that is needed to stay within it.
```

with

```markdown
  machine's rate (127k responses in 30 days). The plan may normalize `model` into a lookup
  table if that is needed to stay within it. It does (`token_models`, plan ruling R8), and keys
  rows by insertion with the hash `UNIQUE` beside it (ruling R7): about 109 bytes a response,
  and a 30-day range read of a year's rows in 16 ms rather than up to 1.4 s.
```

Token report spec hunk 6 (§13, Purge (ruling R8)). Replace

```markdown
  - `purge --provider` deletes that provider's rows from the six provider-keyed tables;
```

with

```markdown
  - `purge --provider` deletes that provider's rows from the seven provider-keyed tables;
```

`README.md`, in order:

README hunk 1 (Why). Replace

```markdown
- **Run accounts side by side.**
```

with

```markdown
- **Count tokens per account.** `tagteam tokens` reports token use and its API-equivalent
  cost from Claude Code's transcripts, by day, month, session or 5-hour block, and which account
  each request went to.
- **Run accounts side by side.**
```

README hunk 2 (Commands). Replace

```markdown
| `history` | Usage history, burn rate, and when each window runs out |
```

with

```markdown
| `history` | Usage history, burn rate, and when each window runs out |
| `tokens` | Token use and API cost per account and model, by day, month, session or 5-hour block, from Claude Code's transcripts |
```

README hunk 3 (Where data lives: the store). Replace

```markdown
| Store (account metadata, usage, history) |
```

with

```markdown
| Store (account metadata, usage, history, the token index) |
```

README hunk 4 (Where data lives: the index). Replace

```markdown
- **The log** never contains an email, a token or any part of a credential.
```

with

```markdown
- **The token index** holds counts, model names, times, session ids and working directories
  read from Claude Code's transcripts, never their content, and response ids only as a hash. It
  keeps 365 days by default (`tokens.retention_days`), about 100 bytes a response: some 150 MB a
  year at a heavy user's rate. It outlives Claude Code's own 30-day transcript cleanup.
- **The log** never contains an email, a token or any part of a credential.
```

README hunk 5 (Network). Replace

```markdown
checks that these hosts are reachable and sends no credential. `tagteam statusline` makes no
  network request.
```

with

```markdown
checks that these hosts are reachable and sends no credential. `tagteam statusline` makes no
  network request, and neither does `tagteam tokens`: its prices ship with tagteam.
```

`CHANGELOG.md`:

CHANGELOG hunk 1 (Unreleased). Replace

```markdown
## [Unreleased]
```

with

```markdown
## [Unreleased]

### Added

- `tagteam tokens`: token use and its API-equivalent cost from Claude Code's transcripts, by
  day, month, session or 5-hour block, per account and per model, as a table, `--json` or
  `--csv`. Each request is credited to the account that made it where tagteam can tell (a
  `tagteam run` session, or the default login at the time), and shown as unattributed where it
  cannot. tagteam keeps the counts, never any content, in an index in its store, so the history
  outlives Claude Code's 30-day transcript cleanup; `tokens.retention_days` (default 365) bounds
  it. Prices ship with tagteam, and `[provider.claude-code.prices."<model>"]` tables in
  `config.toml` override them.
- `tagteam run` and `tagteam statusline` now record which session runs under which account, for
  the token report. Neither changes what it prints or its exit code, and the status line stays
  within its time budget.

### Changed

- `tagteam purge` waits for a running token index refresh before it deletes a provider's data,
  and deletes the index with it. `tagteam remove` keeps a removed account's token usage.
```

- [ ] **Step 4: Run it and see it pass**

Run Step 1's command again. Expected: no output, exit status 0. Then read each hunk once in a rendered view (`gh` or the editor's preview) to confirm the reference link resolves and the tables keep their columns.

- [ ] **Step 5: Lint**

No Rust changed, but the tree must stay green:

```
cargo fmt --all --check
git diff --check
```

- [ ] **Step 6: Commit**

```
git add docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md docs/superpowers/specs/2026-10-14-tagteam-token-report-design.md README.md CHANGELOG.md
git commit -m "Document the token report in the core spec, README and changelog"
```

#### Notes for the controller

1. **The token report spec's own amendments** (rulings R1, R7, R8, as the coordinator asked) go in this commit. They change §4's DDL and §13's count only; behaviour is as the spec says.
2. **§12.5's paragraph states Task 21's close order** (after exit handling), which departs from the outline; if the reviewer reverses that ruling, change "After exit handling" here too.
3. **Not amended:** core §15.3 (spec §16 does not list it; `tokens` joins its invariant through Task 22's test) and core §14.2 (the log rules already cover the index). The core spec's `**Status:**` stays `Implemented — PR #10`: these are additions in a later branch, recorded by the token report spec's own status.
4. **What was verified.** Every hunk was applied to a copy of the current file, each old text matching exactly once; the check fails 21 times on the current tree and passes on the patched copies (its link check aside, which needs the real directory); no new prose line passes 100 columns, except table rows, as in the files today.

---

## Rulings (planning)

R1–R6 are in the plan's header. The writers recorded these:

- **Parser (Task 8).** Input and output counts are required on an assistant line. Cache counts default to 0 when absent. Cache writes the 5m/1h split does not account for count as 5-minute. Discovery follows no link to a directory.
- **Prices (Task 9).** The table follows the pricing page as fetched on 2026-10-14. Where it differed from the outline, the page won: Sonnet 5.5 cache reads are $0.10, Opus 4.8 has a fast card, and Mythos 5 is added. Opus 4.6's fast card is its base card, because the page bills fast requests there at standard rates. Retired models are left out. The Mythos ids are inferred from the naming pattern.
- **Events (Task 18).** `switch-recovered` counts as a switch and `import` as an add. `remove` takes its account from `from_id`, as `lifecycle.rs` records it.
- **Block lookback (Task 18).** A block report reads 24 h before `since` and 5 h past `until`, so block starts don't shift as `since` moves.
- **Block totals (Task 18).** Blocks come from the account's whole usage, and filters choose which responses count. A block's total is the sum of its rows.
- **Drill (Task 18).** Day and month spans are clipped to the query's range; block spans are not. A session drill keeps the account and model filters and drops the range.
- **Index stats (Task 18).** `IndexState.files` and `responses` come from the last completed refresh (`token_state`).
- **Refusal (Task 18).** `token_index_state` refuses a provider without transcripts with `invalid-input`.
- **unpriced_models (Task 18).** It lists every model with a counted response that has no price, including a fast response whose model has no fast card.
- **RowKey (Task 18).** It gains `as_str()` and `Display`, `TokenTotal` gains `Default`, and every view type derives `Debug`.
- **Prices (Task 3).** An override for a dated model name gets its own entry. `price_overrides` lists only overrides that change a price. A card the model lacks needs all five rates, and a new long-prompt card also needs a threshold. A rate that is negative or not finite is ignored.
- **Placeholders (Tasks 2 and 14).** Each placeholder module holds one final contract item instead of a bare doc line, because clippy rejects a glob re-export of an empty module.
- **Compat (Task 1).** The check ids are kebab-case (`transcript-usage-fields`, `session-record-session-id`). The `/clear` check sends a prompt before and after the clear, to see which transcript the post-clear conversation lands in.
- **Time zones (Task 19).** An unknown `--tz` name is a usage error. A system zone that can't be determined falls back to UTC.
- **R7 (Task 11).** `token_usage` has `id INTEGER PRIMARY KEY` in insertion order, and `key INTEGER NOT NULL UNIQUE`. With the hash as rowid, a 30-day range read rows scattered over the table (180–1,365 ms at a year's volume); with R7 it takes 18 ms. Spec §4's DDL is amended in Task 24; behaviour is unchanged.
- **R8 (Task 11).** Model names live in a `token_models` table (spec §4 allows it). Listing the models drops from seconds to microseconds.
- **Store-kept counts (Task 12).** `token_state.responses` is maintained by the store: batches add and prunes subtract.
- **Prune (Task 12).** Prune deletes sightings whose `last_at` is before the cutoff. They can only cover pruned responses. It deletes in chunks of 5,000 rows.
- **Epoch clock (Task 11).** The epoch and the seeded `active_history` rows take the engine clock's `now`, one value for both. The seed closure returns `None` when the profiles can't be listed, which records `seeded = 0`.
- **run (Task 21).** `run` closes its launch after `finish_run`, because until exit handling releases `run`'s own reservation the profile counts as in use. The sighting's end is stamped then, slightly after the child exits. `open_launch` is a store write before the spawn and may wait up to the busy timeout. A failure there is a warning: the session runs, without a launch record.
- **CLI (Task 19).** `Command::Tokens` boxes its period, because of clippy's `large_enum_variant`. `--by` is parsed by the command, as `--include-api-key-accounts` is, and `account,model` is hidden from completion.
- **JSON (Task 19).** Where spec §11.2 left shapes open: a row's `period` object carries a `kind`; `total` carries `byBasis`; `index` uses spec §12's event keys.
- **Lock waits (Tasks 15 and 17).** `Wait::Wait` and purge's wait have no deadline; only the cancel token ends them. They use `lock_until_free`, which loops one-hour `FlockGuard::lock` rounds. A full purge holds every affected provider's tokens lock at once, taken in sorted order. That is the one exception to "nothing is taken while it is held", and it cannot deadlock, because a refresh takes only one tokens lock. An interrupted purge exits 130, and rerunning it finishes.
- **Refresh order (Task 15).** The refresh scans session records before it takes the lock, so the lock stays a strict leaf. `Try` therefore scans before it returns `Busy`.
- **Batching (Task 15).** The writer merges a response's lines within a batch, so the 2,000-row cap counts responses. A checkpoint commits only in the batch that holds the last of its chunk's rows.
- **Tail hash (Task 14).** The reader hashes the bytes it actually read for `tail_hash`, which also catches a rewrite during a read that started at 0.
- **Statusline write (Tasks 12 and 16).** The sighting uses `Store::record_sighting_within` with a 5 ms busy wait. `CACHE_WRITE_WAIT` becomes `pub(crate)`.
- **Index counts (Task 18).** `IndexState.responses` is the live count, which moves with every batch so a TUI can show progress during a cold index. `files` is as of the last completed refresh.
- **Prune budget (Task 15).** Each refresh call lets the first prune chunk through regardless of the budget, and stops at once only on cancel. So a spent budget still makes progress, and a slice overruns by at most one prune chunk.
- **Plan review (Codex, 12 slices).** Each fix followed a finding:
  - **Prices (Task 3).** A dated override keeps its entry even when it equals the shipped price.
  - **Timeline (Task 4).** The pre-epoch timeline uses only the row seeded at the epoch.
  - **Parser (Task 8).** It keeps only the first iteration, and no linked `subagents` directory is followed.
  - **Store (Tasks 11–12).**
    - The epoch is read inside the migration's lock.
    - Every prune chunk moves `changed_at`.
    - The prune is resumable within a budget.
  - **Settings (Task 13).** Price overrides are kept per provider.
  - **Reader (Task 14).** The head bytes actually read are validated at checkpoint.
  - **Refresh (Task 15).** The deadline starts after discovery and the lock.
  - **Evidence (Task 16).** A stale record with a reused PID is ignored. `close_launch` ends the sighting at the child's exit.
  - **Purge (Task 17).** A full purge locks every provider with token rows.
  - **Report (Task 18).** Each report uses its own provider's overrides.
  - **CLI (Task 19).** Named accounts choose the provider.
  - **run (Task 21).** The pid is attached on the poller thread.
  - **Tests.** Contention tests use hook points, and no functional test depends on elapsed time or today's date.
