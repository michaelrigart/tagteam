# tagteam — `autoswitch.ceiling`: a usage level auto treats as full

**Status:** In progress
**Date:** 2026-11-11
**Issue:** https://github.com/michaelrigart/tagteam/issues/56
**Scope:** The auto-switch decision (`crates/tagteam-core/src/autoswitch.rs`, `rank.rs`), one new
setting (`crates/tagteam-engine/src/settings.rs`, `config.rs`), the `no-qualifying-candidate`
sentence, and the records. Manual `switch`, polling, notifications, the TUI's displays and the
statusline do not change.
**Builds on:** the core spec,
[`2026-09-26-tagteam-core-cli-design.md`](2026-09-26-tagteam-core-cli-design.md), cited as
"core §n". This spec amends core §6.4, §11.2 steps 5, 8, 9 and 10, §11.4's loop-delay table and
§11.5. The PR edits those passages and marks each one amended.

Line numbers are as of `688f8492` and will drift.

---

## 1. Purpose

### 1.1 The problem

When every account is above `autoswitch.threshold`, auto has no hard stop below 100%.

- The only forced move is `at-limit`, at headroom ≤ 0 (`autoswitch.rs:352`). It bypasses the
  cooldown and every anti-flap gate (core §11.2 steps 6 and 8).
- Below that, with every account above the threshold, a candidate needs at least twice the live
  account's headroom. When both are within 3 points of none, or either resets within 4 h, it
  needs a binding-window recovery at least 300 s sooner instead (core §11.2 step 8).
- The daemon therefore stays on an account at 97–99% and logs only "no other account has enough
  headroom".

Usage reaches an account from places tagteam cannot see: the web app, other machines, other
tools. Tagteam reads it once per poll, so a reading of 97% can already be 100%.
`autoswitch.threshold` cannot help: it only marks where "getting full" starts, and its maximum is
99.9 (`THRESHOLD_RANGE`, `settings.rs:34`).

### 1.2 The change

A per-provider setting `autoswitch.ceiling`, default 100, which is today's behaviour. An account
at or above the ceiling is *full*.

- Auto moves off a full live account at once, with no cooldown and no ratio, onto an account
  more than 3 points below the ceiling (§3.8).
- Auto never lands on a full account while a candidate below the ceiling exists.
- Only a real limit (100%, or a failover) still opens the full accounts that have room left, and
  the API keys.

### 1.3 Why this shape

This is issue #56's design (by srgvg), with three changes Michael approved on 2026-11-08:

1. **Real headroom is never stranded.** A live account at 100% still moves to an account with
   room, even one above the ceiling. Without this, a ceiling of 95 would leave a 100% account in
   place while another holds 4 points. That is worse than today, where it moves.
2. **The ceiling never moves auto onto an API key.** Keys stay the last resort at a real limit or
   on failover, as today. Otherwise a ceiling of 95 would start metered use at 95%. The way back
   then needs an account below the threshold (core §11.2 step 4), which can take days.
3. **An invalid pair never runs.** A threshold at or above the ceiling falls back to a ceiling
   of 100 with a warning, and `config` refuses to write one.

The issue rejected the alternatives:

- Raising the threshold to 99.9 forces no move.
- Making the 2× ratio a setting gives no hard stop, and adds a key that is hard to explain.
- Polling faster near the limit cannot see usage between polls.

## 2. Goals

- **G1.** At ceiling 100, every decision is exactly today's.
- **G2.** With a lower ceiling, a full live account moves to the candidate more than 3 points
  below the ceiling with the most headroom, whatever the cooldown and the ratio say.
- **G3.** No auto move lands on a full account while a candidate below the ceiling is viable.
- **G4.** A live account at 100%, or a failover, still moves wherever there is real room, as
  today.
- **G5.** When every account is above the threshold, the `no-qualifying-candidate` sentence says
  so.

### Non-goals

- A `--ceiling` flag on `auto`. The setting is the only way to set it.
- Manual `switch` and its strategies. They keep judging the limit at 100% (core §9.3).
- Poll cadence, notifications (the `limit` notice still fires at the threshold), the TUI header,
  History and the statusline. None of them shows the ceiling.
- The issue's other extras: logging the ranking's verdict during the cooldown, and a
  notification the first time every account is above the threshold.

### Anti-goals

