# tagteam — sub-project 3: TUI

**Status:** Implemented — https://github.com/michaelrigart/tagteam/pull/18
**Date:** 2026-10-14
**Scope:** `tagteam tui`: a terminal UI over the existing engine. It is the main place to read
the token report, and it shows accounts with their limit windows and pace, and usage history.
Its actions run the CLI's own commands.
**Builds on:**
- **Core spec:** [`2026-09-26-tagteam-core-cli-design.md`](2026-09-26-tagteam-core-cli-design.md)
  (Implemented). It is the authority for everything that already exists; this spec cites its
  sections as "core §n".
- **Token report spec:** `2026-10-14-tagteam-token-report-design.md` (Approved, commit
  `e6cc92a` on branch `token-report`); "TR §n" cites it. The report screen renders its engine
  view (TR §10, §9 here).
- **Daemon spec:** `2026-10-14-tagteam-daemon-design.md` (Approved, commit `96b9cea` on branch
  `daemon`); "D*n*" cites it. Live updates use its socket protocol (D5, §8), and the
  auto-switch pause uses its `daemon.autoswitch` setting (D7, §7.4).

**Merge order:** the token report, then the daemon, then this branch. This branch wires up both
links when it rebases. Until then it tests them against fixtures.

---

## 1. Purpose

`tagteam tui` puts on one screen what today takes `list`, `history` and the token report's CLI
command. It stays current without a person re-running anything, and it acts on accounts through
the same commands the CLI runs.

### 1.1 Success criteria

1. **The token report is first-class.** Tokens and cost by day, month, session and 5-hour block,
   grouped or filtered by account and model, with drill-down from a total to its sessions and
   from a session to its models.
2. **Start-up feels like `list`.** The first frame showing the accounts table appears within
   50 ms p95 when no usage fetch is due, measured as core §1.1 measures `list`.
3. **The UI thread never waits on I/O** other than the terminal. Store reads, transcript reads,
   the network, the Keychain and the daemon's socket all run on other threads (§4.3).
4. **Usage requests go only through the collector** and its budget (core §8.3, §8.6). The TUI
   never fetches on its own schedule. When a daemon runs, the TUI fetches only when a person
   asks (`r`), or when an action's CLI command does (`switch --strategy best`, for example).
5. **Actions are the CLI's.** Every action runs the CLI's own command path, so its prompts,
   refusals, messages and outcome are identical by construction (§7).
6. **It works without a daemon**, refreshing on a timer, and updates live when one runs (§6).
7. **The terminal is always restored**: on quit, on a signal, on an error and on a panic.
8. **The local-state invariant holds** (core §3). The TUI writes nothing of its own to Claude
   Code's files. Its actions write only what their CLI commands write.

### 1.2 Non-goals

- **CLI-only commands.** `add-token`, `export`, `import`, `purge`, `displaced`, `map`,
  `unmap`, `history --csv` and the report's `--json`/`--csv` stay in the CLI. So does `config`,
  apart from the auto-switch pause (§7.4).
- **Mouse support, themes, custom key bindings,** and any `[tui]` settings in `config.toml`.
- **Notifications.** They are the daemon's.
- **Controlling a foreground `tagteam auto`.** Only the daemon's loop can be paused (§7.4).
- **A `watch` command.** `tui` covers cswap's `tui` and `watch` alike (§12.1).
- **Windows,** as in core §1.2.

## 2. Terminology

| Term | Meaning |
|---|---|
| Lane | A worker thread with its own `Engine`, serving one kind of request in order (§4.3) |
| `Source` | The TUI's read port: store-served views, the collector, and the token report (§4.4) |
| `Host` | The TUI's action port, implemented by the binary over the CLI's command path (§4.5) |
| Timer mode | No daemon connection: the TUI refreshes on its own timer (§6.2) |
| Live mode | Connected to the daemon's socket: its events drive refreshes (§6.3) |
| Suspended | The TUI has handed the terminal back for a foreground action (§7.3) |
| Signal token | The process's cancel token (core §14.1), in which the CLI's handler records SIGINT, SIGTERM and SIGHUP. Only the UI thread and a foreground action read it |
| Shutdown token | A second `Cancel`, which the UI thread sets only when the TUI shuts down (§10.2). Every engine lane and the bridge's `Ask` watch it |

## 3. Command

`tagteam [--provider P] [--no-color] [--debug] tui [accounts|tokens|history]`

- **The screen argument** picks the first screen. The default is `accounts`.
- **A bare `tagteam`** opens the TUI when stdin and stdout are both terminals and `--json` is
  not given. Anywhere else it runs `list`, so scripts and pipes see no change (§12.1 amends core
  §13.1). Its global flags apply as they would to `tui`.
- **Refusals,** before the terminal is touched:
  - `--json` is a usage error (exit 2): the TUI has no JSON output.
  - stdin or stdout not being a terminal is a usage error (exit 2): `tagteam tui needs a
    terminal`.
  - An unreadable profile marker refuses as every command but `statusline` does (core §12.8).
- **`--provider`** narrows every screen to that provider, as it narrows `list`.
- **`--no-color` and `NO_COLOR`** turn colour off (§5.1).
- **`--debug`** records DEBUG in the log file only. While the TUI owns the terminal, nothing is
  written to stderr (§10.3).
- **Exit codes:** 0 after `q` or Ctrl-C. 128 + the signal number after a SIGTERM, a SIGHUP,
  or a SIGINT sent to the process (core §14.1, §10.2). 1 for an error that ends the TUI
  (§10.4). 2 for a usage error.
- **In a run shell** (core §12.8) the TUI works. Read-only views work, the session's own
  account is marked `▶ this`, and account-changing actions refuse with the CLI's own refusal.

## 4. Architecture

### 4.1 Crates

