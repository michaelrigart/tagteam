# tagteam TUI, part A — the core TUI: Implementation Plan

**Status:** Approved

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `tagteam tui` (and a bare `tagteam` on a terminal) opens a ratatui terminal UI with the Accounts and History screens. Its actions run the CLI's own command path. It refreshes on a timer, and it always restores the terminal and drains its workers on the way out.

**Architecture:**
- **A new library crate, `tagteam-tui`**, holds:
  - a pure Elm-style state machine (`update(&mut State, Msg, now_ms) -> Vec<Cmd>`);
  - pure renderers (`view(&State, &mut Frame)`);
  - worker lanes, each owning a `Source` (a read port over `Engine`) built inside its own thread;
  - the action types and the `Host`/`Ask` ports;
  - the runtime: terminal, input thread, UI loop, suspension and shutdown.
- **The `tagteam` binary:**
  - implements `Host` (`tui_host.rs`) by running `app::run` with captured output and a `Prompter` that forwards to the TUI's dialogs;
  - wires the `tui` subcommand and the bare-`tagteam` rule;
  - gains the hidden `--expect-account` guard in `App::resolve`.
- **Shared words:** the words `list` and `history` print move to `tagteam_engine::words`, so the CLI and the TUI say the same things.

**Tech Stack:**
- Rust, edition 2024, MSRV raised to 1.88 (Ruling R1).
- `ratatui` 0.30.2, with its default crossterm backend.
- `crossterm` 0.29.0.
- std threads and `mpsc`; no async runtime (core §4.4).
- Tests:
  - ratatui's `TestBackend`;
  - in-process `app::run` harnesses (`crates/tagteam/tests/app.rs` style);
  - `openpty` pty tests (`crates/tagteam/tests/signals.rs` style);
  - `tests/perf.rs`.

**Spec:** `docs/superpowers/specs/2026-10-14-tagteam-tui-design.md` (Approved, `4960da3`). Its "core §n" citations go to `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`. Read before any task:
- the TUI spec: §1–§7, §10, §11 and §12 (all of them; §8 and §9 belong to parts C and B);
- core §4.3, §8.3, §10.4, §12.5 (the signal rules while `claude` runs), §12.8, §13.1, §13.2, §13.4, §13.5, §14 and §14.1.

**Parts.** The TUI spec is implemented in three plans:
- **A (this plan)** covers everything that needs only `main`'s code.
- **B, the Tokens screen** (TUI spec §5.3, §6.4 and §9), is written once the token report's code exists, on `token-report` or `main`.
- **C, the live daemon client** (TUI spec §6.3, §7.4 and §8: live mode, the daemon lane, the header's auto state and `P`), is written once the daemon's code exists.

Each part gets its own Codex plan review. Part A leaves a slot for each: `Screen` gains `Tokens` in part B, and the header's mode text in part C. This branch merges last (spec §12.3).

## Execution notes

- **Where:** worktree `~/Code/tagteam-tui`, branch `tui`. When execution starts, set this plan's `**Status:**` to `In progress` in one commit.
- **New dependencies:** cargo cannot write `~/.cargo/registry` inside the sandbox. Run `cargo fetch` outside the sandbox once, after Task 3 adds ratatui and crossterm, then build sandboxed.
- **pty tests:** `openpty` fails with EPERM in the sandbox. Run the pty tests (Task 13's `tests/tui_pty.rs`, and `tests/signals.rs` if touched) outside the sandbox: `cargo test -p tagteam --features test-support --test tui_pty`.
- **Format and lint, in every task:** `cargo fmt --all` first. Clippy passes both with and without `--features tagteam/test-support`, with `-D warnings`.
- **Test isolation (core §15.1):** tests never reach the real HOME, network, Keychain or `claude`.
- **Unsafe code:** `libc` calls go only in `tagteam-provider`, with a `// SAFETY:` comment, as the crate already does. Exception: tests may call `libc` directly, as `tests/signals.rs` does.
- **Commits:** one imperative sentence in the repository's style ("Record the activation epoch with the active account"), with no prefix and no attribution lines.

### Integration notes

Each task below was applied as written, in order, in a scratch clone of `tui` at `4960da3`, and committed as its steps say; its own tests, `cargo fmt --all --check` and both clippy runs passed after every task, and the workspace suite (`cargo test --workspace --features tagteam/test-support`) after Tasks 2, 5, 9, 11, 12 and 13: 2991/2991 at the end, and 3033/3033 after the review round below. The sandbox refuses 39 of those tests, which pass outside it: `prompt::tests` (9), `tests/signals.rs` (1), `xtask`'s `compat` tests (14) and `tests/tui_pty.rs` (15). The changes made to the drafted tasks, one line each:

- Task 1: added Cycle A (R7), `UsageView.next_poll_at` with its views_usage test; renumbered the steps; `words.rs`'s `unread`/`read` fixtures set `next_poll_at: None`. Only four `UsageView { … }` literals exist (views.rs 2, render.rs testutil 2), not R7's ten: statusline.rs and tests/views_usage.rs build theirs from those or by struct update.
- Task 1: item 12 puts `use tagteam_engine::words;` in the `tagteam_engine` import group of history.rs instead of where `use crate::render;` stood alone.
- Task 1, 2, 3: removed the pointers to the drafts' dropped "Contract problems" and "integrator notes" sections, folding the one fact they carried into the text (Task 2: the rejected `long`-clearing alternative).
- Task 3: folded in the drafter's release chores (the first crates.io publish of `tagteam-tui`, no licence or advisory job to run) as a note beside the publish loop.
- Task 4: its `gen` paragraph cites R8 instead of the dropped contract problem.
- Task 5: `testutil::{unread, read}` set `next_poll_at: None` (R7); dropped the step adding `unicode-width` to `crates/tagteam-tui/Cargo.toml`, which Task 3 already adds (a second key fails to parse); Depends on 1, 4 as the table says; the ratatui/crossterm APIs and the testutil note moved in from the draft's integrator notes.
- Task 5: Step 4 expects 36 library tests (Task 3's 9, Task 4's 10, its own 17); the draft said 27, leaving out Task 3's.
- Task 6: the detail pane shows `next poll in <countdown>` between the age and the login's expiry while `UsageView.next_poll_at` is ahead (R7, spec §5.2), with the test `the_detail_pane_counts_down_to_the_next_poll_while_it_is_ahead`; the draft's note that the engine had no next poll is gone.
- Task 8: removes Task 3's `#[allow(dead_code)]` on `EngineSource.engine` (Task 3 said Task 8 would; the draft did not); commits `Cargo.lock`, which the dev-dependencies change; expected count 68 library tests (the draft said 58).
- Task 9: `on_done` follows R12: an outcome with nothing on stdout or stderr (a suspended action's) shows no toast or panel whatever its exit, and every outcome re-reads `accounts`, plus History's `LoadHistory` when History is open (spec §7.3 step 4). The draft's `<action> done` toast and `exit N` panel for empty output are gone: `an_empty_success_says_the_action_is_done` became `a_suspended_actions_outcome_shows_nothing_and_rereads_every_view`, and the result-panel test lost its empty `exit 1` row.
- Task 9: Step 4 expects 88 library tests; the draft said 78.
- Task 10: added the missing `**Depends on:** 2, 3`; the `pub` module and the `exe` field are contract C9's now, not a contract problem.
- Task 10: `UNLOCK_QUESTION`'s line is 87 once Task 2 has run (the draft said 85).
- Task 11: added the missing `**Depends on:** 4, 5, 9, 10`; R8, R11 and R12 stated in its readings; it now deletes Task 9's three `#[allow(dead_code)]` in `ask.rs` (Task 9 said Task 11 would; the draft did not), and `ask.rs` joins its commit; `a_sigint_left_by_a_suspended_action_is_taken_and_the_tui_carries_on` asserts no overlay and no running action before it presses `q` (R12 leaves no panel, so `q` is not an overlay's key under R9).
- Task 12: added the missing `**Depends on:** 11`; its line references now name the files as Tasks 2, 10 and 11 leave them (cli.rs, app.rs, lib.rs, completions.rs and the core spec had shifted); `a_bare_tagteam_opens_the_tui_only_for_a_person_at_a_terminal` is written as `cargo fmt` leaves it.
- Task 13: added the missing `**Depends on:** 12`; `debug_lines_go_to_the_log_file_and_never_to_the_terminal` quits with Ctrl-C, not `q`: the switch's two-line outcome opens Task 9's result panel, which takes `q` under R9 (the drafter's stand-in drew the outcome without a panel); a reading states R9 and R12 for the pty tests.

The review round (rulings R15 to R22) changed these tasks, each fix in the task that owns the code, test first:
- Task 5: the banner and a toast wrap onto up to three rows, taken from the body (R21): `a_long_banner_wraps_onto_three_rows_at_most_and_the_body_gives_them_up`, `a_long_toast_wraps_upwards_from_the_footer`; `view::wrap_rows` caps any wrapped text at a number of rows, for Tasks 6, 7 and 9; 38 library tests.
- Task 6: the detail pane grows with its windows' lines, wraps a window's parts under its value, and scrolls the rest with `J`/`K` (R20); the rule's name gives way to its facts; `view::{flow, scrolled_windows, furthest_scroll}` with `flow_wraps_parts_under_the_indent_and_cuts_only_a_piece_too_wide`; `the_detail_pane_grows_to_show_every_window`, `windows_beyond_the_pane_scroll_into_view_with_shift_j_and_k`, `a_window_too_wide_for_80_columns_wraps_its_parts_under_its_value`, `the_rule_cuts_the_name_before_the_facts`; `HELP_ACCOUNTS` lists `J K`; `a_long_unmanaged_live_login_banner_wraps_and_still_says_press_a` (R21); the label column is capped (R20) and J/K start from what shows (R22): `an_oversized_label_goes_above_its_value_and_the_column_stays_capped`, `a_resize_clamps_the_detail_scroll_so_the_next_key_moves_at_once`, `a_resize_keeps_the_tables_scroll_in_range_and_the_next_key_moves_at_once`; a label takes two rows at most and the pane holds its tallest window (R20): `a_1000_character_label_leaves_its_window_reachable_at_80x24_under_a_full_banner`, `a_window_takes_seven_rows_at_most_whatever_its_words`; 27 accounts tests.
- Task 7: History's projections wrap under the sparkline and its windows scroll with `J`/`K` (R20), so it depends on Task 6 too: `a_projection_too_wide_for_the_panel_wraps_under_the_sparkline`, `windows_beyond_the_screen_scroll_into_view_with_shift_j_and_k`; `HELP_HISTORY` lists `J K`; the label column is capped (R20) and J/K start from what shows (R22): `an_oversized_label_goes_above_its_sparkline_and_the_column_stays_capped`, `a_resize_clamps_the_window_scroll_so_the_next_key_moves_at_once`, `a_1000_character_label_leaves_its_window_reachable_at_80x24_under_a_full_banner`; 12 history tests.
- Task 8: `update::drawn`, the first collection after the first frame that shows accounts (R19): `the_first_frame_that_shows_accounts_asks_for_one_collection`; `a_long_collection_error_shows_in_full_at_80_columns` (R21); 88 library tests.
- Task 9: its `actions.rs` carries Tasks 6 and 7's help lines; panels wrap and scroll by rows, and choices wrap (R21): `a_long_output_line_wraps_in_its_panel_and_scrolls_to_its_end`, `a_long_choice_wraps_under_its_text_and_is_selected_whole`; a panel's indent is capped (R21) and its scroll clamped (R22): `an_indent_wider_than_a_third_of_a_panel_is_capped`, `a_resize_clamps_a_panels_scroll_and_the_next_key_moves_at_once`; choices and questions are capped (R21): `a_1000_character_choice_takes_three_rows_and_every_choice_stays_reachable`, `a_long_question_is_cut_so_the_dialog_keeps_its_buttons`; 114 library tests.
- Task 10: `Cancel::take_if` (R15), with `take_if_consumes_only_the_signal_it_names_for_every_clone` and `spending_a_sigint_never_consumes_a_signal_that_replaced_it`; `wait_for_enter` spends only a SIGINT.
- Task 11: the terminal's lock (R16), the signal token before every message (R17), no hand-over without the input thread's acknowledgment (R18), and the first collection after drawing (R19), so it depends on Task 8 too; `term.rs` 7 tests, `input.rs` 5, `runtime.rs` 20.

### Rulings

- **R13, a SIGINT at the hand-over.** A SIGINT sent to the process with `kill` in the
  instant between the loop's signal check and `suspend`'s hand-over counts as the foreground
  action's, so the TUI carries on instead of exiting 130 (Task 11). The window is a few
  instructions wide, and a terminal's Ctrl-C reaches the action, not the TUI, in either case.
  This is kept as a known limitation.
- **R14, Linux.** Nothing in this plan has run on Linux yet. CI's ubuntu job and the Docker
  recipe (memory: Linux Docker repro) run the suite at the pre-merge checkpoint, the pty harness
  included.
- **R15, a foreground action's SIGINT is spent with `Cancel::take_if`.** Spec §7.3's "the TUI
  takes it (`Cancel::take`)" is `take_if(SIGINT)`, which Task 10 adds to `tagteam-provider`: one
  compare-and-swap from SIGINT to none. `requested()` and then `take()` would consume a SIGTERM
  or SIGHUP that replaced the SIGINT in between, and the TUI would wait for Enter instead of
  exiting 128 + n. `CliHost`'s wait for Enter (Task 10) and the runtime's suspension (Task 11)
  both use it.
- **R16, one lock owns the terminal.** Every frame (the check that the TUI holds the terminal,
  the draw and its flush), every enter and restore, the panic hook's restore, and the cursor
  ratatui shows as its `Terminal` is dropped take one lock (`term::Ownership`, Task 11). A lane
  that panics while a frame is written restores after the frame, never under it, and nothing is
  drawn once the terminal is given back. On the thread that holds the lock (a panic inside
  `view`) the hook restores without waiting for itself: the lock records its holder.
- **R17, the signal token before every message.** The UI thread looks at the signal token after
  every wake, before it applies the message or the tick that woke it, in the first frame's wait
  too (spec §4.3, §10.2). A message the shutdown overtakes is not applied; an action's outcome
  in it goes into the report (§10.2 step 6).
- **R18, no hand-over without the input thread's acknowledgment.** `Pause::pause` returns whether
  the input thread acknowledged within its second (`ACK`). Without it a suspension and Ctrl-Z are
  refused: nothing runs, the pause is withdrawn, the TUI keeps the terminal, and the error banner
  says `<action> did not start: the TUI's key reader did not pause in time; try again` (or
  `Ctrl-Z did not suspend: …`). A shutdown's hand-back restores the terminal regardless: the
  process is exiting.
- **R19, the first collection follows the first frame that shows accounts.** Spec §6.1: "the TUI
  collects after drawing". After each frame the runtime calls `update::drawn` (Task 8), which
  asks for one `collect_due` of the shown accounts the first time a frame shows any, once no
  action holds the actions lane (spec §7.2). `gens.collect` is still 0 until a collection has
  been asked for. This holds whether `accounts` came within the first frame's 100 ms or after
  `loading…`, and the first frame's budget is unchanged: the collection runs on the actions lane.
- **R20, every window can be reached, whole, at 80 columns.** Spec §5.1 (80×24 is enough), §5.2
  ("each relevant window with its reset, pace and projection") and §5.4.
  - **Wrapping.** A window line that does not fit the width goes on to continuation lines that
    start under its value (History: under its sparkline), breaking between its parts (reset,
    pace, projection). A part too wide for a whole continuation line breaks at its own ` · `s,
    and only a piece wider still is cut (`view::flow`, Task 6). On the detail pane's rule the
    account's name gives way before the facts after it (`in a session`, `▶ this`).
  - **The label column** is as wide as the widest label, and a third of the pane at most, so
    the value and a continuation row always have room. A label wider than that (any non-empty
    model `display_name` is one) wraps onto rows of its own above its value row, whose label
    column is blank, two rows at most: only text beyond them is cut, the second row ending in
    `…`, as the table cuts long names (Review Focus 2). The other windows keep the capped
    column. Those rows count in the pane's height and in `J`/`K`'s lines. The value is one row,
    cut by width only if a spend's number is wider than the row.
  - **Every window fits its viewport, by construction**, so `J`/`K` can step a whole window at a
    time and still reach everything. An Accounts window is at most 7 rows whatever its words:
    2 label rows, the value's, and one each for reset, pace, rate and where the projection
    leads (a piece is never more than a row; `projection_text` has one ` · `). The detail pane
    is never shorter than its tallest window plus its rule, age and trouble rows: it takes rows
    from the table for that, down to 3 (`TABLE_MIN`). At 80×24 under a 3-row banner and a
    3-row toast the body is 17 rows, so the pane can be 14, and a 7-row window needs 10. A
    History window is at most 5 rows (2 label rows, the sparkline's, rate, where it leads), and
    History's panel is 16 rows at the least.
  - **Scrolling.** The Accounts detail pane grows with the lines of the most windows any account
    shown has, at the current width, up to half the body. Windows that still do not fit, and
    History's beyond the screen, scroll with `J`/`K` (Tasks 6 and 7), a whole window at a time,
    and never show half a window. Help lists the keys, and the pane's rule and History's range
    line say which windows show (`J K: windows 1–5 of 13`).
  - The table's rows, columns and words do not change.
- **R21, nothing a person needs is cut at 80×24.** Spec §5.1 makes 80×24 enough, so text that can
  be wider than the terminal wraps at word boundaries instead of running off its edge:
  - **The banner** (a failed read, `usage was not collected: …`, an unmanaged live login with its
    `press A to add it`) takes as many rows as its words need, three at most, from the body, and
    **a toast** does the same above the footer (Task 5). Only text beyond the third row is cut,
    and the row then ends in `…`.
  - **The result and doctor panels** wrap long output lines, continuation rows indented as the
    line is, the indent a third of the panel at most; a word wider than the panel breaks by
    width. Their scroll, and its limit, count rows at the terminal's width (Task 9).
  - **A choose dialog's option** wraps under its own text, three rows at most (a CLI's
    `<provider> #<n> <name> [<org>]` fits), and the selected one is reversed whole. **A dialog's
    question** (confirm, choose, text) takes a third of the terminal's rows at most, so its
    buttons, choices or input always show (Task 9). Text beyond either cap is cut with `…`.
  - Left as they are: `list`'s table (its columns are `list`'s, Review Focus 2) and History's
    26-column account list, which cut names by display width; the detail pane's age and trouble
    lines, whose words cannot pass 70 columns.

- **R22, every scroll is in range at the current size.** A stored scroll (the result and doctor
  panels', the detail pane's and History's `J`/`K`, the table's offset) can be left past the end
  by a resize or a new read. It is clamped where it is used: when drawn, against the content at
  the current size, so nothing is drawn blank; and as the start of the next key, so that key
  moves from what shows. The choose dialog's list and History's account list keep no scroll
  (they are placed from the selection each frame), and the help overlay does not scroll.
  Whole-window steps reach everything because no window is taller than its viewport (R20).
  `update` does not clamp on `Resize`: the use-site clamp covers a new read too, and keeps the
  screens' state their own.

- **R1, MSRV.** ratatui 0.30.2 declares `rust-version = 1.88.0`. The workspace pins toolchain 1.88.0 already (`rust-toolchain.toml`), and no CI job builds 1.85. Raise `[workspace.package] rust-version` to `"1.88"`, and CONTRIBUTING.md's statement with it. Core §16's "MSRV ≥ 1.85" still holds.
- **R2, parts.** The token report's and the daemon's types do not exist in code yet. The TUI spec forbids shapes of the TUI's own for them (spec §9: "defines no shapes of its own"), so instead of mirror types, part A ships everything else, and parts B and C follow when the code exists.
- **R3, shared words.** `tagteam-tui` cannot depend on the `tagteam` crate, which depends on it. The text helpers both need move down to `tagteam_engine::words`. The engine already owns the views' vocabulary (`views::NO_DATA`). The CLI's output must not change by a byte: its existing tests are the guard.
- **R4, sources are built in their lanes.** Each lane calls a `SourceFactory` inside its own thread, so neither `Engine` nor `Context` needs to be `Send` or `Sync`.
- **R5, the "Press Enter" wait is the host's.** The cancellation-aware terminal read (`prompt::read_terminal_line`) lives in the binary, so `Host::act_foreground` prints `Press Enter to return to tagteam` and waits for it. On a signal it returns early.
- **R7, next poll.** Spec §5.2's detail pane shows the next poll, which `UsageView` does not
  carry: its `retry_at` is set only for a status other than `ok`. Task 1 adds
  `UsageView.next_poll_at: Option<i64>`: the stored `next_poll_at`, filtered by
  `plan_is_skewed` as `retry_at`'s is, and `None` once it has passed. It updates the ten
  `UsageView { … }` literals (`views.rs` 3, `tests/views_usage.rs` 2, `render.rs` 4,
  `statusline.rs` 1). The CLI's output does not change.
- **R8, `gen`.** `gen` is a reserved keyword in edition 2024, so every generation field is
  written `r#gen`: in the type, in constructors, and in patterns.
- **R9, keys under an overlay.** While an overlay is open, every key but Ctrl-C and Ctrl-Z goes
  to it, so a text dialog can take `q`, `1` and `?`. `q` quits only with no overlay open; `Esc`
  closes an overlay first.
- **R10, provider headings.** With several providers, the tables are headed by the provider's
  ID. One provider, the only case this build registers, shows no heading.
- **R11, the order of shutdown triggers.** `update` sets `quitting` for `q`, Ctrl-C (0) and
  `LaneDied` (1). The runtime sets it for a signal (128 + n) and for a UI-thread panic (1). The
  first trigger's code wins; a later one never overwrites it.
- **R12, a suspended action's outcome.** It arrives as `Msg::ActionDone`, with empty `out` and
  `err` (its output went to the terminal). `on_done` then shows no toast or panel. It re-reads
  `accounts`, and `history` when open (spec §7.3 step 4). `act_foreground` never returns
  `NeedsTerminal`.
- **R6, `libc` for the TUI's own process control** (`raise(SIGTSTP)`, `kill`, `waitpid`): any helper that `tagteam-provider` lacks is added there, with `// SAFETY:` comments. Task 11 names each.

## Global Constraints

- **Rendering:** ratatui `=0.30.2` and crossterm `=0.29.0`, pinned exactly in `[workspace.dependencies]`.
- **Start-up:** the first frame showing the accounts table within 50 ms p95 when no usage fetch is due (spec §1.1 criterion 2; measured as core §1.1 measures `list`).
- **The UI thread does no I/O** but the terminal: no store, network, Keychain or transcripts (spec §1.1 criterion 3).
- **Fetching:** usage is fetched only through `CollectMode::OnDemand` (core §8.3), never on a schedule of the TUI's own (spec §1.1 criterion 4).
- **Size:** below 80×24 the body is replaced by `terminal too small (80×24 needed)` (spec §5.1). At 80×24 and up nothing a person needs runs off the edge: it wraps (R20, R21).
- **Colour:** only the 16 named ANSI colours. Severity ≥ 70 % is a warning and ≥ 90 % critical (core §13.5). `--no-color` or `NO_COLOR` gives bold for emphasis and reverse video for the selection.
- **Refresh:**
  - Timer mode re-reads `accounts` every 30 s, and `history` when History is open.
  - `collect_due` runs every 30 s for the shown accounts, and is skipped while any action runs (spec §6.2, §7.2). The first runs after the first frame that shows accounts (spec §6.1, R19).
  - The first frame waits up to 100 ms for the first `accounts` result, then draws `loading…` (spec §6.1).
- **Toasts** last 5 s (spec §5.1).
- **The UI loop** wakes at least every 250 ms and checks the signal token on every wake, before the message that woke it (spec §4.3, §10.2, R17).
- **Exit codes** (spec §3, §10.2):
  - 0 after `q` or Ctrl-C;
  - 128 + n after a SIGTERM, a SIGHUP, or a SIGINT sent to the process;
  - 1 for a fatal error;
  - 2 for a usage error.
- **Refusals before the terminal is touched** (spec §3):
  - `--json` gives `tui has no JSON output; run it without --json` (exit 2);
  - stdin or stdout not being a terminal gives `tagteam tui needs a terminal` (exit 2).

  Both are usage errors (`KIND_USAGE`).
- **Account references in actions:** `--provider P --expect-account <ID> <position>` (spec §4.5). The refusal kind is `stale-selection`, with the message `the selected account changed; nothing was done`.

## Review Focus

1. **A resize below 80×24 and back while a dialog is open:** the dialog survives, nothing panics, and the answer still reaches the waiting lane. Task 5 owns the test.
2. **Account names wider than their column** (an alias with CJK characters or emoji, a 60-character email): cells are cut by display width (`unicode-width`), never by bytes, and the table never overflows or panics at 80 columns. Task 6 owns the test.
3. **No store and no accounts:**
   - Accounts shows the empty state (`No accounts yet. Log in with claude, then press A to add the login.`);
   - History shows `No usage history.`;
   - every account action key does nothing, apart from `A`, `D` and `?`.

   Task 6 owns the test.
4. **More accounts than fit:** the table scrolls so that the selected row is always visible, and `j`/`k` past either end stops there. Task 6 owns the test.
5. **The selection follows the account, not the row:** after a refresh that reorders or removes rows, the selection stays on the same `AccountId`, or moves to the nearest row when that account is gone. An action always names the selected account's ID. Task 6 owns the test.
6. **More windows, or wider ones, than fit (R20):** an account with five or more windows shows every one, with its reset, pace and projection, in a detail pane that grows, and `J`/`K` reach those beyond half the body, a whole window at a time; History's windows scroll the same way. At 80 columns a long model label with a spend value, reset, pace and projection wraps under the value instead of being cut, and at 120 it fits on one line. Tasks 6 and 7 own the tests.
7. **Long text at 80×24 (R21):** a long banner (`Live: <60-character email> (not managed by tagteam); press A to add it`, `usage was not collected: <message>`) wraps onto up to three rows and keeps its last words; a long toast wraps; a result panel line longer than 72 columns wraps, keeps its indent, and scrolls to its last row; a long choice wraps under its text. Tasks 5, 6, 8 and 9 own the tests.
8. **A resize while scrolled (R22):** after `End` or `J` and a resize that leaves fewer rows, every scrolled view shows its last rows at once, never a blank panel, and the next key moves from what shows. An 80-column model label leaves every window's value and projection in view, and a 1,000-character one takes two rows, ending in `…`, with its window still reachable whole at 80×24 under a 3-row banner and a 3-row toast (R20). Tasks 6, 7 and 9 own the tests.

## Interface Contract

Every task's names, signatures and strings must match this section exactly. A task that finds an item unworkable against the real code says so under its own "Contract problems". It does not invent a variant.

### C1. `tagteam_engine::words` (Task 1)

`crates/tagteam-engine/src/words.rs`, `pub mod words;` in `lib.rs`. These move out of `crates/tagteam/src/render.rs` and `crates/tagteam/src/history.rs`, which then call them. The behaviour is unchanged, and the bodies are today's.

```rust
pub const MISSING: &str = "—";
pub const AHEAD: &str = "▲ pace";
pub const IN_SESSION: &str = "▶";
pub const THIS: &str = "this";
pub const SPEND_HEAD: &str = "SPEND";

pub fn email(r: &AccountRow) -> String;            // render::email
pub fn name(r: &AccountRow) -> String;             // render::name
pub fn account_cell(r: &AccountRow) -> String;     // name, plus " [org]" when org_name is set (render::row's `account`)
pub use tagteam_core::rank::span as duration;      // render::duration
pub fn countdown(at: i64, now_s: i64) -> String;   // render::countdown
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity { Normal, Warning, Critical }
pub fn severity(pct: i64) -> Severity;             // ≥ 90 Critical, ≥ 70 Warning
pub fn money(amount: f64, currency: &str) -> String;    // render::money
pub fn spend_text(w: &Window) -> Option<String>;        // render::spend_text
pub fn usage_words(u: &UsageView, now_s: i64) -> String; // render::words
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowColumn { pub key: Option<String>, pub kind: WindowKind, pub head: String }
pub fn window_columns(accounts: &[AccountView]) -> Vec<WindowColumn>; // render::columns
pub fn row_notes(v: &AccountView, this: bool, now_s: i64) -> Vec<String>; // render::row's notes
pub fn rate_signed(rate: f64) -> String;                  // history::signed
pub fn projection_text(hw: &HistoryWindow, now_s: i64) -> String; // history::projection
```

`render.rs` keeps its ANSI codes, and maps `Severity` to them where it used `severity()`'s `Option<&str>`.

### C2. `--expect-account` (Task 2)

- **The flag:** `Cli` gains a hidden global flag:

  ```rust
  /// The account the reference must resolve to (the TUI's guard, §10.4); hidden.
  #[arg(long = "expect-account", global = true, hide = true, value_name = "ID")]
  pub expect_account: Option<String>,
  ```

  It is absent from `--help` and from every completion script.
- **The guard:** `App` gains `expect_account: Option<AccountId>`. `App::resolve` refuses after resolving, when the row's `id` differs: `Failure::Message(KIND_STALE_SELECTION, STALE_SELECTION.into())`. Every account command resolves through it (switch, enable, disable, alias, move, remove, history, map) and acts on that row's ID. `run_agent` resolves through it too.
- **Kind and exit:** `pub const KIND_STALE_SELECTION: &str = "stale-selection";`, with exit 1.
- **Message:** `const STALE_SELECTION: &str = "the selected account changed; nothing was done";`.
- **`Context`** gains `#[derive(Clone)]`.

### C3. `tagteam-tui` public API (Task 3 defines; later tasks implement)

`crates/tagteam-tui/src/lib.rs`:

```rust
pub mod host;      // C3.1
pub mod source;    // C3.2
pub mod lanes;     // C4 (Task 4)
pub mod msg;       // C5 (Task 5)
pub mod state;     // C5 (Task 5)
pub mod update;    // C5 (Task 5)
pub mod view;      // C5 (Task 5)
pub mod screens;   // C6 (Tasks 6, 7)
pub mod actions;   // C7 (Task 9)
mod ask;           // C7 (Task 9)
mod term;          // C8 (Task 11)
mod input;         // C8 (Task 11)
mod runtime;       // C8 (Task 11)
pub use runtime::{run, TuiOptions, TuiExit};
pub use state::Screen;
```

Task 3 creates every module file listed: `host.rs` and `source.rs` with their contract contents, and the rest as empty modules (a file with only a `//!` line), so the crate builds. Each later task fills its own modules. The two `pub use` lines cannot compile against empty modules, so Task 3 leaves them out. Task 5 adds `pub use state::Screen;` and Task 11 adds `pub use runtime::{TuiExit, TuiOptions, run};`.

#### C3.1 `host.rs`

```rust
use tagteam_core::{AccountId, ProviderId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRef { pub id: AccountId, pub provider: ProviderId, pub position: u32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy { Best, NextAvailable }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Switch(AccountRef),
    SwitchBy { provider: ProviderId, strategy: Strategy },
    Enable(AccountRef),
    Disable(AccountRef),
    SetAlias { account: AccountRef, alias: String },
    ClearAlias(AccountRef),
    Move { account: AccountRef, position: u32 },
    Remove(AccountRef),
    AddLive { provider: ProviderId },
    Run(AccountRef),
    Doctor { provider: Option<ProviderId>, online: bool },
}

impl Action {
    /// The arguments after `tagteam`, for the CLI's own parser.
    pub fn args(&self) -> Vec<String>;
    /// The command's name as `command_name` spells it: "switch", "enable", "disable",
    /// "alias", "move", "remove", "add", "run", "doctor".
    pub fn name(&self) -> &'static str;
    /// `Run` only: it always runs suspended (spec §7.3).
    pub fn needs_terminal(&self) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutcome {
    Done { exit: i32, out: String, err: String },
    /// The command needs the terminal itself, and changed nothing (spec §4.5).
    NeedsTerminal,
}

pub trait Ask {
    fn confirm(&mut self, question: &str, default_yes: bool) -> bool;
    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize>;
}

pub trait Host: Send + Sync {
    /// Runs `action` inside the TUI, on the actions lane. Prompts arrive through `ask`.
    fn act(&self, action: &Action, ask: &mut dyn Ask) -> ActionOutcome;
    /// Runs `action` on the real terminal, on the UI thread, after the TUI has suspended. Then
    /// prints `Press Enter to return to tagteam` and waits for Enter, or for a signal (R5).
    fn act_foreground(&self, action: &Action) -> ActionOutcome;
}
```

`Action::args`, exactly (`P` = the provider's string, `ID` = the account ID's string, `N` = its position):

| Action | args |
|---|---|
| `Switch` | `--no-color --provider P --expect-account ID switch N` |
| `SwitchBy` | `--no-color --provider P switch --strategy best` (or `next-available`) |
| `Enable` / `Disable` | `--no-color --provider P --expect-account ID enable N` (or `disable N`) |
| `SetAlias` | `--no-color --provider P --expect-account ID alias N -- NAME` |
| `ClearAlias` | `--no-color --provider P --expect-account ID alias N --unset` |
| `Move` | `--no-color --provider P --expect-account ID move N POS` |
| `Remove` | `--no-color --provider P --expect-account ID remove N` |
| `AddLive` | `--no-color --provider P add` |
| `Run` | `--provider P --expect-account ID run N` |
| `Doctor` | `--no-color [--provider P] doctor [--online]` |

`SetAlias`'s `--` keeps an alias that begins with `-` from being read as a flag. If the CLI's `alias` parser rejects `--` there, that is a contract problem: report it, and do not drop the `--`.

#### C3.2 `source.rs`

```rust
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::Engine;
use tagteam_engine::error::EngineError;
use tagteam_engine::views::{HistoryView, ProviderAccounts};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError { pub kind: String, pub message: String }
impl From<EngineError> for SourceError; // kind: e.kind(), message: e.to_string()

#[derive(Debug, Clone)]
pub struct AccountsSnapshot {
    pub providers: Vec<ProviderAccounts>,
    /// The run shell's own account (`ShellAccount`), for `▶ this`.
    pub this: Option<AccountId>,
    /// The default provider's live login, when tagteam does not manage it (`StatusView::Unmanaged`).
    pub unmanaged_live: Option<String>,
    pub in_run_shell: bool,
    /// The engine clock, in seconds, when this was read.
    pub read_at_s: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectSummary { pub fetched: usize, pub warnings: Vec<String> }

pub trait Source {
    fn accounts(&self, provider: Option<&ProviderId>) -> Result<AccountsSnapshot, SourceError>;
    fn history(&self, account: &AccountId, since_s: i64) -> Result<HistoryView, SourceError>;
    fn collect_due(&self, accounts: &[AccountId]) -> Result<CollectSummary, SourceError>;
}

/// Builds one `Source` inside the thread that will use it (R4).
pub type SourceFactory = std::sync::Arc<dyn Fn() -> Box<dyn Source> + Send + Sync>;

pub struct EngineSource { engine: Engine }
impl EngineSource { pub fn new(engine: Engine) -> Self; }
impl Source for EngineSource { /* Task 8 */ }
```

Task 3 writes `EngineSource`'s methods as `unimplemented!()`, and Task 8 replaces them. That is the one stub allowed, and Task 8's tests pin every method.

### C4. `lanes.rs` (Task 4)

```rust
pub trait Coalesce { fn same_as(&self, other: &Self) -> bool; }

pub struct Lane<Req: Coalesce + Send + 'static> { /* Mutex<VecDeque<Req>> + Condvar + closed flag + JoinHandle */ }

impl<Req: Coalesce + Send + 'static> Lane<Req> {
    /// Spawns the lane's thread. `init` runs first, inside the thread (R4). `work` runs each
    /// request in order. `on_death` runs once if the thread unwinds from a panic.
    pub fn spawn<S: 'static>(
        name: &'static str,
        init: impl FnOnce() -> S + Send + 'static,
        work: impl FnMut(&mut S, Req) + Send + 'static,
        on_death: impl FnOnce() + Send + 'static,
    ) -> Lane<Req>;
    /// Queues `req`. A queued request that `same_as` it is replaced in place (the newest wins).
    /// False once the lane is closed.
    pub fn submit(&self, req: Req) -> bool;
    /// Drops every queued request and wakes the thread, which returns after its current one.
    pub fn close(&self);
    /// Joins the thread; Err when it panicked.
    pub fn join(self) -> std::thread::Result<()>;
}
```

The lanes' request types (`msg.rs`, Task 5):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewsReq {
    Accounts { r#gen: u64 },
    History { r#gen: u64, account: AccountId, since_s: i64 },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionsReq {
    Collect { r#gen: u64, accounts: Vec<AccountId> },
    Act { action: Action },
}
```

Coalescing: `same_as` holds for two requests of the same variant, except `Act`, which never coalesces. Task 4 creates `msg.rs` with only these two enums and their `Coalesce` impls. Task 5 adds the rest of `msg.rs`, so Task 4 runs before Task 5.

### C5. Messages, commands, state, `update` and `view` (Task 5)

`msg.rs`:

```rust
#[allow(clippy::large_enum_variant)]
pub enum Msg {
    Key(crossterm::event::KeyEvent),
    Resize { width: u16, height: u16 },
    Tick,
    Accounts { r#gen: u64, result: Result<AccountsSnapshot, SourceError> },
    History { r#gen: u64, result: Result<HistoryView, SourceError> },
    Collected { r#gen: u64, result: Result<CollectSummary, SourceError> },
    Prompt(Prompt),
    ActionDone { action: Action, outcome: ActionOutcome },
    LaneDied { lane: &'static str },
}
pub struct Prompt { pub question: String, pub kind: PromptKind, pub reply: std::sync::mpsc::Sender<Answer> }
pub enum PromptKind { Confirm { default_yes: bool }, Choose { options: Vec<String> } }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer { Confirm(bool), Choose(Option<usize>) }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    LoadAccounts { r#gen: u64 },
    LoadHistory { r#gen: u64, account: AccountId, since_s: i64 },
    CollectDue { r#gen: u64, accounts: Vec<AccountId> },
    Act(Action),
    /// Run `action` with the terminal handed back (spec §7.3).
    Suspend(Action),
    /// Begin the shutdown sequence (spec §10.2) with this exit code.
    Quit { code: i32 },
    /// Ctrl-Z (spec §10.1).
    Stop,
}
```

`state.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen { Accounts, History }      // part B inserts Tokens between them

pub struct State {
    pub screen: Screen,
    pub size: (u16, u16),
    pub color: bool,
    pub provider: Option<ProviderId>,
    pub accounts: crate::screens::accounts::AccountsState,
    pub history: crate::screens::history::HistoryState,
    pub overlay: Option<crate::actions::Overlay>,
    pub toast: Option<Toast>,
    /// A failed read's error, in the CLI's words (spec §10.4).
    pub banner: Option<String>,
    /// The action running now, in the TUI or suspended (spec §7.2).
    pub running: Option<Action>,
    pub gens: Gens,
    pub refresh: Refresh,
    /// Set once the shutdown has begun: the exit code it will end with.
    pub quitting: Option<i32>,
    /// Whether the next loop iteration must redraw.
    pub dirty: bool,
}
pub struct Toast { pub text: String, pub until_ms: i64 }
#[derive(Default)]
pub struct Gens { pub accounts: u64, pub history: u64, pub collect: u64 }
pub struct Refresh { pub next_ms: i64 }

impl State {
    pub fn new(screen: Screen, color: bool, provider: Option<ProviderId>, now_ms: i64) -> State;
    /// The first `Cmd`s: load `accounts` (generation 1). History is read once that result names
    /// an account.
    pub fn start(&mut self) -> Vec<Cmd>;
}
```

`update.rs`:

```rust
pub fn update(state: &mut State, msg: Msg, now_ms: i64) -> Vec<Cmd>;
/// Task 8 (R19): the runtime calls it after each frame it draws. The first frame that shows
/// accounts, once no action runs, asks for the first `CollectDue`; then nothing.
pub fn drawn(state: &mut State) -> Vec<Cmd>;
```

Global keys (Task 5):
- **Always:** Ctrl-C gives `Cmd::Quit { code: 0 }`, and Ctrl-Z gives `Cmd::Stop`.
- **While an overlay is open:** every other key goes to it (R9).
- **With no overlay open:**
  - `1`, `3`, `Tab` and `Shift-Tab` switch screens (`2` does nothing until part B);
  - `?` opens help;
  - `q` gives `Cmd::Quit { code: 0 }`;
  - every other key goes to the current screen's `handle_key`.
- **While `quitting` is set,** keys are ignored.

`view.rs`:

```rust
pub fn view(state: &State, frame: &mut ratatui::Frame, now_ms: i64);
```

It draws the header (the tabs, and `○ refresh 30s` as the mode; part C adds live mode), the banner, the body, the activity line slot (empty in part A), the footer (the screen's keys, or the toast, or the running action's spinner, or `finishing <action>…` once `quitting` is set), and the overlay. The banner and a toast wrap onto up to three rows, taken from the body (R21). Below 80×24 it draws only `terminal too small (80×24 needed)`.

### C6. Screens (Tasks 6, 7)

`screens/mod.rs`: `pub mod accounts; pub mod history;`

Each screen module exposes:

```rust
#[derive(Default)] pub struct AccountsState { /* Task 6 */ }
pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd>;
pub fn render(state: &State, frame: &mut Frame, area: Rect, now_ms: i64);
```

The same holds for `HistoryState` in `history.rs`. Task 5 creates both modules with a `Default` state, a `handle_key` that returns no commands, and a `render` that draws nothing. Tasks 6 and 7 replace them. Both screens' `handle_key` take `J`/`K` to scroll windows that do not fit (R20).

`AccountsState` (Task 6) must provide the following, which Tasks 8 and 9 use:

```rust
impl AccountsState {
    pub fn snapshot(&self) -> Option<&AccountsSnapshot>;
    pub fn apply(&mut self, snap: AccountsSnapshot);          // keeps the selection by AccountId (Review Focus 5)
    pub fn selected(&self) -> Option<AccountRef>;
    pub fn shown_ids(&self) -> Vec<AccountId>;                 // every account shown, for collect_due
}
```

`HistoryState` (Task 7) must provide:

```rust
impl HistoryState {
    pub fn account(&self) -> Option<&AccountId>;
    pub fn since_s(&self, now_s: i64) -> i64;                 // now − the selected range (1, 7, 14 or 30 days; default 7)
    pub fn apply(&mut self, view: HistoryView);
}
```

### C7. Actions in the TUI (Tasks 5, 9)

`actions.rs` owns `Overlay` and the action keys.
- **Task 5** defines `Overlay`, `ConfirmThen` and `TextThen` exactly as listed below, and the four functions. It implements `Help` (`?` or `Esc` closes it) and `Confirm` (`Reply` sends the answer; `Run` returns `Cmd::Act` on a yes), so that Review Focus 1 can be tested. `Esc` closes the other overlays, sending `Answer::Choose(None)` for a choose dialog. `handle_action_key` returns `None`. `on_prompt` answers every prompt as a decline. `on_done` sets a toast with the first line of the output.
- **Task 9** implements the rest.

The contract:

```rust
pub enum Overlay {
    Help,
    Confirm { question: String, default_yes: bool, choice: bool, then: ConfirmThen },
    Choose { question: String, options: Vec<String>, selected: usize, reply: Sender<Answer> },
    Text { prompt: String, input: String, then: TextThen },
    Result { title: String, text: String, scroll: u16 },
    Doctor { text: String, scroll: u16, online: bool },
}
pub enum ConfirmThen { Reply(Sender<Answer>), Run(Action) }
pub enum TextThen { Alias(AccountRef), Move(AccountRef) }
pub fn handle_overlay_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd>;
pub fn handle_action_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Option<Vec<Cmd>>; // None: not an action key
pub fn on_prompt(state: &mut State, prompt: Prompt);
pub fn on_done(state: &mut State, action: Action, outcome: ActionOutcome, now_ms: i64) -> Vec<Cmd>;
pub fn render_overlay(state: &State, frame: &mut Frame, now_ms: i64);
```

`ask.rs`:

```rust
pub(crate) struct ChannelAsk { tx: Sender<Msg>, shutdown: Cancel }
impl ChannelAsk { pub(crate) fn new(tx: Sender<Msg>, shutdown: Cancel) -> ChannelAsk; }
impl Ask for ChannelAsk { /* sends Msg::Prompt and waits in 100 ms slices; once `shutdown` is set, declines at once and without showing anything (spec §10.2 step 3) */ }
```

### C8. Runtime (Task 11)

```rust
pub struct TuiOptions {
    pub screen: Screen,
    pub color: bool,
    pub provider: Option<ProviderId>,
    /// The process's signal token: the CLI's handlers record SIGINT, SIGTERM and SIGHUP in it.
    pub signal: Cancel,
    /// The shutdown token. The binary has already built every lane's engine, and the host's
    /// in-TUI context, with it.
    pub shutdown: Cancel,
}
pub struct TuiExit {
    pub code: i32,
    /// Printed on stderr after the terminal is restored (spec §10.2 step 6).
    pub report: Option<String>,
}
pub fn run(opts: TuiOptions, sources: SourceFactory, host: Arc<dyn Host>) -> TuiExit;
```

### C9. The binary's side (Tasks 10, 12)

- `crates/tagteam/src/tui_host.rs`:

  ```rust
  pub struct CliHost {          // `pub mod tui_host;` in lib.rs: its tests are a crate of their own
      /// `env.cancel` is the shutdown token.
      inside: Context,
      /// `env.cancel` is the signal token.
      foreground: Context,
      exe: PathBuf,
  }
  impl CliHost {
      /// `exe` is the binary that `Run` spawns as `<exe> --provider P --expect-account ID run N`:
      /// `std::env::current_exe()` in production, `env!("CARGO_BIN_EXE_tagteam")` in tests.
      pub fn new(ctx: &Context, shutdown: Cancel, exe: PathBuf) -> CliHost;
  }
  impl tagteam_tui::host::Host for CliHost { … }
  ```

  - `act` parses `action.args()` with `Cli::try_parse_from`, and runs `app::run` with captured
    output and a prompter whose `confirm` forwards to `ask`. The one exception is the unlock
    question (`app::UNLOCK_QUESTION`, made `pub(crate)`). For that question the prompter
    declines, without asking, and remembers it, and `act` then returns `NeedsTerminal`.
  - `act_foreground` handles two cases:
    - **`Run`:** it spawns `exe` in the terminal's foreground process group, waits in slices
      (`waitpid` with `WNOHANG`, the signal token checked between slices), and forwards
      SIGTERM and SIGHUP to the child as they arrive (spec §7.3). It gives back a SIGINT
      recorded while the child ran with `Cancel::take` (the child's own).
    - **Any other action:** it runs `app::run` with `TtyPrompter` and the real stdout and stderr.

    Either way, it then spends a SIGINT recorded by now with `Cancel::take_if(SIGINT)` (R15),
    and prints `Press Enter to return to tagteam` on stderr. It waits with
    `prompt::read_terminal_line(&signal)`, unless a SIGTERM or SIGHUP is already recorded.

- `crates/tagteam/src/tui_cmd.rs`: `pub(crate) fn run(cli: &Cli, ctx: Context) -> i32`. It applies the refusals, installs `signals::survive_quit`, builds the `SourceFactory` (each call builds an engine as `build_engine` does, with `env.cancel` set to the shutdown token), calls `tagteam_tui::run`, and prints `TuiExit.report` on stderr.
- `Command::Tui { screen: Option<ScreenArg> }`, where `ScreenArg` is `accounts | history` (part B adds `tokens`). Bare `tagteam`, when stdin and stdout are terminals and `--json` is not given, is `tui` (spec §3).
- `logging::LogConfig` gains `stderr: bool`. `tui` passes `false`, so `--debug` and ERROR events go to the log file only (spec §10.3).

## Tasks

| # | Task | Depends on |
|---|---|---|
| 1 | Shared words in `tagteam_engine::words` | — |
| 2 | `--expect-account` and the stale-selection refusal | — |
| 3 | The `tagteam-tui` crate, its ports, and the MSRV | 2 |
| 4 | Lanes | 3 |
| 5 | State machine core and chrome | 1, 4 |
| 6 | Accounts screen | 1, 5 |
| 7 | History screen | 1, 5, 6 |
| 8 | `EngineSource` and the timer refresh | 4, 6, 7 |
| 9 | Actions, dialogs and panels in the TUI | 6, 8 |
| 10 | `CliHost` | 2, 3 |
| 11 | Runtime: terminal, input, loop, suspension, shutdown | 4, 5, 8, 9, 10 |
| 12 | The `tui` command, bare `tagteam`, docs | 11 |
| 13 | End-to-end pty tests and the first-frame timing | 12 |

Every task runs alone, in table order. No pair qualifies as a batch, because each pair's tests build the other's files: Tasks 1 and 2 both edit the `tagteam` crate, Task 10's tests build `tagteam-tui`, which Task 4 edits, and Tasks 6 and 7 share a crate.

### Task 1: Shared words in `tagteam_engine::words`

TUI spec §5.2 says the Accounts table "says what `list` says", and ruling R3 gets there by moving the words, not by copying them: the helpers `list` and `history` print move into one engine module that both the CLI and (from Task 6 on) the TUI call. Nothing the CLI prints changes by a byte; its own tests are the guard.

**What this task commits to:**
- **Bodies are moved, not rewritten.** `email`, `name`, `countdown`, `money`, `spend_text`, `usage_words` (`render::words`), `window_columns` (`render::columns`), `rate_signed` (`history::signed`) and `projection_text` (`history::projection`) keep their bodies. The constants `MISSING`, `AHEAD`, `IN_SESSION`, `THIS` and `SPEND_HEAD` move with their doc comments.
- **Colour stays in the CLI.** `severity` returns `Severity`; `render.rs` keeps `CRITICAL`, `WARNING` and `RESET`, and maps one to the other in a new `render::severity_code`, which `render.rs`, `statusline.rs` and `auto.rs` call where they called `severity()`. `render::spend_shown` and its table cell stay in `render.rs` too: they colour.
- **`row_notes` folds in two places.** Today `render::row` pushes the status words in two branches (no reading: always; a reading: only when the status is not `ok`), then the kind, `disabled` and `this`. `words::row_notes` does the same, in the same order, from the same condition (`windows` absent or empty, or status not `ok`). `render::row` then only builds cells.
- **`render::email` and `render::name` remain callable.** `render.rs` re-exports them (`pub use tagteam_engine::words::{email, name}`). They have eleven call sites in `app.rs` and `transfer_cmd.rs`, and Task 2 edits `app.rs`; leaving those files alone keeps Tasks 1 and 2 file-disjoint, so they can run as one batch.
- **The usage view carries the next poll (R7).** Spec §5.2's detail pane shows the next poll, which `UsageView` lacks: its `retry_at` is set only for a status other than `ok`. Cycle A adds `UsageView.next_poll_at`, the stored `next_poll_at` filtered by `plan_is_skewed` as `retry_at`'s is, and `None` once it has passed. Nothing the CLI prints reads it, so `list`, `status` and `--json` do not change.
- **Tests move with the code.** Every moved function's unit tests move to `words.rs`, and the edge rows the brief names are added: `countdown` before, at and after the reset; `money` for EUR, USD, GBP and another code, whole and fractional; `usage_words` for every `UsageStatus` arm with and without a retry; `severity` at 69, 70, 89 and 90; `rate_signed`'s four bands; `projection_text`'s branches; `window_columns`' order and its always-present SPEND column.

**Depends on:** none (the task table's `—`)

**Files:**
- Modify: `crates/tagteam-engine/src/views.rs` (`UsageView`, lines 57–76; `UsageView::unread`, 78–92; `usage_view`'s `Ok(UsageView { … })`, 330–346)
- Create: `crates/tagteam-engine/src/words.rs`
- Modify: `crates/tagteam-engine/src/lib.rs` (add `pub mod words;` after `pub mod views;`, today's line 39)
- Modify: `crates/tagteam/src/render.rs` (imports 1–11; constants 27–38; `email`/`name` 43–54; `duration`/`countdown`/`severity`/`money` 137–177; `spend_cell` 187–194; `spend_text` 196–205; `pct_cell` 242–254; `words` 269–298; `Column`/`columns` 300–361; `row` 375–425; `table` line 429; `usage_line` line 581; tests)
- Modify: `crates/tagteam/src/history.rs` (imports 9; `A_YEAR_S` 12–13; `block` 75–94; `signed`/`projection` 96–143; callers at 58, 60, 65, 178; four tests)
- Modify: `crates/tagteam/src/statusline.rs` (import 26; calls at 108, 110, 146, 155, 173)
- Modify: `crates/tagteam/src/auto.rs` (import 28; call at 687)
- Modify: `crates/tagteam/src/displaced_cmd.rs` (import 10)
- Test: `crates/tagteam-engine/tests/views_usage.rs` (one new test); `crates/tagteam-engine/src/words.rs` (new unit tests); `crates/tagteam/src/render.rs` (one new test; `testutil::{unread, read}` gain the new field)

**Interfaces:**
- Consumes: `tagteam_core::rank::span`; `tagteam_core::{Window, WindowKind}`; `tagteam_engine::store::AccountRow`; `tagteam_engine::views::{AccountView, HistoryWindow, UsageStatus, UsageView, NO_DATA}`.
- Produces: `UsageView.next_poll_at: Option<i64>` (R7), which Task 6's detail pane shows. C1, exactly: `words::{MISSING, AHEAD, IN_SESSION, THIS, SPEND_HEAD}`, `email`, `name`, `account_cell`, `duration`, `countdown`, `Severity`, `severity`, `money`, `spend_text`, `usage_words`, `WindowColumn`, `window_columns`, `row_notes`, `rate_signed`, `projection_text`. Binary-internal, not in the contract: `render::severity_code(pct: i64) -> Option<&'static str>`, and `render::{email, name}` as re-exports.

**Spec:** R3 (shared words); R7 and TUI spec §5.2 (the detail pane's next poll); core §8.4 (a plan further out than the budget allows is clock skew) and §8.6 (the poll plan); TUI spec §5.2 (the table says what `list` says); core §13.1 (`list`'s columns, notes and `▶`/`this`), core §13.4 (`history`'s text), core §8.7 (pace and projection), core §13.5 (severities: ≥ 70 warning, ≥ 90 critical).

#### Cycle A: the next poll

- [ ] **Step 1: Write the failing test**

In `crates/tagteam-engine/tests/views_usage.rs`, add this test before `a_clock_skewed_plan_is_not_a_plan_in_force_for_trust`. It uses the file's own helpers (`record`, `replan`, `at`, `usage_of`, `reading`):

```rust
#[test]
fn the_next_poll_is_shown_while_it_is_ahead_and_not_clock_skew() {
    // TUI spec §5.2's detail pane shows the next poll, whatever the status: an `ok` reading has
    // no retry time, but its next poll is planned all the same.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let skewed = fx.add("b@x.co", "rt-b");
    let unread = fx.add("c@x.co", "rt-c");
    record(&fx, &a, &reading(T0, 9.0, 40.0), T0, T0 + 180);
    record(&fx, &skewed, &reading(T0, 9.0, 40.0), T0, T0 + 86_400);
    at(&fx, T0 + 60);
    let u = usage_of(&fx.engine, &a);
    assert_eq!(
        (u.status, u.retry_at, u.next_poll_at),
        (UsageStatus::Ok, None, Some(T0 + 180))
    );
    // A plan a day ahead is clock skew, as `retry_at` treats it (§8.4).
    assert_eq!(usage_of(&fx.engine, &skewed).next_poll_at, None);
    // Never read, so nothing is planned.
    assert_eq!(usage_of(&fx.engine, &unread).next_poll_at, None);
    // A re-plan moves it, and once it has passed nothing is ahead.
    replan(&fx, &a, T0 + 1_200);
    assert_eq!(usage_of(&fx.engine, &a).next_poll_at, Some(T0 + 1_200));
    at(&fx, T0 + 1_200);
    assert_eq!(usage_of(&fx.engine, &a).next_poll_at, None);
}
```

- [ ] **Step 2: Run it and see it fail**

Run: `cargo test -p tagteam-engine --test views_usage the_next_poll`

Expected: the test target does not compile: `error[E0609]: no field `next_poll_at` on type `UsageView`` (five times).

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/views.rs`:

1. `UsageView` (lines 57–76) gains a field after `retry_at`:

```rust
    /// The next planned poll (§8.6), whatever the status, while it is still ahead: the TUI's
    /// detail pane shows it (TUI spec §5.2). A plan `reserve_usage` ignores as clock skew (§8.4)
    /// is none.
    pub next_poll_at: Option<i64>,
```

2. In `UsageView::unread` (line 82's `UsageView { … }`), add `next_poll_at: None,` after `retry_at: None,`.
3. In `Engine::usage_view`, the `Ok(UsageView { … })` (line 330) ends with the new field after `retry_at`'s expression, filtered as `retry_at`'s plan is:

```rust
            next_poll_at: state
                .next_poll_at
                .filter(|&at| at > now_s && !plan_is_skewed(at, now_s, budget)),
```

In `crates/tagteam/src/render.rs`, the two `UsageView` literals in `mod testutil` gain the field: in `unread` (line 816) add `next_poll_at: None,` after `retry_at: retry_in.map(|s| NOW + s),`, and in `read` (line 892) add `next_poll_at: None,` after `retry_at: None,`. These four are the only `UsageView { … }` literals in the workspace: `statusline.rs`'s tests build theirs with `render::testutil::read`, and `tests/views_usage.rs` with `..listed.clone()`, so both compile unchanged.

- [ ] **Step 4: Run it and see it pass**, then the crate's whole suite

Run: `cargo test -p tagteam-engine --test views_usage`

Expected: PASS (31 tests).

Run: `cargo test -p tagteam-engine`, then `cargo build -p tagteam --all-targets --features test-support`

Expected: PASS, and the binary builds: nothing it prints reads the field.

- [ ] **Step 5: Format and lint**

```sh
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```sh
git add crates/tagteam-engine/src/views.rs crates/tagteam-engine/tests/views_usage.rs crates/tagteam/src/render.rs
git commit -m "Show the next planned poll in the usage view"
```

#### Cycle B: the module

- [ ] **Step 7: Write the failing tests**

Add the module to the engine's `lib.rs`, after `pub mod views;`:

```rust
pub mod views;
pub mod words;
```

Create `crates/tagteam-engine/src/words.rs` holding only this test module. It carries the moved tests (`durations_read_as_days_hours_or_minutes`, the two `money` tests, the status-words test, `a_quiet_windows_rate_keeps_two_decimals`, and the three projection tests) and the new ones. Its fixtures (`row`, `view`, `unread`, `window`, `spend`, `read`) are small copies of `render::testutil`'s, because that module belongs to the binary crate.

```rust
#[cfg(test)]
mod tests {
    use serde_json::json;
    use tagteam_core::{AccountId, CLAUDE_CODE, Pace, ProjectionMethod, ProviderId};
    use tagteam_provider::KindTraits;

    use super::*;

    /// Any fixed instant, in seconds: every reading and reset below is placed relative to it.
    const NOW: i64 = 1_790_000_000;
    const HOUR: i64 = 3_600;

    const OAUTH: KindTraits = KindTraits {
        refreshable: true,
        managed_key_axis: false,
        default_email_prefix: None,
        display: None,
    };

    const SETUP_TOKEN: KindTraits = KindTraits {
        refreshable: false,
        managed_key_axis: false,
        default_email_prefix: Some("setup-token"),
        display: Some("setup token"),
    };

    fn row(position: u32, email: &str) -> AccountRow {
        AccountRow {
            id: AccountId::from_string(format!("id-{position}")),
            provider: ProviderId::new(CLAUDE_CODE),
            position,
            identity_key: format!("{email}\n"),
            label: email.into(),
            email: Some(email.into()),
            org_uuid: String::new(),
            org_name: None,
            account_uuid: None,
            kind: "oauth".into(),
            alias: None,
            disabled: false,
            identity_json: json!({}),
            login_expires_at: None,
            login_epoch: 0,
            replacing_fp: None,
            quarantine_reason: None,
            quarantine_fp: None,
            quarantine_at: None,
            added_at: 1,
        }
    }

    fn view(position: u32, email: &str, kind: KindTraits, usage: UsageView) -> AccountView {
        AccountView {
            row: row(position, email),
            active: false,
            kind,
            usage,
            in_session: false,
        }
    }

    /// No reading: `status`, with `error` and a retry `retry_in` seconds from now.
    fn unread(status: UsageStatus, error: Option<&str>, retry_in: Option<i64>) -> UsageView {
        UsageView {
            status,
            windows: None,
            decision_grade: false,
            fetched_at: None,
            age_s: None,
            error: error.map(str::to_owned),
            retry_at: retry_in.map(|s| NOW + s),
            next_poll_at: None,
        }
    }

    fn window(
        key: &str,
        label: &str,
        kind: WindowKind,
        pct: f64,
        resets_in: Option<i64>,
    ) -> Window {
        Window {
            key: key.into(),
            label: label.into(),
            kind,
            pct,
            resets_at: resets_in.map(|s| NOW + s),
            period_s: None,
            detail: None,
        }
    }

    fn spend(used: f64, limit: f64, currency: &str) -> Window {
        Window {
            detail: Some(json!({"used": used, "limit": limit, "currency": currency})),
            ..window(
                "spend",
                "spend",
                WindowKind::Spend,
                used / limit * 100.0,
                None,
            )
        }
    }

    /// A good reading of `windows`, none of them ahead of pace.
    fn read(windows: Vec<Window>) -> UsageView {
        UsageView {
            status: UsageStatus::Ok,
            windows: Some(windows.into_iter().map(|w| (w, Pace::default())).collect()),
            decision_grade: true,
            fetched_at: Some(NOW - 120),
            age_s: Some(120),
            error: None,
            retry_at: None,
            next_poll_at: None,
        }
    }

    fn five_hour() -> Window {
        window("5h", "5h", WindowKind::Short, 9.0, Some(9_630))
    }

    fn seven_day() -> Window {
        window("7d", "7d", WindowKind::Long, 77.0, Some(291_630))
    }

    fn fable() -> Window {
        window(
            "scoped:Fable",
            "Fable",
            WindowKind::Scoped,
            0.0,
            Some(291_630),
        )
    }

    fn history_window(window: Window, pace: Pace) -> HistoryWindow {
        HistoryWindow {
            window,
            samples: vec![],
            pace,
        }
    }

    #[test]
    fn the_names_of_a_row_are_its_alias_email_label_and_organisation() {
        let mut r = row(1, "a@x.co");
        assert_eq!(
            (email(&r), name(&r), account_cell(&r)),
            ("a@x.co".into(), "a@x.co".into(), "a@x.co".into())
        );
        r.alias = Some("work".into());
        assert_eq!(name(&r), "work (a@x.co)");
        r.org_name = Some("Acme".into());
        assert_eq!(account_cell(&r), "work (a@x.co) [Acme]");
        // A token account has no email: its label stands in.
        r.email = None;
        r.label = "api-key-4@token.local".into();
        assert_eq!(email(&r), "api-key-4@token.local");
        assert_eq!(name(&r), "work (api-key-4@token.local)");
    }

    #[test]
    fn durations_read_as_days_hours_or_minutes() {
        for (secs, text) in [
            (-5, "<1m"),
            (0, "<1m"),
            (59, "<1m"),
            (60, "1m"),
            (3_599, "59m"),
            (3_600, "1h00m"),
            (9_630, "2h40m"),
            (86_399, "23h59m"),
            (86_400, "1d00h"),
            (291_630, "3d09h"),
        ] {
            assert_eq!(duration(secs), text, "{secs}");
        }
    }

    #[test]
    fn a_countdown_before_at_and_after_the_reset() {
        assert_eq!(countdown(NOW + 9_630, NOW), "2h40m", "before");
        assert_eq!(countdown(NOW + 60, NOW), "1m");
        assert_eq!(
            countdown(NOW + 1, NOW),
            "<1m",
            "a second ahead is still ahead"
        );
        assert_eq!(countdown(NOW, NOW), "reset", "at");
        assert_eq!(countdown(NOW - 1, NOW), "reset", "after");
        assert_eq!(countdown(NOW - 86_400, NOW), "reset");
    }

    #[test]
    fn severity_changes_at_70_and_90() {
        for (pct, expected) in [
            (-5, Severity::Normal),
            (0, Severity::Normal),
            (69, Severity::Normal),
            (70, Severity::Warning),
            (89, Severity::Warning),
            (90, Severity::Critical),
            (100, Severity::Critical),
            (250, Severity::Critical),
        ] {
            assert_eq!(severity(pct), expected, "{pct}");
        }
    }

    #[test]
    fn money_uses_the_symbol_or_the_code() {
        assert_eq!(money(0.0, "EUR"), "€0");
        assert_eq!(money(20.0, "eur"), "€20");
        assert_eq!(money(12.5, "USD"), "$12.50");
        assert_eq!(money(3.0, "GBP"), "£3");
        assert_eq!(money(7.25, "CHF"), "7.25 CHF");
        assert_eq!(
            money(7.0, "chf"),
            "7 CHF",
            "a code is shown in capitals, whole without cents"
        );
    }

    #[test]
    fn money_rounds_to_cents_before_dropping_a_whole_amounts_cents() {
        assert_eq!(money(12.999, "EUR"), "€13");
        assert_eq!(money(0.004, "EUR"), "€0");
        assert_eq!(money(19.996, "USD"), "$20");
        assert_eq!(money(0.006, "EUR"), "€0.01");
        assert_eq!(money(12.994, "EUR"), "€12.99");
        assert_eq!(money(-0.004, "EUR"), "€0");
    }

    #[test]
    fn spend_text_needs_a_spend_window_with_its_amounts() {
        assert_eq!(
            spend_text(&spend(0.0, 20.0, "EUR")).as_deref(),
            Some("€0 of €20")
        );
        assert_eq!(
            spend_text(&spend(3.5, 20.0, "USD")).as_deref(),
            Some("$3.50 of $20")
        );
        let mut no_currency = spend(1.0, 2.0, "EUR");
        no_currency.detail = Some(json!({"used": 1.0, "limit": 2.0}));
        assert_eq!(spend_text(&no_currency), None);
        let mut no_detail = spend(1.0, 2.0, "EUR");
        no_detail.detail = None;
        assert_eq!(spend_text(&no_detail), None);
        let mut not_spend = spend(1.0, 2.0, "EUR");
        not_spend.kind = WindowKind::Scoped;
        assert_eq!(
            spend_text(&not_spend),
            None,
            "only a spend window has amounts"
        );
    }

    #[test]
    fn every_status_says_when_it_is_retried_while_that_is_ahead() {
        let words_of = |status, error: Option<&str>, retry_in: Option<i64>| {
            usage_words(&unread(status, error, retry_in), NOW)
        };
        for (status, text) in [
            (UsageStatus::Ok, "no usage reported"),
            (UsageStatus::KeychainUnavailable, "keychain unavailable"),
            (UsageStatus::TokenExpired, "token expired"),
            (UsageStatus::NoCredentials, "no credentials"),
            (UsageStatus::ForeignCredential, "foreign credential"),
            (UsageStatus::ReloginRequired, "relogin required"),
            (UsageStatus::ApiKey, "api key"),
            (UsageStatus::Unsupported, "usage unsupported"),
        ] {
            assert_eq!(
                words_of(status, None, Some(120)),
                format!("{text} (retry 2m)")
            );
            assert_eq!(words_of(status, None, None), text);
            assert_eq!(
                words_of(status, None, Some(-5)),
                text,
                "a passed retry says nothing"
            );
            assert_eq!(
                words_of(status, None, Some(0)),
                text,
                "a retry that is due now says nothing"
            );
        }
        assert_eq!(
            words_of(UsageStatus::Unavailable, Some("over-budget"), Some(2_400)),
            "over budget (retry 40m)"
        );
        assert_eq!(
            words_of(UsageStatus::Unavailable, Some("over-budget"), None),
            "over budget"
        );
        assert_eq!(
            words_of(UsageStatus::Unavailable, Some(NO_DATA), Some(30)),
            "no data yet (retry <1m)"
        );
        assert_eq!(
            words_of(UsageStatus::Unavailable, None, None),
            "no data yet"
        );
        assert_eq!(
            words_of(UsageStatus::Unavailable, Some("http-429"), Some(330)),
            "unavailable (http-429, retry 5m)"
        );
        assert_eq!(
            words_of(UsageStatus::Unavailable, Some("http-429"), None),
            "unavailable (http-429)"
        );
    }

    #[test]
    fn columns_are_the_windows_some_reading_has_in_kind_order_and_spend_is_always_there() {
        assert_eq!(window_columns(&[]), vec![], "no accounts, no columns");
        let unread_only = view(
            1,
            "a@x.co",
            OAUTH,
            unread(UsageStatus::Unavailable, None, None),
        );
        assert_eq!(
            window_columns(&[unread_only.clone()]),
            vec![],
            "no reading, no columns"
        );
        // Scoped first in the reading and the 7d before the 5h: the kinds decide the order.
        let reading = read(vec![fable(), seven_day(), five_hour()]);
        let heads = |cols: Vec<WindowColumn>| -> Vec<(Option<String>, String)> {
            cols.into_iter().map(|c| (c.key, c.head)).collect()
        };
        assert_eq!(
            heads(window_columns(&[
                unread_only.clone(),
                view(2, "b@x.co", OAUTH, reading)
            ])),
            [
                (Some("5h".to_owned()), "5H".to_owned()),
                (Some("7d".to_owned()), "7D".to_owned()),
                (None, "SPEND".to_owned()),
                (Some("scoped:Fable".to_owned()), "FABLE".to_owned()),
            ],
            "SPEND is a column of its own even with no spend window"
        );
        // A spend window is a column of its own key, in the same place; the placeholder goes.
        let with_spend = read(vec![five_hour(), spend(0.0, 20.0, "EUR"), fable()]);
        let cols = window_columns(&[view(1, "a@x.co", OAUTH, with_spend)]);
        assert_eq!(
            cols.iter()
                .map(|c| (c.key.as_deref(), c.kind))
                .collect::<Vec<_>>(),
            [
                (Some("5h"), WindowKind::Short),
                (Some("spend"), WindowKind::Spend),
                (Some("scoped:Fable"), WindowKind::Scoped),
            ]
        );
        // A key two readings share is one column; scoped windows come in first-seen order.
        let a = read(vec![
            five_hour(),
            window("scoped:Zed", "Zed", WindowKind::Scoped, 1.0, None),
        ]);
        let b = read(vec![five_hour(), fable()]);
        let cols = window_columns(&[view(1, "a@x.co", OAUTH, a), view(2, "b@x.co", OAUTH, b)]);
        assert_eq!(
            cols.iter().map(|c| c.head.as_str()).collect::<Vec<_>>(),
            ["5H", "SPEND", "ZED", "FABLE"]
        );
    }

    #[test]
    fn a_rows_notes_are_its_status_kind_disabled_and_this_in_that_order() {
        let ok = view(1, "a@x.co", OAUTH, read(vec![five_hour()]));
        assert_eq!(
            row_notes(&ok, false, NOW),
            Vec::<String>::new(),
            "a good reading says nothing"
        );
        assert_eq!(row_notes(&ok, true, NOW), ["this"]);
        // A reading with a status that is not `ok` says it beside the windows.
        let mut locked = read(vec![five_hour()]);
        locked.status = UsageStatus::KeychainUnavailable;
        locked.retry_at = Some(NOW + 120);
        let beside = view(1, "a@x.co", OAUTH, locked);
        assert_eq!(
            row_notes(&beside, false, NOW),
            ["keychain unavailable (retry 2m)"]
        );
        // No reading: the words stand in for the windows, the kind follows, then the marks.
        let mut setup = view(
            4,
            "setup-token-4@token.local",
            SETUP_TOKEN,
            unread(UsageStatus::Unavailable, Some("pre-send"), Some(30)),
        );
        setup.row.disabled = true;
        assert_eq!(
            row_notes(&setup, true, NOW),
            [
                "unavailable (pre-send, retry <1m)",
                "setup token",
                "disabled",
                "this"
            ]
        );
        // A kind the words already say is not said twice.
        let mut key = view(
            5,
            "k@x.co",
            SETUP_TOKEN,
            unread(UsageStatus::Ok, None, None),
        );
        key.kind.display = Some("no usage reported");
        assert_eq!(row_notes(&key, false, NOW), ["no usage reported"]);
        // An empty reading is no reading.
        let empty = view(6, "e@x.co", OAUTH, read(vec![]));
        assert_eq!(row_notes(&empty, false, NOW), ["no usage reported"]);
    }

    #[test]
    fn a_quiet_windows_rate_keeps_two_decimals() {
        for (rate, text) in [
            (0.03, "+0.03"),
            (-0.049, "-0.05"),
            (0.005, "+0.01"),
            (0.004, "+<0.01"),
            (-1e-9, "-<0.01"),
            (0.0, "+0.0"),
            (-0.0, "+0.0"),
            (0.05, "+0.1"),
            (0.099, "+0.1"),
            (0.1, "+0.1"),
            (5.0, "+5.0"),
            (-1.26, "-1.3"),
        ] {
            assert_eq!(rate_signed(rate), text, "{rate}");
        }
    }

    #[test]
    fn a_projection_lasts_runs_out_or_is_missing() {
        let with = |pace: Pace| {
            let w = window("7d", "7d", WindowKind::Long, 30.0, None);
            projection_text(&history_window(w, pace), NOW)
        };
        assert_eq!(with(Pace::default()), "no rate yet");
        let past = Pace {
            rate_per_hour: Some(5.0),
            method: Some(ProjectionMethod::Regression),
            exhaustion_at: Some(NOW - 60),
            ..Pace::default()
        };
        assert_eq!(
            with(past),
            "+5.0 pts/h · should have run out 1m ago (regression)"
        );
        let just_past = Pace {
            exhaustion_at: Some(NOW),
            ..past
        };
        assert_eq!(
            with(just_past),
            "+5.0 pts/h · should have run out <1m ago (regression)"
        );
        let soon = Pace {
            exhaustion_at: Some(NOW + 60),
            ..past
        };
        assert_eq!(with(soon), "+5.0 pts/h · runs out in 1m (regression)");
        let lasts = Pace {
            will_last_to_reset: Some(true),
            ..soon
        };
        assert_eq!(with(lasts), "+5.0 pts/h · lasts to reset (regression)");
        let flat = Pace {
            rate_per_hour: Some(0.0),
            method: Some(ProjectionMethod::Average),
            ..Pace::default()
        };
        assert_eq!(with(flat), "+0.0 pts/h · no projection (average)");
    }

    #[test]
    fn an_exhausted_window_is_at_the_limit_with_or_without_a_rate() {
        let exhausted = |pace: Pace| {
            let w = window("5h", "5h", WindowKind::Short, 100.0, Some(HOUR));
            projection_text(&history_window(w, pace), NOW)
        };
        // Read 100, 100, 100: no slope, no average for a Short window, but exhausted at the read.
        let no_rate = Pace {
            exhaustion_at: Some(NOW - 2 * HOUR),
            ..Pace::default()
        };
        assert_eq!(exhausted(no_rate), "at the limit");
        // With a rate the exhaustion time is the read's, hours ago: no countdown.
        let with_rate = Pace {
            rate_per_hour: Some(5.0),
            method: Some(ProjectionMethod::Average),
            exhaustion_at: Some(NOW - 2 * HOUR),
            will_last_to_reset: Some(false),
            ..Pace::default()
        };
        assert_eq!(exhausted(with_rate), "+5.0 pts/h · at the limit (average)");
    }

    #[test]
    fn a_projection_beyond_a_year_says_it_will_not_run_out() {
        let with = |pace: Pace, reset: Option<i64>| {
            let w = window("7d", "7d", WindowKind::Long, 30.0, reset);
            projection_text(&history_window(w, pace), NOW)
        };
        // A quiet window: 70 points to go at 0.006 an hour is well over a year.
        let tiny = Pace {
            rate_per_hour: Some(0.006),
            method: Some(ProjectionMethod::Regression),
            exhaustion_at: Some(NOW + 42_000_000),
            will_last_to_reset: None,
            ..Pace::default()
        };
        assert_eq!(
            with(tiny, None),
            "+0.01 pts/h · won't run out within a year (regression)"
        );
        // With a reset it will not reach, the reset is what counts (§8.7).
        let lasts = Pace {
            will_last_to_reset: Some(true),
            ..tiny
        };
        assert_eq!(
            with(lasts, Some(HOUR)),
            "+0.01 pts/h · lasts to reset (regression)"
        );
        // 70 points at 0.01 an hour is 291 days: a countdown still.
        let slow = Pace {
            rate_per_hour: Some(0.01),
            exhaustion_at: Some(NOW + 25_200_000),
            ..tiny
        };
        assert_eq!(
            with(slow, None),
            "+0.01 pts/h · runs out in 291d16h (regression)"
        );
        // The year itself is still a countdown; a second more is not.
        let year = |s| Pace {
            exhaustion_at: Some(NOW + 365 * 86_400 + s),
            ..tiny
        };
        assert_eq!(
            with(year(0), None),
            "+0.01 pts/h · runs out in 365d00h (regression)"
        );
        assert_eq!(
            with(year(1), None),
            "+0.01 pts/h · won't run out within a year (regression)"
        );
        // A saturated time must not overflow.
        let saturated = Pace {
            exhaustion_at: Some(i64::MAX),
            ..tiny
        };
        assert_eq!(
            with(saturated, None),
            "+0.01 pts/h · won't run out within a year (regression)"
        );
    }
}
```

- [ ] **Step 8: Run them and see them fail**

Run: `cargo test -p tagteam-engine --lib words::tests`

Expected: the crate's test target does not compile. 111 errors, all of them unresolved names, such as `error[E0425]: cannot find function `countdown` in this scope`, `cannot find function `money``, `cannot find type `AccountRow` in this scope`, and `cannot find type `WindowColumn` in this scope`.

- [ ] **Step 9: Implement**

Put this above the test module in `crates/tagteam-engine/src/words.rs`. The function bodies are `render.rs`'s and `history.rs`'s today, character for character; only `severity`'s return type, the visibility, and the new `account_cell` and `row_notes` are new code.

```rust
//! The words `list`, `status` and `history` print, shared with the TUI so that both say the
//! same things (TUI spec §5.2). They moved here from the CLI's `render.rs` and `history.rs`
//! with their bodies unchanged; the CLI adds its ANSI colours around them.

use tagteam_core::{Window, WindowKind};

use crate::store::AccountRow;
use crate::views::{AccountView, HistoryWindow, NO_DATA, UsageStatus, UsageView};

/// A window a reading does not have, or an age that is not known.
pub const MISSING: &str = "—";
/// §8.7's marker for a window ahead of pace.
pub const AHEAD: &str = "▲ pace";
/// §13.1's marker of a session-owned account, in a one-column field after the position
/// (Decision 11).
pub const IN_SESSION: &str = "▶";
/// The trailing note on the run shell's own row: §13.1's `▶ this`, kept with the notes so the
/// columns stay aligned.
pub const THIS: &str = "this";
/// `list` always has a spend column once any account has a reading (§13.1).
pub const SPEND_HEAD: &str = "SPEND";
/// A projection further out than this is not worth a countdown: it will not run out.
const A_YEAR_S: i64 = 365 * 86_400;

/// The account's email (or label when it has none).
pub fn email(r: &AccountRow) -> String {
    r.email.clone().unwrap_or_else(|| r.label.clone())
}

pub fn name(r: &AccountRow) -> String {
    let email = email(r);
    match &r.alias {
        Some(a) => format!("{a} ({email})"),
        None => email,
    }
}

/// `name`, and the organisation in brackets when the login has one: `list`'s ACCOUNT cell.
pub fn account_cell(r: &AccountRow) -> String {
    match &r.org_name {
        Some(org) => format!("{} [{org}]", name(r)),
        None => name(r),
    }
}

// A span of time as `list` and `status` show it: `3d09h`, `2h40m`, `45m`, or `<1m`. The usage
// strategies' messages state a reset in the same words, so there is one formatter.
pub use tagteam_core::rank::span as duration;

/// The time left until `at`, or `reset` once it has passed: the reading predates the window's
/// reset, so its pct no longer applies.
pub fn countdown(at: i64, now_s: i64) -> String {
    if at <= now_s {
        "reset".into()
    } else {
        duration(at - now_s)
    }
}

/// §13.5's severities, which `list`, `status` and the TUI share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Normal,
    Warning,
    Critical,
}

/// ≥ 90 critical, ≥ 70 warning.
pub fn severity(pct: i64) -> Severity {
    if pct >= 90 {
        Severity::Critical
    } else if pct >= 70 {
        Severity::Warning
    } else {
        Severity::Normal
    }
}

/// An amount with its currency's symbol (€, $, £), or its code for any other; a whole amount
/// without cents.
pub fn money(amount: f64, currency: &str) -> String {
    let cents = format!("{amount:.2}");
    let n = match cents.strip_suffix(".00") {
        Some("-0") => "0",
        Some(whole) => whole,
        None => &cents,
    }
    .to_owned();
    match currency.to_ascii_uppercase().as_str() {
        "EUR" => format!("€{n}"),
        "USD" => format!("${n}"),
        "GBP" => format!("£{n}"),
        code => format!("{n} {code}"),
    }
}

/// `€0 of €20`, from a spend window's `detail {used, limit, currency}` (§8.2).
pub fn spend_text(w: &Window) -> Option<String> {
    let d = w.detail.as_ref().filter(|_| w.kind == WindowKind::Spend)?;
    let currency = d["currency"].as_str()?;
    Some(format!(
        "{} of {}",
        money(d["used"].as_f64()?, currency),
        money(d["limit"].as_f64()?, currency)
    ))
}

/// A usage status in words (§13.1): in place of a row's windows when it has no reading, and
/// beside them when its status is not `ok`. Whatever the status, a retry still ahead is said:
/// `keychain unavailable (retry 2m)`, `unavailable (http-429, retry 5m)`.
pub fn usage_words(u: &UsageView, now_s: i64) -> String {
    let (head, why) = match u.status {
        UsageStatus::Ok => ("no usage reported", None),
        UsageStatus::TokenExpired => ("token expired", None),
        UsageStatus::ApiKey => ("api key", None),
        UsageStatus::KeychainUnavailable => ("keychain unavailable", None),
        UsageStatus::ReloginRequired => ("relogin required", None),
        UsageStatus::ForeignCredential => ("foreign credential", None),
        UsageStatus::NoCredentials => ("no credentials", None),
        UsageStatus::Unsupported => ("usage unsupported", None),
        UsageStatus::Unavailable => match u.error.as_deref() {
            None | Some(NO_DATA) => ("no data yet", None),
            Some("over-budget") => ("over budget", None),
            Some(e) => ("unavailable", Some(e.to_owned())),
        },
    };
    let retry = u
        .retry_at
        .filter(|&at| at > now_s)
        .map(|at| format!("retry {}", duration(at - now_s)));
    let detail: Vec<String> = why.into_iter().chain(retry).collect();
    if detail.is_empty() {
        head.to_owned()
    } else {
        format!("{head} ({})", detail.join(", "))
    }
}

/// One of `list`'s window columns: the window key it shows (`None` for the spend column when
/// no account has spend), headed by the window's label in capitals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowColumn {
    pub key: Option<String>,
    pub kind: WindowKind,
    pub head: String,
}

/// §13.1's window columns for one provider's accounts: every window key some reading has, in
/// kind order (Short, Long, Spend, Scoped) and then by first appearance. SPEND is always a
/// column, `—` throughout when no account has spend: only scoped windows come and go. No
/// columns at all while no account has a reading.
pub fn window_columns(accounts: &[AccountView]) -> Vec<WindowColumn> {
    let windows: Vec<&Window> = accounts
        .iter()
        .filter_map(|v| v.usage.windows.as_ref())
        .flatten()
        .map(|(w, _)| w)
        .collect();
    let mut cols: Vec<WindowColumn> = Vec::new();
    if windows.is_empty() {
        return cols;
    }
    for kind in [
        WindowKind::Short,
        WindowKind::Long,
        WindowKind::Spend,
        WindowKind::Scoped,
    ] {
        for w in windows.iter().filter(|w| w.kind == kind) {
            if !cols
                .iter()
                .any(|c| c.key.as_deref() == Some(w.key.as_str()))
            {
                cols.push(WindowColumn {
                    key: Some(w.key.clone()),
                    kind,
                    head: w.label.to_uppercase(),
                });
            }
        }
        if kind == WindowKind::Spend && !windows.iter().any(|w| w.kind == WindowKind::Spend) {
            cols.push(WindowColumn {
                key: None,
                kind,
                head: SPEND_HEAD.into(),
            });
        }
    }
    cols
}

/// The notes that follow a row's cells: its status in words (in place of the windows when it
/// has no reading, beside them when the status is not `ok`), its kind, `disabled`, and `this`
/// when the row is the run shell's own account (§13.1).
pub fn row_notes(v: &AccountView, this: bool, now_s: i64) -> Vec<String> {
    let u = &v.usage;
    let mut notes = Vec::new();
    let has_windows = u.windows.as_deref().is_some_and(|ws| !ws.is_empty());
    if !has_windows || u.status != UsageStatus::Ok {
        notes.push(usage_words(u, now_s));
    }
    if let Some(kind) = v.kind.display {
        if !notes.iter().any(|n| n == kind) {
            notes.push(kind.to_owned());
        }
    }
    if v.row.disabled {
        notes.push("disabled".into());
    }
    if this {
        notes.push(THIS.into());
    }
    notes
}

/// A rate in points per hour with its sign: one decimal, or two below 0.05 in magnitude, where
/// one would print a quiet window's rate as zero (`+0.03`, not `+0.0`). A rate too small for
/// two decimals is `<0.01`, never a zero beside a finite runway; only exactly 0 is `+0.0`.
pub fn rate_signed(rate: f64) -> String {
    let magnitude = rate.abs();
    if magnitude == 0.0 {
        "+0.0".to_owned()
    } else if magnitude < 0.005 {
        format!("{}<0.01", if rate < 0.0 { '-' } else { '+' })
    } else if magnitude < 0.05 {
        format!("{rate:+.2}")
    } else {
        format!("{rate:+.1}")
    }
}

/// The burn rate and where it leads (§8.7): `lasts to reset` when the window will, else when it
/// runs out (or `should have run out` when that time has passed, `won't run out within a year`
/// when it is further off than that), with the method that measured the rate.
pub fn projection_text(hw: &HistoryWindow, now_s: i64) -> String {
    let p = &hw.pace;
    let method = p
        .method
        .map_or_else(String::new, |m| format!(" ({})", m.as_str()));
    // An exhausted window ran out when it was read, which may be long ago: no countdown applies.
    if hw.window.pct >= 100.0 {
        return match p.rate_per_hour {
            Some(rate) => format!("{} pts/h · at the limit{method}", rate_signed(rate)),
            None => "at the limit".to_owned(),
        };
    }
    let Some(rate) = p.rate_per_hour else {
        return "no rate yet".to_owned();
    };
    let eta = match (p.will_last_to_reset, p.exhaustion_at) {
        (Some(true), _) => "lasts to reset".to_owned(),
        (_, Some(at)) if at <= now_s => format!(
            "should have run out {} ago",
            duration(now_s.saturating_sub(at))
        ),
        (_, Some(at)) if at.saturating_sub(now_s) > A_YEAR_S => {
            "won't run out within a year".to_owned()
        }
        (_, Some(at)) => format!("runs out in {}", duration(at - now_s)),
        _ => "no projection".to_owned(),
    };
    format!("{} pts/h · {eta}{method}", rate_signed(rate))
}
```

- [ ] **Step 10: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-engine --lib words::tests`

Expected: `test result: ok. 14 passed; 0 failed`.

Run: `cargo test -p tagteam-engine`

Expected: PASS (nothing else in the engine changed).

- [ ] **Step 11: Format and lint**

```sh
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 12: Commit**

```sh
git add crates/tagteam-engine/src/words.rs crates/tagteam-engine/src/lib.rs
git commit -m "Add the words list and history print to the engine"
```

#### Cycle C: the CLI calls them

- [ ] **Step 13: Write the failing test**

In `crates/tagteam/src/render.rs`, in `mod tests` (which has `use super::*;`, so `CRITICAL` and `WARNING` are in scope), add:

```rust
    #[test]
    fn severity_codes_follow_the_shared_severities() {
        // `words::severity` decides; the CLI only maps it to §13.5's colours.
        for (pct, code) in [
            (0, None),
            (69, None),
            (70, Some(WARNING)),
            (89, Some(WARNING)),
            (90, Some(CRITICAL)),
            (100, Some(CRITICAL)),
        ] {
            assert_eq!(severity_code(pct), code, "{pct}");
        }
    }
```

- [ ] **Step 14: Run it and see it fail**

Run: `cargo test -p tagteam --features test-support --lib render::tests::severity_codes`

Expected: `error[E0425]: cannot find function `severity_code` in this scope`.

- [ ] **Step 15: Implement**

Every edit below names a function and today's lines (before any of these edits). `cargo build -p tagteam` is clean after the last one.

**`render.rs`**

1. Replace the `tagteam_engine::views` import (lines 7–9) with these three statements (`NO_DATA` is only used by a test now; the test module imports it in item 11):

```rust
use tagteam_engine::views::{
    AccountView, ProviderAccounts, ShellAccount, StatusView, UsageStatus, UsageView,
};
use tagteam_engine::words::{
    AHEAD, IN_SESSION, MISSING, Severity, WindowColumn, account_cell, countdown, duration,
    row_notes, severity, spend_text, usage_words, window_columns,
};
// `app` and `transfer_cmd` still spell these `render::email` and `render::name`.
pub use tagteam_engine::words::{email, name};
```

2. Delete the constants `MISSING`, `AHEAD`, `SPEND_HEAD`, `IN_SESSION` and `THIS` with their doc comments (lines 27–38). Keep `CRITICAL`, `WARNING` and `RESET` (23–26).
3. Delete `email` and `name` (lines 43–54).
4. Delete the `duration` re-export with its comment, `countdown`, `severity` and `money` (lines 137–177), and put this where `severity` was:

```rust
/// §13.5's severities as the ANSI codes `list`, `status`, `auto` and the statusline colour a
/// percentage with: ≥ 90 critical, ≥ 70 warning.
pub(crate) fn severity_code(pct: i64) -> Option<&'static str> {
    match severity(pct) {
        Severity::Critical => Some(CRITICAL),
        Severity::Warning => Some(WARNING),
        Severity::Normal => None,
    }
}
```

5. In `spend_cell` (line 189) and `pct_cell` (line 246), change `severity(` to `severity_code(`. Delete `spend_text` (lines 196–205): `spend_cell` calls the `words` one.
6. Delete `words` (lines 269–298), and `Column` with its `impl` and `columns` (lines 300–361). Where `Column::missing` was, put:

```rust
/// The cell of a row whose reading lacks `col`'s window: `—`, under the `%` of a percentage
/// column and at the start of the spend column.
fn missing_cell(col: &WindowColumn) -> Cell {
    match col.kind {
        WindowKind::Spend => Cell::text(MISSING),
        _ => Cell::text(format!("{MISSING:>4}")),
    }
}
```

7. Replace `row` (lines 375–425) with:

```rust
/// `this`: the row is the run shell's own account (§13.1).
fn row(v: &AccountView, cols: &[WindowColumn], this: bool, now_s: i64, color: bool) -> Row {
    let u = &v.usage;
    let cells = match u.windows.as_deref() {
        Some(ws) if !ws.is_empty() => {
            let mut cells: Vec<Cell> = cols
                .iter()
                .map(|c| {
                    ws.iter()
                        .find(|(w, _)| c.key.as_deref() == Some(w.key.as_str()))
                        .map_or_else(|| missing_cell(c), |(w, p)| window_cell(w, p, now_s, color))
                })
                .collect();
            cells.push(Cell::text(
                u.age_s.map_or_else(|| MISSING.to_owned(), duration),
            ));
            Some(cells)
        }
        _ => None,
    };
    Row {
        marker: if v.active { '*' } else { ' ' },
        position: v.row.position,
        session: v.in_session,
        account: account_cell(&v.row),
        cells,
        notes: row_notes(v, this, now_s),
    }
}
```

8. In `table` (line 429) change `columns(accounts)` to `window_columns(accounts)`. In `usage_line` (line 581) change `words(u, now_s)` to `usage_words(u, now_s)`.
9. In `mod tests`, delete `durations_read_as_days_hours_or_minutes`, `money_uses_the_symbol_or_the_code` and `money_rounds_to_cents_before_dropping_a_whole_amounts_cents` (they live in `words.rs` now).
10. In `mod tests`, replace `every_status_says_when_it_is_retried_while_that_is_ahead` (its `words_of` half is `words::tests::every_status_says_when_it_is_retried_while_that_is_ahead`) with the half that renders a table:

```rust
    #[test]
    fn a_row_with_a_reading_shows_the_status_words_beside_it() {
        let mut locked = read(900, 31.0, 12.0, false, vec![]);
        locked.status = UsageStatus::KeychainUnavailable;
        locked.retry_at = Some(NOW + 120);
        assert_eq!(
            list_human(
                &one(vec![view(1, "a@x.co", OAUTH, locked)]),
                None,
                &names,
                NOW,
                false
            ),
            concat!(
                "    #  ACCOUNT  5H           7D           SPEND  AGE\n",
                "    1  a@x.co    31%  2h40m   12%  3d09h  —      15m  keychain unavailable (retry 2m)\n",
            )
        );
    }
```

11. In `mod tests`' imports add `use tagteam_engine::views::NO_DATA;` (after `use tagteam_engine::switch::SwitchReason;`): the status-words test above and `a_row_says_in_words_what_keeps_its_reading_from_being_current` use it.

**`history.rs`**

12. Delete `use crate::render;` (line 9) and the blank line above it, add `use tagteam_engine::words;` after the `tagteam_engine::views` import (line 7), and delete `A_YEAR_S` with its doc comment (lines 12–13).
13. Delete `signed` and `projection` (lines 96–143), and replace `block` (lines 75–94) so that it calls the shared words:

```rust
fn block(hw: &HistoryWindow, now_s: i64) -> String {
    let w = &hw.window;
    let mut head = format!("{}  {:.0}%", w.label, w.pct.round());
    if let Some(at) = w.resets_at {
        // As `list` says it: a reset that has passed is `reset`, not a countdown to it.
        let left = words::countdown(at, now_s);
        if at > now_s {
            head.push_str(&format!("  resets in {left}"));
        } else {
            head.push_str(&format!("  {left}"));
        }
    }
    let pcts: Vec<f64> = hw.samples.iter().map(|x| x.pct).collect();
    let samples = match pcts.len() {
        0 => "no samples".to_owned(),
        1 => format!("{}  1 sample", sparkline(&pcts)),
        n => format!("{}  {n} samples", sparkline(&pcts)),
    };
    format!(
        "{head}\n  {samples}\n  {}\n",
        words::projection_text(hw, now_s)
    )
}
```

14. Replace `render::name(` with `words::name(` (lines 58, 60, 65) and `render::email(` with `words::email(` (line 178).
15. Delete the four tests that moved: `a_projection_lasts_runs_out_or_is_missing`, `an_exhausted_window_is_at_the_limit_with_or_without_a_rate`, `a_quiet_windows_rate_keeps_two_decimals` and `a_projection_beyond_a_year_says_it_will_not_run_out`. `history_window` and `view()` stay: `human`'s tests use them.

**`statusline.rs`**

16. Line 26 becomes `use crate::render::{self, RESET};`, and add `use tagteam_engine::words::{self, MISSING};` after the `tagteam_engine::views` import (line 16).
17. `render::email(` becomes `words::email(` (lines 108, 110); `render::countdown(` becomes `words::countdown(` (line 155); `render::duration(` becomes `words::duration(` (line 173); `render::severity(shown_pct as i64)` becomes `render::severity_code(shown_pct as i64)` (line 146). `render::spend_shown` (line 164) stays.

**`auto.rs`**

18. Line 28 becomes `use crate::render::{RESET, severity_code};`, and add `use tagteam_engine::words::{MISSING, duration};` after `use tagteam_engine::settings::{…};` (line 21). In `pct` (line 687) `match severity(n)` becomes `match severity_code(n)`.

**`displaced_cmd.rs`**

19. Line 10 becomes `use crate::render;`, and add `use tagteam_engine::words::MISSING;` after `use tagteam_engine::displace::{DisplacedEntry, DisplacedList};` (line 8).

- [ ] **Step 16: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam --features test-support --lib render::tests::severity_codes`

Expected: `1 passed`.

Run: `cargo test -p tagteam --features test-support`

Expected: PASS, with `list`, `status`, `history` and the statusline printing what they printed before. The `prompt` unit tests and `tests/signals.rs` open a pty, which the sandbox refuses (`openpty failed: Operation not permitted`), so run this outside the sandbox. In a batch the controller runs it once, and each implementer runs `--lib` with `-- --skip prompt::tests`, plus the integration tests that print what it moved (`--test app --test usage_cli --test history --test statusline`).

- [ ] **Step 17: Format and lint**

```sh
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 18: Commit**

```sh
git add crates/tagteam/src/render.rs crates/tagteam/src/history.rs crates/tagteam/src/statusline.rs crates/tagteam/src/auto.rs crates/tagteam/src/displaced_cmd.rs
git commit -m "Print list and history through the engine's shared words"
```

**Existing tests this task moves or changes:** none fails; eight move, and one of them splits. `render::tests::{durations_read_as_days_hours_or_minutes, money_uses_the_symbol_or_the_code, money_rounds_to_cents_before_dropping_a_whole_amounts_cents}` become `words::tests::` of the same name (the first loses its `countdown` lines, which became `a_countdown_before_at_and_after_the_reset`). `render::tests::every_status_says_when_it_is_retried_while_that_is_ahead` splits into `words::tests::` of that name and `render::tests::a_row_with_a_reading_shows_the_status_words_beside_it`. `history::tests::{a_projection_lasts_runs_out_or_is_missing, an_exhausted_window_is_at_the_limit_with_or_without_a_rate, a_quiet_windows_rate_keeps_two_decimals, a_projection_beyond_a_year_says_it_will_not_run_out}` become `words::tests::` of the same name, calling `projection_text` and `rate_signed`.

---

### Task 2: `--expect-account` and the stale-selection refusal

The TUI acts on the account a person selected, by position (`switch 3`), because the CLI's `ACCOUNT` is a position, an alias or an email. A `move` made elsewhere between the TUI's last read and the action would silently redirect it. Spec §4.5 closes that with a hidden global flag: `--expect-account <ID>` makes `App::resolve` refuse any `ACCOUNT` that resolves to an account with another ID. The CLI then acts on the resolved row's ID, as it already does, so a guarded command can never touch an account other than the one expected.

**What this task commits to:**
- **One chokepoint.** Every `ACCOUNT` goes through `App::resolve` (app.rs:1091): `remove` (1189), `disable` (1203), `enable` (1208), `alias` (1242, 1249), `move` (1257), `export` (1371), `switch` (1764), `run_agent` (1834), `history` (1922) and `map` (1967). `run_agent` already resolves there, so nothing needs routing through it. The check goes at the end of `resolve`, after the ambiguity prompt, so a person's choice from a prompt is checked too.
- **A refusal happens before anything changes**, because `resolve` is the first thing each command does with its `ACCOUNT`. It is a `Failure::Message`, so it exits 1 and `--json` prints `{"schemaVersion":1,"error":{"type":"stale-selection","message":…}}`.
- **A position that no longer holds any account** is still the engine's own `no-such-account` (`no account matches "1"`), not `stale-selection`: nothing resolves, so there is nothing to compare. Both change nothing, and the TUI treats both as "re-read".
- **`export` resolves several `ACCOUNT`s** through `resolve`, so the guard would apply to each. The TUI never passes the flag to `export`; the spec's "no effect on a command that resolves no `ACCOUNT`" is the guard's whole scope.
- **The flag is hidden from `--help` by clap, but clap_complete (4.6.11) still writes a hidden flag into the bash, zsh and fish scripts.** Checked: `hide = true` alone leaves `--expect-account` in 6, 3 and 4 places of the three scripts. Contract C2 says "absent from every completion script", so cycle 2 strips it from the generated text. (Tried and rejected: clearing the flag's `long` for the completion command, which makes zsh write it as a positional.)
- **`Context` is `Clone`** (C2), for Task 10's two contexts.

**Depends on:** none (the task table's `—`)

**Files:**
- Modify: `crates/tagteam/src/cli.rs` (`Cli` struct, lines 19–41; `CompletionShell::script`, 483–496; `options_taking_values`, 584–587)
- Modify: `crates/tagteam/src/app.rs` (error-kind constants, lines 82–87; `Context` derive, 137; `App` struct, 599; `run_command`'s `App { … }`, 710–721; `resolve`, 1090–1107)
- Modify: `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md` (§10.4, after line 1601)
- Test: `crates/tagteam/tests/app.rs` (append), `crates/tagteam/tests/run_cli.rs` (`Home`, lines 386–400; new tests before line 663), `crates/tagteam/tests/completions.rs` (`definitions`, line 42; `options`, line 343; new test)

**Interfaces:**
- Consumes: `Engine::resolve`, `AccountRow.id`, `AccountId::from_string`, `Failure::Message`.
- Produces (C2): `Cli.expect_account: Option<String>` (hidden, global); `App.expect_account: Option<AccountId>`; `pub const tagteam::app::KIND_STALE_SELECTION: &str = "stale-selection"`; the message `the selected account changed; nothing was done`; `#[derive(Clone)] pub struct Context`.

**Spec:** TUI spec §4.5 (the account reference is `--provider P --expect-account <ID> <position>`; the CLI refuses with `stale-selection`), §12.1 (the §10.4 amendment, hidden from `--help` and completions); core §10.4 (account references), core §14 (stable error kinds).

#### Cycle 1: the guard

- [ ] **Step 1: Write the failing tests**

Append to `crates/tagteam/tests/app.rs` (it already imports `Store`, `ProviderId`, `CLAUDE_CODE`, `Value` and `json`, and `two_logins()` is above):

```rust
// ---- `--expect-account`, the TUI's guard (core §10.4) ----

const STALE: &str = "the selected account changed; nothing was done";

/// Every account as `(position, id, alias, disabled)`, and the live login's email: what a
/// refused command must leave as it was.
type Snapshot = (Vec<(u32, String, Option<String>, bool)>, String);

fn snapshot(h: &H) -> Snapshot {
    let store = Store::open_existing(&h.env.data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    let rows = store
        .accounts(&ProviderId::new(CLAUDE_CODE))
        .unwrap()
        .into_iter()
        .map(|r| (r.position, r.id.as_str().to_owned(), r.alias, r.disabled))
        .collect();
    (rows, common::live_email(h._dir.path()))
}

/// The ID of the account at `position`.
fn id_at(snap: &Snapshot, position: u32) -> String {
    snap.0.iter().find(|r| r.0 == position).unwrap().1.clone()
}

/// A person read the list as `a@x.co` at 1 and `b@x.co` at 2 (live), and selected `b@x.co`.
/// Then a `move` elsewhere swapped the two, so position 2 holds `a@x.co`. Returns the harness,
/// that person's expectation (`b@x.co`'s ID) and `a@x.co`'s ID. With `a_disabled`, `a@x.co` was
/// disabled first, so that `enable 2` has something to undo.
fn stale_selection(a_disabled: bool) -> (H, String, String) {
    let h = two_logins();
    if a_disabled {
        h.ok(&["disable", "1"]);
    }
    let before = snapshot(&h);
    let (a, b) = (id_at(&before, 1), id_at(&before, 2));
    h.ok(&["move", "1", "2"]);
    (h, b, a)
}

/// What the TUI runs: `--no-color --provider claude-code --expect-account ID <command>`.
fn guarded<'a>(id: &'a str, command: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "--no-color",
        "--provider",
        CLAUDE_CODE,
        "--expect-account",
        id,
    ];
    args.extend_from_slice(command);
    args
}

/// Each command that resolves an `ACCOUNT` and changes something, aimed at position 2, and
/// whether `a@x.co` must be disabled for the change to show. `run` has its own tests in
/// `run_cli.rs`, where a fake `claude` stands in for the real one.
const ACCOUNT_COMMANDS: [(&[&str], bool); 6] = [
    (&["switch", "2"], false),
    (&["enable", "2"], true),
    (&["disable", "2"], false),
    (&["alias", "2", "--", "work"], false),
    (&["move", "2", "1"], false),
    (&["remove", "2"], false),
];

#[test]
fn an_account_that_moved_is_refused_as_stale_and_nothing_changes() {
    for (command, a_disabled) in ACCOUNT_COMMANDS {
        let (h, expected, _) = stale_selection(a_disabled);
        let before = snapshot(&h);
        let (code, out, err) = h.run(&guarded(&expected, command), &mut Scripted::none());
        assert_eq!(code, 1, "{command:?}: {err}");
        assert_eq!(out, "", "{command:?}");
        assert_eq!(err, format!("tagteam: {STALE}\n"), "{command:?}");
        assert_eq!(snapshot(&h), before, "{command:?} changed something");
    }
}

#[test]
fn the_refusal_is_a_stale_selection_error_under_json_too() {
    let (h, expected, _) = stale_selection(false);
    let (code, out, _) = h.run(
        &guarded(&expected, &["switch", "2", "--json"]),
        &mut Scripted::none(),
    );
    assert_eq!(code, 1);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "stale-selection", "message": STALE}})
    );
}

#[test]
fn the_guard_is_a_global_flag_so_it_may_follow_the_command() {
    let (h, expected, _) = stale_selection(false);
    let (code, _, err) = h.run(
        &["switch", "2", "--expect-account", &expected],
        &mut Scripted::none(),
    );
    assert_eq!(
        (code, err.as_str()),
        (
            1,
            "tagteam: the selected account changed; nothing was done\n"
        )
    );
}

#[test]
fn an_account_where_it_was_read_proceeds() {
    for (command, a_disabled) in ACCOUNT_COMMANDS {
        // `a@x.co` is at 2 now, and its ID is what this person expects there.
        let (h, _, a) = stale_selection(a_disabled);
        let (code, _, err) = h.run(&guarded(&a, command), &mut Scripted::none());
        assert_eq!(code, 0, "{command:?}: {err}");
        let (rows, live) = snapshot(&h);
        let row = rows.iter().find(|r| r.1 == a);
        match command[0] {
            "switch" => assert_eq!(live, "a@x.co"),
            "enable" => assert!(!row.unwrap().3, "enabled"),
            "disable" => assert!(row.unwrap().3, "disabled"),
            "alias" => assert_eq!(row.unwrap().2.as_deref(), Some("work")),
            "move" => assert_eq!(row.unwrap().0, 1),
            "remove" => assert!(row.is_none(), "removed"),
            other => unreachable!("{other}"),
        }
    }
}

#[test]
fn history_and_map_resolve_through_the_guard_too() {
    let (h, expected, a) = stale_selection(false);
    let dir = h._dir.path().to_str().unwrap().to_owned();
    for command in [vec!["history", "2"], vec!["map", "2", dir.as_str()]] {
        let (code, out, err) = h.run(&guarded(&expected, &command), &mut Scripted::none());
        assert_eq!((code, out.as_str()), (1, ""), "{command:?}");
        assert_eq!(err, format!("tagteam: {STALE}\n"), "{command:?}");
        let (code, _, err) = h.run(&guarded(&a, &command), &mut Scripted::none());
        assert_eq!(code, 0, "{command:?}: {err}");
    }
    assert_eq!(
        h.ok(&["map"]).lines().count(),
        1,
        "only the matching map was made"
    );
}

#[test]
fn a_command_without_an_account_ignores_the_guard() {
    // Nothing here resolves an ACCOUNT, so an ID that matches nothing is no reason to refuse.
    let (h, _, _) = stale_selection(false);
    for command in [
        &["list"][..],
        &["status"],
        &["alias"],
        &["history"],
        &["switch"],
    ] {
        let (code, _, err) = h.run(&guarded("someone-else", command), &mut Scripted::none());
        assert_eq!(code, 0, "{command:?}: {err}");
    }
}

#[test]
fn an_account_removed_elsewhere_is_not_found_rather_than_redirected() {
    let h = two_logins();
    let a = id_at(&snapshot(&h), 1);
    h.ok(&["remove", "1"]);
    let before = snapshot(&h);
    let (code, _, err) = h.run(&guarded(&a, &["disable", "1"]), &mut Scripted::none());
    assert_eq!(code, 1);
    assert_eq!(err, "tagteam: no account matches \"1\"\n");
    assert_eq!(snapshot(&h), before);
}
```

In `crates/tagteam/tests/run_cli.rs`, make `Home` keep account `b`'s ID too (the two fresh accounts are `a@x.co` at 1 and `b@x.co` at 2):

```rust
struct Home {
    dir: tempfile::TempDir,
    /// Account `a`'s id.
    a: String,
    /// Account `b`'s id.
    b: String,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = two_fresh_accounts(dir.path());
        fake_claude(dir.path());
        fs::create_dir_all(dir.path().join("work")).unwrap();
        Home { dir, a, b }
    }
}
```

and add these before the `// ---- `--json` and the command line` section (line 663):

```rust
// ---- `--expect-account`, the TUI's guard (core §10.4) ----

#[test]
fn run_refuses_an_account_that_is_not_the_one_expected_and_launches_nothing() {
    // A person who selected `b` at position 1 (a move since put `a` there) runs `run 1`.
    let home = Home::new();
    let out = home.out("stale");
    let output = home
        .tagteam(&out)
        .args(["--provider", "claude-code", "--expect-account", &home.b])
        .args(["--json", "run", "1", "--", "x"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        v["error"],
        json!({"type": "stale-selection",
               "message": "the selected account changed; nothing was done"})
    );
    assert!(
        fake_claude_calls(&out).is_empty(),
        "claude never ran, not even its login check"
    );
    assert!(!home.profile().exists(), "no profile, no reservation");
}

#[test]
fn run_launches_the_account_that_is_the_one_expected() {
    let home = Home::new();
    let out = home.out("matching");
    let output = home
        .tagteam(&out)
        .args(["--provider", "claude-code", "--expect-account", &home.a])
        .args(["run", "1", "--", "x"])
        .env("FAKE_CLAUDE_EXIT", "7")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7), "{}", stderr(&output));
    assert!(session(&out).is_some(), "the session ran");
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --test app -- stale an_account_ the_guard the_refusal a_command_without history_and_map`

Expected: 7 tests fail, each at `tests/app.rs:155` (`Cli::try_parse_from(argv).unwrap()` in `H::run_in`) with `ErrorInner { kind: UnknownArgument, … "--expect-account" … }`.

Run: `cargo test -p tagteam --features test-support --test run_cli -- run_refuses run_launches`

Expected: both fail on the exit code: `left: Some(2)`, clap's usage exit, for `unexpected argument '--expect-account'`.

- [ ] **Step 3: Implement**

`crates/tagteam/src/cli.rs`: add the field to `Cli`, between `provider` and `command`:

```rust
    /// The account the reference must resolve to (the TUI's guard, §10.4); hidden.
    #[arg(long = "expect-account", global = true, hide = true, value_name = "ID")]
    pub expect_account: Option<String>,
```

`crates/tagteam/src/app.rs`:

1. Next to the other kinds (after `KIND_IO`, line 82) add `pub const KIND_STALE_SELECTION: &str = "stale-selection";` with its doc comment, and next to the messages (after `KEYCHAIN_LOCKED`, line 86) add `const STALE_SELECTION: &str = "the selected account changed; nothing was done";`:

```rust
const KIND_IO: &str = "io";
/// `--expect-account` named an account other than the one the reference resolved to (§10.4).
pub const KIND_STALE_SELECTION: &str = "stale-selection";
```

```rust
const STALE_SELECTION: &str = "the selected account changed; nothing was done";
```

2. `Context` (line 137) gets the derive:

```rust
#[derive(Clone)]
pub struct Context {
```

3. `App` (line 599): after `provider_flag: Option<ProviderId>,` add

```rust
    /// `--expect-account`: the only account `resolve` may return (§10.4).
    expect_account: Option<AccountId>,
```

and in `run_command`'s `App { … }` (line 717) after `provider_flag,` add

```rust
        expect_account: cli.expect_account.map(AccountId::from_string),
```

(`cli.provider` and `cli.command` were moved out earlier in the function, so this is another partial move of `cli`; `Cli` has no `Drop`.)

4. Replace `resolve` (lines 1090–1107), the only function that changes behaviour:

```rust
    /// §10.4, with a terminal prompt for an ambiguous email. With `--expect-account`, the
    /// account it resolves to must be that one: another is refused as `stale-selection`, before
    /// the command has changed anything.
    fn resolve(&mut self, input: &str) -> Result<AccountRow, Failure> {
        let row = match self.engine.resolve(input, self.provider_flag.as_ref()) {
            Err(EngineError::Ambiguous { .. }) if self.can_prompt() => {
                let found = self.engine.candidates(input, self.provider_flag.as_ref())?;
                let labels: Vec<String> = found
                    .iter()
                    .map(|r| format!("{} #{} {}", r.provider, r.position, render::name(r)))
                    .collect();
                let choice = self.io.prompter.choose("Which account?", &labels);
                self.after_prompt()?;
                choice
                    .and_then(|i| found.get(i).cloned())
                    .ok_or_else(cancelled)?
            }
            other => other?,
        };
        match &self.expect_account {
            Some(expected) if *expected != row.id => Err(Failure::Message(
                KIND_STALE_SELECTION,
                STALE_SELECTION.into(),
            )),
            _ => Ok(row),
        }
    }
```

`docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`: §10.4 ends with "An empty alias never matches." (line 1601). Add after it:

````markdown
The hidden global flag `--expect-account ID` is the TUI's guard (sub-project 3, §4.5). It is
absent from `--help` and from the completion scripts. When it is given, an `ACCOUNT` that
resolves to an account with another ID is refused with the error kind `stale-selection` (exit 1,
"the selected account changed; nothing was done"), before anything is changed. It has no effect
on a command that resolves no `ACCOUNT`.
````

Core §13.2 and §14 name no list of error kinds (§13.2 says only that `type` is "a stable kind (§14)"), so this paragraph is the only place the new kind is listed.

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam --test app`

Expected: `test result: ok. 64 passed; 0 failed`.

Run: `cargo test -p tagteam --features test-support --test run_cli`

Expected: PASS (40 tests).

Run: `cargo test -p tagteam --features test-support`

Expected: PASS, run outside the sandbox (the `prompt` unit tests and `tests/signals.rs` open a pty).

- [ ] **Step 5: Format and lint**

```sh
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```sh
git add crates/tagteam/src/cli.rs crates/tagteam/src/app.rs crates/tagteam/tests/app.rs crates/tagteam/tests/run_cli.rs docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md
git commit -m "Refuse an account reference that resolves to an account other than --expect-account"
```

#### Cycle 2: out of the completion scripts

- [ ] **Step 7: Write the failing test**

Append to `crates/tagteam/tests/completions.rs`:

```rust
#[test]
fn the_hidden_guard_flag_is_in_no_help_page_and_no_completion_script() {
    // The TUI's `--expect-account` (core §10.4) is not for people. clap keeps it out of `--help`
    // with `hide`, but clap_complete writes a hidden flag into its scripts all the same, so
    // `CompletionShell::script` strips it.
    let d = tempfile::tempdir().unwrap();
    for args in [&["--help"][..], &["switch", "--help"], &["alias", "--help"]] {
        let out = cmd(d.path())
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let help = String::from_utf8(out).unwrap();
        assert!(!help.contains("expect-account"), "{args:?}: {help}");
    }
    for shell in SHELLS {
        let text = script(d.path(), shell);
        assert!(!text.contains("expect-account"), "{shell}");
        assert!(text.contains("--provider") || fish_declares_long(&text, "provider"));
    }
    // It is still a flag of the parser, global and hidden.
    let command = Cli::command();
    let flag = command
        .get_arguments()
        .find(|a| a.get_long() == Some("expect-account"))
        .expect("the flag exists");
    assert!(flag.is_hide_set() && flag.is_global_set());
}
```

- [ ] **Step 8: Run it and see it fail**

Run: `cargo test -p tagteam --features test-support --test completions the_hidden_guard`

Expected: FAIL at `assert!(!text.contains("expect-account"), "{shell}")` with message `bash`. The `--help` loop before it passes: clap's `hide` does keep the flag out of help.

- [ ] **Step 9: Implement**

`crates/tagteam/src/cli.rs`. In `CompletionShell::script` (line 483), filter the generated text before the fish positionals are appended, and add `without_hidden_flags` after the `impl`:

```rust
    pub fn script(self) -> Vec<u8> {
        let shell = match self {
            CompletionShell::Bash => Shell::Bash,
            CompletionShell::Zsh => Shell::Zsh,
            CompletionShell::Fish => Shell::Fish,
        };
        let mut cmd = Cli::command();
        let mut script = Vec::new();
        generate(shell, &mut cmd, "tagteam", &mut script);
        let mut script = without_hidden_flags(script, &cmd);
        if let CompletionShell::Fish = self {
            fish_positionals(&cmd, &mut script);
        }
        script
    }
}

/// `script` without the global flags `cmd` hides (`--expect-account`). clap keeps a hidden flag
/// out of `--help`, but clap_complete's bash, zsh and fish writers still list it, so this
/// removes, line by line, what each writes for it:
/// - bash: the flag's word in each `opts=` list, and its `case "${prev}"` arm;
/// - zsh: its `'--flag=…' \` line in each `_arguments` call;
/// - fish: its `complete … -l flag …` lines, and its word in the global option spec.
fn without_hidden_flags(script: Vec<u8>, cmd: &clap::Command) -> Vec<u8> {
    let hidden: Vec<&str> = cmd
        .get_arguments()
        .filter(|a| a.is_hide_set() && a.is_global_set())
        .filter_map(|a| a.get_long())
        .collect();
    let text = String::from_utf8(script).expect("clap_complete writes UTF-8");
    let mut out = String::with_capacity(text.len());
    let mut in_arm = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if in_arm {
            in_arm = trimmed != ";;";
            continue;
        }
        let mut line = line.to_owned();
        let mut drop = false;
        for flag in &hidden {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            if trimmed == format!("--{flag})") {
                in_arm = true;
                drop = true;
            }
            drop |= trimmed.starts_with(&format!("'--{flag}"));
            drop |= trimmed.starts_with("complete ") && words.windows(2).any(|w| w == ["-l", flag]);
            line = line
                .replace(&format!(" --{flag}"), "")
                .replace(&format!(" {flag}="), "");
        }
        if !drop {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out.into_bytes()
}
```

In `options_taking_values` (line 584), which feeds fish's `__fish_tagteam_words` helper, skip hidden arguments:

```rust
    for arg in cmd
        .get_arguments()
        .filter(|a| !a.is_positional() && !a.is_hide_set() && a.get_action().takes_values())
    {
```

Two existing tests in `crates/tagteam/tests/completions.rs` compare the scripts with `Cli::command()`'s own arguments, and now find the hidden flag missing from the scripts. Both helpers skip hidden arguments:

```rust
/// Every subcommand name and visible alias under `cmd`, and every long flag a person can use:
/// a hidden one (`--expect-account`) is in no script.
fn definitions(cmd: &clap::Command, names: &mut BTreeSet<String>, flags: &mut BTreeSet<String>) {
    flags.extend(
        cmd.get_arguments()
            .filter(|a| !a.is_hide_set())
            .filter_map(|a| a.get_long())
            .map(str::to_owned),
    );
    // … the rest of `definitions` is unchanged
}
```

```rust
/// Every spelling of an option under `cmd` that is not hidden, by whether it takes a value.
fn options(cmd: &clap::Command, valued: &mut BTreeSet<String>, flags: &mut BTreeSet<String>) {
    for arg in cmd
        .get_arguments()
        .filter(|a| !a.is_positional() && !a.is_hide_set())
    {
        // … the loop body and the rest of `options` are unchanged
```

- [ ] **Step 10: Run it and see it pass**, then the crate's completion tests

Run: `cargo test -p tagteam --features test-support --test completions`

Expected: `test result: ok. 8 passed; 0 failed`.

Then, as a spot check, the generated scripts still parse: `target/debug/tagteam completions bash | bash -n` and `target/debug/tagteam completions zsh | zsh -n` print nothing and exit 0.

- [ ] **Step 11: Format and lint**

```sh
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 12: Commit**

```sh
git add crates/tagteam/src/cli.rs crates/tagteam/tests/completions.rs
git commit -m "Keep the hidden --expect-account flag out of the completion scripts"
```

**Existing tests this task changes:** `completions::each_shell_s_script_offers_every_command_flag_key_and_provider` (via `definitions`) and `completions::fish_completes_a_positional_only_in_its_place` (via `options`) would fail once the scripts omit the flag; their helpers now skip hidden arguments (Step 9). Nothing else changes.

---

### Task 3: The `tagteam-tui` crate, its ports, and the MSRV

The crate every later task fills. It is a library, so the binary can depend on it and `tagteam-tui` can never depend on `tagteam` (TUI spec §4.1). This task adds the crate with the two ports that need no screen to be tested, `Host` and `Source` (contract C3.1 and C3.2), and an empty file for each other module, so that Tasks 4–11 each fill their own file and nothing else. It also raises the workspace MSRV (R1) and adds the crate to the release loop.

**What this task commits to:**
- **`Action::args` is the one place the TUI's command lines are built.** Each row of C3.1's table is a unit test in `host.rs`. `tests/tui_args.rs` then parses every action's arguments with the CLI's real `Cli::try_parse_from` (the TUI crate cannot depend on `tagteam`, so the round trip lives in the binary's tests). It includes aliases that begin with `-`, equal `--unset` or `--`, are empty, or hold spaces and emoji. The CLI's parser accepts `alias N -- NAME` for all of them, so C3.1's `--` stands.
- **Only `Host` and `Source`'s types are real.** `EngineSource`'s three methods are `unimplemented!()`: the one stub the plan allows, replaced in Task 8, whose tests pin every method. Its `engine` field is never read until then, so it carries `#[allow(dead_code)]` (clippy `-D warnings` otherwise fails: `field `engine` is never read`); Task 8 removes the attribute.
- **`lib.rs` omits C3's two `pub use` lines.** `pub use runtime::{run, TuiOptions, TuiExit}` and `pub use state::Screen` name items that do not exist while `runtime.rs` and `state.rs` are empty (contract C3 says so). Task 5 adds `pub use state::Screen;` and Task 11 the `runtime` re-export.
- **`#![forbid(unsafe_code)]`** is on the crate, as on `tagteam-core` and `tagteam-engine`: R6 keeps every `libc` call in `tagteam-provider`.
- **`ActionOutcome` derives `Debug, Clone, PartialEq, Eq`**, which C3.1 does not list. It costs nothing, and Tasks 5, 9 and 10 compare outcomes in their tests.
- **MSRV 1.88.** `rust-toolchain.toml` already pins 1.88.0 and no CI job builds 1.85 (R1). ratatui 0.30.2 declares `rust-version = "1.88.0"`, so `[workspace.package] rust-version` becomes `"1.88"`. CONTRIBUTING.md line 13 and README.md line 75 state it, so both change. (Core §16's "MSRV ≥ 1.85" still holds, and stays as written.)
- **New dependencies need `cargo fetch` outside the sandbox, once.** Cargo cannot write `~/.cargo/registry` inside the sandbox. Run it in Step 1, once the crate's manifest and `lib.rs` exist; everything after builds sandboxed. It rewrites `Cargo.lock`, which is committed: about a thousand added lines (ratatui's graph, including its optional `termwiz`/`termina` backends the lockfile must name), one new `tagteam-tui` package, a `tagteam-tui` entry in `tagteam`'s dependencies, and a few existing lists that now name a version (`"nom 8.0.0"`, `"bitflags 2.13.2"`) because a second version of each joined the graph.

**Depends on:** 2. `tests/tui_args.rs` parses `--expect-account`, which Task 2 adds to `Cli`.

**Files:**
- Create: `crates/tagteam-tui/Cargo.toml`, `crates/tagteam-tui/src/{lib,host,source,lanes,msg,state,update,view,actions,ask,term,input,runtime}.rs`, `crates/tagteam-tui/src/screens/mod.rs`
- Create: symlinks `crates/tagteam-tui/LICENSE` and `crates/tagteam-tui/NOTICE` (the other crates have them: `LICENSE -> ../../LICENSE`)
- Create: `crates/tagteam/tests/tui_args.rs`
- Modify: `Cargo.toml` (`rust-version`, line 8; `[workspace.dependencies]`: `tagteam-tui` after line 20, `crossterm` after line 29, `ratatui` after line 32)
- Modify: `Cargo.lock` (by `cargo fetch`)
- Modify: `crates/tagteam/Cargo.toml` (dependency, after line 23)
- Modify: `.github/workflows/publish-crates.yml` (crate loop, line 57)
- Modify: `CONTRIBUTING.md` (line 13; the crate table; "Releasing", lines 140–142), `README.md` (line 75)
- Test: `crates/tagteam-tui/src/host.rs`, `crates/tagteam-tui/src/source.rs` (unit tests), `crates/tagteam/tests/tui_args.rs`

**Interfaces:**
- Consumes: `tagteam_core::{AccountId, ProviderId}`; `tagteam_engine::Engine`, `tagteam_engine::error::EngineError` (`kind()`, `Display`), `tagteam_engine::views::{HistoryView, ProviderAccounts}`; the CLI's `tagteam::cli::{Cli, Command, StrategyArg}` and, from Task 2, `Cli.expect_account`.
- Produces (C3, C3.1, C3.2): `tagteam_tui::host::{AccountRef, Strategy, Action, ActionOutcome, Ask, Host}` with `Action::{args, name, needs_terminal}`; `tagteam_tui::source::{SourceError, AccountsSnapshot, CollectSummary, Source, SourceFactory, EngineSource}`; the modules `lanes`, `msg`, `state`, `update`, `view`, `screens`, `actions` (public) and `ask`, `term`, `input`, `runtime` (private), each empty.

**Spec:** R1 (MSRV); TUI spec §4.1 (the crate and the dependency rule), §4.4 (`Source`), §4.5 (`Host`, `Ask`, the account reference), §12.2 (published in the crates.io loop, in dependency order); core §16 (MSRV, internal crates have no stable API).

- [ ] **Step 1: Wire the crate, and write the failing tests**

First the workspace. In `Cargo.toml`:

```toml
[workspace.package]
rust-version = "1.88"
```

and in `[workspace.dependencies]` add `tagteam-tui` after `tagteam-engine`, `crossterm` after `clap_complete`, and `ratatui` after `libc`:

```toml
tagteam-tui = { path = "crates/tagteam-tui", version = "20261014.2045.0" }
```

```toml
# The terminal UI (TUI spec §4.1): ratatui with its default crossterm backend, and the crossterm
# it is built against. Both are pinned exactly, so a patch release never changes what is drawn;
# ratatui 0.30.2 declares rust-version 1.88.0, which is why the MSRV is 1.88.
crossterm = "=0.29.0"
```

```toml
ratatui = "=0.30.2"
```

`crates/tagteam/Cargo.toml`: after `tagteam-engine.workspace = true` in `[dependencies]` add

```toml
tagteam-tui.workspace = true
```

`.github/workflows/publish-crates.yml` line 57 gains `tagteam-tui` after `tagteam-engine` and before `tagteam` (the crates publish in dependency order, TUI spec §12.2):

```sh
          for crate in tagteam-core tagteam-provider tagteam-cc tagteam-engine tagteam-tui tagteam; do
```

(The daemon's branch inserts `tagteam-daemon` between `tagteam-engine` and `tagteam-tui` when this branch is rebased onto it, spec §12.3.)

Outside this plan, before the first release that carries the crate: its first crates.io publish needs a short-lived token by hand and a trusted-publisher registration for `release.yml` (CONTRIBUTING, "Releasing"; TUI spec §12.2), and Michael is told before that release. The repository has no `deny.toml` or audit job, so spec §12.2's licence and advisory checks have nothing to run yet; the musl smoke job is what checks that the binary stays static.

`CONTRIBUTING.md`: line 13 becomes

```markdown
  workspace declares a minimum supported Rust version of 1.88 (edition 2024), which ratatui 0.30
  requires.
```

add a row to the crate table after `tagteam-engine`:

```markdown
| `tagteam-tui` | The terminal UI: its state machine, screens and ports (`tagteam tui`) |
```

and in "Releasing" (lines 140–142) the version bump now names five internal entries:

```markdown
  zeros in `HHMM` (09:05 is `905`). Change `version` in `[workspace.package]` and the five
  internal `version = "…"` entries in `[workspace.dependencies]` (`tagteam-core`,
  `tagteam-provider`, `tagteam-cc`, `tagteam-engine` and `tagteam-tui`) together, then refresh
  `Cargo.lock`.
```

`README.md` line 75: `You need Rust 1.85 or newer` becomes `You need Rust 1.88 or newer`.

The crate. `crates/tagteam-tui/Cargo.toml` (it has no `include`: like the other crates, it packs the LICENSE and NOTICE that the two symlinks point at):

```toml
[package]
name = "tagteam-tui"
description = "Internal crate of tagteam: the terminal UI. No stable API."
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
tagteam-core.workspace = true
tagteam-provider.workspace = true
tagteam-engine.workspace = true
crossterm.workspace = true
ratatui.workspace = true
unicode-width.workspace = true

[lints]
workspace = true
```

```sh
ln -s ../../LICENSE crates/tagteam-tui/LICENSE
ln -s ../../NOTICE crates/tagteam-tui/NOTICE
```

`crates/tagteam-tui/src/lib.rs`:

```rust
//! tagteam's terminal UI (TUI spec): an Elm-style state machine drawn with ratatui, worker
//! lanes over a read port, and the action port the `tagteam` binary implements. An internal
//! library with no stable API.
#![forbid(unsafe_code)]

pub mod actions;
mod ask;
pub mod host;
mod input;
pub mod lanes;
pub mod msg;
mod runtime;
pub mod screens;
pub mod source;
pub mod state;
mod term;
pub mod update;
pub mod view;
```

and one line each in the modules later tasks fill (`screens/mod.rs` too):

```rust
// lanes.rs
//! Worker lanes: a thread with a coalescing request queue (TUI spec §4.3).
// msg.rs
//! The messages `update` takes and the commands it returns (TUI spec §4.2).
// state.rs
//! The state `update` changes and `view` draws (TUI spec §4.2).
// update.rs
//! `update`: the pure state transition (TUI spec §4.2).
// view.rs
//! `view`: the pure renderer (TUI spec §4.2, §5.1).
// screens/mod.rs
//! The screens (TUI spec §5).
// actions.rs
//! The action keys, dialogs and result panels (TUI spec §7).
// ask.rs
//! The `Ask` handed to the host: dialogs on the UI thread (TUI spec §7.2).
// term.rs
//! Entering and restoring the terminal (TUI spec §10.1).
// input.rs
//! The input thread (TUI spec §4.3).
// runtime.rs
//! The runtime: the UI loop, suspension and shutdown (TUI spec §7.3, §10.2).
```

(The `// name.rs` lines only label the files: each file is its one `//!` line.)

Now fetch the new dependencies, outside the sandbox and once (the manifest must have its `lib.rs` before cargo will read it, so this comes after the files above):

```sh
cargo fetch
```

Then the tests. `crates/tagteam-tui/src/host.rs` holds, for now, only its test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn account(position: u32) -> AccountRef {
        AccountRef {
            id: AccountId::from_string("0192-abc"),
            provider: ProviderId::new("claude-code"),
            position,
        }
    }

    fn provider() -> ProviderId {
        ProviderId::new("claude-code")
    }

    fn args(a: &Action) -> String {
        a.args().join(" ")
    }

    #[test]
    fn each_action_names_its_account_by_provider_id_and_position() {
        let g = "--no-color --provider claude-code --expect-account 0192-abc";
        for (action, tail) in [
            (Action::Switch(account(3)), "switch 3"),
            (Action::Enable(account(3)), "enable 3"),
            (Action::Disable(account(3)), "disable 3"),
            (Action::Remove(account(3)), "remove 3"),
            (Action::ClearAlias(account(3)), "alias 3 --unset"),
            (
                Action::Move {
                    account: account(3),
                    position: 1,
                },
                "move 3 1",
            ),
            (
                Action::SetAlias {
                    account: account(3),
                    alias: "work".into(),
                },
                "alias 3 -- work",
            ),
        ] {
            assert_eq!(args(&action), format!("{g} {tail}"), "{action:?}");
        }
    }

    #[test]
    fn the_actions_that_name_no_account_carry_no_guard() {
        assert_eq!(
            args(&Action::SwitchBy {
                provider: provider(),
                strategy: Strategy::Best
            }),
            "--no-color --provider claude-code switch --strategy best"
        );
        assert_eq!(
            args(&Action::SwitchBy {
                provider: provider(),
                strategy: Strategy::NextAvailable
            }),
            "--no-color --provider claude-code switch --strategy next-available"
        );
        assert_eq!(
            args(&Action::AddLive {
                provider: provider()
            }),
            "--no-color --provider claude-code add"
        );
    }

    #[test]
    fn run_keeps_its_colour_because_the_agent_output_is_not_ours() {
        assert_eq!(
            args(&Action::Run(account(2))),
            "--provider claude-code --expect-account 0192-abc run 2"
        );
    }

    #[test]
    fn doctor_names_a_provider_only_when_it_has_one() {
        let doctor = |provider, online| args(&Action::Doctor { provider, online });
        assert_eq!(doctor(None, false), "--no-color doctor");
        assert_eq!(doctor(None, true), "--no-color doctor --online");
        assert_eq!(
            doctor(Some(provider()), false),
            "--no-color --provider claude-code doctor"
        );
        assert_eq!(
            doctor(Some(provider()), true),
            "--no-color --provider claude-code doctor --online"
        );
    }

    #[test]
    fn an_alias_is_one_argument_after_the_double_dash_whatever_it_holds() {
        for alias in ["-work", "--unset", "--", "a b", "", "日本語 😀"] {
            let args = Action::SetAlias {
                account: account(1),
                alias: alias.into(),
            }
            .args();
            let (head, tail) = args.split_at(args.len() - 1);
            assert_eq!(&head[head.len() - 3..], ["alias", "1", "--"], "{alias:?}");
            assert_eq!(tail, [alias], "one argument, untouched");
        }
    }

    #[test]
    fn names_are_the_clis_command_names() {
        let a = || account(1);
        for (action, name) in [
            (Action::Switch(a()), "switch"),
            (
                Action::SwitchBy {
                    provider: provider(),
                    strategy: Strategy::Best,
                },
                "switch",
            ),
            (Action::Enable(a()), "enable"),
            (Action::Disable(a()), "disable"),
            (
                Action::SetAlias {
                    account: a(),
                    alias: "x".into(),
                },
                "alias",
            ),
            (Action::ClearAlias(a()), "alias"),
            (
                Action::Move {
                    account: a(),
                    position: 2,
                },
                "move",
            ),
            (Action::Remove(a()), "remove"),
            (
                Action::AddLive {
                    provider: provider(),
                },
                "add",
            ),
            (Action::Run(a()), "run"),
            (
                Action::Doctor {
                    provider: None,
                    online: false,
                },
                "doctor",
            ),
        ] {
            assert_eq!(action.name(), name, "{action:?}");
        }
    }

    #[test]
    fn only_run_needs_the_terminal() {
        let a = || account(1);
        assert!(Action::Run(a()).needs_terminal());
        for action in [
            Action::Switch(a()),
            Action::Enable(a()),
            Action::Disable(a()),
            Action::Remove(a()),
            Action::ClearAlias(a()),
            Action::AddLive {
                provider: provider(),
            },
            Action::Doctor {
                provider: None,
                online: true,
            },
        ] {
            assert!(!action.needs_terminal(), "{action:?}");
        }
    }
}
```

`crates/tagteam-tui/src/source.rs` holds only:

```rust
#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    use super::*;

    #[test]
    fn an_engine_error_keeps_its_kind_and_its_words() {
        let e: SourceError = EngineError::NoSuchAccount("9".into()).into();
        assert_eq!(
            e,
            SourceError {
                kind: "no-such-account".into(),
                message: "no account matches \"9\"".into(),
            }
        );
        let e: SourceError = EngineError::InsideRunShell.into();
        assert_eq!(e.kind, EngineError::InsideRunShell.kind());
        assert_eq!(e.message, EngineError::InsideRunShell.to_string());
    }

    /// A source that answers every read with an error naming the read, so a test can tell
    /// which one it called.
    struct Named(&'static str);

    impl Source for Named {
        fn accounts(&self, _: Option<&ProviderId>) -> Result<AccountsSnapshot, SourceError> {
            Err(SourceError {
                kind: "accounts".into(),
                message: self.0.into(),
            })
        }
        fn history(&self, _: &AccountId, _: i64) -> Result<HistoryView, SourceError> {
            Err(SourceError {
                kind: "history".into(),
                message: self.0.into(),
            })
        }
        fn collect_due(&self, _: &[AccountId]) -> Result<CollectSummary, SourceError> {
            Ok(CollectSummary {
                fetched: 0,
                warnings: vec![self.0.into()],
            })
        }
    }

    #[test]
    fn a_factory_builds_a_source_in_the_thread_that_calls_it() {
        // The factory is shared between threads; the source it builds never leaves its own.
        let built = Arc::new(AtomicUsize::new(0));
        let counter = built.clone();
        let factory: SourceFactory = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Box::new(Named("built"))
        });
        let from_a_thread = factory.clone();
        let message = thread::spawn(move || {
            let source = from_a_thread();
            source.accounts(None).unwrap_err().message
        })
        .join()
        .unwrap();
        assert_eq!(message, "built");
        let source = factory();
        assert_eq!(
            source.collect_due(&[]).unwrap().warnings,
            ["built".to_owned()]
        );
        assert_eq!(built.load(Ordering::SeqCst), 2, "one source per call");
    }
}
```

and `crates/tagteam/tests/tui_args.rs` is the round trip through the real parser:

```rust
//! The arguments the TUI builds for each action (`Action::args`), parsed by the CLI's own
//! parser. `tagteam-tui` cannot depend on `tagteam`, so the round trip lives here.

use clap::{CommandFactory, Parser};
use tagteam::cli::{Cli, Command, StrategyArg};
use tagteam_core::{AccountId, ProviderId};
use tagteam_tui::host::{AccountRef, Action, Strategy};

const ID: &str = "0192-abc";

fn account(position: u32) -> AccountRef {
    AccountRef {
        id: AccountId::from_string(ID),
        provider: ProviderId::new("claude-code"),
        position,
    }
}

fn provider() -> ProviderId {
    ProviderId::new("claude-code")
}

/// `tagteam <action.args()>`, parsed.
fn parse(action: &Action) -> Cli {
    let argv = std::iter::once("tagteam".to_owned()).chain(action.args());
    match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(e) => panic!("{:?} does not parse: {e}", action.args()),
    }
}

/// What the flags before the command say, for an action that names an account.
fn guarded(cli: &Cli, no_color: bool) {
    assert_eq!(cli.provider.as_deref(), Some("claude-code"));
    assert_eq!(cli.expect_account.as_deref(), Some(ID));
    assert_eq!(cli.no_color, no_color);
    assert!(!cli.json && !cli.debug);
}

#[test]
fn switch_enable_disable_and_remove_parse_to_their_commands_at_the_position() {
    let cli = parse(&Action::Switch(account(3)));
    guarded(&cli, true);
    assert!(matches!(
        cli.command,
        Some(Command::Switch { account: Some(ref a), force: false, strategy: None, model: None }) if a == "3"
    ));
    let cli = parse(&Action::Enable(account(3)));
    guarded(&cli, true);
    assert!(matches!(cli.command, Some(Command::Enable { ref account }) if account == "3"));
    let cli = parse(&Action::Disable(account(3)));
    guarded(&cli, true);
    assert!(matches!(cli.command, Some(Command::Disable { ref account }) if account == "3"));
    let cli = parse(&Action::Remove(account(3)));
    guarded(&cli, true);
    assert!(matches!(cli.command, Some(Command::Remove { ref account }) if account == "3"));
}

#[test]
fn a_strategy_switch_names_no_account_and_no_guard() {
    for (strategy, expected) in [
        (Strategy::Best, "best"),
        (Strategy::NextAvailable, "next-available"),
    ] {
        let cli = parse(&Action::SwitchBy {
            provider: provider(),
            strategy,
        });
        assert_eq!(cli.provider.as_deref(), Some("claude-code"));
        assert_eq!(cli.expect_account, None);
        assert!(cli.no_color);
        let Some(Command::Switch {
            account: None,
            strategy: Some(parsed),
            ..
        }) = cli.command
        else {
            panic!("not a strategy switch");
        };
        assert!(
            matches!(
                (parsed, strategy),
                (StrategyArg::Best, Strategy::Best)
                    | (StrategyArg::NextAvailable, Strategy::NextAvailable)
            ),
            "{expected}"
        );
    }
}

#[test]
fn an_alias_is_the_name_whatever_it_begins_with() {
    // `--` ends the options, so an alias that looks like a flag, or like `--`, is still a name.
    for alias in [
        "work",
        "-work",
        "--unset",
        "--provider",
        "--",
        "a b",
        "",
        "日本語 😀",
    ] {
        let cli = parse(&Action::SetAlias {
            account: account(2),
            alias: alias.into(),
        });
        guarded(&cli, true);
        let Some(Command::Alias {
            account: Some(position),
            name: Some(name),
            unset: false,
        }) = cli.command
        else {
            panic!("{alias:?} is not an alias to set");
        };
        assert_eq!((position.as_str(), name.as_str()), ("2", alias));
    }
}

#[test]
fn clearing_an_alias_is_unset() {
    let cli = parse(&Action::ClearAlias(account(2)));
    guarded(&cli, true);
    assert!(matches!(
        cli.command,
        Some(Command::Alias { account: Some(ref a), name: None, unset: true }) if a == "2"
    ));
}

#[test]
fn a_move_names_the_position_it_is_at_and_the_one_it_goes_to() {
    let cli = parse(&Action::Move {
        account: account(2),
        position: 5,
    });
    guarded(&cli, true);
    assert!(matches!(
        cli.command,
        Some(Command::Move { ref account, position: 5 }) if account == "2"
    ));
}

#[test]
fn adding_the_live_login_names_only_the_provider() {
    let cli = parse(&Action::AddLive {
        provider: provider(),
    });
    assert_eq!(cli.provider.as_deref(), Some("claude-code"));
    assert_eq!(cli.expect_account, None);
    assert!(cli.no_color);
    assert!(matches!(
        cli.command,
        Some(Command::Add {
            position: None,
            alias: None,
            yes: false
        })
    ));
}

#[test]
fn run_is_launched_with_its_colour_and_the_guard() {
    let cli = parse(&Action::Run(account(1)));
    guarded(&cli, false);
    let Some(Command::Run {
        account: Some(position),
        require_session: false,
        args,
    }) = cli.command
    else {
        panic!("not a plain run");
    };
    assert_eq!(position, "1");
    assert!(args.is_empty(), "nothing is passed to the agent");
}

#[test]
fn doctor_parses_with_and_without_a_provider_and_online() {
    for (provider, online) in [
        (None, false),
        (None, true),
        (Some(provider()), false),
        (Some(provider()), true),
    ] {
        let named = provider.as_ref().map(|p| p.to_string());
        let cli = parse(&Action::Doctor { provider, online });
        assert_eq!(cli.provider, named);
        assert_eq!(cli.expect_account, None);
        assert!(cli.no_color);
        assert!(matches!(
            cli.command,
            Some(Command::Doctor { online: o }) if o == online
        ));
    }
}

#[test]
fn every_action_name_is_a_subcommand_of_the_cli() {
    let command = Cli::command();
    for action in [
        Action::Switch(account(1)),
        Action::SwitchBy {
            provider: provider(),
            strategy: Strategy::Best,
        },
        Action::Enable(account(1)),
        Action::Disable(account(1)),
        Action::SetAlias {
            account: account(1),
            alias: "x".into(),
        },
        Action::ClearAlias(account(1)),
        Action::Move {
            account: account(1),
            position: 2,
        },
        Action::Remove(account(1)),
        Action::AddLive {
            provider: provider(),
        },
        Action::Run(account(1)),
        Action::Doctor {
            provider: None,
            online: false,
        },
    ] {
        assert!(
            command.find_subcommand(action.name()).is_some(),
            "{} is no subcommand",
            action.name()
        );
        // The name is the command the arguments run.
        assert!(
            action.args().iter().any(|a| a == action.name()),
            "{:?} does not run {}",
            action.args(),
            action.name()
        );
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-tui --lib`

Expected: the test target does not compile: `error[E0412]: cannot find type `AccountRef` in this scope`, `cannot find type `ProviderId``-style errors in `host.rs`, and `cannot find type `SourceError`/`Source`/`SourceFactory`/`EngineError`` in `source.rs`.

Run: `cargo test -p tagteam --test tui_args`

Expected: `error[E0432]: unresolved imports `tagteam_tui::host::AccountRef`, `tagteam_tui::host::Action`, `tagteam_tui::host::Strategy``.

- [ ] **Step 3: Implement**

`crates/tagteam-tui/src/host.rs`: put this above the test module.

```rust
//! The action port (TUI spec §4.5). The `tagteam` binary implements `Host` over the CLI's own
//! dispatch, so every action says and does what the same command does on the command line.

use tagteam_core::{AccountId, ProviderId};

/// The selected account as the TUI last read it: the position names it to the CLI, and the ID
/// guards the position (`--expect-account`, core §10.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRef {
    pub id: AccountId,
    pub provider: ProviderId,
    pub position: u32,
}

/// `switch --strategy`'s values (core §9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Best,
    NextAvailable,
}

impl Strategy {
    fn arg(self) -> &'static str {
        match self {
            Strategy::Best => "best",
            Strategy::NextAvailable => "next-available",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Switch(AccountRef),
    SwitchBy {
        provider: ProviderId,
        strategy: Strategy,
    },
    Enable(AccountRef),
    Disable(AccountRef),
    SetAlias {
        account: AccountRef,
        alias: String,
    },
    ClearAlias(AccountRef),
    Move {
        account: AccountRef,
        position: u32,
    },
    Remove(AccountRef),
    AddLive {
        provider: ProviderId,
    },
    Run(AccountRef),
    Doctor {
        provider: Option<ProviderId>,
        online: bool,
    },
}

impl Action {
    /// The arguments after `tagteam`, for the CLI's own parser. An account is named by
    /// `--provider P --expect-account ID` and its position, so a `move` made elsewhere since the
    /// TUI last read cannot redirect the action to another account (spec §4.5). Every action but
    /// `Run`, whose output is the agent's own, asks for plain text.
    pub fn args(&self) -> Vec<String> {
        let account = |a: &AccountRef, command: &[&str]| -> Vec<String> {
            let mut args = vec![
                "--no-color".to_owned(),
                "--provider".to_owned(),
                a.provider.to_string(),
                "--expect-account".to_owned(),
                a.id.to_string(),
            ];
            args.extend(command.iter().map(|s| (*s).to_owned()));
            args
        };
        match self {
            Action::Switch(a) => {
                let n = a.position.to_string();
                account(a, &["switch", &n])
            }
            Action::SwitchBy { provider, strategy } => vec![
                "--no-color".into(),
                "--provider".into(),
                provider.to_string(),
                "switch".into(),
                "--strategy".into(),
                strategy.arg().into(),
            ],
            Action::Enable(a) => account(a, &["enable", &a.position.to_string()]),
            Action::Disable(a) => account(a, &["disable", &a.position.to_string()]),
            Action::SetAlias { account: a, alias } => {
                account(a, &["alias", &a.position.to_string(), "--", alias])
            }
            Action::ClearAlias(a) => account(a, &["alias", &a.position.to_string(), "--unset"]),
            Action::Move {
                account: a,
                position,
            } => account(a, &["move", &a.position.to_string(), &position.to_string()]),
            Action::Remove(a) => account(a, &["remove", &a.position.to_string()]),
            Action::AddLive { provider } => vec![
                "--no-color".into(),
                "--provider".into(),
                provider.to_string(),
                "add".into(),
            ],
            Action::Run(a) => vec![
                "--provider".into(),
                a.provider.to_string(),
                "--expect-account".into(),
                a.id.to_string(),
                "run".into(),
                a.position.to_string(),
            ],
            Action::Doctor { provider, online } => {
                let mut args = vec!["--no-color".to_owned()];
                if let Some(p) = provider {
                    args.extend(["--provider".to_owned(), p.to_string()]);
                }
                args.push("doctor".into());
                if *online {
                    args.push("--online".into());
                }
                args
            }
        }
    }

    /// The command's name as the CLI spells it: "switch", "enable", "disable", "alias", "move",
    /// "remove", "add", "run", "doctor". The footer shows it while the action runs, and says
    /// `busy: <name>` to a second action key (spec §7.2).
    pub fn name(&self) -> &'static str {
        match self {
            Action::Switch(_) | Action::SwitchBy { .. } => "switch",
            Action::Enable(_) => "enable",
            Action::Disable(_) => "disable",
            Action::SetAlias { .. } | Action::ClearAlias(_) => "alias",
            Action::Move { .. } => "move",
            Action::Remove(_) => "remove",
            Action::AddLive { .. } => "add",
            Action::Run(_) => "run",
            Action::Doctor { .. } => "doctor",
        }
    }

    /// `Run` only: it always runs suspended (spec §7.3).
    pub fn needs_terminal(&self) -> bool {
        matches!(self, Action::Run(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutcome {
    Done {
        exit: i32,
        out: String,
        err: String,
    },
    /// The command needs the terminal itself, and changed nothing (spec §4.5).
    NeedsTerminal,
}

/// The questions an action may ask: the CLI's own prompts, answered by the TUI's dialogs.
pub trait Ask {
    fn confirm(&mut self, question: &str, default_yes: bool) -> bool;
    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize>;
}

pub trait Host: Send + Sync {
    /// Runs `action` inside the TUI, on the actions lane. Prompts arrive through `ask`.
    fn act(&self, action: &Action, ask: &mut dyn Ask) -> ActionOutcome;
    /// Runs `action` on the real terminal, on the UI thread, after the TUI has suspended. Then
    /// prints `Press Enter to return to tagteam` and waits for Enter, or for a signal.
    fn act_foreground(&self, action: &Action) -> ActionOutcome;
}
```

`crates/tagteam-tui/src/source.rs`: put this above the test module.

```rust
//! The read port (TUI spec §4.4): what the screens show, read from the engine on a lane's own
//! thread, never on the UI thread.

use std::sync::Arc;

use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::Engine;
use tagteam_engine::error::EngineError;
use tagteam_engine::views::{HistoryView, ProviderAccounts};

/// A read that failed, in the CLI's own words (`kind` is the JSON `error.type`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError {
    pub kind: String,
    pub message: String,
}

impl From<EngineError> for SourceError {
    fn from(e: EngineError) -> Self {
        SourceError {
            kind: e.kind().to_owned(),
            message: e.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AccountsSnapshot {
    pub providers: Vec<ProviderAccounts>,
    /// The run shell's own account (`ShellAccount`), for `▶ this`.
    pub this: Option<AccountId>,
    /// The default provider's live login, when tagteam does not manage it
    /// (`StatusView::Unmanaged`).
    pub unmanaged_live: Option<String>,
    pub in_run_shell: bool,
    /// The engine clock, in seconds, when this was read.
    pub read_at_s: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectSummary {
    pub fetched: usize,
    pub warnings: Vec<String>,
}

pub trait Source {
    fn accounts(&self, provider: Option<&ProviderId>) -> Result<AccountsSnapshot, SourceError>;
    fn history(&self, account: &AccountId, since_s: i64) -> Result<HistoryView, SourceError>;
    fn collect_due(&self, accounts: &[AccountId]) -> Result<CollectSummary, SourceError>;
}

/// Builds one `Source` inside the thread that will use it, so neither `Engine` nor the
/// binary's `Context` needs to be `Send` or `Sync`.
pub type SourceFactory = Arc<dyn Fn() -> Box<dyn Source> + Send + Sync>;

pub struct EngineSource {
    #[allow(dead_code)]
    engine: Engine,
}

impl EngineSource {
    pub fn new(engine: Engine) -> Self {
        EngineSource { engine }
    }
}

impl Source for EngineSource {
    fn accounts(&self, _provider: Option<&ProviderId>) -> Result<AccountsSnapshot, SourceError> {
        unimplemented!("EngineSource::accounts")
    }

    fn history(&self, _account: &AccountId, _since_s: i64) -> Result<HistoryView, SourceError> {
        unimplemented!("EngineSource::history")
    }

    fn collect_due(&self, _accounts: &[AccountId]) -> Result<CollectSummary, SourceError> {
        unimplemented!("EngineSource::collect_due")
    }
}
```

- [ ] **Step 4: Run them and see them pass**, then the workspace's suites

Run: `cargo test -p tagteam-tui`

Expected: `test result: ok. 9 passed; 0 failed`.

Run: `cargo test -p tagteam --test tui_args`

Expected: `test result: ok. 9 passed; 0 failed`.

Run: `cargo test --workspace --features tagteam/test-support`

Expected: PASS, run outside the sandbox (the `prompt` unit tests and `tests/signals.rs` open a pty). Nothing outside the new crate changes behaviour; this proves the manifest and lock changes build everything.

- [ ] **Step 5: Format and lint**

```sh
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```sh
git add Cargo.toml Cargo.lock crates/tagteam/Cargo.toml crates/tagteam-tui crates/tagteam/tests/tui_args.rs .github/workflows/publish-crates.yml CONTRIBUTING.md README.md
git commit -m "Add the tagteam-tui crate with the host and source ports, and raise the MSRV to 1.88"
```

Before committing, check that `git status` lists `crates/tagteam-tui/LICENSE` and `NOTICE` as symlinks (mode 120000), as `crates/tagteam-engine/LICENSE` is, and that `git diff --stat Cargo.lock` shows additions only apart from the handful of version-qualified lines named above.

**Existing tests this task changes:** none. (Raising `rust-version` changes no test, and the toolchain is already 1.88.0.)

---

### Task 4: Lanes

A lane is one worker thread that serves one kind of request in order (spec §4.3). It builds its
own state inside its thread, so neither `Engine` nor `Context` needs to be `Send` or `Sync`
(R4). It merges a queued request with a newer one of the same kind, so a burst of refreshes costs
one read, and it stops taking requests once it is closed or has died. The runtime (Task 11)
builds two: views (`ViewsReq`) and actions (`ActionsReq`).

**Readings of the contract this task commits to:**
- **`close` drops what is queued; the request running finishes.** The thread returns after it,
  so `join` never waits for more than one request (spec §10.2 steps 2 and 5).
- **`join` closes the lane first.** An idle lane blocked waiting for a request would otherwise
  never return, and every caller wants the close anyway. The contract's doc line still holds.
- **A death closes the lane.** A guard dropped while the thread unwinds marks the queue closed
  before it calls `on_death`, so `submit` is false from then on (spec §4.3: "A lane that panics
  closes its channel"). It also covers a panic in `init`.
- **A `Lane` dropped without `join` is closed too,** so its thread ends after its current
  request. This matters only for a panic on the UI thread before the shutdown.
- **`msg.rs` is created here with only the two request enums** and their `Coalesce` impls, as the
  task table splits it. Task 5 adds the rest of the file.

**`gen` is a reserved word in edition 2024.** rustc rejects a field named `gen` with
``expected identifier, found reserved keyword `gen` ``. Every `gen` field of C4 and C5 is written
`r#gen`: the same field, spelled as the edition requires (R8).

**Depends on:** 3

**Files:**
- Modify: `crates/tagteam-tui/src/lanes.rs` (Task 3's one-line module becomes the lanes)
- Modify: `crates/tagteam-tui/src/msg.rs` (Task 3's one-line module gains `ViewsReq`, `ActionsReq`
  and their `Coalesce` impls)
- Test: unit tests in both files

**Interfaces:**
- Consumes: Task 3's `crate::host::Action`; `tagteam_core::AccountId`.
- Produces (C4):
  - `pub trait Coalesce { fn same_as(&self, other: &Self) -> bool; }`
  - `pub struct Lane<Req: Coalesce + Send + 'static>` with `spawn`, `submit`, `close`, `join`
    exactly as C4 lists them; threads are named `tagteam-tui-<name>`.
  - `msg::ViewsReq { Accounts { r#gen }, History { r#gen, account, since_s } }` and
    `msg::ActionsReq { Collect { r#gen, accounts }, Act { action } }`, both
    `#[derive(Debug, Clone, PartialEq, Eq)]`, with `same_as` true for two of one variant, except
    `Act`, which never coalesces.

**Spec:**
- spec §4.3, "Lane" and "Coalescing": a lane merges a queued request with an identical one still
  queued; each request carries its generation.
- spec §4.3, "Joining" and "A lane that panics": a lane returns once its channel is closed and its
  current request has unwound; a panic closes its channel.
- spec §10.2 steps 2 and 5: close the lanes (dropping what has not started), then join them.
- R4: each lane builds its own source inside its thread.

- [ ] **Step 1: Write the failing tests**

Replace the whole of `crates/tagteam-tui/src/lanes.rs` (Task 3 left one `//!` line) with its
module line and this test module:

```rust
//! Worker lanes (spec §4.3): one thread each, serving its requests in order. A lane builds its
//! own state inside its thread (R4), merges a queued request with a newer one of the same kind,
//! and stops taking requests once it is closed or has died.

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::{Duration, Instant};

    use super::*;

    const WAIT: Duration = Duration::from_secs(5);

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Req {
        /// Never coalesces.
        Keep(u32),
        /// Coalesces with any other `Merge`.
        Merge(u32),
        /// Waits for the gate before it is reported.
        Gate(u32),
        /// Panics.
        Boom,
    }

    impl Coalesce for Req {
        fn same_as(&self, other: &Self) -> bool {
            matches!((self, other), (Req::Merge(_), Req::Merge(_)))
        }
    }

    /// A lane that reports every request it ran on `done`; a `Gate` waits on `gate` first.
    fn lane(name: &'static str) -> (Lane<Req>, Sender<()>, Receiver<Req>) {
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) = mpsc::channel::<Req>();
        let lane = Lane::spawn(
            name,
            move || (gate_rx, done_tx),
            |(gate, done): &mut (Receiver<()>, Sender<Req>), req: Req| {
                match req {
                    Req::Gate(_) => gate.recv_timeout(WAIT).expect("the test opens the gate"),
                    Req::Boom => panic!("the lane's work panicked"),
                    Req::Keep(_) | Req::Merge(_) => {}
                }
                let _ = done.send(req);
            },
            || {},
        );
        (lane, gate_tx, done_rx)
    }

    fn take(done: &Receiver<Req>, n: usize) -> Vec<Req> {
        (0..n)
            .map(|_| done.recv_timeout(WAIT).expect("the lane ran the request"))
            .collect()
    }

    #[test]
    fn requests_run_in_the_order_they_were_submitted() {
        let (lane, _gate, done) = lane("order");
        for n in [1, 2, 3] {
            assert!(lane.submit(Req::Keep(n)));
        }
        assert_eq!(take(&done, 3), [Req::Keep(1), Req::Keep(2), Req::Keep(3)]);
        lane.join().unwrap();
    }

    #[test]
    fn a_newer_request_replaces_a_queued_one_in_place_and_others_are_left_alone() {
        let (lane, gate, done) = lane("coalesce");
        assert!(lane.submit(Req::Gate(0)));
        assert!(lane.submit(Req::Merge(1)));
        assert!(lane.submit(Req::Keep(2)));
        assert!(lane.submit(Req::Merge(3)));
        assert!(lane.submit(Req::Keep(2)));
        gate.send(()).unwrap();
        assert_eq!(
            take(&done, 4),
            [Req::Gate(0), Req::Merge(3), Req::Keep(2), Req::Keep(2)]
        );
        lane.join().unwrap();
    }

    #[test]
    fn close_drops_what_is_queued_and_lets_the_current_request_finish() {
        let (lane, gate, done) = lane("close");
        assert!(lane.submit(Req::Gate(0)));
        // The gate request is taken before the others are queued behind it.
        let deadline = Instant::now() + WAIT;
        while !lane.shared.lock().items.is_empty() {
            assert!(
                Instant::now() < deadline,
                "the lane never took its first request"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert!(lane.submit(Req::Keep(1)));
        assert!(lane.submit(Req::Keep(2)));
        lane.close();
        gate.send(()).unwrap();
        lane.join().unwrap();
        assert_eq!(done.try_iter().collect::<Vec<_>>(), [Req::Gate(0)]);
    }

    #[test]
    fn an_idle_lane_joins_once_closed() {
        let (lane, _gate, _done) = lane("idle");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            lane.close();
            let _ = tx.send(lane.join().is_ok());
        });
        assert_eq!(rx.recv_timeout(WAIT), Ok(true));
    }

    #[test]
    fn a_closed_lane_takes_no_request() {
        let (lane, _gate, _done) = lane("closed");
        lane.close();
        assert!(!lane.submit(Req::Keep(1)));
        lane.join().unwrap();
    }

    #[test]
    fn init_runs_on_the_lanes_own_named_thread() {
        let (tx, rx) = mpsc::channel();
        let lane: Lane<Req> = Lane::spawn(
            "views",
            move || {
                let me = thread::current();
                tx.send((me.name().map(str::to_owned), me.id())).unwrap();
            },
            |_: &mut (), _: Req| {},
            || {},
        );
        let (name, id) = rx.recv_timeout(WAIT).unwrap();
        assert_eq!(name.as_deref(), Some("tagteam-tui-views"));
        assert_ne!(id, thread::current().id());
        lane.join().unwrap();
    }

    #[test]
    fn a_panicking_request_calls_on_death_once_and_join_says_so() {
        let deaths = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&deaths);
        let lane: Lane<Req> = Lane::spawn(
            "doomed",
            || (),
            |_: &mut (), req: Req| {
                if req == Req::Boom {
                    panic!("the lane's work panicked");
                }
            },
            move || {
                counted.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert!(lane.submit(Req::Boom));
        let deadline = Instant::now() + WAIT;
        while deaths.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "on_death never ran");
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!lane.submit(Req::Keep(1)), "a dead lane is closed");
        assert!(lane.join().is_err());
        assert_eq!(deaths.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_panicking_init_calls_on_death_too() {
        let deaths = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&deaths);
        let lane: Lane<Req> = Lane::spawn(
            "stillborn",
            || panic!("the lane's state could not be built"),
            |_: &mut (), _: Req| {},
            move || {
                counted.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert!(lane.join().is_err());
        assert_eq!(deaths.load(Ordering::SeqCst), 1);
    }
}
```

The tests drive a lane through a request type of their own. `Gate` holds the lane busy until the
test opens the gate, so the requests behind it are still queued when the test looks at them.

Replace the whole of `crates/tagteam-tui/src/msg.rs` with its module line and this test module:

```rust
//! The lanes' requests (spec §4.3); Task 5 adds what reaches the state machine and what it
//! asks for.

#[cfg(test)]
mod tests {
    use tagteam_core::ProviderId;

    use super::*;

    fn id(s: &str) -> AccountId {
        AccountId::from_string(s)
    }

    #[test]
    fn views_requests_of_one_kind_coalesce_whatever_they_ask() {
        let accounts = |g| ViewsReq::Accounts { r#gen: g };
        let history = |g, a: &str| ViewsReq::History {
            r#gen: g,
            account: id(a),
            since_s: 0,
        };
        assert!(accounts(1).same_as(&accounts(2)));
        assert!(history(1, "a").same_as(&history(2, "b")));
        assert!(!accounts(1).same_as(&history(1, "a")));
        assert!(!history(1, "a").same_as(&accounts(1)));
    }

    #[test]
    fn collections_coalesce_and_actions_never_do() {
        let collect = |g| ActionsReq::Collect {
            r#gen: g,
            accounts: vec![id("a")],
        };
        let act = || ActionsReq::Act {
            action: Action::AddLive {
                provider: ProviderId::new("claude-code"),
            },
        };
        assert!(collect(1).same_as(&collect(2)));
        assert!(!act().same_as(&act()));
        assert!(!collect(1).same_as(&act()));
        assert!(!act().same_as(&collect(1)));
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-tui --lib -- lanes:: msg::`
Expected: the build fails with E0412 (cannot find type `Lane`), E0405 (cannot find trait
`Coalesce`) and E0433 (use of undeclared types `ViewsReq` and `ActionsReq`).

- [ ] **Step 3: Implement**

In `crates/tagteam-tui/src/lanes.rs`, above `#[cfg(test)]`, after the module line:

```rust
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

/// Whether a queued request is replaced by a newer one (spec §4.3, "Coalescing").
pub trait Coalesce {
    fn same_as(&self, other: &Self) -> bool;
}

/// The requests not yet started, and whether the lane still takes any.
struct Queue<Req> {
    items: VecDeque<Req>,
    closed: bool,
}

struct Shared<Req> {
    queue: Mutex<Queue<Req>>,
    ready: Condvar,
}

impl<Req> Shared<Req> {
    /// The queue, whatever a panic left it as: it holds only requests and a flag.
    fn lock(&self) -> MutexGuard<'_, Queue<Req>> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn close(&self) {
        let mut q = self.lock();
        q.closed = true;
        q.items.clear();
        drop(q);
        self.ready.notify_all();
    }

    /// The next request, waiting for one; `None` once the lane is closed.
    fn next(&self) -> Option<Req> {
        let mut q = self.lock();
        loop {
            if q.closed {
                return None;
            }
            if let Some(req) = q.items.pop_front() {
                return Some(req);
            }
            q = self.ready.wait(q).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// Runs `on_death` if the lane's thread unwinds, after closing the lane so that nothing more
/// is queued for a thread that will never take it.
struct Death<Req, F: FnOnce()> {
    shared: Arc<Shared<Req>>,
    on_death: Option<F>,
}

impl<Req, F: FnOnce()> Drop for Death<Req, F> {
    fn drop(&mut self) {
        if thread::panicking() {
            self.shared.close();
            if let Some(f) = self.on_death.take() {
                f();
            }
        }
    }
}

pub struct Lane<Req: Coalesce + Send + 'static> {
    shared: Arc<Shared<Req>>,
    handle: Option<JoinHandle<()>>,
}

impl<Req: Coalesce + Send + 'static> Lane<Req> {
    /// Spawns the lane's thread. `init` runs first, inside the thread (R4). `work` runs each
    /// request in order. `on_death` runs once if the thread unwinds from a panic.
    pub fn spawn<S: 'static>(
        name: &'static str,
        init: impl FnOnce() -> S + Send + 'static,
        mut work: impl FnMut(&mut S, Req) + Send + 'static,
        on_death: impl FnOnce() + Send + 'static,
    ) -> Lane<Req> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                items: VecDeque::new(),
                closed: false,
            }),
            ready: Condvar::new(),
        });
        let inner = Arc::clone(&shared);
        let handle = thread::Builder::new()
            .name(format!("tagteam-tui-{name}"))
            .spawn(move || {
                let _death = Death {
                    shared: Arc::clone(&inner),
                    on_death: Some(on_death),
                };
                let mut state = init();
                while let Some(req) = inner.next() {
                    work(&mut state, req);
                }
            })
            .expect("the system could not start a thread for a TUI lane");
        Lane {
            shared,
            handle: Some(handle),
        }
    }

    /// Queues `req`. A queued request that `same_as` it is replaced in place (the newest wins).
    /// False once the lane is closed.
    pub fn submit(&self, req: Req) -> bool {
        let mut q = self.shared.lock();
        if q.closed {
            return false;
        }
        match q.items.iter_mut().find(|queued| queued.same_as(&req)) {
            Some(queued) => *queued = req,
            None => q.items.push_back(req),
        }
        drop(q);
        self.shared.ready.notify_one();
        true
    }

    /// Drops every queued request and wakes the thread, which returns after its current one.
    pub fn close(&self) {
        self.shared.close();
    }

    /// Joins the thread; Err when it panicked. It closes the lane first, so an idle lane
    /// returns at once and a busy one after its current request.
    pub fn join(mut self) -> std::thread::Result<()> {
        self.close();
        match self.handle.take() {
            Some(handle) => handle.join(),
            None => Ok(()),
        }
    }
}

impl<Req: Coalesce + Send + 'static> Drop for Lane<Req> {
    /// A lane dropped without `join` is closed, so its thread ends after its current request.
    fn drop(&mut self) {
        self.close();
    }
}
```

In `crates/tagteam-tui/src/msg.rs`, above `#[cfg(test)]`, after the module line:

```rust
use tagteam_core::AccountId;

use crate::host::Action;
use crate::lanes::Coalesce;

/// The views lane's requests: store-served reads, no network (spec §4.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewsReq {
    Accounts {
        r#gen: u64,
    },
    History {
        r#gen: u64,
        account: AccountId,
        since_s: i64,
    },
}

/// The actions lane's requests: the collector and every `Host` action, one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionsReq {
    Collect {
        r#gen: u64,
        accounts: Vec<AccountId>,
    },
    Act {
        action: Action,
    },
}

impl Coalesce for ViewsReq {
    /// Any two of the same kind: only the newest read is worth making.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (ViewsReq::Accounts { .. }, ViewsReq::Accounts { .. })
                | (ViewsReq::History { .. }, ViewsReq::History { .. })
        )
    }
}

impl Coalesce for ActionsReq {
    /// Two collections merge; an action never does.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (ActionsReq::Collect { .. }, ActionsReq::Collect { .. })
        )
    }
}
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-tui --lib -- lanes:: msg::`
Expected: PASS, 10 tests. `a_panicking_request_calls_on_death_once_and_join_says_so` and
`a_panicking_init_calls_on_death_too` print their panic messages on stderr; that is the default
panic hook, and the tests still pass.

Run: `cargo test -p tagteam-tui`
Expected: PASS.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-tui/src/lanes.rs crates/tagteam-tui/src/msg.rs
git commit -m "Add the TUI's worker lanes and their requests"
```

---

### Task 5: State machine core and chrome

The Elm-style core (spec §4.2): `State` holds everything on screen, `update` applies one `Msg` and
returns `Cmd`s, and `view` draws the state and reads nothing else. This task gives the TUI its
chrome (the header, the banner line, the footer with its toast, spinner and `finishing…`, the
too-small notice, the help panel and the confirm dialog) and routes keys. The screens are stubs
that Tasks 6 and 7 replace, and the action keys and other dialogs are Task 9's.

**Readings of the spec this task commits to:**
- **Layout (spec §5.1).** Row 0 is the header: ` tagteam  [1 Accounts]  3 History` on the left,
  `○ refresh 30s   ? help` on the right. When there is a banner it takes row 1, and the rows
  below it its words need at the width, three at most (R21). The last row is the footer, and the
  rows above it a toast's words need, three at most. The body is everything between, a row at
  least. Only text beyond a third row is cut, ending in `…`. Part A has no activity line: timer
  mode hides it. Below 80×24 only `terminal too small (80×24 needed)` is drawn, and nothing else, overlays
  included. The overlay itself stays in `State`.
- **The footer** shows, in this order of precedence: `finishing <action>…` (or `finishing…`) once
  `quitting` is set; the toast; the running action's spinner and name (`⠋ switch…`); the screen's
  keys. The keys are dropped from the end when the line is too narrow. The line starts at column
  0, so spec §5.1's key line (80 columns without its indent) fits an 80-column terminal exactly.
- **Key routing.** Keys are ignored once `quitting` is set (a second `q` does not skip the wait,
  spec §10.2 step 4). Ctrl-C quits with 0 and Ctrl-Z stops, always. With an overlay open, every
  other key goes to the overlay, so that a text dialog can take `q`, `1` or `?` (R9). Otherwise `q` quits, `1`, `3`, `Tab` and `Shift-Tab` switch screens (`2` waits for
  part B), `?` opens help, then the action keys (Task 9), then the screen's `handle_key`. A key
  release is ignored.
- **`quit` sets `quitting` and drops the overlay.** A dialog's reply sender is dropped with it,
  so a lane waiting on it gets a decline (spec §10.2 step 3, with Task 9's `ChannelAsk`).
- **`LaneDied`** sets the banner (`the views lane stopped unexpectedly`) and quits with 1, once.
  The runtime prints the banner on stderr after restoring the terminal (spec §10.4).
- **Ticks.** A toast expires on the first tick at or after `until_ms` (5 s, spec §5.1). The
  screen is redrawn once a second, so a countdown's text is never more than a second late
  (spec §5.2, "Countdowns tick"), and on every tick while an action runs or the TUI is
  finishing, so the spinner turns. `State` keeps the second it last drew for in a `pub(crate)
  shown_s` field.
- **Colour (spec §5.1, core §13.5).** `severity_style` maps C1's `Severity` to yellow and red, or
  to bold without colour. An error banner takes the critical style, any other banner the warning
  style. The selection is reverse video with colour or without.
- **What this task leaves to others.** The lanes' results (`Accounts`, `History`, `Collected`)
  change nothing until Task 8 applies them by generation. `on_prompt` declines every prompt and
  `on_done` toasts the first line, as C7 assigns. `handle_overlay_key` and `render_overlay`
  handle `Help` and also `Confirm`, because Review Focus 1 needs a confirm dialog's answer to
  reach its sender (contract C7). `Esc` closes the other overlays.
- **`State::start`** returns `LoadAccounts { r#gen: 1 }` only. History needs an account, which
  only the first `accounts` result names; Task 8 reads History's then (contract C5).

**Depends on:** 1, 4. Task 1 gives `tagteam_engine::words::Severity` and the `UsageView.next_poll_at`
field the fixtures set; Task 4 creates `msg.rs`, which this task extends (above Task 4's enums
and tests, which it keeps), so the two never run in one batch.

**Files:**
- Modify: `crates/tagteam-tui/src/lib.rs` (`pub use state::Screen;` and the test-only
  `testutil` module, after the module list)
- Modify: `crates/tagteam-tui/src/msg.rs` (Task 4's file gains `Msg`, `Prompt`, `PromptKind`,
  `Answer` and `Cmd`)
- Modify: `crates/tagteam-tui/src/state.rs`, `update.rs`, `view.rs`, `actions.rs`,
  `screens/mod.rs` (Task 3's one-line modules)
- Create: `crates/tagteam-tui/src/screens/accounts.rs`, `crates/tagteam-tui/src/screens/history.rs`
  (stubs that Tasks 6 and 7 replace)
- Create: `crates/tagteam-tui/src/testutil.rs` (the fixtures every later task's tests use)
- Test: unit tests in `update.rs`, `view.rs` and `actions.rs`

**Interfaces:**
- Consumes:
  - Task 3: `host::{Action, AccountRef, ActionOutcome}` (`Action::name`), `source::{
    AccountsSnapshot, CollectSummary, SourceError}`.
  - Task 4: `msg::{ViewsReq, ActionsReq}` (kept above the new types).
  - Task 1: `tagteam_engine::words::Severity`.
  - ratatui 0.30.2 (ratatui-core 0.1.2, ratatui-widgets 0.3.2): `Frame::{area, render_widget}`;
    `Terminal::{new, draw, backend, backend_mut}`; `backend::TestBackend::{new, resize, buffer}`
    (its `Backend::Error` is `Infallible`, and `draw` resizes to the backend's size by itself);
    `buffer::{Buffer, CellWidth}` with `Buffer[(x, y)]`, `Buffer::area` and
    `Cell::{symbol, fg, modifier}`; `layout::Rect::new`; `style::{Style, Color, Modifier}`;
    `text::{Line, Span}`; `widgets::{Paragraph, Block, Clear, Padding}` (`Paragraph::{new, block,
    scroll}`, `Block::{bordered, title, padding}`, `Padding::horizontal`).
  - crossterm 0.29.0: `event::{KeyEvent, KeyCode, KeyModifiers, KeyEventKind}`, `KeyEvent::new`
    and `KeyEvent::new_with_kind`, and `KeyCode::BackTab` for Shift-Tab. ratatui's default
    backend is built on the same crossterm, so the runtime's `KeyEvent` is this one.
- Produces:
  - C5 exactly: `Msg`, `Prompt`, `PromptKind`, `Answer`, `Cmd` (`Cmd` derives `Debug, Clone,
    PartialEq, Eq`); `Screen`; `State` with every C5 field, plus `pub(crate) shown_s: i64`;
    `Toast`, `Gens`, `Refresh`; `State::new`, `State::start`; `update::update`;
    `view::view`.
  - C6's stubs: `screens::accounts::{AccountsState, handle_key, render}` and
    `screens::history::{HistoryState, handle_key, render}`. `AccountsState` already has C6's
    `snapshot()` and `apply()` (storing the read), and `HistoryState` has `account()` and
    `pub fn select(&mut self, account: AccountId)`, so that Tasks 6 and 7 can run in parallel:
    Task 7 reads the accounts through `snapshot()`, and Task 6's `Enter` calls `select`.
  - C7's Task 5 part: `Overlay`, `ConfirmThen`, `TextThen` (each `#[derive(Debug)]`) and the four
    functions with C7's signatures.
  - Crate-private, for Tasks 6 to 9 and 11:
    - `State::load_accounts(&mut self) -> Cmd` and `State::toast(&mut self, text, now_ms)`;
    - `update::quit(state, code) -> Vec<Cmd>` and `update::show(state, screen, now_ms) ->
      Vec<Cmd>`;
    - `view::{areas, Areas, banner, width_of, fit, pad, wrap, wrap_rows, centered, bold,
      selected, severity_style}` (`wrap_rows(text, width, max)`: `wrap`, at most `max` rows,
      the last ending in `…` when cut; R20, R21);
    - `screens::accounts::banner(&AccountsState) -> Option<String>` (a stub here) and
      `screens::history::on_enter(state, now_ms) -> Vec<Cmd>` (a stub here);
    - the `testutil` fixtures of Step 1 (`#[cfg(test)] pub(crate) mod testutil`), unchanged by
      every later task. Their names avoid the screens' own `render` and `history`, which a glob
      import would make ambiguous (E0659).

**Spec:**
- spec §4.2: `State`, a pure `update`, a pure `view`.
- spec §5.1: the header, banner, footer, toast (5 s), spinner, colour and the 80×24 minimum.
- spec §5.5: help lists every key for the current screen and the global keys; `Esc` declines a
  dialog.
- spec §5.6 "Everywhere" row: the screen keys, `?`, `q`/Ctrl-C, Ctrl-Z.
- spec §10.2: the triggers that start a shutdown and the exit codes `update` can set (0 and 1);
  step 4's `finishing <action>…`.
- spec §10.4: a lane that died is fatal.
- core §13.5: ≥ 70 % warning, ≥ 90 % critical.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-tui/src/testutil.rs`. It builds the engine's views from their public
fields, as `crates/tagteam/src/render.rs`'s `testutil` module does (`render.rs:742-902` today), and
reads a `TestBackend` back as text: a cell a wide character covers is skipped, as a terminal
shows it.

```rust
//! Fixtures for the state machine's, the screens' and the overlays' tests: views built from
//! the engine's public types, keys, and a `TestBackend` read back as text.
#![allow(dead_code)]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, CellWidth};
use tagteam_core::{AccountId, CLAUDE_CODE, Pace, ProviderId, Sample, Window, WindowKind};
use tagteam_engine::store::AccountRow;
use tagteam_engine::views::{
    AccountView, HistoryView, HistoryWindow, ProviderAccounts, UsageStatus, UsageView,
};
use tagteam_provider::KindTraits;

use crate::msg::Msg;
use crate::source::AccountsSnapshot;
use crate::state::{Screen, State};

/// Any fixed instant: every reading, reset and sample below is placed relative to it.
pub(crate) const NOW_S: i64 = 1_790_000_000;
pub(crate) const NOW_MS: i64 = NOW_S * 1000;
pub(crate) const HOUR: i64 = 3_600;

pub(crate) const OAUTH: KindTraits = KindTraits {
    refreshable: true,
    managed_key_axis: false,
    default_email_prefix: None,
    display: None,
};

pub(crate) const API_KEY: KindTraits = KindTraits {
    refreshable: false,
    managed_key_axis: true,
    default_email_prefix: Some("api-key"),
    display: Some("api key"),
};

pub(crate) fn cc() -> ProviderId {
    ProviderId::new(CLAUDE_CODE)
}

/// The id every fixture account at `position` has.
pub(crate) fn id(position: u32) -> AccountId {
    AccountId::from_string(format!("id-{position}"))
}

/// `email`'s inactive Claude Code account at `position`, with `usage`.
pub(crate) fn account(position: u32, email: &str, usage: UsageView) -> AccountView {
    AccountView {
        row: AccountRow {
            id: id(position),
            provider: cc(),
            position,
            identity_key: format!("{email}\n"),
            label: email.into(),
            email: Some(email.into()),
            org_uuid: String::new(),
            org_name: None,
            account_uuid: None,
            kind: "oauth".into(),
            alias: None,
            disabled: false,
            identity_json: Default::default(),
            login_expires_at: None,
            login_epoch: 0,
            replacing_fp: None,
            quarantine_reason: None,
            quarantine_fp: None,
            quarantine_at: None,
            added_at: 1,
        },
        active: false,
        kind: OAUTH,
        usage,
        in_session: false,
    }
}

/// No reading: `status`, with `error` and a retry `retry_in` seconds from now.
pub(crate) fn unread(status: UsageStatus, error: Option<&str>, retry_in: Option<i64>) -> UsageView {
    UsageView {
        status,
        windows: None,
        decision_grade: false,
        fetched_at: None,
        age_s: None,
        error: error.map(str::to_owned),
        retry_at: retry_in.map(|s| NOW_S + s),
        next_poll_at: None,
    }
}

pub(crate) fn window(
    key: &str,
    label: &str,
    kind: WindowKind,
    pct: f64,
    resets_in: Option<i64>,
) -> Window {
    Window {
        key: key.into(),
        label: label.into(),
        kind,
        pct,
        resets_at: resets_in.map(|s| NOW_S + s),
        period_s: None,
        detail: None,
    }
}

/// A good reading `age_s` old: 5h at `five` resetting in 2h40m30s, 7d at `seven` resetting in
/// 3d09h00m30s and ahead of pace when `ahead`.
pub(crate) fn read(age_s: i64, five: f64, seven: f64, ahead: bool) -> UsageView {
    UsageView {
        status: UsageStatus::Ok,
        windows: Some(vec![
            (
                window("5h", "5h", WindowKind::Short, five, Some(9_630)),
                Pace::default(),
            ),
            (
                window("7d", "7d", WindowKind::Long, seven, Some(291_630)),
                Pace {
                    ahead: Some(ahead),
                    ..Pace::default()
                },
            ),
        ]),
        decision_grade: age_s <= 300,
        fetched_at: Some(NOW_S - age_s),
        age_s: Some(age_s),
        error: None,
        retry_at: None,
        next_poll_at: None,
    }
}

/// One Claude Code provider holding `accounts`, read at `NOW_S`.
pub(crate) fn snapshot(accounts: Vec<AccountView>) -> AccountsSnapshot {
    AccountsSnapshot {
        providers: vec![ProviderAccounts {
            provider: cc(),
            active_position: accounts.iter().find(|v| v.active).map(|v| v.row.position),
            accounts,
        }],
        this: None,
        unmanaged_live: None,
        in_run_shell: false,
        read_at_s: NOW_S,
    }
}

/// `n` accounts `a1@x.co` … at positions 1 to `n`, each with a reading.
pub(crate) fn many(n: u32) -> Vec<AccountView> {
    (1..=n)
        .map(|p| account(p, &format!("a{p}@x.co"), read(60, 10.0, 20.0, false)))
        .collect()
}

/// A State on `screen` at 80×24, without colour, nothing loaded yet.
pub(crate) fn fresh(screen: Screen) -> State {
    let mut state = State::new(screen, false, None, NOW_MS);
    state.size = (80, 24);
    state
}

pub(crate) fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(crate) fn key(code: KeyCode) -> Msg {
    Msg::Key(press(code))
}

pub(crate) fn ch(c: char) -> Msg {
    let modifiers = if c.is_ascii_uppercase() {
        KeyModifiers::SHIFT
    } else {
        KeyModifiers::NONE
    };
    Msg::Key(KeyEvent::new(KeyCode::Char(c), modifiers))
}

pub(crate) fn ctrl(c: char) -> Msg {
    Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

pub(crate) fn terminal(w: u16, h: u16) -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(w, h)).unwrap()
}

/// What `view` draws of `state` on a `w`×`h` terminal at `now_ms`.
pub(crate) fn draw_buffer(state: &State, w: u16, h: u16, now_ms: i64) -> Buffer {
    let mut terminal = terminal(w, h);
    terminal
        .draw(|frame| crate::view::view(state, frame, now_ms))
        .unwrap();
    terminal.backend().buffer().clone()
}

pub(crate) fn draw(state: &State, w: u16, h: u16) -> Vec<String> {
    draw_at(state, w, h, NOW_MS)
}

pub(crate) fn draw_at(state: &State, w: u16, h: u16, now_ms: i64) -> Vec<String> {
    text_of(&draw_buffer(state, w, h, now_ms))
}

/// Each row of `buf` as text, trailing spaces trimmed. A cell a wide character covers is
/// skipped, as a terminal shows it.
pub(crate) fn text_of(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| row(buf, y).0.trim_end().to_owned())
        .collect()
}

/// Row `y` as text, with the column each byte of it starts at.
fn row(buf: &Buffer, y: u16) -> (String, Vec<(usize, u16)>) {
    let mut text = String::new();
    let mut starts = Vec::new();
    let mut hidden = 0u16;
    for x in 0..buf.area.width {
        let cell = &buf[(x, y)];
        if hidden == 0 {
            starts.push((text.len(), x));
            text.push_str(cell.symbol());
        }
        hidden = hidden.max(cell.cell_width()).saturating_sub(1);
    }
    (text, starts)
}

/// Where `needle` first appears in `buf`: its column and row.
pub(crate) fn locate(buf: &Buffer, needle: &str) -> Option<(u16, u16)> {
    (0..buf.area.height).find_map(|y| {
        let (text, starts) = row(buf, y);
        let at = text.find(needle)?;
        starts.iter().find(|(b, _)| *b == at).map(|(_, x)| (*x, y))
    })
}

/// A sample of one window taken `ago_s` before `NOW_S`.
pub(crate) fn sample(ago_s: i64, pct: f64) -> Sample {
    Sample {
        fetched_at: NOW_S - ago_s,
        pct,
        resets_at: None,
    }
}

pub(crate) fn history_window(window: Window, samples: Vec<Sample>, pace: Pace) -> HistoryWindow {
    HistoryWindow {
        window,
        samples,
        pace,
    }
}

pub(crate) fn history_of(account: AccountView, windows: Vec<HistoryWindow>) -> HistoryView {
    HistoryView {
        account,
        windows,
        unmatched_window: false,
    }
}
```

In `crates/tagteam-tui/src/lib.rs`, after the module list Task 3 wrote (rustfmt keeps it in
alphabetical order), add:

```rust
pub use state::Screen;

#[cfg(test)]
pub(crate) mod testutil;
```

Task 3 cannot export `Screen` from an empty module, so the export lands here; Task 11 adds C3's
`pub use runtime::{run, TuiOptions, TuiExit};`.

Replace `crates/tagteam-tui/src/update.rs` with its module line and this test module:

```rust
//! The state machine (spec §4.2): `update` applies one `Msg` and returns the `Cmd`s it needs. It
//! does no I/O and reads no clock: `now_ms` is the caller's.

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    use super::*;
    use crate::testutil::*;

    fn quit_code(cmds: &[Cmd]) -> Option<i32> {
        match cmds {
            [Cmd::Quit { code }] => Some(*code),
            _ => None,
        }
    }

    #[test]
    fn the_screen_keys_switch_screens_and_two_waits_for_part_b() {
        let mut state = fresh(Screen::Accounts);
        update(&mut state, ch('3'), NOW_MS);
        assert_eq!(state.screen, Screen::History);
        update(&mut state, ch('2'), NOW_MS);
        assert_eq!(
            state.screen,
            Screen::History,
            "2 is the Tokens screen, part B's"
        );
        update(&mut state, ch('1'), NOW_MS);
        assert_eq!(state.screen, Screen::Accounts);
        update(&mut state, key(KeyCode::Tab), NOW_MS);
        assert_eq!(state.screen, Screen::History);
        update(
            &mut state,
            Msg::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
            NOW_MS,
        );
        assert_eq!(state.screen, Screen::Accounts);
    }

    #[test]
    fn q_and_ctrl_c_quit_with_0_and_ctrl_z_stops() {
        for msg in [ch('q'), ctrl('c')] {
            let mut state = fresh(Screen::Accounts);
            assert_eq!(quit_code(&update(&mut state, msg, NOW_MS)), Some(0));
            assert_eq!(state.quitting, Some(0));
            // A second `q` does not skip the wait (spec §10.2 step 4): keys are ignored.
            assert!(update(&mut state, ch('q'), NOW_MS).is_empty());
            assert!(update(&mut state, ch('3'), NOW_MS).is_empty());
            assert_eq!(state.screen, Screen::Accounts);
        }
        let mut state = fresh(Screen::Accounts);
        assert_eq!(update(&mut state, ctrl('z'), NOW_MS), [Cmd::Stop]);
        assert_eq!(state.quitting, None);
    }

    #[test]
    fn help_toggles_and_takes_the_keys_while_open() {
        let mut state = fresh(Screen::Accounts);
        update(&mut state, ch('?'), NOW_MS);
        assert!(matches!(state.overlay, Some(Overlay::Help)));
        update(&mut state, ch('3'), NOW_MS);
        assert_eq!(state.screen, Screen::Accounts, "the overlay has the keys");
        update(&mut state, ch('?'), NOW_MS);
        assert!(state.overlay.is_none());
        update(&mut state, ch('?'), NOW_MS);
        assert_eq!(quit_code(&update(&mut state, ctrl('c'), NOW_MS)), Some(0));
        assert!(state.overlay.is_none());
    }

    #[test]
    fn a_resize_is_recorded_and_a_release_is_ignored() {
        let mut state = fresh(Screen::Accounts);
        state.dirty = false;
        update(
            &mut state,
            Msg::Resize {
                width: 120,
                height: 40,
            },
            NOW_MS,
        );
        assert_eq!((state.size, state.dirty), ((120, 40), true));
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert!(update(&mut state, Msg::Key(release), NOW_MS).is_empty());
        assert_eq!(state.quitting, None);
    }

    #[test]
    fn a_tick_redraws_once_a_second_and_while_an_action_runs() {
        let mut state = fresh(Screen::Accounts);
        state.dirty = false;
        update(&mut state, Msg::Tick, NOW_MS + 250);
        assert!(!state.dirty, "the same second");
        update(&mut state, Msg::Tick, NOW_MS + 1_000);
        assert!(state.dirty, "a countdown may have changed");
        state.dirty = false;
        state.running = Some(crate::host::Action::AddLive { provider: cc() });
        update(&mut state, Msg::Tick, NOW_MS + 1_250);
        assert!(state.dirty, "the spinner turns");
    }

    #[test]
    fn a_lane_that_died_ends_the_tui_with_1() {
        let mut state = fresh(Screen::Accounts);
        let cmds = update(&mut state, Msg::LaneDied { lane: "views" }, NOW_MS);
        assert_eq!(quit_code(&cmds), Some(1));
        assert_eq!(
            state.banner.as_deref(),
            Some("the views lane stopped unexpectedly")
        );
        assert!(update(&mut state, Msg::LaneDied { lane: "actions" }, NOW_MS).is_empty());
        assert_eq!(state.quitting, Some(1));
    }

    #[test]
    fn start_reads_the_accounts_first() {
        let mut state = fresh(Screen::History);
        assert_eq!(state.start(), [Cmd::LoadAccounts { r#gen: 1 }]);
        assert_eq!(state.gens.accounts, 1);
    }
}
```

Replace `crates/tagteam-tui/src/view.rs` with its module line and this test module. The last
test is Review Focus 1: a confirm dialog bound to a reply sender is drawn, survives resizes down
to 1×1 and back through the real `view` on one `Terminal<TestBackend>`, and its answer still
arrives.

```rust
//! The renderer (spec §4.2, §5.1): `view` draws the state and reads nothing else.

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use ratatui::style::Modifier;

    use super::*;
    use crate::actions::{ConfirmThen, Overlay};
    use crate::host::Action;
    use crate::msg::{Answer, Msg};
    use crate::testutil::*;
    use crate::update::update;

    #[test]
    fn the_header_shows_the_tabs_and_the_mode_and_the_footer_the_screens_keys() {
        let state = fresh(Screen::Accounts);
        let lines = draw(&state, 80, 24);
        // 33 columns of tabs, 24 of gap, and the mode and `? help` up to the last column.
        assert_eq!(
            lines[0],
            format!(
                " tagteam  [1 Accounts]  3 History{}○ refresh 30s   ? help",
                " ".repeat(24)
            )
        );
        assert_eq!(
            lines[23],
            "s switch  b best  n next  e enable/disable  a alias  m move  x remove  r refresh"
        );
        let wide = draw(&state, 120, 40);
        assert!(wide[0].ends_with("○ refresh 30s   ? help"), "{}", wide[0]);
        assert_eq!(
            wide[39],
            "s switch  b best  n next  e enable/disable  a alias  m move  x remove  r refresh  R run  Enter history"
        );
        let history = draw(&fresh(Screen::History), 80, 24);
        assert_eq!(
            history[0],
            format!(
                " tagteam  1 Accounts  [3 History]{}○ refresh 30s   ? help",
                " ".repeat(24)
            )
        );
        assert_eq!(history[23], "j/k account  [ ] range  r refresh  1 accounts");
    }

    #[test]
    fn below_80_by_24_only_the_notice_is_drawn() {
        let mut state = fresh(Screen::Accounts);
        state.toast("a toast", NOW_MS);
        for (w, h) in [(79, 24), (80, 23), (20, 5)] {
            let lines = draw(&state, w, h);
            let cut: String = TOO_SMALL.chars().take(usize::from(w)).collect();
            assert_eq!(lines[0], cut, "{w}x{h}");
            assert!(
                lines[1..].iter().all(String::is_empty),
                "{w}x{h}: {lines:?}"
            );
        }
    }

    #[test]
    fn the_footer_gives_way_to_a_toast_a_spinner_and_finishing() {
        let mut state = fresh(Screen::Accounts);
        state.running = Some(Action::AddLive { provider: cc() });
        assert_eq!(draw(&state, 80, 24)[23], "⠋ add…");
        let later = draw_at(&state, 80, 24, NOW_MS + 250);
        assert_eq!(later[23], "⠙ add…");
        state.toast("busy: add", NOW_MS);
        assert_eq!(draw(&state, 80, 24)[23], "busy: add");
        state.quitting = Some(0);
        assert_eq!(draw(&state, 80, 24)[23], "finishing add…");
        state.running = None;
        assert_eq!(draw(&state, 80, 24)[23], "finishing…");
    }

    #[test]
    fn a_toast_lasts_five_seconds() {
        let mut state = fresh(Screen::Accounts);
        state.toast("Switched", NOW_MS);
        update(&mut state, Msg::Tick, NOW_MS + 4_999);
        assert!(state.toast.is_some());
        state.dirty = false;
        update(&mut state, Msg::Tick, NOW_MS + 5_000);
        assert!(state.toast.is_none());
        assert!(state.dirty);
    }

    #[test]
    fn a_long_banner_wraps_onto_three_rows_at_most_and_the_body_gives_them_up() {
        // Spec §5.1: 80×24 is enough, so nothing a banner says may be lost at 80 columns.
        let mut state = fresh(Screen::Accounts);
        let words = "the store could not be opened because another tagteam holds its lock";
        state.banner = Some(format!("{words}; {words}"));
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[1..3],
            [
                " the store could not be opened because another tagteam holds its lock; the store",
                " could not be opened because another tagteam holds its lock",
            ]
        );
        assert_eq!(
            areas(&state, Rect::new(0, 0, 80, 24)).body,
            Rect::new(0, 3, 80, 20),
            "two rows, then the body"
        );
        state.banner = Some([words; 5].join("; "));
        let lines = draw(&state, 80, 24);
        assert!(lines[3].ends_with('…'), "{lines:?}");
        assert_eq!(width_of(&lines[3]), 80);
        assert_eq!(
            areas(&state, Rect::new(0, 0, 80, 24)).body,
            Rect::new(0, 4, 80, 19),
            "three rows at most"
        );
    }

    #[test]
    fn a_long_toast_wraps_upwards_from_the_footer() {
        let mut state = fresh(Screen::Accounts);
        let text = "Alias of 2 averyveryverylongname.with.a.long.tail@example.com set to the-most-descriptive-alias-anyone-could-want.";
        state.toast(text, NOW_MS);
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[22..],
            [
                "Alias of 2 averyveryverylongname.with.a.long.tail@example.com set to",
                "the-most-descriptive-alias-anyone-could-want.",
            ]
        );
        assert_eq!(
            areas(&state, Rect::new(0, 0, 80, 24)).body,
            Rect::new(0, 1, 80, 21),
            "the body gives up a row"
        );
    }

    #[test]
    fn an_error_banner_is_red_with_colour_and_bold_without() {
        for color in [true, false] {
            let mut state = fresh(Screen::Accounts);
            state.color = color;
            state.banner = Some("the store is locked".into());
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let lines = text_of(&buf);
            assert_eq!(lines[1], " the store is locked");
            let cell = &buf[(1, 1)];
            if color {
                assert_eq!(cell.fg, Color::Red);
                assert!(!cell.modifier.contains(Modifier::BOLD));
            } else {
                assert_eq!(cell.fg, Color::Reset);
                assert!(cell.modifier.contains(Modifier::BOLD));
            }
        }
    }

    #[test]
    fn text_is_cut_by_display_width_never_by_bytes() {
        assert_eq!(fit("山田太郎@x.co", 5), "山田…");
        assert_eq!(width_of(&fit("山田太郎@x.co", 4)), 3);
        assert_eq!(fit("😀😀😀", 4), "😀…");
        assert_eq!(fit("a@x.co", 6), "a@x.co");
        assert_eq!(fit("abc", 0), "");
        assert_eq!(pad("山田", 6), "山田  ");
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("山田太郎", 3), ["山", "田", "太", "郎"]);
        assert_eq!(wrap("山田", 1), ["山", "田"], "never an empty cut");
    }

    /// Review Focus 1: a dialog survives a resize below 80×24 and back, nothing panics, and its
    /// answer still reaches the lane waiting for it.
    #[test]
    fn a_dialog_survives_a_resize_below_the_minimum_and_back() {
        let (reply, answer) = std::sync::mpsc::channel::<Answer>();
        let mut state = fresh(Screen::Accounts);
        state.overlay = Some(Overlay::Confirm {
            question: "Remove 2 b@x.co?".into(),
            default_yes: false,
            choice: false,
            then: ConfirmThen::Reply(reply),
        });
        let drawn = draw(&state, 80, 24);
        assert!(
            drawn.iter().any(|l| l.contains("Remove 2 b@x.co?")),
            "{drawn:?}"
        );

        let mut terminal = terminal(80, 24);
        for (w, h) in [(60, 20), (10, 3), (1, 1), (80, 24)] {
            terminal.backend_mut().resize(w, h);
            update(
                &mut state,
                Msg::Resize {
                    width: w,
                    height: h,
                },
                NOW_MS,
            );
            terminal
                .draw(|frame| crate::view::view(&state, frame, NOW_MS))
                .unwrap();
            assert!(state.overlay.is_some(), "{w}x{h}");
        }
        let back = text_of(terminal.backend().buffer());
        assert!(
            back.iter().any(|l| l.contains("Remove 2 b@x.co?")),
            "{back:?}"
        );
        update(&mut state, key(KeyCode::Char('y')), NOW_MS);
        assert_eq!(answer.try_recv(), Ok(Answer::Confirm(true)));
        assert!(state.overlay.is_none());
    }
}
```

Replace `crates/tagteam-tui/src/actions.rs` with its module line and this test module:

```rust
//! Actions in the TUI (spec §5.5, §7): the overlays, the CLI's prompts, and what an action's
//! outcome shows. Task 9 adds the action keys, the text dialogs and the panels.

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::msg::Msg;
    use crate::testutil::*;
    use crate::update::update;

    #[test]
    fn until_the_dialogs_exist_every_prompt_is_declined() {
        let mut state = fresh(Screen::Accounts);
        let (reply, answer) = mpsc::channel();
        let confirm = Prompt {
            question: "Add it?".into(),
            kind: PromptKind::Confirm { default_yes: true },
            reply,
        };
        update(&mut state, Msg::Prompt(confirm), NOW_MS);
        assert_eq!(answer.try_recv(), Ok(Answer::Confirm(false)));
        let (reply, answer) = mpsc::channel();
        let choose = Prompt {
            question: "Which?".into(),
            kind: PromptKind::Choose {
                options: vec!["a".into()],
            },
            reply,
        };
        update(&mut state, Msg::Prompt(choose), NOW_MS);
        assert_eq!(answer.try_recv(), Ok(Answer::Choose(None)));
        assert!(state.overlay.is_none());
    }

    #[test]
    fn an_outcome_ends_the_action_with_a_toast_of_its_first_line() {
        let mut state = fresh(Screen::Accounts);
        let action = Action::AddLive { provider: cc() };
        state.running = Some(action.clone());
        let outcome = ActionOutcome::Done {
            exit: 0,
            out: "\nAdded 1 a@x.co.\nmore\n".into(),
            err: String::new(),
        };
        let cmds = update(&mut state, Msg::ActionDone { action, outcome }, NOW_MS);
        assert!(cmds.is_empty());
        assert!(state.running.is_none());
        assert_eq!(state.toast.as_ref().unwrap().text, "Added 1 a@x.co.");
        assert_eq!(draw(&state, 80, 24)[23], "Added 1 a@x.co.");
    }

    #[test]
    fn help_lists_the_keys_and_closes() {
        let mut state = fresh(Screen::Accounts);
        update(&mut state, ch('?'), NOW_MS);
        let drawn = draw(&state, 80, 24);
        assert!(drawn.iter().any(|l| l.contains("Everywhere")), "{drawn:?}");
        assert!(
            drawn
                .iter()
                .any(|l| l.contains("run a session on the account"))
        );
        update(&mut state, ch('?'), NOW_MS);
        assert!(state.overlay.is_none());
        update(&mut state, ch('?'), NOW_MS);
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert!(state.overlay.is_none());
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-tui --lib`
Expected: the build fails: `State`, `Screen`, `Msg`, `Cmd`, `update`, `view`, `Overlay` and
the screens' modules are not defined (E0412, E0425, E0432 and E0433 against `crate::state`,
`crate::update`, `crate::view` and `crate::screens::accounts`).

- [ ] **Step 3: Implement**

`crates/tagteam-tui/src/msg.rs`, everything above its test module, becomes:

```rust
//! What reaches the state machine (`Msg`), what it asks for (`Cmd`), and the lanes' requests
//! (spec §4.2, §4.3).

use std::sync::mpsc::Sender;

use tagteam_core::AccountId;
use tagteam_engine::views::HistoryView;

use crate::host::{Action, ActionOutcome};
use crate::lanes::Coalesce;
use crate::source::{AccountsSnapshot, CollectSummary, SourceError};

/// The views lane's requests: store-served reads, no network (spec §4.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewsReq {
    Accounts {
        r#gen: u64,
    },
    History {
        r#gen: u64,
        account: AccountId,
        since_s: i64,
    },
}

/// The actions lane's requests: the collector and every `Host` action, one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionsReq {
    Collect {
        r#gen: u64,
        accounts: Vec<AccountId>,
    },
    Act {
        action: Action,
    },
}

impl Coalesce for ViewsReq {
    /// Any two of the same kind: only the newest read is worth making.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (ViewsReq::Accounts { .. }, ViewsReq::Accounts { .. })
                | (ViewsReq::History { .. }, ViewsReq::History { .. })
        )
    }
}

impl Coalesce for ActionsReq {
    /// Two collections merge; an action never does.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (ActionsReq::Collect { .. }, ActionsReq::Collect { .. })
        )
    }
}

/// One input to `update`. The data variants are large, as the engine's views are, and a
/// message is moved once, so they are not boxed.
#[allow(clippy::large_enum_variant)]
pub enum Msg {
    Key(crossterm::event::KeyEvent),
    Resize {
        width: u16,
        height: u16,
    },
    Tick,
    Accounts {
        r#gen: u64,
        result: Result<AccountsSnapshot, SourceError>,
    },
    History {
        r#gen: u64,
        result: Result<HistoryView, SourceError>,
    },
    Collected {
        r#gen: u64,
        result: Result<CollectSummary, SourceError>,
    },
    Prompt(Prompt),
    ActionDone {
        action: Action,
        outcome: ActionOutcome,
    },
    LaneDied {
        lane: &'static str,
    },
}

/// A question from the CLI path, and where its answer goes (spec §7.2).
pub struct Prompt {
    pub question: String,
    pub kind: PromptKind,
    pub reply: Sender<Answer>,
}

pub enum PromptKind {
    Confirm { default_yes: bool },
    Choose { options: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Confirm(bool),
    Choose(Option<usize>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    LoadAccounts {
        r#gen: u64,
    },
    LoadHistory {
        r#gen: u64,
        account: AccountId,
        since_s: i64,
    },
    CollectDue {
        r#gen: u64,
        accounts: Vec<AccountId>,
    },
    Act(Action),
    /// Run `action` with the terminal handed back (spec §7.3).
    Suspend(Action),
    /// Begin the shutdown sequence (spec §10.2) with this exit code.
    Quit {
        code: i32,
    },
    /// Ctrl-Z (spec §10.1).
    Stop,
}
```

`crates/tagteam-tui/src/state.rs` (the whole file):

```rust
//! Everything on screen (spec §4.2). `update` changes it, `view` draws it.

use tagteam_core::ProviderId;

use crate::actions::Overlay;
use crate::host::Action;
use crate::msg::Cmd;
use crate::screens::accounts::AccountsState;
use crate::screens::history::HistoryState;

/// Timer mode re-reads and collects this often (spec §6.2).
pub const REFRESH_MS: i64 = 30_000;
/// A toast replaces the footer this long (spec §5.1).
pub const TOAST_MS: i64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Accounts,
    History,
} // part B inserts Tokens between them

impl Screen {
    /// `Tab`'s order.
    pub(crate) fn next(self) -> Screen {
        match self {
            Screen::Accounts => Screen::History,
            Screen::History => Screen::Accounts,
        }
    }

    /// `Shift-Tab`'s order.
    pub(crate) fn prev(self) -> Screen {
        match self {
            Screen::Accounts => Screen::History,
            Screen::History => Screen::Accounts,
        }
    }
}

pub struct State {
    pub screen: Screen,
    pub size: (u16, u16),
    pub color: bool,
    pub provider: Option<ProviderId>,
    pub accounts: AccountsState,
    pub history: HistoryState,
    pub overlay: Option<Overlay>,
    pub toast: Option<Toast>,
    /// A failed read's error, in the CLI's words (spec §10.4).
    pub banner: Option<String>,
    /// The action running now, in the TUI or suspended (spec §7.2).
    pub running: Option<Action>,
    pub gens: Gens,
    pub refresh: Refresh,
    /// Set once the shutdown has begun: the exit code it will end with.
    pub quitting: Option<i32>,
    /// Whether the next loop iteration must redraw.
    pub dirty: bool,
    /// The second the screen was last drawn for: its countdowns change at most once a second
    /// (spec §5.2, "Countdowns tick").
    pub(crate) shown_s: i64,
}

pub struct Toast {
    pub text: String,
    pub until_ms: i64,
}

#[derive(Default)]
pub struct Gens {
    pub accounts: u64,
    pub history: u64,
    pub collect: u64,
}

pub struct Refresh {
    pub next_ms: i64,
}

impl State {
    pub fn new(screen: Screen, color: bool, provider: Option<ProviderId>, now_ms: i64) -> State {
        State {
            screen,
            size: (0, 0),
            color,
            provider,
            accounts: AccountsState::default(),
            history: HistoryState::default(),
            overlay: None,
            toast: None,
            banner: None,
            running: None,
            gens: Gens::default(),
            refresh: Refresh {
                next_ms: now_ms + REFRESH_MS,
            },
            quitting: None,
            dirty: true,
            shown_s: now_ms.div_euclid(1000),
        }
    }

    /// The first `Cmd`s: load `accounts` (gen 1). History needs an account, which only the
    /// first `accounts` result names, so a TUI started on History loads it once that arrives.
    pub fn start(&mut self) -> Vec<Cmd> {
        vec![self.load_accounts()]
    }

    /// A read of `accounts` under the next generation: any read still in flight is stale.
    pub(crate) fn load_accounts(&mut self) -> Cmd {
        self.gens.accounts += 1;
        Cmd::LoadAccounts {
            r#gen: self.gens.accounts,
        }
    }

    /// Shows `text` in place of the footer for `TOAST_MS`.
    pub(crate) fn toast(&mut self, text: impl Into<String>, now_ms: i64) {
        self.toast = Some(Toast {
            text: text.into(),
            until_ms: now_ms + TOAST_MS,
        });
        self.dirty = true;
    }
}
```

In `crates/tagteam-tui/src/update.rs`, above `#[cfg(test)]`, after the module line:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::actions::{self, Overlay};
use crate::msg::{Cmd, Msg};
use crate::screens;
use crate::state::{Screen, State};

pub fn update(state: &mut State, msg: Msg, now_ms: i64) -> Vec<Cmd> {
    match msg {
        Msg::Key(key) => {
            if key.kind == KeyEventKind::Release {
                return vec![];
            }
            state.dirty = true;
            on_key(state, key, now_ms)
        }
        Msg::Resize { width, height } => {
            state.size = (width, height);
            state.dirty = true;
            vec![]
        }
        Msg::Tick => on_tick(state, now_ms),
        Msg::Accounts { .. } | Msg::History { .. } | Msg::Collected { .. } => vec![],
        Msg::Prompt(prompt) => {
            state.dirty = true;
            actions::on_prompt(state, prompt);
            vec![]
        }
        Msg::ActionDone { action, outcome } => {
            state.dirty = true;
            actions::on_done(state, action, outcome, now_ms)
        }
        Msg::LaneDied { lane } => {
            state.dirty = true;
            state.banner = Some(format!("the {lane} lane stopped unexpectedly"));
            if state.quitting.is_some() {
                return vec![];
            }
            quit(state, 1)
        }
    }
}

/// Starts the shutdown (spec §10.2) with `code`. An open dialog is dropped with it, so the
/// action waiting on its answer is declined.
pub(crate) fn quit(state: &mut State, code: i32) -> Vec<Cmd> {
    state.quitting = Some(code);
    state.overlay = None;
    state.dirty = true;
    vec![Cmd::Quit { code }]
}

fn on_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    if state.quitting.is_some() {
        return vec![];
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => quit(state, 0),
            KeyCode::Char('z') => vec![Cmd::Stop],
            _ => vec![],
        };
    }
    // A dialog or panel takes every other key: a text dialog must be able to type `q`.
    if state.overlay.is_some() {
        return actions::handle_overlay_key(state, key, now_ms);
    }
    match key.code {
        KeyCode::Char('q') => quit(state, 0),
        KeyCode::Char('1') => show(state, Screen::Accounts, now_ms),
        KeyCode::Char('3') => show(state, Screen::History, now_ms),
        KeyCode::Tab => show(state, state.screen.next(), now_ms),
        KeyCode::BackTab => show(state, state.screen.prev(), now_ms),
        KeyCode::Char('?') => {
            state.overlay = Some(Overlay::Help);
            vec![]
        }
        _ => match actions::handle_action_key(state, key, now_ms) {
            Some(cmds) => cmds,
            None => match state.screen {
                Screen::Accounts => screens::accounts::handle_key(state, key, now_ms),
                Screen::History => screens::history::handle_key(state, key, now_ms),
            },
        },
    }
}

/// Switches to `screen`; History reads its account's history as it opens.
pub(crate) fn show(state: &mut State, screen: Screen, now_ms: i64) -> Vec<Cmd> {
    if state.screen == screen {
        return vec![];
    }
    state.screen = screen;
    match screen {
        Screen::Accounts => vec![],
        Screen::History => screens::history::on_enter(state, now_ms),
    }
}

fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.toast.as_ref().is_some_and(|t| t.until_ms <= now_ms) {
        state.toast = None;
        state.dirty = true;
    }
    // Countdowns change at most once a second; the spinner turns while anything runs.
    let now_s = now_ms.div_euclid(1000);
    if now_s != state.shown_s || state.running.is_some() || state.quitting.is_some() {
        state.shown_s = now_s;
        state.dirty = true;
    }
    vec![]
}
```

In `crates/tagteam-tui/src/view.rs`, above `#[cfg(test)]`, after the module line:

```rust
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tagteam_engine::words::Severity;
use unicode_width::UnicodeWidthStr;

use crate::actions;
use crate::screens;
use crate::state::{Screen, State};

/// Below this the body is replaced by `TOO_SMALL` (spec §5.1).
pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;
pub const TOO_SMALL: &str = "terminal too small (80×24 needed)";
/// The header's mode in timer mode (spec §5.1); part C adds live mode.
pub const MODE_TIMER: &str = "○ refresh 30s";
/// The spinner's frames, one per tick while an action runs.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The footer's keys for each screen, in the order they are dropped from the end when the
/// line is too narrow (spec §5.6). Help lists every key.
const ACCOUNTS_KEYS: [(&str, &str); 10] = [
    ("s", "switch"),
    ("b", "best"),
    ("n", "next"),
    ("e", "enable/disable"),
    ("a", "alias"),
    ("m", "move"),
    ("x", "remove"),
    ("r", "refresh"),
    ("R", "run"),
    ("Enter", "history"),
];
const HISTORY_KEYS: [(&str, &str); 4] = [
    ("j/k", "account"),
    ("[ ]", "range"),
    ("r", "refresh"),
    ("1", "accounts"),
];

/// Where each part of the screen goes.
pub(crate) struct Areas {
    pub(crate) header: Rect,
    pub(crate) banner: Option<Rect>,
    pub(crate) body: Rect,
    pub(crate) footer: Rect,
}

/// The header on the first row, the banner (when there is one) below it, the footer on the
/// last rows, and the body between. The banner, and a toast in the footer, take as many rows as
/// their words need at the width, `WRAP_ROWS` at most, from the body, which keeps a row at
/// least. Part A has no activity line: timer mode hides it (§5.1).
pub(crate) fn areas(state: &State, area: Rect) -> Areas {
    let width = usize::from(area.width);
    let spare = area.height.saturating_sub(3);
    let rows = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
    let banner = banner(state).map_or(0, |(text, _)| rows(banner_rows(&text, width).len()));
    let banner = banner.min(spare);
    let footer = toast_rows(state, width).map_or(1, |t| rows(t.len()));
    let footer = footer.min(spare - banner + 1).min(area.height);
    let top = area.y + 1.min(area.height) + banner;
    Areas {
        header: Rect::new(area.x, area.y, area.width, 1.min(area.height)),
        banner: (banner > 0).then(|| Rect::new(area.x, area.y + 1, area.width, banner)),
        body: Rect::new(
            area.x,
            top,
            area.width,
            area.height.saturating_sub(top - area.y + footer),
        ),
        footer: Rect::new(
            area.x,
            area.y + area.height.saturating_sub(footer),
            area.width,
            footer,
        ),
    }
}

/// The banner's text, and whether it is an error: a failed read (spec §10.4) first, else what
/// the accounts view has to say (a run shell, an unmanaged live login).
pub(crate) fn banner(state: &State) -> Option<(String, bool)> {
    match &state.banner {
        Some(error) => Some((error.clone(), true)),
        None => screens::accounts::banner(&state.accounts).map(|text| (text, false)),
    }
}

/// The banner and a toast take at most this many rows (spec §5.1).
const WRAP_ROWS: usize = 3;

/// `text` in rows of at most `width` columns, broken at word boundaries (`wrap`), at most `max`
/// of them. Only text beyond the last row is cut, which then ends in `…`.
pub(crate) fn wrap_rows(text: &str, width: usize, max: usize) -> Vec<String> {
    let mut rows = wrap(text, width);
    if rows.len() > max.max(1) {
        let last = max.max(1) - 1;
        let rest = rows[last..].join(" ");
        rows.truncate(last);
        rows.push(fit(&rest, width));
    }
    rows
}

/// The banner's rows: one column in, as the header's text is.
fn banner_rows(text: &str, width: usize) -> Vec<String> {
    wrap_rows(text, width.saturating_sub(1), WRAP_ROWS)
        .into_iter()
        .map(|row| format!(" {row}"))
        .collect()
}

/// The toast's rows, when the footer shows it: not once the shutdown has begun.
fn toast_rows(state: &State, width: usize) -> Option<Vec<String>> {
    let toast = state.toast.as_ref().filter(|_| state.quitting.is_none())?;
    Some(wrap_rows(&toast.text, width, WRAP_ROWS))
}

pub fn view(state: &State, frame: &mut Frame, now_ms: i64) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(Paragraph::new(TOO_SMALL), area);
        return;
    }
    let areas = areas(state, area);
    frame.render_widget(Paragraph::new(header(state, area.width)), areas.header);
    if let (Some(rect), Some((text, error))) = (areas.banner, banner(state)) {
        // An error takes the critical severity's style, anything else the warning's.
        let severity = if error {
            Severity::Critical
        } else {
            Severity::Warning
        };
        let style = severity_style(state.color, severity);
        let rows: Vec<Line<'static>> = banner_rows(&text, usize::from(area.width))
            .into_iter()
            .map(|row| Line::from(Span::styled(row, style)))
            .collect();
        frame.render_widget(Paragraph::new(rows), rect);
    }
    match state.screen {
        Screen::Accounts => screens::accounts::render(state, frame, areas.body, now_ms),
        Screen::History => screens::history::render(state, frame, areas.body, now_ms),
    }
    frame.render_widget(
        Paragraph::new(footer(state, area.width, now_ms)),
        areas.footer,
    );
    actions::render_overlay(state, frame, now_ms);
}

/// ` tagteam  [1 Accounts]  3 History` on the left, the mode and `? help` on the right.
fn header(state: &State, width: u16) -> Line<'static> {
    let mut spans = vec![Span::styled(" tagteam", bold())];
    for (screen, label) in [
        (Screen::Accounts, "1 Accounts"),
        (Screen::History, "3 History"),
    ] {
        spans.push(Span::raw("  "));
        if screen == state.screen {
            spans.push(Span::styled(format!("[{label}]"), bold()));
        } else {
            spans.push(Span::raw(label));
        }
    }
    let right = format!("{MODE_TIMER}   ? help ");
    let used: usize = spans.iter().map(|s| width_of(&s.content)).sum();
    let gap = usize::from(width).saturating_sub(used + width_of(&right));
    spans.push(Span::raw(" ".repeat(gap.max(1))));
    spans.push(Span::raw(right));
    Line::from(spans)
}

/// The footer: `finishing <action>…` once the shutdown has begun, else the toast (on as many
/// rows as it needs, `WRAP_ROWS` at most), else the running action's spinner, else the screen's
/// keys (spec §5.1, §10.2 step 4).
fn footer(state: &State, width: u16, now_ms: i64) -> Vec<Line<'static>> {
    if state.quitting.is_some() {
        let text = match &state.running {
            Some(action) => format!("finishing {}…", action.name()),
            None => "finishing…".to_owned(),
        };
        return vec![Line::from(Span::styled(text, bold()))];
    }
    if let Some(rows) = toast_rows(state, usize::from(width)) {
        return rows
            .into_iter()
            .map(|row| Line::from(Span::styled(row, bold())))
            .collect();
    }
    if let Some(action) = &state.running {
        let frame = SPINNER[(now_ms.div_euclid(250)).rem_euclid(10) as usize];
        return vec![Line::from(format!("{frame} {}…", action.name()))];
    }
    let keys: &[(&str, &str)] = match state.screen {
        Screen::Accounts => &ACCOUNTS_KEYS,
        Screen::History => &HISTORY_KEYS,
    };
    let mut text = String::new();
    for (key, what) in keys {
        let item = format!("{}{key} {what}", if text.is_empty() { "" } else { "  " });
        if width_of(&text) + width_of(&item) > usize::from(width) {
            break;
        }
        text.push_str(&item);
    }
    vec![Line::from(text)]
}

/// The columns `s` takes on a terminal: a CJK character or an emoji takes two.
pub(crate) fn width_of(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// `s` cut to at most `w` columns by display width, never by bytes, ending in `…` when cut.
pub(crate) fn fit(s: &str, w: usize) -> String {
    if width_of(s) <= w {
        return s.to_owned();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    for c in s.chars() {
        out.push(c);
        if width_of(&out) > w - 1 {
            out.pop();
            break;
        }
    }
    out.push('…');
    out
}

/// `s` cut to `w` columns and padded to exactly `w`.
pub(crate) fn pad(s: &str, w: usize) -> String {
    let s = fit(s, w);
    let fill = w.saturating_sub(width_of(&s));
    format!("{s}{}", " ".repeat(fill))
}

/// `text` broken into lines of at most `w` columns, at spaces where it can be.
pub(crate) fn wrap(text: &str, w: usize) -> Vec<String> {
    let w = w.max(1);
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let first = lines.len();
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if width_of(&candidate) <= w {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            // A word wider than a line is cut where the width runs out, at least one character
            // a line.
            let mut rest = word;
            while width_of(rest) > w {
                let mut cut = 0;
                for (i, c) in rest.char_indices() {
                    if width_of(&rest[..i + c.len_utf8()]) > w {
                        break;
                    }
                    cut = i + c.len_utf8();
                }
                if cut == 0 {
                    cut = rest.chars().next().map_or(rest.len(), char::len_utf8);
                }
                lines.push(rest[..cut].to_owned());
                rest = &rest[cut..];
            }
            line = rest.to_owned();
        }
        if !line.is_empty() || lines.len() == first {
            lines.push(line);
        }
    }
    lines
}

/// A box of `w` by `h` in the middle of `area`, no larger than it.
pub(crate) fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let (w, h) = (w.min(area.width), h.min(area.height));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

pub(crate) fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

/// The selection: reverse video, which reads on any terminal, coloured or not (spec §5.1).
pub(crate) fn selected() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

/// Core §13.5's severities: yellow and red with colour, bold without (spec §5.1).
pub(crate) fn severity_style(color: bool, severity: Severity) -> Style {
    match (severity, color) {
        (Severity::Normal, _) => Style::new(),
        (Severity::Warning, true) => Style::new().fg(Color::Yellow),
        (Severity::Critical, true) => Style::new().fg(Color::Red),
        (Severity::Warning | Severity::Critical, false) => bold(),
    }
}
```

`crates/tagteam-tui/src/screens/mod.rs` (the whole file):

```rust
//! The screens (spec §5): each with its state, its keys and its renderer.

pub mod accounts;
pub mod history;
```

Create `crates/tagteam-tui/src/screens/accounts.rs`:

```rust
//! The Accounts screen (spec §5.2). Task 6 draws it; this holds the view it will draw.

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::msg::Cmd;
use crate::source::AccountsSnapshot;
use crate::state::State;

#[derive(Default)]
pub struct AccountsState {
    snapshot: Option<AccountsSnapshot>,
}

impl AccountsState {
    pub fn snapshot(&self) -> Option<&AccountsSnapshot> {
        self.snapshot.as_ref()
    }

    pub fn apply(&mut self, snap: AccountsSnapshot) {
        self.snapshot = Some(snap);
    }
}

/// What the accounts view has to say above the body; Task 6 raises its banners.
pub fn banner(_state: &AccountsState) -> Option<String> {
    None
}

pub fn handle_key(_state: &mut State, _key: KeyEvent, _now_ms: i64) -> Vec<Cmd> {
    vec![]
}

pub fn render(_state: &State, _frame: &mut Frame, _area: Rect, _now_ms: i64) {}
```

Create `crates/tagteam-tui/src/screens/history.rs`:

```rust
//! The History screen (spec §5.4). Task 7 draws it; this holds the account it shows.

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use tagteam_core::AccountId;

use crate::msg::Cmd;
use crate::state::State;

#[derive(Default)]
pub struct HistoryState {
    account: Option<AccountId>,
}

impl HistoryState {
    pub fn account(&self) -> Option<&AccountId> {
        self.account.as_ref()
    }

    /// Shows `account`'s history.
    pub fn select(&mut self, account: AccountId) {
        self.account = Some(account);
    }
}

/// What History reads as it opens; Task 7 reads the account's history.
pub(crate) fn on_enter(_state: &mut State, _now_ms: i64) -> Vec<Cmd> {
    vec![]
}

pub fn handle_key(_state: &mut State, _key: KeyEvent, _now_ms: i64) -> Vec<Cmd> {
    vec![]
}

pub fn render(_state: &State, _frame: &mut Frame, _area: Rect, _now_ms: i64) {}
```

In `crates/tagteam-tui/src/actions.rs`, above `#[cfg(test)]`, after the module line:

```rust
use std::sync::mpsc::Sender;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Paragraph};

use crate::host::{AccountRef, Action, ActionOutcome};
use crate::msg::{Answer, Cmd, Prompt, PromptKind};
use crate::state::{Screen, State};
use crate::view::{bold, centered, pad, selected, wrap};

#[derive(Debug)]
pub enum Overlay {
    Help,
    Confirm {
        question: String,
        default_yes: bool,
        choice: bool,
        then: ConfirmThen,
    },
    Choose {
        question: String,
        options: Vec<String>,
        selected: usize,
        reply: Sender<Answer>,
    },
    Text {
        prompt: String,
        input: String,
        then: TextThen,
    },
    Result {
        title: String,
        text: String,
        scroll: u16,
    },
    Doctor {
        text: String,
        scroll: u16,
        online: bool,
    },
}

/// What a confirm dialog's answer goes to: a waiting CLI prompt, or the TUI's own question
/// before an action (`Remove`'s).
#[derive(Debug)]
pub enum ConfirmThen {
    Reply(Sender<Answer>),
    Run(Action),
}

/// What a text dialog's input is for.
#[derive(Debug)]
pub enum TextThen {
    Alias(AccountRef),
    Move(AccountRef),
}

/// The help panel's keys (spec §5.6): everywhere, then the current screen's.
const HELP_GLOBAL: [(&str, &str); 7] = [
    ("1 3  Tab  Shift-Tab", "screens"),
    ("j k  ↑ ↓", "move"),
    ("A", "add the current login"),
    ("D", "doctor"),
    ("?", "help"),
    ("q  Ctrl-C", "quit"),
    ("Ctrl-Z", "suspend"),
];
const HELP_ACCOUNTS: [(&str, &str); 10] = [
    ("s", "switch to the selected account"),
    ("b", "switch to the best account"),
    ("n", "switch to the next available account"),
    ("e", "enable or disable"),
    ("a", "set or clear the alias"),
    ("m", "move to another position"),
    ("x", "remove"),
    ("R", "run a session on the account"),
    ("r", "refresh"),
    ("Enter", "history of the account"),
];
const HELP_HISTORY: [(&str, &str); 2] = [("[ ]", "range: 1, 7, 14 or 30 days"), ("r", "refresh")];

/// The action keys; Task 9 adds them. `None`: not an action key.
pub fn handle_action_key(_state: &mut State, _key: KeyEvent, _now_ms: i64) -> Option<Vec<Cmd>> {
    None
}

/// A key while an overlay is open. `Esc` declines a dialog and closes a panel (spec §5.5).
pub fn handle_overlay_key(state: &mut State, key: KeyEvent, _now_ms: i64) -> Vec<Cmd> {
    let Some(overlay) = state.overlay.take() else {
        return vec![];
    };
    let (kept, cmds) = match overlay {
        Overlay::Help => match key.code {
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Enter => (None, vec![]),
            _ => (Some(Overlay::Help), vec![]),
        },
        Overlay::Confirm {
            question,
            default_yes,
            choice,
            then,
        } => {
            let answer = match key.code {
                KeyCode::Char('y' | 'Y') => Some(true),
                KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(false),
                KeyCode::Enter => Some(choice),
                _ => None,
            };
            match answer {
                Some(yes) => (None, confirmed(then, yes)),
                None => {
                    let choice = match key.code {
                        KeyCode::Left
                        | KeyCode::Right
                        | KeyCode::Tab
                        | KeyCode::BackTab
                        | KeyCode::Char('h' | 'l') => !choice,
                        _ => choice,
                    };
                    let kept = Overlay::Confirm {
                        question,
                        default_yes,
                        choice,
                        then,
                    };
                    (Some(kept), vec![])
                }
            }
        }
        Overlay::Choose { reply, .. } if key.code == KeyCode::Esc => {
            let _ = reply.send(Answer::Choose(None));
            (None, vec![])
        }
        Overlay::Text { .. } | Overlay::Result { .. } | Overlay::Doctor { .. }
            if key.code == KeyCode::Esc =>
        {
            (None, vec![])
        }
        other => (Some(other), vec![]),
    };
    state.overlay = kept;
    cmds
}

/// A confirm dialog's answer: sent to the prompt waiting for it, or a yes runs the action.
fn confirmed(then: ConfirmThen, yes: bool) -> Vec<Cmd> {
    match then {
        ConfirmThen::Reply(reply) => {
            let _ = reply.send(Answer::Confirm(yes));
            vec![]
        }
        ConfirmThen::Run(action) if yes => vec![Cmd::Act(action)],
        ConfirmThen::Run(_) => vec![],
    }
}

/// A CLI prompt from the actions lane. Until Task 9's dialogs, every prompt is declined.
pub fn on_prompt(_state: &mut State, prompt: Prompt) {
    let answer = match prompt.kind {
        PromptKind::Confirm { .. } => Answer::Confirm(false),
        PromptKind::Choose { .. } => Answer::Choose(None),
    };
    let _ = prompt.reply.send(answer);
}

/// An action's outcome: the action is no longer running, and the first line it printed is a
/// toast. Task 9 adds the panels and the re-read.
pub fn on_done(state: &mut State, action: Action, outcome: ActionOutcome, now_ms: i64) -> Vec<Cmd> {
    state.running = None;
    let text = match outcome {
        ActionOutcome::Done { out, err, .. } => out
            .lines()
            .chain(err.lines())
            .find(|l| !l.trim().is_empty())
            .map_or_else(|| format!("{} done", action.name()), str::to_owned),
        ActionOutcome::NeedsTerminal => format!("{} needs the terminal", action.name()),
    };
    state.toast(text, now_ms);
    vec![]
}

pub fn render_overlay(state: &State, frame: &mut Frame, _now_ms: i64) {
    let Some(overlay) = &state.overlay else {
        return;
    };
    let area = frame.area();
    match overlay {
        Overlay::Help => {
            let keys: &[(&str, &str)] = match state.screen {
                Screen::Accounts => &HELP_ACCOUNTS,
                Screen::History => &HELP_HISTORY,
            };
            let mut lines = vec![Line::from(Span::styled("Everywhere", bold()))];
            lines.extend(HELP_GLOBAL.iter().map(|(k, what)| help_line(k, what)));
            let title = match state.screen {
                Screen::Accounts => "Accounts",
                Screen::History => "History",
            };
            lines.push(Line::from(Span::styled(title, bold())));
            lines.extend(keys.iter().map(|(k, what)| help_line(k, what)));
            boxed(frame, area, " Help ", lines, 64);
        }
        Overlay::Confirm {
            question, choice, ..
        } => {
            let w = 64u16.min(area.width.saturating_sub(4));
            let mut lines: Vec<Line<'static>> = wrap(question, usize::from(w) - 4)
                .into_iter()
                .map(Line::from)
                .collect();
            lines.push(Line::default());
            let button = |label: &str, on: bool| {
                let text = format!("[ {label} ]");
                if on {
                    Span::styled(text, selected())
                } else {
                    Span::raw(text)
                }
            };
            lines.push(Line::from(vec![
                button("Yes", *choice),
                Span::raw("   "),
                button("No", !*choice),
            ]));
            lines.push(Line::from("y yes · n no · Enter choose · Esc no"));
            boxed(frame, area, " Confirm ", lines, w);
        }
        Overlay::Choose { .. }
        | Overlay::Text { .. }
        | Overlay::Result { .. }
        | Overlay::Doctor { .. } => {}
    }
}

fn help_line(key: &str, what: &str) -> Line<'static> {
    Line::from(format!("  {}  {what}", pad(key, 20)))
}

/// `lines` in a bordered box `w` wide, as tall as they need, in the middle of `area`.
fn boxed(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>, w: u16) {
    let h = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2);
    let rect = centered(area, w.min(area.width), h);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(title.to_owned())
                .padding(Padding::horizontal(1)),
        ),
        rect,
    );
}
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-tui --lib`
Expected: PASS, 38 tests (Task 3's 9, Task 4's 10 and this task's 19).

Run: `cargo test -p tagteam-tui`
Expected: PASS.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings. `Msg`
carries `#[allow(clippy::large_enum_variant)]`, as `views::StatusView` does: its data variants
hold the engine's views, and a message is moved once.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-tui/src/lib.rs crates/tagteam-tui/src/msg.rs \
  crates/tagteam-tui/src/state.rs crates/tagteam-tui/src/update.rs crates/tagteam-tui/src/view.rs \
  crates/tagteam-tui/src/actions.rs crates/tagteam-tui/src/screens crates/tagteam-tui/src/testutil.rs
git commit -m "Draw the TUI's chrome and route its keys"
```

---

### Task 6: Accounts screen

The Accounts screen (spec §5.2) is `list`'s table in `list`'s words, with a gauge per window when
the terminal is wide enough, and the selected account's detail below it. Every word, column and
note comes from `tagteam_engine::words` (C1), so the TUI says what `list` says: the columns
(`window_columns`), each window's cell (`9%  2h40m  ▲ pace`, the spend amounts), `AGE`, and the
notes after the cells (`row_notes`: the status in words, the kind, `disabled`, `this`). The
layout follows `list`'s `table` (`crates/tagteam/src/render.rs:428-488` today): the active `*`,
the position, a session column only when some row is session-owned, then the account, the cells
and the notes.

**Readings of the spec this task commits to:**
- **Width (Review Focus 2).** Each provider's table is laid out for the width it has:
  - **Gauges** (six cells, before each percentage window) are added only when every row still
    fits, notes included. An 80-column terminal with `list`'s usual columns shows no gauges; a
    120-column one does. The notes say more than a gauge does.
  - **The ACCOUNT column** is cut, by display width (`unicode-width`), when even `list`'s line
    does not fit, down to 12 columns, ending in `…`. A cut never splits a character, and a CJK
    character or emoji counts two columns.
  - **Anything still too wide** is clipped at the area's edge by ratatui, never drawn past it.
- **The detail pane** is three rows (the rule, the reading's age and the trouble line) taller
  than the lines of the most windows any account shown has at the body's width, at least seven
  rows, and at most half the body: the table keeps the rest (R20). It shows:
  - a rule naming the account: position and name, `active` or `not active`, its kind, `in a
    session`, `▶ this`, `disabled`. When the rule is too narrow the name is cut (`…`), never
    the facts after it;
  - every window (spec §5.2: each relevant window), each with its value, `resets in
    <countdown>` (or `reset`), `▲ ahead of pace (expected N%)`, and `history`'s rate and
    projection (C1's `projection_text`) when a rate is known. When the line does not fit the
    width, the parts after the value go on to continuation lines that start under the value
    (`view::flow`): a part moves whole, the projection breaks at its ` · ` only when it is wider
    than a whole continuation line, and only a piece wider still is cut. The label column is a
    third of the pane at most (R20); a wider label wraps onto rows of its own above its value,
    two at most, the second ending in `…` when there is more. The pane is never shorter than
    its tallest window (7 rows at most) plus its rule, age and trouble rows, taking rows from
    the table down to 3 for it, so every window fits and whole-window scrolling reaches all.
    Windows that do not fit the pane scroll: `J`/`K` move them a whole window at a time and stop
    at either end (the last window showing, or the first), starting from what shows after a
    resize or a new read (R22), no window is ever shown in part, the rule then ends in
    `J K: windows <first>–<last> of <n>`, and another selected account starts at its first
    window;
  - `fetched <age> ago` (or `never fetched`), `next poll in <countdown>` while the next poll is
    ahead (Task 1's `UsageView.next_poll_at`, R7; the frame's own clock hides it once it has
    passed, before the next read), and `login expires in <span>` (or `login expired`), as spec
    §5.2's mockup orders them;
  - `usage: <words>` when the status is not `ok`. C1's `usage_words` carries `usageError` and
    the retry time.
- **Selection (Review Focus 4 and 5).** The selection is an `AccountId`. `apply` keeps it on the
  same account across a refresh; when that account is gone it moves to the row now at its old
  place, or to the last row, and with no rows there is none. `j`/`k`/arrows move one row and stop
  at either end; `PageUp`/`PageDown` move a page, `g`/`Home` and `G`/`End` to either end. The
  table scrolls as little as it can to keep the selected row in view; `render` applies the same
  rule, so a resize never hides it.
- **Several providers.** One table per provider with accounts, each under its provider's ID, as
  `list` prints one per provider. With one provider its column header is pinned above the rows.
- **Empty states (Review Focus 3).** No snapshot yet: ` loading…` (spec §6.1). No accounts:
  ` No accounts yet. Log in with claude, then press A to add the login.` (spec §10.4).
- **Banners.** A run shell: `` inside a `tagteam run` session: account changes are refused here ``.
  Otherwise an unmanaged live login: `Live: <email> (not managed by tagteam); press A to add it`
  (`status`'s words). An error banner (Task 8) wins over both. A long one wraps onto more rows,
  as Task 5's layout does it (R21), so `press A to add it` shows at 80 columns.
- **`Enter`** opens History on the selected account (`HistoryState::select`, then
  `update::show`). With no account it does nothing.

**Depends on:** 1, 5

**Files:**
- Modify: `crates/tagteam-tui/src/screens/accounts.rs` (Task 5's stub becomes the screen)
- Modify: `crates/tagteam-tui/src/view.rs` (adds `body_at`, and R20's `flow`,
  `scrolled_windows` and `furthest_scroll`, after `areas`; a test in its test module)
- Modify: `crates/tagteam-tui/src/actions.rs` (Task 5's `HELP_ACCOUNTS` lists `J K`)
- Test: unit tests in `screens/accounts.rs` and `view.rs`

**Interfaces:**
- Consumes:
  - Task 1 (C1): `words::{account_cell, countdown, duration, row_notes, severity, spend_text,
    usage_words, window_columns, projection_text, WindowColumn, MISSING, AHEAD, IN_SESSION,
    THIS}`; and `UsageView.next_poll_at` (R7).
  - Task 5: `State`, `Screen`, `update::show`, `view::{areas, bold, fit, pad, selected,
    severity_style, width_of, wrap_rows}`, `HistoryState::select`, `testutil`.
  - Task 3: `host::AccountRef`, `source::AccountsSnapshot`.
  - The engine's `views::{AccountView, HistoryWindow, UsageStatus}`.
- Produces:
  - C6: `AccountsState` with `snapshot`, `apply`, `selected`, `shown_ids`; `handle_key`;
    `render`.
  - Crate-private, for Task 9: `AccountsState::selected_view(&self) -> Option<&AccountView>`.
    For Task 5's `view`: `banner(&AccountsState) -> Option<String>`. For this screen's keys:
    `view::body_at(&State) -> Rect`. For this screen and Task 7 (R20):
    `view::flow(head: Vec<Span<'static>>, parts: Vec<String>, indent: usize, width: usize) ->
    Vec<Line<'static>>`, `view::scrolled_windows(heights: &[usize], from: usize, rows: usize)
    -> Range<usize>` and `view::furthest_scroll(heights: &[usize], rows: usize) -> usize`.
  - Public constants `NO_ACCOUNTS`, `LOADING`, `RUN_SHELL`.

**Spec:**
- spec §5.2: the table's rows, columns, markers and words are `list`'s (core §13.1); a gauge per
  window; the detail pane, with each relevant window (R20), the data age and the next poll (R7);
  `Enter` opens History.
- spec §5.1: colour by severity, reverse video for the selection; "Providers"; the banners.
- spec §3, "In a run shell": `▶ this`.
- spec §6.1 and §10.4: `loading…` and the empty state.
- core §13.1 and §13.2: `list`'s table, `usageStatus` in words with `retry <countdown>`.
- Review Focus 2 to 7.

- [ ] **Step 1: Write the failing tests**

Append to `crates/tagteam-tui/src/screens/accounts.rs` (Task 5's stub stays above it until Step
3):

```rust
#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use ratatui::style::{Color, Modifier};
    use tagteam_core::Pace;
    use tagteam_engine::views::{ProviderAccounts, UsageStatus, UsageView};

    use super::*;
    use crate::msg::Msg;
    use crate::testutil::*;
    use crate::update::update;

    /// §13.1's fixture: an active account ahead of pace, a stale spare, a quarantined alias.
    fn three() -> Vec<AccountView> {
        let mut live = account(1, "michael@example.com", read(120, 9.0, 77.0, true));
        live.active = true;
        let spare = account(2, "spare@example.com", read(840, 31.0, 12.0, false));
        let mut work = account(
            3,
            "w@corp.com",
            unread(UsageStatus::ReloginRequired, None, None),
        );
        work.row.alias = Some("work".into());
        work.row.quarantine_reason = Some("invalid_grant".into());
        vec![live, spare, work]
    }

    fn loaded(accounts: Vec<AccountView>) -> State {
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(accounts));
        state
    }

    #[test]
    fn the_table_says_what_list_says_at_80_columns() {
        let lines = draw(&loaded(three()), 80, 24);
        assert_eq!(
            &lines[1..5],
            [
                "    #  ACCOUNT              5H           7D                   SPEND  AGE",
                " *  1  michael@example.com    9%  2h40m   77%  3d09h  ▲ pace  —      2m",
                "    2  spare@example.com     31%  2h40m   12%  3d09h          —      14m",
                "    3  work (w@corp.com)    relogin required",
            ]
        );
    }

    #[test]
    fn a_wider_terminal_adds_a_gauge_per_window() {
        let lines = draw(&loaded(three()), 120, 40);
        assert_eq!(
            &lines[1..5],
            [
                "    #  ACCOUNT              5H                  7D                          SPEND  AGE",
                " *  1  michael@example.com  █░░░░░   9%  2h40m  █████░  77%  3d09h  ▲ pace  —      2m",
                "    2  spare@example.com    ██░░░░  31%  2h40m  █░░░░░  12%  3d09h          —      14m",
                "    3  work (w@corp.com)    relogin required",
            ]
        );
    }

    #[test]
    fn the_detail_pane_shows_the_selected_account() {
        let mut accounts = three();
        accounts[0].usage.windows.as_mut().unwrap()[1].1 = Pace {
            expected_pct: Some(62.0),
            ahead: Some(true),
            rate_per_hour: Some(1.2),
            method: Some(tagteam_core::ProjectionMethod::Regression),
            exhaustion_at: Some(NOW_S + 187_200),
            will_last_to_reset: Some(false),
        };
        accounts[0].row.login_expires_at = Some((NOW_S + 6 * 86_400 + 3 * HOUR) * 1000);
        let lines = draw(&loaded(accounts), 120, 40);
        let top = 40 - 1 - 7;
        assert_eq!(
            &lines[top..top + 4],
            [
                format!("─ 1 michael@example.com · active · oauth {}", "─".repeat(79)),
                " 5h    9%  resets in 2h40m".to_owned(),
                " 7d   77%  resets in 3d09h  ▲ ahead of pace (expected 62%)  +1.2 pts/h · runs out in 2d04h (regression)".to_owned(),
                " fetched 2m ago · login expires in 6d03h".to_owned(),
            ]
        );
    }

    #[test]
    fn a_row_without_a_reading_says_why_in_the_detail_pane() {
        let mut state = loaded(three());
        update(&mut state, ch('G'), NOW_MS);
        let lines = draw(&state, 80, 24);
        let top = 24 - 1 - 7;
        assert!(
            lines[top].starts_with("─ 3 work (w@corp.com) · not active · oauth ──"),
            "{}",
            lines[top]
        );
        assert_eq!(width_of(&lines[top]), 80);
        assert_eq!(lines[top + 1], " never fetched");
        assert_eq!(lines[top + 2], " usage: relogin required");
    }

    #[test]
    fn the_detail_pane_counts_down_to_the_next_poll_while_it_is_ahead() {
        // Spec §5.2: `fetched 2m ago · next poll in 1m · login expires in 6d`.
        let mut accounts = three();
        accounts[0].usage.next_poll_at = Some(NOW_S + 60);
        accounts[0].row.login_expires_at = Some((NOW_S + 6 * 86_400) * 1000);
        let state = loaded(accounts);
        let top = 24 - 1 - 7;
        assert_eq!(
            draw(&state, 80, 24)[top + 3],
            " fetched 2m ago · next poll in 1m · login expires in 6d00h"
        );
        // Once it has passed, a frame drawn before the next read says nothing of it.
        assert_eq!(
            draw_at(&state, 80, 24, NOW_MS + 60_000)[top + 3],
            " fetched 2m ago · login expires in 5d23h"
        );
    }

    /// `read`'s 5h and 7d, then a scoped window for each of `models`, each of those ahead of
    /// pace and running out.
    fn with_windows(models: &[&str]) -> UsageView {
        let mut usage = read(120, 9.0, 77.0, false);
        let windows = usage.windows.as_mut().unwrap();
        for (i, model) in models.iter().enumerate() {
            let key = format!("scoped:{model}");
            let pct = 40.0 + i as f64;
            windows.push((
                window(&key, model, WindowKind::Scoped, pct, Some(291_630)),
                Pace {
                    expected_pct: Some(20.0),
                    ahead: Some(true),
                    rate_per_hour: Some(1.5),
                    method: Some(tagteam_core::ProjectionMethod::Average),
                    exhaustion_at: Some(NOW_S + 86_400),
                    will_last_to_reset: Some(false),
                },
            ));
        }
        usage
    }

    /// The window labels the detail pane shows, between its rule and the reading's age; a
    /// window's continuation lines start further in.
    fn pane_windows(state: &State, w: u16, h: u16) -> Vec<String> {
        let lines = draw(state, w, h);
        let rule = lines.iter().position(|l| l.starts_with("─ ")).unwrap();
        lines[rule + 1..]
            .iter()
            .take_while(|l| !l.starts_with(" fetched"))
            .filter(|l| !l.starts_with("  "))
            .map(|l| l.split_whitespace().next().unwrap_or("").to_owned())
            .collect()
    }

    /// The widest lines a detail pane draws: a long scoped-model label, a spend value, and a
    /// reset, pace and projection on one window.
    fn widest() -> UsageView {
        let mut usage = read(120, 9.0, 77.0, false);
        let windows = usage.windows.as_mut().unwrap();
        windows[1].1 = Pace {
            expected_pct: Some(62.0),
            ahead: Some(true),
            rate_per_hour: Some(1.2),
            method: Some(tagteam_core::ProjectionMethod::Regression),
            exhaustion_at: Some(NOW_S + 187_200),
            will_last_to_reset: Some(false),
        };
        let mut spend = window("spend", "spend", WindowKind::Spend, 61.7, Some(1_209_630));
        spend.detail = Some(
            r#"{"used": 123.45, "limit": 200, "currency": "EUR"}"#
                .parse()
                .unwrap(),
        );
        windows.push((spend, Pace::default()));
        let sonnet = "Claude Sonnet 4.5";
        windows.push((
            window(
                &format!("scoped:{sonnet}"),
                sonnet,
                WindowKind::Scoped,
                41.0,
                Some(291_630),
            ),
            Pace {
                expected_pct: Some(20.0),
                ahead: Some(true),
                rate_per_hour: Some(1.5),
                method: Some(tagteam_core::ProjectionMethod::Average),
                exhaustion_at: Some(NOW_S + 86_400),
                will_last_to_reset: Some(false),
            },
        ));
        usage
    }

    #[test]
    fn a_window_too_wide_for_80_columns_wraps_its_parts_under_its_value() {
        // Spec §5.1: 80×24 is enough; spec §5.2: each window's reset, pace and projection.
        let state = loaded(vec![account(1, "a@x.co", widest())]);
        let lines = draw(&state, 80, 24);
        let rule = lines.iter().position(|l| l.starts_with("─ ")).unwrap();
        let under = " ".repeat(20);
        assert_eq!(
            lines[rule + 1..rule + 8],
            [
                format!(" {:<17}  {:>4}  resets in 2h40m", "5h", "9%"),
                format!(
                    " {:<17}  {:>4}  resets in 3d09h  ▲ ahead of pace (expected 62%)",
                    "7d", "77%"
                ),
                format!("{under}+1.2 pts/h · runs out in 2d04h (regression)"),
                format!(" {:<17}  €123.45 of €200  resets in 14d00h", "spend"),
                format!(
                    " {:<17}  {:>4}  resets in 3d09h  ▲ ahead of pace (expected 20%)",
                    "Claude Sonnet 4.5", "41%"
                ),
                format!("{under}+1.5 pts/h · runs out in 1d00h (average)"),
                " fetched 2m ago".to_owned(),
            ]
        );
        assert!(lines.iter().all(|l| width_of(l) <= 80 && !l.contains('…')));
        // A wide terminal has room for each window on one line.
        let wide = draw(&state, 120, 40);
        let rule = wide.iter().position(|l| l.starts_with("─ ")).unwrap();
        assert_eq!(
            wide[rule + 2],
            format!(
                " {:<17}  {:>4}  resets in 3d09h  ▲ ahead of pace (expected 62%)  +1.2 pts/h · runs out in 2d04h (regression)",
                "7d", "77%"
            )
        );
        assert!(
            wide[rule + 1..rule + 5]
                .iter()
                .all(|l| !l.starts_with("  ")),
            "no continuation lines: {wide:?}"
        );
    }

    /// An 80-column model name: any non-empty `display_name` is a label (the provider's §8.2).
    const OVERSIZED: &str =
        "Claude Opus 4.5 with extended thinking, for long-running agentic coding sessions";

    #[test]
    fn an_oversized_label_goes_above_its_value_and_the_column_stays_capped() {
        // The label column is a third of the pane at most, so every value and what follows it
        // keep room; the oversized label is not cut, but takes rows of its own.
        let mut usage = with_windows(&["Opus"]);
        usage.windows.as_mut().unwrap()[2].0.label = OVERSIZED.to_owned();
        let state = loaded(vec![account(1, "a@x.co", usage)]);
        let lines = draw(&state, 80, 40);
        let rule = lines.iter().position(|l| l.starts_with("─ ")).unwrap();
        let age = lines
            .iter()
            .position(|l| l.starts_with(" fetched"))
            .unwrap();
        let pane = &lines[rule + 1..age];
        assert!(
            pane.iter().all(|l| width_of(l) <= 80 && !l.contains('…')),
            "{pane:?}"
        );
        assert!(
            pane[0].starts_with(&format!(" {:<26}    9%", "5h")),
            "{pane:?}"
        );
        let label = pane
            .iter()
            .position(|l| l.starts_with(" Claude Opus"))
            .unwrap();
        assert_eq!(
            [pane[label].trim(), pane[label + 1].trim()].join(" "),
            OVERSIZED,
            "the whole label, on its own rows"
        );
        assert!(
            pane[label + 2].starts_with(&format!(" {:26}   40%  resets in 3d09h", "")),
            "{pane:?}"
        );
        let rest = pane[label + 2..].join(" ");
        assert!(rest.contains("▲ ahead of pace (expected 20%)"), "{rest}");
        assert!(
            rest.contains("+1.5 pts/h · runs out in 1d00h (average)"),
            "{rest}"
        );
    }

    /// A 1,000-character model name, as the provider accepts any non-empty `display_name`.
    fn thousand() -> String {
        "Claude ".repeat(143).trim_end().to_owned()
    }

    #[test]
    fn a_1000_character_label_leaves_its_window_reachable_at_80x24_under_a_full_banner() {
        // The smallest pane 80×24 gives: a 3-row banner, a 3-row toast, and a trouble line.
        let mut usage = with_windows(&["Opus"]);
        usage.status = UsageStatus::Unavailable;
        usage.error = Some("http-429".into());
        usage.windows.as_mut().unwrap()[2].0.label = thousand();
        let mut state = loaded(vec![account(1, "a@x.co", usage)]);
        state.banner = Some(["the store could not be read"; 12].join("; "));
        state.toast(["Switched to a@x.co (position 1)"; 8].join(" "), NOW_MS);
        for _ in 0..3 {
            update(&mut state, ch('J'), NOW_MS);
        }
        let lines = draw(&state, 80, 24);
        assert!(lines.iter().all(|l| width_of(l) <= 80), "{lines:?}");
        let label = lines
            .iter()
            .position(|l| l.starts_with(" Claude Claude"))
            .unwrap_or_else(|| panic!("{lines:?}"));
        assert!(
            lines[label + 1].ends_with('…'),
            "two rows at most: {lines:?}"
        );
        assert!(
            lines[label + 2].starts_with(&format!(" {:26}   40%  resets in 3d09h", "")),
            "{lines:?}"
        );
        let rest = lines[label + 2..].join(" ");
        assert!(rest.contains("▲ ahead of pace (expected 20%)"), "{lines:?}");
        assert!(
            rest.contains("+1.5 pts/h · runs out in 1d00h (average)"),
            "{lines:?}"
        );
        assert!(rest.contains(" usage: unavailable (http-429)"), "{lines:?}");
    }

    #[test]
    fn a_window_takes_seven_rows_at_most_whatever_its_words() {
        // Two label rows, the value's, and a row for each of reset, pace, rate and where it
        // leads: the bound the pane's height is held to (R20).
        let mut w = window(
            "spend",
            &thousand(),
            WindowKind::Spend,
            99.0,
            Some(400 * 86_400),
        );
        w.detail = Some(
            r#"{"used": 1e40, "limit": 1e41, "currency": "EUR"}"#
                .parse()
                .unwrap(),
        );
        let p = Pace {
            expected_pct: Some(100.0),
            ahead: Some(true),
            rate_per_hour: Some(12_345.6),
            method: Some(tagteam_core::ProjectionMethod::Regression),
            exhaustion_at: Some(NOW_S - 86_400 * 400),
            will_last_to_reset: Some(false),
        };
        for width in [80, 120] {
            let lines = window_lines(&w, &p, width / 3, width, NOW_S, false);
            assert!(lines.len() <= 7, "{width}: {} rows", lines.len());
            for line in &lines {
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                assert!(width_of(&text) <= width, "{width}: {text}");
            }
        }
        // The pane holds it whole at 80×24 under a 3-row banner, a 3-row toast and a trouble
        // line: it takes rows from the table for its tallest window.
        let mut usage = read(120, 9.0, 77.0, false);
        usage.status = UsageStatus::Unavailable;
        usage.error = Some("http-429".into());
        usage.windows.as_mut().unwrap().push((w, p));
        let mut state = loaded(vec![account(1, "a@x.co", usage)]);
        state.banner = Some(["the store could not be read"; 12].join("; "));
        state.toast(["Switched to a@x.co (position 1)"; 8].join(" "), NOW_MS);
        for _ in 0..2 {
            update(&mut state, ch('J'), NOW_MS);
        }
        let lines = draw(&state, 80, 24);
        let label = lines.iter().position(|l| l.starts_with(" Claude")).unwrap();
        let age = lines
            .iter()
            .position(|l| l.starts_with(" fetched"))
            .unwrap();
        let block = lines[label..age].join("\n");
        assert!(block.contains("of €100…"), "{lines:?}");
        assert!(
            block.contains("should have run out 400d00h ago (regression)"),
            "{lines:?}"
        );
        assert_eq!(lines[age + 1], " usage: unavailable (http-429)");
    }

    #[test]
    fn a_resize_clamps_the_detail_scroll_so_the_next_key_moves_at_once() {
        let models = [
            "M1", "M2", "M3", "M4", "M5", "M6", "M7", "M8", "M9", "M10", "M11",
        ];
        let mut state = loaded(vec![account(1, "a@x.co", with_windows(&models))]);
        let rule = |state: &State, h: u16| {
            draw(state, 80, h)
                .into_iter()
                .find(|l| l.starts_with("─ "))
                .unwrap()
        };
        for _ in 0..10 {
            update(&mut state, ch('J'), NOW_MS);
        }
        assert!(rule(&state, 24).ends_with(" J K: windows 10–13 of 13 ─"));
        // Taller: the last window shows from further up, and `K` moves from what shows.
        update(
            &mut state,
            Msg::Resize {
                width: 80,
                height: 30,
            },
            NOW_MS,
        );
        assert!(rule(&state, 30).ends_with(" J K: windows 8–13 of 13 ─"));
        update(&mut state, ch('K'), NOW_MS);
        assert!(
            rule(&state, 30).ends_with(" J K: windows 7–12 of 13 ─"),
            "{}",
            rule(&state, 30)
        );
    }

    #[test]
    fn the_rule_cuts_the_name_before_the_facts() {
        // Spec §5.2: whether the account is session-owned is the detail pane's to say.
        let long = "a".repeat(52) + "@example.com";
        let mut v = account(1, &long, read(60, 9.0, 12.0, false));
        v.in_session = true;
        let lines = draw(&loaded(vec![v]), 80, 24);
        let rule = lines.iter().find(|l| l.starts_with("─ ")).unwrap();
        assert!(rule.starts_with("─ 1 aaaa"), "{rule}");
        assert!(
            rule.contains("… · not active · oauth · in a session ─"),
            "{rule}"
        );
        assert_eq!(width_of(rule), 80);
    }

    #[test]
    fn the_detail_pane_grows_to_show_every_window() {
        // Spec §5.2: each relevant window, with its reset, pace and projection. 5h, 7d and
        // three scoped windows are five: one more than the pane's least height holds.
        let state = loaded(vec![account(
            1,
            "a@x.co",
            with_windows(&["Opus", "Sonnet", "Haiku"]),
        )]);
        for (w, h) in [(80, 24), (120, 40)] {
            assert_eq!(
                pane_windows(&state, w, h),
                ["5h", "7d", "Opus", "Sonnet", "Haiku"],
                "{w}×{h}"
            );
        }
        let lines = draw(&state, 120, 40);
        let haiku = lines.iter().find(|l| l.starts_with(" Haiku")).unwrap();
        assert_eq!(
            haiku.trim_end(),
            " Haiku    42%  resets in 3d09h  ▲ ahead of pace (expected 20%)  +1.5 pts/h · runs out in 1d00h (average)"
        );
        assert!(
            !lines.iter().any(|l| l.contains("J K")),
            "every window fits: nothing to scroll"
        );
    }

    #[test]
    fn windows_beyond_the_pane_scroll_into_view_with_shift_j_and_k() {
        // 13 windows, each scoped one two lines at 80 columns: more than half of 80×24's body
        // holds, so the pane scrolls them, by whole windows.
        let models = [
            "M1", "M2", "M3", "M4", "M5", "M6", "M7", "M8", "M9", "M10", "M11",
        ];
        let mut state = loaded(vec![
            account(1, "a@x.co", with_windows(&models)),
            account(2, "b@x.co", with_windows(&models)),
        ]);
        let first: Vec<&str> = vec!["5h", "7d", "M1", "M2", "M3"];
        assert_eq!(pane_windows(&state, 80, 24), first);
        // Whole windows only: M3's projection is the last line before the reading's age.
        let lines = draw(&state, 80, 24);
        let age = lines
            .iter()
            .position(|l| l.starts_with(" fetched"))
            .unwrap();
        assert!(lines[age - 2].starts_with(" M3 "), "{lines:?}");
        assert!(lines[age - 1].trim_start().starts_with("+1.5 pts/h"));
        let rule = |state: &State| {
            draw(state, 80, 24)
                .into_iter()
                .find(|l| l.starts_with("─ "))
                .unwrap()
        };
        assert!(
            rule(&state).ends_with(" J K: windows 1–5 of 13 ─"),
            "{}",
            rule(&state)
        );
        assert_eq!(width_of(&rule(&state)), 80);
        for _ in 0..10 {
            update(&mut state, ch('J'), NOW_MS);
        }
        assert_eq!(
            pane_windows(&state, 80, 24),
            ["M8", "M9", "M10", "M11"],
            "J stops at the last window"
        );
        assert!(rule(&state).ends_with(" J K: windows 10–13 of 13 ─"));
        update(&mut state, ch('K'), NOW_MS);
        assert_eq!(pane_windows(&state, 80, 24), ["M7", "M8", "M9", "M10"]);
        // Another account's detail starts at its first window.
        update(&mut state, ch('j'), NOW_MS);
        assert_eq!(pane_windows(&state, 80, 24), first);
        // A wider and taller terminal shows them all, a line each.
        assert_eq!(pane_windows(&state, 120, 40).len(), 13);
        // Help lists the keys.
        update(&mut state, ch('?'), NOW_MS);
        let help = draw(&state, 80, 24);
        assert!(
            help.iter()
                .any(|l| l.contains("J K") && l.contains("scroll the detail's windows")),
            "{help:?}"
        );
    }

    #[test]
    fn the_active_marker_session_column_and_this_follow_list() {
        let mut accounts = three();
        accounts[1].in_session = true;
        let mut snap = snapshot(accounts);
        snap.this = Some(id(2));
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        let lines = draw(&state, 80, 24);
        assert_eq!(
            &lines[1..4],
            [
                "    #    ACCOUNT              5H           7D                   SPEND  AGE",
                " *  1    michael@example.com    9%  2h40m   77%  3d09h  ▲ pace  —      2m",
                "    2 ▶  spare@example.com     31%  2h40m   12%  3d09h          —      14m  this",
            ]
        );
    }

    #[test]
    fn percentages_and_gauges_take_their_severity_colour_or_bold_without() {
        let hot = account(1, "a@x.co", read(60, 95.0, 77.0, false));
        for color in [true, false] {
            let mut state = loaded(vec![hot.clone()]);
            state.color = color;
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let (x, y) = locate(&buf, "95%").unwrap();
            let (gx, gy) = locate(&buf, "██████").unwrap();
            let (wx, wy) = locate(&buf, "77%").unwrap();
            if color {
                assert_eq!(buf[(x, y)].fg, Color::Red);
                assert_eq!(buf[(gx, gy)].fg, Color::Red);
                assert_eq!(buf[(wx, wy)].fg, Color::Yellow);
            } else {
                for (x, y) in [(x, y), (gx, gy), (wx, wy)] {
                    assert_eq!(buf[(x, y)].fg, Color::Reset);
                    assert!(buf[(x, y)].modifier.contains(Modifier::BOLD));
                }
            }
        }
    }

    #[test]
    fn the_selected_row_is_reverse_video_across_the_width() {
        let state = loaded(three());
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        for x in [0, 10, 79] {
            assert!(buf[(x, 2)].modifier.contains(Modifier::REVERSED), "{x}");
            assert!(!buf[(x, 3)].modifier.contains(Modifier::REVERSED), "{x}");
        }
    }

    /// Review Focus 2: names wider than their column are cut by display width, never by bytes,
    /// and the table never overflows or panics at 80 columns.
    #[test]
    fn wide_names_are_cut_by_display_width_and_never_overflow() {
        let long = "a".repeat(52) + "@example.com";
        let mut cjk = account(2, "山田太郎@x.co", read(60, 9.0, 12.0, false));
        cjk.row.alias = Some("山田さんの仕事用アカウント😀😀😀😀".into());
        let accounts = vec![
            account(1, &long, read(60, 9.0, 12.0, false)),
            cjk,
            account(
                3,
                "😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀@x.co",
                read(60, 9.0, 12.0, false),
            ),
        ];
        for w in [80, 100, 120] {
            let buf = draw_buffer(&loaded(accounts.clone()), w, 24, NOW_MS);
            let lines = text_of(&buf);
            for line in &lines {
                assert!(width_of(line) <= usize::from(w), "{w}: {line}");
            }
            // Every row's window columns still line up with the header's.
            let col = lines[1].find("5H").unwrap();
            let head = width_of(&lines[1][..col]);
            for row in &lines[2..5] {
                let pct = row.find("9%").unwrap();
                assert_eq!(width_of(&row[..pct]), head + 2, "{w}: {row}");
            }
        }
        let narrow = draw(&loaded(accounts), 80, 24);
        assert!(narrow[2].contains("aaaa…"), "{}", narrow[2]);
        assert!(
            narrow[3].starts_with("    2  山田さんの仕"),
            "{}",
            narrow[3]
        );
    }

    /// Review Focus 3: with no store and no accounts, the empty state.
    #[test]
    fn no_accounts_shows_the_empty_state() {
        let state = loaded(vec![]);
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[1],
            " No accounts yet. Log in with claude, then press A to add the login."
        );
        assert_eq!(state.accounts.selected(), None);
        assert!(state.accounts.shown_ids().is_empty());
        let unloaded = draw(&fresh(Screen::Accounts), 80, 24);
        assert_eq!(unloaded[1], " loading…");
    }

    /// Review Focus 4: the table scrolls so the selected row is always visible, and `j`/`k`
    /// past either end stops there.
    #[test]
    fn more_accounts_than_fit_scroll_with_the_selection() {
        let mut state = loaded(many(30));
        // 80×24: 22 body rows, 7 for the detail, 1 for the header: 14 account rows.
        let shown = |state: &State| {
            draw(state, 80, 24)[2..16]
                .iter()
                .map(|l| {
                    l.split_whitespace()
                        .find(|w| w.contains('@'))
                        .unwrap()
                        .to_owned()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(shown(&state)[0], "a1@x.co");
        update(&mut state, ch('k'), NOW_MS);
        assert_eq!(
            state.accounts.selected().unwrap().position,
            1,
            "k stops at the top"
        );
        for _ in 0..14 {
            update(&mut state, ch('j'), NOW_MS);
        }
        assert_eq!(state.accounts.selected().unwrap().position, 15);
        assert_eq!(shown(&state).first().unwrap(), "a2@x.co");
        assert_eq!(shown(&state).last().unwrap(), "a15@x.co");
        for _ in 0..40 {
            update(&mut state, key(KeyCode::Down), NOW_MS);
        }
        assert_eq!(
            state.accounts.selected().unwrap().position,
            30,
            "j stops at the end"
        );
        assert_eq!(shown(&state).last().unwrap(), "a30@x.co");
        // Going back up scrolls only once the selection reaches the top line.
        for _ in 0..13 {
            update(&mut state, ch('k'), NOW_MS);
        }
        assert_eq!(shown(&state).first().unwrap(), "a17@x.co");
        update(&mut state, ch('k'), NOW_MS);
        assert_eq!(shown(&state).first().unwrap(), "a16@x.co");
        update(&mut state, key(KeyCode::Home), NOW_MS);
        assert_eq!(shown(&state).first().unwrap(), "a1@x.co");
        // A smaller terminal still shows the selected row.
        update(&mut state, key(KeyCode::End), NOW_MS);
        update(
            &mut state,
            Msg::Resize {
                width: 80,
                height: 24,
            },
            NOW_MS,
        );
        let small = draw(&state, 80, 24);
        assert!(small.iter().any(|l| l.contains("a30@x.co")));
    }

    #[test]
    fn a_resize_keeps_the_tables_scroll_in_range_and_the_next_key_moves_at_once() {
        let mut state = loaded(many(30));
        update(&mut state, key(KeyCode::End), NOW_MS);
        let first = |state: &State, h: u16| {
            draw(state, 80, h)[2]
                .split_whitespace()
                .find(|w| w.contains('@'))
                .unwrap()
                .to_owned()
        };
        assert_eq!(first(&state, 24), "a17@x.co");
        // Tall enough for every row: the table shows from the top, never blank rows.
        update(
            &mut state,
            Msg::Resize {
                width: 80,
                height: 60,
            },
            NOW_MS,
        );
        assert_eq!(first(&state, 60), "a1@x.co");
        update(&mut state, ch('k'), NOW_MS);
        assert_eq!(state.accounts.selected().unwrap().position, 29);
        assert_eq!(first(&state, 60), "a1@x.co");
        // And back: the selected row is in view at once.
        update(
            &mut state,
            Msg::Resize {
                width: 80,
                height: 24,
            },
            NOW_MS,
        );
        assert!(draw(&state, 80, 24).iter().any(|l| l.contains("a29@x.co")));
    }

    /// Review Focus 5: the selection follows the account, not the row.
    #[test]
    fn the_selection_follows_the_account_across_a_refresh() {
        let mut st = AccountsState::default();
        let mut accounts = many(4);
        st.apply(snapshot(accounts.clone()));
        st.step(2);
        assert_eq!(st.selected().unwrap().id, id(3));
        // Reordered: 3 moved to the top.
        let moved = accounts.remove(2);
        accounts.insert(0, moved);
        st.apply(snapshot(accounts.clone()));
        assert_eq!(st.selected().unwrap().id, id(3));
        assert_eq!(st.index(), Some(0));
        // Gone: the row now at its old place.
        st.step(1);
        assert_eq!(st.selected().unwrap().id, id(1));
        accounts.remove(1);
        st.apply(snapshot(accounts.clone()));
        assert_eq!(st.selected().unwrap().id, id(2), "{:?}", st.selected());
        // The last row gone: the new last row.
        st.step(10);
        let last = st.selected().unwrap().id;
        accounts.retain(|v| v.row.id != last);
        st.apply(snapshot(accounts.clone()));
        assert_eq!(st.selected().unwrap().id, accounts.last().unwrap().row.id);
        // Everything gone, then back: the first row.
        st.apply(snapshot(vec![]));
        assert_eq!(st.selected(), None);
        st.apply(snapshot(many(2)));
        assert_eq!(st.selected().unwrap().id, id(1));
    }

    #[test]
    fn the_selection_names_the_account_as_last_read() {
        let mut accounts = many(2);
        accounts[1].row.position = 7;
        let mut state = loaded(accounts);
        update(&mut state, ch('j'), NOW_MS);
        assert_eq!(
            state.accounts.selected(),
            Some(AccountRef {
                id: id(2),
                provider: cc(),
                position: 7,
            })
        );
        assert_eq!(state.accounts.shown_ids(), [id(1), id(2)]);
    }

    #[test]
    fn enter_opens_history_for_the_selected_account() {
        let mut state = loaded(many(3));
        update(&mut state, ch('j'), NOW_MS);
        update(&mut state, key(KeyCode::Enter), NOW_MS);
        assert_eq!(state.screen, Screen::History);
        assert_eq!(state.history.account(), Some(&id(2)));
        let mut empty = loaded(vec![]);
        let cmds = update(&mut empty, key(KeyCode::Enter), NOW_MS);
        assert_eq!((empty.screen, cmds), (Screen::Accounts, vec![]));
    }

    #[test]
    fn several_providers_are_grouped_under_their_names() {
        let mut other = account(1, "o@x.co", read(60, 50.0, 60.0, false));
        other.row.provider = ProviderId::new("fake-agent");
        other.row.id = AccountId::from_string("other-1");
        let mut snap = snapshot(many(1));
        snap.providers.push(ProviderAccounts {
            provider: ProviderId::new("fake-agent"),
            active_position: None,
            accounts: vec![other],
        });
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        let lines = draw(&state, 80, 24);
        assert_eq!(lines[1], " claude-code");
        assert!(lines[2].starts_with("    #  ACCOUNT"), "{}", lines[2]);
        assert!(lines[3].contains("a1@x.co"));
        assert_eq!(lines[4], " fake-agent");
        assert!(lines[5].starts_with("    #  ACCOUNT"));
        assert!(lines[6].contains("o@x.co"));
        update(&mut state, ch('j'), NOW_MS);
        assert_eq!(
            state.accounts.selected().unwrap().provider,
            ProviderId::new("fake-agent")
        );
    }

    #[test]
    fn a_run_shell_or_an_unmanaged_live_login_raises_a_banner() {
        let mut snap = snapshot(many(1));
        snap.unmanaged_live = Some("s@x.co".into());
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap.clone());
        assert_eq!(
            draw(&state, 80, 24)[1],
            " Live: s@x.co (not managed by tagteam); press A to add it"
        );
        snap.in_run_shell = true;
        state.accounts.apply(snap);
        assert_eq!(
            draw(&state, 80, 24)[1],
            " inside a `tagteam run` session: account changes are refused here"
        );
        state.banner = Some("the store is locked".into());
        assert_eq!(draw(&state, 80, 24)[1], " the store is locked");
    }

    #[test]
    fn a_long_unmanaged_live_login_banner_wraps_and_still_says_press_a() {
        // Spec §5.1: at 80 columns the whole banner shows, its instruction included.
        let long = "a".repeat(48) + "@example.com";
        let mut snap = snapshot(many(1));
        snap.unmanaged_live = Some(long.clone());
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[1..3],
            [
                format!(" Live: {long} (not managed"),
                " by tagteam); press A to add it".to_owned(),
            ]
        );
        assert!(lines[3].starts_with("    #  ACCOUNT"), "{}", lines[3]);
    }

    #[test]
    fn the_api_key_and_disabled_notes_follow_list() {
        let mut key = account(
            1,
            "api-key-1@token.local",
            unread(UsageStatus::ApiKey, None, None),
        );
        key.kind = API_KEY;
        let mut off = account(2, "b@x.co", read(60, 9.0, 12.0, false));
        off.row.disabled = true;
        let lines = draw(&loaded(vec![key, off]), 80, 24);
        assert!(lines[2].ends_with("api key"), "{}", lines[2]);
        assert!(lines[3].ends_with("disabled"), "{}", lines[3]);
    }
}
```

Append to the test module in `crates/tagteam-tui/src/view.rs` (Task 5's):

```rust
    #[test]
    fn flow_wraps_parts_under_the_indent_and_cuts_only_a_piece_too_wide() {
        let text = |lines: Vec<Line<'static>>| -> Vec<String> {
            lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect()
        };
        let head = vec![Span::raw(" ab   "), Span::raw("99%")];
        let parts = vec![
            "resets in 1h".to_owned(),
            "▲ ahead".to_owned(),
            "+1.0 pts/h · runs out in 2h (regression)".to_owned(),
        ];
        assert_eq!(
            text(flow(head.clone(), parts.clone(), 6, 30)),
            [
                " ab   99%  resets in 1h",
                "      ▲ ahead  +1.0 pts/h",
                "      runs out in 2h (regress…",
            ],
            "a part that fits nowhere whole breaks at its ` · `; only a piece too wide is cut"
        );
        assert_eq!(
            text(flow(head.clone(), parts.clone(), 6, 120)),
            [" ab   99%  resets in 1h  ▲ ahead  +1.0 pts/h · runs out in 2h (regression)"]
        );
        assert_eq!(
            text(flow(head, parts[2..].to_vec(), 6, 50)),
            [
                " ab   99%",
                "      +1.0 pts/h · runs out in 2h (regression)",
            ],
            "a part that fits a continuation line whole moves there whole"
        );
        assert_eq!(
            text(flow(
                vec![Span::raw("    ")],
                vec!["no rate yet".to_owned()],
                4,
                30
            )),
            ["    no rate yet"],
            "a line with nothing past the indent takes its first part unseparated"
        );
    }
```

The expected rows follow `list`'s layout. Compare the first test with `render.rs`'s
`the_list_is_a_table_of_windows_countdowns_pace_and_age`, whose first three accounts are these
three with spend and Fable windows added: the columns, the countdowns, `▲ pace`, `—` for a
missing window and the `relogin required` row are laid out the same way.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-tui --lib screens::accounts`
Expected: the build fails with E0599 (no method named `selected` found for struct
`AccountsState`, and likewise `shown_ids`, `step` and `index`), with `AccountRef`,
`AccountView`, `width_of` and `Screen` not in scope, and with E0425 (cannot find function
`flow`) in `view.rs`'s test.

- [ ] **Step 3: Implement**

In `crates/tagteam-tui/src/view.rs`, after `fn areas`: `body_at`, and R20's helpers, which
History (Task 7) shares: `flow` wraps a window's parts, and `scrolled_windows` and
`furthest_scroll` scroll whole windows by their lines.

```rust
/// The body's area at the terminal's last known size, for the keys that page or scroll.
pub(crate) fn body_at(state: &State) -> Rect {
    let (w, h) = state.size;
    areas(state, Rect::new(0, 0, w, h)).body
}

/// `head`, then `parts`, two spaces apart, in lines of at most `width` columns (R20). A part
/// that does not fit goes on to a continuation line that starts `indent` columns in. One wider
/// than a whole continuation line breaks at its own ` · `s, and only a piece wider even than
/// that is cut (`fit`). A line with nothing past `indent` takes its first part unseparated.
pub(crate) fn flow(
    head: Vec<Span<'static>>,
    parts: Vec<String>,
    indent: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let used = head.iter().map(|s| width_of(&s.content)).sum();
    let mut flow = Flow {
        lines: Vec::new(),
        spans: head,
        used,
        indent,
        width,
    };
    for part in parts {
        if flow.fits("  ", &part) || width_of(&part) <= width.saturating_sub(indent) {
            flow.put("  ", part);
            continue;
        }
        for (i, piece) in part.split(" · ").enumerate() {
            flow.put(if i == 0 { "  " } else { " · " }, piece.to_owned());
        }
    }
    flow.lines.push(Line::from(flow.spans));
    flow.lines
}

/// `flow`'s lines so far, and the one it is filling.
struct Flow {
    lines: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    used: usize,
    indent: usize,
    width: usize,
}

impl Flow {
    /// Nothing on this line yet past the indent.
    fn fresh(&self) -> bool {
        self.used <= self.indent
    }

    fn fits(&self, sep: &str, text: &str) -> bool {
        let sep = if self.fresh() { 0 } else { width_of(sep) };
        self.used + sep + width_of(text) <= self.width
    }

    fn put(&mut self, sep: &str, text: String) {
        if !self.fits(sep, &text) && !self.fresh() {
            let next = vec![Span::raw(" ".repeat(self.indent))];
            self.lines
                .push(Line::from(std::mem::replace(&mut self.spans, next)));
            self.used = self.indent;
        }
        let text = if self.fresh() {
            fit(&text, self.width.saturating_sub(self.used))
        } else {
            format!("{sep}{text}")
        };
        self.used += width_of(&text);
        self.spans.push(Span::raw(text));
    }
}

/// The windows a list scrolled to window `from` shows in `rows` lines, given each window's
/// height (R20): whole windows only, and at least one. `from` goes no further than
/// `furthest_scroll`.
pub(crate) fn scrolled_windows(
    heights: &[usize],
    from: usize,
    rows: usize,
) -> std::ops::Range<usize> {
    let from = from.min(furthest_scroll(heights, rows));
    let (mut to, mut used) = (from, 0);
    while to < heights.len() && (to == from || used + heights[to] <= rows) {
        used += heights[to];
        to += 1;
    }
    from..to
}

/// How far `J` scrolls: the first window from which every later one fits in `rows` lines, so
/// the last window shows.
pub(crate) fn furthest_scroll(heights: &[usize], rows: usize) -> usize {
    let (mut from, mut used) = (heights.len(), 0);
    while from > 0 && used + heights[from - 1] <= rows {
        used += heights[from - 1];
        from -= 1;
    }
    from.min(heights.len().saturating_sub(1))
}
```

In `crates/tagteam-tui/src/actions.rs`, `HELP_ACCOUNTS` (Task 5's) becomes
`[(&str, &str); 11]`, with a last entry after `("Enter", "history of the account")`:

```rust
    ("J K", "scroll the detail's windows"),
```

Everything above the test module in `crates/tagteam-tui/src/screens/accounts.rs` becomes:

```rust
//! The Accounts screen (spec §5.2): `list`'s table, in `list`'s words, with a gauge per window
//! when the terminal is wide enough, and the selected account's detail below it.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tagteam_core::{AccountId, Pace, ProviderId, Window, WindowKind};
use tagteam_engine::views::{AccountView, HistoryWindow, UsageStatus};
use tagteam_engine::words::{self, WindowColumn};

use crate::host::AccountRef;
use crate::msg::Cmd;
use crate::source::AccountsSnapshot;
use crate::state::{Screen, State};
use crate::update;
use crate::view::{
    bold, fit, flow, furthest_scroll, pad, scrolled_windows, selected, severity_style, width_of,
    wrap_rows,
};

/// Spec §10.4's empty state: no store, or no accounts.
pub const NO_ACCOUNTS: &str = "No accounts yet. Log in with claude, then press A to add the login.";
/// Spec §6.1: drawn until the first `accounts` result arrives.
pub const LOADING: &str = "loading…";
/// The banners the accounts view raises (spec §5.1).
pub const RUN_SHELL: &str = "inside a `tagteam run` session: account changes are refused here";
/// A gauge's width, in cells, and the narrowest the ACCOUNT column is cut to.
const GAUGE: usize = 6;
const ACCOUNT_MIN: usize = 12;
/// The detail pane's least height: a rule, four windows, the reading's age, and its trouble.
const DETAIL_MIN: u16 = 7;
/// The detail pane's lines besides its windows' own: the rule, the reading's age, and its
/// trouble.
const DETAIL_FRAME: usize = 3;
/// The table keeps this many rows at least, its header and the selected row among them, when
/// the pane grows past half the body to hold its tallest window.
const TABLE_MIN: u16 = 3;
/// The rows a long label takes above its value, at most (R20).
const LABEL_ROWS: usize = 2;

#[derive(Default)]
pub struct AccountsState {
    snapshot: Option<AccountsSnapshot>,
    /// The selected account, by ID, so a refresh that reorders rows keeps it (Review Focus 5).
    selected: Option<AccountId>,
    /// The first table line shown.
    offset: usize,
    /// The first of the selected account's windows the detail pane shows (`J`/`K`), when they
    /// do not all fit.
    detail_from: usize,
}

impl AccountsState {
    pub fn snapshot(&self) -> Option<&AccountsSnapshot> {
        self.snapshot.as_ref()
    }

    /// Takes a new read. The selection stays on the same account; when that account is gone it
    /// moves to the row now at its old place, or the last row (Review Focus 5).
    pub fn apply(&mut self, snap: AccountsSnapshot) {
        let old = self.index();
        let ids: Vec<AccountId> = rows_of(&snap).iter().map(|v| v.row.id.clone()).collect();
        let selected = match &self.selected {
            Some(id) if ids.contains(id) => Some(id.clone()),
            _ if ids.is_empty() => None,
            _ => Some(ids[old.unwrap_or(0).min(ids.len() - 1)].clone()),
        };
        if selected != self.selected {
            self.detail_from = 0;
        }
        self.selected = selected;
        self.snapshot = Some(snap);
    }

    /// The selected account, as the TUI last read it: what every account action names.
    pub fn selected(&self) -> Option<AccountRef> {
        self.selected_view().map(|v| AccountRef {
            id: v.row.id.clone(),
            provider: v.row.provider.clone(),
            position: v.row.position,
        })
    }

    /// Every account shown, for `collect_due`.
    pub fn shown_ids(&self) -> Vec<AccountId> {
        self.rows().iter().map(|v| v.row.id.clone()).collect()
    }

    pub(crate) fn selected_view(&self) -> Option<&AccountView> {
        let id = self.selected.as_ref()?;
        self.rows().into_iter().find(|v| &v.row.id == id)
    }

    /// The accounts shown, in table order.
    fn rows(&self) -> Vec<&AccountView> {
        self.snapshot.as_ref().map(rows_of).unwrap_or_default()
    }

    fn index(&self) -> Option<usize> {
        let id = self.selected.as_ref()?;
        self.rows().iter().position(|v| &v.row.id == id)
    }

    /// Moves the selection `by` rows, stopping at either end (Review Focus 4).
    fn step(&mut self, by: isize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let at = self.index().unwrap_or(0) as isize;
        let to = (at + by).clamp(0, rows.len() as isize - 1) as usize;
        let id = rows[to].row.id.clone();
        if self.selected.as_ref() != Some(&id) {
            self.detail_from = 0;
        }
        self.selected = Some(id);
    }
}

/// Every provider's accounts, in the order `list` prints them.
fn rows_of(snap: &AccountsSnapshot) -> Vec<&AccountView> {
    snap.providers.iter().flat_map(|p| &p.accounts).collect()
}

/// An account's windows, as its reading lists them; none without a reading.
fn windows_of(v: &AccountView) -> &[(Window, Pace)] {
    v.usage.windows.as_deref().unwrap_or_default()
}

/// What the accounts view has to say above the body: a run shell (spec §3), else a live login
/// tagteam does not manage (`status`'s own words).
pub fn banner(state: &AccountsState) -> Option<String> {
    let snap = state.snapshot.as_ref()?;
    if snap.in_run_shell {
        return Some(RUN_SHELL.to_owned());
    }
    snap.unmanaged_live
        .as_ref()
        .map(|email| format!("Live: {email} (not managed by tagteam); press A to add it"))
}

pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    let body = crate::view::body_at(state);
    let now_s = now_ms.div_euclid(1000);
    let page = visible_rows(&state.accounts, body, now_s).max(1) as isize;
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => state.accounts.step(1),
        KeyCode::Char('k') | KeyCode::Up => state.accounts.step(-1),
        KeyCode::PageDown => state.accounts.step(page),
        KeyCode::PageUp => state.accounts.step(-page),
        KeyCode::Home | KeyCode::Char('g') => state.accounts.step(isize::MIN / 2),
        KeyCode::End | KeyCode::Char('G') => state.accounts.step(isize::MAX / 2),
        KeyCode::Char('J') => {
            scroll_detail(state, body, now_s, 1);
            return vec![];
        }
        KeyCode::Char('K') => {
            scroll_detail(state, body, now_s, -1);
            return vec![];
        }
        KeyCode::Enter => {
            let Some(id) = state.accounts.selected.clone() else {
                return vec![];
            };
            state.history.select(id);
            return update::show(state, Screen::History, now_ms);
        }
        _ => return vec![],
    }
    let lines = layout(&state.accounts).len();
    let at = line_of(&state.accounts);
    let rows = visible_rows(&state.accounts, body, now_s);
    state.accounts.offset = keep_visible(state.accounts.offset, at, rows, lines);
    vec![]
}

/// `J`/`K`: the detail pane's windows move `by` one, stopping once the last one shows, or at
/// the first.
fn scroll_detail(state: &mut State, body: Rect, now_s: i64, by: isize) {
    let st = &mut state.accounts;
    let Some(v) = st.selected_view() else {
        return;
    };
    let heights = window_heights(v, usize::from(body.width), now_s);
    let furthest = furthest_scroll(&heights, window_rows(st, v, body, now_s));
    // From what shows: a resize or a new read may have left the stored scroll past the end.
    let from = st.detail_from.min(furthest) as isize;
    st.detail_from = (from + by).clamp(0, furthest as isize) as usize;
}

/// The table's area and the detail pane's. The pane has room for the lines of the most windows
/// any account shown has, at the body's width (spec §5.2: each relevant window), and at least
/// `DETAIL_MIN` lines, in at most half the body: the table keeps the rest. Windows that still
/// do not fit scroll (`J`/`K`), a whole window at a time, so the pane is never shorter than its
/// tallest window, taking rows from the table down to `TABLE_MIN` for it (R20).
fn split(state: &AccountsState, body: Rect, now_s: i64) -> (Rect, Rect) {
    let width = usize::from(body.width);
    let heights: Vec<Vec<usize>> = state
        .rows()
        .iter()
        .map(|v| window_heights(v, width, now_s))
        .collect();
    let rows = |n: usize| u16::try_from(DETAIL_FRAME + n).unwrap_or(u16::MAX);
    let most = rows(heights.iter().map(|h| h.iter().sum()).max().unwrap_or(0));
    let tallest = rows(heights.iter().flatten().copied().max().unwrap_or(0));
    let detail = most
        .max(DETAIL_MIN)
        .min(body.height / 2)
        .max(tallest)
        .min(body.height.saturating_sub(TABLE_MIN));
    let table = Rect::new(body.x, body.y, body.width, body.height - detail);
    let pane = Rect::new(body.x, body.y + table.height, body.width, detail);
    (table, pane)
}

/// How many lines the detail pane has for `v`'s windows: its height less the rule, the
/// reading's age, and the trouble line when the usage is not current.
fn window_rows(state: &AccountsState, v: &AccountView, body: Rect, now_s: i64) -> usize {
    let trouble = usize::from(v.usage.status != UsageStatus::Ok);
    usize::from(split(state, body, now_s).1.height).saturating_sub(2 + trouble)
}

/// How many scrolling table lines fit in `body`: one provider's header is pinned above them.
fn visible_rows(state: &AccountsState, body: Rect, now_s: i64) -> usize {
    let pinned = sections(state).len() == 1;
    usize::from(split(state, body, now_s).0.height).saturating_sub(usize::from(pinned))
}

/// The first line to show so that line `at` is visible, moving as little as it can.
fn keep_visible(offset: usize, at: usize, rows: usize, lines: usize) -> usize {
    let rows = rows.max(1);
    let offset = if at < offset {
        at
    } else if at >= offset + rows {
        at + 1 - rows
    } else {
        offset
    };
    offset.min(lines.saturating_sub(rows))
}

/// One line of the scrolling part of the table.
enum TableLine<'a> {
    /// A provider's name, when there is more than one (spec §5.1, "Providers").
    Provider(&'a ProviderId),
    /// A provider's column header, below its name; one provider's is pinned instead.
    Header(usize),
    Account(usize, &'a AccountView),
}

/// The sections shown: one per provider with accounts.
fn sections(state: &AccountsState) -> Vec<(&ProviderId, &[AccountView])> {
    state
        .snapshot
        .as_ref()
        .map(|s| {
            s.providers
                .iter()
                .filter(|p| !p.accounts.is_empty())
                .map(|p| (&p.provider, p.accounts.as_slice()))
                .collect()
        })
        .unwrap_or_default()
}

/// The table's scrolling lines. With one provider its header is pinned above them.
fn layout(state: &AccountsState) -> Vec<TableLine<'_>> {
    let sections = sections(state);
    let several = sections.len() > 1;
    let mut lines = Vec::new();
    for (i, (provider, accounts)) in sections.into_iter().enumerate() {
        if several {
            lines.push(TableLine::Provider(provider));
            lines.push(TableLine::Header(i));
        }
        lines.extend(accounts.iter().map(|v| TableLine::Account(i, v)));
    }
    lines
}

/// The selected account's line.
fn line_of(state: &AccountsState) -> usize {
    let Some(id) = state.selected.as_ref() else {
        return 0;
    };
    layout(state)
        .iter()
        .position(|l| matches!(l, TableLine::Account(_, v) if &v.row.id == id))
        .unwrap_or(0)
}

/// A cell: its text, and its parts with their styles.
struct Cell {
    spans: Vec<Span<'static>>,
}

impl Cell {
    fn plain(text: impl Into<String>) -> Self {
        Cell {
            spans: vec![Span::raw(text.into())],
        }
    }

    fn push(&mut self, text: impl Into<String>, style: Style) {
        self.spans.push(Span::styled(text.into(), style));
    }

    fn width(&self) -> usize {
        self.spans.iter().map(|s| width_of(&s.content)).sum()
    }
}

/// One provider's table, laid out for the width it has.
struct Table<'a> {
    cols: Vec<WindowColumn>,
    widths: Vec<usize>,
    account_w: usize,
    gauges: bool,
    sessions: bool,
    this: Option<&'a AccountId>,
}

impl<'a> Table<'a> {
    /// `list`'s columns for `accounts`, with gauges when the whole of every row still fits
    /// beside them, and the ACCOUNT column cut by display width when even `list`'s line does
    /// not (Review Focus 2).
    fn new(
        accounts: &[AccountView],
        this: Option<&'a AccountId>,
        width: usize,
        now_s: i64,
        color: bool,
    ) -> Self {
        let cols = words::window_columns(accounts);
        let sessions = accounts.iter().any(|v| v.in_session);
        let mut table = Table {
            widths: vec![],
            cols,
            account_w: 0,
            gauges: false,
            sessions,
            this,
        };
        let heads: Vec<usize> = table.heads().iter().map(|h| width_of(h)).collect();
        let widths: Vec<usize> = heads
            .iter()
            .enumerate()
            .map(|(i, head)| {
                accounts
                    .iter()
                    .filter_map(|v| table.cells(v, now_s, color))
                    .map(|cells| cells[i].width())
                    .fold(*head, usize::max)
            })
            .collect();
        let account_w = accounts
            .iter()
            .map(|v| width_of(&words::account_cell(&v.row)))
            .fold(width_of("ACCOUNT"), usize::max);
        let fixed =
            5 + if sessions { 2 } else { 0 } + 2 + widths.iter().map(|w| w + 2).sum::<usize>();
        // The notes after a row's cells (its status in words, `disabled`, `this`) must still
        // fit beside the gauges: they say more than a gauge does.
        let notes_w = accounts
            .iter()
            .filter(|v| table.cells(v, now_s, color).is_some())
            .map(|v| {
                words::row_notes(v, this == Some(&v.row.id), now_s)
                    .iter()
                    .map(|n| 2 + width_of(n))
                    .sum::<usize>()
            })
            .max()
            .unwrap_or(0);
        let gauge_w = table.gauge_columns() * (GAUGE + 1);
        table.widths = widths;
        if fixed + account_w + gauge_w + notes_w <= width {
            table.gauges = true;
            table.account_w = account_w;
            for (i, col) in table.cols.iter().enumerate() {
                if col.kind != WindowKind::Spend {
                    table.widths[i] += GAUGE + 1;
                }
            }
        } else {
            table.account_w = account_w.min(width.saturating_sub(fixed).max(ACCOUNT_MIN));
        }
        table
    }

    /// The window columns that take a gauge: every one but spend.
    fn gauge_columns(&self) -> usize {
        self.cols
            .iter()
            .filter(|c| c.kind != WindowKind::Spend)
            .count()
    }

    fn heads(&self) -> Vec<String> {
        self.cols
            .iter()
            .map(|c| c.head.clone())
            .chain((!self.cols.is_empty()).then(|| "AGE".to_owned()))
            .collect()
    }

    fn header(&self) -> Line<'static> {
        let gap = if self.sessions { "    " } else { "  " };
        let mut text = format!("    #{gap}{}", pad("ACCOUNT", self.account_w));
        for (head, w) in self.heads().iter().zip(&self.widths) {
            text.push_str("  ");
            text.push_str(&pad(head, *w));
        }
        Line::from(Span::styled(text.trim_end().to_owned(), bold()))
    }

    /// A row's window cells and AGE, when it has a reading with windows.
    fn cells(&self, v: &AccountView, now_s: i64, color: bool) -> Option<Vec<Cell>> {
        let ws = v.usage.windows.as_deref().filter(|ws| !ws.is_empty())?;
        let mut cells: Vec<Cell> = self
            .cols
            .iter()
            .map(|c| {
                match ws
                    .iter()
                    .find(|(w, _)| c.key.as_deref() == Some(w.key.as_str()))
                {
                    Some((w, p)) => self.window_cell(w, p, now_s, color),
                    None => self.missing(c),
                }
            })
            .collect();
        cells.push(Cell::plain(
            v.usage
                .age_s
                .map_or_else(|| words::MISSING.to_owned(), words::duration),
        ));
        Some(cells)
    }

    /// `list`'s cell (`  9%  2h40m  ▲ pace`, or the spend amounts), after a gauge.
    fn window_cell(&self, w: &Window, p: &Pace, now_s: i64, color: bool) -> Cell {
        let style = severity_style(color, words::severity(w.pct.round() as i64));
        let mut cell = Cell { spans: vec![] };
        match words::spend_text(w) {
            Some(spend) => cell.push(spend, style),
            None => {
                if self.gauges {
                    cell.push(gauge(w.pct), style);
                    cell.push(" ", Style::new());
                }
                cell.push(
                    format!("{:>4}", format!("{}%", w.pct.round() as i64)),
                    style,
                );
            }
        }
        if let Some(at) = w.resets_at {
            cell.push(format!("  {}", words::countdown(at, now_s)), Style::new());
        }
        if p.ahead == Some(true) {
            cell.push(format!("  {}", words::AHEAD), Style::new());
        }
        cell
    }

    /// A window this row's reading lacks: `—` under the `%`, or at the start of spend.
    fn missing(&self, c: &WindowColumn) -> Cell {
        match c.kind {
            WindowKind::Spend => Cell::plain(words::MISSING),
            _ if self.gauges => {
                Cell::plain(format!("{}{:>4}", " ".repeat(GAUGE + 1), words::MISSING))
            }
            _ => Cell::plain(format!("{:>4}", words::MISSING)),
        }
    }

    fn row(&self, v: &AccountView, now_s: i64, color: bool) -> Line<'static> {
        let session = match (self.sessions, v.in_session) {
            (false, _) => String::new(),
            (true, true) => format!(" {}", words::IN_SESSION),
            (true, false) => "  ".to_owned(),
        };
        let marker = if v.active { "*" } else { " " };
        let mut spans = vec![
            Span::raw(" "),
            Span::styled(marker, bold()),
            Span::raw(format!(" {:>2}{session}  ", v.row.position)),
            Span::raw(pad(&words::account_cell(&v.row), self.account_w)),
        ];
        if let Some(cells) = self.cells(v, now_s, color) {
            for (cell, w) in cells.into_iter().zip(&self.widths) {
                let fill = w.saturating_sub(cell.width());
                spans.push(Span::raw("  "));
                spans.extend(cell.spans);
                spans.push(Span::raw(" ".repeat(fill)));
            }
        }
        let this = self.this == Some(&v.row.id);
        for note in words::row_notes(v, this, now_s) {
            spans.push(Span::raw(format!("  {note}")));
        }
        Line::from(spans)
    }
}

/// A gauge of `GAUGE` cells, filled in proportion to `pct`.
fn gauge(pct: f64) -> String {
    let filled = ((pct.clamp(0.0, 100.0) / 100.0) * GAUGE as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(GAUGE - filled))
}

pub fn render(state: &State, frame: &mut Frame, area: Rect, now_ms: i64) {
    let st = &state.accounts;
    let Some(snap) = st.snapshot.as_ref() else {
        frame.render_widget(Paragraph::new(format!(" {LOADING}")), area);
        return;
    };
    if st.rows().is_empty() {
        frame.render_widget(Paragraph::new(format!(" {NO_ACCOUNTS}")), area);
        return;
    }
    let now_s = now_ms.div_euclid(1000);
    let width = usize::from(area.width);
    let (table_area, pane) = split(st, area, now_s);
    let sections = sections(st);
    let tables: Vec<Table<'_>> = sections
        .iter()
        .map(|(_, accounts)| Table::new(accounts, snap.this.as_ref(), width, now_s, state.color))
        .collect();
    let lines = layout(st);
    let mut shown: Vec<Line<'static>> = Vec::new();
    if tables.len() == 1 {
        shown.push(tables[0].header());
    }
    let rows = visible_rows(st, area, now_s);
    let at = line_of(st);
    let offset = keep_visible(st.offset, at, rows, lines.len());
    for line in lines.iter().skip(offset).take(rows) {
        shown.push(match line {
            TableLine::Provider(p) => Line::from(Span::styled(format!(" {p}"), bold())),
            TableLine::Header(i) => tables[*i].header(),
            TableLine::Account(i, v) => {
                let row = tables[*i].row(v, now_s, state.color);
                if st.selected.as_ref() == Some(&v.row.id) {
                    highlight(row, width)
                } else {
                    row
                }
            }
        });
    }
    frame.render_widget(Paragraph::new(shown), table_area);
    if let Some(v) = st.selected_view() {
        let shown = Shown {
            from: st.detail_from,
            rows: window_rows(st, v, area, now_s),
        };
        frame.render_widget(
            Paragraph::new(detail(v, snap, shown, width, now_s, state.color)),
            pane,
        );
    }
}

/// The selected row in reverse video across the whole width.
fn highlight(row: Line<'static>, width: usize) -> Line<'static> {
    let used: usize = row.spans.iter().map(|s| width_of(&s.content)).sum();
    let mut spans = row.spans;
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    Line::from(spans).style(selected())
}

/// Where the detail pane's windows are scrolled to (`from`, a window), and how many lines they
/// have (`rows`).
struct Shown {
    from: usize,
    rows: usize,
}

/// Each of `v`'s windows' lines in the detail pane at `width`: its label and value, then its
/// reset, its pace and its projection, which go on under the value when they do not fit (R20).
/// The label column is a third of the width at most, so the value and what follows it always
/// have room.
fn window_blocks(
    v: &AccountView,
    width: usize,
    now_s: i64,
    color: bool,
) -> Vec<Vec<Line<'static>>> {
    let windows = windows_of(v);
    let label_w = windows
        .iter()
        .map(|(w, _)| width_of(&w.label))
        .max()
        .unwrap_or(0)
        .min(width / 3);
    windows
        .iter()
        .map(|(w, p)| window_lines(w, p, label_w, width, now_s, color))
        .collect()
}

/// How many lines each of `v`'s windows takes in the detail pane at `width`.
fn window_heights(v: &AccountView, width: usize, now_s: i64) -> Vec<usize> {
    window_blocks(v, width, now_s, false)
        .iter()
        .map(Vec::len)
        .collect()
}

fn window_lines(
    w: &Window,
    p: &Pace,
    label_w: usize,
    width: usize,
    now_s: i64,
    color: bool,
) -> Vec<Line<'static>> {
    let style = severity_style(color, words::severity(w.pct.round() as i64));
    let value = words::spend_text(w)
        .unwrap_or_else(|| format!("{:>4}", format!("{}%", w.pct.round() as i64)));
    // A label wider than the column takes rows of its own, above its value: `LABEL_ROWS` at
    // most, the rest cut.
    let mut lines: Vec<Line<'static>> = Vec::new();
    let label = if width_of(&w.label) > label_w {
        let rows = wrap_rows(&w.label, width.saturating_sub(1), LABEL_ROWS);
        lines.extend(rows.into_iter().map(|row| Line::from(format!(" {row}"))));
        ""
    } else {
        w.label.as_str()
    };
    let lead = format!(" {}  ", pad(label, label_w));
    let indent = width_of(&lead);
    // One row for the value whatever it is: a spend can be any number.
    let value = fit(&value, width.saturating_sub(indent));
    let mut parts = Vec::new();
    if let Some(at) = w.resets_at {
        let when = words::countdown(at, now_s);
        parts.push(if at > now_s {
            format!("resets in {when}")
        } else {
            when
        });
    }
    if p.ahead == Some(true) {
        let expected = p.expected_pct.map_or_else(String::new, |e| {
            format!(" (expected {}%)", e.round() as i64)
        });
        parts.push(format!("▲ ahead of pace{expected}"));
    }
    if p.rate_per_hour.is_some() {
        let hw = HistoryWindow {
            window: w.clone(),
            samples: vec![],
            pace: *p,
        };
        parts.push(words::projection_text(&hw, now_s));
    }
    let head = vec![Span::raw(lead), Span::styled(value, style)];
    lines.extend(flow(head, parts, indent, width));
    lines
}

/// The detail pane (spec §5.2): the account, each window with its reset, pace and projection,
/// the reading's age and the login's expiry, and why the usage is not current. Windows that do
/// not fit are scrolled to (`J`/`K`), which the rule says. On the rule the name gives way
/// first: the facts after it are the pane's to say.
fn detail(
    v: &AccountView,
    snap: &AccountsSnapshot,
    shown: Shown,
    width: usize,
    now_s: i64,
    color: bool,
) -> Vec<Line<'static>> {
    let row = &v.row;
    let mut facts = vec![
        if v.active { "active" } else { "not active" }.to_owned(),
        v.kind
            .display
            .map_or_else(|| row.kind.clone(), str::to_owned),
    ];
    if v.in_session {
        facts.push("in a session".into());
    }
    if snap.this.as_ref() == Some(&row.id) {
        facts.push(format!("{} {}", words::IN_SESSION, words::THIS));
    }
    if row.disabled {
        facts.push("disabled".into());
    }
    let blocks = window_blocks(v, width, now_s, color);
    let heights: Vec<usize> = blocks.iter().map(Vec::len).collect();
    let range = scrolled_windows(&heights, shown.from, shown.rows);
    let more = if range != (0..blocks.len()) {
        let (from, to) = (range.start + 1, range.end);
        format!(" J K: windows {from}–{to} of {} ─", blocks.len())
    } else {
        String::new()
    };
    let head = format!("─ {} ", row.position);
    let facts = format!(" · {} ", facts.join(" · "));
    // One column of rule at least.
    let room = width.saturating_sub(width_of(&head) + width_of(&facts) + width_of(&more) + 1);
    let name = fit(&words::account_cell(row), room.max(1));
    let title = fit(
        &format!("{head}{name}{facts}"),
        width.saturating_sub(width_of(&more)),
    );
    let rule = "─".repeat(width.saturating_sub(width_of(&title) + width_of(&more)));
    let mut lines = vec![Line::from(Span::styled(
        format!("{title}{rule}{more}"),
        bold(),
    ))];
    for block in blocks.into_iter().take(range.end).skip(range.start) {
        lines.extend(block);
    }
    let mut status = Vec::new();
    match v.usage.age_s {
        Some(age) => status.push(format!("fetched {} ago", words::duration(age))),
        None => status.push("never fetched".to_owned()),
    }
    if let Some(at) = v.usage.next_poll_at.filter(|&at| at > now_s) {
        status.push(format!("next poll in {}", words::countdown(at, now_s)));
    }
    if let Some(at_ms) = row.login_expires_at {
        let at = at_ms.div_euclid(1000);
        status.push(if at > now_s {
            format!("login expires in {}", words::duration(at - now_s))
        } else {
            "login expired".to_owned()
        });
    }
    lines.push(Line::from(format!(" {}", status.join(" · "))));
    if v.usage.status != UsageStatus::Ok {
        lines.push(Line::from(format!(
            " usage: {}",
            words::usage_words(&v.usage, now_s)
        )));
    }
    lines
}
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-tui --lib screens::accounts`
Expected: PASS, 27 tests.

Run: `cargo test -p tagteam-tui`
Expected: PASS.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-tui/src/screens/accounts.rs crates/tagteam-tui/src/view.rs \
  crates/tagteam-tui/src/actions.rs
git commit -m "Show the accounts table in the TUI"
```

---

### Task 7: History screen

The History screen (spec §5.4) lists the accounts on the left and the shown account's windows on
the right, as `Engine::history` returns them (the relevant windows, core §13.4). Each window gets
a sparkline of its samples across the range, its latest `pct` coloured by severity, and
`history`'s burn rate and projection in `history`'s words (C1's `projection_text`, which calls
`rate_signed` and says whether the window lasts to its reset). It never fetches.

**Readings of the spec this task commits to:**
- **Two lines per window, or more.** At 80 columns the right panel is 53 wide, too narrow for a
  sparkline and `+5.0 pts/h · runs out in 12h00m (regression)` on one line. So the first line is
  the label, the sparkline and the latest `pct`, and the next is the projection, under the
  sparkline. A projection wider than the space there (a long model label leaves 34 columns at
  80) goes on to further lines under it, breaking at its ` · ` (Task 6's `view::flow`, R20).
  The label column is a third of the panel at most; a wider label wraps onto rows of its own
  above its sparkline, two at most, the second ending in `…` when there is more. A window is
  then 5 rows at most, and the panel 16 at the least at 80×24 (R20).
  The reset countdown is left to Accounts; the projection says whether the window lasts to its
  reset.
- **The sparkline** has one bucket per column of the space left after the label and the `pct`,
  from `since_s` to now. Each bucket shows the highest `pct` sampled in it; an empty bucket stays
  blank; a reset shows as a drop; a sample outside the range is left out. The levels are
  `history`'s eight (`crates/tagteam/src/history.rs:11`, `▁` to `█`).
- **The range** is 1, 7, 14 or 30 days, 7 by default (`history`'s default). `]` steps to the
  next, `[` to the previous, and both wrap around. The panel's last line says
  `7 days  [ ] range`.
- **Every window can be reached (R20).** The whole windows whose lines fit above the range line
  are shown, at the panel's width. When there are more, `J`/`K` scroll them a whole window at a
  time and stop at either end (the last window showing, or the first), starting from what shows
  after a resize or a new read (R22), no window is ever shown in part, the range line goes on `  J K: windows <first>–<last> of <n>`, and another account
  starts at its first window. Help lists `J K`.
- **Every read is a new generation.** A range change, an account change (`j`/`k`, stopping at
  either end) or opening the screen returns `Cmd::LoadHistory` under the next history generation
  (`State::load_history`), so Task 8 drops a slower, older result.
- **Which account.** History shows the account `select` named (Accounts' `Enter`), else the first
  account. One that is no longer listed falls back to the first when the screen opens (Task 8
  also does this when a new `accounts` read arrives). Changing the account drops the old view, so
  another account's windows are never shown under its name.
- **The account list** shows `* 1 name` per account (`words::name`), cut to its 26 columns by
  display width, under each provider's ID when there are several (spec §5.1, "Providers"). A
  list longer than the screen scrolls so that the shown account stays in view.
- **Empty states.** No accounts: ` No accounts yet.` on the left and `No usage history.` on the
  right (Review Focus 3). An account with no windows: `No usage history.`. Not read yet:
  `loading…`.

**Depends on:** 1, 5, 6

**Files:**
- Modify: `crates/tagteam-tui/src/screens/history.rs` (Task 5's stub becomes the screen)
- Modify: `crates/tagteam-tui/src/state.rs` (adds `State::load_history`, after
  `load_accounts`, and `AccountId` to its `tagteam_core` import)
- Modify: `crates/tagteam-tui/src/actions.rs` (Task 5's `HELP_HISTORY` lists `J K`)
- Test: unit tests in `screens/history.rs`

**Interfaces:**
- Consumes:
  - Task 1 (C1): `words::{name, severity, projection_text}`.
  - Task 5: `State`, `Screen`, `update::show`, `view::{bold, fit, pad, selected,
    severity_style, width_of, wrap_rows}`, `AccountsState::snapshot`, `testutil`.
  - Task 6: `view::{body_at, flow, scrolled_windows, furthest_scroll}` (R20).
  - The engine's `views::{AccountView, HistoryView, HistoryWindow}` and `tagteam_core::Sample`.
- Produces:
  - C6: `HistoryState` with `account`, `since_s`, `apply`; `handle_key`; `render`.
  - Kept from Task 5: `pub fn HistoryState::select(&mut self, account: AccountId)`.
  - Crate-private, for Tasks 8 and 9: `screens::history::reload(state, now_ms) -> Option<Cmd>`
    and `on_enter(state, now_ms) -> Vec<Cmd>`; `State::load_history(&mut self, account,
    since_s) -> Cmd`.
  - Public constants `NO_HISTORY`, `NO_ACCOUNTS`, `LOADING`.

**Spec:**
- spec §5.4: the account list (`j`/`k`), the windows (every one, R20), the sparkline's buckets,
  the range keys, no fetching.
- spec §4.3, "Coalescing": a slow history load never overwrites a newer selection.
- core §13.4 and §8.7: the windows `history` returns, and their rate and projection.
- Review Focus 3: History shows `No usage history.` with no accounts.
- Review Focus 6: History's windows wrap and scroll.

- [ ] **Step 1: Write the failing tests**

Append to `crates/tagteam-tui/src/screens/history.rs`:

```rust
#[cfg(test)]
mod tests {
    use ratatui::style::{Color, Modifier};
    use tagteam_core::{Pace, ProjectionMethod, WindowKind};
    use tagteam_engine::views::UsageStatus;

    use super::*;
    use crate::msg::{Cmd, Msg};
    use crate::state::Screen;
    use crate::testutil::*;
    use crate::update::update;

    fn on_history(accounts: Vec<AccountView>) -> State {
        let mut state = fresh(Screen::History);
        state.accounts.apply(snapshot(accounts));
        state
    }

    /// 5h with three samples and no rate; 7d running out in 12 hours by regression.
    fn view_of(account: AccountView) -> HistoryView {
        history_of(
            account,
            vec![
                history_window(
                    window("5h", "5h", WindowKind::Short, 9.0, Some(9_600)),
                    vec![
                        sample(6 * HOUR, 5.0),
                        sample(4 * HOUR, 7.0),
                        sample(2 * HOUR, 9.0),
                    ],
                    Pace::default(),
                ),
                history_window(
                    window("7d", "7d", WindowKind::Long, 95.0, Some(266_400)),
                    vec![sample(3 * 86_400, 10.0), sample(HOUR, 95.0)],
                    Pace {
                        rate_per_hour: Some(5.0),
                        method: Some(ProjectionMethod::Regression),
                        exhaustion_at: Some(NOW_S + 43_200),
                        will_last_to_reset: Some(false),
                        ..Pace::default()
                    },
                ),
            ],
        )
    }

    #[test]
    fn the_range_defaults_to_seven_days_and_cycles() {
        let mut state = on_history(many(1));
        state.history.select(id(1));
        assert_eq!(state.history.since_s(NOW_S), NOW_S - 7 * 86_400);
        let mut seen = vec![];
        for _ in 0..4 {
            let cmds = update(&mut state, ch(']'), NOW_MS);
            let [Cmd::LoadHistory { since_s, .. }] = cmds.as_slice() else {
                panic!("{cmds:?}");
            };
            seen.push((NOW_S - since_s) / 86_400);
        }
        assert_eq!(seen, [14, 30, 1, 7]);
        update(&mut state, ch('['), NOW_MS);
        assert_eq!(state.history.since_s(NOW_S), NOW_S - 86_400);
    }

    #[test]
    fn a_range_or_account_change_reads_under_a_new_generation() {
        let mut state = on_history(many(3));
        let opened = crate::update::show(&mut state, Screen::Accounts, NOW_MS);
        assert!(opened.is_empty());
        let cmds = crate::update::show(&mut state, Screen::History, NOW_MS);
        assert_eq!(
            cmds,
            [Cmd::LoadHistory {
                r#gen: 1,
                account: id(1),
                since_s: NOW_S - 7 * 86_400,
            }]
        );
        assert_eq!(
            update(&mut state, ch('j'), NOW_MS),
            [Cmd::LoadHistory {
                r#gen: 2,
                account: id(2),
                since_s: NOW_S - 7 * 86_400,
            }]
        );
        assert_eq!(
            update(&mut state, ch('['), NOW_MS),
            [Cmd::LoadHistory {
                r#gen: 3,
                account: id(2),
                since_s: NOW_S - 86_400,
            }]
        );
        for _ in 0..5 {
            update(&mut state, ch('j'), NOW_MS);
        }
        assert_eq!(state.history.account(), Some(&id(3)), "j stops at the end");
        assert!(update(&mut state, ch('j'), NOW_MS).is_empty());
    }

    #[test]
    fn the_windows_show_a_sparkline_the_latest_pct_and_the_projection() {
        let mut state = on_history(many(2));
        state.history.select(id(1));
        state
            .history
            .apply(view_of(account(1, "a1@x.co", read(60, 9.0, 95.0, false))));
        let lines = draw(&state, 80, 24);
        assert!(lines[1].starts_with(" ACCOUNTS "), "{}", lines[1]);
        assert!(lines[2].starts_with("   1 a1@x.co"), "{}", lines[2]);
        let right: Vec<&str> = lines[1..7]
            .iter()
            .map(|l| l.get(27..).unwrap_or(""))
            .collect();
        let spark = 80 - 27 - 2 - 2 - 2 - 4;
        assert_eq!(
            right[0].chars().count(),
            2 + 2 + spark + 2 + 4,
            "{:?}",
            right[0]
        );
        assert!(right[0].starts_with("5h  "), "{:?}", right[0]);
        assert!(right[0].ends_with("    9%"), "{:?}", right[0]);
        assert_eq!(right[1], "    no rate yet");
        assert!(right[2].ends_with("   95%"), "{:?}", right[2]);
        assert_eq!(right[3], "    +5.0 pts/h · runs out in 12h00m (regression)");
        assert_eq!(lines[22].get(27..).unwrap_or(""), "7 days  [ ] range");
    }

    #[test]
    fn a_projection_too_wide_for_the_panel_wraps_under_the_sparkline() {
        // Spec §5.1: 80×24 is enough; spec §5.4: each window's rate and projection. At 80
        // columns the panel is 53 wide, and a long model label leaves 34 for the projection.
        let mut state = on_history(many(1));
        state.history.select(id(1));
        let sonnet = "Claude Sonnet 4.5";
        let w = window(
            &format!("scoped:{sonnet}"),
            sonnet,
            WindowKind::Scoped,
            95.0,
            None,
        );
        let pace = Pace {
            rate_per_hour: Some(5.0),
            method: Some(ProjectionMethod::Regression),
            exhaustion_at: Some(NOW_S + 43_200),
            will_last_to_reset: Some(false),
            ..Pace::default()
        };
        state.history.apply(history_of(
            account(1, "a1@x.co", read(60, 1.0, 95.0, false)),
            vec![history_window(w, vec![sample(HOUR, 95.0)], pace)],
        ));
        let right = |width: u16| -> Vec<String> {
            draw(&state, width, 24)[1..5]
                .iter()
                .map(|l| l.get(27..).unwrap_or("").trim_end().to_owned())
                .collect()
        };
        let under = " ".repeat(19);
        let narrow = right(80);
        assert!(narrow[0].starts_with("Claude Sonnet 4.5  "), "{narrow:?}");
        assert!(narrow[0].ends_with("  95%"), "{narrow:?}");
        assert_eq!(
            narrow[1..],
            [
                format!("{under}+5.0 pts/h"),
                format!("{under}runs out in 12h00m (regression)"),
                String::new(),
            ]
        );
        let wide = right(120);
        assert_eq!(
            wide[1..3],
            [
                format!("{under}+5.0 pts/h · runs out in 12h00m (regression)"),
                String::new(),
            ]
        );
    }

    #[test]
    fn an_oversized_label_goes_above_its_sparkline_and_the_column_stays_capped() {
        // An 80-column model name: the label column is a third of the 53-column panel at most,
        // so the sparkline, the latest pct and the projection keep room.
        let mut state = on_history(many(1));
        state.history.select(id(1));
        let long =
            "Claude Opus 4.5 with extended thinking, for long-running agentic coding sessions";
        let pace = Pace {
            rate_per_hour: Some(5.0),
            method: Some(ProjectionMethod::Regression),
            exhaustion_at: Some(NOW_S + 43_200),
            will_last_to_reset: Some(false),
            ..Pace::default()
        };
        let five = window("5h", "5h", WindowKind::Short, 9.0, None);
        let wide = window(
            &format!("scoped:{long}"),
            long,
            WindowKind::Scoped,
            95.0,
            None,
        );
        state.history.apply(history_of(
            account(1, "a1@x.co", read(60, 9.0, 95.0, false)),
            vec![
                history_window(five, vec![sample(HOUR, 9.0)], Pace::default()),
                history_window(wide, vec![sample(HOUR, 95.0)], pace),
            ],
        ));
        let right: Vec<String> = draw(&state, 80, 24)[1..8]
            .iter()
            .map(|l| l.get(27..).unwrap_or("").trim_end().to_owned())
            .collect();
        assert!(
            right[0].starts_with(&format!("{:<17}  ", "5h")),
            "{right:?}"
        );
        assert!(right[0].ends_with("    9%"), "{right:?}");
        assert_eq!(right[1], format!("{:19}no rate yet", ""));
        assert_eq!([right[2].as_str(), right[3].as_str()].join(" "), long);
        assert!(right[4].starts_with(&" ".repeat(19)), "{right:?}");
        assert!(right[4].ends_with("   95%"), "{right:?}");
        assert_eq!(
            right[5..7],
            [
                format!("{:19}+5.0 pts/h", ""),
                format!("{:19}runs out in 12h00m (regression)", ""),
            ]
        );
    }

    #[test]
    fn a_1000_character_label_leaves_its_window_reachable_at_80x24_under_a_full_banner() {
        // A label takes two rows at most; under a 3-row banner and a 3-row toast the panel has
        // 16 rows, and a window at most five (R20).
        let mut state = on_history(many(1));
        state.history.select(id(1));
        let long = "Claude ".repeat(143).trim_end().to_owned();
        let pace = Pace {
            rate_per_hour: Some(5.0),
            method: Some(ProjectionMethod::Regression),
            exhaustion_at: Some(NOW_S + 43_200),
            will_last_to_reset: Some(false),
            ..Pace::default()
        };
        let five = window("5h", "5h", WindowKind::Short, 9.0, None);
        let wide = window(
            &format!("scoped:{long}"),
            &long,
            WindowKind::Scoped,
            95.0,
            None,
        );
        state.history.apply(history_of(
            account(1, "a1@x.co", read(60, 9.0, 95.0, false)),
            vec![
                history_window(five, vec![sample(HOUR, 9.0)], Pace::default()),
                history_window(wide, vec![sample(HOUR, 95.0)], pace),
            ],
        ));
        state.banner = Some(["the store could not be read"; 12].join("; "));
        state.toast(["Switched to a@x.co (position 1)"; 8].join(" "), NOW_MS);
        let lines = draw(&state, 80, 24);
        let right: Vec<String> = lines[4..20]
            .iter()
            .map(|l| l.get(27..).unwrap_or("").trim_end().to_owned())
            .collect();
        let label = right
            .iter()
            .position(|r| r.starts_with("Claude Claude"))
            .unwrap_or_else(|| panic!("{right:?}"));
        assert!(
            right[label + 1].ends_with('…'),
            "two rows at most: {right:?}"
        );
        assert!(right[label + 2].ends_with("   95%"), "{right:?}");
        assert_eq!(
            right[label + 3..label + 5],
            [
                format!("{:19}+5.0 pts/h", ""),
                format!("{:19}runs out in 12h00m (regression)", ""),
            ]
        );
    }

    #[test]
    fn a_resize_clamps_the_window_scroll_so_the_next_key_moves_at_once() {
        let mut state = on_history(many(1));
        state.history.select(id(1));
        let windows = (1..=20)
            .map(|i| {
                let (key, label) = (format!("scoped:M{i}"), format!("M{i}"));
                let w = window(&key, &label, WindowKind::Scoped, 10.0, None);
                history_window(w, vec![], Pace::default())
            })
            .collect();
        state.history.apply(history_of(
            account(1, "a1@x.co", read(60, 1.0, 1.0, false)),
            windows,
        ));
        let footer = |state: &State, h: u16| {
            draw(state, 80, h)[usize::from(h) - 2]
                .get(27..)
                .unwrap()
                .to_owned()
        };
        for _ in 0..20 {
            update(&mut state, ch('J'), NOW_MS);
        }
        assert!(footer(&state, 24).ends_with("J K: windows 11–20 of 20"));
        // Taller: the last window shows from further up, and `K` moves from what shows.
        update(
            &mut state,
            Msg::Resize {
                width: 80,
                height: 30,
            },
            NOW_MS,
        );
        assert!(footer(&state, 30).ends_with("J K: windows 8–20 of 20"));
        update(&mut state, ch('K'), NOW_MS);
        assert!(
            footer(&state, 30).ends_with("J K: windows 7–19 of 20"),
            "{}",
            footer(&state, 30)
        );
    }

    #[test]
    fn windows_beyond_the_screen_scroll_into_view_with_shift_j_and_k() {
        // Spec §5.4: the account's windows, every one. 80×24 holds ten of two lines each.
        let mut state = on_history(many(2));
        state.history.select(id(1));
        let twelve = |position| {
            let windows = (1..=12)
                .map(|i| {
                    let (key, label) = (format!("scoped:M{i}"), format!("M{i}"));
                    let w = window(&key, &label, WindowKind::Scoped, 10.0, None);
                    history_window(w, vec![], Pace::default())
                })
                .collect();
            let email = format!("a{position}@x.co");
            history_of(
                account(position, &email, read(60, 1.0, 1.0, false)),
                windows,
            )
        };
        state.history.apply(twelve(1));
        let labels = |state: &State| -> Vec<String> {
            draw(state, 80, 24)[1..22]
                .iter()
                .filter_map(|l| l.get(27..))
                .filter(|r| r.starts_with('M'))
                .map(|r| r.split_whitespace().next().unwrap().to_owned())
                .collect()
        };
        let footer = |state: &State| draw(state, 80, 24)[22].get(27..).unwrap().to_owned();
        let first: Vec<String> = (1..=10).map(|i| format!("M{i}")).collect();
        assert_eq!(labels(&state), first);
        assert_eq!(footer(&state), "7 days  [ ] range  J K: windows 1–10 of 12");
        for _ in 0..5 {
            assert!(
                update(&mut state, ch('J'), NOW_MS).is_empty(),
                "nothing is read"
            );
        }
        let last: Vec<String> = (3..=12).map(|i| format!("M{i}")).collect();
        assert_eq!(labels(&state), last, "J stops at the last window");
        assert_eq!(footer(&state), "7 days  [ ] range  J K: windows 3–12 of 12");
        update(&mut state, ch('K'), NOW_MS);
        assert_eq!(labels(&state)[0], "M2");
        // Another account's windows start at its first.
        update(&mut state, ch('j'), NOW_MS);
        state.history.apply(twelve(2));
        assert_eq!(labels(&state), first);
        // A taller terminal shows them all.
        assert!(draw(&state, 80, 40)[38].get(27..).unwrap() == "7 days  [ ] range");
        // Help lists the keys.
        update(&mut state, ch('?'), NOW_MS);
        let help = draw(&state, 80, 24);
        assert!(
            help.iter()
                .any(|l| l.contains("J K") && l.contains("scroll the windows")),
            "{help:?}"
        );
    }

    #[test]
    fn a_bucket_shows_its_peak_and_an_empty_one_stays_blank() {
        let since = NOW_S - 10 * HOUR;
        // Ten buckets of an hour: a peak in the third, nothing in the fourth, a reset in the
        // eighth.
        let samples = [
            Sample {
                fetched_at: since + 2 * HOUR + 10,
                pct: 20.0,
                resets_at: None,
            },
            Sample {
                fetched_at: since + 2 * HOUR + 20,
                pct: 100.0,
                resets_at: None,
            },
            Sample {
                fetched_at: since + 2 * HOUR + 30,
                pct: 40.0,
                resets_at: None,
            },
            Sample {
                fetched_at: since + 6 * HOUR,
                pct: 90.0,
                resets_at: None,
            },
            Sample {
                fetched_at: since + 7 * HOUR,
                pct: 0.0,
                resets_at: None,
            },
            Sample {
                fetched_at: NOW_S,
                pct: 14.3,
                resets_at: None,
            },
            Sample {
                fetched_at: since - 1,
                pct: 100.0,
                resets_at: None,
            },
        ];
        assert_eq!(sparkline(&samples, since, NOW_S, 10), "  █   ▇▁ ▂");
        assert_eq!(sparkline(&[], since, NOW_S, 4), "    ");
        assert_eq!(sparkline(&samples, since, NOW_S, 0), "");
    }

    #[test]
    fn the_latest_pct_takes_its_severity() {
        let mut state = on_history(many(1));
        state.color = true;
        state.history.select(id(1));
        state
            .history
            .apply(view_of(account(1, "a1@x.co", read(60, 9.0, 95.0, false))));
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        let (x, y) = locate(&buf, "95%").unwrap();
        assert_eq!(buf[(x, y)].fg, Color::Red);
        state.color = false;
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        assert!(buf[(x, y)].modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn several_providers_are_grouped_and_a_long_list_keeps_the_shown_account_in_view() {
        let mut other = account(1, "o@x.co", read(60, 1.0, 1.0, false));
        other.row.provider = tagteam_core::ProviderId::new("fake-agent");
        other.row.id = AccountId::from_string("other-1");
        let mut snap = snapshot(many(1));
        snap.providers
            .push(tagteam_engine::views::ProviderAccounts {
                provider: tagteam_core::ProviderId::new("fake-agent"),
                active_position: None,
                accounts: vec![other],
            });
        let mut state = fresh(Screen::History);
        state.accounts.apply(snap);
        let left = |state: &State| -> Vec<String> {
            draw(state, 80, 24)[1..23]
                .iter()
                .map(|l| l.chars().take(26).collect::<String>().trim_end().to_owned())
                .collect()
        };
        assert_eq!(
            &left(&state)[..5],
            [
                " ACCOUNTS",
                " claude-code",
                "   1 a1@x.co",
                " fake-agent",
                "   1 o@x.co"
            ]
        );
        let mut long = on_history(many(40));
        long.history.select(id(35));
        let shown = left(&long);
        assert_eq!(shown[0], " ACCOUNTS");
        assert_eq!(shown.last().unwrap(), "   35 a35@x.co", "{shown:?}");
    }

    #[test]
    fn the_empty_states_say_so() {
        let state = on_history(vec![]);
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[1].trim_end(),
            " ACCOUNTS                  No usage history."
        );
        assert_eq!(lines[2], " No accounts yet.");
        let mut read = on_history(many(1));
        read.history.select(id(1));
        assert!(draw(&read, 80, 24)[1].ends_with(LOADING));
        read.history.apply(history_of(
            account(1, "a1@x.co", unread(UsageStatus::Unavailable, None, None)),
            vec![],
        ));
        assert!(draw(&read, 80, 24)[1].ends_with(NO_HISTORY));
    }
}
```

`the_windows_show_a_sparkline_the_latest_pct_and_the_projection` reads the right panel from
column 27 on: the account list is 26 wide, then one column of gap.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-tui --lib screens::history`
Expected: the build fails with E0599 (no method named `since_s` found for struct
`HistoryState`, and likewise `apply`), E0425 (cannot find function `sparkline`, nor the values
`LOADING` and `NO_HISTORY`), and with `Sample`, `AccountView` and `HistoryView` not in scope.

- [ ] **Step 3: Implement**

In `crates/tagteam-tui/src/actions.rs`, Task 5's one-line `HELP_HISTORY` becomes:

```rust
const HELP_HISTORY: [(&str, &str); 3] = [
    ("[ ]", "range: 1, 7, 14 or 30 days"),
    ("r", "refresh"),
    ("J K", "scroll the windows"),
];
```

In `crates/tagteam-tui/src/state.rs`, change the `tagteam_core` import to
`use tagteam_core::{AccountId, ProviderId};`, and add after `load_accounts`:

```rust
    /// A read of `account`'s history since `since_s`, under the next generation.
    pub(crate) fn load_history(&mut self, account: AccountId, since_s: i64) -> Cmd {
        self.gens.history += 1;
        Cmd::LoadHistory {
            r#gen: self.gens.history,
            account,
            since_s,
        }
    }
```

Everything above the test module in `crates/tagteam-tui/src/screens/history.rs` becomes:

```rust
//! The History screen (spec §5.4): the accounts on the left, the selected account's windows on
//! the right, each with a sparkline of its samples across the range, its latest `pct`, and
//! `history`'s rate and projection. It never fetches (core §13.4).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tagteam_core::{AccountId, Sample};
use tagteam_engine::views::{AccountView, HistoryView, HistoryWindow};
use tagteam_engine::words;

use crate::msg::Cmd;
use crate::state::State;
use crate::view::{
    bold, fit, flow, furthest_scroll, pad, scrolled_windows, selected, severity_style, width_of,
    wrap_rows,
};

pub const NO_HISTORY: &str = "No usage history.";
pub const NO_ACCOUNTS: &str = "No accounts yet.";
pub const LOADING: &str = "loading…";
/// Eight levels from 0 to 100 %, as `history` draws them.
const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// The account list's width, and the gap after it.
const LIST_W: u16 = 26;
/// The rows a long label takes above its sparkline, at most (R20).
const LABEL_ROWS: usize = 2;

/// The ranges `[` and `]` cycle through (spec §5.4); 7 days is `history`'s default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Range {
    Day,
    #[default]
    Week,
    Fortnight,
    Month,
}

impl Range {
    const ALL: [Range; 4] = [Range::Day, Range::Week, Range::Fortnight, Range::Month];

    fn days(self) -> i64 {
        match self {
            Range::Day => 1,
            Range::Week => 7,
            Range::Fortnight => 14,
            Range::Month => 30,
        }
    }

    fn label(self) -> String {
        match self.days() {
            1 => "1 day".to_owned(),
            n => format!("{n} days"),
        }
    }

    fn step(self, by: isize) -> Range {
        let at = Self::ALL.iter().position(|r| *r == self).unwrap_or(1) as isize;
        Self::ALL[(at + by).rem_euclid(Self::ALL.len() as isize) as usize]
    }
}

#[derive(Default)]
pub struct HistoryState {
    /// The account whose history is shown, by ID.
    account: Option<AccountId>,
    range: Range,
    /// The last history read for `account`.
    view: Option<HistoryView>,
    /// The first window shown (`J`/`K`), when they do not all fit.
    from: usize,
}

impl HistoryState {
    pub fn account(&self) -> Option<&AccountId> {
        self.account.as_ref()
    }

    /// Now less the selected range: 1, 7, 14 or 30 days, 7 by default.
    pub fn since_s(&self, now_s: i64) -> i64 {
        now_s - self.range.days() * 86_400
    }

    pub fn apply(&mut self, view: HistoryView) {
        self.view = Some(view);
    }

    /// Shows `account`'s history; another account's view is dropped until it is read.
    pub fn select(&mut self, account: AccountId) {
        if self.account.as_ref() != Some(&account) {
            self.view = None;
            self.from = 0;
        }
        self.account = Some(account);
    }
}

/// Every account in the accounts view, in its order.
fn accounts(state: &State) -> Vec<&AccountView> {
    state
        .accounts
        .snapshot()
        .map(|s| s.providers.iter().flat_map(|p| &p.accounts).collect())
        .unwrap_or_default()
}

/// A read of the shown account's history over the selected range, under a new generation.
pub(crate) fn reload(state: &mut State, now_ms: i64) -> Option<Cmd> {
    let account = state.history.account.clone()?;
    let since = state.history.since_s(now_ms.div_euclid(1000));
    Some(state.load_history(account, since))
}

/// History opens on its account, or on the first account when it has none or its account is
/// gone, and reads it.
pub(crate) fn on_enter(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    let ids: Vec<AccountId> = accounts(state).iter().map(|v| v.row.id.clone()).collect();
    match &state.history.account {
        Some(id) if ids.contains(id) => {}
        _ => match ids.first() {
            Some(first) => state.history.select(first.clone()),
            None => {
                state.history.account = None;
                state.history.view = None;
                return vec![];
            }
        },
    }
    reload(state, now_ms).into_iter().collect()
}

pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => step_account(state, 1, now_ms),
        KeyCode::Char('k') | KeyCode::Up => step_account(state, -1, now_ms),
        KeyCode::Char('[') => {
            state.history.range = state.history.range.step(-1);
            reload(state, now_ms).into_iter().collect()
        }
        KeyCode::Char(']') => {
            state.history.range = state.history.range.step(1);
            reload(state, now_ms).into_iter().collect()
        }
        KeyCode::Char('J') => scroll(state, 1, now_ms),
        KeyCode::Char('K') => scroll(state, -1, now_ms),
        _ => vec![],
    }
}

/// `J`/`K`: the windows move `by` one, stopping once the last one shows, or at the first.
fn scroll(state: &mut State, by: isize, now_ms: i64) -> Vec<Cmd> {
    let body = crate::view::body_at(state);
    let width = usize::from(body.width.saturating_sub(LIST_W.min(body.width) + 1));
    let heights: Vec<usize> = window_blocks(state, width, now_ms.div_euclid(1000))
        .iter()
        .map(Vec::len)
        .collect();
    let furthest = furthest_scroll(&heights, usize::from(body.height).saturating_sub(1));
    let h = &mut state.history;
    // From what shows: a resize or a new read may have left the stored scroll past the end.
    let from = h.from.min(furthest) as isize;
    h.from = (from + by).clamp(0, furthest as isize) as usize;
    vec![]
}

/// Moves to the account `by` rows away, stopping at either end, and reads its history.
fn step_account(state: &mut State, by: isize, now_ms: i64) -> Vec<Cmd> {
    let ids: Vec<AccountId> = accounts(state).iter().map(|v| v.row.id.clone()).collect();
    if ids.is_empty() {
        return vec![];
    }
    let at = state
        .history
        .account
        .as_ref()
        .and_then(|id| ids.iter().position(|i| i == id))
        .unwrap_or(0) as isize;
    let to = &ids[(at + by).clamp(0, ids.len() as isize - 1) as usize];
    if state.history.account.as_ref() == Some(to) {
        return vec![];
    }
    state.history.select(to.clone());
    reload(state, now_ms).into_iter().collect()
}

/// One bucket per column from `since_s` to `now_s`, each the highest `pct` sampled in it; an
/// empty bucket stays blank, and a reset shows as a drop (spec §5.4).
fn sparkline(samples: &[Sample], since_s: i64, now_s: i64, columns: usize) -> String {
    if columns == 0 {
        return String::new();
    }
    let span = (now_s - since_s).max(1);
    let mut peaks: Vec<Option<f64>> = vec![None; columns];
    for s in samples {
        if s.fetched_at < since_s || s.fetched_at > now_s {
            continue;
        }
        let i = (((s.fetched_at - since_s) as i128 * columns as i128) / span as i128) as usize;
        let slot = &mut peaks[i.min(columns - 1)];
        *slot = Some(slot.map_or(s.pct, |p| p.max(s.pct)));
    }
    peaks
        .into_iter()
        .map(|p| match p {
            Some(pct) => SPARK[(pct.clamp(0.0, 100.0) / 100.0 * 7.0).round() as usize],
            None => ' ',
        })
        .collect()
}

pub fn render(state: &State, frame: &mut Frame, area: Rect, now_ms: i64) {
    let now_s = now_ms.div_euclid(1000);
    let list = Rect::new(area.x, area.y, LIST_W.min(area.width), area.height);
    let right = Rect::new(
        area.x + list.width + 1,
        area.y,
        area.width.saturating_sub(list.width + 1),
        area.height,
    );
    frame.render_widget(
        Paragraph::new(account_list(state, usize::from(list.height))),
        list,
    );
    frame.render_widget(Paragraph::new(windows(state, right, now_s)), right);
}

/// ` ACCOUNTS`, then each account as `* 1 name`, the shown one in reverse video, under its
/// provider's ID when there are several (spec §5.1). A list longer than `rows` scrolls so that
/// the shown account stays in view.
fn account_list(state: &State, rows: usize) -> Vec<Line<'static>> {
    let w = usize::from(LIST_W);
    let head = Line::from(Span::styled(" ACCOUNTS", bold()));
    let groups: Vec<_> = state
        .accounts
        .snapshot()
        .map(|s| {
            s.providers
                .iter()
                .filter(|p| !p.accounts.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if groups.is_empty() {
        return vec![head, Line::from(format!(" {NO_ACCOUNTS}"))];
    }
    let several = groups.len() > 1;
    let mut lines = Vec::new();
    let mut shown_at = 0;
    for group in groups {
        if several {
            lines.push(Line::from(Span::styled(
                format!(" {}", group.provider),
                bold(),
            )));
        }
        for v in &group.accounts {
            let marker = if v.active { '*' } else { ' ' };
            let line = Line::from(pad(
                &format!(" {marker} {} {}", v.row.position, words::name(&v.row)),
                w,
            ));
            if state.history.account.as_ref() == Some(&v.row.id) {
                shown_at = lines.len();
                lines.push(line.style(selected()));
            } else {
                lines.push(line);
            }
        }
    }
    let room = rows.saturating_sub(1).max(1);
    let skip = (shown_at + 1).saturating_sub(room);
    std::iter::once(head)
        .chain(lines.into_iter().skip(skip))
        .collect()
}

/// The shown account's windows (`window_lines`), then the range. Windows that do not fit are
/// scrolled to (`J`/`K`), whole, which the range line says.
fn windows(state: &State, area: Rect, now_s: i64) -> Vec<Line<'static>> {
    let h = &state.history;
    let width = usize::from(area.width);
    let mut lines = Vec::new();
    let mut more = String::new();
    match (&h.account, &h.view) {
        (None, _) => lines.push(Line::from(NO_HISTORY)),
        (Some(_), None) => lines.push(Line::from(LOADING)),
        (Some(_), Some(view)) if view.windows.is_empty() => lines.push(Line::from(NO_HISTORY)),
        (Some(_), Some(_)) => {
            let blocks = window_blocks(state, width, now_s);
            let heights: Vec<usize> = blocks.iter().map(Vec::len).collect();
            let rows = usize::from(area.height).saturating_sub(1);
            let range = scrolled_windows(&heights, h.from, rows);
            if range != (0..blocks.len()) {
                let (from, to) = (range.start + 1, range.end);
                more = format!("  J K: windows {from}–{to} of {}", blocks.len());
            }
            for block in blocks.into_iter().take(range.end).skip(range.start) {
                lines.extend(block);
            }
        }
    }
    let footer_at = usize::from(area.height).saturating_sub(1);
    while lines.len() < footer_at {
        lines.push(Line::default());
    }
    lines.truncate(footer_at);
    lines.push(Line::from(fit(
        &format!("{}  [ ] range{more}", h.range.label()),
        width,
    )));
    lines
}

/// Each window's lines in a panel `width` columns wide. The label column is a third of the
/// width at most, so the sparkline, the latest pct and the projection always have room (R20).
fn window_blocks(state: &State, width: usize, now_s: i64) -> Vec<Vec<Line<'static>>> {
    let Some(view) = &state.history.view else {
        return vec![];
    };
    let label_w = view
        .windows
        .iter()
        .map(|w| width_of(&w.window.label))
        .max()
        .unwrap_or(0)
        .min(width / 3);
    let columns = width.saturating_sub(label_w + 2 + 2 + 4);
    view.windows
        .iter()
        .map(|hw| window_lines(state, hw, label_w, columns, width, now_s))
        .collect()
}

/// `5h  ▁▂▃…  9%`, then the rate and projection in `history`'s words (which say whether it
/// lasts to its reset) under the sparkline, going on to more lines there when they do not fit
/// `width` (R20). A label wider than its column takes rows of its own above: `LABEL_ROWS` at
/// most, the rest cut.
fn window_lines(
    state: &State,
    hw: &HistoryWindow,
    label_w: usize,
    columns: usize,
    width: usize,
    now_s: i64,
) -> Vec<Line<'static>> {
    let w = &hw.window;
    let since = state.history.since_s(now_s);
    let pct = w.pct.round() as i64;
    let style = severity_style(state.color, words::severity(pct));
    let mut lines: Vec<Line<'static>> = Vec::new();
    let label = if width_of(&w.label) > label_w {
        let rows = wrap_rows(&w.label, width, LABEL_ROWS);
        lines.extend(rows.into_iter().map(Line::from));
        ""
    } else {
        w.label.as_str()
    };
    lines.push(Line::from(vec![
        Span::raw(format!("{}  ", pad(label, label_w))),
        Span::raw(sparkline(&hw.samples, since, now_s, columns)),
        Span::styled(format!("  {:>4}", format!("{pct}%")), style),
    ]));
    let indent = label_w + 2;
    let under = vec![Span::raw(" ".repeat(indent))];
    lines.extend(flow(
        under,
        vec![words::projection_text(hw, now_s)],
        indent,
        width,
    ));
    lines
}
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-tui --lib screens::history`
Expected: PASS, 12 tests.

Run: `cargo test -p tagteam-tui`
Expected: PASS.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-tui/src/screens/history.rs crates/tagteam-tui/src/state.rs \
  crates/tagteam-tui/src/actions.rs
git commit -m "Show usage history in the TUI"
```

---

### Task 8: `EngineSource` and the timer refresh

Two halves. `EngineSource` replaces Task 3's stubs with the engine calls C3.2 names. And
`update` starts applying the lanes' results by generation, and refreshes on a timer (spec §6.2):
every 30 s it re-reads `accounts`, and `history` when History is open, and offers the shown
accounts to the collector, unless an action holds the actions lane.

**Readings of the spec this task commits to:**
- **`accounts`** is `list`'s view: `Engine::accounts(provider)` (`crates/tagteam-engine/src/views.rs:603-624`).
  Around it:
  - **`this`:** `shell_account()` (`views.rs:583-600`) when it is `Managed`.
  - **`in_run_shell`:** whether `run_shell()` is `Inside`.
  - **`read_at_s`:** `now_ms() / 1000`.
  - **`unmanaged_live`:** `status(P)` (`views.rs:626-640`) when it is `Unmanaged`. P is the
    provider the TUI shows, `--provider`'s, else the default, as `App::provider` chooses for
    `status`. `status` is asked only when P has no active managed account, since it re-reads
    P's view. A `status` that fails (an unreadable live identity) leaves it `None` rather than
    failing the views, as `list` shows no such login either.
  - **An unreadable run-shell marker** fails the read with `run-shell-unreadable`, which the
    banner shows. Task 12 refuses it before the TUI starts (spec §3).
- **`history`** is `Engine::history(account, None, since_s)` (`views.rs:648-744`): every relevant
  window.
- **`collect_due`** is `list`'s collection, `collect_usage(CollectMode::OnDemand { accounts })`
  (`crates/tagteam-engine/src/collect.rs:199`). With no accounts it returns at once and opens
  nothing, as `App::collect` does (`crates/tagteam/src/app.rs:1002-1018`). `fetched` counts the
  outcomes that changed what the views show (`Collected`, `collect.rs:64-84`):
  - `Recorded`: a reading was recorded;
  - `Failed { kind }`, unless `kind` is `error`: a failure was recorded (`error` means the
    collection ended with an error and recorded nothing);
  - `OverBudget`: the store recorded the refusal as an `over-budget` failure
    (`store/usage.rs:156-161`).

  `Ineligible`, `Dropped` and `Unsupported` recorded nothing.
- **Generations.** `Accounts`, `History` and `Collected` are applied only when their generation is
  the latest asked for; an older one is dropped (spec §4.3).
- **Failed reads (spec §10.4).** A failed `accounts` or `history` read keeps the last good data
  and sets the banner to the error's message. The next good read clears it, unless the TUI is
  shutting down. A failed collection sets the banner to `usage was not collected: <message>`,
  `App::collect`'s words, which wraps when it is long (Task 5's layout, R21). A collection's first warning is a toast, `warning: <warning>`, as the
  CLI prints it.
- **The timer.** On the first `Tick` at or after `refresh.next_ms` (30 s after `State::new`, then
  every 30 s):
  - `LoadAccounts`;
  - `LoadHistory`, when History is open;
  - `CollectDue` with `shown_ids()`, unless an action runs (spec §7.2) or no account is shown.

  Nothing is refreshed once the shutdown has begun.
- **A collection that fetched** (`fetched > 0`) triggers one more `LoadAccounts`, and
  `LoadHistory` when History is open (spec §6.2: "the views are re-read").
- **History follows the accounts.** When an `accounts` read arrives while History is open, and
  History has no account yet, or its account is gone, it moves to the first account and reads it
  (`screens::history::on_accounts`). This is how a TUI started on History loads it.
- **The first collection follows the first frame (R19).** Spec §6.1: "the TUI collects after
  drawing", not 30 s later. `update::drawn(state)`, which Task 11's runtime calls after each frame
  it draws, returns one `CollectDue` of the shown accounts the first time a frame shows any, and
  nothing once a collection has been asked for (`gens.collect` is no longer 0, whoever asked:
  `drawn`, the timer, or `r`). While an action runs it waits (spec §7.2), as does `loading…` or
  the empty state, which show no accounts.

**Where the source's tests live.** In `crates/tagteam-tui/tests/source.rs`, with an engine built
from `EngineConfig`'s public parts:
- `ClaudeCode` and `FakeAgent` over a `FakeKeychain`, a `KeychainVault` over the same keychain;
- `NoOracle`, `FakeClock`, `ScriptedHttp`, `FakeProcessProbe`, `ScriptedSpawner`;
- `Settings::default()`, `Env::for_test`.

The engine's own fixtures are not reachable from another crate: `Fx` lives in
`crates/tagteam-engine/tests/common`, an integration-test module, and `testutil` is
`#[cfg(test)]`. `crates/tagteam/tests` would gain nothing, since `app::build_engine` is private
and the engine would be assembled the same way there. No `test-hooks` feature is needed: no
crash point is used.

**Depends on:** 4, 6, 7

**Files:**
- Modify: `crates/tagteam-tui/Cargo.toml` (a `[dev-dependencies]` section, before `[lints]`), and
  `Cargo.lock` (cargo adds the dev-dependencies to `tagteam-tui`'s entry; no fetch is needed,
  they are workspace crates and crates already locked)
- Modify: `crates/tagteam-tui/src/source.rs` (Task 3's `impl Source for EngineSource`, its
  imports, and the `#[allow(dead_code)]` on `EngineSource.engine`)
- Modify: `crates/tagteam-tui/src/update.rs` (`update`'s data arms, a new `drawn` and
  `clear_read_error`, and `on_tick`; the `state` import)
- Modify: `crates/tagteam-tui/src/state.rs` (adds `State::collect_due`, after `load_history`)
- Modify: `crates/tagteam-tui/src/screens/history.rs` (adds `on_accounts`, after `on_enter`;
  `Screen` in the `state` import)
- Create: `crates/tagteam-tui/tests/source.rs`
- Test: `crates/tagteam-tui/tests/source.rs`; unit tests appended to `update.rs` and
  `screens/history.rs`

**Interfaces:**
- Consumes:
  - The engine: `Engine::{accounts, shell_account, status, history, collect_usage, run_shell,
    default_provider, now_ms}`, `views::{ShellAccount, StatusView}`,
    `collect::{CollectMode, Collected}`, `tagteam_provider::RunShell`.
  - Task 6: `AccountsState::{apply, shown_ids}`.
  - Task 7: `HistoryState::apply`, `screens::history::{reload, on_enter}`.
- Produces:
  - C3.2: `impl Source for EngineSource`, with no stub left.
  - C5's refresh behaviour in `update`, and C5's `update::drawn(state: &mut State) -> Vec<Cmd>`
    (R19), for Task 11.
  - Crate-private: `State::collect_due(&mut self, accounts: Vec<AccountId>) -> Cmd` (Task 9's
    `r`), and `screens::history::on_accounts(state, now_ms) -> Vec<Cmd>`.

**Spec:**
- spec §4.4: the `Source` methods and the engine operations behind them.
- spec §6.1: the first frame never waits on a fetch; the TUI collects after drawing.
- spec §6.2: every 30 s, `accounts` (and `history` when open) re-read, `collect_due` for the shown
  accounts, a re-read when a collection fetched.
- spec §7.2: the timer's `collect_due` is skipped while any action runs.
- spec §10.4: a failed view read keeps the last good data under a banner; the next refresh
  retries.
- spec §11, "`Source` over a real engine": several providers, a run shell, an unmanaged live
  login, a stale reading.
- core §8.3: `OnDemand` fetches only what is due; core §13.4: `history` reads only.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-tui/Cargo.toml`, before `[lints]`:

```toml
[dev-dependencies]
tagteam-cc.workspace = true
tagteam-fake.workspace = true
serde_json.workspace = true
tempfile.workspace = true
```

Create `crates/tagteam-tui/tests/source.rs`:

```rust
//! `EngineSource` over a real engine (spec §4.4, §11 "Source over a real engine"): a fixture
//! home with Claude Code and FakeAgent registered, a fake Keychain, a scripted HTTP port and a
//! fake clock. Nothing reaches the real HOME, the network or the Keychain (core §15.1).

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tagteam_cc::ClaudeCode;
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::live::Platform;
use tagteam_core::{AccountId, CLAUDE_CODE, PollBudget, PollPlan, ProviderId, Window, WindowKind};
use tagteam_engine::oracle::NoOracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::{Eligibility, NewAccount, Reserve, Store};
use tagteam_engine::vault::{KeychainVault, SERVICE, Vault};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_fake::{FAKE_AGENT, FakeAgent};
use tagteam_provider::http::Method;
use tagteam_provider::liveness::FakeProcessProbe;
use tagteam_provider::process::ScriptedSpawner;
use tagteam_provider::{
    Env, FakeClock, FakeKeychain, Identity, ProfileMarker, RunShell, ScriptedHttp,
};
use tagteam_tui::source::{EngineSource, Source};

/// The fixture clock's start, in ms: every reading below is placed relative to it.
const NOW_MS: i64 = 1_790_000_000_000;
const NOW_S: i64 = NOW_MS / 1000;

struct Home {
    _dir: tempfile::TempDir,
    env: Env,
    kc: Arc<FakeKeychain>,
    clock: Arc<FakeClock>,
    http: Arc<ScriptedHttp>,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        fs::create_dir_all(env.home.join(".claude")).unwrap();
        fs::write(env.home.join(".claude.json"), "{\n  \"userID\": \"u\"\n}\n").unwrap();
        Home {
            _dir: dir,
            env,
            kc: Arc::new(FakeKeychain::new()),
            clock: Arc::new(FakeClock::new(NOW_MS)),
            http: Arc::new(ScriptedHttp::new()),
        }
    }

    /// An engine as the binary builds one for `list` (`build_engine`), over the fixture's ports.
    fn engine_in(&self, run_shell: RunShell) -> Engine {
        Engine::new(EngineConfig {
            env: self.env.clone(),
            registry: ProviderRegistry::new()
                .with(Arc::new(ClaudeCode::new(self.kc.clone(), Platform::MacOs)))
                .with(Arc::new(FakeAgent::new())),
            vault: Vault::new(Box::new(KeychainVault::new(self.kc.clone()))),
            oracle: Arc::new(NoOracle),
            clock: self.clock.clone(),
            http: self.http.clone(),
            default_provider: ProviderId::new(CLAUDE_CODE),
            settings: Settings::default(),
            process: Arc::new(FakeProcessProbe::new()),
            spawner: Arc::new(ScriptedSpawner::new()),
            run_shell,
        })
    }

    fn source(&self) -> EngineSource {
        EngineSource::new(self.engine_in(RunShell::Outside))
    }

    fn db(&self) -> PathBuf {
        self.env.data_dir().join("tagteam.db")
    }

    fn store(&self) -> Store {
        Store::open(&self.db()).unwrap()
    }

    /// `email`'s account at `position` under `provider`, as `add` stores it.
    fn add(&self, provider: &str, position: u32, email: &str) -> AccountId {
        let id = AccountId::from_string(format!("{provider}-{position}"));
        self.store()
            .insert_account(&NewAccount {
                id: &id,
                provider: &ProviderId::new(provider),
                position,
                identity_key: &format!("{email}\n"),
                identity: &Identity {
                    label: email.into(),
                    email: Some(email.into()),
                    org_uuid: String::new(),
                    org_name: None,
                    account_uuid: None,
                    raw: json!({"emailAddress": email}),
                },
                kind: "oauth",
                alias: None,
                login_expires_at: None,
                added_at: 1,
            })
            .unwrap();
        id
    }

    /// What `claude /login` as `email` leaves in `~/.claude.json`.
    fn login(&self, email: &str) {
        let config = json!({
            "userID": "u",
            "oauthAccount": {"emailAddress": email, "organizationUuid": "", "accountUuid": format!("uuid-{email}")},
        });
        fs::write(
            self.env.home.join(".claude.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();
    }

    /// A reading of `id` taken `ago_s` before now, through the store's own write path.
    fn reading(&self, id: &AccountId, ago_s: i64, pct: f64) {
        let store = self.store();
        let row = store.account(id).unwrap().unwrap();
        let at_s = NOW_S - ago_s;
        let Reserve::Reserved(r) = store
            .reserve_usage(
                &row,
                at_s * 1000,
                Eligibility::OnDemand,
                &PollBudget::STANDARD,
            )
            .unwrap()
        else {
            panic!("no reservation for a reading {ago_s} s ago");
        };
        let window = Window {
            key: "5h".into(),
            label: "5h".into(),
            kind: WindowKind::Short,
            pct,
            resets_at: Some(NOW_S + 3_600),
            period_s: None,
            detail: None,
        };
        let plan = PollPlan {
            interval_s: 300,
            next_poll_at: at_s + 300,
        };
        assert!(store.record_usage(&r, &[window], at_s, &plan, 180).unwrap());
    }

    /// A usable stored credential for `id`, and one usage reply queued for it.
    fn fetchable(&self, id: &AccountId) {
        let credential = json!({"claudeAiOauth": {
            "accessToken": format!("at-{id}"),
            "refreshToken": format!("rt-{id}"),
            "expiresAt": NOW_MS + 3_600_000,
            "refreshTokenExpiresAt": 1_797_000_000_000i64,
            "scopes": ["user:inference", "user:profile"],
            "subscriptionType": "max",
        }});
        self.kc
            .put(SERVICE, id.as_str(), credential.to_string().as_bytes());
        let recorded: Value = serde_json::from_str(include_str!(
            "../../tagteam-cc/tests/fixtures/endpoints/usage-200.json"
        ))
        .unwrap();
        self.http.push_json(
            Method::Get,
            &Endpoints::production().usage,
            200,
            recorded["body"].clone(),
        );
    }
}

#[test]
fn accounts_is_lists_view_read_at_the_engine_clock() {
    let home = Home::new();
    let a = home.add(CLAUDE_CODE, 1, "a@x.co");
    let b = home.add(CLAUDE_CODE, 2, "b@x.co");
    home.login("a@x.co");
    let snap = home.source().accounts(None).unwrap();
    assert_eq!(snap.providers.len(), 1);
    let p = &snap.providers[0];
    assert_eq!(p.provider, ProviderId::new(CLAUDE_CODE));
    assert_eq!(p.active_position, Some(1));
    let ids: Vec<&AccountId> = p.accounts.iter().map(|v| &v.row.id).collect();
    assert_eq!(ids, [&a, &b]);
    assert_eq!(
        (
            snap.this,
            snap.unmanaged_live,
            snap.in_run_shell,
            snap.read_at_s
        ),
        (None, None, false, NOW_S)
    );
}

#[test]
fn a_live_login_tagteam_does_not_manage_is_named() {
    let home = Home::new();
    home.add(CLAUDE_CODE, 1, "a@x.co");
    home.login("s@x.co");
    let snap = home.source().accounts(None).unwrap();
    assert_eq!(snap.unmanaged_live.as_deref(), Some("s@x.co"));
    assert_eq!(snap.providers[0].active_position, None);
}

#[test]
fn no_store_is_the_default_provider_with_no_accounts_and_creates_nothing() {
    let home = Home::new();
    let snap = home.source().accounts(None).unwrap();
    assert_eq!(snap.providers.len(), 1);
    assert_eq!(snap.providers[0].provider, ProviderId::new(CLAUDE_CODE));
    assert!(snap.providers[0].accounts.is_empty());
    assert!(!home.db().exists(), "a read never creates the store");
    let summary = home.source().collect_due(&[]).unwrap();
    assert_eq!((summary.fetched, summary.warnings.len()), (0, 0));
    assert!(!home.db().exists());
}

#[test]
fn several_providers_are_listed_and_a_provider_narrows_them() {
    let home = Home::new();
    home.add(CLAUDE_CODE, 1, "a@x.co");
    home.add(FAKE_AGENT, 1, "f@x.co");
    let source = home.source();
    let all = source.accounts(None).unwrap();
    let names: Vec<&str> = all.providers.iter().map(|p| p.provider.as_str()).collect();
    assert_eq!(names, [CLAUDE_CODE, FAKE_AGENT]);
    let fake = source.accounts(Some(&ProviderId::new(FAKE_AGENT))).unwrap();
    assert_eq!(fake.providers.len(), 1);
    assert_eq!(fake.providers[0].accounts[0].row.position, 1);
}

#[test]
fn a_run_shell_names_its_own_account() {
    let home = Home::new();
    home.add(CLAUDE_CODE, 1, "a@x.co");
    let b = home.add(CLAUDE_CODE, 2, "b@x.co");
    let run_shell = RunShell::Inside {
        profile: home.env.home.join("profile-b"),
        marker: ProfileMarker {
            provider: ProviderId::new(CLAUDE_CODE),
            account_id: b.clone(),
            config_dir: "profile-b".into(),
            outer: Value::Null,
        },
    };
    let snap = EngineSource::new(home.engine_in(run_shell))
        .accounts(None)
        .unwrap();
    assert_eq!(snap.this, Some(b));
    assert!(snap.in_run_shell);
}

#[test]
fn a_stale_reading_is_shown_with_its_age_and_not_as_current() {
    let home = Home::new();
    let a = home.add(CLAUDE_CODE, 1, "a@x.co");
    home.reading(&a, 3_600, 50.0);
    let snap = home.source().accounts(None).unwrap();
    let usage = &snap.providers[0].accounts[0].usage;
    assert_eq!(usage.age_s, Some(3_600));
    assert!(
        !usage.decision_grade,
        "an hour old is past §8.4's five minutes"
    );
    assert_eq!(usage.windows.as_ref().unwrap()[0].0.pct, 50.0);
}

#[test]
fn history_is_the_samples_since_the_range_starts() {
    let home = Home::new();
    let a = home.add(CLAUDE_CODE, 1, "a@x.co");
    home.reading(&a, 3 * 86_400, 40.0);
    home.reading(&a, 3_600, 60.0);
    let view = home.source().history(&a, NOW_S - 86_400).unwrap();
    assert_eq!(view.account.row.id, a);
    assert_eq!(view.windows.len(), 1);
    let pcts: Vec<f64> = view.windows[0].samples.iter().map(|s| s.pct).collect();
    assert_eq!(pcts, [60.0], "only the day's sample");
    let week = home.source().history(&a, NOW_S - 7 * 86_400).unwrap();
    assert_eq!(week.windows[0].samples.len(), 2);
}

#[test]
fn a_failed_read_carries_the_engines_kind_and_words() {
    let home = Home::new();
    home.add(CLAUDE_CODE, 1, "a@x.co");
    let e = home
        .source()
        .history(&AccountId::from_string("gone"), 0)
        .unwrap_err();
    assert_eq!(e.kind, "no-such-account");
    assert_eq!(e.message, "no account matches \"gone\"");
}

#[test]
fn collect_due_counts_the_readings_and_failures_it_recorded() {
    let home = Home::new();
    let read = home.add(CLAUDE_CODE, 1, "a@x.co");
    let missing = home.add(CLAUDE_CODE, 2, "b@x.co");
    let fresh = home.add(CLAUDE_CODE, 3, "c@x.co");
    home.fetchable(&read);
    home.reading(&fresh, 10, 5.0);
    let source = home.source();
    let summary = source
        .collect_due(&[read.clone(), missing.clone(), fresh.clone()])
        .unwrap();
    // `read` recorded a reading, `missing` a `vault-absent` failure; `fresh` was not due.
    assert_eq!(summary.fetched, 2, "{summary:?}");
    let snap = source.accounts(None).unwrap();
    let a = &snap.providers[0].accounts[0];
    assert_eq!(a.usage.fetched_at, Some(NOW_S));
    // Nothing is due a moment later: a reading is fresh, a failure backs off.
    let again = source.collect_due(&[read, missing, fresh]).unwrap();
    assert_eq!(again.fetched, 0);
}
```

`fetchable` queues the usage endpoint's recorded reply on the scripted port, as the engine's
`usage_fixture` reads it from
`crates/tagteam-cc/tests/fixtures/endpoints/usage-200.json`. It also stores a credential whose
access token is valid for an hour of the fake clock, so the inactive account's fetch needs no
refresh.

In `crates/tagteam-tui/src/update.rs`'s test module, add `use crate::source::{CollectSummary,
SourceError};` after `use super::*;`, and append after `start_reads_the_accounts_first`:

```rust
    fn accounts_msg(r#gen: u64, n: u32) -> Msg {
        Msg::Accounts {
            r#gen,
            result: Ok(snapshot(many(n))),
        }
    }

    fn failed(message: &str) -> SourceError {
        SourceError {
            kind: "store".into(),
            message: message.into(),
        }
    }

    #[test]
    fn a_result_from_an_older_generation_is_dropped() {
        let mut state = fresh(Screen::Accounts);
        state.start();
        let newer = state.load_accounts();
        assert_eq!(newer, Cmd::LoadAccounts { r#gen: 2 });
        update(&mut state, accounts_msg(1, 3), NOW_MS);
        assert!(state.accounts.snapshot().is_none(), "gen 1 is stale");
        update(&mut state, accounts_msg(2, 2), NOW_MS);
        assert_eq!(state.accounts.shown_ids(), [id(1), id(2)]);

        state.history.select(id(1));
        let Cmd::LoadHistory { r#gen, .. } = state.load_history(id(1), 0) else {
            unreachable!()
        };
        state.load_history(id(2), 0);
        update(
            &mut state,
            Msg::History {
                r#gen,
                result: Ok(history_of(
                    account(1, "a1@x.co", read(60, 1.0, 1.0, false)),
                    vec![],
                )),
            },
            NOW_MS,
        );
        update(&mut state, ch('3'), NOW_MS);
        assert!(
            draw(&state, 80, 24)[1].ends_with("loading…"),
            "the stale history was dropped"
        );

        let Cmd::CollectDue { r#gen, .. } = state.collect_due(vec![id(1)]) else {
            unreachable!()
        };
        state.collect_due(vec![id(1)]);
        let stale = Msg::Collected {
            r#gen,
            result: Ok(CollectSummary {
                fetched: 1,
                warnings: vec![],
            }),
        };
        assert!(update(&mut state, stale, NOW_MS).is_empty());
    }

    #[test]
    fn every_30_seconds_the_views_are_read_and_the_shown_accounts_offered_to_the_collector() {
        let mut state = fresh(Screen::Accounts);
        state.start();
        update(&mut state, accounts_msg(1, 2), NOW_MS);
        assert!(update(&mut state, Msg::Tick, NOW_MS + 29_999).is_empty());
        assert_eq!(
            update(&mut state, Msg::Tick, NOW_MS + 30_000),
            [
                Cmd::LoadAccounts { r#gen: 2 },
                Cmd::CollectDue {
                    r#gen: 1,
                    accounts: vec![id(1), id(2)]
                }
            ]
        );
        assert!(update(&mut state, Msg::Tick, NOW_MS + 59_999).is_empty());
        update(&mut state, ch('3'), NOW_MS + 59_999);
        let cmds = update(&mut state, Msg::Tick, NOW_MS + 60_000);
        assert!(
            matches!(
                cmds.as_slice(),
                [
                    Cmd::LoadAccounts { .. },
                    Cmd::LoadHistory { account, .. },
                    Cmd::CollectDue { .. }
                ] if *account == id(1)
            ),
            "{cmds:?}"
        );
    }

    #[test]
    fn the_timer_collects_nothing_while_an_action_runs_or_the_tui_quits() {
        let mut state = fresh(Screen::Accounts);
        state.start();
        update(&mut state, accounts_msg(1, 2), NOW_MS);
        state.running = Some(crate::host::Action::AddLive { provider: cc() });
        assert_eq!(
            update(&mut state, Msg::Tick, NOW_MS + 30_000),
            [Cmd::LoadAccounts { r#gen: 2 }]
        );
        state.running = None;
        state.quitting = Some(0);
        assert!(update(&mut state, Msg::Tick, NOW_MS + 60_000).is_empty());
    }

    #[test]
    fn a_collection_that_fetched_reads_the_views_once_more() {
        let mut state = fresh(Screen::History);
        state.start();
        update(&mut state, accounts_msg(1, 1), NOW_MS);
        let Cmd::CollectDue { r#gen, .. } = state.collect_due(vec![id(1)]) else {
            unreachable!()
        };
        let collected = |fetched| Msg::Collected {
            r#gen,
            result: Ok(CollectSummary {
                fetched,
                warnings: vec![],
            }),
        };
        assert!(update(&mut state, collected(0), NOW_MS).is_empty());
        let cmds = update(&mut state, collected(1), NOW_MS);
        assert!(
            matches!(
                cmds.as_slice(),
                [Cmd::LoadAccounts { r#gen: 2 }, Cmd::LoadHistory { .. }]
            ),
            "{cmds:?}"
        );
    }

    #[test]
    fn a_failed_read_keeps_the_last_good_data_under_a_banner() {
        let mut state = fresh(Screen::Accounts);
        state.start();
        update(&mut state, accounts_msg(1, 2), NOW_MS);
        let Cmd::LoadAccounts { r#gen } = state.load_accounts() else {
            unreachable!()
        };
        update(
            &mut state,
            Msg::Accounts {
                r#gen,
                result: Err(failed("the store is locked")),
            },
            NOW_MS,
        );
        assert_eq!(state.accounts.shown_ids(), [id(1), id(2)]);
        let drawn = draw(&state, 80, 24);
        assert_eq!(drawn[1], " the store is locked");
        assert!(drawn[3].contains("a1@x.co"), "{drawn:?}");
        // The next good read clears it.
        update(&mut state, accounts_msg(r#gen, 2), NOW_MS);
        assert_eq!(state.banner, None);
    }

    #[test]
    fn a_failed_collection_says_so_and_a_warning_is_a_toast() {
        let mut state = fresh(Screen::Accounts);
        let Cmd::CollectDue { r#gen, .. } = state.collect_due(vec![id(1)]) else {
            unreachable!()
        };
        update(
            &mut state,
            Msg::Collected {
                r#gen,
                result: Err(failed("the store is locked")),
            },
            NOW_MS,
        );
        assert_eq!(
            state.banner.as_deref(),
            Some("usage was not collected: the store is locked")
        );
        update(
            &mut state,
            Msg::Collected {
                r#gen,
                result: Ok(CollectSummary {
                    fetched: 0,
                    warnings: vec!["usage for a@x.co (position 1) was not collected: x".into()],
                }),
            },
            NOW_MS,
        );
        assert_eq!(
            state.toast.as_ref().unwrap().text,
            "warning: usage for a@x.co (position 1) was not collected: x"
        );
    }

    #[test]
    fn the_first_frame_that_shows_accounts_asks_for_one_collection() {
        // Spec §6.1: the TUI collects after drawing, not a timer's 30 s later.
        let mut state = fresh(Screen::Accounts);
        state.start();
        assert!(drawn(&mut state).is_empty(), "`loading…` shows no accounts");
        update(&mut state, accounts_msg(1, 0), NOW_MS);
        assert!(drawn(&mut state).is_empty(), "nor does the empty state");
        let Cmd::LoadAccounts { r#gen } = state.load_accounts() else {
            unreachable!()
        };
        update(&mut state, accounts_msg(r#gen, 2), NOW_MS);
        state.running = Some(crate::host::Action::AddLive { provider: cc() });
        assert!(
            drawn(&mut state).is_empty(),
            "an action holds the actions lane (spec §7.2)"
        );
        state.running = None;
        assert_eq!(
            drawn(&mut state),
            [Cmd::CollectDue {
                r#gen: 1,
                accounts: vec![id(1), id(2)]
            }]
        );
        assert!(drawn(&mut state).is_empty(), "once");
        // A collection asked for before any frame showed accounts (`r`) is the first one.
        let mut state = fresh(Screen::Accounts);
        state.collect_due(vec![id(1)]);
        update(&mut state, accounts_msg(0, 1), NOW_MS);
        assert!(drawn(&mut state).is_empty());
    }

    #[test]
    fn a_long_collection_error_shows_in_full_at_80_columns() {
        let mut state = fresh(Screen::Accounts);
        let Cmd::CollectDue { r#gen, .. } = state.collect_due(vec![id(1)]) else {
            unreachable!()
        };
        let message = "the usage endpoint could not be reached for a@x.co (position 1): connection refused by 127.0.0.1:9";
        update(
            &mut state,
            Msg::Collected {
                r#gen,
                result: Err(failed(message)),
            },
            NOW_MS,
        );
        let lines = draw(&state, 80, 24);
        let shown: String = lines[1..3]
            .iter()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(shown, format!("usage was not collected: {message}"));
        assert_eq!(lines[3], " loading…");
    }

    #[test]
    fn the_first_accounts_read_on_history_reads_the_first_accounts_history() {
        let mut state = fresh(Screen::History);
        state.start();
        assert_eq!(
            update(&mut state, accounts_msg(1, 2), NOW_MS),
            [Cmd::LoadHistory {
                r#gen: 1,
                account: id(1),
                since_s: NOW_S - 7 * 86_400,
            }]
        );
    }
```

In `crates/tagteam-tui/src/screens/history.rs`'s test module, append:

```rust
    #[test]
    fn an_accounts_read_moves_history_off_an_account_that_is_gone() {
        let mut state = on_history(many(2));
        let week = NOW_S - 7 * 86_400;
        assert_eq!(
            on_accounts(&mut state, NOW_MS),
            [Cmd::LoadHistory {
                r#gen: 1,
                account: id(1),
                since_s: week,
            }],
            "none chosen yet: the first"
        );
        state
            .history
            .apply(view_of(account(1, "a1@x.co", read(60, 1.0, 1.0, false))));
        assert!(
            on_accounts(&mut state, NOW_MS).is_empty(),
            "still there, already read"
        );
        state.history.select(id(2));
        state
            .history
            .apply(view_of(account(2, "a2@x.co", read(60, 1.0, 1.0, false))));
        state.accounts.apply(snapshot(many(1)));
        assert_eq!(
            on_accounts(&mut state, NOW_MS),
            [Cmd::LoadHistory {
                r#gen: 2,
                account: id(1),
                since_s: week,
            }]
        );
        assert_eq!(state.history.account(), Some(&id(1)));
        state.accounts.apply(snapshot(vec![]));
        assert!(on_accounts(&mut state, NOW_MS).is_empty());
        assert_eq!(state.history.account(), None);
        state.screen = Screen::Accounts;
        state.accounts.apply(snapshot(many(1)));
        assert!(
            on_accounts(&mut state, NOW_MS).is_empty(),
            "only while History is open"
        );
    }
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-tui --test source`
Expected: FAIL, all 9 tests, each panicking with `not implemented` at Task 3's stubs in
`crates/tagteam-tui/src/source.rs`.

Run: `cargo test -p tagteam-tui --lib`
Expected: the build fails with E0599 (no method named `collect_due` found for struct `State`)
and E0425 (cannot find functions `on_accounts` and `drawn`).

- [ ] **Step 3: Implement**

In `crates/tagteam-tui/src/source.rs`, the imports become:

```rust
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::Engine;
use tagteam_engine::collect::{CollectMode, Collected};
use tagteam_engine::error::EngineError;
use tagteam_engine::views::{HistoryView, ProviderAccounts, ShellAccount, StatusView};
use tagteam_provider::RunShell;
```

Delete the `#[allow(dead_code)]` above `EngineSource`'s `engine` field (Task 3's): the methods
read it now. Task 3's `impl Source for EngineSource { … }` is replaced by:

```rust
impl Source for EngineSource {
    /// `list`'s view (`Engine::accounts`), with what the TUI shows around it: the run shell's
    /// own account (`shell_account`), the live login when tagteam does not manage it
    /// (`status`), whether this is a run shell, and the engine clock.
    fn accounts(&self, provider: Option<&ProviderId>) -> Result<AccountsSnapshot, SourceError> {
        let engine = &self.engine;
        let providers = engine.accounts(provider)?;
        let this = match engine.shell_account()? {
            ShellAccount::Managed(row) => Some(row.id),
            ShellAccount::NotInShell | ShellAccount::Unmanaged => None,
        };
        // Only a provider with no managed account active can have an unmanaged live login;
        // asking `status` then reads its view once more. One it cannot read is no reason to
        // fail the views: `list` shows no such login either.
        let live = provider.unwrap_or(engine.default_provider());
        let managed = providers
            .iter()
            .any(|p| &p.provider == live && p.active_position.is_some());
        let unmanaged_live = match managed {
            true => None,
            false => match engine.status(live) {
                Ok(StatusView::Unmanaged { email }) => Some(email),
                Ok(StatusView::NoLogin | StatusView::Managed { .. }) | Err(_) => None,
            },
        };
        Ok(AccountsSnapshot {
            providers,
            this,
            unmanaged_live,
            in_run_shell: matches!(engine.run_shell(), RunShell::Inside { .. }),
            read_at_s: engine.now_ms().div_euclid(1000),
        })
    }

    /// `history`'s view (core §13.4) of every relevant window since `since_s`.
    fn history(&self, account: &AccountId, since_s: i64) -> Result<HistoryView, SourceError> {
        Ok(self.engine.history(account, None, since_s)?)
    }

    /// `list`'s on-demand collection (core §8.3): the collector fetches only what is due.
    /// `fetched` counts the accounts whose stored usage changed.
    fn collect_due(&self, accounts: &[AccountId]) -> Result<CollectSummary, SourceError> {
        if accounts.is_empty() {
            return Ok(CollectSummary {
                fetched: 0,
                warnings: vec![],
            });
        }
        let report = self.engine.collect_usage(CollectMode::OnDemand {
            accounts: accounts.to_vec(),
        })?;
        Ok(CollectSummary {
            fetched: report.outcomes.iter().filter(|(_, c)| recorded(c)).count(),
            warnings: report.warnings,
        })
    }
}

/// Whether one account's collection changed what the views show (`Collected`): a reading was
/// recorded (`Recorded`), or a failure was (`Failed`, and `OverBudget`, which the store records
/// as an `over-budget` failure). `Failed { kind: "error" }` recorded nothing, nor did
/// `Ineligible`, `Dropped` or `Unsupported`.
fn recorded(c: &Collected) -> bool {
    match c {
        Collected::Recorded | Collected::OverBudget { .. } => true,
        Collected::Failed { kind } => kind != "error",
        Collected::Ineligible(_) | Collected::Dropped | Collected::Unsupported => false,
    }
}
```

In `crates/tagteam-tui/src/state.rs`, add after `load_history`:

```rust
    /// A collection of `accounts` (spec §6.2), under the next generation.
    pub(crate) fn collect_due(&mut self, accounts: Vec<AccountId>) -> Cmd {
        self.gens.collect += 1;
        Cmd::CollectDue {
            r#gen: self.gens.collect,
            accounts,
        }
    }
```

In `crates/tagteam-tui/src/screens/history.rs`, change the `state` import to
`use crate::state::{Screen, State};`, and add after `on_enter`:

```rust
/// A new `accounts` read: while History is open, an account that is gone, or none chosen yet,
/// moves it to the first account.
pub(crate) fn on_accounts(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.screen != Screen::History {
        return vec![];
    }
    let present = state
        .history
        .account
        .as_ref()
        .is_some_and(|id| accounts(state).iter().any(|v| &v.row.id == id));
    if present && state.history.view.is_some() {
        return vec![];
    }
    on_enter(state, now_ms)
}
```

In `crates/tagteam-tui/src/update.rs`, change the `state` import to
`use crate::state::{REFRESH_MS, Screen, State};`. Then replace `update` with the following,
which also adds `drawn` and `clear_read_error` after it:

```rust
pub fn update(state: &mut State, msg: Msg, now_ms: i64) -> Vec<Cmd> {
    match msg {
        Msg::Key(key) => {
            if key.kind == KeyEventKind::Release {
                return vec![];
            }
            state.dirty = true;
            on_key(state, key, now_ms)
        }
        Msg::Resize { width, height } => {
            state.size = (width, height);
            state.dirty = true;
            vec![]
        }
        Msg::Tick => on_tick(state, now_ms),
        Msg::Accounts { r#gen, result } => {
            if r#gen != state.gens.accounts {
                return vec![];
            }
            state.dirty = true;
            match result {
                Ok(snapshot) => {
                    state.accounts.apply(snapshot);
                    clear_read_error(state);
                    screens::history::on_accounts(state, now_ms)
                }
                Err(e) => {
                    state.banner = Some(e.message);
                    vec![]
                }
            }
        }
        Msg::History { r#gen, result } => {
            if r#gen != state.gens.history {
                return vec![];
            }
            state.dirty = true;
            match result {
                Ok(view) => {
                    state.history.apply(view);
                    clear_read_error(state);
                }
                Err(e) => state.banner = Some(e.message),
            }
            vec![]
        }
        Msg::Collected { r#gen, result } => {
            if r#gen != state.gens.collect {
                return vec![];
            }
            state.dirty = true;
            match result {
                Ok(summary) => {
                    if let Some(first) = summary.warnings.first() {
                        state.toast(format!("warning: {first}"), now_ms);
                    }
                    if summary.fetched == 0 {
                        return vec![];
                    }
                    // Spec §6.2: a collection that fetched is read back at once.
                    let mut cmds = vec![state.load_accounts()];
                    if state.screen == Screen::History {
                        cmds.extend(screens::history::reload(state, now_ms));
                    }
                    cmds
                }
                Err(e) => {
                    state.banner = Some(format!("usage was not collected: {}", e.message));
                    vec![]
                }
            }
        }
        Msg::Prompt(prompt) => {
            state.dirty = true;
            actions::on_prompt(state, prompt);
            vec![]
        }
        Msg::ActionDone { action, outcome } => {
            state.dirty = true;
            actions::on_done(state, action, outcome, now_ms)
        }
        Msg::LaneDied { lane } => {
            state.dirty = true;
            state.banner = Some(format!("the {lane} lane stopped unexpectedly"));
            if state.quitting.is_some() {
                return vec![];
            }
            quit(state, 1)
        }
    }
}

/// Spec §6.1: the TUI collects after drawing. The runtime calls this after each frame it
/// draws. The first frame that shows accounts asks for the first collection of them, once no
/// action holds the actions lane (spec §7.2); until a collection has been asked for,
/// `gens.collect` is 0. The timer collects from then on (§6.2).
pub fn drawn(state: &mut State) -> Vec<Cmd> {
    if state.gens.collect > 0 || state.quitting.is_some() || state.running.is_some() {
        return vec![];
    }
    let shown = state.accounts.shown_ids();
    if shown.is_empty() {
        return vec![];
    }
    vec![state.collect_due(shown)]
}

/// A read that succeeds clears the banner a failed one left (spec §10.4); a shutdown's never.
fn clear_read_error(state: &mut State) {
    if state.quitting.is_none() {
        state.banner = None;
    }
}
```

and replace `on_tick` with:

```rust
fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.toast.as_ref().is_some_and(|t| t.until_ms <= now_ms) {
        state.toast = None;
        state.dirty = true;
    }
    // Countdowns change at most once a second; the spinner turns while anything runs.
    let now_s = now_ms.div_euclid(1000);
    if now_s != state.shown_s || state.running.is_some() || state.quitting.is_some() {
        state.shown_s = now_s;
        state.dirty = true;
    }
    if state.quitting.is_some() || now_ms < state.refresh.next_ms {
        return vec![];
    }
    // Spec §6.2: every 30 s the views are re-read, and the shown accounts offered to the
    // collector, unless an action holds the actions lane (spec §7.2).
    state.refresh.next_ms = now_ms + REFRESH_MS;
    let mut cmds = vec![state.load_accounts()];
    if state.screen == Screen::History {
        cmds.extend(screens::history::reload(state, now_ms));
    }
    let shown = state.accounts.shown_ids();
    if state.running.is_none() && !shown.is_empty() {
        cmds.push(state.collect_due(shown));
    }
    cmds
}
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-tui --test source`
Expected: PASS, 9 tests.

Run: `cargo test -p tagteam-tui`
Expected: PASS: 88 library tests and 9 in `source`.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add Cargo.lock crates/tagteam-tui/Cargo.toml crates/tagteam-tui/src/source.rs \
  crates/tagteam-tui/src/update.rs crates/tagteam-tui/src/state.rs \
  crates/tagteam-tui/src/screens/history.rs crates/tagteam-tui/tests/source.rs
git commit -m "Read the TUI's views through the engine and refresh them on a timer"
```

---

### Task 9: Actions, dialogs and panels in the TUI

The actions (spec §7.1) run the CLI's own commands through `Host` (Task 10). This task gives them
their keys, the dialogs the TUI adds, the panels that show their outcome, and `ChannelAsk`, the
`Ask` that turns a CLI prompt on the actions lane into a dialog on the UI thread.

**Readings of the spec this task commits to:**
- **Keys (spec §5.6, §7.1).** On Accounts, each acts on `AccountsState::selected()`, the account
  as the TUI last read it, so the `Action` names its ID, provider and position (spec §4.5):
  - `s` is `Switch`; `b` and `n` are `SwitchBy` with `Best` or `NextAvailable`, for the
    selected account's provider;
  - `e` is `Enable` when the account is disabled, else `Disable`;
  - `a` opens the alias dialog, `m` the move dialog, `x` the remove confirmation;
  - `R` is `Run`, always suspended (`Cmd::Suspend`).

  Everywhere: `A` is `AddLive` for `--provider`'s provider, else the default (the accounts view
  lists it first, `Engine::accounts`), and does nothing before the first read; `D` is
  `Doctor { provider: --provider's, online: false }`; `r` re-reads the views and offers the shown
  accounts to the collector at once.
- **Empty selection (Review Focus 3).** With no account selected, `s b n e a m x R` do nothing at
  all: no command, no toast, no dialog. `A`, `D` and `?` still work, and `r` only re-reads.
- **One at a time (spec §7.2).** An action key while one runs shows `busy: <action>` as a toast and
  is dropped; this includes `r` and the dialog keys, which check before opening. A suspended
  action counts as running. `running` is set when an action starts and cleared by its outcome.
- **Dialogs (spec §5.5, §7.1, §7.2).**
  - **Remove's question** is exactly `Remove <position> <email>? Its stored login is deleted;
    Claude Code is not changed.`, defaulting to no: `Enter` takes the highlighted answer, `y`
    and `n` answer, `Esc` is no.
  - **The alias dialog** starts with the current alias, takes any key (up to 64 characters), and
    `Enter` with nothing in it (or only spaces) clears the alias. An alias that begins with `-`
    goes through C3.1's `--`.
  - **The move dialog** takes digits only (up to nine); `Enter` with no number does nothing.
  - **A CLI prompt** opens a confirm dialog (with the CLI's default highlighted) or a choose
    dialog (`↑`/`↓`/`j`/`k`, `1`–`9`, `Enter`), bound to its reply sender. A choice wider than
    the dialog wraps under its own text, three rows at most, and the selected one is reversed
    on every row it takes; the list scrolls by whole choices to keep it in view. A question
    takes a third of the terminal's rows at most, so the buttons, choices or input always show
    (R21). `Esc` declines:
    `Confirm(false)` or `Choose(None)`. During a shutdown a prompt is dropped unshown, which
    declines it.
- **Outcomes (spec §7.2, §5.5).**
  - **`NeedsTerminal`** returns `Cmd::Suspend(action)`, and the action stays running (§7.3).
  - **`Doctor`** opens the doctor panel (out, then err), scrollable; `o` runs `doctor --online`.
    A panel wraps a line wider than its 72 columns (at 80) at word boundaries, continuation rows
    indented as the line is (a third of the panel at most), and its scroll stops at its last
    row, counted at the terminal's width (R21). A scroll a resize left past the end is drawn
    at the last row, and the next key moves from there (R22).
  - **Nothing on stdout or stderr** is a suspended action's outcome (R12). Its output went to
    the terminal, which the person read before pressing Enter, so the runtime delivers it as
    `Msg::ActionDone` with empty `out` and `err`, and it shows no toast and no panel, whatever
    its exit. (An action run inside the TUI always prints something: the CLI prints its result,
    or `tagteam: <error>` on a failure.)
  - **One plain line with exit 0 and nothing on stderr** is a toast.
  - **Otherwise** (warnings on stderr, several lines, or a non-zero exit) the result panel opens
    with the output.
  - **Every `Done` outcome is followed by `LoadAccounts`, and by History's `LoadHistory` when
    History is open** (spec §7.2, and §7.3 step 4: every view is re-read), except during a
    shutdown, when nothing more is read. `NeedsTerminal` is not an end: the action goes on
    suspended, and its own outcome, which the runtime delivers as `Msg::ActionDone`, brings the
    re-read.
- **`ChannelAsk` (spec §7.2, §10.2 step 3).** It sends `Msg::Prompt` and waits for the answer in
  100 ms slices. It declines without sending anything once the shutdown token is set, and
  declines within a slice when the token is set while it waits. An answer that arrives after
  the token is set is a decline too. If the UI thread drops the dialog, the reply channel
  disconnects, and that is a decline as well.

**Depends on:** 6, 8

**Files:**
- Modify: `crates/tagteam-tui/src/actions.rs` (Task 5's file, replaced as a whole)
- Modify: `crates/tagteam-tui/src/ask.rs` (Task 3's one-line module becomes `ChannelAsk`)
- Test: unit tests in both files

**Interfaces:**
- Consumes:
  - Task 3: `host::{Action, AccountRef, ActionOutcome, Ask, Strategy}` (`Action::{name,
    needs_terminal}`).
  - Task 5: `State::{load_accounts, toast}`, `msg::{Answer, Cmd, Msg, Prompt, PromptKind}`,
    `view::{bold, centered, pad, selected, width_of, wrap, wrap_rows}`, `testutil`.
  - Task 6: `AccountsState::{selected, selected_view, shown_ids, snapshot}`, and
    `view::scrolled_windows` for the choose dialog (R21).
  - Task 7: `screens::history::reload`. Task 8: `State::collect_due`.
  - `tagteam_engine::words::email`; `tagteam_provider::Cancel`.
- Produces:
  - C7 in full: every `Overlay` variant handled and drawn; `handle_action_key`,
    `handle_overlay_key`, `on_prompt`, `on_done` and `render_overlay` as specified.
  - `pub(crate) struct ChannelAsk { tx: Sender<Msg>, shutdown: Cancel }`, `impl Ask for
    ChannelAsk`, and, for Task 11, `pub(crate) fn ChannelAsk::new(tx: Sender<Msg>, shutdown:
    Cancel) -> ChannelAsk`.
  - Crate-private: `actions::start(state, action, now_ms) -> Vec<Cmd>` (the busy rule, then
    `Act` or `Suspend`).

**Spec:**
- spec §5.5: dialogs (`Esc` declines), the result panel (`Esc` or `Enter`), the doctor panel
  (`o` for `--online`).
- spec §5.6 and §7.1: the keys and what each runs; Remove's own confirmation; the alias and
  move dialogs.
- spec §7.2: one at a time, `busy: <action>`, prompts as dialogs, the outcome rules, a re-read
  after each action.
- spec §7.3: `Run` always suspends; `NeedsTerminal` re-runs suspended.
- spec §10.2 step 3: prompts are declined unshown from the moment the shutdown token is set.
- Review Focus 3 (with Task 6).

- [ ] **Step 1: Write the failing tests**

Replace the test module of `crates/tagteam-tui/src/actions.rs` (Task 5's three tests) with:

```rust
#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use crossterm::event::KeyCode;

    use super::*;
    use crate::msg::Msg;
    use crate::testutil::*;
    use crate::update::update;

    fn loaded(n: u32) -> State {
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(n)));
        state
    }

    fn sel(position: u32) -> AccountRef {
        AccountRef {
            id: id(position),
            provider: cc(),
            position,
        }
    }

    fn done(exit: i32, out: &str, err: &str) -> ActionOutcome {
        ActionOutcome::Done {
            exit,
            out: out.into(),
            err: err.into(),
        }
    }

    #[test]
    fn the_account_keys_run_the_cli_s_commands_on_the_selected_account() {
        let mut disabled = many(2);
        disabled[1].row.disabled = true;
        for (c, action) in [
            ('s', Action::Switch(sel(1))),
            (
                'b',
                Action::SwitchBy {
                    provider: cc(),
                    strategy: Strategy::Best,
                },
            ),
            (
                'n',
                Action::SwitchBy {
                    provider: cc(),
                    strategy: Strategy::NextAvailable,
                },
            ),
            ('e', Action::Disable(sel(1))),
            ('A', Action::AddLive { provider: cc() }),
            (
                'D',
                Action::Doctor {
                    provider: None,
                    online: false,
                },
            ),
        ] {
            let mut state = loaded(2);
            let cmds = update(&mut state, ch(c), NOW_MS);
            assert_eq!(cmds, [Cmd::Act(action.clone())], "{c}");
            assert_eq!(state.running, Some(action), "{c}");
        }
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(disabled));
        update(&mut state, ch('j'), NOW_MS);
        assert_eq!(
            update(&mut state, ch('e'), NOW_MS),
            [Cmd::Act(Action::Enable(sel(2)))]
        );
    }

    #[test]
    fn run_always_suspends() {
        let mut state = loaded(1);
        assert_eq!(
            update(&mut state, ch('R'), NOW_MS),
            [Cmd::Suspend(Action::Run(sel(1)))]
        );
        assert_eq!(state.running, Some(Action::Run(sel(1))));
    }

    #[test]
    fn a_second_action_while_one_runs_is_busy_and_dropped() {
        let mut state = loaded(2);
        update(&mut state, ch('s'), NOW_MS);
        for c in ['s', 'b', 'x', 'a', 'm', 'A', 'D', 'R', 'r'] {
            let cmds = update(&mut state, ch(c), NOW_MS);
            assert!(cmds.is_empty(), "{c}: {cmds:?}");
            assert_eq!(state.toast.as_ref().unwrap().text, "busy: switch", "{c}");
            assert!(state.overlay.is_none(), "{c}");
        }
        assert_eq!(state.running, Some(Action::Switch(sel(1))));
    }

    /// Review Focus 3: with no accounts, every account key does nothing; `A`, `D` and `?` work.
    #[test]
    fn with_no_accounts_only_add_doctor_and_help_do_anything() {
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(vec![]));
        for c in ['s', 'b', 'n', 'e', 'a', 'm', 'x', 'R'] {
            let cmds = update(&mut state, ch(c), NOW_MS);
            assert!(cmds.is_empty(), "{c}: {cmds:?}");
            assert!(state.overlay.is_none() && state.toast.is_none() && state.running.is_none());
        }
        let cmds = update(&mut state, ch('r'), NOW_MS);
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadAccounts { .. }]),
            "{cmds:?}"
        );
        assert_eq!(
            update(&mut state, ch('A'), NOW_MS),
            [Cmd::Act(Action::AddLive { provider: cc() })]
        );
        state.running = None;
        assert!(matches!(
            update(&mut state, ch('D'), NOW_MS).as_slice(),
            [Cmd::Act(Action::Doctor { .. })]
        ));
        update(&mut state, ch('?'), NOW_MS);
        assert!(matches!(state.overlay, Some(Overlay::Help)));
    }

    #[test]
    fn add_uses_the_provider_flag_over_the_default() {
        let mut state = loaded(1);
        state.provider = Some(tagteam_core::ProviderId::new("fake-agent"));
        assert_eq!(
            update(&mut state, ch('A'), NOW_MS),
            [Cmd::Act(Action::AddLive {
                provider: tagteam_core::ProviderId::new("fake-agent")
            })]
        );
        let mut unloaded = fresh(Screen::Accounts);
        assert!(update(&mut unloaded, ch('A'), NOW_MS).is_empty());
    }

    #[test]
    fn remove_asks_its_own_question_and_defaults_to_no() {
        let mut state = loaded(2);
        update(&mut state, ch('j'), NOW_MS);
        assert!(update(&mut state, ch('x'), NOW_MS).is_empty());
        let Some(Overlay::Confirm {
            question,
            default_yes,
            choice,
            ..
        }) = &state.overlay
        else {
            panic!("{:?}", state.overlay);
        };
        assert_eq!(
            question,
            "Remove 2 a2@x.co? Its stored login is deleted; Claude Code is not changed."
        );
        assert!(!default_yes && !choice);
        let drawn = draw(&state, 80, 24);
        assert!(
            drawn
                .iter()
                .any(|l| l.contains("Remove 2 a2@x.co? Its stored login is")),
            "{drawn:?}"
        );
        // Enter takes the default: no.
        assert!(update(&mut state, key(KeyCode::Enter), NOW_MS).is_empty());
        assert!(state.overlay.is_none() && state.running.is_none());
        update(&mut state, ch('x'), NOW_MS);
        assert!(update(&mut state, key(KeyCode::Esc), NOW_MS).is_empty());
        update(&mut state, ch('x'), NOW_MS);
        assert_eq!(
            update(&mut state, ch('y'), NOW_MS),
            [Cmd::Act(Action::Remove(sel(2)))]
        );
    }

    #[test]
    fn the_alias_dialog_sets_or_clears_and_takes_any_key() {
        let mut state = loaded(1);
        update(&mut state, ch('a'), NOW_MS);
        for c in "-q1?x".chars() {
            update(&mut state, ch(c), NOW_MS);
        }
        let Some(Overlay::Text { input, prompt, .. }) = &state.overlay else {
            panic!("{:?}", state.overlay);
        };
        assert_eq!(input, "-q1?x");
        assert_eq!(prompt, "Alias for 1 a1@x.co (empty clears):");
        assert_eq!(state.quitting, None, "q was typed, not quit");
        assert_eq!(
            update(&mut state, key(KeyCode::Enter), NOW_MS),
            [Cmd::Act(Action::SetAlias {
                account: sel(1),
                alias: "-q1?x".into()
            })]
        );
        state.running = None;
        let mut aliased = many(1);
        aliased[0].row.alias = Some("w".into());
        state.accounts.apply(snapshot(aliased));
        update(&mut state, ch('a'), NOW_MS);
        update(&mut state, key(KeyCode::Backspace), NOW_MS);
        assert_eq!(
            update(&mut state, key(KeyCode::Enter), NOW_MS),
            [Cmd::Act(Action::ClearAlias(sel(1)))]
        );
        state.running = None;
        update(&mut state, ch('a'), NOW_MS);
        assert!(update(&mut state, key(KeyCode::Esc), NOW_MS).is_empty());
        assert!(state.overlay.is_none() && state.running.is_none());
    }

    #[test]
    fn the_move_dialog_takes_digits_only() {
        let mut state = loaded(3);
        update(&mut state, ch('m'), NOW_MS);
        assert!(
            update(&mut state, key(KeyCode::Enter), NOW_MS).is_empty(),
            "nothing typed"
        );
        assert!(state.overlay.is_some());
        for c in "a-3x".chars() {
            update(&mut state, ch(c), NOW_MS);
        }
        let Some(Overlay::Text { input, .. }) = &state.overlay else {
            panic!();
        };
        assert_eq!(input, "3");
        assert_eq!(
            update(&mut state, key(KeyCode::Enter), NOW_MS),
            [Cmd::Act(Action::Move {
                account: sel(1),
                position: 3
            })]
        );
    }

    #[test]
    fn a_cli_prompt_opens_a_dialog_bound_to_its_reply() {
        let mut state = loaded(1);
        let (reply, answer) = mpsc::channel();
        update(
            &mut state,
            Msg::Prompt(Prompt {
                question: "Add it?".into(),
                kind: PromptKind::Confirm { default_yes: true },
                reply,
            }),
            NOW_MS,
        );
        assert!(matches!(
            state.overlay,
            Some(Overlay::Confirm { choice: true, .. })
        ));
        update(&mut state, key(KeyCode::Right), NOW_MS);
        update(&mut state, key(KeyCode::Enter), NOW_MS);
        assert_eq!(answer.try_recv(), Ok(Answer::Confirm(false)));

        let (reply, answer) = mpsc::channel();
        update(
            &mut state,
            Msg::Prompt(Prompt {
                question: "Which account?".into(),
                kind: PromptKind::Choose {
                    options: vec!["one".into(), "two".into(), "three".into()],
                },
                reply,
            }),
            NOW_MS,
        );
        let drawn = draw(&state, 80, 24);
        assert!(drawn.iter().any(|l| l.contains("1. one")), "{drawn:?}");
        update(&mut state, key(KeyCode::Down), NOW_MS);
        update(&mut state, key(KeyCode::Down), NOW_MS);
        update(&mut state, key(KeyCode::Down), NOW_MS);
        update(&mut state, key(KeyCode::Enter), NOW_MS);
        assert_eq!(answer.try_recv(), Ok(Answer::Choose(Some(2))));

        for (code, want) in [
            (KeyCode::Esc, Answer::Choose(None)),
            (KeyCode::Char('2'), Answer::Choose(Some(1))),
        ] {
            let (reply, answer) = mpsc::channel();
            actions_prompt_choose(&mut state, reply);
            update(&mut state, key(code), NOW_MS);
            assert_eq!(answer.try_recv(), Ok(want));
        }
        let (reply, answer) = mpsc::channel();
        on_prompt(
            &mut state,
            Prompt {
                question: "Sure?".into(),
                kind: PromptKind::Confirm { default_yes: true },
                reply,
            },
        );
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert_eq!(
            answer.try_recv(),
            Ok(Answer::Confirm(false)),
            "Esc declines"
        );
    }

    fn actions_prompt_choose(state: &mut State, reply: Sender<Answer>) {
        on_prompt(
            state,
            Prompt {
                question: "Which?".into(),
                kind: PromptKind::Choose {
                    options: vec!["a".into(), "b".into()],
                },
                reply,
            },
        );
    }

    #[test]
    fn a_prompt_during_a_shutdown_is_declined_unshown() {
        let mut state = loaded(1);
        state.quitting = Some(0);
        let (reply, answer) = mpsc::channel();
        update(
            &mut state,
            Msg::Prompt(Prompt {
                question: "Add it?".into(),
                kind: PromptKind::Confirm { default_yes: true },
                reply,
            }),
            NOW_MS,
        );
        assert!(state.overlay.is_none());
        assert_eq!(answer.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }

    #[test]
    fn quitting_with_a_dialog_open_declines_it() {
        let mut state = loaded(1);
        let (reply, answer) = mpsc::channel();
        on_prompt(
            &mut state,
            Prompt {
                question: "Add it?".into(),
                kind: PromptKind::Confirm { default_yes: true },
                reply,
            },
        );
        assert_eq!(
            update(&mut state, ctrl('c'), NOW_MS),
            [Cmd::Quit { code: 0 }]
        );
        assert_eq!(answer.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }

    #[test]
    fn one_plain_line_is_a_toast_and_the_views_are_read_again() {
        let mut state = loaded(1);
        update(&mut state, ch('s'), NOW_MS);
        let cmds = update(
            &mut state,
            Msg::ActionDone {
                action: Action::Switch(sel(1)),
                outcome: done(0, "Enabled 1 a1@x.co.\n", ""),
            },
            NOW_MS,
        );
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadAccounts { .. }]),
            "{cmds:?}"
        );
        assert_eq!(state.toast.as_ref().unwrap().text, "Enabled 1 a1@x.co.");
        assert!(state.running.is_none() && state.overlay.is_none());
        assert_eq!(draw(&state, 80, 24)[23], "Enabled 1 a1@x.co.");
    }

    #[test]
    fn warnings_several_lines_or_a_failure_open_the_result_panel() {
        for (exit, out, err, want) in [
            (
                0,
                "Switched to b@x.co (position 2).\nRestart Claude Code to apply.\n",
                "",
                "Switched to b@x.co (position 2).\nRestart Claude Code to apply.",
            ),
            (
                0,
                "Switched.\n",
                "warning: usage was not collected\n",
                "Switched.\nwarning: usage was not collected",
            ),
            (
                1,
                "",
                "tagteam: the selected account changed; nothing was done\n",
                "tagteam: the selected account changed; nothing was done",
            ),
        ] {
            let mut state = loaded(1);
            update(&mut state, ch('s'), NOW_MS);
            let cmds = on_done(
                &mut state,
                Action::Switch(sel(1)),
                done(exit, out, err),
                NOW_MS,
            );
            assert!(matches!(cmds.as_slice(), [Cmd::LoadAccounts { .. }]));
            let Some(Overlay::Result { title, text, .. }) = &state.overlay else {
                panic!("{exit} {out:?} {err:?}: {:?}", state.overlay);
            };
            assert_eq!((title.as_str(), text.as_str()), ("switch", want));
        }
        let mut state = loaded(1);
        on_done(
            &mut state,
            Action::Switch(sel(1)),
            done(
                1,
                "",
                "tagteam: the selected account changed; nothing was done\n",
            ),
            NOW_MS,
        );
        let drawn = draw(&state, 80, 24);
        assert!(
            drawn
                .iter()
                .any(|l| l.contains("tagteam: the selected account changed; nothing was done")),
            "{drawn:?}"
        );
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert!(state.overlay.is_none());
    }

    /// R12: a suspended action's output went to the terminal, so its outcome arrives empty and
    /// shows nothing, whatever its exit; every view is read again (spec §7.3 step 4).
    #[test]
    fn a_suspended_actions_outcome_shows_nothing_and_rereads_every_view() {
        for exit in [0, 1, 130] {
            let mut state = loaded(1);
            state.history.select(id(1));
            state.screen = Screen::History;
            state.running = Some(Action::Run(sel(1)));
            let cmds = on_done(&mut state, Action::Run(sel(1)), done(exit, "", ""), NOW_MS);
            assert!(
                matches!(
                    cmds.as_slice(),
                    [Cmd::LoadAccounts { .. }, Cmd::LoadHistory { account, .. }]
                        if *account == id(1)
                ),
                "{exit}: {cmds:?}"
            );
            assert!(state.toast.is_none() && state.overlay.is_none(), "{exit}");
            assert!(state.running.is_none(), "{exit}");
        }
    }

    #[test]
    fn needs_terminal_runs_the_action_again_suspended() {
        let mut state = loaded(1);
        update(&mut state, ch('s'), NOW_MS);
        let cmds = on_done(
            &mut state,
            Action::Switch(sel(1)),
            ActionOutcome::NeedsTerminal,
            NOW_MS,
        );
        assert_eq!(cmds, [Cmd::Suspend(Action::Switch(sel(1)))]);
        assert_eq!(
            state.running,
            Some(Action::Switch(sel(1))),
            "still running, suspended"
        );
    }

    #[test]
    fn doctor_opens_its_panel_and_o_runs_it_online() {
        let mut state = loaded(1);
        update(&mut state, ch('D'), NOW_MS);
        let report: String = (1..=40).map(|i| format!("check {i}: ok\n")).collect();
        on_done(
            &mut state,
            Action::Doctor {
                provider: None,
                online: false,
            },
            done(0, &report, ""),
            NOW_MS,
        );
        let drawn = draw(&state, 80, 24);
        assert!(drawn.iter().any(|l| l.contains(" doctor ")), "{drawn:?}");
        assert!(drawn.iter().any(|l| l.contains("check 1: ok")));
        for _ in 0..5 {
            update(&mut state, key(KeyCode::Down), NOW_MS);
        }
        let drawn = draw(&state, 80, 24);
        assert!(drawn.iter().any(|l| l.contains("check 6: ok")));
        assert!(!drawn.iter().any(|l| l.contains("check 5: ok")));
        for _ in 0..100 {
            update(&mut state, key(KeyCode::Down), NOW_MS);
        }
        assert!(matches!(
            state.overlay,
            Some(Overlay::Doctor { scroll: 39, .. })
        ));
        assert_eq!(
            update(&mut state, ch('o'), NOW_MS),
            [Cmd::Act(Action::Doctor {
                provider: None,
                online: true
            })]
        );
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert!(state.overlay.is_none());
    }

    #[test]
    fn a_long_output_line_wraps_in_its_panel_and_scrolls_to_its_end() {
        // Spec §5.5: the panel shows the action's output; at 80 columns it is 72 wide.
        let mut state = loaded(1);
        let long = "  warning: usage for averyveryverylongname@example.com (position 2) was not collected: the endpoint answered http-429, retry in 5m";
        let text: String = (1..=30)
            .map(|i| format!("line {i}\n"))
            .chain([format!("{long}\n")])
            .collect();
        on_done(
            &mut state,
            Action::Switch(sel(1)),
            done(1, &text, ""),
            NOW_MS,
        );
        update(&mut state, key(KeyCode::End), NOW_MS);
        let Some(Overlay::Result { scroll, .. }) = &state.overlay else {
            panic!("{:?}", state.overlay);
        };
        assert_eq!(*scroll, 31, "30 lines, then the long one's two");
        let drawn = draw(&state, 80, 24);
        let inside = |l: &String| l.chars().skip(4).take(72).collect::<String>();
        assert_eq!(
            inside(&drawn[2]).trim_end(),
            "  not collected: the endpoint answered http-429, retry in 5m",
            "the last row, indented as its line is: {drawn:?}"
        );
        update(&mut state, key(KeyCode::Up), NOW_MS);
        let drawn = draw(&state, 80, 24);
        assert_eq!(
            inside(&drawn[2]).trim_end(),
            "  warning: usage for averyveryverylongname@example.com (position 2) was"
        );
    }

    #[test]
    fn a_resize_clamps_a_panels_scroll_and_the_next_key_moves_at_once() {
        // Thirty 100-column lines: two rows each at 80 columns, one at 120.
        let text: String = (1..=30)
            .map(|i| format!("{i:02} {}\n", "word ".repeat(19).trim_end()))
            .collect();
        let first_row = |state: &State, w: u16| -> String {
            let drawn = draw(state, w, 24);
            drawn[2]
                .chars()
                .skip(4)
                .take(usize::from(w) - 8)
                .collect::<String>()
        };
        for action in [
            Action::Switch(sel(1)),
            Action::Doctor {
                provider: None,
                online: false,
            },
        ] {
            let mut state = loaded(1);
            on_done(&mut state, action.clone(), done(1, &text, ""), NOW_MS);
            update(&mut state, key(KeyCode::End), NOW_MS);
            assert!(
                first_row(&state, 80).starts_with("word"),
                "{action:?}: row 60 of 60"
            );
            update(
                &mut state,
                Msg::Resize {
                    width: 120,
                    height: 24,
                },
                NOW_MS,
            );
            assert!(
                first_row(&state, 120).starts_with("30 word"),
                "{action:?}: the last of 30 rows, not a blank panel"
            );
            update(&mut state, key(KeyCode::Up), NOW_MS);
            assert!(
                first_row(&state, 120).starts_with("29 word"),
                "{action:?}: Up moves from what shows"
            );
        }
    }

    #[test]
    fn an_indent_wider_than_a_third_of_a_panel_is_capped() {
        let mut state = loaded(1);
        let line = format!("{}{}", " ".repeat(60), "word ".repeat(20).trim_end());
        on_done(
            &mut state,
            Action::Switch(sel(1)),
            done(1, &line, ""),
            NOW_MS,
        );
        let drawn = draw(&state, 80, 24);
        let rows: Vec<String> = drawn[2..5]
            .iter()
            .map(|l| {
                l.chars()
                    .skip(4)
                    .take(72)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect();
        let indent = " ".repeat(24);
        assert!(
            rows.iter().all(|r| r.starts_with(&indent) && r.len() > 24),
            "every row keeps a third of the panel for its words: {rows:?}"
        );
        assert_eq!(
            rows.iter()
                .map(|r| r.matches("word").count())
                .sum::<usize>(),
            20
        );
    }

    #[test]
    fn a_long_choice_wraps_under_its_text_and_is_selected_whole() {
        let mut state = loaded(1);
        let (reply, _answer) = mpsc::channel();
        let long =
            "claude-code #2 averyveryverylongname.with.a.long.tail@example.com [Example Org]";
        update(
            &mut state,
            Msg::Prompt(Prompt {
                question: "Which account?".into(),
                kind: PromptKind::Choose {
                    options: vec!["claude-code #1 a@x.co".into(), long.into()],
                },
                reply,
            }),
            NOW_MS,
        );
        update(&mut state, key(KeyCode::Down), NOW_MS);
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        let lines = text_of(&buf);
        let at = lines
            .iter()
            .position(|l| l.contains("2. claude-code #2"))
            .unwrap();
        let text = |y: usize| -> String {
            lines[y]
                .chars()
                .skip(10)
                .take(60)
                .collect::<String>()
                .trim_end()
                .to_owned()
        };
        assert_eq!(
            [text(at), text(at + 1), text(at + 2)],
            [
                "2. claude-code #2",
                "   averyveryverylongname.with.a.long.tail@example.com",
                "   [Example Org]",
            ]
        );
        for y in at..at + 3 {
            let cell = &buf[(10, u16::try_from(y).unwrap())];
            let reversed = ratatui::style::Modifier::REVERSED;
            assert!(cell.modifier.contains(reversed), "row {y}");
        }
    }

    #[test]
    fn a_1000_character_choice_takes_three_rows_and_every_choice_stays_reachable() {
        let mut state = loaded(1);
        let (reply, _answer) = mpsc::channel();
        let long = "Claude ".repeat(143).trim_end().to_owned();
        let options: Vec<String> = [long]
            .into_iter()
            .chain((2..=20).map(|i| format!("claude-code #{i} a{i}@x.co")))
            .collect();
        update(
            &mut state,
            Msg::Prompt(Prompt {
                question: "Which account?".into(),
                kind: PromptKind::Choose { options },
                reply,
            }),
            NOW_MS,
        );
        let text = |state: &State| -> Vec<String> {
            draw(state, 80, 24)
                .iter()
                .map(|l| {
                    l.chars()
                        .skip(10)
                        .take(60)
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect()
        };
        let shown = text(&state);
        let first = shown
            .iter()
            .position(|l| l.starts_with("1. Claude"))
            .unwrap();
        assert!(shown[first + 2].ends_with('…'), "{shown:?}");
        assert!(
            shown[first + 3].starts_with("2. claude-code #2"),
            "{shown:?}"
        );
        assert!(shown.iter().any(|l| l == "↑ ↓ choose · Enter · Esc cancel"));
        for _ in 0..19 {
            update(&mut state, key(KeyCode::Down), NOW_MS);
        }
        let shown = text(&state);
        assert!(
            shown.iter().any(|l| l.starts_with("20. claude-code #20")),
            "{shown:?}"
        );
        assert!(shown.iter().any(|l| l == "↑ ↓ choose · Enter · Esc cancel"));
    }

    #[test]
    fn a_long_question_is_cut_so_the_dialog_keeps_its_buttons() {
        let mut state = loaded(1);
        let (reply, _answer) = mpsc::channel();
        let question = format!("{}?", "Claude ".repeat(286).trim_end());
        update(
            &mut state,
            Msg::Prompt(Prompt {
                question,
                kind: PromptKind::Confirm { default_yes: false },
                reply,
            }),
            NOW_MS,
        );
        let drawn = draw(&state, 80, 24);
        assert!(drawn.iter().any(|l| l.contains("[ Yes ]")), "{drawn:?}");
        assert!(
            drawn.iter().any(|l| l.contains("y yes · n no")),
            "{drawn:?}"
        );
        assert!(
            drawn.iter().any(|l| l.contains('…')),
            "the question is cut: {drawn:?}"
        );
    }

    #[test]
    fn a_long_input_shows_its_end() {
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("山田太郎", 5), "太郎");
        assert_eq!(tail("ab", 5), "ab");
    }

    #[test]
    fn help_lists_the_keys_and_closes() {
        let mut state = loaded(1);
        update(&mut state, ch('?'), NOW_MS);
        let drawn = draw(&state, 80, 24);
        assert!(drawn.iter().any(|l| l.contains("Everywhere")), "{drawn:?}");
        assert!(
            drawn
                .iter()
                .any(|l| l.contains("R") && l.contains("run a session"))
        );
        update(&mut state, ch('?'), NOW_MS);
        assert!(state.overlay.is_none());
        update(&mut state, ch('?'), NOW_MS);
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert!(state.overlay.is_none());
    }

    #[test]
    fn refresh_rereads_and_collects_the_shown_accounts() {
        let mut state = loaded(2);
        assert_eq!(
            update(&mut state, ch('r'), NOW_MS),
            [
                Cmd::LoadAccounts { r#gen: 1 },
                Cmd::CollectDue {
                    r#gen: 1,
                    accounts: vec![id(1), id(2)]
                }
            ]
        );
    }
}
```

Replace `crates/tagteam-tui/src/ask.rs` with its module line and this test module:

```rust
//! The bridge's `Ask` (spec §7.2, §10.2 step 3): a CLI prompt on the actions lane becomes a
//! dialog on the UI thread, and the lane waits for its answer in slices it can leave.

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Instant;

    use super::*;

    const WAIT: Duration = Duration::from_secs(5);
    /// What the shutdown token records for a `q` (spec §10.2 step 1).
    const SIGINT: i32 = 2;

    /// The UI thread's side: the next prompt, or `None` when none comes within `within`.
    fn next_prompt(rx: &mpsc::Receiver<Msg>, within: Duration) -> Option<Prompt> {
        match rx.recv_timeout(within) {
            Ok(Msg::Prompt(p)) => Some(p),
            Ok(_) => panic!("only prompts are sent"),
            Err(_) => None,
        }
    }

    #[test]
    fn an_answer_reaches_the_waiting_lane() {
        let (tx, rx) = mpsc::channel();
        let shutdown = Cancel::new();
        let mut ask = ChannelAsk::new(tx.clone(), shutdown.clone());
        let lane = thread::spawn(move || ask.confirm("Add it?", true));
        let prompt = next_prompt(&rx, WAIT).unwrap();
        assert_eq!(prompt.question, "Add it?");
        assert!(matches!(
            prompt.kind,
            PromptKind::Confirm { default_yes: true }
        ));
        prompt.reply.send(Answer::Confirm(true)).unwrap();
        assert!(lane.join().unwrap());

        let mut ask = ChannelAsk::new(tx, shutdown);
        let lane = thread::spawn(move || ask.choose("Which?", &["a".into(), "b".into()]));
        let prompt = next_prompt(&rx, WAIT).unwrap();
        let PromptKind::Choose { options } = &prompt.kind else {
            panic!("a choice");
        };
        assert_eq!(options, &["a", "b"]);
        prompt.reply.send(Answer::Choose(Some(1))).unwrap();
        assert_eq!(lane.join().unwrap(), Some(1));
    }

    #[test]
    fn a_shutdown_before_the_prompt_declines_without_showing_it() {
        let (tx, rx) = mpsc::channel();
        let shutdown = Cancel::new();
        shutdown.request(SIGINT);
        let mut ask = ChannelAsk::new(tx, shutdown);
        assert!(!ask.confirm("Add it?", true));
        assert_eq!(ask.choose("Which?", &["a".into()]), None);
        assert!(next_prompt(&rx, Duration::from_millis(50)).is_none());
    }

    #[test]
    fn a_shutdown_while_waiting_declines_within_one_slice() {
        let (tx, rx) = mpsc::channel();
        let shutdown = Cancel::new();
        let mut ask = ChannelAsk::new(tx, shutdown.clone());
        let lane = thread::spawn(move || ask.confirm("Add it?", true));
        let prompt = next_prompt(&rx, WAIT).unwrap();
        let set = Instant::now();
        shutdown.request(SIGINT);
        assert!(!lane.join().unwrap());
        assert!(set.elapsed() < SLICE * 3, "{:?}", set.elapsed());
        // The dialog's answer, should it still come, goes nowhere.
        let _ = prompt.reply.send(Answer::Confirm(true));
    }

    #[test]
    fn a_dialog_dropped_unanswered_declines() {
        let (tx, rx) = mpsc::channel();
        let mut ask = ChannelAsk::new(tx, Cancel::new());
        let lane = thread::spawn(move || ask.choose("Which?", &["a".into()]));
        drop(next_prompt(&rx, WAIT).unwrap());
        assert_eq!(lane.join().unwrap(), None);
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-tui --lib -- actions:: ask::`
Expected: the build fails with E0433 (use of undeclared types `ChannelAsk` and `Strategy`), E0425
(cannot find function `tail`), and with `Cancel`, `Duration` and `mpsc` not in scope in
`ask.rs`.

- [ ] **Step 3: Implement**

Everything above the test module in `crates/tagteam-tui/src/actions.rs` becomes:

```rust
//! Actions in the TUI (spec §5.5, §7): the action keys, the dialogs and panels, the CLI's
//! prompts, and what an action's outcome shows.

use std::sync::mpsc::Sender;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use tagteam_engine::words;

use crate::host::{AccountRef, Action, ActionOutcome, Strategy};
use crate::msg::{Answer, Cmd, Prompt, PromptKind};
use crate::screens;
use crate::state::{Screen, State};
use crate::view::{bold, centered, pad, scrolled_windows, selected, width_of, wrap, wrap_rows};

#[derive(Debug)]
pub enum Overlay {
    Help,
    Confirm {
        question: String,
        default_yes: bool,
        choice: bool,
        then: ConfirmThen,
    },
    Choose {
        question: String,
        options: Vec<String>,
        selected: usize,
        reply: Sender<Answer>,
    },
    Text {
        prompt: String,
        input: String,
        then: TextThen,
    },
    Result {
        title: String,
        text: String,
        scroll: u16,
    },
    Doctor {
        text: String,
        scroll: u16,
        online: bool,
    },
}

/// What a confirm dialog's answer goes to: a waiting CLI prompt, or the TUI's own question
/// before an action (`Remove`'s).
#[derive(Debug)]
pub enum ConfirmThen {
    Reply(Sender<Answer>),
    Run(Action),
}

/// What a text dialog's input is for.
#[derive(Debug)]
pub enum TextThen {
    Alias(AccountRef),
    Move(AccountRef),
}

/// The help panel's keys (spec §5.6): everywhere, then the current screen's.
const HELP_GLOBAL: [(&str, &str); 7] = [
    ("1 3  Tab  Shift-Tab", "screens"),
    ("j k  ↑ ↓", "move"),
    ("A", "add the current login"),
    ("D", "doctor"),
    ("?", "help"),
    ("q  Ctrl-C", "quit"),
    ("Ctrl-Z", "suspend"),
];
const HELP_ACCOUNTS: [(&str, &str); 11] = [
    ("s", "switch to the selected account"),
    ("b", "switch to the best account"),
    ("n", "switch to the next available account"),
    ("e", "enable or disable"),
    ("a", "set or clear the alias"),
    ("m", "move to another position"),
    ("x", "remove"),
    ("R", "run a session on the account"),
    ("r", "refresh"),
    ("Enter", "history of the account"),
    ("J K", "scroll the detail's windows"),
];
const HELP_HISTORY: [(&str, &str); 3] = [
    ("[ ]", "range: 1, 7, 14 or 30 days"),
    ("r", "refresh"),
    ("J K", "scroll the windows"),
];

/// The rows a choice takes, at most (R21): a CLI's `<provider> #<n> <name> [<org>]` fits.
const CHOICE_ROWS: usize = 3;

/// The rows a dialog's question takes on a terminal `height` rows tall: a third of it at most,
/// so its buttons, choices or input always show (R21).
fn question_rows(question: &str, width: usize, height: u16) -> Vec<String> {
    wrap_rows(question, width, usize::from(height / 3))
}

/// The longest alias and position the text dialogs take.
const ALIAS_MAX: usize = 64;
const POSITION_MAX: usize = 9;

/// Starts `action`: on the actions lane, or suspended when it needs the terminal (spec §7.3).
/// While another runs it shows `busy: <action>` and drops it (spec §7.2).
pub(crate) fn start(state: &mut State, action: Action, now_ms: i64) -> Vec<Cmd> {
    if let Some(running) = &state.running {
        let text = format!("busy: {}", running.name());
        state.toast(text, now_ms);
        return vec![];
    }
    state.running = Some(action.clone());
    state.dirty = true;
    if action.needs_terminal() {
        vec![Cmd::Suspend(action)]
    } else {
        vec![Cmd::Act(action)]
    }
}

/// `busy: <action>` when one runs; the action keys that open a dialog check it first.
fn busy(state: &mut State, now_ms: i64) -> bool {
    match &state.running {
        Some(running) => {
            let text = format!("busy: {}", running.name());
            state.toast(text, now_ms);
            true
        }
        None => false,
    }
}

/// The provider `A` adds to: `--provider`'s, else the default, which the accounts view lists
/// first (`Engine::accounts`).
fn add_provider(state: &State) -> Option<tagteam_core::ProviderId> {
    state.provider.clone().or_else(|| {
        state
            .accounts
            .snapshot()
            .and_then(|s| s.providers.first())
            .map(|p| p.provider.clone())
    })
}

/// The action keys (spec §5.6, §7.1). `None` for any other key. An account key with no
/// account selected does nothing (Review Focus 3).
pub fn handle_action_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Option<Vec<Cmd>> {
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    match c {
        'A' => Some(match add_provider(state) {
            Some(provider) => start(state, Action::AddLive { provider }, now_ms),
            None => vec![],
        }),
        'D' => {
            let provider = state.provider.clone();
            Some(start(
                state,
                Action::Doctor {
                    provider,
                    online: false,
                },
                now_ms,
            ))
        }
        'r' => Some(refresh(state, now_ms)),
        's' | 'b' | 'n' | 'e' | 'a' | 'm' | 'x' | 'R' if state.screen == Screen::Accounts => {
            Some(account_key(state, c, now_ms))
        }
        _ => None,
    }
}

/// `r` (spec §7.1): the views re-read, and the shown accounts offered to the collector at once.
fn refresh(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if busy(state, now_ms) {
        return vec![];
    }
    let mut cmds = vec![state.load_accounts()];
    if state.screen == Screen::History {
        cmds.extend(screens::history::reload(state, now_ms));
    }
    let shown = state.accounts.shown_ids();
    if !shown.is_empty() {
        cmds.push(state.collect_due(shown));
    }
    cmds
}

fn account_key(state: &mut State, c: char, now_ms: i64) -> Vec<Cmd> {
    let (Some(account), Some(view)) = (state.accounts.selected(), state.accounts.selected_view())
    else {
        return vec![];
    };
    let (disabled, alias) = (view.row.disabled, view.row.alias.clone());
    let named = format!("{} {}", account.position, words::email(&view.row));
    if busy(state, now_ms) {
        return vec![];
    }
    let provider = account.provider.clone();
    match c {
        's' => start(state, Action::Switch(account), now_ms),
        'b' | 'n' => {
            let strategy = if c == 'b' {
                Strategy::Best
            } else {
                Strategy::NextAvailable
            };
            start(state, Action::SwitchBy { provider, strategy }, now_ms)
        }
        'e' if disabled => start(state, Action::Enable(account), now_ms),
        'e' => start(state, Action::Disable(account), now_ms),
        'a' => {
            state.overlay = Some(Overlay::Text {
                prompt: format!("Alias for {named} (empty clears):"),
                input: alias.unwrap_or_default(),
                then: TextThen::Alias(account),
            });
            vec![]
        }
        'm' => {
            state.overlay = Some(Overlay::Text {
                prompt: format!("Move {named} to position:"),
                input: String::new(),
                then: TextThen::Move(account),
            });
            vec![]
        }
        'x' => {
            state.overlay = Some(Overlay::Confirm {
                question: format!(
                    "Remove {named}? Its stored login is deleted; Claude Code is not changed."
                ),
                default_yes: false,
                choice: false,
                then: ConfirmThen::Run(Action::Remove(account)),
            });
            vec![]
        }
        'R' => start(state, Action::Run(account), now_ms),
        _ => vec![],
    }
}

/// A key while an overlay is open. `Esc` declines a dialog and closes a panel (spec §5.5).
pub fn handle_overlay_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    let Some(overlay) = state.overlay.take() else {
        return vec![];
    };
    let (kept, cmds) = overlay_key(state, overlay, key, now_ms);
    state.overlay = kept;
    cmds
}

/// The overlay after `key`, or `None` once it is closed, and what it started.
fn overlay_key(
    state: &mut State,
    overlay: Overlay,
    key: KeyEvent,
    now_ms: i64,
) -> (Option<Overlay>, Vec<Cmd>) {
    match overlay {
        Overlay::Help => match key.code {
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Enter => (None, vec![]),
            _ => (Some(Overlay::Help), vec![]),
        },
        Overlay::Confirm {
            question,
            default_yes,
            choice,
            then,
        } => {
            let answer = match key.code {
                KeyCode::Char('y' | 'Y') => Some(true),
                KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(false),
                KeyCode::Enter => Some(choice),
                _ => None,
            };
            match answer {
                Some(yes) => (None, confirmed(state, then, yes, now_ms)),
                None => {
                    let choice = match key.code {
                        KeyCode::Left
                        | KeyCode::Right
                        | KeyCode::Tab
                        | KeyCode::BackTab
                        | KeyCode::Char('h' | 'l') => !choice,
                        _ => choice,
                    };
                    let kept = Overlay::Confirm {
                        question,
                        default_yes,
                        choice,
                        then,
                    };
                    (Some(kept), vec![])
                }
            }
        }
        Overlay::Choose {
            question,
            options,
            selected,
            reply,
        } => {
            let last = options.len().saturating_sub(1);
            let digit = match key.code {
                KeyCode::Char(c @ '1'..='9') => Some(c as usize - '1' as usize),
                _ => None,
            };
            let answer = match key.code {
                KeyCode::Enter if !options.is_empty() => Some(Some(selected)),
                KeyCode::Esc => Some(None),
                _ => digit.filter(|d| *d <= last).map(Some),
            };
            if let Some(answer) = answer {
                let _ = reply.send(Answer::Choose(answer));
                return (None, vec![]);
            }
            let selected = match key.code {
                KeyCode::Up | KeyCode::Char('k') => selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => (selected + 1).min(last),
                _ => selected,
            };
            let kept = Overlay::Choose {
                question,
                options,
                selected,
                reply,
            };
            (Some(kept), vec![])
        }
        Overlay::Text {
            prompt,
            mut input,
            then,
        } => match key.code {
            KeyCode::Esc => (None, vec![]),
            KeyCode::Enter => match submitted(&then, &input) {
                Some(action) => (None, start(state, action, now_ms)),
                None => (
                    Some(Overlay::Text {
                        prompt,
                        input,
                        then,
                    }),
                    vec![],
                ),
            },
            KeyCode::Backspace => {
                input.pop();
                (
                    Some(Overlay::Text {
                        prompt,
                        input,
                        then,
                    }),
                    vec![],
                )
            }
            KeyCode::Char(c) if !c.is_control() => {
                let fits = match then {
                    TextThen::Alias(_) => input.chars().count() < ALIAS_MAX,
                    TextThen::Move(_) => c.is_ascii_digit() && input.len() < POSITION_MAX,
                };
                if fits {
                    input.push(c);
                }
                (
                    Some(Overlay::Text {
                        prompt,
                        input,
                        then,
                    }),
                    vec![],
                )
            }
            _ => (
                Some(Overlay::Text {
                    prompt,
                    input,
                    then,
                }),
                vec![],
            ),
        },
        Overlay::Result {
            title,
            text,
            scroll,
        } => match key.code {
            KeyCode::Esc | KeyCode::Enter => (None, vec![]),
            code => {
                let scroll = scrolled(scroll, code, &text, state.size.0);
                (
                    Some(Overlay::Result {
                        title,
                        text,
                        scroll,
                    }),
                    vec![],
                )
            }
        },
        Overlay::Doctor {
            text,
            scroll,
            online,
        } => match key.code {
            KeyCode::Esc | KeyCode::Enter => (None, vec![]),
            KeyCode::Char('o') => {
                let provider = state.provider.clone();
                let action = Action::Doctor {
                    provider,
                    online: true,
                };
                let cmds = start(state, action, now_ms);
                let kept = Overlay::Doctor {
                    text,
                    scroll,
                    online,
                };
                (Some(kept), cmds)
            }
            code => {
                let scroll = scrolled(scroll, code, &text, state.size.0);
                (
                    Some(Overlay::Doctor {
                        text,
                        scroll,
                        online,
                    }),
                    vec![],
                )
            }
        },
    }
}

/// A confirm dialog's answer: sent to the prompt waiting for it, or a yes starts the action.
fn confirmed(state: &mut State, then: ConfirmThen, yes: bool, now_ms: i64) -> Vec<Cmd> {
    match then {
        ConfirmThen::Reply(reply) => {
            let _ = reply.send(Answer::Confirm(yes));
            vec![]
        }
        ConfirmThen::Run(action) if yes => start(state, action, now_ms),
        ConfirmThen::Run(_) => vec![],
    }
}

/// The action a text dialog's input asks for: an empty alias clears it (spec §7.1), and a
/// position must be a number.
fn submitted(then: &TextThen, input: &str) -> Option<Action> {
    match then {
        TextThen::Alias(account) if input.trim().is_empty() => {
            Some(Action::ClearAlias(account.clone()))
        }
        TextThen::Alias(account) => Some(Action::SetAlias {
            account: account.clone(),
            alias: input.to_owned(),
        }),
        TextThen::Move(account) => input.parse::<u32>().ok().map(|position| Action::Move {
            account: account.clone(),
            position,
        }),
    }
}

/// A panel's scroll after `code`, never past its last row at a terminal `width` wide: rows,
/// not lines, as the panel wraps them. It moves from the row that shows, which a resize may
/// have brought nearer than the stored scroll.
fn scrolled(scroll: u16, code: KeyCode, text: &str, width: u16) -> u16 {
    let last = last_row(text, width);
    let scroll = scroll.min(last);
    let to = match code {
        KeyCode::Down | KeyCode::Char('j') => scroll.saturating_add(1),
        KeyCode::Up | KeyCode::Char('k') => scroll.saturating_sub(1),
        KeyCode::PageDown | KeyCode::Char(' ') => scroll.saturating_add(10),
        KeyCode::PageUp => scroll.saturating_sub(10),
        KeyCode::Home | KeyCode::Char('g') => 0,
        KeyCode::End | KeyCode::Char('G') => last,
        _ => scroll,
    };
    to.min(last)
}

/// A CLI prompt from the actions lane (spec §7.2) opens a dialog bound to its reply. During a
/// shutdown it is dropped, which declines it.
pub fn on_prompt(state: &mut State, prompt: Prompt) {
    if state.quitting.is_some() {
        return;
    }
    let Prompt {
        question,
        kind,
        reply,
    } = prompt;
    state.overlay = Some(match kind {
        PromptKind::Confirm { default_yes } => Overlay::Confirm {
            question,
            default_yes,
            choice: default_yes,
            then: ConfirmThen::Reply(reply),
        },
        PromptKind::Choose { options } => Overlay::Choose {
            question,
            options,
            selected: 0,
            reply,
        },
    });
}

/// An action's outcome (spec §7.2): a command that needs the terminal is run again suspended
/// (§7.3); a suspended action's outcome, whose output went to the terminal, shows nothing (R12);
/// doctor's report opens its panel; one plain line is a toast; warnings, several lines or a
/// failure open the result panel. Every view is re-read after every outcome (§7.3 step 4).
pub fn on_done(state: &mut State, action: Action, outcome: ActionOutcome, now_ms: i64) -> Vec<Cmd> {
    let (exit, out, err) = match outcome {
        ActionOutcome::NeedsTerminal if state.quitting.is_none() => {
            state.running = Some(action.clone());
            return vec![Cmd::Suspend(action)];
        }
        ActionOutcome::NeedsTerminal => {
            state.running = None;
            return vec![];
        }
        ActionOutcome::Done { exit, out, err } => (exit, out, err),
    };
    state.running = None;
    state.dirty = true;
    if state.quitting.is_some() {
        return vec![];
    }
    let text = [out.trim_end(), err.trim_end()]
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    match action {
        // R12: a suspended action's output went to the terminal; there is nothing to show.
        _ if text.is_empty() => {}
        Action::Doctor { online, .. } => {
            state.overlay = Some(Overlay::Doctor {
                text,
                scroll: 0,
                online,
            });
        }
        _ if exit == 0 && err.trim().is_empty() && text.lines().count() == 1 => {
            state.toast(text, now_ms);
        }
        _ => {
            state.overlay = Some(Overlay::Result {
                title: action.name().to_owned(),
                text,
                scroll: 0,
            });
        }
    }
    let mut cmds = vec![state.load_accounts()];
    if state.screen == Screen::History {
        cmds.extend(screens::history::reload(state, now_ms));
    }
    cmds
}

pub fn render_overlay(state: &State, frame: &mut Frame, now_ms: i64) {
    let _ = now_ms;
    let Some(overlay) = &state.overlay else {
        return;
    };
    let area = frame.area();
    match overlay {
        Overlay::Help => {
            let keys: &[(&str, &str)] = match state.screen {
                Screen::Accounts => &HELP_ACCOUNTS,
                Screen::History => &HELP_HISTORY,
            };
            let mut lines = vec![Line::from(Span::styled("Everywhere", bold()))];
            lines.extend(HELP_GLOBAL.iter().map(|(k, what)| help_line(k, what)));
            let title = match state.screen {
                Screen::Accounts => "Accounts",
                Screen::History => "History",
            };
            lines.push(Line::from(Span::styled(title, bold())));
            lines.extend(keys.iter().map(|(k, what)| help_line(k, what)));
            boxed(frame, area, " Help ", lines, 64);
        }
        Overlay::Confirm {
            question, choice, ..
        } => {
            let w = 64u16.min(area.width.saturating_sub(4));
            let mut lines: Vec<Line<'static>> =
                question_rows(question, usize::from(w) - 4, area.height)
                    .into_iter()
                    .map(Line::from)
                    .collect();
            lines.push(Line::default());
            let button = |label: &str, on: bool| {
                let text = format!("[ {label} ]");
                if on {
                    Span::styled(text, selected())
                } else {
                    Span::raw(text)
                }
            };
            lines.push(Line::from(vec![
                button("Yes", *choice),
                Span::raw("   "),
                button("No", !*choice),
            ]));
            lines.push(Line::from("y yes · n no · Enter choose · Esc no"));
            boxed(frame, area, " Confirm ", lines, w);
        }
        Overlay::Choose {
            question,
            options,
            selected: at,
            ..
        } => {
            let w = 64u16.min(area.width.saturating_sub(4));
            let inner = usize::from(w) - 4;
            let asked = question_rows(question, inner, area.height);
            // The box: the question, a blank row, the choices, the keys, the border, and a
            // row's margin above and below.
            let room = usize::from(area.height)
                .saturating_sub(asked.len() + 7)
                .max(CHOICE_ROWS);
            let mut lines: Vec<Line<'static>> = asked.into_iter().map(Line::from).collect();
            lines.push(Line::default());
            let blocks: Vec<Vec<String>> = options
                .iter()
                .enumerate()
                .map(|(i, option)| choice_rows(i, option, inner))
                .collect();
            let heights: Vec<usize> = blocks.iter().map(Vec::len).collect();
            // From the first choice that keeps the selected one in view, whole.
            let mut first = *at;
            let mut used = heights.get(*at).copied().unwrap_or(0);
            while first > 0 && used + heights[first - 1] <= room {
                first -= 1;
                used += heights[first];
            }
            let shown = scrolled_windows(&heights, first, room);
            for (i, block) in blocks.into_iter().enumerate() {
                if !shown.contains(&i) {
                    continue;
                }
                for row in block {
                    let line = Line::from(row);
                    lines.push(if i == *at {
                        line.style(selected())
                    } else {
                        line
                    });
                }
            }
            lines.push(Line::from("↑ ↓ choose · Enter · Esc cancel"));
            boxed(frame, area, " Choose ", lines, w);
        }
        Overlay::Text { prompt, input, .. } => {
            let w = 64u16.min(area.width.saturating_sub(4));
            let inner = usize::from(w) - 4;
            let mut lines: Vec<Line<'static>> = question_rows(prompt, inner, area.height)
                .into_iter()
                .map(Line::from)
                .collect();
            let shown = tail(input, inner.saturating_sub(1));
            lines.push(Line::from(vec![
                Span::raw(shown),
                Span::styled(" ", selected()),
            ]));
            lines.push(Line::from("Enter · Esc cancel"));
            boxed(frame, area, " Input ", lines, w);
        }
        Overlay::Result {
            title,
            text,
            scroll,
        } => panel(
            frame,
            area,
            &format!(" {title} "),
            text,
            *scroll,
            "Esc close · ↑ ↓ scroll",
        ),
        Overlay::Doctor {
            text,
            scroll,
            online,
        } => {
            let title = if *online {
                " doctor --online "
            } else {
                " doctor "
            };
            panel(
                frame,
                area,
                title,
                text,
                *scroll,
                "o online · Esc close · ↑ ↓ scroll",
            );
        }
    }
}

/// Choice `i`'s rows, `w` columns each: `N. ` and its text, which wraps at word boundaries under
/// itself, onto `CHOICE_ROWS` at most; the rest is cut (`wrap_rows`).
fn choice_rows(i: usize, option: &str, w: usize) -> Vec<String> {
    let number = format!("{}. ", i + 1);
    let hang = width_of(&number);
    wrap_rows(option, w.saturating_sub(hang), CHOICE_ROWS)
        .into_iter()
        .enumerate()
        .map(|(row, text)| {
            let lead = if row == 0 {
                number.clone()
            } else {
                " ".repeat(hang)
            };
            pad(&format!("{lead}{text}"), w)
        })
        .collect()
}

/// The end of `s` that fits in `w` columns: what was typed last stays in view.
fn tail(s: &str, w: usize) -> String {
    let mut start = s.len();
    for (i, _) in s.char_indices().rev() {
        if width_of(&s[i..]) > w {
            break;
        }
        start = i;
    }
    s[start..].to_owned()
}

fn help_line(key: &str, what: &str) -> Line<'static> {
    Line::from(format!("  {}  {what}", pad(key, 20)))
}

/// `lines` in a bordered box `w` wide, as tall as they need, in the middle of `area`.
fn boxed(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>, w: u16) {
    let h = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2);
    let rect = centered(area, w.min(area.width), h);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(title.to_owned())
                .padding(ratatui::widgets::Padding::horizontal(1)),
        ),
        rect,
    );
}

/// The last row a panel's text can be scrolled to on a terminal `width` wide.
fn last_row(text: &str, width: u16) -> u16 {
    let rows = panel_rows(text, panel_width(width)).len();
    u16::try_from(rows.saturating_sub(1)).unwrap_or(u16::MAX)
}

/// The width of a panel's text on a terminal `width` wide: two columns of margin, a border and
/// a column of padding on either side.
fn panel_width(width: u16) -> usize {
    usize::from(width.saturating_sub(8))
}

/// `text` in rows of at most `width` columns: each line broken at word boundaries (`wrap`; a
/// word wider than a row breaks by width), its continuation rows indented as the line is. An
/// indent is a third of the width at most, so the words keep room.
fn panel_rows(text: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let body = line.trim_start_matches(' ');
        let indent = " ".repeat((line.len() - body.len()).min(width / 3));
        let room = width - indent.len();
        rows.extend(
            wrap(body, room)
                .into_iter()
                .map(|row| format!("{indent}{row}")),
        );
    }
    rows
}

/// A scrollable panel over most of `area`, its keys on its last line. Its text wraps at the
/// panel's width (`panel_rows`), and its scroll stops at its last row at that width.
fn panel(frame: &mut Frame, area: Rect, title: &str, text: &str, scroll: u16, keys: &str) {
    let scroll = scroll.min(last_row(text, area.width));
    let rect = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(title.to_owned())
        .title_bottom(Line::from(format!(" {keys} ")))
        .padding(ratatui::widgets::Padding::horizontal(1));
    let lines: Vec<Line<'static>> = panel_rows(text, panel_width(area.width))
        .into_iter()
        .map(Line::from)
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), rect);
}
```

In `crates/tagteam-tui/src/ask.rs`, above `#[cfg(test)]`, after the module line:

```rust
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

use tagteam_provider::Cancel;

use crate::host::Ask;
use crate::msg::{Answer, Msg, Prompt, PromptKind};

/// How long the lane waits for an answer before it looks at the shutdown token again.
#[allow(dead_code)]
const SLICE: Duration = Duration::from_millis(100);

/// Task 11's runtime builds one for each action; until then only the tests do.
#[allow(dead_code)]
pub(crate) struct ChannelAsk {
    tx: Sender<Msg>,
    shutdown: Cancel,
}

#[allow(dead_code)]
impl ChannelAsk {
    /// Prompts go to the UI thread on `tx`; once `shutdown` is set, every prompt is declined.
    pub(crate) fn new(tx: Sender<Msg>, shutdown: Cancel) -> Self {
        ChannelAsk { tx, shutdown }
    }

    /// The answer to `question`, or `None` for a decline: the shutdown has begun (before the
    /// prompt, or while it waits), or the UI thread is gone.
    fn ask(&self, question: &str, kind: PromptKind) -> Option<Answer> {
        if self.shutdown.requested().is_some() {
            return None;
        }
        let (reply, answer) = mpsc::channel();
        let prompt = Prompt {
            question: question.to_owned(),
            kind,
            reply,
        };
        self.tx.send(Msg::Prompt(prompt)).ok()?;
        loop {
            match answer.recv_timeout(SLICE) {
                Ok(a) if self.shutdown.requested().is_none() => return Some(a),
                Ok(_) | Err(RecvTimeoutError::Disconnected) => return None,
                Err(RecvTimeoutError::Timeout) => {
                    if self.shutdown.requested().is_some() {
                        return None;
                    }
                }
            }
        }
    }
}

impl Ask for ChannelAsk {
    fn confirm(&mut self, question: &str, default_yes: bool) -> bool {
        matches!(
            self.ask(question, PromptKind::Confirm { default_yes }),
            Some(Answer::Confirm(true))
        )
    }

    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize> {
        match self.ask(
            question,
            PromptKind::Choose {
                options: options.to_vec(),
            },
        ) {
            Some(Answer::Choose(choice)) => choice.filter(|i| *i < options.len()),
            _ => None,
        }
    }
}
```

`ChannelAsk` has no caller outside its tests until Task 11's runtime builds one per action, so
its three items carry `#[allow(dead_code)]`. Task 11 removes them.

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-tui --lib -- actions:: ask::`
Expected: PASS, 29 tests.

Run: `cargo test -p tagteam-tui`
Expected: PASS: 114 library tests and 9 in `source`.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-tui/src/actions.rs crates/tagteam-tui/src/ask.rs
git commit -m "Run the CLI's actions from the TUI with their dialogs and panels"
```

---

### Task 10: `CliHost`

The TUI's `Host` (C3.1) lives in the binary, because only the binary has the CLI's command
path. Every action runs `tagteam <Action::args()>` through the CLI's own `clap` definition and
`app::run`, so its prompts, refusals, messages and exit code are the CLI's by construction
(spec §1.1 criterion 5). Inside the TUI (`act`, on the actions lane) the output is captured and
the prompts go to the TUI's dialogs through `Ask`. Once the TUI has handed the terminal back
(`act_foreground`, on the UI thread), the action runs on the terminal as the CLI would, and
`Run` is a `tagteam run` process of its own.

**Readings of the spec this task commits to:**
- **`act` never runs `Run`.** In-process, `run` would `exec` the agent in the TUI's place. `act`
  returns `NeedsTerminal` for any action whose `needs_terminal()` holds, before parsing anything.
- **The bridge's prompter is a person's** (`interactive()` is true), because a person answers
  in the TUI's dialogs. So every CLI prompt that a terminal would get reaches `Ask`, with its
  question and default exactly as `TtyPrompter` would show them. The one exception is
  `app::UNLOCK_QUESTION`: it is declined without asking and noted, the CLI then fails with
  `keychain-locked` before touching anything (Appendix A.3), and `act` returns `NeedsTerminal`,
  discarding the captured refusal. `secret` is declined, and `reaches_terminal()` is false, since
  no dialog takes a secret.
- **A signal recorded before the hand-over runs nothing.** If the signal token already holds a
  signal when `act_foreground` starts, nothing is spawned or run, no `Press Enter` is shown, and
  the outcome is `Done { exit: 128 + n, out: "", err: "tagteam: interrupted\n" }`. The token is
  left as it is, for the runtime (Task 11) to act on.
- **The wait-and-forward loop is `run`'s own** (R6). `run::wait` already polls `try_wait`
  (`waitpid` with `WNOHANG`) every 50 ms, takes the token between slices, forwards SIGTERM and
  SIGHUP, and drops SIGINT (the terminal delivers it to the foreground group itself: core §12.5,
  Decision 1). It changes only to return the last signal it forwarded. The host records that
  signal again once the child is reaped, so the TUI shuts down after it (spec §7.3: "once the
  action has returned they stay recorded").
- **The spawn is `run`'s own primitive**, `spawn_session(&spec, None)`: the caller's process
  group, which is the terminal's foreground group, and stdio inherited (core §14.1:
  interactive children stay in the foreground group). No reservation fd is passed: the child
  `tagteam run` makes its own.
- **`Press Enter`** (R5): after the action, a SIGINT in the signal token was the action's (typed
  at its prompt, or in its session), so it is spent before the wait, or the wait would end at
  once. It is spent with `Cancel::take_if(SIGINT)` (R15), which this task adds: a check with
  `requested()` and then `take()` would consume a SIGTERM or SIGHUP that replaced the SIGINT in
  between, and wait for Enter instead of ending the TUI. A SIGTERM or SIGHUP means no wait at
  all. Otherwise
  `Press Enter to return to tagteam` goes to stderr, with no newline (the Enter the person types
  ends the line), and `prompt::read_terminal_line` waits for it, in slices the token ends.
- **A foreground outcome carries no text.** What the action printed is on the terminal already,
  so `act_foreground` returns `Done { exit, out: "", err: "" }`.
- **`tui_host` is a public module**, and `CliHost` and `CliHost::new` are `pub`, as `app`,
  `cli` and `prompt` are: the tests in `crates/tagteam/tests/tui_host.rs` are a crate of their
  own (contract C9). A third private field, `exe: PathBuf`, holds what `new` is given.

**Depends on:** 2, 3

**Files:**
- Create: `crates/tagteam/src/tui_host.rs`
- Modify: `crates/tagteam/src/lib.rs` (the module list, lines 7–24: add `pub mod tui_host;` after
  `mod transfer_cmd;`)
- Modify: `crates/tagteam/src/app.rs` (line 87, as Task 2 left it: `UNLOCK_QUESTION` becomes
  `pub(crate)`)
- Modify: `crates/tagteam/src/run.rs` (lines 137–169: `forward` returns the signal it forwarded,
  and `wait` becomes `pub(crate)` and returns the last one; `run_session`, line 76, takes the
  pair)
- Modify: `crates/tagteam-provider/src/cancel.rs` (`take`, lines 31–45: its doc names the TUI,
  and `take_if` follows it; two tests before `an_interruption_names_its_signal`, line 127)
- Test: `crates/tagteam/tests/tui_host.rs`

**Interfaces:**
- Consumes:
  - Task 2: `Context: Clone`, the hidden `--expect-account` flag, and the refusal
    `tagteam: the selected account changed; nothing was done` (kind `stale-selection`, exit 1).
  - Task 3: `tagteam_tui::host::{AccountRef, Action, ActionOutcome, Ask, Host}`,
    `Action::args`, `Action::needs_terminal`.
  - Existing: `app::{run, Context, Io, EXIT_ERROR, EXIT_USAGE}`, `app::UNLOCK_QUESTION`,
    `cli::Cli`, `prompt::{Prompter, TtyPrompter, read_terminal_line}`,
    `run::{wait, forward}`, `tagteam_provider::process::{SpawnSpec, spawn_session, exit_code}`,
    `tagteam_provider::Cancel::{requested, request, take}`.
  - Test helpers in `crates/tagteam/tests/common/mod.rs`: `seed_home`, `login`, `live_email`,
    `cc_profile`, `hold_launch`, `install_script`, `OFFLINE_API_BASE`, `LOCKED`.
- Produces (C9):
  - `pub struct CliHost` (fields `inside: Context`, `foreground: Context`, `exe: PathBuf`).
  - `pub fn CliHost::new(ctx: &Context, shutdown: Cancel, exe: PathBuf) -> CliHost`.
  - `impl tagteam_tui::host::Host for CliHost`.
  - `pub const RETURN_PROMPT: &str = "Press Enter to return to tagteam";`
  - `pub(crate) fn run::wait(child: &mut Child, cancel: &Cancel) -> (io::Result<ExitStatus>, Option<i32>)`.
  - `pub fn tagteam_provider::Cancel::take_if(&self, signal: i32) -> bool` (R15), for this task's
    wait for Enter and Task 11's suspension.

**Spec:**
- spec §4.5: `act` parses the arguments with the CLI's `clap` definition and runs the CLI's
  dispatch with two capture buffers and a prompter that forwards `confirm` and `choose` to
  `ask`, always with `--no-color`; `secret` is declined; the unlock question is declined and
  noted, and `act` returns `NeedsTerminal`; `act_foreground` runs it with `TtyPrompter` and the
  real stdout and stderr; `act` runs with the shutdown token, `act_foreground` with the signal
  token.
- spec §7.3 steps 3–4 and "Signals while suspended": `run` is `<current executable> --provider P
  --expect-account <ID> run <position>` in the terminal's foreground group, waited for in
  `waitpid(WNOHANG)` slices with the signal token checked between them; SIGTERM and SIGHUP are
  forwarded as they arrive and stay recorded; a SIGINT is the action's; then
  `Press Enter to return to tagteam`, read with `read_terminal_line`.
- spec §10.2 step 3: once the shutdown token is set, prompts are declined, and the CLI reports
  the command as interrupted.
- spec §11 "Host bridge": the tests below, one per item.
- core §12.5 "Signals": while the child runs, the parent ignores SIGINT and SIGQUIT and forwards
  SIGTERM and SIGHUP.
- core §14.1 "Child processes": interactive children stay in the terminal's foreground group.
- R5 (the `Press Enter` wait is the host's) and R6 (reuse the provider's process primitives).

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam/tests/tui_host.rs`. It runs in-process, as `tests/app.rs` does, so it
needs no feature. Its harness is its own (`tests/app.rs`'s `H` and `Scripted` are private to
that file); `tests/common/mod.rs` gains nothing.

```rust
//! The TUI's `Host` over the CLI's command path (TUI spec §4.5, §11 "Host bridge"). Each action
//! runs the CLI's own command in-process, as `tests/app.rs` runs them, with its prompts
//! answered by a scripted `Ask` standing in for the TUI's dialogs. `act_foreground`'s session
//! runs a stand-in for `tagteam run` here; the real one, on a terminal, is `tui_pty.rs`'s.

mod common;

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use common::{LOCKED, cc_profile, hold_launch, install_script, live_email, login, seed_home};
use tagteam::app::{self, Context, Io};
use tagteam::cli::Cli;
use tagteam::prompt::Prompter;
use tagteam::tui_host::CliHost;
use tagteam_cc::live::Platform;
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::store::{AccountRow, Store};
use tagteam_provider::{Cancel, Env, FakeKeychain};
use tagteam_tui::host::{AccountRef, Action, ActionOutcome, Ask, Host};

const SWITCHED_TO_A: &str = "Switched to a@x.co (position 1).\nClaude Code picks this up within about 30 s; restart it to apply now.\n";
const ADD_STRANGER_FIRST: &str = "Add the current login (stranger@x.co) first?";
const STALE: &str = "tagteam: the selected account changed; nothing was done\n";
const INTERRUPTED: &str = "tagteam: interrupted\n";
/// What `add` says when every endpoint is offline: the oracle could not attribute the login.
const UNVERIFIED: &str = "note: could not verify that the live credential belongs to";

/// A caller that is not a person: any prompt fails the test.
struct Nobody;

impl Prompter for Nobody {
    fn interactive(&self) -> bool {
        false
    }
    fn confirm(&mut self, q: &str, _default_yes: bool) -> bool {
        panic!("unexpected prompt: {q}")
    }
    fn choose(&mut self, q: &str, _options: &[String]) -> Option<usize> {
        panic!("unexpected prompt: {q}")
    }
    fn secret(&mut self, q: &str) -> Option<String> {
        panic!("unexpected prompt: {q}")
    }
}

/// The TUI's dialogs, scripted. Each `confirm` records its question and default, and takes the
/// next answer. Like the TUI's own `ChannelAsk`, a dialog asked for once the shutdown token is
/// set declines at once, unshown (spec §10.2 step 3). With `quit_while_open`, the person
/// presses `q` while the dialog is open: the shutdown token is set, and the dialog declines.
struct Dialogs {
    answers: VecDeque<bool>,
    asked: Vec<(String, bool)>,
    shutdown: Option<Cancel>,
    quit_while_open: bool,
}

impl Dialogs {
    fn none() -> Dialogs {
        Dialogs::answering(&[])
    }

    fn answering(answers: &[bool]) -> Dialogs {
        Dialogs {
            answers: answers.iter().copied().collect(),
            asked: Vec::new(),
            shutdown: None,
            quit_while_open: false,
        }
    }

    /// Dialogs that watch `shutdown`, as the TUI's do, answering `answers`.
    fn watching(shutdown: &Cancel, answers: &[bool]) -> Dialogs {
        Dialogs {
            shutdown: Some(shutdown.clone()),
            ..Dialogs::answering(answers)
        }
    }
}

impl Ask for Dialogs {
    fn confirm(&mut self, question: &str, default_yes: bool) -> bool {
        if let Some(shutdown) = &self.shutdown {
            if shutdown.requested().is_some() {
                return false;
            }
        }
        self.asked.push((question.to_owned(), default_yes));
        if self.quit_while_open {
            if let Some(shutdown) = &self.shutdown {
                shutdown.request(libc::SIGINT);
            }
            return false;
        }
        self.answers
            .pop_front()
            .unwrap_or_else(|| panic!("unexpected dialog: {question}"))
    }

    fn choose(&mut self, question: &str, _options: &[String]) -> Option<usize> {
        panic!("unexpected choice: {question}")
    }
}

/// An outcome's exit code and captured output. `NeedsTerminal` fails the test.
fn done(outcome: ActionOutcome) -> (i32, String, String) {
    match outcome {
        ActionOutcome::Done { exit, out, err } => (exit, out, err),
        ActionOutcome::NeedsTerminal => panic!("the action asked for the terminal"),
    }
}

/// A home where Claude Code has run, its in-memory Keychain, and the CLI run in-process on
/// it, as `tests/app.rs`'s `H` runs it.
struct Home {
    dir: tempfile::TempDir,
    env: Env,
    kc: Arc<FakeKeychain>,
}

impl Home {
    fn new() -> Home {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        seed_home(&env);
        Home {
            dir,
            env,
            kc: Arc::new(FakeKeychain::new()),
        }
    }

    /// `a@x.co` at position 1 and `b@x.co` at position 2, the live login.
    fn with_two() -> Home {
        let home = Home::new();
        home.login("a@x.co", "rt-a");
        home.cli(&["add"]);
        home.login("b@x.co", "rt-b");
        home.cli(&["add"]);
        home
    }

    /// `a@x.co` stored, and live a login no account holds.
    fn with_unmanaged_login() -> Home {
        let home = Home::new();
        home.login("a@x.co", "rt-a");
        home.cli(&["add"]);
        home.login("stranger@x.co", "rt-s");
        home
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn login(&self, email: &str, rt: &str) {
        login(&self.env, &*self.kc, email, "", rt);
    }

    /// The context the binary builds for this home, its output not a terminal and every
    /// endpoint offline. `env.cancel` is the home's own token: the signal token.
    fn context(&self) -> Context {
        Context {
            env: self.env.clone(),
            keychain: self.kc.clone(),
            vault_keychain: None,
            platform: Platform::MacOs,
            api_base: Some(common::OFFLINE_API_BASE.into()),
            stdout_terminal: false,
            no_color_env: false,
            force_color_env: false,
        }
    }

    /// `tagteam <args>` in-process, with nobody to answer a prompt.
    fn cli_run(&self, args: &[&str]) -> (i32, String, String) {
        let cli =
            Cli::try_parse_from(std::iter::once("tagteam").chain(args.iter().copied())).unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = app::run(
            cli,
            self.context(),
            &mut Io {
                out: &mut out,
                err: &mut err,
                prompter: &mut Nobody,
            },
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    /// A test's setup step through the CLI, which must succeed.
    fn cli(&self, args: &[&str]) {
        let (code, _, err) = self.cli_run(args);
        assert_eq!(code, 0, "{args:?}: {err}");
    }

    /// The host the TUI would get for this home, and its shutdown token. Nothing here spawns
    /// `Run`, so `exe` names nothing.
    fn host(&self) -> (CliHost, Cancel) {
        let shutdown = Cancel::new();
        let host = CliHost::new(
            &self.context(),
            shutdown.clone(),
            PathBuf::from("/nonexistent/tagteam"),
        );
        (host, shutdown)
    }

    /// The stored accounts, in position order; none without a store.
    fn rows(&self) -> Vec<AccountRow> {
        Store::open_existing(&self.env.data_dir().join("tagteam.db"))
            .unwrap()
            .map(|s| s.accounts(&ProviderId::new(CLAUDE_CODE)).unwrap())
            .unwrap_or_default()
    }

    /// The account at `position`, as the TUI read it.
    fn at(&self, position: u32) -> AccountRef {
        let row = self
            .rows()
            .into_iter()
            .find(|r| r.position == position)
            .unwrap();
        AccountRef {
            id: row.id,
            provider: row.provider,
            position,
        }
    }

    /// The stored account `id`, as it is now.
    fn row(&self, id: &AccountId) -> Option<AccountRow> {
        self.rows().into_iter().find(|r| &r.id == id)
    }
}

#[test]
fn a_switch_runs_the_cli_s_switch_and_captures_its_words() {
    let home = Home::with_two();
    let (host, _) = home.host();
    let mut dialogs = Dialogs::none();
    assert_eq!(
        done(host.act(&Action::Switch(home.at(1)), &mut dialogs)),
        (0, SWITCHED_TO_A.to_owned(), String::new())
    );
    assert!(dialogs.asked.is_empty());
    assert_eq!(live_email(home.root()), "a@x.co");
}

#[test]
fn the_offer_to_add_an_unmanaged_login_is_a_dialog_and_a_yes_adds_it_first() {
    let home = Home::with_unmanaged_login();
    let (host, _) = home.host();
    let mut dialogs = Dialogs::answering(&[true]);
    let (code, out, err) = done(host.act(&Action::Switch(home.at(1)), &mut dialogs));
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        format!("Added stranger@x.co at position 2.\n{SWITCHED_TO_A}")
    );
    assert_eq!(err, format!("{UNVERIFIED} stranger@x.co\n"));
    assert_eq!(dialogs.asked, [(ADD_STRANGER_FIRST.to_owned(), true)]);
    assert_eq!(home.rows().len(), 2);
    assert_eq!(live_email(home.root()), "a@x.co");
}

#[test]
fn declining_the_offer_cancels_as_the_cli_does_and_adds_nothing() {
    let home = Home::with_unmanaged_login();
    let (host, _) = home.host();
    let mut dialogs = Dialogs::answering(&[false]);
    assert_eq!(
        done(host.act(&Action::Switch(home.at(1)), &mut dialogs)),
        (1, String::new(), "tagteam: cancelled\n".to_owned())
    );
    assert_eq!(dialogs.asked, [(ADD_STRANGER_FIRST.to_owned(), true)]);
    assert_eq!(home.rows().len(), 1);
    assert_eq!(live_email(home.root()), "stranger@x.co");
}

#[test]
fn an_account_in_a_session_is_refused_as_the_cli_refuses_it() {
    let home = Home::with_two();
    let a = home.at(1);
    let (profile, _) = cc_profile(home.root(), a.id.as_str());
    let _session = hold_launch(&profile);
    let (host, _) = home.host();
    for action in [Action::Switch(a.clone()), Action::Remove(a.clone())] {
        assert_eq!(
            done(host.act(&action, &mut Dialogs::none())),
            (
                1,
                String::new(),
                "tagteam: position 1 (a@x.co) is in use by a `tagteam run` session; exit that session, then retry\n"
                    .to_owned()
            ),
            "{action:?}"
        );
    }
    assert_eq!(live_email(home.root()), "b@x.co");
    assert!(home.row(&a.id).is_some());
}

#[test]
fn enable_and_disable_mark_the_account_as_the_cli_does() {
    let home = Home::with_two();
    let (host, _) = home.host();
    let a = home.at(1);
    assert_eq!(
        done(host.act(&Action::Disable(a.clone()), &mut Dialogs::none())),
        (0, "a@x.co is disabled.\n".to_owned(), String::new())
    );
    assert!(home.row(&a.id).unwrap().disabled);
    assert_eq!(
        done(host.act(&Action::Enable(a.clone()), &mut Dialogs::none())),
        (0, "a@x.co is enabled.\n".to_owned(), String::new())
    );
    assert!(!home.row(&a.id).unwrap().disabled);
}

#[test]
fn an_alias_is_set_and_cleared_and_one_that_starts_with_a_dash_meets_the_cli_s_own_check() {
    let home = Home::with_two();
    let (host, _) = home.host();
    let a = home.at(1);
    let set = |alias: &str| Action::SetAlias {
        account: a.clone(),
        alias: alias.to_owned(),
    };
    assert_eq!(
        done(host.act(&set("work"), &mut Dialogs::none())),
        (0, "Position 1 is now work.\n".to_owned(), String::new())
    );
    assert_eq!(home.row(&a.id).unwrap().alias.as_deref(), Some("work"));
    assert_eq!(
        done(host.act(&Action::ClearAlias(a.clone()), &mut Dialogs::none())),
        (
            0,
            "Position 1 has no alias now.\n".to_owned(),
            String::new()
        )
    );
    assert_eq!(home.row(&a.id).unwrap().alias, None);
    // C3.1's `--`: the name reaches the engine as a name, never as a flag, and is refused there.
    assert_eq!(
        done(host.act(&set("-x"), &mut Dialogs::none())),
        (
            1,
            String::new(),
            "tagteam: an alias cannot start with '-'\n".to_owned()
        )
    );
}

#[test]
fn a_move_takes_the_account_to_its_new_position() {
    let home = Home::with_two();
    let (host, _) = home.host();
    let a = home.at(1);
    let to_2 = Action::Move {
        account: a.clone(),
        position: 2,
    };
    assert_eq!(
        done(host.act(&to_2, &mut Dialogs::none())),
        (
            0,
            "a@x.co is now at position 2.\n".to_owned(),
            String::new()
        )
    );
    assert_eq!(home.row(&a.id).unwrap().position, 2);
}

#[test]
fn remove_deletes_the_stored_account_and_claude_code_keeps_its_login() {
    // Core §10.3: removing the live account forgets it; Claude Code's own login stays.
    let home = Home::with_two();
    let (host, _) = home.host();
    let b = home.at(2);
    assert_eq!(
        done(host.act(&Action::Remove(b.clone()), &mut Dialogs::none())),
        (
            0,
            "Removed b@x.co (position 2).\n".to_owned(),
            String::new()
        )
    );
    assert!(home.row(&b.id).is_none());
    assert_eq!(live_email(home.root()), "b@x.co");
}

#[test]
fn remove_refuses_to_delete_what_the_live_login_lives_in() {
    // Core §10.3, §10.5: the environment names account a's profile as Claude Code's secure
    // storage, so deleting the profile would break the live login. Nothing is deleted.
    let home = Home::with_two();
    let a = home.at(1);
    let (_profile, spelling) = cc_profile(home.root(), a.id.as_str());
    let mut ctx = home.context();
    ctx.env.claude_securestorage_config_dir = Some(spelling.into());
    let host = CliHost::new(&ctx, Cancel::new(), PathBuf::from("/nonexistent/tagteam"));
    assert_eq!(
        done(host.act(&Action::Remove(a.clone()), &mut Dialogs::none())),
        (
            1,
            String::new(),
            "tagteam: the live login's files are inside what this would delete, since the environment names it; nothing was deleted (tagteam never deletes the live login)\n"
                .to_owned()
        )
    );
    assert!(home.row(&a.id).is_some());
}

#[test]
fn add_stores_the_live_login() {
    let home = Home::new();
    home.login("a@x.co", "rt-a");
    let (host, _) = home.host();
    let add = Action::AddLive {
        provider: ProviderId::new(CLAUDE_CODE),
    };
    assert_eq!(
        done(host.act(&add, &mut Dialogs::none())),
        (
            0,
            "Added a@x.co at position 1.\n".to_owned(),
            format!("{UNVERIFIED} a@x.co\n")
        )
    );
    assert_eq!(home.rows().len(), 1);
}

#[test]
fn an_account_moved_elsewhere_is_refused_as_stale_and_nothing_changes() {
    // Spec §4.5: another process moved `a` after the TUI read it; position 1 now holds `b`.
    let home = Home::with_two();
    let a = home.at(1);
    home.cli(&["move", "1", "2"]);
    let (host, _) = home.host();
    for action in [
        Action::Switch(a.clone()),
        Action::Remove(a.clone()),
        Action::Disable(a.clone()),
    ] {
        assert_eq!(
            done(host.act(&action, &mut Dialogs::none())),
            (1, String::new(), STALE.to_owned()),
            "{action:?}"
        );
    }
    assert_eq!(live_email(home.root()), "b@x.co");
    let rows = home.rows();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| !r.disabled));
    assert_eq!(home.row(&a.id).unwrap().position, 2);
}

#[test]
fn a_locked_keychain_hands_the_action_to_the_terminal_with_nothing_written() {
    // Appendix A.3: macOS asks for the password on the terminal itself. The CLI with nobody to
    // ask refuses outright; the TUI's bridge declines the question unshown, and the action is
    // run again on the terminal (spec §7.3).
    let home = Home::with_two();
    let a = home.at(1);
    home.kc.set_locked(true);
    assert_eq!(
        home.cli_run(&["switch", "1"]),
        (1, String::new(), format!("tagteam: {LOCKED}\n"))
    );
    let (host, _) = home.host();
    for action in [
        Action::Switch(a.clone()),
        Action::Remove(a.clone()),
        Action::AddLive {
            provider: ProviderId::new(CLAUDE_CODE),
        },
    ] {
        let mut dialogs = Dialogs::none();
        assert!(
            matches!(
                host.act(&action, &mut dialogs),
                ActionOutcome::NeedsTerminal
            ),
            "{action:?}"
        );
        assert!(
            dialogs.asked.is_empty(),
            "the unlock question is never a dialog"
        );
    }
    assert_eq!(home.kc.unlock_attempts(), 0);
    assert_eq!(live_email(home.root()), "b@x.co");
    assert_eq!(home.rows().len(), 2);
}

#[test]
fn a_shutdown_before_the_prompt_declines_it_unshown_and_the_command_is_interrupted() {
    // Spec §10.2 step 3, §11: the shutdown begins just before the offer would show.
    let home = Home::with_unmanaged_login();
    let (host, shutdown) = home.host();
    shutdown.request(libc::SIGINT);
    let mut dialogs = Dialogs::watching(&shutdown, &[true]);
    assert_eq!(
        done(host.act(&Action::Switch(home.at(1)), &mut dialogs)),
        (130, String::new(), INTERRUPTED.to_owned())
    );
    assert!(dialogs.asked.is_empty());
    assert_eq!(home.rows().len(), 1, "nothing was added");
    assert_eq!(live_email(home.root()), "stranger@x.co");
}

#[test]
fn q_while_a_dialog_is_open_declines_it_and_the_command_is_interrupted() {
    let home = Home::with_unmanaged_login();
    let (host, shutdown) = home.host();
    let mut dialogs = Dialogs {
        quit_while_open: true,
        ..Dialogs::watching(&shutdown, &[])
    };
    assert_eq!(
        done(host.act(&Action::Switch(home.at(1)), &mut dialogs)),
        (130, String::new(), INTERRUPTED.to_owned())
    );
    assert_eq!(dialogs.asked, [(ADD_STRANGER_FIRST.to_owned(), true)]);
    assert_eq!(home.rows().len(), 1);
}

/// A stand-in for `tagteam run`, at `<root>/bin/tagteam`. It records its arguments, then each
/// SIGHUP, SIGINT and SIGTERM it catches, in `<root>/stub.out`, and runs for up to 10 s. A
/// SIGHUP or SIGTERM ends it with 128 + its number, as `run` ends with its session's code.
const STUB: &str = r#"#!/bin/sh
out='@OUT@'
for a in "$@"; do printf 'arg %s\n' "$a" >> "$out"; done
sleeper=
caught() {
  printf 'signal %s\n' "$1" >> "$out"
  if [ "$1" != 2 ]; then
    if [ -n "$sleeper" ]; then kill "$sleeper" 2>/dev/null; fi
    exit $((128 + $1))
  fi
}
trap 'caught 1' HUP
trap 'caught 2' INT
trap 'caught 15' TERM
sleep 10 &
sleeper=$!
echo ready >> "$out"
while kill -0 "$sleeper" 2>/dev/null; do wait "$sleeper"; done
echo exit >> "$out"
"#;

/// Installs `STUB` under `root`: its path, and the file it records in.
fn stub_run(root: &Path) -> (PathBuf, PathBuf) {
    let out = root.join("stub.out");
    let exe = root.join("bin").join("tagteam");
    fs::create_dir_all(root.join("bin")).unwrap();
    install_script(&exe, &STUB.replace("@OUT@", &out.display().to_string()));
    (exe, out)
}

/// The stand-in's arguments, and the signals it caught, by number.
fn recorded(out: &Path) -> (Vec<String>, Vec<String>) {
    let text = fs::read_to_string(out).unwrap_or_default();
    let pick = |key: &str| {
        text.lines()
            .filter_map(|l| l.strip_prefix(key))
            .map(str::to_owned)
            .collect()
    };
    (pick("arg "), pick("signal "))
}

/// Requests `signals` in `token` in turn, 300 ms apart (six of `run::wait`'s looks), once the
/// stand-in is running: a `kill` of the TUI, as its handler records it.
fn signal_once_ready(out: &Path, token: &Cancel, signals: &[i32]) -> thread::JoinHandle<()> {
    let (out, token, signals) = (out.to_owned(), token.clone(), signals.to_vec());
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !fs::read_to_string(&out)
            .unwrap_or_default()
            .contains("ready\n")
        {
            assert!(Instant::now() < deadline, "the stand-in never started");
            thread::sleep(Duration::from_millis(10));
        }
        for signal in signals {
            token.request(signal);
            thread::sleep(Duration::from_millis(300));
        }
    })
}

#[test]
fn run_starts_tagteam_run_on_the_account_and_a_sigterm_is_forwarded_and_kept() {
    // Spec §7.3: the SIGTERM stays recorded, so the TUI shuts down without waiting for Enter.
    let home = Home::with_two();
    let (exe, out) = stub_run(home.root());
    let signal = home.env.cancel.clone();
    let host = CliHost::new(&home.context(), Cancel::new(), exe);
    let a = home.at(1);
    let sender = signal_once_ready(&out, &signal, &[libc::SIGTERM]);
    let (code, _, _) = done(host.act_foreground(&Action::Run(a.clone())));
    sender.join().unwrap();
    assert_eq!(code, 143);
    let (args, signals) = recorded(&out);
    assert_eq!(
        args,
        [
            "--provider",
            "claude-code",
            "--expect-account",
            a.id.as_str(),
            "run",
            "1"
        ]
    );
    assert_eq!(signals, ["15"], "forwarded once");
    assert_eq!(signal.requested(), Some(libc::SIGTERM));
}

#[test]
fn a_sigint_during_the_session_is_the_session_s_and_a_sighup_is_forwarded() {
    // Core §12.5: the terminal sends Ctrl-C to the session itself, so the TUI never forwards
    // it, and spends it.
    let home = Home::with_two();
    let (exe, out) = stub_run(home.root());
    let signal = home.env.cancel.clone();
    let host = CliHost::new(&home.context(), Cancel::new(), exe);
    let sender = signal_once_ready(&out, &signal, &[libc::SIGINT, libc::SIGHUP]);
    let (code, _, _) = done(host.act_foreground(&Action::Run(home.at(1))));
    sender.join().unwrap();
    assert_eq!(code, 129);
    assert_eq!(recorded(&out).1, ["1"], "the SIGINT never reached it");
    assert_eq!(signal.requested(), Some(libc::SIGHUP));
}

#[test]
fn a_signal_recorded_before_the_hand_over_runs_nothing_and_never_waits() {
    let home = Home::with_two();
    let (exe, out) = stub_run(home.root());
    let signal = home.env.cancel.clone();
    let host = CliHost::new(&home.context(), Cancel::new(), exe);
    for n in [libc::SIGTERM, libc::SIGINT] {
        signal.request(n);
        assert_eq!(
            done(host.act_foreground(&Action::Run(home.at(1)))),
            (128 + n, String::new(), INTERRUPTED.to_owned())
        );
        assert_eq!(signal.take(), Some(n), "left for the TUI to act on");
    }
    assert!(!out.exists(), "nothing was spawned");
}

#[test]
fn run_never_runs_inside_the_tui() {
    // In-process, `run` would exec the agent in the TUI's place: it is always suspended.
    let home = Home::with_two();
    let (exe, out) = stub_run(home.root());
    let host = CliHost::new(&home.context(), Cancel::new(), exe);
    let mut dialogs = Dialogs::none();
    assert!(matches!(
        host.act(&Action::Run(home.at(1)), &mut dialogs),
        ActionOutcome::NeedsTerminal
    ));
    assert!(!out.exists());
}
```

None of these tests reaches `Press Enter` with no signal recorded, so none reads the
developer's terminal: the three `act_foreground` tests end with SIGTERM, SIGHUP, or a signal
recorded before the hand-over. Never add one that does: under `cargo test` stdin may be the
developer's terminal.

In `crates/tagteam-provider/src/cancel.rs`'s `tests` module, before
`an_interruption_names_its_signal` (line 127), add (R15):

```rust
    #[test]
    fn take_if_consumes_only_the_signal_it_names_for_every_clone() {
        let c = Cancel::new();
        let clone = c.clone();
        assert!(!c.take_if(2), "nothing is recorded yet");
        clone.request(2);
        assert!(c.take_if(2));
        assert_eq!(clone.requested(), None);
        assert!(!clone.take_if(2), "a signal is taken once");
        c.request(15);
        assert!(!c.take_if(2), "another signal is left as it is");
        assert_eq!(clone.requested(), Some(15));
    }

    #[test]
    fn spending_a_sigint_never_consumes_a_signal_that_replaced_it() {
        // The TUI's interleaving (TUI spec §7.3): a foreground action's SIGINT is seen, then a
        // SIGTERM replaces it before it is spent. The SIGTERM must stay, to end the TUI.
        let c = Cancel::new();
        c.request(2);
        assert_eq!(c.requested(), Some(2));
        c.request(15);
        assert!(!c.take_if(2));
        assert_eq!(c.requested(), Some(15));
    }
```

The second is the interleaving R15 guards against, step by step: no test can stop the TUI
between its own two looks at the token, so the guard is the one-step `take_if` itself.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --test tui_host`
Expected: FAIL to compile: `unresolved import tagteam::tui_host`.

Run: `cargo test -p tagteam-provider --lib cancel::`
Expected: FAIL to compile: `no method named take_if found for struct cancel::Cancel`.

- [ ] **Step 3: Implement**

In `crates/tagteam/src/app.rs`, line 87, make the question visible to the bridge:

```rust
/// Appendix A.3. The default keychain is the login keychain, so the hint names its file.
pub(crate) const UNLOCK_QUESTION: &str =
    "The login keychain is locked (common over SSH). Unlock it now?";
```

In `crates/tagteam/src/run.rs`, replace `forward` and `wait` (lines 137–169) with:

```rust
/// Sends `signal`, when `forwarded` says so, to the child `pid`. Returns the signal when it
/// was one to forward.
fn forward(pid: u32, signal: Option<i32>, first: bool) -> Option<i32> {
    let signal = signal.filter(|&s| forwarded(s, first))?;
    // SAFETY: kill(2) reads no memory of ours. `pid` is this process's child, which only
    // `wait` reaps, and nothing is sent once it has: the pid names the child or its zombie,
    // never a process that reused the number.
    let rc = unsafe { libc::kill(pid as libc::pid_t, signal) };
    if rc != 0 {
        tracing::warn!(
            signal,
            "could not forward the signal to the session: {}",
            io::Error::last_os_error()
        );
    }
    Some(signal)
}

/// Waits for the child, forwarding what the token records meanwhile (Decision 1), and returns
/// its status with the last signal forwarded to it. `run`'s wait for `claude`, and the TUI's for
/// the `tagteam run` it starts (TUI spec §7.3). Nothing here waits on a lock (B.37).
pub(crate) fn wait(child: &mut Child, cancel: &Cancel) -> (io::Result<ExitStatus>, Option<i32>) {
    let pid = child.id();
    let mut last = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return (Ok(status), last),
            Ok(None) => {}
            // No status to poll for: wait it out; it is still the child's exit that ends the run.
            Err(_) => return (child.wait(), last),
        }
        thread::sleep(POLL);
        last = forward(pid, cancel.take(), false).or(last);
    }
}
```

and in `run_session`, replace line 76 (`let status = wait(&mut child, cancel);`) with:

```rust
    let (status, _forwarded) = wait(&mut child, cancel);
```

Line 75's `forward(child.id(), cancel.take(), true);` stays as it is: its result is not needed.

In `crates/tagteam-provider/src/cancel.rs`, replace `take` (lines 31–45), its doc comment
included, with `take` and `take_if`:

```rust
    /// Consumes the recorded signal (§12.5's forwarding): returns it and leaves the token unset,
    /// for every clone. Only `tagteam run`, and the TUI around the `tagteam run` it starts, take
    /// (M4b Decision 1, TUI spec §7.3):
    /// - `run`'s last look before the spawn, where a signal ends the launch;
    /// - its wait loop, which forwards what arrived while `claude` runs; the TUI's wait for
    ///   `tagteam run` is the same loop;
    /// - once `claude` has exited, to clear what forwarding left;
    /// - `abandon`, before a refused or interrupted launch's exit handling.
    ///
    /// So the exit handling's cancellation points see only new signals. The TUI spends only a
    /// SIGINT, with `take_if`. Everything else only reads.
    pub fn take(&self) -> Option<i32> {
        match self.signal.swap(0, Ordering::SeqCst) {
            0 => None,
            n => Some(n as i32),
        }
    }

    /// Consumes the recorded signal only if it is `signal`, in one step: true when it was. A
    /// signal that replaced it, even an instant before, stays recorded. The TUI spends a
    /// foreground action's SIGINT this way, so a SIGTERM or SIGHUP that arrives with it still
    /// ends the TUI (TUI spec §7.3).
    pub fn take_if(&self, signal: i32) -> bool {
        debug_assert!(signal > 0, "signal numbers are positive");
        self.signal
            .compare_exchange(signal as usize, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
```

In `crates/tagteam/src/lib.rs`, after `mod transfer_cmd;` (line 24), add:

```rust
pub mod tui_host;
```

Create `crates/tagteam/src/tui_host.rs`:

```rust
//! TUI spec §4.5: the TUI's `Host`, over the CLI's own command path. An action runs as
//! `tagteam <args>` runs: parsed by the CLI's `clap` definition and run by `app::run`. Inside
//! the TUI (`act`) its output is captured and its prompts go to the TUI's dialogs. Once the TUI
//! has handed the terminal back (`act_foreground`), it runs on the terminal itself, and `Run` is
//! a `tagteam run` process of its own (§7.3).

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;

use clap::Parser;
use tagteam_provider::Cancel;
use tagteam_provider::process::{SpawnSpec, exit_code, spawn_session};
use tagteam_tui::host::{Action, ActionOutcome, Ask, Host};

use crate::app::{self, Context, Io};
use crate::cli::Cli;
use crate::prompt::{self, Prompter, TtyPrompter};
use crate::run;

/// R5, TUI spec §7.3 step 4: what the terminal shows once a foreground action is done.
pub const RETURN_PROMPT: &str = "Press Enter to return to tagteam";

/// The TUI's `Host` (TUI spec §4.5).
pub struct CliHost {
    /// The in-TUI actions' context. `env.cancel` is the shutdown token, so a `q` reaches them
    /// and a Ctrl-C meant for a foreground action does not (§2).
    inside: Context,
    /// The foreground actions' context. `env.cancel` is the signal token.
    foreground: Context,
    /// The binary `Run` spawns (§7.3 step 3).
    exe: PathBuf,
}

impl CliHost {
    /// `ctx` is the process's context, whose token is the signal token. `exe` is the binary
    /// that `Run` spawns as `<exe> --provider P --expect-account ID run N`:
    /// `std::env::current_exe()` in production.
    pub fn new(ctx: &Context, shutdown: Cancel, exe: PathBuf) -> CliHost {
        let mut inside = ctx.clone();
        inside.env.cancel = shutdown;
        // Captured, so never a terminal; `--no-color` keeps it plain either way.
        inside.stdout_terminal = false;
        CliHost {
            inside,
            foreground: ctx.clone(),
            exe,
        }
    }

    /// The signal token (§2).
    fn signal(&self) -> &Cancel {
        &self.foreground.env.cancel
    }

    /// §7.3 step 3 for `Run`: `tagteam run` as a child of the TUI, in its process group, which
    /// is the terminal's foreground group, so Ctrl-C and Ctrl-\ reach the session from the
    /// terminal itself (core §14.1). It is waited for as `run` waits for `claude` (core §12.5):
    /// SIGTERM and SIGHUP are forwarded as they arrive, and a SIGINT is the session's. A
    /// forwarded signal is recorded again once the child is reaped, so the TUI shuts down after
    /// it (§7.3, §10.2).
    fn run_session(&self, action: &Action) -> ActionOutcome {
        let spec = SpawnSpec {
            program: self.exe.clone(),
            args: action.args().into_iter().map(OsString::from).collect(),
            ..SpawnSpec::default()
        };
        let mut child = match spawn_session(&spec, None) {
            Ok(child) => child,
            Err(e) => {
                let err = format!("tagteam: could not start {}: {e}\n", self.exe.display());
                let _ = io::stderr().write_all(err.as_bytes());
                return ActionOutcome::Done {
                    exit: app::EXIT_ERROR,
                    out: String::new(),
                    err,
                };
            }
        };
        let (status, forwarded) = run::wait(&mut child, self.signal());
        if let Some(signal) = forwarded {
            self.signal().request(signal);
        }
        let exit = match status {
            Ok(status) => exit_code(status),
            Err(e) => {
                let _ = writeln!(
                    io::stderr(),
                    "tagteam: lost track of the session's exit status: {e}"
                );
                app::EXIT_ERROR
            }
        };
        ActionOutcome::Done {
            exit,
            out: String::new(),
            err: String::new(),
        }
    }

    /// §4.5: any other action on the terminal, as the CLI runs it: its own prompter, and the
    /// real stdout and stderr.
    fn run_here(&self, action: &Action) -> ActionOutcome {
        let cli = match parse(action) {
            Ok(cli) => cli,
            Err(err) => {
                let _ = io::stderr().write_all(err.as_bytes());
                return ActionOutcome::Done {
                    exit: app::EXIT_USAGE,
                    out: String::new(),
                    err,
                };
            }
        };
        let mut prompter = TtyPrompter::new(self.signal().clone());
        let (mut out, mut err) = (io::stdout(), io::stderr());
        let exit = app::run(
            cli,
            self.foreground.clone(),
            &mut Io {
                out: &mut out,
                err: &mut err,
                prompter: &mut prompter,
            },
        );
        ActionOutcome::Done {
            exit,
            out: String::new(),
            err: String::new(),
        }
    }
}

impl Host for CliHost {
    fn act(&self, action: &Action, ask: &mut dyn Ask) -> ActionOutcome {
        // In-process, `run` would exec the agent in the TUI's place: it always runs suspended.
        if action.needs_terminal() {
            return ActionOutcome::NeedsTerminal;
        }
        let cli = match parse(action) {
            Ok(cli) => cli,
            Err(err) => {
                return ActionOutcome::Done {
                    exit: app::EXIT_USAGE,
                    out: String::new(),
                    err,
                };
            }
        };
        let mut prompter = Bridge {
            ask,
            needs_terminal: false,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let exit = app::run(
            cli,
            self.inside.clone(),
            &mut Io {
                out: &mut out,
                err: &mut err,
                prompter: &mut prompter,
            },
        );
        if prompter.needs_terminal {
            return ActionOutcome::NeedsTerminal;
        }
        ActionOutcome::Done {
            exit,
            out: String::from_utf8_lossy(&out).into_owned(),
            err: String::from_utf8_lossy(&err).into_owned(),
        }
    }

    fn act_foreground(&self, action: &Action) -> ActionOutcome {
        // A signal that arrived before the terminal was handed over is the TUI's: nothing
        // runs, and the TUI shuts down or carries on as its loop decides (§10.2).
        if let Some(signal) = self.signal().requested() {
            return ActionOutcome::Done {
                exit: 128 + signal,
                out: String::new(),
                err: "tagteam: interrupted\n".to_owned(),
            };
        }
        let outcome = match action {
            Action::Run(_) => self.run_session(action),
            _ => self.run_here(action),
        };
        wait_for_enter(self.signal());
        outcome
    }
}

/// `action`'s arguments, parsed by the CLI's own definition (§4.5). They are the TUI's own, so
/// a failure is a bug. It is reported as the usage error it is, without the arguments' values
/// (`lib::without_argument_values`'s rule).
fn parse(action: &Action) -> Result<Cli, String> {
    let args = std::iter::once("tagteam".to_owned()).chain(action.args());
    Cli::try_parse_from(args).map_err(|e| format!("tagteam: {}\n", e.kind()))
}

/// R5, §7.3 step 4. A SIGINT recorded by now was the action's, typed at its prompt or in its
/// session: it is spent, so it does not end the wait at once. Only a SIGINT is spent, in one
/// step (`take_if`), so a SIGTERM or SIGHUP that replaced it stays recorded. A SIGTERM or
/// SIGHUP ends the TUI (§10.2), so nothing is waited for. Enter, the end of input, or a signal
/// ends the wait.
fn wait_for_enter(signal: &Cancel) {
    signal.take_if(libc::SIGINT);
    if signal.requested().is_some() {
        return;
    }
    let mut err = io::stderr();
    let _ = write!(err, "{RETURN_PROMPT}");
    let _ = err.flush();
    let _ = prompt::read_terminal_line(signal);
}

/// §4.5: the CLI's prompts, sent to the TUI's dialogs. A person answers there, so the command
/// prompts as it would on a terminal. The keychain unlock is the exception (Appendix A.3): macOS
/// asks for the password on the terminal itself, so the question is declined unasked and
/// noted, the command fails before touching anything, and `act` hands the action to the
/// terminal. No action asks for a secret; one that did is declined.
struct Bridge<'a> {
    ask: &'a mut dyn Ask,
    needs_terminal: bool,
}

impl Prompter for Bridge<'_> {
    fn interactive(&self) -> bool {
        true
    }

    fn reaches_terminal(&self) -> bool {
        false
    }

    fn confirm(&mut self, question: &str, default_yes: bool) -> bool {
        if question == app::UNLOCK_QUESTION {
            self.needs_terminal = true;
            return false;
        }
        self.ask.confirm(question, default_yes)
    }

    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize> {
        self.ask.choose(question, options)
    }

    fn secret(&mut self, _question: &str) -> Option<String> {
        None
    }
}
```

`run.rs`'s unit tests (`the_first_look_after_the_spawn_forwards_any_signal_and_later_ones_only_term_and_hup`
and `an_exec_that_finds_no_command_is_launch_command_missing_and_any_other_failure_is_io`) test
`forwarded` and `exec_failed`, which do not change; no existing test breaks.

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam --test tui_host`
Expected: PASS, 18 tests.

Run: `cargo test -p tagteam --features test-support --test run_cli`, then
`cargo test -p tagteam --features test-support` (its `prompt::tests` and `tests/signals.rs`
open a pty: outside the sandbox) and `cargo test -p tagteam-provider --lib cancel::` (9 tests)
Expected: PASS. `run_cli.rs`'s signal tests (`sigterm_and_sighup_to_tagteam_reach_the_session_once_and_exit_handling_still_runs`,
`sigint_to_tagteam_alone_never_reaches_the_session`, `a_forwarded_sigterm_that_ends_the_session_exits_143`
and the rest) pin `run`'s forwarding through the changed `wait`.

- [ ] **Step 5: Format and lint**

`cargo fmt --all && cargo fmt --all --check`, then
`cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`, then
`cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam/src/tui_host.rs crates/tagteam/src/lib.rs crates/tagteam/src/app.rs \
  crates/tagteam/src/run.rs crates/tagteam-provider/src/cancel.rs crates/tagteam/tests/tui_host.rs
git commit -m "Run the TUI's actions through the CLI's own commands"
```

---

### Task 11: Runtime: terminal, input, loop, suspension, shutdown

The runtime is the UI thread (spec §4.3). It owns the terminal (`term.rs`), the input thread
(`input.rs`) and the two engine lanes of part A, and it drives Task 5's `update` and `view` in a
loop. It hands the terminal to a foreground action and takes it back (§7.3), and every way out
goes through one shutdown sequence that joins the lanes before the process may exit (§10.2).

The loop is written over two small seams so that it runs in unit tests without a terminal:
`term::Tty` (the terminal: a frame drawn under its lock, handing it back and taking it again) and
`input::Pause` (pausing the input thread, which says whether it acknowledged, and resuming it).
The real ones are `term::RealTty` and `input::Input`; the tests use a `TestBackend` and a
recorder. The lanes, the `Source` and the `Host` in the tests are real lanes over fakes.

**Readings of the spec this task commits to:**
- **The loop.** It waits on the one `Msg` channel for at most the time left to the next tick, and
  ticks (`Msg::Tick`) every 250 ms whatever arrives, so a stream of messages never starves the
  refresh timer (§4.3). It looks at the signal token before it waits, and again after every
  wake, before it applies the message or the tick that woke it (R17): a SIGTERM that arrives
  during the wait comes before the `q` that ends it. A message the shutdown overtakes is not
  applied, and an action's outcome in it goes into the report. It redraws only when
  `state.dirty`, and clears `dirty` after a successful draw. After each frame it executes
  `update::drawn`'s commands (Task 8, R19).
- **The first frame** (§6.1): the runtime sends the terminal's size (`Msg::Resize`), executes
  `State::start()`'s commands, then applies messages for up to 100 ms, until the first
  `Msg::Accounts` has been applied. Then it draws, whether or not the accounts came. The signal
  token comes first here too (R17): a signal recorded before the wait or during it ends the TUI
  with no frame drawn. The first frame that shows accounts, this one or the first after
  `loading…`, is followed by the first collection (R19); the collection runs on the actions lane,
  after the frame, so the first frame's budget does not change.
- **Signals** (§10.2's table). A signal in the token while not suspended starts the shutdown
  with 128 + n: SIGTERM 143, SIGHUP 129, and SIGINT 130 (only `kill -INT` sends one, since raw
  mode turns Ctrl-C into a key). The shutdown token gets the same signal.
- **The shutdown sequence** (§10.2): `begin_shutdown(code, signal, fatal)` sets the shutdown token
  (step 1; SIGINT for `q`, Ctrl-C, a fatal error and a panic), closes both lanes (step 2), sets
  `state.quitting`, and for exit 0 draws `finishing <action>…` (step 4); for a signal or a fatal
  error it restores the terminal first. Step 3 is `ChannelAsk`'s own (Task 9). A second trigger
  changes nothing: the first code stands, and nothing skips the join. Steps 5–7 run after the
  loop, outside the panic boundary (`finish`): the lanes are joined, the terminal restored, the
  report built, and the code returned.
- **The report** (step 6) is what an action finished after the loop stopped reading: the `out`
  and `err` of an `ActionDone` the shutdown overtook (R17), then of every `ActionDone` still in
  the channel once the lanes are joined, in the CLI's words. A switch that committed under the
  shutdown reports `Switched to …` and the CLI's own "interrupted too late" notice. Then the error that ended the TUI: a fatal error's message, and
  `tagteam: the TUI's <lane> lane panicked; the log has the details` for each lane whose join
  returned `Err`. A lane that panicked turns an exit 0 into 1.
- **A panic on the UI thread** is caught by `catch_unwind` around the loop. The panic hook has
  restored the terminal and logged it by then (§10.1). The shutdown then runs as for any fatal
  error, with exit 1, so the lanes still drain.
- **The panic hook** (installed once, by the first `RealTty::enter`) restores the terminal, then calls
  the hook that was there before: logging's (`logging::init`, which the binary runs before
  anything else), which logs the panic and prints it on stderr (core §14.2). A panic on a lane thread restores the terminal
  too, so its message is readable. `term::restore` is idempotent, and the loop never draws once
  the terminal is restored.
- **The terminal's lock (R16).** `term::Ownership` holds whether the TUI has the terminal, behind
  one mutex. A frame (the check, `Terminal::draw` and its flush), an enter with its repaint, a
  restore, and `RealTty`'s drop (ratatui's `Terminal` shows the cursor as it is dropped; the drop
  does it first, under the lock) all hold it, and so does the panic hook's restore. A lane that
  panics while a frame is written therefore restores after the frame, and the next frame finds
  the terminal given back and draws nothing. `Ownership` records the thread that holds the
  mutex: the hook on that thread (a panic inside `view`) restores without waiting for itself. A
  poisoned mutex still serializes. The tests watch `Ownership.waiting` to see a hook held up by
  a frame, so the interleaving is driven by channels, not sleeps.
- **Suspension** (§7.3): pause the input thread, which acknowledges (step 1); without its
  acknowledgment the suspension is refused (R18): the pause is withdrawn, nothing runs,
  `state.running` is cleared, the TUI keeps the terminal, and the error banner says
  `<action> did not start: the TUI's key reader did not pause in time; try again`. Then give
  back the terminal (step 2); `Host::act_foreground` (step 3, and R5's wait for Enter). Then the
  signal token decides: a SIGINT is the action's and is spent with `Cancel::take_if(SIGINT)`
  (R15), and the TUI carries on; a SIGTERM or SIGHUP, even one that replaced that SIGINT, starts
  the shutdown at once, with the terminal left as the action left it. Otherwise the terminal is
  taken again and repainted, the input resumed, the size re-read, and the outcome delivered as
  `Msg::ActionDone` (R17), whose `on_done` (Task 9) re-reads the views (step 4). A
  `NeedsTerminal` from `act_foreground` would loop forever, so it is applied as
  `Done { exit: 1, out: "", err: "" }`.
- **Ctrl-Z** (§10.1): pause the input, give back the terminal, raise SIGTSTP
  (`tagteam_provider::term::stop_self`, R6), and on SIGCONT take the terminal again, repaint, and
  re-read the size. Without the input thread's acknowledgment it is refused as a suspension is
  (R18), with `Ctrl-Z did not suspend: …` in the banner. A shutdown for a signal or a fatal
  error asks the input thread to pause and restores the terminal whether it acknowledged or not:
  the process is exiting.
- **Repainting** never uses `Terminal::clear`, which asks the terminal for the cursor's position
  and waits up to 2 s for an answer. `term::repaint` resizes the terminal to its own size, which
  clears the screen and forgets the last frame, so the next draw writes every cell.
- **`gen` is written `r#gen`** (R8): edition 2024 reserves `gen` as a keyword, so every
  generation field, in a constructor or a pattern, is spelled `r#gen`.
- **The order of shutdown triggers (R11).** `update` sets `state.quitting` itself for `q`,
  Ctrl-C and `LaneDied`, and returns `Cmd::Quit`, which `begin_shutdown` executes with the same
  code. The runtime sets it for a signal (128 + n) and for a fatal error or a UI-thread panic
  (1). Whichever comes first wins: `begin_shutdown` returns at once once a shutdown has begun,
  and `update` ignores keys and a later `LaneDied` once `quitting` is set.
- **A suspended action's outcome (R12)** is applied as `Msg::ActionDone`, carrying what
  `act_foreground` returned: `CliHost`'s has empty `out` and `err` (its output went to the
  terminal), so Task 9's `on_done` shows no toast or panel, and re-reads `accounts`, and
  `history` when History is open.
- **The input thread** reads in 50 ms slices. `pause` returns true once the thread is between
  two polls and has said so, and false when it has not said so within a second (`ACK`): it may be
  held up between its look at the control and its poll, and could still read (R18). Dropping
  `Input` stops it the same way, before the terminal is restored, so it never reads the shell's
  input; the TUI is ending, so the drop waits no longer than `ACK`. It is never joined (§10.2). Key presses and
  repeats become `Msg::Key`, resizes `Msg::Resize`; key releases, mouse and focus events are
  dropped.

**Depends on:** 4, 5, 8, 9, 10

**Files:**
- Modify: `crates/tagteam-provider/src/term.rs` (the module doc, lines 1–3; add `stop_self` after
  `child_is_stopped`, line 177; a test in `spawn_tests`, lines 222–237)
- Modify: `crates/tagteam-tui/src/term.rs` (Task 3's one-line module becomes the terminal)
- Modify: `crates/tagteam-tui/src/input.rs` (Task 3's one-line module becomes the input thread)
- Modify: `crates/tagteam-tui/src/runtime.rs` (Task 3's one-line module becomes the runtime)
- Modify: `crates/tagteam-tui/src/lib.rs` (add `pub use runtime::{TuiExit, TuiOptions, run};`)
- Modify: `crates/tagteam-tui/src/ask.rs` (Task 9's three `#[allow(dead_code)]` go: the runtime
  builds a `ChannelAsk` per action)
- Modify: `crates/tagteam-tui/Cargo.toml` (`[dependencies]` must hold `crossterm`, `libc`,
  `ratatui`, `tagteam-core`, `tagteam-engine` and `tagteam-provider`, each `.workspace = true`;
  Task 3 adds all but `libc`, which this task adds)
- Test: unit tests in `crates/tagteam-tui/src/{term,input,runtime}.rs` and
  `crates/tagteam-provider/src/term.rs`

**Interfaces:**
- Consumes:
  - Task 3: `host::{Action, ActionOutcome, Host, AccountRef, Ask}`,
    `source::{Source, SourceFactory, AccountsSnapshot, CollectSummary, SourceError}`.
  - Task 4: `lanes::Lane::{spawn, submit, close, join}` (the source a lane built is dropped when
    its thread ends, before `join` returns).
  - Task 5: `msg::{Msg, Cmd, ViewsReq, ActionsReq}`, `state::{State, Screen}`
    (`State::new`, `State::start`, `state.dirty`, `state.quitting`, `state.running`),
    `update::update`, `view::view`. `Msg` carries Task 5's `#[allow(clippy::large_enum_variant)]`
    (its `History` variant holds a `HistoryView` of about 600 bytes).
  - Task 6: `AccountsState::snapshot` (the first-frame tests).
  - Task 8: `update::drawn` (R19), and `testutil::{snapshot, many}` in the first-collection
    tests.
  - Task 10: `tagteam_provider::Cancel::take_if` (R15).
  - Task 9: `ask::ChannelAsk::new(tx: Sender<Msg>, shutdown: Cancel) -> ChannelAsk` (contract
    C7), `on_done` returning `Cmd::Suspend(action)` for `NeedsTerminal`, and showing nothing
    for an outcome with empty `out` and `err` (R12).
- Produces (C8):
  - `pub struct TuiOptions { screen, color, provider, signal, shutdown }`,
    `pub struct TuiExit { code, report }`,
    `pub fn run(opts: TuiOptions, sources: SourceFactory, host: Arc<dyn Host>) -> TuiExit`,
    re-exported from the crate root.
  - `tagteam_provider::term::stop_self() -> io::Result<()>` (R6).
  - Crate-private: `term::{Ownership, restore, repaint, Tty, RealTty}`,
    `input::{Events, Crossterm, Pause, Input}`, `runtime::{Lanes, Ui, Ending, run_with, finish}`.

**Spec:**
- spec §4.3: the UI thread blocks on one channel with a 250 ms timeout, checks the signal token
  on every wake, runs `update`, executes the commands, and redraws only when the state changed;
  the input thread polls with a short timeout so it can pause and acknowledge; a lane returns
  once its channel is closed and its current request has unwound.
- spec §6.1: up to 100 ms for the first `accounts`, then the first frame.
- spec §7.3: steps 1–4, and the signals while suspended.
- spec §10.1: raw mode, the alternate screen and a hidden cursor; restored by a guard on every
  path and by a panic hook installed before entering; Ctrl-Z; resizing redraws.
- spec §10.2: the shutdown table, steps 1–7, and the threads that are not joined.
- spec §10.4: a lane that died, or a terminal that can no longer be written, is fatal: exit 1,
  with the error on stderr.
- R6: `raise(SIGTSTP)` is a `tagteam-provider` helper.
- R15 to R19: the action's SIGINT, the terminal's lock, the signal before every message, the
  refused hand-over, and the first collection after drawing.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-provider/src/term.rs`, add to the `spawn_tests` module (after
`a_program_that_cannot_start_is_an_error_and_changes_nothing_without_a_terminal`):

```rust
    #[test]
    fn stop_self_stops_the_process_until_it_is_continued() {
        // A forked child stops itself; the test sees it stopped, continues it, and sees it exit
        // 0. The child leads a group of its own first: in an orphaned group, as a CI runner's
        // may be, SIGTSTP is discarded.
        let _fork = crate::fork_guard();
        // SAFETY: the child calls only async-signal-safe functions: setpgid(2), raise(3)
        // inside `stop_self`, and _exit(2).
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork: {}", io::Error::last_os_error());
        if pid == 0 {
            // SAFETY: in the forked child; both take integers and touch no memory.
            unsafe {
                libc::setpgid(0, 0);
                libc::_exit(if stop_self().is_ok() { 0 } else { 1 });
            }
        }
        let mut status = 0;
        // SAFETY: waitpid(2) writes the child's status into `status`; WUNTRACED reports a stop.
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) },
            pid
        );
        assert!(
            libc::WIFSTOPPED(status) && libc::WSTOPSIG(status) == libc::SIGTSTP,
            "{status:#x}"
        );
        // SAFETY: kill(2) on this test's own child, which is not reaped yet.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGCONT) }, 0);
        // SAFETY: as above, now for its exit.
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "{status:#x}"
        );
    }
```

In `crates/tagteam-tui/src/term.rs`, the tests (the module itself is Step 3's):

```rust
#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::panic::{self, AssertUnwindSafe};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use ratatui::backend::TestBackend;

    use super::*;

    #[test]
    fn restore_gives_the_terminal_back_once_and_only_after_an_enter() {
        let own = Ownership::new();
        let given = Cell::new(0);
        own.restore_with(|| given.set(given.get() + 1));
        assert_eq!(given.get(), 0, "nothing to give back before an enter");
        own.enter_with(|| Ok(())).unwrap();
        own.restore_with(|| given.set(given.get() + 1));
        own.restore_on_panic(|| given.set(given.get() + 1));
        assert_eq!(
            given.get(),
            1,
            "the guard, the hook and the loop may all restore"
        );
        own.enter_with(|| Ok(())).unwrap();
        own.restore_with(|| given.set(given.get() + 1));
        assert_eq!(
            given.get(),
            2,
            "a suspension enters again, and is restored again"
        );
    }

    #[test]
    fn an_enter_that_fails_part_way_is_still_undone() {
        // Raw mode may be on when the alternate screen fails.
        let own = Ownership::new();
        assert!(
            own.enter_with(|| Err::<(), _>(io::Error::other("no alternate screen")))
                .is_err()
        );
        let given = Cell::new(false);
        own.restore_with(|| given.set(true));
        assert!(given.get());
    }

    #[test]
    fn nothing_is_drawn_once_the_terminal_is_given_back() {
        let own = Ownership::new();
        assert_eq!(own.while_entered(|| "frame"), None);
        own.enter_with(|| Ok(())).unwrap();
        assert_eq!(own.while_entered(|| "frame"), Some("frame"));
        own.restore_on_panic(|| {});
        assert_eq!(own.while_entered(|| "frame"), None);
    }

    /// The events of one interleaving, in order.
    #[derive(Clone, Default)]
    struct Events(Arc<Mutex<Vec<&'static str>>>);

    impl Events {
        fn push(&self, what: &'static str) {
            self.0.lock().unwrap().push(what);
        }
        fn seen(&self) -> Vec<&'static str> {
            self.0.lock().unwrap().clone()
        }
    }

    #[test]
    fn a_panic_on_another_thread_gives_the_terminal_back_after_the_frame_in_flight() {
        // The UI thread has checked that the TUI holds the terminal and is writing a frame when
        // a lane panics. Its hook must wait for the frame, or the frame lands on the main screen
        // once the hook has left the alternate one.
        let own = Arc::new(Ownership::new());
        own.enter_with(|| Ok(())).unwrap();
        let events = Events::default();
        let (checked, mid_frame) = mpsc::channel();
        let (flush, flushed) = mpsc::channel::<()>();
        let ui = thread::spawn({
            let (own, events) = (Arc::clone(&own), events.clone());
            move || {
                own.while_entered(|| {
                    checked.send(()).unwrap();
                    flushed.recv().unwrap();
                    events.push("frame");
                })
            }
        });
        mid_frame.recv().unwrap();
        let lane = thread::spawn({
            let (own, events) = (Arc::clone(&own), events.clone());
            move || own.restore_on_panic(|| events.push("given back"))
        });
        // The hook either waits for the lock (fixed) or has already given the terminal back.
        let deadline = Instant::now() + Duration::from_secs(5);
        while own.waiting.load(Ordering::SeqCst) == 0 && events.seen().is_empty() {
            assert!(Instant::now() < deadline, "the hook neither waited nor ran");
            thread::yield_now();
        }
        assert_eq!(
            events.seen(),
            Vec::<&str>::new(),
            "nothing is given back under a frame"
        );
        flush.send(()).unwrap();
        assert_eq!(ui.join().unwrap(), Some(()));
        lane.join().unwrap();
        assert_eq!(events.seen(), ["frame", "given back"]);
        assert_eq!(
            own.while_entered(|| events.push("late frame")),
            None,
            "the loop draws nothing once the terminal is given back"
        );
    }

    #[test]
    fn a_panic_inside_a_frame_gives_the_terminal_back_without_waiting_for_itself() {
        // `view` panics on the UI thread, which holds the lock: the hook runs on that thread.
        let own = Arc::new(Ownership::new());
        own.enter_with(|| Ok(())).unwrap();
        let (done, finished) = mpsc::channel();
        thread::spawn({
            let own = Arc::clone(&own);
            move || {
                let given = Cell::new(false);
                own.while_entered(|| own.restore_on_panic(|| given.set(true)));
                done.send(given.get()).unwrap();
            }
        });
        assert_eq!(
            finished.recv_timeout(Duration::from_secs(5)),
            Ok(true),
            "the hook gave the terminal back, and did not wait for its own thread"
        );
    }

    #[test]
    fn a_panic_that_unwinds_out_of_a_frame_leaves_the_lock_usable() {
        let own = Ownership::new();
        own.enter_with(|| Ok(())).unwrap();
        let unwound = panic::catch_unwind(AssertUnwindSafe(|| {
            own.while_entered(|| panic!("the view panics"));
        }));
        assert!(unwound.is_err());
        assert_eq!(own.holder.load(Ordering::SeqCst), 0, "let go as it unwound");
        let given = Cell::new(false);
        own.restore_with(|| given.set(true));
        assert!(given.get(), "the poisoned lock still serializes");
    }

    #[test]
    fn a_repaint_clears_the_screen_and_forgets_the_last_frame() {
        let mut terminal = Terminal::new(TestBackend::new(20, 2)).unwrap();
        terminal
            .draw(|frame| frame.render_widget("tagteam", frame.area()))
            .unwrap();
        repaint(&mut terminal).unwrap();
        assert_eq!(
            terminal.backend().to_string(),
            "\"                    \"\n\"                    \"\n"
        );
        terminal
            .draw(|frame| frame.render_widget("tagteam", frame.area()))
            .unwrap();
        assert!(
            terminal.backend().to_string().starts_with("\"tagteam "),
            "the same frame is written again in full"
        );
    }
}
```

In `crates/tagteam-tui/src/input.rs`, the tests:

```rust
#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;

    /// Scripted events. `reading` is set while the thread is inside `poll` or `read`, and
    /// `polls` counts its polls.
    #[derive(Clone, Default)]
    struct Script {
        queue: Arc<Mutex<VecDeque<Event>>>,
        reading: Arc<AtomicBool>,
        polls: Arc<AtomicUsize>,
    }

    impl Script {
        fn push(&self, event: Event) {
            self.queue.lock().unwrap().push_back(event);
        }
    }

    impl Events for Script {
        fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
            self.reading.store(true, Ordering::SeqCst);
            self.polls.fetch_add(1, Ordering::SeqCst);
            let ready = !self.queue.lock().unwrap().is_empty();
            if !ready {
                thread::sleep(timeout);
            }
            self.reading.store(false, Ordering::SeqCst);
            Ok(ready)
        }

        fn read(&mut self) -> io::Result<Event> {
            Ok(self
                .queue
                .lock()
                .unwrap()
                .pop_front()
                .expect("poll said ready"))
        }
    }

    /// Events whose poll waits at a gate before it looks: the thread is past its look at the
    /// control and has not polled yet, as if it were descheduled there. Once opened, it polls
    /// as `Script` does.
    #[derive(Clone, Default)]
    struct Gate {
        script: Script,
        open: Arc<(Mutex<bool>, Condvar)>,
        waiting: Arc<AtomicBool>,
    }

    impl Gate {
        fn open(&self) {
            *self.open.0.lock().unwrap() = true;
            self.open.1.notify_all();
        }
    }

    impl Events for Gate {
        fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
            self.waiting.store(true, Ordering::SeqCst);
            let (open, opened) = &*self.open;
            let mut open = open.lock().unwrap();
            while !*open {
                open = opened.wait(open).unwrap();
            }
            drop(open);
            self.script.poll(timeout)
        }

        fn read(&mut self) -> io::Result<Event> {
            self.script.read()
        }
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn key_presses_and_resizes_become_messages_and_nothing_else_does() {
        let script = Script::default();
        let mut release = key('x');
        release.kind = KeyEventKind::Release;
        for event in [
            Event::Key(release),
            Event::FocusGained,
            Event::Key(key('q')),
            Event::Resize(100, 30),
        ] {
            script.push(event);
        }
        let (tx, rx) = mpsc::channel();
        let _input = Input::spawn_with(tx, script.clone()).unwrap();
        let within = Duration::from_secs(2);
        assert!(matches!(
            rx.recv_timeout(within),
            Ok(Msg::Key(k)) if k.code == KeyCode::Char('q')
        ));
        assert!(matches!(
            rx.recv_timeout(within),
            Ok(Msg::Resize {
                width: 100,
                height: 30
            })
        ));
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn a_pause_returns_once_the_thread_reads_no_more_and_resume_reads_again() {
        let script = Script::default();
        let (tx, rx) = mpsc::channel();
        let input = Input::spawn_with(tx, script.clone()).unwrap();
        thread::sleep(Duration::from_millis(30));
        assert!(input.pause());
        assert!(!script.reading.load(Ordering::SeqCst), "between two polls");
        let polls = script.polls.load(Ordering::SeqCst);
        script.push(Event::Key(key('x')));
        thread::sleep(Duration::from_millis(200));
        assert_eq!(
            script.polls.load(Ordering::SeqCst),
            polls,
            "no poll while paused"
        );
        assert!(rx.try_recv().is_err(), "nothing read while paused");
        input.resume();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)),
            Ok(Msg::Key(k)) if k.code == KeyCode::Char('x')
        ));
    }

    #[test]
    fn a_pause_the_thread_does_not_acknowledge_says_so() {
        // The thread has looked at the control (`Reading`) and is held up before its poll. A
        // pause that returned anyway would hand the terminal over while it can still read.
        let gate = Gate::default();
        gate.script.push(Event::Key(key('x')));
        let (tx, rx) = mpsc::channel();
        let input = Input::spawn_with(tx, gate.clone()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !gate.waiting.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "the thread never reached its poll"
            );
            thread::yield_now();
        }
        let started = Instant::now();
        assert!(!input.pause(), "not acknowledged");
        assert!(started.elapsed() >= ACK);
        // What a hand-over would have lost: the thread polls, and reads the next owner's key.
        gate.open();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)),
            Ok(Msg::Key(k)) if k.code == KeyCode::Char('x')
        ));
        // Back at its look, it acknowledges the pause still asked for.
        assert!(input.pause());
        input.resume();
    }

    #[test]
    fn dropping_the_input_stops_its_thread_before_it_returns() {
        let script = Script::default();
        let (tx, _rx) = mpsc::channel();
        let input = Input::spawn_with(tx, script.clone()).unwrap();
        thread::sleep(Duration::from_millis(30));
        drop(input);
        let polls = script.polls.load(Ordering::SeqCst);
        thread::sleep(Duration::from_millis(200));
        assert_eq!(script.polls.load(Ordering::SeqCst), polls);
        assert!(!script.reading.load(Ordering::SeqCst));
    }

    #[test]
    fn with_the_ui_gone_the_thread_stops_and_a_pause_never_hangs() {
        let script = Script::default();
        let (tx, rx) = mpsc::channel();
        let input = Input::spawn_with(tx, script.clone()).unwrap();
        drop(rx);
        script.push(Event::Key(key('q')));
        thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        assert!(input.pause(), "a thread that has stopped reads no more");
        input.resume();
        drop(input);
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
```

In `crates/tagteam-tui/src/runtime.rs`, the tests:

```rust
#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tagteam_core::AccountId;
    use tagteam_engine::views::HistoryView;

    use super::*;
    use crate::host::{AccountRef, Ask};
    use crate::source::{AccountsSnapshot, CollectSummary, SourceError};
    use crate::term::repaint;

    /// What the fake terminal and input were asked to do, in order.
    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<&'static str>>>);

    impl Log {
        fn push(&self, what: &'static str) {
            self.0.lock().unwrap().push(what);
        }
        fn take(&self) -> Vec<&'static str> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
        fn seen(&self) -> Vec<&'static str> {
            self.0.lock().unwrap().clone()
        }
    }

    /// The fake input thread: it logs to `log`, and acknowledges a pause unless `stuck`.
    #[derive(Clone, Default)]
    struct Keys {
        log: Log,
        stuck: Arc<AtomicBool>,
    }

    impl Pause for Keys {
        fn pause(&self) -> bool {
            self.log.push("pause");
            !self.stuck.load(Ordering::SeqCst)
        }
        fn resume(&self) {
            self.log.push("resume");
        }
    }

    /// A 120×40 `TestBackend` that records being handed over in `log`, and each frame in
    /// `frames`. With `panics`, drawing panics; `on_draw` runs once, after the next frame.
    struct FakeTty {
        terminal: Terminal<TestBackend>,
        log: Log,
        frames: Log,
        active: bool,
        panics: bool,
        on_draw: Option<Box<dyn FnOnce()>>,
    }

    impl FakeTty {
        fn new(log: Log, frames: Log) -> FakeTty {
            FakeTty {
                terminal: Terminal::new(TestBackend::new(120, 40)).unwrap(),
                log,
                frames,
                active: true,
                panics: false,
                on_draw: None,
            }
        }

        fn text(&self) -> String {
            self.terminal.backend().to_string()
        }
    }

    impl Tty for FakeTty {
        type B = TestBackend;

        fn draw(&mut self, render: impl FnOnce(&mut ratatui::Frame)) -> Result<bool, Infallible> {
            assert!(!self.panics, "the UI thread panics while it draws");
            if !self.active {
                return Ok(false);
            }
            self.terminal.draw(render)?;
            self.frames.push("draw");
            if let Some(then) = self.on_draw.take() {
                then();
            }
            Ok(true)
        }

        fn size(&mut self) -> io::Result<(u16, u16)> {
            let size = self.terminal.size().unwrap();
            Ok((size.width, size.height))
        }

        fn active(&self) -> bool {
            self.active
        }

        fn release(&mut self) {
            self.log.push("release");
            self.active = false;
        }

        fn reclaim(&mut self) -> io::Result<()> {
            self.log.push("reclaim");
            self.active = true;
            repaint(&mut self.terminal).map_err(|never| match never {})
        }

        fn stop(&mut self) -> io::Result<()> {
            self.log.push("stop");
            Ok(())
        }
    }

    /// A source with no accounts. `accounts` takes `delay`, or panics with `panics`. `drops`
    /// counts the sources dropped: a lane drops its own when its thread ends.
    struct Empty {
        delay: Duration,
        panics: bool,
        drops: Arc<AtomicUsize>,
    }

    impl Drop for Empty {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl Source for Empty {
        fn accounts(
            &self,
            _provider: Option<&ProviderId>,
        ) -> Result<AccountsSnapshot, SourceError> {
            assert!(!self.panics, "the views lane's read panics");
            thread::sleep(self.delay);
            Ok(AccountsSnapshot {
                providers: Vec::new(),
                this: None,
                unmanaged_live: None,
                in_run_shell: false,
                read_at_s: 0,
            })
        }

        fn history(&self, _account: &AccountId, _since_s: i64) -> Result<HistoryView, SourceError> {
            Err(SourceError {
                kind: "test".into(),
                message: "no history in this test".into(),
            })
        }

        fn collect_due(&self, _accounts: &[AccountId]) -> Result<CollectSummary, SourceError> {
            Ok(CollectSummary {
                fetched: 0,
                warnings: Vec::new(),
            })
        }
    }

    fn sources(delay: Duration, panics: bool, drops: &Arc<AtomicUsize>) -> SourceFactory {
        let drops = Arc::clone(drops);
        Arc::new(move || {
            Box::new(Empty {
                delay,
                panics,
                drops: Arc::clone(&drops),
            }) as Box<dyn Source>
        })
    }

    fn quick() -> SourceFactory {
        sources(Duration::ZERO, false, &Arc::default())
    }

    /// A source with one account, read after `delay`. Each collection is logged in `frames`,
    /// beside the fake terminal's frames.
    struct OneAccount {
        delay: Duration,
        frames: Log,
    }

    impl Source for OneAccount {
        fn accounts(
            &self,
            _provider: Option<&ProviderId>,
        ) -> Result<AccountsSnapshot, SourceError> {
            thread::sleep(self.delay);
            Ok(crate::testutil::snapshot(crate::testutil::many(1)))
        }

        fn history(&self, _account: &AccountId, _since_s: i64) -> Result<HistoryView, SourceError> {
            Err(SourceError {
                kind: "test".into(),
                message: "no history in this test".into(),
            })
        }

        fn collect_due(&self, _accounts: &[AccountId]) -> Result<CollectSummary, SourceError> {
            self.frames.push("collect");
            Ok(CollectSummary {
                fetched: 0,
                warnings: Vec::new(),
            })
        }
    }

    fn one_account(delay: Duration, frames: &Log) -> SourceFactory {
        let frames = frames.clone();
        Arc::new(move || {
            Box::new(OneAccount {
                delay,
                frames: frames.clone(),
            }) as Box<dyn Source>
        })
    }

    /// A source whose `accounts` read finds SIGTERM recorded in `signal` by the time it returns:
    /// the signal arrives while the first frame waits for it.
    fn signalled_during_the_first_read(signal: &Cancel) -> SourceFactory {
        let signal = signal.clone();
        Arc::new(move || {
            struct Signalled(Cancel);
            impl Source for Signalled {
                fn accounts(
                    &self,
                    _provider: Option<&ProviderId>,
                ) -> Result<AccountsSnapshot, SourceError> {
                    self.0.request(libc::SIGTERM);
                    Ok(crate::testutil::snapshot(crate::testutil::many(1)))
                }
                fn history(&self, _: &AccountId, _: i64) -> Result<HistoryView, SourceError> {
                    unreachable!("no history is read")
                }
                fn collect_due(&self, _: &[AccountId]) -> Result<CollectSummary, SourceError> {
                    unreachable!("nothing is collected")
                }
            }
            Box::new(Signalled(signal.clone())) as Box<dyn Source>
        })
    }

    /// Waits up to 5 s for `frames` to see `what`; whether it did.
    fn seen_within(frames: &Log, what: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !frames.seen().contains(&what) {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
        true
    }

    type Behaviour = Box<dyn Fn(&Action) -> ActionOutcome + Send + Sync>;

    /// A host whose actions are closures.
    struct Scripted {
        act: Behaviour,
        foreground: Behaviour,
    }

    impl Host for Scripted {
        fn act(&self, action: &Action, _ask: &mut dyn Ask) -> ActionOutcome {
            (self.act)(action)
        }
        fn act_foreground(&self, action: &Action) -> ActionOutcome {
            (self.foreground)(action)
        }
    }

    fn never() -> Behaviour {
        Box::new(|action| panic!("{action:?} was not expected to run"))
    }

    fn idle() -> Scripted {
        Scripted {
            act: never(),
            foreground: never(),
        }
    }

    fn done(exit: i32, out: &str, err: &str) -> ActionOutcome {
        ActionOutcome::Done {
            exit,
            out: out.into(),
            err: err.into(),
        }
    }

    fn options() -> TuiOptions {
        TuiOptions {
            screen: Screen::Accounts,
            color: false,
            provider: None,
            signal: Cancel::new(),
            shutdown: Cancel::new(),
        }
    }

    fn account_a() -> AccountRef {
        AccountRef {
            id: AccountId::from_string("0192-a"),
            provider: ProviderId::new("claude-code"),
            position: 1,
        }
    }

    /// The runtime's wiring around a fake terminal and input, with real lanes. `log` has what
    /// the terminal and the input were asked to do; `frames` has each frame, and what the
    /// sources record.
    struct Fixture {
        opts: TuiOptions,
        tx: Sender<Msg>,
        rx: Receiver<Msg>,
        lanes: Lanes,
        host: Arc<dyn Host>,
        log: Log,
        frames: Log,
        keys: Keys,
    }

    impl Fixture {
        fn new(sources: SourceFactory, host: impl FnOnce(&TuiOptions) -> Scripted) -> Fixture {
            Fixture::with(options(), sources, host)
        }

        fn with(
            opts: TuiOptions,
            sources: SourceFactory,
            host: impl FnOnce(&TuiOptions) -> Scripted,
        ) -> Fixture {
            let host: Arc<dyn Host> = Arc::new(host(&opts));
            let (tx, rx) = mpsc::channel();
            let lanes = Lanes::spawn(None, &sources, &host, &tx, &opts.shutdown);
            let log = Log::default();
            Fixture {
                opts,
                tx,
                rx,
                lanes,
                host,
                keys: Keys {
                    log: log.clone(),
                    stuck: Arc::default(),
                },
                log,
                frames: Log::default(),
            }
        }

        fn ui(&self) -> Ui<'_, FakeTty, Keys> {
            Ui::new(
                &self.opts,
                FakeTty::new(self.log.clone(), self.frames.clone()),
                self.keys.clone(),
                &self.rx,
                &self.lanes,
                self.host.as_ref(),
            )
        }

        fn key(&self, c: char) {
            let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
            self.tx.send(Msg::Key(key)).unwrap();
        }

        fn finish(self, ending: Ending) -> TuiExit {
            finish(self.lanes, &self.opts.shutdown, &self.rx, ending, || {})
        }
    }

    #[test]
    fn the_first_frame_waits_for_the_first_accounts_read() {
        let fx = Fixture::new(quick(), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            assert!(
                ui.state.accounts.snapshot().is_some(),
                "drawn with the accounts, not `loading…`"
            );
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn a_slow_first_read_is_drawn_after_100_ms_without_it() {
        let fx = Fixture::new(
            sources(Duration::from_millis(600), false, &Arc::default()),
            |_| idle(),
        );
        let ending = {
            let mut ui = fx.ui();
            let started = Instant::now();
            ui.start();
            let took = started.elapsed();
            assert!(
                took >= FIRST_FRAME && took < Duration::from_millis(500),
                "{took:?}"
            );
            assert!(ui.state.accounts.snapshot().is_none());
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn the_first_collection_is_asked_for_after_the_first_frame() {
        // Spec §6.1: the TUI collects after drawing, not a timer's 30 s later.
        let frames = Log::default();
        let mut fx = Fixture::new(one_account(Duration::ZERO, &frames), |_| idle());
        fx.frames = frames;
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            seen_within(&fx.frames, "collect");
            fx.key('q');
            ui.run()
        };
        let seen = fx.frames.seen();
        assert_eq!(seen[..2], ["draw", "collect"], "{seen:?}");
        assert_eq!(seen.iter().filter(|&&e| e == "collect").count(), 1);
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn accounts_that_come_after_the_loading_frame_are_collected_after_their_own_frame() {
        let frames = Log::default();
        let mut fx = Fixture::new(one_account(Duration::from_millis(300), &frames), |_| idle());
        fx.frames = frames;
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            assert_eq!(fx.frames.seen(), ["draw"], "`loading…`, and no collection");
            let (frames, tx) = (fx.frames.clone(), fx.tx.clone());
            // `q` once the collection came, or once it is clear it will not: the loop ends.
            let quit = thread::spawn(move || {
                seen_within(&frames, "collect");
                let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
                tx.send(Msg::Key(q)).unwrap();
            });
            let ending = ui.run();
            quit.join().unwrap();
            ending
        };
        let seen = fx.frames.seen();
        let collect = seen.iter().position(|&e| e == "collect");
        let collect = collect.unwrap_or_else(|| panic!("nothing was collected: {seen:?}"));
        assert!(
            seen[..collect].iter().filter(|&&e| e == "draw").count() >= 2,
            "the loading frame, then the accounts': {seen:?}"
        );
        assert_eq!(seen.iter().filter(|&&e| e == "collect").count(), 1);
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn q_with_idle_lanes_joins_them_promptly_and_exits_0() {
        let drops = Arc::default();
        let fx = Fixture::new(sources(Duration::ZERO, false, &drops), |_| idle());
        let shutdown = fx.opts.shutdown.clone();
        let started = Instant::now();
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.key('q');
            ui.run()
        };
        let exit = fx.finish(ending);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!((exit.code, exit.report), (0, None));
        assert_eq!(
            drops.load(Ordering::SeqCst),
            2,
            "both lanes ended before the TUI returned"
        );
        assert_eq!(
            shutdown.requested(),
            Some(libc::SIGINT),
            "§10.2 step 1: SIGINT for q"
        );
    }

    #[test]
    fn q_while_an_action_runs_waits_for_it_and_reports_what_it_did() {
        const SPAN: Duration = Duration::from_millis(300);
        const SWITCHED: &str = "Switched to a@x.co (position 1).\n";
        const TOO_LATE: &str =
            "tagteam: interrupted too late to stop: switch had already finished\n";
        let started = Arc::new(AtomicBool::new(false));
        let running = Arc::clone(&started);
        let fx = Fixture::new(quick(), move |opts| {
            let shutdown = opts.shutdown.clone();
            Scripted {
                // Inside its critical span when the shutdown begins, so it runs to the end
                // (core §14.1) and reports itself switched.
                act: Box::new(move |_| {
                    running.store(true, Ordering::SeqCst);
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while shutdown.requested().is_none() && Instant::now() < deadline {
                        thread::sleep(Duration::from_millis(5));
                    }
                    thread::sleep(SPAN);
                    done(0, SWITCHED, TOO_LATE)
                }),
                foreground: never(),
            }
        });
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            let switch = Action::Switch(account_a());
            ui.state.running = Some(switch.clone());
            ui.execute(vec![Cmd::Act(switch)]);
            let deadline = Instant::now() + Duration::from_secs(5);
            while !started.load(Ordering::SeqCst) {
                assert!(Instant::now() < deadline, "the action never started");
                thread::sleep(Duration::from_millis(5));
            }
            fx.key('q');
            let ending = ui.run();
            assert!(
                ui.tty.text().contains("finishing switch"),
                "{}",
                ui.tty.text()
            );
            ending
        };
        let joining = Instant::now();
        let exit = fx.finish(ending);
        assert!(
            joining.elapsed() >= SPAN - Duration::from_millis(50),
            "the actions lane was joined: {:?}",
            joining.elapsed()
        );
        assert_eq!(exit.code, 0);
        assert_eq!(
            exit.report.as_deref(),
            Some(format!("{SWITCHED}{TOO_LATE}").as_str())
        );
    }

    #[test]
    fn a_sigterm_is_seen_within_one_wake_and_ends_with_143() {
        let fx = Fixture::new(quick(), |_| idle());
        let (signal, shutdown) = (fx.opts.signal.clone(), fx.opts.shutdown.clone());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            let sender = thread::spawn(move || {
                thread::sleep(Duration::from_millis(50));
                signal.request(libc::SIGTERM);
            });
            let started = Instant::now();
            let ending = ui.run();
            sender.join().unwrap();
            assert!(
                started.elapsed() < Duration::from_millis(600),
                "{:?}",
                started.elapsed()
            );
            ending
        };
        assert_eq!(
            fx.log.take(),
            ["pause", "release"],
            "a signal restores the terminal first (§10.2 step 4)"
        );
        let exit = fx.finish(ending);
        assert_eq!((exit.code, exit.report), (143, None));
        assert_eq!(shutdown.requested(), Some(libc::SIGTERM));
    }

    #[test]
    fn a_signal_that_arrives_during_the_wait_comes_before_the_key_that_ends_it() {
        // SIGTERM is recorded while the loop waits, then `q` wakes it: the shutdown is the
        // signal's (143, the terminal restored first), not `q`'s.
        let fx = Fixture::new(quick(), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            // After the loop's look at the token and its frame, before its wait ends.
            let (signal, tx) = (fx.opts.signal.clone(), fx.tx.clone());
            ui.tty.on_draw = Some(Box::new(move || {
                signal.request(libc::SIGTERM);
                let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
                tx.send(Msg::Key(q)).unwrap();
            }));
            ui.state.dirty = true;
            ui.run()
        };
        assert_eq!(fx.log.take(), ["pause", "release"]);
        let exit = fx.finish(ending);
        assert_eq!((exit.code, exit.report), (143, None));
    }

    #[test]
    fn an_outcome_that_arrives_with_a_signal_is_reported_after_the_tui() {
        const SWITCHED: &str = "Switched to a@x.co (position 1).\n";
        let fx = Fixture::new(quick(), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            ui.state.running = Some(Action::Switch(account_a()));
            let (signal, tx) = (fx.opts.signal.clone(), fx.tx.clone());
            ui.tty.on_draw = Some(Box::new(move || {
                signal.request(libc::SIGTERM);
                tx.send(Msg::ActionDone {
                    action: Action::Switch(account_a()),
                    outcome: done(0, SWITCHED, ""),
                })
                .unwrap();
            }));
            ui.state.dirty = true;
            ui.run()
        };
        let exit = fx.finish(ending);
        assert_eq!((exit.code, exit.report.as_deref()), (143, Some(SWITCHED)));
    }

    #[test]
    fn a_signal_during_the_first_frames_wait_comes_before_the_first_read() {
        let opts = options();
        let sources = signalled_during_the_first_read(&opts.signal);
        let fx = Fixture::with(opts, sources, |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            assert!(
                ui.state.accounts.snapshot().is_none(),
                "the read is not applied"
            );
            ui.run()
        };
        assert_eq!(fx.frames.seen(), Vec::<&str>::new(), "no frame was drawn");
        assert_eq!(fx.log.take(), ["pause", "release"]);
        assert_eq!(fx.finish(ending).code, 143);
    }

    #[test]
    fn a_sigint_sent_to_the_process_ends_with_130() {
        // In raw mode Ctrl-C is a key, so a SIGINT came from `kill -INT` (§10.2).
        let fx = Fixture::new(quick(), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.opts.signal.request(libc::SIGINT);
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 130);
    }

    #[test]
    fn a_sigint_left_by_a_suspended_action_is_taken_and_the_tui_carries_on() {
        let fx = Fixture::new(quick(), |opts| {
            let signal = opts.signal.clone();
            Scripted {
                act: never(),
                // Ctrl-C in the session: the terminal sent it to the TUI as well (§7.3).
                foreground: Box::new(move |_| {
                    signal.request(libc::SIGINT);
                    done(130, "", "")
                }),
            }
        });
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            let run = Action::Run(account_a());
            ui.state.running = Some(run.clone());
            ui.execute(vec![Cmd::Suspend(run)]);
            assert_eq!(
                fx.log.take(),
                ["pause", "release", "reclaim", "resume"],
                "§7.3 steps 1, 2 and 4"
            );
            assert_eq!(
                fx.opts.signal.requested(),
                None,
                "the action's SIGINT is spent"
            );
            assert!(ui.ending.is_none(), "the TUI carries on");
            assert!(
                ui.state.overlay.is_none() && ui.state.running.is_none(),
                "R12: the outcome shows nothing, so `q` is not an overlay's key (R9)"
            );
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn a_sigterm_during_a_suspension_shuts_down_without_taking_the_terminal_back() {
        let fx = Fixture::new(quick(), |opts| {
            let signal = opts.signal.clone();
            Scripted {
                act: never(),
                foreground: Box::new(move |_| {
                    signal.request(libc::SIGTERM);
                    done(143, "", "")
                }),
            }
        });
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            ui.execute(vec![Cmd::Suspend(Action::Run(account_a()))]);
            assert_eq!(fx.log.take(), ["pause", "release"]);
            ui.run()
        };
        let exit = fx.finish(ending);
        assert_eq!((exit.code, exit.report), (143, None));
    }

    #[test]
    fn a_sigterm_that_replaced_the_actions_sigint_still_ends_the_tui_with_143() {
        let fx = Fixture::new(quick(), |opts| {
            let signal = opts.signal.clone();
            Scripted {
                act: never(),
                // Ctrl-C in the session, then a `kill` of the TUI before it looks.
                foreground: Box::new(move |_| {
                    signal.request(libc::SIGINT);
                    signal.request(libc::SIGTERM);
                    done(130, "", "")
                }),
            }
        });
        let signal = fx.opts.signal.clone();
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            ui.execute(vec![Cmd::Suspend(Action::Run(account_a()))]);
            assert_eq!(fx.log.take(), ["pause", "release"]);
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 143);
        assert_eq!(signal.requested(), Some(libc::SIGTERM), "not spent");
    }

    #[test]
    fn a_suspension_is_refused_while_the_keys_do_not_pause() {
        // The input thread may still read: the action does not run (`never` would panic), and
        // the TUI keeps the terminal and says why.
        let fx = Fixture::new(quick(), |_| idle());
        fx.keys.stuck.store(true, Ordering::SeqCst);
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            let run = Action::Run(account_a());
            ui.state.running = Some(run.clone());
            ui.execute(vec![Cmd::Suspend(run)]);
            assert_eq!(fx.log.take(), ["pause", "resume"], "never handed over");
            assert!(ui.ending.is_none() && ui.state.running.is_none());
            ui.draw();
            assert!(
                ui.tty.text().contains(
                    "run did not start: the TUI's key reader did not pause in time; try again"
                ),
                "{}",
                ui.tty.text()
            );
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn ctrl_z_is_refused_while_the_keys_do_not_pause() {
        let fx = Fixture::new(quick(), |_| idle());
        fx.keys.stuck.store(true, Ordering::SeqCst);
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            ui.execute(vec![Cmd::Stop]);
            assert_eq!(fx.log.take(), ["pause", "resume"], "never stopped");
            assert_eq!(
                ui.state.banner.as_deref(),
                Some(
                    "Ctrl-Z did not suspend: the TUI's key reader did not pause in time; try again"
                )
            );
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn a_signal_gives_the_terminal_back_even_while_the_keys_do_not_pause() {
        // The process is exiting: the terminal is restored all the same.
        let fx = Fixture::new(quick(), |_| idle());
        fx.keys.stuck.store(true, Ordering::SeqCst);
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            fx.opts.signal.request(libc::SIGTERM);
            ui.run()
        };
        assert_eq!(fx.log.take(), ["pause", "release"]);
        assert_eq!(fx.finish(ending).code, 143);
    }

    #[test]
    fn ctrl_z_hands_the_terminal_back_and_takes_it_again() {
        let fx = Fixture::new(quick(), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.log.take();
            ui.execute(vec![Cmd::Stop]);
            assert_eq!(fx.log.take(), ["pause", "stop", "resume"]);
            assert!(
                ui.ending.is_none() && ui.state.dirty,
                "redrawn, and carrying on"
            );
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn a_lane_that_dies_ends_the_tui_with_exit_1_and_says_so() {
        let fx = Fixture::new(sources(Duration::ZERO, true, &Arc::default()), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            ui.run()
        };
        let exit = fx.finish(ending);
        assert_eq!(exit.code, 1);
        assert_eq!(
            exit.report.as_deref(),
            Some("tagteam: the TUI's views lane panicked; the log has the details\n")
        );
    }

    #[test]
    fn a_panic_on_the_ui_thread_still_joins_every_lane_and_exits_1() {
        let drops = Arc::default();
        let opts = options();
        let shutdown = opts.shutdown.clone();
        let exit = run_with(
            opts,
            sources(Duration::ZERO, false, &drops),
            Arc::new(idle()),
            || {
                Ok(FakeTty {
                    panics: true,
                    ..FakeTty::new(Log::default(), Log::default())
                })
            },
            |_tx| Ok(Keys::default()),
            || {},
        );
        assert_eq!((exit.code, exit.report), (1, None));
        assert_eq!(drops.load(Ordering::SeqCst), 2, "both lanes were joined");
        assert_eq!(shutdown.requested(), Some(libc::SIGINT));
    }
}
```

These tests drive Task 5's `update` and `view` for real: `q` gives `Cmd::Quit { code: 0 }`
(also while an action runs), `LaneDied` gives `Cmd::Quit { code: 1 }`, and the footer reads
`finishing switch…` while `quitting` is set and `running` holds a switch. `Action::Switch`'s
spelling there is `Action::name()`.

The interleavings are driven by seams, never by sleeps: `FakeTty::on_draw` runs after the loop
has looked at the token and drawn, before its wait ends (R17); `Keys::stuck` is an input thread
that never acknowledges (R18); `frames` logs each frame and each collection in order (R19); the
term tests hold a frame open on a channel while a hook waits for the lock (R16); and input's
`Gate` holds the thread between its look at the control and its poll (R18).

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-provider --lib term::`
Expected: FAIL to compile: `cannot find function stop_self in this scope`.

Run: `cargo test -p tagteam-tui --lib`
Expected: FAIL to compile: `cannot find struct Ownership`, `cannot find trait Events`,
`cannot find struct Lanes` (and the rest of what Step 3 defines).

- [ ] **Step 3: Implement**

In `crates/tagteam-provider/src/term.rs`, replace the module doc (lines 1–3) with:

```rust
//! The terminal, descriptor and job-control calls that the development harness (`xtask`) and
//! the TUI (`tagteam-tui`) need, as safe functions: the workspace allows `unsafe` only here and
//! in `tagteam`, so both forbid it and call these.
```

and add after `child_is_stopped` (line 177):

```rust
/// Ctrl-Z for a program that reads its keys raw (TUI spec §10.1): stops this process with
/// SIGTSTP, as the terminal would have, and returns once it is continued (SIGCONT). Where
/// nothing does job control the signal is discarded, and this returns at once.
pub fn stop_self() -> io::Result<()> {
    // SAFETY: raise(3) takes a signal number and touches no memory of ours.
    if unsafe { libc::raise(libc::SIGTSTP) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
```

In `crates/tagteam-tui/Cargo.toml`, `[dependencies]`, add (keeping the list sorted):

```toml
libc.workspace = true
```

and check that `crossterm`, `ratatui`, `tagteam-core`, `tagteam-engine` and `tagteam-provider`
are there with `.workspace = true`; add any that is missing.

In `crates/tagteam-tui/src/lib.rs`, after the module list (before Task 5's
`pub use state::Screen;`), add:

```rust
pub use runtime::{TuiExit, TuiOptions, run};
```

In `crates/tagteam-tui/src/ask.rs`, delete the three `#[allow(dead_code)]` lines Task 9 put on
`SLICE`, `struct ChannelAsk` and `impl ChannelAsk`: `Lanes::spawn` below builds a `ChannelAsk`
for every action, so nothing in the file is dead any more.

Replace `crates/tagteam-tui/src/term.rs` with (its tests are Step 1's, at the end of the file):

```rust
//! The terminal (TUI spec §10.1): raw mode, the alternate screen and a hidden cursor while the
//! TUI holds it, all three undone on every way out, a panic's included. One lock serializes
//! drawing with every change of the terminal's owner, a panic hook's restore included, so no
//! frame is ever written to the main screen. `Tty` is what the loop needs of a terminal, so the
//! loop's tests can use a `TestBackend` instead.

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, Once, PoisonError};

use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::Rect;
use ratatui::{Frame, Terminal};

/// The process's terminal.
static TERMINAL: Ownership = Ownership::new();
static HOOK: Once = Once::new();

thread_local! {
    /// This thread's key for `Ownership::holder`, never 0.
    static KEY: u64 = {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    };
}

/// This thread's key, or 0 once its thread-locals are gone (it holds no lock by then).
fn thread_key() -> u64 {
    KEY.try_with(|key| *key).unwrap_or(0)
}

/// Whether the TUI holds a terminal, and the lock that every change of that, and every frame,
/// is made under (§10.1).
pub(crate) struct Ownership {
    lock: Mutex<()>,
    /// Set by an enter, cleared by the restore that gives the terminal back. Changed only under
    /// `lock`.
    entered: AtomicBool,
    /// The key of the thread inside `lock` now, or 0.
    holder: AtomicU64,
    /// How many threads wait for `lock` now: what the tests watch to see a hook held up.
    waiting: AtomicUsize,
}

/// `lock`, held. Its holder is known until it is let go.
struct Held<'a> {
    ownership: &'a Ownership,
    _guard: MutexGuard<'a, ()>,
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.ownership.holder.store(0, Ordering::SeqCst);
    }
}

impl Ownership {
    pub(crate) const fn new() -> Ownership {
        Ownership {
            lock: Mutex::new(()),
            entered: AtomicBool::new(false),
            holder: AtomicU64::new(0),
            waiting: AtomicUsize::new(0),
        }
    }

    /// Takes the lock. A panic while it was held poisons it; it still serializes.
    fn hold(&self) -> Held<'_> {
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        self.waiting.fetch_sub(1, Ordering::SeqCst);
        self.holder.store(thread_key(), Ordering::SeqCst);
        Held {
            ownership: self,
            _guard: guard,
        }
    }

    /// Marks the terminal taken, then runs `take`, under the lock: a `take` that fails part way
    /// is undone by a restore too, and a panic hook restores before or after it, never during.
    fn enter_with<R>(&self, take: impl FnOnce() -> io::Result<R>) -> io::Result<R> {
        let _held = self.hold();
        self.entered.store(true, Ordering::SeqCst);
        take()
    }

    /// Runs `give_back` if, and only if, the terminal is taken, and marks it given back.
    fn restore_with(&self, give_back: impl FnOnce()) {
        let _held = self.hold();
        self.give_back(give_back);
    }

    /// The panic hook's restore. A thread that panics while it holds the lock (in `view`,
    /// inside a draw) owns the terminal already, and must not wait for itself. Any other waits
    /// for the frame in flight, so the frame lands on the alternate screen before it is left.
    fn restore_on_panic(&self, give_back: impl FnOnce()) {
        let me = thread_key();
        if me != 0 && self.holder.load(Ordering::SeqCst) == me {
            self.give_back(give_back);
        } else {
            self.restore_with(give_back);
        }
    }

    fn give_back(&self, give_back: impl FnOnce()) {
        if self.entered.swap(false, Ordering::SeqCst) {
            give_back();
        }
    }

    /// Runs `draw` if the terminal is taken, the check and the whole of `draw` under the lock;
    /// `None`, and nothing drawn, once it has been given back.
    fn while_entered<R>(&self, draw: impl FnOnce() -> R) -> Option<R> {
        let _held = self.hold();
        self.entered.load(Ordering::SeqCst).then(draw)
    }

    fn entered(&self) -> bool {
        self.entered.load(Ordering::SeqCst)
    }
}

/// Raw mode, the alternate screen, the cursor hidden.
fn take() -> io::Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, Hide)
}

/// Raw mode off first (it has more side effects, as ratatui's own restore says), then the main
/// screen and the cursor.
fn give_back() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
}

/// Gives the terminal back, once however often it is called.
pub(crate) fn restore() {
    TERMINAL.restore_with(give_back);
}

/// §10.1: a panic on any thread restores the terminal, then goes on to the hook installed
/// before: logging's, which logs it and prints it on stderr (core §14.2).
fn install_panic_hook() {
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            TERMINAL.restore_on_panic(give_back);
            previous(info);
        }));
    });
}

/// Clears the screen and forgets the last frame, so the next draw writes every cell. Never
/// `Terminal::clear`, which first asks the terminal where its cursor is, and waits for the
/// answer.
pub(crate) fn repaint<B: Backend>(terminal: &mut Terminal<B>) -> Result<(), B::Error> {
    let size = terminal.size()?;
    terminal.resize(Rect::new(0, 0, size.width, size.height))
}

/// What the loop needs of the terminal (§7.3, §10.1).
pub(crate) trait Tty {
    type B: Backend;
    /// Draws a frame with `render`, unless the terminal has been given back: then `Ok(false)`,
    /// and nothing is written. The check, the frame and its flush hold the terminal's lock, so
    /// a panic hook on another thread gives the terminal back before the frame or after it.
    fn draw(
        &mut self,
        render: impl FnOnce(&mut Frame),
    ) -> Result<bool, <Self::B as Backend>::Error>;
    /// Its size now, in columns and rows.
    fn size(&mut self) -> io::Result<(u16, u16)>;
    /// Whether the TUI holds it: whether there is anything to give back. A panic hook may give
    /// it back just after; `release` is then a no-op.
    fn active(&self) -> bool;
    /// Gives it back: raw mode off, the main screen, the cursor shown (§7.3 step 2).
    fn release(&mut self);
    /// Takes it again, and repaints (§7.3 step 4).
    fn reclaim(&mut self) -> io::Result<()>;
    /// Ctrl-Z: gives it back, stops the process, and takes it again once continued (§10.1).
    fn stop(&mut self) -> io::Result<()>;
}

/// The process's terminal, on stdout.
pub(crate) struct RealTty {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl RealTty {
    /// Takes the terminal (§10.1) and clears the alternate screen. The panic hook is in place
    /// before, so a panic from here on finds the terminal restored first.
    pub(crate) fn enter() -> io::Result<RealTty> {
        install_panic_hook();
        TERMINAL.enter_with(|| {
            take()?;
            let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
            repaint(&mut terminal)?;
            Ok(RealTty { terminal })
        })
    }
}

impl Tty for RealTty {
    type B = CrosstermBackend<io::Stdout>;

    fn draw(&mut self, render: impl FnOnce(&mut Frame)) -> io::Result<bool> {
        let terminal = &mut self.terminal;
        TERMINAL
            .while_entered(|| terminal.draw(render).map(|_| ()))
            .map_or(Ok(false), |drawn| drawn.map(|()| true))
    }

    fn size(&mut self) -> io::Result<(u16, u16)> {
        crossterm::terminal::size()
    }

    fn active(&self) -> bool {
        TERMINAL.entered()
    }

    fn release(&mut self) {
        restore();
    }

    fn reclaim(&mut self) -> io::Result<()> {
        let terminal = &mut self.terminal;
        TERMINAL.enter_with(|| {
            take()?;
            repaint(terminal)
        })
    }

    fn stop(&mut self) -> io::Result<()> {
        restore();
        // Without job control the signal is discarded, and the TUI simply carries on.
        let _ = tagteam_provider::term::stop_self();
        self.reclaim()
    }
}

impl Drop for RealTty {
    /// ratatui's `Terminal` shows the cursor as it is dropped, when it hid it: a write like any
    /// other, so it is made here, under the lock, and the drop that follows writes nothing.
    fn drop(&mut self) {
        let _held = TERMINAL.hold();
        let _ = self.terminal.show_cursor();
    }
}
```

Replace `crates/tagteam-tui/src/input.rs` with (its tests are Step 1's):

```rust
//! The input thread (TUI spec §4.3): the terminal's events as `Msg`s. It polls in 50 ms slices,
//! so it can pause, and acknowledge the pause, before the terminal is handed to a foreground
//! action (§7.3 step 1), and it reads nothing once the TUI has let the terminal go.

use std::io;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyEventKind};

use crate::msg::Msg;

/// How long one poll waits before the thread looks at its control again.
const SLICE: Duration = Duration::from_millis(50);
/// How long `pause` and dropping wait for the thread to say so. Far more than a slice: a thread
/// that takes longer is held up somewhere between its look at the control and its poll, or
/// gone.
const ACK: Duration = Duration::from_secs(1);

/// Where terminal events come from: crossterm in the TUI, a script in the tests.
pub(crate) trait Events: Send + 'static {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool>;
    fn read(&mut self) -> io::Result<Event>;
}

/// The process's terminal, through crossterm's reader.
pub(crate) struct Crossterm;

impl Events for Crossterm {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        crossterm::event::poll(timeout)
    }

    fn read(&mut self) -> io::Result<Event> {
        crossterm::event::read()
    }
}

/// What the UI thread asks of the input thread (§7.3 step 1).
pub(crate) trait Pause {
    /// True once the thread reads no more: it is between two polls, and polls again only after
    /// `resume`. False when it has not said so within `ACK`: it may still poll, and read keys
    /// meant for the terminal's next owner, so the terminal must not be handed over. The pause
    /// stays asked for until `resume`.
    fn pause(&self) -> bool;
    fn resume(&self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Reading,
    PauseAsked,
    Paused,
    StopAsked,
    Stopped,
}

struct Control {
    mode: Mutex<Mode>,
    changed: Condvar,
}

impl Control {
    fn lock(&self) -> MutexGuard<'_, Mode> {
        self.mode.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set(&self, mode: &mut MutexGuard<'_, Mode>, to: Mode) {
        **mode = to;
        self.changed.notify_all();
    }

    /// Waits while the mode is `from`, for at most `ACK`. False when it still is.
    fn wait_while(&self, mut mode: MutexGuard<'_, Mode>, from: Mode) -> bool {
        let deadline = Instant::now() + ACK;
        while *mode == from {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            mode = self
                .changed
                .wait_timeout(mode, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

/// The input thread's handle. Dropping it stops the thread once the thread has said so; the
/// thread is never joined (§10.2).
pub(crate) struct Input {
    control: Arc<Control>,
}

impl Input {
    pub(crate) fn spawn(tx: Sender<Msg>) -> io::Result<Input> {
        Input::spawn_with(tx, Crossterm)
    }

    pub(crate) fn spawn_with(tx: Sender<Msg>, events: impl Events) -> io::Result<Input> {
        let control = Arc::new(Control {
            mode: Mutex::new(Mode::Reading),
            changed: Condvar::new(),
        });
        let theirs = Arc::clone(&control);
        thread::Builder::new()
            .name("tagteam-tui-input".into())
            .spawn(move || read(&theirs, events, &tx))?;
        Ok(Input { control })
    }
}

/// The thread: reads while the mode is `Reading`, and between two polls acknowledges a pause or
/// a stop.
fn read(control: &Control, mut events: impl Events, tx: &Sender<Msg>) {
    loop {
        {
            let mut mode = control.lock();
            loop {
                let now = *mode;
                match now {
                    Mode::Reading => break,
                    Mode::PauseAsked => control.set(&mut mode, Mode::Paused),
                    Mode::Paused => {
                        mode = control
                            .changed
                            .wait(mode)
                            .unwrap_or_else(PoisonError::into_inner);
                    }
                    Mode::StopAsked | Mode::Stopped => {
                        control.set(&mut mode, Mode::Stopped);
                        return;
                    }
                }
            }
        }
        match events.poll(SLICE) {
            Ok(true) => match events.read() {
                Ok(event) => {
                    let sent = message(event).is_none_or(|msg| tx.send(msg).is_ok());
                    if !sent {
                        // The UI thread is gone: nothing reads these any more.
                        control.set(&mut control.lock(), Mode::Stopped);
                        return;
                    }
                }
                Err(_) => thread::sleep(SLICE),
            },
            Ok(false) => {}
            // A terminal that cannot be read now: a hang-up arrives as SIGHUP. No spinning.
            Err(_) => thread::sleep(SLICE),
        }
    }
}

/// The events the TUI acts on: key presses (and repeats) and resizes.
fn message(event: Event) -> Option<Msg> {
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => Some(Msg::Key(key)),
        Event::Resize(width, height) => Some(Msg::Resize { width, height }),
        _ => None,
    }
}

impl Pause for Input {
    fn pause(&self) -> bool {
        let mut mode = self.control.lock();
        if *mode == Mode::Reading {
            self.control.set(&mut mode, Mode::PauseAsked);
        }
        self.control.wait_while(mode, Mode::PauseAsked)
    }

    fn resume(&self) {
        let mut mode = self.control.lock();
        if matches!(*mode, Mode::PauseAsked | Mode::Paused) {
            self.control.set(&mut mode, Mode::Reading);
        }
    }
}

impl Drop for Input {
    /// Stops the thread before the terminal is given back, so it never reads the shell's
    /// input. The TUI is ending, so nothing waits beyond `ACK`: a thread held up longer stops
    /// at its next look.
    fn drop(&mut self) {
        let mut mode = self.control.lock();
        if *mode != Mode::Stopped {
            self.control.set(&mut mode, Mode::StopAsked);
        }
        let _ = self.control.wait_while(mode, Mode::StopAsked);
    }
}
```

`Option::is_none_or` is stable since Rust 1.82, inside the 1.88 MSRV (R1).

Replace `crates/tagteam-tui/src/runtime.rs` with (its tests are Step 1's):

```rust
//! The UI thread (TUI spec §4.3): the lanes it drives, the loop that runs `update` and draws
//! `view`, the suspension that hands the terminal to a foreground action (§7.3), and the one
//! shutdown every way out goes through (§10.2).

use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tagteam_core::ProviderId;
use tagteam_provider::Cancel;

use crate::ask::ChannelAsk;
use crate::host::{Action, ActionOutcome, Host};
use crate::input::{Input, Pause};
use crate::lanes::Lane;
use crate::msg::{ActionsReq, Cmd, Msg, ViewsReq};
use crate::source::{Source, SourceFactory};
use crate::state::{Screen, State};
use crate::term::{self, RealTty, Tty};
use crate::update::{drawn, update};
use crate::view::view;

/// The loop wakes at least this often, to look at the signal token and to tick (§4.3).
const WAKE: Duration = Duration::from_millis(250);
/// How long the first frame waits for the first `accounts` (§6.1).
const FIRST_FRAME: Duration = Duration::from_millis(100);
/// Why a hand-over of the terminal was refused (§7.3 step 1): the input thread could still read
/// keys typed for the terminal's next owner.
const NOT_PAUSED: &str = "the TUI's key reader did not pause in time; try again";

/// How the TUI runs.
pub struct TuiOptions {
    pub screen: Screen,
    pub color: bool,
    pub provider: Option<ProviderId>,
    /// The process's signal token: the CLI's handlers record SIGINT, SIGTERM and SIGHUP in it.
    pub signal: Cancel,
    /// The shutdown token. The binary has already built every lane's engine, and the host's
    /// in-TUI context, with it.
    pub shutdown: Cancel,
}

/// How the TUI ended.
pub struct TuiExit {
    pub code: i32,
    /// Printed on stderr after the terminal is restored (spec §10.2 step 6).
    pub report: Option<String>,
}

/// Runs the TUI on the process's terminal until it is quit, signalled, or fails. It returns
/// once every lane has been joined and the terminal restored (§10.2).
pub fn run(opts: TuiOptions, sources: SourceFactory, host: Arc<dyn Host>) -> TuiExit {
    run_with(
        opts,
        sources,
        host,
        RealTty::enter,
        Input::spawn,
        term::restore,
    )
}

/// `run`, over the terminal `enter` sets up and the input thread `input` starts.
pub(crate) fn run_with<T: Tty, P: Pause>(
    opts: TuiOptions,
    sources: SourceFactory,
    host: Arc<dyn Host>,
    enter: impl FnOnce() -> io::Result<T>,
    input: impl FnOnce(Sender<Msg>) -> io::Result<P>,
    restore: fn(),
) -> TuiExit {
    let _restored = Restore(restore);
    let (tx, rx) = mpsc::channel();
    // The lanes build their engines while the terminal is set up (§6.1).
    let lanes = Lanes::spawn(opts.provider.clone(), &sources, &host, &tx, &opts.shutdown);
    let driven = panic::catch_unwind(AssertUnwindSafe(|| {
        let tty = match enter() {
            Ok(tty) => tty,
            Err(e) => {
                return Ending::fatal(format!("tagteam: the terminal could not be set up: {e}"));
            }
        };
        let input = match input(tx.clone()) {
            Ok(input) => input,
            Err(e) => {
                return Ending::fatal(format!(
                    "tagteam: the terminal's keys could not be read: {e}"
                ));
            }
        };
        let mut ui = Ui::new(&opts, tty, input, &rx, &lanes, host.as_ref());
        ui.start();
        ui.run()
    }));
    // A panic on the UI thread: the hook has restored the terminal and logged it (§10.1), and
    // the lanes still drain (§10.2).
    let ending = driven.unwrap_or(Ending {
        code: 1,
        fatal: None,
        report: String::new(),
    });
    finish(lanes, &opts.shutdown, &rx, ending, restore)
}

/// §10.2 steps 1, 2, 5 and 6, whatever ended the loop. The token is set and the lanes closed,
/// which a panic on the UI thread skipped; every lane is joined; the terminal is restored; and
/// the report is made: what an action still running did, in the CLI's words, then the error
/// that ended the TUI.
fn finish(
    lanes: Lanes,
    shutdown: &Cancel,
    rx: &Receiver<Msg>,
    ending: Ending,
    restore: fn(),
) -> TuiExit {
    if shutdown.requested().is_none() {
        shutdown.request(libc::SIGINT);
    }
    lanes.close();
    let died = lanes.join();
    restore();
    let mut report = ending.report;
    while let Ok(msg) = rx.try_recv() {
        report.push_str(&reported(msg));
    }
    let mut code = ending.code;
    let mut errors: Vec<String> = ending.fatal.into_iter().collect();
    for lane in died {
        errors.push(format!(
            "tagteam: the TUI's {lane} lane panicked; the log has the details"
        ));
        if code == 0 {
            code = 1;
        }
    }
    for error in errors {
        report.push_str(&error);
        report.push('\n');
    }
    TuiExit {
        code,
        report: (!report.is_empty()).then_some(report),
    }
}

/// What a message reports once the TUI has stopped reading (§10.2 step 6): an action's `out` and
/// `err`, in the CLI's words; nothing for any other message.
fn reported(msg: Msg) -> String {
    match msg {
        Msg::ActionDone {
            outcome: ActionOutcome::Done { out, err, .. },
            ..
        } => out + &err,
        _ => String::new(),
    }
}

/// §10.1: restores the terminal when dropped, on every way out of `run`.
struct Restore(fn());

impl Drop for Restore {
    fn drop(&mut self) {
        (self.0)();
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Part A's engine lanes (§4.3): views (`accounts`, `history`, store-served) and actions (the
/// collector and every `Host` action, one at a time).
pub(crate) struct Lanes {
    views: Lane<ViewsReq>,
    actions: Lane<ActionsReq>,
}

impl Lanes {
    /// Spawns both. Each builds its own `Source` in its own thread (R4), and a lane that panics
    /// says so on `tx` (§4.3, §10.4).
    pub(crate) fn spawn(
        provider: Option<ProviderId>,
        sources: &SourceFactory,
        host: &Arc<dyn Host>,
        tx: &Sender<Msg>,
        shutdown: &Cancel,
    ) -> Lanes {
        let views = {
            let (make, out, died) = (Arc::clone(sources), tx.clone(), tx.clone());
            Lane::spawn(
                "views",
                move || make(),
                move |source: &mut Box<dyn Source>, req: ViewsReq| {
                    let msg = match req {
                        ViewsReq::Accounts { r#gen } => Msg::Accounts {
                            r#gen,
                            result: source.accounts(provider.as_ref()),
                        },
                        ViewsReq::History {
                            r#gen,
                            account,
                            since_s,
                        } => Msg::History {
                            r#gen,
                            result: source.history(&account, since_s),
                        },
                    };
                    let _ = out.send(msg);
                },
                move || {
                    let _ = died.send(Msg::LaneDied { lane: "views" });
                },
            )
        };
        let actions = {
            let (make, out, died) = (Arc::clone(sources), tx.clone(), tx.clone());
            let (host, shutdown) = (Arc::clone(host), shutdown.clone());
            Lane::spawn(
                "actions",
                move || make(),
                move |source: &mut Box<dyn Source>, req: ActionsReq| {
                    let msg = match req {
                        ActionsReq::Collect { r#gen, accounts } => Msg::Collected {
                            r#gen,
                            result: source.collect_due(&accounts),
                        },
                        ActionsReq::Act { action } => {
                            // Its prompts become dialogs, declined once the shutdown begins
                            // (§7.2, §10.2 step 3).
                            let mut ask = ChannelAsk::new(out.clone(), shutdown.clone());
                            let outcome = host.act(&action, &mut ask);
                            Msg::ActionDone { action, outcome }
                        }
                    };
                    let _ = out.send(msg);
                },
                move || {
                    let _ = died.send(Msg::LaneDied { lane: "actions" });
                },
            )
        };
        Lanes { views, actions }
    }

    /// §10.2 step 2: nothing more is taken, what is queued is dropped, and an idle lane wakes
    /// and returns.
    fn close(&self) {
        self.views.close();
        self.actions.close();
    }

    /// §10.2 step 5: joins both lanes, and names those that panicked.
    fn join(self) -> Vec<&'static str> {
        let mut died = Vec::new();
        if self.views.join().is_err() {
            died.push("views");
        }
        if self.actions.join().is_err() {
            died.push("actions");
        }
        died
    }
}

/// How the loop ended (§10.2's table): the exit code, a fatal error's message, and what an
/// action reported in a message the shutdown overtook, for the report.
pub(crate) struct Ending {
    code: i32,
    fatal: Option<String>,
    report: String,
}

impl Ending {
    fn fatal(message: String) -> Ending {
        Ending {
            code: 1,
            fatal: Some(message),
            report: String::new(),
        }
    }
}

/// The UI thread's loop and what it owns (§4.3).
pub(crate) struct Ui<'a, T: Tty, P: Pause> {
    state: State,
    tty: T,
    input: P,
    rx: &'a Receiver<Msg>,
    lanes: &'a Lanes,
    host: &'a dyn Host,
    signal: Cancel,
    shutdown: Cancel,
    /// Set when the shutdown begins.
    ending: Option<Ending>,
    next_tick: Instant,
}

impl<'a, T: Tty, P: Pause> Ui<'a, T, P> {
    pub(crate) fn new(
        opts: &TuiOptions,
        tty: T,
        input: P,
        rx: &'a Receiver<Msg>,
        lanes: &'a Lanes,
        host: &'a dyn Host,
    ) -> Self {
        Ui {
            state: State::new(opts.screen, opts.color, opts.provider.clone(), now_ms()),
            tty,
            input,
            rx,
            lanes,
            host,
            signal: opts.signal.clone(),
            shutdown: opts.shutdown.clone(),
            ending: None,
            next_tick: Instant::now() + WAKE,
        }
    }

    /// §6.1: the size, the first loads, up to 100 ms for the first `accounts`, and the first
    /// frame, after which the first collection is asked for (`draw`). The signal token comes
    /// first here too: before the wait, and on every wake before the message (§10.2).
    pub(crate) fn start(&mut self) {
        self.resized();
        let cmds = self.state.start();
        self.execute(cmds);
        let deadline = Instant::now() + FIRST_FRAME;
        while !self.shutting_down() {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(msg) = self.rx.recv_timeout(left) else {
                break;
            };
            let accounts = matches!(msg, Msg::Accounts { .. });
            self.deliver(msg);
            if accounts {
                break;
            }
        }
        if self.ending.is_none() {
            self.state.dirty = true;
            self.draw();
        }
    }

    /// §4.3, until a shutdown begins. Each wake looks at the signal token before anything else
    /// (§10.2): before the wait, and again before the message or the tick that ended it. It
    /// redraws when the state changed, takes one message, and ticks every 250 ms whatever
    /// arrives.
    pub(crate) fn run(&mut self) -> Ending {
        loop {
            self.shutting_down();
            if let Some(ending) = self.ending.take() {
                return ending;
            }
            if self.state.dirty {
                self.draw();
            }
            let wait = self.next_tick.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(wait) {
                Ok(msg) => self.deliver(msg),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => self.begin_shutdown(
                    1,
                    libc::SIGINT,
                    Some("tagteam: the TUI lost its message channel".into()),
                ),
            }
            if self.ending.is_none() && Instant::now() >= self.next_tick {
                self.next_tick = Instant::now() + WAKE;
                self.deliver(Msg::Tick);
            }
        }
    }

    /// Begins the shutdown for a signal in the token: SIGTERM, SIGHUP, or a SIGINT sent to the
    /// process (in raw mode Ctrl-C is a key). True once a shutdown has begun, for any reason.
    fn shutting_down(&mut self) -> bool {
        if self.ending.is_none() {
            if let Some(signal) = self.signal.requested() {
                self.begin_shutdown(128 + signal, signal, None);
            }
        }
        self.ending.is_some()
    }

    /// Applies `msg`, after a look at the signal token: a signal that arrived with it comes
    /// first, and so does a shutdown already begun. Then `msg` is not applied; an action's
    /// outcome in it still goes into the report (§10.2 step 6).
    fn deliver(&mut self, msg: Msg) {
        if !self.shutting_down() {
            self.apply(msg);
        } else if let Some(ending) = &mut self.ending {
            ending.report.push_str(&reported(msg));
        }
    }

    fn apply(&mut self, msg: Msg) {
        let cmds = update(&mut self.state, msg, now_ms());
        self.execute(cmds);
    }

    fn execute(&mut self, cmds: Vec<Cmd>) {
        for cmd in cmds {
            // Once the shutdown has begun nothing new starts (§10.2 step 2).
            if self.ending.is_some() {
                return;
            }
            match cmd {
                Cmd::LoadAccounts { r#gen } => {
                    let _ = self.lanes.views.submit(ViewsReq::Accounts { r#gen });
                }
                Cmd::LoadHistory {
                    r#gen,
                    account,
                    since_s,
                } => {
                    let req = ViewsReq::History {
                        r#gen,
                        account,
                        since_s,
                    };
                    let _ = self.lanes.views.submit(req);
                }
                Cmd::CollectDue { r#gen, accounts } => {
                    let _ = self
                        .lanes
                        .actions
                        .submit(ActionsReq::Collect { r#gen, accounts });
                }
                Cmd::Act(action) => {
                    let _ = self.lanes.actions.submit(ActionsReq::Act { action });
                }
                Cmd::Suspend(action) => self.suspend(action),
                Cmd::Quit { code } => self.begin_shutdown(code, libc::SIGINT, None),
                Cmd::Stop => self.stop(),
            }
        }
    }

    /// Draws the state, unless the terminal has been given back: a panic hook may have done so
    /// from another thread, and `Tty::draw` checks under the lock the hook takes. After a frame,
    /// the first that shows accounts asks for the first collection (§6.1).
    fn draw(&mut self) {
        let now = now_ms();
        let state = &self.state;
        let result = self
            .tty
            .draw(|frame| view(state, frame, now))
            .map_err(|e| e.to_string());
        match result {
            Ok(true) => {
                self.state.dirty = false;
                let cmds = drawn(&mut self.state);
                self.execute(cmds);
            }
            Ok(false) => {}
            // §10.4: a terminal that can no longer be written is fatal. Once the shutdown has
            // begun there is nothing more to do about it.
            Err(e) if self.ending.is_none() => self.begin_shutdown(
                1,
                libc::SIGINT,
                Some(format!("tagteam: the terminal could not be written: {e}")),
            ),
            Err(_) => {}
        }
    }

    /// §10.2 steps 1 to 4. A second trigger, a second `q` among them, changes nothing.
    fn begin_shutdown(&mut self, code: i32, signal: i32, fatal: Option<String>) {
        if self.ending.is_some() {
            return;
        }
        self.shutdown.request(signal);
        self.lanes.close();
        self.state.quitting = Some(code);
        self.state.dirty = true;
        self.ending = Some(Ending {
            code,
            fatal,
            report: String::new(),
        });
        if code == 0 {
            // `q` or Ctrl-C: `finishing <action>…` stays up while the lanes drain.
            self.draw();
        } else {
            // A signal or a fatal error: the terminal is restored first.
            self.hand_back();
        }
    }

    /// §10.2 step 4 for a signal or a fatal error. The input thread is asked to pause, and the
    /// terminal is given back whether it says so or not: the process is exiting.
    fn hand_back(&mut self) {
        if self.tty.active() {
            let _ = self.input.pause();
            self.tty.release();
        }
    }

    /// §7.3 step 1, before the terminal is handed over: true once the input thread has said it
    /// reads no more. Otherwise the pause is withdrawn, the terminal kept, and the banner says
    /// that `refused`, and why.
    fn paused(&mut self, refused: &str) -> bool {
        if self.input.pause() {
            return true;
        }
        self.input.resume();
        self.state.banner = Some(format!("{refused}: {NOT_PAUSED}"));
        self.state.dirty = true;
        false
    }

    /// §7.3: runs `action` with the terminal handed back, then takes the terminal again. When
    /// the input thread does not pause, nothing runs and the TUI keeps the terminal.
    fn suspend(&mut self, action: Action) {
        if !self.paused(&format!("{} did not start", action.name())) {
            self.state.running = None;
            return;
        }
        self.tty.release();
        let outcome = self.host.act_foreground(&action);
        // A SIGINT recorded by now was the action's: Ctrl-C at its prompt or in its session
        // (§7.3). Only a SIGINT is spent, in one step, so a SIGTERM or SIGHUP that replaced it
        // stays recorded.
        self.signal.take_if(libc::SIGINT);
        if let Some(signal) = self.signal.requested() {
            // SIGTERM or SIGHUP: the shutdown, with the terminal as the action left it.
            self.begin_shutdown(128 + signal, signal, None);
            return;
        }
        if let Err(e) = self.tty.reclaim() {
            self.begin_shutdown(
                1,
                libc::SIGINT,
                Some(format!(
                    "tagteam: the terminal could not be set up again: {e}"
                )),
            );
            return;
        }
        self.input.resume();
        self.resized();
        let outcome = match outcome {
            // A foreground action has the terminal; one that asks for it again would loop.
            ActionOutcome::NeedsTerminal => ActionOutcome::Done {
                exit: 1,
                out: String::new(),
                err: String::new(),
            },
            done => done,
        };
        self.deliver(Msg::ActionDone { action, outcome });
        self.state.dirty = true;
    }

    /// Ctrl-Z (§10.1): the terminal is given back while the process is stopped, and taken
    /// again, and redrawn, once it is continued. Refused, as a suspension is, when the input
    /// thread does not pause.
    fn stop(&mut self) {
        if !self.paused("Ctrl-Z did not suspend") {
            return;
        }
        match self.tty.stop() {
            Ok(()) => {
                self.input.resume();
                self.resized();
                self.state.dirty = true;
            }
            Err(e) => self.begin_shutdown(
                1,
                libc::SIGINT,
                Some(format!(
                    "tagteam: the terminal could not be set up again: {e}"
                )),
            ),
        }
    }

    /// The terminal's size, as `update` takes it.
    fn resized(&mut self) {
        if let Ok((width, height)) = self.tty.size() {
            self.apply(Msg::Resize { width, height });
        }
    }
}
```

- [ ] **Step 4: Run them and see them pass**, then the crates' whole suites

Run: `cargo test -p tagteam-provider --lib term::`
Expected: PASS, 5 tests (3 in `tests`, 2 in `spawn_tests`).

Run: `cargo test -p tagteam-tui --lib -- term:: input:: runtime::`
Expected: PASS, 32 tests (7 + 5 + 20).

Run: `cargo test -p tagteam-tui && cargo test -p tagteam-provider`
Expected: PASS.

- [ ] **Step 5: Format and lint**

`cargo fmt --all && cargo fmt --all --check`, then
`cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`, then
`cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider/src/term.rs crates/tagteam-tui/Cargo.toml \
  crates/tagteam-tui/src/lib.rs crates/tagteam-tui/src/term.rs crates/tagteam-tui/src/input.rs \
  crates/tagteam-tui/src/runtime.rs crates/tagteam-tui/src/ask.rs Cargo.lock
git commit -m "Run the TUI's loop, suspension and shutdown over its lanes"
```

---

### Task 12: The `tui` command, bare `tagteam`, and docs

`tagteam tui [accounts|history]` and a bare `tagteam` on a terminal open the TUI (spec §3). The
binary refuses what it must before the terminal is touched, builds the TUI's engines as
`app::run_command` builds one, and prints the TUI's report once the terminal is restored.

**Readings of the spec this task commits to:**
- **Which calls open the TUI** (`lib::opens_tui`): `tui` always does, and its refusals are its
  own; a bare `tagteam` does when stdin and stdout are both terminals and `--json` is not given;
  every other call is unchanged. The bare rule lives in `main_with_args`, at the process
  boundary: `app::run` with no command still runs `list`, so in-process callers see no change.
- **The refusals, in order, before the terminal is touched:** `--json` (exit 2, one JSON usage
  error on stdout, `tui has no JSON output; run it without --json`), then stdin or stdout not
  a terminal (exit 2, `tagteam: tagteam tui needs a terminal` on stderr), then an unreadable
  run-shell marker (exit 1, the CLI's own message), then an unknown `--provider` (exit 1, the
  CLI's own message). `app::run` given `tui` by an in-process caller refuses with the first or
  the second, as `--json` says: it has no terminal to hand over.
- **One probe engine on the UI thread**, built as `run_command` builds one and dropped before
  the TUI starts. It checks `--provider`, prints the settings warnings once, before the alternate
  screen hides them, and reads `ui.color`. It reads the settings file and opens no store. The
  lanes' engines (R4) are built by the `SourceFactory` in their own threads, the same way, with
  the shutdown token; their settings warnings are not printed again.
- **Colour** follows the CLI's rule (`app::color_enabled`): `--no-color` and `NO_COLOR` turn it
  off, `FORCE_COLOR` on, and otherwise `ui.color`, with `auto` true (the TUI only runs on a
  terminal).
- **SIGQUIT** is survived for the TUI's whole life (`signals::survive_quit`, spec §7.3),
  installed after the refusals.
- **Logging** (spec §10.3): the binary initializes logging before it knows anything but the
  arguments. For a call that opens the TUI, `LogConfig.stderr` is false: there is no stderr
  layer at all, so `--debug` and ERROR events go to the log file only, and a log file that
  cannot be written is not announced on stderr either. Task 13's
  `debug_lines_go_to_the_log_file_and_never_to_the_terminal` pins this end to end.
- **The report** (`TuiExit.report`) is printed on stderr as it is, with a newline added if it
  lacks one.
- **`ScreenArg`** has `accounts` and `history`; part B adds `tokens`. `tagteam tui tokens` is a
  usage error until then.
- **`Command::Tui` sits after `Status`** in the command list, so `tagteam --help` lists it third,
  among the commands that show accounts. Completion scripts pick it up from the definition
  (spec §12.2); the fish script gains a positional line for its screen, and
  `fish_completes_a_positional_only_in_its_place` names it first.

**Depends on:** 11

**Files:**
- Create: `crates/tagteam/src/tui_cmd.rs`
- Modify: `crates/tagteam/src/cli.rs` (`Command`, lines 46–294: add `Tui` after `Status`,
  line 52; add `ScreenArg` after `ShellArg`, lines 315–320)
- Modify: `crates/tagteam/src/app.rs` (`build_registry` line 248, `build_engine` line 349 and
  `color_enabled` line 563 become `pub(crate)`; `run_command`, lines 658–780, refuses `tui`
  after the statusline fast path; `command_name`, lines 783–810, gains `tui`; `dispatch`,
  lines 1127–1354, gains `tui`'s unreachable arm after `Command::Run`'s, line 1296)
- Modify: `crates/tagteam/src/lib.rs` (the module list: add `mod tui_cmd;` before Task 10's
  `pub mod tui_host;`; `main_with_args`, lines 94–200: the TUI rule, `LogConfig.stderr`, and
  the branch to `tui_cmd::run`; a new `opens_tui`, and its unit tests in `mod tests`)
- Modify: `crates/tagteam/src/logging.rs` (`LogConfig`, lines 31–43: `stderr`; `init`, lines
  47–63; `log_file`, lines 110–123; a new `stderr_layer`, and a unit test)
- Modify: `crates/tagteam/tests/completions.rs` (`fish_completes_a_positional_only_in_its_place`,
  lines 368–450; `each_shell_s_script_offers_every_possible_value_of_every_argument`, lines
  86–116; both as Task 2 left them)
- Modify: `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md` (§13.1, lines
  2434–2435 and the command table, lines 2448–2463; Appendix C, line 3783; as Task 2 left them)
- Modify: `README.md` (Quick start, lines 112–124; Commands, lines 131–153)
- Modify: `CHANGELOG.md` (`[Unreleased]`, line 8)
- Test: `crates/tagteam/tests/tui_cli.rs`

**Interfaces:**
- Consumes:
  - Task 2: `Context: Clone`.
  - Task 10: `tui_host::CliHost::new`.
  - Task 11: `tagteam_tui::{run, TuiOptions, TuiExit}`.
  - Task 5: `tagteam_tui::Screen` (`pub use state::Screen;`).
  - Task 3: `tagteam_tui::source::{EngineSource, Source, SourceFactory}`.
  - Existing: `app::{locate, error_json, KIND_USAGE, EXIT_USAGE, EXIT_ERROR, Context}`,
    `signals::{install, survive_quit}`, `logging::{init, LogConfig}`.
- Produces (C9):
  - `pub enum ScreenArg { Accounts, History }` and `Command::Tui { screen: Option<ScreenArg> }`.
  - `pub(crate) fn tui_cmd::run(cli: &Cli, ctx: Context) -> i32`.
  - `pub(crate) const tui_cmd::TUI_UNDER_JSON`, `pub(crate) const tui_cmd::NEEDS_TERMINAL`.
  - `LogConfig.stderr: bool`.
  - `pub(crate)` on `app::build_registry`, `app::build_engine`, `app::color_enabled`.

**Spec:**
- spec §3: the command and its screen argument; the bare-`tagteam` rule; the refusals before
  the terminal is touched; `--provider`, `--no-color`, `NO_COLOR`, `--debug`; the exit codes.
- spec §10.3: while the TUI owns the terminal, nothing else writes to stdout or stderr.
- spec §12.1: the core spec's §13.1 and Appendix C amendments (Task 2 made §10.4's).
- spec §12.2: completions pick up `tui`, and their snapshot tests are updated.
- core §12.8: an unreadable marker refuses every command but `statusline`.
- core §14.2: the log's levels; `--debug` adds DEBUG on stderr, except in the TUI.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam/tests/tui_cli.rs`:

```rust
//! `tagteam tui`'s refusals and the bare-`tagteam` rule, through the binary and `app::run`
//! (TUI spec §3). The TUI itself, on a terminal, is `tui_pty.rs`'s. Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use clap::Parser;
use common::{cmd, seed_home};
use serde_json::{Value, json};
use tagteam::app::{self, Context, Io};
use tagteam::cli::Cli;
use tagteam::prompt::Prompter;
use tagteam_cc::live::Platform;
use tagteam_provider::{Env, FakeKeychain};

const NEEDS_TERMINAL: &str = "tagteam: tagteam tui needs a terminal\n";

/// §3's JSON refusal.
fn under_json() -> Value {
    json!({"schemaVersion": 1, "error": {"type": "usage",
           "message": "tui has no JSON output; run it without --json"}})
}

/// Nothing was created under HOME (core §5).
fn assert_home_empty(root: &Path) {
    assert!(
        fs::read_dir(root.join("home")).unwrap().next().is_none(),
        "HOME must stay empty"
    );
}

/// A caller that is not a person: any prompt fails the test.
struct Nobody;

impl Prompter for Nobody {
    fn interactive(&self) -> bool {
        false
    }
    fn confirm(&mut self, q: &str, _default_yes: bool) -> bool {
        panic!("unexpected prompt: {q}")
    }
    fn choose(&mut self, q: &str, _options: &[String]) -> Option<usize> {
        panic!("unexpected prompt: {q}")
    }
    fn secret(&mut self, q: &str) -> Option<String> {
        panic!("unexpected prompt: {q}")
    }
}

#[test]
fn tui_under_json_is_one_json_usage_error_and_creates_nothing() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    for args in [&["tui", "--json"][..], &["--json", "tui", "history"]] {
        let out = cmd(d.path())
            .args(args)
            .assert()
            .code(2)
            .stderr("")
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            under_json(),
            "{args:?}"
        );
    }
    assert_home_empty(d.path());
}

#[test]
fn tui_off_a_terminal_is_a_usage_error_and_creates_nothing() {
    // `assert_cmd` gives the binary a null stdin and a piped stdout: neither is a terminal.
    // Under `--debug`, too, stderr holds the refusal and nothing else (§10.3).
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    for args in [&["tui"][..], &["tui", "history"], &["--debug", "tui"]] {
        cmd(d.path())
            .args(args)
            .assert()
            .code(2)
            .stdout("")
            .stderr(NEEDS_TERMINAL);
    }
    assert_home_empty(d.path());
}

#[test]
fn tui_opens_on_accounts_or_history_and_nothing_else_yet() {
    // Part B adds `tokens`.
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    cmd(d.path())
        .args(["tui", "tokens"])
        .assert()
        .code(2)
        .stdout("");
}

#[test]
fn a_bare_tagteam_off_a_terminal_still_runs_list() {
    // §3, §12.1: scripts and pipes see no change.
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    cmd(d.path())
        .assert()
        .success()
        .stdout("No accounts yet. Log in with `claude`, then run `tagteam add`.\n");
    let out = cmd(d.path())
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap()["accounts"],
        json!([])
    );
}

#[test]
fn app_run_refuses_tui_for_an_in_process_caller() {
    // An in-process caller has no terminal to hand over (§3): `tui` is a usage error there.
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    seed_home(&env);
    let run = |args: &[&str]| {
        let cli =
            Cli::try_parse_from(std::iter::once("tagteam").chain(args.iter().copied())).unwrap();
        let ctx = Context {
            env: env.clone(),
            keychain: Arc::new(FakeKeychain::new()),
            vault_keychain: None,
            platform: Platform::MacOs,
            api_base: Some(common::OFFLINE_API_BASE.into()),
            stdout_terminal: false,
            no_color_env: false,
            force_color_env: false,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = app::run(
            cli,
            ctx,
            &mut Io {
                out: &mut out,
                err: &mut err,
                prompter: &mut Nobody,
            },
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    };
    assert_eq!(run(&["tui"]), (2, String::new(), NEEDS_TERMINAL.to_owned()));
    let (code, out, err) = run(&["tui", "--json"]);
    assert_eq!((code, err.as_str()), (2, ""));
    assert_eq!(serde_json::from_str::<Value>(&out).unwrap(), under_json());
    assert!(!env.data_dir().exists());
}
```

In `crates/tagteam/tests/completions.rs`, `each_shell_s_script_offers_every_possible_value_of_every_argument`,
add `"accounts"` to the list of fixed values it checks (after `"consume-first",`):

```rust
        "accounts",
```

and in `fish_completes_a_positional_only_in_its_place`, replace the expected patterns:

```rust
    assert_eq!(
        patterns,
        [
            "tui",
            "shell-init",
            "config get",
            "config set",
            "config unset",
            "completions"
        ]
    );
```

then, after `let shells = BTreeSet::from(["bash", "zsh", "fish"]);`, add:

```rust
    let screens = BTreeSet::from(["accounts", "history"]);
```

and add these rows to the table of lines and what each offers, after
`("tagteam completions", &shells),`:

```rust
        ("tagteam tui", &screens),
        ("tagteam -p claude-code --no-color tui", &screens),
        ("tagteam tui history", &nothing),
```

In `crates/tagteam/src/lib.rs`'s `mod tests`, add:

```rust
    fn parsed(args: &[&str]) -> cli::Cli {
        cli::Cli::try_parse_from(std::iter::once("tagteam").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn a_bare_tagteam_opens_the_tui_only_for_a_person_at_a_terminal() {
        // TUI spec §3.
        assert!(opens_tui(&parsed(&[]), true));
        assert!(!opens_tui(&parsed(&[]), false), "a pipe gets `list`");
        assert!(
            !opens_tui(&parsed(&["--json"]), true),
            "`--json` gets `list`"
        );
        assert!(
            opens_tui(&parsed(&["--no-color", "--provider", "claude-code"]), true),
            "the global flags apply as they would to `tui`"
        );
        assert!(!opens_tui(&parsed(&["list"]), true));
    }

    #[test]
    fn tui_always_opens_the_tui_which_refuses_for_itself() {
        assert!(opens_tui(&parsed(&["tui"]), false));
        assert!(opens_tui(&parsed(&["tui", "--json"]), true));
        assert!(opens_tui(&parsed(&["tui", "history"]), true));
    }
```

In `crates/tagteam/src/logging.rs`'s `mod tests`, add:

```rust
    #[test]
    fn the_tui_s_log_has_no_stderr_layer() {
        // TUI spec §10.3: while the TUI owns the terminal, `--debug` and ERROR go to the file
        // only.
        assert!(stderr_layer::<tracing_subscriber::Registry>(false, true, true).is_none());
        assert!(stderr_layer::<tracing_subscriber::Registry>(true, false, false).is_some());
    }
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --lib`
Expected: FAIL to compile: `cannot find function opens_tui in this scope` and
`cannot find function stderr_layer in this scope`.

Run: `cargo test -p tagteam --features test-support --test tui_cli --test completions`
Expected: FAIL:
- `tui_under_json_is_one_json_usage_error_and_creates_nothing`: the message is clap's
  (`tui` is not a subcommand yet), not `tui has no JSON output; run it without --json`.
- `tui_off_a_terminal_is_a_usage_error_and_creates_nothing`: stderr is clap's
  "unrecognized subcommand" text.
- `app_run_refuses_tui_for_an_in_process_caller`: panics on `Cli::try_parse_from(...).unwrap()`.
- `fish_completes_a_positional_only_in_its_place`: no `tui` pattern.
- `each_shell_s_script_offers_every_possible_value_of_every_argument`: `accounts is a fixed
  value`.

`tui_opens_on_accounts_or_history_and_nothing_else_yet` and
`a_bare_tagteam_off_a_terminal_still_runs_list` already pass (an unknown subcommand is a usage
error too, and a piped bare `tagteam` is `list` today); they must keep passing.

- [ ] **Step 3: Implement**

In `crates/tagteam/src/cli.rs`, add after `Status,` (line 52):

```rust
    /// Open the terminal UI: accounts with their usage, usage history, and the account actions
    ///
    /// A bare `tagteam` opens it too, when stdin and stdout are terminals and --json is not
    /// given. q quits; ? lists the keys.
    Tui {
        /// The screen to open on
        screen: Option<ScreenArg>,
    },
```

and after the `ShellArg` enum (line 320):

```rust
/// The screen `tui` opens on (TUI spec §3). Part B adds `tokens`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ScreenArg {
    /// The accounts table (the default)
    Accounts,
    /// Usage history
    History,
}
```

In `crates/tagteam/src/app.rs`:
- line 248: `fn build_registry(ctx: &Context) -> ProviderRegistry {` becomes
  `pub(crate) fn build_registry(ctx: &Context) -> ProviderRegistry {`;
- line 349: `fn build_engine(` becomes `pub(crate) fn build_engine(`;
- line 563: `fn color_enabled(` becomes `pub(crate) fn color_enabled(`;
- in `run_command`, after the statusline block (it ends at line 673), add:

```rust
    // TUI spec §3: the TUI runs only from the binary, on its terminal (`tui_cmd`). An
    // in-process caller has none to hand over.
    if let Some(Command::Tui { .. }) = &cli.command {
        let message = if json {
            crate::tui_cmd::TUI_UNDER_JSON
        } else {
            crate::tui_cmd::NEEDS_TERMINAL
        };
        fail(io, json, KIND_USAGE, message);
        return Ended::Code(EXIT_USAGE);
    }
```

- in `command_name`, after `Command::Status => "status",`:

```rust
        Command::Tui { .. } => "tui",
```

- in `dispatch`, after `Command::Run { .. } => unreachable!("run_command answers run before dispatch"),`:

```rust
            Command::Tui { .. } => unreachable!("run_command answers tui before dispatch"),
```

In `crates/tagteam/src/logging.rs`, replace `LogConfig` (lines 31–43) with:

```rust
/// What logging needs from the process boundary.
pub(crate) struct LogConfig {
    /// `--debug`.
    pub debug: bool,
    /// Neither `--no-color` nor `NO_COLOR`: stderr's lines may be coloured, on a terminal.
    pub color: bool,
    /// Whether events go to stderr at all: false while the TUI owns the terminal (TUI spec
    /// §10.3), so `--debug` and ERROR events go to the file only.
    pub stderr: bool,
    /// `Env::log_file` (§5, §14.2).
    pub log_file: PathBuf,
    /// `Env::home`, whose paths the file writes as `~/…`.
    pub home: PathBuf,
    /// `TAGTEAM_LOG`, as the environment holds it.
    pub filter: Option<OsString>,
}
```

replace `init` (lines 47–63) with:

```rust
/// Installs the subscriber, and then the panic hook, once per process: with
/// `set_global_default`, never `try_init`, so the `log` crate's records (ureq's, rustls's) are
/// not bridged into it (Decision 5). An invalid `TAGTEAM_LOG` is one warning on `err`.
pub(crate) fn init(cfg: LogConfig, err: &mut dyn Write) {
    let file = file_directive(cfg.debug, cfg.filter.as_deref(), err).map(|directive| {
        tracing_subscriber::fmt::layer()
            .with_writer(log_file(&cfg))
            .with_ansi(false)
            .log_internal_errors(false)
            .event_format(Line::new(&cfg.home))
            .with_filter(EnvFilter::new(directive))
    });
    let stderr = stderr_layer(cfg.stderr, cfg.debug, cfg.color);
    let subscriber = tracing_subscriber::registry().with(stderr).with(file);
    if tracing::subscriber::set_global_default(subscriber).is_ok() {
        install_panic_hook();
    }
}

/// M2's stderr layer, ERROR or with `--debug` DEBUG; none while the TUI owns the terminal,
/// which nothing else may write to (TUI spec §10.3).
fn stderr_layer<S>(on: bool, debug: bool, color: bool) -> Option<impl Layer<S>>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    on.then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(color && std::io::stderr().is_terminal())
            .with_target(false)
            .with_filter(stderr_filter(debug))
    })
}
```

and in `log_file` (lines 110–123), replace `if !cfg.debug {` with:

```rust
    // The notice goes to stderr, which the TUI owns while it runs (TUI spec §10.3).
    if !cfg.debug || !cfg.stderr {
```

In `crates/tagteam/src/lib.rs`:
- add `mod tui_cmd;` to the module list, before `pub mod tui_host;`;
- after the statusline drain (lines 132–135), add:

```rust
    // TUI spec §3: `tui`, and a bare `tagteam` with a person at the terminal, open the TUI. Its
    // log never writes to stderr, where the TUI draws (§10.3).
    let tui = opens_tui(
        &cli,
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
    );
```

- in `init_logging`'s `LogConfig` (lines 157–163), after `color: …,`, add:

```rust
                stderr: !tui,
```

- after the `signals::install` block (it ends at line 185), add:

```rust
    if tui {
        return tui_cmd::run(&cli, ctx);
    }
```

- after `status_bar_line` (it ends at line 68), add:

```rust
/// TUI spec §3: whether this call opens the TUI. `tui` always does, and refuses for itself
/// what it must; a bare `tagteam` does only for a person at a terminal (`terminal`: stdin and
/// stdout are both terminals) and without `--json`, and runs `list` anywhere else, so scripts
/// and pipes see no change.
fn opens_tui(cli: &cli::Cli, terminal: bool) -> bool {
    match cli.command {
        Some(cli::Command::Tui { .. }) => true,
        None => terminal && !cli.json,
        Some(_) => false,
    }
}
```

Create `crates/tagteam/src/tui_cmd.rs`:

```rust
//! `tagteam tui`, and a bare `tagteam` on a terminal (TUI spec §3): the refusals, then the TUI
//! over engines built as `app::run_command` builds one, with the CLI's own command path behind
//! its actions (`tui_host`).

use std::io::{self, IsTerminal, Write};
use std::sync::Arc;

use tagteam_core::ProviderId;
use tagteam_engine::EngineError;
use tagteam_provider::Cancel;
use tagteam_provider::profile::RunShell;
use tagteam_tui::source::{EngineSource, Source, SourceFactory};
use tagteam_tui::{Screen, TuiOptions};

use crate::app::{self, Context};
use crate::cli::{Cli, Command, ScreenArg};
use crate::signals;
use crate::tui_host::CliHost;

/// §3: the TUI has no JSON form.
pub(crate) const TUI_UNDER_JSON: &str = "tui has no JSON output; run it without --json";
/// §3: it draws on a terminal, and reads its keys from one.
pub(crate) const NEEDS_TERMINAL: &str = "tagteam tui needs a terminal";

/// Runs the TUI, and returns the process's exit code (§3, §10.2).
pub(crate) fn run(cli: &Cli, ctx: Context) -> i32 {
    let (mut out, mut err) = (io::stdout(), io::stderr());
    // §3: the refusals come before the terminal is touched.
    if cli.json {
        let _ = writeln!(out, "{}", app::error_json(app::KIND_USAGE, TUI_UNDER_JSON));
        return app::EXIT_USAGE;
    }
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        let _ = writeln!(err, "tagteam: {NEEDS_TERMINAL}");
        return app::EXIT_USAGE;
    }
    // Core §12.8: an unreadable marker refuses, as it does every command but `statusline`.
    let registry = app::build_registry(&ctx);
    let (run_shell, env) = app::locate(ctx.env.clone(), &registry);
    if let RunShell::Unreadable { marker, detail } = &run_shell {
        let e = EngineError::RunShellUnreadable {
            marker: marker.clone(),
            detail: detail.clone(),
        };
        let _ = writeln!(err, "tagteam: {e}");
        return app::EXIT_ERROR;
    }
    let provider = cli.provider.clone().map(ProviderId::new);
    let shutdown = Cancel::new();
    // §2, §4.3: the lanes' engines take the shutdown token, never the signal token.
    let mut inside = ctx.clone();
    inside.env.cancel = shutdown.clone();
    // An engine of the UI thread's own, built as `run_command` builds one and dropped before
    // the TUI starts. It refuses an unknown `--provider`, prints the settings warnings once,
    // before the alternate screen hides them, and reads `ui.color`. It reads the settings file
    // and nothing else.
    let (probe, warnings) =
        app::build_engine(inside.clone(), registry, run_shell, env, provider.as_ref());
    for w in &warnings {
        let _ = writeln!(err, "warning: {w}");
    }
    if let Some(p) = &provider {
        if let Err(e) = probe.provider(p) {
            let _ = writeln!(err, "tagteam: {e}");
            return app::EXIT_ERROR;
        }
    }
    let color = app::color_enabled(
        cli.no_color,
        ctx.no_color_env,
        ctx.force_color_env,
        probe.settings().color,
        true,
    );
    drop(probe);
    // §7.3: Ctrl-\ belongs to a foreground session, and the TUI survives it for its whole life.
    if let Err(e) = signals::survive_quit() {
        let _ = writeln!(
            err,
            "warning: Ctrl-\\ may stop tagteam while a session runs: {e}"
        );
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            let _ = writeln!(
                err,
                "tagteam: cannot find its own executable, which runs sessions: {e}"
            );
            return app::EXIT_ERROR;
        }
    };
    let host = Arc::new(CliHost::new(&ctx, shutdown.clone(), exe));
    let screen = match &cli.command {
        Some(Command::Tui {
            screen: Some(ScreenArg::History),
        }) => Screen::History,
        _ => Screen::Accounts,
    };
    let opts = TuiOptions {
        screen,
        color,
        provider: provider.clone(),
        signal: ctx.env.cancel.clone(),
        shutdown,
    };
    let exit = tagteam_tui::run(opts, sources(inside, provider), host);
    if let Some(report) = &exit.report {
        let _ = write!(err, "{report}");
        if !report.ends_with('\n') {
            let _ = writeln!(err);
        }
    }
    exit.code
}

/// R4: each lane builds its source inside its own thread, from an engine built as
/// `run_command` builds one. `ctx`'s token is the shutdown token (§4.3). The settings warnings
/// were printed once already.
fn sources(ctx: Context, provider: Option<ProviderId>) -> SourceFactory {
    Arc::new(move || {
        let ctx = ctx.clone();
        let registry = app::build_registry(&ctx);
        let (run_shell, env) = app::locate(ctx.env.clone(), &registry);
        let (engine, _warnings) =
            app::build_engine(ctx, registry, run_shell, env, provider.as_ref());
        Box::new(EngineSource::new(engine)) as Box<dyn Source>
    })
}
```

Existing tests: none breaks. Every existing test that runs the binary with no command captures
its stdout (`assert_cmd` and `.output()` pipe it), so a bare `tagteam` there is still `list`;
the tests that use `.status()` with inherited stdio (`map_cli.rs:342`, `run_cli.rs:117`, 366
and 1189) name a command. `tests/app.rs`'s `h.ok(&[])` drives `app::run`, which still runs
`list` for no command. `cli.rs`'s `Command` gains a variant; the exhaustive matches over it
are `command_name` and `dispatch`, both changed above.

Docs. In `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`, §13.1, replace lines
2434–2435:

```markdown
`tagteam [--json] [--debug] [--no-color] [--provider P | -p P] <command>`. A bare `tagteam`
opens the TUI (sub-project 3) when stdin and stdout are terminals and `--json` is not given;
otherwise it runs `list`.
```

and add a row to the command table, after `| \`status\` | The live account |` (line 2451):

```markdown
| `tui [accounts\|tokens\|history]` | The terminal UI ([sub-project 3](2026-10-14-tagteam-tui-design.md)): the accounts, the token report and usage history, with the account actions. A bare `tagteam` on a terminal opens it |
```

In Appendix C, replace line 3783 (`| \`tui\`, \`watch\`, \`menubar\` | — | Sub-projects 3 and 4 |`)
with:

```markdown
| `tui`, `watch` | `tui` | One command for both ([sub-project 3](2026-10-14-tagteam-tui-design.md)) |
| `menubar` | — | Sub-project 4 |
```

In `README.md`, Quick start, after the code block that ends `tagteam switch --strategy best   # switch to the account with the most headroom`
(line 120), add:

```markdown
On a terminal, `tagteam` on its own opens a terminal UI with the same accounts, their usage
history, and keys for these commands (`?` lists them, `q` quits). `tagteam tui history` opens
it on the history.
```

In Commands, replace the last sentence of the paragraph (lines 133–135),
"`run --json` covers only errors before Claude Code starts, and `statusline`, `shell-init`
and `completions` refuse `--json`. A bare `tagteam` runs `list`.", with:

```markdown
`run --json` covers only errors before Claude Code starts, and `statusline`, `shell-init`,
`completions` and `tui` refuse `--json`. A bare `tagteam` opens `tui` on a terminal, and runs
`list` anywhere else (in a pipe, or with `--json`).
```

and add a row after `| \`list\` (\`ls\`), \`status\` | … |` (line 139):

```markdown
| `tui` | A terminal UI: the accounts with their usage and pace, usage history, and keys for switching, enabling, aliasing, moving, removing, adding, running a session and `doctor` |
```

In `CHANGELOG.md`, under `## [Unreleased]` (line 8), add:

```markdown
### Added

- `tagteam tui`, a terminal UI: the accounts table as `tagteam list` shows it, with gauges and
  a detail pane for the selected account, and usage history with a sparkline per window. Its
  keys run tagteam's own commands (switch, enable and disable, alias, move, remove, add and
  doctor), and `R` runs a session as `tagteam run` does. It refreshes every 30 seconds, and
  collects usage only when `list` would. On a terminal, a bare `tagteam` now opens it; in a
  pipe, or with `--json`, it still runs `list`.
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam --lib` and
`cargo test -p tagteam --features test-support --test tui_cli --test completions --test cli`
Expected: PASS.

Run: `cargo test -p tagteam --features test-support`
Expected: PASS (`tests/signals.rs` and `prompt::tests` open a pty: outside the sandbox).

- [ ] **Step 5: Format and lint**

`cargo fmt --all && cargo fmt --all --check`, then
`cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`, then
`cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam/src/tui_cmd.rs crates/tagteam/src/cli.rs crates/tagteam/src/app.rs \
  crates/tagteam/src/lib.rs crates/tagteam/src/logging.rs crates/tagteam/tests/tui_cli.rs \
  crates/tagteam/tests/completions.rs docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md \
  README.md CHANGELOG.md
git commit -m "Open the TUI with tagteam tui, and with a bare tagteam on a terminal"
```

---

### Task 13: End-to-end pty tests and the first-frame timing

The real binary on a pseudo-terminal, as a person runs it (spec §11 "End to end"): what it
shows, how it leaves the terminal, and how it treats signals with and without a foreground
session. Then the first-frame timing (spec §1.1 criterion 2).

The tests read the screen, not the byte stream. ratatui writes only the cells that changed, so
a word already on the screen is never written again, and a test that searched the raw output
would see a screen change only by luck. `tests/common/pty.rs` holds a small terminal emulator
for what crossterm writes (text, cursor moves, clears, the alternate screen; colours and
attributes ignored) and the harness that runs the binary on a pty and reads everything it
writes.

**Readings of the spec this task commits to:**
- **The pty is the program's controlling terminal.** The harness starts the binary in a session
  of its own (`setsid`) and makes the pty its controlling terminal (`TIOCSCTTY`), as a terminal
  emulator starts a shell. So Ctrl-C and Ctrl-\ typed while the TUI is suspended reach its
  foreground process group (the TUI, `tagteam run` and the fake `claude`), as at a real
  terminal. Window size 120×40, set with `TIOCSWINSZ` before the spawn.
- **"The terminal restored"** means the emulator ends on the main screen (the alternate screen
  was left) and the terminal's `c_lflag` has `ICANON` and `ECHO` set again (raw mode is off).
  The modes are read from the slave, or, on macOS, where the leader's exit revokes the slave
  descriptor the harness holds, from the master, whose modes are the slave's there (checked:
  `stty raw -echo` as the leader leaves the master reading both off, `true` both on).
- **A screen** is identified by what only it shows: the accounts table by both fixture emails
  on the alternate screen, History by `No usage history.` (Task 7's empty state for an account
  with no samples; the fixture's accounts have none), the wait for Enter by
  `Press Enter to return to tagteam` on the main screen.
- **The stub launch command** is `tests/common`'s fake `claude` (`fake_claude`, `path_with`),
  first on `PATH`, held running by `FAKE_CLAUDE_HOLD` (a path that never appears), so it runs
  until a signal ends it. `R` runs `tagteam --provider claude-code --expect-account <a> run 1`
  for the first row, `a@x.co`, which is not the live login, so `run` starts a session.
- **The in-flight action** for "SIGTERM exits 143 after an in-flight action has finished" is a
  switch parked inside its critical span by the engine's own test hook
  (`TAGTEAM_TEST_PAUSE_AT=after-journal`, as `tests/signals.rs` parks one). It commits after
  the signal, so the report says it switched (core §14.1).
- **A lane that dies** is made to die by the engine's injected panic
  (`TAGTEAM_TEST_FAIL_AT=panic:after-journal`) inside the switch `s` runs on the actions lane.
  The binary needs no test code of its own for it.
- **Keys under an overlay (R9).** A two-line outcome, such as a switch's, opens the result
  panel, which takes every key but Ctrl-C and Ctrl-Z, `q` included. A test that ends with the
  panel open quits with Ctrl-C (`\x03`, a key in raw mode). A suspended action's outcome opens
  nothing (R12), so after `R` and Enter a `q` quits.
- **Two items of spec §11's end-to-end list wait for parts B and C:** "with an index refresh that
  was running not interrupted and the daemon connection still live" under Ctrl-C. Part A has
  neither an index lane nor a daemon connection; their plans add those assertions to
  `under_r_ctrl_c_ends_the_session_and_the_tui_carries_on`.
- **The first-frame timing** is measured from the spawn to the arrival of the bytes that first
  put the first account's email on the screen, under `list`'s conditions (`perf.rs`'s
  `nothing_due`: 48 hours of readings, nothing due for 180 s, a 400 KB `~/.claude.json`), with
  every endpoint on a local mock server that must see no request. It is release-only and
  ignored by default, like the other timings.

**Depends on:** 12

**Files:**
- Create: `crates/tagteam/tests/common/pty.rs`
- Modify: `crates/tagteam/tests/common/mod.rs` (after the module's `#![allow(dead_code)]`,
  line 4: `pub mod pty;`)
- Create: `crates/tagteam/tests/tui_pty.rs`
- Modify: `crates/tagteam/tests/perf.rs` (the module doc, lines 1–6; the `common` import,
  lines 16–19; a new test after `list_p95_is_within_50_ms_when_nothing_is_due`, line 202)
- Modify: `CONTRIBUTING.md` ("Pty tests need a real terminal environment", lines 59–67)

**Interfaces:**
- Consumes: the binary as Tasks 10–12 leave it; `tests/common/mod.rs`'s `std_cmd`, `cmd`,
  `two_fresh_accounts`, `fake_claude`, `fake_claude_calls`, `FakeCall`, `path_with`,
  `live_email`, `login`, `seed_home`; Task 5–9's screens (the strings above, and the keys `1`,
  `3`, `q`, `s`, `R` and Ctrl-C). `s` and `R` act on the selected row, and the selection
  starts on the first row, `a@x.co` (Task 6's `AccountsState::apply`).
- Produces (shared test helpers, `crates/tagteam/tests/common/pty.rs`):
  - `pub const COLS: u16 = 120;`, `pub const ROWS: u16 = 40;`
  - `pub struct Pty { pub child: Child, pub started: Instant, … }` with
    `pub fn spawn(cmd: Command, stdin_is_terminal: bool) -> Pty`,
    `pub fn type_keys(&self, keys: &[u8])`, `pub fn signal(&self, signal: i32)`,
    `pub fn screen(&self) -> Screen`, `pub fn output(&self) -> Vec<u8>`,
    `pub fn wait_for(&mut self, within: Duration, what: &str, ready: impl Fn(&Screen) -> bool) -> Screen`,
    `pub fn wait_until(&mut self, within: Duration, what: &str, ready: impl Fn() -> bool)`,
    `pub fn first_shown(&self, ready: impl Fn(&Screen) -> bool) -> Option<Instant>`,
    `pub fn finish(self, within: Duration) -> Finished`.
  - `pub struct Finished { pub status: ExitStatus, pub screen: Screen, pub output: Vec<u8>, pub lflag: libc::tcflag_t }`.
  - `pub struct Screen` with `pub fn of(bytes: &[u8], cols: u16, rows: u16) -> Screen`,
    `pub fn text(&self) -> String`, `pub fn contains(&self, text: &str) -> bool`,
    `pub fn on_alt(&self) -> bool`.

**Spec:**
- spec §11 "End to end": the first frame; the screen keys; `q` exits 0 with the terminal
  restored; SIGTERM exits 143 with the terminal restored, after an in-flight action has
  finished; suspended under `R`: SIGTERM forwarded and both exit, Ctrl-\ ends the stub and the
  TUI survives and returns, Ctrl-C ends the stub and the TUI carries on; a bare `tagteam` in a
  pty opens the TUI; `--json` and a non-terminal stdin are usage errors.
- spec §11 "Host bridge": `--expect-account` refusing a moved account for `run` (Task 10 covers
  switch, remove and disable in-process).
- spec §11 "Performance": the first frame within 50 ms p95 under `list`'s conditions, through
  a pty to the first byte of the accounts table.
- spec §1.1 criteria 2 and 7: start-up as fast as `list`; the terminal always restored, on
  quit, on a signal, on an error and on a panic.
- spec §7.3: a keychain that needs unlocking runs the action on the terminal.
- spec §10.3: `--debug` writes to the log file only.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam/tests/common/mod.rs`, after line 4 (`#![allow(dead_code)]`), add a blank
line and:

```rust
pub mod pty;
```

Create `crates/tagteam/tests/common/pty.rs`:

```rust
//! A program on a pseudo-terminal, and a small terminal emulator to read back what it shows
//! (TUI spec §11 "End to end"). `tui_pty.rs` and `perf.rs` run the TUI through it. `openpty`
//! fails with EPERM inside a sandbox that denies it, so the tests that use it run outside one.

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use unicode_width::UnicodeWidthChar;

/// The terminal every test opens: 120 columns by 40 rows (TUI spec §11).
pub const COLS: u16 = 120;
pub const ROWS: u16 = 40;

/// The cell a wide character's right half takes.
const WIDE: char = '\0';

/// A program on a pseudo-terminal of `COLS`×`ROWS`, started as a terminal emulator starts a
/// shell: in a session of its own whose controlling terminal the pty is, so the terminal's
/// Ctrl-C and Ctrl-\ reach its foreground process group. A thread reads everything it writes.
pub struct Pty {
    pub child: Child,
    /// When it was spawned.
    pub started: Instant,
    master: File,
    slave: OwnedFd,
    written: Arc<Mutex<Written>>,
}

/// What the program wrote, and when each chunk of it was read.
#[derive(Default)]
struct Written {
    bytes: Vec<u8>,
    /// When each chunk arrived, and the length of `bytes` with it.
    chunks: Vec<(Instant, usize)>,
}

/// How a program on a pty ended.
pub struct Finished {
    pub status: ExitStatus,
    /// What the terminal shows once it has exited.
    pub screen: Screen,
    /// Everything it wrote.
    pub output: Vec<u8>,
    /// The terminal's local modes as it left them (`c_lflag`).
    pub lflag: libc::tcflag_t,
}

impl Pty {
    /// Starts `cmd` with the pty as its stdout and stderr, and as its stdin when
    /// `stdin_is_terminal` (else stdin is null).
    pub fn spawn(mut cmd: Command, stdin_is_terminal: bool) -> Pty {
        let (master, slave) = open_pty();
        let stdin = if stdin_is_terminal {
            Stdio::from(slave.try_clone().unwrap())
        } else {
            Stdio::null()
        };
        cmd.env("TERM", "xterm-256color")
            .stdin(stdin)
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave.try_clone().unwrap()));
        // SAFETY: the closure runs in the child between fork and exec, and calls only setsid(2)
        // and ioctl(2), which are async-signal-safe. Its stdout is the pty's slave by then.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(1, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let started = Instant::now();
        let child = cmd.spawn().unwrap();
        let master = File::from(master);
        let written = Arc::new(Mutex::new(Written::default()));
        let (mut from, into) = (master.try_clone().unwrap(), Arc::clone(&written));
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match from.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut w = into.lock().unwrap();
                        w.bytes.extend_from_slice(&buf[..n]);
                        let end = w.bytes.len();
                        w.chunks.push((Instant::now(), end));
                    }
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    // EIO once every slave descriptor is closed (Linux).
                    Err(_) => break,
                }
            }
        });
        Pty {
            child,
            started,
            master,
            slave,
            written,
        }
    }

    /// Types `keys` at the terminal.
    pub fn type_keys(&self, keys: &[u8]) {
        (&self.master).write_all(keys).unwrap();
    }

    /// Sends `signal` to the program alone, as `kill` does.
    pub fn signal(&self, signal: i32) {
        // SAFETY: kill(2) reads no memory of ours. The child is not reaped yet, so its pid
        // names it and no other process.
        let rc = unsafe { libc::kill(self.child.id() as libc::pid_t, signal) };
        assert_eq!(rc, 0, "kill: {}", std::io::Error::last_os_error());
    }

    /// What the terminal shows now.
    pub fn screen(&self) -> Screen {
        Screen::of(&self.written.lock().unwrap().bytes, COLS, ROWS)
    }

    /// Everything the program has written so far.
    pub fn output(&self) -> Vec<u8> {
        self.written.lock().unwrap().bytes.clone()
    }

    /// Polls every 10 ms until `ready` holds of what the terminal shows, and returns that.
    /// Fails the test if the program exits first, or if `within` passes.
    pub fn wait_for(
        &mut self,
        within: Duration,
        what: &str,
        ready: impl Fn(&Screen) -> bool,
    ) -> Screen {
        let deadline = Instant::now() + within;
        loop {
            let screen = self.screen();
            if ready(&screen) {
                return screen;
            }
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!("it exited ({status}) before {what}:\n{}", screen.text());
            }
            assert!(
                Instant::now() < deadline,
                "it never got to {what}:\n{}",
                screen.text()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Polls every 10 ms until `ready` holds. Fails the test if the program exits first, or
    /// if `within` passes.
    pub fn wait_until(&mut self, within: Duration, what: &str, ready: impl Fn() -> bool) {
        let deadline = Instant::now() + within;
        while !ready() {
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!(
                    "it exited ({status}) before {what}:\n{}",
                    self.screen().text()
                );
            }
            assert!(
                Instant::now() < deadline,
                "it never got to {what}:\n{}",
                self.screen().text()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// When what `ready` holds of was first on the terminal: the arrival of the chunk that put
    /// it there.
    pub fn first_shown(&self, ready: impl Fn(&Screen) -> bool) -> Option<Instant> {
        let written = self.written.lock().unwrap();
        written
            .chunks
            .iter()
            .find(|(_, end)| ready(&Screen::of(&written.bytes[..*end], COLS, ROWS)))
            .map(|(at, _)| *at)
    }

    /// The terminal's local modes (`c_lflag`) now. Once the program, the session's leader, has
    /// exited, macOS revokes the slave descriptor this harness holds, and its master reads the
    /// modes the two ends share; Linux keeps the slave usable, and its master has modes of its
    /// own. So the slave is asked first, then the master.
    fn local_modes(&self) -> libc::tcflag_t {
        for fd in [self.slave.as_raw_fd(), self.master.as_raw_fd()] {
            // SAFETY: an all-zero termios is a valid value for tcgetattr(3) to overwrite.
            let mut modes: libc::termios = unsafe { std::mem::zeroed() };
            // SAFETY: `fd` is open, and `modes` is one writable termios.
            if unsafe { libc::tcgetattr(fd, &mut modes) } == 0 {
                return modes.c_lflag;
            }
        }
        panic!("tcgetattr: {}", std::io::Error::last_os_error())
    }

    /// Waits for the program to exit, then for the last of its output, and says how it ended.
    /// Fails the test if it still runs after `within`.
    pub fn finish(mut self, within: Duration) -> Finished {
        let deadline = Instant::now() + within;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "it was still running {within:?} later:\n{}",
                self.screen().text()
            );
            thread::sleep(Duration::from_millis(10));
        };
        // Its last bytes may still be in the pty: read until nothing more comes for 50 ms.
        let mut seen = usize::MAX;
        for _ in 0..40 {
            let now = self.written.lock().unwrap().bytes.len();
            if now == seen {
                break;
            }
            seen = now;
            thread::sleep(Duration::from_millis(50));
        }
        Finished {
            status,
            screen: self.screen(),
            output: self.output(),
            lflag: self.local_modes(),
        }
    }
}

impl Drop for Pty {
    /// A test that failed part way leaves no program running.
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// A pseudo-terminal pair of `COLS`×`ROWS`, neither end inherited by children.
fn open_pty() -> (OwnedFd, OwnedFd) {
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty(3) writes two descriptors into the integers passed; the name, termios
    // and winsize arguments may be null.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(rc, 0, "openpty: {}", std::io::Error::last_os_error());
    // SAFETY: openpty succeeded, so both are open descriptors that nothing else owns.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    for fd in [&master, &slave] {
        // SAFETY: fcntl(2) with F_SETFD takes an open descriptor and an integer.
        let rc = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        assert_eq!(rc, 0, "fcntl: {}", std::io::Error::last_os_error());
    }
    let size = libc::winsize {
        ws_row: ROWS,
        ws_col: COLS,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: `size` is one valid winsize, which ioctl(2) reads for the duration of the call.
    let rc = unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ as _, &size) };
    assert_eq!(rc, 0, "TIOCSWINSZ: {}", std::io::Error::last_os_error());
    (master, slave)
}

/// A terminal's two screens as a small emulator reads what a program wrote: text, cursor moves,
/// clears and the alternate screen. Colours and attributes are ignored, and so is every
/// sequence crossterm and ratatui do not send. Enough to read back what they draw.
#[derive(Clone)]
pub struct Screen {
    cols: usize,
    rows: usize,
    main: Vec<Vec<char>>,
    alt: Vec<Vec<char>>,
    on_alt: bool,
    row: usize,
    col: usize,
    saved: (usize, usize),
}

impl Screen {
    /// The screen after `bytes`, on a terminal of `cols`×`rows`.
    pub fn of(bytes: &[u8], cols: u16, rows: u16) -> Screen {
        let (cols, rows) = (usize::from(cols), usize::from(rows));
        let mut screen = Screen {
            cols,
            rows,
            main: blank(cols, rows),
            alt: blank(cols, rows),
            on_alt: false,
            row: 0,
            col: 0,
            saved: (0, 0),
        };
        screen.feed(&String::from_utf8_lossy(bytes));
        screen
    }

    /// The screen shown now, a line per row, without trailing blanks.
    pub fn text(&self) -> String {
        let grid = if self.on_alt { &self.alt } else { &self.main };
        grid.iter()
            .map(|row| {
                let line: String = row.iter().filter(|&&c| c != WIDE).collect();
                line.trim_end().to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn contains(&self, text: &str) -> bool {
        self.text().contains(text)
    }

    /// Whether the alternate screen is the one shown.
    pub fn on_alt(&self) -> bool {
        self.on_alt
    }

    fn grid(&mut self) -> &mut Vec<Vec<char>> {
        if self.on_alt {
            &mut self.alt
        } else {
            &mut self.main
        }
    }

    fn feed(&mut self, text: &str) {
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            match c {
                '\x1b' => match chars.next() {
                    Some('[') => {
                        let mut params = String::new();
                        for c in chars.by_ref() {
                            if ('\x40'..='\x7e').contains(&c) {
                                self.csi(&params, c);
                                break;
                            }
                            params.push(c);
                        }
                    }
                    Some('7') => self.saved = (self.row, self.col),
                    Some('8') => (self.row, self.col) = self.saved,
                    // A character set (`ESC ( B`): its one argument goes too.
                    Some('(' | ')') => {
                        chars.next();
                    }
                    _ => {}
                },
                '\r' => self.col = 0,
                '\n' => self.line_feed(),
                '\x08' => self.col = self.col.saturating_sub(1),
                c if c.is_control() => {}
                c => self.put(c),
            }
        }
    }

    fn csi(&mut self, params: &str, last: char) {
        let private = params.starts_with('?');
        let numbers: Vec<usize> = params
            .trim_start_matches('?')
            .split(';')
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        // Parameter `i`, or `default` for one that is missing or zero.
        let at = |i: usize, default: usize| {
            numbers
                .get(i)
                .copied()
                .filter(|&n| n > 0)
                .unwrap_or(default)
        };
        let mode = numbers.first().copied().unwrap_or(0);
        let (last_row, last_col) = (self.rows - 1, self.cols - 1);
        match (private, last) {
            (true, 'h') if numbers.contains(&1049) => {
                if !self.on_alt {
                    self.saved = (self.row, self.col);
                    self.alt = blank(self.cols, self.rows);
                    self.on_alt = true;
                }
            }
            (true, 'l') if numbers.contains(&1049) => {
                if self.on_alt {
                    self.on_alt = false;
                    (self.row, self.col) = self.saved;
                }
            }
            (false, 'H' | 'f') => {
                self.row = (at(0, 1) - 1).min(last_row);
                self.col = (at(1, 1) - 1).min(last_col);
            }
            (false, 'A') => self.row = self.row.saturating_sub(at(0, 1)),
            (false, 'B') => self.row = (self.row + at(0, 1)).min(last_row),
            (false, 'C') => self.col = (self.col + at(0, 1)).min(last_col),
            (false, 'D') => self.col = self.col.saturating_sub(at(0, 1)),
            (false, 'G') => self.col = (at(0, 1) - 1).min(last_col),
            (false, 'd') => self.row = (at(0, 1) - 1).min(last_row),
            (false, 'J') => {
                let cursor = (self.row, self.col);
                for (r, line) in self.grid().iter_mut().enumerate() {
                    for (c, cell) in line.iter_mut().enumerate() {
                        let erased = match mode {
                            0 => (r, c) >= cursor,
                            1 => (r, c) <= cursor,
                            _ => true,
                        };
                        if erased {
                            *cell = ' ';
                        }
                    }
                }
            }
            (false, 'K') => {
                let (row, col) = (self.row, self.col);
                for (c, cell) in self.grid()[row].iter_mut().enumerate() {
                    let erased = match mode {
                        0 => c >= col,
                        1 => c <= col,
                        _ => true,
                    };
                    if erased {
                        *cell = ' ';
                    }
                }
            }
            // Colours, attributes, the cursor's visibility and the rest change no text.
            _ => {}
        }
    }

    fn put(&mut self, c: char) {
        let width = c.width().unwrap_or(0);
        if width == 0 {
            return;
        }
        // A character past the last column wraps, as a terminal's autowrap does.
        if self.col + width > self.cols {
            self.col = 0;
            self.line_feed();
        }
        let (row, col) = (self.row, self.col);
        let line = &mut self.grid()[row];
        line[col] = c;
        if width == 2 {
            line[col + 1] = WIDE;
        }
        self.col += width;
    }

    fn line_feed(&mut self) {
        if self.row + 1 < self.rows {
            self.row += 1;
            return;
        }
        let cols = self.cols;
        let grid = self.grid();
        grid.remove(0);
        grid.push(vec![' '; cols]);
    }
}

fn blank(cols: usize, rows: usize) -> Vec<Vec<char>> {
    vec![vec![' '; cols]; rows]
}
```

Create `crates/tagteam/tests/tui_pty.rs`:

```rust
//! The TUI through the real binary on a pseudo-terminal (TUI spec §11 "End to end"): what it
//! shows, how it leaves the terminal, and how it treats signals with and without a foreground
//! session. `openpty` fails inside a sandbox that denies it, so run these outside one:
//! `cargo test -p tagteam --features test-support --test tui_pty`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use common::pty::{Finished, Pty, Screen};
use common::{
    FakeCall, cmd, fake_claude, fake_claude_calls, live_email, login, path_with, seed_home,
    std_cmd, two_fresh_accounts,
};
use tagteam_core::AccountId;
use tagteam_provider::{Env, FileKeychain, MARKER_FILE, profile_path};

/// How long one step may take before it counts as stuck.
const LONG: Duration = Duration::from_secs(20);
/// R5's line, as the person reads it.
const RETURN: &str = "Press Enter to return to tagteam";

/// `a@x.co` at position 1 and `b@x.co` at 2, the live login (`two_fresh_accounts`), the fake
/// `claude` in `<root>/bin`, and a working directory `<root>/work`. The first row is `a`,
/// whose `run` starts a session.
struct Home {
    dir: tempfile::TempDir,
    a: String,
}

impl Home {
    fn new() -> Home {
        let dir = tempfile::tempdir().unwrap();
        let (a, _b) = two_fresh_accounts(dir.path());
        fake_claude(dir.path());
        fs::create_dir_all(dir.path().join("work")).unwrap();
        Home { dir, a }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// The fake `claude`'s record of its runs.
    fn calls(&self) -> PathBuf {
        self.root().join("calls")
    }

    /// `tagteam <args>` as a person starts it: in `<root>/work`, with the fake `claude` first
    /// on `PATH`. It runs until a signal ends it (`FAKE_CLAUDE_HOLD` names a file that never
    /// appears).
    fn command(&self, args: &[&str]) -> Command {
        let mut c = std_cmd(self.root());
        c.args(args)
            .env("PATH", path_with(&self.root().join("bin")))
            .env("FAKE_CLAUDE_OUT", self.calls())
            .env("FAKE_CLAUDE_HOLD", self.root().join("never"))
            .current_dir(self.root().join("work"));
        c
    }

    /// `command(args)` on a terminal.
    fn tui(&self, args: &[&str]) -> Pty {
        Pty::spawn(self.command(args), true)
    }

    /// The session the fake `claude` ran, if one started.
    fn session(&self) -> Option<FakeCall> {
        fake_claude_calls(&self.calls())
            .into_iter()
            .find(|c| c.mode == "session")
    }
}

/// The accounts table, on the alternate screen.
fn accounts(s: &Screen) -> bool {
    s.on_alt() && s.contains("a@x.co") && s.contains("b@x.co")
}

/// The terminal handed back, and the wait for Enter on it (R5).
fn returning(s: &Screen) -> bool {
    !s.on_alt() && s.contains(RETURN)
}

/// Whether the program ever took the alternate screen.
fn entered_alt(output: &[u8]) -> bool {
    output.windows(8).any(|w| w == b"\x1b[?1049h")
}

/// The terminal as the TUI found it: the main screen, canonical mode and echo on.
fn assert_restored(f: &Finished) {
    assert!(
        !f.screen.on_alt(),
        "the TUI never left the alternate screen:\n{}",
        f.screen.text()
    );
    assert_eq!(
        f.lflag & (libc::ICANON | libc::ECHO),
        libc::ICANON | libc::ECHO,
        "raw mode is off"
    );
}

/// Waits for the session `R` started to be running.
fn wait_for_session(home: &Home, tui: &mut Pty) {
    tui.wait_until(LONG, "the session", || {
        home.session().is_some_and(|c| c.ready)
    });
}

#[test]
fn the_emulator_reads_back_text_moves_clears_and_the_alternate_screen() {
    let drawn = b"shell$ \x1b[?1049h\x1b[2J\x1b[1;1Htagteam\x1b[3;5H\x1b[1mbold\x1b[0m\x1b[2;1Hgone\x1b[2;1H\x1b[K";
    let s = Screen::of(drawn, 20, 4);
    assert!(s.on_alt());
    assert_eq!(s.text(), "tagteam\n\n    bold\n");
    let s = Screen::of(&[&drawn[..], b"\x1b[?1049l\r\nback"].concat(), 20, 4);
    assert!(!s.on_alt());
    assert_eq!(s.text(), "shell$\nback\n\n");
    assert_eq!(Screen::of("a界b".as_bytes(), 6, 1).text(), "a界b");
}

#[test]
fn the_first_frame_shows_the_accounts_and_q_leaves_the_terminal_as_it_was() {
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_restored(&f);
    assert_eq!(f.screen.text().trim(), "", "no report: nothing was running");
}

#[test]
fn three_shows_history_and_one_the_accounts_again() {
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"3");
    tui.wait_for(LONG, "History", |s| {
        s.on_alt() && s.contains("No usage history.")
    });
    tui.type_keys(b"1");
    tui.wait_for(LONG, "the accounts table again", |s| {
        accounts(s) && !s.contains("No usage history.")
    });
    tui.type_keys(b"q");
    assert_eq!(tui.finish(LONG).status.code(), Some(0));
}

#[test]
fn tui_history_opens_on_history() {
    let home = Home::new();
    let mut tui = home.tui(&["tui", "history"]);
    tui.wait_for(LONG, "History", |s| {
        s.on_alt() && s.contains("No usage history.")
    });
    tui.type_keys(b"q");
    assert_eq!(tui.finish(LONG).status.code(), Some(0));
}

#[test]
fn sigterm_exits_143_with_the_terminal_restored() {
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.signal(libc::SIGTERM);
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(143), "{}", f.screen.text());
    assert_restored(&f);
}

#[test]
fn sigterm_waits_for_a_switch_inside_its_critical_span_and_reports_it_as_switched() {
    // Spec §10.2: the switch is parked right after its journal row (§9.4 step 6), inside its
    // critical span, when SIGTERM arrives. The TUI restores the terminal, waits for it, and
    // reports it in the CLI's words (core §14.1).
    let home = Home::new();
    let pause = home.root().join("pause");
    fs::create_dir(&pause).unwrap();
    let mut c = home.command(&["tui"]);
    c.env("TAGTEAM_TEST_PAUSE_AT", "after-journal")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause);
    let mut tui = Pty::spawn(c, true);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"s");
    tui.wait_until(LONG, "the switch's journal row", || {
        pause.join("paused").exists()
    });
    tui.signal(libc::SIGTERM);
    thread::sleep(Duration::from_millis(500));
    assert!(
        tui.child.try_wait().unwrap().is_none(),
        "the TUI waits for the switch"
    );
    fs::write(pause.join("resume"), b"").unwrap();
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(143), "{}", f.screen.text());
    assert_restored(&f);
    let shown = f.screen.text();
    assert!(
        shown.contains("Switched to a@x.co (position 1)."),
        "{shown}"
    );
    assert!(
        shown.contains("tagteam: interrupted too late to stop: switch had already finished"),
        "{shown}"
    );
    assert_eq!(live_email(home.root()), "a@x.co");
}

#[test]
fn a_bare_tagteam_on_a_terminal_opens_the_tui_and_with_json_runs_list() {
    let home = Home::new();
    let mut tui = home.tui(&[]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0));
    assert_restored(&f);
    // §3: `--json` is `list --json`, on a terminal too.
    let f = home.tui(&["--json"]).finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert!(!entered_alt(&f.output));
    let printed = String::from_utf8_lossy(&f.output);
    assert!(
        printed.contains("\"schemaVersion\":1") && printed.contains("a@x.co"),
        "{printed}"
    );
}

#[test]
fn a_stdin_that_is_not_a_terminal_is_refused_before_the_terminal_is_touched() {
    let home = Home::new();
    let f = Pty::spawn(home.command(&["tui"]), false).finish(LONG);
    assert_eq!(f.status.code(), Some(2));
    assert!(
        f.screen.contains("tagteam: tagteam tui needs a terminal"),
        "{}",
        f.screen.text()
    );
    assert!(!entered_alt(&f.output));
}

#[test]
fn an_unreadable_marker_is_refused_before_the_terminal_is_touched() {
    // Core §12.8: as for every command but `statusline`.
    let home = Home::new();
    let dir = profile_path(
        &Env::for_test(home.root()),
        &AccountId::from_string(&home.a),
    );
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(MARKER_FILE), "{\"format\": \"tagteam-profile\"").unwrap();
    let mut c = home.command(&["tui"]);
    c.env("CLAUDE_CONFIG_DIR", &dir);
    let f = Pty::spawn(c, true).finish(LONG);
    assert_eq!(f.status.code(), Some(1));
    let marker = dir.join(MARKER_FILE).display().to_string();
    assert!(
        String::from_utf8_lossy(&f.output).contains(&marker),
        "{}",
        f.screen.text()
    );
    assert!(!entered_alt(&f.output));
}

#[test]
fn under_r_a_sigterm_to_the_tui_is_forwarded_and_both_exit() {
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"R");
    wait_for_session(&home, &mut tui);
    tui.signal(libc::SIGTERM);
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(143), "{}", f.screen.text());
    assert_restored(&f);
    let session = home.session().unwrap();
    assert_eq!(
        (session.signals, session.exit),
        (vec![15], Some(143)),
        "forwarded once, through `tagteam run`"
    );
    assert!(
        !String::from_utf8_lossy(&f.output).contains(RETURN),
        "a SIGTERM never waits for Enter"
    );
}

#[test]
fn under_r_ctrl_backslash_ends_the_session_and_the_tui_survives_and_returns() {
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"R");
    wait_for_session(&home, &mut tui);
    tui.type_keys(b"\x1c");
    tui.wait_for(LONG, "the wait for Enter", returning);
    assert_eq!(
        home.session().unwrap().exit,
        None,
        "SIGQUIT killed the session before it could record an exit"
    );
    tui.type_keys(b"\r");
    tui.wait_for(LONG, "the accounts table again", accounts);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_restored(&f);
}

#[test]
fn under_r_ctrl_c_ends_the_session_and_the_tui_carries_on() {
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"R");
    wait_for_session(&home, &mut tui);
    tui.type_keys(b"\x03");
    tui.wait_for(LONG, "the wait for Enter", returning);
    let session = home.session().unwrap();
    assert_eq!(
        (session.signals, session.exit),
        (vec![2], Some(130)),
        "the terminal's Ctrl-C, once"
    );
    tui.type_keys(b"\r");
    tui.wait_for(LONG, "the accounts table again", accounts);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_restored(&f);
}

#[test]
fn run_on_an_account_moved_elsewhere_is_refused_and_launches_nothing() {
    // Spec §4.5, §11: another process moves `a` after the TUI read it, so position 1 holds `b`.
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    // Whatever the TUI collects and re-reads as it starts is done by now (§6.2).
    thread::sleep(Duration::from_secs(1));
    cmd(home.root()).args(["move", "1", "2"]).assert().success();
    tui.type_keys(b"R");
    let shown = tui.wait_for(LONG, "the wait for Enter", returning);
    assert!(
        shown.contains("tagteam: the selected account changed; nothing was done"),
        "{}",
        shown.text()
    );
    assert!(
        fake_claude_calls(&home.calls()).is_empty(),
        "nothing was launched, not even a login check"
    );
    tui.type_keys(b"\r");
    tui.wait_for(LONG, "the accounts table again", accounts);
    tui.type_keys(b"q");
    assert_eq!(tui.finish(LONG).status.code(), Some(0));
}

#[test]
fn a_locked_keychain_runs_the_action_on_the_terminal_and_returns() {
    // Spec §4.5, §7.3: the TUI's dialogs cannot unlock it, so the switch runs again on the
    // terminal, where the CLI asks itself. The file-backed fake Keychain cannot be unlocked.
    let home = Home::new();
    fs::write(home.root().join("keychain").join("LOCKED"), b"").unwrap();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"s");
    tui.wait_for(LONG, "the CLI's unlock question", |s| {
        !s.on_alt() && s.contains("Unlock it now? [Y/n]")
    });
    tui.type_keys(b"y\r");
    let shown = tui.wait_for(LONG, "the wait for Enter", returning);
    assert!(
        shown.contains("tagteam: the login keychain is locked (common over SSH)"),
        "{}",
        shown.text()
    );
    tui.type_keys(b"\r");
    tui.wait_for(LONG, "the accounts table again", accounts);
    tui.type_keys(b"q");
    assert_eq!(tui.finish(LONG).status.code(), Some(0));
    assert_eq!(live_email(home.root()), "b@x.co", "nothing was switched");
}

/// `tests/logging.rs`'s fixture: `a@x.co` at position 1 and `b@x.co` at 2, `b` live, and then
/// Claude Code rotated b's credential in place. With every endpoint offline, `switch 1` logs
/// the oracle's missing answer at DEBUG.
fn rotated_live_login(root: &Path) {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    cmd(root)
        .env("TAGTEAM_LOG", "off")
        .arg("add")
        .assert()
        .success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    cmd(root)
        .env("TAGTEAM_LOG", "off")
        .arg("add")
        .assert()
        .success();
    login(&env, &kc, "b@x.co", "", "rt-b-rotated");
}

#[test]
fn debug_lines_go_to_the_log_file_and_never_to_the_terminal() {
    // Spec §3, §10.3.
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    rotated_live_login(root);
    let mut c = std_cmd(root);
    c.args(["--debug", "tui"]);
    let mut tui = Pty::spawn(c, true);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"s");
    tui.wait_for(LONG, "the switch's outcome", |s| {
        s.contains("Switched to a@x.co")
    });
    // Two lines open the result panel, which takes `q` (R9); Ctrl-C quits under any overlay.
    tui.type_keys(b"\x03");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    let log = fs::read_to_string(Env::for_test(root).state_dir().join("tagteam.log")).unwrap();
    assert!(log.contains("the profile oracle gave no answer"), "{log}");
    let printed = String::from_utf8_lossy(&f.output);
    assert!(
        !printed.contains("the profile oracle gave no answer") && !printed.contains("DEBUG"),
        "{printed}"
    );
}

#[test]
fn a_lane_that_panics_restores_the_terminal_before_the_panic_shows_and_exits_1() {
    // Spec §10.1, §10.4: the switch panics inside its transaction (the engine's injected
    // panic), so the actions lane dies.
    let home = Home::new();
    let mut c = home.command(&["tui"]);
    c.env("TAGTEAM_TEST_FAIL_AT", "panic:after-journal");
    let mut tui = Pty::spawn(c, true);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"s");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(1), "{}", f.screen.text());
    assert_restored(&f);
    let printed = String::from_utf8_lossy(&f.output);
    let left = printed
        .rfind("\x1b[?1049l")
        .expect("the alternate screen was left");
    let panicked = printed.find("panicked at").expect("the panic is shown");
    assert!(left < panicked, "restored before the panic was printed");
    assert!(
        f.screen
            .contains("tagteam: the TUI's actions lane panicked; the log has the details"),
        "{}",
        f.screen.text()
    );
}
```

In `crates/tagteam/tests/perf.rs`, replace the module doc (lines 1–6) with:

```rust
//! §1.1's timing targets through the real binary: `statusline` within 10 ms p95, `list` within
//! 50 ms p95 when no usage fetch is due, and the TUI's first frame within 50 ms p95 under the
//! same conditions (TUI spec §1.1). Timings mean something only in an optimized build on an
//! idle machine, so this file exists only in release builds and its tests are ignored by
//! default. The TUI's opens a pseudo-terminal, so run them outside a sandbox:
//! `cargo test --release -p tagteam --features test-support --test perf -- --ignored`
//! (add `--nocapture` to see each measured p95).
```

replace the `common` import (lines 16–19) with:

```rust
use common::pty::{Pty, Screen};
use common::{
    bloat_claude_json, cc_profile, hold_launch, now_epoch_s, record_history, std_cmd,
    two_fresh_accounts, usage_window,
};
```

and add after `list_p95_is_within_50_ms_when_nothing_is_due` (it ends at line 202):

```rust
#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn tui_first_frame_p95_is_within_50_ms_when_nothing_is_due() {
    // TUI spec §1.1 criterion 2, §11: from the spawn to the arrival of the bytes that first put
    // the accounts table on the screen, through a pty, under `list`'s conditions (core §1.1).
    // The first frame never waits on a fetch (§6.1), and nothing is due, so the collection the
    // TUI starts after it sends nothing either.
    let _one_at_a_time = serial();
    let d = tempfile::tempdir().unwrap();
    nothing_due(d.path());
    let server = MockServer::start();
    let shown = |s: &Screen| s.on_alt() && s.contains("a@x.co");
    let run = || {
        let mut c = std_cmd(d.path());
        c.env("TAGTEAM_TEST_API_BASE", server.base_url()).arg("tui");
        let mut tui = Pty::spawn(c, true);
        tui.wait_for(Duration::from_secs(10), "the accounts table", shown);
        let took = tui.first_shown(shown).unwrap() - tui.started;
        tui.type_keys(b"q");
        let f = tui.finish(Duration::from_secs(10));
        assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
        took
    };
    for _ in 0..WARM_UP {
        run();
    }
    let runs: Vec<Duration> = (0..RUNS).map(|_| run()).collect();
    let p = p95(&runs);
    eprintln!("tui first frame p95 {p:?} over {RUNS} runs");
    assert_eq!(
        server.requests().len(),
        0,
        "a fetch was due, so this measured the wrong path"
    );
    assert!(
        p <= Duration::from_millis(50),
        "tui first frame p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}
```

In `CONTRIBUTING.md`, "Pty tests need a real terminal environment" (lines 59–67), replace the
first sentence and the command block with:

````markdown
- **Pty tests need a real terminal environment.** `tagteam`'s `prompt::tests`,
  `tests/signals.rs`, `tests/tui_pty.rs` and the TUI's first-frame timing in `tests/perf.rs`
  open a pseudo-terminal. They fail with `openpty: Operation not permitted`
  inside a sandbox that denies it, such as an agent or container sandbox. Run them from an
  ordinary, unsandboxed terminal:

  ```sh
  cargo test -p tagteam --features tagteam/test-support --lib prompt::tests
  cargo test -p tagteam --features tagteam/test-support --test signals
  cargo test -p tagteam --features tagteam/test-support --test tui_pty
  ```
````

- [ ] **Step 2: Run them and see them fail**

Run (outside the sandbox): `cargo test -p tagteam --features test-support --test tui_pty`
Expected: only `the_emulator_reads_back_text_moves_clears_and_the_alternate_screen` passes. Every
other test is red if the binary is not yet the TUI of Tasks 10–12: with those tasks in place
they pass at once, since this task adds tests and no production code. Check that each fails
for the right reason by breaking what it pins, one at a time, and restoring it:
- `term::restore`'s give-back emptied:
  `the_first_frame_shows_the_accounts_and_q_leaves_the_terminal_as_it_was` and
  `sigterm_exits_143_with_the_terminal_restored` fail on "the TUI never left the alternate
  screen".
- `tui_cmd::run`'s `signals::survive_quit` call removed:
  `under_r_ctrl_backslash_ends_the_session_and_the_tui_survives_and_returns` fails with
  "it exited (signal: 3 (SIGQUIT)) before the wait for Enter".
- `CliHost::run_session` no longer recording the forwarded signal again:
  `under_r_a_sigterm_to_the_tui_is_forwarded_and_both_exit` fails with "it was still running
  20s later" (the TUI waits for Enter instead of shutting down).
- `LogConfig.stderr` passed as `true` for the TUI:
  `debug_lines_go_to_the_log_file_and_never_to_the_terminal` fails on the DEBUG line in the
  output.

- [ ] **Step 3: Implement**

No production code: Tasks 10–12 implement what these tests pin. If a test fails here, the fix
belongs in the task whose code it names; fix it there, in this branch.

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run (outside the sandbox): `cargo test -p tagteam --features test-support --test tui_pty`
Expected: PASS, 16 tests.

Run (outside the sandbox, on an idle machine):
`cargo test --release -p tagteam --features test-support --test perf -- --ignored --nocapture tui_first_frame`
Expected: PASS, and the printed p95 at or under 50 ms.

Run (outside the sandbox): `cargo test --workspace --features tagteam/test-support`
Expected: PASS.

- [ ] **Step 5: Format and lint**

`cargo fmt --all && cargo fmt --all --check`, then
`cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`, then
`cargo clippy --workspace --all-targets -- -D warnings`, then, since `perf.rs` compiles only in
release builds,
`cargo clippy --release -p tagteam --all-targets --features test-support -- -D warnings`

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam/tests/common/mod.rs crates/tagteam/tests/common/pty.rs \
  crates/tagteam/tests/tui_pty.rs crates/tagteam/tests/perf.rs CONTRIBUTING.md
git commit -m "Test the TUI end to end on a pseudo-terminal, and time its first frame"
```
