# tagteam TUI, part C — the live daemon client: Implementation Plan

**Status:** Implemented — https://github.com/michaelrigart/tagteam/pull/18

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** With a daemon running, the TUI follows it live. A live thread subscribes to the daemon's socket and passes its events on; a daemon lane fetches its `status` over a connection of its own. In live mode the header shows what the daemon does, the activity line its latest event, and the views are read again as events come, while the TUI leaves to the daemon the collection, and the token index, it keeps current. `P` pauses or resumes the daemon's auto-switching through `config set`. Without a daemon, or once it goes, the TUI is in timer mode as parts A and B built it, and it reconnects with backoff.

**Architecture:**
- **`live.rs`** (Task 1) is the TUI's side of the socket, over `tagteam_daemon::client`:
  - **the live thread** connects, subscribes, and passes on `Up` with the link's name, each event, then `Down`, through a bounded queue (`LiveQueue`); it drops the link when the queue is full, or at `stopping`, or when the UI ends that link by its name through the client's new `Events::closer`; it tries again with backoff, and after a protocol mismatch every 60 s;
  - **`fetch_status`** is the daemon lane's work: a connection of its own, the greeting, `status`.

  Neither touches the store, and neither is joined (spec §4.3, §10.2).
- **`daemon/`** is live mode in the state machine:
  - `mod.rs` (Task 2): the link it follows, by name, its `status`, one request at a time, which events read what, the 60 s timer, and which providers something else collects for;
  - `words.rs` (Task 3): the header's mode, from `status` alone, and the activity line's words.
- **The runtime** (Task 2) spawns the daemon lane beside the engine lanes and the live thread beside them, takes the live queue whole at each wake, and applies every message waiting before it draws. `TuiOptions` gains the daemon's socket, which the binary takes from the environment (D5.1).
- **The view** (Task 3) draws the mode in the header, on a second row when the first has no room, and the activity line above the footer in live mode.
- **The token index** (Task 4): a daemon whose `status.jobs` has the index job replaces the 60 s refresh, and its `token-index` events read the report again, at most every 5 s.
- **`P`** (Task 5) is `Action::Autoswitch`, which runs `config set provider.<id>.daemon.autoswitch` through `Host`; `status` is fetched again 2 s later.
- **End to end** (Task 6): the real binary in a pty beside a real `tagteam daemon`, and the first frame timed beside a daemon and beside a socket that never answers.

**Tech Stack:**
- As parts A and B: Rust, edition 2024, at 1.88; `ratatui` 0.30.2; `crossterm` 0.29.0; `jiff` 0.2.37; std threads and `mpsc`.
- Part C adds `tagteam-daemon` (spec §4.1, §12) to `tagteam-tui`'s dependencies, and moves `serde_json` from its dev-dependencies to its dependencies: the events' fields are JSON (D5.7).
- Tests: the daemon's own socket server (`tagteam_daemon::server::Server` over a `StatusBoard`) and fake daemons on Unix sockets (`crates/tagteam-tui/tests/live.rs`); ratatui's `TestBackend`; `openpty` beside a real `tagteam daemon` (`tests/tui_pty.rs`); `tests/perf.rs`.

**Spec:** `docs/superpowers/specs/2026-10-14-tagteam-tui-design.md` (In progress: part A implemented, part B executed and reviewed). "D*n*" cites `docs/superpowers/specs/2026-10-14-tagteam-daemon-design.md` (Implemented), and D5 is the authority on the protocol. Read before any task:
- the TUI spec: §1.1 (criteria 2, 3, 4 and 6), §4.1, §4.3, §5.1, §5.6, §6.3, §6.4, §7.1, §7.2, §7.4, §8, §10.2, §10.4, §11 ("Daemon client", "End to end", "Performance") and §13 (T2, T3);
- the daemon spec: D4.2 (a setting applies within about a second), D4.3 (the modes), D5 (all of it), D7 (`daemon.autoswitch`), D13, and the errata on D5.3 and D13;
- part A's plan, `docs/superpowers/plans/2026-10-14-tagteam-tui-a-core.md`, and part B's, `docs/superpowers/plans/2026-10-16-tagteam-tui-b-tokens.md`: their rulings R1–R22 and B1–B29, which stay in force, and their Interface Contracts, which this plan extends.

**What the daemon gives, beyond D5** (from the daemon's author; Task 1's and Task 4's tests pin each):
- **`status.jobs`** names `sessions-watch` (every 5 s) and `token-index` (every 60 s, or at once while a refresh is incomplete), as `tagteam_daemon::tokens::{SESSIONS_WATCH, TOKEN_INDEX}`.
- **`token-index`** is `Event::TokenIndex { provider, raw }`. `raw` is the index's state in camelCase with ISO 8601 UTC times to the second: `{provider, refreshedAt, changedAt, files, responses, skippedLines, unreadableFiles, oldest, newest}`. It is broadcast once per `changedAt` per provider, never while `changedAt` is null.
- **The index can trail by up to a minute:** the daemon polls, and does not watch `projects/`. During a cold catch-up `changedAt` moves with each one-second slice, so `token-index` can come about once a second.
- **A job's `lastError`** is its run's first error, `<provider>: <message>`, or `store error: …` (or `store: …`) when the store cannot be read.
- **`status` is a snapshot** that the scheduler updates when a job's run ends (D5.4): it can trail the events of a run still going (C-R5).

**Parts.** The TUI spec is implemented in three plans:
- **A, the core TUI,** is implemented on `tui`.
- **B, the Tokens screen,** is on `tui`, executed and reviewed.
- **C (this plan)** implements §8 (the socket client), §6.3 (live mode), the live-mode parts of §6.4, §7.4 (`P`), the daemon lane and the live thread of §4.3, the activity line and the live header of §5.1, `P` in §5.6, §10.2's shutdown as it applies to the live thread and the daemon lane, §11's tests for these, and §12's dependency on `tagteam-daemon`. With it the spec is implemented.

## Execution notes

- **Where:** worktree `~/Code/tagteam-tui`, branch `tui`, from part B's head. This plan was verified on `3575c3b`: part A on main `d6c2e2e`, part B as executed and reviewed, and this plan's own commit. When execution starts, set this plan's `**Status:**` to `In progress` in one commit.
- **No new crates:** `tagteam-daemon` is the workspace's own crate, and `serde_json` is the workspace's. Task 1's build adds `tagteam-daemon` to `tagteam-tui`'s entry in `Cargo.lock`, offline.
- **Outside the sandbox:** binding a Unix socket and `openpty` fail with EPERM inside it. Run these there:
  - `cargo test -p tagteam-tui --test live` (Task 1's socket tests);
  - `cargo test -p tagteam-tui --lib`, for one test from Task 2 on: `runtime::tests::a_failure_judged_after_the_live_thread_replaced_its_link_leaves_the_new_link_alone` runs a live thread against a fake daemon's socket;
  - `cargo test -p tagteam --features test-support --test tui_pty`;
  - the timing tests: `cargo test --release -p tagteam --features test-support --test perf -- --ignored tui_first_frame`.

  The sandbox also refuses what parts A and B list: `tagteam`'s `prompt::tests` and `service_cmd` test, `tests/signals.rs`, `tests/daemon_cli.rs`, `tests/daemon_procs.rs`, `tagteam-daemon`'s `client`, `server` and `umask` tests, two of the engine's `tests/doctor.rs`, and `xtask`'s compat tests. They pass outside it.
- **Format and lint, in every task:** `cargo fmt --all` first. Clippy passes both with and without `--features tagteam/test-support`, with `-D warnings`.
- **Test isolation (core §15.1):** tests never reach the real HOME, network, Keychain, `claude`, or a daemon the person runs. The socket tests bind in short temporary directories (D14); the pty tests run their own `tagteam daemon` on a fixture home, with notifications off.
- **Unsafe code:** none in part C. `tagteam-tui` keeps `#![forbid(unsafe_code)]`.
- **Commits:** one imperative sentence in the repository's style, with no prefix and no attribution lines.
- **The code blocks are exact.**
  - `In PATH, replace:` names text that occurs once in the file as the step finds it; `with:` is what takes its place.
  - `Create PATH:` is a whole file.
  - A new module's tests are created first (Step 1), and its code is then put above them with one blank line between (Step 3).

### Integration notes

Each task below was applied as written, in order, in a fresh scratch clone of `tui` at `3575c3b`, part B's head as executed and reviewed, and committed as its steps say. A script applied every `replace`, `Create` and `put this above` block to the clone exactly as the plan prints it, checked that each `replace` text occurred once, and compared each task's commit with the reference tree byte for byte. After every task:
- Step 2's run failed as the step says (Task 6's passed, as it says);
- its own tests, `cargo fmt --all --check` and both clippy runs passed.

The tasks were first written and verified the same way on `0ed49bc`, part B before its review rounds. They were then rebased onto `941d245` and verified again. They then took the four findings of the first review round, the two of the second with their class, and the one of the third, each test first and in the task that owns it, and were verified from a fresh clone after each round. Last, they were moved onto `3575c3b` and verified once more. The pty suite ran after Tasks 2 and 6, and the workspace suite at the end.

**Totals:**
- The workspace suite at the end: 3792/3792, part B's 3722 at `3575c3b` and 70 new (the two new timing tests build only in a release build). The sandbox refuses 103 of those, which pass outside it: the 85 part B lists, the three new pty tests, 13 of the 16 in `tagteam-tui`'s `tests/live.rs`, the runtime's test of the suspension's order, and the daemon client's new test (Task 6's Step 4 lists them).
- `tests/tui_pty.rs`: 22/22, three runs in a row (after Task 2: 19/19).
- `tagteam-tui`'s `tests/live.rs`: 16/16, and its library 297/297, three runs in a row each; `tagteam-daemon`'s `tests/client.rs`: 16/16.
- The first frame, release build, 50 runs each, on `3575c3b`, at a load average of 10 falling to 6: all four within 50 ms in two runs: 11.0 and 11.8 ms with nothing due, 10.5 and 10.5 ms beside a running daemon, 10.5 and 11.0 ms beside a socket that accepts and never answers, and 10.3 and 10.7 ms for part B's catch-up test. Before the move, after round 3: 21.1 to 24.8 ms. After round 2 the catch-up test once measured 53.1 ms, in a burst of slow samples; run alternately three times with round 1's head at a load average of 35 falling to 5, round 2 kept all four within 50 ms (41.0 ms at most), and round 1's head measured the catch-up at 59.6 ms once: load, not the change. Round 1 measured the four at 30.0, 34.1, 41.1 and 33.1 ms, and before review at 18.3, 23.2, 10.7 and 18.3 ms.

What verification and review found and changed in the drafted tasks, one line each:

