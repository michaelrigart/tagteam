# tagteam refactor R2 — engine structure — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Approved
**Goal:** Split `switch.rs` and `doctor.rs` into modules, give the engine one guard entry point, replace ten of its eleven `too_many_arguments` allows with context structs, and move four helpers to where they belong, with every output byte-identical.
**Architecture:**
- **Moves.** Pure moves land first, each in its own commit, so `git diff --color-moved` shows them as moved. Edits follow in their own commits.
- **New types.** `GuardRequest` replaces the guard-function family in `engine.rs`. `Settle`, `Recovery`, `LockedSwitch`, `ApplyStep`, `ProfileCtx`, `AddRequest` and `LoginWrite` bundle the parameters that are threaded together today.
- **Order.** The switch tasks run last, because `review-hardening` also edits `switch.rs`.
**Tech Stack:** Rust 1.88 (edition 2024), cargo workspace; `tracing`; tests with the workspace's fakes.

**Spec:** docs/superpowers/specs/2026-10-19-tagteam-review-refactor-design.md (§6, with §3 and §14)

**Reading this plan.**
- **Line numbers** are as of `e1b34b8`. They drift as tasks land, so find each site by the code quoted.
- **Focused test commands:**
  - `cargo test -p tagteam-engine --locked --features test-hooks`;
  - `cargo test -p tagteam-daemon --locked --features test-hooks`;
  - `cargo test -p tagteam --locked --features test-support`;
  - `cargo test -p tagteam-core --locked`.
- **Each task** leaves the workspace compiling, with its crates' tests green.
- **Fact sheets.** A task that points to `.superpowers/research/r2-facts/*.md` means the item inventories there. Those are measured, not decided. Where a sheet and this plan differ, this plan wins.

## Global Constraints

- **Byte-identical output.** Every message, JSON field, exit code, doctor check ID and order, log line, and log target stays byte-identical. R2 has no user-visible change and no `CHANGELOG.md` entry (spec §11 has no R2 row). The existing tests that assert text are the check, and none of them is loosened.
- **No wire or store change.** The daemon protocol's JSON, the `--json` schemas and the store schema stay as they are.
- **Hardening-owned files are not edited.** They are:
  - in `tagteam-provider`: `src/security.rs`, `src/process.rs`, `src/keychain.rs` and the fakes;
  - in `tagteam-engine`: `src/refresh.rs`, `src/store/tokens.rs` and `src/tokens/*`;
  - the loops in `tagteam-daemon/src/server.rs`, `notify.rs` and `scheduler.rs`;
  - `.github/workflows/*` and `dist-workspace.toml`.

  There are two sanctioned exceptions, both in Task 1: one import line at the top of `tagteam-daemon/src/notify.rs`, outside the loops, and one `pub mod fmt;` line of its own in `tagteam-core/src/lib.rs`.
- **Held TUI files are not edited:** `crates/tagteam-tui/**`, `crates/tagteam/src/tui_host.rs` and `crates/tagteam/src/tui_cmd.rs`. `tagteam_engine::words::duration` keeps its path, so the TUI's calls to it compile unchanged.
- **Pure moves are separate from edits.** Tasks 1, 2, 4, 6 and 11 are move tasks.
  - **What a move commit may contain:**
    - the moved items, byte-identical;
    - new files and `mod` lines;
    - `use` lines and re-exports;
    - visibility raised to the minimum that compiles (`pub(super)` or `pub(crate)`);
    - rustfmt's reflow of those lines.
  - **What it may not contain:** a body edit, a rename, or a comment edit. The reviewer reads each move commit with `git diff --color-moved=zebra --color-moved-ws=allow-indentation-change`.
- **Paths that survive.** Hook-point names (`hooks::point("…")`), every `crate::switch::…` and `tagteam_engine::switch::…` path, and every `tagteam_engine::doctor::…` path survive every move. Re-export them from the new `mod.rs` (spec §3.1).
- **Neutrality test.** In every file that holds a unit-test module, that module stays the last item, with `#[cfg(test)]` alone on the line directly above `mod tests {`. `crates/tagteam-fake/tests/neutrality.rs` treats everything above it as production code.
- **CI commands** (from `CONTRIBUTING.md`):
  - `cargo fmt --all --check`;
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`;
  - `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`.
- **No agent attribution** in commits: no `Co-authored-by:` agent line, no session link, no "Generated with" footer.
- **Commit messages** are imperative and in sentence case, with no prefix and no trailing period, matching `git log`. Each task's message is given in its last step.
- **No test is weakened.** The only test edits allowed are:
  - mechanical signature changes;
  - a test moving with the code it tests;
  - Task 2's split of one test into two with the same assertions;
  - new tests.
- **No new `#[allow(...)]`.** After Task 13, `crates/tagteam-engine/src` holds exactly one `clippy::too_many_arguments` allow: `store/tokens.rs:547`, which belongs to hardening.

## Review Focus

These are the five failure modes the spec implies but that no existing test exercises, most likely to bite a user first. Each one's test is added to the task that owns it.

1. **Log targets of the switch's moved events.**
   - The split moves seven `tracing` calls into submodules. The default target `module_path!()` would then change from `tagteam_engine::switch` to `…::switch::rollback`, `…::freshen` or `…::transact`.
   - A user's log file would change, and so would which lines a `TAGTEAM_LOG` directive matches exactly. Only the unverified-capture line is pinned, by `crates/tagteam/tests/logging.rs:100`.
   - Test: Task 10, `a_rolled_back_switch_logs_under_the_switch_target` (`crates/tagteam-engine/tests/log_lines.rs`).
2. **Each recovery mode of the one guard.**
   - `metadata_guard` must recover without asking the oracle, because no network is allowed under the guard. `purge_guard` must step over a journal row it cannot decode, where the others fail.
   - Folding three functions into one `match` is where these swap silently.
   - Tests: Task 7, `a_metadata_guard_recovers_without_asking_the_oracle` and `a_purge_guard_steps_over_an_undecodable_journal_row` (`crates/tagteam-engine/src/engine.rs` tests, or `tests/recover.rs` if its fixtures are needed). Skip either one only if an existing test already pins exactly that behaviour; the report then names that test.
3. **The fallback-save error text.**
   - "could not save a credential the Keychain fallback would delete: {e}" is built in two places, `switch.rs:2181-2197` and `active.rs:679-695`. No test pins it.
   - `Settle` makes it one function.
   - Test: Task 12, `a_failed_fallback_save_names_the_credential_it_protects` (`crates/tagteam-engine/src/switch/support.rs` tests).
4. **A refusing guard over several providers.**
   - `import` refuses when any named provider has an interrupted switch. It returns that provider's own blocked-recovery error if recovery was blocked, else `InterruptedSwitch`.
   - The `swap_remove` lookup moves into `guard`.
   - Test: Task 7, `a_refusing_guard_returns_the_blocked_error_of_the_provider_it_names`, with the same skip rule as item 2.
