# tagteam — sub-project 2: the daemon

**Status:** In progress
**Date:** 2026-10-14
**Scope:** Sub-project 2 of 4. It builds on sub-project 1, whose spec,
[`2026-09-26-tagteam-core-cli-design.md`](2026-09-26-tagteam-core-cli-design.md) (the "core
spec"), stays the authority for everything that exists. A reference such as "§8.6" is to the
core spec; "D3" is to section 3 of this one. The token report and the TUI (sub-project 3) run
in parallel workstreams; D13 says what this spec leaves for them.

---

## D1. Purpose

`tagteam daemon` is a long-running process that schedules what the engine already does. It runs
the usage collector on the poll policy (§8.6) and the auto-switch loop (§11). It also tells the
user about the events that matter, and serves a local socket that streams those events and
answers status requests. It adds no business logic: every decision is still the engine's.

A user installs it once (`tagteam daemon install`). From then on readings stay fresh, so
`statusline` and `list` stop showing stale data and seldom have to fetch. Switches happen
unattended, and the user hears about the ones that matter. The TUI (sub-project 3) gets live
updates. The CLI keeps working, unchanged, without the daemon.

### D1.1 Success criteria

1. While the daemon runs, the accounts §8.6's scheduled collection covers (the managed live
   account and the switchable candidates) are refreshed on its schedule without any command
   being run, whether or not the live login is managed. The per-identity budget of §8.6 still
   holds across all tagteam processes. Disabled and session-owned accounts are left to `list`'s
   on-demand collection, as today (§8.3).
2. Auto-switching behaves exactly as `tagteam auto` does (§11), and the two never drive one
   provider at once.
3. `statusline` stays within 10 ms p95, and `list` within 50 ms p95 when no fetch is due, while a
   daemon runs (§1.1 criterion 5).
4. `doctor` still tells which process holds an engine lock (§11.1, §13.6), and also reports the
   daemon and its service unit.
5. A client finds the socket from its environment alone, and a protocol mismatch is detected in
   the first exchange.
6. The daemon is still part of the one `tagteam` binary (§1.1 criterion 6). The only new runtime
   programs it calls are macOS's own `/usr/bin/osascript` and `launchctl`, and on Linux
   `systemctl`, `loginctl` and `notify-send`. Each is optional: without it, only the feature
   that uses it is missing.

### D1.2 Non-goals

- Windows, as in sub-project 1.
- An async runtime. The daemon uses threads (D4).
- Access from another machine or another user: no TCP, and no socket shared between users.
- Control over the socket. Protocol 1 is read-only; pausing auto-switch is a setting (D7).
- The token report's index jobs and the TUI themselves (D13).
- Free readings from the statusline's `rate_limits` (§17 O2).
- More than one daemon per data directory.
- A `daemon watch` command. Scripts use `tagteam auto --json`, or the socket.

## D2. Approach

The `auto` loop (`crates/tagteam/src/auto.rs`: wall-clock deadlines, cancellable 1 s slices,
settings reload) moves into the new crate as a **job scheduler**. `tagteam auto` becomes that
scheduler with only auto jobs and the CLI's sink. Its behaviour, output and exit codes do not
change, and its tests move with it. The daemon is the same scheduler with more jobs.

Two alternatives were rejected:
- **A second loop just for the daemon.** It churns less now, but leaves two loops whose suspend
  and cancellation semantics (§11.4, §14.1) must be kept in step by hand.
- **A daemon that spawns `tagteam auto --json` and relays its output.** It gives no status, no
  collect-only mode, and a second process per daemon.

## D3. Crates and engine changes

**`tagteam-daemon`** is a new library crate on top of `tagteam-engine` (§4.1). It holds the
scheduler, the provider jobs, the socket server, the protocol types and their client, the
notifier, and the service units. The `tagteam` binary gains the `daemon` commands (D8) and runs
`auto` on the scheduler.

- **Publishing.** The crate joins the crate loop in `.github/workflows/publish-crates.yml`
  between `tagteam-engine` and `tagteam`. With the TUI's crate the order is `tagteam-core`,
  `tagteam-provider`, `tagteam-cc`, `tagteam-engine`, `tagteam-daemon`, `tagteam-tui`,
  `tagteam`. Its first crates.io publish needs a short-lived token (CONTRIBUTING "Releasing"),
  so Michael is told before that release.
- **Dependency rule.** Like the engine, the crate names no provider's paths, keys or endpoints.
  It reaches providers through the engine only.

**Code that moves.**
- From `crates/tagteam/src/auto.rs` into the crate's scheduler: the loop driver, its slots, the
  `Sleeper` port, the jitter draw, `AutoFlags`, `auto_config`, the provider selection, `--once`,
  and the settings reload. The human sink stays in the CLI.
- `event_json` moves into the crate's protocol module, so `auto --json` and the socket write the
  same objects from one function.

**Engine and port changes.** None adds a decision or a table.
1. **Settings reload.** `Engine` keeps its settings behind a lock, and
   `Engine::reload_settings()` re-reads `config.toml` and returns its warnings (§6.4). A daemon
   lives for days, and the collector and the views read the engine's settings (`models`,
   `history_retention_days`). `settings()` returns a snapshot.
2. **Event source.** `Engine::auto` takes the source its switches record. `auto` passes `auto`,
   and the daemon passes `daemon`, which `events.source` already reserves (§6.1).
3. **Releasing an engine lock.** An `AutoEngine` that is dropped empties its engine lock file's
   holder record (§11.1) before the lock is released. A daemon that gives up a provider's lock
   (D4.3) stays alive, so a record left behind would name a live process as the holder, and
   `doctor` would report an engine that no longer runs. A process killed by SIGKILL still
   leaves its record, which names a dead process, as today.
4. **Store change detection.** `Engine::data_version()` returns SQLite's `PRAGMA data_version`
   for the engine's own connection. It changes when another connection commits, and never for
   this process's own writes.
