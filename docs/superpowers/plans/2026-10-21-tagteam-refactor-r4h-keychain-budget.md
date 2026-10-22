# tagteam refactor R4h — PERF-R-1: fewer Keychain calls per switch — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Approved
**Branch:** `refactor-r4h-keychain-budget`, based on `refactor-r4g-helpers-comments`
**Goal:** Fewer Keychain calls per manual switch, with the count pinned (§8.11). On the prototype,
`switch <N>` drops from 25 `/usr/bin/security` spawns to 20, and from 20 to 15 under Claude Code's
locks.
**Architecture:**
- **Task 1:** `Vault` counts its writes. A `VaultMemo`, built from `&[AccountLock]` right after
  `lock_accounts`, answers repeat reads of a locked account while that count stands. It reaches
  `rederive`, `read_target`, `settle_outgoing` and `hold_vault` through `LockedSwitch` and
  `Settle`.
- **Task 2:** `clear_managed_key` deletes only what its storage-write read found.
- **Task 3:** a counting `Keychain` pins the totals.

**Tech Stack:** Rust 1.88 (edition 2024).

**Spec:** docs/superpowers/specs/2026-10-19-tagteam-review-refactor-design.md (§8.11, with §1.4,
§3.2, §8.10, §11, §12, §16); core §9.1 and §9.4. This is the last R4 slice.

**Reading this plan.**
- **Sites** are functions at R4g's tip. R4g changes only:
  - email expressions in `switch/mod.rs::login_email` and `switch/plan.rs`;
  - `auto.rs`;
  - the lock-record reader;
  - `jittered`;
  - `Engine::cached_store`;
  - comments.

  Keep what R4g made of every line not named here.
- **Scenario** (§8.11, restated after H5.3):
  - The target `a` is an OAuth account.
  - The outgoing `b` is managed and live. Its live entry is its vault generation plus
    `mcpOAuth`, so its vault form equals the vault while its bytes do not.
  - There is no managed-key item, profile or rescue, and `a` is not due.
- **Recount at `6aeb`,** measured with a counting `Keychain` and counted the way `SecurityCli`
  spawns: an `upsert` is 2, every other call 1.
  - `switch <a>`: 25 spawns, 20 under CC's locks. Bare `switch`: 26 and 20.
  - The 20 under the locks are 7 vault reads (3 of them repeats), 10 live reads that §9.1 keeps
    fresh, and the upsert, the managed-key delete and its probe.
  - The CLI's lock check adds one `show-keychain-info` outside the engine.
- **Michael's ruling (2026-10-22, D-5):** keep item 4. Its delete is the panic point of
  `switch_rollback.rs::a_panic_inside_a_live_write_keeps_the_journal_for_recovery`, so that test gains one
  setup line that plants a managed-key item, and none of its assertions change. The refactor spec records
  it as D-5 and a §12 exception; core §9.1 gains its one sentence.
- **Focused tests:** `cargo test -p tagteam-engine --locked --features test-hooks`;
  `cargo test -p tagteam-cc --locked`.
- **Order:** 1 → 3, one at a time. Tasks 1 and 2 share engine test binaries. Task 3 measures the
  finished switch (§8.10).

## Global Constraints

- **Byte-identical output** (§1.4) for messages, warnings, `--json`, logs, events and stored data.
  Only §11's R4 row changes: one `CHANGELOG.md` entry (Task 3). No wire or schema change.
- **§9.1's freshness rule stays** (§1.4). No live-credential read is cached; the memo serves vault
  reads, and only under the account locks.
- **Hardening's files** (§3.2) are not edited: not `refresh.rs` (whose writes go through
  `Vault::store`), `security.rs`, `keychain.rs`, or any test double. The counting wrapper is new
  and lives in the new test file. `vault.rs` and `cc/live.rs` are edited by both branches and are
  open to R4 now. `tests/common/mod.rs` is not edited.
- **Held files** (§3.4): none edited.
- **Tests:** new ones only, with no existing test edited, except Task 2's one setup line (D-5). No new `#[allow]`.
- **Comments** cite "review refactor §8.11" (a bare "§n" is the core spec, §14) and wrap at 100.
  Unit-test modules stay last. A type that holds credential bytes gets no `Debug`.
