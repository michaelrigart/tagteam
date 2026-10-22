# tagteam TUI redesign T4 — History: the timeline — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Approved
**Goal:** Turn History into one time-based screen (usage charts, a lane per account, auto's events on the same axis, an event list synced to a time cursor) read from the store, record the events that screen needs (failed switches, every account exhausted, pauses and resumes, the window behind an automatic switch, a removed account's label), word them once in the engine, and fill the auto line from the store in both modes, with `w` opening History at its event.
**Architecture:** A new engine module `timeline.rs` holds `Engine::timeline` (bucketed charts, lanes and events in one read, on the views lane) and `Engine::last_auto_event`; the store gains range and kind reads over `events` and `active_history`, with no index and no migration. The writers live in the auto engine, the switch commit, lifecycle `remove` and purge, and the daemon's provider job, under R4e's `EventSource`. `words::event` says each event. The TUI's History screen is rewritten over `TimelineView` (`screens/history.rs` plus `history/{layout,chart,lanes,list,marks}.rs`), and the auto line reads `AccountsSnapshot.last_auto`.
**Tech Stack:** Rust 1.88 (edition 2024), ratatui 0.30.2, crossterm =0.29.0, rusqlite. No new dependency.
**Spec:** docs/superpowers/specs/2026-10-20-tagteam-tui-redesign-design.md (§2, §3, §4, §5.3, §5.4, §7, §9, §10, §11.3, §11.4, §12.3, §12.4, §13, §14, §15's T4 row, §16, §17's VIS-5, VIS-12, UX-3, UX-11, §18, §19); refactor spec §8.7 (R4e).
**Base:** Tasks 1–4 on main 951c9ae; Tasks 5–13 on main after refactor slices R4e and R4e2 merge (R4e2, stacked on R4e, types the store's events and makes every writer read the engine's source).

