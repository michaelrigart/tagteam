# tagteam refactor R5a — neutral session records — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Approved
**Goal:** Session records get a neutral DTO (`SessionRecord` with a `ProcStart`) that `record_is_live` judges. They are read only through two new `Provider` methods and the neutral lister. Every piece of Claude Code's record format is gathered into two regions of `liveness.rs`, which R5b moves into `tagteam-cc` unchanged. Nothing a user sees changes.
**Architecture:**
- **Map, then judge.** `record_is_live` stops parsing text. Claude Code's `procStart` maps to one of four `ProcStart` forms (`proc_start_of`), and each form keeps today's outcome (spec §9's table).
- **Injection.** The lister and the supervisor-lock reader take the file extension and a parse function. The engine passes `p.session_record_ext()` and `p.parse_session_record`, so it no longer names a format, an extension or a kind.
- **Claude Code's regions.** Today's record struct and parser are renamed `CcRecord` and `parse_cc_record`. Beside them go `CcRecord::to_session_record`, `DAEMON_KINDS` (out of the engine) and the `parse_session_record` wrapper. They sit with `parse_lstart` and `proc_start_of` in two regions that nothing neutral calls. Claude Code's new tests start life in `tagteam-cc/tests/records.rs`.
- **FakeAgent** implements the two methods by delegating to Claude Code's parser, as it effectively does today. R5b gives it its own format before the move.

**Tech Stack:** Rust 1.88 (edition 2024), cargo workspace; serde_json (`preserve_order`, `arbitrary_precision`).

**Spec:** docs/superpowers/specs/2026-10-19-tagteam-review-refactor-design.md (§9, with §3, §12, §14 and §16)

**Branch:** `refactor-r5a-neutral-records`, based on `refactor-r4h-keychain-budget` (the last R4 slice's branch). R5b (`docs/superpowers/plans/2026-10-21-tagteam-refactor-r5b-record-formats.md`) builds on it.

**Reading this plan.**
- **Sites are named by function and quoted code.** The spec's line numbers were taken at `683b266`, and this plan's research at `8d44998`. R4a–R4g land in between. They do not touch `liveness.rs`, the reader functions of the engine's `session.rs`, or `tokens/evidence.rs`. Task 0 confirms that.
- **Focused test commands:**
  - `cargo test -p tagteam-provider --locked --lib liveness`;
  - `cargo test -p tagteam-cc --locked`;
  - `cargo test -p tagteam-fake --locked`;
  - `cargo test -p tagteam-engine --locked --features test-hooks --test session --test fake_sessions --test tokens_evidence --test tokens_view --test session_views --test doctor --test purge --test switch`;
  - `cargo test -p xtask --locked`.
- **Each task** leaves the workspace compiling, with its crates' tests green.

## Global Constraints

- **No user-visible change.** Every message, JSON field, exit code, help line and log line stays byte-identical. §11 has no R5 row, so there is no `CHANGELOG.md` entry. The existing tests that assert the session-owned refusals, doctor's `sessions.*` lines and the token report's sightings are the check, and none is loosened.
- **Same liveness outcome for every record.** Each row of §9's `ProcStart` table keeps today's outcome. This includes an all-digit start that overflows `u64`.
- **No wire or store change.**
- **Hardening's code** (§3.2):
  - In `liveness.rs`, R5 edits the parser region: records, the lister, the lock reader, `lstart`, `record_is_live` and their tests. The probe region (`ProcessProbe`, `SystemProcessProbe`, `FakeProcess`, `FakeProcessProbe`, their helpers and tests) is not edited. The one exception is the `SessionRecord` literal in `a_pid_no_process_can_hold_is_gone_and_unprobeable_pids_are_undetermined` (Task 3).
  - In `tokens/evidence.rs`, only the record reads change, about 8 lines: the imports, `scan_sessions`'s lister call, and the path and parse in `observe_launch` and `execution_started_at`.
  - No other hardening-owned file is edited.
- **Held TUI files are not edited:** `crates/tagteam-tui/**`, `crates/tagteam/src/tui_host.rs` and `crates/tagteam/src/tui_cmd.rs`. Nothing they call changes.
- **The layout R5b moves.** After Task 3, Claude Code's record code in `liveness.rs`'s production half is exactly two contiguous regions:
  - **region A:** `CcRecord`, `MAX_DEPTH`, `depth`, `pid_of`, `parse_cc_record`, `DAEMON_KINDS`, `impl CcRecord`, `parse_session_record`, in that order, at the top where `SessionRecord` is today;
  - **region B:** `WEEKDAYS` through `parse_lstart`, then `proc_start_of`, where `lstart` is today.

  Their unit tests are three contiguous blocks of the test module:
  - the `record` helper and the four record-parse tests;
  - the three `lstart` and civil-arithmetic tests;
  - `lstart_of` and `this_process_s_start_reads_back_from_its_lstart_text`.

  After Task 3, no neutral code and no neutral test names a region-A or region-B item, and the Claude Code blocks use no helper of the neutral tests. Claude Code's new tests that need only public items go in `crates/tagteam-cc/tests/records.rs`.
- **Unit-test modules stay last** in each file, with `#[cfg(test)]` alone on the line directly above `mod tests {`.
- **CI commands** (from `CONTRIBUTING.md`), all of which must pass:
  - `cargo fmt --all --check`;
  - `cargo clippy --workspace --locked -- -D warnings`;
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`;
  - `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`.

  Every task runs the clippy variants and the touched crates' focused tests.
- **No agent attribution** in commits. **Commit messages** are imperative and in sentence case, with no prefix and no trailing period. Each task's message is given in its last step.
- **No test is weakened.** The only test edits allowed are:
  - mechanical type and signature changes (records, the lister, the lock reader);
  - `record_is_live`'s tests re-expressed with `ProcStart` (Task 3);
  - the lister's tests moved off Claude Code's format onto a test-local parser (Task 2);
  - new tests.
- **No new `#[allow(...)]`.** Comment lines wrap at 100 columns.

## Review Focus

These are the five failure modes the spec implies but no existing test exercises, most likely to bite first. Each one's test is added to the task that owns it.

1. **A numeric or padded `procStart`.**
   - A JSON number (`"procStart": 5000`) and padded digit text (`" 5000\n"`) must keep the boot-tick path.
   - A negative or fractional number is no start at all.
   - Tests: Task 1, `each_proc_start_text_maps_to_its_form` and `each_proc_start_form_keeps_its_liveness_outcome`.
2. **A `kind` that is almost a daemon kind.**
   - `"BG"`, `" bg"`, `"daemon-workers"` and a `kind` that is not text are not background, because today's check is an exact match.
   - A mapping that lowercases or trims would turn a session into a daemon, which changes the refusal's wording and remedies.
   - Test: Task 3, `a_cc_record_maps_its_kind_to_background_and_its_start_to_a_form`.
3. **The supervisor lock's record is not `background`.**
   - Claude Code's `daemon.lock` has `origin`, not `kind`. Reading it through the provider's parser must still make a live supervisor `daemon`-owned, never a `tagteam run` session.
   - Test: the existing `a_live_supervisor_in_daemon_lock_owns_the_account_and_a_dead_one_does_not` (engine `tests/session.rs`) asserts `session: false, daemon: true`. Task 3 runs it and must not edit it.
4. **Files beside the records.**
   - Listing by extension must skip `keeper.lock`, `notes.txt` and, when the extension is `rec`, a `*.json`.
   - It must still report a directory named like a record as unreadable.
   - Test: Task 2, the rewritten `records_are_listed_in_name_order_and_a_malformed_one_is_unreadable`.
5. **The token report's dating after the lister changes.**
   - The generic lister must still ask `clock` once for each file it reads, right after the read.
   - Test: Task 2, the rewritten `each_record_is_dated_right_after_its_own_read`.

## Spec coverage

| Spec | Tasks |
|---|---|
| §9, `ProcStart` and its table; §12, every branch of `record_is_live` including the overflow | 1, 3 |
| §9, the neutral DTO | 3 |
| §9, the two trait methods; files `<dir>/<pid>.<ext>` | 2 |
| §9, cc parses its JSON, `lstart` and the legacy form, and maps kinds to `background` | 1, 3 (gathered in regions A and B; R5b moves them into cc) |
| §9, `DAEMON_KINDS` leaves the engine | 3 |
| §9, the supervisor lock through `supervisor_lock()` plus `parse_session_record` | 2 |
| §9, `error.rs` stops naming `daemon.lock` | already true since R1; checked in Final verification |
| §9, the neutral lister `read_session_records(dir, ext, parse, clock)` | 2 |
| §9, `evidence.rs`'s readers keep their outcomes | 2 |
| §9, xtask's compat harness follows | 2, 3 |
| §16, core §4.5 lists `parse_session_record` and `session_record_ext` | 2 |
| §14, plan-local comments in code R5a rewrites | 3 |
| §4, status | Final records |

FakeAgent's own format, the move into `tagteam-cc` and the neutrality test's field names are R5b's.

## Task order and batches

Tasks run one at a time, in order. Each task edits `liveness.rs` or a type it defines, so no two tasks can share a batch.

---

### Task 0: Confirm the base (no commit)

**Depends on:** none.

- [ ] **Step 1: The base.**
  - `git merge-base --is-ancestor d0c9dfe HEAD` (hardening) must succeed.
  - `git diff 8d44998 HEAD -- crates/tagteam-provider/src/liveness.rs crates/tagteam-engine/src/session.rs crates/tagteam-engine/src/tokens/evidence.rs crates/tagteam-fake/src crates/xtask/src/compat` is expected to be empty.
  - For any hunk, find each site this plan names by its quoted code, and list the hunk in the report.
- [ ] **Step 2: The callers.**
  - Run `git grep -n -E 'parse_session_record|read_session_records|read_supervisor_lock|parse_lstart|record_is_live|SessionRecord|RecordEntry' -- crates`.
  - Compare the hits with the files Tasks 2 and 3 list. A caller this plan does not list is reported before Task 1 starts.

### Task 1: Judge a session record by the form of its process start

**Depends on:** Task 0.

**Files:**
- Modify: `crates/tagteam-provider/src/liveness.rs` (`ProcStart` above `pub enum RecordEntry`; `proc_start_of` directly below `parse_lstart`; `record_is_live`'s body and doc)
- Modify: `crates/tagteam-provider/src/lib.rs` (`ProcStart` joins the root `pub use liveness::{…}`)
- Create: `crates/tagteam-cc/tests/records.rs`

**Interfaces:**
- Produces:

```rust
// liveness.rs, neutral region
/// When a recorded process started, in the form its record gives it (§12.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcStart {
    /// Epoch seconds.
    EpochS(i64),
    /// Linux boot ticks, `/proc/<pid>/stat` field 22.
    BootTicks(u64),
    /// Boot ticks too large for a `u64`.
    BootTicksIndeterminate,
    /// None given, or none this build can read.
    Unknown,
}
// liveness.rs, region B
/// Claude Code's `procStart`, trimmed: `lstart` text is `EpochS`; otherwise non-empty all-digit
/// text is `BootTicks`, or `BootTicksIndeterminate` when it overflows `u64`; anything else,
/// absence included, is `Unknown`.
pub fn proc_start_of(text: Option<&str>) -> ProcStart;
```

- [ ] **Step 1: Write the pin** `each_proc_start_form_keeps_its_liveness_outcome` in `crates/tagteam-cc/tests/records.rs`. Its header is `//! Claude Code's session records (Appendix A.7) as the neutral liveness judgement reads them (refactor spec §9).`
  - Each case builds `{"pid": 100, "startedAt": 1_790_858_096_000}` plus the case's `procStart` (absent where the case says so). It parses the bytes with `tagteam_provider::parse_session_record` and judges them with `record_is_live(&probe, &r, "claude")`. The `FakeProcessProbe` holds one `FakeProcess` for pid 100, built inline.
  - `"Thu Oct  1 12:34:56 2026"` is `1_790_858_096`. The test defines its own constants and uses no other test's helper.

```
// procStart                                   probe (exists: Some(true) unless given)          live
"Thu Oct  1 12:34:56 2026"                     start 1_790_858_097, mentions Some(false)        true
"Thu Oct  1 12:34:56 2026"                     start 1_790_858_098, mentions Some(false)        false
"Thu Oct  1 12:34:56 2026"                     start 1_790_858_098, mentions None               true
"Thu Oct  1 12:34:56 2026"                     start None,          mentions Some(false)        true
"5000", 5000 (number)                          ticks Some(5000) | Some(5001) | None             true | false | true
"99999999999999999999", 99999999999999999999   ticks Some(5000) | None                          true | true
absent, null, "", "not a date", "-5", -5       start 1_790_858_217, mentions Some(false)        false
the same six                                   start 1_790_858_216, mentions Some(false)        true
the same six                                   start 1_790_858_217, mentions Some(true)         true
every procStart above                          exists Some(false) | exists None                 false | true
```

  Run `cargo test -p tagteam-cc --locked --test records`. Expected: PASS today. This is a characterization pin, and it must stay textually unchanged through Task 3.
- [ ] **Step 2: Write** `each_proc_start_text_maps_to_its_form` in the same file:

```
Some("Thu Oct  1 12:34:56 2026"), Some(" Thu Oct  1 12:34:56 2026\n")  → EpochS(1_790_858_096)
Some("5000"), Some(" 5000\n")                                          → BootTicks(5000)
Some("18446744073709551615")                                           → BootTicks(u64::MAX)
Some("18446744073709551616"), Some("99999999999999999999999999")       → BootTicksIndeterminate
None, Some(""), Some("  "), Some("not a date"), Some("-5"), Some("1.5"),
Some("Thu Oct  1 12:34:56 1969")                                       → Unknown
```

  Run it. Expected: it fails to compile, because `proc_start_of` does not exist.
- [ ] **Step 3: Implement** `ProcStart` and `proc_start_of`, and make `record_is_live` match on the form. The `exists` gate before the match stays as it is.

```rust
match proc_start_of(r.proc_start.as_deref()) {
    ProcStart::EpochS(recorded) => /* today's ±1 s test, else mentions(..).unwrap_or(true) */,
    ProcStart::BootTicks(recorded) => probe.start_ticks(r.pid).is_none_or(|actual| actual == recorded),
    ProcStart::BootTicksIndeterminate => true,
    ProcStart::Unknown => /* today's 120 s `startedAt` heuristic, unchanged */,
}
```

  `record_is_live`'s doc and comments name the forms instead of `procStart` and `lstart`. They keep their §12.6 and Decision 16 references.
- [ ] **Step 4: Verify.**
  - Run Steps 1–2's tests (expected: PASS).
  - Run the provider's liveness tests. They are unchanged and must all pass: they are the proof that no outcome moved.
  - Run the clippy variants.
- [ ] **Step 5: Commit** "Judge a session record by the form of its process start".

### Task 2: Read session records through the provider's parser and file extension

**Depends on:** Task 1.

**Files:**
- Modify: `crates/tagteam-provider/src/provider.rs` (two methods in `trait Provider`, directly below `supervisor_lock`; `supervisor_lock`'s doc)
- Modify: `crates/tagteam-provider/src/liveness.rs` (`RecordEntry`, `read_session_records`, `read_supervisor_lock`, `session_record_path`; the lister and lock tests)
- Modify: `crates/tagteam-provider/src/lib.rs` (drop `read_session_records_dated`; add `session_record_path`)
- Modify: `crates/tagteam-cc/src/provider.rs` and `crates/tagteam-fake/src/provider.rs` (the two methods)
- Modify: `crates/tagteam-engine/src/session.rs` (`session_state_leaving_out`)
- Modify: `crates/tagteam-engine/src/tokens/evidence.rs` (imports, `scan_sessions`, `observe_launch`, `execution_started_at`)
- Modify: `crates/xtask/src/compat/daemon.rs` (`read_lock`, `read_records`) and `crates/xtask/src/compat/checks/profile.rs` (its two `read_supervisor_lock` calls)
- Modify: `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md` (the §4.5 pointer line)

**Interfaces:**
- Consumes: Task 1's `ProcStart`.
- Produces:

```rust
// provider.rs, trait Provider
/// §12.6: one of this agent's session records, or its supervisor lock's content, reduced to
/// what liveness and the token report need. Bytes that are no record are `Err(detail)`, and
/// `detail` never quotes them. `pid` is 1 through `i32::MAX`, never a process group.
fn parse_session_record(&self, bytes: &[u8]) -> Result<SessionRecord, String>;
/// A record's file extension: records are `<session_records_dir>/<pid>.<ext>`.
fn session_record_ext(&self) -> &'static str;
// liveness.rs
#[derive(Debug, Clone, PartialEq)]
pub enum RecordEntry<R = SessionRecord> { Record(R), Unreadable { path: PathBuf, detail: String } }
/// Every `<dir>/*.<ext>` in name order, each parsed by `parse` and dated by `clock` right after
/// its read, before it is parsed and before the next read (token report spec §6.4).
pub fn read_session_records<R>(
    dir: &Path,
    ext: &str,
    parse: &dyn Fn(&[u8]) -> Result<R, String>,
    clock: &mut dyn FnMut() -> i64,
) -> Read<Vec<(RecordEntry<R>, i64)>>;
pub fn read_supervisor_lock<R>(path: &Path, parse: &dyn Fn(&[u8]) -> Result<R, String>) -> Read<R>;
/// `<dir>/<pid>.<ext>`.
pub fn session_record_path(dir: &Path, pid: u32, ext: &str) -> PathBuf;
```

- `read_session_records_dated` goes. Its body becomes `read_session_records`'s, with the extension test against `ext`. Every documented rule of both docs stays: missing or `ENOTDIR` is `Present(vec![])`, a failed listing is `Unreadable`, and a record that vanishes is skipped.
- `read_supervisor_lock` keeps its `Absent`/`Unreadable` rules and parses with `parse`.

- [ ] **Step 1: Rewrite the lister and lock tests over a test-local parser.**
  - `fn pid_text(bytes: &[u8]) -> Result<u32, String>` reads UTF-8 decimal text as a pid above 0, and anything else is `Err`. The tests use `R = u32` and the extension `rec`, so they never name `SessionRecord` and stay neutral after R5b.
  - **`records_are_listed_in_name_order_and_a_malformed_one_is_unreadable`.** A missing directory gives `Present(vec![])`. The directory holds `20.rec`, `10.rec`, `30.rec` (`{`), `notes.txt`, `50.json`, `keeper.lock` and a directory `40.rec`. The entries are `[Record(10), Record(20), Unreadable(30.rec), Unreadable(40.rec)]` and nothing else (Review Focus 4).
  - **`each_record_is_dated_right_after_its_own_read`.** The dates are `[1_000, 2_000, 3_000]` for `10`, `20` and the torn `30`, and `notes.txt` is not dated (Review Focus 5).
  - **`a_records_directory_that_cannot_be_listed_is_unreadable`** and **`no_record_can_lie_under_something_that_is_not_a_directory`** keep their assertions with the new call.
  - **`a_supervisor_lock_is_read_as_a_record_and_absent_or_unreadable_otherwise`** works on `keeper.lock`:
    - missing, and under a missing directory: `Absent`;
    - `77`: `Present(77)`;
    - `{`, `seven`, an empty file and `0`: `Unreadable`, whose `what` ends with `keeper.lock`;
    - a directory: `Unreadable`.
  - Run them. Expected: they fail to compile.
- [ ] **Step 2: Implement the liveness changes** as the Interfaces block gives them.
- [ ] **Step 3: The trait.**
  - Add both methods. `provider.rs` imports `crate::liveness::SessionRecord`.
  - `supervisor_lock`'s doc gains "Its content is read with `parse_session_record`."
  - **`ClaudeCode`** returns `parse_session_record(bytes)` through a separate `use tagteam_provider::liveness::parse_session_record;` line, which R5b repoints, and `"json"`.
  - **`FakeAgent`** does the same for now: the same delegation and `"json"`. R5b gives it its own format.
- [ ] **Step 4: The engine reads through the trait.**
  - **`session.rs`, `session_state_leaving_out`:**
    - `read_session_records(&p.session_records_dir(&profile))` becomes `read_session_records(&p.session_records_dir(&profile), p.session_record_ext(), &|b| p.parse_session_record(b), &mut || 0)`, and the loop takes `(entry, _)`.
    - `read_supervisor_lock(&lock)` becomes `read_supervisor_lock(&lock, &|b| p.parse_session_record(b))`.
    - The comment above it says that a supervisor which writes no record has a lock file, read with its provider's parser and judged as a record is (§12.6).
    - If inference asks, annotate the closures `|b: &[u8]|`.
  - **`evidence.rs`:**
    - The import list drops `parse_session_record` and `read_session_records_dated`, and gains `read_session_records` and `session_record_path`.
    - **`scan_sessions`:** `read_session_records_dated(&dir, &mut clock)` becomes `read_session_records(&dir, p.session_record_ext(), &|b| p.parse_session_record(b), &mut clock)`.
    - **`observe_launch`:** `p.session_records_dir(profile).join(format!("{pid}.json"))` becomes `session_record_path(&p.session_records_dir(profile), pid, p.session_record_ext())`, and `parse_session_record(&bytes)` becomes `p.parse_session_record(&bytes)`.
    - **`execution_started_at`:** `records.join(format!("{pid}.json"))` becomes `session_record_path(records, pid, p.session_record_ext())`, and `parse_session_record(&bytes)` becomes `p.parse_session_record(&bytes)`.
- [ ] **Step 5: xtask follows.**
  - **`daemon.rs`:**
    - `read_lock` calls `read_supervisor_lock(&home.join("daemon.lock"), &parse_session_record)`.
    - `read_records` calls `read_session_records(&home.join("sessions"), "json", &parse_session_record, &mut || 0)` and takes `(entry, _)`.
  - **`checks/profile.rs`:** both `read_supervisor_lock(&home.join("daemon.lock"))` calls take `&parse_session_record`, imported from `tagteam_provider`.
- [ ] **Step 6: Core §4.5.** The line "The review refactor adds `vars`, `words`, `statusline_setup` and `foreign_export` (…)." gains a sentence: "It also adds `parse_session_record` and `session_record_ext` ([review refactor §9](2026-10-19-tagteam-review-refactor-design.md))."
- [ ] **Step 7: Verify.**
  - Run every focused command and the clippy variants.
  - `git grep -n -E 'parse_session_record|read_session_records_dated|\.json"' -- crates/tagteam-engine/src/session.rs crates/tagteam-engine/src/tokens/evidence.rs`. Expected: only `p.parse_session_record` calls.
- [ ] **Step 8: Commit** "Read session records through the provider's parser and file extension".

### Task 3: Make the session record neutral and keep Claude Code's own as `CcRecord`

**Depends on:** Task 2.

**Files:**
- Modify: `crates/tagteam-provider/src/liveness.rs`:
  - region A: rename the struct and parser; add `DAEMON_KINDS`, `impl CcRecord` and the wrapper;
  - the neutral `SessionRecord`, directly below `ProcStart`;
  - `record_is_live`;
  - the tests.
- Modify: `crates/tagteam-engine/src/session.rs` (`DAEMON_KINDS` goes; `session_state_leaving_out`'s record branch; `SessionState::Owned`'s doc)
- Modify: `crates/xtask/src/compat/daemon.rs`, `crates/xtask/src/compat/checks/mod.rs`, `crates/xtask/src/compat/checks/profile.rs`
- Test: `crates/tagteam-cc/tests/records.rs`

**Interfaces:**
- Consumes: Task 1's `ProcStart` and `proc_start_of`; Task 2's lister.
- Produces:

```rust
// liveness.rs, neutral region
/// A session record, reduced to what liveness and the token report's sightings need (§12.6,
/// token report spec §2.4). A provider's `parse_session_record` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub pid: u32,
    pub start: ProcStart,
    /// When the session's execution started, epoch ms.
    pub started_at_ms: Option<i64>,
    pub session_id: Option<String>,
    /// Written by a background session rather than an interactive one.
    pub background: bool,
}
// liveness.rs, region A: today's struct and parser, renamed, bodies unchanged
pub struct CcRecord { pub pid: u32, pub proc_start: Option<String>, pub started_at_ms: Option<i64>, pub kind: Option<String>, pub session_id: Option<String> }
pub fn parse_cc_record(bytes: &[u8]) -> Result<CcRecord, String>;
const DAEMON_KINDS: [&str; 3] = ["bg", "daemon", "daemon-worker"]; // from the engine, with its doc
impl CcRecord {
    /// `start` is `proc_start_of(proc_start)`; `background` is whether `kind` is exactly one of
    /// `DAEMON_KINDS`; `pid`, `started_at_ms` and `session_id` are carried over.
    pub fn to_session_record(&self) -> SessionRecord;
}
pub fn parse_session_record(bytes: &[u8]) -> Result<SessionRecord, String>; // parse_cc_record, then to_session_record
```

- `record_is_live` matches on `r.start` in place of `proc_start_of(r.proc_start.as_deref())`.
- `CcRecord`'s doc is today's, saying it is the record as Claude Code writes it, before `to_session_record`.

- [ ] **Step 1: The tests.**
  - **Claude Code's unit tests:**
    - The `record` helper parses with `parse_cc_record`, and the four record-parse tests compare `CcRecord` literals. Their assertions are otherwise unchanged.
    - In the real-process test, the line `assert_eq!(parse_lstart(&lstart_of(start)), Some(start));` moves into a new test, `this_process_s_start_reads_back_from_its_lstart_text`. That test goes directly below `lstart_of`. It reads `SystemProcessProbe.start_time_s(std::process::id())` and asserts the round trip.
  - **In `crates/tagteam-cc/tests/records.rs`, `a_cc_record_maps_its_kind_to_background_and_its_start_to_a_form`** (Review Focus 2):
    - `kind` `"bg"`, `"daemon"` and `"daemon-worker"` are `background: true`.
    - `"interactive"`, `"BG"`, `" bg"`, `"daemon-workers"`, `3`, `null` and an absent `kind` are `false`.
    - `{"pid":7,"procStart":"Thu Oct  1 12:34:56 2026","startedAt":1790858096000,"sessionId":"s","kind":"bg"}` parses through `parse_session_record` to `SessionRecord { pid: 7, start: ProcStart::EpochS(1_790_858_096), started_at_ms: Some(1_790_858_096_000), session_id: Some("s".into()), background: true }`.
    - `"procStart": 123456` gives `BootTicks(123456)`, and `"procStart": 99999999999999999999` gives `BootTicksIndeterminate`.
    - For `{`, `[1]` and `{"pid": 0}`, `parse_session_record` is the same `Err` as `parse_cc_record`.
  - **The neutral tests:**
    - `rec` becomes `fn rec(start: ProcStart, started_at_ms: Option<i64>) -> SessionRecord`, with pid 100, `session_id: None` and `background: false`.
    - The `record_is_live` tests pass forms: `Some(LSTART)` becomes `EpochS(STARTED)`, and `Some("5000")` becomes `BootTicks(5000)`.
    - The heuristic test's inner loop over `[None, Some(""), Some("not a date")]` becomes the single form `Unknown`. Task 1 pins those three texts' mapping.
    - The real-process test uses `EpochS(start)` and `EpochS(start - 3_600)`, and the gone-pid test's literal takes the new fields.
    - The neutral test constant `LSTART` goes once it is unused.
    - These renames keep each name true:
      - `an_lstart_record_of_another_program_is_live_only_within_a_second_of_the_os_start_time` becomes `an_epoch_start_of_another_program_…`;
      - `an_lstart_mismatch_is_recycled_only_when_the_process_does_not_mention_the_launch_command` becomes `an_epoch_start_mismatch_…`;
      - `an_all_digit_proc_start_must_equal_the_start_ticks` becomes `a_boot_tick_start_must_equal_the_start_ticks`;
      - `without_a_usable_proc_start_cswaps_heuristic_decides` becomes `without_a_known_start_cswaps_heuristic_decides`.
    - New: `an_indeterminate_boot_tick_start_is_live_whatever_the_ticks`. With ticks `Some(5000)`, `Some(0)` and `None` it is live; with `exists: Some(false)` it is dead.
    - The two "Review Focus 2:" comments are rewritten as the rule they stand for (§14).
  - Run them. Expected: they fail to compile.
- [ ] **Step 2: Implement** the liveness changes in the layout Global Constraints gives.
- [ ] **Step 3: The engine.**
  - Delete `DAEMON_KINDS` and its doc from `session.rs`.
  - The record branch becomes `if r.background { daemon = true; } else { session = true; }`.
  - The supervisor branch is unchanged: a live lock is `daemon`, whatever its record's `background` says (Review Focus 3).
  - `SessionState::Owned`'s doc: `daemon` is a background session (a record its provider marks `background`) or the supervisor's lock file. It names no kinds.
- [ ] **Step 4: xtask follows.**
  - It imports `CcRecord` and `parse_cc_record` from `tagteam_provider::liveness`.
  - **`daemon.rs`:**
    - `roster_records` returns `CcRecord`s, through `parse_cc_record` at both of its parses.
    - `roster_pid_lives` takes `&CcRecord` and judges `&record.to_session_record()`. Its `record.proc_start.is_some()` stays.
    - `read_records` lists with `&parse_cc_record`, so `r.kind` and its "a record of kind …" text stay, and judges `&r.to_session_record()`.
    - `read_lock` keeps `parse_session_record`.
  - **`checks/mod.rs`:** `records` and `new_record` hold `CcRecord`.
  - **`checks/profile.rs`:** its records filter judges `&r.to_session_record()`.
  - xtask's own `DAEMON_KINDS` in `ctx.rs` stays.
- [ ] **Step 5: Verify.**
  - Run every focused command (the engine's `tests/session.rs` and xtask's survey tests unchanged) and the clippy variants.
  - `git grep -n -E 'DAEMON_KINDS|daemon-worker' -- crates/tagteam-engine`. Expected: no hits.
- [ ] **Step 6: Commit** "Make the session record neutral and keep Claude Code's own as CcRecord".

## Final verification and records (the controller, before the PR)

- **macOS, outside the sandbox:** `cargo fmt --all --check`, the three clippy commands, and `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`. Report the totals as passed/total.
- **Linux:** the same `cargo test` command in the Docker reproduction (`rust:1.88`, run as a non-root user with a passwd entry, `HOME` outside `/tmp`), against the branch head.
- **The held crates:**
  - `cargo build -p tagteam-tui --locked`;
  - `git diff --stat <base>..HEAD -- crates/tagteam-tui crates/tagteam/src/tui_host.rs crates/tagteam/src/tui_cmd.rs` must be empty.
- **Boundary checks:**
  - `cargo test -p tagteam-fake --locked --test neutrality` passes, unchanged.
  - `git grep -n 'daemon\.lock' -- crates/tagteam-engine/src/error.rs` hits only inside its `#[cfg(test)]` module (§9's `error.rs` item, done by R1).
  - The layout check: in `liveness.rs`'s production half, `CcRecord`, `parse_cc_record`, `DAEMON_KINDS`, `parse_lstart` and `proc_start_of` appear only inside regions A and B.
- **Records, in the final pre-merge commit:**
  - this plan's `**Status:**` becomes `Implemented — <R5a PR link>`;
  - the refactor spec's `**Status:** In progress — …` line gains `, <R5a PR link> (R5a)`.

  Message: "Mark the R5a plan implemented and cite its PR in the refactor spec".

## Spec deltas found while planning

These are rulings inside the spec's intent. Each one is recorded under "Rulings" in the PR.

1. **R5 ships as R5a and R5b, neutral first.** `tagteam-cc` depends on `tagteam-provider`. The provider's own lister and `record_is_live` call Claude Code's parser and `parse_lstart`, so nothing can leave the provider until they are neutral. A pure move cannot come first. FakeAgent's format goes in R5b to balance the two PRs.
2. **Claude Code's record code is gathered, then moved.** R5a puts it in two regions of `liveness.rs`: `CcRecord`, `parse_cc_record`, `to_session_record`, `DAEMON_KINDS`, `parse_lstart` and `proc_start_of`. R5b moves those regions byte-identical. Claude Code's new tests live in `tagteam-cc/tests/records.rs` from the start, so R5b changes only their imports.
3. **cc keeps its full record beside the neutral one.** The compat harness reads `kind` and `procStart` text. `parse_session_record` is `parse_cc_record`, then `to_session_record`.
4. **The lister and the lock reader are generic over the parsed record** (`RecordEntry<R = SessionRecord>`). The harness lists `CcRecord`s through the same neutral lister and keeps its "a record of kind …" evidence.
5. **There is one lister, and it always dates.** Callers that do not need dates pass `&mut || 0`.
6. **`session_record_path(dir, pid, ext)`** carries the `<dir>/<pid>.<ext>` rule for `evidence.rs`'s two reads by pid.
7. **The supervisor lock owns as `daemon` whatever its record's `background` says**, as it does today.
8. **`error.rs` needs no edit.** R1 already words the lock's repair from `supervisor_lock()`.
9. **FakeAgent delegates to Claude Code's parser in R5a.** That is today's behaviour, made explicit. R5b replaces it.
10. **`record_is_live`'s Decision 16 references stay** (§14). The two "Review Focus 2" test comments are rewritten.
11. **xtask keeps its own `DAEMON_KINDS`**, as an independent check of Claude Code's behaviour.