5. **The live account's relevant windows.** A read-only view returns the provider's managed
   live account (its ID, position and label) with its decision-grade relevant windows under
   given `models` (§8.2, §8.4), each with its `pct` and `resets_at`, or nothing when the live
   login is unmanaged or has no such reading. It is the reading a tick's decision already uses,
   exposed for the `limit` notification (D6.1).
6. **Keychain read outcomes** (macOS). The `security` driver (Appendix A.3), which every Keychain
   read in the process goes through (the vault's, and the provider's reads of the live and
   profile credentials), counts its **secret reads** (`find-generic-password -w`) by outcome:
   *answered* (rc 0, a secret returned) and *locked* (rc 36, or a timeout). An absent item
   (rc 44) counts as neither, since a locked keychain can still report absence, and
   attribute-only existence probes are not counted at all. The counters are process-wide and
   read-only; nothing but the daemon's keychain backoff (D4.1) reads them.

There is **no store migration**. Notification dedup uses `leases` markers (D6.3), and
everything else the daemon keeps is in memory.

## D4. Process model

The daemon uses threads and blocking I/O, as sub-project 1 does (§4.4):

| Thread | Does |
|---|---|
| Main | The scheduler: the provider jobs and the watches, one at a time (D4.2–D4.5) |
| Accept | Accepts socket connections (D5) |
| One per connection | Reads requests and writes replies and events (D5) |
| Notifier | Delivers notifications from a queue (D6) |

The scheduler never waits on a socket client or on a notification.

### D4.1 Start-up and single instance

1. **Refusals.** The daemon refuses to run as root (§5) and inside a run shell (§12.8), as `auto`
   does, because it changes the live login.
2. **Single instance.** It tries `locks/daemon.lock` (§5 gains the path), a try-only `flock`
   like the engine locks (§11.1). Holding it, it writes its pid, start time and version into the
   file as `{"pid":N,"start":S,"version":"…"}`, the start time taken as for the
   `switch_journal` holder (§12.6). Like the engine lock's, the record is never used for
   exclusion; `doctor` reads it (D10).
   - If another process holds it, the daemon prints `a daemon already runs (pid N); waiting for
     it to stop` and tries again every 5 s. The wait is a cancellation point (§14.1).
   - A service-started daemon and a foreground one therefore never both run, and a service
     never restart-loops on a lock that is taken.
3. **The Keychain** (macOS).
   - **On a terminal,** the daemon runs Appendix A.3's lock check before any job, and offers to
     unlock a locked keychain, as `auto` does. A declined or failed unlock exits 1.
   - **Without a terminal** (a service), it never runs the lock check, since `show-keychain-info`
     opens a SecurityAgent dialog on a locked keychain in a GUI session and blocks on it (A.3).
     It proceeds as `list` does, and the tri-state reads refuse safely (A.3).
   - **A locked keychain while running** is noticed from the driver's read counters (D3 change
     6), compared before and after each provider run: a run in which some secret read ended
     *locked* and none was *answered*. This covers every read a run makes, whether it
     collects, freshens or switches. The job then backs off (D4.3): its next run is 60 s
     later, doubling with each such run to at most 15 min. A run with any answered secret read
     ends the backoff. A run whose secret reads were all absent, or that made none, changes
     nothing. Linux has no Keychain, so it never backs off for one.
4. **The store.** With no store yet, every provider job is `idle` until one appears. A store
   whose schema is newer than this build (§6.1) is not an exit: a service would restart the same
   binary into the same refusal. The daemon logs it at ERROR once, every provider job waits
   (`waitingFor` `schema`), and the upgrade watch (D4.5) exits 75 once the binary is replaced.
5. **The socket** is bound (D5.1), and the accept thread starts.
6. **The scheduler** starts.

### D4.2 The scheduler

The scheduler runs a list of jobs, each with a wall-clock due time. Each pass either runs the job
whose due time has passed (the earliest, the first on a tie) or sleeps a slice of at most 1 s
toward the earliest due time. This is §11.4's loop, generalized:
- The cancel token is checked before each job and in each slice (§14.1).
- The wall clock is read again after each slice, so a machine suspended past a due time runs
  the job as soon as it wakes.
- A due time is never further away than the delay it was set for, so a clock set back cannot
  stretch a wait.
- **Settings.** `config.toml`'s mtime is checked in every slice, not only before a tick. When it
  changed, the settings are reloaded (D3 change 1), each warning becomes a `config-warning`
  event, and every provider job re-evaluates its mode (D4.3) before the next job runs. A
  setting therefore takes effect within about a second, unless a job is running, in which case
  it takes effect when that job ends.

A job is:

A sketch; the exact signatures are settled in the plan:

```rust
pub trait Job {
    fn name(&self) -> &str;                       // "provider:claude-code", "store-watch", …
    fn due_ms(&self) -> i64;                      // wall clock, epoch ms
    fn run(&mut self, cx: &mut JobCx<'_>) -> JobResult;  // sets its own next due time
}
```

`JobCx` carries the engine, the event sink (D5.5), the notifier queue (D6) and the status
snapshot (D5.4). A job's `run` may block, for example on a tick's network requests; the
scheduler runs one job at a time. `tagteam auto` builds only auto jobs. The daemon builds one
provider job per registered provider plus the watches (D4.4, D4.5). The token report adds its
jobs here (D13).

### D4.3 Provider jobs and their modes

The daemon has one job per registered provider. Its mode is re-evaluated after each of its runs,
every 60 s at most, after any `changed` event (D4.4), and after any settings reload. The rows are
tried in order, and the first that applies wins:

