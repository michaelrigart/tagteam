# tagteam — The TUI switches accounts while it is open

**Status:** Approved
**Date:** 2026-11-11
**Scope:**
- The TUI (`crates/tagteam-tui`) runs auto-switching itself while it is open and no daemon runs,
  with the daemon's own provider jobs (`crates/tagteam-daemon/src/provider_job.rs`) on a TUI
  thread.
- The header, the `P` key, the TUI's timer refresh and the engine lock's holder record follow
  from that.
- The daemon, `tagteam auto`, the decision and every CLI command keep their behaviour, except
  for the words that name a lock holder.

**Builds on:**
- **TUI spec:** [`2026-10-14-tagteam-tui-design.md`](2026-10-14-tagteam-tui-design.md), cited as
  "TUI §n".
- **Redesign spec:** [`2026-10-20-tagteam-tui-redesign-design.md`](2026-10-20-tagteam-tui-redesign-design.md),
  cited as "redesign §n".
- **Daemon spec:** [`2026-10-14-tagteam-daemon-design.md`](2026-10-14-tagteam-daemon-design.md),
  cited as "daemon Dn".
- **Core spec:** [`2026-09-26-tagteam-core-cli-design.md`](2026-09-26-tagteam-core-cli-design.md),
  cited as "core §n".

Line numbers are as of `688f8492` and will drift.

---

## 1. Purpose

### 1.1 Why

Auto-switching runs only in the daemon or in a foreground `tagteam auto`. A user who keeps the
TUI open all day already has a tagteam process running, but without a daemon the header says
`○ auto off · no daemon` and nothing switches. Michael asked on 2026-11-10 that the open TUI do
the switching. He chose the defaults on 2026-11-11: it is on by default whenever no daemon runs,
and it is paused with `P` through the existing `daemon.autoswitch` setting.

### 1.2 What exists

- **One loop, two hosts.** `tagteam auto` and the daemon both drive `AutoEngine::tick` on the
  daemon crate's `Scheduler`. The daemon's `ProviderJob` adds D4.3's modes:
  - `auto` while it holds the engine lock;
  - `collect` while paused or with fewer than two usable accounts;
  - `waiting` while another process holds the lock, re-evaluated every 60 s or less;
  - `idle` with no usable account.
  (`provider_job.rs:80-94`, `:288-317`, `:442-444`)
- **Pausing.** A reload that flips `daemon.autoswitch` drops the engine and its lock within
  about 1 s (`:699-735`). The flip is stored as `autoswitch-paused` or `autoswitch-resumed`.
- **One engine per provider.** The engine lock is only ever tried (core §11.1). A daemon job
  that finds it held waits in `waiting`/`engine-lock` and takes it once it is free.
- **The TUI** already depends on `tagteam-daemon` (`crates/tagteam-tui/Cargo.toml:14`), whose
  scheduler, provider job, events and status board are public (`crates/tagteam-daemon/src/lib.rs`).
  The header's states come from a daemon `status` (`daemon/words.rs:19-58`, `:71-130`). `P`
  works only with a live daemon (`actions.rs:146-177`).
- **`ProviderJobCtx`** needs a status board, an optional notice queue, the Keychain read counts
  and the schema flag (`provider_job.rs:46-56`). The TUI can pass no notice queue.

### 1.3 Why this design

Hosting the daemon's provider jobs, rather than `tagteam auto`'s `AutoJob`, brings pause,
waiting for the lock, `collect` mode, stored pause events and the status board for free. The TUI
already has a renderer for the board's states: the daemon header. A TUI-only loop would be a
third copy of the suspend, cancel and pause rules. The daemon spec rejected a second copy for
that reason (daemon spec, lines 63-67).

## 2. Goals

- **G1.** With no daemon, an open TUI auto-switches every provider whose engine lock it can
  take, exactly as the daemon would: the same decision, cadence, budget and stored events.
- **G2.** The daemon wins. When a daemon comes up, the TUI stops its jobs and releases the
  locks, and the daemon takes over within its 60 s re-evaluation.
- **G3.** `P` pauses and resumes the TUI's auto-switching as it does the daemon's, through
  `provider.<id>.daemon.autoswitch`.
- **G4.** Nothing is polled twice. The timer refresh skips the providers the TUI's own jobs
  collect for.
- **G5.** The header and every lock-holder message say truthfully where auto runs.