**Calls ruled within the spec** (listed for Michael under the PR's Rulings; an objection changes one ruling and its task).
1. **`tagteam auto --once` from cron while every account is exhausted** (T4-24). §12.4 records "the first `AllExhausted` decision after any other decision", which a new process cannot know. A fresh engine's first exhaustion is recorded unless the provider's latest auto-line event is an `all-exhausted` with the same earliest reset. Otherwise a cron job writes one row per run until a reset (C-11 expects "a few rows an hour").
2. **Esc and the scope sync** (T4-17, UX-11). `⏎` on Accounts opens History scoped to the account; `Esc` returns to Accounts *and* resets the scope to all accounts. While History is scoped, moving the Accounts selection moves the scope. Without the reset, `3` would reopen the last ⏎'d account's history.
3. **Re-read triggers stay §7.7's** (T4-19). A switch-failed, an all-exhausted or a pause the daemon stores reaches the timeline and rank 1 of the auto line at the next read (at most 60 s in live mode), since §7.7 and §8 re-read on none of `error`, `all-exhausted` or `mode`. Meanwhile rank 2 shows the daemon's live event. The alternative is three extra triggers.
4. **A new engine file** (T4-3). §3.1 names `views.rs` and the store's readers; the timeline view goes in a new `crates/tagteam-engine/src/timeline.rs` beside it rather than growing `views.rs` by half.

**Reading this plan.**
- TUI paths start `src/` and are under `crates/tagteam-tui/`; every other path is written in full.
- Code is cited by the code quoted. Line numbers are hints as of `951c9ae` (Tasks 1–4) or main after R4e (Tasks 5–13).
- Focused tests: `cargo test -p tagteam-tui --locked <filter>`; `cargo test -p tagteam-engine --locked --features test-hooks <filter>`; `cargo test -p tagteam-daemon --locked --features test-hooks <filter>`; `cargo test -p tagteam --locked --features test-support --test <file>`.
- The pty tests (`tui_pty.rs`, `perf.rs`, `signals.rs`) open a pseudo-terminal: run them outside the sandbox.
- Each task leaves the workspace compiling, with its crates' tests green.
- `TIERS`, `roles`, `canon`, `draw`, `body_of`, `fresh`, `snapshot`, `many`, `read`, `NOW_S`/`NOW_MS` (Mon 2026-09-21 14:13:20 UTC, the engine fixture's clock too) are the existing `testutil`/`common` helpers. Tests run in UTC.
- **Focus** is `daemon::focus(state)`: the selected account's provider, else `--provider`'s, else the default.
- **Hand-computed text.** Where a value this plan spells out disagrees with the rule it states, the rule wins: the implementer fixes the expectation to the rule and says so in the task report.

## Global Constraints

- **CI commands** (`CONTRIBUTING.md`): `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings`, and again with `--features tagteam/test-support`; `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`.
- **Toolchain:** Rust 1.88, edition 2024, ratatui 0.30.2, crossterm =0.29.0 (pinned). `tagteam-tui` stays `#![forbid(unsafe_code)]`. No `#[allow(...)]` is added.
- **No new dependency.** `git diff <base> -- Cargo.toml Cargo.lock 'crates/*/Cargo.toml'` prints nothing at the end.
- **A type, field or function is added in the task that first uses it**; dead code is deleted, or `#[cfg(test)]` when only tests use it.
- **`--json` unchanged** everywhere: no command gains a field (§1.3, §11). The daemon protocol is untouched and its JSON byte-compatible: no `protocol.rs` edit, and `protocol.rs`'s `every_auto_kind_is_written_byte_for_byte_as_auto_wrote_it_before` passes unchanged.
- **The store (§12.4):** new kinds use the free-text `events.kind` column. No migration, no index, no schema change: `schema.sql`, `schema_v3.sql` and the schema version are unchanged, pinned by Task 5's `four_new_kinds_need_no_migration`. Readers leave out a row this build cannot type, and it stays stored (R4e's rule, §12.4's "readers ignore kinds they don't know"). The writers go through `Store::insert_event_on`/`insert_event`, in the change's transaction where there is one.
- **Provider neutrality:** no production line T4 adds under `crates/tagteam-tui/src`, `crates/tagteam-engine/src` or `crates/tagteam-daemon/src` names `claude` or `anthropic` in any case, nor uses `tagteam_cc`. Provider words come from `ProviderNames`. `no_production_line_names_the_provider` and `crates/tagteam-fake/tests/neutrality.rs` run.
- **Words.** §11.4's sentences live in `crates/tagteam-engine/src/words.rs` (Task 10) and reuse its words: `short_name`, `trigger`, `no_switch` (all-exhausted), `threshold`, `NEEDS_LOGIN`, `name`, `email`; the store's `CREDENTIALS_REPLACED`/`ACCOUNT_REPLACED`. Nothing re-spells them. TUI-only labels (`at cursor`, `WHEN`) live in the TUI file that draws them.
- **Reuse T2/T3's TUI pieces:** `gauge::fill_role`, `chrome::{WIDE, RULE_FROM, fit_spans}`, `autoline::when`, `daemon::words::clock`, `view::{fit, pad, width_of, select, tint, keep_visible, centred_block}`, `theme::Role`, `Overlay::Choose`, `chrome::now_strip` (stays on History, untouched).
- **The first frame.** `tests/perf.rs`'s first-frame p95 stays within 50 ms. The only new read before it is §14's `last_auto_event` inside the accounts read (Task 12, pinned by a new perf case over a year of events). The timeline is read only once History shows and the first accounts read has arrived.
- **Example names only** (§3.4): `alex@example.com`, `sam@corp.example`, `alex@personal.example`, `Example Corp`, `ci@example.com`, the fixtures' `a1@x.co` style. Display names are the providers' own (`Claude Code`, `FakeAgent`).
- **No test is weakened.** A test edit is a mechanical change to an API this plan changes (listed in its task), a render test rewritten to the new layout per §16 (named in its task), or a new test. Timing bounds pin which ending fired, never scheduler latency.
- **Moves are separate from edits.** No task moves code between files; replaced code is rewritten in place or deleted.
- **Commits:** imperative, sentence case, no prefix, no trailing period, no agent attribution; each task's message is in its last step. A task's commit stays under about 90 KB (`git show --format= <sha> | wc -c`). A task that would exceed it tells the controller, who splits it by file into two commits. Task 2, which deletes the old screen (about 41 KB), is the one at risk.
- **Files this plan may touch:** those each task's Files list names, plus `CHANGELOG.md`, the TUI, core and daemon specs (Task 13) and this plan. Not edited: `crates/tagteam-core/**`, `crates/tagteam-provider/**`, `crates/tagteam-cc/**`, `crates/tagteam-daemon/src/protocol.rs`, `schema*.sql`.
- **Never committed:** the review harness (`src/ux_*.rs` and their `lib.rs` lines) and any render. Before every commit the controller checks `git status --short` lists no `ux_` file.

### What T4 takes from the refactor

R4e is planned against the refactor session's stated final shape (2026-10-22). It ships as two slices: R4e (the enums, `EngineConfig.source`, `Engine::source()`, `engine_for`'s `source`; no behaviour change) and R4e2, stacked on it (every writer reads the engine's source, per-call sources go, the store's events are typed, and auto and the daemon record their real source).

| Name (final API) | From | Tasks |
|---|---|---|
| `tagteam_engine::{EventKind, EventSource}` (`event.rs`, `str_enum!`): `ALL`, `as_str`, `parse` | R4e | 5–12 |
| `EngineConfig.source`, `Engine::source()`; the daemon's engine is `Daemon`, `tagteam auto`'s `Auto`, the TUI's lanes `Cli` | R4e (the real sources recorded: R4e2) | 6, 7, 8 |
| `app::engine_for(ctx, provider, source)`; `tui_cmd.rs`'s two calls pass `EventSource::Cli` | R4e | Before Task 5 only |
| Every writer reads `self.source()`: no `SwitchRequest.source`, no `AutoEngine.source`/`with_source`, no `GuardRequest.source`; `transact.rs`'s `switch` row uses `self.source()` | R4e2 | 6, 7, 8 |
| `EventRow { kind: EventKind, source: EventSource, .. }`; `event_from_row -> rusqlite::Result<Option<EventRow>>`; `events()` leaves out untypeable rows | R4e2 | 5, 6, 9 |
| `log_event`, `insert_event_on`, `insert_event` over the typed row; `Engine::event(provider, kind: EventKind, from, to)`; `LoginSource.event: EventKind` | R4e2 | 6, 7, 8 |
| `clear_quarantine(.., source: EventSource, ..)`, `finish_replacement(lock, source, at)` (named; T4 calls neither) | R4e2 | — |

Later slices: R4f, R4h, R5a and R5b touch no event API. **R4g** restructures `auto.rs::perform` into `try_target` with no API or stored-value change; Tasks 6 and 7 edit `perform`, so whichever lands second resolves that hunk (Before Task 5 checks it).

## Rulings

Calls inside the spec, for the PR's "Rulings" section.

| # | Ruling | Why |
|---|---|---|
| T4-1 | **Refactor names.** This plan uses R4e's final API as the table above states it. A name the merged R4e differs in is patched into this plan before Task 5; the behaviour asked of it does not change. | R4e Tasks 3–4 were unmerged when this was planned. |
| T4-2 | **Order around R4e.** Tasks 1–4 touch no event API and run on main as it is. Tasks 5–13 run after the branch is rebased onto main with R4e. | §3.2: T4 waits for R4; the brief splits the work around R4e. |
| T4-3 | **`crates/tagteam-engine/src/timeline.rs`** holds `Scope`, `Grid`, `AccountRef`, `Series`, `Lane`, `TimelineView`, `EventView`, `Engine::timeline` and `Engine::last_auto_event` (`pub mod timeline;`). Planned call 4. | §12.3's view with its own bucketing; `views.rs` holds the accounts and history views. |
| T4-4 | **The grid.** `span = max(now_ms − since_ms, 1)`. `column_of(t)` is `None` before `since_ms`, after `now_ms` or with no columns, else `min(⌊(t − since_ms) × columns / span⌋, columns − 1)`: `now_ms` is in the last column. `end_ms(c) = since_ms + ⌈(c + 1) × span / columns⌉`, `now_ms` for the last, so that `column_of(end_ms(c)) == c + 1`; column `c` starts at `end_ms(c − 1)` (`since_ms` for 0). Samples (seconds) are placed at `fetched_at × 1000`. The view's grid is `(since_s × 1000, now_s × 1000, columns)` on both sides. | §12.3: "each column's peak, as `history.rs`'s `sparkline` does". |
| T4-5 | **Binding usage per column** (C-2, `rank::binding_window`). An account's relevant windows are `Engine::history`'s: its last good reading's windows in order, then the other keys `usage_window_keys` lists, described by `Provider::describe_window` (a key it does not know is skipped), kept when `is_relevant` under the provider's effective `autoswitch.models`. A lane's column is the highest `pct` of those windows' samples in it. The all-accounts chart's column is the highest `pct` of any account's relevant samples taken while that account was in use. One account's chart per window is that window's samples' peak. | §7.3, §7.4; "a column spanning a switch shows its peak". |
| T4-6 | **In use.** `active_history_since` gives intervals: each row's account from its `at` to the next row's (the last to `now_ms`); a `None` account is no one. A column is in use for an account when one of its intervals overlaps the column. The row before the range says who was in use when it starts. | §12.3's read; C-3. |
| T4-7 | **Lanes.** All accounts: one per account in use in some column, current accounts by position, then removed ones by first use. One account: that account's lane, used for the in-use lane. A removed account's `AccountRef` has no position and is named by its `remove` label (T4-27). | §7.4, §12.3. |
| T4-8 | **History's layout** (§7.1, §7.8, §19's cap). Fixed rows: the scope row, the events lane and the axis, plus the chart's title row (all accounts) or the in-use lane (one account, whose charts each bring their own title row). From `RULE_FROM` (30) terminal rows a blank follows the scope row and precedes the list; from `WIDE` (120) columns the list has a head row. `avail` = the body less those. **All accounts:** `room = avail − LIST_LEAST (4) − CHART_LEAST (5)`; lanes shown = `min(n, min(LANES_MOST (10), room − 1))`, plus a `+N more · f picks one` row when some are hidden; chart rows = `clamp(avail − 4 − lane rows, 5, 12)`; the list takes the rest. Hidden lanes are those with the fewest in-use columns (ties: higher position). **One account:** charts shown = `min(n, (avail − 4) / 6)`, each `min(8, (avail − 4) / shown − 1)` rows under its title; `J`/`K` scroll them; no relevant window means no chart. **`z`:** the list takes every row under the scope row (and its blank and head). | Reproduces §7.8's 80×24 rows (9-row chart, 3 lanes, 4-row list; 5 rows and a cap of 6 with more accounts) and §7.4's 10 at 120×40. |
| T4-9 | **A chart** (§4.4). A value `v` over `N` rows is `e = round(min(v, 100) / 100 × N × 8)` eighths, at least 1 when present; row `r` (0 at the bottom) is `█` when `e ≥ 8(r+1)`, `▁▂▃▄▅▆▇`[`e − 8r − 1`] when `8r < e < 8(r+1)`, else empty. Its role is `fill_role(v)`. An empty cell is `│` (accent) in the cursor's column, else `┈` (faint; tier 0 `·`) on the threshold row `min(⌊t / 100 × N⌋, N − 1)`, else blank. The y axis is `{:>7}` then two spaces: `100` on the top row, `0` on the bottom, `50` on row `min(⌊N / 2⌋, N − 1)` from 6 rows. `t` is the view's threshold (all accounts) or 90 (one account). | §4.4, §7.3. A present 0 % must not look like an empty bucket. |
| T4-10 | **Geometry.** Column 0 is the marker column and columns 1–8 the gutter, so a chart starts at `CHART_X = 9`; one column is kept at the right, so `columns(width) = width − 10` (70 at 80, 110 at 120). A lane's gutter is ` {fit(name, 7):<7} `. | §7.3's 8-column gutter and §7.1's mock-up. |
| T4-11 | **Readouts** (§7.3). On a chart's title row, right-aligned one column in and never cut: `at cursor {v:>4}`, then `   now {v:>4}`, percentages rounded, `—` without a value. `at cursor` is the cursor column's peak; for a switch at the cursor, the all-accounts chart says `at cursor {before:>4} → {after:>4}`. `now` is the current reading from the accounts view: the in-use account's highest relevant window, or that window's `pct` by label. Values in `fill_role`'s role, labels muted. The all-accounts subtitle shows from `WIDE` columns: `the higher of its 5h and 7d: what auto compares with 90%` (one window `its 5h`; three or more `the highest of its 5h, 7d and Fable`), the in-use account's relevant labels; under `consume-first` without the clause after `:` (C-17). | §7.3 leaves the source of `now` open; the subtitle cannot fit at 80 beside the readouts. |
| T4-12 | **The axis** (§7.4, §19). In the TUI's time zone: 1 day, `╵HH` at local hours divisible by 3; 7 and 14 days, `╵Tue` at local midnights; 30 days, `╵Sep 7` at local Monday midnights. Each label starts at its boundary's column. `now` ends at the last chart column. A label is drawn only when a blank column separates it from the label before it and from `now`. All muted. | §19 leaves the 1- and 30-day labels to the builder. |
| T4-13 | **The events lane** (§7.4). Each column draws its highest-rank event's glyph in its role (ranks 6–1 as §7.4's table, `marks::mark`). A pause's span is `┄` (warn; tier 0 `·`) in each column after the pause's and before the next resume's (to the last column without one), from column 0 when the view is `paused_at_start`; an event's glyph wins over the span. In the cursor's column, the glyph is bold and REVERSED (`Theme::marked`), or `│` (accent) with no event there. | §7.4. |
| T4-14 | **The cursor** (§7.5). `Cursor::{Now, At(ms)}`. `←`/`→` move one column: column `c` reads `At(end_ms(c) − 1)`, the last column `Now`. `j`/`↓` and `k`/`↑` select the next or previous list row (newest first) and put the cursor at its time. `Home` is the oldest event, or column 0 with none; `End` is `Now`. Moving by column selects the newest event at or before the cursor (none: no selection). A cursor time before the range reads as column 0. The scope row says `cursor {when}` or `cursor now`. | §7.5 gives no rule for `Home` without events. |
| T4-15 | **Placement** (§7.5). On open, a scope change and a range change, the cursor goes, once the next view arrives, to the newest event whose kind is in `AUTO_LINE_KINDS`, else `Now`. `w` puts it on the auto line's event (the first with that `at_ms` and kind, else the newest at or before it); rank 2 or 4 puts it at `Now`. A refresh keeps the cursor and re-selects from it. | §7.5. |
| T4-16 | **`w`'s range.** The scope becomes all accounts. The range changes only when it does not hold the event, to the shortest of 1, 7, 14 and 30 days that does. | §5.3's "widening". |
| T4-17 | **Scope and its sync** (§7.2, §6.5, UX-11). `f` opens `Overlay::Choose` asking `Show History for` with `then: ChooseThen::Scope(scopes)`: `all accounts`, then each of focus's accounts as `{position} {words::name(row)}`, the current scope preselected; `⏎` or a digit picks. Choosing an account selects it on Accounts. While scoped, an Accounts selection change moves the scope to it. A scoped account gone from the view returns the scope to all accounts. `⏎` on Accounts scopes History and marks it opened from Accounts; `Esc` there returns to Accounts with the scope back to all accounts; else a scoped History's `Esc` returns to all accounts. Planned call 2. | §10's `Esc` row and "in sync both ways". |
| T4-18 | **History follows focus's provider**: the timeline is read for it, and `f` lists its accounts. | As the header and Now strip do (T3-7). |
| T4-19 | **Re-reads** (§7.7). On open, a scope or range change (the view is cleared: `loading…` until the read), a resize (250 ms after the last, the old view drawn left-aligned and clipped meanwhile), the 30 s timer in timer mode, actions, collections and a link coming up: at once. Live events, debounced 250 ms as today: `switch`, `account-quarantined` and `account-unquarantined` at once; `poll`, `usage`, `changed` and the 60 s live poll paced, at most every 30 s, a paced read inside 30 s of the last running 30 s after it. A TUI started on History reads once the first accounts read arrives. Planned call 3. | §7.7; §14's first frame. |
| T4-20 | **The event list** (§7.6). Newest first. A row: marker column, two spaces, the time (`when`, muted) padded to the widest time in the list, three spaces, the glyph in its role, a space, the sentence in text, the row cut to the width by `fit`. The head `WHEN`/`WHAT` (muted) aligns with them from `WIDE` columns. The selected row is `view::select`ed with its sentence bold. When rows do not fit, the last row is `⋮ {n} more · z full list` (with `z` on, `· z charts`), `n` counting the rows not shown, and the list scrolls with the selection (`keep_visible`). No events: `no events in the last 7 days`, muted. | §7.6. |
| T4-21 | **Empty and loading** (§5.7). Nothing in range is no events, no lanes and no chart value: under the scope row, the centred block `Nothing in the last 7 days` (`… the last day` for 1 day) and `[ ] picks a longer range.`, the sentence left out at 30 days. Before a view, `loading…` (muted). A failed read raises the History banner and keeps the last view. | §5.7's row. |
| T4-22 | **The kinds** (§12.4): `EventKind` gains `SwitchFailed => "switch-failed"`, `AllExhausted => "all-exhausted"`, `AutoswitchPaused => "autoswitch-paused"`, `AutoswitchResumed => "autoswitch-resumed"`. `event.rs` also holds `AUTO_LINE_KINDS`, §5.3's eight in its order, since Task 7's writer reads it before the timeline does. | §5.3, §12.4. |
| T4-23 | **The switch's reason** rides in `SwitchRequest.auto`: `AutoPerform.cause: Option<SwitchCause>`. The auto engine fills it for every automatic switch whose live account has a decision-grade binding window (the window, its label and `pct`, and `cfg.threshold`), and `commit_switch` stores it as `{"window", "label", "pct", "threshold"}`. A manual switch has no `auto`, so none. | §12.4's "SwitchRequest gains an optional reason … manual switches leave it empty", without editing 15 manual literals. |
| T4-24 | **`switch-failed` and `all-exhausted`** (§12.4). `switch-failed` is written in `perform`'s non-fatal `Err(e)` arm only: not a `ProfileConflict`, not the unexpected no-op arm, not step 12's freshen failures (no single target). `from` the live account, `to` the target, `detail {"message": e.to_string(), "transient": transient(&e)}`, skipped when the provider's latest `switch-failed` of the last 30 minutes has the same `to` and message. `all-exhausted` is written when an `AllExhausted` no-switch is announced and this engine's previous decision was not one. The engine's `exhausted` flag is set after every tick's decision: true for an announced `AllExhausted`, false for any other, including a successful switch (which returns from `perform` without `announce`) and a failed one; an engine's first is written unless the provider's latest auto-line event is an `all-exhausted` with the same `earliest_reset_at`. `from` the live account, `detail {"earliest_reset_at": s or null}`. Dry-run writes neither. Planned call 1. | §12.4; C-11's volume. |
| T4-25 | **Pauses and resumes** (§12.4). `Engine::record_autoswitch(provider, on, at_start)`. A reload writes when the effective `daemon.autoswitch` differs from the job's previous settings. A job's first run writes `at_start` when the setting differs from the provider's latest stored `autoswitch-*` (none reads as resumed), with `detail {"at_start": true}`. Neither without an existing store, nor with `schema_newer`. A failure goes through the job's `failed` path and evaluation still runs. | §12.4's "at daemon start, when it differs". |
| T4-26 | **The store's reads** (§12.3). `events_since` (a scan, as §12.3 says); `latest_event(provider, kinds, since_ms, before_ms)`, since inclusive, before exclusive, newest by `(at, rowid)`, skipping rows this build cannot type; `removed_label(provider, id)`, the label of the latest `remove` naming it; `active_history_since` as two statements on `active_history_by_at` (the last row before, then the rows since), so neither scans nor sorts. | §12.3; an index-order read needs no temp B-tree. |
| T4-27 | **`remove`'s label** (C-12): `Engine::event` gains `detail: Option<Value>`, every caller but remove's passing `None`. Lifecycle `remove` and purge write the `remove` event with `{"label": email, else label}` in the deletion's own transaction, through `Store::delete_account_logged`, so an account is never deleted without its label. | §12.4's table; §12.4's writers go in the change's transaction. |
| T4-28 | **`AccountRef`**: `id`, `position` (`None` once removed), `alias`, `email` (the email, else the label; a removed account's from its `remove` label, else `None`) and `removed`. `words::account_name` is `short_name` for a current account, the label for a removed one, else `an account`. | §11.4's names; §12.3. |
| T4-29 | **`words::event(e, frees_at)`**: `frees_at` is `detail.earliest_reset_at` in the caller's clock form, as `no_switch` and `problem_sentence` take times. Edge copy: a manual switch with no `from` is `you switched to 3 spare`; `switch-failed` without a message, `quarantine` without a reason and an `unquarantine` of another reason end at the name's clause; an unknown trigger has no parenthesis; a removed account without a label is `an account`; the possessive is the `from` account's alias, else its email's local part. | §11.4; the engine has no time zone. |
| T4-30 | **The auto line from the store** (§5.3). `AccountsSnapshot.last_auto` holds each provider's `last_auto_event(provider, read_at − 30 days)`; its error fails the accounts read. Rank 1 is that event within 30 minutes, rank 3 within 30 days: a `switch` or `switch-recovered` as `last switch {when} · {sentence}`, any other kind as rank 1's layout. Rank 4 shows whenever a snapshot exists, in both modes (M-7 lifted). The daemon's `status.lastSwitch` is no longer read: the stored row is written in the switch's own transaction. The glyph is `marks::mark`'s. Empty before the first snapshot. | §5.3, M-7. |
| T4-31 | **Help** (§9.2): Marks gains `┄` paused meanwhile, `+` added, `-` removed and `┈` the threshold, each in its role. | §7.4's lane glyphs are new marks. |
| T4-32 | **Records** (§18): core §5's `events` kind list (T3-31's deferral), daemon D4.2's pause sentence, TUI spec §4.4 (`timeline`), §5.4 (the timeline), §5.6 (`w`, History's keys), §8 (what re-reads History; the auto line from the store). | §18. |

