# tagteam refactor R5b — each provider owns its record format — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Implemented — https://github.com/michaelrigart/tagteam/pull/46
**Goal:** FakeAgent gets a session record format and supervisor file of its own. Claude Code's record format moves out of `tagteam-provider` into `tagteam-cc` in one pure move. The neutrality test then keeps Claude Code's field names out of the neutral crates. This completes spec §9, and with it the review refactor.
**Architecture:**
- **FakeAgent first.** Its records become `procs/<pid>.rec` with its own JSON, and its keeper writes `procs/keeper.lock`. Engine tests drive it through the same neutral lister, lock reader, gate and token-report readers as Claude Code. Once it stops delegating to Claude Code's parser, nothing outside regions A and B uses them.
- **Then the move.** R5a left Claude Code's record code in two regions of `liveness.rs`. They move byte-identical into `crates/tagteam-cc/src/records.rs`, with their unit tests, and every caller follows by `use` line.
- **Then the guard.** `neutrality.rs` adds `procStart`, `oauthAccount`, `accountUuid`, `claudeAiOauth` and `daemon-worker`, matched in their exact case.

**Tech Stack:** Rust 1.88 (edition 2024), cargo workspace; serde_json (`preserve_order`, `arbitrary_precision`).

**Spec:** docs/superpowers/specs/2026-10-19-tagteam-review-refactor-design.md (§9 and §5.9, with §3, §4, §12 and §16)

**Branch:** `refactor-r5b-record-formats`, based on `refactor-r5a-neutral-records` (`docs/superpowers/plans/2026-10-21-tagteam-refactor-r5a-neutral-records.md`).

**Reading this plan.**
- **Names.** R5a's names are used as R5a defines them: `ProcStart`, the neutral `SessionRecord`, `CcRecord`, `parse_cc_record`, `proc_start_of`, `read_session_records(dir, ext, parse, clock)`, `read_supervisor_lock(path, parse)`, `session_record_path`, and the trait methods `parse_session_record` and `session_record_ext`. Regions A and B are as R5a's Global Constraints lay them out.
- **Sites are named by function and quoted code.**
- **Focused test commands:**
  - `cargo test -p tagteam-fake --locked`;
  - `cargo test -p tagteam-cc --locked`;
  - `cargo test -p tagteam-provider --locked --lib liveness`;
  - `cargo test -p tagteam-engine --locked --features test-hooks --test fake_sessions --test session --test tokens_evidence --test tokens_view --test session_views --test doctor --test purge --test switch`;
  - `cargo test -p xtask --locked`.
- **Each task** leaves the workspace compiling, with its crates' tests green.

## Global Constraints

- **No user-visible change.** Every message, JSON field, exit code and log line stays byte-identical, and there is no `CHANGELOG.md` entry (§11 has no R5 row). Claude Code's format, its `.json` extension and its `daemon.lock` stay exactly as they are. FakeAgent is test-only.
- **No wire or store change.**
- **Hardening's code** (§3.2):
  - in `liveness.rs`, Task 2 only removes regions A and B and their test blocks, and drops the `use serde_json::Value;` they needed;
  - the probe region is not touched;
  - no other hardening-owned file is edited.
- **Held TUI files are not edited:** `crates/tagteam-tui/**`, `crates/tagteam/src/tui_host.rs` and `crates/tagteam/src/tui_cmd.rs`.
- **Task 2 is a move task.**
  - **Its commit may contain:**
    - the moved items, byte-identical;
    - the new file, with its header (a one-line `//!` doc and its `use` lines) and its `mod` line;
    - `use` lines and re-exports;
    - paths to moved items in expressions (`tagteam_provider::parse_lstart(…)` becomes `tagteam_cc::records::parse_lstart(…)`);
    - rustfmt's reflow of those lines.
  - **The reviewer** reads it with `git diff --color-moved=zebra --color-moved-ws=allow-indentation-change`.