5. **`FileId` in purge's envelope refusal.**
   - Purge refuses a path whose `(dev, ino)` matches a protected file. That covers a hard link to it, and a symlink chain `trace_path` walks to it.
   - The tuple becomes a struct, and its comparisons must keep comparing both fields.
   - Test: Task 5, `a_hard_link_to_a_protected_file_is_still_refused` (`crates/tagteam-engine/tests/purge.rs`), with the same skip rule.

## Spec coverage

| Spec | Tasks |
|---|---|
| §6.1 step 1, `switch.rs` becomes `switch/` | 10, 11 |
| §6.1 step 2, `Settle` | 12 |
| §6.1 step 3, `transact` as named steps | 13 |
| §6.2, `too_many_arguments` | 7 (none), 8, 9, 12, 13 |
| §6.3, `doctor.rs` becomes `doctor/` | 2, 3 |
| §6.3, `span` to `core/fmt.rs` | 1 |
| §6.4, the guard API | 7 |
| §6.5, `fsutil.rs`, `FileId`, `seed_launches` | 4, 5, 6 |
| §3.2, redo the switch move after the rebase | 11 (the split script) |
| §14, "Decision N" where code moves | 3 (doctor), 14 (switch) |

## Task order and batches

Every task compiles `tagteam-engine`, so there are no parallel batches: tasks run one at a time, in order. Each task's `Depends on:` line records why its place is fixed.

**Before Task 10:** check whether `review-hardening` has merged into `main`.
- If it has, the controller rebases R1 and the tasks done so far onto it first. Tasks 10 to 14 then run on the rebased base.
- If it has not, they run here, and Task 11's script regenerates the move after the later rebase (spec §3.2).

---

### Task 1: Move the duration formatter `span` into tagteam-core's `fmt` module

**Depends on:** none.

**Files:**
- Create: `crates/tagteam-core/src/fmt.rs`
- Modify: `crates/tagteam-core/src/lib.rs`, `crates/tagteam-core/src/rank.rs`, `crates/tagteam-core/src/autoswitch.rs:6`
- Modify (import lines only):
  - `crates/tagteam-engine/src/doctor.rs:24`;
  - `crates/tagteam-engine/src/switch.rs:3-6`;
  - `crates/tagteam-engine/src/auto.rs:14`;
  - `crates/tagteam-engine/src/words.rs:50`;
  - `crates/tagteam-daemon/src/notify.rs:15`.

**Interfaces:**
- Produces: `tagteam_core::fmt::span(secs: i64) -> String`, unchanged. `tagteam_core::rank::span` no longer exists.

- [ ] **Step 1: Move the function and its test.**
  - Cut `span` (`rank.rs:106-120`, doc comment included) and its test `spans_read_as_days_hours_or_minutes` (`rank.rs:393-409`).
  - Paste them into the new `fmt.rs`: first the function, then `#[cfg(test)]` on its own line, `mod tests { use super::*; … }` holding the test.
  - Add `pub mod fmt;` to `tagteam-core/src/lib.rs`, on a line of its own in the module list.
- [ ] **Step 2: Repoint every import.**
  - `autoswitch.rs:6` becomes `use crate::fmt::span;` plus `use crate::rank::{binding_window, blocked_until};`.
  - Each engine and daemon import drops `span` from its `rank` import and gains `use tagteam_core::fmt::span;`.
  - `words.rs:50` becomes `pub use tagteam_core::fmt::span as duration;`.
- [ ] **Step 3: Verify.**
  - Run `cargo test -p tagteam-core --locked` and the engine and daemon focused commands. Expected: PASS.
  - Run `cargo build -p tagteam --locked --features test-support`. Expected: it builds, the TUI included.
  - Run `rg -n 'rank::.*span|rank::span' crates`. Expected: no hits.
- [ ] **Step 4: Commit** "Move the duration formatter span into tagteam-core's fmt module".

### Task 2: Split `doctor.rs` into one module per area

**Depends on:** Task 1, which edits `doctor.rs:24`.

**Files:**
- Modify: `crates/tagteam-engine/src/doctor.rs`, which becomes `crates/tagteam-engine/src/doctor/mod.rs`
- Create, all under `crates/tagteam-engine/src/doctor/`: `store.rs`, `files.rs`, `daemon.rs`, `accounts.rs`, `recovery.rs`, `profiles.rs`, `sessions.rs` and `online.rs`

**Interfaces:**
- Produces:
  - `tagteam_engine::doctor::{DoctorOptions, DoctorReport}` and `Engine::doctor`, unchanged.
  - In `doctor/files.rs`: `pub(super) fn readable`, `entries` and `subdirs`.
  - In `doctor/store.rs`: `pub(super) fn restore`.
  - Task 3 uses all of these.

Facts: `.superpowers/research/r2-facts/doctor.md` §1, §2 and §4.

- [ ] **Step 1: Split test 2 (a non-move commit).**
  - `the_daemon_check_and_a_held_reservation_name_the_agent` (`doctor.rs:3289-3322`) uses `Run::daemon_check` and `held_reservation`. After the split, those two items live in different files.
  - Replace the test with two tests in the same place, with assertions unchanged:
    - `the_daemon_check_names_the_agent` gets the `daemon_check` half;
    - `a_held_reservation_names_the_agent` gets the `held_reservation` half.
  - Each copies only the setup lines it uses.
  - Run the engine focused command. Commit "Split the doctor's daemon-check and held-reservation test in two".
- [ ] **Step 2: Rename the file.**
  - Run `git mv crates/tagteam-engine/src/doctor.rs crates/tagteam-engine/src/doctor/mod.rs`, and check that it builds.
  - Commit "Move doctor.rs to doctor/mod.rs".
