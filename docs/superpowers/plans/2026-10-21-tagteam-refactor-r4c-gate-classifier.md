# tagteam refactor R4c — the refresh gate's outcome table — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Implemented — https://github.com/michaelrigart/tagteam/pull/33
**Branch:** `refactor-r4c-gate-classifier`, based on `refactor-r4b2-store-lock-tokens` (R4b ships as two PRs)
**Goal:** One classifier reads the refresh gate's outcome for the manual switch, the automatic switch and launch (CC-E-4), keeping every intentional difference and text, except that `tagteam run` now names the damaged files when a pending rescue cannot be read (C-2).
**Architecture:** `switch/freshen.rs` gains `GateMode`, `GateVerdict` and `Engine::classify_gate` (one `GateOutcome` in, one verdict per mode out). `freshen`, `freshen_auto` and `freshen_for_launch` keep their pre-gate work and map the verdict to `Freshened`, `AutoFreshened` or `LaunchFresh`. Auto's failure reason is typed first (`FreshenFailure`), so the verdict can carry it.
**Tech Stack:** Rust 1.88 (edition 2024).

**Spec:** docs/superpowers/specs/2026-10-19-tagteam-review-refactor-design.md (§8.3, with §2's C-2, §3, §11 and §12)

**Reading this plan.**
- **Sites** are named by function, at `8d44998` after R4a's Task 3, which makes the gate kinds `GateFailure` (`Transient { kind: GateFailure::RescueUnreadable, .. }`, `cannot_refresh(.., &kind.as_str())`, `AutoFreshened::Failed(kind.as_str().into_owned())`). R4b changes no site here.
- **Focused tests:** `cargo test -p tagteam-engine --locked --features test-hooks`; Task 3 also runs it without the feature (`tests/launch.rs` has a hook-gated module).
- **Order:** 1 → 2 → 3; all three edit `switch/freshen.rs`.

## Global Constraints

- **Byte-identical output** (§1.4), except §11's C-2 row (Task 3, with its `CHANGELOG.md` bullet). Every auto event message, manual warning and refusal, launch warning and refusal, and log line stays as it is.
- **No wire, `--json` or store change.** No `EngineError::kind()` changes: launch's refusal stays `rescue-pending`.
- **§8.3's intentional differences are kept:** launch joins session-owned accounts; auto skips or quarantines where manual refuses; manual replans only for a chosen target; auto carries no user-facing warnings; a quarantined row short-circuits launch before its vault is read, where manual applies its quarantined-target rule.
- **Not edited:** §3.2's hardening files (`refresh.rs` included: `GateOutcome` stays as R4a leaves it); `collect.rs`, whose `gate` is not folded in (§8.3); the held TUI files (every new item is `pub(crate)`).
- **Comments** on moved arms move with them, merged where two say the same. New comments cite core spec sections (§7.2, §7.3, §11.2 step 10, §12.3 step 1); R4g does the §14 sweep. Lines wrap at 100.
- **Unit-test modules stay last,** with `#[cfg(test)]` directly above `mod tests {` (`neutrality.rs`).
- **CI** (`CONTRIBUTING.md`): `cargo fmt --all --check`; both clippy variants (with and without `--features tagteam/test-support`) in every task; `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`.
- **Tests and commits.** No test is weakened: only the listed edits, plus new tests. No new `#[allow]`. Commits carry no agent attribution; messages are imperative, sentence case, no prefix or trailing period.

## Review Focus

1. **Several damaged rescue files at a due launch:** all named, in name order, joined by `, `. Test: Task 3, "two unreadable".
2. **A readable rescue the vault cannot adopt at a due launch:** the fallback "a pending rescue could not be adopted", never ` cannot be read` after an empty list. Test: Task 3, "unadoptable".
3. **A due launch into a running session** joins, sends nothing, warns nothing (§8.3's first difference, through the gate). Test: Task 3, the join test's `"due"` case.
4. **Auto's texts for a rescued successor and a systemic refusal, and a lost successor's quarantine,** which no test pins today. Tests: Task 2.
5. **Cells no fixture reaches without a new hook point** (Ruling 5): the reviewer checks each against Task 2's table and the old arms at the R4b head.

## Spec coverage

| Spec | Task |
|---|---|
| §8.3: `Failed`'s payload | 1 (Ruling 1) |
| §8.3: the classifier; manual and auto mapped | 2 |
| §8.3: launch mapped, `LaunchFresh`, `profile_conflict()`; C-2; §11's row | 3 |
| §8.3's and §12's tests | 2, 3 |

---

### Task 1: Type the automatic switch's freshen failures

**Depends on:** none.

**Files:** modify `crates/tagteam-engine/src/switch/freshen.rs` (`AutoFreshened`; `freshen_auto`'s pre-gate writers and gate match; a new tests module), `switch/mod.rs` (the re-export) and `auto.rs` (`perform`'s `Err(e) => AutoFreshened::Failed(..)`).

**Interfaces (produces):**

```rust
/// Why an automatic switch's target could not be freshened (§11.2 step 10), as the tick's
/// error names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FreshenFailure {
    /// A gate kind (§7.3), spelled by `GateFailure::as_str`.
    Gate(GateFailure),
    /// The gate's successor is only in `rescue/` (`Transient { rescued: true, .. }`).
    Rescued,
    /// In words: the token endpoint's systemic refusal, or an error's text.
    Text(String),
}
impl fmt::Display for FreshenFailure { .. }
pub(crate) enum AutoFreshened { Ready, Quarantined, Failed(FreshenFailure), Skip }
```

Re-export: `pub(crate) use freshen::{AutoFreshened, FreshenFailure};`.

- [ ] **Step 1: The test.** Add `a_freshen_failure_reads_as_the_reason_the_tick_reports` in a new `#[cfg(test)] mod tests` at the end of `freshen.rs`. It asserts `to_string()` for each row:

  | Value | Text |
  |---|---|
  | `Gate(GateFailure::RescueUnreadable)` | `"rescue-unreadable"` |
  | `Gate(GateFailure::ProfileUnreadable)` | `"profile-unreadable"` |
  | `Gate(GateFailure::Provider(TransientKind::PreSend))` | `"pre-send"` |
  | `Gate(GateFailure::Provider(TransientKind::Http(503)))` | `"http-503"` |
  | `Rescued` | `"its refreshed token is in rescue/, not yet in the vault"` |
  | `Text("invalid_client".into())` | `"invalid_client"` |

  Expected: it fails to compile.
- [ ] **Step 2: Implement.**
  - **`freshen_auto`'s pre-gate writers.** The `RescuePending` arm gives `Gate(GateFailure::RescueUnreadable)`, and `ProfileCheck::Unreadable` gives `Gate(GateFailure::ProfileUnreadable)`. The two `Err(EngineError::Unreadable(e))` arms and `Read::Unreadable(e)` give `Text(e.to_string())`.
  - **Its gate match.** `rescued: true` gives `Rescued`, `Transient { kind, .. }` gives `Gate(kind)`, and `Systemic(detail)` gives `Text(detail)`.
  - **`auto.rs`.** `Err(e) => AutoFreshened::Failed(FreshenFailure::Text(e.to_string()))`. The `failed.push(format!(.. "{why}"))` line is unchanged.
- [ ] **Step 3: Verify.** Run the focused tests and both clippy variants. The net, unchanged: `auto_tick.rs`'s `a_target_that_cannot_be_freshened_for_now_…`, `a_target_whose_profile_cannot_be_read_…`, `a_target_with_an_unreadable_rescue_…` and `a_dead_target_is_quarantined_once_…`.
- [ ] **Step 4: Commit** "Type the automatic switch's freshen failures".

### Task 2: One classifier for the manual and automatic switch

**Depends on:** Task 1.

**Files:**
- Modify: `switch/freshen.rs` (new items; `freshen`'s gate match and its `label` and `pending` closures; `freshen_auto`'s gate match) and `switch/mod.rs` (the re-export).
- Test: `crates/tagteam-engine/tests/auto_tick.rs`.

**Interfaces:**
- Consumes: Task 1's `FreshenFailure`.
- Produces, in `freshen.rs`, re-exported as `pub(crate) use freshen::{AutoFreshened, FreshenFailure, GateMode, GateVerdict};`:

```rust
/// Which table reads the gate's outcome: the manual switch's (§7.2; `chosen`: a rotation's or
/// a usage strategy's pick) or the automatic switch's (§11.2 step 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GateMode { Manual { chosen: bool }, Auto } // Task 3 adds Launch
#[derive(Debug)]
pub(crate) enum GateVerdict {
    Proceed, ProceedWarn(String), Replan, Refuse(EngineError),
    Skip, Quarantined, Failed(FreshenFailure),
}
impl Engine { // writes nothing, sends nothing
    pub(crate) fn classify_gate(&self, out: GateOutcome, p: &dyn Provider, row: &AccountRow,
        mode: GateMode) -> Result<GateVerdict, EngineError>;
}
```

**The table.** Arms in this order. Task 3 adds the Launch column.

| `GateOutcome` | Manual | Auto | Launch (Task 3) |
|---|---|---|---|
| `Refreshed(_)`, `AlreadyFresh(_)`, `Busy` | Proceed | Proceed | Proceed |
| `Owned(Live)` | Warn(`it may be the live login`) | Proceed | Proceed |
| `Owned(Journal)` | Warn(`an unfinished switch names it`) | Proceed | as Manual |
| `Owned(Session)` | chosen: Replan; else Refuse(`session_owned(p, row, &self.session_state(p, row)?)`) | Skip | Proceed |
| `Conflict` | Refuse(`profile_conflict(row)`) | Skip | as Manual |
| `Dead(_)` | chosen: Replan; else Refuse(`needs_relogin(p, row)`) | Quarantined | Proceed |
| `Unpersisted` | Refuse(`needs_relogin(p, row)`) | Quarantined | as Manual |
| `Transient { rescued: true, .. }` | Refuse(pending(RESCUED)) | Failed(`Rescued`) | as Manual |
| `Transient { kind: RescueUnreadable, .. }` | Refuse(pending(damaged)) | Failed(`Gate(kind)`) | as Manual |
| `Transient { kind, .. }` | Warn(`&kind.as_str()`) | Failed(`Gate(kind)`) | as Manual |
| `Systemic(d)` | Warn(`&d`) | Failed(`Text(d)`) | as Manual |

The table's terms:
- **Warn(w)** is `ProceedWarn(cannot_refresh(p.display_name(), &row.label, w))`.
- **pending(d)** is `EngineError::RescuePending { position: row.position, label: row.label.clone(), detail: d }`.
- **RESCUED** is `the refresh succeeded, but the vault could not be written; the new token is in rescue/`.
- **damaged** moves from `freshen`'s arm unchanged. It is the paths of `self.rescues_for(&row.id)`'s `Unreadable` files, joined by `, `, followed by ` cannot be read`. With none, it is `a pending rescue could not be adopted`.

- [ ] **Step 1: The tests** (`auto_tick.rs`). They pin today's table, so they pass before the change. Each case starts from the fixture of `a_target_that_cannot_be_freshened_for_now_is_an_error_at_the_normal_cadence` (`a`, then `c` live at its limit, both read, `fx.expire_access(&a)`) on a fresh `Fx`.
  - **`a_target_whose_refresh_is_rescued_or_refused_fails_freshening_with_the_reason`:**

    | Case | Setup | `message` |
    |---|---|---|
    | rescued | `fx.script_refresh(Some("rt-a-2"))`; `fx.kc.set_fail_write(SERVICE, true)` | `"could not freshen a@x.co (position 1): its refreshed token is in rescue/, not yet in the vault"` |
    | systemic | `fx.script_token_error(400, "invalid_client")` | `"could not freshen a@x.co (position 1): invalid_client"` |

    For each case: the tick's outcome is `TickOutcome::Error`; `sink.take()[1..]` is `[AutoEvent::Error { provider, message, transient: true }]`; `fx.live_email()` is `c@x.co`. Reset `set_fail_write` after the tick.
  - **`an_unpersisted_target_is_quarantined_and_leaves_no_viable_target`:** setup `script_refresh(Some("rt-a-2"))`, `set_fail_write(SERVICE, true)` and `block_rescue(&fx)`, both undone after the tick. The outcome is `TickOutcome::Blocked`; `sink.take()[1..]` is `[AccountQuarantined { provider, number: 1, email: "a@x.co", reason: "successor_lost" }, no_switch(&provider, "no-viable-target", "")]`; `quarantine_of(&fx, &a).0` is `Some("successor_lost")`; the live login is `c@x.co`.

  Run them. Expected: PASS.
- [ ] **Step 2: Implement `classify_gate`** by the table. Its arms carry the comments of the arms they replace.
- [ ] **Step 3: Map the callers.** Each still does `check_cancel` first, then `classify_gate(self.refresh_stored(p, &target.id, &vault)?, p, target, mode)?`.
  - **`freshen`** (`GateMode::Manual { chosen }`): Proceed → `Go(vec![])`, ProceedWarn(w) → `Go(vec![w])`, Replan → `Replan`, Refuse(e) → `return Err(e)`; the rest `unreachable!("classify_gate gives a manual switch no automatic verdict")`. Its `label` and `pending` closures go.
  - **`freshen_auto`** (`GateMode::Auto`): Proceed → `Ready`; Skip, Quarantined and Failed(f) → their namesakes; the rest `unreachable!("classify_gate gives an automatic switch no manual verdict")`.
- [ ] **Step 4: Verify.** Run the focused tests and both clippy variants. These pin the table and must pass unchanged:
  - `freshen.rs`, every test;
  - `auto_tick.rs`, every test;
  - `provenance.rs`: `a_due_target_with_a_conflicting_profile_refuses_before_any_request`.
- [ ] **Step 5: Commit** "Read the refresh gate's outcome through one classifier for manual and automatic switches".

### Task 3: Launch reads the gate through the classifier and names damaged rescue files

**Depends on:** Task 2.

**Files:**
- Modify: `switch/freshen.rs` (`GateMode::Launch`); `switch/support.rs` and `switch/mod.rs` (`profile_conflict`, `pub(crate)` and re-exported beside `needs_relogin`); `launch.rs` (`Freshened`, `launch`, `freshen_for_launch`, `prepare_quiescent`, `exit_under_locks`, imports); `CHANGELOG.md`.
- Test: `crates/tagteam-engine/tests/launch.rs`.

**Interfaces:**
- Consumes: Task 2's `GateMode`, `GateVerdict` and `classify_gate`.
- Produces:
  - `GateMode::Launch`, the launch column (§12.3 step 1). Its doc says that a refusal is carried across the locks.
  - `#[derive(Default)] struct LaunchFresh { warnings: Vec<String>, refusal: Option<EngineError> }`, the renamed `Freshened` with its docs.
  - `pub(crate) fn profile_conflict(target: &AccountRow) -> EngineError`.

- [ ] **Step 1: The tests** (`launch.rs`).
  - **New, top level: `an_unreadable_rescue_refuses_a_due_launch_naming_the_damaged_files`.**
    - **Each case:** a fresh `Fx`, `a = two_accounts(&fx)`, `fx.expire_access(&a)`, `dir = fx.env.data_dir().join("rescue")`, the setup below, then `err = refused(fx.engine.launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")))`. `err.to_string()` is `format!("a@x.co (position 1) has a refreshed token that is not in the vault yet: {detail}; retry once the vault can be written")`, `token_requests(&fx)` is 0, and `!fx.profile_dir(&a).exists()`.

      | Case | Setup | `detail` |
      |---|---|---|
      | one unreadable | `{a}-0-000000000000.json` in `dir`, holding `{ torn` | `"{p0} cannot be read"` |
      | two unreadable | the same file, and `{a}-1-000000000000.json` | `"{p0}, {p1} cannot be read"` |
      | unadoptable | `fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"))` and `fx.kc.set_fail_write(SERVICE, true)`; reset after the launch | `"a pending rescue could not be adopted"` |

      `p0` and `p1` are the files' `display()`.

    Expected: the unreadable cases FAIL, because their text is today's fixed one. The unadoptable case passes.
  - **Edit:** `a_gate_refusal_before_the_locks_refuses_a_quiescent_launch_before_anything_is_written` (hook-gated). Its `"rescue unreadable"` row expects the same message with `detail` = `"{planted file} cannot be read"`, built inside the loop: §11's change (Ruling 6). Expected: FAIL.
  - **Edit:** `a_join_launches_whatever_the_vault_holds_and_touches_no_credential` gains the case `"due"`: `fx.expire_access(&a)`, no quarantine. The gate answers `Owned(Session)` and the join goes on, under the test's own assertions (no warning, no Keychain item touched, no request). Expected: PASS.
- [ ] **Step 2: Implement.**
  - **`GateMode::Launch`,** with the table's Launch column.
  - **`LaunchFresh`:** rename `Freshened`, in `launch`'s destructuring and in `freshen_for_launch`'s doc ("`LaunchFresh.refusal`").
  - **`freshen_for_launch`** keeps its pre-gate work (the unrefreshable-or-quarantined short-circuit, `vault_generation` with `removed_or`, the due check, `check_cancel`) and maps `GateMode::Launch`'s verdict: Proceed → `LaunchFresh::default()`, ProceedWarn(w) → `{ warnings: vec![w], refusal: None }`, Refuse(e) → `{ warnings: Vec::new(), refusal: Some(e) }`; the rest `unreachable!("classify_gate gives a launch no switch verdict")`. Its `app`, `pending`, `warned` and `refused` closures go.
  - **`profile_conflict(cx.row)`** in `prepare_quiescent` and **`profile_conflict(&row)`** in `exit_under_locks`.
  - **Imports:** drop the now unused `GateOutcome`, `OwnedBy`, `GateFailure` and `cannot_refresh`.
- [ ] **Step 3: `CHANGELOG.md`.** Add a last bullet under `## [Unreleased]` → `### Changed`, after R4b's:

  > - When `tagteam run` cannot refresh an account's token because a pending rescue cannot be read, it names the damaged files, as `tagteam switch` does; it said only that a pending rescue could not be adopted.
- [ ] **Step 4: Verify.**
  - Run the focused tests with and without `--features test-hooks`, and both clippy variants.
  - Every other `launch.rs` test passes unchanged (the gate's offline, rescued, conflict, quarantine and busy cases among them).
  - `git grep -n 'ProfileConflict {$' -- crates/tagteam-engine/src` finds only `support.rs` and `error.rs`'s test.
- [ ] **Step 5: Commit** "Read launch's gate outcome through the classifier and name damaged rescue files".

## Final verification (the controller, before the PR)

- **Tests:** the CI commands on macOS outside the sandbox (passed/total), and `cargo test` in the Linux Docker repro.
- **Untouched:** `git diff --stat refactor-r4b-daemon-store-contracts..HEAD -- crates/tagteam-engine/src/refresh.rs crates/tagteam-engine/src/collect.rs crates/tagteam-tui crates/tagteam/src/tui_host.rs crates/tagteam/src/tui_cmd.rs` is empty.
- **One table:** `git grep -n 'GateOutcome::' -- crates/tagteam-engine/src/switch crates/tagteam-engine/src/launch.rs` finds only `classify_gate`.
- **Records:** add the PR to the spec's Status line. The final pre-merge commit marks this plan `Implemented`.

## Rulings (recorded under "Rulings" in the PR)

1. **`GateVerdict::Failed` carries `FreshenFailure`, not `FailureKind`.** Auto's failures are gate kinds, the rescued phrase and free text (systemic messages, errors), never usage tokens; `FailureKind` is `last_error`'s namespace (§8.6.1), and its `Other(String)` would put endpoint text there. `FreshenFailure` builds on `GateFailure`, its `Display` keeps every text byte-identical, and it types what R4a's Ruling 6 left to §8.3. Rejected: `String`, one enum simpler, but kinds and text become indistinguishable.
2. **One verdict enum for three modes;** a caller marks the verdicts its mode never gets `unreachable!` with a reason, as `app/dispatch.rs` does. Rejected: a verdict type per mode, which splits the table three ways again.
3. **Pre-gate work stays in the callers,** manual's quarantined-target rule and launch's short-circuit included. `classify_gate` reads only the outcome, the session state (manual, direct target) and the rescue listing.
4. **Arm order:** `rescued: true`, then `RescueUnreadable`, then other kinds, in every mode, as today.
5. **§12's "one case per row" holds per row, through at least one caller.** New cells: auto's unpinned texts (rescued, systemic, unpersisted), launch's rescue text, and the due join. Left to review, with no hook point added: manual's `RescueUnreadable` (`freshen` settles rescues under the account lock first, so `settle_rescues` answers), `Owned(Live|Journal|Session)` and a gate `Conflict` under auto, and `Owned(Live|Journal)` under launch.
6. **The launch hook test's "rescue unreadable" row** expects the file-naming text: §11's change, not a weakened test.
7. **Placement and names.** `FreshenFailure` is crate-private in `switch/freshen.rs`, beside `AutoFreshened` (`failure.rs` holds the public vocabularies). Manual's private `Freshened` keeps its name; §8.3 renames only launch's.
8. **`profile_conflict` covers launch's three inline constructions,** which the spec cites at `launch.rs:323,421,629` (`683b266`): now `freshen_for_launch`, `prepare_quiescent` and `exit_under_locks`. It becomes `pub(crate)`.
9. **The `CHANGELOG.md` bullet goes under `### Changed`,** after R4b's.
