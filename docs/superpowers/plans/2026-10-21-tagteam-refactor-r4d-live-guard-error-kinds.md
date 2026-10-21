# tagteam refactor R4d — the live-login guard and error kinds — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Implemented — https://github.com/michaelrigart/tagteam/pull/34
**Branch:** `refactor-r4d-live-guard-error-kinds`, based on `refactor-r4c-gate-classifier`
**Goal:** One walk for the never-delete-the-live-login guard, and `--json` error kinds that say what each refusal is, with every message unchanged.
**Architecture:**
- Refusals become `DeletionRefused`, and purge's prefix `AtPath` (Task 1).
- `live_guard.rs` holds the walk (Task 2).
- `InvalidInput` refusals are classified (Task 3).
- `lock_error()` and `VaultCause` (Task 4).

**Tech Stack:** Rust 1.88 (edition 2024), thiserror.

**Spec:** docs/superpowers/specs/2026-10-19-tagteam-review-refactor-design.md (§8.4, §8.5; with §2, §3, §8.9 CC-E-21, §11, §12, §16)

**Reading this plan.**
- **Sites** are named by function; R4b and R4c move lines.
- **Focused tests:** `cargo test -p tagteam-engine --locked --features test-hooks` and `cargo test -p tagteam --locked --features test-support`.
- **Order:** one task at a time, in order. All compile `tagteam-engine`; three share `error.rs`.

## Global Constraints

- **Messages stay byte-identical,** path prefixes included. Only §11's R4 `error.type` row changes, noted in `CHANGELOG.md` (Task 4). D-3: no kind is renamed or removed.
- **Log lines stay in their modules,** since a line's target is its module.
- **Files not to edit:** hardening's files (§3.2) and the held TUI files. `vault.rs` is edited on top of H5.3's `Vault::store`.
- **The safety net passes unchanged:**
  - engine `tests/purge.rs`, `displaced.rs`, `replacement_unreadable.rs` and `manage.rs`;
  - `tui_host.rs::remove_refuses_to_delete_what_the_live_login_lives_in`.

  New tests may join them.
- **Test edits:** only new tests and Task 3's four kind pins. No new `#[allow]`.
- **Placement:** unit-test modules stay last, with `#[cfg(test)]` alone above `mod tests {`. Comments wrap at 100 columns and cite spec sections (§14's sweep is R4g's).
- **CI** (`CONTRIBUTING.md`): fmt; both clippy variants in every task; the workspace test command.
- **Commits:** imperative, sentence case, no prefix, no trailing period, no agent attribution.

## Review Focus

1. **A listing that fails between `purge_plan` and `purge`** must refuse before anything is deleted, not narrow the check. Task 2: `a_displaced_listing_that_fails_after_the_plan_refuses_the_provider_purge`.
2. **A live file that links from a purged directory into a spared profile** still refuses. A chain per file would spare it. Task 2: `each_touched_path_is_judged_on_its_own_chain`.
3. **Per-scope wording** ("break"/"delete", "it"/"them") is pinned exactly. Task 1: `every_deletion_refusal_keeps_its_text`.
4. **A purge refusal that names its entry** keeps `<path>: ` under `--json`, with its inner kind. Task 1: the `purge_cli.rs` test.
5. **A wrapped interruption or lock failure:** `signal()` and `kind()` see through `AtPath` and `Provider(Lock(..))`. Task 1's `AtPath` signal pin; Task 4's carrier test.

---

### Task 1: Report refused deletions as `deletion-refused`, and keep the inner kind behind a path

**Depends on:** none.

**Files:**
- Modify: `crates/tagteam-engine/src/error.rs`, `lifecycle.rs`, `purge.rs`.
- Test: `error.rs`'s tests, `crates/tagteam/tests/purge_cli.rs`, `crates/tagteam/tests/cli.rs`.

**Interfaces (produces):** in `error.rs`.

```rust
/// Which guard refused, which decides its wording (§8.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardScope { ProfileEntry, Purge } // refuse_live_files_at; refuse_live_in_envelope
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionRefusal {
    /// `position`: the account whose profile names the item; `None` for an orphan entry.
    LiveLoginKeychain { var: &'static str, position: Option<u32> },
    LiveLoginUnresolved(GuardScope),
    LiveLoginInside(GuardScope),
    StoredAccountKeychain,
    StoredAccountUnresolved,
}
impl fmt::Display for DeletionRefusal { .. } // each site's current text, moved verbatim
// EngineError:
#[error("{why}")] DeletionRefused { why: DeletionRefusal },      // "deletion-refused"
#[error("{}: {source}", path.display())]
AtPath { path: PathBuf, source: Box<EngineError> },              // source's kind() and signal()
impl EngineError { pub(crate) fn at_path(self, path: &Path) -> EngineError; }
```

