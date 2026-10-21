# tagteam TUI redesign T2 — visual system and chrome — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Approved
**Goal:** Give the TUI its visual system (roles over three tiers, with the truecolor tier detected safely) and its new chrome: a header that says auto's state, an auto line, a footer and help from one key table, kinded toasts, framed overlays, key guards, empty states and plain words for auto's decisions, with no `--json`, protocol or store change.
**Architecture:** The engine's accounts view gains each provider's threshold, strategy and names, and `words.rs` gains §11.3's sentences. The input thread asks the terminal its background before its first crossterm read and filters any colour reply it did not finish. `theme.rs` grows to seven roles over tiers 0, 1 and 2. New modules hold the chrome (`chrome.rs`, `autoline.rs`, `keys.rs`), detection (`detect.rs`, `replies.rs`) and the hand-over dialog (`overlay/proceed.rs`).
**Tech Stack:** Rust 1.88 (edition 2024), ratatui 0.30.2, crossterm =0.29.0, `tagteam_provider::term`'s safe `poll(2)`/`read(2)` wrappers. No new dependency.
**Spec:** docs/superpowers/specs/2026-10-20-tagteam-tui-redesign-design.md (§1–§5, §9, §10, §11.3, §12.2's T2 fields, §15's T2 row, §16)

**Reading this plan.**
- Paths are under `crates/tagteam-tui/` unless they start with `crates/`, `docs/` or a file at the root.
- Line numbers are as of `ba624bd` (`main`, T1 merged; its tree is byte-identical to `229dba0`). Find each site by the code quoted; earlier tasks move them.
- Focused tests: `cargo test -p tagteam-tui --locked <filter>`; `cargo test -p tagteam-engine --locked <filter>`; `cargo test -p tagteam --locked --features test-support --test <file>`.
- The pty tests (`tui_pty.rs`, `perf.rs`) open a pseudo-terminal: run them outside the sandbox.
- Each task leaves the workspace compiling, with its crates' tests green.
- "Tier 0/1/2" are §4.2's NO_COLOR, ANSI-16 and truecolor tiers. In tests, `TIERS` means `[Theme::new(false), Theme::new(true), Theme::new(true).truecolor(Background::Dark), Theme::new(true).truecolor(Background::Light)]`.

## Global Constraints