- **Review round 1, P1 (Tasks 1, 2):** the live thread forwarded every event into the UI's unbounded channel. A suspension, which takes no messages, let that backlog grow without bound, and the loop then drew once per event ahead of newer keys. The live thread now passes what it learns through `LiveQueue` (256 at most; an overflow drops the link, and the next link's snapshot restores the meaning), and the loop applies every message waiting before it draws (C-R17). Tests first: `a_full_queue_takes_one_down_for_what_it_held` and `a_flood_nobody_takes_stays_within_the_queue_and_the_link_is_dropped` (Task 1); `a_flood_during_a_suspension_stays_within_the_queue_and_a_snapshot_recovers_it` and `a_key_queued_behind_a_burst_is_handled_within_one_frame` (Task 2).
- **Review round 1, P2 (Task 4):** `status.jobs` has one `token-index` line for all providers, so a daemon run with `--provider fake-agent` counted as keeping Claude Code's index. `keeps_index` now also needs a job for the Tokens provider in `status.providers` (C-R12). Test first: `a_daemon_that_runs_another_provider_only_leaves_the_timer_on`.
- **Review round 1, P2 (Task 2):** the first collection followed the first frame that showed accounts, which comes before the first `status`, so the accounts of a provider the daemon collects for were collected too. It now waits until the TUI knows who collects (C-R20). Tests first, in the real order (the accounts and a frame, then `status`): `the_first_collection_waits_for_the_daemon_s_first_status`, `the_first_collection_goes_ahead_when_no_daemon_answers_or_after_2_s`.
- **Review round 1, P2 (Tasks 1, 2):** `stopping` only named its reason, so a daemon that sent it and then stalled with its socket open left the TUI live. It now ends the link at once, in the state machine and in the live thread (C-R18). Tests first: `stopping_ends_the_link_at_once_though_the_daemon_keeps_its_socket_open` (Task 1), `stopping_ends_the_link_at_once_and_names_its_reason_until_the_next_link_or_30_s` (Task 2).
- **Review round 1, the class (Tasks 1, 2):** the other ways to stay live without a working link were a daemon that hangs with its socket open (the live thread's read waits for ever), a `status` that fails or never comes (a lane stuck in `connect`), and a half-closed connection. Each now falls back to timer mode within a bound (C-R19): `Cmd::DropLink` ends the link through the daemon client's new `Events::closer`. Tests first: `end_link_ends_a_link_gone_quiet_and_the_thread_connects_again` and `a_daemon_that_closes_its_side_ends_the_link` (Task 1); `a_status_that_fails_or_never_comes_ends_the_link` (Task 2); the daemon's `a_closer_ends_a_stream_that_another_thread_is_reading`.
- **Review round 1, knock-on (Tasks 2, 4):** tests that mean a link to last now answer `status` as a working daemon does (`tick_answered`, `live_tick`), since a fetch left unanswered for 5 s ends the link. `the_first_status_starts_live_mode_and_a_reply_for_another_link_is_dropped` no longer has a failed fetch keep the last `status`.
- **Review round 2, P2 (Tasks 1, 2):** `Cmd::DropLink` ended whichever link the live thread held. A suspension can queue link 1's failed `status` before the live thread's `Down` and link 2's `Up`; on resume the UI, still on link 1, ended link 2. Each link now has a name, which `Up`, the `status` request and reply, and `DropLink` carry, and `end_link` closes only the link it names (C-R21). Tests first: `end_link_ends_only_the_link_it_names` (Task 1); `a_failure_for_a_link_the_live_thread_has_replaced_ends_that_link_alone`, and the exact order with a real live thread, `a_failure_judged_after_the_live_thread_replaced_its_link_leaves_the_new_link_alone` (Task 2). With `end_link` ignoring the name, the runtime test sees a third connection and the live test a `Down`.
- **Review round 2, P2 (Task 2):** a `status` reply cleared the 5 s bound while a later request on the same link was still out, so a request stuck in the lane's `connect` left the link live. One request is now out at a time on a link; one asked for meanwhile is sent when it is answered, with its own 5 s (C-R22). Test first: `one_status_is_out_at_a_time_and_the_one_out_is_given_up_on_5_s_after_it_was_asked`, which fails with round 1's fetch (`B waits for A`).
- **Review round 2, the class (Tasks 2, 3, 5):** the sweep of what else the UI applies by link (C-R21) found `Up`, `Down`, `Mismatch` and the events safe, being one thread's messages in one queue, and two things a link kept from the one before. A `poll` from an earlier link named the window of a later link's `switch` (Task 3: `a_switch_names_its_window_only_from_a_poll_of_its_own_link`). `P`'s fetch 2 s after the setting was dropped by a link change, though the new link's own fetch can read the daemon before it applies the setting (Task 5: `p_s_fetch_of_the_status_outlives_a_link_change`). Each test failed first.
- **Review round 3, P2 (Task 5):** `P`'s fetch was still lost when the link dropped before the setting's outcome arrived: `recheck` kept no deadline with no link up, and the next link's own fetch could read the daemon before it applied the setting. The deadline is now kept whatever the link; at it, `status` is fetched on whichever link is up, and a link that comes up only after it fetches once, at once, which serves (C-R21). Test first, both orders: `p_s_fetch_of_the_status_is_kept_when_its_outcome_comes_with_no_link_up` (`Down` → outcome → `Up`, the link up before the deadline and after it) beside `p_s_fetch_of_the_status_outlives_a_link_change` (outcome → `Down` → `Up`). It failed first at the fetch due 2 s after the outcome; with the deadline kept but a late link's fetch not counted, it failed at a second fetch.
- **Review round 2, knock-on:** every `Up` in the tests names its link, and `go_live` gives each its own. `a_status_that_fails_or_never_comes_ends_the_link` sends the `status` it fails.
- **The move onto `3575c3b`** (part B as executed and reviewed): Tasks 1 to 3 and 6 rebased without conflict. Two conflicts were context only, and every line the tasks add is as it was:
  - Task 4, `on_refreshed`: part B's `Busy` arm now also resets `busy_after`, beside Task 4's `watch.busy = !keeps`; and the test module's end, where both added tests.
  - Task 5, `HELP_GLOBAL`: part B's screens key is now `1 2 3  Tab Shift-Tab`, beside Task 5's `P` line. Part B's `every_help_line_shows_whole_on_every_screen_at_80_columns` passes with `P`'s line in the help.

  Part B's other changes need nothing of these tasks: the index check's `CHECK_FAILED`, `on_load`, the time triggers skipped while a report is pending (`token-index`'s debounced read is not one of them, and while a report is pending it asks for the follow-up B2 makes), the table, the filter, and Task 7's fixes. The code blocks moved with their context, and only the counts changed.
- **The move onto `941d245`:** the rebase met no conflict, and every code block generated from the rebased commits was byte for byte the one verified on `0ed49bc`. Part B's review changed none of the text these tasks replace, and only the counts moved.
- **Task 2 (found by Task 6's pty test of `P`):** the header said `● live · daemon idle` for a daemon whose first tick was still running. `status` is a snapshot the scheduler updates when a job's run ends (D5.4), after the run's events, so `mode`'s own fetch had read the board from before the run, and nothing fetched it again for a minute. Every event of a provider job's run now also fetches `status`, in the debounced read (C-R5).
- **Task 2:** an accessor for the link, used only by tests, failed clippy's dead-code lint, so the tests take the link from `go_live`.
- **Task 2:** the re-read a link asks for lands on the first tick 250 ms after it, so the tests of the 30 s timer tick at 250 ms first.
- **Task 3:** at 80 columns the tabs take 43 columns; `○ daemon stopping (upgrade)` and `● live · auto off (one account)` fit beside them only without `? help`, which the header now drops before it takes a second row (C-R9).
- **Task 5:** the toast's constant is `NO_DAEMON_RUNNING`, apart from Task 1's `live::NO_DAEMON` (the status fetch's error).

### Rulings

Part A's rulings R1–R22 and part B's B1–B29 stay in force. Part C's:

- **C-R1, neither the live thread nor the daemon lane is joined, and either dying is fatal.** Spec §4.3 and §10.2: they touch no store, so a process exit leaves nothing of theirs half written. The shutdown closes the daemon lane with the others and lets it go; a `status` it waits for may take 2 s (D5.7), and the shutdown does not wait for it. A panic on either is fatal, as a lane's is (§10.4): the binary's panic hook gives the terminal back on any thread, so the TUI could no longer draw. Both are named in the report as a joined lane that panicked is (`the TUI's daemon lane panicked; …`, `the TUI's live lane panicked; …`) (Tasks 1, 2).
- **C-R2, the live thread's backoff** (spec §8): after a failure, or a link that came down, it waits 1 s, then twice as long after each failure, up to 30 s. A link that stayed up 30 s starts the waits over; one the daemon drops at once keeps the longer waits, so a daemon that keeps dropping the subscriber is not reconnected to in a loop. After a mismatch it waits 60 s. It sends `Down` for each failed try, which the state machine takes as no news when it has no link already, and it returns once the TUI no longer listens (Task 1).
- **C-R3, live mode is a link with a `status` of its own.** A `status` reply carries the name of the link it was asked for (C-R21), and one for a link the TUI no longer follows is dropped, so a reply that crossed a disconnect never brings live mode back. A fetch that fails ends the link (C-R19) (Task 2).
- **C-R4, connecting reads the views 250 ms later.** Spec §8 asks, once subscribed, for `status` and for every view shown to be read again. `status` is fetched at once. The views (`accounts`; History's history; Tokens' report) are read with the debounce of C-R6, so the snapshot follows the subscription. An immediate read would also overtake the first frame's own read (§6.1): its generation would make that one stale, and the frame would show `loading…` (Task 2).
- **C-R5, `status` is fetched after a provider job's run.** The daemon updates `status` when a job's run ends (D5.4), after the run's events. A `mode` event fetches `status` at once, as §8 says, and can read the state from before the run: verification saw the header say `daemon idle` for a job whose first tick was still going. So every event of a provider job's run (auto's kinds and `usage`) also fetches `status`, in C-R6's debounced read, which comes after the run's last event. `changed` (another process's commit) reads the views only (Task 2).
- **C-R6, the debounce** (spec §6.3's 250 ms): an event's reads (the views, `status`, or both) are made 250 ms after the first event that asks for them, at the UI's next tick, so 250 to 500 ms after it, once for every event until then. A steady stream of events cannot postpone them (Task 2).
- **C-R7, the timers in live mode** (spec §6.2, §6.3):
  - the 30 s timer offers the collector only the shown accounts of providers that nothing else collects for, and no longer reads the views;
  - while linked, a 60 s timer fetches `status`, and in live mode reads `accounts`, and `history` while History is open;
  - `r` reads the views and offers every shown account to the collector, as `list` would;
  - the first collection after the first frame (R19) waits until the TUI knows who collects (C-R20), and leaves out the providers something else collects for too (spec §1.1 criterion 4).

  (Task 2.)
- **C-R8, what collects elsewhere** (spec §6.3): a provider whose job is `auto` or `collect`, or `waiting` for `engine-lock`. One that is `idle`, `waiting` for `schema`, or has no job in `status` (a daemon started with `--provider` for another), and every provider in timer mode, is collected for on the TUI's timer (Task 2).
- **C-R9, the header** (spec §5.1, §7.4, §8). In live mode, `● live · ` and §7.4's words for the provider the header follows (C-R10):
  - `auto on`, `auto paused`, `auto off (one account)`, `auto in pid N` (`auto in another process` without a pid), `daemon needs restart`, `daemon idle`; `daemon <mode>` for a mode this TUI does not know; `daemon idle` for a provider with no job in `status`;
  - `· keychain locked until HH:MM` with a backoff, in the system zone (B6).

  In timer mode: `○ refresh 30s`; `○ daemon stopping (<reason>)` after `stopping`, until a link comes up or 30 s pass (the backoff's ceiling: spec §8's "or gives up"); `○ daemon protocol N unsupported` until a try finds another daemon or none. The mode goes on the header's row when it fits; `? help` gives way first, and a mode that still does not fit takes a second row, right-aligned, so nothing is lost at 80 columns (R20) (Task 3).
- **C-R10, the provider the header follows and `P` acts on:** the selected account's, else `--provider`'s, else the default (spec §7.4) (Task 3).
- **C-R11, the activity line** (spec §5.1): one row above the footer, in live mode only, blank until the first event. It reads ` HH:MM <words>`: the event's `ts`, in the system zone, or the time it arrived for `usage` and `stopping`, whose typed form drops it. With several providers shown, the provider is named after the time. The words are auto's where it has them:
  - `auto: polled · 1 at 77%`;
  - `auto: 1 → 2 (5h at 95%)`, naming the window that bound the account at its tick's `poll` (as D6.1 finds it), else the trigger; `auto would switch: …` for a dry run;
  - `auto: no switch: <reason>` (`(<detail>)` when there is one); `auto: no switch: all-exhausted, the first back at HH:MM`;
  - `2 b@x.co quarantined (<reason>)`, `… unquarantined (<reason>)`;
  - `error: <message>`, `warning: <message>`;
  - `collected 1, 2 · not collected: 3 (http-429)`; `daemon stopping (<reason>)`; `token index updated`.

  `sleep`, `changed`, `mode` and a kind this TUI does not know leave it as it was (§8) (Task 3).
- **C-R12, a daemon that keeps the index current** (spec §6.4): live mode, with a job for the Tokens provider in `status.providers` (the index job serves the providers the daemon runs, D13; a daemon started with `--provider` for another keeps none of this one's index), and `token-index` among `status.jobs`, whose last run's `lastError` names neither the Tokens provider (`<provider>: …`) nor the store (`store…`). Then:
  - the 60 s refresh does not run; `r` on Tokens still does;
  - the first refresh after the first frame (B5) still runs, since whether the TUI is live is not known yet;
  - `Busy` is not news: no `catching up elsewhere`, and no 5 s checks (B10), since the daemon's events say when the index changes.

  A failing job leaves the TUI's timer on, so the TUI's own refresh shows why (B12). Without the link, the timer is back at once (Task 4).
- **C-R13, `token-index` events** (spec §6.4, §8): for the Tokens provider, while Tokens is open. A `changedAt` newer than the report's `changed_at_ms` reads the report again. It is to the second, so one in the report's own second counts as newer. The first reads at once; one within 5 s of it (`LIVE_REQUERY_MS`) waits, and the report is read once at the end of the 5 s, with every change since. B2's coalescing on the report lane still holds (Task 4).
- **C-R14, `P`** (spec §7.4):
  - a confirm dialog, `Pause auto-switch for <provider>?` or `Resume …?` from the effective `autoswitch` in `status`, default no as `remove`'s (§7.2);
  - a yes runs `Action::Autoswitch`: `--no-color config set provider.<id>.daemon.autoswitch false|true`, named `config`, under the busy rule;
  - in timer mode, and while the first `status` is still to come, the toast `no daemon running`; for a provider with no job in `status`, `the daemon runs no job for <provider>`;
  - `status` is fetched 2 s after the outcome, whatever its exit, whether or not a link is up when the outcome comes: on whichever link is up then, or, with none, by the next link's first fetch (C-R21).

  (Task 5.)
- **C-R15, the socket comes from the binary.** `TuiOptions::daemon` is `daemon_socket_path` of the probe engine's environment (D5.1), the store the lanes read. Tests pass `None`, for timer mode alone, or a fake `StatusFetch` (Task 2).
- **C-R16, the client's failures are not shown.** A failed connect shows as timer mode, and so does a `status` that fails, which ends the link (C-R19); the header says which mode the TUI is in. `tagteam-tui` logs nothing (Tasks 1, 2).
- **C-R17, the live thread's backlog is bounded, and a burst is one frame** (review round 1).
  - The live thread passes what it learns through `LiveQueue`, which holds 256 things at most (`LIVE_QUEUE`, the size of the daemon's own queue for a subscriber, D5.5). It wakes the loop with one `Msg::LiveReady` when the queue goes from empty to not, and the loop takes the queue whole.
  - A push into a full queue drops what the queue holds for one `Down`, and the live thread drops its link. The UI goes to timer mode, and the next link fetches `status` and reads every view again (C-R4): nothing is lost in meaning, only the activity line's history. So a suspension (§7.3), during which the loop takes no messages, holds 256 things and one wake at most, however long the session runs.
  - The loop applies every message waiting, up to 256 (`BATCH`), before it draws, each after its own look at the signal token (R17). A burst costs one frame, and a key behind it waits for none.

  (Tasks 1, 2.)
- **C-R18, `stopping` ends the link at once** (review round 1). The UI goes to timer mode and names the reason (C-R9). The live thread drops the link and connects again with backoff, whether or not the daemon goes on to close its socket (Tasks 1, 2).
- **C-R19, no link stays live without a working daemon** (review round 1).
  - A `status` that fails, or that has not come 5 s after it was sent (`STATUS_WAIT_MS`; the daemon lane's own bound is 2.25 s, D5.7), ends the link: timer mode, and `Cmd::DropLink { link }`, which shuts that connection down through `tagteam_daemon::client::Events::closer` if the live thread still holds it (C-R21). Its read returns, and it connects again with backoff.
  - The live thread's read waits for as long as the daemon is silent, since a subscribed connection carries nothing when nothing happens (D5.2). A daemon that hangs with its socket open is therefore found by the next `status`: within the 60 s timer (C-R7) and 5 s more.
  - A connection the daemon closes, or half-closes, ends the link at once.

  (Tasks 1, 2.)
- **C-R20, the first collection waits until the TUI knows who collects** (review round 1). The first frame never waits. A TUI that follows a daemon offers the collector nothing until one of these comes first: a try finds no daemon, or one of another protocol; the first `status` arrives; or 2 s pass (`SETTLE_MS`, within the daemon lane's bound). It then collects for the providers nothing else collects for (C-R8) (Task 2).
- **C-R21, each link has a name, and a verdict on one link acts on that link alone** (review rounds 2 and 3).
  - The live thread names each link it makes: 1 for its first, then 2 and on. `Live::Up { link }` carries the name. `Cmd::FetchStatus`, `DaemonReq::Status` and `Msg::DaemonStatus` carry the link the `status` is asked for. `Cmd::DropLink { link }` ends that link: `LiveQueue::end_link(link)` closes the connection only while the live thread still holds that link.
  - So a `status` that failed for link 1, applied after the live thread has replaced link 1 with link 2 (a suspension holds the messages back, §7.3), ends link 1, which is gone already. Link 2 carries on.
  - The sweep of what else the UI applies by link:
    - `Up`, `Down`, `Mismatch` and the events come from one thread through one queue, in order, so none can be stale beside another. A `Down` always ends the link of the last `Up` before it, since the live thread sends one before each new `Up`, and the UI takes no event while it follows no link. They need no name beyond `Up`'s.
    - `Msg::DaemonStatus` is the one message from another thread that acts on a link. It carries the link's name, and C-R22 orders it within a link.
    - What one link leaves to the next is decided thing by thing. A `poll` is not left: a `switch` names its window only from a `poll` of its own link, since a new link may follow missed events (Task 3). `P`'s fetch 2 s after the setting is written is left, and its deadline is kept whether or not a link is up when the outcome comes (review round 3). At the deadline it is made on whichever link is up, since a new link's own fetch can come before the daemon applies the setting (D4.2). With none up, the next link's first fetch comes after the deadline, and serves for it (Task 5).
    - The activity line keeps the last event seen, with its time, across links. A new link's events add to it; nothing stale replaces a newer one.

  (Tasks 1, 2, 3, 5.)
- **C-R22, one `status` out at a time on a link** (review round 2).
  - A `status` asked for while another is out is not sent then. It is sent once the one out is answered, so that it reads what came after it was asked; the asks made meanwhile share it.
  - C-R19's 5 s bound is the one out's, from when it was sent. A reply answers the one request out, so it never clears another's bound.
  - Chosen over a name for each request: the daemon lane answers in order, one at a time, so one request out needs no more than its deadline and a flag, and the lane is never asked twice for the same snapshot. The lane still coalesces what waits on it across links.

  (Task 2.)

## Global Constraints

- **Start-up** (spec §1.1 criterion 2): the first frame within 50 ms p95, whatever the daemon does. The live thread connects beside the lanes, and the TUI starts in timer mode (§8). Task 6 measures it beside a running daemon and beside a socket that accepts and never answers.
- **The UI thread waits on nothing but the terminal** (spec §1.1 criterion 3): `connect`, `subscribe`, the events and `status` happen on the live thread and the daemon lane.
- **The socket is read-only** (D5.3, T3). The TUI writes `daemon.autoswitch` only through `config set`, on the actions lane (spec §7.4).
- **`status` is the authority on the daemon's state** (spec §8). An event's fields never reach the header; they reach only the activity line.
- **Usage requests** (spec §1.1 criterion 4): with a daemon that collects for a provider, the TUI fetches for it only on `r` or an action's CLI command.
- **Bounded work under a stream of events:** the views and `status` once per 250 ms debounce (C-R6); the token report once per 5 s (C-R13) and never more than one query on the report lane at a time (B2); one `status` out per link (C-R22), and the daemon lane coalesces what waits on it.
- **A bounded backlog** (C-R17): at most 256 of the live thread's messages wait, whatever the UI does, and a burst is drawn once.
- **Live mode only with a working daemon** (C-R18, C-R19): `stopping`, a closed connection, and a `status` that fails or does not come within 5 s each put the TUI in timer mode and start a reconnection. What the UI decides about a link acts on that link alone (C-R21).
- **Shutdown** (spec §10.2): the engine lanes are joined as before; the daemon lane is closed and let go, and the live thread is let go (C-R1).
- **Timer mode is unchanged** without a daemon: part A's and part B's tests pass as they are, but for the lines this plan names.

## Review Focus

The five failures most likely in real use, each with the tests that own it:

1. **A daemon that restarts under the TUI** (an upgrade's exit 75, a `launchctl` restart, a crash). `stopping` names its reason and ends the link at once, even if the daemon stalls before it closes; the TUI reconnects with backoff; the new link fetches `status` and reads every view again; a reply from the old link is dropped, a failure judged of the old link never ends the new one, and what the old link left (a `poll`, `P`'s fetch) neither misleads nor is lost. Owners:
   - Task 1: `stopping_comes_through_then_the_link_goes_down_and_comes_back_with_the_next_daemon`, `stopping_ends_the_link_at_once_though_the_daemon_keeps_its_socket_open`, `end_link_ends_only_the_link_it_names`;
   - Task 2: `a_link_fetches_the_status_at_once_and_reads_every_view_shown_250_ms_later`, `a_link_that_goes_down_is_timer_mode_and_a_late_reply_does_not_undo_that`, `stopping_ends_the_link_at_once_and_names_its_reason_until_the_next_link_or_30_s`, `a_failure_for_a_link_the_live_thread_has_replaced_ends_that_link_alone`, `a_failure_judged_after_the_live_thread_replaced_its_link_leaves_the_new_link_alone`;
   - Task 3: `a_switch_names_its_window_only_from_a_poll_of_its_own_link`;
   - Task 5: `p_s_fetch_of_the_status_outlives_a_link_change`, `p_s_fetch_of_the_status_is_kept_when_its_outcome_comes_with_no_link_up`;
   - Task 6: `a_daemon_that_stops_leaves_the_tui_on_its_timer_until_the_next_one_answers`.
2. **A protocol mismatch** between an upgraded binary and a daemon not yet restarted. The TUI says so, stays in timer mode, and tries again every 60 s, never in a loop. Owners:
   - Task 1: `a_daemon_of_another_protocol_is_a_mismatch_tried_again_after_its_own_wait`, `status_with_no_daemon_or_another_protocol_fails_at_once`;
   - Task 2: `a_mismatch_is_kept_until_a_try_finds_another_daemon_or_none`;
   - Task 3: `timer_mode_says_when_the_daemon_stopped_or_speaks_another_protocol`.
3. **An event storm:** a cold catch-up's `token-index` about once a second, a burst of account events, a flood while `R`'s session has the terminal, a subscriber the daemon drops for falling behind. The live thread's backlog stays within 256; a burst is drawn once and a key behind it waits for no frame; the report lane is asked at most every 5 s and the last change is read; the views and `status` once per 250 ms; a dropped subscriber reconnects less and less often. Owners:
   - Task 1: `a_full_queue_takes_one_down_for_what_it_held`, `a_flood_nobody_takes_stays_within_the_queue_and_the_link_is_dropped`, `a_subscriber_the_daemon_drops_is_down_and_reconnects_less_often_while_it_keeps_dropping`;
   - Task 2: `a_flood_during_a_suspension_stays_within_the_queue_and_a_snapshot_recovers_it`, `a_key_queued_behind_a_burst_is_handled_within_one_frame`, `an_event_reads_what_it_affects_250_ms_later_once_for_every_event_until_then`;
   - Task 4: `a_catch_up_s_stream_of_events_reads_the_report_every_5_s_and_reads_the_last_change`.
4. **A stalled socket:** a daemon that accepts and never greets, never answers `status`, hangs with its socket open, or half-closes it. The first frame never waits for it; a `status` is given up on after 2 s, and one that fails or never comes ends the link, though an earlier one was answered; a half-closed connection ends it at once; `q` never waits for the daemon lane. Owners:
   - Task 1: `a_daemon_that_never_answers_status_is_given_up_on_after_2_s`, `end_link_ends_a_link_gone_quiet_and_the_thread_connects_again`, `a_daemon_that_closes_its_side_ends_the_link`;
   - Task 2: `the_first_frame_never_waits_for_the_daemon_and_its_status_starts_live_mode`, `a_status_that_fails_or_never_comes_ends_the_link`, `one_status_is_out_at_a_time_and_the_one_out_is_given_up_on_5_s_after_it_was_asked`, `q_never_waits_for_a_status_the_daemon_lane_still_waits_for`;
   - Task 6: `tui_first_frame_p95_is_within_50_ms_beside_a_daemon_that_never_answers`.
5. **A provider whose daemon job is `waiting`** (`engine-lock`: a `tagteam auto` in a terminal; `schema`: the daemon needs a restart), `idle`, or absent (a daemon started with `--provider` for another). The TUI collects for exactly the providers §6.3 names, and not before it knows which they are; it refreshes the index of a provider the daemon does not run; it says the state in §7.4's words; and `P` does not write a setting it cannot show. Owners:
   - Task 2: `the_timer_collects_only_for_the_providers_nothing_else_collects_for`, `the_first_collection_waits_for_the_daemon_s_first_status`, `the_first_collection_goes_ahead_when_no_daemon_answers_or_after_2_s`;
   - Task 4: `a_daemon_that_runs_another_provider_only_leaves_the_timer_on`;
   - Task 3: `each_daemon_state_of_spec_7_4_has_its_words_with_and_without_a_keychain_backoff`, `with_several_providers_the_header_follows_the_selected_account_s`;
   - Task 5: `p_without_a_daemon_says_so_and_changes_nothing`.

## Interface Contract

Every task's names, signatures and strings must match this section exactly. Part A's contract (C1–C9) and part B's (CB1–CB7) hold, amended as below.

### CC1. The live link (Task 1)

`crates/tagteam-tui/src/live.rs`, `pub mod live`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Live {
    Up { link: u64 },            // subscribed: the thread's link `link`, from 1 (C-R21)
    Down,                        // no link: none came up, or it ended
    Mismatch { server: u32 },    // the daemon speaks another protocol (D5.6)
    Event(tagteam_daemon::protocol::Event),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff { pub first: Duration, pub most: Duration, pub mismatch: Duration } // Default: 1 s, 30 s, 60 s
pub const NO_DAEMON: &str = "no daemon answers";
pub const LIVE_QUEUE: usize = 256;                   // C-R17
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery { Taken, Full, Gone }              // Full: the queue now holds one Down
pub struct LiveQueue { /* private */ }
impl LiveQueue {
    pub fn new(cap: usize, wake: impl Fn() -> bool + Send + Sync + 'static) -> LiveQueue;
    pub fn push(&self, live: Live) -> Delivery;      // wakes when it goes from empty to not
    pub fn take(&self) -> Vec<Live>;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn end_link(&self, link: u64);               // C-R19, C-R21: only while it holds `link`
}
pub fn spawn(
    path: PathBuf,
    backoff: Backoff,
    queue: Arc<LiveQueue>,
    on_death: impl FnOnce() + Send + 'static,        // runs if the thread panics
) -> io::Result<JoinHandle<()>>;                     // thread "tagteam-tui-live"
pub fn fetch_status(path: &Path) -> Result<tagteam_daemon::protocol::Status, String>;
```

`tagteam_daemon::client` gains, additively (C-R19):

```rust
impl Events { pub fn closer(&self) -> io::Result<Closer>; }
#[derive(Debug)]
pub struct Closer(/* private */);
impl Closer { pub fn close(&self); }                 // the iterator then ends
```

### CC2. Messages, commands and requests (Task 2)

`msg.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonReq { Status { link: u64 } }         // any two coalesce

// Msg gains:
Live(Live),
LiveReady,                                           // the runtime takes the live queue
DaemonStatus { link: u64, result: Result<Status, String> },

// Cmd gains:
FetchStatus { link: u64 },                           // one out per link (C-R22)
DropLink { link: u64 },                              // C-R19, C-R21
```

### CC3. Live mode: `daemon` (Tasks 2–5)

`crates/tagteam-tui/src/daemon/mod.rs`, `pub mod daemon`:

```rust
pub mod words;                                       // Task 3
pub const REREAD_MS: i64 = 250;
pub const POLL_MS: i64 = 60_000;
pub const STOPPING_MS: i64 = 30_000;
pub const STATUS_WAIT_MS: i64 = 5_000;               // C-R19
pub const SETTLE_MS: i64 = 2_000;                    // C-R20
pub const RECHECK_MS: i64 = 2_000;                   // Task 5
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity { pub at_ms: i64, pub provider: Option<String>, pub text: String } // Task 3
#[derive(Default)]
pub struct DaemonState { /* private */ }
impl DaemonState {
    pub fn live(&self) -> bool;                      // subscribed, with this link's status
    pub fn status(&self) -> Option<&Status>;         // in live mode
    pub fn job(&self, provider: &ProviderId) -> Option<&StatusProvider>;
    pub fn stopping(&self) -> Option<&str>;
    pub fn mismatch(&self) -> Option<u32>;
    pub fn settled(&self) -> bool;                   // C-R20
    pub(crate) fn await_daemon(&mut self, now_ms: i64);
    pub fn activity(&self) -> Option<&Activity>;     // Task 3
}
pub(crate) fn focus(state: &State) -> ProviderId;    // Task 3: C-R10
pub(crate) fn on_live(state: &mut State, live: Live, now_ms: i64) -> Vec<Cmd>;
pub(crate) fn on_status(state: &mut State, link: u64, result: Result<Status, String>, now_ms: i64)
    -> Vec<Cmd>;
pub(crate) fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd>;
pub(crate) fn collected_elsewhere(state: &State, provider: &ProviderId) -> bool;
pub(crate) fn to_collect(state: &State) -> Vec<AccountId>;
pub(crate) fn keeps_index(state: &State) -> bool;    // Task 4: C-R12
pub(crate) fn recheck(state: &mut State, now_ms: i64); // Task 5
```

`State` gains `pub daemon: DaemonState`. `update` sends `Msg::Live` to `on_live` and `Msg::DaemonStatus` to `on_status`, calls `on_tick` on every tick, and offers the collector `to_collect` on the 30 s timer and, once `settled`, after the first frame.

`crates/tagteam-tui/src/daemon/words.rs` (Task 3):

```rust
pub fn mode(state: &State) -> String;                           // the header's mode (C-R9)
pub fn job_words(job: &StatusProvider, tz: &TimeZone) -> String;  // §7.4's table
pub(crate) fn iso_ms(iso: &str) -> Option<i64>;
pub(crate) fn clock(ms: i64, tz: &TimeZone) -> String;           // HH:MM
pub(crate) fn activity(event: &Event, poll: Option<&Value>, tz: &TimeZone) -> Option<String>;
```

### CC4. The view and the Tokens screen (Tasks 3, 4)

- `view.rs`: `Areas` gains `activity: Option<Rect>`. The header is one row, or two when its mode does not fit beside the tabs (C-R9). `MODE_TIMER` is unchanged.
- `TokensState` gains `pub fn tz(&self) -> &TimeZone` (Task 3).
- `screens/tokens/index.rs` (Task 4):

  ```rust
  pub const LIVE_REQUERY_MS: i64 = 5_000;
  pub(crate) fn on_daemon_index(state: &mut State);        // a `Busy` is no longer news
  pub(crate) fn on_token_index(state: &mut State, provider: &str, raw: &Value, now_ms: i64) -> Vec<Cmd>;
  ```

### CC5. `P` (Task 5)

- `host.rs`: `Action::Autoswitch { provider: ProviderId, on: bool }`; `args()` is `["--no-color", "config", "set", "provider.<id>.daemon.autoswitch", "true"|"false"]`; `name()` is `"config"`; it does not need the terminal.
- `actions.rs`: `pub const NO_DAEMON_RUNNING: &str = "no daemon running";`. `P` is a global action key. The help lists `P  pause or resume auto-switch` among the global keys.

### CC6. The runtime and the binary (Task 2)

```rust
pub struct TuiOptions {
    // part B's fields, and:
    pub daemon: Option<PathBuf>,   // the daemon's socket (C-R15); None: timer mode alone
}
pub(crate) type StatusFetch = Arc<dyn Fn() -> Result<Status, String> + Send + Sync>;
impl Lanes {
    pub(crate) fn spawn(provider: Option<ProviderId>, sources: &SourceFactory, host: &Arc<dyn Host>,
        tx: &Sender<Msg>, shutdown: &Cancel, status: Option<StatusFetch>) -> Lanes;
    pub(crate) fn stray_death(&self, name: &'static str, tx: &Sender<Msg>) -> impl FnOnce() + Send + 'static;
}
// Lanes gains `pub(crate) live: Arc<LiveQueue>`, whose wake sends `Msg::LiveReady`.
const BATCH: usize = 256;                            // C-R17: messages a wake applies
```

`run` spawns the daemon lane with `live::fetch_status` on `opts.daemon`, and the live thread on it with `Backoff::default()` and the lanes' live queue. `Ui::new` makes a TUI that follows a daemon await it (C-R20). The loop applies a `LiveReady` as everything in the live queue, and `Cmd::DropLink { link }` as `LiveQueue::end_link(link)`. `tui_cmd.rs` sets `daemon: Some(daemon_socket_path(probe.env()))`.

## Tasks

| # | Task | Depends on |
|---|---|---|
| 1 | The live thread and the status fetch | — |
| 2 | Live mode: the link, the daemon lane, and what each event reads | 1 |
| 3 | The header's daemon state and the activity line | 2 |
| 4 | The token index in live mode | 3 |
| 5 | `P`: pause and resume auto-switch | 4 |
| 6 | End to end beside a real daemon, timing, and docs | 5 |

Every task runs alone, in table order. No pair qualifies as a batch: Tasks 2 to 5 all change `daemon/mod.rs`, each replacing text the one before left, and every pair shares `tagteam-tui`'s library.

### Task 1: The live thread and the status fetch

The TUI's side of the daemon's socket (spec §8), in `live.rs`, over `tagteam_daemon::client` (D5.7):

- **The live thread** (`spawn`) connects, subscribes, and passes on `Live::Up` with the link's name, then each event as it arrives, then `Live::Down` once the link ends. It tries again with C-R2's backoff, and every 60 s after a protocol mismatch. It returns once the TUI no longer listens, and runs `on_death` if it panics (C-R1).
- **`LiveQueue`** carries what it passes on to the UI thread, 256 things at most (C-R17). A push into a full queue keeps one `Down`, and the live thread drops its link.
- **The link ends at once** at `stopping` (C-R18), when the daemon closes or half-closes its side, and when the UI calls `end_link` with its name (C-R19, C-R21), which shuts the connection down through the daemon client's new `Events::closer`. A link the thread has since replaced is not ended twice: its successor carries on.
- **`fetch_status`** is what the daemon lane will run (Task 2): a connection of its own, its greeting, and `status`. A daemon that never answers is given up on after the client's 2 s.

Neither touches the store. The tests run against the daemon's own socket server, `tagteam_daemon::server::Server` over a `StatusBoard`, which emits events through its hub. Fake daemons on Unix sockets cover what the real one does only across versions, under load or when it hangs: a protocol it does not speak, a subscriber dropped for falling behind (D5.5 shuts its socket down), a `stopping` with the socket left open, a half-closed connection, a link gone quiet, a `status` that never comes.

**Readings of the spec this task commits to:**
- **A dropped subscriber is a disconnect** (spec §8: "an error, `stopping`, or a full queue"). The real daemon drops one by shutting its socket down; the fake does the same, after a hold. A client that reads as fast as the live thread does never fills its queue, so a full queue is tested as the daemon ends it.
- **`Down` is sent for every failed try,** not only for a link that ended. The state machine (Task 2) treats a second `Down` as nothing.
- **The tests live in `tests/live.rs`,** an integration test of their own. Binding the daemon's socket narrows the process's umask for an instant (D5.1); a separate test binary keeps that away from the library's tests.
- **`Events::closer` is additive** to `tagteam_daemon::client`: a cloned handle to the stream's socket, shut down from another thread. The daemon's own test shows that it ends an iterator another thread is blocked in.
- **A link's name is the thread's count of the links it made,** 1 for the first. The queue holds the current link's closer under that name, so `end_link` can tell the link the UI judged from the one the thread holds now (C-R21).

**Depends on:** —

**Files:**
- Modify: `crates/tagteam-daemon/tests/client.rs`
- Create: `crates/tagteam-tui/tests/live.rs`
- Modify: `crates/tagteam-daemon/src/client.rs` (`Events::closer`)
- Modify: `Cargo.lock` (the build adds `tagteam-daemon` to `tagteam-tui`'s entry)
- Modify: `crates/tagteam-tui/Cargo.toml`
- Modify: `crates/tagteam-tui/src/lib.rs`
- Create: `crates/tagteam-tui/src/live.rs`

**Interfaces:**
- Consumes: `tagteam_daemon::client::{connect_at, Connect, Events}`, `tagteam_daemon::protocol::{Event, Status}`; in the tests, `tagteam_daemon::server::Server`, `tagteam_daemon::status::{StatusBoard, DaemonInfo, NotifyInfo, ProviderStatus}`, `tagteam_daemon::events::{DaemonEvent, DaemonSink, Mode, StopReason}`.
- Produces: CC1, with `tagteam_daemon::client::{Events::closer, Closer}`, which Task 2's runtime and messages use.

**Spec:**
- TUI spec §4.1 (`tagteam-tui` depends on `tagteam-daemon`), §4.3 (the live thread reads the socket and forwards, never touches the store, is not joined), §8 ("Connecting", "Disconnect", "Version mismatch"; `status` over a short-lived connection of its own), §11 ("Daemon client": `NoDaemon`, `Connected` and `Mismatch`; disconnect, `stopping` and a full queue).
- Daemon spec D5.1 (the path; a stale socket fails at once), D5.2 (the greeting; a subscribed connection is never closed for idling), D5.5 (a slow subscriber is disconnected; `stopping` is the last event), D5.6, D5.7 (250 ms for the greeting, 2 s for `status`).

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-daemon/tests/client.rs`, replace:

```rust
        ]
    );
    daemon.join().unwrap();
}
```

with:

```rust
        ]
    );
    daemon.join().unwrap();
}

#[test]
fn a_closer_ends_a_stream_that_another_thread_is_reading() {
    // A subscriber that finds the daemon no longer answers (the TUI's live thread, whose read
    // waits for as long as the daemon is silent) ends its stream from another thread.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let server = serve(&path);
    let events = connected(connect_at(&path)).subscribe().unwrap();
    let closer = events.closer().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(events.count());
    });
    thread::sleep(Duration::from_millis(100));
    assert!(
        rx.try_recv().is_err(),
        "the reader waits on a silent daemon"
    );
    closer.close();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)),
        Ok(0),
        "the iterator ended, with no event"
    );
    server.shutdown();
}
```

Create `crates/tagteam-tui/tests/live.rs`:

```rust
//! The live thread and the status fetch (spec §8, §11 "Daemon client") against the daemon's own
//! socket server (`tagteam_daemon::server`), and against fake daemons for what the real one
//! does only under load, across versions, or when it hangs: a protocol it does not speak, a
//! dropped subscriber, a `stopping` with the socket left open, a link gone quiet, a `status`
//! that never comes. Run outside a sandbox that refuses `bind` on Unix sockets.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tagteam_core::ProviderId;
use tagteam_daemon::events::{DaemonEvent, DaemonSink, Mode, StopReason};
use tagteam_daemon::protocol::Event;
use tagteam_daemon::server::Server;
use tagteam_daemon::status::{DaemonInfo, NotifyInfo, ProviderStatus, StatusBoard};
use tagteam_engine::tokens::IndexState;
use tagteam_provider::Cancel;
use tagteam_tui::live::{Backoff, Delivery, Live, LiveQueue, NO_DAEMON, fetch_status, spawn};

/// How long anything here may take before it counts as stuck.
const WAIT: Duration = Duration::from_secs(10);
/// 2026-09-21T14:13:20Z.
const T: i64 = 1_790_000_000;

/// A temporary directory short enough for a socket path under it (the daemon's D14): made
/// under `/tmp`, else `$TMPDIR`, and made 0700 after, since a bind elsewhere in this binary
/// may have narrowed the umask at that instant.
fn short_tempdir() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("tt")
        .tempdir_in("/tmp")
        .or_else(|_| tempfile::Builder::new().prefix("tt").tempdir())
        .expect("a temporary directory under /tmp or $TMPDIR");
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

/// A board for a daemon that runs Claude Code's job in `mode`, with `autoswitch`.
fn board(mode: Mode, autoswitch: bool) -> Arc<StatusBoard> {
    let board = StatusBoard::new(
        DaemonInfo {
            pid: 4242,
            version: "20261014.2045.0".into(),
            started_at: T,
            socket: None,
            log: PathBuf::from("/Users/tester/.local/state/tagteam/tagteam.log"),
        },
        NotifyInfo {
            enabled: false,
            backend: None,
            events: vec![],
        },
    );
    board.set_provider(ProviderStatus {
        provider: ProviderId::new("claude-code"),
        mode,
        autoswitch,
        waiting_for: None,
        holder_pid: None,
        backoff: None,
        last_tick_at: Some(T),
        next_tick_at: Some(T + 300),
        last_outcome: None,
        last_switch: None,
    });
    board
}

fn serve(path: &Path) -> Server {
    Server::bind(
        path,
        Path::new("/Users/tester"),
        board(Mode::Auto, true),
        Cancel::new(),
    )
    .unwrap()
}

/// Backoffs short enough for a test.
fn quick(first_ms: u64, most_ms: u64, mismatch_ms: u64) -> Backoff {
    Backoff {
        first: Duration::from_millis(first_ms),
        most: Duration::from_millis(most_ms),
        mismatch: Duration::from_millis(mismatch_ms),
    }
}

/// The live thread on `path`, with a reader that takes its queue at each wake, as the UI
/// thread does, and passes on each thing it took with when it took it.
fn live(path: &Path, backoff: Backoff) -> (Receiver<(Instant, Live)>, Arc<LiveQueue>) {
    let (wake, woken) = mpsc::channel();
    let queue = Arc::new(LiveQueue::new(256, move || wake.send(()).is_ok()));
    spawn(path.to_path_buf(), backoff, Arc::clone(&queue), || {}).unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = Arc::clone(&queue);
    thread::spawn(move || {
        while woken.recv().is_ok() {
            let at = Instant::now();
            for l in reader.take() {
                if tx.send((at, l)).is_err() {
                    return;
                }
            }
        }
    });
    (rx, queue)
}

/// The next thing the live thread passed on.
fn next(rx: &Receiver<(Instant, Live)>) -> (Instant, Live) {
    rx.recv_timeout(WAIT)
        .expect("the live thread passed something on")
}

/// Skips `Down`s until the link comes up again.
fn up_again(rx: &Receiver<(Instant, Live)>, what: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        match next(rx).1 {
            Live::Up { .. } => return,
            Live::Down => assert!(Instant::now() < deadline, "never came up: {what}"),
            other => panic!("{what}: {other:?}"),
        }
    }
}

/// A fake daemon at `path` that serves each connection it accepts with `script`, one at a
/// time, until the test ends.
fn fake(path: &Path, script: impl Fn(UnixStream) + Send + 'static) {
    let listener = UnixListener::bind(path).unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            stream.set_read_timeout(Some(WAIT)).unwrap();
            script(stream);
        }
    });
}

fn hello(protocol: u32) -> String {
    format!(
        "{{\"schemaVersion\":1,\"event\":\"hello\",\"protocol\":{protocol},\"version\":\"20261014.2045.0\",\"pid\":4242,\"startedAt\":\"2026-09-21T14:13:20Z\"}}\n"
    )
}

/// The request a fake daemon reads next, as text.
fn request(stream: &UnixStream) -> String {
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    line
}

/// A fake daemon's greeting and its `ok` to the client's subscription (the client's first
/// request has id 1).
fn subscribed(stream: &mut UnixStream) {
    stream.write_all(hello(1).as_bytes()).unwrap();
    request(stream);
    let ok = "{\"schemaVersion\":1,\"id\":1,\"ok\":true,\"result\":{}}\n";
    stream.write_all(ok.as_bytes()).unwrap();
}

#[test]
fn a_daemon_brings_the_link_up_and_its_events_through_in_order() {
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let server = serve(&path);
    let (rx, _queue) = live(&path, Backoff::default());
    assert_eq!(next(&rx).1, Live::Up { link: 1 });
    let cc = ProviderId::new("claude-code");
    server.hub().emit(&DaemonEvent::Changed);
    server.hub().emit(&DaemonEvent::Usage {
        provider: cc.clone(),
        updated: vec![1],
        fetch_errors: Default::default(),
    });
    server.hub().emit(&DaemonEvent::TokenIndex {
        provider: cc,
        state: IndexState {
            changed_at_ms: Some(T * 1000),
            ..IndexState::default()
        },
    });
    assert_eq!(next(&rx).1, Live::Event(Event::Changed));
    assert!(
        matches!(next(&rx).1, Live::Event(Event::Usage { updated, .. }) if updated == [1]),
        "the usage event second"
    );
    match next(&rx).1 {
        Live::Event(Event::TokenIndex { provider, raw }) => {
            assert_eq!(provider, "claude-code");
            assert_eq!(raw["changedAt"], "2026-09-21T14:13:20Z");
        }
        other => panic!("{other:?}"),
    }
    server.shutdown();
}

#[test]
fn with_no_daemon_it_says_so_and_tries_again_less_and_less_often_until_one_answers() {
    // Spec §8: backoff from `first` up to `most`, never a busy loop.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let (rx, _queue) = live(&path, quick(20, 160, 1_000));
    let started = Instant::now();
    let mut downs = Vec::new();
    while started.elapsed() < Duration::from_millis(800) {
        let (at, l) = next(&rx);
        assert_eq!(l, Live::Down);
        downs.push(at);
    }
    let gaps: Vec<u128> = downs
        .windows(2)
        .map(|w| (w[1] - w[0]).as_millis())
        .collect();
    assert!(
        (6..=10).contains(&downs.len()),
        "20, 40, 80, then 160 ms apart: {gaps:?}"
    );
    for (gap, least) in gaps.iter().zip([20, 40, 80, 160, 160, 160, 160]) {
        assert!(*gap >= least, "{gaps:?}");
    }
    let server = serve(&path);
    up_again(&rx, "the daemon that came");
    server.shutdown();
}

#[test]
fn a_daemon_of_another_protocol_is_a_mismatch_tried_again_after_its_own_wait() {
    // Spec §8, D5.6: in case the daemon restarts on this version.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    fake(&path, |mut stream| {
        stream.write_all(hello(2).as_bytes()).unwrap();
    });
    let (rx, _queue) = live(&path, quick(10, 20, 150));
    let (first, said) = next(&rx);
    assert_eq!(said, Live::Mismatch { server: 2 });
    let (second, said) = next(&rx);
    assert_eq!(said, Live::Mismatch { server: 2 }, "never Down in between");
    assert!(second - first >= Duration::from_millis(150));
}

#[test]
fn stopping_comes_through_then_the_link_goes_down_and_comes_back_with_the_next_daemon() {
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let server = serve(&path);
    let (rx, _queue) = live(&path, quick(20, 1_000, 1_000));
    assert_eq!(next(&rx).1, Live::Up { link: 1 });
    server.hub().emit(&DaemonEvent::Stopping {
        reason: StopReason::Upgrade,
    });
    server.shutdown();
    assert_eq!(
        next(&rx).1,
        Live::Event(Event::Stopping {
            reason: "upgrade".into()
        })
    );
    assert_eq!(next(&rx).1, Live::Down);
    let restarted = serve(&path);
    up_again(&rx, "the next daemon");
    restarted.shutdown();
}

#[test]
fn stopping_ends_the_link_at_once_though_the_daemon_keeps_its_socket_open() {
    // C-R18: a daemon that says `stopping` and then stalls before it closes is not waited for.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    fake(&path, |mut stream| {
        subscribed(&mut stream);
        let stopping = "{\"schemaVersion\":1,\"event\":\"stopping\",\"ts\":\"2026-09-21T14:13:20Z\",\"reason\":\"signal\"}\n";
        stream.write_all(stopping.as_bytes()).unwrap();
        thread::sleep(Duration::from_secs(5));
    });
    let (rx, _queue) = live(&path, quick(50, 100, 1_000));
    assert_eq!(next(&rx).1, Live::Up { link: 1 });
    let (said, stopping) = next(&rx);
    assert_eq!(
        stopping,
        Live::Event(Event::Stopping {
            reason: "signal".into()
        })
    );
    let (down, l) = next(&rx);
    assert_eq!(l, Live::Down);
    assert!(down - said < Duration::from_secs(1), "{:?}", down - said);
}

#[test]
fn a_daemon_that_closes_its_side_ends_the_link() {
    // A half-closed connection: the daemon writes no more, and the link is down at once.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    fake(&path, |mut stream| {
        subscribed(&mut stream);
        stream.shutdown(Shutdown::Write).unwrap();
        thread::sleep(Duration::from_secs(5));
    });
    let (rx, _queue) = live(&path, quick(50, 100, 1_000));
    let (up, l) = next(&rx);
    assert_eq!(l, Live::Up { link: 1 });
    let (down, l) = next(&rx);
    assert_eq!(l, Live::Down);
    assert!(down - up < Duration::from_secs(1), "{:?}", down - up);
}

#[test]
fn end_link_ends_a_link_gone_quiet_and_the_thread_connects_again() {
    // C-R19: the live thread's read waits for as long as the daemon is silent; a daemon that
    // hangs with its socket open is found out by `status` (Task 2), which ends the link here.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    fake(&path, |mut stream| {
        subscribed(&mut stream);
        thread::sleep(Duration::from_secs(5));
    });
    let (rx, queue) = live(&path, quick(50, 100, 1_000));
    assert_eq!(next(&rx).1, Live::Up { link: 1 });
    thread::sleep(Duration::from_millis(100));
    assert!(rx.try_recv().is_err(), "silent, and still up");
    let ended = Instant::now();
    queue.end_link(1);
    let (down, l) = next(&rx);
    assert_eq!(l, Live::Down);
    assert!(down - ended < Duration::from_secs(1), "{:?}", down - ended);
    queue.end_link(1);
}

/// A fake daemon that takes each subscription, then drops the subscriber after the next of
/// `holds` (popped from its end), as the real one drops a subscriber whose queue is full
/// (D5.5: it shuts the socket down).
fn dropping(path: &Path, holds: Vec<Duration>) {
    let holds = Mutex::new(holds);
    fake(path, move |mut stream| {
        subscribed(&mut stream);
        let hold = holds.lock().unwrap().pop().unwrap_or_default();
        thread::sleep(hold);
        let _ = stream.shutdown(Shutdown::Both);
    });
}

#[test]
fn end_link_ends_only_the_link_it_names() {
    // C-R21: the UI may judge a link that the live thread has already replaced (its `status`
    // failed while a suspension held the messages back). Ending that link leaves the new one.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    // The first link is dropped at once; the second holds.
    dropping(&path, vec![Duration::from_secs(5), Duration::ZERO]);
    let (rx, queue) = live(&path, quick(20, 100, 1_000));
    assert_eq!(next(&rx).1, Live::Up { link: 1 });
    assert_eq!(next(&rx).1, Live::Down);
    assert_eq!(next(&rx).1, Live::Up { link: 2 });
    queue.end_link(1);
    thread::sleep(Duration::from_millis(300));
    assert!(rx.try_recv().is_err(), "the second link carries on");
    queue.end_link(2);
    assert_eq!(next(&rx).1, Live::Down);
}

#[test]
fn a_subscriber_the_daemon_drops_is_down_and_reconnects_less_often_while_it_keeps_dropping() {
    // Spec §8: a full queue is a disconnect, and the TUI reconnects with backoff. Links dropped
    // at once double the wait; one that lasted `most` starts the waits over.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let at_once = Duration::ZERO;
    // The fourth link lasts 250 ms; the others are dropped at once.
    dropping(
        &path,
        vec![
            at_once,
            Duration::from_millis(250),
            at_once,
            at_once,
            at_once,
        ],
    );
    let (rx, _queue) = live(&path, quick(30, 200, 1_000));
    let (mut ups, mut downs) = (Vec::new(), Vec::new());
    while ups.len() < 5 {
        match next(&rx) {
            (at, Live::Up { .. }) => ups.push(at),
            (at, Live::Down) => downs.push(at),
            other => panic!("{other:?}"),
        }
    }
    assert!(
        downs[3] - ups[3] >= Duration::from_millis(250),
        "the fourth lasted"
    );
    let waits: Vec<u128> = (1..5)
        .map(|i| (ups[i] - downs[i - 1]).as_millis())
        .collect();
    assert!(
        waits[0] >= 30 && waits[1] >= 60 && waits[2] >= 120,
        "doubling: {waits:?}"
    );
    assert!(
        waits[3] >= 30 && waits[3] < waits[2],
        "after the link that lasted, the first wait again: {waits:?}"
    );
}

#[test]
fn a_full_queue_takes_one_down_for_what_it_held() {
    // C-R17: at most `cap` wait; a full queue keeps a single `Down`, and says so.
    let woken = Arc::new(Mutex::new(0));
    let count = Arc::clone(&woken);
    let queue = LiveQueue::new(3, move || {
        *count.lock().unwrap() += 1;
        true
    });
    assert!(queue.is_empty());
    assert_eq!(queue.push(Live::Up { link: 1 }), Delivery::Taken);
    assert_eq!(queue.push(Live::Event(Event::Changed)), Delivery::Taken);
    assert_eq!(queue.push(Live::Event(Event::Changed)), Delivery::Taken);
    assert_eq!(
        *woken.lock().unwrap(),
        1,
        "woken when it went from empty to not"
    );
    assert_eq!(queue.push(Live::Event(Event::Changed)), Delivery::Full);
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.take(), [Live::Down]);
    assert_eq!(queue.push(Live::Up { link: 1 }), Delivery::Taken);
    assert_eq!(*woken.lock().unwrap(), 2);
    let gone = LiveQueue::new(3, || false);
    assert_eq!(gone.push(Live::Up { link: 1 }), Delivery::Gone);
}

#[test]
fn a_flood_nobody_takes_stays_within_the_queue_and_the_link_is_dropped() {
    // C-R17: while the UI takes nothing (a suspension, §7.3) the backlog never passes the
    // queue's size. The live thread drops the link at the overflow, and the daemon sees it go.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let board = board(Mode::Auto, true);
    let server = Server::bind(
        &path,
        Path::new("/Users/tester"),
        Arc::clone(&board),
        Cancel::new(),
    )
    .unwrap();
    let queue = Arc::new(LiveQueue::new(4, || true));
    spawn(
        path.clone(),
        quick(5_000, 5_000, 5_000),
        Arc::clone(&queue),
        || {},
    )
    .unwrap();
    let deadline = Instant::now() + WAIT;
    while board.snapshot().subscribers == 0 {
        assert!(Instant::now() < deadline, "never subscribed");
        thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..20 {
        server.hub().emit(&DaemonEvent::Changed);
        assert!(queue.len() <= 4, "{}", queue.len());
        thread::sleep(Duration::from_millis(20));
    }
    while board.snapshot().subscribers != 0 {
        assert!(Instant::now() < deadline, "the link was never dropped");
        thread::sleep(Duration::from_millis(5));
    }
    assert!(queue.len() <= 4);
    let taken = queue.take();
    assert_eq!(taken.first(), Some(&Live::Down), "{taken:?}");
    server.shutdown();
}

#[test]
fn the_thread_ends_once_the_tui_no_longer_listens() {
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let queue = Arc::new(LiveQueue::new(4, || false));
    let thread = spawn(path, quick(10, 10, 10), queue, || {}).unwrap();
    let deadline = Instant::now() + WAIT;
    while !thread.is_finished() {
        assert!(Instant::now() < deadline, "the thread never ended");
        thread::sleep(Duration::from_millis(5));
    }
    thread.join().unwrap();
}

#[test]
fn a_panic_on_the_live_thread_calls_on_death() {
    // The TUI treats it as a lane that died (spec §4.3, §10.4).
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let died = Arc::new(AtomicBool::new(false));
    let told = Arc::clone(&died);
    let queue = Arc::new(LiveQueue::new(4, || {
        panic!("the live thread's wake panicked")
    }));
    let thread: JoinHandle<()> = spawn(path, Backoff::default(), queue, move || {
        told.store(true, Ordering::SeqCst)
    })
    .unwrap();
    assert!(thread.join().is_err());
    assert!(died.load(Ordering::SeqCst));
}

#[test]
fn status_comes_over_a_connection_of_its_own_beside_the_link() {
    // Spec §8: `subscribe` keeps the live thread's connection, so `status` opens another.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let server = Server::bind(
        &path,
        Path::new("/Users/tester"),
        board(Mode::Collect, false),
        Cancel::new(),
    )
    .unwrap();
    let (rx, _queue) = live(&path, Backoff::default());
    assert_eq!(next(&rx).1, Live::Up { link: 1 });
    let status = fetch_status(&path).unwrap();
    assert_eq!(status.providers.len(), 1);
    let p = &status.providers[0];
    assert_eq!((p.mode.as_str(), p.autoswitch), ("collect", false));
    server.hub().emit(&DaemonEvent::Changed);
    assert_eq!(
        next(&rx).1,
        Live::Event(Event::Changed),
        "the link carries on"
    );
    server.shutdown();
}

#[test]
fn status_with_no_daemon_or_another_protocol_fails_at_once() {
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let started = Instant::now();
    assert_eq!(fetch_status(&path).unwrap_err(), NO_DAEMON);
    fake(&path, |mut stream| {
        stream.write_all(hello(2).as_bytes()).unwrap();
    });
    assert_eq!(
        fetch_status(&path).unwrap_err(),
        "the daemon speaks protocol 2"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn a_daemon_that_never_answers_status_is_given_up_on_after_2_s() {
    // D5.7's bound: the daemon lane is never held longer, and a shutdown never waits for it
    // (spec §10.2).
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    fake(&path, |mut stream| {
        stream.write_all(hello(1).as_bytes()).unwrap();
        request(&stream);
        thread::sleep(Duration::from_secs(5));
    });
    let started = Instant::now();
    let error = fetch_status(&path).unwrap_err();
    let took = started.elapsed();
    assert_eq!(error, "the daemon did not answer in time");
    assert!(
        took >= Duration::from_secs(2) && took < Duration::from_secs(4),
        "{took:?}"
    );
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p tagteam-tui --test live` and `cargo test -p tagteam-daemon --test client`

Expected: FAIL: `tests/live.rs` stops with ``error[E0433]: failed to resolve: use of unresolved module or unlinked crate `tagteam_daemon` `` and ``error[E0432]: unresolved import `tagteam_tui::live` ``, and the daemon's `tests/client.rs` with ``error[E0599]: no method named `closer` found for struct `Events` in the current scope ``.

- [ ] **Step 3: Write the code**

The client's closer, then the dependency, the module, and its declaration.

In `crates/tagteam-daemon/src/client.rs`, replace:

```rust
//! finds the socket from the environment alone (D5.1), and learns in the first exchange
//! whether a daemon is there and speaks this protocol (D1.1 criterion 5).

use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};
```

with:

```rust
//! finds the socket from the environment alone (D5.1), and learns in the first exchange
//! whether a daemon is there and speaks this protocol (D1.1 criterion 5).

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};
```

In `crates/tagteam-daemon/src/client.rs`, replace:

```rust
    lines: Lines,
    done: bool,
}

impl Iterator for Events {
    type Item = Result<Event, ClientError>;

    fn next(&mut self) -> Option<Self::Item> {
```

with:

```rust
    lines: Lines,
    done: bool,
}

impl Events {
    /// A handle that ends this stream from another thread: the iterator then ends, as when the
    /// daemon closes the connection. Its read waits for as long as the daemon is silent, so a
    /// caller that finds the daemon no longer answers (the TUI, by `status`) ends it this way.
    pub fn closer(&self) -> io::Result<Closer> {
        self.lines.stream.try_clone().map(Closer)
    }
}

/// Ends an `Events` stream from another thread (`Events::closer`).
#[derive(Debug)]
pub struct Closer(UnixStream);

impl Closer {
    pub fn close(&self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

impl Iterator for Events {
    type Item = Result<Event, ClientError>;

    fn next(&mut self) -> Option<Self::Item> {
```

In `Cargo.lock`, replace:

```toml
 "ratatui",
 "serde_json",
 "tagteam-cc",
 "tagteam-core",
 "tagteam-engine",
 "tagteam-fake",
 "tagteam-provider",
 "tempfile",
```

with:

```toml
 "ratatui",
 "serde_json",
 "tagteam-cc",
 "tagteam-core",
 "tagteam-daemon",
 "tagteam-engine",
 "tagteam-fake",
 "tagteam-provider",
 "tempfile",
```

In `crates/tagteam-tui/Cargo.toml`, replace:

```toml
[dependencies]
tagteam-core.workspace = true
tagteam-provider.workspace = true
tagteam-engine.workspace = true
crossterm.workspace = true
jiff.workspace = true
libc.workspace = true
ratatui.workspace = true
```

with:

```toml
[dependencies]
tagteam-core.workspace = true
tagteam-provider.workspace = true
tagteam-engine.workspace = true
tagteam-daemon.workspace = true
crossterm.workspace = true
jiff.workspace = true
libc.workspace = true
ratatui.workspace = true
```

In `crates/tagteam-tui/src/lib.rs`, replace:

```rust
mod ask;
pub mod host;
mod input;
pub mod lanes;
pub mod msg;
mod runtime;
pub mod screens;
pub mod source;
```

with:

```rust
mod ask;
pub mod host;
mod input;
pub mod lanes;
pub mod live;
pub mod msg;
mod runtime;
pub mod screens;
pub mod source;
```

Create `crates/tagteam-tui/src/live.rs`:

```rust
//! The daemon's socket from the TUI's side (spec §8): the live thread, which subscribes and
//! passes on what it reads through a bounded queue, and the `status` fetch that the daemon
//! lane makes over a connection of its own. Neither touches the store, and neither is joined
//! (§4.3, §10.2): each ends with the process, or once the TUI no longer listens.

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tagteam_daemon::client::{Closer, Connect, Events, connect_at};
use tagteam_daemon::protocol::{Event, Status};

/// What the live thread passes on (spec §8).
#[derive(Debug, Clone, PartialEq)]
pub enum Live {
    /// Subscribed: every event the daemon emits from now on follows, until the next `Down`.
    /// `link` names this connection, 1 for the thread's first, then 2 and on (C-R21).
    Up { link: u64 },
    /// No link: no daemon answered, the connection failed, or it ended (`stopping`, a full
    /// queue, an error).
    Down,
    /// The daemon speaks protocol `server`, which this TUI does not (D5.6).
    Mismatch { server: u32 },
    /// One event, as the daemon sent it.
    Event(Event),
}

/// How long the live thread waits before it connects again (spec §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    /// The first wait, after a failure or a link that came down. Each failure doubles it.
    pub first: Duration,
    /// The longest wait. A link that stayed up at least this long starts the waits over.
    pub most: Duration,
    /// The wait after a daemon that speaks another protocol.
    pub mismatch: Duration,
}

impl Default for Backoff {
    /// Spec §8: from 1 s up to 30 s, and every 60 s after a mismatch.
    fn default() -> Self {
        Backoff {
            first: Duration::from_secs(1),
            most: Duration::from_secs(30),
            mismatch: Duration::from_secs(60),
        }
    }
}

/// What `fetch_status` says when no daemon answers.
pub const NO_DAEMON: &str = "no daemon answers";
/// C-R17: the most the live thread's queue holds, as the daemon's own queue for a subscriber
/// (D5.5).
pub const LIVE_QUEUE: usize = 256;

/// What `LiveQueue::push` did with what the live thread passed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// It waits for the UI thread.
    Taken,
    /// The queue was full: what it held was dropped for one `Down`, and the live thread drops
    /// its link.
    Full,
    /// The TUI no longer listens.
    Gone,
}

/// C-R17: what the live thread passes on, on its way to the UI thread. At most `cap` things
/// wait, so a UI thread that takes none (a suspension, spec §7.3) holds a bounded backlog. A
/// push into a full queue drops what it holds for a single `Down`, and the live thread drops its
/// link: the UI goes to timer mode, and the next link's snapshot (`status` and every view
/// shown, C-R4) restores the meaning of what was dropped. `wake` tells the UI thread there is
/// something to take each time the queue goes from empty to not, and says false once nobody
/// listens. The queue also holds the link's closer, by the link's name, so that the UI can end a
/// link that no longer works, and only that one (C-R19, C-R21).
pub struct LiveQueue {
    inner: Mutex<Inner>,
    cap: usize,
    wake: Box<dyn Fn() -> bool + Send + Sync>,
}

struct Inner {
    items: VecDeque<Live>,
    link: Option<(u64, Closer)>,
}

impl LiveQueue {
    pub fn new(cap: usize, wake: impl Fn() -> bool + Send + Sync + 'static) -> LiveQueue {
        LiveQueue {
            inner: Mutex::new(Inner {
                items: VecDeque::new(),
                link: None,
            }),
            cap: cap.max(1),
            wake: Box::new(wake),
        }
    }

    /// The queue, whatever a panic left it as: it holds only messages and a handle.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues `live` for the UI thread, or, when the queue is full, drops what it holds for a
    /// single `Down`.
    pub fn push(&self, live: Live) -> Delivery {
        let mut inner = self.lock();
        let woken = inner.items.is_empty();
        let delivery = if inner.items.len() >= self.cap {
            inner.items.clear();
            inner.items.push_back(Live::Down);
            Delivery::Full
        } else {
            inner.items.push_back(live);
            Delivery::Taken
        };
        drop(inner);
        if woken && !(self.wake)() {
            return Delivery::Gone;
        }
        delivery
    }

    /// Everything waiting, oldest first; the UI thread takes it at each wake.
    pub fn take(&self) -> Vec<Live> {
        self.lock().items.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.lock().items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// C-R19, C-R21: ends link `link`, if the live thread still holds it; it then goes down and
    /// connects again with backoff. The UI calls it for a link that no longer works. A link the
    /// thread has since replaced is gone already, and its successor is left alone.
    pub fn end_link(&self, link: u64) {
        if let Some((held, closer)) = &self.lock().link {
            if *held == link {
                closer.close();
            }
        }
    }

    /// The link the live thread holds now, by its name, if any.
    fn hold(&self, link: Option<(u64, Closer)>) {
        self.lock().link = link;
    }
}

/// Starts the live thread on the daemon at `path`. What it learns goes into `queue`; it returns
/// once nobody listens. `on_death` runs if the thread unwinds from a panic.
pub fn spawn(
    path: PathBuf,
    backoff: Backoff,
    queue: Arc<LiveQueue>,
    on_death: impl FnOnce() + Send + 'static,
) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("tagteam-tui-live".into())
        .spawn(move || {
            let _death = OnPanic(Some(on_death));
            run(&path, backoff, &queue);
        })
}

/// Runs its closure if the thread unwinds from a panic.
struct OnPanic<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Drop for OnPanic<F> {
    fn drop(&mut self) {
        if thread::panicking() {
            if let Some(f) = self.0.take() {
                f();
            }
        }
    }
}

/// What one try at a link came to.
enum Attempt {
    /// It came up, and went down after `lasted`.
    Lost { lasted: Duration },
    /// No link came up.
    Failed,
    /// The daemon speaks another protocol.
    Mismatch,
    /// The TUI no longer listens.
    Gone,
}

/// Spec §8: connects and subscribes, passes on every event, and once the link is down tries
/// again, waiting `first`, then twice as long after each failure, up to `most`; after a
/// mismatch it waits `mismatch`. A link that went down within `most` of coming up keeps the
/// longer waits, so a daemon that drops it at once is not reconnected to in a loop.
fn run(path: &Path, backoff: Backoff, queue: &LiveQueue) {
    let mut wait = backoff.first;
    let mut links = 0;
    loop {
        let pause = match attempt(path, queue, &mut links) {
            Attempt::Gone => return,
            Attempt::Mismatch => backoff.mismatch,
            Attempt::Lost { lasted } if lasted >= backoff.most => {
                wait = (backoff.first * 2).min(backoff.most);
                backoff.first
            }
            Attempt::Lost { .. } | Attempt::Failed => {
                let pause = wait;
                wait = (wait * 2).min(backoff.most);
                pause
            }
        };
        thread::sleep(pause);
    }
}

/// One try: the connection, the greeting and the subscription, then the events until the link
/// ends. `links` counts the links made, and names this one if it comes up.
fn attempt(path: &Path, queue: &LiveQueue, links: &mut u64) -> Attempt {
    let (said, attempt) = match connect_at(path) {
        Ok(Connect::Connected(client)) => match client.subscribe() {
            Ok(events) => {
                *links += 1;
                let link = *links;
                queue.hold(events.closer().ok().map(|closer| (link, closer)));
                let attempt = linked(events, link, queue);
                queue.hold(None);
                return attempt;
            }
            Err(_) => (Live::Down, Attempt::Failed),
        },
        Ok(Connect::Mismatch { server, .. }) => (Live::Mismatch { server }, Attempt::Mismatch),
        Ok(Connect::NoDaemon) | Err(_) => (Live::Down, Attempt::Failed),
    };
    match queue.push(said) {
        Delivery::Gone => Attempt::Gone,
        Delivery::Taken | Delivery::Full => attempt,
    }
}

/// A subscribed link, `link`: `Up`, each event as it arrives, then `Down` once the daemon closes
/// it, it fails, or it is ended (`LiveQueue::end_link`). `stopping` ends it at once, whether or
/// not the daemon goes on to close its socket (C-R18); a full queue ends it too (C-R17).
/// Returning drops the connection.
fn linked(events: Events, link: u64, queue: &LiveQueue) -> Attempt {
    let up = Instant::now();
    let lost = |up: Instant| Attempt::Lost {
        lasted: up.elapsed(),
    };
    match queue.push(Live::Up { link }) {
        Delivery::Taken => {}
        Delivery::Full => return lost(up),
        Delivery::Gone => return Attempt::Gone,
    }
    for event in events {
        let Ok(event) = event else {
            break;
        };
        let stopping = matches!(event, Event::Stopping { .. });
        match queue.push(Live::Event(event)) {
            Delivery::Taken if stopping => break,
            Delivery::Taken => {}
            Delivery::Full => return lost(up),
            Delivery::Gone => return Attempt::Gone,
        }
    }
    match queue.push(Live::Down) {
        Delivery::Gone => Attempt::Gone,
        Delivery::Taken | Delivery::Full => lost(up),
    }
}

/// Spec §8: the daemon's `status`, over a short-lived connection of its own, since the live
/// thread's is the event stream's. It waits 250 ms at most for the greeting and 2 s for the
/// reply (D5.7).
pub fn fetch_status(path: &Path) -> Result<Status, String> {
    match connect_at(path) {
        Ok(Connect::Connected(mut client)) => client.status().map_err(|e| e.to_string()),
        Ok(Connect::NoDaemon) => Err(NO_DAEMON.to_owned()),
        Ok(Connect::Mismatch { server, .. }) => Err(format!("the daemon speaks protocol {server}")),
        Err(e) => Err(e.to_string()),
    }
}
```

- [ ] **Step 4: Run the tests and see them pass**

Run, outside the sandbox:
- `cargo test -p tagteam-tui --test live` — Expected: `16 passed`. Inside the sandbox three pass, and the thirteen that bind a socket fail with `PermissionDenied`.
- `cargo test -p tagteam-daemon --test client` — Expected: `16 passed` (1 new).

- [ ] **Step 5: Format and lint**

Run:
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`

Expected: no output from the first; `Finished` and no warnings from the others.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-daemon/tests/client.rs crates/tagteam-tui/tests/live.rs crates/tagteam-daemon/src/client.rs Cargo.lock crates/tagteam-tui/Cargo.toml crates/tagteam-tui/src/lib.rs crates/tagteam-tui/src/live.rs
git commit -m "Follow the daemon's events on a live thread and fetch its status over a connection of its own"
```

### Task 2: Live mode: the link, the daemon lane, and what each event reads

Live mode in the state machine (spec §6.3, §8), in `daemon/mod.rs`, and the daemon lane and the live thread in the runtime (spec §4.3):

- **The link.** `Live::Up { link }` makes `link` the one followed, fetches `status` at once, and reads every view shown 250 ms later (C-R4). The first `status` reply for that link starts live mode; one for another link is dropped (C-R3, C-R21). `Live::Down` is timer mode, and `Live::Mismatch` is remembered for the header (Task 3).
- **One `status` out at a time** (C-R22): one asked for while another is out is sent when that one is answered.
- **The events** (§8, C-R5, C-R6): `poll`, `switch`, the quarantine kinds and `usage` read the views and fetch `status`; auto's other kinds fetch `status`; `changed` reads the views; `mode` fetches `status` at once; `stopping` is kept for the header; a kind this TUI does not know reads nothing. All but `mode`'s are debounced.
- **The timers** (C-R7): while linked, every 60 s, `status`, and in live mode `accounts` (and `history`). The 30 s timer and the first collection offer the collector only what nothing else collects for (C-R8).
- **A link that does not work** (C-R18, C-R19): `stopping` is timer mode at once; a `status` that fails, or does not come within 5 s of being sent, is timer mode and `Cmd::DropLink { link }`, for that link alone (C-R21).
- **The first collection** waits until the TUI knows who collects: no daemon, the first `status`, or 2 s (C-R20).
- **The runtime.** `TuiOptions::daemon` is the socket (C-R15). `run` spawns the daemon lane (`DaemonReq::Status`, coalesced) beside the engine lanes, and the live thread beside them on the lanes' live queue. A `Msg::LiveReady` applies the whole queue; the loop applies every message waiting before it draws (C-R17); `Cmd::DropLink { link }` ends that link if the live thread still holds it. The shutdown closes the daemon lane and joins only the engine lanes; a daemon lane or live thread that panicked is named in the report (C-R1).

**Readings of the spec this task commits to:**
- **The first frame waits for the accounts only** (§6.1). A link that comes up before it fetches `status` on the daemon lane, which the first frame does not wait for, and reads the views after it (C-R4).
- **"Every view it shows"** (§8) is `accounts`, History's history while History is open, and Tokens' report while Tokens is open.
- **Live mode needs `status`** (§8: "switches to live mode once the first `status` reply arrives"). Until then a link is timer mode, whose 30 s timer reads and collects as before, while the link's 60 s timer fetches `status`.
- **`collect_due`'s skip** (§6.3) is decided per provider from the last `status`; `r` still offers every shown account.
- **The tests answer `status` as a working daemon does** wherever they mean the link to last: a fetch left unanswered for 5 s ends it (C-R19).
- **The flood test** stands in for the live thread inside a suspension: it pushes into the live queue until it is full, as the thread does, then the next link's first things.
- **The order a suspension leaves** (C-R21) is tested twice: in the state machine, message by message, and in the runtime with a real live thread against a fake daemon. There the test holds the loop's messages back, as a suspension does, until link 1's failed `status` is queued and link 2 is up, then applies them in the order they came. It binds a socket, so it runs outside the sandbox.
- **Two requests on one link** (C-R22): the test answers the first and leaves the second unanswered, as a lane stuck in `connect` would.
- **Part A's and part B's tests change only where part C changes behaviour:** the runtime's test `Fixture` builds its lanes with no daemon lane, and `TuiOptions` gains `daemon: None` in its tests.

**Depends on:** 1

**Files:**
- Create: `crates/tagteam-tui/src/daemon/mod.rs`
- Modify: `crates/tagteam-tui/src/lib.rs`
- Modify: `crates/tagteam-tui/src/msg.rs`
- Modify: `crates/tagteam-tui/src/state.rs`
- Modify: `crates/tagteam-tui/src/update.rs`
- Modify: `crates/tagteam-tui/src/testutil.rs` (the daemon's `status` fixtures, and `go_live`)
- Modify: `crates/tagteam-tui/src/runtime.rs`
- Modify: `crates/tagteam/src/tui_cmd.rs` (the daemon's socket)

**Interfaces:**
- Consumes: Task 1's CC1 (`LiveQueue`, `LIVE_QUEUE`, `spawn`, `fetch_status`); `tagteam_daemon::protocol::{Event, Status, StatusProvider}`; `tagteam_engine::daemon::daemon_socket_path`; part B's `screens::tokens::requery` and `screens::history::reload`.
- Produces: CC2; CC3's `DaemonState` with `live`, `status`, `job`, `stopping`, `mismatch`, `settled`, `await_daemon`, and `on_live`, `on_status`, `on_tick`, `collected_elsewhere`, `to_collect`; `State::daemon`; CC6. Tasks 3–5 build on all of it.

**Spec:**
- TUI spec §4.3 (the daemon lane: `status` over a short-lived connection, no engine, never joined; coalescing), §6.1, §6.2, §6.3 (live mode: the skip, the debounced re-reads, the 60 s timer, `r`), §8 ("Behaviour": `status` is the authority; "Connecting"; "What each event does"; "Disconnect"), §10.2 (the shutdown joins the engine lanes; the live thread and the daemon lane are not joined), §10.4 (a lane that died is fatal), §11 ("Daemon client": events mapped to re-reads, `Unknown` ignored, reconnecting re-reading every view, the skip, `status` re-fetched after subscribing, on `mode` and every 60 s).
- Daemon spec D4.3 (the modes and `waitingFor`), D5.4 (the snapshot is updated after each job).

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-tui/src/daemon/mod.rs` with its tests (Step 3 puts the code above them):

```rust
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;
    use tagteam_engine::views::ProviderAccounts;

    use super::*;
    use crate::msg::Msg;
    use crate::testutil::*;
    use crate::update::{drawn, update};

    fn event(e: Event) -> Msg {
        Msg::Live(Live::Event(e))
    }

    /// One of auto's kinds (§11.4), for Claude Code.
    fn auto(kind: &str) -> Event {
        Event::Auto {
            kind: kind.into(),
            provider: "claude-code".into(),
            raw: json!({"schemaVersion": 1, "event": kind, "ts": "2026-09-21T14:13:20Z"}),
        }
    }

    fn usage() -> Event {
        Event::Usage {
            provider: "claude-code".into(),
            updated: vec![1],
            fetch_errors: BTreeMap::new(),
        }
    }

    fn mode(mode: &str) -> Event {
        Event::Mode {
            provider: "claude-code".into(),
            mode: mode.into(),
            waiting_for: None,
            holder_pid: None,
            backoff: None,
        }
    }

    /// Claude Code's job switching accounts.
    fn auto_on() -> Status {
        daemon_status(vec![daemon_job("claude-code", "auto", true)], vec![])
    }

    /// The reads and fetches among `cmds`; the token index's timers are left out.
    fn reads(cmds: Vec<Cmd>) -> Vec<Cmd> {
        cmds.into_iter()
            .filter(|c| {
                matches!(
                    c,
                    Cmd::LoadAccounts { .. }
                        | Cmd::LoadHistory { .. }
                        | Cmd::LoadReport { .. }
                        | Cmd::CollectDue { .. }
                        | Cmd::FetchStatus { .. }
                )
            })
            .collect()
    }

    fn tick(state: &mut State, at_ms: i64) -> Vec<Cmd> {
        reads(update(state, Msg::Tick, at_ms))
    }

    /// `tick`, with each `status` it asks for answered as a working daemon answers it.
    fn tick_answered(state: &mut State, at_ms: i64) -> Vec<Cmd> {
        let cmds = tick(state, at_ms);
        for c in &cmds {
            if let Cmd::FetchStatus { link } = c {
                let result = Ok(auto_on());
                update(
                    state,
                    Msg::DaemonStatus {
                        link: *link,
                        result,
                    },
                    at_ms,
                );
            }
        }
        cmds
    }

    /// The reads `cmds` asks for, by name, in order.
    fn names(cmds: &[Cmd]) -> Vec<&'static str> {
        cmds.iter()
            .map(|c| match c {
                Cmd::LoadAccounts { .. } => "accounts",
                Cmd::LoadHistory { .. } => "history",
                Cmd::LoadReport { .. } => "report",
                Cmd::CollectDue { .. } => "collect",
                Cmd::FetchStatus { .. } => "status",
                _ => "other",
            })
            .collect()
    }

    /// On `screen`, with `n` accounts read, History on the first.
    fn showing(screen: Screen, n: u32) -> State {
        let mut state = fresh(screen);
        state.accounts.apply(snapshot(many(n)));
        state.history.select(id(1));
        state
    }

    #[test]
    fn a_link_fetches_the_status_at_once_and_reads_every_view_shown_250_ms_later() {
        // Spec §8, "Connecting": the snapshot follows the subscription, and the first frame's
        // own read is left to finish (§6.1).
        for (screen, want) in [
            (Screen::Accounts, vec!["accounts"]),
            (Screen::History, vec!["accounts", "history"]),
            (Screen::Tokens, vec!["accounts", "report"]),
        ] {
            let mut state = showing(screen, 1);
            let cmds = update(&mut state, Msg::Live(Live::Up { link: 1 }), NOW_MS);
            assert_eq!(cmds, [Cmd::FetchStatus { link: 1 }], "{screen:?}");
            assert!(!state.daemon.live(), "timer mode until the status comes");
            assert!(tick(&mut state, NOW_MS + 249).is_empty(), "{screen:?}");
            assert_eq!(names(&tick(&mut state, NOW_MS + 250)), want, "{screen:?}");
            assert!(tick(&mut state, NOW_MS + 500).is_empty(), "once");
        }
    }

    #[test]
    fn the_first_status_starts_live_mode_and_a_reply_for_another_link_is_dropped() {
        let mut state = fresh(Screen::Accounts);
        let up = |state: &mut State, link| {
            let cmds = update(state, Msg::Live(Live::Up { link }), NOW_MS);
            assert_eq!(cmds, [Cmd::FetchStatus { link }]);
            link
        };
        let reply = |link, result| Msg::DaemonStatus { link, result };
        let first = up(&mut state, 1);
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        let second = up(&mut state, 2);
        update(&mut state, reply(first, Ok(auto_on())), NOW_MS);
        assert!(!state.daemon.live(), "the reply of a link that went down");
        state.dirty = false;
        update(&mut state, reply(second, Ok(auto_on())), NOW_MS);
        assert!(state.daemon.live() && state.dirty);
        assert_eq!(
            state.daemon.job(&cc()).map(|j| j.mode.as_str()),
            Some("auto")
        );
    }

    #[test]
    fn a_link_that_goes_down_is_timer_mode_and_a_late_reply_does_not_undo_that() {
        let mut state = showing(Screen::Accounts, 1);
        let link = go_live(&mut state, auto_on());
        state.dirty = false;
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        assert!(!state.daemon.live() && state.dirty);
        assert!(state.daemon.status().is_none());
        let late = Msg::DaemonStatus {
            link,
            result: Ok(auto_on()),
        };
        update(&mut state, late, NOW_MS);
        assert!(!state.daemon.live());
        // The link's own timers stop: its re-read and its 60 s fetch.
        assert!(tick(&mut state, NOW_MS + 250).is_empty());
        let at_60 = tick(&mut state, NOW_MS + 60_000);
        assert!(!names(&at_60).contains(&"status"), "{at_60:?}");
        state.dirty = false;
        update(&mut state, Msg::Live(Live::Down), NOW_MS + 61_000);
        assert!(!state.dirty, "a second Down changes nothing");
    }

    #[test]
    fn an_event_reads_what_it_affects_250_ms_later_once_for_every_event_until_then() {
        // Spec §8: `poll`, `switch`, `usage`, `changed` and the quarantine events re-read
        // `accounts`, and `history` while it is open, debounced (§6.3). Every event of a
        // provider job's run also fetches `status` (C-R5); `changed` is another process's. A
        // kind this TUI does not know reads nothing.
        let mut state = showing(Screen::History, 1);
        go_live(&mut state, auto_on());
        tick(&mut state, NOW_MS + 250);
        let unknown = Event::Unknown {
            kind: "added-later".into(),
            raw: json!({"event": "added-later"}),
        };
        let all = vec!["status", "accounts", "history"];
        let mut at = NOW_MS + 1_000;
        for (e, want) in [
            (auto("poll"), all.clone()),
            (auto("switch"), all.clone()),
            (auto("account-quarantined"), all.clone()),
            (auto("account-unquarantined"), all.clone()),
            (usage(), all.clone()),
            (Event::Changed, vec!["accounts", "history"]),
            (auto("no-switch"), vec!["status"]),
            (auto("all-exhausted"), vec!["status"]),
            (auto("error"), vec!["status"]),
            (auto("sleep"), vec!["status"]),
            (auto("config-warning"), vec!["status"]),
            (unknown, vec![]),
        ] {
            assert!(update(&mut state, event(e.clone()), at).is_empty(), "{e:?}");
            assert!(tick(&mut state, at + 249).is_empty(), "{e:?}");
            let read = tick_answered(&mut state, at + 250);
            assert_eq!(names(&read), want, "{e:?}");
            at += 1_000;
        }
        // A burst within the 250 ms: one batch for them all.
        for (i, e) in [auto("poll"), Event::Changed, auto("no-switch")]
            .into_iter()
            .enumerate()
        {
            update(&mut state, event(e), at + i as i64 * 20);
        }
        assert_eq!(names(&tick_answered(&mut state, at + 250)), all);
        assert!(tick(&mut state, at + 500).is_empty());
    }

    #[test]
    fn a_mode_event_fetches_the_status_and_its_fields_are_not_applied() {
        // Spec §8: `status` is the authority; an event is a reason to fetch it.
        let mut state = showing(Screen::Accounts, 1);
        let link = go_live(&mut state, auto_on());
        let cmds = update(&mut state, event(mode("idle")), NOW_MS + 1_000);
        assert_eq!(cmds, [Cmd::FetchStatus { link }]);
        assert_eq!(
            state.daemon.job(&cc()).map(|j| j.mode.as_str()),
            Some("auto"),
            "until the status says otherwise"
        );
    }

    #[test]
    fn every_60_s_a_link_fetches_the_status_and_in_live_mode_reads_the_views() {
        // Spec §6.3: a missed event, or a setting changed elsewhere, costs at most a minute.
        let mut state = showing(Screen::History, 1);
        go_live(&mut state, auto_on());
        tick(&mut state, NOW_MS + 250);
        assert!(tick(&mut state, NOW_MS + 59_999).is_empty());
        assert_eq!(
            names(&tick_answered(&mut state, NOW_MS + 60_000)),
            ["status", "accounts", "history"]
        );
        assert!(tick(&mut state, NOW_MS + 119_999).is_empty());
        assert_eq!(names(&tick(&mut state, NOW_MS + 120_000))[0], "status");
        // A link whose `status` does not come is timer mode, and is dropped after 5 s (C-R19):
        // the 30 s timer reads the views and collects as it does without a daemon.
        let mut state = showing(Screen::Accounts, 1);
        update(&mut state, Msg::Live(Live::Up { link: 1 }), NOW_MS);
        tick(&mut state, NOW_MS + 250);
        let dropped = update(&mut state, Msg::Tick, NOW_MS + 5_000);
        assert_eq!(dropped, [Cmd::DropLink { link: 1 }]);
        assert_eq!(
            names(&tick(&mut state, NOW_MS + 30_000)),
            ["accounts", "collect"]
        );
        assert_eq!(
            names(&tick(&mut state, NOW_MS + 60_000)),
            ["accounts", "collect"]
        );
    }

    /// Claude Code's accounts 1 and 2, and FakeAgent's account 3.
    fn two_providers() -> crate::source::AccountsSnapshot {
        let fake = tagteam_core::ProviderId::new("fake-agent");
        let mut third = account(3, "c@x.co", read(60, 10.0, 20.0, false));
        third.row.provider = fake.clone();
        let mut snap = snapshot(many(2));
        snap.providers.push(ProviderAccounts {
            provider: fake,
            active_position: None,
            accounts: vec![third],
        });
        snap
    }

    #[test]
    fn the_timer_collects_only_for_the_providers_nothing_else_collects_for() {
        // Spec §6.3: `auto`, `collect`, and `waiting` for `engine-lock` collect elsewhere;
        // `idle`, `waiting` for `schema`, and a provider with no job do not.
        for (fake_job, collected) in [
            (Some(("auto", None)), false),
            (Some(("collect", None)), false),
            (Some(("waiting", Some("engine-lock"))), false),
            (Some(("waiting", Some("schema"))), true),
            (Some(("idle", None)), true),
            (None, true),
        ] {
            let mut state = fresh(Screen::Accounts);
            state.accounts.apply(two_providers());
            let mut jobs = vec![daemon_job("claude-code", "auto", true)];
            if let Some((mode, waiting_for)) = fake_job {
                let mut job = daemon_job("fake-agent", mode, false);
                job.waiting_for = waiting_for.map(str::to_owned);
                jobs.push(job);
            }
            go_live(&mut state, daemon_status(jobs, vec![]));
            tick(&mut state, NOW_MS + 250);
            let at_30 = tick(&mut state, NOW_MS + 30_000);
            let want: Vec<Cmd> = if collected {
                vec![Cmd::CollectDue {
                    r#gen: 1,
                    accounts: vec![id(3)],
                }]
            } else {
                vec![]
            };
            assert_eq!(at_30, want, "{fake_job:?}");
            // `r` still collects every account shown, as `list` would.
            let r = update(&mut state, ch('r'), NOW_MS + 31_000);
            let all = r.iter().find_map(|c| match c {
                Cmd::CollectDue { accounts, .. } => Some(accounts.clone()),
                _ => None,
            });
            assert_eq!(all, Some(vec![id(1), id(2), id(3)]), "{fake_job:?}");
        }
        // Without a link, the timer collects for every provider.
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(two_providers());
        let at_30 = tick(&mut state, NOW_MS + 30_000);
        assert!(
            matches!(at_30.as_slice(), [Cmd::LoadAccounts { .. }, Cmd::CollectDue { accounts, .. }] if accounts.len() == 3),
            "{at_30:?}"
        );
    }

    #[test]
    fn the_first_collection_leaves_out_what_the_daemon_collects_for() {
        // Spec §1.1 criterion 4, §6.1, §6.3.
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(two_providers());
        go_live(&mut state, auto_on());
        let first = reads(drawn(&mut state));
        assert_eq!(
            first,
            [Cmd::CollectDue {
                r#gen: 1,
                accounts: vec![id(3)]
            }]
        );
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(2)));
        go_live(&mut state, auto_on());
        assert!(
            reads(drawn(&mut state)).is_empty(),
            "the daemon collects for all"
        );
        assert_eq!(state.gens.collect, 0);
        update(&mut state, Msg::Live(Live::Down), NOW_MS + 1_000);
        assert_eq!(
            reads(drawn(&mut state)),
            [Cmd::CollectDue {
                r#gen: 1,
                accounts: vec![id(1), id(2)]
            }],
            "without the daemon, the next frame collects"
        );
    }

    /// A TUI that follows a daemon, on Accounts with `accounts` read: its first collection
    /// waits until it knows who collects (C-R20).
    fn awaiting(accounts: crate::source::AccountsSnapshot) -> State {
        let mut state = fresh(Screen::Accounts);
        state.daemon.await_daemon(NOW_MS);
        state.accounts.apply(accounts);
        state
    }

    #[test]
    fn the_first_collection_waits_for_the_daemon_s_first_status() {
        // C-R20: the accounts come, and a frame is drawn, before `status` says who collects.
        let mut state = awaiting(two_providers());
        assert!(
            reads(drawn(&mut state)).is_empty(),
            "drawn, and nothing collected"
        );
        let cmds = update(&mut state, Msg::Live(Live::Up { link: 1 }), NOW_MS + 10);
        assert_eq!(cmds, [Cmd::FetchStatus { link: 1 }]);
        assert!(
            reads(drawn(&mut state)).is_empty(),
            "linked, `status` to come"
        );
        let reply = Msg::DaemonStatus {
            link: 1,
            result: Ok(auto_on()),
        };
        update(&mut state, reply, NOW_MS + 20);
        assert_eq!(
            reads(drawn(&mut state)),
            [Cmd::CollectDue {
                r#gen: 1,
                accounts: vec![id(3)]
            }],
            "only what the daemon leaves"
        );
    }

    #[test]
    fn the_first_collection_goes_ahead_when_no_daemon_answers_or_after_2_s() {
        // C-R20: `NoDaemon` or a mismatch settles it at once; a link whose `status` has not come
        // settles it 2 s after start.
        for answer in [Some(Live::Down), Some(Live::Mismatch { server: 2 }), None] {
            let mut state = awaiting(snapshot(many(2)));
            assert!(reads(drawn(&mut state)).is_empty(), "{answer:?}");
            match answer.clone() {
                Some(live) => {
                    update(&mut state, Msg::Live(live), NOW_MS + 10);
                }
                None => {
                    update(&mut state, Msg::Live(Live::Up { link: 1 }), NOW_MS + 10);
                    update(&mut state, Msg::Tick, NOW_MS + 1_999);
                    assert!(reads(drawn(&mut state)).is_empty(), "within the 2 s");
                    update(&mut state, Msg::Tick, NOW_MS + 2_000);
                }
            }
            assert_eq!(
                reads(drawn(&mut state)),
                [Cmd::CollectDue {
                    r#gen: 1,
                    accounts: vec![id(1), id(2)]
                }],
                "{answer:?}"
            );
        }
    }

    #[test]
    fn a_status_that_fails_or_never_comes_ends_the_link() {
        // C-R19: a daemon that holds the link but does not answer `status` is not followed: the
        // TUI is in timer mode, and tells the live thread to end the link, which then connects
        // again with backoff.
        let mut state = showing(Screen::Accounts, 1);
        let link = go_live(&mut state, auto_on());
        let asked = update(&mut state, event(mode("collect")), NOW_MS + 500);
        assert_eq!(asked, [Cmd::FetchStatus { link }]);
        let failed = Msg::DaemonStatus {
            link,
            result: Err("the daemon did not answer in time".into()),
        };
        assert_eq!(
            update(&mut state, failed, NOW_MS + 1_000),
            [Cmd::DropLink { link }]
        );
        assert!(!state.daemon.live() && state.daemon.status().is_none());
        // Asked for, and no answer within 5 s: a lane stuck in `connect`, say.
        let mut state = showing(Screen::Accounts, 1);
        let link = go_live(&mut state, auto_on());
        let asked = update(&mut state, event(mode("collect")), NOW_MS + 1_000);
        assert_eq!(asked, [Cmd::FetchStatus { link }]);
        let before = update(&mut state, Msg::Tick, NOW_MS + 5_999);
        assert!(!before.contains(&Cmd::DropLink { link }), "{before:?}");
        assert_eq!(
            update(&mut state, Msg::Tick, NOW_MS + 6_000),
            [Cmd::DropLink { link }]
        );
        assert!(!state.daemon.live());
        let after = update(&mut state, Msg::Tick, NOW_MS + 7_000);
        assert!(!after.contains(&Cmd::DropLink { link }), "{after:?}");
    }

    #[test]
    fn a_failure_for_a_link_the_live_thread_has_replaced_ends_that_link_alone() {
        // C-R21, the order a suspension (§7.3) can leave the messages in: link 1's `status`
        // failed, and meanwhile the live thread lost link 1 and brought link 2 up. The failure
        // ends link 1 by its name, which the live thread no longer holds; link 2 then comes up
        // as any link does, and link 1's replies change nothing.
        let mut state = showing(Screen::Accounts, 1);
        let up = update(&mut state, Msg::Live(Live::Up { link: 1 }), NOW_MS);
        assert_eq!(up, [Cmd::FetchStatus { link: 1 }]);
        let answered = Msg::DaemonStatus {
            link: 1,
            result: Ok(auto_on()),
        };
        update(&mut state, answered, NOW_MS);
        let asked = update(&mut state, event(mode("collect")), NOW_MS + 1_000);
        assert_eq!(asked, [Cmd::FetchStatus { link: 1 }]);
        let failed = Msg::DaemonStatus {
            link: 1,
            result: Err("the daemon closed the connection".into()),
        };
        let at = NOW_MS + 60_000;
        assert_eq!(update(&mut state, failed, at), [Cmd::DropLink { link: 1 }]);
        assert!(update(&mut state, Msg::Live(Live::Down), at).is_empty());
        let up = update(&mut state, Msg::Live(Live::Up { link: 2 }), at);
        assert_eq!(up, [Cmd::FetchStatus { link: 2 }]);
        let answered = Msg::DaemonStatus {
            link: 2,
            result: Ok(auto_on()),
        };
        update(&mut state, answered, at);
        assert!(state.daemon.live(), "link 2 is followed");
        for result in [Err("late".to_owned()), Ok(auto_on())] {
            let late = Msg::DaemonStatus { link: 1, result };
            assert!(update(&mut state, late, at + 1).is_empty());
            assert!(state.daemon.live());
        }
    }

    #[test]
    fn one_status_is_out_at_a_time_and_the_one_out_is_given_up_on_5_s_after_it_was_asked() {
        // C-R22: A is out when an event asks again. A cannot answer for what came after it was
        // asked, so B is asked when A is answered, with 5 s of its own: B stuck in the lane's
        // `connect` ends the link, though A was answered.
        let mut state = showing(Screen::Accounts, 1);
        let link = go_live(&mut state, auto_on());
        let a = update(&mut state, event(mode("collect")), NOW_MS + 1_000);
        assert_eq!(a, [Cmd::FetchStatus { link }]);
        let b = update(&mut state, event(mode("auto")), NOW_MS + 1_100);
        assert!(b.is_empty(), "B waits for A: {b:?}");
        let tick_b = update(&mut state, Msg::Tick, NOW_MS + 1_150);
        assert!(!tick_b.contains(&Cmd::FetchStatus { link }), "{tick_b:?}");
        let a_answered = Msg::DaemonStatus {
            link,
            result: Ok(auto_on()),
        };
        let b = update(&mut state, a_answered, NOW_MS + 1_200);
        assert_eq!(b, [Cmd::FetchStatus { link }], "B, once A is answered");
        assert!(state.daemon.live());
        let before = update(&mut state, Msg::Tick, NOW_MS + 6_199);
        assert!(!before.contains(&Cmd::DropLink { link }), "{before:?}");
        assert_eq!(
            update(&mut state, Msg::Tick, NOW_MS + 6_200),
            [Cmd::DropLink { link }]
        );
        assert!(!state.daemon.live());
    }

    #[test]
    fn stopping_ends_the_link_at_once_and_names_its_reason_until_the_next_link_or_30_s() {
        // C-R18: timer mode at once, whether or not the daemon goes on to close its socket.
        let mut state = fresh(Screen::Accounts);
        go_live(&mut state, auto_on());
        let stopping = Event::Stopping {
            reason: "upgrade".into(),
        };
        assert!(update(&mut state, event(stopping.clone()), NOW_MS).is_empty());
        assert!(!state.daemon.live() && state.daemon.status().is_none());
        assert_eq!(state.daemon.stopping(), Some("upgrade"));
        let after = update(&mut state, event(Event::Changed), NOW_MS);
        assert!(after.is_empty(), "the link is over");
        state.dirty = false;
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        assert!(!state.dirty, "the live thread's own Down is no news");
        update(&mut state, Msg::Tick, NOW_MS + 29_999);
        assert_eq!(state.daemon.stopping(), Some("upgrade"));
        state.dirty = false;
        update(&mut state, Msg::Tick, NOW_MS + 30_000);
        assert_eq!(state.daemon.stopping(), None);
        assert!(state.dirty);
        go_live(&mut state, auto_on());
        update(&mut state, event(stopping), NOW_MS);
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        update(&mut state, Msg::Live(Live::Up { link: 9 }), NOW_MS + 1_000);
        assert_eq!(state.daemon.stopping(), None, "the next daemon answered");
    }

    #[test]
    fn a_mismatch_is_kept_until_a_try_finds_another_daemon_or_none() {
        let mut state = fresh(Screen::Accounts);
        update(&mut state, Msg::Live(Live::Mismatch { server: 2 }), NOW_MS);
        assert_eq!(state.daemon.mismatch(), Some(2));
        assert!(!state.daemon.live());
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        assert_eq!(state.daemon.mismatch(), None, "no daemon answered");
        update(&mut state, Msg::Live(Live::Mismatch { server: 2 }), NOW_MS);
        update(&mut state, Msg::Live(Live::Up { link: 1 }), NOW_MS);
        assert_eq!(state.daemon.mismatch(), None, "a daemon of this protocol");
    }

    #[test]
    fn nothing_is_asked_of_the_daemon_once_the_shutdown_has_begun() {
        let mut state = showing(Screen::Accounts, 1);
        go_live(&mut state, auto_on());
        state.quitting = Some(0);
        let up = update(&mut state, Msg::Live(Live::Up { link: 9 }), NOW_MS);
        assert!(up.is_empty());
        assert!(update(&mut state, event(mode("collect")), NOW_MS).is_empty());
        assert!(update(&mut state, event(Event::Changed), NOW_MS).is_empty());
        assert!(tick(&mut state, NOW_MS + 60_000).is_empty());
    }
}
```

Declare the module, so that Step 2 builds its tests:

In `crates/tagteam-tui/src/lib.rs`, replace:

```rust
mod ask;
pub mod host;
```

with:

```rust
mod ask;
pub mod daemon;
pub mod host;
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
        assert!(!act().same_as(&act()));
        assert!(!collect(1).same_as(&act()));
        assert!(!act().same_as(&collect(1)));
    }
}
```

with:

```rust
        assert!(!act().same_as(&act()));
        assert!(!collect(1).same_as(&act()));
        assert!(!act().same_as(&collect(1)));
    }

    #[test]
    fn status_requests_always_coalesce() {
        // Spec §8: a burst of events costs one request; the lane keeps the newest link's.
        let status = |link| DaemonReq::Status { link };
        assert!(status(1).same_as(&status(1)));
        assert!(status(1).same_as(&status(2)));
    }
}
```

In `crates/tagteam-tui/src/testutil.rs`, replace:

```rust
//! Fixtures for the state machine's, the screens' and the overlays' tests: views built from
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
```

with:

```rust
//! Fixtures for the state machine's, the screens' and the overlays' tests: views built from
//! the engine's public types, keys, and a `TestBackend` read back as text.
#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use jiff::tz::TimeZone;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, CellWidth};
use tagteam_core::{AccountId, CLAUDE_CODE, Pace, ProviderId, Sample, Window, WindowKind};
use tagteam_daemon::protocol::{
    Status, StatusDaemon, StatusJob, StatusNotifications, StatusProvider,
};
use tagteam_engine::store::AccountRow;
use tagteam_engine::tokens::{
    Cost, IndexState, PeriodKey, RowAccount, RowKey, TokenCounts, TokenGrouping, TokenPeriod,
    TokenQuery, TokenReport, TokenRow, TokenTotal,
```

In `crates/tagteam-tui/src/testutil.rs`, replace:

```rust
    AccountView, HistoryView, HistoryWindow, ProviderAccounts, UsageStatus, UsageView,
};
use tagteam_provider::KindTraits;

use crate::msg::Msg;
use crate::screens::tokens::TokensState;
use crate::source::AccountsSnapshot;
use crate::state::{Screen, State};

```

with:

```rust
    AccountView, HistoryView, HistoryWindow, ProviderAccounts, UsageStatus, UsageView,
};
use tagteam_provider::KindTraits;

use crate::live::Live;
use crate::msg::{Cmd, Msg};
use crate::screens::tokens::TokensState;
use crate::source::AccountsSnapshot;
use crate::state::{Screen, State};

```

In `crates/tagteam-tui/src/testutil.rs`, replace:

```rust
        prices_as_of: "2026-10-01",
        price_overrides: vec![],
    }
}
```

with:

```rust
        prices_as_of: "2026-10-01",
        price_overrides: vec![],
    }
}

/// The fixture daemon's `status` (D5.4): pid 4242, notifications off, with `providers` and
/// `jobs`.
pub(crate) fn daemon_status(providers: Vec<StatusProvider>, jobs: Vec<StatusJob>) -> Status {
    Status {
        daemon: StatusDaemon {
            pid: 4242,
            version: "20261014.2045.0".into(),
            protocol: 1,
            started_at: "2026-09-21T14:13:20Z".into(),
            socket: Some("~/.local/share/tagteam/daemon.sock".into()),
            log: "~/.local/state/tagteam/tagteam.log".into(),
        },
        notifications: StatusNotifications {
            enabled: false,
            backend: None,
            events: vec![],
        },
        providers,
        jobs,
        subscribers: 1,
    }
}

/// `provider`'s job in `mode` (D4.3), with `autoswitch`, waiting for nothing, with no backoff.
pub(crate) fn daemon_job(provider: &str, mode: &str, autoswitch: bool) -> StatusProvider {
    StatusProvider {
        provider: provider.into(),
        mode: mode.into(),
        autoswitch,
        waiting_for: None,
        holder_pid: None,
        backoff: None,
        last_tick_at: None,
        next_tick_at: None,
        last_outcome: None,
        last_switch: None,
    }
}

/// Links `state` to the daemon at `NOW_MS`, by a link of its own, and answers the `status` it
/// asks for: live mode. The re-read the link asks for is still to come. Returns the link.
pub(crate) fn go_live(state: &mut State, status: Status) -> u64 {
    static LINKS: AtomicU64 = AtomicU64::new(100);
    let link = LINKS.fetch_add(1, Ordering::Relaxed);
    let cmds = crate::update::update(state, Msg::Live(Live::Up { link }), NOW_MS);
    assert_eq!(cmds, [Cmd::FetchStatus { link }]);
    let result = Ok(status);
    crate::update::update(state, Msg::DaemonStatus { link, result }, NOW_MS);
    assert!(state.daemon.live());
    link
}
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
    use tagteam_engine::views::HistoryView;

    use super::*;
    use crate::host::{AccountRef, Ask};
    use crate::source::{AccountsSnapshot, CollectSummary, SourceError};
    use crate::term::repaint;

    /// What the fake terminal and input were asked to do, in order.
```

with:

```rust
    use tagteam_engine::views::HistoryView;

    use super::*;
    use crate::host::{AccountRef, Ask};
    use crate::live::Live;
    use crate::source::{AccountsSnapshot, CollectSummary, SourceError};
    use crate::term::repaint;

    /// What the fake terminal and input were asked to do, in order.
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
            tokens_provider: ProviderId::new("claude-code"),
            tz: TimeZone::UTC,
            signal: Cancel::new(),
            shutdown: Cancel::new(),
        }
    }

    fn account_a() -> AccountRef {
```

with:

```rust
            tokens_provider: ProviderId::new("claude-code"),
            tz: TimeZone::UTC,
            signal: Cancel::new(),
            shutdown: Cancel::new(),
            daemon: None,
        }
    }

    fn account_a() -> AccountRef {
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
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
```

with:

```rust
        fn with(
            opts: TuiOptions,
            sources: SourceFactory,
            host: impl FnOnce(&TuiOptions) -> Scripted,
        ) -> Fixture {
            Fixture::build(opts, sources, host, None)
        }

        /// A TUI that follows a daemon (`TuiOptions::daemon`), with `host`, and a daemon lane
        /// whose `status` is `status`'s answer (spec §8). No live thread runs: a test puts what
        /// it would pass on in the lanes' live queue.
        fn with_daemon(
            sources: SourceFactory,
            host: impl FnOnce(&TuiOptions) -> Scripted,
            status: StatusFetch,
        ) -> Fixture {
            let opts = TuiOptions {
                daemon: Some(PathBuf::from("/nonexistent/daemon.sock")),
                ..options()
            };
            Fixture::build(opts, sources, host, Some(status))
        }

        fn build(
            opts: TuiOptions,
            sources: SourceFactory,
            host: impl FnOnce(&TuiOptions) -> Scripted,
            status: Option<StatusFetch>,
        ) -> Fixture {
            let host: Arc<dyn Host> = Arc::new(host(&opts));
            let (tx, rx) = mpsc::channel();
            let lanes = Lanes::spawn(None, &sources, &host, &tx, &opts.shutdown, status);
            let log = Log::default();
            Fixture {
                opts,
                tx,
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
        assert_eq!((exit.code, exit.report), (1, None));
        assert_eq!(drops.load(Ordering::SeqCst), 4, "every lane was joined");
        assert_eq!(shutdown.requested(), Some(libc::SIGINT));
    }
}
```

with:

```rust
        assert_eq!((exit.code, exit.report), (1, None));
        assert_eq!(drops.load(Ordering::SeqCst), 4, "every lane was joined");
        assert_eq!(shutdown.requested(), Some(libc::SIGINT));
    }

    /// Claude Code's job switching accounts, after `wait`; `started` is set when it begins.
    fn slow_status(wait: Duration, started: &Arc<AtomicBool>) -> StatusFetch {
        let started = Arc::clone(started);
        Arc::new(move || {
            started.store(true, Ordering::SeqCst);
            thread::sleep(wait);
            Ok(crate::testutil::daemon_status(
                vec![crate::testutil::daemon_job("claude-code", "auto", true)],
                vec![],
            ))
        })
    }

    /// Waits up to 5 s for `flag`.
    fn raised(flag: &AtomicBool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !flag.load(Ordering::SeqCst) {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
        true
    }

    #[test]
    fn the_first_frame_never_waits_for_the_daemon_and_its_status_starts_live_mode() {
        // Spec §6.1, §8: the TUI starts in timer mode and switches once the first `status`
        // arrives, however long the daemon takes.
        let started = Arc::new(AtomicBool::new(false));
        let status = slow_status(Duration::from_millis(400), &started);
        let fx = Fixture::with_daemon(quick(), |_| idle(), status);
        // The live thread subscribed before the TUI's first read returned.
        fx.tx.send(Msg::Live(Live::Up { link: 1 })).unwrap();
        let ending = {
            let mut ui = fx.ui();
            let begun = Instant::now();
            ui.start();
            assert!(
                begun.elapsed() < Duration::from_millis(300),
                "{:?}",
                begun.elapsed()
            );
            assert!(
                ui.state.accounts.snapshot().is_some(),
                "drawn with the accounts"
            );
            assert!(raised(&started), "the status was asked for at once");
            assert!(!ui.state.daemon.live(), "timer mode until it comes");
            while !ui.state.daemon.live() {
                let msg = fx.rx.recv_timeout(Duration::from_secs(5));
                ui.deliver(msg.expect("the daemon lane answered"));
            }
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn q_never_waits_for_a_status_the_daemon_lane_still_waits_for() {
        // Spec §4.3, §10.2: the daemon lane is not joined. Its `status` from a daemon that
        // never answers takes 2 s (D5.7); the shutdown does not wait for it.
        let started = Arc::new(AtomicBool::new(false));
        let status = slow_status(Duration::from_secs(3), &started);
        let fx = Fixture::with_daemon(quick(), |_| idle(), status);
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            ui.execute(vec![Cmd::FetchStatus { link: 1 }]);
            assert!(raised(&started), "the fetch began");
            fx.key('q');
            ui.run()
        };
        let joining = Instant::now();
        let exit = fx.finish(ending);
        assert!(
            joining.elapsed() < Duration::from_millis(500),
            "{:?}",
            joining.elapsed()
        );
        assert_eq!((exit.code, exit.report), (0, None));
    }

    #[test]
    fn a_daemon_lane_that_panics_ends_the_tui_with_1_and_says_so() {
        let status: StatusFetch = Arc::new(|| panic!("the daemon lane's fetch panicked"));
        let fx = Fixture::with_daemon(quick(), |_| idle(), status);
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            ui.execute(vec![Cmd::FetchStatus { link: 1 }]);
            ui.run()
        };
        let exit = fx.finish(ending);
        assert_eq!(exit.code, 1);
        assert_eq!(
            exit.report.as_deref(),
            Some("tagteam: the TUI's daemon lane panicked; the log has the details\n")
        );
    }

    /// The messages waiting on `rx`, applied as the loop applies them, until `done` holds; at
    /// most 5 s. How many of them woke the loop for the live queue.
    fn pump(
        ui: &mut Ui<'_, FakeTty, Keys>,
        rx: &Receiver<Msg>,
        done: impl Fn(&State) -> bool,
    ) -> usize {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut wakes = 0;
        while !done(&ui.state) {
            let left = deadline.saturating_duration_since(Instant::now());
            let msg = rx
                .recv_timeout(left)
                .expect("a message before the deadline");
            wakes += usize::from(matches!(msg, Msg::LiveReady));
            ui.deliver(msg);
        }
        wakes
    }

    #[test]
    fn a_flood_during_a_suspension_stays_within_the_queue_and_a_snapshot_recovers_it() {
        // C-R17: while `R`'s session has the terminal the loop takes no messages, and the live
        // thread goes on. Its backlog never passes `LIVE_QUEUE`, and it wakes the loop once. The
        // overflow is a disconnect; the next link's snapshot brings live mode back.
        let queue: Arc<std::sync::OnceLock<Arc<LiveQueue>>> = Arc::default();
        let flooding = Arc::clone(&queue);
        let started = Arc::new(AtomicBool::new(false));
        let status = slow_status(Duration::ZERO, &started);
        let fx = Fixture::with_daemon(
            quick(),
            move |_| Scripted {
                act: never(),
                // The live thread, while the session runs: a flood until the queue is full, which
                // drops the link; then the next link and its first events.
                foreground: Box::new(move |_| {
                    let q = flooding.get().expect("the queue is set");
                    let changed = || Live::Event(tagteam_daemon::protocol::Event::Changed);
                    let mut pushed = 0;
                    while q.push(changed()) == crate::live::Delivery::Taken {
                        pushed += 1;
                        assert!(q.len() <= LIVE_QUEUE, "{}", q.len());
                    }
                    assert_eq!(pushed, LIVE_QUEUE);
                    let up = q.push(Live::Up { link: 2 });
                    assert_eq!(up, crate::live::Delivery::Taken);
                    for _ in 0..10 {
                        assert_eq!(q.push(changed()), crate::live::Delivery::Taken);
                    }
                    assert!(q.len() <= LIVE_QUEUE);
                    done(0, "", "")
                }),
            },
            status,
        );
        queue.set(Arc::clone(&fx.lanes.live)).ok();
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            let up = fx.lanes.live.push(Live::Up { link: 1 });
            assert_eq!(up, crate::live::Delivery::Taken);
            pump(&mut ui, &fx.rx, |s| s.daemon.live());
            let run = Action::Run(account_a());
            ui.state.running = Some(run.clone());
            ui.execute(vec![Cmd::Suspend(run)]);
            assert_eq!(
                fx.lanes.live.len(),
                12,
                "a Down, then the next link's Up and 10 events"
            );
            assert!(
                ui.state.daemon.live(),
                "no message was taken while suspended"
            );
            let wakes = pump(&mut ui, &fx.rx, |s| !s.daemon.live());
            assert_eq!(wakes, 1, "one wake for the whole flood");
            assert!(fx.lanes.live.is_empty(), "taken whole");
            pump(&mut ui, &fx.rx, |s| s.daemon.live());
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    /// A daemon on `path` that greets each connection and takes its subscription, and counts
    /// them. It closes the first when `close_first` is sent to, and holds the others open.
    fn daemon_at(path: &std::path::Path) -> (Sender<()>, Arc<AtomicUsize>) {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;
        let listener = UnixListener::bind(path).unwrap();
        let (close_first, closing) = mpsc::channel::<()>();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&accepted);
        thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let hello = "{\"schemaVersion\":1,\"event\":\"hello\",\"protocol\":1,\"version\":\"20261014.2045.0\",\"pid\":4242,\"startedAt\":\"2026-09-21T14:13:20Z\"}\n";
                stream.write_all(hello.as_bytes()).unwrap();
                let mut request = String::new();
                BufReader::new(&stream).read_line(&mut request).unwrap();
                let ok = "{\"schemaVersion\":1,\"id\":1,\"ok\":true,\"result\":{}}\n";
                stream.write_all(ok.as_bytes()).unwrap();
                if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                    let _ = closing.recv();
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                } else {
                    held.push(stream);
                }
            }
        });
        (close_first, accepted)
    }

    #[test]
    fn a_failure_judged_after_the_live_thread_replaced_its_link_leaves_the_new_link_alone() {
        // C-R21, in the order a suspension (§7.3) leaves it: while the loop takes no messages,
        // link 1's `status` fails, and the live thread then loses link 1 and brings link 2 up.
        // On resume the failure ends link 1 by name; the live thread holds link 2, which carries
        // on: no third connection, and no Down.
        let dir = tempfile::Builder::new()
            .prefix("tt")
            .tempdir_in("/tmp")
            .or_else(|_| tempfile::Builder::new().prefix("tt").tempdir())
            .unwrap();
        let path = dir.path().join("daemon.sock");
        let (close_first, accepted) = daemon_at(&path);
        // The daemon lane: the second `status` fails once `fail` is sent to; the others answer.
        let calls = Arc::new(AtomicUsize::new(0));
        let (fail, failing) = mpsc::channel::<()>();
        let failing = Mutex::new(failing);
        let counted = Arc::clone(&calls);
        let status: StatusFetch = Arc::new(move || {
            if counted.fetch_add(1, Ordering::SeqCst) == 1 {
                let _ = failing.lock().unwrap().recv();
                return Err("the daemon closed the connection".into());
            }
            Ok(crate::testutil::daemon_status(
                vec![crate::testutil::daemon_job("claude-code", "auto", true)],
                vec![],
            ))
        });
        let fx = Fixture::with_daemon(quick(), |_| idle(), status);
        let backoff = Backoff {
            first: Duration::from_millis(20),
            most: Duration::from_millis(100),
            mismatch: Duration::from_secs(1),
        };
        crate::live::spawn(path.clone(), backoff, Arc::clone(&fx.lanes.live), || {}).unwrap();
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            pump(&mut ui, &fx.rx, |s| s.daemon.live());
            // An event asks for `status` again, and the lane takes it.
            let mode = tagteam_daemon::protocol::Event::Mode {
                provider: "claude-code".into(),
                mode: "collect".into(),
                waiting_for: None,
                holder_pid: None,
                backoff: None,
            };
            fx.lanes.live.push(Live::Event(mode));
            let deadline = Instant::now() + Duration::from_secs(5);
            while calls.load(Ordering::SeqCst) < 2 {
                assert!(Instant::now() < deadline, "the second status was asked for");
                if let Ok(msg) = fx.rx.recv_timeout(Duration::from_millis(10)) {
                    ui.deliver(msg);
                }
            }
            // Suspended: the failure is queued, then link 1 goes and link 2 comes.
            fail.send(()).unwrap();
            let mut held = Vec::new();
            loop {
                let msg = fx
                    .rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("the failure");
                let failed = matches!(msg, Msg::DaemonStatus { result: Err(_), .. });
                held.push(msg);
                if failed {
                    break;
                }
            }
            close_first.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while fx.lanes.live.len() < 2 {
                assert!(Instant::now() < deadline, "link 2 came up");
                thread::sleep(Duration::from_millis(5));
            }
            // Resumed, in that order.
            for msg in held {
                ui.deliver(msg);
            }
            pump(&mut ui, &fx.rx, |s| s.daemon.live());
            thread::sleep(Duration::from_millis(300));
            assert_eq!(accepted.load(Ordering::SeqCst), 2, "link 2 carried on");
            assert!(fx.lanes.live.is_empty(), "no Down");
            assert!(ui.state.daemon.live());
            fx.key('q');
            ui.run()
        };
        assert_eq!(fx.finish(ending).code, 0);
    }

    #[test]
    fn a_key_queued_behind_a_burst_is_handled_within_one_frame() {
        // C-R17: the loop applies every message waiting before it draws, so a burst costs one
        // frame, not one a message, and a key behind it waits for none of them.
        let fx = Fixture::new(quick(), |_| idle());
        let ending = {
            let mut ui = fx.ui();
            ui.start();
            fx.frames.take();
            for width in 0..200 {
                let resize = Msg::Resize {
                    width: 100 + width % 20,
                    height: 40,
                };
                fx.tx.send(resize).unwrap();
            }
            fx.key('q');
            ui.run()
        };
        assert!(
            fx.frames.seen().len() <= 1,
            "{} frames for 201 messages",
            fx.frames.seen().len()
        );
        assert_eq!(fx.finish(ending).code, 0);
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p tagteam-tui --lib`

Expected: FAIL: the build stops with errors such as ``error[E0433]: failed to resolve: use of undeclared type `Live` `` and ``error[E0412]: cannot find type `Event` in this scope ``, from the new module's tests, whose code is still to come.

- [ ] **Step 3: Write the code**

The module's code, then the messages, the state and `update` that carry it, the runtime's daemon lane and live thread, and the binary's socket.

In `crates/tagteam-tui/src/daemon/mod.rs`, put this above the tests, with one blank line between:

```rust
//! Live mode (spec §6.3, §8): the daemon's link as the state machine sees it. What the live
//! thread learns, and the daemon lane's `status` replies, arrive as messages; what they ask for
//! leaves as commands. The store stays the source of truth: an event is a reason to read the
//! views again, and only `status` says what the daemon does (§7.4, §8).

use tagteam_core::{AccountId, ProviderId};
use tagteam_daemon::protocol::{Event, Status, StatusProvider};

use crate::live::Live;
use crate::msg::Cmd;
use crate::screens;
use crate::state::{Screen, State};

/// Spec §6.3: what an event asks for is read this long after it, and the events that come
/// meanwhile share that read.
pub const REREAD_MS: i64 = 250;
/// Spec §6.3: while linked, `status` is fetched this often, and in live mode `accounts` read.
pub const POLL_MS: i64 = 60_000;
/// Spec §8: after `stopping`, the header names its reason this long, unless a link comes up.
pub const STOPPING_MS: i64 = 30_000;
/// C-R19: a `status` asked for and not answered this long after is given up on, and the link
/// with it: more than the daemon lane's own 2.25 s bound (D5.7), with another link's fetch
/// queued behind it.
pub const STATUS_WAIT_MS: i64 = 5_000;
/// C-R20: the first collection waits at most this long for the daemon's first `status`, no
/// longer than the daemon lane's own bound.
pub const SETTLE_MS: i64 = 2_000;

/// §8: the auto kinds that say an account's reading or state changed.
const VIEW_KINDS: [&str; 4] = [
    "poll",
    "switch",
    "account-quarantined",
    "account-unquarantined",
];

/// The daemon as the TUI knows it (spec §8).
#[derive(Default)]
pub struct DaemonState {
    /// The link followed now, by the live thread's name for it (C-R21): a `status` reply asked
    /// for under another link is dropped, and a link given up on is ended by its name.
    link: Option<u64>,
    /// This link's last `status`. Live mode is a link with one (spec §8).
    status: Option<Status>,
    /// `stopping`'s reason, and until when the header names it.
    stopping: Option<(String, i64)>,
    /// The protocol of a daemon this TUI does not speak.
    mismatch: Option<u32>,
    /// When the next 60 s fetch of `status` is due.
    next_poll_ms: i64,
    /// When the reads the last events asked for are made, all at once.
    reread_ms: Option<i64>,
    /// They read the views: an account's reading or state changed.
    reread_views: bool,
    /// They read every view shown, the token report among them: the link came up.
    reread_all: bool,
    /// They fetch `status`: a provider job ran, and publishes its state as the run ends.
    reread_status: bool,
    /// When the `status` out now, the link's only one, is given up on (C-R19, C-R22).
    status_due_ms: Option<i64>,
    /// `status` was asked for again while one was out: it is asked once that one is answered
    /// (C-R22).
    status_again: bool,
    /// C-R20: until when the first collection waits to know who collects: no daemon, or the
    /// daemon's first `status`. `None` once known, and for a TUI that follows no daemon.
    unsettled_until: Option<i64>,
}

impl DaemonState {
    /// Live mode: subscribed, and the daemon's `status` has come since (spec §8).
    pub fn live(&self) -> bool {
        self.link.is_some() && self.status.is_some()
    }

    /// The daemon's last `status`, in live mode.
    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref().filter(|_| self.link.is_some())
    }

    /// `provider`'s job in the last `status`, in live mode.
    pub fn job(&self, provider: &ProviderId) -> Option<&StatusProvider> {
        self.status()?
            .providers
            .iter()
            .find(|p| p.provider == provider.as_str())
    }

    /// The reason the daemon gave for stopping, while the header names it.
    pub fn stopping(&self) -> Option<&str> {
        self.stopping.as_ref().map(|(reason, _)| reason.as_str())
    }

    /// The protocol of a daemon that speaks another.
    pub fn mismatch(&self) -> Option<u32> {
        self.mismatch
    }

    /// C-R20: whether the TUI knows who collects: no daemon answered, the daemon's first
    /// `status` came, or `SETTLE_MS` passed.
    pub fn settled(&self) -> bool {
        self.unsettled_until.is_none()
    }

    /// C-R20: a TUI that follows a daemon, started at `now_ms`, waits for it before its first
    /// collection.
    pub(crate) fn await_daemon(&mut self, now_ms: i64) {
        self.unsettled_until = Some(now_ms + SETTLE_MS);
    }
}

