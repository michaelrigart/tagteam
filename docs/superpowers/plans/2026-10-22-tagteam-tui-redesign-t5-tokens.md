# tagteam TUI redesign T5 — Tokens — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Implemented — https://github.com/michaelrigart/tagteam/pull/44
**Goal:** Give the Tokens screen §8's layout (one control bar, a headline with the active 5-hour block's progress, a table drawn in roles with plain words, a labelled detail and notes led by their marks), word the token report's two changed notes once for the TUI and `tagteam tokens`, slow the live re-query to 30 s (PERF-D-2), and close the redesign: its spec moves to `Implemented`.
**Architecture:** The screen stays `screens/tokens/` over the engine's unchanged `TokenReport`. `table.rs`'s three-row header is rewritten in place as the control bar and its breadcrumb row; the headline is a new `headline.rs`, thousands separators a new `amounts.rs`, the short model IDs a new `models.rs`; `gauge.rs` gains the block's progress line, the per-day sparkline and the share line. `words::token_footnotes` returns marked notes that both renderers print. The re-query throttle and the banner order are small edits to `index.rs` and `chrome.rs`.
**Tech Stack:** Rust 1.88 (edition 2024), ratatui 0.30.2, crossterm =0.29.0. No new dependency.
**Spec:** docs/superpowers/specs/2026-10-20-tagteam-tui-redesign-design.md (§1–§5, §8, §9.2, §11.1, §13–§19; §15's T5 row; §17's VIS-4 (Tokens), VIS-9, VIS-14, VIS-15); TUI spec `2026-10-14-tagteam-tui-design.md` §5.3, §6.4, §8 for what §8 does not change; token report spec (TR) §9–§11.
**Base:** main 50aec015 (T1–T4, R1–R4f2), rebased onto R4g if it merges first.