- **CI commands** (`CONTRIBUTING.md`):
  - `cargo fmt --all --check`;
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`;
  - `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`.
- **Toolchain:** Rust 1.88, edition 2024, ratatui 0.30.2, crossterm =0.29.0 (pinned).
- **No silenced lints.** No `#[allow(...)]` is added. A role, variant or field is added in the task that first uses it; dead code is deleted, or `#[cfg(test)]` when only tests use it.
- **No `unsafe`.** `tagteam-tui` stays `#![forbid(unsafe_code)]`; it reaches the terminal's file descriptors only through `tagteam_provider::term`.
- **No new dependency.** `Cargo.toml` and `Cargo.lock` do not change: `git diff ba624bd -- Cargo.toml Cargo.lock crates/*/Cargo.toml` prints nothing at the end.
- **No `--json`, protocol, store or migration change.** `crates/tagteam-daemon/` is not edited.
- **Words.** T2 adds §11.3's sentences as new functions in `crates/tagteam-engine/src/words.rs` and changes no existing word there: `list`'s, `status`'s and `history`'s snapshot tests do not move. TUI-only strings may change, as the tasks say.
- **Provider-neutral text.** No production code line that T2 adds or changes under `crates/tagteam-tui/src` or `crates/tagteam-engine/src` names `claude` or `anthropic` in any case. After Task 13, no production line under `crates/tagteam-tui/src` does at all. The engine's existing ones (`doctor.rs`, `export.rs`, `engine.rs`, `lazy_http.rs`) are R1's to move, and T2 leaves them alone. Comment lines and test modules are excepted, as R1's scan reads it. No new production use of `tagteam_core::CLAUDE_CODE` is added in either crate. Provider words come from `ProviderNames` (ARCH-1). R1's neutrality test enforces this once it lands; Task 13 adds the TUI's own.
- **Example names only** (`alex@example.com`, `sam@corp.example`, `Example Corp`, and the fixtures' `a1@x.co`), never a real name. Provider display names in fixtures are the providers' own (`Claude Code`, `FakeAgent`).
- **No test is weakened.** A test edit is one of:
  - a mechanical change to an API this plan changes, listed in its task;
  - a render or text test rewritten to the new layout per §16, named in its task, asserting what the new layout shows;
  - a new test.

  Nothing is deleted to make a run pass. Where a fixture filled the body exactly and the chrome now takes a row, the fixture changes so the test still exercises the same property.
- **Moves are separate from edits.** No task in this plan moves code between files; replaced code is rewritten in place or deleted.
- **Commits.**
  - Imperative and in sentence case, with no prefix and no trailing period, matching `git log`.
  - Each task's message is given in its last step.
  - No agent attribution: no `Co-authored-by:` agent line, no session link, no "Generated with" footer.
- **The first frame.** `tests/perf.rs`'s first-frame p95 stays within 50 ms, and detection never runs in front of it (Task 3 adds a case with the query asked).
- **Files this plan may touch:**
  - `crates/tagteam-tui/**`;
  - `crates/tagteam-engine/src/views.rs` (§12.2: `ProviderAccounts`, `ProviderNames`, `Engine::view`);
  - `crates/tagteam-engine/src/words.rs` (§11.3: new functions only);
  - `crates/tagteam-engine/tests/fake_agent.rs` (Task 1's test);
  - `crates/tagteam/src/tui_cmd.rs` (`TuiOptions`'s new fields);
  - `crates/tagteam/src/render.rs`, its `#[cfg(test)]` module only: the two `ProviderAccounts` literals (`:903-909`, `:945-951`);
  - `crates/tagteam/tests/tui_pty.rs`, `crates/tagteam/tests/common/pty.rs`, `crates/tagteam/tests/perf.rs`;
  - `CHANGELOG.md`;
  - `docs/superpowers/specs/2026-10-14-tagteam-tui-design.md` (Task 15's hunks);
  - this plan.

  `crates/tagteam/src/tui_host.rs`, `app.rs` and `prompt.rs` are not edited: D-2 and CC-O-2/3 wait for R3 (§15).
- **Never committed:**
  - the review harness (`src/ux_gallery.rs`, `src/ux_concept.rs`, `src/ux_redesign.rs` and their `lib.rs` lines);
  - any render (`.txt`, `.svg`, `.png`).

  The harness is parked in `.superpowers/tui-redesign/harness/` whenever a task runs. Before every commit, the controller checks that `git status --short` lists no `ux_` file.

### Where the refactor PRs touch the same files

R1 (#22), R2 (#24) and R3 (#25) have not merged. T2 keeps its edits away from their hunks; whichever lands second resolves these on its rebase.

| File or API | The refactor | T2 | On the rebase |
|---|---|---|---|
| `views.rs`, `words.rs` | test-module imports only (`CLAUDE_CODE` moves to `tagteam_cc`); `words.rs:50`'s `duration` re-export source | `ProviderAccounts`, `Engine::view` (`:473-499`); new functions after `:362` | keep both |
| `tagteam_core::autoswitch::Strategy` | R3 renames it `AutoStrategy` | `ProviderAccounts.strategy` and the header's match use `Strategy` | rename |
| `NoSwitchReason::ALL`, `::parse`, `Trigger::ALL` | R3 adds them to core | `words::NO_SWITCH_REASONS` stands in (T2-27) | use R3's; delete the stand-in |
| `crates/tagteam/src/render.rs` tests | R3 edits hunks near `:881` and `:974` | two literals gain three fields | keep both |
| `src/testutil.rs`, `tests/source.rs` | R1's `CLAUDE_CODE` import line | other lines | keep both |
| `daemon/mod.rs`, `daemon/words.rs` raw keys | R3's §12.5 parsers replace them | T2 reads `reason`, `detail`, `message`, `earliestResetAt`, `headroomPct`, `active.number` raw | the PR that adopts R3 retypes them |
| `crates/tagteam-fake/tests/neutrality.rs` (R1) | allowlists `accounts.rs`'s "Log in with claude, then press A" and `actions.rs`'s "Claude Code is not changed." | both strings take `ProviderNames` (T2-6) | delete the two `ALLOWED` entries |

## Rulings

Calls inside the spec, for the PR's "Rulings" section.

| # | Ruling | Why |
|---|---|---|
| T2-1 | **C-7: the query is hand-rolled**, not `terminal-colorsaurus`. It reads fd 0 byte by byte through `tagteam_provider::term::{wait_readable, read_byte}` and checks the terminal with `stdin_is_our_foreground_terminal()`. | colorsaurus 1.0.3 (source read on docs.rs) reads through `terminal-trx` and a 32-byte `BufReader` with `read_until`. It can take bytes past the DA1 reply, and it drops what it holds on a timeout, so it can neither leave keys typed after the reply unread nor hand an unfinished reply's state to the input thread. Its one timeout (`QueryOptions::timeout`, default 1 s) has no 2 s cap and no shutdown token, and it sets raw mode itself. It would add `terminal-trx`, `xterm-color`, `memchr` and `cfg-if`. The hand-rolled query adds no dependency and needs no `unsafe` of ours, so `cargo deny` and the musl static build are unaffected; neither is needed for this decision. |
| T2-2 | Detection runs **on the input thread, before its first crossterm read**. The query is written under the terminal's ownership lock (`term::query`), so it never splits a frame's escape sequences. Detection waits on `poll(2)` in 50 ms slices and checks the input control between slices: a pause, stop or hang-up ends it within one slice. | One thread owns every byte read from fd 0. The mio wait is edge-triggered, so a byte-wise read cannot use it, and its waker cannot reach `poll(2)`. The UI thread never waits, so the first frame cannot. |
| T2-3 | **The reply filter works on crossterm's key events**, as crossterm 0.29.0 surfaces them (`parse.rs`). `ESC ]` comes out as Alt+`]` in one read, or as Esc then `]` when ESC ends a read. Each body byte is a plain char key, BEL is Ctrl+G, and ST is Alt+`\` or Esc then `\`. crossterm keeps a whole DA1 reply to itself; a split one comes out as Esc, then `[`, `?`, digits, `;` and `c`. The filter is **calm** when no query was asked or detection read the DA1 answer: then it only drops an unasked one-read Alt+`]` reply, and Esc acts at once. It is **wary** when detection ended without the DA1 answer. Then keys that may begin a reply (Esc, Alt+`]`, and the `]`, `11;` or `[` after them) are held until a later key shows what they are, never released by time. A split DA1 answer ending through it makes it calm. An OSC ends only at BEL or a real ST, never at a bare `\`. Task 4's table gives the states. | Only crossterm decodes keys (§4.2: no second decoder), and holding a decoded key is not a replay of detection's bytes. DA1 is answered last (C-7), so once it has been read no asked-for reply can come, and the common case costs nothing. A late reply can come only after a give-up. There, no byte of it may reach `update`, so no timer may release one; a timer would let a paused reply's Esc cancel a dialog. The cost falls only on a terminal that ignored the query for over a second: a lone Esc acts when the next key arrives. `q` still closes overlays at once, and `Backspace` still goes back. That is smaller than the full hold §4.2 already accepts after a hand-off. An 8-bit ST (0x9C) after a hand-off never reaches the filter, because crossterm drops invalid UTF-8. Such a reply stays fail-closed until Ctrl-C, as §4.2's never-ending case does. |
| T2-4 | No tmux special case. | A tmux that neither answers OSC 11 nor passes it through answers DA1 itself, which ends detection in tier 1, as §4.2's fallback requires. A tmux that answers with the pane's colour is believed. |
| T2-5 | `ProviderAccounts.threshold` and `.strategy` come from `Settings::load(&self.env, provider)`, once per provider per `Engine::view`, with the provider's own table first. They do not come from the engine's settings snapshot. T3's `models` reads the same load. | The snapshot holds another provider's settings when there are several, and is stale after an edit. The daemon's jobs read settings this way. A config read is well under a millisecond. |
| T2-6 | **T2 closes ARCH-1's TUI wording.** The two strings R1 allowlists, and every sentence T2 adds, take `ProviderNames`. | §12.2 assigns `names` to T2. T1's plan said T3 before T2's contents were planned. |
| T2-7 | **The auto line before T4:** ranks 1 and 3 come from the daemon's `status.lastSwitch` (within 30 minutes, within 30 days). Rank 2 comes from this link's `no-switch`, `error` and `config-warning` events and, until one arrives, from `status.lastOutcome`, the newer winning. Timer mode has rank 4 only. Switches are worded from their trigger (C-14). The window naming from the tick's `poll` (`daemon/words.rs:175-184`, `binding`) is deleted. | §5.3's last bullet: no store events before T4. `lastOutcome` keeps the line from being blank just after a connect (UX-3). |
| T2-8 | `w why`, the `w` key and History's cursor arrive with T4's timeline. T2 shows no `w` hint and lists no `w` key. | A key that opens today's History would not do what `why` promises. |
| T2-9 | `r` keeps TUI §6.3's collection set in both modes: every shown account, through `collect_due`'s own due rule. The toast is chosen in this order: rate-limited, then warnings, then updated, then already current. | §9.7's "in live mode the same" is read as the same feedback. §18 amends no TUI §6.3. |
| T2-10 | `✓ connected to the daemon` toasts only after `! lost the daemon…` was shown in this session. | The first link at start-up is not a change the person noticed. |
| T2-11 | While the TUI waits for the daemon's first `status` (C-R20's settle, at most 2 s), the header shows no state, only `? keys`. An engine-lock holder that is the daemon itself (`daemon.lock`'s pid and start time) is never "another terminal". | Otherwise `○ auto off · no daemon` would flash before `● auto on`. |
| T2-12 | Header rows the §5.2 table leaves open: a live job waiting on `engine-lock` with no pid reads `● auto runs in another terminal` (80: `● auto in another terminal`). A provider the daemon runs no job for reads `○ auto off`. An unknown mode reads `○ daemon <mode>`. The long forms start at 120 columns. | They complete the table. |
| T2-13 | Auto-line marks for rank 2: none for a no-switch, except `✕` (crit) for all-exhausted; `!` (warn) for `error` and `config-warning`, which show their message as the daemon sent it. Rank 4 is muted, with no mark. | §4.4's marks. |
| T2-14 | **Banners.** A failed read (accounts, history) and a lane's death are crit with `✕ `. Everything else is warn with `! `. One banner shows at a time, first match: held keys, the state's banner, Tokens' error, the start-up warnings, then the accounts view's (run shell, unmanaged login). | §5.6: crit for a failed read, warn otherwise. Held keys come first because the keyboard is unusable meanwhile. |
| T2-15 | **Selection.** Dialog lists (choose, filter) get a two-column gutter that holds `▌` for the selected choice. Buttons and the text cursor take `Theme::focus()`: accent fill, REVERSED in tier 0. Tokens' selected row bolds its first column, and History's account list bolds the name. | §4.3 allows REVERSED only for a small focused control. |
| T2-16 | **`P`.** The question becomes `Pause auto for <display name>?` / `Resume auto for <display name>?`. Without a daemon, `P` toasts `no daemon runs · tagteam daemon install starts one` (Info). With no job, it toasts `the daemon runs no job for <display name>`. | The header calls it "auto". §10 says to say how to install it. |
| T2-17 | **Labels** (§9.4) not listed there: `R` is `starting <display name> as <name>…`, and clearing an alias is `setting the alias…`. `finishing <name>…` (TUI §10.2) keeps the CLI's name. | The spec's list leaves these out. |
| T2-18 | **The 300 ms guard** starts whenever a key closes an overlay. It drops only `s b n e a m x R P A D r`. | §9.5. |
| T2-19 | **Hand-over.** `Esc` cancels the hand-over dialog silently, because the command changed nothing (TUI §4.5). Return toasts (§9.6) are picked by a `State.on_terminal` flag, set when `update` returns `Cmd::Suspend` and taken by `on_done`. | `ActionOutcome` cannot say where an action ran. |
| T2-20 | **`m`.** Enter on a position outside 1–N keeps the dialog open and shows `Choose a position from 1 to N.` (crit) under the input. N is the number of the provider's accounts. | §9.5. |
| T2-21 | **Start-up warnings** show as the CLI printed them (after `warning:`), joined by ` · `. `Esc` on Accounts clears them, and so does a snapshot whose config mtime differs from the one `tui_cmd` read before loading the settings. | §5.6. The mtime is read first, as `Settings::mtime` advises. |
| T2-22 | **A failed Tokens read.** Only a failed report query keeps the last report, DIM, under `! couldn't read the token index (<error>) · r retries`. The index check's and the refresh's banners keep their words, now in warn. | §5.7, §8. Only the report is "the last report". |
| T2-23 | History keeps its own empty and loading words (`No usage history.`, `loading…`) until T4 replaces the screen. | §5.7's History row is the timeline's. |
| T2-24 | **Times** (§5.3): `HH:MM` on today's date in the TUI's zone, `Fri 15:38` within the six days before, else `Oct 12 15:38`. | §5.3 leaves "this week" open. |
| T2-25 | The legend lists every mark of §4.4's Markers row, even ones a later PR draws first. Each is drawn in its role, with its words. | One help, never edited again. |
| T2-26 | `ProviderNames` holds `&'static str`s, from `Provider::display_name` and `Provider::launch_command`. | The provider trait returns them so. |
| T2-27 | `words::NO_SWITCH_REASONS` (all 17) stands in for R3's `NoSwitchReason::ALL`. The TUI finds a reason from its code through it. | core is not this branch's (§3.1). |
| T2-28 | The footer starts at column 0, as today. The header, the auto line and the banner keep a one-column inset. | `tui_pty.rs`'s resize test reads `s switch` at column 0. |
| T2-29 | **The key table's "handled" test:** a key is handled when `update` returns a command, or the frame drawn after it differs from the one before. | It works for every key without a per-key hook. |

## Review Focus

The five inputs or failure modes the spec implies that no test of the existing suite exercises, most likely first. Each one's test goes in the task that owns the code.

1. **A colour reply in a form the parser hasn't met:** 2-digit or uppercase hex, `rgba:`, BEL or 8-bit ST, a body longer than 64 bytes, or garbage. It must end detection without a panic and leave tier 1 unless the colour parses. Test: Task 3, `odd_colour_replies_parse_or_leave_tier_1`.
2. **A shutdown while detection still waits on a silent terminal.** SIGTERM or `q`'s path must not wait out the 1 s bound. Tests: Task 3, `a_stop_ends_detection_within_a_slice` (`detect.rs`) and `sigterm_while_the_terminal_ignores_the_colour_query_exits_at_once` (`tui_pty.rs`, exit 143 within 500 ms).
3. **The config edited while the TUI runs.** The header's threshold must follow the next accounts read, and the start-up warnings banner must clear. Tests: Task 1, `each_provider_s_view_carries_its_own_settings_and_names` (re-read after a write); Task 14, `a_config_change_clears_the_start_up_warnings`.
4. **An outcome too long for the toast's three rows beside the pinned keys at 80 columns.** It must open the result panel, never a cut toast, and the pinned keys stay. Test: Task 10, `a_done_outcome_too_long_for_three_rows_opens_the_panel`.
5. **The 300 ms guard and the keys a person uses right after a dialog.** `j`, `k`, `?`, `q`, `Esc` and `1`–`3` still work at once. An action key at 299 ms is ignored, and at 300 ms it acts. Test: Task 12, `the_guard_drops_only_action_keys_and_only_for_300_ms`.

## Spec coverage

| Spec | Tasks |
|---|---|
| §4.1 roles, tier values; VIS-11 | 3 (tier 2, warn/crit), 6 (text, muted, faint, accent), 8 (sel-bg, focus), 10 (ok), 11 (surface, dim) |
| §4.2 tiers, detection, byte rules, tests | 3, 4 |
| §4.3 selection (VIS-2; R-2's `highlight` removed) | 8 |
| §4.5 below 80×24, the rule from 30 rows | 6 (rule), 13 (notice) |
| §5.1 chrome rows, UX-5 | 6 |
| §5.2 header, C-16, C-17, mode toasts | 6, 7, 10 |
| §5.3 auto line (ranks 2 and 4 plus `lastSwitch`) | 6 |
| §5.5 footer, toasts, spinner; UX-4, VIS-8, VIS-10's hints | 9, 10 |
| §5.6 banner; UX-14 | 6, 14 |
| §5.7 empty and loading states; UX-15, VIS-10 | 13, 14 |
| §9.1 dialogs and panels; VIS-7, UX-16 (`q`, panels) | 11 |
| §9.2 help, legend, one key table (R-1, CC-O-9); UX-13 | 9 |
| §9.3 outcomes; UX-1 | 10 |
| §9.4 labels, `P` toast, busy; UX-8 | 10 |
| §9.5 key guards, `R` confirm, `m`; UX-7, UX-16 | 9 (`g`/`G`), 12 |
| §9.6 hand-over; UX-10 | 10 (return toasts), 12 (dialog) |
| §9.7 `r` feedback; UX-6 | 10 |
| §10 keys | 9 |
| §11.3 sentences | 2, 6 |
| §12.2 `threshold`, `strategy`, `names`, `TuiOptions` names | 1, 6 |
| §16 tests for the above | 1–14 |
| §18 TUI-spec hunks; CHANGELOG | 15 |

Out of T2, by §15: D-2, CC-O-2/3 and §12.5 (R3); §11.1, §11.2, the Accounts screen, the Now strip, `best_candidate` and PERF-R-10a (T3); the store events, the timeline and `w` (T4); Tokens' §8 layout (T5).

**Order.** Every task builds and tests the same crates, so tasks never run in parallel: one implementer at a time, in the order below. The `Depends on` lines name the hard dependencies.

---

## Before Task 1 (controller)

- [ ] **The branch.** `git -C /Users/michael/Code/tagteam-tui-redesign status --short` prints nothing, and `git -C /Users/michael/Code/tagteam-tui-redesign log --oneline -1` shows `tui-redesign-t2` at `ba624bd`.
- [ ] **The harness is parked** in `.superpowers/tui-redesign/harness/` (`ux_gallery.rs`, `ux_concept.rs`, `ux_redesign.rs`, `lib.patch`).
- [ ] **The "before" renders** are `.superpowers/tui-redesign/gallery/after-16`, T1's last check, drawn from this same tree.
- [ ] **The gallery check**, run after Tasks 6, 11 and 14:
  - Copy `ux_gallery.rs` into `src/` and apply its `lib.rs` line from `lib.patch`. Apply `ux_concept.rs` and `ux_redesign.rs` only if they still compile; they hold the spec's mock-ups.
  - Fix only the harness's own compile errors against the new APIs. It is never committed.
  - **The truecolor variant**, from the Task 6 check on. In `ux_gallery.rs`'s `if s.variants` block, after the NO_COLOR render, render twice more:
    - `state.theme = Theme::new(true).truecolor(Background::Dark)`, written with `&DARK` as `<name>-<w>x<h>-truecolor-dark`;
    - `Theme::new(true).truecolor(Background::Light)`, written with `&LIGHT` as `…-truecolor-light`.

    Import `crate::theme::{Background, Theme}`.
  - Render: `UX_GALLERY_DIR=$PWD/.superpowers/tui-redesign/gallery/t2-after-<task> cargo test -p tagteam-tui ux_gallery -- --nocapture`.
  - Check by eye, at 80×24 and 120×40 in tiers 0, 1 and 2 (dark and light), against the spec's mock-ups: §5.2's header table, §5.3's auto line, §5.5's footer and toasts, §5.6's banners, §5.7's empty states, §9's dialogs and panels, and the `ux_concept` dialog render.
  - Park the harness again.

  A difference from the spec is a defect of the task that drew it: send it back to that task's implementer. The renders show real names: never quote them in a commit, the PR or a review.

---

### Task 1: Carry each provider's auto settings and names in the accounts view

**Depends on:** none

**Files:**
- Modify: `crates/tagteam-engine/src/views.rs`. Add `ProviderNames`. `ProviderAccounts` (`:124-129`) gains three fields, which `Engine::view` (`:473-499`) fills (T2-5).
- Test: `crates/tagteam-engine/tests/fake_agent.rs`.
- Modify, mechanically, every `ProviderAccounts { … }` literal to set the new fields:
  - `crates/tagteam/src/render.rs:904`, `:946` (test module): `threshold: 90.0, strategy: Strategy::Best`, with names from `ProviderNames { display_name: "Claude Code", launch_command: "claude" }`;
  - in `crates/tagteam-tui`: `src/testutil.rs:155` (with `pub(crate) const CC_NAMES: ProviderNames` holding those two strings), `daemon/mod.rs:749`, `view.rs:854, 1024`, `actions.rs:1324`, `screens/history.rs:865`, `screens/accounts.rs:1688`. A `fake-agent` literal uses `ProviderNames { display_name: "FakeAgent", launch_command: "fakeagent" }`.

**Interfaces:**
- Produces, in `tagteam_engine::views`:
  ```rust
  /// The provider's own words for the screens (ARCH-1): `Claude Code`, `claude`.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct ProviderNames { pub display_name: &'static str, pub launch_command: &'static str }
  impl ProviderNames { pub fn of(p: &dyn Provider) -> ProviderNames }
  pub struct ProviderAccounts {
      pub provider: ProviderId,
      pub active_position: Option<u32>,
      pub accounts: Vec<AccountView>,
      /// The effective `autoswitch.threshold` and `autoswitch.strategy`, the provider's own table first.
      pub threshold: f64,
      pub strategy: tagteam_core::autoswitch::Strategy,
      pub names: ProviderNames,
  }
  ```
- `Engine::view` reads `let (settings, _) = Settings::load(&self.env, provider);`. The warnings were printed at start-up.

- [ ] **Step 1: Write the test** `each_provider_s_view_carries_its_own_settings_and_names` in `fake_agent.rs`:
  - Write `config.toml` at `tagteam_engine::settings::config_path(&ffx.fx.env)`: `[autoswitch]` `threshold = 80`, then `[provider.fake-agent.autoswitch]` with `threshold = 95` and `strategy = "consume-first"`.
  - Add one account to each provider, as `accounts_lists_every_provider_with_accounts` does.
  - `ffx.engine.accounts(None)` gives claude-code `(80.0, Strategy::Best, ProviderNames { "Claude Code", "claude" })` and fake-agent `(95.0, Strategy::ConsumeFirst, ProviderNames { "FakeAgent", "fakeagent" })`.
  - Rewrite the file with `threshold = 85` at the top level. The next `accounts(None)` gives claude-code `85.0` on the same engine (Review Focus 3).
- [ ] **Step 2: Run it to see it fail.** Run `cargo test -p tagteam-engine --locked --test fake_agent each_provider`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed in Files and Interfaces.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-engine --locked`, `cargo test -p tagteam-tui --locked` and `cargo test -p tagteam --locked --features test-support --lib`. Expected: all pass, with `list`'s and `status`'s snapshots unchanged.
- [ ] **Step 5: Commit.** Run `git add crates/tagteam-engine crates/tagteam-tui crates/tagteam/src/render.rs && git commit -m "Carry each provider's auto settings and names in the accounts view"`.

### Task 2: Say auto's no-switch reasons and triggers in plain words

**Depends on:** none

**Files:**
- Modify: `crates/tagteam-engine/src/words.rs`. Append the new items after `token_footnotes` (before `:363`'s `#[cfg(test)]`), and the tests to its test module.

**Interfaces:**
- Produces, in `tagteam_engine::words`:
  ```rust
  /// Every `NoSwitchReason`, until R3's `NoSwitchReason::ALL` (T2-27).
  pub const NO_SWITCH_REASONS: [NoSwitchReason; 17];
  /// What a `no-switch` sentence says besides its reason (spec §11.3).
  #[derive(Debug, Clone, Copy)]
  pub struct NoSwitchFacts<'a> {
      /// The decision's `detail` as `auto` reports it: `4m` (cooldown), `2/3` (active-usage-unknown); else empty.
      pub detail: &'a str,
      /// The account in use's usage at the tick, for `below-threshold`: 100 less its headroom.
      pub used_pct: Option<f64>,
      pub threshold: f64,
      /// `all-exhausted`: when the first account frees up, in the caller's clock form (`16:40`).
      pub frees_at: Option<&'a str>,
      pub display_name: &'a str,
  }
  pub fn no_switch(reason: NoSwitchReason, facts: &NoSwitchFacts<'_>) -> String;
  /// §11.3's trigger words: `before it reached the limit`, `at the limit`, `its usage couldn't be read`, `to use up the account that resets first`.
  pub fn trigger(trigger: Trigger) -> &'static str;
  /// A threshold as the header and the sentences show it: `90%`, or `92.5%` with a fraction (one decimal).
  pub fn threshold(pct: f64) -> String;
  /// `<position> <alias, else the email's local part>`: `2 work`, `1 alex` (spec §5.3, §11.4).
  pub fn short_name(position: u32, alias: Option<&str>, email_or_label: &str) -> String;
  ```
- **The sentences** are §11.3's table, exactly. `{display}` is `facts.display_name`, `{t}` is `threshold(facts.threshold)`, and `{u}` is `used_pct` rounded, as `list` rounds.
- **When a fact is missing,** these forms apply:
  - `below-threshold` without `used_pct`: `no switch needed (switches at {t})`;
  - `cooldown` with an empty detail: `waiting after the last switch`;
  - `all-exhausted` without `frees_at`: `every account is near its limit`;
  - `active-usage-unknown` whose detail is not `n/m`: `can't read the account in use`.

- [ ] **Step 1: Write the tests** in `words.rs`'s test module:
  - **`every_no_switch_reason_has_its_sentence`.** For all 17, with `NoSwitchFacts { detail, used_pct: Some(77.0), threshold: 90.0, frees_at: Some("16:40"), display_name: "Claude Code" }`, where `detail` is `4m` for cooldown, `2/3` for active-usage-unknown and empty otherwise, `no_switch` equals §11.3's sentence. Examples: `no switch needed (77%, switches at 90%)`, `can't read the account in use (2 of 3 tries)`, `Claude Code isn't logged in`.
  - **`a_sentence_without_its_facts_still_reads`:** the four fallbacks above.
  - **`the_reason_list_is_every_reason_once`.** A local `fn index(r: NoSwitchReason) -> usize`, an exhaustive `match` giving 0 to 16 in declaration order. `NO_SWITCH_REASONS.map(index) == [0, 1, …, 16]`. A new variant then fails to compile here.
  - **`every_trigger_has_its_words`:** the four triggers' words.
  - **`a_threshold_is_whole_unless_it_has_a_fraction`:** `90.0` → `90%`, `92.5` → `92.5%`, `50.0` → `50%`.
  - **`a_short_name_is_the_alias_else_the_email_s_local_part`:**
    - `(2, Some("work"), "sam@corp.example")` → `2 work`;
    - `(1, None, "alex@example.com")` → `1 alex`;
    - `(3, None, "ci-label")` → `3 ci-label`.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-engine --locked --lib words::`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed in Interfaces.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-engine --locked`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add crates/tagteam-engine/src/words.rs && git commit -m "Say auto's no-switch reasons and triggers in plain words"`.

### Task 3: Ask the terminal its background before the first key is read

**Depends on:** none

**Files:**
- Create: `src/detect.rs`: the query, the byte parser, the bounded read, and the background from the reply.
- Modify:
  - `src/lib.rs`: add `mod detect;`.
  - `src/theme.rs`: add `Background` and tier 2 (`Tier::True(Background)`), with §4.1's tier 2 values for `Warn` and `Crit`. `strong` and `selection` stay as they are.
  - `src/term.rs`: add `query` after `restore` (`:143`).
  - `src/input.rs`:
    - `spawn` (`:309`) takes `detect: bool`;
    - `spawn_with` (`:313`) takes `detection: Option<Detection>`;
    - the thread runs detection before `read` (`:335`), and sends `Msg::Detected`.
  - `src/msg.rs`: add `Msg::Detected`.
  - `src/update.rs`: apply it.
  - `src/runtime.rs`:
    - `TuiOptions` (`:42`) gains `truecolor: bool`;
    - `run` (`:70-80`) passes `move |tx| Input::spawn(tx, opts.color && opts.truecolor)`.

    Copy what the closure needs before `opts` moves.
  - `crates/tagteam/src/tui_cmd.rs:104-113`: `truecolor: matches!(std::env::var("COLORTERM").as_deref(), Ok("truecolor" | "24bit"))`.
- Modify tests, mechanically:
  - the runtime's `options()` (`runtime.rs:1063`) and the literals at `:1120, 1297, 1325` gain `truecolor: false`;
  - `input.rs`'s tests pass `None` to `spawn_with`.
- Modify: `crates/tagteam/tests/common/pty.rs`. Add `Pty::wait_for_output(&mut self, within: Duration, what: &str, needle: &[u8])`, which polls `output()` every 10 ms, as `wait_until` does.
- Test: `src/detect.rs`, `src/theme.rs`, `src/runtime.rs`, `crates/tagteam/tests/tui_pty.rs`, `crates/tagteam/tests/perf.rs`.

**Interfaces:**
- Produces:
  ```rust
  // theme.rs
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub(crate) enum Background { Dark, Light }
  impl Theme {
      /// Tier 2 on a terminal whose background is `background`; tier 0 stays tier 0 (spec §4.2).
      pub(crate) fn truecolor(self, background: Background) -> Theme;
  }
  // detect.rs
  /// OSC 11 then DA1: `ESC ] 11 ; ? ESC \` `ESC [ c`.
  pub(crate) const QUERY: &[u8] = b"\x1b]11;?\x1b\\\x1b[c";
  pub(crate) const QUIET: Duration = Duration::from_secs(1);
  pub(crate) const LIMIT: Duration = Duration::from_secs(2);
  /// The byte parser: ground, ESC, CSI (ESC [, its `?` and parameters), OSC (ESC ], its body
  /// up to 64 bytes, then ESC of a two-byte ST).
  pub(crate) struct Replies { /* state, body */ }
  impl Replies {
      pub(crate) fn new() -> Replies;
      /// Takes one byte. True once the DA1 answer (`ESC [ ? … c`) has ended.
      pub(crate) fn push(&mut self, byte: u8) -> bool;
      /// The background an OSC 11 answer gave, if one parsed.
      pub(crate) fn background(&self) -> Option<Background>;
      /// Ctrl-C (0x03) or Ctrl-Z (0x1a), if one was read: it ends detection at once.
      pub(crate) fn control(&self) -> Option<Control>;
  }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub(crate) enum Control { Interrupt, Suspend }
  /// `11;rgb:R/G/B` (or `rgba:R/G/B/A`), each 1–4 hex digits, either case: relative luminance
  /// ≥ 0.5 is light (spec §4.2).
  pub(crate) fn background_of(body: &[u8]) -> Option<Background>;
  /// Reads `fd` byte by byte until the first of: the DA1 answer; Ctrl-C or Ctrl-Z; `QUIET`
  /// with no byte; `LIMIT` after `started`; `stop()`. Waits in 50 ms slices (T2-2), never past those bounds,
  /// and reads nothing past the DA1 answer.
  pub(crate) fn read(fd: RawFd, started: Instant, stop: &dyn Fn() -> bool) -> Replies;
  // term.rs
  /// Writes `bytes` to the terminal and flushes, under the ownership lock: never inside a
  /// frame, and not at all once the terminal is given back (false).
  pub(crate) fn query(bytes: &[u8]) -> bool;
  // input.rs
  /// How the input thread asks for the background (spec §4.2): `ask` writes the query, and the
  /// replies come on `fd`.
  pub(crate) struct Detection { pub(crate) ask: Box<dyn FnOnce() -> bool + Send>, pub(crate) fd: RawFd }
  pub(crate) fn spawn(tx: Sender<Msg>, detect: bool) -> io::Result<Input>;
  pub(crate) fn spawn_with(tx: Sender<Msg>, events: impl Events, detection: Option<Detection>) -> io::Result<Input>;
  // msg.rs
  /// What the terminal said its background is (spec §4.2); `None` when it said nothing.
  Detected { background: Option<Background> },
  ```
- **The parser.**
  - In ground, a byte that is not ESC is dropped (§4.2: detection owns every byte it reads).
  - **0x03 and 0x1a, in any state,** end the read at once and are kept as `control()`. No reply contains either byte, and in raw mode they are Ctrl-C and Ctrl-Z, which §4.2 says always work.
  - `ESC ]` starts an OSC. Its body ends at BEL, `ESC \` or 0x9C. An ESC inside the body that is not followed by `\` starts a new sequence.
  - `ESC [` starts a CSI, which ends at its first byte in `0x40..=0x7E`. A CSI that began with `?` and ends with `c` is the DA1 answer.
  - `ESC` followed by any other byte drops both.
  - An OSC body longer than 64 bytes is kept being consumed to its end, but it is no colour.
- **Luminance.** `c` is each component scaled to 0–1 by `n / (16^digits − 1)`. Linearize it as sRGB: `c ≤ 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055)^2.4`. Then `L = 0.2126 R + 0.7152 G + 0.0722 B`.
- **`spawn(tx, detect)`** builds a `Detection` only when `detect` holds and `tagteam_provider::term::stdin_is_our_foreground_terminal()` does too (§4.2: never on a non-controlling terminal). Its `ask` is `|| term::query(QUERY)`, and its `fd` is `libc::STDIN_FILENO`.
- **The thread**, given a `Detection`:
  1. If `ask()` is true, it runs `detect::read(fd, Instant::now(), &stop)`, where `stop` is true once the control's mode is anything but `Reading`.
  2. It sends `Msg::Detected { background }`.
  3. If `control()` is set, it then sends that key exactly as it sends a key crossterm read: `KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)` or `'z'`, through the same message path. Ctrl-C quits and Ctrl-Z suspends, as at any other time.
  4. Then it runs `read` as today, so the first crossterm call comes after detection.
- **`update`**, for `Detected { background: Some(b) }`, sets `state.theme = state.theme.truecolor(b)` and `dirty`. For `None` it does nothing. The tier never changes after that.
- **Tier 2's values here:**
  - `Warn` is `#e5c07b` / `#9a6700`, and `Crit` is `#f07178` / `#cf222e`, as `Color::Rgb` (dark / light).
  - The rest of tier 2 is tier 1's until the task that adds the role.

- [ ] **Step 1: Write the unit tests.**
  - **`detect.rs`**, over `UnixStream::pair()`, writing from the peer:
    - `an_osc_11_answer_then_da1_gives_the_background`: `\x1b]11;rgb:2828/2c2c/3434\x1b\\\x1b[?62;22c` gives `Dark`; `rgb:ffff/ffff/ffff` with BEL gives `Light`. Both return in under 100 ms.
    - `da1_alone_gives_no_background`.
    - `a_silent_terminal_is_given_up_after_a_second`: nothing written; returns in [1.0 s, 1.3 s).
    - `a_trickle_is_given_up_after_two_seconds`: a byte outside any reply every 300 ms; returns in [2.0 s, 2.3 s).
    - `a_stop_ends_detection_within_a_slice`: `stop` turns true at 100 ms; returns before 200 ms (Review Focus 2).
    - `ctrl_c_or_ctrl_z_ends_detection_at_once_and_is_kept`: `\x03` written at 100 ms on a silent peer returns before 200 ms with `control() == Some(Interrupt)`; `\x1a` gives `Suspend`. Again with each written in the middle of an OSC body (`\x1b]11;rgb:28`), which also leaves `handoff()` at `Osc` (Task 4).
    - `a_key_before_and_between_the_replies_is_dropped_and_the_replies_still_read`: `j`, the OSC answer, `k`, then DA1 gives `Dark`.
    - `a_reply_split_across_writes_is_read_whole`: the OSC answer in two writes 50 ms apart, then DA1.
    - `nothing_past_the_da1_answer_is_read`: DA1 then `q` in one write; afterwards the peer's `q` is still readable on the fd (`wait_readable` then `read_byte` gives `b'q'`).
    - `odd_colour_replies_parse_or_leave_tier_1` (Review Focus 1):
      - `rgb:28/2c/34` and `rgb:FFFF/FFFF/FFFF` parse;
      - `rgba:ffff/ffff/ffff/ffff` parses;
      - an answer ended by 0x9C parses;
      - `rgb:zz/zz/zz`, `rgb:1/2`, a 200-byte body and `11;?` give `None`;
      - none panics, and each is followed by DA1 ending the read.
  - **`theme.rs`:**
    - `tier_2_draws_warn_and_crit_in_its_own_hues`: `Theme::new(true).truecolor(Dark).role(Warn).fg == Some(Color::Rgb(0xe5, 0xc0, 0x7b))`, and similarly for the other three values.
    - `no_colour_stays_no_colour_whatever_the_background`: `Theme::new(false).truecolor(Light) == Theme::new(false)`.
  - **`input.rs`:** `detection_runs_before_the_first_terminal_read`.
    - The `Script` fake, plus a `Detection` over a socket pair whose `ask` records that it ran.
    - Until the peer writes DA1, the script sees no `next` call.
    - After DA1, the thread sends `Msg::Detected { background: None }` before any key.
  - **`input.rs`:** `a_ctrl_c_during_detection_reaches_update_right_after_it`: the peer writes `\x03` while detection waits; the thread sends `Detected { background: None }`, then the Ctrl+C key message, before its first `next` call.
  - **`runtime.rs`:** `the_first_frame_draws_before_detection_and_the_next_in_tier_2`.
    - With `color: true`, `ui.start()` draws one frame while `ui.state.theme == Theme::new(true)`.
    - After `Msg::Detected { background: Some(Dark) }` on the fixture's channel, a second frame is drawn with `ui.state.theme == Theme::new(true).truecolor(Background::Dark)`. Use `FakeTty`'s `on_draw` to send `q` after the second frame.
- [ ] **Step 2: Write the pty tests** in `tui_pty.rs`, with `COLORTERM=truecolor` on `home.command(…)` unless said otherwise:
  - `the_colour_query_is_asked_once_and_an_answer_lets_the_keys_through`: after `wait_for_output(LONG, "the query", QUERY)`, type the OSC and DA1 answers (`rgb:2828/2c2c/3434`), then `3`. History shows within 2 s.
  - `a_terminal_that_answers_only_da1_keeps_the_keys_working`: DA1 only, then `3`, then History.
  - `a_silent_terminal_is_given_up_after_a_second`: answer nothing, sleep 1.2 s, type `3`, then History.
  - `without_colorterm_or_with_no_color_nothing_is_asked`: without `COLORTERM`, and with `COLORTERM=truecolor` plus `NO_COLOR=1`, once the accounts show and 300 ms pass, `output()` does not contain `QUERY`.
  - `a_key_before_the_colour_answer_is_dropped`: after the query, type `3`, then both answers, then `2`. Tokens shows, and `tui.first_shown(|s| s.contains("No usage history."))` is `None`.
  - `a_key_between_the_two_answers_is_dropped`: the OSC answer, `3`, DA1, then `2`. As above.
  - `an_answer_split_across_reads_is_read_whole`: the OSC answer in two writes 100 ms apart, DA1, then `3`. History shows.
  - `sigterm_while_the_terminal_ignores_the_colour_query_exits_at_once`: after the query, `SIGTERM`. It exits 143 within 500 ms, with the terminal restored (Review Focus 2).
  - `ctrl_c_while_the_terminal_ignores_the_colour_query_quits_at_once`: after the query, type `\x03`. It exits 0 within 500 ms, with the terminal restored.
- [ ] **Step 3: Write the perf case** `tui_first_frame_p95_is_within_50_ms_while_the_terminal_is_asked_its_colours` in `perf.rs`. It is `first_frames`' run with `COLORTERM=truecolor`. After the first frame shows, it waits for `QUERY`, types `\x1b[?62c` (DA1 only), then `q`. Expected p95 ≤ 50 ms.
- [ ] **Step 4: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked -- detect:: theme:: detection_runs the_first_frame_draws`. Expected: a compile error.
- [ ] **Step 5: Implement** as listed in Files and Interfaces.
- [ ] **Step 6: Run the tests.**
  - Run `cargo test -p tagteam-tui --locked`.
  - Outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty`.
  - Run `cargo test -p tagteam --locked --features test-support --test perf --no-run` (the timing run is the controller's, at the end).
  - Expected: all pass, the T1 drain tests (`a_key_behind_several_terminal_replies_is_handled`) included.
- [ ] **Step 7: Commit.** Run `git add -A crates/tagteam-tui crates/tagteam/src/tui_cmd.rs crates/tagteam/tests && git commit -m "Ask the terminal its background before the first key is read"`.

### Task 4: Drop a colour reply that detection did not finish

**Depends on:** Task 3

**Files:**
- Create: `src/replies.rs`: the filter.
- Modify:
  - `src/lib.rs`: add `mod replies;`.
  - `src/detect.rs`: `Replies::handoff` and `Replies::answered`.
  - `src/input.rs`: the thread starts its filter from detection's result. `step` (`:371`) and `message` (`:413`) pass each key through it, and the thread sends `Msg::KeysHeld`. `Events::wait` is unchanged: the filter never needs a timer.
  - `src/msg.rs`: add `Msg::KeysHeld`.
  - `src/update.rs`: apply it.
  - `src/state.rs`: `pub(crate) keys_held: bool`.
  - `src/view.rs:195-204`: `banner` returns the held-keys words first (T2-14).
- Test: `src/replies.rs`, `src/input.rs`, `crates/tagteam/tests/tui_pty.rs`.

**Interfaces:**
- Produces:
  ```rust
  // detect.rs
  /// Where a reply stood when detection stopped (spec §4.2): the filter's start.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub(crate) enum Handoff { None, Esc, Csi, Da1, Osc, OscEsc }
  impl Replies {
      pub(crate) fn handoff(&self) -> Handoff;
      /// Whether the read ended at the DA1 answer: every reply asked for has then ended.
      pub(crate) fn answered(&self) -> bool;
  }
  // replies.rs
  pub(crate) struct ReplyFilter { /* mode, state, held keys */ }
  impl ReplyFilter {
      /// No query was asked, or its DA1 answer was read: calm.
      pub(crate) fn calm() -> ReplyFilter;
      /// Detection ended without the DA1 answer: wary, starting where `handoff` says.
      pub(crate) fn wary(handoff: Handoff) -> ReplyFilter;
      /// Judges one key, and returns the keys to pass now, in order: released held keys first.
      pub(crate) fn key(&mut self, key: KeyEvent) -> Vec<KeyEvent>;
      /// Fail-closed: inside an OSC, or a DA1 cut off at the hand-off, until its terminator.
      pub(crate) fn holding(&self) -> bool;
  }
  // msg.rs
  /// The input thread drops every key until an unfinished colour reply ends (spec §4.2).
  KeysHeld { held: bool },
  ```
- **The thread's filter** is `calm()` without a `Detection`, or when `answered()`; otherwise `wary(handoff())`.
- **The filter's states.** These terms apply in the table:
  - "Hold": keep the key back until a later key decides. Nothing releases a held key but a key.
  - "Release": pass the held keys in order, then judge this key as in Open.
  - "Drop": drop this key and every held one.

  Ctrl+C and Ctrl+Z pass at once in every state and mode. They drop the held keys and leave the state as it is.

  | State | Key | Next state |
  |---|---|---|
  | Open | Alt+`]` | O(0), hold |
  | Open | Esc, while wary | E, hold |
  | Open | anything else (Esc too, while calm) | Open, pass |
  | E | `]` | O(0), hold |
  | E | `[` | C, hold |
  | E | Esc | E: pass the held Esc, hold this one |
  | E | anything else | release |
  | O(n) | the n-th char of `11;` | O(n+1), hold; at 3: Osc, drop |
  | O(n) | anything else | release |
  | C | `?` | Late, drop |
  | C | anything else | release |
  | Late | a digit or `;` | Late, drop |
  | Late | `c` | Open and calm, drop |
  | Late | anything else | Open, and the key is judged as in Open |
  | Osc | Ctrl+G (BEL), or Alt+`\` | Open, drop |
  | Osc | Esc | OscEsc, drop |
  | Osc | anything else, a bare `\` included | Osc, drop |
  | OscEsc | `\`, Ctrl+G or Alt+`\` | Open, drop |
  | OscEsc | Esc | OscEsc, drop |
  | OscEsc | anything else | Osc, drop |
  | Da1 | `c` | Open and calm, drop |
  | Da1 | anything else | Da1, drop |
  | Ch | `?` | Da1, drop |
  | Ch | anything else | Open, drop |

  - **`wary(handoff)`** maps `None` to Open, `Esc` to E, `Csi` to Ch, `Da1` to Da1, `Osc` to Osc and `OscEsc` to OscEsc, with nothing held. Ch is a CSI that detection cut off after `ESC [`.
  - **`holding()`** is true in Osc, OscEsc and Da1: a reply cut off at the hand-off fails closed, as §4.2 requires.
  - **Late** is a DA1 answer that began after detection gave up. Its body can only be digits and `;`, so any other key is the person's and is judged afresh. Its `c` ends the last reply asked for, so the filter turns calm.
  - **A filter stays wary** until a DA1 answer ends through it. crossterm swallows a DA1 answer that arrives in one read, so a terminal that answers late in one read leaves it wary for the session. While wary, a lone Esc acts when the next key arrives (T2-3).
- **The banner words:** `the terminal sent an unfinished colour report · keys resume when it ends · Ctrl-C quits`. Task 6 adds its `! `.
- **The thread** keeps `told: bool`.
  - When a drain ends (`Next::Empty`) while `holding()` and not `told`, it sends `KeysHeld { held: true }`.
  - When the filter leaves holding after `told`, it sends `KeysHeld { held: false }`.

- [ ] **Step 1: Write the unit tests** in `replies.rs`:
  - `after_an_osc_hand_off_every_key_drops_until_its_terminator`, from `wary(Osc)`:
    - `j`, `b`, `2`, a bare `\` and Alt+`x` drop;
    - Ctrl+C and Ctrl+Z pass;
    - Ctrl+G ends it, and the next `j` passes.

    Again ended by Alt+`\`, by Esc then `\`, and by Esc then Ctrl+G. Esc then `j` keeps it holding. `wary(OscEsc)` then `\` ends it.
  - `after_a_da1_hand_off_keys_drop_until_its_c`, from `wary(Da1)`; afterwards the filter is calm, and Esc passes at once.
  - `after_an_esc_hand_off_the_next_key_decides`, from `wary(Esc)`:
    - `]` then `11;` makes it hold until BEL;
    - `[` then `?` drops until `c`;
    - `x` passes at once.
  - `an_unasked_osc_answer_is_dropped_whole`, both `calm()` and `wary(None)`: Alt+`]`, `1`, `1`, `;`, `r`, `g`, `b`, `:`, `2`, `b` and Alt+`\` give nothing, and the next `3` passes.
  - `a_split_osc_answer_is_dropped_whole`, from `wary(None)`: Esc, `]`, `1`, `1`, `;`, the body and Ctrl+G give nothing, and the next `3` passes. The filter has no clock, so the time between those keys cannot change this.
  - `a_split_da1_answer_after_detection_is_dropped`, from `wary(None)`: Esc, `[`, `?`, `6`, `2`, `;`, `2`, `2`, `c` give nothing. Then `3` passes, and an Esc after it passes at once (calm).
  - `while_wary_a_person_s_keys_after_esc_pass_in_order`:
    - Esc gives nothing, and `j` then gives `[Esc, j]`;
    - Esc, `]`, `j` gives `[Esc, ], j]` at `j`;
    - Esc, `[`, `j` gives `[Esc, [, j]`.
  - `while_calm_esc_and_brackets_pass_at_once`, from `calm()`: Esc gives `[Esc]`, and `]` gives `[]]`.
  - `ctrl_c_and_ctrl_z_pass_in_every_state`: in each state and mode, Ctrl+C gives `[Ctrl+C]`.
- [ ] **Step 2: Write the thread tests** in `input.rs`:
  - `a_held_reply_is_told_once_and_its_end_once`:
    - The `Detection` reads a socket pair whose peer writes `\x1b]11;rgb:` and nothing more, so detection hands off `Osc` after `QUIET`.
    - The `Script` then yields `j`, then `Empty`, then, after a wait, Ctrl+G and `k`.
    - The thread sends `KeysHeld { held: true }` once, then `KeysHeld { held: false }`, then `k`. It never sends `j`.
  - `after_an_answered_query_esc_is_sent_at_once`: detection reads a DA1 answer, and the `Script` yields Esc then `Empty`; the thread sends the Esc key within 100 ms.
- [ ] **Step 3: Write the pty tests** in `tui_pty.rs`, with `COLORTERM=truecolor`.
  - **The constants.** `ANSWER` is `\x1b]11;rgb:2b3b/2b3b/2b3b\x1b\\`. Its digits `2` and `3` would show Tokens or History if they leaked, and its `b` would switch.
  - **"The give-up"** means: wait for the query, answer nothing, and sleep 1.3 s.
  - `an_answer_after_the_give_up_never_reaches_the_keys`: after the give-up, type `ANSWER` and `\x1b[?62c` in one write. Then type `3`: History shows. `first_shown` never saw Tokens before it.
  - `a_delayed_split_answer_never_reaches_the_keys`:
    - after the give-up, write `\x1b` alone, and 500 ms later `]11;rgb:2b3b/2b3b/2b3b\x07`;
    - type `3`: History shows.

    `first_shown` never saw Tokens, and `live_email(home.root()) == "b@x.co"`.
  - `a_delayed_split_da1_answer_never_reaches_the_keys`:
    - after the give-up, write `\x1b` alone, and 500 ms later `[?62;22c`;
    - type `3`: History shows.

    Help (`keys and marks`) and Tokens never showed.
  - `a_delayed_split_answer_leaves_an_open_dialog_open`:
    - after the give-up, press `x`, and wait for the remove question;
    - write `\x1b`, and 500 ms later `]11;rgb:2b3b/2b3b/2b3b\x07`;
    - after 300 ms more, the question still shows;
    - `n` closes it.
  - `after_an_answer_esc_acts_at_once`: answer `ANSWER` and `\x1b[?62c`, press `?`, wait for help, then `\x1b`: help closes within 300 ms.
  - `a_truncated_answer_whose_end_comes_late_holds_the_keys_until_it_ends`:
    1. `\x1b]11;rgb:2b3b/`; the banner `the terminal sent an unfinished colour report` shows within 2 s.
    2. At 2.5 s, `3` (dropped).
    3. Then `2b3b/2b3b\x1b\\`: the banner goes.
    4. Then `2`: Tokens shows.

    History never showed, and `live_email(home.root()) == "b@x.co"`.
  - `a_typed_backslash_does_not_end_a_held_answer`: `\x1b]11;rgb:2b3b/`; after the banner, type `\`, then `3` (dropped), then `2b3b/2b3b\x1b\\`: the banner goes; then `2`: Tokens shows, and History never did.
  - `a_da1_answer_cut_before_its_c_holds_the_keys_until_the_c`: `ANSWER`, then `\x1b[?6`; after 1.3 s `2;22c`; then `3`. History shows, and Tokens never did.
  - `an_answer_that_never_ends_still_lets_ctrl_c_quit`: `\x1b]11;rgb:2b3b/`; after the banner, `\x03`. It exits 0 with the terminal restored.
- [ ] **Step 4: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked -- replies:: a_held_ after_an_answered`. Expected: a compile error.
- [ ] **Step 5: Implement** as listed in Files and Interfaces.
- [ ] **Step 6: Run the tests.** Run `cargo test -p tagteam-tui --locked`; outside the sandbox, `cargo test -p tagteam --locked --features test-support --test tui_pty`. Expected: all pass, T1's input tests unchanged.
- [ ] **Step 7: Commit.** Run `git add -A crates/tagteam-tui crates/tagteam/tests/tui_pty.rs && git commit -m "Drop a colour reply that detection did not finish"`.