| Mode | When | What the job does |
|---|---|---|
| `waiting` | The store's schema is newer than this build (`waitingFor` `schema`, D4.1) | Nothing; the upgrade watch ends it (D4.5) |
| `idle` | There is no store, or the provider has no switchable account (enabled, with an identity, as `auto` counts them, §11.1) | Nothing. Checked again every 60 s |
| `waiting` | Another live process is the provider's engine (`waitingFor` `engine-lock`) | Collects nothing: an engine elsewhere collects for itself. Checked again every 60 s |
| `auto` | `daemon.autoswitch` is true for the provider (D7), it has at least two switchable accounts, and the daemon holds its engine lock | Runs `AutoEngine::tick` on §11.4's delays, exactly as `auto` does. A tick that ends before its collection because the live login is unmanaged or absent (§11.2 step 2: `unmanaged-active-account`, `no-active-account`) is followed, in the same run, by `collect`'s round, so readings stay fresh while tagteam does not manage the live login |
| `collect` | Otherwise: `daemon.autoswitch` is false, or the provider has one switchable account | Runs `collect_usage(CollectMode::Scheduled {provider, threshold, models})` (§8.3, §8.6) with the provider's settings: the live account when due, then one stalest due candidate, escalating as a tick does. It holds no engine lock. Its delay is `interval × U(0.9, 1.1)`, shortened to the earliest `next_poll_at` of the provider's accounts and floored at 60 s |

- **Entering `auto`.** The job tries the engine lock once (§11.1). If another process holds it,
  the job is `waiting` with the holder's pid from its record, and tries again every 60 s. A
  `tagteam auto` that starts at the instant of such a retry, and loses the race, fails as two
  `auto`s racing do today.
- **Leaving `auto`.** The job drops its `AutoEngine`, which empties the lock record and
  releases the lock (D3 change 3). Pausing is this: `daemon.autoswitch` set to false moves the
  provider to `collect` within about a second (D4.2), and frees the lock for a `tagteam auto` in
  a terminal.
- **`collect` never tries the engine lock,** since a try at the wrong instant would make a
  starting `auto` fail. Instead it reads the lock's holder record and judges it as `doctor` does,
  by an exact pid and start-time match (§13.6). A live holder sends the job to `waiting`.
- **Quarantines in `collect`.** Each round compares the provider's accounts with the round
  before, as a tick's step 1 does (§11.2), and emits `account-quarantined` or
  `account-unquarantined` for each change.
- **Keychain backoff** (D4.1) is not a mode. It only moves a job's next run later. The mode is
  still evaluated as above, before the backoff applies, so a setting or an account change
  takes effect during a backoff exactly as at any other time. In particular, pausing
  (`daemon.autoswitch` false) drops the `AutoEngine` and its lock at once, and the job's
  delayed runs are then `collect` rounds. The backoff carries over a mode change, since the
  keychain is no less locked.
- **Mode changes,** and a backoff starting or ending, emit a `mode` event (D5.5) and are
  logged at INFO.
- **Errors.** A run that fails emits `error`, and the job keeps its normal cadence, as §11.4's
  loop does. An interruption ends the scheduler (D4.6).

### D4.4 Store watch

Every 2 s, the `store-watch` job reads `Engine::data_version()` (D3 change 4). When it changed, it
emits one `changed` event. Another process committed: a switch or `add` from the CLI, a `list`
that collected, a `tagteam auto` tick. The daemon's own writes never trigger it; they come as
their own events (`poll`, `switch`, `usage`). Several commits within one watch period give one
`changed`.

### D4.5 Upgrade watch

Every 60 s, the `upgrade-watch` job checks two things:
- **Was the binary replaced?** At start-up the daemon records the file its program path
  resolves to: device, inode, size and mtime. The program path is its `argv[0]` when that is
  absolute (a service unit passes it so, D9), otherwise the path `daemon install` would choose
  (D9.1). A path that now resolves to another file, or to the same file with another size or
  mtime, means the binary was replaced.
- **Did a newer tagteam migrate the store** past this build's schema version (§6.1) while the
  daemon ran? A store that was already newer at start-up waits for the binary instead (D4.1).

Either way the daemon emits `stopping` with reason `upgrade` or `schema`, stops as it does on a
signal (D4.6), and exits **75**. A service restarts it on the new binary (D9). In the foreground
it prints `tagteam was upgraded; start the daemon again` first. A path that resolves to nothing
(tagteam was uninstalled) also exits 75; the service's restart then fails, and `doctor` reports
the missing program (D10).

### D4.6 Signals and stopping

SIGINT, SIGTERM and SIGHUP record their number in the cancel token (§14.1), as for every
command. The scheduler stops at its next cancellation point, after any critical span in flight
has finished. A tick in progress therefore ends as an interrupted `auto` tick does (§11.4).
The accept and connection threads poll with 1 s timeouts and stop too. Every subscriber gets a
`stopping` event with reason `signal`. As that event is sent, the daemon removes its socket,
while it still holds `daemon.lock` and only if the file is still the socket this daemon bound;
open connections write `stopping`, then close. Then the daemon drops its engine locks
(emptying their records), and releases `daemon.lock`, emptying its record first. It exits 0,
as `auto`'s loop does.

## D5. The socket protocol

This section is the contract the TUI and any other client depend on.

### D5.1 Path and access

- **Path:** `$XDG_DATA_HOME/tagteam/daemon.sock`, next to the store, which in code is
  `env.data_dir().join("daemon.sock")`. It is derived from the environment alone, with no
  settings read and no daemon asked, so a client finds the daemon of the store it uses. A
  missing file (`ENOENT`) or a stale one (`ECONNREFUSED`) fails at once.
- **Mode.** The socket is created 0600, by setting the umask to `0177` around `bind`. That
  happens before any thread that creates a file or a process starts. The notifier thread
  exists by then but only waits on its queue, which nothing can fill before the daemon serves,
  so the umask change affects nothing else. The data
  directory is 0700 (§5). The socket is never chmod'ed afterwards.
