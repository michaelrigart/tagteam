# `keychain.file_fallback` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Implemented — https://github.com/michaelrigart/tagteam/pull/68. This plan is complete; do not run it again.

**Goal:** An opt-in setting that makes tagteam, on macOS, refuse a credential write the Keychain
refuses instead of storing the secret in plain text.

**Architecture:** The engine reads `keychain.file_fallback` and hands the provider a `Fallback`
policy on every live write; under `Refuse` the Claude Code provider fails before any fallback
step. Before a switch or a live-token refresh the engine probes the account's own vault Keychain
item, so a refusal comes before anything is saved or a token is spent. The engine counts refusals
and the daemon's keychain backoff reads the count.

**Tech Stack:** Rust workspace (`tagteam-provider`, `tagteam-cc`, `tagteam-fake`,
`tagteam-engine`, `tagteam-daemon`, `tagteam`), `toml_edit`, the `FakeKeychain` double.

**Spec:** `docs/superpowers/specs/2026-11-27-tagteam-keychain-fallback-setting-design.md`, cited
as "spec §n". Line numbers are as of `b223d07c`.

## Global Constraints

- With the default (`true`) no behaviour and no output changes: every existing test passes
  unchanged except where a signature changed or a test pins the list of every settings key
  (Task 1 names those).
- A test that sets the setting in a fixture writes `config.toml` and then calls
  `engine.reload_settings(&provider)`: only the daemon's `SettingsJob` reloads the engine's
  settings by itself.
- Under `Fallback::Allow` every log line keeps its text word for word
  (`crates/tagteam-engine/tests/log_lines.rs:747-768` pins it).
- Linux never reads the setting and never probes.
- Tests use the doubles only (`FakeKeychain`, temp homes). Nothing touches the login keychain.
- Each new test's name says the regression it catches, and passes the revert check: revert the
  change, see it fail, restore it.
- The error text, verbatim: "the Keychain refused a write and `keychain.file_fallback` is off, so
  the credential was not stored; run tagteam from the desktop login session, or allow the
  fallback with `tagteam config set keychain.file_fallback true`". The provider's variant names
  its item after "refused a write": "refused a write to {item}".
- The error kind, verbatim: `keychain-fallback-refused`.
- Logs name a Keychain service or an `rc`, never bytes (core §14.2).
- No explanatory comments beyond the doc comments the surrounding code carries; match its style.
- Commands: focused tests with `cargo test -p <crate> --locked --features tagteam/test-support
  <filter>` (drop the feature flag for crates other than `tagteam`); the final gate is the six
  commands of `CONTRIBUTING.md` lines 36–41.

## Review Focus

1. A user sets `false` on a Mac whose login already lives in `.credentials.json` with no Keychain
   item: a switch from the desktop session must still succeed (the Keychain write creates the
   item). Test in Task 3.
2. A user sets `false` on Linux by copying a dotfile: nothing changes and no probe write is made.
   Tests in Tasks 2 and 4.
3. The setting is written as a string (`"false"`) or `0` in `config.toml`: it reads as `false`,
   as every boolean key does. Test in Task 1.
4. The setting is flipped while the daemon runs: the next run follows it. Test in Task 5.
5. A refused switch to an API-key account: `~/.claude.json` is byte-identical afterwards,
   `customApiKeyResponses` included. Test in Task 2.

---

### Task 1: The setting

**Depends on:** nothing.

**Files:**
- Modify: `crates/tagteam-engine/src/settings.rs` (`KeyId`, `KEYS` after `notify.events`,
  `Settings`, `Settings::value`, `Settings::apply`, the defaults)
- Test: `crates/tagteam-engine/tests/settings.rs`, `crates/tagteam/tests/config_cli.rs`. Both
  pin every key and gain the new one: `config_cli.rs:103-175` (the list and its JSON), and
  `settings.rs` near 1165 (shapes), 1301 (defaults) and 1977 (`SAMPLES: [_; 19]` becomes 20)
- Modify: `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md` §6.4 (the key's row,
  marked "amended by the keychain-fallback spec")

**Interfaces:**
- Produces: `KeyId::KeychainFileFallback`; the key `keychain.file_fallback`, `KeyKind::Bool`,
  `per_provider: false`; `pub keychain_file_fallback: bool` on `Settings`, default `true`.

