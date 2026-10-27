# tagteam — Accounts in one format, a full list, and pre-tracking token rows

**Status:** Implemented — https://github.com/michaelrigart/tagteam/pull/50
**Date:** 2026-10-26
**Scope:** The TUI's Accounts screen and one label on its Tokens screen. Accounts loses the lead
card, shows every account's details in one format, and gains a full-list view (`v`). Tokens names
the usage it could not attribute because it predates tracking. The engine exposes the token
index's epoch with the report. `tagteam list`, `tagteam tokens` and every `--json` output are
unchanged.
**Builds on:**
- **Redesign spec:** [`2026-10-20-tagteam-tui-redesign-design.md`](2026-10-20-tagteam-tui-redesign-design.md)
  (Implemented), cited as "redesign §n". It stays the authority for what this spec does not
  change. §9 lists the parts this spec replaces.
- **Token report spec:** [`2026-10-14-tagteam-token-report-design.md`](2026-10-14-tagteam-token-report-design.md),
  cited as "TR §n".

Line numbers are as of `eda9605` and will drift.

---

## 1. Purpose

### 1.1 Why

The Accounts screen shows the account in use up to three times: in the lead card, as a table row
and, when it is selected, in the selected card. The lead card and the selected card draw the same
windows in two formats (thick bars with reset and projection; sparklines, with some of those
details left out for the account in use, as below). Which details and which keys belong to which
account is not obvious. Pace marks (`│`, `┿`, `┼`) on the bars read as stray lines.

(Today the in-use card leaves out the reset and projection of the windows the lead card draws,
T3-32, `card.rs:69-75`, and its problem rows, M-9. A window the lead card does not draw, such as
a scoped one at 80 columns, keeps its reset.)

On Tokens, usage from before tagteam started recording which account was in use is often
unattributed, and those rows read `unknown account`. That looks like an error, while it is the
expected result where the evidence before the token index's epoch is thin (TR §7.2-7.3).
Pre-epoch usage that is attributed today, from seeded launches and stored events, keeps its
account's name.

### 1.2 Goals

1. **One format for an account's details.** Title, facts, then one row per window: bar,
   percentage, `▲`, reset, rate, projection. The same rows in the selected card and in the full
   list, for every account, the account in use included.
2. **The account in use is obvious without repeating it.** The Now strip, which Tokens and
   History already have, carries it on Accounts too. In the details it has thick bars and
   `IN USE`; every other account has thin gauges and `not in use`.
3. **Every account's details on one screen** when wanted: the full list (`v`).
4. **Aligned.** Every column at a fixed position in its view; window rows line up across the card
   and across the full list's blocks.
5. **More air around rules,** and no tick marks on bars.
6. **Usage from before tracking says so** on Tokens, without changing what the filter selects.

### 1.3 Non-goals

- **Attributing pre-epoch usage to an account** (from Claude Code's `credential_org` transcript
  lines, an org UUID per session). A separate later spec; it would shrink what §7 labels.
- **`tagteam list`, `tagteam tokens` and `--json`.** The CLI keeps `unattributed`
  (`crates/tagteam/src/tokens_cmd.rs:309,550,745`), and the JSON is built field by field
  (`tokens_cmd.rs:323`), so a new report field does not reach it.
- **History and Tokens layouts.** Their chrome and rule stay as they are. Their Now strip changes
  only as the strip changes on every screen (§3.2): no pace mark in its gauges, `▲` after the
  percentage.
- **Sparklines elsewhere.** The Tokens headline and History keep theirs.

### 1.4 Anti-goals

Redesign §1.4 holds. In particular: 80×24 is the first target, and everything works there; no
meaning carried by colour alone (every state also has a glyph, a word or a shape); the 50 ms p95
first frame; no real names in the spec, tests, fixtures or images; no second vocabulary — words
come from the engine's shared words or the TUI's existing ones.

## 2. Decisions