- [ ] **Step 3: Extract the children.** Each item moves byte-identical:

  | File | Items |
  |---|---|
  | `mod.rs` | the `//!` doc, `DoctorOptions`, `DoctorReport` (+impl), `Stored`, `Secrets` (+`bad`), `Run`, `secrets_of`, `command`, `Engine::doctor`, `Run::{push, store_ref, all_accounts, secrets, provider_secrets, scope, finish}`, `and_list`, `worst`, `state_of`, `skipped`; tests `a_list_of_names_reads_as_a_sentence`, `a_report_is_ok_until_a_check_fails` |
  | `store.rs` | `is_damage`, `settled_quick_check`, `restore_backup`, `out_of_reach`, `restore`, `Run::store`; test `an_integrity_failure_stands_only_when_a_second_run_finds_it_too` |
  | `files.rs` | `mode_of`, `entries`, `subdirs`, `readable`, `Run::{modes, temp_files, settings, log}` |
  | `daemon.rs` | `MANAGER_TIMEOUT`, `daemon_record_check`, `service_state_check`, `Run::{daemon, daemon_running, held_by_daemon, daemon_service, daemon_socket, daemon_notify}` |
  | `accounts.rs` | `EXPIRY_NOTICE_MS`, `engine_record_check`, `quarantine_words`, `Run::{vault_orphans, accounts, replacement, usage, auto, models}`; tests `every_quarantine_reason_has_its_words`, `an_engine_whose_liveness_cannot_be_told_is_a_warning_with_a_fix` |
  | `recovery.rs` | `Run::{rescues, displaced, journal, dead_switch}` |
  | `profiles.rs` | `held_reservation`, `Run::{move_then_delete, profile_dirs, stray_profiles, profile, spelling, splits, reservations, unknown_entries}`; tests `a_reservation_whose_lock_went_away_is_not_reported_as_unknowable`, `a_held_reservation_names_the_agent` |
  | `sessions.rs` | `Run::{sessions, daemon_check, ownership, ownership_walk, orphan_ownership}`; test `the_daemon_check_names_the_agent` |
  | `online.rs` | `ONLINE_TIMEOUT`, `Run::{online, keychain_note}` |

  - **Impl blocks.** Each child holds its `Run` methods in its own `impl Run<'_> { … }`. Associated functions called as `Self::…` stay inside such a block.
  - **Visibility.** Raise to `pub(super)` exactly the items fact sheet §2 lists: 18 that `Engine::doctor` calls, and 9 called across siblings. Nothing else changes visibility.
  - **`use` lines** per sheet §2. Remember the trait-only imports: `std::os::unix::fs::PermissionsExt` in `files.rs` and `daemon.rs`, and `FileTypeExt` in `daemon.rs`. Each moved test module carries the imports its test needs; `super::*` covers only its own file.
  - **Order.** `Engine::doctor` keeps its calls in today's order, because the report's order is that order.
- [ ] **Step 4: Verify.**
  - Run the engine focused command; `engine/tests/doctor.rs` must pass unchanged.
  - Run `cargo test -p tagteam --locked --features test-support --test doctor_cli`. Expected: PASS.
  - Run clippy, both variants. Expected: clean.
- [ ] **Step 5: Commit** "Split the doctor's checks into one module per area".

### Task 3: Name the doctor's check IDs and build its repeated fixes once

**Depends on:** Task 2.

**Files:**
- Modify: every file in `crates/tagteam-engine/src/doctor/`

**Interfaces:**
- Consumes: Task 2's layout.
- Produces:
  - In `doctor/mod.rs`, `struct Id;` with one `pub(super) const NAME: &str = "…";` per check ID.
  - In `doctor/store.rs`, `pub(super) fn warn_restore(engine: &Engine, id: &str, message: impl Into<String>) -> Check` and the private `fn store_skipped(why: &str) -> Check`.

Facts: `.superpowers/research/r2-facts/doctor.md` §5 and §6.

- [ ] **Step 1: Check IDs.**
  - Add `struct Id;` and `impl Id { … }` in `mod.rs`, with the 53 consts that fact sheet §5 names. Each const's name is its ID upper-cased, with `.` and `-` turned into `_`; for example, `Id::ACCOUNTS_VAULT` is `"accounts.vault"`.
  - Replace every production occurrence of an ID literal with its const (133 sites). Delete the five local `const ID` items (`doctor.rs:262`, `:302`, `:1232`, `:1379` and `:1476` in the old numbering).
  - Test literals stay literals, because they are the pin.
- [ ] **Step 2: Builders.**
  - `warn_restore` returns `Check::warn(id, message).fix(restore(engine))`. Each of the nine `.fix(restore(self.engine))` sites becomes `warn_restore(self.engine, Id::…, …)`, and `restore` becomes private to `store.rs`.
  - `store_skipped(why)` returns `Check::info(Id::STORE_SKIPPED, format!("the checks that read the store's rows were skipped: {why}"))`.
    - The two sites at `:558` and `:604` pass `"the store cannot be read"`.
    - The one at `:709` passes `"the store is not one this tagteam can read"`.
- [ ] **Step 3: "Decision N" (spec §14).**
  - Six comments in `doctor/` cite "Decision N", at old `doctor.rs:3`, `:4`, `:1173`, `:1230`, `:2794` and `:2843`.
  - For each, find the plan that numbers it (`rg -n "Decision <N>" docs/superpowers/plans`; the cited subject identifies which plan).
  - Rewrite the comment as the rule in words, or as a spec section reference, keeping the sentence's meaning.
- [ ] **Step 4: Verify.**
  - Run the engine focused command, `--test doctor_cli`, and clippy (both variants).
  - Run `rg -n '"(accounts|auto|daemon|keychain|log|online|pending|sessions|settings|store|switch|usage)\.[a-z-]+"' crates/tagteam-engine/src/doctor`. Expected: hits only in `Id`'s consts and in test modules.
- [ ] **Step 5: Commit** "Name the doctor's check IDs and build its restore and skipped checks once".

### Task 4: Move the path tracing and file identity helpers into `fsutil`

**Depends on:** none.

**Files:**
- Create: `crates/tagteam-engine/src/fsutil.rs`
- Modify:
  - `crates/tagteam-engine/src/lib.rs`, adding `mod fsutil;`;
  - `crates/tagteam-engine/src/lifecycle.rs` (190-264, and the test module 1166-1266);
  - `crates/tagteam-engine/src/purge.rs:26`.

**Interfaces:**
- Produces: `crate::fsutil::{trace_path, identity, absent}`, still `pub(crate)`, with their signatures unchanged. `MAX_LINK_HOPS` and `names` stay private, now in `fsutil`.

Spec §6.5's `engine/fsutil.rs` names this crate's root, like its `engine/vault.rs`. The crate has no `engine/` directory.

- [ ] **Step 1: Move.**
  - Move `MAX_LINK_HOPS`, `trace_path`, `names`, `identity` and `absent` (`lifecycle.rs:190-264`) into `fsutil.rs`, byte-identical, with the imports they need.
  - Move lifecycle's whole `#[cfg(test)] mod tests` (1166-1266, the `root()` helper and seven `trace_path` tests) to the end of `fsutil.rs`.
- [ ] **Step 2: Repoint.**
  - `lifecycle.rs` gains `use crate::fsutil::{absent, identity, trace_path};` and drops the imports it no longer needs (`VecDeque`, `OsString`, `MetadataExt`, `Component`; clippy confirms).
  - `purge.rs:26` takes `absent, identity, trace_path` from `crate::fsutil`.
- [ ] **Step 3: Verify.** Run the engine focused command and clippy. Expected: PASS and clean.
- [ ] **Step 4: Commit** "Move the path tracing and file identity helpers into fsutil".

### Task 5: Name the file identity `FileId`

**Depends on:** Task 4.

**Files:**
- Modify: `crates/tagteam-engine/src/fsutil.rs`, `crates/tagteam-engine/src/lifecycle.rs`, `crates/tagteam-engine/src/purge.rs`
- Test: `crates/tagteam-engine/tests/purge.rs`

**Interfaces:**
- Produces: in `fsutil.rs`, `pub(crate) struct FileId { dev: u64, ino: u64 }` with `pub(crate) fn of(meta: &fs::Metadata) -> FileId`. It derives `Clone, Copy, Debug, PartialEq, Eq`, plus `Hash` only if a set needs it. `fsutil::identity` is gone.