- [ ] **Step 1: Write the failing tests** in `tests/settings.rs`:
  - `keychain_file_fallback_defaults_to_true_and_reads_every_boolean_spelling`: no file gives
    `true`; `[keychain]\nfile_fallback = false`, `= "false"` and `= 0` each give `false`; an
    invalid value (`"maybe"`) gives `true` and one warning naming `keychain.file_fallback`.
  - `keychain_file_fallback_is_refused_in_a_provider_table`: `config_set` with
    `--provider claude-code` returns `SettingsError::NotPerProvider`.
  And in `tests/config_cli.rs`, `config_set_and_unset_round_trip_keychain_file_fallback`:
  `config set keychain.file_fallback false` writes `[keychain]\nfile_fallback = false`;
  `config get` prints `false`; `unset` removes the table.
- [ ] **Step 2: Run them; expect failures** (`unknown key`).
- [ ] **Step 3: Add the key**, following `KeyId::NotifyEnabled` at each place the compiler names.
- [ ] **Step 4: Run the two test files; expect PASS.** `every_key_id_is_in_the_registry_once`
  passes too.
- [ ] **Step 5: Amend core §6.4 and commit** ("Add the keychain.file_fallback setting").

### Task 2: The `Fallback` policy and the refused write

**Depends on:** nothing.

**Files:**
- Modify: `crates/tagteam-provider/src/provider.rs` (`Fallback`, `ProviderError::FallbackRefused`,
  `Provider::write_credential`), `crates/tagteam-provider/src/lib.rs` (re-export)
- Modify: `crates/tagteam-cc/src/live.rs` (`write_credential_entry`, `write_entry`,
  `write_managed_key`), `crates/tagteam-cc/src/provider.rs:505`
- Modify: `crates/tagteam-fake/src/provider.rs:513`, `crates/tagteam-fake/tests/provider.rs:191,250`
- Modify: `crates/tagteam-engine/src/switch/transact.rs:306,668`, `active.rs:672` (pass
  `Fallback::Allow(&mut before_fallback)`; behaviour unchanged in this task)
- Modify: `crates/tagteam-engine/src/error.rs` (`kind()`, and its kind table tests)
- Test: `crates/tagteam-cc/tests/live_store.rs`, `crates/tagteam-cc/tests/provider.rs`

**Interfaces:**
- Produces:
  ```rust
  pub enum Fallback<'a> { Allow(BeforeFallback<'a>), Refuse }
  // ProviderError
  #[error("the Keychain refused a write to {item} and `keychain.file_fallback` is off, so the credential was not stored; run tagteam from the desktop login session, or allow the fallback with `tagteam config set keychain.file_fallback true`")]
  FallbackRefused { item: String },
  // Provider
  fn write_credential<'l>(&self, env: &Env, locks: &'l LiveLocks<'_>, target: &StoredLogin, fallback: Fallback<'_>) -> Result<Written<'l>, ProviderError>;
  ```
  `EngineError::Provider(ProviderError::FallbackRefused { .. })` has kind
  `keychain-fallback-refused`. `LiveStore::write_credential_entry` and `write_managed_key` take
  `Fallback<'_>` in place of `BeforeFallback<'_>`.