### Task 5: Read render tests' body rows from the body's own area

**Depends on:** none. A test-only change: no production line changes.

**Files:**
- Modify: `src/testutil.rs`. Add `body_of`.
- Modify, mechanically, every test that asserts a drawn body row by its absolute index (`lines[n]`, `drawn[n]`, `draw(…)[n]` for a row of the body):
  - `screens/accounts.rs`, `screens/history.rs`;
  - `screens/tokens/table.rs`, `screens/tokens/filter.rs`;
  - `view.rs`, `actions.rs`, `update.rs`.

  Each reads the body's row `n − body.y` from `body_of(…)` instead. Rows of the chrome (header, banner, footer) keep their absolute index.

**Interfaces:**
- Produces:
  ```rust
  /// What `view` draws of `state` in the body's rows alone (`view::areas`), at `w`×`h` and `now_ms`, trailing spaces trimmed.
  pub(crate) fn body_of(state: &State, w: u16, h: u16, now_ms: i64) -> Vec<String>;
  ```

- [ ] **Step 1: Add `body_of`, and move the asserts onto it**, keeping every expected string as it is.
- [ ] **Step 2: Check it changes no expectation.** Run `git diff -U0 HEAD -- crates/tagteam-tui/src | rg '^\+' | rg -v 'body_of|\[[0-9]+\]|^\+\+\+'`. Expected: only the helper's own lines.
- [ ] **Step 3: Run the crate's tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass.
- [ ] **Step 4: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Read render tests' body rows from the body's own area"`.

