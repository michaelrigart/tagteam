# Accounts in one format, a full list, and pre-tracking token rows: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Accounts shows each account once, in one format, with a full-list view on `v`; Tokens
labels unattributed usage from before tracking as `before tracking`.

**Architecture:** The engine adds `TokenReport::tracking_since_ms` and a span test on
`PeriodKey`; the TUI's Tokens table picks its unattributed words per row. On Accounts the lead
card goes and the chrome's Now strip takes its place; one window-row builder in `card.rs` serves
the selected card and the full list's blocks; a new `screens/accounts/list.rs` draws the full
list, scrolled by block.

**Tech Stack:** Rust (workspace), ratatui, crossterm, jiff; tests with ratatui's `TestBackend`
through `crates/tagteam-tui/src/testutil.rs`.

**Spec:** `docs/superpowers/specs/2026-10-26-tagteam-accounts-views-design.md` (cited "S§n").
The redesign spec `docs/superpowers/specs/2026-10-20-tagteam-tui-redesign-design.md` ("redesign
§n") stays the authority for everything S does not change. Line numbers are as of `25a968c`.

## Global Constraints

- 80×24 is the first target; everything works there (S§1.4, redesign §1.4).
- No meaning by colour alone: every state has a glyph, a word or a shape (S§1.4).
- 50 ms p95 first frame (S§1.4).
- No real names in tests, fixtures or images: the `example.com`, `x.co`, `corp.example` names
  the fixtures already use (S§1.4; CONTRIBUTING: no tokens, emails or account IDs).
- One vocabulary: words come from `tagteam_engine::words` or the TUI's existing constants (S§1.4).
- `tagteam list`, `tagteam tokens` and every `--json` output are unchanged (S§1.3).
- Exact words: `before tracking`, `unknown account`, `SELECTED`, `IN USE`, `not in use`,
  `{NN}% expected by now`, `v full list`, `v compact`, `api key · has no usage limits to show`.
- Thresholds: window bars 16 cells below `chrome::WIDE` (120), 26 from it; table gauges unchanged
  (9/14 cells); STATUS from 100 columns; percentages bold from 70 (S§3.3, §4.1).
- Pace (`▲`, its words) only on long and scoped windows (C-8, `lead.rs:265-268`).
- Commit messages follow upstream's style: one plain imperative sentence, no `feat:` prefix.
- Every commit passes: `cargo fmt --all --check`; the clippy runs CONTRIBUTING names, with
  `-D warnings` (so no dead code); `cargo test --workspace --locked --no-fail-fast --features
  tagteam/test-support`. A task changes every existing test its behaviour breaks, in that task.
- Test commands take one filter each: run `cargo test -p <crate> <filter>` once per filter, or the
  whole package.

## Review Focus

1. **The full list at the smallest terminal** (80×24 under a full banner and a toast): the
   selected block's title stays visible and nothing panics on the short body. Task 7.
2. **`v` before the first snapshot, or with no accounts:** the loading or empty state shows as in
   compact, and `v` back works. Task 6.
3. **A refresh that removes accounts while the full list is scrolled down:** the first block shown
   clamps so the selection is whole. Task 6.
4. **A resize from 120×40 to 80×24 in the full list:** the selected block is whole in the first
   frame after the resize, before any key. Task 7.
5. **A day before the epoch that is 23 or 25 hours long (DST):** its span uses the zone's real
   bounds, so the epoch's own day never reads `before tracking`. Task 1.

---

### Task 1: Engine — the epoch on the report, and the span test

**Files:**
- Modify: `crates/tagteam-engine/src/tokens/view/mod.rs` (`TokenReport` :81, `PeriodKey` :117,
  the report literal :381, the early return :398, the epoch read :406; tests module :471)
- Modify: the other `TokenReport` literals: `crates/tagteam-engine/src/words.rs:1646`,
  `crates/tagteam/src/tokens_cmd.rs:1008`, `crates/tagteam-tui/src/testutil.rs:556`
- Test: `view/mod.rs` tests; `crates/tagteam-engine/tests/tokens_view.rs`

**Interfaces:**
- Produces: `TokenReport::tracking_since_ms: Option<i64>`;
  `PeriodKey::before_tracking(&self, epoch_ms: Option<i64>, tz: &TimeZone) -> bool`.

- [ ] **Step 1: Write the failing tests** in `view/mod.rs`'s tests (add `use jiff::civil::date;`;
`day_bounds_ms` and `month_bounds_ms` are already in scope through `super::*`, imported from
`tagteam_core::tokens` at :14):

```rust
#[test]
fn a_period_is_before_tracking_only_when_its_whole_span_ends_by_the_epoch() {
    let tz = TimeZone::get("Europe/Brussels").unwrap();
    let (start_23, _) = day_bounds_ms(date(2026, 10, 23), &tz);
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
    let (start, end) = day_bounds_ms(date(2026, 10, 25), &tz);
    assert_eq!(end - start, 25 * 3_600_000);
    let d = PeriodKey::Day { date: date(2026, 10, 25) };
    assert!(d.before_tracking(Some(end), &tz));
    assert!(!d.before_tracking(Some(end - 1), &tz));
}
```

In `tests/tokens_view.rs`, `the_report_carries_the_epoch_it_read`: with the file's existing
fixture, a store whose token epoch is `E` gives `report.tracking_since_ms == Some(E)`, also for a
query whose `accounts` is `Some(vec![])` (the early return); a store with no epoch gives `None`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p tagteam-engine before_tracking`, then `cargo test -p tagteam-engine carries_the_epoch`
Expected: compile errors: no method `before_tracking`, no field `tracking_since_ms`.

- [ ] **Step 3: Implement.** Add `pub tracking_since_ms: Option<i64>` to `TokenReport`
(`/// The token index's epoch (TR §7.2), \`None\` when the store has none. Not in the JSON.`).
Move `let epoch = store.token_epoch()?;` above the early return at :398 and set
`report.tracking_since_ms = epoch.map(|e| e.at)` there, so both returns carry it. Add
`PeriodKey::before_tracking` per S§7.1: Day through `day_bounds_ms`, Month through
`month_bounds_ms`, `end <= epoch`; Session `last_ms < epoch`; Block `end_ms <= epoch`; a `None`
epoch is `false`. Add `tracking_since_ms: None` to the three other literals.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p tagteam-engine`, then `cargo test -p tagteam`
Expected: PASS; the `tokens --json` tests unchanged (the JSON is built field by field,
`tokens_cmd.rs:323`).

- [ ] **Step 5: Commit** — `Expose the token index's epoch on the token report`