**Calls ruled within the spec** (listed for Michael under the PR's Rulings; an objection changes one ruling and its task).
1. **`tagteam tokens` prints the two changed notes with their marks** (`≥ at least: …`, `$ what this would cost …`), since it reads the same words (§15). Its `*` for a partial cost stays (T5-2).
2. **On Tokens a stale report's banner outranks a warn state banner, not a crit one** (T5-4). This rules T2's final review, Minor 3 (T2-14), which left it to T5.
3. **The time zone leaves the Tokens screen.** §8's control bar has none, and every time the TUI shows is in that one zone (T5-6).
4. **`today` and `per day` show on a Day report only**, the one whose rows are days (T5-12).
5. **The active block's figures under its row stay** (TUI §5.3), next to the headline's new progress line (T5-14).
6. **TR §11.2 gets a hunk for the two notes, and the redesign spec's §18 lists it** (T5-26).

**Reading this plan.**
- TUI paths start `src/` and are under `crates/tagteam-tui/`; Tokens' files are `src/screens/tokens/{mod,table,filter,index}.rs`. Every other path is written in full.
- Code is cited by the code quoted. Line numbers are hints as of `50aec015`.
- Focused tests: `cargo test -p tagteam-tui --locked screens::tokens`; `cargo test -p tagteam-engine --locked --lib words::`; `cargo test -p tagteam --locked --features test-support --test tokens_cli`. The pty suites (`tui_pty.rs`, `perf.rs`, `signals.rs`) run outside the sandbox.
- Each task leaves the workspace compiling, with its crates' tests green.
- Existing helpers: `testutil`'s `fresh`, `snapshot`, `many`, `account`, `read`, `draw`, `draw_at`, `draw_buffer`, `body_of`, `text_of`, `locate`, `roles`, `canon`, `TIERS`, `row_account`, `day`, `counts`, `token_row`, `token_query`, `token_report`, `go_live`, `NOW_MS` (Mon 2026-09-21 14:13:20 UTC); `table.rs`'s test helpers `showing`, `showing_after`, `days()`, `spare()`, `session()`, `block()`, `NOON`, `YESTERDAY`, `many_days`, `realistic()`, `widest()`, `work_models()`, `MODELS`, `scrolled_below`, `reads`; `index.rs`'s `on_tokens`, `live_on_tokens`, `with_index_job`, `token_index`, `answer`, `tick`, `live_tick`, `token_cmds`, `with_a_waiting_event`. Tests run in UTC unless a test names a zone.
- **`days()`** is the reference report: yesterday (2026-09-20) for account 1 (1.2M in, ≥310.0K out with 2 unfinalized, cache write 3.0M + 1.1M, 88.0M read, 412 responses, $41.20) and account 2 `spare` ($9.75), the day before unattributed and unpriced. Its total is 114.2M tokens, 414 responses, $50.95; its index was refreshed at 14:12.
- **The body** is the area under the chrome (header, auto line, Now strip, rule from 30 rows, banner): `body_of` returns its rows. At 80×24 with no banner it is 20 rows.
- **Hand-computed text.** Where a value this plan spells out disagrees with the rule it states, the rule wins: the implementer fixes the expectation to the rule and says so in the task report.

## Global Constraints

- **CI commands** (`CONTRIBUTING.md`): `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings`, and again with `--features tagteam/test-support`; `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`.
- **Toolchain:** Rust 1.88, edition 2024, ratatui 0.30.2, crossterm =0.29.0 (pinned). `tagteam-tui` stays `#![forbid(unsafe_code)]`. No `#[allow(...)]` is added.
- **No new dependency.** `git diff <base> -- Cargo.toml Cargo.lock 'crates/*/Cargo.toml'` prints nothing at the end.
- **No change at the edges:** no `--json` change (`tagteam tokens --json` and every other command), no store change, no migration, no index, no daemon protocol change (`crates/tagteam-daemon/src/protocol.rs` untouched). `tokens_cmd.rs` changes only in `footnotes`.
- **Provider neutrality:** no production line T5 adds under `crates/tagteam-tui/src`, `crates/tagteam-engine/src` or `crates/tagteam-daemon/src` names `claude` or `anthropic` in any case, nor uses `tagteam_cc`. The short model ID rule knows no provider's naming (ARCH-1): it works on separators and date stamps only. `no_production_line_names_the_provider` and `crates/tagteam-fake/tests/neutrality.rs` run. Test modules may use real-format model IDs.
- **Example names only** (§3.4): `alex@example.com`, `sam@corp.example`, `alex@personal.example`, `Example Corp`, the fixtures' `a1@x.co` style, `/Users/alex/Code/…` paths. Display names are the providers' own.
- **No test is weakened.** A test edit is a mechanical change to an API this plan changes (listed in its task), a render test rewritten to the new layout per §16 (named in its task), or a new test. Timing tests pin which ending fired (the read at 30 s, not at 5 s), never scheduler latency.
- **A type, field or function is added in the task that first uses it**; dead code is deleted, or `#[cfg(test)]` when only tests use it.
- **Moves are separate from edits.** No task moves code between files: the old header is rewritten in place in `table.rs`; new drawing code goes in new files.
- **Commits:** imperative, sentence case, no prefix, no trailing period, no agent attribution; each task's message is in its last step. A task's commit stays under about 90 KB (`git show --format= <sha> | wc -c`). A task that would exceed it tells the controller, who splits its review by file. Tasks 4, 6 and 9, which rewrite many render tests, are the ones at risk.
- **Words.** The two changed notes live in `crates/tagteam-engine/src/words.rs` (Task 1); every other footnote keeps its words (§11.1). TUI-only words (`at API prices`, `SELECTED`, `indexed`, `per day`, the bar's words) live in the TUI file that draws them. The CLI's money form, `$1541.64`, is unchanged (§11.1): thousands separators are the TUI's (C-10).
- **Reuse:** `autoline::when` (§5.3's time form), `chrome::{WIDE, fit_spans}`, `keys::RIGHT_INSET`, `view::{fit, width_of, wrap, select, keep_visible, centred_block}`, `theme::Role`, `words::{scaled, plural, short_name, email, MISSING, duration}`, `table::bytes`.
- **The first frame.** `tests/perf.rs`'s first-frame p95 stays within 50 ms; T5 adds no read, and nothing runs before the first frame.
- **Files this plan may touch:** `crates/tagteam-tui/**`; `crates/tagteam-engine/src/words.rs` (the footnotes only); `crates/tagteam/src/tokens_cmd.rs` (`footnotes` only); `crates/tagteam/tests/{tokens_cli,tui_pty}.rs`; `CHANGELOG.md`; the TUI, TR and redesign specs (Task 11 and the status commit); this plan. Not edited: `crates/tagteam-core/**`, `crates/tagteam-provider/**`, `crates/tagteam-cc/**`, `crates/tagteam-daemon/**`, `crates/tagteam-engine/src/{store,tokens,views.rs}`.
- **Never committed:** the review harness (`src/ux_*.rs` and their `lib.rs` lines) and any render. Before every commit the controller checks `git status --short` lists no `ux_` file.

### What T5 takes from the refactor

Nothing by name. T5 reads only the token report's types (`TokenReport`, `TokenRow`, `PeriodKey`, `RowAccount`, `ActiveBlock`, `IndexState`, `Cost`), which no remaining slice changes. Overlaps are by file only:

| Slice | Touches | T5 |
|---|---|---|
| R4g | `words.rs` (`email`'s body), `views.rs`, `protocol.rs` (4 lines), engine and daemon files | Task 1 edits `token_footnotes`, another hunk of `words.rs`; T5 calls `words::email`/`short_name`, whose signatures R4g keeps |
| R4h | the switch path (PERF-R-1) | none |
| R5a/R5b | `tokens/evidence.rs` (about 8 lines), `liveness.rs` | none: T5 reads the report, not the evidence |

## Rulings

Calls inside the spec, for the PR's "Rulings" section.

| # | Ruling | Why |
|---|---|---|
| T5-1 | **Base and order.** Planned on main 50aec015. R4g overlaps only by file (table above); the controller rebases where "Before Task 1" says. A name a merged slice changes is patched into this plan before the task that uses it. | §3.2: T5 waits for R4; the brief. |
| T5-2 | **The footnotes** (§8, §11.1, §15). `words::token_footnotes(report, mark, unreadable) -> Vec<Footnote>`, `Footnote { mark: Option<&'static str>, text }`, `Footnote::line()` = `{mark} {text}` or the text. Two notes change: the lower bound's is `≥` + `at least: {plural(n, "subagent response")} {is|are} still being counted`; the price table's is `$` + `what this would cost on the API at list prices of {date}; not your bill`, with overrides `… of {date}, with config.toml's for {models}; not your bill`. No period ends either (§8's text). Every other note keeps its words and has no mark. `tagteam tokens` prints `line()` (planned call 1); `--json` is unchanged. `words::AT_LEAST = "≥"` and `words::PRICE_MARK = "$"`; the TUI's own `AT_LEAST` goes. | §8 gives both notes with their marks; §15: `tagteam tokens` reads the same words. |
| T5-3 | **The live re-query** (C-13, PERF-D-2). `LIVE_REQUERY_MS` = 30 s. While the TUI's own refresh runs (`watch.refreshing`), the pace is `LIVE_REFRESHING_MS` = `CHECK_MS` (5 s). `IndexWatch.live_read_ms: Option<i64>` (when an event last read the report) replaces `live_next_ms`; an event is due at `live_read_ms + pace`, the pace judged when it is due, so a refresh that starts or ends moves the due time. `r` on Tokens sets `live_read_ms = None`: the next newer event, or one held, reads at once (at the next tick). An event that comes sooner is held (`live_due`), as today. | C-13's two exceptions; one stored instant, so no transition needs its own code. |
| T5-4 | **Banner order** (T2-14's carry). First match: held keys; a crit state banner (a failed accounts or timeline read, a lane's death); on Tokens, a stale report's words (T2-22's `couldn't read the token index (…) · r retries`); a warn state banner (a collection's warnings, a refused hand-over); on Tokens, its other errors (a failed check or refresh); the start-up warnings; the accounts view's. Planned call 2. | §5.7: the DIM report sits under its own banner; a crit failure still says the TUI itself is in trouble. Only the stale report's place changes. |
| T5-5 | **The rows above the table** (§8). From the body's top: the control bar; the breadcrumbs, only while drilled; the headline (Task 5 may add a second row for the block line); a blank row. Then the table's head. A level with no report yet shows the bar (and crumbs) then ` loading…`; an empty report the bar (and crumbs) then today's empty lines and centred block. A stale report draws the headline, the table and what lies below DIM; the bar and the crumbs stay plain (they are the controls). The rows above the table are 3 or 4. | Undrilled, the table's head stays at body row 3, as under the old header. |
| T5-6 | **The bar's parts** (§8, VIS-8). ` p` then the four periods `Day`, `Month`, `Session`, `5h block` two spaces apart; `[ ]` and the range; `g` and the grouping; `f` and the filter; groups five spaces apart (§8's text). Keys accent; the current period, the range, the grouping and the filter text; the other periods muted; nothing bold. Grouping words for the applied top grouping: `no grouping`, `by account`, `by model`, `by account and model`. Range words: `7 days`, `30 days` (`12 months` for Month), `90 days`, `all time` (was `all`). The filter: `all accounts · all models`; one account by `words::short_name` (`2 work`), several `2 accounts`; `unknown account` for the unattributed bucket; one model by its full ID, several `3 models`. On the right, ending `RIGHT_INSET` in, the index words (T5-8), muted. The zone is no longer shown (planned call 3). | §8's control bar; §4.3's group gap; the zone is the one every TUI time uses. |
| T5-7 | **The bar gives way** (§4.5). The first of these forms that fits (`left + 3 + right + RIGHT_INSET ≤ width`) is drawn: 1. as T5-6; 2. groups three spaces apart; 3. form 2 without ` · updating…`; 4. three-space groups, the current period alone, and an unfiltered filter as `all`; 5. form 4 without ` · updating…`; 6. form 5 with the filter cut by `fit` to the room the right part leaves, no narrower than `FILTER_LEAST` (20) or its own width; 7. form 6 with the right part cut from its end by `fit_spans` (dropped under 2 columns). | An unfiltered bar holds every index state at 80; `updating…` gives way before the periods, and the filter before the index words. |
| T5-8 | **The index words** (§8's `indexed 14:12`; TUI §5.3 "Indexing"), joined ` · `: the TUI's own refresh as `indexing {done} / {total}`, `done`'s unit dropped when it is `total`'s (`indexing 1.2 / 3.3 GB`, `indexing 900.0 MB / 3.3 GB`), or `indexing…` before a total; else, after `Busy`, `catching up elsewhere`; else `indexed {autoline::when(refreshed_at_ms)}` in the screen's zone. Then `catching up` while the index has never completed a refresh and the TUI's refresh is not running, and `updating…` while a query of the shown report runs. | Today's states in shorter words, so the bar holds them at 80. |
| T5-9 | **Breadcrumbs** (§8, VIS-15). A row under the bar while drilled: ` › {crumb} › {crumb}`, muted, each crumb its level's selected row's parts joined ` · `: the period (a session as `session {id}`), the account (`account_text`) when the row has one, the model when it has one. Cut by `fit` to the width less one. The filter is on the bar, so it keeps its room. | §8: "the filter keeps its room before them". |
| T5-10 | **The headline's left part** (§8, VIS-5, VIS-9). ` {cost} at API prices    {tokens} tokens    {responses} responses`, four spaces apart (§8's text): the total cost by `amounts::dollars`, `≥` before a partial one (warn), `—` for none, bold (the row's one bold); the total's tokens by `words::scaled` (`1 token` for one); the responses by `amounts::grouped` (`1 response`); values text, labels muted. Shown while the report has rows. | §8; §4.3: one bold per row. |
| T5-11 | **Thousands separators** (C-10) are the TUI's: `amounts::grouped(9891) == "9,891"`, `amounts::dollars` is `words::dollars`' rounding with the whole part grouped (`$1,541.64`). They go on the headline, `API $` cells and total, `RESPONSES` cells and total, the detail's responses, `today`, the block line, and the active block's figures under its row. Token counts stay scaled. | §11.1 keeps the CLI's money form. |
| T5-12 | **`today` and `per day`** (§8), on a report whose period is Day, from `WIDE` (120) columns. After the responses, `    today {cost}` (`today` muted; the sum of today's rows' costs in the report's zone, partial (`≥`, warn) when any of its rows' costs is partial or none, `$0.00` with no row today, `—` with no priced row). On the right, ending one column in, `per day ` (muted) and a sparkline (accent) of the last `min(30, range days)` dates to today (7, 30, 30, 30 for the four ranges), each date's cost summed over its rows: no row is a space, a date with rows but no price is 0. Each on `▁…█` against the largest shown (`gauge::sparkline_of`). With too little room it keeps the latest dates, down to 7, else it is dropped. Planned call 4. | §8's sparkline of cost per day; only a Day report's rows are days. |
| T5-13 | **The block line** (§8, VIS-14), on a report whose period is Block: `▶ block {gauge} {left} left · {rate} tok/min · {cost}/h · ≈ {end} at its end`. The block is the Tokens provider's account in use's active one (`now < end_ms`), else the first active row in report order; its rows by model add (rate, cost per hour, projected cost; a sum is partial when any part is partial or has no cost (`None`), and none only when every part is none). The gauge (`gauge::progress_line`, 22 cells) is the elapsed share `(now − start) / (end − start)`; `{left}` is `words::duration` of the seconds left (`figures_clock`'s `left`, `(end_ms − now_ms) / 1000`; `words::duration` takes seconds); both tick between queries. `{rate}` is `scaled(round(tokens_per_min))`; `{cost}/h` by `amounts::dollars`; `≈ {end}` by `amounts::about` (whole dollars from $10, else cents). A part with no cost is left out; a partial cost reads `≥` in warn. Roles: `▶` accent, `block`, `left`, `tok/min`, `/h`, `at its end` and ` · ` muted, values text. It sits on the headline's row, right-aligned one column in, when it fits whole after a 3-column gap; else on its own row under the headline, from column 1, ending one column in, where the gauge shrinks to fit (at least 8 cells), then ` · {rate} tok/min` goes, then ` · {cost}/h`, then it is cut. At the block's end it goes. | §8's line; there can be one active block per account (TR §9, §10.1). |
| T5-14 | **The row figures stay.** The active block's lines under its row (`figures`, TUI §5.3 "Blocks") are kept, with T5-11's separators, and so is T1's layout cache (R-5). Planned call 5. | TUI §5.3 holds where §8 is silent. |
| T5-15 | **Heads and roles** (§8, §4.3). Heads muted capitals, never bold: `API $` (was `COST`), `RESPONSES` (was `RESP`). Cells: TOTAL and `API $` text, INPUT, OUTPUT, the cache columns and RESPONSES muted, a `≥` cell warn, a `—` cell faint (§4.1), the period, project, account and model text. | §8's table typography. |
| T5-16 | **Cache columns** (§8, §4.5). From `WIDE`: `CACHE W` (both halves) and `CACHE R`. From 160 columns (`CACHE_SPLIT_FROM`), `CACHE W` splits into `CACHE W 5M` and `1H` when that still fits (today's `natural + extra ≤ width` test). | "only from 160 columns". |
| T5-17 | **The share line** (§4.4) replaces the `█` cost bar: `gauge::share_line(cost / largest × 100, 8)`, `━` whole cells and a `╸` half cell in accent, no track, at least `╸` for a cost above 0, nothing for none. The column stays headless. | §4.4's share line. |
| T5-18 | **The total.** A faint `─` rule, from column 1 across the table's laid-out width, sits above the total line at every height (§4.3). The total line bolds `TOTAL` alone; its numbers take their columns' roles. `below_rows` counts the head, the rule and the total. | §4.3: one bold per row; the rule above TOTAL. |
| T5-19 | **Accounts and IDs** (§8, VIS-15). A table cell says `removed account` and `unknown account` (was `removed`, `unattributed`). `account_name` (detail, filter dialog, SELECTED) says `removed account {first 8}` and `unknown account`. A session's ID is its first 8 characters in breadcrumbs and the detail (`session 3f2a9c1d`). `short_id` counts characters, not bytes. | §8. |
| T5-20 | **Projects** (§8). A session's project cell is its basename: the last non-empty component split on `/` or `\`, else the path as stored; cut from its end by `fit`; `fit_left` goes. In `Table::fit` the project's least is its width up to `PROJECT_KEEP` (20) on the first pass, so the period compacts before the project is cut; with no span level fitting, the second pass gives it `PROJECT_MIN` (4); then it leaves, as today. The leftover order is unchanged. The detail keeps the full path. | "outranks the period's long form when the width runs short"; §19's widths. |
| T5-21 | **Estimated blocks** (§8). `Col::Kind` goes. When the report holds an inferred block, every block's period cell gets one leading column: `~` for an inferred one, a space for an observed one; with none, no column. The `~` takes the cell's role. Breadcrumbs lead an inferred block with `~`. The detail says `observed` or `estimated` (was `inferred`). | Help names `~` "estimated" (§4.4, §9.2). |
| T5-22 | **Short model IDs** (§8, §19). Over the report's distinct models (rows' `model` and `models`): drop a trailing date stamp (`-` or `@`, then 8 ASCII digits or `YYYY-MM-DD`, after at least one character); then drop the first segment, up to and including the first of `- . / : _`, when every model's stem starts with that same segment and is longer than it. A model whose short form is empty, equals another model's short form, or equals another model's shown full ID shows its full ID, repeated until none changes. Short in the Model column and the breadcrumbs; full in the detail, the filter dialog, the bar and the footnotes. Computed once per report (`Level.models`, reset by `apply`). | §8's example `claude-opus-4-1-20250805` → `opus-4-1`; generic separators and dates only (ARCH-1). |
| T5-23 | **SELECTED** (§8). Below the total: a blank row; ` SELECTED  ` (muted, `DETAIL_INDENT` = 11 columns) then the row's identity in text, joined ` · ` (muted): its first detail part (the date, month, `session {id8}`, or the block's span) and, when the row has one account, `account_name`; then `   ⏎` (accent) ` its sessions` or ` its models` (muted) while `drill_to` is `Some`. Then, muted, under the identity: the numbers (responses, cache write with its halves, cache read), then the rest (project path, times, `observed`/`estimated`, every account when the row has none of its own, every model in full). All laid out by `laid_out` at `DETAIL_INDENT`, never cut. | §8's labelled section. |
| T5-24 | **The notes** (§8). After a blank row, each note: ` {mark} {text}`, `≥` warn and `$` muted, the text muted; a note without a mark is indented to the same column (`   {text}`); continuation rows indent 3. The `J K: lines a–b of n` hint is muted (was bold), as T3's card hint is. | §8: "each led by its mark"; PR #35's ruling that History, Tokens and the cards share a muted hint. |
| T5-25 | **Help** (§9.2). Marks: `≥` in warn, "at least: still being counted, or a cost missing some prices"; `~` in text, "estimated: a 5h block placed by rule, not read from a reset". After `%/h`, a second lead `5M 1H`: "CACHE W 5M and 1H, from 160 columns: cache writes kept 5 minutes and 1 hour; CACHE W is both". Key rows: `p` "period: day, month, session or 5h block"; `[ ]` "range: 7, 30 or 90 days, or all time". | §9.2: the legend explains `≥`, `~`, `CACHE W 5M`/`1H`. |
| T5-26 | **Records** (§18). TUI spec §5.3 (mock-up, columns, rows that need a mark, footnotes, drill-down, blocks, indexing, zone) and §8 (`token-index`'s pace), each hunk ending *(Amended by the TUI redesign spec §8.)* or *(… §14, C-13.)*; TR §11.2 (the two notes' words); the redesign spec's §18 gains "**Token report spec:** §11.2 (the two notes' words)"; `CHANGELOG.md`. Planned call 6. | §18; T1 recorded its amendment in §18 the same way. |
| T5-27 | **The status commit.** After the PR opens, one commit sets this plan's `**Status:**` to `Implemented — <PR URL>` and the redesign spec's from `In progress — …/pull/40` to `Implemented — …/pull/28, …/29, …/35, …/40, <T5 PR>`. | §15: the last PR moves the spec to Implemented, citing every PR. |

## Review Focus

1. **A report with more than one active block**: two accounts each inside a 5-hour block, or one block by model. The headline names the account in use's, with its model rows added, never one model's share. Test: Task 5, `the_block_line_is_the_account_in_use_s_and_adds_its_model_rows`.
2. **Model IDs the short rule would make alike**: two date stamps of one model, a dated and an undated ID, one model alone, no shared first segment. Each shows unambiguously. Test: Task 8, `short_ids_drop_a_date_stamp_and_a_shared_lead_and_stay_apart`.
3. **A day boundary in the report's zone**: at 14:13 UTC it is already tomorrow in a +14:00 zone. `today` and the last sparkline cell are that zone's date. Test: Task 5, `today_and_the_per_day_cells_follow_the_report_s_zone`.
4. **`r` while the 30 s throttle holds an event.** The held event reads at the next tick, not 30 s later. Test: Task 2, `r_reads_a_held_token_index_event_at_the_next_tick`.
5. **The widest values at 80 columns**: ten-digit counts, a six-figure partial cost with separators, a long filter during a refresh. No row is wider than 80, no number is cut, and the bar's keys stay. Tests: Task 4, `the_bar_cuts_a_long_filter_and_keeps_its_keys`; Task 6, `every_required_value_shows_whole_at_80_columns_in_the_widest_report` (rewritten).

## Spec coverage

| Spec | Tasks |
|---|---|
| §8 control bar (VIS-8's Tokens part), breadcrumbs (VIS-15), the zone | 4 |
| §8 headline (VIS-5's Tokens part, VIS-9), C-10 separators | 4, 6 |
| §8 `today` and `per day`; the block line (VIS-14) | 5 |
| §8 table typography; §4.3; §4.4 share line; `CACHE W 5M`/`1H` from 160 (§4.5); VIS-4 (Tokens) | 6 |
| §8 `removed account`, `unknown account`, 8-character IDs (VIS-15), project basename, `~` blocks | 7 |
| §8 short model IDs; §19 | 8 |
| §8 SELECTED and the notes led by their marks | 9 |
| §8 failed re-query DIM (VIS-10) | built in T2 (T2-22); kept by 4; the banner order in 3 |
| The token report's footnote words, `tagteam tokens` (§15, §11.1) | 1 |
| PERF-D-2, C-13 (§14) | 2 |
| T2-14's carry (T2's final review, Minor 3) | 3 |
| §9.2 legend; §10's Tokens rows | 10 |
| §16 render tests per tier at 80×24 and 120×40; state machine | 2–9 |
| §18 TUI §5.3 and §8, TR §11.2; CHANGELOG | 11 |
| §15 status: `Implemented`, every PR cited | After Task 11 |

**Order.** 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11. Every task builds and tests the shared `tagteam-tui` crate, so one implementer runs at a time.

**Out of T5:** nothing left for a later PR; T5 is the redesign's last.

---

## Before Task 1 (controller)

- [ ] `git -C /Users/michael/Code/tagteam-tui-redesign status --short` prints nothing; the branch is `tui-redesign-t5` on main (`50aec015`, or later main). Record it as `<base>`.
- [ ] **The rebase onto R4g** (T5-1). If R4g is on main (`git -C /Users/michael/Code/tagteam-tui-redesign grep -n 'fn try_target' origin/main -- crates/tagteam-engine/src/auto.rs` prints a hit; a rebase merge gives its commits new hashes), rebase the branch (only the plan's commit) onto `origin/main` and record the new `<base>`. If R4g merges later, rebase at the next task boundary: no T5 file but `words.rs` overlaps it, and their hunks are apart (`email` against `token_footnotes`). After a rebase, `cargo test -p tagteam-engine --locked --lib words::` and `cargo test -p tagteam-tui --locked` pass. If R4g has not merged by "After Task 11", rebase before the pre-merge review.
- [ ] **Names this plan uses.** Each prints at least one hit; a miss is patched into this plan first:
  - `rg -n 'pub fn token_footnotes|pub fn (dollars|scaled|plural|short_name|email)\(' crates/tagteam-engine/src/words.rs`
  - `rg -n 'pub const LIVE_REQUERY_MS: i64 = 5_000|live_next_ms|pub\(crate\) fn refresh_now' crates/tagteam-tui/src/screens/tokens/index.rs`
  - `rg -n 'fn header\(state: &State|fn crumbs|fn filter_text|fn grouping_text|pub fn bytes|const FILTER_LEAST|fn fit_left|Col::Kind' crates/tagteam-tui/src/screens/tokens/table.rs`
  - `rg -n 'pub\(crate\) fn (when|line_gauge|fill_role|fit_spans|banner|select|centred_block)\b|pub\(crate\) const (WIDE|RIGHT_INSET)' crates/tagteam-tui/src`
  - `rg -n 'const MARKS|const RATE' crates/tagteam-tui/src/overlay/help.rs`
  - `rg -n 'const FOOTNOTES|fn price_note' crates/tagteam/tests/tokens_cli.rs`; `rg -n 'fn token_report\(s: &Screen\)|fn tokens\(s: &Screen\)' crates/tagteam/tests/tui_pty.rs`
- [ ] **The harness is parked** in `.superpowers/tui-redesign/harness/`; the "before" renders are `gallery/t4-final`.
- [ ] **The gallery check**, after Tasks 5, 8 and 9 and in "After Task 11", as T4's: copy `ux_gallery.rs` in, apply its `lib.rs` line, fix only its own compile errors, render with `UX_GALLERY_DIR=$PWD/.superpowers/tui-redesign/gallery/t5-after-<task> cargo test -p tagteam-tui ux_gallery -- --nocapture`, park it again. Its 18 `tokens-*` scenarios stay; add, from the tasks' fixtures: from Task 5 `tokens-block-active` (one active block with a projection), `tokens-block-two-accounts`, `tokens-per-day` (30 days of costs); from Task 7 `tokens-block-estimated`, `tokens-removed-unknown`, `tokens-drilled-crumbs`; from Task 8 `tokens-models-short` (three families and a colliding pair); `tokens-stale-under-collect` (T5-4). Check by eye at 80×24, 120×40 and 200×50 in tiers 0, 1 and 2 (dark and light) against §8's text and T5-5 to T5-24. A difference is the drawing task's defect. Never quote the renders' real names.

---

### Task 1: Word the token report's lower-bound and price notes anew

**Depends on:** none.

**Files:**
- Modify: `crates/tagteam-engine/src/words.rs` (`Footnote`, `AT_LEAST`, `PRICE_MARK`, `token_footnotes`; its test).
- Modify: `crates/tagteam/src/tokens_cmd.rs` (`footnotes` prints `Footnote::line`).
- Modify: `crates/tagteam/tests/tokens_cli.rs` (`FOOTNOTES`, `price_note`).
- Modify: `src/screens/tokens/table.rs`: `below` pushes `note.line()`; `const AT_LEAST` goes for `words::AT_LEAST`. Mechanical: the test strings that quote the two notes (`a_day_report_at_80_columns`, `a_session_shows_its_project_and_times_and_a_month_its_month`, `the_footnotes_say_what_the_report_leaves_out`, `a_failed_report_keeps_the_last_one_dim_under_the_banner`'s `"API cost"` needle, `DETAIL[6]`, `under_a_full_banner_and_toast_j_and_k_reach_every_value_of_the_detail_and_the_notes`' notes 1 and 4, `at_80x24_a_2000_column_model_leaves_the_table_three_rows_and_the_detail_reachable`'s price value) take the new words.

**Interfaces:**
- Produces:
  ```rust
  // words.rs (T5-2)
  /// A lower bound (token report §2.3): the TUI's cells, and the lead of its note.
  pub const AT_LEAST: &str = "≥";
  /// The lead of the note that says what an API cost is (redesign §8).
  pub const PRICE_MARK: &str = "$";
  /// One of the token report's footnotes (token report §11.2): its mark, for the two notes the
  /// TUI redesign's §8 leads with one, and its words.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Footnote { pub mark: Option<&'static str>, pub text: String }
  impl Footnote { /// `{mark} {text}`, or the text alone. pub fn line(&self) -> String; }
  pub fn token_footnotes(report: &TokenReport, mark: &str, unreadable: bool) -> Vec<Footnote>;
  ```

- [ ] **Step 1: Write the tests.**
  - **`words.rs`, `the_token_footnotes_name_what_the_report_leaves_out_and_mark_partial_costs_as_asked`** (rewritten to the new type). `report()` alone gives `[Footnote { mark: Some("$"), text: "what this would cost on the API at list prices of 2026-10-01; not your bill" }]`. `every` (as today) gives, as `line()`s:
    - `No price for claude-test-9, claude-x: counted without cost (≥ marks a partial cost).`
    - `≥ at least: 1 subagent response is still being counted`
    - `12 transcript lines could not be read, so these totals may be low.`
    - `3 transcript files could not be read; the next refresh tries again.`
    - `$ what this would cost on the API at list prices of 2026-10-01, with config.toml's for claude-opus-5-5; not your bill`

    The marks are `[None, Some("≥"), None, None, Some("$")]`; `*` still marks the partial cost in the first; without `unreadable` there are four.
  - **`words.rs`, `the_lower_bound_s_note_agrees_with_its_count`:** `unfinalized` 3 gives `at least: 3 subagent responses are still being counted`.
  - **`tokens_cli.rs`:** `FOOTNOTES` becomes `"\n"`, `"No price for claude-test-9: counted without cost (* marks a partial cost).\n"`, `"≥ at least: 1 subagent response is still being counted\n"`; `price_note(as_of)` becomes `"$ what this would cost on the API at list prices of {as_of}, with config.toml's for claude-opus-5-5, claude-sonnet-5-5; not your bill\n"`. Its four users and every `--json` assertion stay as they are.
  - **`table.rs`:** in `a_day_report_at_80_columns`, rows 10 and 11 read ` ≥ at least: 2 subagent responses are still being counted` and ` $ what this would cost on the API at list prices of 2026-10-01; not your bill`; the other quoted tests take the same words, wrapped by today's `laid_out`.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-engine --locked --lib words::` and `cargo test -p tagteam --locked --features test-support --test tokens_cli`. Expected: a compile error, then failures.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-engine --locked --lib`; `cargo test -p tagteam --locked --features test-support --test tokens_cli`; `cargo test -p tagteam --locked --features test-support --lib tokens_cmd::`; `cargo test -p tagteam-tui --locked`. Expected: all pass, `--json` unchanged.
- [ ] **Step 5: Commit.** `git add crates/tagteam-engine/src/words.rs crates/tagteam/src/tokens_cmd.rs crates/tagteam/tests/tokens_cli.rs crates/tagteam-tui/src/screens/tokens/table.rs && git commit -m "Word the token report's lower-bound and price notes anew"`

### Task 2: Re-read the token report on the daemon's index events at most every 30 s

**Depends on:** none.

**Files:**
- Modify: `src/screens/tokens/index.rs` (T5-3): `LIVE_REQUERY_MS`, `LIVE_REFRESHING_MS`, `IndexWatch.live_read_ms` (for `live_next_ms`), `live_due_at`, `on_token_index`, `on_tick`, `refresh_now`, doc comments.
- Test: `index.rs`'s test module.

**Interfaces:**
- Produces:
  ```rust
  /// C-13: a `token-index` event reads the report again at most this often ...
  pub const LIVE_REQUERY_MS: i64 = 30_000;
  /// ... and while the TUI's own refresh runs, at the checks' pace.
  pub const LIVE_REFRESHING_MS: i64 = CHECK_MS;
  impl IndexWatch { /// When a held or new event may read the report: `i64::MIN` after `r` or before any. fn live_due_at(&self) -> i64; }
  ```

- [ ] **Step 1: Write the tests.** Every tick of a live test is `live_tick`, which answers each `status` the daemon is asked for (C-R19 drops a link whose `status` does not come; `tick` would lose the link at the 60 s poll). A "read" is a `LoadReport` among a tick's commands, answered by `answer` with that command alone (the other commands, `FetchStatus` and the checks' `LoadIndexState`s, are left out of it; the checks stay unanswered):
  - **`a_catch_up_s_stream_of_events_reads_the_report_every_30_s_and_reads_the_last_change`** (rewrites the 5 s test). `live_on_tokens(with_index_job(None))`; an event each second from `NOW + 1 s` to `NOW + 62 s` (`changedAt` its own time), three ticks a quarter-second apart after each, then ticks every 250 ms to `NOW + 100 s`, each read answered with the latest change. The reads are at `[1_000, 31_000, 61_000, 91_000]` after `NOW`, and the report shows `NOW + 62_000`. No read at `6_000`.
  - **`while_the_tui_s_own_refresh_runs_events_read_the_report_every_5_s`.** Live, `r` at `t0 = NOW + 500` (`RefreshIndex`), then an event each second from `t0 + 1 s` with ticks between. Reads at `t0 +` `1_000`, `6_000`, `11_000`; `Msg::Refreshed` (done) at `t0 + 12_500`; events go on to `t0 + 45 s`, and the next read is at `t0 + 41_000` (30 s after the last), none between.
  - **`r_reads_a_held_token_index_event_at_the_next_tick`** (Review Focus 4). Live; an event at `t` reads; one at `t + 1_000` is held. `r` at `t + 2_000`; `Refreshed` `Busy` at `t + 2_100`. The tick at `t + 2_250` reads it, not the one at `t + 31_000`. A newer event at `t + 3_000` is then held until the tick at `t + 32_250`.
  - **`a_waiting_event_is_forgotten_when_the_link_goes`** and **`…_when_tokens_opens`:** their last tick moves from `at + 5_250` to `at + 30_250`, past the throttle, so each still pins that the held event was forgotten (rewritten, named here).
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-tui --locked screens::tokens::index`. Expected: failures (reads at 5 s).
- [ ] **Step 3: Implement** T5-3.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-tui --locked`. Expected: all pass; `a_token_index_event_newer_than_the_report_on_screen_reads_it_again` unchanged.
- [ ] **Step 5: Commit.** `git add crates/tagteam-tui/src/screens/tokens/index.rs && git commit -m "Re-read the token report on the daemon's index events at most every 30 s"`

### Task 3: Keep a stale token report's banner over a collection's warnings

**Depends on:** none.

**Files:**
- Modify: `src/chrome.rs` (`banner`, T5-4, and its doc comment), `src/screens/tokens/mod.rs` (`TokensState::stale_words`).
- Test: `chrome.rs`.

**Interfaces:**
- Produces: `impl TokensState { /// T2-22's words while the shown report is the last one read. pub(crate) fn stale_words(&self) -> Option<&str>; }`

- [ ] **Step 1: Write the test** `banners_rank_held_keys_crit_a_stale_report_warn_then_the_rest`, rewriting `the_start_up_warnings_come_after_tokens_error_and_before_the_accounts_views` (its fixture: one account in a run shell, two start-up warnings, Tokens with a failed report `the index is locked`):
  - held keys and a Collect banner: `KEYS_HELD`;
  - keys free, the Collect banner on: `couldn't read the token index (the index is locked) · r retries`, warn (it was the Collect banner);
  - `raise_banner(Accounts, "the store is locked")`: that, crit;
  - no state banner: the stale words;
  - on Accounts (`1`) with a Collect banner: `usage was not collected`; with none, the warnings joined; with none of those, the run shell's;
  - on Tokens with a good report, a failed refresh (`Msg::Refreshed` with an error) and a Collect banner: the Collect banner; without it, the refresh's words.
- [ ] **Step 2: Run it to see it fail.** `cargo test -p tagteam-tui --locked chrome::`. Expected: the second case fails.
- [ ] **Step 3: Implement** T5-4.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-tui --locked`. Expected: all pass, `a_failed_read_is_crit_and_anything_else_warn` unchanged.
- [ ] **Step 5: Commit.** `git add crates/tagteam-tui/src && git commit -m "Keep a stale token report's banner over a collection's warnings"`

### Task 4: Lead Tokens with a control bar, its breadcrumbs and a headline

**Depends on:** Task 1 (`words::AT_LEAST`; the notes in `table.rs`'s tests).

**Files:**
- Create: `src/screens/tokens/amounts.rs` (T5-11), `src/screens/tokens/headline.rs` (T5-10's left part); modify `src/screens/tokens/mod.rs` (`mod amounts; mod headline;`; `Range::label(All)` → `all time`).
- Modify: `src/screens/tokens/table.rs`: `header` is rewritten in place as `top` (T5-5) with the bar (T5-6, T5-7, T5-8) and `crumbs` (T5-9); `filter_text` and `grouping_text` take T5-6's words; `clock` goes (`autoline::when` replaces it); `plan`, `scroll_below` and `render` take `top`'s rows; the stale DIM covers the headline.
- Modify: `crates/tagteam/tests/tui_pty.rs`: `token_report` waits for `[ ] 30 days`, `g by account`, `indexed ` and `1.1K`; `tokens` for `[ ] 30 days` and `g by `; the accounts-again predicate for no `[ ] 30 days`.
- Rewritten (named, per §16): in `table.rs`, every test that reads the old header rows: `a_day_report_at_80_columns`, `from_120_columns_the_cache_the_responses_and_a_cost_bar_join_them` (its table now from row 3), `a_session_shows_its_project_and_times_and_a_month_its_month`, `a_block_shows_its_span_and_whether_it_was_observed`, `the_account_and_model_columns_follow_the_grouping`, `the_header_says_how_current_the_index_is`, `a_cold_catch_up_shows_its_progress_whole_at_80_columns_without_saying_catching_up`, `loading_and_an_empty_report_say_so`, the three `an_empty_report_…` tests (their rows from 1, not 3), `the_drill_down_is_named_in_the_header`, `the_header_names_the_top_levels_grouping_at_every_depth`, `one_sessions_models_read_whole_at_80_columns_without_the_period_and_the_project` (head at 4), `the_cache_write_is_one_column_where_its_halves_do_not_fit` (rows +1 at 120), `a_failed_report_keeps_the_last_one_dim_under_the_banner`, `a_long_report_scrolls_to_keep_the_selection_in_view_and_a_resize_keeps_it_there` (row 4 at 100×30), `the_header_keeps_three_rows_and_a_long_filter_is_cut`, `a_long_drill_down_keeps_its_start_on_the_filters_row`, `at_80x24_a_2000_column_model_leaves_the_table_three_rows_and_the_detail_reachable`, `the_worst_live_stack_at_80_by_24_keeps_three_table_rows_and_scrolls_to_a_2000_column_model_line`.

**Interfaces:**
- Consumes: Task 1's `words::AT_LEAST`; `autoline::when`; `chrome::fit_spans`; `keys::RIGHT_INSET`; `words::{scaled, short_name, email}`.
- Produces:
  ```rust
  // amounts.rs
  /// `n` with thousands separators: `9,891` (C-10).
  pub(super) fn grouped(n: u64) -> String;
  /// Nano-dollars as `words::dollars` rounds them, the dollars grouped: `$1,541.64`.
  pub(super) fn dollars(nanos: u64) -> String;
  // headline.rs
  /// The headline's rows at `width` (T5-10): one, its left part, until Task 5.
  pub(super) fn headline(state: &State, report: &TokenReport, width: usize, now_ms: i64) -> Vec<Line<'static>>;
  // table.rs
  /// T5-5's rows above the table: the bar, the crumbs while drilled, the headline and a blank row.
  fn top(state: &State, width: usize, now_ms: i64) -> Vec<Line<'static>>;
  ```

- [ ] **Step 1: Write the tests** (`table.rs` unless named; `days()` at `NOW_MS`, UTC):
  - **`amounts.rs`, `counts_and_dollars_take_thousands_separators`:** `grouped` of 0, 999, 1_000, 9_891, `u64::MAX` is `0`, `999`, `1,000`, `9,891`, `18,446,744,073,709,551,615`; `dollars` of 0, 5_000_000, 622_016_000, 1_541_640_000_000 and `u64::MAX` is `$0.00`, `$0.01`, `$0.62`, `$1,541.64`, `$18,446,744,073.71`.
  - **`the_control_bar_names_each_choice_and_the_index`.** At 120, body row 0 is ` p Day  Month  Session  5h block     [ ] 30 days     g by account     f all accounts · all models`, nine spaces, `indexed 14:12`. At 80 it is ` p Day   [ ] 30 days   g by account   f all`, 23 spaces, `indexed 14:12`. Roles in tier 1: `p`, `[ ]`, `g`, `f` accent; `Day`, `30 days`, `by account` and the filter text; `Month`, `Session`, `5h block` and `indexed 14:12` muted; no cell bold. After `ppp` the 80 row starts ` p 5h block   [ ] 30 days   g by account`. `g` gives `g by model`, then `g by account and model`, then `g no grouping`. `]` twice gives `[ ] all time`.
  - **`the_bar_s_right_part_says_how_current_the_index_is`** (rewrites `the_header_says_how_current_the_index_is`; the right end of body row 0 at 80): after `drawn`, `indexing…`; with progress 1.2 GB of 3.3 GB, `indexing 1.2 / 3.3 GB`; with 900 MB of 3.3 GB, `indexing 900.0 MB / 3.3 GB`; after `Busy`, `catching up elsewhere`; a partial index, `catching up`, and after `requery`, `catching up · updating…`; refreshed at `YESTERDAY + 1 h`, `indexed Sun 01:00`.
  - **`a_cold_catch_up_shows_its_progress_and_never_says_catching_up`** (rewrites the Brussels test): the zone is not shown; progress 12.3 GB of 33.3 GB with a query running reads `indexing 12.3 / 33.3 GB` at 80 and at 120 (form 5 and form 3 of T5-7), and `indexing 12.3 / 33.3 GB · updating…` at 160.
  - **`the_bar_cuts_a_long_filter_and_keeps_its_keys`** (Review Focus 5; rewrites `the_header_keeps_three_rows_and_a_long_filter_is_cut`, same fixture): row 0 at 80 starts ` p Day   [ ] 30 days   g by account   f 2 a-rather-long-ali…`, is at most 79 columns wide and ends in `…` (form 7). Row 1 is the headline, row 2 blank, row 3 starts ` DAY`.
  - **`the_headline_says_the_report_s_cost_tokens_and_responses`.** Body row 1 is ` $50.95 at API prices    114.2M tokens    414 responses` at 80, and starts so at 120 (Task 5 adds to it there); row 2 is blank; row 3 starts ` DAY`. Roles in tier 1: `$50.95` bold text, the labels muted, `114.2M` and `414` text. With the total partial: `≥$50.95`, warn and bold. With no cost: `— at API prices`. With 1,541,640,000,000 nano-dollars and 9,891 responses: ` $1,541.64 at API prices    114.2M tokens    9,891 responses`. One response: `1 response`.
  - **`a_drilled_level_names_its_way_down_on_a_row_under_the_bar`** (rewrites `the_drill_down_is_named_in_the_header` and `a_long_drill_down_keeps_its_start_on_the_filters_row`). `⏎` on row 1, answered with `session("3f2a9c1d", row_account(1))`: row 0 is the bar (still `p Day`), row 1 ` › 2026-09-20 · 1 a1@x.co` (muted), row 2 ` $1.23 at API prices    923.0K tokens    1 response`, row 3 blank, row 4 starts ` SESSION`. A second `⏎`: row 1 ` › 2026-09-20 · 1 a1@x.co › session 3f2a9c1d · 1 a1@x.co`. `widest()`'s drill-down: row 1 is no wider than 79 columns (cut with `…` while its crumb is longer) and row 0 is untouched.
  - **`the_bar_names_the_top_level_s_grouping_at_every_depth`** (rewrites `the_header_names_the_top_levels_grouping_at_every_depth`): `g by model` at depths 0 and 2.
  - **`a_failed_report_keeps_the_last_one_dim_under_the_banner`** (rewritten): besides its needles, `at API prices` is DIM on every cell while stale and plain after; `[ ] 30 days` is plain throughout.
  - **`loading_and_an_empty_report_say_so`** and the three `an_empty_report_…` tests: row 0 the bar; ` loading…` and ` No token usage in this range.` on row 1; the warnings on rows 2–3; the unbuilt block under them, its bar's right part `catching up`.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-tui --locked screens::tokens`. Expected: a compile error, then failures.
- [ ] **Step 3: Implement** T5-5 to T5-11 (the headline's left part).
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-tui --locked`; outside the sandbox `cargo test -p tagteam --locked --features test-support --test tui_pty`. Expected: all pass.
- [ ] **Step 5: Commit.** `git add -A crates/tagteam-tui/src crates/tagteam/tests/tui_pty.rs && git commit -m "Lead Tokens with a control bar, its breadcrumbs and a headline"`

### Task 5: Show today's cost, a cost per day and the active block on Tokens' headline

**Depends on:** Task 4 (`headline`, `amounts`).

**Files:**
- Modify: `src/screens/tokens/headline.rs` (T5-12, T5-13), `src/screens/tokens/amounts.rs` (`about`), `src/gauge.rs` (`progress_line`, `sparkline_of`; the module's doc names Tokens too).
- Rewritten (named): the Block render tests whose rows move down one at 80, the block line taking a row: `a_block_shows_its_span_and_whether_it_was_observed`, `the_active_block_counts_down_between_queries_and_is_no_longer_active_at_its_end`, `an_active_block_s_countdown_changes_when_it_did` (both find the figures under the block's row, which moves up a row when the block ends and its line goes), `the_active_blocks_figures_show_once_under_the_first_of_its_model_rows` (by position, unchanged).
- Test: `headline.rs`, `gauge.rs`, `amounts.rs`.

**Interfaces:**
- Consumes: Task 4's `headline`, `amounts::dollars`; `state.view_of(state.tokens.provider())`'s `active` account.
- Produces:
  ```rust
  // gauge.rs
  /// The block's progress (T5-13): `━` whole cells and a `╸` half in accent on a faint `─` track
  /// (tier 0 `·`), `cells` wide.
  pub(crate) fn progress_line(pct: f64, cells: usize, theme: Theme) -> Vec<Span<'static>>;
  /// T5-12's sparkline: each value on `▁…█` against the largest (`▁` when it is 0), a space for
  /// none, in accent.
  pub(crate) fn sparkline_of(values: &[Option<u64>], theme: Theme) -> Span<'static>;
  // amounts.rs
  /// A projection (T5-13): whole dollars from $10 (`$38`), else dollars and cents.
  pub(super) fn about(nanos: u64) -> String;
  ```

- [ ] **Step 1: Write the tests:**
  - **`gauge.rs`, `the_progress_line_fills_its_share_in_accent_on_a_track`:** 44.44 % of 22 cells is nine `━`, a `╸` and twelve `─`, fill accent and track faint; of 14, six `━` and eight `─`; tier 0 draws `·` for the track. **`a_sparkline_of_values_scales_to_the_largest`:** `[None, Some(0), Some(2), Some(4), Some(8)]` is ` ▁▃▅█`, accent; all zero is `▁` each.
  - **`amounts.rs`, `a_projection_is_whole_dollars_from_ten`:** `about` of $38.12, $9.99, $1,234.50 is `$38`, `$9.99`, `$1,235`.
  - **`from_120_columns_a_day_report_says_today_and_a_sparkline_of_cost_per_day`.** `days()` at 120: body row 1 is ` $50.95 at API prices    114.2M tokens    414 responses    today $0.00`, eleven spaces, `per day `, 27 spaces and `▁█` (2026-09-19 has rows but no price; 2026-09-21, today, has none). `today` and `per day` muted, `$0.00` text, the sparkline accent. At 80 the row ends at `414 responses`. Rows today ($8.00), a day ago ($4.00) and three days ago ($2.00): `today $8.00`, the sparkline ending `▃ ▅█`; with `[` (7 days) it is seven cells, `   ▃ ▅█`. With a second row today that has no price: `today ≥$8.00`, `≥$8.00` in warn.
  - **`today_and_the_per_day_cells_follow_the_report_s_zone`** (Review Focus 3). `TokensState::new(cc(), Pacific/Kiritimati)`, rows dated 2026-09-22 ($3.00) and 2026-09-21 ($5.00): at 120 `today $3.00`, the sparkline ending `█▅`.
  - **`a_block_report_shows_the_active_block_s_progress`.** `showing_after("ppp", vec![block(NOON, true)])`. At 120, body row 1 is ` $5.00 at API prices    5.5M tokens    1 response`, three spaces, `▶ block ━━━━━━━━━╸──────────── 2h46m left · 41.2K tok/min · $3.20/h`, ending one column in. At 80 row 1 is the left part alone and row 2 ` ▶ block ━━━━━━━━━╸──────────── 2h46m left · 41.2K tok/min · $3.20/h`; row 3 blank; row 4 the head. Roles per T5-13.
  - **`the_block_line_counts_down_and_goes_at_the_block_s_end`:** at `NOW_MS + 61_000`, `2h45m left`; at the block's end, no line (at 80, row 2 is blank and the head is on row 3).
  - **`the_block_line_gives_way_on_its_own_row`.** With a projected cost of $38.12: at 120 the line takes row 2 whole, `… · $3.20/h · ≈ $38 at its end`; at 80 it is ` ▶ block ━━━━━━──────── 2h46m left · 41.2K tok/min · $3.20/h · ≈ $38 at its end` (a 14-cell gauge, 79 columns). A partial projection reads `≈ ≥$38`, `≥$38` in warn.
  - **`the_block_line_is_the_account_in_use_s_and_adds_its_model_rows`** (Review Focus 1). Active blocks for account 1 (41,234.6 tokens a minute, $3.20/h, ≈ $20.00) and account 2 (10,000, $1.00/h, ≈ $3.00). With `snapshot(many(2))`, account 2 active: `10.0K tok/min · $1.00/h · ≈ $3.00 at its end`. With no account in use: account 1's. Account 1's block by model, rows of (41,234.6, $3.20/h, ≈ $20.00) and (1,000, $0.80/h, ≈ $5.00): `42.2K tok/min · $4.00/h · ≈ $25 at its end`. With the second model's row unpriced (`cost_per_hour` and `projected_cost` `None`): `42.2K tok/min · ≥$3.20/h · ≈ ≥$20 at its end`, both `≥` amounts in warn.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-tui --locked screens::tokens`, then `cargo test -p tagteam-tui --locked gauge::` (one filter per run). Expected: a compile error.
- [ ] **Step 3: Implement** T5-12 and T5-13.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-tui --locked`. Expected: all pass.
- [ ] **Step 5: Commit.** `git add -A crates/tagteam-tui/src && git commit -m "Show today's cost, a cost per day and the active block on Tokens' headline"`

### Task 6: Draw the Tokens table in roles with plain heads and a share line

**Depends on:** Task 4 (`amounts`, `top`'s rows), Task 5 (`gauge.rs`).

**Files:**
- Modify: `src/screens/tokens/table.rs`: `Col::head` (`API $`, `RESPONSES`, `CACHE W 5M`, `1H`), the cells' roles (T5-15), `CACHE_SPLIT_FROM` (T5-16), the share cell (T5-17), the rule and the total line (T5-18), `cost_text` and the responses cell through `amounts` (T5-11), `below_rows`' fixed rows. `src/gauge.rs` (`share_line`).
- Rewritten (named): `a_day_report_at_80_columns`, `from_120_columns_the_cache_the_responses_and_a_cost_bar_join_them` (as `from_120_columns_the_cache_the_responses_and_a_share_line_join_them`), `the_cache_write_is_one_column_where_its_halves_do_not_fit` (at 160), `every_required_value_shows_whole_at_80_columns_in_the_widest_report` (`API $`, `≥$123,456.79`, the total `≥$123,456.80`), and the tests whose `TOTAL` row now follows the rule: `the_active_block_counts_down_between_queries_and_is_no_longer_active_at_its_end`, `a_long_report_scrolls_to_keep_the_selection_in_view_and_a_resize_keeps_it_there`, `one_sessions_models_read_whole_at_80_columns_without_the_period_and_the_project`, `an_active_block_s_countdown_changes_when_it_did`, and every other test's head strings (`COST` → `API $`).
- Test: `table.rs`, `gauge.rs`.

**Interfaces:**
- Produces:
  ```rust
  // gauge.rs
  /// Tokens' share line (§4.4, T5-17): `━` whole cells and a `╸` half, at least `╸` above 0.
  pub(crate) fn share_line(pct: f64, cells: usize) -> String;
  // table.rs
  /// From this width the cache write splits into its 5-minute and 1-hour halves (§8).
  const CACHE_SPLIT_FROM: usize = 160;
  ```

- [ ] **Step 1: Write the tests:**
  - **`gauge.rs`, `a_share_line_is_its_share_of_eight_cells`:** 100 % is `━━━━━━━━`, 23.67 % `━╸`, 1 % `╸`, 0 % empty.
  - **`a_day_report_at_80_columns`** (rewritten). Body rows 3–8: ` DAY         ACCOUNT        INPUT   OUTPUT   TOTAL   API $`; the three rows as before; ` ` and 57 `─`; ` TOTAL                       1.5M  ≥390.9K  114.2M  $50.95`.
  - **`from_120_columns_the_cache_the_responses_and_a_share_line_join_them`.** Body rows 3–8 at 120:
    - ` DAY         ACCOUNT        INPUT   OUTPUT  CACHE W  CACHE R   TOTAL  RESPONSES   API $`
    - `▌2026-09-20  1 a1@x.co       1.2M  ≥310.0K     4.1M    88.0M   93.6M        412  $41.20  ━━━━━━━━`
    - `             2 spare       300.0K    80.0K        0    20.1M   20.5M          1   $9.75  ━╸`
    - ` 2026-09-19  unattributed    5.0K      900        0   100.0K  105.9K          1       —`
    - ` ` and 96 `─`
    - ` TOTAL                       1.5M  ≥390.9K     4.1M   108.2M  114.2M        414  $50.95`
  - **`the_cache_write_splits_into_5m_and_1h_from_160_columns`.** At 160 the head reads `CACHE W 5M    1H  CACHE R`; at 159 `CACHE W  CACHE R`. With a 70-column alias at 160, where the halves no longer fit, one `CACHE W`, the alias whole.
  - **`thousands_separators_go_on_dollars_and_responses`:** a row of $1,541.64 and 9,891 responses at 120 shows `$1,541.64` and `9,891`, and the total line both; token counts stay scaled (`1.7B`).
  - **`the_table_draws_its_roles_in_every_tier`** (§16), at 80×24 and 120×40 for `TIERS`, by `roles` and `canon`: the head row muted throughout; the unattributed row's period and account text, INPUT and OUTPUT (and at 120 the cache columns and RESPONSES) muted, TOTAL text, its `—` faint; the selected row's `≥310.0K` warn and (at 120) its share line accent; the rule faint; the total line's `TOTAL` bold (`B`), `≥390.9K` warn, `114.2M` and `$50.95` text, `1.5M` muted. Tier 2 dark and light add `sel-bg` across the selected row only.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-tui --locked screens::tokens`, then `cargo test -p tagteam-tui --locked gauge::` (one filter per run). Expected: failures.
- [ ] **Step 3: Implement** T5-15 to T5-18.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-tui --locked`. Expected: all pass, `the_table_is_laid_out_once_per_report_width_and_clock` unchanged.
- [ ] **Step 5: Commit.** `git add -A crates/tagteam-tui/src && git commit -m "Draw the Tokens table in roles with plain heads and a share line"`

### Task 7: Name Tokens' rows plainly: accounts, IDs, projects and estimated blocks

**Depends on:** Task 6 (`table.rs`'s columns and tests).

**Files:**
- Modify: `src/screens/tokens/table.rs`: `account_text`, `account_name` (T5-19); `short_id`; `crumbs` and `detail_rows` (8-character session IDs); the Project cell and `fit` (T5-20, `PROJECT_KEEP`; `fit_left` goes); `Col::Kind` goes and the period cell takes T5-21's mark; the detail's `estimated`. `src/screens/tokens/filter.rs`'s tests: `removed id-9` → `removed account id-9`, `unattributed` → `unknown account` (mechanical, through `account_name`).
- Rewritten (named): `a_day_report_at_80_columns` and `from_120_columns_…` (row 3 `unknown account`, the account column 15 wide), `a_session_shows_its_project_and_times_and_a_month_its_month`, `the_sessions_by_account_and_model_keep_their_models_apart_at_80_columns`, `a_sessions_detail_shows_its_numbers_first_and_every_value_whole` (`DETAIL[1]` `session 3f2a9c1d`), `a_drilled_level_names_its_way_down_on_a_row_under_the_bar`.
- Test: `table.rs`, `filter.rs`.

**Interfaces:**
- Produces: `fn short_id(id: &str) -> String` (first 8 characters); `fn project_name(path: &str) -> &str`; `const PROJECT_KEEP: usize = 20;`.

- [ ] **Step 1: Write the tests:**
  - **`removed_and_unattributed_rows_read_removed_account_and_unknown_account`.** At 80, `days()`'s row 3 is ` 2026-09-19  unknown account    5.0K      900  105.9K       —` under ` DAY         ACCOUNT           INPUT   OUTPUT   TOTAL   API $`. A row of `RowAccount::Removed { id: "3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c" }` reads `removed account`, and selected, its detail `removed account 3f2a9c1d`. The filter dialog lists `removed account id-9` and `unknown account`.
  - **`ids_show_their_first_8_characters_in_the_crumbs_and_the_detail`:** `⏎` on `realistic()`'s session (`showing_after("pp", …)`) gives the crumbs ` › session 3f2a9c1d · 1 a1@x.co`; its detail says `session 3f2a9c1d`, nowhere the whole ID; a CJK ID keeps 8 characters.
  - **`a_session_s_project_is_its_basename_and_keeps_it_before_the_period_shortens`.** `a_session_shows_…` at 80: ` SESSION            PROJECT  ACCOUNT    INPUT  OUTPUT   TOTAL  API $` and `▌09-20 09:12–11:40  tagteam  1 a1@x.co  20.0K    3.0K  923.0K  $1.23`. `showing_after("pp", vec![realistic()])` at 80: `▌09-20 09:12–11:40  portal-feature-user…  1 a1@x.…`: the span keeps its full form, the project its 20 columns, the account what is left. In `widest()` at 80 the span is its start alone and the project is gone, as today. `/w/` is `w`; the detail keeps the full path.
  - **`estimated_blocks_lead_with_a_tilde`** (rewrites `a_block_shows_its_span_and_whether_it_was_observed`). `showing_after("ppp", vec![block(NOON, true), block(NOON − 5 h, false)])` at 80: head ` BLOCK               ACCOUNT     INPUT  OUTPUT  TOTAL   API $`; rows `▌ 09-21 12:00–17:00  1 a1@x.co  400.0K   50.0K   5.5M   $5.00` and ` ~09-21 07:00–12:00  1 a1@x.co  400.0K   50.0K   5.5M   $5.00`; no `observed` or `inferred` in the table. With only observed blocks there is no leading column. Selecting the inferred one, its detail says `estimated`.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-tui --locked screens::tokens`. Expected: failures.
- [ ] **Step 3: Implement** T5-19 to T5-21.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-tui --locked`. Expected: all pass.
- [ ] **Step 5: Commit.** `git add -A crates/tagteam-tui/src && git commit -m "Name Tokens' rows plainly: accounts, IDs, projects and estimated blocks"`

### Task 8: Shorten model IDs in the Tokens table

**Depends on:** Task 7 (`table.rs`'s `fit` and tests).

**Files:**
- Create: `src/screens/tokens/models.rs` (T5-22); modify `src/screens/tokens/mod.rs` (`mod models;`; `Level.models: OnceCell<ShortModels>`, reset by `apply`), `src/screens/tokens/table.rs` (`Table::new` takes the short forms; the Model cell; `crumbs`).
- Rewritten (named): `one_sessions_models_read_whole_at_80_columns_without_the_period_and_the_project`, `the_sessions_by_account_and_model_keep_their_models_apart_at_80_columns`, `a_drilled_level_names_its_way_down_on_a_row_under_the_bar` (its model crumb), `the_account_and_model_columns_follow_the_grouping`, `every_required_value_shows_whole_at_80_columns_in_the_widest_report` (its model `opus-5-5-with-a-very-long-suffix`, the CJK case), `the_active_blocks_figures_show_once_under_the_first_of_its_model_rows`.
- Test: `models.rs`, `table.rs`.

**Interfaces:**
- Produces:
  ```rust
  /// T5-22: the short form of each of a report's models.
  pub(super) struct ShortModels(HashMap<String, String>);
  impl ShortModels {
      pub(super) fn of<'a>(ids: impl IntoIterator<Item = &'a str>) -> ShortModels;
      pub(super) fn of_report(report: &TokenReport) -> ShortModels;
      /// `id`'s short form, or `id` itself.
      pub(super) fn get<'a>(&'a self, id: &'a str) -> &'a str;
  }
  ```

- [ ] **Step 1: Write the tests:**
  - **`models.rs`, `short_ids_drop_a_date_stamp_and_a_shared_lead_and_stay_apart`** (Review Focus 2):
    - `claude-opus-4-1-20250805`, `claude-sonnet-4-5-20250929`, `claude-haiku-4-5-20251001` → `opus-4-1`, `sonnet-4-5`, `haiku-4-5`;
    - `claude-opus-4-1-20250805` alone → `opus-4-1`;
    - `claude-opus-4-1-20250805` and `claude-opus-4-1-20250901` → both full;
    - `claude-opus-4-1-20250805` and `claude-opus-4-1` → both full;
    - `claude-opus-5-5` and `gpt-5` → unchanged (no shared lead);
    - `gpt-4o-2024-08-06` and `gpt-4o-mini` → `4o`, `4o-mini`;
    - `vendor.model-x@20250805` alone → `model-x`; `opus` → `opus`; `claude-` → `claude-`;
    - an ID not in the report: `get` returns it.
  - **`model_ids_are_short_in_the_table_and_full_in_the_detail`.** `work_models()` at depth 2, 80 columns: the cells read `opus-4-5`, `sonnet-4-5`, `haiku-4-5`; the selected row's detail lists `claude-opus-4-5-20251101`; the bar's filter, set to that model, names it in full. A by-model day row drilled into gives crumbs `› 2026-09-20 · opus-5-5` for `claude-opus-5-5` alone.
  - **`the_short_forms_are_found_once_per_report`:** two draws of one report compute them once (the `OnceCell` is set after the first, unchanged after the second); a new report has none until drawn.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-tui --locked screens::tokens`. Expected: a compile error.
- [ ] **Step 3: Implement** T5-22.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-tui --locked`. Expected: all pass, `no_production_line_names_the_provider` included.
- [ ] **Step 5: Commit.** `git add -A crates/tagteam-tui/src && git commit -m "Shorten model IDs in the Tokens table"`

### Task 9: Label the selected row's detail and lead each note with its mark

**Depends on:** Task 1 (`Footnote`'s marks), Task 6 (the rule and total), Task 7 (`account_name`, `short_id`).

**Files:**
- Modify: `src/screens/tokens/table.rs`: `below` (T5-23, T5-24), `detail_rows`, `laid_out` (its parts' style), `window`'s hint (muted); `const SELECTED`, `const DETAIL_INDENT: usize = 11`.
- Rewritten (named, by the rows below the total): `a_day_report_at_80_columns`, `a_session_shows_its_project_and_times_and_a_month_its_month`, `the_footnotes_say_what_the_report_leaves_out`, `a_sessions_detail_shows_its_numbers_first_and_every_value_whole` (`DETAIL` reads as T5-23 lays it out), `under_a_full_banner_and_toast_j_and_k_reach_every_value_of_the_detail_and_the_notes`, `at_80x24_a_2000_column_model_leaves_the_table_three_rows_and_the_detail_reachable`, `the_worst_live_stack_at_80_by_24_keeps_three_table_rows_and_scrolls_to_a_2000_column_model_line`, `j_scrolls_through_a_footnote_longer_than_three_rows_to_its_last_name`, `j_reaches_every_account_and_model_of_a_session_whose_lists_wrap_past_three_rows`, and `scrolled_below` (it reads from the row after the total, the blank one included).
- Test: `table.rs`.

**Interfaces:**
- Consumes: Task 1's `Footnote`, `AT_LEAST`, `PRICE_MARK`; `TokensState::drill_to`.

- [ ] **Step 1: Write the tests:**
  - **`the_selected_row_s_detail_is_labelled_and_says_what_enter_opens`.** `days()` at 80, body rows 9–15:
    - `` (blank)
    - ` SELECTED  2026-09-20 · 1 a1@x.co   ⏎ its sessions`
    - `           412 responses · cache write 4.1M (5m 3.0M, 1h 1.1M)`
    - `           cache read 88.0M`
    - `` (blank)
    - ` ≥ at least: 2 subagent responses are still being counted`
    - ` $ what this would cost on the API at list prices of 2026-10-01; not your bill`

    Roles in tier 1: `SELECTED` muted, the identity text, ` · ` muted, `⏎` accent, ` its sessions` and the numbers muted. At a session's models (depth 2) there is no `⏎`. An unattributed row's identity is `2026-09-19 · unknown account`.
  - **`each_note_is_led_by_its_mark`.** With `the_footnotes_say_what_the_report_leaves_out`'s report at 80×40 (so nothing scrolls), after the detail and its blank row: `   No price for claude-test-9: counted without cost (≥ marks a partial cost).`; ` ≥ at least: 2 subagent responses are still being counted`; `   12 transcript lines could not be read, so these totals may be low.`; `   3 transcript files could not be read; the next refresh tries again.`; ` $ what this would cost on the API at list prices of 2026-10-01, with` then `   config.toml's for claude-opus-5-5; not your bill`. `≥` warn, `$` muted, the words muted.
  - **`the_j_k_hint_is_muted`:** under a full banner and toast at 80×24, ` J K: lines 1–` is muted, not bold.
  - The B25 and B29 tests named above keep their guarantees: the table keeps three rows, nothing below the total is cut, `J` reaches every value.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-tui --locked screens::tokens`. Expected: failures.
- [ ] **Step 3: Implement** T5-23 and T5-24.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-tui --locked`. Expected: all pass.
- [ ] **Step 5: Commit.** `git add -A crates/tagteam-tui/src && git commit -m "Label the selected row's detail and lead each note with its mark"`

### Task 10: Explain Tokens' marks and cache columns in help

**Depends on:** Task 6 (`CACHE W 5M`), Task 7 (`~`).

**Files:**
- Modify: `src/overlay/help.rs` (T5-25: `MARKS`' `≥` and `~`; `CACHE` beside `RATE`, both leads in the one lead column), `src/keys.rs` (the `p` and `[ ]` rows' words).
- Rewritten (named): `every_mark_s_words_start_in_one_column` counts `marks + 2` entries (the rate and the cache halves).
- Test: `help.rs`.

- [ ] **Step 1: Write the tests:**
  - **`help.rs`, `the_marks_explain_at_least_estimated_and_the_cache_write_halves`**, for `TIERS`, at 80 and 120: `≥` in warn with `at least: still being counted, or a cost missing some prices`; `~` in text with `estimated: a 5h block placed by rule, not read from a reset`; the lead `5M 1H`, muted, with `CACHE W 5M and 1H, from 160 columns: cache writes kept 5 minutes and 1 hour; CACHE W is both`.
  - **`every_mark_is_drawn_in_its_role`** and **`every_mark_is_one_column_wide`** pass unchanged (`5M 1H` is not a mark).
  - **`help_on_tokens_shows_each_key_whole_at_80_columns`** (`mod.rs`) passes with `period: day, month, session or 5h block` and `range: 7, 30 or 90 days, or all time`.
- [ ] **Step 2: Run them to see them fail.** `cargo test -p tagteam-tui --locked overlay::help`. Expected: failures.
- [ ] **Step 3: Implement** T5-25.
- [ ] **Step 4: Run the tests.** `cargo test -p tagteam-tui --locked`. Expected: all pass, `every_key_the_table_lists_is_handled` unchanged.
- [ ] **Step 5: Commit.** `git add -A crates/tagteam-tui/src && git commit -m "Explain Tokens' marks and cache columns in help"`

### Task 11: Record the Tokens redesign in the specs and the changelog

**Depends on:** Tasks 1–10.

**Files:**
- Modify: `CHANGELOG.md`, under `## [Unreleased]`, `### Changed`:
  - `` `tagteam tui`'s Tokens screen has one control bar for the period, range, grouping and filter, and a headline with the report's cost, tokens and responses; from 120 columns, today's cost and a sparkline of cost per day; with 5-hour blocks, the active block's progress. Its table says `API $` and `RESPONSES`, short model names, a project's name, `~` for an estimated block, and `removed account` or `unknown account`; the selected row's detail is labelled `SELECTED`. ``
  - `` `tagteam tokens` and `tagteam tui` word two notes anew: `≥ at least: N subagent responses are still being counted` and `$ what this would cost on the API at list prices of <date>; not your bill`. ``
  - `` `tagteam tui` re-reads the token report on the daemon's index events at most every 30 s, every 5 s while its own refresh runs, and at once after `r`. A report it could not read again keeps its banner over a collection's warnings. ``
- Modify: `docs/superpowers/specs/2026-10-14-tagteam-tui-design.md`, each hunk ending *(Amended by the TUI redesign spec §8.)* unless named:
  - §5.3's mock-up gives way to §8's layout, in example names;
  - "Range": `all time`; "Time zone": the system zone, which the screen no longer names;
  - "Columns": at 80, period, account, input, output, total and `API $`; from 120, cache write, cache read, responses and a share line; from 160 the cache write's `5M` and `1H`; the model column in short IDs; a session's project by its name;
  - "Rows that need a mark": `removed account`, `unknown account`, `≥`, `~` for an estimated block, `—`;
  - "The total line and footnotes": the faint rule above it; the two notes' words, each note led by its mark; the selected row's detail under `SELECTED`;
  - "Drill-down": the breadcrumbs' row; "Blocks": the headline's progress line beside the row's figures;
  - "Indexing": `indexing 1.2 / 3.3 GB`, `catching up`, `catching up elsewhere` and `indexed 14:12` on the bar's right;
  - §8's `token-index` bullet: at most every 30 s, every 5 s while the TUI's own refresh runs, and at once after `r` *(Amended by the TUI redesign spec §14, C-13.)*.
- Modify: `docs/superpowers/specs/2026-10-14-tagteam-token-report-design.md` §11.2's footnotes bullet: the unfinalized responses' note reads `≥ at least: N subagent responses are still being counted`, and the price table's `$ what this would cost on the API at list prices of <date>; not your bill` *(Amended by the TUI redesign spec §8.)*.
- Modify: `docs/superpowers/specs/2026-10-20-tagteam-tui-redesign-design.md` §18: a bullet `**Token report spec:** §11.2 (the two notes' words).` (T5-26). Nothing else in it changes here.

- [ ] **Step 1: Edit** the files as listed.
- [ ] **Step 2: Check the format.** `cargo fmt --all --check`. Expected: clean.
- [ ] **Step 3: Commit.** `git add CHANGELOG.md docs/superpowers/specs && git commit -m "Record the Tokens redesign in the specs and the changelog"`

---

## After Task 11 (controller)

- [ ] **The gallery check** into `gallery/t5-final`, every Tokens scenario at 80×24, 120×40 and 200×50 in all tiers, against §8 and T5-5 to T5-24.
- [ ] **No dependency changed:** `git diff <base> -- Cargo.toml Cargo.lock 'crates/*/Cargo.toml'` prints nothing.
- [ ] **No store, protocol or `--json` change:** `git diff <base> -- crates/tagteam-engine/src/store crates/tagteam-engine/src/tokens crates/tagteam-engine/src/views.rs crates/tagteam-daemon` prints nothing; `git diff <base> -- crates/tagteam/src/tokens_cmd.rs` touches `footnotes` alone; `tokens_cli.rs`'s diff is `FOOTNOTES` and `price_note` alone.
- [ ] **No provider name in neutral source:** `git diff <base> -- crates/tagteam-tui/src crates/tagteam-engine/src | rg '^\+' | rg -i 'claude|anthropic|tagteam_cc'` hits test modules or `testutil.rs` only.
- [ ] **No commit over the slice size,** or it was split for review.
- [ ] **CI in full,** totals as passed/total; the pty suites outside the sandbox; the first frame (`cargo test --release -p tagteam --features test-support --test perf -- --ignored tui_first_frame --nocapture`, every p95 within 50 ms); Linux in Docker (non-root with a passwd entry, HOME outside `/tmp`, the `docker` call unpiped).
- [ ] **Cross-review:** `xreview round --reset`; a pre-merge review of `origin/main...tui-redesign-t5`, in slices under the inline cap: Tasks 1–3, 4, 5, 6, 7–8, 9–11, then fix commits; a commit over the cap is split by file into review-only commits. Fix and re-review until every slice approves.
- [ ] **Push and hand over.** Push `tui-redesign-t5`, run CI on it (`gh workflow run CI --ref tui-redesign-t5`), and send the coordinator the ready message: head, base, CI, Codex coverage, the macOS and Linux totals. The coordinator opens the Draft PR against `main` with the body written from `.github/pull_request_template.md`, its "Rulings" listing the planned calls, T5-1 to T5-27 and every ruling made while executing.
- [ ] **The status commit** (T5-27). Once the PR exists, one commit: this plan's `**Status:**` to `Implemented — <PR URL>`; the redesign spec's `**Status:**` from `In progress — https://github.com/michaelrigart/tagteam/pull/40` to `Implemented — https://github.com/michaelrigart/tagteam/pull/28, https://github.com/michaelrigart/tagteam/pull/29, https://github.com/michaelrigart/tagteam/pull/35, https://github.com/michaelrigart/tagteam/pull/40, <T5 PR URL>`. Give it its own Codex slice, push it, and send the coordinator the new head and the slice's nonce.

## Self-review

- **Coverage.** §15's T5 row: §8 in Tasks 4–9 (the bar 4, the headline 4–5, the table 6–8, the detail and notes 9), the footnote words in 1, PERF-D-2 in 2. VIS-4 (Tokens) is 6–8's widths (the cache split from 160, the project before the period, short models); VIS-9 is 4–6 (headline, separators); VIS-14 is 5; VIS-15 is 4 and 7. §9.2's legend is 10. §16: render tests per tier at 80×24 and 120×40 (4, 6, 9), the state machine of the throttle (2) and banners (3). §18 is 11; §15's status is "After". The carries: T1/T2/T3's "Out of" lines (PERF-D-2, Tokens' §8 layout), T2-14 (3), PR #35's muted hint (9), PR #28's R-5 cache kept (T5-14). VIS-10's Tokens half was built in T2 (T2-22) and is kept.
- **Steps.** Each code step names its file, its signature and the spec's words; no body is written out. The rules the tests leave open (the bar's forms, the index words, the headline's fitting, the block's choice, the short IDs, the project's fit, the SELECTED layout) are stated once, in T5-3 to T5-24.
- **Types.** `Footnote`, `AT_LEAST`, `PRICE_MARK` (1), `LIVE_REFRESHING_MS`, `live_read_ms` (2), `stale_words` (3), `amounts::{grouped, dollars}` and `headline` (4), `progress_line`, `sparkline_of`, `about` (5), `share_line`, `CACHE_SPLIT_FROM` (6), `short_id`, `project_name`, `PROJECT_KEEP` (7), `ShortModels` (8), `SELECTED`, `DETAIL_INDENT` (9) keep their names across tasks.
- **Proportion.** About 510 lines, against T4's 760, for a smaller slice: one screen's drawing, one engine function's words and two small behaviour edits. Code blocks are signatures only; the long parts are the rulings and the exact rows the tests read.