### Non-goals

- Notifications. They stay the daemon's (TUI §1.2).
- The token report's index jobs, the upgrade watch and the socket. The TUI serves no `status`
  to anyone.
- Switching while the TUI is closed or stopped with Ctrl-Z. That is what the daemon is for. A
  hand-over to a foreground action, such as an `R` session, is not a stop: auto keeps running
  through it (§3.1).
- Yielding to a foreground `tagteam auto`. `auto` tries the lock once. While a TUI holds it,
  `auto` refuses, and the refusal says how to free it (§3.6).
- A new setting. `daemon.autoswitch` keeps its name. The README and the setting's docs say it
  covers the TUI.

### Anti-goals

- Two engines driving one provider. The engine lock stays the only arbiter.
- A TUI whose quit hangs on auto. Quit waits only for a critical span in flight (core §14.1).
- Churn when the daemon restarts for an upgrade (exit 75, restarted by launchd or systemd). The
  TUI must not grab the locks in the seconds the daemon is away (§3.2).

## 3. Design

### 3.1 The auto lane

- **The lane:** a new TUI lane thread, `auto`.
  - It gets its `Engine` from a factory the binary supplies, as the other lanes do. The TUI
    crate builds no engine of its own (`crates/tagteam/src/tui_cmd.rs:140-147`).
  - The engine is built inside the thread with `EventSource::Auto`, so History words its
    switches as auto's. It also gets the shutdown token and the counted Keychain
    (`app/mod.rs:158-163`).
- **The jobs:** it runs a `Scheduler` with the daemon's own jobs, in the daemon's order
  (`daemon.rs:290-334`):
  - the settings job, so engine-level settings reload as in the daemon;
  - the store watch, so a store change re-evaluates every job's mode;
  - the schema half of the upgrade watch (§3.2);
  - one `ProviderJob` per provider. Its context is a status board of the TUI's own, no notice
    queue, the Keychain read counts and the schema flag.
- **Not hosted:** the program half of the upgrade watch (the TUI is not restarted for an
  upgrade), and the token jobs.
- **Joining:** the lane is joined at shutdown, unlike the daemon lane, because it writes the
  store. On quit the shutdown token stops the scheduler within one 1 s slice. A tick finishes
  only its critical span (core §14.1).
- **Hand-overs keep it running.** A foreground action, such as an `R` session, runs on the UI
  thread for as long as it lasts (`runtime.rs:578-583`). The lane carries on through it, as
  every background lane does (TUI §7.3). This is when switching matters most. Its start and
  stop rule (§3.2) is judged inside the lane, so it works while the UI thread waits.
  *(My call, from the builder check.)*
- **Ctrl-Z releases the locks.** Before the TUI stops its process (`RealTty::stop`,
  `term.rs:256-261`), it stops the lane and joins it. A stopped process keeps its locks and
  reads as live, which would block a daemon or `tagteam auto` for as long as it stays stopped.
  On resume, the lane starts again by §3.2's rule. *(My call, from the builder check.)*
- **Inside a `tagteam run` shell** the engine refuses (`engine/src/auto.rs:362-374`). The lane
  then starts nothing, and the header stays as it is today.

### 3.2 When the lane runs

The lane judges "a daemon runs" by the daemon's lock record, `daemon.lock`. It is live when its
pid and start time match a running process, the same check `foreign_engine_holders` already
makes (`source.rs:222-225`). The socket link is not the signal:
- a daemon whose socket failed to bind still runs and still wants the locks
  (`crates/tagteam/src/daemon_cmd.rs:54-68`);
- a daemon whose protocol this TUI does not speak still holds the locks it can.

The lane checks on every scheduler pass, so the check needs no UI thread. A pass runs between
jobs, at most 1 s apart while the scheduler sleeps. A job already running (a tick, or a
collection waiting on a request) finishes first, within its requests' own timeouts.

| Lane sees | Jobs |
|---|---|
| A live daemon record | none: provider jobs dropped, engine locks released. The settings and schema jobs keep running |
| No live daemon record, at start | started |
| No live daemon record for 30 s, after one was seen | started |
| The store's schema newer than this build's | none, until the TUI restarts, and the whole TUI stops writing (§3.7) |

- **Hand-over bound.** A daemon that comes up gets the locks once the job in flight returns, at
  the next pass, and takes them at its own next re-evaluation (60 s or less). Nothing switches
  twice in between, because the engine lock is held until the job returns.