/// What the live thread learned (spec §8).
pub(crate) fn on_live(state: &mut State, live: Live, now_ms: i64) -> Vec<Cmd> {
    match live {
        Live::Up { link } => up(state, link, now_ms),
        Live::Down => {
            down(state);
            vec![]
        }
        Live::Mismatch { server } => {
            down(state);
            state.daemon.stopping = None;
            state.daemon.mismatch = Some(server);
            state.dirty = true;
            vec![]
        }
        Live::Event(event) => on_event(state, event, now_ms),
    }
}

/// Spec §8, "Connecting": link `link` is up. `status` is fetched at once. Every view shown is
/// read again `REREAD_MS` later, so that its snapshot follows the subscription and no change
/// falls between the two, while the first frame's read (§6.1) is left to finish.
fn up(state: &mut State, link: u64, now_ms: i64) -> Vec<Cmd> {
    let d = &mut state.daemon;
    d.link = Some(link);
    d.status = None;
    d.status_due_ms = None;
    d.status_again = false;
    d.stopping = None;
    d.mismatch = None;
    d.next_poll_ms = now_ms + POLL_MS;
    d.reread_ms = Some(now_ms + REREAD_MS);
    d.reread_views = true;
    d.reread_all = true;
    d.reread_status = false;
    state.dirty = true;
    fetch(state, now_ms).into_iter().collect()
}