## Review Focus

1. **A row a newer build wrote** (unknown kind, or `switch` from an unknown source). History and the auto line leave it out; the auto line uses the next older readable row; nothing errors. Tests: Task 5, `the_latest_event_skips_a_row_this_build_cannot_type`; Task 9, `rows_a_newer_build_wrote_are_left_out_of_the_timeline_and_the_auto_line`.
2. **An account removed after the events that name it.** Its sentences use its stored label, its lane survives without a position, and nothing assumes a position. Tests: Task 9, `a_removed_account_is_named_by_the_label_its_remove_event_stored`; Task 10, the removed rows of `every_event_kind_has_its_sentence`; Task 11, `a_removed_account_s_lane_and_events_draw_with_its_label`.
3. **`tagteam auto --once` from cron while every account is exhausted.** One `all-exhausted` per episode, not per run. Test: Task 7, `a_fresh_engine_records_an_exhaustion_the_store_already_has_once`.
4. **Several providers, History scoped to an account of the non-default one.** The timeline is that provider's, and `f` lists its accounts. Test: Task 4, `a_scope_on_another_provider_s_account_reads_that_provider`.
5. **A narrower terminal before the debounced re-read.** The old, wider view draws clipped and the next read has the new columns. Test: Task 3, `a_view_wider_than_the_screen_draws_clipped_until_the_re_read`.

## Spec coverage

| Spec | Tasks |
|---|---|
| §7.1–§7.3 layout, scope, range, charts; §7.8 budget; VIS-5, VIS-12 | 2, 3, 4 |
| §7.4 lanes, axis, events lane | 3, 11 |
| §7.5 cursor; §7.6 event list; §10's History keys | 4, 11 |
| §6.5 / UX-11 scope sync, `⏎` and `Esc` | 4 |
| §7.7 one read, re-read rules, no fetch | 1, 2, 4, 9 |
| §12.3 `Engine::timeline`, `events_since`, `active_history_since`, `AccountRef`, `EventView` | 1, 5, 9 |
| §12.2 `last_auto_event` | 9, 12 |
| §12.4 kinds and writers; C-11, C-12 | 5–8 |
| §11.4 sentences; C-14, C-17 | 10 |
| §5.3 store-fed auto line, ranks 1, 3, 4, M-7; `w` (C-4); UX-3 | 12 |
| §9.2 legend | 12 |
| §14 first frame | 12 |
| §16's History, cursor, `w`, scope-sync, engine and writer tests | 1–12 |
| §18 core, daemon and TUI spec hunks; CHANGELOG | 13 |

**Order.** 1 → 2 → 3 → 4 (no R4e), then the rebase onto main with R4e (Before Task 5), then 5 → 6 → 7 → 8 → 9 → 10 → 11 → 12 → 13. Every task builds and tests shared crates, so one implementer runs at a time.

---

## Before Task 1 (controller)

- [ ] `git -C /Users/michael/Code/tagteam-tui-redesign status --short` prints nothing; the branch is `tui-redesign-t4` on main (`951c9ae`, or later main without R4e). Record it as `<base>`.
- [ ] **Names Tasks 1–4 use.** Each prints at least one hit; a miss is patched into this plan first:
  - `rg -n 'pub fn active_history\(' crates/tagteam-engine/src/store/tokens.rs`
  - `rg -n 'pub fn (usage_window_keys|usage_samples|usage_state)' crates/tagteam-engine/src/store/usage.rs`
  - `rg -n 'pub fn binding_window|pub fn (is_relevant|max_relevant_pct)' crates/tagteam-core/src/rank.rs crates/tagteam-core/src/usage.rs`
  - `rg -n 'fn describe_window' crates/tagteam-provider/src/provider.rs`; `rg -n 'pub fn existing_store' crates/tagteam-engine/src/engine.rs`
  - `rg -n 'pub\(crate\) fn (fill_role|fit_spans|when|focus|centred_block|select|keep_visible|clock)\b|pub\(crate\) const (WIDE|RULE_FROM)' crates/tagteam-tui/src`
  - `rg -n 'Choose \{' crates/tagteam-tui/src/overlay/mod.rs`
- [ ] **R4e's final API, on its branch** (T4-1). Once R4e's Tasks 3–4 are on `refactor-r4e-event-source`, run Before Task 5's R4e checks against it (`git grep -nE '<pattern>' refactor-r4e-event-source -- <path>`). A difference patches Tasks 5–13 now, before Task 1 starts; Before Task 5 runs them again on merged main. Until those tasks land, this plan's table above is the API.
- [ ] **The harness is parked** in `.superpowers/tui-redesign/harness/`; the "before" renders are `gallery/t3-after-15`.
- [ ] **The gallery check**, after Tasks 3, 4, 11 and 12, as T3's: copy `ux_gallery.rs` in, apply its `lib.rs` line, fix only its own compile errors, render with `UX_GALLERY_DIR=$PWD/.superpowers/tui-redesign/gallery/t4-after-<task> cargo test -p tagteam-tui ux_gallery -- --nocapture`, park it again. New scenarios, built from the tasks' fixtures: `history-all-3`, `history-all-6`, `history-all-12`, `history-account` (2 and 4 windows), `history-empty`, and from Task 11 `history-events` (every kind, a pause before the range), `history-full-list`, `history-removed`; from Task 12 `autoline-rank1`, `autoline-rank3`, `autoline-timer-rank4`. Check by eye at 80×24 and 120×40 in tiers 0, 1 and 2 against §7.1's mock-up, §7.4's lane table, §7.8's budget and §5.3's line. A difference is the drawing task's defect. Never quote the renders' real names.

---

### Task 1: Read the timeline's usage per column and who was in use

**Depends on:** none. **R4e:** not needed.

**Files:**
- Create: `crates/tagteam-engine/src/timeline.rs`; modify `crates/tagteam-engine/src/lib.rs` (`pub mod timeline;`).
- Modify: `crates/tagteam-engine/src/store/tokens.rs`: `active_history_since` beside `active_history`, and its plan pin in the test module (its `plan` helper).
- Test: create `crates/tagteam-engine/tests/timeline.rs` (`mod common;`, accounts by `common::add`, rows by raw SQL on `tagteam.db`); `crates/tagteam-engine/tests/store_tokens.rs`.

**Interfaces:**
- Produces:
  ```rust
  // store/tokens.rs (T4-26)
  /// The provider's active-account changes from `since_ms` on, after the last one before it:
  /// `(at_ms, account_id)` by `(at, rowid)`, `None` meaning no account was active.
  pub fn active_history_since(&self, provider: &ProviderId, since_ms: i64) -> Result<Vec<(i64, Option<String>)>, StoreError>;
  // timeline.rs
  #[derive(Debug, Clone, PartialEq, Eq)] pub enum Scope { All, Account(AccountId) }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub struct Grid { pub since_ms: i64, pub now_ms: i64, pub columns: usize }
  impl Grid { pub fn column_of(&self, at_ms: i64) -> Option<usize>; pub fn end_ms(&self, column: usize) -> i64; } // T4-4
  #[derive(Debug, Clone, PartialEq)] pub struct AccountRef { pub id: AccountId, pub position: Option<u32>, pub alias: Option<String>, pub email: Option<String>, pub removed: bool }
  #[derive(Debug, Clone, PartialEq)] pub struct Series { pub label: String, pub peaks: Vec<Option<f64>> }
  #[derive(Debug, Clone, PartialEq)] pub struct Lane { pub account: AccountRef, pub peaks: Vec<Option<f64>>, pub in_use: Vec<bool> }
  #[derive(Debug, Clone, PartialEq)] pub struct TimelineView { pub since_s: i64, pub now_s: i64, pub threshold: f64, pub columns: usize, pub charts: Vec<Series>, pub lanes: Vec<Lane> }
  impl TimelineView { pub fn grid(&self) -> Grid; }
  /// The all-accounts chart's label (§7.3).
  pub const IN_USE: &str = "in use";
  impl Engine {
      /// §12.3, reading only, never creating the store: `scope`'s charts and lanes over
      /// `columns` columns from `since_s` to now (T4-4 to T4-7). Never fetches.
      pub fn timeline(&self, provider: &ProviderId, scope: Scope, since_s: i64, columns: usize) -> Result<TimelineView, EngineError>;
  }
  ```
- `threshold` is `Settings::load(env, provider).threshold`. Without a store: all accounts is one `IN_USE` series of `None`s and no lane; one account is `EngineError::NoSuchAccount`, as for an account the provider does not have.
- Samples are read only through `usage_window_keys` and `usage_samples(id, Some(key), since_s)`, whose plans hardening pins; no pace work.

