# tagteam refactor R3 — CLI and the daemon protocol's typed values — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Approved
**Goal:** Split `crates/tagteam/src/app.rs` into `app/`, give the CLI one bootstrap and one way to report, warn and refuse, tidy its small duplications, and give the daemon protocol typed, additive accessors, with one user-visible change: `tagteam completions` exits non-zero when it cannot write.
**Architecture:**
- **Moves and edits.** Edits that shape `app.rs`'s pieces (dispatch arms becoming methods, the log target pin) land first, inside `app.rs`. The move to `app/` then comes from a deterministic generator script, so it can be regenerated after the hardening rebase. After that come the new helpers in `app/` and the small items.
- **The protocol** gains parsing, `Wire<T>`, typed accessors, predicates and payload parsers beside its `String` fields. Nothing on the wire changes.
**Tech Stack:** Rust 1.88 (edition 2024), cargo workspace; clap 4, clap_complete, serde_json (`preserve_order`, `arbitrary_precision`), jiff.

**Spec:** docs/superpowers/specs/2026-10-19-tagteam-review-refactor-design.md (§7, with §3, §11, §12 and §14)

**Reading this plan.**
- **Line numbers** are as of `59ea9cb`. They drift as tasks land, so find each site by the code quoted.
- **Focused test commands:**
  - `cargo test -p tagteam --locked --features test-support`;
  - `cargo test -p tagteam-daemon --locked --features test-hooks`;
  - `cargo test -p tagteam-engine --locked --features test-hooks`;
  - `cargo test -p tagteam-core --locked`.
- **Each task** leaves the workspace compiling, with its crates' tests green.
- **Fact sheets** live in `.superpowers/research/r3-facts/`: `app.md`, `bootstrap-reporting-small.md` and `protocol.md`. They are measured, not decided; where one differs from this plan, this plan wins.

## Global Constraints

- **Byte-identical output.** Every message, JSON field and key order, exit code, help line, completions script, log line and log target stays byte-identical. The one exception is §11's R3 row: `tagteam completions` exits non-zero when it cannot write its output (Task 8, with a `CHANGELOG.md` entry). The existing tests that assert text are the check, and none is loosened.
- **No wire or store change.** The daemon protocol's JSON, the `--json` schemas and the store schema stay as they are.
- **Hardening-owned files are not edited:**
  - in `tagteam-provider`: `security.rs`, `process.rs`, `keychain.rs` and the fakes;
  - in `tagteam-engine`: `refresh.rs`, `store/tokens.rs` and `tokens/*`;
  - in `tagteam-daemon`: the loops in `server.rs`, `notify.rs` and `scheduler.rs`, and also `client.rs`;
  - in `crates/tagteam/src`: `run.rs` and `prompt.rs`;
  - `.github/workflows/*` and `dist-workspace.toml`.

  `render.rs`, `statusline.rs` and `tokens_cmd.rs` are files both branches edit. R3 changes only their `"schemaVersion": 1` literals (Task 6).
- **Held TUI files are not edited:** `crates/tagteam-tui/**`, `crates/tagteam/src/tui_host.rs` and `crates/tagteam/src/tui_cmd.rs`. These keep their paths and signatures:
  - `app::{run, Context, Io}`;
  - `app::{build_registry, locate, build_engine, color_enabled}`;
  - `app::{error_json, KIND_USAGE, KIND_ROOT, EXIT_USAGE, EXIT_ERROR, UNLOCK_QUESTION}`;
  - `Context`'s fields;
  - `cli::Command`'s variants.

  The protocol additions are methods and free functions only. Held code builds `StatusProvider`, `StatusOutcome` and `Event` with struct literals, so none of those structs gains a field.
- **Pure moves are separate from edits.** Tasks 2 and 7 are move tasks.
  - **A move commit may contain:** the moved items, byte-identical; new files and `mod` lines; `use` lines and re-exports; visibility raised to the minimum that compiles; and rustfmt's reflow of those lines.
  - **The reviewer** reads each move commit with `git diff --color-moved=zebra --color-moved-ws=allow-indentation-change`.
- **Unit-test modules stay last** in each file, with `#[cfg(test)]` alone on the line directly above `mod tests {`. `crates/tagteam-fake/tests/neutrality.rs` treats everything above it as production code.
- **CI commands** (from `CONTRIBUTING.md`), all of which must pass:
  - `cargo fmt --all --check`;
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`;
  - `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`.

  Every task runs both clippy variants, plus the touched crates' own test features.
- **No agent attribution** in commits. **Commit messages** are imperative and in sentence case, with no prefix and no trailing period. Each task's message is given in its last step.
- **No test is weakened.** The only test edits allowed are:
  - mechanical signature changes;
  - a test moving with its code;
  - a test literal kept as a literal where production code moves to a constant (Task 6);
  - new tests.
- **No new `#[allow(...)]`.** Comment lines wrap at 100 columns.

## Review Focus

These are the five failure modes the spec implies but that no existing test exercises, most likely to bite a user first. Each one's test is added to the task that owns it.

1. **The `warning:` line format.**
   - `Io::warn` replaces twelve hand-written `warning: {w}` lines. The held `tui_host.rs:72` strips exactly `"warning: {w}\n"` from captured stderr, so one stray space or a missing newline would leak startup warnings into the TUI.
   - Test: Task 4, `io_warn_and_note_write_one_prefixed_line_each` (`app/context.rs` tests).
2. **A failed completions write.**
   - Today `tagteam completions zsh > /full/disk` exits 0 with a truncated script.
   - Test: Task 8, `a_completions_script_that_cannot_be_written_is_an_io_error` (`app/early.rs` tests), with a writer that fails.
3. **An unknown daemon value.**
   - A newer daemon may send a mode or waiting reason this build does not know. `Wire<T>` must keep its spelling, and `collects_for_itself` and `waits_for_upgrade` must fall back to today's defaults (false and false).
   - Tests: Task 13, `a_wire_value_keeps_an_unknown_spelling` and `the_mode_predicates_default_to_false_for_unknown_values` (`protocol.rs` tests).