| ID | Question | Decision |
|---|---|---|
| AV-1 | What replaces the lead card? | The Now strip (redesign §5.4), on Accounts too |
| AV-2 | How are an account's details shown? | One format (§4) in the selected card and the full list; no sparklines on Accounts. `⏎` opens History for the account, as today |
| AV-3 | Which view lists every account in full? | A second view, the full list, toggled by `v`; compact stays the default |
| AV-4 | How is the account in use told apart? | `●` in the Now strip and the table, `IN USE` on its facts row, thick `▆` bars; other accounts `not in use` and thin `━` gauges |
| AV-5 | Where is the selection in the full list? | `▌` on every row of the selected account's block, and bold on its name |
| AV-6 | Pace marks? | None on any bar or gauge. `▲` stays; where the row has room it ends with the expected percentage in words (§4.3) |
| AV-7 | Spacing? | A blank row above and below every rule on Accounts |
| AV-8 | Tokens rows from before the epoch? | `before tracking` where the row's whole span lies before the epoch; `unknown account` everywhere else, the filter included |

AV-1 to AV-7 follow the design agreed in chat on 2026-10-26 (mock v2). AV-8 follows the same
discussion. §10 lists the calls made while writing this spec.

## 3. Accounts: the compact view (default)

### 3.1 Layout

From the top: the chrome with the Now strip (§3.2), the table between two rules, the selected
card. Spare rows go to the bottom (VIS-5). At 80×24, with four accounts of three windows each,
the account in use selected:

```
 tagteam   1 Accounts   2 Tokens   3 History        ● auto on · at 90%   ? keys
 ⇄ 11:28  auto switched 1 alex → 2 work · alex's 5h reached 93%           w why
 ● 2 work   5h  44% 2h25m   7d  82% ▲ 5d04h                 on the best account

────────────────────────────────────────────────────────────────────────────────

    #   ACCOUNT                     5H                7D                 UPDATED
    1   alex@example.com            ─────────   0%    ━━━━━━━━╸  98% ▲        1m
▌   2 ● work   sam@corp.example     ━━━╸─────  44%    ━━━━━━━──  82% ▲        2m
    3   spare  alex@personal.exa…   ─────────   2%    ━━━━━━━━─  90%          1m
    4   kim@example.org             ━━━━━━━━╸  99%    ━────────  13%          2m

────────────────────────────────────────────────────────────────────────────────

SELECTED  2 work  sam@corp.example
          IN USE · updated 2m ago · login valid 27 more days
  5h     ▆▆▆▆▆▆▆▁▁▁▁▁▁▁▁▁  44%    resets in 2h25m · +16.9%/h · lasts
  7d     ▆▆▆▆▆▆▆▆▆▆▆▆▆▁▁▁  82% ▲  resets in 5d04h · +2.3%/h · full in 7h47m
  Fable  ▆▆▆▆▁▁▁▁▁▁▁▁▁▁▁▁  28%    resets in 5d04h · +0.9%/h · full in 3d08h
          ⏎ history   e disable   a alias   x remove



b best  n next  ⏎ history  P pause auto  v full list  r refresh  ? keys  q quit
```

Row count: header, auto line, Now strip (3); body rows 4–23 (20); footer (1). The body uses 17:
blank, rule, blank (3), the table head and four rows (5), blank, rule, blank (3), the card (6).

With account 1 selected, the card and the footer read:

```
SELECTED  1 alex@example.com
          not in use · updated 1m ago · login valid 27 more days
  5h     ────────────────   0%    not started
  7d     ━━━━━━━━━━━━━━━╸  98% ▲  resets in 1d18h · +0.5%/h · full in 4h00m
  Fable  ────────────────   0%    resets in 1d18h
          s switch   ⏎ history   e disable   a alias   x remove
s switch  b best  n next  ⏎ history  P pause auto  v full list   ? keys  q quit
```

At 120×40 the table gets STATUS and 14-cell gauges, the Now strip its gauges, the card its org,
26-cell bars, the long projection and the pace words; the footer the account keys:

