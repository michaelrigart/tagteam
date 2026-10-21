# tagteam refactor R4b — the daemon's switchable reads and the store's lock tokens — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** In progress — https://github.com/michaelrigart/tagteam/pull/31 (§8.1, Tasks 1–4); §8.2 (Task 5) follows in a stacked PR
**Goal:** One switchable rule on `AccountRow`, engine queries in place of the daemon's raw-row reads, one structural poll-candidate test shared by collection and the daemon's wake-up, and lock tokens on three store mutators. User-visible: the daemon no longer wakes at its 60 s floor for an account collection skips.
**Architecture:** The rule's store-only halves become `AccountRow` methods. A round's structural candidacy becomes one function in `collect.rs`, which `scheduled_candidates` filters by due-ness and `Engine::earliest_poll_at` minimises over. The mutators take `&MutationGuard` or `&AccountLock`, as `Vault::store` does.
**Tech Stack:** Rust 1.88 (edition 2024), cargo workspace; rusqlite.
**Branch:** `refactor-r4b-daemon-store-contracts`, based on `refactor-r4a-failure-kinds`.
**Spec:** docs/superpowers/specs/2026-10-19-tagteam-review-refactor-design.md (§8.1, §8.2, with §2, §3, §11, §12)

**Reading this plan.**
- **Sites** are cited at `c7bfb84`. R4a lands in between and retypes the usage failure tokens in `collect.rs`, `engine/auto.rs`, `provider_job.rs`, `store/usage.rs` and `words.rs`. Find sites by function name; line numbers are hints.
- **Focused tests:** `cargo test -p tagteam-engine --locked --features test-hooks`; `cargo test -p tagteam-daemon --locked --features test-hooks`.
- **Order:** every task compiles `tagteam-engine`, so tasks run one at a time, in order, each leaving its crates green.

## Global Constraints

- **Byte-identical output** except §11's §8.1 row (Task 4, with a `CHANGELOG.md` entry). No wire, `--json` or store-schema change.
- **C-1:** the `Store` and every mutator stay `pub`.
- **Hardening's files are not edited:** `refresh.rs`, `store/tokens.rs`, `tokens/*`; the provider's `security.rs`, `process.rs`, `keychain.rs` and fakes; the loops in `server.rs`, `notify.rs`, `scheduler.rs`; `client.rs`; CI files. `notify.rs`'s lease read stays (§8.1).
- **Held TUI files** are not edited; none calls a changed API.
- **Unit-test modules stay last**, `#[cfg(test)]` alone above `mod tests {`.
- **CI commands** (`CONTRIBUTING.md`) all pass. Every task runs both clippy variants (with and without `--features tagteam/test-support`) and its focused tests.
- **Test edits:** mechanical signature changes (Task 5) and new tests only. No new `#[allow(...)]`. Comments wrap at 100.
- **Commits:** imperative, sentence case, no prefix, no trailing period, no agent attribution.

## Review Focus

1. **The live account is the one the live login names.** A disabled live account still wakes the daemon (phase 1 collects it); after `claude /login` as an unstored account, a disabled former live row does not. Test: Task 3, first test.
2. **Rows of a provider this build does not register** (a store shared with a newer build) never make `auto` run it. Test: Task 2, daemon test.
3. **A quarantine still counts as switchable** for the mode table and `auto`. Test: Task 2, engine test.
4. **`earliest_poll_at` is read-only:** no request, never creates the store. Test: Task 3, first test.
5. Checked, already covered: a failed or unwinding forced switch puts back the superseded row (`switch_rollback.rs::a_rolled_back_forced_switch_puts_the_superseded_row_back`).

## Spec coverage

| Spec | Task |
|---|---|
| §8.1 `AccountRow` methods; the engine's copies | 1 |
| §8.1 the two counts; `daemon/auto.rs`, `inputs` | 2 |
| §8.1 structural test, `earliest_poll_at`; §12's tests | 3 |
| §8.1 `earliest_next_poll` replaced; regression; §11; §14 | 4 |
| §8.2 | 5 |

---

### Task 1: The switchable and poll-candidate rules on `AccountRow`

**Depends on:** none.

**Files:**
- Modify: `crates/tagteam-engine/src/store/mod.rs` (after `pub struct AccountRow`, `:177-198`)
- Modify: `crates/tagteam-engine/src/switch/select.rs` (`is_candidate`, `:21-27`; `Engine::switch_candidate`)
- Modify: `crates/tagteam-engine/src/auto.rs` (`AutoEngine::snapshot`, the `switchable:` field)
- Test: `crates/tagteam-engine/tests/store.rs`