- [ ] **Step 1: Write the tests.** They must fail to compile.
  - **`error.rs`, `every_deletion_refusal_keeps_its_text`.** All eight forms' `to_string()` equal their site's text at `8d44998`:
    - `LiveLoginKeychain` with `var: "CLAUDE_CONFIG_DIR"`: with `Some(2)`, `refuse_live_item`; with `None`, `refuse_orphan_items_live`;
    - both scopes: `ProfileEntry` is `refuse_live_files_at`, `Purge` is `refuse_live_in_envelope`;
    - the stored-account forms: `refuse_orphan_items_stored`, `stored_spellings`.
  - **`error.rs`, `kind_is_pinned_for_every_variant`.** New rows:
    - every `DeletionRefusal` gives `"deletion-refused"`;
    - `AtPath` gives its inner error's kind, over `DeletionRefused`, `Io` and `Store(StoreError::NoSuchAccount)`.
  - **`error.rs`, `signal_reads_every_carrier_of_an_interruption`.** `AtPath` over `Interrupted(2)` has signal `Some(2)`.
  - **`purge_cli.rs`, `a_purge_that_would_delete_the_live_login_reports_deletion_refused_with_the_entry`.**
    - Setup: `two_fresh_accounts`, then `{"oauthAccount":{}}` in `<data>/displaced/live/.claude.json`, where `<data>` is `Env::for_test(root).data_dir()`.
    - Run `purge --yes --json` with `CLAUDE_CONFIG_DIR` set to that `live` directory.
    - It exits 1, and stdout is exactly `{"schemaVersion":1,"error":{"type":"deletion-refused","message":"<data>/displaced: the live login's files are inside what this would delete, since the environment names it; nothing was deleted (tagteam never deletes the live login)"}}`.
    - `a`'s vault item and the live file remain.
  - **`cli.rs`, `remove_reports_deletion_refused_when_the_live_login_lives_in_the_profile`.**
    - Setup: `two_accounts`, and `spelling` from `cc_profile(root, &a)`.
    - Run `remove 1 --json` with `CLAUDE_SECURESTORAGE_CONFIG_DIR=spelling`.
    - It exits 1 with the same JSON, minus the prefix. `a`'s row remains.
    - The fixture is `tests/tui_host.rs::remove_refuses_to_delete_what_the_live_login_lives_in`'s, which pins this file-guard text today through `CliHost` with only the secure-storage directory set; this test runs it through the binary with `--json`.
- [ ] **Step 2: Implement.**
  - The eight `Io(io::Error::other(..))` refusals in those functions become `DeletionRefused`; `var` is the fallback each already computes.
  - The four rewraps become `.map_err(|e| e.at_path(&x))`: `purge`'s orphan loop, `refuse_live_in_targets`, and both loops of `purge_without_data`.
  - Conversions that feed `report.failures` stay.
- [ ] **Step 3: Verify.** Focused commands, both clippy variants, the safety net.
- [ ] **Step 4: Commit** "Report refused deletions as deletion-refused and keep the inner kind behind a path".

### Task 2: Share the live-login walk, and propagate purge's listing errors

**Depends on:** Task 1.

**Files:**
- Create: `crates/tagteam-engine/src/live_guard.rs`.
- Modify: `lib.rs`, `lifecycle.rs` and `purge.rs`, at the functions named in the steps.
- Test: `live_guard.rs`, and one new test in `tests/purge.rs`.

**Interfaces (produces):**

```rust
pub(crate) enum LiveGuardError { Unresolved(GuardScope), Inside(GuardScope) }
impl From<LiveGuardError> for EngineError { .. } // DeletionRefused { LiveLoginUnresolved | LiveLoginInside }
/// `p`'s identity surface: its credential files, then its JSON files.
pub(crate) fn live_files(p: &dyn Provider, env: &Env) -> Vec<PathBuf>;
/// Every path `trace_path` touches is judged on its own chain once it is built: `Inside(scope)`
/// when `inside(&chain)`. A `trace_path` failure is `Unresolved(scope)`.
pub(crate) fn refuse_inside(files: &[PathBuf], scope: GuardScope,
    inside: impl Fn(&[FileId]) -> bool) -> Result<(), LiveGuardError>;
/// `path`, then each ancestor: its `symlink_metadata` id, then its `metadata` id when that
/// resolves. Absent ends the chain; any other `symlink_metadata` failure is `Unresolved(scope)`.
fn touched_chain(path: &Path, scope: GuardScope) -> Result<Vec<FileId>, LiveGuardError>;
// purge.rs
fn deletion_targets(&self, plan: &PurgePlan, rows: &[AccountRow]) -> Result<Vec<PathBuf>, EngineError>;
```