### Task 6: Say auto's state in the header and its last action on the auto line

**Depends on:** Tasks 1, 2, 3, 5

**Files:**
- Create:
  - `src/chrome.rs`: the header row, the auto line's row, the rule, and the banner's rows and style.
  - `src/autoline.rs`: what the auto line says (§5.3, T2-7).
- Modify:
  - `src/lib.rs`: add `mod autoline; mod chrome;`.
  - `src/theme.rs`: add `Role::{Text, Muted, Faint, Accent}` and §4.1's values for every role in every tier. Tier 0's `Crit` becomes bold and underline.
  - `src/view.rs`:
    - `Areas` and `areas` (`:49-88`) become §5.1's rows: header 1, auto 1, the rule from 30 rows, the banner 0–3, the body, the footer 1–3. `activity` goes.
    - `view` (`:240-275`) draws them through `chrome`.
    - Delete `header`, `right_aligned`, `activity`, `MODE_TIMER` and `HELP_HINT`. `banner` moves into `chrome`, rewritten (T2-14).
  - `src/daemon/mod.rs`:
    - delete `Activity` and `DaemonState.activity`/`activity()`;
    - `note` (`:287-316`) records `Noted` per provider instead;
    - `up` and `down` clear it.
  - `src/daemon/words.rs`:
    - `mode` and `job_words` (`:16-55`) become `auto_state`;
    - delete `activity`, `auto` and `binding` (`:78-184`).

    `iso_ms` and `clock` stay.
  - `src/state.rs`: `State::new` takes `names: ProviderNames` (the default provider's, §12.2's last bullet), kept as `fallback_names`. Add `names_of`.
  - `src/runtime.rs`: `TuiOptions` gains `names: ProviderNames`, passed to `State::new`.
  - `crates/tagteam/src/tui_cmd.rs`: `names: ProviderNames::of(&*probe.provider(&tokens_provider)?)`. A failure prints `tagteam: {e}` and returns `EXIT_ERROR`, as the `--provider` check above it does.
  - `src/testutil.rs`:
    - `fresh` passes `CC_NAMES`;
    - add `roles` and `canon`.