### Task 2: Tokens — `before tracking`

**Files:**
- Modify: `crates/tagteam-tui/src/screens/tokens/table.rs` (`account_text` :252, `account_name`
  :267, the cell :638-643, the breadcrumb parents :1035, `detail_rows` :1212 and its calls
  :1268-1270, its caller in `below` :1320)
- Modify: `crates/tagteam-tui/src/screens/tokens/filter.rs:155`
- Modify: `crates/tagteam-tui/src/overlay/help.rs` (`RATE` :66, `CACHE` :74, their use :141-144)
- Test: `table.rs` tests (`showing` :1596, `days()`), `filter.rs` tests, `help.rs` tests

**Interfaces:**
- Consumes: `PeriodKey::before_tracking`, `TokenReport::tracking_since_ms` (Task 1).
- Produces: `fn account_text(account: &RowAccount, before: bool) -> String`;
  `pub fn account_name(account: &RowAccount, before: bool) -> String`;
  `detail_rows(row, tz, before: bool, now_ms, width, enter, theme)`;
  `const BEFORE_TRACKING: &str = "before tracking"` in `table.rs`.

- [ ] **Step 1: Write the failing tests** in `table.rs`'s tests. `days()` has an unattributed
row on 2026-09-19 (`a_day_report_at_80_columns`, :1721); `YESTERDAY` is 2026-09-20T00:00Z.

```rust
#[test]
fn an_unattributed_day_wholly_before_the_epoch_reads_before_tracking() {
    let state = showing(days(), |r| r.tracking_since_ms = Some(YESTERDAY));
    let lines = body_of(&state, 80, 24, NOW_MS);
    assert!(lines.contains(&" 2026-09-19  before tracking    5.0K      900  105.9K       —".to_owned()));
    // The attributed rows keep their names.
    assert!(lines.iter().any(|l| l.starts_with("▌2026-09-20  1 a1@x.co")));
}

#[test]
fn a_day_holding_the_epoch_or_no_epoch_reads_unknown_account() {
    for epoch in [Some(YESTERDAY - 1), None] {
        let state = showing(days(), |r| r.tracking_since_ms = epoch);
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert!(lines.contains(&" 2026-09-19  unknown account    5.0K      900  105.9K       —".to_owned()), "{epoch:?}");
    }
}

#[test]
fn months_sessions_and_blocks_follow_their_own_spans() {
    // One unattributed row per period kind, each ending one ms before the epoch, and one each
    // ending at or after it (`p`, `s`, `b` set the period as `showing_after` does):
    // a month 2026-08 (before) and 2026-09 (holds it); a session with last_ms = E-1 (before) and
    // last_ms = E (unknown); a block ending at E (before) and E+1 (unknown).
    // Assert each row's ACCOUNT cell reads its word.
}

#[test]
fn an_ungrouped_row_names_before_tracking_in_its_account_list() {
    // Grouping none (`g` until `none`): a 2026-09-19 row whose `accounts` is
    // [Unattributed] lists `before tracking`; with `accounts` [row_account(1), Unattributed]
    // it lists `1 a1@x.co, before tracking`.
}

#[test]
fn the_detail_and_a_breadcrumb_use_their_row_s_words() {
    let mut state = showing(days(), |r| r.tracking_since_ms = Some(YESTERDAY));
    // Select the 2026-09-19 row (End).
    update(&mut state, key(KeyCode::End), NOW_MS);
    let lines = body_of(&state, 80, 24, NOW_MS);
    assert!(lines.iter().any(|l| l.starts_with(" SELECTED  2026-09-19 · before tracking")));
    // ⏎ into its sessions: the breadcrumb row names `before tracking`.
}
```

In `filter.rs`: `the_filter_keeps_unknown_account_for_every_unattributed_row` — the same report
with `tracking_since_ms = Some(YESTERDAY)`: open the filter (`f`); its account choice reads
`unknown account`; choose only it; the control bar's summary reads `unknown account`. In
`help.rs`: on Tokens, help lists `before tracking` and its words name the filter's `unknown
account`.

- [ ] **Step 2: Run to verify they fail** — `cargo test -p tagteam-tui before_tracking`, then
`cargo test -p tagteam-tui unknown_account`, `cargo test -p tagteam-tui breadcrumb`. FAIL.

- [ ] **Step 3: Implement.** `account_text`/`account_name` take `before`; `Unattributed` gives
`BEFORE_TRACKING` when `before`, else `unknown account`. The cell and the breadcrumb compute
`row.period.before_tracking(report.tracking_since_ms, &report.query.tz)` for their own row (a
breadcrumb for its parent row). `below` computes it for the selected row and passes it to
`detail_rows`, which uses it for both `row.account` and the `row.accounts` list. `filter.rs:155`
and the filter summary pass `false`. Help gains a third pair beside `RATE` and `CACHE`:
`("before tracking", "usage from before tagteam recorded which account was in use; the filter's unknown account includes it")`.
Update every direct caller of `account_text`, `account_name` and `detail_rows` in tests.

- [ ] **Step 4: Run to verify they pass** — `cargo test -p tagteam-tui`. PASS.

- [ ] **Step 5: Commit** — `Name unattributed usage from before tracking on Tokens`

### Task 3: No pace marks; `▲` on the Now strip

**Files:**
- Modify: `crates/tagteam-tui/src/gauge.rs` (`pace_cell` :33 goes; `line_gauge` :56,
  `progress_line` :66, `gauge` :79, `lead_bar` :106)
- Modify: callers `screens/accounts/table.rs:442`, `screens/accounts/lead.rs:265-278`,
  `chrome.rs:233-239` (`in_use_parts`)
- Test: `gauge.rs` tests :234, :257, :317; `screens/accounts.rs` tests :780, :4229, :4273;
  `chrome.rs` tests

**Interfaces:**
- Produces: `line_gauge(pct: f64, cells: usize, theme: Theme) -> Vec<Span<'static>>`;
  `lead_bar(pct: f64, cells: usize, theme: Theme) -> Vec<Span<'static>>`.