**Interfaces:**
- Produces, in `store/mod.rs`:
  - `impl AccountRow { pub fn is_switchable(&self) -> bool; pub fn is_poll_candidate(&self) -> bool }`.
  - `is_switchable`: `!disabled && identity_json.is_object()`.
  - `is_poll_candidate`: `is_switchable()` and not quarantined, the store half of a rotation candidate (§9.3) and a scheduled poll (§8.6). `is_candidate`'s doc moves here.

- [ ] **Step 1: The test.** `an_account_row_is_switchable_when_enabled_with_an_identity_and_a_poll_candidate_unless_quarantined`: one row from `add(&s, &cc(), "a", "a@x.co", 1)`, and three struct-update variants of it. Assert `(is_switchable(), is_poll_candidate())` for each:
  - as stored: `(true, true)`;
  - `disabled: true`: `(false, false)`;
  - `identity_json: Value::Null`: `(false, false)`;
  - `quarantine_reason: Some("invalid_grant".into())`: `(true, false)`.

  Run it. Expected: compile failure.
- [ ] **Step 2: Implement the two methods.**
- [ ] **Step 3: The engine's copies.** Delete `switch::select::is_candidate`; `switch_candidate` calls `row.is_poll_candidate()` in its place. `snapshot`'s `switchable:` becomes `a.row.is_switchable()`.
- [ ] **Step 4: Verify.** Engine focused command; both clippy variants.
- [ ] **Step 5: Commit** "Give AccountRow the switchable and poll-candidate rules".

### Task 2: The engine counts switchable accounts and providers for the daemon

**Depends on:** Task 1.