- Modify, mechanically: the runtime tests' `TuiOptions` literals (`runtime.rs:1063, 1120, 1297, 1325`) gain `names: CC_NAMES`.
- Rewrite tests to the new layout:
  - **`view.rs`**, rewritten in `chrome.rs`:
    - `the_header_shows_the_tabs_and_the_mode_and_the_footer_the_screens_keys` (header half; its footer half waits for Task 9);
    - `in_live_mode_the_header_says_the_daemon_s_state_from_its_status_alone`;
    - `with_several_providers_the_header_follows_the_selected_account_s`;
    - `a_mode_too_wide_for_the_row_drops_help_and_then_takes_a_row_of_its_own`, now the one-row header;
    - `timer_mode_says_when_the_daemon_stopped_or_speaks_another_protocol`;
    - `an_error_banner_is_red_with_colour_and_bold_without`, now roles;
    - `a_long_banner_wraps_onto_three_rows_at_most_and_the_body_gives_them_up`, with the rows moved under the auto line.
  - **Rewritten in `autoline.rs`:**
    - `view.rs`'s `the_activity_line_shows_the_latest_event_above_the_footer_in_live_mode_only`, `a_switch_names_its_window_only_from_a_poll_of_its_own_link` and `with_several_providers_the_activity_line_names_the_event_s_provider`;
    - `daemon/words.rs`'s `the_activity_line_says_each_event_in_auto_s_words_and_leaves_out_four_kinds` and `a_switch_names_the_window_that_bound_it_at_its_tick_s_poll`.
  - **`daemon/words.rs`**, `each_daemon_state_of_spec_7_4_has_its_words_with_and_without_a_keychain_backoff`: §5.2's table, both widths.
  - **The colour tests** at `accounts.rs:1445`, `history.rs:842` and `table.rs:1551`: tier 0's crit now asserts bold and underline.
  - **The body is shorter:** 21 rows at 80×24 (was 22), and 36 at 120×40 (was 38). A test that filled the body to its last row, or scrolled at it, changes its fixture (one account or row fewer, or one more), per Global Constraints, never its assertion.
  - **`theme.rs`**, `tier_1_and_no_colour_draw_today_s_styles`: becomes `every_role_has_section_4_1_s_style_in_every_tier`.
  - **The pty predicates** in `tui_pty.rs`:
    - `"● live · auto on"` → `"● auto on"`;
    - `"● live · auto paused"` → `"‖ auto paused"`;
    - `"○ daemon stopping (signal)"` → `"! daemon stopping (signal)"`.
  - **Task 3's pty tests:** `the_colour_query_is_asked_once…` also asserts that the output after the answer contains `38;2;` (the header's accent in tier 2). `a_terminal_that_answers_only_da1…` asserts it never does. A new `the_first_frame_draws_before_a_late_answer_and_the_next_in_truecolor` answers 300 ms after the accounts show: `38;2;` appears only in output written after the answer.

**Interfaces:**
- Produces:
  ```rust
  // theme.rs
  pub(crate) enum Role { Text, Muted, Faint, Accent, Warn, Crit }   // Ok: Task 10
  // state.rs
  impl State {
      /// `provider`'s names: from the accounts view, else the default provider's (`TuiOptions.names`).
      pub(crate) fn names_of(&self, provider: &ProviderId) -> ProviderNames;
  }
  // chrome.rs
  pub(crate) fn header(state: &State, width: u16) -> Line<'static>;
  pub(crate) fn auto_line(state: &State, width: u16, now_ms: i64) -> Line<'static>;
  pub(crate) fn rule(state: &State, width: u16) -> Line<'static>;
  /// The banner's words and role (T2-14), its rows as `banner_rows` wraps them.
  pub(crate) fn banner(state: &State) -> Option<(String, Role)>;
  // daemon/words.rs
  /// §5.2's state for the focus provider, long from 120 columns; each piece with its role.
  pub(crate) fn auto_state(state: &State, wide: bool) -> Vec<(String, Role)>;
  // daemon/mod.rs
  /// The daemon's latest decision for a provider on this link (spec §5.3's rank 2).
  pub(crate) struct Noted { pub(crate) at_ms: i64, pub(crate) what: NotedWhat }
  pub(crate) enum NotedWhat {
      NoSwitch { reason: NoSwitchReason, detail: String, used_pct: Option<f64>, frees_at_ms: Option<i64> },
      Error(String),
      Warning(String),
  }
  impl DaemonState { pub(crate) fn noted(&self, provider: &str) -> Option<&Noted>; }
  // autoline.rs
  pub(crate) struct Said { pub(crate) mark: Option<(&'static str, Role)>, pub(crate) when: Option<String>, pub(crate) text: String, pub(crate) muted: bool }
  pub(crate) fn said(state: &State, width: u16, now_ms: i64) -> Said;
  /// §5.3's time form in `tz` (T2-24).
  pub(crate) fn when(at_ms: i64, now_ms: i64, tz: &TimeZone) -> String;
  // testutil.rs
  /// Row `y` of `buf` as role letters: `c w a o f m` (crit, warn, accent, ok, faint, muted), the
  /// first whose non-empty style the cell's contains (fg equal where the role sets one, its
  /// modifiers present), else `t`; a space cell is ` `.
  pub(crate) fn roles(buf: &Buffer, theme: Theme, y: u16) -> String;
  /// An expected row in the same letters, collapsed as `theme` draws them (tier 0 shares
  /// styles): each letter's role drawn in one cell and read back by `roles`; `B` is bold text.
  pub(crate) fn canon(theme: Theme, row: &str) -> String;
  ```
- **The header** is ` tagteam   1 Accounts   2 Tokens   3 History`, then the state, three spaces, `? keys` and one space, right-aligned.
  - The brand is bold. The tabs are muted, and the current tab is accent and bold.
  - `?` is accent and `keys` muted.
  - The state is cut with `fit` to the room left; `? keys` is never dropped.
  - §5.2's roles: `●` accent; `‖` and `!` warn; `○` muted; the words text; what follows ` · ` muted; the backoff suffix warn.
  - The threshold (written by `words::threshold`) and the strategy are the focus provider's `ProviderAccounts`'. Before the first snapshot, the ` · switches at …` suffix is left out.
  - T2-11 and T2-12 apply.
- **The auto line** is ` {mark} {when}  {text}`, rank 3's ` ⇄ last switch {when} · {text}`, or rank 4's `   no switches in the last 30 days`, then ` · next check HH:MM` from 120 columns in live mode with the focus job in `auto`.
  - The mark is in its role, the time muted, the text in text, and the suffix muted.
  - The provider is `daemon::focus`'s.
  - Names are `words::short_name` of the account at that position in the snapshot, else the position alone.
  - A switch's text is `auto switched {from} → {to} ({words::trigger})`.
  - Rank 2's text is `words::no_switch` with `NoSwitchFacts`:
    - `used_pct` from the link's last `poll` for the provider, which `note` reads when the `no-switch` arrives;
    - `frees_at` from the following `all-exhausted`'s `earliestResetAt` in `clock` form;
    - `display_name` from `names_of`;
    - the threshold from the view.
  - `status.lastOutcome` gives `NoSwitch` (no detail) for kind `no-switch`, and `Error(reason)` for kind `error`.
  - The line is cut with `fit`.
- **The rule** is `─` across the width in `Faint`, drawn when the terminal has at least 30 rows.

- [ ] **Step 1: Write the tests.**
  - **`theme.rs`**, `every_role_has_section_4_1_s_style_in_every_tier`: §4.1's table, row by row, for `TIERS`.
  - **`chrome.rs`:**
    - `the_header_says_each_state_of_spec_5_2_with_keys_always_there`. For each row below, at 120 and 80 columns, row 0 ends with `{state}   ? keys` and starts ` tagteam   1 Accounts   2 Tokens   3 History`:

      | Case | 120 | 80 |
      |---|---|---|
      | live `auto` | `● auto on · switches at 90%` | `● auto on · at 90%` |
      | `auto`, strategy `ConsumeFirst` in the view | `● auto on · uses the soonest reset first` | `● auto on · soonest reset` |
      | `collect`, autoswitch false | `‖ auto paused · P resumes` | `‖ paused · P resumes` |
      | `collect`, autoswitch true | `○ auto idle · needs 2 usable accounts` | `○ needs 2 usable accounts` |
      | `waiting` `engine-lock`, pid 4711 | `● auto runs in another terminal (pid 4711)` | `● auto in pid 4711` |
      | `waiting` `engine-lock`, no pid | `● auto runs in another terminal` | `● auto in another terminal` |
      | `waiting` `schema` | `! daemon is outdated · restarting` | `! daemon outdated` |
      | `idle` | `○ auto off · no usable account` | `○ no usable account` |
      | `auto`, keychain backoff until 15:05 | `● auto on · switches at 90% · keychain locked until 15:05` | `! keychain locked 15:05` |
      | no job for the provider | `○ auto off` | `○ auto off` |
      | an unknown mode, `added-later` | `○ daemon added-later` | `○ daemon added-later` |
      | `stopping` (upgrade) | `! daemon stopping (upgrade)` | `! daemon stopping` |
      | protocol mismatch | `! daemon version unsupported` | `! daemon unsupported` |
      | no daemon | `○ auto off · no daemon` | `○ auto off · no daemon` |

      Before the daemon's first `status` (`await_daemon`), row 0 ends with `3 History`, a gap and `? keys`, with no state.
    - `the_header_s_roles_follow_its_marks`, for `TIERS` via `roles`/`canon`, on the `auto`, `collect` and backoff rows: the brand `B`, the tabs `m`, the current tab `a`, `●` `a`, `‖` `w`, words `t`, ` · switches at 90%` `m`, the backoff `w`.
    - `the_rows_above_the_body_are_the_same_in_both_modes`: `areas` at 80×24 and 120×40 gives the same `body` in timer mode, in live mode and after `Live::Down` (UX-5).
    - `from_30_rows_a_faint_rule_sits_under_the_chrome`: at 120×40, row 2 is `─` × 120 and its roles are `f`; at 80×24 the body starts at row 2.
    - `a_failed_read_is_crit_and_anything_else_warn`: an `Accounts` banner reads ` ✕ the store is locked` in `c`; a `Collect` one ` ! usage was not collected: …` in `w`; the held-keys banner comes first when both are raised (T2-14).
    - `every_screen_s_chrome_in_each_tier`: Accounts, Tokens and History at 80×24 and 120×40, for `TIERS`. The header, auto line and rule rows have the roles above.
  - **`autoline.rs`**, with `daemon_job("claude-code", "auto", true)` live, `many(3)`, `TimeZone::UTC` and `NOW_MS` (Monday 2026-09-21 14:13:20):
    - `the_daemon_s_last_switch_is_worded_from_its_trigger`: `last_switch` 14:11:20, 1 → 2, `proactive` gives row 1 ` ⇄ 14:11  auto switched 1 a1 → 2 a2 (before it reached the limit)`, `⇄` in `a`.
    - `a_poll_or_usage_never_replaces_a_recent_switch`: then a `poll`, a `usage` and a `no-switch` `cooldown` event: row 1 is unchanged (§16).
    - `a_no_switch_shows_in_words_without_a_recent_switch`:
      - with no `last_switch`, a `no-switch` `cooldown` `4m` gives `   14:13  waiting 4m after the last switch`;
      - after a `poll` with `active.number` 1 and `headroomPct {"1": 23}`, a `below-threshold` gives `   14:13  no switch needed (77%, switches at 90%)`.
    - `all_exhausted_names_when_the_first_frees_up`: a `no-switch` `all-exhausted`, then `all-exhausted` with `earliestResetAt` 16:40Z, gives ` ✕ 14:13  every account is near its limit; the first frees up at 16:40`, `✕` in `c`.
    - `an_error_or_a_config_warning_shows_its_message`: ` ! 14:13  the store is locked`.
    - `the_last_outcome_fills_the_line_on_connect`: `last_outcome` `no-switch` `cooldown` at 14:10 gives `   14:10  waiting after the last switch`; a later live event replaces it.
    - `an_older_switch_reads_as_the_last_switch_with_its_day`: `last_switch` Sat 2026-09-19 15:38, 3 → 1, gives ` ⇄ last switch Sat 15:38 · auto switched 3 a3 → 1 a1 (before it reached the limit)`; on 2026-09-13 it gives `Sep 13 15:38`.
    - `with_nothing_it_says_no_switches_in_the_last_30_days`: timer mode, and live mode with a `last_switch` 31 days old: `   no switches in the last 30 days`, in `m`.
    - `next_check_shows_from_120_columns_with_auto_on`: `next_tick_at` 14:16Z adds ` · next check 14:16` at 120 columns, but not at 80, and not when the job is `collect`.
    - `the_line_follows_the_selected_account_s_provider`: with a fake-agent job too, selecting the fake account shows the fake job's line.
    - `times_read_hh_mm_today_the_weekday_this_week_and_the_date_before`: `when` at 14:11 today, at Sat 15:38, and at Sep 13 15:38.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked -- chrome:: autoline:: theme::`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed in Files and Interfaces, and rewrite the listed tests.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-tui --locked` and `cargo test -p tagteam --locked --features test-support --lib`; outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui crates/tagteam/src/tui_cmd.rs crates/tagteam/tests/tui_pty.rs && git commit -m "Say auto's state in the header and its last action on the auto line"`.

### Task 7: Name a foreground tagteam auto in the header

**Depends on:** Task 6

**Files:**
- Modify:
  - `src/source.rs`: `AccountsSnapshot` (`:31-42`) gains `auto_holders`. `EngineSource::accounts` (`:86-114`) fills it.
  - `src/daemon/words.rs`: `auto_state`'s timer-mode branch.
- Modify, mechanically: every `AccountsSnapshot { … }` literal (`testutil.rs:154`, `runtime.rs:800`, `daemon/mod.rs:744`) gains `auto_holders: vec![]`.
- Test: `crates/tagteam-tui/tests/source.rs`, `src/chrome.rs`.

**Interfaces:**
- Produces:
  ```rust
  pub struct AccountsSnapshot {
      // …
      /// Each listed provider whose engine lock names a live process other than the daemon:
      /// its pid (C-16, T2-11).
      pub auto_holders: Vec<(ProviderId, u32)>,
  }
  ```
- **What counts.** For each provider in `providers`, take `engine.engine_holder(p)`. A `Read::Present(stamp)` whose `stamp.liveness()` is not `Dead` counts. One whose pid and start equal `read_daemon_record(&daemon_lock_path(engine.env()))`'s does not.
- **The header.** In timer mode, a holder for the focus provider shows `● auto runs in another terminal (pid N)` (80: `● auto in pid N`). Live mode ignores `auto_holders`.

- [ ] **Step 1: Write the tests.**
  - **`tests/source.rs`**, `a_live_engine_lock_holder_is_named_and_a_dead_one_is_not`:
    - Write `<data_dir>/locks/autoswitch-claude-code.lock` with this process's `ProcessStamp::current()` (`{"pid":…,"start":…}`). `accounts(None)` gives `auto_holders == [(cc, own pid)]`.
    - With a `start` one less, the holder is dead, and the list is empty.
    - With `daemon.lock`'s record naming the same stamp, the list is empty.
  - **`chrome.rs`**, `a_foreground_auto_shows_in_timer_mode_only`: `auto_holders: [(cc, 4711)]` gives `● auto runs in another terminal (pid 4711)` at 120 and `● auto in pid 4711` at 80; live with `auto` gives `● auto on · …`.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked -- engine_lock_holder foreground_auto`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Name a foreground tagteam auto in the header"`.

