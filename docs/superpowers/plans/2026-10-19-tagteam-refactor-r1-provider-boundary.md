# tagteam refactor R1 — provider boundary, step 1 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Approved
**Goal:** Move every Claude Code fact out of the neutral crates and the binary's non-root modules into `tagteam-cc` or provider-supplied data, give the binary one composition root, and add a test that keeps both, with Claude Code's output byte-identical except §11's two R1 changes.
**Architecture:** Neutral code reaches provider facts through four new `Provider` methods: `vars()` and `words()` (two static data structs), `statusline_setup()` and `foreign_export()`. It also gets a neutral `Env.homes` map, which `tagteam-cc` reads and writes through its own accessors. `app::registry` is the only code that names `ClaudeCode`, and the statusline and completions build their registries through it. `tagteam-fake/tests/neutrality.rs` scans production source text so none of this can regress.
**Tech Stack:** Rust 1.88 (edition 2024), cargo workspace; clap, serde_json, thiserror; tests with the workspace's fakes.
**Spec:** docs/superpowers/specs/2026-10-19-tagteam-review-refactor-design.md (§5)

**Reading this plan.**
- Line numbers are as of `7b27210`. They drift as tasks land, so find each site by the code quoted.
- Focused test commands use each crate's own test feature:
  - `--features test-hooks` for `tagteam-cc`, `tagteam-engine` and `tagteam-daemon`;
  - `--features test-support` for `tagteam`;
  - none for `tagteam-provider`, `tagteam-core`, `tagteam-fake` and `xtask`.
- Each task leaves the workspace compiling, with its crates' tests green.

## Global Constraints

- **Byte-identical output.** Claude Code's user-visible output stays byte-identical: every message, JSON field, exit code and help line. The two exceptions are §11's R1 rows: clap help for provider-generic behaviour drops "Claude Code" (C-4, Task 15), and `default_provider`'s description drops its `such as "claude-code"` example (C-9, Task 9). The existing tests that assert text are the check, and none of them is loosened.
- **No wire or store change.** The daemon protocol's JSON, the `--json` schemas and the store schema stay as they are, with no migration.
- **Hardening-owned files are not edited:**
  - in `tagteam-provider`: `src/security.rs`, `src/process.rs`, `src/keychain.rs`, the fakes (`FakeKeychain`, `FakeClock`, `ScriptedHttp`, `FakeProcessProbe`, `ScriptedSpawner`) and `Env::for_test`'s gating;
  - in `tagteam-engine`: `src/refresh.rs`, `src/store/tokens.rs` and `src/tokens/*`;
  - the loops in `tagteam-daemon/src/server.rs`, `notify.rs` and `scheduler.rs`;
  - `.github/workflows/*` and `dist-workspace.toml`.

  There are three sanctioned exceptions. `tagteam-provider/src/lib.rs` only gains re-exports and `pub mod` lines, each on a line of its own. `Env::for_test`'s body changes in Task 5. `server.rs` gains one `#![allow(unsafe_code)]` line at its top, outside the loops (spec §5.8, Task 2).
- **Held TUI files are edited only to compile.** They are `crates/tagteam-tui/**`, `crates/tagteam/src/tui_host.rs` and `crates/tagteam/src/tui_cmd.rs`.
  - R1's only edits are one import line each in `crates/tagteam-tui/src/testutil.rs` and `crates/tagteam-tui/tests/source.rs` (Task 14).
  - `app::build_registry`, `app::locate`, `app::build_engine`, `app::color_enabled`, `app::run`, `app::UNLOCK_QUESTION` and `app::EXIT_*` keep their signatures.
- **Pure moves are separate from edits.** Tasks 1, 2 and 12 are move tasks. Task 13 moves `read_cswap_v1` with a body that is byte-identical apart from its `use` lines, so `git diff --color-moved` shows it as moved. No behaviour edit goes into a move task.
- **CI commands** (from `CONTRIBUTING.md`):
  - `cargo fmt --all --check`;
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`;
  - `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`.
- **No agent attribution** in commits: no `Co-authored-by:` agent line, no session link, no "Generated with" footer.
- **Commit messages** are imperative and in sentence case, with no prefix and no trailing period, matching `git log` (for example "Show the index's warnings under an empty token report"). Each task's message is given in its last step.
- **No test is weakened.** A test edit is one this plan lists: §5.3's list, a mechanical signature change, a held-file compile fix, or a new test.

## Review Focus

These are the five input classes or failure modes the spec implies but that no existing test exercises, most likely to bite a user first. Each one's test is added to the task that owns it.

1. **`CLAUDE_SECURESTORAGE_CONFIG_DIR` reaching `Env.homes` at the process boundary.**
   - Every binary-level test either builds `Context` by hand or sets only `CLAUDE_CONFIG_DIR`.
   - If `home_vars` dropped the secure-storage variable, every Keychain item for a user who sets it would be named wrong, and `add`, `switch` and `statusline` would read or write the wrong item.
   - Test: Task 5, `the_binary_names_the_live_item_from_the_secure_storage_dir_it_was_started_with` (`crates/tagteam/tests/cli.rs`).
2. **The ownership refusals and doctor lines for a profile a Claude Code background daemon owns,** now assembled from `ProviderWords`.
   - Today these are pinned only by `contains("background daemon")` and `contains("claude daemon stop --any")`.
   - A broken article, separator or order would ship.
   - Tests: Task 7, `the_ownership_texts_name_the_provider_s_supervisor_and_launch_command` (`engine/src/error.rs`) and `the_daemon_check_and_a_held_reservation_name_the_agent` (`engine/src/doctor.rs`).
3. **The managed-key restart hint,** now chosen in the engine from `kind_traits(kind).managed_key_axis` and no longer from CC's kind string in `render.rs`.
   - "Restart Claude Code to apply." has no end-to-end pin today; `render.rs`'s own tests built the outcome by hand.
   - Tests: Task 6, `the_activation_hint_is_the_provider_s_words_for_the_axis_and_the_store` and `a_fake_agent_switch_takes_its_hint_from_fake_agent` (`engine/tests/switch.rs`).
4. **The config lock's "frees itself" hint and each lock's role,** moved from path-sniffing in `mkdir_lock.rs` to per-spec data that cc sets.
   - A hint on the wrong lock, or none on `wait_config_idle`, would give users wrong advice on every config-lock timeout.
   - Only `mkdir_lock.rs`'s own unit tests pin them today, and those tests move.
   - Test: Task 4, `each_lock_names_its_role_and_only_the_config_lock_says_it_frees_itself` (`cc/src/locks.rs`).
5. **A panic in one thread poisoning the daemon's long-lived `LiveStore`.**
   - The daemon shares one `ClaudeCode` across threads. A poisoned `seen` ledger would make every later switch panic until restart.
   - Test: Task 3, `a_poisoned_ledger_still_ends_the_operation` (`cc/src/live.rs`).

Further byte-identity pins added on the way:
- `UnknownProvider`'s CLI text (Task 9);
- the held-reservation doctor text (Task 7);
- `ShadowingItem`'s full text (Task 4);
- `--print-config` through `statusline_setup` (Task 11).

## Spec coverage

| Spec | Tasks |
|---|---|
| §5.1 `Env.homes`, cc accessors, `apply_outer_home` | 5 |
| §5.2 `ProviderVars`, `ProviderWords`, capture list, `recorded_vars`, `activation_hint` | 5 (`homes`), 6 |
| §5.3 messages, `SessionOwner::of`, C-9 | 7 (engine), 8 (CLI), 9 (settings), 11 (`statusline.rs:196`) |
| §5.4 `CLAUDE_CODE` + C-3, `Platform`, lock wording, `ShadowingItem`, `format_iso8601` | 14, 1, 4, 4, 1 |
| §5.5 composition root, completions, sighting home, setup, test probes, `shell_init` | 11 (`shell_init` needs no change, see the deltas) |
| §5.6 cswap via `foreign_export` | 12, 13 |
| §5.7 `ValidateRequest` | 10 |
| §5.8 `user_name`, `forbid(unsafe_code)`, daemon deny/allow, `term.rs`, `lock_unpoisoned` | 2, 3 |
| §5.9 neutrality test | 17 |
| §11 R1 CHANGELOG entries | 9, 15 |
| §14 plan-local comments in R1's files | 16 (and 2 and 4 for the two docs they rewrite) |
| §16 core §4.5 pointer | 13 |

**Order and batching.** Every task changes a crate that every later task's tests compile (`tagteam-provider` or `tagteam-engine`), or shares a file with its predecessor, so there are no parallel batches and the chain is linear. Besides that, a few orders are hard:
- 5 before 11: the sighting home reads `Env.homes`.
- 6 before 7: the supervisor words are in `ProviderWords`.
- 12 before 13 before 14: the engine must stop naming `CLAUDE_CODE` in `read_cswap_v1` before the constant leaves `tagteam-core`, and Task 14 uses `Engine::registry()` from Task 13.
- 11 before 14: `cli.rs`'s `COMPLETED_PROVIDERS` is gone by then.
- 16 and 17 after everything else.

---

### Task 1: Move `Platform` into tagteam-provider and take `format_iso8601` from tagteam-core

**Depends on:** none

**Files:**
- Create: `crates/tagteam-provider/src/platform.rs`, holding the enum and its `impl` from `crates/tagteam-cc/src/live.rs:160-174`, moved verbatim, with a one-line module doc ("The OS a build runs on: it picks the credential store, the notifier and the service manager.").
- Modify: `crates/tagteam-provider/src/lib.rs`. Add `pub mod platform;` between `pub mod mock_server;` and `pub mod process;`, and `pub use platform::Platform;` on a line of its own.
- Modify: `crates/tagteam-cc/src/live.rs:160-174`. Replace them with `pub use tagteam_provider::Platform;`, which keeps `tagteam_cc::live::Platform` for cc's tests and every other crate's.
- Modify: `crates/tagteam/src/app.rs:10`. Drop the line and add `Platform` to the `use tagteam_provider::{…}` at `:51`.
- Modify: `crates/tagteam/src/displaced_cmd.rs:5`, `history.rs:5`, `logging.rs:10` and `render.rs:3`. Each becomes `use tagteam_core::time::format_iso8601;`.
- Test: no new test; the existing suites below prove the move.

**Interfaces:**
- Produces `tagteam_provider::Platform` (`enum Platform { MacOs, Linux }`, `pub fn current() -> Self`). `tagteam_cc::live::Platform` stays the same type through the re-export.
- Consumes `tagteam_core::time::format_iso8601(epoch_s: i64) -> String`, which already exists (`time.rs:117`).

- [ ] **Step 1: Confirm the existing tests that prove the move.** These suites use `Platform` from both paths and must stay green unchanged:
  - `tagteam/src/app.rs` `the_test_overrides_exist_only_with_test_support`;
  - `tagteam-cc/src/naming.rs` `the_live_item_spelling_is_what_its_name_hashes` (`crate::live::Platform::MacOs`);
  - `engine/tests/switch.rs` `a_switch_reports_the_linux_file_as_its_only_store` (`tagteam_cc::live::Platform::Linux`);
  - `engine/tests/purge.rs`, `tagteam-tui/tests/source.rs` and `tagteam-daemon/src/testutil.rs`, which import `tagteam_cc::live::Platform`.
- [ ] **Step 2: Move.** Make exactly the edits listed in Files: nothing else in `live.rs`, and no new derive.
- [ ] **Step 3: Build everything.** Run `cargo build --workspace --all-targets --features tagteam/test-support`. Expected: success.
- [ ] **Step 4: Run the focused suites.**
  - `cargo test -p tagteam-cc --features test-hooks --lib`
  - `cargo test -p tagteam --features test-support --lib app::tests`
  - `cargo test -p tagteam-engine --features test-hooks --test switch a_switch_reports_the_linux_file_as_its_only_store`

  Expected: PASS.
- [ ] **Step 5: Check that it reads as a move.** `git diff --color-moved=dimmed-zebra crates/tagteam-cc/src/live.rs crates/tagteam-provider/src/platform.rs` should show the enum and its `impl` as moved lines only.
- [ ] **Step 6: Commit (controller).** Message: `Move Platform into tagteam-provider and take format_iso8601 from tagteam-core`

---

### Task 2: Move the passwd lookup into tagteam-provider and forbid unsafe code in tagteam-cc

**Depends on:** Task 1 (`tagteam-provider/src/lib.rs`)

**Files:**
- Create: `crates/tagteam-provider/src/passwd.rs`. Move `passwd_name` from `crates/tagteam-cc/src/naming.rs:46-85` into it, renamed `user_name` and made `pub`.
  - Its doc keeps the rule and drops the plan reference: "The effective user's passwd name, through the re-entrant `getpwuid_r`: tagteam's threads may resolve Keychain names at the same time, and `getpwuid` answers into one shared static buffer."
  - The test moves too, from `naming.rs:117-131`.
- Modify: `crates/tagteam-provider/src/lib.rs`. Add `pub mod passwd;` on its own line, between `pub mod mock_server;` and `pub mod platform;`.
- Modify: `crates/tagteam-cc/src/naming.rs`:
  - delete `:1` (`use std::ffi::CStr;`), `:46-85` and `:117-131`;
  - `:94` becomes `.or_else(tagteam_provider::passwd::user_name)`.
- Modify: `crates/tagteam-cc/src/lib.rs`. Add `#![forbid(unsafe_code)]` as line 1.
- Modify: `crates/tagteam-daemon/src/lib.rs`. Add `#![deny(unsafe_code)]` after the `//!` docs (`:1-3`).
- Modify: `crates/tagteam-daemon/src/server.rs`. Add `#![allow(unsafe_code)]` as line 1. This is a one-line hunk outside the loops hardening owns.
- Modify: `crates/tagteam-provider/src/term.rs:1-3`. The module doc becomes, exactly:
  ```rust
  //! The terminal, descriptor and job-control calls that the development harness (`xtask`) and
  //! the TUI (`tagteam-tui`) need, as safe functions. Both crates forbid `unsafe` and call these.
  //! Only `tagteam-provider`, the `tagteam` binary and the daemon's `server.rs` may use `unsafe`;
  //! every other crate forbids it, and the daemon denies it everywhere else.
  ```
- Test: `crates/tagteam-provider/src/passwd.rs` (`the_user_name_is_this_user_s_from_any_thread`).

**Interfaces:**
- Produces `pub fn user_name() -> Option<String>` in `tagteam_provider::passwd`.

- [ ] **Step 1: Write the failing test.** Create `passwd.rs` with the moved test only, and add `pub mod passwd;`.
  - It must hold `crate::fork_guard()`, because `Command::new("id")` forks (see `lib.rs:24-43`).

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn the_user_name_is_this_user_s_from_any_thread() {
          let _fork = crate::fork_guard();
          let id = std::process::Command::new("id").arg("-un").output().unwrap();
          let expected = String::from_utf8(id.stdout).unwrap().trim().to_owned();
          let names: Vec<Option<String>> = std::thread::scope(|s| {
              let handles: Vec<_> = (0..8).map(|_| s.spawn(user_name)).collect();
              handles.into_iter().map(|h| h.join().unwrap()).collect()
          });
          for name in names {
              assert_eq!(name.as_deref(), Some(expected.as_str()));
          }
      }
  }
  ```
- [ ] **Step 2: Run it.** `cargo test -p tagteam-provider --lib passwd::tests::the_user_name_is_this_user_s_from_any_thread`. Expected: FAIL to compile, "cannot find function `user_name`".
- [ ] **Step 3: Move the function** from `naming.rs`, making the naming edits listed in Files.
- [ ] **Step 4: Run it again** (the same command). Expected: PASS.
- [ ] **Step 5: Add the lint attributes and the corrected comment.**
  - Run `cargo build -p tagteam-cc -p tagteam-daemon --all-targets`. Expected: success; `forbid` would fail it if any `unsafe` were left in cc.
  - `rg -n 'unsafe' crates/tagteam-cc/src` should print nothing.
- [ ] **Step 6: Run the focused suites.**
  - `cargo test -p tagteam-cc --features test-hooks --lib naming::tests`
  - `cargo test -p tagteam-daemon --features test-hooks --lib`
  - `cargo test -p tagteam-provider --lib`

  Expected: PASS. In particular, `keychain_account`'s `$USER`-else-passwd tests are unchanged.
- [ ] **Step 7: Commit (controller).** Message: `Move the passwd lookup into tagteam-provider and forbid unsafe code in tagteam-cc`

---

### Task 3: Take poisoned locks over in the lock heartbeat and in Claude Code's ledger

**Depends on:** Task 2 (`tagteam-provider/src/lib.rs`)

**Files:**
- Create: `crates/tagteam-provider/src/poison.rs`, holding `lock_unpoisoned` and its unit test.
- Modify: `crates/tagteam-provider/src/lib.rs`. Add `pub mod poison;` on its own line.
- Modify: `crates/tagteam-provider/src/mkdir_lock.rs`:
  - `:320`, `:336`, `:383` and `:403` call `lock_unpoisoned(&…)`;
  - `:329` becomes `.wait_timeout_while(stop, every, |s| !*s).unwrap_or_else(PoisonError::into_inner)`;
  - add a new unit test.
- Modify: `crates/tagteam-cc/src/live.rs`:
  - the eight `self.seen.lock().unwrap()` calls at `:408, 456, 470, 513, 533, 823, 1082, 1186` call `lock_unpoisoned(&self.seen)`;
  - append a `#[cfg(test)] mod tests` with one test.