- **Peer check.** The server closes, before its greeting, any connection whose peer uid
  (`getpeereid` on macOS, `SO_PEERCRED` on Linux) is not its own.
- **A leftover socket file** is removed before binding, because a daemon that holds
  `daemon.lock` knows no other daemon serves it.
- **Path too long.** A path longer than `sun_path` allows (104 bytes on macOS, 108 on Linux)
  cannot be bound. The daemon then runs without a socket, logs at ERROR, and `doctor` fails the
  check (D10). Any other bind failure is treated the same way: scheduling never depends on the
  socket.
- **Connection limit.** At most 16 connections. Beyond that, a connection is closed at once,
  before the greeting.
- **No secrets** ever cross the socket. Events name accounts by position and email, as
  `auto --json` does.

### D5.2 Framing and handshake

- **Framing.** JSON Lines: one UTF-8 JSON object per line, at most 64 KiB. A client line that is
  longer, or is not a JSON object, closes the connection.
- **Greeting.** The server speaks first:
  `{"schemaVersion":1,"event":"hello","protocol":1,"version":"20261014.2045.0","pid":4242,"startedAt":"2026-10-14T20:45:00Z"}`.
  `version` is the daemon's tagteam version, and `startedAt` is when the daemon started.
- **Idle connections.** A connection that has not subscribed and sends nothing for 30 s is
  closed. A subscribed connection stays open.

### D5.3 Requests and replies

- **Request:** `{"id":<string or integer>,"op":"status"|"subscribe"}`. Unknown fields are ignored.
- **Success:** `{"schemaVersion":1,"id":<the request's>,"ok":true,"result":{…}}`.
- **Failure:** `{"schemaVersion":1,"id":<the request's, or null>,"ok":false,"error":{"type":"unknown-op"|"bad-request","message":"…"}}`.
  The connection stays open.
- **Ordering.** A client may send requests at any time, subscribed or not. Replies carry `id`
  and events carry `event`, so they never collide. Each is written whole, in the order it was
  produced.

### D5.4 `status`

```json
{ "daemon": { "pid": 4242, "version": "20261014.2045.0", "protocol": 1,
              "startedAt": "…Z", "socket": "~/.local/share/tagteam/daemon.sock",
              "log": "~/.local/state/tagteam/tagteam.log" },
  "notifications": { "enabled": true, "backend": "osascript", "events": ["switch", "quarantine", "limit"] },
  "providers": [ { "provider": "claude-code", "mode": "auto", "autoswitch": true,
                   "waitingFor": null, "holderPid": null, "backoff": null,
                   "lastTickAt": "…Z", "nextTickAt": "…Z",
                   "lastOutcome": { "kind": "no-switch", "reason": "below-threshold", "at": "…Z" },
                   "lastSwitch": { "at": "…Z", "from": 1, "to": 2, "trigger": "proactive" } } ],
  "jobs": [ { "name": "store-watch", "lastRunAt": "…Z", "nextRunAt": "…Z", "lastError": null } ],
  "subscribers": 1 }
```

- **Providers.** `mode` is D4.3's. `autoswitch` is the effective `daemon.autoswitch` (D7).
  `waitingFor` is `engine-lock`, `schema` or null, and `holderPid` is the other engine's pid
  when `waitingFor` is `engine-lock`. `backoff` is `{"reason": "keychain", "until": "…Z"}`
  while the keychain backoff (D4.1) holds, else null. `lastTickAt` and `nextTickAt` hold for `collect` rounds
  too. `lastOutcome.kind` is `switch`, `no-switch`, `collect` or `error`, with a `reason` for
  `no-switch` and `error`. `lastSwitch` is the last switch this daemon made, or null.
- **`backend`** is `osascript`, `notify-send` or null (D6).
- **`jobs`** lists every job other than the provider jobs, including those added later (D13).
- **Paths** are written `~/…` under the home directory, as in the log (§14.2).
- The reply is built from a snapshot that the scheduler updates after each job, so `status`
  never touches the store or waits for a job.

### D5.5 `subscribe` and events

After the `ok` reply, the connection receives every event the daemon emits, one per line, in
§11.4's envelope: `{"schemaVersion":1,"event":<kind>,"ts":"…Z","provider":…,…}`.

- **Auto kinds** (`poll`, `switch`, `no-switch`, `account-quarantined`,
  `account-unquarantined`, `all-exhausted`, `sleep`, `error`, `config-warning`) are exactly
  the objects `tagteam auto --json` prints, from the same function (D3).
- **Daemon kinds:**

  | Kind | Fields | When |
  |---|---|---|
  | `usage` | `provider`, `updated` (positions whose reading was recorded), `fetchErrors?` (position → error kind) | After a `collect` round that recorded or failed something. A tick emits `poll` instead |
  | `changed` | — | Another process committed to the store (D4.4) |
  | `mode` | `provider`, `mode`, `waitingFor?`, `holderPid?`, `backoff?` | A provider job changed mode, or its keychain backoff started or ended (D4.3) |
  | `stopping` | `reason`: `signal`, `upgrade` or `schema` | The last event before the daemon closes |

- **Reserved:** `token-index`, for the token report (D13). Its fields are that workstream's to
  define.
- **A slow subscriber.** Each subscriber has a queue of 256 events. One whose queue is full is
  disconnected, and the scheduler never waits for it. A client that is disconnected re-reads
  what it shows, then subscribes again.

### D5.6 Versioning

- `protocol` (an integer, 1 here) covers the framing, the ops, and the reply and event
  envelopes. `schemaVersion` stays 1, as in all of tagteam's JSON (§13.2).
- New ops, event kinds and fields are additive and do not change `protocol`. Consumers ignore
  kinds and fields they do not know, as §11.4 already requires.
- A breaking change increments `protocol`. A client that does not speak the greeting's protocol
  closes the connection and works without the daemon. Since the daemon is part of the same
  binary, a mismatch exists only between an upgrade and the daemon's restart (D4.5).