**Files:**
- Modify: `crates/tagteam-engine/src/engine.rs` (after `provider_ids`)
- Modify: `crates/tagteam-daemon/src/auto.rs` (`providers`, `:105-132`; its test module's imports)
- Modify: `crates/tagteam-daemon/src/provider_job.rs` (`ProviderJob::inputs`)
- Test: `crates/tagteam-engine/tests/engine_basics.rs`, `crates/tagteam-daemon/src/auto.rs` tests

**Interfaces:**
- Produces, on `Engine`, in `engine.rs`:
  - `pub fn switchable_count(&self, provider: &ProviderId) -> Result<usize, EngineError>`: the provider's rows where `is_switchable()`; 0 without a store, which it never creates.
  - `pub fn switchable_providers(&self) -> Result<Vec<ProviderId>, EngineError>`: every provider in the store with at least two `is_switchable()` rows, in `ProviderId` order; empty without a store. It reads the store alone: no registry check.

- [ ] **Step 1: The tests.**
  - `engine_basics.rs`, `switchable_counts_read_the_store_alone_and_count_a_quarantine` (Review Focus 3):
    - Fresh `Fx`: count 0, providers `[]`, and `!fx.engine.store_file().exists()`.
    - `a`, `b` added; `fx.quarantine(&a, "invalid_grant", "sha256:x")`. The count is 2, "a quarantine still counts".
    - `common::add` two rows of `ProviderId::new("newer-agent")`. Providers are `[claude-code, newer-agent]`.
    - `fx.engine.set_disabled(&b, true)`. Count 1; providers `[newer-agent]`.
  - `daemon/auto.rs`, `rows_of_a_provider_this_build_does_not_register_never_make_auto_run_it` (Review Focus 2):
    - `two(&fx)`, then two `store.insert_account(&NewAccount { .. })` rows of `newer-agent` with `kind: "oauth"` and an object identity.
    - `fx.engine.switchable_providers()` is `[cc(), newer-agent]`.
    - `providers(&fx.engine, None)` is `[cc()]`.
  - Expected: compile failure.
- [ ] **Step 2: Implement the two queries.**
- [ ] **Step 3: The daemon.**
  - `auto::providers`: the `only` branch is unchanged; otherwise `engine.switchable_providers()?` filtered by `engine.provider(p).is_ok()`, the registry clause.
  - `BTreeMap`'s import moves into the test module, its only remaining user.
  - `ProviderJob::inputs`: `has_store` is `existing_store()?.is_some()`; `switchable` is `switchable_count(&self.provider)?`.
  - Existing `providers` and mode-table tests pass unchanged.
- [ ] **Step 4: Verify.** Both focused commands; both clippy variants.
- [ ] **Step 5: Commit** "Count switchable accounts and providers through the engine".

### Task 3: One structural poll-candidate test, and `Engine::earliest_poll_at`

**Depends on:** Task 2.

**Files:**
- Modify: `crates/tagteam-engine/src/collect.rs`: `collect_scheduled`, `scheduled_candidates`, `role_of`, `collect_one`, plus new items
- Modify: `crates/tagteam-engine/tests/common/mod.rs` (`plan_poll`)
- Test: `crates/tagteam-engine/tests/collect.rs`, `crates/tagteam-engine/tests/session_usage.rs`

**Interfaces:**
- Produces, in `collect.rs`:
  - `fn has_usage(p: &dyn Provider, row: &AccountRow) -> bool`: `p.capabilities().usage && !p.kind_traits(&row.kind).managed_key_axis`. It replaces the same expression in `collect_one` and `role_of`.
  - `struct PollRows<'r> { live: Option<&'r AccountRow>, candidates: Vec<&'r AccountRow> }`.
  - `fn poll_rows<'r>(&self, p: &dyn Provider, rows: &'r [AccountRow], live: Option<&'r AccountRow>) -> Result<PollRows<'r>, EngineError>`, the structural test (§8.6): what a round could ever collect, due or not.
    - `live`: the given row if `has_usage` and not quarantined. A disabled live row stays: phase 1 collects it.
    - `candidates`: every other row with `has_usage` and `switch_candidate(p, row)?`, tried in that order. That order skips the session read for the live row and for managed keys, as today.
  - `pub fn earliest_poll_at(&self, provider: &ProviderId) -> Result<Option<i64>, EngineError>`: the minimum `usage_state(..).next_poll_at` over `poll_rows`' live row and candidates. The live row comes from `live_accounts(&rows)`, as in `collect_scheduled`. No hook point, `Eligibility`, `backoff_holds` or `scheduled_pick`. `Ok(None)` without a store (never created) or a plan.
- Produces, in `common/mod.rs`: `pub fn plan_poll(fx: &Fx, id: &AccountId, at: i64)`, a raw-SQL upsert of `usage_state.next_poll_at`, like `collect.rs`'s `back_off`.

- [ ] **Step 1: The tests** (§12's list). `cc` is `ProviderId::new(CLAUDE_CODE)`; "earliest" means `fx.engine.earliest_poll_at(&cc).unwrap()`.
  - `collect.rs`, `the_earliest_poll_counts_the_live_account_and_a_candidate_planned_ahead` (Review Focus 1 and 4):
    - Fresh `Fx`: earliest is `None`, and the store file does not exist.
    - `three_accounts` (`c` live). Plans: `a` at `NOW_S + 120`, `b` at `+400`, `c` at `+300`. Earliest is `Some(NOW_S + 120)`: a candidate 120 s ahead inside a 300 s interval.
    - `c` re-planned at `+90`: `Some(NOW_S + 90)`.
    - `store.set_disabled(&c, true)`: still `Some(NOW_S + 90)`.
    - `fx.login("z@x.co", "rt-z")`, an unmanaged live login: `Some(NOW_S + 120)`.
    - Last, `usage_requests(&fx) == 0`.
  - `collect.rs`, `the_earliest_poll_skips_what_a_round_skips_and_a_round_reads_what_it_counts`:
    - The fixture of `only_switchable_accounts_with_usage_are_candidates`: `q` quarantined, `d` disabled, `ok`, `c` live, and the API key `k`.
    - Plans: `q` `+60`, `d` `+70`, `k` `+80`, `ok` `+200`, `c` `+300`. Earliest is `Some(NOW_S + 200)`.
    - Advance 400 s. `script_usage(200, usage_fixture())`, then `scheduled(&fx, 50.0)`. Its outcomes are `[(c, Recorded), (ok, Recorded)]`: exactly the accounts earliest counted.
    - `fx.quarantine(&c, ..)`, then plan `c` at `NOW_S + 410`. Earliest equals `ok`'s new `next_poll_at`.
  - `session_usage.rs`, `the_earliest_poll_skips_a_session_owned_candidate`:
    - `a`, `c`, then `b` live. Plans: `a` `+60`, `c` `+200`, `b` `+300`. Earliest is `Some(NOW_S + 60)`.
    - `let _s = owned(&fx, &a, credential("a@x.co", "rt-p"));`. Earliest is `Some(NOW_S + 200)`.
  - Expected: compile failure.
- [ ] **Step 2: Extract.**
  - `collect_scheduled` computes `poll_rows` after phase 1, where phase 2 reads the store today, and passes `&polls.candidates` to `scheduled_candidates`. That parameter replaces `rows` (`candidates: &[&'r AccountRow]`); picks map back through it.
  - `scheduled_candidates` keeps its capability early return, due test and escalation.
  - Phase 1 still hands the live row to `collect_one` unfiltered (ruling 3).
- [ ] **Step 3: Implement `earliest_poll_at`** beside `poll_rows`.
- [ ] **Step 4: Verify.** Engine focused tests: every scheduled-collection test in `collect.rs`, `session_usage.rs` and `invariant.rs` passes unchanged. Both clippy variants.
- [ ] **Step 5: Commit** "Share one structural poll-candidate test between collection and earliest_poll_at".

### Task 4: The daemon wakes for what a round can collect

**Depends on:** Task 3.

**Files:**
- Modify: `crates/tagteam-daemon/src/provider_job.rs` (`collect`; delete `earliest_next_poll`; the `ProviderJobCtx.notices` doc)
- Modify: `CHANGELOG.md`
- Test: `crates/tagteam-daemon/src/provider_job.rs` tests

**Interfaces:**
- Consumes: `Engine::earliest_poll_at` (Task 3).

- [ ] **Step 1: The regression test.** `a_collect_round_waits_for_an_account_it_collects_never_one_it_skips`:
  - `two(&fx)`. `fx.read(&a, 10.0, 10.0, T0, T0 + 120)`; `fx.read(&b, 20.0, 50.0, T0, T0 + 400)`.
  - `fx.engine.add_token(AddTokenOptions { provider: cc(), token: "sk-ant-api03-abcdefghijklmnopqrstuvwxyz".into(), position: None, email: None, alias: None, yes: false })`. It makes no network call. Then `fx.read(&key, 0.0, 0.0, T0, T0 + 10)`.
  - `fx.settings("[autoswitch]\ninterval_seconds = 300\n\n[daemon]\nautoswitch = false\n", 1)` before `ProviderJob::new`, then one `job.run`.
  - Assert `events.modes(&cc()) == [(0, Mode::Collect, None, None)]`, `status(&board, &cc()).next_tick_at == Some(T0 + 120)` ("not the 60 s floor the key's plan forced"), and `fx.http.count(Method::Get, CC_USAGE) == 0`.

  Run it. Expected: FAIL, `Some(T0 + 60)`.
- [ ] **Step 2: Switch over.** `collect` takes `self.engine.earliest_poll_at(&self.provider).unwrap_or(None)`, keeping its comment. Delete `earliest_next_poll`. §14: `ProviderJobCtx.notices`' doc drops "(Task 11)", if R4a left it.
- [ ] **Step 3: `CHANGELOG.md`.** Last bullet under `## [Unreleased]` → `### Changed`:

  > - The daemon no longer wakes every 60 s because of an account its usage collection skips (one a `tagteam run` session owns, or a managed API key); it waits for the next account a round will collect.
- [ ] **Step 4: Verify.** Daemon focused command; both clippy variants.
- [ ] **Step 5: Commit** "Wake the daemon for the accounts a collect round can collect".

### Task 5: Lock tokens on `insert_journal`, `begin_replacement` and `finish_replacement`

**Depends on:** Task 4.

**Files:**
- Modify: `crates/tagteam-engine/src/store/mod.rs` (`insert_journal` `:1556`, `begin_replacement` `:1075`, `finish_replacement` `:1151`)
- Modify, the callers:
  - `engine.rs`: `reconcile_replacement_as`, and the unit tests at `:841` and `:859`;
  - `lifecycle.rs`: `commit_login`;
  - `recover.rs`: `Recovery`, `recover_one`, `finish_backward`;
  - `switch/mod.rs`: the `LockedSwitch` literal;
  - `switch/transact.rs`: `LockedSwitch`, `transact`'s `Rollback` literal, `journal_row`;
  - `switch/rollback.rs`: `Rollback`, `roll_back`.
- Test (mechanical), under `crates/tagteam-engine/tests/`: `common/mod.rs`, `store.rs`, `doctor.rs`, `engine_basics.rs`, `freshen.rs`, `log_lines.rs`, `manage.rs`, `oracle.rs`, `provenance.rs`, `purge.rs`, `recover.rs`, `replacement_unreadable.rs`, `storage_write.rs`, `switch_rollback.rs`

**Interfaces:**
- Produces, on `Store`:
  - `pub fn insert_journal(&self, _guard: &MutationGuard, j: &JournalRow) -> Result<(), StoreError>`;
  - `pub fn begin_replacement(&self, lock: &AccountLock, fp: &str, meta: &LoginMeta<'_>, live_names_account: bool) -> Result<(), StoreError>`;
  - `pub fn finish_replacement(&self, lock: &AccountLock, at: i64) -> Result<(), StoreError>`.
  - The lock replaces the `&AccountId`: the account is `lock.id()`, as in `Vault::store`. Each doc gains one line: holding the token is the proof (§6.2, §9.1).
- Produces fields `guard: &'a MutationGuard` on `LockedSwitch` (from `&guard` in `switch/mod.rs`), `Rollback` (from `ls.guard`) and `Recovery`.
- `recover_one` builds its `Recovery` once, before `match direction`. `finish_backward` becomes `(&self, p, store, rec: &Recovery<'_, '_>, established: &str)`, reading `rec.row`, `rec.locks`, `rec.live`: within clippy's seven parameters.
- Produces, in `tests/common/mod.rs`: `pub fn held_guard(env: &Env) -> MutationGuard` and `pub fn held_lock(env: &Env, id: &AccountId) -> AccountLock`.
  - Both acquire directly (`MutationGuard::TIMEOUT`, `AccountLock::WAIT`) under a fresh `Cancel`, as `mutation_lock_free` does.
  - Never through `Engine::mutation_guard` or `lock_account`, which recover the row or reconcile the replacement the test plants.

- [ ] **Step 1: The store and its callers.** Change the three signatures. `reconcile_replacement_as` passes `lock`; `commit_login` passes `lock_for(&row.id)` to both; `journal_row` passes `ls.guard`; `roll_back` `self.guard`; `finish_backward` `rec.guard`.
- [ ] **Step 2: The tests, mechanically.**
  - `x.insert_journal(&row)` becomes `x.insert_journal(&held_guard(&fx.env), &row)`; `x.begin_replacement(&id, …)` and `x.finish_replacement(&id, …)` take `&held_lock(&fx.env, &id)` in place of `&id`.
  - `store.rs` has no `Fx`: it builds `Env::for_test(d.path())` once per test that needs it. The `engine.rs` unit tests use `MutationGuard::acquire(engine.env(), MutationGuard::TIMEOUT).unwrap()`.
  - **Where a token is already held, pass it:** `freshen.rs`'s `guard`, `Fx::replace_login`'s `_guard` and `lock`. A second `flock` on one file in one process waits out the timeout, then panics.
  - `provenance.rs`'s `freshen-before-lock` callback gets a cloned `Env`. No lock is held there, nor at `recover.rs`'s `before-mutation-lock` callback.
  - `Fx::begin_replacement` and `begin_replacement_with` keep their signatures.
- [ ] **Step 3: Verify.** Engine focused tests, and `cargo test -p tagteam-engine --locked`; both clippy variants. `git grep -n 'insert_journal\|begin_replacement\|finish_replacement' -- crates/tagteam crates/tagteam-daemon` finds nothing.
- [ ] **Step 4: Commit** "Require lock tokens on insert_journal, begin_replacement and finish_replacement".

## Final verification (the controller, before the PR)

- **macOS, outside the sandbox:** fmt, both clippy commands, the workspace `cargo test`; totals as passed/total.
- **Linux:** the same `cargo test` in the Docker repro.
- **Untouched:** `git diff --stat refactor-r4a-failure-kinds..HEAD -- crates/tagteam-tui crates/tagteam/src/tui_host.rs crates/tagteam/src/tui_cmd.rs crates/tagteam-daemon/src/notify.rs crates/tagteam-engine/src/refresh.rs` is empty.

## Rulings (recorded under "Rulings" in the PR)

1. **`switchable_providers` takes no `only`.** `daemon/auto.rs` returns a named provider uncounted (pinned by `auto_runs_every_provider_…`), so an engine `only` would have no caller.
2. **The registry clause stays in `daemon/auto.rs`**, filtering the engine's store-only result. Counts are per provider, so this equals today's per-row clause.
3. **Phase 1's collection is unchanged**: `collect_one`'s guards still report `Unsupported` and `Ineligible(Quarantined)`. `poll_rows` states the same rule, sharing `has_usage`; phase 2's test is extracted literally. Task 3's second test pins agreement.
4. **The replacement mutators take the lock in place of the id**, so a lock for one account cannot authorize a write to another.
5. **The guard travels as fields.** Rejected: a `LiveLocks::guard()` accessor, a smaller diff that widens `tagteam-provider`'s API for an engine contract.
6. **`is_poll_candidate` replaces `switch::select::is_candidate`**, the fourth of §8.1's five copies. `recheck_ids` stays.
7. **Query homes:** `engine.rs` for the two counts; `collect.rs` for `earliest_poll_at`, beside the private items it uses.
8. **§8.2's "about 21 test files"** measures 14 plus `engine.rs`'s unit tests; the `Fx` replacement helpers keep their signatures.
9. **CHANGELOG:** one entry under `### Changed`, where R3 filed its fix.