```
 tagteam   1 Accounts   2 Tokens   3 History                                       ● auto on · switches at 90%   ? keys
 ⇄ 11:28  auto switched 1 alex → 2 work · alex's 5h reached 93%                                                   w why
 ● 2 work   5h ━━━━──────  44% 2h25m   7d ━━━━━━━━──  82% ▲ 5d04h                                   on the best account

────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────

    #   ACCOUNT                        5H                     7D                      UPDATED  STATUS
    1   alex@example.com               ──────────────   0%    ━━━━━━━━━━━━━╸  98% ▲        1m
▌   2 ● work   sam@corp.example        ━━━━━━────────  44%    ━━━━━━━━━━━───  82% ▲        2m  in use
    3   spare  alex@personal.example   ──────────────   2%    ━━━━━━━━━━━━╸─  90%          1m
    4   kim@example.org                ━━━━━━━━━━━━━╸  99%    ━╸────────────  13%          2m

────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────

SELECTED  2 work  sam@corp.example · Example Corp
          IN USE · updated 2m ago · login valid 27 more days · next update in 1m
  5h     ▆▆▆▆▆▆▆▆▆▆▆▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁  44%    resets in 2h25m · +16.9%/h · lasts until it resets
  7d     ▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▁▁▁▁▁  82% ▲  resets in 5d04h · +2.3%/h · full in 7h47m · 26% expected by now
  Fable  ▆▆▆▆▆▆▆▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁  28%    resets in 5d04h · +0.9%/h · full in 3d08h
          ⏎ history   e disable   a alias   x remove
                                   (spare rows)
b best  n next  ⏎ history  P pause auto  v full list  r refresh  e disable  a alias  x remove            ? keys  q quit
```

### 3.2 The Now strip on Accounts

Redesign §5.4, now on all three screens, with the one change below: `●`, position, name, the short and long
windows, and on the right the best pick in its Now strip form (`lead::pick_words`,
`best now 3 spare` or `on the best account`). `b` stays in the footer. The lead card's keyed
forms go with it (`best now  3 spare   b switch`, and `b checks usage first, then picks` before
any usage is read), so `pick_words` keeps only the strip's form; the footer and help still name
`b`.

With several providers the strip follows focus's provider, the selected account's, and does not
name it, as on Tokens and History; the table's provider headings name it. The lead card's
`in use · Claude Code` goes with the card.

One change on every screen: the strip's gauges (from 120 columns) lose their pace mark (§4.2),
and `▲` in warn follows a window's percentage, after one space, when that window is ahead of pace
(`Pace.ahead`), at every width: `7d  82% ▲ 5d04h`. Today the strip passes the expected pace to
the gauge and draws no `▲` (`chrome.rs:228-249`).

Two Accounts-only differences, both because the strip's reader is already on Accounts:

- A problem with the account in use reads `● 2 work   ✕ needs a new login`, without
  ` · see Accounts` (`chrome.rs` `SEE_ACCOUNTS`): its card or block, and the table's STATUS, are
  on the same screen.