### Task 8: Mark the selection with a bar, not reverse video

**Depends on:** Task 6

**Files:**
- Modify:
  - `src/theme.rs`:
    - add `sel_bg` (tier 2: `#2f3542` / `#e8edf2` as the background; else none) and `focus`;
    - delete `selection`.
  - `src/view.rs`: replace `highlight` (`:489-495`) with `select`.
  - `src/screens/accounts.rs:583`, `src/screens/tokens/table.rs:1271`: `select(row, name, width, state.theme)`, with the name span the account cell, or the first column (T2-15).
  - `src/screens/history.rs:288`: the shown account's line through `select`, its name bold.
  - `src/overlay/choose.rs`, `src/screens/tokens/filter.rs:327`: a two-column gutter, `▌` for the selected choice, its text bold, and `sel_bg`.
  - `src/overlay/confirm.rs:49-56` (the focused button) and `src/overlay/text.rs:57` (the cursor): `theme.focus()`.
- Rewrite tests to the new layout:
  - `accounts.rs:1462` (`the_selected_row_is_reverse_video_across_the_width`) becomes `the_selected_row_has_the_bar_and_a_bold_name_and_no_reverse_video`;
  - `actions.rs:1107` (`a_long_choice_wraps_under_its_text_and_is_selected_whole`) asserts the gutter's `▌` on its first row and the bold text on all three rows;
  - choose, filter and table tests whose expected rows now carry the two-column gutter;
  - `theme.rs`'s test of `selection`, rewritten for `focus`.

**Interfaces:**
- Produces:
  ```rust
  impl Theme {
      /// §4.1's `sel-bg`: the selected row's background in tier 2, nothing otherwise.
      pub(crate) fn sel_bg(self) -> Style;
      /// A focused button or the text cursor (§4.3, §9.1): accent fill in tiers 1 and 2
      /// (`bg` accent, `fg` the tier's background: `Color::Black` in tier 1, `surface`'s hex in
      /// tier 2), REVERSED in tier 0.
      pub(crate) fn focus(self) -> Style;
  }
  /// Spec §4.3: `▌` in accent over column 0, span `name` bold, and `sel_bg` across `width`.
  pub(crate) fn select(row: Line<'static>, name: Option<usize>, width: usize, theme: Theme) -> Line<'static>;
  ```
- R-2 is closed: `rg -n "highlight|selection\(\)|REVERSED" crates/tagteam-tui/src` finds only `theme.rs`'s tier 0 `focus`.

- [ ] **Step 1: Write the tests.**
  - **`view.rs`**, `select_puts_the_bar_in_column_0_bolds_the_name_and_tints_only_in_tier_2`, for `TIERS`:
    - column 0 is `▌` in `a`;
    - the name's cells are bold;
    - no cell is REVERSED;
    - in tier 2 every cell to `width` has `bg == Color::Rgb(0x2f, 0x35, 0x42)` (dark) or `(0xe8, 0xed, 0xf2)` (light), and in tiers 0 and 1 none has a background.
  - **The rewritten tests** listed in Files.
  - **`overlay/confirm.rs`**, `a_focused_button_is_filled_and_reversed_only_without_colour`.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked -- select_puts the_selected_row a_focused_button`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run the crate's tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Mark the selection with a bar, not reverse video"`.

### Task 9: Build the footer and the help from one key table

**Depends on:** Task 8

**Files:**
- Create: `src/keys.rs`: the key table (R-1, CC-O-9).
- Modify:
  - `src/lib.rs`: add `mod keys;`.
  - `src/chrome.rs`: add `footer` for the keys case, called from `view.rs`'s `footer` (`:346-378`). Delete `ACCOUNTS_KEYS` and `HISTORY_KEYS` (`view.rs:29-46`), and `screens/tokens/mod.rs`'s `footer_keys` (`:370-390`).
  - `src/overlay/help.rs`:
    - delete `HELP_GLOBAL`, `HELP_ACCOUNTS`, `HELP_TOKENS` and `HELP_HISTORY`;
    - help draws from `keys::KEYS` and `MARKS`, as a scrolling panel.
  - `src/overlay/panel.rs`: `panel` (`:142-160`) takes its rows as `Vec<Line<'static>>` instead of `text: &str`. Result and Doctor pass `panel_rows`' rows as lines; Help passes its own styled lines. `scrolled` and `furthest` count those rows.
  - `src/overlay/mod.rs`, `src/state.rs`, `src/update.rs:217-220`: `Overlay::Help` becomes `Overlay::Help { scroll: u16 }`.
  - `src/screens/accounts.rs:157-158`: drop `Char('g')` and `Char('G')` (UX-7).
- Modify, mechanically:
  - `Overlay::Help` → `Overlay::Help { .. }` in matches, and `Overlay::Help { scroll: 0 }` where one is built;
  - `accounts.rs:902, 1769`'s `ch('G')` → `key(KeyCode::End)`.
- Rewrite tests to the new layout:
  - `view.rs`'s `the_header_shows_the_tabs_and_the_mode_and_the_footer_the_screens_keys` (footer half);
  - `help.rs`'s `every_help_line_shows_whole_on_every_screen_at_80_columns` (now over `KEYS`);
  - `actions.rs`'s `help_lists_the_keys_and_closes`;
  - the Tokens footer assertions (`table.rs`, `tokens/mod.rs`) that read the old `Enter sessions` form: `⏎ sessions`.

**Interfaces:**
- Produces:
  ```rust
  // keys.rs
  /// One row of the key table (spec §9.2, §10): what help lists, and what the footer shows.
  pub(crate) struct KeyRow {
      /// `None`: every screen.
      pub(crate) screen: Option<Screen>,
      /// As help shows them: `j k ↑ ↓ PgUp PgDn Home End`.
      pub(crate) keys: &'static str,
      pub(crate) does: &'static str,
      pub(crate) footer: Footer,
      /// The key events this row stands for, for `every_key_the_table_lists_is_handled`.
      pub(crate) probes: &'static [(KeyCode, KeyModifiers)],
  }
  pub(crate) enum Footer {
      No,
      /// From `from` columns: `key` and the label `label` gives for the state, or nothing when it cannot apply.
      Show { key: &'static str, label: fn(&State) -> Option<&'static str>, from: u16 },
  }
  pub(crate) const KEYS: &[KeyRow];
  /// The current screen's footer items, in the table's order, that fit beside the pinned ones.
  pub(crate) fn footer_items(state: &State, width: u16) -> Vec<(&'static str, &'static str)>;
  // overlay/help.rs
  /// §4.4's marks with their words (T2-25), each drawn in its role.
  pub(crate) const MARKS: &[(&str, Role, &str)];
  ```
- **The table** is §10's, as T2 has it: no `w` (T2-8), History's rows today's.
  - **Everywhere:**
    - `1 2 3  Tab Shift-Tab` "Accounts, Tokens, History";
    - `?` "keys and marks";
    - `q` "close the open overlay, else quit";
    - `Esc` "close the overlay; else back one level";
    - `P` "pause or resume auto (confirm)", with footer `pause auto`/`resume auto` on Accounts only;
    - `r` "update usage now (Tokens: the index)", with footer `refresh`;
    - `A` "add the current login";
    - `D` "doctor";
    - `Ctrl-C` "quit";
    - `Ctrl-Z` "suspend".
  - **Accounts:**
    - `j k ↑ ↓ PgUp PgDn Home End` "move";
    - `s` "switch to the selected account", footer `switch`;
    - `b` "switch to the best account", footer `best`;
    - `n` "switch to the next account", footer `next`;
    - `⏎` "its history", footer `history`;
    - `J K` "scroll the selected card";
    - `e` "disable or enable", footer `disable`/`enable` from 120;
    - `a` "set or clear the alias", footer `alias` from 120;
    - `m` "move to another position";
    - `x` "remove (confirm)", footer `remove` from 120;
    - `R` "run a session on it here (confirm)".

    Every account key's label is `None` with no account selected (VIS-10).
  - **Tokens:**
    - `⏎` "drill down", footer the drill target;
    - `p` "period", `g` "group", `f` "filter", `[ ]` "range", each with a footer;
    - `Esc Backspace` "back up a level", footer `back` at depth > 0;
    - `j k ↑ ↓ PgUp PgDn Home End` "move";
    - `J K` "scroll the detail and the notes".
  - **History:**
    - `j k ↑ ↓` "previous or next account", footer `account`;
    - `[ ]` "range: 1, 7, 14 or 30 days", footer `range`;
    - `J K` "scroll the windows".
- **The footer** is the items, two spaces apart, each key in accent and its label muted, dropped from the end. `? keys  q quit` is pinned on the right. Accounts' order at 80 is `s switch  b best  n next  ⏎ history  P pause auto  r refresh`, then from 120 `e disable  a alias  x remove`.
- **Help** is a panel with `scroll`.
  - Its sections are the current screen's rows under its name, then `Everywhere`, then `Marks`.
  - From 100 columns its rows run in two columns, left then right; below 100, in one.
  - `j k ↑ ↓ PgUp PgDn Home End` scroll it, stopping at its last full page. `?`, `Esc`, `Enter` and `q` close it.

- [ ] **Step 1: Write the tests** in `keys.rs` and `chrome.rs`:
  - `the_footer_pins_keys_and_quit_on_every_screen_at_80_columns`: on all three screens, the last row ends with `? keys  q quit`, `?` and `q` in `a`, labels in `m`.
  - `accounts_footer_order_at_80_and_120`: the two strings above, each followed by the pinned items.
  - `e_s_label_follows_the_selected_row`: a disabled selected row shows `e enable` at 120.
  - `with_no_accounts_the_footer_hides_the_account_keys`: the last row is `r refresh`, a gap, then the pinned items.
  - `every_key_the_table_lists_is_handled` (T2-29). For each row and probe:
    - start from a state in which that key must act, built per probe by the test's own `match` on the key:
      - a screen key from another screen: `1` from Tokens, `2` and `3` from Accounts, `Tab` and `Shift-Tab` from Accounts;
      - `?` with nothing open;
      - `q` and `Esc` with Help open (`q` with nothing open must return `Cmd::Quit`);
      - Accounts keys on Accounts with `many(3)` and row 2 selected, and `e` on an enabled row;
      - Tokens keys on a three-row day report with row 2 selected, and `Esc`/`Backspace` one drill-down level deep;
      - History keys showing account 2 of 3 at 7 days;
      - each `J K` in a state whose card, notes or windows overflow;
      - `Ctrl-C` and `Ctrl-Z` anywhere;
    - `update` returns a command, or `draw` after it differs from `draw` before.

    A probe with no arm in that `match` fails the test, so a row added later needs its state too.
  - `help_lists_this_screen_then_every_screen_then_the_marks`: on Accounts, rows `Accounts`, then `Everywhere`, then `Marks` appear in that order, and every `KEYS` row of Accounts and of every screen appears with its `does`.
  - `help_has_two_columns_from_100_columns_and_scrolls_to_its_end`: at 120×40 `Marks` is on the right; at 80×24, `End` shows the last mark.
  - `every_mark_is_one_column_wide`: each `MARKS` glyph has `width_of == 1` (§4.4).
  - `g_does_nothing_on_accounts`: `g` and `G` leave the selection and draw the same.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked -- keys:: footer help_`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed, and rewrite the listed tests.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-tui --locked`; outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty`. Expected: all pass; `a_resize_while_idle_redraws_at_the_new_size` still reads `s switch` at column 0 (T2-28).
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Build the footer and the help from one key table"`.

### Task 10: Say each outcome in a toast of its kind

**Depends on:** Task 9