- Any change in behaviour at the default ceiling.
- Stranding real headroom, or starting metered API-key use, because of the ceiling.
- Flapping between accounts near the ceiling, however many there are (§3.8).

## 3. Design

### 3.1 Terms

| Term | Meaning |
|---|---|
| **full** | known headroom ≤ `100 − ceiling`: usage at or above the ceiling |
| **out** | known headroom ≤ 0: core §8.2's "at the limit". Every out account is full. |
| **room** | known headroom > 0 |
| **hard move** | the trigger is `failover`, or it is `at-limit` and the live account is out |
| **soft move** | the trigger is `at-limit` and the live account is full but not out |
| **margin** | 3 points, §11.3's recovery step (`RECOVERED_PTS`, `autoswitch.rs:298`) |
| **clear** | known headroom > `100 − ceiling + margin`: usage more than 3 points below the ceiling. A soft move lands only on a clear account. |

At ceiling 100, full and out are the same set, and every `at-limit` is hard. That is G1.

"Full" has one definition. A headroom test and a window test (§3.4) must agree on a window at
exactly the ceiling: both call it full. Judging headroom as `h ≤ 100 − ceiling` and a window as
`pct ≥ ceiling` gives that. Unknown headroom is never full, as it is never out today (core §8.2).

### 3.2 The setting

- **Key:** `autoswitch.ceiling`. A float, default 100.0, valid 50–100 on its own, per provider.
  It sits after `autoswitch.threshold` in `KEYS` and in the core §6.4 table.
- **Pair rule:** for each provider, the effective `threshold < ceiling`. Each value is the
  provider's own table first, then the top level, then the default.
- **Reading** stays forgiving (core §6.4). A provider whose effective pair breaks the rule runs
  with ceiling 100. It gets one warning naming both keys and their values, and the threshold is
  kept.
- **`config set` and `config unset`** refuse a write that breaks a provider's pair: one after
  which a registered provider's effective pair breaks the rule when it held before the write.
  The refusal names the provider and both values, for example "`autoswitch.ceiling` must be
  above `autoswitch.threshold` (90 for claude-code)". As with every refusal, nothing is written.
  This covers both spellings, `provider.<id>.…` and `--provider`.
  - A pair that was already broken, by a hand edit, does not block other writes. Reading
    already runs it at ceiling 100 with a warning, and the TUI's `P` writes
    `daemon.autoswitch` through `config set` (`crates/tagteam-tui/src/host.rs:285-297`), which
    must keep working. *(My call, from the builder check.)*
- **`auto --threshold`** still clamps to the threshold's range. If the flagged threshold is at or
  above a provider's ceiling, that provider runs with ceiling 100. It emits one `config-warning`
  at start, and again after each reload that still has the conflict.

### 3.3 The decision (core §11.2)

- **Step 5.** With known headroom, a full live account gives `at-limit`; before, it was headroom
  ≤ 0. `below-threshold`, `consume-first` and `proactive` are unchanged. `at-limit` is checked
  first, as now (`autoswitch.rs:541-547`).
- **Step 6** is unchanged. `at-limit` and `failover` bypass the cooldown.
- **Step 8, `proactive` and `consume-first`:** skip full candidates; before, it skipped headroom
  ≤ 0. The rest is unchanged.
  - **"Spent"** in the every-account-above rule ("within 3 points of headroom") is measured from
    the ceiling: `headroom − (100 − ceiling) ≤ 3` (`SPENT_HEADROOM_PCT`,
    `recovery_axis_useful`, `autoswitch.rs:408-418`). Without this, a ceiling of 97 or lower
    makes "both spent" impossible, since every ranked headroom is above 3.
- **Step 8, a soft move:** the clear OAuth candidates, most headroom first, ties to the lower
  position. Nothing else is a target.
- **Step 8, a hard move:** two tiers, each ordered by most headroom, ties to the lower
  position.
  1. OAuth candidates that are not full.
  2. Full OAuth candidates with room.
- **No §11.3 bar.** Neither move applies the no-return bar, as `at-limit` and `failover` don't
  today. The margin does its work for soft moves (§3.8).
- **Steps 9 and 10, API keys:** appended after the OAuth targets on a hard move only. Today
  every `at-limit` and `failover` gets them (`autoswitch.rs:752-758`).