- Riders (spec §5.8, optional): none. Every `lock().unwrap()` in `cc/locks.rs`, `provider.rs`, `atomic.rs`, `transfer_cmd.rs` and `logging.rs` is in unit-test code (see the deltas).

**Interfaces:**
- Produces `pub fn lock_unpoisoned<T: ?Sized>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T>` in `tagteam_provider::poison`. Its doc: "Takes `m`, ignoring poison: a holder that panicked left the value as it was, and a long-lived process must not die of it."

- [ ] **Step 1: Write the failing tests.**

  In `poison.rs`:
  ```rust
  #[test]
  fn a_poisoned_mutex_still_hands_over_its_value() {
      let m = std::sync::Mutex::new(7);
      let _ = std::thread::scope(|s| {
          s.spawn(|| {
              let _held = m.lock().unwrap();
              panic!("poisoned on purpose");
          })
          .join()
      });
      assert!(m.is_poisoned());
      assert_eq!(*lock_unpoisoned(&m), 7);
  }
  ```
  In `mkdir_lock.rs`'s `tests` (it uses the module's `spec`, `mtime` and `eventually`):
  ```rust
  #[test]
  fn a_poisoned_timestamp_lock_neither_stops_the_heartbeat_nor_fails_the_check() {
      let d = tempfile::tempdir().unwrap();
      let s = spec(d.path(), 60_000, 100, 20);
      let l = MkdirLock::acquire(&s).unwrap();
      let state = l.state.clone();
      let _ = std::thread::spawn(move || {
          let _held = state.last_set.lock().unwrap();
          panic!("poisoned on purpose");
      })
      .join();
      assert!(l.state.last_set.is_poisoned());
      let before = mtime(&s.path);
      assert!(
          eventually(Duration::from_secs(5), || mtime(&s.path) > before),
          "the heartbeat still beats"
      );
      assert!(l.check_owned().is_ok());
      drop(l);
      assert!(!s.path.exists(), "released as ever");
  }
  ```
  In `cc/src/live.rs`, a new module at the end:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use tagteam_provider::FakeKeychain;

      #[test]
      fn a_poisoned_ledger_still_ends_the_operation() {
          let store = LiveStore::new(Arc::new(FakeKeychain::new()), Platform::MacOs);
          let _ = std::thread::scope(|s| {
              s.spawn(|| {
                  let _held = store.seen.lock().unwrap();
                  panic!("poisoned on purpose");
              })
              .join()
          });
          assert!(store.seen.is_poisoned());
          store.end_operation();
          store.end_operation();
      }
  }
  ```
- [ ] **Step 2: Run them, each alone.**
  - `cargo test -p tagteam-provider --lib poison::tests`. Expected: FAIL to compile (no `lock_unpoisoned`). Then create the function as `todo!()` and rerun the next two:
  - `cargo test -p tagteam-provider --lib mkdir_lock::tests::a_poisoned_timestamp_lock_neither_stops_the_heartbeat_nor_fails_the_check`. Expected: FAIL. The heartbeat thread dies on `last_set.lock().unwrap()`, so the assertion "the heartbeat still beats" fails, or the run aborts on `check_owned`'s second panic while unwinding.
  - `cargo test -p tagteam-cc --features test-hooks --lib live::tests::a_poisoned_ledger_still_ends_the_operation`. Expected: FAIL, panicking with `PoisonError` from `end_operation`.
- [ ] **Step 3: Implement.** Write `lock_unpoisoned` as `m.lock().unwrap_or_else(PoisonError::into_inner)` and make the call-site edits listed in Files.
- [ ] **Step 4: Run the three tests again.** Expected: PASS.
- [ ] **Step 5: Run the focused suites.** `cargo test -p tagteam-provider --lib mkdir_lock` and `cargo test -p tagteam-cc --features test-hooks`. Expected: PASS, including `checks_racing_the_heartbeat_never_see_a_false_takeover` and `the_heartbeat_keeps_the_mtime_fresh`.
- [ ] **Step 6: Commit (controller).** Message: `Take poisoned locks over in the lock heartbeat and in Claude Code's ledger`

---

### Task 4: Let each lock carry its role and timeout hint, and the shadowing error its agent

**Depends on:** Task 3 (`mkdir_lock.rs`, `cc/src/live.rs`)

