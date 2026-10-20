# tagteam refactor R4a — failure kinds — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Approved
**Branch:** `refactor-r4a-failure-kinds`, based on `refactor-r3-cli-protocol` (`c7bfb84`)
**Goal:** Type §8.6's two failure vocabularies, usage failure tokens as `FailureKind` and the gate's transient kinds as `GateFailure`, with every stored, wire, printed and logged string unchanged.
**Architecture:** A leaf module, `crates/tagteam-engine/src/failure.rs`, holds both enums. Each lands with its consumers: `FailureKind` with its readers (Task 1), then its writers and `Collected::Errored` (Task 2); `GateFailure` with the gate and its readers (Task 3). Store, view and wire types stay `String`.
**Tech Stack:** Rust 1.88 (edition 2024).

**Spec:** docs/superpowers/specs/2026-10-19-tagteam-review-refactor-design.md (§8.6, with §3, §8.9's CC-E-21 bullet, §11 and §12)

**Reading this plan.**
- **Line numbers** are as of `c7bfb84`. Find each site by the code quoted.
- **Focused tests:** `cargo test -p <crate> --locked`, adding `--features test-hooks` for `tagteam-engine` and `tagteam-daemon`, and `--features test-support` for `tagteam`. `tagteam-provider` and `tagteam-tui` take no feature.
- **Each task** leaves the workspace compiling, with its crates' tests green. The tasks run in order (1 → 2 → 3): Tasks 2 and 3 share `collect.rs` and `failure.rs`.

## Global Constraints

- **Byte-identical output** (§1.4). §8.6 has no §11 row, so no `CHANGELOG.md` entry.
- **No wire or store change.** `last_error`, `usageError`, `fetchErrors` and `poll`'s `fetch_errors` keep their strings. `UsageStateRow.last_error`, `UsageView.error`, both `fetch_errors` maps and `Store::record_usage_failure(.., kind: &str, ..)` stay string-typed.
- **Log fields keep `&str` values** (`kind="pre-send"`, quoted). Neither enum implements `Display`, and `?kind` is forbidden.
- **Hardening-owned files** (§3.2's list) are not edited, except `refresh.rs` (Task 3, as §3.2 allows).
- **Held TUI files:** only `crates/tagteam-tui/src/source.rs` changes, in two match arms (Task 2).
- **Out of scope:** `AutoFreshened::Failed(String)`; `EngineError::kind()`'s `foreign-credential` and `rescue-unreadable` (§8.5); `views::NO_DATA`; and `collect.rs`'s `gate` policy (§8.3).
- **Unit-test modules stay last,** with `#[cfg(test)]` directly above `mod tests {` (for `neutrality.rs`). New comments are neutral, wrap at 100 columns, and cite spec sections, not plan numbering (§14).
- **CI** (`CONTRIBUTING.md`): `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --locked [--features tagteam/test-support] -- -D warnings`, both variants in every task; `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`.
- **Tests and commits.** No test is weakened: the only test edits are each task's listed ones, plus new tests. Add no `#[allow]`. Commits carry no agent attribution, and their messages are imperative, in sentence case, with no prefix or trailing period.

## Review Focus

1. **A log field written through `Display` or `Debug`.** The gate's quoted `kind` is pinned, but §7.5's is not. Test: Task 3, `an_active_token_refresh_that_fails_transiently_logs_its_kind_as_a_string`.
2. **An errored collection.** `fetchErrors` and `poll` must still say `error`; nothing pins it. Test: Task 2, `fetch_error_spells_each_reported_outcome`.
3. **An unknown stored token,** from a newer build or non-canonical (`http-0401`). It reads `unavailable`, shows as stored, and parses back to the same string. Tests: Task 1, `parse_reads_back_exactly_what_was_stored` and the two `an_unknown_*` tests.
4. **`http-401` from two sources.** `Unauthorized`'s token and a provider's `Http(401)` compare equal, as equal strings did. Test: Task 1, `a_transient_401_is_the_http_401_token`.
5. **The TUI's fetched count.** `Errored` must not count as fetched, and every `Failed` must. The file is held, so it gets no test. The reviewer checks Task 2's arms against the old `kind != "error"`.

## Spec coverage

| Spec | Task |
|---|---|
| §8.6.1: `FailureKind` and its readers | 1 |
| §8.6.1: the writers, `over-budget`, `Errored`, `source.rs` | 2 |
| §8.6.2: `GateFailure` | 3 |
| §8.9: `str_enum!` | none (Ruling 1) |

---

### Task 1: Read usage failure tokens through `FailureKind`

**Depends on:** none.

**Files:**
- Create: `crates/tagteam-engine/src/failure.rs`.
- Modify:
  - `lib.rs` (`pub mod failure;`, `:14`);
  - `crates/tagteam-provider/src/provider.rs` (`impl TransientKind`, `:479-489`, and tests);
  - `views.rs` (`usage_status`, `:171-181`, and tests);
  - `words.rs` (`usage_words`, `:123-127`, and tests).

**Interfaces (produces):**

```rust
impl TransientKind { // provider.rs
    /// `token`'s inverse: `Some` only for a spelling `token` writes (`http-0401` is `None`).
    pub fn parse(s: &str) -> Option<TransientKind>;
}
/// `usage_state.last_error`'s kind token (§8.3, §6.1): what a usage collection recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureKind {
    NoAccessToken, Http401, RefreshFailed, KeychainUnavailable, ProfileDrifted,
    LiveReplaced, ForeignCredential, VaultAbsent, TokenExpired, OverBudget,
    /// Spelled by `TransientKind::token`. `From` and `parse` never build `Http(401)` here.
    Provider(TransientKind),
    /// A token this build does not know, read back from the store as it was.
    Other(String),
}
impl FailureKind {
    pub fn as_str(&self) -> Cow<'_, str>;
    pub fn parse(s: &str) -> FailureKind; // named, then TransientKind::parse via From, else Other
}
impl From<TransientKind> for FailureKind { .. } // Http(401) becomes Http401
```

The named spellings, in order, are `no-access-token`, `http-401`, `refresh-failed`, `keychain-unavailable`, `profile-drifted`, `live-replaced`, `foreign-credential`, `vault-absent`, `token-expired` and `over-budget`.

- [ ] **Step 1: Write the tests.** They must fail to compile.
  - **`provider.rs`,** `a_transient_kind_parses_only_the_tokens_it_writes`:
    - `parse(&k.token()) == Some(k)` for `PreSend`, `Ambiguous`, `BadResponse`, `Http(100)`, `Http(429)` and `Http(65535)`;
    - `None` for `"http-0401"`, `"http-"`, `"http-+5"`, `"http-70000"`, `"http-401 "`, `"Pre-Send"` and `""`.
  - **`failure.rs`:**
    - `every_failure_kind_keeps_its_stored_spelling`. The table is the ten named variants, plus `Provider(PreSend)` as `"pre-send"`, `Provider(Ambiguous)` as `"ambiguous"`, `Provider(BadResponse)` as `"bad-response"`, `Provider(Http(429))` as `"http-429"` and `Other("x-new")` as `"x-new"`. Each row has `as_str() == s` and `parse(s) == kind`.
    - `parse_reads_back_exactly_what_was_stored`: `""`, `"error"`, `"no-data"`, `"http-0401"`, `"HTTP-401"`, `"over_budget"` and `" over-budget"` each parse to `Other(s)`, with `as_str() == s`.
    - `a_transient_401_is_the_http_401_token`: `from(Http(401)) == Http401`, `parse("http-401") == Http401`, and `from(Http(429)) == Provider(Http(429))`.
  - **`views.rs`,** `an_unknown_last_error_reads_as_unavailable`: for `"a-token-from-a-newer-build"`, `"http-0401"`, `"error"` and `""`, `usage_status(true, &OAUTH, &row(false), Some(&state(1, Some(e), true)))` is `Unavailable`.
  - **`words.rs`,** `an_unknown_failure_token_is_unavailable_in_its_own_spelling`: `usage_words(&unread(Unavailable, Some("a-token-from-a-newer-build"), None), NOW)` is `"unavailable (a-token-from-a-newer-build)"`.
- [ ] **Step 2: Implement.**
  - **`TransientKind::parse`** accepts `pre-send`, `ambiguous`, `bad-response`, and `http-<u16>` only where `format!("http-{code}") == s`.
  - **`FailureKind::parse`** finds a named token in a private `const NAMED: [FailureKind; 10]` by `as_str`, so each spelling is written once.
  - **`usage_status`** matches `FailureKind::parse(last_error)`. `NoAccessToken | VaultAbsent` give `NoCredentials`; `ForeignCredential`, `KeychainUnavailable` and `TokenExpired` give their own statuses; anything else gives `Unavailable`.
  - **`usage_words`** checks `NO_DATA` first. `OverBudget` says `over budget`. Any other token says `unavailable ({e})`, written with the stored `e`.
- [ ] **Step 3: Verify.** Run the engine and provider tests and both clippy variants. `usage_status_follows_the_table_row_by_row` and `every_status_says_when_it_is_retried_while_that_is_ahead` pass unchanged.
- [ ] **Step 4: Commit** "Read usage failure tokens through FailureKind".

### Task 2: Write usage failures as `FailureKind` and name the errored outcome

**Depends on:** Task 1.

**Files:**
- Modify:
  - `collect.rs`: `Collected` (`:62-85`), `CollectReport::add` (`:104-125`), `Failure`/`failed` (`:575-637`), `windows` (`:654-670`), the writers and `refusal_kind` (`:696-1051`), `authorize` (`:1119-1122`), `record_to_store` (`:1244-1254`), the docs at `:71-75`, `:105`, `:191` and `:610`, and its tests;
  - `store/usage.rs` (`OVER_BUDGET`, `:50-51`, and its use at `:458`);
  - `auto.rs` (`poll`, `:854-864`);
  - `crates/tagteam-daemon/src/provider_job.rs` (`:602-616`);
  - **held, compile fix only:** `crates/tagteam-tui/src/source.rs` (`:165-167`).
- Test edits: `tests/common/mod.rs:1518-1520` (`failed`), `tests/collect.rs:1468`, `tests/collect_active.rs:370,565`.

**Interfaces:**
- Consumes: Task 1's `FailureKind` and `From<TransientKind>`.
- Produces:

```rust
pub enum Collected { /* others unchanged */
    Failed { kind: FailureKind },
    /// The collection ended with an error (the store or a hook): nothing was recorded, and the
    /// report's warnings say why. Not a stored token: reported as `error` (`fetch_error`).
    Errored,
}
impl Collected {
    /// What `fetchErrors` and the auto `poll` event report: `Failed`'s token, `over-budget`
    /// for `OverBudget`, `error` for `Errored`; `None` for the rest.
    pub fn fetch_error(&self) -> Option<String>;
}
// private: Failure.kind: FailureKind; Failure::new(FailureKind); failed(FailureKind) -> Stop;
// Collection::refusal_kind(&self) -> FailureKind
```

- [ ] **Step 1: Write the test** `fetch_error_spells_each_reported_outcome` (`collect.rs` tests). It must fail to compile.
  - `Errored` gives `"error"`, and `OverBudget { next_free_at: 1 }` gives `"over-budget"`.
  - `Failed { Http401 }` gives `"http-401"`, and `Failed { Provider(Http(429)) }` gives `"http-429"`.
  - `Recorded`, `Dropped` and `Unsupported` give `None`.
- [ ] **Step 2: Implement.**
  - **The writers.** Each `failed("<token>")` becomes `failed(FailureKind::<Variant>)`. `windows` takes `is_429` from the `TransientKind` before storing `kind.into()`. `authorize` uses `Failure::new(FailureKind::OverBudget)`.
  - **`record_to_store`** passes `&f.kind.as_str()` to the store and returns `Failed { kind: f.kind }`.
  - **`CollectReport::add`** writes `Errored`. Docs that named `Failed { kind: "error" }` or `Failed { "over-budget" }` now name `Errored` or `Failed { kind: OverBudget }`.
  - **`store/usage.rs`:** `OVER_BUDGET` goes, and `reserve_usage` binds `FailureKind::OverBudget.as_str()`.
  - **`auto.rs`'s `poll`** keeps `(position(id)?, collected.fetch_error()?)`. In `provider_job.rs`, `Recorded` goes to `updated`, and any outcome with a `fetch_error()` goes into `fetch_errors`.
  - **The held `source.rs`.** `Collected::Failed { kind } => kind != "error",` becomes `Collected::Failed { .. } => true,`. `Collected::Errored` joins the `false` arm with `Ineligible`, `Dropped` and `Unsupported`, laid out by rustfmt. Nothing else changes (Ruling 9).
  - **The test edits.** `common::failed(kind)` builds `Failed { kind: FailureKind::parse(kind) }`. The three `failed("error")` become `Collected::Errored`.
- [ ] **Step 3: Verify.** Run the engine, daemon, TUI and `tagteam` tests and both clippy variants. The safety net, which must pass unchanged:
  - engine `collect.rs`: `a_dead_refresh_quarantines_the_account_and_gives_its_slot_back`, `a_missing_token_or_an_unreadable_vault_is_recorded_without_sending`, `an_expired_token_that_cannot_be_refreshed_is_token_expired_and_sends_nothing`, `failures_never_touch_the_last_good_reading`, `an_identity_over_its_hourly_budget_is_sent_nothing`;
  - `collect_active.rs`: `one_accounts_error_keeps_the_others_outcomes_and_warnings`, `a_replaced_live_login_reports_live_replaced_and_names_the_forced_switch`;
  - `session_usage.rs`: `an_expired_profile_token_is_token_expired_without_a_request_and_gives_its_slot_back`;
  - `tagteam` `usage_cli.rs`: `a_429_is_a_row_that_says_when_it_is_retried_never_a_command_error`, `an_unread_account_over_its_hourly_budget_reads_as_over_budget_not_no_data`, `a_locked_keychain_row_says_when_it_is_retried_in_words_and_in_json`;
  - `daemon_procs.rs`: `the_hourly_budget_holds_with_a_daemon_collecting_beside_list`;
  - daemon `provider_job.rs`: `an_unreadable_item_beside_answered_reads_starts_no_backoff`, `without_keychain_counts_a_provider_never_backs_off`;
  - `protocol.rs`: `each_daemon_kind_is_one_object_in_the_envelope_and_leaves_out_what_it_lacks`.
- [ ] **Step 4: Commit** "Write usage failures as FailureKind and name the errored outcome".

### Task 3: Type the refresh gate's transient kinds as `GateFailure`

**Depends on:** Tasks 1 and 2.

**Files:**
- Modify:
  - `failure.rs`;
  - `refresh.rs`: `GateOutcome::Transient` (`:59-64`) and its `Debug` arm (`:79-81`), `transient` (`:87-92`), `log_gate` (`:399-405`), and the writers at `:495`, `:511`, `:521-522`, `:526`, `:540`, `:543`, `:548-549`, `:644`, `:654`, `:762-765` and `:773-776`;
  - `active.rs` (`:56-58`; the log at `:145-151`; `:408-410`; `:417`);
  - `switch/freshen.rs` (`:158`, `:180-182`, `:257`, `:271`, `:304`);
  - `launch.rs` (`:330`, `:334`);
  - `collect.rs` (`:855-857`).
- Test edits: `tests/gate.rs:320,451,461,477,536`, `gate_persist.rs:88,117,237,458`, `log_lines.rs:982`, `provenance.rs:58`, and `active.rs:330` with its imports.
- New test: `tests/log_lines.rs`.

**Interfaces (produces):**

```rust
/// Why the refresh gate (§7.3) or active-token refresh (§7.5) did nothing decisive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateFailure {
    VaultAbsent, NotRefreshable, VaultUnreadable, RescueUnreadable, ProfileUnreadable,
    RefreshFailed, VaultWrite,
    Provider(TransientKind),
}
impl GateFailure { pub fn as_str(&self) -> Cow<'static, str>; }
// refresh.rs: GateOutcome::Transient { kind: GateFailure, rescued: bool };
//             fn transient(kind: GateFailure) -> GateOutcome
// active.rs:  ActiveOutcome::Transient { kind: GateFailure }
```

The named spellings, in order, are `vault-absent`, `not-refreshable`, `vault-unreadable`, `rescue-unreadable`, `profile-unreadable`, `refresh-failed` and `vault-write`.

- [ ] **Step 1: Write the tests.** They must fail to compile.
  - **`failure.rs`,** `every_gate_failure_keeps_its_spelling`: the seven named variants, plus `Provider(PreSend)` as `"pre-send"` and `Provider(Http(503))` as `"http-503"`, each with `as_str() == s`.
  - **`log_lines.rs`,** `an_active_token_refresh_that_fails_transiently_logs_its_kind_as_a_string`. Under `one_at_a_time()`, set the live `expiresAt` to now, as `an_active_token_refresh_logs_its_outcome_by_provider_and_account` does, and script no token reply. `capture_logs` around `refresh_active(.., ActiveTrigger::Expired)` gives `Transient { kind: GateFailure::Provider(TransientKind::PreSend) }`. On the one `"active-token refresh outcome"` line, `field(line, "outcome")` is `"\"transient\""` and `field(line, "kind")` is `"\"pre-send\""`.
- [ ] **Step 2: Implement.**
  - **The writers.** Each literal becomes its variant. `RefreshResult::Transient(kind)` becomes `GateFailure::Provider(kind)`, in `verdict` and in `active.rs`.
  - **The readers.** `Transient { kind, .. } if kind == "rescue-unreadable"` becomes the pattern `Transient { kind: GateFailure::RescueUnreadable, .. }`, in `freshen.rs` and `launch.rs`.
  - **Text that embeds a kind** goes through `as_str()`: `cannot_refresh(.., &kind.as_str())`, `AutoFreshened::Failed(kind.as_str().into_owned())` and its two pre-gate writers, and `warn_refresh(&kind.as_str())`.
  - **Logs and `Debug`.** Both outcome lines log `kind = &*kind.as_str()`. `GateOutcome`'s `Debug` prints `kind.as_str()` with `{:?}`.
  - **The test edits.** `kind == "…"` becomes `kind.as_str() == "…"`, with the same literals. `gate.rs:320` compares `(&*kind.as_str(), rescued)`. `active.rs:330` expects `GateFailure::Provider(TransientKind::PreSend)`.
- [ ] **Step 3: Verify.** Run the engine tests and both clippy variants. The safety net, which must pass unchanged:
  - `gate.rs`: `transport_failures_are_transient_and_change_nothing`, `an_unreadable_or_absent_vault_is_transient_without_a_request`, `an_unreadable_rescue_blocks_the_request`, `invalid_grant_after_the_lineage_moved_is_not_a_strike`;
  - `gate_persist.rs`: its four `vault-write` tests;
  - `log_lines.rs`: `a_gate_refresh_logs_its_outcome_once_at_its_level`;
  - `freshen.rs`: `offline_the_switch_proceeds_with_the_vault_generation_and_a_warning`, `a_rescue_the_vault_cannot_adopt_refuses_with_a_detail_that_says_so`;
  - `launch.rs`: `offline_the_launch_goes_on_with_the_stored_credential_and_a_warning`, `a_gate_refusal_before_the_locks_refuses_a_quiescent_launch_before_anything_is_written`;
  - `auto_tick.rs`: `a_target_that_cannot_be_freshened_for_now_is_an_error_at_the_normal_cadence`, `a_target_whose_profile_cannot_be_read_fails_freshening`, `a_target_with_an_unreadable_rescue_fails_freshening`;
  - `collect_active.rs`: `a_stuck_rejected_fp_goes_to_active_refresh_before_any_request`.
- [ ] **Step 4: Commit** "Type the refresh gate's transient kinds as GateFailure".

## Final verification (the controller, before the PR)

- **Tests:** all CI commands on macOS outside the sandbox, with totals as passed/total; `cargo test` in the Linux Docker reproduction.
- **Held files:** `git diff --stat refactor-r3-cli-protocol..HEAD -- crates/tagteam-tui crates/tagteam/src/tui_host.rs crates/tagteam/src/tui_cmd.rs` lists only `source.rs`.
- **Spellings:** in the engine's, daemon's and `tagteam`'s production code, each enum token appears only in `failure.rs`, except `error.rs`'s `foreign-credential` and `rescue-unreadable` (§8.5). The outcome's `"error"` appears only in `fetch_error`.
- **Records:** add the PR to the spec's Status line. The final pre-merge commit marks this plan `Implemented`.

## Rulings made while planning

These are recorded under "Rulings" in the PR.

1. **No `str_enum!`.** Both enums carry data, so `as_str` cannot return `&'static str`, and `FailureKind::parse` is total. A unit-only macro fits §8.7's enums, and lands with them.
2. **`as_str` returns a `Cow`,** because `Http(n)` is formatted. `FailureKind::parse` returns `Self`, falling back to `Other`.
3. **Canonical values.** `From` maps `Http(401)` to `Http401`, and `parse` prefers named tokens. `TransientKind::parse` (beside `token`) accepts only what `token` writes, so `parse(s).as_str() == s`.
4. **No `GateFailure::parse`:** nothing reads one back.
5. **`GateFailure` also types `ActiveOutcome::Transient`.** The spec's "`active.rs:409`" is §7.5's outcome, and it has the same kinds.
6. **`AutoFreshened::Failed` stays a `String`,** because it also carries free text. Typing it is §8.3's work.
7. **The enums live in `failure.rs`,** not in hardening's `refresh.rs`, and not in `collect.rs`, which the store would have to import.
8. **`NO_DATA` stays outside `FailureKind`:** it is the view's "never read" sentinel, not a token. `Collected::fetch_error` is the one place for the reported spellings.
9. **The held `source.rs` gets a code-only edit.** Its stale doc comment (`Failed { kind: "error" }`) is left for the TUI redesign.
10. **`refresh.rs` edits** beyond the writers are limited to the field type, its `Debug` arm, `transient` and `log_gate`.
11. **§14's comment sweep of R4's files** goes in R4f (§8.9's slice), which sweeps every engine and daemon file that still carries a plan-local reference. R4a's new comments cite spec sections only.