- [ ] **Step 1: Write the failing tests.** Rewrite `gauge.rs:234` as
`a_line_gauge_draws_no_pace_mark` and `:257` as `a_lead_bar_draws_no_pace_mark`: for every tier
in `TIERS`, `line_gauge(54.0, 9, theme)` draws `━━━━╸────` (tier 0 track `·`) and
`lead_bar(53.0, 52, theme)` 28 `▆` then 24 `▁`, neither containing `┿`, `┼` or `│`. `:317`
drops its `expected` argument. Rewrite `accounts.rs:780` as `the_table_draws_no_pace_mark`
(no `┿`/`┼` in any table row of `three()` with 7d ahead). Delete `accounts.rs:4229` (it only
pinned the mark); keep `:4273`'s `▲` in warn assertion as it is (the lead card still draws `▲`
until Task 4). In `chrome.rs`'s tests, define `fn state_on(screen: Screen) -> State`: `fresh(screen)`
then `state.accounts.apply(snapshot(…))` (as `accounts.rs:521` `loaded` does), the snapshot
holding `account(2, "sam@corp.example", read(60, 53.0, 6.0, true))` with alias `work`, in use.
Then:

```rust
#[test]
fn the_now_strip_marks_an_ahead_window_after_its_percentage() {
    // Tokens and History, `work` in use with 7d ahead: `read(60, 53.0, 6.0, true)`.
    for screen in [Screen::Tokens, Screen::History] {
        for w in [80, 120] {
            let rows = draw(&state_on(screen), w, 30);
            let strip = &rows[2];
            assert!(strip.contains("  6% ▲ "), "{strip}");
            assert!(!strip.contains('┿') && !strip.contains('┼'), "{strip}");
        }
    }
}
```

and `locate(&buf, "▲")` on that row is in `Role::Warn` in every tier (use `roles`).

- [ ] **Step 2: Run to verify they fail** — `cargo test -p tagteam-tui pace_mark`, then
`cargo test -p tagteam-tui now_strip_marks`. FAIL (first on the new arity).

- [ ] **Step 3: Implement.** Drop `expected` from `line_gauge`, `gauge` and `lead_bar`; remove
`pace_cell`; `progress_line` calls `gauge(pct, Role::Accent, cells, theme)`. Callers stop
computing `expected`. In `in_use_parts`, after `card::value_cell(w, theme)`, push `Span::raw(" ")`
and `Span::styled("▲", theme.role(Role::Warn))` when `p.ahead == Some(true)` and the window is
long or scoped.

- [ ] **Step 4: Run to verify they pass** — `cargo test -p tagteam-tui`. PASS.

- [ ] **Step 5: Commit** — `Drop pace marks from the bars and mark ahead windows on the Now strip`

### Task 4: The lead card goes; the Now strip and the rules come

**Files:**
- Modify: `crates/tagteam-tui/src/view.rs` (`areas` :45-84)
- Modify: `crates/tagteam-tui/src/chrome.rs` (`now_strip` :151, `in_use_parts` :203: no
  `SEE_ACCOUNTS` on Accounts)
- Modify: `crates/tagteam-tui/src/screens/accounts.rs` (module doc; `BLANK`, `LEAD_LEAST`;
  `Parts` :277, `parts` :289, `render` :401-487)
- Modify: `crates/tagteam-tui/src/screens/accounts/lead.rs` (keep `in_use`, `pick_words`,
  `PICK_GAP`, `LEFT_LEAST`, `NO_ACCOUNT`'s users if any remain; delete `Lead` (with its
  `shown`), `lead`, `height`, `no_account`, `with_pick`, `title`, `facts_of`, `bar_rows`,
  `fold_line`, `BAR_WIDE`, `BAR`, `TITLE_INDENT`, `BOLD_FROM`; move `SOON_S` and `LOG_IN_AGAIN`
  to `card.rs` for Task 5; the module doc says what is left)
- Modify: `crates/tagteam-tui/src/screens/accounts/card.rs` (`lead_keys` :148, `is_on_lead`
  :57, `own_problem_rows` :62 and `lines`' `on_lead` parameter go: with no lead card the in-use
  card shows its reset, projection and problems, M-9 no longer applies)
- Test: `chrome.rs` :1038, :1147, :1352, :1378; `accounts.rs` :1277, :1306, :3739, :3767, :3975,
  and the lead-card tests :3236-4415

**Interfaces:**
- Produces: `card::lines(state, v, width, rows, from, now_ms)` (no `on_lead`);
  `struct Parts { top: Rect, table: Rect, under: Rect, card: Rect }`, where `top` and `under` are
  the blank/rule/blank rows (height 3, 2 when its blanks are dropped to one, 1 for the rule
  alone, 0 when dropped).

- [ ] **Step 1: Write the failing tests.**

