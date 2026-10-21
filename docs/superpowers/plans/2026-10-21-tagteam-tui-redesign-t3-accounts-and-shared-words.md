# tagteam TUI redesign T3 — Accounts and the shared words — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Implemented — https://github.com/michaelrigart/tagteam/pull/35
**Goal:** Rebuild the Accounts screen around the account in use (a lead card with bars and the best pick, a quiet table with a loud STATUS column, a selected card with sparklines, problems and remedies), show the account in use on Tokens and History, say every usage state in one plain vocabulary shared by `list`, `status`, `history`, `auto` and the TUI, and close the R3 items the refactor handed to this branch, with no `--json`, store or wire change.
**Architecture:** The engine gains `best_candidate`, the pure pre-vault rule of `switch --strategy best`, which both the switch and the accounts view call. The view carries each provider's `models`, `live` login and `forecast`, from one live-identity read. `words.rs` changes the shared words and adds the problem sentences and remedies. The TUI gains `gauge.rs` (lead bar, line gauge, sparkline), three Accounts parts (`screens/accounts/{table,card,lead}.rs` and `status.rs`) and the Now strip in `chrome.rs`. The binary adopts R3's `engine_for`, `fail_to` and `ColorEnv`, hands the host its warnings and the unlock prompt (D-2), and the TUI reads the daemon through R3's typed wire, after which `protocol.rs`'s `String` fields become `Wire<T>`.
**Tech Stack:** Rust 1.88 (edition 2024), ratatui 0.30.2, crossterm =0.29.0. No new dependency.
**Spec:** docs/superpowers/specs/2026-10-20-tagteam-tui-redesign-design.md (§1–§2, §4.3–§4.5, §5.4, §6, §11.1–§11.3, §12.1, §12.2, §13, §14, §15's T3 row, §16–§18)
**Base:** main after #25 and R4a merge (today 9af7450 plus refactor-r4a-failure-kinds).

**Reading this plan.**
- Paths are under `crates/tagteam-tui/` unless they start with `crates/`, `docs/` or a file at the root.
- Code is cited by the code quoted. Line numbers, where given, are as of `9af7450` for files the refactor does not touch, and as on `refactor-r4a-failure-kinds` for files it does (`app/`, `switch/`, `protocol.rs`, `failure.rs`). The rebases move them.
- Focused tests: `cargo test -p tagteam-tui --locked <filter>`; `cargo test -p tagteam-engine --locked --features test-hooks <filter>`; `cargo test -p tagteam-daemon --locked --features test-hooks <filter>`; `cargo test -p tagteam --locked --features test-support --test <file>` (or `--lib`).
- The pty tests (`tui_pty.rs`, `perf.rs`, `signals.rs`) open a pseudo-terminal: run them outside the sandbox.
- Each task leaves the workspace compiling, with its crates' tests green.
- "Tier 0/1/2" and `TIERS` are T2's: `[Theme::new(false), Theme::new(true), Theme::new(true).truecolor(Background::Dark), Theme::new(true).truecolor(Background::Light)]`. `roles` and `canon` are T2's `testutil` helpers. `body_of` reads the body's rows alone.
- **"Focus"** is `daemon::focus(state)`: the selected account's provider, else `--provider`'s, else the default (Tokens') provider.
- **Hand-computed text.** Where a row this plan spells out disagrees with the rule it states, the rule wins: the implementer fixes the expectation to the rule and says so in the task report.

## Global Constraints

- **CI commands** (`CONTRIBUTING.md`):
  - `cargo fmt --all --check`;
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`;
  - `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`.
- **Toolchain:** Rust 1.88, edition 2024, ratatui 0.30.2, crossterm =0.29.0 (pinned).
- **No silenced lints.** No `#[allow(...)]` is added. A type, field or function is added in the task that first uses it; dead code is deleted, or `#[cfg(test)]` when only tests use it.
- **No `unsafe`.** `tagteam-tui` stays `#![forbid(unsafe_code)]`.
- **No new dependency.** `git diff <base> -- Cargo.toml Cargo.lock 'crates/*/Cargo.toml'` prints nothing at the end. No feature is added to a dev-dependency either.
- **The edges stay put.** No `--json` change, no store change, no migration. The daemon protocol changes only as §12.5's typed-wire adoption allows (Task 9), and its JSON stays byte-compatible: `service_cmd.rs`'s `daemon status --json` tests and `protocol.rs`'s `every_auto_kind_is_written_byte_for_byte_as_auto_wrote_it_before` pass unchanged.
- **Words.** §11.1's changes land in Task 2, with every test that pins those words moving there, once. TUI-only strings change in the tasks that draw them. `CHANGELOG.md` lists the user-visible changes (Task 15).
- **Provider-neutral text.** No production line T3 adds under `crates/tagteam-tui/src`, `crates/tagteam-engine/src` or `crates/tagteam-daemon/src` names `claude` or `anthropic` in any case, nor uses `tagteam_cc`. Provider words come from `ProviderNames`. T2's `no_production_line_names_the_provider` and R1's `neutrality.rs` both run.
- **Example names only.** Fixtures T3 writes or rewrites use §3.4's names: `alex@example.com`, `sam@corp.example`, `alex@personal.example`, `Example Corp`, and the fixtures' `a1@x.co` style. The rewritten Accounts fixtures drop `michael@example.com` and `w@corp.com` (T3-27). Display names in fixtures are the providers' own (`Claude Code`, `FakeAgent`).
- **No test is weakened.** A test edit is one of:
  - a mechanical change to an API this plan changes, listed in its task;
  - a word test moved to §11.1's words (Task 2), or a render test rewritten to the new layout per §16, named in its task, asserting what the new layout shows;
  - a new test.

  Nothing is deleted to make a run pass. Where a fixture filled the body exactly and a row moved (the Now strip, Task 14), the fixture changes so the test still exercises the same property. Timing bounds pin which ending fired, never scheduler latency (T2's lesson).
- **Moves are separate from edits.** No task moves code between files. Replaced code is rewritten in place or deleted; new parts go in new files.
- **Commits.**
  - Imperative and in sentence case, with no prefix and no trailing period, matching `git log`.
  - Each task's message is given in its last step.
  - No agent attribution: no `Co-authored-by:` agent line, no session link, no "Generated with" footer.
  - A task's commit stays under about 90 KB of diff (`git show --format= <sha> | wc -c`), so that it is one review slice. A task that would exceed it tells the controller, who splits it by file into two commits.
- **The first frame.** `tests/perf.rs`'s first-frame p95 stays within 50 ms in every case. The accounts read gains no read but drops one (PERF-R-10a); the card's history read waits 250 ms after the first snapshot, so nothing new runs in front of the first frame.
- **Files this plan may touch:**
  - `crates/tagteam-tui/**`;
  - in `crates/tagteam-engine/`: `src/views.rs`, `src/words.rs`, `src/switch/select.rs`, `src/switch/mod.rs` (re-exports only); tests `tests/strategy.rs`, `tests/fake_agent.rs`, `tests/views_usage.rs`;
  - in `crates/tagteam-daemon/`: `src/protocol.rs` (Task 9), and the literals of the retyped fields in its tests (`tests/client.rs`, and `src/` test modules that build `StatusProvider`, `StatusOutcome` or `Event::Mode`);
  - in `crates/tagteam/src/` (§3.1, §13 and the R3 items, nothing else):
    - `tui_host.rs`, `tui_cmd.rs` (§3.1);
    - `prompt.rs`, `app/mod.rs` (`ensure_unlocked`, `run_command`'s warning loop, the `context` re-exports), `app/context.rs` (`Context.shown_warnings`, `locate`, `color_enabled`) (D-2, CC-O-2/3);
    - `render.rs`, `auto.rs` (the CLI renderers that print the shared words, §3.1);
    - `history.rs`, `daemon_cmd.rs`, `statusline.rs`: their `#[cfg(test)]` modules only (moved words; `Context` literals);
    - `service_cmd.rs`: `as_str()` at the retyped fields' reads, and its tests' literals of them (Task 9);
  - in `crates/tagteam/tests/`: `tui_host.rs`, `tui_cli.rs`, `app.rs`, `daemon_in_process.rs`, `usage_cli.rs`, `cli.rs`, `run_shell_cli.rs`, `history.rs`, `auto_cli.rs`, `daemon_cli.rs`, `daemon_procs.rs` (Task 9's mechanical read), `tui_pty.rs`;
  - `crates/tagteam-fake/tests/neutrality.rs` (Task 1, only if R1's two TUI entries are still there);
  - `CHANGELOG.md`;
  - `docs/superpowers/specs/2026-10-14-tagteam-tui-design.md`, `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md` (Task 15's hunks);
  - this plan.

  Not edited: `crates/tagteam-core/**`, `crates/tagteam-provider/**`, `crates/tagteam-cc/**`, the rest of the engine, the daemon and the binary (`cli.rs` included).
- **Never committed:**
  - the review harness (`src/ux_gallery.rs`, `src/ux_concept.rs`, `src/ux_redesign.rs` and their `lib.rs` lines);
  - any render (`.txt`, `.svg`, `.png`).

  The harness is parked in `.superpowers/tui-redesign/harness/` whenever a task runs. Before every commit, the controller checks that `git status --short` lists no `ux_` file.

### What T3 takes from the refactor

R1–R4a merge before T3 starts. This plan uses their APIs as `refactor-r4a-failure-kinds` shows them today. "Before Task 1" checks each name; one that changed in the final push is patched into this plan first (T3-1).

| Name, as today | From | Tasks |
|---|---|---|
| core `autoswitch`: `AutoStrategy` (was `Strategy`), `NoSwitchReason::{ALL, parse}`, `Trigger::{ALL, parse}` | R3 | 1, 2, 4, 9 |
| `tagteam_daemon::protocol::{Wire, Spelled}`, `Wire::{from_wire, as_str, known}` | R3 | 9 |
| `StatusProvider::{mode(), waiting_for(), collects_for_itself(), waits_for_upgrade()}`, `StatusOutcome::kind()`, `Event::{mode(), waiting_for(), poll(), switch(), no_switch()}` | R3 | 9 |
| `PollPayload { active: Option<ActiveAccount { number, email }>, headroom_pct, threshold, fetch_errors, windows_pct }`, `PollPayload::binding(position) -> Option<BindingWindow { window, used_pct: i64 }>`, `NoSwitchPayload { reason: Wire<NoSwitchReason>, detail }`, `parse_poll`/`parse_switch`/`parse_no_switch` | R3 | 9 |
| `tagteam_daemon::events::{Mode, WaitingFor}`, `tagteam_daemon::status::OutcomeKind` | R3 | 9 |
| `app::{engine_for, locate_run_shell, Located, locate, build_engine, build_registry, fail_to, ColorEnv, color_enabled, UNLOCK_QUESTION, Context, Io}`; `ColorEnv::{of, enabled}`; `Io::warn`; `App::ensure_unlocked` and `run_command` in `app/mod.rs`; `Failure::report` (`pub(super)`) | R3 | 7, 8 |
| `tagteam_engine::failure::FailureKind::{parse, as_str}` and its variants; `tagteam_provider::TransientKind::Http(u16)`; `Collected::{Failed { kind: FailureKind }, Errored}` | R4a | 1, 2, 3 |
| `switch/select.rs`: `Rated`, `is_candidate`, `Engine::{switch_candidate, candidate_order, usage_pick, best_pick, walk, exhausted_message}`, `nothing_to_activate`; `switch/plan.rs`'s unmanaged refusal before `usage_pick`; `tagteam_core::rank::{best_order, BestOrder, Candidate}`, `tagteam_core::usage::headroom` | R2 (core: before R1) | 5 |
| `tagteam_cc::CLAUDE_CODE` (was core's); `tagteam_core::fmt::span`; `tagteam_core::time::format_iso8601` | R1 | test imports |
| `crates/tagteam-fake/tests/neutrality.rs`'s `ALLOWED` | R1 | 1 |
| `Engine::on_point` (test hooks) | earlier | 6 |

Also expected from the rebases, and finished by Task 1 if not: T2's `Strategy` uses renamed `AutoStrategy`; `source.rs`'s `rate_limited` test compiling against `FailureKind`; R1's two TUI `ALLOWED` entries gone.

## Rulings

Calls inside the spec, for the PR's "Rulings" section.

| # | Ruling | Why |
|---|---|---|
| T3-1 | **Refactor names.** This plan uses R1–R4a's names as `refactor-r4a-failure-kinds` has them. A name the final push changed is patched into this plan before Task 1; behaviour asked of it does not change. | The refactor is still being rebased (brief). |
| T3-2 | **`best_candidate` and its types live in `switch/select.rs`**, re-exported from `tagteam_engine::switch`: `LiveLogin`, `PickInput`, `PickRow`, `BestForecast`, `NoPick`. `views.rs`'s private statusline struct `LiveLogin` is renamed `CachedLogin`, with no other change. | §12.1 puts the function there; §12.2's field uses the same `LiveLogin`. One type, two callers. |
| T3-3 | **`NoPick` for a provider with no candidate and no managed live login is `TooFewCandidates`** (the switch's `NoValidTarget`). `best_pick` maps every forecast back to today's outcomes and messages. It sets the live row's `candidate` with `switch_candidate`, so the fewer-than-two count is the switch's own. | The spec's four `NoPick`s; the switch's messages do not change (§12.1). |
| T3-4 | **`is_switchable()` is R4's, not R4a's.** `PickRow.candidate` is `is_candidate(row) && !in_session`, the two layers `switch_candidate` has. R4's `is_switchable` replaces `is_candidate` there when it lands. | §12.1 says to use it "where it applies"; it does not exist at T3's base. |
| T3-5 | **The view's `live`** is `Managed { position }` when the view marks an account active (the stored-active fallback of an unreadable live identity included), else `Unmanaged { email }` with the live login's email (else its label), else `None`. The forecast and the lead card follow it. | §12.2 keeps `Read<T>`'s states and the fallback; `b`'s toast is the truth (§12.1). |
| T3-6 | **PERF-R-10a by construction.** `EngineSource::accounts` no longer calls `Engine::status`, and `AccountsSnapshot.unmanaged_live` goes: each provider's `live` comes from the view's one `liveness` read. A `"view-live-identity"` hook point in `Engine::liveness` lets an engine test count the reads. | §12.2's single read; `Read<T>` and the fallback stay in `liveness`. A TUI-side count would need a dev-dependency feature (Global Constraints). |
| T3-7 | **One provider for `A`, the unmanaged-login banner, the one-account hint, the lead card and the Now strip: focus.** `A` adds focus's live login (was `--provider`'s, else the first listed). The banner names focus's unmanaged login. The hint shows when focus has exactly one account. | T2 left the banner and the hint to T3 (per provider). The header and `P` already follow focus (§5.2, §5.4); a hint that says "press A" must name the provider `A` adds. With one provider nothing changes. |
| T3-8 | **Failure words (§11.1).** `http-429` is `rate-limited`; `over-budget` is `paused to stay within the request budget`; every other usage failure is `usage check failed (<token>)`, its token as stored. A retry still ahead joins the parentheses as today: `usage check failed (refresh-failed, retry 5m)`, `rate-limited (retry 5m)`. | §11.1's example; R4a's `FailureKind` has no words of its own. |
| T3-9 | **§11.1's edges.** `▲ fast` replaces `▲ pace` in `list` and `status`. `status`'s `<age> old` stays: §11.1's table does not list it. `history`'s human output drops `(regression)`/`(average)`; `--json`'s `projectionMethod` stays. The TUI's help names the method in one line under Marks (`%/h`). The CLI's `history --help` is not edited (`cli.rs` is outside §3.1). | §11.1's "help names the method"; §1.3: no `--json` change. |
| T3-10 | **`login valid`** (§11.1): `login valid N more days` from two days left, `login valid 1 more day` from one, `login valid 5h20m more` under a day, `login expired` once passed. On the lead card it is warn under 7 days, followed by ` · log in again before then` (§6.2). | The spec gives the day form only. |
| T3-11 | **Problems (§6.3, §11.2).** `words::Problem` has ten kinds, one per account: `NeedsLogin`, `LoginExpired`, `NoLogin` (crit); `RateLimited`, `Keychain`, `Budget`, `OtherAccount`, `Retrying`, `OldReading` (warn); `NotChecked` (muted). `OldReading` is a reading with windows, status `ok`, and not decision-grade. `ForeignCredential` is `OtherAccount`, whose STATUS phrase is `! retrying`. `NotChecked` has no STATUS phrase: the window columns say it. API keys and unsupported providers have none. | §6.3's table names no phrase for a foreign credential or an unread account; §11.2 gives both remedies. |
| T3-12 | **The problem sentences** (§11.2 gives one): `Needs a new login: its stored login was refused (<reason>), <when>.`; `Login expired: its stored login ran out and could not be renewed.`; `No stored login: tagteam holds no login for this account.`; `Can't read the Keychain: it is locked, or it refused tagteam.`; `Rate-limited: the usage check was turned away for now.`; `Paused to stay within the request budget.`; `Logged in as another account: the login tagteam read belongs to a different account.`; `Usage check failed (<token>).`; `Old reading: too old to pick the best account by.`; `Not checked yet.` Without a reason or a time, the parenthesis or `, <when>` is left out. | The builder fixes copy the spec leaves open. |
| T3-13 | **Remedies** are §11.2's, with `remedy(problem, &AccountView, &ProviderNames, now_s)`: the spec's `status` is the `Problem`, and `now_s` gives the countdowns. `OldReading` takes `NotChecked`'s: `tagteam checks it within <span>.` while a poll is planned. With no time ahead, `RateLimited` and `Retrying` have no remedy, `Budget` says `tagteam checks again when the hourly budget allows.`, and `NotChecked` and `OldReading` say `Press r to check it now.` | The spec's table has countdowns that can be absent, and no row for an old reading. |
| T3-14 | **STATUS order** (§6.3) adds `▶ this session` (muted) after `○ disabled` and before `in use`. At under 100 columns only `✕`, `!` and `○` show, as a glyph after UPDATED. | §6.3's last bullet places it nowhere else. |
| T3-15 | **Table grammar** (§4.3, §6.3, §19 widths): `{sel}{pos:>4} {mark} {name}   {5H}  {7D}[  {scoped}…][  {SPEND}]   {UPDATED:>7}[  {STATUS}]`. A window cell is `{gauge} {pct:>4}{▲}`, the `▲` slot always two columns (` ▲` or two spaces). The name cell is the alias padded to the widest alias, two spaces, and the email muted, or the email alone. Its column is the widest name, at least 16 (`ACCOUNT` at least); orgs (` · Example Corp`, muted) show only when every row's fits. Shrinking, when the row is too wide: the name down to 16 (cut by `fit`), then the 7D gauge to 6, the 5H gauge to 6, the 7D gauge to none, the 5H gauge to none. Scoped and SPEND columns show from 160 columns only when they fit whole after the name's 16. With several providers, each section has a muted heading (its display name) and its own column heads; with one, the heads are pinned. | §19 leaves exact widths to the builder, within §4.3's spacing and §6.3's priorities. A fixed `▲` slot keeps the columns still when pace changes. |
| T3-16 | **Bold in the table:** only the selected row's name (the alias, or the email). The active row has `●`, not bold. **Tier 0** keeps §4.1's forms: accent, warn and crit are bold there, so a selected active row in tier 0 draws `▌`, `●` and the name bold. §4.3's "one bold per row" counts emphasis (`Theme::strong`); a role's tier-0 form is §4.1's, the more specific rule for that tier. | The lead card's bars are the bold element (§1.2 goal 1). Tier 0 has no other way to draw a role. |
| T3-17 | **Geometry** (§4.4). Line gauge of `n` cells: `x = clamp(pct, 0, 100) / 100 × n`; `floor(x)` cells of `━`, then `╸` when the fraction is at least ½, then `─` (tier 0 `·`). The pace cell is `min(floor(expected / 100 × n), n − 1)`: `┿` (the fill's role) inside the whole cells, else `┼` (muted) in place of that cell. Lead bar: `round(x)` cells of `▆` in the fill's role, `▁` faint after, `│` at the pace cell (text inside the fill, muted outside). Sparkline: `n` buckets over `[from, to)`, each the peak of its samples as `▁▂▃▄▅▆▇█[round(peak / 100 × 7)]`, accent; an empty bucket is a space. A NaN percentage draws as 0. | §4.4 gives the parts, not their arithmetic; the sparkline's is `history.rs`'s. |
| T3-18 | **Fills** are accent below 70 %, warn from 70, crit from 90; a percentage's text is text, warn or crit. The lead card's percentage is bold from 70 %. | §4.1's `accent` "normal fills"; §6.2. |
| T3-19 | **Lead card** (§6.2). Title: `  ● {pos} {name}[ · {org}]    {facts}` on the left, the org from 120 columns, cut by `fit_spans` to the room the right part leaves; on the right the forecast's words, never cut (dropped only below 16 columns of room). Facts: `in use`, the kind's display name, `in a session`, `login valid …`. Bars: `    {label}  {bar}  {pct:>4}[ ▲]  resets in {span}[ · {projection}]`, cut by `fit`; the label column is the widest shown label, at least 4. At 80 columns only the first short and the first long window, and the projection is its clause (`lasts`, `full in 2d04h`); from 120 every window, a spend window showing `€12.40 of €50` in place of its percentage. The projection is warn when `will_last_to_reset` is false. Its height is bounded by T3-22 and folds by T3-32. | §6.2's parts, laid out as its mock-up. |
| T3-20 | **Selected card** (§6.4). Title: `    {pos} {name}[ · {org}]    {facts}`, the org from 120 columns, the name cut before the facts. Facts: `in use`/`not in use`, the kind's display name, `in a session`, `▶ this session`, `disabled`, `updated {age} ago` (warn for an old reading) or `not checked yet`, `next update in {span}` while one is planned and the status is `ok`, `login valid …`. Window rows: `    {label}  {spark} {tag}  {pct:>4}[ ▲]  {reset}[ · {projection}]`, the tag padded to 3 (`24h`, `7d `); the label column is the widest label, at least 4 and at most a quarter of the width (a longer one is cut). The reset is `resets in {span}`, `reset` once passed, or `not started` for a short or long window without one. The projection shows only with a rate. From 120 columns `▲` becomes `▲ faster than an even pace (62% expected by now)` when the row still fits. For the account in use, no reset and no projection on the windows the lead card shows (T3-32). | §6.4's parts. |
| T3-21 | **The card's history read** is `Cmd::LoadCard`/`Msg::Card` on the views lane, under its own generation (`gens.card`), coalescing like `History`. It is due 250 ms after the selection changes (the first selection included), checked on the 250 ms tick, and again after an accounts read in which the selected account's `fetched_at` changed. A result for another account or an older generation is dropped. A failed read keeps the last view of that account, else draws empty buckets; it raises no banner. | §6.4: one `history` read, debounced to 250 ms. The accounts read's banner already covers a broken store. |
| T3-22 | **Accounts layout** (§6.1), top down: the lead card, a blank row, the table, a blank row, the selected card; spare rows stay at the bottom. Each part's height is bounded, in this order:<br>1. **Reserved first:** the table's minimum, `min(its lines, 4)`; the two blank rows; the card's minimum, its title, one window row (when it has windows), its problem rows and its keys row.<br>2. **The lead card** gets its lines, at most what the reservation leaves (T3-32 folds the rest), and never less than 2 rows.<br>3. **The selected card** gets its lines, at most what is left after the lead card, the blanks and the table's minimum. Past that its windows scroll by `J`/`K`, a whole window at a time; the title, problem rows and keys row stay.<br>4. **The table** gets the rest, at most its lines, and scrolls with the selection past that. | VIS-5: content anchors to the top. Without the reservation, an account with many windows at 120 columns would push the table and the card off the body, where neither the selection nor `J`/`K` reaches them. |
| T3-23 | **Selection start** (§6.5): with no selection yet, the first active row in table order (any provider), else row 1. A refresh keeps it by ID, as today. | UX-2. |
| T3-24 | **Now strip** (§5.4): ` ● {pos} {alias, else email}   {label} [{gauge 10} ]{pct:>4} {span}   …` for focus's account in use, its first short and first long window, the gauge from 120 columns; on the right `best now {pos} {name}` or `on the best account` (muted). A problem with the account in use replaces the windows with `{glyph} {status words} · see Accounts` in its role. No account in use: `no account in use · s on Accounts switches to one` (muted). No snapshot yet, or no accounts: the row stays empty. It is one row whatever the account holds (two windows at most). Its left part is cut by `fit_spans` to the room the right part leaves, and the right part is dropped below 16 columns of room. | §5.4; the example's words are the status words, not the STATUS phrase. |
| T3-25 | **D-2's warnings.** `Context.shown_warnings: Arc<[String]>` holds the settings warnings a host has already shown. `run_command`'s one loop that prints the engine's settings warnings skips them; `Io::warn` itself is unchanged, since every other warning it prints is the action's own. `CliHost::new` takes them; the foreground context keeps none, so a foreground action's terminal shows its warnings as today. | §13. `Io` carries no `Context`, and filtering inside it would hide an action's warning that happens to read the same. |
| T3-26 | **CC-O-2/3.** `tui_cmd` builds through `engine_for` and reports its refusals through `app::fail_to`, R3's form of `Failure::report` for sites with no `Io` (`Failure` stays `pub(super)`). It reads `Settings::mtime` from `ctx.env` before `engine_for`, since the run-shell lookup changes no config directory. A lane whose `engine_for` fails builds a `Refused` source that answers every read with that error. The tier's colour is `ColorEnv { stdout_terminal: true, ..ColorEnv::of(&ctx, cli.no_color) }.enabled(setting)`. `color_enabled` is deleted and `locate` becomes private to `context.rs`. | §13's "R3's engine_for and Failure::report; ColorEnv for the tier"; refactor §10: the wrappers retire with the held callers. |
| T3-27 | **Fixtures.** The Accounts fixtures T3 rewrites use §3.4's names; `michael@example.com` and `w@corp.com` (a real domain) go from those tests. | §3.4. |
| T3-28 | **§12.5's retype.** `StatusProvider.mode: Wire<Mode>`, `.waiting_for: Option<Wire<WaitingFor>>`, `StatusOutcome.kind: Wire<OutcomeKind>`, and `Event::Mode`'s two the same. `Wire<T: Spelled>` deserializes from a string. The struct accessors `StatusProvider::{mode, waiting_for}` and `StatusOutcome::kind` go, since the fields are typed; `Event::{mode, waiting_for}` stay. | §12.5, C-11's "the fields can then be retyped". |
| T3-29 | **The keys R3's parsers leave.** `protocol.rs` gains, beside R3's: `Event::changes_an_account()` (`poll`, `switch`, `account-quarantined`, `account-unquarantined`), `Event::all_exhausted() -> Option<AllExhaustedPayload { earliest_reset_at: Option<String> }>`, `Event::error() -> Option<String>`, `Event::config_warning() -> Option<String>` (their `message`), `Event::at() -> Option<&str>` (the envelope's `ts`), and `Event::token_index() -> Option<TokenIndexPayload { changed_at: Option<String> }>` (`token-index`'s `changedAt`, null or absent as `None`), each as lenient as R3's. The no-switch sentence's usage is `PollPayload::binding(active)`'s `used_pct`. No raw key is read in the TUI after Task 9. | The brief: the raw keys go through parsers. R3 has none for these. A poll with no matching window gives the sentence's form without a percentage. |
| T3-30 | **`tagteam auto`'s and `tagteam daemon`'s human lines** (§11.3, shared `HumanSink`): a no-switch is `words::no_switch`'s sentence; `all-exhausted` keeps its countdown after the clock: `every account is near its limit; the first frees up at 16:40:00 (2h40m)`; a switch is `switched to #2 (before it reached the limit)` (`would switch` in dry run); a reason this build cannot parse still prints `no switch: <code>`; the `engine-running` warning stays. | §15: `auto` adopts §11.3. R3's Review Focus 5 pins the unknown reason. |
| T3-31 | **Records.** T3 amends TUI §5.1 (the Now strip row), §5.2 (Accounts) and §5.4's per-window words; core §11.4's human output, §13.1 and §13.4. Core's `events` kind list waits for T4, which writes those kinds. | §18; T3 adds no event kind (§12.4 is T4's). |
| T3-32 | **A lead card taller than its room folds.** Within T3-22's bound, the title and the problem rows are reserved first, so a problem is never folded away; the bars take the rows left, in window order. It draws in §6.2's order: the title, the bars that fit (and the fold line), then the problem and its remedy. When not every bar fits, the last bar row that fits becomes one muted line in place of a bar: `+N more windows · in the card below` when the account in use is the selected one, else `+N more windows · select it for its card`. The 80-column rule (short and long windows only) is §6.2's choice, not a lack of room, so it never folds. The card of the account in use leaves out the reset and projection only for the windows the lead card shows; a folded window, or one the 80-column lead card leaves out, keeps them on the card. | §6.2 shows every window from 120 columns, and §6.4 leaves out what "the lead card already shows". A bounded lead card has to say what it left out and where to find it, and the card must then show it. |

## Review Focus

The five inputs or failure modes the spec implies that no existing test exercises, most likely first. Each one's test goes in the task that owns the code.

1. **A usage failure token this build does not know**, from a newer build, on the account in use. It reads `usage check failed (<token>)`, STATUS `! retrying`, the sentence `Usage check failed (<token>).`, no remedy without a retry time, and nothing panics or hides the row. Tests: Task 3, `an_unknown_failure_token_is_a_failed_check_with_no_remedy_until_a_retry`; Task 11, `a_token_from_a_newer_build_reads_as_retrying`.
2. **Several providers, the selected account's provider having no account in use while another has one.** The lead card says `No account in use`; the Now strip, the banner, the one-account hint and `A` follow that same provider. Tests: Task 6, `a_adds_the_login_of_the_provider_the_screen_follows`; Task 13, `with_several_providers_the_lead_card_follows_the_selected_account_s_provider`; Task 14, `the_now_strip_follows_the_selected_account_s_provider`.
3. **A name far wider than the screen** (a 200-character alias, a CJK email) at 80 columns. The table cuts it by display width to its column and the gauges, UPDATED and the glyph keep their columns; the cards cut the name before their facts; nothing wraps or overflows. Tests: Task 11, `wide_names_are_cut_by_display_width_and_never_overflow`; Task 12, `the_card_title_cuts_the_name_before_the_facts`; Task 13, `a_long_name_on_the_lead_card_keeps_the_best_pick`.
4. **The selected account removed or moved while its card's history read is in flight.** The late result is dropped, and the account now selected reads its own. Test: Task 12, `a_card_read_for_another_account_or_an_older_generation_is_dropped`.
5. **An unreadable live login** (a garbled `~/.claude.json`). The view falls back to the store's active account for `active`, `live` and the forecast; with no stored active account, `live` is `None` and the lead card says no account is in use. Test: Task 6, `an_unreadable_live_login_falls_back_to_the_stored_active_account`.

## Spec coverage

| Spec | Tasks |
|---|---|
| §4.3 emphasis (one bold, DIM disabled row, selection), §4.4 lead bar, line gauge, sparkline, markers; VIS-6 | 10, 11 (T3-16, tint fix), 12, 13 |
| §4.5 widths: 9/14-cell gauges, STATUS from 100, scoped and SPEND from 160, Now-strip gauges from 120 | 11, 13, 14 |
| §5.1 the Now strip row; §5.4 | 14 |
| §5.6 unmanaged-login banner per provider; §5.7 one-account hint | 6 |
| §6.1 layout; VIS-5 | 13 |
| §6.2 lead card and the best pick; VIS-1, UX-2 | 13 |
| §6.3 table, STATUS; VIS-3, VIS-4 | 11 |
| §6.4 selected card; VIS-13 | 12 |
| §6.5 selection start, `J`/`K`, `⏎` | 12, 13 |
| §11.1 shared words; UX-12; `list`'s and `history`'s snapshots | 2 |
| §11.2 problems and remedies; UX-9 | 3, 11–13 |
| §11.3 in `tagteam auto` | 4 |
| §12.1 `best_candidate`, parity | 5, 6 |
| §12.2 `models`, `live`, `forecast`; PERF-R-10a (§14) | 6 |
| §13 D-2 (unlock, warnings) | 7 |
| §13 CC-O-2/3 (`engine_for`, `Failure::report`, `ColorEnv`) | 8 |
| §13 ARCH-3/CC-O-7 TUI side, CC-O-23f; §12.5 | 9 |
| §13 ARCH-1 allowlist; T2's stand-ins | 1 |
| §16 tests for the above | 1–14 |
| §18 TUI-spec and core-spec hunks; CHANGELOG | 15 |

Out of T3, by §15: `Engine::timeline`, the store events and their writers, `last_auto_event`, §11.4's sentences, `w` and the History screen (T4); Tokens' §8 layout and PERF-D-2 (T5).

**Order.** Every task builds and tests the same crates, so tasks never run in parallel: one implementer at a time, in the order below. The `Depends on` lines name the hard dependencies.

---

## Before Task 1 (controller)

- [ ] **The branch.** `git -C /Users/michael/Code/tagteam-tui-redesign status --short` prints nothing, and the T3 branch sits on main after #25 and R4a merged. Record that SHA as `<base>` in this plan.
- [ ] **The refactor's names** (T3-1). Each command prints at least one hit; a miss means the name changed, and this plan is patched before Task 1:
  - `rg -n 'fn engine_for|fn locate_run_shell|struct Located|struct ColorEnv|fn color_enabled|pub\(crate\) fn locate' crates/tagteam/src/app/context.rs`
  - `rg -n 'fn fail_to|fn report' crates/tagteam/src/app/failure.rs`
  - `rg -n 'UNLOCK_QUESTION|fn ensure_unlocked|io.warn\(w\)' crates/tagteam/src/app/mod.rs`
  - `rg -n 'pub enum Wire|pub trait Spelled|fn collects_for_itself|fn waits_for_upgrade|pub fn parse_poll|pub fn binding|pub struct NoSwitchPayload' crates/tagteam-daemon/src/protocol.rs`
  - `rg -n 'pub enum AutoStrategy|pub const ALL' crates/tagteam-core/src/autoswitch.rs`
  - `rg -n 'pub enum FailureKind' crates/tagteam-engine/src/failure.rs`; `rg -n 'Errored' crates/tagteam-engine/src/collect.rs`
  - `rg -n 'fn best_pick|fn usage_pick|fn is_candidate|fn switch_candidate' crates/tagteam-engine/src/switch/select.rs`
- [ ] **What the rebases left for Task 1:** `rg -n '\bNO_SWITCH_REASONS\b|\bno_switch_reason\(|autoswitch::Strategy\b|\bStrategy::(Best|ConsumeFirst)\b' crates --glob '!**/target/**'` and `rg -n 'crates/tagteam-tui/src/(screens/accounts|actions)\.rs' crates/tagteam-fake/tests/neutrality.rs` (R1's two `ALLOWED` entries, not the `NEUTRAL` crate list). Note the hits in Task 1's report.
- [ ] **The harness is parked** in `.superpowers/tui-redesign/harness/` (`ux_gallery.rs`, `ux_concept.rs`, `ux_redesign.rs`, `lib.patch`).
- [ ] **The "before" renders** are `.superpowers/tui-redesign/gallery/t2-after-15`.
- [ ] **The gallery check**, run after Tasks 11, 13 and 14:
  - Copy `ux_gallery.rs` into `src/` and apply its `lib.rs` line from `lib.patch`. Apply `ux_concept.rs` and `ux_redesign.rs` only if they still compile; they hold the spec's mock-ups.
  - Fix only the harness's own compile errors against the new APIs (`ProviderAccounts`' three fields, `AccountsSnapshot` without `unmanaged_live`, `StatusProvider`'s typed fields). It is never committed.
  - **New scenarios**, added once, from Task 11's check on, built from its fixtures:
    - `accounts-best-pick`, `accounts-already-best`, `accounts-usage-unknown` (the three forecasts);
    - `accounts-problem-in-use` (the account in use quarantined) and `accounts-old-reading`;
    - `accounts-no-account-in-use` (an unmanaged live login) and `accounts-one-account`;
    - `accounts-disabled-selected`;
    - `accounts-twelve` (the table scrolling between the cards);
    - `accounts-scoped-spend` at 160×40 as well as the usual sizes;
    - from Task 14's check on, `tokens-now-strip` and `history-now-strip`.
  - Render: `UX_GALLERY_DIR=$PWD/.superpowers/tui-redesign/gallery/t3-after-<task> cargo test -p tagteam-tui ux_gallery -- --nocapture`.
  - Check by eye, at 80×24 and 120×40 (and 160×40 for `accounts-scoped-spend`) in tiers 0, 1 and 2 (dark and light), against the spec's mock-ups: §6.1's Accounts, §6.2's forecast table, §6.3's STATUS table, §6.4's card, §5.4's Now strip, §5.7's one-account hint, and on History at 80×24 that the header, the auto line and the Now strip take three rows and the footer one, leaving 20 (§7.8's budget).
  - Park the harness again.

  A difference from the spec is a defect of the task that drew it: send it back to that task's implementer. The renders show real names: never quote them in a commit, the PR or a review.

---

### Task 1: Use the refactor's types in place of T2's stand-ins

**Depends on:** none

**Files:**
- Modify: `crates/tagteam-engine/src/words.rs`. Delete `NO_SWITCH_REASONS`. Its tests iterate `NoSwitchReason::ALL`: `every_no_switch_reason_has_its_sentence`, and `the_reason_list_is_every_reason_once`, which keeps its exhaustive `index` match over `ALL`. `every_trigger_has_its_words` iterates `Trigger::ALL` (§16: a test over each enum's `ALL`).
- Modify: `src/daemon/mod.rs`. Delete `no_switch_reason`; its callers (`note`, and `autoline.rs`'s `outcome`) call `NoSwitchReason::parse`.
- Modify: `src/source.rs`. `rate_limited` matches `Collected::Failed { kind: FailureKind::Provider(TransientKind::Http(429)) }`. The docs of `recorded` and `CollectSummary.rate_limited` name `Collected::Errored` and that `FailureKind` (R4a's Ruling 9 left them stale).
- Modify, only where the rebases left them: every `tagteam_core::autoswitch::Strategy` becomes `AutoStrategy` (`views.rs`'s `ProviderAccounts.strategy`, `daemon/words.rs`, the TUI's test literals, `render.rs`'s and `fake_agent.rs`'s tests); `crates/tagteam-fake/tests/neutrality.rs` loses its `accounts.rs` and `actions.rs` `ALLOWED` entries.

- [ ] **Step 1: Make the edits** listed in Files.
- [ ] **Step 2: Check nothing is left.** Run `rg -n '\bNO_SWITCH_REASONS\b|\bno_switch_reason\(|autoswitch::Strategy\b|\bStrategy::(Best|ConsumeFirst)\b|kind == "http-429"' crates --glob '!**/target/**'`. Expected: no hit.
  - Each pattern names only the old identifier. `\bStrategy::` cannot match `AutoStrategy::` or `UsageStrategy::`, since no word boundary falls inside a name.
  - `\bno_switch_reason\(` matches the helper's declaration and its call sites (`fn no_switch_reason(`, `daemon::no_switch_reason(`), but not the tests that keep the words in their names (`every_no_switch_reason_has_its_sentence`).
- [ ] **Step 3: Run the tests.** Run `cargo test -p tagteam-engine --locked --lib words::`, `cargo test -p tagteam-tui --locked` and `cargo test -p tagteam-fake --locked --test neutrality`. Expected: all pass.
- [ ] **Step 4: Commit.** Run `git add -A crates/tagteam-engine/src crates/tagteam-engine/tests crates/tagteam-tui crates/tagteam/src/render.rs crates/tagteam-fake/tests && git commit -m "Use the refactor's reason list, strategy name and failure kinds in place of T2's stand-ins"`.

### Task 2: Say usage states, pace and projections in one plain vocabulary

**Depends on:** none

**Files:**
- Modify: `crates/tagteam-engine/src/words.rs`: §11.1's table, T3-8, T3-9, T3-10.
- Modify: `crates/tagteam-engine/src/views.rs`: `UsageStatus::ALL`.
- Modify: `crates/tagteam/src/render.rs`: `table`'s `"AGE"` head becomes `words::UPDATED_HEAD`; `status_human`'s `", relogin required"` becomes `", needs a new login"`.
- Modify: `src/overlay/help.rs`: the rate line (T3-9).
- Move the expected words, and nothing else, in:
  - `words.rs`'s and `views.rs`'s tests; `render.rs`'s and `history.rs`'s test modules;
  - `crates/tagteam/tests/usage_cli.rs`, `app.rs`, `cli.rs`, `run_shell_cli.rs`, `history.rs`;
  - `src/screens/accounts.rs`'s and `src/screens/history.rs`'s tests that read these functions' output (`relogin required`, `▲ pace`, `pts/h`, `runs out`, `(regression)`, `no rate yet`, `unavailable (…)`);
  - `crates/tagteam/tests/tui_pty.rs:993`'s comment (`"no data yet"`, `"unavailable (pre-send, ...)"`).

  Where a shorter projection now fits a line that the old one wrapped (`src/screens/history.rs`'s tests at `:622` and `:683`), the fixture's width shrinks so the test still wraps. `UPDATED` is wider than `AGE`, so in `list`'s expected rows the notes after that column move right by the difference (`UPDATED`'s 7 less the column's old width); that is the only spacing change.

**Interfaces:**
- Produces, in `tagteam_engine::words`:
  ```rust
  /// §8.7's marker in `list` and `status` (§11.1): was `▲ pace`.
  pub const AHEAD: &str = "▲ fast";
  /// `list`'s head for a reading's age (§11.1): was `AGE`.
  pub const UPDATED_HEAD: &str = "UPDATED";
  /// An account never read (§11.1): was `no data yet`, and `never fetched` in the TUI.
  pub const NOT_CHECKED: &str = "not checked yet";
  /// A usage status in words without its retry (§11.1, T3-8): `needs a new login`,
  /// `rate-limited`, `usage check failed (refresh-failed)`, `not checked yet`.
  pub fn status_words(u: &UsageView) -> String;
  /// `▲ faster than an even pace (62% expected by now)`, or without the parenthesis.
  pub fn ahead_words(expected_pct: Option<f64>) -> String;
  /// A projection's clause (§11.1): `lasts until it resets` (`lasts` when `short`),
  /// `full in 2d04h`, `should have been full 3h ago`, `won't fill within a year`,
  /// `at the limit`, `not enough data yet`, `no projection`.
  pub fn projection_clause(hw: &HistoryWindow, now_s: i64, short: bool) -> String;
  /// `updated 2m ago`.
  pub fn updated(age_s: i64) -> String;
  /// `next update in 1m`.
  pub fn next_update(at: i64, now_s: i64) -> String;
  /// T3-10: `login valid 6 more days`, `… 1 more day`, `… 5h20m more`, `login expired`.
  pub fn login_valid(expires_at_ms: i64, now_s: i64) -> String;
  // views.rs
  impl UsageStatus { pub const ALL: [UsageStatus; 9]; }
  ```
- Changed, signatures kept:
  - `usage_words(u, now_s)` is `status_words` with the retry joined as today (T3-8);
  - `projection_text(hw, now_s)` is `{rate_signed(rate)}%/h · {clause}`, or the clause alone without a rate, with no method.
- **The heads** (§11.1): `ReloginRequired` `needs a new login`, `TokenExpired` `login expired`, `KeychainUnavailable` `can't read the Keychain`, `ForeignCredential` `logged in as another account`, `NoCredentials` `no stored login`, `Unavailable` by `FailureKind::parse(error)` (T3-8; `NO_DATA` or none is `not checked yet`). `api key`, `no usage reported` and `usage unsupported` do not change.
- **The help line** is an entry under Marks: `%/h` (muted), then `how fast a window fills: a fit through its last two days of readings, or its average so far when there are too few`.

- [ ] **Step 1: Write the tests** in `words.rs`'s test module:
  - **`every_usage_status_has_its_words`.** For each of `UsageStatus::ALL` with no error and no retry, `status_words` is its head above; `the_status_list_is_every_status_once` holds `ALL` to an exhaustive `index` match (0–8), as T2's reason list test does.
  - **`a_failure_token_is_said_in_its_kind_s_words`**, `usage_words` of `Unavailable` with:
    - `http-429` and a retry in 5m: `rate-limited (retry 5m)`;
    - `refresh-failed` and 5m: `usage check failed (refresh-failed, retry 5m)`;
    - `pre-send`, no retry: `usage check failed (pre-send)`;
    - `over-budget` and 12m: `paused to stay within the request budget (retry 12m)`;
    - `http-503`: `usage check failed (http-503)`;
    - `no-data`: `not checked yet`.
  - **Moved:** `every_status_says_when_it_is_retried_while_that_is_ahead` (`can't read the Keychain (retry 2m)`); `a_rows_notes_are_its_status_kind_disabled_and_this_in_that_order`; R4a's `an_unknown_failure_token_is_unavailable_in_its_own_spelling`, now `usage check failed (a-token-from-a-newer-build)`.
  - **`a_projection_says_how_fast_and_when_the_window_is_full`**, rewriting `a_projection_lasts_runs_out_or_is_missing`, `an_exhausted_window_is_at_the_limit_with_or_without_a_rate` and `a_projection_beyond_a_year_says_it_will_not_run_out`: `+1.2%/h · lasts until it resets`, `+5.0%/h · full in 12h00m`, `+5.0%/h · should have been full 3h00m ago`, `+0.03%/h · won't fill within a year`, `+1.0%/h · at the limit`, `at the limit`, `not enough data yet`. No result contains `regression`, `average` or `pts/h`.
  - **`the_short_clause_is_lasts_or_the_full_time`:** `projection_clause(.., true)` gives `lasts` where the long form is `lasts until it resets`, and `full in 12h00m` unchanged.
  - **`ahead_words_say_the_expected_share`:** `Some(62.0)` gives `▲ faster than an even pace (62% expected by now)`; `Some(61.6)` rounds to 62; `None` gives `▲ faster than an even pace`.
  - **`updated_next_update_and_login_valid_read_as_the_tui_shows_them`:** `updated(120)` is `updated 2m ago`; `next_update(NOW + 60, NOW)` is `next update in 1m`; `login_valid` at `NOW + 6d03h` is `login valid 6 more days`, at `+1d02h` `login valid 1 more day`, at `+5h20m` `login valid 5h20m more`, at `NOW − 1 s` `login expired` (all as ms).
  - **`help_names_how_the_rate_is_measured`** (`help.rs`): the help's Marks section has a row starting `%/h` with those words.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-engine --locked --lib words::`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed in Interfaces, then move the listed expectations.
- [ ] **Step 4: Check the moves change only words.** Run `git diff -U0 HEAD -- crates/tagteam/tests crates/tagteam/src/render.rs crates/tagteam/src/history.rs crates/tagteam-tui/src/screens | rg '^\+[^+]' | rg -v "needs a new login|login expired|can't read the Keychain|logged in as another account|no stored login|rate-limited|usage check failed|paused to stay within|not checked yet|▲ fast|%/h|full in|lasts until it resets|not enough data yet|should have been full|won't fill|UPDATED"`. Expected: only lines whose change Files names (the two narrowed fixture widths, and `list` rows whose notes moved right after `UPDATED`), or none.
- [ ] **Step 5: Run the tests.** Run `cargo test -p tagteam-engine --locked`, `cargo test -p tagteam-tui --locked`, `cargo test -p tagteam --locked --features test-support --lib` and `cargo test -p tagteam --locked --features test-support --test usage_cli --test app --test cli --test run_shell_cli --test history`. Expected: all pass.
- [ ] **Step 6: Commit.** Run `git add -A crates/tagteam-engine/src crates/tagteam/src/render.rs crates/tagteam/src/history.rs crates/tagteam/tests crates/tagteam-tui && git commit -m "Say usage states, pace and projections in one plain vocabulary"`.

### Task 3: Name each account's problem and how to fix it

**Depends on:** Task 2

**Files:**
- Modify: `crates/tagteam-engine/src/words.rs`: append the items below before `#[cfg(test)]`, and their tests to its test module.

**Interfaces:**
- Produces, in `tagteam_engine::words`:
  ```rust
  /// What needs attention on an account (redesign §6.3, §11.2, T3-11).
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Problem { NeedsLogin, LoginExpired, NoLogin, RateLimited, Keychain, Budget, OtherAccount, Retrying, OldReading, NotChecked }
  impl Problem { pub const ALL: [Problem; 10]; }
  /// The account's one problem, from its usage: `None` for a healthy reading, an API key or an
  /// unsupported provider. Disabled is not a problem.
  pub fn problem(v: &AccountView) -> Option<Problem>;
  /// T3-12's sentence. `when` is the quarantine's time in the caller's form (`Sat 09:02`).
  pub fn problem_sentence(problem: Problem, v: &AccountView, when: Option<&str>) -> String;
  /// §11.2's remedy (T3-13): `Log in to Claude Code as alex@example.com, then press A here.`
  pub fn remedy(problem: Problem, v: &AccountView, names: &ProviderNames, now_s: i64) -> Option<String>;
  ```
- **`problem`:** `ReloginRequired` → `NeedsLogin`, `TokenExpired` → `LoginExpired`, `NoCredentials` → `NoLogin`, `KeychainUnavailable` → `Keychain`, `ForeignCredential` → `OtherAccount`; `Unavailable` by `FailureKind::parse`: `Provider(Http(429))` → `RateLimited`, `OverBudget` → `Budget`, `NO_DATA` or none → `NotChecked`, anything else → `Retrying`; `Ok` with windows and not decision-grade → `OldReading`.
- **Remedies** (§11.2, T3-13), `{display}` from `names`, `{email}` the row's email else its label, `{span}` a `duration` to the time named:
  - `NeedsLogin`, `LoginExpired`, `NoLogin`: `Log in to {display} as {email}, then press A here.`
  - `Keychain`: `Unlock it (security unlock-keychain), then press r.`
  - `RateLimited`, `Retrying`: `tagteam tries again in {span}.` (`usage.retry_at`), else none.
  - `Budget`: `tagteam checks again in {span}, when the hourly budget allows.`, else `tagteam checks again when the hourly budget allows.`
  - `OtherAccount`: `{display} is logged in as a different account. Switch with s, or log in again.`
  - `NotChecked`, `OldReading`: `tagteam checks it within {span}.` (`usage.next_poll_at`, else `retry_at`), else `Press r to check it now.`

- [ ] **Step 1: Write the tests:**
  - **`every_problem_has_its_sentence_and_remedy`:** for each of `Problem::ALL`, with an account built to have it (a quarantine `invalid_grant` at a time the test names `Sat 09:02`, retry and poll times 4m and 12m ahead), the sentence is T3-12's and the remedy §11.2's, exactly. Example: `Needs a new login: its stored login was refused (invalid_grant), Sat 09:02.` and `Log in to Claude Code as alex@example.com, then press A here.`
  - **`the_problem_list_is_every_problem_once`:** an exhaustive `index` match over `ALL`.
  - **`a_problem_follows_the_account_s_usage`:** the mapping above, row by row; a fresh decision-grade reading, an API key, an unsupported provider and a disabled healthy account give `None`.
  - **`an_unknown_failure_token_is_a_failed_check_with_no_remedy_until_a_retry`** (Review Focus 1): `a-token-from-a-newer-build` gives `Retrying`, `Usage check failed (a-token-from-a-newer-build).`, and no remedy until `retry_at` is set, then `tagteam tries again in 4m.`
  - **`a_problem_without_its_facts_still_reads`:** no quarantine reason, no time, no planned poll: `Needs a new login: its stored login was refused.`, `Press r to check it now.`, the budget's fallback.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-engine --locked --lib words::`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-engine --locked`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add crates/tagteam-engine/src/words.rs && git commit -m "Name each account's problem and how to fix it"`.

### Task 4: Say auto's decisions in plain words in tagteam auto and tagteam daemon

**Depends on:** Task 1

**Files:**
- Modify: `crates/tagteam/src/auto.rs`: `HumanSink` (T3-30). `Polled` gains `threshold: f64`, from the poll.
- Move the expected lines in: `auto.rs`'s test module, `crates/tagteam/src/daemon_cmd.rs`'s test module (`no switch: below-threshold` at `:426`), `crates/tagteam/tests/auto_cli.rs`, `crates/tagteam/tests/daemon_cli.rs`.

**Interfaces:**
- **The lines** (T3-30):
  - **A parsed no-switch,** not `engine-running` or `all-exhausted`: the tick's line ends `words::no_switch(reason, &facts)`. The facts are:
    - `detail` as the event has it;
    - `used_pct` and `threshold` from the provider's poll this tick, read before `tick` takes it (without one, `DEFAULT_THRESHOLD`);
    - `display_name` from `self.names`.
  - **`all-exhausted`:** `facts.frees_at` is `{clock(at)} ({duration(at − now)})`, giving `every account is near its limit; the first frees up at 16:40:00 (2h40m)`; without a reset, `every account is near its limit`.
  - **A switch:** `{verb} to #{to} ({words::trigger(trigger)})`.
  - **Unchanged:** a reason `parse` refuses keeps `no switch: {reason}[ ({detail})]`; the `engine-running` warning stays.

- [ ] **Step 1: Write the tests** in `auto.rs`'s test module:
  - `each_no_switch_line_is_its_plain_sentence`: through `human(..)`, `cooldown` `4m` gives `waiting 4m after the last switch`; `below-threshold` after a poll at 77 % with threshold 90 gives `#2 b@x.co  77%  no switch needed (77%, switches at 90%)`; `active-usage-unknown` `2/3` gives `can't read the account in use (2 of 3 tries)`; `no-active-account` gives `Claude Code isn't logged in`.
  - `all_exhausted_says_when_the_first_frees_up_and_how_long_that_is`: with and without `earliest_reset_at`.
  - `a_switch_line_says_its_trigger_in_words`: `switched to #2 (before it reached the limit)`, and `would switch to #2 (at the limit)` in dry run.
  - R3's `an_unknown_no_switch_reason_is_still_printed` passes unchanged.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam --locked --features test-support --lib auto::`. Expected: failures.
- [ ] **Step 3: Implement** as listed, and move the listed expectations.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam --locked --features test-support --lib` and `cargo test -p tagteam --locked --features test-support --test auto_cli --test daemon_cli`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add crates/tagteam/src/auto.rs crates/tagteam/src/daemon_cmd.rs crates/tagteam/tests/auto_cli.rs crates/tagteam/tests/daemon_cli.rs && git commit -m "Say auto's decisions in plain words in tagteam auto and tagteam daemon"`.

### Task 5: Forecast switch --strategy best's pick with one pure function

**Depends on:** none

**Files:**
- Modify: `crates/tagteam-engine/src/switch/select.rs`: the types and `best_candidate` (T3-2), and `best_pick` through it (T3-3). Its unit tests go in a new `#[cfg(test)] mod tests` at the end of the file.
- Modify: `crates/tagteam-engine/src/switch/mod.rs`: `pub use select::{BestForecast, LiveLogin, NoPick, PickInput, PickRow, best_candidate};` and `pub(crate) use select::is_candidate;`.
- Test: `select.rs`; `tests/strategy.rs` is the safety net, unchanged.

**Interfaces:**
- Produces, in `tagteam_engine::switch` (spec §12.1):
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum LiveLogin { None, Unmanaged { email: String }, Managed { position: u32 } }
  #[derive(Debug, Clone, PartialEq)]
  pub struct PickInput { pub live: LiveLogin, pub force: bool, pub rows: Vec<PickRow> }
  #[derive(Debug, Clone, Copy, PartialEq)]
  pub struct PickRow { pub position: u32, pub candidate: bool, pub headroom: Option<f64> }
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum BestForecast { Pick { order: Vec<u32> }, AlreadyBest, None(NoPick) }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum NoPick { UnmanagedLogin, TooFewCandidates, UsageUnknown, CandidatesExhausted }
  pub fn best_candidate(input: &PickInput) -> BestForecast;
  ```
- **`best_candidate`, in this order** (§12.1, T3-3):
  1. `Unmanaged` without `force`: `None(UnmanagedLogin)`. With `force` it reads as `None` from here on.
  2. With `Managed { position }`: fewer than two rows with `candidate` (the live row counted if it is one) give `None(TooFewCandidates)`.
  3. The ranked set is every row with `candidate` other than the live position. Empty, with no managed live login: `None(TooFewCandidates)`.
  4. The live headroom is the live row's `headroom` (managed only).
  5. `best_order(live_headroom, ranked)`:
     - `UsageUnavailable` gives `None(UsageUnknown)`;
     - `AlreadyBest` gives `AlreadyBest`;
     - `Try(order)` with a managed live login of unknown headroom keeps the positions whose headroom is above 0, and none left gives `None(CandidatesExhausted)`. Otherwise it is `Pick { order }`.
- **`best_pick`** gains `p: &dyn Provider` (from `usage_pick`). It builds the input from `rated` (`candidate: true`, `headroom: r.headroom(models)`) and the live row (`candidate: self.switch_candidate(p, live)?`, its decision-grade headroom). The live login is `Managed` or `None`, and `force` is false. The forecast's arms map to today's:
  - `None(UnmanagedLogin | TooFewCandidates)` is `nothing_to_activate(live_row)`;
  - `None(UsageUnknown)` is today's `UsageUnavailable` arm;
  - `AlreadyBest` is today's arm, with its leader;
  - `None(CandidatesExhausted)` is today's, with the exhausted candidates: the rated ones whose known headroom is at most 0, in `best_order`'s order;
  - `Pick { order }` walks `order` as today, with today's fallbacks after a walk that finds none.

  No outcome, reason, message or warning changes.

- [ ] **Step 1: Write the test** `best_candidate_follows_the_switch_s_rules_in_order` in `select.rs`, one `PickInput` per row:

  | Case | Forecast |
  |---|---|
  | unmanaged live, `force` false | `None(UnmanagedLogin)` |
  | unmanaged live, `force` true, candidates 1 (h 40) and 2 (h 10) | `Pick { [1, 2] }` |
  | managed live 1 not a candidate (disabled), one other candidate 2 (h 40) | `None(TooFewCandidates)` |
  | managed live 1 (a candidate, h 20), candidate 2 (h 40) | `Pick { [2] }` |
  | no live login, no candidate | `None(TooFewCandidates)` |
  | no live login, candidate 2 (h −5) | `Pick { [2] }` |
  | managed live 1 (unknown), candidates 2 (h 10), 3 (h −5) | `Pick { [2] }` |
  | managed live 1 (unknown), candidates 2 (h −5), 3 (h 0) | `None(CandidatesExhausted)` |
  | managed live 1 (h 30), candidates 2 and 3 unknown | `None(UsageUnknown)` |
  | managed live 1 (a candidate, h 47), candidate 2 (h 46) | `AlreadyBest` |
  | managed live 1 (a candidate, h 47), candidate 2 with a NaN headroom | `None(UsageUnknown)` |
  | no live login, candidates 3 (h 40) and 1 (h 40) | `Pick { [1, 3] }` |
- [ ] **Step 2: Run it to see it fail.** Run `cargo test -p tagteam-engine --locked --lib switch::select`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed in Interfaces.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-engine --locked --features test-hooks --test strategy --test switch --test rotation` and `cargo test -p tagteam-engine --locked --lib`. Expected: all pass, `strategy.rs` unchanged.
- [ ] **Step 5: Commit.** Run `git add crates/tagteam-engine/src/switch && git commit -m "Forecast switch --strategy best's pick with one pure function"`.

### Task 6: Carry each provider's models, live login and best pick in the accounts view

**Depends on:** Task 5

**Files:**
- Modify: `crates/tagteam-engine/src/views.rs`:
  - `ProviderAccounts` gains three fields, which `Engine::view` fills from the `Settings::load` it already makes and from `Liveness`;
  - `liveness` calls `crate::hooks::point(self, "view-live-identity")?` before `p.live_identity(&self.env)`;
  - the private `struct LiveLogin` and its uses become `CachedLogin` (T3-2);
  - add `forecast_of`.
- Modify: `src/source.rs`: `EngineSource::accounts` drops its `engine.status(live)` call and `unmanaged_live` (T3-6); the doc says each provider's `live` says it.
- Modify: `src/screens/accounts.rs`: `banner` and the one-account hint follow focus (T3-7); `followed` goes.
- Modify: `src/actions.rs`: `add_provider` returns `Some(daemon::focus(state))`.
- Modify, mechanically:
  - every `ProviderAccounts { … }` literal gains `models: vec![]`, its `live` and its `forecast`: `render.rs`'s two test literals, and in the TUI `testutil.rs`'s `snapshot`, `chrome.rs`, `actions.rs` (two), `autoline.rs`, `daemon/mod.rs`, `screens/accounts.rs` (two) and `screens/history.rs`. Each sets `live` to `Managed` for its active account, else `None`, and `forecast` to `forecast_of(&accounts, &live, &[])`;
  - every `AccountsSnapshot { … }` literal loses `unmanaged_live` (`testutil.rs`, `runtime.rs`, `daemon/mod.rs`). A test that set it sets the provider's `live` to `Unmanaged { email }` instead;
  - `testutil::account`'s `identity_json` becomes `serde_json::json!({})`, as a stored account's is, so that `is_candidate` holds for a fixture account and `forecast_of` ranks it.
- Test: `crates/tagteam-engine/tests/strategy.rs`, `tests/fake_agent.rs`, `tests/views_usage.rs`, `crates/tagteam-tui/tests/source.rs`, `src/screens/accounts.rs`, `src/actions.rs`.

**Interfaces:**
- Consumes: Task 5's types and `best_candidate`.
- Produces:
  ```rust
  pub struct ProviderAccounts {
      // … T2's fields
      /// The effective `autoswitch.models` (§8.2's relevance), the provider's own table first.
      pub models: Vec<String>,
      /// The provider's live login as the view read it (T3-5).
      pub live: LiveLogin,
      /// `best_candidate` over this view (§12.1), computed with it.
      pub forecast: BestForecast,
  }
  /// `best_candidate` over `accounts` as a view shows them (§12.1): each row a candidate when
  /// `is_candidate` and not in a session (T3-4), its headroom from a decision-grade reading's
  /// relevant windows under `models`; no I/O.
  pub fn forecast_of(accounts: &[AccountView], live: &LiveLogin, models: &[String]) -> BestForecast;
  ```

- [ ] **Step 1: Write the engine tests:**
  - **`strategy.rs`, `the_accounts_view_s_forecast_is_what_switch_best_picks`** (§12.1's parity test). Each case runs in a fresh `Fx`. A known reading comes from `read` (decision-grade, its plan in force, so not due). An unknown one is never read, or recorded stale with `record_at`; `prepare_usage` tries it through the scripted port, which has no reply, so it stays unknown. That is the collection stub, as `best_without_a_known_candidate_is_usage_unavailable` already uses it. For each case, `fx.engine.accounts(Some(&fx.provider()))?[0].forecast` is taken first, then `best(&fx)` runs, and they agree:
    - `Pick { order }` with `outcome.switched` and `outcome.to.position == order[0]`;
    - `AlreadyBest` with `SwitchReason::AlreadyBest`;
    - `None(UnmanagedLogin)` with `UnmanagedAccount`;
    - `None(TooFewCandidates)` with `OnlyOneAccount` or `NoValidTarget`;
    - `None(UsageUnknown)` with `UsageUnavailable`;
    - `None(CandidatesExhausted)` with `CandidatesExhausted`.

    The cases are §12.1's: the fewer-than-two quirk with a live account that is itself disabled; an unmanaged live login; no live login and a candidate at negative headroom; unknown live headroom; a stale candidate reading; a tie, where the lower position wins.
  - **`fake_agent.rs`, `each_provider_s_view_carries_its_models_and_live_login`.** `config.toml` has `[autoswitch] models = ["fable"]` and `[provider.fake-agent.autoswitch] models = ["all"]`. Claude Code's live login is an added account, and FakeAgent's is a login it never added. `accounts(None)` gives:
    - claude-code `models == ["fable"]` and `live == Managed { position }`;
    - fake-agent `models == ["all"]` and `live == Unmanaged { email }`.
  - **`fake_agent.rs`, `#[cfg(feature = "test-hooks")] the_accounts_view_reads_each_provider_s_live_login_once`.** `on_point("view-live-identity", …)` counts calls. One `accounts(None)` over the two providers counts 2.
  - **`views_usage.rs`, `an_unreadable_live_login_falls_back_to_the_stored_active_account`** (Review Focus 5). With two accounts, the store's active one at position 2, and a garbled `.claude.json`, `accounts()`'s row 2 is `active` and `live == Managed { position: 2 }`. With no stored active account, `live == LiveLogin::None`, no row is active, and the forecast is computed as with no live login.
- [ ] **Step 2: Write the TUI tests:**
  - **`tests/source.rs`:** the test that read `snap.unmanaged_live` (`:238`, `:252`) reads `snap.providers[0].live == LiveLogin::Unmanaged { email: "s@x.co".into() }`.
  - **`screens/accounts.rs`, `the_unmanaged_login_banner_follows_the_selected_account_s_provider`:** with a FakeAgent account selected and FakeAgent's `live` `Unmanaged`, the banner reads `FakeAgent is logged in as …, which tagteam doesn't manage · press A to add it`. With a Claude Code account selected and only FakeAgent's login unmanaged, there is no banner.
  - **`screens/accounts.rs`, `with_one_account_in_the_provider_followed_the_table_says_how_to_add_a_second`:** one Claude Code account and two FakeAgent ones. With the Claude Code account selected the hint names Claude Code; with a FakeAgent one selected there is no hint.
  - **`actions.rs`, `a_adds_the_login_of_the_provider_the_screen_follows`** (Review Focus 2): `A` gives `AddLive { provider }` for the selected account's provider; with no accounts, the default's.
- [ ] **Step 3: Run them to see them fail.** Run `cargo test -p tagteam-engine --locked --features test-hooks --test strategy --test fake_agent --test views_usage`. Expected: a compile error.
- [ ] **Step 4: Implement** as listed, and make the mechanical edits.
- [ ] **Step 5: Run the tests.** Run `cargo test -p tagteam-engine --locked --features test-hooks`, `cargo test -p tagteam-tui --locked` and `cargo test -p tagteam --locked --features test-support --lib`. Expected: all pass, `list`'s and `status`'s output unchanged.
- [ ] **Step 6: Commit.** Run `git add -A crates/tagteam-engine crates/tagteam-tui crates/tagteam/src/render.rs && git commit -m "Carry each provider's models, live login and best pick in the accounts view"`.

### Task 7: Ask the host to unlock the keychain and tell it which warnings were shown

**Depends on:** none

**Files:**
- Modify: `crates/tagteam/src/prompt.rs`: `Prompter::confirm_unlock`.
- Modify: `crates/tagteam/src/app/mod.rs`: `ensure_unlocked`'s `self.io.prompter.confirm(UNLOCK_QUESTION, true)` becomes `self.io.prompter.confirm_unlock()`; `run_command` takes `let shown = Arc::clone(&ctx.shown_warnings);` before `engine_for` and prints only the warnings `shown` does not hold (T3-25). `App.settings_warnings` keeps them all, so `config`'s `read_warnings` filter is unchanged.
- Modify: `crates/tagteam/src/app/context.rs`: `Context.shown_warnings`, empty in `from_process`.
- Modify: `crates/tagteam/src/tui_host.rs`:
  - `CliHost::new` takes `shown: Vec<String>` and sets `inside.shown_warnings`;
  - delete `startup_warnings`, `with_startup_warnings`, `without_startup_warnings` and the engine build in `new`;
  - `act` returns the captured stderr as it is;
  - `Bridge` overrides `confirm_unlock` (it notes `needs_terminal` and returns false), and `Bridge::confirm` only forwards.
- Modify: `crates/tagteam/src/tui_cmd.rs`: `CliHost::new(&ctx, shutdown.clone(), exe, warnings.clone())`.
- Modify, mechanically: every `Context { … }` literal gains `shown_warnings: Arc::from(Vec::<String>::new())`. They are in `app/context.rs`'s and `statusline.rs`'s test modules, and in `tests/app.rs`, `tests/daemon_in_process.rs`, `tests/tui_cli.rs` and `tests/tui_host.rs` (`rg -n 'Context \{' crates/tagteam`). `tests/tui_host.rs`'s `host()` passes its warnings to `CliHost::new`.
- Test: `prompt.rs`, `tui_host.rs` (a new test module), `tests/tui_host.rs`, `tests/app.rs`.

**Interfaces:**
- Produces:
  ```rust
  // prompt.rs
  pub trait Prompter {
      // …
      /// Appendix A.3: whether to unlock the login keychain now. `confirm` of
      /// `app::UNLOCK_QUESTION`, default yes, unless the prompter hands the terminal over instead.
      fn confirm_unlock(&mut self) -> bool { self.confirm(crate::app::UNLOCK_QUESTION, true) }
  }
  // app/context.rs
  pub struct Context { /* … */ pub shown_warnings: Arc<[String]> }
  // tui_host.rs
  impl CliHost { pub fn new(ctx: &Context, shutdown: Cancel, exe: PathBuf, shown: Vec<String>) -> CliHost; }
  ```

- [ ] **Step 1: Write the tests:**
  - **`prompt.rs`, `the_unlock_prompt_is_a_confirm_of_its_question_by_default`:** a prompter that records its confirms answers `confirm_unlock()` with what its `confirm` returned for `(UNLOCK_QUESTION, true)`.
  - **`tui_host.rs`, `the_bridge_hands_the_unlock_over_and_forwards_every_confirm`:**
    - over a recording `Ask`, `confirm_unlock()` is false, sets `needs_terminal`, and asks nothing;
    - `confirm(app::UNLOCK_QUESTION, true)` reaches the `Ask` unchanged.
  - **`tests/app.rs`, `a_warning_the_host_showed_is_left_out_and_a_new_one_is_printed`:** a `config.toml` that gives two warnings, and a `Context` whose `shown_warnings` holds the first. `list`'s stderr has only the second, once.
  - **`tests/tui_host.rs`, rewritten:**
    - `the_settings_warnings_the_tui_printed_are_not_repeated_in_an_action_s_output`: the host is given the warnings `cli_run(["list"])` printed, as `tui_cmd` passes what it printed;
    - `the_start_up_warnings_the_tui_names_are_the_ones_left_out`: its two hosts are built with `CliHost::new(.., vec![text, "another".into()])` and `CliHost::new(.., vec![])`.

    `a_settings_warning_that_appears_after_start_up_is_shown_by_the_action` and `a_locked_keychain_hands_the_action_to_the_terminal_with_nothing_written` pass unchanged.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam --locked --features test-support --lib prompt:: tui_host::`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam --locked --features test-support` (the lib and every integration test). Outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add crates/tagteam/src/prompt.rs crates/tagteam/src/app crates/tagteam/src/tui_host.rs crates/tagteam/src/tui_cmd.rs crates/tagteam/src/statusline.rs crates/tagteam/tests && git commit -m "Ask the host to unlock the keychain and tell it which warnings were shown"`.

### Task 8: Build the TUI's engines through engine_for and choose its colour through ColorEnv

**Depends on:** Task 7

**Files:**
- Modify: `crates/tagteam/src/tui_cmd.rs` (T3-26):
  - `run`:
    - `let config_mtime = Settings::mtime(&inside.env);` first;
    - then `app::engine_for(inside.clone(), provider.as_ref())`, whose `Err(e)` returns `app::fail_to(&mut out, &mut err, false, e.kind(), &e.to_string())`;
    - the unknown `--provider` and the names refusals go through `fail_to` too;
    - `color` is `ColorEnv`'s.

    `TUI_UNDER_JSON`'s refusal keeps its own copy (§13).
  - `sources` builds through `app::engine_for`. On `Err(e)` it returns `Box::new(Refused(SourceError::from(e)))`.
  - Add the private `struct Refused(SourceError)`, whose every `Source` method returns `Err(self.0.clone())`.
- Modify: `crates/tagteam/src/app/context.rs`: delete `color_enabled`. `locate` becomes private (or is inlined into `locate_run_shell`). The test `colour_is_off_under_no_color_forced_by_force_color_and_else_the_settings` calls `ColorEnv { … }.enabled(setting)` (a mechanical API change).
- Modify: `crates/tagteam/src/app/mod.rs`: the `context` re-exports lose `locate` and `color_enabled`, and whatever else no caller uses now.

- [ ] **Step 1: Write the test** `a_source_whose_engine_cannot_be_built_refuses_every_read_in_the_cli_s_words` in `tui_cmd.rs`'s test module. `Refused` built from `EngineError::RunShellUnreadable { … }` answers `accounts`, `history`, `collect_due`, `refresh_tokens`, `token_report` and `token_index_state` with that error's `kind()` and `to_string()`.
- [ ] **Step 2: Run it to see it fail.** Run `cargo test -p tagteam --locked --features test-support --lib tui_cmd::`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Check the wrappers are gone.** Run `rg -n 'app::locate\b|color_enabled|build_registry|build_engine' crates/tagteam/src/tui_cmd.rs crates/tagteam/src/tui_host.rs`. Expected: no hit.
- [ ] **Step 5: Run the tests.** Run `cargo test -p tagteam --locked --features test-support`. Outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty --test signals` (the colour, `NO_COLOR` and unreadable-marker cases). Expected: all pass.
- [ ] **Step 6: Commit.** Run `git add crates/tagteam/src/tui_cmd.rs crates/tagteam/src/app && git commit -m "Build the TUI's engines through engine_for and choose its colour through ColorEnv"`.

### Task 9: Read the daemon's states and events through typed values in the TUI

**Depends on:** Task 1

**Files:**
- Modify: `crates/tagteam-daemon/src/protocol.rs`:
  - the retyped fields (T3-28), and `impl<'de, T: Spelled> serde::Deserialize<'de> for Wire<T>` (a string read through `from_wire`);
  - `collects_for_itself` and `waits_for_upgrade` read the fields;
  - `typed_event`'s `"mode"` arm builds the `Wire`s;
  - T3-29's additions beside R3's parsers;
  - test literals that build these types.
- Modify: `crates/tagteam/src/service_cmd.rs`: `.as_str()` where it reads `p.mode`, `p.waiting_for` and `o.kind`, in `status_json`'s `json!` and in the human line. Its test literals (`:441-443`) use `Wire::from_wire`.
- Modify: `crates/tagteam-daemon/tests/client.rs:109`: `status.providers[0].mode.as_str()`; `:404-407`'s `Event::Mode` literal uses `Wire`s. `crates/tagteam/tests/daemon_procs.rs:357`'s `p.waiting_for.as_deref()` becomes `p.waiting_for.as_ref().map(Wire::as_str)`. Any other read of the four fields that no longer compiles changes the same way (`rg -n '\.waiting_for|\.mode\b|last_outcome' crates --glob '*.rs'`).
- Modify, in the TUI:
  - `src/daemon/mod.rs`:
    - `DaemonState.polls` holds `PollPayload`s;
    - `note` reads `event.poll()`, `event.no_switch()` (`reason.known()`), `event.all_exhausted()`, `event.error()`, `event.config_warning()` and `event.at()`;
    - `used_pct` is `binding`'s (T3-29);
    - `VIEW_KINDS` gives way to `event.changes_an_account()`;
    - `keeps_index` uses `job.waits_for_upgrade()`, and `collected_elsewhere` `job.collects_for_itself()`;
    - `on_event`'s `Event::TokenIndex` arm passes `event.token_index()`'s payload, not the raw object.
  - `src/screens/tokens/index.rs`: `on_token_index(state, provider, index: &TokenIndexPayload, now_ms)` reads `index.changed_at` in place of `raw["changedAt"]`. Its timing does not change: `a_token_index_event_newer_than_the_report_on_screen_reads_it_again` (`:1352`), the catch-up stream test after it (`:1385`) and the other users of the `token_index` helper (`:1247`) pass unchanged, still building their events from JSON.
  - Both files lose `serde_json::Value` from their production imports.
  - `src/daemon/words.rs`: `job_state` matches `(job.mode.known(), job.waiting_for.as_ref().and_then(Wire::known))` on `Mode` and `WaitingFor`; an unknown mode keeps `daemon {job.mode.as_str()}`.
  - `src/autoline.rs`: `j.mode.known() == Some(Mode::Auto)`, and `outcome` matches `o.kind.known()` on `OutcomeKind::{NoSwitch, Error}`.
  - The literals in `testutil.rs` (`daemon_job`, `daemon_status`), `tests/live.rs`, and the runtime, view and chrome tests: `Wire::from_wire(..)`.
- Test: `protocol.rs`, `src/daemon/mod.rs`, `src/screens/tokens/index.rs` (unchanged).

**Interfaces:**
- Produces, in `tagteam_daemon::protocol` (T3-28, T3-29):
  ```rust
  pub struct StatusProvider { /* … */ pub mode: Wire<Mode>, pub waiting_for: Option<Wire<WaitingFor>>, /* … */ }
  pub struct StatusOutcome { pub kind: Wire<OutcomeKind>, pub reason: Option<String>, pub at: String }
  // Event::Mode { provider, mode: Wire<Mode>, waiting_for: Option<Wire<WaitingFor>>, holder_pid, backoff }
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct AllExhaustedPayload { pub earliest_reset_at: Option<String> }
  /// The `token-index` fields the TUI reads (token report §12): when the index last changed.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct TokenIndexPayload { pub changed_at: Option<String> }
  impl Event {
      /// `poll`, `switch`, `account-quarantined` or `account-unquarantined`: an account's
      /// reading or state changed (D5.5).
      pub fn changes_an_account(&self) -> bool;
      pub fn all_exhausted(&self) -> Option<AllExhaustedPayload>;
      /// An `error`'s `message`; a missing one reads as empty.
      pub fn error(&self) -> Option<String>;
      /// A `config-warning`'s `message`; a missing one reads as empty.
      pub fn config_warning(&self) -> Option<String>;
      /// An auto event's envelope `ts`.
      pub fn at(&self) -> Option<&str>;
      /// `Some` only for `Event::TokenIndex`; a null or absent `changedAt` is `None`.
      pub fn token_index(&self) -> Option<TokenIndexPayload>;
  }
  ```
  `StatusProvider::{mode, waiting_for}` and `StatusOutcome::kind` (the methods) are deleted; `Event::{mode, waiting_for}` stay and clone the field.

- [ ] **Step 1: Write the tests:**
  - **`protocol.rs`, `a_status_reads_its_typed_fields_as_the_daemon_wrote_them`:**
    - `status_json`'s own output, deserialized as `Status`, has `mode == Wire::Known(Mode::Auto)`, `waiting_for == Some(Wire::Known(WaitingFor::EngineLock))` for a waiting job, and `last_outcome.kind == Wire::Known(OutcomeKind::NoSwitch)`;
    - a hand-written status with mode `paused` gives `Wire::Other("paused")`, and `collects_for_itself()` is false.
  - **`protocol.rs`, `the_kinds_the_tui_reads_parse_from_event_json_s_own_output`:**
    - over `event_json` of `auto_lines()`' events, `all_exhausted()` gives its `earliestResetAt`;
    - `error()` and `config_warning()` give their message;
    - `at()` gives the envelope's `ts`;
    - `changes_an_account()` holds for exactly the four kinds;
    - a kind's object without its field reads as empty or `None`, never `Event::Unknown`;
    - `daemon_event_json` of a `TokenIndex` gives `token_index()`'s `changed_at` as its `changedAt`, and `None` for an index that never changed (`"changedAt": null`) or for an object without the key.
  - **`daemon/mod.rs`, `a_poll_without_a_binding_window_leaves_the_sentence_without_its_percentage`:** a `poll` without `windowsPct`, then a `below-threshold`, gives `no switch needed (switches at 90%)`.

    T2's auto-line, header and live-mode tests pass unchanged, `○ daemon added-later` among them.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-daemon --locked --features test-hooks --lib protocol::`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Check no raw key or mode literal is left in the TUI.** Run `rg -n 'raw\[|\.get\("|as_object\(\)|serde_json::Value|"changedAt"|\.mode == |\.mode\.as_str\(\) ==|"engine-lock"|"schema"|VIEW_KINDS|"no-switch" =>' crates/tagteam-tui/src`. Expected: hits only in test modules and `testutil.rs`. (At `9af7450` the production reads were `daemon/mod.rs`'s `note` and `used_pct` and `screens/tokens/index.rs`'s `on_token_index`; this task covers each.)
- [ ] **Step 5: Run the tests.** Run `cargo test -p tagteam-daemon --locked --features test-hooks`, `cargo test -p tagteam-tui --locked` and `cargo test -p tagteam --locked --features test-support`. Outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty`. Expected: all pass, `daemon status --json`'s tests unchanged.
- [ ] **Step 6: Commit.** Run `git add -A crates/tagteam-daemon crates/tagteam-tui crates/tagteam/src/service_cmd.rs crates/tagteam/tests && git commit -m "Read the daemon's states and events through typed values in the TUI"`.

### Task 10: Draw the lead bar, the line gauge and the sparkline

**Depends on:** none

**Files:**
- Create: `src/gauge.rs`.
- Modify: `src/lib.rs` (`mod gauge;`), `src/theme.rs` (`colourless`).
- Test: `src/gauge.rs`.

**Interfaces:**
- Produces:
  ```rust
  // theme.rs
  impl Theme { /// Tier 0 (§4.2): parts that colour would tell apart draw another glyph.
      pub(crate) fn colourless(self) -> bool; }
  // gauge.rs (T3-17, T3-18)
  /// The fill's role for `pct` (T3-18): accent, warn from 70, crit from 90.
  pub(crate) fn fill_role(pct: f64) -> Role;
  pub(crate) fn line_gauge(pct: f64, expected: Option<f64>, cells: usize, theme: Theme) -> Vec<Span<'static>>;
  pub(crate) fn lead_bar(pct: f64, expected: Option<f64>, cells: usize, theme: Theme) -> Vec<Span<'static>>;
  pub(crate) fn sparkline(samples: &[Sample], from_s: i64, to_s: i64, cells: usize, theme: Theme) -> Span<'static>;
  ```
  `expected` is `Pace.expected_pct`; only long and scoped windows have one (C-8).

- [ ] **Step 1: Write the tests** (text by `Line::from(spans).to_string()`, roles by `roles` over a one-row buffer, for `TIERS`):
  - `a_line_gauge_fills_whole_and_half_cells_on_a_track`:
    - `(54, None, 9)` is `━━━━╸────`;
    - `(0, None, 9)` is `─────────`, and `·········` in tier 0;
    - `(100, None, 9)` and `(150, None, 9)` are `━━━━━━━━━`;
    - `(f64::NAN, None, 9)` draws as 0.

    The fill is in `fill_role`'s role (accent for 54, warn for 75, crit for 95) and the track in faint.
  - `a_line_gauge_s_pace_mark_is_inside_or_outside_the_fill`:
    - `(54, Some(62), 9)` is `━━━━╸┼───`, its `┼` muted;
    - `(31, Some(24), 14)` is `━━━┿──────────`, its `┿` in the fill's role;
    - `(6, Some(10), 14)` is `╸┼────────────`.
  - `a_lead_bar_fills_on_a_baseline_with_its_pace_mark`:
    - `(53, None, 52)` has 28 `▆` then 24 `▁`, the `▁` faint;
    - `(6, Some(10), 52)` is `▆▆▆▁▁│` followed by `▁`s, its `│` muted;
    - `(53, Some(30), 52)` has its `│` at cell 15, in text.
  - `a_sparkline_shows_each_bucket_s_peak_and_leaves_an_empty_one_blank`: samples at 10 s (0 %), 110 s (50 %), 120 s (100 %) and 390 s (20 %) over `[0, 400)` in 4 cells give `▁█ ▂`, in accent. A sample outside the range is ignored, and no sample gives four spaces.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked gauge::`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-tui --locked`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add crates/tagteam-tui/src && git commit -m "Draw the lead bar, the line gauge and the sparkline"`.

### Task 11: Rebuild the Accounts table around markers, gauges, UPDATED and STATUS

**Depends on:** Tasks 2, 3, 6, 10

**Files:**
- Create: `src/screens/accounts/table.rs`: the table (T3-15, T3-16), replacing `accounts.rs`'s `Table`, `Cell`, `gauge`, `age_now` and their constants.
- Create: `src/screens/accounts/status.rs`: the STATUS phrase and the no-reading words (T3-11, T3-14).
- Modify: `src/screens/accounts.rs`:
  - `mod status; mod table;`;
  - `render`'s table part draws `table::Table`;
  - a disabled row's `Line` takes `theme.dim()` before `select`;
  - `TableLine::Provider` draws the provider's display name, muted.

  The detail pane (`detail`, `window_blocks`, `split`) is not edited here.
- Modify: `src/view.rs`: `tint` keeps the row's own `Line` style, patching `sel_bg` over it.
- Modify: `src/chrome.rs`: `fit_spans` becomes `pub(crate)` (visibility only).
- Rewrite tests to the new layout, in `accounts.rs`, with T3-27's fixture: `the_table_says_what_list_says_at_80_columns`, `a_wider_terminal_adds_a_gauge_per_window`, `the_active_marker_session_column_and_this_follow_list`, `percentages_and_gauges_take_their_severity_colour_or_bold_without`, `the_selected_row_has_the_bar_and_a_bold_name_and_no_reverse_video`, `a_selected_active_row_bolds_its_name_alone_and_draws_the_marker_in_accent`, `wide_names_are_cut_by_display_width_and_never_overflow`, `several_providers_are_grouped_under_their_names`, `the_api_key_and_disabled_notes_follow_list`, `columns_give_way_to_the_notes_at_80_columns`, `a_note_too_long_for_its_room_ends_in_an_ellipsis`, and the table half of `a_readings_age_advances_with_the_clock_between_reads`. `more_accounts_than_fit_scroll_with_the_selection` and `a_resize_keeps_the_tables_scroll_in_range_and_the_next_key_moves_at_once` keep their assertions over the new rows.

**Interfaces:**
- Consumes: Task 3's `Problem`, `problem`; Task 2's `status_words`, `UPDATED_HEAD`; Task 6's `forecast`; Task 10's `line_gauge`, `fill_role`.
- Produces:
  ```rust
  // status.rs
  /// The STATUS column's phrase for a row, first match (§6.3, T3-14), with its role: `✕ new
  /// login` crit … `api key` muted. `this` is the run shell's account; `best` the forecast's pick.
  pub(crate) fn phrase(v: &AccountView, this: bool, best: bool) -> Option<(String, Role)>;
  /// The phrase's glyph alone, for under 100 columns: `✕`, `!` or `○`.
  pub(crate) fn glyph(v: &AccountView, this: bool, best: bool) -> Option<(&'static str, Role)>;
  /// A row with no reading, across its window columns (§6.3): `✕ needs a new login`,
  /// `api key · has no usage limits to show`, `! can't read the Keychain · retry in 2m`.
  pub(crate) fn unread_words(v: &AccountView, now_s: i64) -> (String, Role);
  // table.rs
  pub(crate) const STATUS_FROM: u16 = 100;
  pub(crate) const EXTRA_FROM: u16 = 160;
  pub(crate) struct Table<'a> { /* columns and widths for one provider's section */ }
  impl<'a> Table<'a> {
      pub(crate) fn new(accounts: &'a [AccountView], forecast: &BestForecast, this: Option<&'a AccountId>, width: usize, read_at_s: i64, now_s: i64) -> Table<'a>;
      pub(crate) fn header(&self, theme: Theme) -> Line<'static>;
      pub(crate) fn row(&self, v: &AccountView, now_s: i64, theme: Theme) -> Line<'static>;
  }
  /// The span `select` bolds from: the name's first span.
  pub(crate) const NAME_SPAN: usize;
  ```
- **The phrases** (§6.3, T3-14): `NeedsLogin` `✕ new login`, `LoginExpired` `✕ expired`, `NoLogin` `✕ no login` (crit); `RateLimited` `! rate-limited`, `Keychain` `! keychain`, `Budget` `! budget`, `OtherAccount` and `Retrying` `! retrying`, `OldReading` `! old reading` (warn); then `○ disabled` (muted), `▶ this session` (muted), `in use` (accent), `best` (text), `api key` (muted).
- **The marker column:** `●` (accent) in use, else `▶` (accent) in a session, else `○` (muted) disabled, else a space.
- **UPDATED** is the reading's age as `words::duration`, advancing with the clock as `age_now` does today, muted; warn for `OldReading`; `—` (faint) without a reading.
- **A no-reading row** puts `unread_words` from the 5H column to the end of the window group, cut by `fit`; its UPDATED is `—`.

- [ ] **Step 1: Write the tests.** The fixture `three()` is:
  - 1 `alex@example.com`: 5h 0 %, 7d 30 %, read 120 s ago;
  - 2 `work` `sam@corp.example` of `Example Corp`: in use, 5h 53 %, 7d 6 %, 60 s;
  - 3 `spare` `alex@personal.example`: 5h 0 %, 7d 31 %, 840 s (old).

  Its forecast is `Pick { [1] }`, and the selection is on row 2. Column positions are display columns (`width_of` of the text before them).
  - **`the_table_at_120_columns_has_14_cell_gauges_and_a_status_column`.** At 120×40, the head row's `ACCOUNT`, `5H`, `7D`, `UPDATED` and `STATUS` are at columns 8, 39, 62, 86 and 95, all muted. Row 2 is `▌   2 ● work   sam@corp.example` + 8 spaces + `━━━━━━━───────  53%` + 4 spaces + `╸─────────────   6%` + 10 spaces + `1m  in use`. Row 1 ends `2m  best`, and row 3 ends `14m  ! old reading`, its `14m` and phrase in warn. No org shows (T3-15: `Example Corp` does not fit every row).
  - **`the_table_at_80_columns_has_9_cell_gauges_and_a_status_glyph`.** At 80×24, the name column is cut to 23: row 3's name is `spare  alex@personal.e…` (`fit` keeps 22 columns and the `…`). The gauges are 9 cells, so row 1's 7D is `━━╸──────  30%`; row 3 ends `14m !`, and rows 1 and 2 end with their age.
  - **`status_is_the_first_phrase_that_applies_with_a_quarantined_account_in_use`** (§16): the account in use quarantined reads `✕ new login` (crit) with `●` in its marker column. A disabled quarantined one reads `✕ new login`, and a disabled healthy one `○ disabled`.
  - **`a_token_from_a_newer_build_reads_as_retrying`** (Review Focus 1): `Unavailable` with `a-token-from-a-newer-build` and no reading reads `! usage check failed (a-token-from-a-newer-build)` across its windows and `! retrying` in STATUS.
  - **`a_row_without_a_reading_says_its_status_across_the_windows`:** `✕ needs a new login` (crit), `api key · has no usage limits to show` (muted), `! can't read the Keychain · retry in 2m` (warn), `not checked yet` (muted); each cut by `fit` to the window group at 80 columns.
  - **`the_marker_column_shows_in_use_in_a_session_and_disabled`,** and in a run shell `▶ this session` in STATUS.
  - **`percentages_and_fills_take_their_severity_role_in_every_tier`,** for `TIERS`:
    - 53 % is text with an accent fill;
    - 75 % is warn with a warn fill;
    - 95 % is crit with a crit fill;
    - `▲` is warn;
    - tracks are faint.
  - **`only_the_selected_row_s_name_is_bold`** (T3-16):
    - in tiers 1 and 2, the selected row's only bold cells are its alias's (or, without one, its email's), and the active unselected row has none;
    - in tier 0 the bold cells are exactly `▌`, `●` and the name.
  - **`a_disabled_row_is_dim_even_when_selected`:** every cell of a disabled row has `Modifier::DIM`, selected or not. In tier 2 the selected one also has `sel-bg`. `view.rs` gains `select_keeps_the_row_s_own_style`.
  - **`from_160_columns_scoped_windows_and_spend_get_columns`:** with a `Fable` scoped window and a spend window, at 160 columns the heads `FABLE` and `SPEND` show, with `€0 of €20`; at 120 they do not.
  - **`a_narrow_table_shrinks_the_name_then_the_gauges`** (T3-15's order, below the TUI's 80 columns, so `Table::new` is called directly): over `three()`, at width 72 the name is 16 and the 7D gauge 6 (5H 9); at 60 the 5H gauge is 6 and 7D has none. Each row fits its width, and the percentages always show.
  - **`wide_names_are_cut_by_display_width_and_never_overflow`** (Review Focus 3): a 200-character alias and a CJK email at 80 columns. Every row is at most 80 columns wide, UPDATED stays at its column, and the glyph shows.
  - **`several_providers_are_grouped_under_their_display_names`:** headings `Claude Code` and `FakeAgent`, muted, each followed by its own head row.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked screens::accounts`. Expected: failures.
- [ ] **Step 3: Implement** as listed, and rewrite the listed tests.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-tui --locked`; outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui/src && git commit -m "Rebuild the Accounts table around markers, gauges, UPDATED and STATUS"`.

### Task 12: Show the selected account's card with sparklines, problems and remedies

**Depends on:** Task 11

**Files:**
- Create: `src/screens/accounts/card.rs`: the card (T3-20), and the problem rows the lead card reuses.
- Modify: `src/screens/accounts.rs`:
  - `detail`, `window_blocks`, `window_lines`, `window_heights` and `Shown` give way to `card`;
  - `split`'s pane holds the card, sized by `height`, and never less than `min_height` (in place of `DETAIL_MIN`), so both are used from this task on;
  - `scroll_detail` scrolls the card's windows;
  - `AccountsState` gains the card's view and when its read is due;
  - add `on_tick` and the card's half of `apply` (T3-21).
- Modify: `src/msg.rs` (`Cmd::LoadCard`, `Msg::Card`, `ViewsReq::Card` coalescing like `History`), `src/lanes.rs` (the views lane serves `Card` through `source.history`), `src/runtime.rs` (`execute` submits it), `src/state.rs` (`Gens.card`, `State::load_card`), `src/update.rs` (`Msg::Card`; `on_tick` calls `screens::accounts::on_tick`).
- Rewrite tests to the new layout, in `accounts.rs`:
  - `the_detail_pane_shows_the_selected_account`, `a_row_without_a_reading_says_why_in_the_detail_pane`, `the_detail_pane_counts_down_to_the_next_poll_while_it_is_ahead`, `the_detail_pane_announces_no_poll_the_status_rules_out`, `the_rule_cuts_the_name_before_the_facts`, `the_detail_pane_grows_to_show_every_window`, `windows_beyond_the_pane_scroll_into_view_with_shift_j_and_k`, `a_resize_clamps_the_detail_scroll_so_the_next_key_moves_at_once`, and the card half of `a_readings_age_advances_with_the_clock_between_reads`;
  - R20's long-label tests (`a_window_too_wide_for_80_columns_wraps_its_parts_under_its_value`, `an_oversized_label_goes_above_its_value_and_the_column_stays_capped`, `a_1000_character_label_leaves_its_window_reachable_at_80x24_under_a_full_banner`, `the_worst_live_stack_at_80_by_24_still_shows_a_long_labelled_window_whole`, `a_window_takes_seven_rows_at_most_whatever_its_words`) become the card's guarantee: a label cut to its column, one row per window, and every window reachable by `J`/`K` at 80×24 under a full banner.

  `keys.rs`'s `every_key_the_table_lists_is_handled` builds its `J`/`K` state with a card that overflows.
- Modify, mechanically: a test elsewhere that asserts a tick's exact commands on Accounts with an account selected now meets the card's `LoadCard` 250 ms after the selection. It ticks before the read is due, or asserts the `LoadCard` too; it never drops an assertion.

**Interfaces:**
- Consumes: Task 3's sentences and remedies, Task 10's `sparkline`, `autoline::when` for the quarantine's time.
- Produces:
  ```rust
  // msg.rs
  Cmd::LoadCard { r#gen: u64, account: AccountId, since_s: i64 },
  Msg::Card { r#gen: u64, account: AccountId, result: Result<HistoryView, SourceError> },
  // state.rs
  impl State { pub(crate) fn load_card(&mut self, account: AccountId, since_s: i64) -> Cmd; }
  // card.rs
  pub(crate) const CARD_DEBOUNCE_MS: i64 = 250;
  /// The card's lines at `width`, its windows from `from` on, in at most `rows` (T3-20, T3-22).
  /// `on_lead` names the windows the lead card shows for the account in use (T3-32): those
  /// rows leave out their reset and projection. It is empty for any other account.
  pub(crate) fn lines(state: &State, v: &AccountView, width: u16, rows: usize, from: usize, on_lead: &[String], now_ms: i64) -> Vec<Line<'static>>;
  /// §6.2's windows for the lead card at `width`, before any fold, by key: the first short and
  /// the first long below 120 columns, every window from 120. Until Task 13 draws the lead
  /// card, `render` passes these as `on_lead`; Task 13 picks its bars from them.
  pub(crate) fn lead_keys(v: &AccountView, width: u16) -> Vec<String>;
  /// The card's lines at `width` with every window: what the layout gives it when it fits.
  pub(crate) fn height(state: &State, v: &AccountView, width: u16, now_ms: i64) -> usize;
  /// The fewest lines the card is drawn in (T3-22): its title, one window row when it has
  /// windows, its problem rows and its keys row.
  pub(crate) fn min_height(state: &State, v: &AccountView, width: u16, now_ms: i64) -> usize;
  /// A problem and its remedy as rows (§6.2, §6.4): `✕ Needs a new login: …` in its role, the
  /// remedy two columns further in, in text.
  pub(crate) fn problem_rows(state: &State, v: &AccountView, width: u16, now_ms: i64) -> Vec<Line<'static>>;
  ```
- **The read** is `since_s = now − 7 d`. Short windows draw their sparkline over the last 24 hours (tag `24h`), and the rest over 7 days (tag `7d`), the tag faint. The sparkline is 32 cells from 120 columns, else 16.
- **The keys row** (from 120 columns): `    ⏎ history of {alias, else the email's local part}   e {disable|enable}   a alias   x remove`, keys in accent and labels muted.

- [ ] **Step 1: Write the tests:**
  - **`the_selected_card_shows_its_facts_sparklines_and_windows`.** Spare (row 3, `next_poll_at` 2m ahead) is selected at 120×40, with a card view whose samples cover 7 days.
    - The title is `    3 spare  alex@personal.example    not in use · updated 14m ago · next update in 2m`, its `updated 14m ago` in warn (an old reading).
    - The 5h row starts `    5h    `, then 32 sparkline cells (accent), a space and `24h` (faint), two spaces, `  0%`, two spaces and `resets in 2h40m`.
    - The 7d row has `7d ` as its tag and ends ` 31%  resets in 3d09h`; there is no projection without a rate.
    - Then the problem rows: `! Old reading: too old to pick the best account by.` in warn, and `tagteam checks it within 2m.` in text, two columns further in.
    - The keys row reads `    ⏎ history of spare   e disable   a alias   x remove`.
  - **`the_card_of_the_account_in_use_leaves_out_its_reset_and_projection`:** row 2's window rows end at their percentage (and `▲`), its `on_lead` being `lead_keys`'.
  - **`a_window_the_lead_card_does_not_show_keeps_its_reset_on_the_in_use_card`** (T3-32): at 80 columns, the account in use with a `Fable` window shows `resets in …` on its `Fable` row and not on its 5h and 7d rows.
  - **`a_problem_shows_its_sentence_and_remedy_under_the_windows`:** a quarantined account (`invalid_grant`, quarantined on Saturday at 09:02) reads `✕ Needs a new login: its stored login was refused (invalid_grant), Sat 09:02.` in crit, then `Log in to Claude Code as alex@personal.example, then press A here.` in text, two columns further in.
  - **`a_window_without_a_reset_says_not_started`,** and at 120 columns `▲ faster than an even pace (62% expected by now)` for an ahead window when the row fits.
  - **`the_card_title_cuts_the_name_before_the_facts`** (Review Focus 3): a 200-character alias at 80 columns keeps `not in use · updated 2m ago` whole.
  - **`a_selection_change_reads_the_card_s_history_once_after_250_ms`:**
    - `j` at `NOW_MS` gives no `LoadCard`, and neither does a tick at `+249`;
    - a tick at `+250` gives one `LoadCard` for the new account with `since_s == NOW_S − 7·86400`, and the next tick none;
    - an accounts read where that account's `fetched_at` moved gives one more, 250 ms later.
  - **`a_card_read_for_another_account_or_an_older_generation_is_dropped`** (Review Focus 4): a `Msg::Card` with the previous generation, or for an account no longer selected, changes nothing drawn. A failed read keeps the last view and raises no banner.
  - **`every_window_is_reachable_by_shift_j_and_k_at_80x24_under_a_full_banner`:** with seven windows and a three-row banner, `J` pressed until nothing moves shows the last window. A resize clamps the scroll, and the next `K` moves at once.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked screens::accounts`. Expected: a compile error.
- [ ] **Step 3: Implement** as listed, and rewrite the listed tests.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-tui --locked`; outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty` and `cargo test --release -p tagteam --features test-support --test perf -- --ignored tui_first_frame --nocapture`. Expected: all pass; every p95 within 50 ms.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui/src && git commit -m "Show the selected account's card with sparklines, problems and remedies"`.

### Task 13: Lead Accounts with the account in use and the best pick

**Depends on:** Task 12

**Files:**
- Create: `src/screens/accounts/lead.rs`: the lead card (T3-19), its fold (T3-32) and the forecast's words.
- Modify: `src/screens/accounts.rs`:
  - `split` becomes the §6.1 layout (T3-22), with `visible_rows` and `window_rows` following it;
  - `AccountsState::apply` starts the selection on the account in use (T3-23);
  - `render` draws the lead card, the table and the card in their areas, and passes the lead card's `shown` as the card's `on_lead` when the selected account is the one in use (in place of Task 12's `lead_keys`).
- Rewrite tests to the new layout: `the_selection_follows_the_account_across_a_refresh` (its first selection is now the account in use), and `no_accounts_shows_the_empty_state` (still the empty block, with no lead card).

**Interfaces:**
- Consumes: Task 12's `problem_rows`, `lead_keys`, `height` and `min_height`; Task 10's `lead_bar`; Task 6's `forecast` and `live`.
- Produces:
  ```rust
  // lead.rs
  /// The lead card as drawn, and the windows whose bars it drew, by key (T3-32).
  pub(crate) struct Lead { pub(crate) lines: Vec<Line<'static>>, pub(crate) shown: Vec<String> }
  /// The lead card at `width` in at most `rows` rows (§6.2, T3-19, T3-32), for focus's
  /// provider. The title and the problem rows are counted first; the bars of
  /// `card::lead_keys` take the rows left, the last of them becoming the fold line when one is
  /// left out. Drawn in §6.2's order: the title, the bars and the fold line, then the problem
  /// rows under them.
  pub(crate) fn lead(state: &State, width: u16, rows: usize, now_ms: i64) -> Lead;
  /// Its rows with no bound: what T3-22 gives it when that fits.
  pub(crate) fn height(state: &State, width: u16, now_ms: i64) -> usize;
  /// The forecast's words: the lead card's when `with_key` (§6.2's table), the Now strip's
  /// otherwise (§5.4: no key, and nothing for any `None`).
  pub(crate) fn pick_words(state: &State, view: &ProviderAccounts, with_key: bool) -> Option<Vec<Span<'static>>>;
  // accounts.rs
  struct Parts { lead: Rect, table: Rect, card: Rect }
  fn parts(state: &State, body: Rect, now_ms: i64) -> Parts;
  ```
- **The forecast's words** (§6.2), with `with_key`: `Pick` is `best now  {pos} {alias, else email}   b switch` (`best now` muted, the name text, `b` accent, `switch` muted); `AlreadyBest` is `on the best account` (muted); `None(UsageUnknown)` is `b checks usage first, then picks` (`b` accent, the rest muted); any other `None` is nothing. Without `with_key` (§5.4): `best now {pos} {alias, else email}`, `on the best account`, or nothing.
- **No account in use** (focus has none): the lines are `  No account in use` (bold) and `  s switches to the selected account` (muted).

- [ ] **Step 1: Write the tests,** over `three()`:
  - **`the_lead_card_shows_the_account_in_use_with_its_bars_and_the_best_pick`:**
    - at 120×40, row 0 starts `  ● 2 work  sam@corp.example · Example Corp    in use` and ends `best now  1 alex@example.com   b switch` at the right edge;
    - rows 1 and 2 are `    5h    ` and `    7d    `, each bar 52 cells from column 10, then two spaces, the percentage in four columns (` 53%`, `  6%`), two spaces and `resets in 2h40m` or `resets in 3d09h`;
    - at 80×24 the bars are 30 cells, with only 5h and 7d, and the title has no org.

    `●` and `b` are accent, the alias bold, the bars' fills accent, the baselines faint.
  - **`each_forecast_has_its_words_on_the_lead_card`:** `Pick`, `AlreadyBest`, `None(UsageUnknown)`, and `None(TooFewCandidates)`, which shows nothing.
  - **`a_problem_with_the_account_in_use_shows_under_its_bars`:** row 2 quarantined gives its problem rows under the bars, and `✕ new login` in the table's STATUS.
  - **`a_login_close_to_expiry_is_warn_and_says_to_log_in_again`:** at `+6d03h` the facts read `in use · login valid 6 more days · log in again before then`, the last two in warn. At `+27d` it is `login valid 27 more days`, muted.
  - **`from_120_columns_the_lead_card_shows_every_window_and_spend_as_amounts`:** a `Fable` window and a spend window add two rows, the spend row reading `€12.40 of €50` where a percentage would be.
  - **`with_no_account_in_use_the_lead_card_says_so`.**
  - **`with_several_providers_the_lead_card_follows_the_selected_account_s_provider`** (Review Focus 2): selecting a FakeAgent account with none in use gives `No account in use`, although Claude Code has one.
  - **`a_long_name_on_the_lead_card_keeps_the_best_pick`** (Review Focus 3).
  - **`the_body_stacks_lead_table_and_card_from_the_top_and_leaves_spare_rows_at_the_bottom`** (VIS-5): at 80×24 the lead card's three rows, a blank, the table's four, a blank, then the card, and the body's last rows are empty.
  - **`with_sixteen_accounts_at_80_by_24_the_table_scrolls_between_the_lead_and_the_card`:** over `many(16)` with row 1 in use, the table shows fewer than 16 rows; `End` selects row 16, which shows, and the lead card's and the selected card's titles keep their rows.
  - **`twenty_windows_on_the_account_in_use_fold_the_lead_card_and_keep_the_table_and_the_card`** (T3-22, T3-32). `three()`'s row 2, in use and selected, has 20 decision-grade windows: 5h, 7d and 18 scoped ones, `m1` to `m18`, none with a problem. The body is 21 rows.
    - **At 120×24.** The lead card is rows 0–11: its title, ten bars (5h, 7d, `m1`–`m8`), then `+10 more windows · in the card below` (muted) in place of an eleventh. Row 12 is blank, rows 13–16 are the table's head and three rows, row 17 is blank, and rows 18–20 are the card's title, one window row and its keys row.
      - The card's 5h row has no `resets in`.
      - Ten `J`s bring `m9` into the card's window row, and it has its `resets in …`.
      - `J` pressed until nothing moves shows `m18`, and the table's rows never move.
    - **At 80×24.** The lead card is its three rows, with no fold line (§6.2's 80-column rule). The table keeps its four rows, and the card gets the remaining twelve: its title and eleven window rows. Its 5h and 7d rows have no `resets in`, and its `m1` row has one. `J` reaches `m18`.
    - With row 1 selected instead, the fold line at 120×24 reads `+10 more windows · select it for its card`.
  - **`a_folded_lead_card_keeps_its_problem_under_its_bars`** (§6.2, T3-32). The same 20-window row 2, now quarantined (`invalid_grant`), in use and selected, at 120×24. The card's minimum is now its title, one window row, its two problem rows and its keys row: five rows. So the lead card gets ten rows:
    - row 0, its title;
    - rows 1–6, six bars (5h, 7d, `m1`–`m4`);
    - row 7, `+14 more windows · in the card below`;
    - rows 8–9, `✕ Needs a new login: …` (crit) and its remedy.

    Then row 10 is blank, rows 11–14 are the table's head and three rows (row 2's STATUS `✕ new login`), row 15 is blank, and rows 16–20 are the card's five. With the problem and two windows only (`three()`'s row 2 quarantined) at 120×40, the title, both bars, then the problem rows, in that order, as `a_problem_with_the_account_in_use_shows_under_its_bars` has it.
  - **`selection_starts_on_the_account_in_use`,** else on row 1 (UX-2).
  - **`the_accounts_screen_s_roles_in_each_tier`** (§16): at 80×24 and 120×40, for `TIERS`. The lead card's, the table's and the card's rows have the roles named above, and the selected row has `sel-bg` in tier 2.
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked screens::accounts`. Expected: failures.
- [ ] **Step 3: Implement** as listed, and rewrite the listed tests.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-tui --locked`; outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui/src && git commit -m "Lead Accounts with the account in use and the best pick"`.

### Task 14: Show the account in use on Tokens and History

**Depends on:** Task 13

**Files:**
- Modify: `src/chrome.rs`: add `now_strip` (T3-24).
- Modify: `src/view.rs`: `Areas` gains `now: Option<Rect>`, a row under the auto line on Tokens and History, and `view` draws it there.
- Modify, per Global Constraints: the Tokens and History tests whose body is now a row shorter, in `screens/tokens/{table,filter,mod}.rs`, `screens/history.rs` and `update.rs`. Each changes its fixture (a row or an account fewer), never its assertion.

**Interfaces:**
- Consumes: Task 13's `pick_words(.., false)`, Task 11's problem glyph, Task 2's `status_words`, Task 10's `line_gauge`.
- Produces:
  ```rust
  // chrome.rs
  /// §5.4's Now strip for focus's provider (T3-24): on Tokens and History only.
  pub(crate) fn now_strip(state: &State, width: u16, now_ms: i64) -> Line<'static>;
  // view.rs
  pub(crate) struct Areas { /* … */ pub(crate) now: Option<Rect>, /* … */ }
  ```

- [ ] **Step 1: Write the tests** in `chrome.rs`:
  - **`the_now_strip_shows_the_account_in_use_and_the_best_pick`.** On Tokens at 120×40, row 2 starts ` ● 2 work   5h ` then a 10-cell gauge, `  53% 2h40m   7d `, a gauge and `   6% 3d09h`, and ends `best now 1 alex@example.com`. At 80×24 it is ` ● 2 work   5h  53% 2h40m   7d   6% 3d09h`, with no gauge. `●` is accent, `work` bold, the percentages text, the spans muted.
  - **`a_problem_replaces_the_now_strip_s_windows`:** row 2 quarantined gives ` ● 2 work   ✕ needs a new login · see Accounts`, `✕ needs a new login` in crit.
  - **`with_no_account_in_use_the_now_strip_says_how_to_switch_to_one`:** `no account in use · s on Accounts switches to one`, muted. Before the first snapshot the row is empty.
  - **`the_now_strip_follows_the_selected_account_s_provider`** (Review Focus 2).
  - **`the_rows_above_the_body_on_tokens_and_history_are_three_and_on_accounts_two`:** at 80×24 the body starts at row 3 on Tokens and History and at row 2 on Accounts. On History it is 20 rows high (§7.8), and the same in live mode, in timer mode and after `Live::Down` (UX-5).
- [ ] **Step 2: Run them to see them fail.** Run `cargo test -p tagteam-tui --locked chrome::`. Expected: failures.
- [ ] **Step 3: Implement** as listed, and change the listed fixtures.
- [ ] **Step 4: Run the tests.** Run `cargo test -p tagteam-tui --locked`; outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty`. Expected: all pass.
- [ ] **Step 5: Commit.** Run `git add -A crates/tagteam-tui/src && git commit -m "Show the account in use on Tokens and History"`.

### Task 15: Record the Accounts redesign and the plain words in the specs and the changelog

**Depends on:** Tasks 1–14

**Files:**
- Modify: `CHANGELOG.md`, under `## [Unreleased]` `### Changed`:
  - `` `list`, `status` and `history` say usage states in plain words (`needs a new login`, `rate-limited (retry 5m)`, `not checked yet`, `▲ fast`). `list`'s `AGE` column is `UPDATED`. `history` gives burn rates in `%/h` and says when a window is full, without the projection method, which `--json` still has. ``
  - `` `tagteam auto` and `tagteam daemon` say auto's decisions in plain words, such as `no switch needed (77%, switches at 90%)`. ``
  - `` `tagteam tui`'s Accounts screen leads with the account in use, its bars with pace marks and the best account to switch to. Its table has a STATUS column, and the selected account's card shows sparklines, what is wrong and how to fix it. ``
  - `` `tagteam tui` shows the account in use on Tokens and History, and `A` adds the login of the provider the screen follows. ``
- Modify: `docs/superpowers/specs/2026-10-14-tagteam-tui-design.md`, each hunk ending *(Amended by the TUI redesign spec §n.)*:
  - **§5.1:** the chrome rows include the Now strip on Tokens and History (§5.1, §5.4); drop T2's "(the Now strip on Tokens and History comes with T3)".
  - **§5.2:** the mock-up and its bullets give way to the lead card, the table and the card (redesign §6.1–§6.5). "Same words as `list`" means words, not columns (M-3, §11.1).
  - **§5.4:** the per-window bullet says the burn rate in `%/h` and when the window is full (§11.1). The rest is T4's.
- Modify: `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`, each hunk ending *(Amended by the TUI redesign spec §n.)*:
  - **§11.4 "Human output":** the outcome in plain words, as §11.3's sentences and trigger words.
  - **§13.1:** `list`'s layout block and bullets use `UPDATED`, `▲ fast` and §11.1's status words.
  - **§13.4:** the burn rate in `%/h`, and when each window is full or whether it lasts until it resets; the projection method in `--json` only.

- [ ] **Step 1: Edit** the three files as listed.
- [ ] **Step 2: Check the format.** Run `cargo fmt --all --check`. Expected: clean.
- [ ] **Step 3: Commit.** Run `git add CHANGELOG.md docs/superpowers/specs/2026-10-14-tagteam-tui-design.md docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md && git commit -m "Record the Accounts redesign and the plain words in the specs and the changelog"`.

---

## After Task 15 (controller)

- [ ] **The gallery check** (above), into `gallery/t3-after-15`.
- [ ] **No dependency changed.** Run `git diff <base> -- Cargo.toml Cargo.lock 'crates/*/Cargo.toml'`. Expected: nothing.
- [ ] **Only the listed files changed.** Run `git diff --stat <base>`. Expected: only paths in Global Constraints' list.
- [ ] **No provider name in neutral source.** Run `git diff <base> -- crates/tagteam-tui/src crates/tagteam-engine/src crates/tagteam-daemon/src | rg '^\+' | rg -i 'claude|anthropic|tagteam_cc'`. Expected: only lines in test modules or `testutil.rs`.
- [ ] **No commit over the slice size.** For each commit, `git show --format= <sha> | wc -c` is under about 90 KB, or it was split.
- [ ] **CI, in full.** Run the four CI commands. Expected: all clean, with totals reported as passed/total.
- [ ] **The pty suites.** Outside the sandbox, run `cargo test -p tagteam --locked --features test-support --test tui_pty --test signals`.
- [ ] **The first frame.** Outside the sandbox, on an idle machine, run `cargo test --release -p tagteam --features test-support --test perf -- --ignored tui_first_frame --nocapture`. Expected: every p95 within 50 ms.
- [ ] **Linux.** Run the full suite in Docker: non-root with a passwd entry, HOME outside `/tmp`, and the `docker` call unpiped.
- [ ] **Cross-review.**
  - `xreview round --reset`.
  - A pre-merge review of `origin/main...<T3 branch>`. The PR gate opens only on a full-range approval.
  - If the diff exceeds xreview's inline cap, the full range needs `XREVIEW_MAX_DIFF_BYTES` raised, which is Michael's call each time. Until he makes it, review in slices of commit ranges, each a `--diff` range under the cap: Tasks 1–4 (words), 5–6 (engine), 7–9 (R3), 10–11, 12, 13–15, then any fix commits. A slice over the cap splits by commit.
  - Fix and re-review until it approves.
- [ ] **The Draft PR.**
  - Push, then open a Draft PR against `main` that fills `.github/pull_request_template.md`. Its "Rulings" section lists T3-1 to T3-32, plus any ruling made while executing or in review.
  - In one commit, set this plan's status to `Implemented — <PR URL>`; the redesign spec stays `In progress`.
  - Re-review that commit, then push it.