- [ ] **Step 1: Write the tests.** They must fail to compile.
  - **`live_guard.rs`'s tests,** each in a temporary directory `t`. `id(p)` is a path's unfollowed `FileId`, and `fid(p)` its followed one.
    - **`a_chain_holds_each_ancestor_unfollowed_and_followed`.** With `t/real/x` and a link `t/link → real`, the chain of `t/link/x` starts with `id(t/real/x)`, and holds `id(t/link)`, `fid(t/link)` and `id(t)`.
    - **`an_absent_entry_ends_the_chain`.** `touched_chain(t/none/x, Purge)` is `Ok(vec![])`.
    - **`an_unreadable_ancestor_is_unresolved_in_its_scope`.** With `t/shut` at mode `0o000`, the chain of `t/shut/x` is `Err(Unresolved(ProfileEntry))`.
    - **`each_touched_path_is_judged_on_its_own_chain`.** Take a link `t/wall/live → t/spared/f`, and `inside = |c| c.contains(&fid(t/wall)) && !c.contains(&fid(t/spared))`. Then `refuse_inside` over `[t/wall/live]` is `Err(Inside(Purge))`, and over `[t/spared/f]` is `Ok(())`.
  - **`tests/purge.rs`, `a_displaced_listing_that_fails_after_the_plan_refuses_the_provider_purge`** (test-hooks):
    - Add `a`, then `plant_displaced(&fx, &fx.provider(), "1790000000-aaaaaaaaaaaa-abc123", "x@x.co")`.
    - Take the plan for that provider.
    - A `purge-guarded` hook sets `data/displaced` to mode `0o000`.
    - `purge` fails with kind `"io"`, and `fx.vault_bytes(&a)` is `Some`.
- [ ] **Step 2: Implement `live_guard.rs`.**
- [ ] **Step 3: `refuse_live_files_at`.**
  - It keeps its location and doomed-set code. Its early refusals become `Unresolved(ProfileEntry)`.
  - The walk becomes `refuse_inside(&live_files(..), ProfileEntry, |c| c.iter().any(|id| doomed.contains(id)))`.
  - On `Inside`, it logs its WARN line.
- [ ] **Step 4: `refuse_live_in_envelope`.**
  - Extract `envelope_walls(&self, plan)` and `spared_ids(&self, store, rows) -> Spared { profiles, on_the_way }`, each fallible.
  - Keep the order: walls (none: `Ok`), then `refuse_live_vault_files`, then spared ids.
  - For each provider, call `refuse_inside` with `Purge` and today's rule: a chain touching a wall refuses, unless it touches a spared profile, or its first id is on the way to one.
  - On `Inside`, it logs purge's WARN line.
- [ ] **Step 5: CC-E-20.**
  - `deletion_targets` propagates `displaced_of(provider)?`.
  - It maps a `rescue_paths_for` error to `RescueUnlistable { path, detail }`, as `remove_unregistered` does.
  - `refuse_live_in_targets` propagates that unwrapped; only `refuse_live_at_all`'s errors get a path.
  - `a_rescue_file_is_deleted_by_a_full_purge_and_refused_by_remove_and_a_provider_purge` passes unchanged.
- [ ] **Step 6: Verify.** The engine focused command, `tagteam`'s `--test tui_host --test purge_cli --test cli`, both clippy variants, the safety net.
- [ ] **Step 7: Commit** "Share the live-login walk between remove and purge, and refuse when purge cannot list its targets".

### Task 3: Classify the remaining state refusals, and name a missing profile marker

**Depends on:** Task 1.

**Files:**
- Modify: `error.rs`, plus each site the table below moves.
- Test edits: four kind pins, in engine `tests/`:

  | Test | New kind |
  |---|---|
  | `active.rs::an_unmanaged_live_login_is_refused` | `unmanaged-account` |
  | `launch.rs::a_target_whose_stored_credential_went_away_before_the_locks_is_refused_under_them` | `relogin-required` |
  | `rotation.rs::a_target_emptied_while_a_direct_switch_waits_reports_no_stored_credential` | `relogin-required` |
  | `profiles.rs::a_profile_whose_marker_or_record_cannot_be_read_is_not_synced`, first pin | `profile-marker-missing` |