/// The link is down, or none came up: timer mode (spec §8), and the TUI knows who collects
/// (C-R20). A mismatch seen before is gone too, since no daemon answered this time.
fn down(state: &mut State) {
    let d = &mut state.daemon;
    if d.mismatch.take().is_some() {
        state.dirty = true;
    }
    if d.unsettled_until.take().is_some() {
        state.dirty = true;
    }
    if d.link.take().is_none() {
        return;
    }
    d.status = None;
    d.status_due_ms = None;
    d.status_again = false;
    d.reread_ms = None;
    d.reread_views = false;
    d.reread_all = false;
    d.reread_status = false;
    state.dirty = true;
}

/// `status`, on the daemon lane, for this link. One is out at a time (C-R22): one asked for
/// while another is out is asked once that one is answered, so that it reads what came after
/// it. The one out is answered within `STATUS_WAIT_MS`, or the link is given up on (C-R19).
/// Nothing is asked once the shutdown has begun.
fn fetch(state: &mut State, now_ms: i64) -> Option<Cmd> {
    let d = &mut state.daemon;
    let link = d.link.filter(|_| state.quitting.is_none())?;
    if d.status_due_ms.is_some() {
        d.status_again = true;
        return None;
    }
    d.status_due_ms = Some(now_ms + STATUS_WAIT_MS);
    Some(Cmd::FetchStatus { link })
}

/// C-R19: link `link` does not work. Timer mode, and the live thread is told to end that link,
/// if it still holds it (C-R21), which it then connects again with backoff.
fn broken(state: &mut State, link: u64) -> Vec<Cmd> {
    down(state);
    vec![Cmd::DropLink { link }]
}

/// Spec §8, "What each event does". An account's change re-reads the views; `mode` fetches
/// `status`, whose words the header shows; `stopping` names its reason. Every event of a
/// provider job's run (auto's kinds, `usage`) also fetches `status`, since the daemon publishes
/// a job's state when its run ends, after its events (D5.4): `mode`'s own fetch can read the
/// state from before the run (C-R5). All but `mode`'s are debounced. A kind this TUI does not
/// know reads nothing.
fn on_event(state: &mut State, event: Event, now_ms: i64) -> Vec<Cmd> {
    if state.daemon.link.is_none() || state.quitting.is_some() {
        return vec![];
    }
    match event {
        Event::Auto { kind, .. } => {
            let views = VIEW_KINDS.contains(&kind.as_str());
            reread(state, now_ms, views, true);
        }
        Event::Usage { .. } => reread(state, now_ms, true, true),
        Event::Changed => reread(state, now_ms, true, false),
        Event::Mode { .. } => return fetch(state, now_ms).into_iter().collect(),
        // C-R18: the link ends here, whether or not the daemon goes on to close it; the live
        // thread drops it and connects again with backoff.
        Event::Stopping { reason } => {
            down(state);
            state.daemon.stopping = Some((reason, now_ms + STOPPING_MS));
            state.dirty = true;
        }
        Event::TokenIndex { .. } | Event::Unknown { .. } => {}
    }
    vec![]
}

/// What an event asks for is read `REREAD_MS` from the first event that asks, once for every
/// event until then: the views, `status`, or both.
fn reread(state: &mut State, now_ms: i64, views: bool, status: bool) {
    let d = &mut state.daemon;
    d.reread_ms.get_or_insert(now_ms + REREAD_MS);
    d.reread_views |= views;
    d.reread_status |= status;
}

/// The daemon lane's `status` for link `link` (spec §8): the link's one out (C-R22). A reply
/// asked for under another link is dropped (C-R21). A link's first reply starts live mode, and
/// says who collects (C-R20); the `status` asked for meanwhile is asked now. One that failed
/// ends the link (C-R19): the daemon holds it but does not answer.
pub(crate) fn on_status(
    state: &mut State,
    link: u64,
    result: Result<Status, String>,
    now_ms: i64,
) -> Vec<Cmd> {
    let d = &mut state.daemon;
    if d.link != Some(link) {
        return vec![];
    }
    d.status_due_ms = None;
    match result {
        Ok(status) => {
            d.status = Some(status);
            d.unsettled_until = None;
            state.dirty = true;
            if !std::mem::take(&mut state.daemon.status_again) {
                return vec![];
            }
            fetch(state, now_ms).into_iter().collect()
        }
        Err(_) => broken(state, link),
    }
}

