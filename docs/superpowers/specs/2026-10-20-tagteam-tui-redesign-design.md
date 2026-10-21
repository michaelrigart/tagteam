# tagteam — TUI redesign: one timeline, a quiet screen that gets loud, plain words

**Status:** In progress — https://github.com/michaelrigart/tagteam/pull/29
**Date:** 2026-10-20
**Scope:** Redesigns `tagteam tui` from the 2026-10-19 UI/UX review, on branch `tui-redesign`.
Every VIS-n and UX-n finding of that review is assigned here (§17). So are the TUI items the
refactor spec held for this work (refactor §10) and hardening's deferred TUI rows. Beyond the TUI,
it changes the engine's shared words (so `list` reads the same), adds four kinds of stored event,
and adds two engine views.
**Builds on:**
- **TUI spec:** [`2026-10-14-tagteam-tui-design.md`](2026-10-14-tagteam-tui-design.md)
  (Implemented), cited as "TUI §n". It stays the authority for what this spec does not change.
  §18 lists the hunks this branch applies to it.
- **Core spec:** [`2026-09-26-tagteam-core-cli-design.md`](2026-09-26-tagteam-core-cli-design.md),
  cited as "core §n".
- **Daemon spec:** [`2026-10-14-tagteam-daemon-design.md`](2026-10-14-tagteam-daemon-design.md),
  cited as "D*n*".
- **Token report spec:** [`2026-10-14-tagteam-token-report-design.md`](2026-10-14-tagteam-token-report-design.md),
  cited as "TR §n".
- **Refactor spec:** `2026-10-19-tagteam-review-refactor-design.md` on branch `review-refactor`
  (Approved, `7b27210`), cited as "refactor §n". Its R2, R3 and R4 change APIs this spec builds
  on (§3.2).
- **Hardening spec:** `2026-10-19-tagteam-review-hardening-design.md` on branch
  `review-hardening` (`5c9a114`), cited as "hardening §n". It merges first.

**Review baseline.** The UI/UX review ran on `main` at `683b266`, the base of this branch. Its two
reports (`visual.md`: VIS-1..15, `interaction.md`: UX-1..16), its render gallery and its harness
patch sit in the main checkout's gitignored `.superpowers/research/tui-ux-review-2026-10-19/`, so
they are not part of this record. Every finding was re-verified at `683b266` before this spec was
written; §17 notes where a finding was wrong or incomplete. The facts this spec relies on are
restated here. Line numbers are as of `683b266`, and will drift.

---

## 1. Purpose

### 1.1 Why

Michael finds the TUI "far from user friendly nor beautiful", and wants real improvements, not
polish. The review agrees. The TUI is correct and carefully engineered, but it renders the CLI
faithfully and never answers the three questions a person opens it for:
1. Which account am I on, and how much is left?
2. Should I switch, and to what?
3. What did auto do, and why?

Today the active account is a one-character `*`, and the strongest thing on screen is a
reverse-video selection bar that inverts the data it highlights. Broken and stale accounts look
healthy. Auto's reasons last 60 s on one line, until the next poll overwrites them. The most
common action, a switch, ends in a modal. The `?` hint is gone at 80 columns. The words are
internal (`relogin required`, `pts/h (regression)`, `no-qualifying-candidate`).

### 1.2 Goals

1. **The three questions are answered at a glance, on every screen.** A header names auto's
   state and threshold. An auto line gives auto's last action in plain words. The account in use
   leads Accounts as the one bold element on screen: thick bars with pace marks, plus the best
   pick with its key. On the other screens a one-row Now strip carries it.
2. **One timeline explains auto.** History becomes a time-based screen: usage charts with auto's
   events (switches, pauses, failed switches, logins that need you) on the same time axis, a
   swimlane per account showing who carried the load, an event list synced to the range, and a
   time cursor that shows a switch beside its cause. It reads the store, so it works without the
   daemon.
3. **Quiet when fine, loud when not.** Healthy rows carry no colour beyond their gauges. Every
   problem gets colour, a glyph and words, and says what to do.
4. **One visual system.** Colour roles over three tiers: NO_COLOR, ANSI-16 (the default) and an
   optional truecolor tier. Selection never inverts data.
5. **Safe, discoverable keys.** `? keys` is always visible. Nothing changes state that the person
   didn't ask for: `R` confirms, and action keys are ignored briefly after a dialog closes.
6. **Successful actions don't interrupt.** A success is a toast; only failures, warnings and
   doctor open a panel.
7. **One plain vocabulary**, in the engine's shared words, so `list` reads the same as the TUI.
8. **The held items are closed** (§13), and the TUI's deferred performance rows are fixed (§14).
9. **TUI §1.1's criteria still hold**, the 50 ms p95 first frame included.

### 1.3 Non-goals

- **Mouse support, themes, custom bindings or `[tui]` settings** (TUI §1.2 stands). The
  truecolor tier is detection, not a theme: nothing configures it (§4.2).
- **Typed calls instead of argv** for actions. D-2 keeps argv; typed calls wait for the menu-bar
  spec.
- **Storing every auto decision.** No-switch outcomes stay live-only (§12.4).
- **A store migration.** New event kinds use the existing free-text `kind` column (§12.4).
- **Changing `--json` output** of any command.
- **Doctor's own texts.** Doctor keeps its checks and fix lines; the TUI only colours its glyphs.
- **Anything hardening or the refactor own**, except the call sites §3.2 names.

### 1.4 Anti-goals: failure even if it works

- **A screen that is louder than its problems.** If anything but a problem, the active account's
  bars or the selection marker draws the eye, the design failed.
- **A forecast presented as a promise.** "Best now" must come from the same code `b` runs (§12.1),
  and never be presented as what auto would do.
- **A layout that only works wide.** Every screen is designed at 80×24 first.
- **Colour-only meaning.** Every colour has a glyph or word twin, so NO_COLOR and colour-blind
  users lose nothing.
- **A second vocabulary.** The TUI and `list` never word the same state differently.
- **A slower start.** The first frame budget (TUI §1.1, 50 ms p95) holds, with no detection,
  timeline read or event query in front of it.
- **Real names in the record.** No real email or organisation name in this spec, tests, fixtures
  or committed images. The gallery's PNGs are never committed.

## 2. Decisions

**Michael's decisions (2026-10-20):**

| # | Question | Decision |
|---|---|---|
| M-1 | History or an Activity screen? | **Both, on one time-based screen**: charts carry auto's events as markers, an event list sits below synced to the range, a time cursor shows the values at a moment, events come from the store. A "Now" strip on every screen carries auto's last action, with a key to jump to it. Accounts keeps compact sparklines, and `Enter` opens the timeline filtered to the account. Fall back to four tabs only if 80×24 doesn't work. It works (§7.8), so there is no fallback |
| M-2 | Colour | **ANSI-16 stays the default, plus an optional truecolor tier** (selection tint, dialog surface) when detected. TUI §5.1 is amended |
| M-3 | Wording | **One plain vocabulary, in the shared engine words**, so `list` reads the same. TUI §5.2's "same words as `list`" is amended to mean words, not columns. Human output only; `--json` is unchanged |
| M-4 | Visual direction | **Direction A** of `visual.md`: quiet and borderless; `▌` and bold for the selection; a loud status column; the active account's bars with pace marks as the single bold element; "best now: N · b switch"; dialogs with a dimmed backdrop and a rounded frame |
| M-5 | Store pauses and failed switches as events, outside the TUI's files? | "Do what is best": **yes**, after refactor R4 (§12.4) |
| M-6 | (2026-10-21, T2 review) A late DA1 reply cut short is held whole by crossterm 0.29, Ctrl-C included. Fix it or accept it? | **Accept, and amend §4.2**: a known residual. No crossterm upgrade, no byte pre-reader |
| M-7 | (2026-10-21, T2 review) Before T4, should timer mode say `no switches in the last 30 days` without reading the store? | **Show nothing until T4**: rank 4's place stays empty in timer mode (§5.3) |
| M-8 | (2026-10-21, T3 review) Scheduled polls skip disabled accounts, so a disabled account's reading always goes stale. Should STATUS say `! old reading` or `○ disabled`? | **Disabled wins**: a disabled account's stale reading is not a problem, so STATUS says `○ disabled`. A real problem still wins (`✕ new login`), and UPDATED keeps the stale age in warn (§6.3) |

**Inherited rulings:**
- **D-2** (refactor §2): keep driving the CLI through argv, and fix its two string couplings with
  `Prompter::confirm_unlock` and warnings passed in via `Context` (§13).

**Calls made while drafting** (Michael checks these at sign-off):

| # | Call | Why |
|---|---|---|
| C-1 | Tabs stay `1 Accounts · 2 Tokens · 3 History`; History becomes the timeline | The name matches `tagteam history` and `tui history`; the screen still answers "what happened" |
| C-2 | The all-accounts chart plots the in-use account's **binding** usage (the higher of its relevant windows), not separate 5h and 7d charts | It is exactly what auto compares with its threshold, so a switch and its cause line up on one chart. Per-window charts are one `f` away |
| C-3 | A swimlane per account replaces a separate "in use" lane | It shows who carried the load and how close each got, from `active_history`, which the store already keeps |
| C-4 | `w` ("why") on every screen opens History with the cursor on the auto line's event | `w` is free on every screen; `h`/`l` stay free for vim users |
| C-5 | The auto line sits directly under the header on every screen | One place for "what auto did", next to auto's state; the footer stays keys only |
| C-6 | "Best now" is a new pure engine function shared with `switch --strategy best` (§12.1) | The TUI cannot reproduce the pick from `rank::best_order`: `switch` collects first, skips empty vaults and has two no-op rules (§12.1) |
| C-7 | The truecolor tier needs `COLORTERM` = `truecolor`/`24bit` **and** a background reported by OSC 11, queried with a DA1 sentinel. Use `terminal-colorsaurus` if it passes `cargo deny` and the musl static build and can meet §4.2's byte rules; otherwise hand-roll the query | A terminal that ignores OSC 11 still answers DA1 at once, so nothing waits on a timeout. The perf harness's pty sets no `COLORTERM`, so it never queries |
| C-8 | No pace mark on 5h windows | Core gives pace only to long and scoped windows with at least a day elapsed (`WindowKind::has_pace`, core `pace.rs:123-143`). The review's mock-up drew one on 5h; that was wrong |
| C-9 | Scoped and spend windows leave the Accounts table below 160 columns; they stay on the cards | They are `—` for most accounts and cost the names their width (VIS-4). `list` keeps its columns: parity is of words (M-3) |
| C-10 | Token counts keep the report's scaled form (`12.4M`); dollars and counts below scaling get thousands separators | Scaling is TR §11.2's rule; separators help only where digits are shown |
| C-11 | Events are not pruned | Auto writes at most a few rows an hour (§12.4); the table holds years at that rate |
| C-12 | A removed account's `remove` event stores its label in `detail` | Otherwise the timeline can only say "an account was removed": the row is gone (§12.4) |
| C-13 | The live `token-index` re-query throttle rises from 5 s to 30 s, except while the TUI's own refresh runs and after `r` | Each live session's heartbeat moves `changed_at` (PERF-D-2); hardening keeps the store side as is (H2 decision 3) |
| C-14 | Old switch events without a stored reason are worded from their trigger | Reconstructing the binding window from samples is guesswork for rows that predate §12.4 |
| C-15 | The shared words (§11.1) change in T3, with the Accounts screen, and T3 waits for R4 | T3's screens are written in the new words; shipping them earlier would either edit `usage_words` under R4 or make the TUI and `list` disagree until T5 |
| C-16 | Without a daemon, the header checks the engine lock: a foreground `tagteam auto` shows as `● auto runs in another terminal (pid N)` | `Engine::engine_holder` (`auto.rs:385`) reads it without trying the lock; "auto off" would be false while that runs. A cron `auto --once` between runs is genuinely off |
| C-17 | Under `autoswitch.strategy = consume-first` the header says `● auto on · uses the soonest reset first`, and switch sentences use the trigger's words, never the stored percentage | That strategy switches below the threshold (`autoswitch.rs:526-528`), so "switches at 90%" and "alex's 5h reached 40%" would mislead |