| Crate | Change |
|---|---|
| `tagteam-tui` (new library) | The state machine, the screens, the lanes, the `Source` over the engine, the live thread, and terminal setup and restore. It depends on `tagteam-core`, `tagteam-provider`, `tagteam-engine`, `tagteam-daemon` (for its protocol types and client, §8), `ratatui` (with its default crossterm backend), `crossterm`, `unicode-width`, and `jiff` (the report's time zone, TR §10) |
| `tagteam` (binary) | The `tui` subcommand, the bare-`tagteam` rule, and `tui_host.rs`: the `Host` implementation over the CLI's dispatch (§4.5) |

**Dependency rule.** `tagteam-tui` never depends on the `tagteam` crate, which would be a cycle.
Everything that must match the CLI exactly crosses the `Host` trait. Core §4.1's rules hold
otherwise: `tagteam-tui` reaches providers only through the engine.

**Rendering.** `ratatui` with its crossterm backend. crossterm supports macOS and Linux, needs
no C library (so the static musl builds stay static, core §16), and is ratatui's default. The
plan pins both versions.

### 4.2 State machine

The TUI is an Elm-style loop.
- **`State`** holds everything on screen: the current screen, each screen's data and selection,
  the drill-down stack, the open overlay, the mode, the toast and the auto line. *(Amended by the TUI redesign spec §5.3.)*
- **`update(&mut State, Msg) -> Vec<Cmd>`** is pure. A `Msg` is a key, a resize, a tick, a
  lane's result, or a live event. A `Cmd` is a lane request, a live-client request, a suspend,
  or quit.
- **`view(&State, &mut Frame)`** is pure. It renders the state and reads nothing else.

### 4.3 Threads and lanes

- **The UI thread** owns the terminal. It blocks on one `Msg` channel with a 250 ms timeout,
  checks the signal token on every wake (§10.2), runs `update`, executes the returned commands,
  and redraws only when the state changed.
- **The input thread** turns crossterm events into `Msg`s. It waits on the terminal, on SIGWINCH
  and on a waker the UI thread uses to pause, resume or stop it, so it makes no wake-up while idle
  and acknowledges a pause at once (§7.3). *(Amended by the TUI redesign spec §14, PERF-R-9.)*
- **Five lanes.** The four engine lanes each own an `Engine` that the binary's factory builds
  from the same `Context`, so `Engine` never needs to be `Sync`. Each lane's engine takes the shutdown token
  as its cancel token, never the signal token (§2). A Ctrl-C meant for a foreground action
  (§7.3) therefore never interrupts a lane.

  | Lane | Serves |
  |---|---|
  | views | `accounts`, `history`: store-served, no network |
  | report | `token_report`: reads the index only |
  | index | `refresh_tokens`: the transcript catch-up, which can run for minutes on a cold index |
  | actions | the collector and every `Host` action, one at a time |
  | daemon | the daemon's `status`, over a short-lived connection of its own (§8); no engine |

  The index has its own lane so that report queries never wait behind a catch-up (§5.3). The
  daemon lane keeps a slow `status` reply (up to 2 s, D5.7) off the views lane.

- **Coalescing.** A lane merges a queued request with an identical one still queued. Each
  request carries the generation of the state that asked for it, and `update` drops a result
  whose generation is out of date. A slow history load therefore never overwrites a newer
  selection.
- **The live thread** owns the daemon connection (§8). It only reads the socket and forwards
  what it reads, and it never touches the store.
- **Joining.** An engine lane returns once its request channel is closed and its current
  request has unwound. The shutdown closes the channels and sets the token before it joins the
  engine lanes (§10.2). The live thread and the daemon lane are not joined: they never touch the
  store, so a process exit can leave nothing of theirs half written.
- **A lane that panics** closes its channel. The UI thread sees the disconnect and ends the TUI
  as an error (§10.4).

### 4.4 `Source`

`Source` is the read port, implemented over `Engine`, with a fixture implementation for tests.

| Method | Engine operation |
|---|---|
| `accounts(provider)` | `Engine::accounts` (core §4.2, the view `list` renders) and `shell_account` |
| `history(account, window, since)` | `Engine::history` (core §13.4) |
| `collect_due(accounts)` | `Engine::collect_usage(CollectMode::OnDemand { accounts })` (core §8.3) |
| `refresh_tokens(provider, progress)` | `Engine::refresh_tokens`, always with `Wait::Try` and no budget (TR §5, §9) |
| `token_report(query)` | `Engine::token_report` (TR §10) |
| `token_index_state(provider)` | `Engine::token_index_state` (TR §10) |

### 4.5 `Host`

`Host` is the action port. The binary implements it in `tui_host.rs`.

```rust
pub trait Host: Send + Sync {
    /// Runs `action` inside the TUI, on the actions lane. Prompts arrive through `ask`.
    fn act(&self, action: &Action, ask: &mut dyn Ask) -> ActionOutcome;
    /// Runs `action` on the real terminal, on the UI thread, after the TUI has suspended
    /// (§7.3).
    fn act_foreground(&self, action: &Action) -> ActionOutcome;
}

pub trait Ask {
    fn confirm(&mut self, question: &str, default_yes: bool) -> bool;
    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize>;
}

pub enum ActionOutcome {
    Done { exit: i32, out: String, err: String },
    /// The command needs the terminal itself, and has changed nothing (§7.3).
    NeedsTerminal,
}
```

- **`act`** builds the command's arguments, parses them with the CLI's own `clap` definition,
  and runs the CLI's dispatch (the part of `app::run` after process setup). The `Io` it passes
  in carries two capture buffers and a `Prompter` that forwards `confirm` and `choose` to `ask`.
  It always passes `--no-color`, so the captured text is plain.
- **The account reference** is `--provider P --expect-account <ID> <position>`, from the
  selected account's ID, provider and position as the TUI last read them.
  - `--expect-account` is a new hidden global flag (§12.1). The CLI's `App::resolve`, through
    which every account command resolves its `ACCOUNT` (core §10.4), refuses with
    `stale-selection` unless the account it resolves has that ID. The command then acts on that
    ID, as it already does.
  - A `move` made elsewhere between the TUI's last read and the action therefore cannot
    redirect it to another account. The refusal (`the selected account changed; nothing was
    done`) re-reads the views.
- **`secret` is declined.** No action in this spec asks for a secret.
- **The keychain unlock** (core Appendix A.3) needs the terminal for macOS's own password
  prompt. The bridge's prompter declines that one question and notes it. The command then fails
  before touching anything, as Appendix A.3 guarantees, and `act` returns `NeedsTerminal`. The
  TUI re-runs the action with `act_foreground` (§7.3). Because this is not a separate pre-check,
  a keychain that locks between two actions is caught too.
- **`act_foreground`** runs the same dispatch with the CLI's own `TtyPrompter` and the real
  stdout and stderr, as the CLI itself would.
- **Tokens.** `act` runs the CLI path with the shutdown token as its cancel token, and
  `act_foreground` with the signal token (§2). A `q` reaches an action in the TUI, and a Ctrl-C
  reaches an action in the foreground.

## 5. Screens

### 5.1 Layout

```
 tagteam   1 Accounts   2 Tokens   3 History                    ● auto on · switches at 90%   ? keys
 ⇄ 11:28  auto switched 1 alex → 2 work · alex's 5h reached 93% · next check 14:16
 ────────────────────────────────────────────────────────────────────────────────────────────
 (banner, 0–3 rows)
 (body)
 s switch  b best  n next  ⏎ history  P pause auto  r refresh                    ? keys  q quit
```

- **The chrome rows** are the header, the auto line, the Now strip on Tokens and History, a faint
  rule from 30 rows, the banner, the body and the footer. Which rows show does not change with the
  daemon's mode. The header shows the tabs and auto's state with its threshold (§7.4), the auto
  line shows auto's last action in plain words, the banner shows an unmanaged live login, a run
  shell, the start-up warnings or an error (§10.4), and the footer shows the screen's keys with
  `? keys` and `q quit` pinned. A toast replaces the footer's keys, and a running action shows a
  spinner with its label. *(Amended by the TUI redesign spec §5.1–§5.6.)*
- **Colour.**
  - Three tiers: tier 0 under `NO_COLOR`, `--no-color` or `ui.color = never`; tier 1 with the 16
    named ANSI colours; tier 2 in truecolor when `COLORTERM` says so and the terminal reports its background. Each
    colour is a role (`text`, `muted`, `faint`, `accent`, `warn`, `crit`, `ok`, `sel-bg`, `surface`), and the theme is
    the only place that names one.
  - Percentages and gauges take core §13.5's severities (≥ 70 % warning, ≥ 90 % critical).
  - The selection is `▌` in the first column with the name in bold. Without colour, emphasis is
    bold, underline or DIM.

  *(Amended by the TUI redesign spec §4.1–§4.3.)*
- **Size.** Below 80×24 the body is replaced by `Terminal W×H · tagteam needs 80×24`. Wider
  terminals get more columns (§5.3). *(Amended by the TUI redesign spec §4.5.)*
- **Providers.** With several providers, the Accounts and History lists are grouped by
  provider, as `list` groups them. With one, no provider is shown.

### 5.2 Accounts

```
    #  ACCOUNT               5H               7D                       SPEND     AGE
 *  1  michael@example.com   ██░░░░  9% 2h40m ███████░ 77% 3d09h ▲pace  €0/€20     2m
 ▶  2  spare@example.com     ███░░░ 31% 4h02m █░░░░░░░ 12% 5d01h       —         14m
    3  work (w@corp.com)     relogin required
 ─ 1 michael@example.com · active · oauth ───────────────────────────────────────────
 5h    9%  resets 2h40m
 7d   77%  resets 3d09h  ▲ ahead of pace (expected 62%)  runs out ~2d04h (regression)
 fetched 2m ago · next poll in 1m · login expires in 6d
```

- **The table** has the same rows, columns, markers and words as `list` (core §13.1): the
  active `*`, session-owned `▶` (`▶ this` in a run shell), disabled, quarantined, `▲ pace`, a
  scoped-window column per model that any account has, `usageStatus` in words with
  `retry <countdown>`, and `AGE`. Each window adds a gauge.
- **The detail pane** shows the selected account:
  - each relevant window with its reset, pace and projection (core §8.7);
  - the data age and the next poll;
  - `usageError` and `usageRetryAt` when the status is not `ok`;
  - the login's expiry, and whether it is session-owned.
- **Countdowns tick.** The UI's tick redraws when a visible countdown's text changes. The data
  itself is re-read only on refresh (§6).
- **`Enter`** opens History for the selected account.

### 5.3 Tokens

```
 [Day] Month Session Block    30 days    group: account    filter: all accounts · all models
 DAY         ACCOUNT    INPUT  OUTPUT  CACHE W  CACHE R   TOTAL     COST
 2026-10-14  michael     1.2M    310k     4.1M    88.0M   93.6M   $41.20 ████████
             spare       0.3M     80k     0.9M    20.1M   21.4M    $9.75 ██
 2026-10-13  michael     …
 Enter sessions  p period  g group  f filter  [ ] range  Esc back
```

- **The query** is TR §10's `TokenQuery`, for the default provider or `--provider`'s.
  - **Period** (`p` cycles): `Day`, `Month`, `Session` or `Block`. Rows come in the report's
    order (TR §10.1): newest first, then accounts by position, removed accounts, unattributed,
    and models by name.
  - **Range** (`[`/`]`): 7 days, 30 days, 90 days or all, as `since_ms` with no `until_ms`. The
    default is 30 days, as for `tagteam tokens`. For `Month`, the default starts on the first
    day of the eleventh month before the current one (TR §11.1).
  - **Group** (`g` cycles): `None`, `Account`, `Model` or `AccountModel`. Blocks are per
    account (TR §10.1), so with `Block` the header shows the grouping the report applies.
  - **Filter** (`f`): a dialog that sets `accounts` and `models`.
    - **The choices** come from an inventory independent of the active filter. When the dialog
      opens, the report lane runs one unfiltered query: `Month`, `AccountModel`, no range, no
      filters. The choices are that query's `accounts` (current, removed and unattributed) and
      `models`, together with the current accounts from the accounts view.
    - **Selecting every choice** of a kind sends `None` for it, so models indexed later are
      included as well. Any narrower selection sends `Some` of exactly those, and selecting
      none of a kind is refused in the dialog (TR §10.1: `Some` of an empty list matches
      nothing).
  - **Time zone:** the system zone. The header shows the report's `time_zone`.
- **Columns.**
  - At 80 columns: period, account, input, output, total and cost.
  - From 120 columns, also cache write (5-minute and 1-hour where they fit), cache read, the
    response count, and a bar of cost relative to the largest row.
  - The model column appears when the grouping includes the model.
  - Token counts are scaled (`12.4M`) and costs are dollars with cents, as in TR §11.2.
- **Rows that need a mark.**
  - `RowAccount::Account` shows as its alias, else its label, with its position.
    `Removed` shows as `removed`, and `Unattributed` as `unattributed`.
  - A row with `unfinalized > 0` marks its output `≥`: a lower bound (TR §2.3).
  - A `Cost` with `partial` marks the cost `≥`. A missing cost shows `—`.
- **The total line and footnotes,** as TR §11.2 has them. The total line is `TokenTotal`. The
  footnotes cover:
  - unpriced models (`unpriced_models`);
  - unfinalized responses (`output is a lower bound for N subagent responses`);
  - skipped lines and unreadable files (`IndexState`);
  - price overrides (`price_overrides`);
  - the price table's date (`prices_as_of`).
- **Drill-down.** `Enter` on a row runs `TokenQuery::drill(row)` (TR §10). From a day, month or
  block row, that row's sessions (project, first and last time, accounts, models, tokens and
  cost). From a session row, its per-model rows. At the per-model rows `drill` returns `None`,
  and `Enter` does nothing. `Esc` or `Backspace` goes back one level, and keeps the selection by
  `RowKey`, which is stable across refreshes.