4. **The binding window's tolerance and tie rule.**
   - The TUI re-derives the window that bound an account from `windowsPct`, within 0.05 of its usage, taking the first in key order. A parser that rounds, or picks the highest instead, names the wrong window in "auto: 1 → 2 (5h at 95%)".
   - Test: Task 14, `the_binding_window_is_the_first_within_the_tolerance` (`protocol.rs` tests).
5. **An unknown no-switch reason in `tagteam auto`.**
   - After the switch to `NoSwitchReason::parse`, a reason this build cannot parse must still print `no switch: {reason}`, not be swallowed with `all-exhausted`'s branch.
   - Test: Task 14, `an_unknown_no_switch_reason_is_still_printed` (`crates/tagteam/src/auto.rs` tests).

Further pins added on the way:
- `local_time(i64::MAX)` does not panic (Task 12).
- `or_inactive`'s log line keeps its `tagteam::app` target (Task 1).

## Spec coverage

| Spec | Tasks |
|---|---|
| §7.1, `app.rs` becomes `app/` | 1, 2, 15 |
| §7.1, exhaustive dispatch | 0 (the spike and §7.1's decision rule) |
| §7.2, bootstrap | 3 |
| §7.3, `Failure::report`, `Io::warn/note/fail`, the pre-`Io` sites | 4 |
| §7.3, `refuse_in_run_shell` | 5 |
| §7.3, `SCHEMA_VERSION` | 6 |
| §7.3, `main_with_args` | 9 |
| §7.4, CC-O-11 (completions) | 7, 8 |
| §7.4, CC-O-12 (`ColorEnv`) | 10 |
| §7.4, CC-O-19 (`AutoStrategy`) | 11 |
| §7.4, ARCH-12 (`local_time`) | 12 |
| §7.5, parsing, `Wire<T>`, accessors, predicates | 13 |
| §7.5, payload parsers, `envelope()`, the CLI on `parse` | 14 |
| §11, R3's row (completions) | 8 |
| §14, "Decision N" in moved code, plan-local references in R3's files | 14 (protocol.rs), 15 (app/) |

## Task order and batches

Every task compiles the `tagteam` crate, so tasks run one at a time, in order.

**Before Task 2:** check whether `review-hardening` (#23) has merged into `main`.
- If it has, R1–R3 are rebased onto it first.
- If not, Task 2's generator regenerates the move after the later rebase (spec §3.2). Hardening's one `app.rs` hunk lands in `App::token`, in `commands/transfer.rs`.

---

### Task 0: Spike the flattened command enum (no commit)

**Depends on:** none.

**Files:** none committed. The spike edits `crates/tagteam/src/cli.rs` and the callers it breaks in the working tree, records what it observes, and reverts. `git -C <repo> status` must be clean at the end.

- [ ] **Step 1: Record the baseline.** At HEAD, build `cargo build -p tagteam --locked --features test-support`. Save `target/debug/tagteam --help` and `tagteam completions bash`, `zsh` and `fish` to files under the plan workspace.
- [ ] **Step 2: The spike.**
  - Split `Command` with `#[command(flatten)]` into stage groups, so each of `run_command`'s three early stages and `dispatch` can match exhaustively (§7.1):
    - one group for `Statusline` and `Tui`;
    - one for `ShellInit` and `Completions`;
    - `Run`;
    - the rest.
  - Fix only what the compiler requires to build, the held `tui_cmd.rs:142` pattern included.
  - Rebuild. Diff the four outputs against the baseline. Run `cargo test -p tagteam --locked --features test-support --test completions --test tui_args`.
- [ ] **Step 3: Record and revert.**
  - Write to the report: whether help order changed, whether any completions script changed (with the first differing lines), whether `tui_args` passed, and which held-file lines the spike had to touch.
  - Revert every change. `git status` must be clean.
- [ ] **Step 4: The decision**, by §7.1's rule:
  - **If help order, any completions script or `tui_args` changed**, keep the single enum and its `unreachable!` arms. The controller records the observed differences under "Rulings" in the PR, and the plan continues at Task 1 unchanged.
  - **If none changed**, stop and report. The controller then adds a task for the flatten before Task 1, with only a minimal compile fix in the held `tui_cmd.rs` (§3.4).

### Task 1: Make the dispatch's inline arms methods and pin `or_inactive`'s log target

**Depends on:** Task 0 (its decision).

**Files:**
- Modify: `crates/tagteam/src/app.rs` (the arms in `dispatch`, `:1306-1532`; `run_command`'s tui and completions answers, `:807-851`; `or_inactive`'s `tracing::warn!`, `:503`)

**Interfaces:**
- Produces these methods on `App`, all in `app.rs` for now:
  - `fn list(&mut self) -> Result<(), Failure>`;
  - `fn status(&mut self) -> Result<(), Failure>`;
  - `fn remove(&mut self, account: &str) -> Result<(), Failure>`;
  - `fn alias(&mut self, account: Option<String>, name: Option<String>, unset: bool) -> Result<(), Failure>`;
  - `fn move_account(&mut self, account: &str, position: u32) -> Result<(), Failure>`;
  - `fn doctor(&mut self, online: bool) -> Result<i32, Failure>`;
  - `fn auto_command(&mut self, once: bool, dry_run: bool, flags: AutoFlags) -> Result<i32, Failure>`;
  - `fn add_token(&mut self, token: Option<String>, position: Option<u32>, email: Option<String>, alias: Option<String>, yes: bool) -> Result<(), Failure>`. Its arm calls `token`, `confirming` and `added`; as a method beside them, it keeps them private to `commands/accounts.rs` after Task 2.
- Produces two free functions:
  - `fn refuse_tui(io: &mut Io<'_>, json: bool) -> Ended`, the tui refusal;
  - `fn answer_completions(io: &mut Io<'_>, json: bool, shell: CompletionShell) -> Ended`, the completions answer, with its `let _ = io.out.write_all(..)` unchanged. Task 8 changes it.

Facts: `app.md` §4 and §7.1.

- [ ] **Step 1: The arms become methods.**
  - Move each named arm's body into its method, unchanged.
  - The `Alias` arm keeps its runtime usage check and `ALIAS_USAGE` (C-10).
  - The `Auto` arm keeps building `flags` with `auto_flags(...)?` and passes `(once, dry_run, flags)`, which keeps `auto_command` within clippy's limit of seven parameters.
  - `AddToken` becomes a method too, so the helpers it calls stay private to the file they land in. `Disable` and `Enable` stay inline, and call only `print_account`.
  - `Move`'s method is `move_account`, because `move` is a keyword.
- [ ] **Step 2: Extract the two `run_command` answers** into `refuse_tui` and `answer_completions`, called from the same places in the same order.
- [ ] **Step 3: Pin the target.** Add `target: "tagteam::app",` as the first argument of `or_inactive`'s `tracing::warn!`. Task 2 moves it into `commands/accounts.rs`, which would otherwise change its target.
- [ ] **Step 4: Verify.** Run the `tagteam` focused command and both clippy variants.
- [ ] **Step 5: Commit** "Make the CLI dispatch's inline arms methods and pin or_inactive's log target".

### Task 2: Split `app.rs` into `app/`

**Depends on:** Task 1.

**Files:**
- Modify: `crates/tagteam/src/app.rs`, which becomes `crates/tagteam/src/app/mod.rs`
- Create, under `crates/tagteam/src/app/`:
  - `early.rs`, `context.rs`, `failure.rs`, `dispatch.rs`;
  - `commands/mod.rs`, `commands/accounts.rs`, `commands/launch.rs`, `commands/transfer.rs`, `commands/daemon.rs`, `commands/tokens.rs`, `commands/mappings.rs` and `commands/config.rs`.
- Create, untracked: `<workspace>/split_app.py`, the generator

**Interfaces:**
- Produces: every path listed in Global Constraints resolves unchanged.
  - `app/mod.rs` re-exports `pub use context::{Context, Io};`, `pub use failure::KIND_STALE_SELECTION;`, the `pub(crate)` items `app.md` §2 lists, and `run` and `UNLOCK_QUESTION`, which are defined in `mod.rs`.
  - `tagteam/tests/app.rs` and the held files compile without an edit.

Facts: `app.md` §1–§3 and §7.

- [ ] **Step 1: Rename the file.** The controller runs `git mv crates/tagteam/src/app.rs crates/tagteam/src/app/mod.rs`, builds, and commits "Move app.rs to app/mod.rs".
- [ ] **Step 2: Write the generator.** `split_app.py` follows the switch generator of R2:
  - **Input and output.** It reads `app/mod.rs` and writes the files.
  - **Finding items.** It finds each item by its declaration, never by line number. It takes each item's doc comments and attributes with it, and ends the item at its matching brace.
  - **Headers.** It writes each file's header: the `use` lines it needs, plus the `mod` lines and re-exports.
  - **Visibility and tests.** It applies exactly the raises `app.md` §2 lists. It distributes the unit tests to their items' files (`app.md` §7.3), each test module last.
  - **Failure.** It fails loudly on an item it cannot place.
  - **Why.** Spec §3.2: hardening edits `app.rs`, so after the rebase this commit is regenerated, not merged by hand. Keep the script in the plan workspace until R3 is rebased.
- [ ] **Step 3: Run it.** Destinations are `app.md` §1's table, with Task 1's new methods placed thus:
  - `list`, `status`, `remove`, `alias` and `move_account` go to `commands/accounts.rs`;
  - `doctor` goes to `commands/config.rs`;
  - `auto_command` goes to `commands/daemon.rs`;
  - `refuse_tui` and `answer_completions` go to `early.rs`.

  `add_token` also goes to `commands/accounts.rs`. `resolve` goes there as well, as §7.1's table has it, at `pub(in crate::app)`.

  Beyond `app.md` §2's list of raises:
  - `add_token` becomes `pub(in crate::app)`, because `dispatch` calls it;
  - `auto_flags` becomes `pub(in crate::app)`, because the `Auto` arm in `dispatch.rs` still calls it. `dispatch.rs` imports it.

  `confirming` and `added` stay private, because only `add_token` and `add` call them. Any other raise the compiler requires is the minimum that compiles, and is listed in the report. Then run `cargo fmt --all`.
- [ ] **Step 4: Verify.**
  - Run the `tagteam` focused command, both clippy variants, and `cargo build -p tagteam --locked` without features. The `#[cfg(feature = "test-support")]` items moved to `context.rs` must build both ways.
  - Run `cargo test -p tagteam-fake --locked --test neutrality`.
  - Check with `git diff --color-moved=zebra` that the only non-moved lines are headers and visibility.
- [ ] **Step 5: Commit** "Split the CLI's app into context, failure, early, dispatch and command modules".

### Task 3: Find the run shell through one `locate_run_shell`

**Depends on:** Task 2.

**Files:**
- Modify: `crates/tagteam/src/app/context.rs`, `crates/tagteam/src/app/mod.rs` (`run_command`)

**Interfaces:**
- Produces, in `context.rs`:

```rust
pub(crate) struct Located {
    pub(crate) registry: ProviderRegistry,
    pub(crate) run_shell: RunShell, // never RunShell::Unreadable
    pub(crate) env: Env,
}
pub(crate) fn locate_run_shell(ctx: &Context) -> Result<Located, EngineError>;
pub(crate) fn engine_for(ctx: Context, provider: Option<&ProviderId>) -> Result<(Engine, Vec<String>), EngineError>;
// = let l = locate_run_shell(&ctx)?; Ok(build_engine(ctx, l.registry, l.run_shell, l.env, provider))
```

- [ ] **Step 1: Implement it.** It runs `build_registry(ctx)`, then `locate(ctx.env.clone(), &registry)`, and turns `RunShell::Unreadable { marker, detail }` into `EngineError::RunShellUnreadable { marker, detail }`. `locate`, `build_registry` and `build_engine` keep their signatures for the held callers.
- [ ] **Step 2: Use both in `run_command`, locating the run shell exactly once per invocation.** `detect_run_shell` reads the marker from disk, so a second lookup could see a different marker.
  - **`shell-init` and `completions`** call `locate_run_shell(&ctx)`. On `Err(e)` they fail exactly as today, with `e.kind()` and `e.to_string()` through `fail`. `shell-init` uses `located.registry`. Both return without an engine.
  - **Every other command** goes straight to `engine_for(ctx, provider_flag.as_ref())`, after copying out the `ctx` fields it copies today. On `Err(e)`, the unreadable marker, it fails exactly as the refusal did, before the `--provider` check and dispatch, as today.
  - **The order of answers stays as it is:**
    - `statusline` and `tui`, before any lookup;
    - the refusal of an unreadable marker before anything else runs;
    - `shell-init` and `completions` before any engine.
- [ ] **Step 3: Verify.** Run the `tagteam` focused command (the unreadable-marker tests in `tests/cli.rs`) and both clippy variants.
- [ ] **Step 4: Commit** "Find the run shell through one locate_run_shell".

### Task 4: Report, warn and note through `Failure::report` and `Io`

**Depends on:** Task 3.

**Files:**
- Modify:
  - `crates/tagteam/src/app/failure.rs`;
  - `app/context.rs` (`Io`'s methods and their tests);
  - `app/mod.rs` (`run_command`'s report);
  - `app/early.rs` (`run_statusline`'s report);
  - the files holding the twelve `warning:` sites and two `note:` sites;
  - `crates/tagteam/src/lib.rs` (the three pre-`Io` sites).

**Interfaces:**
- Produces:

```rust
// failure.rs
impl Failure { pub(super) fn report(self, io: &mut Io<'_>, json: bool, signals: bool) -> Ended; }
pub(crate) fn fail_to(out: &mut dyn Write, err: &mut dyn Write, json: bool, kind: &str, message: &str) -> i32;
// context.rs
impl Io<'_> {
    pub(crate) fn warn(&mut self, warning: &str);          // writes exactly "warning: {warning}\n" to err
    pub(crate) fn note(&mut self, note: &str);             // writes exactly "note: {note}\n" to err
    pub(crate) fn fail(&mut self, json: bool, kind: &str, message: &str) -> i32; // = fail_to(out, err, ..)
}
pub(crate) fn note_to(err: &mut dyn Write, note: &str);    // for the split-borrow site
```

Facts: `bootstrap-reporting-small.md` §2.a–§2.c.

- [ ] **Step 1: Write the test** `io_warn_and_note_write_one_prefixed_line_each` (Review Focus 1). Assert that `warn("x")` writes exactly `b"warning: x\n"` and `note("y")` writes exactly `b"note: y\n"` to `err`, and nothing to `out`. Run it. Expected: FAIL, because the methods do not exist.
- [ ] **Step 2: `report`.**
  - With `signals: true`, it is `run_command`'s match today: a signal gives `Ended::Interrupted`.
  - With `signals: false`, the signal check is skipped, which is `run_statusline`'s behaviour today. `run_statusline` maps `Ended::Code(c)` to `c`.
  - Both call sites use it. `fail` and `fail_purge` stay as the leaf writers; `fail` becomes `Io::fail`, delegating to `fail_to`.
- [ ] **Step 3: `lib.rs`.**
  - `refuse_then_init_logging`'s refusal and `Context::from_process`'s failure call `fail_to(out, err, json, kind, &message)`.
  - The JSON usage error calls `fail_to(&mut stdout, &mut std::io::sink(), true, app::KIND_USAGE, &message)` and still returns `EXIT_USAGE`.
  - `refuse_then_init_logging`'s signature and its unit tests (`lib.rs:234-281`) stay as they are.
- [ ] **Step 4: The `warning:` and `note:` sites.**
  - The twelve `warning:` sites and the `note:` site in `App::notices` use `io.warn` and `io.note`.
  - The `note:` site inside `Daemon::start`'s closure, a split borrow of `io.err`, uses `note_to(err, notice)`.
  - Out of scope, left as they are:
    - the `warning:` and `note:` writes in `run.rs`, `logging.rs` and `lib.rs`'s signal handler;
    - `tokens_cmd.rs`'s pre-formatted notices;
    - the held files;
    - hardening's `prompt.rs` warning.
- [ ] **Step 5: Verify.** Run the Step 1 test (expected: PASS), the `tagteam` focused command (the `tests/*.rs` that assert warning and error text) and both clippy variants.
- [ ] **Step 6: Commit** "Report, warn and note through Failure::report and the Io helpers".

### Task 5: Refuse inside a run shell through the engine's one check

**Depends on:** Task 4.

**Files:**
- Modify: `crates/tagteam-engine/src/engine.rs:321` (`refuse_inside_run_shell` becomes `pub`, with a doc line saying who calls it), and `crates/tagteam/src/app/` at the six refusal sites

**Interfaces:**
- Produces: `App::refuse_in_run_shell(&self) -> Result<(), Failure>`, which is `Ok(self.engine.refuse_inside_run_shell()?)`.

- [ ] **Step 1: Replace the six copies** (`bootstrap-reporting-small.md` §2.d) with `self.refuse_in_run_shell()?;` at the same place in each function:
  - in `lock_check`, after the `--keychain-orphans` usage error;
  - in `daemon`, after `ProgramStamp::take`.

  `auto` keeps its exemption: `if !run.dry_run { self.refuse_in_run_shell()?; }`.
- [ ] **Step 2: Verify.** Run the `tagteam` focused command (the run-shell refusal tests), the engine focused command, and both clippy variants.
- [ ] **Step 3: Commit** "Refuse inside a run shell through the engine's one check".

### Task 6: Write the JSON schema version from one constant

**Depends on:** Task 5.

**Files:**
- Modify: `crates/tagteam/src/lib.rs` (the constant), and the 27 production literals in `app/`, `render.rs`, `service_cmd.rs`, `config_cmd.rs`, `displaced_cmd.rs`, `transfer_cmd.rs`, `purge_cmd.rs`, `doctor_cmd.rs`, `tokens_cmd.rs` and `history.rs`

**Interfaces:**
- Produces: `pub(crate) const SCHEMA_VERSION: u32 = 1;` in `lib.rs`, with the doc `/// The `schemaVersion` every `--json` object carries.`. The path is `crate::SCHEMA_VERSION`.

- [ ] **Step 1: Replace the literals.** Every production `"schemaVersion": 1` in `crates/tagteam/src` (`bootstrap-reporting-small.md` §2.e) becomes `"schemaVersion": SCHEMA_VERSION`.
  - The twelve inside `#[cfg(test)]` modules stay literals, because they pin the wire.
  - `tagteam_engine::store::SCHEMA_VERSION` is a different constant. Never glob-import both.
  - The daemon's literals are out of scope.
- [ ] **Step 2: Verify.**
  - Run the `tagteam` focused command and both clippy variants.
  - Run `rg -n '"schemaVersion": 1' crates/tagteam/src`. Expected: hits only inside test modules.
- [ ] **Step 3: Commit** "Write the JSON schema version from one constant".

### Task 7: Move the completions patching into `completions.rs`

**Depends on:** Task 6.

**Files:**
- Create: `crates/tagteam/src/completions.rs`
- Modify: `crates/tagteam/src/cli.rs` (`:693-895`), `crates/tagteam/src/lib.rs` (`mod completions;`)

**Interfaces:**
- Produces: `impl CompletionShell { pub fn script(self) -> Vec<u8> }` and its private patchers, now in `completions.rs`. `CompletionShell` itself stays in `cli.rs`, because clap's derive names it in `Command`.

- [ ] **Step 1: Move**, byte-identical:
  - `impl CompletionShell` with `script`;
  - `without_hidden_flags`, `fish_positionals`, `fish_global_option_values`, `options_taking_values` and `fish_positional_lines`;
  - the imports only they use (`BTreeSet`, `clap_complete::aot::{Shell, generate}`, and `Write` if only they use it).

  This is a move task: no body edit.
- [ ] **Step 2: Verify.** Run `cargo test -p tagteam --locked --features test-support --test completions` and both clippy variants.
- [ ] **Step 3: Commit** "Move the completions patching into completions.rs".

### Task 8: Collect completion values once, and fail a completions script that cannot be written

**Depends on:** Task 7.

**Files:**
- Modify: `crates/tagteam/src/completions.rs`, `crates/tagteam/src/app/early.rs` (`answer_completions`), `CHANGELOG.md` (Unreleased)
- Test: `crates/tagteam/src/app/early.rs` (tests module)

**Interfaces:**
- Produces, in `completions.rs`:
  - `fn visible_values(arg: &clap::Arg) -> Vec<String>`, which replaces the two identical possible-values collectors;
  - `fn spellings(arg: &clap::Arg) -> Vec<String>`: shorts as `-x`, then longs as `--long`. It replaces the two spellings collectors.

- [ ] **Step 1: Write the test** `a_completions_script_that_cannot_be_written_is_an_io_error` (Review Focus 2).
  - Call `answer_completions` with an `Io` whose `out` always fails to write.
  - Assert that the result is `Ended::Code(EXIT_ERROR)` and that stderr holds the `io` error's `tagteam: …` line.
  - Run it. Expected: FAIL, because the result is `Ended::Code(0)` today.
- [ ] **Step 2: Handle the write.**
  - `answer_completions` reports a failed `write_all` like any command's output write: `Failure::from(EngineError::from(e)).report(io, json, true)`, as `export -` does.
  - A broken pipe (`ErrorKind::BrokenPipe`) is the exception: it still ends with exit 0 and nothing on stderr, as the closed-stdout test in `tests/cli.rs` pins (see delta 9).
- [ ] **Step 3: Extract the collectors.** Use `visible_values` and `spellings` at the four sites (`bootstrap-reporting-small.md` §3.a). The scripts stay byte-identical.
- [ ] **Step 4: The changelog.** Add an entry under `Unreleased`: "`tagteam completions` exits with an error when it cannot write the script, rather than exiting 0 with a partial one; a reader that closes the pipe early is still not an error."
- [ ] **Step 5: Verify.** Run the Step 1 test (expected: PASS), `--test completions`, the `tagteam` focused command, and both clippy variants.
- [ ] **Step 6: Commit** "Collect completion values once and fail a completions script that cannot be written".

### Task 9: Split `main_with_args` into its stages

**Depends on:** Task 8.

**Files:**
- Modify: `crates/tagteam/src/lib.rs` (`main_with_args`, `:109-226`)

**Interfaces:**
- Produces three private functions in `lib.rs`:
  - `fn parse_args(args: &[OsString]) -> Result<cli::Cli, i32>`: the parse, with its JSON usage error;
  - `fn boundary(cli: &cli::Cli) -> Result<(bool, app::Context), i32>`: the stdin drain, `tui`, and `Context::from_process` with its failure;
  - `fn install_signals(ctx: &app::Context)`.

  `refuse_then_init_logging` is unchanged. `main_with_args` calls the four stages in today's order: drain before the signal handlers, the root refusal before logging, and `HOME` before logging.
- [ ] **Step 1: Split.** Each stage's body moves unchanged.
- [ ] **Step 2: Verify.** Run the `tagteam` focused command (`tests/cli.rs`, `tests/signals.rs` and `tests/tui_args.rs` cover the stages) and both clippy variants.
- [ ] **Step 3: Commit** "Split main_with_args into parsing, the process boundary, logging and signals".

### Task 10: Carry the colour inputs as one `ColorEnv`

**Depends on:** Task 9.

**Files:**
- Modify: `crates/tagteam/src/app/context.rs`, `app/mod.rs` (`App`, `run_command`), `app/early.rs` (`run_statusline`)

**Interfaces:**
- Produces, in `context.rs`:

```rust
#[derive(Clone, Copy)]
pub(crate) struct ColorEnv { pub(crate) no_color_flag: bool, pub(crate) no_color_env: bool, pub(crate) force_color_env: bool, pub(crate) stdout_terminal: bool }
impl ColorEnv {
    pub(crate) fn of(ctx: &Context, no_color_flag: bool) -> Self;
    pub(crate) fn enabled(&self, setting: ColorMode) -> bool;  // today's color_enabled rule
}
```

- `color_enabled(flag_off, no_color, force_color, setting, stdout_terminal)` stays, as a wrapper over `ColorEnv { .. }.enabled(setting)`, for the held `tui_cmd.rs`. `Context`'s three fields stay.

- [ ] **Step 1: Use it.**
  - `App` holds one `color: ColorEnv` in place of its four booleans, and `App::color()` reads `self.color.enabled(self.engine.settings().color)`.
  - `run_statusline` takes a `ColorEnv` with `stdout_terminal: true` in place of `no_color` and the copy it makes today.
  - The existing colour test keeps covering the rule through the wrapper.
- [ ] **Step 2: Verify.** Run the `tagteam` focused command and both clippy variants.
- [ ] **Step 3: Commit** "Carry the colour inputs as one ColorEnv".

### Task 11: Rename core's `Strategy` to `AutoStrategy`

**Depends on:** Task 10.

**Files:**
- Modify:
  - `crates/tagteam-core/src/autoswitch.rs` and `crates/tagteam-core/tests/simulate.rs`;
  - in `crates/tagteam-engine/src`: `settings.rs`, `auto.rs` and `doctor/accounts.rs`;
  - `crates/tagteam-daemon/src/auto.rs`;
  - `crates/tagteam/src/app/commands/daemon.rs` (the `auto_flags` hand map);
  - the engine tests that name it (`bootstrap-reporting-small.md` §3.c lists them).

- [ ] **Step 1: Rename.** `tagteam_core::autoswitch::Strategy` becomes `AutoStrategy` everywhere it is that enum.
  - Do not touch `proptest::Strategy` (core `tokens/` and `tests/simulate.rs`'s `impl Strategy<Value = ..>`).
  - Do not touch the held TUI's `host::Strategy`.
  - Do not touch the engine's `UsageStrategy`.
  - The CLI's clap enums `StrategyArg` and `AutoStrategyArg` stay.
- [ ] **Step 2: Verify.**
  - Run the core, engine, daemon and `tagteam` focused commands, and both clippy variants.
  - Run `rg -n 'autoswitch::Strategy\b|\bStrategy::(Best|ConsumeFirst)' crates --glob '!crates/tagteam-tui/**'`. Expected: no hits.
- [ ] **Step 3: Commit** "Rename core's autoswitch Strategy to AutoStrategy".

### Task 12: Read the local time of day through jiff

**Depends on:** Task 11.

**Files:**
- Modify: `crates/tagteam/src/auto.rs` (`local_time`, `:315-334`, and its tests)

**Interfaces:**
- Produces: `pub fn local_time(epoch_s: i64) -> String`, with its signature unchanged, because it is stored as a `fn(i64) -> String`.

- [ ] **Step 1: Write the test** `local_time_of_an_out_of_range_epoch_still_reads_as_a_time` (`local_time(i64::MAX)` has length 8 and does not panic). Run it. Expected: PASS today; it pins the fallback.
- [ ] **Step 2: Reimplement.**
  - `jiff::Timestamp::from_second(epoch_s)`, then `to_zoned(TimeZone::try_system().unwrap_or(TimeZone::UTC))`, then `HH:MM:SS`.
  - An out-of-range epoch keeps today's `rem_euclid(86_400)` fallback.
  - The `libc::localtime_r` `unsafe` block goes. `local_time_is_a_time_of_day` stays as written.
- [ ] **Step 3: Verify.** Run the `tagteam` focused command and both clippy variants.
- [ ] **Step 4: Commit** "Read the local time of day through jiff".

### Task 13: Type the daemon's mode, waiting reason, outcome and no-switch reason for clients

**Depends on:** Task 12.

**Files:**
- Modify:
  - `crates/tagteam-daemon/src/events.rs` (`Mode`, `WaitingFor`);
  - `crates/tagteam-daemon/src/status.rs` (`OutcomeKind`);
  - `crates/tagteam-core/src/autoswitch.rs` (`NoSwitchReason`, and `Trigger`'s `ALL`);
  - `crates/tagteam-daemon/src/protocol.rs` (`Wire`, `Spelled`, the accessors and predicates).
- Test: the `mod tests` of those files.

**Interfaces:**
- Produces:

```rust
// on each of Mode, WaitingFor, OutcomeKind, NoSwitchReason (and Trigger, which already has parse):
pub const ALL: [Self; N];
pub fn parse(s: &str) -> Option<Self>;   // exact, case-sensitive; ALL.into_iter().find(|v| v.as_str() == s)
// protocol.rs
pub trait Spelled: Sized + Copy { fn parse(s: &str) -> Option<Self>; fn as_str(self) -> &'static str; }
// impl Spelled for Mode, WaitingFor, OutcomeKind, NoSwitchReason, Trigger (delegating to the inherent fns)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wire<T> { Known(T), Other(String) }
impl<T: Spelled> Wire<T> {
    pub fn from_wire(s: &str) -> Self;   // Known when T::parse accepts it, else Other(s.to_owned())
    pub fn as_str(&self) -> &str;        // the spelling either way
    pub fn known(&self) -> Option<T>;
}
impl StatusProvider {
    pub fn mode(&self) -> Wire<Mode>;
    pub fn waiting_for(&self) -> Option<Wire<WaitingFor>>;
    /// The daemon job collects usage for this provider itself: `auto`, `collect`, or `waiting` for
    /// `engine-lock`. False for anything else, an unknown value included.
    pub fn collects_for_itself(&self) -> bool;
    /// The job waits for a newer binary (`waiting` for `schema`). False for anything else.
    pub fn waits_for_upgrade(&self) -> bool;
}
impl StatusOutcome { pub fn kind(&self) -> Wire<OutcomeKind>; }
impl Event {
    /// `Some` only for `Event::Mode`.
    pub fn mode(&self) -> Option<Wire<Mode>>;
    /// `Some` only for an `Event::Mode` that names what it waits for.
    pub fn waiting_for(&self) -> Option<Wire<WaitingFor>>;
}
```

Facts: `protocol.md` §1, §2 and §4a.

- [ ] **Step 1: Write the tests.** Run them, and see them fail to compile.
  - Per enum, in its own file's tests: `every_<enum>_parses_from_its_own_spelling`, over `ALL`. Core's `every_no_switch_reason_is_spelled_in_kebab_case` iterates `ALL` in place of its hand list.
  - In `protocol.rs`:
    - `a_wire_value_keeps_an_unknown_spelling`: `Wire::<Mode>::from_wire("paused")` is `Other("paused")`, its `as_str()` is `"paused"`, and `known()` is `None`; `"auto"` is `Known(Mode::Auto)`.
    - `the_mode_predicates_default_to_false_for_unknown_values`: the full table in `protocol.md` §4a, where `auto`/any, `collect`/any and `waiting`/`engine-lock` give `collects_for_itself`, and only `waiting`/`schema` gives `waits_for_upgrade`. It includes an unknown mode, an unknown `waitingFor`, and `waiting` with none.
- [ ] **Step 2: Implement.** The `String` fields stay, and no struct gains a field.
- [ ] **Step 3: Verify.**
  - Run the daemon and core focused commands, and both clippy variants.
  - Run `cargo build -p tagteam-tui --locked`. The held TUI must compile unchanged.
- [ ] **Step 4: Commit** "Type the daemon's mode, waiting reason, outcome and no-switch reason for clients".

### Task 14: Parse the auto events' payloads, write `event_json` through `envelope`, and read no-switch reasons by parsing

**Depends on:** Task 13.

**Files:**
- Modify: `crates/tagteam-daemon/src/protocol.rs`, `crates/tagteam/src/auto.rs` (the `NoSwitch` arm, `:232-251`)
- Test: `protocol.rs` tests, `crates/tagteam/src/auto.rs` tests

**Interfaces:**
- Produces, in `protocol.rs`:

```rust
pub struct ActiveAccount { pub number: u32, pub email: String }
pub struct PollPayload {
    pub active: Option<ActiveAccount>,
    pub headroom_pct: BTreeMap<u32, Option<f64>>,           // empty when absent
    pub threshold: Option<f64>,
    pub fetch_errors: BTreeMap<u32, String>,                // empty when absent
    pub windows_pct: BTreeMap<u32, Vec<(String, f64)>>,     // per position, in wire key order; empty when absent
}
pub struct BindingWindow { pub window: String, pub used_pct: i64 }
impl PollPayload { pub fn binding(&self, position: u32) -> Option<BindingWindow>; }
pub struct SwitchPayload { pub trigger: Wire<Trigger>, pub from: u32, pub to: u32, pub warnings: Vec<String>, pub dry_run: bool }
pub struct NoSwitchPayload { pub reason: Wire<NoSwitchReason>, pub detail: String }
pub fn parse_poll(raw: &Value) -> Option<PollPayload>;
pub fn parse_switch(raw: &Value) -> Option<SwitchPayload>;     // None without `from` or `to`
pub fn parse_no_switch(raw: &Value) -> Option<NoSwitchPayload>;
impl Event {
    pub fn poll(&self) -> Option<PollPayload>;           // Event::Auto with kind "poll" only
    pub fn switch(&self) -> Option<SwitchPayload>;
    pub fn no_switch(&self) -> Option<NoSwitchPayload>;
}
```

- **Leniency** matches the TUI's reader today, `protocol.md` §4b:
  - a missing `trigger`, `reason` or `detail` reads as `""`;
  - a missing `dryRun` reads as `false`, and missing `warnings` as empty;
  - a `null` or absent `headroomPct` entry is `None`.

  Numbers are read with `as_f64()` and `as_u64()`, never deserialised into `f64` fields, because `arbitrary_precision` is on.
- **`binding(position)`:**
  - `None` when `headroom_pct[position]` is unknown.
  - Otherwise usage is `100 - headroom`. The window is the first in `windows_pct[position]`'s wire order whose percentage is strictly within 0.05 of the usage.
  - `used_pct` is `(100 - headroom).round()`.

- [ ] **Step 1: Write the tests.**
  - `each_auto_payload_parses_from_event_json_s_own_output`: feed `event_json(&e, T0)` for the `auto_lines()` events into the parsers, and compare to the source `AutoEvent` fields, including a poll without `fetchErrors` or `windowsPct`.
  - `the_binding_window_is_the_first_within_the_tolerance` (Review Focus 4):
    - two windows within 0.05 give the first in key order;
    - 0.049 away matches and 0.051 does not;
    - a `null` headroom and a missing `windowsPct` give `None`.
  - In `crates/tagteam/src/auto.rs`, `an_unknown_no_switch_reason_is_still_printed` (Review Focus 5): a `no_switch_event("some-new-reason", "")` prints `no switch: some-new-reason`.
  - Run them. Expected: compile failure for the parsers; the auto test passes today, as a pin.
- [ ] **Step 2: Implement the parsers.**
- [ ] **Step 3: `envelope`.** `event_json`'s hand-built envelope (`protocol.rs:118-129`) becomes `envelope(kind, Some(provider), fields, now_s)`. `every_auto_kind_is_written_byte_for_byte_as_auto_wrote_it_before` is the check.
- [ ] **Step 4: The CLI.** `tagteam/src/auto.rs`'s `NoSwitch` arm matches on `NoSwitchReason::parse(reason)`:
  - `Some(EngineRunning)` gives the warning line;
  - `Some(AllExhausted)` prints nothing;
  - anything else, `None` included, prints `no switch: …` as today.
- [ ] **Step 5: Plan-local references (spec §14).** `protocol.rs:3` ("Plan B adds…"), `:1176` ("(Task 3)") and `:1508` ("(Task 2)") are rewritten as the rule they stood for. Comments only.
- [ ] **Step 6: Verify.** Run the daemon and `tagteam` focused commands, both clippy variants, and `cargo build -p tagteam-tui --locked`.
- [ ] **Step 7: Commit** "Parse the auto events' payloads, write event_json through envelope, and read no-switch reasons by parsing".

### Task 15: Rewrite the app's "Decision N" references as the rules they stood for

**Depends on:** Task 14.

**Files:**
- Modify: the files under `crates/tagteam/src/app/` that hold the 27 references (old `app.rs`), plus the two in `crates/tagteam/src/lib.rs` and the one in `cli.rs` only where Tasks 7 and 9 moved that code

- [ ] **Step 1: Rewrite.**
  - For each reference, find the plan that numbers it (`rg -n "Decision <N>" docs/superpowers/plans`, matched by subject; "plan B Decision 6" names the daemon's plan B).
  - Rewrite it as the rule in words, keeping any spec § reference.
  - Comments only. Lines wrap at 100 columns.
- [ ] **Step 2: Verify.** Run `rg -n 'Decision [0-9]' crates/tagteam/src/app` (expected: no hits) and `cargo fmt --all --check`.
- [ ] **Step 3: Commit** "Rewrite the app's Decision references as the rules they stood for".

## Final verification (the controller, before the PR)

- **macOS, outside the sandbox:** `cargo fmt --all --check`, both clippy commands, and `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`. Report the totals as passed/total.
- **Linux:** the same `cargo test` command, in the Docker reproduction (`rust:1.88`, run as a non-root user with a passwd entry, `HOME` outside `/tmp`), against the branch head.
- **The held crates:** `cargo build -p tagteam-tui --locked` and `git diff --stat <R2 head>..HEAD -- crates/tagteam-tui crates/tagteam/src/tui_host.rs crates/tagteam/src/tui_cmd.rs`, which must be empty.

## Spec deltas found while planning

These are rulings inside the spec's intent. Each one is recorded under "Rulings" in the PR.

1. **The exhaustive dispatch is decided by Task 0's spike**, under §7.1's own rule.
   - **The expected outcome** is the fallback. clap orders subcommands by insertion, and `flatten` inserts a group where its variant sits. The five early commands are interleaved: `Tui` is 3rd, `ShellInit`, `Statusline` and `Run` are 18th–20th, and `Completions` is 27th.
   - **Either way,** the PR records the observed result.
2. **`run_command` uses both bootstrap functions,** and each invocation locates the run shell once.
   - `shell-init` and `completions` need the registry but no engine, so they call `locate_run_shell`.
   - Every engine-backed command calls `engine_for`.
   - The held TUI files keep their own sequence (§7.2).
3. **`Failure::report` takes `json`.** The spec's `Output` type does not exist; `Io` is the output and carries no `json` flag.
4. **`auto_command` takes `(once, dry_run, flags)`.** The `Auto` arm keeps building `AutoFlags` through `auto_flags`, which therefore becomes `pub(in crate::app)`. That keeps the method within clippy's limit.
5. **`AddToken` becomes a method as well,** so `confirming` and `added` stay private to `commands/accounts.rs`. §7.1 does not list it, but the move would otherwise widen their visibility. `Disable` and `Enable` stay inline.
6. **`or_inactive`'s log target is pinned before the move** (Task 1), as R2 did for the switch.
7. **The `SCHEMA_VERSION` path is `crate::SCHEMA_VERSION`** (`u32`, in `lib.rs`), named apart from the engine's store `SCHEMA_VERSION` by its crate. Test-module literals stay literals, because they pin the wire.
8. **`Io::warn` and `Io::note` cover the twelve `warning:` and two `note:` sites in the app.** The writers in `run.rs` and `prompt.rs` (hardening's files), `logging.rs`, `lib.rs`'s signal handler and `tokens_cmd.rs`'s pre-formatted notices take bare writers or pre-built text, so they stay.
9. **A completions write that fails reports like `export -`, except a broken pipe.** A reader that closes the pipe early still exits 0, as `tests/cli.rs`'s closed-stdout test pins for `list` and `completions` alike. This was ruled during execution, superseding the first wording, "a broken pipe included".
10. **`Wire<T>` parses through a local `Spelled` trait,** implemented for the four enums and `Trigger`. `ALL` is a `[Self; N]` array.
11. **`Event`'s mode accessors return `Option`,** being `None` for other variants. `StatusOutcome` gets only `kind()`, as the spec lists.
12. **The payload parsers are as lenient as the TUI's reader today.** `windows_pct` keeps wire key order, because the binding rule takes the first match.
13. **`client.rs` is not edited.** Hardening changes it, and §7.5 needs nothing there.