- With no account in use, the strip reads `no account in use · s switches to the selected
  account`, the lead card's hint (`lead.rs:37` `NO_ACCOUNT_HINT`). Tokens and History keep `no
  account in use · s on Accounts switches to one`.

On Accounts the chrome draws no rule under the strip at any height; the body's own rule (§5.2)
takes its place. Tokens and History keep the chrome rule from 30 rows (`view.rs:45-69`).

### 3.3 The table

Redesign §6.3 and today's layout (`accounts/table.rs`) unchanged, except that gauges carry no
pace mark (AV-6): the name from column 8, the alias padded to the widest alias, the window columns
and UPDATED after it (at 80 columns 5H, 7D and UPDATED start at zero-based columns 36, 54 and 73
in the mock; at 120, 39, 62 and 86). The table does not change with `v`: compact shows it, the
full list replaces it.

### 3.4 The selected card

- **Title:** the muted capital label `SELECTED`, two spaces, the position, the alias bold and the
  email muted (or the email bold), the org muted from 120 columns.
- **Facts row,** from column 10 (under the position): today's facts (`card.rs:353` `facts_of`),
  in its order and roles, joined by ` · `: `IN USE` (accent, now in capitals) for the account in
  use or `not in use` (muted); the account kind's display name; `in a session`; the run shell's
  `this session` marker; `disabled`; `updated 2m ago` (warn when the reading is old), or
  `words::NOT_CHECKED` (`not checked yet`) only when the problem is `Problem::NotChecked`
  (`card.rs:380-385`); `login valid 27 more days` (warn once passed). Two changes: `next update in 1m` moves
  to the end and shows from 120 columns only, and a login under 7 days is in warn with `· log in
  again before then`, as the lead card had it (redesign §6.2). A row too wide is cut at its end.
- **A row per window,** every window the account has, in §4's format; thick bars for the account
  in use, thin gauges otherwise.
- **No windows** (no reading, or a reading with an empty list): an account with a problem shows
  its problem rows in the windows' place. One without (an API key, an unsupported provider, an
  empty list) shows one row from column 2 with the words the table puts across its window
  columns (`status.rs:124-138` `unread_words`, redesign §6.3), in their role:
  `api key · has no usage limits to show`. Today's card shows nothing there (`card.rs:49-52`).
  This rule is the full list's too (§6.1).
- **Problems** under the windows, as redesign §6.4: the sentence from the window rows' column
  (2 in the card), then the remedy two columns further in, in their role. For the account in use
  too: no lead card shows them any more (M-9 no longer applies).
- **Keys row** last, from column 10: `s switch` (not shown for the account in use, §8), `⏎ history`,
  `e disable` (or `e enable`), `a alias`, `x remove`, three spaces apart. At every width while
  the body has room (§5.4).

## 4. One format for a window

### 4.1 The row

```
{indent}{label:<L}{bar} {pct:>4} {▲ or space}  {tail}
```

The tail is the parts the window has, in this order, joined by ` · `: the reset, the rate, the
projection, `{NN}% expected by now`. A part the window lacks leaves no separator: a scoped or
spend window without a reset starts its tail with the rate, at the reset's column.

- `L`: today's label column (`card.rs:340-349` `label_width`: the widest label, at least
  `LABEL_LEAST`, at most a quarter of the width; a longer label is cut to it), taken over every
  window on the screen, plus two. One `L` per screen, so bars line up across the card and across
  the full list's blocks, and a long label never pushes the values off the row.
- **Bar:** 16 cells below 120 columns, 26 from 120 (`chrome::WIDE`).
  - The account in use: `lead_bar`, `▆` fill in the severity's role on a faint `▁` baseline.
  - Any other account: `line_gauge`, `━`/`╸` in the severity's role on a faint `─` track
    (NO_COLOR: `·`).
- **`pct`** right-aligned in four columns, bold from 70 % (redesign §6.2); `▲` in warn when
  ahead of pace, else a space, so the reset starts at the same column on every row.
- **A spend window** keeps its value as today (`card.rs:543` `value_cell`, `€12.40 of €50`).
  The value column is then as wide as the widest value on the screen, one width for every row of
  the card or of the full list, every value right-aligned in it.
- **Reset, rate, projection** in redesign §11.1's words, as the lead card had them (`words.rs`): `not
  started`, `resets in 5d04h`, `+2.3%/h`, `full in 7h47m`, `lasts until it resets`. The
  projection is in warn when the window fills before it resets. Below 120 columns the projection
  is its last clause (`lasts`, `full in 7h47m`), as redesign §6.2 says for 80 columns.

### 4.2 No pace marks

`line_gauge` and `lead_bar` (`gauge.rs:56`, `:106`) draw no `┿`/`┼`/`│`; `pace_cell`
(`gauge.rs:33`) goes. This covers the table and the Now strip as well as the details.

### 4.3 Pace in words

A row with `▲` ends with ` · 26% expected by now` when the whole row then fits the width;
otherwise those words are left out and `▲` alone remains. The words are the parenthesis of the
engine's `ahead_words` (`words.rs:333`), `Pace.expected_pct` rounded. The builder splits that
clause into its own function so both read the same text. `▲` keeps its column; it no longer
expands in place to `▲ faster than an even pace (…)` (today at `card.rs:450`). Help keeps
`▲  faster than an even pace` (`overlay/help.rs:30`). Windows without pace (5h, C-8) never show
the words.

## 5. Spacing, alignment and overflow

### 5.1 Alignment

Every column has a fixed position within its view: the table's as redesign §6.3; a window row's
label, bar, percentage, `▲` and reset as §4.1, the same across every block of the full list.
Tests assert column positions (§11).

### 5.2 Rules and blank rows

On Accounts, a faint `─` rule across the full width (`chrome::rule`) separates the chrome from the
table and the table from the card, with a blank row above and below each. In the full list the
same rule sits under the chrome, and blocks are separated by a faint `┈` rule indented three
columns and ending three columns in (NO_COLOR: `·`), with a blank row above and below.

### 5.3 Width

At 80 columns everything works (redesign §4.5). When a window row is too wide, in order: the
pace words go, then the row is cut at its end (today's `fit_spans`). The table shrinks as redesign
§6.3 says.

### 5.4 Height

The table and the card scroll before any separator goes. When the body is shorter than compact
needs, rows go in this order:

1. the table's rows beyond `min(its lines, TABLE_LEAST)` (`TABLE_LEAST` is 4, `accounts.rs:36`):
   it scrolls with the selection, as today (`accounts.rs:282-330`, T3-22);
2. the card's windows beyond its least (title, facts, one window, problems): they scroll with
   `J`/`K`, as today;
3. the four blank rows around the two rules: the lower pair first, and in each pair the
   blank under the rule before the one above it;
4. the card's keys row (help lists the keys; the footer keeps `s` and `⏎` at every width);
5. the two rules.

What is left, the table's and the card's least, is reserved first, as today. Problem rows never
drop. At 80×24 with four accounts and three windows nothing drops; with seven accounts the body
is full (20 rows); from eight, the table scrolls and the separators stay.

## 6. Accounts: the full list (`v`)

### 6.1 Layout

Every account as a block: title, facts row, a row per window (§4), problems. Blocks follow table
order, separated as §5.2. At 80×24 with account 1 selected:

```
 tagteam   1 Accounts   2 Tokens   3 History        ● auto on · at 90%   ? keys
 ⇄ 11:28  auto switched 1 alex → 2 work · alex's 5h reached 93%           w why
 ● 2 work   5h  44% 2h25m   7d  82% ▲ 5d04h                 on the best account