- **Blocks.** A block row shows its start and end, and whether it is observed or inferred
  (TR §9). The active block's row also shows elapsed and remaining time, burn rate (tokens per
  minute, cost per hour) and the projected tokens and cost at its end (`ActiveBlock`).
- **Indexing.**
  - **While the TUI's own refresh runs,** the header shows its `IndexProgress` as
    `indexing transcripts 1.2 GB / 3.3 GB`.
  - **While `refreshed_at_ms` is `None`** (no complete refresh yet, TR §14), the table shows
    what the index holds, labelled `catching up`.
  - **When the TUI's refresh returns `Busy`,** the header shows `catching up elsewhere`.
  - **Otherwise** the header shows the time of the last complete refresh.

### 5.4 History

```
 ACCOUNTS               5h   ▁▂▃▅▇█▁▁▂▃▅▆▁▂▃     9%  +4.1 pts/h  lasts to reset
 * 1 michael@…          7d   ▂▂▃▃▄▄▅▅▆▆▆▇▇▇█    77%  +1.2 pts/h  runs out ~2d04h (regression)
   2 spare@…            Fable ▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁     0%
                        7 days  [ ] range
```

- **The account list** is on the left (`j`/`k`). The selected account's windows, as
  `Engine::history` returns them (core §13.4: the relevant windows), are on the right.