```rust
/// `three()` plus `kim@example.org` at 4 (S§3.1's mock): `work` in use and selected.
fn four() -> Vec<AccountView> {
    let mut v = three();
    v.push(account(4, "kim@example.org", read(300, 99.0, 13.0, false)));
    v
}

#[test]
fn compact_at_80_by_24_has_no_lead_card_and_rules_between_blank_rows() {
    let state = loaded(four());
    let rows = draw(&state, 80, 24);
    assert!(rows[2].starts_with(" ● 2 work   5h"));                      // the Now strip
    assert_eq!(rows[3], "");
    assert!(rows[4].chars().all(|c| c == '─') && rows[4].chars().count() == 80);
    assert_eq!(rows[5], "");
    assert!(rows[6].starts_with("    #   ACCOUNT"));
    assert_eq!(rows[11], "");
    assert!(rows[12].chars().all(|c| c == '─'));
    assert_eq!(rows[13], "");
    assert!(rows[14].starts_with("    2 work"));                          // the card, old title
    let header = &rows[6];
    assert_eq!((col(header, "5H"), col(header, "7D"), col(header, "UPDATED")), (36, 54, 73));
}

#[test]
fn compact_at_120_by_40_puts_the_columns_where_the_spec_does() {
    let rows = draw(&loaded(four()), 120, 40);
    let header = &rows[6];
    assert_eq!((col(header, "5H"), col(header, "7D"), col(header, "UPDATED")), (39, 62, 86));
    assert!(header.contains("STATUS"));
}

#[test]
fn a_short_body_drops_the_blank_rows_then_the_rules() {
    // `many(8)` at 80×24 (S§5.4): the body (20 rows) cannot hold blank/rule/blank twice, the
    // table's least and the card: the lower pair of blanks goes first (rows[13] is the card's
    // first row right under the lower rule), then the upper pair, then the rules; the table
    // keeps `min(lines, TABLE_LEAST)` and the card its least.
}

#[test]
fn on_accounts_a_problem_in_use_does_not_say_see_accounts() {
    // `quarantined_in_use(three())` (accounts.rs:3413): the strip on Accounts reads
    // `● 2 work   ✕ needs a new login` and does not contain `see Accounts`; on Tokens it does.
}

#[test]
fn the_in_use_card_shows_its_reset_projection_and_problem() {
    // Replaces :1277, :1306 and :3975 (M-9): `work` in use and selected: every window row has
    // `resets in`; quarantined, its problem sentence and remedy are under its windows.
}
```

Update the chrome tests: `:1038` (`the_rows_above_the_body_are_the_same_in_both_modes`) —
Accounts' body now starts at row 3 at 80×24 and 120×40 (no rule on Accounts), Tokens/History
unchanged; `:1147` — Accounts has the Now strip; `:1352` — Accounts: header, auto line, Now
strip, no rule at any height; Tokens and History keep the rule from 30 rows; `:1378`
(`from_30_rows_a_faint_rule_sits_under_the_chrome`) — runs on Tokens and History, and asserts
Accounts has no chrome rule at row 3 (its body's own rule is at row 4). Rewrite `:3739` and
`:3767` for the new stack (strip, blank/rule/blank, table, blank/rule/blank, card). Delete the
lead-card tests whose subject goes (:3245-3265 helpers, :3265, :3425, :3515, :3559, :3839, :3937,
:4016, :4203, :4229, :4273, :4289, :4361); port to the Now strip on Accounts those whose subject
stays: the best pick and forecast words (:3338, :3640, :4336) and focus's provider (:3587,
:3691, :4388). Move `:3450`'s login-expiry subject to Task 5.

- [ ] **Step 2: Run to verify they fail** — `cargo test -p tagteam-tui compact_at`, then
`cargo test -p tagteam-tui chrome`. FAIL.

- [ ] **Step 3: Implement.** In `areas`: `strip` is true on every screen; `rule` is
`state.screen != Screen::Accounts && height >= RULE_FROM`. `in_use_parts` gets the screen and
skips `SEE_ACCOUNTS` on Accounts. `parts` lays out `top` (blank, rule, blank), the table, `under`
(blank, rule, blank), the card; it reserves the table's `min(lines, TABLE_LEAST)` and
`card::min_height` first, then takes back rows in S§5.4's order: `under`'s blanks, `top`'s
blanks, the two rules. `render` draws `chrome::rule(state, width)` on each rule row. The card
keeps its current format in this task.

- [ ] **Step 4: Run to verify they pass** — `cargo test -p tagteam-tui`. PASS; clippy clean
(no field of `Lead` survives).

- [ ] **Step 5: Commit** — `Replace the lead card with the Now strip on Accounts`

### Task 5: One window row, the card in one format, and no history read

**Files:**
- Modify: `crates/tagteam-engine/src/words.rs:333` (`ahead_words`)
- Modify: `crates/tagteam-tui/src/screens/accounts/card.rs` (all of `lines` :75, `title` :238,
  `facts_of` :353, `window_rows`, `Row` and `reset_tail` :~430-520, `value_cell` :543,
  `keys_row`, `min_height`, `height`, `furthest`; the sparkline constants, `CARD_DEBOUNCE_MS`,
  `CARD_SINCE_S`, `SHORT_SINCE_S`)
- Modify: `crates/tagteam-tui/src/gauge.rs` (`sparkline` :127 and its tests :290, and
  sparkline's half of :317, go; its only caller was the card)
- Modify, the card's history read, all of it: `screens/accounts.rs` (`AccountsState::card`,
  `card_for`, `card_due_ms`; `schedule_card` :94; `on_tick` :189; `on_card` :206; the
  `schedule_card` call in `handle_key`), `state.rs:293` (`load_card`), `msg.rs` (`Cmd::LoadCard`,
  `Msg::Card`, `ViewsReq::Card` :31 and its arm in `same_as` :90), `update.rs` (`Msg::Card` :68,
  the `schedule_card` call :42, the `on_tick` call if `on_tick` is left empty), `lanes.rs:224`,
  `runtime.rs:455` (the `LoadCard` dispatch), and the `card` generation in `gens`
- Test: `accounts.rs` :1120-1197 helpers (`card_at`, `card_rows`, `card_windows`, `with_card`
  :1153), :1198, :1406, :1441, :1455, :1470-1600 (`card_reads` :1470, :1482, :1535), :1711,
  :1770, :1788, :1812, :1844, :1877, :1939, :1980, :2007, :2046, :2123, :2188, :2213, :2238;
  `runtime.rs:1506`; `daemon/mod.rs:943`; `update.rs:756` (`first_card`); `words.rs` tests