- **Step 9, nothing ranked:** `all-exhausted` when every OAuth candidate is known to be full;
  before, every one known to be out. Its `earliestResetAt` is the earliest of the candidates'
  back-at times, at the level §3.4 gives for the case. The other outcomes
  (`no-comparison`, `no-qualifying-candidate`, and so on) are unchanged. In particular, a soft
  move whose candidates are below the ceiling but none clear gets `no-qualifying-candidate` at
  the normal cadence.

### 3.4 Back-at time (`rank::blocked_until`)

`blocked_until` takes the level as a parameter: the latest reset among relevant windows at or
above it. A candidate is "back" at that time. `all-exhausted` uses two such times, and at
ceiling 100 they are the same time, which is G1.

- **The reported time** (`earliestResetAt`, the "frees up at" sentence) is when the move this
  tick wanted could first land:
  - **On a hard move**, the level is 100. A hard move can land on a full candidate with room
    (§3.3 tier 2), so a window between the ceiling and 100 must not postpone it.
  - **On a soft move**, the level is `ceiling − margin`. Only a clear candidate can be its
    target.
  - **On `proactive`**, the level is the ceiling. Only a candidate below the ceiling can be its
    target.
  - For example, with ceiling 95, take a candidate whose 5h is at 100% and resets in 2 minutes,
    and whose 7d is at 97% and resets in 3 days. It is back in 2 minutes for a live account at
    100%, and in 3 days for one at 96%.
- **The wake time** is the earliest back time at level 100. It is used only when every OAuth
  candidate is out (§3.5). At that time a candidate first has room, which is when a hard move
  first becomes possible. Today's sleep is exactly this.