- **Per window:** a sparkline, the latest `pct`, the burn rate in points per hour, the ETA with
  its projection method, and whether the window will last to its reset (core §8.7).
- **The sparkline** has one bucket per column across the range. Each bucket shows the highest
  `pct` sampled in it, an empty bucket stays blank, and a reset shows as a drop.
- **Range** (`[`/`]`): 1, 7, 14 or 30 days. The default is 7, as for `history`.
- **It never fetches** (core §13.4).

### 5.5 Overlays

- **Help** (`?`) lists the keys of the current screen and then those of every screen, from one
  key table, and ends with what each mark and colour means. Every key list is on one line.
  *(Amended by the TUI redesign spec §9.2.)*
- **Dialogs.** A CLI prompt shows as a confirm or choose dialog (§7.2). Alias and position
  input use a one-line text dialog. `Esc` declines. Dialogs and panels float over a dimmed
  screen in a rounded frame; panels cover the body. `q` closes the open overlay first, except in
  a text input, and quits only when nothing is open. *(Amended by the TUI redesign spec §9.1.)*
- **The result panel** shows an action's captured output when it does not fit a toast (§7.2), and
  is dismissed with `Esc`, `Enter` or `q`. *(Amended by the TUI redesign spec §9.3.)*
- **The doctor panel** (`D`) shows `doctor`'s report, scrollable. `o` re-runs it with
  `--online`.

### 5.6 Keys

The keys are the TUI's one key table (redesign §10's), without `w`, and with History keeping its
account keys (`j`/`k` account, `[` `]` range, `J`/`K` scroll) until T4. Accounts' `g` and `G` are
dropped. *(Amended by the TUI redesign spec §10.)*

## 6. Data and refresh

### 6.1 First frame

The binary builds the context and engines as it does for `list`, then the UI thread enters the
alternate screen. It waits up to 100 ms for the views lane's first `accounts` result, so it
does not flash an empty table, and then draws. A slower result draws `Loading accounts…` first. *(Amended by the TUI redesign spec §5.7.)* The
first frame never waits on a fetch: unlike `list`, the TUI collects after drawing (§6.2). The
report lane starts the report's catch-up in the background (§6.4).

### 6.2 Timer mode

With no daemon connection, every 30 s:
- the views lane re-reads `accounts`, and `history` when History is open;
- the actions lane runs `collect_due` for the shown accounts. `CollectMode::OnDemand` fetches
  only an account whose reading is older than the 180 s floor and whose poll is due (core §8.3),
  within the hourly budget (core §8.6). The poll plan, not the timer, decides what is fetched.
- the views are re-read once more when a collection returns a fetch.

### 6.3 Live mode

With a daemon connection, the timer's `collect_due` skips every provider that something else
collects for. That is a provider whose daemon mode (D4.3) is `auto` or `collect`, or
`waiting` with `waitingFor` `engine-lock`, where another engine collects for itself. A provider
that is `idle`, or `waiting` with `waitingFor` `schema`, is collected for on the timer as in
timer mode, since nothing else collects for it.
- Each event names the views it affects (§8), and the TUI re-reads them, debounced to 250 ms.
  The store stays the source of truth: an event is a reason to re-read, never data to show,
  except on the auto line (§8). *(Amended by the TUI redesign spec §5.3.)*
- A 60 s timer still re-reads `accounts` and re-fetches the daemon's `status`, so a missed
  event, or a setting changed elsewhere, costs at most a minute.