- [ ] **Step 1: The tests** (Review Focus 5).
  - **The hard link.** Write `a_hard_link_to_a_protected_file_is_still_refused`, unless an existing purge test already pins it (the report then names that test).
    - Purge must refuse a path that is a hard link to a file its envelope protects, exactly as it refuses that file's own path.
    - Run it. Expected: PASS on today's code, because this is a regression pin.
    - Confirm it can fail: temporarily make purge compare the paths themselves in place of their file identities, see the test fail, and revert. A hard link shares both device and inode, so it cannot tell the two fields apart.
  - **Both fields.** After Step 2, add `a_file_id_compares_device_and_inode` to `fsutil.rs`'s test module.
    - Assert that `FileId { dev: 1, ino: 5 }` is not equal to `FileId { dev: 2, ino: 5 }`, nor to `FileId { dev: 1, ino: 6 }`, and is equal to itself.
- [ ] **Step 2: The rename.**
  - `identity(&meta)` becomes `FileId::of(&meta)` everywhere: `lifecycle.rs` 497, 500 and 522, and `purge.rs` 948-1007.
  - `Vec<(u64, u64)>` becomes `Vec<FileId>` at `purge.rs:931`, `:964` and `:965`.
  - The doc comment becomes `/// What a file is, whatever it is called: its device and inode.`
- [ ] **Step 3: Verify.** Run the engine focused command and clippy.
- [ ] **Step 4: Commit** "Name the file identity FileId".

### Task 6: Move `seed_launches` next to the session state it reads

**Depends on:** none.

**Files:**
- Modify: `crates/tagteam-engine/src/engine.rs` (234-282), `crates/tagteam-engine/src/session.rs`

**Interfaces:**
- Produces: `Engine::seed_launches(&self) -> Option<Vec<SeedLaunch>>`, now `pub(crate)` in `session.rs`'s `impl Engine`. Both callers (`engine.rs:210` and `:223`) are unchanged.

- [ ] **Step 1: Move.**
  - Move the method, doc comment included, byte-identical, into `session.rs`'s `impl Engine` (171), after `session_state_at`.
  - `session.rs` gains `use std::time::UNIX_EPOCH;` and `crate::store::SeedLaunch`.
  - `engine.rs` drops the imports it no longer uses (`fs`, `io`, `UNIX_EPOCH`, `ProfileMarker`, `SeedLaunch`; clippy confirms).
- [ ] **Step 2: Verify.** Run the engine focused command (`tests/store.rs` covers seeding) and clippy.
- [ ] **Step 3: Commit** "Move seed_launches next to the session state it reads".

### Task 7: Take every mutation guard through one `GuardRequest`

**Depends on:** Task 6 (`engine.rs`).

**Files:**
- Modify:
  - `crates/tagteam-engine/src/engine.rs` (the guard functions, 334-620, and the unit tests at 872-900);
  - `switch.rs:1669` and `:1704`;
  - `import.rs:119`;
  - `launch.rs:125` and `:133`;
  - `lifecycle.rs:844`, `:982` and `:1093`;
  - `auto.rs:727`;
  - the comments that name a removed function: `switch.rs:1480`, `collect.rs:932`, `recover.rs:85`, and `engine.rs` around the guards.
- Test: `crates/tagteam-engine/src/engine.rs` (tests module), or `crates/tagteam-engine/tests/recover.rs`

**Interfaces:**
- Produces, in `engine.rs`:

```rust
pub(crate) enum RecoveryMode { Oracle, FingerprintsOnly, DecodableOnly }
pub(crate) type Blocked = Vec<(ProviderId, EngineError)>;
pub(crate) struct GuardRequest<'a> {
    pub(crate) providers: &'a [ProviderId],
    pub(crate) timeout: Duration,
    pub(crate) recovery: RecoveryMode,
    pub(crate) refuse_interrupted: bool,
    pub(crate) source: &'static str, // until R4's §8.7
}
impl Default for GuardRequest<'_> // no providers, MutationGuard::TIMEOUT, Oracle, false, "cli"
impl<'a> GuardRequest<'a> {
    pub(crate) fn refusing(providers: &'a [ProviderId]) -> Self;   // refuse_interrupted: true, rest default
    pub(crate) fn refusing_one(provider: &'a ProviderId) -> Self;  // refusing(slice::from_ref(provider))
}
impl Engine {
    pub(crate) fn guard(&self, r: &GuardRequest) -> Result<(MutationGuard, Blocked), EngineError>;
    pub(crate) fn guard_or_refuse(&self, provider: &ProviderId) -> Result<MutationGuard, EngineError>;
    pub(crate) fn settle_or_refuse(&self, r: &GuardRequest) -> Result<(), EngineError>;
    pub fn mutation_guard(&self) -> Result<MutationGuard, EngineError>;
    pub(crate) fn metadata_guard(&self) -> Result<MutationGuard, EngineError>;
    pub(crate) fn purge_guard(&self) -> Result<MutationGuard, EngineError>;
    pub(crate) fn acquire_guard(&self, timeout: Duration) -> Result<MutationGuard, EngineError>; // unchanged
}
```

- **Removed:** `guard_or_refuse_as`, `_within`, `_for`, `_each`, `_each_within`, `settle_or_refuse_as`, `guard_recovering` and `guard_recovering_from`.

Facts: `.superpowers/research/r2-facts/guard-args-placement.md` §1 and §2.

- [ ] **Step 1: Write the tests** (Review Focus 2 and 4), each unless an existing test already pins it; the report names that test.
  - `a_metadata_guard_recovers_without_asking_the_oracle`:
    - Setup: a dead journal row whose direction needs no oracle, and `ScriptedHttp` with nothing scripted.
    - Assert: `metadata_guard()` succeeds, and the HTTP fake saw no request.
  - `a_purge_guard_steps_over_an_undecodable_journal_row`:
    - Setup: a journal row the store cannot decode.
    - Assert: `purge_guard()` succeeds, and `mutation_guard()` returns an error.
  - `a_refusing_guard_returns_the_blocked_error_of_the_provider_it_names`:
    - Setup: an interrupted switch whose recovery is blocked, for provider A.
    - Assert: guarding A through `guard_or_refuse` returns `EngineError::RecoveryBlocked`, or `RecoveryMoved` as the row dictates, not `InterruptedSwitch`.
  - Run them. Expected: PASS on today's code, as regression pins. Confirm each can fail by temporarily breaking the line it pins, then revert.