/// Spec §6.3 and §8 on the UI's tick: the end of `stopping`'s words, and of the first
/// collection's wait (C-R20); a link whose `status` did not come (C-R19); the events' debounced
/// reads; and every 60 s, `status`, and in live mode `accounts` (and `history` when History is
/// open).
pub(crate) fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    let d = &mut state.daemon;
    if d.stopping
        .as_ref()
        .is_some_and(|(_, until)| now_ms >= *until)
    {
        d.stopping = None;
        state.dirty = true;
    }
    if d.unsettled_until.is_some_and(|until| now_ms >= until) {
        d.unsettled_until = None;
        state.dirty = true;
    }
    let Some(link) = d.link.filter(|_| state.quitting.is_none()) else {
        return vec![];
    };
    if d.status_due_ms.is_some_and(|due| now_ms >= due) {
        return broken(state, link);
    }
    let (mut views, mut all, mut fetch_now) = (false, false, false);
    if d.reread_ms.is_some_and(|at| now_ms >= at) {
        d.reread_ms = None;
        views = std::mem::take(&mut d.reread_views);
        all = std::mem::take(&mut d.reread_all);
        fetch_now = std::mem::take(&mut d.reread_status);
    }
    if now_ms >= d.next_poll_ms {
        d.next_poll_ms = now_ms + POLL_MS;
        fetch_now = true;
        views |= d.live();
    }
    let mut cmds = Vec::new();
    if fetch_now {
        cmds.extend(fetch(state, now_ms));
    }
    if views {
        cmds.push(state.load_accounts());
        if state.screen == Screen::History {
            cmds.extend(screens::history::reload(state, now_ms));
        }
        if all && state.screen == Screen::Tokens {
            cmds.extend(screens::tokens::requery(state, now_ms));
        }
    }
    cmds
}

/// Spec §6.3: whether something else collects for `provider`. In live mode, that is a daemon
/// job in `auto` or `collect`, or one waiting for another engine, which collects for itself. A
/// job that is `idle` or waits for a newer binary (`schema`), and a provider the daemon runs no
/// job for, leave it to the TUI.
pub(crate) fn collected_elsewhere(state: &State, provider: &ProviderId) -> bool {
    state
        .daemon
        .job(provider)
        .is_some_and(|job| match job.mode.as_str() {
            "auto" | "collect" => true,
            "waiting" => job.waiting_for.as_deref() == Some("engine-lock"),
            _ => false,
        })
}

/// The accounts the timer offers the collector (spec §6.2, §6.3): those shown whose provider
/// nothing else collects for. In timer mode, every one shown.
pub(crate) fn to_collect(state: &State) -> Vec<AccountId> {
    let Some(snap) = state.accounts.snapshot() else {
        return vec![];
    };
    snap.providers
        .iter()
        .filter(|p| !collected_elsewhere(state, &p.provider))
        .flat_map(|p| p.accounts.iter().map(|v| v.row.id.clone()))
        .collect()
}
```

In `crates/tagteam-tui/src/lib.rs`, replace:

```rust
#![forbid(unsafe_code)]

pub mod actions;
mod ask;
pub mod daemon;
pub mod host;
mod input;
pub mod lanes;
pub mod live;
```

with:

```rust
#![forbid(unsafe_code)]

pub mod actions;
mod ask;
pub mod daemon;
pub mod host;
mod input;
pub mod lanes;
pub mod live;
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust

use std::sync::mpsc::Sender;

use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::tokens::{IndexProgress, IndexState, RefreshOutcome, TokenQuery, TokenReport};
use tagteam_engine::views::HistoryView;

use crate::host::{Action, ActionOutcome};
use crate::lanes::Coalesce;
use crate::source::{AccountsSnapshot, CollectSummary, SourceError};

/// The views lane's requests: store-served reads, no network (spec §4.3).
#[derive(Debug, Clone, PartialEq, Eq)]
```

with:

```rust

use std::sync::mpsc::Sender;

use tagteam_core::{AccountId, ProviderId};
use tagteam_daemon::protocol::Status;
use tagteam_engine::tokens::{IndexProgress, IndexState, RefreshOutcome, TokenQuery, TokenReport};
use tagteam_engine::views::HistoryView;

use crate::host::{Action, ActionOutcome};
use crate::lanes::Coalesce;
use crate::live::Live;
use crate::source::{AccountsSnapshot, CollectSummary, SourceError};

/// The views lane's requests: store-served reads, no network (spec §4.3).
#[derive(Debug, Clone, PartialEq, Eq)]
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
pub enum IndexReq {
    Refresh { provider: ProviderId },
}

impl Coalesce for ViewsReq {
    /// Any two of the same kind: only the newest read is worth making.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
```

with:

```rust
pub enum IndexReq {
    Refresh { provider: ProviderId },
}

/// The daemon lane's requests: the daemon's `status`, over a connection of its own (spec §4.3,
/// §8). `link` is the link it is asked for (C-R21).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonReq {
    Status { link: u64 },
}

impl Coalesce for ViewsReq {
    /// Any two of the same kind: only the newest read is worth making.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
        self == other
    }
}

impl Coalesce for ActionsReq {
    /// Two collections merge; an action never does.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
```

with:

```rust
        self == other
    }
}

impl Coalesce for DaemonReq {
    /// Any two: a burst of events costs one request, and the newest link's is the one made
    /// (spec §8).
    fn same_as(&self, _other: &Self) -> bool {
        true
    }
}

impl Coalesce for ActionsReq {
    /// Two collections merge; an action never does.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
    },
    LaneDied {
        lane: &'static str,
    },
}

/// A question from the CLI path, and where its answer goes (spec §7.2).
pub struct Prompt {
```

with:

```rust
    },
    LaneDied {
        lane: &'static str,
    },
    /// What the live thread learned (spec §8). The runtime makes these from the live queue.
    Live(Live),
    /// The live queue has something: the runtime takes it all, and applies each as `Live`
    /// (C-R17).
    LiveReady,
    /// The daemon's `status`, from the daemon lane, for link `link` (spec §8, C-R21).
    DaemonStatus {
        link: u64,
        result: Result<Status, String>,
    },
}

/// A question from the CLI path, and where its answer goes (spec §7.2).
pub struct Prompt {
```

In `crates/tagteam-tui/src/msg.rs`, replace:

```rust
        code: i32,
    },
    /// Ctrl-Z (spec §10.1).
    Stop,
}

#[cfg(test)]
mod tests {
```

with:

```rust
        code: i32,
    },
    /// Ctrl-Z (spec §10.1).
    Stop,
    /// The daemon's `status` for link `link`, on the daemon lane (spec §8, C-R21).
    FetchStatus {
        link: u64,
    },
    /// End link `link`, which no longer works, if the live thread still holds it; it connects
    /// again (C-R19, C-R21).
    DropLink {
        link: u64,
    },
}

#[cfg(test)]
mod tests {
```

In `crates/tagteam-tui/src/state.rs`, replace:

```rust

use tagteam_core::{AccountId, ProviderId};

use crate::actions::Overlay;
use crate::host::Action;
use crate::msg::Cmd;
use crate::screens::accounts::AccountsState;
use crate::screens::history::HistoryState;
```

with:

```rust

use tagteam_core::{AccountId, ProviderId};

use crate::actions::Overlay;
use crate::daemon::DaemonState;
use crate::host::Action;
use crate::msg::Cmd;
use crate::screens::accounts::AccountsState;
use crate::screens::history::HistoryState;
```

In `crates/tagteam-tui/src/state.rs`, replace:

```rust
    pub provider: Option<ProviderId>,
    pub accounts: AccountsState,
    pub tokens: TokensState,
    pub history: HistoryState,
    pub overlay: Option<Overlay>,
    pub toast: Option<Toast>,
    /// A failed read's error, in the CLI's words (spec §10.4).
    pub banner: Option<String>,
```

with:

```rust
    pub provider: Option<ProviderId>,
    pub accounts: AccountsState,
    pub tokens: TokensState,
    pub history: HistoryState,
    /// The daemon's link, and what it says (spec §8).
    pub daemon: DaemonState,
    pub overlay: Option<Overlay>,
    pub toast: Option<Toast>,
    /// A failed read's error, in the CLI's words (spec §10.4).
    pub banner: Option<String>,
```

In `crates/tagteam-tui/src/state.rs`, replace:

```rust
            provider,
            accounts: AccountsState::default(),
            tokens,
            history: HistoryState::default(),
            overlay: None,
            toast: None,
            banner: None,
            banner_from: None,
```

with:

```rust
            provider,
            accounts: AccountsState::default(),
            tokens,
            history: HistoryState::default(),
            daemon: DaemonState::default(),
            overlay: None,
            toast: None,
            banner: None,
            banner_from: None,
```

In `crates/tagteam-tui/src/update.rs`, replace:

```rust

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::actions::{self, Overlay};
use crate::msg::{Cmd, Msg};
use crate::screens;
use crate::state::{BannerKind, REFRESH_MS, Screen, State};

```

with:

```rust

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::actions::{self, Overlay};
use crate::daemon;
use crate::msg::{Cmd, Msg};
use crate::screens;
use crate::state::{BannerKind, REFRESH_MS, Screen, State};

```

In `crates/tagteam-tui/src/update.rs`, replace:

```rust
                return vec![];
            }
            quit(state, 1)
        }
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
```

with:

```rust
                return vec![];
            }
            quit(state, 1)
        }
        Msg::Live(live) => daemon::on_live(state, live, now_ms),
        // The runtime takes the live queue at each wake, and applies what it holds as `Live`.
        Msg::LiveReady => vec![],
        Msg::DaemonStatus { link, result } => daemon::on_status(state, link, result, now_ms),
    }
}

/// Spec §6.1: the TUI collects after drawing. The runtime calls this after each frame it
/// draws. The first frame asks for the token index's first refresh (B5). The first frame that
/// shows accounts nothing else collects for (§6.3) asks for the first collection of them, once
/// no action holds the actions lane (spec §7.2), and once the TUI knows whether a daemon
/// collects for them (C-R20); until a collection has been asked for, `gens.collect` is 0. The
/// timer collects from then on (§6.2).
pub fn drawn(state: &mut State) -> Vec<Cmd> {
    let mut cmds = screens::tokens::index::first_refresh(state);
    let waits = state.running.is_some() || !state.daemon.settled();
    if state.gens.collect > 0 || state.quitting.is_some() || waits {
        return cmds;
    }
    let shown = daemon::to_collect(state);
    if !shown.is_empty() {
        cmds.push(state.collect_due(shown));
    }
    cmds
```

In `crates/tagteam-tui/src/update.rs`, replace:

```rust
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
    if state.running.is_none() && !shown.is_empty() {
        cmds.push(state.collect_due(shown));
    }
    cmds
}

```

with:

```rust
        state.dirty = true;
    }
    // Spec §6.4: the token index's own timers, while Tokens is open.
    let mut cmds = screens::tokens::index::on_tick(state, now_ms);
    // Spec §6.3, §8: the daemon's link.
    cmds.extend(daemon::on_tick(state, now_ms));
    if state.quitting.is_some() || now_ms < state.refresh.next_ms {
        return cmds;
    }
    // Spec §6.2: every 30 s the views are re-read, and the shown accounts offered to the
    // collector, unless an action holds the actions lane (spec §7.2). In live mode the views
    // are read on the daemon's events and every 60 s instead, and the collector is offered
    // only the accounts nothing else collects for (§6.3).
    state.refresh.next_ms = now_ms + REFRESH_MS;
    if !state.daemon.live() {
        cmds.push(state.load_accounts());
        if state.screen == Screen::History {
            cmds.extend(screens::history::reload(state, now_ms));
        }
    }
    let due = daemon::to_collect(state);
    if state.running.is_none() && !due.is_empty() {
        cmds.push(state.collect_due(due));
    }
    cmds
}

```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
//! shutdown every way out goes through (§10.2).

use std::io;
use std::panic::{self, AssertUnwindSafe};
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
use crate::msg::{ActionsReq, Cmd, IndexReq, Msg, ReportReq, ViewsReq};
use crate::screens::tokens::TokensState;
use crate::source::{Source, SourceFactory};
use crate::state::{Screen, State};
use crate::term::{self, RealTty, Tty};
```

with:

```rust
//! shutdown every way out goes through (§10.2).

use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jiff::tz::TimeZone;
use tagteam_core::ProviderId;
use tagteam_daemon::protocol::Status;
use tagteam_provider::Cancel;

use crate::ask::ChannelAsk;
use crate::host::{Action, ActionOutcome, Host};
use crate::input::{Input, Pause};
use crate::lanes::Lane;
use crate::live::{self, Backoff, LIVE_QUEUE, LiveQueue};
use crate::msg::{ActionsReq, Cmd, DaemonReq, IndexReq, Msg, ReportReq, ViewsReq};
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
/// The index lane passes on a refresh's progress at most this often: the loop's own pace.
const PROGRESS_EVERY: Duration = WAKE;
/// Why a hand-over of the terminal was refused (§7.3 step 1): the input thread could still read
/// keys typed for the terminal's next owner.
```

with:

```rust
/// The loop wakes at least this often, to look at the signal token and to tick (§4.3).
const WAKE: Duration = Duration::from_millis(250);
/// How long the first frame waits for the first `accounts` (§6.1).
const FIRST_FRAME: Duration = Duration::from_millis(100);
/// C-R17: the most messages a wake applies before the loop draws, so a burst is one frame and
/// the frames still come under a stream that never ends.
const BATCH: usize = 256;
/// The index lane passes on a refresh's progress at most this often: the loop's own pace.
const PROGRESS_EVERY: Duration = WAKE;
/// Why a hand-over of the terminal was refused (§7.3 step 1): the input thread could still read
/// keys typed for the terminal's next owner.
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
    pub signal: Cancel,
    /// The shutdown token. The binary has already built every lane's engine, and the host's
    /// in-TUI context, with it.
    pub shutdown: Cancel,
}

/// How the TUI ended.
pub struct TuiExit {
```

with:

```rust
    pub signal: Cancel,
    /// The shutdown token. The binary has already built every lane's engine, and the host's
    /// in-TUI context, with it.
    pub shutdown: Cancel,
    /// The socket of the daemon of the store the TUI reads (`daemon_socket_path`, D5.1), which
    /// it follows (spec §8); `None` for timer mode alone.
    pub daemon: Option<PathBuf>,
}

/// How the TUI ended.
pub struct TuiExit {
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
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
```

with:

```rust
    restore: fn(),
) -> TuiExit {
    let _restored = Restore(restore);
    let (tx, rx) = mpsc::channel();
    // The lanes build their engines while the terminal is set up (§6.1), and the live thread
    // connects meanwhile. Neither is waited for: the TUI starts in timer mode (§8).
    let status = opts
        .daemon
        .clone()
        .map(|path| -> StatusFetch { Arc::new(move || live::fetch_status(&path)) });
    let lanes = Lanes::spawn(
        opts.provider.clone(),
        &sources,
        &host,
        &tx,
        &opts.shutdown,
        status,
    );
    if let Some(path) = opts.daemon.clone() {
        let died = lanes.stray_death("live", &tx);
        // A thread that cannot start leaves the TUI in timer mode.
        let _ = live::spawn(path, Backoff::default(), Arc::clone(&lanes.live), died);
    }
    let driven = panic::catch_unwind(AssertUnwindSafe(|| {
        let tty = match enter() {
            Ok(tty) => tty,
            Err(e) => {
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
        .duration_since(UNIX_EPOCH)
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
    /// Spawns them. Each builds its own `Source` in its own thread (R4), and a lane that panics
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
```

with:

```rust
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// What fetches the daemon's `status` on the daemon lane: `live::fetch_status` on its socket.
pub(crate) type StatusFetch = Arc<dyn Fn() -> Result<Status, String> + Send + Sync>;

/// The lanes (§4.3). The engine lanes: views (`accounts`, `history`, store-served), report
/// (the token report, which reads the index only), index (the token index's catch-up) and
/// actions (the collector and every `Host` action, one at a time). And the daemon lane, which
/// fetches the daemon's `status` and has no engine.
pub(crate) struct Lanes {
    views: Lane<ViewsReq>,
    report: Lane<ReportReq>,
    index: Lane<IndexReq>,
    actions: Lane<ActionsReq>,
    /// When the TUI follows a daemon.
    daemon: Option<Lane<DaemonReq>>,
    /// The threads that are let go rather than joined, the daemon lane and the live thread,
    /// that died: named in the report as a joined lane that panicked is (§10.4).
    strays: Arc<Mutex<Vec<&'static str>>>,
    /// What the live thread passes on, bounded; a `Msg::LiveReady` says it has something
    /// (C-R17).
    pub(crate) live: Arc<LiveQueue>,
}

impl Lanes {
    /// Spawns them. Each engine lane builds its own `Source` in its own thread (R4), and a lane
    /// that panics says so on `tx` (§4.3, §10.4). The daemon lane is spawned when `status` is
    /// given.
    pub(crate) fn spawn(
        provider: Option<ProviderId>,
        sources: &SourceFactory,
        host: &Arc<dyn Host>,
        tx: &Sender<Msg>,
        shutdown: &Cancel,
        status: Option<StatusFetch>,
    ) -> Lanes {
        let strays = Arc::default();
        let views = {
            let (make, out, died) = (Arc::clone(sources), tx.clone(), tx.clone());
            Lane::spawn(
                "views",
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
                    let _ = died.send(Msg::LaneDied { lane: "actions" });
                },
            )
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
    fn join(self) -> Vec<&'static str> {
        let mut died = Vec::new();
        if self.views.join().is_err() {
            died.push("views");
```

with:

```rust
                    let _ = died.send(Msg::LaneDied { lane: "actions" });
                },
            )
        };
        let daemon = status.map(|fetch| {
            let out = tx.clone();
            let died = stray_death(&strays, "daemon", tx);
            Lane::spawn(
                "daemon",
                || (),
                move |_: &mut (), req: DaemonReq| {
                    let DaemonReq::Status { link } = req;
                    let result = fetch();
                    let _ = out.send(Msg::DaemonStatus { link, result });
                },
                died,
            )
        });
        let wake = tx.clone();
        let live = Arc::new(LiveQueue::new(LIVE_QUEUE, move || {
            wake.send(Msg::LiveReady).is_ok()
        }));
        Lanes {
            views,
            report,
            index,
            actions,
            daemon,
            strays,
            live,
        }
    }

    /// What a thread that is let go rather than joined runs if it panics: it is named in the
    /// report, and the loop is told, which ends the TUI (§10.4).
    pub(crate) fn stray_death(
        &self,
        name: &'static str,
        tx: &Sender<Msg>,
    ) -> impl FnOnce() + Send + 'static {
        stray_death(&self.strays, name, tx)
    }

    /// §10.2 step 2: nothing more is taken, what is queued is dropped, and an idle lane wakes
    /// and returns. A refresh the index lane runs stops at its next chunk: its engine takes
    /// the shutdown token, which step 1 set (§9).
    fn close(&self) {
        self.views.close();
        self.report.close();
        self.index.close();
        self.actions.close();
        if let Some(daemon) = &self.daemon {
            daemon.close();
        }
    }

    /// §10.2 step 5: joins every engine lane, lets the daemon lane go (§4.3: it touches no
    /// store, and a `status` it waits for may take 2 s), and names the lanes that panicked.
    fn join(self) -> Vec<&'static str> {
        let mut died = Vec::new();
        if self.views.join().is_err() {
            died.push("views");
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
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
```

with:

```rust
        }
        if self.actions.join().is_err() {
            died.push("actions");
        }
        drop(self.daemon);
        died.extend(lock(&self.strays).iter());
        died
    }
}

/// `stray_death` over the list it adds to.
fn stray_death(
    strays: &Arc<Mutex<Vec<&'static str>>>,
    name: &'static str,
    tx: &Sender<Msg>,
) -> impl FnOnce() + Send + 'static {
    let (strays, tx) = (Arc::clone(strays), tx.clone());
    move || {
        lock(&strays).push(name);
        let _ = tx.send(Msg::LaneDied { lane: name });
    }
}

/// The list of dead strays, whatever a panic left it as: it only holds names.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How the loop ended (§10.2's table): the exit code, a fatal error's message, and what an
/// action reported in a message the shutdown overtook, for the report.
pub(crate) struct Ending {
    code: i32,
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
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

with:

```rust
        lanes: &'a Lanes,
        host: &'a dyn Host,
    ) -> Self {
        let tokens = TokensState::new(opts.tokens_provider.clone(), opts.tz.clone());
        let now = now_ms();
        let mut state = State::new(opts.screen, opts.color, opts.provider.clone(), tokens, now);
        if opts.daemon.is_some() {
            // C-R20: the first collection waits to know who collects.
            state.daemon.await_daemon(now);
        }
        Ui {
            state,
            tty,
            input,
            rx,
            lanes,
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
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
```

with:

```rust
        }
    }

    /// §4.3, until a shutdown begins. Each wake looks at the signal token before anything else
    /// (§10.2): before the wait, and again before each message or the tick that ended it. It
    /// redraws when the state changed, applies every message waiting (`BATCH` at most, C-R17),
    /// and ticks every 250 ms whatever arrives.
    pub(crate) fn run(&mut self) -> Ending {
        loop {
            self.shutting_down();
            if let Some(ending) = self.ending.take() {
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
                self.draw();
            }
            let wait = self.next_tick.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(wait) {
                Ok(msg) => self.deliver(msg),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => self.begin_shutdown(
                    1,
                    libc::SIGINT,
```

with:

```rust
                self.draw();
            }
            let wait = self.next_tick.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(wait) {
                Ok(msg) => {
                    self.deliver(msg);
                    self.drain();
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => self.begin_shutdown(
                    1,
                    libc::SIGINT,
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
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
```

with:

```rust
        }
        self.ending.is_some()
    }

    /// C-R17: the messages already waiting, after the one that woke the loop, up to `BATCH` in
    /// all, so that a burst is drawn once and a key behind it waits for no frame of its own.
    fn drain(&mut self) {
        for _ in 1..BATCH {
            if self.ending.is_some() {
                return;
            }
            match self.rx.try_recv() {
                Ok(msg) => self.deliver(msg),
                Err(_) => return,
            }
        }
    }

    /// Applies `msg`, after a look at the signal token: a signal that arrived with it comes
    /// first, and so does a shutdown already begun. Then `msg` is not applied; an action's
    /// outcome in it still goes into the report (§10.2 step 6). `LiveReady` applies everything
    /// in the live queue, each thing as a message of its own (C-R17).
    fn deliver(&mut self, msg: Msg) {
        if let Msg::LiveReady = msg {
            for live in self.lanes.live.take() {
                self.deliver(Msg::Live(live));
            }
            return;
        }
        if !self.shutting_down() {
            self.apply(msg);
        } else if let Some(ending) = &mut self.ending {
            ending.report.push_str(&reported(msg));
```

In `crates/tagteam-tui/src/runtime.rs`, replace:

```rust
                }
                Cmd::Suspend(action) => self.suspend(action),
                Cmd::Quit { code } => self.begin_shutdown(code, libc::SIGINT, None),
                Cmd::Stop => self.stop(),
            }
        }
    }

```

with:

```rust
                }
                Cmd::Suspend(action) => self.suspend(action),
                Cmd::Quit { code } => self.begin_shutdown(code, libc::SIGINT, None),
                Cmd::Stop => self.stop(),
                Cmd::FetchStatus { link } => {
                    if let Some(daemon) = &self.lanes.daemon {
                        let _ = daemon.submit(DaemonReq::Status { link });
                    }
                }
                Cmd::DropLink { link } => self.lanes.live.end_link(link),
            }
        }
    }

```

In `crates/tagteam/src/tui_cmd.rs`, replace:

```rust
use std::sync::Arc;

use tagteam_core::ProviderId;
use tagteam_engine::EngineError;
use tagteam_provider::Cancel;
use tagteam_provider::profile::RunShell;
use tagteam_tui::source::{EngineSource, Source, SourceFactory};
use tagteam_tui::{Screen, TuiOptions};
```

with:

```rust
use std::sync::Arc;