- Which providers `collect_due` skips is decided from the latest `status` reply (§8).
- `r` still runs `collect_due`, exactly as `list` would.

### 6.4 Token index

- **Timer mode.** The index lane runs `refresh_tokens(provider, Wait::Try, None, …)` (TR §5)
  when the TUI starts, then every 60 s while Tokens is open. `r` on Tokens runs one at once.
- **Live mode, with a daemon that keeps the index current.** That is, one whose `status.jobs`
  includes the index refresh job (D13, TR §12). The TUI does not refresh on a timer. It re-runs
  `token_report` on the daemon's `token-index` event when the event's `changedAt` is newer than
  the report on screen (§8, §9). `r` on Tokens still runs a refresh, as a person's request. A
  daemon without that job leaves the TUI refreshing as in timer mode.
- **When to re-query.** While Tokens is open, the report lane re-runs `token_report` when any of
  these holds:
  - **The index changed.** `changed_at_ms` moved past the report on screen. The report lane
    checks `token_index_state(provider)` (TR §10, a few small reads) every 5 s while a refresh
    runs, so the table fills in as batches commit. It also checks after each refresh returns,
    and on a `token-index` event.
  - **The accounts changed.** A re-read of `accounts` returned different rows: an alias, a
    position, an account added or removed. `RowAccount` carries them.
  - **Time moved.** Every 60 s, because the active block's burn rate and projection, a rolling
    range, and the day or month boundary all depend on now (TR §9). This also catches evidence
    that `run` or the statusline recorded (TR §12).
  - **The active block ended.** At its `end_ms`.

  Between re-queries, the active block's remaining time counts down on the UI's 1 s tick, from
  `end_ms`.
- **Never on the UI thread.** `token_report` only reads (TR §5), so a query stays cheap while a
  refresh runs.

## 7. Actions

### 7.1 The actions

| Key | Action | What runs | Added by the TUI |
|---|---|---|---|
| `s` | Switch to the selected account | `switch <ref>` | — |
| `b` / `n` | Switch by strategy | `switch --strategy best` / `next-available` | — |
| `e` | Enable or disable | `enable <ref>` / `disable <ref>` | — |
| `a` | Set or clear the alias | `alias <ref> NAME` / `alias <ref> --unset` | Text dialog; empty clears |
| `m` | Move | `move <ref> POS` | Number dialog |
| `x` | Remove | `remove <ref>` | Confirm dialog, default no (§7.2) |
| `A` | Add the current login | `add` | — |
| `R` | Run a session on the account | `tagteam run <ref>`, as a child process | Always suspended (§7.3) |
| `D` | Doctor | `doctor`, `doctor --online` | Panel (§5.5) |
| `P` | Pause or resume auto-switch | `config set provider.<id>.daemon.autoswitch false\|true` | Confirm dialog; live mode only (§7.4) |
| `r` | Refresh | `collect_due` (§6.2), or `refresh_tokens` on Tokens | — |

### 7.2 Running an action

- **One at a time.** Actions and `collect_due` share the actions lane. While an action runs,
  the footer shows its spinner. Another action key toasts `still <label> · wait for it to finish`
  and is dropped, not queued. A suspended action counts as running (§7.3), and the timer's
  `collect_due` is skipped while any action runs. *(Amended by the TUI redesign spec §9.4.)*
- **Prompts.** A `confirm` or `choose` from the CLI path opens a dialog. The actions lane
  waits on a reply channel until the person answers, or until a shutdown declines it (§10.2).
  `Esc` answers as a decline, which the CLI reports as cancelled.
- **Remove's confirmation** is the TUI's own: `Remove 2 spare@example.com? Its stored login is
  deleted; Claude Code is not changed.` The default is no, because one key is easier to hit
  than a typed command. The CLI's own refusals (core §10.3: never the live login) still apply.