- **Unit-test modules stay last** in each file, with `#[cfg(test)]` alone on the line directly above `mod tests {`.
- **CI commands** (from `CONTRIBUTING.md`), all of which must pass:
  - `cargo fmt --all --check`;
  - `cargo clippy --workspace --locked -- -D warnings`;
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`;
  - `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`.

  Every task runs the clippy variants and the touched crates' focused tests.
- **No agent attribution** in commits. **Commit messages** are imperative and in sentence case, with no prefix and no trailing period.
- **No test is weakened.** The only test edits allowed are:
  - FakeAgent's tests moving to its own format;
  - the moved tests and their imports;
  - new tests.
- **No new `#[allow(...)]`.** Comment lines wrap at 100 columns.

## Review Focus

These are the five failure modes the spec implies but no existing test exercises, most likely to bite first. Each one's test is added to the task that owns it.

1. **The keeper file next to the records.**
   - `procs/keeper.lock` sits in FakeAgent's records directory. If the lister ever listed it, every profile with a keeper would read as an unreadable record.
   - Test: Task 1, `a_live_keeper_file_owns_a_fake_agent_profile_whatever_its_role` asserts `damaged: vec![]` while the keeper is live.
2. **A keeper file whose record is not a keeper's.**
   - Ownership by the supervisor file must not depend on the record's `role`, just as Claude Code's `daemon.lock` has no `kind`.
   - Test: the same test plants `keeper.lock` with role `driver` and expects `session: false, daemon: true`.
3. **FakeAgent's records at every reader of the token report.**
   - The scan lists them, and `observe_launch` and the statusline each read one by pid. A `.json` left at either read by pid would drop FakeAgent's sightings without a word.
   - Test: Task 1, `fake_agent_records_reach_the_token_report_s_readers`.
4. **`ProcStart` is not `procStart`.**
   - The neutral type must not trip the field-name check, and the field must.
   - Test: Task 3, `a_field_name_leaks_only_in_its_exact_case`.
5. **A field name in a comment or a test module.**
   - The scanner's skips must apply to the new names as they do to `claude`.
   - Test: Task 3 extends `the_scanner_keeps_production_and_drops_test_halves_and_comments`.

## Spec coverage

| Spec | Tasks |
|---|---|
| §9, FakeAgent's own format: `procs/<pid>.rec`, its content, `procs/keeper.lock`, `tagteam_fake::session_record(..)` | 1 |
| §12, FakeAgent's own record format through `tests/fake_sessions.rs` | 1 |
| §9, cc parses its JSON, `lstart` and the legacy form, and maps kinds to `background` (now in `tagteam-cc`); `DAEMON_KINDS` in cc | 2 |
| §9, xtask's compat harness follows the moved imports | 2 |
| §5.9 and §9, the neutrality test adds Claude Code's field names | 3 |
| §16, the amendments to other specs | Final verification (checked) |
| §4, the spec moves to `Implemented`, citing every PR | Final records |

About 600 of §9's roughly 1,000 changed lines are Task 2's move: 341 lines that leave `liveness.rs` at `8d44998`, plus what R5a added to the two regions.

## Task order and batches

Tasks run one at a time, in order.
- Task 2 needs Task 1, because FakeAgent stops calling Claude Code's parser there.
- Task 3 needs Task 2, because before the move the provider still names `procStart`.

---

### Task 0: Confirm the base (no commit)

**Depends on:** none.

- [ ] **Step 1: Find the regions.**
  - Run `rg -n '^pub struct CcRecord|^pub fn parse_session_record|^const WEEKDAYS|^pub fn proc_start_of|^pub fn parse_lstart' crates/tagteam-provider/src/liveness.rs`. Each must appear once.
  - Region A runs from `CcRecord`'s doc to the end of `parse_session_record`. Region B runs from `WEEKDAYS` to the end of `proc_start_of`. Record both line ranges and the three Claude Code test blocks in the report.
  - If anything neutral sits inside a region, stop and report.
- [ ] **Step 2: Find the callers.**
  - Run `git grep -n -E 'CcRecord|parse_cc_record|proc_start_of|parse_lstart|parse_session_record\(' -- crates`.
  - Expected: liveness.rs, `tagteam-cc/src/provider.rs`, `tagteam-cc/tests/records.rs`, `tagteam-fake/src/provider.rs`, xtask's `compat/daemon.rs`, `checks/mod.rs` and `checks/profile.rs`, and the engine tests Task 2 lists, plus `p.parse_session_record` calls in the engine.