use tagteam_core::ProviderId;
use tagteam_engine::EngineError;
use tagteam_engine::daemon::daemon_socket_path;
use tagteam_provider::Cancel;
use tagteam_provider::profile::RunShell;
use tagteam_tui::source::{EngineSource, Source, SourceFactory};
use tagteam_tui::{Screen, TuiOptions};
```

In `crates/tagteam/src/tui_cmd.rs`, replace:

```rust
    // zone, which is read here so that the UI thread reads no zone file (B6).
    let tokens_provider = provider
        .clone()
        .unwrap_or_else(|| probe.default_provider().clone());
    drop(probe);
    let tz = jiff::tz::TimeZone::try_system().unwrap_or(jiff::tz::TimeZone::UTC);
    // §7.3: Ctrl-\ belongs to a foreground session, and the TUI survives it for its whole life.
    if let Err(e) = signals::survive_quit() {
```

with:

```rust
    // zone, which is read here so that the UI thread reads no zone file (B6).
    let tokens_provider = provider
        .clone()
        .unwrap_or_else(|| probe.default_provider().clone());
    // Spec §8: the daemon of the store the TUI reads, found from the environment alone (D5.1).
    let daemon = daemon_socket_path(probe.env());
    drop(probe);
    let tz = jiff::tz::TimeZone::try_system().unwrap_or(jiff::tz::TimeZone::UTC);
    // §7.3: Ctrl-\ belongs to a foreground session, and the TUI survives it for its whole life.
    if let Err(e) = signals::survive_quit() {
```

In `crates/tagteam/src/tui_cmd.rs`, replace:

```rust
        tokens_provider,
        tz,
        signal: ctx.env.cancel.clone(),
        shutdown,
    };
    let exit = tagteam_tui::run(opts, sources(inside, provider), host);
    if let Some(report) = &exit.report {
        let _ = write!(err, "{report}");
```

with:

```rust
        tokens_provider,
        tz,
        signal: ctx.env.cancel.clone(),
        shutdown,
        daemon: Some(daemon),
    };
    let exit = tagteam_tui::run(opts, sources(inside, provider), host);
    if let Some(report) = &exit.report {
        let _ = write!(err, "{report}");
```

- [ ] **Step 4: Run the tests and see them pass**

Run:
- Outside the sandbox: `cargo test -p tagteam-tui --lib` — Expected: `271 passed` (23 new: 16 in `daemon`, 6 in `runtime`, 1 in `msg`). Inside it, `runtime::tests::a_failure_judged_after_the_live_thread_replaced_its_link_leaves_the_new_link_alone` fails with `PermissionDenied`, and the rest pass.
- `cargo test -p tagteam --features test-support --lib tui_cmd` — Expected: `1 passed`.
- Outside the sandbox: `cargo test -p tagteam --features test-support --test tui_pty` — Expected: `19 passed`: with no daemon, the live thread finds none, and the TUI is in timer mode as before.

- [ ] **Step 5: Format and lint**

Run:
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`

Expected: no output from the first; `Finished` and no warnings from the others.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-tui/src/daemon/mod.rs crates/tagteam-tui/src/lib.rs crates/tagteam-tui/src/msg.rs crates/tagteam-tui/src/state.rs crates/tagteam-tui/src/update.rs crates/tagteam-tui/src/testutil.rs crates/tagteam-tui/src/runtime.rs crates/tagteam/src/tui_cmd.rs
git commit -m "Go live on the daemon's status and read the views again on its events, collecting only for what it leaves"
```

### Task 3: The header's daemon state and the activity line

What the daemon says, on screen (spec §5.1, §7.4, §8), from `daemon/words.rs`:

- **The header's mode** (C-R9): `● live · ` and §7.4's words for the provider the header follows (C-R10), from `status` alone; in timer mode `○ refresh 30s`, or why the daemon is not followed. It goes on the header's row beside the tabs when it fits, without `? help` when only that makes it fit, and on a second row of its own otherwise.
- **The activity line** (C-R11): the daemon's latest event, at its time, in auto's words, one row above the footer in live mode. A `switch` names the window that bound the account at its tick's `poll`, so it reads as the spec's mock: `12:04 auto: 1 → 2 (5h at 95%)`. A new link starts with no `poll` (C-R21).

`TokensState::tz` gives the header's and the activity line's times the system zone (B6).

**Readings of the spec this task commits to:**
- **"The header shows each provider's state from the latest `status` reply"** (§7.4): the words come from `status` alone. A `mode` event's fields never reach them (§8); the test sends one that says `idle` while `status` says `collect`.
- **"Every event except `sleep`, `changed` and `mode` also updates the activity line, using its `ts`"** (§8). `usage` and `stopping` reach the TUI typed, without their `ts`, so they take the time they arrived, the same second in practice. A kind this TUI does not know is ignored (§8), so it leaves the line as it was.
- **The binding window** of a switch is the one D6.1 finds: the window at the usage `100 − headroomPct` names, among the account's `windowsPct` in the tick's `poll`. Without that `poll`, the line names the trigger. A `poll` from an earlier link is not that `poll`: a link can come up after events were missed (C-R21).
- **At 80 columns** the tabs take 43 of them. `○ refresh 30s` and `● live · auto on` fit beside them with `? help`; `● live · auto off (one account)` and `○ daemon stopping (upgrade)` fit without it; a keychain backoff takes the second row.
- **Part A's and part B's tests are unchanged:** timer mode draws exactly what it drew.

**Depends on:** 2

**Files:**
- Create: `crates/tagteam-tui/src/daemon/words.rs`
- Modify: `crates/tagteam-tui/src/daemon/mod.rs`
- Modify: `crates/tagteam-tui/src/screens/tokens/mod.rs` (`TokensState::tz`)
- Modify: `crates/tagteam-tui/src/view.rs`
- Modify: `crates/tagteam-tui/Cargo.toml` (`serde_json` from the dev-dependencies to the dependencies)

**Interfaces:**
- Consumes: Task 2's `DaemonState`; `tagteam_daemon::protocol::{Event, StatusProvider, StatusBackoff}`; part A's `view::{fit, width_of, MODE_TIMER}`; `tagteam_engine::words::MISSING`.
- Produces: CC3's `words`, `Activity`, `DaemonState::activity`, `focus`; CC4's `Areas::activity` and `TokensState::tz`. Task 5's `P` acts on `focus`'s provider.

**Spec:**
- TUI spec §5.1 (the header's mode: `● live` with the daemon's auto state, or `○ refresh 30s`; the activity line above the footer in live mode, hidden in timer mode), §7.4 (the header's table, with a backoff; several providers), §8 (`status` is the authority; the activity line's events and `ts`; `stopping`'s and a mismatch's words), §11 ("Daemon client": the `status` reply filling the header, for every row of §7.4's table, with and without a keychain backoff; an event's fields never applied to the header; "Rendering").
- Daemon spec D5.4 (`backoff.until`), D5.5 (the events' fields), D6.1 (the binding window).

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-tui/src/daemon/words.rs` with its tests (Step 3 puts the code above them):

```rust
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;
    use tagteam_daemon::protocol::StatusBackoff;

    use super::*;
    use crate::testutil::*;

    fn utc() -> TimeZone {
        TimeZone::UTC
    }

    #[test]
    fn each_daemon_state_of_spec_7_4_has_its_words_with_and_without_a_keychain_backoff() {
        for (mode, autoswitch, waiting_for, holder, words) in [
            ("auto", true, None, None, "auto on"),
            ("collect", false, None, None, "auto paused"),
            ("collect", true, None, None, "auto off (one account)"),
            (
                "waiting",
                true,
                Some("engine-lock"),
                Some(4711),
                "auto in pid 4711",
            ),
            (
                "waiting",
                true,
                Some("engine-lock"),
                None,
                "auto in another process",
            ),
            (
                "waiting",
                true,
                Some("schema"),
                None,
                "daemon needs restart",
            ),
            ("idle", true, None, None, "daemon idle"),
            ("added-later", true, None, None, "daemon added-later"),
        ] {
            let mut job = daemon_job("claude-code", mode, autoswitch);
            job.waiting_for = waiting_for.map(str::to_owned);
            job.holder_pid = holder;
            assert_eq!(job_words(&job, &utc()), words, "{mode} {waiting_for:?}");
            job.backoff = Some(StatusBackoff {
                reason: "keychain".into(),
                until: "2026-09-21T15:05:00Z".into(),
            });
            assert_eq!(
                job_words(&job, &utc()),
                format!("{words} · keychain locked until 15:05"),
                "{mode} {waiting_for:?}"
            );
        }
    }

    fn auto_event(kind: &str, fields: Value) -> Event {
        let mut raw = json!({
            "schemaVersion": 1,
            "event": kind,
            "ts": "2026-09-21T14:13:20Z",
            "provider": "claude-code",
        });
        for (k, v) in fields.as_object().unwrap() {
            raw[k] = v.clone();
        }
        Event::Auto {
            kind: kind.into(),
            provider: "claude-code".into(),
            raw,
        }
    }

    #[test]
    fn the_activity_line_says_each_event_in_auto_s_words_and_leaves_out_four_kinds() {
        // Spec §8: every event but `sleep`, `changed` and `mode`; a kind it does not know is
        // ignored.
        let poll = json!({"active": {"number": 1, "email": "a@x.co"}, "headroomPct": {"1": 23.0, "2": 80.0}, "threshold": 90.0});
        let cases = [
            (auto_event("poll", poll), Some("auto: polled · 1 at 77%")),
            (
                auto_event("poll", json!({"active": null, "headroomPct": {}})),
                Some("auto: polled"),
            ),
            (
                auto_event(
                    "switch",
                    json!({"trigger": "proactive", "from": 1, "to": 2, "warnings": [], "dryRun": false}),
                ),
                Some("auto: 1 → 2 (proactive)"),
            ),
            (
                auto_event(
                    "switch",
                    json!({"trigger": "reactive", "from": 2, "to": 1, "warnings": [], "dryRun": true}),
                ),
                Some("auto would switch: 2 → 1 (reactive)"),
            ),
            (
                auto_event(
                    "no-switch",
                    json!({"reason": "below-threshold", "detail": ""}),
                ),
                Some("auto: no switch: below-threshold"),
            ),
            (
                auto_event(
                    "no-switch",
                    json!({"reason": "cooldown", "detail": "4m left"}),
                ),
                Some("auto: no switch: cooldown (4m left)"),
            ),
            (
                auto_event(
                    "all-exhausted",
                    json!({"earliestResetAt": "2026-09-21T16:40:00Z"}),
                ),
                Some("auto: no switch: all-exhausted, the first back at 16:40"),
            ),
            (
                auto_event("all-exhausted", json!({"earliestResetAt": null})),
                Some("auto: no switch: all-exhausted"),
            ),
            (
                auto_event(
                    "account-quarantined",
                    json!({"number": 2, "email": "b@x.co", "reason": "invalid_grant"}),
                ),
                Some("2 b@x.co quarantined (invalid_grant)"),
            ),
            (
                auto_event(
                    "account-unquarantined",
                    json!({"number": 2, "email": "b@x.co", "reason": "relogin"}),
                ),
                Some("2 b@x.co unquarantined (relogin)"),
            ),
            (
                auto_event(
                    "error",
                    json!({"message": "the store is locked", "transient": true}),
                ),
                Some("error: the store is locked"),
            ),
            (
                auto_event("config-warning", json!({"message": "unknown key x"})),
                Some("warning: unknown key x"),
            ),
            (
                Event::Usage {
                    provider: "claude-code".into(),
                    updated: vec![1, 2],
                    fetch_errors: BTreeMap::from([(3, "http-429".to_owned())]),
                },
                Some("collected 1, 2 · not collected: 3 (http-429)"),
            ),
            (
                Event::Usage {
                    provider: "claude-code".into(),
                    updated: vec![],
                    fetch_errors: BTreeMap::from([(3, "http-429".to_owned())]),
                },
                Some("not collected: 3 (http-429)"),
            ),
            (
                Event::Stopping {
                    reason: "upgrade".into(),
                },
                Some("daemon stopping (upgrade)"),
            ),
            (
                Event::TokenIndex {
                    provider: "claude-code".into(),
                    raw: json!({}),
                },
                Some("token index updated"),
            ),
            (
                auto_event("sleep", json!({"seconds": 300.0, "until": "x"})),
                None,
            ),
            (Event::Changed, None),
            (
                Event::Mode {
                    provider: "claude-code".into(),
                    mode: "auto".into(),
                    waiting_for: None,
                    holder_pid: None,
                    backoff: None,
                },
                None,
            ),
            (
                Event::Unknown {
                    kind: "added-later".into(),
                    raw: json!({}),
                },
                None,
            ),
        ];
        for (event, words) in cases {
            assert_eq!(
                activity(&event, None, &utc()).as_deref(),
                words,
                "{event:?}"
            );
        }
    }

    #[test]
    fn a_switch_names_the_window_that_bound_it_at_its_tick_s_poll() {
        // Spec §5.1's `auto: 1 → 2 (5h at 95%)`: the binding window is the one at the usage
        // `headroomPct` leaves, as D6.1 finds it.
        let poll = json!({
            "headroomPct": {"1": 5.0},
            "windowsPct": {"1": {"7d": 40.0, "5h": 95.0, "7d_opus": 98.0}},
        });
        let switch = auto_event(
            "switch",
            json!({"trigger": "proactive", "from": 1, "to": 2, "warnings": [], "dryRun": false}),
        );
        assert_eq!(
            activity(&switch, Some(&poll), &utc()).as_deref(),
            Some("auto: 1 → 2 (5h at 95%)"),
            "not the scoped window the relevant ones leave out"
        );
        let other = json!({"headroomPct": {"2": 5.0}, "windowsPct": {"2": {"5h": 95.0}}});
        assert_eq!(
            activity(&switch, Some(&other), &utc()).as_deref(),
            Some("auto: 1 → 2 (proactive)"),
            "a poll that did not read the account switched from"
        );
    }

    #[test]
    fn an_unreadable_time_is_a_dash() {
        assert_eq!(iso_clock("soon", &utc()), MISSING);
        assert_eq!(clock(NOW_MS, &utc()), "14:13");
    }
}
```

Declare the module, so that Step 2 builds its tests:

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust

use tagteam_core::{AccountId, ProviderId};
```

with:

```rust

pub mod words;

use tagteam_core::{AccountId, ProviderId};
```

In `crates/tagteam-tui/src/view.rs`, replace:

```rust
        assert_eq!(a.footer.height, 1);
        assert_eq!(a.body.height, 24 - 1 - 1);
        assert_eq!(draw(&state, 80, 24)[23].trim_end(), "");
    }
}
```

with:

```rust
        assert_eq!(a.footer.height, 1);
        assert_eq!(a.body.height, 24 - 1 - 1);
        assert_eq!(draw(&state, 80, 24)[23].trim_end(), "");
    }

    /// Claude Code's job in `mode`, with `autoswitch`.
    fn one_job(mode: &str, autoswitch: bool) -> tagteam_daemon::protocol::Status {
        daemon_status(vec![daemon_job("claude-code", mode, autoswitch)], vec![])
    }

    fn live_event(e: tagteam_daemon::protocol::Event) -> Msg {
        Msg::Live(crate::live::Live::Event(e))
    }

    #[test]
    fn in_live_mode_the_header_says_the_daemon_s_state_from_its_status_alone() {
        // Spec §5.1, §7.4, §8: `● live` and the provider's state; an event's fields never
        // reach the header.
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(1)));
        let link = go_live(&mut state, one_job("collect", false));
        let paused = draw(&state, 80, 24);
        assert!(
            paused[0].ends_with("3 History       ● live · auto paused   ? help"),
            "{}",
            paused[0]
        );
        let mode = tagteam_daemon::protocol::Event::Mode {
            provider: "claude-code".into(),
            mode: "idle".into(),
            waiting_for: None,
            holder_pid: None,
            backoff: None,
        };
        update(&mut state, live_event(mode), NOW_MS);
        assert_eq!(
            draw(&state, 80, 24)[0],
            paused[0],
            "until the status says so"
        );
        let reply = Msg::DaemonStatus {
            link,
            result: Ok(one_job("idle", true)),
        };
        update(&mut state, reply, NOW_MS);
        assert!(draw(&state, 80, 24)[0].ends_with("● live · daemon idle   ? help"));
    }

    #[test]
    fn with_several_providers_the_header_follows_the_selected_account_s() {
        // Spec §7.4.
        let mut state = fresh(Screen::Accounts);
        let fake = tagteam_core::ProviderId::new("fake-agent");
        let mut third = account(3, "c@x.co", read(60, 10.0, 20.0, false));
        third.row.provider = fake.clone();
        let mut snap = snapshot(many(1));
        snap.providers
            .push(tagteam_engine::views::ProviderAccounts {
                provider: fake,
                active_position: None,
                accounts: vec![third],
            });
        state.accounts.apply(snap);
        let mut waiting = daemon_job("fake-agent", "waiting", true);
        waiting.waiting_for = Some("engine-lock".into());
        waiting.holder_pid = Some(77);
        let status = daemon_status(
            vec![daemon_job("claude-code", "auto", true), waiting],
            vec![],
        );
        go_live(&mut state, status);
        assert!(draw(&state, 80, 24)[0].ends_with("● live · auto on   ? help"));
        update(&mut state, ch('j'), NOW_MS);
        assert!(
            draw(&state, 80, 24)[0].ends_with("● live · auto in pid 77   ? help"),
            "{}",
            draw(&state, 80, 24)[0]
        );
    }

    #[test]
    fn a_mode_too_wide_for_the_row_drops_help_and_then_takes_a_row_of_its_own() {
        // R20: at 80 columns nothing the header says is lost.
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(1)));
        go_live(&mut state, one_job("collect", true));
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[0],
            " tagteam  [1 Accounts]  2 Tokens  3 History     ● live · auto off (one account)"
        );
        assert!(draw(&state, 120, 40)[0].ends_with("● live · auto off (one account)   ? help"));
        let mut locked = daemon_job("claude-code", "auto", true);
        locked.backoff = Some(tagteam_daemon::protocol::StatusBackoff {
            reason: "keychain".into(),
            until: "2026-09-21T15:05:00Z".into(),
        });
        go_live(&mut state, daemon_status(vec![locked], vec![]));
        let lines = draw(&state, 80, 24);
        assert!(lines[0].ends_with("? help"), "{}", lines[0]);
        assert_eq!(
            lines[1],
            format!(
                "{}● live · auto on · keychain locked until 15:05",
                " ".repeat(33)
            )
        );
        assert_eq!(areas(&state, Rect::new(0, 0, 80, 24)).body.y, 2);
        assert!(
            lines[2].contains("ACCOUNT"),
            "the table under it: {lines:?}"
        );
        assert!(draw(&state, 120, 40)[0].ends_with("keychain locked until 15:05   ? help"));
    }

    #[test]
    fn timer_mode_says_when_the_daemon_stopped_or_speaks_another_protocol() {
        // Spec §8.
        let mut state = fresh(Screen::Accounts);
        go_live(&mut state, one_job("auto", true));
        let stopping = tagteam_daemon::protocol::Event::Stopping {
            reason: "upgrade".into(),
        };
        update(&mut state, live_event(stopping), NOW_MS);
        update(&mut state, Msg::Live(crate::live::Live::Down), NOW_MS);
        assert_eq!(
            draw(&state, 80, 24)[0],
            format!(
                " tagteam  [1 Accounts]  2 Tokens  3 History{}○ daemon stopping (upgrade)",
                " ".repeat(9)
            ),
            "no room for `? help` at 80 columns"
        );
        assert!(draw(&state, 120, 40)[0].ends_with("○ daemon stopping (upgrade)   ? help"));
        update(&mut state, Msg::Tick, NOW_MS + 30_000);
        assert!(draw(&state, 80, 24)[0].ends_with("○ refresh 30s   ? help"));
        let mismatch = crate::live::Live::Mismatch { server: 2 };
        update(&mut state, Msg::Live(mismatch), NOW_MS);
        assert!(
            draw(&state, 80, 24)[0].ends_with("○ daemon protocol 2 unsupported"),
            "{}",
            draw(&state, 80, 24)[0]
        );
    }

    #[test]
    fn the_activity_line_shows_the_latest_event_above_the_footer_in_live_mode_only() {
        // Spec §5.1: hidden in timer mode; in live mode it takes a row from the body.
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(1)));
        let timer = areas(&state, Rect::new(0, 0, 80, 24));
        assert!(timer.activity.is_none());
        go_live(&mut state, one_job("auto", true));
        let live = areas(&state, Rect::new(0, 0, 80, 24));
        assert_eq!(live.activity, Some(Rect::new(0, 22, 80, 1)));
        assert_eq!(live.body.height, timer.body.height - 1);
        assert_eq!(draw(&state, 80, 24)[22], "", "blank until an event");
        let switch = tagteam_daemon::protocol::Event::Auto {
            kind: "switch".into(),
            provider: "claude-code".into(),
            raw: serde_json::json!({"event": "switch", "ts": "2026-09-21T14:04:00Z", "trigger": "proactive", "from": 1, "to": 2, "dryRun": false}),
        };
        update(&mut state, live_event(switch), NOW_MS);
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[22], " 14:04 auto: 1 → 2 (proactive)",
            "at the event's own ts"
        );
        assert!(lines[23].starts_with("s switch"), "the footer below it");
        // A kind it leaves out keeps the last; `usage` has no `ts` of its own: now.
        update(
            &mut state,
            live_event(tagteam_daemon::protocol::Event::Changed),
            NOW_MS,
        );
        assert_eq!(draw(&state, 80, 24)[22], " 14:04 auto: 1 → 2 (proactive)");
        let usage = tagteam_daemon::protocol::Event::Usage {
            provider: "claude-code".into(),
            updated: vec![1],
            fetch_errors: Default::default(),
        };
        update(&mut state, live_event(usage), NOW_MS);
        assert_eq!(draw(&state, 80, 24)[22], " 14:13 collected 1");
        update(&mut state, Msg::Live(crate::live::Live::Down), NOW_MS);
        assert!(areas(&state, Rect::new(0, 0, 80, 24)).activity.is_none());
        assert_eq!(draw(&state, 80, 24)[22], "", "hidden again");
    }

    #[test]
    fn a_switch_names_its_window_only_from_a_poll_of_its_own_link() {
        // C-R21: a link can come up after events were missed (a link dropped, a queue that
        // overflowed), so a `switch` names its window only from a `poll` the same link carried.
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(2)));
        go_live(&mut state, one_job("auto", true));
        let auto = |kind: &str, raw| tagteam_daemon::protocol::Event::Auto {
            kind: kind.into(),
            provider: "claude-code".into(),
            raw,
        };
        let poll = auto(
            "poll",
            serde_json::json!({"event": "poll", "ts": "2026-09-21T14:03:00Z", "active": {"number": 1}, "headroomPct": {"1": 5.0}, "windowsPct": {"1": {"5h": 95.0}}}),
        );
        let switch = auto(
            "switch",
            serde_json::json!({"event": "switch", "ts": "2026-09-21T14:04:00Z", "trigger": "proactive", "from": 1, "to": 2, "dryRun": false}),
        );
        update(&mut state, live_event(poll), NOW_MS);
        update(&mut state, live_event(switch.clone()), NOW_MS);
        assert_eq!(draw(&state, 80, 24)[22], " 14:04 auto: 1 → 2 (5h at 95%)");
        update(&mut state, Msg::Live(crate::live::Live::Down), NOW_MS);
        go_live(&mut state, one_job("auto", true));
        update(&mut state, live_event(switch), NOW_MS);
        assert_eq!(
            draw(&state, 80, 24)[22],
            " 14:04 auto: 1 → 2 (proactive)",
            "the poll was the last link's"
        );
    }

    #[test]
    fn with_several_providers_the_activity_line_names_the_event_s_provider() {
        let mut state = fresh(Screen::Accounts);
        let fake = tagteam_core::ProviderId::new("fake-agent");
        let mut snap = snapshot(many(1));
        snap.providers
            .push(tagteam_engine::views::ProviderAccounts {
                provider: fake,
                active_position: None,
                accounts: vec![],
            });
        state.accounts.apply(snap);
        go_live(&mut state, one_job("auto", true));
        let usage = tagteam_daemon::protocol::Event::Usage {
            provider: "fake-agent".into(),
            updated: vec![1],
            fetch_errors: Default::default(),
        };
        update(&mut state, live_event(usage), NOW_MS);
        assert_eq!(draw(&state, 80, 24)[22], " 14:13 fake-agent  collected 1");
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p tagteam-tui --lib`

Expected: FAIL: the build stops with errors such as ``error[E0412]: cannot find type `TimeZone` in this scope `` and ``error[E0412]: cannot find type `Value` in this scope ``, from `daemon::words`'s tests.

- [ ] **Step 3: Write the code**

The words, then the activity line's record in `daemon/mod.rs`, the zone, the view, and `serde_json`.

In `crates/tagteam-tui/src/daemon/words.rs`, put this above the tests, with one blank line between:

```rust
//! The daemon in words (spec §5.1, §7.4, §8): the header's mode, from `status` alone, and the
//! activity line's account of the latest event.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::Value;
use tagteam_daemon::protocol::{Event, StatusProvider};
use tagteam_engine::words::MISSING;

use crate::state::State;
use crate::view::MODE_TIMER;

/// Spec §5.1, §7.4, §8: the header's mode. In live mode, `● live` and the daemon's state for
/// the provider the header follows (`focus`), from the last `status`; else timer mode's, or why
/// the daemon is not followed: a protocol this TUI does not speak, or the daemon's `stopping`.
pub fn mode(state: &State) -> String {
    let d = &state.daemon;
    if d.live() {
        let words = d.job(&super::focus(state)).map_or_else(
            || "daemon idle".to_owned(),
            |job| job_words(job, state.tokens.tz()),
        );
        return format!("● live · {words}");
    }
    if let Some(server) = d.mismatch() {
        return format!("○ daemon protocol {server} unsupported");
    }
    if let Some(reason) = d.stopping() {
        return format!("○ daemon stopping ({reason})");
    }
    MODE_TIMER.to_owned()
}

/// §7.4's table: a provider job's state, and its keychain backoff's end in `tz`.
pub fn job_words(job: &StatusProvider, tz: &TimeZone) -> String {
    let state = match (job.mode.as_str(), job.waiting_for.as_deref()) {
        ("auto", _) => "auto on".to_owned(),
        ("collect", _) if !job.autoswitch => "auto paused".to_owned(),
        ("collect", _) => "auto off (one account)".to_owned(),
        ("waiting", Some("engine-lock")) => match job.holder_pid {
            Some(pid) => format!("auto in pid {pid}"),
            None => "auto in another process".to_owned(),
        },
        ("waiting", Some("schema")) => "daemon needs restart".to_owned(),
        ("idle", _) => "daemon idle".to_owned(),
        (mode, _) => format!("daemon {mode}"),
    };
    match &job.backoff {
        Some(backoff) => format!(
            "{state} · keychain locked until {}",
            iso_clock(&backoff.until, tz)
        ),
        None => state,
    }
}

/// An ISO 8601 time the daemon wrote, as `HH:MM` in `tz`; `—` when it cannot be read.
fn iso_clock(iso: &str, tz: &TimeZone) -> String {
    iso_ms(iso).map_or_else(|| MISSING.to_owned(), |ms| clock(ms, tz))
}

/// An ISO 8601 time, in epoch ms.
pub(crate) fn iso_ms(iso: &str) -> Option<i64> {
    iso.parse::<Timestamp>().ok().map(|t| t.as_millisecond())
}

/// `ms` as `HH:MM` in `tz`.
pub(crate) fn clock(ms: i64, tz: &TimeZone) -> String {
    Timestamp::from_millisecond(ms).map_or_else(
        |_| MISSING.to_owned(),
        |t| t.to_zoned(tz.clone()).strftime("%H:%M").to_string(),
    )
}

/// The activity line's words for `event` (spec §5.1, §8), or none for the kinds it leaves out:
/// `sleep`, `changed`, `mode`, and a kind this TUI does not know. `poll` is the provider's last
/// `poll`, from whose tick a switch names its window. Times are in `tz`.
pub(crate) fn activity(event: &Event, poll: Option<&Value>, tz: &TimeZone) -> Option<String> {
    let text = match event {
        Event::Auto { kind, raw, .. } => auto(kind, raw, poll, tz)?,
        Event::Usage {
            updated,
            fetch_errors,
            ..
        } => {
            let mut parts = Vec::new();
            if !updated.is_empty() {
                let positions: Vec<String> = updated.iter().map(u32::to_string).collect();
                parts.push(format!("collected {}", positions.join(", ")));
            }
            if !fetch_errors.is_empty() {
                let failed: Vec<String> = fetch_errors
                    .iter()
                    .map(|(position, kind)| format!("{position} ({kind})"))
                    .collect();
                parts.push(format!("not collected: {}", failed.join(", ")));
            }
            if parts.is_empty() {
                parts.push("collected".to_owned());
            }
            parts.join(" · ")
        }
        Event::Stopping { reason } => format!("daemon stopping ({reason})"),
        Event::TokenIndex { .. } => "token index updated".to_owned(),
        Event::Changed | Event::Mode { .. } | Event::Unknown { .. } => return None,
    };
    Some(text)
}

/// One of auto's kinds (§11.4), from its JSON, in auto's own words where it has them.
fn auto(kind: &str, raw: &Value, poll: Option<&Value>, tz: &TimeZone) -> Option<String> {
    let str_of = |field: &str| raw[field].as_str().unwrap_or_default().to_owned();
    Some(match kind {
        "poll" => {
            let active = raw["active"]["number"].as_u64();
            match active {
                Some(n) => match raw["headroomPct"][n.to_string()].as_f64() {
                    Some(headroom) => format!("auto: polled · {n} at {}%", used(headroom)),
                    None => format!("auto: polled · {n}"),
                },
                None => "auto: polled".to_owned(),
            }
        }
        "switch" => {
            let (from, to) = (raw["from"].as_u64()?, raw["to"].as_u64()?);
            let why = poll.and_then(|p| binding(p, from)).map_or_else(
                || str_of("trigger"),
                |(window, pct)| format!("{window} at {pct}%"),
            );
            let verb = if raw["dryRun"].as_bool() == Some(true) {
                "auto would switch"
            } else {
                "auto"
            };
            format!("{verb}: {from} → {to} ({why})")
        }
        "no-switch" => {
            let (reason, detail) = (str_of("reason"), str_of("detail"));
            if detail.is_empty() {
                format!("auto: no switch: {reason}")
            } else {
                format!("auto: no switch: {reason} ({detail})")
            }
        }
        "all-exhausted" => match raw["earliestResetAt"].as_str().and_then(iso_ms) {
            Some(at) => format!(
                "auto: no switch: all-exhausted, the first back at {}",
                clock(at, tz)
            ),
            None => "auto: no switch: all-exhausted".to_owned(),
        },
        "account-quarantined" | "account-unquarantined" => {
            let what = kind.trim_start_matches("account-");
            format!(
                "{} {} {what} ({})",
                raw["number"].as_u64()?,
                str_of("email"),
                str_of("reason")
            )
        }
        "error" => format!("error: {}", str_of("message")),
        "config-warning" => format!("warning: {}", str_of("message")),
        _ => return None,
    })
}

/// A headroom as the usage it leaves, rounded as `list` rounds a percentage.
fn used(headroom: f64) -> i64 {
    (100.0 - headroom).round() as i64
}

/// The window that bound account `position` at `poll`: its relevant usage is 100 less its
/// `headroomPct`, and the binding window is the one at that usage among its `windowsPct` (D6.1
/// picks it the same way).
fn binding(poll: &Value, position: u64) -> Option<(String, i64)> {
    let at = position.to_string();
    let headroom = poll["headroomPct"][&at].as_f64()?;
    let usage = 100.0 - headroom;
    poll["windowsPct"][&at]
        .as_object()?
        .iter()
        .find(|(_, pct)| pct.as_f64().is_some_and(|p| (p - usage).abs() < 0.05))
        .map(|(window, _)| (window.clone(), used(headroom)))
}
```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust
//! Live mode (spec §6.3, §8): the daemon's link as the state machine sees it. What the live
//! thread learns, and the daemon lane's `status` replies, arrive as messages; what they ask for
//! leaves as commands. The store stays the source of truth: an event is a reason to read the
//! views again, and only `status` says what the daemon does (§7.4, §8).

pub mod words;

use tagteam_core::{AccountId, ProviderId};
use tagteam_daemon::protocol::{Event, Status, StatusProvider};

use crate::live::Live;
```

with:

```rust
//! Live mode (spec §6.3, §8): the daemon's link as the state machine sees it. What the live
//! thread learns, and the daemon lane's `status` replies, arrive as messages; what they ask for
//! leaves as commands. The store stays the source of truth: an event is a reason to read the
//! views again, and only `status` says what the daemon does (§7.4, §8). An event's words show
//! on the activity line (§5.1).

pub mod words;

use std::collections::BTreeMap;

use serde_json::Value;
use tagteam_core::{AccountId, ProviderId};
use tagteam_daemon::protocol::{Event, Status, StatusProvider};

use crate::live::Live;
```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust
    "account-quarantined",
    "account-unquarantined",
];

/// The daemon as the TUI knows it (spec §8).
#[derive(Default)]
pub struct DaemonState {
    /// The link followed now, by the live thread's name for it (C-R21): a `status` reply asked
```

with:

```rust
    "account-quarantined",
    "account-unquarantined",
];

/// The activity line (spec §5.1): the daemon's latest event, when it happened, and whose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub at_ms: i64,
    pub provider: Option<String>,
    pub text: String,
}

/// The daemon as the TUI knows it (spec §8).
#[derive(Default)]
pub struct DaemonState {
    /// The link followed now, by the live thread's name for it (C-R21): a `status` reply asked
```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust
    status_again: bool,
    /// C-R20: until when the first collection waits to know who collects: no daemon, or the
    /// daemon's first `status`. `None` once known, and for a TUI that follows no daemon.
    unsettled_until: Option<i64>,
}

impl DaemonState {
    /// Live mode: subscribed, and the daemon's `status` has come since (spec §8).
```

with:

```rust
    status_again: bool,
    /// C-R20: until when the first collection waits to know who collects: no daemon, or the
    /// daemon's first `status`. `None` once known, and for a TUI that follows no daemon.
    unsettled_until: Option<i64>,
    /// The latest event the activity line shows.
    activity: Option<Activity>,
    /// Each provider's last `poll` on this link, whose windows a `switch` of the same tick
    /// names. A link can miss events before it, so a new one starts with none (C-R21).
    polls: BTreeMap<String, Value>,
}

impl DaemonState {
    /// Live mode: subscribed, and the daemon's `status` has come since (spec §8).
```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust
    /// collection.
    pub(crate) fn await_daemon(&mut self, now_ms: i64) {
        self.unsettled_until = Some(now_ms + SETTLE_MS);
    }
}

/// What the live thread learned (spec §8).
pub(crate) fn on_live(state: &mut State, live: Live, now_ms: i64) -> Vec<Cmd> {
```

with:

```rust
    /// collection.
    pub(crate) fn await_daemon(&mut self, now_ms: i64) {
        self.unsettled_until = Some(now_ms + SETTLE_MS);
    }

    /// The daemon's latest event, for the activity line, which only live mode shows (§5.1).
    pub fn activity(&self) -> Option<&Activity> {
        self.activity.as_ref()
    }
}

/// The provider the header follows and `P` acts on (spec §7.4): the selected account's, else
/// `--provider`'s, else the default, which Tokens reports on.
pub(crate) fn focus(state: &State) -> ProviderId {
    state
        .accounts
        .selected()
        .map(|a| a.provider)
        .or_else(|| state.provider.clone())
        .unwrap_or_else(|| state.tokens.provider().clone())
}