- **CI** (`CONTRIBUTING.md`): fmt, both clippy variants every task, and the workspace test
  command. Commits are imperative, in sentence case, with no prefix, no trailing period and no
  agent attribution.

## Review Focus

1. **A stale read never activates a spent generation.** A capture, rescue adoption or profile
   capture under the locks forces a fresh read. Covered by Task 1's capture test, and by the
   unchanged `a_resolved_self_switch_activates_the_generation_it_captured` and `rescue_switch.rs`.
2. **The memo stays within the locks.** It is built per attempt from `&[AccountLock]`. Any
   account outside that slice reads straight through. A replan starts empty. Covered by Task 1's
   unit tests and the unchanged freshen quarantine-under-the-locks test.
3. **Failures read as they do today.** `Unreadable` is never kept, and a failed write still
   invalidates. Covered by Task 1.
4. **Item 4 decides from the storage-write read, never the snapshot.** CC writing the item
   during the wait still aborts with `EntryMoved`. Covered by Task 2's pin.
5. **The budget measures the post-hardening scenario, exactly.** Covered by Task 3.

## Spec coverage

| Spec | Task |
|---|---|
| §8.11 item 1: the write version; the memo from `&[AccountLock]`, never shared; through `LockedSwitch`/`Settle`; `recover.rs`/`active.rs` read uncached | 1 (Ruling 2) |
| §8.11 item 2: `rederive`'s read enters the memo; a write forces a fresh read | 1 |
| §8.11 item 4; §16's core §9.1 sentence | 2 (Rulings 9, 11) |
| §8.11 item 3; §12's budget test; §11's R4 row | 3 |

---

### Task 1: Read each locked vault entry once per switch attempt

**Depends on:** none.

**Files:**
- `crates/tagteam-engine/src/vault.rs`: modify `Vault`, `store` and `delete`; add `VaultMemo` and
  a new last `mod tests`.
- `switch/mod.rs`: `switch_planned`.
- `switch/transact.rs`: `LockedSwitch`, `rederive`, `read_target`, `transact`, `settle_branch`,
  `settle_outgoing`.
- `switch/support.rs`: `Settle`, `hold_vault`, `vault_holds_login`, `has_login`, and a new
  `read_vault`.
- Pass `None` in `plan.rs::plan`, `select.rs::{walk, auto_refusal}` and `freshen.rs::freshen`.
- New test file: `tests/keychain_budget.rs`.

**Interfaces (produces):**

```rust
pub struct Vault { backend: Box<dyn VaultBackend>, writes: AtomicU64 }
// `store` and `delete` bump `writes` before touching the backend, success or not (Ruling 5).

/// Vault reads under one switch attempt's account locks (review refactor §8.11). Writes need the
/// account lock and bump the vault's count; a read kept at an older count is read again. Never
/// kept: `Unreadable`, or an account outside `locks`. Never shared: the daemon's threads share
/// `Vault` and `Engine`. It holds credentials, so it has no `Debug`.
pub(crate) struct VaultMemo<'a> { vault: &'a Vault, locks: &'a [AccountLock], kept: RefCell<Vec<Kept>> }
impl<'a> VaultMemo<'a> {
    pub(crate) fn new(vault: &'a Vault, locks: &'a [AccountLock]) -> Self;
    pub(crate) fn read(&self, id: &AccountId) -> Read<Vec<u8>>;
    pub(crate) fn read_prev(&self, id: &AccountId) -> Read<Vec<u8>>;
}
// private: enum Slot { Current, Prev }; struct Kept { id, slot, at: u64, read: Read<Vec<u8>> }

// transact.rs
pub(super) struct LockedSwitch<'a, 'g> { /* four fields as now */ pub(super) memo: &'a VaultMemo<'a> }
pub(super) fn rederive(&self, p, store, req, plan, outgoing: Option<&AccountRow>,
    memo: &VaultMemo<'_>) -> Result<Rederived, EngineError>;
pub(super) fn read_target(&self, target: &AccountRow, memo: Option<&VaultMemo<'_>>)
    -> Result<Vec<u8>, EngineError>;
// support.rs
pub(crate) struct Settle<'a> { /* as now */ pub(super) memo: Option<&'a VaultMemo<'a>> } // new(): None
pub(super) fn vault_holds_login(&self, row, memo: Option<&VaultMemo<'_>>) -> Result<bool, EngineError>;
pub(super) fn has_login(&self, row, memo: Option<&VaultMemo<'_>>) -> Result<bool, EngineError>;
pub(super) fn read_vault(&self, id: &AccountId, memo: Option<&VaultMemo<'_>>) -> Read<Vec<u8>>; // transact.rs's read_target calls it
```