### Task 1: Give FakeAgent a session record format and keeper file of its own

**Depends on:** Task 0.

**Files:**
- Modify: `crates/tagteam-fake/src/shape.rs` (its record parser)
- Modify: `crates/tagteam-fake/src/provider.rs`:
  - `parse_session_record` and `session_record_ext` replace R5a's delegation, whose `use tagteam_provider::liveness::parse_session_record;` line goes;
  - a new `supervisor_lock`.
- Modify: `crates/tagteam-fake/src/lib.rs` (`session_record`)
- Test: `crates/tagteam-fake/tests/provider.rs`, `crates/tagteam-engine/tests/fake_sessions.rs`

**Interfaces:**
- Produces:

```rust
// tagteam-fake/src/lib.rs, beside credential_json and identity_json
/// A FakeAgent session record as a run writes it to `procs/<pid>.rec`:
/// `{"process":{"id":pid,"since":since_s},"session":session,"role":role}`. The role `keeper`
/// marks its background keeper.
pub fn session_record(pid: u32, since_s: i64, session: &str, role: &str) -> Value;
// tagteam-fake/src/shape.rs
pub(crate) fn parse_session_record(bytes: &[u8]) -> Result<SessionRecord, String>;
// tagteam-fake/src/provider.rs, impl Provider for FakeAgent
fn parse_session_record(&self, bytes: &[u8]) -> Result<SessionRecord, String>; // shape::parse_session_record
fn session_record_ext(&self) -> &'static str; // "rec"
fn supervisor_lock(&self, profile: &Path) -> Option<PathBuf>; // Some(session_records_dir(profile).join("keeper.lock"))
```

- **FakeAgent's parse rules:**
  - Bytes that are not JSON give `Err("it is not JSON")`, and a non-object gives `Err("it is not a JSON object")`.
  - `process.id` must be an integer in 1..=`i32::MAX`. Otherwise it is `Err("its process id is missing or out of range")`.
  - An integer `process.since` gives `start: EpochS(since)` and `started_at_ms: Some(since.saturating_mul(1000))`. Anything else gives `Unknown` and `None`.
  - A text `session` gives `session_id`.
  - `background` is `role == "keeper"`.
  - No detail quotes the bytes.
- **FakeAgent's `words()` stay as they are** (`supervisor: None`, `supervisor_stop: None`). The neutral fallback names its keeper "a background supervisor".

- [ ] **Step 1: Write the FakeAgent test** `its_session_records_are_its_own_format` (`tagteam-fake/tests/provider.rs`).
  - `session_record_ext()` is `"rec"`, and `supervisor_lock(Path::new("/p/0193"))` is `Some("/p/0193/procs/keeper.lock")`.
  - `parse_session_record` of `session_record(7, 1_790_858_096, "s-7", "keeper")` is `SessionRecord { pid: 7, start: ProcStart::EpochS(1_790_858_096), started_at_ms: Some(1_790_858_096_000), session_id: Some("s-7".into()), background: true }`. With the role `driver`, `background` is `false`.
  - `{"process":{"id":7}}` and `{"process":{"id":7,"since":"soon"}}` give `start: Unknown`, `started_at_ms: None`, `session_id: None` and `background: false`.
  - Each of these is `Err` with no `SENTINEL` in its detail:
    - Claude Code's shape `{"pid":7,"procStart":"SENTINEL"}`;
    - `{"process":{"id":0}}`, `{"process":{"id":-1}}`, `{"process":{"id":2147483648}}` and `{"process":{"id":"SENTINEL"}}`;
    - `["SENTINEL"]` and `SENTINEL{`.