────────────────────────────────────────────────────────────────────────────────

▌  1   alex@example.com
▌      not in use · updated 1m ago · login valid 27 more days
▌      5h     ────────────────   0%    not started
▌      7d     ━━━━━━━━━━━━━━━╸  98% ▲  resets in 1d18h · +0.5%/h · full in 4h00m
▌      Fable  ────────────────   0%    resets in 1d18h

   ┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈

   2 ● work  sam@corp.example
       IN USE · updated 2m ago · login valid 27 more days
       5h     ▆▆▆▆▆▆▆▁▁▁▁▁▁▁▁▁  44%    resets in 2h25m · +16.9%/h · lasts
       7d     ▆▆▆▆▆▆▆▆▆▆▆▆▆▁▁▁  82% ▲  resets in 5d04h · +2.3%/h · full in 7h47m
       Fable  ▆▆▆▆▁▁▁▁▁▁▁▁▁▁▁▁  28%    resets in 5d04h · +0.9%/h · full in 3d08h

   ┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈

   3   spare  alex@personal.example
s switch  b best  n next  ⏎ history  P pause auto  v compact     ? keys  q quit
```

The body's 20 rows hold two whole blocks; the third starts on the last row, which tells the
reader the list goes on.

- **Title:** `▌` in column 0 (selected only, else a space), the position right-aligned in
  columns 1–3, a space, `●` for the account in use (else a space), a space, the name from column
  7 (bold when selected), the org muted from 120 columns. Positions 10 to 999 keep the gap before
  `●` and the name at column 7.
- **Facts row** as §3.4, from column 7.
- **Problems** under the windows, as the card's, the sentence from column 7 and the remedy from
  column 9. A disabled account's block is DIM throughout, as its table row (redesign §4.3). An
  account with no windows follows §3.4's rule, its row from column 7.
- **No keys row:** the footer carries the selected account's keys (`s` (not for the account in
  use), `⏎`, and from 120 columns `e`, `a`, `x`), as it already does.
- **Several providers:** a muted provider heading before each provider's blocks, as the table
  has. The heading belongs to its provider's first block: it is the first row of that block's
  scroll unit and counts in its height, so a list scrolled to that block starts with the heading.
  A list scrolled to a later block of the same provider starts with that block's title, without
  the heading. With one provider there is no heading.
- **One account:** when focus's provider has one account (`accounts.rs:339-344` `one_account`),
  today's muted hint (`accounts.rs:462-474`, redesign §5.7) is the row after that account's
  block, after one blank row. Both rows belong to the block's scroll unit and count in its
  height. Compact keeps the hint under the table, as today.
- **The heading and the hint are not the account's rows:** they scroll with its block, but take
  no `▌` when it is selected and no DIM when it is disabled; both stay muted.

### 6.2 Moving and scrolling

- `j`/`k`/`↑`/`↓` move the selection a block; `Home`/`End` to the first or last.
- **Scrolling is by block,** as `view.rs:82` `scrolled_windows` does for windows: the scroll
  state is the first block shown, so the list always starts at a block's title. The blank, rule
  and blank under the chrome stay pinned. Blocks follow from the first shown while they fit; the
  next one is drawn cut at the body's end, as in the mock.
- The first block shown moves as little as needed to keep the selected block whole: down when
  the selection goes past the last whole block, up when it goes above the first. `v` into the
  full list scrolls the same way from the top.
- `PgUp`/`PgDn` move the selection by the number of whole blocks shown, at least one.
- **The block viewport** is the body less the three pinned rows (17 rows at 80×24); "fits" and
  "whole" are measured against it. A selected block taller than the viewport shows alone: its
  title, facts and problem rows stay, and its windows scroll with `J`/`K`, as the compact card's
  do today (`card.rs:85-113`).
- The selection is the account ID (`AccountsState::selected`), so it survives `v` and refreshes;
  `v` keeps it and scrolls it into view.
- The view (compact or full list) lasts for the session; tagteam starts in compact.

## 7. Tokens: rows from before tracking

### 7.1 The rule

The report exposes the token index's epoch, the instant this store started recording sightings
and launches (TR §7.2, `token_epoch.at`). A row's unattributed account reads:

- **`before tracking`** when the row's whole span lies before the epoch;
- **`unknown account`** otherwise: a row whose span reaches the epoch or later, and every row when
  the report has no epoch (after a full purge, before the next refresh).

"Wholly before" by period (`view/mod.rs` `PeriodKey`):

| Period | Its span | Before tracking when |
|---|---|---|
| Day, Month | the calendar day or month in `report.query.tz`; the end is the next one's start, exclusive (calendar helpers: `tagteam-core/src/tokens/bucket.rs:40-54`) | `end <= epoch` |
| Session | `first_ms` to `last_ms`, both response times, inclusive (`group.rs:650-651`, `:709-713`) | `last_ms < epoch` |
| Block | `start_ms` to `end_ms`, the end exclusive | `end_ms <= epoch` |

A session with a response exactly at the epoch is therefore `unknown account`. Drilled rows follow
the same rule for their own span. Only unattributed rows change: an attributed pre-epoch row keeps
its account's name (§1.1).

Both words are 15 columns, so the ACCOUNT column keeps its width.

### 7.2 Where it applies

Every place a row names its account:

- the table cell (`table.rs:638`, via `account_text` `:252`), including the list of accounts in
  an ungrouped row;
- the SELECTED detail line (`table.rs:1268`, via `account_name` `:267`);
- a breadcrumb drilled from that row (`table.rs:1035`).

The filter is about accounts, not rows, and keeps `unknown account`: its dialog choice
(`filter.rs:155`, which also uses `account_name`) and the control bar's summary (`table.rs:980`).
That choice selects every unattributed row, before tracking included. Help says so in one entry
at the end of its Tokens section, shown only on Tokens: `before tracking  usage from before
tagteam recorded which account was in use; the filter's unknown account includes it`. The entry
has its own lead column; the Marks lead column (`overlay/help.rs:138-142`) keeps its width.