## 3. Constraints

### 3.1 Ownership

This branch owns `crates/tagteam-tui/**`, `crates/tagteam/src/tui_host.rs` and
`crates/tagteam/src/tui_cmd.rs`. It also edits, at the points named and nowhere else:
- `crates/tagteam-engine/src/words.rs` (§11), and the CLI renderers that print its words;
- `crates/tagteam-engine/src/views.rs` and the store's event and history readers (§12.2, §12.3);
- the switch pick, after R2 moves it to `switch/select.rs` (§12.1);
- the event writers in the auto engine, the daemon's provider job and lifecycle `remove` (§12.4);
- `crates/tagteam/src/prompt.rs` and the `Context` for D-2 (§13).

### 3.2 Order with hardening and the refactor

- **Hardening merges first.** Execution starts after it, rebased onto it.
- **The refactor ships as stacked PRs R1–R5** (refactor §4), rebased onto hardening. This
  branch's PRs (§15) rebase as each lands, and wait where they must:

  | This branch needs | Because | From |
  |---|---|---|
  | H10.2's window-key listing and per-window sample reads | `Engine::timeline` (§12.3) | hardening |
  | `engine_for`, `Failure::report`, `Io::warn`, `ColorEnv` | CC-O-2/3, D-2's warnings | R3 (§7.2–7.4) |
  | `Wire<T>`, `collects_for_itself()`, `waits_for_upgrade()`, the `poll`/`switch`/`no-switch` payload parsers | ARCH-3, CC-O-23f | R3 (§7.5) |
  | `switch/select.rs` with `best_pick` | `best_candidate` (§12.1) | R2 (§6.1) |
  | `EventKind`, `EventSource`, `EngineConfig.source` | the new kinds and writers (§12.4) | R4 (§8.7) |
  | `FailureKind` | the plain words for usage failures (§11) | R4 (§8.6) |

- **Where the refactor holds a file for this branch** (refactor §3.4), this branch adopts the
  refactor's API once it lands, then retypes (the `protocol.rs` `String` fields become `Wire<T>`
  in the PR that adopts R3).
- **Wording on top of R4.** R4 edits `usage_words` (`words.rs:125`). §11.1's changes land after
  R4, in T3, together with the screens that show them (C-15). T3, T4 and T5 therefore all wait
  for R4, and for what merges before it in the refactor's order (R1–R3). No two branches edit one
  function at once.

### 3.3 Size

GitHub's rebase merge refuses large branches, and the xreview pre-merge gate carries about 100 KB
of inline diff. Hence five stacked PRs (§15), each reviewed by Codex in slices where it exceeds
that.

### 3.4 Privacy

The repository is public. Fixtures, tests, this spec and any committed image use example names
only (`alex@example.com`, `sam@corp.example`, `Example Corp`). The review gallery's PNGs show
real names and are never committed. The gallery harness patch and the brainstorm mock-ups
(`ux_gallery.rs`, `ux_concept.rs`, `ux_redesign.rs`) stay uncommitted tooling: before/after
checks re-render locally into a scratch directory.

### 3.5 Verification

- The full suite on macOS and on Linux in Docker before each PR is reported done, totals as
  passed/total.
- `tests/perf.rs`'s first-frame p95 stays within 50 ms.
- Each PR's screens are re-rendered through the gallery harness (locally, uncommitted) at 80×24
  and 120×40 in ANSI dark, ANSI light, NO_COLOR and truecolor, and checked against this spec's
  mock-ups.

## 4. Visual system

### 4.1 Roles

A new `theme.rs` maps roles to `Style`s per tier. It replaces `bold()`, `selected()` and
`severity_style()` (`view.rs:469-486`) and is the only place outside tests that names a `Color` or
a `Modifier`.

| Role | Used for | Tier 1 (ANSI-16) | Tier 2 dark / light | Tier 0 (NO_COLOR) |
|---|---|---|---|---|
| `text` | values, names | default fg | default fg | plain |
| `muted` | labels, units, countdowns, secondary columns, key labels, hints | DIM | `#9da5b4` / `#57606a` | DIM |
| `faint` | rules, tracks, separators, borders, `—`; never text that must be read | DarkGray | `#4b5263` / `#c4c9cf` | DIM |
| `accent` | current tab, selection `▌`, `●`, key letters, normal fills | Cyan | `#56b6c2` / `#0a7d8c` | bold |
| `warn` | 70–89 %, `▲`, an old reading, retries, `≥` | Yellow | `#e5c07b` / `#9a6700` | bold |
| `crit` | ≥ 90 %, `✕`, errors, destructive actions | Red | `#f07178` / `#cf222e` | bold + underline |
| `ok` | `✓` only (toasts, doctor, "works again") | Green | `#98c379` / `#1a7f37` | plain, with `✓` |
| `sel-bg` | the selected row's background | none | `#2f3542` / `#e8edf2` | none |
| `surface` | a dialog's background | none | `#1f2329` / `#f6f8fa` | none |

- **Severity thresholds** stay core §13.5's: ≥ 70 warn, ≥ 90 crit.
- **Green never pairs with red** in one place, so red/green colour blindness cannot confuse a state.
- **Contrast.** The tier 2 hues are 4.5:1 or better on their background (`visual.md` §3.2's
  figures). Tier 1 follows the user's own theme.

### 4.2 Tiers and detection