- [ ] **Step 2: Move FakeAgent's engine tests onto its format** (`tests/fake_sessions.rs`).
  - **Imports.** It drops `LSTART`, `record_json` and `parse_lstart`, and imports `tagteam_fake::session_record` and `tagteam_provider::session_record_path`.
  - **The start.** `const STARTED_S: i64 = 1_790_858_096;` is FakeAgent's session start in its records, epoch seconds.
    - `plant_fake_record` writes `session_record(pid, STARTED_S, "s-1", "driver")` to `session_record_path(&dir, pid, ffx.fake.session_record_ext())`.
    - `start_session` sets `start_time_s: Some(STARTED_S)`, and the recycled pid `Some(STARTED_S + 3_600)`.
    - The docs of both name `since` instead of `procStart`.
  - **In `fake_agent_session_state_comes_from_its_own_records`:**
    - the malformed record is `procs/7.rec`;
    - after the Claude Code record check, `procs/4242.json` holding `{"pid": 4242, "procStart": "Thu Oct  1 12:34:56 2026"}` leaves the state `Quiescent` ("not FakeAgent's file name"), and is then removed.
  - **New: `a_live_keeper_file_owns_a_fake_agent_profile_whatever_its_role`** (Review Focus 1, 2):
    - alice's profile holds `procs/keeper.lock` = `session_record(4343, STARTED_S, "s-k", "driver")`;
    - with pid 4343 running at `STARTED_S`, the state is `Owned { profile, session: false, daemon: true, damaged: vec![] }`;
    - `ffx.engine.remove(&alice)` fails with kind `session-owned`, and its text contains `a background supervisor` and `delete '<profile>/procs/keeper.lock'`;
    - with pid 4343 gone, the state is `Quiescent`;
    - with `keeper.lock` = `{`, the state is `Unreadable`, with one `Damaged` of kind `DamagedKind::SupervisorLock` whose file is that path.
  - **New: `fake_agent_records_reach_the_token_report_s_readers`** (Review Focus 3), in this order:
    1. **The scan.** The default home's `procs/4242.rec` = `session_record(4242, STARTED_S, "s-f", "driver")`, with pid 4242 running. `ffx.engine.sight_sessions(&ffx.fake_provider())` returns `1`, and the store's FakeAgent sightings hold one row: session `s-f`, home `""`, `started_at` `STARTED_S * 1000`, source `Scan`. The default home's records directory is `ffx.fake.default_session_records_dir(&ffx.fx.env)`.
    2. **The statusline.** This process (`std::process::id()`) is set running with `ppid: Some(4242)`. `statusline_sighting(&fake, "s-line", None)` records a `Statusline` sighting of `s-line` with `started_at` `STARTED_S * 1000`.
    3. **run's poll.** alice's profile holds `procs/777.rec` = `session_record(777, STARTED_S, "s-run", "driver")`, with pid 777 running. `observe_launch(&LaunchId { provider: fake, account: alice, started_at_ms: STARTED_S * 1000 }, &profile, 777, &mut state)` leaves `state.tenure` naming `s-run` with `started_at` `STARTED_S * 1000`.
  - Run Steps 1–2's tests. Expected: FAIL, because FakeAgent still reads Claude Code's JSON at `.json`.
- [ ] **Step 3: Implement** as the Interfaces block gives it.
- [ ] **Step 4: Verify.**
  - Run the fake and engine focused commands and the clippy variants.
  - `git grep -n 'liveness::parse_session_record' -- crates/tagteam-fake`. Expected: no hits.
- [ ] **Step 5: Commit** "Give FakeAgent a session record format and keeper file of its own".

### Task 2: Move Claude Code's session record format into `tagteam-cc`

**Depends on:** Task 1.

**Files:**
- Create: `crates/tagteam-cc/src/records.rs`
- Modify: `crates/tagteam-cc/src/lib.rs` (`pub mod records;`, after `pub mod provider;`)
- Modify: `crates/tagteam-provider/src/liveness.rs` (regions A and B and their three test blocks leave; `use serde_json::Value;` goes)
- Modify: `crates/tagteam-provider/src/lib.rs` (the root re-export drops `parse_lstart` and `parse_session_record`)
- Modify, `use` lines and paths only:
  - `crates/tagteam-cc/src/provider.rs`: `use tagteam_provider::liveness::parse_session_record;` becomes `use crate::records::parse_session_record;`;
  - `crates/tagteam-cc/tests/records.rs`;
  - xtask's `crates/xtask/src/compat/daemon.rs`, `checks/mod.rs` and `checks/profile.rs` (its `tagteam_provider::parse_lstart` path too);
  - the engine tests `tests/common/mod.rs`, `session.rs`, `session_views.rs`, `tokens_evidence.rs` and `tokens_view.rs` (`use` lines), and `doctor.rs`, `purge.rs` and `switch.rs` (each `tagteam_provider::parse_lstart(` path).