**Interfaces:**
- Produces, in `card.rs`:

```rust
/// What every window row on a screen shares (S§4.1).
pub(super) struct RowLayout {
    pub(super) indent: usize,  // 2 in the card, 7 in the full list
    pub(super) label_w: usize, // label_width over every window on the screen, plus two
    pub(super) value_w: usize, // 4, or the widest spend value on the screen
    pub(super) width: u16,
}
pub(super) fn row_layout<'a>(windows: impl IntoIterator<Item = &'a Window>, indent: usize, width: u16) -> RowLayout;
pub(super) fn window_line(w: &Window, p: &Pace, at: &RowLayout, thick: bool, now_s: i64, theme: Theme) -> Line<'static>;
pub(super) fn facts_row(state: &State, v: &AccountView, indent: usize, width: u16, now_ms: i64) -> Line<'static>;
pub(super) fn no_windows_row(v: &AccountView, indent: usize, width: u16, now_s: i64, theme: Theme) -> Option<Line<'static>>;
pub(crate) fn lines(state: &State, v: &AccountView, width: u16, rows: usize, from: usize, keys: bool, now_ms: i64) -> Vec<Line<'static>>;
```

  `problem_rows` stays as it is. In `words.rs`: `pub fn expected_words(expected_pct: f64) ->
  String` (`"26% expected by now"`); `ahead_words` builds its parenthesis from it.

- [ ] **Step 1: Write the failing tests.** In `card.rs`'s tests (`:583`; add
`use crate::testutil::*;` for `window`, `NOW_S`, `TIERS`):

```rust
fn line_text(line: Line<'static>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}
// `col(line, needle)`: copy `accounts.rs:535`'s helper (display column of `needle`).

fn ahead(expected: f64, rate: f64) -> Pace {
    Pace { ahead: Some(true), expected_pct: Some(expected), rate_per_hour: Some(rate),
           will_last_to_reset: Some(false), exhaustion_at: Some(NOW_S + 28_020), ..Pace::default() }
}

#[test]
fn a_window_row_has_its_parts_at_fixed_columns() {
    let long = window("7d", "7d", WindowKind::Long, 82.0, Some(5 * 86_400 + 4 * 3_600));
    let at = RowLayout { indent: 2, label_w: 7, value_w: 4, width: 80 };
    let text = line_text(window_line(&long, &ahead(26.0, 2.3), &at, true, NOW_S, Theme::new(false)));
    // indent 2 + label 7 + 16 cells + 1, then ` 82%` right-aligned in 4
    assert_eq!(&text[..9], "  7d     ");
    assert_eq!((col(&text, "82%"), col(&text, "▲"), col(&text, "resets in 5d04h")), (27, 31, 34));
    assert!(text.contains("resets in 5d04h · +2.3%/h · full in 7h47m"), "{text}"); // rate kept at 80
    assert!(!text.contains("expected by now"));                                   // 79 + 22 > 80
    let wide = RowLayout { width: 120, ..at };
    let text = line_text(window_line(&long, &ahead(26.0, 2.3), &wide, true, NOW_S, Theme::new(false)));
    assert_eq!(col(&text, "82%"), 37);                                            // 26 cells
    assert!(text.ends_with("+2.3%/h · full in 7h47m · 26% expected by now"), "{text}");
}

#[test]
fn pace_words_show_at_80_when_they_fit_and_never_on_short_or_spend_windows() {
    // A long window with no rate and reset in 0h12m: `  7d     {bar}  82% ▲  resets in 0h12m ·
    // 26% expected by now` fits 80: the words show. The same pace on a Short (5h) window and on
    // a Spend window: no `▲`, no words.
}

#[test]
fn bars_are_thick_for_the_account_in_use_and_thin_otherwise() {
    // thick: the bar's 16 cells are only `▆`/`▁`; thin: only `━`/`╸`/`─` (tier 0 `·`), in
    // every tier.
}

#[test]
fn a_percentage_is_bold_from_70() {
    // Ports accounts.rs:4203: 69% not bold, 70% bold, thick and thin, every tier (`bold_cells`).
}

#[test]
fn a_spend_value_widens_the_value_column_for_every_row() {
    // 5h at 44% and a Spend `€12.40 of €50`: `row_layout` gives value_w 13; `44%` ends where
    // `€50` ends, and both rows' resets start at the same column.
}

#[test]
fn the_label_column_is_capped_at_a_quarter_of_the_width() {
    // A 40-character label at 80 columns: label_w is 20 + 2, the label cut with `…`, the
    // percentage still on the row (keeps accounts.rs:1980's subject).
}
```

In `accounts.rs`, the card at its new layout (S§3.1, §3.4), `three()` plus a `Fable` window
(Long, 28%) on every account so the label column is `Fable` + 2 = 7. Define
`fn state_selecting(position: u32) -> State`: `loaded` of that set, then `j`/`k` until
`selected_view()`'s position is `position`.

```rust
#[test]
fn the_card_reads_selected_then_facts_windows_and_keys() {
    let (_, rows) = card_rows(&state_selecting(1), 80, 24);
    assert!(rows[0].starts_with("SELECTED  1 alex@example.com"));
    assert_eq!(col(&rows[1], "not in use"), 10);
    assert!(rows[2].starts_with("  5h     "));
    assert!(rows[4].starts_with("  Fable  "));
    let keys = rows.last().unwrap();
    assert_eq!(col(keys, "s switch"), 10);
    assert!(keys.contains("s switch   ⏎ history   e disable   a alias   x remove"));
}

#[test]
fn the_in_use_card_says_in_use_and_hides_s_but_s_still_switches() {
    // `work` selected: facts row starts `IN USE` (accent); the keys row starts `⏎ history`;
    // `update(&mut state, ch('s'), NOW_MS)` still returns the switch command it returns today.
}