- [ ] **Step 1: Write the tests.** They must fail to compile or fail as stated.
  - **Unit tests, in `vault.rs`.** The fixture: a test-local `VaultBackend` over a
    `Mutex<BTreeMap>` that counts reads per key and can refuse writes or read a key as
    unreadable; `AccountLock::acquire` over `Env::for_test(tempdir)`;
    `tagteam_cc::ClaudeCode::new(Arc::new(FakeKeychain::new()), Platform::MacOs)` as the
    provider; credentials `{"claudeAiOauth":{"refreshToken":"rt-N"},"mcpOAuth":{}}`. Read counts
    are taken around the memo calls, because `store` and `delete` read the backend themselves.
    - **`a_memo_reads_a_locked_entry_once_until_the_vault_is_written`.** With `a` stored (`rt-1`)
      and a memo over `[lock(a)]`:
      - Calling `read(a)` and `read_prev(a)` twice each costs one backend read of `a` and one of
        `a.prev`, and each returns what `vault.read` and `vault.read_prev` return.
      - After `vault.store(&lock, rt-2, p)`, `read(a)` returns `p.vault_form(rt-2)` and
        `read_prev(a)` returns the vault form of `rt-1`, at one more backend read each.
      - After `vault.delete(&lock)`, `read(a)` returns `Absent`, at one more read.
    - **`a_failed_write_still_makes_the_next_read_fresh`.** Call `read(a)`. With the backend
      refusing writes, `store` returns `Err(VaultError::Write(_))`, and the next `read(a)` reads
      the backend again.
    - **`a_memo_keeps_no_unreadable_read_and_no_unlocked_account`.**
      - With `a` unreadable, `read(a)` twice returns `Unreadable` both times, at two backend
        reads.
      - Once `a` is readable again, `read(a)` returns `Present`, at a third read.
      - `c` is not in the slice, so `read(c)` twice costs two backend reads.
  - **Integration tests, in `tests/keychain_budget.rs`.** Task 3 reuses these helpers:
    - `Counting`, a `Keychain` that forwards to `fx.kc` and logs `Call { op, item, locked }`.
      `item` is `vault:a`, `vault:b.prev`, … or CC's service. `locked` means CC's refresh-lock
      directory exists, judged as `switch.rs`'s roster test judges it.
    - `spawns(&[Call], locked: bool) -> usize`, which counts an `upsert` as 2.
    - `locked_reads(&[Call], item) -> usize`, the `find`s of `item` made under the locks.
    - `counting_engine(&Fx, names) -> (Engine, Arc<Counting>)`, an `EngineConfig` over `fx`'s
      Env, clock, oracle and ports. Its Claude Code (`LiveStore`, zero retry delay) and its
      `KeychainVault` both go through `Counting`.
    - `scenario() -> (Fx, a, b)`, which calls `fx.add` for `a`, then for `b`, leaving `b` live.

    - **`each_locked_vault_entry_is_read_once_while_nothing_writes_the_vault`.** After
      `switch <a>`, `locked_reads` is 1 for each of `vault:a`, `vault:b`, `vault:a.prev` and
      `vault:b.prev`. This fails today: `vault:a` is read 3 times and `vault:b` 2 times.
    - **`a_capture_under_the_locks_makes_the_next_vault_reads_fresh`.**
      - Setup, as in `switch.rs::a_resolved_self_switch_activates_the_generation_it_captured`:
        switch to `a`, `rotate_live("rt-a2")`, and have the oracle answer `a`. Then run
        `switch <a>` through the counting engine.
      - The outcome is `Activated`, and both the live and the vault refresh tokens are `rt-a2`.
      - Under the locks, `vault:a` is read 4 times: by `rederive`, twice by `Vault::store` (its
        read and its verify read), and by the activation.
      - `vault:a.prev` is read twice: by `settle_outgoing` before the capture, and by
        `hold_vault` after it.
      - This fails today: `vault:a` is read 6 times.