### D5.7 The Rust client

`tagteam_daemon::client`, used by the TUI and by `tagteam daemon status`:
- `connect(env) -> Result<Connect, ClientError>`. The result is `Connect::Connected(Client)`,
  `Connect::NoDaemon` (no socket file, a refused connection, or EOF before the greeting), or
  `Connect::Mismatch { server, client }`. The greeting read is bounded at 250 ms.
- `Client::status() -> Result<Status, ClientError>`, which waits up to 2 s for the reply.
- `Client::subscribe() -> Result<Events, ClientError>`. `Events` iterates typed events, and
  yields `Unknown { kind, raw }` for a kind it does not know.
- The types are in `tagteam_daemon::protocol`: requests, replies, `Status`, and the event enum
  with each kind's fields.

## D6. Notifications

### D6.1 What is notified

| Event | Notified when |
|---|---|
| `switch` | The daemon made a switch: a `switch` event that is not a dry run |
| `quarantine` | An `account-quarantined` event, from a tick or from a `collect` round |
| `limit` | The live account's relevant usage (§8.2) is at or above `autoswitch.threshold`, and the daemon did not switch it away. In `auto`: a tick that ended without a switch (`no-switch`, whatever the reason, or `error`). In `collect`: any round |

`limit` is judged after the run, from the store, through engine change 5 (D3): the managed live
account's decision-grade relevant windows under the provider's `autoswitch.models`. With no
managed live account, or no decision-grade reading, nothing is notified. The binding window is
the relevant window with the highest `pct`; its key names the dedup marker, and its
`resets_at` sets the marker's expiry (D6.3).

### D6.2 Text and delivery

- **Title** `tagteam`. The body names accounts by position and the statusline's `{account}`
  (the alias, else the email's local part, §13.5). It never carries a credential, an
  organization name or a full email. Examples:
  - `Switched 1 · work → 2 · spare (5h at 92%)`
  - `2 · spare needs a new login (invalid_grant)`
  - `1 · work: 7d at 93% (resets in 1d04h)`

  The `limit` text does not say why the daemon did not switch (paused, a cooldown, every
  account exhausted), since it is true for only some of those. Durations are written as `list`
  writes them (`1d04h`). A binding window whose reset has already passed is not notified.
- **macOS:** `/usr/bin/osascript -e 'on run argv' -e 'display notification (item 2 of argv) with title (item 1 of argv)' -e 'end run' -- <title> <body>`.
  The text travels as argv, so no account name can inject AppleScript. The notification shows
  as coming from Script Editor, which needs notification permission once (D15).
- **Linux:** `notify-send --app-name=tagteam -- <title> <body>`, when `notify-send` is on `PATH`.
  Without it there is no backend.
- **Each delivery** runs on the notifier thread through the engine's `ProcessSpawner` port,
  with a 5 s timeout, in its own process group (§14.1). Its stdin, stdout and stderr are
  closed.
- **A failure** is logged at WARN the first time and at DEBUG after that, and never fails
  anything. A notification still queued when the daemon stops is dropped.

### D6.3 Dedup

- `switch` and `quarantine` are notified once per event.
- `limit` is notified once per account, binding window and window instance. The marker is a
  lease (§6.1) named `notify:limit:<account id>:<window key>`, taken with §6.1's single
  statement, whose expiry is the window's reset plus 60 s, or one hour from now when the reset
  is unknown. That expiry is what bounds one window instance: a provider can report a window's
  reset a second apart between polls (the codebase treats resets within ±60 s as one instance),
  and a name that carried the reset would notify again. The lease is taken before the notification is queued, so it also holds
  across a restart and across two daemons on one store, which D4.1 rules out anyway.

### D6.4 Turning it off

`notify.enabled = false` turns every notification off. `notify.events` chooses which kinds are
sent, per provider (D7). The foreground daemon notifies exactly as the service does.

## D7. Settings

Three keys are added to §6.4's registry, with every rule of §6.4 (forgiving reads, strict
`set`, provider overrides):

| Key | Default | Valid | Per provider |
|---|---|---|---|
| `daemon.autoswitch` | `true` | bool | yes |
| `notify.enabled` | `true` | bool | — |
| `notify.events` | `["switch","quarantine","limit"]` | a list of those three; duplicates collapse; `''` is the empty list | yes |

- **`daemon.autoswitch`** turns the daemon's auto-switching on or off per provider (D4.3).
  Setting it to false is how auto-switching is paused, from the CLI or from the TUI, through the
  existing `config set` path (§6.4), which works in a run shell too. It persists across
  restarts. It does not affect `tagteam auto`, which the user starts on purpose.
- **The `autoswitch.*` keys** drive the daemon's ticks and `collect` rounds exactly as they drive
  `auto`. The daemon takes no tuning flags: a service has no command line to change, so the
  settings are its interface.

## D8. CLI

| Command | Notes |
|---|---|
| `daemon [--provider P] [--json]` | Runs the daemon in the foreground (D8.1) |
| `daemon install [--print] [--no-start]` | Writes and loads the service unit (D9) |
| `daemon uninstall` | Stops the daemon and removes the unit (D9) |
| `daemon status` | The daemon's state, from the socket, else the service's (D8.2) |

All four take the global `--json`. `--provider` limits the daemon to one provider's job; the
others are not created.

### D8.1 `tagteam daemon`

- **Human output:** `auto`'s line per tick (§11.4), plus one line per mode change, notification,
  upgrade exit and start and stop. Errors and config warnings go to stderr, as `auto`'s do.