- [ ] **Step 2: Implement `guard`.** It is `guard_recovering_from`'s body followed by `guard_or_refuse_each_within`'s check, with their order and comments kept (`engine.rs:494-560` and `:394-421`). The journal reader is `Self::dead_decodable_journals` for `DecodableOnly`, else `Self::dead_journals`. The sequence:
  1. **Hints, before any lock.** Read the journal rows. For every mode but `FingerprintsOnly`, ask the oracle for each row's hints (`self.recovery_hints(&row)?`); `FingerprintsOnly` gives each row no hints. No network call is made while the guard is held.
  2. **The hook.** `hooks::point(self, "before-mutation-lock")?`.
  3. **The lock.** `self.acquire_guard(r.timeout)?`.
  4. **Recovery, under the lock.** Read the journal rows again, because a switch may have died meanwhile. Recover each row with the hints step 1 found for an equal row, or none. Map `EntryMoved` to `RecoveryMoved`. Return at once on an error that carries a signal. Log the rest, and collect `RecoveryBlocked | RecoveryMoved` into `Blocked`. All of this is exactly as today.
  5. **The refusal.** If `r.refuse_interrupted`, check each of `r.providers` in order: `self.interrupted(p)?`. If it is interrupted, return the blocked error for `p` (`swap_remove`), else `EngineError::InterruptedSwitch`.
  6. **The result.** Return `(guard, blocked)`.
- [ ] **Step 3: The wrappers.**
  - `guard_or_refuse(provider)` is `self.guard(&GuardRequest::refusing_one(provider)).map(|(g, _)| g)`.
  - `settle_or_refuse(r)` keeps today's conditional. It checks `r.providers` in order with `self.interrupted(p)?`. On the first one that is interrupted, it takes `self.guard(r)?` once, drops it, and returns. With no store, nothing is interrupted and no lock is taken.
  - `mutation_guard` is `guard(&GuardRequest::default())`. `metadata_guard` uses `recovery: FingerprintsOnly`. `purge_guard` uses `recovery: DecodableOnly`. Each drops `Blocked`.
- [ ] **Step 4: The call sites.**
  - `switch.rs:1669` (`settle_or_refuse_as(&req.provider, req.source)`) becomes `self.settle_or_refuse(&GuardRequest { source: req.source, ..GuardRequest::refusing_one(&req.provider) })`. `auto.rs:727` changes the same way, with `self.provider` and `self.source`.
  - `switch.rs:1704` (`guard_or_refuse_as`) becomes `self.guard(&GuardRequest { source: req.source, ..GuardRequest::refusing_one(&req.provider) })?.0`.
  - `launch.rs:133` (`guard_or_refuse_within`) becomes `self.guard(&GuardRequest { timeout: MutationGuard::BOOTSTRAP_TIMEOUT, ..GuardRequest::refusing_one(&account.provider) })?.0`.
  - `import.rs:119` (`guard_or_refuse_each`) becomes `self.guard(&GuardRequest::refusing(&providers))?.0`.
  - `settle_or_refuse(&x)` at `launch.rs:125` and `lifecycle.rs:844`, `:982` and `:1093` becomes `settle_or_refuse(&GuardRequest::refusing_one(&x))`. The `engine.rs` unit tests at 878 and 892 change the same way.
  - Comments that name a removed function name `guard` and the request instead.
- [ ] **Step 5: Verify.**
  - Run the engine focused command, the daemon focused command, and clippy (both variants).
  - Run `rg -n 'guard_or_refuse_(as|within|for|each)|settle_or_refuse_as|guard_recovering' crates`. Expected: no hits.
- [ ] **Step 6: Commit** "Take every mutation guard through one GuardRequest".

### Task 8: Pass the profile bootstrap its context as one `ProfileCtx`

**Depends on:** Task 7 (`launch.rs`).

**Files:**
- Modify: `crates/tagteam-engine/src/bootstrap.rs` (283, 492, and the unit test at 535), `crates/tagteam-engine/src/launch.rs` (195, 407-447)

**Interfaces:**
- Produces, in `bootstrap.rs`:

```rust
pub(crate) struct ProfileCtx<'a> {
    pub(crate) row: &'a AccountRow,
    pub(crate) profile: &'a Path,
    pub(crate) cwd: &'a Path,
    pub(crate) program: &'a Path,
    pub(crate) guard: &'a MutationGuard,
    pub(crate) lock: &'a AccountLock,
}
pub(crate) fn bootstrap_profile(&self, p: &dyn Provider, cx: &ProfileCtx<'_>) -> Result<Option<String>, EngineError>;
// launch.rs
fn prepare_quiescent(&self, p: &dyn Provider, cx: &ProfileCtx<'_>) -> Result<(bool, Option<String>), EngineError>;
```

- [ ] **Step 1: Change both signatures.**
  - Drop both `#[allow(clippy::too_many_arguments)]`.
  - Inside the bodies, `row` becomes `cx.row`, and so on for each field.
  - Each call site builds the `ProfileCtx` inline from the same values it passes today.
- [ ] **Step 2: Verify.** Run the engine focused command (launch and bootstrap tests) and clippy (both variants).
- [ ] **Step 3: Commit** "Pass the profile bootstrap its context as one ProfileCtx".

### Task 9: Pass add's preparation and login write their inputs as `AddRequest` and `LoginWrite`

**Depends on:** Tasks 5 and 7 (`lifecycle.rs`).

**Files:**
- Modify: `crates/tagteam-engine/src/lifecycle.rs` (663-830, 903, 947, 1015, 1054), `crates/tagteam-engine/src/import.rs` (259, 332)

**Interfaces:**
- Produces, in `lifecycle.rs`:

```rust
struct AddRequest<'a> {
    identity: &'a Identity,
    claimed_uuid: Option<&'a str>,
    position: Option<u32>,
    alias: Option<&'a str>,
    yes: bool,
}
pub(crate) struct LoginWrite<'a> {
    pub(crate) identity: &'a Identity,
    pub(crate) kind: &'a str,
    pub(crate) secret: &'a [u8],
    pub(crate) position: Option<u32>,
    pub(crate) alias: Option<&'a str>,
    pub(crate) source: LoginSource,
}
fn prepare(&self, store: &Store, p: &dyn Provider, provider: &ProviderId, req: &AddRequest<'_>) -> Result<Prepared, EngineError>;
pub(crate) fn commit_login(&self, store: &Store, p: &dyn Provider, provider: &ProviderId, prep: &Prepared, locks: &[AccountLock], w: &LoginWrite<'_>) -> Result<(AccountRow, bool), EngineError>;
```

- [ ] **Step 1: Change both signatures.**
  - Drop both allows. Bodies read through `req.` and `w.`.
  - Each of the six call sites builds its struct from the values it passes today.
  - Do not touch the `self.vault.store(…)` lines inside `commit_login`, because hardening rewrites them.
- [ ] **Step 2: Verify.** Run the engine focused command (add, import and lifecycle tests) and clippy (both variants).
- [ ] **Step 3: Commit** "Pass add's preparation and login write their inputs as AddRequest and LoginWrite".

### Task 10: Pin the switch's log target on the events its split will move

**Depends on:** Tasks 1 and 7 (`switch.rs`).

**Files:**
- Modify: `crates/tagteam-engine/src/switch.rs` (the `tracing` calls at 632, 643, 691, 695, 1541, 1554 and 2324)
- Test: `crates/tagteam-engine/tests/common/mod.rs`, `crates/tagteam-engine/tests/log_lines.rs`

