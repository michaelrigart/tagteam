# Accounts in one format, a full list, and pre-tracking token rows: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Accounts shows each account once, in one format, with a full-list view on `v`; Tokens
labels unattributed usage from before tracking as `before tracking`.

**Architecture:** The engine adds `TokenReport::tracking_since_ms` and a span test on
`PeriodKey`; the TUI's Tokens table picks its unattributed words per row. On Accounts, one
window-row builder in `card.rs` serves the selected card and the full list's blocks; the lead
card goes and the chrome's Now strip takes its place; a new `screens/accounts/list.rs` draws the
full list, scrolled by block.

**Tech Stack:** Rust (workspace), ratatui, crossterm, jiff; tests with ratatui's `TestBackend`
through `crates/tagteam-tui/src/testutil.rs`.

**Spec:** `docs/superpowers/specs/2026-10-26-tagteam-accounts-views-design.md` (cited "S§n").
Redesign spec `docs/superpowers/specs/2026-10-20-tagteam-tui-redesign-design.md` ("redesign
§n") stays the authority for everything S does not change. Line numbers are as of `25a968c`.

## Global Constraints

- 80×24 is the first target; everything works there (S§1.4, redesign §1.4).
- No meaning by colour alone: every state has a glyph, a word or a shape (S§1.4).
- 50 ms p95 first frame (S§1.4).
- No real names in tests, fixtures or images: `example.com`, `example.org`, `corp.example`,
  `personal.example`, as `testutil.rs` uses (S§1.4).
- One vocabulary: words come from `tagteam_engine::words` or the TUI's existing constants (S§1.4).
- `tagteam list`, `tagteam tokens` and every `--json` output are unchanged (S§1.3).
- Exact words: `before tracking`, `unknown account`, `SELECTED`, `IN USE`, `not in use`,
  `{NN}% expected by now`, `v full list`, `v compact`, `api key · has no usage limits to show`
  (S§3.4, §4.3, §7, §8).
- Thresholds: bars 16 cells below `chrome::WIDE` (120), 26 from it; table gauges unchanged (9/14);
  STATUS from 100 columns (S§3.3, §4.1).
- Commit messages follow upstream's style: one plain imperative sentence, no `feat:` prefix
  (`git log` on `main`).
- Checks before every commit: `cargo fmt --all --check`; the three clippy runs CONTRIBUTING
  names; `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`.

## Review Focus

1. **The full list at the smallest terminal** (80×24 with a full banner and a toast): the
   selected block's title stays visible and nothing panics on the short body. Test in Task 9.
2. **`v` before the first snapshot, or with no accounts:** the loading or empty state shows as in
   compact, and `v` back works. Test in Task 8.
3. **A refresh that removes accounts while the full list is scrolled down:** the first block shown
   clamps so the selection is whole. Test in Task 8.
4. **A resize from 120×40 to 80×24 in the full list:** the selected block stays whole at once.
   Test in Task 9.
5. **A day before the epoch that is 23 or 25 hours long (DST):** its span uses the zone's
   real bounds, so the day of the epoch never reads `before tracking`. Test in Task 1.

---

### Task 1: Engine — the epoch on the report, and the span test

**Files:**
- Modify: `crates/tagteam-engine/src/tokens/view/mod.rs` (`TokenReport` :81, `PeriodKey` :114,
  the report's construction :381, the epoch read :406)
- Modify: the other `TokenReport` literals: `crates/tagteam-engine/src/words.rs:1646`,
  `crates/tagteam/src/tokens_cmd.rs:1008`, `crates/tagteam-tui/src/testutil.rs:556`
- Test: `crates/tagteam-engine/src/tokens/view/mod.rs` (its `tests` module) and
  `crates/tagteam-engine/tests/tokens_view.rs`

**Interfaces:**
- Produces: `TokenReport::tracking_since_ms: Option<i64>`;
  `PeriodKey::before_tracking(&self, epoch_ms: Option<i64>, tz: &TimeZone) -> bool`.

- [ ] **Step 1: Write the failing tests** in `view/mod.rs`'s tests:

```rust
#[test]
fn a_period_is_before_tracking_only_when_its_whole_span_ends_by_the_epoch() {
    let tz = TimeZone::get("Europe/Brussels").unwrap();
    let (start_23, _) = bucket::day_bounds_ms(date(2026, 10, 23), &tz);
    let epoch = start_23 + 9 * 3_600_000; // 09:00 on the 23rd
    let day = |d| PeriodKey::Day { date: d };
    assert!(day(date(2026, 10, 22)).before_tracking(Some(epoch), &tz));
    assert!(!day(date(2026, 10, 23)).before_tracking(Some(epoch), &tz));
    assert!(!day(date(2026, 10, 22)).before_tracking(None, &tz));
    let month = |m| PeriodKey::Month { year: 2026, month: m };
    assert!(month(9).before_tracking(Some(epoch), &tz));
    assert!(!month(10).before_tracking(Some(epoch), &tz));
    let session = |last_ms| PeriodKey::Session {
        session_id: "s".into(), project: None, first_ms: start_23, last_ms,
    };
    assert!(session(epoch - 1).before_tracking(Some(epoch), &tz));
    assert!(!session(epoch).before_tracking(Some(epoch), &tz)); // a response at the epoch
    let block = |end_ms| PeriodKey::Block {
        start_ms: end_ms - 5 * 3_600_000, end_ms, observed: true, active: None,
    };
    assert!(block(epoch).before_tracking(Some(epoch), &tz));
    assert!(!block(epoch + 1).before_tracking(Some(epoch), &tz));
}

#[test]
fn a_dst_day_before_the_epoch_ends_at_its_zone_s_midnight() {
    // 2026-10-25 is 25 hours long in Europe/Brussels.
    let tz = TimeZone::get("Europe/Brussels").unwrap();
    let (_, end) = bucket::day_bounds_ms(date(2026, 10, 25), &tz);
    let d = PeriodKey::Day { date: date(2026, 10, 25) };
    assert!(d.before_tracking(Some(end), &tz));
    assert!(!d.before_tracking(Some(end - 1), &tz));
}
```

And in `tests/tokens_view.rs`: `the_report_carries_the_epoch_it_read` — a store with a token
epoch at `E` gives `report.tracking_since_ms == Some(E)`; a store with none gives `None`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-engine before_tracking dst_day carries_the_epoch`
Expected: compile errors, `no method named before_tracking`, `no field tracking_since_ms`.

- [ ] **Step 3: Implement.** Add `pub tracking_since_ms: Option<i64>` to `TokenReport` with the
doc `/// The token index's epoch (TR §7.2), \`None\` when the store has none. Not in the JSON.`
Set it from `epoch.map(|e| e.at)` at :406 (initialise it `None` in the literal at :381; the
report returned early at :398 keeps `None` only if the epoch is read after it, so move the
`store.token_epoch()?` read above the early return and set the field there). Add
`PeriodKey::before_tracking` per S§7.1's table: Day and Month through
`tagteam_core::tokens::bucket::{day_bounds_ms, month_bounds_ms}`, `end <= epoch`; Session
`last_ms < epoch`; Block `end_ms <= epoch`; `None` epoch → `false`. Add
`tracking_since_ms: None` to the three other literals.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p tagteam-engine` and `cargo test -p tagteam tokens_cmd`
Expected: PASS; the `tokens --json` golden tests unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/tagteam-engine crates/tagteam/src/tokens_cmd.rs crates/tagteam-tui/src/testutil.rs
git commit -m "Expose the token index's epoch on the token report"
```

### Task 2: Tokens — `before tracking`

**Files:**
- Modify: `crates/tagteam-tui/src/screens/tokens/table.rs` (`account_text` :252,
  `account_name` :267, the cell :638, the breadcrumb :1035, the detail :1268)
- Modify: `crates/tagteam-tui/src/screens/tokens/filter.rs:155`
- Modify: `crates/tagteam-tui/src/overlay/help.rs` (`RATE`/`CACHE` pairs :66-80, their use
  :141-144)
- Test: `crates/tagteam-tui/src/screens/tokens/table.rs`, `filter.rs`, `overlay/help.rs`

**Interfaces:**
- Consumes: `PeriodKey::before_tracking`, `TokenReport::tracking_since_ms` (Task 1).
- Produces: `fn account_text(account: &RowAccount, before: bool) -> String`;
  `pub fn account_name(account: &RowAccount, before: bool) -> String`;
  `const BEFORE_TRACKING: &str = "before tracking"`.

- [ ] **Step 1: Write the failing tests** in `table.rs`'s tests, a report grouped by account
with `tracking_since_ms = Some(day(1) start)` (use `testutil::day`, `token_row`,
`token_report`), unattributed rows on `day(3)`, `day(1)` and `day(0)`, and an attributed row
on `day(3)`:

```rust
#[test]
fn unattributed_rows_wholly_before_the_epoch_read_before_tracking() {
    let rows = draw(&state, 80, 24);
    assert!(row_of(&rows, &period(day(3))).contains("before tracking"));
    assert!(row_of(&rows, &period(day(1))).contains("unknown account"));  // holds the epoch
    assert!(row_of(&rows, &period(day(0))).contains("unknown account"));
    assert!(row_of(&rows, "1 work").contains(&period(day(3))));           // attributed keeps its name
    // Both words are 15 columns: the next column starts at the same place.
    assert_eq!(col(row_of(&rows, &period(day(3))), "5.0K"), col(row_of(&rows, &period(day(1))), "1.2K"));
}

#[test]
fn with_no_epoch_every_unattributed_row_reads_unknown_account() { /* tracking_since_ms None */ }

#[test]
fn the_detail_and_a_breadcrumb_use_their_row_s_words() {
    // Select the day(3) unattributed row: the SELECTED detail says `before tracking`;
    // ⏎ into its sessions: the breadcrumb says `before tracking`.
}
// `row_of` and `col` as the Tokens tests' helpers, or `accounts.rs`' `col` moved to testutil.
```

In `filter.rs`'s tests: `the_filter_keeps_unknown_account_for_every_unattributed_row` — with the
same report, the dialog's choice reads `unknown account` and the control bar's summary for that
choice reads `unknown account`. In `help.rs`'s tests: help on Tokens lists
`before tracking` with words naming the filter's `unknown account`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-tui before_tracking unknown_account breadcrumb`
Expected: FAIL; the day(3) row reads `unknown account`.

- [ ] **Step 3: Implement.** `account_text`/`account_name` take `before`; `Unattributed` gives
`BEFORE_TRACKING` when `before`, else `unknown account`. At each row site pass
`row.period.before_tracking(report.tracking_since_ms, &report.query.tz)` (for a breadcrumb, the
parent row's own period). `filter.rs:155` passes `false`. Help gains a third pair beside
`RATE` and `CACHE`:
`("before tracking", "usage from before tagteam recorded which account was in use; the filter's unknown account includes it")`
(S§7.2; the final wording is the builder's within that meaning, S§12).

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p tagteam-tui tokens`
Expected: PASS, every existing Tokens test unchanged.

- [ ] **Step 5: Commit** — `Name unattributed usage from before tracking on Tokens`

### Task 3: No pace marks; `▲` on the Now strip

**Files:**
- Modify: `crates/tagteam-tui/src/gauge.rs` (`pace_cell` :33 goes; `line_gauge` :56, `gauge`
  :79, `lead_bar` :106)
- Modify: callers `screens/accounts/table.rs:442`, `screens/accounts/lead.rs:278`,
  `chrome.rs:239` (`in_use_parts`)
- Test: `gauge.rs` tests :234, :257, :317; `screens/accounts.rs` :780, :4229, :4273;
  `chrome.rs` tests

**Interfaces:**
- Produces: `line_gauge(pct: f64, cells: usize, theme: Theme) -> Vec<Span<'static>>`;
  `lead_bar(pct: f64, cells: usize, theme: Theme) -> Vec<Span<'static>>`.

- [ ] **Step 1: Write the failing tests.** Reverse `gauge.rs:234` into
`a_line_gauge_draws_no_pace_mark` and `:257` into `a_lead_bar_draws_no_pace_mark`: for every
tier in `TIERS`, `line_gauge(54.0, 9, theme)` and `lead_bar(53.0, 52, theme)` contain none of
`┿ ┼ │`. Reverse `accounts.rs:780` (`the_table_draws_no_pace_mark`). In `chrome.rs`:

```rust
#[test]
fn the_now_strip_marks_an_ahead_window_after_its_percentage() {
    // Tokens and History, work in use with 7d ahead (`read(60, 53.0, 6.0, true)`):
    for screen in [Screen::Tokens, Screen::History] {
        for w in [80, 120] {
            let strip = now_strip_text(screen, w);
            assert!(strip.contains("7d") && strip.contains("6% ▲"), "{strip}");
            assert!(!strip.contains('┿') && !strip.contains('┼'));
        }
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-tui pace_mark now_strip_marks`
Expected: FAIL (compile error on the new arity first).

- [ ] **Step 3: Implement.** Drop `expected` from `line_gauge`, `gauge` and `lead_bar`; remove
`pace_cell`; `progress_line` calls `gauge(pct, Role::Accent, cells, theme)`. Callers stop
computing `expected`. In `in_use_parts`, after `card::value_cell(w, theme)`, push
`Span::raw(" ")` and `Span::styled("▲", theme.role(Role::Warn))` when `p.ahead == Some(true)`.
Delete the lead tests that only pinned the mark (:4229, :4273); the lead card goes in Task 6.

- [ ] **Step 4: Run them to verify they pass** — `cargo test -p tagteam-tui`, PASS.

- [ ] **Step 5: Commit** — `Drop pace marks from the bars and mark ahead windows on the Now strip`

### Task 4: One window row

**Files:**
- Modify: `crates/tagteam-engine/src/words.rs:333` (`ahead_words`)
- Modify: `crates/tagteam-tui/src/screens/accounts/card.rs` (`Row` :~430-500, `window_rows`,
  `label_width` :340, `value_cell` :543, the sparkline constants)
- Modify: `crates/tagteam-tui/src/gauge.rs` (`sparkline` :127 and its tests go: its only caller
  was the card)
- Test: `card.rs` tests; `words.rs` tests

**Interfaces:**
- Produces, in `card.rs`:

```rust
/// What every window row on a screen shares (S§4.1).
pub(super) struct RowLayout {
    pub(super) indent: usize,   // 2 in the card, 7 in the full list
    pub(super) label_w: usize,  // label_width over every window on the screen, plus two
    pub(super) value_w: usize,  // 4, or the widest spend value on the screen
    pub(super) width: u16,
}
pub(super) fn row_layout<'a>(windows: impl IntoIterator<Item = &'a Window>, indent: usize, width: u16) -> RowLayout;
pub(super) fn window_line(w: &Window, p: &Pace, at: &RowLayout, thick: bool, now_s: i64, theme: Theme) -> Line<'static>;
```

- In `words.rs`: `pub fn expected_words(expected_pct: f64) -> String` → `"26% expected by now"`;
  `ahead_words` builds its parenthesis from it.

- [ ] **Step 1: Write the failing tests** in `card.rs`:

```rust
#[test]
fn a_window_row_has_its_parts_at_fixed_columns() {
    let at = RowLayout { indent: 2, label_w: 7, value_w: 4, width: 80 };
    let long = window("7d", "7d", WindowKind::Long, 82.0, Some(5 * 86_400 + 4 * 3_600));
    let ahead = Pace { ahead: Some(true), expected_pct: Some(26.0), rate_per_hour: Some(2.3), ..Pace::default() };
    let text = line_text(window_line(&long, &ahead, &at, true, NOW_S, Theme::new(false)));
    assert_eq!(&text[..9], "  7d     ");
    // indent 2 + label 7 + 16 cells + 1, then ` 82%` right-aligned in 4
    assert_eq!(col(&text, "82%"), 27);
    assert_eq!(col(&text, "▲"), 31);
    assert_eq!(col(&text, "resets in 5d04h"), 34);
    assert!(!text.contains("expected by now"));         // the row is full at 80
    let wide = RowLayout { width: 120, ..at };
    let text = line_text(window_line(&long, &ahead, &wide, true, NOW_S, Theme::new(false)));
    assert!(text.ends_with(" · 26% expected by now"), "{text}");
    assert_eq!(col(&text, "82%"), 37);   // 26-cell bar from 120
}

#[test]
fn thick_bars_mark_the_account_in_use_and_thin_gauges_the_rest() {
    // thick: only ▆/▁ in the bar; thin: only ━/╸/─ (tier 0: · track).
}

#[test]
fn a_spend_value_widens_the_value_column_for_every_row() {
    // windows 5h (44%) and a Spend `€12.40 of €50`: value_w is 13; `44%` right-aligned in it,
    // and both rows' resets start at the same column.
}

#[test]
fn the_label_column_is_capped_at_a_quarter_of_the_width() {
    // A 40-character label at 80 columns: label_w is 20 + 2, the label cut with `…`,
    // the percentage still on the row (as accounts.rs:1980 today).
}
```

In `words.rs`: `expected_words(26.4) == "26% expected by now"` and
`ahead_words(Some(26.4)) == "▲ faster than an even pace (26% expected by now)"` (unchanged).

- [ ] **Step 2: Run them to verify they fail** — `cargo test -p tagteam-tui window_row thick spend_value label_column`; `cargo test -p tagteam-engine expected_words`. FAIL.

- [ ] **Step 3: Implement.** `window_line` follows S§4.1's template:
`{indent}{label:<label_w}{bar} {value:>value_w} {▲ or space}  {reset_tail}` with the bar
`gauge::lead_bar` when `thick`, else `gauge::line_gauge`, 16 or 26 cells by `chrome::WIDE`;
`reset_tail` as today; then ` · {expected_words}` when `p.ahead == Some(true)`,
`p.expected_pct` is `Some`, the window kind has pace, and the whole row fits `width`; else the
row through `fit_spans`. `▲` never expands in place any more (S§4.3). Remove `Row`, the
sparkline constants, `SHORT_SINCE_S`, and `gauge::sparkline` with its tests :290, :317's
sparkline half.

- [ ] **Step 4: Run them to verify they pass** — `cargo test -p tagteam-tui -p tagteam-engine`.
`window_rows` now calls `window_line` (indent 4 until Task 5 moves the card to its new layout),
so the card tests that pinned sparklines (`accounts.rs:1198`, `:2123`) fail only on those
assertions: delete those assertions in this task; Task 5 rewrites the rest of those tests.

- [ ] **Step 5: Commit** — `Draw every window row in one format`

### Task 5: The selected card in one format

**Files:**
- Modify: `crates/tagteam-tui/src/screens/accounts/card.rs` (`lines` :75, `title` :~205,
  `facts_of` :353, `keys_row` :~560, `min_height`/`height`, `is_on_lead`, `own_problem_rows`)
- Modify: `crates/tagteam-tui/src/screens/accounts.rs` (`render` :486-488 stops passing
  `on_lead`)
- Test: `screens/accounts.rs` tests :1198, :1277, :1306, :1334, :1406, :1441, :1455, :1711,
  :1770, :1877, :3450, :3975

**Interfaces:**
- Consumes: `RowLayout`, `row_layout`, `window_line` (Task 4).
- Produces: `pub(crate) fn lines(state: &State, v: &AccountView, width: u16, rows: usize, from: usize, keys: bool, now_ms: i64) -> Vec<Line<'static>>`
  (no `on_lead`; `keys` false leaves the keys row out, S§5.4 step 2; Task 6 drives it); `pub(super) fn facts_row(state: &State, v: &AccountView, width: u16, indent: usize, now_ms: i64) -> Line<'static>`;
  `pub(super) fn no_windows_row(v: &AccountView, indent: usize, width: u16, now_s: i64, theme: Theme) -> Option<Line<'static>>`;
  `pub(super) fn problem_rows(...)` as today. The full list (Task 7) reuses all three.

- [ ] **Step 1: Write the failing tests** (rewrite the listed ones; new names):

```rust
#[test]
fn the_card_reads_selected_then_facts_windows_and_keys() {
    // three(), 80×24, account 1 selected:
    let (y, rows) = card_rows(&state, 80, 24);
    assert!(rows[0].starts_with("SELECTED  1 alex@example.com"));
    assert_eq!(col(&rows[1], "not in use"), 10);
    assert!(rows[2].starts_with("  5h     "));
    assert_eq!(col(&rows.last().unwrap(), "s switch"), 10);
    assert!(rows.last().unwrap().contains("s switch   ⏎ history   e disable   a alias   x remove"));
}

#[test]
fn the_in_use_card_shows_every_reset_projection_and_problem() {
    // Reverses :1277 and :3975 (M-9): work in use, quarantined: its rows carry `resets in`,
    // and its problem sentence and remedy are under its windows; the facts row says `IN USE`.
}

#[test]
fn the_in_use_card_hides_s_but_s_still_switches() {
    // Keys row has no `s switch`; pressing `s` still returns the switch command (actions.rs:179).
}

#[test]
fn an_account_without_windows_shows_the_table_s_words_or_its_problem() {
    // ApiKey: one row `  api key · has no usage limits to show`, muted;
    // ReloginRequired: its problem rows and no extra row.
}

#[test]
fn next_update_ends_the_facts_from_120_columns_only() {}

#[test]
fn a_login_under_seven_days_is_warn_and_says_to_log_in_again() {}   // ports :3450 to the card
```

- [ ] **Step 2: Run them to verify they fail** — `cargo test -p tagteam-tui card`. FAIL.

- [ ] **Step 3: Implement.** Title: `SELECTED` muted, two spaces, `{position} `, `name_spans`.
Facts on their own row from column 10 (`facts_of` with `IN USE` in accent for the account in
use, `next update` moved last and only from `WIDE`, and the lead card's soon-to-expire rule
moved here: under `SOON_S` warn plus `log in again before then`). Window rows through
`window_line` with `indent: 2`, `thick: v.active`. No windows: `problem_rows` when
`words::problem(v)` is `Some`, else `no_windows_row` from `status::unread_words`. Keys row from
column 10 at every width: `s switch` (left out when `v.active`), `⏎ history`, `e disable` /
`e enable`, `a alias`, `x remove`, `KEYS_GAP` apart. Remove `is_on_lead`,
`own_problem_rows`, `on_lead`. `min_height` = title + facts + one window (or the no-windows
row) + problem rows; `height` adds the other windows and the keys row. Move `SOON_S` and
`LOG_IN_AGAIN` from `lead.rs` to `card.rs`.

- [ ] **Step 4: Run them to verify they pass** — `cargo test -p tagteam-tui`. PASS (lead-card
tests still pass: the lead card is still drawn until Task 6).

- [ ] **Step 5: Commit** — `Show the selected account in one format, its keys and problems included`

### Task 6: Compact layout — the lead card goes, the Now strip comes

**Files:**
- Modify: `crates/tagteam-tui/src/view.rs` (`areas` :45-84: the strip on every screen; no chrome
  rule on Accounts)
- Modify: `crates/tagteam-tui/src/chrome.rs` (`in_use_parts`: no `SEE_ACCOUNTS` on Accounts)
- Modify: `crates/tagteam-tui/src/screens/accounts.rs` (`Parts` :257, `parts` :269, `render`
  :425; `LEAD_LEAST`, `BLANK`; module doc)
- Modify: `crates/tagteam-tui/src/screens/accounts/lead.rs` (keep `in_use`, `pick_words`,
  `PICK_GAP`, `LEFT_LEAST`; delete `Lead`, `lead`, `height`, `no_account`, `with_pick`, `title`,
  `facts_of`, `bar_rows`, `fold_line`; module doc says what is left)
- Modify: `crates/tagteam-tui/src/screens/accounts/card.rs` (`lead_keys` goes)
- Test: `chrome.rs:1147`, `chrome.rs:1352`; `accounts.rs` :3236-4415 (the lead-card tests),
  :3739, :3767, :3839

**Interfaces:**
- Produces: `struct Parts { top: Option<Rect>, table: Rect, under: Option<Rect>, card: Rect }`
  (the two rule rows, each with its blanks, or `None` when dropped); `const RULE_ROWS: u16 = 3`.

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn compact_at_80_by_24_matches_the_spec_s_row_counts() {
    // three() plus kim (4 accounts × 3 windows as S§3.1), work selected:
    let rows = draw(&state, 80, 24);
    assert!(rows[2].starts_with(" ● 2 work   5h"));         // the Now strip, row 3
    assert_eq!(rows[3], "");
    assert!(rows[4].chars().all(|c| c == '─'));
    assert_eq!(rows[5], "");
    assert!(rows[6].trim_start().starts_with("#   ACCOUNT"));
    assert_eq!(rows[11], ""); assert!(rows[12].chars().all(|c| c == '─')); assert_eq!(rows[13], "");
    assert!(rows[14].starts_with("SELECTED  2 work"));
    assert_eq!(rows.iter().filter(|r| r.starts_with("SELECTED")).count(), 1);
    // 5H, 7D and UPDATED at 36, 54 and 73 (S§3.3).
}

#[test]
fn compact_at_120_by_40_matches_the_spec() { /* 39, 62, 86; org on the title; 26-cell bars */ }

#[test]
fn a_short_body_drops_blanks_then_the_keys_row_then_the_rules() {
    // Eight accounts at 80×24: the lower blanks go first, then the upper; with more, the card's
    // keys row; then the rules; problem rows stay throughout (S§5.4).
}

#[test]
fn on_accounts_a_problem_in_use_does_not_say_see_accounts() {}
```

Reverse `chrome.rs:1147` (Accounts now has the Now strip) and `:1352` (the rows above the body:
Accounts has header, auto line, Now strip and no rule at any height; Tokens and History keep
the rule from 30 rows). Delete the lead-card tests; port each one whose subject survives:
best pick and forecast words (:3338, :3640, :4336) to the Now strip on Accounts; the
several-providers ones (:3587, :3691, :4388) to the Now strip following focus.

- [ ] **Step 2: Run them to verify they fail** — `cargo test -p tagteam-tui compact_at chrome`. FAIL.

- [ ] **Step 3: Implement.** In `areas`, `strip` is every screen and `rule` is
`state.screen != Screen::Accounts && height >= RULE_FROM`. In `in_use_parts`, skip
`SEE_ACCOUNTS` when `state.screen == Screen::Accounts` (pass the screen in). `parts` lays out
blank/rule/blank, the table, blank/rule/blank, the card; it reserves the table's
`min(lines, TABLE_LEAST)` and the card's `min_height` first, then gives back in S§5.4's order:
the lower blanks, the upper blanks, the keys row (`card::lines`' `keys`), the rules.
`render` draws `chrome::rule` in each rule row.

- [ ] **Step 4: Run them to verify they pass** — `cargo test -p tagteam-tui`. PASS.

- [ ] **Step 5: Commit** — `Replace the lead card with the Now strip on Accounts`

### Task 7: Drop the card's history read

**Files:**
- Modify: `crates/tagteam-tui/src/screens/accounts.rs` (`AccountsState` fields `card`,
  `card_for`, `card_due_ms`; `schedule_card` :88; `on_tick` :189; `on_card` :206; the calls in
  `handle_key` :250)
- Modify: `crates/tagteam-tui/src/state.rs:293` (`load_card`), `update.rs:68` (`Msg::Card`),
  `lanes.rs:224`, `msg.rs` (`Cmd::LoadCard`, `Msg::Card`), the `card` generation in `gens`;
  `card.rs` `CARD_DEBOUNCE_MS`, `CARD_SINCE_S`
- Test: `accounts.rs` :1153 (`with_card`), :1482, :1535 go

- [ ] **Step 1: Check nothing else reads it:** `rg -n "load_card|LoadCard|Msg::Card|gens.card|on_card|schedule_card" crates`
Expected: only the sites above (S§12 lets the builder drop it if nothing else needs it).
- [ ] **Step 2: Remove them and the three tests;** `on_tick` goes if it then does nothing
(remove its caller in `update.rs`).
- [ ] **Step 3: Run** `cargo test -p tagteam-tui` and the clippy runs. PASS, no dead code.
- [ ] **Step 4: Commit** — `Drop the card's history read, which only fed its sparklines`

### Task 8: `v` and the full list

**Files:**
- Create: `crates/tagteam-tui/src/screens/accounts/list.rs`
- Modify: `crates/tagteam-tui/src/screens/accounts.rs` (`AccountsState`: `view`, `list_from`;
  `handle_key`; `render`)
- Modify: `crates/tagteam-tui/src/keys.rs` (`KEYS` :105: a `v` row between `P` :386 and `r`
  :398; `s`'s label :170; the probe test :753)
- Test: `list.rs` tests; `keys.rs` tests; `accounts.rs` tests

**Interfaces:**
- Consumes: `card::{facts_row, no_windows_row, problem_rows, row_layout, window_line}`.
- Produces:

```rust
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum View { #[default] Compact, List }
// AccountsState gains `view: View` and `list_from: usize` (the first block shown).
// list.rs:
pub(super) struct Block { pub(super) lines: Vec<Line<'static>>, pub(super) id: AccountId }
pub(super) fn blocks(state: &State, width: u16, now_ms: i64) -> Vec<Block>;
pub(super) fn render(state: &State, frame: &mut Frame, body: Rect, now_ms: i64);
pub(super) fn keep_whole(heights: &[usize], from: usize, at: usize, rows: usize) -> usize;
```

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn v_toggles_the_full_list_and_keeps_the_selection() {
    // three(), account 1 selected: `v` → View::List, selected unchanged; footer `v compact`;
    // `v` → Compact, footer `v full list`.
}

#[test]
fn the_full_list_at_80_by_24_matches_the_spec() {
    // S§6.1's mock: rows 4–6 blank/rule/blank; rows 7–11 account 1's block with `▌` in column 0
    // on each, none elsewhere; dotted rule `   ┈…┈` ending 3 columns in; block 2 thick bars and
    // `IN USE`; block 3's title on the last body row; facts from column 7; no keys row.
}

#[test]
fn moving_past_the_last_whole_block_scrolls_one_block() {
    // j from block 2 to 3: list_from becomes 1; the first body row after the pinned three is
    // block 2's title. k back to 1: list_from 0.
}

#[test]
fn keep_whole_moves_as_little_as_needed() {
    assert_eq!(keep_whole(&[5, 5, 5, 5], 0, 2, 17), 1);
    assert_eq!(keep_whole(&[5, 5, 5, 5], 2, 0, 17), 0);
    assert_eq!(keep_whole(&[5, 5, 5, 5], 0, 1, 17), 0);
    assert_eq!(keep_whole(&[20], 0, 0, 17), 0);     // taller than the viewport: alone
}

#[test]
fn v_before_the_first_read_or_with_no_accounts_shows_the_same_state() {}   // Review Focus 2

#[test]
fn a_refresh_that_removes_accounts_clamps_the_list() {}                      // Review Focus 3

#[test]
fn s_has_no_footer_label_for_the_account_in_use() {
    // compact and list: footer lacks `s switch` with work selected, has it with alex selected.
}
```

In `keys.rs` the table test gains `v`'s probe; the footer test asserts at 80 columns
`s switch  b best  n next  ⏎ history  P pause auto  v full list` and no `r refresh`, and with the
account in use selected `b best  n next  ⏎ history  P pause auto  v full list  r refresh`.

- [ ] **Step 2: Run them to verify they fail** — `cargo test -p tagteam-tui full_list v_toggles keep_whole`. FAIL.

- [ ] **Step 3: Implement.** The `v` row: `screen: Some(Screen::Accounts)`, `keys: "v"`,
`does: "full list, or back to compact"`, `Footer::Show { key: "v", label: |s| Some(if s.accounts.view == View::List { "compact" } else { "full list" }), from: 0 }`,
probe `plain('v')`. `s`'s label: `state.accounts.selected_view().filter(|v| !v.active).map(|_| "switch")`.
`handle_key` handles `v` (toggle, then `keep_whole`), and in `View::List` maps `j/k/↑/↓/Home/End`
to block steps and `PgUp/PgDn` to the whole blocks shown (at least one). A block: title
(`▌` when selected, `{position:>2} `, `●` or space, name bold when selected, org from `WIDE`),
`facts_row` at indent 7, window rows via `window_line` at indent 7 and `thick: v.active` (or
the no-windows row, or problem rows), problems; a disabled block DIM; the selected block's every
row starts `▌`. `row_layout` over every window of every account, once per frame. Blocks are
separated by blank, `   ┈…┈   ` faint (tier 0 `·`), blank. The viewport is the body less the
pinned blank/rule/blank; blocks from `list_from` while they fit, then the next one cut.
`keep_whole` follows `view::scrolled_windows`' shape: whole blocks, at least one.

- [ ] **Step 4: Run them to verify they pass** — `cargo test -p tagteam-tui`. PASS.

- [ ] **Step 5: Commit** — `List every account in full on v`

### Task 9: The full list's edges

**Files:**
- Modify: `crates/tagteam-tui/src/screens/accounts/list.rs`, `accounts.rs` (`handle_key` `J`/`K`)
- Modify: `crates/tagteam-tui/src/keys.rs` (`J K` row :411: `does: "scroll the selected card or block"`)
- Test: `list.rs`, `accounts.rs`

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn a_provider_heading_starts_its_first_block_s_scroll_unit() {
    // Two providers: the heading is the first row of each provider's first block; scrolled to
    // provider 2's first block the list starts with its heading; to its second, with its title.
}

#[test]
fn with_one_account_its_block_is_followed_by_the_add_a_second_hint() {
    // `Add a second account to switch: log in to Claude Code as it, then press A.` after one
    // blank row under the block (S§6.1); compact keeps it under the table.
}

#[test]
fn a_block_taller_than_the_viewport_shows_alone_and_scrolls_its_windows() {
    // `scoped(24)` on the selected account at 80×24: its title, facts and problem rows stay;
    // J/K move its windows; the `J K: windows a–b of n` hint as the card has it.
}

#[test]
fn at_80_by_24_under_a_full_banner_and_toast_the_selected_title_shows() {}   // Review Focus 1

#[test]
fn a_resize_keeps_the_selected_block_whole() {}                              // Review Focus 4
```

- [ ] **Step 2: Run them to verify they fail.** FAIL.
- [ ] **Step 3: Implement.** The heading (muted ` {display_name}`) is pushed onto the first
block of each provider when there are several; the one-account hint is pushed, after a blank,
onto the sole block of focus's provider (`one_account`). A selected block taller than the
viewport draws through the card's `Shown` logic with `card_from`, shared with compact's `J`/`K`
(`scroll_card` takes the rows the view gives the selected account).
- [ ] **Step 4: Run** `cargo test -p tagteam-tui`. PASS.
- [ ] **Step 5: Commit** — `Scroll the full list's tall blocks and place its headings and hint`

### Task 10: Changelog, help, and the run

**Files:**
- Modify: `CHANGELOG.md` (`## [Unreleased]` :8)
- Modify: `docs/superpowers/specs/2026-10-26-tagteam-accounts-views-design.md` (`**Status:**
  Draft` → `Implemented`)
- Test: `overlay/help.rs` tests

- [ ] **Step 1:** Help test: on Accounts, help lists `v` (`full list, or back to compact`) and
still lists `s` and `r`; the marks legend keeps `▲ faster than an even pace`. Run, FAIL if the
row's words differ, fix the words.
- [ ] **Step 2:** `CHANGELOG.md` under `[Unreleased]`, in the file's existing style:
`### Changed`: Accounts shows the account in use in the Now strip and every account's windows in
one format; bars have no pace marks, `▲` adds the expected share in words where it fits;
`### Added`: `v` lists every account in full; `### Fixed`: Tokens names unattributed usage from
before tracking `before tracking` instead of `unknown account`.
- [ ] **Step 3: Run the full checks**

Run: `cargo fmt --all --check`, the three clippy runs, and
`cargo test --workspace --locked --no-fail-fast --features tagteam/test-support` (the pty tests
from a real terminal).
Expected: all pass; report the counts.

- [ ] **Step 4: Run the built TUI against a real store** at 80×24 and 120×40, both views, and
take screenshots (no real names visible, S§1.4: use a fixture store or crop).
- [ ] **Step 5: Commit** — `Record the accounts views in the changelog and mark the spec implemented`
