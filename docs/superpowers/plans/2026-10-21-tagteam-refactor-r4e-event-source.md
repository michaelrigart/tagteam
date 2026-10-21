# tagteam refactor R4e — event source and kind — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Approved
**Branch:** `refactor-r4e-event-source`, based on `refactor-r4d-live-guard-error-kinds`
**Goal:** Each engine records events under its command's source, so quarantines, releases and recovered switches raised by `tagteam auto` or the daemon stop recording `cli`; event sources and kinds become types, with every stored string unchanged.
**Architecture:** `str_enum!` (core) spells `EventSource` and `EventKind` (`engine/event.rs`, Task 1). The CLI sets `EngineConfig.source` per command (Task 2). Per-call sources go; every writer reads `Engine::source()` (Task 3). `EventRow` and the store's event functions take the enums (Task 4).
**Tech Stack:** Rust 1.88 (edition 2024), rusqlite, tracing.

**Spec:** docs/superpowers/specs/2026-10-19-tagteam-review-refactor-design.md (§8.7, §8.9's CC-E-21 bullet; with §3, §6.4, §11, §12)

**Reading this plan.**
- **Sites** are functions at `8d44998`; line numbers are hints. R4b gives `finish_replacement` an `&AccountLock` and `recover_one` one `Recovery` holding the guard.
- **Focused tests:** `cargo test -p <crate> --locked`, with `--features test-hooks` for `tagteam-engine` and `tagteam-daemon`, `--features test-support` for `tagteam`.
- **Order:** 1 → 2 → 3 → 4, one at a time, each leaving its crates green.

## Global Constraints

- **Byte-identical output** (§1.4) except §11's §8.7 row (Task 3, with its `CHANGELOG.md` bullet). No wire, `--json` or schema change; `events.kind` and `events.source` keep their strings.
- **Log fields stay `&str`** (`kind = e.kind.as_str()`, quoted). Neither enum implements `Display`; `?kind` is forbidden.
- **Hardening's files** (§3.2): only `refresh.rs`'s two `"cli"` sources and test pin, and `tokens/refresh.rs`'s test constructor (one field; Ruling 12). `tokens/view/group.rs` compiles unchanged.
- **Held files** (§3.4): only `crates/tagteam-tui/tests/source.rs` (one field, one import). `app::build_engine` keeps its signature.
- **Comments** cite spec sections, wrap at 100; R4g does §14. Unit-test modules stay last, `#[cfg(test)]` alone above `mod tests {`.
- **CI** (`CONTRIBUTING.md`): fmt; both clippy variants in every task; `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`.
- **Tests and commits.** Only listed test edits, plus new tests; no new `#[allow]`. Commits: imperative, sentence case, no prefix, period or agent attribution.

## Review Focus

1. **A row a newer build wrote** with an unknown kind or source: left out of `events()`, still stored. Task 4: `events_leave_out_a_row_this_build_cannot_type_and_keep_it_stored`.
2. **A dead replacer's replacement reconciled by the daemon:** one `account-replaced` event, source `daemon`. Task 3: `a_landed_replacement_reconciled_under_the_daemon_records_the_daemon`.
3. **`tagteam daemon status|install|uninstall`** are commands, not the daemon: `cli`. Task 2: `each_command_records_its_events_under_its_own_source`.
4. **The log keeps a quoted string** (`source="daemon"`), never `Debug`'s `Daemon`. Task 3: `a_daemon_engine_records_and_logs_a_recovered_switch_as_the_daemon_s`.
5. **The TUI's lanes stay `cli`:** no test reaches the held callers; the reviewer checks `build_engine`.

## Spec coverage

| Spec | Task |
|---|---|
| §8.9 CC-E-21 `str_enum!`; §8.7's enums | 1 |
| §8.7 `EngineConfig.source`, per command | 2 |
| §8.7 gate, guards (§6.4's field), recovery, quarantine read `self.source()`; `with_source` goes; §11's row; §12's tests | 3 |
| §8.7 `EventRow` and the store's event functions take the enums | 4 |

---

### Task 1: `str_enum!` and the event vocabularies

**Depends on:** none.

**Files:** create `crates/tagteam-core/src/str_enum.rs` and `crates/tagteam-engine/src/event.rs`; modify both crates' `lib.rs` (`mod str_enum;`; `pub mod event;` and `pub use event::{EventKind, EventSource};`).

**Interfaces (produces):**

```rust
#[macro_export] // tagteam_core::str_enum! { .. }
macro_rules! str_enum {
    ($(#[$m:meta])* $vis:vis enum $name:ident {
        $($(#[$vm:meta])* $variant:ident => $spelling:literal),+ $(,)?
    }) => { .. }; // the enum, #[derive(Debug, Clone, Copy, PartialEq, Eq)], and:
}
impl $name {
    pub const ALL: [Self; N];              // declaration order
    pub fn as_str(self) -> &'static str;
    pub fn parse(s: &str) -> Option<Self>; // Some only for a spelling as_str writes
}
// event.rs. Docs: who raised an `events` row (core §6.1); every kind any build has written.
str_enum! { pub enum EventSource { Cli => "cli", Auto => "auto", Daemon => "daemon" } }
str_enum! { pub enum EventKind { Switch => "switch", SwitchRecovered => "switch-recovered",
    Quarantine => "quarantine", Unquarantine => "unquarantine", Add => "add",
    Import => "import", Remove => "remove" } }
```

- [ ] **Step 1: Write the tests.** They must fail to compile.
  - **`str_enum.rs`, `a_str_enum_spells_each_variant_and_parses_only_those_spellings`,** over a test enum with docs on it and on a variant, and a trailing comma: `Probe { One => "one", TwoWords => "two-words", }`. `ALL == [One, TwoWords]`; `as_str` and `parse` round-trip both; `parse` is `None` for `"One"`, `"two_words"`, `" one"`, `""`.
  - **`event.rs`, `every_event_kind_and_source_keeps_its_stored_spelling`:** each of the ten spellings above has `as_str() == s` and `parse(s) == Some(v)`; `parse(v.as_str()) == Some(v)` over both `ALL`s (a spelling used twice fails here); `EventSource::parse` is `None` for `"menubar"`, `"CLI"`, `""`, and `EventKind::parse` for `"Switch"`, `"switch "`, `"limit"`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Verify.** Core and engine focused tests; both clippy variants.
- [ ] **Step 4: Commit** "Add str_enum and the event source and kind vocabularies".

### Task 2: Each command's engine knows its event source

**Depends on:** Task 1.

**Files:**
- Modify: `engine.rs` (`EngineConfig`, `Engine`, `Engine::new`, `test_config`, tests); `crates/tagteam/src/cli.rs` (`impl Command`, tests); `app/context.rs` (`engine_for`, `build_engine`); `app/mod.rs` (the `engine_for` call).
- One `source:` line in every other `EngineConfig` literal (the compiler lists them), with its `use`:
  - **`Daemon`:** `crates/tagteam-daemon/src/testutil.rs` (`engine_on`), `crates/tagteam-daemon/tests/tokens.rs`;
  - **`Auto`:** `crates/tagteam/src/auto.rs` (tests, `Fx::new`);
  - **`Cli`:** the other 17: `statusline.rs` (`assemble`), the engine's unit-test and `tests/` constructors (`common/mod.rs` has five), hardening's `tokens/refresh.rs` (`tests::engine`) and the held `crates/tagteam-tui/tests/source.rs`.
- Test: `crates/tagteam/tests/auto_cli.rs`.

**Interfaces (produces):**

```rust
pub struct EngineConfig { /* .. */ pub source: EventSource }
impl Engine { pub fn source(&self) -> EventSource; }
impl Command { pub fn event_source(&self) -> EventSource; }
// Auto { .. } => Auto; Daemon { action: None } => Daemon; the rest => Cli
// app/context.rs: `engine_for` takes a trailing `source: EventSource`, as does
// `build_engine_as`, today's `build_engine`; `build_engine` keeps its signature and calls it
// with `EventSource::Cli`, for the held files.
```

- [ ] **Step 1: Write the tests.**
  - **`engine.rs`, `the_engine_records_events_under_the_source_it_was_built_with`:** for each of `EventSource::ALL`, `Engine::new(EngineConfig { source, ..test_config(..) }).source() == source`.
  - **`cli.rs`, `each_command_records_its_events_under_its_own_source`,** through `Cli::try_parse_from`: `auto` and `auto --once --dry-run` give `Auto`; `daemon` gives `Daemon`; `daemon status`, `daemon install`, `daemon uninstall`, `switch`, `tui`, `run` and `list` give `Cli`.
  - Both fail to compile.
  - **`auto_cli.rs`, `auto_records_its_switch_as_auto_and_a_later_switch_as_cli`,** a pin, green now and after Task 3: `switching(d.path())`; `auto(d.path(), &["--once"])` exits 0; the last of `store(d.path()).events()` gives `(kind.as_str(), trigger.as_deref(), source.as_str()) == ("switch", Some("proactive"), "auto")`. `cmd(d.path()).args(["switch", "2"])` succeeds; the last event is then `("switch", Some("manual"), "cli")`.
- [ ] **Step 2: Implement.** `app/mod.rs` passes `command.event_source()`.
- [ ] **Step 3: Verify.** Engine, daemon, `tagteam` and `tagteam-tui` focused tests; both clippy variants. Nothing reads `source()` yet: all else passes unchanged.
- [ ] **Step 4: Commit** "Give each command's engine its event source".

### Task 3: Every event takes the engine's source

**Depends on:** Task 2.

**Files:**
- Modify (engine unless named):
  - `engine.rs` (`GuardRequest`'s field and `Default` line; `guard`; `reconcile_replacement_as`); `recover.rs` (`recover_one`, `finish_forward`);
  - `quarantine.rs` (`quarantine_event`, `quarantine`, `unquarantine`, `release_unbound_quarantines`, unit test); `testutil.rs` (`T::sourced`);
  - `refresh.rs` (`persist_generation`'s and `run_gate`'s `unquarantine` calls only);
  - `switch/mod.rs` (`SwitchRequest`, `switch`, `switch_planned`); `switch/freshen.rs` (`prepare_usage`); `switch/transact.rs` (`settle_outgoing`, its call, the `switch` event);
  - `auto.rs` (`AutoEngine`, `Engine::auto`, `with_source`, `run`, `perform`, `recover`); `lifecycle.rs` (`event`, `commit_login`); `store/mod.rs` (`finish_replacement`);
  - `crates/tagteam-daemon/src/provider_job.rs` (`evaluate`); `crates/tagteam/src/app/commands/accounts.rs`; `CHANGELOG.md`.
- Test edits (Ruling 10): the engine `tests/` files Step 2 names. New tests: `gate.rs`, `unquarantine.rs`, `log_lines.rs`.

**Interfaces:**
- Consumes: Task 2's `Engine::source()`.
- Produces: `source` leaves `SwitchRequest`, `GuardRequest`, `AutoEngine`, and the parameters of `recover_one`, `finish_forward`, `quarantine_event`, `unquarantine`, `release_unbound_quarantines` and `settle_outgoing`; `with_source` goes. Besides:

```rust
pub fn finish_replacement(&self, lock: &AccountLock, source: EventSource, at: i64) -> Result<(), StoreError>;
pub(crate) fn sourced(source: EventSource) -> T; // testutil; T::new() is T::sourced(Cli)
/// On Fx and FakeFx: an engine over the fixture's Env, vault (`fixture_vault`), oracle, clock,
/// port and default settings, recording `source`. FakeFx's registers what FakeFx::new does.
pub fn engine_as(&self, source: EventSource) -> Engine;
```

- [ ] **Step 1: Write the tests** (§12). They fail: each source reads `cli`. Assertions read `.kind.as_str()`/`.source.as_str()` (Ruling 9).
  - **`gate.rs`, `a_quarantine_the_gate_raises_under_the_daemon_records_the_daemon`:** `invalid_grant_quarantines_bound_to_the_generation_sent` with the gate on `fx.engine_as(EventSource::Daemon)`: `Dead(InvalidGrant)`, and the `quarantine` event's source is `"daemon"`.
  - **`unquarantine.rs`, `the_gate_s_release_under_auto_records_auto`:** `a_refresh_after_the_gate_releases_a_stale_quarantine_records_one_event` on `fx.engine_as(EventSource::Auto)`: `one(&a, "credentials-replaced", "auto")`.
  - **`unquarantine.rs`, `a_landed_replacement_reconciled_under_the_daemon_records_the_daemon`:** `a_landed_replacement_reconciled_by_the_next_lock_holder_…`, ending `drop(fx.engine_as(EventSource::Daemon).lock_account(&a).unwrap())`: `one(&a, "account-replaced", "daemon")`.
  - **`log_lines.rs`, `a_daemon_engine_records_and_logs_a_recovered_switch_as_the_daemon_s`** (`one_at_a_time()`): `a`, `b`, `crashed_switch(&fx, &b, &a)`, `write_target_credential(&fx, &a)`, then `capture_logs(|| drop(daemon.mutation_guard().unwrap()))` with `daemon = fx.engine_as(EventSource::Daemon)`. The last event is `("switch-recovered", "daemon")`, and the `event recorded` line whose `kind` is `"\"switch-recovered\""` has `field(line, "source") == Some("\"daemon\"")`.
- [ ] **Step 2: Implement.**
  - **Readers.** `quarantine_event`, `event`, `finish_forward`'s row and the `switch` row write `self.source().as_str().into()` while `EventRow.source` is a `String`; `unquarantine` passes `self.source().as_str()`; `finish_replacement` passes `source.as_str()` to `clear_quarantine_on`, and `reconcile_replacement_as` and `commit_login` pass `self.source()`.
  - **Removed arguments.** `quarantine`'s and `refresh.rs`'s `"cli"`; the sources of `run`'s and `prepare_usage`'s releases; `switch` takes `GuardRequest::refusing_one(&req.provider)`, `switch_planned` `self.guard_or_refuse(&req.provider)?`, `recover` `GuardRequest::refusing_one(&self.provider)`; `evaluate` keeps `Some(auto)`; the CLI's `source: "cli"`.
  - **Docs** that promised a per-call source, or `cli` for a replacement, name the engine's (§8.7).
  - **The quarantine unit test** builds `T::sourced(EventSource::Daemon)`, drops its `"auto"` arguments and expects `"daemon"` for both events.
  - **Mechanical edits.** `source:` lines leave the `SwitchRequest` literals (`common/mod.rs` ×3, `fake_agent.rs` ×2, `strategy.rs`, `switch.rs` ×2, `switch_rollback.rs`). `release_unbound_quarantines(.., "cli")` drops it (`strategy.rs` ×10). `finish_replacement` calls gain `EventSource::Cli` (`common/mod.rs`, `provenance.rs`, `store.rs` ×9). A removed `"auto"` or `"daemon"` becomes `engine_as` with it, assertions unchanged:
    - `strategy.rs`: `a_quarantine_the_vault_has_moved_past_…`; `switch.rs`: `an_automatic_switch_records_its_departure_in_the_commit`;
    - `unquarantine.rs`: `the_switch_s_outgoing_capture_…` (over `[EventSource::Cli, EventSource::Auto]`, expecting `source.as_str()`) and both `the_tick_s_release_…`;
    - `auto_engine.rs`: the three `a_daemon_engine_…`; `invariant.rs`: the daemon closure;
    - `auto_tick.rs`: `a_proactive_tick_switches_and_records_its_departure` (`ffx.engine_as`), `a_tick_recovers_an_interrupted_switch_as_auto_and_goes_on`, `quarantines_other_processes_set_or_clear_…`.
- [ ] **Step 3: Verify.** Engine, daemon and `tagteam` focused tests; both clippy variants. Passing unchanged: Task 2's `auto_cli.rs` pin; `recover.rs::a_recovered_switch_records_no_auto_switch_state`; `log_lines.rs`'s add and switch tests (`source="cli"`); `refresh.rs::a_new_generation_rotates_prev_records_expiry_and_clears_the_quarantine` (`"cli"`).
- [ ] **Step 4: `CHANGELOG.md`.** Last bullet under `## [Unreleased]` → `### Changed`, after R4d's:

  > - Quarantines, quarantine releases and recovered switches that `tagteam auto` or the daemon carries out are recorded with `auto` or `daemon` as their source, in the event history and the log; some were recorded as `cli`.
- [ ] **Step 5: Commit** "Record every event under its engine's source".

### Task 4: `EventRow` and the store's event functions take the enums

**Depends on:** Task 3.

**Files:**
- Modify: `store/mod.rs` (`EventRow`, `event_from_row`, `events`, `log_event`, `insert_event_on`, `clear_quarantine_on`, `clear_quarantine`, `finish_replacement`); `quarantine.rs`; `lifecycle.rs` (`LoginSource.event`, `event`, `remove_locked`, `add_live`, `add_token`); `import.rs` (`import_over`, `import_new`); `purge.rs` (`remove_unregistered`); `recover.rs`; `switch/transact.rs`; `refresh.rs` (the pin only).
- Test edits, same literals throughout:
  - a kind or source compared to a literal, or read as a `String`, gains `.as_str()` (`.to_owned()` where owned): `auto_tick.rs:714,717`, `gate.rs:126,247`, `import.rs:70`, `replacement_unreadable.rs:137,147`, `strategy.rs:572,725,858`, `switch.rs:92,1373`, `unquarantine.rs:19,27`, `cancel.rs:276`, `storage_write.rs:294`, `auto_engine.rs:267`, and `refresh.rs`'s pin;
  - `EventRow` literals take variants: `store.rs` ×7 and its `switch_event` (`source: EventSource`); `tokens_view.rs`'s `event` uses `EventKind::parse(kind).expect(..)`;
  - `clear_quarantine` takes its literal's variant: `store.rs` ×3, `auto_tick.rs:1090`, `launch.rs:1968`, daemon `provider_job.rs:1312`.
- New test: `store.rs`.

**Interfaces (produces):**

```rust
pub struct EventRow { /* .. */ pub kind: EventKind, pub source: EventSource, /* .. */ }
fn event_from_row(r: &Row<'_>) -> rusqlite::Result<Option<EventRow>>; // None: unknown kind or source
/// Every event this build can read, in order; a newer build's row it cannot type is left out
/// and stays stored.
pub fn events(&self) -> Result<Vec<EventRow>, StoreError>;
// `source: EventSource` in clear_quarantine_on and Store::clear_quarantine; `kind: EventKind`
// in lifecycle's `event` and `quarantine_event`, and as `LoginSource.event`.
```

- [ ] **Step 1: Write the test** `events_leave_out_a_row_this_build_cannot_type_and_keep_it_stored` (`store.rs`). Raw SQL inserts `(at, provider, kind, source)` rows `(1, 'claude-code', 'switch', 'cli')`, `(2, 'claude-code', 'x-new-kind', 'cli')`, `(3, 'claude-code', 'switch', 'menubar')`. `events()` is one row (`at == 1`, `EventKind::Switch`, `EventSource::Cli`); `SELECT COUNT(*) FROM events` is 3. It fails to compile.
- [ ] **Step 2: Implement.** The store binds and logs `as_str()`. Writers pass `Quarantine`, `Unquarantine`, `Remove`, `Add`, `Import`, `SwitchRecovered`, `Switch`. Task 3's `self.source().as_str()` sinks become `self.source()`. Then the test edits.
- [ ] **Step 3: Verify.** Engine, daemon and `tagteam` focused tests; both clippy variants. `tokens_view.rs` and `log_lines.rs` pass unchanged.
- [ ] **Step 4: Commit** "Type the events row's kind and source".

## Final verification (the controller, before the PR)

- **Tests:** the CI commands on macOS outside the sandbox (passed/total); `cargo test` in the Linux Docker repro.
- **Held:** `git diff --stat refactor-r4d-live-guard-error-kinds..HEAD -- crates/tagteam-tui crates/tagteam/src/tui_host.rs crates/tagteam/src/tui_cmd.rs` lists only `crates/tagteam-tui/tests/source.rs`.
- **Hardening:** of §3.2's files, the range touches only `refresh.rs` (two calls, the pin) and `tokens/refresh.rs` (one field, one import).
- **No string source left:** `git grep -nE "source: &('static )?str|source: \"(cli|auto|daemon)\"|with_source" -- 'crates/*/src/**'` is empty (the test helper is `T::sourced`, so nothing is exempt).
- **Records:** the PR joins the spec's Status line; the final pre-merge commit marks this plan `Implemented`.

## Rulings (recorded under "Rulings" in the PR)

1. **`str_enum!` lives in `tagteam-core`, exported,** in the hand-written `Trigger` shape, so R4f's enums and any core pair it touches share it. Rejected: an engine-private macro. R4e touches, so converts, no existing pair.
2. **`EventKind` covers every kind any build has written** (§15), checked against the history, so older stores read back whole.
3. **`events()` leaves out a row it cannot type, which stays stored,** as `autoswitch_state` reads an unknown `left_trigger` as none. Residual: a newer build's new source on a `switch` row drops that switch from this build's token timeline. Rejected: failing the read, which breaks the token report, and a catch-all variant.
4. **`SwitchRequest.source` goes too,** so a switch and the recoveries and releases under its guard cannot disagree. Rejected: typing it, which keeps two sources per engine.
5. **Engine methods lose their source parameters;** the store's `clear_quarantine` and `finish_replacement` keep one, typed: a store has no engine.
6. **The replacement unquarantine takes its finishing holder's source** (§11): `tagteam add` still records `cli`; the daemon's reconciliation records `daemon`.
7. **The CLI derives the source from the command;** `daemon`'s management actions and the TUI's in-process commands are `cli`. `build_engine` keeps its signature and builds `cli` (§3.4).
8. **Fixtures:** the main fixture engine stays `cli`, like the commands that set a test up; `engine_as` builds others. The daemon crate's fixture is `daemon`, `tagteam`'s auto-loop one `auto`.
9. **Task 3's assertions use `.as_str()`,** valid before and after Task 4.
10. **A removed source argument becomes an engine built with it,** assertions unchanged; the quarantine unit test, its `"auto"` gone, pins the engine's source. §12's list omits these edits (as R4d's ruling 6).
11. **Plumbing before typing:** typing first would retype the per-call `&'static str`s Task 3 deletes.
12. **`tokens/refresh.rs`'s test constructor** gets the `source` line §8.7's field forces; §3.2 does not list it (flagged).