- **`--json`:** every event as one JSONL object, exactly as `subscribe` delivers them (D5.5).
- **Exit codes:**
  - `0` after a signal once the daemon serves (D4.6). A signal during the wait for
    `daemon.lock` (D4.1), before the daemon serves, ends it as an interrupted command:
    128 + the signal number, with the `interrupted` error (§14.1)
  - `75` to be restarted on a new binary or schema (D4.5)
  - `1` when it cannot start: as root, in a run shell, with a store it cannot open, or after a
    declined or failed keychain unlock on a terminal (D4.1)
  - `2` for a usage error

### D8.2 `tagteam daemon status`

- **With a daemon:** the socket's `status` (D5.4), rendered as text, or under `--json` as
  `{schemaVersion, running: true, daemon: <D5.4's object>, service: <below>}`.
- **Without one:** `running: false` and `daemon: null`, plus the service's state:
  `service: {kind: "launchd"|"systemd"|null, path, installed, loaded, lastExitStatus}`. The
  state is read with `launchctl print gui/$UID/<label>` or `systemctl --user show tagteam.service`.
  `kind` is null on a platform without either.
- **A daemon that holds `daemon.lock` but does not answer** (no socket, D5.1): `running: true`,
  with the pid from the lock's record and `daemon: null`.
- **Exit codes:** `0` running, `3` not running (the LSB status convention), `1` on an error.

### D8.3 `tagteam auto` with a daemon running

`auto` behaves as before (§11.1). Its message for a provider whose engine lock is held now names
the holder when the holder record matches `daemon.lock`'s: `auto-switch already runs for
claude-code (the daemon, pid 4242; pause it with tagteam config set daemon.autoswitch false)`.

## D9. Service units

`daemon install` writes a unit that starts `tagteam daemon` at login and restarts it on failure,
then loads it and starts it, restarting a daemon the unit already runs, so a reinstall's new
program path and environment take effect at once (D9.2, D9.3).
- **`--no-start`** writes the unit and enables it for the next login, and starts or stops
  nothing: on macOS the plist is written but not bootstrapped, on Linux the unit is enabled but
  not started. A daemon that is already running keeps its old unit until it restarts, and
  `install` warns so.
- **Refusals.** It refuses as root and inside a run shell. Running it again replaces the unit
  and reloads it.
- **Warning.** Its output says plainly that the daemon will switch accounts unattended, and how
  to pause that (D7).
- **`--print`** prints the unit for the current platform and changes nothing, for users who
  manage units themselves.

### D9.1 What the unit records

- **The program path.** This is the first `tagteam` on `PATH` when it is the running binary
  (same device and inode), else the running binary's own path (`std::env::current_exe`). Homebrew's
  and cargo's stable links therefore survive upgrades, and D4.5's watch sees the new binary
  through them.
- **The arguments** are `daemon` only.
- **The environment**, so that the daemon resolves the same store, Claude Code home, proxy and log
  as the shell that installed it. These are the variables that are set at install time:
  - `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and `XDG_STATE_HOME`
  - `CLAUDE_CONFIG_DIR` and `CLAUDE_SECURESTORAGE_CONFIG_DIR`
  - `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` and `NO_PROXY`, in either case
  - `TAGTEAM_LOG`
  - `PATH`, which `notify-send` is found on

  No other variable is recorded. Of the variables `run` scrubs from a session (§12.5), only
  `CLAUDE_SECURESTORAGE_CONFIG_DIR` is: it carries no credential, but names where the live
  login's credential is stored, which a switch must find exactly as the user's shell does. No
  variable that carries a credential or points at one, the rest of §12.5's list, is recorded.
- **The unit file** is created 0600 (`O_EXCL`, write, rename, as §5 does for secrets), because a
  proxy URL can carry a password.

### D9.2 macOS (launchd)

- **File:** `~/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist`, label
  `io.github.michaelrigart.tagteam`.
- **Keys:**
  - `ProgramArguments` `[<path>, "daemon"]`
  - `EnvironmentVariables`
  - `RunAtLoad` true
  - `KeepAlive` `{SuccessfulExit: false}`: restarted after a failure and after exit 75, never
    after exit 0
  - `ProcessType` `Background`
  - `StandardOutPath` and `StandardErrorPath` `/dev/null`: the log file is the record (§14.2)
- **Loading.** `launchctl bootout gui/$UID/<label>` first, which stops a running daemon and where
  "not loaded" is not an error, then `launchctl bootstrap gui/$UID <plist>`, which starts the new
  one (`RunAtLoad`). Without a GUI session (SSH only), `bootstrap` fails. `install` then reports
  it and suggests running `tagteam daemon` in the foreground.
- **`uninstall`:** `bootout`, then the file is deleted.

### D9.3 Linux (systemd user unit)

- **File:** `$XDG_CONFIG_HOME/systemd/user/tagteam.service`:

  ```ini
  [Unit]
  Description=tagteam daemon

  [Service]
  Type=simple
  ExecStart=<path> daemon
  Restart=on-failure
  RestartSec=10
  StandardOutput=null
  Environment="XDG_DATA_HOME=…"

  [Install]
  WantedBy=default.target
  ```

- **Quoting** follows systemd's rules, which differ by setting (systemd.exec(5), systemd.service(5)):
  - `Environment=` assignments are double-quoted, with `\` and `"` escaped and `%` written
    `%%` (specifiers are expanded there). `$` is left alone: `Environment=` expands no
    variables.
  - `ExecStart=`'s program path is double-quoted, with `\` and `"` escaped, `%` written `%%`
    and `$` written `$$` (command lines expand both).
  - A value or path holding a newline or a NUL is refused.
- **Loading:** `systemctl --user daemon-reload`, `systemctl --user enable tagteam.service`, then
  `systemctl --user restart tagteam.service`, which starts it when it is stopped and restarts
  it when it runs, so a reinstall's new environment takes effect at once.
- **Lingering.** When `loginctl show-user $USER -p Linger` reports `no`, `install` adds a hint:
  the daemon stops at logout unless `loginctl enable-linger` is run.