- The 30 s grace covers an upgrade restart. A daemon that comes back within it finds its locks
  free.
- Stopping and starting the provider jobs is the lane's whole life. It has no pause state of its
  own, since the jobs read `daemon.autoswitch` themselves.
- *(My call, from the builder check.)*

### 3.3 What the TUI shows

- **Hosted status:** while the lane runs, the TUI treats its board as it treats a live daemon's
  `status`. The header renders through `job_state` (`daemon/words.rs:71-130`), and `P` works.
  The board's lines are turned into the same `StatusProvider` values the socket gives. How
  (a direct conversion, or the protocol's own encoding) is the builder's choice; the header code
  must not fork.
- **Header words.** The states read as the daemon's (redesign §5.2's table), except:

  | State | ≥ 120 columns | 80 columns |
  |---|---|---|
  | hosted `auto` | `● auto on in this window · switches at 90%` | `● auto on · at 90%` |
  | hosted `waiting` for `engine-lock`, holder a TUI | `● auto runs in another tagteam window (pid N)` | `● auto in pid N` |
  | no daemon, holder a TUI (C-16's case, lane not started) | as above | as above |

  `consume-first` keeps C-17's words after "in this window". `paused`, `idle` and the keychain
  backoff read as the daemon's. One more row:

  | State | ≥ 120 columns | 80 columns |
  |---|---|---|
  | lane stopped for a newer store schema | `! auto stopped · tagteam was upgraded; restart the TUI` | `! restart the TUI` |
  | a live daemon record, no socket link, the provider's engine lock held by the daemon (§3.6) | `● auto runs in the daemon (pid N) · its socket is down` | `● auto in daemon` |
  | the same, the lock held by another process | that holder's row, as above or C-16's | as above |
  | the same, the lock not held (the daemon paused, idle or waiting) | `○ the daemon runs (pid N) · its socket is down, so its state is unknown` | `○ daemon state unknown` |

  With a live daemon and a link, the header is the daemon's, as today. The lane has no jobs
  then. The last three rows replace today's false `○ auto off · no daemon` for a daemon that
  runs without its socket: `foreign_engine_holders` leaves the daemon's pid out, so nothing
  showed it. Only the lock's holder record says where switching runs. A live daemon record
  alone does not.
- **Live events:** the lane's `DaemonEvent`s reach the auto line and History's re-reads exactly
  as the socket's do (`daemon/mod.rs:281-374`): the last `poll`, the latest decision, a re-read
  after a switch. The builder routes them through the existing message channel.
- **Own pid:** `foreign_engine_holders` (`source.rs:218-238`) leaves out the TUI's own pid as it
  leaves out the daemon's.

### 3.4 `P`

`P` asks and toggles exactly as with a daemon (`actions.rs:146-177`). It writes
`provider.<id>.daemon.autoswitch` through `config set`. The lane's scheduler sees the mtime
change, and the job drops to `collect` and records `autoswitch-paused`.

- **The toast for "no daemon runs"** now appears only when the lane cannot run either: inside a
  run shell, or after a newer store schema.
- **A daemon without its socket:** `P` changes nothing and says "the daemon's socket is down, so
  the TUI can't show its state; `tagteam daemon status` says more". The TUI changes no setting
  whose effect it cannot show (T2-16).
- **A protocol mismatch:** `P` behaves as today.

### 3.5 The timer refresh

The 30 s refresh and the first-frame collection (`update.rs:180-198`, `:316-327`) skip each
provider whose hosted job `collects_for_itself`, with the same check live mode uses
(`daemon/mod.rs:583-605`). `r` still collects on demand, as with a daemon (TUI §1.1 goal 4).

### 3.6 Naming the lock holder

The engine lock's holder record (`engine/src/auto.rs:274-283`) gains who holds it: `daemon`,
`auto` or `tui`. The field is additive, and a record without it reads as today.

- The TUI's header uses it (§3.3).
- `tagteam auto`'s refusal (`commands/daemon.rs:65-90`) names the holder. For a TUI: "auto-switch
  already runs for claude-code (a tagteam window, pid N; press P there to pause it, or quit it)".
- `doctor`, wherever it names a holder, does the same.

### 3.7 A newer store schema stops every TUI write

Once the lane's schema check finds the store newer than this build, the TUI as a whole stops
writing. Today's TUI has this exposure already: an engine's cached connection is never
schema-checked again (`engine.rs:288-303`). The check now exists, so the TUI uses it.
- The flag is set once and never cleared.
- The timer refresh and the first-frame collection stop.
- `r` and every action that runs a command which writes the store refuse with the header's words
  ("tagteam was upgraded; restart the TUI"), before they start.
- Reads go on, so the screens stay as they were.
- **Work in flight when the flag is set finishes, and may write once.** That is exactly the
  guarantee the daemon's upgrade watch gives (daemon D4.5): detection stops new work. Nothing
  checks the schema at the moment of a write. A cached connection, in the daemon, `tagteam auto`
  or the TUI, writes what it was already writing.
- **Out of scope here:** enforcing "never write a newer store" at each write (a schema check
  inside every write transaction, in the store). That would cover every writer at once, and it
  is a separate change from this one. *(My call, from the spec review; whether to make that
  change is Michael's.)*

## 4. Tests

- **Lane life (engine-level tests with the fake provider):**
  - no daemon: the lane takes the lock, ticks, and a switch is stored with source `auto`;
  - a live daemon record appears, with or without a socket: the locks are free at the next pass,
    including when it appears while a usage request is held open (the lock goes once the
    request times out and the job returns);
  - the record goes dead: the jobs start again after 30 s, and not before;
  - the store's schema becomes newer: the jobs stop and stay stopped, the timer refresh stops,
    and `r` and a store-writing action refuse (§3.7);
  - a settings change reaches the lane's engine;
  - quit releases the lock (the lane is joined);
  - Ctrl-Z releases the lock before the process stops, and resume takes it again;
  - inside a run shell: no lane.
- **Hand-over:** during a foreground action the lane keeps ticking, and it still stops for a
  daemon that comes up meanwhile.
- **Two TUIs:** the second's job waits on `engine-lock`, and its header names "another tagteam
  window".
- **`P`:** with the lane running it writes the setting, the job enters `collect`, and
  `autoswitch-paused` is stored. Resume reverses it.
- **Refresh:** the timer skips a hosted provider in `auto`, and collects one in `idle`.
- **Header:** every row of §3.3, at both widths. That includes a live daemon record with no
  socket, with the lock held by the daemon, held by a foreground `auto`, and not held (paused).
  `P` in those states changes nothing.
- **Holder record:** a record without the field still reads, and `auto`'s refusal names a TUI
  holder.
- **The daemon is unchanged:** its suites pass as they are. Its job against a TUI-held lock
  waits, then takes the lock when the TUI quits. That is an existing test, kept.
- **PTY:** one end-to-end run in `tui_pty`, where the TUI switches an account at its limit with
  no daemon. The binary registers only Claude Code (`crates/tagteam/src/app/context.rs:179-189`),
  so this runs Claude Code with the fake `claude` and the mock usage server, as the daemon's PTY
  test does.

## 5. Records

- **TUI spec:**
  - §1.1 goal 4 ("never fetches on its own schedule") and goal 6;
  - §1.2's pause non-goal;
  - the threads section, for the new lane and its joining;
  - §7.3, for the lane through a hand-over and its stop around Ctrl-Z;
  - §7.4 `P`.
  Each passage gets "*(Amended by the TUI autoswitch spec.)*".
- **Redesign spec:** §5.2's header table, C-16, and `P`'s row (line 835).
- **Daemon spec:** where it says auto and the daemon never both drive one provider (lines
  32-33), add the TUI. D7's `daemon.autoswitch` covers the TUI too.
- **Core spec:** §11.1's one-engine rule lists the TUI as a possible holder. The §6.4 note on
  `daemon.autoswitch` gets the same addition.
- **README:**
  - "Automatic switching" and "Daemon": the open TUI switches when no daemon runs; the daemon
    covers the hours it is closed; `P` and `daemon.autoswitch` pause both; `tagteam auto` is
    refused while a TUI switches.
- **CHANGELOG.**

## 6. Left to the builder

- How the board's lines become `StatusProvider` values (§3.3), and how lane events reach the
  UI thread.
- The lane's start and stop mechanics inside its scheduler, and where the 30 s grace is timed.
- How the engine factory reaches the lane.
- The holder field's name and spelling (§3.6).
- Exact header words within §3.3's meaning, if a width check forces a change.
- Test fixtures.