**Interfaces:**
- Produces `tagteam_cc::records::{CcRecord, parse_cc_record, parse_session_record, parse_lstart, proc_start_of}`.
  - `DAEMON_KINDS`, `MAX_DEPTH`, `depth`, `pid_of` and the calendar helpers stay private.
  - `liveness.rs` keeps only neutral items: `ProcStart`, `SessionRecord`, `RecordEntry`, the lister, the lock reader, `session_record_path`, the probes and `record_is_live`.

- [ ] **Step 1: Move the production code.**
  - Region A, then region B, byte-identical, into `records.rs`.
  - The header is `//! Claude Code's session records (Appendix A.7): their format, and the neutral record each one becomes.`, then `use serde_json::Value;` and `use tagteam_provider::liveness::{ProcStart, SessionRecord};`.
- [ ] **Step 2: Move the unit tests.**
  - The three Claude Code test blocks go byte-identical into `records.rs`'s `#[cfg(test)] mod tests`.
  - That module's `use` lines are whatever compiles: `super::*`, `serde_json::json`, and `tagteam_provider::liveness::{ProcessProbe, SystemProcessProbe}` for the round trip.
- [ ] **Step 3: Follow the imports** in every file listed above.
  - In `tagteam-cc/tests/records.rs`, the moved items (`CcRecord`, `parse_cc_record`, `parse_session_record`, `proc_start_of`) come from `tagteam_cc::records`. The neutral ones stay `tagteam_provider`'s.
  - xtask takes `CcRecord`, `parse_cc_record`, `parse_session_record` and `parse_lstart` from `tagteam_cc::records`.
- [ ] **Step 4: Verify.**
  - Run the CI commands.
  - **Counts.** `cargo test -p tagteam-cc --locked --lib records -- --list` lists exactly the tests that `cargo test -p tagteam-provider --locked --lib liveness -- --list` lost. Compare against the list taken at the base.
  - **The diff.** `git diff --color-moved=zebra --color-moved-ws=allow-indentation-change HEAD` shows only moved lines, the new file's header, `mod` and `use` lines, the re-export line and the paths.
- [ ] **Step 5: Commit** "Move Claude Code's session record format into tagteam-cc".

### Task 3: Keep Claude Code's field names out of the neutral crates

**Depends on:** Task 2.

**Files:**
- Modify: `crates/tagteam-fake/tests/neutrality.rs`

**Interfaces:**
- Produces:

```rust
/// Claude Code's field names and record kinds, matched in their exact case: the neutral
/// `ProcStart` type is not the field `procStart`.
const FIELDS: [&str; 5] = ["procStart", "oauthAccount", "accountUuid", "claudeAiOauth", "daemon-worker"];
/// Whether a production line of a neutral crate names Claude Code: a `LEAKS` word in any case,
/// or a `FIELDS` name exactly.
fn names_claude_code(line: &str) -> bool;
```

- `no_neutral_crate_names_claude_code` uses `names_claude_code` in place of its `LEAKS` test. The binary's test keeps `LEAKS` alone, because §5.9 adds the names for the neutral crates. `ALLOWED` is unchanged.

- [ ] **Step 1: Write the tests.**
  - **`a_field_name_leaks_only_in_its_exact_case`** (Review Focus 4):
    - true for `o.get("procStart")`, `"daemon-worker"`, `v["oauthAccount"]`, `accountUuid`, `claudeAiOauth` and `Claude`;
    - false for `start: ProcStart,`, `proc_start`, `account_uuid` and `daemon_worker`.
  - **Extend `the_scanner_keeps_production_and_drops_test_halves_and_comments`** (Review Focus 5): `production("fn a() {}\n// procStart\n#[cfg(test)]\nmod tests {\n    \"oauthAccount\"\n}\n")` is `[(1, "fn a() {}")]`.
  - Run them. Expected: they fail to compile.