- **Without `systemctl`,** `install` fails and names `--print`.
- **`uninstall`:** `systemctl --user disable --now tagteam.service`, the file deleted, then
  `daemon-reload`.

### D9.4 Results

Each command runs `launchctl` or `systemctl` through the `ProcessSpawner` port with a 10 s
timeout. A failure reports that program's stderr and exit status. The commands return `0` on
success, `1` on failure, and `2` for a usage error. JSON shapes:
- `install`: `{schemaVersion, ok, service: {kind, path, label}, started, warnings}`
- `uninstall`: `{schemaVersion, ok, removed}`. `uninstall` with no unit installed succeeds with
  `removed: false`.

## D10. Doctor

New checks for §13.6, all read-only. `doctor` takes no lock and does not connect to the socket.
It reads the service state with `launchctl print`, `systemctl --user show` or `loginctl show-user`,
each bounded by a 5 s timeout; none changes anything.

| Check | Reports |
|---|---|
| `daemon.running` | info: running, with the pid and version from `daemon.lock`'s record (judged live by an exact pid and start-time match, §12.6). warn: its version differs from this binary's, "restart it". info: not running |
| `daemon.service` | info: installed and loaded, or not installed. fail: the unit's program path does not exist. warn: a recorded variable (D9.1) other than `PATH` differs from doctor's own environment, so the daemon uses another store or home; the fix is `tagteam daemon install` again. `PATH` places no store or home, and a version manager changes it in every directory, so it is not compared. info (Linux): lingering is off. warn (Linux): whether lingering is on cannot be told (`loginctl` missing, hung or failing), with the reason |
| `daemon.socket` | info: a socket file with no live daemon (the next start replaces it). warn: its mode is not 0600, or a live daemon has no socket file (it runs without one, D5.1). fail: its path is too long to bind (D5.1), once a daemon runs or a unit is installed; until then the length is not judged, so `doctor` never fails for someone who does not use the daemon |
| `daemon.notify` | warn: notifications are on but there is no backend (`notify-send` missing) |

The existing auto-switch check ("an engine running", §13.6) says `the daemon (pid N)` when the
engine lock's record matches `daemon.lock`'s.

## D11. Logging

The daemon logs to the one shared log file (§14.2), under every rule there. At INFO it records
start and stop, each mode change, upgrade exits, `install` and `uninstall`, socket bind
failures, and everything a tick logs today. Notifications sent, and connections opened and
closed, log at DEBUG. Under a service, stdout and stderr are discarded (D9), so the log file is
the daemon's only record.

## D12. Constraints kept

- **The usage budget** (§8.6, §1.1 criterion 3). The daemon collects only through the
  collector (§8.3), under its leases and the `usage_requests` budget, so it is one more process
  inside the per-identity budget. `list`, `status` and `switch` will mostly find readings
  younger than the 180 s floor, or not due, and fetch nothing.
- **`statusline` and `list` p95.** The daemon is a separate process whose transactions are as
  short as `auto`'s. `tests/perf.rs` gains runs with a fixture daemon at the shortest
  auto-switch interval (15 s, §6.4), so it ticks while the runs are timed (D14).
- **`doctor`'s engine check** keeps working from the engine lock records, which the daemon
  writes as `auto` does, and empties when it lets a lock go (D3 change 3).
- **The local-state invariant** (§3). The daemon writes CC state only through the operations
  that already may: a switch and the active-token refresh. The pinned test (§15.3) covers the
  daemon's runs.
- **Signals and logging** follow §14.1 and §14.2 (D4.6, D11).

## D13. Room for the other workstreams

- **Token report.** Its index refresh and its watch on every home's `sessions/` records become
  two more scheduler jobs (D4.2). They appear in `status.jobs` with no protocol change. When
  the index advances, the job emits the reserved `token-index` event (D5.5). Whichever branch
  merges later wires these.
- **TUI.** It uses `tagteam_daemon::client` (D5.7). `changed`, `usage`, `poll`, `switch` and the
  quarantine events are its refresh triggers. Pausing auto-switching from the TUI writes
  `daemon.autoswitch` (D7), and the TUI shows each provider's `mode` and `autoswitch` from
  `status`.

## D14. Testing

All tests use the isolation rules of §15.1. Socket tests bind in a temporary directory whose
path is short enough for `sun_path`: macOS's `$TMPDIR` can push a socket path past it.

- **Scheduler.**
  - `auto`'s loop tests move with the loop and pass unchanged, including the suspend jump and
    the settings reload.
  - Mode transitions:
    - the engine lock held by another process (`waiting`), then freed, then taken (`auto`)
    - accounts added (`idle` → `collect` → `auto`) and disabled again
    - `daemon.autoswitch` set false (`auto` → `collect`) takes effect within one slice when no
      job is running, and the lock's record is empty afterwards
  - `collect` never tries the engine lock.
  - `collect` reports quarantine changes.
  - an `auto`-mode tick with an unmanaged, or no, live login still collects the candidates
  - on macOS without a terminal: no `show-keychain-info` is spawned. With a scripted `security`,
    a run whose secret reads all end rc 36, or time out, starts the backoff, whether the
    reads came from collecting, freshening or switching. A locked OAuth read beside an absent
    managed-key item (rc 44) still starts it. A run with one answered secret read does not, and
    a run with an answered secret read ends the backoff. Setting `daemon.autoswitch` false during a
    backoff releases the engine lock at once, and the next delayed run is a `collect` round
- **Socket.**
  - the greeting and the peer check
  - `status` and `subscribe` interleaved on one connection
  - event order within a tick
  - a subscriber that never reads is dropped without delaying a tick (a tick under a stalled
    subscriber takes as long as without one)
  - an oversize line, a non-object line, and the 17th connection
  - the idle timeout
  - a stale socket file replaced
  - mode 0600, and a path too long to bind