**Interfaces:**
- Produces: in `tests/common/mod.rs`, `pub fn capture_logs_with_targets<T>(f: impl FnOnce() -> T) -> (T, Vec<String>)`. It is `capture_logs` with `.with_target(true)`.

- [ ] **Step 1: Write the test** `a_rolled_back_switch_logs_under_the_switch_target` (Review Focus 1).
  - Drive the rolled-back switch of `a_rolled_back_switch_names_its_accounts_and_its_cause_s_kind` (`log_lines.rs:426`) through `capture_logs_with_targets`.
  - Assert that its WARN rollback line contains ` tagteam_engine::switch: ` exactly, so `…::switch::rollback:` fails.
  - Run it. Expected: PASS today.
- [ ] **Step 2: Pin the targets.**
  - Add `target: "tagteam_engine::switch",` as the first argument of each of the seven `tracing` calls listed in Files. Those are the calls Task 11 moves out of `mod.rs`: four go to `rollback.rs`, two to `freshen.rs` and one to `transact.rs`.
  - Calls that stay in `mod.rs` (1790, 1797, 1837) are untouched.
- [ ] **Step 3: Verify.**
  - Run the engine focused command, and `cargo test -p tagteam --locked --features test-support --test logging`. Expected: PASS.
- [ ] **Step 4: Commit** "Pin the switch's log target on the events its split will move".

### Task 11: Split the switch into selection, planning, freshening, rollback and transaction modules

**Depends on:** Task 10.

**Files:**
- Modify: `crates/tagteam-engine/src/switch.rs`, which becomes `crates/tagteam-engine/src/switch/mod.rs`
- Create, all under `crates/tagteam-engine/src/switch/`: `support.rs`, `select.rs`, `plan.rs`, `freshen.rs`, `rollback.rs` and `transact.rs`
- Create, untracked: `<workspace>/split_switch.py`, the generator

**Interfaces:**
- Produces:
  - In `switch/mod.rs`:
    - `pub(crate) use freshen::AutoFreshened;`
    - `pub(crate) use support::{Axis, Held, OracleHint, answer_for, cannot_refresh, needs_relogin, refuse_unreadable, refuse_unsafe_live_reads, works_until_expiry};`
  - Every `crate::switch::…` and `tagteam_engine::switch::…` path resolves unchanged.

Facts: `.superpowers/research/r2-facts/switch.md` §1–§4 and §8.

- [ ] **Step 1: Rename the file.**
  - Run `git mv crates/tagteam-engine/src/switch.rs crates/tagteam-engine/src/switch/mod.rs`, then build.
  - Commit "Move switch.rs to switch/mod.rs".
- [ ] **Step 2: Write the generator.**
  - **Input.** `split_switch.py` reads `switch/mod.rs` and writes the seven files.
  - **Finding items.** It finds each item by its declaration (for example `fn transact(` or `struct Plan`), never by line number. It takes the item's doc comments and attributes with it, and ends the item at its matching brace.
  - **Output.**
    - It writes each file's header: the `use` lines (fact sheet §2), plus the `mod` lines and re-exports in `mod.rs`.
    - It applies the visibility raises in the list below.
    - It leaves `mod.rs`'s test module last.
  - **Why.** Spec §3.2: hardening edits `switch.rs`, so after the rebase this commit is regenerated by rerunning the script on the rebased `mod.rs`, not merged by hand. Keep the script in the plan workspace until R2 is rebased.
- [ ] **Step 3: Run it.** Destinations:

  | File | Items |
  |---|---|
  | `mod.rs` | request and outcome types with their impls (`UsageStrategy`, `SwitchTarget`, `SwitchRequest`, `AutoPerform`, `SwitchReason`, `SwitchOutcome`); `ATTEMPTS` and the message consts; `unmanaged_message`, `login_email`, `strategy_of`, `noop`, `activation_hint`, `already_active`; `Engine::{without_store, switch, switch_planned, reverify_identity, replan_polls}`; the test module, last |
  | `support.rs` | `empty_live_read`, `Axis` (+impl), `refuse_unsafe_live_reads`, `same_generation`, `Held` (+impl), `refuse_unreadable`, `unless_forced`, `attributed`, `OracleHint`, `answer_for`, `needs_relogin`, `cannot_refresh`, `works_until_expiry`, `session_owned`, `profile_conflict`, `Engine::{vault_holds_login, has_login, matches_vault, read_live_identity, live_row}` |
  | `select.rs` | `is_candidate`, `Rotation`, `Ranked`, `Rated` (+impl), `binding_text`, `nothing_to_activate`, `Engine::{switch_candidate, candidate_order, walk, rotation, rotation_pick_stands, usage_pick, best_pick, next_available_pick, exhausted_message, oracle_hint, auto_refusal}` |
  | `plan.rs` | `Ask`, `Planned`, `Plan`, `Engine::plan` |
  | `freshen.rs` | `FRESHEN_WINDOW_MS`, `Freshened`, `AutoFreshened`, `Engine::{freshen_plan, due, freshen, freshen_auto, prepare_usage}` |
  | `rollback.rs` | `Rollback`, `impl Rollback`, `impl Drop for Rollback` |
  | `transact.rs` | `login_of`, `Locked`, `Rederived`, `Engine::{rederive, read_target, settle_profile, transact, settle_outgoing, displace_live, hold_vault, save_unheld, apply}` |

  Visibility raised to `pub(super)`, and nothing else:
  - `support.rs`: `same_generation`, `unless_forced`, `attributed`, `session_owned`, `profile_conflict`, `Engine::{vault_holds_login, has_login, matches_vault, live_row}`.
  - `select.rs`: `Rotation`, `Ranked`, `Engine::{rotation, rotation_pick_stands, usage_pick, oracle_hint, auto_refusal}`.
  - `plan.rs`: `Ask`, `Planned`, `Plan` and its eight fields, `Engine::plan`.
  - `freshen.rs`: `Engine::{freshen_plan, prepare_usage}`.
  - `rollback.rs`: `Rollback` and its nine fields, `Rollback::{write, disarm, fail}`.
  - `transact.rs`: `Locked`, `Rederived`, `Engine::{rederive, read_target, settle_profile, transact}`.

  Then run `cargo fmt --all`.
- [ ] **Step 4: Verify.**
  - Run the engine focused command; every switch, rollback, freshen, strategy and invariant test must pass unchanged.
  - Run `--test logging` for `tagteam`, and clippy (both variants).
  - Run `git diff --color-moved=zebra HEAD~1 --stat` and check that the only non-moved lines are headers and `pub(super)`.
- [ ] **Step 5: Commit** "Split the switch into selection, planning, freshening, rollback and transaction modules".

### Task 12: Thread the switch's settle state through one `Settle`, and recovery's through `Recovery`

**Depends on:** Task 11.

