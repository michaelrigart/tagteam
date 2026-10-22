# tagteam refactor R4g — shared helpers and plan-local comments — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Approved
**Branch:** `refactor-r4g-helpers-comments`, based on `refactor-r4f2-typed-items`; rebased onto `main` after R4f2 (#39) and T4 (#40) merged (see "Task 3 amendment")
**Goal:** CC-E-13's shared helpers (`display_email`, one lock-record reader, one jitter, one store cache), CC-E-16's `try_target`, and §14's sweep of comments citing plan-local numbering; every output unchanged.
**Architecture:** `display_email` on `Identity` and `AccountRow` replaces 14 copies (Task 1). `read_lock_record`, a public `poll::jittered` and `Engine::cached_store` each serve their copies (Task 2). `perform` loops over `try_target`, handing a decision back only when it replaces the `Switch` (Task 3). Task 4 rewrites comments only.
**Tech Stack:** Rust 1.88 (edition 2024); serde_json.

**Spec:** docs/superpowers/specs/2026-10-19-tagteam-review-refactor-design.md (§8.9's CC-E-13 and CC-E-16; §14; with §3, §8.10). R4f holds §8.8 and the rest of §8.9.

**Reading this plan.**
- **Sites** are functions at `8d44998`. R4e drops `SwitchRequest.source` and `AutoEngine::with_source`; R4f changes `QuarantineWatch::report`'s `set` branch and `auto.rs`'s conversions. Keep what R4a–R4f made of every line not named here.
- **Focused tests:** `cargo test -p <crate> --locked`, `--features test-hooks` for engine and daemon, `--features test-support` for `tagteam`.
- **Order:** 1 → 4, one at a time (1 and 3 share `auto.rs`; 4 sweeps the final text).

## Global Constraints

- **Byte-identical output** (§1.4): messages, `--json`, log lines, events, stored data. No §11 row: no `CHANGELOG.md`. No wire or schema change.
- **Hardening's files** (§3.2, plus the daemon's `client.rs` and the binary's `run.rs`/`prompt.rs`): none edited. `tagteam-cc` changes only in `doctor.rs`, `provider.rs` and `session.rs`, one expression each.
- **Held files** (§3.4): none edited. `words::email` keeps its signature: the held `tagteam-tui/src/actions.rs` calls it.
- **Task 4 changes comments only;** Task 3's moved code keeps its comments. New comments cite spec sections.
- Unit-test modules stay last, `#[cfg(test)]` alone above `mod tests {`. Comments wrap at 100.
- **CI** (`CONTRIBUTING.md`): fmt; both clippy variants every task; the workspace test command.
- New tests only, no existing test edited; no new `#[allow]`. Commits imperative, sentence case, no prefix/trailing period/agent attribution.

## Review Focus

1. **A login or account without an email** (an API key, a provider whose logins have none) is named by its label at every converted site. Task 1's tests.
2. **A lock record with a pid over `u32`, no version, or no JSON** stays `Unreadable`, each record in its own words. Task 2's pins; `daemon.rs`'s `a_missing_or_empty_record_is_absent_and_any_other_shape_unreadable`.
3. **The decision a tick reports from each exit of `perform`:** the `Switch` when it switched, dry-ran or failed; the no-switch for a cooldown, a changed live login or no viable target. Task 3's net.
4. **A non-finite or out-of-range jitter** in all three callers. Task 2's net.
5. **A sweep that slips in a code change.** Task 4's comment-only check.

## Spec coverage

| Spec | Task |
|---|---|
| §8.9 CC-E-13: `display_email` | 1 |
| §8.9 CC-E-13: `read_lock_record`, `poll::jittered`, the cache-or-open helper | 2 |
| §8.9 CC-E-16 | 3 |
| §14, including `bootstrap.rs`'s stale comment | 4 |

---

### Task 1: Name an account by `display_email`

**Depends on:** none.

**Files:**
- Modify: `crates/tagteam-provider/src/provider.rs` (`impl Identity`, tests); `crates/tagteam-engine/src/store/mod.rs` (R4b's `impl AccountRow`).
- **Engine copies:** `auto.rs` (`email_of` goes; its callers in `QuarantineWatch::report` ×2, `note_quarantined`, `poll`); `import.rs` (`email_of` goes; `imported`; `import`'s failure row); `provenance.rs` (`identity_drifted`, both sides); `switch/mod.rs` (`login_email` goes; its caller `switch/plan.rs::plan`); `views.rs` (`LiveLogin::of`, `view`); `words.rs` (`email`'s body).
- **Elsewhere:** binary `app/commands/accounts.rs` (`session_login`), `displaced_cmd.rs` (`identity`); `tagteam-cc` `doctor.rs` (`auth`), `provider.rs` (`identity_key`), `session.rs` (`validity`).
- Test: `crates/tagteam-engine/tests/store.rs`.

**Interfaces (produces):**

```rust
impl Identity { /// The email, or the label when the login has none: how tagteam names a login.
    pub fn display_email(&self) -> &str; }
impl AccountRow { /// The stored email, or the label: how tagteam names an account (§13.1).
    pub fn display_email(&self) -> &str; }
```

- [ ] **Step 1: Write the tests** (they must fail to compile).
  - **`a_login_is_named_by_its_email_or_else_its_label`** (`provider.rs` tests): an `Identity` with `email: Some("a@x.co")`, `label: "A"` gives `"a@x.co"`; with `email: None`, `"A"`.
  - **`an_account_row_is_named_by_its_email_or_else_its_label`** (`tests/store.rs`): the row `add(&s, &cc(), "a", "a@x.co", 1)` stores gives `"a@x.co"`; `AccountRow { email: None, label: "key-1".into(), ..row }` gives `"key-1"`.
- [ ] **Step 2: Implement.** Each copy becomes `x.display_email()`, with `.to_owned()` where a `String` is needed; an owned `Identity` borrows before its fields move (`displaced_cmd.rs`).
- [ ] **Step 3: Verify.** Provider, cc, engine, `tagteam` focused tests; both clippy. Every test asserting an account's email or label, in text, JSON or an event, passes unchanged (`words.rs`'s `api-key-4@token.local` case among them).
- [ ] **Step 4: Commit** "Name accounts and logins by display_email".

### Task 2: Share the lock-record reader, the jitter and the store cache

**Depends on:** Task 1.

**Files:** modify engine `auto.rs` (`read_lock_record`, `read_holder`, tests), `daemon.rs` (`read_daemon_record`, tests), `engine.rs` (`cached_store`, `store`, `existing_store`); core `poll.rs` (`jittered`), `autoswitch.rs` (`next_delay`); daemon `provider_job.rs` (`collect_delay_s`).

**Interfaces (produces):**

```rust
/// A lock holder's record at `path` (§11.1, D4.1), read without trying the lock. `Absent`: no
/// file, or an empty one. `Unreadable` with `shape` as its detail: not JSON, without a `u32`
/// `pid` and a `u64` `start`, or refused by `fields`. An I/O failure keeps its own detail.
pub(crate) fn read_lock_record<T>(path: &Path, shape: &'static str,
    fields: impl FnOnce(ProcessStamp, &serde_json::Value) -> Option<T>) -> Read<T>;
/// `interval_s × (1 + jitter · frac)`, rounded; `jitter` clamped into [-1, 1], a non-finite
/// one read as 0 (§8.6, §11.4).
pub fn jittered(interval_s: i64, jitter: f64, frac: f64) -> i64; // tagteam_core::poll, was private
/// The cached store, else one opened (created when `create`) and cached (§5, Decision 11).
/// `None` only when `!create` and there is no store.
fn cached_store(&self, create: bool) -> Result<Option<Arc<Store>>, EngineError>;
```

- [ ] **Step 1: Write the pins** (they pass now and must pass after).
  - **`a_holder_record_is_absent_present_or_unreadable_in_its_own_words`** (`auto.rs` tests): no file and an empty one are `Absent`; `{"pid":1,"start":2}\n` is `Present(ProcessStamp { pid: 1, start: 2 })`; `garbage`, `{"pid":4294967296,"start":2}` and `{"pid":1}` are `Unreadable` with `detail == "it is not a pid and a start time"`.
  - **`an_unreadable_daemon_record_names_its_shape`** (`daemon.rs` tests): `{"pid":1,"start":2}\n` is `Unreadable` with `detail == "it is not a pid, a start time and a version"`.
- [ ] **Step 2: Implement.**
  - `read_holder` is `read_lock_record(path, "it is not a pid and a start time", |stamp, _| Some(stamp))`; `read_daemon_record` passes its shape and builds `DaemonRecord` from the stamp and `v["version"].as_str()?`.
  - `next_delay`'s jittered delay is `jittered(interval, jitter, JITTER_FRAC).max(1)`; `collect_delay_s`'s is `jittered(interval_s, jitter, JITTER_FRAC)`.
  - `store()` is `Ok(self.cached_store(true)?.expect("a store opened to be created is there"))`; `existing_store()` is `self.cached_store(false)`.
- [ ] **Step 3: Verify.** Core, engine, daemon focused tests; both clippy. Unchanged: `daemon.rs`'s `the_record_is_one_json_line_that_reads_back` and `a_missing_or_empty_record_is_absent_and_any_other_shape_unreadable`; `auto_engine.rs`'s `the_engine_lock_records_its_holder_as_one_json_line` and `a_dropped_engine_leaves_its_lock_file_empty_and_free`; `poll.rs`'s `jitter_scales_the_interval_by_up_to_ten_percent` and `jitter_outside_minus_one_to_one_is_clamped_and_nan_is_ignored`; `autoswitch.rs`'s `everything_else_sleeps_the_jittered_interval`; `provider_job.rs`'s `a_collect_round_waits_its_interval_shortened_to_the_earliest_poll_and_floored`; `engine_basics.rs`'s `reading_commands_never_create_the_store` and `forget_store_makes_the_next_use_open_the_file_again`.
- [ ] **Step 4: Commit** "Share the lock-record reader, the jitter and the store cache".

### Task 3: Extract `try_target` from the automatic switch's `perform`

**Depends on:** Task 2.

**Files:** modify engine `auto.rs` (`run`'s `Switch` arm, `perform`, new `TargetStep` and `try_target`).

**Interfaces (produces):**

```rust
/// What one target of a `Switch` decision came to (§11.2 steps 10 and 11).
enum TargetStep {
    /// Passed over; with the line step 12's error names when freshening failed.
    Next(Option<String>),
    /// The tick ends with this outcome, reporting the `Switch` decision.
    Stop(TickOutcome),
    /// The tick ends with this no-switch, reported in the `Switch` decision's place.
    NoSwitch(TickOutcome, Decision),
}
fn try_target(&mut self, p: &dyn Provider, t: &Ticked<'_>, trigger: Trigger, id: &AccountId,
    sink: &dyn EventSink) -> Result<TargetStep, EngineError>;
/// Steps 10 to 12; `Some` is a no-switch reported in the `Switch` decision's place.
fn perform(&mut self, p: &dyn Provider, t: &Ticked<'_>, trigger: Trigger,
    targets: &[AccountId], sink: &dyn EventSink) -> Result<(TickOutcome, Option<Decision>), EngineError>;
```

- [ ] **Step 1: The net.** At the R4f head, run the engine focused tests; these pin each exit's outcome, decision and events, and must pass unchanged after:
  - `auto_tick.rs`: `a_proactive_tick_switches_and_records_its_departure`, `a_dry_run_tick_writes_nothing_and_says_what_it_would_switch`, `a_dead_target_is_quarantined_once_and_the_next_target_is_switched_to`, `a_target_that_cannot_be_freshened_for_now_is_an_error_at_the_normal_cadence`, `a_target_without_a_stored_credential_leaves_no_viable_target`, `a_target_that_stops_being_a_candidate_moves_the_tick_to_the_next`, `a_cooldown_found_under_the_lock_is_no_switch_cooldown`, `a_manual_switch_during_the_tick_s_perform_is_live_changed`, `a_live_changed_refusal_writes_no_unknown_usage_count`, `a_profile_conflict_that_arises_after_freshening_passes_the_target_over`, `a_switch_that_commits_despite_a_late_signal_is_switched`, `a_target_a_session_takes_after_the_snapshot_is_passed_over`, and R4c's `a_target_whose_refresh_is_rescued_or_refused_fails_freshening_with_the_reason` and `an_unpersisted_target_is_quarantined_and_leaves_no_viable_target`.
- [ ] **Step 2: Implement.**
  - `try_target` holds one iteration's body, moved with its comments, counts and events: a missing row is `Next(None)`; the dry run `Stop(Switched)`; freshening's `Quarantined` (after `note_quarantined`) and `Skip` are `Next(None)`, `Failed(why)` is `Next(Some(line))`; a switch that switched is `Stop(Switched)`; `NotCandidate` and `ProfileConflict` are `Next(None)`; `Cooldown` and `LiveChanged` are `NoSwitch` from `no_switch`; any other no-op answer or error is `Stop(Error)`.
  - `perform` loops, collecting `Next(Some(..))` lines, and returns at the first `Stop` (`None`) or `NoSwitch` (`Some`); step 12 is unchanged (its error `None`, its no-viable-target `Some`).
  - `run` returns `instead.unwrap_or(decided.decision)`, moving the decision: no `decision.clone()` remains.
  - The reviewer reads it with `git diff --color-moved=zebra --color-moved-ws=allow-indentation-change`.
- [ ] **Step 3: Verify.** Engine focused tests; both clippy. `git grep -n 'decision.clone()' -- crates/tagteam-engine/src/auto.rs` finds nothing.
- [ ] **Step 4: Commit** "Extract try_target from the automatic switch's perform".

### Task 4: Rewrite the comments that cite plan-local numbering

**Depends on:** Task 3.

**Files** (each a comment; reflow any paragraph a rewrite shortens):

| File | Item | Now | Becomes |
|---|---|---|---|
| engine `daemon.rs` | `MAX_SOCKET_PATH` (Linux) doc | "Plan B's server refuses" | "The daemon's server refuses" |
| engine `service.rs` | module doc | "(plan B Decision 4)" | "(daemon plan B, Decision 4)" |
| engine `provenance.rs` | `identity_drifted` doc | "(`collect.rs`, Task 12)" | "(`collect.rs`)" |
| engine `views.rs` | `usage_status` doc | "Task 8 stores a reading with no windows as a null `last_good`" | "a reading with no windows is stored with a null `last_good`" |
| engine `bootstrap.rs` | `bootstrap_quiescent` doc | the short first line left by R1–R3's removal of "(Task 9, before `launch` exists)" | reflowed |
| engine `bootstrap.rs` | test `a_bootstrap_of_a_linked_profile_refuses_before_anything_is_written` | "(controller ruling)" | dropped |
| engine `launch.rs` | `launch`, above `refuse_linked_profile` | "(controller ruling)" | dropped |
| engine `launch.rs` | `locked_login` doc | "(controller ruling)" | dropped |
| daemon `daemon.rs` | module doc | "Plan B binds the socket between the two." | "The socket is bound between the two (D5.1)." |
| daemon `daemon.rs` | `DaemonOptions::upgrade_watch_ms` doc | "shorter only in plan B's tests" | "shorter only in the multi-process tests (`TAGTEAM_TEST_UPGRADE_WATCH_MS`)" |
| daemon `daemon.rs` | `Started::board` doc | "plan B's server answers `status` from (Decision 12)" | "the socket server answers `status` from (D5.4, Decision 12)" |
| daemon `daemon.rs` | `Started::serve` doc | "(Task 7)" | dropped |
| daemon `daemon.rs` | test `a_store_created_while_serving_wakes_the_provider_jobs_at_once` | "(Task 10)" | dropped |
| daemon `daemon.rs` | test `a_store_replaced_between_start_and_serving_is_reopened_before_any_provider_work` | "(plan B binds its socket in between)" | "(the socket is bound in between)" |
| daemon `protocol.rs` | `status_json` doc | "(Decision 12 of plan A)" | "(daemon plan A, Decision 12)" |
| daemon `service.rs` | module doc | "plan B Decision 4" | "daemon plan B, Decision 4" |
| daemon `watch.rs` | tests' `baseline` doc | "(Task 12)" | dropped |
| daemon `watch.rs` | test `the_first_run_compares_with_the_baseline` | "(plan B binds its socket in between)" | "(the socket is bound in between)" |
| daemon `watch.rs` | test `the_watch_compares_with_the_stamp_it_is_given_never_with_the_file_when_built` | "(Task 12)" | dropped |
| binary `daemon_cmd.rs` | `TEST_UPGRADE_WATCH_MS` doc | "Plan B Decision 7:" | "Daemon plan B, Decision 7:" |
| binary `logfile.rs` | test `a_disable_callback_may_log_into_the_same_file_without_deadlocking` | "Task 6's `--debug` callback" | "The `--debug` callback" |
| provider `cancel.rs` | test comment at `:160` | "(Task 6)" | dropped |
| provider `http.rs` | test comment at `:453` | "Task 3's adapter test" | "the adapter test", naming it if it exists |

- [ ] **Step 1: Rewrite** each row.
- [ ] **Step 2: Verify.**
  - `git diff -U0 HEAD -- crates | grep -E '^[+-][^+-]' | grep -vE '^[+-]\s*//'` prints nothing: only comment lines changed.
  - `rg -n -e 'Tasks? [0-9]+' -e '\b[Pp]lan [AB]\b' -e 'Controller note' -e 'controller ruling' -e 'fix round' -e 'review, item' -e '\bL[0-9]{3,4}\b' crates/tagteam-engine/src crates/tagteam-daemon/src crates/tagteam/src crates/tagteam-provider/src crates/tagteam-core/src crates/tagteam-cc/src crates/tagteam-fake/src` lists only `refresh.rs` (6), `notify.rs` (2), `server.rs` (1), and the "daemon plan A/B" qualifiers kept.
  - fmt and both clippy variants.
- [ ] **Step 3: Commit** "Rewrite comments that cite plan-local numbering".

## Final verification (the controller, before the PR)

- **Tests:** every CI command on macOS outside the sandbox (passed/total); `cargo test` in the Linux Docker repro.
- **Untouched:** `git diff --stat refactor-r4f-settings-small-items..HEAD` lists no held file and none of §3.2's files.
- **Records:** the PR joins the spec's Status line; the final pre-merge commit marks this plan `Implemented`.

## Rulings (recorded under "Rulings" in the PR)

1. **`display_email` returns `&str`.** `words::email` stays a wrapper for the held `actions.rs` (§3.4); its callers through `render::email` are calls, not copies, and stay.
2. **The 14 copies** are the engine's nine, the binary's two and `tagteam-cc`'s three (its `identity_key` applies the same rule). `notify.rs` (hardening) and FakeAgent's handle fallback (not an email) stay.
3. **`read_lock_record` lives in `auto.rs`,** beside the engine record's writer that the daemon's mirrors (§11.1); each caller keeps its detail through `shape` and its extra fields through `fields`.
4. **`jittered` also serves the daemon's `collect_delay_s`,** a third copy of the normalisation. The two `JITTER_FRAC` constants stay: the spec names only the normalisation.
5. **`cached_store(create)`, and `store` `expect`s its `Some`.** Rejected: an opener closure, which only moves the impossible `None`.
6. **`TargetStep` adds `NoSwitch` to the review's three outcomes:** a cooldown or a changed live login reports its own decision. `perform` gives `Option<Decision>`; `run` moves the `Switch` decision.
7. **§14's rule:** a "Task N" or "controller ruling" goes where the sentence states the rule, and is rewritten where it carried it. A bare "plan A/B" becomes the rule, or "daemon plan A/B" where it qualifies a Decision. "Decision N", milestone names (M2–M5), commit hashes and review IDs (`R-T9-sidecars`) stay: they resolve against tracked records (§14).
8. **Skipped:** hardening's `refresh.rs` (6), `notify.rs` (2) and `server.rs` (1); `provider_job.rs`'s "(Task 11)", which R4b's Task 4 removes; `tests/` and the held files. `bootstrap.rs`'s stale clause already went in R1–R3, leaving a reflow.

## Task 3 amendment: `try_target` over T4's `perform` (TUI redesign T4, PR #40)

T4 merges before R4g and changes `auto.rs::perform`. Task 3 was planned and done against the code before T4. This amendment states how Task 3's extraction reads on top of T4. It runs when R4g is rebased onto the main that contains T4. Everything in Task 3 not named here stands.

**What T4 changes in the code Task 3 moves:**
- `SwitchRequest`'s `AutoPerform` gains `cause: cause(&t.snap, &self.cfg, &t.live.id)`.
- The non-fatal `Err(e)` arm of `self.engine.switch(req)` calls `self.switch_failed(&t.live.id, id, &e)?` after `self.error(..)` and before returning `TickOutcome::Error`.
- `no_switch` returns `Result<(TickOutcome, Decision), EngineError>`. `announce` can now fail, because it stores `all-exhausted`. Its callers return it directly, without wrapping it in `Ok`.
- `tick` sets `self.exhausted` from the decision in `run`'s `Ok((_, decision))`.

**The mapping on top of T4** (it replaces Step 2's mapping only where it differs):
- **The request** is built inside `try_target` with T4's `cause` line, moved with the rest of the body.
- **The non-fatal error arm.** `try_target` does `self.count(t)?`, then `self.error(..)`, then `self.switch_failed(&t.live.id, id, &e)?`, in that order, and then returns `Ok(TargetStep::Stop(TickOutcome::Error))`. An `Err` from `switch_failed` propagates out of `try_target`, `perform` and `run`, as it does from T4's `perform`.
- **`Cooldown` and `LiveChanged`.** `let (outcome, decision) = self.no_switch(..)?;` then `Ok(TargetStep::NoSwitch(outcome, decision))`. An `Err` from `no_switch` propagates. Each arm's other side effects stay where T4 has them: `count` and `cooldown_left` for `Cooldown`, and `set_unhealthy_ticks(.., 0)` for `LiveChanged`.
- **Step 12's no-viable-target.** `let (outcome, decision) = self.no_switch(..)?;` then `Ok((outcome, Some(decision)))`.
- **`run`** still returns `instead.unwrap_or(decided.decision)`. `tick`'s exhaustion flag is not edited: it reads the decision `run` returns, which is the same decision T4's code returns on every path.
- `TargetStep`'s variants and `perform`'s signature are as Task 3 defined them. `perform` takes no `decision` parameter.

**The net** adds T4's tests to Step 1's list. They must pass unchanged after the rebase:
- `auto_tick.rs`:
  - `an_automatic_switch_records_the_window_that_triggered_it`
  - `a_failover_from_an_unread_account_records_no_cause`
  - `a_failed_switch_is_recorded_once_per_target_and_message_for_30_minutes`
  - `every_account_exhausted_is_recorded_on_entry_only`
  - `an_exhaustion_after_a_switch_is_a_new_episode`
  - `an_exhaustion_after_a_failed_switch_is_a_new_episode`
  - `a_fresh_engine_records_an_exhaustion_the_store_already_has_once`
  - `a_dry_run_records_no_failed_switch_and_no_exhaustion`
- `switch.rs`: `a_manual_switch_records_no_detail`.

**Procedure:**
1. `git rebase --onto origin/main <old base> refactor-r4g-helpers-comments`.
2. Resolve Task 1's and Task 2's hunks by keeping both sides' intent: `display_email()` in place of `email_of` wherever T4's new code names an account, and T4's `existing_store()` calls kept, now backed by `cached_store`.
3. Redo Task 3's hunk as `try_target` over T4's `perform`, following the mapping above.
4. Task 4's comment rows apply to the text as it stands after T4. A T4 comment that cites plan-local numbering in a file this branch may edit (for example "T4-24", "T4-25") stays: it names the redesign's tracked finding IDs, which §14 keeps.
5. Verify: the net above, the engine and daemon focused suites, and both clippy variants. `git diff --color-moved=zebra --color-moved-ws=allow-indentation-change` against T4's `perform` shows only control-flow edges unmoved.

**Recorded in the PR's Rulings:** T4 landed first, and this amendment resolved the overlap. Review refactor §8.9's CC-E-16 is unchanged in intent.

Codex approved this amendment in its own plan round (`xr-1792662078-111d3415`) before the rebase ran.