**Files:**
- Modify:
  - `src/theme.rs`: add `Role::Ok` (§4.1's values).
  - `src/state.rs`:
    - `Toast` (`:94-97`) gains `kind`, and `State::toast` (`:218-224`) takes it;
    - add `TOAST_LONG_MS`;
    - add `checking`, `checked` and `on_terminal`.
  - `src/chrome.rs`: the footer's toast and spinner cases (`view.rs:346-378`'s): the toast on the left and `{spinner}  ? keys  q quit` on the right.
  - `src/host.rs`: add `Action::label`.
  - `src/actions.rs`:
    - `start` and `busy` (`:20-45`): the busy toast;
    - `refresh` (`:92-105`): sets `checking`;
    - `pause` (`:111-135`): the toasts of T2-16;
    - `on_done` (`:228-277`): §9.3's classification;
    - `NO_DAEMON_RUNNING` changes its words (T2-16).
  - `src/source.rs`: `CollectSummary` (`:44-48`) gains `updated` and `rate_limited`. `collect_due` (`:123-137`) fills them.
  - `src/update.rs`:
    - `Msg::Collected` (`:64-91`): the `r` toast, made now when nothing was fetched, else on the next accounts read (`checked`);
    - `Msg::SuspendRefused` clears `on_terminal`.
  - `src/daemon/mod.rs`: `down` and `on_status` raise the mode toasts (T2-10). `down` takes `now_ms`.
  - `src/screens/tokens/index.rs:131`: `ALREADY` as `ToastKind::Info`.
- Modify, mechanically:
  - every other `state.toast(text, now)` call becomes `state.toast(kind, text, now)`, `Info` unless this task says otherwise;
  - every `CollectSummary { … }` literal gains `updated: 0, rate_limited: vec![]`.
- Rewrite tests to the new layout:
  - **`view.rs`:**
    - `the_footer_gives_way_to_a_toast_a_spinner_and_finishing` (`⠋ adding Claude Code's login…`; the spinner beside a toast);
    - `a_toast_lasts_five_seconds` (now per kind);
    - `a_long_toast_wraps_upwards_from_the_footer`;
    - `an_empty_toast_still_takes_the_footer_s_one_row`.
  - **`actions.rs`:**
    - `a_second_action_while_one_runs_is_busy_and_dropped`: `still switching to 1 a1 · wait for it to finish`;
    - `one_plain_line_is_a_toast_and_the_views_are_read_again`;
    - `warnings_several_lines_or_a_failure_open_the_result_panel`: its one-line failure is now a Failed toast; its two-line cases stay panels;
    - `a_suspended_actions_outcome_shows_nothing_and_rereads_every_view` becomes `a_suspended_action_s_outcome_toasts_from_its_exit_code`;
    - `p_without_a_daemon_says_so_and_changes_nothing`;
    - `two_seconds_after_p_the_status_is_read_and_the_header_says_paused_with_no_event`, for its toast;
    - `refresh_rereads_and_collects_the_shown_accounts`.
  - **`update.rs`:** `a_failed_collection_says_so_and_a_warning_is_a_toast` (`Warn`, `! …`).
  - **The toast fixtures** in `accounts.rs:1119, 1171, 1259`, `history.rs:669` and `table.rs:2235, 2372, 2440` pass `ToastKind::Info`.

**Interfaces:**
- Produces:
  ```rust
  // state.rs
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum ToastKind { Done, Info, Warn, Failed }
  pub struct Toast { pub kind: ToastKind, pub text: String, pub until_ms: i64 }
  pub const TOAST_LONG_MS: i64 = 8_000;
  impl State {
      /// Done 5 s (8 s when it takes more than one row at `self.size`'s width, as the footer
      /// lays it out), Info 5 s, Warn and Failed 8 s (spec §5.5).
      pub(crate) fn toast(&mut self, kind: ToastKind, text: impl Into<String>, now_ms: i64);
  }
  pub(crate) checking: Option<u64>,           // the collect generation a person's `r` asked for
  pub(crate) checked: Option<CollectSummary>, // its summary, until the accounts read after it
  pub(crate) on_terminal: bool,               // T2-19
  // host.rs
  impl Action {
      /// The UI's words while it runs (spec §9.4, T2-17), without the `…`. `named` gives
      /// `words::short_name` of an account, and `display` the provider's display name.
      pub fn label(&self, named: &dyn Fn(&AccountRef) -> String, display: &str) -> String;
  }
  // source.rs
  pub struct CollectSummary { pub fetched: usize, pub warnings: Vec<String>, pub updated: usize, pub rate_limited: Vec<AccountId> }
  ```
  `updated` counts `Collected::Recorded`, and `rate_limited` lists `Collected::Failed { kind: "http-429" }`.
- **The toast row** is `{mark} {text}`: `✓` in `Ok`, `!` in `Warn`, `✕` in `Crit`, and no mark for `Info`.
  - It wraps to at most `WRAP_ROWS` rows, in the width less the right part and two columns, when that leaves 40 columns. Otherwise it takes the full width, and the right part is dropped.
  - The right part sits on the last row.
  - The spinner (`{frame} {label}…`, in accent) is the running action's label, or `checking usage` while `checking`.
- **`on_done`**, in order:
  1. `NeedsTerminal`: as today; Task 12 adds the dialog.
  2. `on_terminal`, taken: §9.6's return toasts. `Run` is Info `session on {named} ended`. Else exit 0 is Done `{name} finished`, and any other exit is Failed `{name} failed · its output is above in your terminal's scrollback`.
  3. `Doctor`: its panel, whatever the exit code, even with no text.
  4. `Autoswitch` with exit 0 and no stderr: Done `Auto paused for {display}. P resumes.` / `Auto resumed.`. With stderr it falls through to item 7, so a warning is never hidden.
  5. Nothing printed: Done `{name} finished`, or Failed `{name} failed (exit {exit})`.
  6. Exit 0 with no stderr: Done with stdout's lines joined by a space, when it fits the toast's three rows; else the result panel.
  7. Exit 0 with stderr: Warn when the text is one line; else the panel.
  8. A non-zero exit: Failed when the text is one line; else the panel.

  `start` sets `on_terminal` when it returns `Cmd::Suspend`.
- **The `r` toast** (T2-9):
  - `rate_limited` non-empty: Warn `{plural(n, "account")} rate-limited · retry in {span}`, `span` from the soonest of their `usage.retry_at` in the new snapshot;
  - else the first warning: Warn;
  - else `updated` > 0: Done `updated {plural(updated, "account")}`;
  - else Info `already current · next update in {span}`, from the soonest `next_poll_at` of the shown accounts, or `already current` when there is none.

  A collection the timer asked for keeps today's warning toast, as `Warn`.
- **Mode toasts** (T2-10): `down` of a live link (not quitting) gives Warn `lost the daemon; tagteam updates on its own every 30 s`, and sets `lost`. The first `status` of a link after that gives Done `connected to the daemon`, and clears `lost`.

- [ ] **Step 1: Write the tests.**
  - **`state.rs`**, `each_toast_kind_lasts_as_long_as_it_says`: Done 5 s, and 8 s when wrapped at 80 columns; Info 5 s; Warn 8 s; Failed 8 s.
  - **`chrome.rs`:**
    - `a_toast_keeps_the_pinned_keys_and_the_spinner_on_its_right`: running `Switch(1)` with an Info toast gives the last row `still switching to 1 a1 · wait for it to finish`, a gap, then `⠋ switching to 1 a1…  ? keys  q quit`.
    - `each_kind_has_its_mark_in_its_role`, for `TIERS`.
  - **`host.rs`**, `every_action_has_its_label`: §9.4's eleven, plus T2-17's two.
  - **`actions.rs`:**
    - `the_outcome_table_in_its_order`: one case per row of the list above, plus an `Autoswitch` exit 0 with a one-line stderr, which gives the Warn toast of item 7, not item 4's.
    - `a_done_outcome_too_long_for_three_rows_opens_the_panel`: at 80 columns, stdout of 300 characters on one line opens the result panel, and a 120-character one is a two-row Done toast with `? keys  q quit` on its last row (Review Focus 4).
    - `r_says_what_it_did`, one case per toast above, through `update` and the accounts read after the collection.
    - `p_toasts_the_pause_in_the_provider_s_name`.
  - **`daemon/mod.rs`**, `losing_the_daemon_toasts_once_and_connecting_again_toasts_once`:
    - the first link's `status` toasts nothing;
    - `Live::Down` toasts the loss once;
    - the next link's first `status` toasts `connected to the daemon` once.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked -- toast outcome label r_says losing_the_daemon`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed, and rewrite the listed tests.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-tui --locked`; outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty`. Expected: all pass; `debug_lines_go_to_the_log_file_and_never_to_the_terminal` still finds `Switched to a@x.co` in the Done toast.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Say each outcome in a toast of its kind"`.

### Task 11: Frame dialogs and panels over a dimmed screen

**Depends on:** Task 10

**Files:**
- Modify:
  - `src/theme.rs`: add `surface` (tier 2: `#1f2329` / `#f6f8fa` as the background) and `dim`.
  - `src/overlay/mod.rs`:
    - `render` (`:161-190`) first sets `theme.dim()` over the whole buffer (VIS-7);
    - `boxed` (`:193-207`) takes a `Frame`;
    - `handle_key` (`:122-159`) turns `q` into `Esc` for every overlay but `Text` (UX-16).
  - `src/overlay/{confirm,choose,text}.rs`, `src/screens/tokens/filter.rs:275-345`: through `boxed`. Their keys line moves into the bottom border.
  - `src/overlay/panel.rs`: `panel` covers the body (`view::areas(state, frame.area()).body`), with the same frame as a box. `panel_height` and `panel_width` follow it. This frames Result, Doctor and Help alike. Doctor's lines are coloured by their first glyph.
- Rewrite tests to the new layout:
  - the dialog tests that read a keys row (`↑ ↓ choose · Enter · Esc cancel`, `y yes · n no · Enter choose · Esc no`, `Enter · Esc cancel`, `Esc cancel`) now read it in the box's bottom border;
  - the panel geometry tests take the body's rows less four (border and padding):
    - `doctor_opens_its_panel_and_o_runs_it_online`;
    - `a_long_output_line_wraps_in_its_panel_and_scrolls_to_its_end`;
    - `a_resize_clamps_a_panels_scroll_and_the_next_key_moves_at_once`;
    - `a_panel_scrolls_no_further_than_its_last_page`;
    - `blank_output_lines_are_blank_rows_in_the_panels`;
    - `an_indent_wider_than_a_third_of_a_panel_is_capped`;
  - `a_1000_character_choice_takes_three_rows_and_every_choice_stays_reachable` and `a_long_question_is_cut_so_the_dialog_keeps_its_buttons` keep their guarantees with the padding's rows (R21).

**Interfaces:**
- Produces:
  ```rust
  impl Theme {
      /// §4.1's `surface`: a dialog's background in tier 2, nothing otherwise.
      pub(crate) fn surface(self) -> Style;
      /// DIM: the backdrop (§9.1), and later a disabled row or a stale report.
      pub(crate) fn dim(self) -> Style;
  }
  // overlay/mod.rs
  pub(crate) struct BoxSpec { pub(crate) title: String, pub(crate) keys: &'static str, pub(crate) destructive: bool, pub(crate) width: u16 }
  pub(crate) fn boxed(frame: &mut Frame, theme: Theme, spec: BoxSpec, lines: Vec<Line<'static>>);
  ```
- **A box** is drawn as follows:
  - `BorderType::Rounded` in `Faint`;
  - the title in `Accent` and bold, or `Crit` and bold when `destructive`;
  - `Padding::new(2, 2, 1, 1)`;
  - `keys` right-aligned in the bottom border, muted;
  - `surface` under it all.

  `destructive` is `ConfirmThen::Run(Action::Remove(_))`.
- **Buttons.** A focused button takes `focus()`, or `Crit`'s fill for a destructive Yes. An unfocused destructive Yes is in `Crit`.
- **Doctor.** A line whose first non-space glyph is `✓` is `Ok`, `!` is `Warn`, and `✗` is `Crit`. The text is the CLI's.

- [ ] **Step 1: Write the tests** in `overlay/mod.rs`, `panel.rs` and `confirm.rs`:
  - `an_overlay_dims_everything_behind_it`: with Help open, every cell outside the box has `Modifier::DIM`.
  - `a_dialog_has_a_rounded_faint_frame_its_title_in_accent_and_its_keys_in_the_border`: the corners are `╭╮╰╯` in `f`, the title in `a`, and the bottom border ends `y yes · n no · Esc no ─╯`.
  - `a_destructive_confirm_titles_and_fills_in_crit`: `x`'s confirm.
  - `in_truecolor_a_dialog_draws_on_its_surface`: tier 2's inner cells have `bg` `#1f2329` (dark) or `#f6f8fa` (light); tiers 0 and 1 have none.
  - `a_panel_covers_the_body_whole`: the Result panel's border rows are `body.y` and `body.y + body.height − 1`, and its columns 0 and `width − 1`, at 80×24 and 120×40.
  - `doctor_colours_each_line_by_its_glyph`.
  - `q_closes_an_overlay_before_it_quits`:
    - Help, Result and Doctor close;
    - a Confirm prompt answers `Confirm(false)`, and a Choose answers `Choose(None)`;
    - a Text dialog types `q`;
    - then `q` quits.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked -- dims a_dialog_has destructive surface covers doctor_colours q_closes`. Expected: a compile error or failures.
- [ ] **Step 3: Implement** as listed, and rewrite the listed tests.
- [ ] **Step 4: Run the crate's tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui && git commit -m "Frame dialogs and panels over a dimmed screen"`.

### Task 12: Guard the action keys and ask before handing the terminal over

**Depends on:** Task 11

**Files:**
- Create: `src/overlay/proceed.rs`: a dialog with a question, a sentence, `⏎ {go} · Esc cancel` in its border, and what `⏎` does.
- Modify:
  - `src/overlay/mod.rs`:
    - add `Overlay::Proceed` and `ProceedThen`, and route keys and drawing to `proceed`;
    - `decline` names `Proceed` as `None`;
    - every key that closes an overlay sets `state.guard_until_ms = now_ms + GUARD_MS` (T2-18).
  - `src/state.rs`: `pub(crate) guard_until_ms: i64`, `pub const GUARD_MS: i64 = 300`.
  - `src/update.rs:221`: before `actions::handle_action_key`, a `KeyCode::Char` in `s b n e a m x R P A D r` while `now_ms < state.guard_until_ms` returns `vec![]`.
  - `src/actions.rs`:
    - `R` (`:186`) opens `Proceed { then: Start(Action::Run(account)) }`;
    - `m` (`:168-175`) asks `Move {named} to position (1–{n}):`, with `TextThen::Move { account, last: n }`;
    - `on_done`'s `NeedsTerminal` for any action but `Run` opens `Proceed { then: Suspend(action) }`, keeping `running`.
  - `src/overlay/text.rs`: `submitted` refuses `0` or `> last` with `Choose a position from 1 to {last}.` under the input, in crit (T2-20). `TextThen::Move` becomes `Move { account: AccountRef, last: u32 }`.