- [ ] **Step 2: Implement** `FIELDS` and `names_claude_code`.
- [ ] **Step 3: Run** `cargo test -p tagteam-fake --locked --test neutrality`.
  - Expected: PASS. At `8d44998` the only production hits were `liveness.rs:81` and the engine's `DAEMON_KINDS`, and R5 removed both.
  - Any hit in R5's files is fixed here.
  - A hit anywhere else (code R4 landed) is reported to the controller, not fixed here.
- [ ] **Step 4: The planted check, once, with no commit.**
  - Add `const _PLANTED: &str = "oauthAccount";` to the production half of `crates/tagteam-engine/src/session.rs`.
  - Run the neutrality test. Expected: FAIL, naming `crates/tagteam-engine/src/session.rs:<line>`.
  - Revert. `git status` shows only `neutrality.rs`.
- [ ] **Step 5: Commit** "Keep Claude Code's field names out of the neutral crates".

## Final verification and records (the controller, before the PR)

- **macOS, outside the sandbox:** `cargo fmt --all --check`, the three clippy commands, and `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`. Report the totals as passed/total.
- **Linux:** the same `cargo test` command in the Docker reproduction (`rust:1.88`, run as a non-root user with a passwd entry, `HOME` outside `/tmp`), against the branch head.
- **The held crates:**
  - `cargo build -p tagteam-tui --locked`;
  - `git diff --stat <R5a head>..HEAD -- crates/tagteam-tui crates/tagteam/src/tui_host.rs crates/tagteam/src/tui_cmd.rs` must be empty.
- **The boundary:**
  - `git grep -n -E 'CcRecord|parse_cc_record|parse_lstart|proc_start_of|DAEMON_KINDS' -- crates/tagteam-provider crates/tagteam-engine/src`. Expected: no hits.
  - `git grep -n procStart -- crates/tagteam-provider/src`. Expected: comment lines only, such as `ProcessProbe::start_ticks`'s doc in hardening's probe region.
- **§16.** Check each amendment, and report a missing one rather than write it here:
  - core §4.5 names `parse_session_record` and `session_record_ext` (R5a);
  - core §13.2 and §14 state D-3 and the kinds `deletion-refused` and `profile-marker-missing` (R4);
  - core §9.1 has its sentence if §8.11 item 4 held (R4).
- **Records, in the final pre-merge commit:**
  - this plan's `**Status:**` becomes `Implemented — <R5b PR link>`;
  - the refactor spec's `**Status:**` becomes `Implemented — `, followed by every PR as a link with its slice in parentheses, in delivery order:
    - `https://github.com/michaelrigart/tagteam/pull/22 (R1)`, `…/pull/24 (R2)`, `…/pull/25 (R3)`;
    - the R4a–R4g PRs;
    - the R5a and R5b PRs.

    The controller fills in the numbers. §10's held items do not block it (§4).

  Message: "Mark the R5b plan and the refactor spec implemented, citing every PR".

## Spec deltas found while planning

These are rulings inside the spec's intent. Each one is recorded under "Rulings" in the PR.

1. **FakeAgent's `started_at_ms` is `since × 1000`.** Its record has one start time. The role `keeper` marks `background`, and its `words()` stay `None`, so its keeper is named by the neutral fallback.
2. **The move commit may rewrite paths to moved items in expressions** (three engine tests and one xtask line). That is the same kind of edit as a `use` line.
3. **The neutrality test matches the field names in their exact case.** §5.9's case-insensitive match covers `claude` and `anthropic`. Applied to the field names, it would flag the spec's own `ProcStart`.
4. **The engine's tests keep using Claude Code's `parse_lstart` for Claude Code records**, now from `tagteam_cc::records`. FakeAgent's tests use a constant start.
5. **The new file's one-line module doc** counts as part of its header in the move commit.
