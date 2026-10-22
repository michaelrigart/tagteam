# tagteam refactor R4f — the settings registry and small typed items — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Approved
**Branch:** `refactor-r4f-settings-small-items`, based on `refactor-r4e2-event-writers`
**Goal:** A settings registry the compiler checks (§8.8), one millisecond-to-second conversion (CC-E-8), one stated quarantine-reason fallback (CC-E-12), and serde structs for the store's hand-written JSON (CC-E-18, -19 part); every output and stored byte unchanged.
**Architecture:** Settings first (Tasks 1–2): enums through R4e's `str_enum!`, kinds holding their sets and ranges, and a crate-private `KeyId` matched exhaustively. Then one time helper (3), one reason reader (4) and two serde structs (5).
**Tech Stack:** Rust 1.88 (edition 2024); serde (derive), serde_json (`preserve_order`, `arbitrary_precision`), toml_edit, rusqlite.

**Spec:** docs/superpowers/specs/2026-10-19-tagteam-review-refactor-design.md (§8.8; §8.9's CC-E-8, -12, -18, -19; with §2, §3, §12). R4g carries CC-E-13, CC-E-16 and §14.

**Reading this plan.**
- **Sites** are functions at `8d44998`. R4b gives the replacement and journal mutators lock tokens (tests: `held_guard`/`held_lock`); R4e adds `str_enum!`, `EngineConfig.source` and `finish_replacement`'s `source`. Keep what R4a–R4e made of every line not named here.
- **Focused tests:** `cargo test -p <crate> --locked`, `--features test-hooks` for engine and daemon, `--features test-support` for `tagteam`.
- **Order:** 1 → 5, one at a time (1–2 share `settings.rs`; 3–4 share `auto.rs`, `doctor/accounts.rs`).

## Global Constraints

- **Byte-identical output** (§1.4): messages, warnings, `--json`, help, log lines, stored JSON. No §11 row: no `CHANGELOG.md`. No wire or schema change.
- **Hardening's files** (§3.2, plus the daemon's `client.rs` and the binary's `run.rs`/`prompt.rs`): only `refresh.rs`'s fallback in `run_gate` (Task 4). **Held files** (§3.4): none; none calls a changed API.
- **`str_enum!` is R4e's** (`tagteam_core::str_enum! { $vis enum T { V => "s", .. } }`, with `ALL`, `as_str`, `parse`); never redefine it. **No `#[serde(flatten)]`/`untagged`** (`arbitrary_precision` breaks both).
- Unit-test modules stay last, `#[cfg(test)]` alone above `mod tests {`. Comments wrap at 100, cite spec sections.
- **CI** (`CONTRIBUTING.md`): fmt; both clippy variants every task; the workspace test command.
- New tests only, no existing test edited; no new `#[allow]`. Commits imperative, sentence case, no prefix/trailing period/agent attribution.

## Review Focus

1. **A choice's spelling order** drives `expect`'s list. Task 1's first test.
2. **Stored JSON keeps its bytes;** a reordered key or dropped `null` strands a replacement after a crash (§12.5). Task 5's two pins.
3. **Conversion extremes:** an `i64::MAX` lease expiry must not overflow; negative ms round down. Task 3's first test.
4. **A newer build's quarantine reason** holds as `invalid_grant`; `doctor` and the auto event keep its spelling. Task 4's test; `every_quarantine_reason_has_its_words`.
5. **Malformed records are refused, never misread** (a truncated pid, a dropped mistyped field). Task 5's last two tests.

## Spec coverage

| Spec | Task |
|---|---|
| §8.8: `Choice` from `ALL`, `ColorMode::parse`, ranges once | 1 |
| §8.8: `KeyId`, exhaustive `value`/`apply`, `KEYS: &[Key]` | 2 |
| §8.9: CC-E-8; CC-E-12; CC-E-18/19 | 3; 4 (Ruling 5); 5 |

---

### Task 1: Derive the settings choices from their enums and declare each range once

**Depends on:** none.

**Files:** modify core `autoswitch.rs` (`AutoStrategy`); engine `settings.rs` (constants, `ColorMode`, `KeyKind`, `STRATEGIES`/`COLORS`, `KEYS`, `Key::expect`, `parse_item`, `parse_arg`, `apply`'s strategy and color arms, docs). Test: engine `tests/settings.rs`; `crates/tagteam/src/app/commands/daemon.rs` tests.

**Interfaces (produces):**

```rust
// core: str_enum! { pub enum AutoStrategy { Best => "best", ConsumeFirst => "consume-first" } }
// settings.rs: str_enum! { pub enum ColorMode { Auto => "auto", Always => "always", Never => "never" } }
//   (docs kept; both gain ALL, ColorMode gains parse)
/// What a `Choice` key's value must be: one of an enum's spellings, in its `ALL` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choices { Strategy, Color }
impl Choices { pub fn values(self) -> Vec<&'static str>; } // ALL.map(as_str)
pub enum KeyKind { /* others unchanged */
    Float(&'static RangeInclusive<f64>), Int(&'static RangeInclusive<i64>), Choice(Choices) }
pub const UNHEALTHY_TICKS_RANGE: RangeInclusive<i64> = 1..=100; // was u32
pub const HISTORY_RETENTION_DAYS_RANGE: RangeInclusive<i64> = 1..=3650;
pub const TOKENS_RETENTION_DAYS_RANGE: RangeInclusive<i64> = 30..=3650;
```

- [ ] **Step 1: Write the tests** (they must fail to compile).
  - **`the_choice_keys_say_their_enums_spellings_in_order`** (`tests/settings.rs`): `key("autoswitch.strategy").unwrap().expect()` is `"must be \"best\" or \"consume-first\""`, `key("ui.color")`'s is `"must be \"auto\", \"always\" or \"never\""`; their kinds are `Choice(Choices::Strategy)` and `Choice(Choices::Color)`; `Choices::Color.values() == ColorMode::ALL.map(ColorMode::as_str)`, likewise `Strategy`.
  - **`every_numeric_key_reads_its_declared_range`** (`tests/settings.rs`): each of the seven numeric keys' kind holds its constant (`autoswitch.threshold` is `Float(&THRESHOLD_RANGE)`, …, `tokens.retention_days` `Int(&TOKENS_RETENTION_DAYS_RANGE)`); the new constants equal `1..=3650` and `30..=3650`; both ends of the three `u32` fields' ranges pass `u32::try_from` (`whole`'s premise).
  - **`the_auto_flags_words_name_the_settings_ranges`** (`daemon.rs` tests): `BAD_THRESHOLD == format!("--threshold takes a number from {} to {}", THRESHOLD_RANGE.start(), THRESHOLD_RANGE.end())`; via `clap::CommandFactory`, `auto`'s `threshold`, `interval`, `cooldown` help ends with `({start}–{end})` (en dash) of `THRESHOLD_RANGE`, `INTERVAL_SECONDS_RANGE`, `COOLDOWN_SECONDS_RANGE`.
  - **Mechanical adaptations** to the new variant shapes, every asserted value unchanged and still written as a literal: `tests/config.rs`'s `every_numeric_key_in_the_registry_refuses_a_step_past_either_end` and `sample` (`Float(r)`/`Int(r)` read `r.start()`/`r.end()`; `Choice(c)` reads `c.values().last()`); `tests/settings.rs`'s kind table (`the_autoswitch_ranges_are_the_specs_table`, compared through the range's ends and `Choices::values()`), its `bounds` helper, and the `let KeyKind::Choice(values)` destructure. These are the only edits to existing tests.
- [ ] **Step 2: Implement.**
  - `KEYS`: `KeyKind::Float(&THRESHOLD_RANGE)` and so on (`&CONST` is promoted to `'static`); `Choice(Choices::Strategy)`, `Choice(Choices::Color)`. `STRATEGIES`/`COLORS` go.
  - `expect`, `parse_item`, `parse_arg` read `r.start()`, `r.end()`, `r.contains(..)`, `c.values()`; texts unchanged.
  - `apply`: `AutoStrategy::parse(&s).expect("parse_item admits only AutoStrategy::ALL's spellings")`, `ColorMode::parse` likewise; the silent `Best`/`Auto` fallbacks go.
  - Docs: numeric `Settings` fields name their constant ("in [`THRESHOLD_RANGE`]"); `KEYS`' and `whole`'s docs name the constants, not "M3b's".
- [ ] **Step 3: Verify.** Core, engine, `tagteam` focused tests; both clippy. Unchanged: `each_autoswitch_warning_says_what_its_key_accepts`, `an_invalid_color_falls_back_to_auto_with_a_warning`, `negative_numeric_flags_parse_and_clamp_to_their_ranges`; adapted only mechanically (Step 1): `the_autoswitch_ranges_are_the_specs_table`.
- [ ] **Step 4: Commit** "Derive the settings choices from their enums and declare each range once".

### Task 2: Dispatch the settings registry on `KeyId`

**Depends on:** Task 1.

**Files:** modify `settings.rs` (`KeyId`, `Key`, `KEYS`, `Settings::value`, `apply`, `Value`, a new last `mod tests`).

**Interfaces (produces):**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)] // doc: Ruling 2's reason for `pub(crate)`
pub(crate) enum KeyId {
    DefaultProvider, Threshold, IntervalSeconds, CooldownSeconds, HysteresisPct, Strategy,
    IncludeApiKeyAccounts, UnhealthyTicks, Models, HistoryRetentionDays, TokensRetentionDays,
    StatuslineFormat, ShareExtra, Color, DaemonAutoswitch, NotifyEnabled, NotifyEvents,
}
pub struct Key { pub(crate) id: KeyId, /* name, kind, per_provider unchanged */ }
impl Value { // private
    fn into_float(self) -> f64; fn into_int(self) -> i64; fn into_bool(self) -> bool;
    fn into_string(self) -> String; fn into_list(self) -> Vec<String>;
}
```

- [ ] **Step 1: Write the tests** (`settings.rs` tests; they must fail to compile).
  - **`every_key_id_is_in_the_registry_once`:** `KEYS`' ids are pairwise distinct; a failure names both keys.
  - **`each_key_applies_the_value_it_shows`:** for every key, `apply(key, Settings::default().value(key))` on a default `Settings` leaves `value(key)` equal to the default's, driving every `apply` arm with its `value` arm's variant.
- [ ] **Step 2: Implement.** Each `KEYS` entry gains its `id` first. `value` and `apply` match `key.id` with no wildcard or `unreachable!`; `apply` arms take their variant through the accessors, each of which panics on another variant with "KEYS pairs each kind with one Value variant". `parse_item`/`parse_arg` keep matching `kind`.
- [ ] **Step 3: Verify.** Engine, `tagteam` focused tests; both clippy. Unchanged: `every_key_is_read_from_a_full_file`, `inspect_and_load_read_every_key_alike`, the binary's config and completions tests.
- [ ] **Step 4: Commit** "Dispatch the settings registry on KeyId".

### Task 3: Convert milliseconds to seconds through one helper

**Depends on:** Task 2.

**Files:**
- Modify: core `time.rs` (helpers, module doc, test); engine `engine.rs` (`Engine::now_s`, a unit test).
- **The engine's production conversions:** `auto.rs` (`active_next_poll_at` ×2, `snapshot`, `cooldown_left`); `collect.rs` (`scheduled_candidates`, `record_to_store`; `Collection::now_s` goes); `doctor/accounts.rs` (`accounts`' truncating pair, `usage`); `switch/mod.rs` (`replan_polls`), `switch/select.rs` (`exhausted_message`, `auto_refusal`), `switch/transact.rs` (`apply`'s `at:`); `views.rs` (`usage_view`, `is_decision_grade`); `store/usage.rs` (`reserve_usage`, `authorize_send`); `transfer.rs` (`envelope` ×2).
- **Engine-clock reads elsewhere, and the daemon's ceiling:** daemon `auto.rs` (`run`), `daemon.rs` (`start_with`), `tokens.rs` (`report`), `backoff.rs` (`KeychainBackoff::current`), `provider_job.rs` (`ProviderJob::now_s` goes; its callers in `failed`, `tick`, `collect` read `self.engine.now_s()`); binary `app/mod.rs` (`App::now_s` goes; `commands/accounts.rs` ×2 and `commands/tokens.rs` read `self.engine.now_s()`).
- **Left as they are:** the truncating `now_ms / 1000` in `displace.rs::new_id` (the ID's second, which `is_displaced_id` reads as unsigned digits) and `engine.now_ms() / 1000` in `app/early.rs::run_statusline` keep their rounding. §8.9 converts the `div_euclid(1000)` sites, and changes only doctor's truncating pair.

**Interfaces (produces):**

```rust
/// Epoch milliseconds as whole seconds, rounded down (`div_euclid`): -1 ms is -1 s.
pub fn ms_to_s(ms: i64) -> i64;      // tagteam_core::time
/// Epoch milliseconds as whole seconds, rounded up; never overflows.
pub fn ms_to_s_ceil(ms: i64) -> i64; // ms.div_euclid(1000) + i64::from(ms.rem_euclid(1000) != 0)
impl Engine { /// The clock in whole seconds, rounded down.
    pub fn now_s(&self) -> i64; }
```

- [ ] **Step 1: Write the tests** (they must fail to compile).
  - **`milliseconds_become_seconds_rounded_down_or_up`** (`time.rs`): `ms_to_s` 0 → 0, 999 → 0, 1000 → 1, 1999 → 1, -1 → -1, -1000 → -1, -1001 → -2; `ms_to_s_ceil` 0 → 0, 1 → 1, 1000 → 1, 1001 → 2, -1 → 0, -999 → 0, -1000 → -1, `i64::MAX` → `9_223_372_036_854_776`.
  - **`now_s_is_the_clock_in_whole_seconds_rounded_down`** (`engine.rs`): on `FakeClock::new(1_999)` (else `test_config`), `now_s() == 1`; on `FakeClock::new(-1)`, -1.
- [ ] **Step 2: Implement.** Engine-clock reads become `now_s()`; parameters and fields `ms_to_s(x)`; the two ceilings `ms_to_s_ceil`.
- [ ] **Step 3: Verify.** Core, engine, daemon, `tagteam` focused tests; both clippy.
- [ ] **Step 4: Commit** "Convert milliseconds to seconds through Engine::now_s and the time helpers".

### Task 4: Read a stored quarantine reason with one stated fallback

**Depends on:** Task 3.

**Files:** modify `quarantine.rs` (`QuarantineReason`, tests), `active.rs` (`active_quarantine`), `refresh.rs` (`run_gate`'s `Dead(..)` line), `auto.rs` (`QuarantineWatch::report`'s `set` branch), `doctor/accounts.rs` (`quarantine_words`).

**Interfaces (produces):**

```rust
// str_enum! { pub enum QuarantineReason { InvalidGrant => "invalid_grant",
//   NoRefreshToken => "no_refresh_token", IdentityConflict => "identity_conflict",
//   SuccessorLost => "successor_lost" } }   (docs kept; From<DeadReason> stays)
impl QuarantineReason {
    /// The reason a stored `quarantine_reason` (§6.1) is acted on as. One this build does not
    /// know, which a newer build may store, holds as `InvalidGrant` does (§7.4).
    pub fn from_stored(stored: &str) -> QuarantineReason;
}
```

- [ ] **Step 1: Write the test** `a_stored_reason_this_build_does_not_know_holds_as_invalid_grant` (`quarantine.rs`): `from_stored(r.as_str()) == r` over `ALL`; `"refresh_revoked"`, `""`, `"Invalid_Grant"` give `InvalidGrant`. It must fail to compile.
- [ ] **Step 2: Implement.** `active_quarantine` and `run_gate` call `from_stored`. `quarantine_words` keeps its signature and texts, matching `QuarantineReason::parse(reason)` per variant plus `None => "it cannot be refreshed"`. `report`'s `set` branch becomes `else if let Some(reason) = row.quarantine_reason.as_ref().filter(|_| set)`, the event carrying `reason.clone()`.
- [ ] **Step 3: Verify.** Engine focused tests; both clippy. Unchanged: `reasons_round_trip_their_column_values`, `every_quarantine_reason_has_its_words`, `gate.rs`'s `invalid_grant_quarantines_bound_to_the_generation_sent`, `auto_tick.rs`'s two quarantine-report tests.
- [ ] **Step 4: Commit** "Read a stored quarantine reason with one stated fallback".

### Task 5: Write the replacement metadata and journal snapshots through serde structs

**Depends on:** Task 4.

**Files:** modify `store/mod.rs` (`begin_replacement`, `finish_replacement`, `replacing_meta_parses`, `insert_journal`, `journal_from_row`, `set_wal_mode`; `journal_to_json`/`journal_from_json` go). Test: `crates/tagteam-engine/tests/store.rs`.

**Interfaces (produces):**

```rust
/// What a replacement records with its marker (§12.5): its keys' one declaration, stored order.
#[derive(Serialize, Deserialize)]
struct ReplacementMeta {
    identity_key: String, label: String, email: Option<String>,
    #[serde(default)] org_uuid: String,
    org_name: Option<String>, account_uuid: Option<String>, kind: String,
    #[serde(default)] identity_json: Value,
    login_expires_at: Option<i64>,
    #[serde(default)] from_live: bool,
}
impl ReplacementMeta {
    fn of(meta: &LoginMeta<'_>) -> ReplacementMeta;
    /// `Err` is `ReplacementUnreadable`'s detail, which never quotes the metadata.
    fn read(stored: &str) -> Result<ReplacementMeta, String>;
}
/// A journal row as `prior` stores it (§9.6), keys in stored order.
#[derive(Serialize, Deserialize)]
struct JournalSnapshot {
    provider: String, holder_pid: u32, holder_start: u64, from_id: Option<String>,
    to_id: String, from_fp: Option<String>, from_identity: Option<Value>, to_fp: String,
    to_epoch: Option<i64>, started_at: i64, prior: Option<Box<JournalSnapshot>>,
}
impl From<&JournalRow> for JournalSnapshot { .. }
impl From<JournalSnapshot> for JournalRow { .. }
/// `v` as compact JSON; infallible for these structs (string keys, derived `Serialize`).
fn to_json(v: &impl Serialize) -> String; // the one `expect`
```

`read`, in order: not JSON gives `"it is not JSON"`; `identity_key`, `label` or `kind` (that order) absent or not a string gives `"it has no {field} field"`; any other field mistyped gives `"one of its fields has the wrong type"`.

- [ ] **Step 1: Write the tests** (`tests/store.rs`, with R4b's `held_lock`/`held_guard` over `Env::for_test(d.path())`).
  - **`a_replacement_records_its_metadata_as_it_did`** (a pin, passes now): `begin_replacement` with `login_meta(&identity("a@x.co"), true)`; `replacing_meta(&a)` is exactly `json!({"identity_key": "a@x.co\n", "label": "a@x.co", "email": "a@x.co", "org_uuid": "", "org_name": null, "account_uuid": null, "kind": "oauth", "identity_json": {"emailAddress": "a@x.co"}, "login_expires_at": null, "from_live": true}).to_string()`; `finish_replacement` then leaves `replacing_fp` `None`.
  - **`a_prior_snapshot_is_stored_as_it_was`** (a pin): with `commit_switch_is_…`'s `j` and `replaced`, the raw `prior` column after inserting `replaced` equals `json!` of `j`'s eleven keys in `JournalSnapshot`'s order (`"prior": null`); `journal(&cc())` is `replaced`.
  - **`a_prior_snapshot_whose_pid_exceeds_u32_is_malformed`:** plant `prior` by raw SQL with `"holder_pid":4294967296`, all else valid; `journal(&cc())` is `Err`. FAILS now (truncates to 0).
  - **`replacement_metadata_details_say_what_is_wrong_without_quoting_it`:** one begun replacement of `a`; each planted metadata gives `ReplacementUnreadable(d)` from `finish_replacement`, `d` holds no `secret`, and the marker stays:

    | Metadata | `d` |
    |---|---|
    | `{"identity_key": "secret@x.co` | `it is not JSON` |
    | `{"identity_key":"secret@x.co"}` | `it has no label field` |
    | `{"identity_key":"secret@x.co","label":"secret@x.co","kind":"oauth","login_expires_at":"secret@x.co"}` | `one of its fields has the wrong type` |

    `replacing_meta_parses` is false for the third, which FAILS now (it lands).
- [ ] **Step 2: Implement.** `begin_replacement` binds `to_json(&ReplacementMeta::of(meta))`. `finish_replacement` installs from `ReplacementMeta::read` (`Err(d)` → `ReplacementUnreadable(d)`), its other steps as R4e leaves them. `replacing_meta_parses` is `read(meta).is_ok()`. `insert_journal` binds `to_json(&JournalSnapshot::from(p))`; `journal_from_row` reads `serde_json::from_value::<JournalSnapshot>`, any error the existing `malformed()` failure. `set_wal_mode` waits `Instant::now() + BUSY_TIMEOUT`, its doc naming it.
- [ ] **Step 3: Verify.** Engine focused tests; both clippy. Unchanged: `store.rs`'s malformed-snapshot, pre-`to_epoch` and `recorded_replacement_metadata_parses_…` tests; `replacement_unreadable.rs`; `doctor.rs`'s `replacing_meta` cases.
- [ ] **Step 4: Commit** "Write replacement metadata and journal snapshots through serde structs".

## Final verification (the controller, before the PR)

- **Tests:** every CI command on macOS outside the sandbox (passed/total); `cargo test` in the Linux Docker repro.
- **Untouched:** `git diff --stat refactor-r4e-event-source..HEAD` lists no held file, and of §3.2's files only `refresh.rs`.
- **Greps:** engine production code outside hardening's files has no `div_euclid(1000)` or `+ 999)`, and its only `/ 1000` is `displace.rs::new_id`'s; `settings.rs` no `unreachable!`; nothing `unwrap_or(QuarantineReason::InvalidGrant)`.
- **Records:** the PR joins the spec's Status line; the final pre-merge commit marks this plan `Implemented`.

## Rulings (recorded under "Rulings" in the PR)

1. **Split:** CC-E-13, CC-E-16 and §14 go to R4g; one plan would pass 20 KB, and plan plus diff about 95 KB.
2. **`KeyId` is crate-private, without a spelling:** names stay in `KEYS`; `dead_code` refuses an unnamed variant, a test a duplicate. Rejected: `str_enum!` with dotted names, making `Key.name` a method at eight call sites for no stronger check.
3. **`ColorMode`, `AutoStrategy`, `QuarantineReason` move onto `str_enum!`,** being touched (CC-E-21). `KeyKind::Choice(Choices)` computes its values, as the macro's `as_str` is not `const`. Rejected: a kind per enum.
4. **Ranges once:** kinds hold `&'static` refs to `*_RANGE`; `UNHEALTHY_TICKS_RANGE` becomes `i64` like its kind (`whole`'s premise pinned). Clap help and `BAD_THRESHOLD` stay literal, tied to the constants by a test. Rejected: copying `min`/`max` out of the constants, today's second declaration.
5. **CC-E-12's helper serves the sites that act on a reason** (gate, active refresh). Display sites keep the stored spelling: `doctor` matches exhaustively with an explicit unknown arm; the auto event binds the reason it proved present. The finding's defect was showing an unknown reason as `invalid_grant`.
6. **CC-E-8's reach:** every engine `div_euclid(1000)` conversion (the review's 14, `transfer.rs` ×2) and doctor's truncating pair; the truncating `displace.rs::new_id` and `run_statusline` keep their rounding; engine-clock reads in the daemon and binary, both ceilings; other parameter conversions there stay. `ms_to_s` joins `ms_to_s_ceil` (§15), which cannot overflow, unlike `(ms + 999)`.
7. **Serde reads check every field's type** (a snapshot pid over `u32` included: CC-E-19's `try_from`). Only data no tagteam build writes reads differently, and it is refused, never misread (§1.4); details never quote stored text. No `CHANGELOG.md`.