**Files:**
- Modify: `crates/tagteam-provider/src/mkdir_lock.rs`:
  - `:12-41`: `LockError::Timeout(TimedOut)` with `#[error("{0}")]`. Add `TimedOut`, its `Display` and `From<PathBuf>`, and delete `timeout_text`.
  - `:65-94`: `MkdirLockSpec` gains `pub role: &'static str` and `pub timeout_hint: Option<&'static str>`. `new` sets `role: "a lock directory"` and `timeout_hint: None`. Add the builders `with_role` and `with_timeout_hint`.
  - `:117-149`: delete `lock_role`. `remove_if_ours` and `warn_if_left` take the role.
  - `State` keeps `role` for the heartbeat's WARN (`:350`), and `MkdirLock` gains `pub fn role(&self) -> &'static str`.
  - `:247` and `:272` build `TimedOut { path: spec.path.clone(), hint: spec.timeout_hint }`.
  - The doc comments at `:25-27`, `:125-127` and `:138-140` lose their Claude Code wording.
  - Unit tests:
    - `spec()` (`:567`) sets `role: "the test lock", timeout_hint: None`;
    - `:663` becomes `LockError::Timeout(p.clone().into())`;
    - `:739` becomes `Err(LockError::Timeout(t)) if t.path == s.path`;
    - `both_spellings_of_the_global_config_s_lock_are_the_config_lock` (`:753`) is deleted; its property moves to cc;
    - `a_config_lock_timeout_says_it_frees_itself_and_other_locks_do_not` (`:767`) is replaced by `a_timeout_carries_its_lock_s_hint_and_only_that`;
    - the two `logged` tests (`:1082`, `:1117`) expect `"could not remove the test lock"`, and the `lock_role` asserts at `:1100-1112` are deleted (moved to cc).
- Modify: `crates/tagteam-provider/src/lib.rs`. Add `pub use mkdir_lock::TimedOut;` on its own line.
- Modify: `crates/tagteam-provider/src/flock.rs:119`. It becomes `LockError::Timeout(path.to_path_buf().into())`.
- Modify: `crates/tagteam-provider/src/provider.rs:369-376`. `ShadowingItem { item: String, agent: &'static str }` with `#[error("the Keychain item {item} could not be verified gone, so {agent} may still read it")]`. Its doc drops "Claude Code" and "(L397)". Add a unit test.
- Modify: `crates/tagteam-cc/src/locks.rs`:
  - add the role and hint constants (below);
  - the six production constructors (`:55`, `:58`, `:98`, `:121`, `:154`, `:162`) add `.with_role(…)`, and the config lock's two (`:98`, `:121`) also add `.with_timeout_hint(CONFIG_HINT)`;
  - `:64` becomes `LockError::Timeout(legacy_spec.path.into())`;
  - tests `:534` and `:562` bind `Timeout(t)` and compare `t.path`;
  - add a new test.
- Modify: `crates/tagteam-cc/src/provider.rs`. Add `pub(crate) const DISPLAY_NAME: &str = "Claude Code";`, which `display_name()` (`:253`) returns.
- Modify: `crates/tagteam-cc/src/live.rs:687` and `:1049`. Each becomes `ProviderError::ShadowingItem { item: svc, agent: crate::provider::DISPLAY_NAME }`.
- Modify: `crates/tagteam-engine/src/recover.rs:147`. `ProviderError::Lock(LockError::Timeout(t)) => EngineError::RecoveryBlocked { …, lock: t.path }`.
- Modify: `crates/tagteam-engine/src/error.rs` unit tests at `:546`, `:561`, `:814`, `:907` and `:937`. Each becomes `LockError::Timeout(PathBuf::from("x").into())` (§5.3's list).
- Test, edited for the new shapes: `crates/tagteam-cc/tests/live_store.rs:620` (`ShadowingItem { .. }`), `:654-659` (destructure, plus a full-text pin), `:1594` (`t.path`); `crates/tagteam-cc/tests/session.rs:489` (`ShadowingItem { item, .. }`), `:689` and `:793` (`t.path`).

**Interfaces:**
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedOut { pub path: PathBuf, pub hint: Option<&'static str> }
// Display: "timed out waiting for the lock {path}", then "; {hint}" when there is one.
impl From<PathBuf> for TimedOut   // hint: None
impl MkdirLockSpec {
    pub fn with_role(self, role: &'static str) -> Self;
    pub fn with_timeout_hint(self, hint: &'static str) -> Self;
}
impl MkdirLock { pub fn role(&self) -> &'static str; }
// tagteam-cc/src/locks.rs
const REFRESH_ROLE: &str = "Claude Code's refresh lock";
const CREDENTIAL_ROLE: &str = "Claude Code's credential lock";        // the legacy lock, :58
const CONFIG_ROLE: &str = "Claude Code's config lock";
const STORAGE_WRITE_ROLE: &str = "Claude Code's storage-write lock";  // both spellings
const CONFIG_HINT: &str = "Claude Code may have left it behind after a short command, and it frees itself within about 11 s, so retry";
```

- [ ] **Step 1: Write the failing tests.**

  In `cc/src/locks.rs` `tests` (Review Focus 4):
  ```rust
  #[test]
  fn each_lock_names_its_role_and_only_the_config_lock_says_it_frees_itself() {
      let timed_out = |r: Result<(), LockError>| match r {
          Err(LockError::Timeout(t)) => t.to_string(),
          other => panic!("expected a timeout, got {other:?}"),
      };
      for legacy in [false, true] {
          let d = tempfile::tempdir().unwrap();
          let p0 = paths(d.path());
          if legacy {
              fs::write(p0.config_home.join(".config.json"), "{}").unwrap();
          }
          let p = CcPaths::resolve(&Env::for_test(d.path()));
          let cancel = Cancel::new();
          let creds = acquire_credentials(&p, ACQUIRE_TIMEOUT, &cancel).unwrap();
          assert_eq!(
              (creds.refresh.role(), creds.legacy.role()),
              ("Claude Code's refresh lock", "Claude Code's credential lock")
          );
          assert_eq!(
              timed_out(acquire_credentials(&p, Duration::ZERO, &cancel).map(drop)),
              format!("timed out waiting for the lock {}", p.refresh_lock.display())
          );
          drop(creds);
          let config = acquire_config(&p, ACQUIRE_TIMEOUT, &cancel).unwrap();
          assert_eq!(config.config.role(), "Claude Code's config lock", "legacy {legacy}");
          let frees = format!(
              "timed out waiting for the lock {}; Claude Code may have left it behind after a short command, and it frees itself within about 11 s, so retry",
              p.config_lock.display()
          );
          assert_eq!(timed_out(acquire_config(&p, Duration::ZERO, &cancel).map(drop)), frees);
          assert_eq!(timed_out(wait_config_idle(&p, Duration::ZERO, &cancel)), frees);
          drop(config);
          let sw = acquire_storage_write(&p, ACQUIRE_TIMEOUT, &cancel).unwrap();
          assert_eq!(
              (sw.v1.role(), sw.v2.role()),
              ("Claude Code's storage-write lock", "Claude Code's storage-write lock")
          );
      }
  }
  ```
  In `provider/src/provider.rs` `tests`:
  ```rust
  #[test]
  fn a_shadowing_item_names_the_agent_that_may_still_read_it() {
      let e = ProviderError::ShadowingItem { item: "Svc-credentials".into(), agent: "Agent" };
      assert_eq!(e.to_string(), "the Keychain item Svc-credentials could not be verified gone, so Agent may still read it");
  }
  ```
  In `mkdir_lock.rs` `tests`, replacing `:767`:
  ```rust
  #[test]
  fn a_timeout_carries_its_lock_s_hint_and_only_that() {
      let hinted = TimedOut { path: PathBuf::from("/h/x.lock"), hint: Some("it frees itself") };
      assert_eq!(LockError::Timeout(hinted).to_string(), "timed out waiting for the lock /h/x.lock; it frees itself");
      assert_eq!(LockError::Timeout(PathBuf::from("/h/y.lock").into()).to_string(), "timed out waiting for the lock /h/y.lock");
      let d = tempfile::tempdir().unwrap();
      let s = spec(d.path(), 60_000, 0, 3_000).with_timeout_hint("it frees itself");
      let _held = MkdirLock::acquire(&s).unwrap();
      match MkdirLock::acquire(&s) {
          Err(LockError::Timeout(t)) => assert_eq!(t, TimedOut { path: s.path.clone(), hint: Some("it frees itself") }),
          Err(e) => panic!("expected a timeout, got {e:?}"),
          Ok(_) => panic!("acquired a lock another holder has"),
      }
  }
  ```
  In `cc/tests/live_store.rs:654-659`:
  - bind `let ProviderError::ShadowingItem { item: name, agent } = &err`;
  - assert `*agent == "Claude Code"`;
  - assert `shown == format!("the Keychain item {name} could not be verified gone, so Claude Code may still read it")`, in place of the `contains`.
- [ ] **Step 2: Run them.**
  - `cargo test -p tagteam-cc --features test-hooks --lib locks::tests::each_lock_names_its_role_and_only_the_config_lock_says_it_frees_itself`
  - `cargo test -p tagteam-provider --lib provider::tests::a_shadowing_item_names_the_agent_that_may_still_read_it`

  Expected: FAIL to compile (no `role()`, no `TimedOut`, and `ShadowingItem` is a tuple variant).
- [ ] **Step 3: Implement** the edits listed in Files.
- [ ] **Step 4: Run the tests from Step 2 again**, plus `cargo test -p tagteam-provider --lib mkdir_lock` and `cargo test -p tagteam-cc --features test-hooks --test live_store`. Expected: PASS.
- [ ] **Step 5: Run the focused suites.**
  - `cargo test -p tagteam-provider`
  - `cargo test -p tagteam-cc --features test-hooks`
  - `cargo test -p tagteam-engine --features test-hooks --lib error::tests`
  - `cargo test -p tagteam-engine --features test-hooks --test recover --test bootstrap`
  - `cargo test -p tagteam-fake`

  Expected: PASS. In particular `engine/tests/bootstrap.rs:477` still finds "could not be verified gone".
- [ ] **Step 6: Commit (controller).** Message: `Let each lock carry its role and timeout hint, and the shadowing error its agent`

---

### Task 5: Keep each provider's home variables in Env.homes, not in two Claude Code fields

**Depends on:** Task 4 (`provider.rs`, `cc/src/provider.rs`, `cc/src/live.rs`)

**Files:**
- Modify: `crates/tagteam-provider/src/env.rs`:
  - `:54-57`: replace the two fields with `pub homes: BTreeMap<String, OsString>`.
  - `Debug` (`:69-87`) prints `.field("homes", &self.homes)`, names and values, as it printed the two fields.
  - `from_process` (`:103-104`) and `for_test`'s body (`:126-127`) set `homes: BTreeMap::new()`. `for_test`'s gating is hardening's, and its signature stays.
  - Add `capture_homes`.
  - The `vars` doc at `:61-62` drops its `CLAUDECODE` example.
  - Add two tests.
- Modify: `crates/tagteam-provider/src/provider.rs`. Add `pub struct ProviderVars { pub homes: &'static [&'static str] }` and the required trait method `fn vars(&self) -> &'static ProviderVars;`.
- Modify: `crates/tagteam-provider/src/lib.rs`. Add `pub use provider::ProviderVars;` on its own line.
- Create: `crates/tagteam-cc/src/env.rs`, holding the constants and the four accessors. `crates/tagteam-cc/src/lib.rs` gains `pub mod env;`.
- Modify: `crates/tagteam-cc/src/session.rs`:
  - `:94-95`: delete the constants and use `crate::env::{CONFIG_DIR, SECURE_STORAGE_DIR}`;
  - `:107-112`: `outer_home` reads through the accessors;
  - `:119-142`: `apply_outer_home` keeps both rules (below);
  - `:149-165`: `profile_env` and `profile_paths` write through the setters, and the doc at `:144` is updated.
- Modify: `crates/tagteam-cc/src/naming.rs:16-26`, `paths.rs:42` and `:54`, `provider.rs:619` (`session_dir`), and `doctor.rs`:
  - in `doctor.rs`, delete the `:34-35` constants and change `:102-103`, `:332`, `:335`, `:342`, `:360`, `:394` and `:501-502` to use the accessors.
- Modify: `crates/tagteam-cc/src/provider.rs`. Add `static VARS: ProviderVars = ProviderVars { homes: &[env::CONFIG_DIR, env::SECURE_STORAGE_DIR] };` and `fn vars(&self) -> &'static ProviderVars { &VARS }`.
- Modify: `crates/tagteam-fake/src/provider.rs`. Add `static VARS: ProviderVars = ProviderVars { homes: &[] };` and `vars()`. FakeAgent finds its home through `vars` (`HOME_VAR`), as before.
- Modify: `crates/tagteam/src/app.rs`:
  - add `home_vars`;
  - `Context::from_process` (`:244-249`) calls `ctx.env.capture_homes(&home_vars(&registry));` right after `build_registry`, before `capture_vars`;
  - the comment at `:259-262` names "a provider's home variable" instead of the CC name;
  - add a test.
- Modify: `crates/tagteam/src/statusline.rs:288` and `:292`. They become `tagteam_cc::env::config_dir(env)` and `tagteam_cc::env::set_config_dir(&mut plain, None)`; Task 11 replaces both.
- Modify: `crates/xtask/src/compat/guard.rs:80-81`. They become `tagteam_cc::env::set_config_dir(&mut env, Some(spelling.into()));` and `tagteam_cc::env::set_securestorage_dir(&mut env, None);`.
- Test, accessor spelling only (each `x.claude_config_dir = v` becomes `set_config_dir(&mut x, v)`, and each read becomes `config_dir(&x)`):
  - `crates/tagteam-cc/src/paths.rs` tests `:118-180`;
  - `crates/tagteam-cc/src/naming.rs` tests `:144-279`, which must pass unchanged apart from this (spec §5.1);
  - `crates/tagteam-cc/tests/doctor.rs` (8 lines), `live_store.rs` (3), `provider.rs` (3), `session.rs:64-65` and `:224-229`;
  - `crates/tagteam-engine/tests/common/mod.rs:260, 1796-1797, 1851-1852`, `manage.rs:333, 337, 415, 589`, `profiles.rs:128`, `purge.rs:1364, 1367, 1563, 1666, 1670, 1752, 1955, 2058, 2060, 2206`, `run_plan.rs:357, 386-387`, `run_real_keychain.rs:189-190`, `run_shell.rs:110, 129, 183-184, 193, 196, 199, 221, 245, 313`;
  - `crates/tagteam/src/statusline.rs` tests `:614-622, 771, 886, 935`;
  - `crates/tagteam/tests/tui_host.rs:478`;
  - `crates/tagteam-fake/tests/provider.rs:621`, which becomes `cc_shaped.homes.insert("CLAUDE_CONFIG_DIR".into(), "/p/0193".into());` (tagteam-fake has no cc dependency).
  - After the edit, `rg -n 'claude_config_dir|claude_securestorage_config_dir' crates` must print nothing.
- Test, new:
  - `crates/tagteam-provider/src/env.rs` (two tests);
  - `crates/tagteam-cc/tests/session.rs` (the combination table);
  - `crates/tagteam/src/app.rs` (`home_vars`);
  - `crates/tagteam/tests/cli.rs` (Review Focus 1).

**Interfaces:**
```rust
// tagteam-provider
impl Env { pub fn capture_homes(&mut self, names: &[&str]); }  // set: inserted as is (empty too); unset: removed
pub struct ProviderVars { pub homes: &'static [&'static str] }
trait Provider { fn vars(&self) -> &'static ProviderVars; }
// tagteam-cc/src/env.rs
pub const CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";
pub const SECURE_STORAGE_DIR: &str = "CLAUDE_SECURESTORAGE_CONFIG_DIR";
pub fn config_dir(env: &Env) -> Option<&OsStr>;
pub fn securestorage_dir(env: &Env) -> Option<&OsStr>;
pub fn set_config_dir(env: &mut Env, value: Option<OsString>);        // None removes the name
pub fn set_securestorage_dir(env: &mut Env, value: Option<OsString>);
// tagteam/src/app.rs
pub(crate) fn home_vars(registry: &ProviderRegistry) -> Vec<&'static str>;  // vars().homes, registration order, each once
```
`apply_outer_home` keeps both of today's rules, for each of the two names:
- `homes[name]` becomes the outer record's value, unconditionally (removed for `null`).
- `vars[name]` follows that value only when `vars` already tracks the name.
- A record that is not CC's is still `Invalid` naming no value.

- [ ] **Step 1: Write the failing tests.**

  In `env.rs` `tests`:
  ```rust
  #[test]
  fn provider_homes_are_captured_only_when_asked_for() {
      const NEVER: &str = "TAGTEAM_TEST_NEVER_SET_7F3B";
      let mut env = Env::for_test(Path::new("/tmp/fixture"));
      assert!(env.homes.is_empty());
      env.capture_homes(&["HOME", NEVER]);
      assert_eq!(env.homes.get("HOME").map(OsString::as_os_str), std::env::var_os("HOME").as_deref());
      assert!(!env.homes.contains_key(NEVER), "an unset variable is not inserted");
      env.homes.insert(NEVER.into(), "stale".into());
      env.capture_homes(&[NEVER]);
      assert!(!env.homes.contains_key(NEVER), "a variable gone from the process is dropped");
      assert!(Env::from_process().unwrap().homes.is_empty(), "from_process names no provider's variable");
  }

  #[test]
  fn debug_shows_homes_with_their_values() {
      let mut env = Env::for_test(Path::new("/tmp/fixture"));
      env.homes.insert("AGENT_HOME".into(), "/p".into());
      assert!(format!("{env:?}").contains(r#"homes: {"AGENT_HOME": "/p"}"#), "{env:?}");
  }
  ```
  In `cc/tests/session.rs`, a new test `apply_outer_home_restores_homes_always_and_vars_only_where_tracked`:
  - It starts from `inside`, a run shell: `homes = {CLAUDE_CONFIG_DIR: "/data/sessions/0192"}`, with secure storage scrubbed (absent).
  - For each row, `var` is tracked by inserting `vars[var]`: `"/data/sessions/0192"` for `CLAUDE_CONFIG_DIR`, and the presence marker `""` for `CLAUDE_SECURESTORAGE_CONFIG_DIR`.
  - The outer record gives `var` the row's value and the other variable `null`.
  - It calls `f.cc.apply_outer_home(&inside, &outer)` and asserts `restored.homes.get(var)`, `restored.vars.get(var)` and `restored.var(var).is_some()`, which is the presence `launch.rs:215` reads.
  - It also asserts that the other variable is absent from both maps, and that `restored.home == inside.home`.

  | var | outer value | tracked | `homes[var]` after | `vars[var]` after | present (`launch.rs:215`) |
  |---|---|---|---|---|---|
  | `CLAUDE_CONFIG_DIR` | `"/outer"` | yes | `"/outer"` | `"/outer"` | yes |
  | `CLAUDE_CONFIG_DIR` | `"/outer"` | no | `"/outer"` | absent | no |
  | `CLAUDE_CONFIG_DIR` | `""` | yes | `""` | `""` | yes |
  | `CLAUDE_CONFIG_DIR` | `""` | no | `""` | absent | no |
  | `CLAUDE_CONFIG_DIR` | `null` | yes | absent | absent | no |
  | `CLAUDE_CONFIG_DIR` | `null` | no | absent | absent | no |
  | `CLAUDE_SECURESTORAGE_CONFIG_DIR` | `"/outer-secure"` | yes | `"/outer-secure"` | `"/outer-secure"` | yes |
  | `CLAUDE_SECURESTORAGE_CONFIG_DIR` | `"/outer-secure"` | no | `"/outer-secure"` | absent | no |
  | `CLAUDE_SECURESTORAGE_CONFIG_DIR` | `""` | yes | `""` | `""` | yes |
  | `CLAUDE_SECURESTORAGE_CONFIG_DIR` | `""` | no | `""` | absent | no |
  | `CLAUDE_SECURESTORAGE_CONFIG_DIR` | `null` | yes | absent | absent | no |
  | `CLAUDE_SECURESTORAGE_CONFIG_DIR` | `null` | no | absent | absent | no |

  In `app.rs` `tests` (the `Context` literal as in `:2856-2865`):
  ```rust
  #[test]
  fn the_process_boundary_captures_each_provider_s_home_variables() {
      let dir = tempfile::tempdir().unwrap();
      let ctx = Context { env: Env::for_test(dir.path()), keychain: Arc::new(FakeKeychain::new()), vault_keychain: None,
          platform: Platform::MacOs, api_base: None, stdout_terminal: false, no_color_env: false, force_color_env: false };
      assert_eq!(home_vars(&build_registry(&ctx)), ["CLAUDE_CONFIG_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR"]);
      assert!(ctx.env.homes.is_empty(), "only the process boundary captures");
  }
  ```
  In `tagteam/tests/cli.rs` (Review Focus 1):
  ```rust
  #[test]
  fn the_binary_names_the_live_item_from_the_secure_storage_dir_it_was_started_with() {
      // Review refactor §5.1: `Context::from_process` fills `Env.homes` for every provider's home
      // variables, so the live item is named from CLAUDE_SECURESTORAGE_CONFIG_DIR (Appendix A.2).
      let d = tempfile::tempdir().unwrap();
      let secure = d.path().join("secure");
      std::fs::create_dir_all(&secure).unwrap();
      let mut env = Env::for_test(d.path());
      tagteam_cc::env::set_securestorage_dir(&mut env, Some(secure.clone().into_os_string()));
      seed_home(&env);
      login(&env, &FileKeychain::new(d.path().join("keychain")), "a@x.co", "", "rt-a");
      cmd(d.path()).env("CLAUDE_SECURESTORAGE_CONFIG_DIR", &secure).arg("add").assert().success();
      cmd(d.path())
          .arg("add")
          .assert()
          .code(1)
          .stderr(predicates::str::contains("there is no live login to add; log in with `claude` first"));
  }
  ```
- [ ] **Step 2: Run them.**
  - `cargo test -p tagteam-provider --lib env::tests`
  - `cargo test -p tagteam-cc --features test-hooks --test session apply_outer_home_restores_homes_always_and_vars_only_where_tracked`
  - `cargo test -p tagteam --features test-support --lib app::tests::the_process_boundary_captures_each_provider_s_home_variables`

  Expected: FAIL to compile (no field `homes`, no `capture_homes`, no `home_vars`).
- [ ] **Step 3: Implement the provider side.** Make the `env.rs`, `provider.rs` and `lib.rs` changes, then `cc/src/env.rs` and the cc call sites, then `fake`, `app.rs`, `statusline.rs` and xtask.
- [ ] **Step 4: Migrate every test site** listed under "accessor spelling only". `rg -n 'claude_config_dir|claude_securestorage_config_dir' crates` must print nothing.
- [ ] **Step 5: Run the new tests again**, plus `cargo test -p tagteam --features test-support --test cli the_binary_names_the_live_item_from_the_secure_storage_dir_it_was_started_with`. Expected: PASS.
- [ ] **Step 6: Run the focused suites.**
  - `cargo test -p tagteam-provider`
  - `cargo test -p tagteam-cc --features test-hooks`, including the `naming.rs` Keychain-service tests unchanged and `the_outer_home_round_trips_undefined_set_and_defined_but_empty`
  - `cargo test -p tagteam-fake`
  - `cargo test -p tagteam-engine --features test-hooks --test run_shell --test run_plan --test purge --test manage --test profiles --test session`
  - `cargo test -p tagteam --features test-support --lib`
  - `cargo test -p tagteam --features test-support --test run_shell_cli --test run_cli --test statusline --test tui_host`
  - `cargo test -p xtask`

  Expected: PASS.
- [ ] **Step 7: Commit (controller).** Message: `Keep each provider's home variables in Env.homes, not in two Claude Code fields`

---

### Task 6: Take the captured and unit variables and the switch hint from the provider

**Depends on:** Task 5 (`ProviderVars`, `app.rs`)

**Files:**
- Modify: `crates/tagteam-provider/src/provider.rs`. `ProviderVars` gains `captured`, `unit` and `session_id`. Add `ProviderWords` and the required `fn words(&self) -> &'static ProviderWords;`.
- Modify: `crates/tagteam-provider/src/lib.rs`. The re-export line becomes `pub use provider::{ProviderVars, ProviderWords};`.
- Modify: `crates/tagteam-cc/src/provider.rs`. Complete `VARS` and add `WORDS` and `words()` (values below).
- Modify: `crates/tagteam-fake/src/provider.rs`. Complete `VARS` (empty lists, `session_id: None`) and add `WORDS` and `words()` (values below).
- Modify: `crates/tagteam-engine/src/service.rs`:
  - `:27-47`: replace `RECORDED_VARS` with `XDG_VARS`, `TAIL_VARS`, `recorded_vars` and the private `recorded_vars_of`;
  - `:146-161`: `recorded_env` takes the list;
  - add `impl Engine { pub fn recorded_vars(&self) }`;
  - the doc at `:27-30` names "each provider's homes";
  - tests `:972-1020` are rewritten as below.
- Modify: `crates/tagteam-engine/src/doctor.rs:50` (import) and `:1315`, which becomes `for name in self.engine.recorded_vars().into_iter().filter(|n| *n != "PATH")`.
- Modify: `crates/tagteam-engine/src/switch.rs`:
  - `SwitchOutcome` (`:128-143`) gains `pub activation_hint: Option<&'static str>`;
  - `noop` (`:193-211`) sets `None`;
  - the success path (`:2197`) sets `Some(activation_hint(p, outgoing.as_ref().map(|r| r.kind.as_str()), &target.kind, &stored_in, managed_key_replaced))`, computed before the struct is built;
  - add `fn activation_hint`;
  - add a `#[cfg(test)] mod tests` at the end of `switch.rs` with the hint-matrix test below (it moves with the code in R2).
- Modify: `crates/tagteam/src/app.rs`:
  - delete the constants `:145-149`;
  - `session_vars` (`:284-298`) takes the `session_dir_var`s, then each provider's `vars().captured`, then `"PATH"`, each name once;
  - `:263` becomes `ctx.env.capture_vars(&recorded_vars(&registry));`;
  - `run_statusline` (`:972-978`) reads the session id after `statusline::engine`, from `engine.provider(&provider).ok().and_then(|p| p.vars().session_id)` looked up in `engine.env()`, with the same `to_str` and non-empty filter;
  - `:1785` becomes `recorded_env(&self.engine.recorded_vars(), &|name| …)`;
  - the import at `:32` changes;
  - the docs at `:284-287` drop the CC names.
- Modify: `crates/tagteam/src/render.rs`:
  - delete `:2` (`KIND_API_KEY`) and the three hint constants at `:22-29`;
  - `switch_human` (`:497-520`) prints `o.activation_hint`;
  - tests: the helper `stored()` (`:750`) gains `activation_hint: None`. The three hint tests at `:822-867` (`a_switch_to_or_from_an_api_key_account_says_to_restart_claude_code`, `a_switch_that_replaces_a_live_managed_key_no_account_holds_says_to_restart_too`, `a_switch_between_other_kinds_keeps_the_store_s_hint`) are replaced by one render test, because their logic moves to the engine and its tests below (§5.3's "render.rs's unit tests").
- Test: `crates/tagteam-engine/tests/switch.rs` (two new tests), `crates/tagteam-engine/src/switch.rs` (the hint matrix), `crates/tagteam-engine/src/service.rs` tests, `crates/tagteam/src/render.rs` tests.

**Interfaces:**
```rust
pub struct ProviderVars {
    pub homes: &'static [&'static str],
    /// Variables the binary captures into `Env.vars` at start-up.
    pub captured: &'static [&'static str],
    /// Variables a service unit records (D9.1), beside tagteam's own.
    pub unit: &'static [&'static str],
    /// The variable a running session exports with its session id, if any.
    pub session_id: Option<&'static str>,
}
pub struct ProviderWords {
    pub after_switch_file: &'static str,
    pub after_switch_keychain: &'static str,
    pub after_switch_managed_key: &'static str,
    /// The provider's background supervisor, as a sentence names it, article included.
    pub supervisor: Option<&'static str>,
    /// The command that stops it.
    pub supervisor_stop: Option<&'static str>,
}
trait Provider { fn words(&self) -> &'static ProviderWords; }
// tagteam-engine/src/service.rs
pub fn recorded_vars(registry: &ProviderRegistry) -> Vec<&'static str>;   // XDG_VARS, ∪ vars().unit in registration order, TAIL_VARS
fn recorded_vars_of(units: &[&[&'static str]]) -> Vec<&'static str>;
pub fn recorded_env(names: &[&str], var: &dyn Fn(&str) -> Option<OsString>) -> Result<Vec<(String, String)>, UnitError>;
impl Engine { pub fn recorded_vars(&self) -> Vec<&'static str>; }
// tagteam-engine/src/switch.rs
fn activation_hint(p: &dyn Provider, from_kind: Option<&str>, to_kind: &str, stored_in: &SecretStore, managed_key_replaced: bool) -> &'static str;
//   either side's kind on the managed-key axis, or managed_key_replaced → words.after_switch_managed_key;
//   else File | Fallback → after_switch_file; else Keychain → after_switch_keychain
```
Pinned values:
- `XDG_VARS = ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"]`.
- `TAIL_VARS = ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy", "TAGTEAM_LOG", "PATH"]`.
- CC `VARS`:
  - `homes: &[CONFIG_DIR, SECURE_STORAGE_DIR]`;
  - `captured: &["CLAUDECODE", "CLAUDE_CODE_SESSION_ID"]`;
  - `unit: &[CONFIG_DIR, SECURE_STORAGE_DIR]`;
  - `session_id: Some("CLAUDE_CODE_SESSION_ID")`.
- CC `WORDS`:
  - `after_switch_file: "Active on your next message."`;
  - `after_switch_keychain: "Claude Code picks this up within about 30 s; restart it to apply now."`;
  - `after_switch_managed_key: "Restart Claude Code to apply."`;
  - `supervisor: Some("a Claude Code background daemon")`;
  - `supervisor_stop: Some("claude daemon stop --any")`.
  - The three hint docs move from `render.rs:22-29` with the strings.
- FakeAgent `WORDS`:
  - `after_switch_file: "FakeAgent reads the new login when it next starts."`;
  - `after_switch_keychain: "FakeAgent keeps no Keychain item."`;
  - `after_switch_managed_key: "FakeAgent has no managed key."`;
  - `supervisor: None`, `supervisor_stop: None`.

- [ ] **Step 1: Write the failing tests.**

  In `engine/tests/switch.rs` (Review Focus 3):
  ```rust
  #[test]
  fn the_activation_hint_is_the_provider_s_words_for_the_axis_and_the_store() {
      const KEYCHAIN: &str = "Claude Code picks this up within about 30 s; restart it to apply now.";
      const MANAGED: &str = "Restart Claude Code to apply.";
      let fx = Fx::new();
      let a = fx.add("a@x.co", "rt-a");
      let b = fx.add("b@x.co", "rt-b"); // live
      let k = fx.add_api_key(API_KEY);
      assert_eq!(switch(&fx, to(&a), false).unwrap().activation_hint, Some(KEYCHAIN));
      assert_eq!(switch(&fx, to(&k), false).unwrap().activation_hint, Some(MANAGED), "to an API key");
      assert_eq!(switch(&fx, to(&b), false).unwrap().activation_hint, Some(MANAGED), "from an API key");
      fx.put_managed_key(STRAY_API_KEY.as_bytes());
      assert_eq!(switch(&fx, to(&a), false).unwrap().activation_hint, Some(MANAGED), "a live key no account holds");
      assert_eq!(switch(&fx, to(&a), false).unwrap().activation_hint, None, "already active: nothing switched");
      let linux = Fx::with_platform(tagteam_cc::live::Platform::Linux);
      let c = linux.add("c@x.co", "rt-c");
      linux.add("d@x.co", "rt-d");
      assert_eq!(switch(&linux, to(&c), false).unwrap().activation_hint, Some("Active on your next message."));
  }

  #[test]
  fn a_fake_agent_switch_takes_its_hint_from_fake_agent() {
      let f = FakeFx::new();
      let x = f.fake_add("x", "tok-x", "renew-x");
      f.fake_add("y", "tok-y", "renew-y");
      assert_eq!(f.switch_fake(&x).activation_hint, Some("FakeAgent reads the new login when it next starts."));
  }
  ```
  In `engine/src/switch.rs`, a new `#[cfg(test)] mod tests`. It keeps the whole matrix the three deleted `render.rs` tests pinned (`render.rs:815-867`), now against the engine's choice:
  ```rust
  use super::*;
  use std::{path::PathBuf, sync::Arc};

  #[test]
  fn the_hint_matrix_is_the_one_the_renderer_used_to_hold() {
      let cc = tagteam_cc::ClaudeCode::new(Arc::new(tagteam_provider::FakeKeychain::new()), tagteam_provider::Platform::MacOs);
      let w = cc.words();
      let stores = [
          SecretStore::Keychain,
          SecretStore::File(PathBuf::from("/c.json")),
          SecretStore::Fallback(PathBuf::from("/c.json")),
      ];
      for store in &stores {
          for (from, to) in [
              (Some("oauth"), "api_key"),
              (Some("api_key"), "oauth"),
              (Some("api_key"), "api_key"),
              (None, "api_key"),
              (Some("setup_token"), "api_key"),
          ] {
              assert_eq!(activation_hint(&cc, from, to, store, false), w.after_switch_managed_key, "{from:?} -> {to} via {store:?}");
          }
          assert_eq!(activation_hint(&cc, None, "oauth", store, true), w.after_switch_managed_key, "a live key no account holds, via {store:?}");
      }
      assert_eq!(activation_hint(&cc, Some("oauth"), "setup_token", &stores[0], false), w.after_switch_keychain);
      assert_eq!(activation_hint(&cc, Some("oauth"), "oauth", &stores[1], false), w.after_switch_file);
      assert_eq!(activation_hint(&cc, Some("oauth"), "oauth", &stores[2], false), w.after_switch_file);
      assert_eq!(w.after_switch_managed_key, "Restart Claude Code to apply.");
      assert_eq!(w.after_switch_file, "Active on your next message.");
  }
  ```
  Run it with `cargo test -p tagteam-engine --features test-hooks --lib switch::tests`; it fails to compile before Step 3 (no `activation_hint`, no `words()`).

  In `engine/src/service.rs` `tests`, rewrite `recorded_env_keeps_the_list_s_order_and_only_what_is_set` to pass `&cc` and assert `cc` equals the 15-name list it pinned as `RECORDED_VARS`. Here `cc = recorded_vars(&ProviderRegistry::new().with(Arc::new(tagteam_cc::ClaudeCode::new(Arc::new(tagteam_provider::FakeKeychain::new()), tagteam_provider::Platform::MacOs))))`. The other assertions are unchanged. Add:
  ```rust
  #[test]
  fn each_provider_s_unit_variables_come_once_between_tagteam_s_own() {
      assert_eq!(
          recorded_vars_of(&[&["A", "B"], &["B", "C"]]),
          [&XDG_VARS[..], &["A", "B", "C"], &TAIL_VARS[..]].concat()
      );
  }
  ```
  In `render.rs` `tests`, replacing the three hint tests:
  ```rust
  #[test]
  fn switch_human_prints_the_engine_s_hint_under_the_line() {
      let hinted = SwitchOutcome { activation_hint: Some("Hint."), ..switched(Some("oauth"), "oauth", SecretStore::Keychain) };
      assert_eq!(switch_human(&hinted), "Switched to x@x.co (position 2).\nHint.\n");
      let bare = switched(Some("oauth"), "oauth", SecretStore::Keychain);
      assert_eq!(switch_human(&bare), "Switched to x@x.co (position 2).\n");
  }
  ```
- [ ] **Step 2: Run them.**
  - `cargo test -p tagteam-engine --features test-hooks --test switch the_activation_hint`
  - `cargo test -p tagteam-engine --features test-hooks --lib service::tests`

  Expected: FAIL to compile (no field `activation_hint`, no `recorded_vars`).
- [ ] **Step 3: Implement** the provider, cc and fake data first, then the engine (`service.rs`, `doctor.rs`, `switch.rs`), then the binary (`app.rs`, `render.rs`).
- [ ] **Step 4: Run the tests from Step 2 again**, plus `cargo test -p tagteam --features test-support --lib render::tests`. Expected: PASS.
- [ ] **Step 5: Run the focused suites.**
  - `cargo test -p tagteam-engine --features test-hooks --lib --test switch --test doctor --test fake_agent`
  - `cargo test -p tagteam --features test-support --lib`, including `the_process_boundary_captures_every_session_variable_and_claudecode`, still `["CLAUDE_CONFIG_DIR", "CLAUDECODE", "CLAUDE_CODE_SESSION_ID", "PATH"]`
  - `cargo test -p tagteam --features test-support --test app --test strategy_cli --test tui_host --test statusline --test daemon_service_cli --test doctor_cli`

  Expected: PASS. These pin the hints and the unit contents end to end: `tests/app.rs:378`, `:392` and `:541`, `strategy_cli.rs:153`, `tui_host.rs:27`, and `statusline.rs`'s `the_statusline_records_a_session_only_in_a_home_tagteam_knows` (`s-shell`: the session id is read from the outer env).
- [ ] **Step 6: Commit (controller).** Message: `Take the captured and unit variables and the switch hint from the provider`

---

### Task 7: Build the engine's messages from the provider's name, launch command and words

**Depends on:** Task 6 (`ProviderWords`, `switch.rs`, `doctor.rs`)

**Files:**
- Modify: `crates/tagteam-engine/src/error.rs`:
  - `:49-50` becomes `NoLiveLogin { launch: &'static str }`, with "…log in with `{launch}` first";
  - `:71-74` becomes `NeedsRelogin { position, label, launch: &'static str }`, with "…log in with `{launch}`, then run `tagteam add`";
  - `kind()` (`:448`, `:455`) matches `{ .. }`;
  - `daemon_advice` (`:292-302`) takes `p` (below);
  - `SessionOwner` (`:304-414`) gains `app`, `supervisor` and `stop`, and `of` takes `p`;
  - `who()` builds its live phrase with `self.supervisor`, `repairs()` builds the supervisor-lock repair with `self.app`, and `remedies()` pushes `self.stop`;
  - the unit tests `:590` (`NoLiveLogin { launch: "claude" }`) and `:613` (`launch: "claude"`) change, and a new test is added.
- Modify: `crates/tagteam-engine/src/lifecycle.rs`. `live_fresh` (`:89-98`) takes `p`, and its callers at `:854` and `:932` pass it. `:848` and `:860` become `EngineError::NoLiveLogin { launch: p.launch_command() }`.
- Modify: `crates/tagteam-engine/src/active.rs:430`, `:454` and `:494`.
- Modify: `crates/tagteam-engine/src/switch.rs`:
  - `needs_relogin` (`:520-525`) takes `p`, with callers at `:1391`, `:1413`, `:1449` and `:1897`;
  - `session_owned` (`:540-542`) takes `p`, with callers at `:1476` and `:1867`;
  - `:1166` becomes `SessionOwner::of(&state, p)`;
  - `:1762-1771`: both log lines become `"a starting {} had overwritten the switched account's identity, which was spliced again"` and `"a starting {} may have overwritten the switched account's identity"`, with `p.display_name()`. These lines are missing from §5.3's list; see the deltas.
- Modify: `crates/tagteam-engine/src/launch.rs:173`, `:313` and `:391`; `session.rs:331-337` (`session_owned_error` takes `p`) and `:323`; `purge.rs:495` and `:662`; `import.rs:290`.
- Modify: `crates/tagteam-engine/src/doctor.rs`:
  - `daemon_check` (`:2534-2540`) and `ownership` (`:2546-2575`) take `p`, with callers at `:2607` and `:2666`;
  - `:2537` becomes `format!("{} runs in {whose} until it stops", supervisor_name(p))`;
  - `reservations` (`:2972`) takes `p`, with its caller at `:2714`;
  - `held_reservation` (`:3202`) takes `launch`, and `:3233` names it;
  - the unit-test calls `:3258-3267` pass `"claude"`;
  - add a new test.
- Modify: `crates/tagteam-engine/src/collect.rs:1124`. It uses `{app}` from `self.p.display_name()`; the struct holds `p` at `:547`.
- Modify: `crates/tagteam-engine/src/export.rs:32`. Delete `WIPED`, and `:442` and `:530` call `broken(row, wiped(p))`.
- Test: `crates/tagteam-engine/tests/add.rs:120` and `:172` become `Err(EngineError::NoLiveLogin { .. })` (§5.3's list).

**Interfaces:**
```rust
EngineError::NoLiveLogin { launch: &'static str }
EngineError::NeedsRelogin { position: u32, label: String, launch: &'static str }
pub(crate) fn daemon_advice(p: &dyn Provider, profile: &Path) -> String;
//   "stop it" + (supervisor_stop: " with `{stop}`") + (session_dir_var: " run with {var} set to {quoted(profile)}")
//   + (supervisor_lock(profile): "; if nothing is running at that pid, delete {quoted(lock)}")
pub(crate) fn supervisor_name(p: &dyn Provider) -> &'static str; // words().supervisor, else "a background supervisor"
pub struct SessionOwner {
    pub session: bool, pub daemon: Option<PathBuf>, pub damaged: Vec<Damaged>,
    /// `display_name()`, for the supervisor lock's repair.
    pub app: &'static str,
    /// `supervisor_name(p)`.
    pub supervisor: &'static str,
    /// `daemon_advice(p, profile)` when `daemon` is set.
    pub stop: Option<String>,
}
impl SessionOwner { pub fn of(state: &SessionState, p: &dyn Provider) -> Self; }
pub(crate) fn needs_relogin(p: &dyn Provider, target: &AccountRow) -> EngineError;
pub(crate) fn session_owned_error(p: &dyn Provider, row: &AccountRow, state: &SessionState) -> EngineError;
fn live_fresh(p: &dyn Provider, r: Read<Credential>) -> Result<Credential, EngineError>;
fn wiped(p: &dyn Provider) -> String;  // "{display_name} wiped the copy in use after its login was rejected"
// doctor.rs (associated fns of Run and a free fn)
fn daemon_check(p: &dyn Provider, profile: &Path, whose: &str) -> Check;
fn ownership(p: &dyn Provider, state: &SessionState, whose: &str, runs_in: &str, consequence: &str) -> Vec<Check>;
fn reservations(&self, p: &dyn Provider, dir: &Path, n: u32, found: &mut Vec<Check>);
fn held_reservation(path: &Path, parent: u32, n: u32, launch: &str, holders: Option<Vec<u32>>, still_there: bool) -> Check;
```

- [ ] **Step 1: Write the failing tests.**

  In `error.rs` `tests` (Review Focus 2). `tagteam-cc` is a dev-dependency.
  ```rust
  #[test]
  fn the_ownership_texts_name_the_provider_s_supervisor_and_launch_command() {
      let cc = tagteam_cc::ClaudeCode::new(std::sync::Arc::new(tagteam_provider::FakeKeychain::new()), tagteam_provider::Platform::MacOs);
      let state = SessionState::Owned {
          profile: PathBuf::from("/p"),
          session: true,
          daemon: true,
          damaged: vec![Damaged { kind: DamagedKind::SupervisorLock, file: PathBuf::from("/p/daemon.lock"), detail: "torn".into() }],
      };
      assert_eq!(
          SessionOwner::of(&state, &cc).reason("position 1 (a@x.co)"),
          "position 1 (a@x.co) is in use by a `tagteam run` session and by a Claude Code background daemon, and also '/p/daemon.lock' cannot be read (torn); exit that session; stop it with `claude daemon stop --any` run with CLAUDE_CONFIG_DIR set to '/p'; if nothing is running at that pid, delete '/p/daemon.lock'; if nothing runs as Claude Code for that profile, delete the lock, then retry"
      );
      let daemon_only = SessionState::Owned { profile: PathBuf::from("/p"), session: false, daemon: true, damaged: vec![] };
      assert_eq!(SessionOwner::of(&daemon_only, &cc).who(), "is in use by a Claude Code background daemon");
      assert_eq!(EngineError::NoLiveLogin { launch: "claude" }.to_string(), "there is no live login to add; log in with `claude` first");
  }
  ```
  In `doctor.rs` `tests`:
  ```rust
  #[test]
  fn the_daemon_check_and_a_held_reservation_name_the_agent() {
      let cc = tagteam_cc::ClaudeCode::new(std::sync::Arc::new(tagteam_provider::FakeKeychain::new()), tagteam_provider::Platform::MacOs);
      let c = Run::daemon_check(&cc, Path::new("/p"), "account 1's profile, so it is session-owned");
      assert_eq!(c.message, "a Claude Code background daemon runs in account 1's profile, so it is session-owned until it stops");
      assert_eq!(c.fix.as_deref(), Some("stop it with `claude daemon stop --any` run with CLAUDE_CONFIG_DIR set to '/p'; if nothing is running at that pid, delete '/p/daemon.lock'"));
      let held = held_reservation(Path::new("/d/.tagteam-launch/42.lock"), 42, 1, "claude", Some(vec![7, 9]), true);
      assert_eq!(held.message, "/d/.tagteam-launch/42.lock is still held although its tagteam (pid 42) is gone (held by pid 7, 9): a process its `claude` started still runs, so account 1 stays in a session");
  }
  ```
- [ ] **Step 2: Run them.**
  - `cargo test -p tagteam-engine --features test-hooks --lib error::tests::the_ownership_texts_name_the_provider_s_supervisor_and_launch_command`
  - `cargo test -p tagteam-engine --features test-hooks --lib doctor::tests::the_daemon_check_and_a_held_reservation_name_the_agent`

  Expected: FAIL to compile (`of` takes one argument, `NoLiveLogin` is a unit variant, `daemon_check` and `held_reservation` have the old arities).
- [ ] **Step 3: Implement** every edit listed in Files. `rg -n -i 'claude|anthropic' crates/tagteam-engine/src` outside comments and `#[cfg(test)]` halves should leave only `settings.rs:18`, `:341`, `:761` and `:825`, and `transfer.rs:17` and `:353`, which Tasks 9, 13 and 14 remove.
- [ ] **Step 4: Run the tests from Step 2 again.** Expected: PASS.
- [ ] **Step 5: Run the focused suites.**
  - `cargo test -p tagteam-engine --features test-hooks`
  - `cargo test -p tagteam --features test-support --test cli --test doctor_cli --test purge_cli --test run_cli`

  Expected: PASS, with these text pins unchanged:
  - `engine/tests/launch.rs:1915`, `freshen.rs:118` and `tagteam/tests/cli.rs:436` (`NeedsRelogin`);
  - `export.rs:406` (wiped); `collect_active.rs:854` (replaced);
  - `session.rs:267-470`, `purge.rs:526-570`, `switch.rs:1033`, `:1042` and `:1652`, and `doctor.rs:1507-1768` (daemon, lock repair and "a starting Claude Code");
  - `import.rs:298`.
- [ ] **Step 6: Commit (controller).** Message: `Build the engine's messages from the provider's name, launch command and words`

---

### Task 8: Name the provider's launch command and name in the CLI's own messages

**Depends on:** Task 7 (`render.rs` and `app.rs` follow Task 6's edits; the engine compiles against Task 7)

**Files:**
- Modify: `crates/tagteam/src/app.rs`:
  - delete the const `NO_LIVE_LOGIN` (`:114-115`) and add `fn no_live_login`;
  - `live_row` (`:2460`) calls `no_live_login(self.engine.provider(&self.provider())?.launch_command())`;
  - `:1274` passes `launch` (the launch command of `self.provider()`) to `list_human`;
  - `:2325` passes `display_name` to `tokens_cmd::notices`;
  - add a test.
- Modify: `crates/tagteam/src/render.rs`:
  - delete the const `NO_ACCOUNTS` (`:21`) and add `fn no_accounts`;
  - `list_human` (`:326-334`) takes `launch: &str`;
  - the unit-test calls at `:977`, `:1013`, `:1030`, `:1051`, `:1080`, `:1114` and `:1120` pass `"claude"` (§5.3's list);
  - add a new test.
- Modify: `crates/tagteam/src/tokens_cmd.rs`. `notices` (`:255-274`) takes `app: &str`; `:263` uses `{app}'s transcript format may have changed`. The unit test (`:1015-1040` and on) passes `"Claude Code"` and gains one FakeAgent assertion.

**Interfaces:**
```rust
fn no_live_login(launch: &str) -> String;   // app.rs: "there is no live login; name an account, or log in with `{launch}` first"
fn no_accounts(launch: &str) -> String;     // render.rs: "No accounts yet. Log in with `{launch}`, then run `tagteam add`.\n"
pub fn list_human(lists: &[ProviderAccounts], this: Option<&AccountId>, display_names: &dyn Fn(&str) -> String, launch: &str, now_s: i64, color: bool) -> String;
pub(crate) fn notices(report: &TokenReport, no_refresh: bool, app: &str) -> Vec<String>;
```

- [ ] **Step 1: Write the failing tests.**
  ```rust
  // render.rs tests
  #[test]
  fn the_empty_list_names_the_provider_s_launch_command() {
      let names = |id: &str| id.to_owned();
      assert_eq!(list_human(&[accounts(CLAUDE_CODE, None)], None, &names, "claude", NOW, false),
                 "No accounts yet. Log in with `claude`, then run `tagteam add`.\n");
      assert_eq!(list_human(&[accounts("fake-agent", None)], None, &names, "fakeagent", NOW, false),
                 "No accounts yet. Log in with `fakeagent`, then run `tagteam add`.\n");
  }
  // app.rs tests
  #[test]
  fn no_live_login_names_the_launch_command() {
      assert_eq!(no_live_login("claude"), "there is no live login; name an account, or log in with `claude` first");
  }
  ```
  In `tokens_cmd.rs`'s `notices_name_a_partial_index_skipped_lines_and_unreadable_files`, add:
  ```rust
  assert!(notices(&report_with(drift), false, "FakeAgent")[0].ends_with("FakeAgent's transcript format may have changed"));
  ```
- [ ] **Step 2: Run them.** `cargo test -p tagteam --features test-support --lib render::tests::the_empty_list_names_the_provider_s_launch_command`. Expected: FAIL to compile (arity).
- [ ] **Step 3: Implement** the edits listed in Files.
- [ ] **Step 4: Run the tests again** (the same command, plus `app::tests::no_live_login_names_the_launch_command` and `tokens_cmd::tests`). Expected: PASS.
- [ ] **Step 5: Run the focused suites.** `cargo test -p tagteam --features test-support --lib`, `cargo test -p tagteam --features test-support --test app --test history --test tui_cli --test tokens_cli`. Expected: PASS, with these pins unchanged: `tests/app.rs:343` and `:347` and `tui_cli.rs:124` (no accounts); `history.rs:297` (no live login).
- [ ] **Step 6: Commit (controller).** Message: `Name the provider's launch command and name in the CLI's own messages`

---

### Task 9: List the registry's providers in the unknown-provider refusal and drop the claude-code example

**Depends on:** Task 8 (`error.rs` from Task 7; `CHANGELOG.md` is first touched here)

**Files:**
- Modify: `crates/tagteam-engine/src/settings.rs`:
  - `:339-342`: the `KeyKind::Provider` reason becomes `"must be a provider id of lowercase letters, digits and dashes"`;
  - `:760-762`: `UnknownProvider { id: String, known: Vec<String> }` with `#[error("unknown provider {id:?}; name one this build has, such as {}", known.join(", "))]`.
- Modify: `crates/tagteam-engine/src/config.rs:119`. It becomes `SettingsError::UnknownProvider { id: id.clone(), known: self.registry.all().iter().map(|p| p.id().to_string()).collect() }`, which is C-9's only producer.
- Modify: `crates/tagteam-engine/src/error.rs:487` (`UnknownProvider { .. }`) and the unit test `:803` (`UnknownProvider { id: "p".into(), known: vec!["claude-code".into()] }`).
- Test:
  - `crates/tagteam-engine/tests/settings.rs:1153` expects the new reason (§5.3, §11).
  - `crates/tagteam-engine/tests/config.rs:697` matches `SettingsError::UnknownProvider { id, .. }` and gains a text pin.
  - `crates/tagteam/tests/config_cli.rs` gains a CLI pin.
- Modify: `CHANGELOG.md`. Add under `## [Unreleased]`, in a new `### Changed`:
  ```markdown
  - `tagteam config`'s description of `default_provider`, and its warning for an invalid value, no
    longer give `claude-code` as an example: they read "must be a provider id of lowercase
    letters, digits and dashes".
  ```

**Interfaces:**
- `SettingsError::UnknownProvider { id: String, known: Vec<String> }`. Its text is unchanged for a one-provider build.

- [ ] **Step 1: Write the failing tests.**
  - `engine/tests/settings.rs:1153`: expected `"must be a provider id of lowercase letters, digits and dashes"`.
  - `engine/tests/config.rs:697`: replace with
    ```rust
    let err = fx.engine.config_set("default_provider", None, "fake-agent").unwrap_err();
    assert!(matches!(&err, EngineError::Settings(SettingsError::UnknownProvider { id, .. }) if id == "fake-agent"));
    assert_eq!(err.to_string(), "unknown provider \"fake-agent\"; name one this build has, such as claude-code");
    ```
  - `tagteam/tests/config_cli.rs`:
    ```rust
    #[test]
    fn an_unknown_default_provider_names_the_ones_this_build_has() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("home")).unwrap();
        cmd(d.path()).args(["config", "set", "default_provider", "codex"]).assert().code(1)
            .stdout("").stderr("tagteam: unknown provider \"codex\"; name one this build has, such as claude-code\n");
    }
    ```
- [ ] **Step 2: Run them.** `cargo test -p tagteam-engine --features test-hooks --test settings each_key_says_what_a_valid_value_is` and `--test config`. Expected: FAIL (the old reason text, and a struct pattern on a tuple variant).
- [ ] **Step 3: Implement** the edits listed in Files, and the CHANGELOG entry.
- [ ] **Step 4: Run the tests again**, plus `cargo test -p tagteam --features test-support --test config_cli`. Expected: PASS. The CLI pin passes both before and after, which is its purpose: byte identity.
- [ ] **Step 5: Run the focused suite.** `cargo test -p tagteam-engine --features test-hooks --lib --test settings --test config`. Expected: PASS. `a_malformed_default_provider_falls_back_to_claude_code_with_a_warning` still finds "`default_provider` must be a provider id".
- [ ] **Step 6: Commit (controller).** Message: `List the registry's providers in the unknown-provider refusal and drop the claude-code example`

---

### Task 10: Pass the login check its inputs as one ValidateRequest

**Depends on:** Task 9 (the trait in `provider.rs`; `launch.rs` from Task 7)

**Files:**
- Modify: `crates/tagteam-provider/src/provider.rs:809-822`. Delete the `#[allow(clippy::too_many_arguments)]`, add `ValidateRequest`, and change the signature (below).
- Modify: `crates/tagteam-provider/src/lib.rs`. Add `pub use provider::ValidateRequest;` on its own line, or add it to Task 6's line.
- Modify: the implementers `crates/tagteam-cc/src/provider.rs:789` and `crates/tagteam-fake/src/provider.rs:982`, which destructure `req`.
- Modify: the callers `crates/tagteam-engine/src/bootstrap.rs:435` and `launch.rs:473`.
- Test, mechanical: `crates/tagteam-cc/tests/session.rs:1884`, `:1929`, `:1972`, `:2008` and `:2224`; `crates/tagteam-fake/tests/provider.rs:1049` and `:1090`. Each builds a `ValidateRequest`.

**Interfaces:**
```rust
pub struct ValidateRequest<'a> {
    pub env: &'a Env,
    pub spelling: &'a str,
    pub cwd: &'a Path,
    pub program: &'a Path,
    pub expect: &'a Identity,
}
fn validate_profile(&self, req: &ValidateRequest<'_>, spawner: &dyn ProcessSpawner, cancel: &Cancel) -> Validity;
```

- [ ] **Step 1: Identify the existing tests that prove it.** These must stay green unchanged except for the call shape:
  - `cc/tests/session.rs` `validate` (`:1880`) and its callers;
  - the spawn-spec test at `:1925-1955`, which asserts `spec.program` and `spec.cwd`, so a swapped `cwd`/`program` fails it;
  - `fake/tests/provider.rs:1049-1100`;
  - `engine/tests/launch.rs` and `bootstrap.rs` (`login_checks`).
- [ ] **Step 2: Change the trait first.** Run `cargo build --workspace --all-targets --features tagteam/test-support`. Expected: FAIL at the 2 implementers, 2 callers and 7 test sites.
- [ ] **Step 3: Update them** as listed in Files.
- [ ] **Step 4: Run the suites.**
  - `cargo test -p tagteam-cc --features test-hooks --test session`
  - `cargo test -p tagteam-fake --test provider`
  - `cargo test -p tagteam-engine --features test-hooks --test launch --test bootstrap`

  Expected: PASS. `rg -n too_many_arguments crates/tagteam-provider` prints nothing.
- [ ] **Step 5: Run clippy on the touched crates.** `cargo clippy -p tagteam-provider -p tagteam-cc -p tagteam-fake -p tagteam-engine --all-targets --features tagteam-engine/test-hooks -- -D warnings`. Expected: clean.
- [ ] **Step 6: Commit (controller).** Message: `Pass the login check its inputs as one ValidateRequest`

---

### Task 11: Build every registry, the statusline's and completions' included, in one function

**Depends on:** Task 10 (`provider.rs` trait, `cc/src/provider.rs`); Task 5 (`Env.homes`)

**Files:**
- Modify: `crates/tagteam-provider/src/provider.rs`. Add the trait method `fn statusline_setup(&self, _env: &Env, _command: &str) -> Option<(PathBuf, String)> { None }`, with the doc "Where and what to write so the agent shows tagteam's statusline; `None` if unsupported."
- Modify: `crates/tagteam-cc/src/provider.rs`. Implement it as `Some((CcPaths::resolve(env).config_home.join("settings.json"), format!("{}\n", serde_json::to_string_pretty(&json!({"statusLine": {"type": "command", "command": command}})).expect("a JSON value always serializes"))))`.
- Modify: `crates/tagteam/src/app.rs`:
  - add `registry`;
  - `build_registry` (`:274-282`) becomes `registry(ctx.keychain.clone(), ctx.platform, ctx.api_base.as_deref())`, with its signature kept for the held callers;
  - `run_statusline` (`:978-982`) uses `let (engine, provider) = statusline::engine(…)`;
  - `--print-config` uses `statusline::print_config`. When it gives `None`, the command fails `KIND_UNSUPPORTED` with `format!("{} has no statusline", p.display_name())`, the existing text; the case is unreachable for CC.
  - the test `completion_offers_only_providers_this_build_registers` (`:2672-2695`) is replaced (spec §5.5).
- Modify: `crates/tagteam/src/statusline.rs`:
  - `:10`: drop `use tagteam_cc::{CcPaths, ClaudeCode};`;
  - `:182-199`: replace `config_snippet` and `config_hint` with `print_config`;
  - `:235-266`: `engine` returns `(Engine, ProviderId)` through the private `walled`, which builds the registry with `crate::app::registry(keychain.clone(), ctx.platform, None)`;
  - `:283-300`: `sighting_home` takes `p` (below);
  - `record_session` (`:315-333`) passes the provider it already fetches;
  - tests:
    - `print_config_is_claude_codes_settings_snippet` (`:577`) asserts `print_config(&cc, &env)`, with the same strings;
    - `the_sighting_home_is_the_profile_the_default_home_or_elsewhere` (`:604`) passes `&cc` and gains one case;
    - the 4-tuple destructurings at `:772`, `:793`, `:822`, `:835`, `:910`, `:917`, `:930`, `:937` and `:1048` use `walled` or `engine`.
- Modify: `crates/tagteam/src/cli.rs`:
  - `:9` drops the `CLAUDE_CODE` import;
  - `:593-594` replaces `COMPLETED_PROVIDERS` with `completed_providers()`;
  - `:616` and `:644` iterate it, with `PossibleValue::new(String)` (clap's `string` feature).
- `crates/tagteam/src/shell_init.rs`: no change. `run_shell_init` (`app.rs:1040-1048`) already builds `Wrapped { id, launch: p.launch_command() }` for each registered provider (see the deltas).

**Interfaces:**
```rust
// app.rs: the only code that names ClaudeCode
pub(crate) fn registry(keychain: Arc<dyn Keychain>, platform: Platform, api_base: Option<&str>) -> ProviderRegistry;
pub(crate) fn build_registry(ctx: &Context) -> ProviderRegistry;            // unchanged signature
// statusline.rs
pub(crate) struct Walls { http: Arc<LazyHttp>, keychain: Arc<NoKeychain> }
fn walled(ctx: Context, flag: Option<&str>) -> (Engine, ProviderId, Walls);
pub(crate) fn engine(ctx: Context, flag: Option<&str>) -> (Engine, ProviderId);
pub(crate) fn print_config(p: &dyn Provider, env: &Env) -> Option<(String, String)>;
//   (hint, snippet): hint = "Add this to {path}; tagteam never edits {display_name}'s settings."
pub(crate) fn sighting_home(shell: &RunShell, env: &Env, p: &dyn Provider) -> SightingHome;
//   Outside: p.session_dir(env) unset or empty → Default; else plain = env with session_dir_var()
//   removed from `homes` and `vars`; same_dir(dir, p.share_policy(&plain).source) → Default, else Elsewhere
// cli.rs
pub(crate) fn completed_providers() -> Vec<String>;
//   ids of app::registry(Arc::new(NoKeychain::default()), Platform::current(), None)
```
`sighting_home` reads the effective home through `p.session_dir(env)`, which for CC is `Env.homes["CLAUDE_CONFIG_DIR"]` filtered non-empty. That is exactly the spec's "the value is in `Env.homes`; unset or empty: Default". The `Walls` design follows the spec's intent with a private function instead of a `#[cfg(test)]` accessor; see the deltas.

- [ ] **Step 1: Write the failing tests.**
  ```rust
  // app.rs tests, replacing :2672-2695
  #[test]
  fn the_statusline_builds_the_registry_every_other_command_builds() {
      let dir = tempfile::tempdir().unwrap();
      let ctx = Context { env: Env::for_test(dir.path()), keychain: Arc::new(FakeKeychain::new()), vault_keychain: None,
          platform: Platform::MacOs, api_base: None, stdout_terminal: false, no_color_env: false, force_color_env: false };
      let registered: Vec<ProviderId> = build_registry(&ctx).all().iter().map(|p| p.id()).collect();
      let (statusline, _) = statusline::engine(ctx, None);
      assert_eq!(statusline.provider_ids(), registered);
      assert_eq!(crate::cli::completed_providers(), registered.iter().map(ToString::to_string).collect::<Vec<_>>());
  }
  ```
  In `statusline.rs`'s sighting test, add after the `elsewhere` case (spec §5.5):
  ```rust
  let other = dir.path().join("other-home");
  std::fs::create_dir_all(&other).unwrap();
  set_config_dir(&mut env, Some(other.into_os_string()));
  assert_eq!(outside(&env), SightingHome::Elsewhere, "an existing directory other than the default");
  ```
  Here `outside` becomes `|env: &Env| sighting_home(&RunShell::Outside, env, &cc)`, where `cc = ClaudeCode::new(Arc::new(FakeKeychain::new()), Platform::MacOs)`. `print_config_is_claude_codes_settings_snippet` becomes:
  ```rust
  let dir = tempfile::tempdir().unwrap();
  assert_eq!(print_config(&cc, &Env::for_test(dir.path())), Some((
      format!("Add this to {}; tagteam never edits Claude Code's settings.", dir.path().join("home/.claude/settings.json").display()),
      "{\n  \"statusLine\": {\n    \"type\": \"command\",\n    \"command\": \"tagteam statusline\"\n  }\n}\n".to_owned(),
  )));
  assert_eq!(print_config(&tagteam_fake::FakeAgent::new(), &Env::for_test(dir.path())), None);
  ```
- [ ] **Step 2: Run them.** `cargo test -p tagteam --features test-support --lib app::tests::the_statusline_builds_the_registry_every_other_command_builds`. Expected: FAIL to compile (`engine` returns a 4-tuple, and there is no `completed_providers`).
- [ ] **Step 3: Implement** the edits listed in Files.
  - Production `statusline.rs` and `cli.rs` must no longer name `tagteam_cc`, `ClaudeCode` or `CcPaths`.
  - `rg -n 'tagteam_cc|ClaudeCode' crates/tagteam/src --glob '!app.rs'` should match only `#[cfg(test)]` halves and `logging.rs:27-28`.
- [ ] **Step 4: Run the tests again**, plus `cargo test -p tagteam --features test-support --lib statusline::tests`. Expected: PASS.
- [ ] **Step 5: Run the focused suites.** `cargo test -p tagteam --features test-support --lib`, `cargo test -p tagteam --features test-support --test statusline --test completions --test home_cli --test app`. Expected: PASS, with these pins unchanged:
  - `statusline.rs:340-360` (`--print-config` output and hint);
  - `completions.rs:97`, `:265` and `:401` (`claude-code` offered);
  - `the_statusline_records_a_session_only_in_a_home_tagteam_knows`;
  - `shell_init.rs`'s unit tests and the `shell-init` CLI tests.
- [ ] **Step 6: Commit (controller).** Message: `Build every registry, the statusline's and completions' included, in one function`

---

### Task 12: Move ImportRecord and the import field readers into tagteam-provider

**Depends on:** Task 11 (`tagteam-provider/src/lib.rs`, `provider.rs`)

**Files:**
- Create: `crates/tagteam-provider/src/import.rs`. Move into it, verbatim apart from the error type:
  - `ImportRecord` and its `Debug` impl (`crates/tagteam-engine/src/transfer.rs:153-184`);
  - the readers `field_error`, `optional_string`, `position`, `disabled`, the `Numbered` alias and `accounts` (`:186-247`), made `pub`.
  - Add `FormatError`.
  - `payload` (`:222-228`) stays in the engine: only `read_envelope` uses it.
- Modify: `crates/tagteam-provider/src/lib.rs`. Add `pub mod import;` and `pub use import::{FormatError, ImportRecord};`, each on its own line.
- Modify: `crates/tagteam-engine/src/transfer.rs`:
  - delete the moved items and add `pub use tagteam_provider::import::ImportRecord;` (it keeps `crate::transfer::ImportRecord` and `tagteam_engine::transfer::ImportRecord`);
  - `use tagteam_provider::import::{accounts, disabled, field_error, optional_string, position};`;
  - add `impl From<FormatError> for TransferError` (→ `TransferError::Format(e.0)`);
  - the engine's retained readers convert explicitly where `?` does not:
    - `payload` (`:222-228`) returns `Err(field_error(account, field, "present").into())`;
    - in `read_envelope` (`:252-297`), the `.map(|(i, o)| …)` closure is annotated
      `-> Result<ImportRecord, TransferError>`, and its two explicit
      `return Err(field_error(…))` (provider, kind) become `return Err(field_error(…).into())`;
    - its `optional_string(…)?`, `position(…)?`, `disabled(…)?`, `accounts(…)?` and
      `.ok_or_else(|| field_error(…))?` convert through `?`;
  - `read_cswap_v1` (`:299-367`) now returns `Result<Vec<ImportRecord>, FormatError>`, so its `format_error(…)` calls become `FormatError(…into())`, and `decode` (`:616-622`) maps it with `?`. This readies it for Task 13's verbatim move.
- Test: no new test. `engine/tests/transfer.rs`, `import.rs` and `log_lines.rs` (which import `tagteam_engine::transfer::ImportRecord`) prove the move with unchanged text. `read_cswap_v1`'s `err.to_string()` assertions (`transfer.rs:465-490`) compile unchanged, because `FormatError: Display`.

**Interfaces:**
```rust
// tagteam_provider::import
#[derive(Clone)] pub struct ImportRecord { /* fields as at transfer.rs:157-170 */ }
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)] #[error("{0}")] pub struct FormatError(pub String);
pub fn field_error(account: usize, field: &str, must_be: &str) -> FormatError;
pub fn optional_string(o: &Map<String, Value>, account: usize, field: &str) -> Result<Option<String>, FormatError>;
pub fn position(o: &Map<String, Value>, account: usize, field: &str) -> Result<u32, FormatError>;
pub fn disabled(o: &Map<String, Value>, account: usize) -> Result<bool, FormatError>;
pub type Numbered = Vec<(usize, Map<String, Value>)>;
pub fn accounts(v: &Value, what: &str) -> Result<Numbered, FormatError>;
// tagteam-engine
impl From<FormatError> for TransferError;
pub fn read_cswap_v1(v: &Value) -> Result<Vec<ImportRecord>, FormatError>;  // still in transfer.rs until Task 13
```

- [ ] **Step 1: Identify the existing tests that prove the move.** Every test in `engine/tests/transfer.rs` (the field-error cases at `:340-380` pin each reader's message), `engine/tests/import.rs`, `engine/tests/log_lines.rs:598`, and `tagteam/tests/transfer_cli.rs`.
- [ ] **Step 2: Move and adjust the error type.** Run `cargo build --workspace --all-targets --features tagteam/test-support`. Expected: success.
- [ ] **Step 3: Run the suites.**
  - `cargo test -p tagteam-engine --features test-hooks --test transfer --test import --test log_lines --test export`
  - `cargo test -p tagteam --features test-support --test transfer_cli`
  - `cargo test -p tagteam-provider --lib`

  Expected: PASS.
- [ ] **Step 4: Check that it reads as a move.** `git diff --color-moved=dimmed-zebra` shows `ImportRecord` and the readers as moved, with only `TransferError` changed to `FormatError` inside them.
- [ ] **Step 5: Commit (controller).** Message: `Move ImportRecord and the import field readers into tagteam-provider`

---

### Task 13: Move the cswap importer into tagteam-cc behind foreign_export

**Depends on:** Task 12 (`import.rs`, the `transfer.rs` shape)

**Files:**
- Create: `crates/tagteam-cc/src/cswap.rs`:
  - `read_cswap_v1`, moved verbatim from `crates/tagteam-engine/src/transfer.rs` (only its `use` lines differ, with `ProviderId::new(CLAUDE_CODE)` still taking `tagteam_core::CLAUDE_CODE` until Task 14);
  - `fn is_cswap(v: &Value) -> bool { v.get("swapVersion").is_some() }`;
  - `pub static CSWAP: ForeignFormat = ForeignFormat { name: "cswap", detect: is_cswap, read: read_cswap_v1 };`.
  - `crates/tagteam-cc/src/lib.rs` gains `pub mod cswap;`.
- Modify: `crates/tagteam-provider/src/import.rs`. Add `ForeignFormat`.
- Modify: `crates/tagteam-provider/src/lib.rs`. The import re-export line adds `ForeignFormat`.
- Modify: `crates/tagteam-provider/src/provider.rs`. Add the trait method `fn foreign_export(&self) -> Option<&'static ForeignFormat> { None }`, with the doc "An export format of another tool that this provider can import."
- Modify: `crates/tagteam-cc/src/provider.rs`. `fn foreign_export(&self) -> Option<&'static ForeignFormat> { Some(&crate::cswap::CSWAP) }`.
- Modify: `crates/tagteam-engine/src/transfer.rs`:
  - delete `read_cswap_v1` and the `CLAUDE_CODE` import at `:17`;
  - `Decoded.cswap` (`:595`) becomes `foreign: Option<&'static str>`;
  - `decode` (`:604-629`) takes the registry: an envelope when `format` is present, else the first registered provider whose `foreign_export()` detects the file, read through it, else `NOT_AN_EXPORT` (text unchanged);
  - the module doc (`:1-5`) says foreign formats come from the providers.
- Modify: `crates/tagteam-engine/src/engine.rs`. Add `pub fn registry(&self) -> &ProviderRegistry` beside `provider_ids` (`:292`).
- Modify: `crates/tagteam/src/app.rs:1626`. It becomes `transfer::decode(&input, &keys, self.engine.registry(), &mut |need| …)`.
- Create: `crates/tagteam-cc/tests/cswap.rs`. Move these from `engine/tests/transfer.rs:389-490`: `API_KEY`, `SETUP_TOKEN`, `cswap`, `token_account`, `cswap_three`, a local `credential(rt)` helper as in `transfer.rs`, and the `read_cswap_v1` assertions of `a_cswap_export_maps_its_oauth_api_key_and_setup_token_accounts_to_claude_code` and `a_cswap_export_that_is_encrypted_or_not_version_1_is_refused_by_name`. They are the same assertions, through `tagteam_cc::cswap::read_cswap_v1`.
- Modify: `crates/tagteam-engine/tests/transfer.rs`:
  - every `decode(` call passes `&registry()`, where `fn registry() -> ProviderRegistry { ProviderRegistry::new().with(Arc::new(ClaudeCode::new(Arc::new(FakeKeychain::new()), Platform::MacOs))) }`;
  - `:100` becomes `assert_eq!(d.foreign, None)`;
  - the decode-level assertions of the moved tests become one new test.
- Modify: `crates/tagteam-engine/tests/import.rs:377`, `crates/tagteam/tests/app.rs:1595` and `crates/tagteam/tests/transfer_cli.rs:42-43`. Each passes a registry built the same way.
- Modify: `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md` §4.5. Add one line after "It is expected to change when the second provider lands (§17, R7).":
  ```markdown
  The review refactor adds `vars`, `words`, `statusline_setup` and `foreign_export` ([review refactor §5.2](2026-10-19-tagteam-review-refactor-design.md)).
  ```

**Interfaces:**
```rust
// tagteam_provider::import
pub struct ForeignFormat {
    /// What the format is called; `Decoded.foreign` carries it.
    pub name: &'static str,
    /// Whether a parsed file is in this format.
    pub detect: fn(&Value) -> bool,
    /// The file's accounts, as records for this provider.
    pub read: fn(&Value) -> Result<Vec<ImportRecord>, FormatError>,
}
trait Provider { fn foreign_export(&self) -> Option<&'static ForeignFormat> { None } }
// tagteam-engine
pub struct Decoded { pub encrypted: bool, pub foreign: Option<&'static str>, pub records: Vec<ImportRecord> }
pub fn decode(input: &[u8], identities: &[IdentityFile], registry: &ProviderRegistry,
              ask: &mut dyn FnMut(&Need) -> Option<SecretString>) -> Result<Decoded, TransferError>;
impl Engine { pub fn registry(&self) -> &ProviderRegistry; }
// tagteam-cc
pub mod cswap { pub fn read_cswap_v1(v: &Value) -> Result<Vec<ImportRecord>, FormatError>; pub static CSWAP: ForeignFormat; }
```

- [ ] **Step 1: Write the failing tests.** Create `cc/tests/cswap.rs` with the moved assertions; it fails to compile until `tagteam_cc::cswap` exists. Add to `engine/tests/transfer.rs`:
  ```rust
  #[test]
  fn a_cswap_export_is_read_by_the_provider_that_names_its_format() {
      let file = serde_json::to_vec(&json!({"version": 1, "swapVersion": "0.27.0b1", "encrypted": false, "accounts": [
          {"number": 1, "email": "a@x.co", "credentials": credential("rt-a"),
           "config": {"oauthAccount": {"emailAddress": "a@x.co"}}}]})).unwrap();
      let d = decode(&file, &[], &registry(), &mut ask_nothing).unwrap();
      assert_eq!((d.foreign, d.encrypted, d.records.len()), (Some("cswap"), false, 1));
      assert_eq!(d.records[0].provider.as_str(), "claude-code");
      let mut v2: Value = serde_json::from_slice(&file).unwrap();
      v2["version"] = json!(2);
      let err = decode(&serde_json::to_vec(&v2).unwrap(), &[], &registry(), &mut ask_nothing).unwrap_err();
      assert_eq!(err.to_string(), "the cswap export's version is 2; tagteam reads version 1");
      let fake_only = ProviderRegistry::new().with(Arc::new(tagteam_fake::FakeAgent::new()));
      let err = decode(&file, &[], &fake_only, &mut ask_nothing).unwrap_err();
      assert_eq!(err.to_string(), "the file is not a tagteam export, an age-encrypted one, or a cswap version 1 export");
  }
  ```
- [ ] **Step 2: Run them.** `cargo test -p tagteam-cc --features test-hooks --test cswap` and `cargo test -p tagteam-engine --features test-hooks --test transfer a_cswap_export_is_read_by_the_provider_that_names_its_format`. Expected: FAIL to compile (no `tagteam_cc::cswap`, and `decode` takes 3 arguments).
- [ ] **Step 3: Implement.** Move `read_cswap_v1` verbatim, then wire `ForeignFormat`, `decode` and `Engine::registry`, then update the callers and the core spec line.
- [ ] **Step 4: Run the tests again.** Expected: PASS.
- [ ] **Step 5: Run the focused suites.**
  - `cargo test -p tagteam-engine --features test-hooks --test transfer --test import`
  - `cargo test -p tagteam-cc --features test-hooks`
  - `cargo test -p tagteam --features test-support --test transfer_cli --test app`

  Expected: PASS. `transfer_cli.rs:424` `a_cswap_v1_export_imports_through_the_cli` proves the import behaviour is unchanged end to end.
- [ ] **Step 6: Commit (controller).** Message: `Move the cswap importer into tagteam-cc behind foreign_export`

---

### Task 14: Move CLAUDE_CODE into tagteam-cc and fall back to the registry's first provider

**Depends on:** Task 13 (the engine no longer names `CLAUDE_CODE` in `transfer.rs`; `Engine::registry`), Task 11 (`cli.rs`), Task 9 (`settings.rs`)

**Files:**
- Modify: `crates/tagteam-core/src/ids.rs`. Delete `:3`; the unit test at `:38` uses the literal `"claude-code"`.
- Modify: `crates/tagteam-core/src/lib.rs:22`. It becomes `pub use ids::{AccountId, IdentityKey, ProviderId};`.
- Modify: `crates/tagteam-cc/src/lib.rs`. Add `pub const CLAUDE_CODE: &str = "claude-code";`. `crates/tagteam-cc/src/provider.rs:9`, `:250` and `crates/tagteam-cc/src/cswap.rs` use `crate::CLAUDE_CODE`.
- Modify: `crates/tagteam-engine/src/settings.rs` (C-3):
  - `:18` drops `CLAUDE_CODE`;
  - `default_provider: Option<ProviderId>`, whose doc reads "`None` when the file names none, or names it invalidly; the CLI falls back to the registry's first provider (C-3)";
  - `Default` (`:825`) is `None`;
  - `value` (`:870`) is `Value::Str(self.default_provider.as_ref().map_or_else(String::new, ToString::to_string))`;
  - `apply` (`:900`) stores `Some(ProviderId::new(id))`;
  - `inspect` (`:686`) takes `fallback` and fills an unset `default_provider` with it, in both the read settings and the defaults, before reading values.
- Modify: `crates/tagteam-engine/src/doctor.rs:962`. It becomes `settings::inspect(&self.engine.env, id, &self.engine.default_provider)`; doctor reads only `warnings` and `unknown`.
- Modify: `crates/tagteam/src/app.rs`:
  - `:12` drops `CLAUDE_CODE`, and the test module imports `tagteam_cc::CLAUDE_CODE`;
  - add `fallback_provider`;
  - `command_settings` (`:344-372`) takes `fallback`;
  - `:349` is deleted, and `:351` becomes `settings.default_provider.clone().unwrap_or_else(|| fallback.clone())`;
  - `:356` says `using {fallback}`;
  - `:362` compares with `*fallback`;
  - `build_engine` (`:403`) passes `&fallback_provider(&registry)`;
  - `config list` and `get` (`:1658`, `:1676`) pass `&fallback_provider(self.engine.registry())`;
  - the docs at `:332` and `:339-343` say "the registry's first provider";
  - the tests at `:2589`, `:2607`, `:2615`, `:2630`, `:2651`, `:2663` and `:2667` pass `&ProviderId::new(CLAUDE_CODE)`;
  - `:2641` asserts `chosen.settings.default_provider.as_ref().map(ProviderId::as_str) == Some("other")`;
  - add a new test.
- Modify: `crates/tagteam/src/statusline.rs:246`. It passes `&crate::app::fallback_provider(&registry)`.
- Modify, one import each (`tagteam_core::CLAUDE_CODE` becomes `tagteam_cc::CLAUDE_CODE`, nothing else; every file `rg -l -P '\bCLAUDE_CODE\b(?!_)' crates` lists outside core and cc):
  - `crates/tagteam-daemon/src/events.rs`, `protocol.rs` and `testutil.rs` (test modules), and `tests/tokens.rs`;
  - `crates/tagteam-engine/src/engine.rs`, `lazy_http.rs`, `testutil.rs`, `views.rs` and `words.rs` (test modules);
  - `crates/tagteam-engine/tests/collect.rs`, `common/mod.rs`, `import.rs`, `log_lines.rs`, `oracle.rs`, `run_real_keychain.rs`, `session_usage.rs`, `tokens_view.rs` and `transfer.rs`;
  - `crates/tagteam/src/auto.rs`, `daemon_cmd.rs`, `render.rs`, `statusline.rs` and `transfer_cmd.rs` (test modules);
  - `crates/tagteam/tests/app.rs`, `auto_cli.rs`, `common/mod.rs`, `daemon_cli.rs`, `daemon_procs.rs`, `kill.rs`, `run_shell_cli.rs`, `run_tokens.rs`, `signals.rs`, `statusline.rs`, `strategy_cli.rs`, `tokens_cli.rs`, `tui_host.rs` and `usage_cli.rs`;
  - held, compile fix only: `crates/tagteam-tui/src/testutil.rs:12` and `crates/tagteam-tui/tests/source.rs:14` (`CLAUDE_CODE` leaves the `tagteam_core` list, and `use tagteam_cc::CLAUDE_CODE;` is added).
- Test: `crates/tagteam-engine/tests/settings.rs`:
  - `:39` becomes `assert_eq!(d.default_provider, None)`;
  - `:128` becomes `default_provider: Some(ProviderId::new("fake-agent"))`;
  - `:1124` becomes `("default_provider", Value::Str(String::new()))`;
  - `:1367` becomes `Some(ProviderId::new("fake-agent"))`;
  - `:1414` becomes `None`;
  - `inspect` calls at `:999`, `:1490`, `:1579` and `:1843` pass `&ProviderId::new(PROVIDER)`.

**Interfaces:**
```rust
pub const CLAUDE_CODE: &str = "claude-code";                            // tagteam_cc
pub default_provider: Option<ProviderId>,                               // Settings
pub fn inspect(env: &Env, provider: &ProviderId, fallback: &ProviderId) -> Inspection;
pub(crate) fn fallback_provider(registry: &ProviderRegistry) -> ProviderId;  // first registered; panics on an empty registry
pub(crate) fn command_settings(env: &Env, fallback: &ProviderId, registered: &dyn Fn(&ProviderId) -> bool,
                               choose: impl FnOnce(&ProviderId) -> ProviderId) -> CommandSettings;
```
`Settings::load(env, provider)` keeps its signature (C-3). `config get` and `config list` print the effective id: the file's value when it sets one, registered or not (`config_cli.rs:391`); else the fallback, in both `value` and `default` (`config_cli.rs:121`, `:159` and `:196`).

- [ ] **Step 1: Write the failing tests.** Make the `engine/tests/settings.rs:39` edit, and add to `app.rs` `tests`:
  ```rust
  #[test]
  fn without_a_default_provider_setting_a_command_acts_on_the_fallback_it_is_given() {
      // C-3: the fallback is the registry's first provider, not a name core or the engine knows.
      let dir = tempfile::tempdir().unwrap();
      let env = Env::for_test(dir.path());
      let chosen = command_settings(&env, &ProviderId::new("other"), &and_other, |d| d.clone());
      assert_eq!((chosen.provider.as_str(), chosen.default.as_str()), ("other", "other"));
      assert!(chosen.warnings.is_empty(), "{:?}", chosen.warnings);
  }
  ```
- [ ] **Step 2: Run them.** `cargo test -p tagteam-engine --features test-hooks --test settings the_defaults_are_the_specs_table`. Expected: FAIL to compile (mismatched types `ProviderId` / `Option<_>`).
- [ ] **Step 3: Implement.** Do C-3 in `settings.rs`, `doctor.rs`, `app.rs` and `statusline.rs`, then move the constant and update every import listed in Files. `rg -n -P '\bCLAUDE_CODE\b(?!_)' crates/tagteam-core crates/tagteam-engine/src crates/tagteam-daemon/src` should match only `#[cfg(test)]` halves.
- [ ] **Step 4: Run the tests again.** Run the Step 2 command and `cargo test -p tagteam --features test-support --lib app::tests`. Expected: PASS.
- [ ] **Step 5: Run the focused suites.**
  - `cargo build --workspace --all-targets --features tagteam/test-support`
  - `cargo test -p tagteam-core`
  - `cargo test -p tagteam-engine --features test-hooks --test settings --test config --lib`
  - `cargo test -p tagteam --features test-support --test config_cli --lib`
  - `cargo test -p tagteam-tui`

  Expected: PASS. `config_cli.rs:121`, `:159`, `:196`, `:383-395` and `:527` stay green unchanged.
- [ ] **Step 6: Commit (controller).** Message: `Move CLAUDE_CODE into tagteam-cc and fall back to the registry's first provider`

---

### Task 15: Describe provider-generic commands in help without naming Claude Code

**Depends on:** Task 14 (`cli.rs`, `CHANGELOG.md`)

**Files:**
- Modify: `crates/tagteam/src/cli.rs` (C-4):
  - `:30` (`--provider`): "The agent CLI to act on (default: the default_provider setting, else this build's first provider)"
  - `:75` (`add`): "Store the current live login"
  - `:181` (`tokens`): "Token use and API cost per account, from the agent's transcripts"
  - `:537` (`config get` KEY): "A setting, such as autoswitch.threshold or provider.<id>.autoswitch.models"
  - `:569-570` (`daemon install` long help): "…the variables that place its store, each agent's home, a proxy and the log."
  - Kept, as Claude-Code-only features (C-4): `:214` (`shell-init`) and `:220` (`statusline`).
  - Add a unit test.
- Modify: `CHANGELOG.md`, under `### Changed` in `[Unreleased]`:
  ```markdown
  - `tagteam --help` and each command's help describe what works for every provider without
    naming Claude Code. The help for the statusline and the shell wrapper, which only Claude
    Code has, still names it.
  ```

**Interfaces:**
- None (help text only).

- [ ] **Step 1: Write the failing test** in `cli.rs` `tests`:
  ```rust
  #[test]
  fn help_for_what_every_provider_does_names_no_provider() {
      // C-4: the statusline and the shell wrapper are Claude Code's alone, so their help may name it.
      fn texts(cmd: &clap::Command, out: &mut Vec<String>) {
          if matches!(cmd.get_name(), "statusline" | "shell-init") {
              return;
          }
          out.extend([cmd.get_about(), cmd.get_long_about()].into_iter().flatten().map(|s| s.to_string()));
          for a in cmd.get_arguments() {
              out.extend([a.get_help(), a.get_long_help()].into_iter().flatten().map(|s| s.to_string()));
          }
          for s in cmd.get_subcommands() {
              texts(s, out);
          }
      }
      let mut all = Vec::new();
      texts(&<super::Cli as clap::CommandFactory>::command(), &mut all);
      let named: Vec<&String> = all.iter().filter(|t| t.to_lowercase().contains("claude")).collect();
      assert!(named.is_empty(), "{named:#?}");
  }
  ```
- [ ] **Step 2: Run it.** `cargo test -p tagteam --features test-support --lib cli::tests::help_for_what_every_provider_does_names_no_provider`. Expected: FAIL, listing the five texts above.
- [ ] **Step 3: Change the five help texts and add the CHANGELOG entry.**
- [ ] **Step 4: Run the test again.** Expected: PASS.
- [ ] **Step 5: Run the focused suites.** `cargo test -p tagteam --features test-support --lib cli::tests` and `--test completions --test cli --test tui_args`. Expected: PASS. Completions pin values, not descriptions.
- [ ] **Step 6: Commit (controller).** Message: `Describe provider-generic commands in help without naming Claude Code`

---

### Task 16: Rewrite plan-local references in R1's files as the rules they stood for

**Depends on:** Tasks 1-15 (every file below is edited by one of them)

**Files** (src only; spec §14 skips hardening's files, the held TUI files and `tests/`). Each line is rewritten as the rule it stood for, or as a spec section. Re-find them with the check command in Step 1, because earlier tasks moved lines.
- `crates/tagteam-provider/src/env.rs:207` (Review I2)
- `crates/tagteam-provider/src/mkdir_lock.rs:97`, `:343`, `:487`, `:900`, `:921` and `:949`. "L356" names §9.1's rule that every touch and check goes through the held fd, never by path.
- `crates/tagteam-provider/src/provider.rs:884` (L370 is §14.2's rule)
- `crates/tagteam-cc/src/doctor.rs:59` (Task 12), `locks.rs:289` and `:456` (Review Focus 1)
- `crates/tagteam-engine/src/error.rs:198`, `collect.rs:819` and `:993`, `doctor.rs:214`, `:229`, `:300`, `:1358`, `:1747` and `:3201`, `service.rs:657`, `:828` and `:1316`, `switch.rs:1843`
- `crates/tagteam-engine/src/lifecycle.rs:747`, `:769`, `:802`, `:855`, `:864`, `:883`, `:906`, `:923` and `:1019`; `active.rs:326`, `:332`, `:340` and `:350`
- `crates/tagteam-engine/src/launch.rs:1`, `:2`, `:45`, `:309`, `:312`, `:728` and `:735`; `bootstrap.rs:246` and `:465`. The stale "Tests only (Task 9, before `launch` exists)" becomes "Tests only: what a quiescent launch runs between its …".
- `crates/tagteam-engine/src/purge.rs:1157` and `recover.rs:345`
- `crates/tagteam/src/app.rs:1492`, `:1859` and `:2189`; `statusline.rs:844`, `:927` and `:1020`; `cli.rs:498`

The "Decision N" references stay (spec §14). The docs Tasks 2 and 4 moved or rewrote (`passwd.rs`'s L380, `ShadowingItem`'s L397) are already clean.

**Interfaces:**
- None (comments only).

- [ ] **Step 1: Run the check, which fails.** `rg -n 'Task [0-9]+|Plan [A-Z]\b|Controller note|fix round|review, item|\bL[0-9]{3,4}\b|Review (Focus|[IM][0-9])|R-T[0-9]+' crates/tagteam-provider/src/{env,mkdir_lock,provider}.rs crates/tagteam-cc/src/{doctor,locks}.rs crates/tagteam-engine/src/{error,collect,doctor,service,switch,lifecycle,active,launch,bootstrap,purge,recover}.rs crates/tagteam/src/{app,statusline,cli}.rs`. Expected: the lines listed above.
- [ ] **Step 2: Rewrite each comment.**
  - Where the sentence already states the rule, drop the parenthetical.
  - Otherwise state the rule, or cite the spec section (§n of the core spec, Dn, TR §n).
  - No code changes.
- [ ] **Step 3: Run the check again.** Expected: no output.
- [ ] **Step 4: Run the format and build checks.** `cargo fmt --all --check` and `cargo build --workspace --all-targets --features tagteam/test-support`. Expected: clean.
  - Confirm that every changed line is a comment: `git diff -U0 -- crates | rg '^[+-]' | rg -v '^(\+\+\+|---)' | rg -v '^[+-]\s*//'` should print nothing.
- [ ] **Step 5: Commit (controller).** Message: `Rewrite plan-local references in comments as the rules they stood for`

---

### Task 17: Test that no neutral crate names Claude Code and only the composition root names tagteam-cc

**Depends on:** Task 16 (all leaks removed; the scan reads every file)

**Files:**
- Create: `crates/tagteam-fake/tests/neutrality.rs` (std only; no Cargo change, per C-5)

**Interfaces:**
```rust
const NEUTRAL: [&str; 5] = ["tagteam-provider", "tagteam-core", "tagteam-engine", "tagteam-daemon", "tagteam-tui"];
const LEAKS: [&str; 2] = ["claude", "anthropic"];          // matched on the lowercased line
/// Held for the TUI redesign (spec §10), pinned by file and text. Each must still match, so an
/// entry goes once its line does.
const ALLOWED: [(&str, &str); 2] = [
    ("crates/tagteam-tui/src/screens/accounts.rs", "Log in with claude, then press A"),
    ("crates/tagteam-tui/src/actions.rs", "Claude Code is not changed."),
];
const COMPOSITION_ROOT: &str = "crates/tagteam/src/app.rs";  // app/context.rs after R3
const BINARY_ALLOWED: [(&str, &str); 2] = [
    ("crates/tagteam/src/logging.rs", "tagteam_cc=info"),
    ("crates/tagteam/src/logging.rs", "tagteam_cc=debug"),
];
fn workspace() -> PathBuf;                                       // CARGO_MANIFEST_DIR/../.., canonical
fn sources(src: &Path) -> Vec<PathBuf>;                          // every .rs under src, sorted, less test-only module files
fn test_mod(lines: &[&str], at: usize) -> Option<(String, bool)>; // (name, inline)
fn production(text: &str) -> Vec<(usize, String)>;               // (1-based line, text)
```
The scanner's rules, which are the deliverable, so they are given in full:
```rust
/// `#[cfg(test)]` on a line of its own, then the next line that is not blank and not another
/// attribute: `[pub[(…)] ]mod name;` (a test-only file) or `… mod name {` (inline).
fn test_mod(lines: &[&str], at: usize) -> Option<(String, bool)> {
    if lines[at].trim() != "#[cfg(test)]" { return None; }
    let next = lines[at + 1..].iter().map(|l| l.trim()).find(|l| !l.is_empty() && !l.starts_with("#["))?;
    let rest = ["pub(crate) ", "pub(super) ", "pub "]
        .iter()
        .find_map(|v| next.strip_prefix(v))
        .unwrap_or(next);
    let decl = rest.strip_prefix("mod ")?;
    let name: String = decl.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    Some((name.clone(), decl[name.len()..].trim_start().starts_with('{')))
}

/// The production half: everything before the first inline `#[cfg(test)] mod`, without lines
/// whose trimmed text starts with `//`.
fn production(text: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let end = (0..lines.len()).find(|&i| matches!(test_mod(&lines, i), Some((_, true)))).unwrap_or(lines.len());
    (0..end).filter(|&i| !lines[i].trim_start().starts_with("//")).map(|i| (i + 1, lines[i].to_owned())).collect()
}
```
`sources(src)` walks `src` recursively. For every `(name, false)` that `test_mod` finds in a file `f`, it skips `<base>/name.rs` and everything under `<base>/name/`. `<base>` is `f`'s directory for `lib.rs`, `main.rs` and `mod.rs`, else `f` without its extension. Those are the engine's, daemon's and TUI's `testutil.rs` (`engine/src/lib.rs:37-38`, `daemon/src/lib.rs:17-18`, `tui/src/lib.rs:25-26` with `pub(crate)`) and cc's `compat.rs`.

- [ ] **Step 1: Write the test file** with five tests:
  - `the_scanner_keeps_production_and_drops_test_halves_and_comments` (the scanner's own test, so a broken scanner cannot pass vacuously):
    ```rust
    assert_eq!(production("fn a() {}\n// claude\n#[cfg(test)]\nmod tests {\n    claude\n}\n"), [(1, "fn a() {}".to_owned())]);
    assert_eq!(production("fn a() {}\n#[cfg(test)]\n#[allow(dead_code)]\npub(crate) mod t {\nclaude\n}\n"), [(1, "fn a() {}".to_owned())]);
    assert_eq!(production("#[cfg(test)]\nmod testutil;\nfn b() {}\n").last(), Some(&(3, "fn b() {}".to_owned())));
    assert_eq!(test_mod(&["#[cfg(test)]", "pub(crate) mod testutil;"], 0), Some(("testutil".to_owned(), false)));
    ```
  - `no_neutral_crate_names_claude_code`:
    - For each crate in `NEUTRAL`, for each file in `sources(crates/<crate>/src)`, check each `production` line whose lowercase contains a `LEAKS` word.
    - A hit is allowed only if `ALLOWED` has `(file, text)` with `text` contained in the line.
    - It collects `"{file}:{line}: {text}"` for the rest and asserts that list is empty.
    - It also asserts that every `ALLOWED` entry matched at least once ("remove the allowlist entry once its line is gone"), and that `tagteam-engine` contributed at least 30 files (no vacuous pass).
  - `no_neutral_crate_depends_on_tagteam_cc`: for each `NEUTRAL` crate, take the lines of its `Cargo.toml` from `[dependencies]` to the next line starting with `[`. Assert none, trimmed, starts with `tagteam-cc`. `[dev-dependencies]` is allowed.
  - `only_the_composition_root_names_tagteam_cc_in_the_binary`:
    - For each file in `sources(crates/tagteam/src)` other than `COMPOSITION_ROOT`, a `production` line containing `tagteam_cc` or `ClaudeCode` is a hit unless `BINARY_ALLOWED` has `(file, text)` contained in it.
    - Assert there are no hits, and that `COMPOSITION_ROOT` itself contains `ClaudeCode` (so the root has not moved without this test).
- [ ] **Step 2: Run it.** `cargo test -p tagteam-fake --test neutrality`. Expected: PASS, because Tasks 1-16 removed every leak. If it fails, the message names `file:line`; fix the leak in its owning task's style and record it under the deltas, rather than widening `ALLOWED`.
- [ ] **Step 3: Check once that each assertion fails on a planted leak, then revert.**
  1. Plant a leak in a neutral crate. Run `printf '\nconst _LEAK: &str = "Claude";\n' >> crates/tagteam-core/src/lib.rs` (a file with no test half), then `cargo test -p tagteam-fake --test neutrality no_neutral_crate_names_claude_code`. Expected: FAIL naming `crates/tagteam-core/src/lib.rs:<n>: const _LEAK: &str = "Claude";`. Revert with `git checkout -- crates/tagteam-core/src/lib.rs`.
  2. Plant a dependency. Add `tagteam-cc.workspace = true` under `[dependencies]` in `crates/tagteam-engine/Cargo.toml`. That is not a held file, and it adds no cycle: `tagteam-cc` does not depend on the engine, and the engine already lists it under `[dev-dependencies]`. Then run `cargo test -p tagteam-fake --test neutrality no_neutral_crate_depends_on_tagteam_cc`, without `--locked`, because `Cargo.lock` may change. Expected: FAIL naming `tagteam-engine`. Revert with `git checkout -- crates/tagteam-engine/Cargo.toml Cargo.lock`.
  3. Plant a binary leak. Run `printf '\nconst _LEAK: &str = "tagteam_cc";\n' >> crates/tagteam/src/main.rs` (production, no test half), then `cargo test -p tagteam-fake --test neutrality only_the_composition_root_names_tagteam_cc_in_the_binary`. Expected: FAIL naming `crates/tagteam/src/main.rs:<n>`. Revert with `git checkout -- crates/tagteam/src/main.rs`.
  4. Confirm the reverts. `git status --short` should show only `crates/tagteam-fake/tests/neutrality.rs`. Run `cargo test -p tagteam-fake --test neutrality` again. Expected: PASS.
- [ ] **Step 4: Verify the whole workspace with the three CI commands.**
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - `cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`
  - `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`

  Expected: clean, and all tests pass; report totals as passed/total. The pty tests (`tagteam` `prompt::tests`, `tests/signals.rs`, `tests/tui_pty.rs`, and the TUI's first-frame timing in `tests/perf.rs`) fail with `openpty: Operation not permitted` inside a sandbox. Run those from an unsandboxed terminal, as `CONTRIBUTING.md` says, and report them separately.
- [ ] **Step 5: Run the Linux suite (spec §3.1).** Run the full test command in the `rust:1.88` Docker repro, as `CONTRIBUTING.md` "Linux tests run in Docker" describes: a non-root user with a passwd entry, `HOME` outside `/tmp`, `--init`, and the docker call unpiped. Expected: all pass, reported as passed/total.
- [ ] **Step 6: Check the R1 records.**
  - `CHANGELOG.md` `[Unreleased]` has Task 9's and Task 15's entries and nothing else from R1.
  - `rg -n -i 'claude|anthropic' crates/tagteam-{provider,core,engine,daemon}/src` matches only comments and `#[cfg(test)]` halves; the test is the authority.
- [ ] **Step 7: Commit (controller).** Message: `Test that no neutral crate names Claude Code and only the composition root names tagteam-cc`

---

## Spec deltas found while planning

1. **§5.3's known sites miss two log lines.** They are `crates/tagteam-engine/src/switch.rs:1764` and `:1770`: an INFO and a WARN that say "a starting Claude Code had/may have overwritten the switched account's identity". The neutrality test would fail on them. Task 7 builds them from `p.display_name()`, and `engine/tests/switch.rs:1042` still matches their unchanged tail.
2. **§5.5 `shell_init.rs` is already neutral.** `crates/tagteam/src/app.rs:1040-1048` builds `Wrapped { id: p.id(), launch: p.launch_command() }` for every registered provider with sessions, and `crates/tagteam/src/shell_init.rs:1-56` names no provider. Task 11 makes no change and re-runs its tests.
3. **§5.4 counts seven cc lock constructors; there are six in production.** They are `crates/tagteam-cc/src/locks.rs:55, 58, 98, 121, 154, 162`. The seventh, `MkdirLockSpec::new`, at `:263`, is in the unit tests. `crates/xtask/src/compat/checks/profile.rs:847` builds CC's storage-write spec too, and keeps the default role ("a lock directory"); only that probe's own WARN line could show it. Task 4 also adds `MkdirLock::role()` (not in the spec), so cc can test its roles.
4. **§5.2's `Engine::recorded_vars()` cannot serve `app.rs:263`.** That call runs inside `Context::from_process` (`crates/tagteam/src/app.rs:231-265`), before any `Engine` exists. Task 6 adds `tagteam_engine::service::recorded_vars(&ProviderRegistry)`, which `Engine::recorded_vars()` wraps and `app.rs:263` calls.
5. **§5.4: the `format_iso8601` re-export at `crates/tagteam-cc/src/usage.rs:15-17` stays.** Its condition ("if nothing else uses it") does not hold. It is still used by:
   - `tagteam-cc/src/usage.rs:56, 156, 241, 256, 276`;
   - `tagteam-cc/tests/usage.rs:8`;
   - `tagteam/tests/daemon_procs.rs:23`, `usage_cli.rs:11`, `displaced_cli.rs:12` and `history.rs:13`;
   - `xtask/src/compat/keychain.rs:400` and `report.rs:463,487`.
6. **§5.6: moving `ImportRecord` alone leaves `read_cswap_v1` without its readers.** It needs the private readers at `crates/tagteam-engine/src/transfer.rs:186-247`, which `read_envelope` also uses. Task 12 moves them beside `ImportRecord` into `tagteam-provider/src/import.rs` with a `FormatError` that `TransferError::Format` takes, so neither crate copies them. This also splits §5.6 into a move task (12) and a wiring task (13).
7. **§5.6: `decode` takes `&ProviderRegistry`, but its only production caller cannot reach one.** That caller (`crates/tagteam/src/app.rs:1626`) holds an `Engine`, whose `registry` is `pub(crate)` (`crates/tagteam-engine/src/engine.rs:55`). Task 13 adds `Engine::registry()`, and Task 14's `config` fallback uses it too.
8. **§5.5 test probes: a private function instead of a `#[cfg(test)]` accessor.** Task 11 returns the walls through `walled()`, which production `engine()` calls and drops; the unit tests in the same module call it. Production's signature becomes `(Engine, ProviderId)` as intended, without a cfg-gated API. Flagged for the reviewer.
9. **§5.4 `Timeout(TimedOut)`: the path is bound or built in more places than the `Timeout(_)` patterns.** Production binds it at `crates/tagteam-engine/src/recover.rs:147` and builds it at `crates/tagteam-provider/src/flock.rs:119` and `crates/tagteam-cc/src/locks.rs:64`. Tests bind it at `crates/tagteam-cc/src/locks.rs:534,562`, `crates/tagteam-cc/tests/live_store.rs:1594`, `crates/tagteam-cc/tests/session.rs:689,793` and `crates/tagteam-provider/src/mkdir_lock.rs:739`. All change mechanically (Task 4).
10. **§5.8's optional riders are all in test code.** Every `lock().unwrap()` in `crates/tagteam-cc/src/locks.rs:529,539`, `crates/tagteam-provider/src/provider.rs:958,980`, `crates/tagteam-provider/src/atomic.rs:468,488`, `crates/tagteam/src/transfer_cmd.rs:280,307` and `crates/tagteam/src/logging.rs:263,282` is unit-test code. Task 3 takes none.
11. **§12's test-edit list is narrower than R1's mechanical and C-3 edits.** R1 also edits tests for its signature changes: the `held_reservation`, `notices`, `list_human` and `validate_profile` arities; `Timeout`/`ShadowingItem`/`UnknownProvider`/`NoLiveLogin` shapes; `statusline::engine`'s tuple; and `decode`'s registry. C-3 changes expectations at `crates/tagteam-engine/tests/settings.rs:39, 128, 1124, 1367, 1414` and `crates/tagteam/src/app.rs:2641`. `settings::inspect` gains a `fallback` parameter (`crates/tagteam-engine/src/settings.rs:686`), so `config get`/`list` keep printing the effective id while `Settings::load` keeps its signature, as C-3 requires. Each is listed in its task.
12. **§5.1's "about 126 test lines" counts every line.** 126 is all lines naming the two fields, production included (`rg -c 'claude_config_dir|claude_securestorage_config_dir' crates` totals 126); about 85 of them are tests.
13. **§5.3's `statusline.rs:196` comes from §5.5.** The hint comes from `statusline_setup()` and `display_name()` (Task 11), not from a separate §5.3 edit.
14. **C-9 with several providers.** `UnknownProvider` joins the registry's ids with ", ", so a two-provider build would say "such as claude-code, fake-agent". With one provider the text is byte-identical.
15. **§5.9's "followed by `mod`" needs to allow a visibility.** `crates/tagteam-tui/src/lib.rs:25-26` and `crates/tagteam-core/src/tokens/mod.rs:17-18` put `pub(crate)` before `mod`. The test treats an optional visibility, and attribute lines between, as part of that rule (Task 17).
