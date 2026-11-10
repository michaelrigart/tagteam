# `autoswitch.ceiling`: implementation plan

**Status:** Approved

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** A per-provider setting `autoswitch.ceiling` (default 100, today's behaviour) at or above
which auto treats an account as full: it leaves a full live account at once for a clear one, never
lands on a full account while one below the ceiling is viable, and keeps real limits (100%,
failover) as the only way onto full accounts with room and onto API keys.

**Architecture:** `tagteam-engine::settings` gains the key, the pair rule (`threshold < ceiling`)
and the forgiving read; `config.rs` refuses a write that breaks a provider's pair. The pure decision
in `tagteam-core::autoswitch` gains `AutoConfig::ceiling` and the full/clear/hard terms;
`rank::blocked_until` takes a level, so `all-exhausted` reports when the wanted move could land and
the loop sleeps toward when a hard move could. `Decision::NoSwitch` carries which `all-exhausted`
case it is. The daemon applies the pair rule to `--threshold`, the engine records the ceiling in a
switch's detail, and the words say when every account is above the threshold.

**Tech Stack:** Rust 1.99.1 (MSRV 1.89, edition 2024), `toml_edit`, proptest (the §11.5
simulation), `cargo test` with `--features tagteam/test-support`.

**Spec:** `docs/superpowers/specs/2026-11-11-tagteam-autoswitch-ceiling-design.md` (cited "S§n").
The core spec (`2026-09-26-tagteam-core-cli-design.md`, "core §n") stays the authority for
everything S does not change. Line numbers are as of `e731e070` and will drift.

## Global Constraints

- G1 (S§2): at ceiling 100 every decision is exactly today's. The existing tests keep their
  assertions; they change only where S changes behaviour at every ceiling, and each such change is
  named in its task: the `all-above` detail (Task 5, S§3.7) and the `ceiling` field in a switch's
  stored detail (Task 6, S§3.6). Any other existing assertion that must change is a G1 regression:
  stop and report it.
- Adding `AutoConfig::ceiling` and `Decision::NoSwitch::exhausted` makes every full struct literal
  in the workspace name them. Those literals get `ceiling: 100.0` and `exhausted: None`, nothing
  else, unless the task says otherwise.
- `autoswitch.ceiling`: float, default `100.0`, valid `50–100`, per provider, after
  `autoswitch.threshold` in `KEYS` (S§3.2).
- The margin is `RECOVERED_PTS` (3.0, `autoswitch.rs:298`); no new constant for it (S§3.1).
- No `--ceiling` flag, no change to manual `switch`, polling, notifications, the TUI's displays or
  the statusline (S§2 non-goals).
- Commit messages: one plain imperative sentence, no prefix, no attribution.
- Every commit passes `cargo fmt --all --check`, the three clippy runs in CONTRIBUTING.md with
  `-D warnings`, and the focused tests of what it changed. The controller runs the full suite once
  per batch and on the final commit:
  `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`.
- pty and unix-socket tests fail inside the sandbox (`openpty: Operation not permitted`, `EPERM`):
  run the full suite unsandboxed. `cargo fetch --locked` runs unsandboxed if a crate is new (none
  is expected). Don't run `mise trust`.
- Run notes go in `.superpowers/research/autoswitch-ceiling-run/` (untracked).

## Terms (S§3.1), as code

| Term | Code (in `autoswitch.rs`) |
|---|---|
| full | `fn full(headroom: f64, ceiling: f64) -> bool`: `headroom <= 100.0 - ceiling` |
| out | `at_limit(headroom)` (unchanged): `headroom <= 0.0` |
| clear | `fn clear(headroom: f64, ceiling: f64) -> bool`: `headroom > 100.0 - ceiling + RECOVERED_PTS` |
| hard | `Ranking::hard(&self) -> bool`: trigger `Failover`, or `AtLimit` with the active headroom out |
| soft | trigger `AtLimit` and not `hard()` |

Unknown headroom is never full (a `None` never reaches `full`). A window is full when
`pct >= ceiling` (`blocked_until`'s level test), which agrees with `full` at exactly the ceiling.

## Rulings (inside the spec; each goes under "Rulings" in the PR)

- **R1. A broken pair on read** sets the ceiling to `DEFAULT_CEILING` and its `Source` to
  `Default`, as any invalid value does, so `config get autoswitch.ceiling` shows `100` and
  `config list` says `default`. The warning names each key as it was read
  (`provider.<id>.autoswitch.ceiling` when the provider table gave it).
- **R2. The hard move's two tiers** (S§3.3) are exactly most-headroom order over the candidates
  with room: every full candidate has less headroom than every one that is not. The hard path keeps
  today's `must_move` ranking unchanged; a test pins the tier order.
- **R3. `all-above`** (S§3.7) is set only where core §11.2 step 8's exception applies: a move that
  is not hard (`proactive`, or a soft `at-limit`), not from an API key, with every account above the
  threshold. A hard move's nothing-ranked never gets it; its candidates are out or unknown, not
  "not far enough ahead".
- **R4. The `all-exhausted` case** (S§3.5) rides on `Decision::NoSwitch` as
  `exhausted: Option<Exhausted>`, `Some` only for `all-exhausted`: `AllOut { wake_at }` or
  `RoomLeft`. `earliest_reset` stays the reported time.
- **R5. The write refusal** runs only for writes of `autoswitch.threshold` and `autoswitch.ceiling`
  (no other key moves a pair), against every registered provider in registry order, in both
  passes of `edit` (before the lock and under it). It is `SettingsError::Invalid`
  (`invalid-input`). Its reason names the provider and both values after the write (Task 2).
- **R6. The `--threshold` conflict** is emitted as `config-warning` from `configured()` (start of
  the loop and of `--once`) and from `AutoJob::settings_changed` (each reload that still has it).

## Review Focus

1. **A fractional ceiling with a window exactly at it** (ceiling 97.3, a window at 97.3%): full by
   the headroom test and by the window test alike, so it is both skipped as a target and counted by
   the back-at level. Tasks 3 and 4 pin both sides.
2. **An unknown candidate beside full ones**: unknown is never full, so a soft move with one unknown
   and one full candidate is `no-qualifying-candidate`, never `all-exhausted`. Task 5.
3. **A hand-edited broken pair** never blocks the TUI's `P` (`config set daemon.autoswitch`) or any
   other write, and reading it runs ceiling 100 with one warning. Tasks 1 and 2.
4. **A ceiling in one provider's table and a global threshold**: read, refused and warned per
   provider, under both spellings (`provider.<id>.…`, `--provider`), and a global write refused for
   a registered provider whose own table breaks. Tasks 1 and 2.
5. **`--threshold` across a reload**: the warning recurs after each reload that still conflicts and
   stops once it does not; the ceiling in force follows. Task 6.

---

## Execution order

Each task's `Depends on:` line is binding. Every engine and daemon task compiles the core crate,
so only tasks whose tests load disjoint files share a batch:

| Batch | Tasks |
|---|---|
| 1 | 1 |
| 2 | 2 |
| 3 | 3 |
| 4 | 4 |
| 5 | 5 |
| 6 | 6, 8 |
| 7 | 7, 9 |

---

### Task 1: The `autoswitch.ceiling` setting and its read-side pair rule

**Depends on:** none

**Files:**
- Modify: `crates/tagteam-engine/src/settings.rs` (constants `:22-40`, `KeyId` `:190`, `KEYS`
  `:225`, `Settings` `:866`, `Default` `:919`, `value`/`apply` `:965-1032`, `read_file` `:1076`)
- Test: `crates/tagteam-engine/tests/settings.rs`, `crates/tagteam/tests/config_cli.rs`

**Interfaces:**
- Produces:
  - `pub const DEFAULT_CEILING: f64 = 100.0;`
  - `pub const CEILING_RANGE: RangeInclusive<f64> = 50.0..=100.0;`
  - `Settings::ceiling: f64` (doc: "`autoswitch.ceiling`, in [`CEILING_RANGE`], above
    `threshold`. The provider's own table first.")
  - `pub fn pair_holds(threshold: f64, ceiling: f64) -> bool` — `threshold < ceiling`; the one
    statement of S§3.2's rule, used by Tasks 2, 4 and 6.
  - `KeyId::Ceiling`, and its `KEYS` entry right after `Threshold`:
    `Key { id: KeyId::Ceiling, name: "autoswitch.ceiling", kind: KeyKind::Float(&CEILING_RANGE), per_provider: true }`.

- [ ] **Step 1: Write the failing tests** in `crates/tagteam-engine/tests/settings.rs`:
  - `the_defaults_are_the_specs_table`: add `assert_eq!(d.ceiling, 100.0);`.
  - `the_ceiling_reads_from_the_provider_table_first`: `"[autoswitch]\nceiling = 97\n[provider.claude-code.autoswitch]\nceiling = 95\n"` loads `95.0`; without the provider table, `97.0`;
    `ceiling = 49` and `ceiling = 100.5` load `100.0` with the warning
    `` "{path}: `autoswitch.ceiling` must be a number from 50 to 100 (ignored)" ``.
  - `a_threshold_at_or_above_the_ceiling_runs_with_a_ceiling_of_100`, table-driven, each case
    `(text, threshold, ceiling, warnings)`:
    - `"[autoswitch]\nthreshold = 95\nceiling = 95\n"` → `95.0, 100.0`,
      `` ["{path}: `autoswitch.ceiling` (95) must be above `autoswitch.threshold` (95); using a ceiling of 100"] ``;
    - `"[autoswitch]\nthreshold = 92\n[provider.claude-code.autoswitch]\nceiling = 90\n"` →
      `92.0, 100.0`, the warning naming `` `provider.claude-code.autoswitch.ceiling` (90) `` and
      `` `autoswitch.threshold` (92) ``;
    - `"[autoswitch]\nceiling = 85\n"` (default threshold) → `90.0, 100.0`, naming
      `` `autoswitch.ceiling` (85) `` and `` `autoswitch.threshold` (90) ``;
    - `"[autoswitch]\nthreshold = 90\nceiling = 95.5\n"` → `90.0, 95.5`, no warning;
    - the provider-table case loaded as another provider (`load_as(text, "fake-agent")`) →
      `92.0, 100.0` from the default ceiling, no warning.
  - `a_broken_pair_shows_the_default_ceiling_from_the_default` (R1): `inspect` on
    `"[autoswitch]\nceiling = 85\n"` gives the ceiling `KeyState` `value == Value::Float(100.0)`,
    `source == Source::Default`, and the pair warning in `warnings`.
  - In `crates/tagteam/tests/config_cli.rs` `config_list_json_is_the_specs_shape`: insert
    `row("autoswitch.ceiling", json!(100.0), json!(100.0), "default")` after the threshold row.

- [ ] **Step 2: Run them to verify they fail**

  Run: `cargo test -p tagteam-engine --test settings` and
  `cargo test -p tagteam --features tagteam/test-support --test config_cli`
  Expected: compile error (`no field ceiling`) or the new assertions failing.

- [ ] **Step 3: Implement the key, the field and the rule.** Add the constants, `KeyId::Ceiling`,
  the `KEYS` entry, the field (default `DEFAULT_CEILING`), and the `value`/`apply` arms. In
  `read_file`, after the `KEYS` loop: when `!pair_holds(threshold, ceiling)`, push the warning
  `` "{path}: `{ceiling key}` ({c}) must be above `{threshold key}` ({t}); using a ceiling of 100" ``
  (each key dotted as read: `provider.<id>.autoswitch.<leaf>` for `Source::Provider`, else
  `autoswitch.<leaf>`; values by `Value::Float(x).display()`), set the ceiling to
  `DEFAULT_CEILING` and its source to `Source::Default`. The unit tests in `settings.rs` that walk
  `KEYS` (`each_key_applies_the_value_it_shows`) need a value for the new key.

- [ ] **Step 4: Run the tests to verify they pass**

  Run: `cargo test -p tagteam-engine --lib settings`, `cargo test -p tagteam-engine --test settings`,
  `cargo test -p tagteam --features tagteam/test-support --test config_cli`
  Expected: PASS.

- [ ] **Step 5: Commit**

  `git commit -m "Add autoswitch.ceiling, which a provider runs at 100 when its threshold reaches it"`

---

### Task 2: `config set` and `unset` refuse a write that breaks a provider's pair

**Depends on:** Task 1

**Files:**
- Modify: `crates/tagteam-engine/src/settings.rs` (a `pub(crate)` reader of one doc's pair),
  `crates/tagteam-engine/src/config.rs` (`edit` `:165-205`)
- Test: `crates/tagteam-engine/tests/config.rs`

**Interfaces:**
- Consumes: `pair_holds`, `KeyId::{Threshold, Ceiling}` (Task 1).
- Produces:
  - `pub(crate) fn autoswitch_pair(doc: &DocumentMut, provider: &ProviderId) -> (f64, f64)` in
    `settings.rs`: the effective `(threshold, ceiling)` as `read_file` takes each value (provider
    table, then global, then default; an invalid value skipped), before the pair rule. Built on
    `Reader::read_key`; its warnings are dropped.
  - `fn check_pair(&self, entry: &Entry, before: Option<&DocumentMut>, after: &str) -> Result<(), SettingsError>`
    on `Engine` in `config.rs` (R5). No file before is all defaults, which holds.

- [ ] **Step 1: Write the failing tests** in `crates/tagteam-engine/tests/config.rs`. Each refusal
  asserts `kind() == "invalid-input"`, its full message, and the file byte for byte unchanged
  (or still absent).
  - `a_write_that_breaks_the_pair_is_refused_and_nothing_is_written`, on `Fx` (Claude Code only),
    file `"[autoswitch]\nthreshold = 90\n"`:
    - `set autoswitch.ceiling 90` → `` "`autoswitch.ceiling` must be above `autoswitch.threshold` (claude-code would have a threshold of 90 and a ceiling of 90)" ``;
    - `set autoswitch.ceiling 95` succeeds; then `set autoswitch.threshold 95` →
      `` "`autoswitch.threshold` must be below `autoswitch.ceiling` (claude-code would have a threshold of 95 and a ceiling of 95)" ``.
  - `both_spellings_of_a_provider_entry_are_refused_alike`: file `"[autoswitch]\nthreshold = 90\n"`;
    `provider.claude-code.autoswitch.ceiling 85` with no flag, and `autoswitch.ceiling 85` with
    `Some(&cc())`, both refuse with the key `provider.claude-code.autoswitch.ceiling` and
    `(claude-code would have a threshold of 90 and a ceiling of 85)`.
  - `an_unset_that_breaks_the_pair_is_refused`: file `"[autoswitch]\nthreshold = 80\nceiling = 85\n"`;
    `unset autoswitch.threshold` refuses (`… a threshold of 90 and a ceiling of 85)`);
    `unset autoswitch.ceiling` succeeds. File
    `"[autoswitch]\nthreshold = 92\n[provider.claude-code.autoswitch]\nthreshold = 80\nceiling = 85\n"`:
    `unset provider.claude-code.autoswitch.threshold` refuses (`… a threshold of 92 and a ceiling of 85)`).
  - `a_global_write_is_checked_for_every_registered_provider`, on `FakeFx`: file
    `"[provider.fake-agent.autoswitch]\nthreshold = 96\n"`; `set autoswitch.ceiling 95` refuses
    with `(fake-agent would have a threshold of 96 and a ceiling of 95)`.
  - `a_pair_the_file_already_breaks_blocks_no_write` (Review Focus 3): file
    `"[autoswitch]\nthreshold = 95\nceiling = 90\n"`; `set daemon.autoswitch false`,
    `set autoswitch.cooldown_seconds 600` and `set autoswitch.ceiling 92` (still broken) each
    succeed with `changed: true`; `set autoswitch.ceiling 96` succeeds.

- [ ] **Step 2: Run them to verify they fail**

  Run: `cargo test -p tagteam-engine --test config`
  Expected: the refusals come back `Ok`.

- [ ] **Step 3: Implement `autoswitch_pair` and `check_pair`, and call `check_pair` in both passes of
  `edit`.** `check_pair` returns `Ok` unless `entry.key.id` is `Threshold` or `Ceiling`; for each
  `self.registry.all()` provider, in order, it refuses the first whose pair held in `before` and
  breaks in `after` (parsed with `DocumentMut`). Reason, `t` and `c` the values after the write,
  printed by `Value::Float(x).display()`:
  - ceiling written: `` "must be above `autoswitch.threshold` ({id} would have a threshold of {t} and a ceiling of {c})" ``;
  - threshold written: `` "must be below `autoswitch.ceiling` ({id} would have a threshold of {t} and a ceiling of {c})" ``.

  `edit` keeps a clone of the document it read before handing it to `apply`, in each pass.

- [ ] **Step 4: Run the tests to verify they pass**

  Run: `cargo test -p tagteam-engine --test config` and `cargo test -p tagteam-engine --lib config`
  Expected: PASS.

- [ ] **Step 5: Commit**

  `git commit -m "Refuse a config write that leaves a provider's ceiling at or below its threshold"`

---

### Task 3: `rank::blocked_until` takes the level it judges at

**Depends on:** none (runs after Task 2 in batch order)

**Files:**
- Modify: `crates/tagteam-core/src/rank.rs:95-104`, `crates/tagteam-core/src/autoswitch.rs:341`
  (`Rated::of`), `crates/tagteam-engine/src/switch/select.rs:572`
- Test: `crates/tagteam-core/src/rank.rs` (tests module)

**Interfaces:**
- Produces: `pub fn blocked_until(windows: &[Window], models: &[String], level: f64) -> Option<i64>`
  — the latest reset among relevant windows with `pct >= level`. `Rated::of` and `select.rs` pass
  `100.0` (S§3.4: manual `switch`'s text does not change).

- [ ] **Step 1: Write the failing test** `a_level_below_100_waits_for_every_window_at_or_above_it`:
  windows `5h` 100% resetting at `1_000` and `7d` 97% at `9_000`: level `100.0` → `Some(1_000)`;
  `98.0` → `Some(1_000)`; `95.0` → `Some(9_000)`; `7d` at exactly `97.3` with level `97.3` →
  `Some(9_000)` (Review Focus 1). The existing
  `an_account_is_blocked_until_every_relevant_window_at_its_limit_has_reset` passes `100.0` and
  keeps its assertions.

- [ ] **Step 2: Run it to verify it fails**

  Run: `cargo test -p tagteam-core --lib rank`
  Expected: compile error (three arguments).

- [ ] **Step 3: Add the parameter and pass `100.0` at both callers.**

- [ ] **Step 4: Run the tests to verify they pass**

  Run: `cargo test -p tagteam-core --lib` and `cargo test -p tagteam-engine --test switch`
  Expected: PASS.

- [ ] **Step 5: Commit**

  `git commit -m "Let blocked_until judge an account back at a usage level below 100"`

---

### Task 4: A full live account moves; no move lands on a full account

**Depends on:** Task 1, Task 3

**Files:**
- Modify: `crates/tagteam-core/src/autoswitch.rs` (`AutoConfig` `:65`, `triggered` `:541`,
  `Ranking` `:564`, `rank` `:587`, `gated` `:609`, `recovery_axis_useful` `:408`, `decide` `:752`),
  `crates/tagteam-daemon/src/auto.rs:40` (`auto_config`), and every full `AutoConfig` literal the
  compiler names (`tagteam-core` tests and `tests/simulate.rs`, `tagteam-engine/tests/*`,
  `tagteam-daemon` tests).
- Test: `crates/tagteam-core/src/autoswitch.rs` (tests module)

**Interfaces:**
- Consumes: `Settings::ceiling`, `pair_holds`, `DEFAULT_CEILING` (Task 1).
- Produces:
  - `AutoConfig::ceiling: f64` (doc: "`autoswitch.ceiling` (ceiling spec §3.2): usage at or above
    it is full. 100 decides as before the ceiling existed."), after `threshold`.
  - `fn full`, `fn clear`, `Ranking::hard` (Terms table). `recovery_axis_useful` gains
    `ceiling: f64`.
  - `auto_config` sets `ceiling: if pair_holds(threshold, s.ceiling) { s.ceiling } else { DEFAULT_CEILING }`,
    with `threshold` the clamped flag or the setting. Task 6 adds the warning.

- [ ] **Step 1: Write the failing tests.** Threshold 90, ceiling 95 unless stated; a helper
  `fn ceiling(c: f64) -> AutoConfig { AutoConfig { ceiling: c, ..cfg() } }`, and `cfg()` gains
  `ceiling: 100.0`. Accounts by the existing `at(position, pct)` (7d at `pct`) unless stated. Each
  test also runs its snapshot at ceiling 100 and asserts today's decision, which passes before the
  change. A "no switch" whose reason Task 5 decides is asserted as `Decision::NoSwitch { .. }`.
  - `a_full_live_account_moves_at_once_past_the_cooldown_and_the_bar`: live 2 at 95, candidate 1
    at 80; `AutoState { last_switch_at: Some(NOW - 10), last_switch_from: Some(id(1)),
    last_switch_to: Some(id(2)), left_headroom: Some(50.0), left_trigger: Some(Trigger::Proactive), .. }`.
    Ceiling 95 → `(AtLimit, [1])`. Ceiling 100 → `Cooldown`.
  - `a_soft_move_lands_only_more_than_three_points_below_the_ceiling`: live 2 at 95; candidate 1 at
    93 → `(NoQualifyingCandidate, Blocked)`; at 92.0 (exactly ceiling − margin) → the same; at 91.9
    → `(AtLimit, [1])`; live at 100 with the candidate at 93 → `(AtLimit, [1])`. Ceiling 100: 93
    and 91.9 → `NoQualifyingCandidate`; live 100 → `(AtLimit, [1])`.
  - `a_hard_move_takes_room_wherever_it_is` (G4, R2): live 5 at 100, candidate 1 at 96 →
    `(AtLimit, [1])` at both ceilings. Live 5 at 100, candidates 1 at 97, 2 at 93, 3 at 99, 4 at 85
    → `(AtLimit, [4, 2, 1, 3])` at both ceilings.
  - `the_ceiling_never_moves_auto_onto_an_api_key`: `with_keys()` at ceiling 95, live 2 at 95,
    candidate 1 at 97, `api_key(3)` → no switch. Live 2 at 100, same others → `(AtLimit, [1, 3])`.
    Ceiling 100: live 95 → `NoQualifyingCandidate`; live 100 → `(AtLimit, [1, 3])`.
  - `proactive_never_lands_on_a_full_account` (G3): live 2 at 92, candidate `oauth(1, 96.0, 10.0)`
    (5h binding at 96, resets in an hour). Ceiling 100 → `(Proactive, [1])` by the recovery axis;
    ceiling 95 → no switch.
  - `spent_is_measured_from_the_ceiling` (S§3.3): live 2 at 93; candidate
    `reset(at(1, 92.5), "7d", Some(NOW + 20_000))`. Ceiling 95 → `(Proactive, [1])` (both within 3
    points of the ceiling, so the recovery axis, and 1 recovers sooner); ceiling 100 →
    `NoQualifyingCandidate` (headroom axis: 7.5 < 2 × 7).
  - `a_dip_below_the_ceiling_never_draws_a_soft_move_back` (S§3.8): two accounts, live 2 at 95,
    candidate 1 at 94.9 → `NoQualifyingCandidate`. Three: live 1 (A) at 95, 2 (B) at 94.9, 3 (C)
    at 91 → `(AtLimit, [3])`; then with the state that switch leaves (`last_switch_from` 1,
    `last_switch_to` 3, `left_trigger` `AtLimit`, `left_headroom` `Some(5.0)`, `last_switch_at`
    `NOW`), live 3 at 95, A and B at 94.9 → `NoQualifyingCandidate`. Ceiling 100: the first tick →
    `NoQualifyingCandidate`, the second → `Cooldown`.
  - `full_agrees_with_the_window_test_at_a_fractional_ceiling` (Review Focus 1): ceiling 97.3,
    candidate 1 at 50; live 2 at exactly 97.3 → `(AtLimit, [1])`; live 2 at 97.2 → `(Proactive,
    [1])`.

  Where a test asserts `NoQualifyingCandidate`, it asserts the reason and outcome only: Task 5
  sets the detail.

- [ ] **Step 2: Run them to verify they fail**

  Run: `cargo test -p tagteam-core --lib autoswitch`
  Expected: compile error (`no field ceiling`), then the ceiling-95 assertions failing.

- [ ] **Step 3: Implement S§3.3 steps 5, 8 and 10.**
  - `triggered`: `Some(h) if full(h, cfg.ceiling) => go(Trigger::AtLimit, 0)`, still first.
  - `rank`: when `must_move()` and `hard()`, today's path (known headroom with room, most headroom,
    R2). When soft: the candidates with known `clear` headroom, `most_headroom`.
  - `gated`: skip `full(h, ceiling)` where it skipped `at_limit(h)`; pass `cfg.ceiling` to
    `recovery_axis_useful`, whose `spent` is `h - (100.0 - ceiling) <= SPENT_HEADROOM_PCT`.
  - `decide`: append the API keys only when `ranking.hard()` (today: every `must_move`).
  - `auto_config` as in Interfaces; the full literals get `ceiling: 100.0`.

- [ ] **Step 4: Run the tests to verify they pass**

  Run: `cargo test -p tagteam-core`, `cargo test -p tagteam-daemon --lib auto`,
  `cargo test -p tagteam-engine --test auto_tick --test auto_engine`
  Expected: PASS, every existing test unchanged.

- [ ] **Step 5: Commit**

  `git commit -m "Move auto off an account at its ceiling and never onto a full one"`

---

### Task 5: Nothing ranked: `all-exhausted` at the ceiling, its times, its sleep, and `all-above`

**Depends on:** Task 3, Task 4

**Files:**
- Modify: `crates/tagteam-core/src/autoswitch.rs` (`Decision` `:247`, `Rated` `:310-344`,
  `every_candidate_exhausted` `:383`, `gated` `:614`, `nothing_ranked` `:671`, `next_delay` `:810`),
  and every full `Decision::NoSwitch` literal the compiler names (`tagteam-engine/src/auto.rs`
  `:458`, `:1055`; `tagteam-daemon/src/auto.rs:316`; `tagteam-engine/tests/auto_tick.rs`).
- Test: `crates/tagteam-core/src/autoswitch.rs` (tests module)

**Interfaces:**
- Consumes: `full`, `Ranking::hard`, `AutoConfig::ceiling` (Task 4); `blocked_until(.., level)`
  (Task 3).
- Produces:
  - `pub enum Exhausted { AllOut { wake_at: Option<i64> }, RoomLeft }` (`Debug, Clone, Copy,
    PartialEq, Eq`): `AllOut` when every OAuth candidate is out, `wake_at` S§3.4's wake time;
    `RoomLeft` when one is full but has room.
  - `Decision::NoSwitch::exhausted: Option<Exhausted>`, after `earliest_reset`; `Some` only for
    `all-exhausted` (R4). `earliest_reset` is S§3.4's reported time.
  - `pub const ALL_ABOVE: &str = "all-above";` — `no-qualifying-candidate`'s detail (R3).
  - `Rated::back_at(&self, models: &[String], level: f64) -> Option<i64>` replaces the `back_at`
    field.
  - `fn all_above(r: &Ranking, oauth: &[Rated]) -> bool` — `!api_key && every_account_above(..)`,
    shared by `gated` and `nothing_ranked`.
  - `nothing_ranked(r: &Ranking, t: &Triggered, oauth: &[Rated]) -> Decided`.

- [ ] **Step 1: Write the failing tests.** Threshold 90, ceiling 95 unless stated.
  - `a_soft_move_with_every_candidate_full_is_all_exhausted_at_the_normal_cadence`: live 3 at 96;
    candidate 1 `oauth(1, 97.0, 93.0)` (5h 97 at `NOW + 3_600`, 7d 93 at `NOW + 86_400`),
    candidate 2 at 99. Decision `NoSwitch { reason: AllExhausted, outcome: Blocked, detail: "1d00h",
    earliest_reset: Some(NOW + 86_400), exhausted: Some(Exhausted::RoomLeft) }` (level 92), and
    `next_delay(&d, &cfg, NOW, None, 0.0) == 60`. With live 3 at 92 (proactive, level 95):
    `earliest_reset == Some(NOW + 3_600)`, `RoomLeft`.
  - `every_candidate_out_sleeps_toward_the_first_room_whatever_it_reports`: candidate 1 with 5h at
    100% resetting at `NOW + 120` and 7d at 97% at `NOW + 259_200`. Live 2 at 100 →
    `earliest_reset: Some(NOW + 120)`; live 2 at 96 → `Some(NOW + 259_200)`. Both
    `exhausted: Some(AllOut { wake_at: Some(NOW + 120) })` and `next_delay(.., 0.0) == 180`.
  - `every_candidate_out_with_no_known_reset_waits_as_any_blocked_tick`: candidate 1 at 100 with no
    `resets_at` → `AllOut { wake_at: None }`, `earliest_reset: None`, `next_delay == 300`.
  - `full_oauth_candidates_and_a_key_never_draw_a_soft_move`: `with_keys()`, live 2 at 95,
    candidate 1 at 97, `api_key(3)` → `AllExhausted`, `RoomLeft`.
  - `an_unknown_candidate_is_never_full` (Review Focus 2): live 3 at 96, candidate 1 at 97,
    `unknown(2)` → `(NoQualifyingCandidate, Blocked, ALL_ABOVE)` (an unknown candidate does not
    count against "every account above", as today).
  - `no_qualifying_candidate_says_when_every_account_is_above_the_threshold` (S§3.7, R3):
    live 2 at 95, candidate 1 at 93 → `(NoQualifyingCandidate, Blocked, ALL_ABOVE)` at ceiling 95
    and at ceiling 100; live 2 at 92, candidate 1 at 85 → `""`; a quarantined live 2 at 95,
    candidates 1 at 100 and `unknown(3)` (a hard move) → `""`.
  - The existing tests that assert `(NoQualifyingCandidate, Blocked, "")` where every account is
    above the threshold change that detail to `ALL_ABOVE`, and no other part of their assertions.
    The existing `next_delay` tests and `every_candidate_exhausted…` tests construct
    `exhausted: Some(Exhausted::AllOut { wake_at })` where they had a known `earliest_reset`, and
    keep their expected delays.

- [ ] **Step 2: Run them to verify they fail**

  Run: `cargo test -p tagteam-core --lib autoswitch`
  Expected: compile error (`Exhausted`), then the new assertions failing.

- [ ] **Step 3: Implement S§3.3 step 9, S§3.4, S§3.5 and S§3.7.**
  - `every_candidate_exhausted` becomes `every_candidate_full(oauth, ceiling)` (non-empty, every
    one known and `full`); `every_candidate_out(oauth)` (every one known and `at_limit`) chooses
    `AllOut` over `RoomLeft`.
  - The reported level: `100.0` when `hard()`, `ceiling - RECOVERED_PTS` on a soft `AtLimit`, and
    `ceiling` otherwise (`Proactive`). `earliest_reset` is the minimum `back_at` at it; `detail` is
    `span` of it as today. `wake_at` is the minimum `back_at` at `100.0`.
  - `nothing_ranked`'s `no-qualifying-candidate` detail is `ALL_ABOVE` when `!r.hard()` and
    `all_above(r, oauth)`, else empty.
  - `next_delay`: the sleep toward a reset keys on `exhausted: Some(AllOut { wake_at: Some(at) })`
    with `at` in place of `earliest_reset`; `Some(RoomLeft)` keeps the normal cadence, as
    `no-qualifying-candidate` does; `AllOut { wake_at: None }` stays `max(interval, 300)`. Its doc
    comment says so.
  - The full `Decision::NoSwitch` literals outside core get `exhausted: None`;
    `auto_tick.rs`'s `every_candidate_exhausted_is_blocked_until_the_earliest_recovery` gets
    `exhausted: Some(Exhausted::AllOut { wake_at: Some(SHORT_RESETS) })`.

- [ ] **Step 4: Run the tests to verify they pass**

  Run: `cargo test -p tagteam-core`, `cargo test -p tagteam-engine --test auto_tick --test auto_engine --test invariant`,
  `cargo test -p tagteam-daemon --lib`
  Expected: PASS. The unix-socket tests among the daemon's may need the run unsandboxed.

- [ ] **Step 5: Commit**

  `git commit -m "Block on full accounts at the ceiling, report when the wanted move can land, and say when every account is above the threshold"`

---

### Task 6: The daemon's `--threshold` conflict and the switch record's ceiling

**Depends on:** Task 4, Task 5

**Files:**
- Modify: `crates/tagteam-daemon/src/auto.rs` (`configured` `:275`, `run_loop` `:155`, `once`
  `:248`, `AutoJob::settings_changed` `:234`), `crates/tagteam-engine/src/switch/mod.rs:94-108`,
  `crates/tagteam-engine/src/auto.rs:1216-1225` (`cause`)
- Test: `crates/tagteam-daemon/src/auto.rs` (tests module),
  `crates/tagteam-engine/tests/auto_tick.rs`, and the tests that assert a switch's stored detail.

**Interfaces:**
- Consumes: `AutoConfig::ceiling`, `auto_config` (Task 4); `Exhausted` (Task 5).
- Produces:
  - `pub fn threshold_flag_warning(s: &Settings, cfg: &AutoConfig) -> Option<String>` in
    `tagteam-daemon/src/auto.rs`: `Some` when `cfg.ceiling != s.ceiling` (only a flag can cause
    it), text `` "--threshold {t} is at or above `autoswitch.ceiling` ({c}); using a ceiling of 100" ``,
    `t` the clamped flag, `c` the setting, each by `Value::Float(x).display()`.
  - `configured(engine, provider, flags, sink: &dyn EventSink)` emits that warning as
    `AutoEvent::ConfigWarning { provider, message }`.
  - `SwitchCause::ceiling: f64`; `detail()` is `{"window","label","pct","threshold","ceiling"}`.

- [ ] **Step 1: Write the failing tests.**
  - Daemon, `a_threshold_flag_at_or_above_the_ceiling_runs_with_a_ceiling_of_100`: settings with
    `ceiling: 95.0`: flag `95.0` → `cfg.ceiling == 100.0` and the warning naming `95` and `95`;
    flag `94.5` → `95.0`, `None`; no flag → `95.0`, `None`; flag `120.0` → threshold `99.9`,
    ceiling `100.0`, warning `--threshold 99.9 …`.
  - Daemon, `the_threshold_flag_warning_comes_at_start_and_after_each_reload_that_keeps_it` (Review
    Focus 5), on the Slices pattern of
    `changed_settings_are_read_again_before_the_next_tick_and_flags_still_win`: file
    `"[autoswitch]\nceiling = 95\n"`, flag `--threshold 96`; the file becomes `ceiling = 96` during
    the first sleep and `ceiling = 98` during the second. `ConfigWarning` events: one at start naming
    `(95)`, one after the first reload naming `(96)`, none after the second. `--once` with the first
    file emits exactly one.
  - Engine, `a_ceiling_below_100_moves_a_full_account_and_records_the_ceiling`: `cfg` with
    `ceiling: 95.0`; b live at `reading(false, 20.0, 95.0)`, a at `reading(false, 10.0, 50.0)` → a
    switch b → a with trigger `at-limit`, and the stored switch's detail
    `{"window":"7d","label":"7d","pct":95.0,"threshold":90.0,"ceiling":95.0}`.
  - Engine, `a_ceiling_below_100_blocks_on_full_accounts_at_the_normal_cadence`: `cfg` with
    `ceiling: 95.0`; b live at `reading(false, 96.0, 40.0)`, a at `reading(false, 97.0, 40.0)` →
    `TickOutcome::Blocked`; decision `NoSwitch { AllExhausted, Blocked, "2h40m",
    Some(SHORT_RESETS), Some(Exhausted::RoomLeft) }`; events after the poll `[no_switch(…,
    "all-exhausted", "2h40m"), AllExhausted { earliest_reset_at: Some(SHORT_RESETS) }]`; one stored
    `all-exhausted` with detail `{"earliest_reset_at": SHORT_RESETS}`; and
    `next_delay(&decision, &cfg, T0, None, 0.0) == 60`.
  - Existing assertions of a stored switch detail (`an_automatic_switch_records_the_window_that_triggered_it`,
    and any the compiler or the suite names) gain `"ceiling": 100.0` (S§3.6), nothing else.

- [ ] **Step 2: Run them to verify they fail**

  Run: `cargo test -p tagteam-daemon --lib auto` and `cargo test -p tagteam-engine --test auto_tick`
  Expected: compile errors, then the new assertions failing.

- [ ] **Step 3: Implement** `threshold_flag_warning`, the emission in `configured` and in
  `settings_changed` (after the read's own warnings), the `SwitchCause` field and `cause`'s
  `ceiling: cfg.ceiling`.

- [ ] **Step 4: Run the tests to verify they pass**

  Run: `cargo test -p tagteam-daemon`, `cargo test -p tagteam-engine --test auto_tick`,
  `cargo test -p tagteam-tui`, `cargo test -p tagteam --features tagteam/test-support --lib`
  Expected: PASS (unsandboxed for the daemon's socket tests).

- [ ] **Step 5: Commit**

  `git commit -m "Warn when --threshold reaches a provider's ceiling, and record the ceiling with each automatic switch"`

---

### Task 7: The words for `no-qualifying-candidate` when every account is above the threshold

**Depends on:** Task 5

**Files:**
- Modify: `crates/tagteam-engine/src/words.rs:578`
- Test: `crates/tagteam-engine/src/words.rs` (tests), `crates/tagteam/src/auto.rs` (tests)

**Interfaces:**
- Consumes: `ALL_ABOVE` (Task 5).
- Produces: `no_switch(NoSwitchReason::NoQualifyingCandidate, facts)` is
  `format!("every account is above {t}, and none is far enough ahead to switch to")` when
  `facts.detail == ALL_ABOVE` (`t` by `words::threshold`), else unchanged.

- [ ] **Step 1: Write the failing tests.** In `words.rs`,
  `no_qualifying_candidate_names_the_threshold_when_every_account_is_above_it`: detail `ALL_ABOVE`,
  threshold `90.0` → `"every account is above 90%, and none is far enough ahead to switch to"`;
  threshold `92.5` → `"every account is above 92.5%, …"`; detail `""` →
  `"no other account has enough headroom"`. In `crates/tagteam/src/auto.rs`, beside
  `a_below_threshold_line_says_the_threshold_of_its_poll`: a poll at threshold 85 then
  `no_switch_event("no-qualifying-candidate", "all-above")` prints a line ending
  `every account is above 85%, and none is far enough ahead to switch to`.

- [ ] **Step 2: Run them to verify they fail**

  Run: `cargo test -p tagteam-engine --lib words` and
  `cargo test -p tagteam --features tagteam/test-support --lib auto`
  Expected: the old sentence.

- [ ] **Step 3: Implement the arm.**

- [ ] **Step 4: Run the tests to verify they pass**, plus `cargo test -p tagteam-tui --lib autoline`.
  Expected: PASS.

- [ ] **Step 5: Commit**

  `git commit -m "Say when every account is above the threshold and none is far enough ahead"`

---

### Task 8: The simulation draws a ceiling and checks S§4's properties

**Depends on:** Task 4, Task 5

**Files:**
- Modify: `crates/tagteam-core/tests/simulate.rs` (`settings` `:142`, `back_at` `:451`, `check`
  `:493`, `check_delay` `:627`)

**Interfaces:**
- Consumes: `AutoConfig::ceiling`, `Decision::NoSwitch::exhausted`, `Exhausted` (Tasks 4, 5).
  The simulation restates `full`, `clear` and hard from S§3.1 itself; it never calls core's private
  helpers.

- [ ] **Step 1: Change the generator and the properties** (they are the test):
  - `settings()` draws `ceiling`: one time in three exactly `100.0`, otherwise
    `100.0 - u * (100.0 - threshold)` with `u` in `0.0..0.999`, so it lies in
    `(threshold, 100]`.
  - `back_at(a, level)` takes the level (windows with `pct >= level`).
  - `check` asserts, with `hard` = `Failover`, or `AtLimit` with the live headroom ≤ 0:
    1. a landing on a full OAuth account only when `hard` and no OAuth candidate has known headroom
       that is not full;
    2. never a landing on headroom ≤ 0 (kept);
    3. a live OAuth account that is full, not quarantined, with a clear OAuth candidate → a
       `Switch`;
    4. an `AtLimit` switch that is not hard lands on a clear OAuth account;
    5. never idling out while a viable candidate exists (kept);
    6. a landing on an API key only when `hard` and no OAuth candidate has room (replaces "only for
       at-limit or failover");
    7. `all-exhausted` only when every OAuth candidate is known to be full; `earliest_reset` is the
       minimum `back_at` at the level of the tick's move; `exhausted` is
       `AllOut { wake_at: min back_at at 100 }` when every one is out, else `RoomLeft`. The level
       comes from the live account as core §11.2 step 5 reads it: quarantined, or usage unknown
       (an `all-exhausted` then means failover) → 100; an API key → `ceiling`; out → 100; full →
       `ceiling − 3`; otherwise → `ceiling`.
  - `check_delay`: `AllOut { wake_at: Some(_) }` → `interval.min(600)..=600`;
    `AllOut { wake_at: None }` and the other BLOCKED reasons except `no-qualifying-candidate` →
    `interval.max(300)`; `RoomLeft` → the normal-cadence bound.
  - The hard-coded `h <= 0.0` checks at `:526`, `:557`, `:617` become these.

- [ ] **Step 2: Run it, then show the new properties bite.** The decision these properties judge
  is already built, so the failing run is a deliberate fault: in a scratch edit of
  `autoswitch.rs`, make a soft move rank every candidate with room (as a hard one does), and see
  property 1 or 4 fail; restore it and see the run pass. Record both outcomes in the run notes.

  Run: `cargo test -p tagteam-core --test simulate`
  Expected: FAIL with the fault, PASS without it.

- [ ] **Step 3: Commit**

  `git commit -m "Simulate auto with a ceiling and check that it never lands on a full account it could avoid"`

---

### Task 9: The records

**Depends on:** Task 1–Task 8 for their wording (fixed by this plan); edits documents only.

**Files:**
- Modify: `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md` (§6.4 `:774`, §11.2
  steps 5, 8, 9, 10 `:1859-1903`, §11.4 loop-delay table `:1957`, §11.5 `:2008`),
  `docs/superpowers/specs/2026-10-20-tagteam-tui-redesign-design.md:924`, `README.md:173`
  ("Automatic switching"), `CHANGELOG.md` (`[Unreleased]`)

- [ ] **Step 1: Amend the core spec.** Each passage ends "*(Amended by the autoswitch-ceiling
  spec.)*", linking `2026-11-11-tagteam-autoswitch-ceiling-design.md`:
  - §6.4: the row `` | `autoswitch.ceiling` | 100.0 | 50–100, above the threshold | yes | `` after
    the threshold row, and a paragraph on the pair rule (S§3.2): a read with a broken pair runs
    ceiling 100 with one warning; `set`/`unset` refuse a write that breaks a pair that held, in
    either spelling; a pair already broken blocks no write; `--threshold` at or above a provider's
    ceiling runs it at 100 with a `config-warning` at start and after each reload that keeps it.
  - §11.2 step 5: usage at or above `autoswitch.ceiling` → `at-limit`. Step 8: skip full
    candidates; "spent" measured from the ceiling; a soft move ranks only clear candidates; a hard
    move ranks every candidate with room, most headroom first. Steps 9 and 10: API keys only on a
    hard move; `all-exhausted` when every candidate is known to be full, with S§3.4's reported
    time. Define full, out, clear, hard and soft once, as S§3.1 does.
  - §11.4's loop-delay table: `all-exhausted` with every OAuth candidate out and a known reset
    sleeps toward the wake time; with a candidate that still has room it keeps the normal cadence.
  - §11.5: S§4's simulation properties in place of the two `headroom ≤ 0` ones and the API-key one.
- [ ] **Step 2: Amend the redesign spec §11.3's `no-qualifying-candidate` row**: with detail
  `all-above`, `every account is above 90%, and none is far enough ahead to switch to`; otherwise as
  it was. Mark it amended.
- [ ] **Step 3: README "Automatic switching"**: a paragraph on `autoswitch.ceiling` (for example
  `tagteam config set autoswitch.ceiling 95`): at or above it auto leaves the account at once for
  one more than 3 points below it, and never moves onto a full account or an API key because of
  it. tagteam picks the account but cannot stop usage that reaches it from elsewhere (the web app,
  other machines), so a ceiling below 100 is a margin, not a guarantee.
- [ ] **Step 4: CHANGELOG `[Unreleased]`**: *Added* `autoswitch.ceiling`, with what it does in one
  sentence and the pair rule; *Changed* auto's `no switch` line when every account is above the
  threshold, the `no-switch` event's `detail` `all-above` for `--json` consumers, and the
  `ceiling` field in a stored switch's detail.
- [ ] **Step 5: Commit**

  `git commit -m "Record autoswitch.ceiling in the core and redesign specs, the README and the changelog"`

---

## Finishing (the controller, after the last batch)

- Full suite (unsandboxed), `cargo fmt --all --check`, the three clippy runs, and
  `cargo-deny check advisories` (unsandboxed, path in the brief) on the final commit; report
  passed/total.
- Cold diff review (`sp-reviewer`) and Codex's pre-merge review together; fix in-scope findings on
  the branch, re-review.
- The spec's Status: `In progress` plus the PR while under review; `Implemented` plus the PR in the
  final pre-merge commit. This plan's Status follows.