- [ ] **Step 2: Implement.**
  - **`VaultMemo::read`.** An account outside `locks` reads through uncached. Otherwise it loads
    `writes` before reading, drops kept reads taken at another count, and keeps `Present` and
    `Absent` results.
  - **`switch_planned`.**
    - Add `let memo = VaultMemo::new(&self.vault, &accounts);` right after `lock_accounts`.
    - Pass `&memo` to `rederive`, and give `LockedSwitch` the field `memo: &memo`.
    - Call `drop(memo)` before `drop(locks)`.
  - **`rederive`.** Calls `has_login(&target, Some(memo))`; its quarantine branch calls
    `read_target(&target, Some(memo))`.
  - **`read_target`, `has_login` and `vault_holds_login`.** Read through
    `self.read_vault(&row.id, memo)`, which reads `memo` when it is `Some` and the vault otherwise.
  - **`transact`.** Builds `Settle { memo: Some(ls.memo), .. }`.
  - **`settle_branch`.** Both `read_target` calls pass `Some(ls.memo)`.
  - **`settle_outgoing`.** Reads with `ls.memo.read(&out.id)` and `ls.memo.read_prev(&out.id)`.
  - **`hold_vault`.** Reads through `self.memo` when it is `Some`.
  - **Docs** cite review refactor §8.11:
    - `rederive`: its target read enters the memo.
    - `read_target` and `has_login`: pass a memo only under the switch's account locks.
- [ ] **Step 3: Verify.** Run the engine's focused tests and both clippy variants. These tests
  stay unchanged and must pass: `switch.rs`'s self-switch capture and
  `the_rotation_roster_is_read_before_cc_s_locks`, `rotation.rs`'s read tests, `rescue_switch.rs`,
  freshen's quarantine-under-the-locks test, `a_replan_keeps_the_mutation_lock`, `recover.rs`,
  `active.rs` and `tests/vault.rs`.
- [ ] **Step 4: Commit** "Read each locked vault entry once per switch attempt".

### Task 2: Skip deleting a managed-key item the locked read found absent

**Depends on:** Task 1 (shared engine test binaries). Michael's ruling D-5 (2026-10-22) applies.

**Files:**
- Modify `crates/tagteam-cc/src/live.rs`: `clear_managed_key` and its doc.
- Modify `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`: one sentence in §9.1.
- Test: `crates/tagteam-cc/tests/live_store.rs`, two new tests.
- Modify the refactor spec `docs/superpowers/specs/2026-10-19-tagteam-review-refactor-design.md`: §2 gains
  D-5 under a new "Michael's ruling during R4 (2026-10-22, relayed by the coordinator session)" table
  (question: may that test gain one setup line so item 4 keeps its panic point? ruling: **Keep item 4,
  with the one setup line.**); §12's list of test edits gains "the one setup line in
  `switch_rollback.rs`'s panic test that keeps §8.11 item 4's delete as its panic point (D-5)".
- One setup line in
  `crates/tagteam-engine/tests/switch_rollback.rs::a_panic_inside_a_live_write_keeps_the_journal_for_recovery`.

**Interfaces:** none new. `clear_managed_key`'s closure takes `now`, the managed-key item as read
under the same storage-write hold (for this entry, `places_now` reads that one place). It returns
`Ok(())` when `now` is `None`.