**Files:**
- Modify:
  - `crates/tagteam-engine/src/switch/support.rs` (`Settle` and its tests);
  - `crates/tagteam-engine/src/switch/transact.rs`;
  - `crates/tagteam-engine/src/switch/mod.rs` (re-export `Settle`);
  - `crates/tagteam-engine/src/recover.rs` (239-320, 362-400);
  - `crates/tagteam-engine/src/active.rs` (621-700).

**Interfaces:**
- Produces, in `switch/support.rs`:

```rust
pub(crate) struct Settle<'a> {
    pub(crate) p: &'a dyn Provider,
    pub(crate) provider: &'a ProviderId,
    pub(crate) held: Held,
    pub(crate) force: bool,
    pub(crate) live_identity: Option<&'a Identity>,
    pub(crate) own_secret: Option<&'a [u8]>,
    pub(crate) warnings: Vec<String>,
}
impl<'a> Settle<'a> {
    /// `force` false, no live identity or own secret, nothing held.
    pub(crate) fn new(p: &'a dyn Provider, provider: &'a ProviderId, warnings: Vec<String>) -> Self;
    pub(crate) fn hold_vault(&mut self, engine: &Engine, id: &AccountId);
    pub(crate) fn save_unheld(&mut self, engine: &Engine, bytes: &[u8]) -> Result<(), EngineError>;
    pub(crate) fn displace_live(&mut self, engine: &Engine, bytes: &[u8], reason: &str, identity: Option<&Identity>) -> Result<(), EngineError>;
    pub(crate) fn before_fallback(&mut self, engine: &Engine, bytes: &[u8]) -> Result<(), ProviderError>;
}
fn fallback_save_error(e: EngineError) -> ProviderError; // ProviderError::Invalid(format!("could not save a credential the Keychain fallback would delete: {e}"))
```

- In `recover.rs`:

```rust
struct Recovery<'a, 'g> {
    row: &'a JournalRow,
    accounts: &'a [AccountLock],
    locks: &'a LiveLocks<'g>,
    live: &'a LiveAuth,
    hints: &'a [OracleHint],
}
fn finish_forward(&self, p: &dyn Provider, store: &Store, rec: &Recovery<'_, '_>, established: &str, source: &'static str) -> Result<Option<Identity>, EngineError>;
fn capture_rotated_outgoing(&self, cx: &mut Settle<'_>, store: &Store, rec: &Recovery<'_, '_>, from: &AccountRow, bytes: &[u8]) -> Result<bool, EngineError>;
```

- **Removed:** `Engine::{save_unheld, displace_live, hold_vault}`, which become `Settle`'s methods, and the allows on `save_unheld`, `finish_forward` and `capture_rotated_outgoing`.

Facts: `.superpowers/research/r2-facts/switch.md` §5, and `guard-args-placement.md` §4.1.

- [ ] **Step 1: Write the test** `a_failed_fallback_save_names_the_credential_it_protects` (Review Focus 3).
  - Place it in a `#[cfg(test)] mod tests` at the end of `support.rs`.
  - Assert that `fallback_save_error` of an engine error displays exactly `"could not save a credential the Keychain fallback would delete: "` followed by that error's own text.
  - Run it. Expected: FAIL, because `fallback_save_error` does not exist yet.
- [ ] **Step 2: Add `Settle` and move the bodies.**
  - Move the bodies of `save_unheld`, `displace_live` and `hold_vault` into `Settle`'s methods. Their logic is unchanged; where they read the engine, they go through `engine`.
  - `displace_live` keeps `reason` and `identity` as explicit parameters, because its callers pass an identity already passed through `attributed` (fact sheet §5 item 1).
  - `save_unheld` calls `self.displace_live(engine, bytes, "displaced-live-login", attributed(self.live_identity, self.own_secret, bytes))`.
  - `before_fallback` is `self.save_unheld(engine, bytes).map_err(fallback_save_error)`.
- [ ] **Step 3: Callers.**
  - **`transact`** builds `let mut cx = Settle { force: req.force, live_identity: live_identity.as_ref(), own_secret: own_secret.as_deref(), ..Settle::new(p, provider, warnings) };` where it builds `held` and `warnings` today (`:2048-2060`). It uses `cx` in both branches, the hold loop, and the fallback closure `let mut before_fallback = |b: &[u8]| cx.before_fallback(self, b);`. The outcome takes `cx.warnings`.
  - **`settle_outgoing`** temporarily takes `cx: &mut Settle<'_>` in place of `p`, `live_identity` and `warnings`. Its allow stays until Task 13.
  - **`finish_forward`** takes `Recovery`, and builds `Settle::new(p, &rec.row.provider, Vec::new())` where it builds `held` and `warnings` today. After the loop it logs `cx.warnings` as it does today.
  - **`active.rs` `publish`** builds `Settle::new(p, &row.provider, Vec::new())` once. The loop finishes before the fallback closure borrows `cx`.
  - **`recover_one`** (`recover.rs:165`) builds the `Recovery` inline.
- [ ] **Step 4: Verify.**
  - Run the test from Step 1. Expected: PASS.
  - Run the engine focused command (`switch`, `switch_rollback`, `recover`, `storage_write`, `active`, `displaced`, `log_lines`) and clippy (both variants).
- [ ] **Step 5: Commit** "Thread the switch's settle state through one Settle and recovery's through Recovery".

### Task 13: Run the switch transaction as named steps over `LockedSwitch` and `ApplyStep`

**Depends on:** Task 12.

**Files:**
- Modify: `crates/tagteam-engine/src/switch/transact.rs`, `crates/tagteam-engine/src/switch/mod.rs` (the one `transact` call in `switch_planned`)

**Interfaces:**
- Produces, in `transact.rs`:

```rust
pub(super) struct LockedSwitch<'a, 'g> {
    pub(super) store: &'a Store,
    pub(super) account_locks: &'a [AccountLock],
    pub(super) locks: &'a LiveLocks<'g>,
}
pub(super) struct ApplyStep<'a> {
    pub(super) target: &'a AccountRow,
    pub(super) login: &'a StoredLogin,
    pub(super) outgoing: Option<&'a AccountRow>,
}
pub(super) fn transact(&self, p: &dyn Provider, plan: &Plan, locked: Locked, ls: &LockedSwitch<'_, '_>, req: &SwitchRequest) -> Result<SwitchOutcome, EngineError>;
fn settle_outgoing(&self, cx: &mut Settle<'_>, ls: &LockedSwitch<'_, '_>, out: &AccountRow, live: &LiveAuth, hint: Option<&OracleHint>, source: &str) -> Result<(), EngineError>;
fn apply<'a>(&self, p: &dyn Provider, step: ApplyStep<'_>, req: &SwitchRequest, tx: &mut Rollback<'a, '_>, before_fallback: BeforeFallback<'_>) -> Result<SecretStore, EngineError>;
```

- **Removed:** the allows on `transact`, `settle_outgoing` and `apply`.

Facts: `.superpowers/research/r2-facts/switch.md` §6.