- [ ] **Step 1: Write the failing tests** in `tests/live_store.rs` (macOS platform, fixture as
  `file_fallback_requires_the_shadowing_item_to_be_gone` at line 476):
  - `a_refused_login_write_leaves_the_item_the_file_and_the_pin_alone`: item `b"old"`,
    `set_fail_write(svc, true)`, `Fallback::Refuse` → `Err(FallbackRefused { item })` with
    `item == svc`; the item is still `b"old"`; `credentials_file` does not exist;
    `!s.file_mode_pinned()`.
  - `a_refused_login_write_with_an_existing_file_leaves_its_bytes`: same, with
    `.credentials.json` holding `b"file"` beforehand; its bytes are unchanged.
  - `a_refused_api_key_write_leaves_the_item_and_no_primary_api_key`: managed-key item present,
    write fails, `Refuse` → `FallbackRefused`; the item is unchanged; the global config has no
    `primaryApiKey`.
  - `a_refused_write_under_a_pin_is_refused_too`: pin the file mode with an `Allow` fallback,
    then write with `Refuse` → `FallbackRefused`, the file unchanged.
  - `linux_ignores_the_refuse_policy`: `Platform::Linux`, `Refuse` → `Ok(SecretStore::File(_))`.
  In `tests/provider.rs` (through `ClaudeCode::write_credential`, so `guarded`'s undo runs):
  - `a_refused_api_key_switch_leaves_the_global_config_byte_identical`: a global config with
    other keys and no `customApiKeyResponses`; `Refuse`, failing write → `FallbackRefused`; the
    file's bytes equal the bytes before.
  - `a_refused_write_never_calls_before_fallback`: covered by construction (`Refuse` carries no
    callback); assert instead that an `Allow` fallback still calls it once (the existing
    fallback tests, re-pointed to `Fallback::Allow`).
- [ ] **Step 2: Run them; expect compile failures.**
- [ ] **Step 3: Add `Fallback` and `FallbackRefused`; change the trait and the three
  implementations.** In `write_entry` and `write_managed_key`, match the policy at the point the
  Keychain write has failed (and, in `write_entry`, when the pin is set): `Refuse` returns
  `FallbackRefused { item: svc }` before `report_item`; `Allow(before)` continues as today, its two WARN
  lines unchanged. Under `Refuse` the WARN line is "keychain write failed, and the fallback is
  off" with the same `rc` field. The engine call sites pass `Allow`.
- [ ] **Step 4: Add the kind** in `EngineError::kind()` and its two table tests
  (`error.rs:701`, `:765`).
- [ ] **Step 5: Run `cargo test --locked` for `-p tagteam-cc`, `-p tagteam-fake` and
  `-p tagteam-engine`; expect PASS.**
- [ ] **Step 6: Commit** ("Let a credential write refuse the Keychain fallback").

### Task 3: The engine applies the setting and reports the refusal

**Depends on:** Task 1, Task 2.

**Files:**
- Modify: `crates/tagteam-engine/src/engine.rs` (the refusal count),
  `switch/transact.rs:306`, `switch/rollback.rs:50` (`Rollback::fail`), `active.rs:634-686`
  (`publish`)
- Test: `crates/tagteam-engine/tests/switch_rollback.rs`, `tests/storage_write.rs` or the file
  that holds the refresh tests (`rg -n "PersistedNotPublished" crates/tagteam-engine/tests`)

**Interfaces:**
- Consumes: `Settings::keychain_file_fallback`, `Fallback`, `ProviderError::FallbackRefused`.
- Produces:
  ```rust
  impl Engine {
      /// How many writes this engine has refused under `keychain.file_fallback = false`.
      pub fn fallback_refusals(&self) -> u64;
      /// `Refuse` when the setting is off, else `Allow(before)`.
      pub(crate) fn fallback<'a>(&self, before: BeforeFallback<'a>) -> Fallback<'a>;
      /// Counts `e` when its kind is `keychain-fallback-refused`.
      pub(crate) fn note_refusal(&self, e: &EngineError);
  }
  ```

- [ ] **Step 1: Write the failing tests** (macOS engine fixture of `tests/common/mod.rs`, the
  setting written to the fixture's `config.toml` and `reload_settings` called):
  - `a_refused_switch_reports_the_refusal_and_not_a_rollback`: setting `false`, the live item's
    service set to fail writes → `Err(e)` with `e.kind() == "keychain-fallback-refused"`; the
    live item, `.credentials.json`, and the journal are as before; `fallback_refusals() == 1`.
  - In `rollback.rs`'s unit tests, for the pure function below:
    `a_clean_rollback_of_a_refusal_keeps_the_refusal` (kind `keychain-fallback-refused`),
    `a_clean_rollback_of_another_cause_is_rolled_back`, and
    `a_failed_rollback_of_a_refusal_is_rollback_failed`.
  - Add `assert_eq!(engine.fallback_refusals(), 0)` to the existing
    `a_rolled_back_fallback_leaves_the_next_switch_on_the_keychain` (`switch_rollback.rs:409`).
  - `a_switch_from_a_file_login_with_the_setting_off_creates_the_item` (Review Focus 1): no
    item, login in `.credentials.json`, writes succeed → `SecretStore::Keychain`. It guards
    against a refusal that fires without a refused write, and passes before the change too.
  - `a_refused_publish_is_not_published_and_counted`: a refresh whose publish write is refused
    under `false` → `ActiveOutcome::PersistedNotPublished`, `fallback_refusals() == 1`, no
    `.credentials.json` created.
- [ ] **Step 2: Run them; expect failures.**
- [ ] **Step 3: Implement.** `fallback()` reads `self.settings()`. Both call sites build the
  policy with it. `Rollback::fail` takes its returned error from a pure function
  `fn rolled_back(cause: EngineError, failed: Vec<String>) -> EngineError` in `rollback.rs`:
  with `failed` empty and `cause` an `EngineError::Provider(ProviderError::FallbackRefused { .. })`
  it returns `cause`; otherwise what `fail` returns today. The log lines are unchanged.
  `Rollback` holds no engine, so `transact` calls `note_refusal(&cause)` at `transact.rs:322`
  before `tx.fail(cause)`; `publish`'s `not_published` calls it too. `note_refusal` counts a
  `RestoreFailed` whose `cause` is a `FallbackRefused` as well: the write was refused either way.
- [ ] **Step 4: Run `cargo test -p tagteam-engine --locked`; expect PASS.**
- [ ] **Step 5: Commit** ("Refuse the Keychain fallback when keychain.file_fallback is off").

### Task 4: The write probe

**Depends on:** Task 3.

**Files:**
- Modify: `crates/tagteam-engine/src/vault.rs` (`VaultBackend`, `KeychainVault`, `Vault`),
  `crates/tagteam-engine/src/error.rs` (`EngineError::KeychainFallbackRefused`),
  `switch/transact.rs:262` (after `preflight`, before `settle_pre`),
  `active.rs` (`run_active_refresh`, after `active_quarantine` and before `reconcile_active`,
  which may persist a generation), `refresh.rs:569` (the refresh gate)
- Test: `crates/tagteam-engine/tests/vault.rs`, `tests/switch.rs`, the refresh tests' file

**Interfaces:**
- Consumes: `Engine::fallback_refusals`, `note_refusal`, `Settings::keychain_file_fallback`.
- Produces:
  ```rust
  // VaultBackend, default `Ok(true)`; KeychainVault rewrites the item with its own bytes.
  /// Whether the backend takes a write of `key`'s item as it is. `Err` when the item cannot
  /// be read; `Ok(true)` when it is absent or the backend has nothing to probe.
  fn probe_write(&self, key: &str) -> Result<bool, VaultError>;
  impl Vault { pub fn probe_write(&self, lock: &AccountLock) -> Result<bool, VaultError>; }
  // EngineError, kind `keychain-fallback-refused`, text per Global Constraints.
  KeychainFallbackRefused,
  impl Engine {
      /// Spec §3.4: with the setting off, refuses unless `lock`'s vault item takes a write.
      /// `Ok` when the item is unreadable or absent: the operation then ends with the error
      /// it has today.
      pub(crate) fn probe_keychain_write(&self, lock: &AccountLock) -> Result<(), EngineError>;
  }
  ```
  `Vault::probe_write` does not count in `Vault::writes` and logs nothing at INFO.

- [ ] **Step 0: Read the setting fresh (Task 3's review).** Add
  `pub(crate) fn file_fallback_allowed(&self) -> bool` to `Engine`:
  `Settings::load(&self.env, self.default_provider()).0.keychain_file_fallback`. `fallback()`,
  the probe and `doctor` use it in place of `self.settings()`. Test first, in
  `tests/switch_rollback.rs`: `a_fallback_turned_off_after_the_engine_was_built_is_followed`:
  write `[keychain]\nfile_fallback = false` to the fixture's `config.toml` without
  `reload_settings`, fail the live item's writes → kind `keychain-fallback-refused`.
- [ ] **Step 1: Write the failing tests:**
  - `vault.rs`: `a_probe_rewrites_the_item_with_its_own_bytes` (bytes unchanged after `Ok(true)`);
    `a_probe_of_a_keychain_that_refuses_writes_is_false` (`set_fail_write("tagteam", true)`);
    `a_probe_of_an_unreadable_item_is_an_error`; `a_file_vault_never_probes` (`Ok(true)`, the
    file's mtime unchanged).
  - `switch.rs`: `a_refused_probe_stops_a_switch_before_anything_is_saved`: setting `false`, an
    unmanaged live login (so the switch would displace it), `set_fail_write("tagteam", true)` →
    kind `keychain-fallback-refused`; `displaced/` and `rescue/` are empty, the vault items and
    the journal are unchanged; `fallback_refusals() == 1`.
    `with_the_default_a_switch_never_probes`: setting absent, `set_fail_write("tagteam", true)`,
    and a switch that needs no vault write (the outgoing generation already held) → it succeeds.
    Linux (Review Focus 2) is `a_file_vault_never_probes` above: the file vault is Linux's.
  - refresh: `a_refused_probe_sends_no_token_request`, with a Claude Code item and without one:
    setting `false`, `set_fail_write("tagteam", true)`, an expired live token → `Err` of kind
    `keychain-fallback-refused`; the fake HTTP saw no request; the vault generation is unchanged.
  - gate (beside the tests of `tests/gate_persist.rs`):
    `a_refused_probe_sends_no_token_request_for_a_stored_account`: setting `false`,
    `set_fail_write("tagteam", true)`, an inactive account with an expired token, refreshed
    through `refresh_stored` → kind `keychain-fallback-refused`; no HTTP request; `rescue/` is
    empty. `with_the_default_a_stored_refresh_never_probes`: setting absent, same keychain →
    today's outcome for a failed vault write, unchanged.
- [ ] **Step 2: Run them; expect failures.**
- [ ] **Step 3: Implement the probe.** `KeychainVault::probe_write`: `find`; `Absent` →
  `Ok(true)`; `Unreadable` → `Err`; `Present(b)` → `upsert(SERVICE, key, &b)`, `Ok(true)` on
  success, `Ok(false)` on failure with the `rc` at WARN. `probe_keychain_write` returns `Ok(())`
  at once when the setting is `true`, and on a probe `Err`; `Ok(false)` becomes
  `KeychainFallbackRefused`, counted with `note_refusal`. An item over `security`'s line limit
  goes on argv as every vault write of it does (`security.rs:515`); the probe adds no new case.
- [ ] **Step 4: Call it.** Switch: with the target's account lock from `ls.account_locks` (as
  `transact.rs:373` finds it), directly after `preflight` returns. Refresh: with `lock`, inside the pass loop directly after the quarantine check, so it
  precedes both the self-heal `publish` and `request_active`. Refresh gate: with the account's
  lock, directly before `p.refresh` at `refresh.rs:569`, so a refused probe sends no request and
  returns the refusal as the gate's error.
- [ ] **Step 5: Run `cargo test -p tagteam-engine --locked`; expect PASS.**
- [ ] **Step 6: Commit** ("Probe the vault item before a switch or refresh that may not fall back").

### Task 5: The daemon backs off after a refused run

**Depends on:** Task 4.

**Files:**
- Modify: `crates/tagteam-daemon/src/backoff.rs` (`after_run`),
  `crates/tagteam-daemon/src/provider_job.rs:452-467`
- Modify: `docs/superpowers/specs/2026-10-14-tagteam-daemon-design.md` D4.1 (amended)
- Test: the unit tests in `backoff.rs`, the job tests in `provider_job.rs`

**Interfaces:**
- Consumes: `Engine::fallback_refusals() -> u64`.
- Produces: `KeychainBackoff::after_run(&mut self, reads: ReadTally, refused: bool, now_ms: i64)
  -> Option<i64>`.

- [ ] **Step 1: Write the failing tests:**
  - `backoff.rs`: `a_refused_run_backs_off_even_when_its_reads_were_answered` (60 000 ms);
    `a_second_refused_run_doubles_the_delay` (120 000 ms); `a_run_without_a_refusal_and_with_an_
    answered_read_ends_it` (`None`); `refused_runs_stop_doubling_at_fifteen_minutes` (900 000 ms).
  - `provider_job.rs`: `a_refused_auto_tick_delays_the_next_run_by_the_backoff`: setting `false`,
    a tick that decides a switch, `set_fail_write("tagteam", true)` → the job's next delay is at
    least 60 s and `daemon status` shows `backoff`; one `switch-failed` event; a second tick
    inside 30 minutes adds none (spec §3.6).
    `a_setting_flipped_back_ends_the_refusals_at_the_next_run` (Review Focus 4): write `true`,
    call `engine.reload_settings`, clear the `tagteam` failure and fail Claude Code's service
    instead; the next tick switches by the fallback and the backoff ends.
- [ ] **Step 2: Run them; expect failures.**
- [ ] **Step 3: Implement.** `refused` is checked first: it raises the level whatever `reads`
  holds; otherwise today's logic. The job reads `engine.fallback_refusals()` before the run and
  after it, beside `self.reads()`.
- [ ] **Step 4: Run `cargo test -p tagteam-daemon --locked`; expect PASS.**
- [ ] **Step 5: Amend D4.1 and commit** ("Back the daemon off after a refused Keychain fallback").

### Task 6: `doctor`

**Depends on:** Task 1.

**Files:**
- Modify: `crates/tagteam-provider/src/provider.rs:911` (`doctor_checks`),
  `crates/tagteam-cc/src/provider.rs:877`, `crates/tagteam-cc/src/doctor.rs:114`,
  `crates/tagteam-engine/src/doctor/mod.rs:164` (the fake has no override and needs no change)
- Modify: core spec §13.6 (amended)
- Test: `crates/tagteam-cc/tests/doctor.rs` (its helper at line 63 gains the argument, `true`)

**Interfaces:**
- Consumes: `Settings::keychain_file_fallback`.
- Produces: `fn doctor_checks(&self, env: &Env, spawner: &dyn ProcessSpawner, cancel: &Cancel,
  file_fallback: bool) -> Vec<Check>`; the check id `cc.keychain-fallback`.

- [ ] **Step 1: Write the failing tests** in `tests/doctor.rs`:
  - `with_the_default_there_is_no_fallback_check` (no check with that id, on macOS).
  - `with_the_fallback_off_and_no_plain_text_secret_the_check_is_ok`.
  - `with_the_fallback_off_a_login_in_the_credentials_file_warns_and_names_the_file`.
  - `with_the_fallback_off_a_primary_api_key_warns_and_names_the_config`.
  - `the_fallback_check_never_appears_on_linux`.
  The warn message: "{path} holds a {login|API key} in plain text; `keychain.file_fallback` is
  off, which stops new fallbacks and removes nothing". `fix` is `None`.
- [ ] **Step 2: Run them; expect failures.**
- [ ] **Step 3: Implement.** A login is present when `.credentials.json` parses to an object
  with account-scoped keys (the test `keep_shared` already makes in `live.rs`); an API key when
  `read_primary_api_key` is `Present`. The engine passes
  `self.settings().keychain_file_fallback` (the receiver at `doctor/mod.rs:164` is `self`).
- [ ] **Step 4: Run `cargo test -p tagteam-cc --locked --test doctor` and
  `-p tagteam-engine --locked doctor`; expect PASS.**
- [ ] **Step 5: Amend core §13.6 and commit** ("Report plain-text secrets in doctor when the fallback is off").

### Task 7: Documents and the full gate

**Depends on:** Tasks 1–6.

**Files:**
- Modify: `README.md` (the settings list; the credential-storage passage near line 385),
  `SECURITY.md` (near line 61), `CHANGELOG.md` (`## [Unreleased]`, `### Added`), core spec
  Appendix A.3 (amended)

- [ ] **Step 1: Write the documents.** README and SECURITY say: what the setting does, that the
  default mirrors Claude Code's own fallback, that it does not cover session profiles
  (`tagteam run` writes `sessions/<id>/.credentials.json`) or a plain-text login already there,
  that a refusal is fixed by running from the desktop login session, and that an edit is followed at the next write, with no restart. The CHANGELOG entry
  names the setting, the error kind and the `doctor` check.
- [ ] **Step 2: Run the six commands of `CONTRIBUTING.md` lines 36–41.** Expected: no
  formatting diff, no clippy warning, every test passing; report passed/total. The pty tests
  need an unsandboxed run.
- [ ] **Step 3: Commit** ("Document keychain.file_fallback").
