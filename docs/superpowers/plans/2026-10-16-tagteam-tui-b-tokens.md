# tagteam TUI, part B — the Tokens screen: Implementation Plan

**Status:** In progress — https://github.com/michaelrigart/tagteam/pull/18

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The TUI's Tokens screen (`2`, or `tagteam tui tokens`): the token report by day, month, session or 5-hour block, grouped and filtered by account and model, with drill-down from a period to its sessions and from a session to its models. Its index is brought up to date in timer mode on a lane of its own, and the report is read again when the index, the accounts or the clock move.

**Architecture:**
- **The engine** shares the token report's words with the CLI (`tagteam_engine::words`), as part A's Task 1 did for `list`'s, and gains `tokens::default_since_ms` and a public `RowKey::new` (Task 1).
- **`Source`** gains the three token methods of spec §4.4. `EngineSource` calls `refresh_tokens` with `Wait::Try`, no budget and the engine's own cancel token, which is the shutdown token (Task 2).
- **Two more lanes** (spec §4.3):
  - **report** serves `token_report`, `token_index_state` and the filter's inventory;
  - **index** serves `refresh_tokens`, and passes its progress on at most every 250 ms.

  Both are spawned, closed and joined with the views and actions lanes, so the shutdown (spec §10.2) drains them too (Tasks 3, 4).
- **`screens/tokens/`**, the screen in four files:
  - `mod.rs`: its state, query, drill-down and keys (Task 3);
  - `index.rs`: the index's state, its refresh and every trigger that reads the report again (Task 4);
  - `table.rs`: the drawing (Task 5);
  - `filter.rs`: the filter dialog, an overlay within part A's (Task 6).
- **The binary** gives the TUI the Tokens provider and the system zone (`TuiOptions`), and `tui` takes `tokens` (Tasks 3, 7).

**Tech Stack:**
- As part A: Rust, edition 2024, at 1.88; `ratatui` 0.30.2; `crossterm` 0.29.0; std threads and `mpsc`.
- Part B adds `jiff` (=0.2.37, already the workspace's for the token report) to `tagteam-tui`'s dependencies (spec §4.1).
- Tests: ratatui's `TestBackend`; the engine over FakeAgent's transcripts (`crates/tagteam-tui/tests/source.rs`); `openpty` (`tests/tui_pty.rs`); `tests/perf.rs`.

**Spec:** `docs/superpowers/specs/2026-10-14-tagteam-tui-design.md` (In progress: part A implemented). "TR §n" cites `docs/superpowers/specs/2026-10-14-tagteam-token-report-design.md` (Implemented), and TR §10 is the authority on the types. Read before any task:
- the TUI spec: §1.1, §3, §4.3, §4.4, §5.1, §5.3, §5.5, §5.6, §6.1, §6.4 (timer mode only), §9, §10.2, §10.4, §11 and §12.3;
- TR §5 (the refresh, `Wait`, `Busy`, the cancel), §9 (blocks), §10 and §10.1 (the view and its rules), §11.1 and §11.2 (the defaults and the words), and §14 (the partial index);
- part A's plan, `docs/superpowers/plans/2026-10-14-tagteam-tui-a-core.md`: its rulings R1–R22, which stay in force, and its Interface Contract, which this plan extends.

**Parts.** The TUI spec is implemented in three plans:
- **A, the core TUI,** is implemented on `tui`.
- **B (this plan)** implements spec §5.3, §6.4 in timer mode, §9, the report and index lanes of §4.3, the token methods of §4.4, the Tokens keys and `2` of §5.6, `tagteam tui tokens` (§3), §11's tests for these, and §12.3.
- **C, the live daemon client,** takes the rest of §6.4: live mode, the `token-index` event, and a daemon whose jobs keep the index current. The daemon has merged, but part B does not use it.

## Execution notes

- **Where:** worktree `~/Code/tagteam-tui`, branch `tui`, from `56b1cc2` (part A on main `d6c2e2e`, which has the token report and the daemon). When execution starts, set this plan's `**Status:**` to `In progress` in one commit.
- **No new crates:** `jiff` is in the workspace and in `Cargo.lock` already. Task 2's build adds it to `tagteam-tui`'s entry in the lock, offline.
- **Outside the sandbox:** `openpty` fails with EPERM inside it. Run these there:
  - `cargo test -p tagteam --features test-support --test tui_pty`;
  - the timing tests: `cargo test --release -p tagteam --features test-support --test perf -- --ignored tui_first_frame`.

  The sandbox also refuses `tagteam`'s `prompt::tests`, `tests/signals.rs` and `xtask`'s compat tests, as part A found, and since the daemon merged its socket and process tests: `tagteam`'s `service_cmd` unit test, `tests/daemon_cli.rs` and `tests/daemon_procs.rs`; `tagteam-daemon`'s `client`, `server` and `umask` tests; and two of the engine's `tests/doctor.rs`. They pass outside it.
- **Format and lint, in every task:** `cargo fmt --all` first. Clippy passes both with and without `--features tagteam/test-support`, with `-D warnings`.
- **Test isolation (core §15.1):** tests never reach the real HOME, network, Keychain or `claude`.
- **Unsafe code:** none in part B.
- **Commits:** one imperative sentence in the repository's style, with no prefix and no attribution lines.
- **The code blocks are exact.**
  - `In PATH, replace:` names text that occurs once in the file as the step finds it; `with:` is what takes its place.
  - `Create PATH:` is a whole file.
  - A new module's tests are created first (Step 1), and its code is then put above them with one blank line between (Step 3).

### Integration notes

Each task below was applied as written, in order, in a scratch clone of `tui` at `56b1cc2`, and committed as its steps say. The tasks were first written and verified on `be464c9`. They were then rebased onto `56b1cc2`, which is part A rebased onto main `d6c2e2e` with the daemon, and verified again. After each of the plan review's first three rounds, its fixes were folded into the tasks that own them (round 1: Tasks 3–6; round 2: Tasks 5 and 6; round 3: Task 5), each test first, and the whole plan verified again the same way. A script applied every `replace`, `Create` and `put this above` block to the clone exactly as the plan prints it, checked that each `replace` text occurred once, and compared each task's commit with the reference tree byte for byte. After every task:
- Step 2's run failed as the step says;
- its own tests, `cargo fmt --all --check` and both clippy runs passed.

The workspace suite (`cargo test --workspace --features tagteam/test-support --no-fail-fast`) ran after Tasks 1 and 7, and the pty suite after Tasks 4 and 7.

**Totals:**
- The workspace suite at the end: 3708/3708 (after Task 1: 3641/3641; Task 1 did not change in any review round, so that run stands). The sandbox refuses 85 of those tests, which pass outside it:
  - `tagteam`'s library (10: `prompt::tests` 9, `service_cmd` 1);
  - `tests/daemon_cli.rs` (4), `tests/daemon_procs.rs` (5), `tests/signals.rs` (1), `tests/tui_pty.rs` (18 of its 19);
  - `tagteam-daemon`'s `client` (13), `server` (17) and `umask` (1);
  - the engine's `tests/doctor.rs` (2);
  - `xtask`'s compat tests (14).
- `tests/tui_pty.rs`: 19/19 (after Task 4: 16/16).
- The first frame, release build, 50 runs each, p95 with nothing due and while the index catches up over 7 MB of transcripts:
  - after round 3, at a load average of 3–8: 10.8 ms and 10.5 ms, then 11.3 ms and 10.5 ms on a second run;
  - after round 2, at a load average of 7–16: 11.5 ms and 11.3 ms, then 12.8 ms and 11.5 ms on a second run;
  - after round 1, at a load average of 5–9: 33.1 ms and 10.8 ms, then 13.2 ms and 10.0 ms on a second run;
  - before it, at a load average of 3–7: 31.9 ms and 31.9 ms, then 34.5 ms and 33.0 ms;
  - on `be464c9`, on a quieter machine: 11.1 ms and 9.7 ms.

  Part A's own test and part B's measure alike, so the catch-up adds nothing to the first frame; the spread is the machine's load.

What verification found and changed in the drafted tasks, one line each:

- **The rebase onto `56b1cc2`:** Tasks 1–6 applied unchanged, with the same tests and counts. In Task 7, `tests/perf.rs`'s module doc and imports now start from the daemon's: its doc names the daemon's store, and its imports add `Guarded` and `short_tempdir`.

- **Task 3:** `Cmd` no longer derives `Eq`, because `TokenQuery` derives only `PartialEq` (B22).
- **Task 3:** testutil spells its 30 days in hours. A `DAY_MS` of its own clashed with the screen module's under the tests' two glob imports.
- **Task 3:** `tui_cmd.rs` changes in the same task as `TuiOptions`, whose two new fields it must set for the build to pass (B6).
- **Task 4:**
  - `update::drawn` now asks for the index's first refresh, so part A's `the_first_frame_that_shows_accounts_asks_for_one_collection` expects `RefreshIndex` from the first frame (B5);
  - the runtime's idle-lane and UI-panic tests count four joined lanes (Task 3: three).
- **Task 4:** the trigger tests answer each re-query before the next trigger. An unanswered one defers the next (B2), which the first draft of `the_active_blocks_end_reads_the_report_again_once` had read as a missing trigger. Its block now ends before the 60 s re-query is due.
- **Task 5:** a report's column widths are found once per report (`table::Widths`, B17). Measured on a 30,000-row session report at 120×40, a frame took 41 ms in a release build and 320 ms in a debug build with the widths found each frame, and about 1–6 ms in a release build with them found once.
- **Task 5:** Step 1 declares `mod table;`, so that Step 2's run builds the new tests. Its tests are the module's only code until Step 3.
- **Task 5:** `the_account_and_model_columns_follow_the_grouping` also draws grouping `None`, whose `ACCOUNT` cell lists a row's accounts.
- **Task 6:** the dialog's key function is `filter::handle_key`. A function named `key` clashed with testutil's `key` under the tests' glob imports. The dialog takes `Home` and `End`, which the drafted tests pressed.
- **Task 7:** the pty test of Ctrl-C under `R` parks the index's refresh at the engine's `refresh-tokens-locked` point (`TAGTEAM_TEST_PAUSE_AT`, from `test-support`'s test hooks). It holds the refresh's lock across the whole session, then lets it go and reads `refreshed ` in the report's status line.

**The plan review, round 1.** Each fix began with a test that failed on the code before it; the red runs are below. The fixes are B24–B28.

- **Columns at 80 (B24, Task 5).**
  - **Red:** at 80×24 the widest report's heading ended at `TOTAL`. `COST` and every cost were off the edge.
  - **Why the old test passed:** `nothing_runs_off_the_edge_at_80_columns` checked only that each line fit the buffer, which ratatui guarantees by clipping.
  - **The fix:** `every_required_value_shows_whole_at_80_columns_in_the_widest_report` replaces it, and reads each required value at 80×24. The fix itself is the period's compaction and the never-cut numbers.
- **The detail (B25, Task 5).**
  - **Red:** the detail's first row was the session's text, not its numbers, and its three-row cap could cut the cache split away.
  - **The fix:** the numbers come first and are never cut. Below the total, the detail and footnotes scroll with `J`/`K`. Under a full banner and toast, `J` reaches every value of the realistic session's detail and of five footnotes, and `K` returns to the top.
- **Lane isolation (B28, Task 4).**
  - **The mutation:** a lock that the report lane's work takes, and that the index lane holds through its refresh.
  - **The old test passed under it:** its report was asked for before the refresh started, so nothing made it wait.
  - **The new test fails under it** with `the report lane answered while the catch-up ran: Timeout` after 5.01 s. It asks for the report only once it has seen the refresh start, and passes without the mutation.
- **The sweep for Tokens text at 80×24 that can hide information:**
  - **The header and status line (B26):**
    - **Red:** a filter on a long alias ended in `…`.
    - **Red:** under part A's `flow`, a drill-down from a session with a long model cut its crumb at `claude-o…`.
    - **The fix:** both now wrap whole, with tests at 80×24.
  - **The help (B27, Task 3):**
    - **Red:** the box clipped `[ ]`'s and `Enter`'s descriptions at 80 (`… 90 days or a`, `… a session's mod`).
    - **The fix:** they are shorter now, and a test reads every Tokens help line whole.
  - **The filter dialog (B27, Task 6):**
    - **Red:** a long choice and a long failure each ended in `…`. Against the old render the test fails on both halves.
    - **The fix:** both wrap, and the list is placed so that the cursor's whole choice shows.
  - **The active block's figures and the footnotes:** they are laid out by their parts (B26) and no longer pass through a cut.
  - **Fine as they were:**
    - the total line holds only numbers, which B24 covers;
    - the footer's longest keys line is 75 columns;
    - the banner and the toast are part A's, and they wrap.
- **The sweep for tests that wait or loop:** every one in part B was read for whether it can pass without the thing it names.
  - **Vacuous:** only the lane test and the old width test, both replaced above.
  - **Not vacuous:**
    - `tokens_reads_its_report_on_the_report_lane` requires the report lane's own error;
    - the progress test requires two to four progress messages in 600 ms;
    - the `Source` tests take the lock, or request the cancel, before the call;
    - the pty Ctrl-C test waits for the refresh to be parked before `R`, and requires `refreshed ` after it is let go;
    - the timing test forgets the index before each run.

**The plan review, round 2.** Wrapped text now has a row cap (B29), which amends round 1's B26 and B27; each fix began with a failing test.

- **The header (Task 5).**
  - **Red:** at 80×24 under a full banner and toast, a filter on a 2,000-column model wrapped onto more rows than the 17-row body holds. The table and the detail got none.
  - **Red:** round 1's tests of a long filter and a long drill-down drew five header rows each.
  - **The fix:** the header is three rows at most, the filter cut first, then the drill-down's rows. The two round-1 tests now pin the three rows and the cut: `the_header_keeps_three_rows_and_a_long_filter_is_cut` and `a_long_drill_down_keeps_its_start_on_the_filters_row`.
  - **The worst case:** `at_80x24_a_2000_column_model_leaves_the_table_three_rows_and_the_detail_reachable` puts a 2,000-column model in the filter, the detail and a footnote, and a 600-column project in the detail. It shows four table rows and an 8-row region below them, and `J` reaches that region's last line. (In round 2 each long piece below the total showed its start on three rows, ending in `…`; round 3 reads them whole.)
- **The filter dialog (Task 6).**
  - **Red:** a 1,400-column model wrapped onto 25 rows under its mark, more than the list has room for. With the cursor on it, the box was clipped at 24 rows: no keys, and no end to the choice.
  - **The fix:** a choice and a failure take part A's `CHOICE_ROWS` at most. `a_choice_takes_three_rows_at_most_and_every_choice_and_the_keys_show` draws the choice, and a 1,400-column failure, at 80×24.
- **The sweep:** round 2 capped every place the Tokens screen wraps text: the header, the dialog's choices, failure, `loading` and keys lines, and the pieces below the total. Round 3 kept only the first two.

**The plan review, round 3.** The cap of round 2 is narrowed to text that does not scroll (B29): the header and the filter dialog. Below the total, text wraps in full again, since its region is bounded by `below_rows` and scrolls.

- **Red, with the cap:**
  - a footnote naming twenty unpriced models lost its last names and its reason;
  - a session's twelve accounts and fifteen models lost their tails;
  - in both, what was left fit the region, so there was nothing to scroll: no `J K` hint;
  - the round-1 test of twelve unpriced models (round 2 had cut it to eight) no longer read its footnote whole;
  - the worst case read its 600-column project cut.
- **The fix:** `laid_out` wraps in full again (Task 5).
  - `j_scrolls_through_a_footnote_longer_than_three_rows_to_its_last_name` and `j_reaches_every_account_and_model_of_a_session_whose_lists_wrap_past_three_rows` read the region line by line as each `J` brings one in.
  - The worst case now reads the model, its footnote and the project whole by scrolling, and the table keeps its rows.
  - The round-1 test names its twelve models again.

### Rulings

Part A's rulings R1–R22 stay in force. Part B's:

- **B1, labels come from the account list.** Tokens names an account as the report's `RowAccount` does. The "accounts changed" trigger of spec §6.4 is driven by the accounts view, not by `changed_at_ms`. Per the token report's author, `changed_at_ms` moves on index writes (a session's project fill and a recorded pending launch among them) and on changes of the active account. It does not move on an alias rename, a reorder or the removal of an account that is not active. So a re-read of `accounts` that lists the Tokens provider's accounts differently (ID, position, label or alias; one added or removed) reads the report again while Tokens is open (Task 4).
- **B2, the report off the UI thread, and the last one kept.** `token_report` can take about 1.4 s over a year of data, so it runs only on the report lane.
  - **The same query again** (a trigger of §6.4, or opening the screen) keeps the report on screen, and the status line says `updating…`. A second one asked for while one runs waits for it, and one more query follows it, so a slow query is never starved by newer ones.
  - **A new query** (a period, grouping, range or filter, or a level of the drill-down) is asked for under a new generation at once, and shows `loading…` until it returns.
  - Generations drop every stale result (Task 3).
- **B3, pending launches.** `token_report` also reads the launches `run` kept in pending files. A profile whose pending evidence cannot be read leaves the provider's unsighted usage unattributed. The TUI renders what the report returns.
- **B4, the index lane and the shutdown.** The index lane's engine takes the shutdown token, so a refresh stops at its next chunk (TR §5.2). The shutdown's seven steps close and join the report and index lanes with the others, and the footer's `finishing…` covers them (Tasks 3, 4).
- **B5, the first refresh follows the first frame.** Spec §6.4's refresh "when the TUI starts" is asked for by `update::drawn` after the first frame, whatever that frame shows, once. R19's first collection works the same way. The 50 ms first frame (spec §1.1) is then untouched by the catch-up's four parser threads and its batches (Task 4; Task 7 measures it with a cold index).
- **B6, the Tokens provider and the zone come from the binary.**
  - `TuiOptions` gains `tokens_provider`, which is `--provider`'s, else the default (spec §5.3).
  - It gains `tz`, the system zone, UTC when it cannot be found, as `tagteam tokens` takes it. The binary reads it before the terminal is touched, so the UI thread reads no zone file (spec §1.1 criterion 3).
- **B7, the filter dialog** (spec §5.3) is a multi-select overlay within part A's: `Overlay::Filter`.
  - **Keys:** it takes every key but Ctrl-C and Ctrl-Z (R9). `j`/`k`, the arrows, `Home` and `End` move; `Space` chooses; `a` chooses all or none of the cursor's kind; `Enter` applies; `Esc` cancels.
  - **The choices:** each starts with what tells it apart: the position, `removed <id>`, or the model's name. One wider than the dialog wraps under its mark (B27), on three rows at most (B29).
  - **The list** is placed from the cursor each frame (R22's rule for lists that keep no scroll). The keys always show below it.
  - **While its inventory is read,** the dialog says so. A failed read says why, in the dialog.
- **B8, a new query starts at the top.** `p`, `g`, `[`/`]` and an applied filter change the top level's query and drop the drill-down. `Enter`, `Esc` and `Backspace` move within it, and a level that is returned to shows its report and its selection at once while it is read again (Task 3).
- **B9, the ranges.** `[`/`]` cycle 7 days, the default slot, 90 days and all. The default slot is 30 days, or for `Month` the twelve calendar months to this one (TR §11.1). The header calls it `30 days` or `12 months`. The top level's `since_ms` is worked out at each query, so the range rolls with the clock.
- **B10, the status line** (spec §5.3, "Indexing"), the header's second part, in this order:
  - the report's `time_zone`;
  - `indexing transcripts 1.2 GB / 3.3 GB` while the TUI's own refresh runs; else `catching up elsewhere` after a `Busy`; else `refreshed HH:MM`, with the date when that was not today;
  - `catching up` while `refreshed_at_ms` is `None`;
  - `updating…` while the report on screen is read again (B2).

  The 5 s checks of spec §6.4 run while the TUI's refresh runs, and also while its last refresh returned `Busy`. Then another process is catching up, and the table fills in as its batches commit too (Task 4).
- **B11, the selected row's detail.** Below the total line, the selected row is shown in full: the responses and the cache split, then a session's ID, whole project and times, a block's span and kind, every account and model. So at 80 columns what a cell cuts, or what only 120 columns show, can still be read (R21, spec §5.3's drill rows). How it is laid out, and how it scrolls, is B25's (Task 5).
- **B12, a failed token read is Tokens' banner.** A report, a check or a refresh that fails keeps the report on screen. Its words show as the banner while Tokens is open (spec §10.4); the other screens do not show them (Tasks 3, 4).
  - A failed report or check shows the engine's message; the next good report clears it.
  - A failed refresh says `the token index was not refreshed: <message>`; the next refresh that returns clears it.
  - A failed inventory shows in the filter dialog itself (Task 6).
- **B13, `r` on Tokens** (spec §7.1) refreshes the index at once, on the index lane, so no running action holds it up (it is not `busy:`). While a refresh runs it shows the toast `the token index is already being refreshed`, and asks for no second one (Task 4).
- **B14, the report's own types in the tests** (spec §12.3).
  - `RowKey::new` becomes public, so the TUI's fixtures key their rows as the report does. No type changes shape (spec §9).
  - `update` and `view` tests build `TokenReport` values from the engine's types.
  - `EngineSource`'s token methods run over FakeAgent's transcripts, with a real drill-down.
  - The pty tests run over Claude Code's (Tasks 1, 2, 7).
- **B15, the words.**
  - **Shared with the CLI:** `scaled`, `dollars`, `plural`, `NO_TOKEN_USAGE`, `unreadable_transcripts` and `token_footnotes` move to `tagteam_engine::words`, and `default_since_ms` to `tagteam_engine::tokens`. The CLI's output does not change by a byte; its tests are the guard.
  - **The `≥` mark:** the TUI marks a partial cost and a lower-bound output with `≥` (spec §5.3), so its unpriced footnote says `(≥ marks a partial cost)` where the CLI's says `(* marks …)`.
  - **Unreadable files:** the TUI lists them among its footnotes (spec §5.3), where the CLI warns on stderr (TR §11.3) (Task 1).
- **B16, the period column.**
  - `DAY` reads `2026-10-14` and `MONTH` `2026-10`.
  - `SESSION` and `BLOCK` read `10-14 09:12–11:40` in the report's zone, with the year when it is not this one, and the second date when the span ends on another day. Where that leaves the text columns too little, the spans compact (B24).
  - A block has a column of its own saying `observed` or `inferred`. A session has a `PROJECT` column, cut from the left so the path keeps its end.
  - A row whose period is the row's above has its period columns left blank, as the spec's mock shows, except at the top of the view (Task 5).
- **B17, the other columns** (spec §5.3).
  - **At 80:** `ACCOUNT` always, listing a row's accounts when the rows are not split by account; `MODEL` when the grouping the report applies has the model; `INPUT`, `OUTPUT`, `TOTAL` and `COST`.
  - **From 120:** `CACHE W` (or `CW 5M` and `CW 1H` when the table still fits with both), `CACHE R`, `RESP`, and an 8-cell `█` bar of cost relative to the largest row, rounded up.
  - **Text columns** share what the others leave, each cut by display width (B24).
  - **Marked cells:** a `≥` cell takes the warning style (yellow with colour, bold without). `—` is a missing cost.
  - **Widths** are found once per report, not per frame (Task 5).
- **B18, the timers** (spec §6.4), on the UI's tick while Tokens is open, with no clock of their own:
  - the index is refreshed 60 s after the last refresh returned, at once on opening the screen if that is past;
  - the report is read again 60 s after the last one arrived, or failed;
  - the index's state is checked every 5 s while a refresh runs (B10);
  - the report is read once more when the shown active block's `end_ms` passes, once per end;
  - between queries, the active block's elapsed and remaining time count on from its `start_ms` and `end_ms`, and at its end the row shows no active figures (Task 4, Task 5).
- **B19, opening Tokens reads the report again.** Whatever happened while another screen was open, the report on screen may be stale (Task 3).
- **B20, the defaults** are `tagteam tokens`'s: by day, grouped by account (the spec's mock shows `group: account`), over the default range, unfiltered.
- **B21, the keys.**
  - `j`/`k` and the arrows move a row; `PageUp`/`PageDown` a page; `Home`/`End` to either end. `g` is the grouping, so Tokens has no `g`/`G` for the ends.
  - `Tab` goes Accounts → Tokens → History.
  - The footer names what `Enter` opens (`Enter sessions`, `Enter models`) and leaves it out at a session's per-model rows.
  - The header names the rows the drill-down came through (`› 2026-09-20 1 a1@x.co`).
- **B22, `Cmd` drops `Eq`.** `Cmd::LoadReport` carries a `TokenQuery`, which derives `PartialEq` only. Tests compare commands with `assert_eq!`, which needs no more.
- **B23, the start-up refresh creates the store when there is none.** `refresh_tokens` writes the index, so it creates the store, as `tagteam tokens` does (TR §11.1). Part A's TUI only ever read the store. The spec does not say which; this plan takes the refresh as the engine gives it, and `r` on Tokens does the same.
- **B24, at 80 columns the numbers always show whole** (spec §5.3's columns at 80). The period, `INPUT`, `OUTPUT`, `TOTAL` and `COST`, the account, and the model when the report applies a grouping by model always show; a number is never cut or dropped. The text gives way, in this order:
  1. **The period's spans compact**, each step only as far as the columns need, the same for every row of the report:
     - `2025-09-20 09:12–2025-09-21 11:40`, as B16 says;
     - `09-20 09:12–09-21 11:40`, without years;
     - `09-20 09:12–11:40+1d`, the end's time alone, with the days it ends after;
     - `09-20 09:12`, the start alone.

     The step is the fullest that leaves each text column its least: 6 columns for the account and model (`TEXT_MIN`), 4 for the project (`PROJECT_MIN`), or its own width when that is less.
  2. **The account, model and project are cut** by display width with `…` to share the rest, the narrowest first; the project keeps its end. The header's names are cut as their column is.

  The detail (B25) reads every value whole. Each step's widest span is found with the report's widths, once (B17) (Task 5).
- **B25, the detail and the footnotes below the table.** The selected row's detail comes first, then the footnotes:
  - **Order:** the numbers (responses and the cache split) come first, laid out by their parts and never cut. The text (the session's ID, project and times, a block's span and kind, the accounts, the models) follows, and wraps.
  - **Room:** they take as many rows as they need, half the body at most, and never the table's last three (`TABLE_MIN`).
  - **Scrolling:** when they need more, `J`/`K` scroll them a line at a time, as Accounts' detail pane scrolls (R20). The last row says `J K: lines <a>–<b> of <n>`. The scroll is kept for the selected row, starts at the top for another, and is clamped where it is used (R22). The help lists `J K` on Tokens (Task 5).
- **B26, text below the total wraps, and is never cut.** The active block's figures, the detail and the footnotes are laid out by their parts. A part that fits no row whole wraps at its spaces, and a word wider than a row breaks by width (part A's `view::wrap`). The region scrolls (B25), so no piece of it is capped (B29). The header has rows of its own (B29), and the table's text cells are cut (B24) (Task 5).
- **B27, the dialog and the help at 80×24.** The filter dialog wraps a choice under its mark, up to B29's cap, and places its list so that the cursor's whole choice shows. Its failure, `loading` and keys lines wrap too, with the same cap (B7). Each line of the Tokens help fits the help box (Tasks 3, 6).
- **B28, a test that waits proves what it waits for.** A test that waits for something to happen while something else runs observes that the other thing runs first, and requires the answer to the request made after it. The lane test asks for a report only once it has seen the refresh start, and requires that report's answer before `q` stops the refresh; a report lane made to wait behind the refresh fails it (Task 4). A layout test reads the values a person needs, not only the buffer's width (Task 5).
- **B29, text that does not scroll has a row cap** (amends B26 and B27, as part A's R20 and R21 cap labels, choices and questions). Nothing scrolls within a piece of text, so what passes its cap is cut with `…`. Only the header and the filter dialog are capped; text below the total scrolls instead.
  - **The header is three rows at most:**
    1. the periods, the range and the grouping, 60 columns at the widest;
    2. the filter and the drill-down's rows. They join the first row when they fit there whole, as the default filter does at 120 columns. Otherwise they take a row of their own: the filter gives way first, down to `FILTER_LEAST` (20) columns, then the drill-down's rows are cut. The filter's dialog shows the whole filter, and each row's detail its own session, account and model;
    3. the status line, one row.
  - **The filter dialog:** a choice and a failure take `CHOICE_ROWS` rows at most (3, part A's choose dialog's), and the keys keep their own rows. So the cursor's whole choice and the keys always fit.
  - **Below the total, nothing is capped.** The detail's project, accounts and models, every footnote, and the active block's figures wrap in full. The region is bounded by `below_rows` and scrolls a line at a time with `J`/`K` (B25), so every line is reachable.
  - **By construction,** the table always keeps three rows and `J`/`K` reach every line below it. At 80×24 under a full banner and toast (three rows each at most, part A), the body is 17 rows:
    - the header takes 3 at most;
    - the table's heading and total take 2;
    - what lies below the total takes half the body at most, and never the table's last 3 (B25).

    So the table keeps 3 rows at least. The region below it gets 8 rows, or all its lines when they are fewer, and `J`/`K` scroll it.
    - **Task 5's worst-case test** draws this with a 2,000-column model in the filter, the detail and a footnote. The table keeps its rows, and scrolling reads the detail and the footnote whole.
    - **Two more Task 5 tests** scroll to the last name of a footnote naming twenty unpriced models, and through a session's long lists of accounts and models.
    - **Task 6's test** draws a 1,400-column model as a choice (Tasks 5, 6).

## Global Constraints

- **Start-up:** the first frame showing the accounts table within 50 ms p95 when no usage fetch is due (spec §1.1 criterion 2), whatever the index holds. The index's catch-up starts after the first frame (B5).
- **The UI thread does no I/O** but the terminal (spec §1.1 criterion 3). `token_report`, `token_index_state` and `refresh_tokens` run only on the report and index lanes. The zone is read by the binary (B6).
- **The index refresh:** `refresh_tokens(provider, Wait::Try, None, <the shutdown token>, progress)` (spec §9), on its own lane (spec §4.3). `Busy` shows `catching up elsewhere`.
- **Timer mode** (spec §6.4):
  - the index is refreshed when the TUI starts (after the first frame), then every 60 s while Tokens is open, and at once on `r`;
  - while Tokens is open the report is read again when the index changed (`changed_at_ms` past the report's; checked every 5 s while a refresh runs, and after each refresh returns), when the accounts changed (B1), every 60 s, and at the active block's end;
  - a check that finds no change reads nothing.
- **The query** (spec §5.3, TR §10):
  - the provider is `--provider`'s, else the default; the zone is the system zone;
  - `p` cycles `Day`, `Month`, `Session` and `Block`; `g` cycles `None`, `Account`, `Model` and `AccountModel`;
  - `[`/`]` give 7, 30 or 90 days, or all, as `since_ms` with no `until_ms`, and for `Month` the default starts on the first day of the eleventh month before this one;
  - the filter sends `None` for a kind whose every choice is chosen, `Some` of exactly the chosen ones otherwise, and refuses none of a kind.
- **Marks:** `≥` before an output with `unfinalized > 0` and before a partial cost; `—` for a missing cost. Token counts are scaled (`12.4M`) and costs are dollars with cents (TR §11.2).
- **Size:** at 80×24 and up nothing a person needs runs off the edge (R20, R21).
  - The table's numbers are never cut; its text cells are cut by display width after the period compacts (B24), and the detail reads them whole.
  - Everything else wraps (B26, B27). The header takes three rows at most, and so does a dialog's choice (B29). The detail and the footnotes take half the body at most, leave the table three rows (`TABLE_MIN`), and scroll with `J`/`K` (B25), so nothing below the total is cut.
  - Below 80×24 part A's notice replaces the body.
- **Colour:** only the 16 named ANSI colours; without colour, emphasis is bold and the selection reverse video (spec §5.1).
- **Coalescing and generations** (spec §4.3): a queued report lane request is replaced by a newer one of its kind, and so is a queued refresh. `update` drops a report, a check or an inventory whose generation is out of date.

## Review Focus

1. **A cold index on a large tree.** The first frame is not delayed. The header shows `indexing transcripts <done> / <total>`, and the table fills in as batches commit (the 5 s checks). Owners:
   - Task 4: `while_a_refresh_runs_a_check_every_5_s_reads_the_report_again_only_when_the_index_moved`;
   - Task 5: `the_header_says_how_current_the_index_is`;
   - Task 7: `tui_first_frame_p95_is_within_50_ms_while_the_index_catches_up`.
2. **A slow report (a year of sessions) under a stream of triggers.** A re-query never starves the one running, a result of an older generation never overwrites a newer one, and a large report is drawn without redoing its widths each frame. Owners:
   - Task 3: `a_query_asked_for_while_one_runs_follows_it_once` and `a_report_of_an_older_generation_is_dropped`;
   - Task 5: `a_reports_widths_are_found_once_and_anew_for_the_next` (the cost is measured in the integration notes).
3. **Another process holds the index** (`tagteam tokens`, a daemon). The TUI's refresh returns `Busy`, the header says `catching up elsewhere`, and the table still fills in. Owners:
   - Task 2: `a_refresh_another_process_holds_is_busy_at_once`;
   - Task 4: `busy_keeps_checking_while_another_process_catches_up`;
   - Task 5: `the_header_says_how_current_the_index_is`.
4. **80×24 with long names under a full banner and toast.** Every required column's value shows whole, whatever the report: all time, by session, by account and model, long spans, wide CJK aliases, long models, 10-digit counts. The header and a dialog's choice are capped at three rows (B29); below the total, text wraps in full and scrolls. So, by construction, the table keeps three rows, `J`/`K` reach every line below it whole, and the dialog always shows the cursor's choice and its keys, even with a 2,000-column model. Owners:
   - Task 3: `help_on_tokens_shows_each_key_whole_at_80_columns`;
   - Task 5: `every_required_value_shows_whole_at_80_columns_in_the_widest_report`, `a_sessions_detail_shows_its_numbers_first_and_every_value_whole`, `under_a_full_banner_and_toast_j_and_k_reach_every_value_of_the_detail_and_the_notes`, `the_header_keeps_three_rows_and_a_long_filter_is_cut`, `a_long_drill_down_keeps_its_start_on_the_filters_row`, `at_80x24_a_2000_column_model_leaves_the_table_three_rows_and_the_detail_reachable`, `j_scrolls_through_a_footnote_longer_than_three_rows_to_its_last_name` and `j_reaches_every_account_and_model_of_a_session_whose_lists_wrap_past_three_rows`;
   - Task 6: `a_long_choice_wraps_under_its_mark_whole_and_the_cursors_shows_whole` and `a_choice_takes_three_rows_at_most_and_every_choice_and_the_keys_show`.
5. **Quitting, or Ctrl-C in an `R` session, during a long catch-up.**
   - `q` stops the refresh at its next chunk, and every lane is joined promptly.
   - The terminal's Ctrl-C belongs to the session and leaves the refresh to finish.

   Owners:
   - Task 2: `a_refresh_under_the_shutdown_token_stops_and_says_it_was_interrupted`;
   - Task 4: `a_report_asked_for_while_the_index_catches_up_is_answered_and_q_stops_the_catch_up`, which fails if reports wait behind the refresh (B28);
   - Task 7: `under_r_ctrl_c_ends_the_session_and_a_running_index_refresh_goes_on`.

## Interface Contract

Every task's names, signatures and strings must match this section exactly. Part A's contract (C1–C9) holds, amended as below.

### CB1. The engine (Task 1)

`crates/tagteam-engine/src/words.rs` gains, with the CLI's bodies:

```rust
pub const NO_TOKEN_USAGE: &str = "No token usage in this range.";
pub fn plural(n: u64, noun: &str) -> String;                 // "1 noun", "n nouns"
pub fn scaled(n: u64) -> String;                             // tokens_cmd::scaled
pub fn dollars(nanos: u64) -> String;                        // tokens_cmd::dollars
pub fn unreadable_transcripts(n: u64) -> String;             // "N transcript files could not be read; the next refresh tries again"
pub fn token_footnotes(report: &TokenReport, mark: &str, unreadable: bool) -> Vec<String>;
```

`crates/tagteam-engine/src/tokens/view/mod.rs`:

```rust
pub fn default_since_ms(period: &TokenPeriod, now_ms: i64, tz: &TimeZone) -> i64; // tokens_cmd's
impl RowKey { pub fn new(period: &PeriodKey, account: Option<&RowAccount>, model: Option<&str>) -> Self; }
```

### CB2. `Source` (Task 2)

```rust
pub trait Source {
    // part A's three methods, then:
    fn refresh_tokens(&self, provider: &ProviderId, progress: &mut dyn FnMut(&IndexProgress))
        -> Result<RefreshOutcome, SourceError>;
    fn token_report(&self, query: &TokenQuery) -> Result<TokenReport, SourceError>;
    fn token_index_state(&self, provider: &ProviderId) -> Result<IndexState, SourceError>;
}
```

`EngineSource::refresh_tokens` is `engine.refresh_tokens(provider, Wait::Try, None, engine.cancel(), progress)`.

### CB3. Messages, commands and requests (Tasks 3, 4, 6)

`msg.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum ReportReq {
    Report { r#gen: u64, query: TokenQuery },                // Task 3
    IndexState { r#gen: u64, provider: ProviderId },         // Task 4
    Inventory { r#gen: u64, query: TokenQuery },             // Task 6
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexReq { Refresh { provider: ProviderId } }      // Task 4

// Msg gains:
TokenReport { r#gen: u64, result: Result<TokenReport, SourceError> },  // Task 3
IndexState { r#gen: u64, result: Result<IndexState, SourceError> },    // Task 4
IndexProgress(IndexProgress),                                          // Task 4
Refreshed { result: Result<RefreshOutcome, SourceError> },             // Task 4
Inventory { r#gen: u64, result: Result<TokenReport, SourceError> },    // Task 6

// Cmd (now `#[derive(Debug, Clone, PartialEq)]`, B22) gains:
LoadReport { r#gen: u64, query: TokenQuery },          // Task 3
LoadIndexState { r#gen: u64, provider: ProviderId },   // Task 4
RefreshIndex { provider: ProviderId },                 // Task 4
LoadInventory { r#gen: u64, query: TokenQuery },       // Task 6
```

Coalescing: two report lane requests of the same variant; two `IndexReq`s that are equal.

### CB4. State (Tasks 3, 4, 6)

```rust
pub enum Screen { Accounts, Tokens, History }        // Tab: Accounts → Tokens → History
pub struct State { /* part A's, and */ pub tokens: TokensState, pub(crate) started_ms: i64 }
impl State {
    pub fn new(screen: Screen, color: bool, provider: Option<ProviderId>, tokens: TokensState, now_ms: i64) -> State;
}
pub struct Gens { /* part A's, and */ pub report: u64, pub index: u64, pub inventory: u64 }
```

`State::start` also reads the report when the TUI opens on Tokens.

### CB5. `screens::tokens` (Tasks 3–6)

`mod.rs` (Task 3; Task 4 adds `watch`, Task 5 `render` and `J`/`K`, Task 6 `f`):

```rust
pub mod filter; pub mod index; mod table;
pub use self::table::render;
pub enum Range { Week, Default, Quarter, All }        // label(self, period) -> &'static str
pub struct TokensState { /* private */ }
impl TokensState {
    pub fn new(provider: ProviderId, tz: TimeZone) -> TokensState;
    pub fn provider(&self) -> &ProviderId;
    pub fn period(&self) -> TokenPeriod;
    pub fn grouping(&self) -> TokenGrouping;
    pub fn range(&self) -> Range;
    pub fn report(&self) -> Option<&TokenReport>;
    pub fn selected(&self) -> Option<&RowKey>;
    pub fn depth(&self) -> usize;
    pub fn pending(&self) -> bool;
    pub fn error(&self) -> Option<&str>;
    pub fn watch(&self) -> &IndexWatch;                // Task 4
    pub fn query(&self, now_ms: i64) -> TokenQuery;
    pub fn selected_row(&self) -> Option<&TokenRow>;
    pub fn drill_to(&self) -> Option<&'static str>;    // "sessions", "models", or None
    pub fn selected_index(&self) -> Option<usize>;
    pub fn apply(&mut self, report: TokenReport);      // keeps the selection by RowKey
}
pub(crate) fn load(state: &mut State, now_ms: i64) -> Vec<Cmd>;     // a new query
pub(crate) fn requery(state: &mut State, now_ms: i64) -> Vec<Cmd>;  // the same query (B2)
pub(crate) fn on_enter(state: &mut State, now_ms: i64) -> Vec<Cmd>;
pub(crate) fn on_report(state: &mut State, r#gen: u64, result: Result<TokenReport, SourceError>, now_ms: i64) -> Vec<Cmd>;
pub fn footer_keys(state: &TokensState) -> Vec<(&'static str, &'static str)>;
pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd>;
```

`index.rs` (Task 4):

```rust
pub const REFRESH_MS: i64 = 60_000;
pub const REQUERY_MS: i64 = 60_000;
pub const CHECK_MS: i64 = 5_000;
pub const ALREADY: &str = "the token index is already being refreshed";
pub struct IndexWatch { /* private */ }               // Default
impl IndexWatch {
    pub fn state(&self) -> Option<&IndexState>;
    pub fn refreshing(&self) -> Option<&IndexProgress>;
    pub fn busy(&self) -> bool;
}
pub(crate) fn first_refresh(state: &mut State) -> Vec<Cmd>;            // from update::drawn (B5)
pub(crate) fn refresh_now(state: &mut State, now_ms: i64) -> Vec<Cmd>; // `r` on Tokens
pub(crate) fn on_progress(state: &mut State, progress: IndexProgress);
pub(crate) fn on_refreshed(state: &mut State, result: Result<RefreshOutcome, SourceError>, now_ms: i64) -> Vec<Cmd>;
pub(crate) fn on_index_state(state: &mut State, r#gen: u64, result: Result<IndexState, SourceError>, now_ms: i64) -> Vec<Cmd>;
pub(crate) fn on_report(state: &mut State, now_ms: i64);
pub(crate) fn on_accounts(state: &mut State, now_ms: i64) -> Vec<Cmd>;
pub(crate) fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd>;
```

`table.rs` (Task 5):

```rust
pub const WIDE: usize = 120;
pub const LOADING: &str = "loading…";
pub fn bytes(n: u64) -> String;                       // "1.2 GB"
pub fn account_name(account: &RowAccount) -> String;  // "1 work (a@x.co)", "removed <id>", "unattributed"
pub(super) struct Widths;                             // a report's column widths, and the period's at each step of B24, found once
pub(super) fn page(state: &State, area: Rect, now_ms: i64) -> usize;
pub(super) fn follow(state: &mut State, area: Rect, now_ms: i64);
pub(super) fn scroll_below(state: &mut State, area: Rect, now_ms: i64, by: isize); // `J`/`K` (B25)
pub fn render(state: &State, frame: &mut Frame, area: Rect, now_ms: i64);
```

`filter.rs` (Task 6):

```rust
pub const LOADING: &str = "loading the accounts and models the index holds…";
pub const NO_ACCOUNT: &str = "Choose at least one account.";
pub const NO_MODEL: &str = "Choose at least one model.";
pub struct FilterDialog { /* private */ }
pub(super) fn open(state: &mut State) -> Vec<Cmd>;
pub(crate) fn on_inventory(state: &mut State, r#gen: u64, result: Result<TokenReport, SourceError>);
pub(crate) fn handle_key(state: &mut State, dialog: FilterDialog, key: KeyEvent, now_ms: i64) -> (Option<Overlay>, Vec<Cmd>);
pub(crate) fn render(state: &State, dialog: &FilterDialog, frame: &mut Frame);
```

`actions.rs`: `Overlay::Filter(FilterDialog)`; `boxed` and `CHOICE_ROWS` become `pub(crate)`.

### CB6. The runtime (Tasks 3, 4)

```rust
pub struct TuiOptions {
    pub screen: Screen,
    pub color: bool,
    pub provider: Option<ProviderId>,
    pub tokens_provider: ProviderId,   // B6
    pub tz: TimeZone,                  // B6
    pub signal: Cancel,
    pub shutdown: Cancel,
}
```

`Lanes` holds `views`, `report`, `index` and `actions`, closed and joined in that order. The index lane passes `Msg::IndexProgress` on at most every `PROGRESS_EVERY` (250 ms), then `Msg::Refreshed`.

### CB7. The binary (Tasks 3, 7)

- `tui_cmd.rs` sets `tokens_provider` (`--provider`'s, else `probe.default_provider()`) and `tz` (`TimeZone::try_system()`, else UTC).
- `ScreenArg` gains `Tokens` ("The token report"), between `Accounts` and `History`.

## Tasks

| # | Task | Depends on |
|---|---|---|
| 1 | Shared token words and default range in the engine | — |
| 2 | `Source`'s token methods | — |
| 3 | The Tokens screen's query, drill-down and report lane | 2 |
| 4 | The index lane and the re-query triggers | 3 |
| 5 | Drawing the Tokens screen | 1, 4 |
| 6 | The filter dialog | 5 |
| 7 | `tagteam tui tokens`, docs, and the end-to-end and timing tests | 6 |

Every task runs alone, in table order. No pair qualifies as a batch: Tasks 1 and 2 touch different files, but Task 2's tests build `tagteam-engine`, which Task 1 edits, and every later pair shares `tagteam-tui`.

### Task 1: Shared token words and default range in the engine

The CLI's `tagteam tokens` and the TUI's Tokens screen must say the same numbers and footnotes in the same words (spec §5.3: "as in TR §11.2"), and start a report's range at the same default (TR §11.1). `tagteam-tui` cannot depend on the `tagteam` crate (spec §4.1), so, as part A's Task 1 moved `list`'s words, this task moves `scaled`, `dollars`, `plural`, the empty report's words, the unreadable-files warning and the footnotes to `tagteam_engine::words`, and `default_since_ms` to `tagteam_engine::tokens`. It also makes `RowKey::new` public (B14). The CLI's output does not change by a byte: its unit and snapshot tests are the guard, and the tests of what moved move with it.

**Readings of the spec this task commits to:**
- **The footnotes take the mark** that flags a partial cost (`*` in the CLI, `≥` in the TUI, spec §5.3), and whether to list the files the last refresh could not read: the TUI lists them (spec §5.3), the CLI warns of them on stderr (TR §11.3) (B15).
- **`RowKey::new` becomes public** with its signature and body unchanged. No type changes shape (spec §9).

**Depends on:** —

**Files:**
- Modify: `crates/tagteam-engine/src/tokens/view/mod.rs`
- Modify: `crates/tagteam-engine/src/words.rs`
- Modify: `crates/tagteam/src/tokens_cmd.rs`

**Interfaces:**
- Consumes: the engine's `tokens::{TokenReport, TokenPeriod, PeriodKey, RowAccount}`, `tagteam_core::tokens::{month_of, month_bounds_ms}`.
- Produces: CB1 — `words::{NO_TOKEN_USAGE, plural, scaled, dollars, unreadable_transcripts, token_footnotes}`, `tokens::default_since_ms`, `pub fn RowKey::new`. Task 5 draws with them; Task 3 takes `default_since_ms`; Task 3's fixtures key rows with `RowKey::new`.

**Spec:**
- TUI spec §5.3 (scaled counts and dollars "as in TR §11.2", the footnotes, the `Month` default), §9 (no shapes of the TUI's own).
- TR §11.1 (the default `--since`), §11.2 (the human output's words), §11.3 (the warnings).

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/src/tokens/view/mod.rs`, replace:

```rust
        oldest_ms,
        newest_ms,
    })
}
```

with:

```rust
        oldest_ms,
        newest_ms,
    })
}

#[cfg(test)]
mod tests {
    use tagteam_core::time::{format_iso8601, parse_iso8601_ms};

    use super::*;

    /// `ms` as an ISO 8601 UTC time, to the second.
    fn iso(ms: i64) -> String {
        format_iso8601(ms.div_euclid(1000))
    }

    /// `s`, an ISO 8601 time, in epoch ms.
    fn at(s: &str) -> i64 {
        parse_iso8601_ms(s).unwrap()
    }

    #[test]
    fn default_ranges_follow_the_zones_calendar() {
        // Spec §11.1, at fixed instants: 30 days back for every period but month, and for
        // month the first of the eleventh month back, at midnight where the report's days are.
        let utc = TimeZone::UTC;
        let brussels = TimeZone::get("Europe/Brussels").unwrap();
        let since = |period: TokenPeriod, now: &str, tz: &TimeZone| {
            iso(default_since_ms(&period, at(now), tz))
        };
        let now = "2026-09-21T14:13:20Z";
        assert_eq!(since(TokenPeriod::Day, now, &utc), "2026-08-22T14:13:20Z");
        assert_eq!(
            since(TokenPeriod::Block, now, &brussels),
            "2026-08-22T14:13:20Z",
            "30 days of 24 hours, whatever the zone"
        );
        assert_eq!(since(TokenPeriod::Month, now, &utc), "2025-10-01T00:00:00Z");
        // Across the turn of a month and of a year.
        assert_eq!(
            since(TokenPeriod::Month, "2026-02-28T23:59:59.999Z", &utc),
            "2025-03-01T00:00:00Z"
        );
        assert_eq!(
            since(TokenPeriod::Month, "2026-03-01T00:00:00Z", &utc),
            "2025-04-01T00:00:00Z"
        );
        assert_eq!(
            since(TokenPeriod::Month, "2026-01-15T12:00:00Z", &utc),
            "2025-02-01T00:00:00Z"
        );
        // 23:30 UTC on 31 October is already 1 November in Brussels, on winter time (UTC+1):
        // the range starts on 1 December 2025 at its midnight there.
        assert_eq!(
            since(TokenPeriod::Month, "2026-10-31T23:30:00Z", &brussels),
            "2025-11-30T23:00:00Z"
        );
        assert_eq!(
            since(TokenPeriod::Month, "2026-10-31T23:30:00Z", &utc),
            "2025-11-01T00:00:00Z",
            "still October in UTC"
        );
        // From late March 2026, eleven months back is April 2025, on summer time (UTC+2).
        assert_eq!(
            since(TokenPeriod::Month, "2026-03-29T12:00:00Z", &brussels),
            "2025-03-31T22:00:00Z"
        );
    }
}
```

In `crates/tagteam-engine/src/words.rs`, replace:

```rust
            with(saturated, None),
            "+0.01 pts/h · won't run out within a year (regression)"
        );
    }
}
```

with:

```rust
            with(saturated, None),
            "+0.01 pts/h · won't run out within a year (regression)"
        );
    }

    #[test]
    fn counts_scale_to_one_decimal_from_a_thousand() {
        for (n, text) in [
            (0, "0"),
            (999, "999"),
            (1_000, "1.0K"),
            (1_234, "1.2K"),
            (1_250, "1.3K"),
            (51_000, "51.0K"),
            (999_949, "999.9K"),
            (999_950, "1.0M"),
            (12_400_000, "12.4M"),
            (3_100_000_000, "3.1B"),
            (999_950_000_000, "1.0T"),
            (u64::MAX, "18446744.1T"),
        ] {
            assert_eq!(scaled(n), text, "{n}");
        }
    }

    #[test]
    fn costs_are_dollars_and_cents_rounded_half_up() {
        assert_eq!(dollars(0), "$0.00");
        assert_eq!(dollars(4_999_999), "$0.00");
        assert_eq!(dollars(5_000_000), "$0.01");
        assert_eq!(dollars(622_016_000), "$0.62");
        assert_eq!(dollars(312_400_000_000), "$312.40");
        assert_eq!(dollars(u64::MAX), "$18446744073.71");
    }

    fn report() -> TokenReport {
        use crate::tokens::{IndexState, TokenGrouping, TokenPeriod, TokenQuery, TokenTotal};
        TokenReport {
            query: TokenQuery {
                provider: ProviderId::new(CLAUDE_CODE),
                period: TokenPeriod::Day,
                grouping: TokenGrouping::Account,
                since_ms: None,
                until_ms: None,
                accounts: None,
                models: None,
                session: None,
                tz: jiff::tz::TimeZone::UTC,
            },
            time_zone: "UTC".into(),
            rows: vec![],
            total: TokenTotal::default(),
            index: IndexState::default(),
            unpriced_models: vec![],
            prices_as_of: "2026-10-01",
            price_overrides: vec![],
        }
    }

    #[test]
    fn the_token_footnotes_name_what_the_report_leaves_out_and_mark_partial_costs_as_asked() {
        let plain = report();
        assert_eq!(
            token_footnotes(&plain, "*", true),
            ["API cost at list prices as of 2026-10-01."]
        );
        let mut every = report();
        every.unpriced_models = vec!["claude-test-9".into(), "claude-x".into()];
        every.total.unfinalized = 1;
        every.index.skipped_lines = 12;
        every.index.unreadable_files = 3;
        every.price_overrides = vec!["claude-opus-5-5".into()];
        assert_eq!(
            token_footnotes(&every, "≥", true),
            [
                "No price for claude-test-9, claude-x: counted without cost (≥ marks a partial cost).",
                "Output is a lower bound for 1 subagent response.",
                "12 transcript lines could not be read, so these totals may be low.",
                "3 transcript files could not be read; the next refresh tries again.",
                "API cost at list prices as of 2026-10-01, with config.toml's for claude-opus-5-5.",
            ]
        );
        assert_eq!(
            token_footnotes(&every, "*", false)[0],
            "No price for claude-test-9, claude-x: counted without cost (* marks a partial cost)."
        );
        assert_eq!(
            token_footnotes(&every, "*", false).len(),
            4,
            "the CLI warns of unreadable files on stderr instead (token report §11.3)"
        );
        assert_eq!(
            unreadable_transcripts(1),
            "1 transcript file could not be read; the next refresh tries again"
        );
        assert_eq!(plural(2, "subagent response"), "2 subagent responses");
    }
}
```

In `crates/tagteam/src/tokens_cmd.rs`, replace:

```rust
            Some("unknown time zone \"Mars/Olympus_Mons\"")
        );
    }

    /// `s`, an ISO 8601 time, in epoch ms.
    fn at(s: &str) -> i64 {
        parse_iso8601_ms(s).unwrap()
    }

    #[test]
    fn default_ranges_follow_the_zones_calendar() {
        // Spec §11.1, at fixed instants: 30 days back for every period but month, and for
        // month the first of the eleventh month back, at midnight where the report's days are.
        let utc = TimeZone::UTC;
        let brussels = TimeZone::get("Europe/Brussels").unwrap();
        let since = |period: TokenPeriod, now: &str, tz: &TimeZone| {
            iso(default_since_ms(&period, at(now), tz))
        };
        let now = "2026-09-21T14:13:20Z";
        assert_eq!(since(TokenPeriod::Day, now, &utc), "2026-08-22T14:13:20Z");
        assert_eq!(
            since(TokenPeriod::Block, now, &brussels),
            "2026-08-22T14:13:20Z",
            "30 days of 24 hours, whatever the zone"
        );
        assert_eq!(since(TokenPeriod::Month, now, &utc), "2025-10-01T00:00:00Z");
        // Across the turn of a month and of a year.
        assert_eq!(
            since(TokenPeriod::Month, "2026-02-28T23:59:59.999Z", &utc),
            "2025-03-01T00:00:00Z"
        );
        assert_eq!(
            since(TokenPeriod::Month, "2026-03-01T00:00:00Z", &utc),
            "2025-04-01T00:00:00Z"
        );
        assert_eq!(
            since(TokenPeriod::Month, "2026-01-15T12:00:00Z", &utc),
            "2025-02-01T00:00:00Z"
        );
        // 23:30 UTC on 31 October is already 1 November in Brussels, on winter time (UTC+1):
        // the range starts on 1 December 2025 at its midnight there.
        assert_eq!(
            since(TokenPeriod::Month, "2026-10-31T23:30:00Z", &brussels),
            "2025-11-30T23:00:00Z"
        );
        assert_eq!(
            since(TokenPeriod::Month, "2026-10-31T23:30:00Z", &utc),
            "2025-11-01T00:00:00Z",
            "still October in UTC"
        );
        // From late March 2026, eleven months back is April 2025, on summer time (UTC+2).
        assert_eq!(
            since(TokenPeriod::Month, "2026-03-29T12:00:00Z", &brussels),
            "2025-03-31T22:00:00Z"
        );
    }

    #[test]
    fn a_model_pattern_names_every_model_containing_it() {
        let indexed: Vec<String> = ["claude-opus-5-5", "claude-sonnet-5-5", "claude-test-9"]
            .map(str::to_owned)
```

with:

```rust
            Some("unknown time zone \"Mars/Olympus_Mons\"")
        );
    }

    #[test]
    fn a_model_pattern_names_every_model_containing_it() {
        let indexed: Vec<String> = ["claude-opus-5-5", "claude-sonnet-5-5", "claude-test-9"]
            .map(str::to_owned)
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p tagteam-engine --lib -- words:: tokens::view::`

Expected: FAIL: the build stops with errors such as ``error[E0425]: cannot find function `scaled` in this scope``, ``error[E0425]: cannot find function `default_since_ms` in this scope`` and ``error[E0412]: cannot find type `TokenReport` in this scope``.

- [ ] **Step 3: Move the words and the default range**

The bodies are the CLI's, moved. The CLI then calls them, and its `footnotes` keeps its blank line and its `*`.

In `crates/tagteam-engine/src/tokens/view/mod.rs`, replace:

```rust

use jiff::civil::Date;
use jiff::tz::TimeZone;
pub use tagteam_core::tokens::{Basis, TokenCounts};
use tagteam_core::tokens::{Prices, day_bounds_ms, month_bounds_ms};
use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::Provider;

use crate::engine::Engine;
```

with:

```rust

use jiff::civil::Date;
use jiff::tz::TimeZone;
pub use tagteam_core::tokens::{Basis, TokenCounts};
use tagteam_core::tokens::{Prices, day_bounds_ms, month_bounds_ms, month_of};
use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::Provider;

use crate::engine::Engine;
```

In `crates/tagteam-engine/src/tokens/view/mod.rs`, replace:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RowKey(String);

impl RowKey {
    fn new(period: &PeriodKey, account: Option<&RowAccount>, model: Option<&str>) -> Self {
        let mut key = match period {
            PeriodKey::Day { date } => format!("day:{date}"),
            PeriodKey::Month { year, month } => format!("month:{year:04}-{month:02}"),
            PeriodKey::Session { session_id, .. } => format!("session:{session_id}"),
```

with:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RowKey(String);

impl RowKey {
    /// The key of the row for `period`, `account` and `model`: what the report keys its rows
    /// by, public so that a renderer's fixtures key their rows as the report does.
    pub fn new(period: &PeriodKey, account: Option<&RowAccount>, model: Option<&str>) -> Self {
        let mut key = match period {
            PeriodKey::Day { date } => format!("day:{date}"),
            PeriodKey::Month { year, month } => format!("month:{year:04}-{month:02}"),
            PeriodKey::Session { session_id, .. } => format!("session:{session_id}"),
```

In `crates/tagteam-engine/src/tokens/view/mod.rs`, replace:

```rust
        })
    }
}

impl Engine {
    /// Spec §10: the report `q` asks for, from the index and the evidence as they stand. It
    /// reads only: it never refreshes the index (spec §5) and never creates the store, without
    /// which the report is empty. A block report reports each block that meets the range
```

with:

```rust
        })
    }
}

/// Spec §11.1: how far back a report's range starts by default, for every period but `month`.
const DEFAULT_SINCE_MS: i64 = 30 * 86_400_000;

/// Spec §11.1's default `since_ms`, a function of the time and the zone alone: 30 days back, or
/// for `month` the first day of the eleventh month before `now_ms`'s, at midnight in `tz`, so
/// the report spans twelve calendar months with this one. `tagteam tokens` and the TUI both
/// take it (TUI spec §5.3).
pub fn default_since_ms(period: &TokenPeriod, now_ms: i64, tz: &TimeZone) -> i64 {
    if !matches!(period, TokenPeriod::Month) {
        return now_ms.saturating_sub(DEFAULT_SINCE_MS);
    }
    let (year, month) = month_of(now_ms, tz);
    let index = i32::from(year) * 12 + i32::from(month) - 1 - 11;
    let (year, month) = (index.div_euclid(12), index.rem_euclid(12) + 1);
    month_bounds_ms(year as i16, month as i8, tz).0
}

impl Engine {
    /// Spec §10: the report `q` asks for, from the index and the evidence as they stand. It
    /// reads only: it never refreshes the index (spec §5) and never creates the store, without
    /// which the report is empty. A block report reports each block that meets the range
```

In `crates/tagteam-engine/src/words.rs`, replace:

```rust
//! The words `list`, `status` and `history` print, shared with the TUI so that both say the
//! same things (TUI spec §5.2). They moved here from the CLI's `render.rs` and `history.rs`
//! with their bodies unchanged; the CLI adds its ANSI colours around them.

use tagteam_core::{Window, WindowKind};

use crate::store::AccountRow;
use crate::views::{AccountView, HistoryWindow, NO_DATA, UsageStatus, UsageView};

/// A window a reading does not have, or an age that is not known.
pub const MISSING: &str = "—";
```

with:

```rust
//! The words `list`, `status`, `history` and `tokens` print, shared with the TUI so that both
//! say the same things (TUI spec §5.2, §5.3). They moved here from the CLI's `render.rs`,
//! `history.rs` and `tokens_cmd.rs` with their bodies unchanged; the CLI adds its ANSI colours
//! around them.

use tagteam_core::{Window, WindowKind};

use crate::store::AccountRow;
use crate::tokens::TokenReport;
use crate::views::{AccountView, HistoryWindow, NO_DATA, UsageStatus, UsageView};

/// A window a reading does not have, or an age that is not known.
pub const MISSING: &str = "—";
```

In `crates/tagteam-engine/src/words.rs`, replace:

```rust
    };
    format!("{} pts/h · {eta}{method}", rate_signed(rate))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tagteam_core::{AccountId, CLAUDE_CODE, Pace, ProjectionMethod, ProviderId};
```

with:

```rust
    };
    format!("{} pts/h · {eta}{method}", rate_signed(rate))
}

/// What a token report with no rows says (token report §11.2).
pub const NO_TOKEN_USAGE: &str = "No token usage in this range.";

/// `1 noun`, or `n nouns`.
pub fn plural(n: u64, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// A token count as the report shows it (token report §11.2): as is below 1,000, else to one
/// decimal in thousands, millions, billions or trillions (`12.4M`), rounded half up.
pub fn scaled(n: u64) -> String {
    const UNITS: [(u128, char); 4] = [
        (1_000, 'K'),
        (1_000_000, 'M'),
        (1_000_000_000, 'B'),
        (1_000_000_000_000, 'T'),
    ];
    if n < 1_000 {
        return n.to_string();
    }
    let tenths = |unit: u128| (u128::from(n) * 10 + unit / 2) / unit;
    let (unit, suffix) = UNITS
        .iter()
        .copied()
        .find(|&(unit, _)| tenths(unit) < 10_000)
        .unwrap_or(UNITS[3]);
    let t = tenths(unit);
    format!("{}.{}{suffix}", t / 10, t % 10)
}

/// Nano-dollars as dollars and cents (`$312.40`), rounded half up.
pub fn dollars(nanos: u64) -> String {
    let cents = (u128::from(nanos) + 5_000_000) / 10_000_000;
    format!("${}.{:02}", cents / 100, cents % 100)
}

/// Transcript files the token index's last refresh could not read (token report §11.3).
pub fn unreadable_transcripts(n: u64) -> String {
    format!(
        "{} could not be read; the next refresh tries again",
        plural(n, "transcript file")
    )
}

/// The token report's footnotes (token report §11.2), one sentence each: unpriced models,
/// responses whose output is a lower bound (token report §2.3), skipped lines, the transcript
/// files the last refresh could not read when `unreadable` asks for them, and the price
/// table's date. `mark` is what marks a partial cost: `*` in the CLI, `≥` in the TUI (TUI spec
/// §5.3).
pub fn token_footnotes(report: &TokenReport, mark: &str, unreadable: bool) -> Vec<String> {
    let mut notes = Vec::new();
    if !report.unpriced_models.is_empty() {
        notes.push(format!(
            "No price for {}: counted without cost ({mark} marks a partial cost).",
            report.unpriced_models.join(", ")
        ));
    }
    if report.total.unfinalized > 0 {
        notes.push(format!(
            "Output is a lower bound for {}.",
            plural(report.total.unfinalized, "subagent response")
        ));
    }
    if report.index.skipped_lines > 0 {
        notes.push(format!(
            "{} could not be read, so these totals may be low.",
            plural(report.index.skipped_lines, "transcript line")
        ));
    }
    if unreadable && report.index.unreadable_files > 0 {
        notes.push(format!(
            "{}.",
            unreadable_transcripts(report.index.unreadable_files)
        ));
    }
    notes.push(if report.price_overrides.is_empty() {
        format!("API cost at list prices as of {}.", report.prices_as_of)
    } else {
        format!(
            "API cost at list prices as of {}, with config.toml's for {}.",
            report.prices_as_of,
            report.price_overrides.join(", ")
        )
    });
    notes
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tagteam_core::{AccountId, CLAUDE_CODE, Pace, ProjectionMethod, ProviderId};
```

In `crates/tagteam/src/tokens_cmd.rs`, replace:

```rust
use jiff::tz::TimeZone;
use serde_json::{Map, Value, json};
use tagteam_core::ProviderId;
use tagteam_core::time::{format_iso8601, parse_iso8601_ms};
use tagteam_core::tokens::{day_bounds_ms, month_bounds_ms, month_of};
use tagteam_engine::tokens::{
    ActiveBlock, Basis, Cost, IndexProgress, IndexState, PeriodKey, RowAccount, TokenCounts,
    TokenGrouping, TokenPeriod, TokenReport, TokenRow, TokenTotal, Wait,
};
use tagteam_engine::{Engine, EngineError};

use crate::cli::TokensArgs;
use crate::history;
use crate::render::{self, MISSING};

/// Spec §11.1: how far back `--since` defaults to, for every period but `month`.
const DEFAULT_SINCE_MS: i64 = 30 * 86_400_000;
/// Spec §11.2: a refresh shows progress only once it has run this long.
const PROGRESS_AFTER: Duration = Duration::from_secs(1);

const BAD_SINCE: &str =
```

with:

```rust
use jiff::tz::TimeZone;
use serde_json::{Map, Value, json};
use tagteam_core::ProviderId;
use tagteam_core::time::{format_iso8601, parse_iso8601_ms};
use tagteam_core::tokens::day_bounds_ms;
use tagteam_engine::tokens::{
    ActiveBlock, Basis, Cost, IndexProgress, IndexState, PeriodKey, RowAccount, TokenCounts,
    TokenGrouping, TokenPeriod, TokenReport, TokenRow, TokenTotal, Wait, default_since_ms,
};
use tagteam_engine::words::{self, dollars, plural, scaled};
use tagteam_engine::{Engine, EngineError};

use crate::cli::TokensArgs;
use crate::history;
use crate::render::{self, MISSING};

/// Spec §11.2: a refresh shows progress only once it has run this long.
const PROGRESS_AFTER: Duration = Duration::from_secs(1);

const BAD_SINCE: &str =
```

In `crates/tagteam/src/tokens_cmd.rs`, replace:

```rust
const BAD_UNTIL: &str =
    "--until takes a span like 30d, 12h or 90m, a date YYYY-MM-DD, or an ISO 8601 time";
const BAD_BY: &str = "--by takes account, model, account,model or none";
const PARTIAL_INDEX: &str = "the token index has not completed its first refresh, so this report is partial; run `tagteam tokens` without --no-refresh to complete it";
const NO_USAGE: &str = "No token usage in this range.\n";

/// What `tokens`' flags ask for (spec §11.1), each checked before the index is touched.
pub(crate) struct Flags {
    pub(crate) grouping: TokenGrouping,
```

with:

```rust
const BAD_UNTIL: &str =
    "--until takes a span like 30d, 12h or 90m, a date YYYY-MM-DD, or an ISO 8601 time";
const BAD_BY: &str = "--by takes account, model, account,model or none";
const PARTIAL_INDEX: &str = "the token index has not completed its first refresh, so this report is partial; run `tagteam tokens` without --no-refresh to complete it";

/// What `tokens`' flags ask for (spec §11.1), each checked before the index is touched.
pub(crate) struct Flags {
    pub(crate) grouping: TokenGrouping,
```

In `crates/tagteam/src/tokens_cmd.rs`, replace:

```rust
    let (year, month, day) = (part(0..4)?, part(5..7)?, part(8..10)?);
    jiff::civil::Date::new(year, i8::try_from(month).ok()?, i8::try_from(day).ok()?).ok()
}

/// Spec §11.1's default `--since`, a function of the time and the zone alone: 30 days back, or
/// for `month` the first day of the eleventh month before `now_ms`'s, at midnight in `tz`, so
/// the report spans twelve calendar months with this one.
pub(crate) fn default_since_ms(period: &TokenPeriod, now_ms: i64, tz: &TimeZone) -> i64 {
    if !matches!(period, TokenPeriod::Month) {
        return now_ms.saturating_sub(DEFAULT_SINCE_MS);
    }
    let (year, month) = month_of(now_ms, tz);
    let index = i32::from(year) * 12 + i32::from(month) - 1 - 11;
    let (year, month) = (index.div_euclid(12), index.rem_euclid(12) + 1);
    month_bounds_ms(year as i16, month as i8, tz).0
}

/// `--model` (spec §11.1): each pattern names every indexed model that contains it, ignoring
/// case, and the report takes those exact names. A pattern that names none is a usage error
/// that lists the indexed models.
pub(crate) fn resolve_models(
```

with:

```rust
    let (year, month, day) = (part(0..4)?, part(5..7)?, part(8..10)?);
    jiff::civil::Date::new(year, i8::try_from(month).ok()?, i8::try_from(day).ok()?).ok()
}

/// `--model` (spec §11.1): each pattern names every indexed model that contains it, ignoring
/// case, and the report takes those exact names. A pattern that names none is a usage error
/// that lists the indexed models.
pub(crate) fn resolve_models(
```

In `crates/tagteam/src/tokens_cmd.rs`, replace:

```rust
        ));
    }
    if index.unreadable_files > 0 {
        out.push(format!(
            "warning: {} could not be read; the next refresh tries again",
            plural(index.unreadable_files, "transcript file")
        ));
    }
    out
}

fn plural(n: u64, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// A time as JSON and CSV spell it: ISO 8601 UTC, to the second.
fn iso(ms: i64) -> String {
    format_iso8601(ms.div_euclid(1000))
}
```

with:

```rust
        ));
    }
    if index.unreadable_files > 0 {
        out.push(format!(
            "warning: {}",
            words::unreadable_transcripts(index.unreadable_files)
        ));
    }
    out
}

/// A time as JSON and CSV spell it: ISO 8601 UTC, to the second.
fn iso(ms: i64) -> String {
    format_iso8601(ms.div_euclid(1000))
}
```

In `crates/tagteam/src/tokens_cmd.rs`, replace:

```rust
/// Session rows add their first and last times and their project, block rows their end and a
/// note. Times are in the report's zone.
pub(crate) fn human(report: &TokenReport) -> String {
    if report.rows.is_empty() {
        return NO_USAGE.to_owned();
    }
    let q = &report.query;
    let by_model = report.rows.iter().any(|r| r.model.is_some());
    let mark = report
```

with:

```rust
/// Session rows add their first and last times and their project, block rows their end and a
/// note. Times are in the report's zone.
pub(crate) fn human(report: &TokenReport) -> String {
    if report.rows.is_empty() {
        return format!("{}\n", words::NO_TOKEN_USAGE);
    }
    let q = &report.query;
    let by_model = report.rows.iter().any(|r| r.model.is_some());
    let mark = report
```

In `crates/tagteam/src/tokens_cmd.rs`, replace:

```rust
        (false, false) => text,
    }
}

/// A token count as the table shows it (spec §11.2): as is below 1,000, else to one decimal
/// in thousands, millions, billions or trillions (`12.4M`), rounded half up.
pub(crate) fn scaled(n: u64) -> String {
    const UNITS: [(u128, char); 4] = [
        (1_000, 'K'),
        (1_000_000, 'M'),
        (1_000_000_000, 'B'),
        (1_000_000_000_000, 'T'),
    ];
    if n < 1_000 {
        return n.to_string();
    }
    let tenths = |unit: u128| (u128::from(n) * 10 + unit / 2) / unit;
    let (unit, suffix) = UNITS
        .iter()
        .copied()
        .find(|&(unit, _)| tenths(unit) < 10_000)
        .unwrap_or(UNITS[3]);
    let t = tenths(unit);
    format!("{}.{}{suffix}", t / 10, t % 10)
}

/// Nano-dollars as dollars and cents (`$312.40`), rounded half up.
pub(crate) fn dollars(nanos: u64) -> String {
    let cents = (u128::from(nanos) + 5_000_000) / 10_000_000;
    format!("${}.{:02}", cents / 100, cents % 100)
}

/// A block row's note (spec §11.2): `observed` for a block a reset time placed (spec §9), and
/// for the active block its time left, burn rate and projection.
pub(crate) fn block_note(observed: bool, active: Option<&ActiveBlock>) -> String {
    let mut parts = Vec::new();
```

with:

```rust
        (false, false) => text,
    }
}

/// A block row's note (spec §11.2): `observed` for a block a reset time placed (spec §9), and
/// for the active block its time left, burn rate and projection.
pub(crate) fn block_note(observed: bool, active: Option<&ActiveBlock>) -> String {
    let mut parts = Vec::new();
```

In `crates/tagteam/src/tokens_cmd.rs`, replace:

```rust
    parts.join("; ")
}

/// Spec §11.2's footnotes, after a blank line: unpriced models, responses whose output is a
/// lower bound (spec §2.3), skipped lines, and the date of the price table.
fn footnotes(report: &TokenReport) -> String {
    let mut notes = Vec::new();
    if !report.unpriced_models.is_empty() {
        notes.push(format!(
            "No price for {}: counted without cost (* marks a partial cost).",
            report.unpriced_models.join(", ")
        ));
    }
    if report.total.unfinalized > 0 {
        notes.push(format!(
            "Output is a lower bound for {}.",
            plural(report.total.unfinalized, "subagent response")
        ));
    }
    if report.index.skipped_lines > 0 {
        notes.push(format!(
            "{} could not be read, so these totals may be low.",
            plural(report.index.skipped_lines, "transcript line")
        ));
    }
    notes.push(if report.price_overrides.is_empty() {
        format!("API cost at list prices as of {}.", report.prices_as_of)
    } else {
        format!(
            "API cost at list prices as of {}, with config.toml's for {}.",
            report.prices_as_of,
            report.price_overrides.join(", ")
        )
    });
    format!("\n{}\n", notes.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_scale_to_one_decimal_from_a_thousand() {
        for (n, text) in [
            (0, "0"),
            (999, "999"),
            (1_000, "1.0K"),
            (1_234, "1.2K"),
            (1_250, "1.3K"),
            (51_000, "51.0K"),
            (999_949, "999.9K"),
            (999_950, "1.0M"),
            (12_400_000, "12.4M"),
            (3_100_000_000, "3.1B"),
            (999_950_000_000, "1.0T"),
            (u64::MAX, "18446744.1T"),
        ] {
            assert_eq!(scaled(n), text, "{n}");
        }
    }

    #[test]
    fn costs_are_dollars_and_cents_rounded_half_up() {
        assert_eq!(dollars(0), "$0.00");
        assert_eq!(dollars(4_999_999), "$0.00");
        assert_eq!(dollars(5_000_000), "$0.01");
        assert_eq!(dollars(622_016_000), "$0.62");
        assert_eq!(dollars(312_400_000_000), "$312.40");
        assert_eq!(dollars(u64::MAX), "$18446744073.71");
        let partial = Cost {
            usd_nanos: 97_000_000,
            partial: true,
        };
```

with:

```rust
    parts.join("; ")
}

/// Spec §11.2's footnotes, after a blank line: unpriced models, responses whose output is a
/// lower bound (spec §2.3), skipped lines, and the date of the price table. Unreadable files
/// are warned of on stderr instead (spec §11.3).
fn footnotes(report: &TokenReport) -> String {
    format!(
        "\n{}\n",
        words::token_footnotes(report, "*", false).join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_cost_is_starred_and_a_whole_one_keeps_the_cents_in_line() {
        let partial = Cost {
            usd_nanos: 97_000_000,
            partial: true,
        };
```

- [ ] **Step 4: Run the tests and see them pass**

Run:
- `cargo test -p tagteam-engine --lib -- words:: tokens::view::` — Expected: `18 passed` (17 in `words`, 1 in `tokens::view`).
- `cargo test -p tagteam --features test-support --lib tokens_cmd` — Expected: `8 passed`.
- `cargo test -p tagteam --features test-support --test tokens_cli --test run_tokens` — Expected: `11 passed` (`run_tokens`) and `13 passed` (`tokens_cli`): the CLI prints what it printed.
- The workspace suite, since the engine and the CLI are shared code: `cargo test --workspace --features tagteam/test-support --no-fail-fast` — Expected: 3641 of 3641 pass: 3559 in the sandbox, and outside it the 82 it refuses (`tagteam`'s library 10, `tests/daemon_cli.rs` 4, `tests/daemon_procs.rs` 5, `tests/signals.rs` 1, `tests/tui_pty.rs` 15, `tagteam-daemon`'s `client` 13, `server` 17 and `umask` 1, the engine's `tests/doctor.rs` 2, `xtask`'s compat tests 14).

- [ ] **Step 5: Format and lint**

Run:
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`

Expected: no output from the first; `Finished` and no warnings from the others.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/tokens/view/mod.rs crates/tagteam-engine/src/words.rs crates/tagteam/src/tokens_cmd.rs
git commit -m "Share the token report's words and default range between the CLI and the TUI"
```

### Task 2: `Source`'s token methods

Spec §4.4 gives `Source` three token methods: `refresh_tokens(provider, progress)`, `token_report(query)` and `token_index_state(provider)`. `EngineSource` implements them over the engine: the refresh always with `Wait::Try` and no budget (spec §9), and with the engine's own cancel token. The binary builds every lane's engine with the shutdown token, so a refresh stops at its next chunk when the TUI shuts down (B4). They are required methods, so part A's test sources implement them too. Their tests run over a real engine and FakeAgent's transcripts (B14).

**Readings of the spec this task commits to:**
- **A refresh returns what the engine returns**: `Done(IndexReport)`, `Busy`, or the interruption as a `SourceError` of kind `interrupted`. The lanes decide what to show (Task 4).
- **`tagteam-tui` takes `jiff`** (spec §4.1), the workspace's, for `TokenQuery`'s zone.

**Depends on:** —

**Files:**
- Modify: `Cargo.lock` (the build adds `jiff` to `tagteam-tui`'s entry)
- Modify: `crates/tagteam-tui/Cargo.toml`
- Modify: `crates/tagteam-tui/src/runtime.rs` (part A's test sources implement the new methods)
- Modify: `crates/tagteam-tui/src/source.rs`
- Modify: `crates/tagteam-tui/tests/source.rs`

**Interfaces:**
- Consumes: `tagteam_engine::tokens::{IndexProgress, IndexState, RefreshOutcome, TokenQuery, TokenReport, Wait}`; `Engine::{refresh_tokens, token_report, token_index_state, cancel}`; `tagteam_fake::{FakeUsage, append_transcript, usage_line_json}` in the tests.
- Produces: CB2 — `Source::{refresh_tokens, token_report, token_index_state}`, which Tasks 3, 4 and 6's lanes call.

**Spec:**
- TUI spec §4.4 (the three methods), §9 (`Wait::Try`, `budget: None`, a contended refresh returns `Busy` at once, a refresh cancelled by the shutdown token stops at its next chunk), §11 ("`Source` over a real engine").
- TR §5 (the refresh), §10 (the view), §10.1 (drill-down, and `RowKey`s stable across refreshes).

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tagteam_core::AccountId;
    use tagteam_engine::views::HistoryView;

    use super::*;
    use crate::host::{AccountRef, Ask};
```

with:

```rust
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tagteam_core::AccountId;
    use tagteam_engine::tokens::{
        IndexProgress, IndexState, RefreshOutcome, TokenQuery, TokenReport,
    };
    use tagteam_engine::views::HistoryView;

    use super::*;
    use crate::host::{AccountRef, Ask};
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
                fetched: 0,
                warnings: Vec::new(),
            })
        }
    }

    fn sources(delay: Duration, panics: bool, drops: &Arc<AtomicUsize>) -> SourceFactory {
        let drops = Arc::clone(drops);
```

with:

```rust
                fetched: 0,
                warnings: Vec::new(),
            })
        }

        fn refresh_tokens(
            &self,
            _provider: &ProviderId,
            _progress: &mut dyn FnMut(&IndexProgress),
        ) -> Result<RefreshOutcome, SourceError> {
            Ok(RefreshOutcome::Busy)
        }

        fn token_report(&self, _query: &TokenQuery) -> Result<TokenReport, SourceError> {
            Err(no_tokens())
        }

        fn token_index_state(&self, _provider: &ProviderId) -> Result<IndexState, SourceError> {
            Ok(IndexState::default())
        }
    }

    /// What a test source with no token report answers a report query with.
    fn no_tokens() -> SourceError {
        SourceError {
            kind: "test".into(),
            message: "no token report in this test".into(),
        }
    }

    fn sources(delay: Duration, panics: bool, drops: &Arc<AtomicUsize>) -> SourceFactory {
        let drops = Arc::clone(drops);
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
                fetched: 0,
                warnings: Vec::new(),
            })
        }
    }

    fn one_account(delay: Duration, frames: &Log) -> SourceFactory {
        let frames = frames.clone();
```

with:

```rust
                fetched: 0,
                warnings: Vec::new(),
            })
        }

        fn refresh_tokens(
            &self,
            _provider: &ProviderId,
            _progress: &mut dyn FnMut(&IndexProgress),
        ) -> Result<RefreshOutcome, SourceError> {
            Ok(RefreshOutcome::Busy)
        }

        fn token_report(&self, _query: &TokenQuery) -> Result<TokenReport, SourceError> {
            Err(no_tokens())
        }

        fn token_index_state(&self, _provider: &ProviderId) -> Result<IndexState, SourceError> {
            Ok(IndexState::default())
        }
    }

    fn one_account(delay: Duration, frames: &Log) -> SourceFactory {
        let frames = frames.clone();
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
                }
                fn collect_due(&self, _: &[AccountId]) -> Result<CollectSummary, SourceError> {
                    unreachable!("nothing is collected")
                }
            }
            Box::new(Signalled(signal.clone())) as Box<dyn Source>
        })
    }
```

with:

```rust
                }
                fn collect_due(&self, _: &[AccountId]) -> Result<CollectSummary, SourceError> {
                    unreachable!("nothing is collected")
                }
                fn refresh_tokens(
                    &self,
                    _: &ProviderId,
                    _: &mut dyn FnMut(&IndexProgress),
                ) -> Result<RefreshOutcome, SourceError> {
                    unreachable!("the token index is refreshed after the first frame")
                }
                fn token_report(&self, _: &TokenQuery) -> Result<TokenReport, SourceError> {
                    unreachable!("no token report is read")
                }
                fn token_index_state(&self, _: &ProviderId) -> Result<IndexState, SourceError> {
                    unreachable!("no token index state is read")
                }
            }
            Box::new(Signalled(signal.clone())) as Box<dyn Source>
        })
    }
```

In `crates/tagteam-tui/src/source.rs`, replace:

```rust
                fetched: 0,
                warnings: vec![self.0.into()],
            })
        }
    }

    #[test]
    fn a_factory_builds_a_source_in_the_thread_that_calls_it() {
```

with:

```rust
                fetched: 0,
                warnings: vec![self.0.into()],
            })
        }
        fn refresh_tokens(
            &self,
            _: &ProviderId,
            _: &mut dyn FnMut(&IndexProgress),
        ) -> Result<RefreshOutcome, SourceError> {
            Ok(RefreshOutcome::Busy)
        }
        fn token_report(&self, _: &TokenQuery) -> Result<TokenReport, SourceError> {
            Err(SourceError {
                kind: "token_report".into(),
                message: self.0.into(),
            })
        }
        fn token_index_state(&self, _: &ProviderId) -> Result<IndexState, SourceError> {
            Err(SourceError {
                kind: "token_index_state".into(),
                message: self.0.into(),
            })
        }
    }

    #[test]
    fn a_factory_builds_a_source_in_the_thread_that_calls_it() {
```

In `crates/tagteam-tui/tests/source.rs`, replace:

```rust
//! `EngineSource` over a real engine (spec §4.4, §11 "Source over a real engine"): a fixture
//! home with Claude Code and FakeAgent registered, a fake Keychain, a scripted HTTP port and a
//! fake clock. Nothing reaches the real HOME, the network or the Keychain (core §15.1).

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
```

with:

```rust
//! `EngineSource` over a real engine (spec §4.4, §11 "Source over a real engine"): a fixture
//! home with Claude Code and FakeAgent registered, a fake Keychain, a scripted HTTP port and a
//! fake clock, and FakeAgent's transcripts for the token report (§9). Nothing reaches the real
//! HOME, the network or the Keychain (core §15.1).

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
```

In `crates/tagteam-tui/tests/source.rs`, replace:

```rust
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
```

with:

```rust
use tagteam_engine::oracle::NoOracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::{Eligibility, NewAccount, Reserve, Store};
use tagteam_engine::tokens::{
    PeriodKey, RefreshOutcome, RowAccount, TokenGrouping, TokenPeriod, TokenQuery, tokens_lock_path,
};
use tagteam_engine::vault::{KeychainVault, SERVICE, Vault};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_fake::{FAKE_AGENT, FakeAgent, FakeUsage, append_transcript, usage_line_json};
use tagteam_provider::http::Method;
use tagteam_provider::liveness::FakeProcessProbe;
use tagteam_provider::process::ScriptedSpawner;
use tagteam_provider::{
    Cancel, Env, FakeClock, FakeKeychain, FlockGuard, Identity, ProfileMarker, RunShell,
    ScriptedHttp,
};
use tagteam_tui::source::{EngineSource, Source};

/// The fixture clock's start, in ms: every reading below is placed relative to it.
```

In `crates/tagteam-tui/tests/source.rs`, replace:

```rust
    }

    /// An engine as the binary builds one for `list` (`build_engine`), over the fixture's ports.
    fn engine_in(&self, run_shell: RunShell) -> Engine {
        Engine::new(EngineConfig {
            env: self.env.clone(),
            registry: ProviderRegistry::new()
                .with(Arc::new(ClaudeCode::new(self.kc.clone(), Platform::MacOs)))
                .with(Arc::new(FakeAgent::new())),
            vault: Vault::new(Box::new(KeychainVault::new(self.kc.clone()))),
```

with:

```rust
    }

    /// An engine as the binary builds one for `list` (`build_engine`), over the fixture's ports.
    fn engine_in(&self, run_shell: RunShell) -> Engine {
        self.engine_with(self.env.clone(), run_shell)
    }

    /// `engine_in`, over `env`.
    fn engine_with(&self, env: Env, run_shell: RunShell) -> Engine {
        Engine::new(EngineConfig {
            env,
            registry: ProviderRegistry::new()
                .with(Arc::new(ClaudeCode::new(self.kc.clone(), Platform::MacOs)))
                .with(Arc::new(FakeAgent::new())),
            vault: Vault::new(Box::new(KeychainVault::new(self.kc.clone()))),
```

In `crates/tagteam-tui/tests/source.rs`, replace:

```rust
    fn source(&self) -> EngineSource {
        EngineSource::new(self.engine_in(RunShell::Outside))
    }

    fn db(&self) -> PathBuf {
        self.env.data_dir().join("tagteam.db")
    }

```

with:

```rust
    fn source(&self) -> EngineSource {
        EngineSource::new(self.engine_in(RunShell::Outside))
    }

    /// A source whose engine takes `shutdown` as its cancel token, as every lane's does (§4.3).
    fn source_under(&self, shutdown: &Cancel) -> EngineSource {
        let mut env = self.env.clone();
        env.cancel = shutdown.clone();
        EngineSource::new(self.engine_with(env, RunShell::Outside))
    }

    /// `usage`, appended to FakeAgent's transcript `name`, a line each.
    fn transcript(&self, name: &str, usage: &[FakeUsage]) {
        let text: String = usage
            .iter()
            .map(|u| format!("{}\n", usage_line_json(u)))
            .collect();
        append_transcript(&self.env, name, &text);
    }

    fn db(&self) -> PathBuf {
        self.env.data_dir().join("tagteam.db")
    }

```

In `crates/tagteam-tui/tests/source.rs`, replace:

```rust
    // Nothing is due a moment later: a reading is fresh, a failure backs off.
    let again = source.collect_due(&[read, missing, fresh]).unwrap();
    assert_eq!(again.fetched, 0);
}
```

with:

```rust
    // Nothing is due a moment later: a reading is fresh, a failure backs off.
    let again = source.collect_due(&[read, missing, fresh]).unwrap();
    assert_eq!(again.fetched, 0);
}

fn fake() -> ProviderId {
    ProviderId::new(FAKE_AGENT)
}

/// FakeAgent's report by day and account, over all time, in UTC.
fn by_day() -> TokenQuery {
    TokenQuery {
        provider: fake(),
        period: TokenPeriod::Day,
        grouping: TokenGrouping::Account,
        since_ms: None,
        until_ms: None,
        accounts: None,
        models: None,
        session: None,
        tz: jiff::tz::TimeZone::UTC,
    }
}

/// A second response, a minute after the first, in session `session`.
fn another(session: &str) -> FakeUsage {
    FakeUsage {
        message_id: "m2".into(),
        request_id: "r2".into(),
        session_id: session.into(),
        at_ms: FakeUsage::default().at_ms + 60_000,
        ..FakeUsage::default()
    }
}

#[test]
fn the_index_refresh_reads_the_transcripts_and_the_report_shows_them() {
    let home = Home::new();
    home.transcript("s1.jsonl", &[FakeUsage::default(), another("s1")]);
    let source = home.source();
    let mut seen = Vec::new();
    let outcome = source
        .refresh_tokens(&fake(), &mut |p| seen.push(*p))
        .unwrap();
    let RefreshOutcome::Done(done) = outcome else {
        panic!("{outcome:?}");
    };
    assert!(done.complete);
    assert_eq!(done.responses_added, 2);
    assert_eq!(
        seen.last().map(|p| (p.files_done, p.files_total)),
        Some((1, 1)),
        "progress names the files"
    );
    let state = source.token_index_state(&fake()).unwrap();
    assert!(state.refreshed_at_ms.is_some());
    let report = source.token_report(&by_day()).unwrap();
    assert_eq!(report.index, state, "the report carries the index's state");
    assert_eq!(report.rows.len(), 1);
    let row = &report.rows[0];
    assert_eq!(row.account, Some(RowAccount::Unattributed));
    assert_eq!((row.responses, row.counts.total()), (2, 2 * 15));
}

#[test]
fn a_refresh_another_process_holds_is_busy_at_once() {
    // Spec §9: the TUI always tries (`Wait::Try`).
    let home = Home::new();
    home.transcript("s1.jsonl", &[FakeUsage::default()]);
    let _held = FlockGuard::try_lock(&tokens_lock_path(&home.env, &fake()))
        .unwrap()
        .unwrap();
    let source = home.source();
    let outcome = source.refresh_tokens(&fake(), &mut |_| {}).unwrap();
    assert_eq!(outcome, RefreshOutcome::Busy);
    let state = source.token_index_state(&fake()).unwrap();
    assert_eq!(
        (state.refreshed_at_ms, state.responses),
        (None, 0),
        "it read nothing"
    );
}

#[test]
fn a_refresh_under_the_shutdown_token_stops_and_says_it_was_interrupted() {
    // Spec §9: a refresh cancelled by the shutdown token stops at its next chunk.
    let home = Home::new();
    home.transcript("s1.jsonl", &[FakeUsage::default()]);
    let shutdown = Cancel::new();
    shutdown.request(2);
    let e = home
        .source_under(&shutdown)
        .refresh_tokens(&fake(), &mut |_| {})
        .unwrap_err();
    assert_eq!(e.kind, "interrupted");
    let state = home.source().token_index_state(&fake()).unwrap();
    assert_eq!(state.refreshed_at_ms, None, "no refresh completed");
}

#[test]
fn a_day_drills_to_its_sessions_and_a_session_to_its_models_whose_keys_hold() {
    // Spec §5.3: `TokenQuery::drill`, and `RowKey`s stable across a re-query.
    let home = Home::new();
    home.transcript("s1.jsonl", &[FakeUsage::default()]);
    home.transcript("s2.jsonl", &[another("s2")]);
    let source = home.source();
    source.refresh_tokens(&fake(), &mut |_| {}).unwrap();
    let days = source.token_report(&by_day()).unwrap();
    assert_eq!(days.rows.len(), 1);
    let sessions = source
        .token_report(&days.query.drill(&days.rows[0]).unwrap())
        .unwrap();
    let ids: Vec<&str> = sessions
        .rows
        .iter()
        .map(|r| match &r.period {
            PeriodKey::Session { session_id, .. } => session_id.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(ids, ["s2", "s1"], "newest first");
    let models = source
        .token_report(&sessions.query.drill(&sessions.rows[1]).unwrap())
        .unwrap();
    assert_eq!(models.rows.len(), 1);
    assert_eq!(models.rows[0].model.as_deref(), Some("fake-1"));
    assert_eq!(models.query.drill(&models.rows[0]), None, "the bottom");
    let again = source.token_report(&sessions.query).unwrap();
    let keys = |r: &tagteam_engine::tokens::TokenReport| {
        r.rows.iter().map(|row| row.key.clone()).collect::<Vec<_>>()
    };
    assert_eq!(keys(&again), keys(&sessions));
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p tagteam-tui`

Expected: FAIL: the build stops with ``error[E0407]: method `refresh_tokens` is not a member of trait `Source` `` (and the same for `token_report` and `token_index_state`), from part A's test sources and the new tests.

- [ ] **Step 3: Write the code**

`jiff` joins `tagteam-tui`'s dependencies; `cargo` adds it to the crate's entry in `Cargo.lock` on the next build.

In `Cargo.lock`, replace:

```toml
name = "tagteam-tui"
version = "20261014.2045.0"
dependencies = [
 "crossterm",
 "libc",
 "ratatui",
 "serde_json",
 "tagteam-cc",
```

with:

```toml
name = "tagteam-tui"
version = "20261014.2045.0"
dependencies = [
 "crossterm",
 "jiff",
 "libc",
 "ratatui",
 "serde_json",
 "tagteam-cc",
```

In `crates/tagteam-tui/Cargo.toml`, replace:

```toml
tagteam-core.workspace = true
tagteam-provider.workspace = true
tagteam-engine.workspace = true
crossterm.workspace = true
libc.workspace = true
ratatui.workspace = true
unicode-width.workspace = true

```

with:

```toml
tagteam-core.workspace = true
tagteam-provider.workspace = true
tagteam-engine.workspace = true
crossterm.workspace = true
jiff.workspace = true
libc.workspace = true
ratatui.workspace = true
unicode-width.workspace = true

```

In `crates/tagteam-tui/src/source.rs`, replace:

```rust
//! The read port (TUI spec §4.4): what the screens show, read from the engine on a lane's own
//! thread, never on the UI thread.

use std::sync::Arc;

use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::Engine;
use tagteam_engine::collect::{CollectMode, Collected};
use tagteam_engine::error::EngineError;
use tagteam_engine::views::{HistoryView, ProviderAccounts, ShellAccount, StatusView};
use tagteam_provider::RunShell;
/// A read that failed, in the CLI's own words (`kind` is the JSON `error.type`).
#[derive(Debug, Clone, PartialEq, Eq)]
```

with:

```rust
//! The read port (TUI spec §4.4): what the screens show, read from the engine on a lane's own
//! thread, never on the UI thread, and the token index's catch-up (§6.4).

use std::sync::Arc;

use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::Engine;
use tagteam_engine::collect::{CollectMode, Collected};
use tagteam_engine::error::EngineError;
use tagteam_engine::tokens::{
    IndexProgress, IndexState, RefreshOutcome, TokenQuery, TokenReport, Wait,
};
use tagteam_engine::views::{HistoryView, ProviderAccounts, ShellAccount, StatusView};
use tagteam_provider::RunShell;
/// A read that failed, in the CLI's own words (`kind` is the JSON `error.type`).
#[derive(Debug, Clone, PartialEq, Eq)]
```

In `crates/tagteam-tui/src/source.rs`, replace:

```rust
pub trait Source {
    fn accounts(&self, provider: Option<&ProviderId>) -> Result<AccountsSnapshot, SourceError>;
    fn history(&self, account: &AccountId, since_s: i64) -> Result<HistoryView, SourceError>;
    fn collect_due(&self, accounts: &[AccountId]) -> Result<CollectSummary, SourceError>;
}

/// Builds one `Source` inside the thread that will use it, so neither `Engine` nor the
/// binary's `Context` needs to be `Send` or `Sync`.
```

with:

```rust
pub trait Source {
    fn accounts(&self, provider: Option<&ProviderId>) -> Result<AccountsSnapshot, SourceError>;
    fn history(&self, account: &AccountId, since_s: i64) -> Result<HistoryView, SourceError>;
    fn collect_due(&self, accounts: &[AccountId]) -> Result<CollectSummary, SourceError>;
    /// The token index's catch-up (token report §5), on the index lane: `Busy` at once while
    /// another process refreshes, and stopped at its next chunk by the shutdown token (§9).
    fn refresh_tokens(
        &self,
        provider: &ProviderId,
        progress: &mut dyn FnMut(&IndexProgress),
    ) -> Result<RefreshOutcome, SourceError>;
    /// The token report (token report §10), on the report lane. It only reads.
    fn token_report(&self, query: &TokenQuery) -> Result<TokenReport, SourceError>;
    /// The index's state alone (token report §10): a few small reads, for deciding whether to
    /// query again (§6.4).
    fn token_index_state(&self, provider: &ProviderId) -> Result<IndexState, SourceError>;
}

/// Builds one `Source` inside the thread that will use it, so neither `Engine` nor the
/// binary's `Context` needs to be `Send` or `Sync`.
```

In `crates/tagteam-tui/src/source.rs`, replace:

```rust
            fetched: report.outcomes.iter().filter(|(_, c)| recorded(c)).count(),
            warnings: report.warnings,
        })
    }
}

/// Whether one account's collection changed what the views show (`Collected`): a reading was
/// recorded (`Recorded`), or a failure was (`Failed`, and `OverBudget`, which the store records
```

with:

```rust
            fetched: report.outcomes.iter().filter(|(_, c)| recorded(c)).count(),
            warnings: report.warnings,
        })
    }

    /// `Engine::refresh_tokens` as spec §9 has the TUI call it: `Wait::Try` and no budget, with
    /// the engine's own cancel token, which is the shutdown token (§4.3).
    fn refresh_tokens(
        &self,
        provider: &ProviderId,
        progress: &mut dyn FnMut(&IndexProgress),
    ) -> Result<RefreshOutcome, SourceError> {
        let engine = &self.engine;
        Ok(engine.refresh_tokens(provider, Wait::Try, None, engine.cancel(), progress)?)
    }

    fn token_report(&self, query: &TokenQuery) -> Result<TokenReport, SourceError> {
        Ok(self.engine.token_report(query)?)
    }

    fn token_index_state(&self, provider: &ProviderId) -> Result<IndexState, SourceError> {
        Ok(self.engine.token_index_state(provider)?)
    }
}

/// Whether one account's collection changed what the views show (`Collected`): a reading was
/// recorded (`Recorded`), or a failure was (`Failed`, and `OverBudget`, which the store records
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-tui`

Expected: `174 passed` (the library, unchanged) and `13 passed` (`tests/source.rs`, four of them new).

- [ ] **Step 5: Format and lint**

Run:
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`

Expected: no output from the first; `Finished` and no warnings from the others.

- [ ] **Step 6: Commit**

```bash
git add Cargo.lock crates/tagteam-tui/Cargo.toml crates/tagteam-tui/src/runtime.rs crates/tagteam-tui/src/source.rs crates/tagteam-tui/tests/source.rs
git commit -m "Read the token report and refresh its index through the TUI's read port"
```

### Task 3: The Tokens screen's query, drill-down and report lane

The Tokens screen's state and its keys, and the report lane that answers its queries (spec §4.3, §5.3), with no drawing yet: `render` stands in until Task 5.

- `TokensState` holds the settings (period, grouping, range and filter), the drill-down's levels (each with its query, its last report and its selection, kept by `RowKey`), and whether a query is on the lane.
- `load` asks for a new query under a new generation. `requery` asks for the same one, waiting for one already running (B2).
- `p`, `g` and `[`/`]` start again at the top (B8). `Enter` drills (`TokenQuery::drill`), and `Esc`/`Backspace` go back up.
- `2` and `Tab` open the screen, which reads its report again (B19).
- The binary gives the TUI its provider and zone (B6).

**Readings of the spec this task commits to:**
- **The top level's query is rebuilt at each query** from the settings, so its range ends now. A drilled level's query is the one `drill` gave, fixed.
- **`Enter` drills from the report the selected row came from** (`report.query.drill(row)`), so a drill never mixes a query with another query's rows. At a session's per-model rows `drill` returns `None`, and `Enter` does nothing.
- **A report applies to the shown level only when its generation is the newest.** The selection stays on its `RowKey`. When that row is gone, it moves to the row now at its place, or the last. An empty report keeps the key for the next one.
- **A failed report keeps the last one on screen.** Its words are the banner while Tokens is open, and the next good report clears it (B12).
- **The footer names what `Enter` opens:** `Enter sessions` from a period's row, `Enter models` from a session's, nothing at the bottom (B21). Task 4 adds `r` to it, and Task 6 `f`.
- **Every line of the Tokens help shows whole at 80×24** (R21, B27): each description fits the help box's 60 columns, and a test reads each one there. Tasks 4–6 add their keys to it.
- **Part A's tests change only where part B changes behaviour:**
  - `2` now opens Tokens;
  - the header's tabs read `1 Accounts  2 Tokens  3 History`;
  - the runtime's idle and panic tests count three joined lanes.

**Depends on:** 2

**Files:**
- Modify: `crates/tagteam-tui/src/testutil.rs` (`fresh` takes a `TokensState`; token report fixtures)
- Modify: `crates/tagteam-tui/src/screens/mod.rs`
- Create: `crates/tagteam-tui/src/screens/tokens/mod.rs`
- Modify: `crates/tagteam-tui/src/state.rs`
- Modify: `crates/tagteam-tui/src/msg.rs`
- Modify: `crates/tagteam-tui/src/update.rs`
- Modify: `crates/tagteam-tui/src/view.rs`
- Modify: `crates/tagteam-tui/src/actions.rs`
- Modify: `crates/tagteam-tui/src/runtime.rs`
- Modify: `crates/tagteam/src/tui_cmd.rs` (`TuiOptions`' two new fields (B6))

**Interfaces:**
- Consumes:
  - Task 2: `Source::token_report`.
  - The engine's `tokens::{AccountSel, RowKey, TokenGrouping, TokenPeriod, TokenQuery, TokenReport, TokenRow, default_since_ms}` (Task 1's `default_since_ms`).
  - Part A: `State`, `Screen`, `update::show`, `view::body_at`, `lanes::Lane`, `testutil`.
- Produces:
  - CB3: `ReportReq::Report`, `Msg::TokenReport`, `Cmd::LoadReport`.
  - CB4: `Screen::Tokens`, `State::tokens`, `State::new(screen, color, provider, tokens, now_ms)`, `Gens::report`.
  - CB5's `mod.rs`: `TokensState`, `Range`, `load`, `requery`, `on_enter`, `on_report`, `footer_keys`, `handle_key`, and a `render` that draws nothing until Task 5.
  - CB6: `TuiOptions::{tokens_provider, tz}` and the report lane.
  - `testutil`: `row_account`, `day`, `counts`, `token_row`, `token_query`, `token_report`.

**Spec:**
- TUI spec §5.3 (the query, its period, range, group and filter; the drill-down; the selection by `RowKey`), §5.6 (`2`, the Tokens keys), §4.3 (the report lane, coalescing, generations), §6.4 ("never on the UI thread").
- TR §10.1 (drill-down: a period's row to its sessions over its span, filtered by its account and model; a session's row to its models over its whole life), §11.1 (the default range).

- [ ] **Step 1: Write the failing tests**

`fresh` builds a `TokensState` for Claude Code in UTC. The fixtures build `TokenReport` values from the engine's own types (B14).

In `crates/tagteam-tui/src/testutil.rs`, replace:

```rust
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
```

with:

```rust
//! the engine's public types, keys, and a `TestBackend` read back as text.
#![allow(dead_code)]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use jiff::tz::TimeZone;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, CellWidth};
use tagteam_core::{AccountId, CLAUDE_CODE, Pace, ProviderId, Sample, Window, WindowKind};
use tagteam_engine::store::AccountRow;
use tagteam_engine::tokens::{
    Cost, IndexState, PeriodKey, RowAccount, RowKey, TokenCounts, TokenGrouping, TokenPeriod,
    TokenQuery, TokenReport, TokenRow, TokenTotal,
};
use tagteam_engine::views::{
    AccountView, HistoryView, HistoryWindow, ProviderAccounts, UsageStatus, UsageView,
};
use tagteam_provider::KindTraits;

use crate::msg::Msg;
use crate::screens::tokens::TokensState;
use crate::source::AccountsSnapshot;
use crate::state::{Screen, State};

/// Any fixed instant: every reading, reset and sample below is placed relative to it.
```

In `crates/tagteam-tui/src/testutil.rs`, replace:

```rust
        .map(|p| account(p, &format!("a{p}@x.co"), read(60, 10.0, 20.0, false)))
        .collect()
}

/// A State on `screen` at 80×24, without colour, nothing loaded yet.
pub(crate) fn fresh(screen: Screen) -> State {
    let mut state = State::new(screen, false, None, NOW_MS);
    state.size = (80, 24);
    state
}

```

with:

```rust
        .map(|p| account(p, &format!("a{p}@x.co"), read(60, 10.0, 20.0, false)))
        .collect()
}

/// A State on `screen` at 80×24, without colour, nothing loaded yet. Tokens reports on Claude
/// Code in UTC.
pub(crate) fn fresh(screen: Screen) -> State {
    let tokens = TokensState::new(cc(), TimeZone::UTC);
    let mut state = State::new(screen, false, None, tokens, NOW_MS);
    state.size = (80, 24);
    state
}

```

In `crates/tagteam-tui/src/testutil.rs`, replace:

```rust
        windows,
        unmatched_window: false,
    }
}
```

with:

```rust
        windows,
        unmatched_window: false,
    }
}

/// Fixture account `position` as a report row names it.
pub(crate) fn row_account(position: u32) -> RowAccount {
    RowAccount::Account {
        id: id(position),
        position,
        label: format!("a{position}@x.co"),
        alias: None,
    }
}

/// The day `days_ago` days before `NOW_MS`'s, in UTC.
pub(crate) fn day(days_ago: i64) -> PeriodKey {
    let at = jiff::Timestamp::from_millisecond(NOW_MS - days_ago * 24 * HOUR * 1000).unwrap();
    PeriodKey::Day {
        date: at.to_zoned(TimeZone::UTC).date(),
    }
}

/// `input` and `output` tokens, and `cache_read` read from the cache.
pub(crate) fn counts(input: u64, output: u64, cache_read: u64) -> TokenCounts {
    TokenCounts {
        input,
        output,
        cache_read,
        ..TokenCounts::default()
    }
}

/// A row of `period`, grouped by `account` and `model` when they are given, of one response
/// counting `counts`, costing `cost` nano-dollars (or nothing), keyed as the report keys it.
pub(crate) fn token_row(
    period: PeriodKey,
    account: Option<RowAccount>,
    model: Option<&str>,
    counts: TokenCounts,
    cost: Option<u64>,
) -> TokenRow {
    TokenRow {
        key: RowKey::new(&period, account.as_ref(), model),
        accounts: account.iter().cloned().collect(),
        models: model.iter().map(|m| (*m).to_owned()).collect(),
        period,
        account,
        model: model.map(str::to_owned),
        counts,
        responses: 1,
        unfinalized: 0,
        cost: cost.map(|usd_nanos| Cost {
            usd_nanos,
            partial: false,
        }),
    }
}

/// Claude Code's report by `period`, grouped by account, over the 30 days to `NOW_MS`, in UTC.
pub(crate) fn token_query(period: TokenPeriod) -> TokenQuery {
    TokenQuery {
        provider: cc(),
        period,
        grouping: TokenGrouping::Account,
        since_ms: Some(NOW_MS - 30 * 24 * HOUR * 1000),
        until_ms: None,
        accounts: None,
        models: None,
        session: None,
        tz: TimeZone::UTC,
    }
}

/// The report `query` asked for, holding `rows`: its total is theirs, and its index was
/// refreshed and last changed a minute before `NOW_MS`.
pub(crate) fn token_report(query: TokenQuery, rows: Vec<TokenRow>) -> TokenReport {
    let mut total = TokenTotal::default();
    for row in &rows {
        let (t, c) = (&mut total.counts, &row.counts);
        t.input += c.input;
        t.output += c.output;
        t.cache_write_5m += c.cache_write_5m;
        t.cache_write_1h += c.cache_write_1h;
        t.cache_read += c.cache_read;
        total.responses += row.responses;
        total.unfinalized += row.unfinalized;
        total.cost = match (total.cost, row.cost) {
            (None, cost) | (cost, None) => cost,
            (Some(a), Some(b)) => Some(Cost {
                usd_nanos: a.usd_nanos + b.usd_nanos,
                partial: a.partial || b.partial,
            }),
        };
    }
    TokenReport {
        query,
        time_zone: "UTC".into(),
        rows,
        total,
        index: IndexState {
            refreshed_at_ms: Some(NOW_MS - 60_000),
            changed_at_ms: Some(NOW_MS - 60_000),
            files: 3,
            responses: 3,
            ..IndexState::default()
        },
        unpriced_models: vec![],
        prices_as_of: "2026-10-01",
        price_overrides: vec![],
    }
}
```

Create `crates/tagteam-tui/src/screens/tokens/mod.rs` with its tests (Step 3 puts the code above them):

```rust
#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use tagteam_engine::tokens::PeriodKey;

    use super::*;
    use crate::msg::Msg;
    use crate::testutil::*;
    use crate::update::update;

    /// 2025-10-01T00:00:00Z: the first day of the eleventh month before `NOW_MS`'s.
    const OCTOBER_2025: i64 = 1_759_276_800_000;
    /// 2026-09-20T00:00:00Z, the day before `NOW_MS`'s.
    const YESTERDAY: i64 = 1_789_862_400_000;

    /// The query the one `LoadReport` in `cmds` asks for, and its generation.
    fn asked(cmds: &[Cmd]) -> (u64, TokenQuery) {
        match cmds {
            [Cmd::LoadReport { r#gen, query }] => (*r#gen, query.clone()),
            other => panic!("{other:?}"),
        }
    }

    /// Answers the query in `cmds` with `rows`, as the report lane would.
    fn answer(state: &mut State, cmds: &[Cmd], rows: Vec<TokenRow>) -> Vec<Cmd> {
        let (r#gen, query) = asked(cmds);
        let result = Ok(token_report(query, rows));
        update(state, Msg::TokenReport { r#gen, result }, NOW_MS)
    }

    fn row(period: PeriodKey, position: u32) -> TokenRow {
        token_row(
            period,
            Some(row_account(position)),
            None,
            counts(100, 10, 1_000),
            Some(1_000_000),
        )
    }

    fn session(id: &str, position: u32) -> TokenRow {
        let period = PeriodKey::Session {
            session_id: id.into(),
            project: Some("/w".into()),
            first_ms: YESTERDAY + 3_600_000,
            last_ms: YESTERDAY + 7_200_000,
        };
        row(period, position)
    }

    /// Tokens, opened, showing yesterday and the day before for accounts 1 and 2.
    fn showing_days() -> State {
        let mut state = fresh(Screen::Accounts);
        let cmds = update(&mut state, ch('2'), NOW_MS);
        answer(
            &mut state,
            &cmds,
            vec![row(day(1), 1), row(day(1), 2), row(day(2), 1)],
        );
        state
    }

    #[test]
    fn opening_tokens_reads_the_last_30_days_by_day_and_account() {
        // Spec §5.3: `tagteam tokens`'s defaults.
        let mut state = fresh(Screen::Accounts);
        let cmds = update(&mut state, ch('2'), NOW_MS);
        assert_eq!(
            cmds,
            [Cmd::LoadReport {
                r#gen: 1,
                query: token_query(TokenPeriod::Day),
            }]
        );
        assert!(state.tokens.pending());
        let mut started = fresh(Screen::Tokens);
        let cmds = started.start();
        assert!(
            matches!(
                cmds.as_slice(),
                [
                    Cmd::LoadAccounts { r#gen: 1 },
                    Cmd::LoadReport { r#gen: 1, .. }
                ]
            ),
            "{cmds:?}"
        );
    }

    #[test]
    fn the_period_cycles_and_a_month_report_spans_twelve_calendar_months() {
        let mut state = showing_days();
        let (_, month) = asked(&update(&mut state, ch('p'), NOW_MS));
        assert_eq!(
            (month.period, month.since_ms),
            (TokenPeriod::Month, Some(OCTOBER_2025)),
            "the first of the eleventh month back (token report §11.1)"
        );
        assert_eq!(state.tokens.range().label(TokenPeriod::Month), "12 months");
        let mut seen = vec![];
        for _ in 0..3 {
            let (_, q) = asked(&update(&mut state, ch('p'), NOW_MS));
            seen.push((q.period, q.since_ms));
        }
        assert_eq!(
            seen,
            [
                (TokenPeriod::Session, Some(NOW_MS - 30 * DAY_MS)),
                (TokenPeriod::Block, Some(NOW_MS - 30 * DAY_MS)),
                (TokenPeriod::Day, Some(NOW_MS - 30 * DAY_MS)),
            ]
        );
    }

    #[test]
    fn the_range_cycles_through_7_30_and_90_days_and_all() {
        let mut state = showing_days();
        let mut seen = vec![];
        for _ in 0..4 {
            let (_, q) = asked(&update(&mut state, ch(']'), NOW_MS));
            seen.push((q.since_ms, q.until_ms));
        }
        assert_eq!(
            seen,
            [
                (Some(NOW_MS - 90 * DAY_MS), None),
                (None, None),
                (Some(NOW_MS - 7 * DAY_MS), None),
                (Some(NOW_MS - 30 * DAY_MS), None),
            ]
        );
        let (_, q) = asked(&update(&mut state, ch('['), NOW_MS));
        assert_eq!(q.since_ms, Some(NOW_MS - 7 * DAY_MS));
        assert_eq!(state.tokens.range().label(TokenPeriod::Day), "7 days");
    }

    #[test]
    fn the_grouping_cycles_and_a_new_query_starts_over_at_the_top() {
        let mut state = showing_days();
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer(&mut state, &down, vec![session("s1", 1)]);
        assert_eq!(state.tokens.depth(), 1);
        let mut seen = vec![];
        for _ in 0..4 {
            let (_, q) = asked(&update(&mut state, ch('g'), NOW_MS));
            seen.push(q.grouping);
            assert_eq!((q.period, q.session), (TokenPeriod::Day, None));
        }
        assert_eq!(
            seen,
            [
                TokenGrouping::Model,
                TokenGrouping::AccountModel,
                TokenGrouping::None,
                TokenGrouping::Account,
            ]
        );
        assert_eq!(state.tokens.depth(), 0, "back at the top (B8)");
        assert!(state.tokens.report().is_none(), "`loading…` until it comes");
    }

    #[test]
    fn enter_goes_from_a_day_to_its_sessions_and_from_a_session_to_its_models_and_no_further() {
        let mut state = showing_days();
        assert_eq!(state.tokens.drill_to(), Some("sessions"));
        let (_, sessions) = asked(&update(&mut state, key(KeyCode::Enter), NOW_MS));
        assert_eq!(sessions.period, TokenPeriod::Session);
        assert_eq!(
            (sessions.since_ms, sessions.until_ms),
            (Some(YESTERDAY), Some(YESTERDAY + DAY_MS)),
            "the day's span"
        );
        assert_eq!(
            sessions.accounts,
            Some(vec![AccountSel::Account(id(1))]),
            "the row's account"
        );
        assert!(state.tokens.report().is_none(), "`loading…`");
        let asked_for = [Cmd::LoadReport {
            r#gen: state.gens.report,
            query: sessions,
        }];
        answer(&mut state, &asked_for, vec![session("s1", 1)]);
        assert_eq!(state.tokens.drill_to(), Some("models"));
        let (_, models) = asked(&update(&mut state, key(KeyCode::Enter), NOW_MS));
        assert_eq!(
            (models.session.as_deref(), models.grouping, models.since_ms),
            (Some("s1"), TokenGrouping::AccountModel, None),
            "the whole session, by account and model"
        );
        let per_model = PeriodKey::Session {
            session_id: "s1".into(),
            project: Some("/w".into()),
            first_ms: YESTERDAY + 3_600_000,
            last_ms: YESTERDAY + 7_200_000,
        };
        let opus = token_row(
            per_model,
            Some(row_account(1)),
            Some("claude-opus-5-5"),
            counts(1, 2, 3),
            None,
        );
        let asked_for = [Cmd::LoadReport {
            r#gen: state.gens.report,
            query: models,
        }];
        answer(&mut state, &asked_for, vec![opus]);
        assert_eq!(state.tokens.depth(), 2);
        assert_eq!(state.tokens.drill_to(), None);
        assert!(
            update(&mut state, key(KeyCode::Enter), NOW_MS).is_empty(),
            "Enter does nothing at the bottom"
        );
        assert_eq!(state.tokens.depth(), 2);
    }

    #[test]
    fn esc_and_backspace_go_back_a_level_to_its_report_and_selection() {
        let mut state = showing_days();
        update(&mut state, ch('j'), NOW_MS);
        let selected = state.tokens.selected().cloned();
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer(&mut state, &down, vec![session("s1", 2)]);
        let back = update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert_eq!(state.tokens.depth(), 0);
        assert_eq!(state.tokens.selected(), selected.as_ref());
        assert_eq!(
            state.tokens.report().map(|r| r.rows.len()),
            Some(3),
            "shown at once, while it is read again"
        );
        let (_, q) = asked(&back);
        assert_eq!(q, token_query(TokenPeriod::Day));
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer(&mut state, &down, vec![session("s1", 2)]);
        update(&mut state, key(KeyCode::Backspace), NOW_MS);
        assert_eq!(state.tokens.depth(), 0);
        assert!(
            update(&mut state, key(KeyCode::Esc), NOW_MS).is_empty(),
            "the top has no level above it"
        );
    }

    #[test]
    fn the_selection_follows_its_row_key_across_a_refresh() {
        // Spec §5.3: a refresh that adds a row above the selected one keeps the selection on
        // the same row; when that row is gone, the row now at its place is selected.
        let mut state = showing_days();
        update(&mut state, ch('j'), NOW_MS);
        let key2 = row(day(1), 2).key;
        assert_eq!(state.tokens.selected(), Some(&key2));
        let again = on_enter(&mut state, NOW_MS);
        answer(
            &mut state,
            &again,
            vec![
                row(day(0), 1),
                row(day(1), 1),
                row(day(1), 2),
                row(day(2), 1),
            ],
        );
        assert_eq!(state.tokens.selected(), Some(&key2));
        assert_eq!(state.tokens.selected_index(), Some(2));
        let again = on_enter(&mut state, NOW_MS);
        answer(
            &mut state,
            &again,
            vec![row(day(0), 1), row(day(1), 1), row(day(2), 1)],
        );
        assert_eq!(state.tokens.selected(), Some(&row(day(2), 1).key));
        let again = on_enter(&mut state, NOW_MS);
        answer(&mut state, &again, vec![]);
        assert_eq!(
            state.tokens.selected(),
            Some(&row(day(2), 1).key),
            "an empty report keeps the key for the next"
        );
    }

    #[test]
    fn j_and_k_move_the_selection_and_stop_at_either_end() {
        let mut state = showing_days();
        let keys = |state: &State| state.tokens.selected().cloned();
        assert_eq!(keys(&state), Some(row(day(1), 1).key));
        for _ in 0..5 {
            update(&mut state, ch('j'), NOW_MS);
        }
        assert_eq!(keys(&state), Some(row(day(2), 1).key));
        update(&mut state, key(KeyCode::Home), NOW_MS);
        assert_eq!(keys(&state), Some(row(day(1), 1).key));
        update(&mut state, ch('k'), NOW_MS);
        assert_eq!(keys(&state), Some(row(day(1), 1).key));
        update(&mut state, key(KeyCode::End), NOW_MS);
        assert_eq!(keys(&state), Some(row(day(2), 1).key));
    }

    #[test]
    fn a_report_of_an_older_generation_is_dropped() {
        // Spec §4.3: a slow query never overwrites a newer one.
        let mut state = showing_days();
        let older = update(&mut state, ch('p'), NOW_MS);
        let newer = update(&mut state, ch('p'), NOW_MS);
        let month = PeriodKey::Month {
            year: 2026,
            month: 9,
        };
        assert!(answer(&mut state, &older, vec![row(month, 1)]).is_empty());
        assert!(state.tokens.report().is_none(), "the month report is stale");
        assert!(
            state.tokens.pending(),
            "the session report is still to come"
        );
        answer(&mut state, &newer, vec![session("s1", 1)]);
        assert_eq!(
            state.tokens.report().map(|r| r.query.period),
            Some(TokenPeriod::Session)
        );
    }

    #[test]
    fn a_query_asked_for_while_one_runs_follows_it_once() {
        // B2: the slow query is let finish, then the screen is read once more.
        let mut state = showing_days();
        let first = on_enter(&mut state, NOW_MS);
        assert!(on_enter(&mut state, NOW_MS).is_empty());
        assert!(on_enter(&mut state, NOW_MS).is_empty());
        let follow = answer(&mut state, &first, vec![row(day(1), 1)]);
        let (r#gen, _) = asked(&follow);
        assert_eq!(r#gen, state.gens.report);
        assert!(answer(&mut state, &follow, vec![row(day(1), 1)]).is_empty());
        assert!(!state.tokens.pending());
    }

    #[test]
    fn a_failed_report_keeps_the_last_one_under_a_banner_shown_on_tokens_only() {
        let mut state = showing_days();
        let again = on_enter(&mut state, NOW_MS);
        let (r#gen, _) = asked(&again);
        let failed = Err(SourceError {
            kind: "store".into(),
            message: "the store is locked".into(),
        });
        update(
            &mut state,
            Msg::TokenReport {
                r#gen,
                result: failed,
            },
            NOW_MS,
        );
        assert_eq!(state.tokens.report().map(|r| r.rows.len()), Some(3));
        assert_eq!(draw(&state, 80, 24)[1], " the store is locked");
        update(&mut state, ch('1'), NOW_MS);
        assert!(
            !draw(&state, 80, 24)[1].contains("the store is locked"),
            "a token read's failure is Tokens' own"
        );
        let again = update(&mut state, ch('2'), NOW_MS);
        answer(&mut state, &again, vec![row(day(1), 1)]);
        assert_eq!(state.tokens.error(), None, "the next good read clears it");
    }

    #[test]
    fn help_on_tokens_shows_each_key_whole_at_80_columns() {
        // R21: the help box is 60 columns inside at 80; no description runs past it.
        let mut state = showing_days();
        update(&mut state, ch('?'), NOW_MS);
        let lines = draw(&state, 80, 24);
        for what in [
            "period: day, month, session or block",
            "group: none, account, model, or both",
            "range: 7, 30 or 90 days, or all",
            "drill down: sessions, then models",
            "back up a level",
        ] {
            assert!(lines.iter().any(|l| l.contains(what)), "{what}: {lines:?}");
        }
    }

    #[test]
    fn the_footer_says_what_enter_opens() {
        let mut state = showing_days();
        assert_eq!(
            draw(&state, 80, 24)[23],
            "Enter sessions  p period  g group  [ ] range  Esc back"
        );
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer(&mut state, &down, vec![session("s1", 1)]);
        assert!(draw(&state, 80, 24)[23].starts_with("Enter models  p period"));
    }
}
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
        assert!(!accounts(1).same_as(&history(1, "a")));
        assert!(!history(1, "a").same_as(&accounts(1)));
    }

    #[test]
    fn collections_coalesce_and_actions_never_do() {
        let collect = |g| ActionsReq::Collect {
            r#gen: g,
```

with:

```rust
        assert!(!accounts(1).same_as(&history(1, "a")));
        assert!(!history(1, "a").same_as(&accounts(1)));
    }

    #[test]
    fn report_queries_coalesce_whatever_they_ask() {
        let query = |days: i64| TokenQuery {
            provider: ProviderId::new("claude-code"),
            period: tagteam_engine::tokens::TokenPeriod::Day,
            grouping: tagteam_engine::tokens::TokenGrouping::Account,
            since_ms: Some(-days),
            until_ms: None,
            accounts: None,
            models: None,
            session: None,
            tz: jiff::tz::TimeZone::UTC,
        };
        let report = |g, days| ReportReq::Report {
            r#gen: g,
            query: query(days),
        };
        assert!(report(1, 7).same_as(&report(2, 30)));
    }

    #[test]
    fn collections_coalesce_and_actions_never_do() {
        let collect = |g| ActionsReq::Collect {
            r#gen: g,
```

In `crates/tagteam-tui/src/update.rs`, replace:

```rust
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
```

with:

```rust
        }
    }

    #[test]
    fn the_screen_keys_switch_screens_in_their_numbers_order() {
        let mut state = fresh(Screen::Accounts);
        update(&mut state, ch('3'), NOW_MS);
        assert_eq!(state.screen, Screen::History);
        update(&mut state, ch('2'), NOW_MS);
        assert_eq!(state.screen, Screen::Tokens);
        update(&mut state, ch('1'), NOW_MS);
        assert_eq!(state.screen, Screen::Accounts);
        let back_tab = || Msg::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        for screen in [Screen::Tokens, Screen::History, Screen::Accounts] {
            update(&mut state, key(KeyCode::Tab), NOW_MS);
            assert_eq!(state.screen, screen);
        }
        for screen in [Screen::History, Screen::Tokens, Screen::Accounts] {
            update(&mut state, back_tab(), NOW_MS);
            assert_eq!(state.screen, screen);
        }
    }

    #[test]
    fn q_and_ctrl_c_quit_with_0_and_ctrl_z_stops() {
```

In `crates/tagteam-tui/src/update.rs`, replace:

```rust
                        [Cmd::LoadAccounts { .. }, Cmd::LoadHistory { .. }]
                    ),
                    "{cmds:?}"
                ),
            }
            assert!(state.toast.is_none() && state.banner.is_none());
            // A failure of the older run says nothing and reads nothing.
            let failure = Msg::Collected {
```

with:

```rust
                        [Cmd::LoadAccounts { .. }, Cmd::LoadHistory { .. }]
                    ),
                    "{cmds:?}"
                ),
                Screen::Tokens => unreachable!("not among the screens this test opens"),
            }
            assert!(state.toast.is_none() && state.banner.is_none());
            // A failure of the older run says nothing and reads nothing.
            let failure = Msg::Collected {
```

In `crates/tagteam-tui/src/view.rs`, replace:

```rust
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
```

with:

```rust
    #[test]
    fn the_header_shows_the_tabs_and_the_mode_and_the_footer_the_screens_keys() {
        let state = fresh(Screen::Accounts);
        let lines = draw(&state, 80, 24);
        // 43 columns of tabs, 14 of gap, and the mode and `? help` up to the last column.
        assert_eq!(
            lines[0],
            format!(
                " tagteam  [1 Accounts]  2 Tokens  3 History{}○ refresh 30s   ? help",
                " ".repeat(14)
            )
        );
        assert_eq!(
            lines[23],
```

In `crates/tagteam-tui/src/view.rs`, replace:

```rust
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
```

with:

```rust
        let history = draw(&fresh(Screen::History), 80, 24);
        assert_eq!(
            history[0],
            format!(
                " tagteam  1 Accounts  2 Tokens  [3 History]{}○ refresh 30s   ? help",
                " ".repeat(14)
            )
        );
        assert_eq!(history[23], "j/k account  [ ] range  r refresh  1 accounts");
        let tokens = draw(&fresh(Screen::Tokens), 80, 24);
        assert_eq!(
            tokens[0],
            format!(
                " tagteam  1 Accounts  [2 Tokens]  3 History{}○ refresh 30s   ? help",
                " ".repeat(14)
            )
        );
        assert_eq!(tokens[23], "p period  g group  [ ] range  Esc back");
    }

    #[test]
    fn below_80_by_24_only_the_notice_is_drawn() {
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
        TuiOptions {
            screen: Screen::Accounts,
            color: false,
            provider: None,
            signal: Cancel::new(),
            shutdown: Cancel::new(),
        }
    }
```

with:

```rust
        TuiOptions {
            screen: Screen::Accounts,
            color: false,
            provider: None,
            tokens_provider: ProviderId::new("claude-code"),
            tz: TimeZone::UTC,
            signal: Cancel::new(),
            shutdown: Cancel::new(),
        }
    }
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
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
```

with:

```rust
        );
        assert_eq!((exit.code, exit.report), (0, None));
        assert_eq!(
            drops.load(Ordering::SeqCst),
            3,
            "every lane ended before the TUI returned"
        );
        assert_eq!(
            shutdown.requested(),
            Some(libc::SIGINT),
            "§10.2 step 1: SIGINT for q"
        );
    }

    #[test]
    fn tokens_reads_its_report_on_the_report_lane() {
        let opts = TuiOptions {
            screen: Screen::Tokens,
            ..options()
        };
        let fx = Fixture::with(opts, quick(), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            // The first frame waited for the accounts only; the report may come after it.
            while ui.state.tokens.error().is_none() {
                let msg = fx.rx.recv_timeout(Duration::from_secs(5));
                ui.deliver(msg.expect("the report lane answered"));
            }
            assert_eq!(
                ui.state.tokens.error(),
                Some("no token report in this test")
            );
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn q_while_an_action_runs_waits_for_it_and_reports_what_it_did() {
        const SPAN: Duration = Duration::from_millis(300);
        const SWITCHED: &str = "Switched to a@x.co (position 1).\n";
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
            |_tx| Ok(Keys::default()),
            || {},
        );
        assert_eq!((exit.code, exit.report), (1, None));
        assert_eq!(drops.load(Ordering::SeqCst), 2, "both lanes were joined");
        assert_eq!(shutdown.requested(), Some(libc::SIGINT));
    }
}
```

with:

```rust
            |_tx| Ok(Keys::default()),
            || {},
        );
        assert_eq!((exit.code, exit.report), (1, None));
        assert_eq!(drops.load(Ordering::SeqCst), 3, "every lane was joined");
        assert_eq!(shutdown.requested(), Some(libc::SIGINT));
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p tagteam-tui --lib`

Expected: FAIL: the build stops with errors such as ``error[E0432]: unresolved import `crate::screens::tokens` ``, ``error[E0433]: failed to resolve: use of undeclared type `TimeZone` `` and ``error[E0560]: struct `runtime::TuiOptions` has no field named `tokens_provider` ``.

- [ ] **Step 3: Write the code**

The screen's module, then the state, messages, update, view, help and runtime that carry it, and the binary's two new options.

In `crates/tagteam-tui/src/screens/mod.rs`, replace:

```rust
//! The screens (spec §5): each with its state, its keys and its renderer.

pub mod accounts;
pub mod history;
```

with:

```rust
//! The screens (spec §5): each with its state, its keys and its renderer.

pub mod accounts;
pub mod history;
pub mod tokens;
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, put this above the tests, with one blank line between:

```rust
//! The Tokens screen (spec §5.3): the token report (token report §10) by day, month, session or
//! 5-hour block, grouped and filtered, with drill-down from a period to its sessions and from a
//! session to its models. Every query runs on the report lane (§4.3), never on the UI thread.

use crossterm::event::{KeyCode, KeyEvent};
use jiff::tz::TimeZone;
use ratatui::Frame;
use ratatui::layout::Rect;
use tagteam_core::ProviderId;
use tagteam_engine::tokens::{
    AccountSel, RowKey, TokenGrouping, TokenPeriod, TokenQuery, TokenReport, TokenRow,
    default_since_ms,
};

use crate::msg::Cmd;
use crate::source::SourceError;
use crate::state::{Screen, State};

const DAY_MS: i64 = 86_400_000;

/// The ranges `[` and `]` cycle through (spec §5.3, B9): 7 days, the default (30 days, or for
/// `Month` the twelve calendar months to this one, token report §11.1), 90 days, or all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Range {
    Week,
    #[default]
    Default,
    Quarter,
    All,
}

impl Range {
    const ALL: [Range; 4] = [Range::Week, Range::Default, Range::Quarter, Range::All];

    fn step(self, by: isize) -> Range {
        let at = Self::ALL.iter().position(|r| *r == self).unwrap_or(1) as isize;
        Self::ALL[(at + by).rem_euclid(Self::ALL.len() as isize) as usize]
    }

    /// Where the range starts at `now_ms` for `period`, in `tz`; `None` for all.
    fn since_ms(self, period: TokenPeriod, now_ms: i64, tz: &TimeZone) -> Option<i64> {
        match self {
            Range::Week => Some(now_ms.saturating_sub(7 * DAY_MS)),
            Range::Default => Some(default_since_ms(&period, now_ms, tz)),
            Range::Quarter => Some(now_ms.saturating_sub(90 * DAY_MS)),
            Range::All => None,
        }
    }

    /// The header's words for the range of a `period` report.
    pub fn label(self, period: TokenPeriod) -> &'static str {
        match (self, period) {
            (Range::Week, _) => "7 days",
            (Range::Default, TokenPeriod::Month) => "12 months",
            (Range::Default, _) => "30 days",
            (Range::Quarter, _) => "90 days",
            (Range::All, _) => "all",
        }
    }
}

/// One level of the drill-down (spec §5.3): its query, the report last read for it, and the
/// selected row, kept by `RowKey`.
struct Level {
    /// `None` at the top, whose query is built from the settings each time it is read, so its
    /// range rolls with the clock; a drilled level's is `TokenQuery::drill`'s, fixed.
    query: Option<TokenQuery>,
    report: Option<TokenReport>,
    selected: Option<RowKey>,
}

impl Level {
    fn top() -> Level {
        Level {
            query: None,
            report: None,
            selected: None,
        }
    }
}

/// What the Tokens screen shows and asks for.
pub struct TokensState {
    /// `--provider`'s, else the default provider (spec §5.3).
    provider: ProviderId,
    /// The system zone (spec §5.3), read before the TUI started (B6).
    tz: TimeZone,
    period: TokenPeriod,
    grouping: TokenGrouping,
    range: Range,
    /// The filter (spec §5.3): `None` for every one of a kind.
    accounts: Option<Vec<AccountSel>>,
    models: Option<Vec<String>>,
    /// The top level first; never empty.
    levels: Vec<Level>,
    /// A query of the shown level is on the report lane.
    pending: bool,
    /// Asked to query again while `pending`: done once that query returns (B2).
    again: bool,
    /// A failed read's words, the banner while Tokens is open (spec §10.4, B12).
    error: Option<String>,
}

impl TokensState {
    /// By day and account over the default range, unfiltered (spec §5.3; `tagteam tokens`'s
    /// defaults).
    pub fn new(provider: ProviderId, tz: TimeZone) -> TokensState {
        TokensState {
            provider,
            tz,
            period: TokenPeriod::Day,
            grouping: TokenGrouping::Account,
            range: Range::Default,
            accounts: None,
            models: None,
            levels: vec![Level::top()],
            pending: false,
            again: false,
            error: None,
        }
    }

    pub fn provider(&self) -> &ProviderId {
        &self.provider
    }

    pub fn period(&self) -> TokenPeriod {
        self.period
    }

    pub fn grouping(&self) -> TokenGrouping {
        self.grouping
    }

    pub fn range(&self) -> Range {
        self.range
    }

    /// The report shown: the last one read for the shown level.
    pub fn report(&self) -> Option<&TokenReport> {
        self.level().report.as_ref()
    }

    /// The selected row's key, kept across refreshes (spec §5.3).
    pub fn selected(&self) -> Option<&RowKey> {
        self.level().selected.as_ref()
    }

    /// How deep the drill-down is: 0 at the top.
    pub fn depth(&self) -> usize {
        self.levels.len() - 1
    }

    /// Whether a query of the shown level is on the report lane.
    pub fn pending(&self) -> bool {
        self.pending
    }

    /// A failed read's words, for the banner (B12).
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The shown level's query at `now_ms`: the top's from the settings, its range ending now;
    /// a drilled level's as it was drilled.
    pub fn query(&self, now_ms: i64) -> TokenQuery {
        self.level()
            .query
            .clone()
            .unwrap_or_else(|| self.top_query(now_ms))
    }

    fn top_query(&self, now_ms: i64) -> TokenQuery {
        TokenQuery {
            provider: self.provider.clone(),
            period: self.period,
            grouping: self.grouping,
            since_ms: self.range.since_ms(self.period, now_ms, &self.tz),
            until_ms: None,
            accounts: self.accounts.clone(),
            models: self.models.clone(),
            session: None,
            tz: self.tz.clone(),
        }
    }

    fn level(&self) -> &Level {
        self.levels.last().expect("the top level is never left")
    }

    fn level_mut(&mut self) -> &mut Level {
        self.levels.last_mut().expect("the top level is never left")
    }

    /// The selected row of the shown report.
    pub fn selected_row(&self) -> Option<&TokenRow> {
        let level = self.level();
        let key = level.selected.as_ref()?;
        level.report.as_ref()?.rows.iter().find(|r| &r.key == key)
    }

    /// What `Enter` opens from the selected row: `sessions` from a period's row, `models` from
    /// a session's, and nothing from a session's per-model rows (spec §5.3).
    pub fn drill_to(&self) -> Option<&'static str> {
        let query = self.report()?.query.drill(self.selected_row()?)?;
        Some(if query.session.is_some() {
            "models"
        } else {
            "sessions"
        })
    }

    /// The selected row's place in the shown report.
    pub fn selected_index(&self) -> Option<usize> {
        let level = self.level();
        let key = level.selected.as_ref()?;
        level
            .report
            .as_ref()?
            .rows
            .iter()
            .position(|r| &r.key == key)
    }

    /// Takes a new report for the shown level. The selection stays on its `RowKey`; when that
    /// row is gone it moves to the row now at its old place, or the last row (spec §5.3). A
    /// report with no rows keeps the key, for the next one that has the row.
    pub fn apply(&mut self, report: TokenReport) {
        let old = self.selected_index();
        let level = self.level_mut();
        let kept = level
            .selected
            .as_ref()
            .is_some_and(|k| report.rows.iter().any(|r| &r.key == k));
        if !kept && !report.rows.is_empty() {
            let at = old.unwrap_or(0).min(report.rows.len() - 1);
            level.selected = Some(report.rows[at].key.clone());
        }
        level.report = Some(report);
    }

    /// Moves the selection `by` rows, stopping at either end.
    fn step(&mut self, by: isize) {
        let at = self.selected_index().unwrap_or(0) as isize;
        let level = self.level_mut();
        let Some(rows) = level.report.as_ref().map(|r| &r.rows) else {
            return;
        };
        if rows.is_empty() {
            return;
        }
        let to = (at.saturating_add(by)).clamp(0, rows.len() as isize - 1) as usize;
        level.selected = Some(rows[to].key.clone());
    }

    /// Back to the top level, whose report is read anew: the query changed (B8). The selected
    /// key stays, for a row the new report still has.
    fn reset(&mut self) {
        self.levels.truncate(1);
        self.levels[0].report = None;
    }
}

/// A query of the shown level under a new generation: whatever is on the report lane is for
/// another query, or older, and is dropped when it returns (spec §4.3).
pub(crate) fn load(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    state.gens.report += 1;
    let t = &mut state.tokens;
    t.pending = true;
    t.again = false;
    vec![Cmd::LoadReport {
        r#gen: state.gens.report,
        query: t.query(now_ms),
    }]
}

/// The same query again (spec §6.4's triggers). One already on the lane is let finish, so a
/// slow query is never starved by newer ones, and is followed by one more (B2).
pub(crate) fn requery(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.tokens.pending {
        state.tokens.again = true;
        return vec![];
    }
    load(state, now_ms)
}

/// Tokens opens: the report on screen may be stale, so it is read again (B19).
pub(crate) fn on_enter(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    requery(state, now_ms)
}

/// A report from the report lane. One of an older generation is dropped. A failure keeps the
/// report on screen under the banner (spec §10.4). A query asked for meanwhile goes out now.
pub(crate) fn on_report(
    state: &mut State,
    r#gen: u64,
    result: Result<TokenReport, SourceError>,
    now_ms: i64,
) -> Vec<Cmd> {
    if r#gen != state.gens.report {
        return vec![];
    }
    let t = &mut state.tokens;
    t.pending = false;
    match result {
        Ok(report) => {
            t.error = None;
            t.apply(report);
        }
        Err(e) => t.error = Some(e.message),
    }
    if state.screen == Screen::Tokens {
        state.dirty = true;
    }
    if std::mem::take(&mut state.tokens.again) && state.quitting.is_none() {
        return requery(state, now_ms);
    }
    vec![]
}

/// The footer's keys (spec §5.6), in the order they give way at the end of a narrow line.
/// `Enter` names what the selected row opens, and is left out where it opens nothing.
pub fn footer_keys(state: &TokensState) -> Vec<(&'static str, &'static str)> {
    let mut keys: Vec<(&'static str, &'static str)> = state
        .drill_to()
        .map(|next| ("Enter", next))
        .into_iter()
        .collect();
    keys.extend([
        ("p", "period"),
        ("g", "group"),
        ("[ ]", "range"),
        ("Esc", "back"),
    ]);
    keys
}

/// The Tokens screen's keys (spec §5.6): `p` period, `g` group, `[` `]` range, `Enter` down a
/// level, `Esc`/`Backspace` up one, and the rows.
pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    let t = &mut state.tokens;
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => t.step(1),
        KeyCode::Char('k') | KeyCode::Up => t.step(-1),
        KeyCode::Home => t.step(isize::MIN / 2),
        KeyCode::End => t.step(isize::MAX / 2),
        KeyCode::Char('p') => {
            t.period = match t.period {
                TokenPeriod::Day => TokenPeriod::Month,
                TokenPeriod::Month => TokenPeriod::Session,
                TokenPeriod::Session => TokenPeriod::Block,
                TokenPeriod::Block => TokenPeriod::Day,
            };
            t.reset();
            return load(state, now_ms);
        }
        KeyCode::Char('g') => {
            t.grouping = match t.grouping {
                TokenGrouping::None => TokenGrouping::Account,
                TokenGrouping::Account => TokenGrouping::Model,
                TokenGrouping::Model => TokenGrouping::AccountModel,
                TokenGrouping::AccountModel => TokenGrouping::None,
            };
            t.reset();
            return load(state, now_ms);
        }
        KeyCode::Char('[') | KeyCode::Char(']') => {
            let by = if key.code == KeyCode::Char('[') {
                -1
            } else {
                1
            };
            t.range = t.range.step(by);
            t.reset();
            return load(state, now_ms);
        }
        KeyCode::Enter => return drill(state, now_ms),
        KeyCode::Esc | KeyCode::Backspace => return up(state, now_ms),
        _ => {}
    }
    vec![]
}

/// `Enter`: the selected row's next level down (`TokenQuery::drill`), from the query its report
/// answered. At a session's per-model rows there is none, and nothing happens.
fn drill(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    let t = &mut state.tokens;
    let (Some(report), Some(row)) = (t.report(), t.selected_row()) else {
        return vec![];
    };
    let Some(query) = report.query.drill(row) else {
        return vec![];
    };
    t.levels.push(Level {
        query: Some(query),
        report: None,
        selected: None,
    });
    load(state, now_ms)
}

/// `Esc` or `Backspace`: back one level, to its report and its selection as they were, read
/// again.
fn up(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.tokens.levels.len() == 1 {
        return vec![];
    }
    state.tokens.levels.pop();
    load(state, now_ms)
}

/// Stands in for the report's table until it is drawn.
pub fn render(_state: &State, _frame: &mut Frame, _area: Rect, _now_ms: i64) {}
```

In `crates/tagteam-tui/src/state.rs`, replace:

```rust
use crate::host::Action;
use crate::msg::Cmd;
use crate::screens::accounts::AccountsState;
use crate::screens::history::HistoryState;

/// Timer mode re-reads and collects this often (spec §6.2).
pub const REFRESH_MS: i64 = 30_000;
/// A toast replaces the footer this long (spec §5.1).
```

with:

```rust
use crate::host::Action;
use crate::msg::Cmd;
use crate::screens::accounts::AccountsState;
use crate::screens::history::HistoryState;
use crate::screens::tokens::TokensState;

/// Timer mode re-reads and collects this often (spec §6.2).
pub const REFRESH_MS: i64 = 30_000;
/// A toast replaces the footer this long (spec §5.1).
```

In `crates/tagteam-tui/src/state.rs`, replace:

```rust

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

```

with:

```rust

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Accounts,
    Tokens,
    History,
}

impl Screen {
    /// `Tab`'s order: the screens' numbers (spec §5.6).
    pub(crate) fn next(self) -> Screen {
        match self {
            Screen::Accounts => Screen::Tokens,
            Screen::Tokens => Screen::History,
            Screen::History => Screen::Accounts,
        }
    }

    /// `Shift-Tab`'s order.
    pub(crate) fn prev(self) -> Screen {
        match self {
            Screen::Accounts => Screen::History,
            Screen::Tokens => Screen::Accounts,
            Screen::History => Screen::Tokens,
        }
    }
}

```

In `crates/tagteam-tui/src/state.rs`, replace:

```rust
    pub size: (u16, u16),
    pub color: bool,
    pub provider: Option<ProviderId>,
    pub accounts: AccountsState,
    pub history: HistoryState,
    pub overlay: Option<Overlay>,
    pub toast: Option<Toast>,
    /// A failed read's error, in the CLI's words (spec §10.4).
```

with:

```rust
    pub size: (u16, u16),
    pub color: bool,
    pub provider: Option<ProviderId>,
    pub accounts: AccountsState,
    pub tokens: TokensState,
    pub history: HistoryState,
    pub overlay: Option<Overlay>,
    pub toast: Option<Toast>,
    /// A failed read's error, in the CLI's words (spec §10.4).
```

In `crates/tagteam-tui/src/state.rs`, replace:

```rust
    pub dirty: bool,
    /// The second the screen was last drawn for: its countdowns change at most once a second
    /// (spec §5.2, "Countdowns tick").
    pub(crate) shown_s: i64,
}

/// What raised the banner (spec §10.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
```

with:

```rust
    pub dirty: bool,
    /// The second the screen was last drawn for: its countdowns change at most once a second
    /// (spec §5.2, "Countdowns tick").
    pub(crate) shown_s: i64,
    /// When the state was made: the time the first reads are asked for at (`start`).
    pub(crate) started_ms: i64,
}

/// What raised the banner (spec §10.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
```

In `crates/tagteam-tui/src/state.rs`, replace:

```rust
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
```

with:

```rust
pub struct Gens {
    pub accounts: u64,
    pub history: u64,
    pub collect: u64,
    pub report: u64,
}

pub struct Refresh {
    pub next_ms: i64,
}

impl State {
    pub fn new(
        screen: Screen,
        color: bool,
        provider: Option<ProviderId>,
        tokens: TokensState,
        now_ms: i64,
    ) -> State {
        State {
            screen,
            size: (0, 0),
            color,
            provider,
            accounts: AccountsState::default(),
            tokens,
            history: HistoryState::default(),
            overlay: None,
            toast: None,
            banner: None,
```

In `crates/tagteam-tui/src/state.rs`, replace:

```rust
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
```

with:

```rust
            },
            quitting: None,
            dirty: true,
            shown_s: now_ms.div_euclid(1000),
            started_ms: now_ms,
        }
    }

    /// The first `Cmd`s: load `accounts` (gen 1), and on Tokens its report. History needs an
    /// account, which only the first `accounts` result names, so a TUI started on History loads
    /// it once that arrives.
    pub fn start(&mut self) -> Vec<Cmd> {
        let mut cmds = vec![self.load_accounts()];
        if self.screen == Screen::Tokens {
            cmds.extend(crate::screens::tokens::on_enter(self, self.started_ms));
        }
        cmds
    }

    /// A read of `accounts` under the next generation: any read still in flight is stale.
    pub(crate) fn load_accounts(&mut self) -> Cmd {
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust

use std::sync::mpsc::Sender;

use tagteam_core::AccountId;
use tagteam_engine::views::HistoryView;

use crate::host::{Action, ActionOutcome};
use crate::lanes::Coalesce;
```

with:

```rust

use std::sync::mpsc::Sender;

use tagteam_core::AccountId;
use tagteam_engine::tokens::{TokenQuery, TokenReport};
use tagteam_engine::views::HistoryView;

use crate::host::{Action, ActionOutcome};
use crate::lanes::Coalesce;
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
        action: Action,
    },
}

impl Coalesce for ViewsReq {
    /// Any two of the same kind: only the newest read is worth making.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
```

with:

```rust
        action: Action,
    },
}

/// The report lane's requests: the token report, which reads the index only (spec §4.3).
#[derive(Debug, Clone, PartialEq)]
pub enum ReportReq {
    Report { r#gen: u64, query: TokenQuery },
}

impl Coalesce for ViewsReq {
    /// Any two of the same kind: only the newest read is worth making.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
        )
    }
}

impl Coalesce for ActionsReq {
    /// Two collections merge; an action never does.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
```

with:

```rust
        )
    }
}

impl Coalesce for ReportReq {
    /// Any two of the same kind: only the newest query is worth making.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (ReportReq::Report { .. }, ReportReq::Report { .. })
        )
    }
}

impl Coalesce for ActionsReq {
    /// Two collections merge; an action never does.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
    Collected {
        r#gen: u64,
        result: Result<CollectSummary, SourceError>,
    },
    Prompt(Prompt),
    ActionDone {
        action: Action,
        outcome: ActionOutcome,
```

with:

```rust
    Collected {
        r#gen: u64,
        result: Result<CollectSummary, SourceError>,
    },
    TokenReport {
        r#gen: u64,
        result: Result<TokenReport, SourceError>,
    },
    Prompt(Prompt),
    ActionDone {
        action: Action,
        outcome: ActionOutcome,
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
    Confirm(bool),
    Choose(Option<usize>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    LoadAccounts {
        r#gen: u64,
    },
```

with:

```rust
    Confirm(bool),
    Choose(Option<usize>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    LoadAccounts {
        r#gen: u64,
    },
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
    CollectDue {
        r#gen: u64,
        accounts: Vec<AccountId>,
    },
    Act(Action),
    /// Run `action` with the terminal handed back (spec §7.3).
    Suspend(Action),
    /// Begin the shutdown sequence (spec §10.2) with this exit code.
```

with:

```rust
    CollectDue {
        r#gen: u64,
        accounts: Vec<AccountId>,
    },
    /// The token report `query` asks for, on the report lane (spec §4.3).
    LoadReport {
        r#gen: u64,
        query: TokenQuery,
    },
    Act(Action),
    /// Run `action` with the terminal handed back (spec §7.3).
    Suspend(Action),
    /// Begin the shutdown sequence (spec §10.2) with this exit code.
```

In `crates/tagteam-tui/src/update.rs`, replace:

```rust
                    vec![]
                }
            }
        }
        Msg::Prompt(prompt) => {
            state.dirty = true;
            actions::on_prompt(state, prompt);
            vec![]
```

with:

```rust
                    vec![]
                }
            }
        }
        Msg::TokenReport { r#gen, result } => {
            screens::tokens::on_report(state, r#gen, result, now_ms)
        }
        Msg::Prompt(prompt) => {
            state.dirty = true;
            actions::on_prompt(state, prompt);
            vec![]
```

In `crates/tagteam-tui/src/update.rs`, replace:

```rust
    }
    match key.code {
        KeyCode::Char('q') => quit(state, 0),
        KeyCode::Char('1') => show(state, Screen::Accounts, now_ms),
        KeyCode::Char('3') => show(state, Screen::History, now_ms),
        KeyCode::Tab => show(state, state.screen.next(), now_ms),
        KeyCode::BackTab => show(state, state.screen.prev(), now_ms),
        KeyCode::Char('?') => {
```

with:

```rust
    }
    match key.code {
        KeyCode::Char('q') => quit(state, 0),
        KeyCode::Char('1') => show(state, Screen::Accounts, now_ms),
        KeyCode::Char('2') => show(state, Screen::Tokens, now_ms),
        KeyCode::Char('3') => show(state, Screen::History, now_ms),
        KeyCode::Tab => show(state, state.screen.next(), now_ms),
        KeyCode::BackTab => show(state, state.screen.prev(), now_ms),
        KeyCode::Char('?') => {
```

In `crates/tagteam-tui/src/update.rs`, replace:

```rust
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

```

with:

```rust
        _ => match actions::handle_action_key(state, key, now_ms) {
            Some(cmds) => cmds,
            None => match state.screen {
                Screen::Accounts => screens::accounts::handle_key(state, key, now_ms),
                Screen::Tokens => screens::tokens::handle_key(state, key, now_ms),
                Screen::History => screens::history::handle_key(state, key, now_ms),
            },
        },
    }
}

/// Switches to `screen`; Tokens reads its report and History its account's history as they
/// open.
pub(crate) fn show(state: &mut State, screen: Screen, now_ms: i64) -> Vec<Cmd> {
    if state.screen == screen {
        return vec![];
    }
    state.screen = screen;
    match screen {
        Screen::Accounts => vec![],
        Screen::Tokens => screens::tokens::on_enter(state, now_ms),
        Screen::History => screens::history::on_enter(state, now_ms),
    }
}

```

In `crates/tagteam-tui/src/view.rs`, replace:

```rust
    }
    from.min(heights.len().saturating_sub(1))
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
```

with:

```rust
    }
    from.min(heights.len().saturating_sub(1))
}

/// The banner's text, and whether it is an error: a failed read (spec §10.4) first, then on
/// Tokens a failed token read (B12), else what the accounts view has to say (a run shell, an
/// unmanaged live login).
pub(crate) fn banner(state: &State) -> Option<(String, bool)> {
    let tokens = (state.screen == Screen::Tokens)
        .then(|| state.tokens.error())
        .flatten();
    match (&state.banner, tokens) {
        (Some(error), _) => Some((error.clone(), true)),
        (None, Some(error)) => Some((error.to_owned(), true)),
        (None, None) => screens::accounts::banner(&state.accounts).map(|text| (text, false)),
    }
}

/// The banner and a toast take at most this many rows (spec §5.1).
```

In `crates/tagteam-tui/src/view.rs`, replace:

```rust
        frame.render_widget(Paragraph::new(rows), rect);
    }
    match state.screen {
        Screen::Accounts => screens::accounts::render(state, frame, areas.body, now_ms),
        Screen::History => screens::history::render(state, frame, areas.body, now_ms),
    }
    frame.render_widget(
        Paragraph::new(footer(state, area.width, now_ms)),
```

with:

```rust
        frame.render_widget(Paragraph::new(rows), rect);
    }
    match state.screen {
        Screen::Accounts => screens::accounts::render(state, frame, areas.body, now_ms),
        Screen::Tokens => screens::tokens::render(state, frame, areas.body, now_ms),
        Screen::History => screens::history::render(state, frame, areas.body, now_ms),
    }
    frame.render_widget(
        Paragraph::new(footer(state, area.width, now_ms)),
```

In `crates/tagteam-tui/src/view.rs`, replace:

```rust
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
```

with:

```rust
    );
    actions::render_overlay(state, frame, now_ms);
}

/// ` tagteam  [1 Accounts]  2 Tokens  3 History` on the left, the mode and `? help` on the
/// right.
fn header(state: &State, width: u16) -> Line<'static> {
    let mut spans = vec![Span::styled(" tagteam", bold())];
    for (screen, label) in [
        (Screen::Accounts, "1 Accounts"),
        (Screen::Tokens, "2 Tokens"),
        (Screen::History, "3 History"),
    ] {
        spans.push(Span::raw("  "));
        if screen == state.screen {
```

In `crates/tagteam-tui/src/view.rs`, replace:

```rust
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
```

with:

```rust
    if let Some(action) = &state.running {
        let frame = SPINNER[(now_ms.div_euclid(250)).rem_euclid(10) as usize];
        return vec![Line::from(format!("{frame} {}…", action.name()))];
    }
    let keys: Vec<(&str, &str)> = match state.screen {
        Screen::Accounts => ACCOUNTS_KEYS.to_vec(),
        Screen::Tokens => screens::tokens::footer_keys(&state.tokens),
        Screen::History => HISTORY_KEYS.to_vec(),
    };
    let mut text = String::new();
    for (key, what) in keys {
        let item = format!("{}{key} {what}", if text.is_empty() { "" } else { "  " });
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
}

/// The help panel's keys (spec §5.6): everywhere, then the current screen's.
const HELP_GLOBAL: [(&str, &str); 7] = [
    ("1 3  Tab  Shift-Tab", "screens"),
    ("j k  ↑ ↓", "move"),
    ("A", "add the current login"),
    ("D", "doctor"),
    ("?", "help"),
```

with:

```rust
}

/// The help panel's keys (spec §5.6): everywhere, then the current screen's.
const HELP_GLOBAL: [(&str, &str); 7] = [
    ("1 2 3  Tab  Shift-Tab", "screens"),
    ("j k  ↑ ↓", "move"),
    ("A", "add the current login"),
    ("D", "doctor"),
    ("?", "help"),
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
    ("r", "refresh"),
    ("Enter", "history of the account"),
    ("J K", "scroll the detail's windows"),
];
const HELP_HISTORY: [(&str, &str); 3] = [
    ("[ ]", "range: 1, 7, 14 or 30 days"),
    ("r", "refresh"),
    ("J K", "scroll the windows"),
```

with:

```rust
    ("r", "refresh"),
    ("Enter", "history of the account"),
    ("J K", "scroll the detail's windows"),
];
const HELP_TOKENS: [(&str, &str); 5] = [
    ("p", "period: day, month, session or block"),
    ("g", "group: none, account, model, or both"),
    ("[ ]", "range: 7, 30 or 90 days, or all"),
    ("Enter", "drill down: sessions, then models"),
    ("Esc  Backspace", "back up a level"),
];
const HELP_HISTORY: [(&str, &str); 3] = [
    ("[ ]", "range: 1, 7, 14 or 30 days"),
    ("r", "refresh"),
    ("J K", "scroll the windows"),
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
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
```

with:

```rust
    match overlay {
        Overlay::Help => {
            let keys: &[(&str, &str)] = match state.screen {
                Screen::Accounts => &HELP_ACCOUNTS,
                Screen::Tokens => &HELP_TOKENS,
                Screen::History => &HELP_HISTORY,
            };
            let mut lines = vec![Line::from(Span::styled("Everywhere", bold()))];
            lines.extend(HELP_GLOBAL.iter().map(|(k, what)| help_line(k, what)));
            let title = match state.screen {
                Screen::Accounts => "Accounts",
                Screen::Tokens => "Tokens",
                Screen::History => "History",
            };
            lines.push(Line::from(Span::styled(title, bold())));
            lines.extend(keys.iter().map(|(k, what)| help_line(k, what)));
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
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
```

with:

```rust
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jiff::tz::TimeZone;
use tagteam_core::ProviderId;
use tagteam_provider::Cancel;

use crate::ask::ChannelAsk;
use crate::host::{Action, ActionOutcome, Host};
use crate::input::{Input, Pause};
use crate::lanes::Lane;
use crate::msg::{ActionsReq, Cmd, Msg, ReportReq, ViewsReq};
use crate::screens::tokens::TokensState;
use crate::source::{Source, SourceFactory};
use crate::state::{Screen, State};
use crate::term::{self, RealTty, Tty};
use crate::update::{drawn, update};
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
pub struct TuiOptions {
    pub screen: Screen,
    pub color: bool,
    pub provider: Option<ProviderId>,
    /// The process's signal token: the CLI's handlers record SIGINT, SIGTERM and SIGHUP in it.
    pub signal: Cancel,
    /// The shutdown token. The binary has already built every lane's engine, and the host's
    /// in-TUI context, with it.
```

with:

```rust
pub struct TuiOptions {
    pub screen: Screen,
    pub color: bool,
    pub provider: Option<ProviderId>,
    /// The provider Tokens reports on: `--provider`'s, else the default (spec §5.3).
    pub tokens_provider: ProviderId,
    /// The zone of Tokens' days and months: the system zone (spec §5.3), read by the binary
    /// before the terminal is touched, so the UI thread reads no zone file (B6).
    pub tz: TimeZone,
    /// The process's signal token: the CLI's handlers record SIGINT, SIGTERM and SIGHUP in it.
    pub signal: Cancel,
    /// The shutdown token. The binary has already built every lane's engine, and the host's
    /// in-TUI context, with it.
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
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
```

with:

```rust
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// The engine lanes (§4.3): views (`accounts`, `history`, store-served), report (the token
/// report, which reads the index only) and actions (the collector and every `Host` action, one
/// at a time).
pub(crate) struct Lanes {
    views: Lane<ViewsReq>,
    report: Lane<ReportReq>,
    actions: Lane<ActionsReq>,
}

impl Lanes {
    /// Spawns them. Each builds its own `Source` in its own thread (R4), and a lane that panics
    /// says so on `tx` (§4.3, §10.4).
    pub(crate) fn spawn(
        provider: Option<ProviderId>,
        sources: &SourceFactory,
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
                    let _ = died.send(Msg::LaneDied { lane: "views" });
                },
            )
        };
        let actions = {
            let (make, out, died) = (Arc::clone(sources), tx.clone(), tx.clone());
            let (host, shutdown) = (Arc::clone(host), shutdown.clone());
            Lane::spawn(
```

with:

```rust
                    let _ = died.send(Msg::LaneDied { lane: "views" });
                },
            )
        };
        let report = {
            let (make, out, died) = (Arc::clone(sources), tx.clone(), tx.clone());
            Lane::spawn(
                "report",
                move || make(),
                move |source: &mut Box<dyn Source>, req: ReportReq| {
                    let msg = match req {
                        ReportReq::Report { r#gen, query } => Msg::TokenReport {
                            r#gen,
                            result: source.token_report(&query),
                        },
                    };
                    let _ = out.send(msg);
                },
                move || {
                    let _ = died.send(Msg::LaneDied { lane: "report" });
                },
            )
        };
        let actions = {
            let (make, out, died) = (Arc::clone(sources), tx.clone(), tx.clone());
            let (host, shutdown) = (Arc::clone(host), shutdown.clone());
            Lane::spawn(
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
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
```

with:

```rust
                    let _ = died.send(Msg::LaneDied { lane: "actions" });
                },
            )
        };
        Lanes {
            views,
            report,
            actions,
        }
    }

    /// §10.2 step 2: nothing more is taken, what is queued is dropped, and an idle lane wakes
    /// and returns.
    fn close(&self) {
        self.views.close();
        self.report.close();
        self.actions.close();
    }

    /// §10.2 step 5: joins every lane, and names those that panicked.
    fn join(self) -> Vec<&'static str> {
        let mut died = Vec::new();
        if self.views.join().is_err() {
            died.push("views");
        }
        if self.report.join().is_err() {
            died.push("report");
        }
        if self.actions.join().is_err() {
            died.push("actions");
        }
        died
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
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
```

with:

```rust
        rx: &'a Receiver<Msg>,
        lanes: &'a Lanes,
        host: &'a dyn Host,
    ) -> Self {
        let tokens = TokensState::new(opts.tokens_provider.clone(), opts.tz.clone());
        Ui {
            state: State::new(
                opts.screen,
                opts.color,
                opts.provider.clone(),
                tokens,
                now_ms(),
            ),
            tty,
            input,
            rx,
            lanes,
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
                        .lanes
                        .actions
                        .submit(ActionsReq::Collect { r#gen, accounts });
                }
                Cmd::Act(action) => {
                    let _ = self.lanes.actions.submit(ActionsReq::Act { action });
                }
                Cmd::Suspend(action) => self.suspend(action),
```

with:

```rust
                        .lanes
                        .actions
                        .submit(ActionsReq::Collect { r#gen, accounts });
                }
                Cmd::LoadReport { r#gen, query } => {
                    let _ = self.lanes.report.submit(ReportReq::Report { r#gen, query });
                }
                Cmd::Act(action) => {
                    let _ = self.lanes.actions.submit(ActionsReq::Act { action });
                }
                Cmd::Suspend(action) => self.suspend(action),
```

In `crates/tagteam/src/tui_cmd.rs`, replace:

```rust
        ctx.force_color_env,
        probe.settings().color,
        true,
    );
    drop(probe);
    // §7.3: Ctrl-\ belongs to a foreground session, and the TUI survives it for its whole life.
    if let Err(e) = signals::survive_quit() {
        let _ = writeln!(
            err,
```

with:

```rust
        ctx.force_color_env,
        probe.settings().color,
        true,
    );
    // Spec §5.3: Tokens reports on `--provider`'s provider, else the default, in the system
    // zone, which is read here so that the UI thread reads no zone file (B6).
    let tokens_provider = provider
        .clone()
        .unwrap_or_else(|| probe.default_provider().clone());
    drop(probe);
    let tz = jiff::tz::TimeZone::try_system().unwrap_or(jiff::tz::TimeZone::UTC);
    // §7.3: Ctrl-\ belongs to a foreground session, and the TUI survives it for its whole life.
    if let Err(e) = signals::survive_quit() {
        let _ = writeln!(
            err,
```

In `crates/tagteam/src/tui_cmd.rs`, replace:

```rust
    let opts = TuiOptions {
        screen,
        color,
        provider: provider.clone(),
        signal: ctx.env.cancel.clone(),
        shutdown,
    };
    let exit = tagteam_tui::run(opts, sources(inside, provider), host);
```

with:

```rust
    let opts = TuiOptions {
        screen,
        color,
        provider: provider.clone(),
        tokens_provider,
        tz,
        signal: ctx.env.cancel.clone(),
        shutdown,
    };
    let exit = tagteam_tui::run(opts, sources(inside, provider), host);
```

- [ ] **Step 4: Run the tests and see them pass**

Run:
- `cargo test -p tagteam-tui --lib` — Expected: `189 passed` (13 in `screens::tokens`).
- `cargo test -p tagteam --features test-support --lib tui_cmd` — Expected: `1 passed`.
- `cargo test -p tagteam --features test-support --test tui_cli --test tui_host` — Expected: `5 passed` (`tui_cli`) and `22 passed` (`tui_host`).

- [ ] **Step 5: Format and lint**

Run:
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`

Expected: no output from the first; `Finished` and no warnings from the others.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-tui/src/testutil.rs crates/tagteam-tui/src/screens/mod.rs crates/tagteam-tui/src/screens/tokens/mod.rs crates/tagteam-tui/src/state.rs crates/tagteam-tui/src/msg.rs crates/tagteam-tui/src/update.rs crates/tagteam-tui/src/view.rs crates/tagteam-tui/src/actions.rs crates/tagteam-tui/src/runtime.rs crates/tagteam/src/tui_cmd.rs
git commit -m "Add the Tokens screen's query, drill-down and report lane"
```

### Task 4: The index lane and the re-query triggers

Spec §6.4 in timer mode, in `screens/tokens/index.rs`, with the index lane of spec §4.3.

- **The index lane** runs `refresh_tokens` and passes its progress on at most every 250 ms (`PROGRESS_EVERY`), then the outcome. The report lane also answers `token_index_state`.
- **`IndexWatch`** keeps the index's latest state, the TUI's refresh while it runs, whether the last returned `Busy`, a refresh's failure, and the timers.
- **The triggers** read the report again: the index changed, the accounts changed (B1), 60 s passed, or the active block ended (B18).
- **The first refresh** is asked for after the first frame (B5). `r` on Tokens refreshes at once (B13).

**Readings of the spec this task commits to:**
- **"Index changed"** compares a check's `changed_at_ms` with the report on screen's `index.changed_at_ms` (`None` is earlier than any time). With no report on screen there is nothing to compare: a query is on the lane, or failed, and the 60 s timer retries it.
- **The checks** run every 5 s while the TUI's refresh runs, and while its last one returned `Busy` (B10), and once after each refresh returns. All only while Tokens is open.
- **A report's own `index`** is the latest state the header knows of, until the next check.
- **Off Tokens, a returned refresh** changes nothing that shows: it does not redraw, and does not check.
- **A trigger while a query runs waits for it** (B2). So the tests answer each query before the next trigger.
- **A report asked for while the catch-up runs is answered while it runs** (spec §4.3, §11; B28). The runtime test asks for one (`p`) only once it has seen the refresh start, and requires that report's answer before `q` stops the refresh. A report made to wait behind the refresh fails it.
- **Part A's tests change only where part B changes behaviour:**
  - `drawn`'s first call asks for `RefreshIndex` too (B5);
  - the runtime's lane counts are four.

**Depends on:** 3

**Files:**
- Create: `crates/tagteam-tui/src/screens/tokens/index.rs`
- Modify: `crates/tagteam-tui/src/screens/tokens/mod.rs`
- Modify: `crates/tagteam-tui/src/msg.rs`
- Modify: `crates/tagteam-tui/src/state.rs`
- Modify: `crates/tagteam-tui/src/update.rs`
- Modify: `crates/tagteam-tui/src/view.rs`
- Modify: `crates/tagteam-tui/src/actions.rs`
- Modify: `crates/tagteam-tui/src/runtime.rs`

**Interfaces:**
- Consumes:
  - Task 2: `Source::{refresh_tokens, token_index_state}`.
  - Task 3: `TokensState`, `requery`, `on_report`, the report lane.
  - The engine's `tokens::{IndexProgress, IndexState, RefreshOutcome, PeriodKey, ActiveBlock, IndexReport}`.
- Produces:
  - CB3: `ReportReq::IndexState`, `IndexReq`, `Msg::{IndexState, IndexProgress, Refreshed}`, `Cmd::{LoadIndexState, RefreshIndex}`.
  - CB4: `Gens::index`.
  - CB5's `index.rs`.
  - `TokensState::watch`; the index lane and `PROGRESS_EVERY`.
  - `update::drawn` asks for the first refresh, and `update`'s tick and accounts reads drive the triggers.

**Spec:**
- TUI spec §6.4 (timer mode; when to re-query; the countdown), §6.1 (the first frame never waits), §4.3 (the index lane), §7.1 (`r` on Tokens), §9 (`Busy`; a refresh cancelled by the shutdown token), §10.2 (the shutdown joins every engine lane), §11 (a report query answered while a catch-up still runs; the re-query triggers, and none on a 5 s check that finds no change).
- TR §5.1 (`Wait::Try` and `Busy`), §5.2 (cancel at the next chunk), §10 (`changed_at_ms`), §12 (the 5 s cadence the daemon also keeps).

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-tui/src/screens/tokens/index.rs` with its tests (Step 3 puts the code above them):

```rust
#[cfg(test)]
mod tests {
    use tagteam_engine::tokens::{ActiveBlock, IndexReport, TokenPeriod, TokenRow};

    use super::*;
    use crate::msg::Msg;
    use crate::testutil::*;
    use crate::update::{drawn, update};

    /// The token index's commands among `cmds`; the views' and the collector's are left out.
    fn token_cmds(cmds: Vec<Cmd>) -> Vec<Cmd> {
        cmds.into_iter()
            .filter(|c| {
                matches!(
                    c,
                    Cmd::LoadReport { .. } | Cmd::LoadIndexState { .. } | Cmd::RefreshIndex { .. }
                )
            })
            .collect()
    }

    fn tick(state: &mut State, at_ms: i64) -> Vec<Cmd> {
        token_cmds(update(state, Msg::Tick, at_ms))
    }

    /// Answers the report query in `cmds` with `rows` at `at_ms`, its index changed at
    /// `changed_ms`.
    fn answer(state: &mut State, cmds: &[Cmd], rows: Vec<TokenRow>, changed_ms: i64, at_ms: i64) {
        let [Cmd::LoadReport { r#gen, query }] = cmds else {
            panic!("{cmds:?}");
        };
        let mut report = token_report(query.clone(), rows);
        report.index.changed_at_ms = Some(changed_ms);
        update(
            state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result: Ok(report),
            },
            at_ms,
        );
    }

    fn done() -> Result<RefreshOutcome, SourceError> {
        Ok(RefreshOutcome::Done(IndexReport {
            complete: true,
            ..IndexReport::default()
        }))
    }

    fn index_changed_at(changed_ms: i64) -> IndexState {
        IndexState {
            refreshed_at_ms: Some(NOW_MS),
            changed_at_ms: Some(changed_ms),
            ..IndexState::default()
        }
    }

    /// Tokens, its first frame drawn (so its refresh runs) and its report read, its index
    /// changed at `NOW_MS - 60_000`.
    fn on_tokens() -> State {
        let mut state = fresh(Screen::Tokens);
        let cmds = state.start();
        assert_eq!(drawn(&mut state), [Cmd::RefreshIndex { provider: cc() }]);
        answer(
            &mut state,
            &cmds[1..],
            vec![token_row(
                day(0),
                Some(row_account(1)),
                None,
                counts(1, 2, 3),
                None,
            )],
            NOW_MS - 60_000,
            NOW_MS,
        );
        state
    }

    #[test]
    fn the_first_frame_starts_one_refresh_whatever_the_screen_shows() {
        // B5: after the first frame, never before it, and once.
        let mut state = fresh(Screen::Accounts);
        state.start();
        assert_eq!(drawn(&mut state), [Cmd::RefreshIndex { provider: cc() }]);
        assert!(drawn(&mut state).is_empty());
        assert!(state.tokens.watch().refreshing().is_some());
        let progress = IndexProgress {
            bytes_done: 1_200_000_000,
            bytes_total: 3_300_000_000,
            ..IndexProgress::default()
        };
        update(&mut state, Msg::IndexProgress(progress), NOW_MS);
        assert_eq!(state.tokens.watch().refreshing(), Some(&progress));
        let cmds = update(&mut state, Msg::Refreshed { result: done() }, NOW_MS);
        assert!(cmds.is_empty(), "off Tokens nothing is checked: {cmds:?}");
        assert!(state.tokens.watch().refreshing().is_none());
        assert!(
            tick(&mut state, NOW_MS + 120_000).is_empty(),
            "nor refreshed"
        );
        let mut quitting = fresh(Screen::Accounts);
        quitting.quitting = Some(0);
        assert!(drawn(&mut quitting).is_empty());
    }

    #[test]
    fn on_tokens_the_index_is_refreshed_every_60_s_from_the_last_return() {
        let mut state = on_tokens();
        assert!(
            tick(&mut state, NOW_MS + 120_000)
                .iter()
                .all(|c| !matches!(c, Cmd::RefreshIndex { .. })),
            "one runs already"
        );
        let cmds = update(
            &mut state,
            Msg::Refreshed { result: done() },
            NOW_MS + 10_000,
        );
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadIndexState { .. }]),
            "checked after each refresh returns: {cmds:?}"
        );
        assert!(!tick(&mut state, NOW_MS + 69_999).contains(&Cmd::RefreshIndex { provider: cc() }));
        assert!(tick(&mut state, NOW_MS + 70_000).contains(&Cmd::RefreshIndex { provider: cc() }));
        // Away from Tokens it waits; back on Tokens a due refresh goes at once.
        update(
            &mut state,
            Msg::Refreshed { result: done() },
            NOW_MS + 71_000,
        );
        update(&mut state, ch('1'), NOW_MS + 71_000);
        assert!(tick(&mut state, NOW_MS + 200_000).is_empty());
        update(&mut state, ch('2'), NOW_MS + 200_000);
        assert!(tick(&mut state, NOW_MS + 200_250).contains(&Cmd::RefreshIndex { provider: cc() }));
    }

    #[test]
    fn while_a_refresh_runs_a_check_every_5_s_reads_the_report_again_only_when_the_index_moved() {
        // Spec §6.4: the table fills in as batches commit, and a check that finds no change
        // reads nothing.
        let mut state = on_tokens();
        let check = tick(&mut state, NOW_MS + 1_000);
        let [Cmd::LoadIndexState { r#gen, .. }] = check.as_slice() else {
            panic!("{check:?}");
        };
        assert!(tick(&mut state, NOW_MS + 5_999).is_empty(), "every 5 s");
        let same = Msg::IndexState {
            r#gen: *r#gen,
            result: Ok(index_changed_at(NOW_MS - 60_000)),
        };
        assert!(update(&mut state, same, NOW_MS + 2_000).is_empty());
        let check = tick(&mut state, NOW_MS + 6_000);
        let [Cmd::LoadIndexState { r#gen, .. }] = check.as_slice() else {
            panic!("{check:?}");
        };
        let moved = Msg::IndexState {
            r#gen: *r#gen,
            result: Ok(index_changed_at(NOW_MS + 5_500)),
        };
        let cmds = update(&mut state, moved, NOW_MS + 6_100);
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadReport { .. }]),
            "{cmds:?}"
        );
        assert_eq!(
            state.tokens.watch().state().and_then(|s| s.changed_at_ms),
            Some(NOW_MS + 5_500)
        );
        // No refresh, no checks.
        update(
            &mut state,
            Msg::Refreshed { result: done() },
            NOW_MS + 7_000,
        );
        let later = tick(&mut state, NOW_MS + 30_000);
        assert!(
            later
                .iter()
                .all(|c| !matches!(c, Cmd::LoadIndexState { .. })),
            "{later:?}"
        );
    }

    #[test]
    fn a_check_of_an_older_generation_is_dropped() {
        let mut state = on_tokens();
        let older = tick(&mut state, NOW_MS + 1_000);
        let [Cmd::LoadIndexState { r#gen, .. }] = older.as_slice() else {
            panic!("{older:?}");
        };
        tick(&mut state, NOW_MS + 6_000);
        let stale = Msg::IndexState {
            r#gen: *r#gen,
            result: Ok(index_changed_at(NOW_MS + 5_500)),
        };
        assert!(update(&mut state, stale, NOW_MS + 6_100).is_empty());
    }

    #[test]
    fn busy_keeps_checking_while_another_process_catches_up() {
        // B10: the table fills in as the other process's batches commit.
        let mut state = on_tokens();
        let busy = Ok(RefreshOutcome::Busy);
        update(&mut state, Msg::Refreshed { result: busy }, NOW_MS + 1_000);
        assert!(state.tokens.watch().busy());
        assert!(
            tick(&mut state, NOW_MS + 7_000)
                .iter()
                .any(|c| matches!(c, Cmd::LoadIndexState { .. }))
        );
        update(
            &mut state,
            Msg::Refreshed { result: done() },
            NOW_MS + 61_000,
        );
        assert!(!state.tokens.watch().busy());
    }

    #[test]
    fn an_account_list_that_changed_reads_the_report_again() {
        // B1, spec §6.4: an alias, a position, an account added or removed.
        let mut state = on_tokens();
        let read = |state: &mut State, accounts| {
            state.gens.accounts += 1;
            let r#gen = state.gens.accounts;
            token_cmds(update(
                state,
                Msg::Accounts {
                    r#gen,
                    result: Ok(snapshot(accounts)),
                },
                NOW_MS,
            ))
        };
        assert!(read(&mut state, many(2)).is_empty(), "the first read");
        assert!(read(&mut state, many(2)).is_empty(), "the same accounts");
        let mut aliased = many(2);
        aliased[0].row.alias = Some("work".into());
        let again = read(&mut state, aliased.clone());
        assert!(
            matches!(again.as_slice(), [Cmd::LoadReport { .. }]),
            "{again:?}"
        );
        answer(&mut state, &again, vec![], NOW_MS - 60_000, NOW_MS);
        aliased[1].row.position = 3;
        let again = read(&mut state, aliased);
        assert_eq!(again.len(), 1, "a position");
        answer(&mut state, &again, vec![], NOW_MS - 60_000, NOW_MS);
        let again = read(&mut state, many(1));
        assert_eq!(again.len(), 1, "an account removed");
        answer(&mut state, &again, vec![], NOW_MS - 60_000, NOW_MS);
        update(&mut state, ch('1'), NOW_MS);
        assert!(read(&mut state, many(2)).is_empty(), "off Tokens");
        let cmds = update(&mut state, ch('2'), NOW_MS);
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadReport { .. }]),
            "opening Tokens reads it again (B19): {cmds:?}"
        );
    }

    #[test]
    fn the_report_is_read_again_every_60_s() {
        let mut state = on_tokens();
        update(&mut state, Msg::Refreshed { result: done() }, NOW_MS);
        let reports = |cmds: &[Cmd]| {
            cmds.iter()
                .filter(|c| matches!(c, Cmd::LoadReport { .. }))
                .count()
        };
        assert_eq!(reports(&tick(&mut state, NOW_MS + 59_999)), 0);
        let cmds = tick(&mut state, NOW_MS + 60_000);
        assert_eq!(reports(&cmds), 1, "{cmds:?}");
        let again: Vec<Cmd> = cmds
            .into_iter()
            .filter(|c| matches!(c, Cmd::LoadReport { .. }))
            .collect();
        answer(&mut state, &again, vec![], NOW_MS - 60_000, NOW_MS + 61_000);
        assert_eq!(
            reports(&tick(&mut state, NOW_MS + 120_999)),
            0,
            "60 s from it"
        );
        assert_eq!(reports(&tick(&mut state, NOW_MS + 121_000)), 1);
    }

    fn block(start_ms: i64, end_ms: i64, active: bool) -> TokenRow {
        let period = PeriodKey::Block {
            start_ms,
            end_ms,
            observed: true,
            active: active.then_some(ActiveBlock {
                elapsed_s: (NOW_MS - start_ms) / 1000,
                remaining_s: (end_ms - NOW_MS) / 1000,
                tokens_per_min: 100.0,
                cost_per_hour: None,
                projected_tokens: 1_000,
                projected_cost: None,
            }),
        };
        token_row(period, Some(row_account(1)), None, counts(1, 2, 3), None)
    }

    #[test]
    fn the_active_blocks_end_reads_the_report_again_once() {
        let mut state = on_tokens();
        update(&mut state, Msg::Refreshed { result: done() }, NOW_MS);
        update(&mut state, ch('p'), NOW_MS);
        update(&mut state, ch('p'), NOW_MS);
        let blocks = update(&mut state, ch('p'), NOW_MS);
        let [Cmd::LoadReport { query, .. }] = blocks.as_slice() else {
            panic!("{blocks:?}");
        };
        assert_eq!(query.period, TokenPeriod::Block);
        // It ends before the 60 s re-query is due.
        let end = NOW_MS + 30_000;
        let rows = vec![
            block(end - 5 * 3_600_000, end, true),
            block(end - 10 * 3_600_000, end - 5 * 3_600_000, false),
        ];
        answer(&mut state, &blocks, rows, NOW_MS - 60_000, NOW_MS);
        let reports = |cmds: Vec<Cmd>| {
            cmds.into_iter()
                .filter(|c| matches!(c, Cmd::LoadReport { .. }))
                .collect::<Vec<_>>()
        };
        assert!(reports(tick(&mut state, end - 1)).is_empty());
        let at_end = reports(tick(&mut state, end));
        assert_eq!(at_end.len(), 1, "at its end");
        let rows = vec![block(end - 5 * 3_600_000, end, true)];
        answer(&mut state, &at_end, rows, NOW_MS - 60_000, end);
        assert!(
            reports(tick(&mut state, end + 1_000)).is_empty(),
            "once, though the report read again still says active"
        );
    }

    #[test]
    fn r_refreshes_the_index_at_once_and_says_so_while_one_runs() {
        let mut state = on_tokens();
        assert!(update(&mut state, ch('r'), NOW_MS).is_empty());
        assert_eq!(state.toast.as_ref().map(|t| t.text.as_str()), Some(ALREADY));
        update(&mut state, Msg::Refreshed { result: done() }, NOW_MS);
        state.running = Some(crate::host::Action::AddLive { provider: cc() });
        assert_eq!(
            update(&mut state, ch('r'), NOW_MS),
            [Cmd::RefreshIndex { provider: cc() }],
            "on its own lane: an action does not hold it up"
        );
    }

    #[test]
    fn a_failed_refresh_shows_on_tokens_and_the_next_good_one_clears_it() {
        let mut state = on_tokens();
        let failed = Err(SourceError {
            kind: "store".into(),
            message: "the store is locked".into(),
        });
        update(&mut state, Msg::Refreshed { result: failed }, NOW_MS);
        assert_eq!(
            draw(&state, 80, 24)[1],
            " the token index was not refreshed: the store is locked"
        );
        update(&mut state, ch('r'), NOW_MS);
        update(&mut state, Msg::Refreshed { result: done() }, NOW_MS);
        assert_eq!(state.tokens.error(), None);
    }
}
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
            "group: none, account, model, or both",
            "range: 7, 30 or 90 days, or all",
            "drill down: sessions, then models",
            "back up a level",
        ] {
            assert!(lines.iter().any(|l| l.contains(what)), "{what}: {lines:?}");
        }
    }
```

with:

```rust
            "group: none, account, model, or both",
            "range: 7, 30 or 90 days, or all",
            "drill down: sessions, then models",
            "back up a level",
            "refresh the token index",
        ] {
            assert!(lines.iter().any(|l| l.contains(what)), "{what}: {lines:?}");
        }
    }
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
    fn the_footer_says_what_enter_opens() {
        let mut state = showing_days();
        assert_eq!(
            draw(&state, 80, 24)[23],
            "Enter sessions  p period  g group  [ ] range  Esc back"
        );
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer(&mut state, &down, vec![session("s1", 1)]);
        assert!(draw(&state, 80, 24)[23].starts_with("Enter models  p period"));
```

with:

```rust
    fn the_footer_says_what_enter_opens() {
        let mut state = showing_days();
        assert_eq!(
            draw(&state, 80, 24)[23],
            "Enter sessions  p period  g group  [ ] range  Esc back  r refresh"
        );
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer(&mut state, &down, vec![session("s1", 1)]);
        assert!(draw(&state, 80, 24)[23].starts_with("Enter models  p period"));
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
            r#gen: g,
            query: query(days),
        };
        assert!(report(1, 7).same_as(&report(2, 30)));
    }

    #[test]
    fn collections_coalesce_and_actions_never_do() {
```

with:

```rust
            r#gen: g,
            query: query(days),
        };
        assert!(report(1, 7).same_as(&report(2, 30)));
        let state = |g| ReportReq::IndexState {
            r#gen: g,
            provider: ProviderId::new("claude-code"),
        };
        assert!(state(1).same_as(&state(2)));
        assert!(!state(1).same_as(&report(1, 7)));
        let refresh = |p: &str| IndexReq::Refresh {
            provider: ProviderId::new(p),
        };
        assert!(refresh("claude-code").same_as(&refresh("claude-code")));
    }

    #[test]
    fn collections_coalesce_and_actions_never_do() {
```

In `crates/tagteam-tui/src/update.rs`, replace:

```rust
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
```

with:

```rust
    }

    #[test]
    fn the_first_frame_that_shows_accounts_asks_for_one_collection() {
        // Spec §6.1: the TUI collects after drawing, not a timer's 30 s later. The first frame
        // starts the token index's catch-up whatever it shows (B5).
        let mut state = fresh(Screen::Accounts);
        state.start();
        assert_eq!(
            drawn(&mut state),
            [Cmd::RefreshIndex { provider: cc() }],
            "`loading…` shows no accounts"
        );
        update(&mut state, accounts_msg(1, 0), NOW_MS);
        assert!(drawn(&mut state).is_empty(), "nor does the empty state");
        let Cmd::LoadAccounts { r#gen } = state.load_accounts() else {
            unreachable!()
```

In `crates/tagteam-tui/src/update.rs`, replace:

```rust
        // A collection asked for before any frame showed accounts (`r`) is the first one.
        let mut state = fresh(Screen::Accounts);
        state.collect_due(vec![id(1)]);
        update(&mut state, accounts_msg(0, 1), NOW_MS);
        assert!(drawn(&mut state).is_empty());
    }

    #[test]
```

with:

```rust
        // A collection asked for before any frame showed accounts (`r`) is the first one.
        let mut state = fresh(Screen::Accounts);
        state.collect_due(vec![id(1)]);
        update(&mut state, accounts_msg(0, 1), NOW_MS);
        assert_eq!(drawn(&mut state), [Cmd::RefreshIndex { provider: cc() }]);
        assert!(drawn(&mut state).is_empty());
    }

    #[test]
```

In `crates/tagteam-tui/src/view.rs`, replace:

```rust
                " tagteam  1 Accounts  [2 Tokens]  3 History{}○ refresh 30s   ? help",
                " ".repeat(14)
            )
        );
        assert_eq!(tokens[23], "p period  g group  [ ] range  Esc back");
    }

    #[test]
    fn below_80_by_24_only_the_notice_is_drawn() {
```

with:

```rust
                " tagteam  1 Accounts  [2 Tokens]  3 History{}○ refresh 30s   ? help",
                " ".repeat(14)
            )
        );
        assert_eq!(
            tokens[23],
            "p period  g group  [ ] range  Esc back  r refresh"
        );
    }

    #[test]
    fn below_80_by_24_only_the_notice_is_drawn() {
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tagteam_core::AccountId;
    use tagteam_engine::tokens::{
        IndexProgress, IndexState, RefreshOutcome, TokenQuery, TokenReport,
    };
    use tagteam_engine::views::HistoryView;

    use super::*;
```

with:

```rust
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tagteam_core::AccountId;
    use tagteam_engine::tokens::{
        IndexProgress, IndexState, RefreshOutcome, TokenPeriod, TokenQuery, TokenReport,
    };
    use tagteam_engine::views::HistoryView;

    use super::*;
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
            Box::new(Signalled(signal.clone())) as Box<dyn Source>
        })
    }

    /// Waits up to 5 s for `frames` to see `what`; whether it did.
    fn seen_within(frames: &Log, what: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !frames.seen().contains(&what) {
```

with:

```rust
            Box::new(Signalled(signal.clone())) as Box<dyn Source>
        })
    }

    /// A source with one account whose index refresh runs until the shutdown token is set,
    /// telling its progress every 5 ms and logging `refresh` and `stopped` in `log`, while its
    /// report answers at once with no rows.
    struct CatchingUp {
        shutdown: Cancel,
        log: Log,
    }

    impl Source for CatchingUp {
        fn accounts(&self, _: Option<&ProviderId>) -> Result<AccountsSnapshot, SourceError> {
            Ok(crate::testutil::snapshot(crate::testutil::many(1)))
        }
        fn history(&self, _: &AccountId, _: i64) -> Result<HistoryView, SourceError> {
            Err(no_tokens())
        }
        fn collect_due(&self, _: &[AccountId]) -> Result<CollectSummary, SourceError> {
            Ok(CollectSummary {
                fetched: 0,
                warnings: Vec::new(),
            })
        }
        fn refresh_tokens(
            &self,
            _: &ProviderId,
            progress: &mut dyn FnMut(&IndexProgress),
        ) -> Result<RefreshOutcome, SourceError> {
            self.log.push("refresh");
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.shutdown.requested().is_none() && Instant::now() < deadline {
                progress(&IndexProgress::default());
                thread::sleep(Duration::from_millis(5));
            }
            self.log.push("stopped");
            Err(SourceError {
                kind: "interrupted".into(),
                message: "interrupted".into(),
            })
        }
        fn token_report(&self, query: &TokenQuery) -> Result<TokenReport, SourceError> {
            Ok(crate::testutil::token_report(query.clone(), vec![]))
        }
        fn token_index_state(&self, _: &ProviderId) -> Result<IndexState, SourceError> {
            Ok(IndexState::default())
        }
    }

    fn catching_up(shutdown: &Cancel, log: &Log) -> SourceFactory {
        let (shutdown, log) = (shutdown.clone(), log.clone());
        Arc::new(move || {
            Box::new(CatchingUp {
                shutdown: shutdown.clone(),
                log: log.clone(),
            }) as Box<dyn Source>
        })
    }

    /// Waits up to 5 s for `frames` to see `what`; whether it did.
    fn seen_within(frames: &Log, what: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !frames.seen().contains(&what) {
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
        );
        assert_eq!((exit.code, exit.report), (0, None));
        assert_eq!(
            drops.load(Ordering::SeqCst),
            3,
            "every lane ended before the TUI returned"
        );
        assert_eq!(
            shutdown.requested(),
```

with:

```rust
        );
        assert_eq!((exit.code, exit.report), (0, None));
        assert_eq!(
            drops.load(Ordering::SeqCst),
            4,
            "every lane ended before the TUI returned"
        );
        assert_eq!(
            shutdown.requested(),
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn q_while_an_action_runs_waits_for_it_and_reports_what_it_did() {
        const SPAN: Duration = Duration::from_millis(300);
        const SWITCHED: &str = "Switched to a@x.co (position 1).\n";
```

with:

```rust
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn a_report_asked_for_while_the_index_catches_up_is_answered_and_q_stops_the_catch_up() {
        // Spec §4.3, §11: the index has a lane of its own, so a report asked for while a
        // catch-up runs is answered before the catch-up ends; the shutdown token then stops the
        // catch-up, and its lane is joined (§9, §10.2).
        let opts = TuiOptions {
            screen: Screen::Tokens,
            ..options()
        };
        let shutdown = opts.shutdown.clone();
        let log = Log::default();
        let fx = Fixture::with(opts, catching_up(&shutdown, &log), |_| idle());
        let started = Instant::now();
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            assert!(seen_within(&log, "refresh"), "the first frame started it");
            // Asked for only now that the catch-up runs: `p` reads the months.
            ui.apply(Msg::Key(KeyEvent::new(
                KeyCode::Char('p'),
                KeyModifiers::NONE,
            )));
            let asked = ui.state.gens.report;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                let msg = fx.rx.recv_timeout(left);
                let msg = msg.expect("the report lane answered while the catch-up ran");
                let answer = matches!(&msg, Msg::TokenReport { r#gen, .. } if *r#gen == asked);
                ui.deliver(msg);
                if answer {
                    break;
                }
            }
            assert_eq!(
                ui.state.tokens.report().map(|r| r.query.period),
                Some(TokenPeriod::Month)
            );
            assert_eq!(log.seen(), ["refresh"], "the catch-up still runs");
            assert!(ui.state.tokens.watch().refreshing().is_some());
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
        assert_eq!(log.seen(), ["refresh", "stopped"]);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_refresh_tells_the_loop_its_progress_at_most_every_250_ms() {
        let opts = options();
        let shutdown = opts.shutdown.clone();
        let log = Log::default();
        let fx = Fixture::with(opts, catching_up(&shutdown, &log), |_| idle());
        let provider = ProviderId::new("claude-code");
        assert!(fx.lanes.index.submit(IndexReq::Refresh { provider }));
        let started = Instant::now();
        let mut told = 0;
        while started.elapsed() < Duration::from_millis(600) {
            if let Ok(Msg::IndexProgress(_)) = fx.rx.recv_timeout(Duration::from_millis(10)) {
                told += 1;
            }
        }
        assert!(
            (2..=4).contains(&told),
            "{told} in 600 ms, of a call every 5 ms"
        );
        shutdown.request(libc::SIGINT);
        let ending = Ending {
            code: 0,
            fatal: None,
            report: String::new(),
        };
        assert_eq!(fx.finish(ending).code, 0);
        assert_eq!(log.seen(), ["refresh", "stopped"]);
    }

    #[test]
    fn q_while_an_action_runs_waits_for_it_and_reports_what_it_did() {
        const SPAN: Duration = Duration::from_millis(300);
        const SWITCHED: &str = "Switched to a@x.co (position 1).\n";
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
            |_tx| Ok(Keys::default()),
            || {},
        );
        assert_eq!((exit.code, exit.report), (1, None));
        assert_eq!(drops.load(Ordering::SeqCst), 3, "every lane was joined");
        assert_eq!(shutdown.requested(), Some(libc::SIGINT));
    }
}
```

with:

```rust
            |_tx| Ok(Keys::default()),
            || {},
        );
        assert_eq!((exit.code, exit.report), (1, None));
        assert_eq!(drops.load(Ordering::SeqCst), 4, "every lane was joined");
        assert_eq!(shutdown.requested(), Some(libc::SIGINT));
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p tagteam-tui --lib`

Expected: FAIL: the build stops with errors such as ``error[E0599]: no variant named `IndexState` found for enum `msg::ReportReq` ``, ``error[E0599]: no method named `watch` found for struct `TokensState` `` and ``error[E0609]: no field `index` on type `runtime::Lanes` ``.

- [ ] **Step 3: Write the code**

In `crates/tagteam-tui/src/screens/tokens/index.rs`, put this above the tests, with one blank line between:

```rust
//! The token index on the Tokens screen (spec §5.3 "Indexing", §6.4 in timer mode): the TUI's
//! own catch-up on the index lane, the index's state for the header, and when the report on
//! screen is read again.

use tagteam_core::AccountId;
use tagteam_engine::tokens::{IndexProgress, IndexState, PeriodKey, RefreshOutcome, TokenReport};

use super::requery;
use crate::msg::Cmd;
use crate::source::SourceError;
use crate::state::{Screen, State};

/// Spec §6.4: in timer mode the index is refreshed this often while Tokens is open, counted
/// from the last refresh's return...
pub const REFRESH_MS: i64 = 60_000;
/// ... the report on screen is read again this often, because the active block, a rolling
/// range and the day's or month's boundary depend on the clock ...
pub const REQUERY_MS: i64 = 60_000;
/// ... and while a refresh runs, the index's state is checked this often, so the table fills
/// in as its batches commit.
pub const CHECK_MS: i64 = 5_000;
/// `r` while the TUI's own refresh runs.
pub const ALREADY: &str = "the token index is already being refreshed";

/// An account as the report's rows name it (`RowAccount::Account`): its ID, position, label
/// and alias.
type AccountKey = (AccountId, u32, String, Option<String>);

/// The index's state as the screen knows it, and when it acts on it.
pub struct IndexWatch {
    /// The index's state as last read: with the last report, or by the last check.
    state: Option<IndexState>,
    /// The TUI's own refresh, while it runs: how far it has come.
    refreshing: Option<IndexProgress>,
    /// The last refresh returned `Busy`: another process is catching up (spec §5.3).
    busy: bool,
    /// The first refresh has been asked for (B5).
    started: bool,
    /// The last refresh's failure, in words.
    error: Option<String>,
    next_refresh_ms: i64,
    next_query_ms: i64,
    next_check_ms: i64,
    /// The Tokens provider's accounts as the last `accounts` read listed them (B1).
    accounts: Option<Vec<AccountKey>>,
    /// The end of the active block the report was last read again for.
    ended: Option<i64>,
}

impl Default for IndexWatch {
    /// Nothing read yet. The index is refreshed when the screen is first open; the report is
    /// read again 60 s after one arrives.
    fn default() -> Self {
        IndexWatch {
            state: None,
            refreshing: None,
            busy: false,
            started: false,
            error: None,
            next_refresh_ms: 0,
            next_query_ms: i64::MAX,
            next_check_ms: 0,
            accounts: None,
            ended: None,
        }
    }
}

impl IndexWatch {
    /// The index's state as last read.
    pub fn state(&self) -> Option<&IndexState> {
        self.state.as_ref()
    }

    /// The TUI's own refresh's progress while it runs.
    pub fn refreshing(&self) -> Option<&IndexProgress> {
        self.refreshing.as_ref()
    }

    /// Whether the last refresh found another process catching up.
    pub fn busy(&self) -> bool {
        self.busy
    }

    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// A refresh of the index on the index lane (spec §6.4).
fn refresh(state: &mut State) -> Vec<Cmd> {
    let watch = &mut state.tokens.watch;
    watch.started = true;
    watch.refreshing = Some(IndexProgress::default());
    vec![Cmd::RefreshIndex {
        provider: state.tokens.provider.clone(),
    }]
}

/// B5: the first refresh, asked for once the first frame is drawn, so the catch-up never
/// delays it (spec §1.1 criterion 2).
pub(crate) fn first_refresh(state: &mut State) -> Vec<Cmd> {
    if state.tokens.watch.started || state.quitting.is_some() {
        return vec![];
    }
    refresh(state)
}

/// `r` on Tokens (spec §7.1): a refresh at once, on the index lane, so no action holds it up.
pub(crate) fn refresh_now(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.tokens.watch.refreshing.is_some() {
        state.toast(ALREADY, now_ms);
        return vec![];
    }
    refresh(state)
}

/// The index's state from the report lane, under the next generation.
fn check(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    state.gens.index += 1;
    state.tokens.watch.next_check_ms = now_ms + CHECK_MS;
    vec![Cmd::LoadIndexState {
        r#gen: state.gens.index,
        provider: state.tokens.provider.clone(),
    }]
}

/// The refresh's progress, for the header (spec §5.3).
pub(crate) fn on_progress(state: &mut State, progress: IndexProgress) {
    let watch = &mut state.tokens.watch;
    if watch.refreshing.is_some() {
        watch.refreshing = Some(progress);
        state.dirty |= state.screen == Screen::Tokens;
    }
}

/// The refresh returned: the next is due in `REFRESH_MS`, and on Tokens the index's state is
/// checked at once (spec §6.4).
pub(crate) fn on_refreshed(
    state: &mut State,
    result: Result<RefreshOutcome, SourceError>,
    now_ms: i64,
) -> Vec<Cmd> {
    let watch = &mut state.tokens.watch;
    watch.refreshing = None;
    watch.next_refresh_ms = now_ms + REFRESH_MS;
    match result {
        Ok(RefreshOutcome::Done(_)) => {
            watch.busy = false;
            watch.error = None;
        }
        Ok(RefreshOutcome::Busy) => {
            watch.busy = true;
            watch.error = None;
        }
        Err(e) => {
            watch.busy = false;
            watch.error = Some(format!("the token index was not refreshed: {}", e.message));
        }
    }
    if state.screen != Screen::Tokens || state.quitting.is_some() {
        return vec![];
    }
    state.dirty = true;
    check(state, now_ms)
}

/// The index's state from a check. One of an older generation is dropped. When the index
/// changed after the report on screen was read, the report is read again (spec §6.4).
pub(crate) fn on_index_state(
    state: &mut State,
    r#gen: u64,
    result: Result<IndexState, SourceError>,
    now_ms: i64,
) -> Vec<Cmd> {
    if r#gen != state.gens.index {
        return vec![];
    }
    state.dirty |= state.screen == Screen::Tokens;
    let index = match result {
        Ok(index) => index,
        Err(e) => {
            state.tokens.error = Some(e.message);
            return vec![];
        }
    };
    let shown = state.tokens.report().map(|r| r.index.changed_at_ms);
    state.tokens.watch.state = Some(index);
    let moved = shown.is_some_and(|shown| index.changed_at_ms > shown);
    if moved && state.screen == Screen::Tokens && state.quitting.is_none() {
        return requery(state, now_ms);
    }
    vec![]
}

/// A report arrived, or failed: the 60 s re-query counts from now, and a report's index state
/// is the latest read.
pub(crate) fn on_report(state: &mut State, now_ms: i64) {
    let index = state.tokens.report().map(|r| r.index);
    let watch = &mut state.tokens.watch;
    watch.next_query_ms = now_ms + REQUERY_MS;
    if index.is_some() {
        watch.state = index;
    }
}

/// The Tokens provider's accounts in `state`'s last `accounts` read.
fn account_keys(state: &State) -> Vec<AccountKey> {
    let Some(snap) = state.accounts.snapshot() else {
        return vec![];
    };
    snap.providers
        .iter()
        .filter(|p| &p.provider == state.tokens.provider())
        .flat_map(|p| &p.accounts)
        .map(|v| {
            let row = &v.row;
            (
                row.id.clone(),
                row.position,
                row.label.clone(),
                row.alias.clone(),
            )
        })
        .collect()
}

/// A new `accounts` read (B1): when it lists the Tokens provider's accounts differently from
/// the last (an alias, a position, an account added or removed), the rows' labels may be out of
/// date, and while Tokens is open the report is read again (spec §6.4).
pub(crate) fn on_accounts(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    let keys = account_keys(state);
    let before = state.tokens.watch.accounts.replace(keys);
    let changed =
        before.is_some_and(|before| Some(&before) != state.tokens.watch.accounts.as_ref());
    if changed && state.screen == Screen::Tokens {
        return requery(state, now_ms);
    }
    vec![]
}

/// The end of the active block in `report`, if it has one.
fn active_end(report: &TokenReport) -> Option<i64> {
    report
        .rows
        .iter()
        .filter_map(|r| match &r.period {
            PeriodKey::Block {
                end_ms,
                active: Some(_),
                ..
            } => Some(*end_ms),
            _ => None,
        })
        .min()
}

/// Spec §6.4 on the UI's tick, while Tokens is open: the refresh every 60 s; the index's state
/// every 5 s while a refresh runs here or elsewhere (`Busy`); and the report again every 60 s,
/// and once at the active block's end.
pub(crate) fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.screen != Screen::Tokens || state.quitting.is_some() {
        return vec![];
    }
    let mut cmds = Vec::new();
    let watch = &state.tokens.watch;
    if watch.refreshing.is_none() && now_ms >= watch.next_refresh_ms {
        cmds.extend(refresh(state));
    }
    let watch = &state.tokens.watch;
    if (watch.refreshing.is_some() || watch.busy) && now_ms >= watch.next_check_ms {
        cmds.extend(check(state, now_ms));
    }
    let ended = state
        .tokens
        .report()
        .and_then(active_end)
        .filter(|end| now_ms >= *end && state.tokens.watch.ended != Some(*end));
    if let Some(end) = ended {
        state.tokens.watch.ended = Some(end);
        cmds.extend(requery(state, now_ms));
    } else if now_ms >= state.tokens.watch.next_query_ms {
        state.tokens.watch.next_query_ms = now_ms + REQUERY_MS;
        cmds.extend(requery(state, now_ms));
    }
    cmds
}
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
//! The Tokens screen (spec §5.3): the token report (token report §10) by day, month, session or
//! 5-hour block, grouped and filtered, with drill-down from a period to its sessions and from a
//! session to its models. Every query runs on the report lane (§4.3), never on the UI thread.

use crossterm::event::{KeyCode, KeyEvent};
use jiff::tz::TimeZone;
use ratatui::Frame;
```

with:

```rust
//! The Tokens screen (spec §5.3): the token report (token report §10) by day, month, session or
//! 5-hour block, grouped and filtered, with drill-down from a period to its sessions and from a
//! session to its models. Every query runs on the report lane (§4.3), never on the UI thread,
//! and the index's catch-up on the index lane (`index`).

pub mod index;

use crossterm::event::{KeyCode, KeyEvent};
use jiff::tz::TimeZone;
use ratatui::Frame;
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
    AccountSel, RowKey, TokenGrouping, TokenPeriod, TokenQuery, TokenReport, TokenRow,
    default_since_ms,
};

use crate::msg::Cmd;
use crate::source::SourceError;
use crate::state::{Screen, State};

```

with:

```rust
    AccountSel, RowKey, TokenGrouping, TokenPeriod, TokenQuery, TokenReport, TokenRow,
    default_since_ms,
};

use self::index::IndexWatch;
use crate::msg::Cmd;
use crate::source::SourceError;
use crate::state::{Screen, State};

```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
    /// Asked to query again while `pending`: done once that query returns (B2).
    again: bool,
    /// A failed read's words, the banner while Tokens is open (spec §10.4, B12).
    error: Option<String>,
}

impl TokensState {
    /// By day and account over the default range, unfiltered (spec §5.3; `tagteam tokens`'s
```

with:

```rust
    /// Asked to query again while `pending`: done once that query returns (B2).
    again: bool,
    /// A failed read's words, the banner while Tokens is open (spec §10.4, B12).
    error: Option<String>,
    /// The index's state, its refresh, and the timers that read the report again (§6.4).
    watch: IndexWatch,
}

impl TokensState {
    /// By day and account over the default range, unfiltered (spec §5.3; `tagteam tokens`'s
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
            levels: vec![Level::top()],
            pending: false,
            again: false,
            error: None,
        }
    }

    pub fn provider(&self) -> &ProviderId {
```

with:

```rust
            levels: vec![Level::top()],
            pending: false,
            again: false,
            error: None,
            watch: IndexWatch::default(),
        }
    }

    pub fn provider(&self) -> &ProviderId {
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
    pub fn pending(&self) -> bool {
        self.pending
    }

    /// A failed read's words, for the banner (B12).
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The shown level's query at `now_ms`: the top's from the settings, its range ending now;
    /// a drilled level's as it was drilled.
```

with:

```rust
    pub fn pending(&self) -> bool {
        self.pending
    }

    /// A failed read's words, for the banner (B12): a report's or a check's, else the last
    /// refresh's.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref().or_else(|| self.watch.error())
    }

    /// The index's state and the TUI's refresh of it.
    pub fn watch(&self) -> &IndexWatch {
        &self.watch
    }

    /// The shown level's query at `now_ms`: the top's from the settings, its range ending now;
    /// a drilled level's as it was drilled.
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
            t.apply(report);
        }
        Err(e) => t.error = Some(e.message),
    }
    if state.screen == Screen::Tokens {
        state.dirty = true;
    }
    if std::mem::take(&mut state.tokens.again) && state.quitting.is_none() {
```

with:

```rust
            t.apply(report);
        }
        Err(e) => t.error = Some(e.message),
    }
    index::on_report(state, now_ms);
    if state.screen == Screen::Tokens {
        state.dirty = true;
    }
    if std::mem::take(&mut state.tokens.again) && state.quitting.is_none() {
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
        ("p", "period"),
        ("g", "group"),
        ("[ ]", "range"),
        ("Esc", "back"),
    ]);
    keys
}

```

with:

```rust
        ("p", "period"),
        ("g", "group"),
        ("[ ]", "range"),
        ("Esc", "back"),
        ("r", "refresh"),
    ]);
    keys
}

```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
//! (spec §4.2, §4.3).

use std::sync::mpsc::Sender;

use tagteam_core::AccountId;
use tagteam_engine::tokens::{TokenQuery, TokenReport};
use tagteam_engine::views::HistoryView;

use crate::host::{Action, ActionOutcome};
use crate::lanes::Coalesce;
```

with:

```rust
//! (spec §4.2, §4.3).

use std::sync::mpsc::Sender;

use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::tokens::{IndexProgress, IndexState, RefreshOutcome, TokenQuery, TokenReport};
use tagteam_engine::views::HistoryView;

use crate::host::{Action, ActionOutcome};
use crate::lanes::Coalesce;
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
        action: Action,
    },
}

/// The report lane's requests: the token report, which reads the index only (spec §4.3).
#[derive(Debug, Clone, PartialEq)]
pub enum ReportReq {
    Report { r#gen: u64, query: TokenQuery },
}

impl Coalesce for ViewsReq {
    /// Any two of the same kind: only the newest read is worth making.
```

with:

```rust
        action: Action,
    },
}

/// The report lane's requests: the token report, which reads the index only, and the index's
/// state (spec §4.3, §6.4).
#[derive(Debug, Clone, PartialEq)]
pub enum ReportReq {
    Report { r#gen: u64, query: TokenQuery },
    IndexState { r#gen: u64, provider: ProviderId },
}

/// The index lane's requests: the token index's catch-up, which can run for minutes on a cold
/// index (spec §4.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexReq {
    Refresh { provider: ProviderId },
}

impl Coalesce for ViewsReq {
    /// Any two of the same kind: only the newest read is worth making.
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
    fn same_as(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (ReportReq::Report { .. }, ReportReq::Report { .. })
        )
    }
}

impl Coalesce for ActionsReq {
    /// Two collections merge; an action never does.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
```

with:

```rust
    fn same_as(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (ReportReq::Report { .. }, ReportReq::Report { .. })
                | (ReportReq::IndexState { .. }, ReportReq::IndexState { .. })
        )
    }
}

impl Coalesce for IndexReq {
    /// A refresh queued behind the running one is one refresh.
    fn same_as(&self, other: &Self) -> bool {
        self == other
    }
}

impl Coalesce for ActionsReq {
    /// Two collections merge; an action never does.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
    TokenReport {
        r#gen: u64,
        result: Result<TokenReport, SourceError>,
    },
    Prompt(Prompt),
    ActionDone {
        action: Action,
        outcome: ActionOutcome,
```

with:

```rust
    TokenReport {
        r#gen: u64,
        result: Result<TokenReport, SourceError>,
    },
    IndexState {
        r#gen: u64,
        result: Result<IndexState, SourceError>,
    },
    /// How far the TUI's refresh of the index has come, at most every 250 ms.
    IndexProgress(IndexProgress),
    /// The TUI's refresh of the index returned.
    Refreshed {
        result: Result<RefreshOutcome, SourceError>,
    },
    Prompt(Prompt),
    ActionDone {
        action: Action,
        outcome: ActionOutcome,
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
    LoadReport {
        r#gen: u64,
        query: TokenQuery,
    },
    Act(Action),
    /// Run `action` with the terminal handed back (spec §7.3).
    Suspend(Action),
    /// Begin the shutdown sequence (spec §10.2) with this exit code.
```

with:

```rust
    LoadReport {
        r#gen: u64,
        query: TokenQuery,
    },
    /// The index's state, on the report lane (spec §6.4).
    LoadIndexState {
        r#gen: u64,
        provider: ProviderId,
    },
    /// A refresh of the index, on the index lane (spec §6.4).
    RefreshIndex {
        provider: ProviderId,
    },
    Act(Action),
    /// Run `action` with the terminal handed back (spec §7.3).
    Suspend(Action),
    /// Begin the shutdown sequence (spec §10.2) with this exit code.
```

In `crates/tagteam-tui/src/state.rs`, replace:

```rust
    pub accounts: u64,
    pub history: u64,
    pub collect: u64,
    pub report: u64,
}

pub struct Refresh {
    pub next_ms: i64,
```

with:

```rust
    pub accounts: u64,
    pub history: u64,
    pub collect: u64,
    pub report: u64,
    pub index: u64,
}

pub struct Refresh {
    pub next_ms: i64,
```

In `crates/tagteam-tui/src/update.rs`, replace:

```rust
                    clear_read_error(state, BannerKind::Accounts);
                    if state.quitting.is_some() {
                        return vec![];
                    }
                    screens::history::on_accounts(state, now_ms)
                }
                Err(e) => {
                    raise(state, BannerKind::Accounts, e.message);
                    vec![]
```

with:

```rust
                    clear_read_error(state, BannerKind::Accounts);
                    if state.quitting.is_some() {
                        return vec![];
                    }
                    let mut cmds = screens::history::on_accounts(state, now_ms);
                    cmds.extend(screens::tokens::index::on_accounts(state, now_ms));
                    cmds
                }
                Err(e) => {
                    raise(state, BannerKind::Accounts, e.message);
                    vec![]
```

In `crates/tagteam-tui/src/update.rs`, replace:

```rust
        }
        Msg::TokenReport { r#gen, result } => {
            screens::tokens::on_report(state, r#gen, result, now_ms)
        }
        Msg::Prompt(prompt) => {
            state.dirty = true;
            actions::on_prompt(state, prompt);
            vec![]
```

with:

```rust
        }
        Msg::TokenReport { r#gen, result } => {
            screens::tokens::on_report(state, r#gen, result, now_ms)
        }
        Msg::IndexState { r#gen, result } => {
            screens::tokens::index::on_index_state(state, r#gen, result, now_ms)
        }
        Msg::IndexProgress(progress) => {
            screens::tokens::index::on_progress(state, progress);
            vec![]
        }
        Msg::Refreshed { result } => screens::tokens::index::on_refreshed(state, result, now_ms),
        Msg::Prompt(prompt) => {
            state.dirty = true;
            actions::on_prompt(state, prompt);
            vec![]
```

In `crates/tagteam-tui/src/update.rs`, replace:

```rust
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

/// A failed read's banner, remembering which kind raised it.
fn raise(state: &mut State, kind: BannerKind, text: String) {
```

with:

```rust
    }
}

/// Spec §6.1: the TUI collects after drawing. The runtime calls this after each frame it
/// draws. The first frame asks for the token index's first refresh (B5). The first frame that
/// shows accounts asks for the first collection of them, once no action holds the actions lane
/// (spec §7.2); until a collection has been asked for, `gens.collect` is 0. The timer collects
/// from then on (§6.2).
pub fn drawn(state: &mut State) -> Vec<Cmd> {
    let mut cmds = screens::tokens::index::first_refresh(state);
    if state.gens.collect > 0 || state.quitting.is_some() || state.running.is_some() {
        return cmds;
    }
    let shown = state.accounts.shown_ids();
    if !shown.is_empty() {
        cmds.push(state.collect_due(shown));
    }
    cmds
}

/// A failed read's banner, remembering which kind raised it.
fn raise(state: &mut State, kind: BannerKind, text: String) {
```

In `crates/tagteam-tui/src/update.rs`, replace:

```rust
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
```

with:

```rust
    if now_s != state.shown_s || state.running.is_some() || state.quitting.is_some() {
        state.shown_s = now_s;
        state.dirty = true;
    }
    // Spec §6.4: the token index's own timers, while Tokens is open.
    let mut cmds = screens::tokens::index::on_tick(state, now_ms);
    if state.quitting.is_some() || now_ms < state.refresh.next_ms {
        return cmds;
    }
    // Spec §6.2: every 30 s the views are re-read, and the shown accounts offered to the
    // collector, unless an action holds the actions lane (spec §7.2).
    state.refresh.next_ms = now_ms + REFRESH_MS;
    cmds.push(state.load_accounts());
    if state.screen == Screen::History {
        cmds.extend(screens::history::reload(state, now_ms));
    }
    let shown = state.accounts.shown_ids();
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
    ("r", "refresh"),
    ("Enter", "history of the account"),
    ("J K", "scroll the detail's windows"),
];
const HELP_TOKENS: [(&str, &str); 5] = [
    ("p", "period: day, month, session or block"),
    ("g", "group: none, account, model, or both"),
    ("[ ]", "range: 7, 30 or 90 days, or all"),
    ("Enter", "drill down: sessions, then models"),
    ("Esc  Backspace", "back up a level"),
];
const HELP_HISTORY: [(&str, &str); 3] = [
    ("[ ]", "range: 1, 7, 14 or 30 days"),
    ("r", "refresh"),
```

with:

```rust
    ("r", "refresh"),
    ("Enter", "history of the account"),
    ("J K", "scroll the detail's windows"),
];
const HELP_TOKENS: [(&str, &str); 6] = [
    ("p", "period: day, month, session or block"),
    ("g", "group: none, account, model, or both"),
    ("[ ]", "range: 7, 30 or 90 days, or all"),
    ("Enter", "drill down: sessions, then models"),
    ("Esc  Backspace", "back up a level"),
    ("r", "refresh the token index"),
];
const HELP_HISTORY: [(&str, &str); 3] = [
    ("[ ]", "range: 1, 7, 14 or 30 days"),
    ("r", "refresh"),
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
        _ => None,
    }
}

/// `r` (spec §7.1): the views re-read, and the shown accounts offered to the collector at once.
fn refresh(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if busy(state, now_ms) {
        return vec![];
    }
    let mut cmds = vec![state.load_accounts()];
```

with:

```rust
        _ => None,
    }
}

/// `r` (spec §7.1): the views re-read, and the shown accounts offered to the collector at once;
/// on Tokens, a refresh of the token index, on its own lane.
fn refresh(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.screen == Screen::Tokens {
        return screens::tokens::index::refresh_now(state, now_ms);
    }
    if busy(state, now_ms) {
        return vec![];
    }
    let mut cmds = vec![state.load_accounts()];
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
use crate::ask::ChannelAsk;
use crate::host::{Action, ActionOutcome, Host};
use crate::input::{Input, Pause};
use crate::lanes::Lane;
use crate::msg::{ActionsReq, Cmd, Msg, ReportReq, ViewsReq};
use crate::screens::tokens::TokensState;
use crate::source::{Source, SourceFactory};
use crate::state::{Screen, State};
use crate::term::{self, RealTty, Tty};
```

with:

```rust
use crate::ask::ChannelAsk;
use crate::host::{Action, ActionOutcome, Host};
use crate::input::{Input, Pause};
use crate::lanes::Lane;
use crate::msg::{ActionsReq, Cmd, IndexReq, Msg, ReportReq, ViewsReq};
use crate::screens::tokens::TokensState;
use crate::source::{Source, SourceFactory};
use crate::state::{Screen, State};
use crate::term::{self, RealTty, Tty};
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
/// The loop wakes at least this often, to look at the signal token and to tick (§4.3).
const WAKE: Duration = Duration::from_millis(250);
/// How long the first frame waits for the first `accounts` (§6.1).
const FIRST_FRAME: Duration = Duration::from_millis(100);
/// Why a hand-over of the terminal was refused (§7.3 step 1): the input thread could still read
/// keys typed for the terminal's next owner.
const NOT_PAUSED: &str = "the TUI's key reader did not pause in time; try again";

```

with:

```rust
/// The loop wakes at least this often, to look at the signal token and to tick (§4.3).
const WAKE: Duration = Duration::from_millis(250);
/// How long the first frame waits for the first `accounts` (§6.1).
const FIRST_FRAME: Duration = Duration::from_millis(100);
/// The index lane passes on a refresh's progress at most this often: the loop's own pace.
const PROGRESS_EVERY: Duration = WAKE;
/// Why a hand-over of the terminal was refused (§7.3 step 1): the input thread could still read
/// keys typed for the terminal's next owner.
const NOT_PAUSED: &str = "the TUI's key reader did not pause in time; try again";

```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
        .map_or(0, |d| d.as_millis() as i64)
}

/// The engine lanes (§4.3): views (`accounts`, `history`, store-served), report (the token
/// report, which reads the index only) and actions (the collector and every `Host` action, one
/// at a time).
pub(crate) struct Lanes {
    views: Lane<ViewsReq>,
    report: Lane<ReportReq>,
    actions: Lane<ActionsReq>,
}

impl Lanes {
```

with:

```rust
        .map_or(0, |d| d.as_millis() as i64)
}

/// The engine lanes (§4.3): views (`accounts`, `history`, store-served), report (the token
/// report, which reads the index only), index (the token index's catch-up) and actions (the
/// collector and every `Host` action, one at a time).
pub(crate) struct Lanes {
    views: Lane<ViewsReq>,
    report: Lane<ReportReq>,
    index: Lane<IndexReq>,
    actions: Lane<ActionsReq>,
}

impl Lanes {
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
                        ReportReq::Report { r#gen, query } => Msg::TokenReport {
                            r#gen,
                            result: source.token_report(&query),
                        },
                    };
                    let _ = out.send(msg);
                },
                move || {
                    let _ = died.send(Msg::LaneDied { lane: "report" });
                },
            )
        };
        let actions = {
            let (make, out, died) = (Arc::clone(sources), tx.clone(), tx.clone());
            let (host, shutdown) = (Arc::clone(host), shutdown.clone());
            Lane::spawn(
```

with:

```rust
                        ReportReq::Report { r#gen, query } => Msg::TokenReport {
                            r#gen,
                            result: source.token_report(&query),
                        },
                        ReportReq::IndexState { r#gen, provider } => Msg::IndexState {
                            r#gen,
                            result: source.token_index_state(&provider),
                        },
                    };
                    let _ = out.send(msg);
                },
                move || {
                    let _ = died.send(Msg::LaneDied { lane: "report" });
                },
            )
        };
        let index = {
            let (make, out, died) = (Arc::clone(sources), tx.clone(), tx.clone());
            Lane::spawn(
                "index",
                move || make(),
                move |source: &mut Box<dyn Source>, req: IndexReq| {
                    let IndexReq::Refresh { provider } = req;
                    // The engine reports progress per chunk; the loop is told at its own pace.
                    let mut told: Option<Instant> = None;
                    let result = source.refresh_tokens(&provider, &mut |progress| {
                        if told.is_none_or(|at| at.elapsed() >= PROGRESS_EVERY) {
                            told = Some(Instant::now());
                            let _ = out.send(Msg::IndexProgress(*progress));
                        }
                    });
                    let _ = out.send(Msg::Refreshed { result });
                },
                move || {
                    let _ = died.send(Msg::LaneDied { lane: "index" });
                },
            )
        };
        let actions = {
            let (make, out, died) = (Arc::clone(sources), tx.clone(), tx.clone());
            let (host, shutdown) = (Arc::clone(host), shutdown.clone());
            Lane::spawn(
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
        };
        Lanes {
            views,
            report,
            actions,
        }
    }

    /// §10.2 step 2: nothing more is taken, what is queued is dropped, and an idle lane wakes
    /// and returns.
    fn close(&self) {
        self.views.close();
        self.report.close();
        self.actions.close();
    }

    /// §10.2 step 5: joins every lane, and names those that panicked.
```

with:

```rust
        };
        Lanes {
            views,
            report,
            index,
            actions,
        }
    }

    /// §10.2 step 2: nothing more is taken, what is queued is dropped, and an idle lane wakes
    /// and returns. A refresh the index lane runs stops at its next chunk: its engine takes
    /// the shutdown token, which step 1 set (§9).
    fn close(&self) {
        self.views.close();
        self.report.close();
        self.index.close();
        self.actions.close();
    }

    /// §10.2 step 5: joins every lane, and names those that panicked.
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
        }
        if self.report.join().is_err() {
            died.push("report");
        }
        if self.actions.join().is_err() {
            died.push("actions");
        }
        died
```

with:

```rust
        }
        if self.report.join().is_err() {
            died.push("report");
        }
        if self.index.join().is_err() {
            died.push("index");
        }
        if self.actions.join().is_err() {
            died.push("actions");
        }
        died
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
                }
                Cmd::LoadReport { r#gen, query } => {
                    let _ = self.lanes.report.submit(ReportReq::Report { r#gen, query });
                }
                Cmd::Act(action) => {
                    let _ = self.lanes.actions.submit(ActionsReq::Act { action });
                }
                Cmd::Suspend(action) => self.suspend(action),
```

with:

```rust
                }
                Cmd::LoadReport { r#gen, query } => {
                    let _ = self.lanes.report.submit(ReportReq::Report { r#gen, query });
                }
                Cmd::LoadIndexState { r#gen, provider } => {
                    let req = ReportReq::IndexState { r#gen, provider };
                    let _ = self.lanes.report.submit(req);
                }
                Cmd::RefreshIndex { provider } => {
                    let _ = self.lanes.index.submit(IndexReq::Refresh { provider });
                }
                Cmd::Act(action) => {
                    let _ = self.lanes.actions.submit(ActionsReq::Act { action });
                }
                Cmd::Suspend(action) => self.suspend(action),
```

- [ ] **Step 4: Run the tests and see them pass**

Run:
- `cargo test -p tagteam-tui --lib` — Expected: `201 passed` (10 in `screens::tokens::index`).
- Outside the sandbox, since the TUI now refreshes the index at start-up: `cargo test -p tagteam --features test-support --test tui_pty` — Expected: `16 passed`.

- [ ] **Step 5: Format and lint**

Run:
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`

Expected: no output from the first; `Finished` and no warnings from the others.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-tui/src/screens/tokens/index.rs crates/tagteam-tui/src/screens/tokens/mod.rs crates/tagteam-tui/src/msg.rs crates/tagteam-tui/src/state.rs crates/tagteam-tui/src/update.rs crates/tagteam-tui/src/view.rs crates/tagteam-tui/src/actions.rs crates/tagteam-tui/src/runtime.rs
git commit -m "Refresh the token index on its own lane and read the report again when it changes"
```

### Task 5: Drawing the Tokens screen

The screen drawn, in `screens/tokens/table.rs` (spec §5.3).

- **The header,** three rows at most (B29): the periods with the shown one bracketed, the range, the grouping the report applies, the filter, and the drill-down's rows. Then the status line: the zone, the index (B10), `catching up`, `updating…`.
- **The table** at the terminal's width (B16, B17), the active block's figures under its row, counting on with the clock, and the total line.
- **Below it:** the selected row's detail (B11, B25), its numbers first, then the footnotes (TR §11.2, B15). They take half the body at most and leave the table three rows; what does not fit scrolls with `J`/`K`, and a last row says which lines show.
- **Scrolling:** the rows keep the selected one in view; `PageUp`/`PageDown` move a page; a resize clamps the stored scroll where it is used (R22).
- **Widths** are found once per report (B17).

**Readings of the spec this task commits to:**
- **The total follows the last row shown**, so a short report has no gap above its total, and a long one has its total at the foot of the body.
- **At 80 columns no number is cut** (B24). The period, the account, the input, output, total and cost, and the model when the report is grouped by model, always show. The text columns give way first: the period's spans compact in steps, then the account, model and project are cut by display width, the project from the left. A test draws the widest report (all time, `Session` by account and model, spans across days and years, a CJK alias 20 columns wide, a long model and 10-digit counts) at 80×24 and reads every required value in full.
- **The header is three rows at most** (B29): the periods, range and grouping; the filter and the drill-down's rows, the filter cut first; the status line. Each is cut with `…` past its row.
- **Below the total, text wraps in full** (B26, B29). The active block's figures, the detail and the footnotes are laid out by their parts, and a word wider than a row breaks by width. Nothing there is cut: the region scrolls, and `J` reaches every line.
- **The table always keeps three rows, and `J`/`K` reach every line below it** (B29). A test proves it at 80×24 under a full banner and toast, with a 2,000-column model in the filter, the detail and a footnote, and a 600-column project, each read whole by scrolling. Two more scroll through a footnote naming twenty unpriced models and a session's long lists of accounts and models.
- **The active block's figures** are `active · <elapsed> elapsed · <left> left · <n> tokens/min · <cost>/h · projected <n> tokens, <cost>`, laid out under the row at three columns in. From its `end_ms` on the row shows none (spec §6.4: "turning inactive at its end").
- **Nothing here reads a clock or a file:** `now_ms` is the caller's, and the zone is the report's.

**Depends on:** 1, 4

**Files:**
- Create: `crates/tagteam-tui/src/screens/tokens/table.rs`
- Modify: `crates/tagteam-tui/src/screens/tokens/mod.rs`
- Modify: `crates/tagteam-tui/src/actions.rs` (`J K` in the Tokens help)

**Interfaces:**
- Consumes:
  - Task 1: `words::{scaled, dollars, plural, NO_TOKEN_USAGE, token_footnotes, duration, name, MISSING, Severity}`.
  - Task 3: `TokensState` and its levels.
  - Task 4: `IndexWatch::{state, refreshing, busy}`.
  - Part A: `view::{bold, fit, selected, severity_style, width_of, wrap, body_at}`.
- Produces: CB5's `table.rs`: `render`, `bytes`, `account_name` (Task 6's dialog names accounts with it), `Widths`, `page`, `follow` and `scroll_below`. `mod.rs` re-exports `render` in place of Task 3's, and gives `J`/`K` to `scroll_below`.

**Spec:**
- TUI spec §5.3 (columns at 80 and 120; marks; the total and footnotes; drill rows; blocks and the active block; the indexing states), §5.1 (size, colour), §6.4 (the countdown between queries), §11 ("Rendering" and the token report's list).
- TR §9 (blocks, observed and inferred, the active block), §11.2 (scaled counts, dollars, the footnotes).

- [ ] **Step 1: Write the failing tests**

The tests draw the screen at 80×24 and 120×40, with and without colour, from reports built of the engine's types. The help test of Task 3 gains `J K`.

Create `crates/tagteam-tui/src/screens/tokens/table.rs` with its tests (Step 3 puts the code above them):

```rust
#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use ratatui::style::{Color, Modifier};
    use tagteam_engine::tokens::{ActiveBlock, IndexProgress, IndexState, RefreshOutcome};

    use super::*;
    use crate::msg::{Cmd, Msg};
    use crate::state::Screen;
    use crate::testutil::*;
    use crate::update::update;

    /// 2026-09-20T00:00:00Z, the day before `NOW_MS`'s.
    const YESTERDAY: i64 = 1_789_862_400_000;
    const HOUR_MS: i64 = 3_600_000;
    const DAY_MS: i64 = 24 * HOUR_MS;

    /// Tokens at 80×24, its report for the query it asked for, `rows`, then `edit`ed.
    fn showing(rows: Vec<TokenRow>, edit: impl FnOnce(&mut TokenReport)) -> State {
        let mut state = fresh(Screen::Tokens);
        let cmds = state.start();
        let [_, Cmd::LoadReport { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let mut report = token_report(query.clone(), rows);
        edit(&mut report);
        let result = Ok(report);
        update(
            &mut state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        state
    }

    /// The same, after `keys` set the query up: each key's query is answered with `rows`.
    fn showing_after(keys: &str, rows: Vec<TokenRow>) -> State {
        let mut state = fresh(Screen::Tokens);
        state.start();
        let mut last = vec![];
        for c in keys.chars() {
            last = update(&mut state, ch(c), NOW_MS);
        }
        let [Cmd::LoadReport { r#gen, query }] = last.as_slice() else {
            panic!("{last:?}");
        };
        let result = Ok(token_report(query.clone(), rows));
        update(
            &mut state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        state
    }

    fn spare() -> RowAccount {
        RowAccount::Account {
            id: id(2),
            position: 2,
            label: "a2@x.co".into(),
            alias: Some("spare".into()),
        }
    }

    /// Yesterday for accounts 1 (with lower-bound output and its cache split) and 2 (`spare`),
    /// and the day before, unattributed and unpriced.
    fn days() -> Vec<TokenRow> {
        let mut one = token_row(
            day(1),
            Some(row_account(1)),
            None,
            counts(1_200_000, 310_000, 88_000_000),
            Some(41_200_000_000),
        );
        one.counts.cache_write_5m = 3_000_000;
        one.counts.cache_write_1h = 1_100_000;
        one.unfinalized = 2;
        one.responses = 412;
        let two = token_row(
            day(1),
            Some(spare()),
            None,
            counts(300_000, 80_000, 20_100_000),
            Some(9_750_000_000),
        );
        let three = token_row(
            day(2),
            Some(RowAccount::Unattributed),
            None,
            counts(5_000, 900, 100_000),
            None,
        );
        vec![one, two, three]
    }

    #[test]
    fn a_day_report_at_80_columns() {
        // Spec §5.3: period, account, input, output, total and cost; the period once per day.
        let state = showing(days(), |_| {});
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[1..12],
            [
                " [Day] Month Session Block  30 days  group: account",
                " filter: all accounts · all models",
                " UTC · refreshed 14:12",
                " DAY         ACCOUNT        INPUT   OUTPUT   TOTAL    COST",
                " 2026-09-20  1 a1@x.co       1.2M  ≥310.0K   93.6M  $41.20",
                "             2 spare       300.0K    80.0K   20.5M   $9.75",
                " 2026-09-19  unattributed    5.0K      900  105.9K       —",
                " TOTAL                       1.5M  ≥390.9K  114.2M  $50.95",
                " 412 responses · cache write 4.1M (5m 3.0M, 1h 1.1M) · cache read 88.0M",
                " 2026-09-20 · 1 a1@x.co",
                " Output is a lower bound for 2 subagent responses.",
            ]
        );
        assert_eq!(lines[12], " API cost at list prices as of 2026-10-01.");
        assert_eq!(
            lines[23],
            "Enter sessions  p period  g group  [ ] range  Esc back  r refresh"
        );
    }

    #[test]
    fn from_120_columns_the_cache_the_responses_and_a_cost_bar_join_them() {
        let state = showing(days(), |_| {});
        let lines = draw(&state, 120, 40);
        assert_eq!(
            lines[1],
            " [Day] Month Session Block  30 days  group: account  filter: all accounts · all models"
        );
        assert_eq!(
            lines[3..8],
            [
                " DAY         ACCOUNT        INPUT   OUTPUT  CW 5M  CW 1H  CACHE R   TOTAL  RESP    COST",
                " 2026-09-20  1 a1@x.co       1.2M  ≥310.0K   3.0M   1.1M    88.0M   93.6M   412  $41.20  ████████",
                "             2 spare       300.0K    80.0K      0      0    20.1M   20.5M     1   $9.75  ██",
                " 2026-09-19  unattributed    5.0K      900      0      0   100.0K  105.9K     1       —",
                " TOTAL                       1.5M  ≥390.9K   3.0M   1.1M   108.2M  114.2M   414  $50.95",
            ]
        );
    }

    /// A session of yesterday, 09:12 to 11:40 UTC, in the tagteam checkout.
    fn session(id: &str, account: RowAccount) -> TokenRow {
        let period = PeriodKey::Session {
            session_id: id.into(),
            project: Some("/Users/michael/Code/tagteam".into()),
            first_ms: YESTERDAY + 9 * HOUR_MS + 12 * 60_000,
            last_ms: YESTERDAY + 11 * HOUR_MS + 40 * 60_000,
        };
        let counts = counts(20_000, 3_000, 900_000);
        token_row(period, Some(account), None, counts, Some(1_230_000_000))
    }

    /// 12:00 to 17:00 UTC today.
    const NOON: i64 = 1_789_992_000_000;

    /// Account 1's block of five hours from `start_ms`: active at `NOW_MS` (14:13:20) and
    /// observed when `active`, else inferred.
    fn block(start_ms: i64, active: bool) -> TokenRow {
        let end_ms = start_ms + 5 * HOUR_MS;
        let period = PeriodKey::Block {
            start_ms,
            end_ms,
            observed: active,
            active: active.then_some(ActiveBlock {
                elapsed_s: (NOW_MS - start_ms) / 1000,
                remaining_s: (end_ms - NOW_MS) / 1000,
                tokens_per_min: 41_234.6,
                cost_per_hour: Some(Cost {
                    usd_nanos: 3_200_000_000,
                    partial: false,
                }),
                projected_tokens: 9_800_000,
                projected_cost: None,
            }),
        };
        let counts = counts(400_000, 50_000, 5_000_000);
        token_row(
            period,
            Some(row_account(1)),
            None,
            counts,
            Some(5_000_000_000),
        )
    }

    #[test]
    fn a_session_shows_its_project_and_times_and_a_month_its_month() {
        let sessions = showing_after(
            "pp",
            vec![
                session("3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c", row_account(1)),
                session("88aa", spare()),
            ],
        );
        let lines = draw(&sessions, 80, 24);
        assert_eq!(
            lines[1..12],
            [
                " Day Month [Session] Block  30 days  group: account",
                " filter: all accounts · all models",
                " UTC · refreshed 14:12",
                " SESSION            PROJECT              ACCOUNT    INPUT  OUTPUT   TOTAL   COST",
                " 09-20 09:12–11:40  …chael/Code/tagteam  1 a1@x.co  20.0K    3.0K  923.0K  $1.23",
                " 09-20 09:12–11:40  …chael/Code/tagteam  2 spare    20.0K    3.0K  923.0K  $1.23",
                " TOTAL                                              40.0K    6.0K    1.8M  $2.46",
                " 1 response · cache write 0 (5m 0, 1h 0) · cache read 900.0K",
                " session 3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c · /Users/michael/Code/tagteam",
                " 2026-09-20 09:12 – 2026-09-20 11:40 · 1 a1@x.co",
                " API cost at list prices as of 2026-10-01.",
            ]
        );
        let month = PeriodKey::Month {
            year: 2026,
            month: 9,
        };
        let months = showing_after(
            "p",
            vec![token_row(
                month,
                Some(row_account(1)),
                None,
                counts(1, 2, 3),
                None,
            )],
        );
        let lines = draw(&months, 80, 24);
        assert_eq!(
            lines[1],
            " Day [Month] Session Block  12 months  group: account"
        );
        assert_eq!(lines[4], " MONTH    ACCOUNT    INPUT  OUTPUT  TOTAL  COST");
        assert_eq!(lines[5], " 2026-09  1 a1@x.co      1       2      6     —");
    }

    #[test]
    fn a_block_shows_its_span_and_whether_it_was_observed() {
        // Spec §5.3, token report §9; blocks are per account, so the header shows the
        // grouping the report applies.
        let blocks = showing_after(
            "pppggg",
            vec![block(NOON, true), block(NOON - 5 * HOUR_MS, false)],
        );
        let lines = draw(&blocks, 80, 24);
        assert_eq!(
            lines[1..10],
            [
                " Day Month Session [Block]  30 days  group: account",
                " filter: all accounts · all models",
                " UTC · refreshed 14:12",
                " BLOCK                        ACCOUNT     INPUT  OUTPUT  TOTAL    COST",
                " 09-21 12:00–17:00  observed  1 a1@x.co  400.0K   50.0K   5.5M   $5.00",
                "   active · 2h13m elapsed · 2h46m left · 41.2K tokens/min · $3.20/h",
                "   projected 9.8M tokens, —",
                " 09-21 07:00–12:00  inferred  1 a1@x.co  400.0K   50.0K   5.5M   $5.00",
                " TOTAL                                   800.0K  100.0K  10.9M  $10.00",
            ]
        );
        assert_eq!(blocks.tokens.grouping(), TokenGrouping::None);
    }

    #[test]
    fn the_active_block_counts_down_between_queries_and_is_no_longer_active_at_its_end() {
        let blocks = showing_after("ppp", vec![block(NOON, true)]);
        let figures = |at: i64| draw_at(&blocks, 80, 24, at)[6].clone();
        assert!(figures(NOW_MS).starts_with("   active · 2h13m elapsed · 2h46m left"));
        assert!(
            figures(NOW_MS + 61_000).starts_with("   active · 2h14m elapsed · 2h45m left"),
            "from its start and end, with no query between"
        );
        let end = NOON + 5 * HOUR_MS;
        assert!(figures(end - 60_000).starts_with("   active · 4h59m elapsed · 1m left"));
        assert_eq!(
            figures(end),
            " TOTAL                                   400.0K   50.0K   5.5M  $5.00",
            "no longer active, until the query its end asks for returns"
        );
    }

    #[test]
    fn the_account_and_model_columns_follow_the_grouping() {
        // Spec §5.3: the model column when the grouping has the model; the account column
        // always, listing a row's accounts when the rows are not split by account.
        let mut opus = token_row(day(1), None, Some("claude-opus-5-5"), counts(1, 2, 3), None);
        opus.accounts = vec![row_account(1), spare()];
        let models = showing_after("g", vec![opus.clone()]);
        let lines = draw(&models, 80, 24);
        assert_eq!(
            lines[1],
            " [Day] Month Session Block  30 days  group: model"
        );
        assert_eq!(
            lines[4],
            " DAY         ACCOUNT             MODEL            INPUT  OUTPUT  TOTAL  COST"
        );
        assert_eq!(
            lines[5],
            " 2026-09-20  1 a1@x.co, 2 spare  claude-opus-5-5      1       2      6     —"
        );
        opus.model = None;
        let none = showing_after("ggg", vec![opus]);
        let lines = draw(&none, 80, 24);
        assert_eq!(lines[1], " [Day] Month Session Block  30 days  group: none");
        assert_eq!(
            lines[4],
            " DAY         ACCOUNT             INPUT  OUTPUT  TOTAL  COST"
        );
    }

    #[test]
    fn lower_bounds_are_marked_and_stand_out_and_a_missing_cost_is_a_dash() {
        // Spec §5.3: `≥` for output with unfinalized responses and for a partial cost, `—` for
        // none; with colour the marked cells are yellow, without it bold.
        let mut rows = days();
        rows[1].cost = Some(Cost {
            usd_nanos: 9_750_000_000,
            partial: true,
        });
        for color in [true, false] {
            let mut state = showing(rows.clone(), |_| {});
            state.color = color;
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let lines = text_of(&buf);
            assert_eq!(
                lines[6],
                "             2 spare       300.0K    80.0K   20.5M   ≥$9.75"
            );
            assert!(lines[7].ends_with("       —"), "{}", lines[7]);
            for needle in ["≥310.0K", "≥$9.75"] {
                let (x, y) = locate(&buf, needle).unwrap();
                let cell = &buf[(x, y)];
                if color {
                    assert_eq!(cell.fg, Color::Yellow, "{needle}");
                } else {
                    assert_eq!(cell.fg, Color::Reset, "{needle}");
                    assert!(cell.modifier.contains(Modifier::BOLD), "{needle}");
                }
            }
            let (x, y) = locate(&buf, "$41.20").unwrap();
            assert_eq!(buf[(x, y)].fg, Color::Reset, "a whole cost is plain");
        }
    }

    #[test]
    fn the_footnotes_say_what_the_report_leaves_out() {
        // Spec §5.3, token report §11.2: with the TUI's `≥` and the unreadable files.
        let state = showing(days(), |r| {
            r.unpriced_models = vec!["claude-test-9".into()];
            r.index.skipped_lines = 12;
            r.index.unreadable_files = 3;
            r.price_overrides = vec!["claude-opus-5-5".into()];
        });
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[11..17],
            [
                " No price for claude-test-9: counted without cost (≥ marks a partial cost).",
                " Output is a lower bound for 2 subagent responses.",
                " 12 transcript lines could not be read, so these totals may be low.",
                " 3 transcript files could not be read; the next refresh tries again.",
                " API cost at list prices as of 2026-10-01, with config.toml's for",
                " claude-opus-5-5.",
            ]
        );
    }

    #[test]
    fn the_header_says_how_current_the_index_is() {
        // Spec §5.3 "Indexing".
        let mut state = showing(days(), |_| {});
        assert_eq!(state.tokens.watch().refreshing(), None);
        assert!(crate::update::drawn(&mut state).contains(&Cmd::RefreshIndex { provider: cc() }));
        assert_eq!(draw(&state, 80, 24)[3], " UTC · indexing transcripts…");
        let progress = IndexProgress {
            bytes_done: 1_200_000_000,
            bytes_total: 3_300_000_000,
            ..IndexProgress::default()
        };
        update(&mut state, Msg::IndexProgress(progress), NOW_MS);
        assert_eq!(
            draw(&state, 80, 24)[3],
            " UTC · indexing transcripts 1.2 GB / 3.3 GB"
        );
        update(
            &mut state,
            Msg::Refreshed {
                result: Ok(RefreshOutcome::Busy),
            },
            NOW_MS,
        );
        assert_eq!(draw(&state, 80, 24)[3], " UTC · catching up elsewhere");
        // A report of an index that has never completed a refresh (token report §14).
        let partial = IndexState {
            refreshed_at_ms: None,
            ..IndexState::default()
        };
        let mut state = showing(days(), |r| r.index = partial);
        assert_eq!(draw(&state, 80, 24)[3], " UTC · catching up");
        crate::screens::tokens::requery(&mut state, NOW_MS);
        assert_eq!(draw(&state, 80, 24)[3], " UTC · catching up · updating…");
        // Refreshed on another day: its date.
        let state = showing(days(), |r| {
            r.index.refreshed_at_ms = Some(YESTERDAY + HOUR_MS)
        });
        assert_eq!(draw(&state, 80, 24)[3], " UTC · refreshed 2026-09-20 01:00");
    }

    #[test]
    fn loading_and_an_empty_report_say_so() {
        let mut state = fresh(Screen::Tokens);
        state.start();
        assert_eq!(draw(&state, 80, 24)[4], " loading…");
        let empty = showing(vec![], |_| {});
        let lines = draw(&empty, 80, 24);
        assert_eq!(lines[4], " No token usage in this range.");
        assert_eq!(lines[5], "");
    }

    #[test]
    fn the_drill_down_is_named_in_the_header() {
        let mut state = showing(days(), |_| {});
        let cmds = update(&mut state, key(KeyCode::Enter), NOW_MS);
        let [Cmd::LoadReport { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let rows = vec![session("3f2a9c1d", row_account(1))];
        let result = Ok(token_report(query.clone(), rows));
        update(
            &mut state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[1],
            " [Day] Month Session Block  30 days  group: account"
        );
        assert_eq!(
            lines[2],
            " filter: all accounts · all models  › 2026-09-20 1 a1@x.co"
        );
    }

    #[test]
    fn the_cache_write_is_one_column_where_its_halves_do_not_fit() {
        let mut rows = days();
        if let Some(RowAccount::Account { alias, .. }) = &mut rows[1].account {
            *alias = Some("an-alias-thirty-five-characters-long".into());
        }
        let state = showing(rows, |_| {});
        let lines = draw(&state, 120, 40);
        assert!(lines[3].contains("  CACHE W  CACHE R  "), "{}", lines[3]);
        assert!(
            lines[5].contains("2 an-alias-thirty-five-characters-long  "),
            "{}",
            lines[5]
        );
    }

    /// `n` days of account 1, newest first.
    fn many_days(n: i64) -> Vec<TokenRow> {
        (0..n)
            .map(|d| {
                token_row(
                    day(d),
                    Some(row_account(1)),
                    None,
                    counts(1, 2, 3),
                    Some(10_000_000),
                )
            })
            .collect()
    }

    #[test]
    fn a_long_report_scrolls_to_keep_the_selection_in_view_and_a_resize_keeps_it_there() {
        // R22: the stored scroll is clamped where it is used.
        let mut state = showing(many_days(30), |_| {});
        state.size = (120, 40);
        update(&mut state, key(KeyCode::End), NOW_MS);
        let lines = draw(&state, 120, 40);
        assert!(
            lines.iter().any(|l| l.starts_with(" 2026-08-23")),
            "the last row shows"
        );
        for (w, h) in [(80, 24), (100, 30)] {
            state.size = (w, h);
            let lines = draw(&state, w, h);
            let last = lines.iter().position(|l| l.starts_with(" 2026-08-23"));
            assert!(last.is_some(), "{w}x{h}: {lines:?}");
            assert!(lines[last.unwrap() + 1].starts_with(" TOTAL"), "{w}x{h}");
        }
        update(&mut state, key(KeyCode::Home), NOW_MS);
        assert_eq!(
            draw(&state, 100, 30)[4],
            " 2026-09-21  1 a1@x.co      1       2      6  $0.01"
        );
        update(&mut state, key(KeyCode::PageDown), NOW_MS);
        let page = page(&state, crate::view::body_at(&state), NOW_MS);
        assert_eq!(state.tokens.selected_index(), Some(page));
        update(&mut state, key(KeyCode::PageUp), NOW_MS);
        assert_eq!(state.tokens.selected_index(), Some(0));
    }

    /// `text` with its white space taken out: what a person reads across wrapped rows.
    fn squash(text: &str) -> String {
        text.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// Whether `lines`, read across their rows, hold `text` whole.
    fn reads(lines: &[String], text: &str) -> bool {
        squash(&lines.concat()).contains(&squash(text))
    }

    /// Account 3, whose alias and label are both wider than any column at 80.
    fn wide() -> RowAccount {
        RowAccount::Account {
            id: id(3),
            position: 3,
            label: "averyveryverylongname.with.a.long.tail@example.com".into(),
            alias: Some("山田太郎のアカウント".into()),
        }
    }

    /// The widest a report gets (spec §5.3): all time, by session, account and model; a span
    /// from one year into another; names wider than their columns; ten-digit counts and a
    /// partial six-figure cost.
    fn widest() -> State {
        let long = PeriodKey::Session {
            session_id: "3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c".into(),
            project: Some("/Users/michael/Code/ViuMore/VM.Portal-feature-user-manuals".into()),
            first_ms: YESTERDAY - 365 * DAY_MS + 9 * HOUR_MS + 12 * 60_000,
            last_ms: YESTERDAY - 364 * DAY_MS + 11 * HOUR_MS + 40 * 60_000,
        };
        let model = "claude-opus-5-5-with-a-very-long-suffix";
        let ten_digits = counts(9_876_543_210, 1_234_567_890, 5_555_555_555);
        let mut big = token_row(long, Some(wide()), Some(model), ten_digits, None);
        big.unfinalized = 3;
        big.cost = Some(Cost {
            usd_nanos: 123_456_789_000_000,
            partial: true,
        });
        let short = PeriodKey::Session {
            session_id: "88aa".into(),
            project: Some("/w".into()),
            first_ms: YESTERDAY + 9 * HOUR_MS,
            last_ms: YESTERDAY + 10 * HOUR_MS,
        };
        let small = token_row(
            short,
            Some(row_account(1)),
            Some("claude-sonnet-5-5"),
            counts(1, 2, 3),
            Some(10_000_000),
        );
        showing_after("ppgg]]", vec![big, small])
    }

    #[test]
    fn every_required_value_shows_whole_at_80_columns_in_the_widest_report() {
        // Spec §5.3 at 80 columns: the period, account, model, input, output, total and cost
        // always show. The numbers are never cut; the span compacts (B24), and the names give
        // way, cut by display width.
        let state = widest();
        let lines = draw(&state, 80, 24);
        let head = lines
            .iter()
            .position(|l| l.starts_with(" SESSION"))
            .unwrap();
        assert_eq!(
            lines[head],
            " SESSION               PROJE…  ACCOU…  MODEL   INPUT  OUTPUT  TOTAL         COST"
        );
        let big = &lines[head + 1];
        for value in [
            "09-20 09:12–11:40+1d",
            "9.9B",
            "≥1.2B",
            "16.7B",
            "≥$123456.79",
        ] {
            assert!(big.contains(value), "{value} in {big:?}");
        }
        assert!(
            big.contains(" 3 山…"),
            "the account by position, cut: {big:?}"
        );
        assert!(big.contains("claud…"), "the model, cut: {big:?}");
        assert!(big.contains("…nuals"), "the project keeps its end: {big:?}");
        let small = &lines[head + 2];
        assert!(small.contains("09-20 09:00–10:00"), "{small:?}");
        let total = &lines[head + 3];
        for value in ["9.9B", "≥1.2B", "16.7B", "≥$123456.80"] {
            assert!(total.contains(value), "{value} in {total:?}");
        }
        // At 120 the cache, the responses and the bar take room, and the span drops its years;
        // wider still, it shows them.
        let at_120 = draw(&state, 120, 40);
        assert!(
            at_120
                .iter()
                .any(|l| l.contains(" 09-20 09:12–09-21 11:40  ")),
            "{at_120:?}"
        );
        let at_160 = draw(&state, 160, 40);
        assert!(
            at_160
                .iter()
                .any(|l| l.contains("2025-09-20 09:12–2025-09-21 11:40")),
            "{at_160:?}"
        );
        // Day by account and model: the CJK alias is cut on a character, never in one.
        let day = showing_after(
            "g",
            vec![token_row(
                day(1),
                Some(wide()),
                Some("claude-opus-5-5-with-a-very-long-suffix"),
                counts(1, 2, 3),
                None,
            )],
        );
        let lines = draw(&day, 80, 24);
        assert!(lines[5].starts_with(" 2026-09-20  3 山田"), "{}", lines[5]);
    }

    /// A session as the engine gives one: a UUID, a long project, two models and the cache.
    fn realistic() -> TokenRow {
        let period = PeriodKey::Session {
            session_id: "3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c".into(),
            project: Some("/Users/michael/Code/ViuMore/VM.Portal-feature-user-manuals".into()),
            first_ms: YESTERDAY + 9 * HOUR_MS + 12 * 60_000,
            last_ms: YESTERDAY + 11 * HOUR_MS + 40 * 60_000,
        };
        let mut row = token_row(
            period,
            Some(row_account(1)),
            None,
            counts(1_200_000, 310_000, 88_000_000),
            Some(41_200_000_000),
        );
        row.counts.cache_write_5m = 3_000_000;
        row.counts.cache_write_1h = 1_100_000;
        row.responses = 412;
        row.models = vec!["claude-opus-5-5".into(), "claude-sonnet-5-5".into()];
        row
    }

    /// What the selected row's detail must let a person read (B25).
    const DETAIL: [&str; 7] = [
        "412 responses · cache write 4.1M (5m 3.0M, 1h 1.1M) · cache read 88.0M",
        "session 3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c",
        "/Users/michael/Code/ViuMore/VM.Portal-feature-user-manuals",
        "2026-09-20 09:12 – 2026-09-20 11:40",
        "1 a1@x.co",
        "claude-opus-5-5, claude-sonnet-5-5",
        "API cost at list prices as of 2026-10-01.",
    ];

    #[test]
    fn a_sessions_detail_shows_its_numbers_first_and_every_value_whole() {
        // B25: the numbers first, never cut; then the session's text, wrapped.
        let state = showing_after("pp", vec![realistic()]);
        let lines = draw(&state, 80, 24);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        assert_eq!(
            lines[total + 1],
            " 412 responses · cache write 4.1M (5m 3.0M, 1h 1.1M) · cache read 88.0M"
        );
        let below = &lines[total + 1..];
        for value in DETAIL {
            assert!(reads(below, value), "{value}: {below:?}");
        }
        assert!(
            !lines.iter().any(|l| l.contains("J K:")),
            "it all fits: {lines:?}"
        );
    }

    #[test]
    fn under_a_full_banner_and_toast_j_and_k_reach_every_value_of_the_detail_and_the_notes() {
        // R21 at 80×24: a three-row banner and a three-row toast leave the body 17 rows. The
        // table keeps three rows; the detail and the notes below it scroll (B25).
        let mut rows = vec![realistic()];
        rows.extend((1..=9).map(|n| session(&format!("s-{n}"), row_account(1))));
        rows[0].unfinalized = 1;
        let mut state = showing_after("pp", rows);
        if let Some(report) = state.tokens.levels[0].report.as_mut() {
            report.unpriced_models = (1..=12).map(|n| format!("claude-test-{n}")).collect();
            report.index.skipped_lines = 12;
            report.index.unreadable_files = 3;
            report.price_overrides = vec!["claude-opus-5-5".into(), "claude-sonnet-5-5".into()];
        }
        let words = "the store could not be opened because another tagteam holds its lock";
        state.banner = Some([words; 3].join("; "));
        state.toast([words; 3].join("; "), NOW_MS);
        let notes = [
            "No price for claude-test-1, claude-test-2, claude-test-3, claude-test-4, claude-test-5, claude-test-6, claude-test-7, claude-test-8, claude-test-9, claude-test-10, claude-test-11, claude-test-12: counted without cost (≥ marks a partial cost).",
            "Output is a lower bound for 1 subagent response.",
            "12 transcript lines could not be read, so these totals may be low.",
            "3 transcript files could not be read; the next refresh tries again.",
            "API cost at list prices as of 2026-10-01, with config.toml's for claude-opus-5-5, claude-sonnet-5-5.",
        ];
        let lines = draw(&state, 80, 24);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        assert!(total >= 4 + 1 + 3, "the table keeps three rows: {lines:?}");
        assert_eq!(
            lines[total + 1],
            format!(" {}", DETAIL[0]),
            "the numbers first"
        );
        assert!(lines[20].starts_with(" J K: lines 1–"), "{lines:?}");
        let mut seen = lines[total + 1..21].to_vec();
        for _ in 0..40 {
            update(&mut state, ch('J'), NOW_MS);
            let lines = draw(&state, 80, 24);
            let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
            seen.extend(lines[total + 1..21].iter().cloned());
        }
        for value in DETAIL[..6].iter().chain(&notes) {
            assert!(reads(&seen, value), "{value}: {seen:?}");
        }
        for _ in 0..40 {
            update(&mut state, ch('K'), NOW_MS);
        }
        let lines = draw(&state, 80, 24);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        assert_eq!(
            lines[total + 1],
            format!(" {}", DETAIL[0]),
            "K goes back to the top"
        );
        assert!(
            lines[21].starts_with("the store could not"),
            "the toast: {lines:?}"
        );
    }

    #[test]
    fn the_header_keeps_three_rows_and_a_long_filter_is_cut() {
        // B29: the periods, the filter and the status line take a row each at 80 columns. The
        // filter, which its dialog shows whole, is cut with `…`; the status line shows whole.
        let alias = "a-rather-long-alias-for-the-work-account";
        let label = "averyveryverylongname.with.a.long.tail@example.com";
        let model = "claude-opus-5-5-with-a-very-long-suffix-and-then-some-more-of-it-yet";
        let mut accounts = many(2);
        accounts[1].row.alias = Some(alias.into());
        accounts[1].row.label = label.into();
        accounts[1].row.email = Some(label.into());
        let mut state = showing(days(), |_| {});
        state.accounts.apply(snapshot(accounts));
        state.tokens.accounts = Some(vec![tagteam_engine::tokens::AccountSel::Account(id(2))]);
        state.tokens.models = Some(vec![model.into()]);
        crate::update::drawn(&mut state);
        let progress = IndexProgress {
            bytes_done: 1_200_000_000,
            bytes_total: 3_300_000_000,
            ..IndexProgress::default()
        };
        update(&mut state, Msg::IndexProgress(progress), NOW_MS);
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[1],
            " [Day] Month Session Block  30 days  group: account"
        );
        assert!(
            lines[2].starts_with(&format!(" filter: 2 {alias} (avery")),
            "{lines:?}"
        );
        assert!(lines[2].ends_with('…'), "{lines:?}");
        assert!(width_of(&lines[2]) <= 80, "{lines:?}");
        assert_eq!(lines[3], " UTC · indexing transcripts 1.2 GB / 3.3 GB");
        assert!(lines[4].starts_with(" DAY"), "{lines:?}");
    }

    #[test]
    fn a_long_drill_down_keeps_its_start_on_the_filters_row() {
        // B29: the rows a drill-down came through share the filter's row. The filter gives way
        // first, down to `FILTER_LEAST` columns, then they are cut; the header keeps three rows.
        let mut state = widest();
        let rows = state.tokens.report().map(|r| r.rows[..1].to_vec());
        let cmds = update(&mut state, key(KeyCode::Enter), NOW_MS);
        let [Cmd::LoadReport { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let rows = rows.unwrap_or_default();
        let result = Ok(token_report(query.clone(), rows));
        update(
            &mut state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[1..4],
            [
                " Day Month [Session] Block  all  group: account, model",
                " filter: all account…  › session 3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c 3 山田太…",
                " UTC · refreshed 14:12",
            ]
        );
        assert!(lines[4].starts_with(" SESSION"), "{lines:?}");
    }

    #[test]
    fn at_80x24_a_2000_column_model_leaves_the_table_three_rows_and_the_detail_reachable() {
        // B29, by construction: under a full banner and toast the body is 17 rows. The header
        // takes three at most; the table's heading and total two; what lies below the total
        // half the body at most and never the table's last three. Below the total nothing is
        // cut: it scrolls, and J reaches every line of it.
        let long = format!("claude-{}", "x".repeat(1993));
        let project = format!("/Users/michael/{}", "p".repeat(600));
        let mut big = realistic();
        if let PeriodKey::Session { project: p, .. } = &mut big.period {
            *p = Some(project.clone());
        }
        big.models = vec![long.clone(), "claude-sonnet-5-5".into()];
        let mut rows = vec![big];
        rows.extend((1..=9).map(|n| session(&format!("s-{n}"), row_account(1))));
        let mut state = showing_after("pp", rows);
        state.tokens.models = Some(vec![long.clone()]);
        if let Some(report) = state.tokens.levels[0].report.as_mut() {
            report.unpriced_models = vec![long.clone()];
        }
        let words = "the store could not be opened because another tagteam holds its lock";
        state.banner = Some([words; 3].join("; "));
        state.toast([words; 3].join("; "), NOW_MS);
        let lines = draw(&state, 80, 24);
        assert!(lines.iter().all(|l| width_of(l) <= 80), "{lines:?}");
        assert_eq!(
            lines[4],
            " Day Month [Session] Block  30 days  group: account"
        );
        assert!(
            lines[5].starts_with(" filter: all accounts · claude-xxxx"),
            "{lines:?}"
        );
        assert!(lines[5].ends_with('…'), "{lines:?}");
        assert!(lines[6].starts_with(" UTC · "), "{lines:?}");
        assert!(lines[7].starts_with(" SESSION"), "{lines:?}");
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        assert!(total >= 8 + 3, "the table keeps three rows: {lines:?}");
        assert_eq!(lines[total + 1], format!(" {}", DETAIL[0]), "{lines:?}");
        assert!(lines[20].starts_with(" J K: lines 1–"), "{lines:?}");
        assert!(lines[21].starts_with("the store could not"), "{lines:?}");
        // Every line below the total, read by scrolling: each long piece whole.
        let seen = scrolled_below(&mut state);
        for value in [
            project.clone(),
            format!("{long}, claude-sonnet-5-5"),
            format!("No price for {long}: counted without cost (≥ marks a partial cost)."),
            "API cost at list prices as of 2026-10-01.".to_owned(),
        ] {
            assert!(reads(&seen, &value), "{seen:?}");
        }
        assert!(!seen.iter().any(|l| l.contains('…')), "{seen:?}");
        assert!(seen.iter().all(|l| width_of(l) <= 80), "{seen:?}");
        let lines = draw(&state, 80, 24);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        assert!(
            total >= 8 + 3,
            "scrolled, the table keeps three rows: {lines:?}"
        );
    }

    #[test]
    fn j_scrolls_through_a_footnote_longer_than_three_rows_to_its_last_name() {
        // B29: text below the total scrolls, so it wraps in full. A footnote naming twenty
        // unpriced models runs past three rows, and J reaches its last name and its reason.
        let mut state = showing_after("pp", vec![realistic()]);
        let names: Vec<String> = (1..=20).map(|n| format!("claude-model-{n:02}")).collect();
        if let Some(report) = state.tokens.levels[0].report.as_mut() {
            report.unpriced_models = names.clone();
        }
        let note = format!(
            "No price for {}: counted without cost (≥ marks a partial cost).",
            names.join(", ")
        );
        assert!(width_of(&note) > 3 * 79, "longer than three rows");
        let seen = scrolled_below(&mut state);
        assert!(reads(&seen, &note), "{seen:?}");
        let lines = draw(&state, 80, 24);
        let tail = "claude-model-20: counted without cost (≥ marks a partial cost).";
        assert!(reads(&lines, tail), "the last window: {lines:?}");
    }

    #[test]
    fn j_reaches_every_account_and_model_of_a_session_whose_lists_wrap_past_three_rows() {
        // B29: the detail's lists wrap in full below the total, and J reaches every name.
        let mut row = realistic();
        row.accounts = (1..=12)
            .map(|n| RowAccount::Account {
                id: id(n),
                position: n,
                label: format!("account-{n:02}@example.com"),
                alias: None,
            })
            .collect();
        row.models = (1..=15)
            .map(|n| format!("claude-model-family-{n:02}"))
            .collect();
        let accounts: Vec<String> = row.accounts.iter().map(account_name).collect();
        let accounts = accounts.join(", ");
        let models = row.models.join(", ");
        assert!(width_of(&accounts) > 3 * 79, "longer than three rows");
        assert!(width_of(&models) > 3 * 79, "longer than three rows");
        let mut state = showing_after("pp", vec![row]);
        let seen = scrolled_below(&mut state);
        assert!(reads(&seen, &accounts), "{seen:?}");
        assert!(reads(&seen, &models), "{seen:?}");
    }

    /// Every line below the total at 80×24, read as a person reads it: the first window, then
    /// the line each `J` brings in, until the hint says the last shows.
    fn scrolled_below(state: &mut State) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        loop {
            let lines = draw(state, 80, 24);
            let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
            let hint = lines
                .iter()
                .position(|l| l.starts_with(" J K: lines "))
                .unwrap_or_else(|| panic!("it scrolls: {lines:?}"));
            let [from, to, of] = lines[hint]
                .split(|c: char| !c.is_ascii_digit())
                .filter(|n| !n.is_empty())
                .map(|n| n.parse::<usize>().unwrap())
                .collect::<Vec<_>>()[..]
            else {
                panic!("{}", lines[hint]);
            };
            for (i, line) in lines[total + 1..hint].iter().enumerate() {
                if from - 1 + i == seen.len() {
                    seen.push(line.clone());
                }
            }
            if to == of {
                assert_eq!(seen.len(), of, "{seen:?}");
                return seen;
            }
            update(state, ch('J'), NOW_MS);
        }
    }

    #[test]
    fn a_reports_widths_are_found_once_and_anew_for_the_next() {
        // B17: a year by session is thousands of rows; a frame lays them out, and does not
        // measure every one of them again.
        let mut state = showing(many_days(30), |_| {});
        let found = |state: &State| state.tokens.level().widths.0.get().cloned();
        assert_eq!(found(&state), None, "not drawn yet");
        draw(&state, 80, 24);
        let first = found(&state).expect("found by the first frame");
        draw(&state, 120, 40);
        assert_eq!(found(&state).as_ref(), Some(&first), "whatever the width");
        let again = crate::screens::tokens::requery(&mut state, NOW_MS);
        let [Cmd::LoadReport { r#gen, query }] = again.as_slice() else {
            panic!("{again:?}");
        };
        let result = Ok(token_report(query.clone(), days()));
        update(
            &mut state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        assert_eq!(found(&state), None, "a new report is measured anew");
        draw(&state, 80, 24);
        assert!(found(&state).is_some_and(|w| w != first));
    }

    #[test]
    fn byte_counts_read_in_decimal_units() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1_000), "1.0 kB");
        assert_eq!(bytes(1_234_567), "1.2 MB");
        assert_eq!(bytes(3_420_000_000), "3.4 GB");
        assert_eq!(bytes(u64::MAX), "18446744.1 TB");
    }
}
```

Declare the module, so that Step 2 builds its tests:

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
pub mod index;
```

with:

```rust
pub mod index;
mod table;
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
            "range: 7, 30 or 90 days, or all",
            "drill down: sessions, then models",
            "back up a level",
            "refresh the token index",
        ] {
            assert!(lines.iter().any(|l| l.contains(what)), "{what}: {lines:?}");
        }
    }
```

with:

```rust
            "range: 7, 30 or 90 days, or all",
            "drill down: sessions, then models",
            "back up a level",
            "refresh the token index",
            "scroll the detail and the notes",
        ] {
            assert!(lines.iter().any(|l| l.contains(what)), "{what}: {lines:?}");
        }
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p tagteam-tui --lib screens::tokens::table`

Expected: FAIL: the build stops with errors such as ``error[E0412]: cannot find type `TokenRow` in this scope`` and ``error[E0412]: cannot find type `State` in this scope``: the new module holds only its tests so far.

- [ ] **Step 3: Draw the screen**

In `crates/tagteam-tui/src/screens/tokens/table.rs`, put this above the tests, with one blank line between:

```rust
//! The Tokens screen drawn (spec §5.3): its header, the report's table at the terminal's width,
//! the total line, the selected row's detail, and the footnotes. It reads nothing but the state.

use std::cell::OnceCell;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tagteam_engine::tokens::{
    AccountSel, Cost, PeriodKey, RowAccount, TokenCounts, TokenGrouping, TokenPeriod, TokenReport,
    TokenRow,
};
use tagteam_engine::words::{self, Severity};

use super::TokensState;
use crate::state::State;
use crate::view::{bold, fit, selected, severity_style, width_of, wrap};

/// From this width the table adds the cache, the response count and a cost bar (spec §5.3).
pub const WIDE: usize = 120;
/// Drawn while a query for a report not yet read runs.
pub const LOADING: &str = "loading…";
/// The cost bar's cells.
const BAR: usize = 8;
/// The table keeps this many rows at least; the detail and the notes below it scroll (B25).
const TABLE_MIN: usize = 3;
/// The filter gives way to the drill-down on their row down to this many columns (B29).
const FILTER_LEAST: usize = 20;
/// The account and model columns are cut no narrower than this while a more compact period can
/// make room (B24)...
const TEXT_MIN: usize = 6;
/// ... and the project column no narrower than this.
const PROJECT_MIN: usize = 4;
/// How many ways a session's or a block's span can be written (`span`).
const SPAN_LEVELS: usize = 4;
/// The active block's figures start this far in.
const FIGURES_INDENT: usize = 3;
/// A lower bound (spec §5.3): output with unfinalized responses, a partial cost.
const AT_LEAST: &str = "≥";

/// The table's columns, in the order they are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Col {
    /// The day, month, session's times or block's span.
    Period,
    /// A block's `observed` or `inferred`.
    Kind,
    Project,
    Account,
    Model,
    Input,
    Output,
    CacheWrite,
    CacheWrite5m,
    CacheWrite1h,
    CacheRead,
    Total,
    Responses,
    Cost,
    Bar,
}

impl Col {
    const ALL: [Col; 15] = [
        Col::Period,
        Col::Kind,
        Col::Project,
        Col::Account,
        Col::Model,
        Col::Input,
        Col::Output,
        Col::CacheWrite,
        Col::CacheWrite5m,
        Col::CacheWrite1h,
        Col::CacheRead,
        Col::Total,
        Col::Responses,
        Col::Cost,
        Col::Bar,
    ];

    fn head(self, period: TokenPeriod) -> &'static str {
        match self {
            Col::Period => match period {
                TokenPeriod::Day => "DAY",
                TokenPeriod::Month => "MONTH",
                TokenPeriod::Session => "SESSION",
                TokenPeriod::Block => "BLOCK",
            },
            Col::Kind | Col::Bar => "",
            Col::Project => "PROJECT",
            Col::Account => "ACCOUNT",
            Col::Model => "MODEL",
            Col::Input => "INPUT",
            Col::Output => "OUTPUT",
            Col::CacheWrite => "CACHE W",
            Col::CacheWrite5m => "CW 5M",
            Col::CacheWrite1h => "CW 1H",
            Col::CacheRead => "CACHE R",
            Col::Total => "TOTAL",
            Col::Responses => "RESP",
            Col::Cost => "COST",
        }
    }

    /// Text columns share what the fixed ones leave, and are cut to it.
    fn text(self) -> bool {
        matches!(self, Col::Project | Col::Account | Col::Model)
    }

    /// Numbers are right-aligned.
    fn number(self) -> bool {
        !matches!(
            self,
            Col::Period | Col::Kind | Col::Project | Col::Account | Col::Model | Col::Bar
        )
    }
}

/// A cell's text, and a style for when it says `≥`.
struct Cell {
    text: String,
    style: Style,
}

impl Cell {
    fn plain(text: impl Into<String>) -> Cell {
        Cell {
            text: text.into(),
            style: Style::new(),
        }
    }
}

/// `n` bytes as the header shows a refresh's progress: decimal units to one decimal place
/// (`1.2 GB`), rounded half up.
pub fn bytes(n: u64) -> String {
    const UNITS: [(u128, &str); 4] = [
        (1_000, "kB"),
        (1_000_000, "MB"),
        (1_000_000_000, "GB"),
        (1_000_000_000_000, "TB"),
    ];
    if n < 1_000 {
        return format!("{n} B");
    }
    let tenths = |unit: u128| (u128::from(n) * 10 + unit / 2) / unit;
    let (unit, suffix) = UNITS
        .iter()
        .copied()
        .find(|&(unit, _)| tenths(unit) < 10_000)
        .unwrap_or(UNITS[3]);
    let t = tenths(unit);
    format!("{}.{} {suffix}", t / 10, t % 10)
}

/// `ms` in `tz`.
fn zoned(ms: i64, tz: &TimeZone) -> Option<jiff::Zoned> {
    Some(Timestamp::from_millisecond(ms).ok()?.to_zoned(tz.clone()))
}

/// `ms` in `tz` as `2026-10-14 09:12`.
fn when(ms: i64, tz: &TimeZone) -> String {
    zoned(ms, tz).map_or_else(
        || words::MISSING.to_owned(),
        |z| z.strftime("%Y-%m-%d %H:%M").to_string(),
    )
}

/// A span in `tz` (B16, B24), written at `level`, each more compact than the one before:
/// 0. `10-14 09:12–11:40`, the year added when it is not `now_ms`'s, and the second date when
///    it ends on another day;
/// 1. the same without years;
/// 2. the end's time alone, with `+Nd` when it ends `N` days later: `10-14 23:50–00:30+1d`;
/// 3. the start alone, which the selected row's detail completes (B25).
fn span(start_ms: i64, end_ms: i64, tz: &TimeZone, now_ms: i64, level: usize) -> String {
    let (Some(a), Some(b), Some(now)) = (zoned(start_ms, tz), zoned(end_ms, tz), zoned(now_ms, tz))
    else {
        return words::MISSING.to_owned();
    };
    let date = |z: &jiff::Zoned| {
        if level == 0 && z.year() != now.year() {
            z.strftime("%Y-%m-%d").to_string()
        } else {
            z.strftime("%m-%d").to_string()
        }
    };
    let start = format!("{} {}", date(&a), a.strftime("%H:%M"));
    let days = (b.date() - a.date()).get_days();
    match level {
        _ if level >= 3 => start,
        _ if days == 0 => format!("{start}–{}", b.strftime("%H:%M")),
        2 => format!("{start}–{}+{days}d", b.strftime("%H:%M")),
        _ => format!("{start}–{} {}", date(&b), b.strftime("%H:%M")),
    }
}

/// A row's period as its first column says it, a span written at `level` (`span`).
fn period_text(period: &PeriodKey, tz: &TimeZone, now_ms: i64, level: usize) -> String {
    match period {
        PeriodKey::Day { date } => {
            format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
        }
        PeriodKey::Month { year, month } => format!("{year:04}-{month:02}"),
        PeriodKey::Session {
            first_ms, last_ms, ..
        } => span(*first_ms, *last_ms, tz, now_ms, level),
        PeriodKey::Block {
            start_ms, end_ms, ..
        } => span(*start_ms, *end_ms, tz, now_ms, level),
    }
}

/// An account as the table names it (spec §5.3): its position and alias, else its label;
/// `removed`; `unattributed`.
fn account_text(account: &RowAccount) -> String {
    match account {
        RowAccount::Account {
            position,
            label,
            alias,
            ..
        } => format!("{position} {}", alias.as_deref().unwrap_or(label)),
        RowAccount::Removed { .. } => "removed".to_owned(),
        RowAccount::Unattributed => "unattributed".to_owned(),
    }
}

/// An account in full, as the detail and the filter name it: `1 work (a@x.co)`, `removed <id>`.
pub fn account_name(account: &RowAccount) -> String {
    match account {
        RowAccount::Account {
            position,
            label,
            alias: Some(alias),
            ..
        } => format!("{position} {alias} ({label})"),
        RowAccount::Account {
            position, label, ..
        } => format!("{position} {label}"),
        RowAccount::Removed { id } => format!("removed {id}"),
        RowAccount::Unattributed => "unattributed".to_owned(),
    }
}

/// A cost as a cell says it: dollars and cents, `≥` before a partial one, `—` for none.
fn cost_text(cost: Option<&Cost>) -> String {
    match cost {
        Some(c) if c.partial => format!("{AT_LEAST}{}", words::dollars(c.usd_nanos)),
        Some(c) => words::dollars(c.usd_nanos),
        None => words::MISSING.to_owned(),
    }
}

/// Each column's widest cell over one report's rows and total, and the period's at each level
/// of `span`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Found {
    cols: Vec<usize>,
    periods: [usize; SPAN_LEVELS],
}

/// A report's `Found`, found the first time the report is drawn: a year by session is thousands
/// of rows, which a frame then only lays out.
#[derive(Debug, Default)]
pub(super) struct Widths(OnceCell<Found>);

/// The table laid out for one report at one width.
struct Table<'a> {
    report: &'a TokenReport,
    widths: &'a Widths,
    cols: Vec<(Col, usize)>,
    /// How compact the period's spans are (`span`).
    level: usize,
    /// The largest row cost, for the bar.
    most: u64,
    color: bool,
    now_ms: i64,
}

impl<'a> Table<'a> {
    /// The columns `report` takes at `width` (spec §5.3): period, account, input, output, total
    /// and cost always, with the project for sessions, the kind for blocks and the model when
    /// grouped by it. From `WIDE` columns, the cache write (split into 5-minute and 1-hour when
    /// that still fits), the cache read, the responses and the bar. Text columns share the rest.
    fn new(
        report: &'a TokenReport,
        widths: &'a Widths,
        width: usize,
        color: bool,
        now_ms: i64,
    ) -> Table<'a> {
        let q = &report.query;
        let mut cols = vec![Col::Period];
        match q.period {
            TokenPeriod::Block => cols.push(Col::Kind),
            TokenPeriod::Session => cols.push(Col::Project),
            TokenPeriod::Day | TokenPeriod::Month => {}
        }
        cols.push(Col::Account);
        if matches!(
            q.applied_grouping(),
            TokenGrouping::Model | TokenGrouping::AccountModel
        ) {
            cols.push(Col::Model);
        }
        cols.extend([Col::Input, Col::Output]);
        let wide = width >= WIDE;
        if wide {
            cols.extend([Col::CacheWrite, Col::CacheRead]);
        }
        cols.push(Col::Total);
        if wide {
            cols.push(Col::Responses);
        }
        cols.push(Col::Cost);
        if wide {
            cols.push(Col::Bar);
        }
        let most = report
            .rows
            .iter()
            .filter_map(|r| r.cost.map(|c| c.usd_nanos))
            .max()
            .unwrap_or(0);
        let mut table = Table {
            report,
            widths,
            cols: cols.into_iter().map(|c| (c, 0)).collect(),
            level: 0,
            most,
            color,
            now_ms,
        };
        let split = [Col::CacheWrite5m, Col::CacheWrite1h];
        let natural = table.natural();
        let at = table.cols.iter().position(|(c, _)| *c == Col::CacheWrite);
        if let Some(at) = at {
            let extra: usize = split
                .iter()
                .map(|c| table.widest(*c) + 2)
                .sum::<usize>()
                .saturating_sub(table.widest(Col::CacheWrite) + 2);
            if natural + extra <= width {
                table.cols.splice(at..=at, split.map(|c| (c, 0)));
            }
        }
        table.fit(width);
        table
    }

    /// The width every column wants, its gaps and the one-column margin included.
    fn natural(&self) -> usize {
        1 + self
            .cols
            .iter()
            .map(|(c, _)| self.widest(*c))
            .sum::<usize>()
            + 2 * self.cols.len().saturating_sub(1)
    }

    /// What `Widths` holds, found once per report.
    fn found(&self) -> &Found {
        self.widths.0.get_or_init(|| Found {
            cols: Col::ALL.iter().map(|c| self.widest_cell(*c, 0)).collect(),
            periods: std::array::from_fn(|level| self.widest_cell(Col::Period, level)),
        })
    }

    /// The widest of `col`'s heading and cells, the total's among them, the period's at the
    /// table's level.
    fn widest(&self, col: Col) -> usize {
        if col == Col::Period {
            return self.found().periods[self.level];
        }
        self.found().cols[Col::ALL.iter().position(|c| *c == col).unwrap_or(0)]
    }

    fn widest_cell(&self, col: Col, level: usize) -> usize {
        if col == Col::Bar {
            return BAR;
        }
        let tz = &self.report.query.tz;
        let cell = |r: &TokenRow| match col {
            Col::Period => period_text(&r.period, tz, self.now_ms, level),
            _ => self.cell(col, r, tz).text,
        };
        self.report
            .rows
            .iter()
            .map(|r| width_of(&cell(r)))
            .chain([width_of(&self.total_cell(col).text)])
            .fold(width_of(col.head(self.report.query.period)), usize::max)
    }

    /// Lays the columns out in `width` (B17, B24). The numbers take their widest and are never
    /// cut. The period takes the fullest form of its spans that leaves each text column its
    /// least (`TEXT_MIN`, `PROJECT_MIN`, or its own width when that is less). The text columns
    /// share what is left, each its own width or an equal share of the rest, narrowest first,
    /// and are cut by display width. Where even the most compact period leaves the text columns
    /// no column each, the period is cut too; a number never is.
    fn fit(&mut self, width: usize) {
        let gaps = 1 + 2 * self.cols.len().saturating_sub(1);
        let fixed: usize = self
            .cols
            .iter()
            .filter(|(c, _)| !c.text() && *c != Col::Period)
            .map(|(c, _)| self.widest(*c))
            .sum();
        let texts: Vec<usize> = (0..self.cols.len())
            .filter(|&i| self.cols[i].0.text())
            .collect();
        let least: usize = texts
            .iter()
            .map(|&i| {
                let col = self.cols[i].0;
                let floor = if col == Col::Project {
                    PROJECT_MIN
                } else {
                    TEXT_MIN
                };
                self.widest(col).min(floor)
            })
            .sum();
        let periods = self.found().periods;
        self.level = (0..SPAN_LEVELS)
            .find(|&l| fixed + gaps + periods[l] + least <= width)
            .unwrap_or(SPAN_LEVELS - 1);
        let period = periods[self.level].min(width.saturating_sub(fixed + gaps + texts.len()));
        let mut room = width.saturating_sub(fixed + gaps + period);
        let mut order = texts.clone();
        order.sort_by_key(|&i| self.widest(self.cols[i].0));
        let mut left = order.len();
        for i in order {
            let w = self.widest(self.cols[i].0).min(room / left.max(1));
            self.cols[i].1 = w;
            room -= w;
            left -= 1;
        }
        for i in 0..self.cols.len() {
            let col = self.cols[i].0;
            if col == Col::Period {
                self.cols[i].1 = period;
            } else if !col.text() {
                self.cols[i].1 = self.widest(col);
            }
        }
    }

    fn counts_cell(&self, col: Col, c: &TokenCounts, unfinalized: u64, responses: u64) -> Cell {
        let warning = severity_style(self.color, Severity::Warning);
        match col {
            Col::Input => Cell::plain(words::scaled(c.input)),
            Col::Output if unfinalized > 0 => Cell {
                text: format!("{AT_LEAST}{}", words::scaled(c.output)),
                style: warning,
            },
            Col::Output => Cell::plain(words::scaled(c.output)),
            Col::CacheWrite => Cell::plain(words::scaled(c.cache_write())),
            Col::CacheWrite5m => Cell::plain(words::scaled(c.cache_write_5m)),
            Col::CacheWrite1h => Cell::plain(words::scaled(c.cache_write_1h)),
            Col::CacheRead => Cell::plain(words::scaled(c.cache_read)),
            Col::Total => Cell::plain(words::scaled(c.total())),
            Col::Responses => Cell::plain(responses.to_string()),
            _ => Cell::plain(""),
        }
    }

    fn cost_cell(&self, cost: Option<&Cost>) -> Cell {
        let style = match cost {
            Some(c) if c.partial => severity_style(self.color, Severity::Warning),
            _ => Style::new(),
        };
        Cell {
            text: cost_text(cost),
            style,
        }
    }

    /// `row`'s cell in `col`.
    fn cell(&self, col: Col, row: &TokenRow, tz: &TimeZone) -> Cell {
        match col {
            Col::Period => Cell::plain(period_text(&row.period, tz, self.now_ms, self.level)),
            Col::Kind => Cell::plain(match row.period {
                PeriodKey::Block { observed: true, .. } => "observed",
                PeriodKey::Block { .. } => "inferred",
                _ => "",
            }),
            Col::Project => Cell::plain(match &row.period {
                PeriodKey::Session {
                    project: Some(p), ..
                } => p.clone(),
                _ => words::MISSING.to_owned(),
            }),
            Col::Account => Cell::plain(match &row.account {
                Some(a) => account_text(a),
                None => row
                    .accounts
                    .iter()
                    .map(account_text)
                    .collect::<Vec<_>>()
                    .join(", "),
            }),
            Col::Model => Cell::plain(row.model.clone().unwrap_or_else(|| row.models.join(", "))),
            Col::Cost => self.cost_cell(row.cost.as_ref()),
            Col::Bar => Cell::plain(match row.cost {
                Some(c) if self.most > 0 && c.usd_nanos > 0 => {
                    let cells =
                        (u128::from(c.usd_nanos) * BAR as u128).div_ceil(u128::from(self.most));
                    "█".repeat((cells as usize).clamp(1, BAR))
                }
                _ => String::new(),
            }),
            _ => self.counts_cell(col, &row.counts, row.unfinalized, row.responses),
        }
    }

    /// The total line's cell in `col`.
    fn total_cell(&self, col: Col) -> Cell {
        let t = &self.report.total;
        match col {
            Col::Period => Cell::plain("TOTAL"),
            Col::Kind | Col::Project | Col::Account | Col::Model | Col::Bar => Cell::plain(""),
            Col::Cost => self.cost_cell(t.cost.as_ref()),
            _ => self.counts_cell(col, &t.counts, t.unfinalized, t.responses),
        }
    }

    /// One line of cells: each padded or cut to its column, numbers on the right, two columns
    /// apart, after a one-column margin. `blank` leaves the period's own columns empty. A row's
    /// project keeps its end when it is cut; every other cell, its start.
    fn line(&self, cells: Vec<Cell>, blank: bool, row: bool) -> Line<'static> {
        let mut spans = vec![Span::raw(" ")];
        for (i, ((col, w), cell)) in self.cols.iter().zip(cells).enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            let text = if blank && matches!(col, Col::Period | Col::Kind | Col::Project) {
                String::new()
            } else if *col == Col::Project && row {
                fit_left(&cell.text, *w)
            } else {
                fit(&cell.text, *w)
            };
            let gap = " ".repeat(w.saturating_sub(width_of(&text)));
            if col.number() {
                spans.push(Span::raw(gap));
                spans.push(Span::styled(text, cell.style));
            } else {
                spans.push(Span::styled(text, cell.style));
                spans.push(Span::raw(gap));
            }
        }
        trim(Line::from(spans))
    }

    fn header(&self) -> Line<'static> {
        let period = self.report.query.period;
        let cells = self
            .cols
            .iter()
            .map(|(c, _)| Cell::plain(c.head(period)))
            .collect();
        self.line(cells, false, false).style(bold())
    }

    fn row(&self, row: &TokenRow, blank: bool) -> Line<'static> {
        let tz = &self.report.query.tz;
        let cells = self
            .cols
            .iter()
            .map(|(c, _)| self.cell(*c, row, tz))
            .collect();
        self.line(cells, blank, true)
    }

    fn total(&self) -> Line<'static> {
        let cells = self.cols.iter().map(|(c, _)| self.total_cell(*c)).collect();
        self.line(cells, false, false).style(bold())
    }
}

/// `s` cut to `w` columns from the left, so a path keeps its end: `…/Code/tagteam`.
fn fit_left(s: &str, w: usize) -> String {
    if width_of(s) <= w {
        return s.to_owned();
    }
    if w == 0 {
        return String::new();
    }
    let mut kept: Vec<char> = Vec::new();
    for c in s.chars().rev() {
        kept.push(c);
        let tail: String = kept.iter().rev().collect();
        if width_of(&tail) > w - 1 {
            kept.pop();
            break;
        }
    }
    let tail: String = kept.iter().rev().collect();
    format!("…{tail}")
}

/// `line` without the spaces its last cell was padded with.
fn trim(line: Line<'static>) -> Line<'static> {
    let style = line.style;
    let mut spans = line.spans;
    while spans.last().is_some_and(|s| s.content.trim().is_empty()) {
        spans.pop();
    }
    Line::from(spans).style(style)
}

/// `parts` laid out after `head` in rows of at most `width` columns, `sep` between two parts on
/// a row, each row after the first `indent` columns in (B26). A part that fits no row whole
/// wraps at its spaces, and a word wider than a row breaks by width (`view::wrap`): nothing is
/// cut, for what lies below the total scrolls (B29).
fn laid_out(
    head: Vec<Span<'static>>,
    parts: &[String],
    sep: &str,
    indent: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let room = width.saturating_sub(indent).max(1);
    let pad = || Span::raw(" ".repeat(indent));
    let mut lines = Vec::new();
    let mut used: usize = head.iter().map(|s| width_of(&s.content)).sum();
    let mut spans = head;
    for part in parts {
        let fresh = used <= indent;
        let w = width_of(part);
        if fresh && used + w <= width {
            spans.push(Span::raw(part.clone()));
            used += w;
            continue;
        }
        if !fresh && used + width_of(sep) + w <= width {
            spans.push(Span::raw(format!("{sep}{part}")));
            used += width_of(sep) + w;
            continue;
        }
        if !fresh {
            lines.push(Line::from(std::mem::replace(&mut spans, vec![pad()])));
            used = indent;
        }
        for (i, chunk) in wrap(part, room).into_iter().enumerate() {
            if i > 0 {
                lines.push(Line::from(std::mem::replace(&mut spans, vec![pad()])));
                used = indent;
            }
            used += width_of(&chunk);
            spans.push(Span::raw(chunk));
        }
    }
    lines.push(Line::from(spans));
    lines
}

/// The active block's figures (spec §5.3, §6.4) at `now_ms`: its elapsed and remaining time,
/// which count on between queries from its start and end, its burn rate and its projection.
/// `None` once it has ended: the row is no longer active.
fn figures(row: &TokenRow, now_ms: i64) -> Option<Vec<String>> {
    let PeriodKey::Block {
        start_ms,
        end_ms,
        active: Some(a),
        ..
    } = &row.period
    else {
        return None;
    };
    if now_ms >= *end_ms {
        return None;
    }
    let elapsed = (now_ms - start_ms).max(0) / 1000;
    let left = (end_ms - now_ms) / 1000;
    Some(vec![
        "active".to_owned(),
        format!("{} elapsed", words::duration(elapsed)),
        format!("{} left", words::duration(left)),
        format!(
            "{} tokens/min",
            words::scaled(a.tokens_per_min.max(0.0).round() as u64)
        ),
        format!("{}/h", cost_text(a.cost_per_hour.as_ref())),
        format!(
            "projected {} tokens, {}",
            words::scaled(a.projected_tokens),
            cost_text(a.projected_cost.as_ref())
        ),
    ])
}

/// One line of the table's scrolling part.
enum Item {
    /// A report row; `repeat` when its period is the row's before it.
    Row { repeat: bool },
    /// A line of the active block's figures under its row.
    Figures(Line<'static>),
}

/// The table's lines at `width`, and the line each row starts at.
fn items(report: &TokenReport, width: usize, now_ms: i64) -> (Vec<(usize, Item)>, Vec<usize>) {
    let mut items = Vec::new();
    let mut starts = Vec::with_capacity(report.rows.len());
    for (index, row) in report.rows.iter().enumerate() {
        starts.push(items.len());
        let repeat = index > 0 && report.rows[index - 1].period == row.period;
        items.push((index, Item::Row { repeat }));
        if let Some(parts) = figures(row, now_ms) {
            let head = vec![Span::raw(" ".repeat(FIGURES_INDENT))];
            for line in laid_out(head, &parts, " · ", FIGURES_INDENT, width) {
                items.push((index, Item::Figures(line)));
            }
        }
    }
    (items, starts)
}

/// The first line to show so that lines `first..=last` show, moving as little as it can, and
/// never past the last full page (R22).
fn keep_visible(offset: usize, first: usize, last: usize, rows: usize, lines: usize) -> usize {
    let rows = rows.max(1);
    let offset = if first < offset {
        first
    } else if last >= offset + rows {
        (last + 1 - rows).min(first)
    } else {
        offset
    };
    offset.min(lines.saturating_sub(rows))
}

/// The grouping's words in the header.
fn grouping_text(g: TokenGrouping) -> &'static str {
    match g {
        TokenGrouping::None => "none",
        TokenGrouping::Account => "account",
        TokenGrouping::Model => "model",
        TokenGrouping::AccountModel => "account, model",
    }
}

/// The filter's words in the header: `all accounts · all models`, else what it keeps.
fn filter_text(state: &State) -> String {
    let t = &state.tokens;
    let accounts = match t.accounts.as_deref() {
        None => "all accounts".to_owned(),
        Some([AccountSel::Unattributed]) => "unattributed".to_owned(),
        Some([AccountSel::Account(id)]) => state
            .accounts
            .snapshot()
            .and_then(|s| {
                s.providers
                    .iter()
                    .flat_map(|p| &p.accounts)
                    .find(|v| &v.row.id == id)
            })
            .map_or_else(
                || "1 account".to_owned(),
                |v| format!("{} {}", v.row.position, words::name(&v.row)),
            ),
        Some(list) => words::plural(list.len() as u64, "account"),
    };
    let models = match t.models.as_deref() {
        None => "all models".to_owned(),
        Some([one]) => one.clone(),
        Some(list) => words::plural(list.len() as u64, "model"),
    };
    format!("filter: {accounts} · {models}")
}

/// The rows the drill-down came through, from the top: each one's period and its account and
/// model when it has them.
fn crumbs(t: &TokensState, now_ms: i64) -> Option<String> {
    let parents: Vec<String> = t.levels[..t.levels.len() - 1]
        .iter()
        .filter_map(|level| {
            let report = level.report.as_ref()?;
            let key = level.selected.as_ref()?;
            let row = report.rows.iter().find(|r| &r.key == key)?;
            let period = match &row.period {
                PeriodKey::Session { session_id, .. } => format!("session {session_id}"),
                p => period_text(p, &report.query.tz, now_ms, 0),
            };
            let mut parts = vec![period];
            parts.extend(row.account.as_ref().map(account_text));
            parts.extend(row.model.clone());
            Some(parts.join(" "))
        })
        .collect();
    (!parents.is_empty()).then(|| format!("› {}", parents.join(" › ")))
}

/// The screen's own header (spec §5.3), three rows at most (B29):
/// 1. the periods with the shown one bracketed, the range, and the grouping the report applies
///    (blocks are per account): 60 columns at the widest;
/// 2. the filter and the rows the drill-down came through, on the first row when they fit
///    there whole; else on a row of their own, the filter cut first, to `FILTER_LEAST`, then
///    the drill-down, each with `…`. The filter's dialog shows it whole, and the detail the
///    rows;
/// 3. the zone and the index's state, `catching up` while the index is partial, and `updating…`
///    while the report on screen is read again, cut with `…` past the row.
fn header(state: &State, width: usize, now_ms: i64) -> Vec<Line<'static>> {
    let t = &state.tokens;
    let mut tabs = vec![Span::raw(" ")];
    for (i, (period, name)) in [
        (TokenPeriod::Day, "Day"),
        (TokenPeriod::Month, "Month"),
        (TokenPeriod::Session, "Session"),
        (TokenPeriod::Block, "Block"),
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            tabs.push(Span::raw(" "));
        }
        if period == t.period {
            tabs.push(Span::styled(format!("[{name}]"), bold()));
        } else {
            tabs.push(Span::raw(name));
        }
    }
    let applied = t.query(now_ms).applied_grouping();
    tabs.push(Span::raw(format!(
        "  {}  group: {}",
        t.range.label(t.period),
        grouping_text(applied)
    )));
    let filter = filter_text(state);
    let crumb = crumbs(t, now_ms);
    let whole = match &crumb {
        Some(crumb) => format!("{filter}  {crumb}"),
        None => filter.clone(),
    };
    let used: usize = tabs.iter().map(|s| width_of(&s.content)).sum();
    let room = width.saturating_sub(1);
    let mut lines = if used + 2 + width_of(&whole) <= width {
        tabs.push(Span::raw(format!("  {whole}")));
        vec![Line::from(tabs)]
    } else {
        let second = match &crumb {
            None => fit(&filter, room),
            Some(crumb) => {
                let rest = room.saturating_sub(2);
                let least = rest.saturating_sub(width_of(crumb)).max(FILTER_LEAST);
                let filter = fit(&filter, width_of(&filter).min(least));
                let crumb = fit(crumb, rest.saturating_sub(width_of(&filter)));
                format!("{filter}  {crumb}")
            }
        };
        vec![Line::from(tabs), Line::from(format!(" {second}"))]
    };
    let zone = t
        .report()
        .map(|r| r.time_zone.clone())
        .unwrap_or_else(|| t.tz.iana_name().unwrap_or("UTC").to_owned());
    let mut status = vec![zone];
    let index = t.watch.state();
    let tz = &t.tz;
    if let Some(p) = t.watch.refreshing() {
        status.push(if p.bytes_total > 0 {
            format!(
                "indexing transcripts {} / {}",
                bytes(p.bytes_done),
                bytes(p.bytes_total)
            )
        } else {
            "indexing transcripts…".to_owned()
        });
    } else if t.watch.busy() {
        status.push("catching up elsewhere".to_owned());
    } else if let Some(at) = index.and_then(|i| i.refreshed_at_ms) {
        status.push(format!("refreshed {}", clock(at, tz, now_ms)));
    }
    if index.is_some_and(|i| i.refreshed_at_ms.is_none()) {
        status.push("catching up".to_owned());
    }
    if t.pending && t.report().is_some() {
        status.push("updating…".to_owned());
    }
    lines.push(Line::from(format!(" {}", fit(&status.join(" · "), room))));
    lines
}

/// A time in `tz` as the header says it: `14:12` today, else with its date.
fn clock(ms: i64, tz: &TimeZone, now_ms: i64) -> String {
    let (Some(at), Some(now)) = (zoned(ms, tz), zoned(now_ms, tz)) else {
        return words::MISSING.to_owned();
    };
    if at.date() == now.date() {
        at.strftime("%H:%M").to_string()
    } else {
        at.strftime("%Y-%m-%d %H:%M").to_string()
    }
}

/// The selected row in full (B11, B25), in rows at `width`: its numbers first, laid out by
/// their parts and never cut; then what tells it apart and what it holds, wrapped: a session's
/// ID, project and times, a block's span and kind, every account and model.
fn detail_rows(row: &TokenRow, tz: &TimeZone, now_ms: i64, width: usize) -> Vec<Line<'static>> {
    let c = &row.counts;
    let numbers = [
        words::plural(row.responses, "response"),
        format!(
            "cache write {} (5m {}, 1h {})",
            words::scaled(c.cache_write()),
            words::scaled(c.cache_write_5m),
            words::scaled(c.cache_write_1h)
        ),
        format!("cache read {}", words::scaled(c.cache_read)),
    ];
    let mut text = match &row.period {
        PeriodKey::Session {
            session_id,
            project,
            first_ms,
            last_ms,
        } => vec![
            format!("session {session_id}"),
            project.clone().unwrap_or_else(|| "no project".to_owned()),
            format!("{} – {}", when(*first_ms, tz), when(*last_ms, tz)),
        ],
        PeriodKey::Block {
            start_ms,
            end_ms,
            observed,
            ..
        } => vec![
            format!("{} – {}", when(*start_ms, tz), when(*end_ms, tz)),
            if *observed { "observed" } else { "inferred" }.to_owned(),
        ],
        p => vec![period_text(p, tz, now_ms, 0)],
    };
    if !row.accounts.is_empty() {
        let names: Vec<String> = row.accounts.iter().map(account_name).collect();
        text.push(names.join(", "));
    }
    if !row.models.is_empty() {
        text.push(row.models.join(", "));
    }
    let mut rows = laid_out(vec![Span::raw(" ")], &numbers, " · ", 1, width);
    rows.extend(laid_out(vec![Span::raw(" ")], &text, " · ", 1, width));
    rows
}

/// What lies below the total (B25): the selected row's detail, then the footnotes (TR §11.2,
/// B15), each wrapped at `width` and never cut, for the region scrolls (B29).
fn below(state: &State, width: usize, now_ms: i64) -> Vec<Line<'static>> {
    let t = &state.tokens;
    let mut lines = Vec::new();
    let Some(report) = t.report().filter(|r| !r.rows.is_empty()) else {
        return lines;
    };
    if let Some(row) = t.selected_row() {
        lines.extend(detail_rows(row, &report.query.tz, now_ms, width));
    }
    for note in words::token_footnotes(report, AT_LEAST, true) {
        lines.extend(laid_out(vec![Span::raw(" ")], &[note], " ", 1, width));
    }
    lines
}

/// What the body holds below the header, at its size: the table's rows, then the total, then
/// what lies below it (`below`) in as many rows as it needs, half the body at most and leaving
/// the table `TABLE_MIN` rows. What does not fit there scrolls with `J`/`K` (B25), and its last
/// row says which lines show. The total follows the last row shown, so it is at the foot of a
/// table that fills the body.
struct Plan {
    header: Vec<Line<'static>>,
    rows: usize,
    below: Vec<Line<'static>>,
}

/// How many rows what lies below the total takes: as many as its `lines`, half of `height` at
/// most, and never the table's last `TABLE_MIN`.
fn below_rows(lines: usize, header: usize, height: usize) -> usize {
    let fixed = header + 2;
    lines.min(height.saturating_sub(fixed + TABLE_MIN).min(height / 2))
}

/// The lines that show of `lines` in `rows` rows from line `from` (clamped, R22), with a last row
/// that says which when they do not all fit; and the furthest `from` goes.
fn window(lines: Vec<Line<'static>>, from: usize, rows: usize) -> (Vec<Line<'static>>, usize) {
    let total = lines.len();
    if total <= rows {
        return (lines, 0);
    }
    let shown = if rows >= 2 { rows - 1 } else { rows };
    let furthest = total - shown;
    let from = from.min(furthest);
    let mut out: Vec<Line<'static>> = lines.into_iter().skip(from).take(shown).collect();
    if rows >= 2 {
        let hint = format!(" J K: lines {}–{} of {total}", from + 1, from + shown);
        out.push(Line::from(Span::styled(hint, bold())));
    }
    (out, furthest)
}

/// Where what lies below the total is scrolled to: from its top for a row other than the one it
/// was scrolled for.
fn below_from(t: &TokensState) -> usize {
    match (&t.below_for, t.selected()) {
        (Some(key), Some(selected)) if key == selected => t.below_from,
        _ => 0,
    }
}

fn plan(state: &State, area: Rect, now_ms: i64) -> Plan {
    let width = usize::from(area.width);
    let height = usize::from(area.height);
    let header = header(state, width, now_ms);
    let lines = below(state, width, now_ms);
    let rows = below_rows(lines.len(), header.len(), height);
    let (below, _) = window(lines, below_from(&state.tokens), rows);
    let table = height.saturating_sub(header.len() + 2 + below.len());
    Plan {
        header,
        rows: table,
        below,
    }
}

/// `J`/`K` (B25): what lies below the total moves `by` a line, stopping at either end.
pub(super) fn scroll_below(state: &mut State, area: Rect, now_ms: i64, by: isize) {
    let width = usize::from(area.width);
    let header = header(state, width, now_ms).len();
    let lines = below(state, width, now_ms);
    let rows = below_rows(lines.len(), header, usize::from(area.height));
    let from = below_from(&state.tokens);
    let (_, furthest) = window(lines, from, rows);
    let from = from.min(furthest) as isize;
    let t = &mut state.tokens;
    t.below_from = (from + by).clamp(0, furthest as isize) as usize;
    t.below_for = t.selected().cloned();
}

/// How many table rows show in `area`, for the keys that page (spec §5.6).
pub(super) fn page(state: &State, area: Rect, now_ms: i64) -> usize {
    plan(state, area, now_ms).rows
}

/// Keeps the selected row in view after a key moved it (R22: from what shows).
pub(super) fn follow(state: &mut State, area: Rect, now_ms: i64) {
    let rows = page(state, area, now_ms);
    let t = &state.tokens;
    let (Some(report), Some(at)) = (t.report(), t.selected_index()) else {
        return;
    };
    let (items, starts) = items(report, usize::from(area.width), now_ms);
    let last = starts.get(at + 1).copied().unwrap_or(items.len()) - 1;
    let offset = keep_visible(t.level().offset, starts[at], last, rows, items.len());
    state.tokens.level_mut().offset = offset;
}

/// The selected row in reverse video across the whole width.
fn highlight(row: Line<'static>, width: usize) -> Line<'static> {
    let used: usize = row.spans.iter().map(|s| width_of(&s.content)).sum();
    let mut spans = row.spans;
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    Line::from(spans).style(selected())
}

pub fn render(state: &State, frame: &mut Frame, area: Rect, now_ms: i64) {
    let width = usize::from(area.width);
    let t = &state.tokens;
    let plan = plan(state, area, now_ms);
    let mut lines = plan.header;
    let Some(report) = t.report() else {
        if t.pending {
            lines.push(Line::from(format!(" {LOADING}")));
        }
        frame.render_widget(Paragraph::new(lines), area);
        return;
    };
    if report.rows.is_empty() {
        lines.push(Line::from(format!(" {}", words::NO_TOKEN_USAGE)));
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }
    let table = Table::new(report, &t.level().widths, width, state.color, now_ms);
    lines.push(table.header());
    let (items, starts) = items(report, width, now_ms);
    let selected_at = t.selected_index();
    let offset = match selected_at {
        Some(at) => {
            let last = starts.get(at + 1).copied().unwrap_or(items.len()) - 1;
            keep_visible(t.level().offset, starts[at], last, plan.rows, items.len())
        }
        None => t.level().offset.min(items.len().saturating_sub(plan.rows)),
    };
    for (n, (index, item)) in items.into_iter().skip(offset).take(plan.rows).enumerate() {
        lines.push(match item {
            Item::Row { repeat } => {
                let row = table.row(&report.rows[index], repeat && n > 0);
                if Some(index) == selected_at {
                    highlight(row, width)
                } else {
                    row
                }
            }
            Item::Figures(line) => line,
        });
    }
    lines.push(table.total());
    lines.extend(plan.below);
    frame.render_widget(Paragraph::new(lines), area);
}
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
//! session to its models. Every query runs on the report lane (§4.3), never on the UI thread,
//! and the index's catch-up on the index lane (`index`).

pub mod index;
mod table;

use crossterm::event::{KeyCode, KeyEvent};
use jiff::tz::TimeZone;
use ratatui::Frame;
use ratatui::layout::Rect;
use tagteam_core::ProviderId;
use tagteam_engine::tokens::{
    AccountSel, RowKey, TokenGrouping, TokenPeriod, TokenQuery, TokenReport, TokenRow,
    default_since_ms,
};

use self::index::IndexWatch;
use crate::msg::Cmd;
use crate::source::SourceError;
use crate::state::{Screen, State};

```

with:

```rust
//! session to its models. Every query runs on the report lane (§4.3), never on the UI thread,
//! and the index's catch-up on the index lane (`index`).

pub mod index;
mod table;

use crossterm::event::{KeyCode, KeyEvent};
use jiff::tz::TimeZone;
use tagteam_core::ProviderId;
use tagteam_engine::tokens::{
    AccountSel, RowKey, TokenGrouping, TokenPeriod, TokenQuery, TokenReport, TokenRow,
    default_since_ms,
};

use self::index::IndexWatch;
pub use self::table::render;
use crate::msg::Cmd;
use crate::source::SourceError;
use crate::state::{Screen, State};

```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
    /// range rolls with the clock; a drilled level's is `TokenQuery::drill`'s, fixed.
    query: Option<TokenQuery>,
    report: Option<TokenReport>,
    selected: Option<RowKey>,
}

impl Level {
    fn top() -> Level {
        Level {
            query: None,
            report: None,
            selected: None,
        }
    }
}

```

with:

```rust
    /// range rolls with the clock; a drilled level's is `TokenQuery::drill`'s, fixed.
    query: Option<TokenQuery>,
    report: Option<TokenReport>,
    selected: Option<RowKey>,
    /// The first table line shown; clamped where it is used (R22).
    offset: usize,
    /// The report's column widths, found when it is first drawn.
    widths: table::Widths,
}

impl Level {
    fn top() -> Level {
        Level::with(None)
    }

    fn with(query: Option<TokenQuery>) -> Level {
        Level {
            query,
            report: None,
            selected: None,
            offset: 0,
            widths: table::Widths::default(),
        }
    }
}

```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
    /// A failed read's words, the banner while Tokens is open (spec §10.4, B12).
    error: Option<String>,
    /// The index's state, its refresh, and the timers that read the report again (§6.4).
    watch: IndexWatch,
}

impl TokensState {
    /// By day and account over the default range, unfiltered (spec §5.3; `tagteam tokens`'s
```

with:

```rust
    /// A failed read's words, the banner while Tokens is open (spec §10.4, B12).
    error: Option<String>,
    /// The index's state, its refresh, and the timers that read the report again (§6.4).
    watch: IndexWatch,
    /// How far what lies below the total is scrolled (`J`/`K`, B25), and for which row: another
    /// row's starts at its top.
    below_from: usize,
    below_for: Option<RowKey>,
}

impl TokensState {
    /// By day and account over the default range, unfiltered (spec §5.3; `tagteam tokens`'s
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
            pending: false,
            again: false,
            error: None,
            watch: IndexWatch::default(),
        }
    }

    pub fn provider(&self) -> &ProviderId {
```

with:

```rust
            pending: false,
            again: false,
            error: None,
            watch: IndexWatch::default(),
            below_from: 0,
            below_for: None,
        }
    }

    pub fn provider(&self) -> &ProviderId {
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
            let at = old.unwrap_or(0).min(report.rows.len() - 1);
            level.selected = Some(report.rows[at].key.clone());
        }
        level.report = Some(report);
    }

    /// Moves the selection `by` rows, stopping at either end.
    fn step(&mut self, by: isize) {
```

with:

```rust
            let at = old.unwrap_or(0).min(report.rows.len() - 1);
            level.selected = Some(report.rows[at].key.clone());
        }
        level.report = Some(report);
        level.widths = table::Widths::default();
    }

    /// Moves the selection `by` rows, stopping at either end.
    fn step(&mut self, by: isize) {
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
    /// key stays, for a row the new report still has.
    fn reset(&mut self) {
        self.levels.truncate(1);
        self.levels[0].report = None;
    }
}

/// A query of the shown level under a new generation: whatever is on the report lane is for
```

with:

```rust
    /// key stays, for a row the new report still has.
    fn reset(&mut self) {
        self.levels.truncate(1);
        self.levels[0].report = None;
        self.levels[0].widths = table::Widths::default();
    }
}

/// A query of the shown level under a new generation: whatever is on the report lane is for
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
    keys
}

/// The Tokens screen's keys (spec §5.6): `p` period, `g` group, `[` `]` range, `Enter` down a
/// level, `Esc`/`Backspace` up one, and the rows.
pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    let t = &mut state.tokens;
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => t.step(1),
        KeyCode::Char('k') | KeyCode::Up => t.step(-1),
        KeyCode::Home => t.step(isize::MIN / 2),
        KeyCode::End => t.step(isize::MAX / 2),
        KeyCode::Char('p') => {
            t.period = match t.period {
```

with:

```rust
    keys
}

/// The Tokens screen's keys (spec §5.6): `p` period, `g` group, `[` `]` range, `Enter` down a
/// level, `Esc`/`Backspace` up one, the rows, which keep the selected one in view, and `J`/`K`,
/// which scroll what lies below the total (B25).
pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    let body = crate::view::body_at(state);
    let page = table::page(state, body, now_ms).max(1) as isize;
    let t = &mut state.tokens;
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => t.step(1),
        KeyCode::Char('k') | KeyCode::Up => t.step(-1),
        KeyCode::PageDown => t.step(page),
        KeyCode::PageUp => t.step(-page),
        KeyCode::Home => t.step(isize::MIN / 2),
        KeyCode::End => t.step(isize::MAX / 2),
        KeyCode::Char('p') => {
            t.period = match t.period {
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
            t.range = t.range.step(by);
            t.reset();
            return load(state, now_ms);
        }
        KeyCode::Enter => return drill(state, now_ms),
        KeyCode::Esc | KeyCode::Backspace => return up(state, now_ms),
        _ => {}
    }
    vec![]
}

/// `Enter`: the selected row's next level down (`TokenQuery::drill`), from the query its report
```

with:

```rust
            t.range = t.range.step(by);
            t.reset();
            return load(state, now_ms);
        }
        KeyCode::Char('J') | KeyCode::Char('K') => {
            let by = if key.code == KeyCode::Char('J') {
                1
            } else {
                -1
            };
            table::scroll_below(state, body, now_ms, by);
            return vec![];
        }
        KeyCode::Enter => return drill(state, now_ms),
        KeyCode::Esc | KeyCode::Backspace => return up(state, now_ms),
        _ => return vec![],
    }
    table::follow(state, body, now_ms);
    vec![]
}

/// `Enter`: the selected row's next level down (`TokenQuery::drill`), from the query its report
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
    };
    let Some(query) = report.query.drill(row) else {
        return vec![];
    };
    t.levels.push(Level {
        query: Some(query),
        report: None,
        selected: None,
    });
    load(state, now_ms)
}

/// `Esc` or `Backspace`: back one level, to its report and its selection as they were, read
```

with:

```rust
    };
    let Some(query) = report.query.drill(row) else {
        return vec![];
    };
    t.levels.push(Level::with(Some(query)));
    load(state, now_ms)
}

/// `Esc` or `Backspace`: back one level, to its report and its selection as they were, read
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
    state.tokens.levels.pop();
    load(state, now_ms)
}

/// Stands in for the report's table until it is drawn.
pub fn render(_state: &State, _frame: &mut Frame, _area: Rect, _now_ms: i64) {}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use tagteam_engine::tokens::PeriodKey;
```

with:

```rust
    state.tokens.levels.pop();
    load(state, now_ms)
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use tagteam_engine::tokens::PeriodKey;
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
    ("r", "refresh"),
    ("Enter", "history of the account"),
    ("J K", "scroll the detail's windows"),
];
const HELP_TOKENS: [(&str, &str); 6] = [
    ("p", "period: day, month, session or block"),
    ("g", "group: none, account, model, or both"),
    ("[ ]", "range: 7, 30 or 90 days, or all"),
    ("Enter", "drill down: sessions, then models"),
    ("Esc  Backspace", "back up a level"),
    ("r", "refresh the token index"),
];
const HELP_HISTORY: [(&str, &str); 3] = [
    ("[ ]", "range: 1, 7, 14 or 30 days"),
    ("r", "refresh"),
```

with:

```rust
    ("r", "refresh"),
    ("Enter", "history of the account"),
    ("J K", "scroll the detail's windows"),
];
const HELP_TOKENS: [(&str, &str); 7] = [
    ("p", "period: day, month, session or block"),
    ("g", "group: none, account, model, or both"),
    ("[ ]", "range: 7, 30 or 90 days, or all"),
    ("Enter", "drill down: sessions, then models"),
    ("Esc  Backspace", "back up a level"),
    ("r", "refresh the token index"),
    ("J K", "scroll the detail and the notes"),
];
const HELP_HISTORY: [(&str, &str); 3] = [
    ("[ ]", "range: 1, 7, 14 or 30 days"),
    ("r", "refresh"),
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-tui --lib`

Expected: `224 passed` (23 in `screens::tokens::table`).

- [ ] **Step 5: Format and lint**

Run:
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`

Expected: no output from the first; `Finished` and no warnings from the others.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-tui/src/screens/tokens/table.rs crates/tagteam-tui/src/screens/tokens/mod.rs crates/tagteam-tui/src/actions.rs
git commit -m "Draw the Tokens screen: its header, the report's table, the total and the footnotes"
```

### Task 6: The filter dialog

`f` opens the filter dialog (spec §5.3, B7), `Overlay::Filter`, while the report lane reads its inventory. The inventory is one unfiltered query: `Month`, `AccountModel`, no range, no filters.

- **The choices** are the inventory's accounts (current, removed and unattributed) with the Tokens provider's accounts in the accounts view, then its models, together with anything the filter in force names that neither has.
- **Each is chosen as the filter is:** all of a kind for `None`.
- **Applying sends `None`** for a kind with every choice chosen, so models indexed later are in too, and `Some` of exactly the chosen ones otherwise. None chosen of a kind is refused in the dialog.
- **The filter applies at the top level** (B8).

**Readings of the spec this task commits to:**
- **The inventory has a generation of its own.** One that arrives after its dialog closed, or after it was opened again, is dropped.
- **The header names a filter of one:** the account by position and name, or the model. More are counted.
- **A choice wraps under its mark, three rows at most** (B27, B29). A choice wider than the dialog wraps under its mark onto `CHOICE_ROWS` at most, part A's, the rest cut with `…`. A failure, `loading` and the keys wrap with the same cap. The list is placed from the cursor so that its whole choice shows. With every block that short, the cursor's choice and the keys always fit at 80×24.

**Depends on:** 5

**Files:**
- Create: `crates/tagteam-tui/src/screens/tokens/filter.rs`
- Modify: `crates/tagteam-tui/src/msg.rs`
- Modify: `crates/tagteam-tui/src/screens/tokens/mod.rs`
- Modify: `crates/tagteam-tui/src/screens/tokens/table.rs`
- Modify: `crates/tagteam-tui/src/view.rs`
- Modify: `crates/tagteam-tui/src/state.rs`
- Modify: `crates/tagteam-tui/src/update.rs`
- Modify: `crates/tagteam-tui/src/actions.rs`
- Modify: `crates/tagteam-tui/src/runtime.rs`

**Interfaces:**
- Consumes:
  - Task 3: `TokensState`, `load`; the report lane.
  - Task 5: `table::account_name`.
  - Part A: `actions::{Overlay, boxed, CHOICE_ROWS}`, `view::{bold, scrolled_windows, selected, severity_style, wrap_rows}`.
- Produces:
  - CB3: `ReportReq::Inventory`, `Msg::Inventory`, `Cmd::LoadInventory`.
  - CB4: `Gens::inventory`.
  - CB5's `filter.rs`; `Overlay::Filter`.
  - `f` in the footer and help.
  - `actions::CHOICE_ROWS` becomes `pub(crate)`.

**Spec:**
- TUI spec §5.3 ("Filter"), §5.5 (dialogs; `Esc` declines), §11 ("the filter's choices staying the full inventory after filtering to one model"; "selecting everything sending `None`, a narrower selection sending exactly it, and an empty selection being refused").
- TR §10.1 (`Some` of an empty list matches nothing; an account filter naming a removed account's ID matches its rows).

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-tui/src/screens/tokens/filter.rs` with its tests (Step 3 puts the code above them):

```rust
#[cfg(test)]
mod tests {
    use tagteam_core::AccountId;
    use tagteam_engine::tokens::PeriodKey;

    use super::*;
    use crate::msg::Msg;
    use crate::state::Screen;
    use crate::testutil::*;
    use crate::update::update;

    const OPUS: &str = "claude-opus-5-5";
    const SONNET: &str = "claude-sonnet-5-5";

    fn removed() -> RowAccount {
        RowAccount::Removed {
            id: AccountId::from_string("id-9"),
        }
    }

    /// The index: account 1 with opus and sonnet, a removed account with opus, unattributed
    /// sonnet; by month, account and model.
    fn indexed(query: TokenQuery) -> TokenReport {
        let month = PeriodKey::Month {
            year: 2026,
            month: 9,
        };
        let row = |account: RowAccount, model: &str| {
            token_row(
                month.clone(),
                Some(account),
                Some(model),
                counts(1, 2, 3),
                None,
            )
        };
        token_report(
            query,
            vec![
                row(row_account(1), OPUS),
                row(row_account(1), SONNET),
                row(removed(), OPUS),
                row(RowAccount::Unattributed, SONNET),
            ],
        )
    }

    /// Tokens, its accounts 1 and 2 read, and the filter dialog open with its inventory.
    fn dialog() -> State {
        let mut state = fresh(Screen::Tokens);
        state.accounts.apply(snapshot(many(2)));
        state.start();
        let cmds = update(&mut state, ch('f'), NOW_MS);
        let [Cmd::LoadInventory { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let result = Ok(indexed(query.clone()));
        update(
            &mut state,
            Msg::Inventory {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        state
    }

    /// The dialog's choices: each label, and whether it is chosen.
    fn shown(state: &State) -> Vec<(String, bool)> {
        let Some(Overlay::Filter(d)) = &state.overlay else {
            panic!("no filter dialog");
        };
        d.choices
            .iter()
            .flatten()
            .map(|c| (c.label.clone(), c.on))
            .collect()
    }

    fn all(on: bool) -> Vec<(String, bool)> {
        [
            "1 a1@x.co",
            "2 a2@x.co",
            "removed id-9",
            "unattributed",
            OPUS,
            SONNET,
        ]
        .iter()
        .map(|l| ((*l).to_owned(), on))
        .collect()
    }

    /// The query that applying the dialog asks for.
    fn applied(state: &mut State) -> TokenQuery {
        let cmds = update(state, key(KeyCode::Enter), NOW_MS);
        let [Cmd::LoadReport { query, .. }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        query.clone()
    }

    #[test]
    fn f_opens_the_dialog_on_an_unfiltered_inventory() {
        let mut state = fresh(Screen::Tokens);
        state.start();
        let cmds = update(&mut state, ch('f'), NOW_MS);
        assert_eq!(
            cmds,
            [Cmd::LoadInventory {
                r#gen: 1,
                query: TokenQuery {
                    period: TokenPeriod::Month,
                    grouping: TokenGrouping::AccountModel,
                    since_ms: None,
                    ..token_query(TokenPeriod::Day)
                },
            }]
        );
        let lines = draw(&state, 80, 24);
        assert!(lines.iter().any(|l| l.contains(LOADING)), "{lines:?}");
    }

    #[test]
    fn the_choices_are_the_indexed_accounts_and_models_with_the_current_accounts() {
        // Spec §5.3: current, removed and unattributed accounts, the accounts view's (2 has no
        // usage yet), and the models; with no filter, every one is chosen.
        let state = dialog();
        assert_eq!(shown(&state), all(true));
        let lines = draw(&state, 80, 24);
        for text in [
            "Accounts",
            "[x] 1 a1@x.co",
            "[x] removed id-9",
            "Models",
            "[x] claude-sonnet-5-5",
        ] {
            assert!(lines.iter().any(|l| l.contains(text)), "{text}: {lines:?}");
        }
        assert!(lines.iter().any(|l| l.contains(KEYS)));
    }

    #[test]
    fn every_choice_of_a_kind_sends_none_and_fewer_send_exactly_them() {
        let mut state = dialog();
        let q = applied(&mut state);
        assert_eq!(
            (q.accounts, q.models),
            (None, None),
            "models indexed later are in too"
        );
        assert!(state.overlay.is_none());
        let mut state = dialog();
        for _ in 0..5 {
            update(&mut state, ch('j'), NOW_MS);
        }
        update(&mut state, ch(' '), NOW_MS);
        update(&mut state, key(KeyCode::Home), NOW_MS);
        update(&mut state, ch('k'), NOW_MS);
        update(&mut state, ch('j'), NOW_MS);
        update(&mut state, ch(' '), NOW_MS);
        let q = applied(&mut state);
        assert_eq!(q.models, Some(vec![OPUS.to_owned()]));
        assert_eq!(
            q.accounts,
            Some(vec![
                AccountSel::Account(id(1)),
                AccountSel::Account(AccountId::from_string("id-9")),
                AccountSel::Unattributed,
            ])
        );
        assert_eq!(state.tokens.depth(), 0);
    }

    #[test]
    fn the_choices_stay_the_whole_inventory_after_filtering_to_one_model() {
        let mut state = dialog();
        for _ in 0..5 {
            update(&mut state, ch('j'), NOW_MS);
        }
        update(&mut state, ch(' '), NOW_MS);
        applied(&mut state);
        let cmds = update(&mut state, ch('f'), NOW_MS);
        let [Cmd::LoadInventory { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        assert_eq!(query.models, None, "the inventory is never filtered");
        let result = Ok(indexed(query.clone()));
        update(
            &mut state,
            Msg::Inventory {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        let mut expected = all(true);
        expected[5].1 = false;
        assert_eq!(shown(&state), expected);
        let lines = draw(&state, 120, 40);
        assert!(lines.iter().any(|l| l.contains("[ ] claude-sonnet-5-5")));
        assert!(
            lines[1].contains("filter: all accounts · claude-opus-5-5"),
            "the header names the one model: {}",
            lines[1]
        );
    }

    #[test]
    fn none_of_a_kind_is_refused_in_the_dialog() {
        let mut state = dialog();
        for _ in 0..5 {
            update(&mut state, ch('j'), NOW_MS);
        }
        update(&mut state, ch('a'), NOW_MS);
        assert_eq!(
            &shown(&state)[4..],
            [(OPUS.to_owned(), false), (SONNET.to_owned(), false)]
        );
        assert!(update(&mut state, key(KeyCode::Enter), NOW_MS).is_empty());
        let lines = draw(&state, 80, 24);
        assert!(lines.iter().any(|l| l.contains(NO_MODEL)), "{lines:?}");
        update(&mut state, ch('a'), NOW_MS);
        update(&mut state, key(KeyCode::Home), NOW_MS);
        update(&mut state, ch('a'), NOW_MS);
        assert!(update(&mut state, key(KeyCode::Enter), NOW_MS).is_empty());
        let Some(Overlay::Filter(d)) = &state.overlay else {
            panic!("closed");
        };
        assert_eq!(d.error.as_deref(), Some(NO_ACCOUNT));
    }

    #[test]
    fn the_dialog_takes_every_key_and_esc_leaves_the_filter_as_it_was() {
        // R9: `q` and `1` are the dialog's while it is open.
        let mut state = dialog();
        update(&mut state, ch('q'), NOW_MS);
        update(&mut state, ch('1'), NOW_MS);
        assert_eq!((state.quitting, state.screen), (None, Screen::Tokens));
        update(&mut state, ch(' '), NOW_MS);
        assert!(update(&mut state, key(KeyCode::Esc), NOW_MS).is_empty());
        assert!(state.overlay.is_none());
        assert_eq!(state.tokens.query(NOW_MS).accounts, None);
    }

    #[test]
    fn an_inventory_for_a_dialog_closed_or_reopened_since_is_dropped() {
        let mut state = fresh(Screen::Tokens);
        state.start();
        let first = update(&mut state, ch('f'), NOW_MS);
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        let [Cmd::LoadInventory { r#gen, query }] = first.as_slice() else {
            panic!("{first:?}");
        };
        let result = Ok(indexed(query.clone()));
        update(
            &mut state,
            Msg::Inventory {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        assert!(state.overlay.is_none(), "closed stays closed");
        update(&mut state, ch('f'), NOW_MS);
        let result = Ok(indexed(query.clone()));
        update(
            &mut state,
            Msg::Inventory {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        assert!(
            shown(&state).is_empty(),
            "the reopened dialog waits for its own"
        );
        let failed = Err(SourceError {
            kind: "store".into(),
            message: "the store is locked".into(),
        });
        let r#gen = state.gens.inventory;
        update(
            &mut state,
            Msg::Inventory {
                r#gen,
                result: failed,
            },
            NOW_MS,
        );
        let lines = draw(&state, 80, 24);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("the choices could not be read: the store is locked")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_long_list_keeps_the_cursor_and_the_keys_in_view_at_80x24() {
        let mut state = fresh(Screen::Tokens);
        state.start();
        let cmds = update(&mut state, ch('f'), NOW_MS);
        let [Cmd::LoadInventory { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let month = PeriodKey::Month {
            year: 2026,
            month: 9,
        };
        let rows = (1..=40)
            .map(|n| {
                let model = format!("model-{n:02}");
                token_row(
                    month.clone(),
                    Some(row_account(1)),
                    Some(&model),
                    counts(1, 1, 1),
                    None,
                )
            })
            .collect();
        let result = Ok(token_report(query.clone(), rows));
        update(
            &mut state,
            Msg::Inventory {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        for _ in 0..40 {
            update(&mut state, ch('j'), NOW_MS);
        }
        let lines = draw(&state, 80, 24);
        assert!(
            lines.iter().any(|l| l.contains("[x] model-40")),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains(KEYS)), "{lines:?}");
        update(&mut state, key(KeyCode::Home), NOW_MS);
        let lines = draw(&state, 80, 24);
        assert!(lines.iter().any(|l| l.contains("Accounts")), "{lines:?}");
    }

    #[test]
    fn a_long_choice_wraps_under_its_mark_whole_and_the_cursors_shows_whole() {
        // B27: a choice wider than the dialog wraps under its mark, a word wider than a row
        // breaking by width; one that fits `CHOICE_ROWS` shows whole, and the list keeps the
        // cursor's whole choice in view.
        let alias = "a-rather-long-alias-for-the-work-account";
        let label = "averyveryverylongname.with.a.long.tail@example.com";
        let model = "claude-opus-5-5-with-a-very-long-suffix-and-then-some-more-of-it-yet-again";
        let mut state = fresh(Screen::Tokens);
        let mut accounts = many(2);
        accounts[1].row.alias = Some(alias.into());
        accounts[1].row.label = label.into();
        accounts[1].row.email = Some(label.into());
        state.accounts.apply(snapshot(accounts));
        state.start();
        let cmds = update(&mut state, ch('f'), NOW_MS);
        let [Cmd::LoadInventory { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let month = PeriodKey::Month {
            year: 2026,
            month: 9,
        };
        let mut rows: Vec<_> = (1..=12)
            .map(|n| {
                let name = format!("{model}-{n:02}");
                token_row(
                    month.clone(),
                    Some(row_account(1)),
                    Some(&name),
                    counts(1, 1, 1),
                    None,
                )
            })
            .collect();
        rows.push(token_row(
            month,
            Some(row_account(1)),
            Some(OPUS),
            counts(1, 1, 1),
            None,
        ));
        let result = Ok(token_report(query.clone(), rows));
        update(
            &mut state,
            Msg::Inventory {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        let squash = |text: &str| {
            text.chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
        };
        let inside = |lines: &[String]| -> Vec<String> {
            lines
                .iter()
                .filter(|l| l.contains('│'))
                .map(|l| l.split('│').nth(1).unwrap_or("").to_owned())
                .collect()
        };
        let lines = inside(&draw(&state, 80, 24));
        let read = squash(&lines.concat());
        assert!(
            read.contains(&squash(&format!("[x] 2 {alias} ({label})"))),
            "{lines:?}"
        );
        assert!(!lines.iter().any(|l| l.contains('…')), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains(KEYS)), "{lines:?}");
        update(&mut state, key(KeyCode::End), NOW_MS);
        let lines = inside(&draw(&state, 80, 24));
        assert!(
            squash(&lines.concat()).contains(&squash(&format!("[x] {model}-12"))),
            "the last choice whole: {lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains(KEYS)), "{lines:?}");
        // A long failure wraps too.
        let mut state = fresh(Screen::Tokens);
        state.start();
        update(&mut state, ch('f'), NOW_MS);
        let message = "the store could not be opened because another tagteam holds its lock";
        let failed = Err(SourceError {
            kind: "store".into(),
            message: message.into(),
        });
        let r#gen = state.gens.inventory;
        let msg = Msg::Inventory {
            r#gen,
            result: failed,
        };
        update(&mut state, msg, NOW_MS);
        let lines = inside(&draw(&state, 80, 24));
        let whole = format!("the choices could not be read: {message}");
        assert!(
            squash(&lines.concat()).contains(&squash(&whole)),
            "{lines:?}"
        );
    }

    #[test]
    fn a_choice_takes_three_rows_at_most_and_every_choice_and_the_keys_show() {
        // B29: a 1,400-column model shows its start on three rows, the last cut with `…`. The
        // dialog keeps its own rows for the keys and a failure, so at 80×24 the keys always
        // show, and so does whichever choice the cursor is on.
        let long = format!("claude-{}", "x".repeat(1393));
        let mut state = fresh(Screen::Tokens);
        state.start();
        let cmds = update(&mut state, ch('f'), NOW_MS);
        let [Cmd::LoadInventory { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let month = PeriodKey::Month {
            year: 2026,
            month: 9,
        };
        let mut models: Vec<String> = (1..=12).map(|n| format!("claude-test-{n:02}")).collect();
        models.push(long.clone());
        let rows = models
            .iter()
            .map(|m| {
                token_row(
                    month.clone(),
                    Some(row_account(1)),
                    Some(m),
                    counts(1, 1, 1),
                    None,
                )
            })
            .collect();
        let result = Ok(token_report(query.clone(), rows));
        update(
            &mut state,
            Msg::Inventory {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        let inside = |lines: &[String]| -> Vec<String> {
            lines
                .iter()
                .filter(|l| l.contains('│'))
                .map(|l| l.split('│').nth(1).unwrap_or("").trim_end().to_owned())
                .collect()
        };
        let lines = inside(&draw(&state, 80, 24));
        assert!(
            lines.iter().any(|l| l.contains("[x] 1 a1@x.co")),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains(KEYS)), "{lines:?}");
        // The long model is the last; End puts the cursor on it.
        update(&mut state, key(KeyCode::End), NOW_MS);
        let lines = inside(&draw(&state, 80, 24));
        let at = lines
            .iter()
            .position(|l| l.starts_with(" [x] claude-xxx"))
            .unwrap_or_else(|| panic!("{lines:?}"));
        assert!(lines[at + 1].contains(&"x".repeat(40)), "{lines:?}");
        assert!(lines[at + 2].ends_with('…'), "{lines:?}");
        assert!(lines[at + 3].contains(KEYS), "{lines:?}");
        // A long failure takes three rows too, and the dialog's own line still shows.
        let mut state = fresh(Screen::Tokens);
        state.start();
        update(&mut state, ch('f'), NOW_MS);
        let failed = Err(SourceError {
            kind: "store".into(),
            message: "x".repeat(1400),
        });
        let r#gen = state.gens.inventory;
        let msg = Msg::Inventory {
            r#gen,
            result: failed,
        };
        update(&mut state, msg, NOW_MS);
        let lines = inside(&draw(&state, 80, 24));
        let at = lines
            .iter()
            .position(|l| l.contains("the choices could not be read"))
            .unwrap_or_else(|| panic!("{lines:?}"));
        assert!(lines[at + 2].ends_with('…'), "{lines:?}");
        assert!(lines[at + 3].contains("Esc cancel"), "{lines:?}");
    }
}
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
            provider: ProviderId::new("claude-code"),
        };
        assert!(state(1).same_as(&state(2)));
        assert!(!state(1).same_as(&report(1, 7)));
        let refresh = |p: &str| IndexReq::Refresh {
            provider: ProviderId::new(p),
        };
        assert!(refresh("claude-code").same_as(&refresh("claude-code")));
```

with:

```rust
            provider: ProviderId::new("claude-code"),
        };
        assert!(state(1).same_as(&state(2)));
        assert!(!state(1).same_as(&report(1, 7)));
        let inventory = |g| ReportReq::Inventory {
            r#gen: g,
            query: query(0),
        };
        assert!(inventory(1).same_as(&inventory(2)));
        assert!(!inventory(1).same_as(&report(1, 0)));
        let refresh = |p: &str| IndexReq::Refresh {
            provider: ProviderId::new(p),
        };
        assert!(refresh("claude-code").same_as(&refresh("claude-code")));
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
            "drill down: sessions, then models",
            "back up a level",
            "refresh the token index",
            "scroll the detail and the notes",
        ] {
            assert!(lines.iter().any(|l| l.contains(what)), "{what}: {lines:?}");
        }
    }
```

with:

```rust
            "drill down: sessions, then models",
            "back up a level",
            "refresh the token index",
            "scroll the detail and the notes",
            "filter accounts and models",
        ] {
            assert!(lines.iter().any(|l| l.contains(what)), "{what}: {lines:?}");
        }
    }
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
    fn the_footer_says_what_enter_opens() {
        let mut state = showing_days();
        assert_eq!(
            draw(&state, 80, 24)[23],
            "Enter sessions  p period  g group  [ ] range  Esc back  r refresh"
        );
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer(&mut state, &down, vec![session("s1", 1)]);
        assert!(draw(&state, 80, 24)[23].starts_with("Enter models  p period"));
```

with:

```rust
    fn the_footer_says_what_enter_opens() {
        let mut state = showing_days();
        assert_eq!(
            draw(&state, 80, 24)[23],
            "Enter sessions  p period  g group  f filter  [ ] range  Esc back  r refresh"
        );
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer(&mut state, &down, vec![session("s1", 1)]);
        assert!(draw(&state, 80, 24)[23].starts_with("Enter models  p period"));
```

In `crates/tagteam-tui/src/screens/tokens/table.rs`, replace:

```rust
        );
        assert_eq!(lines[12], " API cost at list prices as of 2026-10-01.");
        assert_eq!(
            lines[23],
            "Enter sessions  p period  g group  [ ] range  Esc back  r refresh"
        );
    }

    #[test]
```

with:

```rust
        );
        assert_eq!(lines[12], " API cost at list prices as of 2026-10-01.");
        assert_eq!(
            lines[23],
            "Enter sessions  p period  g group  f filter  [ ] range  Esc back  r refresh"
        );
    }

    #[test]
```

In `crates/tagteam-tui/src/view.rs`, replace:

```rust
            )
        );
        assert_eq!(
            tokens[23],
            "p period  g group  [ ] range  Esc back  r refresh"
        );
    }

    #[test]
```

with:

```rust
            )
        );
        assert_eq!(
            tokens[23],
            "p period  g group  f filter  [ ] range  Esc back  r refresh"
        );
    }

    #[test]
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p tagteam-tui --lib`

Expected: FAIL: the build stops with ``error[E0599]: no variant named `Inventory` found for enum `msg::ReportReq` ``.

- [ ] **Step 3: Write the code**

In `crates/tagteam-tui/src/screens/tokens/filter.rs`, put this above the tests, with one blank line between:

```rust
//! The Tokens screen's filter (spec §5.3): a dialog of every account and model the index holds,
//! whatever the filter is now, each chosen or not. A multi-select overlay within part A's: it
//! takes every key but Ctrl-C and Ctrl-Z (R9), wraps a choice wider than itself under its mark
//! onto `CHOICE_ROWS` at most as part A's choose dialog does (B27, B29), and places its list
//! from the cursor each frame (R22).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::text::{Line, Span};
use tagteam_engine::tokens::{
    AccountSel, RowAccount, TokenGrouping, TokenPeriod, TokenQuery, TokenReport,
};

use super::table::account_name;
use super::{TokensState, load};
use crate::actions::{CHOICE_ROWS, Overlay, boxed};
use crate::msg::Cmd;
use crate::source::SourceError;
use crate::state::State;
use crate::view::{bold, scrolled_windows, selected, severity_style, wrap_rows};

/// Shown until the inventory arrives.
pub const LOADING: &str = "loading the accounts and models the index holds…";
pub const NO_ACCOUNT: &str = "Choose at least one account.";
pub const NO_MODEL: &str = "Choose at least one model.";
const KEYS: &str = "Space choose · a all · Enter apply · Esc cancel";

/// What a choice selects.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Account(AccountSel),
    Model(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Choice {
    kind: Kind,
    label: String,
    on: bool,
}

/// The filter dialog's state.
#[derive(Debug)]
pub struct FilterDialog {
    /// `None` until the inventory arrives.
    choices: Option<Vec<Choice>>,
    /// The choice the cursor is on.
    cursor: usize,
    /// Why Enter was refused, or why the choices could not be read.
    error: Option<String>,
}

/// The query whose rows are the inventory (spec §5.3): by month, account and model, over all
/// time, unfiltered, so the choices never depend on the filter in force.
fn inventory(t: &TokensState) -> TokenQuery {
    TokenQuery {
        provider: t.provider.clone(),
        period: TokenPeriod::Month,
        grouping: TokenGrouping::AccountModel,
        since_ms: None,
        until_ms: None,
        accounts: None,
        models: None,
        session: None,
        tz: t.tz.clone(),
    }
}

/// `f`: the dialog opens while its inventory is read on the report lane.
pub(super) fn open(state: &mut State) -> Vec<Cmd> {
    state.overlay = Some(Overlay::Filter(FilterDialog {
        choices: None,
        cursor: 0,
        error: None,
    }));
    state.gens.inventory += 1;
    vec![Cmd::LoadInventory {
        r#gen: state.gens.inventory,
        query: inventory(&state.tokens),
    }]
}

/// Where an account comes in the dialog: as a report orders them (token report §10.1), by
/// position, then removed accounts, then unattributed.
fn rank(a: &RowAccount) -> (u8, u32, String) {
    match a {
        RowAccount::Account { id, position, .. } => (0, *position, id.as_str().to_owned()),
        RowAccount::Removed { id } => (1, 0, id.as_str().to_owned()),
        RowAccount::Unattributed => (2, 0, String::new()),
    }
}

fn sel(a: &RowAccount) -> AccountSel {
    match a {
        RowAccount::Account { id, .. } | RowAccount::Removed { id } => {
            AccountSel::Account(id.clone())
        }
        RowAccount::Unattributed => AccountSel::Unattributed,
    }
}

/// The choices (spec §5.3): the inventory's accounts (current, removed and unattributed) and
/// the Tokens provider's accounts in the accounts view, then the inventory's models, with
/// whatever the filter in force names that neither has. Each is chosen as the filter is: all of
/// a kind for `None`.
fn choices(state: &State, report: &TokenReport) -> Vec<Choice> {
    let t = &state.tokens;
    let mut accounts: Vec<RowAccount> = Vec::new();
    let mut add = |a: RowAccount| {
        if !accounts.iter().any(|b| sel(b) == sel(&a)) {
            accounts.push(a);
        }
    };
    for row in &report.rows {
        row.account
            .iter()
            .chain(&row.accounts)
            .cloned()
            .for_each(&mut add);
    }
    if let Some(snap) = state.accounts.snapshot() {
        for v in snap
            .providers
            .iter()
            .filter(|p| &p.provider == t.provider())
            .flat_map(|p| &p.accounts)
        {
            add(RowAccount::Account {
                id: v.row.id.clone(),
                position: v.row.position,
                label: v.row.label.clone(),
                alias: v.row.alias.clone(),
            });
        }
    }
    for s in t.accounts.iter().flatten() {
        add(match s {
            AccountSel::Account(id) => RowAccount::Removed { id: id.clone() },
            AccountSel::Unattributed => RowAccount::Unattributed,
        });
    }
    accounts.sort_by_key(rank);
    let mut models: Vec<String> = report
        .rows
        .iter()
        .flat_map(|r| r.model.iter().chain(&r.models).cloned())
        .chain(t.models.iter().flatten().cloned())
        .collect();
    models.sort();
    models.dedup();
    let account = |a: RowAccount| Choice {
        on: t.accounts.as_ref().is_none_or(|f| f.contains(&sel(&a))),
        label: account_name(&a),
        kind: Kind::Account(sel(&a)),
    };
    let model = |m: String| Choice {
        on: t.models.as_ref().is_none_or(|f| f.contains(&m)),
        label: m.clone(),
        kind: Kind::Model(m),
    };
    accounts
        .into_iter()
        .map(account)
        .chain(models.into_iter().map(model))
        .collect()
}

/// The inventory from the report lane: the dialog's choices, if it is still open and this is
/// its newest read.
pub(crate) fn on_inventory(
    state: &mut State,
    r#gen: u64,
    result: Result<TokenReport, SourceError>,
) {
    if r#gen != state.gens.inventory || !matches!(state.overlay, Some(Overlay::Filter(_))) {
        return;
    }
    let read = result.map(|report| choices(state, &report));
    if let Some(Overlay::Filter(dialog)) = &mut state.overlay {
        match read {
            Ok(choices) => dialog.choices = Some(choices),
            Err(e) => dialog.error = Some(format!("the choices could not be read: {}", e.message)),
        }
        state.dirty = true;
    }
}

/// What the choices of a kind select: `None` when every one is chosen, so that what the index
/// gains later is in too; else exactly those chosen (spec §5.3).
fn selection<T>(choices: &[&Choice], pick: impl Fn(&Kind) -> Option<T>) -> Option<Vec<T>> {
    if choices.iter().all(|c| c.on) {
        return None;
    }
    Some(
        choices
            .iter()
            .filter(|c| c.on)
            .filter_map(|c| pick(&c.kind))
            .collect(),
    )
}

/// A key in the dialog: the cursor (`j`/`k`, arrows, `Home`/`End`), Space, `a` for all or none
/// of the cursor's kind, Enter to apply, Esc to cancel. Enter refuses a kind with none chosen (token report §10.1: `Some` of an
/// empty list matches nothing).
pub(crate) fn handle_key(
    state: &mut State,
    mut dialog: FilterDialog,
    key: KeyEvent,
    now_ms: i64,
) -> (Option<Overlay>, Vec<Cmd>) {
    if key.code == KeyCode::Esc {
        return (None, vec![]);
    }
    let Some(choices) = dialog.choices.as_mut() else {
        return (Some(Overlay::Filter(dialog)), vec![]);
    };
    let last = choices.len().saturating_sub(1);
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => dialog.cursor = dialog.cursor.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => dialog.cursor = (dialog.cursor + 1).min(last),
        KeyCode::Home => dialog.cursor = 0,
        KeyCode::End => dialog.cursor = last,
        KeyCode::Char(' ') if !choices.is_empty() => {
            choices[dialog.cursor].on = !choices[dialog.cursor].on;
            dialog.error = None;
        }
        KeyCode::Char('a') if !choices.is_empty() => {
            let account = matches!(choices[dialog.cursor].kind, Kind::Account(_));
            let mine = |c: &Choice| matches!(c.kind, Kind::Account(_)) == account;
            let every = choices.iter().filter(|c| mine(c)).all(|c| c.on);
            for c in choices.iter_mut().filter(|c| mine(c)) {
                c.on = !every;
            }
            dialog.error = None;
        }
        KeyCode::Enter => {
            let (accounts, models): (Vec<&Choice>, Vec<&Choice>) = choices
                .iter()
                .partition(|c| matches!(c.kind, Kind::Account(_)));
            let none = |kind: &[&Choice]| !kind.is_empty() && kind.iter().all(|c| !c.on);
            if none(&accounts) || none(&models) {
                let refusal = if none(&accounts) {
                    NO_ACCOUNT
                } else {
                    NO_MODEL
                };
                dialog.error = Some(refusal.to_owned());
                return (Some(Overlay::Filter(dialog)), vec![]);
            }
            let accounts = selection(&accounts, |k| match k {
                Kind::Account(s) => Some(s.clone()),
                Kind::Model(_) => None,
            });
            let models = selection(&models, |k| match k {
                Kind::Model(m) => Some(m.clone()),
                Kind::Account(_) => None,
            });
            let t = &mut state.tokens;
            t.accounts = accounts;
            t.models = models;
            t.reset();
            return (None, load(state, now_ms));
        }
        _ => {}
    }
    (Some(Overlay::Filter(dialog)), vec![])
}

/// The dialog (B7, B27, B29): the accounts, then the models, each `[x]` when chosen and wrapped
/// under its mark, the cursor's in reverse video; the list placed from the cursor so that its
/// whole choice shows, and the keys always below it. A choice and a failure take `CHOICE_ROWS`
/// at most, the rest cut with `…`, so the dialog's own lines and the cursor's choice always fit
/// at 80×24.
pub(crate) fn render(state: &State, dialog: &FilterDialog, frame: &mut Frame) {
    let area = frame.area();
    let w = 64u16.min(area.width.saturating_sub(4));
    let inner = usize::from(w).saturating_sub(4).max(1);
    let mut tail: Vec<Line<'static>> = Vec::new();
    if let Some(error) = &dialog.error {
        let style = severity_style(state.color, tagteam_engine::words::Severity::Critical);
        tail.extend(
            wrap_rows(error, inner, CHOICE_ROWS)
                .into_iter()
                .map(|row| Line::from(Span::styled(row, style))),
        );
    }
    let Some(choices) = &dialog.choices else {
        let mut lines: Vec<Line<'static>> = wrap_rows(LOADING, inner, CHOICE_ROWS)
            .into_iter()
            .map(Line::from)
            .collect();
        lines.extend(tail);
        lines.push(Line::from("Esc cancel"));
        boxed(frame, area, " Filter ", lines, w);
        return;
    };
    tail.extend(
        wrap_rows(KEYS, inner, CHOICE_ROWS)
            .into_iter()
            .map(Line::from),
    );
    // Each choice's rows, after its kind's heading when it is the first of its kind.
    let mut blocks: Vec<Vec<Line<'static>>> = Vec::new();
    let mut at = 0;
    for (i, c) in choices.iter().enumerate() {
        let account = matches!(c.kind, Kind::Account(_));
        let mut block = Vec::new();
        if i == 0 || matches!(choices[i - 1].kind, Kind::Account(_)) != account {
            let heading = if account { "Accounts" } else { "Models" };
            block.push(Line::from(Span::styled(heading, bold())));
        }
        let mark = if c.on { "[x] " } else { "[ ] " };
        for (n, row) in wrap_rows(&c.label, inner.saturating_sub(4).max(1), CHOICE_ROWS)
            .into_iter()
            .enumerate()
        {
            let lead = if n == 0 { mark } else { "    " };
            let line = Line::from(format!("{lead}{row}"));
            block.push(if i == dialog.cursor {
                line.style(selected())
            } else {
                line
            });
        }
        if i == dialog.cursor {
            at = blocks.len();
        }
        blocks.push(block);
    }
    // The box: the list, the tail, the border, and a row's margin above and below.
    let room = usize::from(area.height)
        .saturating_sub(tail.len() + 4)
        .max(1);
    let heights: Vec<usize> = blocks.iter().map(Vec::len).collect();
    // From the first choice that keeps the cursor's whole choice in view.
    let mut first = at;
    let mut used = heights.get(at).copied().unwrap_or(0);
    while first > 0 && used + heights[first - 1] <= room {
        first -= 1;
        used += heights[first];
    }
    let shown = scrolled_windows(&heights, first, room);
    let mut lines: Vec<Line<'static>> = blocks
        .into_iter()
        .enumerate()
        .filter(|(i, _)| shown.contains(i))
        .flat_map(|(_, block)| block)
        .collect();
    lines.extend(tail);
    boxed(frame, area, " Filter ", lines, w);
}
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
/// The report lane's requests: the token report, which reads the index only, and the index's
/// state (spec §4.3, §6.4).
#[derive(Debug, Clone, PartialEq)]
pub enum ReportReq {
    Report { r#gen: u64, query: TokenQuery },
    IndexState { r#gen: u64, provider: ProviderId },
}

/// The index lane's requests: the token index's catch-up, which can run for minutes on a cold
/// index (spec §4.3).
```

with:

```rust
/// The report lane's requests: the token report, which reads the index only, and the index's
/// state (spec §4.3, §6.4).
#[derive(Debug, Clone, PartialEq)]
pub enum ReportReq {
    Report {
        r#gen: u64,
        query: TokenQuery,
    },
    IndexState {
        r#gen: u64,
        provider: ProviderId,
    },
    /// The filter's inventory: an unfiltered report (spec §5.3).
    Inventory {
        r#gen: u64,
        query: TokenQuery,
    },
}

/// The index lane's requests: the token index's catch-up, which can run for minutes on a cold
/// index (spec §4.3).
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
        matches!(
            (self, other),
            (ReportReq::Report { .. }, ReportReq::Report { .. })
                | (ReportReq::IndexState { .. }, ReportReq::IndexState { .. })
        )
    }
}

```

with:

```rust
        matches!(
            (self, other),
            (ReportReq::Report { .. }, ReportReq::Report { .. })
                | (ReportReq::IndexState { .. }, ReportReq::IndexState { .. })
                | (ReportReq::Inventory { .. }, ReportReq::Inventory { .. })
        )
    }
}

```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
    /// The TUI's refresh of the index returned.
    Refreshed {
        result: Result<RefreshOutcome, SourceError>,
    },
    Prompt(Prompt),
    ActionDone {
        action: Action,
        outcome: ActionOutcome,
```

with:

```rust
    /// The TUI's refresh of the index returned.
    Refreshed {
        result: Result<RefreshOutcome, SourceError>,
    },
    /// The filter's inventory (spec §5.3).
    Inventory {
        r#gen: u64,
        result: Result<TokenReport, SourceError>,
    },
    Prompt(Prompt),
    ActionDone {
        action: Action,
        outcome: ActionOutcome,
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
    /// A refresh of the index, on the index lane (spec §6.4).
    RefreshIndex {
        provider: ProviderId,
    },
    Act(Action),
    /// Run `action` with the terminal handed back (spec §7.3).
    Suspend(Action),
    /// Begin the shutdown sequence (spec §10.2) with this exit code.
```

with:

```rust
    /// A refresh of the index, on the index lane (spec §6.4).
    RefreshIndex {
        provider: ProviderId,
    },
    /// The filter's inventory, on the report lane (spec §5.3).
    LoadInventory {
        r#gen: u64,
        query: TokenQuery,
    },
    Act(Action),
    /// Run `action` with the terminal handed back (spec §7.3).
    Suspend(Action),
    /// Begin the shutdown sequence (spec §10.2) with this exit code.
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
//! 5-hour block, grouped and filtered, with drill-down from a period to its sessions and from a
//! session to its models. Every query runs on the report lane (§4.3), never on the UI thread,
//! and the index's catch-up on the index lane (`index`).

pub mod index;
mod table;

use crossterm::event::{KeyCode, KeyEvent};
```

with:

```rust
//! 5-hour block, grouped and filtered, with drill-down from a period to its sessions and from a
//! session to its models. Every query runs on the report lane (§4.3), never on the UI thread,
//! and the index's catch-up on the index lane (`index`).

pub mod filter;
pub mod index;
mod table;

use crossterm::event::{KeyCode, KeyEvent};
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
        .collect();
    keys.extend([
        ("p", "period"),
        ("g", "group"),
        ("[ ]", "range"),
        ("Esc", "back"),
        ("r", "refresh"),
    ]);
    keys
}

/// The Tokens screen's keys (spec §5.6): `p` period, `g` group, `[` `]` range, `Enter` down a
/// level, `Esc`/`Backspace` up one, the rows, which keep the selected one in view, and `J`/`K`,
/// which scroll what lies below the total (B25).
pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    let body = crate::view::body_at(state);
    let page = table::page(state, body, now_ms).max(1) as isize;
    let t = &mut state.tokens;
```

with:

```rust
        .collect();
    keys.extend([
        ("p", "period"),
        ("g", "group"),
        ("f", "filter"),
        ("[ ]", "range"),
        ("Esc", "back"),
        ("r", "refresh"),
    ]);
    keys
}

/// The Tokens screen's keys (spec §5.6): `p` period, `g` group, `f` filter, `[` `]` range,
/// `Enter` down a level, `Esc`/`Backspace` up one, the rows, which keep the selected one in view,
/// and `J`/`K`, which scroll what lies below the total (B25).
pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    let body = crate::view::body_at(state);
    let page = table::page(state, body, now_ms).max(1) as isize;
    let t = &mut state.tokens;
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
            t.range = t.range.step(by);
            t.reset();
            return load(state, now_ms);
        }
        KeyCode::Char('J') | KeyCode::Char('K') => {
            let by = if key.code == KeyCode::Char('J') {
                1
            } else {
```

with:

```rust
            t.range = t.range.step(by);
            t.reset();
            return load(state, now_ms);
        }
        KeyCode::Char('f') => return filter::open(state),
        KeyCode::Char('J') | KeyCode::Char('K') => {
            let by = if key.code == KeyCode::Char('J') {
                1
            } else {
```

In `crates/tagteam-tui/src/state.rs`, replace:

```rust
    pub history: u64,
    pub collect: u64,
    pub report: u64,
    pub index: u64,
}

pub struct Refresh {
    pub next_ms: i64,
```

with:

```rust
    pub history: u64,
    pub collect: u64,
    pub report: u64,
    pub index: u64,
    pub inventory: u64,
}

pub struct Refresh {
    pub next_ms: i64,
```

In `crates/tagteam-tui/src/update.rs`, replace:

```rust
            screens::tokens::index::on_progress(state, progress);
            vec![]
        }
        Msg::Refreshed { result } => screens::tokens::index::on_refreshed(state, result, now_ms),
        Msg::Prompt(prompt) => {
            state.dirty = true;
            actions::on_prompt(state, prompt);
            vec![]
```

with:

```rust
            screens::tokens::index::on_progress(state, progress);
            vec![]
        }
        Msg::Refreshed { result } => screens::tokens::index::on_refreshed(state, result, now_ms),
        Msg::Inventory { r#gen, result } => {
            screens::tokens::filter::on_inventory(state, r#gen, result);
            vec![]
        }
        Msg::Prompt(prompt) => {
            state.dirty = true;
            actions::on_prompt(state, prompt);
            vec![]
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust

use crate::host::{AccountRef, Action, ActionOutcome, Strategy};
use crate::msg::{Answer, Cmd, Prompt, PromptKind};
use crate::screens;
use crate::state::{Screen, State};
use crate::view::{bold, centered, pad, scrolled_windows, selected, width_of, wrap, wrap_rows};

#[derive(Debug)]
```

with:

```rust

use crate::host::{AccountRef, Action, ActionOutcome, Strategy};
use crate::msg::{Answer, Cmd, Prompt, PromptKind};
use crate::screens;
use crate::screens::tokens::filter::{self, FilterDialog};
use crate::state::{Screen, State};
use crate::view::{bold, centered, pad, scrolled_windows, selected, width_of, wrap, wrap_rows};

#[derive(Debug)]
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
        text: String,
        scroll: u16,
        online: bool,
    },
}

/// What a confirm dialog's answer goes to: a waiting CLI prompt, or the TUI's own question
/// before an action (`Remove`'s).
```

with:

```rust
        text: String,
        scroll: u16,
        online: bool,
    },
    /// Tokens' filter (spec §5.3).
    Filter(FilterDialog),
}

/// What a confirm dialog's answer goes to: a waiting CLI prompt, or the TUI's own question
/// before an action (`Remove`'s).
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
    ("r", "refresh"),
    ("Enter", "history of the account"),
    ("J K", "scroll the detail's windows"),
];
const HELP_TOKENS: [(&str, &str); 7] = [
    ("p", "period: day, month, session or block"),
    ("g", "group: none, account, model, or both"),
    ("[ ]", "range: 7, 30 or 90 days, or all"),
    ("Enter", "drill down: sessions, then models"),
    ("Esc  Backspace", "back up a level"),
    ("r", "refresh the token index"),
```

with:

```rust
    ("r", "refresh"),
    ("Enter", "history of the account"),
    ("J K", "scroll the detail's windows"),
];
const HELP_TOKENS: [(&str, &str); 8] = [
    ("p", "period: day, month, session or block"),
    ("g", "group: none, account, model, or both"),
    ("f", "filter accounts and models"),
    ("[ ]", "range: 7, 30 or 90 days, or all"),
    ("Enter", "drill down: sessions, then models"),
    ("Esc  Backspace", "back up a level"),
    ("r", "refresh the token index"),
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
    ("J K", "scroll the windows"),
];

/// The rows a choice takes, at most (R21): a CLI's `<provider> #<n> <name> [<org>]` fits.
const CHOICE_ROWS: usize = 3;

/// The rows a dialog's question takes on a terminal `height` rows tall: a third of it at most,
/// so its buttons, choices or input always show (R21).
fn question_rows(question: &str, width: usize, height: u16) -> Vec<String> {
```

with:

```rust
    ("J K", "scroll the windows"),
];

/// The rows a choice takes, at most (R21): a CLI's `<provider> #<n> <name> [<org>]` fits.
pub(crate) const CHOICE_ROWS: usize = 3;

/// The rows a dialog's question takes on a terminal `height` rows tall: a third of it at most,
/// so its buttons, choices or input always show (R21).
fn question_rows(question: &str, width: usize, height: u16) -> Vec<String> {
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
                    vec![],
                )
            }
        },
    }
}

/// A confirm dialog's answer: sent to the prompt waiting for it, or a yes starts the action.
```

with:

```rust
                    vec![],
                )
            }
        },
        Overlay::Filter(dialog) => filter::handle_key(state, dialog, key, now_ms),
    }
}

/// A confirm dialog's answer: sent to the prompt waiting for it, or a yes starts the action.
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
                *scroll,
                "o online · Esc close · ↑ ↓ scroll",
            );
        }
    }
}

/// Choice `i`'s rows, `w` columns each: `N. ` and its text, which wraps at word boundaries under
```

with:

```rust
                *scroll,
                "o online · Esc close · ↑ ↓ scroll",
            );
        }
        Overlay::Filter(dialog) => filter::render(state, dialog, frame),
    }
}

/// Choice `i`'s rows, `w` columns each: `N. ` and its text, which wraps at word boundaries under
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
    Line::from(format!("  {}  {what}", pad(key, 20)))
}

/// `lines` in a bordered box `w` wide, as tall as they need, in the middle of `area`.
fn boxed(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>, w: u16) {
    let h = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2);
    let rect = centered(area, w.min(area.width), h);
```

with:

```rust
    Line::from(format!("  {}  {what}", pad(key, 20)))
}

/// `lines` in a bordered box `w` wide, as tall as they need, in the middle of `area`.
pub(crate) fn boxed(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>, w: u16) {
    let h = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2);
    let rect = centered(area, w.min(area.width), h);
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
                        ReportReq::IndexState { r#gen, provider } => Msg::IndexState {
                            r#gen,
                            result: source.token_index_state(&provider),
                        },
                    };
                    let _ = out.send(msg);
                },
                move || {
```

with:

```rust
                        ReportReq::IndexState { r#gen, provider } => Msg::IndexState {
                            r#gen,
                            result: source.token_index_state(&provider),
                        },
                        ReportReq::Inventory { r#gen, query } => Msg::Inventory {
                            r#gen,
                            result: source.token_report(&query),
                        },
                    };
                    let _ = out.send(msg);
                },
                move || {
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
                }
                Cmd::RefreshIndex { provider } => {
                    let _ = self.lanes.index.submit(IndexReq::Refresh { provider });
                }
                Cmd::Act(action) => {
                    let _ = self.lanes.actions.submit(ActionsReq::Act { action });
                }
                Cmd::Suspend(action) => self.suspend(action),
```

with:

```rust
                }
                Cmd::RefreshIndex { provider } => {
                    let _ = self.lanes.index.submit(IndexReq::Refresh { provider });
                }
                Cmd::LoadInventory { r#gen, query } => {
                    let req = ReportReq::Inventory { r#gen, query };
                    let _ = self.lanes.report.submit(req);
                }
                Cmd::Act(action) => {
                    let _ = self.lanes.actions.submit(ActionsReq::Act { action });
                }
                Cmd::Suspend(action) => self.suspend(action),
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-tui`

Expected: `234 passed` (the library; 10 in `screens::tokens::filter`) and `13 passed` (`tests/source.rs`).

- [ ] **Step 5: Format and lint**

Run:
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`

Expected: no output from the first; `Finished` and no warnings from the others.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-tui/src/screens/tokens/filter.rs crates/tagteam-tui/src/msg.rs crates/tagteam-tui/src/screens/tokens/mod.rs crates/tagteam-tui/src/screens/tokens/table.rs crates/tagteam-tui/src/view.rs crates/tagteam-tui/src/state.rs crates/tagteam-tui/src/update.rs crates/tagteam-tui/src/actions.rs crates/tagteam-tui/src/runtime.rs
git commit -m "Filter the Tokens screen by account and model from a dialog of the whole index"
```

### Task 7: `tagteam tui tokens`, docs, and the end-to-end and timing tests

`tagteam tui tokens` opens the TUI on the token report (spec §3), and the shell completions offer it (spec §12.2). The README and CHANGELOG say what the screen does.

The end-to-end tests run the real binary in a pty over Claude Code's transcripts:
- `2` and `tagteam tui tokens` show the report once the index has completed a refresh;
- a terminal Ctrl-C in an `R` session leaves a running index refresh to finish. The engine's `refresh-tokens-locked` pause point parks the refresh, holding its lock, across the session.

The timing test measures the first frame with a cold index whose catch-up starts after it (B5).

**Readings of the spec this task commits to:**
- **The pty tests' report** is one response a minute old; its 1,120 tokens show as `1.1K`. `refreshed ` in the status line says a refresh completed (B10).
- **The timing test forgets the index before each run** (`DELETE FROM token_files`), so each run catches up anew over some 7 MB of transcripts.

**Depends on:** 6

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `README.md`
- Modify: `crates/tagteam/src/cli.rs`
- Modify: `crates/tagteam/src/tui_cmd.rs`
- Modify: `crates/tagteam/tests/completions.rs`
- Modify: `crates/tagteam/tests/perf.rs`
- Modify: `crates/tagteam/tests/tui_pty.rs`

**Interfaces:**
- Consumes: `Screen::Tokens` (Task 3); the whole screen (Tasks 3–6); the tests' `common::{write_transcript, turn, Turn, now_epoch_s}`.
- Produces: CB7's `ScreenArg::Tokens`.

**Spec:**
- TUI spec §3 (`tui [accounts|tokens|history]`), §11 ("End to end": the screen keys work; Ctrl-C ends the stub and the TUI carries on, with an index refresh that was running not interrupted; "Performance"), §12.2 (completions).

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam/src/tui_cmd.rs`, replace:

```rust
    #[test]
    fn tui_opens_on_the_screen_it_names_and_on_accounts_otherwise() {
        for (args, screen) in [
            (&["tagteam", "tui", "history"][..], Screen::History),
            (&["tagteam", "tui", "accounts"], Screen::Accounts),
            (&["tagteam", "tui"], Screen::Accounts),
            (&["tagteam"], Screen::Accounts),
        ] {
```

with:

```rust
    #[test]
    fn tui_opens_on_the_screen_it_names_and_on_accounts_otherwise() {
        for (args, screen) in [
            (&["tagteam", "tui", "history"][..], Screen::History),
            (&["tagteam", "tui", "tokens"], Screen::Tokens),
            (&["tagteam", "tui", "accounts"], Screen::Accounts),
            (&["tagteam", "tui"], Screen::Accounts),
            (&["tagteam"], Screen::Accounts),
        ] {
```

In `crates/tagteam/tests/completions.rs`, replace:

```rust
            .map(|k| format!("provider.claude-code.{}", k.name)),
    );
    let keys: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    let shells = BTreeSet::from(["bash", "zsh", "fish"]);
    let screens = BTreeSet::from(["accounts", "history"]);
    let nothing = BTreeSet::new();
    for (line, offered) in [
        ("tagteam shell-init", &shells),
        ("tagteam config get", &keys),
```

with:

```rust
            .map(|k| format!("provider.claude-code.{}", k.name)),
    );
    let keys: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    let shells = BTreeSet::from(["bash", "zsh", "fish"]);
    let screens = BTreeSet::from(["accounts", "tokens", "history"]);
    let nothing = BTreeSet::new();
    for (line, offered) in [
        ("tagteam shell-init", &shells),
        ("tagteam config get", &keys),
```

In `crates/tagteam/tests/perf.rs`, replace:

```rust
//! §1.1's timing targets through the real binary: `statusline` within 10 ms p95, `list` within
//! 50 ms p95 when no usage fetch is due, alone and while a daemon runs against the same store
//! (D1.1 criterion 3, D12), and the TUI's first frame within 50 ms p95 under the same
//! conditions (TUI spec §1.1). Timings mean something only in an optimized build on an idle
//! machine, so this file exists only in release builds and its tests are ignored by default.
//! The TUI's opens a pseudo-terminal, so run them outside a sandbox:
//! `cargo test --release -p tagteam --features test-support --test perf -- --ignored`
//! (add `--nocapture` to see each measured p95).
#![cfg(all(feature = "test-support", not(debug_assertions)))]

```

with:

```rust
//! §1.1's timing targets through the real binary: `statusline` within 10 ms p95, `list` within
//! 50 ms p95 when no usage fetch is due, alone and while a daemon runs against the same store
//! (D1.1 criterion 3, D12), and the TUI's first frame within 50 ms p95 under the same
//! conditions (TUI spec §1.1), with the token index caught up or not. Timings mean something
//! only in an optimized build on an idle machine, so this file exists only in release builds
//! and its tests are ignored by default. The TUI's opens a pseudo-terminal, so run them
//! outside a sandbox:
//! `cargo test --release -p tagteam --features test-support --test perf -- --ignored`
//! (add `--nocapture` to see each measured p95).
#![cfg(all(feature = "test-support", not(debug_assertions)))]

```

In `crates/tagteam/tests/perf.rs`, replace:

```rust
use std::time::{Duration, Instant, SystemTime};

use common::pty::{Pty, Screen};
use common::{
    Guarded, bloat_claude_json, cc_profile, hold_launch, now_epoch_s, record_history,
    short_tempdir, std_cmd, two_fresh_accounts, usage_window,
};
use tagteam_core::{Window, WindowKind};
use tagteam_provider::Env;
use tagteam_provider::mock_server::MockServer;
```

with:

```rust
use std::time::{Duration, Instant, SystemTime};

use common::pty::{Pty, Screen};
use common::{
    Guarded, Turn, bloat_claude_json, cc_profile, hold_launch, now_epoch_s, record_history,
    short_tempdir, std_cmd, turn, two_fresh_accounts, usage_window, write_transcript,
};
use tagteam_core::{Window, WindowKind};
use tagteam_provider::Env;
use tagteam_provider::mock_server::MockServer;
```

In `crates/tagteam/tests/perf.rs`, replace:

```rust
    bloat_claude_json(root, CLAUDE_JSON_BYTES);
    (a, b)
}

/// The wall time of each of `RUNS` runs of `args`, after `WARM_UP` untimed ones, with every
/// endpoint pointed at `base` and `envs` set. `before(n)` runs untimed ahead of run `n`.
fn timings(
    root: &Path,
```

with:

```rust
    bloat_claude_json(root, CLAUDE_JSON_BYTES);
    (a, b)
}

/// `sessions` transcripts of `turns` Claude Code responses each, a minute apart, back from now:
/// with the defaults below, some 7 MB that a cold token index reads.
fn transcripts(root: &Path, sessions: usize, turns: usize) {
    let now_ms = now_epoch_s() * 1000;
    for s in 0..sessions {
        let all: Vec<Turn> = (0..turns)
            .map(|t| {
                let ago = (s * turns + t) as i64 * 60_000;
                turn("claude-opus-5-5", now_ms - ago, [100, 20, 30, 40, 1_000])
            })
            .collect();
        write_transcript(root, &format!("s-{s}"), &all);
    }
}

/// The wall time of each of `RUNS` runs of `args`, after `WARM_UP` untimed ones, with every
/// endpoint pointed at `base` and `envs` set. `before(n)` runs untimed ahead of run `n`.
fn timings(
    root: &Path,
```

In `crates/tagteam/tests/perf.rs`, replace:

```rust
        "tui first frame p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn statusline_in_a_run_shell_p95_is_within_10_ms() {
    // §12.8, §13.5: in a run shell the line names the marker's account. The marker is one
```

with:

```rust
        "tui first frame p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn tui_first_frame_p95_is_within_50_ms_while_the_index_catches_up() {
    // TUI spec §1.1 criterion 2 with a cold token index: its catch-up starts after the first
    // frame (B5), so the transcripts it reads never delay it. Before each run the index forgets
    // every file it has read, so each run catches up anew; `q` stops the catch-up (§9).
    let _one_at_a_time = serial();
    let d = tempfile::tempdir().unwrap();
    nothing_due(d.path());
    transcripts(d.path(), 50, 200);
    let db = Env::for_test(d.path()).data_dir().join("tagteam.db");
    let server = MockServer::start();
    let shown = |s: &Screen| s.on_alt() && s.contains("a@x.co");
    let run = || {
        let forget = rusqlite::Connection::open(&db).unwrap();
        forget.execute("DELETE FROM token_files", []).unwrap();
        drop(forget);
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
    eprintln!("tui first frame while the index catches up p95 {p:?} over {RUNS} runs");
    assert_eq!(server.requests().len(), 0, "a fetch was due");
    assert!(
        p <= Duration::from_millis(50),
        "tui first frame while the index catches up p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn statusline_in_a_run_shell_p95_is_within_10_ms() {
    // §12.8, §13.5: in a run shell the line names the marker's account. The marker is one
```

In `crates/tagteam/tests/tui_pty.rs`, replace:

```rust
use std::time::Duration;

use common::pty::{Finished, Pty, Screen};
use common::{
    FakeCall, cmd, fake_claude, fake_claude_calls, live_email, login, path_with, seed_home,
    std_cmd, two_fresh_accounts,
};
use tagteam_core::AccountId;
use tagteam_provider::{Env, FileKeychain, MARKER_FILE, profile_path};

```

with:

```rust
use std::time::Duration;

use common::pty::{Finished, Pty, Screen};
use common::{
    FakeCall, cmd, fake_claude, fake_claude_calls, live_email, login, now_epoch_s, path_with,
    seed_home, std_cmd, turn, two_fresh_accounts, write_transcript,
};
use tagteam_core::AccountId;
use tagteam_provider::{Env, FileKeychain, MARKER_FILE, profile_path};

```

In `crates/tagteam/tests/tui_pty.rs`, replace:

```rust
        "raw mode is off"
    );
}

/// Waits for the session `R` started to be running.
fn wait_for_session(home: &Home, tui: &mut Pty) {
    tui.wait_until(LONG, "the session", || {
        home.session().is_some_and(|c| c.ready)
```

with:

```rust
        "raw mode is off"
    );
}

/// One Claude Code response a minute ago, of 100 input, 20 output and 1,000 cache-read tokens:
/// 1.1K tokens in all.
fn one_response(home: &Home) {
    let at_ms = now_epoch_s() * 1000 - 60_000;
    let counts = [100, 20, 0, 0, 1_000];
    write_transcript(
        home.root(),
        "s-1",
        &[turn("claude-opus-5-5", at_ms, counts)],
    );
}

/// The Tokens screen, its report read from an index that has completed a refresh.
fn token_report(s: &Screen) -> bool {
    s.on_alt()
        && s.contains("[Day] Month Session Block")
        && s.contains("refreshed ")
        && s.contains("1.1K")
}

/// Waits for the session `R` started to be running.
fn wait_for_session(home: &Home, tui: &mut Pty) {
    tui.wait_until(LONG, "the session", || {
        home.session().is_some_and(|c| c.ready)
```

In `crates/tagteam/tests/tui_pty.rs`, replace:

```rust
    tui.type_keys(b"q");
    assert_eq!(tui.finish(LONG).status.code(), Some(0));
}

#[test]
fn tui_history_opens_on_history() {
    let home = Home::new();
    let mut tui = home.tui(&["tui", "history"]);
```

with:

```rust
    tui.type_keys(b"q");
    assert_eq!(tui.finish(LONG).status.code(), Some(0));
}

#[test]
fn two_shows_the_token_report_and_one_the_accounts_again() {
    // Spec §5.3, §6.4: the index is refreshed after the first frame, and the report read from
    // it on the report lane.
    let home = Home::new();
    one_response(&home);
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"2");
    tui.wait_for(LONG, "the token report", token_report);
    tui.type_keys(b"1");
    tui.wait_for(LONG, "the accounts table again", |s| {
        accounts(s) && !s.contains("[Day]")
    });
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_restored(&f);
}

#[test]
fn tui_tokens_opens_on_the_token_report() {
    let home = Home::new();
    one_response(&home);
    let mut tui = home.tui(&["tui", "tokens"]);
    tui.wait_for(LONG, "the token report", token_report);
    tui.type_keys(b"q");
    assert_eq!(tui.finish(LONG).status.code(), Some(0));
}

#[test]
fn tui_history_opens_on_history() {
    let home = Home::new();
    let mut tui = home.tui(&["tui", "history"]);
```

In `crates/tagteam/tests/tui_pty.rs`, replace:

```rust
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_restored(&f);
}

#[test]
fn run_on_an_account_moved_elsewhere_is_refused_and_launches_nothing() {
    // Spec §4.5, §11: another process moves `a` after the TUI read it, so position 1 holds `b`.
    let home = Home::new();
```

with:

```rust
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_restored(&f);
}

#[test]
fn under_r_ctrl_c_ends_the_session_and_a_running_index_refresh_goes_on() {
    // Spec §7.3, §11: the lanes watch only the shutdown token, so the terminal's Ctrl-C for the
    // session leaves the index's catch-up running, parked here with its lock held; it then
    // completes, and the report shows it.
    let home = Home::new();
    one_response(&home);
    let pause = home.root().join("pause");
    fs::create_dir(&pause).unwrap();
    let mut c = home.command(&["tui"]);
    c.env("TAGTEAM_TEST_PAUSE_AT", "refresh-tokens-locked")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause);
    let mut tui = Pty::spawn(c, true);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.wait_until(LONG, "the index's refresh, parked", || {
        pause.join("paused").exists()
    });
    tui.type_keys(b"R");
    wait_for_session(&home, &mut tui);
    tui.type_keys(b"\x03");
    tui.wait_for(LONG, "the wait for Enter", returning);
    tui.type_keys(b"\r");
    tui.wait_for(LONG, "the accounts table again", accounts);
    fs::write(pause.join("resume"), b"").unwrap();
    tui.type_keys(b"2");
    let shown = tui.wait_for(LONG, "the token report", token_report);
    assert!(
        !shown.contains("not refreshed"),
        "the refresh was not interrupted: {}",
        shown.text()
    );
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_restored(&f);
}

#[test]
fn run_on_an_account_moved_elsewhere_is_refused_and_launches_nothing() {
    // Spec §4.5, §11: another process moves `a` after the TUI read it, so position 1 holds `b`.
    let home = Home::new();
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p tagteam --features test-support --lib tui_cmd`

Expected: FAIL: the build passes (`Screen::Tokens` is Task 3's), and `tui_cmd::tests::tui_opens_on_the_screen_it_names_and_on_accounts_otherwise` panics on `unwrap()` with clap's `InvalidValue` for `tokens`, whose valid values are still `accounts` and `history`.

- [ ] **Step 3: Write the code**

In `CHANGELOG.md`, replace:

```markdown
  keys run tagteam's own commands (switch, enable and disable, alias, move, remove, add and
  doctor), and `R` runs a session as `tagteam run` does. It refreshes every 30 seconds, and
  collects usage only when `list` would. On a terminal, a bare `tagteam` now opens it; in a
  pipe, or with `--json`, it still runs `list`.

### Changed

- `tagteam purge` waits for a running token index refresh before it deletes a provider's data,
```

with:

```markdown
  keys run tagteam's own commands (switch, enable and disable, alias, move, remove, add and
  doctor), and `R` runs a session as `tagteam run` does. It refreshes every 30 seconds, and
  collects usage only when `list` would. On a terminal, a bare `tagteam` now opens it; in a
  pipe, or with `--json`, it still runs `list`.
- The TUI's Tokens screen (`2`, or `tagteam tui tokens`) shows the token report by day,
  month, session or 5-hour block, grouped and filtered by account and model, with drill-down
  from a period to its sessions and from a session to its models, and the active block's burn
  rate counting down. It brings the token index up to date in the background, and reads the
  report again as the index, the accounts or the clock move.

### Changed

- `tagteam purge` waits for a running token index refresh before it deletes a provider's data,
```

In `README.md`, replace:

````markdown
tagteam switch work     # switch to an account by position, alias or email
tagteam switch --strategy best   # switch to the account with the most headroom
```

On a terminal, `tagteam` on its own opens a terminal UI with the same accounts, their usage
history, and keys for these commands (`?` lists them, `q` quits). `tagteam tui history` opens
it on the history.

A running Claude Code session picks up the new login within about 30 seconds on macOS, and on
the next message on Linux. If an API-key account is on either side of the switch, restart
Claude Code.
````

with:

````markdown
tagteam switch work     # switch to an account by position, alias or email
tagteam switch --strategy best   # switch to the account with the most headroom
```

On a terminal, `tagteam` on its own opens a terminal UI with the same accounts, the token
report, their usage history, and keys for these commands (`?` lists them, `q` quits).
`tagteam tui tokens` opens it on the token report, and `tagteam tui history` on the history.

A running Claude Code session picks up the new login within about 30 seconds on macOS, and on
the next message on Linux. If an API-key account is on either side of the switch, restart
Claude Code.
````

In `README.md`, replace:

```markdown

| Command | What it does |
|---|---|
| `list` (`ls`), `status` | Show the stored accounts with usage, or only the live account |
| `tui` | A terminal UI: the accounts with their usage and pace, usage history, and keys for switching, enabling, aliasing, moving, removing, adding, running a session and `doctor` |
| `add`, `add-token` | Store the current Claude Code login, or an API key or setup token |
| `remove` (`rm`), `disable`, `enable` | Delete an account (never the live login), or hold it out of automatic selection |
| `alias`, `move` | Name an account, or change its position |
| `switch` | Switch to the next account, to a named one, or to the one a strategy picks (`best`, `next-available`) |
```

with:

```markdown

| Command | What it does |
|---|---|
| `list` (`ls`), `status` | Show the stored accounts with usage, or only the live account |
| `tui` | A terminal UI: the accounts with their usage and pace, the token report, usage history, and keys for switching, enabling, aliasing, moving, removing, adding, running a session and `doctor` |
| `add`, `add-token` | Store the current Claude Code login, or an API key or setup token |
| `remove` (`rm`), `disable`, `enable` | Delete an account (never the live login), or hold it out of automatic selection |
| `alias`, `move` | Name an account, or change its position |
| `switch` | Switch to the next account, to a named one, or to the one a strategy picks (`best`, `next-available`) |
```

In `crates/tagteam/src/cli.rs`, replace:

```rust
        ))
    }
}

/// The screen `tui` opens on (TUI spec §3). Part B adds `tokens`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ScreenArg {
    /// The accounts table (the default)
    Accounts,
    /// Usage history
    History,
}

```

with:

```rust
        ))
    }
}

/// The screen `tui` opens on (TUI spec §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ScreenArg {
    /// The accounts table (the default)
    Accounts,
    /// The token report
    Tokens,
    /// Usage history
    History,
}

```

In `crates/tagteam/src/tui_cmd.rs`, replace:

```rust
        Some(Command::Tui {
            screen: Some(arg), ..
        }) => match arg {
            ScreenArg::Accounts => Screen::Accounts,
            ScreenArg::History => Screen::History,
        },
        _ => Screen::Accounts,
    }
```

with:

```rust
        Some(Command::Tui {
            screen: Some(arg), ..
        }) => match arg {
            ScreenArg::Accounts => Screen::Accounts,
            ScreenArg::Tokens => Screen::Tokens,
            ScreenArg::History => Screen::History,
        },
        _ => Screen::Accounts,
    }
```

- [ ] **Step 4: Run the tests and see them pass**

Run:
- `cargo test -p tagteam --features test-support --lib tui_cmd` — Expected: `1 passed`.
- `cargo test -p tagteam --features test-support --test completions` — Expected: `8 passed`.
- Outside the sandbox: `cargo test -p tagteam --features test-support --test tui_pty` — Expected: `19 passed`.
- Outside the sandbox, on an idle machine: `cargo test --release -p tagteam --features test-support --test perf -- --ignored --nocapture tui_first_frame` — Expected: `2 passed`, each printing its p95 within 50 ms (`tui first frame p95 …` and `tui first frame while the index catches up p95 …`).
- The workspace suite: `cargo test --workspace --features tagteam/test-support --no-fail-fast` — Expected: 3708 of 3708 pass: 3623 in the sandbox, and outside it the 85 it refuses (`tagteam`'s library 10, `tests/daemon_cli.rs` 4, `tests/daemon_procs.rs` 5, `tests/signals.rs` 1, `tests/tui_pty.rs` 18, `tagteam-daemon`'s `client` 13, `server` 17 and `umask` 1, the engine's `tests/doctor.rs` 2, `xtask`'s compat tests 14).

- [ ] **Step 5: Format and lint**

Run:
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`

Expected: no output from the first; `Finished` and no warnings from the others.

- [ ] **Step 6: Commit**

```bash
git add CHANGELOG.md README.md crates/tagteam/src/cli.rs crates/tagteam/src/tui_cmd.rs crates/tagteam/tests/completions.rs crates/tagteam/tests/perf.rs crates/tagteam/tests/tui_pty.rs
git commit -m "Open the TUI on the token report with tagteam tui tokens, and test the Tokens screen end to end"
```