- [ ] **Step 1: Write the tests** in `live_store.rs`, using its `RecordingKeychain`.
  - **`an_absent_managed_key_item_is_not_deleted_and_primary_api_key_still_goes`.**
    - Setup: the config is
      `{"primaryApiKey":"sk-ant-api03-plain","customApiKeyResponses":{"approved":["x"]}}` and there
      is no item. Call `snapshot`, then `clear_managed_key`, which returns `Ok`.
    - No `delete` of the managed-key service is recorded.
    - `primaryApiKey` is gone, `customApiKeyResponses` is kept, and no item exists.
    - This fails today: a delete is recorded.
  - **`a_managed_key_item_cc_writes_while_tagteam_waits_stops_the_clear`** (a pin).
    - Setup: call `snapshot` with no item, then hold `f.paths.storage_write_lock` as CC does in
      `provider.rs`'s `a_write_that_finds_cc_changed_the_entry_aborts_…`.
    - A thread puts `sk-ant-api03-cc-wrote-this` in the item after 300 ms, then releases the lock.
    - `clear_managed_key` returns `Err(ProviderError::EntryMoved(svc))`.
    - The item keeps CC's key, and nothing is recorded.
  - **The setup line (D-5):** add `fx.put_managed_key(common::STRAY_API_KEY.as_bytes());` before
    `cfg_before`. The test's assertions stay as they are.
- [ ] **Step 2: Implement.**
  - **The closure.** `None` returns `Ok(())`, with a comment citing core §9.1 ("Nothing to
    delete") and review refactor §8.11. Anything else calls `remove_item` as today.
  - **The doc** says the item is deleted, and verified, only when the read under the lock finds
    it.
  - **Core §9.1.** Under "Under it, the entry is read again", after the machine-shared-keys
    bullet, add:
    > - **Nothing to delete.** When that read finds the managed-key item already absent, clearing it makes no delete and no existence probe: the item is as the delete would leave it, and the operation's record of the entry is unchanged ([review refactor §8.11](2026-10-19-tagteam-review-refactor-design.md)).
- [ ] **Step 3: Verify.** Run the cc and engine focused tests and both clippy variants. These tests
  stay unchanged and must pass: `live_store.rs`'s managed-key tests (including the
  will-not-delete report and the storage-write timeouts),
  `the_storage_write_lock_is_a_leaf_taken_only_around_each_entry_write`, `storage_write.rs` and
  `recover.rs`.
- [ ] **Step 4: Commit** "Skip deleting a managed-key item the locked read found absent".

### Task 3: Pin a manual switch's Keychain budget

**Depends on:** Tasks 1 and 2.

**Files:** modify `tests/keychain_budget.rs` (one test) and `CHANGELOG.md` (`Unreleased`,
`Changed`).

- [ ] **Step 1: Write `a_manual_switch_spends_its_keychain_budget`** (a pin).
  - **Preconditions:**
    - The live OAuth item's bytes (`fx.live_item(ItemKind::OAuth)`) differ from
      `fx.vault_bytes(&b)`, and their `fx.cc.vault_form` equals it.
    - `fx.managed_key()` is `None`, and there are no `rescue/` files.
  - **`switch <a>`:** the switch happens, `displaced/` stays empty, `spawns(_, false)` is **20**
    and `spawns(_, true)` is **15**.
  - **A fresh `scenario()` with a bare `switch`:** it switches to `a`, with **21** and **15**.
  - **The failure message** lists every call (op, item, locked) and never any bytes.
  - **The pins** are named constants, with a comment: lower them on purpose when a change saves
    calls.
- [ ] **Step 2: Check the pin once.** Plant `None` in `settle_branch`'s second `read_target`; the
  test must then fail with 21 and 16. Revert the plant, and do not
  commit it.
- [ ] **Step 3: Add the `CHANGELOG.md` entry** under `Changed`: "A switch makes 5 fewer Keychain
  calls while it holds Claude Code's locks (15 instead of 20 for `tagteam switch <N>`), so a
  Claude Code waiting on those locks waits less."
- [ ] **Step 4: Verify.** Run the engine's focused tests, both clippy variants and the workspace
  test command.
- [ ] **Step 5: Commit** "Pin a manual switch's Keychain budget".