- *(My call, from the builder check and the spec review: a single level would either report a
  time at which the move cannot happen, or lengthen today's sleep.)*
- Manual `switch`'s `candidates-exhausted` message (`switch/select.rs:572`) passes 100, so its
  text does not change.

### 3.5 Loop delay (core §11.4)

`all-exhausted` sleeps toward the wake time (§3.4, capped at 600 s) only when every OAuth
candidate is out, so the sleep is exactly today's. When one still has room and is only full,
the loop keeps the normal cadence,
as `no-qualifying-candidate` does. Otherwise a live account that reaches 100% during a 600 s
sleep would wait out the sleep before the hard move in §3.3.

Either way, `all-exhausted` stays BLOCKED, and keeps its event, its stored entry and its
`--once` exit code. The decision must carry which case it is and, where they differ, both
times. How it does so is the builder's choice.

### 3.6 The switch record

`SwitchCause` gains `ceiling` beside `threshold` (`engine/src/auto.rs:1216-1225`,
`switch/mod.rs:94-108`). A stored switch's `detail` becomes
`{"window","label","pct","threshold","ceiling"}`. The field is additive, and readers ignore
unknown fields. The sentence stays as it is ("alex's 5h reached 96%").

### 3.7 The `no-qualifying-candidate` sentence

When nothing ranked and every account is above the threshold (core §11.2 step 8's exception),
`no-switch` carries `detail` = `all-above`; today the detail is empty.

- The sentence (redesign spec §11.3, `words.rs:578`) becomes "every account is above 90%, and
  none is far enough ahead to switch to". The threshold is printed as `words::threshold`
  prints it.
  - It must not name a particular test that failed. Which gates applied differs per pair: the
    headroom axis, the recovery axis, or the no-return rule.
- Every other case keeps "no other account has enough headroom".
- `--json` consumers see the new detail value.

### 3.8 Why it does not flap

- **A soft move leaves an account at or above the ceiling, and lands only on one more than 3
  points below it.** For auto to soft-move onto an account it once soft-moved off, that
  account's reading must fall by more than 3 points in between. Only a window reset or a
  genuine recovery does that. It holds however many accounts take part, with no state beyond
  the readings.
  - Two accounts, ceiling 95: A at 95% moves to B only if B is under 92%. If B reads 94.9%,
    nothing moves (`no-qualifying-candidate`).
  - Three accounts, ceiling 95: A at 95% moves to C at 91%. When C reaches 95%, A and B must
    be under 92% to be targets. A reading that only dips to 94.9% never qualifies, so the cycle
    in the spec review cannot start.
- **A hard move happens only when the live account is out, or on failover, and only to an
  account with room.** Leaving an out account needs real usage to reach 100%, and a move onto
  a full account (hard tier 2) is followed by no move until that account is out.
- *(My call, from the spec review: the margin replaces a no-return bar on soft moves. A bar
  covers only the account just left, so three accounts could still cycle.)*

## 4. Tests

Each new rule gets a test that fails before the change.

- **Decision, in `autoswitch.rs`.** Threshold 90 and ceiling 95 unless a case says otherwise.
  - Live at 95, a candidate at 80, inside the cooldown, and the candidate the account the engine
    last left: an `at-limit` switch to it (G2; no cooldown, no bar).
  - Live at 96, candidates at 97 and 99: `all-exhausted` at the normal cadence, and
    `earliestResetAt` is the earlier back-at time at `ceiling − margin`, since the move is soft
    (§3.4, §3.5).
  - Every candidate out, one with 5h at 100% resetting in 2 min and 7d at 97% resetting in 3
    days. With the live account at 100%, `earliestResetAt` is in 2 min. At 96%, it is in 3
    days. Either way the sleep is today's, toward the 2 min reset (§3.4).
  - Live at 95, one candidate at 93: no soft move (`no-qualifying-candidate`). At 91.9 it is
    clear, and the move happens. Live at 100 with the candidate at 93: the hard move takes it.
  - Both sequences in §3.8: no move onto an account that only dipped below the ceiling, with
    two accounts and with three.
  - Live at 100, a candidate at 96: an `at-limit` switch to it (G4).
  - Live at 95, keys included, every OAuth candidate full: no move onto a key. Live at 100 in
    the same state: the full OAuth candidates with room come first, then the key.
  - `proactive`, live at 92, a candidate at 96: the candidate is never a target (G3).
  - "Spent" measured from the ceiling (§3.3).
  - At ceiling 100, each of the above gives today's decision.
- **`rank`.** `blocked_until` at a level below 100, and at 100 unchanged.
- **Loop delay.** `all-exhausted` with a candidate that has room keeps the normal cadence; with
  every candidate out, it sleeps toward the reset.
- **Settings.** A read with a bad pair, in a provider table and at the top level. `set` and
  `unset` refusals in both spellings, with nothing written. An unrelated write, such as
  `daemon.autoswitch`, succeeds on a file whose pair is already broken. A `--threshold` above the
  ceiling.
- **Simulation (`crates/tagteam-core/tests/simulate.rs`, core §11.5).** The ceiling is drawn from
  (threshold, 100], with a fixed share at exactly 100. The properties become:
  - never landing on a full account, unless the move is hard and no OAuth candidate below the
    ceiling is viable;
  - never landing on an account with headroom ≤ 0 (unchanged);
  - never staying on a full live account while a clear OAuth candidate exists;
  - a soft move lands only on a clear account;
  - never idling out while a viable candidate exists (unchanged);
  - never landing on an API key unless the move is hard and no OAuth candidate has room;
  - `all-exhausted` only when every OAuth candidate is full, with the earliest reset as §3.4
    defines it.
  The hard-coded `h <= 0.0` checks become these (`simulate.rs:526`, `:557`, `:617`).
- **Engine.** One `auto_tick` test with a ceiling below 100: the event stream and the stored
  `all-exhausted`.
- **Words.** Both `no-qualifying-candidate` sentences.

The existing suites run unchanged at the default ceiling. That is the regression check for G1.

## 5. Records

- **Core spec:**
  - the §6.4 table row and the pair rule;
  - §11.2 steps 5, 8, 9 and 10;
  - §11.4's loop-delay table;
  - §11.5's properties.
  Each passage gets "*(Amended by the autoswitch-ceiling spec.)*".
- **Redesign spec §11.3:** the `no-qualifying-candidate` sentence row.
- **README, "Automatic switching":**
  - the setting and what it is for;
  - that tagteam picks the account but cannot stop usage that reaches it from elsewhere, so a
    ceiling below 100 is a margin, not a guarantee.
- **CHANGELOG.**

## 6. Left to the builder

- How full, out and hard are expressed in code, for example a level type or methods on `Rated`.
- How `next_delay` learns which `all-exhausted` case it has (§3.5).
- Where the settings pair check lives.
- The sentence's final wording within §3.7's constraint, and the test fixtures.