- **Client.** `NoDaemon` for a missing socket, a refused connection and EOF before the greeting;
  `Mismatch` for another protocol; `Unknown` for an unknown kind.
- **Protocol.** Every event and reply shape is snapshot tested. The auto kinds are asserted equal
  to `auto --json`'s output for the same tick.
- **Notifier.**
  - A scripted spawner records argv: osascript's and notify-send's, the text never carrying a
    credential, organization or full email, and an account name with quotes and newlines
    passing as argv.
  - `limit` is notified once per window instance, across a daemon restart, in `auto` and in
    `collect` mode, and never without a managed live account.
  - `notify.enabled` and `notify.events` are honoured.
- **Service units.** The plist and the systemd unit are snapshot tested, with recorded variables
  and quoting, including a value holding `$`, `%`, `"` and `\` that must reach the daemon
  unchanged. A reinstall restarts a running daemon; `--no-start` starts and stops nothing. `install`, `uninstall` and `status` are run against a scripted spawner that checks
  `launchctl` and `systemctl` argv. A real launchd and systemd round trip is an `#[ignore]` test,
  run by hand.
- **Concurrency** (in separate processes, §15.2):
  - a daemon and `tagteam auto` on one provider: exactly one is the engine, and the other
    waits or reports `engine-running`
  - two daemons: the second waits, and takes over when the first stops
  - a CLI switch while the daemon runs gives `changed`
  - the 20-per-hour budget holds with the daemon collecting, including alongside `list`
- **Exits.**
  - a replaced binary, and a schema version bumped while running, each give `stopping` and exit
    75 after a clean stop
  - a schema already newer at start-up waits (`waitingFor` `schema`) and does not exit until the
    binary is replaced
  - SIGTERM gives exit 0, no socket file, empty engine lock records and a free `daemon.lock`
- **Doctor.** Each new check in each state, snapshot tested, with every file left byte-identical.
- **Performance.** `tests/perf.rs`: `statusline` and `list` p95 with a fixture daemon running
  at the shortest auto-switch interval (15 s, §6.4), at least one tick landing among the timed
  runs.
- **Provider neutrality.** The daemon's tests run against both providers (§15.2), with
  `FakeAgent`'s missing capabilities degrading as §4.5 says.

## D15. Risks and open items

| # | Risk / item | Mitigation |
|---|---|---|
| D-R1 | Whether `osascript`'s notifications show on macOS 27 when run by a launchd agent, and what Script Editor's permission prompt looks like on first use | Checked by hand during implementation, on this Mac, from a loaded agent. If they do not show, macOS notifications wait for the menu bar app (sub-project 4); the daemon itself is unaffected |
| D-R2 | A locked login keychain in a GUI session. `show-keychain-info` opens a SecurityAgent dialog there (A.3), and whether a secret read (`-w`) does too, and what rc it then ends with, is not known. An unattended daemon could raise dialogs repeatedly | Without a terminal the daemon never runs the lock check, and a run whose secret reads all end locked or time out backs off to 15 min (D4.1). Whether reads prompt, and their rc, is checked by hand during implementation, from a loaded agent with the login keychain locked; if they prompt, or end with an rc the counters do not class as locked, the backoff is revisited before release |
| D-R3 | Keychain access from a launchd agent. R1 verified GUI and SSH sessions, not a `gui/$UID` agent | Checked by hand during implementation, by a switch and a refresh made from the loaded agent |
| D-R4 | A `brew upgrade` that leaves the old binary in place until `brew cleanup` | D4.5 watches the stable link's target, not the running file, so the daemon restarts as soon as the link moves |
| D-R5 | The daemon holds one SQLite connection for days | WAL with short transactions, as every command uses (§6.1). The upgrade watch restarts it when a newer tagteam migrates the store |

## D16. Amendments to the core spec

These are made in small hunks in the same branch:
- **§1.2:** the daemon is no longer a non-goal; it points here.
- **§4.1:** `tagteam-daemon` joins the crate table.
- **§5:** add the socket (`daemon.sock`), `locks/daemon.lock`, and the unit files.
- **§6.4:** add the three keys (D7).
- **§11.1:** the daemon's line points to D4.3 and says that a released lock's record is emptied.
- **§13.1:** add the `daemon` commands.
- **§13.6:** add the daemon checks (D10).

## Errata

Found while planning the implementation, and corrected in place above. None changes the
design:
- **D4.6:** the socket is removed when `stopping` is sent, before `daemon.lock` is released,
  and only if it is still the socket this daemon bound; `daemon.lock`'s record is emptied
  before the lock goes.
- **D5.1:** the umask is set before any thread that creates a file or a process starts; the
  notifier thread exists by then but only waits on its queue, which nothing can fill before the
  daemon serves.
- **D6.2:** the `limit` example no longer claims that no account is left to switch to, which
  the daemon cannot know, and writes durations as `list` does.
- **D6.1, D6.3:** the `limit` lease is named `notify:limit:<account id>:<window key>`, without the
  reset; its expiry bounds one window instance, since a provider can report a reset a second
  apart between polls and a name that carried it would notify again. D6.1 says the same: the
  window's key names the marker, and its `resets_at` sets the marker's expiry.
- **D8.1:** a signal during the wait for `daemon.lock`, before the daemon serves, exits
  128 + n as an interrupted command.
- **D9.1:** `CLAUDE_SECURESTORAGE_CONFIG_DIR` is recorded although `run` scrubs it; no
  variable that carries or points at a credential is.
- **D10:** `PATH` is left out of the variable comparison; a lingering that cannot be told warns
  with its reason; a too-long socket path fails only once a daemon runs or a unit is installed;
  a live daemon with no socket file warns; the intro names `loginctl show-user` among the
  commands it reads.
- **D12, D14:** the perf daemon runs at the shortest auto-switch interval (15 s), since
  §6.4 allows nothing shorter.