- [ ] **Step 1: The steps.** `transact`'s body becomes a sequence of private methods, called in this order:

  | # | Method | Body today | Returns |
  |---|---|---|---|
  | 1 | `preflight` | `:2018-2030` | a `Preflight` struct: `target_identity`, `live`, `managed_key_replaced`, `doomed`, `live_identity`, `target`, `outgoing`, `locked_warnings` |
  | 2 | `settle_pre` | `:2032-2046` | — |
  | 3 | `settle_branch` | `:2061-2109` | the target secret |
  | 4 | `hold_doomed` | `:2110-2129` | — |
  | 5 | `journal_row` | `:2131-2162` | `prior`, after the row is inserted |
  | 6 | the `Rollback`, then `apply` | `:2164-2212` | `stored_in` |
  | 7 | `outcome` | `:2214-2245` | the `SwitchOutcome` |

  - `transact` itself builds `Settle` between steps 2 and 3.
  - `transact` builds the `Rollback` in the statement right after `journal_row` returns, so no `?` and no fallible call sits between `insert_journal` and the armed `Rollback`.
  - Each step takes at most seven parameters, `self` included.
- [ ] **Step 2: The order rules.**
  - The fallible calls keep today's order: `parse_identity` before `refuse_unsafe_live_reads`, and `settle_rescues` before `settle_profile` before either branch reads the vault.
  - `target` and `outgoing` are only borrowed until `outcome` takes them by value.
- [ ] **Step 3: The other two signatures.**
  - `settle_outgoing` takes `ls: &LockedSwitch` in place of `store` and `account_locks`.
  - `apply` takes an `ApplyStep`.
  - `switch_planned` builds the `LockedSwitch` from the `store`, `account_locks` and `locks` it passes today.
- [ ] **Step 4: Verify.**
  - Run the engine focused command (`switch_rollback`'s hook points `after-journal`, `after-credential` and `after-identity` are the transaction's check), `--test logging` for `tagteam`, and clippy (both variants).
  - Run `rg -n 'too_many_arguments' crates/tagteam-engine/src`. Expected: exactly one hit, `store/tokens.rs:547`.
- [ ] **Step 5: Commit** "Run the switch transaction as named steps over LockedSwitch and ApplyStep".

### Task 14: Rewrite the switch's "Decision N" references as the rules they stood for

**Depends on:** Task 13.

**Files:**
- Modify: the files in `crates/tagteam-engine/src/switch/` that hold the eight references (old `switch.rs:362`, `:418`, `:1623`, `:1624`, `:1937`, `:1983`, `:2052` and `:2407`)

- [ ] **Step 1: Rewrite.**
  - For each reference, find the plan that numbers it (`rg -n "Decision <N>" docs/superpowers/plans`).
  - Rewrite the comment as the rule in words, keeping any spec section reference it already carries (for example "§6.3").
  - Comments only; no code changes.
- [ ] **Step 2: Verify.**
  - Run `rg -n 'Decision [0-9]' crates/tagteam-engine/src/switch crates/tagteam-engine/src/doctor`. Expected: no hits.
  - Run `cargo fmt --all --check`.
- [ ] **Step 3: Commit** "Rewrite the switch's Decision references as the rules they stood for".

## Final verification (the controller, before the PR)

Spec §3.1 asks for tests on macOS and on Linux before R2 is reported done.

- **macOS, outside the sandbox:** `cargo fmt --all --check`, both clippy commands, and `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`. Report the totals as passed/total.
- **Linux:** the same `cargo test` command, in `CONTRIBUTING.md`'s Docker reproduction (`rust:1.88`, run as a non-root user with a passwd entry, `HOME` outside `/tmp`), against the branch head.
- **The allow count:** `rg -c 'too_many_arguments' crates/tagteam-engine/src` reports only `store/tokens.rs`.

## Spec deltas found while planning

These are rulings inside the spec's intent. Each one is recorded under "Rulings" in the PR.

1. **The guard's enum is `RecoveryMode`, not `Recovery`.** §6.2 also names a `Recovery` struct for `recover.rs`. One name for two types would make both harder to read.
2. **`guard_or_refuse` stays a thin wrapper**, a sixth beside §6.4's list, because it has five callers with today's exact combination.
   - `settle_or_refuse` takes a `GuardRequest`, which absorbs `settle_or_refuse_as`.
   - The call sites that change number 10 in 7 files, not "about 20 in 9".
   - `Blocked` is a type alias for today's `Vec<(ProviderId, EngineError)>`.
3. **`LockedSwitch`'s third field is `locks`, not `live`.** It holds the `LiveLocks`, and `live` is the `LiveAuth` that `transact` reads under them.
4. **`settle_outgoing` takes `LockedSwitch` as well as `Settle`.** With `Settle` alone it keeps eight parameters, and clippy still fires.
5. **`hold_vault` becomes a `Settle` method too.** Its three callers pass `p` and `held`, which `Settle` carries.
6. **`displace_live` keeps `reason` and `identity`.** Its callers pass an identity already passed through `attributed`; folding that in would attribute it twice.
7. **The fallback closure keeps its error mapping,** in `Settle::before_fallback`. §6.1's one-liner `|b| cx.save_unheld(self, b)` would drop the `map_err`.
8. **`Ask` and `Planned` go to `plan.rs`, not `select.rs`.** Their users are `plan.rs`, `freshen.rs` and `mod.rs`, so `select.rs` depends on `plan.rs` only for `Plan`. `FRESHEN_WINDOW_MS` goes to `freshen.rs`, its one user.
9. **Items the table does not name** go where fact sheets list them:
   - `activation_hint` goes to `mod.rs`, with its test;
   - `needs_relogin`, `cannot_refresh` and `works_until_expiry` go to `support.rs`.
10. **Log targets are pinned before the split (Task 10).** §3.1 names hook points and paths; log targets are output, so §1.4 keeps them.
11. **`switch.rs` has a unit-test module** (spec: none). It stays last in `mod.rs`.
12. **Doctor's test 2 splits into two tests before the move,** because its two halves land in different files.
13. **Check IDs are consts on a local `Id` type.** `Check` belongs to `tagteam-provider`, and an inherent impl on it is impossible here.
14. **`store.skipped` has two texts, not three:** the same one twice, and a different one once. `store_skipped(why)` covers both.
15. **`fsutil.rs` also carries `MAX_LINK_HOPS`, `names` and lifecycle's whole test module,** which `trace_path` needs. Its current range is `lifecycle.rs:190-264`.
16. **`FileId` is a struct `{ dev, ino }` with `FileId::of`,** not an alias, so it cannot be mixed with another `(u64, u64)`.
17. **`Recovery`'s fields are those of §6.2.** `store`, `established` and `source` stay parameters of `finish_forward`.
18. **The doctor's sizes differ from the spec's estimates.** `doctor/mod.rs` is about 400 lines, not 720; `accounts.rs` and `profiles.rs` run about 100 over. That comes from the items §6.3 does not list.