Example, with tracking since 2026-10-23 09:07 local time, grouped by account:

```
 2026-10-22  before tracking    5.0K      900  105.9K       —
 2026-10-23  unknown account    1.2K      210   20.4K       —
 2026-10-24  2 work            48.3K    6,104    1.1M  $12.40
```

### 7.3 The engine

`TokenReport` (`view/mod.rs`) gains `tracking_since_ms: Option<i64>`: the epoch the report
already reads (`view/mod.rs:406`, `store.token_epoch()?`), `None` when it has none. No other
token-report or attribution behaviour changes; the only other engine change is the shared-words
split of §4.3. The report's JSON is unchanged (§1.3).

## 8. Keys

| Where | Key | Does | Footer |
|---|---|---|---|
| Accounts | `v` | the full list, or back to compact | `v full list` / `v compact`, at every width, while an account is selected |

- `v`'s hint hides with no account selected (before the first snapshot, or with no accounts), as
  every Accounts key's does (VIS-10, `keys.rs:66` `on_account`). The key still toggles: the
  loading and empty states read the same in both views.
- In `KEYS` (`keys.rs:105`) the `v` row goes after `P` and before `r`. The footer drops items
  from the end (`keys.rs:525` `footer_items`), so at 80 columns `r refresh` is left out when `s`
  shows; help lists it.
