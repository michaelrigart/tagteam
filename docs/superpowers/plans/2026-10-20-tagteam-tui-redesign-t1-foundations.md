# tagteam TUI redesign T1 — foundations — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Implemented — https://github.com/michaelrigart/tagteam/pull/28
**Goal:** Close the TUI clean-ups the refactor held for the redesign that need nothing new (CC-O-8, -9, -10, -16, -18, -19, -23c), fix PERF-R-9 and PERF-R-10b, and route every style through a theme module whose tier 1 is today's look, with nothing on screen changing.
**Architecture:** The runtime keeps everything that is not state: prompt senders, by id; the shutdown and a refused hand-over reach `State` only as messages. Overlays move into `overlay/`, one module per kind, with their keys handled in place. The input thread blocks on the terminal, SIGWINCH and a waker instead of polling. Tokens lays its table out once a second. `theme.rs` becomes the only code that names a colour or a modifier.
**Tech Stack:** Rust 1.88 (edition 2024), ratatui 0.30.2, crossterm 0.29.0, mio 1 and signal-hook-mio 0.2 (both already built for crossterm).
**Spec:** docs/superpowers/specs/2026-10-20-tagteam-tui-redesign-design.md (§4.1, §13, §14, §15's T1 row)

**Reading this plan.**
- Paths are under `crates/tagteam-tui/` unless they start with `crates/`, `docs/` or a file at the root.
- Line numbers are as of `683b266`, the branch base. Hardening touches no file under `crates/tagteam-tui/src`, so they hold after the rebase too, until tasks move them. Find each site by the code quoted.
- Focused tests: `cargo test -p tagteam-tui --locked <filter>` (the crate has no test feature), and `cargo test -p tagteam --locked --features test-support --test <file>` for the binary's tests.
- The pty tests (`tui_pty.rs`) open a pseudo-terminal: run them outside the sandbox.
- Each task leaves the workspace compiling, with `tagteam-tui`'s tests green.

## Global Constraints

- **No visible change.** Every screen, overlay, footer, toast, banner and header draws the same cells with the same styles, with colour and without. The one exception is timing: the input thread no longer wakes while idle (Task 12). No existing render or text test is loosened. The controller's gallery comparison (below) checks it cell by cell.
- **No CLI, JSON, protocol or store change.** The binary's `src/` is not edited. Under `crates/tagteam/` only `tests/tui_args.rs` (Task 14), `tests/tui_pty.rs` and `tests/common/pty.rs` (Task 12) change.
- **Files this plan may touch:**
  - `crates/tagteam-tui/**`;
  - the three binary test files above;
  - the workspace `Cargo.toml` and `Cargo.lock` (Task 12: two dependency entries, no version change);
  - `CHANGELOG.md`;
  - the TUI spec and the redesign spec (Task 16);
  - this plan.
- **Never committed:**
  - the review harness: `src/ux_concept.rs`, `src/ux_gallery.rs`, `src/ux_redesign.rs`, and their three `lib.rs` lines;
  - any render it writes (`.txt`, `.svg`, `.png`).

  The harness is parked outside the tree whenever a task runs. Before every commit, the controller checks that `git status --short` lists no `ux_` file.
- **Example names only.** Fixtures use example names (`alex@example.com`, `sam@corp.example`), never a real email or organisation.
- **Moves are separate from edits.** Tasks 7 and 9 only move code: bodies stay byte-identical apart from `use` lines, visibility words and the signature a moved match arm needs, so `git diff --color-moved` shows them as moved. No behaviour change goes into a move task.
- **No silenced lints.** No `#[allow(dead_code)]`, `#[allow(unused…)]` or `#[allow(clippy::…)]` is added. Dead code is deleted, or `#[cfg(test)]` when only tests use it.
- **No test is weakened.** A test edit is one of these:
  - a mechanical change to an API this plan changes, listed in its task;
  - a test moved verbatim with the code it tests (Task 7);
  - a new test.

  The timing bounds in `input.rs`'s tests stay as they are or get tighter.
- **CI commands** (`CONTRIBUTING.md`):
  - `cargo fmt --all --check`;
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`;
  - `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`.
- **Commits.**
  - Imperative and in sentence case, with no prefix and no trailing period, matching `git log` (for example "Show the index's warnings under an empty token report").
  - Each task's message is given in its last step.
  - No agent attribution: no `Co-authored-by:` agent line, no session link, no "Generated with" footer.

## Rulings

Calls inside the spec, for the PR's "Rulings" section.

| # | Ruling | Why |
|---|---|---|
| R-1 | The one key table (§9.2, part of CC-O-9) lands in T2, with the help and footer it generates | Today's help and footer disagree in labels, order and content. A table that reproduced both byte for byte would be thrown away by T2's §10 rewrite. T1 moves the overlays and fixes their key handling |
| R-2 | `highlight` is merged into one `view::highlight`, not removed (CC-O-16) | Today's reverse-video selection relies on its padding to reach the full width. T2 removes it when the selection becomes `▌` and bold (§4.3) |
| R-3 | `KeyOutcome` has a fourth case, `KeepWith(Vec<Cmd>)` | Doctor's `o` keeps its panel open while it starts the online run (`actions.rs:470-483`) |
| R-4 | The input thread waits through mio (`Poll`, `Waker`, signal-hook-mio's `Signals`), not a direct `poll(2)` (PERF-R-9) | mio is what crossterm itself uses to wait on the terminal on both platforms. It needs no `unsafe` under the crate's `#![forbid(unsafe_code)]`, and both crates are already built for crossterm with the features this needs |
| R-5 | Tokens' layout cache is keyed by the report, the width, and the clock the active block's figures show: elapsed and left seconds, and whether the block has ended. It is built on first use, not keyed by the second or built eagerly in `on_report` and on resize (PERF-R-10b) | A per-second key would show a countdown up to a second late. A draw at a second's first millisecond would cache `1m left`, and today that text becomes `<1m left` one millisecond later. The figures' own clock reproduces today's text exactly, and a report with no active block is laid out once. The next draw builds the cache once either way. A key's area (`state.size`) and a draw's (`frame.area()`) can differ, so an eager build would need a width it doesn't have |
| R-8 | The input thread reports a resize from its own SIGWINCH registration, and drops crossterm's `Resize` events (PERF-R-9) | The two handlers run one after the other. The thread can wake on its own before crossterm's pipe is written, so waiting for crossterm's event could leave an idle resize unseen. Reading the size after its own signal never misses one |
| R-6 | The daemon lane keeps its own `Lane::spawn`; `spawn_lane` serves the four engine lanes (CC-O-10) | The daemon lane has no `Source` and is never joined (TUI §4.3) |
| R-7 | T1's theme has only the roles today's screens use (warn, crit), plus `strong` and `selection`. T2 adds the other seven and tier 2 | Roles nobody draws would be dead code under `-D warnings` |

## Review Focus

These are the five inputs or failure modes the spec implies that no existing test exercises, most likely first. Each one's test goes in the task that owns the code.

1. **Several keys in one read.** A paste, key repeat or a fast typist delivers several keys in one read. crossterm parses them into a queue the wait cannot see. A thread that blocks again before draining that queue leaves keys stuck until the next byte. Test: Task 12, `a_burst_of_keys_in_one_write_is_handled_whole` (`tui_pty.rs`).
2. **A resize while idle.** Today the 50 ms poll picks up crossterm's `Resize` by chance. A blocking wait must be woken by SIGWINCH itself, even when it wakes before crossterm has seen the signal (R-8). Tests: Task 12, `a_resize_is_sent_from_the_outer_signal_even_before_crossterm_has_it` (`input.rs`) and `a_resize_while_idle_redraws_at_the_new_size` (`tui_pty.rs`).
3. **A terminal that hangs up.** A closed pty makes the terminal readable forever, and crossterm's read loops forever on it. The thread must notice the hang-up before every crossterm call, not only in its wait. Tests: Task 12, `a_closed_terminal_stops_the_thread_wherever_it_shows` and `a_closed_terminal_is_seen_without_blocking` (`input.rs`).
4. **A CLI prompt arriving while a prompt dialog is open.** Today the replaced dialog's sender drops, which declines it. With ids, a forgotten id would leave its lane waiting until the shutdown. Test: Task 6, `a_second_prompt_declines_the_dialog_it_replaces` (`actions.rs`).
5. **A SIGTERM or a fatal error while a CLI prompt waits.** The spec requires an explicit decline once the drop no longer does it (§13, CC-O-8). Test: Task 6, `a_shutdown_declines_a_waiting_prompt_at_once` (`runtime.rs`).

## Spec coverage

| Spec | Tasks |
|---|---|
| §4.1 `theme.rs`, the only place that names a `Color` or a `Modifier` (tier 1 = today) | 1 |
| §13 CC-O-8: `Banner`, `reload_views`, `ShutdownBegan`/`SuspendRefused`, `AnswerTo` + `Cmd::Answer` | 3, 4, 5, 6 |
| §13 CC-O-9: `overlay/` modules, `KeyOutcome`, `&mut Overlay`, no `now_ms` (key table: R-1) | 7, 8 |
| §13 CC-O-10: `spawn_lane`, `Lanes` in `lanes.rs` | 9, 10 |
| §13 CC-O-16: one `keep_visible`, one `highlight` (R-2) | 2 |
| §13 CC-O-18: `pub(crate)` except `host`, `source`, `live` | 15 |
| §13 CC-O-19: `Action::SwitchBy { strategy: UsageStrategy }` | 14 |
| §13 CC-O-23c: `StatusError` through `Msg::DaemonStatus` | 11 |
| §14 PERF-R-9: no idle wake-ups, a pause acknowledged within 50 ms | 12 |
| §14 PERF-R-10b: Tokens' layout cached by report, width and the figures' clock (R-5) | 13 |
| §16 tests for the above; §18's TUI §4.3 hunk; CHANGELOG | 1–16 |

Out of T1, by §15: D-2, CC-O-2/3 and §12.5 go in the first PR planned after R3 merges. ARCH-1's wording goes in T3, PERF-R-10a in T3, and PERF-D-2 in T5.

**Order.** Every task builds and tests the same crate, so tasks never run in parallel: one implementer at a time, in the order below. The `Depends on` lines name the hard dependencies.

---

## Before Task 1 (controller)

- [ ] **Rebase.** Once hardening has merged: `git -C /Users/michael/Code/tagteam-tui-redesign fetch origin`, then `git -C /Users/michael/Code/tagteam-tui-redesign rebase origin/main`. The branch holds only the spec commit and this plan.
- [ ] **Park the harness outside the tree.**
  - Move `crates/tagteam-tui/src/ux_concept.rs`, `ux_gallery.rs` and `ux_redesign.rs` to `.superpowers/tui-redesign/harness/` (gitignored).
  - Save the three `lib.rs` lines there as `lib.patch`, then `git checkout crates/tagteam-tui/src/lib.rs`.
  - Expected: `git status --short` prints nothing.
- [ ] **Render the baseline.**
  - Copy the harness back in and apply `lib.patch`.
  - Run `UX_GALLERY_DIR=$PWD/.superpowers/tui-redesign/gallery/base cargo test -p tagteam-tui ux_gallery -- --nocapture`, then the same into `gallery/base2`.
  - `diff -r --exclude='*.png' gallery/base gallery/base2` must print nothing, which shows the renders are deterministic.
  - Park the harness again.
- [ ] **The gallery check**, run after Tasks 1, 2, 8 and 13 and after Task 16:
  - copy the harness in;
  - fix only its own compile errors against the new APIs (it is never committed);
  - render into `gallery/after-<task>`;
  - `diff -r --exclude='*.png' gallery/base gallery/after-<task>`, which must print nothing;
  - park the harness again.

  A difference is a regression in that task: send it back to the task's implementer.

---

### Task 1: Draw every style through one theme module

**Depends on:** none

**Files:**
- Create: `src/theme.rs`.
- Modify:
  - `src/lib.rs`: add `mod theme;` after `mod term;`.
  - `src/state.rs`: the field `pub color: bool` (`:48`) becomes `pub theme: Theme`, and `State::new`'s `color: bool` parameter becomes `theme: Theme`.
  - `src/runtime.rs:487`: pass `Theme::new(opts.color)`. `TuiOptions.color` stays a `bool`.
  - `src/testutil.rs:177`: `State::new(screen, Theme::new(false), None, tokens, NOW_MS)`.
  - `src/view.rs`: delete `bold`, `selected` and `severity_style` (`:469-486`) and drop `Color, Modifier` from the import at `:5`. The view tests import `ratatui::style::{Color, Modifier}` themselves.
- Modify, at every caller of those three helpers and every `color: bool` parameter or field, which becomes `theme: Theme`:
  - `view.rs:258, 281, 289, 352, 357`;
  - `actions.rs:647, 654, 671, 721, 740`;
  - `screens/accounts.rs`: `Table::new`, `cells`, `window_cell`, `row`, `window_blocks`, `window_lines`, `detail`, and the reads at `:575, 592, 608`. The literal `false` passed to `window_blocks` in `window_heights` (`:654`) becomes `Theme::new(false)`: heights do not depend on colour;
  - `screens/history.rs:253, 279, 290, 376`;
  - `screens/tokens/table.rs`: the field `Table.color` (`:292`) and `:517, 537, 630, 645, 890, 1066, 1132, 1178`;
  - `screens/tokens/filter.rs:281, 315, 325`.
- Modify tests, mechanically:
  - `state.color = c` becomes `state.theme = Theme::new(c)`, at `accounts.rs:1445-1452`, `history.rs:842`, `view.rs:639` and `table.rs:1551`;
  - `window_lines(…, false)` (`accounts.rs:1252`) becomes `Theme::new(false)`.

**Interfaces:**
- Produces, in `crate::theme`:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub(crate) enum Role { Warn, Crit }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub(crate) struct Theme { tier: Tier } // Tier { NoColor, Ansi }, private to the module
  impl Theme {
      pub(crate) fn new(color: bool) -> Theme;                // true: Ansi
      pub(crate) fn role(self, role: Role) -> Style;
      pub(crate) fn severity(self, severity: Severity) -> Style; // Normal: Style::new()
      pub(crate) fn strong(self) -> Style;
      pub(crate) fn selection(self) -> Style;
  }
  ```
  The values are today's:

  | | Ansi | NoColor |
  |---|---|---|
  | `role(Warn)` | `fg(Color::Yellow)` | `BOLD` |
  | `role(Crit)` | `fg(Color::Red)` | `BOLD` |
  | `strong()` | `BOLD` | `BOLD` |
  | `selection()` | `REVERSED` | `REVERSED` |

  `severity` maps `Warning` to `Warn` and `Critical` to `Crit`. `Severity` is `tagteam_engine::words::Severity`.
- Later tasks use `Theme` as `Copy`, through `state.theme` or a `theme: Theme` parameter.

- [ ] **Step 1: Write the failing tests** in `src/theme.rs`'s test module.
  ```rust
  #[test]
  fn tier_1_and_no_colour_draw_today_s_styles() {
      let (ansi, plain) = (Theme::new(true), Theme::new(false));
      let bold = Style::new().add_modifier(Modifier::BOLD);
      assert_eq!(ansi.severity(Severity::Normal), Style::new());
      assert_eq!(ansi.severity(Severity::Warning), Style::new().fg(Color::Yellow));
      assert_eq!(ansi.severity(Severity::Critical), Style::new().fg(Color::Red));
      assert_eq!(plain.severity(Severity::Normal), Style::new());
      assert_eq!(plain.severity(Severity::Warning), bold);
      assert_eq!(plain.severity(Severity::Critical), bold);
      for theme in [ansi, plain] {
          assert_eq!(theme.strong(), bold);
          assert_eq!(theme.selection(), Style::new().add_modifier(Modifier::REVERSED));
      }
  }
  ```
  `no_module_but_the_theme_names_a_colour_or_a_modifier`:
  - walk `concat!(env!("CARGO_MANIFEST_DIR"), "/src")` recursively for `.rs` files, skipping `theme.rs` and `testutil.rs`;
  - keep each file's text up to its first line whose trimmed text is `#[cfg(test)]`;
  - flag every line whose trimmed text does not start with `//` and that contains `Color`, `Modifier` or `Stylize` as a whole word, meaning the character on either side is not alphanumeric or `_`;
  - `assert!(flagged.is_empty(), "{flagged:#?}")`, with entries `path:line: text`.

  `KeyModifiers` does not match, since `Key` and `s` touch the word.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked theme`. Expected: a compile error, since `theme` does not exist yet.
- [ ] **Step 3: Implement `Theme`, then move every call site onto it** as listed in Files.
- [ ] **Step 4: Run the crate's tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass, including the colour tests (`accounts.rs:1445`, `history.rs:842`, `view.rs:638`, `table.rs:1551`), which are unchanged apart from the mechanical edits.
- [ ] **Step 5: Lint.** Run `cargo clippy -p tagteam-tui --all-targets --locked -- -D warnings`. Expected: clean.
- [ ] **Step 6: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Draw every style through one theme module"`.

### Task 2: Keep one keep_visible and one selection highlight

**Depends on:** Task 1

**Files:**
- Modify: `src/view.rs`. Add `keep_visible`, whose body is `screens/tokens/table.rs:789-801`'s, and `highlight`, whose body is `screens/accounts.rs:614-620`'s with `selected()` replaced by `theme.selection()`.
- Modify: `src/screens/accounts.rs`. Delete `keep_visible` (`:235-246`) and `highlight` (`:614-620`). The calls at `:179` and `:586` become `keep_visible(offset, at, at, rows, lines)`, and `:594` becomes `highlight(row, width, theme)`.
- Modify: `src/screens/tokens/table.rs`. Delete `keep_visible` (`:789-801`) and `highlight` (`:1127-1133`), and import both from `crate::view`. The no-selection clamp at `:1187` stays as it is.
- Test: `src/view.rs`.

**Interfaces:**
- Produces:
  - `pub(crate) fn keep_visible(offset: usize, first: usize, last: usize, rows: usize, lines: usize) -> usize`. With `first == last == at` it is exactly accounts' old function, for every input.
  - `pub(crate) fn highlight(row: Line<'static>, width: usize, theme: Theme) -> Line<'static>`.

- [ ] **Step 1: Write the test** `keep_visible_shows_the_whole_item_and_moves_as_little_as_it_can` in `view.rs`:
  ```rust
  assert_eq!(keep_visible(0, 5, 5, 3, 10), 3);
  assert_eq!(keep_visible(4, 2, 2, 3, 10), 2);
  assert_eq!(keep_visible(0, 9, 9, 3, 10), 7);
  assert_eq!(keep_visible(0, 4, 8, 3, 20), 4, "a tall item keeps its first line");
  assert_eq!(keep_visible(8, 9, 9, 3, 10), 7, "never past the last full page");
  ```
- [ ] **Step 2: Run it to see it fail.** Run `cargo test -p tagteam-tui --locked keep_visible_shows`. Expected: a compile error, since `view::keep_visible` does not exist yet.
- [ ] **Step 3: Move the helpers** as listed in Files.
- [ ] **Step 4: Run the crate's tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass. These pin the behaviour:
  - `the_selected_row_is_reverse_video_across_the_width` (`accounts.rs:1466`);
  - `a_long_report_scrolls_to_keep_the_selection_in_view_and_a_resize_keeps_it_there` (`table.rs:1937`).
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Keep one keep_visible and one selection highlight"`.

### Task 3: Keep the banner's text and kind together

**Depends on:** none

**Files:**
- Modify: `src/state.rs`. `banner: Option<String>` and `banner_from` (`:57-60`) become `pub(crate) banner: Option<Banner>`, and `BannerKind` gains `Refusal`.
- Modify: `src/update.rs`. Delete `raise` and `clear_read_error` (`:156-169`). Their callers (`:35, 44, 57, 59, 71, 90`) use the new methods, and `LaneDied`'s check (`:121`) becomes `state.banner.as_ref().map(|b| b.kind) != Some(BannerKind::Lane)`.
- Modify: `src/runtime.rs:749-750`. `self.state.raise_banner(BannerKind::Refusal, format!("{refused}: {NOT_PAUSED}"))`. Task 5 moves this into `update`.
- Modify: `src/view.rs:195-204`. `banner()` reads `state.banner_text()`.
- Modify tests, mechanically:
  - `state.banner = Some(x)` becomes `state.raise_banner(BannerKind::Refusal, x)`, at `view.rs:592, 606, 642`, `accounts.rs:1132, 1181, 1266, 1733`, `history.rs:665` and `table.rs:2155, 2292, 2360`;
  - `.banner.as_deref()` becomes `.banner_text()`;
  - `banner == None` becomes `banner.is_none()`.
- Modify: `runtime.rs:2147-2158`. The test raises a `Collect` banner where it set `banner`/`banner_from`, and its `banner_from == None` assert becomes `ui.state.banner.as_ref().map(|b| b.kind) == Some(BannerKind::Refusal)`.

**Interfaces:**
- Produces, in `crate::state`:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub(crate) struct Banner { pub(crate) text: String, pub(crate) kind: BannerKind }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub(crate) enum BannerKind { Accounts, History, Collect, Lane, Refusal }
  impl State {
      pub(crate) fn raise_banner(&mut self, kind: BannerKind, text: impl Into<String>);
      /// A read of `kind` succeeded: clears a banner of that kind, or a refusal, which no read
      /// owns. Never while quitting (today's `clear_read_error`).
      pub(crate) fn clear_banner(&mut self, kind: BannerKind);
      pub(crate) fn banner_text(&self) -> Option<&str>;
  }
  ```
- `Refusal`'s doc comment: "A hand-over the input thread did not allow (spec §7.3): any successful read clears it."

- [ ] **Step 1: Write the test** `a_banner_clears_for_its_own_kind_or_a_refusal_and_never_while_quitting` in `state.rs`, on `testutil::fresh(Screen::Accounts)`:
  1. Raise `Collect`. `clear_banner(Accounts)` leaves it; `clear_banner(Collect)` clears it.
  2. Raise `Refusal`. `clear_banner(History)` clears it.
  3. Raise `Accounts` and set `quitting = Some(0)`. `clear_banner(Accounts)` leaves it.
- [ ] **Step 2: Run it to see it fail.** Run `cargo test -p tagteam-tui --locked a_banner_clears`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed in Files.
- [ ] **Step 4: Run the crate's tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass, including `a_read_clears_only_the_banner_its_own_kind_raised` (`update.rs:745`) and `a_second_lane_death_keeps_the_first_ones_words` (`:845`).
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Keep the banner's text and kind together"`.

### Task 4: Re-read the views through one State::reload_views

**Depends on:** none

**Files:**
- Modify: `src/state.rs`. Add `reload_views`.
- Modify the five copies, keeping each one's guard and the commands it adds after the reload:
  - `update.rs:79-84`;
  - `update.rs:271-276` (it extends `cmds`);
  - `actions.rs:207-210` (the collect of every shown id follows);
  - `actions.rs:627-631`;
  - `daemon/mod.rs:441-445` (it extends `cmds`; the Tokens `requery` under `all` stays after it).
- Test: `src/screens/history.rs`, since its tests can set the history account.

**Interfaces:**
- Produces `impl State { pub(crate) fn reload_views(&mut self, now_ms: i64) -> Vec<Cmd> }`. It returns `self.load_accounts()`, followed by `screens::history::reload(self, now_ms)` when `self.screen == Screen::History`. Each reload takes exactly one generation of each kind.

- [ ] **Step 1: Write the test** `reload_views_reads_the_accounts_and_on_history_the_account_shown` in `history.rs`'s tests:
  - on Accounts it gives `[LoadAccounts { gen: g + 1 }]`;
  - on History showing an account it gives `[LoadAccounts { gen: g + 1 }, LoadHistory { gen: h + 1, account, .. }]`;
  - on History with no account yet it gives `[LoadAccounts { gen: g + 1 }]`.
- [ ] **Step 2: Run it to see it fail.** Run `cargo test -p tagteam-tui --locked reload_views_reads`. Expected: a compile error.
- [ ] **Step 3: Implement it, and replace the five copies.**
- [ ] **Step 4: Run the crate's tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass, unchanged. These pin the order and generations:
  - `daemon/mod.rs`'s `names()` asserts (`:597, 668, 697, 723, 737`);
  - `update.rs:532, 541-552, 590`;
  - `actions.rs:1426, 1862`.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Re-read the views through one State::reload_views"`.

### Task 5: Tell update when the shutdown begins and when a hand-over is refused

**Depends on:** Task 3

**Files:**
- Modify: `src/msg.rs`. Add the two `Msg` variants.
- Modify: `src/update.rs`. Handle them.
- Modify: `src/runtime.rs`, in three places:
  - **`begin_shutdown` (`:705-725`).** `self.state.quitting = Some(code); self.state.dirty = true;` becomes `self.apply(Msg::ShutdownBegan { code })`, after `self.lanes.close()` and before `self.ending` is set, so the message is applied.
  - **`paused` (`:744-753`).** It becomes `fn paused(&mut self, action: Option<&Action>) -> bool`. On a refusal it calls `self.apply(Msg::SuspendRefused { action: action.cloned(), why: NOT_PAUSED })`.
  - **The two callers.** `suspend` (`:757-760`) calls `paused(Some(&action))` and no longer clears `running` itself. `stop` (`:814`) calls `paused(None)`.

**Interfaces:**
- Produces, in `Msg`:
  ```rust
  /// The runtime began the shutdown (spec §10.2 step 1); it exits with `code`.
  ShutdownBegan { code: i32 },
  /// The input thread did not pause, so the terminal was not handed over (§7.3): `action` did
  /// not start, or, for `None`, Ctrl-Z did not suspend.
  SuspendRefused { action: Option<Action>, why: &'static str },
  ```
- `update`, for `ShutdownBegan`: sets `quitting = Some(code)` and `dirty`, and returns no commands. It leaves the overlay as it is: on a signal the terminal is handed back without another frame, as today.
- `update`, for `SuspendRefused`:
  - raises `Refusal` with `"{name} did not start: {why}"` (`name` is `action.name()`), or `"Ctrl-Z did not suspend: {why}"` for `None`;
  - clears `running` only for `Some`;
  - sets `dirty`, and returns no commands.

- [ ] **Step 1: Write the tests** in `update.rs`:
  - `the_runtime_s_shutdown_sets_the_exit_code_and_leaves_the_dialog`: with `Overlay::Help` open, `ShutdownBegan { code: 143 }` returns `vec![]`, `quitting == Some(143)`, and the overlay is still open;
  - `a_refused_hand_over_raises_a_refusal_and_frees_only_its_own_action`:
    - with `running = Some(run)`, `SuspendRefused { action: Some(run.clone()), why: "w" }` gives `banner_text() == Some(format!("{} did not start: w", run.name()))`, kind `Refusal`, and `running == None`;
    - with `running = Some(switch)`, `SuspendRefused { action: None, why: "w" }` gives `Some("Ctrl-Z did not suspend: w")`, and `running` stays `Some(switch)`.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked -- the_runtime_s_shutdown a_refused_hand_over`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed in Files.
- [ ] **Step 4: Check the runtime's writes.** Run `rg -n 'self\.state\.[a-z_.]+ =' crates/tagteam-tui/src/runtime.rs`. Expected: outside the test module only `dirty` lines are left. The runtime still calls `State::start` and `daemon.await_daemon` while it builds the state.
- [ ] **Step 5: Run the crate's tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass, including:
  - `a_suspension_is_refused_while_the_keys_do_not_pause` (`runtime.rs:1861`);
  - `ctrl_z_is_refused_while_the_keys_do_not_pause` (`:1890`);
  - `a_refusal_banner_is_no_lane_death_and_the_next_good_read_clears_it` (`:2140`).
- [ ] **Step 6: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Tell update when the shutdown begins and when a hand-over is refused"`.

### Task 6: Answer CLI prompts by id and keep their senders out of State

**Depends on:** none

**Files:**
- Modify: `src/msg.rs`, in three places:
  - **`AnswerTo`**, which is new;
  - **`Prompt` (`:183-190`)**, whose `reply: Sender<Answer>` becomes `to: AnswerTo`;
  - **`Cmd::Answer`**, which is new.
- Modify: `src/ask.rs`. Add `Answers`, the senders still waiting, shared by the runtime and the actions lane. `ChannelAsk` keeps its sender there and sends only the id. On every way out of `ask`, it forgets the id.
- Modify: `src/actions.rs`, in three places:
  - **The overlay fields.** `Overlay::Choose`'s `reply` and `ConfirmThen::Reply` hold an `AnswerTo`.
  - **The answering arms (`:375-378`, `:501-510`).** They return `vec![Cmd::Answer { to, answer }]` where they used to send.
  - **`on_prompt` (`:550-573`).** It returns `Vec<Cmd>`:
    - while quitting, it opens nothing and declines;
    - when it replaces an open dialog that is bound to a prompt, it declines that dialog's prompt first.
- Modify: `src/update.rs:110-114`. It returns `on_prompt`'s commands. `quit`'s doc comment (`:171`) now says that the runtime declines the waiting prompt when the shutdown begins.
- Modify: `src/runtime.rs`, in three places:
  - **The state.** `Ui` holds an `Answers`, `Lanes::spawn` takes `answers: &Answers`, and the actions lane builds `ChannelAsk::new(out.clone(), shutdown.clone(), answers.clone())`.
  - **`execute`.** It runs `Cmd::Answer { to, answer } => self.answers.answer(to, answer)`.
  - **`begin_shutdown`.** It calls `self.answers.decline_all()` right after `self.shutdown.request(signal)`.
- Modify tests, mechanically:
  - `ask.rs`'s three other tests answer through `answers.answer(p.to, …)`;
  - `view.rs:674` builds `ConfirmThen::Reply(AnswerTo(1))` and asserts `y` returns `[Cmd::Answer { to: AnswerTo(1), answer: Answer::Confirm(true) }]`;
  - `actions.rs:1705, 1753, 1806` use an `AnswerTo` in place of the unused sender.

**Interfaces:**
- Produces:
  ```rust
  // msg.rs
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct AnswerTo(pub u64);
  pub struct Prompt { pub question: String, pub kind: PromptKind, pub to: AnswerTo }
  // in Cmd:
  /// Answers the CLI prompt `to` names (spec §7.2); the runtime holds its sender.
  Answer { to: AnswerTo, answer: Answer },

  // ask.rs
  #[derive(Clone, Default)]
  pub(crate) struct Answers(/* Arc<Mutex<{ next id, BTreeMap<u64, Sender<Answer>> }>> */);
  impl Answers {
      pub(crate) fn keep(&self, reply: Sender<Answer>) -> AnswerTo;
      /// Sends `answer` once and forgets the sender; an id no longer waiting gets nothing.
      pub(crate) fn answer(&self, to: AnswerTo, answer: Answer);
      pub(crate) fn forget(&self, to: AnswerTo);
      /// Drops every sender (spec §10.2 step 3): each waiting lane sees a decline at once.
      pub(crate) fn decline_all(&self);
  }
  pub(crate) fn new(tx: Sender<Msg>, shutdown: Cancel, answers: Answers) -> ChannelAsk;
  // actions.rs
  pub(crate) fn on_prompt(state: &mut State, prompt: Prompt) -> Vec<Cmd>;
  ```
- A poisoned `Answers` lock is recovered with `PoisonError::into_inner`, never a panic.
- **The decline** of an unanswered dialog is `Answer::Confirm(false)` for a confirm and `Answer::Choose(None)` for a choice. That is what `ChannelAsk` hands the CLI today on a dropped sender.

- [ ] **Step 1: Write the new tests.**
  - **`ask.rs`, `declining_every_prompt_frees_its_lane_before_the_slice_ends`.** It replaces `a_dialog_dropped_unanswered_declines`. A lane thread asks, and the test takes the prompt off `rx`. `answers.decline_all()` then makes `ask` return `None` within 50 ms, half of `SLICE`, with the shutdown token never set.
  - **`ask.rs`, `a_prompt_that_stopped_waiting_is_forgotten`.** After `ask` returns on a shutdown, `answers.answer(to, …)` does nothing and nothing is still waiting. Add `#[cfg(test)] fn waiting(&self) -> usize` to check that.
  - **`actions.rs`, rewritten.**
    - `a_cli_prompt_opens_a_dialog_bound_to_its_reply` asserts the returned commands, `[Cmd::Answer { to, answer: … }]`, for each case it covers today: confirm no, choose `Some(2)`, choose `None` on `Esc`, choose `Some(1)` on `2`, and confirm no on `Esc`.
    - `a_prompt_during_a_shutdown_is_declined_unshown`: with `quitting` set, `on_prompt` returns the decline and opens nothing.
    - `quitting_with_a_dialog_open_declines_it`: Ctrl-C returns `[Cmd::Quit { code: 0 }]` and closes the dialog.
  - **`actions.rs`, `a_second_prompt_declines_the_dialog_it_replaces`.** With a confirm bound to `AnswerTo(1)` open, a choose prompt for `AnswerTo(2)` returns `[Cmd::Answer { to: AnswerTo(1), answer: Answer::Confirm(false) }]` and shows the choice.
  - **`runtime.rs`, `a_shutdown_declines_a_waiting_prompt_at_once`.** `let (tx, rx) = mpsc::channel(); ui.answers.keep(tx);`. Then:
    - `ui.execute(vec![Cmd::Quit { code: 0 }])` gives `rx.try_recv() == Err(TryRecvError::Disconnected)`;
    - for a signal, `begin_shutdown(143, libc::SIGTERM, None)` on a fresh fixture gives the same.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked -- ask:: prompt dialog shutdown_declines`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed in Files.
- [ ] **Step 4: Check that the senders left State.** Run `rg -n "Sender<Answer>" crates/tagteam-tui/src`. Expected: only `ask.rs`.
- [ ] **Step 5: Run the crate's tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass.
- [ ] **Step 6: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Answer CLI prompts by id and keep their senders out of State"`.

### Task 7: Move each overlay's keys and drawing into a module of its own

**Depends on:** Task 6. This is a move only: no behaviour or text changes.

**Files:**
- Create the overlay modules:

  | File | What moves into it |
  |---|---|
  | `src/overlay/mod.rs` | `Overlay`, `ConfirmThen`, `TextThen`; `handle_overlay_key`, renamed `handle_key`; `overlay_key`'s dispatch; `render_overlay`, renamed `render`; the helpers more than one overlay uses (`boxed`, `CHOICE_ROWS`, and others as found) |
  | `src/overlay/confirm.rs` | the Confirm arms of `overlay_key` and `render_overlay`; `confirmed`; `question_rows` |
  | `src/overlay/choose.rs` | the Choose arms; `choice_rows` |
  | `src/overlay/text.rs` | the Text arms; `submitted`, `tail`, `ALIAS_MAX`, `POSITION_MAX` |
  | `src/overlay/panel.rs` | the Result and Doctor arms; `scrolled`, `panel`, `panel_height`, `furthest`, `panel_width`, `panel_rows` |
  | `src/overlay/help.rs` | the Help arms; `HELP_GLOBAL`, `HELP_ACCOUNTS`, `HELP_TOKENS`, `HELP_HISTORY`, `help_line` |

- Modify:
  - `src/actions.rs`: remove what moved. `start` becomes `pub(crate)`.
  - `src/lib.rs`: add `pub mod overlay;`. Task 15 narrows it.
  - Imports in `state.rs:5`, `update.rs:6`, `view.rs:274` and its tests (`:494`), and `screens/tokens/filter.rs:16`.
- Tests: tests that call a moved private item move with it, verbatim:
  - `every_help_line_shows_whole_on_every_screen_at_80_columns` goes to `help.rs`;
  - `a_long_input_shows_its_end` goes to `text.rs`.

  The rest stay in `actions.rs`.

**Interfaces:**
- Produces, for Task 8:
  - `crate::overlay::{Overlay, ConfirmThen, TextThen}`;
  - `overlay::handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd>`;
  - `overlay::render(state: &State, frame: &mut Frame, now_ms: i64)`.
- Each kind's module has these, with each arm's body verbatim:
  - `pub(super) fn key(state: &mut State, <the variant's fields by value>, key: KeyEvent, now_ms: i64) -> (Option<Overlay>, Vec<Cmd>)`;
  - a `pub(super) fn render(…)` taking what the arm used.

- [ ] **Step 1: Confirm the tests that pin the overlays.** They stay green with only import edits:
  - `actions.rs:907-1890`'s dialog, panel, doctor, help and `P` tests;
  - `update.rs:336`;
  - `view.rs:561, 618, 674`;
  - `screens/tokens/filter.rs`'s tests;
  - `screens/tokens/mod.rs:835`.
- [ ] **Step 2: Move** as listed in Files.
- [ ] **Step 3: Check it reads as a move.** Run `git diff --color-moved=dimmed-zebra HEAD`. Expected: the moved bodies show as moved. The only changed lines are `use` lines, `pub(super)`/`pub(crate)`, the new function signatures around the arms, and the dispatch.
- [ ] **Step 4: Run the crate's tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Move each overlay's keys and drawing into a module of its own"`.

### Task 8: Handle overlay keys in place and say what each key did

**Depends on:** Task 7

**Files:**
- Modify: `src/overlay/{mod,confirm,choose,text,panel,help}.rs`.
- Modify: `src/screens/tokens/filter.rs`. Change `handle_key` (`:206-268`) to the new shape.
- Modify: `src/view.rs:274`. Call `overlay::render(state, frame)`.
- Test: `src/overlay/mod.rs`.

**Interfaces:**
- Produces:
  ```rust
  pub(crate) enum KeyOutcome { Keep, KeepWith(Vec<Cmd>), Close, CloseWith(Vec<Cmd>) }
  pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd>;
  pub fn render(state: &State, frame: &mut Frame);
  // each kind:
  pub(super) fn key(state: &mut State, <the variant's fields as &mut>, key: KeyEvent, now_ms: i64) -> KeyOutcome;
  // filter.rs:
  pub(crate) fn handle_key(state: &mut State, dialog: &mut FilterDialog, key: KeyEvent, now_ms: i64) -> KeyOutcome;
  ```
- **`handle_key`** takes the overlay out of `state` (the borrow of `state` that `start` needs makes an in-place match impossible). It matches on `&mut overlay` and calls the kind's `key`. For `Keep` and `KeepWith` it puts the overlay back.
- **The changes inside the handlers:**
  - edits mutate the fields in place, with no rebuilt variant;
  - `confirmed` takes `&ConfirmThen` and clones the `Action` it starts;
  - doctor's `o` returns `KeepWith` (R-3).
- **Unchanged:** Ctrl keys still never reach an overlay (`update.rs:194-207`), and Ctrl-U still clears a text dialog there.

- [ ] **Step 1: Write the test** `a_kept_overlay_is_the_one_its_key_changed` in `overlay/mod.rs`:
  - with `Overlay::Text { input: "ab" }` open, `handle_key` for `c` returns `vec![]` and leaves `Text { input: "abc" }`;
  - for `Esc` it returns `vec![]` and leaves no overlay.
- [ ] **Step 2: Run it.** Run `cargo test -p tagteam-tui --locked a_kept_overlay`. Expected: it passes on Task 7's code, since behaviour is unchanged. It pins the put-back across the rewrite.
- [ ] **Step 3: Rewrite the handlers** to `&mut` and `KeyOutcome`, and drop `now_ms` from `render`.
- [ ] **Step 4: Check that only one place takes the overlay out.** Run `rg -n "overlay\.take\(\)|let _ = now_ms" crates/tagteam-tui/src`. Expected: only `overlay/mod.rs`'s `handle_key` takes.
- [ ] **Step 5: Run the crate's tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass, including:
  - `doctor_opens_its_panel_and_o_runs_it_online` (`KeepWith`);
  - `another_ctrl_key_does_nothing_in_a_dialog`;
  - the filter's tests.
- [ ] **Step 6: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Handle overlay keys in place and say what each key did"`.

### Task 9: Move Lanes beside Lane

**Depends on:** Task 6. This is a move only.

**Files:**
- Modify: `src/runtime.rs`. Move out:
  - `StatusFetch` (`:211`);
  - the `Lanes` struct and its `impl`: `spawn`, `stray_death`, `close`, `join` (`:217-422`);
  - the free `stray_death` (`:426-436`);
  - `PROGRESS_EVERY` (`:38`).

  `WAKE` becomes `pub(crate)` so that `PROGRESS_EVERY` can still be `WAKE`.
- Modify: `src/lanes.rs`. Receive the moved code after `Lane` and before the tests. The fields the runtime reads (`views`, `report`, `index`, `actions`, `daemon`, `live`) and `close` and `join` become `pub(crate)`.
- Modify: the runtime's tests import `Lanes`, and keep reaching `fx.lanes.index` and `fx.lanes.live`.

**Interfaces:** `crate::lanes::{Lanes, StatusFetch}`, with the signatures unchanged.

- [ ] **Step 1: Move.**
- [ ] **Step 2: Check it reads as a move.** Run `git diff --color-moved=dimmed-zebra HEAD`. Expected: only `use` lines and visibility words differ.
- [ ] **Step 3: Run the crate's tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass.
- [ ] **Step 4: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Move Lanes beside Lane"`.

### Task 10: Spawn the four engine lanes through one spawn_lane

**Depends on:** Task 9

**Files:**
- Modify: `src/lanes.rs`. Add `spawn_lane`, and build views, report, index and actions with it. The daemon lane keeps its own `Lane::spawn` (R-6).

**Interfaces:**
- Produces:
  ```rust
  /// An engine lane (spec §4.3): it builds its own `Source` from `sources` on its thread, and
  /// hands `handle` each request with the sender its results go to. A panic sends
  /// `Msg::LaneDied { lane: name }`.
  fn spawn_lane<Req: Coalesce + Send + 'static>(
      name: &'static str,
      sources: &SourceFactory,
      tx: &Sender<Msg>,
      handle: impl FnMut(&mut Box<dyn Source>, Req, &Sender<Msg>) + Send + 'static,
  ) -> Lane<Req>
  ```
- **What `handle` sends.** It sends as many messages as it likes: the index lane sends progress and then `Refreshed`. The actions lane passes the sender on to `ChannelAsk`.
- **Unchanged:** lane deaths are still reported through `join` for the engine lanes and through `strays` for the daemon and live threads.

- [ ] **Step 1: Confirm the tests that pin the lanes:**
  - `every_lane_was_joined`'s four `Source` drops (`runtime.rs:1480, 2226`);
  - the one panic line per lane (`:2192-2205`);
  - `q` returning within 500 ms while a status fetch runs (`:2290`);
  - `lanes.rs`'s own tests.
- [ ] **Step 2: Implement it, and replace the four copies.**
- [ ] **Step 3: Run the crate's tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass, unchanged.
- [ ] **Step 4: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Spawn the four engine lanes through one spawn_lane"`.

### Task 11: Type the daemon status error

**Depends on:** Task 9

**Files:**
- Modify: `src/live.rs`. Add `StatusError` beside `fetch_status` (`:311-318`), and make `fetch_status` return it.
- Modify the code that carries the error:
  - `src/msg.rs:176-180`: `DaemonStatus`'s `result`;
  - `src/lanes.rs`: `StatusFetch`;
  - `src/daemon/mod.rs:334`: `on_status`'s parameter. It still ignores the error (`Err(_) => broken(..)`).
- Modify tests: `tests/live.rs:594-637`, `daemon/mod.rs:920, 961, 974` and `runtime.rs:2467, 2509, 2541`. They build or match `StatusError` values in place of strings.

**Interfaces:**
- Produces, in `tagteam_tui::live` (public, since `live` stays `pub`):
  ```rust
  /// Why the daemon's `status` could not be read (CC-O-23c).
  #[derive(Debug)]
  pub enum StatusError {
      /// Nothing answers on the socket.
      NoDaemon,
      /// The daemon speaks protocol `server`, which this client does not.
      Mismatch { server: u32 },
      /// The connection or the reply failed.
      Client(tagteam_daemon::client::ClientError),
  }
  impl std::fmt::Display for StatusError; // today's strings: NO_DAEMON, "the daemon speaks protocol {server}", the ClientError's own
  impl std::error::Error for StatusError; // source(): the ClientError
  pub fn fetch_status(path: &Path) -> Result<Status, StatusError>;
  ```
- `connect_at`'s `Err(e)` and `status()`'s error both become `Client(e)`.

- [ ] **Step 1: Rewrite the asserts in `tests/live.rs`.**
  - `:612`: `assert!(matches!(err, StatusError::NoDaemon)); assert_eq!(err.to_string(), NO_DAEMON);`
  - `:616-619`: `matches!(err, StatusError::Mismatch { server: 2 })`, and its text `"the daemon speaks protocol 2"`.
  - `:635-637`: `matches!(err, StatusError::Client(ClientError::Timeout))`, and its text `"the daemon did not answer in time"`, with the same 2 to 4 s timing check.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked --test live`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed in Files.
- [ ] **Step 4: Run the crate's tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Type the daemon status error"`.

### Task 12: Wait for keys without waking while idle

**Depends on:** none

**Files:**
- Modify: the workspace `Cargo.toml`. Add to `[workspace.dependencies]`:
  - `mio = { version = "1.2", features = ["os-poll", "os-ext"] }`;
  - `signal-hook-mio = { version = "0.2.5", features = ["support-v1_0"] }`.
- Modify: `crates/tagteam-tui/Cargo.toml`. Add `mio.workspace = true` and `signal-hook-mio.workspace = true`, plus `signal-hook.workspace = true` under `[dev-dependencies]` for `low_level::raise` in a test.
- Modify: `Cargo.lock`, updated by one unlocked build. Expected: only `tagteam-tui`'s dependency list gains `mio`, `signal-hook-mio` and `signal-hook`, with no version change.
- Modify: `src/input.rs`, in five places:
  - **The new seam.** Replace `Events` with the seam below, and `Crossterm` with `Terminal`.
  - **`Wait`.** It blocks through mio: fd 0 readable, SIGWINCH through `signal_hook_mio::v1_0::Signals::new([libc::SIGWINCH])`, and a `mio::Waker`.
  - **The waker calls.** `pause`, `resume`, `hang_up` and `Drop` call the waker after they change the mode.
  - **The fakes.** `Script` and `Gate` are rebuilt on the new seam. Their `wait` blocks on a condvar until an event is queued or the waker is called.
  - **The constants.** `SLICE` stays only as the back-off after an error.
- Modify: `crates/tagteam/tests/common/pty.rs`. Add `Pty::resize(&self, cols: u16, rows: u16)`. It does `TIOCSWINSZ` on the master, which makes the kernel send SIGWINCH to the foreground group. `screen()` then emulates at the new size.
- Test: `src/input.rs`, `crates/tagteam/tests/tui_pty.rs`.

**Interfaces:**
- Produces:
  ```rust
  /// What woke the input thread.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub(crate) enum Ready { Input, Resized, Woken, Closed }
  /// What the terminal holds now.
  pub(crate) enum Next { Event(Event), Empty, Closed }

  pub(crate) trait Events: Send + 'static {
      /// Blocks until the terminal has bytes (`Input`), SIGWINCH arrived (`Resized`), the
      /// waker was called or a signal interrupted the wait (`Woken`), or the terminal hung up
      /// (`Closed`).
      fn wait(&mut self) -> io::Result<Ready>;
      /// An event crossterm has read and parsed, without blocking. `Closed` once the terminal
      /// has hung up, and then crossterm is not called.
      fn next(&mut self) -> io::Result<Next>;
      /// The terminal's size now, for a resize.
      fn size(&mut self) -> io::Result<(u16, u16)>;
      /// Wakes the current or the next `wait`, from any thread.
      fn waker(&self) -> Arc<dyn Fn() + Send + Sync>;
  }

  /// The wait itself, on any fd, so tests can drive it with a socket pair.
  pub(crate) struct Wait { /* mio::Poll, mio::Events, Signals, Arc<mio::Waker>, closed, resized */ }
  impl Wait {
      pub(crate) fn on(fd: RawFd) -> io::Result<Wait>;
      pub(crate) fn wait(&mut self) -> io::Result<Ready>;
      /// Without blocking: whether `fd` has hung up. Once seen it stays true. A SIGWINCH this
      /// poll takes is kept, and the next `wait` returns `Resized` at once.
      pub(crate) fn closed(&mut self) -> io::Result<bool>;
      pub(crate) fn waker(&self) -> Arc<dyn Fn() + Send + Sync>;
  }

  /// fd 0 through `Wait`. Its events come through crossterm (`event::poll(Duration::ZERO)`
  /// and `read`), and only after `Wait::closed` has said no. Its size comes from
  /// `crossterm::terminal::size()`.
  pub(crate) struct Terminal { /* Wait */ }
  ```
  `Input::spawn` and `Input::spawn_with` keep their signatures.
- **What `Wait` reports.**
  - An event on fd 0 with `is_read_closed()` or `is_error()` is `Closed`, and it is remembered.
  - Our own SIGWINCH registration is `Resized`, with the signal queue drained. It is made in `Wait::on`, before crossterm's lazy one.
  - Readable is `Input`.
  - The waker's token, or `ErrorKind::Interrupted`, is `Woken`.
- **The thread's loop:**
  ```text
  more = true              // crossterm may hold events from before
  loop {
      take the lock and act on the mode, as today; leaving Paused sets more = true
      if more { match next() { Event(e) => send it (crossterm's Resize is dropped, R-8;
                                            a failed send stops),
                               Empty => more = false,
                               Closed => set Stopped and return,
                               error => more = false, sleep SLICE }
                continue }   // one event per turn: the mode is checked between events
      match wait() { Input => more = true,
                     Resized => send Msg::Resize from size(), more = true,
                     Woken => {},
                     Closed => set Stopped and return,
                     error => sleep SLICE }
  }
  ```
- **Crossterm is never called on a terminal known to be closed.** Its read loops forever on a hung-up terminal (`input.rs:49-51`), so `Terminal::next` asks `Wait::closed` first, every time. A hang-up in the instant between that check and crossterm's read can still spin, as any read can today. The UI does not wait for this thread (`hang_up`), so it never holds up the shutdown.

- [ ] **Step 1: Write the tests** in `input.rs`:
  - `an_idle_thread_waits_without_waking`: a fake that counts calls. After spawning and 300 ms of no input it has seen exactly one `next` and one `wait`.
  - `a_pause_is_acknowledged_within_50_ms_while_idle`: on the blocking fake, `pause()` returns true in under 50 ms.
  - `a_closed_terminal_stops_the_thread_wherever_it_shows`, for three cases:
    - the fake's first `next` returns `Closed`, which is the drain at startup;
    - its first `next` after a resume returns `Closed`;
    - its `next` returns `Closed` after two events in one drain.

    In each case the thread stops, the fake sees no further `next` or `wait`, and `pause()` returns true at once.
  - `a_resize_is_sent_from_the_outer_signal_even_before_crossterm_has_it`: the fake's `wait` returns `Resized` while its `next` returns `Empty`. The thread sends `Msg::Resize` with the fake's size.
  - `the_wait_blocks_until_bytes_its_waker_or_a_resize`, on `Wait::on(fd)` over `UnixStream::pair()`:
    - with nothing written, `wait` does not return within 200 ms;
    - the waker makes it return `Woken` within 50 ms;
    - `signal_hook::low_level::raise(libc::SIGWINCH)` makes it return `Resized`;
    - a written byte makes it return `Input`.
  - `a_closed_terminal_is_seen_without_blocking`: with the peer dropped before any wait, `closed()` returns true at once, and `wait` returns `Closed`.
  - The two `Wait` tests share a `static` mutex, so that the process-wide SIGWINCH cannot land in the other one's 200 ms window.
  - The seven existing tests are rebuilt on the new fakes, with their bounds (`:313, 325, 369-370, 418, 430, 446`). `key_presses_and_resizes_become_messages_and_nothing_else_does` changes in one way only: a crossterm `Resize` is now dropped, and the fake's `Resized` gives the `Msg::Resize` (R-8).
- [ ] **Step 2: Write the pty tests** in `tui_pty.rs`:
  - `a_burst_of_keys_in_one_write_is_handled_whole`: `tui.type_keys(b"23")` in one write shows History within 2 s, using the predicate `three_shows_history_and_one_the_accounts_again` uses.
  - `a_resize_while_idle_redraws_at_the_new_size`: once the accounts show at 120×40, `tui.resize(100, 30)` gives, within 2 s, a screen whose row 29 starts with `s switch`.
- [ ] **Step 3: Run them to see them fail.**
  - Run `cargo test -p tagteam-tui --locked input::`. Expected: a compile error.
  - The pty tests may already pass on the polling thread. They pin the behaviour across the rewrite.
- [ ] **Step 4: Implement** as listed in Files and Interfaces. Run `cargo build -p tagteam-tui` once, without `--locked`, to record the two dependencies. Then `git diff Cargo.lock` must show only that.
- [ ] **Step 5: Run the tests.**
  - Run `cargo test -p tagteam-tui --locked`.
  - Outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty`.
  - Expected: all pass, including `a_locked_keychain_runs_the_action_on_the_terminal_and_returns` (a paused thread reads nothing) and the `under_r_*` tests.
- [ ] **Step 6: Lint the dependencies.** If the rebased tree has a `deny.toml`, run `cargo deny check`. Expected: clean.
- [ ] **Step 7: Commit.** Run `git add -A Cargo.toml Cargo.lock crates/tagteam-tui crates/tagteam/tests/common/pty.rs crates/tagteam/tests/tui_pty.rs && git commit -m "Wait for keys without waking while idle"`.

### Task 13: Lay out the token table once a second, not every frame

**Depends on:** Task 2

**Files:**
- Modify: `src/screens/tokens/table.rs`. Add `Layout` and `Rows`, and have `render` (`:1154-1205`) and `follow` (`:1109-1125`) take the rows from it. `items` becomes `Layout`'s builder.
- Modify: `src/screens/tokens/mod.rs`. `Level` gains `layout: table::Layout`, and `apply` (`:263-275`) and `reset` (`:294-297`) replace it wherever they replace `widths`.
- Test: `src/screens/tokens/table.rs`.

**Interfaces:**
- Produces, in `table.rs`:
  ```rust
  /// The table's lines and the line each row starts at (PERF-R-10b).
  pub(super) struct Rows { items: Vec<(usize, Item)>, starts: Vec<usize> }
  /// `Rows` for the level's report, kept while the width and the active blocks' clocks stay.
  #[derive(Default)]
  pub(super) struct Layout(/* RefCell<Option<{ width, active rows, clocks, Rc<Rows> }>>, plus a #[cfg(test)] build count */);
  impl Layout {
      pub(super) fn rows(&self, report: &TokenReport, width: usize, now_ms: i64) -> Rc<Rows>;
      #[cfg(test)] pub(super) fn builds(&self) -> usize;
  }
  /// What of `row`'s figures depends on the time: the elapsed and left seconds while its
  /// block is active, else `None` (not a block, no `active`, or ended).
  fn figures_clock(row: &TokenRow, now_ms: i64) -> Option<(i64, i64)>;
  ```
- **`figures` takes its time from `figures_clock`.** `figures` (`:729-759`) takes its two durations and its end check from `figures_clock`, so the key and the text cannot disagree. (`clock` is taken: `table.rs:958` formats the header's refresh time.)
- **The key** is `(width, the clocks of the report's rows whose period is a block with active: Some(_))`.
  - Those rows' indices are found when the rows are built.
  - Rows are laid out again only when the width changes or one of those clocks changes. That reproduces today's text at every millisecond (R-5).
  - A report with no active block is laid out once per width.
- **Nothing else in the rows depends on the time.** `items` (`:770-787`) reads the time only through `figures`. The period column and the crumbs are drawn per frame, outside `Rows`.
- **How `render` reads the rows.** It iterates `&rows.items` and clones only the figure lines it shows.

- [ ] **Step 1: Write the tests** in `table.rs`. Let `base = NOW_MS - NOW_MS.rem_euclid(1000)`.
  - `the_table_is_laid_out_once_per_report_width_and_clock`. On a 60-row day report with no block, `builds()` counts:
    - 1 after a draw at `base + 100`, 120 wide;
    - still 1 after `j` (`follow`), and after a draw at `base + 5_100`;
    - 2 after a draw 80 wide;
    - for the new level's `Layout`, 0 after a new report is applied, then 1 after a draw.
  - `an_active_block_s_countdown_changes_when_it_did`. Take a block report whose active block started at `base - 17_940_000` and ends at `base + 60_000`. At `base`, `base + 100`, `base + 150` and `base + 60_000`:
    - its draw equals the draw of a state built the same way and drawn only at that time. The text goes from `1m left` to `<1m left`, and at the end the figures are gone;
    - `builds()` counts 1, 2, 2 and 3.
  - `a_new_width_in_the_same_second_draws_like_a_fresh_layout`: drawing at 120 and then at 80, both at `base + 100`, gives the same 80-wide buffer as a state built the same way and drawn only at 80.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked -- laid_out_once countdown_changes a_new_width`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed in Files and Interfaces.
- [ ] **Step 4: Run the crate's tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass, including:
  - `the_active_block_counts_down_between_queries_and_is_no_longer_active_at_its_end` (`:1477`);
  - `a_long_report_scrolls_to_keep_the_selection_in_view_and_a_resize_keeps_it_there` (`:1937`);
  - `a_reports_widths_are_found_once_and_anew_for_the_next` (`:2481`).
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Lay out the token table once a second, not every frame"`.

### Task 14: Switch by the engine's UsageStrategy

**Depends on:** none

**Files:**
- Modify: `src/host.rs`. Delete `Strategy` (`:15-29`). `Action::SwitchBy` (`:34-37`) holds a `tagteam_engine::switch::UsageStrategy`, and the argv (`:88-95`) spells it through a private `strategy_arg`.
- Modify: `src/actions.rs:14, 262-268`. `b` gives `UsageStrategy::Best`, and `n` gives `UsageStrategy::NextAvailable`.
- Modify tests: `host.rs:242-253, 332-334` and `actions.rs:957-967` use `UsageStrategy`. In `crates/tagteam/tests/tui_args.rs:7, 60-89`, the import becomes `tagteam_engine::switch::UsageStrategy`, and the round trip against clap's `StrategyArg` is unchanged otherwise.

**Interfaces:**
- Produces:
  - `Action::SwitchBy { provider: ProviderId, strategy: UsageStrategy }`;
  - `fn strategy_arg(strategy: UsageStrategy) -> &'static str`, which gives `"best"` and `"next-available"` through an exhaustive match. It is not `UsageStrategy::as_str`, which names `switch --json`'s value.
- **Not this one:** `tagteam_core::autoswitch::Strategy` (`ConsumeFirst`) is `auto`'s strategy, not `switch`'s.

- [ ] **Step 1: Edit the tests** as listed in Files.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam --locked --features test-support --test tui_args`. Expected: a compile error, since `SwitchBy` still holds `host::Strategy`.
- [ ] **Step 3: Implement** as listed in Files.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-tui --locked` and `cargo test -p tagteam --locked --features test-support --test tui_args`. Expected: all pass, including `a_strategy_switch_names_no_account_and_no_guard`.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui crates/tagteam/tests/tui_args.rs && git commit -m "Switch by the engine's UsageStrategy"`.

### Task 15: Keep the TUI's modules to the crate

**Depends on:** Tasks 1–14, so that the dead-code sweep sees the final code

**Files:**
- Modify: `src/lib.rs`. `actions`, `daemon`, `lanes`, `msg`, `overlay`, `screens`, `state`, `update` and `view` become `pub(crate) mod`. `host`, `source` and `live` stay `pub`. `pub use runtime::{TuiExit, TuiOptions, run};` and `pub use state::Screen;` stay.
- Modify: the dead code clippy then reports:
  - delete `TokensState::period` (`screens/tokens/mod.rs:156`), which has no caller;
  - make `TokensState::{grouping, range, watch}` (`:160, 164, 195`) and `HistoryState::account` (`screens/history.rs:78`) `#[cfg(test)]`;
  - anything else clippy reports: delete it if nothing calls it, or `#[cfg(test)]` it if only tests do.

**Interfaces:** the binary and the integration tests keep using only these, unchanged:
- `host::{AccountRef, Action, ActionOutcome, Ask, Host}`;
- `source::{EngineSource, Source, SourceFactory}`;
- `live::{Backoff, Delivery, Live, LiveQueue, NO_DAEMON, StatusError, fetch_status, spawn}`;
- `Screen`, `TuiOptions` and `run`.

- [ ] **Step 1: Narrow the modules.**
- [ ] **Step 2: Lint.** Run `cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`. Expected: only `dead_code` reports, which Step 3 fixes.
- [ ] **Step 3: Fix each report** as listed in Files, then lint again. Expected: clean.
- [ ] **Step 4: Run the tests.** Run `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`. Expected: all pass, with `tui_cmd.rs` and `tui_host.rs` unchanged.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Keep the TUI's modules to the crate"`.

### Task 16: Record the input thread's change

**Depends on:** Task 12

**Files:**
- Modify: `CHANGELOG.md`. Under `## [Unreleased]`, add `### Changed` with `- \`tagteam tui\` no longer wakes up 20 times a second while it waits for a key.`
- Modify: `docs/superpowers/specs/2026-10-14-tagteam-tui-design.md` §4.3. The input-thread bullet becomes: "**The input thread** turns crossterm events into `Msg`s. It waits on the terminal, on SIGWINCH and on a waker the UI thread uses to pause, resume or stop it, so it makes no wake-up while idle and acknowledges a pause at once (§7.3). *(Amended by the TUI redesign spec §14, PERF-R-9.)*"
- Modify: `docs/superpowers/specs/2026-10-20-tagteam-tui-redesign-design.md` §18. Add `§4.3 (the input thread, §14)` to the TUI spec's list.

- [ ] **Step 1: Edit** the three files as listed.
- [ ] **Step 2: Check the format.** Run `cargo fmt --all --check`. Expected: clean, since no code changed.
- [ ] **Step 3: Commit.** Run `git add CHANGELOG.md docs/superpowers/specs && git commit -m "Record that the TUI's input thread no longer polls"`.

---

## After Task 16 (controller)

- [ ] **The gallery check** (above). Expected: no difference from `gallery/base`.
- [ ] **CI, in full.** Run the four CI commands. Expected: all clean, with totals reported as passed/total.
- [ ] **The pty suites.** Outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty --test signals`.
- [ ] **The first frame.** Outside the sandbox, on an idle machine, run `cargo test --release -p tagteam --features test-support --test perf -- --ignored tui_first_frame --nocapture`. Expected: both p95s are within 50 ms.
- [ ] **Linux.** Run the full suite in Docker: non-root with a passwd entry, HOME outside `/tmp`, and the `docker` call unpiped.
- [ ] **Cross-review.**
  - `xreview round --reset`.
  - A pre-merge review of `origin/main...tui-redesign`, sliced where the diff exceeds about 100 KB.
  - Fix and re-review until it approves.
- [ ] **The Draft PR.**
  - Push, then open a Draft PR that fills `.github/pull_request_template.md`. Its "Rulings" section lists R-1 to R-8, plus any ruling made in review.
  - In one commit, set this plan's status to `Implemented — <PR URL>` and the redesign spec's to `In progress — <PR URL>`.
  - Re-review that commit, then push it.