- Modify: `crates/tagteam/tests/tui_pty.rs`.
  - Each test that types `R`, after `wait_for(… "in this terminal?")`, types `\r`:
    - the four `under_r_*` tests;
    - `run_on_an_account_moved_elsewhere_is_refused_and_launches_nothing`;
    - `under_r_ctrl_c_ends_the_session_and_the_daemon_connection_stays_live`.
  - `a_locked_keychain_runs_the_action_on_the_terminal_and_returns` waits for `Your login keychain is locked.` after `s` and types `\r` before it waits for the CLI's question.

**Interfaces:**
- Produces:
  ```rust
  pub enum ProceedThen { /// R: `start` it.
      Start(Action), /// A hand-over: `running` is already set; ⏎ suspends it, Esc clears it (T2-19).
      Suspend(Action) }
  Overlay::Proceed { question: String, detail: String, go: &'static str, then: ProceedThen },
  ```
- **The words.**
  - `R`: question `Start {display} as {named} in this terminal?`, detail `tagteam steps aside until the session ends, then comes back.`, go `starts`.
  - The hand-over: question `Your login keychain is locked.`, detail `tagteam hands you the terminal to unlock it, then comes back.`, go `continue`.
- **`⏎`** on `Start` returns `start(state, action)`. On `Suspend` it sets `on_terminal` and returns `[Cmd::Suspend(action)]`. `Esc` and `q` close; on `Suspend` they clear `running`.

- [ ] **Step 1: Write the tests.**
  - **`update.rs`**, `the_guard_drops_only_action_keys_and_only_for_300_ms` (Review Focus 5):
    - after Help closes at `NOW_MS`, `n` at `+299` does nothing;
    - `j`, `k`, `?`, `q` (in a fresh state), `Esc` and `1`–`3` act at `+1`;
    - `n` at `+300` switches.
  - **`actions.rs`:**
    - `r_asks_before_it_starts_a_session`: `R` opens the dialog with §9.5's words, `⏎` gives `[Cmd::Suspend(Run(1))]`, and `Esc` gives nothing.
    - `a_locked_keychain_asks_before_it_hands_the_terminal_over`: `NeedsTerminal` for `Switch(1)` opens the hand-over dialog with `running` kept. `⏎` gives `[Cmd::Suspend(Switch(1))]`, and the outcome after it toasts `✓ switch finished`. `Esc` clears `running` and gives nothing.
    - `move_asks_for_a_position_in_range_and_refuses_one_outside_it`: with three accounts, `m` asks `Move 1 a1 to position (1–3):`. `4` then `⏎` keeps the dialog and shows `Choose a position from 1 to 3.`. `2` then `⏎` moves.
    - `remove_on_the_account_in_use_confirms_as_any_other` (§9.5).
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked -- the_guard r_asks locked_keychain_asks move_asks remove_on_the_account`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-tui --locked`; outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui crates/tagteam/tests/tui_pty.rs && git commit -m "Guard the action keys and ask before handing the terminal over"`.

### Task 13: Show the empty states and say the provider's own names

**Depends on:** Task 12

**Files:**
- Modify:
  - `src/view.rs`:
    - `TOO_SMALL` (`:19`) becomes `too_small(w, h)`, drawn centred (`view` `:242-245`);
    - add `centred_block`.
  - `src/screens/accounts.rs`:
    - `NO_ACCOUNTS` and `LOADING` (`:25-27`) become §5.7's blocks, drawn at `:540-548`;
    - the one-account line goes under the table;
    - `banner` (`:138-146`) takes the provider's names for the unmanaged login.
  - `src/screens/tokens/table.rs`: `NOT_INDEXED` (`:31`) becomes §5.7's block.
  - `src/actions.rs:176-185`: `x`'s question takes `display_name`.
  - `src/actions.rs:129-133`: the `P` question (T2-16).
- Rewrite tests to the new layout:
  - `view.rs`'s `below_80_by_24_only_the_notice_is_drawn`;
  - `accounts.rs:1513`'s empty-state test;
  - the `loading…` assertions of Accounts (`update.rs`'s `a_long_collection_error_shows_in_full_at_80_columns` reads `Loading accounts…`);
  - `table.rs:1777`'s `No token usage indexed yet.`;
  - `actions.rs`'s `remove_asks_its_own_question_and_defaults_to_no` and the `P` question tests;
  - in `tui_pty.rs`, `p_pauses_the_daemon_s_auto_switch_and_resumes_it`'s questions: `Pause auto for Claude Code?`, `Resume auto for Claude Code?`.

**Interfaces:**
- Produces:
  ```rust
  /// `Terminal 72×20 · tagteam needs 80×24` (§4.5, UX-16).
  pub(crate) fn too_small(width: u16, height: u16) -> String;
  /// A title (bold) and a sentence (muted) in the middle of `area` (spec §5.7).
  pub(crate) fn centred_block(frame: &mut Frame, area: Rect, title: &str, sentence: Option<&str>, theme: Theme);
  ```
- **The words** (§5.7, §5.6), with `{display}` and `{launch}` from `state.names_of(provider)`:
  - loading: `Loading accounts…`;
  - no accounts: `No accounts yet`, then `Log in with {launch}, then press A to add that login.`;
  - one account, under the table: `Add a second account to switch: log in to {display} as it, then press A.`;
  - Tokens, nothing indexed: `No token usage yet`, then `tagteam reads {display}'s transcripts; they appear here as you use it.`;
  - the unmanaged login: `{display} is logged in as {email}, which tagteam doesn't manage · press A to add it`;
  - `x`: `Remove {position} {email}? Its stored login is deleted; {display} is not changed.`

- [ ] **Step 1: Write the tests:**
  - `below_80_by_24_the_body_says_the_size_it_needs`: at 72×20, the middle row is `Terminal 72×20 · tagteam needs 80×24`, centred, and every other row is empty; at 79×24 and 80×23 the same with their sizes.
  - `the_empty_states_say_what_to_do`: each block above, centred in the body, at 80×24 and 120×40.
  - `with_one_account_the_table_says_how_to_add_a_second`.
  - `the_words_name_each_provider_by_its_own_names`: with a fake-agent account selected, the remove question names `FakeAgent`, and its unmanaged banner names it too.
  - `no_production_line_names_the_provider` in `theme.rs`'s test module, beside the colour scan. It walks `src`'s non-test text as that scan does, skips lines whose trimmed text starts with `//`, and finds no `claude` or `anthropic` in any case.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked -- below_80 empty_states one_account names_each_provider no_production_line`. Expected: failures.
- [ ] **Step 3: Implement** as listed, and rewrite the listed tests.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-tui --locked`; outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui crates/tagteam/tests/tui_pty.rs && git commit -m "Show the empty states and say the provider's own names"`.

### Task 14: Show the start-up warnings and keep a failed token report on screen

**Depends on:** Task 13

**Files:**
- Modify:
  - `src/runtime.rs`: `TuiOptions` gains `warnings: Vec<String>` and `config_mtime: Option<SystemTime>`, and `State` takes them.
  - `crates/tagteam/src/tui_cmd.rs`:
    - read `let config_mtime = Settings::mtime(&env);` before `build_engine` (`:58`);
    - pass it, and `warnings.clone()`;
    - the warnings are still printed before the alternate screen.
  - `src/source.rs`: `AccountsSnapshot` gains `config_mtime: Option<SystemTime>` (`Settings::mtime(engine.env())`).
  - `src/state.rs`: `pub(crate) startup: Startup`.
  - `src/update.rs`:
    - `Msg::Accounts` clears `startup` when the snapshot's `config_mtime` differs from it;
    - `Esc` on Accounts with no overlay clears it.
  - `src/chrome.rs`: `banner`'s start-up-warnings case (T2-14, T2-21).
  - `src/screens/tokens/mod.rs`:
    - `reset` (`:298-304`) keeps `levels[0].report`;
    - `on_report` (`:338-368`) keeps the report on `Err` and sets `stale`, with `error` `couldn't read the token index ({message}) · r retries`.
  - `src/screens/tokens/table.rs:1230-1290`: a `stale` report's rows are drawn `theme.dim()`.
- Modify, mechanically:
  - every `TuiOptions` and `AccountsSnapshot` literal gains the new fields;
  - the Tokens tests that assert `report().is_none()` right after a query change (`tokens/mod.rs:633, 652, 784`) assert instead that the last report is kept, `pending()` is true, and the next report replaces it (§8, VIS-10).

**Interfaces:**
- Produces:
  ```rust
  pub(crate) struct Startup { pub(crate) warnings: Vec<String>, pub(crate) mtime: Option<SystemTime> }
  // TokensState
  stale: bool,   // the last report query failed: its report is drawn DIM (T2-22)
  ```
- **The banner** is `! {warnings joined by " · "}`, in warn, while `startup.warnings` is not empty.

- [ ] **Step 1: Write the tests:**
  - `the_start_up_warnings_show_until_esc_on_accounts`: the banner reads `! <path>: unknown key x (ignored)`, `Esc` on Tokens leaves it (it goes back a level, or does nothing), and `Esc` on Accounts clears it.
  - `a_config_change_clears_the_start_up_warnings`: an accounts read with the same mtime keeps it, and one with a later mtime clears it (Review Focus 3).
  - `a_failed_report_keeps_the_last_one_dim_under_the_banner`, for `TIERS`: after a good day report, `p` and an `Err` keep the day rows, with `Modifier::DIM` on each table cell, and the banner `! couldn't read the token index (the index is locked) · r retries`. The next good report clears both.
  - `the_index_check_s_banner_keeps_its_words_in_warn`.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked -- start_up_warnings config_change failed_report index_check`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-tui --locked` and `cargo test -p tagteam --locked --features test-support --lib`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui crates/tagteam/src/tui_cmd.rs && git commit -m "Show the start-up warnings and keep a failed token report on screen"`.

### Task 15: Record the redesigned chrome in the TUI spec and the changelog

**Depends on:** Tasks 1–14

**Files:**
- Modify: `CHANGELOG.md`, under `## [Unreleased]` `### Changed`:
  - `` `tagteam tui`'s header says auto's state and threshold, and a line under it says auto's last action in plain words. ``
  - `` `tagteam tui` marks the selected row with a bar instead of reverse video, and draws in truecolor where the terminal supports it and reports its background. ``
  - `` `tagteam tui`'s footer always shows `? keys` and `q quit`. Help lists every key and what each mark means. ``
  - `` `tagteam tui` reports a successful action in a toast. Only failures, warnings and doctor open a panel. `r` says what it updated. ``
  - `` `tagteam tui` asks before `R` starts a session and before it hands the terminal over to unlock the keychain. It ignores action keys for 300 ms after a dialog closes. ``
  - `` `tagteam tui` shows the settings warnings in a banner, and keeps the last token report on screen when a new one cannot be read. ``
- Modify: `docs/superpowers/specs/2026-10-14-tagteam-tui-design.md`, each hunk ending *(Amended by the TUI redesign spec §n.)*:
  - **§5.1:**
    - the mock-up and its header, footer and activity-line bullets give way to the chrome rows (redesign §5.1–§5.6);
    - "Colour" becomes the three tiers and roles, with the selection as `▌` and bold (§4.1–§4.3);
    - "Size" becomes `Terminal W×H · tagteam needs 80×24` (§4.5).
  - **§5.5:** help becomes the key table's two sections and the legend (§9.2); dialogs and panels as §9.1; `q` closes an overlay first.
  - **§5.6:** the table becomes "the key table (redesign §10), less `w` until T4". Accounts' `g`/`G` are dropped.
  - **§7.2:** "The outcome" becomes §9.3's table and §9.4's labels and `still …` toast (was `busy: …`).
  - **§7.4:** the header table becomes redesign §5.2's. The `P` question becomes `Pause auto for <display name>?`. Timer mode's `P` becomes §10's.
  - **§8:** "Every event except `sleep`, `changed` and `mode` also updates the activity line" becomes "the daemon's `no-switch`, `error` and `config-warning` events and its `status.lastSwitch` feed the auto line (redesign §5.3)".
  - **§10.4:** the empty state becomes §5.7's.

- [ ] **Step 1: Edit** the two files as listed.
- [ ] **Step 2: Check the format.** Run `cargo fmt --all --check`. Expected: clean.
- [ ] **Step 3: Commit.** Run `git add CHANGELOG.md docs/superpowers/specs/2026-10-14-tagteam-tui-design.md && git commit -m "Record the redesigned chrome in the TUI spec and the changelog"`.

---

## After Task 15 (controller)

- [ ] **The gallery check** (above), into `gallery/t2-after-15`.
- [ ] **No dependency changed.** Run `git diff ba624bd -- Cargo.toml Cargo.lock crates/*/Cargo.toml`. Expected: nothing.
- [ ] **No provider name in neutral source.** Run `git diff ba624bd -- crates/tagteam-tui/src crates/tagteam-engine/src | rg '^\+' | rg -i 'claude|anthropic'`. Expected: only lines inside test modules or `testutil.rs`.
- [ ] **CI, in full.** Run the four CI commands. Expected: all clean, with totals reported as passed/total.
- [ ] **The pty suites.** Outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty --test signals`.
- [ ] **The first frame.** Outside the sandbox, on an idle machine, run `cargo test --release -p tagteam --features test-support --test perf -- --ignored tui_first_frame --nocapture`. Expected: every p95 is within 50 ms, the colour-query case included.
- [ ] **Linux.** Run the full suite in Docker: non-root with a passwd entry, HOME outside `/tmp`, and the `docker` call unpiped.
- [ ] **Cross-review.**
  - `xreview round --reset`.
  - A pre-merge review of `origin/main...tui-redesign-t2`. The PR gate opens only on a full-range approval.
  - If the diff exceeds xreview's inline cap, the full range needs `XREVIEW_MAX_DIFF_BYTES` raised, which is Michael's call each time (as for T1). Until he makes it, review in slices: Tasks 1–4 (engine, detection), 5–9 (chrome, selection, keys), 10–15. Then run one full-range round once he allows it.
  - Fix and re-review until it approves.
- [ ] **The Draft PR.**
  - Push, then open a Draft PR against `main` that fills `.github/pull_request_template.md`. Its "Rulings" section lists T2-1 to T2-29, plus any ruling made in review. It names the refactor overlaps above.
  - In one commit, set this plan's status to `Implemented — <PR URL>`; the redesign spec stays `In progress`.
  - Re-review that commit, then push it.