/// What the live thread learned (spec §8).
pub(crate) fn on_live(state: &mut State, live: Live, now_ms: i64) -> Vec<Cmd> {
```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust
    d.link = Some(link);
    d.status = None;
    d.status_due_ms = None;
    d.status_again = false;
    d.stopping = None;
    d.mismatch = None;
    d.next_poll_ms = now_ms + POLL_MS;
    d.reread_ms = Some(now_ms + REREAD_MS);
```

with:

```rust
    d.link = Some(link);
    d.status = None;
    d.status_due_ms = None;
    d.status_again = false;
    d.polls.clear();
    d.stopping = None;
    d.mismatch = None;
    d.next_poll_ms = now_ms + POLL_MS;
    d.reread_ms = Some(now_ms + REREAD_MS);
```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust
fn on_event(state: &mut State, event: Event, now_ms: i64) -> Vec<Cmd> {
    if state.daemon.link.is_none() || state.quitting.is_some() {
        return vec![];
    }
    match event {
        Event::Auto { kind, .. } => {
            let views = VIEW_KINDS.contains(&kind.as_str());
            reread(state, now_ms, views, true);
```

with:

```rust
fn on_event(state: &mut State, event: Event, now_ms: i64) -> Vec<Cmd> {
    if state.daemon.link.is_none() || state.quitting.is_some() {
        return vec![];
    }
    note(state, &event, now_ms);
    match event {
        Event::Auto { kind, .. } => {
            let views = VIEW_KINDS.contains(&kind.as_str());
            reread(state, now_ms, views, true);
```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust
    }
    vec![]
}

/// What an event asks for is read `REREAD_MS` from the first event that asks, once for every
/// event until then: the views, `status`, or both.
fn reread(state: &mut State, now_ms: i64, views: bool, status: bool) {
    let d = &mut state.daemon;
```

with:

```rust
    }
    vec![]
}

/// The activity line's record of `event` (spec §5.1): its words, and its `ts`, or now for a kind
/// whose typed form carries none. A `poll` is kept for the `switch` of its tick.
fn note(state: &mut State, event: &Event, now_ms: i64) {
    let (provider, raw) = match event {
        Event::Auto { provider, raw, .. } | Event::TokenIndex { provider, raw } => {
            (Some(provider.clone()), Some(raw))
        }
        Event::Usage { provider, .. } => (Some(provider.clone()), None),
        _ => (None, None),
    };
    let d = &mut state.daemon;
    let poll = provider.as_ref().and_then(|p| d.polls.get(p));
    if let Some(text) = words::activity(event, poll, state.tokens.tz()) {
        let ts = raw.and_then(|r| r["ts"].as_str()).and_then(words::iso_ms);
        d.activity = Some(Activity {
            at_ms: ts.unwrap_or(now_ms),
            provider,
            text,
        });
        state.dirty = true;
    }
    if let Event::Auto {
        kind,
        provider,
        raw,
    } = event
    {
        if kind == "poll" {
            d.polls.insert(provider.clone(), raw.clone());
        }
    }
}

/// What an event asks for is read `REREAD_MS` from the first event that asks, once for every
/// event until then: the views, `status`, or both.
fn reread(state: &mut State, now_ms: i64, views: bool, status: bool) {
    let d = &mut state.daemon;
```

In `crates/tagteam-tui/src/screens/tokens/mod.rs`, replace:

```rust
    pub fn provider(&self) -> &ProviderId {
        &self.provider
    }

    pub fn period(&self) -> TokenPeriod {
        self.period
    }

```

with:

```rust
    pub fn provider(&self) -> &ProviderId {
        &self.provider
    }

    /// The zone the screen's days and months are in (B6), which the TUI's other times use too.
    pub fn tz(&self) -> &TimeZone {
        &self.tz
    }

    pub fn period(&self) -> TokenPeriod {
        self.period
    }

```

In `crates/tagteam-tui/src/view.rs`, replace:

```rust
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
```

with:

```rust
use tagteam_engine::words::Severity;
use unicode_width::UnicodeWidthStr;

use crate::actions;
use crate::daemon::words;
use crate::screens;
use crate::state::{Screen, State};

/// Below this the body is replaced by `TOO_SMALL` (spec §5.1).
pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;
pub const TOO_SMALL: &str = "terminal too small (80×24 needed)";
/// The header's mode in timer mode (spec §5.1); live mode's is `daemon::words::mode`'s.
pub const MODE_TIMER: &str = "○ refresh 30s";
/// The header's hint at the help panel, which gives way first when the mode needs the room.
const HELP_HINT: &str = "? help ";
/// The spinner's frames, one per tick while an action runs.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The footer's keys for each screen, in the order they are dropped from the end when the
```

In `crates/tagteam-tui/src/view.rs`, replace:

```rust
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

/// The body's area at the terminal's last known size, for the keys that page or scroll.
```

with:

```rust
pub(crate) struct Areas {
    pub(crate) header: Rect,
    pub(crate) banner: Option<Rect>,
    pub(crate) body: Rect,
    /// The activity line, in live mode only (§5.1).
    pub(crate) activity: Option<Rect>,
    pub(crate) footer: Rect,
}

/// The header on the first row, or two when its mode needs a row of its own; the banner (when
/// there is one) below it; the footer on the last rows, with the activity line above it in live
/// mode; and the body between. The banner, and a toast in the footer, take as many rows as their
/// words need at the width, `WRAP_ROWS` at most, from the body, which keeps a row at least.
pub(crate) fn areas(state: &State, area: Rect) -> Areas {
    let width = usize::from(area.width);
    let rows = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
    let head = rows(header(state, area.width).len()).min(area.height);
    let activity = u16::from(state.daemon.live()).min(area.height.saturating_sub(head));
    let spare = area.height.saturating_sub(head + activity + 2);
    let banner = banner(state).map_or(0, |(text, _)| rows(banner_rows(&text, width).len()));
    let banner = banner.min(spare);
    let footer = toast_rows(state, width).map_or(1, |t| rows(t.len()));
    let footer = footer
        .min(spare - banner + 1)
        .min(area.height.saturating_sub(head + activity));
    let top = area.y + head + banner;
    let bottom = area.y + area.height.saturating_sub(footer);
    Areas {
        header: Rect::new(area.x, area.y, area.width, head),
        banner: (banner > 0).then(|| Rect::new(area.x, area.y + head, area.width, banner)),
        body: Rect::new(
            area.x,
            top,
            area.width,
            bottom.saturating_sub(activity).saturating_sub(top),
        ),
        activity: (activity > 0).then(|| Rect::new(area.x, bottom - activity, area.width, 1)),
        footer: Rect::new(area.x, bottom, area.width, footer),
    }
}

/// The body's area at the terminal's last known size, for the keys that page or scroll.
```

In `crates/tagteam-tui/src/view.rs`, replace:

```rust
        return;
    }
    let areas = areas(state, area);
    frame.render_widget(Paragraph::new(header(state, area.width)), areas.header);
    if let (Some(rect), Some((text, error))) = (areas.banner, banner(state)) {
        // An error takes the critical severity's style, anything else the warning's.
        let severity = if error {
            Severity::Critical
```

with:

```rust
        return;
    }
    let areas = areas(state, area);
    frame.render_widget(Paragraph::new(header(state, area.width)), areas.header);
    if let Some(rect) = areas.activity {
        frame.render_widget(Paragraph::new(activity(state, area.width)), rect);
    }
    if let (Some(rect), Some((text, error))) = (areas.banner, banner(state)) {
        // An error takes the critical severity's style, anything else the warning's.
        let severity = if error {
            Severity::Critical
```

In `crates/tagteam-tui/src/view.rs`, replace:

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
```

with:

```rust
    );
    actions::render_overlay(state, frame, now_ms);
}

/// ` tagteam  [1 Accounts]  2 Tokens  3 History` on the left, the mode (`daemon::words::mode`)
/// and `? help` on the right. A mode too wide for the row drops `? help`; one too wide even
/// then takes a second row of its own, right-aligned, cut to the width (R20).
fn header(state: &State, width: u16) -> Vec<Line<'static>> {
    let mut tabs = vec![Span::styled(" tagteam", bold())];
    for (screen, label) in [
        (Screen::Accounts, "1 Accounts"),
        (Screen::Tokens, "2 Tokens"),
        (Screen::History, "3 History"),
    ] {
        tabs.push(Span::raw("  "));
        if screen == state.screen {
            tabs.push(Span::styled(format!("[{label}]"), bold()));
        } else {
            tabs.push(Span::raw(label));
        }
    }
    let (width, mode) = (usize::from(width), words::mode(state));
    let used: usize = tabs.iter().map(|s| width_of(&s.content)).sum();
    let fits = |right: &str| used + 1 + width_of(right) <= width;
    for right in [format!("{mode}   {HELP_HINT}"), format!("{mode} ")] {
        if fits(&right) {
            return vec![right_aligned(tabs, used, right, width)];
        }
    }
    let first = if fits(HELP_HINT) {
        right_aligned(tabs, used, HELP_HINT.to_owned(), width)
    } else {
        Line::from(tabs)
    };
    let mode = format!("{} ", fit(&mode, width.saturating_sub(2)));
    let pad = " ".repeat(width.saturating_sub(width_of(&mode)));
    vec![first, Line::from(format!("{pad}{mode}"))]
}

/// `spans`, `used` columns wide, then `right` against the right edge of `width`.
fn right_aligned(
    mut spans: Vec<Span<'static>>,
    used: usize,
    right: String,
    width: usize,
) -> Line<'static> {
    let gap = width.saturating_sub(used + width_of(&right));
    spans.push(Span::raw(" ".repeat(gap.max(1))));
    spans.push(Span::raw(right));
    Line::from(spans)
}

/// The activity line (spec §5.1): the daemon's latest event, at its time in the TUI's zone,
/// naming its provider when the TUI shows several. Blank until the first event.
fn activity(state: &State, width: u16) -> Line<'static> {
    let Some(a) = state.daemon.activity() else {
        return Line::default();
    };
    let time = words::clock(a.at_ms, state.tokens.tz());
    let several = state
        .accounts
        .snapshot()
        .is_some_and(|s| s.providers.len() > 1);
    let text = match &a.provider {
        Some(provider) if several => format!(" {time} {provider}  {}", a.text),
        _ => format!(" {time} {}", a.text),
    };
    Line::from(fit(&text, usize::from(width)))
}

/// The footer: `finishing <action>…` once the shutdown has begun, else the toast (on as many
/// rows as it needs, `WRAP_ROWS` at most), else the running action's spinner, else the screen's
/// keys (spec §5.1, §10.2 step 4).
fn footer(state: &State, width: u16, now_ms: i64) -> Vec<Line<'static>> {
```

In `crates/tagteam-tui/Cargo.toml`, replace:

```toml
crossterm.workspace = true
jiff.workspace = true
libc.workspace = true
ratatui.workspace = true
unicode-width.workspace = true

[dev-dependencies]
tagteam-cc.workspace = true
tagteam-fake.workspace = true
serde_json.workspace = true
tempfile.workspace = true

[lints]
workspace = true
```

with:

```toml
crossterm.workspace = true
jiff.workspace = true
libc.workspace = true
ratatui.workspace = true
serde_json.workspace = true
unicode-width.workspace = true

[dev-dependencies]
tagteam-cc.workspace = true
tagteam-fake.workspace = true
tempfile.workspace = true

[lints]
workspace = true
```

- [ ] **Step 4: Run the tests and see them pass**

Run, outside the sandbox: `cargo test -p tagteam-tui --lib`

Expected: `282 passed` (11 new: 4 in `daemon::words`, 7 in `view`).

- [ ] **Step 5: Format and lint**

Run:
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`

Expected: no output from the first; `Finished` and no warnings from the others.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-tui/src/daemon/words.rs crates/tagteam-tui/src/daemon/mod.rs crates/tagteam-tui/src/screens/tokens/mod.rs crates/tagteam-tui/src/view.rs crates/tagteam-tui/Cargo.toml
git commit -m "Show the daemon's state in the header and its latest event on the activity line"
```

### Task 4: The token index in live mode

Spec §6.4's live mode, in `screens/tokens/index.rs`:

- **A daemon that keeps the index current** (`daemon::keeps_index`, C-R12) replaces the TUI's 60 s refresh: one that runs a job for the Tokens provider and has the index job, working for it. `r` on Tokens still refreshes. A `Busy` while it does is not news. When the link goes, the timer is back.
- **`token-index` events** (C-R13) read the report again when their `changedAt` is newer than the report on screen, at most every 5 s (`LIVE_REQUERY_MS`): the first at once, the rest of the 5 s in one read at its end, which has the last change.

**Readings of the spec this task commits to:**
- **"A daemon whose `status.jobs` includes the index refresh job"** (§6.4): `token-index` (`tagteam_daemon::tokens::TOKEN_INDEX`), whose `lastError` names neither the Tokens provider nor the store. A job that fails for this provider does not keep its index current, so the TUI refreshes it, and its own refresh says why (B12).
- **The job's providers:** `status.jobs` has one line for all providers, and the job serves the providers the daemon runs (D13), which `status.providers` names. A Tokens provider missing from it is not covered: a daemon started with `--provider` for another, or one whose job for it has not yet run.
- **The live tests' daemon answers `status`** (`live_tick`) where a test means the link to last: a fetch left unanswered for 5 s ends it (C-R19).
- **"Newer than the report on screen"** compares `changedAt`, to the second, with the report's `index.changed_at_ms`, to the millisecond. An event in the report's own second counts as newer: at worst one read too many, never one missed.
- **The first refresh still follows the first frame** (B5): whether the TUI is live is not known yet. The daemon's lock makes it `Busy`, or it runs; either is harmless.
- **Off Tokens an event reads nothing;** opening Tokens reads the report again anyway (B19).

**Depends on:** 3

**Files:**
- Modify: `crates/tagteam-tui/src/screens/tokens/index.rs`
- Modify: `crates/tagteam-tui/src/daemon/mod.rs`

**Interfaces:**
- Consumes: Task 2's `DaemonState::status`, `go_live`; Task 3's `words::iso_ms`; part B's `IndexWatch`, `requery`, `refresh`; `tagteam_daemon::tokens::TOKEN_INDEX`; `tagteam_daemon::protocol::StatusJob`.
- Produces: CC3's `keeps_index`; CC4's `index.rs` additions.

**Spec:**
- TUI spec §6.4 (live mode: no timer with the index job, `token-index` re-runs the report when `changedAt` is newer than the report on screen, `r` still refreshes, a daemon without the job leaves the timer; when to re-query), §8 (`token-index`), §9 (the event's fields), §11 ("Token report": re-querying for an index change).
- Daemon spec D13 and its erratum (the job polls; `skippedLines`); token report spec §12.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-tui/src/screens/tokens/index.rs`, replace:

```rust
}

#[cfg(test)]
mod tests {
    use tagteam_engine::tokens::{ActiveBlock, IndexReport, TokenPeriod, TokenRow};

    use super::*;
    use crate::msg::Msg;
    use crate::testutil::*;
    use crate::update::{drawn, update};

```

with:

```rust
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tagteam_daemon::protocol::{Event, Status, StatusJob};
    use tagteam_engine::tokens::{ActiveBlock, IndexReport, TokenPeriod, TokenRow};

    use super::*;
    use crate::live::Live;
    use crate::msg::Msg;
    use crate::testutil::*;
    use crate::update::{drawn, update};

```

In `crates/tagteam-tui/src/screens/tokens/index.rs`, replace:

```rust
            NOW_MS + 6_100,
        );
        assert_eq!(state.tokens.error(), None);
    }
}
```

with:

```rust
            NOW_MS + 6_100,
        );
        assert_eq!(state.tokens.error(), None);
    }

    /// The fixture daemon switching Claude Code's accounts, with the index job (D13), whose last
    /// run failed with `error`, when `error` is given.
    fn with_index_job(error: Option<&str>) -> Status {
        let job = StatusJob {
            name: "token-index".into(),
            last_run_at: None,
            next_run_at: None,
            last_error: error.map(str::to_owned),
        };
        daemon_status(vec![daemon_job("claude-code", "auto", true)], vec![job])
    }

    /// `on_tokens`, its first refresh returned at `NOW_MS`, then live on a daemon whose
    /// `status` is `status`, and the report the link reads again answered: changed a minute
    /// before `NOW_MS`, as before.
    fn live_on_tokens(status: Status) -> State {
        let mut state = on_tokens();
        update(&mut state, Msg::Refreshed { result: done() }, NOW_MS);
        go_live(&mut state, status);
        let again = tick(&mut state, NOW_MS + 250);
        answer(&mut state, &again, vec![], NOW_MS - 60_000, NOW_MS + 250);
        state
    }

    /// The daemon's `token-index` event for `provider`, its index changed at `changed_ms`.
    fn token_index(provider: &str, changed_ms: Option<i64>) -> Msg {
        let changed =
            changed_ms.map(|ms| jiff::Timestamp::from_millisecond(ms).unwrap().to_string());
        let raw = json!({"event": "token-index", "provider": provider, "changedAt": changed});
        Msg::Live(Live::Event(Event::TokenIndex {
            provider: provider.into(),
            raw,
        }))
    }

    fn refreshes(cmds: &[Cmd]) -> bool {
        cmds.contains(&Cmd::RefreshIndex { provider: cc() })
    }

    /// `tick` on a live TUI whose daemon answers each `status` it asks for with the last one,
    /// as a working daemon does (C-R19 drops a link whose `status` does not come).
    fn live_tick(state: &mut State, at_ms: i64) -> Vec<Cmd> {
        let cmds = update(state, Msg::Tick, at_ms);
        for c in &cmds {
            if let (Cmd::FetchStatus { link }, Some(status)) = (c, state.daemon.status()) {
                let result = Ok(status.clone());
                update(
                    state,
                    Msg::DaemonStatus {
                        link: *link,
                        result,
                    },
                    at_ms,
                );
            }
        }
        token_cmds(cmds)
    }

    #[test]
    fn a_daemon_that_keeps_the_index_takes_the_timer_s_place_and_r_still_refreshes() {
        // Spec §6.4: in live mode with the index job, no refresh on a timer; `r` is a person's
        // request. Without the link, the timer is back.
        let mut state = live_on_tokens(with_index_job(None));
        assert!(!refreshes(&live_tick(&mut state, NOW_MS + 60_000)));
        assert!(!refreshes(&live_tick(&mut state, NOW_MS + 180_000)));
        assert!(state.daemon.live(), "the daemon answered throughout");
        assert_eq!(
            update(&mut state, ch('r'), NOW_MS + 181_000),
            [Cmd::RefreshIndex { provider: cc() }]
        );
        update(
            &mut state,
            Msg::Refreshed { result: done() },
            NOW_MS + 182_000,
        );
        update(&mut state, Msg::Live(Live::Down), NOW_MS + 250_000);
        assert!(
            refreshes(&tick(&mut state, NOW_MS + 250_250)),
            "60 s after the last returned"
        );
    }

    #[test]
    fn a_daemon_that_runs_another_provider_only_leaves_the_timer_on() {
        // C-R12: the index job serves the providers the daemon runs (D13); a daemon started
        // with `--provider fake-agent` keeps no Claude Code index, however well its job runs.
        let job = StatusJob {
            name: "token-index".into(),
            last_run_at: None,
            next_run_at: None,
            last_error: None,
        };
        let other = daemon_status(vec![daemon_job("fake-agent", "auto", true)], vec![job]);
        let mut state = live_on_tokens(other);
        assert!(refreshes(&tick(&mut state, NOW_MS + 60_000)));
    }

    #[test]
    fn without_the_index_job_or_with_one_that_fails_here_the_timer_refreshes() {
        // Spec §6.4: a daemon without that job leaves the TUI refreshing as in timer mode; one
        // whose job fails for this provider, or for the store, too.
        for (status, timer) in [
            (
                daemon_status(vec![daemon_job("claude-code", "auto", true)], vec![]),
                true,
            ),
            (
                with_index_job(Some("claude-code: a transcript could not be read")),
                true,
            ),
            (
                with_index_job(Some("store error: database is locked")),
                true,
            ),
            (
                with_index_job(Some("fake-agent: a transcript could not be read")),
                false,
            ),
            (with_index_job(None), false),
        ] {
            let errors = status.jobs.first().and_then(|j| j.last_error.clone());
            let mut state = live_on_tokens(status);
            assert_eq!(
                refreshes(&tick(&mut state, NOW_MS + 60_000)),
                timer,
                "{errors:?}"
            );
        }
    }

    #[test]
    fn a_token_index_event_newer_than_the_report_on_screen_reads_it_again() {
        // Spec §6.4, §8. `changedAt` is to the second: one in the report's own second counts as
        // newer.
        let shown = NOW_MS - 60_000;
        let mut state = live_on_tokens(with_index_job(None));
        let at = NOW_MS + 1_000;
        for (msg, why) in [
            (token_index("claude-code", Some(shown - 1_000)), "older"),
            (token_index("claude-code", None), "never changed"),
            (
                token_index("fake-agent", Some(NOW_MS)),
                "another provider's",
            ),
        ] {
            assert!(token_cmds(update(&mut state, msg, at)).is_empty(), "{why}");
        }
        let again = token_cmds(update(
            &mut state,
            token_index("claude-code", Some(shown)),
            at,
        ));
        assert!(
            matches!(again.as_slice(), [Cmd::LoadReport { .. }]),
            "{again:?}"
        );
        answer(&mut state, &again, vec![], NOW_MS + 500, at);
        // Off Tokens the event reads nothing; opening Tokens reads it again anyway (B19).
        update(&mut state, ch('1'), at + 10_000);
        let later = token_index("claude-code", Some(NOW_MS + 9_000));
        assert!(token_cmds(update(&mut state, later, at + 10_000)).is_empty());
    }

    #[test]
    fn a_catch_up_s_stream_of_events_reads_the_report_every_5_s_and_reads_the_last_change() {
        // During a cold catch-up `changedAt` moves about once a second: the report lane is
        // asked at most every `LIVE_REQUERY_MS`, and the change that came last is read.
        let mut state = live_on_tokens(with_index_job(None));
        let mut read_at = Vec::new();
        let mut changed = NOW_MS;
        let mut serve = |state: &mut State, cmds: Vec<Cmd>, at: i64, changed: i64| {
            if !cmds.is_empty() {
                answer(state, &cmds, vec![], changed, at);
                read_at.push(at - NOW_MS);
            }
        };
        for s in 1..=12 {
            let at = NOW_MS + s * 1_000;
            changed = at;
            let cmds = token_cmds(update(&mut state, token_index("claude-code", Some(at)), at));
            serve(&mut state, cmds, at, changed);
            for quarter in 1..4 {
                let cmds = tick(&mut state, at + quarter * 250);
                serve(&mut state, cmds, at + quarter * 250, changed);
            }
        }
        for quarter in 1..=72 {
            let at = NOW_MS + 12_750 + quarter * 250;
            let cmds = tick(&mut state, at);
            serve(&mut state, cmds, at, changed);
        }
        assert_eq!(read_at, [1_000, 6_000, 11_000, 16_000]);
        assert_eq!(
            state.tokens.report().and_then(|r| r.index.changed_at_ms),
            Some(NOW_MS + 12_000),
            "the last change is on screen"
        );
    }

    #[test]
    fn a_busy_refresh_is_not_news_while_the_daemon_keeps_the_index() {
        // B10's `catching up elsewhere` and its 5 s checks are timer mode's: with a daemon that
        // keeps the index, its events say when the index changes.
        let mut state = on_tokens();
        let busy = || Msg::Refreshed {
            result: Ok(RefreshOutcome::Busy),
        };
        update(&mut state, busy(), NOW_MS);
        assert!(state.tokens.watch().busy());
        go_live(&mut state, with_index_job(None));
        assert!(!state.tokens.watch().busy());
        let shown = draw(&state, 80, 24).join("\n");
        assert!(!shown.contains("catching up elsewhere"), "{shown}");
        let checks = |cmds: Vec<Cmd>| {
            cmds.iter()
                .filter(|c| matches!(c, Cmd::LoadIndexState { .. }))
                .count()
        };
        assert_eq!(checks(tick(&mut state, NOW_MS + 7_000)), 0);
        update(&mut state, ch('r'), NOW_MS + 8_000);
        update(&mut state, busy(), NOW_MS + 8_000);
        assert!(!state.tokens.watch().busy(), "a person's `r` too");
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p tagteam-tui --lib`

Expected: FAIL: the build passes, and outside the sandbox `test result: FAILED. 283 passed; 5 failed`: five of the six new tests in `screens::tokens::index`. The timer still refreshes beside the daemon, a `token-index` event reads nothing, and a `Busy` sticks (`assertion failed: !state.tokens.watch().busy()`). The sixth, `a_daemon_that_runs_another_provider_only_leaves_the_timer_on`, passes already: it pins the timer that a `keeps_index` blind to the provider would stop.

- [ ] **Step 3: Write the code**

In `crates/tagteam-tui/src/screens/tokens/index.rs`, replace:

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

```

with:

```rust
//! The token index on the Tokens screen (spec §5.3 "Indexing", §6.4): the TUI's own catch-up on
//! the index lane, the index's state for the header, and when the report on screen is read
//! again. A daemon that keeps the index current (live mode) takes the timer's place, and its
//! `token-index` events say when the index changed.

use serde_json::Value;
use tagteam_core::AccountId;
use tagteam_engine::tokens::{IndexProgress, IndexState, PeriodKey, RefreshOutcome, TokenReport};

use super::requery;
use crate::daemon::{self, words};
use crate::msg::Cmd;
use crate::source::SourceError;
use crate::state::{Screen, State};

```

In `crates/tagteam-tui/src/screens/tokens/index.rs`, replace:

```rust
/// What a failed check of the index's state is shown as, before the engine's words.
const CHECK_FAILED: &str = "the token index's state was not read: ";
/// `r` while the TUI's own refresh runs.
pub const ALREADY: &str = "the token index is already being refreshed";

/// An account as the report's rows name it (`RowAccount::Account`): its ID, position, label
/// and alias.
type AccountKey = (AccountId, u32, String, Option<String>);
```

with:

```rust
/// What a failed check of the index's state is shown as, before the engine's words.
const CHECK_FAILED: &str = "the token index's state was not read: ";
/// `r` while the TUI's own refresh runs.
pub const ALREADY: &str = "the token index is already being refreshed";
/// Spec §6.4 in live mode: the daemon's `token-index` events read the report again at most this
/// often, the 5 s of a refresh's checks. During a cold catch-up they come about once a second.
pub const LIVE_REQUERY_MS: i64 = 5_000;

/// An account as the report's rows name it (`RowAccount::Account`): its ID, position, label
/// and alias.
type AccountKey = (AccountId, u32, String, Option<String>);
```

In `crates/tagteam-tui/src/screens/tokens/index.rs`, replace:

```rust
    /// The Tokens provider's accounts as the last `accounts` read listed them (B1).
    accounts: Option<Vec<AccountKey>>,
    /// The end of the active block the report was last read again for.
    ended: Option<i64>,
}

impl Default for IndexWatch {
    /// Nothing read yet. The index is refreshed when the screen is first open; the report is
```

with:

```rust
    /// The Tokens provider's accounts as the last `accounts` read listed them (B1).
    accounts: Option<Vec<AccountKey>>,
    /// The end of the active block the report was last read again for.
    ended: Option<i64>,
    /// When a `token-index` event may next read the report again (`LIVE_REQUERY_MS`).
    live_next_ms: i64,
    /// One came sooner: the report is read again at `live_next_ms`.
    live_due: bool,
}

impl Default for IndexWatch {
    /// Nothing read yet. The index is refreshed when the screen is first open; the report is
```

In `crates/tagteam-tui/src/screens/tokens/index.rs`, replace:

```rust
            next_query_ms: i64::MAX,
            next_check_ms: 0,
            accounts: None,
            ended: None,
        }
    }
}

```

with:

```rust
            next_query_ms: i64::MAX,
            next_check_ms: 0,
            accounts: None,
            ended: None,
            live_next_ms: 0,
            live_due: false,
        }
    }
}

```

In `crates/tagteam-tui/src/screens/tokens/index.rs`, replace:

```rust
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
            watch.busy_after = None;
            watch.error = None;
        }
        Err(e) => {
```

with:

```rust
    state: &mut State,
    result: Result<RefreshOutcome, SourceError>,
    now_ms: i64,
) -> Vec<Cmd> {
    let keeps = daemon::keeps_index(state);
    let watch = &mut state.tokens.watch;
    watch.refreshing = None;
    watch.next_refresh_ms = now_ms + REFRESH_MS;
    match result {
        Ok(RefreshOutcome::Done(_)) => {
            watch.busy = false;
            watch.error = None;
        }
        // With a daemon that keeps the index, another process catching up is the usual state
        // of things, and its events say when the index changes.
        Ok(RefreshOutcome::Busy) => {
            watch.busy = !keeps;
            watch.busy_after = None;
            watch.error = None;
        }
        Err(e) => {
```

In `crates/tagteam-tui/src/screens/tokens/index.rs`, replace:

```rust
        watch.state = index;
    }
}

/// The Tokens provider's accounts in `state`'s last `accounts` read.
fn account_keys(state: &State) -> Vec<AccountKey> {
    let Some(snap) = state.accounts.snapshot() else {
        return vec![];
```

with:

```rust
        watch.state = index;
    }
}

/// Spec §6.4: the daemon keeps the index current (`daemon::keeps_index`). A `Busy` the TUI's
/// last refresh met is no longer news: the daemon's events, not the 5 s checks, say when the
/// index changes.
pub(crate) fn on_daemon_index(state: &mut State) {
    state.tokens.watch.busy = false;
}

/// Spec §6.4 in live mode, §8: the daemon's `token-index` event for `provider`. While Tokens shows
/// that provider's report, a `changedAt` newer than the report's reads it again, at most every
/// `LIVE_REQUERY_MS`: an event that comes sooner waits for then, and the report read then has
/// every change since. `changedAt` is to the second, so a change in the second the report's
/// was in counts as newer.
pub(crate) fn on_token_index(
    state: &mut State,
    provider: &str,
    raw: &Value,
    now_ms: i64,
) -> Vec<Cmd> {
    let ours = provider == state.tokens.provider().as_str();
    if !ours || state.screen != Screen::Tokens || state.quitting.is_some() {
        return vec![];
    }
    let Some(changed_ms) = raw["changedAt"].as_str().and_then(words::iso_ms) else {
        return vec![];
    };
    let shown = state.tokens.report().and_then(|r| r.index.changed_at_ms);
    if shown.is_some_and(|shown| changed_ms + 1_000 <= shown) {
        return vec![];
    }
    let watch = &mut state.tokens.watch;
    if now_ms < watch.live_next_ms {
        watch.live_due = true;
        return vec![];
    }
    watch.live_next_ms = now_ms + LIVE_REQUERY_MS;
    watch.live_due = false;
    requery(state, now_ms)
}

/// The Tokens provider's accounts in `state`'s last `accounts` read.
fn account_keys(state: &State) -> Vec<AccountKey> {
    let Some(snap) = state.accounts.snapshot() else {
        return vec![];
```

In `crates/tagteam-tui/src/screens/tokens/index.rs`, replace:

```rust
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
```

with:

```rust
        })
        .min()
}

/// Spec §6.4 on the UI's tick, while Tokens is open: the refresh every 60 s, unless the daemon
/// keeps the index current; the index's state every 5 s while a refresh runs here or elsewhere
/// (`Busy`); the report again every 60 s, once at the active block's end, and when a
/// `token-index` event waited for `LIVE_REQUERY_MS`.
pub(crate) fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.screen != Screen::Tokens || state.quitting.is_some() {
        return vec![];
    }
    let mut cmds = Vec::new();
    let keeps = daemon::keeps_index(state);
    let watch = &state.tokens.watch;
    if watch.refreshing.is_none() && now_ms >= watch.next_refresh_ms && !keeps {
        cmds.extend(refresh(state));
    }
    let watch = &mut state.tokens.watch;
    if watch.live_due && now_ms >= watch.live_next_ms {
        watch.live_due = false;
        watch.live_next_ms = now_ms + LIVE_REQUERY_MS;
        cmds.extend(requery(state, now_ms));
    }
    let watch = &state.tokens.watch;
    if (watch.refreshing.is_some() || watch.busy) && now_ms >= watch.next_check_ms {
        cmds.extend(check(state, now_ms));
    }
```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust

use serde_json::Value;
use tagteam_core::{AccountId, ProviderId};
use tagteam_daemon::protocol::{Event, Status, StatusProvider};

use crate::live::Live;
use crate::msg::Cmd;
use crate::screens;
```

with:

```rust

use serde_json::Value;
use tagteam_core::{AccountId, ProviderId};
use tagteam_daemon::protocol::{Event, Status, StatusProvider};
use tagteam_daemon::tokens::TOKEN_INDEX;

use crate::live::Live;
use crate::msg::Cmd;
use crate::screens;
```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust
            down(state);
            state.daemon.stopping = Some((reason, now_ms + STOPPING_MS));
            state.dirty = true;
        }
        Event::TokenIndex { .. } | Event::Unknown { .. } => {}
    }
    vec![]
}

```

with:

```rust
            down(state);
            state.daemon.stopping = Some((reason, now_ms + STOPPING_MS));
            state.dirty = true;
        }
        Event::TokenIndex { provider, raw } => {
            return screens::tokens::index::on_token_index(state, &provider, &raw, now_ms);
        }
        Event::Unknown { .. } => {}
    }
    vec![]
}

```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust
        Ok(status) => {
            d.status = Some(status);
            d.unsettled_until = None;
            state.dirty = true;
            if !std::mem::take(&mut state.daemon.status_again) {
                return vec![];
            }
            fetch(state, now_ms).into_iter().collect()
```

with:

```rust
        Ok(status) => {
            d.status = Some(status);
            d.unsettled_until = None;
            state.dirty = true;
            if keeps_index(state) {
                screens::tokens::index::on_daemon_index(state);
            }
            if !std::mem::take(&mut state.daemon.status_again) {
                return vec![];
            }
            fetch(state, now_ms).into_iter().collect()
```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust
        Err(_) => broken(state, link),
    }
}

/// Spec §6.3 and §8 on the UI's tick: the end of `stopping`'s words, and of the first
/// collection's wait (C-R20); a link whose `status` did not come (C-R19); the events' debounced
/// reads; and every 60 s, `status`, and in live mode `accounts` (and `history` when History is
/// open).
```

with:

```rust
        Err(_) => broken(state, link),
    }
}

/// Spec §6.4, C-R12: whether the daemon keeps the Tokens provider's index current. In live
/// mode, the daemon runs a job for that provider (its index job serves the providers it runs,
/// D13, which `status` names in `providers`), and its `status.jobs` has the index job, whose
/// last run did not fail for this provider or for the store (its `lastError` names one or the
/// other, as `<provider>: …` or `store…`). A daemon that runs other providers only, or whose job
/// fails, leaves the TUI to refresh on its timer, which shows its own refresh's failure.
pub(crate) fn keeps_index(state: &State) -> bool {
    let provider = state.tokens.provider();
    let Some(status) = state.daemon.status() else {
        return false;
    };
    if state.daemon.job(provider).is_none() {
        return false;
    }
    let ours = format!("{provider}: ");
    status.jobs.iter().any(|job| {
        job.name == TOKEN_INDEX
            && job
                .last_error
                .as_deref()
                .is_none_or(|e| !(e.starts_with(&ours) || e.starts_with("store")))
    })
}

/// Spec §6.3 and §8 on the UI's tick: the end of `stopping`'s words, and of the first
/// collection's wait (C-R20); a link whose `status` did not come (C-R19); the events' debounced
/// reads; and every 60 s, `status`, and in live mode `accounts` (and `history` when History is
/// open).
```

- [ ] **Step 4: Run the tests and see them pass**

Run, outside the sandbox: `cargo test -p tagteam-tui --lib`

Expected: `288 passed` (6 new, in `screens::tokens::index`).

- [ ] **Step 5: Format and lint**

Run:
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`

Expected: no output from the first; `Finished` and no warnings from the others.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-tui/src/screens/tokens/index.rs crates/tagteam-tui/src/daemon/mod.rs
git commit -m "Let a daemon that keeps the token index current replace the index timer, and read the report again on its events"
```

### Task 5: `P`: pause and resume auto-switch

Spec §7.4's `P` (C-R14). It asks `Pause auto-switch for <provider>?` (or `Resume …?`) from the effective `autoswitch` in `status`, for the provider the header follows. A yes runs `Action::Autoswitch` on the actions lane: `config set provider.<id>.daemon.autoswitch false` (or `true`) through `Host`, exactly as the CLI runs it. The socket stays read-only (T3). Two seconds after the outcome, `status` is fetched again, on whichever link is up then, even if none was when the outcome came (C-R21): a pause need not change the daemon's mode, so no event may follow. In timer mode `P` says `no daemon running`.

**Readings of the spec this task commits to:**
- **The dialog's default is no,** as `remove`'s (§7.2): one key changes a setting that lasts across restarts.
- **"`P` shows `no daemon running`" in timer mode,** which includes a link whose first `status` has not come: the TUI cannot yet say what it would change.
- **A provider with no job in `status`** (a daemon started with `--provider` for another) is refused in words of its own: the TUI changes no setting whose effect it cannot show (§7.4).
- **The fetch 2 s later** comes whatever the command's exit: it costs one local request, and shows what the daemon does either way. A link change does not cancel it, before the outcome or after it: the new link's own fetch can come before the daemon applies the setting (D4.2), within the second it takes. A link that comes up only after the 2 s fetches at once, and that fetch serves.
- **The host bridge's test** runs the real CLI path, as the other `Host` tests do, and reads the setting back with `config get`.

**Depends on:** 4

**Files:**
- Modify: `crates/tagteam-tui/src/host.rs`
- Modify: `crates/tagteam-tui/src/actions.rs`
- Modify: `crates/tagteam-tui/src/daemon/mod.rs`
- Modify: `crates/tagteam/tests/tui_host.rs`

**Interfaces:**
- Consumes: Task 2's `DaemonState::{live, job}`, `go_live`; Task 3's `focus` and the header; part A's `Overlay::Confirm`, `ConfirmThen::Run`, `busy`, `start`, `on_done`.
- Produces: CC3's `RECHECK_MS` and `recheck`; CC5.

**Spec:**
- TUI spec §5.6 (`P` everywhere), §7.1 (`P`'s row), §7.2 (the busy rule), §7.4 (all of it), §11 ("Daemon client": `status` re-fetched 2 s after `P`; `P` writing the setting through `Host`; with one switchable account, a pause leaves the mode `collect` and sends no event, and the header still turns to `auto paused`), §13 (T3).
- Daemon spec D4.2 (a setting applies within about a second), D7 (`daemon.autoswitch`).

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-tui/src/host.rs`, replace:

```rust
            "--no-color --provider claude-code add"
        );
    }

    #[test]
    fn run_keeps_its_colour_because_the_agent_output_is_not_ours() {
        assert_eq!(
            args(&Action::Run(account(2))),
```

with:

```rust
            "--no-color --provider claude-code add"
        );
    }

    #[test]
    fn pausing_and_resuming_auto_switch_is_config_set_of_the_provider_s_daemon_setting() {
        // Spec §7.4, D7: the socket stays read-only; the setting is written as the CLI writes
        // it.
        let set = |on| {
            args(&Action::Autoswitch {
                provider: provider(),
                on,
            })
        };
        assert_eq!(
            set(false),
            "--no-color config set provider.claude-code.daemon.autoswitch false"
        );
        assert_eq!(
            set(true),
            "--no-color config set provider.claude-code.daemon.autoswitch true"
        );
        let pause = Action::Autoswitch {
            provider: provider(),
            on: false,
        };
        assert_eq!(pause.name(), "config");
        assert!(!pause.needs_terminal());
    }

    #[test]
    fn run_keeps_its_colour_because_the_agent_output_is_not_ours() {
        assert_eq!(
            args(&Action::Run(account(2))),
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
                }
            ]
        );
    }
}
```

with:

```rust
                }
            ]
        );
    }

    /// Claude Code's job in `mode`, with `autoswitch`.
    fn job_in(mode: &str, autoswitch: bool) -> tagteam_daemon::protocol::Status {
        daemon_status(vec![daemon_job("claude-code", mode, autoswitch)], vec![])
    }

    /// The confirm dialog's question, its default, and the action a yes runs.
    fn asked(state: &State) -> (&str, bool, &Action) {
        match &state.overlay {
            Some(Overlay::Confirm {
                question,
                default_yes,
                then: ConfirmThen::Run(action),
                ..
            }) => (question.as_str(), *default_yes, action),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn p_asks_to_pause_the_daemon_s_auto_switch_and_a_yes_sets_it_through_the_cli() {
        // Spec §7.4: from the effective `autoswitch` in `status`; default no.
        let mut state = loaded(2);
        go_live(&mut state, job_in("auto", true));
        assert!(update(&mut state, ch('P'), NOW_MS).is_empty());
        let pause = Action::Autoswitch {
            provider: cc(),
            on: false,
        };
        assert_eq!(
            asked(&state),
            ("Pause auto-switch for claude-code?", false, &pause)
        );
        let drawn = draw(&state, 80, 24);
        assert!(
            drawn
                .iter()
                .any(|l| l.contains("Pause auto-switch for claude-code?")),
            "{drawn:?}"
        );
        assert!(
            update(&mut state, key(KeyCode::Enter), NOW_MS).is_empty(),
            "no"
        );
        assert!(state.overlay.is_none() && state.running.is_none());
        update(&mut state, ch('P'), NOW_MS);
        assert_eq!(
            update(&mut state, ch('y'), NOW_MS),
            [Cmd::Act(pause.clone())]
        );
        assert_eq!(state.running, Some(pause));
        assert_eq!(draw(&state, 80, 24)[23], "⠋ config…");
    }

    #[test]
    fn p_asks_to_resume_auto_switch_that_is_paused() {
        let mut state = loaded(2);
        go_live(&mut state, job_in("collect", false));
        update(&mut state, ch('P'), NOW_MS);
        let resume = Action::Autoswitch {
            provider: cc(),
            on: true,
        };
        assert_eq!(
            asked(&state),
            ("Resume auto-switch for claude-code?", false, &resume)
        );
    }

    #[test]
    fn p_without_a_daemon_says_so_and_changes_nothing() {
        // Spec §7.4: in timer mode `P` shows `no daemon running`, also while the first
        // `status` is still to come; and so for a provider the daemon runs no job for.
        let mut state = loaded(1);
        assert!(update(&mut state, ch('P'), NOW_MS).is_empty());
        assert_eq!(state.toast.as_ref().unwrap().text, NO_DAEMON_RUNNING);
        assert!(state.overlay.is_none());
        update(
            &mut state,
            Msg::Live(crate::live::Live::Up { link: 1 }),
            NOW_MS,
        );
        update(&mut state, ch('P'), NOW_MS);
        assert_eq!(state.toast.as_ref().unwrap().text, NO_DAEMON_RUNNING);
        let mut state = loaded(1);
        let other = daemon_status(vec![daemon_job("fake-agent", "auto", true)], vec![]);
        go_live(&mut state, other);
        assert!(update(&mut state, ch('P'), NOW_MS).is_empty());
        assert_eq!(
            state.toast.as_ref().unwrap().text,
            "the daemon runs no job for claude-code"
        );
        assert!(state.overlay.is_none());
    }

    #[test]
    fn p_acts_on_the_selected_account_s_provider() {
        // Spec §7.4: with several providers.
        let fake = tagteam_core::ProviderId::new("fake-agent");
        let mut third = account(3, "c@x.co", read(60, 10.0, 20.0, false));
        third.row.provider = fake.clone();
        let mut snap = snapshot(many(1));
        snap.providers
            .push(tagteam_engine::views::ProviderAccounts {
                provider: fake.clone(),
                active_position: None,
                accounts: vec![third],
            });
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        let status = daemon_status(
            vec![
                daemon_job("claude-code", "auto", true),
                daemon_job("fake-agent", "collect", false),
            ],
            vec![],
        );
        go_live(&mut state, status);
        update(&mut state, ch('j'), NOW_MS);
        update(&mut state, ch('P'), NOW_MS);
        let resume = Action::Autoswitch {
            provider: fake,
            on: true,
        };
        assert_eq!(
            asked(&state),
            ("Resume auto-switch for fake-agent?", false, &resume)
        );
    }

    #[test]
    fn p_while_an_action_runs_is_busy() {
        let mut state = loaded(2);
        go_live(&mut state, job_in("auto", true));
        update(&mut state, ch('s'), NOW_MS);
        assert!(update(&mut state, ch('P'), NOW_MS).is_empty());
        assert_eq!(state.toast.as_ref().unwrap().text, "busy: switch");
        assert!(state.overlay.is_none());
    }

    #[test]
    fn two_seconds_after_p_the_status_is_read_and_the_header_says_paused_with_no_event() {
        // Spec §7.4, §11: with one switchable account the job is `collect` before and after,
        // so no `mode` event follows; the fetch 2 s later shows the pause.
        let mut state = loaded(1);
        let link = go_live(&mut state, job_in("collect", true));
        update(&mut state, Msg::Tick, NOW_MS + 250);
        assert!(draw(&state, 80, 24)[0].ends_with("● live · auto off (one account)"));
        update(&mut state, ch('P'), NOW_MS);
        let cmds = update(&mut state, ch('y'), NOW_MS);
        let [Cmd::Act(pause)] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let set = "Set provider.claude-code.daemon.autoswitch = false in ~/.config/tagteam/config.toml.\n";
        let cmds = update(
            &mut state,
            Msg::ActionDone {
                action: pause.clone(),
                outcome: done(0, set, ""),
            },
            NOW_MS + 500,
        );
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadAccounts { .. }]),
            "{cmds:?}"
        );
        assert_eq!(state.toast.as_ref().unwrap().text, set.trim_end());
        let fetch = |cmds: Vec<Cmd>| cmds.contains(&Cmd::FetchStatus { link });
        assert!(!fetch(update(&mut state, Msg::Tick, NOW_MS + 2_499)));
        assert!(fetch(update(&mut state, Msg::Tick, NOW_MS + 2_500)));
        assert!(
            !fetch(update(&mut state, Msg::Tick, NOW_MS + 2_750)),
            "once"
        );
        let reply = Msg::DaemonStatus {
            link,
            result: Ok(job_in("collect", false)),
        };
        update(&mut state, reply, NOW_MS + 2_800);
        assert!(draw(&state, 80, 24)[0].ends_with("● live · auto paused   ? help"));
    }

    #[test]
    fn p_s_fetch_of_the_status_outlives_a_link_change() {
        // C-R21: the link changes after the setting is written. The new link's own fetch can
        // come before the daemon applies the setting (D4.2), so the fetch 2 s after `P` is
        // still made, on the new link.
        let mut state = loaded(1);
        go_live(&mut state, job_in("collect", true));
        update(&mut state, ch('P'), NOW_MS);
        let cmds = update(&mut state, ch('y'), NOW_MS);
        let [Cmd::Act(pause)] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let set = "Set provider.claude-code.daemon.autoswitch = false in ~/.config/tagteam/config.toml.\n";
        let written = Msg::ActionDone {
            action: pause.clone(),
            outcome: done(0, set, ""),
        };
        update(&mut state, written, NOW_MS + 500);
        update(&mut state, Msg::Live(crate::live::Live::Down), NOW_MS + 600);
        let up = update(
            &mut state,
            Msg::Live(crate::live::Live::Up { link: 2 }),
            NOW_MS + 700,
        );
        assert_eq!(up, [Cmd::FetchStatus { link: 2 }]);
        let early = Msg::DaemonStatus {
            link: 2,
            result: Ok(job_in("collect", true)),
        };
        update(&mut state, early, NOW_MS + 800);
        assert!(
            draw(&state, 80, 24)[0].ends_with("● live · auto off (one account)"),
            "read before the daemon applied it"
        );
        let fetch = |cmds: Vec<Cmd>| cmds.contains(&Cmd::FetchStatus { link: 2 });
        assert!(!fetch(update(&mut state, Msg::Tick, NOW_MS + 2_499)));
        assert!(fetch(update(&mut state, Msg::Tick, NOW_MS + 2_500)));
        let applied = Msg::DaemonStatus {
            link: 2,
            result: Ok(job_in("collect", false)),
        };
        update(&mut state, applied, NOW_MS + 2_600);
        assert!(draw(&state, 80, 24)[0].ends_with("● live · auto paused   ? help"));
    }

    #[test]
    fn p_s_fetch_of_the_status_is_kept_when_its_outcome_comes_with_no_link_up() {
        // C-R21: the link drops before the setting's outcome arrives. The fetch 2 s after the
        // outcome is still owed. A link up before then fetches at the 2 s; one up only after
        // them fetches once, at once, which is after the daemon applied the setting.
        let set = "Set provider.claude-code.daemon.autoswitch = false in ~/.config/tagteam/config.toml.\n";
        let fetches = |cmds: &[Cmd]| {
            cmds.iter()
                .filter(|c| matches!(c, Cmd::FetchStatus { .. }))
                .count()
        };
        for up_at in [NOW_MS + 1_500, NOW_MS + 4_000] {
            let mut state = loaded(1);
            go_live(&mut state, job_in("collect", true));
            update(&mut state, ch('P'), NOW_MS);
            let cmds = update(&mut state, ch('y'), NOW_MS);
            let [Cmd::Act(pause)] = cmds.as_slice() else {
                panic!("{cmds:?}");
            };
            update(&mut state, Msg::Live(crate::live::Live::Down), NOW_MS + 100);
            let written = Msg::ActionDone {
                action: pause.clone(),
                outcome: done(0, set, ""),
            };
            update(&mut state, written, NOW_MS + 500);
            let down = update(&mut state, Msg::Tick, NOW_MS + 1_000);
            assert_eq!(fetches(&down), 0, "no link to fetch on");
            let up = update(
                &mut state,
                Msg::Live(crate::live::Live::Up { link: 2 }),
                up_at,
            );
            assert_eq!(up, [Cmd::FetchStatus { link: 2 }], "{up_at}");
            let applied = up_at >= NOW_MS + 2_500;
            let first = Msg::DaemonStatus {
                link: 2,
                result: Ok(job_in("collect", !applied)),
            };
            update(&mut state, first, up_at + 100);
            if !applied {
                let early = update(&mut state, Msg::Tick, NOW_MS + 2_499);
                assert_eq!(fetches(&early), 0, "{up_at}");
                let due = update(&mut state, Msg::Tick, NOW_MS + 2_500);
                assert_eq!(fetches(&due), 1, "{up_at}");
                let paused = Msg::DaemonStatus {
                    link: 2,
                    result: Ok(job_in("collect", false)),
                };
                update(&mut state, paused, NOW_MS + 2_600);
            }
            assert!(
                draw(&state, 80, 24)[0].ends_with("● live · auto paused   ? help"),
                "{up_at}"
            );
            let later = update(&mut state, Msg::Tick, up_at + 250);
            assert_eq!(fetches(&later), 0, "owed once: {up_at}");
            let later = update(&mut state, Msg::Tick, NOW_MS + 6_000);
            assert_eq!(fetches(&later), 0, "owed once: {up_at}");
        }
    }
}
```