**Interfaces (produces):** new `EngineError` variants.

```rust
#[error("{} has no profile marker", path.display())]
ProfileMarkerMissing { path: PathBuf },                          // "profile-marker-missing"
#[error("{} is missing, so the profile's outer home is unknown", marker.display())]
OuterHomeUnknown { marker: PathBuf },                            // "profile-marker-missing"
#[error("{0}")] NoStoredCredential(String),                       // "relogin-required"
#[error("the live login is not managed by tagteam; only {app} refreshes it")]
UnmanagedLiveLogin { app: &'static str },                        // "unmanaged-account"
```

**The classification** (§8.5; spec sites are as of `683b266`):

| Spec site | Now | Kind |
|---|---|---|
| `launch.rs:238` | `recorded_spelling` | `profile-marker-missing` |
| `bootstrap.rs:314` | `bootstrap_profile` (marker) | `profile-marker-missing` |
| `profiles.rs:95` | `sync_profile_links` | `profile-marker-missing` (`OuterHomeUnknown`) |
| `switch.rs:1264` | `plan.rs::plan` | `relogin-required` |
| `switch.rs:1943` | `transact.rs::read_target` | `relogin-required` |
| `bootstrap.rs:165` | `vault_generation` | `relogin-required` |
| `active.rs:438` | `active_login` (unmanaged) | `unmanaged-account` |
| `active.rs:444` | `active_login` (not refreshable) | stays `invalid-input` |
| `bootstrap.rs:117`, `:343` | `own_marker`; `bootstrap_profile` (generation) | stay |
| `switch.rs:1335` | `freshen.rs::freshen_plan` | stays |
| `profiles.rs:323`, `:329`, `:557` | `check` ×2, `link_to_nothing` | stay |

- [ ] **Step 1: Write the test.** Add the four variants and their kinds to `kind_is_pinned_for_every_variant`. It must fail to compile.
- [ ] **Step 2: Implement.**
  - Each moved site builds its variant with its current text. The three `NoStoredCredential` sites keep their own `format!`.
  - Then edit the four pins.
- [ ] **Step 3: Verify.** Focused commands, both clippy variants.
- [ ] **Step 4: Commit** "Report a missing profile marker, a missing credential and an unmanaged live login by their own kinds".

### Task 4: One `lock_error()`, the vault's Keychain errors kept, and the record

**Depends on:** Task 3.

**Files:**
- Modify: `error.rs`, `vault.rs`, `CHANGELOG.md`, and `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`.
- Test: `error.rs`, and the engine's `tests/vault.rs`.

**Interfaces (produces):**

```rust
impl EngineError {
    /// The lock failure behind this error, whichever carrier holds it: `Lock`,
    /// `Provider(ProviderError::Lock)` or `Settings(SettingsError::Lock)` (§8.5).
    fn lock_error(&self) -> Option<&LockError>;
}
/// What a vault write or delete failed on (§8.5).
#[derive(Debug, thiserror::Error)]
pub enum VaultCause {
    #[error(transparent)] Keychain(#[from] KeychainError),
    #[error(transparent)] Io(#[from] io::Error),
    #[error("{0}")] Check(String),
}
// VaultError::Write(#[source] VaultCause), VaultError::Delete(#[source] VaultCause); texts unchanged
```

- [ ] **Step 1: Write the tests.** They must fail.
  - **`error.rs`, `a_lock_failure_reads_alike_whichever_carrier_holds_it`.**
    - For each of `Timeout`, `Compromised`, `Io` and `Interrupted { signal: 2 }`, the carriers `Lock`, `Provider(ProviderError::Lock)` and `Settings(SettingsError::Lock)` give equal `kind()` and equal `signal()`.
    - `Provider(Lock(Compromised))` and `Provider(Lock(Io))` are `"lock"`, also pinned in `kind_is_pinned_for_every_variant`.
  - **`tests/vault.rs`, `a_keychain_failure_stays_the_source_of_a_vault_write_or_delete`.**
    - With `kc.set_fail_write(SERVICE, true)`, `KeychainVault::new(kc).write("k", b"x")` matches `Write(VaultCause::Keychain(KeychainError { rc: Some(25), .. }))`.
    - It reads `"vault write failed: keychain operation failed (rc 25): injected write failure"`, and `Error::source` reads the text after `"failed: "`.
    - Likewise `set_fail_delete` with `delete`, giving `Delete` and `"injected delete failure"`.