- **Tier 0** when `--no-color`, `NO_COLOR` or `ui.color = never` say so, by today's precedence
  (`app.rs:647`, R3's `ColorEnv`).
- **Tier 1** otherwise. It must look right on its own; every mock-up marked "ANSI" uses only it.
- **Tier 2** when tier 1 applies, `COLORTERM` is `truecolor` or `24bit`, and the terminal reports
  its background:
  - **Query** OSC 11, then DA1, on the controlling terminal, right after raw mode is entered.
    Detection reads the terminal's bytes until the first of: the DA1 answer; 1 s in which no byte
    arrived; 2 s after it started; the shutdown token (TUI §2, §10.2). Detection itself never
    holds the keys for more than 2 s; a reply it cut off is handled below.
  - **Light or dark** from the reported background's relative luminance (≥ 0.5 is light).
  - **Fallback** to tier 1, silently, when there is no answer, under tmux without passthrough, or
    when the terminal is not the controlling terminal.
  - **Never configured** (TUI §1.2): no setting, no flag.
- **The first frame never waits for detection.** The first frame draws when TUI §6.1 says it
  does, in tier 1 if detection hasn't finished; when detection then reports a background, the
  next frame draws in tier 2. A local terminal answers in a few milliseconds, so in practice tier
  2 is known before the first draw. After detection, the tier does not change.
- **The answers never reach the keys.** The answer bytes (`ESC ] 11 ; rgb:…` contains `r`, `g`
  and `b`, and `b` switches accounts) must never be read as keys:
  - The input thread starts its first crossterm read only after detection has finished.
  - **Detection owns every byte it reads.** Bytes that are not part of a reply, a key typed in the
    few milliseconds before the DA1 answer, are dropped, never replayed: dropping a key can
    change nothing, while a replay would need a second key decoder beside crossterm's.
  - **A reply detection didn't finish** must not become keys either: one it stopped partway
    through (the 2 s deadline), or one that begins after it gave up (a terminal silent for over a
    second, then answering). Detection hands its parse state to the input thread. The input
    thread's filter drops a report it didn't ask for (an OSC 11 or DA1 reply, however crossterm
    surfaces it) whole.
  - **After a hand-off mid-reply the filter fails closed.** Silence cannot prove a reply has
    ended, so every key is dropped until that reply's own terminator arrives, however long that
    takes. The hand-off carries the reply's type, because the terminators differ: an OSC 11 reply
    ends with BEL or ST (`ESC \`), a DA1 reply (`ESC [ ? … c`) with its final byte `c`. Ctrl-C and Ctrl-Z still work, since no reply contains those bytes, and a
    warn banner says `! the terminal sent an unfinished colour report · keys resume when it ends ·
    Ctrl-C quits`. A key dropped this way is lost, never misread. The builder confirms how
    crossterm surfaces those bytes before choosing the filter.
  - **Known residual (M-6).** The terminal may stay silent past the give-up and then send a DA1
    reply cut short (`ESC [ ? 62` with no final `c`). crossterm 0.29 keeps that unfinished CSI in
    its own parser and holds every byte after it, Ctrl-C and Ctrl-Z included, until a `c` or `u`
    arrives. The filter never sees those keys, so no banner shows. They are lost, never misread.
    SIGTERM and SIGHUP still end the TUI, and typing `c` ends the held reply. T2 accepts this. It
    adds no crossterm upgrade and no byte pre-reader.
  - **Tests**, through a pty: a key before the OSC reply; a key between the two replies; a reply
    split across reads; a reply starting after the give-up; a truncated reply whose suffix arrives
    long after the deadline (its bytes, `b` included, never reach `update`; keys resume after its
    terminator); a DA1 reply cut off before its `c` (keys resume after the `c`); a truncated reply
    that never ends (Ctrl-C quits with exit 0, the terminal restored).

### 4.3 Emphasis and spacing

- **Bold is rare:** at most one bold per row. The active or selected account's name, TOTAL, a
  warn/crit percentage, a card or chart title. Column heads are muted capitals, never bold.
- **Severity** colours the percentage, the gauge fill and the status word, never a whole row. The
  one exception: a disabled row is entirely DIM.
- **Selection** is `▌` in accent in column 0, plus bold on the row's name; tier 2 adds `sel-bg`.
  REVERSED is used only on a small focused control under NO_COLOR (a dialog button) and on the
  timeline's cursor glyph (§7.5).
- **Numbers** right-aligned. Dollars and unscaled counts get thousands separators (`$1,541.64`,
  `9,891`); token counts keep TR §11.2's scaled form (C-10).
- **Columns:** column 0 is the selection marker; two spaces between columns, three between groups
  (identity | windows | meta).
- **Sections** are separated by a blank row and, where they need a name, a muted capital label
  (`SELECTED`). A faint rule goes under the chrome when the terminal is at least 30 rows tall, and
  above TOTAL.
- **Borders** only for what floats: dialogs and panels (§9.1). Screens are borderless.

### 4.4 Components

All glyphs are one column wide under `unicode-width` (East Asian ambiguous counts as narrow, as
the TUI already assumes for `─`, `█` and `●`), with no emoji presentation. `⏸`, `⚠` and `‼` are
not used.

| Component | Drawing |
|---|---|
| Lead bar (the bold element) | `▆` fill in the severity's role on a faint `▁` baseline; pace mark `│` (text inside the fill, muted outside); about 2 % per cell at 52 cells |
| Line gauge (tables, Now strip) | `━` fill and a `╸` half cell in the severity's role on a faint `─` track; pace mark `┿` inside the fill, `┼` outside; NO_COLOR track `·` |
| Share line (Tokens) | `━`/`╸` in accent, no track, relative to the largest row |
| Chart (History) | `▁▂▃▄▅▆▇█` stacked over N rows, one bucket per column, each column in its own value's severity; a faint `┈` (NO_COLOR `·`) at the threshold; an empty bucket stays blank |
| Sparkline (cards, Tokens headline) | `▁…█` in accent, one row |
| Swimlane (History) | one row per account: its binding usage as `▁…█` in the severity's role where it was in use, a faint `─` where it was not |
| Events lane (History) | one glyph per column (§7.4) |
| Markers | `●` in use · `▶` in a session · `○` disabled · `▲` faster than an even pace · `✕` needs you · `!` tagteam retries · `✓` done · `≥` at least · `~` estimated · `›` breadcrumb · `·` separator · `⏎` Enter · `⇄` switch · `‖` pause · `⠋⠙⠹…` spinner |

**The pace mark** sits where `Pace.expected_pct` says usage should be by now. A fill past it
means "faster than the window allows". Only long and scoped windows have one (C-8).

### 4.5 Width and height

| Width | What changes |
|---|---|
| 80 (minimum) | Everything works: short header forms (§5.2), 9-cell gauges, status as a glyph after UPDATED, help in one column |
| 100 | Accounts gets its STATUS column; help gets two columns |
| 120 | Wide forms: longer header words, gauges of 14, the Now strip's gauges, Tokens' cache columns and per-day sparkline, the History scope's range choices |
| 160 | Accounts gets a percentage column per scoped window that some account has, and SPEND when any account has spend |

- **Below 80×24** the body is a centred `Terminal 72×20 · tagteam needs 80×24` (UX-16).
- **From 30 rows** a faint rule sits under the chrome.

## 5. Chrome

### 5.1 Rows

From the top:
1. **Header** (§5.2), one row.
2. **Auto line** (§5.3), one row, in both modes.
3. **Now strip** (§5.4), one row, on Tokens and History. Accounts' lead card plays this role.
4. **Rule**, faint, when the terminal has at least 30 rows.
5. **Banner** (§5.6), 0–3 rows.
6. **Body.**
7. **Footer** (§5.5), one row, or up to three for a toast.

The rows above the body never change with the daemon's mode, so the body doesn't jump on connect
or disconnect (UX-5).

### 5.2 Header

` tagteam   1 Accounts   2 Tokens   3 History` on the left: the brand bold, the tabs muted with
the current one in accent and bold, no brackets. On the right, auto's state for the selected
account's provider, then `? keys`, which is never dropped.

| Daemon state (TUI §7.4) | 120 columns | 80 columns |
|---|---|---|
| `auto` | `● auto on · switches at 90%` | `● auto on · at 90%` |
| `auto`, strategy `consume-first` (C-17) | `● auto on · uses the soonest reset first` | `● auto on · soonest reset` |
| `collect`, `autoswitch` false | `‖ auto paused · P resumes` | `‖ paused · P resumes` |
| `collect`, `autoswitch` true | `○ auto idle · needs 2 usable accounts` | `○ needs 2 usable accounts` |
| `waiting` for `engine-lock` | `● auto runs in another terminal (pid 4711)` | `● auto in pid 4711` |
| `waiting` for `schema` | `! daemon is outdated · restarting` | `! daemon outdated` |
| `idle` | `○ auto off · no usable account` | `○ no usable account` |
| any, with `backoff` | adds ` · keychain locked until 15:05` in warn | `! keychain locked 15:05` replaces the state |
| no daemon, the engine lock held by a live `tagteam auto` (C-16) | `● auto runs in another terminal (pid 4711)` | `● auto in pid 4711` |
| no daemon, no live holder | `○ auto off · no daemon` | `○ auto off · no daemon` |
| `stopping` | `! daemon stopping (upgrade)` | `! daemon stopping` |
| protocol mismatch | `! daemon version unsupported` | `! daemon unsupported` |

- **Roles:** `●` accent, `‖` and `!` warn, `○` muted; the words in text; what follows `·` muted.
- **The threshold and strategy** are the effective `autoswitch.threshold` and
  `autoswitch.strategy` from the accounts view (§12.2), not from an event.
- **The engine lock** (timer mode only) is read with every `accounts` by `EngineSource`, through
  `Engine::engine_holder`, and judged live by an exact pid and start-time match, as `doctor` and
  the daemon's `collect` mode judge it.
- **"Live"** is no longer used for the daemon link: it also meant Claude Code's current login.
- **Mode changes toast once:** `! lost the daemon; tagteam updates on its own every 30 s` and
  `✓ connected to the daemon` (UX-5).

### 5.3 Auto line

One row: a glyph, a time, auto's last action in plain words, and on the right `w why`.

```
 ⇄ 11:28  auto switched 1 alex → 2 work · alex's 5h reached 93% · next check 14:16      w why
```

**What it shows**, first match wins:
1. A stored auto-line event of the last 30 minutes. The auto-line kinds are `switch`,
   `switch-recovered`, `switch-failed`, `all-exhausted`, `autoswitch-paused`,
   `autoswitch-resumed`, `quarantine` and `unquarantine`; `add`, `import` and `remove` are not.
2. In live mode, the daemon's latest `no-switch`, `error` or `config-warning`, in §11.3's words.
   A `poll` or `usage` event never replaces a line of rank 1 (UX-3).
3. The latest stored auto-line event of the last 30 days: `last switch Fri 15:38 · auto switched
   …`. Thirty days is History's longest range, so `w` can always show it.
4. `no switches in the last 30 days`.

- **`next check HH:MM`** from `status.nextTickAt`, in live mode with auto on, from 120 columns.
- **Account names** are `<position> <alias, else the email's local part>` (`2 work`, `1 alex`), as
  the notifications have them, resolved when read; the position keeps two `alex`es apart.
- **The time** is in the TUI's time zone, `HH:MM` today, `Fri 15:38` this week, `Oct 12 15:38`
  before that.
- **Roles:** the glyph in its event's role (§7.4), the time muted, the text in text, `w` accent.
- **`w`** opens History at its default scope with the cursor on this event (§7.5), widening the
  range to the shortest of 1, 7, 14 and 30 days that holds it. For a live-only line (rank 2), or
  rank 4, the cursor goes to now.
- **On start** the line fills from the store with the first `accounts` read (§12.2's
  `last_auto_event`), so it is never blank because the TUI just started (UX-3).
- **Before T4** (§15) the store's events are not read yet: the line takes ranks 2 and 4 only, plus
  the daemon's `status.lastSwitch` worded from its trigger (§11.3). T4 adds ranks 1 and 3.
- **In timer mode before T4** (M-7) the line stays empty in rank 4's place. Without the store, the
  TUI cannot know there were no switches. T4 brings rank 4 back there.

### 5.4 Now strip (Tokens and History)

```
 ● 2 work   5h ━━━━━─────  53% 2h20m   7d ━┼────────   6% 5d16h                best now 3 spare
```

- The account in use: `●`, position, alias or email (bold), then its short and long windows: a
  10-cell line gauge (from 120 columns), the percentage, and the time to reset.
- On the right, the best pick (§6.2), or `on the best account`, or nothing when there is no pick.
- A problem with the account in use replaces its windows with its STATUS phrase (§6.3), in its
  role: `● 2 work   ✕ needs a new login · see Accounts`.
- With no account in use: `no account in use · s on Accounts switches to one`.
- With several providers, the selected account's provider.

### 5.5 Footer and toasts

- **Keys** in accent, labels muted, ordered by how often they are used (§10). Items drop from the
  end. `? keys` and `q quit` are pinned on the right and never dropped (UX-4).
- **Labels follow state:** `e disable` or `e enable` by the selected row.
- **Hints that cannot apply** are hidden: no account keys with no accounts (VIS-10).
- **Toasts** replace the left side of the footer, and keep `? keys  q quit` on the right where
  they fit:

  | Kind | Glyph and role | Lasts |
  |---|---|---|
  | `Done` | `✓` ok | 5 s, 8 s when it wraps |
  | `Info` | none | 5 s |
  | `Warn` | `!` warn | 8 s |
  | `Failed` | `✕` crit | 8 s |

  `State::toast` takes the kind. A toast wraps to at most three rows (`WRAP_ROWS`), taken from the
  body.
- **The spinner** shows the running action's label (§9.4), in accent, and stays visible while a
  `busy` toast shows (it moves right of the toast).

### 5.6 Banner

Unchanged triggers (TUI §5.1, §10.4), restyled: crit for a failed read, warn otherwise, one column
in. New:
- **Start-up warnings** (UX-14) show as a warn banner, `! config: unknown key autoswitch.treshold
  (ignored)`, until the person presses `Esc` on Accounts or the next config change clears them.
- **An unmanaged live login:** `Claude Code is logged in as x, which tagteam doesn't manage ·
  press A to add it` (the provider's `display_name`, §12.2).

### 5.7 Empty and loading states

A centred block: a title, one sentence, the key to press (VIS-10, UX-15).

| State | Title | Sentence |
|---|---|---|
| Loading (after TUI §6.1's 100 ms) | `Loading accounts…` | none |
| No accounts | `No accounts yet` | `Log in with <launch command>, then press A to add that login.` |
| One account | (Accounts shows it normally; the header says `needs 2 usable accounts`) | under the table: `Add a second account to switch: log in to <display name> as it, then press A.` |
| Tokens, nothing indexed | `No token usage yet` | `tagteam reads <display name>'s transcripts; they appear here as you use it.` |
| History, nothing in range | `Nothing in the last 7 days` | `[ ] picks a longer range.` |
| A failed Tokens read | the last report stays, DIM, with `! couldn't read the token index (<error>) · r retries` above it | |

## 6. Accounts

### 6.1 Layout

```
  ● 2 work  sam@corp.example · Example Corp    in use · login valid 6 more days   best now 3 spare   b switch
    5h    ▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▆▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁   53%  resets in 2h20m · +15.7%/h · lasts until it resets
    7d    ▆▆▆│▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁   6%  resets in 5d16h · lasts until it resets

    #  ACCOUNT                           5H                    7D                    UPDATED   STATUS
    1   alex@example.com                 ──────────────   0%   ━━━━━━━━┿─────  54%        2m
    2 ● work   sam@corp.example          ━━━━━━━───────  53%   ━┼────────────   6%        1m   in use
 ▌  3   spare  alex@personal.example     ──────────────   0%   ━━━━━───┼────  31%       14m   best

    3 spare  alex@personal.example    not in use · updated 14m ago · next update in 2m
    5h    ▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁ 24h       0%  not started
    7d    ▂▂▂▃▃▃▃▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄ 7d       31%  resets in 1d06h · lasts until it resets
    ⏎ history of spare   e disable   a alias   x remove
```

From the top: the lead card (§6.2), a blank row, the table (§6.3), a blank row, the selected
account's card (§6.4). Content anchors to the top; spare rows go to the bottom (VIS-5).

### 6.2 Lead card and the best pick

- **The account in use**, from `AccountView.active`: `●`, position, alias bold and email muted
  (or the email bold), the org muted from 120 columns, then its facts muted (`in use · login valid
  6 more days`, warn under 7 days with `· log in again before then`).
- **Its windows as lead bars** (§4.4): 52 cells at 120 columns, 30 at 80. At 80 columns only the
  short and long windows; from 120 every window, spend included as `€12.40 of €50`.
- **Beside each bar:** the percentage (bold from 70 %), `▲` in warn when ahead of pace, the reset,
  and the projection in §11.1's words, in warn when the window fills before it resets. At 80
  columns the projection is its last clause (`lasts`, `full in 2d04h`).
- **A problem with the account in use** (quarantined, expired, an old reading, a retry) shows
  under its bars as on the selected card (§6.4): the problem sentence and the remedy, in their
  role. The table's STATUS shows it too, so it is visible whatever is selected.
- **With no account in use:** the card says `No account in use` and `s switches to the selected
  account`; with an unmanaged live login, the banner explains it (§5.6).
- **The best pick** on the title row's right: `best now  3 spare   b switch`. It is §12.1's
  `best_candidate` over the view the screen shows:

  | Forecast | Shows |
  |---|---|
  | `Pick` | `best now  <position> <alias or email>   b switch` |
  | `AlreadyBest` | `on the best account` (muted) |
  | `None(UsageUnknown)` | `b checks usage first, then picks` (muted) |
  | any other `None` | nothing |

  It is never called auto's pick: auto ranks differently (§12.1). After `b`, the toast reports the
  CLI's real outcome.

### 6.3 Table

- **Columns,** by priority when the width runs short (VIS-4): selection marker; position; marker
  (`●` in use, `▶` in a session, `○` disabled); name; 5H; 7D; UPDATED; STATUS (from 100 columns);
  scoped and SPEND (from 160 columns, C-9).
- **The name** is the alias bold-if-selected and the email muted, or the email. The org is a muted
  suffix outside the name's budget, shown only where it fits. The name keeps at least 16 columns
  before anything else shrinks.
- **5H and 7D** are line gauges with the percentage and `▲`, 14 cells from 120 columns, 9 at 80.
  When the width runs short a gauge shrinks to 6 cells, then to the percentage alone, per column,
  not for the whole table (VIS-4).
- **UPDATED** is the reading's age, muted; in warn when the reading is not decision-grade (an old
  reading).
- **STATUS**, one short phrase: the first row of this table that applies. Problems come before
  `in use` and `best`, because the `●` marker already shows the account in use:

  | State | Phrase | Role |
  |---|---|---|
  | needs a new login / login expired / no stored login | `✕ new login` / `✕ expired` / `✕ no login` | crit |
  | rate-limited / keychain / budget / other retry | `! rate-limited` / `! keychain` / `! budget` / `! retrying` | warn |
  | old reading | `! old reading` | warn |
  | disabled | `○ disabled` (row DIM) | muted |
  | in use | `in use` | accent |
  | best pick (§6.2) | `best` | text |
  | API key | `api key` | muted |

  A disabled account's stale reading is not a problem (M-8): scheduled polls skip disabled
  accounts, so such a row says `○ disabled`, its UPDATED still in warn.

  At 80 columns the phrase's glyph follows UPDATED.
- **A row with no reading** shows its status words across the window columns:
  `✕ needs a new login`, `api key · has no usage limits to show`, `! can't read the Keychain ·
  retry in 2m`.
- **Several providers** group under muted provider headings, as `list` does.
- **In a run shell** the session's own row shows `▶ this session`.

### 6.4 Selected card

- **The title:** position, alias bold and email muted, the org from 120 columns, then the facts
  muted (`not in use · updated 14m ago · next update in 2m`, `· login valid 27 more days`).
- **A row per window** (all of them): the label, a sparkline (32 cells from 120 columns, 16 at
  80; the last 24 hours for short windows, 7 days for the others, tagged `24h`/`7d` in faint),
  then the percentage, `▲`, the reset and the projection, as on the lead card.
- **Problems,** under the windows, in their role, from §11.2: the problem in one sentence, then
  the remedy.
  ```
  ✕ Needs a new login: its stored login was refused (invalid_grant), Sat 09:02.
    Log in to Claude Code as contractor@example.com, then press A here.
  ```
- **The keys for this account** on the last row: `⏎ history of spare   e disable   a alias
  x remove` (from 120 columns; at 80, help lists them).
- **The sparklines** come from one `history` read for the selected account (TUI §4.4), on
  selection change, debounced to 250 ms, on the views lane.
- **When the selected account is the one in use,** the card shows the sparklines and the facts but
  not the reset and projection, which the lead card already shows.

### 6.5 Behaviour

- **Selection starts on the account in use**, else row 1 (UX-2). It survives refreshes by account
  ID, as today.
- **`⏎`** opens History scoped to the selected account (§7.2), and the History scope and the
  Accounts selection stay in sync both ways (UX-11).
- **`J`/`K`** scroll the selected card when it doesn't fit.
- **`g`/`G` are dropped**; `Home`/`End` stay (UX-7).

## 7. History: the timeline

### 7.1 Layout

```
 f all accounts   [ ] 7 days   1 · 7 · 14 · 30                                       cursor Mon 11:28

 in use   the higher of its 5h and 7d: what auto compares with 90%       at cursor  90% →   3%   now  53%
    100  ┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈
         (chart, as tall as the body allows)
      0
          ⇄     ⇄ ⇄ ⇄         ⇄      ‖┄‖⇄      ✕ ⇄      ✕✓                                       ⇄
 1 alex  ▆▇─────────────────────────────▅▅▅▁▁▁▄█─▂▆▆▃▃▃▃▃▃▃▃▄▃▃▃▃▃▃▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▇─
 2 work  ─▄▄▄▁▁▁▄─▄▅▇─────────▅▅▅▃▃▃▃▇█▇▇──────────────────────────────────────────────────────────▂▅
 3 spare ───────▂▆▇─▃▃▃▁▁▁▄▆▇▇▇────────────────▂▄▇──────────────────────────────────────────────────│─
             ╵Tue      ╵Wed      ╵Thu      ╵Fri      ╵Sat      ╵Sun                              now

   WHEN        WHAT
▌  Mon 11:28   ⇄ auto switched 1 alex → 2 work · alex's 5h reached 93%
   Sat 11:33   ✓ 3 spare works again · new login stored
   Sat 08:58   ✕ 3 spare needs a new login · its stored login was refused (invalid_grant)
   Fri 15:38   ⇄ auto switched 3 spare → 1 alex · spare's 5h reached 91%
```

From the top: the scope row (§7.2), the chart or charts (§7.3), the events lane (§7.4), the
swimlanes or the in-use lane (§7.4), the day axis, then the event list (§7.6).

### 7.2 Scope and range

- **Scope:** all accounts (the default), or one account. `f` opens a choose dialog listing `all
  accounts` and each account; `⏎` on Accounts opens History scoped to the selected one.
- **Range:** `[`/`]` cycle 1, 7, 14 and 30 days; the default is 7, as for `history`. From 120
  columns the choices show muted beside the current one.
- **The cursor's time** on the scope row's right, bold.

### 7.3 Charts

- **All accounts:** one chart, titled `in use`, of the in-use account's binding usage per column
  (the higher of its relevant windows, `rank::binding_window`), C-2. A column spanning a switch
  shows its peak, so the climb to the threshold and the drop after it are both visible.
- **One account:** a chart per relevant window (`Engine::history`'s windows, core §13.4), titled by
  window label. At 80×24 the short and long windows show; `J`/`K` scroll to the rest.
- **Every chart:**
  - a muted y axis (`100`, `50` from 6 rows, `0`) in an 8-column gutter;
  - a faint `┈` at the threshold (all accounts) or at 90 % (one account);
  - columns in their own value's severity;
  - in the cursor's column, a `│` in accent through empty cells;
  - on the title row's right: `at cursor <before> → <after>` for a switch at the cursor (the
    binding usage just before and just after it), `at cursor <value>` otherwise, then `now <value>`.
- **Height:** charts share what the event list leaves (§7.8), at most 12 rows (all accounts) or 8
  (each of several).

### 7.4 Lanes

**The events lane** sits under the charts: one glyph per column, the most severe event in that
column winning.

| Event (§12.4) | Glyph | Role | Rank |
|---|---|---|---|
| `switch-failed`, `all-exhausted`, `quarantine` | `✕` | crit | 6 |
| `autoswitch-paused` (and the span until resumed: `┄`, NO_COLOR `·`) | `‖` | warn | 5 |
| `switch` by auto or the daemon | `⇄` | accent | 4 |
| `switch` by the CLI or the TUI, `switch-recovered` | `⇄` | text | 3 |
| `autoswitch-resumed` | `‖` | muted | 2 |
| `unquarantine`, `add`, `import`, `remove` | `✓`, `+`, `+`, `-` | ok, muted | 1 |

The cursor's event glyph is bold and REVERSED; with no event at the cursor, the lane shows `│` in
accent.

**Swimlanes** (all accounts): one row per account, named in the gutter (`1 alex`). Each column is
the account's binding usage as one eighth-block, in its severity where the account was in use in
that column (from `active_history`), a faint `─` where it was not. Accounts never in use in the
range are left out; at most 6 at 80×24 and 10 at 120×40, then `+2 more · f picks one` muted.

**In-use lane** (one account): `━` in accent where it was in use, faint `─` elsewhere.

**The axis:** `╵Tue` at each day's first column, `now` at the right, muted. For 1 day, hours
(`╵09`, `╵12`); for 30 days, dates every few days (`╵Oct 6`).

### 7.5 Cursor

- **`←`/`→`** move the cursor one column; **`j`/`k`** (and `↑`/`↓`) move it to the next or previous
  event in the list. The list's selection and the cursor are one thing: moving one moves the other
  (to the nearest event at or before the cursor).
- **`Home`/`End`:** the oldest event, now.
- **On open:** the cursor is on the newest event of the auto line's kinds (§5.3); through `w`, on
  the auto line's event; with no events, at now.
- **Readouts** on the chart titles (§7.3) and the scope row.

### 7.6 Event list

- **Rows:** the selection marker, the time (muted; §5.3's form), the event's glyph in its role,
  then its sentence (§11.4). Newest first. Only events in the range and the scope (one account:
  events naming it, plus pauses and resumes).
- **The selected row** is bold, with `sel-bg` in tier 2.
- **Overflow:** `⋮ 9 more · z full list` muted on the last row.
- **`z`** toggles the list between its share of the body and the whole body (charts hidden), for
  80×24 and for reading long ranges.
- **A muted head** `WHEN  WHAT` from 120 columns.

### 7.7 Data and refresh

- **One read:** `Engine::timeline(provider, scope, since_s, columns)` (§12.3) on the views lane,
  returning bucketed series, swimlanes and events, so the UI thread only draws.
- **Re-read on:** opening the screen, a scope or range change, a resize (debounced to 250 ms), a
  `switch`, `account-quarantined`, `account-unquarantined`, `usage` or `changed` event (debounced
  as today), and the 30 s timer in timer mode. In live mode, at most every 30 s for `usage` and
  `changed`.
- **It never fetches** (core §13.4).
- **Without a daemon** everything above still shows: the events and `active_history` are the
  store's.

### 7.8 80×24

The render in `ux_redesign.rs` (uncommitted, §3.4) shows it fits: the header, the auto line and
the Now strip take three rows and the footer one, which leaves 20 for the body. The scope row 1,
the chart's title 1 and its 9 rows, the events lane 1, three swimlanes 3, the axis 1 and the event
list 4 (three events and the overflow line). With more accounts the swimlanes take rows from the
chart down to 5 rows, then cap (§7.4). `z` gives the list the whole body. So there is no fallback
to four tabs (M-1).

## 8. Tokens

Behaviour stays TUI §5.3's and §6.4's; the screen changes:
- **Chrome:** the Now strip (§5.4) under the auto line.
- **Control bar** (VIS-8): one row, `p Day  Month  Session  5h block     [ ] 30 days     g by
  account     f all accounts · all models`, each key in accent, the current choice in text, the
  others muted, and `indexed 14:12` on the right. It replaces the bracketed second tab row.
- **Headline** (VIS-5, VIS-9): `$933.44 at API prices    1.7B tokens    9,891 responses`, then from
  120 columns `today $129.11` and `per day` with a sparkline of cost per day in accent. With
  period `5h block`, the active block's progress line takes the headline's right side (VIS-14):
  `▶ block ━━━━━━━━━━━━━━──────── 2h12m left · 1.2K tok/min · $4.10/h · ≈ $38 at its end`.
- **Table typography:**
  - TOTAL and `API $` (was `COST`) in text, the other numbers muted, `≥` cells in warn.
  - Heads `CACHE W` and `CACHE R`, with the 5-minute and 1-hour split as `CACHE W 5M` and `1H`
    only from 160 columns; help explains them.
  - Thousands separators on dollars and responses (C-10).
  - Model IDs short in the table and full in the detail. The short form drops the prefix every
    model in the report shares and a trailing date stamp (`claude-opus-4-1-20250805` →
    `opus-4-1`), and keeps the full ID where that would make two models look alike. The rule
    knows no provider's naming (ARCH-1).
  - A session's project as its basename (`portal-api`), which outranks the period's long form when
    the width runs short.
  - Estimated blocks marked `~` before their start time, in place of the `observed`/`inferred`
    column; help explains it.
  - A removed account as `removed account` and an unattributed row as `unknown account`; IDs in
    breadcrumbs and details as their first 8 characters (VIS-15).
- **Labelled sections:** `SELECTED` with the selected row's detail and `⏎ its sessions`, then the
  notes, each led by its mark (`≥ at least: 3 subagent responses are still being counted`, `$ what
  this would cost on the API at list prices of 2026-10-01; not your bill`).
- **Breadcrumbs** muted, joined by `›`; the filter keeps its room before them (VIS-15).
- **A failed re-query** keeps the last report, DIM, under the error banner (VIS-10). Today
  `TokensState::reset()` drops it before the read (`tokens/mod.rs:294-298`); the reset keeps the
  last report until the new one arrives.

## 9. Overlays and actions

### 9.1 Dialogs and panels

- **Backdrop:** the whole buffer is set DIM before an overlay draws (VIS-7).
- **Frame:** rounded, faint; the title in accent and bold (crit for a destructive action); 2
  columns and 1 row of padding; the keys in the bottom border, right-aligned; `surface` in tier 2.
- **Buttons:** the focused one in accent fill (tier 0: REVERSED); a destructive one in crit.
- **Panels** (result, doctor, help) cover the body completely: no slivers of the screen at their
  edges.
- **Doctor** colours lines by their first glyph: `✓` ok, `!` warn, `✗` crit (the text is the
  CLI's).
- **`q`** closes an open panel or dialog first and quits only when nothing is open (UX-16).

### 9.2 Help

- **Two sections** for the current screen and for every screen, then **what the marks mean**: a
  legend of every glyph and colour in §4.4, each with its words (UX-4, UX-13).
- **Two columns** from 100 columns wide, one below that, scrollable.
- **One key table** generates both the help and the footer, and a test checks that every listed
  key is handled (CC-O-9).

### 9.3 Outcomes

The classification in `on_done` (`actions.rs:579-632`) becomes, in this order (UX-1):

| Outcome | Shows |
|---|---|
| `NeedsTerminal` | the hand-over dialog (§9.6) |
| doctor | the doctor panel, whatever the exit code |
| exit 0 and no stderr after D-2's filtering (§13) | a `Done` toast of the CLI's stdout, lines joined by a space, when it fits the toast's three rows; else the result panel |
| exit 0 with stderr | a `Warn` toast when it is one line, else the result panel |
| non-zero exit | a `Failed` toast when the text is one line, else the result panel |
| nothing printed | a `Done` or `Failed` toast from the exit code: `switch finished` / `switch failed (exit 1)` |

The text is the CLI's, unchanged (TUI §1.1 criterion 5). Each action is still followed by a
re-read of the views.

### 9.4 Labels, spinner and busy

- **`Action::label()`** gives the UI's words beside the CLI's `name()` (UX-8):
  `switching to 3 spare…`, `switching to the best account…`, `switching to the next account…`,
  `disabling 2 work…`, `setting the alias…`, `moving 2 work…`, `removing 3 spare…`, `adding
  Claude Code's login…`, `running doctor…`, `pausing auto…`, `resuming auto…`.
- **The `P` outcome** toasts `Auto paused for Claude Code. P resumes.` (or `Auto resumed.`), in
  place of `config set`'s line, which names a file path. The provider's `display_name`, never its
  ID.
- **A second action key** while one runs toasts `still switching to 3 spare · wait for it to
  finish`, and the spinner stays.

### 9.5 Key guards

- **`R`** opens a confirm: `Start Claude Code as 2 work in this terminal?` and `tagteam steps aside
  until the session ends, then comes back.`; `⏎` starts, `Esc` cancels (UX-7).
- **For 300 ms after any dialog or panel closes,** the action keys (`s b n e a m x R P A D r`) are
  ignored, so a repeated `n` cannot switch the live login (UX-7).
- **`x`** on the account in use confirms as for any other: the CLI does not refuse it (§17 notes
  the review's error).
- **`m`** asks `Move 2 work to position (1–4):` and refuses out-of-range input in the dialog
  (UX-16).

### 9.6 Handing the terminal over

- **Before a suspended action** that is not `run`, a dialog: `Your login keychain is locked.
  tagteam hands you the terminal to unlock it, then comes back.` `⏎ continue · Esc cancel`
  (UX-10).
- **On return,** a toast from the exit code: `✓ switch finished`, or `✕ switch failed · its output
  is above in your terminal's scrollback`. For `R`: `session on 2 work ended`.

### 9.7 `r`

- **A spinner** `checking usage…` while the collection runs (UX-6).
- **Then a toast:** `✓ updated 2 accounts`, `already current · next update in 1m`, or
  `! 1 account rate-limited · retry in 4m`. In live mode the same, for the accounts the daemon
  doesn't collect.

## 10. Keys

| Where | Key | Does | Footer (80 columns) |
|---|---|---|---|
| Everywhere | `1` `2` `3`, `Tab`/`Shift-Tab` | Accounts, Tokens, History | — |
| Everywhere | `?` | keys and marks (§9.2) | pinned |
| Everywhere | `q` | close the open overlay, else quit | pinned |
| Everywhere | `Esc` | close the overlay; else back one level (Tokens drill-down; History opened from Accounts returns there; a History scope returns to all accounts) | — |
| Everywhere | `w` | History at auto's last action (§5.3) | — |
| Everywhere | `P` | pause or resume auto (confirm); without a daemon, says how to install it | Accounts |
| Everywhere | `r` | update usage now (Tokens: the index), with feedback (§9.7) | yes |
| Everywhere | `A`, `D` | add Claude Code's current login, doctor | — |
| Everywhere | `Ctrl-C`, `Ctrl-Z` | quit, suspend | — |
| Accounts | `j` `k` `↑` `↓` `PgUp` `PgDn` `Home` `End` | move | — |
| Accounts | `s` | switch to the selected account | yes |
| Accounts | `b` | switch to the best account (the lead card names it) | yes |
| Accounts | `n` | switch to the next account | yes |
| Accounts | `⏎` | its history | yes |
| Accounts | `J` `K` | scroll the selected card | — |
| Accounts | `e` | disable / enable (label follows the row) | from 120 |
| Accounts | `a` `m` `x` | alias, move, remove (confirm) | from 120 |
| Accounts | `R` | start Claude Code as it, here (confirm) | — |
| Tokens | `p` `g` `f` `[` `]` | period, group, filter, range | yes |
| Tokens | `⏎`, `Esc`/`Backspace` | drill down, back | yes |
| History | `j` `k` `↑` `↓` | previous or next event | yes |
| History | `←` `→`, `Home` `End` | the cursor by column; oldest, now | yes |
| History | `[` `]` | range | yes |
| History | `f` | scope | yes |
| History | `z` | full-height list | yes |
| History | `J` `K` | scroll charts (one account at 80×24) | — |

Accounts' footer order: `s switch  b best  n next  ⏎ history  P pause auto  r refresh`, then from
120 columns `e disable  a alias  x remove`.

## 11. Words

All of this is human output. `--json` does not change anywhere.

### 11.1 The shared vocabulary

These live in `tagteam-engine/src/words.rs` and change `list`, `status`, `history` and `auto`'s
human output with the TUI (M-3). The TUI-only words of §11.3–§11.4 live in the same module, so the
CLI can adopt them.

| Today | Plain | Where it shows |
|---|---|---|
| `relogin required` | `needs a new login` | `list`, TUI |
| `token expired` | `login expired` | `list`, TUI |
| `keychain unavailable` | `can't read the Keychain` | `list`, TUI |
| `foreign credential` | `logged in as another account` | `list`, TUI |
| `no credentials` | `no stored login` | `list`, TUI |
| `unavailable (http-429, retry 5m)` | `rate-limited (retry 5m)` | `list`, TUI |
| `unavailable (<kind>)` | the `FailureKind`'s words (R4 §8.6), for example `usage check failed (refresh-failed)` | `list`, TUI |
| `over budget` | `paused to stay within the request budget` | `list`, TUI |
| `no data yet` / `never fetched` | `not checked yet` | `list`, TUI |
| `▲ pace` | `▲ fast` | `list` |
| `▲ ahead of pace (expected 62%)` | `▲ faster than an even pace (62% expected by now)` | TUI detail |
| `+1.2 pts/h` | `+1.2%/h` | `history`, TUI |
| `(regression)`, `(average)` | dropped; help names the method | `history`, TUI |
| `runs out in 2d04h` | `full in 2d04h` | `history`, TUI |
| `lasts to reset` | `lasts until it resets` | `history`, TUI |
| `no rate yet` | `not enough data yet` | `history`, TUI |
| `should have run out 3h ago` | `should have been full 3h ago` | `history`, TUI |
| `won't run out within a year` | `won't fill within a year` | `history`, TUI |
| `AGE` (head) / `fetched 2m ago · next poll in 1m` | `UPDATED` / `updated 2m ago · next update in 1m` | `list`, TUI |
| `login expires in 6d03h` | `login valid 6 more days` | TUI |

- **Unchanged:** `api key`, `disabled`, `▶`, `▶ this`, `—`, the countdown form (`3d09h`), the
  money form, `scaled`, the token report's footnotes other than §8's.
- **R4 first** (§3.2). The usage-failure words key on R4's `FailureKind`, not on strings.

### 11.2 Remedies

`words::remedy(status, &AccountView, &ProviderNames) -> Option<String>`, shown under a problem on
the TUI's cards (UX-9):

| Status | Remedy |
|---|---|
| needs a new login (quarantined), login expired, no stored login | `Log in to <display name> as <email>, then press A here.` |
| can't read the Keychain | `Unlock it (security unlock-keychain), then press r.` |
| rate-limited, or any other usage failure with a retry | `tagteam tries again in 4m.`; the problem sentence before it names the failure in its `FailureKind`'s words (R4 §8.6) |
| paused for the budget | `tagteam checks again in 12m, when the hourly budget allows.` |
| logged in as another account | `<display name> is logged in as a different account. Switch with s, or log in again.` |
| not checked yet | `tagteam checks it within 3m.` |

The problem sentence before it names the cause: `Needs a new login: its stored login was refused
(invalid_grant), Sat 09:02.` The provider's display name comes from the provider (ARCH-1, §12.2).

### 11.3 Auto's decisions

`words::no_switch(reason, detail, threshold)` for the 17 `NoSwitchReason`s, used by the auto line
(§5.3) and by `tagteam auto`'s human output in place of `no switch: <code>`:

| Code | Sentence |
|---|---|
| `below-threshold` | `no switch needed (77%, switches at 90%)` |
| `cooldown` | `waiting 4m after the last switch` |
| `all-exhausted` | `every account is near its limit; the first frees up at 16:40` |
| `no-qualifying-candidate` | `no other account has enough headroom` |
| `no-candidates` | `no other account can be used` |
| `no-comparison` | `no other account has a reading to compare` |
| `stale-usage` | `the best account's reading is too old; checking it first` |
| `active-usage-unknown` | `can't read the account in use (2 of 3 tries)` |
| `unmanaged-active-account` | `Claude Code is logged in as an account tagteam doesn't manage` |
| `no-active-account` | `Claude Code isn't logged in` |
| `active-api-key` | `the account in use is an API key, which auto leaves alone` |
| `reset-unknown` | `can't tell when the account in use resets` |
| `already-consuming-soonest` | `already on the account that resets first` |
| `engine-running` | `auto already runs in another process` |
| `interrupted-switch` | `finishing an interrupted switch first` |
| `live-changed` | `the login changed meanwhile; trying again next check` |
| `no-viable-target` | `every target failed; trying again next check` |

"Claude Code" in these comes from the provider's `display_name`.

**Triggers** (`events.trigger`, and the daemon's `lastSwitch.trigger`), for switches that predate
§12.4's `detail` (C-14):

| Trigger | Words |
|---|---|
| `proactive` | `before it reached the limit` |
| `at-limit` | `at the limit` |
| `failover` | `its usage couldn't be read` |
| `consume-first` | `to use up the account that resets first` |

### 11.4 Events

`words::event(&EventView) -> String`, the timeline's and the auto line's sentences:

| Event | Sentence |
|---|---|
| `switch`, by auto or the daemon, trigger `proactive` or `at-limit`, with `detail` | `auto switched 1 alex → 2 work · alex's 5h reached 93%` |
| `switch`, by auto or the daemon, any other trigger or no `detail` | `auto switched 1 alex → 2 work (to use up the account that resets first)`, from §11.3's trigger words (C-14, C-17) |
| `switch`, by the CLI or the TUI | `you switched 2 work → 3 spare` |
| `switch-recovered` | `tagteam finished an interrupted switch to 2 work` |
| `switch-failed` | `auto couldn't switch to 3 spare · the Keychain was locked` (its message) |
| `all-exhausted` | `auto couldn't switch · every account is near its limit; the first frees up at 16:40` |
| `autoswitch-paused` / `-resumed` | `auto paused` / `auto resumed` |
| `quarantine` | `3 spare needs a new login · its stored login was refused (invalid_grant)` |
| `unquarantine` | `3 spare works again · new login stored` (`credentials-replaced`) / `· account replaced` |
| `add` / `import` / `remove` | `4 ci added` / `4 ci imported` / `ci@example.com removed` |

Names are `<position> <alias, else the email's local part>` as the account reads now; a removed
account by the label in its `remove` event (C-12), else `an account`.

## 12. Engine, store and daemon changes

### 12.1 `best_candidate` (after R2)

**Why.** `switch --strategy best` (`switch.rs:851-1007`) does more than `rank::best_order`:
- it collects usage first (`prepare_usage`, `:1599-1630`);
- its candidates pass `switch_candidate` (`:732`: enabled, not quarantined, an identity, not
  session-owned);
- it returns no pick when fewer than two candidates qualify (`:759`), counting the live account,
  and when the live login is unmanaged and `--force` is not given (`:1204-1211`);
- with a managed live login of unknown headroom it drops candidates with headroom ≤ 0;
- it walks vaults, skipping a candidate with no stored login (`walk`, `:791`).

The TUI can reproduce none of that from `rank` alone. Auto ranks differently again
(`autoswitch.rs`: hysteresis, recovery axis, API-key exclusion), so "best now" is never auto's
pick.

**Change.** A pure function in `switch/select.rs`, holding the whole pre-vault rule in this order:
the unmanaged / no-login branch, the fewer-than-two rule, `switch_candidate` over the row and the
session test, decision-grade windows only, `best_order`, and the unknown-live `> 0` filter.

```rust
pub struct PickInput {
    pub live: LiveLogin,                 // None, Unmanaged, Managed { position }
    pub force: bool,
    pub rows: Vec<PickRow>,              // every account of the provider, the live one included
}
pub struct PickRow {
    pub position: u32,
    pub candidate: bool,                 // switch_candidate: enabled, not quarantined, identity, not session-owned
    pub headroom: Option<f64>,           // from decision-grade, relevant windows only; else None
}

pub fn best_candidate(input: &PickInput) -> BestForecast;

pub enum BestForecast {
    /// Most headroom first: the first is the pick, the rest are what the vault walk tries next.
    Pick { order: Vec<u32> },
    /// The account in use has the most headroom.
    AlreadyBest,
    None(NoPick),   // UnmanagedLogin, TooFewCandidates, UsageUnknown, CandidatesExhausted
}
```

- **Two callers, one input type.** `best_pick` builds `PickInput` from the rows and rated windows
  it already reads, after `prepare_usage` has collected, then walks the vaults as today; its
  outcomes and messages do not change. The accounts view builds the same input from its
  `AccountView`s (`usage.decision_grade`, `ProviderAccounts.models`, `live`) without I/O.
- **Only decision-grade readings rank,** as in the switch. When none do, the forecast is
  `None(UsageUnknown)`, and the lead card says `b checks usage first, then picks` (§6.2): `b`
  collects before it picks.
- **The vault walk stays private to the switch.** The TUI's forecast assumes vaults hold logins;
  the toast after `b` is the truth.
- **R4's `is_switchable()`** is used where it applies; the session-owned test stays a second
  layer, as `switch_candidate` has it.

**Test.** A parity test runs `best_candidate` and `switch --strategy best` (with collection
stubbed) over each case where they could differ: the fewer-than-two quirk with a live account
that is itself excluded, an unmanaged live login, no live login with negative headroom, unknown
live headroom, a stale reading, and a tie (lower position wins).

### 12.2 `ProviderAccounts` additions

`ProviderAccounts` (`views.rs:125`) gains what the screens need without guessing:

| Field | Type | Why | PR |
|---|---|---|---|
| `threshold`, `strategy` | `f64`, `Strategy` | the header's `switches at 90%` or `uses the soonest reset first` (§5.2): the effective `autoswitch.*` settings | T2 |
| `names` | `ProviderNames { display_name, launch_command }` | ARCH-1: the TUI's two strings, the empty states and the remedies come from the provider | T2 |
| `models` | `Vec<String>` | the relevant-window rule (`usage::is_relevant`) for `binding_window` and `best_candidate` | T3 |
| `live` | `LiveLogin { None, Unmanaged { email }, Managed { position } }` | `best_candidate`'s first rule; the lead card's and banner's states | T3 |
| `forecast` | `BestForecast` | `best_candidate` over the view, computed once with it, so the screen and the forecast never disagree | T3 |

- **`Engine::last_auto_event(provider, since_ms) -> Option<EventView>`**: the latest row of the
  provider whose kind is one of §5.3's auto-line kinds, at or after `since_ms` (now − 30 days).
  One query, read with every `accounts` by `EngineSource` (§5.3). T4, with `EventView`.
- **PERF-R-10a:** `EngineSource::accounts` reads the live identity once per call and hands it to
  both `Engine::accounts` and `Engine::status`, so an unmanaged live login is no longer parsed
  twice (`source.rs:86-114`, `views.rs:509`). The read keeps `Read<T>`'s three states and
  today's fallback to the stored active account when the live file is unreadable; the
  statusline's cache, which folds unreadable into absent, is not used (§14).
- **The default provider's names** for the empty state before the first snapshot come from
  `TuiOptions` (tui_cmd fills them from the registry).

### 12.3 `Engine::timeline`

```rust
pub fn timeline(&self, provider: &ProviderId, scope: Scope, since_s: i64, columns: usize)
    -> Result<TimelineView, EngineError>;

pub enum Scope { All, Account(AccountId) }

pub struct TimelineView {
    pub since_s: i64,
    pub now_s: i64,
    pub threshold: f64,
    /// All: one series, the in-use account's binding usage. Account: one per relevant window.
    pub charts: Vec<Series>,
    /// All: one per account in use at some point in the range. Account: that account's.
    pub lanes: Vec<Lane>,
    pub events: Vec<EventView>,
}
pub struct Series { pub label: String, pub peaks: Vec<Option<f64>> }  // one per column
pub struct Lane { pub account: AccountRef, pub peaks: Vec<Option<f64>>, pub in_use: Vec<bool> }
pub struct EventView {
    pub at_ms: i64,
    pub kind: EventKind,
    pub source: EventSource,
    pub trigger: Option<String>,
    pub from: Option<AccountRef>,
    pub to: Option<AccountRef>,
    pub detail: Option<serde_json::Value>,
    /// The in-use account's binding usage just before and just after, for switches.
    pub before_pct: Option<f64>,
    pub after_pct: Option<f64>,
}
```

- **Reads:**
  - `Store::events_since(provider, since_ms)` (new: `WHERE provider = ?1 AND at >= ?2 ORDER BY
    at, rowid`; a scan, which is fine at event volumes, and no index, so no migration);
  - the state at the range's start: the provider's latest `autoswitch-paused` or
    `autoswitch-resumed` before `since_ms`, so a pause that began before the range draws from its
    first column (it is not listed as an event of the range);
  - `Store::active_history_since(provider, since_ms)` (new: the rows since plus the last one
    before, on the existing `active_history_by_at` index);
  - samples per account and window through hardening H10.2's key listing and `SAMPLES_OF_WINDOW`
    range reads, never `SAMPLES_OF_ALL`, which reads an account's whole retained history (up to
    180 days) to keep a day of it.
  - No pace work.
- **Buckets** are computed in the engine: each column's peak, as `history.rs`'s `sparkline` does
  today. The UI thread only draws.
- **`AccountRef`** carries the position, alias, email and a removed flag, resolved at read time.
- **Cost:** 30 days of 12 accounts is at most a few hundred thousand sample rows, read on the views
  lane; §7.7 bounds how often.

### 12.4 Stored events (T4, after R4)

`events.kind` is free text (`schema.sql:93-102`), and readers ignore kinds they don't know
(`tokens/view/group.rs:318-335`), so these need no migration. They join R4's `EventKind`, keeping
their string form.

| Kind | Written by | When | `from`/`to` | `detail` |
|---|---|---|---|---|
| `autoswitch-paused`, `autoswitch-resumed` | the daemon's provider job | the effective `daemon.autoswitch` changes on a settings reload (`provider_job.rs:~1041`); at daemon start, when it differs from the provider's latest stored `autoswitch-*` event | none | `{"at_start": true}` for the start case |
| `switch-failed` | the auto engine | an attempted switch fails (`auto.rs:~689-707`), unless the provider's latest `switch-failed` names the same target and message within 30 minutes | live, target | `{"message", "transient"}` |
| `all-exhausted` | the auto engine | the first `AllExhausted` decision after any other decision | live | `{"earliest_reset_at"}` |
| `switch` (existing), by auto or the daemon | `commit_switch` | as today | as today | new: `{"window", "label", "pct", "threshold"}`, the binding window that triggered it, from the decision |
| `remove` (existing) | lifecycle `remove` and purge | as today | as today | new: `{"label"}` (C-12) |

- **Sources** are R4's `EventSource`, so a pause written by the daemon is `daemon`.
- **No-switch outcomes are never stored:** auto decides most ticks, and the table is not pruned
  (C-11).
- **The writers** go through `Store::insert_event_on` as today, in the same transaction as the
  change they record where there is one.
- **`SwitchRequest`** gains an optional reason that the auto engine fills from its decision and
  `commit_switch` stores as `detail`. Manual switches leave it empty.
- **Core spec:** the `events` table's kind list (core §5, `CREATE TABLE events`) gains these kinds
  (§18). **Daemon spec:** D4.2 gains the pause-recording sentence (§18).

### 12.5 The daemon protocol (R3)

The TUI adopts R3's `Wire<T>` accessors, `collects_for_itself()`, `waits_for_upgrade()` and the
`poll`/`switch`/`no-switch` payload parsers in place of the literals at `daemon/mod.rs:40-45, 312,
374, 459-466` and the raw keys in `daemon/words.rs:112-162`. The binding window and "used = 100 −
headroom" come from the parser (CC-O-23f). The `String` fields in `protocol.rs` are then retyped
to `Wire<T>`.

## 13. Held items

From refactor §10, each re-verified at `683b266`:

| Item | Today | Change |
|---|---|---|
| ARCH-7 / CC-O-20, unlock | `Bridge::confirm` compares `question == app::UNLOCK_QUESTION` (`tui_host.rs:264-270`) | `Prompter::confirm_unlock()` (`prompt.rs:11-21`), defaulting to `confirm(UNLOCK_QUESTION, true)`, called by `ensure_unlocked` (`app.rs:1207`, wherever R3 moves it). `Bridge` overrides it to note `needs_terminal` and decline. `Bridge::confirm` just forwards |
| ARCH-7 / CC-O-20, warnings | `CliHost::new` builds an engine to learn the warnings, then strips `"warning: {w}\n"` from captured stderr (`tui_host.rs:33-75`) | `Context.shown_warnings: Arc<[String]>`; R3's `Io::warn` skips those; `CliHost` sets it from the warnings `tui_cmd` already has. `startup_warnings`, `with_startup_warnings`, `without_startup_warnings` and the extra engine build go |
| CC-O-2 / CC-O-3 | the bootstrap written three times in `tui_cmd.rs:39-48, 127-136` and `tui_host.rs:50-53`; `locate` and `color_enabled` wrappers | R3's `engine_for` and `Failure::report`; `ColorEnv` for the tier (§4.2). `tui_cmd.rs:30-33`'s `--json` refusal keeps its own copy |
| ARCH-3 / CC-O-7 (TUI side) | literals and raw JSON keys | §12.5 |
| ARCH-1 (TUI wording) | `actions.rs:291` and `screens/accounts.rs:25` name Claude Code | `ProviderAccounts.names` (§12.2); both neutrality-test allowlist entries are removed |
| CC-O-8, `Sender` in `State` | `Overlay::Choose` and `ConfirmThen::Reply` hold `Sender<Answer>`; `update` sends on them (`actions.rs:30-61, 376, 504`) | `AnswerTo(u64)` in the overlay and `Cmd::Answer { to, answer }`; the runtime keeps the senders. **The shutdown must decline an open prompt explicitly** (TUI §10.2 step 3), since the drop no longer does |
| CC-O-8, banner | `banner` and `banner_from` set apart, the runtime bypassing `raise` (`state.rs:57-60`, `runtime.rs:749-750`) | `struct Banner { text, kind }`, `raise_banner`/`clear_banner` |
| CC-O-8, runtime writes | the runtime sets `quitting`, `running`, the banner (`runtime.rs:711, 749-759`) | `Msg::ShutdownBegan { code }`, `Msg::SuspendRefused { action, why }` through `update`; `dirty` stays the runtime's |
| CC-O-8, reload ×5 | `update.rs:80-83, 272-275`, `actions.rs:207-209, 627-630`, `daemon/mod.rs:441-445` | `State::reload_views(now_ms) -> Vec<Cmd>` |
| CC-O-9 | `overlay_key`'s take-and-rebuild (`actions.rs:305-498`); `render_overlay`'s unused `now_ms` | match on `&mut Overlay`, return `KeyOutcome { Keep, Close, CloseWith(Vec<Cmd>) }`; `overlay/{confirm,choose,text,panel,help}.rs`, each with its key handling beside its rendering; one key table (§9.2); drop `now_ms` |
| CC-O-10 | four copies of the lane spawn (`runtime.rs:236-375`) | `spawn_lane(name, sources, tx, handler)`; `Lanes` moves to `lanes.rs` |
| CC-O-16 | `highlight` and `keep_visible` twice (`accounts.rs:236-246, 615-621`, `table.rs:791-801, 1128-1134`) | one `keep_visible(offset, first, last, rows, lines)` in `view.rs`; `highlight` goes (selection is the theme's, §4.3) |
| CC-O-18 | every module `pub` (`lib.rs:6-20`) | `pub(crate)`, except `host`, `source` and `live` (the binary and integration tests use them) |
| CC-O-19 | `host::Strategy` duplicates `UsageStrategy` (`host.rs:15-29`, `switch.rs:33-47`) | `Action::SwitchBy { strategy: UsageStrategy }` |
| CC-O-23c | `fetch_status -> Result<Status, String>` (`live.rs:311`) | `StatusError { NoDaemon, Mismatch { server }, Client(ClientError) }` through `Msg::DaemonStatus` |
| CC-O-23f | `daemon/words.rs:167-184` re-derives used and the binding window | R3's poll parser (§12.5) |

## 14. Performance

| Item | Today | Change |
|---|---|---|
| PERF-R-9 | the input thread polls crossterm every 50 ms (`input.rs:16, 147`), about 20 wake-ups a second idle; pause and drop wait up to a slice | the thread waits on the terminal, a wake pipe (pause, resume, stop) and a SIGWINCH pipe with `poll(2)`, and reads crossterm events without blocking once one is ready. No idle wake-ups; pause acknowledged within 50 ms. `input.rs`'s bounds (`:313, 411, 422, 446`) stay or tighten |
| PERF-R-10a | `liveness` re-parses the live login per view (`views.rs:509`); an unmanaged login twice | read once per `accounts` call, keeping `Read<T>`'s states and the stored-active fallback (§12.2) |
| PERF-R-10b | Tokens lays out every row each frame (`table.rs:1154-1205`) | cache `(items, starts)` on `TokensState`, keyed by report generation, width and the second; `render`, `follow` and `page` share it; built in `on_report` and on resize |
| PERF-D-2 (TUI half) | every `changed_at` rise re-queries (`tokens/index.rs:193-233, 270-300`) | C-13: the live throttle `LIVE_REQUERY_MS` goes from 5 s to 30 s, except while the TUI's own refresh runs and after `r` |
| First frame | 25–27 ms p95 measured at #18 | stays within 50 ms: it never waits for detection (§4.2), `last_auto_event` and the engine-lock read are small reads on the views lane, and the timeline is not read until History opens |

## 15. Delivery

Five stacked Draft PRs. Each has its own plan, written just in time
(`docs/superpowers/plans/2026-10-xx-tagteam-tui-redesign-tN-<topic>.md`) and reviewed by Codex
before execution. Each fills `.github/pull_request_template.md`, adds `CHANGELOG.md` entries for
its user-visible changes, and records rulings under "Rulings".

| PR | Contents | Findings | Depends on | Waits for |
|---|---|---|---|---|
| T1 | Foundations, no visible change: the held clean-ups that need nothing new (CC-O-8, -9, -10, -16, -18, -19, -23c), PERF-R-9, PERF-R-10b, the theme module with tier 1 mapped to today's look | §13, §14 | hardening | — |
| T2 | Visual system and chrome: the tiers and detection, the header (with `threshold`, `strategy`, `names` and the engine-lock check, §5.2, §12.2), the auto line from live events (§5.3), footer, toasts, banner, empty states, overlays, help and legend, outcomes, labels, key guards, `r` feedback, §11.3's sentences as new functions | VIS-2, -7, -8, -10, -11; UX-1, -4, -5, -6, -7, -8, -10, -13, -14, -15, -16 | T1 | — |
| T3 | Accounts and the shared words: lead card, table, cards, status, remedies, Now strip, `best_candidate`, the rest of §12.2, PERF-R-10a; §11.1 (`list`, `status`, `history`) and §11.2; `tagteam auto`'s human output adopts §11.3 | VIS-1, -3, -4, -5, -6, -13; UX-2, -9, -12 | T2 | R4 (C-15) |
| T4 | History: `Engine::timeline`, the new readers, `last_auto_event`, the event writers, §11.4's sentences, the store-fed auto line, the screen | VIS-5, -12; UX-3, -11 | T3 | R4 |
| T5 | Tokens: §8, the token report's footnote words (`tagteam tokens` reads them too), PERF-D-2 | VIS-4 (Tokens), -9, -14, -15 | T4 | R4 |

- **The R3 items** (D-2, CC-O-2/3, §12.5) go in the first PR planned after R3 merges: T2 if it has
  merged by then, else the next.
- **Merge order:** T1 to T5, each after Michael's review. A PR whose refactor dependency hasn't
  landed waits; the plan for the next one is written when it can start.
- **Status:** this spec moves to `In progress` with T1's PR, and to `Implemented`, citing every PR,
  in T5's final pre-merge commit.
- **Words:** T2 adds §11.3's sentences as new functions and changes no existing word. T3 changes
  the shared words together with the screens that show them, so `list` and the TUI never disagree
  in between, and `list`'s and `history`'s snapshot tests move once, in T3.

## 16. Testing

- **Existing suites** stay the net for behaviour this spec doesn't change (TUI §11).
- **Render tests** (TUI §11's TestBackend tests) are rewritten for the new layout:
  - a `roles(&Buffer)` helper maps each cell back to its role, so style tests assert roles, not
    colours, and run once per tier;
  - each screen at 80×24 and 120×40, in tiers 0 and 1, plus tier 2 dark and light for the
    selection and the dialog surface;
  - the 80×24 History budget (§7.8) with 3, 6 and 12 accounts;
  - the header's states (§5.2) at 80 and 120 columns, with `? keys` present in every one;
  - the footer: pinned items present at 80 columns on every screen.
- **State machine:** the toast kinds and durations; the outcome table (§9.3); the 300 ms guard;
  `R`'s confirm; `q` closing overlays first; the cursor and list staying in sync; `w`; scope sync
  between Accounts and History; the auto line's priority (§5.3), including a poll that must not
  replace a switch, an `add` after a switch that must not hide it, and `w` widening the range;
  the STATUS order (§6.3), with a quarantined account in use.
- **Theme detection:** OSC 11 and DA1 answered, DA1 only, a silent terminal (given up after
  1 s), no `COLORTERM`, NO_COLOR over everything; the first frame drawn before detection
  finishes, then redrawn in tier 2; §4.2's seven byte-order cases; through a pty, as
  `tests/signals.rs` does.
- **Engine:** `best_candidate` parity (§12.1); `timeline`'s buckets, in-use lanes, before/after
  values and scopes over a fixture store; a pause that began before the range; `events_since`
  and `active_history_since` boundaries; the samples read through H10.2's key listing (an
  EXPLAIN QUERY PLAN pin); `last_auto_event`'s kinds and 30-day bound; the live identity read
  once per `accounts`, an unreadable live file still falling back to the stored active account.
- **Writers:** pause and resume through a settings reload and at daemon start; `switch-failed`
  dedupe; `all-exhausted` on entry only; the switch `detail`; `remove`'s label; sources per R4.
- **Words:** every `NoSwitchReason`, trigger, event kind and usage status has a sentence (a test
  over each enum's `ALL`); `list`'s and `history`'s snapshot tests move with §11, and the CHANGELOG
  lists the change.
- **Performance:** `tests/perf.rs`'s first frame within 50 ms p95; a test that the input thread
  makes no wake-up while idle and acknowledges a pause within 50 ms.
- **Linux** in Docker before each PR is reported done.

## 17. Triage of every finding

| ID | Verdict at `683b266` | Where |
|---|---|---|
| VIS-1 | holds | §6.2, §5.2 |
| VIS-2 | holds; also `actions.rs:671, 721, 740` use `selected()` | §4.3 |
| VIS-3 | holds; the shared `row_notes` never reads the quarantine reason | §6.3, §6.4 |
| VIS-4 | holds; SPEND's column is `list`'s rule | §6.3, C-9, §8 |
| VIS-5 | holds; the Accounts split also feeds `J`/`K` scrolling | §6.1, §7, §8 |
| VIS-6 | holds; no pace exists for 5h (C-8) | §4.4 |
| VIS-7 | holds; doctor's glyphs come from the CLI's text | §9.1 |
| VIS-8 | holds; toasts need a kind | §5.2, §5.5, §8 |
| VIS-9 | holds; separators only where digits show (C-10) | §8 |
| VIS-10 | holds; `TokensState::reset()` drops the report before the read | §5.7, §8 |
| VIS-11 | holds | §4 |
| VIS-12 | partly: the range prints once, its key hint twice | §7 |
| VIS-13 | holds | §6.4 |
| VIS-14 | holds | §8 |
| VIS-15 | partly: the table cell says `removed`; the UUID shows in the filter and the detail | §8 |
| UX-1 | holds; only `switch` succeeds with two lines | §9.3 |
| UX-2 | partly: the detail rule already says active or not | §6.2, §6.5, §12.1 |
| UX-3 | mostly; with a prior poll the line names the window; 4 triggers plus `manual`; `last_outcome` is overwritten every tick | §5.3, §7, §12.4 |
| UX-4 | holds; help also omits `g G Home End PgUp PgDn Esc` | §5.5, §9.2, §10 |
| UX-5 | holds | §5.1, §5.2 |
| UX-6 | holds; warnings and failures already toast or banner | §9.7 |
| UX-7 | holds; `p` and `[ ]` also reset the Tokens drill-down (unchanged: they change the query) | §9.5, §10 |
| UX-8 | holds | §9.4 |
| UX-9 | holds; `A` works only while Claude Code is logged in as that account, which the remedy says | §11.2 |
| UX-10 | holds; a non-zero exit with no text is silent too | §9.3, §9.6 |
| UX-11 | holds | §6.5, §7 |
| UX-12 | holds | §11 |
| UX-13 | holds | §4.1, §9.2 |
| UX-14 | holds | §5.6, §13 |
| UX-15 | holds | §5.7 |
| UX-16 | mostly; **wrong** that the CLI refuses removing the account in use (core §10.3 refuses session-owned accounts and live-vault cases only) | §4.5, §9.1, §9.5 |

## 18. Amendments to other specs

Applied in this branch, as small hunks, without rewriting point-in-time facts:
- **TUI spec:** §4.3 (the input thread, §14), §5.1 (colour, selection, chrome rows), §5.2 (the
  table, the cards, "same words as `list`" meaning words), §5.3 (the control bar, headline,
  column words), §5.4 (the timeline), §5.5 (overlays, help), §5.6 (keys), §7.2 (outcomes), §7.4
  (the header table), §8 (what each event does, the auto line), §11 (the tests). Each hunk points
  to the section here.
- **Core spec:** §13.1 (`list`'s words and the `UPDATED` head), §13.4 (`history`'s projection
  words), the `events` kind list, and §11's `auto` human output.
- **Daemon spec:** D4.2 records pauses and resumes as events.

## 19. Left to the builder

- **Names** of new types, fields and helpers where this spec gives a shape.
- **Commit granularity** inside each PR, moves separate from edits.
- **`terminal-colorsaurus` or a hand-rolled query** (C-7), decided in T2's plan after `cargo deny`
  and the musl build.
- **Exact column widths** within §4.5's and §6.3's rules, checked against the gallery renders.
- **The swimlane cap** beyond §7.4's numbers at other heights.
- **How the short model ID is derived** (§8), as long as it is unambiguous among the report's
  models; the full ID stays in the detail.
- **The axis's labels** for 1 and 30 days, within §7.4's rule.