## Final verification (the controller, before the PR)

- **Tests:** every CI command on macOS outside the sandbox (passed/total), and `cargo test` in the
  Linux Docker repro. The fixture's Keychain is fake, so the budget holds on Linux.
- **Untouched files:** `git diff --stat refactor-r4g-helpers-comments..HEAD` lists only these:
  - `vault.rs`, the six `switch/` files and `cc/live.rs`;
  - the test files `keychain_budget.rs` and `live_store.rs`, plus `switch_rollback.rs`;
  - `CHANGELOG.md`, the two specs and this plan.

  So no §3.2 file, no held file and not `tests/common/mod.rs`.
- **Greps:**
  - `switch/transact.rs` has no `self.vault.read`.
  - `switch/support.rs` reads the vault directly only in the `None` arms of `read_vault` and
    `hold_vault`, and in `matches_vault`.
- **Records:**
  - The PR joins the spec's Status line.
  - If Task 2 ran, core §9.1 carries its sentence.
  - The final pre-merge commit marks this plan `Implemented`.

## Rulings (recorded under "Rulings" in the PR)

1. **One task holds the memo and its readers.** A memo with no reader fails `dead_code` under
   `-D warnings`, and no `#[allow]` may cover that. Rejected: a task that adds only the type.
2. **The memo rides in `LockedSwitch`,** beside the `account_locks` it borrows, and in
   `Settle.memo`, which `Settle::new` sets to `None`. That gives `recover.rs` and `active.rs`
   their uncached pass with no edit. Rejected:
   - a field on `Locked`, an owned struct of rows that would then need a lifetime;
   - a `hold_vault` parameter, which edits two files for no change in behaviour.
3. **`read_target`, `has_login` and `vault_holds_login` take an explicit `Option<&VaultMemo>`.**
   Callers before the locks pass `None`. Rejected: memo-aware twins of these functions, which
   would copy the error mapping.
4. **The memo keeps `Present` and `Absent`, for locked accounts only, and never `Unreadable`.** A
   Keychain timeout is transient, and keeping it would repeat a failure that today's code
   re-reads.
5. **The write count.** `store` and `delete` bump it on entry, success or not, because a failed
   store may already have written `.prev`. `sweep` does not bump it: only purge runs `sweep`,
   under the mutation lock a switch also holds. The counter stays private. Rejected:
   - a public `write_version()`;
   - a cache on `Vault` or `Engine`, which §8.11 forbids.
6. **One memo per attempt.** It is dropped with the locks, and a replan starts empty.
7. **The memo does not serve:**
   - reads made before the locks;
   - `auto_refusal`'s re-read under the locks, which costs one read per automatic switch;
   - `settle_rescues`' and `apply_provenance`'s own reads, which are rare and shared, and whose
     writes bump the count;
   - `Vault::store`'s internal reads.
8. **The budget counts in §8.11's unit, spawns,** and pins exact totals and under-lock counts for
   both `switch <N>` and bare `switch`. Rejected:
   - `<=`, which would pass a regression that offsets a saving;
   - counting raw trait calls.
9. **Item 4 meets both of §8.11's conditions.** The strict read and the delete share one hold
   (`under_storage_write`, `live.rs:542-557` at `6aeb`). A skipped delete leaves the ledger
   unchanged: `settled` records nothing new, so `first` stays empty and rollback is identical
   (`:498-511`, `:1194-1197`). §12 blocked its one test edit until Michael's D-5 (Ruling 11).
10. **`CHANGELOG.md` states the measured numbers.** §11's "two fewer under the locks" undercounts:
    all five savings happen under the locks.
11. **Michael's decision (2026-10-22, D-5):** keep item 4, with one setup line in
    `switch_rollback.rs::a_panic_inside_a_live_write_keeps_the_journal_for_recovery`
    (`fx.put_managed_key(common::STRAY_API_KEY.as_bytes());`), no assertion changed. Recorded as the
    refactor spec's D-5 and a §12 exception; core §9.1 gains "Nothing to delete". Rejected by him:
    dropping item 4 (3 calls saved instead of 5).