- [ ] **Step 2: `lock_error()`.**
  - `kind()` returns `lock_kind(e)` first, for `Some(e)`. The two `Provider(ProviderError::Lock(..))` arms go.
  - `Lock(_) | Settings(SettingsError::Lock(_)) => "lock"` stays, for exhaustiveness.
  - `signal()`: `Interrupted(s)`, then `AtPath` → `source.signal()`, else `lock_error().and_then(LockError::signal)`.
  - `RestoreFailed` keeps its arm.
- [ ] **Step 3: `VaultCause`.**
  - `Keychain`: from `KeychainVault`, and from `sweep`'s `delete_service`.
  - `Io`: from `FileVault`, and from `sweep`'s removal.
  - `Check`: `sweep`'s "cannot be swept", and `delete`'s two verification texts.
- [ ] **Step 4: `CHANGELOG.md`.** Append under `## [Unreleased]` → `### Changed`:

  > - `--json` error types now say what each refusal is; none was renamed or removed. A deletion refused to keep the live login or a stored account's Keychain item reports `deletion-refused`, and a `purge` refusal naming its entry reports its own type (both were `io`).
  > - A session profile without its marker reports `profile-marker-missing`, an account with no stored credential `relogin-required`, and an active-token refresh of an unmanaged live login `unmanaged-account` (all were `invalid-input`). A lock failure under a provider reports `lock` (it was `provider`).
- [ ] **Step 5: §16.** Add the line below to the core spec twice: after §13.2's bullet "Errors produce `{"schemaVersion":1,"error":…}`", and after §14's first bullet.

  > Kind values never change, but a refusal may move to a kind that describes it ([review refactor D-3, §8.5](2026-10-19-tagteam-review-refactor-design.md)), which adds `deletion-refused` and `profile-marker-missing`.
- [ ] **Step 6: Verify.** Focused commands, `cargo test -p tagteam-daemon --locked --features test-hooks`, both clippy variants.
- [ ] **Step 7: Commit** "Read lock kinds and signals through one lock_error and keep the vault's Keychain errors".

## Final verification (the controller, before the PR)

- **macOS:** every CI command, outside the sandbox. Report totals as passed/total.
- **Linux:** `cargo test` in the Docker repro.
- **Untouched:** `git diff --stat refactor-r4c-gate-classifier..HEAD` lists no held TUI file, no `refresh.rs` and nothing in `tagteam-provider`.
- **`io::Error::other`** remains in `lifecycle.rs` and `purge.rs` only for `report.failures`.

## Rulings (recorded under "Rulings" in the PR)

1. **`DeletionRefused` has no `path`.** `AtPath` carries every rewrap's path, since the orphan rewrap also wraps `Io` and `Store` errors.
2. **The shared walk is `refuse_inside`.** `touched_chain` takes one touched path, not the live file: purge's spared rule reads that path's first id. Callers keep their ids and WARN lines.
3. **A touched path is judged once walked,** as purge does now. So where one path is inside and a later one cannot be resolved, `remove` now says inside, not unresolved; both are `deletion-refused`.
4. **A kind moves only to one that describes the refusal exactly** (Task 3's table):
   - `relogin-required` (log in and add again), the spec's `needs-relogin`;
   - `unmanaged-account`, the CLI's kind for this refusal;
   - the new `profile-marker-missing`.
5. **A moved kind with unchanged text gets its own variant,** as `ReservationHeld` shares `launch-unreachable`.
6. **The four tests pinning a moved kind are updated:** they are §8.5's correction, though §12's edit list omits them.
7. **`VaultCause` is an enum,** because `Write` and `Delete` also carry the file backend's `io::Error` and the vault's own checks.
8. **No `str_enum!`:** `DeletionRefusal` and `VaultCause` carry data, and `GuardScope` has no string form. R4e brings it.
9. **CC-E-20 propagates both listings** (`:884`, `:888`). Only a listing that fails after `purge_plan` behaves differently.
10. **§16's core pointer lands with the new kinds** (Task 4).
11. **§12's `profile-marker-missing` through `--json`:** the binary cannot reach it without a race, since every site runs after the marker is written. Task 3 pins it in the engine, and Task 1 proves the JSON path.