In `crates/tagteam/tests/tui_host.rs`, replace:

```rust
    );
    assert_eq!(home.rows().len(), 1);
}

#[test]
fn an_account_moved_elsewhere_is_refused_as_stale_and_nothing_changes() {
    // Spec §4.5: another process moved `a` after the TUI read it; position 1 now holds `b`.
    let home = Home::with_two();
```

with:

```rust
    );
    assert_eq!(home.rows().len(), 1);
}

#[test]
fn p_writes_the_daemon_s_autoswitch_setting_through_config_set_as_the_cli_does() {
    // Spec §7.4, §11: the socket stays read-only (T3); the setting persists in config.toml.
    let home = Home::with_two();
    let (host, _) = home.host();
    let file = home.env.config_dir().join("config.toml");
    let key = "provider.claude-code.daemon.autoswitch";
    for on in [false, true] {
        let action = Action::Autoswitch {
            provider: ProviderId::new(CLAUDE_CODE),
            on,
        };
        assert_eq!(
            done(host.act(&action, &mut Dialogs::none())),
            (
                0,
                format!("Set {key} = {on} in {}.\n", file.display()),
                String::new()
            ),
        );
        let (code, value, _) = home.cli_run(&["config", "get", key]);
        assert_eq!((code, value), (0, format!("{on}\n")));
    }
}

#[test]
fn an_account_moved_elsewhere_is_refused_as_stale_and_nothing_changes() {
    // Spec §4.5: another process moved `a` after the TUI read it; position 1 now holds `b`.
    let home = Home::with_two();
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p tagteam-tui --lib`

Expected: FAIL: the build stops with ``error[E0599]: no variant named `Autoswitch` found for enum `host::Action` `` and ``error[E0425]: cannot find value `NO_DAEMON_RUNNING` in this scope ``.

- [ ] **Step 3: Write the code**

In `crates/tagteam-tui/src/host.rs`, replace:

```rust
    Doctor {
        provider: Option<ProviderId>,
        online: bool,
    },
}

impl Action {
    /// The arguments after `tagteam`, for the CLI's own parser. An account is named by
```

with:

```rust
    Doctor {
        provider: Option<ProviderId>,
        online: bool,
    },
    /// The daemon's auto-switching for `provider`, on or paused (spec §7.4): its setting
    /// `daemon.autoswitch` (D7).
    Autoswitch {
        provider: ProviderId,
        on: bool,
    },
}

impl Action {
    /// The arguments after `tagteam`, for the CLI's own parser. An account is named by
```

In `crates/tagteam-tui/src/host.rs`, replace:

```rust
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
```

with:

```rust
                    args.push("--online".into());
                }
                args
            }
            Action::Autoswitch { provider, on } => vec![
                "--no-color".into(),
                "config".into(),
                "set".into(),
                format!("provider.{provider}.daemon.autoswitch"),
                on.to_string(),
            ],
        }
    }

    /// The command's name as the CLI spells it: "switch", "enable", "disable", "alias", "move",
    /// "remove", "add", "run", "doctor", "config". The footer shows it while the action runs,
    /// and says `busy: <name>` to a second action key (spec §7.2).
    pub fn name(&self) -> &'static str {
        match self {
            Action::Switch(_) | Action::SwitchBy { .. } => "switch",
            Action::Enable(_) => "enable",
```

In `crates/tagteam-tui/src/host.rs`, replace:

```rust
            Action::Remove(_) => "remove",
            Action::AddLive { .. } => "add",
            Action::Run(_) => "run",
            Action::Doctor { .. } => "doctor",
        }
    }

    /// `Run` only: it always runs suspended (spec §7.3).
```

with:

```rust
            Action::Remove(_) => "remove",
            Action::AddLive { .. } => "add",
            Action::Run(_) => "run",
            Action::Doctor { .. } => "doctor",
            Action::Autoswitch { .. } => "config",
        }
    }

    /// `Run` only: it always runs suspended (spec §7.3).
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use tagteam_engine::words;

use crate::host::{AccountRef, Action, ActionOutcome, Strategy};
use crate::msg::{Answer, Cmd, Prompt, PromptKind};
use crate::screens;
use crate::screens::tokens::filter::{self, FilterDialog};
```

with:

```rust
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use tagteam_engine::words;

use crate::daemon;
use crate::host::{AccountRef, Action, ActionOutcome, Strategy};
use crate::msg::{Answer, Cmd, Prompt, PromptKind};
use crate::screens;
use crate::screens::tokens::filter::{self, FilterDialog};
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
    Alias(AccountRef),
    Move(AccountRef),
}

/// The help panel's keys (spec §5.6): everywhere, then the current screen's.
const HELP_GLOBAL: [(&str, &str); 7] = [
    ("1 2 3  Tab Shift-Tab", "screens"),
    ("j k  ↑ ↓", "move"),
    ("A", "add the current login"),
    ("D", "doctor"),
    ("?", "help"),
    ("q  Ctrl-C", "quit"),
    ("Ctrl-Z", "suspend"),
];
```

with:

```rust
    Alias(AccountRef),
    Move(AccountRef),
}

/// `P` without a daemon to pause (spec §7.4).
pub const NO_DAEMON_RUNNING: &str = "no daemon running";

/// The help panel's keys (spec §5.6): everywhere, then the current screen's.
const HELP_GLOBAL: [(&str, &str); 8] = [
    ("1 2 3  Tab Shift-Tab", "screens"),
    ("j k  ↑ ↓", "move"),
    ("A", "add the current login"),
    ("D", "doctor"),
    ("P", "pause or resume auto-switch"),
    ("?", "help"),
    ("q  Ctrl-C", "quit"),
    ("Ctrl-Z", "suspend"),
];
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
                now_ms,
            ))
        }
        'r' => Some(refresh(state, now_ms)),
        's' | 'b' | 'n' | 'e' | 'a' | 'm' | 'x' | 'R' if state.screen == Screen::Accounts => {
            Some(account_key(state, c, now_ms))
        }
        _ => None,
```

with:

```rust
                now_ms,
            ))
        }
        'r' => Some(refresh(state, now_ms)),
        'P' => Some(pause(state, now_ms)),
        's' | 'b' | 'n' | 'e' | 'a' | 'm' | 'x' | 'R' if state.screen == Screen::Accounts => {
            Some(account_key(state, c, now_ms))
        }
        _ => None,
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
    }
    cmds
}

fn account_key(state: &mut State, c: char, now_ms: i64) -> Vec<Cmd> {
    let (Some(account), Some(view)) = (state.accounts.selected(), state.accounts.selected_view())
    else {
        return vec![];
```

with:

```rust
    }
    cmds
}

/// `P` (spec §7.4): pause the daemon's auto-switching for the provider the header follows, or
/// resume it, as its last `status` says it is. A confirm dialog asks first, default no. Without a
/// daemon (timer mode) it says so and changes nothing, and so for a provider the daemon runs no
/// job for: the TUI changes no setting whose effect it cannot show.
fn pause(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if busy(state, now_ms) {
        return vec![];
    }
    if !state.daemon.live() {
        state.toast(NO_DAEMON_RUNNING, now_ms);
        return vec![];
    }
    let provider = daemon::focus(state);
    let Some(job) = state.daemon.job(&provider) else {
        state.toast(format!("the daemon runs no job for {provider}"), now_ms);
        return vec![];
    };
    let (verb, on) = if job.autoswitch {
        ("Pause", false)
    } else {
        ("Resume", true)
    };
    state.overlay = Some(Overlay::Confirm {
        question: format!("{verb} auto-switch for {provider}?"),
        default_yes: false,
        choice: false,
        then: ConfirmThen::Run(Action::Autoswitch { provider, on }),
    });
    vec![]
}

fn account_key(state: &mut State, c: char, now_ms: i64) -> Vec<Cmd> {
    let (Some(account), Some(view)) = (state.accounts.selected(), state.accounts.selected_view())
    else {
        return vec![];
```

In `crates/tagteam-tui/src/actions.rs`, replace:

```rust
    state.dirty = true;
    if state.quitting.is_some() {
        return vec![];
    }
    let text = [out.trim_end(), err.trim_end()]
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
```

with:

```rust
    state.dirty = true;
    if state.quitting.is_some() {
        return vec![];
    }
    // Spec §7.4: a pause need not change the daemon's mode, so no event may follow; its
    // `status` is fetched again once it has applied the setting.
    if matches!(action, Action::Autoswitch { .. }) {
        daemon::recheck(state, now_ms);
    }
    let text = [out.trim_end(), err.trim_end()]
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust
pub const STATUS_WAIT_MS: i64 = 5_000;
/// C-R20: the first collection waits at most this long for the daemon's first `status`, no
/// longer than the daemon lane's own bound.
pub const SETTLE_MS: i64 = 2_000;

/// §8: the auto kinds that say an account's reading or state changed.
const VIEW_KINDS: [&str; 4] = [
    "poll",
```

with:

```rust
pub const STATUS_WAIT_MS: i64 = 5_000;
/// C-R20: the first collection waits at most this long for the daemon's first `status`, no
/// longer than the daemon lane's own bound.
pub const SETTLE_MS: i64 = 2_000;
/// Spec §7.4: `status` is fetched again this long after `P`'s setting was written. The daemon
/// applies a setting within about a second (D4.2).
pub const RECHECK_MS: i64 = 2_000;

/// §8: the auto kinds that say an account's reading or state changed.
const VIEW_KINDS: [&str; 4] = [
    "poll",
```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust
    /// The protocol of a daemon this TUI does not speak.
    mismatch: Option<u32>,
    /// When the next 60 s fetch of `status` is due.
    next_poll_ms: i64,
    /// When the reads the last events asked for are made, all at once.
    reread_ms: Option<i64>,
    /// They read the views: an account's reading or state changed.
    reread_views: bool,
```

with:

```rust
    /// The protocol of a daemon this TUI does not speak.
    mismatch: Option<u32>,
    /// When the next 60 s fetch of `status` is due.
    next_poll_ms: i64,
    /// When `P`'s fetch of `status` is due, whether or not a link was up when the setting was
    /// written. It is made on whichever link is up then: a new link's own fetch can come before
    /// the daemon applies the setting. A link that comes up after it fetches at once, which
    /// serves (C-R21).
    recheck_ms: Option<i64>,
    /// When the reads the last events asked for are made, all at once.
    reread_ms: Option<i64>,
    /// They read the views: an account's reading or state changed.
    reread_views: bool,
```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust
    d.polls.clear();
    d.stopping = None;
    d.mismatch = None;
    d.next_poll_ms = now_ms + POLL_MS;
    d.reread_ms = Some(now_ms + REREAD_MS);
    d.reread_views = true;
    d.reread_all = true;
    d.reread_status = false;
```

with:

```rust
    d.polls.clear();
    d.stopping = None;
    d.mismatch = None;
    d.next_poll_ms = now_ms + POLL_MS;
    // The link's own fetch comes after `P`'s, if that is due, and serves for it (C-R21).
    if d.recheck_ms.is_some_and(|at| now_ms >= at) {
        d.recheck_ms = None;
    }
    d.reread_ms = Some(now_ms + REREAD_MS);
    d.reread_views = true;
    d.reread_all = true;
    d.reread_status = false;
```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust
    d.reread_status = false;
    state.dirty = true;
}

/// `status`, on the daemon lane, for this link. One is out at a time (C-R22): one asked for
/// while another is out is asked once that one is answered, so that it reads what came after
/// it. The one out is answered within `STATUS_WAIT_MS`, or the link is given up on (C-R19).
/// Nothing is asked once the shutdown has begun.
```

with:

```rust
    d.reread_status = false;
    state.dirty = true;
}

/// Spec §7.4: `P`'s setting was written; `status` is fetched `RECHECK_MS` from now, whether or
/// not a link is up now (C-R21).
pub(crate) fn recheck(state: &mut State, now_ms: i64) {
    state.daemon.recheck_ms = Some(now_ms + RECHECK_MS);
}

/// `status`, on the daemon lane, for this link. One is out at a time (C-R22): one asked for
/// while another is out is asked once that one is answered, so that it reads what came after
/// it. The one out is answered within `STATUS_WAIT_MS`, or the link is given up on (C-R19).
/// Nothing is asked once the shutdown has begun.
```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust
                .is_none_or(|e| !(e.starts_with(&ours) || e.starts_with("store")))
    })
}

/// Spec §6.3 and §8 on the UI's tick: the end of `stopping`'s words, and of the first
/// collection's wait (C-R20); a link whose `status` did not come (C-R19); the events' debounced
/// reads; and every 60 s, `status`, and in live mode `accounts` (and `history` when History is
/// open).
pub(crate) fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    let d = &mut state.daemon;
    if d.stopping
        .as_ref()
```

with:

```rust
                .is_none_or(|e| !(e.starts_with(&ours) || e.starts_with("store")))
    })
}

/// Spec §6.3, §7.4 and §8 on the UI's tick: the end of `stopping`'s words, and of the first
/// collection's wait (C-R20); a link whose `status` did not come (C-R19); the events' debounced
/// reads; `P`'s fetch of `status`; and every 60 s, `status`, and in live mode `accounts` (and
/// `history` when History is open).
pub(crate) fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    let d = &mut state.daemon;
    if d.stopping
        .as_ref()
```

In `crates/tagteam-tui/src/daemon/mod.rs`, replace:

```rust
        views = std::mem::take(&mut d.reread_views);
        all = std::mem::take(&mut d.reread_all);
        fetch_now = std::mem::take(&mut d.reread_status);
    }
    if now_ms >= d.next_poll_ms {
        d.next_poll_ms = now_ms + POLL_MS;
        fetch_now = true;
        views |= d.live();
```

with:

```rust
        views = std::mem::take(&mut d.reread_views);
        all = std::mem::take(&mut d.reread_all);
        fetch_now = std::mem::take(&mut d.reread_status);
    }
    if d.recheck_ms.is_some_and(|at| now_ms >= at) {
        d.recheck_ms = None;
        fetch_now = true;
    }
    if now_ms >= d.next_poll_ms {
        d.next_poll_ms = now_ms + POLL_MS;
        fetch_now = true;
        views |= d.live();
```

- [ ] **Step 4: Run the tests and see them pass**

Run:
- Outside the sandbox: `cargo test -p tagteam-tui --lib` — Expected: `297 passed` (9 new: 8 in `actions`, 1 in `host`).
- `cargo test -p tagteam --features test-support --test tui_host` — Expected: `23 passed` (1 new).

- [ ] **Step 5: Format and lint**

Run:
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`

Expected: no output from the first; `Finished` and no warnings from the others.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-tui/src/host.rs crates/tagteam-tui/src/actions.rs crates/tagteam-tui/src/daemon/mod.rs crates/tagteam/tests/tui_host.rs
git commit -m "Pause and resume the daemon's auto-switching with P, through config set"
```

### Task 6: End to end beside a real daemon, timing, and docs

The real binary in a pty beside a real `tagteam daemon` on the fixture home (spec §11 "End to end"):
- an `R` session ended with the terminal's Ctrl-C leaves the daemon connection live;
- `P` pauses the daemon's auto-switching, the header turns to `auto paused`, and `P` resumes it;
- a daemon that stops leaves the TUI on its timer, saying `daemon stopping (signal)`, until the next one answers and the TUI is live again.

The timing tests measure the first frame beside a running daemon, and beside a socket that accepts and never answers (spec §1.1 criterion 2). The README and CHANGELOG say what live mode and `P` do.

**Readings of the spec this task commits to:**
- **The pty tests' daemon** runs on a fixture home in a short temporary directory (D14), with notifications off, so it starts no `osascript`. It switches the fixture's two accounts (`auto`), so the header reads `● live · auto on`. It is killed if a test fails.
- **The silent socket** is a listener the test holds, which accepts each connection and never writes: each TUI's live thread waits out the 250 ms greeting bound, beside the first frame.

**Depends on:** 5

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `README.md`
- Modify: `crates/tagteam/tests/perf.rs` (two timing tests and their helper)
- Modify: `crates/tagteam/tests/tui_pty.rs`

**Interfaces:**
- Consumes: everything above; the tests' `common::{Guarded, short_tempdir}`, `tagteam_engine::daemon::daemon_socket_path`.
- Produces: nothing new.

**Spec:**
- TUI spec §11 ("End to end": Ctrl-C ends the stub and the TUI carries on, with the daemon connection still live; "Performance": the first frame within 50 ms p95), §12.3 (the end-to-end tests against the merged daemon).

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam/tests/perf.rs`, replace:

```rust
//! §1.1's timing targets through the real binary: `statusline` within 10 ms p95, `list` within
//! 50 ms p95 when no usage fetch is due, alone and while a daemon runs against the same store
//! (D1.1 criterion 3, D12), and the TUI's first frame within 50 ms p95 under the same
//! conditions (TUI spec §1.1), with the token index caught up or not. Timings mean something
//! only in an optimized build on an idle machine, so this file exists only in release builds
//! and its tests are ignored by default. The TUI's opens a pseudo-terminal, so run them
//! outside a sandbox:
//! `cargo test --release -p tagteam --features test-support --test perf -- --ignored`
```

with:

```rust
//! §1.1's timing targets through the real binary: `statusline` within 10 ms p95, `list` within
//! 50 ms p95 when no usage fetch is due, alone and while a daemon runs against the same store
//! (D1.1 criterion 3, D12), and the TUI's first frame within 50 ms p95 under the same
//! conditions (TUI spec §1.1), with the token index caught up or not, and beside a daemon that
//! answers or one that never does. Timings mean something
//! only in an optimized build on an idle machine, so this file exists only in release builds
//! and its tests are ignored by default. The TUI's opens a pseudo-terminal, so run them
//! outside a sandbox:
//! `cargo test --release -p tagteam --features test-support --test perf -- --ignored`
```

In `crates/tagteam/tests/perf.rs`, replace:

```rust

mod common;

use std::fs::File;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use common::pty::{Pty, Screen};
use common::{
    Guarded, Turn, bloat_claude_json, cc_profile, hold_launch, now_epoch_s, record_history,
    short_tempdir, std_cmd, turn, two_fresh_accounts, usage_window, write_transcript,
};
use tagteam_core::{Window, WindowKind};
use tagteam_provider::Env;
use tagteam_provider::mock_server::MockServer;

const RUNS: usize = 50;
```

with:

```rust

mod common;

use std::fs::File;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use common::pty::{Pty, Screen};
use common::{
    Guarded, Turn, bloat_claude_json, cc_profile, hold_launch, now_epoch_s, record_history,
    short_tempdir, std_cmd, turn, two_fresh_accounts, usage_window, write_transcript,
};
use tagteam_core::{Window, WindowKind};
use tagteam_engine::daemon::daemon_socket_path;
use tagteam_provider::Env;
use tagteam_provider::mock_server::MockServer;

const RUNS: usize = 50;
```

In `crates/tagteam/tests/perf.rs`, replace:

```rust
        p <= Duration::from_millis(50),
        "list p95 while a daemon runs {p:?} over {RUNS} runs: {runs:?}"
    );
}
```

with:

```rust
        p <= Duration::from_millis(50),
        "list p95 while a daemon runs {p:?} over {RUNS} runs: {runs:?}"
    );
}

/// The TUI's first frame, from the spawn to the arrival of the bytes that first put the
/// accounts table on the screen, through a pty, over `RUNS` runs after `WARM_UP` untimed ones,
/// with every endpoint pointed at `base`.
fn first_frames(root: &Path, base: &str) -> Vec<Duration> {
    let shown = |s: &Screen| s.on_alt() && s.contains("a@x.co");
    let run = || {
        let mut c = std_cmd(root);
        c.env("TAGTEAM_TEST_API_BASE", base).arg("tui");
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
    (0..RUNS).map(|_| run()).collect()
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn tui_first_frame_p95_is_within_50_ms_while_a_daemon_runs() {
    // TUI spec §1.1 criterion 2, §8: the live thread connects and subscribes beside the first
    // frame, which never waits for it; the TUI goes live after it.
    let _one_at_a_time = serial();
    let d = short_tempdir();
    nothing_due(d.path());
    let server = MockServer::start();
    let daemon = start_daemon(d.path(), &server.base_url());
    let runs = first_frames(d.path(), &server.base_url());
    stop(daemon);
    let p = p95(&runs);
    eprintln!("tui first frame while a daemon runs p95 {p:?} over {RUNS} runs");
    assert_eq!(server.requests().len(), 0, "a fetch was due");
    assert!(
        p <= Duration::from_millis(50),
        "tui first frame while a daemon runs p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn tui_first_frame_p95_is_within_50_ms_beside_a_daemon_that_never_answers() {
    // TUI spec §1.1 criteria 2 and 3: a socket that accepts and never greets holds the live
    // thread for the 250 ms greeting bound (D5.7), never the first frame.
    let _one_at_a_time = serial();
    let d = short_tempdir();
    nothing_due(d.path());
    let listener = UnixListener::bind(daemon_socket_path(&Env::for_test(d.path()))).unwrap();
    thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => held.push(stream),
                Err(_) => return,
            }
        }
    });
    let server = MockServer::start();
    let runs = first_frames(d.path(), &server.base_url());
    let p = p95(&runs);
    eprintln!("tui first frame beside a silent daemon p95 {p:?} over {RUNS} runs");
    assert_eq!(server.requests().len(), 0, "a fetch was due");
    assert!(
        p <= Duration::from_millis(50),
        "tui first frame beside a silent daemon p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}
```

In `crates/tagteam/tests/tui_pty.rs`, replace:

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
    FakeCall, cmd, fake_claude, fake_claude_calls, live_email, login, now_epoch_s, path_with,
    seed_home, std_cmd, turn, two_fresh_accounts, write_transcript,
};
use tagteam_core::AccountId;
use tagteam_provider::{Env, FileKeychain, MARKER_FILE, profile_path};

```

with:

```rust
//! The TUI through the real binary on a pseudo-terminal (TUI spec §11 "End to end"): what it
//! shows, how it leaves the terminal, how it treats signals with and without a foreground
//! session, and how it follows a real daemon. `openpty` fails inside a sandbox that denies it,
//! and so does the daemon's socket, so run these outside one:
//! `cargo test -p tagteam --features test-support --test tui_pty`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use common::pty::{Finished, Pty, Screen};
use common::{
    FakeCall, Guarded, cmd, fake_claude, fake_claude_calls, live_email, login, now_epoch_s,
    path_with, seed_home, short_tempdir, std_cmd, turn, two_fresh_accounts, write_transcript,
};
use tagteam_core::AccountId;
use tagteam_provider::{Env, FileKeychain, MARKER_FILE, profile_path};

```

In `crates/tagteam/tests/tui_pty.rs`, replace:

```rust
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

```

with:

```rust
}

impl Home {
    fn new() -> Home {
        Home::in_dir(tempfile::tempdir().unwrap())
    }

    /// `new`, in a directory short enough for the daemon's socket (D5.1), with the daemon's
    /// notifications off, so that it starts no `osascript`.
    fn for_daemon() -> Home {
        let home = Home::in_dir(short_tempdir());
        cmd(home.root())
            .args(["config", "set", "notify.enabled", "false"])
            .assert()
            .success();
        home
    }

    fn in_dir(dir: tempfile::TempDir) -> Home {
        let (a, _b) = two_fresh_accounts(dir.path());
        fake_claude(dir.path());
        fs::create_dir_all(dir.path().join("work")).unwrap();
        Home { dir, a }
    }

    /// `tagteam daemon --json` on this home, as a service runs it (stdin closed), once it
    /// switches Claude Code's two accounts (`auto`). It is killed if the test fails.
    fn daemon(&self) -> Guarded {
        let mut c = std_cmd(self.root());
        c.args(["daemon", "--json"]).stdin(Stdio::null());
        let mut daemon = Guarded::spawn(c);
        daemon.wait_for(LONG, "the daemon in auto mode", |out| {
            out.contains(r#""event":"mode""#) && out.contains(r#""mode":"auto""#)
        });
        daemon
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

```

In `crates/tagteam/tests/tui_pty.rs`, replace:

```rust
        && s.contains("refreshed ")
        && s.contains("1.1K")
}

/// Waits for the session `R` started to be running.
fn wait_for_session(home: &Home, tui: &mut Pty) {
    tui.wait_until(LONG, "the session", || {
        home.session().is_some_and(|c| c.ready)
```

with:

```rust
        && s.contains("refreshed ")
        && s.contains("1.1K")
}

/// The accounts table, with the TUI following the daemon, which switches automatically.
fn live(s: &Screen) -> bool {
    accounts(s) && s.contains("● live · auto on")
}

/// SIGTERM, and the daemon's exit 0 (D4.6).
fn stop(daemon: Guarded) {
    daemon.signal(libc::SIGTERM);
    let out = daemon.finish(LONG);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Waits for the session `R` started to be running.
fn wait_for_session(home: &Home, tui: &mut Pty) {
    tui.wait_until(LONG, "the session", || {
        home.session().is_some_and(|c| c.ready)
```

In `crates/tagteam/tests/tui_pty.rs`, replace:

```rust
        "{}",
        f.screen.text()
    );
}
```

with:

```rust
        "{}",
        f.screen.text()
    );
}

#[test]
fn under_r_ctrl_c_ends_the_session_and_the_daemon_connection_stays_live() {
    // Spec §7.3, §11: the live thread watches no token, so the session's Ctrl-C leaves the
    // daemon connection as it was.
    let home = Home::for_daemon();
    let daemon = home.daemon();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "live mode", live);
    tui.type_keys(b"R");
    wait_for_session(&home, &mut tui);
    tui.type_keys(b"\x03");
    tui.wait_for(LONG, "the wait for Enter", returning);
    tui.type_keys(b"\r");
    tui.wait_for(LONG, "the accounts table, still live", live);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_restored(&f);
    stop(daemon);
}

#[test]
fn p_pauses_the_daemon_s_auto_switch_and_resumes_it() {
    // Spec §7.4: through `config set`; the header turns as the daemon's `status` does.
    let home = Home::for_daemon();
    let daemon = home.daemon();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "live mode", live);
    tui.type_keys(b"P");
    tui.wait_for(LONG, "the question", |s| {
        s.contains("Pause auto-switch for claude-code?")
    });
    tui.type_keys(b"y");
    tui.wait_for(LONG, "the pause", |s| s.contains("● live · auto paused"));
    let config = fs::read_to_string(home.root().join("home/.config/tagteam/config.toml")).unwrap();
    assert!(config.contains("autoswitch = false"), "{config}");
    tui.type_keys(b"P");
    tui.wait_for(LONG, "the question", |s| {
        s.contains("Resume auto-switch for claude-code?")
    });
    tui.type_keys(b"y");
    tui.wait_for(LONG, "auto-switch again", live);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    stop(daemon);
}

#[test]
fn a_daemon_that_stops_leaves_the_tui_on_its_timer_until_the_next_one_answers() {
    // Spec §8: `stopping` names its reason; the TUI refreshes on its own timer, and reconnects
    // with backoff, reading every view again.
    let home = Home::for_daemon();
    let daemon = home.daemon();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "live mode", live);
    stop(daemon);
    tui.wait_for(LONG, "timer mode", |s| {
        accounts(s) && s.contains("○ daemon stopping (signal)")
    });
    let daemon = home.daemon();
    tui.wait_for(LONG, "live mode again", live);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    stop(daemon);
}
```

- [ ] **Step 2: Run them**

These tests pin what Tasks 1–5 built, end to end, so they pass as written; Step 3 is the docs.

Run, outside the sandbox: `cargo test -p tagteam --features test-support --test tui_pty`

Expected: `22 passed` (3 new).

- [ ] **Step 3: Write the docs**

In `CHANGELOG.md`, replace:

```markdown
  month, session or 5-hour block, grouped and filtered by account and model, with drill-down
  from a period to its sessions and from a session to its models, and the active block's remaining
  time, burn rate and projection. It brings the token index up to date in the background, and reads the
  report again as the index, the accounts or the clock move.

### Changed

- `tagteam purge` waits for a running token index refresh before it deletes a provider's data,
```

with:

```markdown
  month, session or 5-hour block, grouped and filtered by account and model, with drill-down
  from a period to its sessions and from a session to its models, and the active block's remaining
  time, burn rate and projection. It brings the token index up to date in the background, and reads the
  report again as the index, the accounts or the clock move.
- With the daemon running, the TUI follows it live. The header shows what the daemon does for
  the selected account's provider (`● live · auto on`, `auto paused`, …), the line above the
  footer shows its latest event, and the TUI reads its views again as the daemon's events come
  in. It leaves collecting usage, and keeping the token index current, to the daemon. `P`
  pauses or resumes the daemon's auto-switching for that provider, as
  `tagteam config set provider.<id>.daemon.autoswitch` does. Without a daemon, or once it
  stops, the TUI refreshes on its own timer, and it connects again when a daemon is back.

### Changed

- `tagteam purge` waits for a running token index refresh before it deletes a provider's data,
```

In `README.md`, replace:

````markdown
tagteam config set daemon.autoswitch false   # pause
tagteam config set daemon.autoswitch true    # resume
```

While the daemon switches a provider, `tagteam auto` refuses to run for it and says so. Pause
the daemon first if you want `auto` in a terminal.

`tagteam daemon status` shows whether the daemon runs, what it is doing for each provider, and
````

with:

````markdown
tagteam config set daemon.autoswitch false   # pause
tagteam config set daemon.autoswitch true    # resume
```

In the TUI, `P` pauses or resumes it for the selected account's provider, and the header says
`auto paused` or `auto on`.

While the daemon switches a provider, `tagteam auto` refuses to run for it and says so. Pause
the daemon first if you want `auto` in a terminal.

`tagteam daemon status` shows whether the daemon runs, what it is doing for each provider, and
````

- [ ] **Step 4: Run the tests and see them pass**

Run:
- Outside the sandbox: `cargo test -p tagteam --features test-support --test tui_pty` — Expected: `22 passed`.
- Outside the sandbox, on an idle machine: `cargo test --release -p tagteam --features test-support --test perf -- --ignored --nocapture tui_first_frame` — Expected: `4 passed`, each printing its p95 within 50 ms.
- The workspace suite: `cargo test --workspace --features tagteam/test-support --no-fail-fast` — Expected: 3792 of 3792 pass: 3689 in the sandbox, and outside it the 103 it refuses (`tagteam`'s library 10, `tests/daemon_cli.rs` 4, `tests/daemon_procs.rs` 5, `tests/signals.rs` 1, `tests/tui_pty.rs` 21, `tagteam-daemon`'s `client` 14, `server` 17 and `umask` 1, the engine's `tests/doctor.rs` 2, `xtask`'s compat tests 14, `tagteam-tui`'s `tests/live.rs` 13 and its library 1).

- [ ] **Step 5: Format and lint**

Run:
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`

Expected: no output from the first; `Finished` and no warnings from the others.

- [ ] **Step 6: Commit**

```bash
git add CHANGELOG.md README.md crates/tagteam/tests/perf.rs crates/tagteam/tests/tui_pty.rs
git commit -m "Test the TUI live beside a real daemon end to end, and time its first frame beside one"
```
