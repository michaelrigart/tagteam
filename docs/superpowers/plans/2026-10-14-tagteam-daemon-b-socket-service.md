# tagteam daemon B — Socket, Client, Service Units, Doctor and Docs Implementation Plan

**Status:** In progress — https://github.com/michaelrigart/tagteam/pull/17

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The daemon's outward surface:
- the 0600 socket and protocol 1 (D5), with the Rust client the TUI uses
- `tagteam daemon install|uninstall|status` with launchd and systemd units (D8, D9)
- the `doctor` checks (D10), and `auto` naming the daemon when it holds a provider (D8.3)
- multi-process and performance tests (D14)
- the core spec amendments (D16), and the daemon spec's errata found while planning
- once the token report has merged, its two scheduler jobs (D13; Task 9, gated)

It builds on plan A (`2026-10-14-tagteam-daemon-a-scheduler.md`), which must be complete.

**Architecture:**
- **`tagteam-daemon`:**
  - `protocol.rs` gains the requests, replies, greeting and status JSON, and their typed
    client-side mirror (Task 1)
  - `server.rs`: the socket, its threads and the subscriber hub (Task 2)
  - `client.rs` (Task 3)
  - `service.rs`: install and uninstall over the engine's units (Task 5)
  - `tokens.rs`: the token report's two jobs (Task 9, gated)
- **`tagteam-engine`:**
  - `service.rs`: unit paths, rendering, quoting, recorded variables, the one way to call a
    service manager, and the read-only state probe (Task 4)
  - `doctor.rs` gains the daemon checks (Task 6)
- **`tagteam-provider`:** `process::current_uid` (Task 4), since the engine forbids `unsafe`.
- **`tagteam` (CLI):**
  - the server wired into `tagteam daemon` (`daemon_cmd.rs`, `app.rs`; Task 2)
  - `daemon install|uninstall|status` (`cli.rs`, `app.rs`, the new `service_cmd.rs`; Task 5)
  - `doctor`'s platform, the recorded variables captured at the process boundary, and `auto`'s
    message (Task 6)
  - Decision 7's test hooks (Task 7)

**Tech Stack:** Rust (edition 2024), `std::os::unix::net`, `libc` (`umask`, `poll`,
`getpeereid`, `SO_PEERCRED`, `getuid`), and `serde` with derive (already a workspace
dependency) for the client's typed replies. Tests use `ScriptedSpawner` (the `launchctl`,
`systemctl` and `loginctl` argv), stand-in manager scripts on `PATH`, real Unix sockets in short
temporary directories, and the binary via `assert_cmd`. No new external crate, and no manifest
changes: plan A's `tagteam-daemon` manifest already has every dependency these tasks use.

**Spec:** `docs/superpowers/specs/2026-10-14-tagteam-daemon-design.md` (D1–D16), signed off at
`96b9cea`. Read D5, D8, D9, D10, D13, D14 and D16 before starting any task.

## Execution notes

- **Order.** Run this plan after plan A, in `~/Code/tagteam-daemon` on branch `daemon`. Both
  plans go into one Draft PR.
- **One task at a time.** Tasks run one at a time, in the Task Overview's order: 1, 2, 3, 4, 6,
  5, 7, 8, 9, 10 (Task 5 uses Task 6's `App::service()`). Two tasks in one crate share a
  compile, and a crate's tests build the crates it depends on, so tasks run in parallel would
  build each other's unfinished code.
- **Status.** When execution starts, set this plan's `**Status:**` to `In progress`.
- **Flags and formatting** are as in plan A. Clippy must pass both with
  `--features tagteam/test-support` and with no features. Binary tests are gated
  `#![cfg(feature = "test-support")]` and run with `cargo test -p tagteam --features
  test-support`. A full run of the daemon crate's tests is
  `cargo test -p tagteam-daemon --features test-hooks` (plan A's feature; `tagteam/test-support`
  turns it on). Every task runs `cargo fmt --all` before `cargo fmt --all --check`.
- **Test fixtures.** Plan A's `crates/tagteam-daemon/src/testutil.rs` is crate-private and
  `#[cfg(test)]`: only unit tests in `src/` reach it. This plan's integration tests in
  `crates/tagteam-daemon/tests/` use their own `tests/common/mod.rs` (Task 2). Each crate has
  one short-directory helper for socket paths, `short_tempdir()`: the daemon crate's
  `tests/common` (Task 2), the CLI's `tests/common` (Task 2), and the engine's
  `tests/doctor.rs` (Task 6).
- **Sandbox.** Claude Code's macOS sandbox refuses `bind` on a Unix socket (`EPERM`) and writes
  under `/tmp`, where `short_tempdir()` makes its directories. Run every command sandboxed
  first, and again outside only when it failed with sandbox evidence (`Operation not
  permitted`, `EPERM`). These always need it, because they bind Unix sockets or spawn and
  signal `tagteam daemon`:
  - `cargo test -p tagteam-daemon --test server --test umask` (Task 2) and `--test client`
    (Task 3), and every full run of the crate's tests, which includes them:
    `cargo test -p tagteam-daemon --features test-hooks`
  - `cargo test -p tagteam --features test-support --test daemon_cli` (plan A Task 12; Task 2)
  - `cargo test -p tagteam --lib service_cmd` (Task 5's
    `a_daemon_that_holds_its_lock_but_does_not_greet_is_running` binds under `/tmp`)
  - `cargo test -p tagteam-engine --test doctor` (Task 6's
    `the_daemon_checks_change_no_byte_take_no_lock_and_never_connect` binds a listener)
  - `cargo test -p tagteam --features test-support --test daemon_procs` (Tasks 7 and 9)
  - `cargo test --release -p tagteam --features test-support --test perf -- --ignored`
    (Task 7's daemon runs; Task 10)
  - every full CLI or workspace run (`cargo test -p tagteam --features test-support`,
    `cargo test --workspace …`), which includes the above, and plan A's pty tests
  - Task 5's ignored round trip, run by hand in Task 10

  `docker` (Task 10) is declared in `sandbox.excludedCommands`: it runs as its own Bash call,
  `docker` first and nothing piped, never with the escape flag.
- **Socket tests** bind in a directory `short_tempdir()` makes under `/tmp`, falling back to
  `$TMPDIR` when `/tmp` cannot be written. macOS's `$TMPDIR` can push a socket path past
  `sun_path` (`MAX_SOCKET_PATH`), and the CLI's helper fails the test, saying so, when it does.
- **No test runs the real `launchctl`, `systemctl` or `loginctl`**, except D14's `#[ignore]`d
  round trip (Task 5), which Task 10 runs by hand with Michael's go-ahead.
- **Task 9 is gated.** It runs only once the token report's branch has merged into `main` and
  `daemon` has been rebased onto it (merge order: token report, daemon, TUI). If the daemon PR
  is ready first, Task 9 waits, Task 10 runs without it, and the PR description says so.

## Decisions

Rulings made while planning. Each names what it would cost if wrong.

1. **Protocol framing violations close the connection; malformed requests get a reply.** A
   line longer than 64 KiB, invalid UTF-8, or JSON that is not an object closes the connection
   (D5.2). An object whose `id` is missing or not a string or integer, or whose `op` is not a
   string, gets `bad-request`, with the `id` when there was a usable one. An unknown `op` gets
   `unknown-op` (D5.3). Cost if wrong: one class of input moves between replying and closing.
2. **One thread per connection, which also writes.** It reads with a 100 ms timeout. Between
   reads it drains its subscriber queue (`sync_channel(256)`) and writes the queued events,
   then any reply, in the order produced. The hub's `emit` does a `try_send` to each
   subscriber; a full queue marks that subscriber dropped, and its thread closes the
   connection (D5.5). Cost if wrong: latency of up to 100 ms per event, which is well inside
   what the TUI needs. A burst of more than 256 events within one 100 ms window would drop
   every subscriber; a tick emits a handful, and the fix (waking the connection thread per
   event) is a larger design that was not taken.
3. **Peer check.** It uses `getpeereid` on macOS and `getsockopt(SO_PEERCRED)` on Linux,
   through `libc`. A uid other than `geteuid()` is closed before the greeting. Cost if wrong:
   none for a same-user socket in a 0700 directory, since this is defence in depth.
4. **Units are the engine's.** `tagteam_engine::service` renders and parses both unit kinds,
   calls the managers (`run_manager`) and reads the service state, because `doctor` (engine)
   needs all of that. The daemon crate's `service.rs` only performs the install and uninstall
   commands. Cost if wrong: a module moves crates.
5. **The plist is rendered by hand** (escaped XML, fixed key order), not with a plist crate. It
   is parsed back for `doctor` by a small reader that only understands what we write. A unit
   we did not write reads as "installed, environment unknown" and is never an error. Cost if
   wrong: `doctor` cannot judge a hand-edited plist, and says so.
6. **`daemon status` exit codes.** 0 running, 3 not running, 1 error (D8.2). `--json` prints
   its object whatever the code. Cost if wrong: a script's exit-code check.
7. **Test hooks.** Under `test-support` only, `tagteam daemon` reads two variables in
   `App::daemon`, where plan A produces the values they replace (Task 7):
   - `TAGTEAM_TEST_DAEMON_PROGRAM` replaces the program path the upgrade watch follows
     (`daemon_cmd::test_program`), inside the `ProgramStamp::take` that is `App::daemon`'s
     first statement (plan A Decision 17), so the stamp is of the test's file.
   - `TAGTEAM_TEST_UPGRADE_WATCH_MS` replaces `DaemonOptions::upgrade_watch_ms`
     (`daemon_cmd::test_upgrade_watch_ms`), the period `serve` gives `UpgradeWatch::with_period`,
     so Task 7's exit-75 tests take seconds.

   A release build ignores both. Cost if wrong: slow tests.
8. **The manual checks (D15 D-R1, D-R2, D-R3) and D14's real round trip touch Michael's
   machine.** They load a real LaunchAgent and may switch real accounts, so Task 10 asks Michael
   before running any of them. It uses throwaway agents and fixture stores where it can. Cost if
   wrong: an unattended switch of Michael's live login.
9. **The socket goes when `stopping` does** (D4.6). Plan A's `Started::serve` emits `stopping`
   before it drops the jobs, the notifier and, last, `daemon.lock`. The hub, on receiving it,
   queues it for every subscriber, stops the accept thread from accepting, and removes the
   socket file if its device and inode are still the ones `bind` created. `Server::shutdown`,
   which runs after `serve` returns and so after the lock has gone, then closes the
   connections and joins the threads; its own removal is guarded the same way and does nothing
   a second time, so it never removes a successor's socket. Open connections stay served until
   `shutdown`, so each subscriber reads `stopping`. Cost if wrong: a client connecting in the
   last instants before the stop finds no socket rather than a greeting it would lose anyway.
10. **D9.1's list governs: `CLAUDE_SECURESTORAGE_CONFIG_DIR` is recorded** although `run`
    scrubs it (§12.5), which D9.1's last sentence forbade. The daemon works on the outer home,
    where this variable places the live credential, and `run` removes it only from a session's
    profile. No variable that carries or points at a credential is recorded. Task 8 corrects
    D9.1 (erratum a). Cost if wrong: one entry leaves `RECORDED_VARS`, and a user with a
    non-default secure-storage directory has a daemon that cannot find the live credential.
11. **Doctor's environment comparison leaves out `PATH`.** It places no store or home, and a
    version manager (mise, direnv) changes it in every directory, so comparing it would warn on
    almost every run. `PATH` still decides where the daemon finds `notify-send`, which
    `daemon.notify` reads from the unit. Cost if wrong: a daemon whose recorded `PATH` lost
    something it needs is not flagged by `daemon.service`.
12. **A socket path too long to bind fails `doctor` only once the daemon is in use**: a daemon
    runs, or a unit is installed. Before that the length is not judged. Cost if wrong: a user
    with a long data path learns of it at `install` or first start, not from an earlier
    `doctor`.
13. **A live daemon with no socket file warns** (`daemon.socket`). D10 did not list the state; a
    failed bind leaves the daemon running without a socket (D5.1), and the TUI and
    `daemon status` cannot reach it. Cost if wrong: one more warning state to keep.
14. **`daemon status`**: `--json` adds a top-level `pid` (the daemon's, the lock holder's, or
    null), beside D8.2's keys. A daemon that accepts but does not greet within 250 ms, or does
    not answer `status` within 2 s, while a live process holds `daemon.lock`, counts as running
    (D8.2's "holds `daemon.lock` but does not answer"), with the lock's pid; without a live
    holder the timeout is an error, exit 1. Cost if wrong: one JSON key, and a hung daemon
    reported as an error instead.
15. **`daemon install --print`** is refused inside a run shell, as `install` is: what it prints
    would record a session's environment. Under `--json` it prints
    `{schemaVersion, service: {kind, path, label}, unit}`. Cost if wrong: a user who manages
    units by hand must leave the run shell first.
16. **D8.3's message names the provider by its display name** ("Claude Code"), as the
    `engine-running` refusal already does, where D8.3's example wrote `claude-code`. Cost if
    wrong: one `format!` and its tests.
17. **Service-manager calls run under a fresh cancel token** (`run_manager`), bounded by their
    timeout (10 s from `install`, `uninstall` and `status`; 5 s from `doctor`). An install's
    unload and load are one change that a Ctrl-C between them would leave half made, and a
    probe is short. A signal meets the command at its next cancellation point. Cost if wrong:
    a Ctrl-C during a hung manager waits for its timeout.
18. **The perf daemon runs at the shortest auto-switch interval, 15 s** (§6.4), not "every
    second" as D12 said, which §6.4 does not allow. Its scheduler still wakes every second and
    its store watch reads every 2 s, and the timed runs are spread over about 19 s so a tick
    lands among them (Task 7). Task 8 corrects D12 and D14 (erratum b). Cost if wrong: the perf
    runs see fewer ticks than a faster daemon would cause.

## Global Constraints

Plan A's Global Constraints apply unchanged. In addition:
- **Socket** (D5.1): `env.data_dir().join("daemon.sock")`, created 0600 (umask `0177` around
  `bind`), and closed for a different peer uid. It is removed when `stopping` is emitted, while
  `daemon.lock` is still held, and only while it is the file this daemon bound (Decision 9).
  - At most 16 connections; the 17th is closed before the greeting.
  - At most 64 KiB per line.
  - 30 s idle timeout before a subscribe; 256 queued events per subscriber.
  - The client bounds its greeting read at 250 ms and its `status` reply at 2 s.
- **Protocol 1** (D5): the greeting is `{"schemaVersion":1,"event":"hello","protocol":1,…}`,
  requests are `{"id","op"}`, and replies are `{"schemaVersion":1,"id","ok","result"|"error"}`.
  Events use §11.4's envelope. Additive changes never bump `protocol`.
- **Units** (D9):
  - launchd: label `io.github.michaelrigart.tagteam`, at
    `~/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist`.
  - systemd: `$XDG_CONFIG_HOME/systemd/user/tagteam.service`.
  - Files are created 0600 with `O_EXCL`, write, rename.
  - The recorded variables are only those set at install time, from D9.1's list
    (`RECORDED_VARS`), read from `Env.vars`, which `Context::from_process` fills.
- **`launchctl`, `systemctl` and `loginctl`** are found on the environment's `PATH` and run
  through `ProcessSpawner` (`run_manager`), with a 10 s timeout from `install`, `uninstall` and
  `status`, and 5 s from `doctor` (D9.4, D10).
- **`doctor` stays read-only:** no lock, no socket connection, and every file byte-identical
  (§13.6).

## Review Focus

1. **A client that connects while the daemon is starting or stopping.** The socket file can
   exist before `listen` or after the accept thread has gone. `connect` must answer `NoDaemon`,
   never hang or error. Test: `a_socket_without_a_listener_is_no_daemon` (Task 3).
2. **Sixteen subscribers, one of them stalled,** while a tick emits a burst. The others get
   every event in order, and the stalled one is dropped alone. Test:
   `a_stalled_subscriber_is_dropped_alone_and_the_rest_see_every_event` (Task 2).
3. **A program path with spaces or non-ASCII characters** (`/Users/…/Application
   Support/…`). The plist and the systemd unit both carry it so the service starts the same
   file. Test: `unit_files_carry_a_program_path_with_spaces_and_unicode` (Task 4).
4. **A reinstall over a unit from another data directory.** `install` replaces it, and the new
   environment is the one that runs: a running daemon is restarted. Test:
   `reinstall_replaces_the_unit_and_restarts_a_running_daemon` (Task 5).
5. **`launchctl` or `systemctl` missing, hanging or failing during `doctor`.** Each check
   reports `warn` with the reason within its 5 s bound, and never fails the run by itself.
   Test: `doctor_reports_a_hanging_or_missing_service_manager_as_a_warning` (Task 6).

## File Structure

| File | Task | Responsibility |
|---|---|---|
| `crates/tagteam-daemon/src/protocol.rs` | 1, 9 | Requests, replies, greeting, `status_json`, `home_relative`; typed `Hello`, `Status`, `Event`, `parse_event`; Task 9's `token-index` |
| `crates/tagteam-daemon/src/server.rs` (new), `lib.rs` | 2 | Bind, peer check, accept and connection threads, `Hub` and its close on `stopping` |
| `crates/tagteam-daemon/tests/common/mod.rs`, `tests/server.rs`, `tests/umask.rs` (new) | 2 | The socket tests' fixture (`short_tempdir`, `board`, `Peer`, …) and the server's tests |
| `crates/tagteam-daemon/src/client.rs`, `tests/client.rs` (new) | 3 (9 edits one test) | `connect`, `Client`, `Events` |
| `crates/tagteam-provider/src/{process,lib}.rs` | 4 | `current_uid` |
| `crates/tagteam-engine/src/service.rs` (new), `lib.rs` | 4 | Unit paths, rendering, parsing, recorded variables, `run_manager`, state probe, lingering |
| `crates/tagteam-daemon/src/service.rs` (new) | 5 | `install`, `uninstall` over `launchctl` and `systemctl` |
| `crates/tagteam/src/daemon_cmd.rs` | 2, 7, 9 | `bind_socket` and `serve`; Decision 7's `test_program` and `test_upgrade_watch_ms`; the `token-index` arm |
| `crates/tagteam/src/app.rs` | 2, 5, 6, 7 | `App::daemon`'s socket and hooks; `daemon_action` and the service commands; `App::service`, `RECORDED_VARS` capture, doctor's platform, D8.3 |
| `crates/tagteam/src/cli.rs` | 5 | `Command::Daemon { action }`, `DaemonAction` |
| `crates/tagteam/src/service_cmd.rs` (new), `lib.rs` | 5 | The unit's program, `find_daemon`, the commands' human and JSON forms |
| `crates/tagteam/tests/daemon_service_cli.rs` (new) | 5 | `install`, `uninstall`, `status` through the binary; D14's ignored round trip |
| `crates/tagteam-engine/src/doctor.rs`, `tests/doctor.rs` | 6 | `daemon.running`, `daemon.service`, `daemon.socket`, `daemon.notify`; `auto.engine` naming the daemon |
| `crates/tagteam/tests/{doctor_cli,auto_cli}.rs` | 6 | The checks and D8.3 through the binary |
| `crates/tagteam/tests/common/mod.rs` | 2, 7 | `short_tempdir`; `login_with_token`, `Guarded` |
| `crates/tagteam/tests/daemon_cli.rs` | 2 | The socket through `tagteam daemon` |
| `crates/tagteam/tests/daemon_procs.rs` (new), `tests/perf.rs` | 7, 9 | Multi-process tests; p95 with a daemon running |
| `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`, `…-tagteam-daemon-design.md`, `README.md`, `CHANGELOG.md` | 8 (9, 10) | D16's amendments; the daemon spec's errata; user docs |
| `crates/tagteam-daemon/src/tokens.rs` (new), `events.rs`, `daemon.rs`, `tests/tokens.rs` (new) | 9 | The token report's jobs (gated) |

## Interfaces

Plan A's interfaces hold. These are added; the blocks describe the code the tasks write.

```rust
// tagteam-provider/src/process.rs (Task 4), re-exported at the crate root
pub fn current_uid() -> u32;                                  // getuid(2)

// tagteam-daemon/src/protocol.rs (Task 1)
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum RequestId { Int(i64), Str(String) }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op { Status, Subscribe }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request { pub id: RequestId, pub op: Op }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestError { pub id: Option<RequestId>, pub kind: &'static str, pub message: String } // BAD_REQUEST | UNKNOWN_OP
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed { Request(Request), Refuse(RequestError), Close }
pub const MAX_LINE: usize = 64 * 1024;                       // D5.2, the newline left out
pub const BAD_REQUEST: &str = "bad-request";
pub const UNKNOWN_OP: &str = "unknown-op";
pub fn parse_request(line: &[u8]) -> Parsed;                 // Decision 1
pub fn hello_json(d: &DaemonInfo) -> serde_json::Value;      // D5.2
pub fn ok_json(id: &RequestId, result: serde_json::Value) -> serde_json::Value;
pub fn error_json(e: &RequestError) -> serde_json::Value;
pub fn status_json(s: &StatusSnapshot, home: &Path) -> serde_json::Value;  // D5.4; every key, null when empty
pub fn home_relative(path: &Path, home: &Path) -> String;   // §14.2's `~/…`; a home of `/` rewrites nothing
// The client's typed view: #[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)],
// rename_all = "camelCase"; times are the ISO 8601 strings the daemon wrote, paths `~/…`.
pub struct Hello { pub protocol: u32, pub version: String, pub pid: u32, pub started_at: String }
pub struct Status { pub daemon: StatusDaemon, pub notifications: StatusNotifications, pub providers: Vec<StatusProvider>, pub jobs: Vec<StatusJob>, pub subscribers: usize }
pub struct StatusDaemon { pub pid: u32, pub version: String, pub protocol: u32, pub started_at: String, pub socket: Option<String>, pub log: String }
pub struct StatusNotifications { pub enabled: bool, pub backend: Option<String>, pub events: Vec<String> }
pub struct StatusProvider {
    pub provider: String, pub mode: String, pub autoswitch: bool,
    pub waiting_for: Option<String>, pub holder_pid: Option<u32>, pub backoff: Option<StatusBackoff>,
    pub last_tick_at: Option<String>, pub next_tick_at: Option<String>,
    pub last_outcome: Option<StatusOutcome>, pub last_switch: Option<StatusSwitch>,
}
pub struct StatusBackoff { pub reason: String, pub until: String }
pub struct StatusOutcome { pub kind: String, pub reason: Option<String>, pub at: String }
pub struct StatusSwitch { pub at: String, pub from: u32, pub to: u32, pub trigger: String }
pub struct StatusJob { pub name: String, pub last_run_at: Option<String>, pub next_run_at: Option<String>, pub last_error: Option<String> }
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// One of §11.4's kinds; `raw` is the whole object.
    Auto { kind: String, provider: String, raw: serde_json::Value },
    Usage { provider: String, updated: Vec<u32>, fetch_errors: BTreeMap<u32, String> },
    Changed,
    Mode { provider: String, mode: String, waiting_for: Option<String>, holder_pid: Option<u32>, backoff: Option<StatusBackoff> },
    Stopping { reason: String },
    TokenIndex { provider: String, raw: serde_json::Value },   // Task 9 (gated)
    /// A kind this client does not know, or a known kind whose fields it cannot read.
    Unknown { kind: String, raw: serde_json::Value },
}
pub fn parse_event(v: &serde_json::Value) -> Option<Event>;  // None: not an object with a string `event`

// tagteam-daemon/src/server.rs (Task 2)
pub use tagteam_engine::daemon::MAX_SOCKET_PATH;            // plan A Task 3
pub const MAX_CONNECTIONS: usize = 16;
pub const QUEUE: usize = 256;
pub const IDLE: std::time::Duration = std::time::Duration::from_secs(30);
pub const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits { pub idle: Duration, pub write_timeout: Duration }   // Default: IDLE, WRITE_TIMEOUT
#[derive(Debug, thiserror::Error)]
pub enum BindError { TooLong(PathBuf), Io(#[from] std::io::Error) }
pub struct Server { /* shared: board, hub, home, limits, stop flag, open count, threads; accept thread */ }
impl Server {
    /// D5.1: removes a leftover file, binds 0600, starts the accept thread. The caller holds
    /// `daemon.lock`. `home` is the one `status` writes paths under as `~/…` (D5.4).
    pub fn bind(path: &Path, home: &Path, board: Arc<StatusBoard>, cancel: Cancel) -> Result<Server, BindError>;
    pub fn bind_with(path: &Path, home: &Path, board: Arc<StatusBoard>, cancel: Cancel, limits: Limits) -> Result<Server, BindError>;
    pub fn hub(&self) -> &Hub;
    /// Closes every connection once it has written what it holds, joins the threads, and
    /// removes the socket file if the hub has not, while it is still the one bound.
    pub fn shutdown(self);
}
impl Drop for Server { /* as shutdown */ }
pub struct Hub { /* board, Mutex<Vec<Subscriber>>, the bound file's path, device and inode, closed flag */ } // Send + Sync
impl DaemonSink for Hub {
    /* try_send to each subscriber; drops a full one. On `Stopping`: queues it, then stops the
       accept thread from accepting and removes the socket file if its (dev, inode) still match
       (Decision 9). `notified` and `tick_done` keep the no-op defaults. */
}
pub fn peer_uid(stream: &std::os::unix::net::UnixStream) -> std::io::Result<u32>;

// tagteam-daemon/src/client.rs (Task 3)
pub const GREETING_TIMEOUT: Duration = Duration::from_millis(250);
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(2);
#[derive(Debug)]
pub enum Connect { Connected(Client), NoDaemon, Mismatch { server: u32, client: u32 } }
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)] Io(#[from] std::io::Error),
    #[error("the daemon answered something this client cannot read: {0}")] Protocol(String),
    #[error("the daemon did not answer in time")] Timeout,     // also: accepted, but no greeting in 250 ms
    #[error("the daemon refused the request ({kind}): {message}")] Refused { kind: String, message: String },
}
pub fn connect(env: &Env) -> Result<Connect, ClientError>;            // daemon_socket_path(env)
pub fn connect_at(path: &Path) -> Result<Connect, ClientError>;
#[derive(Debug)]
pub struct Client { /* lines, hello, next id */ }
impl Client {
    pub fn hello(&self) -> &Hello;
    pub fn status(&mut self) -> Result<Status, ClientError>;
    pub fn subscribe(self) -> Result<Events, ClientError>;
}
#[derive(Debug)]
pub struct Events { /* lines, done */ }
impl Iterator for Events { type Item = Result<Event, ClientError>; }

// tagteam-engine/src/service.rs (Task 4)
pub const LAUNCHD_LABEL: &str = "io.github.michaelrigart.tagteam";
pub const SYSTEMD_UNIT: &str = "tagteam.service";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind { Launchd, Systemd }
impl ServiceKind {
    pub fn for_platform(macos: bool) -> Self;
    pub fn as_str(self) -> &'static str;    // "launchd" | "systemd"
    pub fn noun(self) -> &'static str;      // "launchd agent" | "systemd user unit"
    pub fn label(self) -> &'static str;     // LAUNCHD_LABEL | SYSTEMD_UNIT
}
pub fn unit_path(env: &Env, kind: ServiceKind) -> PathBuf;
pub fn launchd_domain() -> String;          // "gui/<uid>"
pub fn launchd_target() -> String;          // "gui/<uid>/<label>"
pub const RECORDED_VARS: &[&str];           // D9.1's list, in this order, both cases of each proxy variable
pub fn recorded_env(var: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> Result<Vec<(String, String)>, UnitError>;
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UnitError {
    #[error("{0} holds a newline, a NUL or another control character, which a unit file cannot carry")] Unrepresentable(String),
    #[error("{0} is not valid UTF-8")] NotUtf8(String),
}
pub fn render_launchd(program: &Path, env: &[(String, String)]) -> Result<String, UnitError>;
pub fn render_systemd(program: &Path, env: &[(String, String)]) -> Result<String, UnitError>;
pub fn render_unit(kind: ServiceKind, program: &Path, env: &[(String, String)]) -> Result<String, UnitError>;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitContents { pub program: PathBuf, pub env: Vec<(String, String)> }
/// Only what render_* writes, byte for byte; None for anything else (Decision 5).
pub fn parse_unit(kind: ServiceKind, contents: &str) -> Option<UnitContents>;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagerRun { Missing, Exited { code: i32, stdout: String, stderr: String }, NoAnswer(String) }
/// `program` found on `env`'s PATH, run through `spawner` under a fresh cancel token (Decision 17).
pub fn run_manager<A: AsRef<OsStr>>(env: &Env, program: &str, args: &[A], spawner: &dyn ProcessSpawner, timeout: Duration) -> ManagerRun;
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServiceState {
    pub installed: bool,
    pub loaded: Option<bool>,
    pub running: Option<bool>,
    pub last_exit_status: Option<i32>,
    /// Why a field is unknown: the manager is missing, hung or failed.
    pub unknown: Option<String>,
}
pub fn service_state(env: &Env, kind: ServiceKind, spawner: &dyn ProcessSpawner, timeout: Duration) -> ServiceState;
/// Linux, D9.3. `Err` says why lingering cannot be told: `loginctl` missing, hung or failing.
pub fn lingering(env: &Env, spawner: &dyn ProcessSpawner, user: &str, timeout: Duration) -> Result<bool, String>;

// tagteam-daemon/src/service.rs (Task 5)
pub const MANAGER_TIMEOUT: Duration = Duration::from_secs(10);   // D9.4
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport { pub kind: ServiceKind, pub path: PathBuf, pub label: String, pub started: bool, pub warnings: Vec<String> }
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)] Unit(#[from] UnitError),
    #[error(transparent)] Io(#[from] std::io::Error),
    #[error("{program} failed (exit {status}): {stderr}")] Manager { program: String, status: String, stderr: String },
    #[error("{program} gave no answer: {reason}")] NoAnswer { program: String, reason: String },
    #[error("{0} was not found; `tagteam daemon install --print` prints the unit to install yourself")] NoManager(String),
}
pub fn install(env: &Env, kind: ServiceKind, program: &Path, recorded: &[(String, String)], start: bool, spawner: &dyn ProcessSpawner) -> Result<InstallReport, InstallError>;
pub fn uninstall(env: &Env, kind: ServiceKind, spawner: &dyn ProcessSpawner) -> Result<bool /* removed */, InstallError>;

// tagteam-engine/src/doctor.rs (Task 6)
pub struct DoctorOptions { pub online: bool, pub provider: Option<ProviderId>, pub service: Option<ServiceKind> }
// Check ids: daemon.running, daemon.service, daemon.socket, daemon.notify (`service: None`
// leaves out daemon.service and daemon.notify). auto.engine says "auto-switch runs in the
// daemon (pid N)" when the engine lock's record matches daemon.lock's.

// tagteam-daemon/src/events.rs and tokens.rs (Task 9, gated)
pub enum DaemonEvent { /* plan A's … */ TokenIndex { provider: ProviderId, state: tagteam_engine::tokens::IndexState } }
pub const SESSIONS_WATCH: &str = "sessions-watch";
pub const TOKEN_INDEX: &str = "token-index";
pub const SESSIONS_EVERY_MS: i64 = 5_000;
pub const INDEX_EVERY_MS: i64 = 60_000;
pub const INDEX_SLICE: Duration = Duration::from_secs(1);
pub fn token_jobs(providers: Vec<ProviderId>, board: Arc<StatusBoard>, schema_newer: bool) -> (SessionsWatch, TokenIndexJob);
pub struct SessionsWatch { /* the shared providers, board and broadcast record */ }   // impl Job
pub struct TokenIndexJob { /* the same, and its refresh slice */ }                     // impl Job
impl TokenIndexJob { pub fn with_slice(self, slice: Duration) -> Self; }
```

```rust
// tagteam (CLI)

// cli.rs (Task 5)
Command::Daemon { #[command(subcommand)] action: Option<DaemonAction> }  // plan A's `Daemon {}`
#[derive(Subcommand)]
pub enum DaemonAction { Install { print: bool, no_start: bool }, Uninstall, Status }  // --print conflicts with --no-start
// app.rs's `stops_on_a_signal` and `dispatch` match the foreground run as
// `Command::Daemon { action: None }`; `command_name` keeps `Command::Daemon { .. }`.

// app.rs
impl Context { /* from_process also captures RECORDED_VARS into Env.vars (Task 6) */ }
impl App {
    fn service(&self) -> ServiceKind;                                  // Task 6, from plan A's `App.platform`
    fn daemon_action(&mut self, action: DaemonAction) -> Result<i32, Failure>;   // Task 5
    fn daemon_install(&mut self, print: bool, no_start: bool) -> Result<i32, Failure>;
    fn daemon_uninstall(&mut self) -> Result<i32, Failure>;
    fn daemon_status(&mut self) -> Result<i32, Failure>;
}
fn install_failure(e: InstallError) -> Failure;                         // Task 5
fn auto_failure(e: AutoError, name: &dyn Fn(&ProviderId) -> String, daemon: &dyn Fn(&ProviderId) -> Option<u32>) -> Failure;  // Task 6
fn daemon_engine(engine: &Engine, provider: &ProviderId) -> Option<u32>;  // Task 6, D8.3
const KIND_SERVICE: &str = "service";                                   // Task 5
const KIND_NO_SERVICE_MANAGER: &str = "no-service-manager";
const KIND_DAEMON: &str = "daemon";

// daemon_cmd.rs (plan A's, extended)
pub(crate) fn bind_socket(env: &Env, board: &Arc<StatusBoard>) -> Option<Server>;            // Task 2
pub(crate) fn serve(started: Started<'_>, sink: &dyn DaemonSink, server: Option<&Server>) -> DaemonExit;  // Task 2
pub(crate) fn test_program(program: PathBuf, var: &dyn Fn(&str) -> Option<OsString>) -> PathBuf;  // Task 7, Decision 7
pub(crate) fn test_upgrade_watch_ms(ms: i64, var: &dyn Fn(&str) -> Option<OsString>) -> i64;       // Task 7, Decision 7

// service_cmd.rs (new, Task 5)
pub(crate) const EXIT_NOT_RUNNING: i32 = 3;
pub(crate) fn install_program(env: &Env) -> std::io::Result<PathBuf>;   // D9.1, via plan A's `program_path`
pub(crate) fn print_json(kind: ServiceKind, path: &Path, unit: &str) -> Value;
pub(crate) fn install_human(r: &InstallReport) -> String;
pub(crate) fn install_json(r: &InstallReport) -> Value;
pub(crate) fn uninstall_human(kind: ServiceKind, path: &Path, removed: bool) -> String;
pub(crate) fn uninstall_json(removed: bool) -> Value;
pub(crate) enum Found { Answered(Box<Status>), Mismatch { server: u32, client: u32, pid: Option<u32> }, Silent { pid: u32 }, NotRunning }
impl Found { pub(crate) fn running(&self) -> bool; }
pub(crate) fn find_daemon(env: &Env) -> Result<Found, ClientError>;     // Decision 14
pub(crate) fn status_json(found: &Found, kind: ServiceKind, path: &Path, state: &ServiceState) -> Value;  // adds top-level `pid`
pub(crate) fn status_human(found: &Found, kind: ServiceKind, path: &Path, state: &ServiceState) -> String;
```

```rust
// Test helpers
// crates/tagteam-daemon/tests/common/mod.rs (Task 2)
pub const HOME: &str = "/Users/tester";
pub const T: i64 = 1_790_000_000;
pub fn short_tempdir() -> tempfile::TempDir;   // under /tmp, chmod 0700
pub fn daemon_info() -> DaemonInfo;
pub fn board() -> Arc<StatusBoard>;
pub fn wait_until(what: &str, ready: impl FnMut() -> bool);
pub struct Peer { /* reader, writer */ }  // connect, greeted, subscribed, send, send_bytes, next_line, line, rest

// crates/tagteam/tests/common/mod.rs
pub fn short_tempdir() -> tempfile::TempDir;   // Task 2; fails when the fixture's socket path would not fit
pub fn login_with_token(env: &Env, kc: &dyn Keychain, email: &str, org: &str, at: &str, rt: &str);  // Task 7; `login` delegates
pub struct Guarded(Option<Running>);  // Task 7: spawn, pid, stdout, stderr, signal, is_running, wait_for, wait_for_output, finish; kills on drop
// (`Running::stderr` and `Running::wait_for_stderr` are plan A's, Task 12)
```

## Task Overview

| # | Task | Depends on |
|---|---|---|
| 1 | Protocol requests, replies, status JSON and the typed client view | plan A |
| 2 | The socket server and hub, wired into `tagteam daemon` | 1 |
| 3 | The client | 1, 2 |
| 4 | Service units in the engine | plan A |
| 5 | `tagteam daemon install`, `uninstall` and `status` | 1, 3, 4, 6 |
| 6 | `doctor`'s daemon checks, and `auto` naming the daemon | 2, 4 |
| 7 | Multi-process and performance tests | 2, 3, 5, 6 |
| 8 | Spec amendments, errata and user docs | 6, 7 |
| 9 | The token report's jobs (gated on its merge) | 8, and the token report merged |
| 10 | Verification and the manual checks | 8 (and 9 when it ran) |

Tasks run one at a time, in this order. Task 5 comes after Task 6, whose `App::service()` and
`RECORDED_VARS` capture it uses; the numbers are kept so that every cross-reference stays
right, and the order of execution is 1, 2, 3, 4, 6, 5, 7, 8, 9, 10.

---

## Tasks

### Task 1: Protocol requests, replies, status JSON and the typed client view

**Depends on:** plan A (Task 6's `protocol.rs` and `events.rs`, Task 8's `status.rs`)

D5.2–D5.5 give protocol 1 its shapes, and Decision 1 says which bad input gets a reply and which
closes the connection. This task adds them to `protocol.rs` as pure functions, with no socket:
- `parse_request`, which classifies one line;
- `hello_json`, `ok_json`, `error_json` and `status_json`, which the server writes (Task 2);
- the typed mirror the client reads: `Hello`, `Status` and its parts, `Event` and
  `parse_event` (Task 3).

**Readings of the spec this task commits to:**
- **"At most 64 KiB per line" leaves out the newline.** A line of exactly `MAX_LINE` bytes is
  read. `parse_request` closes on one byte more, so Task 2's reader, which hands it every whole
  line, closes on a terminated line over the limit wherever its newline arrived, and closes
  before a newline arrives once the pending bytes pass the limit. Task 3's client checks its
  newline's offset the same way.
- **An `id` is a JSON string or a JSON integer that fits `i64`.** `1.0`, `1e2`, `null`, `true`,
  arrays and integers past `i64` are not usable ids. `parse_request` reads ids from a
  `serde_json::Value` and never through `RequestId`'s untagged `Deserialize`. The workspace
  enables serde_json's `arbitrary_precision`, so reading through the `Value` keeps the number
  handling in one place.
- **A missing or unusable `id` is `bad-request` whatever the `op`.** An unknown op gets
  `unknown-op` only when the id is usable, so the reply can carry it (Decision 1).
- **The refusal messages never echo the client's input.** A 64 KiB op name is not sent back.
- **`status_json`'s paths.** Nothing in the engine renders `~/…` paths for reuse. `doctor`
  prints full paths, and the log's `~/` rewrite is private to the CLI (`logging.rs`, `Line`),
  where it works on whole lines. So this task adds `home_relative`, under the log's rule
  (§14.2): a path under `home` is written `~/…`, `home` itself is `~`, and a home of `/`
  rewrites nothing. It compares components, so `/Users/tester2` is not under `/Users/tester`.
- **Times are epoch seconds.** `status_json` and `hello_json` read every `i64` time in
  `StatusSnapshot` as epoch seconds, as plan A's Decision 12 and its `status.rs` fix them, and
  write them with `format_iso8601` (§13.2).
- **The backoff object is written twice, the same way.** Plan A's `daemon_event_json` writes
  the `mode` event's `backoff` inline as `{"reason":"keychain","until":"…Z"}`. This task's
  private `status_backoff_json` writes D5.4's the same way, and
  `every_daemon_kind_reads_back_from_daemon_event_json` reads both back through one type.
- **`status_json` writes every key D5.4 lists, null when empty.** That covers `lastOutcome.reason`
  for `switch` and `collect` too, so a reader never has to tell "absent" from "null".
- **`parse_event` is total over event objects.** A kind it does not know is
  `Event::Unknown`. So is a known kind whose fields it cannot read, which is also
  `Unknown { kind, raw }`: D5.6 has consumers ignore what they cannot read, and the TUI
  re-reads on `Unknown` anyway. Optional fields (D5.5's `?`) read as none whether they are
  absent or `null`. Anything that is not an object with a string `event` is `None`; a reply is
  one such thing.

**Files:**
- Modify: `crates/tagteam-daemon/src/protocol.rs` (new items after plan A's `daemon_event_json`
  and its private `envelope`; a second test module, `socket_protocol_tests`, at the end of the
  file)
- Test: `crates/tagteam-daemon/src/protocol.rs` (`mod socket_protocol_tests`)

Plan A's manifest (Task 6) already lists `serde` (the workspace's, with `derive`) and
`serde_json`, so `Cargo.toml` does not change.

**Interfaces:**
- Consumes (plan A):
  - `tagteam_daemon::protocol::{PROTOCOL, event_json, daemon_event_json}`
  - `tagteam_daemon::events::{Backoff, DaemonEvent, Mode, StopReason, WaitingFor}`, each with
    `as_str()` where plan A gives one
  - `tagteam_daemon::status::{StatusSnapshot, DaemonInfo, NotifyInfo, ProviderStatus,
    LastOutcome, LastSwitch, OutcomeKind, JobStatus}`
  - `tagteam_engine::settings::NotifyEvent` (`as_str()`)
  - `tagteam_core::time::format_iso8601(epoch_s: i64) -> String`
- Produces (`tagteam_daemon::protocol`, as plan B's Interfaces show it):
  - `RequestId`, `Op`, `Request`, `RequestError`, `Parsed`, `MAX_LINE`
  - `pub const BAD_REQUEST: &str = "bad-request"`, `pub const UNKNOWN_OP: &str = "unknown-op"`
  - `parse_request(line: &[u8]) -> Parsed`
  - `hello_json(d: &DaemonInfo) -> Value`
  - `ok_json(id: &RequestId, result: Value) -> Value`
  - `error_json(e: &RequestError) -> Value`
  - `status_json(s: &StatusSnapshot, home: &Path) -> Value`
  - `pub fn home_relative(path: &Path, home: &Path) -> String`
  - `Hello`, `Status`, `StatusDaemon`, `StatusNotifications`, `StatusProvider`, `StatusBackoff`,
    `StatusOutcome`, `StatusSwitch`, `StatusJob`. Each derives
    `Debug, Clone, PartialEq, Eq, serde::Deserialize` with `rename_all = "camelCase"`.
  - `Event` (`Debug, Clone, PartialEq`) and `parse_event(v: &Value) -> Option<Event>`

- [ ] **Step 1: Write the failing tests**

Append this module to the end of `crates/tagteam-daemon/src/protocol.rs`. Leave plan A's own
`mod tests` as it is.

```rust
#[cfg(test)]
mod socket_protocol_tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};
    use tagteam_core::ProviderId;
    use tagteam_core::autoswitch::Trigger;
    use tagteam_core::time::format_iso8601;
    use tagteam_engine::auto::AutoEvent;
    use tagteam_engine::settings::NotifyEvent;

    use super::*;
    use crate::events::{Backoff, DaemonEvent, Mode, StopReason, WaitingFor};
    use crate::status::{
        DaemonInfo, JobStatus, LastOutcome, LastSwitch, NotifyInfo, OutcomeKind, ProviderStatus,
        StatusSnapshot,
    };

    /// 2026-09-21T14:13:20Z.
    const T: i64 = 1_790_000_000;
    const HOME: &str = "/Users/tester";

    fn iso(s: i64) -> String {
        format_iso8601(s)
    }

    /// `parse_request`'s answer without the message: the request, a refusal's id and kind, or
    /// `(None, "close")`.
    type Class = Result<Request, (Option<RequestId>, &'static str)>;

    fn class(line: &[u8]) -> Class {
        match parse_request(line) {
            Parsed::Request(r) => Ok(r),
            Parsed::Refuse(e) => {
                assert!(!e.message.is_empty(), "a refusal says why");
                Err((e.id, e.kind))
            }
            Parsed::Close => Err((None, "close")),
        }
    }

    fn check(line: &[u8], want: Class) {
        assert_eq!(class(line), want, "{}", String::from_utf8_lossy(line));
    }

    #[test]
    fn requests_are_classified_as_decision_1_says() {
        use RequestId::{Int, Str};
        let status = |id| Ok(Request { id, op: Op::Status });
        check(br#"{"id":1,"op":"status"}"#, status(Int(1)));
        check(
            br#"{"id":"tui-7","op":"subscribe"}"#,
            Ok(Request {
                id: Str("tui-7".into()),
                op: Op::Subscribe,
            }),
        );
        // D5.3: unknown fields are ignored, whatever their order.
        check(br#"{"op":"status","id":-3,"since":"later"}"#, status(Int(-3)));
        check(br#"  {"id":2,"op":"status"}  "#, status(Int(2)));
        // No usable id: bad-request, with a null id.
        let no_usable_id: [&[u8]; 9] = [
            br#"{"op":"status"}"#,
            br#"{"id":1.5,"op":"status"}"#,
            br#"{"id":2.0,"op":"status"}"#,
            br#"{"id":1e2,"op":"status"}"#,
            br#"{"id":null,"op":"status"}"#,
            br#"{"id":true,"op":"status"}"#,
            br#"{"id":[1],"op":"status"}"#,
            br#"{"id":9223372036854775808,"op":"status"}"#,
            br#"{"op":"pause"}"#,
        ];
        for line in no_usable_id {
            check(line, Err((None, BAD_REQUEST)));
        }
        // A usable id and no string op: bad-request, with the id.
        check(br#"{"id":7}"#, Err((Some(Int(7)), BAD_REQUEST)));
        check(br#"{"id":7,"op":3}"#, Err((Some(Int(7)), BAD_REQUEST)));
        // An op this daemon does not know: unknown-op, with the id.
        check(
            br#"{"id":"x","op":"pause"}"#,
            Err((Some(Str("x".into())), UNKNOWN_OP)),
        );
        check(br#"{"id":8,"op":"Status"}"#, Err((Some(Int(8)), UNKNOWN_OP)));
        // Framing violations close the connection (D5.2).
        let closing: [&[u8]; 9] = [
            b"[1,2]",
            b"\"status\"",
            b"42",
            b"null",
            b"",
            b"not json",
            br#"{"id":1,"op":"status"} {"id":2}"#,
            b"{\"id\":\"\xff\",\"op\":\"status\"}",
            b"\xfe\xff",
        ];
        for line in closing {
            check(line, Err((None, "close")));
        }
        // The length limit leaves out the newline: a request padded to it is read, one byte
        // more closes.
        let padded = |len: usize| {
            let mut line = br#"{"id":1,"op":"status"}"#.to_vec();
            line.resize(len, b' ');
            line
        };
        check(padded(MAX_LINE).as_slice(), status(Int(1)));
        check(padded(MAX_LINE + 1).as_slice(), Err((None, "close")));
    }

    #[test]
    fn replies_have_d5_3s_shapes() {
        assert_eq!(
            ok_json(&RequestId::Int(7), json!({})).to_string(),
            r#"{"schemaVersion":1,"id":7,"ok":true,"result":{}}"#
        );
        assert_eq!(
            ok_json(&RequestId::Str("a".into()), json!({"x": 1})).to_string(),
            r#"{"schemaVersion":1,"id":"a","ok":true,"result":{"x":1}}"#
        );
        let refused = RequestError {
            id: Some(RequestId::Int(3)),
            kind: UNKNOWN_OP,
            message: "m".into(),
        };
        assert_eq!(
            error_json(&refused).to_string(),
            r#"{"schemaVersion":1,"id":3,"ok":false,"error":{"type":"unknown-op","message":"m"}}"#
        );
        let anonymous = RequestError {
            id: None,
            kind: BAD_REQUEST,
            message: "m".into(),
        };
        assert_eq!(
            error_json(&anonymous).to_string(),
            r#"{"schemaVersion":1,"id":null,"ok":false,"error":{"type":"bad-request","message":"m"}}"#
        );
    }

    #[test]
    fn request_ids_round_trip_through_a_value() {
        // The client reads a reply's id through a `Value` (Task 3), and so does this.
        assert_eq!(serde_json::to_value(RequestId::Int(7)).unwrap(), json!(7));
        assert_eq!(
            serde_json::to_value(RequestId::Str("a".into())).unwrap(),
            json!("a")
        );
        assert_eq!(
            serde_json::from_value::<RequestId>(json!(7)).unwrap(),
            RequestId::Int(7)
        );
        assert_eq!(
            serde_json::from_value::<RequestId>(json!(-7)).unwrap(),
            RequestId::Int(-7)
        );
        assert_eq!(
            serde_json::from_value::<RequestId>(json!("a")).unwrap(),
            RequestId::Str("a".into())
        );
        assert!(serde_json::from_value::<RequestId>(json!(1.5)).is_err());
    }

    fn daemon_info() -> DaemonInfo {
        let home = Path::new(HOME);
        DaemonInfo {
            pid: 4242,
            version: "20261014.2045.0".into(),
            started_at: T,
            socket: Some(home.join(".local/share/tagteam/daemon.sock")),
            log: home.join(".local/state/tagteam/tagteam.log"),
        }
    }

    #[test]
    fn the_greeting_has_d5_2s_shape_and_reads_back_as_hello() {
        let v = hello_json(&daemon_info());
        assert_eq!(
            v.to_string(),
            r#"{"schemaVersion":1,"event":"hello","protocol":1,"version":"20261014.2045.0","pid":4242,"startedAt":"2026-09-21T14:13:20Z"}"#
        );
        assert_eq!(
            serde_json::from_value::<Hello>(v).unwrap(),
            Hello {
                protocol: 1,
                version: "20261014.2045.0".into(),
                pid: 4242,
                started_at: "2026-09-21T14:13:20Z".into(),
            }
        );
    }

    #[test]
    fn paths_under_home_are_written_from_a_tilde() {
        // D5.4: "as in the log" (§14.2).
        let home = Path::new(HOME);
        let rel = |p: &str| home_relative(Path::new(p), home);
        assert_eq!(
            rel("/Users/tester/.local/share/tagteam/daemon.sock"),
            "~/.local/share/tagteam/daemon.sock"
        );
        assert_eq!(rel("/Users/tester"), "~");
        assert_eq!(
            rel("/Users/tester2/x"),
            "/Users/tester2/x",
            "a sibling that shares the prefix is not under home"
        );
        assert_eq!(
            rel("/srv/data/tagteam/daemon.sock"),
            "/srv/data/tagteam/daemon.sock"
        );
        assert_eq!(
            home_relative(Path::new("/Users/tester/x"), Path::new("/Users/tester/")),
            "~/x"
        );
        assert_eq!(
            home_relative(Path::new("/x/y"), Path::new("/")),
            "/x/y",
            "a home of / would rewrite every path, so it rewrites none"
        );
    }

    fn provider(id: &str, mode: Mode) -> ProviderStatus {
        ProviderStatus {
            provider: ProviderId::new(id),
            mode,
            autoswitch: true,
            waiting_for: None,
            holder_pid: None,
            backoff: None,
            last_tick_at: None,
            next_tick_at: None,
            last_outcome: None,
            last_switch: None,
        }
    }

    /// A board with a provider in each of D4.3's modes, one in a keychain backoff.
    fn snapshot() -> StatusSnapshot {
        StatusSnapshot {
            daemon: daemon_info(),
            notifications: NotifyInfo {
                enabled: true,
                backend: Some("osascript"),
                events: vec![NotifyEvent::Switch, NotifyEvent::Quarantine, NotifyEvent::Limit],
            },
            providers: vec![
                ProviderStatus {
                    last_tick_at: Some(T + 60),
                    next_tick_at: Some(T + 360),
                    last_outcome: Some(LastOutcome {
                        kind: OutcomeKind::NoSwitch,
                        reason: Some("below-threshold".into()),
                        at: T + 60,
                    }),
                    last_switch: Some(LastSwitch {
                        at: T + 30,
                        from: 1,
                        to: 2,
                        trigger: "proactive".into(),
                    }),
                    ..provider("claude-code", Mode::Auto)
                },
                ProviderStatus {
                    autoswitch: false,
                    backoff: Some(Backoff { until: T + 120 }),
                    last_tick_at: Some(T + 60),
                    next_tick_at: Some(T + 120),
                    last_outcome: Some(LastOutcome {
                        kind: OutcomeKind::Collect,
                        reason: None,
                        at: T + 60,
                    }),
                    ..provider("fake", Mode::Collect)
                },
                ProviderStatus {
                    waiting_for: Some(WaitingFor::EngineLock),
                    holder_pid: Some(5151),
                    next_tick_at: Some(T + 60),
                    ..provider("third", Mode::Waiting)
                },
                ProviderStatus {
                    waiting_for: Some(WaitingFor::Schema),
                    ..provider("fourth", Mode::Waiting)
                },
                ProviderStatus {
                    last_tick_at: Some(T),
                    next_tick_at: Some(T + 60),
                    last_outcome: Some(LastOutcome {
                        kind: OutcomeKind::Error,
                        reason: Some("the store is locked".into()),
                        at: T,
                    }),
                    ..provider("fifth", Mode::Idle)
                },
            ],
            jobs: vec![
                JobStatus {
                    name: "store-watch".into(),
                    last_run_at: Some(T + 58),
                    next_run_at: Some(T + 60),
                    last_error: None,
                },
                JobStatus {
                    name: "upgrade-watch".into(),
                    last_run_at: Some(T),
                    next_run_at: Some(T + 60),
                    last_error: Some("the program path resolves to nothing".into()),
                },
            ],
            subscribers: 2,
        }
    }

    #[test]
    fn status_json_matches_d5_4_for_a_provider_in_each_mode() {
        let want = json!({
            "daemon": {
                "pid": 4242, "version": "20261014.2045.0", "protocol": 1,
                "startedAt": "2026-09-21T14:13:20Z",
                "socket": "~/.local/share/tagteam/daemon.sock",
                "log": "~/.local/state/tagteam/tagteam.log",
            },
            "notifications": {
                "enabled": true, "backend": "osascript",
                "events": ["switch", "quarantine", "limit"],
            },
            "providers": [
                {
                    "provider": "claude-code", "mode": "auto", "autoswitch": true,
                    "waitingFor": null, "holderPid": null, "backoff": null,
                    "lastTickAt": "2026-09-21T14:14:20Z", "nextTickAt": "2026-09-21T14:19:20Z",
                    "lastOutcome": {"kind": "no-switch", "reason": "below-threshold", "at": "2026-09-21T14:14:20Z"},
                    "lastSwitch": {"at": "2026-09-21T14:13:50Z", "from": 1, "to": 2, "trigger": "proactive"},
                },
                {
                    "provider": "fake", "mode": "collect", "autoswitch": false,
                    "waitingFor": null, "holderPid": null,
                    "backoff": {"reason": "keychain", "until": "2026-09-21T14:15:20Z"},
                    "lastTickAt": "2026-09-21T14:14:20Z", "nextTickAt": "2026-09-21T14:15:20Z",
                    "lastOutcome": {"kind": "collect", "reason": null, "at": "2026-09-21T14:14:20Z"},
                    "lastSwitch": null,
                },
                {
                    "provider": "third", "mode": "waiting", "autoswitch": true,
                    "waitingFor": "engine-lock", "holderPid": 5151, "backoff": null,
                    "lastTickAt": null, "nextTickAt": "2026-09-21T14:14:20Z",
                    "lastOutcome": null, "lastSwitch": null,
                },
                {
                    "provider": "fourth", "mode": "waiting", "autoswitch": true,
                    "waitingFor": "schema", "holderPid": null, "backoff": null,
                    "lastTickAt": null, "nextTickAt": null,
                    "lastOutcome": null, "lastSwitch": null,
                },
                {
                    "provider": "fifth", "mode": "idle", "autoswitch": true,
                    "waitingFor": null, "holderPid": null, "backoff": null,
                    "lastTickAt": "2026-09-21T14:13:20Z", "nextTickAt": "2026-09-21T14:14:20Z",
                    "lastOutcome": {"kind": "error", "reason": "the store is locked", "at": "2026-09-21T14:13:20Z"},
                    "lastSwitch": null,
                },
            ],
            "jobs": [
                {"name": "store-watch", "lastRunAt": "2026-09-21T14:14:18Z",
                 "nextRunAt": "2026-09-21T14:14:20Z", "lastError": null},
                {"name": "upgrade-watch", "lastRunAt": "2026-09-21T14:13:20Z",
                 "nextRunAt": "2026-09-21T14:14:20Z", "lastError": "the program path resolves to nothing"},
            ],
            "subscribers": 2,
        });
        // Compared as text, so the key order is pinned too (`preserve_order`).
        assert_eq!(
            serde_json::to_string_pretty(&status_json(&snapshot(), Path::new(HOME))).unwrap(),
            serde_json::to_string_pretty(&want).unwrap()
        );
        // No socket (D5.1's bind failure), and a data directory outside home.
        let mut s = snapshot();
        s.daemon.socket = None;
        assert_eq!(status_json(&s, Path::new(HOME))["daemon"]["socket"], Value::Null);
        s.daemon.socket = Some(PathBuf::from("/srv/data/tagteam/daemon.sock"));
        assert_eq!(
            status_json(&s, Path::new(HOME))["daemon"]["socket"],
            "/srv/data/tagteam/daemon.sock"
        );
    }

    #[test]
    fn status_json_reads_back_as_the_typed_status() {
        let v = status_json(&snapshot(), Path::new(HOME));
        let status: Status = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(
            status.daemon,
            StatusDaemon {
                pid: 4242,
                version: "20261014.2045.0".into(),
                protocol: 1,
                started_at: iso(T),
                socket: Some("~/.local/share/tagteam/daemon.sock".into()),
                log: "~/.local/state/tagteam/tagteam.log".into(),
            }
        );
        assert_eq!(
            status.notifications,
            StatusNotifications {
                enabled: true,
                backend: Some("osascript".into()),
                events: vec!["switch".into(), "quarantine".into(), "limit".into()],
            }
        );
        assert_eq!(
            status.providers[0],
            StatusProvider {
                provider: "claude-code".into(),
                mode: "auto".into(),
                autoswitch: true,
                waiting_for: None,
                holder_pid: None,
                backoff: None,
                last_tick_at: Some(iso(T + 60)),
                next_tick_at: Some(iso(T + 360)),
                last_outcome: Some(StatusOutcome {
                    kind: "no-switch".into(),
                    reason: Some("below-threshold".into()),
                    at: iso(T + 60),
                }),
                last_switch: Some(StatusSwitch {
                    at: iso(T + 30),
                    from: 1,
                    to: 2,
                    trigger: "proactive".into(),
                }),
            }
        );
        assert_eq!(
            status.providers[1].backoff,
            Some(StatusBackoff {
                reason: "keychain".into(),
                until: iso(T + 120),
            })
        );
        assert_eq!(status.providers[2].waiting_for.as_deref(), Some("engine-lock"));
        assert_eq!(status.providers[2].holder_pid, Some(5151));
        assert_eq!(status.providers[3].waiting_for.as_deref(), Some("schema"));
        assert_eq!(status.providers[4].mode, "idle");
        assert_eq!(
            status.jobs[1],
            StatusJob {
                name: "upgrade-watch".into(),
                last_run_at: Some(iso(T)),
                next_run_at: Some(iso(T + 60)),
                last_error: Some("the program path resolves to nothing".into()),
            }
        );
        assert_eq!(status.subscribers, 2);
        // D5.6: a field a later daemon adds is ignored.
        let mut later = v;
        later["daemon"]["uptime"] = json!(5);
        later["providers"][0]["queue"] = json!([]);
        assert_eq!(serde_json::from_value::<Status>(later).unwrap(), status);
    }

    #[test]
    fn every_daemon_kind_reads_back_from_daemon_event_json() {
        // D5.5's daemon kinds, through the function the hub writes them with (Task 2).
        let cc = || ProviderId::new("claude-code");
        let errors = BTreeMap::from([(2, "rate-limited".to_owned())]);
        let cases = vec![
            (
                DaemonEvent::Usage {
                    provider: cc(),
                    updated: vec![1, 3],
                    fetch_errors: errors.clone(),
                },
                Event::Usage {
                    provider: "claude-code".into(),
                    updated: vec![1, 3],
                    fetch_errors: errors,
                },
            ),
            (
                DaemonEvent::Usage {
                    provider: cc(),
                    updated: vec![],
                    fetch_errors: BTreeMap::new(),
                },
                Event::Usage {
                    provider: "claude-code".into(),
                    updated: vec![],
                    fetch_errors: BTreeMap::new(),
                },
            ),
            (DaemonEvent::Changed, Event::Changed),
            (
                DaemonEvent::Mode {
                    provider: cc(),
                    mode: Mode::Waiting,
                    waiting_for: Some(WaitingFor::EngineLock),
                    holder_pid: Some(4242),
                    backoff: None,
                },
                Event::Mode {
                    provider: "claude-code".into(),
                    mode: "waiting".into(),
                    waiting_for: Some("engine-lock".into()),
                    holder_pid: Some(4242),
                    backoff: None,
                },
            ),
            (
                DaemonEvent::Mode {
                    provider: cc(),
                    mode: Mode::Collect,
                    waiting_for: None,
                    holder_pid: None,
                    backoff: Some(Backoff { until: T + 60 }),
                },
                Event::Mode {
                    provider: "claude-code".into(),
                    mode: "collect".into(),
                    waiting_for: None,
                    holder_pid: None,
                    backoff: Some(StatusBackoff {
                        reason: "keychain".into(),
                        until: iso(T + 60),
                    }),
                },
            ),
            (
                DaemonEvent::Stopping {
                    reason: StopReason::Upgrade,
                },
                Event::Stopping {
                    reason: "upgrade".into(),
                },
            ),
            (
                DaemonEvent::Stopping {
                    reason: StopReason::Schema,
                },
                Event::Stopping {
                    reason: "schema".into(),
                },
            ),
        ];
        for (sent, want) in cases {
            let v = daemon_event_json(&sent, T);
            assert_eq!(parse_event(&v), Some(want), "{v}");
        }
    }

    #[test]
    fn auto_kinds_read_back_as_auto_with_their_whole_object() {
        // D5.5: the auto kinds are `auto --json`'s objects, and the client keeps each whole.
        let cc = ProviderId::new("claude-code");
        let switch = AutoEvent::Switch {
            provider: cc.clone(),
            trigger: Trigger::Proactive,
            from: 1,
            to: 2,
            warnings: vec![],
            dry_run: false,
        };
        let quarantined = AutoEvent::AccountQuarantined {
            provider: cc,
            number: 2,
            email: "b@example.com".into(),
            reason: "refresh-revoked".into(),
        };
        for (e, kind) in [(switch, "switch"), (quarantined, "account-quarantined")] {
            let v = event_json(&e, T);
            let want = Event::Auto {
                kind: kind.into(),
                provider: "claude-code".into(),
                raw: v.clone(),
            };
            assert_eq!(parse_event(&v), Some(want.clone()));
            assert_eq!(
                parse_event(&daemon_event_json(&DaemonEvent::Auto(e), T)),
                Some(want),
                "the socket writes them as auto --json does"
            );
        }
    }

    #[test]
    fn unknown_and_unreadable_events() {
        // D5.6: a kind this client does not know, and a known kind whose fields it cannot
        // read, are `Unknown`, whole; anything that is not an event object is no event.
        let token = json!({"schemaVersion":1,"event":"token-index","ts":"2026-09-21T14:13:20Z","files":3});
        assert_eq!(
            parse_event(&token),
            Some(Event::Unknown {
                kind: "token-index".into(),
                raw: token.clone(),
            })
        );
        for raw in [
            json!({"schemaVersion":1,"event":"mode","ts":"x","mode":"auto"}),
            json!({"schemaVersion":1,"event":"mode","ts":"x","provider":"p","mode":"auto","holderPid":-1}),
            json!({"schemaVersion":1,"event":"usage","ts":"x","provider":"p","updated":[-1]}),
            json!({"schemaVersion":1,"event":"usage","ts":"x","provider":"p","fetchErrors":{"two":"x"}}),
            json!({"schemaVersion":1,"event":"stopping","ts":"x"}),
            json!({"schemaVersion":1,"event":"switch","ts":"x"}),
        ] {
            let kind = raw["event"].as_str().unwrap().to_owned();
            assert_eq!(
                parse_event(&raw),
                Some(Event::Unknown {
                    kind,
                    raw: raw.clone(),
                }),
                "{raw}"
            );
        }
        // D5.5's optional fields read as none, absent or null.
        let bare = json!({"schemaVersion":1,"event":"mode","ts":"x","provider":"p","mode":"auto"});
        assert_eq!(
            parse_event(&bare),
            Some(Event::Mode {
                provider: "p".into(),
                mode: "auto".into(),
                waiting_for: None,
                holder_pid: None,
                backoff: None,
            })
        );
        let nulls = json!({"schemaVersion":1,"event":"mode","ts":"x","provider":"p","mode":"auto",
            "waitingFor":null,"holderPid":null,"backoff":null});
        assert_eq!(parse_event(&nulls), parse_event(&bare));
        let usage = json!({"schemaVersion":1,"event":"usage","ts":"x","provider":"p"});
        assert_eq!(
            parse_event(&usage),
            Some(Event::Usage {
                provider: "p".into(),
                updated: vec![],
                fetch_errors: BTreeMap::new(),
            })
        );
        for not_an_event in [
            json!([1]),
            json!("changed"),
            json!({"schemaVersion":1,"id":1,"ok":true,"result":{}}),
            json!({"event":7}),
        ] {
            assert_eq!(parse_event(&not_an_event), None, "{not_an_event}");
        }
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-daemon --lib protocol::socket_protocol_tests`
Expected: the build fails. The tests name items that do not exist yet:
- `error[E0425]: cannot find function \`parse_request\``, and the same for `hello_json`,
  `ok_json`, `error_json`, `status_json`, `home_relative` and `parse_event`;
- `error[E0412]`/`E0433` for `Request`, `RequestId`, `Op`, `Parsed`, `RequestError`, `Hello`,
  `Status`, `StatusDaemon` and the other status types, and `Event`;
- `cannot find value` for `MAX_LINE`, `BAD_REQUEST` and `UNKNOWN_OP`.

- [ ] **Step 3: Implement**

Plan A's `protocol.rs` (Task 6) opens with these imports:

```rust
use std::collections::BTreeMap;

use serde_json::{Value, json};
use tagteam_core::ProviderId;
use tagteam_core::time::format_iso8601;
use tagteam_engine::auto::AutoEvent;

use crate::events::DaemonEvent;
```

Replace them with:

```rust
use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};
use tagteam_core::ProviderId;
use tagteam_core::time::format_iso8601;
use tagteam_engine::auto::AutoEvent;

use crate::events::{Backoff, DaemonEvent};
use crate::status::{DaemonInfo, JobStatus, ProviderStatus, StatusSnapshot};
```

Then add these items after plan A's private `envelope` (the last function before the test
modules). Plan A's `daemon_event_json` writes the `mode` event's backoff inline, with no
helper, so `status_backoff_json` below is the only named one. The two write the same object,
which `every_daemon_kind_reads_back_from_daemon_event_json` checks.

```rust
// Protocol 1's requests, replies and status (D5.2–D5.4), and their typed mirror for the
// client (D5.7).

/// D5.2: the longest line either side sends, its newline left out.
pub const MAX_LINE: usize = 64 * 1024;
/// D5.3's `error.type` for a request without a usable `id`, or without a string `op`.
pub const BAD_REQUEST: &str = "bad-request";
/// D5.3's `error.type` for an `op` this daemon does not know.
pub const UNKNOWN_OP: &str = "unknown-op";

/// D5.3: a request's `id`, which its reply carries back.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Int(i64),
    Str(String),
}

/// D5.3's ops. Protocol 1 is read-only (D1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Status,
    Subscribe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: RequestId,
    pub op: Op,
}

/// A request answered with `ok: false` (D5.3); the connection stays open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestError {
    /// The request's id when it had a usable one; the reply's `id` is null otherwise.
    pub id: Option<RequestId>,
    /// `BAD_REQUEST` or `UNKNOWN_OP`.
    pub kind: &'static str,
    pub message: String,
}

/// What a client's line comes to (Decision 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Request(Request),
    Refuse(RequestError),
    /// A framing violation (D5.2): the connection is closed, with no reply.
    Close,
}

/// Decision 1: one line from a client, its newline left out. A line over `MAX_LINE`, one that
/// is not UTF-8, and JSON that is not an object close the connection. An object without a
/// usable `id` (a string, or an integer that fits `i64`) or without a string `op` is a
/// `bad-request`, which carries the id when it was usable. An op this daemon does not know is
/// `unknown-op`. Unknown fields are ignored (D5.3). No message repeats the client's input.
pub fn parse_request(line: &[u8]) -> Parsed {
    if line.len() > MAX_LINE {
        return Parsed::Close;
    }
    let Ok(text) = std::str::from_utf8(line) else {
        return Parsed::Close;
    };
    let Ok(Value::Object(request)) = serde_json::from_str::<Value>(text) else {
        return Parsed::Close;
    };
    let refuse = |id: Option<RequestId>, kind: &'static str, message: &str| {
        Parsed::Refuse(RequestError {
            id,
            kind,
            message: message.to_owned(),
        })
    };
    let id = match request.get("id") {
        Some(Value::String(s)) => Some(RequestId::Str(s.clone())),
        Some(Value::Number(n)) => n.as_i64().map(RequestId::Int),
        _ => None,
    };
    let Some(id) = id else {
        return refuse(
            None,
            BAD_REQUEST,
            "a request needs an id that is a string or an integer",
        );
    };
    match request.get("op").and_then(Value::as_str) {
        Some("status") => Parsed::Request(Request { id, op: Op::Status }),
        Some("subscribe") => Parsed::Request(Request {
            id,
            op: Op::Subscribe,
        }),
        Some(_) => refuse(
            Some(id),
            UNKNOWN_OP,
            "this daemon knows the ops status and subscribe",
        ),
        None => refuse(Some(id), BAD_REQUEST, "a request needs an op that is a string"),
    }
}

/// D5.2: the greeting, which the server writes first.
pub fn hello_json(d: &DaemonInfo) -> Value {
    json!({
        "schemaVersion": 1,
        "event": "hello",
        "protocol": PROTOCOL,
        "version": d.version,
        "pid": d.pid,
        "startedAt": format_iso8601(d.started_at),
    })
}

/// D5.3: a request's success.
pub fn ok_json(id: &RequestId, result: Value) -> Value {
    json!({"schemaVersion": 1, "id": id, "ok": true, "result": result})
}

/// D5.3: a request's failure; `id` is null when the request had no usable one.
pub fn error_json(e: &RequestError) -> Value {
    json!({
        "schemaVersion": 1,
        "id": e.id,
        "ok": false,
        "error": {"type": e.kind, "message": e.message},
    })
}

/// D5.4, §14.2: `path` as the log writes it, `~/…` under `home` and `~` for `home` itself. A
/// home of `/` rewrites nothing, since every path would be under it.
pub fn home_relative(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if home.parent().is_some() => {
            if rest.as_os_str().is_empty() {
                "~".to_owned()
            } else {
                format!("~/{}", rest.display())
            }
        }
        _ => path.display().to_string(),
    }
}

/// D5.4's `status` result, built from the board's snapshot (Decision 12 of plan A), so it never
/// touches the store or waits for a job. Every key is written, null when empty.
pub fn status_json(s: &StatusSnapshot, home: &Path) -> Value {
    let d = &s.daemon;
    json!({
        "daemon": {
            "pid": d.pid,
            "version": d.version,
            "protocol": PROTOCOL,
            "startedAt": format_iso8601(d.started_at),
            "socket": d.socket.as_deref().map(|p| home_relative(p, home)),
            "log": home_relative(&d.log, home),
        },
        "notifications": {
            "enabled": s.notifications.enabled,
            "backend": s.notifications.backend,
            "events": s.notifications.events.iter().map(|e| e.as_str()).collect::<Vec<_>>(),
        },
        "providers": s.providers.iter().map(status_provider_json).collect::<Vec<_>>(),
        "jobs": s.jobs.iter().map(status_job_json).collect::<Vec<_>>(),
        "subscribers": s.subscribers,
    })
}

fn status_provider_json(p: &ProviderStatus) -> Value {
    json!({
        "provider": p.provider.as_str(),
        "mode": p.mode.as_str(),
        "autoswitch": p.autoswitch,
        "waitingFor": p.waiting_for.map(|w| w.as_str()),
        "holderPid": p.holder_pid,
        "backoff": p.backoff.map(status_backoff_json),
        "lastTickAt": p.last_tick_at.map(format_iso8601),
        "nextTickAt": p.next_tick_at.map(format_iso8601),
        "lastOutcome": p.last_outcome.as_ref().map(|o| json!({
            "kind": o.kind.as_str(),
            "reason": o.reason,
            "at": format_iso8601(o.at),
        })),
        "lastSwitch": p.last_switch.as_ref().map(|w| json!({
            "at": format_iso8601(w.at),
            "from": w.from,
            "to": w.to,
            "trigger": w.trigger,
        })),
    })
}

/// D5.4's `backoff`: the keychain backoff (D4.1) is the only one there is.
fn status_backoff_json(b: Backoff) -> Value {
    json!({"reason": "keychain", "until": format_iso8601(b.until)})
}

fn status_job_json(j: &JobStatus) -> Value {
    json!({
        "name": j.name,
        "lastRunAt": j.last_run_at.map(format_iso8601),
        "nextRunAt": j.next_run_at.map(format_iso8601),
        "lastError": j.last_error,
    })
}

/// D5.2's greeting, as a client reads it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub protocol: u32,
    pub version: String,
    pub pid: u32,
    pub started_at: String,
}

/// D5.4's `status` result, as a client reads it. Times are ISO 8601 strings and paths are
/// written `~/…`, as the daemon wrote them. Fields a later daemon adds are ignored (D5.6).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub daemon: StatusDaemon,
    pub notifications: StatusNotifications,
    pub providers: Vec<StatusProvider>,
    pub jobs: Vec<StatusJob>,
    pub subscribers: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusDaemon {
    pub pid: u32,
    pub version: String,
    pub protocol: u32,
    pub started_at: String,
    /// None when the daemon runs without its socket (D5.1).
    pub socket: Option<String>,
    pub log: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusNotifications {
    pub enabled: bool,
    /// `osascript`, `notify-send`, or none (D6).
    pub backend: Option<String>,
    pub events: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusProvider {
    pub provider: String,
    /// D4.3's mode: `auto`, `collect`, `waiting` or `idle`.
    pub mode: String,
    pub autoswitch: bool,
    /// `engine-lock` or `schema`.
    pub waiting_for: Option<String>,
    pub holder_pid: Option<u32>,
    pub backoff: Option<StatusBackoff>,
    pub last_tick_at: Option<String>,
    pub next_tick_at: Option<String>,
    pub last_outcome: Option<StatusOutcome>,
    pub last_switch: Option<StatusSwitch>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusBackoff {
    /// `keychain` (D4.1).
    pub reason: String,
    pub until: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusOutcome {
    /// `switch`, `no-switch`, `collect` or `error`.
    pub kind: String,
    pub reason: Option<String>,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusSwitch {
    pub at: String,
    pub from: u32,
    pub to: u32,
    pub trigger: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusJob {
    pub name: String,
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
    pub last_error: Option<String>,
}

/// An event as a subscriber reads it (D5.5, D5.7).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// One of §11.4's kinds; `raw` is the whole object, as `auto --json` prints it.
    Auto {
        kind: String,
        provider: String,
        raw: Value,
    },
    /// Positions whose reading a `collect` round recorded, and each failed fetch's error kind.
    Usage {
        provider: String,
        updated: Vec<u32>,
        fetch_errors: BTreeMap<u32, String>,
    },
    /// Another process committed to the store (D4.4).
    Changed,
    Mode {
        provider: String,
        mode: String,
        waiting_for: Option<String>,
        holder_pid: Option<u32>,
        backoff: Option<StatusBackoff>,
    },
    /// The last event before the daemon closes: `signal`, `upgrade` or `schema`.
    Stopping { reason: String },
    /// A kind this client does not know, or a known kind whose fields it cannot read (D5.6).
    Unknown { kind: String, raw: Value },
}

/// §11.4's kinds: what `auto --json` prints, carried unchanged on the socket (D5.5).
const AUTO_EVENT_KINDS: [&str; 9] = [
    "poll",
    "switch",
    "no-switch",
    "account-quarantined",
    "account-unquarantined",
    "all-exhausted",
    "sleep",
    "error",
    "config-warning",
];

/// D5.7: a line's event, typed. `None` for anything that is not an object with a string
/// `event`; a reply is one such thing.
pub fn parse_event(v: &Value) -> Option<Event> {
    let o = v.as_object()?;
    let kind = o.get("event")?.as_str()?;
    Some(
        typed_event(kind, o, v).unwrap_or_else(|| Event::Unknown {
            kind: kind.to_owned(),
            raw: v.clone(),
        }),
    )
}

/// `kind`'s typed event, or `None` when the kind is not one this client knows or its fields
/// are not what D5.5 gives it.
fn typed_event(kind: &str, o: &Map<String, Value>, raw: &Value) -> Option<Event> {
    let provider = || o.get("provider")?.as_str().map(str::to_owned);
    Some(match kind {
        _ if AUTO_EVENT_KINDS.contains(&kind) => Event::Auto {
            kind: kind.to_owned(),
            provider: provider()?,
            raw: raw.clone(),
        },
        "usage" => Event::Usage {
            provider: provider()?,
            updated: read_positions(o.get("updated"))?,
            fetch_errors: read_by_position(o.get("fetchErrors"))?,
        },
        "changed" => Event::Changed,
        "mode" => Event::Mode {
            provider: provider()?,
            mode: o.get("mode")?.as_str()?.to_owned(),
            waiting_for: nullable_str(o.get("waitingFor"))?,
            holder_pid: nullable_u32(o.get("holderPid"))?,
            backoff: match o.get("backoff") {
                None | Some(Value::Null) => None,
                Some(b) => Some(serde_json::from_value(b.clone()).ok()?),
            },
        },
        "stopping" => Event::Stopping {
            reason: o.get("reason")?.as_str()?.to_owned(),
        },
        _ => return None,
    })
}

/// An optional string field: `Some(None)` when absent or null, `None` when it holds anything
/// but a string.
fn nullable_str(v: Option<&Value>) -> Option<Option<String>> {
    match v {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(s)) => Some(Some(s.clone())),
        Some(_) => None,
    }
}

/// An optional `u32` field, read as `nullable_str` reads a string.
fn nullable_u32(v: Option<&Value>) -> Option<Option<u32>> {
    match v {
        None | Some(Value::Null) => Some(None),
        Some(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()).map(Some),
    }
}

/// `usage`'s `updated`: account positions, none when absent or null.
fn read_positions(v: Option<&Value>) -> Option<Vec<u32>> {
    match v {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|n| n.as_u64().and_then(|n| u32::try_from(n).ok()))
            .collect(),
        Some(_) => None,
    }
}

/// `usage`'s `fetchErrors`: error kinds keyed by position, as §11.4's objects key accounts.
fn read_by_position(v: Option<&Value>) -> Option<BTreeMap<u32, String>> {
    match v {
        None | Some(Value::Null) => Some(BTreeMap::new()),
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| Some((k.parse().ok()?, v.as_str()?.to_owned())))
            .collect(),
        Some(_) => None,
    }
}
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-daemon --lib protocol`
Expected: PASS. The 10 new tests pass, and plan A's four protocol tests still pass. Plan A's
`daemon_event_json` leaves optional fields out, writes `backoff` as
`{"reason":"keychain","until":"…Z"}`, keys `fetchErrors` by position strings (`by_position`),
and gives `changed` and `stopping` no `provider`. `parse_event` reads exactly those shapes.
Should `every_daemon_kind_reads_back_from_daemon_event_json` fail, the defect is in whichever
side left D5.4's shapes; never change the test.

Then `cargo fmt --all`, `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`.
Expected: clean.

- [ ] **Step 5: Commit** (the controller commits; implementers do not)

```
git add crates/tagteam-daemon/src/protocol.rs
git commit -m "Add protocol 1's requests, replies, status JSON and typed client view"
```

---

### Task 2: The socket server and hub, wired into `tagteam daemon`

**Depends on:** 1; plan A (Task 3's `MAX_SOCKET_PATH` and `daemon_socket_path`, Task 12's
`Daemon::start`, `Started` and `App::daemon`)

D5.1 binds the socket 0600 once `daemon.lock` is held. D4.1 step 5 binds it before the
scheduler starts, and D4.6 removes it when `stopping` goes out, before `daemon.lock` is
released. Decisions 2, 3 and 9 settle the threads, the peer check and that order. This task
builds `server.rs` (the bind, the accept thread, one thread per connection, and the `Hub` that
the scheduler's events go through) and wires it into `tagteam daemon` between `Daemon::start`
and `Started::serve`.

**Readings and choices this task commits to:**
- **The umask window.** `bind` creates the socket under umask `0177` (D5.1). The umask belongs
  to the process, so D5.1 relies on nothing else creating a file or starting a process during
  the bind. That holds when `tagteam daemon` binds:
  - the scheduler has not started;
  - plan A's notifier thread is idle, since only jobs queue notices;
  - the only other threads production code leaves are detached pipe drainers
    (`process.rs`, `drain`) and a resolver thread (`net.rs`), and they only read.

  Signal handlers (`signal-hook`) and `tracing` start no thread. Within one process, a `static`
  mutex makes binds take turns, so one bind never restores a mask that another narrowed. The
  parent directory is created inside the same turn, before the mask is narrowed. A test proves
  the mask is restored, in a test binary of its own (`tests/umask.rs`).
- **Where the tests live.** All socket tests are integration tests (`tests/server.rs`,
  `tests/client.rs` in Task 3, `tests/umask.rs`), not unit tests in `src/`, for two reasons:
  - The umask is process-wide, and plan A's unit tests in the crate's lib test binary create
    directories concurrently. A directory made in another test's window would come out `0600`
    and unusable.
  - In the integration binaries, `short_tempdir()` chmods its directory to `0700` after
    creating it, which makes it safe there.
- **Accepting.** The listener is non-blocking, and the accept thread waits in `poll(2)` for at
  most 100 ms between looks at the cancel token and the stop flag. A connection is therefore
  greeted at once, which the client's 250 ms bound on the greeting needs. A loop that slept
  100 ms between `accept`s would spend 40% of that bound. When the accept thread returns, it
  drops the listener, so a later connect is refused (`ECONNREFUSED`): Review Focus 1's
  `NoDaemon`, never a client left waiting for a greeting. It returns on a signal, on
  `stopping`, or on `shutdown`.
- **Connection threads stop on `shutdown`, not on the signal.** On SIGTERM, `serve` emits
  `stopping` and only then returns (D4.6). A connection thread that quit on the signal itself
  would never write that event. So connection threads ignore the cancel token. While the
  scheduler winds down, an open connection keeps answering `status`. Each thread reads the stop
  flag before it writes out its queue, so whatever was queued before `shutdown` (the `stopping`
  event among it) is written before the connection closes.
- **A subscriber that falls behind.** The hub's `emit` only `try_send`s. When a queue is full,
  the hub drops that subscriber and shuts its socket down (`shutdown(2)` on a clone of the
  stream). This wakes its thread even when the thread is blocked writing to a client that
  reads nothing.
  - A write may block for at most `WRITE_TIMEOUT` (2 s). This bounds how long `shutdown` waits
    for a client that has stopped reading.
  - Any failed write closes the connection: a line cut short cannot be continued.
- **Throughput.** Decision 2 drains each queue between reads, at least every 100 ms. A burst of
  more than `QUEUE` events within one such window would drop every subscriber. A tick emits a
  handful of events.
  - Review Focus 2's test therefore emits in batches of 32, each waiting until the healthy
    readers have caught up.
  - It goes on until the stalled subscriber is dropped, at least `QUEUE + 50` events. A
    Linux socket buffer (about 200 KiB by default) holds about a hundred of its 2 KiB events
    before that subscriber's thread blocks.
- **Ordering on one connection (D5.3).** Before a reply is written, every event queued ahead of
  it is written. The `ok` to a first `subscribe` comes before any event (D5.5). A second
  `subscribe` on the same connection is answered `ok` and changes nothing.
- **The socket file goes when `stopping` does** (D4.6, Decision 9). Plan A's `Started::serve`
  emits `stopping` to its sink before it drops the jobs, the notifier and, last,
  `daemon.lock`. When the hub receives it, it queues it for every subscriber like any other
  event, then closes the socket: the accept thread stops accepting, and the file is removed
  while its device and inode are still the ones `bind` created. `daemon.lock` is still held
  then, so no successor can have bound a socket of its own at the path. Subscribers still read
  the `stopping` line, which their connection threads write before `shutdown` closes them.
- **`shutdown` removes only the file it bound, and only once.** It runs after `serve` has
  returned, and so after `daemon.lock` was released: a daemon that was waiting may have bound
  the path already. `shutdown` (and `Drop`) still does the hub's removal, for a server whose
  hub never saw `stopping`, guarded by the same device and inode. The removal runs at most
  once, so after `stopping` it does nothing.
- **`bind` takes the home** that `status` writes paths under (D5.4) as its second parameter.
  Plan A's `DaemonInfo` has no home, and the socket and log can sit outside it under `XDG_*`,
  so nothing else the server holds can supply it.
- **`sun_path`.** `MAX_SOCKET_PATH` is plan A's (`tagteam_engine::daemon`, Task 3): 103 bytes
  on macOS and 107 on Linux, `sun_path` less its NUL. `server` re-exports it. It is std's own
  limit, checked first so that it reads as `TooLong` rather than a generic `InvalidInput`.
  Nothing is created for a path that is too long.
- **The peer check** (Decision 3) runs in the accept thread, before the connection limit and
  before any thread is started. A connection of another user cannot be made in a test without
  root, so `peer_uid` is tested on a socket pair. The comparison is a single `==` in `admit`.
- **Logging (D11).** Connections opened and closed log at DEBUG with why they closed, and so does
  a subscriber that falls behind. `daemon_cmd` logs a failed bind at ERROR (D5.1).

**Files:**
- Create: `crates/tagteam-daemon/src/server.rs`
- Modify: `crates/tagteam-daemon/src/lib.rs` (add `pub mod server;` after `pub mod scheduler;`)
- Create: `crates/tagteam-daemon/tests/common/mod.rs` (`short_tempdir`, `board`, `daemon_info`,
  `HOME`, `T`, `Peer`, `wait_until`). Plan A creates no `tests/` directory in this crate: its
  tests are unit tests over `src/testutil.rs`, which integration tests cannot reach.
- Create: `crates/tagteam-daemon/tests/server.rs`
- Create: `crates/tagteam-daemon/tests/umask.rs`
- Modify: `crates/tagteam/src/daemon_cmd.rs` (imports; `bind_socket` and `serve`; a test
  module, `socket_tests`)
- Modify: `crates/tagteam/src/app.rs` (`App::daemon`: bind the socket between `Daemon::start`
  and serving, serve through `daemon_cmd::serve`, shut the server down after)
- Modify: `crates/tagteam/tests/common/mod.rs` (an import; `short_tempdir`, after
  `now_epoch_s`)
- Test: `crates/tagteam/tests/daemon_cli.rs` (plan A's binary tests of `tagteam daemon`: its
  imports, two helpers and a test)

Plan A's manifest (Task 6) already has `libc`, `thiserror`, `tracing` and `serde_json`, and
`tempfile` among its dev-dependencies, so `Cargo.toml` does not change. Plan A's `lib.rs` does
not forbid `unsafe_code` (its Task 6 says why), and the workspace's lints only deny
`unsafe_op_in_unsafe_fn`, so `server.rs` needs no lint attribute; each `unsafe` block carries a
`// SAFETY:` comment.

**Interfaces:**
- Consumes:
  - Task 1:
    - `protocol::{MAX_LINE, Op, Parsed, Request, parse_request, hello_json, ok_json,
      error_json, status_json}`
    - plan A's `protocol::daemon_event_json`
  - Plan A:
    - `events::{DaemonEvent, DaemonSink, Fanout}`
    - `status::StatusBoard`, with `snapshot`, `set_subscribers` and `set_socket`
    - `daemon::{Daemon, DaemonExit, Started}`, where `Started::board() -> Arc<StatusBoard>`
      and `Started::serve(self, sink, sleeper, jitter) -> DaemonExit`
    - `scheduler::{ThreadSleeper, uniform_jitter}`
    - `tagteam_engine::daemon::{MAX_SOCKET_PATH, daemon_socket_path}`
    - the CLI's `App::daemon` and `daemon_cmd` module (Task 12)
  - `tagteam_provider`:
    - `Cancel`
    - `atomic::ensure_private_dir(path: &Path) -> io::Result<()>`
    - `Clock` and `SystemClock`
- Produces (`tagteam_daemon::server`, as plan B's Interfaces show it):
  - `pub use tagteam_engine::daemon::MAX_SOCKET_PATH`
  - `pub const MAX_CONNECTIONS: usize = 16`, `pub const QUEUE: usize = 256`,
    `pub const IDLE: Duration` (30 s), `pub const WRITE_TIMEOUT: Duration` (2 s)
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub struct Limits { pub idle: Duration,
    pub write_timeout: Duration }`, with `Default`
  - `#[derive(Debug, thiserror::Error)] pub enum BindError { TooLong(PathBuf), Io(io::Error) }`
  - `Server::bind(path: &Path, home: &Path, board: Arc<StatusBoard>, cancel: Cancel) ->
    Result<Server, BindError>`
  - `Server::bind_with(path: &Path, home: &Path, board: Arc<StatusBoard>, cancel: Cancel,
    limits: Limits) -> Result<Server, BindError>`
  - `Server::hub(&self) -> &Hub`, `Server::shutdown(self)`; `impl Drop for Server` (the same
    clean-up)
  - `pub struct Hub` (`Send + Sync`), with `impl DaemonSink for Hub`. On `stopping` it closes
    the socket to new connections and removes its file (Decision 9).
  - `pub fn peer_uid(stream: &UnixStream) -> io::Result<u32>`
  - The CLI's `daemon_cmd::bind_socket(env: &Env, board: &Arc<StatusBoard>) -> Option<Server>`
    and `daemon_cmd::serve(started: Started<'_>, sink: &dyn DaemonSink, server:
    Option<&Server>) -> DaemonExit` (crate-private)
  - Tests:
    - `tests/common/mod.rs`: `short_tempdir()`, `board()`, `daemon_info()`, `HOME`, `T`,
      `wait_until`, `Peer`. Task 3 uses them.
    - `crates/tagteam/tests/common/mod.rs`: `short_tempdir()`. Tasks 6, 7 and 9 use it.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-daemon/tests/common/mod.rs`:

```rust
//! Shared by the socket tests (`server.rs`, `client.rs`, `umask.rs`). Each test file is its own
//! crate and uses only part of this module, so an item unused by one of them is not dead code
//! overall.
#![allow(dead_code)]

use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use tagteam_core::ProviderId;
use tagteam_daemon::events::Mode;
use tagteam_daemon::status::{DaemonInfo, NotifyInfo, ProviderStatus, StatusBoard};
use tagteam_engine::settings::NotifyEvent;

/// The home `status` writes paths under (D5.4).
pub const HOME: &str = "/Users/tester";
/// 2026-09-21T14:13:20Z, the fixture daemon's start.
pub const T: i64 = 1_790_000_000;

/// A temporary directory short enough for a socket path under it (D14). `sun_path` holds 104
/// bytes on macOS, and macOS's own `$TMPDIR` can take most of them. It is made under `/tmp`,
/// else under `$TMPDIR` where `/tmp` cannot be written (a sandbox).
///
/// It is chmod'ed 0700 after it is made: a bind in another test of this binary may have
/// narrowed the umask to 0177 at that instant, which would leave the directory 0600 and
/// unusable.
pub fn short_tempdir() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("tt")
        .tempdir_in("/tmp")
        .or_else(|_| tempfile::Builder::new().prefix("tt").tempdir())
        .expect("a temporary directory under /tmp or $TMPDIR");
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        dir.path().as_os_str().len() <= 48,
        "{} is too long for socket tests; set TMPDIR to a short directory",
        dir.path().display()
    );
    dir
}

/// The fixture daemon: pid 4242, started at `T`, logging under `HOME`.
pub fn daemon_info() -> DaemonInfo {
    DaemonInfo {
        pid: 4242,
        version: "20261014.2045.0".into(),
        started_at: T,
        socket: None,
        log: Path::new(HOME).join(".local/state/tagteam/tagteam.log"),
    }
}

/// A board with notifications on and one provider in `auto`.
pub fn board() -> Arc<StatusBoard> {
    let board = StatusBoard::new(
        daemon_info(),
        NotifyInfo {
            enabled: true,
            backend: Some("osascript"),
            events: vec![NotifyEvent::Switch, NotifyEvent::Quarantine, NotifyEvent::Limit],
        },
    );
    board.set_provider(ProviderStatus {
        provider: ProviderId::new("claude-code"),
        mode: Mode::Auto,
        autoswitch: true,
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

/// Polls `ready` every 10 ms; fails the test, naming `what`, if it does not hold within 10 s.
pub fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "never: {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

/// A raw client of the socket, which reads and writes lines itself.
pub struct Peer {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Peer {
    /// Connected, with nothing read yet. A read waits 10 s at most.
    pub fn connect(path: &Path) -> Peer {
        let stream = UnixStream::connect(path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        Peer {
            writer: stream.try_clone().unwrap(),
            reader: BufReader::new(stream),
        }
    }

    /// Connected, with the greeting read.
    pub fn greeted(path: &Path) -> Peer {
        let mut peer = Peer::connect(path);
        let hello = peer.line();
        assert_eq!(hello["event"], "hello", "{hello}");
        peer
    }

    /// Greeted and subscribed with the string id `id`, its `ok` read.
    pub fn subscribed(path: &Path, id: &str) -> Peer {
        let mut peer = Peer::greeted(path);
        peer.send(&format!(r#"{{"id":"{id}","op":"subscribe"}}"#));
        let ok = peer.line();
        assert_eq!(ok["ok"], true, "{ok}");
        peer
    }

    /// `line`, then its newline.
    pub fn send(&mut self, line: &str) {
        self.send_bytes(format!("{line}\n").as_bytes()).unwrap();
    }

    /// Raw bytes. It fails once the server has closed the connection.
    pub fn send_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)
    }

    /// The next line, or `None` at the end of the connection. A reset, or a last line cut short
    /// by the close, counts as the end. It fails the test when nothing comes within 10 s.
    pub fn next_line(&mut self) -> Option<Value> {
        let mut buf = String::new();
        match self.reader.read_line(&mut buf) {
            Ok(0) => None,
            Ok(_) if buf.ends_with('\n') => Some(serde_json::from_str(&buf).unwrap()),
            Ok(_) => None,
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => None,
            Err(e) => panic!("no line within 10 s: {e}"),
        }
    }

    /// The next line; it fails the test at the end of the connection.
    pub fn line(&mut self) -> Value {
        self.next_line()
            .expect("a line, not the end of the connection")
    }

    /// Every line left, up to the end of the connection.
    pub fn rest(&mut self) -> Vec<Value> {
        std::iter::from_fn(|| self.next_line()).collect()
    }
}
```

Create `crates/tagteam-daemon/tests/server.rs`:

```rust
//! D5's socket server, over real sockets in short temporary directories (D14). Run outside a
//! sandbox that refuses `bind` on Unix sockets.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tagteam_core::ProviderId;
use tagteam_daemon::events::{DaemonEvent, DaemonSink, Mode, StopReason, WaitingFor};
use tagteam_daemon::protocol::{
    MAX_LINE, RequestId, daemon_event_json, hello_json, ok_json, status_json,
};
use tagteam_daemon::server::{
    BindError, Hub, IDLE, Limits, MAX_CONNECTIONS, MAX_SOCKET_PATH, QUEUE, Server, WRITE_TIMEOUT,
    peer_uid,
};
use tagteam_daemon::status::StatusBoard;
use tagteam_engine::auto::AutoEvent;
use tagteam_provider::Cancel;

use common::{HOME, Peer, board, short_tempdir, wait_until};

/// A server on `daemon.sock` in `dir`, with the default limits, and its board.
fn start(dir: &Path) -> (Server, Arc<StatusBoard>) {
    start_with(dir, Limits::default())
}

fn start_with(dir: &Path, limits: Limits) -> (Server, Arc<StatusBoard>) {
    let board = board();
    let server = Server::bind_with(
        &dir.join("daemon.sock"),
        Path::new(HOME),
        Arc::clone(&board),
        Cancel::new(),
        limits,
    )
    .unwrap();
    (server, board)
}

/// An event line without its `ts`, which the hub stamps from the wall clock.
fn without_ts(mut v: Value) -> Value {
    v.as_object_mut().unwrap().remove("ts");
    v
}

fn mode_event() -> DaemonEvent {
    DaemonEvent::Mode {
        provider: ProviderId::new("claude-code"),
        mode: Mode::Waiting,
        waiting_for: Some(WaitingFor::EngineLock),
        holder_pid: Some(5151),
        backoff: None,
    }
}

fn usage_event() -> DaemonEvent {
    DaemonEvent::Usage {
        provider: ProviderId::new("claude-code"),
        updated: vec![1, 2],
        fetch_errors: BTreeMap::new(),
    }
}

/// Event `n` of a burst: about 2 KiB, numbered at the start of its message.
fn burst_event(n: usize) -> DaemonEvent {
    DaemonEvent::Auto(AutoEvent::Error {
        provider: ProviderId::new("claude-code"),
        message: format!("{n:06} {}", "x".repeat(2000)),
        transient: true,
    })
}

#[test]
fn the_server_speaks_first_with_the_greeting() {
    // D5.2: the greeting comes before any request.
    let dir = short_tempdir();
    let (server, board) = start(dir.path());
    let mut peer = Peer::connect(&dir.path().join("daemon.sock"));
    assert_eq!(peer.line(), hello_json(&board.snapshot().daemon));
    server.shutdown();
}

#[test]
fn status_answers_from_the_board_and_the_connection_stays_open() {
    // D5.3, D5.4: a reply carries its request's id, and its result is the board as it is
    // now, written by `status_json`.
    let dir = short_tempdir();
    let (server, board) = start(dir.path());
    let mut peer = Peer::greeted(&dir.path().join("daemon.sock"));
    peer.send(r#"{"id":1,"op":"status"}"#);
    assert_eq!(
        peer.line(),
        ok_json(
            &RequestId::Int(1),
            status_json(&board.snapshot(), Path::new(HOME))
        )
    );
    // What the scheduler writes next shows in the next reply.
    let mut moved = board.snapshot().providers[0].clone();
    moved.mode = Mode::Collect;
    board.set_provider(moved);
    peer.send(r#"{"id":"two","op":"status"}"#);
    let reply = peer.line();
    assert_eq!(reply["id"], "two");
    assert_eq!(reply["result"]["providers"][0]["mode"], "collect");
    server.shutdown();
}

#[test]
fn a_malformed_request_is_refused_and_the_connection_stays_open() {
    // Decision 1, D5.3: a request that is an object but not a usable one gets `ok: false`.
    let dir = short_tempdir();
    let (server, _board) = start(dir.path());
    let mut peer = Peer::greeted(&dir.path().join("daemon.sock"));
    peer.send(r#"{"op":"status"}"#);
    let refused = peer.line();
    assert_eq!(refused["id"], Value::Null, "{refused}");
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"]["type"], "bad-request");
    peer.send(r#"{"id":"q","op":"pause"}"#);
    let unknown = peer.line();
    assert_eq!(unknown["id"], "q", "{unknown}");
    assert_eq!(unknown["error"]["type"], "unknown-op");
    peer.send(r#"{"id":3,"op":"status"}"#);
    assert_eq!(peer.line()["ok"], true);
    server.shutdown();
}

#[test]
fn subscribe_delivers_every_event_in_order_after_its_ok() {
    // D5.5: after the ok, every event the daemon emits, as `daemon_event_json` writes it. The
    // hub keeps the board's count of subscribers.
    let dir = short_tempdir();
    let (server, board) = start(dir.path());
    let mut peer = Peer::greeted(&dir.path().join("daemon.sock"));
    assert_eq!(board.snapshot().subscribers, 0);
    peer.send(r#"{"id":"s","op":"subscribe"}"#);
    assert_eq!(
        peer.line(),
        json!({"schemaVersion": 1, "id": "s", "ok": true, "result": {}})
    );
    assert_eq!(board.snapshot().subscribers, 1);
    let sent = [
        DaemonEvent::Changed,
        mode_event(),
        usage_event(),
        burst_event(0),
        DaemonEvent::Stopping {
            reason: StopReason::Upgrade,
        },
    ];
    for e in &sent {
        server.hub().emit(e);
    }
    for e in &sent {
        assert_eq!(without_ts(peer.line()), without_ts(daemon_event_json(e, 0)));
    }
    drop(peer);
    wait_until("the subscriber gone from the board", || {
        board.snapshot().subscribers == 0
    });
    server.shutdown();
}

#[test]
fn status_and_subscribe_interleave_on_one_connection() {
    // D5.3: requests may come at any time, subscribed or not. Replies carry `id` and events
    // `event`, each written whole, in the order produced.
    let dir = short_tempdir();
    let (server, board) = start(dir.path());
    let mut peer = Peer::subscribed(&dir.path().join("daemon.sock"), "sub");
    server.hub().emit(&DaemonEvent::Changed);
    peer.send(r#"{"id":5,"op":"status"}"#);
    assert_eq!(
        peer.line()["event"],
        "changed",
        "queued before the request, so written before its reply"
    );
    let reply = peer.line();
    assert_eq!(reply["id"], 5);
    assert_eq!(reply["result"]["subscribers"], 1);
    server.hub().emit(&mode_event());
    peer.send(r#"{"id":"again","op":"subscribe"}"#);
    assert_eq!(peer.line()["event"], "mode");
    assert_eq!(
        peer.line(),
        json!({"schemaVersion": 1, "id": "again", "ok": true, "result": {}}),
        "a second subscribe is answered and changes nothing"
    );
    server.hub().emit(&DaemonEvent::Changed);
    assert_eq!(peer.line()["event"], "changed");
    peer.send(r#"{"id":6,"op":"status"}"#);
    assert_eq!(peer.line()["id"], 6, "one queue: the event came once");
    assert_eq!(board.snapshot().subscribers, 1);
    server.shutdown();
}

#[test]
fn a_stalled_subscriber_is_dropped_alone_and_the_rest_see_every_event() {
    // Review Focus 2, D5.5: sixteen subscribers, one of which never reads, while the scheduler
    // emits a burst. The stalled one's socket buffers fill, its thread blocks writing, and its
    // queue fills: the hub drops it alone and never waits for it. The fifteen others get every
    // event, in order.
    const BATCH: usize = 32;
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    // A write may block for a minute: the full queue, not the write timeout, drops it.
    let (server, board) = start_with(
        dir.path(),
        Limits {
            idle: IDLE,
            write_timeout: Duration::from_secs(60),
        },
    );
    let mut stalled = Peer::subscribed(&path, "stalled");
    let read: Vec<Arc<AtomicUsize>> = (0..MAX_CONNECTIONS - 1).map(|_| Arc::default()).collect();
    let readers: Vec<_> = read
        .iter()
        .enumerate()
        .map(|(n, count)| {
            let mut peer = Peer::subscribed(&path, &format!("healthy-{n}"));
            let count = Arc::clone(count);
            thread::spawn(move || {
                let mut seen = Vec::new();
                loop {
                    let event = peer.line();
                    if event["event"] == "stopping" {
                        return seen;
                    }
                    seen.push(event["message"].as_str().unwrap()[..6].to_owned());
                    count.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();
    assert_eq!(board.snapshot().subscribers, MAX_CONNECTIONS);

    // At least QUEUE + 50, and on until the stalled one is dropped, however much its socket
    // buffers hold. Each batch waits until the fifteen have read every event so far, so only
    // the stalled one can fall behind.
    let (mut emitted, mut in_emit) = (0, Duration::ZERO);
    while emitted < QUEUE + 50 || board.snapshot().subscribers == MAX_CONNECTIONS {
        assert!(
            emitted < 16 * QUEUE,
            "the stalled subscriber was never dropped"
        );
        for _ in 0..BATCH {
            let event = burst_event(emitted);
            let started = Instant::now();
            server.hub().emit(&event);
            in_emit += started.elapsed();
            emitted += 1;
        }
        wait_until("the healthy subscribers to read the batch", || {
            read.iter().all(|c| c.load(Ordering::SeqCst) == emitted)
        });
    }
    assert!(
        in_emit < Duration::from_secs(1),
        "emit waited on a client: {in_emit:?} for {emitted} events"
    );
    assert_eq!(board.snapshot().subscribers, MAX_CONNECTIONS - 1);

    server.hub().emit(&DaemonEvent::Stopping {
        reason: StopReason::Signal,
    });
    let want: Vec<String> = (0..emitted).map(|n| format!("{n:06}")).collect();
    for reader in readers {
        assert_eq!(reader.join().unwrap(), want);
    }
    // The stalled one finds what its buffers held, then the end of its connection.
    let got = stalled.rest();
    assert!(
        got.len() < emitted,
        "{} of {emitted} events reached it",
        got.len()
    );
    server.shutdown();
}

#[test]
fn a_line_over_64_kib_closes_the_connection() {
    // D5.2: at most 64 KiB per line, newline left out. A request padded to the limit is read;
    // one byte more closes the connection with no reply, whether its newline never comes,
    // comes with it, or comes later.
    let dir = short_tempdir();
    let (server, _board) = start(dir.path());
    let path = dir.path().join("daemon.sock");
    let padded = |len: usize| {
        let mut line = br#"{"id":1,"op":"status"}"#.to_vec();
        line.resize(len, b' ');
        line
    };
    let mut at_limit = Peer::greeted(&path);
    let mut line = padded(MAX_LINE);
    line.push(b'\n');
    at_limit.send_bytes(&line).unwrap();
    assert_eq!(at_limit.line()["id"], 1);
    let mut over = Peer::greeted(&path);
    // The server may close before the last byte is taken; the write's error is no matter.
    let _ = over.send_bytes(&padded(MAX_LINE + 1));
    assert!(over.rest().is_empty());
    // Terminated, with its newline in the same write or in a later one: closed too, with no
    // reply. A whole line goes through `parse_request`, which closes on one byte over.
    let mut terminated = padded(MAX_LINE + 1);
    terminated.push(b'\n');
    let mut same = Peer::greeted(&path);
    let _ = same.send_bytes(&terminated);
    assert!(same.rest().is_empty());
    let mut later = Peer::greeted(&path);
    let _ = later.send_bytes(&padded(MAX_LINE + 1));
    thread::sleep(Duration::from_millis(200));
    let _ = later.send_bytes(b"\n");
    assert!(later.rest().is_empty());
    server.shutdown();
}

#[test]
fn a_line_that_is_not_a_json_object_closes_the_connection() {
    // D5.2: JSON that is not an object, a line that is not JSON, and one that is not UTF-8.
    let dir = short_tempdir();
    let (server, _board) = start(dir.path());
    let path = dir.path().join("daemon.sock");
    let lines: [&[u8]; 5] = [b"[1,2]\n", b"\"status\"\n", b"not json\n", b"\xff\xfe\n", b"\n"];
    for line in lines {
        let mut peer = Peer::greeted(&path);
        peer.send_bytes(line).unwrap();
        assert!(peer.rest().is_empty(), "{}", String::from_utf8_lossy(line));
    }
    server.shutdown();
}

#[test]
fn the_seventeenth_connection_is_closed_before_the_greeting() {
    // D5.1: at most sixteen connections. The next is closed at once, and a place given back
    // goes to the next connection.
    let dir = short_tempdir();
    let (server, _board) = start(dir.path());
    let path = dir.path().join("daemon.sock");
    let mut peers: Vec<Peer> = (0..MAX_CONNECTIONS).map(|_| Peer::greeted(&path)).collect();
    assert!(
        Peer::connect(&path).rest().is_empty(),
        "the seventeenth was greeted"
    );
    peers.pop();
    wait_until("a new connection greeted in the freed place", || {
        Peer::connect(&path).next_line().is_some()
    });
    server.shutdown();
}

#[test]
fn an_idle_connection_is_closed_unless_it_subscribed() {
    // D5.2: a connection that has not subscribed and sends nothing for the idle time is
    // closed. One that keeps sending stays, and so does a subscribed one.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    // 300 ms idle against a request every 100 ms: three times the margin a loaded machine
    // needs.
    let (server, _board) = start_with(
        dir.path(),
        Limits {
            idle: Duration::from_millis(300),
            write_timeout: WRITE_TIMEOUT,
        },
    );
    let mut quiet = Peer::greeted(&path);
    let mut talking = Peer::greeted(&path);
    let mut listening = Peer::subscribed(&path, "l");
    let quiet = thread::spawn(move || {
        let started = Instant::now();
        let rest = quiet.rest();
        (started.elapsed(), rest)
    });
    for n in 0..8 {
        thread::sleep(Duration::from_millis(100));
        talking.send(&format!(r#"{{"id":{n},"op":"status"}}"#));
        assert_eq!(talking.line()["id"], n);
    }
    let (after, rest) = quiet.join().unwrap();
    assert!(rest.is_empty());
    assert!(
        after >= Duration::from_millis(250) && after < Duration::from_secs(2),
        "{after:?}"
    );
    server.hub().emit(&DaemonEvent::Changed);
    assert_eq!(listening.line()["event"], "changed");
    server.shutdown();
}

#[test]
fn a_leftover_file_is_replaced_and_the_socket_is_0600_in_a_0700_directory() {
    // D5.1: a missing parent is created 0700, and the socket 0600 through the umask, never
    // chmod'ed. A leftover file at the path, a regular one or a dead socket, is replaced.
    let dir = short_tempdir();
    let path = dir.path().join("data/tagteam/daemon.sock");
    let mode = |p: &Path| fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777;
    let is_socket = |p: &Path| fs::symlink_metadata(p).unwrap().file_type().is_socket();
    let bind = || Server::bind(&path, Path::new(HOME), board(), Cancel::new()).unwrap();

    let server = bind();
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    assert!(is_socket(&path));
    assert_eq!(mode(&path), 0o600);
    server.shutdown();

    fs::write(&path, "left behind").unwrap();
    let server = bind();
    assert!(is_socket(&path));
    Peer::greeted(&path);
    server.shutdown();

    drop(UnixListener::bind(&path).unwrap());
    assert!(path.exists(), "a dropped listener leaves its file");
    let server = bind();
    Peer::greeted(&path);
    assert_eq!(mode(&path), 0o600);
    server.shutdown();
}

#[test]
fn a_path_too_long_for_a_socket_address_is_refused() {
    // D5.1: `sun_path` bounds the path. One byte over is `TooLong`, with nothing created; the
    // longest that fits binds.
    let dir = short_tempdir();
    let room = MAX_SOCKET_PATH - dir.path().as_os_str().len() - 1;
    let fits = dir.path().join("s".repeat(room));
    let over = dir.path().join("s".repeat(room + 1));
    assert_eq!(fits.as_os_str().len(), MAX_SOCKET_PATH);
    let Err(BindError::TooLong(refused)) =
        Server::bind(&over, Path::new(HOME), board(), Cancel::new())
    else {
        panic!("a path of {} bytes was not refused as too long", MAX_SOCKET_PATH + 1);
    };
    assert_eq!(refused, over);
    assert!(!over.exists());
    let server = Server::bind(&fits, Path::new(HOME), board(), Cancel::new()).unwrap();
    Peer::greeted(&fits);
    server.shutdown();
}

#[test]
fn shutdown_closes_connections_and_removes_the_socket_file() {
    // D4.6: every subscriber gets what was emitted before the shutdown, `stopping` last; then
    // every connection ends, the file is gone, and a connect finds nothing.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let (server, board) = start(dir.path());
    let mut subscriber = Peer::subscribed(&path, "s");
    let mut idle = Peer::greeted(&path);
    server.hub().emit(&DaemonEvent::Stopping {
        reason: StopReason::Signal,
    });
    server.shutdown();
    assert_eq!(subscriber.line()["event"], "stopping");
    assert!(subscriber.next_line().is_none());
    assert!(idle.next_line().is_none());
    assert!(!path.exists());
    let refused = UnixStream::connect(&path).unwrap_err();
    assert_eq!(refused.kind(), io::ErrorKind::NotFound);
    assert_eq!(board.snapshot().subscribers, 0);
}

#[test]
fn shutdown_leaves_a_socket_another_daemon_bound() {
    // A daemon that took `daemon.lock` once this one let it go may have replaced the socket
    // already. Neither this one's `stopping` nor its shutdown may remove it.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let (server, _board) = start(dir.path());
    fs::remove_file(&path).unwrap();
    let successor = UnixListener::bind(&path).unwrap();
    server.hub().emit(&DaemonEvent::Stopping {
        reason: StopReason::Signal,
    });
    assert!(path.exists(), "`stopping` removed the successor's socket");
    server.shutdown();
    assert!(path.exists(), "the successor's socket was removed");
    drop(successor);
}

#[test]
fn stopping_removes_the_socket_at_once_and_subscribers_still_read_it() {
    // D4.6, Decision 9: `serve` emits `stopping` while it still holds `daemon.lock`. The hub
    // removes the socket file then, so no successor's socket can be the one removed, and a new
    // connect fails at once. A subscriber still reads `stopping`, and its connection is served
    // until `shutdown`.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let (server, _board) = start(dir.path());
    let mut subscriber = Peer::subscribed(&path, "s");
    server.hub().emit(&DaemonEvent::Stopping {
        reason: StopReason::Signal,
    });
    assert!(!path.exists(), "the socket file outlived `stopping`");
    let refused = UnixStream::connect(&path).unwrap_err();
    assert_eq!(refused.kind(), io::ErrorKind::NotFound);
    assert_eq!(subscriber.line()["event"], "stopping");
    subscriber.send(r#"{"id":3,"op":"status"}"#);
    assert_eq!(subscriber.line()["id"], 3, "an open connection is still answered");
    server.shutdown();
    assert!(subscriber.next_line().is_none());
    assert!(!path.exists());
}

#[test]
fn a_signal_stops_new_connections_but_subscribers_still_hear_stopping() {
    // D4.6, Review Focus 1: on the signal the accept thread goes and drops the listener, so
    // a new connect is refused at once. Open connections are still served until `shutdown`,
    // which comes after `serve` has emitted `stopping`.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let cancel = Cancel::new();
    let server = Server::bind(&path, Path::new(HOME), board(), cancel.clone()).unwrap();
    let mut subscriber = Peer::subscribed(&path, "s");
    cancel.request(libc::SIGTERM);
    wait_until("a new connect refused", || {
        matches!(UnixStream::connect(&path), Err(e) if e.kind() == io::ErrorKind::ConnectionRefused)
    });
    assert!(path.exists(), "the file stays until `stopping`");
    subscriber.send(r#"{"id":2,"op":"status"}"#);
    assert_eq!(subscriber.line()["id"], 2);
    server.hub().emit(&DaemonEvent::Stopping {
        reason: StopReason::Signal,
    });
    assert_eq!(subscriber.line()["event"], "stopping");
    assert!(!path.exists(), "`stopping` removed the file");
    server.shutdown();
    assert!(subscriber.next_line().is_none());
}

#[test]
fn peer_uid_names_this_process_for_its_own_connection() {
    // Decision 3. A connection from another user cannot be made without root; `admit`
    // compares this value with `geteuid()` and closes on any other.
    let (a, _b) = UnixStream::pair().unwrap();
    // SAFETY: geteuid has no preconditions and cannot fail.
    let euid = unsafe { libc::geteuid() };
    assert_eq!(peer_uid(&a).unwrap(), euid);
}

#[test]
fn the_hub_can_be_shared_between_threads() {
    // The scheduler emits on the main thread while connection threads subscribe.
    fn shared<T: Send + Sync>() {}
    shared::<Hub>();
}
```

Create `crates/tagteam-daemon/tests/umask.rs`:

```rust
//! D5.1: `bind` narrows the umask only around its own bind. The umask belongs to the whole
//! process, so this is a test binary of its own: no other test creates a file, or binds, while
//! it reads the mask back.

mod common;

use std::path::Path;

use tagteam_daemon::server::Server;
use tagteam_provider::Cancel;

#[test]
fn bind_puts_back_the_umask_it_found() {
    let dir = common::short_tempdir();
    // SAFETY: umask(2) only swaps the process's mask, and nothing else runs in this binary.
    let found = unsafe { libc::umask(0o022) };
    let server = Server::bind(
        &dir.path().join("daemon.sock"),
        Path::new(common::HOME),
        common::board(),
        Cancel::new(),
    )
    .unwrap();
    // SAFETY: as above.
    let after = unsafe { libc::umask(found) };
    server.shutdown();
    assert_eq!(after, 0o022);
}
```

- [ ] **Step 2: Run them and see them fail**

Socket tests bind real Unix sockets. Claude Code's macOS sandbox refuses that with
`Operation not permitted`, so run them outside it.

Run: `cargo test -p tagteam-daemon --test server --test umask`
Expected: the build fails with `error[E0432]: unresolved import \`tagteam_daemon::server\``.

- [ ] **Step 3: Implement the server**

In `crates/tagteam-daemon/src/lib.rs`, add the module after `pub mod scheduler;`:

```rust
pub mod server;
```

`server.rs` calls `libc` (`umask`, `poll`, `getpeereid`, `getsockopt`). Plan A's `lib.rs` has
no `forbid(unsafe_code)`, so no lint attribute is added; each `unsafe` block has its
`// SAFETY:` comment, as `crates/tagteam/src/auto.rs`'s `local_time` does.

Create `crates/tagteam-daemon/src/server.rs`:

```rust
//! D5: the daemon's socket. Start-up binds it once `daemon.lock` is held (D4.1 step 5). It
//! serves protocol 1 to at most sixteen connections of this user: the greeting, `status` from
//! the board, and every event to the subscribers (D5.2–D5.5). One thread accepts, and each
//! connection has a thread that reads its requests and writes its replies and events
//! (Decision 2). The scheduler only hands events to the hub, which never waits on a client
//! (D4). The hub closes the socket when `stopping` passes through it, while the daemon still
//! holds `daemon.lock` (D4.6, Decision 9).

use std::fs;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tagteam_provider::atomic::ensure_private_dir;
use tagteam_provider::{Cancel, Clock, SystemClock};

use crate::events::{DaemonEvent, DaemonSink};
use crate::protocol::{
    MAX_LINE, Op, Parsed, Request, daemon_event_json, error_json, hello_json, ok_json,
    parse_request, status_json,
};
use crate::status::StatusBoard;

/// D5.1: the longest socket path `bind` takes, in bytes: `sun_path` less its NUL. The engine
/// owns it, because `doctor` checks the same bound (plan A Decision 2).
pub use tagteam_engine::daemon::MAX_SOCKET_PATH;

/// D5.1: at most sixteen connections; the next is closed before the greeting.
pub const MAX_CONNECTIONS: usize = 16;
/// D5.5: the events a subscriber may have waiting before it is disconnected.
pub const QUEUE: usize = 256;
/// D5.2: how long a connection that has not subscribed may send nothing.
pub const IDLE: Duration = Duration::from_secs(30);
/// How long one write to a client may block. A client that takes nothing for that long is not
/// reading, and its connection is closed, so `shutdown` never waits on it for longer.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// Decision 2: how long a connection's read waits before it looks at its queue again. D4.6:
/// how long the accept thread waits before it looks at the cancel token again.
const POLL: Duration = Duration::from_millis(100);

/// The umask is the process's: binds in one process take turns, so one never puts back a mask
/// another narrowed.
static UMASK: Mutex<()> = Mutex::new(());

/// A server's timeouts. `Default` is D5's; tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// D5.2's idle timeout, before a subscribe.
    pub idle: Duration,
    /// How long one write to a client may block (`WRITE_TIMEOUT`).
    pub write_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            idle: IDLE,
            write_timeout: WRITE_TIMEOUT,
        }
    }
}

/// Why the socket was not bound. The daemon logs it at ERROR and runs without a socket (D5.1).
#[derive(Debug, thiserror::Error)]
pub enum BindError {
    #[error(
        "the socket path {} is longer than the {} bytes a Unix socket address holds",
        .0.display(),
        MAX_SOCKET_PATH
    )]
    TooLong(PathBuf),
    #[error("the socket could not be bound: {0}")]
    Io(#[from] io::Error),
}

/// D5's socket server: its accept thread, its connections and their hub.
pub struct Server {
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

/// What the accept and connection threads share.
struct Shared {
    board: Arc<StatusBoard>,
    hub: Hub,
    /// The home `status` writes paths under as `~/…` (D5.4).
    home: PathBuf,
    limits: Limits,
    /// Set by `shutdown`: each connection writes what it holds, then closes.
    stop: AtomicBool,
    /// Connections being served, for D5.1's limit.
    open: AtomicUsize,
    /// The connection threads; finished ones are let go at each accept.
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl Server {
    /// D5.1: removes a leftover file, binds 0600, starts the accept thread. The caller holds
    /// `daemon.lock`. `home` is the one `status` writes paths under as `~/…` (D5.4).
    pub fn bind(
        path: &Path,
        home: &Path,
        board: Arc<StatusBoard>,
        cancel: Cancel,
    ) -> Result<Server, BindError> {
        Self::bind_with(path, home, board, cancel, Limits::default())
    }

    /// `bind` with other timeouts.
    pub fn bind_with(
        path: &Path,
        home: &Path,
        board: Arc<StatusBoard>,
        cancel: Cancel,
        limits: Limits,
    ) -> Result<Server, BindError> {
        if path.as_os_str().as_bytes().len() > MAX_SOCKET_PATH {
            return Err(BindError::TooLong(path.to_path_buf()));
        }
        let listener = bind_private(path)?;
        let bound = identity(path).map(|identity| Bound {
            path: path.to_path_buf(),
            identity,
        });
        let shared = Arc::new(Shared {
            hub: Hub::new(Arc::clone(&board), bound),
            board,
            home: home.to_path_buf(),
            limits,
            stop: AtomicBool::new(false),
            open: AtomicUsize::new(0),
            threads: Mutex::new(Vec::new()),
        });
        let accepting = Arc::clone(&shared);
        let accept = listener
            .set_nonblocking(true)
            .and_then(|()| {
                thread::Builder::new()
                    .name("tagteam-accept".into())
                    .spawn(move || accept_loop(listener, &accepting, &cancel))
            })
            .inspect_err(|_| {
                let _ = fs::remove_file(path);
            })?;
        tracing::debug!(path = %path.display(), "the socket listens");
        Ok(Server {
            shared,
            accept: Some(accept),
        })
    }

    /// Where the scheduler's events go (D5.5).
    pub fn hub(&self) -> &Hub {
        &self.shared.hub
    }

    /// D4.6: closes every connection once it has written what it holds (the `stopping` event
    /// among it), and joins the threads. The socket file was removed when `stopping` went out;
    /// for a hub that never saw it, this removes the file now, if it is still the one this
    /// server bound.
    pub fn shutdown(mut self) {
        self.close();
    }

    fn close(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.hub.close_socket();
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
        let threads = std::mem::take(&mut *lock(&self.shared.threads));
        for thread in threads {
            let _ = thread.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.close();
    }
}

/// A poisoned lock's data is still whole here: no holder leaves it half-changed.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// D5.1: the parent made 0700 when it is missing, a leftover file removed, and the socket bound
/// with the umask narrowed to 0177, so the file is created 0600 and never chmod'ed. When the
/// daemon binds (D4.1 step 5), the scheduler has not started and the notifier has nothing to
/// send, so no thread creates a file or starts a process while the mask is narrowed. Binds in
/// one process take turns (`UMASK`), the parent's creation included.
fn bind_private(path: &Path) -> io::Result<UnixListener> {
    let _turn = lock(&UMASK);
    if let Some(dir) = path.parent() {
        ensure_private_dir(dir)?;
    }
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    // SAFETY: umask(2) only swaps the process's file mode creation mask; it cannot fail.
    let found = unsafe { libc::umask(0o177) };
    let bound = UnixListener::bind(path);
    // SAFETY: as above; this puts back the mask that was in force.
    unsafe { libc::umask(found) };
    bound
}

/// A file's device and inode, as `Hub::close_socket` compares them.
fn identity(path: &Path) -> Option<(u64, u64)> {
    fs::symlink_metadata(path)
        .ok()
        .map(|m| (m.dev(), m.ino()))
}

/// D5.1, D4.6: accepts until a signal, `stopping` or `shutdown`, looking at each every `POLL`.
/// Returning drops the listener, so a later connect is refused at once: a client finds no
/// daemon, never one that does not greet (Review Focus 1).
fn accept_loop(listener: UnixListener, shared: &Arc<Shared>, cancel: &Cancel) {
    while !shared.stop.load(Ordering::SeqCst)
        && shared.hub.accepting()
        && cancel.requested().is_none()
    {
        match pending(&listener, POLL) {
            Ok(true) => accept_pending(&listener, shared),
            Ok(false) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                tracing::debug!(error = %e, "the socket could not be polled");
                thread::sleep(POLL);
            }
        }
    }
}

/// Every connection waiting now; the listener does not block.
fn accept_pending(listener: &UnixListener, shared: &Arc<Shared>) {
    loop {
        match listener.accept() {
            Ok((stream, _)) => admit(stream, shared),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                // Out of descriptors, say: the next poll tries again.
                tracing::debug!(error = %e, "a connection could not be accepted");
                thread::sleep(POLL);
                return;
            }
        }
    }
}

/// Whether a connection waits on `listener`, after at most `timeout`.
fn pending(listener: &UnixListener, timeout: Duration) -> io::Result<bool> {
    let mut fd = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let ms = libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX);
    // SAFETY: `fd` is one writable `pollfd`, and its descriptor stays open while `listener`
    // lives.
    match unsafe { libc::poll(&mut fd, 1, ms) } {
        -1 => Err(io::Error::last_os_error()),
        0 => Ok(false),
        _ => Ok(true),
    }
}

/// D5.1, Decision 3: another user's connection, and one past the limit, are closed before the
/// greeting. Any other gets a thread of its own.
fn admit(stream: UnixStream, shared: &Arc<Shared>) {
    match peer_uid(&stream) {
        Ok(uid) if uid == own_uid() => {}
        Ok(uid) => {
            tracing::debug!(uid, "a connection from another user was closed");
            return;
        }
        Err(e) => {
            tracing::debug!(error = %e, "a connection whose peer is unknown was closed");
            return;
        }
    }
    if shared.open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
        shared.open.fetch_sub(1, Ordering::SeqCst);
        tracing::debug!(
            limit = MAX_CONNECTIONS,
            "a connection past the limit was closed"
        );
        return;
    }
    let slot = Slot(Arc::clone(shared));
    let spawned = thread::Builder::new()
        .name("tagteam-socket".into())
        .spawn(move || serve(stream, slot));
    match spawned {
        Ok(thread) => {
            let mut threads = lock(&shared.threads);
            threads.retain(|t| !t.is_finished());
            threads.push(thread);
        }
        Err(e) => {
            tracing::debug!(error = %e, "a connection's thread could not start; it was closed");
        }
    }
}

/// One of D5.1's sixteen places, given back when its connection ends.
struct Slot(Arc<Shared>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::SeqCst);
    }
}

fn own_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Decision 3: the effective uid of the process at the other end of `stream`, as the kernel
/// recorded it when that process connected.
#[cfg(target_os = "linux")]
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` is writable for `len` bytes, which `getsockopt` updates; the descriptor
    // stays open while `stream` lives.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &mut len,
        )
    };
    if rc == 0 {
        Ok(cred.uid)
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Decision 3: the effective uid of the process at the other end of `stream`, as the kernel
/// recorded it when that process connected.
#[cfg(not(target_os = "linux"))]
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let (mut uid, mut gid): (libc::uid_t, libc::gid_t) = (0, 0);
    // SAFETY: `uid` and `gid` are writable; the descriptor stays open while `stream` lives.
    let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if rc == 0 {
        Ok(uid)
    } else {
        Err(io::Error::last_os_error())
    }
}

/// D5.5's fan-out to the subscribers. `emit` only `try_send`s, so the scheduler never waits on
/// a client. A subscriber whose queue is full is dropped and its socket shut down, which ends
/// its connection even while its thread is blocked writing; the client re-reads what it shows
/// and subscribes again. The hub keeps the board's `subscribers` current. On `stopping` it
/// closes the socket (D4.6, Decision 9).
pub struct Hub {
    board: Arc<StatusBoard>,
    subscribers: Mutex<Vec<Subscriber>>,
    next: AtomicU64,
    /// The socket file as `bind` created it, until `close_socket` has dealt with it.
    file: Mutex<Option<Bound>>,
    /// Set by `close_socket`: the accept thread returns, dropping the listener.
    closed: AtomicBool,
}

/// The socket file `bind` created: its path, and its device and inode then.
struct Bound {
    path: PathBuf,
    identity: (u64, u64),
}

struct Subscriber {
    id: u64,
    queue: SyncSender<Arc<str>>,
    /// Its connection's socket, shut down when the hub drops the subscriber.
    socket: UnixStream,
}

impl Hub {
    fn new(board: Arc<StatusBoard>, file: Option<Bound>) -> Self {
        Self {
            board,
            subscribers: Mutex::new(Vec::new()),
            next: AtomicU64::new(0),
            file: Mutex::new(file),
            closed: AtomicBool::new(false),
        }
    }

    /// Whether the accept thread still takes new connections.
    fn accepting(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
    }

    /// D4.6, Decision 9: no new connection is accepted, and the socket file is removed while
    /// its device and inode are still the ones `bind` created. `stopping` calls this while the
    /// daemon still holds `daemon.lock`; `shutdown` calls it again, after the lock has gone,
    /// when a successor may have bound its own socket at the path, which then stays. Only the
    /// first call removes anything.
    fn close_socket(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let Some(bound) = lock(&self.file).take() else {
            return;
        };
        if identity(&bound.path) != Some(bound.identity) {
            return;
        }
        match fs::remove_file(&bound.path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                tracing::warn!(error = %e, path = %bound.path.display(), "the socket file could not be removed");
            }
            _ => tracing::debug!(path = %bound.path.display(), "the socket file was removed"),
        }
    }

    /// `e`, serialized once and stamped with the wall clock, for every subscriber.
    fn queue(&self, e: &DaemonEvent) {
        let mut subscribers = lock(&self.subscribers);
        if subscribers.is_empty() {
            return;
        }
        let now_s = SystemClock.now_ms().div_euclid(1000);
        let line: Arc<str> = format!("{}\n", daemon_event_json(e, now_s)).into();
        let before = subscribers.len();
        subscribers.retain(|s| match s.queue.try_send(Arc::clone(&line)) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                tracing::debug!(queue = QUEUE, "a subscriber fell behind and was disconnected");
                let _ = s.socket.shutdown(Shutdown::Both);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        });
        if subscribers.len() != before {
            self.board.set_subscribers(subscribers.len());
        }
    }

    /// A new subscriber on `socket`'s connection: the id `unsubscribe` takes, and its queue.
    fn subscribe(&self, socket: UnixStream) -> (u64, Receiver<Arc<str>>) {
        let (queue, events) = sync_channel(QUEUE);
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let mut subscribers = lock(&self.subscribers);
        subscribers.push(Subscriber { id, queue, socket });
        self.board.set_subscribers(subscribers.len());
        (id, events)
    }

    fn unsubscribe(&self, id: u64) {
        let mut subscribers = lock(&self.subscribers);
        let before = subscribers.len();
        subscribers.retain(|s| s.id != id);
        if subscribers.len() != before {
            self.board.set_subscribers(subscribers.len());
        }
    }
}

impl DaemonSink for Hub {
    /// Each event is queued for every subscriber. `stopping` is queued like any other, then the
    /// socket closes to new connections and its file goes (D4.6, Decision 9).
    fn emit(&self, e: &DaemonEvent) {
        self.queue(e);
        if matches!(e, DaemonEvent::Stopping { .. }) {
            self.close_socket();
        }
    }
}

/// A connection's thread (Decision 2): the greeting, then requests answered and queued events
/// written in the order they came. It ends when the client leaves, breaks the framing, falls
/// behind, or idles, or when the server shuts down.
fn serve(stream: UnixStream, slot: Slot) {
    let Ok(mut connection) = Connection::new(stream, &slot.0) else {
        return;
    };
    tracing::debug!("a socket connection opened");
    let why = connection.run();
    tracing::debug!(why, "a socket connection closed");
}

struct Connection<'s> {
    shared: &'s Shared,
    stream: UnixStream,
    /// Bytes received that are not yet a whole line.
    pending: Vec<u8>,
    /// When the client last sent anything (D5.2's idle timeout).
    heard: Instant,
    /// Its id at the hub and its queue, once it has subscribed.
    subscription: Option<(u64, Receiver<Arc<str>>)>,
}

impl<'s> Connection<'s> {
    fn new(stream: UnixStream, shared: &'s Shared) -> io::Result<Self> {
        // On macOS an accepted socket inherits the listener's O_NONBLOCK; the timeouts below
        // need a blocking one.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(POLL))?;
        stream.set_write_timeout(Some(shared.limits.write_timeout))?;
        Ok(Self {
            shared,
            stream,
            pending: Vec::new(),
            heard: Instant::now(),
            subscription: None,
        })
    }

    /// Serves the connection, and says why it ended, for the DEBUG line (D11).
    fn run(&mut self) -> &'static str {
        if self
            .send(&hello_json(&self.shared.board.snapshot().daemon))
            .is_err()
        {
            return "the greeting could not be written";
        }
        let mut chunk = [0u8; 8192];
        loop {
            // Read before the queue is written: whatever was queued before `shutdown`, the
            // `stopping` event among it, is written before the connection closes (D4.6).
            let stopping = self.shared.stop.load(Ordering::SeqCst);
            if self.flush().is_err() {
                return "an event could not be written";
            }
            if stopping {
                return "the daemon stopped";
            }
            if self.subscription.is_none() && self.heard.elapsed() >= self.shared.limits.idle {
                return "it was idle";
            }
            match self.stream.read(&mut chunk) {
                Ok(0) => return "the client closed it",
                Ok(n) => {
                    self.heard = Instant::now();
                    self.pending.extend_from_slice(&chunk[..n]);
                    if let Err(why) = self.answer() {
                        return why;
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return "a read failed",
            }
        }
    }

    /// Answers each whole line received (D5.2, D5.3, Decision 1). `Err` closes the connection.
    fn answer(&mut self) -> Result<(), &'static str> {
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            match parse_request(&line[..end]) {
                Parsed::Close => return Err("it broke the framing"),
                Parsed::Refuse(e) => self.reply(&error_json(&e))?,
                Parsed::Request(r) => self.request(r)?,
            }
        }
        if self.pending.len() > MAX_LINE {
            return Err("it sent a line over 64 KiB");
        }
        Ok(())
    }

    fn request(&mut self, r: Request) -> Result<(), &'static str> {
        match r.op {
            Op::Status => {
                let status = status_json(&self.shared.board.snapshot(), &self.shared.home);
                self.reply(&ok_json(&r.id, status))
            }
            Op::Subscribe if self.subscription.is_some() => {
                self.reply(&ok_json(&r.id, json!({})))
            }
            Op::Subscribe => {
                let socket = self
                    .stream
                    .try_clone()
                    .map_err(|_| "its socket could not be shared with the hub")?;
                self.subscription = Some(self.shared.hub.subscribe(socket));
                // D5.5: the `ok` comes before any event, so it goes ahead of the new queue.
                self.send(&ok_json(&r.id, json!({})))
                    .map_err(|_| "a reply could not be written")
            }
        }
    }

    /// A reply, after every event queued before it (D5.3's order).
    fn reply(&self, v: &Value) -> Result<(), &'static str> {
        self.flush()
            .map_err(|_| "an event could not be written")?;
        self.send(v).map_err(|_| "a reply could not be written")
    }

    /// Writes every event queued for this subscriber.
    fn flush(&self) -> io::Result<()> {
        if let Some((_, queue)) = &self.subscription {
            while let Ok(line) = queue.try_recv() {
                (&self.stream).write_all(line.as_bytes())?;
            }
        }
        Ok(())
    }

    /// One object, as one line.
    fn send(&self, v: &Value) -> io::Result<()> {
        let mut line = v.to_string();
        line.push('\n');
        (&self.stream).write_all(line.as_bytes())
    }
}

impl Drop for Connection<'_> {
    fn drop(&mut self) {
        if let Some((id, _)) = self.subscription.take() {
            self.shared.hub.unsubscribe(id);
        }
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}
```

- [ ] **Step 4: Run the tests and see them pass**

Run, outside the sandbox: `cargo test -p tagteam-daemon --test server --test umask`
Expected: PASS: 18 tests in `server`, 1 in `umask`.
`a_stalled_subscriber_is_dropped_alone_and_the_rest_see_every_event` takes about one to two
seconds. Then `cargo test -p tagteam-daemon --features test-hooks` (all of the crate's tests,
plan A's included) passes.

- [ ] **Step 5: Write the failing tests for the wiring**

In `crates/tagteam/tests/common/mod.rs`, after `use tagteam_engine::vault::SERVICE;` add:

```rust
use tagteam_engine::daemon::{MAX_SOCKET_PATH, daemon_socket_path};
```

and after `now_epoch_s`:

```rust
/// A fixture root short enough for the daemon's socket (D5.1, D14):
/// `<root>/home/.local/share/tagteam/daemon.sock` must fit `sun_path`, and macOS's own
/// `$TMPDIR` can push it past. It is made under `/tmp`, else under `$TMPDIR` where `/tmp`
/// cannot be written (a sandbox), and it fails the test, saying why, when the socket path
/// would not fit there.
pub fn short_tempdir() -> tempfile::TempDir {
    let d = tempfile::Builder::new()
        .prefix("tt")
        .tempdir_in("/tmp")
        .or_else(|_| tempfile::Builder::new().prefix("tt").tempdir())
        .expect("a temporary directory under /tmp or $TMPDIR");
    let sock = daemon_socket_path(&Env::for_test(d.path()));
    assert!(
        sock.as_os_str().len() <= MAX_SOCKET_PATH,
        "{} is too long for a Unix socket; run the tests where /tmp is writable",
        sock.display()
    );
    d
}
```

Append to `crates/tagteam/tests/daemon_cli.rs` (plan A's binary tests of `tagteam daemon`).
Plan A's file imports `std::path::{Path, PathBuf}`, `std::time::Duration`,
`serde_json::{Value, json}` and `tagteam_provider::Env` already. Replace its
`use std::time::Duration;` with:

```rust
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};
```

Then append:

```rust
/// Connects to `sock` once the daemon has bound it. It fails the test, killing the daemon,
/// after `within`.
fn connect_socket_within(daemon: &mut common::Running, sock: &Path, within: Duration) -> UnixStream {
    let deadline = Instant::now() + within;
    loop {
        match UnixStream::connect(sock) {
            Ok(stream) => return stream,
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                let _ = daemon.child.kill();
                panic!("the daemon never served {}: {e}", sock.display());
            }
        }
    }
}

/// The next line, as JSON.
fn next_json(reader: &mut BufReader<UnixStream>) -> Value {
    let mut line = String::new();
    assert!(reader.read_line(&mut line).unwrap() > 0, "the connection ended");
    serde_json::from_str(&line).unwrap()
}

#[test]
fn the_daemon_serves_its_socket_until_a_signal_and_then_removes_it() {
    // D4.1 step 5, D4.6, D5.1: the socket is bound before the scheduler starts and answers
    // from the board. On SIGTERM a subscriber hears `stopping` before its connection ends;
    // then the file is gone, and the daemon exits 0. Notifications are off, as in every test
    // of this file.
    let root = common::short_tempdir();
    common::seed_home(&Env::for_test(root.path()));
    cmd(root.path())
        .args(["config", "set", "notify.enabled", "false"])
        .assert()
        .success();
    let sock = root.path().join("home/.local/share/tagteam/daemon.sock");
    let mut cmd = common::std_cmd(root.path());
    cmd.arg("daemon").stdin(Stdio::null());
    let mut daemon = common::Running::spawn(cmd);

    let stream = connect_socket_within(&mut daemon, &sock, Duration::from_secs(10));
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let hello = next_json(&mut reader);
    assert_eq!(hello["event"], "hello", "{hello}");
    assert_eq!(hello["protocol"], 1);
    assert_eq!(hello["pid"], json!(daemon.child.id()));

    (&stream).write_all(b"{\"id\":1,\"op\":\"status\"}\n").unwrap();
    let status = next_json(&mut reader);
    assert_eq!(status["id"], 1, "{status}");
    assert_eq!(status["ok"], true, "{status}");
    let result = &status["result"];
    assert_eq!(result["daemon"]["pid"], json!(daemon.child.id()));
    assert_eq!(
        result["daemon"]["socket"],
        "~/.local/share/tagteam/daemon.sock"
    );
    assert_eq!(result["subscribers"], 0);

    (&stream).write_all(b"{\"id\":2,\"op\":\"subscribe\"}\n").unwrap();
    assert_eq!(next_json(&mut reader)["ok"], true);
    daemon.signal(libc::SIGTERM);
    let mut last = Value::Null;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 {
            break;
        }
        last = serde_json::from_str(&line).unwrap();
    }
    assert_eq!(last["event"], "stopping", "{last}");
    assert_eq!(last["reason"], "signal");
    let out = daemon.finish(Duration::from_secs(10));
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!sock.exists(), "the socket file was left behind");
}
```

Append to `crates/tagteam/src/daemon_cmd.rs`, as its own test module. It only exercises a bind
that fails before the umask is narrowed, so it is safe in the CLI's lib test binary, where other
tests create files concurrently:

```rust
#[cfg(test)]
mod socket_tests {
    use std::path::Path;

    use tagteam_daemon::status::{DaemonInfo, NotifyInfo, StatusBoard};
    use tagteam_provider::Env;

    use super::bind_socket;

    #[test]
    fn a_socket_that_cannot_be_bound_leaves_the_daemon_running_without_one() {
        // D5.1: scheduling never depends on the socket. A path past `sun_path` is not bound,
        // nothing is created for it, and the board names no socket.
        let mut env = Env::for_test(Path::new("/tmp/tagteam-unused"));
        env.xdg_data_home = Some(Path::new("/tmp").join("d".repeat(120)));
        let board = StatusBoard::new(
            DaemonInfo {
                pid: 1,
                version: "v".into(),
                started_at: 0,
                socket: None,
                log: env.log_file(),
            },
            NotifyInfo {
                enabled: false,
                backend: None,
                events: Vec::new(),
            },
        );
        assert!(bind_socket(&env, &board).is_none());
        assert_eq!(board.snapshot().daemon.socket, None);
        assert!(!env.data_dir().exists(), "nothing was created");
    }
}
```

- [ ] **Step 6: Run them and see them fail**

Run: `cargo test -p tagteam --features test-support --lib daemon_cmd::socket_tests`
Expected: the build fails with
`error[E0432]: unresolved import \`super::bind_socket\``.

Run, outside the sandbox:
`cargo test -p tagteam --features test-support --test daemon_cli the_daemon_serves_its_socket`
Expected: FAIL. Nothing binds the socket yet, so the test panics with
`the daemon never served …/home/.local/share/tagteam/daemon.sock`.

- [ ] **Step 7: Wire the server into `tagteam daemon`**

In `crates/tagteam/src/daemon_cmd.rs`, plan A's Task 12 imports:

```rust
use tagteam_daemon::events::{DaemonEvent, DaemonSink, Mode, StopReason, WaitingFor};
use tagteam_daemon::notify::Notice;
use tagteam_daemon::protocol::daemon_event_json;
use tagteam_daemon::watch::program_path;
```

Replace those four lines with:

```rust
use tagteam_daemon::daemon::{DaemonExit, Started};
use tagteam_daemon::events::{DaemonEvent, DaemonSink, Fanout, Mode, StopReason, WaitingFor};
use tagteam_daemon::notify::Notice;
use tagteam_daemon::protocol::daemon_event_json;
use tagteam_daemon::scheduler::{ThreadSleeper, uniform_jitter};
use tagteam_daemon::server::Server;
use tagteam_daemon::status::StatusBoard;
use tagteam_daemon::watch::program_path;
use tagteam_engine::daemon::daemon_socket_path;
```

(`std::sync::Arc` and `tagteam_provider::{Cancel, Env, Keychain}` are imported already.) Then
add these two functions after `program`, above `mode_text`:

```rust
/// D4.1 step 5, D5.1: the socket, bound once start-up holds `daemon.lock` and before the
/// scheduler runs. A bind that fails is logged at ERROR, and the daemon runs without a socket:
/// scheduling never depends on it.
pub(crate) fn bind_socket(env: &Env, board: &Arc<StatusBoard>) -> Option<Server> {
    let path = daemon_socket_path(env);
    match Server::bind(&path, &env.home, Arc::clone(board), env.cancel.clone()) {
        Ok(server) => {
            board.set_socket(Some(path));
            Some(server)
        }
        Err(e) => {
            tracing::error!(error = %e, "the daemon runs without its socket");
            None
        }
    }
}

/// D5.5, D8.1: the daemon's run, every event going to `sink` (the command's own output) and
/// then, when the socket is bound, to its subscribers through the hub. The hub closes the
/// socket when `stopping` reaches it, before `serve` lets `daemon.lock` go (D4.6).
pub(crate) fn serve(
    started: Started<'_>,
    sink: &dyn DaemonSink,
    server: Option<&Server>,
) -> DaemonExit {
    let mut sinks: Vec<&dyn DaemonSink> = vec![sink];
    if let Some(server) = server {
        sinks.push(server.hub());
    }
    started.serve(&Fanout(sinks), &ThreadSleeper, &mut uniform_jitter)
}
```

`Fanout` forwards `tick_done` and `notified` to each sink, and the hub keeps the defaults, so
the CLI's sink still hears every notice `NotifySink` announces.

In `crates/tagteam/src/app.rs`, `fn daemon` (plan A Task 12) ends:

```rust
        let started = Daemon::start(engine, opts, &mut |notice| {
            let _ = writeln!(err, "note: {notice}");
        })?;
        let exit = if self.json {
            let sink = daemon_cmd::DaemonJson::new(&mut **out, &now_ms).stopping(engine.cancel());
            started.serve(&sink, &ThreadSleeper, &mut uniform_jitter)
        } else {
            let auto = HumanSink::new(&mut **out, &mut **err, &now_ms, &names, color, several)
                .stopping(engine.cancel());
            let sink = daemon_cmd::DaemonHuman::new(auto, engine.env());
            sink.started(std::process::id());
            started.serve(&sink, &ThreadSleeper, &mut uniform_jitter)
        };
```

Replace those lines with:

```rust
        let started = Daemon::start(engine, opts, &mut |notice| {
            let _ = writeln!(err, "note: {notice}");
        })?;
        // D4.1 step 5: the socket, once `daemon.lock` is held and before the scheduler runs.
        let server = daemon_cmd::bind_socket(engine.env(), &started.board());
        let exit = if self.json {
            let sink = daemon_cmd::DaemonJson::new(&mut **out, &now_ms).stopping(engine.cancel());
            daemon_cmd::serve(started, &sink, server.as_ref())
        } else {
            let auto = HumanSink::new(&mut **out, &mut **err, &now_ms, &names, color, several)
                .stopping(engine.cancel());
            let sink = daemon_cmd::DaemonHuman::new(auto, engine.env());
            sink.started(std::process::id());
            daemon_cmd::serve(started, &sink, server.as_ref())
        };
        // D4.6: the hub removed the socket file when `stopping` went out, while `daemon.lock`
        // was still held. Each connection has written `stopping`; this closes them.
        if let Some(server) = server {
            server.shutdown();
        }
```

The `Ok(match exit { … })` that follows is unchanged. `ThreadSleeper` and `uniform_jitter`
stay imported in `app.rs`: `App::auto` uses them (plan A Task 7). `engine.env()` is the Env the
signal handlers set: `engine.cancel()` is `&env.cancel`.

- [ ] **Step 8: Run the tests and see them pass**

Run: `cargo test -p tagteam --features test-support --lib daemon_cmd::socket_tests`
Expected: PASS, 1 test.

Run, outside the sandbox:
`cargo test -p tagteam --features test-support --test daemon_cli`
Expected: PASS. The new test passes, and so do plan A's.

Then run `cargo test -p tagteam-daemon --features test-hooks` and
`cargo test -p tagteam --features test-support` (outside the sandbox), `cargo fmt --all`,
`cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
and `cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`.
Expected: PASS, and clean. `daemon_cli` takes about 20 s, as in plan A.

- [ ] **Step 9: Commit** (the controller commits; implementers do not)

```
git add crates/tagteam-daemon/src/server.rs crates/tagteam-daemon/src/lib.rs \
  crates/tagteam-daemon/tests/common/mod.rs crates/tagteam-daemon/tests/server.rs \
  crates/tagteam-daemon/tests/umask.rs crates/tagteam/src/daemon_cmd.rs \
  crates/tagteam/src/app.rs crates/tagteam/tests/common/mod.rs \
  crates/tagteam/tests/daemon_cli.rs
git commit -m "Serve the daemon's socket: greeting, status and subscribers"
```

---

### Task 3: The client

**Depends on:** 1, 2

D5.7 defines `tagteam_daemon::client`, which `tagteam daemon status` (Task 5) and the TUI use. A
client finds the socket from its environment alone and learns in the first exchange whether a
daemon is there and speaks its protocol (D1.1 criterion 5). This task builds `connect`,
`Client` and `Events`, tested against Task 2's real server and against fake daemons that do
what the real one never does.

**Readings and choices this task commits to:**
- **`NoDaemon`** covers four cases:
  - `ENOENT`;
  - `ECONNREFUSED`, which is a socket file with nobody listening (Review Focus 1), and also
    Linux's answer for a file that is not a socket;
  - `ENOTSOCK`, which is macOS's answer for a file that is not a socket;
  - the end of the connection before the greeting, whether as EOF or `ECONNRESET`. A daemon at
    its connection limit closes this way (D5.1).

  Any other connect error is `Io`. That covers a socket in a directory this user cannot
  search, among others.
- **A greeting that does not come within 250 ms is `Err(Timeout)`, not `NoDaemon`.** A
  listener that accepts and says nothing is a daemon that is there but hung, and D8.2 reports
  a daemon that holds `daemon.lock` but does not answer as running. If this were `NoDaemon`,
  `daemon status` would wrongly say "not running". Task 2's server closes the listener when its
  accept thread goes, so a stopping daemon answers `NoDaemon`, never a timeout.
- **The greeting is checked in this order:**
  1. it is JSON with `event: "hello"`;
  2. its `protocol` is this client's, or the answer is `Mismatch`;
  3. only then are the other fields read.

  A later protocol may change the rest of the greeting, but never where `protocol` sits (D5.6).
  A first line that is anything else is `Protocol`.
- **Requests use integer ids from 1, one per request.** `status` and `subscribe` wait for the
  reply with their own id, for at most 2 s in all. They pass over event lines and replies to
  other ids (D5.3). The end of the connection while waiting is an `Io` error
  (`UnexpectedEof`). `subscribe`'s `ok` is bounded the same way.
- **`Events` waits for as long as it takes:** a daemon may be silent for hours. It yields each
  event line as `parse_event` reads it, and passes over any line that is no event. It ends at
  the end of the connection, a reset or a last line cut short included. A line that is not JSON
  yields one `Err(Protocol)`, and the iterator then ends. So does a read error.
- **The client applies `MAX_LINE` to what it reads too** (D5.2 frames both directions). The
  newline's offset is checked, not only the bytes still waiting for one, so a line one byte
  over the limit is refused even when its newline arrives in the same read as its last bytes.

**Files:**
- Create: `crates/tagteam-daemon/src/client.rs`
- Modify: `crates/tagteam-daemon/src/lib.rs` (add `pub mod client;` after `pub mod backoff;`,
  keeping the list in alphabetical order)
- Create: `crates/tagteam-daemon/tests/client.rs`

**Interfaces:**
- Consumes:
  - Task 1:
    - `protocol::{Event, Hello, MAX_LINE, PROTOCOL, Status, parse_event}`
  - Task 2:
    - `server::{Server, MAX_CONNECTIONS}`, and `Hub` through `DaemonSink`, in tests only
    - `tests/common/mod.rs`
  - Plan A:
    - `tagteam_engine::daemon::daemon_socket_path`
    - `tagteam_provider::Env`
- Produces (`tagteam_daemon::client`, as plan B's Interfaces show it):
  - `#[derive(Debug)] pub enum Connect { Connected(Client), NoDaemon, Mismatch { server: u32,
    client: u32 } }`
  - `#[derive(Debug, thiserror::Error)] pub enum ClientError { Io, Protocol(String), Timeout,
    Refused { kind, message } }`
  - `pub fn connect(env: &Env) -> Result<Connect, ClientError>` and
    `pub fn connect_at(path: &Path) -> Result<Connect, ClientError>`
  - `#[derive(Debug)] pub struct Client`, with `hello(&self) -> &Hello`,
    `status(&mut self) -> Result<Status, ClientError>` and
    `subscribe(self) -> Result<Events, ClientError>`
  - `#[derive(Debug)] pub struct Events`, which is
    `Iterator<Item = Result<Event, ClientError>>`
  - `pub const GREETING_TIMEOUT: Duration` (250 ms) and `pub const REPLY_TIMEOUT: Duration`
    (2 s)

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-daemon/tests/client.rs`:

```rust
//! D5.7: the client, against the real server (Task 2) and against fake daemons for what the
//! real one never does. Run outside a sandbox that refuses `bind` on Unix sockets.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tagteam_core::ProviderId;
use tagteam_core::autoswitch::Trigger;
use tagteam_daemon::client::{Client, ClientError, Connect, connect, connect_at};
use tagteam_daemon::events::{DaemonEvent, DaemonSink, Mode, StopReason, WaitingFor};
use tagteam_daemon::protocol::{Event, Hello, MAX_LINE, PROTOCOL, Status, status_json};
use tagteam_daemon::server::{MAX_CONNECTIONS, Server};
use tagteam_engine::auto::AutoEvent;
use tagteam_engine::daemon::daemon_socket_path;
use tagteam_provider::{Cancel, Env};

use common::{HOME, Peer, board, short_tempdir, wait_until};

/// The fixture daemon's greeting (`common::daemon_info`).
const HELLO: &str = r#"{"schemaVersion":1,"event":"hello","protocol":1,"version":"20261014.2045.0","pid":4242,"startedAt":"2026-09-21T14:13:20Z"}"#;

fn serve(path: &Path) -> Server {
    Server::bind(path, Path::new(HOME), board(), Cancel::new()).unwrap()
}

/// The `Client` a connect gave, failing the test on any other answer.
fn connected(answer: Result<Connect, ClientError>) -> Client {
    match answer {
        Ok(Connect::Connected(client)) => client,
        other => panic!("not connected: {other:?}"),
    }
}

fn assert_no_daemon(answer: Result<Connect, ClientError>, case: &str) {
    match answer {
        Ok(Connect::NoDaemon) => {}
        other => panic!("{case}: {other:?}"),
    }
}

/// A fake daemon's side of its one connection.
struct Fake {
    reader: BufReader<UnixStream>,
    stream: UnixStream,
}

impl Fake {
    fn write(&mut self, line: &str) {
        self.stream
            .write_all(format!("{line}\n").as_bytes())
            .unwrap();
    }

    /// The next request, as JSON.
    fn request(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
}

/// A fake daemon at `path` that serves one connection with `script`, on a thread of its own.
fn fake(path: &Path, script: impl FnOnce(Fake) + Send + 'static) -> JoinHandle<()> {
    let listener = UnixListener::bind(path).unwrap();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        script(Fake {
            reader: BufReader::new(stream.try_clone().unwrap()),
            stream,
        });
    })
}

#[test]
fn connect_reads_the_greeting_and_status_answers_from_the_board() {
    // D5.7: `Connected`, with the greeting typed; `status` (D5.4) typed, as often as asked.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let server = serve(&path);
    let mut client = connected(connect_at(&path));
    assert_eq!(
        client.hello(),
        &Hello {
            protocol: PROTOCOL,
            version: "20261014.2045.0".into(),
            pid: 4242,
            started_at: "2026-09-21T14:13:20Z".into(),
        }
    );
    let status = client.status().unwrap();
    assert_eq!(status.daemon.pid, 4242);
    assert_eq!(status.daemon.protocol, PROTOCOL);
    assert_eq!(status.daemon.log, "~/.local/state/tagteam/tagteam.log");
    assert_eq!(status.notifications.backend.as_deref(), Some("osascript"));
    assert_eq!(status.providers.len(), 1);
    assert_eq!(status.providers[0].provider, "claude-code");
    assert_eq!(status.providers[0].mode, "auto");
    assert_eq!(status.subscribers, 0);
    assert_eq!(
        client.status().unwrap(),
        status,
        "a second request, with the next id, is answered too"
    );
    drop(client);
    server.shutdown();
}

#[test]
fn connect_finds_the_socket_from_the_environment_alone() {
    // D5.1, D1.1 criterion 5: `$XDG_DATA_HOME/tagteam/daemon.sock`, from the Env, with no
    // settings read.
    let dir = short_tempdir();
    let env = Env::for_test(dir.path());
    let server = Server::bind(&daemon_socket_path(&env), &env.home, board(), Cancel::new()).unwrap();
    assert_eq!(connected(connect(&env)).hello().pid, 4242);
    server.shutdown();
    assert_no_daemon(connect(&env), "after shutdown");
}

#[test]
fn a_missing_socket_is_no_daemon() {
    let dir = short_tempdir();
    assert_no_daemon(connect_at(&dir.path().join("daemon.sock")), "no file");
    assert_no_daemon(
        connect_at(&dir.path().join("absent/daemon.sock")),
        "no directory",
    );
}

#[test]
fn a_socket_without_a_listener_is_no_daemon() {
    // Review Focus 1: the file can exist with nobody listening, before start-up binds or once
    // the accept thread has gone. `connect` answers NoDaemon at once; it never hangs or
    // fails.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let quick = |case: &str| {
        let started = Instant::now();
        assert_no_daemon(connect_at(&path), case);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{case}: {:?}",
            started.elapsed()
        );
    };
    drop(UnixListener::bind(&path).unwrap());
    assert!(path.exists());
    quick("a socket file whose listener is gone");
    fs::remove_file(&path).unwrap();
    fs::write(&path, "").unwrap();
    quick("a file that is not a socket");
    fs::remove_file(&path).unwrap();
    // A daemon that is stopping: its accept thread went on the signal, and the file stays
    // until `stopping` or shutdown removes it.
    let cancel = Cancel::new();
    let server = Server::bind(&path, Path::new(HOME), board(), cancel.clone()).unwrap();
    cancel.request(libc::SIGTERM);
    wait_until("the accept thread gone", || {
        matches!(UnixStream::connect(&path), Err(e) if e.kind() == io::ErrorKind::ConnectionRefused)
    });
    assert!(path.exists());
    quick("a stopping daemon");
    server.shutdown();
    quick("a daemon that has stopped");
}

#[test]
fn the_end_of_the_connection_before_the_greeting_is_no_daemon() {
    // D5.7: a listener that closes before it greets, as a daemon at its limit does (D5.1).
    let dir = short_tempdir();
    let path = dir.path().join("fake.sock");
    let closing = fake(&path, drop);
    assert_no_daemon(connect_at(&path), "closed before the greeting");
    closing.join().unwrap();

    let path = dir.path().join("daemon.sock");
    let server = serve(&path);
    let full: Vec<Peer> = (0..MAX_CONNECTIONS).map(|_| Peer::greeted(&path)).collect();
    assert_no_daemon(connect_at(&path), "the seventeenth connection");
    drop(full);
    server.shutdown();
}

#[test]
fn another_protocol_is_a_mismatch() {
    // D5.6: a client that does not speak the greeting's protocol closes the connection and
    // works without the daemon. Nothing else of that greeting is read.
    let dir = short_tempdir();
    let path = dir.path().join("fake.sock");
    let daemon = fake(&path, |mut f| {
        f.write(r#"{"schemaVersion":1,"event":"hello","protocol":2,"version":"20270101.0000.0","renamed":4242}"#);
        let mut rest = String::new();
        assert_eq!(f.reader.read_line(&mut rest).unwrap(), 0, "the client closes");
    });
    match connect_at(&path) {
        Ok(Connect::Mismatch { server, client }) => assert_eq!((server, client), (2, PROTOCOL)),
        other => panic!("{other:?}"),
    }
    daemon.join().unwrap();
}

#[test]
fn a_first_line_that_is_not_a_greeting_is_a_protocol_error() {
    let dir = short_tempdir();
    let firsts = [
        "not json",
        r#"{"schemaVersion":1,"event":"changed","ts":"2026-09-21T14:13:20Z"}"#,
        r#"{"schemaVersion":1,"event":"hello","version":"x","pid":1,"startedAt":"x"}"#,
        r#"{"schemaVersion":1,"event":"hello","protocol":1,"version":"x","startedAt":"x"}"#,
    ];
    for (n, first) in firsts.into_iter().enumerate() {
        let path = dir.path().join(format!("fake{n}.sock"));
        let daemon = fake(&path, move |mut f| f.write(first));
        match connect_at(&path) {
            Err(ClientError::Protocol(_)) => {}
            other => panic!("{first}: {other:?}"),
        }
        daemon.join().unwrap();
    }
}

#[test]
fn a_daemon_that_does_not_greet_within_250_ms_times_out() {
    // D5.7: the greeting read is bounded. A daemon that accepts and says nothing is there but
    // hung: `Timeout`, not `NoDaemon`, so `daemon status` can say so (D8.2).
    let dir = short_tempdir();
    let path = dir.path().join("fake.sock");
    let daemon = fake(&path, |f| {
        thread::sleep(Duration::from_secs(1));
        drop(f);
    });
    let started = Instant::now();
    let answer = connect_at(&path);
    let waited = started.elapsed();
    assert!(matches!(answer, Err(ClientError::Timeout)), "{answer:?}");
    assert!(
        waited >= Duration::from_millis(200) && waited < Duration::from_millis(900),
        "{waited:?}"
    );
    daemon.join().unwrap();
}

#[test]
fn status_passes_over_events_and_other_replies_to_its_own() {
    // D5.3: replies carry `id` and events `event`; the client takes the reply with its id.
    let dir = short_tempdir();
    let path = dir.path().join("fake.sock");
    let status = status_json(&board().snapshot(), Path::new(HOME));
    let result = status.clone();
    let daemon = fake(&path, move |mut f| {
        f.write(HELLO);
        let request = f.request();
        assert_eq!(request["op"], "status");
        assert!(request["id"].is_i64(), "{request}");
        f.write(r#"{"schemaVersion":1,"event":"changed","ts":"2026-09-21T14:13:20Z"}"#);
        f.write(r#"{"schemaVersion":1,"id":"someone else's","ok":true,"result":{}}"#);
        f.write(
            &json!({"schemaVersion": 1, "id": request["id"], "ok": true, "result": result})
                .to_string(),
        );
    });
    let mut client = connected(connect_at(&path));
    assert_eq!(
        client.status().unwrap(),
        serde_json::from_value::<Status>(status).unwrap()
    );
    daemon.join().unwrap();
}

#[test]
fn a_refused_request_is_a_refused_error() {
    let dir = short_tempdir();
    let path = dir.path().join("fake.sock");
    let daemon = fake(&path, |mut f| {
        f.write(HELLO);
        let request = f.request();
        f.write(
            &json!({"schemaVersion": 1, "id": request["id"], "ok": false,
                    "error": {"type": "bad-request", "message": "no"}})
            .to_string(),
        );
    });
    let mut client = connected(connect_at(&path));
    match client.status() {
        Err(ClientError::Refused { kind, message }) => {
            assert_eq!((kind.as_str(), message.as_str()), ("bad-request", "no"));
        }
        other => panic!("{other:?}"),
    }
    daemon.join().unwrap();
}

#[test]
fn status_waits_two_seconds_at_most() {
    // D5.7: `status` waits up to 2 s for its reply.
    let dir = short_tempdir();
    let path = dir.path().join("fake.sock");
    let daemon = fake(&path, |mut f| {
        f.write(HELLO);
        f.request();
        thread::sleep(Duration::from_secs(3));
    });
    let mut client = connected(connect_at(&path));
    let started = Instant::now();
    let answer = client.status();
    let waited = started.elapsed();
    assert!(matches!(answer, Err(ClientError::Timeout)), "{answer:?}");
    assert!(
        waited >= Duration::from_millis(1900) && waited < Duration::from_millis(2900),
        "{waited:?}"
    );
    drop(client);
    daemon.join().unwrap();
}

#[test]
fn subscribe_yields_typed_events_in_order_until_the_daemon_stops() {
    // D5.5, D5.7: every event, typed, in order; the iterator ends when the daemon closes the
    // connection after `stopping` (D4.6).
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let server = serve(&path);
    let events = connected(connect_at(&path)).subscribe().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        for e in events {
            if tx.send(e.map_err(|e| e.to_string())).is_err() {
                return;
            }
        }
    });
    let cc = || ProviderId::new("claude-code");
    let sent = [
        DaemonEvent::Changed,
        DaemonEvent::Mode {
            provider: cc(),
            mode: Mode::Waiting,
            waiting_for: Some(WaitingFor::EngineLock),
            holder_pid: Some(5151),
            backoff: None,
        },
        DaemonEvent::Usage {
            provider: cc(),
            updated: vec![2],
            fetch_errors: BTreeMap::new(),
        },
        DaemonEvent::Auto(AutoEvent::Switch {
            provider: cc(),
            trigger: Trigger::AtLimit,
            from: 1,
            to: 2,
            warnings: vec![],
            dry_run: false,
        }),
        DaemonEvent::Stopping {
            reason: StopReason::Signal,
        },
    ];
    for e in &sent {
        server.hub().emit(e);
    }
    server.shutdown();
    let next = || {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("an event within 5 s")
            .expect("an event the client can read")
    };
    assert_eq!(next(), Event::Changed);
    assert_eq!(
        next(),
        Event::Mode {
            provider: "claude-code".into(),
            mode: "waiting".into(),
            waiting_for: Some("engine-lock".into()),
            holder_pid: Some(5151),
            backoff: None,
        }
    );
    assert_eq!(
        next(),
        Event::Usage {
            provider: "claude-code".into(),
            updated: vec![2],
            fetch_errors: BTreeMap::new(),
        }
    );
    match next() {
        Event::Auto { kind, provider, raw } => {
            assert_eq!((kind.as_str(), provider.as_str()), ("switch", "claude-code"));
            assert_eq!(raw["trigger"], "at-limit");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        next(),
        Event::Stopping {
            reason: "signal".into()
        }
    );
    assert!(
        matches!(
            rx.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ),
        "the iterator ends with the connection"
    );
    reader.join().unwrap();
}

/// `line` padded with spaces to `len` bytes, its newline left out.
fn padded(line: &str, len: usize) -> Vec<u8> {
    let mut bytes = line.as_bytes().to_vec();
    bytes.resize(len, b' ');
    bytes
}

#[test]
fn a_line_over_64_kib_is_a_protocol_error_wherever_its_newline_arrives() {
    // D5.2 frames both directions: at most `MAX_LINE` bytes, the newline left out. A line of
    // exactly that is read. One byte more is refused, whether its newline comes in the same
    // write as the rest (so in the read that passes the limit) or in a later one.
    const CHANGED: &str = r#"{"schemaVersion":1,"event":"changed","ts":"2026-09-21T14:13:20Z"}"#;
    for (case, later) in [("newline with the line", false), ("newline written later", true)] {
        let dir = short_tempdir();
        let path = dir.path().join("fake.sock");
        let daemon = fake(&path, move |mut f| {
            f.write(HELLO);
            let request = f.request();
            f.write(
                &json!({"schemaVersion": 1, "id": request["id"], "ok": true, "result": {}})
                    .to_string(),
            );
            let mut fits = padded(CHANGED, MAX_LINE);
            fits.push(b'\n');
            f.stream.write_all(&fits).unwrap();
            // The client may close once it has seen too much; a failed write is no matter.
            let over = padded(CHANGED, MAX_LINE + 1);
            if later {
                let _ = f.stream.write_all(&over);
                thread::sleep(Duration::from_millis(200));
                let _ = f.stream.write_all(b"\n");
            } else {
                let _ = f.stream.write_all(&[over.as_slice(), b"\n".as_slice()].concat());
            }
        });
        let mut events = connected(connect_at(&path)).subscribe().unwrap();
        assert_eq!(
            events.next().unwrap().unwrap(),
            Event::Changed,
            "{case}: a line of exactly MAX_LINE bytes is read"
        );
        match events.next() {
            Some(Err(ClientError::Protocol(m))) => assert!(m.contains("64 KiB"), "{case}: {m}"),
            other => panic!("{case}: {other:?}"),
        }
        assert!(events.next().is_none(), "{case}: the stream ends");
        drop(events);
        daemon.join().unwrap();
    }
}

#[test]
fn a_kind_the_client_does_not_know_is_yielded_as_unknown() {
    // D5.6, D5.7: an added kind (the token report's `token-index`, D13) reaches the caller as
    // `Unknown`, whole, and the stream goes on.
    let dir = short_tempdir();
    let path = dir.path().join("fake.sock");
    let token_index =
        json!({"schemaVersion":1,"event":"token-index","ts":"2026-09-21T14:13:20Z","files":3});
    let line = token_index.to_string();
    let daemon = fake(&path, move |mut f| {
        f.write(HELLO);
        let request = f.request();
        assert_eq!(request["op"], "subscribe");
        f.write(
            &json!({"schemaVersion": 1, "id": request["id"], "ok": true, "result": {}})
                .to_string(),
        );
        f.write(&line);
        f.write(r#"{"schemaVersion":1,"event":"changed","ts":"2026-09-21T14:13:20Z"}"#);
    });
    let events: Vec<Event> = connected(connect_at(&path))
        .subscribe()
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        events,
        [
            Event::Unknown {
                kind: "token-index".into(),
                raw: token_index,
            },
            Event::Changed,
        ]
    );
    daemon.join().unwrap();
}
```

- [ ] **Step 2: Run them and see them fail**

Run, outside the sandbox: `cargo test -p tagteam-daemon --test client`
Expected: the build fails with `error[E0432]: unresolved import \`tagteam_daemon::client\``.

- [ ] **Step 3: Implement**

In `crates/tagteam-daemon/src/lib.rs`, add the module after `pub mod backoff;`:

```rust
pub mod client;
```

Create `crates/tagteam-daemon/src/client.rs`:

```rust
//! D5.7: the client of the daemon's socket, used by `tagteam daemon status` and the TUI. It
//! finds the socket from the environment alone (D5.1), and learns in the first exchange
//! whether a daemon is there and speaks this protocol (D1.1 criterion 5).

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tagteam_engine::daemon::daemon_socket_path;
use tagteam_provider::Env;

use crate::protocol::{Event, Hello, MAX_LINE, PROTOCOL, Status, parse_event};

/// D5.7: how long `connect` waits for the greeting.
pub const GREETING_TIMEOUT: Duration = Duration::from_millis(250);
/// D5.7: how long `status`, and `subscribe`'s `ok`, may take.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// What `connect` found.
#[derive(Debug)]
pub enum Connect {
    Connected(Client),
    /// No socket file, a refused connection, or the end of it before the greeting: no daemon
    /// serves this data directory, and the caller works without one.
    NoDaemon,
    /// The daemon speaks another protocol (D5.6). The connection is closed, and the caller
    /// works without the daemon until it restarts on this binary (D4.5).
    Mismatch { server: u32, client: u32 },
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("the daemon answered something this client cannot read: {0}")]
    Protocol(String),
    #[error("the daemon did not answer in time")]
    Timeout,
    #[error("the daemon refused the request ({kind}): {message}")]
    Refused { kind: String, message: String },
}

/// D5.7: the daemon of the store this environment uses, at `daemon_socket_path(env)`.
pub fn connect(env: &Env) -> Result<Connect, ClientError> {
    connect_at(&daemon_socket_path(env))
}

/// D5.7: the daemon at `path`, greeted within `GREETING_TIMEOUT`. A greeting that does not come
/// in time is `Timeout`: something accepted and said nothing, so a daemon is there but does
/// not answer (D8.2).
pub fn connect_at(path: &Path) -> Result<Connect, ClientError> {
    let stream = match UnixStream::connect(path) {
        Ok(stream) => stream,
        Err(e) if no_daemon(&e) => return Ok(Connect::NoDaemon),
        Err(e) => return Err(e.into()),
    };
    stream.set_write_timeout(Some(REPLY_TIMEOUT))?;
    let mut lines = Lines::new(stream);
    let greeting = match lines.read_line(Some(Instant::now() + GREETING_TIMEOUT)) {
        Ok(Some(line)) => line,
        Ok(None) => return Ok(Connect::NoDaemon),
        Err(ClientError::Io(e)) if e.kind() == io::ErrorKind::ConnectionReset => {
            return Ok(Connect::NoDaemon);
        }
        Err(e) => return Err(e),
    };
    let v: Value = serde_json::from_slice(&greeting)
        .map_err(|e| ClientError::Protocol(format!("the greeting is not JSON ({e})")))?;
    if v.get("event").and_then(Value::as_str) != Some("hello") {
        return Err(ClientError::Protocol(
            "the first line is not the greeting".into(),
        ));
    }
    let server = v
        .get("protocol")
        .and_then(Value::as_u64)
        .ok_or_else(|| ClientError::Protocol("the greeting names no protocol".into()))?;
    if server != u64::from(PROTOCOL) {
        return Ok(Connect::Mismatch {
            server: u32::try_from(server).unwrap_or(u32::MAX),
            client: PROTOCOL,
        });
    }
    let hello = serde_json::from_value(v)
        .map_err(|e| ClientError::Protocol(format!("the greeting cannot be read ({e})")))?;
    Ok(Connect::Connected(Client {
        lines,
        hello,
        next_id: 1,
    }))
}

/// D5.1: what a connect that finds no daemon fails with. `ENOTSOCK` is macOS's answer for a
/// file that is not a socket, where Linux answers `ECONNREFUSED`.
fn no_daemon(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    ) || e.raw_os_error() == Some(libc::ENOTSOCK)
}

/// A greeted connection.
#[derive(Debug)]
pub struct Client {
    lines: Lines,
    hello: Hello,
    /// The next request's id (D5.3).
    next_id: i64,
}

impl Client {
    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    /// D5.4's status, waited for at most `REPLY_TIMEOUT`.
    pub fn status(&mut self) -> Result<Status, ClientError> {
        let result = self.request("status")?;
        serde_json::from_value(result)
            .map_err(|e| ClientError::Protocol(format!("the status cannot be read ({e})")))
    }

    /// D5.5: every event from the daemon's `ok` on.
    pub fn subscribe(mut self) -> Result<Events, ClientError> {
        self.request("subscribe")?;
        Ok(Events {
            lines: self.lines,
            done: false,
        })
    }

    /// Sends `op` with the next id and returns its reply's `result`. Events, and replies to
    /// other ids, are passed over (D5.3).
    fn request(&mut self, op: &str) -> Result<Value, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        self.lines.send(&json!({"id": id, "op": op}))?;
        let deadline = Instant::now() + REPLY_TIMEOUT;
        let want = json!(id);
        loop {
            let Some(line) = self.lines.read_line(Some(deadline))? else {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the daemon closed the connection",
                )
                .into());
            };
            let reply: Value = serde_json::from_slice(&line)
                .map_err(|e| ClientError::Protocol(format!("a reply is not JSON ({e})")))?;
            if reply.get("event").is_some() || reply.get("id") != Some(&want) {
                continue;
            }
            return match reply.get("ok").and_then(Value::as_bool) {
                Some(true) => reply
                    .get("result")
                    .cloned()
                    .ok_or_else(|| ClientError::Protocol("a reply has no result".into())),
                Some(false) => Err(ClientError::Refused {
                    kind: reply["error"]["type"].as_str().unwrap_or_default().to_owned(),
                    message: reply["error"]["message"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                }),
                None => Err(ClientError::Protocol("a reply has no ok".into())),
            };
        }
    }
}

/// D5.5's events, typed (D5.7). It waits for as long as the daemon is silent, and ends when the
/// daemon closes the connection: after `stopping`, or when it dropped this subscriber for
/// falling behind, after which the caller re-reads what it shows and subscribes again.
#[derive(Debug)]
pub struct Events {
    lines: Lines,
    done: bool,
}

impl Iterator for Events {
    type Item = Result<Event, ClientError>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.done {
            let line = match self.lines.read_line(None) {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(ClientError::Io(e)) if e.kind() == io::ErrorKind::ConnectionReset => break,
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            };
            match serde_json::from_slice::<Value>(&line) {
                Ok(v) => {
                    // A line that is no event (a stray reply) is passed over.
                    if let Some(event) = parse_event(&v) {
                        return Some(Ok(event));
                    }
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(ClientError::Protocol(format!(
                        "an event is not JSON ({e})"
                    ))));
                }
            }
        }
        self.done = true;
        None
    }
}

/// The connection's lines, each read by a deadline.
#[derive(Debug)]
struct Lines {
    stream: UnixStream,
    /// Bytes read that are not yet a whole line.
    buf: Vec<u8>,
}

impl Lines {
    fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            buf: Vec::new(),
        }
    }

    /// One request, as one line.
    fn send(&mut self, v: &Value) -> io::Result<()> {
        let mut line = v.to_string();
        line.push('\n');
        self.stream.write_all(line.as_bytes())
    }

    /// The next whole line, its newline left off. `None` at the end of the connection; a last
    /// line cut short goes with it. `Timeout` once `deadline` passes; with no deadline it waits
    /// for as long as it takes. A line over `MAX_LINE` is `Protocol`, whether its newline came
    /// in the read that passed the limit or in a later one.
    fn read_line(&mut self, deadline: Option<Instant>) -> Result<Option<Vec<u8>>, ClientError> {
        let mut chunk = [0u8; 8192];
        loop {
            let newline = self.buf.iter().position(|b| *b == b'\n');
            if newline.unwrap_or(self.buf.len()) > MAX_LINE {
                return Err(ClientError::Protocol(
                    "the daemon sent a line over 64 KiB".into(),
                ));
            }
            if let Some(end) = newline {
                let mut line: Vec<u8> = self.buf.drain(..=end).collect();
                line.pop();
                return Ok(Some(line));
            }
            let wait = match deadline {
                Some(deadline) => match deadline.checked_duration_since(Instant::now()) {
                    Some(left) if !left.is_zero() => Some(left),
                    _ => return Err(ClientError::Timeout),
                },
                None => None,
            };
            self.stream.set_read_timeout(wait)?;
            match self.stream.read(&mut chunk) {
                Ok(0) => return Ok(None),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
}
```

- [ ] **Step 4: Run the tests and see them pass**

Run, outside the sandbox: `cargo test -p tagteam-daemon --test client`
Expected: PASS, 14 tests. `status_waits_two_seconds_at_most` takes about 2 s.

Then run `cargo test -p tagteam-daemon --features test-hooks` (outside the sandbox),
`cargo fmt --all`, `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`.
Expected: PASS, and clean.

- [ ] **Step 5: Commit** (the controller commits; implementers do not)

```
git add crates/tagteam-daemon/src/client.rs crates/tagteam-daemon/src/lib.rs \
  crates/tagteam-daemon/tests/client.rs
git commit -m "Add the daemon's client: connect, status and typed events"
```

---

### Task 4: Service units in the engine

**Depends on:** plan A (Task 3's `tagteam_engine::daemon::find_on_path`)

D9 fixes two unit files: a launchd agent and a systemd user unit, each holding the program path,
the argument `daemon`, and the variables D9.1 lists. `doctor` reads them back and asks the
service manager about them (D10), and `doctor` lives in the engine, so the engine owns them
(Decision 4). This task builds `tagteam_engine::service`:
- `unit_path`, and `RECORDED_VARS` with `recorded_env`
- `render_launchd` and `render_systemd`, which escape and quote by each format's rules
- `parse_unit`, which reads back exactly what they render (Decision 5)
- the read-only probes `service_state` and `lingering`
- `run_manager`, the one way the engine and the daemon crate call `launchctl`, `systemctl` and
  `loginctl`

The engine is `#![forbid(unsafe_code)]`, so `getuid(2)` comes from a new
`tagteam_provider::process::current_uid`.

**Readings of the spec this task commits to:**
- **The managers are found on the environment's `PATH`** (`env.var("PATH")`, which the CLI
  captures at its boundary), as `doctor` already finds `claude`. A missing one is `Missing`,
  and nothing is spawned for it. This keeps every engine test that never scripts a manager
  from spawning one, and lets binary tests put a stand-in on `PATH`.
- **What a unit cannot carry.** D9.3 refuses a newline and a NUL in a systemd unit. XML 1.0 has
  no form at all for the C0 controls other than tab, newline and carriage return. A parser
  also normalizes newline and carriage return, so a value holding one cannot round-trip. Both
  units therefore refuse every control character but the tab (`char::is_control`), in the
  program path, a name or a value. The error names the variable or "the program path", never
  the value, because a proxy URL can carry a password.
- **systemd quoting** is D9.3's, and nothing more:
  - `Environment="NAME=value"`: `\` becomes `\\`, `"` becomes `\"`, `%` becomes `%%`, and `$`
    stays.
  - `ExecStart="<path>" daemon`: the same, plus `$` becomes `$$`.
- **`parse_unit` reads, then re-renders and compares.** A unit counts as tagteam's only when
  rendering what was read gives back the same text, byte for byte. Anything else is `None`: a
  hand-edited unit, another tool's, or a different version's layout. `doctor` reports such a
  unit as "installed, environment unknown" (Decision 5).
- **A recorded variable is recorded with the value it has**, an empty one included. "Set" means
  present in the environment (D9.1).
- **`CLAUDE_SECURESTORAGE_CONFIG_DIR` is recorded.** D9.1 lists it, although §12.5's `run` also
  scrubs it (`CC_SCRUB`), and D9.1 says no scrubbed variable is recorded. The list governs
  (Decision 10). The daemon works on the outer home, where this variable places the live
  credential, and `run` removes it only from a session's profile. Task 8 corrects D9.1's
  sentence (erratum a).
- **`launchctl print`.** An exit of 0 means the agent is loaded. Its top-level `state = running`
  means it runs. `last exit code = N` (or `N: NAME`) is its last exit status; `(never exited)`
  gives none. Only the service's own lines count, one brace level in, because nested blocks
  have keys of their own. An agent that is not loaded is reported by exit 113 and the words
  `Could not find service` on current macOS. Either one counts, in case the code differs on a
  later release. Task 10's manual check confirms this on this Mac.
- **`systemctl --user show`** prints `Key=Value` lines in any order:
  - `LoadState=loaded` means loaded.
  - `ActiveState` of `active` or `reloading` means running.
  - `ExecMainStatus` is the last exit status, read only for a loaded unit.
  - No `LoadState` line at all leaves the state unknown.
- **A token of its own.** `run_manager` runs each call under a fresh `Cancel`, bounded by its
  timeout. A probe is short and bounded, and the caller's next cancellation point meets the
  signal. `install`'s unload and load are one change, which a Ctrl-C between them would leave
  half made (Task 5).
- **`lingering` takes the `Env`** to find `loginctl` on its `PATH`, like the other two programs,
  as its first parameter. It answers `Result<bool, String>`: `Err` carries why lingering cannot
  be told, in `service_state`'s words (`why_unknown`), so `doctor` can warn with the reason
  (Review Focus 5) rather than drop it.

**Files:**
- Create: `crates/tagteam-engine/src/service.rs`
- Modify: `crates/tagteam-engine/src/lib.rs` (`pub mod service;` between `pub mod run;` and
  `pub mod session;`)
- Modify: `crates/tagteam-provider/src/process.rs` (`current_uid`, after `exit_code`; a test)
- Modify: `crates/tagteam-provider/src/lib.rs` (re-export `current_uid`)

**Interfaces:**
- Consumes:
  - `tagteam_engine::daemon::find_on_path(name: &str, path_var: Option<&OsStr>) -> Option<PathBuf>`
    (plan A Task 3)
  - `tagteam_provider::{Env, Cancel}`
  - `tagteam_provider::process::{ProcessSpawner, SpawnSpec, Captured, ScriptedSpawner}`
  - `Env::guard`, `Env::var`, `Env::home`, `Env::xdg_config_home`
- Produces (`tagteam_engine::service`), as plan B's Interfaces show it:
  ```rust
  pub const LAUNCHD_LABEL: &str = "io.github.michaelrigart.tagteam";
  pub const SYSTEMD_UNIT: &str = "tagteam.service";
  pub const RECORDED_VARS: &[&str];
  pub enum ServiceKind { Launchd, Systemd }
  impl ServiceKind {
      pub fn for_platform(macos: bool) -> Self;
      pub fn as_str(self) -> &'static str;   // "launchd" | "systemd"
      pub fn noun(self) -> &'static str;     // "launchd agent" | "systemd user unit"
      pub fn label(self) -> &'static str;    // LAUNCHD_LABEL | SYSTEMD_UNIT
  }
  pub fn unit_path(env: &Env, kind: ServiceKind) -> PathBuf;
  pub fn launchd_domain() -> String;         // "gui/<uid>"
  pub fn launchd_target() -> String;         // "gui/<uid>/<label>"
  pub fn recorded_env(var: &dyn Fn(&str) -> Option<OsString>) -> Result<Vec<(String, String)>, UnitError>;
  pub enum UnitError { Unrepresentable(String), NotUtf8(String) }  // any control character but tab
  pub fn render_launchd(program: &Path, env: &[(String, String)]) -> Result<String, UnitError>;
  pub fn render_systemd(program: &Path, env: &[(String, String)]) -> Result<String, UnitError>;
  pub fn render_unit(kind: ServiceKind, program: &Path, env: &[(String, String)]) -> Result<String, UnitError>;
  pub struct UnitContents { pub program: PathBuf, pub env: Vec<(String, String)> }
  pub fn parse_unit(kind: ServiceKind, contents: &str) -> Option<UnitContents>;
  pub enum ManagerRun { Missing, Exited { code: i32, stdout: String, stderr: String }, NoAnswer(String) }
  pub fn run_manager<A: AsRef<OsStr>>(env: &Env, program: &str, args: &[A], spawner: &dyn ProcessSpawner, timeout: Duration) -> ManagerRun;
  pub struct ServiceState { pub installed: bool, pub loaded: Option<bool>, pub running: Option<bool>, pub last_exit_status: Option<i32>, pub unknown: Option<String> }
  pub fn service_state(env: &Env, kind: ServiceKind, spawner: &dyn ProcessSpawner, timeout: Duration) -> ServiceState;
  pub fn lingering(env: &Env, spawner: &dyn ProcessSpawner, user: &str, timeout: Duration) -> Result<bool, String>; // Err: why it cannot be told
  ```
  and `tagteam_provider::process::current_uid() -> u32`, re-exported at the crate root.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-provider/src/process.rs`, add to `mod tests`, after
`the_exit_code_is_the_child_s_or_128_plus_the_signal`:

```rust
    #[test]
    fn the_current_uid_owns_the_files_this_process_creates() {
        use std::os::unix::fs::MetadataExt;
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("mine");
        fs::write(&file, "").unwrap();
        assert_eq!(fs::metadata(&file).unwrap().uid(), current_uid());
    }
```

Create `crates/tagteam-engine/src/service.rs` containing only this test module. Step 3 puts the
implementation above it:

```rust
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::PermissionsExt;

    use tagteam_provider::ScriptedSpawner;

    use super::*;

    const BIN: &str = "/opt/homebrew/bin/tagteam";
    const FIVE: Duration = Duration::from_secs(5);
    const KINDS: [ServiceKind; 2] = [ServiceKind::Launchd, ServiceKind::Systemd];

    fn plain_env() -> Vec<(String, String)> {
        vec![
            ("XDG_DATA_HOME".to_owned(), "/Users/me/.local/share".to_owned()),
            ("PATH".to_owned(), "/opt/homebrew/bin:/usr/bin:/bin".to_owned()),
        ]
    }

    /// D9.2's plist for `BIN` and `plain_env`, key by key.
    const PLIST: &str = concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
        "<plist version=\"1.0\">\n",
        "<dict>\n",
        "\t<key>Label</key>\n",
        "\t<string>io.github.michaelrigart.tagteam</string>\n",
        "\t<key>ProgramArguments</key>\n",
        "\t<array>\n",
        "\t\t<string>/opt/homebrew/bin/tagteam</string>\n",
        "\t\t<string>daemon</string>\n",
        "\t</array>\n",
        "\t<key>EnvironmentVariables</key>\n",
        "\t<dict>\n",
        "\t\t<key>XDG_DATA_HOME</key>\n",
        "\t\t<string>/Users/me/.local/share</string>\n",
        "\t\t<key>PATH</key>\n",
        "\t\t<string>/opt/homebrew/bin:/usr/bin:/bin</string>\n",
        "\t</dict>\n",
        "\t<key>RunAtLoad</key>\n",
        "\t<true/>\n",
        "\t<key>KeepAlive</key>\n",
        "\t<dict>\n",
        "\t\t<key>SuccessfulExit</key>\n",
        "\t\t<false/>\n",
        "\t</dict>\n",
        "\t<key>ProcessType</key>\n",
        "\t<string>Background</string>\n",
        "\t<key>StandardOutPath</key>\n",
        "\t<string>/dev/null</string>\n",
        "\t<key>StandardErrorPath</key>\n",
        "\t<string>/dev/null</string>\n",
        "</dict>\n",
        "</plist>\n",
    );

    /// `PLIST`'s environment block, which an empty environment leaves out.
    const PLIST_ENV: &str = concat!(
        "\t<key>EnvironmentVariables</key>\n",
        "\t<dict>\n",
        "\t\t<key>XDG_DATA_HOME</key>\n",
        "\t\t<string>/Users/me/.local/share</string>\n",
        "\t\t<key>PATH</key>\n",
        "\t\t<string>/opt/homebrew/bin:/usr/bin:/bin</string>\n",
        "\t</dict>\n",
    );

    /// D9.3's unit for `BIN` and `plain_env`.
    const UNIT: &str = concat!(
        "[Unit]\n",
        "Description=tagteam daemon\n",
        "\n",
        "[Service]\n",
        "Type=simple\n",
        "ExecStart=\"/opt/homebrew/bin/tagteam\" daemon\n",
        "Restart=on-failure\n",
        "RestartSec=10\n",
        "StandardOutput=null\n",
        "Environment=\"XDG_DATA_HOME=/Users/me/.local/share\"\n",
        "Environment=\"PATH=/opt/homebrew/bin:/usr/bin:/bin\"\n",
        "\n",
        "[Install]\n",
        "WantedBy=default.target\n",
    );

    fn contents(program: &Path, env: &[(String, String)]) -> Option<UnitContents> {
        Some(UnitContents {
            program: program.to_path_buf(),
            env: env.to_vec(),
        })
    }

    #[test]
    fn each_unit_lives_where_its_manager_looks_and_is_named_as_it_names_it() {
        let mut env = Env::for_test(Path::new("/tmp/fixture"));
        assert_eq!(
            unit_path(&env, ServiceKind::Launchd),
            Path::new(
                "/tmp/fixture/home/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist"
            )
        );
        assert_eq!(
            unit_path(&env, ServiceKind::Systemd),
            Path::new("/tmp/fixture/home/.config/systemd/user/tagteam.service")
        );
        env.xdg_config_home = Some(PathBuf::from("/cfg"));
        assert_eq!(
            unit_path(&env, ServiceKind::Systemd),
            Path::new("/cfg/systemd/user/tagteam.service")
        );
        assert_eq!(ServiceKind::for_platform(true), ServiceKind::Launchd);
        assert_eq!(ServiceKind::for_platform(false), ServiceKind::Systemd);
        let l = ServiceKind::Launchd;
        let s = ServiceKind::Systemd;
        assert_eq!(
            (l.as_str(), l.label(), l.noun()),
            ("launchd", "io.github.michaelrigart.tagteam", "launchd agent")
        );
        assert_eq!(
            (s.as_str(), s.label(), s.noun()),
            ("systemd", "tagteam.service", "systemd user unit")
        );
        assert_eq!(launchd_domain(), format!("gui/{}", current_uid()));
        assert_eq!(
            launchd_target(),
            format!("gui/{}/io.github.michaelrigart.tagteam", current_uid())
        );
    }

    #[test]
    fn the_plist_and_the_unit_are_rendered_key_by_key() {
        // D9.2, D9.3: fixed key order, so a snapshot pins both.
        assert_eq!(render_launchd(Path::new(BIN), &plain_env()).unwrap(), PLIST);
        assert_eq!(render_systemd(Path::new(BIN), &plain_env()).unwrap(), UNIT);
        // No variable set: no EnvironmentVariables key, and no Environment= line.
        assert_eq!(
            render_launchd(Path::new(BIN), &[]).unwrap(),
            PLIST.replace(PLIST_ENV, "")
        );
        assert_eq!(
            render_systemd(Path::new(BIN), &[]).unwrap(),
            UNIT.replace("Environment=\"XDG_DATA_HOME=/Users/me/.local/share\"\n", "")
                .replace("Environment=\"PATH=/opt/homebrew/bin:/usr/bin:/bin\"\n", "")
        );
        for kind in KINDS {
            assert_eq!(
                render_unit(kind, Path::new(BIN), &plain_env()).unwrap(),
                match kind {
                    ServiceKind::Launchd => PLIST,
                    ServiceKind::Systemd => UNIT,
                }
            );
        }
    }

    #[test]
    fn unit_files_carry_a_program_path_with_spaces_and_unicode() {
        // Review Focus 3: the path a person's tools put tagteam at, which both units must
        // carry so the service starts that very file.
        let program =
            Path::new("/Users/Zoë Ångström/Library/Application Support/tagteam bin/tagteam");
        let env = vec![(
            "XDG_DATA_HOME".to_owned(),
            "/Users/Zoë Ångström/Library/Application Support/tagteam data".to_owned(),
        )];
        for kind in KINDS {
            let text = render_unit(kind, program, &env).unwrap();
            assert_eq!(parse_unit(kind, &text), contents(program, &env), "{kind:?}");
        }
        let plist = render_launchd(program, &env).unwrap();
        assert!(
            plist.contains(
                "\t\t<string>/Users/Zoë Ångström/Library/Application Support/tagteam bin/tagteam</string>\n"
            ),
            "{plist}"
        );
        let unit = render_systemd(program, &env).unwrap();
        assert!(
            unit.contains(
                "ExecStart=\"/Users/Zoë Ångström/Library/Application Support/tagteam bin/tagteam\" daemon\n"
            ),
            "{unit}"
        );
    }

    #[test]
    fn dollar_percent_quote_and_backslash_are_quoted_by_each_unit_s_rules() {
        // D9.3, D14: a proxy URL can hold any of them, and must reach the daemon unchanged.
        let proxy = r#"http://us$er:p%40ss"w\rd@proxy.example:8080"#;
        let env = vec![("HTTPS_PROXY".to_owned(), proxy.to_owned())];
        let program = Path::new(r#"/opt/te$t 100%/"q"\tagteam"#);
        let unit = render_systemd(program, &env).unwrap();
        assert!(
            unit.contains(r#"ExecStart="/opt/te$$t 100%%/\"q\"\\tagteam" daemon"#),
            "{unit}"
        );
        assert!(
            unit.contains(
                r#"Environment="HTTPS_PROXY=http://us$er:p%%40ss\"w\\rd@proxy.example:8080""#
            ),
            "{unit}"
        );
        let plist = render_launchd(program, &env).unwrap();
        assert!(
            plist.contains("<string>/opt/te$t 100%/&quot;q&quot;\\tagteam</string>"),
            "{plist}"
        );
        assert!(
            plist.contains("<string>http://us$er:p%40ss&quot;w\\rd@proxy.example:8080</string>"),
            "{plist}"
        );
        for kind in KINDS {
            let text = render_unit(kind, program, &env).unwrap();
            assert_eq!(parse_unit(kind, &text), contents(program, &env), "{kind:?}");
        }
        // XML's other three, and an entity spelled out in the path itself.
        let odd = Path::new("/a&b/<c>/'d'/&amp;");
        let plist = render_launchd(odd, &[]).unwrap();
        assert!(
            plist.contains("<string>/a&amp;b/&lt;c&gt;/&apos;d&apos;/&amp;amp;</string>"),
            "{plist}"
        );
        assert_eq!(parse_unit(ServiceKind::Launchd, &plist), contents(odd, &[]));
    }

    #[test]
    fn a_control_character_or_a_path_that_is_not_utf8_is_refused_and_never_shown() {
        for bad in ["a\nb", "a\0b", "a\rb", "a\x1bb", "a\x7fb"] {
            let env = vec![("HTTPS_PROXY".to_owned(), format!("http://user:{bad}@proxy"))];
            let program = PathBuf::from(format!("/opt/{bad}/tagteam"));
            for kind in KINDS {
                let e = render_unit(kind, Path::new(BIN), &env).unwrap_err();
                assert_eq!(e, UnitError::Unrepresentable("HTTPS_PROXY".into()), "{bad:?}");
                assert!(!e.to_string().contains("user:"), "{e}");
                assert_eq!(
                    render_unit(kind, &program, &[]).unwrap_err(),
                    UnitError::Unrepresentable("the program path".into()),
                    "{bad:?}"
                );
            }
        }
        // A tab is carried.
        for kind in KINDS {
            let program = Path::new("/opt/a\tb/tagteam");
            let text = render_unit(kind, program, &[]).unwrap();
            assert_eq!(parse_unit(kind, &text), contents(program, &[]));
        }
        let program = PathBuf::from(OsString::from_vec(b"/opt/\xff/tagteam".to_vec()));
        for kind in KINDS {
            assert_eq!(
                render_unit(kind, &program, &[]).unwrap_err(),
                UnitError::NotUtf8("the program path".into())
            );
        }
        assert_eq!(
            UnitError::Unrepresentable("HTTPS_PROXY".into()).to_string(),
            "HTTPS_PROXY holds a newline, a NUL or another control character, which a unit file cannot carry"
        );
        assert_eq!(
            UnitError::NotUtf8("XDG_DATA_HOME".into()).to_string(),
            "XDG_DATA_HOME is not valid UTF-8"
        );
    }

    #[test]
    fn recorded_env_keeps_the_list_s_order_and_only_what_is_set() {
        // D9.1: XDG first, then Claude Code's, the proxies, the log and PATH; nothing else.
        let set: BTreeMap<&str, OsString> = [
            ("PATH", "/usr/bin"),
            ("HTTPS_PROXY", "http://p:3128"),
            ("XDG_DATA_HOME", "/d"),
            ("HOME", "/home/me"),
            ("ANTHROPIC_API_KEY", "sk-ant-x"),
            ("NO_PROXY", ""),
            ("https_proxy", "http://q:3128"),
        ]
        .into_iter()
        .map(|(k, v)| (k, OsString::from(v)))
        .collect();
        let pairs = |list: &[(&str, &str)]| -> Vec<(String, String)> {
            list.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        assert_eq!(
            recorded_env(&|k| set.get(k).cloned()).unwrap(),
            pairs(&[
                ("XDG_DATA_HOME", "/d"),
                ("HTTPS_PROXY", "http://p:3128"),
                ("https_proxy", "http://q:3128"),
                ("NO_PROXY", ""),
                ("PATH", "/usr/bin"),
            ])
        );
        assert_eq!(recorded_env(&|_| None).unwrap(), Vec::new());
        let bad = OsString::from_vec(vec![b'/', 0xff]);
        assert_eq!(
            recorded_env(&|k| (k == "XDG_STATE_HOME").then(|| bad.clone())),
            Err(UnitError::NotUtf8("XDG_STATE_HOME".into()))
        );
        assert_eq!(
            RECORDED_VARS,
            [
                "XDG_CONFIG_HOME",
                "XDG_DATA_HOME",
                "XDG_STATE_HOME",
                "CLAUDE_CONFIG_DIR",
                "CLAUDE_SECURESTORAGE_CONFIG_DIR",
                "HTTPS_PROXY",
                "https_proxy",
                "HTTP_PROXY",
                "http_proxy",
                "ALL_PROXY",
                "all_proxy",
                "NO_PROXY",
                "no_proxy",
                "TAGTEAM_LOG",
                "PATH",
            ]
        );
    }

    #[test]
    fn a_unit_tagteam_did_not_write_as_it_stands_is_not_read() {
        // Decision 5: only what render_* writes; anything else is "environment unknown".
        assert_eq!(
            parse_unit(ServiceKind::Launchd, PLIST),
            contents(Path::new(BIN), &plain_env())
        );
        assert_eq!(
            parse_unit(ServiceKind::Systemd, UNIT),
            contents(Path::new(BIN), &plain_env())
        );
        for edited in [
            PLIST.replace("<true/>", "<false/>"),
            PLIST.replace('\t', "    "),
            format!("{PLIST}\n"),
            PLIST.replace("/dev/null", "/tmp/log"),
            String::new(),
        ] {
            assert_eq!(parse_unit(ServiceKind::Launchd, &edited), None, "{edited}");
        }
        for edited in [
            UNIT.replace("Restart=on-failure", "Restart=always"),
            UNIT.replace("ExecStart=\"/opt/homebrew/bin/tagteam\"", "ExecStart=/opt/homebrew/bin/tagteam"),
            format!("# mine\n{UNIT}"),
            String::new(),
        ] {
            assert_eq!(parse_unit(ServiceKind::Systemd, &edited), None, "{edited}");
        }
        assert_eq!(parse_unit(ServiceKind::Systemd, PLIST), None, "another kind's file");
        assert_eq!(parse_unit(ServiceKind::Launchd, UNIT), None, "another kind's file");
    }

    /// A fixture Env whose `PATH` holds an executable file for each of `programs`: enough for
    /// `find_on_path`. None of them ever runs: a scripted spawner answers in their place.
    fn with_managers(root: &Path, programs: &[&str]) -> Env {
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        for p in programs {
            let file = bin.join(p);
            fs::write(&file, "#!/bin/sh\nexit 99\n").unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut env = Env::for_test(root);
        env.vars.insert("PATH".into(), bin.into_os_string());
        env
    }

    fn exited(code: i32, stdout: &str, stderr: &str) -> Captured {
        Captured::Exited {
            code: Some(code),
            signal: None,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    /// `launchctl print` of a loaded agent that runs. A nested block has a `state` of its own,
    /// which is not the service's.
    const RUNNING: &str = "gui/501/io.github.michaelrigart.tagteam = {\n\tactive count = 1\n\tpath = /Users/me/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist\n\ttype = LaunchAgent\n\tstate = running\n\n\tprogram = /opt/homebrew/bin/tagteam\n\targuments = {\n\t\t/opt/homebrew/bin/tagteam\n\t\tdaemon\n\t}\n\n\tendpoints = {\n\t\tstate = active\n\t}\n\tpid = 4242\n\tlast exit code = (never exited)\n}\n";
    /// The same agent stopped after exit 75; its nested block says `running`, which is not the
    /// service's state.
    const STOPPED: &str = "gui/501/io.github.michaelrigart.tagteam = {\n\tactive count = 0\n\tstate = not running\n\tspawn = {\n\t\tstate = running\n\t}\n\tlast exit code = 75: EX_TEMPFAIL\n}\n";

    #[test]
    fn launchctl_print_tells_running_stopped_and_not_loaded() {
        let d = tempfile::tempdir().unwrap();
        let env = with_managers(d.path(), &["launchctl"]);
        let s = ScriptedSpawner::new();
        s.push(exited(0, RUNNING, ""));
        s.push(exited(0, STOPPED, ""));
        s.push(exited(
            113,
            "",
            "Could not find service \"io.github.michaelrigart.tagteam\" in domain for user gui: 501\n",
        ));
        s.push(exited(1, "", "Could not find service \"x\" in domain for user gui: 501"));
        assert_eq!(
            service_state(&env, ServiceKind::Launchd, &s, FIVE),
            ServiceState {
                installed: false,
                loaded: Some(true),
                running: Some(true),
                last_exit_status: None,
                unknown: None,
            }
        );
        let path = unit_path(&env, ServiceKind::Launchd);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, PLIST).unwrap();
        assert_eq!(
            service_state(&env, ServiceKind::Launchd, &s, FIVE),
            ServiceState {
                installed: true,
                loaded: Some(true),
                running: Some(false),
                last_exit_status: Some(75),
                unknown: None,
            }
        );
        let not_loaded = ServiceState {
            installed: true,
            loaded: Some(false),
            running: Some(false),
            last_exit_status: None,
            unknown: None,
        };
        assert_eq!(service_state(&env, ServiceKind::Launchd, &s, FIVE), not_loaded);
        assert_eq!(
            service_state(&env, ServiceKind::Launchd, &s, FIVE),
            not_loaded,
            "the words alone are enough"
        );
        for spec in s.specs() {
            assert_eq!(spec.program, d.path().join("bin/launchctl"));
            assert_eq!(spec.args, args(&["print", launchd_target().as_str()]));
        }
    }

    #[test]
    fn systemctl_show_tells_running_stopped_and_not_found() {
        let d = tempfile::tempdir().unwrap();
        let env = with_managers(d.path(), &["systemctl"]);
        let s = ScriptedSpawner::new();
        s.push(exited(0, "LoadState=loaded\nActiveState=active\nExecMainStatus=0\n", ""));
        s.push(exited(0, "ExecMainStatus=1\nLoadState=loaded\nActiveState=failed\n", ""));
        s.push(exited(0, "LoadState=not-found\nActiveState=inactive\nExecMainStatus=0\n", ""));
        s.push(exited(0, "", ""));
        let state = |s: &ScriptedSpawner| service_state(&env, ServiceKind::Systemd, s, FIVE);
        let st = state(&s);
        assert_eq!(
            (st.loaded, st.running, st.last_exit_status, st.unknown),
            (Some(true), Some(true), Some(0), None)
        );
        let st = state(&s);
        assert_eq!(
            (st.loaded, st.running, st.last_exit_status),
            (Some(true), Some(false), Some(1))
        );
        let st = state(&s);
        assert_eq!(
            (st.loaded, st.running, st.last_exit_status),
            (Some(false), Some(false), None)
        );
        let st = state(&s);
        assert_eq!(
            st.unknown.as_deref(),
            Some("`systemctl --user show tagteam.service` printed no LoadState")
        );
        assert_eq!(st.loaded, None);
        for spec in s.specs() {
            assert_eq!(spec.program, d.path().join("bin/systemctl"));
            assert_eq!(
                spec.args,
                args(&[
                    "--user",
                    "show",
                    "tagteam.service",
                    "-p",
                    "LoadState,ActiveState,ExecMainStatus",
                ])
            );
        }
    }

    #[test]
    fn a_missing_hanging_or_failing_manager_leaves_the_state_unknown_with_its_reason() {
        let d = tempfile::tempdir().unwrap();
        let s = ScriptedSpawner::new();
        let nothing = Env::for_test(d.path());
        for (kind, program) in [
            (ServiceKind::Launchd, "launchctl"),
            (ServiceKind::Systemd, "systemctl"),
        ] {
            let st = service_state(&nothing, kind, &s, FIVE);
            assert_eq!(st.unknown, Some(format!("{program} is not on PATH")));
            assert_eq!((st.loaded, st.running, st.last_exit_status), (None, None, None));
        }
        assert!(s.specs().is_empty(), "nothing is spawned without the program");
        let env = with_managers(d.path(), &["launchctl", "systemctl"]);
        s.push(Captured::TimedOut);
        s.push(exited(1, "", "Failed to connect to bus: No medium found\n"));
        s.push(Captured::SpawnFailed("Permission denied (os error 13)".into()));
        s.push(exited(5, "", ""));
        let unknown = |kind| service_state(&env, kind, &s, FIVE).unknown;
        assert_eq!(
            unknown(ServiceKind::Launchd).as_deref(),
            Some("launchctl did not answer within 5 s")
        );
        assert_eq!(
            unknown(ServiceKind::Systemd).as_deref(),
            Some(
                "`systemctl --user show tagteam.service` exited 1: Failed to connect to bus: No medium found"
            )
        );
        assert_eq!(
            unknown(ServiceKind::Launchd).as_deref(),
            Some("launchctl could not be run: Permission denied (os error 13)")
        );
        assert_eq!(
            unknown(ServiceKind::Launchd),
            Some(format!("`launchctl print {}` exited 5", launchd_target()))
        );
    }

    #[test]
    fn run_manager_finds_its_program_on_the_environment_s_path_and_says_how_it_ended() {
        let d = tempfile::tempdir().unwrap();
        let s = ScriptedSpawner::new();
        assert_eq!(
            run_manager(&Env::for_test(d.path()), "systemctl", &["--user"], &s, FIVE),
            ManagerRun::Missing
        );
        let env = with_managers(d.path(), &["systemctl"]);
        s.push(Captured::Exited {
            code: None,
            signal: Some(9),
            stdout: b"out".to_vec(),
            stderr: b"  killed \n".to_vec(),
        });
        s.push(Captured::Interrupted(2));
        assert_eq!(
            run_manager(&env, "systemctl", &["--user", "daemon-reload"], &s, FIVE),
            ManagerRun::Exited {
                code: 137,
                stdout: "out".into(),
                stderr: "killed".into(),
            }
        );
        assert_eq!(
            run_manager(&env, "systemctl", &["--user"], &s, FIVE),
            ManagerRun::NoAnswer("systemctl was interrupted (signal 2)".into())
        );
        // A token of its own: a signal recorded on the process's token does not cut a call.
        env.cancel.request(2);
        s.push(exited(0, "", ""));
        assert!(matches!(
            run_manager(&env, "systemctl", &["--user"], &s, FIVE),
            ManagerRun::Exited { code: 0, .. }
        ));
        assert_eq!(s.specs()[0].args, args(&["--user", "daemon-reload"]));
    }

    #[test]
    fn lingering_is_read_from_loginctl_and_says_why_it_cannot_be() {
        // Review Focus 5: a `loginctl` that is missing, hangs, fails or answers nothing usable
        // leaves lingering unknown, with the reason, for doctor's warning.
        let d = tempfile::tempdir().unwrap();
        let env = with_managers(d.path(), &["loginctl"]);
        let s = ScriptedSpawner::new();
        s.push(exited(0, "Linger=no\n", ""));
        s.push(exited(0, "Linger=yes\n", ""));
        s.push(exited(1, "", "Failed to get user: User ID 1000 is not logged in or lingering\n"));
        s.push(Captured::TimedOut);
        s.push(exited(0, "Linger=maybe\n", ""));
        assert_eq!(lingering(&env, &s, "tester", FIVE), Ok(false));
        assert_eq!(lingering(&env, &s, "tester", FIVE), Ok(true));
        assert_eq!(
            lingering(&env, &s, "tester", FIVE),
            Err("`loginctl show-user tester -p Linger` exited 1: Failed to get user: User ID 1000 is not logged in or lingering".to_owned())
        );
        assert_eq!(
            lingering(&env, &s, "tester", FIVE),
            Err("loginctl did not answer within 5 s".to_owned())
        );
        assert_eq!(
            lingering(&env, &s, "tester", FIVE),
            Err("`loginctl show-user tester -p Linger` printed no Linger".to_owned())
        );
        assert_eq!(
            lingering(&Env::for_test(d.path()), &s, "tester", FIVE),
            Err("loginctl is not on PATH".to_owned()),
            "no loginctl, and nothing spawned"
        );
        assert_eq!(s.specs().len(), 5);
        assert_eq!(s.specs()[0].args, args(&["show-user", "tester", "-p", "Linger"]));
    }
}
```

In `crates/tagteam-engine/src/lib.rs`, replace:

```rust
pub mod run;
pub mod session;
```

with:

```rust
pub mod run;
pub mod service;
pub mod session;
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-provider --lib current_uid`
Expected: the build fails with `error[E0425]: cannot find function \`current_uid\` in this scope`.

Run: `cargo test -p tagteam-engine --lib service::`
Expected: the build fails. The test module is the whole file, so it reports `E0425` / `E0433`
for `render_launchd`, `render_systemd`, `render_unit`, `parse_unit`, `unit_path`,
`service_state`, `lingering`, `run_manager`, `ServiceKind`, `UnitError`, `UnitContents`,
`ServiceState`, `ManagerRun`, `RECORDED_VARS`, `recorded_env`, `launchd_domain`,
`launchd_target`, `current_uid`, `Env`, `Path`, `PathBuf`, `Duration`, `OsString` and
`Captured`.

- [ ] **Step 3: Implement**

In `crates/tagteam-provider/src/process.rs`, after `exit_code`:

```rust
/// §12.5: the child's code, `128 + signal` when a signal ended it.
pub fn exit_code(status: ExitStatus) -> i32 {
    match (status.code(), status.signal()) {
        (Some(code), _) => code,
        (None, Some(signal)) => 128 + signal,
        (None, None) => 1,
    }
}
```

add:

```rust

/// This process's real user ID (`getuid(2)`). D9.2: launchd runs a user's agents in the
/// `gui/<uid>` domain. D9.3: `loginctl` is asked about this uid when `$USER` is unset.
pub fn current_uid() -> u32 {
    // SAFETY: getuid(2) takes no arguments, has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}
```

In `crates/tagteam-provider/src/lib.rs`, replace:

```rust
pub use process::{
    Captured, Liveness, ProcessSpawner, ProcessStamp, ScriptedSpawner, SpawnSpec, SystemSpawner,
    exec_command, exit_code, find_on_path, run_captured, spawn_session,
};
```

with:

```rust
pub use process::{
    Captured, Liveness, ProcessSpawner, ProcessStamp, ScriptedSpawner, SpawnSpec, SystemSpawner,
    current_uid, exec_command, exit_code, find_on_path, run_captured, spawn_session,
};
```

In `crates/tagteam-engine/src/service.rs`, insert above `#[cfg(test)]`:

```rust
//! D9's service units. `tagteam daemon install` and `uninstall` (the daemon crate) write and
//! load them, and `doctor` (D10) reads them, so they live in the engine (plan B Decision 4).
//! This module holds:
//! - where each unit lives, and the variables it records (D9.1)
//! - rendering and quoting (D9.2, D9.3), and a reader for exactly what is rendered
//!   (Decision 5)
//! - a read-only probe of what the service manager says of the unit
//!
//! `launchctl`, `systemctl` and `loginctl` are found on the environment's `PATH`, as `doctor`
//! finds `claude`. Each call goes through the `ProcessSpawner` port, bounded by the caller's
//! timeout (D9.4, D10). Each program is optional (D1.1): without it, what it would tell is
//! unknown, and the state says why.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tagteam_provider::process::{Captured, ProcessSpawner, SpawnSpec, current_uid};
use tagteam_provider::{Cancel, Env};

use crate::daemon::find_on_path;

/// D9.2: the agent's label, and its plist's name.
pub const LAUNCHD_LABEL: &str = "io.github.michaelrigart.tagteam";
/// D9.3: the user unit's name.
pub const SYSTEMD_UNIT: &str = "tagteam.service";

/// D9.1: the variables a unit records, in this order, each only when it is set at install time.
/// They make the daemon resolve the same store, Claude Code home, proxy and log as the shell
/// that installed it. `PATH` is where it finds `notify-send` (D6.2). No other variable is
/// recorded.
pub const RECORDED_VARS: &[&str] = &[
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_SECURESTORAGE_CONFIG_DIR",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
    "TAGTEAM_LOG",
    "PATH",
];

/// What an error calls the program path. A variable is called by its name. Neither error ever
/// shows the value, because a proxy URL can carry a password.
const PROGRAM_PATH: &str = "the program path";
/// The words `launchctl print` uses for an agent launchd has not loaded.
const NOT_FOUND: &str = "Could not find service";

/// The service manager of a platform this build runs on (D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    Launchd,
    Systemd,
}

impl ServiceKind {
    /// launchd on macOS, systemd on every other platform this build runs on (Linux).
    pub fn for_platform(macos: bool) -> Self {
        if macos {
            ServiceKind::Launchd
        } else {
            ServiceKind::Systemd
        }
    }

    /// The JSON `kind` of `daemon status` and `install` (D8.2, D9.4).
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceKind::Launchd => "launchd",
            ServiceKind::Systemd => "systemd",
        }
    }

    /// What a person calls the unit, in messages.
    pub fn noun(self) -> &'static str {
        match self {
            ServiceKind::Launchd => "launchd agent",
            ServiceKind::Systemd => "systemd user unit",
        }
    }

    /// The name the manager knows the unit by: the agent's label, or the unit's file name.
    pub fn label(self) -> &'static str {
        match self {
            ServiceKind::Launchd => LAUNCHD_LABEL,
            ServiceKind::Systemd => SYSTEMD_UNIT,
        }
    }
}

/// D9.2: `~/Library/LaunchAgents/<label>.plist`. D9.3:
/// `$XDG_CONFIG_HOME/systemd/user/tagteam.service`, `~/.config` standing in for an unset
/// `XDG_CONFIG_HOME` as systemd reads it.
pub fn unit_path(env: &Env, kind: ServiceKind) -> PathBuf {
    let path = match kind {
        ServiceKind::Launchd => env
            .home
            .join("Library/LaunchAgents")
            .join(format!("{LAUNCHD_LABEL}.plist")),
        ServiceKind::Systemd => env
            .xdg_config_home
            .clone()
            .unwrap_or_else(|| env.home.join(".config"))
            .join("systemd/user")
            .join(SYSTEMD_UNIT),
    };
    env.guard(path)
}

/// `gui/<uid>`: the launchd domain of this user's login session (D9.2).
pub fn launchd_domain() -> String {
    format!("gui/{}", current_uid())
}

/// `gui/<uid>/<label>`: the daemon's agent in that domain.
pub fn launchd_target() -> String {
    format!("{}/{LAUNCHD_LABEL}", launchd_domain())
}

/// Why a unit cannot be written. It names the variable, or the program path, never the value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UnitError {
    /// D9.3 refuses a newline and a NUL. XML 1.0, which the plist is written in, cannot carry
    /// any other control character either.
    #[error(
        "{0} holds a newline, a NUL or another control character, which a unit file cannot carry"
    )]
    Unrepresentable(String),
    #[error("{0} is not valid UTF-8")]
    NotUtf8(String),
}

/// D9.1: the variables of `RECORDED_VARS` that `var` finds set, in that order, each with its
/// value as set, an empty one included. A value that is not UTF-8 cannot go into a unit.
pub fn recorded_env(
    var: &dyn Fn(&str) -> Option<OsString>,
) -> Result<Vec<(String, String)>, UnitError> {
    let mut recorded = Vec::new();
    for name in RECORDED_VARS {
        if let Some(value) = var(name) {
            let value = value
                .into_string()
                .map_err(|_| UnitError::NotUtf8((*name).to_owned()))?;
            recorded.push(((*name).to_owned(), value));
        }
    }
    Ok(recorded)
}

/// Refuses every control character but the tab in `text`, which `name` names.
fn carried(name: &str, text: &str) -> Result<(), UnitError> {
    if text.chars().any(|c| c.is_control() && c != '\t') {
        return Err(UnitError::Unrepresentable(name.to_owned()));
    }
    Ok(())
}

/// The program path as a unit's text: UTF-8, and carried.
fn program_text(program: &Path) -> Result<&str, UnitError> {
    let text = program
        .to_str()
        .ok_or_else(|| UnitError::NotUtf8(PROGRAM_PATH.to_owned()))?;
    carried(PROGRAM_PATH, text)?;
    Ok(text)
}

/// XML 1.0's five escapes, for a plist `<key>` or `<string>`.
fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// The inverse of `xml`, `&amp;` last so an escaped entity stays one. `parse_unit` checks the
/// result by rendering it again.
fn unxml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// D9.2: the agent. Its keys are in a fixed order, indented with tabs as `plutil` writes them:
/// - `Label`
/// - `ProgramArguments` `[program, "daemon"]`
/// - `EnvironmentVariables` (left out when there is none)
/// - `RunAtLoad` true
/// - `KeepAlive` `{SuccessfulExit: false}`: restarted after a failure and after exit 75,
///   never after exit 0
/// - `ProcessType` `Background`
/// - `StandardOutPath` and `StandardErrorPath` `/dev/null`, since the log file is the record
pub fn render_launchd(program: &Path, env: &[(String, String)]) -> Result<String, UnitError> {
    let program = program_text(program)?;
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str("<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n");
    s.push_str("<plist version=\"1.0\">\n<dict>\n");
    s.push_str(&format!("\t<key>Label</key>\n\t<string>{LAUNCHD_LABEL}</string>\n"));
    s.push_str("\t<key>ProgramArguments</key>\n\t<array>\n");
    s.push_str(&format!("\t\t<string>{}</string>\n", xml(program)));
    s.push_str("\t\t<string>daemon</string>\n\t</array>\n");
    if !env.is_empty() {
        s.push_str("\t<key>EnvironmentVariables</key>\n\t<dict>\n");
        for (name, value) in env {
            carried(name, name)?;
            carried(name, value)?;
            s.push_str(&format!(
                "\t\t<key>{}</key>\n\t\t<string>{}</string>\n",
                xml(name),
                xml(value)
            ));
        }
        s.push_str("\t</dict>\n");
    }
    s.push_str("\t<key>RunAtLoad</key>\n\t<true/>\n");
    s.push_str("\t<key>KeepAlive</key>\n\t<dict>\n");
    s.push_str("\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>\n");
    s.push_str("\t<key>ProcessType</key>\n\t<string>Background</string>\n");
    s.push_str("\t<key>StandardOutPath</key>\n\t<string>/dev/null</string>\n");
    s.push_str("\t<key>StandardErrorPath</key>\n\t<string>/dev/null</string>\n");
    s.push_str("</dict>\n</plist>\n");
    Ok(s)
}

/// D9.3's quoting, double quotes around:
/// - `Environment=` (systemd.exec(5)): `\` and `"` are escaped, and `%` is written `%%`, since
///   specifiers expand there. `$` stays, since `Environment=` expands no variables.
/// - `ExecStart=` (systemd.service(5), with `exec`): the same, and `$` is written `$$`, since a
///   command line also expands variables.
fn quoted(text: &str, exec: bool) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' if exec => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The inverse of `quoted`; `None` for text it never writes. `parse_unit` checks the result by
/// rendering it again.
fn unquote(text: &str, exec: bool) -> Option<String> {
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next()?),
            '%' => {
                if chars.next()? != '%' {
                    return None;
                }
                out.push('%');
            }
            '$' if exec => {
                if chars.next()? != '$' {
                    return None;
                }
                out.push('$');
            }
            c => out.push(c),
        }
    }
    Some(out)
}

/// D9.3: the user unit, with one `Environment=` line per recorded variable, in order. Its
/// standard error follows its standard output (systemd's default `StandardError=inherit`),
/// so both are discarded and the log file is the record (D11).
pub fn render_systemd(program: &Path, env: &[(String, String)]) -> Result<String, UnitError> {
    let program = program_text(program)?;
    let mut s = String::from("[Unit]\nDescription=tagteam daemon\n\n[Service]\nType=simple\n");
    s.push_str(&format!("ExecStart={} daemon\n", quoted(program, true)));
    s.push_str("Restart=on-failure\nRestartSec=10\nStandardOutput=null\n");
    for (name, value) in env {
        carried(name, name)?;
        carried(name, value)?;
        s.push_str(&format!(
            "Environment={}\n",
            quoted(&format!("{name}={value}"), false)
        ));
    }
    s.push_str("\n[Install]\nWantedBy=default.target\n");
    Ok(s)
}

/// The unit of `kind`.
pub fn render_unit(
    kind: ServiceKind,
    program: &Path,
    env: &[(String, String)],
) -> Result<String, UnitError> {
    match kind {
        ServiceKind::Launchd => render_launchd(program, env),
        ServiceKind::Systemd => render_systemd(program, env),
    }
}

/// What a unit tagteam wrote starts, and with which variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitContents {
    pub program: PathBuf,
    pub env: Vec<(String, String)>,
}

/// Decision 5: the program and variables of a unit exactly as `render_*` wrote it. It returns
/// `None` for any other text: a hand-edited unit, another tool's, another kind's. `doctor`
/// reports such a unit as installed, with its environment unknown. What is read is rendered
/// again and must give back `contents` byte for byte, so no near miss is ever taken for
/// tagteam's.
pub fn parse_unit(kind: ServiceKind, contents: &str) -> Option<UnitContents> {
    let read = match kind {
        ServiceKind::Launchd => read_launchd(contents)?,
        ServiceKind::Systemd => read_systemd(contents)?,
    };
    let again = render_unit(kind, &read.program, &read.env).ok()?;
    (again == contents).then_some(read)
}

/// The program and variables where `render_launchd` puts them.
fn read_launchd(text: &str) -> Option<UnitContents> {
    let (_, rest) = text.split_once("\t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>")?;
    let (program, rest) = rest.split_once("</string>\n")?;
    let mut env = Vec::new();
    if let Some((_, block)) = rest.split_once("\t<key>EnvironmentVariables</key>\n\t<dict>\n") {
        let (block, _) = block.split_once("\t</dict>\n")?;
        let mut lines = block.lines();
        while let Some(key) = lines.next() {
            let key = key.strip_prefix("\t\t<key>")?.strip_suffix("</key>")?;
            let value = lines
                .next()?
                .strip_prefix("\t\t<string>")?
                .strip_suffix("</string>")?;
            env.push((unxml(key), unxml(value)));
        }
    }
    Some(UnitContents {
        program: PathBuf::from(unxml(program)),
        env,
    })
}

/// The program and variables where `render_systemd` puts them.
fn read_systemd(text: &str) -> Option<UnitContents> {
    let mut program = None;
    let mut env = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("ExecStart=") {
            program = Some(unquote(rest.strip_suffix(" daemon")?, true)?);
        } else if let Some(rest) = line.strip_prefix("Environment=") {
            let assignment = unquote(rest, false)?;
            let (name, value) = assignment.split_once('=')?;
            env.push((name.to_owned(), value.to_owned()));
        }
    }
    Some(UnitContents {
        program: PathBuf::from(program?),
        env,
    })
}

/// How one call of `launchctl`, `systemctl` or `loginctl` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagerRun {
    /// The program is not on the environment's `PATH`, so nothing ran.
    Missing,
    /// It exited. `code` is its exit code, or 128 + the signal when one ended it. `stderr` is
    /// trimmed.
    Exited {
        code: i32,
        stdout: String,
        stderr: String,
    },
    /// It gave no exit: it ran past the timeout, could not be started, or was interrupted. The
    /// reason names the program.
    NoAnswer(String),
}

/// Runs `program`, found on `env`'s `PATH`, with `args` through `spawner`, bounded by `timeout`.
/// It runs under a token of its own: a probe is short and bounded, and the caller's next
/// cancellation point meets a signal. `install`'s unload and load are one change, which a
/// signal between them would leave half made. The child gets this process's environment,
/// which `systemctl --user` needs to find the user's manager.
pub fn run_manager<A: AsRef<OsStr>>(
    env: &Env,
    program: &str,
    args: &[A],
    spawner: &dyn ProcessSpawner,
    timeout: Duration,
) -> ManagerRun {
    let Some(path) = find_on_path(program, env.var("PATH")) else {
        return ManagerRun::Missing;
    };
    let spec = SpawnSpec {
        program: path,
        args: args.iter().map(|a| a.as_ref().to_os_string()).collect(),
        ..SpawnSpec::default()
    };
    match spawner.run_captured(&spec, timeout, &Cancel::new()) {
        Captured::Exited {
            code,
            signal,
            stdout,
            stderr,
        } => ManagerRun::Exited {
            code: code.unwrap_or_else(|| 128 + signal.unwrap_or_default()),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).trim().to_owned(),
        },
        Captured::TimedOut => ManagerRun::NoAnswer(format!(
            "{program} did not answer within {} s",
            timeout.as_secs()
        )),
        Captured::SpawnFailed(e) => ManagerRun::NoAnswer(format!("{program} could not be run: {e}")),
        Captured::Interrupted(signal) => {
            ManagerRun::NoAnswer(format!("{program} was interrupted (signal {signal})"))
        }
    }
}

/// What the service manager says of the daemon's unit (D8.2, D10). Each `Option` is `None` when
/// it cannot be told, and `unknown` then says why.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServiceState {
    /// The unit's file exists.
    pub installed: bool,
    pub loaded: Option<bool>,
    pub running: Option<bool>,
    pub last_exit_status: Option<i32>,
    /// Why the manager's view is unknown: it is missing, it hung, or it failed.
    pub unknown: Option<String>,
}

/// D8.2, D10: whether the unit's file exists, and the manager's view of it from
/// `launchctl print gui/<uid>/<label>` or `systemctl --user show tagteam.service`. Neither
/// command changes anything. A manager that is missing, hangs past `timeout` or fails leaves
/// `loaded`, `running` and `last_exit_status` unknown, and `unknown` gives the reason.
pub fn service_state(
    env: &Env,
    kind: ServiceKind,
    spawner: &dyn ProcessSpawner,
    timeout: Duration,
) -> ServiceState {
    let mut state = ServiceState {
        installed: unit_path(env, kind).exists(),
        ..ServiceState::default()
    };
    match kind {
        ServiceKind::Launchd => {
            let target = launchd_target();
            match run_manager(env, "launchctl", &["print", target.as_str()], spawner, timeout) {
                ManagerRun::Exited { code: 0, stdout, .. } => {
                    let (running, last) = launchctl_print(&stdout);
                    state.loaded = Some(true);
                    state.running = Some(running);
                    state.last_exit_status = last;
                }
                // An agent launchd has not loaded: exit 113 and "Could not find service" on
                // current macOS. The words count alone too, should the code differ.
                ManagerRun::Exited {
                    code,
                    stdout,
                    stderr,
                } if code == 113 || stdout.contains(NOT_FOUND) || stderr.contains(NOT_FOUND) => {
                    state.loaded = Some(false);
                    state.running = Some(false);
                }
                other => {
                    let command = format!("launchctl print {target}");
                    state.unknown = Some(why_unknown("launchctl", &command, other));
                }
            }
        }
        ServiceKind::Systemd => {
            let command = format!("systemctl --user show {SYSTEMD_UNIT}");
            let args = [
                "--user",
                "show",
                SYSTEMD_UNIT,
                "-p",
                "LoadState,ActiveState,ExecMainStatus",
            ];
            match run_manager(env, "systemctl", &args, spawner, timeout) {
                ManagerRun::Exited { code: 0, stdout, .. } => match systemctl_show(&stdout) {
                    Some((loaded, running, status)) => {
                        state.loaded = Some(loaded);
                        state.running = Some(running);
                        state.last_exit_status = status;
                    }
                    None => state.unknown = Some(format!("`{command}` printed no LoadState")),
                },
                other => state.unknown = Some(why_unknown("systemctl", &command, other)),
            }
        }
    }
    state
}

/// Why a manager's answer gives no state. `command` is what ran, for an exit's message.
fn why_unknown(program: &str, command: &str, run: ManagerRun) -> String {
    match run {
        ManagerRun::Missing => format!("{program} is not on PATH"),
        ManagerRun::NoAnswer(why) => why,
        ManagerRun::Exited { code, stderr, .. } if stderr.is_empty() => {
            format!("`{command}` exited {code}")
        }
        ManagerRun::Exited { code, stderr, .. } => format!("`{command}` exited {code}: {stderr}"),
    }
}

/// From `launchctl print`: whether the service's own `state` is `running`, and its
/// `last exit code` (`N`, or `N: NAME`; `(never exited)` gives none). Only lines one brace
/// level in count, since nested blocks have keys of their own.
fn launchctl_print(stdout: &str) -> (bool, Option<i32>) {
    let (mut depth, mut running, mut last) = (0usize, false, None);
    for line in stdout.lines() {
        let line = line.trim();
        if line == "}" {
            depth = depth.saturating_sub(1);
            continue;
        }
        if depth == 1
            && let Some((key, value)) = line.split_once(" = ")
        {
            match key {
                "state" => running = value == "running",
                "last exit code" => {
                    last = value
                        .split(|c: char| c == ':' || c.is_whitespace())
                        .next()
                        .and_then(|n| n.parse().ok());
                }
                _ => {}
            }
        }
        if line.ends_with('{') {
            depth += 1;
        }
    }
    (running, last)
}

/// From `systemctl --user show -p LoadState,ActiveState,ExecMainStatus`: loaded
/// (`LoadState=loaded`), running (`ActiveState` `active` or `reloading`), and a loaded unit's
/// `ExecMainStatus`. `None` without a `LoadState` line.
fn systemctl_show(stdout: &str) -> Option<(bool, bool, Option<i32>)> {
    let (mut load, mut active, mut status) = (None, None, None);
    for line in stdout.lines() {
        match line.split_once('=') {
            Some(("LoadState", v)) => load = Some(v.trim().to_owned()),
            Some(("ActiveState", v)) => active = Some(v.trim().to_owned()),
            Some(("ExecMainStatus", v)) => status = v.trim().parse().ok(),
            _ => {}
        }
    }
    let loaded = load? == "loaded";
    let running = loaded && matches!(active.as_deref(), Some("active" | "reloading"));
    Some((loaded, running, status.filter(|_| loaded)))
}

/// D9.3: whether lingering is on for `user`, by `loginctl show-user <user> -p Linger`. `Err`
/// says why that cannot be told, as `ServiceState::unknown` does: no `loginctl`, a hang, a
/// failure (a user who is neither logged in nor lingering among them), or an answer with no
/// `Linger` in it.
pub fn lingering(
    env: &Env,
    spawner: &dyn ProcessSpawner,
    user: &str,
    timeout: Duration,
) -> Result<bool, String> {
    let command = format!("loginctl show-user {user} -p Linger");
    match run_manager(
        env,
        "loginctl",
        &["show-user", user, "-p", "Linger"],
        spawner,
        timeout,
    ) {
        ManagerRun::Exited { code: 0, stdout, .. } => match stdout.trim() {
            "Linger=yes" => Ok(true),
            "Linger=no" => Ok(false),
            _ => Err(format!("`{command}` printed no Linger")),
        },
        other => Err(why_unknown("loginctl", &command, other)),
    }
}

```

`launchctl_print` uses a let chain (`if depth == 1 && let Some(..) = ..`), as `doctor.rs`'s
`log` check already does.

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-provider --lib current_uid`. Expected: PASS, 1 test.
Run: `cargo test -p tagteam-engine --lib service::`. Expected: PASS, 12 tests.
Then `cargo test -p tagteam-provider` and `cargo test -p tagteam-engine`. Expected: PASS.
Nothing outside the module uses it yet.
Then `cargo fmt --all && cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`.
Expected: clean.

- [ ] **Step 5: Commit** (the controller commits; implementers do not)

```
git add crates/tagteam-engine/src/service.rs crates/tagteam-engine/src/lib.rs \
  crates/tagteam-provider/src/process.rs crates/tagteam-provider/src/lib.rs
git commit -m "Render, read back and probe the daemon's service units"
```

---

### Task 5: `tagteam daemon install`, `uninstall` and `status`

**Depends on:** 1, 3, 4, 6 (Task 6 adds `App::service()`, and the capture of `RECORDED_VARS` into
`Env.vars` at the process boundary, so Task 6 runs first: the order is 1, 2, 3, 4, 6, 5)

D9 and D8.2 as commands:
- `install` writes the unit (0600, `O_EXCL`, write, rename) and loads it, restarting a daemon
  it already runs. `--no-start` writes and enables it only; `--print` prints it and changes
  nothing.
- `uninstall` stops the daemon and removes the unit.
- `status` asks the daemon's socket for its `status`. Without a daemon it reads `daemon.lock`'s
  record and the service manager's view. It exits 0 when a daemon runs, 3 when none does, and
  1 on an error (Decision 6).

The work is split in two:
- **The daemon crate's `service.rs`** runs `launchctl` and `systemctl` over the engine's units
  (Decision 4). Its unit tests drive it with a `ScriptedSpawner` and check every argv.
- **The CLI** renders the results. Its binary tests put stand-in `launchctl`, `systemctl` and
  `loginctl` scripts on `PATH`, so no test ever touches this machine's launchd or systemd.

**Readings of the spec this task commits to:**
- **`bootout` fails only for a reason other than "not loaded".** "Not loaded" is its usual case
  on a first install, and on an `uninstall` after a logout. macOS reports it with a non-zero
  exit: 113, "Could not find specified service", on current releases, and 3, "No such
  process", on older ones. Either the code or those words counts, should one of them differ on
  a later release; such an exit is logged at DEBUG, by its code only, and is no error. Any
  other exit leaves the agent loaded and running, so it is an error with `launchctl`'s status
  and words (`InstallError::Manager`), and so is a `launchctl` that gives no answer
  (`NoAnswer`). `install` and `uninstall` share this one rule (`bootout`): `uninstall` then
  keeps the plist, since the agent still runs from it, and `install` never runs `bootstrap`
  over a loaded old agent. Task 10's manual check confirms the codes on this Mac.
- **A failed `systemctl --user disable --now`** stops `uninstall` the same way: the unit is
  still enabled and may still run, so its file stays, and no `daemon-reload` follows. Only the
  reload after a removal is merely logged, since the unit is already gone by then.
- **`--no-start` on macOS runs nothing.** The plist is written, and launchd loads it at the
  next login (D9: "starts or stops nothing"). On Linux, `--no-start` runs `daemon-reload` and
  `enable`, so the unit starts at the next login. Neither runs `bootout`, `bootstrap` or
  `restart`.
- **Nothing is written when the manager is missing** (D9.3: "Without `systemctl`, `install`
  fails and names `--print`"). The same holds on macOS, where `launchctl` is always there.
- **A unit written before a failed load stays.** If `bootstrap` or `restart` fails, the new
  unit is in place and is used at the next login. The error says what failed.
- **`bootstrap`'s failure names the GUI session** (D9.2): launchd loads an agent only in a GUI
  login session. The CLI's message adds that, and suggests running `tagteam daemon` in the
  foreground.
- **`InstallError::NoAnswer`** is a new variant for a manager that timed out or could not be
  started. D9.4's "stderr and exit status" has neither in that case.
- **Warnings** are each one sentence: the running daemon kept by `--no-start` (D9), and
  lingering off (D9.3). Human output prints them on stderr as `warning: …`. `--json` carries
  them in `warnings`.
- **The program path** is D9.1's alone: the first `tagteam` on `PATH` when it is this binary,
  else `current_exe`. `program_path` is given a relative argv[0], `tagteam`, so D4.5's
  preference for an absolute argv[0] does not apply. That preference is for the daemon's own
  start.
- **The recorded variables come from `Env.vars`**, which Task 6 fills at the process boundary,
  not from the process environment. This holds for `install` and `--print` alike.
- **The run-shell refusal covers `--print`** (Decision 15). Inside a run shell, the process's
  `CLAUDE_CONFIG_DIR` names a session's profile. A unit printed there would record the wrong
  home. Root is refused for every command already.
- **`--print` with `--json`** prints `{schemaVersion, service: {kind, path, label}, unit}`, so
  "all four take `--json`" (D8) holds.
- **`status`**:
  - It looks for the socket file before it connects. A path too long to bind can never exist,
    and `connect` would report it as an error rather than find no daemon (D5.1).
  - A daemon that does not greet, or does not answer `status`, in time (Task 3's
    `ClientError::Timeout`) is there but hung. With a live holder of `daemon.lock`, that is
    D8.2's "holds `daemon.lock` but does not answer": running, with the lock's pid. Without one,
    whatever listens is no daemon tagteam knows, and `status` fails with exit 1 (Decision 14).
  - `--json` adds a top-level `pid`: the daemon's, the lock holder's, or null (D8.2: "with the
    pid from the lock's record"; Decision 14).
  - The `daemon` object is D5.4's, rebuilt from the typed `Status`, with every field present
    and `null` when absent. Task 1's `status_json` writes the same, nulls included, so the two
    agree.
  - A protocol mismatch counts as running, since a daemon runs: it restarts on the new binary
    within a minute (D4.5, D5.6).
  - `service` has exactly D8.2's keys. The reason a state is unknown appears in the human form
    only.
- **D14's real round trip** is one `#[ignore]`d test,
  `a_real_service_manager_starts_and_stops_the_daemon`, for the platform it runs on. launchd
  reads agents only from `~/Library/LaunchAgents`, and the systemd user manager reads units only
  from its own search path, so the test installs into the real unit directory under the real
  label, with this user's environment: it breaks §15.1's isolation on purpose, which is why it
  is ignored. It stops at once when a tagteam unit is installed, and the unit records a
  fixture's store, state and Claude Code home, so the daemon it starts touches no account.
  Task 10 runs it only with Michael's go-ahead.
- **Rendering lives in a new `service_cmd.rs`**, not in plan A's `daemon_cmd.rs`. Neither file's
  imports and helpers can then collide with the other's. For the same reason
  the binary tests are in a new `tests/daemon_service_cli.rs`.

**Files:**
- Create: `crates/tagteam-daemon/src/service.rs`
- Modify: `crates/tagteam-daemon/src/lib.rs` (`pub mod service;` among the modules, in order)
- Modify: `crates/tagteam/src/cli.rs` (`Command::Daemon` gains `action`; `DaemonAction`; a
  parse test)
- Modify: `crates/tagteam/src/app.rs` (every `Command::Daemon {}` pattern; the dispatch arm;
  `daemon_install`, `daemon_uninstall`, `daemon_status`; `install_failure`; three error
  kinds)
- Create: `crates/tagteam/src/service_cmd.rs`
- Modify: `crates/tagteam/src/lib.rs` (`mod service_cmd;` after `mod run;`)
- Test: `crates/tagteam/tests/daemon_service_cli.rs` (new)

**Interfaces:**
- Consumes:
  - From Task 4 (`tagteam_engine::service`):
    - `ServiceKind` (`as_str`, `noun`, `label`), `SYSTEMD_UNIT`
    - `unit_path`, `launchd_domain`, `launchd_target`
    - `recorded_env`, `render_unit`, `parse_unit`
    - `run_manager`, `ManagerRun`, `service_state`, `ServiceState`, `lingering`, `UnitError`
  - From Task 4: `tagteam_provider::process::current_uid`.
  - From Task 3 (`tagteam_daemon::client`): `connect(env) -> Result<Connect, ClientError>`,
    `Connect::{Connected, NoDaemon, Mismatch { server, client }}`, `Client::status(&mut self)`.
  - From Task 1 (`tagteam_daemon::protocol`): `Status`, `StatusDaemon`, `StatusNotifications`,
    `StatusProvider`, `StatusBackoff`, `StatusOutcome`, `StatusSwitch` and `StatusJob`, with
    the fields of plan B's Interfaces.
  - From plan A:
    - `tagteam_engine::daemon::{daemon_lock_path, daemon_socket_path, read_daemon_record,
      find_on_path}`, `DaemonRecord`
    - `tagteam_daemon::watch::program_path`
    - `Engine::spawner() -> Arc<dyn ProcessSpawner>`
    - `Command::Daemon {}`, the foreground run
  - From Task 6: `App::service() -> ServiceKind`, and `RECORDED_VARS` captured into `Env.vars`
    by `Context::from_process`.
- Produces:
  ```rust
  // tagteam-daemon/src/service.rs
  pub const MANAGER_TIMEOUT: Duration = Duration::from_secs(10);
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct InstallReport { pub kind: ServiceKind, pub path: PathBuf, pub label: String, pub started: bool, pub warnings: Vec<String> }
  #[derive(Debug, thiserror::Error)]
  pub enum InstallError {
      #[error(transparent)] Unit(#[from] UnitError),
      #[error(transparent)] Io(#[from] std::io::Error),
      #[error("{program} failed (exit {status}): {stderr}")] Manager { program: String, status: String, stderr: String },
      #[error("{program} gave no answer: {reason}")] NoAnswer { program: String, reason: String },
      #[error("{0} was not found; `tagteam daemon install --print` prints the unit to install yourself")] NoManager(String),
  }
  pub fn install(env: &Env, kind: ServiceKind, program: &Path, recorded: &[(String, String)], start: bool, spawner: &dyn ProcessSpawner) -> Result<InstallReport, InstallError>;
  pub fn uninstall(env: &Env, kind: ServiceKind, spawner: &dyn ProcessSpawner) -> Result<bool, InstallError>;

  // tagteam/src/cli.rs
  Command::Daemon { #[command(subcommand)] action: Option<DaemonAction> }
  pub enum DaemonAction { Install { print: bool, no_start: bool }, Uninstall, Status }
  ```

#### Part A: install and uninstall in the daemon crate

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-daemon/src/service.rs` containing only this test module. Step 3 puts the
implementation above it:

```rust
#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    use tagteam_engine::service::render_unit;
    use tagteam_provider::Cancel;
    use tagteam_provider::process::{Captured, ScriptedSpawner, SpawnSpec};

    use super::*;

    const ALL: [&str; 3] = ["launchctl", "systemctl", "loginctl"];
    const KINDS: [ServiceKind; 2] = [ServiceKind::Launchd, ServiceKind::Systemd];

    fn exited(code: i32, stdout: &str, stderr: &str) -> Captured {
        Captured::Exited {
            code: Some(code),
            signal: None,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// A fixture Env whose `PATH` holds an executable file for each of `programs`, for
    /// `find_on_path`. None of them ever runs: the scripted spawner answers in their place.
    fn env_with(root: &Path, programs: &[&str]) -> Env {
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        for p in programs {
            let file = bin.join(p);
            fs::write(&file, "#!/bin/sh\nexit 99\n").unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut env = Env::for_test(root);
        env.vars.insert("PATH".into(), bin.into_os_string());
        env
    }

    /// Each call as words: the program's file name, then its arguments.
    fn calls(specs: &[SpawnSpec]) -> Vec<Vec<String>> {
        specs
            .iter()
            .map(|s| {
                let mut words = vec![s.program.file_name().unwrap().to_string_lossy().into_owned()];
                words.extend(s.args.iter().map(|a| a.to_string_lossy().into_owned()));
                words
            })
            .collect()
    }

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|w| (*w).to_owned()).collect()
    }

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn program() -> PathBuf {
        PathBuf::from("/opt/homebrew/bin/tagteam")
    }

    fn recorded() -> Vec<(String, String)> {
        vec![("XDG_DATA_HOME".to_owned(), "/new/data".to_owned())]
    }

    fn unit(kind: ServiceKind) -> String {
        render_unit(kind, &program(), &recorded()).unwrap()
    }

    /// D4.1's record in `daemon.lock`, naming this test process: a daemon that runs.
    fn live_daemon(env: &Env) -> ProcessStamp {
        let me = ProcessStamp::current().unwrap();
        let lock = daemon_lock_path(env);
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        fs::write(
            &lock,
            format!(
                "{{\"pid\":{},\"start\":{},\"version\":\"0.0.1\"}}\n",
                me.pid, me.start
            ),
        )
        .unwrap();
        me
    }

    const LINGERING_OFF: &str = "lingering is off for tester, so systemd stops the daemon when you log out; `loginctl enable-linger tester` keeps it running";

    #[test]
    fn reinstall_replaces_the_unit_and_restarts_a_running_daemon() {
        // Review Focus 4: a unit from another data directory is replaced, and the daemon it
        // runs is stopped and started again, so the new program and environment are the ones
        // that run.
        for kind in KINDS {
            let d = tempfile::tempdir().unwrap();
            let env = env_with(d.path(), &ALL);
            let path = unit_path(&env, kind);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let old = vec![("XDG_DATA_HOME".to_owned(), "/old/data".to_owned())];
            fs::write(
                &path,
                render_unit(kind, Path::new("/usr/local/bin/tagteam"), &old).unwrap(),
            )
            .unwrap();
            let s = ScriptedSpawner::new();
            let expected = match kind {
                ServiceKind::Launchd => {
                    s.push(exited(0, "", "")); // bootout: it was loaded, and is stopped
                    s.push(exited(0, "", "")); // bootstrap: RunAtLoad starts the new one
                    let (target, domain) = (launchd_target(), launchd_domain());
                    vec![
                        words(&["launchctl", "bootout", target.as_str()]),
                        words(&[
                            "launchctl",
                            "bootstrap",
                            domain.as_str(),
                            path.to_str().unwrap(),
                        ]),
                    ]
                }
                ServiceKind::Systemd => {
                    for _ in 0..3 {
                        s.push(exited(0, "", ""));
                    }
                    s.push(exited(0, "Linger=yes\n", ""));
                    vec![
                        words(&["systemctl", "--user", "daemon-reload"]),
                        words(&["systemctl", "--user", "enable", "tagteam.service"]),
                        words(&["systemctl", "--user", "restart", "tagteam.service"]),
                        words(&["loginctl", "show-user", "tester", "-p", "Linger"]),
                    ]
                }
            };
            let report = install(&env, kind, &program(), &recorded(), true, &s).unwrap();
            assert_eq!(
                report,
                InstallReport {
                    kind,
                    path: path.clone(),
                    label: kind.label().to_owned(),
                    started: true,
                    warnings: vec![],
                }
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), unit(kind), "{kind:?}");
            assert_eq!(mode(&path), 0o600, "D9.1: a proxy URL can carry a password");
            assert_eq!(calls(&s.specs()), expected, "{kind:?}");
        }
    }

    #[test]
    fn no_start_writes_the_unit_and_starts_or_stops_nothing() {
        // D9: on macOS the plist is written, not bootstrapped; on Linux the unit is enabled for
        // the next login. A daemon that runs keeps its old unit until it restarts, and says so.
        for kind in KINDS {
            let d = tempfile::tempdir().unwrap();
            let env = env_with(d.path(), &ALL);
            let me = live_daemon(&env);
            let s = ScriptedSpawner::new();
            let mut warnings = vec![format!(
                "a daemon already runs (pid {}) and keeps its old unit until it restarts; `tagteam daemon install` without --no-start restarts it now",
                me.pid
            )];
            let expected = match kind {
                ServiceKind::Launchd => vec![],
                ServiceKind::Systemd => {
                    s.push(exited(0, "", ""));
                    s.push(exited(0, "", ""));
                    s.push(exited(0, "Linger=no\n", ""));
                    warnings.push(LINGERING_OFF.to_owned());
                    vec![
                        words(&["systemctl", "--user", "daemon-reload"]),
                        words(&["systemctl", "--user", "enable", "tagteam.service"]),
                        words(&["loginctl", "show-user", "tester", "-p", "Linger"]),
                    ]
                }
            };
            let report = install(&env, kind, &program(), &recorded(), false, &s).unwrap();
            assert!(!report.started);
            assert_eq!(report.warnings, warnings, "{kind:?}");
            assert_eq!(fs::read_to_string(unit_path(&env, kind)).unwrap(), unit(kind));
            assert_eq!(calls(&s.specs()), expected, "{kind:?}");
        }
    }

    #[test]
    fn without_its_manager_install_writes_nothing_and_names_print() {
        // D9.3: without systemctl, install fails and names --print. launchd alike.
        for (kind, manager) in [
            (ServiceKind::Launchd, "launchctl"),
            (ServiceKind::Systemd, "systemctl"),
        ] {
            let d = tempfile::tempdir().unwrap();
            let env = Env::for_test(d.path());
            let s = ScriptedSpawner::new();
            let e = install(&env, kind, &program(), &recorded(), true, &s).unwrap_err();
            assert!(matches!(&e, InstallError::NoManager(m) if m == manager), "{e:?}");
            assert_eq!(
                e.to_string(),
                format!(
                    "{manager} was not found; `tagteam daemon install --print` prints the unit to install yourself"
                )
            );
            assert!(!unit_path(&env, kind).exists());
            assert!(s.specs().is_empty());
        }
    }

    #[test]
    fn a_bootout_with_nothing_loaded_is_no_error_and_a_failed_bootstrap_is() {
        let d = tempfile::tempdir().unwrap();
        let env = env_with(d.path(), &["launchctl"]);
        let s = ScriptedSpawner::new();
        let not_loaded = || exited(113, "", "Boot-out failed: 113: Could not find specified service\n");
        s.push(not_loaded());
        s.push(exited(0, "", ""));
        assert!(install(&env, ServiceKind::Launchd, &program(), &[], true, &s).unwrap().started);
        s.push(not_loaded());
        s.push(exited(5, "", "Bootstrap failed: 5: Input/output error\n"));
        let e = install(&env, ServiceKind::Launchd, &program(), &[], true, &s).unwrap_err();
        assert_eq!(
            e.to_string(),
            "launchctl bootstrap failed (exit 5): Bootstrap failed: 5: Input/output error"
        );
        assert!(
            unit_path(&env, ServiceKind::Launchd).exists(),
            "the unit stays for the next login"
        );
        s.push(Captured::TimedOut);
        let e = install(&env, ServiceKind::Launchd, &program(), &[], true, &s).unwrap_err();
        assert_eq!(
            e.to_string(),
            "launchctl bootout gave no answer: launchctl did not answer within 10 s"
        );
        assert_eq!(s.specs().len(), 5, "nothing after a bootout that gave no answer");
    }

    #[test]
    fn a_systemctl_call_that_fails_stops_the_install_with_its_stderr() {
        let d = tempfile::tempdir().unwrap();
        let env = env_with(d.path(), &ALL);
        let s = ScriptedSpawner::new();
        s.push(exited(0, "", ""));
        s.push(exited(
            1,
            "",
            "Failed to enable unit: Unit file tagteam.service does not exist.\n",
        ));
        let e = install(&env, ServiceKind::Systemd, &program(), &recorded(), true, &s).unwrap_err();
        assert_eq!(
            e.to_string(),
            "systemctl enable failed (exit 1): Failed to enable unit: Unit file tagteam.service does not exist."
        );
        assert_eq!(s.specs().len(), 2, "nothing after the failure");
        s.push(exited(0, "", ""));
        s.push(exited(0, "", ""));
        s.push(exited(1, "", ""));
        let e = install(&env, ServiceKind::Systemd, &program(), &recorded(), true, &s).unwrap_err();
        assert_eq!(
            e.to_string(),
            "systemctl restart failed (exit 1): it printed nothing"
        );
    }

    #[test]
    fn a_value_no_unit_can_carry_is_refused_before_anything_is_written() {
        let d = tempfile::tempdir().unwrap();
        let env = env_with(d.path(), &ALL);
        let s = ScriptedSpawner::new();
        let bad = vec![("HTTPS_PROXY".to_owned(), "http://a\nb".to_owned())];
        let e = install(&env, ServiceKind::Systemd, &program(), &bad, true, &s).unwrap_err();
        assert!(
            matches!(&e, InstallError::Unit(UnitError::Unrepresentable(n)) if n == "HTTPS_PROXY"),
            "{e:?}"
        );
        assert!(!unit_path(&env, ServiceKind::Systemd).exists());
        assert!(s.specs().is_empty());
    }

    #[test]
    fn uninstall_stops_and_removes_the_unit_and_a_second_run_finds_nothing() {
        // D9.2, D9.3, D9.4: bootout then delete; disable --now, delete, then daemon-reload.
        for kind in KINDS {
            let d = tempfile::tempdir().unwrap();
            let env = env_with(d.path(), &ALL);
            let path = unit_path(&env, kind);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, unit(kind)).unwrap();
            let s = ScriptedSpawner::new();
            let expected = match kind {
                ServiceKind::Launchd => {
                    s.push(exited(0, "", ""));
                    vec![words(&["launchctl", "bootout", launchd_target().as_str()])]
                }
                ServiceKind::Systemd => {
                    s.push(exited(0, "", ""));
                    s.push(exited(1, "", "Failed to reload daemon: Access denied\n"));
                    vec![
                        words(&["systemctl", "--user", "disable", "--now", "tagteam.service"]),
                        words(&["systemctl", "--user", "daemon-reload"]),
                    ]
                }
            };
            assert!(uninstall(&env, kind, &s).unwrap(), "{kind:?}");
            assert!(!path.exists());
            assert!(!uninstall(&env, kind, &s).unwrap(), "nothing installed: removed false");
            assert_eq!(
                calls(&s.specs()),
                expected,
                "{kind:?}: a failed reload after the removal is only logged"
            );
        }
        // A unit whose manager is gone stays where it is.
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let path = unit_path(&env, ServiceKind::Systemd);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, unit(ServiceKind::Systemd)).unwrap();
        let e = uninstall(&env, ServiceKind::Systemd, &ScriptedSpawner::new()).unwrap_err();
        assert!(matches!(&e, InstallError::NoManager(m) if m == "systemctl"), "{e:?}");
        assert!(path.exists());
    }

    #[test]
    fn an_unload_that_fails_stops_the_command_and_keeps_the_unit() {
        // D9.2, D9.3, D9.4: only "not loaded" is no error. Any other failure to unload leaves
        // the daemon's unit loaded and running, so `uninstall` keeps its file and reports the
        // manager's status and words, and `install` never loads over it.
        let failed = || exited(5, "", "Boot-out failed: 5: Input/output error\n");
        let bootout_failed =
            "launchctl bootout failed (exit 5): Boot-out failed: 5: Input/output error";
        let d = tempfile::tempdir().unwrap();
        let env = env_with(d.path(), &ALL);
        let path = unit_path(&env, ServiceKind::Launchd);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, unit(ServiceKind::Launchd)).unwrap();
        let s = ScriptedSpawner::new();
        s.push(failed());
        let e = uninstall(&env, ServiceKind::Launchd, &s).unwrap_err();
        assert!(matches!(&e, InstallError::Manager { .. }), "{e:?}");
        assert_eq!(e.to_string(), bootout_failed);
        assert!(path.exists(), "the agent still runs from its plist, which stays");
        // An older macOS's "No such process" is "not loaded", as 113 is.
        s.push(exited(3, "", "Boot-out failed: 3: No such process\n"));
        assert!(uninstall(&env, ServiceKind::Launchd, &s).unwrap());
        assert!(!path.exists());
        // `install` unloads by the same rule: no bootstrap over a loaded old agent.
        s.push(failed());
        let e = install(&env, ServiceKind::Launchd, &program(), &recorded(), true, &s).unwrap_err();
        assert_eq!(e.to_string(), bootout_failed);
        assert_eq!(
            calls(&s.specs()),
            vec![
                words(&["launchctl", "bootout", launchd_target().as_str()]);
                3
            ],
            "nothing after a failed bootout"
        );

        // systemd: a failed `disable --now` leaves the unit enabled and perhaps running, so its
        // file stays and no reload follows.
        let d = tempfile::tempdir().unwrap();
        let env = env_with(d.path(), &ALL);
        let path = unit_path(&env, ServiceKind::Systemd);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, unit(ServiceKind::Systemd)).unwrap();
        let s = ScriptedSpawner::new();
        s.push(exited(1, "", "Failed to connect to bus: No medium found\n"));
        let e = uninstall(&env, ServiceKind::Systemd, &s).unwrap_err();
        assert_eq!(
            e.to_string(),
            "systemctl disable failed (exit 1): Failed to connect to bus: No medium found"
        );
        assert!(path.exists());
        assert_eq!(s.specs().len(), 1, "no daemon-reload after a failed disable");
    }

    /// The scripted spawner, recording each call's timeout and whether its token was set.
    #[derive(Default)]
    struct Recording {
        inner: ScriptedSpawner,
        seen: Mutex<Vec<(Duration, bool)>>,
    }

    impl ProcessSpawner for Recording {
        fn run_captured(&self, spec: &SpawnSpec, timeout: Duration, cancel: &Cancel) -> Captured {
            self.seen
                .lock()
                .unwrap()
                .push((timeout, cancel.requested().is_some()));
            self.inner.run_captured(spec, timeout, cancel)
        }
    }

    #[test]
    fn every_call_is_bounded_at_ten_seconds_and_a_signal_does_not_cut_an_install_in_half() {
        // D9.4's timeout, and a token of the install's own: a Ctrl-C already recorded on the
        // process's token neither skips the load nor stops the unload's partner.
        let d = tempfile::tempdir().unwrap();
        let env = env_with(d.path(), &ALL);
        env.cancel.request(2);
        let s = Recording::default();
        for _ in 0..3 {
            s.inner.push(exited(0, "", ""));
        }
        s.inner.push(exited(0, "Linger=yes\n", ""));
        s.inner.push(exited(0, "", ""));
        s.inner.push(exited(0, "", ""));
        install(&env, ServiceKind::Systemd, &program(), &recorded(), true, &s).unwrap();
        assert!(uninstall(&env, ServiceKind::Systemd, &s).unwrap());
        let seen = s.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 6);
        assert!(
            seen.iter()
                .all(|&(t, cancelled)| t == Duration::from_secs(10) && !cancelled),
            "{seen:?}"
        );
    }
}
```

In `crates/tagteam-daemon/src/lib.rs`, add `pub mod service;` to the module list, in
alphabetical order (after `pub mod server;`, which Task 2 added). Plan A's manifest already
has `thiserror` and `tracing`, and `tempfile` among its dev-dependencies.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-daemon --lib service::`
Expected: the build fails. The test module is the whole file, so it reports `E0425` / `E0433`
for `install`, `uninstall`, `InstallReport`, `InstallError`, `ServiceKind`, `unit_path`,
`launchd_target`, `launchd_domain`, `daemon_lock_path`, `UnitError`, `Env`, `Path`,
`PathBuf`, `Duration`, `ProcessStamp` and `ProcessSpawner`.

- [ ] **Step 3: Implement**

In `crates/tagteam-daemon/src/service.rs`, insert above `#[cfg(test)]`:

```rust
//! D9: `tagteam daemon install` and `uninstall`. They write or remove the engine's units
//! (`tagteam_engine::service`, plan B Decision 4) and load or unload them with `launchctl` or
//! `systemctl`. Each call goes through the `ProcessSpawner` port with a 10 s timeout (D9.4),
//! under a token of its own (`run_manager`): unloading the old unit and loading the new one are
//! one change, which a Ctrl-C between them would leave half made.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tagteam_engine::daemon::{daemon_lock_path, find_on_path, read_daemon_record};
use tagteam_engine::service::{
    ManagerRun, SYSTEMD_UNIT, ServiceKind, UnitError, launchd_domain, launchd_target, lingering,
    render_unit, run_manager, unit_path,
};
use tagteam_provider::atomic::{ensure_private_dir, remove_target, write_atomic_private};
use tagteam_provider::process::{ProcessSpawner, current_uid};
use tagteam_provider::{Env, ProcessStamp, Read};

/// D9.4: how long each `launchctl`, `systemctl` and `loginctl` call may take.
pub const MANAGER_TIMEOUT: Duration = Duration::from_secs(10);

/// What `install` did (D9.4's JSON).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    pub kind: ServiceKind,
    pub path: PathBuf,
    /// The name the manager knows the unit by: the agent's label, or the unit's file name.
    pub label: String,
    /// The daemon was started, or restarted. False under `--no-start`.
    pub started: bool,
    /// What the person should know, one sentence each.
    pub warnings: Vec<String>,
}

/// Why `install` or `uninstall` failed (D9.4).
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)]
    Unit(#[from] UnitError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The manager refused: its words and its exit status.
    #[error("{program} failed (exit {status}): {stderr}")]
    Manager {
        program: String,
        status: String,
        stderr: String,
    },
    /// The manager timed out, could not be started, or was interrupted.
    #[error("{program} gave no answer: {reason}")]
    NoAnswer { program: String, reason: String },
    #[error(
        "{0} was not found; `tagteam daemon install --print` prints the unit to install yourself"
    )]
    NoManager(String),
}

/// The program that loads `kind`'s units.
fn manager_of(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Launchd => "launchctl",
        ServiceKind::Systemd => "systemctl",
    }
}

/// D9: writes the unit for `program` and `recorded`, 0600 through `O_EXCL`, write and rename
/// (D9.1). With `start`, it loads the unit and starts the daemon, restarting one the unit
/// already runs, so a new program path and environment run at once (D9.2, D9.3). Without
/// `start` (`--no-start`), the unit is only written, and on Linux enabled for the next login.
/// A daemon that runs then keeps its old unit until it restarts, and a warning says so.
/// Nothing is written when the platform's manager is missing, or when a value cannot be
/// carried.
pub fn install(
    env: &Env,
    kind: ServiceKind,
    program: &Path,
    recorded: &[(String, String)],
    start: bool,
    spawner: &dyn ProcessSpawner,
) -> Result<InstallReport, InstallError> {
    let unit = render_unit(kind, program, recorded)?;
    let manager = manager_of(kind);
    if find_on_path(manager, env.var("PATH")).is_none() {
        return Err(InstallError::NoManager(manager.to_owned()));
    }
    let path = unit_path(env, kind);
    if let Some(dir) = path.parent() {
        ensure_private_dir(dir)?;
    }
    write_atomic_private(&path, unit.as_bytes(), 0o600)?;
    match kind {
        ServiceKind::Launchd if start => {
            bootout(env, spawner)?;
            let domain = launchd_domain();
            let args = [OsStr::new("bootstrap"), OsStr::new(&domain), path.as_os_str()];
            call(env, "launchctl", "launchctl bootstrap", &args, spawner)?;
        }
        ServiceKind::Launchd => {}
        ServiceKind::Systemd => {
            let reload = ["--user", "daemon-reload"];
            call(env, "systemctl", "systemctl daemon-reload", &reload, spawner)?;
            let enable = ["--user", "enable", SYSTEMD_UNIT];
            call(env, "systemctl", "systemctl enable", &enable, spawner)?;
            if start {
                let restart = ["--user", "restart", SYSTEMD_UNIT];
                call(env, "systemctl", "systemctl restart", &restart, spawner)?;
            }
        }
    }
    let mut warnings = Vec::new();
    if !start && let Some(pid) = running_daemon(env) {
        warnings.push(format!(
            "a daemon already runs (pid {pid}) and keeps its old unit until it restarts; `tagteam daemon install` without --no-start restarts it now"
        ));
    }
    if kind == ServiceKind::Systemd {
        let user = env.user.clone().unwrap_or_else(|| current_uid().to_string());
        match lingering(env, spawner, &user, MANAGER_TIMEOUT) {
            Ok(false) => warnings.push(format!(
                "lingering is off for {user}, so systemd stops the daemon when you log out; `loginctl enable-linger {user}` keeps it running"
            )),
            Ok(true) => {}
            // D9.3 adds its hint only for a `no`; `doctor` reports an unknown (D10).
            Err(why) => tracing::debug!(why = %why, "whether lingering is on could not be told"),
        }
    }
    tracing::info!(
        kind = kind.as_str(),
        started = start,
        "installed the daemon's service unit"
    );
    Ok(InstallReport {
        kind,
        path,
        label: kind.label().to_owned(),
        started: start,
        warnings,
    })
}

/// D9: stops the daemon the unit runs, and removes the unit. On macOS that is `launchctl
/// bootout`, then the file. On Linux it is `systemctl --user disable --now`, the file, then
/// `daemon-reload`. With no unit installed, it runs nothing and returns false (D9.4).
pub fn uninstall(
    env: &Env,
    kind: ServiceKind,
    spawner: &dyn ProcessSpawner,
) -> Result<bool, InstallError> {
    let path = unit_path(env, kind);
    if !path.exists() {
        return Ok(false);
    }
    match kind {
        ServiceKind::Launchd => {
            bootout(env, spawner)?;
            remove_target(&path)?;
        }
        ServiceKind::Systemd => {
            let disable = ["--user", "disable", "--now", SYSTEMD_UNIT];
            call(env, "systemctl", "systemctl disable", &disable, spawner)?;
            remove_target(&path)?;
            // The unit is now disabled, stopped and gone. A reload that fails leaves only
            // systemd's cached copy, which its next reload drops, so the failure is logged and
            // nothing more.
            let reload = ["--user", "daemon-reload"];
            if let Err(e) = call(env, "systemctl", "systemctl daemon-reload", &reload, spawner) {
                tracing::warn!("systemd was not reloaded after the daemon's unit was removed: {e}");
            }
        }
    }
    tracing::info!(kind = kind.as_str(), "removed the daemon's service unit");
    Ok(true)
}

/// D9.2: stops and unloads the agent when it is loaded. An agent that is not loaded
/// (`not_loaded`) is no error; any other failure is, because the agent then still runs.
/// `install` and `uninstall` both unload through this.
fn bootout(env: &Env, spawner: &dyn ProcessSpawner) -> Result<(), InstallError> {
    const WHAT: &str = "launchctl bootout";
    let target = launchd_target();
    match run_manager(env, "launchctl", &["bootout", target.as_str()], spawner, MANAGER_TIMEOUT) {
        ManagerRun::Exited { code: 0, .. } => Ok(()),
        ManagerRun::Exited { code, stderr, .. } if not_loaded(code, &stderr) => {
            tracing::debug!(code, "launchctl bootout stopped nothing: the agent was not loaded");
            Ok(())
        }
        ManagerRun::Exited { code, stderr, .. } => Err(refused(WHAT, code, stderr)),
        ManagerRun::NoAnswer(reason) => Err(InstallError::NoAnswer {
            program: WHAT.into(),
            reason,
        }),
        ManagerRun::Missing => Err(InstallError::NoManager("launchctl".into())),
    }
}

/// D9.2: how `launchctl bootout` reports an agent that is not loaded: exit 113, "Could not find
/// specified service", on current macOS, and exit 3, "No such process", on older releases.
/// Either the code or the words is enough, should one of them differ on a later release.
fn not_loaded(code: i32, stderr: &str) -> bool {
    matches!(code, 3 | 113)
        || stderr.contains("Could not find specified service")
        || stderr.contains("No such process")
}

/// D9.4: a manager call that exited non-zero, with its status and its own words.
fn refused(what: &str, code: i32, stderr: String) -> InstallError {
    InstallError::Manager {
        program: what.to_owned(),
        status: code.to_string(),
        stderr: if stderr.is_empty() {
            "it printed nothing".to_owned()
        } else {
            stderr
        },
    }
}

/// One manager call that must succeed. `what` names it in an error, such as `systemctl
/// enable`.
fn call<A: AsRef<OsStr>>(
    env: &Env,
    program: &str,
    what: &str,
    args: &[A],
    spawner: &dyn ProcessSpawner,
) -> Result<(), InstallError> {
    match run_manager(env, program, args, spawner, MANAGER_TIMEOUT) {
        ManagerRun::Exited { code: 0, .. } => Ok(()),
        ManagerRun::Exited { code, stderr, .. } => Err(refused(what, code, stderr)),
        ManagerRun::NoAnswer(reason) => Err(InstallError::NoAnswer {
            program: what.to_owned(),
            reason,
        }),
        ManagerRun::Missing => Err(InstallError::NoManager(program.to_owned())),
    }
}

/// The pid of a daemon that runs now: `daemon.lock`'s record, judged by an exact pid and
/// start-time match (D4.1, §12.6).
fn running_daemon(env: &Env) -> Option<u32> {
    match read_daemon_record(&daemon_lock_path(env)) {
        Read::Present(r) if (ProcessStamp { pid: r.pid, start: r.start }).is_live() => Some(r.pid),
        _ => None,
    }
}

```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-daemon --lib service::`. Expected: PASS, 9 tests.

#### Part B: the commands

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam/src/cli.rs`, add to `mod tests`:

```rust
    #[test]
    fn daemon_alone_is_the_foreground_run_and_its_service_commands_parse() {
        use clap::Parser;

        use super::{Cli, Command, DaemonAction};
        let parse = |args: &[&str]| Cli::try_parse_from(args).map(|c| c.command);
        assert!(matches!(
            parse(&["tagteam", "daemon"]),
            Ok(Some(Command::Daemon { action: None }))
        ));
        assert!(matches!(
            parse(&["tagteam", "daemon", "install"]),
            Ok(Some(Command::Daemon {
                action: Some(DaemonAction::Install {
                    print: false,
                    no_start: false
                })
            }))
        ));
        assert!(matches!(
            parse(&["tagteam", "daemon", "install", "--no-start"]),
            Ok(Some(Command::Daemon {
                action: Some(DaemonAction::Install {
                    print: false,
                    no_start: true
                })
            }))
        ));
        assert!(matches!(
            parse(&["tagteam", "daemon", "install", "--print"]),
            Ok(Some(Command::Daemon {
                action: Some(DaemonAction::Install {
                    print: true,
                    no_start: false
                })
            }))
        ));
        assert!(
            parse(&["tagteam", "daemon", "install", "--print", "--no-start"]).is_err(),
            "--print changes nothing, so --no-start means nothing beside it"
        );
        assert!(matches!(
            parse(&["tagteam", "daemon", "uninstall"]),
            Ok(Some(Command::Daemon {
                action: Some(DaemonAction::Uninstall)
            }))
        ));
        let cli = Cli::try_parse_from(["tagteam", "daemon", "status", "--json"]).unwrap();
        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Some(Command::Daemon {
                action: Some(DaemonAction::Status)
            })
        ));
    }
```

Create `crates/tagteam/src/service_cmd.rs` containing only this test module. Step 3 puts the
implementation above it:

```rust
#[cfg(test)]
mod tests {
    use tagteam_daemon::protocol::{StatusBackoff, StatusDaemon, StatusOutcome, StatusSwitch};

    use super::*;

    const UNATTENDED_LINE: &str = "The daemon will switch accounts unattended; pause it with `tagteam config set daemon.autoswitch false`.\n";

    fn agent() -> PathBuf {
        PathBuf::from("/Users/me/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist")
    }

    fn status() -> Status {
        Status {
            daemon: StatusDaemon {
                pid: 4242,
                version: "20261014.2045.0".into(),
                protocol: 1,
                started_at: "2026-10-14T20:45:00Z".into(),
                socket: Some("~/.local/share/tagteam/daemon.sock".into()),
                log: "~/.local/state/tagteam/tagteam.log".into(),
            },
            notifications: StatusNotifications {
                enabled: true,
                backend: Some("osascript".into()),
                events: vec!["switch".into(), "quarantine".into(), "limit".into()],
            },
            providers: vec![
                StatusProvider {
                    provider: "claude-code".into(),
                    mode: "auto".into(),
                    autoswitch: true,
                    waiting_for: None,
                    holder_pid: None,
                    backoff: None,
                    last_tick_at: Some("2026-10-14T21:00:00Z".into()),
                    next_tick_at: Some("2026-10-14T21:05:00Z".into()),
                    last_outcome: Some(StatusOutcome {
                        kind: "no-switch".into(),
                        reason: Some("below-threshold".into()),
                        at: "2026-10-14T21:00:00Z".into(),
                    }),
                    last_switch: Some(StatusSwitch {
                        at: "2026-10-14T20:50:00Z".into(),
                        from: 1,
                        to: 2,
                        trigger: "proactive".into(),
                    }),
                },
                StatusProvider {
                    provider: "fake-agent".into(),
                    mode: "waiting".into(),
                    autoswitch: false,
                    waiting_for: Some("engine-lock".into()),
                    holder_pid: Some(777),
                    backoff: Some(StatusBackoff {
                        reason: "keychain".into(),
                        until: "2026-10-14T21:15:00Z".into(),
                    }),
                    last_tick_at: None,
                    next_tick_at: Some("2026-10-14T21:01:00Z".into()),
                    last_outcome: None,
                    last_switch: None,
                },
            ],
            jobs: vec![
                StatusJob {
                    name: "store-watch".into(),
                    last_run_at: Some("2026-10-14T21:00:58Z".into()),
                    next_run_at: Some("2026-10-14T21:01:00Z".into()),
                    last_error: None,
                },
                StatusJob {
                    name: "upgrade-watch".into(),
                    last_run_at: None,
                    next_run_at: Some("2026-10-14T21:01:00Z".into()),
                    last_error: Some("the program path cannot be read".into()),
                },
            ],
            subscribers: 1,
        }
    }

    fn running() -> ServiceState {
        ServiceState {
            installed: true,
            loaded: Some(true),
            running: Some(true),
            last_exit_status: None,
            unknown: None,
        }
    }

    #[test]
    fn status_with_a_daemon_shows_its_own_status_and_its_service() {
        let found = Found::Answered(Box::new(status()));
        assert!(found.running());
        assert_eq!(
            status_human(&found, ServiceKind::Launchd, &agent(), &running()),
            "The daemon runs as pid 4242: tagteam 20261014.2045.0, protocol 1, since 2026-10-14T20:45:00Z.\n\
             \x20 socket: ~/.local/share/tagteam/daemon.sock\n\
             \x20 log: ~/.local/state/tagteam/tagteam.log\n\
             \x20 notifications: switch, quarantine, limit, through osascript\n\
             \x20 claude-code: auto; last no-switch (below-threshold) at 2026-10-14T21:00:00Z; next at 2026-10-14T21:05:00Z; last switch 1 → 2 (proactive) at 2026-10-14T20:50:00Z\n\
             \x20 fake-agent: waiting for engine-lock (pid 777), auto-switching paused, backing off (keychain) until 2026-10-14T21:15:00Z; next at 2026-10-14T21:01:00Z\n\
             \x20 store-watch: last run 2026-10-14T21:00:58Z, next 2026-10-14T21:01:00Z\n\
             \x20 upgrade-watch: not run yet, next 2026-10-14T21:01:00Z, last error: the program path cannot be read\n\
             \x20 subscribers: 1\n\
             Service: the launchd agent at /Users/me/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist is installed, loaded and running.\n"
        );
        assert_eq!(
            status_json(&found, ServiceKind::Launchd, &agent(), &running()),
            json!({
                "schemaVersion": 1,
                "running": true,
                "pid": 4242,
                "daemon": {
                    "daemon": {"pid": 4242, "version": "20261014.2045.0", "protocol": 1,
                               "startedAt": "2026-10-14T20:45:00Z",
                               "socket": "~/.local/share/tagteam/daemon.sock",
                               "log": "~/.local/state/tagteam/tagteam.log"},
                    "notifications": {"enabled": true, "backend": "osascript",
                                      "events": ["switch", "quarantine", "limit"]},
                    "providers": [
                        {"provider": "claude-code", "mode": "auto", "autoswitch": true,
                         "waitingFor": null, "holderPid": null, "backoff": null,
                         "lastTickAt": "2026-10-14T21:00:00Z", "nextTickAt": "2026-10-14T21:05:00Z",
                         "lastOutcome": {"kind": "no-switch", "reason": "below-threshold",
                                         "at": "2026-10-14T21:00:00Z"},
                         "lastSwitch": {"at": "2026-10-14T20:50:00Z", "from": 1, "to": 2,
                                        "trigger": "proactive"}},
                        {"provider": "fake-agent", "mode": "waiting", "autoswitch": false,
                         "waitingFor": "engine-lock", "holderPid": 777,
                         "backoff": {"reason": "keychain", "until": "2026-10-14T21:15:00Z"},
                         "lastTickAt": null, "nextTickAt": "2026-10-14T21:01:00Z",
                         "lastOutcome": null, "lastSwitch": null}
                    ],
                    "jobs": [
                        {"name": "store-watch", "lastRunAt": "2026-10-14T21:00:58Z",
                         "nextRunAt": "2026-10-14T21:01:00Z", "lastError": null},
                        {"name": "upgrade-watch", "lastRunAt": null,
                         "nextRunAt": "2026-10-14T21:01:00Z",
                         "lastError": "the program path cannot be read"}
                    ],
                    "subscribers": 1
                },
                "service": {"kind": "launchd",
                            "path": "/Users/me/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist",
                            "installed": true, "loaded": true, "lastExitStatus": null}
            })
        );
    }

    #[test]
    fn status_without_an_answer_says_what_the_lock_and_the_service_say() {
        let unit = PathBuf::from("/home/me/.config/systemd/user/tagteam.service");
        let unknown = ServiceState {
            installed: false,
            unknown: Some("systemctl is not on PATH".into()),
            ..ServiceState::default()
        };
        let stopped = ServiceState {
            installed: true,
            loaded: Some(true),
            running: Some(false),
            last_exit_status: Some(1),
            unknown: None,
        };
        let not_loaded = ServiceState {
            installed: true,
            loaded: Some(false),
            running: Some(false),
            ..ServiceState::default()
        };
        let cases = [
            (
                Found::NotRunning,
                &unknown,
                "The daemon is not running.\nService: the systemd user unit at /home/me/.config/systemd/user/tagteam.service is not installed; its state cannot be read (systemctl is not on PATH).\n",
                json!(null),
            ),
            (
                Found::Silent { pid: 4242 },
                &stopped,
                "A daemon runs as pid 4242, but its socket does not answer; the log says why.\nService: the systemd user unit at /home/me/.config/systemd/user/tagteam.service is installed, loaded, not running (last exit status 1).\n",
                json!(4242),
            ),
            (
                Found::Mismatch {
                    server: 2,
                    client: 1,
                    pid: Some(4242),
                },
                &not_loaded,
                "A daemon runs as pid 4242, but it speaks protocol 2 and this tagteam 1; it restarts on the new tagteam within a minute.\nService: the systemd user unit at /home/me/.config/systemd/user/tagteam.service is installed, not loaded.\n",
                json!(4242),
            ),
        ];
        for (found, state, human, pid) in cases {
            assert_eq!(status_human(&found, ServiceKind::Systemd, &unit, state), human);
            let v = status_json(&found, ServiceKind::Systemd, &unit, state);
            assert_eq!(v["running"], json!(found.running()));
            assert_eq!(v["pid"], pid);
            assert_eq!(v["daemon"], Value::Null);
            assert_eq!(
                v["service"],
                json!({"kind": "systemd", "path": unit.display().to_string(),
                       "installed": state.installed, "loaded": state.loaded,
                       "lastExitStatus": state.last_exit_status})
            );
        }
        assert!(!Found::NotRunning.running());
        assert!(
            Found::Mismatch {
                server: 2,
                client: 1,
                pid: None
            }
            .running()
        );
    }

    #[test]
    fn install_and_uninstall_say_what_they_did() {
        let path = PathBuf::from("/home/me/.config/systemd/user/tagteam.service");
        let started = InstallReport {
            kind: ServiceKind::Systemd,
            path: path.clone(),
            label: "tagteam.service".into(),
            started: true,
            warnings: vec!["lingering is off".into()],
        };
        assert_eq!(
            install_human(&started),
            format!(
                "Installed the systemd user unit at {}, and started the daemon.\n{UNATTENDED_LINE}",
                path.display()
            )
        );
        assert_eq!(
            install_json(&started),
            json!({"schemaVersion": 1, "ok": true,
                   "service": {"kind": "systemd", "path": path.display().to_string(),
                               "label": "tagteam.service"},
                   "started": true, "warnings": ["lingering is off"]})
        );
        let waiting = InstallReport {
            started: false,
            ..started
        };
        assert_eq!(
            install_human(&waiting),
            format!(
                "Installed the systemd user unit at {}; it starts at the next login.\n{UNATTENDED_LINE}",
                path.display()
            )
        );
        assert_eq!(
            uninstall_human(ServiceKind::Launchd, &agent(), true),
            "Stopped the daemon and removed the launchd agent at /Users/me/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist.\n"
        );
        assert_eq!(
            uninstall_human(ServiceKind::Launchd, &agent(), false),
            "No launchd agent is installed at /Users/me/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist.\n"
        );
        assert_eq!(
            uninstall_json(true),
            json!({"schemaVersion": 1, "ok": true, "removed": true})
        );
        assert_eq!(
            print_json(ServiceKind::Launchd, &agent(), "<plist/>"),
            json!({"schemaVersion": 1,
                   "service": {"kind": "launchd", "path": agent().display().to_string(),
                               "label": "io.github.michaelrigart.tagteam"},
                   "unit": "<plist/>"})
        );
    }

    #[test]
    fn the_program_installed_is_this_binary_when_no_tagteam_on_path_is_it() {
        // D9.1: no PATH, so no stable link to prefer.
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        assert_eq!(
            install_program(&env).unwrap(),
            std::env::current_exe().unwrap()
        );
    }

    #[test]
    fn a_daemon_that_holds_its_lock_but_does_not_greet_is_running() {
        // D8.2: a listener that never greets is a daemon there but hung (Task 3's Timeout).
        // With a live holder of daemon.lock it runs, with the lock's pid. Without one, whatever
        // listens is no daemon tagteam knows, and that is an error.
        let short = tempfile::Builder::new()
            .prefix("tt")
            .tempdir_in("/tmp")
            .or_else(|_| tempfile::tempdir())
            .unwrap();
        let mut env = Env::for_test(short.path());
        env.xdg_data_home = Some(short.path().join("d"));
        let sock = daemon_socket_path(&env);
        fs::create_dir_all(sock.parent().unwrap()).unwrap();
        let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let me = ProcessStamp::current().unwrap();
        let lock = daemon_lock_path(&env);
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        fs::write(
            &lock,
            format!(
                "{{\"pid\":{},\"start\":{},\"version\":\"0\"}}\n",
                me.pid, me.start
            ),
        )
        .unwrap();
        assert!(matches!(find_daemon(&env), Ok(Found::Silent { pid }) if pid == me.pid));
        fs::remove_file(&lock).unwrap();
        assert!(matches!(find_daemon(&env), Err(ClientError::Timeout)));
    }
}
```

In `crates/tagteam/src/lib.rs`, replace:

```rust
mod run;
mod shell_init;
```

with:

```rust
mod run;
mod service_cmd;
mod shell_init;
```

Create `crates/tagteam/tests/daemon_service_cli.rs`:

```rust
//! `tagteam daemon install`, `uninstall` and `status` through the binary (D8, D9). The service
//! manager is a stand-in script on `PATH` (`install_script`) that records its arguments, so no
//! test touches this machine's launchd or systemd (§15.1). The platform is the test-support
//! override, so both units run on either OS. The one exception is D14's real round trip,
//! `#[ignore]`d and run by hand (plan B Task 10). Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::{install_script, short_tempdir, std_cmd, two_fresh_accounts};
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::service::{ServiceKind, parse_unit, unit_path};
use tagteam_provider::process::current_uid;
use tagteam_provider::{Env, ProcessStamp, ProfileMarker, profile_path};

const UNATTENDED: &str = "The daemon will switch accounts unattended; pause it with `tagteam config set daemon.autoswitch false`.\n";
const LINGERING_OFF: &str = "lingering is off for tester, so systemd stops the daemon when you log out; `loginctl enable-linger tester` keeps it running";

/// `tagteam <args>` as `platform` (`macos` or `linux`), with `<root>/bin` alone on `PATH`.
fn service_cmd(root: &Path, platform: &str, args: &[&str]) -> Output {
    std_cmd(root)
        .env("TAGTEAM_TEST_PLATFORM", platform)
        .env("PATH", root.join("bin"))
        .args(args)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn json_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", text(&out.stdout)))
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// `<root>/bin`, created.
fn bin(root: &Path) -> std::path::PathBuf {
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    bin
}

/// A stand-in `launchctl`, logging each call's arguments to `bin/launchctl.log`. `bootout` and
/// `print` find nothing loaded, with exit 113 as macOS reports it; anything else succeeds.
fn launchctl(bin: &Path) {
    let log = bin.join("launchctl.log");
    install_script(
        &bin.join("launchctl"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\ncase \"$1\" in\n  bootout) echo 'Boot-out failed: 113: Could not find specified service' >&2; exit 113 ;;\n  print) echo 'Could not find service \"io.github.michaelrigart.tagteam\" in domain for user gui: 501' >&2; exit 113 ;;\nesac\nexit 0\n",
            log = log.display()
        ),
    );
}

/// A stand-in `systemctl` that succeeds at everything, and a `loginctl` that reports lingering
/// off. Each logs its arguments beside itself.
fn systemd(bin: &Path) {
    let log = bin.join("systemctl.log");
    install_script(
        &bin.join("systemctl"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\nexit 0\n",
            log = log.display()
        ),
    );
    let log = bin.join("loginctl.log");
    install_script(
        &bin.join("loginctl"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\necho Linger=no\n",
            log = log.display()
        ),
    );
}

/// The arguments of each call of `program`, one line per call.
fn calls(bin: &Path, program: &str) -> Vec<String> {
    fs::read_to_string(bin.join(format!("{program}.log")))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn install_print_shows_this_platform_s_unit_and_writes_nothing() {
    // D9: --print changes nothing. D9.1: the program is this binary (no tagteam on PATH), and
    // only the recorded variables that are set are recorded, quoting and all.
    let proxy = r#"http://us$er:p%40ss"w\rd@proxy.example:8080"#;
    for (platform, kind) in [("macos", ServiceKind::Launchd), ("linux", ServiceKind::Systemd)] {
        let d = tempfile::tempdir().unwrap();
        let bin = bin(d.path());
        let data = d.path().join("data dir ü");
        let out = std_cmd(d.path())
            .env("TAGTEAM_TEST_PLATFORM", platform)
            .env("PATH", &bin)
            .env("XDG_DATA_HOME", &data)
            .env("HTTPS_PROXY", proxy)
            .env("NOT_RECORDED", "x")
            .args(["daemon", "install", "--print"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        let unit = parse_unit(kind, &text(&out.stdout)).expect("the unit tagteam renders");
        assert_eq!(
            fs::canonicalize(&unit.program).unwrap(),
            fs::canonicalize(assert_cmd::cargo::cargo_bin("tagteam")).unwrap()
        );
        assert_eq!(
            unit.env,
            vec![
                ("XDG_DATA_HOME".to_owned(), data.display().to_string()),
                ("HTTPS_PROXY".to_owned(), proxy.to_owned()),
                ("PATH".to_owned(), bin.display().to_string()),
            ]
        );
        let path = unit_path(&Env::for_test(d.path()), kind);
        assert!(!path.exists(), "{platform}: nothing written");
        assert!(!data.exists(), "{platform}: no data directory made");

        let out = service_cmd(d.path(), platform, &["--json", "daemon", "install", "--print"]);
        assert_eq!(out.status.code(), Some(0));
        let v = json_of(&out);
        assert_eq!(
            v["service"],
            json!({"kind": kind.as_str(), "path": path.display().to_string(), "label": kind.label()})
        );
        assert!(parse_unit(kind, v["unit"].as_str().unwrap()).is_some());
    }
}

#[test]
fn install_and_uninstall_drive_systemd_through_the_systemctl_on_path() {
    let d = tempfile::tempdir().unwrap();
    let bin = bin(d.path());
    systemd(&bin);
    let path = d.path().join("home/.config/systemd/user/tagteam.service");

    let out = service_cmd(d.path(), "linux", &["--json", "daemon", "install"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        json_of(&out),
        json!({"schemaVersion": 1, "ok": true,
               "service": {"kind": "systemd", "path": path.display().to_string(),
                           "label": "tagteam.service"},
               "started": true, "warnings": [LINGERING_OFF]})
    );
    assert_eq!(mode(&path), 0o600);
    assert_eq!(
        parse_unit(ServiceKind::Systemd, &fs::read_to_string(&path).unwrap())
            .unwrap()
            .env,
        vec![("PATH".to_owned(), bin.display().to_string())]
    );
    assert_eq!(
        calls(&bin, "systemctl"),
        [
            "--user daemon-reload",
            "--user enable tagteam.service",
            "--user restart tagteam.service"
        ]
    );
    assert_eq!(calls(&bin, "loginctl"), ["show-user tester -p Linger"]);

    let out = service_cmd(d.path(), "linux", &["daemon", "install", "--no-start"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        format!(
            "Installed the systemd user unit at {}; it starts at the next login.\n{UNATTENDED}",
            path.display()
        )
    );
    assert_eq!(text(&out.stderr), format!("warning: {LINGERING_OFF}\n"));
    assert_eq!(
        calls(&bin, "systemctl")[3..],
        ["--user daemon-reload", "--user enable tagteam.service"]
    );

    let out = service_cmd(d.path(), "linux", &["--json", "daemon", "uninstall"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        json_of(&out),
        json!({"schemaVersion": 1, "ok": true, "removed": true})
    );
    assert!(!path.exists());
    assert_eq!(
        calls(&bin, "systemctl")[5..],
        ["--user disable --now tagteam.service", "--user daemon-reload"]
    );
    let out = service_cmd(d.path(), "linux", &["daemon", "uninstall"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        text(&out.stdout),
        format!("No systemd user unit is installed at {}.\n", path.display())
    );
    assert_eq!(calls(&bin, "systemctl").len(), 7, "nothing to stop the second time");
}

#[test]
fn install_on_macos_loads_the_agent_after_an_unload_that_finds_nothing() {
    let d = tempfile::tempdir().unwrap();
    let bin = bin(d.path());
    launchctl(&bin);
    let path = d
        .path()
        .join("home/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist");
    let out = service_cmd(d.path(), "macos", &["daemon", "install"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        format!(
            "Installed the launchd agent at {}, and started the daemon.\n{UNATTENDED}",
            path.display()
        )
    );
    assert_eq!(text(&out.stderr), "");
    assert_eq!(mode(&path), 0o600);
    let uid = current_uid();
    assert_eq!(
        calls(&bin, "launchctl"),
        [
            format!("bootout gui/{uid}/io.github.michaelrigart.tagteam"),
            format!("bootstrap gui/{uid} {}", path.display()),
        ]
    );
}

#[test]
fn without_a_service_manager_install_fails_naming_print() {
    let d = tempfile::tempdir().unwrap();
    bin(d.path());
    let out = service_cmd(d.path(), "linux", &["--json", "daemon", "install"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        json_of(&out),
        json!({"schemaVersion": 1, "error": {"type": "no-service-manager",
               "message": "systemctl was not found; `tagteam daemon install --print` prints the unit to install yourself"}})
    );
    assert!(!d.path().join("home/.config/systemd").exists());
}

#[test]
fn status_without_a_daemon_exits_3_with_the_service_s_state() {
    // D8.2, Decision 6.
    let d = tempfile::tempdir().unwrap();
    let bin = bin(d.path());
    let unit = d.path().join("home/.config/systemd/user/tagteam.service");
    let out = service_cmd(d.path(), "linux", &["--json", "daemon", "status"]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out.stderr));
    assert_eq!(
        json_of(&out),
        json!({"schemaVersion": 1, "running": false, "pid": null, "daemon": null,
               "service": {"kind": "systemd", "path": unit.display().to_string(),
                           "installed": false, "loaded": null, "lastExitStatus": null}})
    );
    let out = service_cmd(d.path(), "linux", &["daemon", "status"]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(
        text(&out.stdout),
        format!(
            "The daemon is not running.\nService: the systemd user unit at {} is not installed; its state cannot be read (systemctl is not on PATH).\n",
            unit.display()
        )
    );
    launchctl(&bin);
    let agent = d
        .path()
        .join("home/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist");
    let out = service_cmd(d.path(), "macos", &["--json", "daemon", "status"]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(
        json_of(&out)["service"],
        json!({"kind": "launchd", "path": agent.display().to_string(),
               "installed": false, "loaded": false, "lastExitStatus": null})
    );
    assert_eq!(
        calls(&bin, "launchctl"),
        [format!("print gui/{}/io.github.michaelrigart.tagteam", current_uid())]
    );
}

#[test]
fn a_live_process_holding_the_daemon_lock_without_a_socket_is_running() {
    // D8.2: running, with the pid from the lock's record, and no status to show.
    let d = tempfile::tempdir().unwrap();
    bin(d.path());
    let me = ProcessStamp::current().unwrap();
    let lock = Env::for_test(d.path()).data_dir().join("locks/daemon.lock");
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let record = |start: u64| {
        format!(
            "{{\"pid\":{},\"start\":{start},\"version\":\"{}\"}}\n",
            me.pid,
            env!("CARGO_PKG_VERSION")
        )
    };
    fs::write(&lock, record(me.start)).unwrap();
    let out = service_cmd(d.path(), "linux", &["--json", "daemon", "status"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let v = json_of(&out);
    assert_eq!(
        (&v["running"], &v["pid"], &v["daemon"]),
        (&json!(true), &json!(me.pid), &Value::Null)
    );
    let out = service_cmd(d.path(), "linux", &["daemon", "status"]);
    assert!(
        text(&out.stdout).starts_with(&format!(
            "A daemon runs as pid {}, but its socket does not answer; the log says why.\n",
            me.pid
        )),
        "{}",
        text(&out.stdout)
    );
    // The same record naming a process that has exited: not running.
    fs::write(&lock, record(me.start + 1)).unwrap();
    assert_eq!(
        service_cmd(d.path(), "linux", &["daemon", "status"]).status.code(),
        Some(3)
    );
}

#[test]
fn install_and_uninstall_refuse_inside_a_run_shell_and_status_answers_there() {
    // D9: a run shell's environment is a session's, not the one the daemon should keep.
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_fresh_accounts(d.path());
    let env = Env::for_test(d.path());
    let profile = profile_path(&env, &AccountId::from_string(&a));
    ProfileMarker {
        provider: tagteam_core::ProviderId::new("claude-code"),
        account_id: AccountId::from_string(&a),
        config_dir: profile.display().to_string(),
        outer: json!({"CLAUDE_CONFIG_DIR": null, "CLAUDE_SECURESTORAGE_CONFIG_DIR": null}),
    }
    .write(&profile)
    .unwrap();
    let bin = bin(d.path());
    launchctl(&bin);
    let inside = |args: &[&str]| {
        std_cmd(d.path())
            .env("PATH", &bin)
            .env("CLAUDE_CONFIG_DIR", &profile)
            .args(args)
            .output()
            .unwrap()
    };
    for args in [
        &["--json", "daemon", "install"][..],
        &["--json", "daemon", "install", "--print"],
        &["--json", "daemon", "uninstall"],
    ] {
        let out = inside(args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(json_of(&out)["error"]["type"], json!("inside-run-shell"), "{args:?}");
    }
    assert_eq!(
        calls(&bin, "launchctl"),
        Vec::<String>::new(),
        "a refusal runs nothing"
    );
    let out = inside(&["--json", "daemon", "status"]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out.stderr));
    assert_eq!(json_of(&out)["running"], json!(false));
}

/// `tagteam <args>` in this user's own environment, except the variables that place tagteam's
/// store, its state and Claude Code's home, which point into `root` (on macOS its config too,
/// which no unit path depends on there). A unit installed from here records them (D9.1), so
/// the daemon it starts serves an empty store under `root`, never this user's accounts.
fn real_env_cmd(root: &Path, args: &[&str]) -> Output {
    let mut c = std::process::Command::new(assert_cmd::cargo::cargo_bin("tagteam"));
    c.env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("CLAUDE_CONFIG_DIR", root.join("claude"))
        .env_remove("CLAUDE_SECURESTORAGE_CONFIG_DIR")
        .args(args)
        .stdin(Stdio::null());
    if cfg!(target_os = "macos") {
        c.env("XDG_CONFIG_HOME", root.join("config"));
    }
    c.output().unwrap()
}

#[test]
#[ignore = "loads a real launchd agent or systemd user unit under tagteam's own label: run by hand, with Michael's go-ahead (plan B Task 10)"]
fn a_real_service_manager_starts_and_stops_the_daemon() {
    // D14: the real launchd or systemd round trip, on the platform this runs on. The unit goes
    // where the manager reads it, under the real label, so the test stops at once when a
    // tagteam unit is installed already. The unit records a fixture's store, state and Claude
    // Code home, so the daemon it starts has no account to touch.
    let d = short_tempdir();
    let root = d.path();
    let status = || {
        let out = real_env_cmd(root, &["--json", "daemon", "status"]);
        (out.status.code(), json_of(&out))
    };
    let (code, before) = status();
    assert_eq!(code, Some(3), "{before}");
    assert_eq!(
        before["service"]["installed"],
        json!(false),
        "a tagteam unit is installed already; `tagteam daemon uninstall` it first: {before}"
    );
    let unit = PathBuf::from(before["service"]["path"].as_str().unwrap());

    let out = real_env_cmd(root, &["--json", "daemon", "install"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(json_of(&out)["started"], json!(true));
    assert_eq!(mode(&unit), 0o600);
    let deadline = Instant::now() + Duration::from_secs(30);
    let running = loop {
        let (code, v) = status();
        if code == Some(0) && v["daemon"].is_object() {
            break v;
        }
        assert!(
            Instant::now() < deadline,
            "the service never started the daemon: {v}"
        );
        thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(running["service"]["loaded"], json!(true), "{running}");
    assert_eq!(
        running["daemon"]["daemon"]["socket"],
        json!(root.join("data/tagteam/daemon.sock").display().to_string()),
        "the daemon serves the fixture's store"
    );

    let out = real_env_cmd(root, &["--json", "daemon", "uninstall"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(json_of(&out)["removed"], json!(true));
    assert!(!unit.exists());
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (code, v) = status();
        if code == Some(3) {
            assert_eq!(v["service"]["installed"], json!(false), "{v}");
            break;
        }
        assert!(Instant::now() < deadline, "the daemon outlived its unit: {v}");
        thread::sleep(Duration::from_millis(200));
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --lib`
Expected: the build fails. `cli.rs` reports `error[E0432]: unresolved import
\`super::DaemonAction\`` and `error[E0026]: variant \`Command::Daemon\` does not have a field
named \`action\``. `service_cmd.rs`, whose test module is the whole file, reports `E0425` /
`E0433` / `E0412` for `Found`, `status_human`, `status_json`, `install_human`, `install_json`,
`uninstall_human`, `uninstall_json`, `print_json`, `install_program`, `Status`, `StatusJob`,
`StatusNotifications`, `StatusProvider`, `ServiceState`, `ServiceKind`, `InstallReport`,
`PathBuf`, `Value`, `json!` and `Env`. The binary tests cannot build until the library does.

- [ ] **Step 3: Implement**

**`crates/tagteam/src/cli.rs`.** Plan A's variant (Task 12) is:

```rust
    Daemon {},
```

Keep the doc comment plan A wrote above it (`--provider` and `--json` are global, so the
variant has no arguments of its own). Replace the variant with:

```rust
    Daemon {
        #[command(subcommand)]
        action: Option<DaemonAction>,
    },
```

and add, after `enum ConfigAction`:

```rust
/// `tagteam daemon`'s service commands (D8, D9). Without one, `tagteam daemon` runs the daemon
/// in the foreground.
#[derive(Subcommand)]
pub enum DaemonAction {
    /// Install the service that runs the daemon at login, and start it
    ///
    /// macOS: a launchd agent in ~/Library/LaunchAgents. Linux: a systemd user unit. The unit
    /// records this tagteam's path, and the variables that place its store, Claude Code's
    /// home, a proxy and the log. Running it again replaces the unit and restarts the daemon.
    /// --json prints {schemaVersion, ok, service: {kind, path, label}, started, warnings}.
    Install {
        /// Print the unit for this platform and change nothing
        #[arg(long, conflicts_with = "no_start")]
        print: bool,
        /// Write the unit and enable it for the next login, starting and stopping nothing
        #[arg(long = "no-start")]
        no_start: bool,
    },
    /// Stop the daemon and remove its service
    ///
    /// --json prints {schemaVersion, ok, removed}; removed is false when none was installed.
    Uninstall,
    /// Whether the daemon runs: its own status from its socket, else its lock and its service
    ///
    /// Exits 0 when it runs, 3 when it does not, and 1 on an error. --json prints
    /// {schemaVersion, running, pid, daemon, service: {kind, path, installed, loaded,
    /// lastExitStatus}}, daemon being the daemon's own status, or null.
    Status,
}
```

**`crates/tagteam/src/app.rs`.**
1. **Patterns.** Plan A's Task 12 matches the variant as `Command::Daemon { .. }`, which would
   now match the service commands too. `rg -n 'Command::Daemon' crates/tagteam/src` finds
   three places:
   - `run`'s `stops_on_a_signal`:
     `Some(Command::Auto { once: false, .. } | Command::Daemon { .. })` becomes
     `Some(Command::Auto { once: false, .. } | Command::Daemon { action: None })`. Only the
     foreground run stops on a signal.
   - `command_name`: `Command::Daemon { .. } => "daemon",` stays as it is.
   - `App::dispatch`: the arm in step 2.

   Plan A keeps `Command::Daemon` out of `touches_keychain` and
   `touches_keychain_only_with_a_store`, so there is no fourth place. The service commands
   touch no Keychain item and do not stop on a signal.
2. **Dispatch.** In `App::dispatch`, replace plan A's arm:

```rust
            Command::Daemon { .. } => return self.daemon(),
```

with:

```rust
            Command::Daemon { action: None } => return self.daemon(),
            // D8, D9: the service commands. Root is refused for every command already.
            Command::Daemon {
                action: Some(action),
            } => return self.daemon_action(action),
```

3. **Imports.** Change `use std::ffi::OsString;` to `use std::ffi::{OsStr, OsString};`. Add:

```rust
use tagteam_daemon::service::{self as daemon_service, InstallError, MANAGER_TIMEOUT};
```

and replace Task 6's `use tagteam_engine::service::{RECORDED_VARS, ServiceKind};` with:

```rust
use tagteam_engine::service::{
    RECORDED_VARS, ServiceKind, recorded_env, render_unit, service_state, unit_path,
};
```

Add `DaemonAction` to the `use crate::cli::{…}` list, and `service_cmd` to the `use crate::{…}`
list.

4. **Error kinds.** After `const KIND_IO: &str = "io";`, add:

```rust
/// `daemon install` and `uninstall`: the service manager refused, or gave no answer (D9.4).
const KIND_SERVICE: &str = "service";
/// D9.3: no `launchctl` or `systemctl` to load the unit with.
const KIND_NO_SERVICE_MANAGER: &str = "no-service-manager";
/// `daemon status`: the daemon's socket answered something unusable, or nothing in time.
const KIND_DAEMON: &str = "daemon";
```

5. **`install_failure`.** After `auto_failure`, add:

```rust
/// How `daemon install` and `uninstall` fail as a command (D9.4): exit 1, with the manager's
/// own words. D9.2: launchd loads an agent only in a GUI login session, which a failed
/// `bootstrap`'s message says.
fn install_failure(e: InstallError) -> Failure {
    let kind = match &e {
        InstallError::Unit(_) => KIND_INVALID_INPUT,
        InstallError::Io(_) => KIND_IO,
        InstallError::NoManager(_) => KIND_NO_SERVICE_MANAGER,
        InstallError::Manager { .. } | InstallError::NoAnswer { .. } => KIND_SERVICE,
    };
    let message = match &e {
        InstallError::Manager { program, .. } if program == "launchctl bootstrap" => format!(
            "{e}; launchd loads an agent only in a GUI login session, not over SSH alone: run `tagteam daemon` in the foreground instead"
        ),
        _ => e.to_string(),
    };
    Failure::Message(kind, message)
}
```

6. **The methods.** In `impl App`, after `fn auto`, add:

```rust
    /// D8, D9: `daemon install`, `uninstall` and `status`. `tagteam daemon` with no action is
    /// the foreground run.
    fn daemon_action(&mut self, action: DaemonAction) -> Result<i32, Failure> {
        match action {
            DaemonAction::Install { print, no_start } => self.daemon_install(print, no_start),
            DaemonAction::Uninstall => self.daemon_uninstall(),
            DaemonAction::Status => self.daemon_status(),
        }
    }

    /// D9: writes the unit and loads it, restarting a daemon it already runs. `--no-start`
    /// writes it (and on Linux enables it) only, and `--print` prints it and changes nothing.
    /// It refuses inside a run shell, whose environment is a session's, not the one the daemon
    /// should keep: `--print` too, since what it prints would record that environment.
    fn daemon_install(&mut self, print: bool, no_start: bool) -> Result<i32, Failure> {
        if matches!(self.engine.run_shell(), RunShell::Inside { .. }) {
            return Err(EngineError::InsideRunShell.into());
        }
        let env = self.engine.env().clone();
        let kind = self.service();
        let program = service_cmd::install_program(&env).map_err(|e| {
            Failure::Message(
                KIND_IO,
                format!("this tagteam's own path cannot be read: {e}"),
            )
        })?;
        let recorded = recorded_env(&|name| env.var(name).map(OsStr::to_os_string))
            .map_err(|e| Failure::Message(KIND_INVALID_INPUT, e.to_string()))?;
        if print {
            let unit = render_unit(kind, &program, &recorded)
                .map_err(|e| Failure::Message(KIND_INVALID_INPUT, e.to_string()))?;
            let json = service_cmd::print_json(kind, &unit_path(&env, kind), &unit);
            self.print(&unit, json);
            return Ok(0);
        }
        let spawner = self.engine.spawner();
        let report =
            daemon_service::install(&env, kind, &program, &recorded, !no_start, spawner.as_ref())
                .map_err(install_failure)?;
        if !self.json {
            for w in &report.warnings {
                let _ = writeln!(self.io.err, "warning: {w}");
            }
        }
        self.print(
            &service_cmd::install_human(&report),
            service_cmd::install_json(&report),
        );
        Ok(0)
    }

    /// D9: stops the daemon the unit runs and removes the unit. With none installed it changes
    /// nothing and succeeds (D9.4). It refuses inside a run shell, as `install` does.
    fn daemon_uninstall(&mut self) -> Result<i32, Failure> {
        if matches!(self.engine.run_shell(), RunShell::Inside { .. }) {
            return Err(EngineError::InsideRunShell.into());
        }
        let env = self.engine.env().clone();
        let kind = self.service();
        let spawner = self.engine.spawner();
        let removed =
            daemon_service::uninstall(&env, kind, spawner.as_ref()).map_err(install_failure)?;
        self.print(
            &service_cmd::uninstall_human(kind, &unit_path(&env, kind), removed),
            service_cmd::uninstall_json(removed),
        );
        Ok(0)
    }

    /// D8.2: the daemon's own `status` from its socket, else what its lock and its service
    /// say. It exits 0 when a daemon runs, 3 when none does, and 1 on an error. `--json`
    /// prints its object either way (plan B Decision 6). It is read-only, so it answers inside
    /// a run shell too.
    fn daemon_status(&mut self) -> Result<i32, Failure> {
        let env = self.engine.env().clone();
        let kind = self.service();
        let found = service_cmd::find_daemon(&env)
            .map_err(|e| Failure::Message(KIND_DAEMON, e.to_string()))?;
        let spawner = self.engine.spawner();
        let state = service_state(&env, kind, spawner.as_ref(), MANAGER_TIMEOUT);
        let path = unit_path(&env, kind);
        self.print(
            &service_cmd::status_human(&found, kind, &path, &state),
            service_cmd::status_json(&found, kind, &path, &state),
        );
        Ok(if found.running() {
            0
        } else {
            service_cmd::EXIT_NOT_RUNNING
        })
    }
```

**`crates/tagteam/src/service_cmd.rs`.** Insert above `#[cfg(test)]`:

```rust
//! `tagteam daemon install`, `uninstall` and `status` (D8.2, D9.4). This module holds the
//! program a unit starts, what the daemon's socket or lock says of it, and each command's
//! human and JSON forms. `app.rs` runs the commands; `tagteam_daemon::service` and the engine
//! do the work.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tagteam_daemon::client::{ClientError, Connect, connect};
use tagteam_daemon::protocol::{Status, StatusJob, StatusNotifications, StatusProvider};
use tagteam_daemon::service::InstallReport;
use tagteam_daemon::watch::program_path;
use tagteam_engine::daemon::{daemon_lock_path, daemon_socket_path, read_daemon_record};
use tagteam_engine::service::{ServiceKind, ServiceState};
use tagteam_provider::{Env, ProcessStamp, Read};

/// D8.2: the LSB status convention's "not running".
pub(crate) const EXIT_NOT_RUNNING: i32 = 3;

/// D9: `install` always says this.
const UNATTENDED: &str = "The daemon will switch accounts unattended; pause it with `tagteam config set daemon.autoswitch false`.";

/// D9.1: the program a unit starts. That is the first `tagteam` on `PATH` when it is this
/// binary (a stable link from Homebrew or cargo, which survives an upgrade), else this
/// binary's own path. The relative argv[0] keeps the rule D9.1's alone: D4.5's preference for
/// an absolute argv[0] is for the daemon's own start.
pub(crate) fn install_program(env: &Env) -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(program_path(OsStr::new("tagteam"), env.var("PATH"), &exe))
}

/// The service a unit is, as `install`'s JSON names it (D9.4).
fn service_value(kind: ServiceKind, path: &Path) -> Value {
    json!({"kind": kind.as_str(), "path": path.display().to_string(), "label": kind.label()})
}

/// `install --print --json`: the unit, as a string, beside where it would go.
pub(crate) fn print_json(kind: ServiceKind, path: &Path, unit: &str) -> Value {
    json!({"schemaVersion": 1, "service": service_value(kind, path), "unit": unit})
}

pub(crate) fn install_human(r: &InstallReport) -> String {
    let done = if r.started {
        ", and started the daemon."
    } else {
        "; it starts at the next login."
    };
    format!(
        "Installed the {} at {}{done}\n{UNATTENDED}\n",
        r.kind.noun(),
        r.path.display()
    )
}

/// D9.4: `{schemaVersion, ok, service: {kind, path, label}, started, warnings}`.
pub(crate) fn install_json(r: &InstallReport) -> Value {
    json!({
        "schemaVersion": 1,
        "ok": true,
        "service": service_value(r.kind, &r.path),
        "started": r.started,
        "warnings": r.warnings,
    })
}

pub(crate) fn uninstall_human(kind: ServiceKind, path: &Path, removed: bool) -> String {
    if removed {
        format!(
            "Stopped the daemon and removed the {} at {}.\n",
            kind.noun(),
            path.display()
        )
    } else {
        format!("No {} is installed at {}.\n", kind.noun(), path.display())
    }
}

/// D9.4: `{schemaVersion, ok, removed}`.
pub(crate) fn uninstall_json(removed: bool) -> Value {
    json!({"schemaVersion": 1, "ok": true, "removed": removed})
}

/// What `daemon status` found (D8.2).
pub(crate) enum Found {
    /// The daemon answered `status` (D5.4).
    Answered(Box<Status>),
    /// A daemon speaks another protocol (D5.6): one from before an upgrade, which restarts on
    /// the new binary within a minute (D4.5). `pid` is its lock record's.
    Mismatch {
        server: u32,
        client: u32,
        pid: Option<u32>,
    },
    /// A live process holds `daemon.lock`, but no socket answers in time: there is none (D5.1),
    /// or it hangs (D8.2).
    Silent { pid: u32 },
    NotRunning,
}

impl Found {
    pub(crate) fn running(&self) -> bool {
        !matches!(self, Found::NotRunning)
    }

    fn pid(&self) -> Option<u32> {
        match self {
            Found::Answered(s) => Some(s.daemon.pid),
            Found::Mismatch { pid, .. } => *pid,
            Found::Silent { pid } => Some(*pid),
            Found::NotRunning => None,
        }
    }
}

/// D8.2: the daemon's own `status`, from its socket. Without an answer, it is the holder
/// `daemon.lock` names, judged by an exact pid and start-time match (§12.6). A daemon that
/// greets or answers too late (`Timeout`) is there but hung. With a live holder it runs and
/// does not answer, as D8.2 says. Without one, whatever listens is no daemon tagteam knows,
/// and the timeout is the error.
pub(crate) fn find_daemon(env: &Env) -> Result<Found, ClientError> {
    let holder = || match read_daemon_record(&daemon_lock_path(env)) {
        Read::Present(r) if (ProcessStamp { pid: r.pid, start: r.start }).is_live() => {
            Some(r.pid)
        }
        _ => None,
    };
    let silent = |pid: Option<u32>| pid.map_or(Found::NotRunning, |pid| Found::Silent { pid });
    let hung = || {
        holder()
            .map(|pid| Found::Silent { pid })
            .ok_or(ClientError::Timeout)
    };
    // No socket file means nothing to connect to. A path too long to bind (D5.1) can never
    // exist, and `connect` would report it as an error rather than find no daemon.
    if fs::symlink_metadata(daemon_socket_path(env)).is_err() {
        return Ok(silent(holder()));
    }
    match connect(env) {
        Ok(Connect::Connected(mut client)) => match client.status() {
            Ok(status) => Ok(Found::Answered(Box::new(status))),
            Err(ClientError::Timeout) => hung(),
            Err(e) => Err(e),
        },
        Ok(Connect::Mismatch { server, client }) => Ok(Found::Mismatch {
            server,
            client,
            pid: holder(),
        }),
        Ok(Connect::NoDaemon) => Ok(silent(holder())),
        Err(ClientError::Timeout) => hung(),
        Err(e) => Err(e),
    }
}

/// D8.2's JSON: `{schemaVersion, running, pid, daemon, service}`. `daemon` is D5.4's object,
/// with every field present and `null` when absent, or `null` without an answer. `service` is
/// `{kind, path, installed, loaded, lastExitStatus}`.
pub(crate) fn status_json(
    found: &Found,
    kind: ServiceKind,
    path: &Path,
    state: &ServiceState,
) -> Value {
    json!({
        "schemaVersion": 1,
        "running": found.running(),
        "pid": found.pid(),
        "daemon": match found {
            Found::Answered(s) => daemon_value(s),
            _ => Value::Null,
        },
        "service": {
            "kind": kind.as_str(),
            "path": path.display().to_string(),
            "installed": state.installed,
            "loaded": state.loaded,
            "lastExitStatus": state.last_exit_status,
        },
    })
}

/// D5.4's object, from the client's typed view of it.
fn daemon_value(s: &Status) -> Value {
    let providers: Vec<Value> = s
        .providers
        .iter()
        .map(|p| {
            json!({
                "provider": p.provider,
                "mode": p.mode,
                "autoswitch": p.autoswitch,
                "waitingFor": p.waiting_for,
                "holderPid": p.holder_pid,
                "backoff": p.backoff.as_ref().map(|b| json!({"reason": b.reason, "until": b.until})),
                "lastTickAt": p.last_tick_at,
                "nextTickAt": p.next_tick_at,
                "lastOutcome": p.last_outcome.as_ref().map(|o| json!({"kind": o.kind, "reason": o.reason, "at": o.at})),
                "lastSwitch": p.last_switch.as_ref().map(|w| json!({"at": w.at, "from": w.from, "to": w.to, "trigger": w.trigger})),
            })
        })
        .collect();
    let jobs: Vec<Value> = s
        .jobs
        .iter()
        .map(|j| {
            json!({
                "name": j.name,
                "lastRunAt": j.last_run_at,
                "nextRunAt": j.next_run_at,
                "lastError": j.last_error,
            })
        })
        .collect();
    let d = &s.daemon;
    let n = &s.notifications;
    json!({
        "daemon": {
            "pid": d.pid,
            "version": d.version,
            "protocol": d.protocol,
            "startedAt": d.started_at,
            "socket": d.socket,
            "log": d.log,
        },
        "notifications": {"enabled": n.enabled, "backend": n.backend, "events": n.events},
        "providers": providers,
        "jobs": jobs,
        "subscribers": s.subscribers,
    })
}

/// D8.2's text: what runs, then the service.
pub(crate) fn status_human(
    found: &Found,
    kind: ServiceKind,
    path: &Path,
    state: &ServiceState,
) -> String {
    let mut s = String::new();
    match found {
        Found::Answered(st) => {
            let d = &st.daemon;
            s.push_str(&format!(
                "The daemon runs as pid {}: tagteam {}, protocol {}, since {}.\n",
                d.pid, d.version, d.protocol, d.started_at
            ));
            s.push_str(&format!(
                "  socket: {}\n",
                d.socket.as_deref().unwrap_or("none (the log says why)")
            ));
            s.push_str(&format!("  log: {}\n", d.log));
            s.push_str(&notifications_line(&st.notifications));
            for p in &st.providers {
                s.push_str(&provider_line(p));
            }
            for j in &st.jobs {
                s.push_str(&job_line(j));
            }
            s.push_str(&format!("  subscribers: {}\n", st.subscribers));
        }
        Found::Mismatch {
            server,
            client,
            pid,
        } => {
            let who = pid.map(|p| format!(" as pid {p}")).unwrap_or_default();
            s.push_str(&format!(
                "A daemon runs{who}, but it speaks protocol {server} and this tagteam {client}; it restarts on the new tagteam within a minute.\n"
            ));
        }
        Found::Silent { pid } => s.push_str(&format!(
            "A daemon runs as pid {pid}, but its socket does not answer; the log says why.\n"
        )),
        Found::NotRunning => s.push_str("The daemon is not running.\n"),
    }
    s.push_str(&service_line(kind, path, state));
    s
}

fn notifications_line(n: &StatusNotifications) -> String {
    match (n.enabled, &n.backend) {
        (false, _) => "  notifications: off\n".to_owned(),
        (true, None) => "  notifications: on, but nothing here can show them\n".to_owned(),
        (true, Some(backend)) if n.events.is_empty() => {
            format!("  notifications: none chosen (notify.events), through {backend}\n")
        }
        (true, Some(backend)) => {
            format!("  notifications: {}, through {backend}\n", n.events.join(", "))
        }
    }
}

fn provider_line(p: &StatusProvider) -> String {
    let mut line = format!("  {}: {}", p.provider, p.mode);
    if let Some(what) = &p.waiting_for {
        line.push_str(&format!(" for {what}"));
        if let Some(pid) = p.holder_pid {
            line.push_str(&format!(" (pid {pid})"));
        }
    }
    if !p.autoswitch {
        line.push_str(", auto-switching paused");
    }
    if let Some(b) = &p.backoff {
        line.push_str(&format!(", backing off ({}) until {}", b.reason, b.until));
    }
    if let Some(o) = &p.last_outcome {
        let reason = o.reason.as_ref().map(|r| format!(" ({r})")).unwrap_or_default();
        line.push_str(&format!("; last {}{reason} at {}", o.kind, o.at));
    }
    if let Some(next) = &p.next_tick_at {
        line.push_str(&format!("; next at {next}"));
    }
    if let Some(w) = &p.last_switch {
        line.push_str(&format!(
            "; last switch {} → {} ({}) at {}",
            w.from, w.to, w.trigger, w.at
        ));
    }
    line.push('\n');
    line
}

fn job_line(j: &StatusJob) -> String {
    let mut line = format!("  {}:", j.name);
    match &j.last_run_at {
        Some(at) => line.push_str(&format!(" last run {at}")),
        None => line.push_str(" not run yet"),
    }
    if let Some(next) = &j.next_run_at {
        line.push_str(&format!(", next {next}"));
    }
    if let Some(e) = &j.last_error {
        line.push_str(&format!(", last error: {e}"));
    }
    line.push('\n');
    line
}

/// The service's line: whether the unit is installed, and what its manager says, or why that
/// cannot be told.
fn service_line(kind: ServiceKind, path: &Path, s: &ServiceState) -> String {
    let installed = if s.installed {
        "installed"
    } else {
        "not installed"
    };
    let state = match (&s.unknown, s.loaded, s.running, s.last_exit_status) {
        (Some(why), ..) => format!("; its state cannot be read ({why})"),
        (None, Some(true), Some(true), _) => ", loaded and running".to_owned(),
        (None, Some(true), _, Some(code)) => {
            format!(", loaded, not running (last exit status {code})")
        }
        (None, Some(true), _, None) => ", loaded, not running".to_owned(),
        (None, Some(false), ..) => ", not loaded".to_owned(),
        (None, None, ..) => String::new(),
    };
    format!(
        "Service: the {} at {} is {installed}{state}.\n",
        kind.noun(),
        path.display()
    )
}

```

- [ ] **Step 4: Run the tests and see them pass**

Run:
- `cargo test -p tagteam-daemon --lib service::`. Expected: PASS, 9 tests.
- `cargo test -p tagteam --lib cli::tests::daemon_alone`. Expected: PASS, 1 test.
- `cargo test -p tagteam --lib service_cmd`. Expected: PASS, 5 tests. Its socket test binds
  under `/tmp`, so a sandbox must allow binding there (plan B's execution notes).
- `cargo test -p tagteam --features test-support --test daemon_service_cli`. Expected: PASS,
  7 tests, and 1 ignored (`a_real_service_manager_starts_and_stops_the_daemon`, which Task 10
  runs by hand).
- `cargo test -p tagteam --features test-support`, then `cargo test --workspace`. Expected:
  PASS. Plan A's `daemon_cli.rs` still runs `tagteam daemon` in the foreground.

Then `cargo fmt --all && cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`.
Expected: clean.

- [ ] **Step 5: Commit** (the controller commits; implementers do not)

```
git add crates/tagteam-daemon/src/service.rs crates/tagteam-daemon/src/lib.rs \
  crates/tagteam/src/cli.rs crates/tagteam/src/app.rs \
  crates/tagteam/src/service_cmd.rs crates/tagteam/src/lib.rs \
  crates/tagteam/tests/daemon_service_cli.rs
git commit -m "Install, uninstall and report the daemon's service"
```

---

### Task 6: `doctor`'s daemon checks, and `auto` naming the daemon

**Depends on:** 2 (the CLI's `tests/common` `short_tempdir`), 4; plan A (Task 2's
`Engine::engine_holder`, Task 3's `tagteam_engine::daemon` with `MAX_SOCKET_PATH`, Task 5's
`Settings::notify_enabled`, Task 7's `AutoError` in `tagteam_daemon::auto`, Task 12's
`App.platform`)

D10 adds four checks to `doctor`, all read-only:
- `daemon.running`: `daemon.lock`'s record, judged by an exact pid and start-time match.
- `daemon.service`: the unit, what its manager says of it, the program it starts, and the
  variables it records against doctor's own.
- `daemon.socket`: the socket file against the daemon that should own it.
- `daemon.notify`: whether anything can show a notification.

The existing `auto.engine` check names the daemon when the engine lock's record matches
`daemon.lock`'s. D8.3 makes `auto`'s `engine-running` refusal do the same.

On the CLI side, this task:
- passes the platform's `ServiceKind` to `doctor` (a new `DoctorOptions::service`)
- derives that kind from plan A's `App.platform` (`App::service()`), for Task 5 too
- captures D9.1's variables into `Env.vars` at the process boundary, for doctor's comparison
  and for `install`

**Readings of the spec this task commits to:**
- **`DoctorOptions::service: Option<ServiceKind>`** is how the engine learns the platform's
  manager. The engine knows no platform, and the CLI's test-support override must reach it, so
  both units can be tested on either OS. `None`, the default and every existing engine test's,
  leaves out `daemon.service` and `daemon.notify`. `daemon.running` and `daemon.socket` need no
  platform and always run.
- **The manager is asked only about a unit that is there.** No unit means no `launchctl` and no
  `systemctl` (D10: "info: … not installed"). This also keeps every existing test free of new
  spawns.
- **Variables compared** (D9.1, D10, Decision 11): every recorded variable but `PATH`. `PATH`
  is recorded only so the daemon finds `notify-send`. It places no store or home, and a version
  manager (mise, direnv) changes it in every directory it enters, so comparing it would warn on
  almost every run. A variable counts as different when the unit records it and doctor's environment
  lacks it, or the other way round, or both have it with different values. The warning names
  the variables, never their values.
- **The socket's length** (D5.1, D10's fail) is judged only once a daemon runs or a unit is
  installed. Then the daemon runs, or would run, without its socket. With neither, the length
  is not judged, so `doctor` never fails for someone who does not use the daemon. A machine whose
  data path is long is told when it first installs or starts one (Decision 12). The limit is
  plan A's `MAX_SOCKET_PATH` (`tagteam_engine::daemon`), the bound the server's `bind` checks:
  103 bytes on macOS and 107 on Linux, `sun_path` less its NUL.
- **A live daemon with no socket file** warns: it runs without its socket (D5.1: a failed bind
  is logged and the daemon runs on). D10 did not list this state; it is the same condition as
  the too-long path, seen from the other side (Decision 13, and Task 8's erratum e).
- **`daemon.running`**: a liveness that cannot be told warns, as `auto.engine` does, and counts
  as running for the socket check, as `is_live` counts it.
- **The version doctor compares with** is `env!("CARGO_PKG_VERSION")` of the engine crate. Every
  crate takes the workspace's one version, as `transfer.rs`'s `tagteamVersion` does.
- **`daemon.notify`**: on macOS, `/usr/bin/osascript` is always the backend (D6.2). On Linux it
  is `notify-send` on the daemon's `PATH`: the one its unit records when doctor can read the
  unit, else doctor's own. `notify.enabled` is read from `config.toml` as the `auto` check reads
  its settings. Doctor adds ok and info states beside D10's warning.
- **A lingering that cannot be told warns** (Review Focus 5). On Linux, `loginctl` is asked
  beside `systemctl`. When it is missing, hangs or fails, `lingering` says why (Task 4), and
  `daemon.service` reports a `warn` with that reason and how to look (`loginctl show-user`),
  never silence and never a failure. Off is `info`, as D10 says.
- **The probes run under tokens of their own** (Task 4's `run_manager`, Decision 17), each
  bounded at 5 s. A signal meets doctor at its next cancellation point, the provider loop's
  `check_cancel`.
- **D8.3** names the daemon in the `engine-running` refusal, when every provider `auto` would
  drive is held, with the display name that refusal already uses ("Claude Code", Decision 16).
  The per-provider warning ("… in another process; skipping it") for a provider skipped beside
  others is unchanged: D8.3 speaks of the one refusal.
- **Capturing D9.1's variables.** They go into `Env.vars` after the presence markers of the
  variables a session scrubs (the run plan's Decision 18, which `app.rs`'s comment cites).
  `CLAUDE_SECURESTORAGE_CONFIG_DIR` is one of those.
  Its value replaces its marker, which keeps it present for the launch's warnings. It names a
  directory that the Env already holds raw, never a login.

**Files:**
- Modify: `crates/tagteam-engine/src/doctor.rs`:
  - the module doc and imports
  - `DoctorOptions::service`
  - constants
  - `engine_record_check` and its unit test
  - `doctor()`'s order
  - the `auto` check
  - `Run::daemon` and its four checks
  - `daemon_record_check`, `service_state_check` and `and_list`
- Modify: `crates/tagteam-engine/tests/doctor.rs`:
  - imports
  - `tree` skips a socket's bytes
  - `populated` gains the daemon's files
  - a new section of tests
- Modify: `crates/tagteam/src/app.rs`:
  - imports
  - `Context::from_process` captures `RECORDED_VARS`
  - `App::service()`
  - the `doctor` arm
  - `auto_failure`, `daemon_engine` and `App::auto`'s call
  - two unit tests
- Modify: `crates/tagteam/tests/doctor_cli.rs`:
  - imports
  - the JSON shape snapshot
  - the read-only fixture
  - a new test
- Modify: `crates/tagteam/tests/auto_cli.rs`:
  - imports
  - a new test

**Interfaces:**
- Consumes:
  - From Task 4:
    - `ServiceKind` (`noun`), `LAUNCHD_LABEL`, `SYSTEMD_UNIT`, `RECORDED_VARS`
    - `unit_path`, `parse_unit`, `UnitContents`, `render_unit` (tests)
    - `service_state`, `ServiceState`, `lingering`
    - `tagteam_provider::process::current_uid`
  - From Task 2: the CLI's `tests/common` `short_tempdir()`.
  - From plan A:
    - `tagteam_engine::daemon::{daemon_lock_path, daemon_socket_path, read_daemon_record,
      find_on_path, DaemonRecord, MAX_SOCKET_PATH}`
    - `Engine::engine_holder(&ProviderId) -> Read<ProcessStamp>`
    - `Settings::notify_enabled`
    - `AutoError::AlreadyRuns(Vec<ProviderId>)`
  - From plan A's Task 12: `App.platform: Platform`.
  - Existing: `Fx`, `Fx::with`, `Fx::engine_with_spawner`, `ScriptedSpawner` (`fx.spawner`),
    `dead_holder`, `FlockGuard::try_lock`, and `tests/common`'s `std_cmd`.
- Produces:
  - `DoctorOptions { pub online: bool, pub provider: Option<ProviderId>, pub service: Option<ServiceKind> }`
  - Check ids `daemon.running`, `daemon.service`, `daemon.socket` and `daemon.notify`.
  - `auto.engine`'s "auto-switch runs in the daemon (pid N)".
  - `App::service(&self) -> ServiceKind` (crate-private), from plan A's `App.platform`; Task 5
    calls it.
  - `RECORDED_VARS` captured into `Env.vars` by `Context::from_process`.
  - `fn auto_failure(e, name, daemon: &dyn Fn(&ProviderId) -> Option<u32>) -> Failure`.

#### Part A: the engine's checks

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/doctor.rs`, replace the imports:

```rust
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::*;
use tagteam_cc::live::Platform;
use tagteam_core::{AccountId, ProviderId, WindowKind};
use tagteam_engine::doctor::{DoctorOptions, DoctorReport};
use tagteam_engine::store::{DisplacedRow, Store};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::liveness::FakeProcess;
use tagteam_provider::{Check, CheckStatus, Keychain, ProcessStamp, Provider};
```

with:

```rust
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use tagteam_cc::live::Platform;
use tagteam_core::{AccountId, ProviderId, WindowKind};
use tagteam_engine::daemon::{daemon_lock_path, daemon_socket_path};
use tagteam_engine::doctor::{DoctorOptions, DoctorReport};
use tagteam_engine::service::{ServiceKind, render_unit, unit_path};
use tagteam_engine::store::{DisplacedRow, Store};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::liveness::FakeProcess;
use tagteam_provider::process::{Captured, ProcessSpawner, SpawnSpec, current_uid};
use tagteam_provider::{
    Cancel, Check, CheckStatus, Env, FlockGuard, Keychain, ProcessStamp, Provider,
};
```

In `tree`, a socket has no bytes to read. Replace:

```rust
            } else if path.to_string_lossy().ends_with("-shm") {
                out.insert(path, (mode, None));
```

with:

```rust
            } else if path.to_string_lossy().ends_with("-shm") || m.file_type().is_socket() {
                out.insert(path, (mode, None));
```

and add to `tree`'s doc comment: "A socket is listed without bytes too: it has none to read."

In `populated`, after the engine lock's record is written:

```rust
    fs::write(
        locks.join("autoswitch-claude-code.lock"),
        "{\"pid\":1,\"start\":1}\n",
    )
    .unwrap();
```

add:

```rust
    // D10: the daemon's lock record, and a socket file a daemon left.
    fs::write(
        locks.join("daemon.lock"),
        "{\"pid\":1,\"start\":1,\"version\":\"0\"}\n",
    )
    .unwrap();
    fs::write(fx.env.data_dir().join("daemon.sock"), "").unwrap();
```

Append to the end of the file:

```rust
// ---- The daemon (D10) ----

/// doctor as the CLI runs it: with this platform's service manager, `kind`.
fn doctor_as(fx: &Fx, kind: ServiceKind) -> DoctorReport {
    fx.engine
        .doctor(DoctorOptions {
            service: Some(kind),
            ..DoctorOptions::default()
        })
        .unwrap()
}

/// A directory short enough for a socket path (D14: macOS's `$TMPDIR` can push one past
/// `sun_path`): under `/tmp`, or under `$TMPDIR` where `/tmp` cannot be written. The daemon
/// crate's and the CLI's socket tests have a helper of the same name.
fn short_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("tt")
        .tempdir_in("/tmp")
        .or_else(|_| tempfile::tempdir())
        .unwrap()
}

/// A fixture for the daemon's checks. Its data directory is short enough for a socket path,
/// so the socket's length never decides a check, and its `PATH` (in `Env.vars`, where the CLI
/// captures it) is `<home>/bin`. `adjust` changes the Env further, before anything is made.
/// The directory must outlive the fixture.
fn daemon_fx(adjust: impl FnOnce(&mut Env)) -> (tempfile::TempDir, Fx) {
    let short = short_tempdir();
    let data = short.path().join("d");
    let fx = Fx::with(Platform::MacOs, |e| {
        e.xdg_data_home = Some(data);
        let bin = e.home.join("bin");
        e.vars.insert("PATH".into(), bin.into_os_string());
        adjust(e);
    });
    (short, fx)
}

/// An executable `name` in `<home>/bin`, for doctor to find on `PATH`. It never runs: the
/// fixture's scripted spawner answers in its place.
fn manager_stub(fx: &Fx, name: &str) {
    let bin = fx.env.home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let file = bin.join(name);
    fs::write(&file, "#!/bin/sh\nexit 99\n").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
}

/// D4.1's record in `daemon.lock`, naming `stamp`.
fn daemon_lock_record(fx: &Fx, stamp: ProcessStamp, version: &str) {
    let lock = daemon_lock_path(&fx.env);
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    fs::write(
        &lock,
        format!(
            "{{\"pid\":{},\"start\":{},\"version\":\"{version}\"}}\n",
            stamp.pid, stamp.start
        ),
    )
    .unwrap();
}

/// The unit `daemon install` writes for `program` and `env`, at its path.
fn plant_unit(fx: &Fx, kind: ServiceKind, program: &Path, env: &[(String, String)]) -> PathBuf {
    let path = unit_path(&fx.env, kind);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, render_unit(kind, program, env).unwrap()).unwrap();
    path
}

fn answered(code: i32, stdout: &str, stderr: &str) -> Captured {
    Captured::Exited {
        code: Some(code),
        signal: None,
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

/// A program that exists: this test binary.
fn existing_program() -> PathBuf {
    std::env::current_exe().unwrap()
}

#[test]
fn the_daemon_runs_by_its_lock_record_and_another_version_is_told_to_restart() {
    let fx = Fx::new();
    let lock = daemon_lock_path(&fx.env);
    let running = |fx: &Fx| one(&doctor(fx), "daemon.running").clone();
    let c = running(&fx);
    assert_eq!(
        (c.status, c.message.as_str()),
        (CheckStatus::Info, "the daemon is not running")
    );
    assert!(!lock.exists(), "doctor never tries, so never creates, the lock");
    let me = ProcessStamp::current().unwrap();
    let this = env!("CARGO_PKG_VERSION");
    daemon_lock_record(&fx, me, this);
    let c = running(&fx);
    assert_eq!(
        (c.status, c.message.clone(), c.fix.clone()),
        (
            CheckStatus::Info,
            format!("the daemon runs (pid {}, version {this})", me.pid),
            None
        )
    );
    daemon_lock_record(&fx, me, "19990101.0000.0");
    let c = running(&fx);
    assert_eq!(c.status, CheckStatus::Warn);
    assert_eq!(
        c.message,
        format!(
            "the daemon runs (pid {}) on version 19990101.0000.0, not this tagteam's {this}",
            me.pid
        )
    );
    assert_eq!(
        fix(&c),
        "restart it: a daemon restarts by itself within a minute of an upgrade; if this one has not, `tagteam daemon install` restarts the service's daemon, and a foreground one restarts once you stop it (Ctrl-C) and start it again"
    );
    daemon_lock_record(&fx, dead_holder(), this);
    let c = running(&fx);
    assert_eq!(
        (c.status, c.message.clone()),
        (
            CheckStatus::Info,
            format!(
                "the daemon is not running (its last run, pid {}, has exited)",
                i32::MAX
            )
        )
    );
    fs::write(&lock, "garbage").unwrap();
    let c = running(&fx);
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(
        c.message
            .starts_with("whether the daemon runs cannot be told: "),
        "{}",
        c.message
    );
    fix(&c);
}

#[test]
fn an_engine_lock_held_by_the_daemon_names_the_daemon() {
    // D10: the auto-switch check says "the daemon (pid N)" when the engine lock's record
    // matches daemon.lock's.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let me = ProcessStamp::current().unwrap();
    let engine_lock = fx.env.data_dir().join("locks/autoswitch-claude-code.lock");
    fs::create_dir_all(engine_lock.parent().unwrap()).unwrap();
    fs::write(
        &engine_lock,
        format!("{{\"pid\":{},\"start\":{}}}\n", me.pid, me.start),
    )
    .unwrap();
    daemon_lock_record(&fx, me, env!("CARGO_PKG_VERSION"));
    let c = one(&doctor(&fx), "auto.engine").clone();
    assert_eq!(
        (c.status, c.message),
        (
            CheckStatus::Info,
            format!("auto-switch runs in the daemon (pid {})", me.pid)
        )
    );
    // Another start time: not the daemon, whatever its pid.
    let other = ProcessStamp {
        pid: me.pid,
        start: me.start + 1,
    };
    daemon_lock_record(&fx, other, env!("CARGO_PKG_VERSION"));
    assert_eq!(
        one(&doctor(&fx), "auto.engine").message,
        format!("auto-switch runs as pid {}", me.pid)
    );
}

#[test]
fn the_service_unit_is_reported_with_what_its_manager_says() {
    for kind in [ServiceKind::Launchd, ServiceKind::Systemd] {
        let (_short, fx) = daemon_fx(|_| {});
        for name in ["launchctl", "systemctl", "loginctl"] {
            manager_stub(&fx, name);
        }
        let linux = kind == ServiceKind::Systemd;
        let path = unit_path(&fx.env, kind);
        let r = doctor_as(&fx, kind);
        let c = one(&r, "daemon.service");
        assert_eq!(
            (c.status, c.message.clone(), c.fix.clone()),
            (
                CheckStatus::Info,
                format!(
                    "the daemon's {} is not installed ({})",
                    kind.noun(),
                    path.display()
                ),
                None
            )
        );
        assert!(
            fx.spawner.specs().is_empty(),
            "nothing is asked about a unit that is not there"
        );

        plant_unit(&fx, kind, &existing_program(), &[]);
        let unit = format!(
            "the daemon's {} is installed at {}",
            kind.noun(),
            path.display()
        );
        let (running, stopped, unloaded) = match kind {
            ServiceKind::Launchd => (
                answered(
                    0,
                    "gui/501/io.github.michaelrigart.tagteam = {\n\tstate = running\n\tlast exit code = (never exited)\n}\n",
                    "",
                ),
                answered(
                    0,
                    "gui/501/io.github.michaelrigart.tagteam = {\n\tstate = not running\n\tlast exit code = 1\n}\n",
                    "",
                ),
                answered(
                    113,
                    "",
                    "Could not find service \"io.github.michaelrigart.tagteam\" in domain for user gui: 501",
                ),
            ),
            ServiceKind::Systemd => (
                answered(0, "LoadState=loaded\nActiveState=active\nExecMainStatus=0\n", ""),
                answered(0, "LoadState=loaded\nActiveState=failed\nExecMainStatus=1\n", ""),
                answered(0, "LoadState=not-found\nActiveState=inactive\nExecMainStatus=0\n", ""),
            ),
        };
        let linger = |on: &str| answered(0, &format!("Linger={on}\n"), "");

        fx.spawner.push(running);
        if linux {
            fx.spawner.push(linger("yes"));
        }
        let r = doctor_as(&fx, kind);
        let c = one(&r, "daemon.service");
        assert_eq!(
            (c.status, c.message.clone()),
            (CheckStatus::Info, format!("{unit}, loaded and running"))
        );

        fx.spawner.push(stopped);
        if linux {
            fx.spawner.push(linger("no"));
        }
        let r = doctor_as(&fx, kind);
        let c = found(&r, "daemon.service");
        assert_eq!(
            c[0].message,
            format!("{unit}, loaded but not running (its last exit status was 1)")
        );
        if linux {
            assert_eq!(c.len(), 2, "{c:#?}");
            assert_eq!(
                (c[1].status, c[1].message.as_str()),
                (
                    CheckStatus::Info,
                    "lingering is off for tester, so systemd stops the daemon when you log out"
                )
            );
            assert_eq!(fix(c[1]), "`loginctl enable-linger tester` keeps it running");
        } else {
            assert_eq!(c.len(), 1, "{c:#?}");
        }

        fx.spawner.push(unloaded);
        if linux {
            fx.spawner.push(linger("yes"));
        }
        let r = doctor_as(&fx, kind);
        let c = one(&r, "daemon.service");
        assert_eq!(
            (c.status, c.message.clone()),
            (
                CheckStatus::Info,
                format!("{unit} but not loaded, so it starts at the next login")
            )
        );
        assert_eq!(fix(c), "`tagteam daemon install` loads and starts it now");

        let asked = fx.spawner.specs();
        assert_eq!(asked.len(), if linux { 6 } else { 3 });
        let manager = if linux { "systemctl" } else { "launchctl" };
        assert_eq!(asked[0].program, fx.env.home.join("bin").join(manager));
    }
}

#[test]
fn a_unit_whose_program_is_gone_fails_and_one_recording_another_store_warns() {
    // No manager on PATH here: the unit's state is unknown, which
    // doctor_reports_a_hanging_or_missing_service_manager_as_a_warning pins.
    let (_short, fx) = daemon_fx(|e| {
        e.vars.insert("XDG_STATE_HOME".into(), "/state".into());
    });
    let kind = ServiceKind::Launchd;
    let path = plant_unit(&fx, kind, Path::new("/nowhere/tagteam"), &[]);
    let r = doctor_as(&fx, kind);
    let gone = found(&r, "daemon.service")
        .into_iter()
        .find(|c| c.status == CheckStatus::Fail)
        .unwrap_or_else(|| panic!("{:#?}", r.checks))
        .clone();
    assert_eq!(
        gone.message,
        format!(
            "{} starts /nowhere/tagteam, which does not exist, so the service cannot start the daemon",
            path.display()
        )
    );
    assert_eq!(
        fix(&gone),
        "`tagteam daemon install` writes the unit again for this tagteam"
    );
    assert!(!r.ok());

    // Recorded: another data directory, a proxy with a password, and another PATH. Doctor's
    // own environment has a state directory the unit does not record.
    let recorded = vec![
        ("XDG_DATA_HOME".to_owned(), "/old/data".to_owned()),
        (
            "HTTPS_PROXY".to_owned(),
            "http://u:secret@proxy:3128".to_owned(),
        ),
        ("PATH".to_owned(), "/somewhere/else".to_owned()),
    ];
    plant_unit(&fx, kind, &existing_program(), &recorded);
    let r = doctor_as(&fx, kind);
    let service = found(&r, "daemon.service");
    let differ = service
        .iter()
        .find(|c| c.message.contains("differently"))
        .unwrap_or_else(|| panic!("{service:#?}"));
    assert_eq!(differ.status, CheckStatus::Warn);
    assert_eq!(
        differ.message,
        format!(
            "{} records XDG_DATA_HOME, XDG_STATE_HOME and HTTPS_PROXY differently from this environment, so the daemon may use another store, home, proxy or log level",
            path.display()
        )
    );
    assert!(!differ.message.contains("secret") && !differ.message.contains("/old/data"));
    assert_eq!(
        fix(differ),
        "run `tagteam daemon install` again from the environment whose store and home the daemon should use"
    );
    assert!(r.ok(), "a difference is a warning: {:#?}", r.checks);

    // A unit tagteam did not write as it stands is installed, and not judged.
    fs::write(
        &path,
        fs::read_to_string(&path).unwrap().replace("<true/>", "<false/>"),
    )
    .unwrap();
    let r = doctor_as(&fx, kind);
    let service = found(&r, "daemon.service");
    assert!(
        service.iter().any(|c| c.status == CheckStatus::Info
            && c.message
                == format!(
                    "{} was not written by tagteam as it stands, so the program it starts and its environment are not checked",
                    path.display()
                )),
        "{service:#?}"
    );
    assert!(service.iter().all(|c| !c.message.contains("differently")));
}

/// A service manager that never answers: every call runs past its timeout. It records each
/// timeout it was given.
#[derive(Default)]
struct Hanging(Mutex<Vec<Duration>>);

impl ProcessSpawner for Hanging {
    fn run_captured(&self, _: &SpawnSpec, timeout: Duration, _: &Cancel) -> Captured {
        self.0.lock().unwrap().push(timeout);
        Captured::TimedOut
    }
}

#[test]
fn doctor_reports_a_hanging_or_missing_service_manager_as_a_warning() {
    // Review Focus 5: launchctl or systemctl missing, hanging or failing during doctor. Each
    // is a warning with its reason, asked within 5 s, and never fails the run by itself. On
    // Linux, `loginctl` beside it is judged the same way.
    for (kind, manager) in [
        (ServiceKind::Launchd, "launchctl"),
        (ServiceKind::Systemd, "systemctl"),
    ] {
        let (_short, fx) = daemon_fx(|_| {});
        let path = plant_unit(&fx, kind, &existing_program(), &[]);
        let head = format!(
            "the daemon's {} is installed at {}, but whether it is loaded cannot be told: ",
            kind.noun(),
            path.display()
        );
        let unknown = |r: &DoctorReport| -> Check {
            let warned: Vec<&Check> = found(r, "daemon.service")
                .into_iter()
                .filter(|c| c.status == CheckStatus::Warn && c.message.starts_with(&head))
                .collect();
            assert_eq!(warned.len(), 1, "{:#?}", r.checks);
            warned[0].clone()
        };
        // On Linux, lingering's warning and its reason; none on macOS.
        let lingering = |r: &DoctorReport, why: &str| {
            let said: Vec<(CheckStatus, String)> = found(r, "daemon.service")
                .into_iter()
                .filter(|c| c.message.starts_with("whether lingering is on"))
                .map(|c| (c.status, c.message.clone()))
                .collect();
            let want = match kind {
                ServiceKind::Launchd => vec![],
                ServiceKind::Systemd => vec![(
                    CheckStatus::Warn,
                    format!("whether lingering is on for tester cannot be told: {why}"),
                )],
            };
            assert_eq!(said, want, "{:#?}", r.checks);
        };
        let fix_for = match kind {
            ServiceKind::Launchd => {
                "`launchctl print gui/$(id -u)/io.github.michaelrigart.tagteam` shows its state"
            }
            ServiceKind::Systemd => "`systemctl --user status tagteam.service` shows its state",
        };

        // Missing: no manager on PATH, and nothing spawned.
        let r = doctor_as(&fx, kind);
        let c = unknown(&r);
        assert_eq!(c.message, format!("{head}{manager} is not on PATH"));
        assert_eq!(fix(&c), fix_for);
        lingering(&r, "loginctl is not on PATH");
        assert!(r.ok(), "{:#?}", r.checks);
        assert!(fx.spawner.specs().is_empty());

        // Hanging: every call is bounded at 5 s.
        manager_stub(&fx, manager);
        manager_stub(&fx, "loginctl");
        let hanging = Arc::new(Hanging::default());
        let r = fx
            .engine_with_spawner(hanging.clone())
            .doctor(DoctorOptions {
                service: Some(kind),
                ..DoctorOptions::default()
            })
            .unwrap();
        assert_eq!(
            unknown(&r).message,
            format!("{head}{manager} did not answer within 5 s")
        );
        lingering(&r, "loginctl did not answer within 5 s");
        assert!(r.ok(), "{:#?}", r.checks);
        let asked = hanging.0.lock().unwrap().clone();
        assert_eq!(asked.len(), if kind == ServiceKind::Systemd { 2 } else { 1 });
        assert!(
            asked.iter().all(|t| *t == Duration::from_secs(5)),
            "{asked:?}"
        );

        // Failing: its exit and its own words.
        fx.spawner.push(answered(1, "", "Bad request.\n"));
        if kind == ServiceKind::Systemd {
            fx.spawner.push(answered(1, "", ""));
        }
        let r = doctor_as(&fx, kind);
        let command = match kind {
            ServiceKind::Launchd => format!(
                "launchctl print gui/{}/io.github.michaelrigart.tagteam",
                current_uid()
            ),
            ServiceKind::Systemd => "systemctl --user show tagteam.service".to_owned(),
        };
        assert_eq!(
            unknown(&r).message,
            format!("{head}`{command}` exited 1: Bad request.")
        );
        lingering(&r, "`loginctl show-user tester -p Linger` exited 1");
        assert!(r.ok(), "{:#?}", r.checks);
    }
}

#[test]
fn lingering_that_cannot_be_told_is_a_warning_with_its_reason_while_systemctl_answers() {
    // Review Focus 5, D9.3: `systemctl` describes the unit, but `loginctl` is missing, fails or
    // hangs. Each is a warning that names why, beside the unit's own line, and never fails the
    // run.
    let (_short, fx) = daemon_fx(|_| {});
    manager_stub(&fx, "systemctl");
    let path = plant_unit(&fx, ServiceKind::Systemd, &existing_program(), &[]);
    let loaded = || {
        answered(
            0,
            "LoadState=loaded\nActiveState=active\nExecMainStatus=0\n",
            "",
        )
    };
    let lingering = |r: &DoctorReport| -> Check {
        let said: Vec<&Check> = found(r, "daemon.service")
            .into_iter()
            .filter(|c| c.message.contains("lingering"))
            .collect();
        assert_eq!(said.len(), 1, "{:#?}", r.checks);
        said[0].clone()
    };
    let running = format!(
        "the daemon's systemd user unit is installed at {}, loaded and running",
        path.display()
    );

    // Missing: no `loginctl` on PATH, so only `systemctl` is asked.
    fx.spawner.push(loaded());
    let r = doctor_as(&fx, ServiceKind::Systemd);
    assert_eq!(found(&r, "daemon.service")[0].message, running);
    let c = lingering(&r);
    assert_eq!(
        (c.status, c.message.as_str()),
        (
            CheckStatus::Warn,
            "whether lingering is on for tester cannot be told: loginctl is not on PATH"
        )
    );
    assert_eq!(fix(&c), "`loginctl show-user tester -p Linger` shows it");
    assert!(r.ok(), "{:#?}", r.checks);
    assert_eq!(fx.spawner.specs().len(), 1);

    // Failing: its exit and its own words.
    manager_stub(&fx, "loginctl");
    fx.spawner.push(loaded());
    fx.spawner.push(answered(
        1,
        "",
        "Failed to get user: User ID 1000 is not logged in or lingering\n",
    ));
    let r = doctor_as(&fx, ServiceKind::Systemd);
    assert_eq!(
        lingering(&r).message,
        "whether lingering is on for tester cannot be told: `loginctl show-user tester -p Linger` exited 1: Failed to get user: User ID 1000 is not logged in or lingering"
    );
    assert!(r.ok(), "{:#?}", r.checks);

    // Hanging: past its 5 s.
    fx.spawner.push(loaded());
    fx.spawner.push(Captured::TimedOut);
    let r = doctor_as(&fx, ServiceKind::Systemd);
    assert_eq!(
        lingering(&r).message,
        "whether lingering is on for tester cannot be told: loginctl did not answer within 5 s"
    );
    assert!(r.ok(), "{:#?}", r.checks);
}

#[test]
fn the_socket_is_judged_against_the_daemon_that_should_own_it() {
    let (_short, fx) = daemon_fx(|_| {});
    let sock = daemon_socket_path(&fx.env);
    let socket = |fx: &Fx| one(&doctor(fx), "daemon.socket").clone();
    let c = socket(&fx);
    assert_eq!(
        (c.status, c.message.as_str()),
        (CheckStatus::Ok, "no daemon runs, and none left a socket behind")
    );
    fs::create_dir_all(sock.parent().unwrap()).unwrap();
    fs::write(&sock, "").unwrap();
    fs::set_permissions(&sock, fs::Permissions::from_mode(0o600)).unwrap();
    let c = socket(&fx);
    assert_eq!(
        (c.status, c.message.clone()),
        (
            CheckStatus::Info,
            format!(
                "{} is left from a daemon that is not running; the next one to start replaces it",
                sock.display()
            )
        )
    );
    fs::set_permissions(&sock, fs::Permissions::from_mode(0o644)).unwrap();
    let c = socket(&fx);
    assert_eq!(
        (c.status, c.message.clone()),
        (
            CheckStatus::Warn,
            format!(
                "{} has mode 644, not 600: another user may reach the daemon through it",
                sock.display()
            )
        )
    );
    assert_eq!(fix(&c), format!("rm '{}'", sock.display()));

    daemon_lock_record(&fx, ProcessStamp::current().unwrap(), env!("CARGO_PKG_VERSION"));
    let c = socket(&fx);
    assert_eq!(c.status, CheckStatus::Warn);
    assert_eq!(
        fix(&c),
        format!(
            "`chmod 600 '{}'`, or restart the daemon, which creates it 0600",
            sock.display()
        )
    );
    fs::set_permissions(&sock, fs::Permissions::from_mode(0o600)).unwrap();
    let c = socket(&fx);
    assert_eq!(
        (c.status, c.message.clone()),
        (
            CheckStatus::Ok,
            format!("the daemon listens on {}", sock.display())
        )
    );
    fs::remove_file(&sock).unwrap();
    let c = socket(&fx);
    assert_eq!(
        (c.status, c.message.clone()),
        (
            CheckStatus::Warn,
            format!(
                "the daemon runs without its socket ({} does not exist), so `tagteam daemon status` and the TUI cannot reach it",
                sock.display()
            )
        )
    );
    assert_eq!(
        fix(&c),
        "the log says why it could not bind; restart the daemon once that is fixed"
    );
}

#[test]
fn a_socket_path_too_long_to_bind_fails_once_a_daemon_runs_or_is_installed() {
    // D5.1, D10. Without a daemon or a unit the length is not judged: doctor never fails for
    // someone who does not use the daemon.
    let fx = Fx::with(Platform::MacOs, |e| {
        e.xdg_data_home = Some(e.home.join("d".repeat(120)))
    });
    let sock = daemon_socket_path(&fx.env);
    assert!(sock.as_os_str().len() > 108);
    assert_eq!(one(&doctor(&fx), "daemon.socket").status, CheckStatus::Ok);

    let unit = plant_unit(&fx, ServiceKind::Launchd, &existing_program(), &[]);
    let r = doctor_as(&fx, ServiceKind::Launchd);
    let c = one(&r, "daemon.socket");
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(
        c.message.starts_with(&format!(
            "{} is {} bytes, too long for a socket here (at most ",
            sock.display(),
            sock.as_os_str().len()
        )),
        "{}",
        c.message
    );
    assert!(
        c.message
            .ends_with("), so the daemon runs without it and nothing can reach it"),
        "{}",
        c.message
    );
    assert_eq!(
        fix(c),
        "set XDG_DATA_HOME to a shorter directory, then `tagteam daemon install` again"
    );
    assert!(!r.ok());

    fs::remove_file(&unit).unwrap();
    daemon_lock_record(&fx, ProcessStamp::current().unwrap(), env!("CARGO_PKG_VERSION"));
    assert_eq!(
        one(&doctor(&fx), "daemon.socket").status,
        CheckStatus::Fail,
        "a live daemon runs without it"
    );
}

#[test]
fn notifications_that_are_on_with_nothing_to_show_them_warn() {
    // D6.2, D10.
    let (_short, fx) = daemon_fx(|_| {});
    let c = one(&doctor_as(&fx, ServiceKind::Launchd), "daemon.notify").clone();
    assert_eq!(
        (c.status, c.message.as_str()),
        (CheckStatus::Ok, "notifications are on, through osascript")
    );
    let r = doctor_as(&fx, ServiceKind::Systemd);
    let c = one(&r, "daemon.notify");
    assert_eq!(
        (c.status, c.message.as_str()),
        (
            CheckStatus::Warn,
            "notifications are on, but notify-send is not on the daemon's PATH, so none is shown"
        )
    );
    assert_eq!(
        fix(c),
        "install notify-send (libnotify), or turn them off with `tagteam config set notify.enabled false`"
    );
    manager_stub(&fx, "notify-send");
    let c = one(&doctor_as(&fx, ServiceKind::Systemd), "daemon.notify").clone();
    assert_eq!(
        (c.status, c.message),
        (
            CheckStatus::Ok,
            format!(
                "notifications are on, through {}",
                fx.env.home.join("bin/notify-send").display()
            )
        )
    );

    // The daemon's own PATH, as its unit records it, is the one that counts.
    fs::remove_file(fx.env.home.join("bin/notify-send")).unwrap();
    let elsewhere = fx.dir.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let notify = elsewhere.join("notify-send");
    fs::write(&notify, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&notify, fs::Permissions::from_mode(0o755)).unwrap();
    plant_unit(
        &fx,
        ServiceKind::Systemd,
        &existing_program(),
        &[("PATH".to_owned(), elsewhere.display().to_string())],
    );
    let c = one(&doctor_as(&fx, ServiceKind::Systemd), "daemon.notify").clone();
    assert_eq!(
        c.message,
        format!("notifications are on, through {}", notify.display())
    );

    fs::create_dir_all(fx.env.config_dir()).unwrap();
    fs::write(
        fx.env.config_dir().join("config.toml"),
        "notify.enabled = false\n",
    )
    .unwrap();
    let c = one(&doctor_as(&fx, ServiceKind::Systemd), "daemon.notify").clone();
    assert_eq!(
        (c.status, c.message.as_str()),
        (CheckStatus::Info, "notifications are off (notify.enabled is false)")
    );
}

#[test]
fn the_daemon_checks_change_no_byte_take_no_lock_and_never_connect() {
    // §13.6, D10: doctor reads daemon.lock's record without trying the lock, looks at the
    // socket without connecting to it, and asks the service manager only to describe the
    // unit. Every file stays byte for byte as it was.
    use std::os::unix::net::UnixListener;
    let (short, fx) = daemon_fx(|_| {});
    for name in ["launchctl", "systemctl", "loginctl"] {
        manager_stub(&fx, name);
    }
    daemon_lock_record(&fx, ProcessStamp::current().unwrap(), env!("CARGO_PKG_VERSION"));
    let held = FlockGuard::try_lock(&daemon_lock_path(&fx.env))
        .unwrap()
        .expect("this test holds daemon.lock, as a daemon does");
    let sock = daemon_socket_path(&fx.env);
    let listener = UnixListener::bind(&sock).unwrap();
    listener.set_nonblocking(true).unwrap();
    fs::set_permissions(&sock, fs::Permissions::from_mode(0o600)).unwrap();
    let elsewhere = vec![("XDG_DATA_HOME".to_owned(), "/elsewhere".to_owned())];
    for kind in [ServiceKind::Launchd, ServiceKind::Systemd] {
        plant_unit(&fx, kind, &existing_program(), &elsewhere);
    }
    let before = (tree(fx.dir.path()), tree(short.path()));
    fx.spawner
        .push(answered(0, "x = {\n\tstate = running\n}\n", ""));
    fx.spawner.push(answered(
        0,
        "LoadState=loaded\nActiveState=active\nExecMainStatus=0\n",
        "",
    ));
    fx.spawner.push(answered(0, "Linger=yes\n", ""));
    for kind in [ServiceKind::Launchd, ServiceKind::Systemd] {
        let r = doctor_as(&fx, kind);
        assert_eq!(one(&r, "daemon.running").status, CheckStatus::Info);
        assert_eq!(one(&r, "daemon.socket").status, CheckStatus::Ok);
        assert!(
            found(&r, "daemon.service")
                .iter()
                .any(|c| c.message.ends_with("loaded and running")),
            "{:#?}",
            r.checks
        );
    }
    assert_eq!(
        changed(&before.0, &tree(fx.dir.path())),
        Vec::<PathBuf>::new()
    );
    assert_eq!(
        changed(&before.1, &tree(short.path())),
        Vec::<PathBuf>::new()
    );
    assert!(
        matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "doctor never connected"
    );
    drop(held);
}
```

In `crates/tagteam-engine/src/doctor.rs`, replace the unit test
`an_engine_whose_liveness_cannot_be_told_is_a_warning_with_a_fix` with:

```rust
    #[test]
    fn an_engine_whose_liveness_cannot_be_told_is_a_warning_with_a_fix() {
        let c = engine_record_check(4242, Liveness::Unknown("permission denied".into()), false);
        assert_eq!(c.id, "auto.engine");
        assert_eq!(c.status, CheckStatus::Warn);
        assert!(c.message.contains("4242") && c.message.contains("permission denied"));
        assert!(c.fix.as_deref().is_some_and(|f| f.contains("pid 4242")));
        assert_eq!(
            engine_record_check(4242, Liveness::Live, false).status,
            CheckStatus::Info
        );
        assert_eq!(
            engine_record_check(4242, Liveness::Dead, false).status,
            CheckStatus::Ok
        );
        // D10: a record that is daemon.lock's names the daemon.
        assert_eq!(
            engine_record_check(4242, Liveness::Live, true).message,
            "auto-switch runs in the daemon (pid 4242)"
        );
        let c = engine_record_check(4242, Liveness::Unknown("permission denied".into()), true);
        assert_eq!(
            c.message,
            "whether auto-switch runs as the daemon (pid 4242) cannot be told: the process's start time cannot be read (permission denied)"
        );
        assert_eq!(
            c.fix.as_deref(),
            Some(
                "check whether pid 4242 is still running a `tagteam daemon`, then `tagteam doctor` again"
            )
        );
    }

    #[test]
    fn a_list_of_names_reads_as_a_sentence() {
        assert_eq!(and_list(&["A"]), "A");
        assert_eq!(and_list(&["A", "B"]), "A and B");
        assert_eq!(and_list(&["A", "B", "C"]), "A, B and C");
    }
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test doctor`
Expected: the build fails with `error[E0560]: struct \`DoctorOptions\` has no field named
\`service\``.

Run: `cargo test -p tagteam-engine --lib doctor`
Expected: the build fails with `error[E0061]: this function takes 2 arguments but 3 arguments
were supplied` (`engine_record_check`) and `error[E0425]: cannot find function \`and_list\``.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/doctor.rs`:

1. **The module doc.** Replace its last two lines:

```rust
//! `claude auth status` in a home whose global config does not exist. Every finding names its
//! fix.
```

with:

```rust
//! `claude auth status` in a home whose global config does not exist. Every finding names its
//! fix. The daemon's checks (D10) read `daemon.lock`'s record without trying the lock. They
//! look at its socket without connecting to it, and ask `launchctl`, `systemctl` or
//! `loginctl` only to describe its service, each within 5 s.
```

2. **Imports.** Replace `use std::ffi::OsString;` with `use std::ffi::{OsStr, OsString};`.
   Replace:

```rust
use tagteam_provider::{
    Check, CheckStatus, Env, Liveness, LockProbe, LockState, Provenance, Provider, Read, holders_of,
};
```

with:

```rust
use tagteam_provider::process::current_uid;
use tagteam_provider::{
    Check, CheckStatus, Env, Liveness, LockProbe, LockState, ProcessStamp, Provenance, Provider,
    Read, holders_of,
};
```

After `use crate::auto::{engine_lock_path, read_holder};` add:

```rust
use crate::daemon::{
    DaemonRecord, MAX_SOCKET_PATH, daemon_lock_path, daemon_socket_path, find_on_path,
    read_daemon_record,
};
```

After `use crate::recover::Direction;` add:

```rust
use crate::service::{
    LAUNCHD_LABEL, RECORDED_VARS, SYSTEMD_UNIT, ServiceKind, ServiceState, UnitContents,
    lingering, parse_unit, service_state, unit_path,
};
```

3. **`DoctorOptions`.** Replace:

```rust
    /// `--provider`: only this provider's checks, besides tagteam's own (§13.1).
    pub provider: Option<ProviderId>,
}
```

with:

```rust
    /// `--provider`: only this provider's checks, besides tagteam's own (§13.1).
    pub provider: Option<ProviderId>,
    /// This platform's service manager (D9), which the CLI names from its platform. `None`
    /// leaves out the checks that need one: `daemon.service` and `daemon.notify`.
    pub service: Option<ServiceKind>,
}
```

4. **Constants.** After:

```rust
/// How long `--online` waits for each host (§13.6).
const ONLINE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
```

add:

```rust

/// D10: how long `launchctl print`, `systemctl --user show` and `loginctl show-user` may take.
const MANAGER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
```

The socket path's bound is plan A's `MAX_SOCKET_PATH`, imported above, the one the server's
`bind` checks.

5. **`engine_record_check`.** Replace the whole function with:

```rust
/// `auto.engine` for an engine lock record naming `pid`: an undetermined liveness is an input
/// doctor could not read, so a warning (R-T9-liveness), as for `switch.interrupted`. `daemon`
/// means the record matches `daemon.lock`'s, so the engine is the daemon's (D10).
fn engine_record_check(pid: u32, liveness: Liveness, daemon: bool) -> Check {
    let who = if daemon {
        format!("the daemon (pid {pid})")
    } else {
        format!("pid {pid}")
    };
    match liveness {
        Liveness::Live if daemon => {
            Check::info("auto.engine", format!("auto-switch runs in {who}"))
        }
        Liveness::Live => Check::info("auto.engine", format!("auto-switch runs as {who}")),
        Liveness::Dead => Check::ok(
            "auto.engine",
            format!("auto-switch is not running (its last engine, {who}, has exited)"),
        ),
        Liveness::Unknown(e) => Check::warn(
            "auto.engine",
            format!(
                "whether auto-switch runs as {who} cannot be told: the process's start time cannot be read ({e})"
            ),
        )
        .fix(format!(
            "check whether pid {pid} is still running a `tagteam {}`, then `tagteam doctor` again",
            if daemon { "daemon" } else { "auto" }
        )),
    }
}

/// `daemon.running` for `daemon.lock`'s record (D4.1): a live daemon of another version is told
/// to restart (D10).
fn daemon_record_check(r: &DaemonRecord, liveness: Liveness) -> Check {
    const ID: &str = "daemon.running";
    let this = env!("CARGO_PKG_VERSION");
    match liveness {
        Liveness::Live if r.version == this => Check::info(
            ID,
            format!("the daemon runs (pid {}, version {})", r.pid, r.version),
        ),
        Liveness::Live => Check::warn(
            ID,
            format!(
                "the daemon runs (pid {}) on version {}, not this tagteam's {this}",
                r.pid, r.version
            ),
        )
        .fix("restart it: a daemon restarts by itself within a minute of an upgrade; if this one has not, `tagteam daemon install` restarts the service's daemon, and a foreground one restarts once you stop it (Ctrl-C) and start it again"),
        Liveness::Dead => Check::info(
            ID,
            format!(
                "the daemon is not running (its last run, pid {}, has exited)",
                r.pid
            ),
        ),
        Liveness::Unknown(e) => Check::warn(
            ID,
            format!(
                "whether the daemon runs as pid {} cannot be told: the process's start time cannot be read ({e})",
                r.pid
            ),
        )
        .fix(format!(
            "check whether pid {} is still a `tagteam daemon`, then `tagteam doctor` again",
            r.pid
        )),
    }
}

/// `daemon.service`'s finding for an installed unit, from what its manager says (D10). A
/// manager that is missing, hangs or fails is a warning with its reason, never a failure
/// (Review Focus 5).
fn service_state_check(kind: ServiceKind, path: &Path, state: &ServiceState) -> Check {
    const ID: &str = "daemon.service";
    let unit = format!(
        "the daemon's {} is installed at {}",
        kind.noun(),
        path.display()
    );
    if let Some(why) = &state.unknown {
        let fix = match kind {
            ServiceKind::Launchd => {
                format!("`launchctl print gui/$(id -u)/{LAUNCHD_LABEL}` shows its state")
            }
            ServiceKind::Systemd => format!("`systemctl --user status {SYSTEMD_UNIT}` shows its state"),
        };
        return Check::warn(
            ID,
            format!("{unit}, but whether it is loaded cannot be told: {why}"),
        )
        .fix(fix);
    }
    match (state.loaded, state.running, state.last_exit_status) {
        (Some(true), Some(true), _) => Check::info(ID, format!("{unit}, loaded and running")),
        (Some(true), _, Some(code)) => Check::info(
            ID,
            format!("{unit}, loaded but not running (its last exit status was {code})"),
        ),
        (Some(true), _, None) => Check::info(ID, format!("{unit}, loaded but not running")),
        _ => Check::info(
            ID,
            format!("{unit} but not loaded, so it starts at the next login"),
        )
        .fix("`tagteam daemon install` loads and starts it now"),
    }
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}
```

6. **Order.** In `Engine::doctor`, replace:

```rust
        run.log();
        let providers = run.scope(opts.provider.as_ref());
```

with:

```rust
        run.log();
        run.daemon(opts.service);
        let providers = run.scope(opts.provider.as_ref());
```

7. **`auto.engine`.** In `fn auto`, replace:

```rust
            Read::Present(stamp) => engine_record_check(stamp.pid, stamp.liveness()),
```

with:

```rust
            Read::Present(stamp) => {
                engine_record_check(stamp.pid, stamp.liveness(), self.held_by_daemon(&stamp))
            }
```

8. **The checks.** In `impl Run<'_>`, insert before the doc comment
`/// §13.6 Accounts, orphaned vault items: …` (`fn vault_orphans`):

```rust
    /// D10: the daemon, its service unit, its socket and its notifications. They are read-only
    /// like every check (§13.6):
    /// - `daemon.lock`'s record is read without trying the lock.
    /// - The socket is looked at and never connected to.
    /// - The service manager is only asked, within 5 s each: `launchctl print`,
    ///   `systemctl --user show`, `loginctl show-user`.
    ///
    /// `service` is this platform's manager. Without one, the unit and the notifications are
    /// not checked.
    fn daemon(&mut self, service: Option<ServiceKind>) {
        let live = self.daemon_running();
        let (installed, unit) = match service {
            Some(kind) => self.daemon_service(kind),
            None => (false, None),
        };
        self.daemon_socket(live, installed);
        if let Some(kind) = service {
            self.daemon_notify(kind, unit.as_ref());
        }
    }

    /// `daemon.running`: the daemon `daemon.lock`'s record names (D4.1), judged by an exact pid
    /// and start-time match (§12.6). It returns whether the daemon runs. A liveness that cannot
    /// be told counts as running, as `is_live` counts it.
    fn daemon_running(&mut self) -> bool {
        let lock = daemon_lock_path(&self.engine.env);
        let (check, live) = match read_daemon_record(&lock) {
            Read::Absent => (
                Check::info("daemon.running", "the daemon is not running"),
                false,
            ),
            Read::Present(r) => {
                let liveness = ProcessStamp {
                    pid: r.pid,
                    start: r.start,
                }
                .liveness();
                let live = liveness != Liveness::Dead;
                (daemon_record_check(&r, liveness), live)
            }
            Read::Unreadable(e) => (
                Check::warn(
                    "daemon.running",
                    format!("whether the daemon runs cannot be told: {e}"),
                )
                .fix(readable(&lock)),
                false,
            ),
        };
        self.push(None, check);
        live
    }

    /// Whether `holder`, an engine lock's record, names the process `daemon.lock` names, by pid
    /// and start time (D10).
    fn held_by_daemon(&self, holder: &ProcessStamp) -> bool {
        matches!(
            read_daemon_record(&daemon_lock_path(&self.engine.env)),
            Read::Present(r) if r.pid == holder.pid && r.start == holder.start
        )
    }

    /// `daemon.service` (D10) reports:
    /// - whether the unit is installed, and what its manager says of it
    /// - the program it starts, which must exist
    /// - the variables it records against doctor's own (D9.1)
    /// - on Linux, lingering (D9.3)
    ///
    /// The manager is asked only about a unit that is there. It returns whether one is
    /// installed, and the unit as read when tagteam wrote it as it stands (Decision 5).
    fn daemon_service(&mut self, kind: ServiceKind) -> (bool, Option<UnitContents>) {
        const ID: &str = "daemon.service";
        let engine = self.engine;
        let env = &engine.env;
        let path = unit_path(env, kind);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                self.push(
                    None,
                    Check::info(
                        ID,
                        format!(
                            "the daemon's {} is not installed ({})",
                            kind.noun(),
                            path.display()
                        ),
                    ),
                );
                return (false, None);
            }
            Err(e) => {
                self.push(
                    None,
                    Check::warn(
                        ID,
                        format!(
                            "the daemon's {} at {} cannot be read ({})",
                            kind.noun(),
                            path.display(),
                            e.kind()
                        ),
                    )
                    .fix(readable(&path)),
                );
                return (true, None);
            }
        };
        let state = service_state(env, kind, engine.spawner.as_ref(), MANAGER_TIMEOUT);
        self.push(None, service_state_check(kind, &path, &state));
        let unit = parse_unit(kind, &text);
        match &unit {
            None => self.push(
                None,
                Check::info(
                    ID,
                    format!(
                        "{} was not written by tagteam as it stands, so the program it starts and its environment are not checked",
                        path.display()
                    ),
                ),
            ),
            Some(u) => {
                match u.program.try_exists() {
                    Ok(true) => {}
                    Ok(false) => self.push(
                        None,
                        Check::fail(
                            ID,
                            format!(
                                "{} starts {}, which does not exist, so the service cannot start the daemon",
                                path.display(),
                                u.program.display()
                            ),
                        )
                        .fix("`tagteam daemon install` writes the unit again for this tagteam"),
                    ),
                    Err(e) => self.push(
                        None,
                        Check::warn(
                            ID,
                            format!(
                                "whether {}, which {} starts, exists cannot be told ({})",
                                u.program.display(),
                                path.display(),
                                e.kind()
                            ),
                        )
                        .fix(readable(&u.program)),
                    ),
                }
                // PATH is left out: it places no store or home, and a version manager changes
                // it in every directory it enters.
                let mut differ = Vec::new();
                for name in RECORDED_VARS.iter().copied().filter(|n| *n != "PATH") {
                    let recorded = u
                        .env
                        .iter()
                        .find(|(k, _)| k == name)
                        .map(|(_, v)| OsStr::new(v.as_str()));
                    if recorded != env.var(name) {
                        differ.push(name);
                    }
                }
                if !differ.is_empty() {
                    self.push(
                        None,
                        Check::warn(
                            ID,
                            format!(
                                "{} records {} differently from this environment, so the daemon may use another store, home, proxy or log level",
                                path.display(),
                                and_list(&differ)
                            ),
                        )
                        .fix("run `tagteam daemon install` again from the environment whose store and home the daemon should use"),
                    );
                }
            }
        }
        if kind == ServiceKind::Systemd {
            let user = env.user.clone().unwrap_or_else(|| current_uid().to_string());
            match lingering(env, engine.spawner.as_ref(), &user, MANAGER_TIMEOUT) {
                Ok(true) => {}
                Ok(false) => self.push(
                    None,
                    Check::info(
                        ID,
                        format!(
                            "lingering is off for {user}, so systemd stops the daemon when you log out"
                        ),
                    )
                    .fix(format!("`loginctl enable-linger {user}` keeps it running")),
                ),
                // Review Focus 5: a `loginctl` missing, hung or failing is a warning with its
                // reason, never silence and never a failure.
                Err(why) => self.push(
                    None,
                    Check::warn(
                        ID,
                        format!("whether lingering is on for {user} cannot be told: {why}"),
                    )
                    .fix(format!("`loginctl show-user {user} -p Linger` shows it")),
                ),
            }
        }
        (true, unit)
    }

    /// `daemon.socket` (D5.1, D10): the socket file, against the daemon that should own it. A
    /// path too long to bind matters once a daemon runs or a unit is installed: the daemon then
    /// runs, or would run, without its socket, and the check fails. With neither, the length is
    /// not judged.
    fn daemon_socket(&mut self, live: bool, installed: bool) {
        const ID: &str = "daemon.socket";
        let path = daemon_socket_path(&self.engine.env);
        let len = path.as_os_str().len();
        if len > MAX_SOCKET_PATH && (live || installed) {
            self.push(
                None,
                Check::fail(
                    ID,
                    format!(
                        "{} is {len} bytes, too long for a socket here (at most {MAX_SOCKET_PATH}), so the daemon runs without it and nothing can reach it",
                        path.display()
                    ),
                )
                .fix("set XDG_DATA_HOME to a shorter directory, then `tagteam daemon install` again"),
            );
            return;
        }
        let check = match fs::symlink_metadata(&path) {
            Ok(m) => {
                let mode = m.permissions().mode() & 0o777;
                if mode != 0o600 {
                    let fix = if live {
                        format!(
                            "`chmod 600 {}`, or restart the daemon, which creates it 0600",
                            quoted(&path)
                        )
                    } else {
                        format!("rm {}", quoted(&path))
                    };
                    Check::warn(
                        ID,
                        format!(
                            "{} has mode {mode:o}, not 600: another user may reach the daemon through it",
                            path.display()
                        ),
                    )
                    .fix(fix)
                } else if live {
                    Check::ok(ID, format!("the daemon listens on {}", path.display()))
                } else {
                    Check::info(
                        ID,
                        format!(
                            "{} is left from a daemon that is not running; the next one to start replaces it",
                            path.display()
                        ),
                    )
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                if live {
                    Check::warn(
                        ID,
                        format!(
                            "the daemon runs without its socket ({} does not exist), so `tagteam daemon status` and the TUI cannot reach it",
                            path.display()
                        ),
                    )
                    .fix("the log says why it could not bind; restart the daemon once that is fixed")
                } else {
                    Check::ok(ID, "no daemon runs, and none left a socket behind")
                }
            }
            Err(e) => Check::warn(
                ID,
                format!("{} cannot be read ({})", path.display(), e.kind()),
            )
            .fix(readable(&path)),
        };
        self.push(None, check);
    }

    /// `daemon.notify` (D6.2, D10): notifications that are on, with nothing to show them. macOS
    /// always has `/usr/bin/osascript`. Linux needs `notify-send` on the daemon's `PATH`: the
    /// one its unit records, when doctor can read the unit, else doctor's own.
    /// `notify.enabled` is read from `config.toml`, as the `auto` check reads its settings.
    fn daemon_notify(&mut self, kind: ServiceKind, unit: Option<&UnitContents>) {
        const ID: &str = "daemon.notify";
        let engine = self.engine;
        let (settings, _) = Settings::load(&engine.env, engine.default_provider());
        let check = if !settings.notify_enabled {
            Check::info(ID, "notifications are off (notify.enabled is false)")
        } else {
            match kind {
                ServiceKind::Launchd => Check::ok(ID, "notifications are on, through osascript"),
                ServiceKind::Systemd => {
                    let recorded = unit
                        .and_then(|u| u.env.iter().find(|(k, _)| k == "PATH"))
                        .map(|(_, v)| OsStr::new(v.as_str()));
                    match find_on_path("notify-send", recorded.or_else(|| engine.env.var("PATH"))) {
                        Some(bin) => Check::ok(
                            ID,
                            format!("notifications are on, through {}", bin.display()),
                        ),
                        None => Check::warn(
                            ID,
                            "notifications are on, but notify-send is not on the daemon's PATH, so none is shown",
                        )
                        .fix("install notify-send (libnotify), or turn them off with `tagteam config set notify.enabled false`"),
                    }
                }
            }
        };
        self.push(None, check);
    }

```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-engine --lib doctor`. Expected: PASS.
Run, outside the sandbox (`the_daemon_checks_change_no_byte_take_no_lock_and_never_connect`
binds a Unix socket under `/tmp`): `cargo test -p tagteam-engine --test doctor`. Expected:
PASS, including the 10 new tests. Every earlier test passes unchanged, since
`DoctorOptions::default()` asks no service manager.
Run: `cargo test -p tagteam-engine`. Expected: PASS.

#### Part B: the CLI

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam/src/app.rs`'s `mod tests`, change the call in
`consume_first_named_for_a_provider_without_a_long_window_is_a_usage_error`:

```rust
        let Failure::Usage(message) = auto_failure(AutoError::NoLongWindow(fake.id()), &name)
```

to:

```rust
        let Failure::Usage(message) =
            auto_failure(AutoError::NoLongWindow(fake.id()), &name, &|_| None)
```

and add after that test:

```rust
    #[test]
    fn auto_names_the_daemon_when_it_holds_the_engine_lock() {
        // D8.3: the holder's record matches daemon.lock's, so the refusal names the daemon and
        // how to pause it.
        let cc = ProviderId::new(CLAUDE_CODE);
        let name = |_: &ProviderId| "Claude Code".to_owned();
        let refusal = |daemon: Option<u32>| {
            match auto_failure(AutoError::AlreadyRuns(vec![cc.clone()]), &name, &|_| daemon) {
                Failure::Message(kind, message) => {
                    assert_eq!(kind, KIND_ENGINE_RUNNING);
                    message
                }
                _ => panic!("not engine-running"),
            }
        };
        assert_eq!(
            refusal(Some(4242)),
            "auto-switch already runs for Claude Code (the daemon, pid 4242; pause it with tagteam config set daemon.autoswitch false)"
        );
        assert_eq!(
            refusal(None),
            "auto-switch already runs for Claude Code"
        );
    }
```

In `crates/tagteam/tests/doctor_cli.rs`:
- Add `use tagteam_engine::service::{ServiceKind, render_unit};` to the imports.
- In `the_json_shape_of_a_fixture_with_two_accounts`, replace:

```rust
            own("log.file", "info"),
            own("log.writable", "ok"),
            own("store.temp-files", "ok"),
```

with:

```rust
            own("log.file", "info"),
            own("log.writable", "ok"),
            own("daemon.running", "info"),
            own("daemon.service", "info"),
            own("daemon.socket", "ok"),
            own("daemon.notify", "ok"),
            own("store.temp-files", "ok"),
```

- In `doctor_over_a_full_fixture_home_leaves_every_byte_as_it_was`, before
  `let before = tree(d.path());`, add:

```rust
    // D10: the daemon's lock record, a socket file it left, and its launchd agent, whose
    // program is gone.
    fs::write(
        data.join("locks/daemon.lock"),
        "{\"pid\":1,\"start\":1,\"version\":\"0\"}\n",
    )
    .unwrap();
    fs::write(data.join("daemon.sock"), "").unwrap();
    let agent = env
        .home
        .join("Library/LaunchAgents/io.github.michaelrigart.tagteam.plist");
    fs::create_dir_all(agent.parent().unwrap()).unwrap();
    fs::write(
        &agent,
        render_unit(ServiceKind::Launchd, Path::new("/opt/tagteam"), &[]).unwrap(),
    )
    .unwrap();
```

- Append (`short_tempdir` is the one Task 2 added to `tests/common`, which this file imports
  with `use common::*;`):

```rust
#[test]
fn the_daemon_s_checks_on_each_platform() {
    // D10 through the binary, on a machine with no daemon: the four checks, as JSON, with the
    // platform the test-support override names.
    for (platform, noun, unit, notify) in [
        (
            "macos",
            "launchd agent",
            "home/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist",
            json!({"id": "daemon.notify", "status": "ok",
                   "message": "notifications are on, through osascript", "fix": null}),
        ),
        (
            "linux",
            "systemd user unit",
            "home/.config/systemd/user/tagteam.service",
            json!({"id": "daemon.notify", "status": "warn",
                   "message": "notifications are on, but notify-send is not on the daemon's PATH, so none is shown",
                   "fix": "install notify-send (libnotify), or turn them off with `tagteam config set notify.enabled false`"}),
        ),
    ] {
        let d = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            std_cmd(d.path())
                .env("TAGTEAM_TEST_PLATFORM", platform)
                .env("PATH", d.path().join("no-bin"))
                .args(args)
                .stdin(Stdio::null())
                .output()
                .unwrap()
        };
        let v: Value = serde_json::from_slice(&run(&["doctor", "--json"]).stdout).unwrap();
        let daemon: Vec<Value> = v["checks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["id"].as_str().unwrap().starts_with("daemon."))
            .map(|c| json!({"id": c["id"], "status": c["status"], "message": c["message"], "fix": c["fix"]}))
            .collect();
        assert_eq!(
            daemon,
            vec![
                json!({"id": "daemon.running", "status": "info",
                       "message": "the daemon is not running", "fix": null}),
                json!({"id": "daemon.service", "status": "info",
                       "message": format!("the daemon's {noun} is not installed ({})", d.path().join(unit).display()),
                       "fix": null}),
                json!({"id": "daemon.socket", "status": "ok",
                       "message": "no daemon runs, and none left a socket behind", "fix": null}),
                notify,
            ],
            "{platform}"
        );
        let human = String::from_utf8_lossy(&run(&["doctor"]).stdout).into_owned();
        assert!(human.contains("· the daemon is not running\n"), "{human}");
    }

    // A unit recording another data directory: a warning naming the variable, beside the
    // one for a launchctl that is not on PATH. Doctor still exits 0. Run with the data
    // directory the unit records, nothing differs. The data directory is short, so the
    // socket's length, judged once a unit is installed, passes.
    let d = tempfile::tempdir().unwrap();
    let short = short_tempdir();
    let data = short.path().join("d");
    let agent = d
        .path()
        .join("home/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist");
    fs::create_dir_all(agent.parent().unwrap()).unwrap();
    let program = assert_cmd::cargo::cargo_bin("tagteam");
    let plant = |recorded: &str| {
        let env = [("XDG_DATA_HOME".to_owned(), recorded.to_owned())];
        fs::write(
            &agent,
            render_unit(ServiceKind::Launchd, &program, &env).unwrap(),
        )
        .unwrap();
    };
    let service = || {
        let out = std_cmd(d.path())
            .env("PATH", d.path().join("no-bin"))
            .env("XDG_DATA_HOME", &data)
            .args(["doctor", "--json"])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        let found: Vec<(String, String)> = with_id(&v, "daemon.service")
            .iter()
            .map(|c| {
                (
                    c["status"].as_str().unwrap().to_owned(),
                    c["message"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        (found, out.status.code())
    };
    let unknown = (
        "warn".to_owned(),
        format!(
            "the daemon's launchd agent is installed at {}, but whether it is loaded cannot be told: launchctl is not on PATH",
            agent.display()
        ),
    );
    plant("/elsewhere");
    assert_eq!(
        service(),
        (
            vec![
                unknown.clone(),
                (
                    "warn".to_owned(),
                    format!(
                        "{} records XDG_DATA_HOME differently from this environment, so the daemon may use another store, home, proxy or log level",
                        agent.display()
                    )
                ),
            ],
            Some(0)
        )
    );
    plant(data.to_str().unwrap());
    assert_eq!(service(), (vec![unknown], Some(0)), "the same data directory");
}
```

In `crates/tagteam/tests/auto_cli.rs`, replace `use tagteam_provider::Env;` with
`use tagteam_provider::{Env, FlockGuard, ProcessStamp};`, and append:

```rust
#[test]
fn auto_names_the_daemon_when_the_daemon_holds_the_engine_lock() {
    // D8.3: the engine lock is held, and its record matches daemon.lock's. This test process
    // stands in for the daemon: it holds the lock and both records name it.
    let d = tempfile::tempdir().unwrap();
    staying(d.path());
    let locks = Env::for_test(d.path()).data_dir().join("locks");
    fs::create_dir_all(&locks).unwrap();
    let me = ProcessStamp::current().unwrap();
    let engine_lock = locks.join("autoswitch-claude-code.lock");
    let _held = FlockGuard::try_lock(&engine_lock).unwrap().unwrap();
    fs::write(
        &engine_lock,
        format!("{{\"pid\":{},\"start\":{}}}\n", me.pid, me.start),
    )
    .unwrap();
    let daemon_record = |start: u64| {
        format!(
            "{{\"pid\":{},\"start\":{start},\"version\":\"{}\"}}\n",
            me.pid,
            env!("CARGO_PKG_VERSION")
        )
    };
    fs::write(locks.join("daemon.lock"), daemon_record(me.start)).unwrap();
    let message = format!(
        "auto-switch already runs for Claude Code (the daemon, pid {}; pause it with tagteam config set daemon.autoswitch false)",
        me.pid
    );
    let out = auto(d.path(), &[]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(text(&out.stderr), format!("tagteam: {message}\n"));
    let out = auto(d.path(), &["--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "engine-running", "message": message}})
    );
    // daemon.lock naming another start time: the holder is not the daemon.
    fs::write(locks.join("daemon.lock"), daemon_record(me.start + 1)).unwrap();
    assert_eq!(
        text(&auto(d.path(), &[]).stderr),
        "tagteam: auto-switch already runs for Claude Code\n"
    );
}
```

`staying`, `auto` and `text` are the file's own helpers.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --lib app::tests`
Expected: the build fails with `error[E0061]: this function takes 2 arguments but 3 arguments
were supplied` (`auto_failure`).

Run: `cargo test -p tagteam --features test-support --test doctor_cli --test auto_cli`
Expected: FAIL. `the_json_shape_of_a_fixture_with_two_accounts` and
`the_daemon_s_checks_on_each_platform` find `daemon.running` and `daemon.socket` but not
`daemon.service` or `daemon.notify`, which the CLI does not ask for yet.
`auto_names_the_daemon_when_the_daemon_holds_the_engine_lock` finds
`tagteam: auto-switch already runs for Claude Code` without the daemon.

- [ ] **Step 3: Implement**

In `crates/tagteam/src/app.rs`:

1. **Imports.** Add:

```rust
use tagteam_engine::daemon::{daemon_lock_path, read_daemon_record};
use tagteam_engine::service::{RECORDED_VARS, ServiceKind};
```

and add `Read` to `use tagteam_provider::{Clock, Env, EnvError, Keychain, LockState, SystemClock};`,
giving `use tagteam_provider::{Clock, Env, EnvError, Keychain, LockState, Read, SystemClock};`.

2. **The process boundary.** In `Context::from_process`, replace:

```rust
        for name in scrubbed_vars(&registry) {
            if std::env::var_os(&name).is_some() {
                ctx.env.vars.insert(name, OsString::new());
            }
        }
        Ok(ctx)
```

with:

```rust
        for name in scrubbed_vars(&registry) {
            if std::env::var_os(&name).is_some() {
                ctx.env.vars.insert(name, OsString::new());
            }
        }
        // D9.1, D10: the variables a service unit records, as this process has them, for
        // `daemon install` and for doctor's comparison with an installed unit. They come after
        // the presence markers above. `CLAUDE_SECURESTORAGE_CONFIG_DIR` is among both: its
        // value, a directory the Env already holds raw, replaces its marker and keeps it present.
        ctx.env.capture_vars(RECORDED_VARS);
        Ok(ctx)
```

3. **`App::service`.** Plan A's Task 12 keeps the platform on `App` (`platform: Platform`). In
   `impl App`, before `fn auto`, add:

```rust
    /// This platform's service manager (D9): launchd on macOS, systemd on Linux, by
    /// `Context::platform`, which a test-support build can override.
    fn service(&self) -> ServiceKind {
        ServiceKind::for_platform(self.platform == Platform::MacOs)
    }
```

4. **`doctor`.** In `App::dispatch`'s `Command::Doctor` arm, replace:

```rust
                let report = self.engine.doctor(DoctorOptions {
                    online,
                    provider: self.provider_flag.clone(),
                })?;
```

with:

```rust
                let report = self.engine.doctor(DoctorOptions {
                    online,
                    provider: self.provider_flag.clone(),
                    service: Some(self.service()),
                })?;
```

5. **D8.3.** Replace `auto_failure` with:

```rust
/// How `auto` fails as a command, each provider named by `name`. A provider whose engine is the
/// daemon's (`daemon` gives its pid) is named with the daemon and how to pause it (D8.3).
/// Consume-first named for a provider without a long window is a usage error, exit 2, as every
/// bad flag value is (§4.5).
fn auto_failure(
    e: AutoError,
    name: &dyn Fn(&ProviderId) -> String,
    daemon: &dyn Fn(&ProviderId) -> Option<u32>,
) -> Failure {
    match e {
        AutoError::AlreadyRuns(providers) => {
            let names: Vec<String> = providers
                .iter()
                .map(|p| match daemon(p) {
                    Some(pid) => format!(
                        "{} (the daemon, pid {pid}; pause it with tagteam config set daemon.autoswitch false)",
                        name(p)
                    ),
                    None => name(p),
                })
                .collect();
            Failure::Message(
                KIND_ENGINE_RUNNING,
                format!("auto-switch already runs for {}", names.join(", ")),
            )
        }
        AutoError::NothingToSwitch => {
            Failure::Message(KIND_NO_CANDIDATES, NOTHING_TO_SWITCH.into())
        }
        AutoError::NoLongWindow(p) => Failure::Usage(format!(
            "--strategy consume-first needs a long usage window to rank by, and {} has none",
            name(&p)
        )),
        AutoError::Engine(e) => e.into(),
    }
}

/// D8.3: the daemon's pid when `provider`'s engine lock record names the process `daemon.lock`
/// names, by pid and start time; `None` for any other holder, or a record that cannot be read.
fn daemon_engine(engine: &Engine, provider: &ProviderId) -> Option<u32> {
    let Read::Present(holder) = engine.engine_holder(provider) else {
        return None;
    };
    match read_daemon_record(&daemon_lock_path(engine.env())) {
        Read::Present(d) if d.pid == holder.pid && d.start == holder.start => Some(d.pid),
        _ => None,
    }
}
```

Plan A's `AutoError` (Task 7, in `tagteam_daemon::auto`) keeps today's four variants, so the
other three arms are as they were. Only `AlreadyRuns` and the signature change.

In `App::auto`, beside `let names = |p: &ProviderId| display_name(engine, p);`, add:

```rust
        let daemon = |p: &ProviderId| daemon_engine(engine, p);
```

and change its call `auto_failure(e, &names)` to `auto_failure(e, &names, &daemon)`.

- [ ] **Step 4: Run the tests and see them pass**

Run:
- `cargo test -p tagteam --lib app::tests`. Expected: PASS.
- `cargo test -p tagteam --features test-support --test doctor_cli --test auto_cli`. Expected:
  PASS.
- `cargo test -p tagteam --features test-support`, then `cargo test --workspace`. Expected:
  PASS.

Then `cargo fmt --all && cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`.
Expected: clean.

- [ ] **Step 5: Commit** (the controller commits; implementers do not)

```
git add crates/tagteam-engine/src/doctor.rs crates/tagteam-engine/tests/doctor.rs \
  crates/tagteam/src/app.rs crates/tagteam/tests/doctor_cli.rs crates/tagteam/tests/auto_cli.rs
git commit -m "Report the daemon in doctor, and name it when auto finds it running"
```

---

### Task 7: Multi-process and performance tests

**Depends on:** 2, 3, 5, 6 (and all of plan A)

D14's "Concurrency" and "Exits" items, run as real processes against one fixture home (§15.2):
a daemon beside `tagteam auto`, two daemons, a CLI switch reaching a socket subscriber as
`changed`, the hourly budget across processes, exit 75 on a replaced program (also one
replaced while a second daemon waits for `daemon.lock`), and exit 0 on a signal. Decision 7's
two test hooks make the exit-75 tests take seconds. `tests/perf.rs` gains
D12's runs: `statusline` and `list` p95 while a daemon ticks against the same fixture.

**Readings this task commits to:**
- **Every daemon here runs with stdin closed and `notify.enabled` false.** It is never on a
  terminal, so it runs no Keychain lock check (D4.1 step 3, as under a service). It also never
  starts `osascript` or `notify-send` on the developer's machine.
- **Roots are short** (Task 2's `short_tempdir`, under `/tmp`).
  `<root>/home/.local/share/tagteam/daemon.sock` must fit `sun_path` (D5.1), and macOS's
  `$TMPDIR` can push it past 104 bytes. Binding and connecting Unix sockets, and writing under
  `/tmp`, are denied inside an agent's sandbox, so `daemon_procs` and the new perf runs run
  outside it.
- **These tests characterize earlier tasks.** Before this task's code, only the two exit-75
  tests and the hook unit tests fail. If any other test here fails, the defect belongs to the task named
  in that test's comment, and is fixed there, not worked around here.
- **Every wait is bounded:** 20 s to reach a state (`START`), 10 s to stop (`STOP`). Every child
  is a `Guarded`, which kills and reaps it when the test fails part-way.
- **No test assumes which event comes first.** Plan A adds the settings, store and upgrade
  watches before the provider jobs, so on a daemon's first pass a `changed`, a
  `config-warning` or even `stopping` can come before the first `mode`. Each test waits for the
  event it needs among all printed so far (`has_event`, `mode_event`, `next_event`), and only
  `stopping`'s place is pinned: it is the last.
- **The budget test makes the budget the only thing that can stop a send.** `a@x.co`'s
  identity has spent all 20 slots of its hour, `b@x.co`'s has spent 19, and neither account has
  a reading, a backoff, or a live lease (the slots were taken 100 s ago, and a lease lasts
  90 s). So every process finds both accounts due, and only §8.6 decides. The server's shared
  log is counted per identity by bearer token, so each account logs in with its own access token
  (`login_with_token`).
- **D8.3's message is matched in parts**: `the daemon, pid <N>` and
  `tagteam config set daemon.autoswitch false`. Task 6 spells the provider by its display name
  (Decision 16), and its own tests pin the whole message.
- **The waiting notice** (D4.1) is looked for in a human-mode daemon, on stdout or stderr.
  Under `--json`, it may go to either stream, or nowhere.
- **Perf** (Decision 18). §6.4's shortest `autoswitch.interval_seconds` is 15, so the fixture
  daemon ticks about every 15 s, not "every second" as D12 first said (Task 8's erratum b). Its
  scheduler wakes every second, and its store watch reads the store every 2 s. The timed runs
  are spread over about 19 s (350 ms apart, untimed), longer than the longest tick delay
  (16.5 s), so at least one tick lands among them; the test asserts that it did.
- **`daemon.lock`'s record is empty after every stop.** Plan A's Task 12 empties it before the
  lock goes (its private `DaemonLock`), so `assert_stopped_cleanly` checks it after exit 0 and
  after exit 75.
- **The hooks are applied where plan A produces the values.** Plan A's `App::daemon` (in
  `app.rs`) stamps the program first (`ProgramStamp::take`, its Decision 17), then builds the
  `DaemonOptions` value. This task wraps the path in `daemon_cmd::test_program` inside that
  stamp, so the upgrade watch's baseline is the test's file, and `UPGRADE_WATCH_MS` in
  `daemon_cmd::test_upgrade_watch_ms`.
- **Where the stamp is taken is pinned too.** A second daemon stamps its program before it
  waits for `daemon.lock`. A program replaced during that wait is therefore an upgrade at the
  second daemon's first upgrade-watch run, once it serves: exit 75
  (`a_program_replaced_while_a_second_daemon_waits_is_an_upgrade_at_its_first_watch_run`).
  A stamp taken after the wait would make the new file the baseline, and the daemon would run
  on.

**Files:**
- Modify: `crates/tagteam/src/daemon_cmd.rs`: Decision 7's hooks (`test_program`,
  `test_upgrade_watch_ms` and their two constants), and a test module, `test_hook_tests`, at
  the end of the file
- Modify: `crates/tagteam/src/app.rs`: in `App::daemon`, the program path and the upgrade
  watch's period go through the hooks
- Modify: `crates/tagteam/tests/common/mod.rs`: `login` delegates to a new `login_with_token`;
  a new `Guarded`
- Create: `crates/tagteam/tests/daemon_procs.rs`
- Modify: `crates/tagteam/tests/perf.rs`: two tests and their helpers
- Test: `daemon_cmd.rs`, `daemon_procs.rs` and `perf.rs`

**Interfaces:**
- Consumes:
  - plan A Task 12's `App::daemon`, whose first statement is
    `ProgramStamp::take(daemon_cmd::program(…))`, and which builds `DaemonOptions { provider,
    version, program: ProgramStamp, upgrade_watch_ms, keychain_counts, backend }`
  - `Running::stderr` (plan A Task 12) and Task 2's `short_tempdir`, in `tests/common`
  - `tagteam_engine::daemon::{daemon_lock_path, daemon_socket_path, read_daemon_record}` and
    `DaemonRecord { pid, start, version }` (plan A Task 3)
  - `tagteam_daemon::client::{connect_at, Connect, Client}`, `Client::status`,
    `Client::subscribe`, `Events: Iterator<Item = Result<Event, ClientError>>` (Task 3)
  - `tagteam_daemon::protocol::{Event, Status}` (Task 1)
  - `Store::{reserve_usage, authorize_send, usage_request_count}`, `PollBudget::STANDARD`
- Produces:
  - in `crate::daemon_cmd`, honouring `TAGTEAM_TEST_DAEMON_PROGRAM` and
    `TAGTEAM_TEST_UPGRADE_WATCH_MS` under `test-support` only:
    `pub(crate) fn test_program(program: PathBuf, var: &dyn Fn(&str) -> Option<OsString>) -> PathBuf`
    and `pub(crate) fn test_upgrade_watch_ms(ms: i64, var: &dyn Fn(&str) -> Option<OsString>) -> i64`
  - In `tests/common`: `login_with_token(env, kc, email, org, at, rt)` and `Guarded` (`spawn`,
    `pid`, `stdout`, `stderr`, `signal`, `is_running`, `wait_for`, `wait_for_output`,
    `finish`). Task 9 uses `Guarded` and the `daemon_procs.rs` helpers.

- [ ] **Step 1: Write the failing tests**

(a) Decision 7's hooks. Append to `crates/tagteam/src/daemon_cmd.rs`. The test module has a name
of its own, beside plan A's `mod tests` and Task 2's `mod socket_tests`:

```rust
#[cfg(test)]
mod test_hook_tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::*;

    /// The path `tagteam daemon` would follow, before the hooks.
    const INSTALLED: &str = "/opt/homebrew/bin/tagteam";

    fn all_set(k: &str) -> Option<OsString> {
        match k {
            TEST_UPGRADE_WATCH_MS => Some("200".into()),
            TEST_DAEMON_PROGRAM => Some("/tmp/tt/bin/tagteam".into()),
            _ => None,
        }
    }

    /// Runs under both builds: with `test-support` the hooks are honoured; without it (a
    /// release build) they are ignored, whatever the environment holds (Decision 7).
    #[test]
    fn the_daemon_test_hooks_exist_only_with_test_support() {
        let honoured = cfg!(feature = "test-support");
        assert_eq!(
            test_upgrade_watch_ms(60_000, &all_set),
            if honoured { 200 } else { 60_000 }
        );
        assert_eq!(
            test_program(PathBuf::from(INSTALLED), &all_set),
            PathBuf::from(if honoured {
                "/tmp/tt/bin/tagteam"
            } else {
                INSTALLED
            })
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn a_hook_value_that_is_not_usable_changes_nothing() {
        let unusable = |k: &str| match k {
            TEST_UPGRADE_WATCH_MS => Some(OsString::from("soon")),
            TEST_DAEMON_PROGRAM => Some(OsString::new()),
            _ => None,
        };
        assert_eq!(test_upgrade_watch_ms(60_000, &unusable), 60_000);
        assert_eq!(
            test_program(PathBuf::from(INSTALLED), &unusable),
            PathBuf::from(INSTALLED)
        );
        for zero_or_less in ["0", "-5"] {
            let var = |k: &str| (k == TEST_UPGRADE_WATCH_MS).then(|| OsString::from(zero_or_less));
            assert_eq!(test_upgrade_watch_ms(60_000, &var), 60_000, "{zero_or_less}");
        }
    }
}
```

The hooks take and give back plain values (a path, a period), so they do not depend on how plan
A's `DaemonOptions` carries the program.

(b) Shared helpers. In `crates/tagteam/tests/common/mod.rs`:

Replace `login`'s body so that it delegates, and add `login_with_token` below it:

```rust
/// What `claude /login` leaves behind, for a login in organization `org` ("" is personal): its
/// `oauthAccount`, and its credential in the Keychain item Claude Code reads. Logging in again
/// with the same identity and a new `rt` is Claude Code rotating the credential.
pub fn login(env: &Env, kc: &dyn Keychain, email: &str, org: &str, rt: &str) {
    login_with_token(env, kc, email, org, "at", rt);
}

/// As `login`, with access token `at`. Tests that tell accounts' requests apart by their bearer
/// token give each account its own.
pub fn login_with_token(env: &Env, kc: &dyn Keychain, email: &str, org: &str, at: &str, rt: &str) {
    let path = env.home.join(".claude.json");
    let doc = fs::read(&path).unwrap();
    let acct = json!({"emailAddress": email, "organizationUuid": org, "accountUuid": format!("uuid-{email}-{org}")});
    fs::write(
        &path,
        replace_top_level(&doc, "oauthAccount", &acct).unwrap(),
    )
    .unwrap();
    let cred = json!({"claudeAiOauth": {"accessToken": at, "refreshToken": rt, "refreshTokenExpiresAt": 1_797_000_000_000i64}});
    kc.upsert(
        &keychain_service(env, ItemKind::OAuth),
        &keychain_account(env),
        cred.to_string().as_bytes(),
    )
    .unwrap();
}
```

`Running::stderr` and `Running::wait_for_stderr` are plan A's (Task 12), and `short_tempdir`
is Task 2's. After `impl Running` (before `fn drain`), add:

```rust
/// A `Running` that is killed and reaped when it is dropped, so a test that fails part-way
/// never leaves a daemon or an `auto` loop running behind it.
pub struct Guarded(Option<Running>);

impl Guarded {
    pub fn spawn(cmd: std::process::Command) -> Self {
        Guarded(Some(Running::spawn(cmd)))
    }

    fn running(&self) -> &Running {
        self.0.as_ref().expect("still running")
    }

    fn running_mut(&mut self) -> &mut Running {
        self.0.as_mut().expect("still running")
    }

    pub fn pid(&self) -> u32 {
        self.running().child.id()
    }

    pub fn stdout(&self) -> String {
        self.running().stdout()
    }

    pub fn stderr(&self) -> String {
        self.running().stderr()
    }

    pub fn signal(&self, signal: i32) {
        self.running().signal(signal)
    }

    pub fn is_running(&mut self) -> bool {
        self.running_mut().child.try_wait().unwrap().is_none()
    }

    /// Polls every 10 ms until `ready` holds for its stdout so far. Fails the test, with what
    /// it printed, if it exits first or `within` passes.
    pub fn wait_for(&mut self, within: Duration, what: &str, ready: impl Fn(&str) -> bool) {
        self.wait_for_output(within, what, |out, _| ready(out));
    }

    /// As `wait_for`, with its stdout and stderr so far.
    pub fn wait_for_output(
        &mut self,
        within: Duration,
        what: &str,
        ready: impl Fn(&str, &str) -> bool,
    ) {
        let deadline = Instant::now() + within;
        loop {
            let r = self.running_mut();
            let (out, err) = (r.stdout(), r.stderr());
            if ready(&out, &err) {
                return;
            }
            if let Some(status) = r.child.try_wait().unwrap() {
                panic!("tagteam exited ({status}) before {what}\nstdout:\n{out}\nstderr:\n{err}");
            }
            if Instant::now() >= deadline {
                panic!("tagteam never got to {what} within {within:?}\nstdout:\n{out}\nstderr:\n{err}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Its status and everything it printed, once it has exited (`Running::finish`).
    pub fn finish(mut self, within: Duration) -> Output {
        self.0.take().expect("still running").finish(within)
    }
}

impl Drop for Guarded {
    fn drop(&mut self) {
        if let Some(mut r) = self.0.take() {
            let _ = r.child.kill();
            let _ = r.child.wait();
        }
    }
}
```

(c) Create `crates/tagteam/tests/daemon_procs.rs`:

```rust
//! The daemon in real processes (D14 "Concurrency" and "Exits", §15.2): a daemon beside
//! `tagteam auto`, two daemons, a CLI switch reaching a socket subscriber, the hourly usage
//! budget across processes, and the exits on an upgrade and on a signal. Every child is drained
//! while it runs and killed if the test fails (`Guarded`). Roots are short, so the socket fits
//! `sun_path` (D5.1). Needs `--features test-support`, and an environment that may bind and
//! connect Unix sockets and write under `/tmp`: outside an agent's sandbox.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use common::{
    Guarded, cmd, login_with_token, now_epoch_s, record_reading, seed_home, short_tempdir,
    std_cmd, two_fresh_accounts, usage_window,
};
use serde_json::{Value, json};
use tagteam_cc::usage::format_iso8601;
use tagteam_core::{AccountId, CLAUDE_CODE, PollBudget, ProviderId, Window, WindowKind};
use tagteam_daemon::client::{Client, Connect, connect_at};
use tagteam_daemon::protocol::{Event, Status};
use tagteam_engine::daemon::{daemon_lock_path, daemon_socket_path, read_daemon_record};
use tagteam_engine::store::{Eligibility, Reserve, SendGrant, Store};
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{Env, FileKeychain, FlockGuard, Read};

/// How long a binary may take to reach a state: a debug build, with tests in parallel.
const START: Duration = Duration::from_secs(20);
/// How long a stop may take: the scheduler's next slice, then the socket and the locks.
const STOP: Duration = Duration::from_secs(10);
const USAGE: &str = "/api/oauth/usage";

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn cc() -> ProviderId {
    ProviderId::new(CLAUDE_CODE)
}

fn store(root: &Path) -> Store {
    Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap()
}

fn config_set(root: &Path, key: &str, value: &str) {
    cmd(root).args(["config", "set", key, value]).assert().success();
}

/// A reading taken at `now`: 5h at `five` and 7d at `seven` (as auto_cli.rs's).
fn reading(now: i64, five: f64, seven: f64) -> Vec<Window> {
    vec![
        usage_window("5h", "5h", WindowKind::Short, five, Some(now + 9_630), Some(18_000)),
        usage_window("7d", "7d", WindowKind::Long, seven, Some(now + 291_630), Some(604_800)),
    ]
}

/// `a@x.co` at position 1 and `b@x.co` at position 2 (live), both read just now and below the
/// threshold, so a tick stays and its scheduled collection sends nothing. Notifications are
/// off, so no daemon here starts `osascript` or `notify-send`.
fn staying(root: &Path) -> (String, String) {
    let (a, b) = two_fresh_accounts(root);
    let now = now_epoch_s();
    record_reading(root, &a, now, &reading(now, 10.0, 20.0));
    record_reading(root, &b, now, &reading(now, 10.0, 60.0));
    config_set(root, "notify.enabled", "false");
    (a, b)
}

/// `tagteam daemon <args>` with stdin closed: never a terminal, as under a service, so it runs
/// no Keychain lock check (D4.1 step 3).
fn daemon_cmd(root: &Path, args: &[&str]) -> std::process::Command {
    let mut c = std_cmd(root);
    c.arg("daemon").args(args).stdin(Stdio::null());
    c
}

/// `tagteam <args>`, run to its end.
fn run(root: &Path, args: &[&str]) -> Output {
    let mut c = std_cmd(root);
    c.args(args).stdin(Stdio::null());
    Guarded::spawn(c).finish(START)
}

/// Every complete line of `out` that is a JSON object. A line still being written, and any
/// line that is not JSON, is left out.
fn json_lines(out: &str) -> Vec<Value> {
    let complete = out.rfind('\n').map_or("", |i| &out[..=i]);
    complete
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(Value::is_object)
        .collect()
}

fn has_event(out: &str, want: impl Fn(&Value) -> bool) -> bool {
    json_lines(out).iter().any(want)
}

/// Claude Code's provider job reported mode `mode` (D5.5's `mode` event).
fn mode_event(mode: &'static str) -> impl Fn(&str) -> bool {
    move |out: &str| {
        has_event(out, |e| {
            e["event"] == "mode" && e["provider"] == CLAUDE_CODE && e["mode"] == mode
        })
    }
}

fn engine_lock(root: &Path) -> PathBuf {
    Env::for_test(root)
        .data_dir()
        .join("locks/autoswitch-claude-code.lock")
}

/// The pid the engine lock's record names (§11.1). None for no file, an empty one, or one
/// being rewritten at this instant.
fn engine_holder(root: &Path) -> Option<u32> {
    let bytes = fs::read(engine_lock(root)).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    u32::try_from(v["pid"].as_u64()?).ok()
}

/// The pid `daemon.lock`'s record names (D4.1). None for no record, or one being rewritten.
fn daemon_holder(root: &Path) -> Option<u32> {
    match read_daemon_record(&daemon_lock_path(&Env::for_test(root))) {
        Read::Present(r) => Some(r.pid),
        Read::Absent | Read::Unreadable(_) => None,
    }
}

fn socket(root: &Path) -> PathBuf {
    daemon_socket_path(&Env::for_test(root))
}

/// Polls every 20 ms until `ready` holds. Fails the test after `within`.
fn wait_until(within: Duration, what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + within;
    while !ready() {
        assert!(Instant::now() < deadline, "never got to {what} within {within:?}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// D4.6 after the daemon has exited: no socket file; the engine lock's record emptied; and
/// `daemon.lock` with no record. Both locks are free.
fn assert_stopped_cleanly(root: &Path) {
    assert!(!socket(root).exists(), "the socket file was left behind");
    let record = fs::read(engine_lock(root)).unwrap_or_default();
    assert!(record.is_empty(), "the engine lock's record: {}", text(&record));
    let daemon_lock = daemon_lock_path(&Env::for_test(root));
    assert!(
        matches!(read_daemon_record(&daemon_lock), Read::Absent),
        "daemon.lock's record: {:?}",
        read_daemon_record(&daemon_lock)
    );
    for lock in [engine_lock(root), daemon_lock] {
        assert!(
            FlockGuard::try_lock(&lock).unwrap().is_some(),
            "{} is still held",
            lock.display()
        );
    }
}

/// A connection to the daemon at `sock`. `NoDaemon` while it is still starting is retried
/// (Review Focus 1).
fn connected(sock: &Path) -> Client {
    let deadline = Instant::now() + START;
    loop {
        match connect_at(sock).unwrap() {
            Connect::Connected(c) => return c,
            Connect::NoDaemon => {
                assert!(Instant::now() < deadline, "no daemon answered at {}", sock.display());
                thread::sleep(Duration::from_millis(50));
            }
            Connect::Mismatch { server, client } => {
                panic!("the daemon speaks protocol {server}, this client {client}")
            }
        }
    }
}

fn status(sock: &Path) -> Status {
    connected(sock).status().unwrap()
}

/// Subscribes at `sock` and forwards each event on a thread of its own, so a test can wait for
/// one with a deadline. The thread ends when the daemon closes the connection.
fn subscribe(sock: &Path) -> Receiver<Event> {
    let events = connected(sock).subscribe().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for e in events {
            let Ok(e) = e else { break };
            if tx.send(e).is_err() {
                break;
            }
        }
    });
    rx
}

/// The first event from `rx` that `want` accepts, within `within`.
fn next_event(
    rx: &Receiver<Event>,
    within: Duration,
    what: &str,
    want: impl Fn(&Event) -> bool,
) -> Event {
    let deadline = Instant::now() + within;
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(e) if want(&e) => return e,
            Ok(_) => {}
            Err(e) => panic!("no {what} within {within:?}: {e}"),
        }
    }
}

#[test]
fn while_the_daemon_drives_a_provider_auto_reports_engine_running_and_names_it() {
    // D14, D8.3 (Task 6), §11.1: one engine per provider per machine. The daemon holds the
    // engine lock, so `auto --once` reports `engine-running` and a loop refuses, naming the
    // daemon; neither ticks. Pausing then hands the provider over (D4.3, D7; plan A Task 8).
    let d = short_tempdir();
    let root = d.path();
    staying(root);
    let mut daemon = Guarded::spawn(daemon_cmd(root, &["--json"]));
    let pid = daemon.pid();
    daemon.wait_for(START, "auto mode", mode_event("auto"));
    wait_until(START, "the engine lock naming the daemon", || {
        engine_holder(root) == Some(pid)
    });

    let out = run(root, &["auto", "--once", "--json"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    let events = json_lines(&text(&out.stdout));
    assert_eq!(events.len(), 1, "no tick, so no poll: {events:?}");
    assert_eq!(
        (&events[0]["event"], &events[0]["reason"]),
        (&json!("no-switch"), &json!("engine-running"))
    );

    let out = run(root, &["auto"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(out.stdout, b"");
    let err = text(&out.stderr);
    assert!(
        err.contains(&format!("the daemon, pid {pid}"))
            && err.contains("tagteam config set daemon.autoswitch false"),
        "{err}"
    );
    let out = run(root, &["auto", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["error"]["type"], "engine-running", "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains(&format!("the daemon, pid {pid}")),
        "{v}"
    );
    assert_eq!(engine_holder(root), Some(pid), "the daemon kept the lock throughout");

    // Paused, the daemon only collects. It lets the lock go, empties its record, and a tick in
    // another process now runs.
    config_set(root, "daemon.autoswitch", "false");
    daemon.wait_for(STOP, "collect mode", mode_event("collect"));
    wait_until(STOP, "the engine lock's record emptied", || {
        fs::read(engine_lock(root)).unwrap_or_default().is_empty()
    });
    let out = run(root, &["auto", "--once", "--json"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert!(
        has_event(&text(&out.stdout), |e| {
            e["event"] == "no-switch" && e["reason"] == "below-threshold"
        }),
        "{}",
        text(&out.stdout)
    );

    daemon.signal(libc::SIGTERM);
    let out = daemon.finish(STOP);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
}

#[test]
fn a_daemon_started_beside_a_running_auto_waits_for_its_engine_lock() {
    // D14, D4.3 (plan A Task 8), D5.4 (Task 1): the other way round. `auto` is the engine; the
    // daemon waits, names it in its event and its status, and never ticks.
    let d = short_tempdir();
    let root = d.path();
    staying(root);
    let mut c = std_cmd(root);
    c.args(["auto", "--json"]).stdin(Stdio::null());
    let mut auto = Guarded::spawn(c);
    let auto_pid = auto.pid();
    auto.wait_for(START, "its first tick", |out| out.contains(r#""event":"no-switch""#));

    let mut daemon = Guarded::spawn(daemon_cmd(root, &["--json"]));
    daemon.wait_for(START, "waiting for the engine lock", |out| {
        has_event(out, |e| {
            e["event"] == "mode"
                && e["mode"] == "waiting"
                && e["waitingFor"] == "engine-lock"
                && e["holderPid"] == auto_pid
        })
    });
    let s = status(&socket(root));
    let p = &s.providers[0];
    assert_eq!(
        (
            p.provider.as_str(),
            p.mode.as_str(),
            p.waiting_for.as_deref(),
            p.holder_pid,
            p.autoswitch
        ),
        ("claude-code", "waiting", Some("engine-lock"), Some(auto_pid), true)
    );
    thread::sleep(Duration::from_secs(2));
    assert!(
        !has_event(&daemon.stdout(), |e| matches!(
            e["event"].as_str(),
            Some("poll" | "switch" | "no-switch")
        )),
        "the waiting daemon ticked: {}",
        daemon.stdout()
    );
    assert_eq!(engine_holder(root), Some(auto_pid));

    daemon.signal(libc::SIGTERM);
    assert_eq!(daemon.finish(STOP).status.code(), Some(0));
    auto.signal(libc::SIGTERM);
    let out = auto.finish(STOP);
    assert_eq!(out.status.code(), Some(0));
    let polls = json_lines(&text(&out.stdout))
        .iter()
        .filter(|e| e["event"] == "poll")
        .count();
    assert_eq!(polls, 1, "auto ticked once, and nothing else ticked");
}

#[test]
fn a_second_daemon_waits_and_takes_over_when_the_first_stops() {
    // D4.1 step 2, D14 (plan A Task 12): one daemon per data directory. The second says whom it
    // waits for, retries every 5 s, and takes over the lock, the socket and the engine once the
    // first has stopped.
    let d = short_tempdir();
    let root = d.path();
    staying(root);
    let mut first = Guarded::spawn(daemon_cmd(root, &["--json"]));
    let first_pid = first.pid();
    first.wait_for(START, "auto mode", mode_event("auto"));
    assert_eq!(daemon_holder(root), Some(first_pid));

    let mut second = Guarded::spawn(daemon_cmd(root, &[]));
    let second_pid = second.pid();
    let notice = format!("a daemon already runs (pid {first_pid}); waiting for it to stop");
    second.wait_for_output(START, "the waiting notice", |out, err| {
        out.contains(&notice) || err.contains(&notice)
    });
    thread::sleep(Duration::from_secs(1));
    assert!(second.is_running());
    assert_eq!(daemon_holder(root), Some(first_pid), "the waiting daemon wrote no record");
    assert_eq!(status(&socket(root)).daemon.pid, first_pid, "the socket is still the first's");

    first.signal(libc::SIGTERM);
    assert_eq!(first.finish(STOP).status.code(), Some(0));
    wait_until(Duration::from_secs(15), "the second daemon's record", || {
        daemon_holder(root) == Some(second_pid)
    });
    wait_until(START, "the engine lock naming the second daemon", || {
        engine_holder(root) == Some(second_pid)
    });
    assert_eq!(status(&socket(root)).daemon.pid, second_pid);

    second.signal(libc::SIGTERM);
    assert_eq!(second.finish(STOP).status.code(), Some(0));
    assert_stopped_cleanly(root);
}

#[test]
fn a_cli_switch_while_the_daemon_runs_reaches_a_subscriber_as_changed() {
    // D4.4 (plan A Task 10), D5.5 (Task 2), D14: another process's commit reaches subscribers
    // as `changed`, within the store watch's 2 s period.
    let d = short_tempdir();
    let root = d.path();
    staying(root);
    let mut daemon = Guarded::spawn(daemon_cmd(root, &["--json"]));
    daemon.wait_for(START, "its first tick", |out| out.contains(r#""event":"no-switch""#));
    let events = subscribe(&socket(root));
    // Whatever the daemon's start produced has arrived after more than one watch period.
    thread::sleep(Duration::from_millis(2_500));
    while events.try_recv().is_ok() {}

    let out = run(root, &["switch", "1", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    next_event(&events, Duration::from_secs(5), "changed after the switch", |e| {
        matches!(e, Event::Changed)
    });

    daemon.signal(libc::SIGTERM);
    assert_eq!(daemon.finish(STOP).status.code(), Some(0));
}

/// The recorded usage reply, its windows' resets moved to fixed distances from `now` (as
/// usage_cli.rs's).
fn usage_reply(now: i64) -> MockReply {
    let recorded: Value = serde_json::from_str(include_str!(
        "../../tagteam-cc/tests/fixtures/endpoints/usage-200.json"
    ))
    .unwrap();
    let mut body = recorded["body"].clone();
    body["five_hour"]["resets_at"] = json!(format_iso8601(now + 9_630));
    body["seven_day"]["resets_at"] = json!(format_iso8601(now + 291_630));
    body["limits"][2]["resets_at"] = json!(format_iso8601(now + 291_630));
    MockReply::Json { status: 200, body }
}

/// `a@x.co` at position 1 and `b@x.co` at position 2 (live), each with its own access token, so
/// the server's log tells their identities apart. Neither has a reading.
fn two_bearers(root: &Path) -> (AccountId, AccountId) {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    login_with_token(&env, &kc, "a@x.co", "", "at-a", "rt-a");
    cmd(root).arg("add").assert().success();
    login_with_token(&env, &kc, "b@x.co", "", "at-b", "rt-b");
    cmd(root).arg("add").assert().success();
    let rows = store(root).accounts(&cc()).unwrap();
    (rows[0].id.clone(), rows[1].id.clone())
}

/// Spends `n` of `id`'s identity's hourly slots (§8.6) at `at_s`, as fetches that never
/// recorded would: one reservation, then `n − 1` further sends under it. With `at_s` more
/// than 90 s ago, its lease has lapsed, so only the budget stands between the account and a
/// fetch.
fn spend(root: &Path, id: &AccountId, n: u32, at_s: i64) {
    let s = store(root);
    let row = s.account(id).unwrap().unwrap();
    let r = match s
        .reserve_usage(&row, at_s * 1000, Eligibility::Scheduled, &PollBudget::STANDARD)
        .unwrap()
    {
        Reserve::Reserved(r) => r,
        other => panic!("the first slot is free: {other:?}"),
    };
    for _ in 1..n {
        let grant = s
            .authorize_send(&r, None, None, at_s * 1000, &PollBudget::STANDARD)
            .unwrap();
        assert!(matches!(grant, SendGrant::Send(_)), "a slot was refused: {grant:?}");
    }
}

/// The usage requests the server logged with `Bearer <at>`, from any process.
fn sent_with(server: &MockServer, at: &str) -> usize {
    let bearer = format!("Bearer {at}");
    server
        .requests()
        .iter()
        .filter(|r| {
            r.path.starts_with(USAGE)
                && r.headers
                    .iter()
                    .any(|(k, v)| k == "authorization" && *v == bearer)
        })
        .count()
}

#[test]
fn the_hourly_budget_holds_with_a_daemon_collecting_beside_list() {
    // §8.6, D12, D14: the budget is per identity, across every process. a's identity has spent
    // its hour and b's has one request left; both accounts are due everywhere. The daemon
    // collects (`daemon.autoswitch` false: D4.3's `collect` row, plan A Task 8), then `list`
    // runs again and again, one after another and four at once. Exactly one usage request
    // leaves, for b.
    let d = short_tempdir();
    let root = d.path();
    let (a, b) = two_bearers(root);
    let spent_at = now_epoch_s() - 100;
    spend(root, &a, PollBudget::STANDARD.hourly_requests, spent_at);
    spend(root, &b, PollBudget::STANDARD.hourly_requests - 1, spent_at);
    config_set(root, "notify.enabled", "false");
    config_set(root, "daemon.autoswitch", "false");
    let server = MockServer::start();
    server.on("GET", USAGE, usage_reply(now_epoch_s()));

    let mut c = daemon_cmd(root, &["--json"]);
    c.env("TAGTEAM_TEST_API_BASE", server.base_url());
    let mut daemon = Guarded::spawn(c);
    daemon.wait_for(START, "a collect round that read b", |out| {
        has_event(out, |e| {
            e["event"] == "usage"
                && e["provider"] == CLAUDE_CODE
                && e["updated"].as_array().is_some_and(|u| u.contains(&json!(2)))
        })
    });

    let list = |args: &[&str]| -> Output {
        let mut c = std_cmd(root);
        c.env("TAGTEAM_TEST_API_BASE", server.base_url())
            .args(args)
            .stdin(Stdio::null());
        Guarded::spawn(c).finish(START)
    };
    for _ in 0..3 {
        let out = list(&["list"]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }
    let together: Vec<Output> = thread::scope(|s| {
        let runs: Vec<_> = (0..4).map(|_| s.spawn(|| list(&["list"]))).collect();
        runs.into_iter().map(|r| r.join().unwrap()).collect()
    });
    for out in &together {
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }
    let v: Value = serde_json::from_slice(&list(&["list", "--json"]).stdout).unwrap();

    daemon.signal(libc::SIGTERM);
    assert_eq!(daemon.finish(STOP).status.code(), Some(0));

    assert_eq!(sent_with(&server, "at-a"), 0, "a's identity had spent its hour");
    assert_eq!(
        sent_with(&server, "at-b"),
        1,
        "b's identity had one request left, and the daemon sent it"
    );
    assert_eq!(server.hits("GET", USAGE), 1, "nothing else was sent");
    assert_eq!(v["accounts"][0]["usageError"], "over-budget", "{v}");
    let since = now_epoch_s() - PollBudget::STANDARD.count_window_s;
    let s = store(root);
    for id in [&a, &b] {
        let key = s.account(id).unwrap().unwrap().identity_key;
        assert_eq!(
            s.usage_request_count(&cc(), &key, since).unwrap(),
            PollBudget::STANDARD.hourly_requests,
            "{key:?}: the hour is full, never over"
        );
    }
}

#[test]
fn a_replaced_program_file_stops_the_daemon_with_exit_75() {
    // D4.5, D8.1, D14 (plan A Tasks 10 and 12): the file the service starts was replaced, as an
    // upgrade replaces it, so the daemon stops as it does on a signal and exits 75, for the
    // service to start the new one. Decision 7's hooks point the watch at a scratch file and
    // check it every 200 ms.
    for json in [true, false] {
        let d = short_tempdir();
        let root = d.path();
        staying(root);
        let program = root.join("bin/tagteam");
        fs::create_dir_all(program.parent().unwrap()).unwrap();
        fs::write(&program, "the installed tagteam\n").unwrap();
        let args: &[&str] = if json { &["--json"] } else { &[] };
        let mut c = daemon_cmd(root, args);
        c.env("TAGTEAM_TEST_UPGRADE_WATCH_MS", "200")
            .env("TAGTEAM_TEST_DAEMON_PROGRAM", &program);
        let mut daemon = Guarded::spawn(c);
        let pid = daemon.pid();
        wait_until(START, "the engine lock naming the daemon", || {
            engine_holder(root) == Some(pid)
        });
        thread::sleep(Duration::from_secs(1)); // five watch periods with the file unchanged
        assert!(daemon.is_running(), "it stopped before any upgrade");
        // A new file renamed over the path, as a package manager installs one: another inode,
        // another size.
        let staged = root.join("bin/.tagteam.new");
        fs::write(&staged, "a newer tagteam, a little longer\n").unwrap();
        fs::rename(&staged, &program).unwrap();
        let out = daemon.finish(STOP);
        assert_eq!(out.status.code(), Some(75), "{}", text(&out.stderr));
        if json {
            let events = json_lines(&text(&out.stdout));
            let last = events.last().unwrap();
            assert_eq!(
                (&last["event"], &last["reason"]),
                (&json!("stopping"), &json!("upgrade")),
                "{events:?}"
            );
            assert_eq!(events.iter().filter(|e| e["event"] == "stopping").count(), 1);
        } else {
            let all = text(&out.stdout) + &text(&out.stderr);
            assert!(all.contains("tagteam was upgraded; start the daemon again"), "{all}");
        }
        assert_stopped_cleanly(root);
    }
}

#[test]
fn a_program_replaced_while_a_second_daemon_waits_is_an_upgrade_at_its_first_watch_run() {
    // D4.5, plan A Decision 17: `tagteam daemon` stamps its program first thing, before the
    // `daemon.lock` wait. A tagteam installed while a second daemon waits for the first to stop
    // is an upgrade at the second's first upgrade-watch run once it serves (exit 75), never its
    // own baseline. Decision 7's hooks point the second's watch at a scratch file and check it
    // every 200 ms; the first daemon has no hooks.
    let d = short_tempdir();
    let root = d.path();
    staying(root);
    let program = root.join("bin/tagteam");
    fs::create_dir_all(program.parent().unwrap()).unwrap();
    fs::write(&program, "the installed tagteam\n").unwrap();
    let mut first = Guarded::spawn(daemon_cmd(root, &["--json"]));
    let first_pid = first.pid();
    first.wait_for(START, "auto mode", mode_event("auto"));

    let mut c = daemon_cmd(root, &["--json"]);
    c.env("TAGTEAM_TEST_UPGRADE_WATCH_MS", "200")
        .env("TAGTEAM_TEST_DAEMON_PROGRAM", &program);
    let mut second = Guarded::spawn(c);
    // The notice comes from the `daemon.lock` wait, so the stamp has been taken by now.
    let notice = format!("a daemon already runs (pid {first_pid}); waiting for it to stop");
    second.wait_for_output(START, "the waiting notice", |out, err| {
        out.contains(&notice) || err.contains(&notice)
    });
    // A newer tagteam is installed while the second waits: a new file renamed over the path,
    // another inode and another size.
    let staged = root.join("bin/.tagteam.new");
    fs::write(&staged, "a newer tagteam, a little longer\n").unwrap();
    fs::rename(&staged, &program).unwrap();
    assert!(second.is_running(), "it stopped while it waited");
    assert_eq!(daemon_holder(root), Some(first_pid), "the second never held daemon.lock");

    first.signal(libc::SIGTERM);
    assert_eq!(first.finish(STOP).status.code(), Some(0));
    // The lock is tried again every 5 s; then the first watch run comes as the daemon serves.
    let out = second.finish(START);
    assert_eq!(out.status.code(), Some(75), "{}", text(&out.stderr));
    let events = json_lines(&text(&out.stdout));
    let last = events.last().unwrap();
    assert_eq!(
        (&last["event"], &last["reason"]),
        (&json!("stopping"), &json!("upgrade")),
        "{events:?}"
    );
    assert_eq!(events.iter().filter(|e| e["event"] == "stopping").count(), 1);
    assert_stopped_cleanly(root);
}

#[test]
fn a_signal_stops_the_daemon_with_exit_0_and_leaves_no_socket_or_lock_record() {
    // D4.6, D8.1, D14 (plan A Task 12, Task 2): SIGTERM and SIGHUP, as for every command
    // (§14.1). The subscriber hears `stopping` with reason `signal` last, then the connection
    // closes; the socket goes, the engine lock's record is emptied, and `daemon.lock` is free
    // with no record.
    for signal in [libc::SIGTERM, libc::SIGHUP] {
        let d = short_tempdir();
        let root = d.path();
        staying(root);
        let mut daemon = Guarded::spawn(daemon_cmd(root, &["--json"]));
        let pid = daemon.pid();
        daemon.wait_for(START, "auto mode", mode_event("auto"));
        wait_until(START, "the engine lock naming the daemon", || {
            engine_holder(root) == Some(pid)
        });
        let events = subscribe(&socket(root));
        daemon.signal(signal);
        let last = next_event(&events, STOP, "stopping", |e| {
            matches!(e, Event::Stopping { .. })
        });
        assert_eq!(last, Event::Stopping { reason: "signal".into() }, "{signal}");
        assert!(
            matches!(events.recv_timeout(STOP), Err(RecvTimeoutError::Disconnected)),
            "{signal}: the connection closed after `stopping`"
        );
        let out = daemon.finish(STOP);
        assert_eq!(out.status.code(), Some(0), "{signal}: {}", text(&out.stderr));
        let printed = json_lines(&text(&out.stdout));
        let final_event = printed.last().unwrap();
        assert_eq!(
            (&final_event["event"], &final_event["reason"]),
            (&json!("stopping"), &json!("signal")),
            "{signal}"
        );
        assert_stopped_cleanly(root);
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run, outside the sandbox (Unix sockets and `/tmp`):
- `cargo test -p tagteam --lib test_hook_tests`
- `cargo test -p tagteam --features test-support --test daemon_procs`

Expected:
- The first fails to compile: `cannot find function test_program`, `test_upgrade_watch_ms`
  and the two constants.
- In the second, `a_replaced_program_file_stops_the_daemon_with_exit_75` fails with `tagteam
  was still running 10s later`, and
  `a_program_replaced_while_a_second_daemon_waits_is_an_upgrade_at_its_first_watch_run` with
  `tagteam was still running 20s later`: the hooks are not read yet, so each watch follows the
  cargo binary, every 60 s. The other six pass. If one of them fails, the defect is in the task its
  comment names. Fix it there, and record the fix in that task's commit.

- [ ] **Step 3: Implement the hooks**

In `crates/tagteam/src/daemon_cmd.rs`, add above the test modules:

```rust
/// Plan B Decision 7: the multi-process tests' hooks, read only by a `test-support` build.
/// `TAGTEAM_TEST_UPGRADE_WATCH_MS` shortens the upgrade watch's period (D4.5), so an exit-75
/// test takes seconds. `TAGTEAM_TEST_DAEMON_PROGRAM` names the file the watch follows, so a
/// test can replace it without touching the binary it runs.
#[cfg(any(test, feature = "test-support"))]
const TEST_UPGRADE_WATCH_MS: &str = "TAGTEAM_TEST_UPGRADE_WATCH_MS";
#[cfg(any(test, feature = "test-support"))]
const TEST_DAEMON_PROGRAM: &str = "TAGTEAM_TEST_DAEMON_PROGRAM";

/// Decision 7: the path the upgrade watch follows. It is `program`, unless `var` gives
/// `TAGTEAM_TEST_DAEMON_PROGRAM` a value that is not empty.
#[cfg(feature = "test-support")]
pub(crate) fn test_program(
    program: PathBuf,
    var: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> PathBuf {
    var(TEST_DAEMON_PROGRAM)
        .filter(|p| !p.is_empty())
        .map_or(program, PathBuf::from)
}

/// Decision 7: the upgrade watch's period. It is `ms`, unless `var` gives
/// `TAGTEAM_TEST_UPGRADE_WATCH_MS` a positive number of milliseconds.
#[cfg(feature = "test-support")]
pub(crate) fn test_upgrade_watch_ms(
    ms: i64,
    var: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> i64 {
    var(TEST_UPGRADE_WATCH_MS)
        .and_then(|v| v.into_string().ok())
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(ms)
}

/// A release build has no test hooks, whatever the environment holds.
#[cfg(not(feature = "test-support"))]
pub(crate) fn test_program(
    program: PathBuf,
    _var: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> PathBuf {
    program
}

/// A release build has no test hooks, whatever the environment holds.
#[cfg(not(feature = "test-support"))]
pub(crate) fn test_upgrade_watch_ms(
    ms: i64,
    _var: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> i64 {
    ms
}
```

`PathBuf` is imported by plan A's `daemon_cmd.rs` already. `OsString` is written in full, so no
import is added and none goes unused in either build.

At the call site, in `crates/tagteam/src/app.rs`'s `fn daemon` (plan A Task 12), the hooks
wrap the two values where `App::daemon` produces them. Plan A stamps the program as the
function's first statement (its Decision 17), so the program hook goes inside that stamp, and
`hook` is defined before it. Plan A's function opens:

```rust
    fn daemon(&mut self) -> Result<i32, Failure> {
        // D4.5, Decision 17: before anything that can wait (the unlock offer, the
        // `daemon.lock` wait), so a binary replaced meanwhile is an upgrade at the upgrade
        // watch's first run, never this daemon's own baseline.
        let program = ProgramStamp::take(daemon_cmd::program(self.engine.env().var("PATH")));
```

Replace those lines with:

```rust
    fn daemon(&mut self) -> Result<i32, Failure> {
        // Plan B Decision 7: a test-support build's hooks; a release build ignores them.
        let hook = |k: &str| std::env::var_os(k);
        // D4.5, Decision 17: before anything that can wait (the unlock offer, the
        // `daemon.lock` wait), so a binary replaced meanwhile is an upgrade at the upgrade
        // watch's first run, never this daemon's own baseline.
        let program = ProgramStamp::take(daemon_cmd::test_program(
            daemon_cmd::program(self.engine.env().var("PATH")),
            &hook,
        ));
```

Further down, plan A builds:

```rust
        let opts = DaemonOptions {
            provider: self.provider_flag.clone(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            program,
            upgrade_watch_ms: UPGRADE_WATCH_MS,
            keychain_counts: self.keychain_counts.clone(),
            backend: Backend::detect(self.platform == Platform::MacOs, path),
        };
```

Replace its `upgrade_watch_ms` line:

```rust
            upgrade_watch_ms: UPGRADE_WATCH_MS,
```

with:

```rust
            upgrade_watch_ms: daemon_cmd::test_upgrade_watch_ms(UPGRADE_WATCH_MS, &hook),
```

`program` stays as plan A passes it: the `ProgramStamp` taken above, of the hooked path.

The hooks read the process's environment directly, as `test_overrides` in `app.rs` does: they
are test plumbing, not variables the daemon records or acts on.

- [ ] **Step 4: Run the tests and see them pass**

Run, outside the sandbox:
- `cargo test -p tagteam --lib test_hook_tests` (no feature: one test, the hooks ignored)
- `cargo test -p tagteam --features test-support --lib test_hook_tests` (two tests)
- `cargo test -p tagteam --features test-support --test daemon_procs` (eight tests)
- `cargo test -p tagteam --features test-support --test auto_cli --test usage_cli --test cli --test signals`
  (the binary tests that use `login` and `Running`; `signals` opens a pty)

Expected: PASS.

- [ ] **Step 5: Write the performance runs**

In `crates/tagteam/tests/perf.rs`:

Replace the module comment's first sentence:

```rust
//! §1.1's timing targets through the real binary: `statusline` within 10 ms p95, and `list`
//! within 50 ms p95 when no usage fetch is due, alone and while a daemon runs against the same
//! store (D1.1 criterion 3, D12). Timings mean something only in an optimized
```

Extend the imports:

```rust
use std::process::Stdio;

use common::{
    Guarded, bloat_claude_json, cc_profile, hold_launch, now_epoch_s, record_history,
    short_tempdir, std_cmd, two_fresh_accounts, usage_window,
};
```

Add after `p95`:

```rust
/// The ticks a daemon's `--json` output has finished: each ends in one `switch` or `no-switch`
/// event (§11.4).
fn ticks(out: &str) -> usize {
    out.matches(r#""event":"no-switch""#).count() + out.matches(r#""event":"switch""#).count()
}

/// Untimed, before each run: 350 ms, so the 53 runs span about 19 s, more than the longest tick
/// delay (15 s × 1.1), and at least one tick lands among them.
fn spread(_: usize) {
    std::thread::sleep(Duration::from_millis(350));
}

/// D12, D14: a daemon on the fixture while the runs are timed. It is in `auto` mode, with the
/// shortest interval §6.4 allows (15 s), so it ticks while the runs go on. Its scheduler also
/// wakes every second, and its store watch reads the store every 2 s. Notifications are off,
/// so it starts nothing. Returns once its first tick is in.
fn start_daemon(root: &Path, base: &str) -> Guarded {
    for (key, value) in [("autoswitch.interval_seconds", "15"), ("notify.enabled", "false")] {
        let out = std_cmd(root)
            .args(["config", "set", key, value])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "config set {key}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let mut cmd = std_cmd(root);
    cmd.env("TAGTEAM_TEST_API_BASE", base)
        .args(["daemon", "--json"])
        .stdin(Stdio::null());
    let mut daemon = Guarded::spawn(cmd);
    daemon.wait_for(Duration::from_secs(30), "the daemon's first tick", |out| {
        ticks(out) >= 1
    });
    daemon
}

/// SIGTERM, and the daemon's exit 0 (D4.6).
fn stop(daemon: Guarded) {
    daemon.signal(libc::SIGTERM);
    let out = daemon.finish(Duration::from_secs(10));
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
```

Add at the end:

```rust
#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn statusline_p95_is_within_10_ms_while_a_daemon_runs() {
    let _one_at_a_time = serial();
    let d = short_tempdir();
    nothing_due(d.path());
    let server = MockServer::start();
    let daemon = start_daemon(d.path(), &server.base_url());
    let before = ticks(&daemon.stdout());
    let runs = timings(d.path(), &server.base_url(), &["statusline"], &[], &spread);
    let ticked = ticks(&daemon.stdout()) - before;
    stop(daemon);
    let p = p95(&runs);
    eprintln!("statusline p95 while a daemon runs {p:?} over {RUNS} runs, {ticked} ticks among them");
    assert!(ticked >= 1, "the daemon never ticked while the runs went on");
    assert_eq!(server.requests().len(), 0, "a request was sent");
    assert!(
        p <= Duration::from_millis(10),
        "statusline p95 while a daemon runs {p:?} over {RUNS} runs: {runs:?}"
    );
}

#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn list_p95_is_within_50_ms_when_nothing_is_due_while_a_daemon_runs() {
    let _one_at_a_time = serial();
    let d = short_tempdir();
    nothing_due(d.path());
    let server = MockServer::start();
    let daemon = start_daemon(d.path(), &server.base_url());
    let before = ticks(&daemon.stdout());
    let runs = timings(d.path(), &server.base_url(), &["list"], &[], &spread);
    let ticked = ticks(&daemon.stdout()) - before;
    stop(daemon);
    let p = p95(&runs);
    eprintln!("list p95 while a daemon runs {p:?} over {RUNS} runs, {ticked} ticks among them");
    assert!(ticked >= 1, "the daemon never ticked while the runs went on");
    assert_eq!(
        server.requests().len(),
        0,
        "a fetch was due, so this measured the wrong path"
    );
    assert!(
        p <= Duration::from_millis(50),
        "list p95 while a daemon runs {p:?} over {RUNS} runs: {runs:?}"
    );
}
```

The fixture's last readings carry a plan 183 s ahead (`record_history`), so neither the daemon's
ticks nor `list` fetch during the roughly 40 s a test takes. The server's log stays empty, and
the assertion pins that.

- [ ] **Step 6: Run the performance runs**

Run, outside the sandbox, on an idle machine:
- `cargo test --release -p tagteam --features test-support --test perf --no-run`
- `cargo test --release -p tagteam --features test-support --test perf -- --ignored while_a_daemon_runs --nocapture`

Expected: PASS. Each prints its p95 and the number of ticks among its runs (at least 1).
Before this step these tests did not exist, so there is no failing run to see. They measure;
they do not drive new code.

- [ ] **Step 7: Format and lint**

Run: `cargo fmt --all`, then `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`.
Expected: PASS.

- [ ] **Step 8: Commit** (the controller commits; implementers do not)

`git commit -m "Test the daemon against auto, a second daemon and the budget, in real processes"`

---

### Task 8: Spec amendments, errata and user docs

**Depends on:** 6, 7

D16's amendments to the core spec, in small hunks. The errata found in the daemon spec while
planning, in small hunks too. A `Daemon` section in the README, in its own voice, with the
README's other references updated. An `Unreleased` entry in the changelog, which keeps a
`## [Unreleased]` heading in Keep a Changelog format (CONTRIBUTING: "Update the changelog under
`Unreleased` for any change a user can see"). Each hunk is an exact before and after; apply
each with an exact-match edit, never a rewrite of the section.

CONTRIBUTING needs nothing here: plan A's Task 6 adds `tagteam-daemon` to its crate table and
changes "the four internal `version` entries" to five.

**Readings this task commits to:**
- **The core spec cites the daemon spec as "daemon spec D*n*"**, introduced in the §1.2 hunk, so
  "D4.3" inside the core spec is never read as a core section.
- **§4.1's engine row loses "auto-switch loop".** The loop moved to `tagteam-daemon` (D2, D3),
  and the engine keeps the auto-switch engine and its tick. Leaving the old words would make
  §4.1 wrong about the very crate this hunk adds.
- **§5 says `daemon.lock`'s record is emptied when the daemon stops.** Plan A's Task 12
  empties it before the lock goes, as D3 change 3 does for engine locks, and Task 7 asserts it.
- **The daemon spec's errata** (Step 2) change no design. Each makes the spec say what plan A
  and this plan build, where planning found the text wrong or silent: D4.6's socket order
  (Decision 9), D6.2's limit text (plan A Decision 16), D8.1's exit during the `daemon.lock`
  wait (plan A Decision 15), D9.1's recorded variables (Decision 10), D10's doctor states
  (Decisions 11 to 13), and D12 and D14's perf daemon (Decision 18). An `Errata` section at the
  spec's end lists them. They leave its `**Status:**` alone: it moves only as the execution
  notes and the merge say (`In progress`, then `Implemented`).
- **The README's Daemon section** sits under `## Commands`, after `### Automatic switching`,
  and the old sentence "tagteam has no background service in this release" goes. Notification
  wording follows D6.2 as signed off; Task 10 changes it only if D-R1 fails.
- **No code changes,** so the checks are textual: every hunk landed, links resolve, and
  `git diff --check` is clean.

**Files:**
- Modify: `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md` (§1.2, §4.1, §5, §6.4,
  §11.1, §13.1, §13.6)
- Modify: `docs/superpowers/specs/2026-10-14-tagteam-daemon-design.md` (D4.6, D6.2, D8.1,
  D9.1, D10, D12, D14, and a closing `Errata` section)
- Modify: `README.md` (features list, Commands, Automatic switching, new `### Daemon`, data
  table and credentials list, Uninstall)
- Modify: `CHANGELOG.md` (`## [Unreleased]`)

**Interfaces:**
- Consumes: the behaviour of Tasks 1–7 and plan A, as the daemon spec states it; the command
  names, flags, exit codes, keys and paths in plan B's Global Constraints.
- Produces: nothing code consumes. The PR-description note at the end of this task is used by
  Task 10.

- [ ] **Step 1: Amend the core spec**

In `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`:

§1.2. Before:

```markdown
- **Later sub-projects.** The daemon, socket API and notifications (#2), the TUI (#3) and the
  menu bar (#4).
```

After:

```markdown
- **Later sub-projects.** The TUI (#3) and the menu bar (#4). The daemon, its socket API and
  notifications (#2) are specified in
  [`2026-10-14-tagteam-daemon-design.md`](2026-10-14-tagteam-daemon-design.md), cited here as
  "daemon spec D*n*".
```

§4.1, the crate table. Before:

```markdown
| `tagteam-engine` | The SQLite store, the vault, the `ureq`-backed `Http` implementation, the provider registry, and the provider-generic operations: switch, refresh gate, usage collector, auto-switch loop, sessions, export/import, doctor | Yes |
| `tagteam` | The CLI binary (clap), human and JSON rendering, and the prompts | Thin |
```

After:

```markdown
| `tagteam-engine` | The SQLite store, the vault, the `ureq`-backed `Http` implementation, the provider registry, and the provider-generic operations: switch, refresh gate, usage collector, the auto-switch engine and its tick, sessions, export/import, doctor, and the daemon's paths, lock record and service units (daemon spec D3) | Yes |
| `tagteam-daemon` | The job scheduler that `auto` and the daemon run on, the daemon's provider jobs and watches, its socket server, the protocol types and their client, the notifier, and service install and uninstall (daemon spec D3) | Yes, through `tagteam-engine` |
| `tagteam` | The CLI binary (clap), human and JSON rendering, and the prompts | Thin |
```

§4.1, after the dependency rules. Before:

```markdown
Later sub-projects add crates that depend on `tagteam-engine`: `tagteam-daemon`, `tagteam-tui`
and `tagteam-menubar`.
```

After:

```markdown
`tagteam-daemon` (sub-project 2) depends on `tagteam-engine` and, like it, names no provider's
paths, keys or endpoints. Later sub-projects add `tagteam-tui` and `tagteam-menubar`.
```

§5, the paths table. Before:

```markdown
| Displaced lock | `$XDG_DATA_HOME/tagteam/locks/displaced.lock` (§6.3) |
```

After:

```markdown
| Displaced lock | `$XDG_DATA_HOME/tagteam/locks/displaced.lock` (§6.3) |
| Daemon lock | `$XDG_DATA_HOME/tagteam/locks/daemon.lock`, held by the running daemon. It holds the daemon's pid, start time and version as `{"pid":N,"start":S,"version":"…"}`, and is emptied when the daemon stops (daemon spec D4.1, D4.6) |
| Daemon socket | `$XDG_DATA_HOME/tagteam/daemon.sock`, mode 0600 (daemon spec D5.1) |
| Service unit | macOS: `~/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist`, the one path outside XDG, because launchd reads agents only there. Linux: `$XDG_CONFIG_HOME/systemd/user/tagteam.service`. Written 0600 by `tagteam daemon install` (daemon spec D9) |
```

§6.4, the end of the keys table and the paragraph after it. Before:

```markdown
| `ui.color` | `auto` | `auto`, `always`, `never` (`NO_COLOR` and `FORCE_COLOR` also honoured) | — |

CLI flags override settings for a single invocation and are clamped to the same ranges. A
running `auto` re-reads the file whenever its mtime changes (§11.4); its flags still win.
Nothing else caches settings across commands.
```

After:

```markdown
| `ui.color` | `auto` | `auto`, `always`, `never` (`NO_COLOR` and `FORCE_COLOR` also honoured) | — |
| `daemon.autoswitch` | true | bool | yes |
| `notify.enabled` | true | bool | — |
| `notify.events` | `["switch","quarantine","limit"]` | a list of `switch`, `quarantine` and `limit`; duplicates collapse; `''` is the empty list | yes |

The `daemon.*` and `notify.*` keys are the daemon's (daemon spec D7). `daemon.autoswitch` set
to false pauses the daemon's auto-switching for a provider. It does not affect `tagteam auto`.

CLI flags override settings for a single invocation and are clamped to the same ranges. A
running `auto` or daemon re-reads the file whenever its mtime changes (§11.4, daemon spec
D4.2); `auto`'s flags still win. Nothing else caches settings across commands.
```

§11.1, the loop's refusal. Before:

```markdown
  - A loop skips a provider whose lock another process holds, with a warning. If it gets no
    provider's lock at all, it exits 1, saying auto-switch already runs for them.
```

After:

```markdown
  - A loop skips a provider whose lock another process holds, with a warning. If it gets no
    provider's lock at all, it exits 1, saying auto-switch already runs for them. When the
    holder is the daemon, the message names it and says how to pause it (daemon spec D8.3).
```

§11.1, the daemon's line and the record. Before:

```markdown
  - The daemon (sub-project 2) takes the same lock, so `auto` and the daemon never both drive
    one provider.
  - Once it holds the lock, the engine writes its pid and start time into the lock file, the
    start time taken as for the `switch_journal` holder (§12.6). The record is never used for
    exclusion. `doctor` reads it to tell whether an engine runs, by an exact pid and
    start-time match (§13.6), instead of trying the lock, which an `auto` starting at that
    instant would then find taken.
```

After:

```markdown
  - The daemon (sub-project 2) takes the same lock, so `auto` and the daemon never both drive
    one provider. When it takes the lock, and when it gives it up (pausing through
    `daemon.autoswitch`), is daemon spec D4.3.
  - Once it holds the lock, the engine writes its pid and start time into the lock file, the
    start time taken as for the `switch_journal` holder (§12.6). The record is never used for
    exclusion. `doctor` reads it to tell whether an engine runs, by an exact pid and
    start-time match (§13.6), instead of trying the lock, which an `auto` starting at that
    instant would then find taken.
  - An engine that ends empties the record before it releases the lock (daemon spec D3 change
    3). A daemon that gives a provider up lives on, and a record left behind would name it as
    that provider's engine. A process killed by SIGKILL still leaves its record, which names a
    dead process.
```

§13.1, the commands table. Before:

```markdown
| `auto` | §11 |
```

After:

```markdown
| `auto` | §11 |
| `daemon [--provider P]` | Runs the daemon in the foreground (daemon spec D8.1) |
| `daemon install [--print] [--no-start]`, `daemon uninstall`, `daemon status` | Installs or removes the service unit that starts the daemon at login (daemon spec D9), and reports the daemon's state (D8.2) |
```

§13.1, exit codes. Before:

```markdown
**Exit codes:** `0` OK · `1` error · `2` usage error · `130` interrupted by SIGINT (128 + the
signal number for SIGTERM and SIGHUP, §14.1). `auto --once` uses 0–3.
```

After:

```markdown
**Exit codes:** `0` OK · `1` error · `2` usage error · `130` interrupted by SIGINT (128 + the
signal number for SIGTERM and SIGHUP, §14.1). `auto --once` uses 0–3. `daemon` exits `0`
after a signal, `75` to be restarted on a new binary or schema, `1` when it cannot start and
`2` for a usage error. `daemon status` exits `0` when the daemon runs, `3` when it does not, and
`1` on an error (daemon spec D8).
```

§13.6, the read-only list. Before:

```markdown
- It tests locks without waiting, and takes none that another process could wait on.
```

After:

```markdown
- It tests locks without waiting, and takes none that another process could wait on.
- It never connects to the daemon's socket. It reads the daemon's service state with
  `launchctl print`, or `systemctl --user show` and `loginctl show-user`, each bounded by 5 s,
  and none of them changes anything (daemon spec D10).
```

§13.6, the auto-switch checks. Before:

```markdown
  - an engine running → info, with its pid, from the engine lock's record (§11.1)
```

After:

```markdown
  - an engine running → info, with its pid, from the engine lock's record (§11.1); `the
    daemon (pid N)` when the record matches `daemon.lock`'s (daemon spec D10)
```

§13.6, a new group before Settings. Before:

```markdown
- **Settings** (§6.4): an unparseable file → fail, since every command runs on the defaults;
```

After:

```markdown
- **Daemon** (daemon spec D10):
  - `daemon.running`: a daemon running → info, with its pid and version from `daemon.lock`'s
    record, judged live by an exact pid and start-time match (§12.6); its version differs from
    this binary's → warn, naming the restart; none running → info
  - `daemon.service`: the unit installed and loaded, or not installed → info; the unit's
    program path missing → fail; a recorded variable (daemon spec D9.1) other than `PATH` that
    differs from doctor's own environment → warn, because the daemon then uses another store
    or home, with `tagteam daemon install` as the fix; on Linux, lingering off → info, and
    lingering that cannot be told → warn, naming why
  - `daemon.socket`: a socket file with no live daemon → info (the next start replaces it); a
    mode other than 0600, or a live daemon with no socket file → warn; a path too long to bind
    (daemon spec D5.1) → fail, once a daemon runs or a unit is installed, and not judged
    before
  - `daemon.notify`: notifications on and no backend (`notify-send` missing) → warn
  - `launchctl`, `systemctl` or `loginctl` missing, hung or failing → warn, naming why; it
    never fails the run by itself
- **Settings** (§6.4): an unparseable file → fail, since every command runs on the defaults;
```

- [ ] **Step 2: Correct the daemon spec (errata found while planning)**

In `docs/superpowers/specs/2026-10-14-tagteam-daemon-design.md`, apply each hunk exactly.

(a) D9.1, the recorded variables. Before:

```markdown
  No other variable is recorded. In particular none that `run` scrubs (§12.5) is.
```

After:

```markdown
  No other variable is recorded. Of the variables `run` scrubs from a session (§12.5), only
  `CLAUDE_SECURESTORAGE_CONFIG_DIR` is: it carries no credential, but names where the live
  login's credential is stored, which a switch must find exactly as the user's shell does. No
  variable that carries a credential or points at one, the rest of §12.5's list, is recorded.
```

(b) D12, the perf daemon. Before:

```markdown
  short as `auto`'s. `tests/perf.rs` gains runs with a fixture daemon ticking every second
  (D14).
```

After:

```markdown
  short as `auto`'s. `tests/perf.rs` gains runs with a fixture daemon at the shortest
  auto-switch interval (15 s, §6.4), so it ticks while the runs are timed (D14).
```

D14, the same. Before:

```markdown
- **Performance.** `tests/perf.rs`: `statusline` and `list` p95 with a fixture daemon running.
```

After:

```markdown
- **Performance.** `tests/perf.rs`: `statusline` and `list` p95 with a fixture daemon running
  at the shortest auto-switch interval (15 s, §6.4), at least one tick landing among the timed
  runs.
```

(c) D8.1, a signal before the daemon serves. Before:

```markdown
- **Exit codes:**
  - `0` after a signal (D4.6)
```

After:

```markdown
- **Exit codes:**
  - `0` after a signal once the daemon serves (D4.6). A signal during the wait for
    `daemon.lock` (D4.1), before the daemon serves, ends it as an interrupted command:
    128 + the signal number, with the `interrupted` error (§14.1)
```

(d) D6.2, the `limit` example and its durations. Before:

```markdown
  - `1 · work: 7d at 93%, no account to switch to (resets in 1d 4h)`
```

After:

```markdown
  - `1 · work: 7d at 93% (resets in 1d04h)`

  The `limit` text does not say why the daemon did not switch (paused, a cooldown, every
  account exhausted), since it is true for only some of those. Durations are written as `list`
  writes them (`1d04h`). A binding window whose reset has already passed is not notified.
```

(e) D10, what doctor reports. Before:

```markdown
| `daemon.service` | info: installed and loaded, or not installed. fail: the unit's program path does not exist. warn: a recorded variable (D9.1) differs from doctor's own environment, so the daemon uses another store or home; the fix is `tagteam daemon install` again. info (Linux): lingering is off |
| `daemon.socket` | info: a socket file with no live daemon (the next start replaces it). warn: its mode is not 0600. fail: its path is too long to bind (D5.1) |
```

After:

```markdown
| `daemon.service` | info: installed and loaded, or not installed. fail: the unit's program path does not exist. warn: a recorded variable (D9.1) other than `PATH` differs from doctor's own environment, so the daemon uses another store or home; the fix is `tagteam daemon install` again. `PATH` places no store or home, and a version manager changes it in every directory, so it is not compared. info (Linux): lingering is off. warn (Linux): whether lingering is on cannot be told (`loginctl` missing, hung or failing), with the reason |
| `daemon.socket` | info: a socket file with no live daemon (the next start replaces it). warn: its mode is not 0600, or a live daemon has no socket file (it runs without one, D5.1). fail: its path is too long to bind (D5.1), once a daemon runs or a unit is installed; until then the length is not judged, so `doctor` never fails for someone who does not use the daemon |
```

(f) D4.6, the order of the stop. Before:

```markdown
The accept and connection threads poll with 1 s timeouts and stop too. Every subscriber gets a
`stopping` event with reason `signal`. Then the daemon removes its socket, drops its engine
locks (emptying their records), and releases `daemon.lock`. It exits 0, as `auto`'s loop
does.
```

After:

```markdown
The accept and connection threads poll with 1 s timeouts and stop too. Every subscriber gets a
`stopping` event with reason `signal`. As that event is sent, the daemon removes its socket,
while it still holds `daemon.lock` and only if the file is still the socket this daemon bound;
open connections write `stopping`, then close. Then the daemon drops its engine locks
(emptying their records), and releases `daemon.lock`, emptying its record first. It exits 0,
as `auto`'s loop does.
```

The record. After D16's last line:

```markdown
- **§13.6:** add the daemon checks (D10).
```

add:

```markdown

## Errata

Found while planning the implementation, and corrected in place above. None changes the
design:
- **D4.6:** the socket is removed when `stopping` is sent, before `daemon.lock` is released,
  and only if it is still the socket this daemon bound; `daemon.lock`'s record is emptied
  before the lock goes.
- **D6.2:** the `limit` example no longer claims that no account is left to switch to, which
  the daemon cannot know, and writes durations as `list` does.
- **D8.1:** a signal during the wait for `daemon.lock`, before the daemon serves, exits
  128 + n as an interrupted command.
- **D9.1:** `CLAUDE_SECURESTORAGE_CONFIG_DIR` is recorded although `run` scrubs it; no
  variable that carries or points at a credential is.
- **D10:** `PATH` is left out of the variable comparison; a lingering that cannot be told warns
  with its reason; a too-long socket path fails only once a daemon runs or a unit is installed;
  a live daemon with no socket file warns.
- **D12, D14:** the perf daemon runs at the shortest auto-switch interval (15 s), since
  §6.4 allows nothing shorter.
```

- [ ] **Step 3: Document the daemon in the README**

In `README.md`:

The features list. Before:

```markdown
- **Switch automatically.** `tagteam auto` moves to a better account before the current one
  reaches a threshold.
```

After:

```markdown
- **Switch automatically.** `tagteam auto` moves to a better account before the current one
  reaches a threshold. The daemon does the same in the background, keeps every account's usage
  fresh, and tells you when it switches.
```

The Commands introduction. Before:

```markdown
one JSON object on stdout. The exceptions: `auto --json` prints one event per line,
```

After:

```markdown
one JSON object on stdout. The exceptions: `auto --json` and `daemon --json` print one event
per line,
```

The commands table. Before:

```markdown
| `auto` | Switch automatically before a rate limit, until stopped. `--once` ticks once; `--dry-run` decides without switching |
```

After:

```markdown
| `auto` | Switch automatically before a rate limit, until stopped. `--once` ticks once; `--dry-run` decides without switching |
| `daemon` | Switch automatically and keep usage fresh in the background. `install` and `uninstall` its login service, show its `status`, or run it in the foreground |
```

Exit codes. Before:

```markdown
Exit codes: `0` success, `1` error, `2` usage error, and 128 plus the signal number when
interrupted (`130` for Ctrl-C). `tagteam auto --once` uses `0` switched, `1` error, `2` no
action and `3` blocked.
```

After:

```markdown
Exit codes: `0` success, `1` error, `2` usage error, and 128 plus the signal number when
interrupted (`130` for Ctrl-C). `tagteam auto --once` uses `0` switched, `1` error, `2` no
action and `3` blocked. `tagteam daemon` exits `0` when a signal stops it and `75` when it
must be restarted on a new version. `tagteam daemon status` exits `0` when the daemon runs
and `3` when it does not.
```

Automatic switching, its last sentence. Before:

```markdown
`autoswitch.strategy`, for example `tagteam config set autoswitch.threshold 85`. tagteam has
no background service in this release, so keep `auto` running in a terminal multiplexer or
under a process supervisor of your choice.
```

After:

```markdown
`autoswitch.strategy`, for example `tagteam config set autoswitch.threshold 85`. To switch in
the background, without a terminal, use the [daemon](#daemon), which reads the same settings.
```

A new section, between `### Automatic switching` and `### Running accounts side by side`.
Before:

````markdown
### Running accounts side by side
````

After:

````markdown
### Daemon

The daemon runs tagteam in the background. Install it once:

```sh
tagteam daemon install
```

From then on it starts when you log in, and restarts if it fails. While it runs, it:

- keeps every account's usage fresh, on the same schedule and within the same request budget
  as the commands, so `list` and `statusline` rarely have to fetch;
- switches accounts automatically, exactly as `tagteam auto` does, for every provider with at
  least two switchable accounts;
- tells you about the events that matter (see below);
- serves a local socket that the TUI and scripts can read.

The daemon switches accounts unattended, and `install` says so. To pause automatic switching
and keep only the usage collection, change a setting. It takes effect within about a second,
and lasts until you change it back:

```sh
tagteam config set daemon.autoswitch false   # pause
tagteam config set daemon.autoswitch true    # resume
```

While the daemon switches a provider, `tagteam auto` refuses to run for it and says so. Pause
the daemon first if you want `auto` in a terminal.

`tagteam daemon status` shows whether the daemon runs, what it is doing for each provider, and
the state of its service. `tagteam daemon uninstall` stops it and removes the service.

**Notifications.** The daemon tells you when it switches accounts, when an account needs a new
login, and when the live account reaches the switch threshold and the daemon does not switch
away from it:

```
Switched 1 · work → 2 · spare (5h at 92%)
```

On macOS they come from `osascript` and show as Script Editor, which asks once for permission
to send notifications. On Linux they need `notify-send`. Turn them off with
`tagteam config set notify.enabled false`, or choose which ones you get with `notify.events`,
for example `tagteam config set notify.events switch,quarantine`.

**The service.** On macOS it is a LaunchAgent,
`~/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist`, which needs you to be logged
in to the desktop; over SSH alone, run `tagteam daemon` in the foreground instead. On Linux it
is a systemd user unit, `~/.config/systemd/user/tagteam.service`. To keep it running after you
log out, run `loginctl enable-linger`. The unit records the variables that decide which store,
Claude Code home, proxy and log the daemon uses (`XDG_*`, `CLAUDE_CONFIG_DIR`, the proxy
variables, `TAGTEAM_LOG` and `PATH`), so run `tagteam daemon install` again after you change
them. `install --print` prints the unit without installing anything, and `--no-start` installs
it for your next login without starting it now.

If the macOS login keychain is locked, the daemon waits for it, trying again after a minute and
then less and less often, up to every 15 minutes. When you upgrade tagteam, the daemon notices
within a minute, exits with code 75, and the service starts the new version. `tagteam daemon`
runs it in the foreground, with a line per check as `auto` prints, for use under a process
supervisor of your own.

### Running accounts side by side
````

The data table. Before:

```markdown
| Log | `$XDG_STATE_HOME/tagteam/tagteam.log` (default `~/.local/state/tagteam/`) |
```

After:

```markdown
| Log | `$XDG_STATE_HOME/tagteam/tagteam.log` (default `~/.local/state/tagteam/`) |
| Daemon socket and lock | `$XDG_DATA_HOME/tagteam/daemon.sock` and `locks/daemon.lock` |
| Daemon service unit | macOS: `~/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist`; Linux: `$XDG_CONFIG_HOME/systemd/user/tagteam.service` |
```

The credentials list. Before:

```markdown
- **The log** never contains an email, a token or any part of a credential.
```

After:

```markdown
- **The log** never contains an email, a token or any part of a credential.
- **The daemon's socket** has mode 0600 and answers only your own user. No credential ever
  crosses it. Its service unit has mode 0600 too, because a proxy URL it records can hold a
  password.
```

Uninstall. Before:

```markdown
## Uninstall and purge

`tagteam purge` deletes everything tagteam stores: the accounts and their vault entries
```

After:

```markdown
## Uninstall and purge

If you installed the daemon, remove it first: `tagteam daemon uninstall` stops it and deletes
its service unit.

`tagteam purge` deletes everything tagteam stores: the accounts and their vault entries
```

- [ ] **Step 4: Add the changelog entry**

In `CHANGELOG.md`. Before:

```markdown
## [Unreleased]

## [20261014.2045.0] - 2026-10-14
```

After:

```markdown
## [Unreleased]

### Added

- `tagteam daemon`: a background process that keeps every account's usage fresh and switches
  accounts automatically, as `tagteam auto` does, for each provider with two or more switchable
  accounts. Every command still works without it.
- `tagteam daemon install`, `uninstall` and `status`. `install` sets up a LaunchAgent on macOS
  or a systemd user unit on Linux, which starts the daemon at login and restarts it after a
  failure or an upgrade. `install --print` prints the unit instead.
- Notifications from the daemon when it switches accounts, when an account needs a new login,
  and when the live account reaches the switch threshold and the daemon does not switch away
  from it: through `osascript` on macOS and `notify-send` on Linux.
- Settings `daemon.autoswitch` (pause the daemon's switching, per provider), `notify.enabled`
  and `notify.events`.
- A local socket, `daemon.sock` (mode 0600, your user only), that streams the daemon's events
  and answers status requests, for the TUI and for scripts.
- `tagteam doctor` reports the daemon, its service unit, its socket and its notifications.
- The internal `tagteam-daemon` crate on crates.io.

### Changed

- `tagteam auto` runs on the daemon's scheduler. Its behaviour, output and exit codes are
  unchanged. When the daemon already switches a provider, `auto` says so and names the command
  that pauses it.
- An auto-switch engine that stops now empties its lock file's record, so `tagteam doctor`
  never names a process that no longer drives the provider.

## [20261014.2045.0] - 2026-10-14
```

- [ ] **Step 5: Verify the hunks**

Run:
- `rg -n "daemon spec D" docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`.
  Expected: hits in each of §1.2, §4.1, §5, §6.4, §11.1, §13.1 and §13.6.
- `rg -n "daemon.autoswitch|notify.events|daemon.sock|daemon.lock" docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md README.md CHANGELOG.md`.
  Expected: each key and path in the core spec, the README and the changelog.
- `rg -n "ticking every second|no account to switch to|In particular none that|^## Errata$" docs/superpowers/specs/2026-10-14-tagteam-daemon-design.md`.
  Expected: one hit, `## Errata`: the old wording is gone from D6.2, D9.1 and D12.
- `rg -n '^\*\*Status:\*\*' docs/superpowers/specs/2026-10-14-tagteam-daemon-design.md`.
  Expected: the status line this task found, unchanged.
- `test -f docs/superpowers/specs/2026-10-14-tagteam-daemon-design.md && rg -n "^### Daemon$|\(#daemon\)" README.md`.
  Expected: the file exists. The README shows the heading once and the link once (the anchor
  `#daemon` is GitHub's slug for `### Daemon`).
- `rg -n "no background service|no account to switch to" README.md CHANGELOG.md`. Expected: no
  hit.
- `git diff --check`. Expected: no output.

- [ ] **Step 6: Commit** (the controller commits; implementers do not)

`git commit -m "Amend the specs for the daemon and document it"`

**For the PR description** (Task 10 puts this under a "Before the next release" heading, after
the template's sections):

```markdown
**Before the next release: `tagteam-daemon` is a new crate on crates.io.** crates.io trusted
publishing can only be added to a crate that exists, so its first version is published by hand
with a short-lived API token (CONTRIBUTING, "Releasing"). `publish-crates.yml` already lists it
between `tagteam-engine` and `tagteam`, so on the first release:

1. The release's crates.io job publishes `tagteam-core`, `tagteam-provider`, `tagteam-cc` and
   `tagteam-engine`, then fails on `tagteam-daemon`, which has no trusted publisher yet.
2. Create a crates.io API token with the `publish-new` scope and a short expiry. From the
   release tag's checkout, run `cargo publish -p tagteam-daemon --locked` with it.
3. Register the crate's trusted publisher (crates.io Settings, Trusted Publishing, GitHub:
   owner `michaelrigart`, repository `tagteam`, workflow `release.yml`, no environment). Then
   revoke the token.
4. Re-run the failed job. It skips every crate already published and publishes `tagteam`. If
   crates.io answers HTTP 429 for the new name, wait ten minutes and re-run.
```

---

### Task 9: The token report's jobs (gated: run only after the token report's branch has merged into `main` and `daemon` is rebased onto it)

**Depends on:** 8, and the token report merged into `main` with `daemon` rebased onto it

D13: the token report's index refresh and its session watch become two scheduler jobs. When
the index changes, the daemon emits the reserved `token-index` event (D5.5). The token report
spec (`e6cc92a`, `docs/superpowers/specs/2026-10-14-tagteam-token-report-design.md`) defines
the engine calls (§5, §6, §10) and the daemon contract (§12):
- `sight_sessions` about every 5 s;
- `refresh_tokens(provider, Wait::Try, Some(1 s), cancel, progress)` about every 60 s, with an
  incomplete refresh due again at once, after any other due job, and `Busy` tried on the next
  tick;
- after each call, `token_index_state`, and a broadcast when `changed_at_ms` moved since the
  last one.

The event is the `IndexState` in camelCase, with ISO 8601 UTC times: `{provider, refreshedAt,
changedAt, files, responses, skippedLines, unreadableFiles, oldest, newest}`.

If the daemon PR is ready before the token report has merged, this task waits, and the PR
description says so (Task 10).

**Readings this task commits to:**
- **Both jobs read the index state after each call, and share one broadcast record.** Token
  report §12 says "After each call it reads `token_index_state(provider)`". So a sighting the
  5-second watch records reaches the TUI within 5 s, not at the next 60-second refresh. One
  `changed_at_ms` per provider, kept for both jobs, means a change is broadcast once, whichever
  job saw it first.
- **The first broadcast** is the first state with a `changed_at_ms`. An index that has never
  changed (`None`) is not broadcast.
- **No store, no work.** With no store yet, or with a store whose schema is newer than this
  build, both jobs skip their calls and keep their cadence. The daemon never creates the store
  (D4.1 step 4), and `refresh_tokens` would. A newer schema ends in the upgrade watch's exit 75
  (D4.5).
- **Providers** are the list the daemon builds its provider jobs from (all registered, or the
  one `--provider` names), keeping those whose `capabilities().transcripts` is true (token
  report §3.2).
- **Errors** are logged at WARN with `provider=<id>`, and recorded as the job's `lastError` in
  `status.jobs`: the run's first error, as `<provider>: <message>`. A run with none clears it.
  The jobs emit no `error` event, because those are the provider jobs' (§11.4). An
  `EngineError::Interrupted` stops the scheduler with `StopReason::Signal`, as an interrupted
  provider run does (D4.3, D4.6).
- **Delays:** sessions-watch, 5 s. token-index: 0 when any provider's refresh came back
  incomplete; otherwise 60 s, whether it completed, found the index `Busy`, or failed.
- **Status times** are epoch seconds, as every time on the board is (plan A Decision 12): a
  job divides its milliseconds when it writes its line.

**Files:**
- Create: `crates/tagteam-daemon/src/tokens.rs`
- Modify: `crates/tagteam-daemon/src/lib.rs` (`pub mod tokens;`)
- Modify: `crates/tagteam-daemon/src/events.rs` (`DaemonEvent::TokenIndex`)
- Modify: `crates/tagteam-daemon/src/protocol.rs` (`daemon_event_json`'s arm; the client's
  `Event::TokenIndex`; `parse_event`'s arm)
- Modify: `crates/tagteam-daemon/src/daemon.rs` (`Started::serve` adds the two jobs)
- Modify: every other exhaustive `match` on `DaemonEvent` (the CLI's human daemon sink in
  `crates/tagteam/src/daemon_cmd.rs`, and any other the compiler names)
- Modify: `crates/tagteam-engine/src/tokens*.rs`, only to derive `Debug, Clone, PartialEq, Eq` on
  `IndexState` if the merged type lacks any of them
- Modify: `crates/tagteam-daemon/src/protocol.rs`'s and `crates/tagteam-daemon/tests/client.rs`'s
  unknown-kind tests (Tasks 1 and 3), whose example kind, `token-index`, is now known
- Modify: `README.md`, `CHANGELOG.md` (one line each)

Plan A's manifest already has `tagteam-cc` (with `test-hooks`) and `tempfile` among the daemon
crate's dev-dependencies, so `Cargo.toml` does not change.
- Test: `crates/tagteam-daemon/tests/tokens.rs` (new), `crates/tagteam/tests/daemon_procs.rs`
  (one end-to-end test)

**Interfaces:**
- Consumes (the token report's, as merged):
  - `Engine::sight_sessions(&self, &ProviderId) -> Result<u64, EngineError>`
  - `Engine::refresh_tokens(&self, &ProviderId, Wait, Option<Duration>, &Cancel, &mut dyn FnMut(&IndexProgress)) -> Result<RefreshOutcome, EngineError>`
  - `enum RefreshOutcome { Done(IndexReport), Busy }`, `IndexReport { complete, .. }`
  - `Engine::token_index_state(&self, &ProviderId) -> Result<IndexState, EngineError>`
  - `IndexState { refreshed_at_ms, changed_at_ms, files, responses, skipped_lines,
    unreadable_files, oldest_ms, newest_ms }`
  - `Capabilities::transcripts`; the lock `locks/tokens-<provider>.lock`
  - plan A: `Job`, `JobCx`, `Ran`, `StopReason`, `StatusBoard::set_job`, `JobStatus`,
    `DaemonSink`; Task 1: `parse_event`, `Event`
- Produces:
  - `pub const SESSIONS_WATCH: &str = "sessions-watch"`, `pub const TOKEN_INDEX: &str = "token-index"`
  - `pub const SESSIONS_EVERY_MS: i64 = 5_000`, `pub const INDEX_EVERY_MS: i64 = 60_000`,
    `pub const INDEX_SLICE: Duration` (1 s)
  - `pub fn token_jobs(providers: Vec<ProviderId>, board: Arc<StatusBoard>, schema_newer: bool) -> (SessionsWatch, TokenIndexJob)`
  - `pub struct SessionsWatch` and `pub struct TokenIndexJob`, both `impl Job`;
    `TokenIndexJob::with_slice(self, Duration) -> Self`
  - `DaemonEvent::TokenIndex { provider: ProviderId, state: IndexState }`
  - client: `Event::TokenIndex { provider: String, raw: serde_json::Value }`

- [ ] **Step 1: Confirm the gate and the merged names**

Run:
- `git fetch origin`
- `git log --oneline -1 origin/main -- docs/superpowers/specs/2026-10-14-tagteam-token-report-design.md`
- `git merge-base --is-ancestor origin/main HEAD && echo "daemon is on main"`
- `rg -n "pub fn (refresh_tokens|sight_sessions|token_index_state)|pub (enum|struct) (Wait|RefreshOutcome|IndexReport|IndexState)\b|transcripts:" crates/tagteam-engine/src crates/tagteam-provider/src`

Expected: the spec's commit on `main`; `daemon is on main`; each name found. If the gate fails,
stop: this task waits.

Read token report §5, §6, §10 and §12 (`git show e6cc92a:docs/superpowers/specs/2026-10-14-tagteam-token-report-design.md`)
and the merged code. Check each name and signature in **Interfaces** against them: the module
path (`tagteam_engine::tokens` below), the fields of `IndexState` and `IndexReport`, and the
tokens lock's file name. Where the merged code differs, use the merged name in every step below,
and record the difference under "Rulings" in the PR description. If `IndexState` lacks
`Debug`, `Clone`, `PartialEq` or `Eq`, add the derive in the engine (every field is a `u64` or
an `Option<i64>`). `DaemonEvent` derives `Debug, Clone, PartialEq` and must keep them.

- [ ] **Step 2: Write the failing tests**

Create `crates/tagteam-daemon/tests/tokens.rs`:

```rust
//! The token report's jobs (D13; token report spec §12) against a real engine over a fixture
//! home: transcripts under `~/.claude/projects`, session records under `~/.claude/sessions`, a
//! store, and the index's own lock.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tagteam_cc::ClaudeCode;
use tagteam_cc::live::Platform;
use tagteam_core::{CLAUDE_CODE, ProviderId};
use tagteam_daemon::events::{DaemonEvent, DaemonSink};
use tagteam_daemon::protocol::{Event, daemon_event_json, parse_event};
use tagteam_daemon::scheduler::{Job, JobCx, Ran};
use tagteam_daemon::status::{DaemonInfo, JobStatus, NotifyInfo, StatusBoard};
use tagteam_daemon::tokens::{INDEX_EVERY_MS, SESSIONS_EVERY_MS, token_jobs};
use tagteam_engine::oracle::NoOracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::tokens::IndexState;
use tagteam_engine::vault::{KeychainVault, Vault};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_provider::{
    Env, FakeClock, FakeKeychain, FakeProcess, FakeProcessProbe, FlockGuard, RunShell,
    ScriptedHttp, ScriptedSpawner,
};

/// 2026-10-14T20:45:00Z.
const T0_MS: i64 = 1_792_010_700_000;

fn cc() -> ProviderId {
    ProviderId::new(CLAUDE_CODE)
}

struct Fx {
    _dir: tempfile::TempDir,
    env: Env,
    clock: Arc<FakeClock>,
    process: Arc<FakeProcessProbe>,
    engine: Engine,
    board: Arc<StatusBoard>,
}

/// A macOS engine over a fresh fixture home whose Claude Code has run, with its store created
/// when `store` is true.
fn fx(store: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    let kc = Arc::new(FakeKeychain::new());
    let clock = Arc::new(FakeClock::new(T0_MS));
    let process = Arc::new(FakeProcessProbe::new());
    let engine = Engine::new(EngineConfig {
        env: env.clone(),
        registry: ProviderRegistry::new()
            .with(Arc::new(ClaudeCode::new(kc.clone(), Platform::MacOs))),
        vault: Vault::new(Box::new(KeychainVault::new(kc))),
        oracle: Arc::new(NoOracle),
        clock: clock.clone(),
        http: Arc::new(ScriptedHttp::new()),
        default_provider: cc(),
        settings: Settings::default(),
        process: process.clone(),
        spawner: Arc::new(ScriptedSpawner::new()),
        run_shell: RunShell::Outside,
    });
    if store {
        engine.store().unwrap();
    }
    let board = StatusBoard::new(
        DaemonInfo {
            pid: 4242,
            version: "test".into(),
            started_at: T0_MS / 1000,
            socket: None,
            log: dir.path().join("tagteam.log"),
        },
        NotifyInfo {
            enabled: false,
            backend: None,
            events: Vec::new(),
        },
    );
    Fx {
        _dir: dir,
        env,
        clock,
        process,
        engine,
        board,
    }
}

#[derive(Default)]
struct Recorded(Mutex<Vec<DaemonEvent>>);

impl DaemonSink for Recorded {
    fn emit(&self, e: &DaemonEvent) {
        self.0.lock().unwrap().push(e.clone());
    }
}

impl Recorded {
    /// The `token-index` events so far.
    fn token_index(&self) -> Vec<(ProviderId, IndexState)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                DaemonEvent::TokenIndex { provider, state } => {
                    Some((provider.clone(), state.clone()))
                }
                _ => None,
            })
            .collect()
    }
}

fn run(job: &mut dyn Job, fx: &Fx, sink: &Recorded) -> Ran {
    let mut jitter = || 0.0;
    let mut cx = JobCx {
        engine: &fx.engine,
        sink,
        jitter: &mut jitter,
    };
    job.run(&mut cx)
}

fn job_status(fx: &Fx, name: &str) -> JobStatus {
    fx.board
        .snapshot()
        .jobs
        .into_iter()
        .find(|j| j.name == name)
        .unwrap_or_else(|| panic!("{name} is not on the board"))
}

/// Response `n` of `session` as one final assistant line (token report §2.1, §2.2), with its own
/// message and request IDs, `n` input tokens and 10 output tokens.
fn line(session: &str, n: u64) -> String {
    json!({
        "type": "assistant",
        "sessionId": session,
        "requestId": format!("req_{session}_{n:06}"),
        "timestamp": "2026-10-14T20:00:00.000Z",
        "cwd": "/work/app",
        "isSidechain": false,
        "message": {
            "id": format!("msg_{session}_{n:06}"),
            "model": "claude-opus-5-5",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": n,
                "output_tokens": 10,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0
            }
        }
    })
    .to_string()
}

/// Appends responses `from..to` of `session` to its transcript in the default home.
fn append(fx: &Fx, session: &str, from: u64, to: u64) -> PathBuf {
    let path = fx
        .env
        .home
        .join(format!(".claude/projects/-work-app/{session}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap();
    for n in from..to {
        writeln!(f, "{}", line(session, n)).unwrap();
    }
    path
}

#[test]
fn the_index_job_refreshes_and_broadcasts_each_change_once() {
    // §12: a refresh, then the index's state, broadcast when `changed_at_ms` moved.
    let fx = fx(true);
    let sink = Recorded::default();
    let (_, mut index) = token_jobs(vec![cc()], fx.board.clone(), false);
    append(&fx, "s1", 0, 2);
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    let events = sink.token_index();
    assert_eq!(events.len(), 1, "{events:?}");
    let (provider, state) = &events[0];
    assert_eq!(provider, &cc());
    assert_eq!((state.files, state.responses), (1, 2), "{state:?}");
    assert!(state.refreshed_at_ms.is_some() && state.changed_at_ms.is_some(), "{state:?}");
    let job = job_status(&fx, "token-index");
    assert_eq!(job.last_error, None);
    assert!(job.last_run_at.is_some() && job.next_run_at > job.last_run_at);

    fx.clock.advance_ms(60_000);
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    assert_eq!(sink.token_index().len(), 1, "nothing new, nothing broadcast");

    fx.clock.advance_ms(60_000);
    append(&fx, "s1", 2, 3);
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    let events = sink.token_index();
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[1].1.responses, 3);
}

#[test]
fn an_incomplete_refresh_is_due_again_at_once_until_it_completes() {
    // §5.2's budget and §12: a slice that runs out first returns `complete: false`, and the
    // job is due again at once. A transcript of several 1 MiB chunks under a zero budget stops
    // after its first chunk.
    let fx = fx(true);
    let sink = Recorded::default();
    let (_, index) = token_jobs(vec![cc()], fx.board.clone(), false);
    let mut index = index.with_slice(Duration::ZERO);
    const N: u64 = 12_000; // about 4 MiB of lines
    append(&fx, "big", 0, N);
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(0));
    let mut runs = 1;
    while run(&mut index, &fx, &sink) == Ran::After(0) {
        runs += 1;
        assert!(runs < 200, "the refresh never completed");
    }
    let state = fx.engine.token_index_state(&cc()).unwrap();
    assert_eq!(state.responses, N);
    assert!(state.refreshed_at_ms.is_some(), "{state:?}");
}

#[test]
fn a_refresh_another_process_holds_is_tried_again_in_a_minute() {
    // §5.1: `Wait::Try` answers `Busy` while another refresh holds the provider's tokens lock.
    // The job is due again in 60 s, with no error.
    let fx = fx(true);
    let sink = Recorded::default();
    let (_, mut index) = token_jobs(vec![cc()], fx.board.clone(), false);
    append(&fx, "s1", 0, 1);
    let held = FlockGuard::try_lock(&fx.env.data_dir().join("locks/tokens-claude-code.lock"))
        .unwrap()
        .expect("nothing else holds the tokens lock");
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    assert_eq!(fx.engine.token_index_state(&cc()).unwrap().responses, 0, "nothing was read");
    assert_eq!(job_status(&fx, "token-index").last_error, None);
    drop(held);
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    assert_eq!(fx.engine.token_index_state(&cc()).unwrap().responses, 1);
}

#[test]
fn a_failing_refresh_is_recorded_in_status_and_keeps_its_cadence() {
    // D5.4's `lastError`, and D4.3's rule that a failed run keeps its cadence. A `locks` path
    // that is a regular file makes the tokens lock impossible to open.
    let fx = fx(true);
    let sink = Recorded::default();
    let (_, mut index) = token_jobs(vec![cc()], fx.board.clone(), false);
    append(&fx, "s1", 0, 1);
    let locks = fx.env.data_dir().join("locks");
    let _ = fs::remove_dir_all(&locks);
    fs::write(&locks, "").unwrap();
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    let error = job_status(&fx, "token-index").last_error;
    assert!(
        error.as_deref().is_some_and(|e| e.starts_with("claude-code: ")),
        "{error:?}"
    );
    fs::remove_file(&locks).unwrap();
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    assert_eq!(job_status(&fx, "token-index").last_error, None, "a clean run clears it");
}

#[test]
fn a_session_the_watch_sights_is_broadcast_as_token_index() {
    // §12: the watch's sightings move `changed_at_ms` too, so the TUI hears of evidence that no
    // refresh brought.
    let fx = fx(true);
    let sink = Recorded::default();
    let (mut sessions, _) = token_jobs(vec![cc()], fx.board.clone(), false);
    assert_eq!(run(&mut sessions, &fx, &sink), Ran::After(SESSIONS_EVERY_MS));
    assert!(sink.token_index().is_empty(), "nothing sighted, nothing changed");

    fx.process.set(
        4242,
        FakeProcess {
            exists: Some(true),
            ..FakeProcess::default()
        },
    );
    let records = fx.env.home.join(".claude/sessions");
    fs::create_dir_all(&records).unwrap();
    fs::write(
        records.join("4242.json"),
        json!({"pid": 4242, "sessionId": "s-live", "cwd": "/work/app",
               "startedAt": T0_MS - 60_000, "kind": "interactive"})
        .to_string(),
    )
    .unwrap();
    fx.clock.advance_ms(5_000);
    assert_eq!(run(&mut sessions, &fx, &sink), Ran::After(SESSIONS_EVERY_MS));
    let events = sink.token_index();
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(events[0].1.changed_at_ms.is_some());
    assert_eq!(job_status(&fx, "sessions-watch").last_error, None);
}

#[test]
fn without_a_store_the_jobs_do_nothing_and_create_nothing() {
    // D4.1 step 4: the daemon never creates the store; the token jobs wait for one, as the
    // provider jobs do.
    let fx = fx(false);
    let sink = Recorded::default();
    let (mut sessions, mut index) = token_jobs(vec![cc()], fx.board.clone(), false);
    append(&fx, "s1", 0, 1);
    assert_eq!(run(&mut sessions, &fx, &sink), Ran::After(SESSIONS_EVERY_MS));
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    assert!(!fx.env.data_dir().exists(), "no data directory was created");
    assert!(sink.0.lock().unwrap().is_empty());
}

#[test]
fn with_a_newer_schema_the_jobs_wait_for_the_upgrade() {
    // D4.1 step 4: every job waits, and the upgrade watch ends it with exit 75 (D4.5).
    let fx = fx(true);
    let sink = Recorded::default();
    let (mut sessions, mut index) = token_jobs(vec![cc()], fx.board.clone(), true);
    append(&fx, "s1", 0, 1);
    assert_eq!(run(&mut sessions, &fx, &sink), Ran::After(SESSIONS_EVERY_MS));
    assert_eq!(run(&mut index, &fx, &sink), Ran::After(INDEX_EVERY_MS));
    assert_eq!(fx.engine.token_index_state(&cc()).unwrap().responses, 0);
    assert!(sink.0.lock().unwrap().is_empty());
}

#[test]
fn token_index_is_the_index_state_in_camel_case_and_the_client_reads_it() {
    // D5.5's reserved kind, with token report §12's fields; times in ISO 8601 UTC.
    let state = IndexState {
        refreshed_at_ms: Some(1_792_010_640_000),
        changed_at_ms: Some(1_792_010_670_500),
        files: 3,
        responses: 120,
        skipped_lines: 1,
        unreadable_files: 0,
        oldest_ms: Some(1_790_841_600_000),
        newest_ms: Some(1_792_010_640_000),
    };
    let v = daemon_event_json(
        &DaemonEvent::TokenIndex {
            provider: cc(),
            state,
        },
        T0_MS / 1000,
    );
    assert_eq!(
        v,
        json!({"schemaVersion": 1, "event": "token-index", "ts": "2026-10-14T20:45:00Z",
               "provider": "claude-code", "refreshedAt": "2026-10-14T20:44:00Z",
               "changedAt": "2026-10-14T20:44:30Z", "files": 3, "responses": 120,
               "skippedLines": 1, "unreadableFiles": 0, "oldest": "2026-10-01T08:00:00Z",
               "newest": "2026-10-14T20:44:00Z"})
    );
    assert!(
        v.to_string()
            .starts_with(r#"{"schemaVersion":1,"event":"token-index","ts":"#),
        "the envelope comes first"
    );
    assert_eq!(
        parse_event(&v),
        Some(Event::TokenIndex {
            provider: "claude-code".into(),
            raw: v.clone(),
        })
    );

    let empty = IndexState {
        refreshed_at_ms: None,
        changed_at_ms: None,
        files: 0,
        responses: 0,
        skipped_lines: 0,
        unreadable_files: 0,
        oldest_ms: None,
        newest_ms: None,
    };
    let v = daemon_event_json(
        &DaemonEvent::TokenIndex {
            provider: cc(),
            state: empty,
        },
        T0_MS / 1000,
    );
    for key in ["refreshedAt", "changedAt", "oldest", "newest"] {
        assert!(v[key].is_null(), "{key}: {v}");
    }
}
```

If the merged parser needs a transcript field `line` lacks, or the merged `sight_sessions`
needs a session-record field the record above lacks, copy the missing field from the token
report's own synthetic fixtures (its §15 CC parser and evidence tests). Keep the values here.

In `crates/tagteam/tests/daemon_procs.rs`, add the end-to-end test:

```rust
/// One final assistant line of a transcript (token report §2.1), as tokens.rs's.
fn transcript_line(session: &str, n: u64) -> String {
    json!({
        "type": "assistant", "sessionId": session, "requestId": format!("req_{n:06}"),
        "timestamp": "2026-10-14T20:00:00.000Z", "cwd": "/work/app", "isSidechain": false,
        "message": {"id": format!("msg_{n:06}"), "model": "claude-opus-5-5",
                    "stop_reason": "end_turn",
                    "usage": {"input_tokens": n, "output_tokens": 10,
                              "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}
    })
    .to_string()
}

#[test]
fn a_daemon_keeps_the_token_index_and_reports_its_jobs() {
    // D13, token report §12 (Task 9): the daemon indexes the default home's transcripts, says
    // so with `token-index`, and lists both jobs in `status` (D5.4).
    let d = short_tempdir();
    let root = d.path();
    staying(root);
    let transcript = Env::for_test(root)
        .home
        .join(".claude/projects/-work-app/s1.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(&transcript, format!("{}\n", transcript_line("s1", 1))).unwrap();

    let mut daemon = Guarded::spawn(daemon_cmd(root, &["--json"]));
    daemon.wait_for(START, "a token-index event", |out| {
        has_event(out, |e| {
            e["event"] == "token-index" && e["provider"] == CLAUDE_CODE && e["responses"] == 1
        })
    });
    let s = status(&socket(root));
    for name in ["sessions-watch", "token-index"] {
        let job = s
            .jobs
            .iter()
            .find(|j| j.name == name)
            .unwrap_or_else(|| panic!("{name} is not in status"));
        assert_eq!(job.last_error, None, "{name}");
        assert!(job.last_run_at.is_some() && job.next_run_at.is_some(), "{name}");
    }

    daemon.signal(libc::SIGTERM);
    assert_eq!(daemon.finish(STOP).status.code(), Some(0));
}
```

- [ ] **Step 3: Run them and see them fail**

Run: `cargo test -p tagteam-daemon --test tokens`
Expected: FAIL to compile: `unresolved import tagteam_daemon::tokens`, no variant
`DaemonEvent::TokenIndex`, no variant `Event::TokenIndex`.

- [ ] **Step 4: Implement**

Create `crates/tagteam-daemon/src/tokens.rs`:

```rust
//! The token report's two scheduler jobs (D13; token report spec §12). `sessions-watch`
//! records the sessions every home is running, every 5 s (§6.2). `token-index` reads new
//! transcript lines into the index every 60 s, in slices of about a second (§5.2). After each
//! call, each job reads the index's state, and a `changed_at_ms` that moved since the last
//! broadcast becomes one `token-index` event (D5.5), so the TUI re-queries. Both take only the
//! token report's own lock (§5.1), never an engine lock or the mutation lock.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tagteam_core::ProviderId;
use tagteam_engine::tokens::{RefreshOutcome, Wait};
use tagteam_engine::{Engine, EngineError};

use crate::events::{DaemonEvent, DaemonSink, StopReason};
use crate::scheduler::{Job, JobCx, Ran};
use crate::status::{JobStatus, StatusBoard};

pub const SESSIONS_WATCH: &str = "sessions-watch";
pub const TOKEN_INDEX: &str = "token-index";
/// Token report §12: sightings "about every 5 s".
pub const SESSIONS_EVERY_MS: i64 = 5_000;
/// Token report §12: a refresh "about every 60 s", and at once after an incomplete one.
pub const INDEX_EVERY_MS: i64 = 60_000;
/// One refresh slice (§5.2's budget): the scheduler runs one job at a time.
pub const INDEX_SLICE: Duration = Duration::from_secs(1);

/// What both jobs share: the providers they serve, the board, and per provider the
/// `changed_at_ms` its last `token-index` event carried.
struct Common {
    providers: Vec<ProviderId>,
    board: Arc<StatusBoard>,
    schema_newer: bool,
    broadcast: Mutex<BTreeMap<ProviderId, i64>>,
}

impl Common {
    /// The providers whose index this daemon keeps: those with transcripts (token report
    /// §3.2). None while there is no store (the daemon never creates one, D4.1 step 4), or
    /// while its schema is newer than this build (the upgrade watch ends that, D4.5).
    fn serving(&self, engine: &Engine) -> Result<Vec<ProviderId>, EngineError> {
        if self.schema_newer || engine.existing_store()?.is_none() {
            return Ok(Vec::new());
        }
        Ok(self
            .providers
            .iter()
            .filter(|p| {
                engine
                    .provider(p)
                    .is_ok_and(|provider| provider.capabilities().transcripts)
            })
            .cloned()
            .collect())
    }

    /// Token report §12: reads `provider`'s index state, and emits `token-index` when its
    /// `changed_at_ms` moved since the last event for it. An index that never changed is not
    /// broadcast.
    fn after_call(
        &self,
        engine: &Engine,
        provider: &ProviderId,
        sink: &dyn DaemonSink,
    ) -> Result<(), EngineError> {
        let state = engine.token_index_state(provider)?;
        let Some(changed) = state.changed_at_ms else {
            return Ok(());
        };
        {
            let mut last = self.broadcast.lock().unwrap_or_else(PoisonError::into_inner);
            if last.get(provider) == Some(&changed) {
                return Ok(());
            }
            last.insert(provider.clone(), changed);
        }
        sink.emit(&DaemonEvent::TokenIndex {
            provider: provider.clone(),
            state,
        });
        Ok(())
    }

    /// D5.4's `jobs` entry for one run.
    fn report(&self, name: &str, engine: &Engine, delay_ms: i64, error: Option<String>) {
        let now_s = engine.now_ms().div_euclid(1000);
        self.board.set_job(JobStatus {
            name: name.to_owned(),
            last_run_at: Some(now_s),
            next_run_at: Some(now_s + delay_ms.div_euclid(1000)),
            last_error: error,
        });
    }
}

/// What a run does with one call's error: an interruption stops the scheduler (D4.6); anything
/// else is logged at WARN and, if it is the run's first, kept for `lastError`.
fn note(
    result: Result<(), EngineError>,
    provider: &ProviderId,
    what: &str,
    error: &mut Option<String>,
) -> Option<Ran> {
    match result {
        Ok(()) => None,
        Err(EngineError::Interrupted(_)) => Some(Ran::Stop(StopReason::Signal)),
        Err(e) => {
            tracing::warn!(provider = %provider, error = %e, "{what} failed");
            error.get_or_insert_with(|| format!("{provider}: {e}"));
            None
        }
    }
}

/// Both jobs, sharing one record of what was broadcast.
pub fn token_jobs(
    providers: Vec<ProviderId>,
    board: Arc<StatusBoard>,
    schema_newer: bool,
) -> (SessionsWatch, TokenIndexJob) {
    let common = Arc::new(Common {
        providers,
        board,
        schema_newer,
        broadcast: Mutex::new(BTreeMap::new()),
    });
    (
        SessionsWatch {
            common: Arc::clone(&common),
        },
        TokenIndexJob {
            common,
            slice: INDEX_SLICE,
        },
    )
}

/// `sessions-watch`: `sight_sessions` for each provider, every 5 s (token report §6.2, §12).
pub struct SessionsWatch {
    common: Arc<Common>,
}

impl Job for SessionsWatch {
    fn name(&self) -> String {
        SESSIONS_WATCH.to_owned()
    }

    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
        let (engine, sink) = (cx.engine, cx.sink);
        let mut error = None;
        let providers = self.common.serving(engine).unwrap_or_else(|e| {
            error = Some(e.to_string());
            Vec::new()
        });
        for p in &providers {
            let called = engine
                .sight_sessions(p)
                .and_then(|_| self.common.after_call(engine, p, sink));
            if let Some(stop) = note(called, p, "sighting sessions", &mut error) {
                return stop;
            }
        }
        self.common
            .report(SESSIONS_WATCH, engine, SESSIONS_EVERY_MS, error);
        Ran::After(SESSIONS_EVERY_MS)
    }
}

/// `token-index`: one budgeted refresh slice per provider, every 60 s, and again at once while
/// a refresh is incomplete (token report §5.2, §12).
pub struct TokenIndexJob {
    common: Arc<Common>,
    slice: Duration,
}

impl TokenIndexJob {
    /// A refresh slice other than `INDEX_SLICE`. Tests use `Duration::ZERO` to end a refresh
    /// after its first chunk.
    pub fn with_slice(mut self, slice: Duration) -> Self {
        self.slice = slice;
        self
    }
}

impl Job for TokenIndexJob {
    fn name(&self) -> String {
        TOKEN_INDEX.to_owned()
    }

    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
        let (engine, sink) = (cx.engine, cx.sink);
        let mut error = None;
        let mut delay = INDEX_EVERY_MS;
        let providers = self.common.serving(engine).unwrap_or_else(|e| {
            error = Some(e.to_string());
            Vec::new()
        });
        for p in &providers {
            let refreshed = engine
                .refresh_tokens(p, Wait::Try, Some(self.slice), engine.cancel(), &mut |_| {})
                .map(|outcome| {
                    if matches!(outcome, RefreshOutcome::Done(ref report) if !report.complete) {
                        delay = 0;
                    }
                });
            if let Some(stop) = note(refreshed, p, "the token index refresh", &mut error) {
                return stop;
            }
            let read = self.common.after_call(engine, p, sink);
            if let Some(stop) = note(read, p, "reading the token index", &mut error) {
                return stop;
            }
        }
        self.common.report(TOKEN_INDEX, engine, delay, error);
        Ran::After(delay)
    }
}
```

In `crates/tagteam-daemon/src/lib.rs`, add `pub mod tokens;` in the module list.

In `crates/tagteam-daemon/src/events.rs`, add the variant at the end of `DaemonEvent`:

```rust
    /// D5.5's reserved `token-index` (token report spec §12): the index's state after a change.
    TokenIndex {
        provider: ProviderId,
        state: tagteam_engine::tokens::IndexState,
    },
```

In `crates/tagteam-daemon/src/protocol.rs`:

Add the function next to `daemon_event_json`:

```rust
/// The `token-index` event (D5.5's reserved kind; token report spec §12): the index's state in
/// camelCase, its times in ISO 8601 UTC, in §11.4's envelope.
fn token_index_json(provider: &ProviderId, s: &IndexState, now_s: i64) -> Value {
    let iso = |ms: Option<i64>| ms.map(|ms| format_iso8601(ms.div_euclid(1000)));
    json!({
        "schemaVersion": 1,
        "event": "token-index",
        "ts": format_iso8601(now_s),
        "provider": provider.as_str(),
        "refreshedAt": iso(s.refreshed_at_ms),
        "changedAt": iso(s.changed_at_ms),
        "files": s.files,
        "responses": s.responses,
        "skippedLines": s.skipped_lines,
        "unreadableFiles": s.unreadable_files,
        "oldest": iso(s.oldest_ms),
        "newest": iso(s.newest_ms),
    })
}
```

and add `use tagteam_engine::tokens::IndexState;` to the imports (`format_iso8601` is imported
already). In `daemon_event_json`'s `match` on the event, after the `DaemonEvent::Auto(e) =>
return event_json(e, now_s),` arm, add an arm that returns the whole object, as that one does:

```rust
        DaemonEvent::TokenIndex { provider, state } => {
            return token_index_json(provider, state, now_s);
        }
```

In the client's `Event`, add the variant before `Unknown`:

```rust
    /// D5.5's `token-index`; `raw` is the whole object, whose fields token report §12 defines.
    TokenIndex { provider: String, raw: serde_json::Value },
```

In Task 1's private `typed_event`, which `parse_event` calls, add an arm before
`_ => return None,`. A missing `provider` makes it `Unknown`, as a missing field does for the
other kinds:

```rust
        "token-index" => Event::TokenIndex {
            provider: provider()?,
            raw: raw.clone(),
        },
```

Two earlier tests use `token-index` as their example of a kind the client does not know, and it
is now known. Change their example kind to `some-future-kind`:
- In `crates/tagteam-daemon/src/protocol.rs`, `socket_protocol_tests`'s
  `unknown_and_unreadable_events`: in `let token = json!({…"event":"token-index"…});` and in
  `kind: "token-index".into(),` below it.
- In `crates/tagteam-daemon/tests/client.rs`,
  `a_kind_the_client_does_not_know_is_yielded_as_unknown`: in `let token_index = json!({…});`
  and in `kind: "token-index".into(),`.

In `Started::serve` (`crates/tagteam-daemon/src/daemon.rs`, plan A Task 12), the token jobs are
built and added between the upgrade watch and the provider jobs. `token_jobs` takes a clone of
the provider list, since the loop that follows consumes it. Replace:

```rust
        scheduler.add(Box::new(upgrade), now);
        for provider in providers {
```

with:

```rust
        scheduler.add(Box::new(upgrade), now);
        // D13: the token report's jobs serve the same providers. After the watches (Decision 11's
        // invariant holds: a tie runs in the order added), before the provider jobs.
        let (sessions, index) =
            crate::tokens::token_jobs(providers.clone(), Arc::clone(&board), schema_newer);
        scheduler.add(Box::new(sessions), now);
        scheduler.add(Box::new(index), now);
        for provider in providers {
```

They are first due at the same start time as the watches, and come after them on a tie.

Then run `cargo build --workspace --features tagteam/test-support`. Each exhaustive `match` on
`DaemonEvent` that the compiler names gets an arm. In the CLI's human daemon sink
(`daemon_cmd.rs`), the arm prints nothing: D8.1's human output has no line for it.

```rust
            DaemonEvent::TokenIndex { .. } => {}
```

The JSON sink and the server's hub serialize through `daemon_event_json`, so they need nothing
more. `NotifySink` notifies nothing for it (D6.1).

In `README.md`, the Daemon section's list. Before:

```markdown
- tells you about the events that matter (see below);
```

After:

```markdown
- keeps the `tagteam tokens` index current, so token reports open without a wait;
- tells you about the events that matter (see below);
```

In `CHANGELOG.md`, under `## [Unreleased]`, `### Added`, after the `tagteam daemon` bullet:

```markdown
- The daemon keeps the `tagteam tokens` index current. It records running Claude Code sessions
  every 5 seconds and indexes new transcript lines every minute, and its socket announces each
  change with a `token-index` event.
```

- [ ] **Step 5: Run the tests and see them pass**

Run, outside the sandbox (the end-to-end test binds a socket):
- `cargo test -p tagteam-daemon --test tokens` (eight tests)
- `cargo test -p tagteam-daemon --features test-hooks` (the whole crate: the protocol and
  client tests of Tasks 1 and 3 after the `parse_event` change, and plan A's `daemon::tests`
  after the `serve` change)
- `cargo test -p tagteam --features test-support --test daemon_procs` (nine tests)

then `cargo fmt --all`, `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`.
Expected: PASS.

- [ ] **Step 6: Commit** (the controller commits; implementers do not)

`git commit -m "Keep the token report's index from the daemon"`

---

### Task 10: Verification and the manual checks

**Depends on:** 8 (and 9 when it ran)

The whole branch, both plans, verified as CI verifies it, plus the release-only perf runs and
the Linux run in Docker. Then the three manual checks of D15 (D-R1, D-R2, D-R3) and D14's
real service round trip (Task 5's ignored test). Each loads a LaunchAgent on Michael's Mac, and
some read or switch real accounts, so each needs his go-ahead first (Decision 8). Results go
into the PR description.

**Readings this task commits to:**
- **Totals are reported as passed/total** per command, summed from cargo's `test result:`
  lines.
- **Sandbox.** The socket tests, `daemon_procs`, the pty tests (`signals`, `prompt::tests`),
  `real_keychain` and the perf runs bind Unix sockets, open ptys, write under `/tmp` or run
  `/usr/bin/security`. All of these are denied inside the sandbox, so these commands run
  outside it, each after it has first failed inside with the sandbox's
  `Operation not permitted`. `docker` is declared in `sandbox.excludedCommands`: its call runs
  outside the sandbox without the escape flag, as its own Bash call, with `docker` first and
  nothing piped.
- **Manual checks use throwaway agents wherever they can.** D-R1 needs no tagteam at all.
  D-R2's and D-R3's first halves run `/usr/bin/security` only, with every secret sent to
  `/dev/null`. Only D-R2's and D-R3's second halves run the real daemon over real accounts,
  and only if Michael says yes to that half.
- **A failed D-R1 is not a release blocker.** D15's mitigation applies: macOS notifications
  wait for the menu bar app. Step 9 writes that change. A D-R2 dialog, or an rc the counters
  do not class as locked, is an open item for Michael: D15 says "the backoff is revisited
  before release", which is a design decision.

**Files:**
- No changes, unless a check fails.
- If D-R1 fails (Step 9): `crates/tagteam-daemon/src/notify.rs`,
  `crates/tagteam-engine/src/doctor.rs` and its test, `docs/superpowers/specs/2026-10-14-tagteam-daemon-design.md`
  (D6.2, D10, D15), `README.md`, `CHANGELOG.md`.

**Interfaces:**
- Consumes: everything in both plans.
- Produces: the verification record in the PR description.

- [ ] **Step 1: Formatting and lints**

Run:
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`

Expected: no output from fmt; both clippy runs finish with no warning.

- [ ] **Step 2: The full suite, as CI runs it**

Run, outside the sandbox:

```sh
cargo test --workspace --locked --no-fail-fast --features tagteam/test-support 2>&1 | tee "$TMPDIR/tagteam-full.log" | tail -n 30
rg -o 'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored' -r '$1 $2 $3' "$TMPDIR/tagteam-full.log" | awk '{p+=$1; f+=$2; i+=$3} END {print p" passed, "f" failed, "i" ignored, "p"/"p+f}'
```

Expected: `0 failed`. Report it as `passed/total`.

Then without features. This runs the release-build halves of the test-hook tests (Task 7, and
`app.rs`'s `the_test_overrides_exist_only_with_test_support`):

```sh
cargo test --workspace --locked --no-fail-fast 2>&1 | tee "$TMPDIR/tagteam-nofeat.log" | tail -n 30
```

Expected: `0 failed`, summed the same way.

- [ ] **Step 3: The real Keychain driver**

Plan A Task 4 changed how `/usr/bin/security`'s secret reads are counted. Run, outside the
sandbox:

`cargo test -p tagteam-provider --locked --features real_keychain --test real_keychain`

Expected: PASS. It uses a throwaway keychain file, never the login keychain.

- [ ] **Step 4: The release perf runs**

Run, outside the sandbox, on an idle machine (close builds and other heavy processes):

`cargo test --release -p tagteam --features test-support --test perf -- --ignored --nocapture`

Expected: six tests pass. Copy each printed p95 into the PR description: the four existing
ones, and the two "while a daemon runs" ones with their tick counts. A p95 over budget is a
failure, never a ruling. Rerun once on a quieter machine before treating it as one, and report
both runs.

- [ ] **Step 5: Linux in Docker**

Run CONTRIBUTING's recipe, with the worktree's absolute path in place of `$PWD` (the Bash
tool's working directory is not kept between calls). Run it as its own Bash call, with `docker`
first and nothing piped:

```sh
docker run --rm -v /Users/michael/Code/tagteam-daemon:/src:ro -v tagteam-target:/target -v tagteam-registry:/usr/local/cargo/registry -e CARGO_TARGET_DIR=/target -e RUSTUP_TOOLCHAIN=1.88.0 -w /src rust:1.88 bash -c 'useradd -m -u 1000 tester && chown -R tester /target /usr/local/cargo && setpriv --reuid=1000 --regid=1000 --clear-groups env HOME=/home/tester USER=tester cargo test --workspace --locked --no-fail-fast --features tagteam/test-support'
```

Expected: every `test result:` line says `0 failed`. Sum them as in Step 2 from the output, and
report it as `passed/total (Linux)`. A failure here that macOS does not show (`SO_PEERCRED`,
the systemd unit's rendering, `notify-send` detection, `/proc`) is fixed in this branch, with a
test that runs on Linux.

- [ ] **Step 6: Ask Michael before any manual check**

Send one message, then wait for his answer:

> The daemon's three manual checks (spec D15) and its real service round trip (D14) are next.
> Each is optional, and I run only what you approve:
> 1. **D-R1, notifications from a LaunchAgent.** I load a throwaway agent,
>    `io.github.michaelrigart.tagteam-check-dr1`, that runs `osascript` once to show a
>    notification titled "tagteam". It runs no tagteam and touches no account. You tell me
>    whether it showed, and whether macOS asked for permission first. Then I remove it.
> 2. **D-R2, a locked login keychain.**
>    (a) You lock your login keychain yourself (`security lock-keychain ~/Library/Keychains/login.keychain-db`).
>    I load a throwaway agent that runs one `security find-generic-password -w` for Claude
>    Code's item, with the secret sent to `/dev/null`, and records only its exit code. You
>    tell me whether a dialog appeared; if one does, press Cancel rather than typing your
>    password. Then you unlock.
>    (b) Optional: the same with the real daemon installed over your accounts, with
>    `daemon.autoswitch` set to false first, so it only collects. I watch its log for the
>    keychain backoff.
> 3. **D-R3, Keychain access from a LaunchAgent.**
>    (a) A throwaway agent reads Claude Code's item and one `tagteam` vault item (secrets to
>    `/dev/null`, exit codes only). It also adds and then deletes a throwaway item,
>    `tagteam-check-dr3`.
>    (b) Optional: the real daemon installed over your accounts, left running until it has
>    made a switch and a token refresh on its own. Or, if you prefer, you lower
>    `autoswitch.threshold` for one tick to make it switch now. This switches your live login
>    unattended.
>
> 4. **D14's real service round trip** (`a_real_service_manager_starts_and_stops_the_daemon`).
>    It installs the daemon's real launchd agent, under its real label, starts it, checks that
>    it answers, and uninstalls it. The daemon it starts serves an empty fixture store, never
>    your accounts. It refuses to run while you have the daemon installed.

Run only the parts he approves. Record a declined part as "not run (declined)".

- [ ] **Step 7: D-R1, a notification from a LaunchAgent**

Make the scratch directory and print its path (use the printed path below; a variable does not
survive between Bash calls):

```sh
mkdir -p "$TMPDIR/tagteam-checks" && cd "$TMPDIR/tagteam-checks" && pwd -P
```

Write the agent, with `CHECKS` replaced by the printed path. Its argv is exactly D6.2's:

```sh
cat > CHECKS/dr1.plist <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>io.github.michaelrigart.tagteam-check-dr1</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/bin/osascript</string>
    <string>-e</string><string>on run argv</string>
    <string>-e</string><string>display notification (item 2 of argv) with title (item 1 of argv)</string>
    <string>-e</string><string>end run</string>
    <string>--</string>
    <string>tagteam</string>
    <string>D-R1 check: this notification came from a launchd agent</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>CHECKS/dr1.out</string>
  <key>StandardErrorPath</key><string>CHECKS/dr1.err</string>
</dict>
</plist>
EOF
plutil -lint CHECKS/dr1.plist
```

Load it (outside the sandbox if `launchctl` is refused inside):

```sh
launchctl bootstrap gui/$(id -u) CHECKS/dr1.plist
```

Ask Michael: did a notification titled "tagteam" appear? Which app did it name? Did macOS ask
for permission first, and in what words? If it asked, and he allowed it, kick it again with
`launchctl kickstart gui/$(id -u)/io.github.michaelrigart.tagteam-check-dr1` and ask again.

Read the result, then remove the agent:

```sh
launchctl print gui/$(id -u)/io.github.michaelrigart.tagteam-check-dr1 | rg -n 'last exit code|state'
cat CHECKS/dr1.out CHECKS/dr1.err
launchctl bootout gui/$(id -u)/io.github.michaelrigart.tagteam-check-dr1
rm -f CHECKS/dr1.plist CHECKS/dr1.out CHECKS/dr1.err
```

D-R1 passes when the notification showed, at once or after the one permission grant. It fails
when it never showed, even with Script Editor allowed in System Settings, Notifications. Then
Step 9 applies.

- [ ] **Step 8: D-R2 and D-R3 with throwaway agents, then the real daemon**

D-R2 (a). Write `CHECKS/dr2.plist` (unquoted heredoc: `\$` keeps the agent's own `$`):

```sh
cat > CHECKS/dr2.plist <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>io.github.michaelrigart.tagteam-check-dr2</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/sh</string>
    <string>-c</string>
    <string>/usr/bin/security find-generic-password -s 'Claude Code-credentials' -w &gt;/dev/null 2&gt;"\$1"; echo "rc=\$?" &gt;"\$2"</string>
    <string>sh</string>
    <string>CHECKS/dr2.err</string>
    <string>CHECKS/dr2.rc</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Background</string>
</dict>
</plist>
EOF
plutil -lint CHECKS/dr2.plist
```

Michael locks the login keychain himself. Then:

```sh
launchctl bootstrap gui/$(id -u) CHECKS/dr2.plist
```

Ask: did a dialog appear (SecurityAgent, asking for the keychain password)? If one did, he
presses Cancel. Then `cat CHECKS/dr2.rc CHECKS/dr2.err`, and record the rc. Michael unlocks the
keychain. Run the agent again unlocked with
`launchctl kickstart gui/$(id -u)/io.github.michaelrigart.tagteam-check-dr2`, and record that
rc (expected `rc=0`). Remove it with
`launchctl bootout gui/$(id -u)/io.github.michaelrigart.tagteam-check-dr2 && rm -f CHECKS/dr2.*`.

How to read it, against D4.1's counters (plan A Task 4):
- `rc=36` with no dialog: the read counts as *locked*, and the backoff works as specified.
- A dialog: an unattended daemon would raise one at every backoff step. Record it as an open
  item for Michael, with the rc after Cancel. D15 D-R2 then requires the backoff to be
  revisited before release.
- Any other rc with no dialog: the counters do not class it as locked. Record the rc; this is
  an open item too.

D-R3 (a). Write `CHECKS/dr3.plist` the same way, with this script string, and run it on an
unlocked keychain:

```text
/usr/bin/security find-generic-password -s 'Claude Code-credentials' -w &gt;/dev/null 2&gt;&gt;"\$1"; echo "read-cc rc=\$?" &gt;"\$2"; /usr/bin/security find-generic-password -s tagteam -w &gt;/dev/null 2&gt;&gt;"\$1"; echo "read-vault rc=\$?" &gt;&gt;"\$2"; /usr/bin/security add-generic-password -U -s tagteam-check-dr3 -a check -w throwaway 2&gt;&gt;"\$1"; echo "write rc=\$?" &gt;&gt;"\$2"; /usr/bin/security delete-generic-password -s tagteam-check-dr3 -a check &gt;/dev/null 2&gt;&gt;"\$1"; echo "delete rc=\$?" &gt;&gt;"\$2"
```

with label `io.github.michaelrigart.tagteam-check-dr3` and files `CHECKS/dr3.err` and
`CHECKS/dr3.rc`. Expected: four `rc=0` lines, and no dialog. Remove the agent and its files.

D-R2 (b) and D-R3 (b), only as approved:
1. `tagteam config set daemon.autoswitch false` (for D-R2 b only), then
   `tagteam daemon install`. Check `tagteam daemon status --json`: `running: true`.
2. D-R2 (b): Michael locks the keychain. After at least two minutes,
   `tagteam daemon status --json` shows `providers[0].backoff` as `{"reason": "keychain", …}`.
   Check `rg -n "backoff" ~/.local/state/tagteam/tagteam.log | tail -n 5`, and ask whether
   any dialog appeared. Michael unlocks, and the backoff ends at the next run (status shows
   `backoff: null`; the log says it ended).
3. D-R3 (b): `tagteam config set daemon.autoswitch true`. Wait for a switch and an active-token
   refresh in the log, or for the switch Michael forces by lowering `autoswitch.threshold` for
   one tick (then `tagteam config unset autoswitch.threshold`). Check
   `rg -n "switch|refresh" ~/.local/state/tagteam/tagteam.log | tail -n 20`: no Keychain
   error. `tagteam list` shows the new live account.
4. Ask Michael whether to keep the daemon installed. If not: `tagteam daemon uninstall`. Either
   way, restore `daemon.autoswitch` to what he wants.

D14's round trip, if approved (Task 5's ignored test), outside the sandbox:

```sh
cargo test -p tagteam --features test-support --test daemon_service_cli -- --ignored --exact a_real_service_manager_starts_and_stops_the_daemon
```

Expected: PASS, and afterwards `launchctl print gui/$(id -u)/io.github.michaelrigart.tagteam`
finds nothing loaded. Run it before D-R2 (b) and D-R3 (b) install the daemon for real, or
after they uninstall it: it refuses while a unit is installed.

- [ ] **Step 9: Only if D-R1 failed: macOS notifications wait for the menu bar app**

D15 D-R1's mitigation. Make these changes in one commit.

In `crates/tagteam-daemon/src/notify.rs`, `Backend::detect` gives no backend on macOS. Keep
the `Osascript` variant for the menu bar app, and keep the Linux branch's `notify-send` lookup
exactly as Task 11 wrote it; only the macOS branch and the doc comment change:

```rust
    /// D6.2 as amended after D15 D-R1: no backend on macOS. `osascript` run by a launchd agent
    /// showed no notification on macOS 27, so macOS notifications wait for the menu bar app
    /// (sub-project 4). Linux uses `notify-send` when it is on `PATH`.
    pub fn detect(macos: bool, path_var: Option<&std::ffi::OsStr>) -> Option<Backend> {
        if macos {
            return None;
        }
        find_on_path("notify-send", path_var).map(Backend::NotifySend)
    }
```

In `notify.rs`'s tests, plan A Task 11's
`macos_always_has_osascript_and_linux_has_notify_send_only_on_path` opens with:

```rust
        // D6.2: macOS's own program, never one on PATH; `notify-send` only where PATH has it.
        assert_eq!(Backend::detect(true, None), Some(Backend::Osascript));
```

Replace those two lines with:

```rust
        // D6.2 after D15 D-R1: no backend on macOS until the menu bar app; `notify-send` only
        // where PATH has it.
        assert_eq!(Backend::detect(true, None), None);
        assert_eq!(
            Backend::detect(true, Some(std::ffi::OsStr::new("/usr/bin:/bin"))),
            None
        );
```

and rename the test `macos_has_no_backend_and_linux_has_notify_send_only_on_path`. No other
plan A test calls `Backend::detect(true, …)`; `rg -n 'detect\(true' crates` confirms it.

In `crates/tagteam-engine/src/doctor.rs`, `daemon.notify` on macOS with notifications on
reports `info` with the message `macOS notifications are not available yet; they come with the
menu bar app` and no fix, instead of the no-backend `warn`, which stays for Linux. Update Task
6's macOS `daemon.notify` snapshot to that status and message.

In `docs/superpowers/specs/2026-10-14-tagteam-daemon-design.md`, D6.2. Before:

```markdown
- **macOS:** `/usr/bin/osascript -e 'on run argv' -e 'display notification (item 2 of argv) with title (item 1 of argv)' -e 'end run' -- <title> <body>`.
  The text travels as argv, so no account name can inject AppleScript. The notification shows
  as coming from Script Editor, which needs notification permission once (D15).
```

After:

```markdown
- **macOS:** no backend yet. `osascript` run by a launchd agent showed no notification on
  macOS 27 (D15 D-R1), so macOS notifications wait for the menu bar app (sub-project 4). The
  foreground daemon does not notify either, so it never differs from the service (D6.4).
```

D10's table. Before:

```markdown
| `daemon.notify` | warn: notifications are on but there is no backend (`notify-send` missing) |
```

After:

```markdown
| `daemon.notify` | warn: notifications are on but there is no backend (`notify-send` missing). info (macOS): notifications wait for the menu bar app (D15 D-R1) |
```

D15's D-R1 row: add to its Mitigation cell `Checked <date>: they did not show, so the
fallback applies (D6.2).`, with the date of the check.

In `README.md`, the Daemon section's notification paragraph. Before:

```markdown
On macOS they come from `osascript` and show as Script Editor, which asks once for permission
to send notifications. On Linux they need `notify-send`. Turn them off with
```

After:

```markdown
On Linux they need `notify-send`. macOS notifications are not available yet; they will come
with the menu bar app. Turn them off with
```

In `CHANGELOG.md`, the notifications bullet ends `through `notify-send` on Linux (macOS
notifications come later, with the menu bar app).` in place of `through `osascript` on macOS
and `notify-send` on Linux.`

Run, outside the sandbox: `cargo test -p tagteam-daemon --features test-hooks`,
`cargo test -p tagteam-engine --test doctor`, fmt and both clippy runs. Expected: PASS.

Commit (the controller commits): `git commit -m "Turn macOS notifications off until the menu bar app"`.
Steps 1 and 2 then run again, because commits after verification need it.

- [ ] **Step 10: Record the results**

Write the results into the PR description (the Draft PR's body with
`gh pr edit <n> --body-file <file>` once it is open; until then, the draft the controller
keeps):
- **How to verify** (the template's section): Steps 1–5 with their totals as passed/total,
  macOS and Linux; the six p95 values; each manual check's outcome, or "not run (declined)".
  Tick a template checkbox only for what was done.
- **Rulings**: every ruling from both plans' runs, any name Task 9 took from the merged token
  report, and, if Step 9 ran, D-R1's fallback.
- **Before the next release**: Task 8's crates.io note. If Task 9 has not run because the
  token report has not merged, add: "Task 9 of plan B (the token report's daemon jobs) waits
  for the token report to merge; it runs after `daemon` is rebased onto it."
- **Open items**: any D-R2 dialog or unclassified rc, for Michael to decide.

End the task's report with the state line: pushed or not, the review receipt, open items, next
step.