- `s` gets no footer label, and no card keys row entry, when the selected account is the one in
  use. Only the hint is hidden: the key still works and keeps today's path, since switching to the
  account in use can be a real self-switch that installs replacement credentials when the live
  ones diverge (`tagteam-engine/src/switch/plan.rs:147-165`, `crates/tagteam-tui/src/actions.rs:179-191`). Help keeps
  listing `s`.
- `J`/`K` scroll the card in compact and a tall block in the full list.
- The key table's test (`keys.rs:753`) gains `v`'s probe.

Accounts' footer order is now `s switch  b best  n next  ⏎ history  P pause auto  v full list  r
refresh`, then from 120 columns `e disable  a alias  x remove`.

## 9. Changes to the redesign spec

What each changes; the rest of each section stands. Each of these passages in the redesign spec
carries an `*(Amended by the accounts-views spec §n.)*` note pointing here.

| Redesign | Change |
|---|---|
| M-1 | Accounts keeps no sparklines (AV-2); `Enter` still opens the timeline for the account. The History parts stand |
| M-4 | The lead card and its bars with pace marks go (AV-1, AV-6). Thick bars mark the account in use in its card and block. The rest of Direction A stands |
| M-9 | No longer applies: no lead card. The in-use card shows its reset, projection and problems |
| §1.2 goal 1 | "The account in use leads Accounts … thick bars with pace marks, plus the best pick" → the Now strip carries it on every screen; on Accounts its details have thick bars |
| §4.3 Sections | On Accounts, a rule with a blank row above and below separates sections (§5.2 here); no chrome rule on Accounts |
| §4.4 Lead bar, Line gauge | No pace marks; "The pace mark" paragraph goes. `▲` plus words (§4.3 here) |
| §5.1 row 3 | The Now strip on every screen; "Accounts' lead card plays this role" goes |
| §5.4 | Title "(Tokens and History)" → every screen; its gauges without pace mark and `▲` after an ahead window's percentage, on every screen; on Accounts no ` · see Accounts` (§3.2 here) |
| §5.7 row 3 | "under the table" holds for compact; in the full list the hint follows the account's block (§6.1 here) |
| §6.1 | Replaced by §3.1 and §6 here |
| §6.2 | The lead card goes. Its facts and window format move to the card (§3.4, §4). The best pick stays §12.1's `best_candidate`, shown in the Now strip. Its keyed pick forms and, with several providers, its provider name go (§3.2 here) |
| §6.3 | Gauges without pace marks |
| §6.4 | Replaced by §3.4: no sparklines; the reset and projection for every account; the keys row at every width while it fits |
| §6.5 | `J`/`K` also scroll a tall block in the full list; `v` added |
| §8 | "an unattributed row as `unknown account`" → §7 here |
| §10 | `v` added; Accounts' footer order as §8 here |

## 10. Calls made in this spec

These are my choices where the agreed design left a detail open; each can change without
changing the design.

1. **Pace words** read `26% expected by now`, the engine's existing words, not the mock's
   `even pace 26%`, so there is one vocabulary (§4.3).
2. **No chrome rule on Accounts;** its body draws its own rules, so the spacing is the same at
   every height (§3.2, §5.2).
3. **Bars:** 16 cells below 120 columns, 26 from 120, one threshold (`WIDE`) like the rest of the
   card (§4.1).
4. **The keys row now shows at 80 columns** when it fits, and lists `s` for an account not in use
   (§3.4).
5. **`v` sits before `r`** in the footer, so the new key is visible at 80 columns; `r refresh`
   then drops at 80 when `s` shows (§8).
6. **Height overflow order** (§5.4): the table, then the card's windows, scroll before any blank
   row or rule goes; **width:** pace words go first (§5.3).
7. **The full list starts with a rule** under the chrome, like compact (§6.1).
8. **On Accounts the Now strip leaves out ` · see Accounts`** (§3.2).
9. **The view does not persist** across launches (§6.2).
10. **Tokens' label is `before tracking`,** in every row-naming place; the filter keeps `unknown
    account` (§7).
11. **The full list scrolls by block** with its top rule pinned, and moves as little as needed
    (§6.2).
12. **The facts row keeps every fact the card has today;** only `next update` moves to the end,
    from 120 columns (§3.4).
13. **A spend window keeps its amount;** the value column widens to the widest value (§4.1).
14. **The Now strip gains `▲`** on every screen, where its gauges lose the pace mark (§3.2).
15. **`s` on the account in use:** only its hint is hidden; the key keeps working (§8).
16. **Compact's card facts and keys start at column 10,** under the position (§3.4). The full
    list's facts start at column 7, under the name; its blocks have no keys row (§6.1).
17. **An account with no windows** shows the table's no-reading words in their place, or its
    problem rows when it has a problem, in both views (§3.4).
18. **The mocks' numbers follow the engine:** `▲` only where the reading is 15 points or more
    ahead of the even pace (`tagteam-core/src/pace.rs:17`), and each projection follows from
    its rate.
19. **With several providers the Now strip does not name its provider** (§3.2): the table's
    headings do, and the strip reads the same on every screen.
20. **On Accounts, the strip with no account in use keeps the lead card's hint,** `s switches to
    the selected account` (§3.2).
21. **The keyed pick forms go with the lead card** (§3.2); `b` stays in the footer and help.
22. **A full-list block's position is right-aligned in three columns** (§6.1), so the name
    stays at column 7 up to 999 accounts.
23. **The provider heading and the one-account hint take no `▌` or DIM** (§6.1): they are not
    the account's rows.

## 11. Verification

- Renders with `testutil.rs` (`draw_at`, `text_of`, `locate`) at 80×24 and 120×40, both views,
  every tier in `TIERS` (`testutil.rs:50`): column positions of the table, the card and every
  block; compact has no lead card and exactly one `SELECTED`; the full list has `▌` on every row
  of the selected account's block and on no other row, its provider heading and the one-account
  hint included; thick bars on the in-use account's rows only; no `┿`,
  `┼` or `│` in any bar; the 80×24 row counts of §3.1 and §6.1.
- Tests reversed on purpose: `chrome.rs:1147` (Accounts had no Now strip), `chrome.rs:1352` (the
  rows above the body: Accounts gains the Now strip and never has the chrome rule), `accounts.rs:1277` and `:3975`
  (the in-use card left out what the lead card showed, M-9), `gauge.rs:234` and `:257` (pace
  marks).
- The Now strip on Tokens and History: no pace mark in its gauges at 120 columns, `▲` after an
  ahead window's percentage at 80 and 120.
- A window label longer than a quarter of the width is cut, and the values stay on the row (as
  today's tests at `accounts.rs:1975-2035`).
- `s` with the account in use selected: no hint in the footer or the card, and the key still
  dispatches the switch.
- An API-key account and one needing a login, selected in compact and shown in the full list:
  the same replacement row or problem rows in both views (§3.4).
- Tokens: a day, a month, a session and a block wholly before the epoch read `before tracking`;
  one spanning it, one after it, every row with no epoch, and a session with a response exactly
  at the epoch read `unknown account`; an attributed pre-epoch row keeps its account's name; the filter's
  choice and summary read `unknown account`; the detail line and a breadcrumb use the row's
  words.
- `v`: toggles, keeps the selection across `v` and a refresh, PgUp/PgDn by whole blocks.
- The CONTRIBUTING checks: `cargo fmt --all --check`, the three clippy runs, and `cargo test
  --workspace --locked --no-fail-fast --features tagteam/test-support` (the pty tests need a
  terminal).
- Screenshots of both views at 80×24 and 120×40 from a test store (`tagteam-fake` and a scratch
  directory, as CONTRIBUTING requires); these are the only images that may be published (§1.4).
- A private run of the built TUI against a real store, both views at 80×24 and 120×40 and Tokens
  across the epoch. Only its outcome is recorded; no screenshot, name or figure from it is
  published.

## 12. Left to the builder

- How `parts` (`accounts.rs:289`) splits the body without the lead card, and where the full list
  lives (a new `screens/accounts/list.rs`, say).
- Whether `lead.rs` keeps its name once only the Now strip and the best pick use it
  (`lead::pick_words`, `lead::in_use`, `PICK_GAP`, `LEFT_LEAST`).
- Dropping the selected account's history read that fed the card's sparklines, if nothing else
  needs it.
- Where the before-tracking span test lives (an engine helper on `TokenRow`, or the TUI).
- The help wording's final shape, within §7.2's meaning.
- CHANGELOG `Unreleased` entries.