- [ ] **Step 1: Write the tests.** In the fixtures, `SINCE = T0 − 7 × 86_400`, columns 7 (column `c` is day `c`), `day(c, h) = SINCE + c × 86_400 + h × 3_600`. Accounts: `a` 1 `alex@example.com`, `b` 2 `sam@corp.example` alias `work`, `c` 3 `alex@personal.example` alias `spare`.
  - **`timeline.rs` unit, `the_grid_puts_each_instant_in_its_column`.** `Grid { 0, 7000, 7 }` gives `column_of`: `−1 → None`, `0 → 0`, `999 → 0`, `1000 → 1`, `6999 → 6`, `7000 → 6`, `7001 → None`; `end_ms(0) == 1000`, `end_ms(6) == 7000`. `Grid { 0, 10, 3 }` gives `3 → 0`, `4 → 1`, `6 → 1`, `7 → 2`, and `end_ms` 4, 7, 10. With no columns every instant is `None`.
  - **`a_lane_is_each_column_s_peak_of_the_account_s_relevant_windows`.** `a` in use from `SINCE − 3_600`. Samples: `5h` 40 at `day(0,1)`, 60 at `day(0,2)`; `7d` 30 at `day(1,5)`; `spend` 99 at `day(0,3)`; `scoped:Fable` 88 at `day(2,1)`. Lane `a`'s peaks are `[60, 30, —, —, —, —, —]` (`Some`/`None`), `in_use` all true. With `[autoswitch] models = ["fable"]`, column 2 is 88.
  - **`the_in_use_chart_takes_each_sample_only_while_its_account_was_in_use`.** `a` from `SINCE − 3_600`, `b` from `day(3,0)`. Samples (`5h`): `a` 50 at `day(1,0)` and 70 at `day(5,0)`; `b` 80 at `day(1,0)` and 20 at `day(4,0)`. The chart is `[IN_USE]` with peaks `[—, 50, —, —, 20, —, —]`. Lane `a`: peaks `[—, 50, —, —, —, 70, —]`, `in_use [t, t, t, f, f, f, f]`. Lane `b`: `[—, 80, —, —, 20, —, —]`, `[f, f, f, t, t, t, t]`.
  - **`a_column_spanning_a_switch_shows_its_peak`.** `b` from `day(2,12)`; `a` 93 at `day(2,11)`, `b` 3 at `day(2,13)`: the chart's column 2 is 93.
  - **`accounts_never_in_use_in_the_range_have_no_lane`.** `c` active at `SINCE − 7_200`, `a` at `SINCE − 3_600`, `c` has samples: the lanes are `[a]`.
  - **`one_account_has_a_chart_per_relevant_window_and_its_in_use_lane`.** `Scope::Account(a)` over the first fixture: charts `5h` and `7d` in that order, `5h`'s peaks `[60, —, …]`; lanes `[a]`. `Scope::Account` of an unknown id is `NoSuchAccount`.
  - **`a_removed_account_in_use_in_the_range_keeps_its_lane`.** `a` in use, then `store.delete_account(&a)`: lane `AccountRef { id: a, position: None, alias: None, email: None, removed: true }`, peaks all `None`, `in_use` as before.
  - **`no_store_is_an_empty_timeline_and_creates_nothing`.** A fresh `Fx`: one `IN_USE` series of seven `None`s, no lane, threshold 90, and no data directory.
  - **`the_threshold_is_the_provider_s_effective_setting`.** `[autoswitch] threshold = 85` gives 85.
  - **`store_tokens.rs`, `active_history_since_is_the_rows_since_and_the_last_one_before`.** Rows `(cc, 100, a)`, `(cc, 200, b)`, `(cc, 300, NULL)`, `(other, 250, x)`, `(cc, 300, a)`: since 250 is `[(200, b), (300, None), (300, a)]`; since 100 is `[(100, a), (200, b), (300, None), (300, a)]`; since 50 the same.
  - **`store/tokens.rs`, `active_history_since_reads_along_its_index`.** Both statements' plans name `active_history_by_at`, with no `SCAN` and no `TEMP B-TREE`.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-engine --locked --test timeline --test store_tokens` and `--lib timeline:: store::tokens::`. Expected: a compile error.
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-engine --locked --features test-hooks`. Expected: all pass.
- [ ] **Step 5: Commit.** `git add crates/tagteam-engine && git commit -m "Read the timeline's usage per column and who was in use"`

### Task 2: Read the timeline for History in place of one account's windows

**Depends on:** Task 1. **R4e:** not needed.

**Files:**
- Rewrite: `src/screens/history.rs` (state, reads, keys, render; the per-account list, `SPARK`, `sparkline`, `window_*` and their tests go). `Range` stays.
- Modify: `src/source.rs` (`Source::timeline`, `EngineSource`, the `Named` test fake), `src/msg.rs`, `src/lanes.rs`, `src/runtime.rs` (`execute`; the five test fakes), `src/state.rs`, `src/update.rs`, `src/keys.rs` (History's rows; the test's `history` state), `src/screens/accounts.rs` (`⏎`), `src/testutil.rs` (`timeline_view`, `lane_of`), `crates/tagteam/src/tui_cmd.rs` (`Refused::timeline`).
- Mechanical: `ViewsReq::History`, `Cmd::LoadHistory`, `Msg::History` and `State::load_history` give way to the timeline's. Their tests in `update.rs`, `daemon/mod.rs` (`names` maps `LoadTimeline` to `"history"`), `actions.rs` and `keys.rs` build a `TimelineView` in place of a `HistoryView`. `crates/tagteam/tests/tui_pty.rs`'s `history` predicate and `tui_history_opens_on_history` look for `Nothing in the last 7 days` in place of `No usage history.`.
- Test: `src/screens/history.rs`, `tests/source.rs`.

**Interfaces:**
- Consumes: Task 1's `Scope`, `TimelineView`, `Engine::timeline`.
- Produces:
  ```rust
  // source.rs
  fn timeline(&self, provider: &ProviderId, scope: &Scope, since_s: i64, columns: usize) -> Result<TimelineView, SourceError>;
  // msg.rs
  ViewsReq::Timeline { r#gen: u64, provider: ProviderId, scope: Scope, since_s: i64, columns: usize },
  Cmd::LoadTimeline { r#gen: u64, provider: ProviderId, scope: Scope, since_s: i64, columns: usize },
  Msg::Timeline { r#gen: u64, result: Result<TimelineView, SourceError> },
  // state.rs (gens.history, coalescing as History did)
  impl State { pub(crate) fn load_timeline(&mut self, provider: ProviderId, scope: Scope, since_s: i64, columns: usize) -> Cmd; }
  // screens/history.rs
  pub struct HistoryState { pub(crate) scope: Scope, range: Range, provider: Option<ProviderId>, view: Option<TimelineView>, resized_ms: Option<i64> }
  pub(crate) const RESIZE_DEBOUNCE_MS: i64 = 250;
  /// T4-10: the chart's columns at `width`.
  pub(crate) fn columns(width: u16) -> usize;
  /// A read for focus's provider, the scope, the range and the width, under a new generation;
  /// `None` before the first accounts read (T4-19).
  pub(crate) fn reload(state: &mut State, now_ms: i64) -> Option<Cmd>;
  pub(crate) fn on_enter(state: &mut State, now_ms: i64) -> Vec<Cmd>;
  pub(crate) fn on_accounts(state: &mut State, now_ms: i64) -> Vec<Cmd>;
  pub(crate) fn on_timeline(state: &mut State, r#gen: u64, result: Result<TimelineView, SourceError>);
  pub(crate) fn on_resize(state: &mut State, now_ms: i64);
  pub(crate) fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd>;
  pub(crate) fn scope_to(state: &mut State, account: AccountId, now_ms: i64) -> Vec<Cmd>; // ⏎ on Accounts
  // testutil.rs
  /// A view over `columns` columns of the 7 days to NOW_S, threshold 90.
  pub(crate) fn timeline_view(columns: usize, charts: Vec<Series>, lanes: Vec<Lane>) -> TimelineView;
  /// `many()`'s account `position` as a lane.
  pub(crate) fn lane_of(position: u32, peaks: Vec<Option<f64>>, in_use: Vec<bool>) -> Lane;
  ```
- **The scope row** (body row 0): ` f {all accounts | short name}   [ ] {range label}`, then from `WIDE` `   1 · 7 · 14 · 30`, and on the right, one column in, `cursor now` (Task 4 adds the cursor's times). `f` and `[ ]` accent, the scope and range text, the choices and `cursor` muted, the time bold.
- **Render** (T4-21): the scope row; then `loading…` before a view; the empty block when nothing is in range; otherwise nothing yet (Task 3 draws the rest).
- **Keys:** `[`/`]` only. The key table loses History's `j k ↑ ↓` (account) and `J K` (windows) rows; `[ ]` stays.

- [ ] **Step 1: Write the tests** (`history.rs`; tier 0 unless named):
  - **`history_reads_the_timeline_for_focus_s_provider_scope_range_and_width`.** On History at 80×24 with `snapshot(many(3))`: the accounts result gives `[Cmd::LoadTimeline { gen, provider: cc(), scope: Scope::All, since_s: NOW_S − 604_800, columns: 70 }]`. `]` gives one with `since_s: NOW_S − 14 × 86_400` and the next generation. A `Msg::Timeline` of the older generation changes nothing; a failed one raises the History banner and keeps the view.
  - **`a_tui_started_on_history_reads_the_timeline_once_the_accounts_arrive`.** `start()` on History is `[LoadAccounts]` alone; the accounts result adds the `LoadTimeline`.
  - **`a_resize_re_reads_the_timeline_250_ms_after_it_settles`.** With a view, `Resize { 120, 40 }` at `NOW_MS`: a tick at `+249` gives nothing, at `+250` one `LoadTimeline` with `columns: 110`. A second resize at `+100` restarts the wait. On Accounts a resize reads nothing.
  - **`the_scope_row_names_the_scope_the_range_and_the_cursor`.** At 80: ` f all accounts   [ ] 7 days`, and `cursor now` ending at column 78. At 120: ` f all accounts   [ ] 7 days   1 · 7 · 14 · 30`. Roles in tier 1: `f` and `[ ]` accent, `1 · 7 · 14 · 30` and `cursor` muted, `now` bold.
  - **`nothing_in_range_says_so_and_how_to_pick_a_longer_one`.** An empty view (no lanes, all `None`): `Nothing in the last 7 days` and `[ ] picks a longer range.` centred under the scope row; at 1 day `Nothing in the last day`; at 30 days no sentence. Before a view: `loading…`.
  - **`enter_on_accounts_opens_history_scoped_to_the_selected_account`** (rewrites `accounts.rs`'s ⏎ test): `⏎` on row 2 gives History with `scope == Scope::Account(id(2))` and a `LoadTimeline` with that scope; the scope row reads ` f 2 a2`.
  - **`tests/source.rs`, `timeline_is_the_engine_s_for_the_scope_and_range`:** two accounts, a sample each: `source.timeline(&cc, &Scope::All, since, 7)` equals `engine.timeline(&cc, Scope::All, since, 7)`.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-tui --locked screens::history`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed, and make the mechanical edits.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-tui --locked`; `cargo test -p tagteam --locked --features test-support --lib tui_cmd::`; outside the sandbox `cargo test -p tagteam --locked --features test-support --test tui_pty`. Expected: all pass.
- [ ] **Step 5: Commit.** `git add -A crates/tagteam-tui crates/tagteam/src/tui_cmd.rs crates/tagteam/tests/tui_pty.rs && git commit -m "Read the timeline for History in place of one account's windows"`

### Task 3: Draw History's charts, swimlanes and day axis

**Depends on:** Task 2. **R4e:** not needed.

**Files:**
- Create: `src/screens/history/layout.rs` (T4-8), `src/screens/history/chart.rs` (T4-9, T4-11), `src/screens/history/lanes.rs` (lanes, the in-use lane, the axis: T4-7, T4-10, T4-12).
- Modify: `src/screens/history.rs` (`mod layout; mod chart; mod lanes;`, `render`), `crates/tagteam-engine/src/words.rs` (`account_name`).
- Test: the three new files; `history.rs`.

**Interfaces:**
- Consumes: Task 2's state and fixtures; `gauge::fill_role`; `ProviderAccounts.models`, `strategy`; `tagteam_core::usage::max_relevant_pct`.
- Produces:
  ```rust
  // words.rs (T4-28)
  pub fn account_name(r: &AccountRef) -> String;
  // layout.rs
  pub(crate) const CHART_X: u16 = 9;
  pub(crate) const LIST_LEAST: u16 = 4; pub(crate) const CHART_LEAST: u16 = 5;
  pub(crate) const CHART_MOST_ALL: u16 = 12; pub(crate) const CHART_MOST_EACH: u16 = 8;
  pub(crate) const LANES_MOST: usize = 10;
  pub(crate) struct Layout {
      pub(crate) scope: Rect,
      /// Each chart shown: its index in `view.charts`, and its title row and rows.
      pub(crate) charts: Vec<(usize, Rect)>,
      pub(crate) events: Rect,
      /// The lanes shown, by index in `view.lanes`, in position order (T4-7, T4-8).
      pub(crate) lanes: Vec<(usize, Rect)>,
      /// The `+N more · f picks one` row, and N.
      pub(crate) more: Option<(Rect, usize)>,
      pub(crate) axis: Rect,
      pub(crate) head: Option<Rect>,
      pub(crate) list: Rect,
  }
  pub(crate) fn layout(view: &TimelineView, scope: &Scope, body: Rect, terminal_rows: u16, chart_from: usize) -> Layout;
  // chart.rs
  pub(crate) fn chart_rows(peaks: &[Option<f64>], rows: u16, line_pct: f64, cursor: usize, theme: Theme) -> Vec<Line<'static>>;
  pub(crate) fn title_row(state: &State, series: &Series, all: bool, width: u16) -> Line<'static>;
  // lanes.rs
  pub(crate) fn lane_row(lane: &Lane, theme: Theme) -> Line<'static>;
  pub(crate) fn in_use_row(lane: &Lane, theme: Theme) -> Line<'static>;
  pub(crate) fn axis_row(grid: Grid, days: i64, tz: &TimeZone, theme: Theme) -> Line<'static>;
  ```
- Until Task 4, `render` passes `chart_from` 0 and draws the cursor in the last column (now).
- **Lane cells:** in use with a value, `▁…█`[`round(min(v, 100) / 100 × 7)`] in `fill_role(v)`; in use without one, blank; not in use, `─` faint. **In-use lane:** `━` accent where in use, `─` faint elsewhere. **The more row:** ` +{n} more · f picks one`, muted.

- [ ] **Step 1: Write the tests:**
  - **`layout.rs`, `the_80_by_24_budget_fits_3_6_and_12_accounts`** (§16, §7.8). Body `Rect(0, 3, 80, 20)`, 24 terminal rows. Rows are body rows:
    - 3 lanes: scope 0, chart 1–10 (title and 9 rows), events 11, lanes 12–14, axis 15, list 16–19;
    - 6 lanes: chart 1–7 (6 rows), events 8, lanes 9–14, axis 15, list 16–19;
    - 12 lanes: chart 1–6 (5 rows), events 7, lanes 8–13 (six), more 14 with `n == 6`, axis 15, list 16–19. The six kept have the most in-use columns.
  - **`layout.rs`, `at_120_by_40_the_chart_takes_12_rows_and_the_list_the_rest`.** Body `Rect(0, 4, 120, 35)`, 40 rows. 3 lanes: scope 0, blank 1, title 2, chart 3–14, events 15, lanes 16–18, axis 19, blank 20, head 21, list 22–34. 12 lanes: lanes 16–25 (ten), more 26 with `n == 2`, axis 27, head 29, list 30–34.
  - **`layout.rs`, `one_account_s_charts_share_the_rows_and_scroll`.** At 80×24 with 2 windows: charts at 1–6 and 7–12 (title and 5 rows each), events 13, in-use lane 14, axis 15, list 16–19. With 4 windows the same two show, and `chart_from == 2` shows windows 3 and 4. At 120×40 with 3: three charts of 7 rows under their titles.
  - **`chart.rs`, `a_chart_stacks_eighths_in_its_value_s_severity_with_the_threshold_and_the_cursor`.** `chart_rows([100, 50, 0, —, 95, 75], 9, 90, 5)`: the top row is `█┈┈┈▄│`, row 4 from the top `█▄  ██`, the bottom `██▁ ██`. Roles (tier 1): columns 0 and 4 crit, 1 and 2 accent, 5 warn; `┈` faint; `│` accent. Tier 0 draws `·` for `┈`.
  - **`chart.rs`, `the_y_axis_labels_100_50_from_6_rows_and_0`.** 9 rows: `    100  ` on the top row, `     50  ` on the fifth, `      0  ` on the bottom; 5 rows: no `50`.
  - **`chart.rs`, `the_in_use_chart_s_title_names_its_windows_and_the_threshold_from_120_columns`.** Account 2 in use at 53 %/6 %, the chart's last column 53. At 120: ` in use   the higher of its 5h and 7d: what auto compares with 90%`, and on the right `at cursor  53%   now  53%`, `in use` bold. At 80: ` in use` and the readouts only. Under `consume-first`: `the higher of its 5h and 7d`. With no account in use, `now    —`.
  - **`lanes.rs`, `a_lane_draws_its_usage_where_in_use_and_a_track_elsewhere`.** Lane 1 (`a1@x.co`), peaks `[40, 95, —, 10]`, `in_use [t, t, t, f]`: ` 1 a1    ▄█ ─`; roles accent, crit, none, faint. The in-use lane of `[t, f, t]` is `━─━`, accent, faint, accent.
  - **`lanes.rs`, `the_axis_marks_each_day_and_now`** (T4-12; 70 columns, UTC). 7 days: `╵Tue` at column 13, `╵Wed` 23, `╵Thu` 33, `╵Fri` 43, `╵Sat` 53, `╵Sun` 63, no `╵Mon`, `now` at 76–78. 1 day: `╵15` at 11, `╵18` 20, `╵21` 28, `╵00` 37, `╵03` 46, `╵06` 55, `╵09` 63, `╵12` 72. 30 days: `╵Aug 24` at 12, `╵Aug 31` 28, `╵Sep 7` 44, `╵Sep 14` 61, no `╵Sep 21`.
  - **`history.rs`, `history_draws_its_chart_lanes_and_axis_in_their_rows`** at 80×24 and 120×40, `snapshot(many(3))` and a view of `lane_of(1..=3)` all in use, for `TIERS`: the rows of the budget test hold the chart's title, its rows, the three lane rows starting ` 1 a1`, ` 2 a2`, ` 3 a3`, and the axis. A lane's name is `words::account_name`'s.
  - **`history.rs`, `a_view_wider_than_the_screen_draws_clipped_until_the_re_read`** (Review Focus 5). A 110-column view drawn at 80×24: no row is wider than 80, the chart starts at column 9, and nothing panics. The tick 250 ms after `Resize { 80, 24 }` asks for `columns: 70`.
  - **`words.rs`, `an_account_is_named_by_position_and_alias_or_by_its_stored_label`:** `1 alex`, `2 work`; removed with `ci@example.com` is `ci@example.com`; removed without one is `an account`.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-tui --locked screens::history`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-tui --locked`; `cargo test -p tagteam-engine --locked --lib words::`. Expected: all pass.
- [ ] **Step 5: Commit.** `git add -A crates/tagteam-tui/src crates/tagteam-engine/src/words.rs && git commit -m "Draw History's charts, swimlanes and day axis"`

### Task 4: Move History's cursor and keep its scope with Accounts

**Depends on:** Task 3. **R4e:** not needed.

**Files:**
- Modify: `src/screens/history.rs` (cursor keys, `f`, `Esc`, `J`/`K`, `follow`, `reload_paced`), `src/overlay/mod.rs` and `src/overlay/choose.rs` (`ChooseThen`), `src/screens/accounts.rs` (`AccountsState::select_id`; a selection change calls `history::follow`; ⏎ marks `from_accounts`), `src/state.rs` (`reload_views_as`), `src/daemon/mod.rs` (T4-19's pacing), `src/keys.rs` (rows and their test states).
- Mechanical: `Overlay::Choose`'s literals and patterns (`actions.rs`'s `on_prompt`, `overlay/mod.rs`'s `decline`, their tests) use `then: ChooseThen::Reply(..)`.
- Test: `history.rs`, `daemon/mod.rs`, `keys.rs`.

**Interfaces:**
- Consumes: Task 3's `layout`, `Grid`.
- Produces:
  ```rust
  // overlay/mod.rs: `Overlay::Choose { question, options, selected, then: ChooseThen }` in place of `reply`
  pub enum ChooseThen { Reply(AnswerTo), Scope(Vec<Scope>) }
  // state.rs
  pub(crate) enum Reread { Now, Paced }
  impl State { pub(crate) fn reload_views_as(&mut self, now_ms: i64, timeline: Reread) -> Vec<Cmd>; } // reload_views = Reread::Now
  // screens/history.rs
  pub(crate) const PACED_MS: i64 = 30_000;
  /// T4-17: the scope follows the Accounts selection while it names an account.
  pub(crate) fn follow(state: &mut State, now_ms: i64) -> Vec<Cmd>;
  /// T4-19: `reload`, unless the last read was asked for under `PACED_MS` ago, in which case
  /// `on_tick` asks for it `PACED_MS` after that one.
  pub(crate) fn reload_paced(state: &mut State, now_ms: i64) -> Option<Cmd>;
  // screens/accounts.rs
  impl AccountsState { pub(crate) fn select_id(&mut self, id: &AccountId) -> bool; }
  ```
- `HistoryState` gains `pub(crate) cursor: Cursor` (`pub(crate) enum Cursor { Now, At(i64) }`, T4-14), `chart_from: usize`, `from_accounts: bool`, `read_ms: Option<i64>`, `paced_due_ms: Option<i64>`. `ChooseThen::Scope`'s `decline` is `None`; `choose::key` takes `&mut State` to apply the scope.
- `DaemonState` gains `reread_timeline: bool`, set by an `Event::Auto` with `changes_an_account()` and no `poll()`, and by a link coming up. The batch's views read uses `Reread::Now` when it is set, else `Reread::Paced`; the 60 s live poll uses `Paced`.
- **Keys** (T4-14, T4-17): `←`/`→`, `Home`/`End`, `f`, `J`/`K` (one account), `Esc`. **Key rows:** `← → Home End` "the cursor by column; the oldest event, now" (footer `←/→` `cursor`), `f` "scope: all accounts or one" (footer `f` `scope`), `J K` "scroll the charts" (no footer). The test's states: for the cursor keys, a view with the cursor one column left of now (after `←`), so each of `←`, `→`, `Home` and `End` moves it; a view for `f`; a 4-window account scope for `J`, scrolled for `K`.

- [ ] **Step 1: Write the tests:**
  - **`the_cursor_moves_by_column_and_end_goes_to_now`.** A 70-column view from `cursor now`: `←` gives `cursor 11:49` (column 68 ends 2h24m before now, less 1 ms); `→` gives `cursor now`; `→` stays; `Home` with no events gives `cursor Sep 14 16:37` (column 0); `←` there stays; `End` gives `cursor now`. The chart's `│` follows the cursor's column.
  - **`f_chooses_all_accounts_or_one_and_the_accounts_selection_follows`.** `f` opens a Choose whose options are `all accounts`, `1 a1@x.co`, `2 a2@x.co`, `3 a3@x.co`, with index 0 selected. `j`, `⏎` gives `Scope::Account(id(1))`, `state.accounts.selected() == id(1)`, one `LoadTimeline` of that scope, and the scope row ` f 1 a1`. `f`, `3` gives `2 a2`. `f`, `⏎` on `all accounts` keeps the Accounts selection.
  - **`esc_after_enter_returns_to_accounts_and_all_accounts`.** `⏎` on row 2, `Esc`: Accounts, `scope == All`; `3` reads all accounts.
  - **`esc_on_a_scoped_history_returns_to_all_accounts`**, and `Esc` on all accounts does nothing.
  - **`while_scoped_the_scope_follows_the_accounts_selection`.** Scoped to 2 by `f`, `1` (Accounts), `j`: the scope is 3. Scoped to all, `j` on Accounts leaves it.
  - **`a_scoped_account_gone_from_the_view_returns_history_to_all_accounts`.** Scoped to 3, an accounts read without account 3: `All` and a `LoadTimeline`.
  - **`shift_j_and_k_scroll_one_account_s_charts`.** 4 windows at 80×24: `J` shows windows 2–3, `J` 3–4, `J` stays; `K` back to 2–3.
  - **`a_scope_on_another_provider_s_account_reads_that_provider`** (Review Focus 4). Claude Code and FakeAgent accounts; `⏎` on a FakeAgent row reads `provider == fake-agent`; its `f` lists FakeAgent's accounts only.
  - **`daemon/mod.rs`, `in_live_mode_usage_re_reads_the_timeline_at_most_every_30_s_and_a_switch_at_once`.** Live, on History, a timeline read asked at `NOW_MS`. A `usage` at `+10_000` gives at `+10_250` `LoadAccounts` alone; a tick at `+30_000` gives the `LoadTimeline`. A `switch` at `+31_000` gives both at `+31_250`. A `changed` and a `poll` pace as `usage` does. In timer mode the 30 s timer reads the timeline at once.
  - **`keys.rs`, `every_key_the_table_lists_is_handled`** passes with the new rows' states.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-tui --locked screens::history daemon::`. Expected: failures.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-tui --locked`; outside the sandbox `cargo test -p tagteam --locked --features test-support --test tui_pty`. Expected: all pass.
- [ ] **Step 5: Commit.** `git add -A crates/tagteam-tui/src && git commit -m "Move History's cursor and keep its scope with Accounts"`

---

## Before Task 5 (controller): the rebase onto R4e

- [ ] **R4e and R4e2 have merged into main.** Rebase `tui-redesign-t4` onto main. The expected conflicts are `crates/tagteam-tui/tests/source.rs` (R4e's `source: EventSource::Cli` beside Task 2's test) and `crates/tagteam/src/tui_cmd.rs` (R4e's `engine_for(.., EventSource::Cli)` beside Task 2's `Refused::timeline`). After it, `cargo test -p tagteam-tui --locked` and `cargo test -p tagteam-engine --locked --features test-hooks` pass.
- [ ] **R4e's final API** (T4-1). Each prints the hit described; a miss is patched into Tasks 5–13 first, and the plan re-reviewed if a task's text changes:
  - `rg -n 'pub enum (EventSource|EventKind)' crates/tagteam-engine/src/event.rs`; `rg -n 'pub use event::\{EventKind, EventSource\}' crates/tagteam-engine/src/lib.rs`
  - `rg -n 'pub source: EventSource|pub fn source\(&self\) -> EventSource' crates/tagteam-engine/src/engine.rs`
  - `rg -n 'pub kind: EventKind|pub source: EventSource|Result<Option<EventRow>>' crates/tagteam-engine/src/store/mod.rs`; R4e's `events_leave_out_a_row_this_build_cannot_type_and_keep_it_stored` is in `crates/tagteam-engine/tests/store.rs`.
  - `rg -n 'fn event\(' -A6 crates/tagteam-engine/src/lifecycle.rs` shows `kind: EventKind` and no source.
  - `rg -n 'fn engine_for' -A5 crates/tagteam/src/app/context.rs` shows `source: EventSource`; `rg -c 'EventSource::Cli' crates/tagteam/src/tui_cmd.rs` is 2.
  - No hit: `git grep -nE 'with_source|source: &(.static )?str|source: "(cli|auto|daemon)"' -- 'crates/*/src/**'`, and no `source` field in `SwitchRequest` (`rg -n 'pub struct SwitchRequest' -A8 crates/tagteam-engine/src/switch/mod.rs`).
  - `rg -n 'fn timeline_event' -A20 crates/tagteam-engine/src/tokens/view/group.rs` has a catch-all arm, so new kinds are ignored there. If it matches `EventKind` exhaustively, Task 5 adds the four kinds to its `None` arm.
  - **R4g:** `rg -n 'fn perform|fn try_target' crates/tagteam-engine/src/auto.rs`. With `try_target` present, Tasks 6 and 7's `perform` edits go in `try_target`'s arms of the same names. With R4g still open, the controller tells the refactor session that Tasks 6–7 edit `perform`.

### Task 5: Store four new event kinds and read events by range and kind

**Depends on:** none (after the rebase). **R4e and R4e2:** `EventKind`, the typed `EventRow`, `event_from_row`'s `Option`.

**Files:**
- Modify: `crates/tagteam-engine/src/event.rs` (four variants and `AUTO_LINE_KINDS`, T4-22; the spelling test gains their four rows), `crates/tagteam-engine/src/store/mod.rs` (three reads, T4-26).
- Test: `crates/tagteam-engine/tests/store.rs`.

**Interfaces:**
- Produces:
  ```rust
  // event.rs
  /// §5.3's auto-line kinds, in its order; `add`, `import` and `remove` are not.
  pub const AUTO_LINE_KINDS: [EventKind; 8] = [Switch, SwitchRecovered, SwitchFailed, AllExhausted, AutoswitchPaused, AutoswitchResumed, Quarantine, Unquarantine];
  impl Store {
      /// §12.3: the provider's events at or after `since_ms`, by `(at, rowid)`, leaving out a row this build cannot type.
      pub fn events_since(&self, provider: &ProviderId, since_ms: i64) -> Result<Vec<EventRow>, StoreError>;
      /// The provider's newest event of one of `kinds` with `since_ms <= at < before_ms` (either bound optional), by `(at, rowid)`, skipping rows this build cannot type.
      pub fn latest_event(&self, provider: &ProviderId, kinds: &[EventKind], since_ms: Option<i64>, before_ms: Option<i64>) -> Result<Option<EventRow>, StoreError>;
      /// C-12: the `label` of the latest `remove` event whose `from_id` is `id`.
      pub fn removed_label(&self, provider: &ProviderId, id: &AccountId) -> Result<Option<String>, StoreError>;
  }
  ```

- [ ] **Step 1: Write the tests** (raw rows `(at, provider, kind, from_id, to_id, source, detail)`):
  - **`events_since_is_the_provider_s_rows_from_its_bound_in_order`.** Rows at 99, 100, 100 (inserted second), 150, and the other provider's at 120: `events_since(cc, 100)` is the rows at 100, 100 and 150, in insertion order for the tie.
  - **`the_latest_event_of_some_kinds_is_found_within_its_bounds`.** `switch` 100, `quarantine` 200, `autoswitch-paused` 300, `add` 400. `[Switch, Quarantine]` is the quarantine; with `before 200` the switch; with `since 250` none. `[AutoswitchPaused, AutoswitchResumed]` is the pause.
  - **`the_latest_event_skips_a_row_this_build_cannot_type`** (Review Focus 1): a `switch` with source `menubar` at 500 leaves `[Switch]` at the switch of 100, and the row stays stored.
  - **`a_removed_account_s_label_is_read_from_its_latest_remove_event`.** `remove` rows for `x` at 100 (`{"label":"old@x.co"}`) and 200 (`{"label":"ci@example.com"}`) give `ci@example.com`. None, or a detail without a label, gives `None`.
  - **`event.rs`, `every_event_kind_and_source_keeps_its_stored_spelling`** gains `switch-failed`, `all-exhausted`, `autoswitch-paused` and `autoswitch-resumed`.
  - **`four_new_kinds_need_no_migration`.** Each new kind inserted through `insert_event` reads back through `events()`. `schema_version()` and `PRAGMA user_version` are the same before and after, and a store with none of them reads as before.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-engine --locked --test store`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-engine --locked --features test-hooks`. Expected: all pass, `tokens_view.rs` unchanged.
- [ ] **Step 5: Commit.** `git add crates/tagteam-engine && git commit -m "Store four new event kinds and read events by range and kind"`

### Task 6: Record the window behind an automatic switch and a removed account's label

**Depends on:** none (after the rebase). **R4e and R4e2:** `Engine::event`'s typed kind, `SwitchRequest` without a source, `transact.rs`'s `switch` row under `self.source()`.

**Files:**
- Modify: `crates/tagteam-engine/src/switch/mod.rs` (`AutoPerform.cause`, `SwitchCause`), `crates/tagteam-engine/src/switch/transact.rs` (the `switch` row's `detail`), `crates/tagteam-engine/src/auto.rs` (`perform`'s `AutoPerform` fills `cause`, T4-23), `crates/tagteam-engine/src/store/mod.rs` (`delete_account_logged`), `crates/tagteam-engine/src/lifecycle.rs` (`event`'s `detail`; `remove_locked` calls `delete_account_logged` in place of `delete_account` then `event`), `crates/tagteam-engine/src/purge.rs` (`remove_unregistered`, the same); the other `event` callers pass `None`.
- Mechanical: `crates/tagteam-engine/tests/switch.rs`'s `AutoPerform` literal gains `cause: None`.
- Test: `crates/tagteam-engine/tests/{auto_tick,switch,manage,purge,store}.rs`.

**Interfaces:**
- Produces:
  ```rust
  /// The window that triggered an automatic switch (redesign §12.4), its `switch` event's `detail`.
  #[derive(Debug, Clone, PartialEq)]
  pub struct SwitchCause { pub window: String, pub label: String, pub pct: f64, pub threshold: f64 }
  impl SwitchCause { pub fn detail(&self) -> serde_json::Value; } // {"window","label","pct","threshold"}
  pub struct AutoPerform { /* … */ pub cause: Option<SwitchCause> }
  // lifecycle.rs
  pub(crate) fn event(&self, provider: &ProviderId, kind: EventKind, from: Option<&AccountId>, to: Option<&AccountId>, detail: Option<Value>) -> Result<(), EngineError>;
  // store/mod.rs (T4-27)
  /// `delete_account`, with `event` inserted through `insert_event_on` in the same transaction.
  pub fn delete_account_logged(&self, id: &AccountId, event: &EventRow) -> Result<(), StoreError>;
  ```
- `perform`'s cause is `binding_window` over the live account's snapshot windows under `cfg.models`, with `cfg.threshold`.

- [ ] **Step 1: Write the tests:**
  - **`auto_tick.rs`, `an_automatic_switch_records_the_window_that_triggered_it`.** As `a_proactive_tick_switches_and_records_its_departure` (b live at 20/95): the last event's `detail` is `{"window":"7d","label":"7d","pct":95.0,"threshold":90.0}` (FakeAgent: `monthly`).
  - **`switch.rs`, `a_manual_switch_records_no_detail`:** `fx.switch_to(&a, false)`'s `switch` row has `detail == None`.
  - **`manage.rs`, `remove_records_the_account_s_label`:** `fx.add("a@x.co", "rt-a")`, then `remove`. The last event is `Remove`, `from_id == a`, `detail == {"label":"a@x.co"}`.
  - **`purge.rs`, `a_purged_account_s_remove_event_keeps_its_label`:** over `unregistered_account`, the `remove` row's detail is `{"label": <the row's email, else label>}`, read before the purge.
  - **`store.rs`, `an_account_is_never_deleted_without_its_remove_event`:** with a test trigger `CREATE TRIGGER no_events BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'refused'); END` (raw SQL on `tagteam.db`), `delete_account_logged` fails and the account's `accounts` row and its cascaded `usage_state`, `usage_samples` and `mappings` rows are all still there; without the trigger the account and its rows go and the `remove` event is stored, in one commit.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-engine --locked --features test-hooks --test auto_tick --test manage`. Expected: failures.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-engine --locked --features test-hooks` and `cargo test -p tagteam --locked --features test-support --test auto_cli`. Expected: all pass, R4e's `auto_records_its_switch_as_auto_and_a_later_switch_as_cli` unchanged.
- [ ] **Step 5: Commit.** `git add crates/tagteam-engine && git commit -m "Record the window behind an automatic switch and a removed account's label"`

### Task 7: Record failed switches and when every account is exhausted

**Depends on:** Task 5. **R4e and R4e2:** `Engine::source()`, typed `EventRow`, `AutoEngine` without a source. **R4g:** see Before Task 5.

**Files:**
- Modify: `crates/tagteam-engine/src/auto.rs`: `perform`'s `Err(e)` arm, `announce`, and the tick's return paths (T4-24). `AutoEngine` gains `exhausted: Option<bool>`, `None` before its first decision; every decision sets it, a switch's `TickOutcome::Switched` return included.
- Test: `crates/tagteam-engine/tests/auto_tick.rs` (`--features test-hooks`).

**Interfaces:**
- Consumes: Task 5's `latest_event` and `AUTO_LINE_KINDS`; `fail_at` (test hooks).
- `switch-failed`: `self.engine.event(provider, SwitchFailed, Some(live), Some(target), Some(json!({"message", "transient"})))`, under the dedupe. `all-exhausted`: `event(provider, AllExhausted, Some(live), None, Some(json!({"earliest_reset_at": earliest_reset})))`. A store error writing either is fatal, as every store failure in a tick is.

- [ ] **Step 1: Write the tests:**
  - **`a_failed_switch_is_recorded_once_per_target_and_message_for_30_minutes`.** `Fx`: `a`, then `b` live; `a` read at 10/20, `b` at 20/95; `fx.engine.fail_at(Some("after-journal"))`.
    - Tick 1 is `TickOutcome::Error`, and the last event is `SwitchFailed`, `from b`, `to a`, source `auto`, detail `{"message": EngineError::InvalidInput("injected failure at after-journal".into()).to_string(), "transient": false}`.
    - Tick 2 adds none.
    - `clock.advance_ms(30 × 60_000 + 1_000)`, both read again, then tick 3 adds a second.
    - `fail_at(Some("after-credential"))`, then tick 4 adds a third.
  - **`every_account_exhausted_is_recorded_on_entry_only`.** As `every_candidate_exhausted_is_blocked_until_the_earliest_recovery`.
    - Two ticks give one `AllExhausted`: `from b`, `to None`, source `auto`, detail `{"earliest_reset_at": SHORT_RESETS}`.
    - `b` read at 50/40 and a tick (below threshold), then `b` at 100/40 and a tick, give a second.
  - **`an_exhaustion_after_a_switch_is_a_new_episode`.** Exhausted (one `AllExhausted`); `a` recovers and the next tick switches to it (`TickOutcome::Switched`); every account exhausted again: a second `AllExhausted`. Without the switch's reset the second is missed.
  - **`a_fresh_engine_records_an_exhaustion_the_store_already_has_once`** (Review Focus 3). After the first case's first event, a new `auto()` engine's exhausted tick adds none. With every reset moved one hour later, it adds one.
  - **`a_dry_run_records_no_failed_switch_and_no_exhaustion`:** both cases under `auto(.., true)` leave `events()` as before.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-engine --locked --features test-hooks --test auto_tick`. Expected: failures.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-engine --locked --features test-hooks`, `cargo test -p tagteam-daemon --locked --features test-hooks` and `cargo test -p tagteam --locked --features test-support --test auto_cli --test daemon_cli`. Expected: all pass; the daemon's and `auto`'s event streams unchanged.
- [ ] **Step 5: Commit.** `git add crates/tagteam-engine && git commit -m "Record failed switches and when every account is exhausted"`

### Task 8: Record when the daemon pauses and resumes auto

**Depends on:** Task 5. **R4e and R4e2:** the daemon's engine source (`Daemon`); `evaluate` without `with_source`.

**Files:**
- Modify: `crates/tagteam-engine/src/auto.rs` (`Engine::record_autoswitch`), `crates/tagteam-daemon/src/provider_job.rs` (`settings_changed`, and the first `run` after its warnings; `ProviderJob` gains `started: bool`).
- Test: `provider_job.rs`'s test module; `crates/tagteam-engine/tests/auto_engine.rs`.

**Interfaces:**
- Produces:
  ```rust
  impl Engine {
      /// Redesign §12.4, T4-25: `autoswitch-resumed` (`on`) or `autoswitch-paused` under this
      /// engine's source. `at_start` writes only when `on` differs from the provider's latest
      /// stored `autoswitch-*` (none reads as resumed), with `{"at_start": true}`. Nothing
      /// without an existing store. Whether it wrote.
      pub fn record_autoswitch(&self, provider: &ProviderId, on: bool, at_start: bool) -> Result<bool, EngineError>;
  }
  ```

- [ ] **Step 1: Write the tests:**
  - **`auto_engine.rs`, `a_pause_is_recorded_at_start_only_when_it_differs_from_the_store`.** No store gives `Ok(false)`, and none is created. With one: `(on false, at_start)` writes a pause with `{"at_start": true}` and returns `true`; again, `false`; `(true, at_start)` writes a resume; `(true, false)` always writes, with no detail.
  - **`provider_job.rs`, `a_reload_that_pauses_or_resumes_records_it_once_as_the_daemon_s`.** After a first run with `autoswitch` true:
    - `[daemon]\nautoswitch = false\n` adds one `AutoswitchPaused` (source `daemon`, no `from`/`to`, no detail);
    - a reload changing only `[autoswitch] threshold` adds none;
    - `autoswitch = true` adds an `AutoswitchResumed`.
  - **`provider_job.rs`, `the_first_run_records_a_pause_the_store_does_not_have`.**
    - Built paused with none stored: the first run adds a pause with `{"at_start": true}`, and a second job's first run adds none.
    - Stored paused, built with `autoswitch` true: one resume with `at_start`.
    - None stored, built true: none.
  - **`provider_job.rs`, `a_newer_store_records_no_pause`:** with `schema_newer`, a paused build and a reload write nothing.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-daemon --locked --features test-hooks --lib provider_job::`. Expected: failures.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-daemon --locked --features test-hooks`, `cargo test -p tagteam-engine --locked --features test-hooks --test auto_engine`; outside the sandbox `cargo test -p tagteam --locked --features test-support --test daemon_procs`. Expected: all pass; `daemon status --json` unchanged.
- [ ] **Step 5: Commit.** `git add crates/tagteam-engine crates/tagteam-daemon && git commit -m "Record when the daemon pauses and resumes auto"`

### Task 9: Put auto's events on the timeline and read the auto line's last event

**Depends on:** Tasks 1, 5 (`AUTO_LINE_KINDS`, the store's reads). **R4e and R4e2:** `EventKind`, `EventSource`, typed rows.

**Files:**
- Modify: `crates/tagteam-engine/src/timeline.rs`: `EventView`, the view's `events` and `paused_at_start`, `AccountRef` resolution with `removed_label`, `before_pct`/`after_pct`, `Engine::last_auto_event`. `crates/tagteam-engine/src/store/usage.rs`: `usage_sample_before`, and its plan pin in the test module.
- Mechanical: the TUI's `testutil::timeline_view` and any `TimelineView` literal gain `events: vec![]`, `paused_at_start: false`.
- Test: `crates/tagteam-engine/tests/timeline.rs`.

**Interfaces:**
- Produces:
  ```rust
  #[derive(Debug, Clone, PartialEq)]
  pub struct EventView {
      pub at_ms: i64, pub kind: EventKind, pub source: EventSource, pub trigger: Option<String>,
      pub from: Option<AccountRef>, pub to: Option<AccountRef>, pub detail: Option<serde_json::Value>,
      /// For `switch` and `switch-recovered`: the binding usage just before and just after (below).
      pub before_pct: Option<f64>, pub after_pct: Option<f64>,
  }
  pub struct TimelineView { /* … */ pub events: Vec<EventView>, pub paused_at_start: bool }
  impl Engine {
      /// §12.2: the newest of the provider's `AUTO_LINE_KINDS` at or after `since_ms`, its accounts named as they read now; `None` without a store.
      pub fn last_auto_event(&self, provider: &ProviderId, since_ms: i64) -> Result<Option<EventView>, EngineError>;
  }
  // store/usage.rs
  /// The window's latest sample at or before `at_s`: `(fetched_at, pct)`, one seek on the
  /// `(account_id, window, fetched_at)` key.
  pub fn usage_sample_before(&self, id: &AccountId, window: &str, at_s: i64) -> Result<Option<(i64, f64)>, StoreError>;
  ```
- **Events** are `events_since`'s, oldest first. One account keeps those whose `from` or `to` is it, and every `autoswitch-*`. **`paused_at_start`:** the latest `autoswitch-*` before `since_ms` is a pause. **Before/after:** before is the highest relevant `pct` of `from`'s latest reading at or before the switch, and after is `to`'s earliest at or after it. A reading is the samples sharing one `fetched_at`. The range's samples come from Task 1's reads; when `from`'s latest reading at or before the switch is older than `since_s`, it comes from `Store::usage_sample_before`, one bounded read per relevant window (never the account's whole history). `None` without one.

- [ ] **Step 1: Write the tests** (Task 1's fixture):
  - **`the_timeline_lists_the_scope_s_events_in_the_range_oldest_first`.** Raw events:
    - `switch a→b` (`auto`, `proactive`) at `SINCE − 1` and again at `day(0,1)`;
    - `quarantine c` (`{"reason":"invalid_grant"}`) at `day(2,0)`;
    - `autoswitch-paused` at `day(4,0)`, `autoswitch-resumed` at `day(5,0)`.

    All accounts gives the last four, in order, with kinds, sources, triggers and details as stored, `from` `AccountRef { id: a, position: Some(1), alias: None, email: Some("alex@example.com"), removed: false }`. `Account(c)` gives the quarantine, pause and resume; `Account(a)` the switch, pause and resume.
  - **`a_pause_that_began_before_the_range_draws_from_its_first_column`.** A pause at `SINCE − 3_600` gives `paused_at_start` and no event; adding a resume at `SINCE − 60` clears it.
  - **`a_switch_carries_the_usage_just_before_and_just_after_it`.** `b` from `day(2,12)`, switch `a→b` at `day(2,12)`.
    - Readings: `a` at `day(2,10)` 5h 91 / 7d 40, and at `day(2,11)` 93 / 41; `b` at `day(2,13)` 3 / 2.
    - The switch has `before 93`, `after 3`. A quarantine has both `None`, and so does a switch from an account with no reading before it.
    - **At the range's edge:** a switch one minute after `SINCE`, whose `from` was last read one minute before `SINCE` at 5h 91, has `before 91` (read through `usage_sample_before`).
  - **`store/usage.rs`, `the_sample_before_an_instant_reads_one_row_along_the_key`.** Samples at 100, 200 and 300: `usage_sample_before(.., 250)` is `(200, pct)`, `(.., 99)` is `None`; its plan names the table's primary key, with no `SCAN` and no `TEMP B-TREE`.
  - **`a_removed_account_is_named_by_the_label_its_remove_event_stored`** (Review Focus 2). A switch `a→c` at `day(1,0)`; `c` removed at `day(2,0)` with `{"label":"alex@personal.example"}` and `store.delete_account(&c)`. The switch's `to`, and `c`'s lane, are `AccountRef { id: c, position: None, alias: None, email: Some("alex@personal.example"), removed: true }`. A `remove` without detail gives `email: None`.
  - **`rows_a_newer_build_wrote_are_left_out_of_the_timeline_and_the_auto_line`** (Review Focus 1). In range, `('x-new-kind', 'cli')` at `T0 − 60 s` and `('switch', 'menubar')` at `T0 − 120 s`, after a valid `quarantine` at `T0 − 1 h`. Neither is in `events`, and `last_auto_event` is the quarantine.
  - **`last_auto_event_is_the_newest_auto_line_kind_in_its_bound`.**
    - Each of `AUTO_LINE_KINDS` alone at `T0 − 60 s` is returned; `add`, `import` and `remove` alone give `None`.
    - A switch at `T0 − 31 days`, with `since T0 − 30 days`, gives `None`.
    - An `add` after a switch leaves the switch (§16); two at the same `at` give the later row.
  - **`last_auto_event_names_its_accounts_as_they_read_now`:** after `set_alias(b, "work")` and a move, `to` has the new alias and position.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-engine --locked --test timeline`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-engine --locked --features test-hooks`, `cargo test -p tagteam-tui --locked`. Expected: all pass.
- [ ] **Step 5: Commit.** `git add crates/tagteam-engine crates/tagteam-tui/src && git commit -m "Put auto's events on the timeline and read the auto line's last event"`

### Task 10: Say each stored event in plain words

**Depends on:** Task 9. **R4e and R4e2:** `EventKind::ALL`, `EventSource`.

**Files:**
- Modify: `crates/tagteam-engine/src/words.rs`.

**Interfaces:**
- Produces: `pub fn event(e: &EventView, frees_at: Option<&str>) -> String` (§11.4, T4-29). It uses `account_name`, `short_name`, `trigger`, `no_switch(AllExhausted, ..)`, `NEEDS_LOGIN`, and the store's `CREDENTIALS_REPLACED`/`ACCOUNT_REPLACED`.

- [ ] **Step 1: Write the test** `every_event_kind_has_its_sentence`.
  - It iterates `EventKind::ALL`, and a kind without a row fails it.
  - `1 alex` is `alex@example.com`, `2 work` alias `work`, `3 spare` alias `spare`, `4 ci` is `ci@example.com`.

  | Event | Sentence |
  |---|---|
  | `switch` `auto` `proactive`, detail `{window 5h, label 5h, pct 93.4, threshold 90}`, 1→2 | `auto switched 1 alex → 2 work · alex's 5h reached 93%` |
  | the same by `daemon`, `at-limit` | `auto switched 1 alex → 2 work · alex's 5h reached 93%` |
  | `auto` `consume-first` with that detail | `auto switched 1 alex → 2 work (to use up the account that resets first)` |
  | `auto` `proactive`, no detail | `auto switched 1 alex → 2 work (before it reached the limit)` |
  | `auto`, trigger `added-later` | `auto switched 1 alex → 2 work` |
  | `cli` `manual` 2→3; with no `from` | `you switched 2 work → 3 spare`; `you switched to 3 spare` |
  | `switch-recovered` to 2 | `tagteam finished an interrupted switch to 2 work` |
  | `switch-failed` to 3, `{"message":"the Keychain was locked"}`; no message | `auto couldn't switch to 3 spare · the Keychain was locked`; `auto couldn't switch to 3 spare` |
  | `all-exhausted`, `frees_at` `16:40`; `None` | `auto couldn't switch · every account is near its limit; the first frees up at 16:40`; `auto couldn't switch · every account is near its limit` |
  | `autoswitch-paused` / `-resumed` | `auto paused` / `auto resumed` |
  | `quarantine` 3, `{"reason":"invalid_grant"}`; none | `3 spare needs a new login · its stored login was refused (invalid_grant)`; `3 spare needs a new login` |
  | `unquarantine` 3: `credentials-replaced`, `account-replaced`, other | `3 spare works again · new login stored`, `3 spare works again · account replaced`, `3 spare works again` |
  | `add` / `import` 4 | `4 ci added` / `4 ci imported` |
  | `remove`, `from` removed with `ci@example.com`; without | `ci@example.com removed`; `an account removed` |
  | `switch` `auto` 1→ removed `c` labelled `alex@personal.example` | `auto switched 1 alex → alex@personal.example (before it reached the limit)` |

  No sentence contains `claude` in any case (Global Constraints).
- [ ] **Step 2: Run it to see it fail.** `cargo test -p tagteam-engine --locked --lib words::`. Expected: a compile error.
- [ ] **Step 3: Implement** `event`.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-engine --locked --lib`. Expected: all pass.
- [ ] **Step 5: Commit.** `git add crates/tagteam-engine/src/words.rs && git commit -m "Say each stored event in plain words"`

### Task 11: Show auto's events on History's lane and in its list

**Depends on:** Tasks 4, 9, 10. **R4e and R4e2:** `EventKind`, `EventSource`.

**Files:**
- Create: `src/screens/history/marks.rs` (T4-13's table), `src/screens/history/list.rs` (T4-20).
- Modify: `src/screens/history.rs`: the selection, `j`/`k`/`↑`/`↓`, `Home` on events, `z`, placement (T4-15), the switch readout. `src/screens/history/layout.rs` (`full_list`), `src/screens/history/lanes.rs` (`events_row`), `src/screens/history/chart.rs` (before → after), `src/theme.rs` (`marked`), `src/keys.rs` (`j k ↑ ↓` "previous or next event", footer `j/k` `event`; `z` "the event list on the whole body", footer `z` `list`; their test states).
- Test: the new files, `history.rs`.

**Interfaces:**
- Consumes: Task 9's `EventView`, `AUTO_LINE_KINDS`, `paused_at_start`; Task 10's `words::event`; `autoline::when`; `daemon::words::clock` for `frees_at`.
- Produces:
  ```rust
  // marks.rs
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub(crate) struct Mark { pub(crate) glyph: &'static str, pub(crate) role: Role, pub(crate) rank: u8 }
  pub(crate) fn mark(e: &EventView) -> Mark;
  // lanes.rs
  pub(crate) fn events_row(view: &TimelineView, cursor: usize, theme: Theme) -> Line<'static>;
  // list.rs
  /// The list's rows in `rows` rows, newest first, the selected one kept in view (T4-20).
  pub(crate) fn list_rows(state: &State, view: &TimelineView, rows: u16, width: u16, now_ms: i64) -> Vec<Line<'static>>;
  // theme.rs
  /// The cursor's event glyph (§7.5): `role`, bold and REVERSED.
  pub(crate) fn marked(self, role: Role) -> Style;
  ```
- `HistoryState` gains `selected: Option<usize>` (into the newest-first list), `list_from: usize`, `full_list: bool` and `place: Option<Place>`, where `enum Place { Open, Event { at_ms: i64, kind: EventKind }, Now }`.

- [ ] **Step 1: Write the tests:**
  - **`marks.rs`, `each_event_kind_has_its_glyph_role_and_rank`**, over `EventKind::ALL`, with `switch` once per source. `switch-failed`, `all-exhausted`, `quarantine`: `✕` crit 6. `autoswitch-paused`: `‖` warn 5. `switch` by `auto`/`daemon`: `⇄` accent 4. `switch` by `cli`, and `switch-recovered`: `⇄` text 3. `autoswitch-resumed`: `‖` muted 2. `unquarantine` `✓` ok, `add`/`import` `+` muted, `remove` `-` muted: 1.
  - **`lanes.rs`, `the_events_lane_shows_each_column_s_most_severe_event_and_a_pause_s_span`.** Ten columns.
    - Events: `switch` (auto) and `quarantine` in column 2, pause in 5, resume in 8, `add` in 9. With the cursor at column 0, the row after the gutter is `│ ✕  ‖┄┄‖+`, `┄` in warn and `│` accent.
    - With `paused_at_start`, only a resume in column 3 and the cursor on it: `┄┄┄‖`, the `‖` bold and REVERSED.
    - Tier 0 draws `·` for `┄`. With the cursor on column 2, its `✕` is bold and REVERSED.
  - **`list.rs`, `the_list_is_newest_first_with_its_time_glyph_and_sentence`.** Events at `NOW − 2h45m` (a switch 1→2 with detail), `Sat 11:33` (unquarantine 3), `Sat 08:58` (quarantine 3). At 80×24 the list's rows are:
    - `▌  11:28       ⇄ auto switched 1 a1 → 2 a2 · a1's 5h reached 93%`
    - `   Sat 11:33   ✓ 3 a3 works again · new login stored`
    - `   Sat 08:58   ✕ 3 a3 needs a new login · its stored login was refused (invali…` (cut by `fit` at 80 columns)

    Times muted, glyphs in their roles. The selected row's sentence is bold, and in tier 2 the row has `sel-bg`. At 120×40 the head is `   WHEN        WHAT`.
  - **`list.rs`, `the_list_overflows_into_a_more_line_and_z_gives_it_the_body`.** Twelve events at 80×24: three rows and `⋮ 9 more · z full list`, muted. `z` shows the charts' rows as list rows, its last `⋮ N more · z charts`; `z` again restores. `j` to row 4 scrolls the list.
  - **`history.rs`, `j_k_home_and_end_move_the_selection_and_the_cursor_together`.** Placed on the newest: `j` selects row 2, and the scope row says `cursor Sat 11:33`; `k` back; `Home` selects the oldest, `End` the newest at `cursor now`.
  - **`history.rs`, `moving_the_cursor_by_column_selects_the_nearest_event_at_or_before_it`.** From the newest event, `←` to a column between two events selects the older; `←` to before the oldest selects none.
  - **`history.rs`, `on_open_the_cursor_is_on_the_newest_auto_line_event`.** An `add` newer than a switch: the cursor is on the switch. With no auto-line kind: `cursor now`. A refresh keeps a cursor moved by hand.
  - **`history.rs`, `a_switch_at_the_cursor_reads_before_and_after_on_the_in_use_chart`.** `before 93`, `after 3`: `at cursor  93% →   3%   now  53%`. One column `←` reads `at cursor` and that column's peak.
  - **`history.rs`, `a_removed_account_s_lane_and_events_draw_with_its_label`** (Review Focus 2). Its lane's gutter is ` alex@pe `; its switch row ends `→ alex@personal.example (before it reached the limit)`.
  - **`history.rs`, `the_80_by_24_budget_with_events`:** with 3 lanes and 12 events, rows 16–19 are three events and the more line; the events lane is row 11.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-tui --locked screens::history`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-tui --locked`; outside the sandbox `cargo test -p tagteam --locked --features test-support --test tui_pty`. Expected: all pass.
- [ ] **Step 5: Commit.** `git add -A crates/tagteam-tui/src && git commit -m "Show auto's events on History's lane and in its list"`

### Task 12: Fill the auto line from the store and open its event with w

**Depends on:** Tasks 9, 10, 11. **R4e and R4e2:** `EventKind`, `EventSource`.

**Files:**
- Modify: `src/source.rs` (`AccountsSnapshot.last_auto`; `EngineSource::accounts` reads it per provider), `src/autoline.rs` (T4-30: ranks 1, 3, 4 from the store; the `status.lastSwitch` path and `switched` go; `Said.pin`), `src/update.rs` (`w`), `src/screens/history.rs` (`why`, T4-16), `src/keys.rs` (row `w` "History at auto's last action", everywhere, no footer; its state), `src/overlay/help.rs` (T4-31).
- Mechanical: every `AccountsSnapshot` literal gains `last_auto: vec![]` (`rg -n 'AccountsSnapshot \{' crates`); `autoline.rs`'s and `chrome.rs`'s T2 tests of `lastSwitch` and of timer mode's empty line (M-7) are rewritten to a stored event.
- Test: `autoline.rs`, `history.rs`, `help.rs`, `tests/source.rs`; `crates/tagteam/tests/perf.rs`.

**Interfaces:**
- Consumes: Task 9's `last_auto_event`; Task 10's `event`; Task 11's `mark`, `Place`.
- Produces:
  ```rust
  // source.rs
  pub struct AccountsSnapshot { /* … */ /// Each listed provider's `last_auto_event` of the last 30 days (§5.3, §12.2).
      pub last_auto: Vec<(ProviderId, EventView)> }
  // autoline.rs
  pub(crate) struct Said { /* … */ /// Ranks 1 and 3: the event `w` opens (T4-15).
      pub(crate) pin: Option<(i64, EventKind)> }
  // screens/history.rs
  /// `w` (C-4): History at the auto line's event, or at now.
  pub(crate) fn why(state: &mut State, now_ms: i64) -> Vec<Cmd>;
  ```

- [ ] **Step 1: Write the tests:**
  - **`tests/source.rs`, `accounts_carries_each_provider_s_last_auto_line_event`.** A stored switch and a later `add`: `snap.last_auto == [(cc, <the switch's EventView>)]`. Without a store, `[]`.
  - **`autoline.rs`, `a_stored_event_of_the_last_30_minutes_leads_the_line`.** In both modes, a stored switch 1→2 (`auto`, `proactive`, 5h 93) at `14:11:20`: ` ⇄ 14:11  auto switched 1 a1 → 2 a2 · a1's 5h reached 93%`, `⇄` accent. In live mode a later `no-switch` and a `poll` leave it (UX-3).
  - **`autoline.rs`, `a_decision_shows_without_a_recent_stored_event`** (rank 2). A stored switch at 12:13 and, in live mode, a `no-switch` `cooldown` `4m` at 14:13: `   14:13  waiting 4m after the last switch`. In timer mode the same store gives rank 3, ` ⇄ last switch 12:13 · auto switched …`.
  - **`autoline.rs`, `an_older_event_reads_as_the_last_switch_with_its_day`.** A switch on Sat 15:38: ` ⇄ last switch Sat 15:38 · auto switched …`, the lead muted. A quarantine then: ` ✕ Sat 08:58  3 a3 needs a new login · …`, `✕` crit.
  - **`autoline.rs`, `timer_mode_says_there_were_no_switches_when_the_store_has_none`** (M-7 lifted). With a snapshot and an empty `last_auto`: ` no switches in the last 30 days`, muted, in timer and live mode. Before the first snapshot the row is empty.
  - **`autoline.rs`, `the_line_follows_the_selected_account_s_provider`** (rewritten): FakeAgent's stored event shows with a FakeAgent account selected.
  - **`history.rs`, `w_opens_history_at_the_auto_line_s_event_widening_the_range`.**
    - Ten days old: `w` on Accounts gives History, all accounts, 14 days, and the event's row selected once its view arrives.
    - Two days old at 7 days: the range stays.
    - Rank 2 or 4: `cursor now`. From History scoped to an account, `w` is all accounts.
  - **`help.rs`, `the_marks_list_history_s_lane_glyphs`:** the Marks section has `┄ paused meanwhile` (warn), `+ added`, `- removed` (muted), `┈ the threshold` (faint).
  - **`perf.rs`, `tui_first_frame_p95_is_within_50_ms_with_a_year_of_events`** (`#[ignore]`, as its neighbours): `nothing_due`'s home with 20,000 raw `events` rows over 365 days. The p95 is within 50 ms.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-tui --locked autoline:: screens::history`. Expected: failures.
- [ ] **Step 3: Implement** as listed, and make the mechanical edits.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-tui --locked`; outside the sandbox `cargo test -p tagteam --locked --features test-support --test tui_pty` and `cargo test --release -p tagteam --features test-support --test perf -- --ignored tui_first_frame --nocapture`. Expected: all pass; every p95 within 50 ms.
- [ ] **Step 5: Commit.** `git add -A crates/tagteam-tui crates/tagteam/tests/perf.rs && git commit -m "Fill the auto line from the store and open its event with w"`

### Task 13: Record the timeline and the new event kinds in the specs and the changelog

**Depends on:** Tasks 1–12.

**Files:**
- Modify: `CHANGELOG.md`, under `## [Unreleased]`.
  - `### Added`: `` The event history records failed switches, when every account is exhausted, and when the daemon pauses or resumes auto; an automatic switch records the window that triggered it, and a removed account its label. ``
  - `### Changed`:
    - `` `tagteam tui`'s History is a timeline: usage charts with auto's switches, pauses and failures on the same time axis, a lane per account, and an event list that follows a time cursor. `f` picks one account, and `⏎` on Accounts opens its history. ``
    - `` `tagteam tui`'s auto line reads auto's last action from the store, also without the daemon, and `w` opens History at it. ``
- Modify: `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`. Core §5's `CREATE TABLE events` comment lists `'switch' | 'switch-recovered' | 'switch-failed' | 'all-exhausted' | 'autoswitch-paused' | 'autoswitch-resumed' | 'quarantine' | 'unquarantine' | 'add' | 'import' | 'remove'`. A line under the block names each new kind's writer and `detail`. *(Amended by the TUI redesign spec §12.4.)*
- Modify: `docs/superpowers/specs/2026-10-14-tagteam-daemon-design.md`. D4.2's settings bullet gains: a reload that changes a provider's `daemon.autoswitch`, and a start whose setting differs from the provider's latest stored pause or resume, records `autoswitch-paused` or `autoswitch-resumed`. *(Amended by the TUI redesign spec §12.4.)*
- Modify: `docs/superpowers/specs/2026-10-14-tagteam-tui-design.md`, each hunk ending *(Amended by the TUI redesign spec §n.)*:
  - §4.4's table gains `timeline(provider, scope, since, columns)` → `Engine::timeline` (§12.3);
  - §5.4's mock-up and bullets give way to the timeline (§7);
  - §5.6 drops "without `w`, and with History keeping its account keys … until T4" (§10);
  - §8's "What each event does": History's timeline re-reads per §7.7, and the auto line comes from the store, the daemon's `no-switch`, `error` and `config-warning` events feeding rank 2 (§5.3).

- [ ] **Step 1: Edit** the four files as listed.
- [ ] **Step 2: Check the format.** `cargo fmt --all --check`. Expected: clean.
- [ ] **Step 3: Commit.** `git add CHANGELOG.md docs/superpowers/specs && git commit -m "Record the timeline and the new event kinds in the specs and the changelog"`

---

## After Task 13 (controller)

- [ ] **The gallery check** into `gallery/t4-after-13`.
- [ ] **No dependency changed:** `git diff <base> -- Cargo.toml Cargo.lock 'crates/*/Cargo.toml'` prints nothing.
- [ ] **No schema change:** `git diff <base> -- crates/tagteam-engine/src/store/schema.sql crates/tagteam-engine/src/store/schema_v3.sql` prints nothing.
- [ ] **No protocol change:** `git diff <base> -- crates/tagteam-daemon/src/protocol.rs` prints nothing.
- [ ] **No provider name in neutral source:** `git diff <base> -- crates/tagteam-tui/src crates/tagteam-engine/src crates/tagteam-daemon/src | rg '^\+' | rg -i 'claude|anthropic|tagteam_cc'` hits test modules or `testutil.rs` only.
- [ ] **No commit over the slice size,** or it was split.
- [ ] **CI in full,** totals as passed/total; the pty suites outside the sandbox; the first frame (`perf`'s `tui_first_frame` cases, every p95 within 50 ms); Linux in Docker (non-root with a passwd entry, HOME outside `/tmp`, the `docker` call unpiped).
- [ ] **Cross-review:** `xreview round --reset`; a pre-merge review of `origin/main...tui-redesign-t4`, in slices under the inline cap until Michael raises `XREVIEW_MAX_DIFF_BYTES`: Tasks 1–2, 3–4, 5–8, 9–10, 11, 12–13, then fix commits. Fix and re-review until it approves.
- [ ] **Push and hand over.** Push `tui-redesign-t4`, run CI on it (`gh workflow run CI --ref tui-redesign-t4`), and send the coordinator the ready message: head, base, CI, Codex coverage, and the macOS and Linux totals. The coordinator opens the Draft PR against `main` with the body the controller wrote from `.github/pull_request_template.md`, its "Rulings" listing T4-1 to T4-32 and every ruling made while executing.
- [ ] **The status commit.** Once the PR exists: in one commit, set this plan's status to `Implemented — <PR URL>` and the redesign spec's `In progress` to cite it. Give that commit its own Codex slice, push it, and send the coordinator the new head and the slice's nonce.

## Self-review

- **Coverage.** Every §15 T4 item has a task: `Engine::timeline` (1, 9), the new readers (1, 5), `last_auto_event` (9), the writers (6, 7, 8), §11.4 (10), the store-fed auto line (12), the screen (2, 3, 4, 11). VIS-5 (content from the top, §7.8's budget) is in 3. VIS-12 (the range said once, its key in the scope row and footer) is in 2. UX-3 (the line never overwritten by a poll; the store behind it) is in 12. UX-11 (scope sync) is in 4. §16's History items: the 80×24 budget with 3, 6 and 12 accounts (3); cursor and list in sync (11); `w` widening (12); scope sync (4); `timeline`'s buckets, lanes, before/after, scopes and a pause before the range (1, 9); the reads' boundaries and plan pins (1, 5; §16's EXPLAIN pin of the samples read is hardening's two pins of the only two statements the timeline uses, and Task 1 pins `active_history_since`); `last_auto_event`'s kinds and bound (9); every writer (6–8); a sentence per kind over `ALL` (10).
- **Steps.** Each code step names its file, signature and the spec's values; bodies appear nowhere. The algorithms the tests leave open (grid, layout, chart cells, axis, pacing, dedupe) are stated once, in rulings T4-4 to T4-24.
- **Types.** `Scope`, `Grid`, `AccountRef`, `Series`, `Lane`, `TimelineView` (Task 1, widened in 9), `EventView` (9), `AUTO_LINE_KINDS` (5), `Cursor` (4), `Place` (11), `ChooseThen`, `Reread` (4), `SwitchCause` (6) and `Mark` (11) keep their names across tasks. `Source::timeline` takes `&Scope` and the engine `Scope` by value.
- **Proportion.** About 740 lines, against T3's 988, for a slice of about T3's size in code (two engine views, four writers, a rewritten screen). Code blocks are signatures only; the long parts are the rulings and the exact test values.