- **The outcome** follows the redesign spec's table: a doctor report opens the doctor panel; a
  clean exit 0 toasts the CLI's output when it fits; a warning or a failure toasts when it is one
  line that fits, and opens the result panel otherwise. The spinner and toasts use the action's label (`switching to 3
  spare…`). Each action is followed by a re-read of `accounts`. *(Amended by the TUI redesign
  spec §9.3, §9.4.)*
- **Inside a run shell**, account-changing actions fail with the CLI's refusal (core §12.8),
  shown like any outcome.

### 7.3 Suspending for the terminal

An action runs suspended when the CLI path needs the terminal itself: `run` always, and any
action whose keychain needs unlocking (`NeedsTerminal`, §4.5).

1. The input thread is paused, and acknowledges, so it cannot steal the child's keystrokes.
2. The TUI leaves the alternate screen, turns raw mode off and shows the cursor.
3. `Host::act_foreground` runs the action on the UI thread.
   - For `run`, the bridge spawns `<current executable> --provider P --expect-account <ID> run
     <position>` in the terminal's foreground process group. A separate process keeps `run`'s
     own process model and signal handling (core §12.5, §14.1) apart from the TUI's.
   - It waits for the child in slices, as `run` waits for `claude`: `waitpid` with `WNOHANG`,
     checking the signal token between slices, until it reaps the child.
4. The TUI prints `Press Enter to return to tagteam` and reads the answer with the CLI's
   cancellation-aware terminal read (`read_terminal_line`). It then re-enters the alternate
   screen, resumes the input thread and re-reads every view.

**Signals while suspended.** The TUI treats them as `run`'s parent treats them while `claude`
runs (core §12.5):
- **SIGINT and SIGQUIT** belong to the foreground action, which receives them from the terminal
  itself. The TUI survives SIGQUIT for its whole life, through the same unread-cell handler
  `run` uses (`signals::survive_quit`). In raw mode Ctrl-\ is only a key, so this changes
  nothing else. A SIGINT recorded in the signal token when the action returns is the action's:
  the TUI takes it (`Cancel::take`) and carries on.
- **SIGTERM and SIGHUP** sent to the TUI are forwarded to a `run` child as they arrive. An
  in-process foreground action sees them in the signal token, at its own cancellation points.
  Either way, once the action has returned they stay recorded, and the TUI shuts down (§10.2)
  without waiting for Enter.
- **Lanes and the live thread are untouched.** Lanes watch only the shutdown token (§4.3), and
  the live thread watches no token, so background work and the daemon connection carry on across
  a suspension.

### 7.4 Pause and resume auto

Pausing is the daemon's persistent setting `provider.<id>.daemon.autoswitch` (§8), so it
survives a daemon restart.
- **In live mode,** `P` asks `Pause auto for <display name>?` (or `Resume…`) *(Amended by the TUI redesign spec §9.4.)*, from the
  effective `autoswitch` in the daemon's `status`. A yes runs
  `config set provider.<id>.daemon.autoswitch false` (or `true`) through `Host`, exactly as the
  CLI runs it. Two seconds later the TUI re-fetches `status`: the daemon applies a settings
  change within about a second (D4.2), and a pause need not change the mode, so no event may
  follow (one switchable account is `collect` either way).
- **The header** shows each provider's state from the latest `status` reply (D4.3, D5.4).
  Events never set it directly (§8). Its words, glyphs and 80-column forms are the redesign spec's
  §5.2 table. *(Amended by the TUI redesign spec §5.2.)*

  With several providers, the header shows the state of the selected account's provider.
- **In timer mode,** `P` behaves as the redesign spec's §10 says: without a daemon it says how to
  install one, and the TUI does not change a setting whose effect it cannot show. *(Amended by
  the TUI redesign spec §10.)*
- **With several providers,** `P` acts on the selected account's provider.

## 8. Daemon socket client

The protocol is the daemon spec's D5 (`2026-10-14-tagteam-daemon-design.md`, Approved, commit
`96b9cea` on `daemon`), and D5 is the authority. What the TUI relies on:

- **The client** is `tagteam_daemon::client` (D5.7), and its types are in
  `tagteam_daemon::protocol`. `connect(env)` derives the socket path from `Env` alone
  (`env.data_dir().join("daemon.sock")`, D5.1). It returns `Connect::Connected(Client)`,
  `Connect::NoDaemon` at once when no daemon answers, or `Connect::Mismatch { server, client }`.
  The greeting read is bounded at 250 ms.
- **`Client::status()`** (D5.4) waits up to 2 s. For each provider it gives:
  - `mode`: `auto`, `collect`, `waiting` or `idle` (D4.3);
  - `autoswitch`;
  - `waitingFor`: `engine-lock`, `schema` or null;
  - `holderPid`;
  - `backoff`: `{reason: "keychain", until}` or null;
  - `lastTickAt`, `nextTickAt`, `lastOutcome` and `lastSwitch`.
- **`Client::subscribe()`** (D5.5) yields typed events in `auto --json`'s envelope, each with
  `ts`:
  - **auto's kinds:** `poll`, `switch`, `no-switch`, `account-quarantined`,
    `account-unquarantined`, `all-exhausted`, `sleep`, `error`, `config-warning`;
  - **the daemon's kinds:**
    - `usage`, after a `collect` round;
    - `changed`, when another process committed to the store, within 2 s;
    - `mode`, when a mode changes or a keychain backoff starts or ends;
    - `stopping`;
  - **reserved:** `token-index` (§9);
  - **anything else:** `Unknown { kind, raw }`, which the TUI ignores.

  A subscriber whose 256-event queue fills is disconnected (D5.5).
- **The socket is read-only** (D5.3: `status` and `subscribe` only). Pausing auto-switching is
  the setting `daemon.autoswitch` (D7), which the TUI writes through `config set` (§7.4).

**Behaviour.**
- **`status` is the authority for daemon state**, as the store is for account data. Events are
  reasons to re-fetch, and their fields are never applied to the header. Every `status` request
  opens its own short-lived connection on the daemon lane (§4.3), because `subscribe` hands the
  live thread's connection to the event iterator. Queued requests coalesce, so a burst of
  events costs one request, and the reply received last wins.
- **Connecting.** At start-up the live thread calls `connect`, then `subscribe`. Once it has
  subscribed, it asks the daemon lane to fetch `status`, and the UI thread to re-read every view
  it shows. Because the snapshot is taken after the subscription started, no state change falls
  between the two. This never holds up the first frame: the TUI starts in timer mode, and
  switches to live mode once the first `status` reply arrives.
- **What each event does.**
  - `poll`, `switch`, `usage`, `changed`, `account-quarantined` and `account-unquarantined`
    re-read `accounts`, and `history` when it is open.
  - `mode` re-fetches `status` (§7.4).
  - `token-index` re-runs `token_report` when Tokens is open and the event's `changedAt` is
    newer than the report on screen (§6.4).
  - The daemon's `no-switch`, `error` and `config-warning` events and its `status.lastSwitch`
    feed the auto line. *(Amended by the TUI redesign spec §5.3.)*
- **Disconnect** (an error, `stopping`, or a full queue). The TUI falls back to timer mode, and
  reconnects with backoff from 1 s up to 30 s. Each reconnection runs the connecting sequence
  above, so whatever the TUI missed is re-read (D5.5). The live thread forwards events to the
  UI channel as they arrive, so the TUI's own queue never lags. With `stopping`, the header says
  `daemon stopping (<reason>)` until the TUI reconnects or gives up.
- **Version mismatch.** The TUI stays in timer mode, the header says
  `daemon protocol <server> unsupported`, and it tries again every 60 s, in case the daemon has
  restarted on the same version (D5.6).

## 9. Token report view

The view is the token report spec's §10 (`2026-10-14-tagteam-token-report-design.md`, Approved,
commit `e6cc92a` on `token-report`), and TR §10 is the authority. The TUI renders those types
and defines no shapes of its own. What the TUI relies on:

- **The calls.** Both take `&self`, do their own store I/O, and run on the TUI's worker lanes,
  each with an engine of its own (TR §10.1).
  - `Engine::token_report(&TokenQuery) -> TokenReport` only reads. Before the first complete
    refresh it returns partial rows, with `IndexState.refreshed_at_ms` `None` (TR §14).
  - `Engine::token_index_state(provider) -> IndexState` is a few small reads (§6.4).
  - `Engine::refresh_tokens(provider, wait, budget, cancel, progress) -> RefreshOutcome`
    (TR §5): `Done(IndexReport)` or `Busy`.
- **The types.**
  - `TokenQuery`: `period`, `grouping`, `since_ms`, `until_ms`, `accounts`, `models`, `session`
    and `tz`. `TokenQuery::drill` is the drill-down.
  - `TokenReport`: `rows`, `total`, `index`, `time_zone`, `unpriced_models`, `prices_as_of` and
    `price_overrides`.
  - `TokenRow`: `key: RowKey`, `period: PeriodKey`, `account`, `model`, `accounts`, `models`,
    `counts: TokenCounts`, `responses`, `unfinalized` and `cost: Option<Cost>`.
  - `PeriodKey`: `Day`, `Month`, `Session` (with project and first and last time) or `Block`
    (with `observed` and `active: Option<ActiveBlock>`).
  - `RowAccount`: `Account`, `Removed` or `Unattributed`.
  - `IndexState`: `refreshed_at_ms`, `changed_at_ms`, `files`, `responses`, `skipped_lines`,
    `unreadable_files`, `oldest_ms` and `newest_ms`.
- **Concurrent refreshes.** A refresh holds the per-provider leaf `flock`
  `locks/tokens-<provider>.lock` (TR §5.1). The TUI always passes `Wait::Try` and
  `budget: None`, and runs on its own index lane. A contended refresh returns `Busy` at once.
  A refresh cancelled by the shutdown token stops at its next chunk and keeps what it committed
  (TR §5.2).
- **The `token-index` event** is the `IndexState` in camelCase: `provider`, `refreshedAt`,
  `changedAt`, `files`, `responses`, `skippedLines`, `unreadableFiles`, `oldest` and `newest`
  (TR §12). The daemon's branch emits it.

## 10. Terminal, signals and errors

### 10.1 The terminal

- **Entering:** raw mode, the alternate screen, cursor hidden.
- **Restoring:** a guard's `Drop` undoes all three, on every exit path. A panic hook installed
  before entering restores the terminal before the panic is logged (core §14.2). A panic on the
  UI thread then goes through the shutdown (§10.2), so the lanes still drain.
- **Ctrl-Z** suspends: the TUI restores the terminal, raises SIGTSTP, and on SIGCONT
  re-enters and redraws.
- **Resizing** redraws at the new size.

### 10.2 Signals and quitting

**The process never exits with a lane still running.** A lane may be inside a critical span
(core §14.1): a switch transaction, or the refresh gate between sending a token request and
persisting its successor. So every way out goes through one shutdown sequence.

**What starts a shutdown:**

| Trigger | Exit code |
|---|---|
| `q`, or Ctrl-C (a key in raw mode) | 0 |
| SIGTERM or SIGHUP, recorded in the signal token by the CLI's handler (core §14.1). The UI thread sees it within 250 ms, or when a suspended action returns (§7.3) | 128 + the signal number |
| SIGINT in the signal token while not suspended. Only `kill -INT` sends one, since in raw mode Ctrl-C is a key | 130 |
| A fatal error (§10.4) | 1 |
| A panic on the UI thread, caught at the TUI's top level with `catch_unwind`, after the panic hook has restored the terminal and logged it | 1 |

**The sequence:**

1. **Set the shutdown token,** with the triggering signal, or SIGINT for the other triggers.
   Every lane unwinds at its next cancellation point, and critical spans run to completion.
2. **Close the lanes.** The TUI stops submitting requests, drops every queued request that has
   not started, and closes every lane's request channel. An idle lane, blocked waiting for a
   request, wakes and returns.
3. **Decline prompts from here on.** The bridge's `Ask` waits for its reply in slices, and
   checks the shutdown token between them. From the moment the token is set, it answers a
   dialog that is already open, and any prompt that arrives later, as a decline, without
   showing it. The CLI then reports the command as cancelled or interrupted, as at its own
   prompts.
4. **Show progress.** While the terminal can still be drawn, the TUI shows
   `finishing <action>…`. For a signal or a fatal error it restores the terminal first. A
   second `q` does not skip the wait.
5. **Join** every engine lane.
6. **Report.** After the terminal is restored, the TUI prints on stderr what an action that was
   running did, in the CLI's words. A switch that committed is reported as switched (core
   §14.1).
7. **Exit** with the code above.

A lane that panicked has nothing left to join, and the others drain as usual. The input thread,
the live thread and the daemon lane are not joined: none touches the store, and all end with
the process.

### 10.3 Output and logging

While the TUI owns the terminal, nothing writes to stdout or stderr except the TUI:
- the log's stderr layer is off, so `--debug` and ERROR events go to the log file only (core
  §14.2);
- the bridge captures the CLI's stdout and stderr (§4.5).

A suspended action writes to the terminal as the CLI does.

### 10.4 Errors

- **A failed view read** keeps the last good data on screen and shows a banner with the error
  in the CLI's wording. The next refresh retries.
- **No store, or no accounts:** the Accounts body shows the title `No accounts yet` and the
  sentence `Log in with <launch command>, then press A to add that login.` *(Amended by the
  TUI redesign spec §5.7.)*
- **A failed collection** shows per account in its row (`usageStatus`, core §13.2), as in
  `list`.
- **A lane that died**, or a terminal that can no longer be written, is fatal. The TUI shuts
  down (§10.2), prints the error on stderr, and exits 1.

## 11. Testing

Tests follow core §15's isolation (core §15.1): fixture homes, the file-backed fake Keychain,
and no network.

- **State machine.** `update` unit tests cover:
  - navigation and the drill-down stack;
  - the dialogs' reply flow;
  - the refresh schedule on a fake clock, in timer mode and live mode;
  - switching between the two modes;
  - dropping out-of-date results;
  - the busy rule;
  - a report query answered while a catch-up still runs on the index lane;
  - the shutdown sequence for each trigger, which joins every engine lane before it exits, including
    from idle lanes blocked waiting for requests.
- **Rendering.** `ratatui`'s `TestBackend` renders each screen at 80×24 and 120×40 from fixture
  data, with and without colour, plus the too-small notice, the banners and the overlays.
  Expected buffers are kept as text in the tests, as the repository's other snapshot tests are.
- **`Source` over a real engine.** These use fixture homes with the CC provider and
  `tagteam-fake`: several providers, a run shell, an unmanaged live login, a stale reading.
- **`Host` bridge.** These mirror the CLI's tests:
  - a switch;
  - the unmanaged-login prompt answered both ways;
  - the session-owned refusal;
  - enable and disable;
  - alias set and clear;
  - a move;
  - remove, including the live-login refusal;
  - `add`;
  - a locked fake keychain returning `NeedsTerminal` with nothing written;
  - `--expect-account` refusing with `stale-selection` when another account has moved into
    the position, for switch, remove, disable and `run`, with nothing changed;
  - a shutdown that starts just before a prompt arrives: the prompt is declined unshown, and
    the command reports cancelled.
- **Daemon client.** Against a fixture socket server until the daemon merges, then against the
  real daemon. The tests cover:
  - `NoDaemon`, `Connected` and `Mismatch`;
  - the `status` reply filling the header, for every row of §7.4's table, with and without a
    keychain backoff;
  - events mapped to re-reads;
  - `Unknown` kinds being ignored;
  - disconnect, `stopping` and a full queue, with reconnecting re-reading every view;
  - the timer's `collect_due` skipping exactly the providers §6.3 names;
  - `status` being re-fetched after subscribing, on each `mode` event, 2 s after `P`, and every
    60 s, with an event's fields never applied to the header;
  - `P` writing the setting through `Host`. With one switchable account, a pause leaves the
    mode `collect` and sends no event, and the header still turns to `auto paused`.
- **Token report.** Against fixture `TokenReport` values until the token report merges, then
  over fixture transcripts. The tests cover:
  - each period and grouping, including `Block`'s per-account grouping;
  - the `≥` marks for `unfinalized` and a partial `Cost`, and `—` for a missing one;
  - the footnotes;
  - drill-down to sessions and to per-model rows, with `Enter` doing nothing at the bottom;
  - the selection kept by `RowKey` across a refresh;
  - `catching up` while `refreshed_at_ms` is `None`, and `catching up elsewhere` on `Busy`;
  - re-querying for each of §6.4's triggers (an index change, an accounts change, the 60 s
    timer, the active block's end), and not on a 5 s check that finds no change;
  - the active block counting down between re-queries, and turning inactive at its end;
  - the `Month` default range;
  - the filter's choices staying the full inventory after filtering to one model;
  - selecting everything sending `None`, a narrower selection sending exactly it, and an empty
    selection being refused.
- **End to end.** The real binary in a pty (`openpty`, as `tests/signals.rs` does):
  - the first frame appears;
  - the screen keys work;
  - `q` exits 0 with the terminal restored (alternate screen left, raw mode off);
  - SIGTERM exits 143 with the terminal restored, after an in-flight action has finished;
  - suspended under `R`, with a stub launch command in place of `claude`:
    - SIGTERM sent to the TUI's pid is forwarded, and both processes exit;
    - Ctrl-\ ends the stub, while the TUI survives and returns;
    - Ctrl-C ends the stub, and the TUI carries on, with an index refresh that was running
      not interrupted and the daemon connection still live;
  - a bare `tagteam` in a pty opens the TUI, and a piped one prints `list`;
  - `--json` and a non-terminal stdin are usage errors.
- **Performance.** `tests/perf.rs` adds the first frame within 50 ms p95 under `list`'s
  conditions (core §1.1), measured through a pty to the first byte of the accounts table.

## 12. Repository and release

### 12.1 Core spec amendments

Small hunks in core §10.4, §13.1 and Appendix C:
- **§13.1:** "A bare `tagteam` runs `list`, until the TUI lands in sub-project 3" becomes "A
  bare `tagteam` opens the TUI (sub-project 3) when stdin and stdout are terminals and `--json`
  is not given; otherwise it runs `list`." The command table gains
  `tui [accounts|tokens|history]`.
- **§10.4:** a closing paragraph on the hidden global flag `--expect-account ID`. It is hidden
  from `--help` and from completions, and the TUI passes it (§4.5). When it is given, an
  `ACCOUNT` that resolves to an account with another ID is refused with the error kind
  `stale-selection` (exit 1), before anything is changed. It has no effect on a command that
  resolves no `ACCOUNT`.
- **Appendix C:** the `tui`, `watch`, `menubar` row splits in two. `tui`, `watch` → `tui`
  ("one command for both"). `menubar` → sub-project 4.

### 12.2 Release

- **Publishing.** `tagteam-tui` is published (core §16, an internal library with no stable
  API). It joins the loop in `.github/workflows/publish-crates.yml` in dependency order: core,
  provider, cc, engine, daemon, tui, `tagteam`. It registers `release.yml` as its trusted
  publisher.
- **First publish.** The first crates.io publish of a new crate needs a short-lived token
  (CONTRIBUTING, "Releasing"). Michael is told before that release.
- **Dependencies.** `ratatui` and `crossterm` must pass the repository's licence and advisory
  checks, and the musl smoke job must still find a static binary (core §16).
- **Completions.** Shell completions pick up `tui` from the command definition (core §13.7), and
  their snapshot tests are updated.

### 12.3 Merge and rebase

This branch merges last. On rebase it:
- replaces the report fixtures with the token report's types and calls;
- replaces the socket fixtures with the daemon's client types;
- renumbers nothing in the store, because this branch adds no migration;
- re-runs the full suite and the end-to-end tests against the merged daemon.

## 13. Open items

| # | Item | Resolution |
|---|---|---|
| T1 | Token report types (§9) | Closed: TR §10, signed off at `e6cc92a` |
| T2 | Daemon socket protocol (§8) | Closed: daemon spec D5, signed off at `96b9cea` |
| T3 | Pause and resume auto (§7.4) | Closed: the daemon's persistent `provider.<id>.daemon.autoswitch` setting, written through `config set`; the socket stays read-only |
| T4 | Whether `refresh_tokens` may run concurrently across processes (§9) | Closed: yes, under a per-provider leaf `flock`; the TUI calls it with `Wait::Try` |