#[test]
fn an_account_without_windows_shows_the_table_s_words_or_its_problem() {
    // `unread(UsageStatus::ApiKey, None, None)`: one row from column 2,
    // `api key · has no usage limits to show`, muted; `unread(ReloginRequired, …)`: its problem
    // rows and no extra row.
}

#[test]
fn next_update_ends_the_facts_from_120_columns_only() {
    // `next_poll_at = Some(NOW_S + 60)`: at 120 the facts row ends `· next update in 1m`; at 80
    // it has no `next update`.
}

#[test]
fn a_login_under_seven_days_is_warn_and_says_to_log_in_again() {
    // Ports accounts.rs:3450: `login_expires_at` in 3 days: `login valid 3 more days · log in
    // again before then`, both warn; passed: `login expired`, warn, alone.
}

#[test]
fn a_short_body_drops_the_keys_row_after_the_blank_rows() {
    // S§5.4 step 2: once both pairs of blanks are gone, the keys row goes before the rules.
}
```

In `words.rs`: `expected_words(26.4) == "26% expected by now"`; `ahead_words(Some(26.4))` still
`"▲ faster than an even pace (26% expected by now)"`.

Rewrite, in this task, the existing tests whose assertions this task's behaviour breaks: the
listed card tests take the new title/facts/keys rows and lose their sparkline and in-place
`▲ faster…` assertions; `:2123` (`a_window_takes_one_row_whatever_its_words`) keeps its
oversized-text, spend and overflow assertions at the new columns; the read-scheduling tests
(:1482, :1535), `runtime.rs:1506`, `daemon/mod.rs:943` and `update.rs`'s `first_card` go with
the read (a daemon test that only used `first_card` as one expected command among others keeps
the others).

- [ ] **Step 2: Run to verify they fail** — `cargo test -p tagteam-tui card`, then
`cargo test -p tagteam-engine expected_words`. FAIL.

- [ ] **Step 3: Implement.**
  - `window_line` follows S§4.1:
    `{indent}{label:<label_w}{bar} {value:>value_w} {▲ or space}  {tail}`. The bar is
    `gauge::lead_bar` when `thick`, else `gauge::line_gauge`, 16 or 26 cells by `chrome::WIDE`.
    The value is `value_cell`, bold (`theme.strong()` patched on) from 70%. `▲` shows only on
    long and scoped windows with `p.ahead == Some(true)`.
  - The tail is the reset (as `reset_tail` has it: `resets in …`, `reset`, `not started`), then
    ` · {rate}%/h` when there is a rate, then ` · {projection}`:
    `projection_clause(…, short = width < WIDE)`, warn when the window fills before it resets.
    Today's narrow tail drops the rate (`card.rs:508-518`); this one keeps it.
  - Then ` · {expected_words}` when `▲` shows and `expected_pct` is `Some`, if the whole row fits
    the width; otherwise the row goes through `fit_spans`. `▲` never expands in place.
  - `row_layout`: `label_width(windows, width) + 2`; `value_w` is 4, or the widest
    `words::spend_text` among the windows.
  - The card (S§3.4):
    - title `SELECTED` muted, two spaces, `{position} `, `name_spans`;
    - `facts_row` at indent 10: `facts_of` with `IN USE` in accent for the account in use,
      `next update` last and only from `WIDE`, and the soon-to-expire rule (`SOON_S`,
      `LOG_IN_AGAIN`);
    - window rows at indent 2, `thick: v.active`;
    - with no windows, `problem_rows` when `words::problem(v)` is `Some`, else `no_windows_row`
      (`status::unread_words`, its role);
    - problem rows;
    - the keys row at indent 10, at every width when `keys`: `s switch` (left out when
      `v.active`), `⏎ history`, `e disable`/`e enable`, `a alias`, `x remove`, `KEYS_GAP` apart.
  - `min_height`: title + facts + one window (or the no-windows row) + problem rows. `height`
    adds the other windows and the keys row. `parts` (Task 4) passes `keys` and adds S§5.4
    step 2.
  - Remove the card's history read end to end, as listed under Files.

- [ ] **Step 4: Run to verify they pass** — `cargo test --workspace --locked --no-fail-fast
--features tagteam/test-support` and the clippy runs. PASS, no dead code.

- [ ] **Step 5: Commit** — `Show the selected account in one format and drop its sparkline read`

### Task 6: `v` and the full list

**Files:**
- Create: `crates/tagteam-tui/src/screens/accounts/list.rs`
- Modify: `crates/tagteam-tui/src/screens/accounts.rs` (`AccountsState`, `handle_key` :221,
  `render`, `apply`)
- Modify: `crates/tagteam-tui/src/keys.rs` (`KEYS` :105: a `v` row between `P` :386 and `r`
  :398; `s`'s label :170-180; the probe test :753 and the footer tests)
- Test: `list.rs` tests; `keys.rs` tests; `accounts.rs` tests

**Interfaces:**
- Consumes: `card::{RowLayout, row_layout, window_line, facts_row, no_windows_row,
  problem_rows, name_spans}` (Task 5).
- Produces:

```rust
// screens/accounts.rs
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum View { #[default] Compact, List }
// AccountsState gains `view: View` and `list_from: usize` (private), and:
pub(crate) fn view(&self) -> View;
// list.rs
pub(super) struct Block { pub(super) id: AccountId, pub(super) lines: Vec<Line<'static>> }
pub(super) fn blocks(state: &State, width: u16, now_ms: i64) -> Vec<Block>;
pub(super) fn keep_whole(heights: &[usize], from: usize, at: usize, rows: usize) -> usize;
pub(super) fn render(state: &State, frame: &mut Frame, body: Rect, now_ms: i64);
```

  `keep_whole` returns the first block to show so the block `at` is whole in `rows` rows,
  moving `from` as little as needed; a block taller than `rows` is shown alone (from = at).

- [ ] **Step 1: Write the failing tests.**

```rust
#[test]
fn keep_whole_moves_as_little_as_needed() {
    // Heights include the separator rows above every block but the first (3 each).
    assert_eq!(keep_whole(&[5, 8, 8, 8], 0, 1, 17), 0);
    assert_eq!(keep_whole(&[5, 8, 8, 8], 0, 2, 17), 1);
    assert_eq!(keep_whole(&[5, 8, 8, 8], 2, 0, 17), 0);
    assert_eq!(keep_whole(&[5, 20, 8], 0, 1, 17), 1);   // taller than the viewport: alone
    assert_eq!(keep_whole(&[5, 8], 9, 1, 17), 1);       // a stale `from` clamps
}

#[test]
fn v_toggles_the_full_list_and_keeps_the_selection() {
    let mut state = loaded(three());
    update(&mut state, key(KeyCode::Down), NOW_MS);
    let before = state.accounts.selected();
    update(&mut state, ch('v'), NOW_MS);
    assert_eq!(state.accounts.view(), View::List);
    assert_eq!(state.accounts.selected(), before);
    assert!(draw(&state, 80, 24)[23].contains("v compact"));
    update(&mut state, ch('v'), NOW_MS);
    assert_eq!(state.accounts.view(), View::Compact);
    assert!(draw(&state, 80, 24)[23].contains("v full list"));
}

#[test]
fn the_full_list_at_80_by_24_matches_the_spec() {
    // S§6.1's mock: Task 4's `four()`, account 1 selected (`k`), then `v`.
    let mut state = loaded(four());
    update(&mut state, ch('k'), NOW_MS);
    update(&mut state, ch('v'), NOW_MS);
    let rows = draw(&state, 80, 24);
    assert_eq!(rows[3], ""); assert!(rows[4].chars().all(|c| c == '─')); assert_eq!(rows[5], "");
    assert!(rows[6].starts_with("▌  1   alex@example.com"));
    for r in &rows[6..11] { assert!(r.starts_with('▌'), "{r}"); }
    assert!(rows[11..].iter().all(|r| !r.starts_with('▌')));
    assert_eq!(rows[12], format!("   {}", "┈".repeat(74)));     // ends three columns in
    assert!(rows[14].starts_with("   2 ● work  sam@corp.example"));
    assert_eq!(col(&rows[15], "IN USE"), 7);
    assert!(rows[16].starts_with("       5h     ▆"));             // thick, from column 7
    assert!(rows[22].starts_with("   3   spare"));                // cut at the body's end
    assert!(!rows.iter().any(|r| r.contains("x remove") && !r.contains("q quit"))); // no keys row
}

#[test]
fn the_full_list_at_120_by_40_lines_every_block_up_in_every_tier() {
    // For every tier in TIERS: every window row's `%` column is the same in every block; 26-cell
    // bars; the org on each title; thick bars only on `work`'s rows; the dotted rule is `·` in
    // tier 0.
}

#[test]
fn blocks_without_windows_match_the_compact_card() {
    // An ApiKey account and a ReloginRequired one: the block's row from column 7 is
    // `api key · has no usage limits to show`; the other's problem rows equal the compact card's
    // problem rows, indent aside.
}

#[test]
fn moving_past_the_last_whole_block_scrolls_one_block() {
    // View::List at 80×24, account 1 selected: `j` → 2 (no scroll); `j` → 3: the first body row
    // after the pinned three (rows[6]) is block 2's title. `k`, `k` back to 1: rows[6] is block
    // 1's title again.
}

#[test]
fn page_keys_move_by_the_whole_blocks_shown() {
    // `many(8)` (two whole blocks fit at 80×24): from 1, PgDn selects 3, PgDn 5; PgUp 3; at least
    // one block when only one fits (80×24 under a full banner).
}

#[test]
fn v_into_the_list_starts_at_the_top_and_scrolls_the_selection_into_view() {
    // `many(8)`, account 7 selected in compact; `v`: the list's first block is the fewest moves
    // from block 1 that make block 7 whole (rows[6] is block 6's title); `v`, `v` again from
    // account 1: rows[6] is block 1's title.
}

#[test]
fn v_before_the_first_read_or_with_no_accounts_shows_the_same_state() {
    // `fresh(Screen::Accounts)`: `v` then draw: `Loading accounts…` centred; `loaded(vec![])`:
    // `No accounts yet`; `v` again: compact, same text.
}

#[test]
fn a_refresh_that_removes_accounts_clamps_the_list() {
    // `many(8)`, View::List, account 8 selected (list scrolled); apply `snapshot(many(3))`: the
    // selection moves to account 3 (as `apply` does) and the next frame shows block 3 whole.
}

#[test]
fn s_has_no_footer_label_for_the_account_in_use() {
    // three(): with `work` selected the footer at 80 is
    // `b best  n next  ⏎ history  P pause auto  v full list  r refresh` then the pinned keys;
    // with account 1 selected it is `s switch  b best  n next  ⏎ history  P pause auto  v full
    // list` then the pinned keys (no `r refresh`, S§8). The same in View::List with `v compact`.
}
```

In `keys.rs` the table test gains `v`'s probe (`plain('v')`).

- [ ] **Step 2: Run to verify they fail** — `cargo test -p tagteam-tui keep_whole`, then
`cargo test -p tagteam-tui full_list`. FAIL.

- [ ] **Step 3: Implement.**
  - **The `v` row:** `screen: Some(Screen::Accounts)`, `keys: "v"`,
    `does: "full list, or back to compact"`,
    `Footer::Show { key: "v", label: |s| Some(if s.accounts.view() == View::List { "compact" } else { "full list" }), from: 0 }`.
    `s`'s label becomes `state.accounts.selected_view().filter(|v| !v.active).map(|_| "switch")`.
  - **`v` itself:** toggles `view`. On entering the list it sets `list_from = 0` and then
    `keep_whole`.
  - **Keys in the list:** `j`/`k`/`↑`/`↓`/`Home`/`End` step the selection as today (one block
    is one account), and `PgUp`/`PgDn` by the number of whole blocks shown, at least one.
    Afterwards `list_from = keep_whole(…)`.
  - **Rendering:** `render` and every key recompute `keep_whole` from the stored `list_from`,
    so a resize or refresh is clamped in the next frame.
  - **A block:**
    - the title: `▌` when selected, else a space; two spaces; `{position:<2}`; `● ` or two
      spaces; `name_spans` bold when selected; the org from `WIDE`;
    - `facts_row` at indent 7;
    - the window rows through `window_line` at indent 7, `thick: v.active`, or the no-windows
      row, or problem rows;
    - then the problem rows.
  - **Block styling:** a disabled block is DIM. Every row of the selected block starts with
    `▌`, in place of its first column.
  - **Shared layout:** `row_layout` over every window of every account, once per frame.
  - **Separators:** blocks are separated by a blank row, `   ` + `┈` × (width − 6) faint (tier 0:
    `·`), then a blank row.
  - **The viewport:** the body less the pinned blank/rule/blank. Blocks are shown from
    `list_from` while they fit; the next one is drawn cut at the body's end.

- [ ] **Step 4: Run to verify they pass** — `cargo test -p tagteam-tui`. PASS.

- [ ] **Step 5: Commit** — `List every account in full on v`

### Task 7: The full list's edges

**Files:**
- Modify: `crates/tagteam-tui/src/screens/accounts/list.rs`; `accounts.rs` (`handle_key`
  `J`/`K`, `scroll_card` :258)
- Modify: `crates/tagteam-tui/src/keys.rs` (the `J K` row :411: `does: "scroll the selected card
  or block"`)
- Test: `list.rs`, `accounts.rs`

- [ ] **Step 1: Write the failing tests.**

```rust
#[test]
fn a_provider_heading_starts_its_first_block_s_scroll_unit() {
    // Two providers (as `several_providers_are_grouped_under_their_display_names`, :2939, sets
    // up), View::List at 80×40: the row above provider 2's first block title is its muted
    // heading; scrolled so that block is first (select provider 2's last account at 80×24),
    // rows[6] is the heading; scrolled to provider 2's second block, rows[6] is that block's
    // title.
}

#[test]
fn with_one_account_its_block_is_followed_by_the_add_a_second_hint() {
    // `loaded(vec![one account])`, View::List at 80×24: after the block, a blank row, then
    // ` Add a second account to switch: log in to Claude Code as it, then press A.` muted;
    // compact still shows it under the table.
}

#[test]
fn a_block_taller_than_the_viewport_shows_alone_and_scrolls_its_windows() {
    // The selected account with `scoped(24)` (:1628) at 80×24: rows[6] is its title, rows[7] its
    // facts; the last body row is `J K: windows 1–n of 24`; `J` moves the first window shown by
    // one; its problem rows (quarantined) stay on screen throughout.
}

#[test]
fn at_80_by_24_under_a_full_banner_and_toast_the_selected_title_shows() {
    // As `every_window_is_reachable_by_shift_j_and_k_at_80x24_under_a_full_banner` (:1634) sets
    // the banner and toast up, View::List, `many(8)`, account 8 selected: the draw does not
    // panic and the body's first row after the pinned three is block 8's title.
}

#[test]
fn a_resize_keeps_the_selected_block_whole() {
    // `many(8)` at 120×40, View::List, account 6 selected and shown; deliver
    // `Msg::Resize { width: 80, height: 24 }` (no key): the next draw at 80×24 shows block 6
    // from its title to its last window row inside the body.
}
```

- [ ] **Step 2: Run to verify they fail** — `cargo test -p tagteam-tui heading`, then
`cargo test -p tagteam-tui taller_than`, `cargo test -p tagteam-tui resize_keeps`. FAIL.

- [ ] **Step 3: Implement.**
  - **Provider headings:** with several providers, the muted ` {display_name}` heading is the
    first line of each provider's first block.
  - **The one-account hint:** it is appended, after a blank row, to the sole block of focus's
    provider (`one_account`).
  - **Tall blocks:** a selected block taller than the viewport draws its windows through the
    card's `Shown` logic with `card_from`. `J`/`K` call `scroll_card` with the rows the active
    view gives the selected account.

- [ ] **Step 4: Run to verify they pass** — `cargo test -p tagteam-tui`. PASS.

- [ ] **Step 5: Commit** — `Scroll the full list's tall blocks and place its headings and hint`

### Task 8: Changelog, help, and the run

**Files:**
- Modify: `CHANGELOG.md` (`## [Unreleased]` :8)
- Modify: `docs/superpowers/specs/2026-10-26-tagteam-accounts-views-design.md`
  (`**Status:** Draft` → `Implemented`)
- Test: `overlay/help.rs` tests

- [ ] **Step 1: Help test** — on Accounts, help lists `v` with `full list, or back to compact`,
still lists `s` and `r`, and lists `J K` as `scroll the selected card or block`. The marks
legend keeps `▲ faster than an even pace`. Run `cargo test -p tagteam-tui help`; PASS (the rows
come from `KEYS`).
- [ ] **Step 2: Changelog** — under `[Unreleased]`, in the file's style:
  - `### Changed`: Accounts shows the account in use in the Now strip, and every account's
    windows in one format; bars have no pace marks; `▲` adds the expected share in words where
    it fits; the Now strip shows `▲` on every screen.
  - `### Added`: `v` lists every account in full.
  - `### Fixed`: Tokens says `before tracking`, not `unknown account`, for unattributed usage
    from before tracking started.
- [ ] **Step 3: Full checks** — `cargo fmt --all --check`; the clippy runs CONTRIBUTING names;
`cargo test --workspace --locked --no-fail-fast --features tagteam/test-support` (the pty
tests from a real terminal). Expected: all pass; report the counts.
- [ ] **Step 4: Screenshots from a test store** (`tagteam-fake` and a scratch directory, as
CONTRIBUTING requires; never a real account or a real machine's store) at 80×24 and 120×40,
both views. The person driving the work may also check the build against their own store
privately; nothing from that goes into the PR (S§1.4).
- [ ] **Step 5: Commit** — `Record the accounts views in the changelog and mark the spec implemented`
