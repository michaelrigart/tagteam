# tagteam daemon A — Engine Hooks, Scheduler and the Daemon Process Implementation Plan

**Status:** Implemented — https://github.com/michaelrigart/tagteam/pull/17

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `tagteam daemon` runs in the foreground. It schedules the usage collector and the
auto-switch loop per provider, pauses through `daemon.autoswitch`, backs off on a locked
Keychain, watches the store and its own binary, and sends notifications. `tagteam auto` runs on
the same scheduler, unchanged in behaviour. Plan B (`2026-10-14-tagteam-daemon-b-socket-service.md`)
adds the socket, its client, the service units, `doctor` and the docs.

**Architecture:**
- **`tagteam-provider`:** Keychain secret reads are counted by outcome (Task 4).
- **`tagteam-engine`:**
  - settings that reload in place (Task 1)
  - `AutoEngine`'s source, and the lock record it empties on drop, plus a reusable quarantine
    watch (Task 2)
  - `data_version`, the live account's relevant reading, a named-lease call, the spawner
    accessor, and the daemon's paths, lock record and socket-path bound (Task 3)
  - three settings keys (Task 5)
- **`tagteam-daemon`, a new library crate:**
  - the protocol's event JSON and the event types (Task 6)
  - the scheduler, with `tagteam auto` moved onto it (Task 7)
  - the provider job and its modes (Task 8), and the keychain backoff (Task 9)
  - the store and upgrade watches (Task 10)
  - the notifier (Task 11)
  - daemon start-up and serving (Task 12)
- **`tagteam` (CLI):** `auto` delegates to the crate (Task 7), and `tagteam daemon` runs it
  (Task 12).

**Tech Stack:** Rust (edition 2024, toolchain 1.88, MSRV 1.85), std threads and channels, no
async runtime. Libraries: `rusqlite` (bundled), `serde_json`, `libc`, `tracing`, `fastrand`,
`thiserror` and `uuid`. Tests use `tempfile`, `tracing-subscriber`, `FakeKeychain`,
`FileKeychain`, `FakeClock`, `ScriptedHttp`, `ScriptedSpawner`, `MockServer`, `assert_cmd` and
the existing `tests/common` fixtures. Every one is a workspace dependency already: no new
external crate.

**Spec:** `docs/superpowers/specs/2026-10-14-tagteam-daemon-design.md` (the daemon spec, signed
off at `96b9cea`; sections are cited as D1–D16). Before starting any task, read D3, D4, D6, D7,
D8.1, D11, D12 and D14. It builds on the core spec,
`docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`, whose sections are cited as
§N: §4.2–§4.4, §6.1, §6.4, §8.3, §8.6, §11, §12.6, §13.5, §14.1 and §14.2.

## Execution notes

- **Worktree.** Run this plan in `~/Code/tagteam-daemon` (Michael's `wt` worktree, branch
  `daemon`). Plan B follows it in the same worktree, and both go into one Draft PR.
- **Status.** When execution starts, set this plan's `**Status:**` to `In progress` in one
  commit, and the spec's too.
- **Flags.** Clippy must pass both with `--features tagteam/test-support` and with no
  features. Binary tests are gated `#![cfg(feature = "test-support")]` and run with
  `cargo test -p tagteam --features test-support`.
- **The daemon crate's `test-hooks` feature** (`= ["tagteam-engine/test-hooks"]`, Task 7) builds
  the engine's crash and cancellation points for its tests, and `tagteam/test-support` turns it
  on. A daemon-crate test that needs an engine hook is gated `#[cfg(feature = "test-hooks")]`.
  Every full run of the crate's tests, from Task 7 on, is
  `cargo test -p tagteam-daemon --features test-hooks`; a run without the feature skips those
  tests.
- **One test fixture for the daemon crate:** `crates/tagteam-daemon/src/testutil.rs`
  (`#[cfg(test)] mod testutil;`). Task 7 creates it; Tasks 8, 10 and 11 extend it; every
  daemon-crate test imports from `crate::testutil`. A helper is added in the task whose tests
  first use it, so no task's build carries dead test code.
- **Formatting.** Every task runs `cargo fmt --all` before `cargo fmt --all --check`.
- **Isolation (§15.1).** Tests never reach the real network, the real HOME or the login
  keychain.
- **New crate.** `tagteam-daemon` needs no new external crate. If a sandbox refuses cargo's
  registry, run `cargo fetch --locked` once outside it.
- **Sandbox.** Claude Code's macOS sandbox refuses `openpty`, `bind` on a Unix socket, and
  writes under `/tmp`. Run every command sandboxed first, and run it again outside only when it
  failed with sandbox evidence (`Operation not permitted`, `EPERM`):
  - The pty tests always need it: `cargo test -p tagteam --features test-support --lib
    prompt::tests` and `cargo test -p tagteam --features test-support --test signals`, and
    `xtask`'s process-group tests. Tasks 7 and 13's workspace runs include them.
  - The tests that spawn `tagteam daemon` and signal it may need it:
    `cargo test -p tagteam --features test-support --test daemon_cli` (Task 12) and
    `cargo test -p tagteam --features test-support --test logging every_command` (Task 13).
  - Plan A binds no socket. Plan B's socket tests always need it, and bind under a short
    directory, because macOS's `$TMPDIR` can exceed `sun_path` (`MAX_SOCKET_PATH`, Task 3).

## Decisions

Rulings made while planning. Each names what it would cost if wrong.

1. **Crate layout.** `crates/tagteam-daemon/src/`:
   - `lib.rs`
   - `events.rs`: `DaemonEvent` and its sinks
   - `protocol.rs`: event JSON, and in plan B the requests and replies
   - `scheduler.rs`
   - `auto.rs`: `tagteam auto`'s jobs and entry points, moved from the CLI
   - `provider_job.rs`
   - `backoff.rs`: the keychain backoff
   - `watch.rs`
   - `notify.rs`
   - `status.rs`
   - `daemon.rs`: start and serve
   - `testutil.rs`: the crate's shared test fixture, `#[cfg(test)]` only

   Plan B adds `server.rs` and `client.rs`. Cost if wrong: module moves.
2. **What lives in the engine, not the daemon crate:** the daemon's paths and lock record
   (`tagteam_engine::daemon`, Task 3), with the longest socket path `sun_path` holds
   (`MAX_SOCKET_PATH`, which plan B's server and `doctor` both check), and in plan B the
   service units (`tagteam_engine::service`). `doctor` lives in the engine and must read them,
   and the engine cannot depend on the daemon crate. `MAX_SOCKET_PATH` is a `cfg(target_os)`
   constant rather than computed from `sockaddr_un`, because the engine forbids `unsafe`. Cost
   if wrong: a module moves crates.
3. **`Engine::settings()` returns `Arc<Settings>`.** The field becomes
   `RwLock<Arc<Settings>>`. `reload_settings(&self, provider)` re-reads `config.toml` for that
   provider's table (§6.4) and returns its warnings. The scheduler never calls it: on an mtime
   change it only calls each job's `settings_changed` (Task 7), so `tagteam auto` keeps today's
   behaviour. The daemon reloads for the provider the command read its settings for
   (`--provider`'s, else `engine.default_provider()`, as the CLI's `command_settings` chooses
   it): once in `Daemon::start`, right after it reads `config.toml`'s mtime (Decision 9), and
   then in its private settings job's `settings_changed` (Task 12). It drops the warnings,
   because each provider job reads its own provider's `Settings` (Decision 8) and emits that
   read's warnings as `config-warning` events; a second emitter would print every warning twice.
   Cost if wrong: call sites that hold a borrow (`&self.settings().models`) change to bind the
   `Arc` first, which is mechanical.
4. **The source is a builder.** `AutoEngine::with_source(self, source: &'static str)` sets
   it, and `Engine::auto` keeps its signature with the source defaulting to `"auto"`. The
   source reaches `release_unbound_quarantines`, `settle_or_refuse_as` and every
   `SwitchRequest` the engine makes. This keeps the engine's many `engine.auto(..)` test calls
   unchanged. Cost if wrong: one parameter added later.
5. **`impl Drop for AutoEngine`** empties the engine lock file (`set_len(0)` through a second
   descriptor, as `write_holder` writes it) while the guard is still held. `Drop::drop` runs
   before the fields drop, so the lock is still held. An I/O error is logged at WARN and
   otherwise ignored. A dry-run engine holds no lock and does nothing. Cost if wrong: `doctor`
   reports a live daemon as the engine of a provider it paused.
6. **Quarantine reporting is shared.** `AutoEngine`'s `seen`, `report_quarantines` and
   `note_quarantined` move into `pub struct QuarantineWatch` (engine `auto.rs`), which the
   tick and the daemon's `collect` round both use (D4.3). Behaviour is unchanged, and the
   engine's tick tests pin that. The type is shared, not an instance: `AutoEngine` keeps its
   own, and a provider job starts a fresh one each time it enters `collect` (Task 8). A
   quarantine another process sets or clears between the last tick and the first `collect`
   round is therefore not reported; the next comparison sees the account as it is. Cost if
   wrong: duplicated logic in the daemon crate, or, for the gap, an accessor for the engine's
   watch.
7. **Keychain read counts are per instance.** `ReadCounts` (atomics) belongs to each
   `Keychain` instance (`SecurityCli`, `FakeKeychain`, `FileKeychain`), exposed through
   `Keychain::read_counts(&self) -> Option<Arc<ReadCounts>>`, which defaults to `None`. They
   are not process globals, because engine tests run in parallel threads of one process. The
   CLI hands the daemon the counts of `ctx.keychain`, and of `ctx.vault_keychain` when that is
   a different instance. Cost if wrong: a flaky test, or a missed count.
8. **Each provider job reads its own settings.** When it is built, and on every reload, it
   runs `Settings::load(env, provider)` and derives its `AutoConfig` with
   `auto_config(&settings, &AutoFlags::default(), long_window)`, `daemon.autoswitch` and
   `notify.events`. `notify.enabled` is global and read by the notifier. The daemon passes no
   flags. Every read's warnings are the job's to emit as `config-warning` events: a reload's
   before its evaluation, and the first read's at the start of the job's first run, before any
   mode work (a reload before the first run supersedes them). The command printed its own
   read's warnings, but `config.toml` may change between that read and the job's (an unlock
   offer can wait on a person), and the scheduler's mtime baseline is taken after it, so no
   reload would ever report a file corrupted in between: it would resume switching
   (`daemon.autoswitch` reads true) without a word. A warning from a global key reaches every
   provider's read, so it is emitted once per provider, as a reload's are and as `auto`'s are.
   The engine-level reads (`Daemon::start`'s and the settings job's `reload_settings`) drop
   their warnings, since these job reads cover them. `tagteam auto` keeps today's behaviour:
   its jobs' first read (`configured`) reports nothing, and the command's print stands. Cost if
   wrong: a file already bad when the command started is reported twice in the human output,
   by the command's `warning:` line and by each job's `config-warning`.
9. **The scheduler checks `config.toml`'s mtime in every slice and before every job**
   (D4.2). For `tagteam auto` a reload can therefore happen during a sleep rather than just
   before the tick. The reload still precedes the next tick, so what a tick decides is
   unchanged. One thing moves: a reload's `config-warning` now comes in the slice after the
   write, not just before the next tick. The moved test
   `changed_settings_are_read_again_before_the_next_tick_and_flags_still_win` therefore changes
   one expectation, the warning's time, from 60 s to 1 s (Task 7). Every other moved test is
   unchanged apart from its feature gate. The first check uses the mtime the caller read before
   its own settings read (`Scheduler::with_settings_mtime`), so a write landing in between is a
   change, and without one the first check tells no job. The daemon's engine read its settings
   when the command built it, before `Daemon::start` reads the mtime and before any unlock
   offer, which can wait on a person; `start` therefore reads them again right after the mtime
   (Decision 3), so no settings read the daemon serves with comes before its baseline. Cost if
   wrong: that one test's expectation about when warnings print.
10. **`collect` releases no quarantine.** It only reports changes (D4.3). Releasing stays with
    the tick (step 1) and the switch. Cost if wrong: a re-logged-in account stays quarantined
    while paused until a `switch`, an `auto` tick or the next unpause, which is also what
    `list` does today.
11. **A purged or replaced store.** The store watch also compares the store file's device and
    inode every 2 s. When they changed, or the file vanished, it calls
    `Engine::forget_store()` (Task 3), which drops the cached connection so the next use
    reopens, and emits `changed`. `data_version` compares only on one connection, so the new
    connection's first value is a baseline, never compared with one read before. This is the
    store-watch's job as D4.4 defines it ("another process committed"), extended to a store
    that another process replaced. The watch's first run compares too, never only records:
    `Daemon::start` takes its baseline (`StoreBaseline::take`, Task 10) once `daemon.lock` is
    held and before any provider job uses the store. It forgets the engine's connection and
    reopens the store between two looks at the file's device and inode, retrying while they
    differ, so the identity it records is the file the engine's connection has open. A store
    replaced, or committed to, between start and the watch's first run is then a change at that
    run. **Invariant: the first pass's store check precedes all provider work.** `serve` adds
    the store watch (and the upgrade watch) before every provider job, all due at once, and the
    scheduler runs a tie in the order added (Task 7 pins it), so no tick or `collect` round runs
    before the watch has compared with the baseline and, on a move, forgotten the connection.
    The one provider action that can come earlier is the scheduler's settings check, when
    `config.toml` changed between start and serving: a provider job's `settings_changed`
    evaluates its mode then, which only reads the store and takes or drops the engine lock
    (`Engine::auto` writes no store row); its work waits for its run, after the watches. Cost
    if wrong: a daemon in `collect` mode writes into an unlinked file after `tagteam purge`
    until it restarts.
12. **Status is a snapshot board.** `StatusBoard` (`Mutex<StatusSnapshot>`, Task 8) is written
    by the scheduler's jobs after each run. Plan B's server reads it, and fills in
    `subscribers` itself. Every time on the board is epoch seconds, as D5.4 prints them; the
    scheduler's milliseconds are divided (`div_euclid(1000)`) when a job writes its line. Cost
    if wrong: none for plan A.
13. **Start and serve are two calls.** `Daemon::start` covers the refusals, the
    `daemon.lock` wait, the Keychain decision, and the store probe with the store watch's
    baseline (Decision 11). `Started::serve` runs the scheduler. Plan B binds the socket
    between them, with no change to plan A's code (D4.1 steps 1–6). `serve` empties
    `daemon.lock`'s record and lets the lock go before it returns, so a successor daemon may
    start while plan B's server is still shutting down; plan B's shutdown must never remove a
    socket it did not bind. Cost if wrong: plan B edits `daemon.rs`.
14. **The terminal's Keychain check stays in the CLI.** On a terminal, `tagteam daemon` runs
    `App::ensure_unlocked` (Appendix A.3's check, and its unlock offer) before
    `Daemon::start`, exactly as `auto` does, and a declined or failed unlock exits 1 (D4.1
    step 3, D8.1). Without a terminal it runs no check, and the daemon crate never calls
    `lock_state`. The check therefore comes before the `daemon.lock` wait rather than after
    it; nothing depends on that order. The program's stamp comes before both (Decision 17).
    Cost if wrong: one reordering in `App::daemon`.
15. **A signal before serving is an interruption.** The `daemon.lock` wait is a cancellation
    point (§14.1). A signal during it ends `Daemon::start` with `EngineError::Interrupted`,
    which the CLI reports as it reports any interrupted command: exit 128 + n, with the
    `interrupted` error. D8.1's "0 after a signal" applies once the daemon serves, because only
    a serving daemon has D4.6's stop (a `stopping` event, the engine locks let go).
    `Command::Daemon` joins `auto`'s `stops_on_a_signal`, so a run a signal ends never prints
    the "too late" notice. Cost if wrong: a service manager stopping a daemon that is still
    waiting sees exit 143 rather than 0; the fix is one match arm in `App::daemon`.
16. **The limit notice's text.** D6.2's examples are illustrative. The `limit` notice reads
    `2 · a: 7d at 93% (resets in 3d09h)`. It leaves out the example's "no account to switch
    to": `limit_notice` is not told why the run did not switch (paused, cooldown, every account
    exhausted…), and that phrase is true for only some of those reasons. Durations are the
    CLI's (`tagteam_core::rank::span`, `1d04h`), as `list` prints them, not the example's
    `1d 4h`. A binding window whose reset has already passed is not notified. Cost if wrong:
    one `format!` in `limit_notice` and its tests' expected strings.
17. **The upgrade watch's baseline is taken first, by the CLI.** D4.5 records the program
    file "at start-up". `tagteam daemon` takes that stamp (`ProgramStamp::take`, Task 10) as
    the first statement of `App::daemon`: before the run-shell refusal, the unlock offer
    (Decision 14) and the `daemon.lock` wait, any of which can last minutes or hours. It rides
    in `DaemonOptions::program` through `Started` into `serve`, and the upgrade watch compares
    every run, its first included, with it; the watch never stats the file when it is built.
    A tagteam installed while an old daemon waited for another to stop is therefore an upgrade
    at the old daemon's first watch run, never that daemon's own baseline. `Daemon::start`
    could not take it as early, since the CLI runs the unlock offer before calling it. Cost if
    wrong: the few milliseconds between `exec` and that statement remain a window in which a
    replacement goes unseen.

## Global Constraints

- One binary: `daemon` is a subcommand of `tagteam`. `tagteam-daemon` is a library on top of
  `tagteam-engine` (D3, §4.1). It names no provider's paths, keys or endpoints.
- No async runtime; threads and blocking I/O only (D4, §4.4).
- No new business logic: the daemon schedules `AutoEngine::tick` and `Engine::collect_usage`
  as they are (D1). No store migration (D3).
- The usage budget of 20 requests per identity per rolling hour holds across all processes
  (§8.6). The daemon collects only through `collect_usage`.
- `statusline` stays within 10 ms p95 and `list` within 50 ms p95 with a daemon running
  (D1.1, criterion 3). Plan B's perf task measures it.
- Signals: SIGINT, SIGTERM and SIGHUP set the cancel token, and work stops at cancellation
  points, never inside a critical span (§14.1). The daemon exits 0 after a signal, 75 to be
  restarted, 1 when it cannot start, and 2 for a usage error (D8.1). A signal that comes while
  it still waits for `daemon.lock` is an interruption, 128 + n (Decision 15).
- Logging (§14.2): one shared log file. Accounts appear as `account=<id> position=<n>`. Never
  log an email, label, organization name, token or credential. Notifications and the socket
  are not logs, but they carry no credential either (D6.2, D5.1).
- §3: the daemon writes CC state only through the operations that already may.
- Settings follow §6.4: forgiving reads, strict `set`, provider overrides. The new keys are
  `daemon.autoswitch` (bool, default true, per provider), `notify.enabled` (bool, default
  true, global) and `notify.events` (list of `switch`, `quarantine`, `limit`, default all
  three, per provider) (D7).
- Mode delays: re-evaluated at least every 60 s. `collect` runs at
  `interval × U(0.9, 1.1)`, shortened to the earliest `next_poll_at` and floored at 60 s. The
  keychain backoff starts at 60 s and doubles to at most 900 s. The store watch runs every 2 s
  and the upgrade watch every 60 s. The `daemon.lock` wait retries every 5 s (D4).
- Exit 75 means "restart me on the new binary or schema" (D4.5).

## Review Focus

These input classes and failure modes are implied by the spec but no task's main tests
exercise them. Each line names its test, which the owning task adds.

1. **A corrupt `config.toml` while paused.** Forgiving reads (§6.4) bring back the defaults,
   so `daemon.autoswitch` reads true and switching resumes. A person expects to be told:
   each reload's warning reaches the sink as `config-warning`, and the job leaves `collect`
   for `auto` only with that warning emitted. Test:
   `a_corrupt_settings_file_resumes_switching_with_a_warning` (Task 8).
2. **The store purged or replaced under a running daemon** (`tagteam purge` while paused). The
   daemon must reopen the new store, never keep writing into the unlinked one. Test:
   `a_replaced_store_file_is_reopened_and_reported_as_changed` (Task 10).
3. **A fresh machine:** no data directory when the daemon starts, then a first `tagteam add`.
   Every provider is `idle`, then the store appears, `changed` is emitted, and the provider
   moves to `collect`. Test: `a_store_that_appears_later_moves_the_provider_out_of_idle`
   (Task 10).
4. **A suspend across a backoff or a watch.** Wall-clock deadlines mean a machine that wakes
   past them runs each due job once, never one run per missed period. Test:
   `a_suspend_runs_each_overdue_job_once_on_waking` (Task 10).
5. **Two providers, one paused** through `provider.<id>.daemon.autoswitch false`. Only that
   provider leaves `auto`; the other keeps its engine lock and ticks. Test:
   `a_provider_table_pauses_only_its_provider` (Task 8).

## File Structure

| File | Task | Responsibility |
|---|---|---|
| `crates/tagteam-engine/src/engine.rs` | 1, 3, 12 | `settings: RwLock<Arc<Settings>>`, `settings()`, `reload_settings`; `data_version`, `forget_store`, `store_file`, `spawner()`; `provider_ids` |
| `crates/tagteam-engine/src/{views,profiles}.rs` | 1 | The two call sites of `settings()` that kept a borrow bind the `Arc` |
| `crates/tagteam-engine/src/auto.rs` | 2 | `with_source`, `provider()`, `Drop`, `QuarantineWatch`, `pub fn engine_holder` |
| `crates/tagteam-engine/src/views.rs` | 3 | `LiveReading` and `Engine::live_reading` |
| `crates/tagteam-engine/src/store/{mod,usage}.rs` | 3 | `Store::data_version`, `Store::take_lease` |
| `crates/tagteam-engine/src/daemon.rs` (new), `lib.rs` | 3 | `daemon_lock_path`, `daemon_socket_path`, `MAX_SOCKET_PATH`, `DaemonRecord`, `write_daemon_record`, `read_daemon_record`, the `find_on_path` re-export |
| `crates/tagteam-provider/src/{keychain,security,lib}.rs` | 4 | `ReadCounts`, `ReadTally`, `Keychain::read_counts`; counting in the three keychains; the crate-root re-exports |
| `crates/tagteam-engine/src/settings.rs`, `tests/{settings,config}.rs`, `crates/tagteam/tests/config_cli.rs` | 5 | The three keys, `NotifyEvent`, `KeyKind::NotifyEvents` |
| `crates/tagteam-daemon/{Cargo.toml,src/lib.rs,src/events.rs,src/protocol.rs}` | 6 | The crate, `DaemonEvent`, sinks, `event_json`, `daemon_event_json` |
| `.github/workflows/publish-crates.yml`, `CONTRIBUTING.md`, root `Cargo.toml` | 6 | The crate loop, the crate table, the workspace dependency |
| `crates/tagteam-daemon/src/{scheduler,auto}.rs`, `Cargo.toml` | 7 | `Job`, `Scheduler`, `Sleeper`; `tagteam auto`'s jobs (moved); the `test-hooks` feature |
| `crates/tagteam-daemon/src/testutil.rs` (new, test-only) | 7, 8, 10, 11 | The crate's one test fixture: `Fx`, `Slices`, `Emitted` and their helpers |
| `crates/tagteam/src/auto.rs`, `app.rs`, `Cargo.toml` | 7 | The CLI keeps only its sinks and calls the crate |
| `crates/tagteam-daemon/src/{status,provider_job}.rs`, `auto.rs` | 8 | `StatusBoard`; `ProviderJob` and its modes; `auto::no_switch` becomes `pub(crate)` |
| `crates/tagteam-daemon/src/backoff.rs`, `provider_job.rs` | 9 | `KeychainBackoff`, and the job's use of it |
| `crates/tagteam-daemon/src/watch.rs`, `scheduler.rs` | 10 | `StoreBaseline`, `StoreWatch`, `ProgramStamp`, `UpgradeWatch`, `schema_newer`, `program_path`; the scheduler's `Wake` |
| `crates/tagteam-daemon/src/notify.rs`, `events.rs`, `provider_job.rs` | 11 | `Backend`, `Notifier`, `NotifySink` (without its echo, which Task 12 adds), `limit_notice`; `DaemonSink::notified`; `ProviderJobCtx::notices` |
| `crates/tagteam-daemon/src/daemon.rs`, `notify.rs` | 12 | `Daemon::start`, `Started::serve`, `DaemonOptions`, `DaemonExit`, `DaemonError`; `NotifySink::echoing`, beside its one caller |
| `crates/tagteam/src/{cli,app,auto,lib}.rs`, `crates/tagteam/src/daemon_cmd.rs` (new) | 12 | `Command::Daemon`, the human and JSON sinks, exit codes |
| `crates/tagteam/tests/daemon_cli.rs` (new), `tests/{app,run_shell_cli}.rs`, `tests/common/mod.rs` | 12 | Binary and in-process tests of `tagteam daemon` |
| `crates/tagteam-engine/tests/invariant.rs`, `crates/tagteam/tests/logging.rs` | 13 | §15.3's local-state and redaction pins, extended to the daemon |

## Interfaces

Every name below is fixed by this plan. A task's implementer sees only its own task, so these
are the contract between tasks. The blocks describe the code the tasks write: where a task adds
a name beyond the first skeleton, it is listed here with the task that adds it.

### `tagteam-provider` (Task 4)

```rust
// keychain.rs
#[derive(Debug, Default)]
pub struct ReadCounts { /* answered: AtomicU64, locked: AtomicU64 */ }
impl ReadCounts {
    pub fn new() -> std::sync::Arc<Self>;
    pub fn record_answered(&self);   // a secret read that returned a secret (rc 0)
    pub fn record_locked(&self);     // rc 36, or a timeout
    pub fn tally(&self) -> ReadTally;
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadTally { pub answered: u64, pub locked: u64 }
impl ReadTally {
    /// The reads made between `before` and `self` (saturating, never wraps).
    pub fn since(self, before: ReadTally) -> ReadTally;
    /// D4.1: some secret read ended locked and none was answered.
    pub fn looks_locked(self) -> bool;
}
/// Two instances' reads together, field by field (saturating): a provider job sums the
/// keychains it was handed (Decision 7).
impl std::ops::Add for ReadTally { type Output = ReadTally; }
pub trait Keychain: Send + Sync {
    // …existing methods unchanged…
    /// The counts of this instance's secret reads (`find`), when it keeps them.
    fn read_counts(&self) -> Option<std::sync::Arc<ReadCounts>> { None }
}

// lib.rs: re-exported at the crate root, beside `Keychain`
pub use keychain::{FakeKeychain, Keychain, KeychainError, LockState, ReadCounts, ReadTally};
```

Counting happens in `find` only (D3, change 6).
- **`SecurityCli`:** rc 0 counts as answered, once the read yields `Present`; rc 36 or
  `TimedOut` counts as locked. rc 44 and other failures are not counted.
- **`FakeKeychain` and `FileKeychain`:** `Present` counts as answered, and a read refused
  because the keychain is locked (`set_locked(true)`, or a `LOCKED` file) counts as locked,
  an absent item's included (real `security` answers rc 44 there; the fakes refused it already).
  `set_unreadable` items, I/O failures, and absent items of an unlocked keychain are not counted.
- **`impl Keychain for Arc<K>`** forwards `read_counts`.

### `tagteam-engine` (Tasks 1, 2, 3, 5, 12)

```rust
// engine.rs
impl Engine {
    pub fn settings(&self) -> Arc<Settings>;                               // Task 1
    pub fn reload_settings(&self, provider: &ProviderId) -> Vec<String>;   // Task 1
    /// Task 3. None: no store yet. Per connection: a value read after `forget_store` comes from
    /// a new connection and is a new baseline, never compared with one read before.
    pub fn data_version(&self) -> Result<Option<i64>, EngineError>;
    pub fn forget_store(&self);                                           // Task 3; next use reopens
    pub fn store_file(&self) -> PathBuf;                                  // Task 3; `<data>/tagteam.db`
    pub fn spawner(&self) -> Arc<dyn ProcessSpawner>;                     // Task 3
    pub fn provider_ids(&self) -> Vec<ProviderId>;                        // Task 12; registration order
}

// auto.rs (Task 2)
impl<'e> AutoEngine<'e> {
    pub fn with_source(self, source: &'static str) -> Self;   // default "auto"
    pub fn provider(&self) -> &ProviderId;
}
impl Drop for AutoEngine<'_> { /* empties the held lock's record (Decision 5) */ }
/// Decision 6: §11.2 step 1's report, shared by the tick and the daemon's collect round.
#[derive(Debug, Default)]
pub struct QuarantineWatch { /* seen: Option<BTreeMap<AccountId, Seen>> */ }
impl QuarantineWatch {
    pub fn new() -> Self;
    /// Releases this round made, and every quarantine set or cleared since the previous call.
    pub fn report(&mut self, provider: &ProviderId, rows: &[AccountRow], released: &[AccountId], sink: &dyn EventSink);
    /// A quarantine this round's own work found, reported once.
    pub fn note_quarantined(&mut self, provider: &ProviderId, row: &AccountRow, sink: &dyn EventSink);
}
impl Engine {
    /// The holder an engine lock's record names (§11.1), read without trying the lock.
    pub fn engine_holder(&self, provider: &ProviderId) -> Read<ProcessStamp>;  // Task 2
}

// views.rs (Task 3)
#[derive(Debug, Clone, PartialEq)]
pub struct LiveReading {
    pub account: AccountId,
    pub position: u32,
    pub alias: Option<String>,
    pub email: Option<String>,
    pub label: String,
    /// The decision-grade (§8.4) windows relevant under `models` (§8.2, `is_relevant`).
    pub windows: Vec<Window>,
}
impl Engine {
    /// D3 change 5: the managed live account and its decision-grade relevant windows; None when
    /// there is no store, the live login is unmanaged or absent, or it has no decision-grade
    /// reading. An unreadable live identity is `Err(EngineError::Unreadable)`, as for a tick.
    pub fn live_reading(&self, provider: &ProviderId, models: &[String]) -> Result<Option<LiveReading>, EngineError>;
}

// store (Task 3)
impl Store {
    pub fn data_version(&self) -> Result<i64, StoreError>;
    /// §6.1's single lease statement: true when this call took or renewed `name` (its
    /// previous holder's lease had expired, or there was none). Times are epoch ms.
    pub fn take_lease(&self, name: &str, holder: &str, now_ms: i64, expires_at_ms: i64) -> Result<bool, StoreError>;
}

// daemon.rs (new, Task 3)
pub fn daemon_lock_path(env: &Env) -> PathBuf;    // <data>/locks/daemon.lock
pub fn daemon_socket_path(env: &Env) -> PathBuf;  // <data>/daemon.sock
/// D5.1: the longest socket path, in bytes, that `sockaddr_un.sun_path` holds with its NUL.
/// Plan B's server and `doctor` check a socket path against it.
#[cfg(target_os = "linux")]
pub const MAX_SOCKET_PATH: usize = 107;
#[cfg(not(target_os = "linux"))]
pub const MAX_SOCKET_PATH: usize = 103;           // macOS
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonRecord { pub pid: u32, pub start: u64, pub version: String }
/// Replaces the held lock file's content with `{"pid":N,"start":S,"version":"…"}\n`.
pub fn write_daemon_record(lock: &FlockGuard, version: &str) -> Result<(), EngineError>;
/// Absent: no file, or an empty one. Unreadable: anything that is not that shape.
pub fn read_daemon_record(path: &Path) -> Read<DaemonRecord>;
/// The first executable file named `name` in a `PATH` value (D6.2's `notify-send`, D9.1):
/// `tagteam_provider::process::find_on_path`, re-exported, so there is one copy.
pub use tagteam_provider::process::find_on_path;  // fn(name: &str, path_var: Option<&OsStr>) -> Option<PathBuf>

// settings.rs (Task 5)
pub struct Settings {
    // …existing fields…
    pub daemon_autoswitch: bool,          // daemon.autoswitch, per provider
    pub notify_enabled: bool,             // notify.enabled, global
    pub notify_events: Vec<NotifyEvent>,  // notify.events, per provider, in the order written
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NotifyEvent { Switch, Quarantine, Limit }
impl NotifyEvent {
    pub const ALL: [NotifyEvent; 3];                // D7's order, and notify.events' default
    pub fn as_str(self) -> &'static str;            // "switch" | "quarantine" | "limit"
    pub fn parse(s: &str) -> Option<Self>;          // exactly as `as_str` spells it
}
pub enum KeyKind { /* …existing… */ NotifyEvents }  // notify.events: a kind, or a list of kinds
```

### `tagteam-daemon`

```rust
// Cargo.toml: [features] test-hooks = ["tagteam-engine/test-hooks"] (Task 7);
// `tagteam/test-support` turns it on.

// events.rs (Task 6; Task 11 adds `notified`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode { Auto, Collect, Waiting, Idle }                   // as_str(): "auto" | …
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitingFor { EngineLock, Schema }                       // as_str(): "engine-lock" | "schema"
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff { pub until: i64 }                            // keychain backoff, epoch s
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason { Signal, Upgrade, Schema }                  // as_str(): "signal" | "upgrade" | "schema"
#[derive(Debug, Clone, PartialEq)]
pub enum DaemonEvent {
    Auto(AutoEvent),
    Usage { provider: ProviderId, updated: Vec<u32>, fetch_errors: BTreeMap<u32, String> },
    Changed,
    Mode { provider: ProviderId, mode: Mode, waiting_for: Option<WaitingFor>, holder_pid: Option<u32>, backoff: Option<Backoff> },
    Stopping { reason: StopReason },
}
pub trait DaemonSink {
    fn emit(&self, e: &DaemonEvent);
    fn tick_done(&self, _provider: &ProviderId) {}
    /// Task 11: a notice was queued for `provider` (D8.1's line per notification). Not an
    /// event: neither `--json` nor the socket carries it.
    fn notified(&self, _provider: &ProviderId, _notice: &Notice) {}
}
/// Every `AutoEvent` an engine emits, as `DaemonEvent::Auto`, into a `DaemonSink`.
pub struct AutoEvents<'a>(pub &'a dyn DaemonSink);
impl EventSink for AutoEvents<'_> { /* emit → Auto(e.clone()); tick_done forwarded */ }
/// A `DaemonSink` over a `tagteam auto` `EventSink`: Auto events are forwarded, others dropped.
pub struct AutoOnly<'a>(pub &'a dyn EventSink);
impl DaemonSink for AutoOnly<'_> { /* … `notified` keeps the no-op default */ }
/// Several sinks, in order.
pub struct Fanout<'a>(pub Vec<&'a dyn DaemonSink>);
impl DaemonSink for Fanout<'_> { /* each, in order; tick_done and notified too */ }

// protocol.rs (Task 6; plan B adds requests and replies)
pub const PROTOCOL: u32 = 1;
pub fn event_json(e: &AutoEvent, now_s: i64) -> serde_json::Value;          // moved verbatim from the CLI
pub fn daemon_event_json(e: &DaemonEvent, now_s: i64) -> serde_json::Value;  // D5.5's shapes

// scheduler.rs (Task 7; Task 10 adds `Wake`)
pub const SLICE_MS: i64 = 1_000;
pub trait Sleeper { fn sleep(&self, d: std::time::Duration); }
pub struct ThreadSleeper;                                   // impl Sleeper
pub fn uniform_jitter() -> f64;                             // in [-1, 1)
pub struct JobCx<'a> {
    pub engine: &'a Engine,
    pub sink: &'a dyn DaemonSink,
    pub jitter: &'a mut dyn FnMut() -> f64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ran { After(i64 /* delay ms */), Stop(StopReason) }
pub trait Job {
    fn name(&self) -> String;
    /// One run; returns its next delay, or a reason to stop the scheduler.
    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran;
    /// `config.toml` changed (D4.2); default: nothing. The scheduler calls it on every job, in
    /// the order they were added, and reloads nothing itself (Decision 3).
    fn settings_changed(&mut self, _cx: &mut JobCx<'_>) {}
}
/// Task 10, D4.3: a handle a job keeps to make other jobs due at once.
#[derive(Debug, Clone, Default)]
pub struct Wake { /* Arc<Mutex<Vec<String>>> */ }
impl Wake {
    /// Every job whose name starts with `prefix` is due before the scheduler next sleeps or
    /// picks a job; each runs once and then keeps its own cadence.
    pub fn wake(&self, prefix: &str);
}
#[derive(Default)]
pub struct Scheduler<'j> { /* slots: Vec<Slot<'j>> (job, due_ms, delay_ms), settings: Option<Option<SystemTime>>, wake: Wake */ }
impl<'j> Scheduler<'j> {
    pub fn new() -> Self;
    /// `config.toml`'s mtime as the caller read it before its own settings read, so a write
    /// landing in between is a change at the first check. Without it, the first check takes
    /// the mtime it finds and tells no job (Decision 9).
    pub fn with_settings_mtime(self, mtime: Option<std::time::SystemTime>) -> Self;
    /// Adds a job, first due at `due_ms` (epoch ms).
    pub fn add(&mut self, job: Box<dyn Job + 'j>, due_ms: i64);
    /// Task 10: the handle jobs wake each other through.
    pub fn wake(&self) -> Wake;
    /// Runs until a job returns `Stop` or the cancel token is set (→ `StopReason::Signal`).
    pub fn run(&mut self, cx: &mut JobCx<'_>, sleeper: &dyn Sleeper) -> StopReason;
}

// auto.rs (Task 7, moved from crates/tagteam/src/auto.rs with the same public names; its
// `AutoJob`, `settle` and `configured` stay private, and Task 8 makes `no_switch` pub(crate))
pub struct AutoFlags { /* unchanged */ }
pub fn auto_config(s: &Settings, flags: &AutoFlags, long_window: Option<&str>) -> AutoConfig;
pub struct AutoRun { /* unchanged */ }
pub enum AutoError { AlreadyRuns(Vec<ProviderId>), NothingToSwitch, NoLongWindow(ProviderId), Engine(EngineError) }
pub fn providers(engine: &Engine, only: Option<&ProviderId>) -> Result<Vec<ProviderId>, EngineError>;
pub fn run_loop(engine: &Engine, run: &AutoRun, sink: &dyn EventSink, sleeper: &dyn Sleeper, jitter: &mut dyn FnMut() -> f64) -> Result<i32, AutoError>;

// status.rs (Task 8). Every `i64` time here is epoch SECONDS: `DaemonInfo.started_at`,
// `ProviderStatus.last_tick_at` and `next_tick_at`, `LastOutcome.at`, `LastSwitch.at`,
// `JobStatus.last_run_at` and `next_run_at`, and `Backoff.until`. A job divides its
// milliseconds (`div_euclid(1000)`) when it writes its line (Decision 12).
#[derive(Debug, Clone, PartialEq)]
pub struct StatusSnapshot {
    pub daemon: DaemonInfo,
    pub notifications: NotifyInfo,
    pub providers: Vec<ProviderStatus>,
    pub jobs: Vec<JobStatus>,
    pub subscribers: usize,
}
#[derive(Debug, Clone, PartialEq)]
pub struct DaemonInfo { pub pid: u32, pub version: String, pub started_at: i64, pub socket: Option<PathBuf>, pub log: PathBuf }
#[derive(Debug, Clone, PartialEq)]
pub struct NotifyInfo { pub enabled: bool, pub backend: Option<&'static str>, pub events: Vec<NotifyEvent> }
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderStatus {
    pub provider: ProviderId, pub mode: Mode, pub autoswitch: bool,
    pub waiting_for: Option<WaitingFor>, pub holder_pid: Option<u32>, pub backoff: Option<Backoff>,
    pub last_tick_at: Option<i64>, pub next_tick_at: Option<i64>,
    pub last_outcome: Option<LastOutcome>, pub last_switch: Option<LastSwitch>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeKind { Switch, NoSwitch, Collect, Error }        // as_str(): "switch" | "no-switch" | "collect" | "error"
#[derive(Debug, Clone, PartialEq)]
pub struct LastOutcome { pub kind: OutcomeKind, pub reason: Option<String>, pub at: i64 }
#[derive(Debug, Clone, PartialEq)]
pub struct LastSwitch { pub at: i64, pub from: u32, pub to: u32, pub trigger: String }
#[derive(Debug, Clone, PartialEq)]
pub struct JobStatus { pub name: String, pub last_run_at: Option<i64>, pub next_run_at: Option<i64>, pub last_error: Option<String> }
pub struct StatusBoard { /* Mutex<StatusSnapshot> */ }
impl StatusBoard {
    pub fn new(daemon: DaemonInfo, notifications: NotifyInfo) -> std::sync::Arc<Self>;
    pub fn snapshot(&self) -> StatusSnapshot;
    pub fn set_provider(&self, s: ProviderStatus);   // replaces the provider's entry, kept in provider order
    pub fn set_job(&self, s: JobStatus);             // replaces the job's entry by name, else adds it last
    pub fn set_subscribers(&self, n: usize);         // plan B's server
    pub fn set_socket(&self, path: Option<PathBuf>); // plan B's server
    pub fn set_notifications(&self, n: NotifyInfo);
}

// provider_job.rs (Task 8; Task 9 adds the backoff, Task 11 the notices)
/// Every provider job's name starts with it: D5.4's `jobs` leaves them out, and the store
/// watch wakes them by it.
pub const JOB_PREFIX: &str = "provider:";
pub struct ProviderJob<'e> { /* engine, provider, ctx, settings: Settings, first_warnings: Vec<String>, cfg, auto: Option<AutoEngine<'e>>, … */ }
impl<'e> ProviderJob<'e> {
    /// Reads the provider's settings (Decision 8) and keeps that read's warnings for its first
    /// run, which emits them as `config-warning` events before any mode work.
    pub fn new(engine: &'e Engine, provider: ProviderId, ctx: ProviderJobCtx) -> Self;
}
pub struct ProviderJobCtx {
    pub board: std::sync::Arc<StatusBoard>,
    pub notices: Option<NoticeQueue>,                        // Task 11 adds it; None: no notifier
    pub keychain_counts: Vec<std::sync::Arc<ReadCounts>>,    // Task 9 reads them; empty on Linux
    pub schema_newer: bool,                                  // D4.1 step 4; Task 12 sets it from its store probe
}
impl Job for ProviderJob<'_> { /* name(): "provider:<id>" */ }
/// D4.3's table as a pure function, for its unit tests.
pub fn mode_for(i: &ModeInputs) -> (Mode, Option<WaitingFor>);
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModeInputs {
    pub schema_newer: bool, pub has_store: bool, pub switchable: usize,
    pub autoswitch: bool, pub holder_elsewhere: Option<u32>,
    /// This job tried the engine lock just now and another process held it: `waiting`, with
    /// the record's pid only when it names a live holder (D4.3).
    pub refused: bool,
    pub holds_lock: bool,
}

// backoff.rs (Task 9)
#[derive(Debug, Default)]
pub struct KeychainBackoff { /* level: u32, until_ms: Option<i64> */ }
impl KeychainBackoff {
    pub fn new() -> Self;
    /// After a run: `Some(delay_ms)` while backing off (60 s doubling to 900 s), `None` when
    /// the run ended the backoff or never started one. A run with no counted read keeps the
    /// level. The job's next work waits `max(its mode's delay, this delay)` (D4.3).
    pub fn after_run(&mut self, reads: ReadTally, now_ms: i64) -> Option<i64>;
    /// While it holds; `until` in epoch seconds, rounded up.
    pub fn current(&self) -> Option<Backoff>;
    pub(crate) fn until_ms(&self) -> Option<i64>;
}

// watch.rs (Task 10)
pub const STORE_WATCH_MS: i64 = 2_000;
pub const UPGRADE_WATCH_MS: i64 = 60_000;
/// Decision 11: what the store watch's first run compares with. `Daemon::start` (Task 12) takes
/// it once `daemon.lock` is held and before any provider job uses the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreBaseline { /* file: Option<(dev, ino)>, version: Option<i64> */ }
impl StoreBaseline {
    /// Forgets the engine's connection and reopens the store between two looks at the store
    /// file's device and inode, up to 3 tries while they differ. A store this build refuses
    /// (`UnsupportedSchema`) is no version, not an error; any other failure to open is the error.
    pub fn take(engine: &Engine) -> Result<StoreBaseline, EngineError>;
}
pub struct StoreWatch { /* board, period, wake, last file identity, last data_version, last failure */ }
impl StoreWatch {
    /// impl Job; "store-watch", every 2 s. Its first run compares with `baseline`, as every later
    /// run compares with the run before.
    pub fn new(board: Arc<StatusBoard>, baseline: StoreBaseline) -> Self;
    pub fn with_period(self, ms: i64) -> Self;
    /// After each `changed`, wakes every job whose name starts with `JOB_PREFIX` (D4.3). Task 12
    /// builds the daemon's as `StoreWatch::new(board, baseline).waking(scheduler.wake())`.
    pub fn waking(self, wake: Wake) -> Self;
}
/// D4.5, Decision 17: a program path and the file it resolved to (device, inode, size, mtime)
/// when the stamp was taken; `tagteam daemon` takes it first thing (Task 12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramStamp { /* path: PathBuf, start: Option<FileStamp> (private) */ }
impl ProgramStamp {
    /// Stats `path` now, following links. Nothing there, or metadata that cannot be read, is
    /// recorded as no file.
    pub fn take(path: PathBuf) -> Self;
    pub fn path(&self) -> &Path;
}
pub struct UpgradeWatch { /* the program's stamp, schema newer at start */ }
impl UpgradeWatch {
    /// impl Job; "upgrade-watch", every 60 s. It never stats the program when built: every run,
    /// the first included, compares with `program`'s stamp.
    pub fn new(program: ProgramStamp, schema_newer_at_start: bool, board: Arc<StatusBoard>) -> Self;
    pub fn with_period(self, ms: i64) -> Self;                    // plan B's test hook
}
pub fn program_path(argv0: &std::ffi::OsStr, path_var: Option<&std::ffi::OsStr>, current_exe: &Path) -> PathBuf; // D4.5, D9.1
/// D4.1 step 4, D4.5: the store's schema is newer than this build's, read through the engine's
/// connection, or an open that refuses it (`UnsupportedSchema`). No store is not newer.
pub fn schema_newer(engine: &Engine) -> bool;

// notify.rs (Task 11)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend { Osascript, NotifySend(PathBuf) }
impl Backend {
    pub fn detect(macos: bool, path_var: Option<&std::ffi::OsStr>) -> Option<Backend>;
    pub fn name(&self) -> &'static str;                           // "osascript" | "notify-send"
    pub fn spec(&self, title: &str, body: &str) -> SpawnSpec;     // D6.2's argv
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice { pub title: String, pub body: String }
#[derive(Clone)]
pub struct NoticeQueue { /* tx: SyncSender<Notice>, 32 deep */ }
impl NoticeQueue {
    pub fn send(&self, n: Notice);                                // never blocks: drops when full
    #[cfg(test)]
    pub(crate) fn for_test() -> (NoticeQueue, std::sync::mpsc::Receiver<Notice>);
}
pub struct Notifier { /* stop flag, thread handle */ }           // `Drop` stops it as `stop` does
impl Notifier {
    pub fn start(backend: Backend, spawner: Arc<dyn ProcessSpawner>) -> (Notifier, NoticeQueue);
    pub fn stop(self);                                           // drops what is queued
}
/// D6.1–D6.3: notices for `switch` (never a dry run) and `account-quarantined` events. It reads
/// `Settings::load(env, provider)` for each such event, so a setting changed a moment ago counts.
pub struct NotifySink<'a> { /* engine: &'a Engine, queue: NoticeQueue; Task 12 adds echo: Option<&'a dyn DaemonSink> */ }
impl<'a> NotifySink<'a> {
    pub fn new(engine: &'a Engine, queue: NoticeQueue) -> Self;
    /// Task 12, beside its one caller (`Started::serve`): each notice it queues is also
    /// announced to `to` (`DaemonSink::notified`).
    pub(crate) fn echoing(self, to: &'a dyn DaemonSink) -> Self;
}
impl DaemonSink for NotifySink<'_> {}
/// D6.1 `limit`, Decision 16's text. An unreadable live identity is no notice (logged at DEBUG).
pub fn limit_notice(engine: &Engine, provider: &ProviderId, settings: &Settings, now_ms: i64) -> Result<Option<Notice>, EngineError>;
/// What `ProviderJob::after_run_notices` delegates to: after a run that did not switch, and
/// only with a queue, the limit is judged, queued and announced to `sink`. A failure to judge
/// is logged and never fails the run.
pub(crate) fn after_run(engine: &Engine, provider: &ProviderId, settings: &Settings, queue: Option<&NoticeQueue>, sink: &dyn DaemonSink, switched: bool);

// daemon.rs (Task 12)
pub struct DaemonOptions {
    pub provider: Option<ProviderId>,
    pub version: String,
    pub program: ProgramStamp,                              // the watched path, stamped by the caller first (D4.5, Decision 17)
    pub upgrade_watch_ms: i64,                              // watch::UPGRADE_WATCH_MS; plan B's test hook shortens it
    pub keychain_counts: Vec<Arc<ReadCounts>>,
    pub backend: Option<Backend>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonExit { Signal, Restart }                     // exit 0, exit 75
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error(transparent)] Engine(#[from] EngineError),     // a signal in the daemon.lock wait: Interrupted (Decision 15)
}
pub struct Daemon;
impl Daemon {
    pub fn start<'e>(engine: &'e Engine, opts: DaemonOptions, notice: &mut dyn FnMut(&str)) -> Result<Started<'e>, DaemonError>;
}
pub struct Started<'e> { /* engine, daemon.lock (its record emptied before the lock goes), board, options (the program's stamp among them), schema_newer, settings mtime, settings provider, store baseline, notifier */ }
impl<'e> Started<'e> {
    pub fn board(&self) -> Arc<StatusBoard>;
    pub fn serve(self, sink: &dyn DaemonSink, sleeper: &dyn Sleeper, jitter: &mut dyn FnMut() -> f64) -> DaemonExit;
}

// testutil.rs (#[cfg(test)], crate-private): Task 7 creates it; Tasks 8, 10 and 11 extend it
pub(crate) const T0: i64 = 1_790_000_000;                            // Task 7
pub(crate) const FAR_MS: i64 = 4_102_444_800_000;                    // Task 7
pub(crate) const CC_USAGE: &str = "https://api.anthropic.com/api/oauth/usage";  // Task 8
pub(crate) const SHORT_RESETS: i64 = T0 + 9_630;                     // Task 11
pub(crate) const LONG_RESETS: i64 = T0 + 291_630;                    // Task 11
pub(crate) fn cc() -> ProviderId;                                    // Task 7
pub(crate) fn fake() -> ProviderId;                                  // Task 7
pub(crate) fn two(fx: &Fx) -> (AccountId, AccountId);                // Task 8, from `auto`'s tests
pub(crate) fn windows(short: f64, long: f64) -> Vec<Window>;         // Task 11
pub(crate) fn board() -> Arc<StatusBoard>;                           // Task 8
pub(crate) fn job_ctx(board: &Arc<StatusBoard>) -> ProviderJobCtx;   // Task 8; Task 11 adds `notices: None`
pub(crate) fn status(board: &StatusBoard, of: &ProviderId) -> ProviderStatus;  // Task 8
pub(crate) fn capture_logs<T>(f: impl FnOnce() -> T) -> (T, Vec<String>);      // Task 11
/// Claude Code and FakeAgent on a fake clock, a silent `ScriptedHttp` and a `ScriptedSpawner`.
pub(crate) struct Fx { _dir, env, kc, clock, http, engine, spawner /* Task 11 */ }
impl Fx {
    pub(crate) fn new() -> Self;                                     // Task 7
    pub(crate) fn with_run_shell(run_shell: RunShell) -> Self;       // Task 11
    pub(crate) fn login(&self, email: &str);                         // Task 8
    pub(crate) fn add(&self, email: &str) -> AccountId;              // Task 7
    pub(crate) fn add_fake(&self, handle: &str) -> AccountId;        // Task 7
    pub(crate) fn read(&self, id: &AccountId, short: f64, long: f64, at: i64, next_poll_at: i64);  // Task 7
    pub(crate) fn settings(&self, text: &str, mtime_s: u64);         // Task 7
    pub(crate) fn other(&self) -> Engine;                            // Task 10: another process's engine
    pub(crate) fn add_on(&self, engine: &Engine, email: &str) -> AccountId;        // Task 10
    pub(crate) fn remove_store(&self);                               // Task 10: the store file and its sidecars deleted
    pub(crate) fn login_as(&self, email: &str, org: &str);           // Task 11: names an organization
    pub(crate) fn add_as(&self, email: &str, org: &str, alias: Option<&str>) -> AccountId;  // Task 11
    pub(crate) fn read_windows(&self, id: &AccountId, windows: &[Window], at: i64, next_poll_at: i64);  // Task 11
}
pub(crate) struct Slices<'a> { pub(crate) taken: RefCell<Vec<Duration>>, /* … */ }  // Task 7: new(clock, then), none(clock)
pub(crate) struct Emitted<'a> { /* … */ }  // Task 8: new(clock), events(), modes(of), polls(of); Task 11: announced()
```

### `tagteam` (CLI, Tasks 7 and 12)

```rust
// Cargo.toml: test-support gains "tagteam-daemon/test-hooks" (Task 7)

// cli.rs (Task 12)
Command::Daemon {}                     // plan B turns it into `Daemon { action: Option<DaemonAction> }`

// app.rs (Task 12)
struct App { /* … */ platform: Platform, keychain_counts: Vec<Arc<ReadCounts>> }
impl App { fn daemon(&mut self) -> Result<i32, Failure>; }
impl From<DaemonError> for Failure {}

// auto.rs (Task 12): `HumanSink` gains pub(crate) `line`, `plain`, `now_s`, `time_of`, and a
// cfg(test) `with_clock`; `written` becomes pub(crate)

// daemon_cmd.rs (new, Task 12)
pub(crate) const EXIT_RESTART: i32 = 75;
pub(crate) fn keychain_counts<'k>(keychains: impl IntoIterator<Item = &'k Arc<dyn Keychain>>) -> Vec<Arc<ReadCounts>>;
pub(crate) fn program(path_var: Option<&OsStr>) -> PathBuf;
pub(crate) struct DaemonHuman<'a>;     // new(auto: HumanSink<'a>, env: &'a Env), started(pid); impl DaemonSink
pub(crate) struct DaemonJson<'a>;      // new(out, now_ms), stopping(cancel); impl DaemonSink

// tests/common/mod.rs (Task 12)
impl Running {
    pub fn stderr(&self) -> String;
    pub fn wait_for_stderr(&mut self, within: Duration, what: &str, ready: impl Fn(&str) -> bool);
}
```

## Task Overview

| # | Task | Depends on |
|---|---|---|
| 1 | Engine settings that reload in place | — |
| 2 | `AutoEngine`'s source, its lock record on drop, `QuarantineWatch` | — |
| 3 | Engine hooks: `data_version`, `forget_store`, `live_reading`, `take_lease`, `spawner`, daemon paths and record | 1 |
| 4 | Keychain read counts | — |
| 5 | Settings keys `daemon.autoswitch`, `notify.enabled`, `notify.events` | 1 |
| 6 | The `tagteam-daemon` crate, its events and event JSON | 2 |
| 7 | The scheduler, and `tagteam auto` on it | 6 |
| 8 | Provider jobs, their modes and the status board | 2, 3, 4, 5, 7 |
| 9 | The keychain backoff | 4, 8 |
| 10 | Store and upgrade watches | 3, 8, 9 |
| 11 | Notifications | 3, 5, 8, 9, 10 |
| 12 | Start-up, serving and `tagteam daemon` | 9, 10, 11 |
| 13 | Verification | 12 |

Tasks run one at a time, in order: each task's tests build the crates earlier tasks touch, and
tasks in one crate would build each other's unfinished code. Tasks 1 and 2 both edit
`tagteam-engine`; Task 4 edits `tagteam-provider`, which the engine builds on; Tasks 3 and 5 edit
the engine, which Task 6's crate builds. No two tasks run together, even where the table shows
no dependency between them.

---

## Tasks

### Task 1: Engine settings that reload in place

**Depends on:** none

D3 change 1: "`Engine` keeps its settings behind a lock, and `Engine::reload_settings()` re-reads
`config.toml` and returns its warnings (§6.4). A daemon lives for days, and the collector and the
views read the engine's settings (`models`, `history_retention_days`). `settings()` returns a
snapshot." Decision 3 fixes the shape: the field becomes `RwLock<Arc<Settings>>`, `settings()`
clones the `Arc`, and `reload_settings(&self, provider)` re-reads the file for that provider's
table and returns its warnings.

**Readings this task commits to:**
- **A snapshot stays as it was.** `settings()` hands out the `Arc` the engine holds now. A reload
  swaps in a new `Arc` and never mutates the old one, so a caller holding a snapshot (a
  collection in progress) finishes with the values it started with.
- **The reload is §6.4's forgiving read, for `provider`.** It is `Settings::load(env, provider)`:
  a missing or corrupt file gives the defaults, an invalid value its default and a warning, and
  the provider's own table wins. The warnings are returned, never logged here; the caller turns
  them into `config-warning` events (D4.2).
- **Only call sites that keep a borrow past their statement change.** Two do not compile with an
  `Arc`: `views.rs`'s `let models = &self.settings().models;` (the temporary would die at the end
  of the `let`) and `profiles.rs`'s direct field access `&self.settings.share_extra`. Every other
  call site already binds the result, copies a field, or borrows it only as an argument, which a
  temporary outlives:
  - `collect.rs:207` (`let settings = self.settings();`) and `collect.rs:1208`
    (`.history_retention_days`, a `u32`)
  - `switch.rs:1251` (`.models.clone()`)
  - `views.rs:313` (`&self.settings().models` as an argument inside the `&&`'s right operand)
  - `crates/tagteam/src/app.rs:854` and `:1031`, and `crates/tagteam/src/statusline.rs:686`,
    `:718` and `:756`

  These stay as they are. Clippy over the workspace proves it.
- **The field becomes private.** After `profiles.rs` calls `settings()`, nothing reads the field
  directly.

**Files:**
- Modify: `crates/tagteam-engine/src/engine.rs` (the `std::sync` import; the field
  `pub(crate) settings: Settings,`; `Engine::new`'s `settings: cfg.settings,`; `pub fn settings`;
  a new `reload_settings`; the unit test `the_engine_keeps_the_settings_it_was_built_with`)
- Modify: `crates/tagteam-engine/src/views.rs` (`history`: `let models = &self.settings().models;`)
- Modify: `crates/tagteam-engine/src/profiles.rs` (`allowlist(&policy, &self.settings.share_extra, …)`)
- Test: `crates/tagteam-engine/src/engine.rs` (`mod tests`), `crates/tagteam-engine/tests/views_usage.rs`

**Interfaces:**
- Consumes: `Settings::load(env: &Env, provider: &ProviderId) -> (Settings, Vec<String>)`
  (`settings.rs`, unchanged).
- Produces (`tagteam_engine::Engine`):
  - `pub fn settings(&self) -> Arc<Settings>`
  - `pub fn reload_settings(&self, provider: &ProviderId) -> Vec<String>`

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/src/engine.rs`'s `mod tests`, change the one assertion that compares
the old reference. Both before and after this task, `*engine.settings()` is a `Settings`:

```rust
        assert_eq!(engine.settings(), &settings);
```

becomes

```rust
        assert_eq!(*engine.settings(), settings);
```

Then add this test to the same module, after `the_engine_keeps_the_settings_it_was_built_with`:

```rust
    #[test]
    fn a_reload_reads_config_toml_again_and_returns_its_warnings() {
        // D3 change 1 (Decision 3): a daemon lives for days, so what the engine's readers see
        // follows the file. A snapshot taken before a reload keeps the values it had.
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let engine = test_engine(env.clone());
        let cc = ProviderId::new("claude-code");
        let before = engine.settings();
        let path = crate::settings::config_path(&env);
        std::fs::create_dir_all(env.config_dir()).unwrap();
        std::fs::write(
            &path,
            "[autoswitch]\nmodels = [\"Fable\"]\n\n[usage]\nhistory_retention_days = 30\n\n\
             [provider.claude-code.autoswitch]\nmodels = [\"Opus\"]\n",
        )
        .unwrap();

        assert!(engine.reload_settings(&ProviderId::new("fake-agent")).is_empty());
        assert_eq!(engine.settings().models, ["Fable"]);
        assert!(engine.reload_settings(&cc).is_empty());
        let after = engine.settings();
        assert_eq!(after.models, ["Opus"], "the provider's own table first (§6.4)");
        assert_eq!(after.history_retention_days, 30);
        assert_eq!(*before, Settings::default(), "a snapshot keeps its values");

        // Forgiving (§6.4): an invalid value reads as its default, with one warning naming it,
        // and the rest of the file applies; this file names no models any more.
        std::fs::write(&path, "[usage]\nhistory_retention_days = 0\n").unwrap();
        let warnings = engine.reload_settings(&cc);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("`usage.history_retention_days`"),
            "{warnings:?}"
        );
        let invalid = engine.settings();
        assert_eq!(invalid.history_retention_days, 180);
        assert!(invalid.models.is_empty(), "{:?}", invalid.models);
        assert_eq!(after.models, ["Opus"], "the earlier snapshot is untouched");
    }
```

And add this test to `crates/tagteam-engine/tests/views_usage.rs`, after
`history_follows_the_configured_models`. It shows a reload reaching a view, through the
`views.rs` call site this task rewrites:

```rust
#[test]
fn history_follows_the_models_a_settings_reload_brings() {
    // D3 change 1: the views read the engine's settings as they are now, not as they were
    // when the engine was built.
    let fx = Fx::new();
    let a = with_history(&fx);
    assert_eq!(keys(&fx.engine.history(&a, None, 0).unwrap()), ["5h", "7d"]);
    fs::create_dir_all(fx.env.config_dir()).unwrap();
    fs::write(
        fx.env.config_dir().join("config.toml"),
        "[autoswitch]\nmodels = [\"fable\"]\n",
    )
    .unwrap();
    assert!(fx.engine.reload_settings(&fx.provider()).is_empty());
    assert_eq!(
        keys(&fx.engine.history(&a, None, 0).unwrap()),
        ["5h", "7d", "scoped:Fable"]
    );
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --lib engine::tests`
Expected: the build fails with `error[E0599]: no method named `reload_settings` found for struct
`Engine``. `cargo test -p tagteam-engine --test views_usage history_follows` fails the same way.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/engine.rs`, the import:

```rust
use std::sync::{Arc, Mutex, PoisonError};
```

becomes

```rust
use std::sync::{Arc, Mutex, PoisonError, RwLock};
```

The field in `pub struct Engine`:

```rust
    pub(crate) settings: Settings,
```

becomes

```rust
    /// `config.toml` as last read (§6.4): the command's read at first, then each
    /// `reload_settings` (D3 change 1). Readers take a snapshot through `settings()`.
    settings: RwLock<Arc<Settings>>,
```

In `Engine::new`:

```rust
            settings: cfg.settings,
```

becomes

```rust
            settings: RwLock::new(Arc::new(cfg.settings)),
```

Replace `settings()`:

```rust
    /// The settings this engine was built with (§6.4).
    pub fn settings(&self) -> &Settings {
        &self.settings
    }
```

with:

```rust
    /// The settings as last read (§6.4), as a snapshot. A reload replaces what later calls
    /// return, and leaves a snapshot already taken as it was. Bind it before borrowing from
    /// it: `let settings = engine.settings();`.
    pub fn settings(&self) -> Arc<Settings> {
        self.settings
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// D3 change 1: reads `config.toml` again as `provider`'s commands read it (§6.4:
    /// forgiving, the provider's own table first), makes it the engine's settings, and returns
    /// the read's warnings. A missing or corrupt file gives the defaults, as at start-up. The
    /// daemon reloads for the provider its command acts on, at start and whenever the file's
    /// mtime changes (D4.2); the collector and the views read the result.
    pub fn reload_settings(&self, provider: &ProviderId) -> Vec<String> {
        let (settings, warnings) = Settings::load(&self.env, provider);
        *self.settings.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(settings);
        warnings
    }
```

In `crates/tagteam-engine/src/views.rs`, in `history`:

```rust
        let models = &self.settings().models;
```

becomes

```rust
        let settings = self.settings();
        let models = &settings.models;
```

In `crates/tagteam-engine/src/profiles.rs`:

```rust
        let wanted = allowlist(&policy, &self.settings.share_extra, &mut report.warnings);
```

becomes

```rust
        let wanted = allowlist(&policy, &self.settings().share_extra, &mut report.warnings);
```

(`allowlist` returns an owned `Vec<Wanted>`, so the temporary `Arc` may end with the statement.)

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-engine --lib engine::tests`, then
`cargo test -p tagteam-engine --test views_usage history`, then
`cargo test -p tagteam --lib statusline` (the CLI's untouched call sites).
Then `cargo fmt --all && cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`.
Expected: PASS. If clippy or the build names another call site that holds a borrow of
`settings()` past its statement, bind the `Arc` first, as in `views.rs`.

- [ ] **Step 5: Commit** (the controller commits; implementers do not)

`git commit -m "Let the engine reload its settings in place"`

---

### Task 2: `AutoEngine`'s source, its lock record on drop, `QuarantineWatch`

**Depends on:** none

Three changes to `crates/tagteam-engine/src/auto.rs`, each one a daemon plan Decision:
- **Decision 4, the source.** `AutoEngine::with_source(self, source)` sets what the engine's
  switches, recoveries and quarantine releases record as `events.source` (§6.1). The three
  hard-coded `"auto"`s use the field instead: `run` (`release_unbound_quarantines`), `recover`
  (`settle_or_refuse_as`) and `perform` (`SwitchRequest.source`). `Engine::auto` keeps its
  signature and starts every engine at `"auto"`, so no existing caller changes.
- **Decision 5, the record on drop.** `impl Drop for AutoEngine` empties the engine lock file
  (`set_len(0)` through a second descriptor, as `write_holder` writes it) while the guard is
  still held. `Drop::drop` runs before the fields drop. A dry-run engine has no lock and touches
  nothing.
- **Decision 6, the quarantine watch.** `seen`, `report_quarantines` and `note_quarantined` move
  into `pub struct QuarantineWatch`, which the daemon's `collect` rounds will use too (D4.3). The
  logic moves verbatim. `crates/tagteam-engine/tests/auto_tick.rs` pins it and is not edited.

`Engine::engine_holder(provider)` also lands here, in `auto.rs`'s `impl Engine` block beside
`Engine::auto`, wrapping `read_holder(&engine_lock_path(..))`. The daemon's `collect` mode
judges the holder with it without trying the lock (D4.3). Nothing in this task touches
`engine.rs`.

**Readings this task commits to:**
- **"Empty" means a zero-length file.** `read_holder` already reads a zero-length file as
  `Absent` ("no engine has written its record there yet"). The lock file itself stays, as every
  lock file does (§5).
- **An I/O error while emptying is a WARN, by kind.** It names the provider, which `Engine::auto`
  checked is registered (§14.2), and the error's `ErrorKind`, never a path.
- **`with_source` reaches a dry-run engine too.** Its `recover` can finish a recovery (§11.1),
  which records the source. The daemon never runs a dry-run engine.

**Files:**
- Modify: `crates/tagteam-engine/src/auto.rs`:
  - add `QuarantineWatch` after `struct Seen`
  - the `AutoEngine` struct's fields
  - `Engine::auto`'s constructor, and a new `Engine::engine_holder` beside it
  - `with_source` and `provider` at the top of `impl AutoEngine<'_>`
  - `run`, `perform`, `recover` and `report_collection`
  - delete `report_quarantines` and `note_quarantined`
  - add `impl Drop` after the `impl AutoEngine<'_>` block
- Test: `crates/tagteam-engine/tests/auto_engine.rs`
- Unchanged, and must pass: `crates/tagteam-engine/tests/auto_tick.rs`

**Interfaces:**
- Consumes (existing, unchanged): `fn write_holder(path: &Path)`,
  `pub(crate) fn engine_lock_path(env: &Env, provider: &ProviderId) -> PathBuf`,
  `pub(crate) fn read_holder(path: &Path) -> Read<ProcessStamp>`,
  `Engine::release_unbound_quarantines(&self, &ProviderId, &'static str)`,
  `Engine::settle_or_refuse_as(&self, &ProviderId, &'static str)`,
  `SwitchRequest { source: &'static str, .. }`.
- Produces (`tagteam_engine::auto`):
  - `impl<'e> AutoEngine<'e> { pub fn with_source(self, source: &'static str) -> Self; pub fn provider(&self) -> &ProviderId; }`
  - `impl Drop for AutoEngine<'_>`
  - `#[derive(Debug, Default)] pub struct QuarantineWatch`, with
    `pub fn new() -> Self`,
    `pub fn report(&mut self, provider: &ProviderId, rows: &[AccountRow], released: &[AccountId], sink: &dyn EventSink)` and
    `pub fn note_quarantined(&mut self, provider: &ProviderId, row: &AccountRow, sink: &dyn EventSink)`
  - `impl Engine { pub fn engine_holder(&self, provider: &ProviderId) -> Read<ProcessStamp>; }`

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/auto_engine.rs`, replace the module doc and the imports:

```rust
//! §11.1's engine: one per provider per machine, held by a try-only lock that records its
//! holder; dry-run holds no lock and writes no auto-switch state; the live account's next
//! fetchable time for the loop.
mod common;

use std::fs;

use common::{FakeFx, Fx, Recorded, record_reading, usage_window};
use serde_json::Value;
use tagteam_core::autoswitch::{AutoConfig, AutoState, Decision, NoSwitchReason, Strategy};
use tagteam_core::{ProviderId, WindowKind};
use tagteam_engine::EngineError;
use tagteam_engine::auto::{AutoEvent, TickOutcome};
use tagteam_provider::{ProcessStamp, Provider};
```

with

```rust
//! §11.1's engine: one per provider per machine, held by a try-only lock that records its
//! holder; dry-run holds no lock and writes no auto-switch state; the live account's next
//! fetchable time for the loop. The daemon's hooks (daemon plan A, Decisions 4 to 6): the
//! source an engine records, the record it empties when dropped, and the quarantine watch its
//! ticks share with the daemon's collect rounds.
mod common;

use std::fs;

use common::{
    FakeFx, Fx, Recorded, crashed_switch, record_reading, usage_window, write_target_credential,
};
use serde_json::{Value, json};
use tagteam_core::autoswitch::{AutoConfig, AutoState, Decision, NoSwitchReason, Strategy};
use tagteam_core::{AccountId, ProviderId, Window, WindowKind};
use tagteam_engine::EngineError;
use tagteam_engine::auto::{AutoEvent, QuarantineWatch, TickOutcome};
use tagteam_engine::store::AccountRow;
use tagteam_provider::{FlockGuard, ProcessStamp, Provider, Read};
```

Then append to the end of the file:

```rust
/// Claude Code's 5h and 7d windows at `short` and `long` percent, as every reading here has
/// them.
fn windows(short: f64, long: f64) -> Vec<Window> {
    vec![
        usage_window("5h", WindowKind::Short, short, T0 + 9_630),
        usage_window("7d", WindowKind::Long, long, T0 + 291_630),
    ]
}

/// A reading taken now with its next poll five minutes out: decision-grade, and not due.
fn read(fx: &Fx, id: &AccountId, w: &[Window]) {
    record_reading(&fx.engine, id, w, T0, T0 + 300);
}

/// The last event the store recorded, as `(kind, source)`.
fn last_event(fx: &Fx) -> (String, String) {
    let last = fx.engine.store().unwrap().events().unwrap().pop().unwrap();
    (last.kind, last.source)
}

#[test]
fn a_daemon_engine_records_its_switches_as_the_daemon_s() {
    // Decision 4: `events.source` reserves `daemon` (§6.1). The engine starts as `auto`.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx, &a, &windows(10.0, 20.0));
    read(&fx, &b, &windows(20.0, 95.0));
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap()
        .with_source("daemon");
    assert_eq!(engine.provider(), &fx.provider());
    let (outcome, _) = engine.tick(&Recorded::default()).unwrap();
    assert_eq!(outcome, TickOutcome::Switched);
    assert_eq!(last_event(&fx), ("switch".into(), "daemon".into()));
}

#[test]
fn a_daemon_engine_recovers_an_interrupted_switch_as_the_daemon() {
    // Decision 4: §11.2 step 2's recovery records the engine's source too (§9.6).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    read(&fx, &a, &windows(10.0, 20.0));
    read(&fx, &b, &windows(10.0, 20.0));
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap()
        .with_source("daemon");
    engine.tick(&Recorded::default()).unwrap();
    assert_eq!(
        last_event(&fx),
        ("switch-recovered".into(), "daemon".into())
    );
}

#[test]
fn a_daemon_engine_releases_an_unbound_quarantine_as_the_daemon() {
    // Decision 4: step 1's release records the engine's source (§7.4). The vault has moved
    // past the generation the quarantine names, so it no longer binds.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx, &a, &windows(10.0, 10.0));
    read(&fx, &b, &windows(10.0, 50.0));
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    let mut engine = fx
        .engine
        .auto(&fx.provider(), cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap()
        .with_source("daemon");
    engine.tick(&Recorded::default()).unwrap();
    assert_eq!(last_event(&fx), ("unquarantine".into(), "daemon".into()));
}

#[test]
fn a_dropped_engine_leaves_its_lock_file_empty_and_free() {
    // Decision 5, D3 change 3: a daemon that lets a provider go lives on, so a record left
    // behind would name a live process as the provider's engine.
    let fx = Fx::new();
    let provider = fx.provider();
    let path = lock_path(&fx, &provider);
    assert!(matches!(fx.engine.engine_holder(&provider), Read::Absent));
    let engine = fx
        .engine
        .auto(&provider, cfg(fx.cc.as_ref()), false)
        .unwrap()
        .unwrap();
    let me = ProcessStamp::current().unwrap();
    assert!(matches!(fx.engine.engine_holder(&provider), Read::Present(s) if s == me));
    drop(engine);
    assert_eq!(fs::read(&path).unwrap(), b"", "the lock file stays, emptied");
    assert!(matches!(fx.engine.engine_holder(&provider), Read::Absent));
    assert!(
        FlockGuard::try_lock(&path).unwrap().is_some(),
        "and the lock is free"
    );
}

#[test]
fn a_dry_run_engine_s_drop_touches_no_lock_file() {
    // Decision 5: a dry run holds no lock (§11.1), so the record belongs to someone else.
    let fx = Fx::new();
    let provider = fx.provider();
    let path = lock_path(&fx, &provider);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let record = "{\"pid\":4242,\"start\":1}\n";
    fs::write(&path, record).unwrap();
    let dry = fx
        .engine
        .auto(&provider, cfg(fx.cc.as_ref()), true)
        .unwrap()
        .unwrap();
    drop(dry);
    assert_eq!(fs::read_to_string(&path).unwrap(), record);
    assert!(matches!(
        fx.engine.engine_holder(&provider),
        Read::Present(ProcessStamp { pid: 4242, start: 1 })
    ));
    fs::write(&path, "garbage").unwrap();
    assert!(matches!(
        fx.engine.engine_holder(&provider),
        Read::Unreadable(_)
    ));
}

/// A Claude Code account row at `position`, quarantined for `reason` when one is given.
fn row(id: &str, position: u32, reason: Option<&str>, login_epoch: i64) -> AccountRow {
    let email = format!("{id}@x.co");
    AccountRow {
        id: AccountId::from_string(id),
        provider: ProviderId::new("claude-code"),
        position,
        identity_key: format!("{email}\n"),
        label: email.clone(),
        email: Some(email),
        org_uuid: String::new(),
        org_name: None,
        account_uuid: None,
        kind: "oauth".into(),
        alias: None,
        disabled: false,
        identity_json: json!({}),
        login_expires_at: None,
        login_epoch,
        replacing_fp: None,
        quarantine_reason: reason.map(str::to_owned),
        quarantine_fp: reason.map(|_| "sha256:q".to_owned()),
        quarantine_at: reason.map(|_| 1),
        added_at: 1,
    }
}

fn quarantined(p: &ProviderId, number: u32, email: &str, reason: &str) -> AutoEvent {
    AutoEvent::AccountQuarantined {
        provider: p.clone(),
        number,
        email: email.into(),
        reason: reason.into(),
    }
}

fn unquarantined(p: &ProviderId, number: u32, email: &str, reason: &str) -> AutoEvent {
    AutoEvent::AccountUnquarantined {
        provider: p.clone(),
        number,
        email: email.into(),
        reason: reason.into(),
    }
}

#[test]
fn a_quarantine_watch_reports_only_releases_on_its_first_call() {
    // Decision 6, as the tick's step 1 did (Decision 10): with no previous call to compare
    // with, a quarantine that is already set is not news.
    let p = ProviderId::new("claude-code");
    let sink = Recorded::default();
    let mut watch = QuarantineWatch::new();
    let rows = [row("a", 1, Some("invalid_grant"), 0), row("b", 2, None, 0)];
    watch.report(&p, &rows, &[AccountId::from_string("b")], &sink);
    assert_eq!(
        sink.take(),
        [unquarantined(&p, 2, "b@x.co", "credentials-replaced")]
    );
}

#[test]
fn a_quarantine_watch_reports_what_was_set_or_cleared_since_its_previous_call() {
    let p = ProviderId::new("claude-code");
    let sink = Recorded::default();
    let mut watch = QuarantineWatch::new();
    watch.report(
        &p,
        &[row("a", 1, None, 0), row("b", 2, Some("invalid_grant"), 0)],
        &[],
        &sink,
    );
    assert!(sink.take().is_empty());
    // Set on a, cleared on b, and c added since, already quarantined.
    watch.report(
        &p,
        &[
            row("a", 1, Some("invalid_grant"), 0),
            row("b", 2, None, 0),
            row("c", 3, Some("invalid_grant"), 0),
        ],
        &[],
        &sink,
    );
    assert_eq!(
        sink.take(),
        [
            quarantined(&p, 1, "a@x.co", "invalid_grant"),
            unquarantined(&p, 2, "b@x.co", "credentials-replaced"),
            quarantined(&p, 3, "c@x.co", "invalid_grant"),
        ]
    );
    // An explicit replacement (§12.5) moved a's login epoch as it cleared the quarantine.
    let now = [
        row("a", 1, None, 1),
        row("b", 2, None, 0),
        row("c", 3, Some("invalid_grant"), 0),
    ];
    watch.report(&p, &now, &[], &sink);
    assert_eq!(
        sink.take(),
        [unquarantined(&p, 1, "a@x.co", "account-replaced")]
    );
    watch.report(&p, &now, &[], &sink);
    assert!(sink.take().is_empty(), "nothing changed since");
}

#[test]
fn a_quarantine_a_round_found_itself_is_reported_once() {
    // Decision 6: a collection's or a freshening's quarantine is news once; the next
    // comparison already knows it.
    let p = ProviderId::new("claude-code");
    let sink = Recorded::default();
    let mut watch = QuarantineWatch::new();
    let a = row("a", 1, Some("invalid_grant"), 0);
    watch.note_quarantined(&p, &a, &sink);
    watch.note_quarantined(&p, &a, &sink);
    assert_eq!(sink.take(), [quarantined(&p, 1, "a@x.co", "invalid_grant")]);
    watch.report(&p, std::slice::from_ref(&a), &[], &sink);
    assert!(sink.take().is_empty());
    watch.note_quarantined(&p, &row("b", 2, None, 0), &sink);
    assert!(sink.take().is_empty(), "an account with no quarantine is no news");
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test auto_engine`
Expected: the build fails: `unresolved import `tagteam_engine::auto::QuarantineWatch``,
`no method named `with_source` found for struct `AutoEngine``, and
`no method named `engine_holder` found for struct `Engine``.

- [ ] **Step 3: Implement**

All edits are in `crates/tagteam-engine/src/auto.rs`.

**(a) `QuarantineWatch`.** Insert it after `struct Seen { … }`. Its two methods are
`report_quarantines` and `note_quarantined` moved verbatim, with `self.emit(sink, |provider| …)`
written as `sink.emit(&AutoEvent::… { provider: provider.clone(), … })`:

```rust
/// Decision 6 (daemon plan A): §11.2 step 1's report of quarantine changes, shared by an
/// engine's ticks and the daemon's `collect` rounds (D4.3). It keeps each account's quarantine
/// as the previous call left it (Decision 10), so each change is reported once.
#[derive(Debug, Default)]
pub struct QuarantineWatch {
    /// Each account's quarantine as the previous call left it; `None` before the first.
    seen: Option<BTreeMap<AccountId, Seen>>,
}

impl QuarantineWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// The releases this round made (`released`), and, against the previous call's map,
    /// every quarantine another process set or cleared since (Decision 10): `account-replaced`
    /// when the login epoch moved, else `credentials-replaced`. The first call has no previous
    /// map, so it reports only the releases.
    pub fn report(
        &mut self,
        provider: &ProviderId,
        rows: &[AccountRow],
        released: &[AccountId],
        sink: &dyn EventSink,
    ) {
        let before = self.seen.take();
        for row in rows {
            let was = before.as_ref().and_then(|m| m.get(&row.id));
            let reason = || {
                match was {
                    Some(s) if s.login_epoch != row.login_epoch => "account-replaced",
                    _ => "credentials-replaced",
                }
                .to_owned()
            };
            let cleared = released.contains(&row.id)
                || (was.is_some_and(|s| s.reason.is_some()) && row.quarantine_reason.is_none());
            let set = before.is_some()
                && row.quarantine_reason.is_some()
                && was.is_none_or(|s| s.reason.is_none());
            if cleared {
                sink.emit(&AutoEvent::AccountUnquarantined {
                    provider: provider.clone(),
                    number: row.position,
                    email: email_of(row),
                    reason: reason(),
                });
            } else if set {
                sink.emit(&AutoEvent::AccountQuarantined {
                    provider: provider.clone(),
                    number: row.position,
                    email: email_of(row),
                    reason: row.quarantine_reason.clone().unwrap_or_default(),
                });
            }
        }
        self.seen = Some(
            rows.iter()
                .map(|r| {
                    let seen = Seen {
                        reason: r.quarantine_reason.clone(),
                        login_epoch: r.login_epoch,
                    };
                    (r.id.clone(), seen)
                })
                .collect(),
        );
    }

    /// A quarantine this round's own work found (a collection's, or a tick's step 10
    /// freshening), reported once: the next `report` already knows it.
    pub fn note_quarantined(
        &mut self,
        provider: &ProviderId,
        row: &AccountRow,
        sink: &dyn EventSink,
    ) {
        let Some(reason) = row.quarantine_reason.clone() else {
            return;
        };
        let seen = self.seen.get_or_insert_with(BTreeMap::new);
        let entry = seen.entry(row.id.clone()).or_insert(Seen {
            reason: None,
            login_epoch: row.login_epoch,
        });
        if entry.reason.is_some() {
            return;
        }
        entry.reason = Some(reason.clone());
        sink.emit(&AutoEvent::AccountQuarantined {
            provider: provider.clone(),
            number: row.position,
            email: email_of(row),
            reason,
        });
    }
}
```

**(b) The struct.** Replace:

```rust
    dry_run: bool,
    /// The engine lock, held for this engine's life; `None` in dry-run.
    _lock: Option<FlockGuard>,
    /// Dry-run's `unhealthy_ticks`: it writes no auto-switch state (§11.1).
    unhealthy: Option<u32>,
    /// Decision 10: each account's quarantine as the previous tick left it.
    seen: Option<BTreeMap<AccountId, Seen>>,
```

with:

```rust
    dry_run: bool,
    /// What its switches, recoveries and quarantine releases record as `events.source`
    /// (§6.1): `auto`, or the daemon's `daemon` (Decision 4).
    source: &'static str,
    /// The engine lock, held for this engine's life; `None` in dry-run. `Drop` empties its
    /// record while it is still held (Decision 5).
    lock: Option<FlockGuard>,
    /// Dry-run's `unhealthy_ticks`: it writes no auto-switch state (§11.1).
    unhealthy: Option<u32>,
    /// Decision 10: each account's quarantine as the previous tick left it.
    quarantines: QuarantineWatch,
```

**(c) `Engine::auto` and `Engine::engine_holder`.** In `Engine::auto`, replace the constructor:

```rust
        Ok(Some(AutoEngine {
            engine: self,
            provider: provider.clone(),
            cfg,
            dry_run,
            _lock: lock,
            unhealthy: None,
            seen: None,
            checked_models: None,
            checked_strategy: None,
            judged: None,
            live: None,
        }))
    }
}
```

with:

```rust
        Ok(Some(AutoEngine {
            engine: self,
            provider: provider.clone(),
            cfg,
            dry_run,
            source: "auto",
            lock,
            unhealthy: None,
            quarantines: QuarantineWatch::new(),
            checked_models: None,
            checked_strategy: None,
            judged: None,
            live: None,
        }))
    }

    /// The holder `provider`'s engine lock record names (§11.1), read without trying the
    /// lock, as `doctor` reads it (§13.6): a try at the wrong instant would make a starting
    /// `auto` fail (D4.3). `Absent` with no record: no engine has run, or the last one let the
    /// lock go (Decision 5). Never proof that the holder runs: judge that by an exact pid and
    /// start-time match (`ProcessStamp::liveness`).
    pub fn engine_holder(&self, provider: &ProviderId) -> Read<ProcessStamp> {
        read_holder(&engine_lock_path(&self.env, provider))
    }
}
```

**(d) `with_source` and `provider`.** At the top of `impl AutoEngine<'_> {`, before `pub fn tick`:

```rust
    /// Decision 4: the source this engine's switches, recoveries and quarantine releases
    /// record (§6.1). `Engine::auto` starts every engine at `auto`; the daemon passes `daemon`.
    pub fn with_source(mut self, source: &'static str) -> Self {
        self.source = source;
        self
    }

    /// The provider this engine drives.
    pub fn provider(&self) -> &ProviderId {
        &self.provider
    }
```

**(e) `run`.** Replace:

```rust
            self.engine
                .release_unbound_quarantines(&self.provider, "auto")?
```

with:

```rust
            self.engine
                .release_unbound_quarantines(&self.provider, self.source)?
```

and replace:

```rust
        self.report_quarantines(&rows, &released, sink);
```

with:

```rust
        self.quarantines
            .report(&self.provider, &rows, &released, sink);
```

**(f) `perform`.** Replace:

```rust
                    if let Some(now) = t.store.account(id)? {
                        self.note_quarantined(&now, sink);
                    }
```

with:

```rust
                    if let Some(now) = t.store.account(id)? {
                        self.quarantines
                            .note_quarantined(&self.provider, &now, sink);
                    }
```

and in the `SwitchRequest`, replace `source: "auto",` with `source: self.source,`.

**(g) `recover`.** Replace its doc and body's first line:

```rust
    /// §11.2 step 2: an unresolved switch journal for the provider is recovered first, under
    /// `MutationGuard`, its events carrying `source` = `auto` (§9.6). `Some` with recovery's
    /// refusal while a row stays: one recovery could not decide, could not take the live
    /// locks or found its agent writing, or the mutation lock was held past its timeout.
    fn recover(&self) -> Result<Option<EngineError>, EngineError> {
        match self.engine.settle_or_refuse_as(&self.provider, "auto") {
```

with:

```rust
    /// §11.2 step 2: an unresolved switch journal for the provider is recovered first, under
    /// `MutationGuard`, its events carrying the engine's source (§9.6, Decision 4). `Some`
    /// with recovery's refusal while a row stays: one recovery could not decide, could not
    /// take the live locks or found its agent writing, or the mutation lock was held past its
    /// timeout.
    fn recover(&self) -> Result<Option<EngineError>, EngineError> {
        match self.engine.settle_or_refuse_as(&self.provider, self.source) {
```

**(h) Delete** the two methods `fn report_quarantines(…)` and `fn note_quarantined(…)`, with
their doc comments: from `/// Step 1's quarantine report: this tick's own releases, …` through
the closing brace of `note_quarantined`. Their bodies now live in `QuarantineWatch`.

**(i) `report_collection`.** Replace:

```rust
            if let Some(a) = accounts.iter().find(|a| &a.row.id == id) {
                self.note_quarantined(&a.row, sink);
            }
```

with:

```rust
            if let Some(a) = accounts.iter().find(|a| &a.row.id == id) {
                self.quarantines
                    .note_quarantined(&self.provider, &a.row, sink);
            }
```

**(j) `Drop`.** Insert after the closing brace of `impl AutoEngine<'_> { … }`, before
`/// What a tick has read by the time it decided.`:

```rust
/// Decision 5, D3 change 3: a dropped engine empties its lock file's holder record before the
/// lock goes, so a process that lives on (a daemon that paused the provider, D4.3) is never
/// named as its engine. `drop` runs before the fields drop, so the lock is still held while
/// the record is emptied, through a second descriptor as `write_holder` writes it. A failure
/// is logged and otherwise ignored. A dry-run engine holds no lock and touches nothing. A
/// process killed by SIGKILL still leaves its record, which names a dead process.
impl Drop for AutoEngine<'_> {
    fn drop(&mut self) {
        let Some(lock) = &self.lock else {
            return;
        };
        let emptied = OpenOptions::new()
            .write(true)
            .open(lock.path())
            .and_then(|file| file.set_len(0));
        if let Err(e) = emptied {
            tracing::warn!(
                provider = %self.provider,
                kind = %e.kind(),
                "could not empty the engine lock's holder record"
            );
        }
    }
}
```

`OpenOptions` is already imported for `write_holder`.

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-engine --test auto_engine --test auto_tick`. Expected: PASS;
`auto_tick.rs` is untouched and pins the moved quarantine logic.
Then `cargo fmt --all && cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`.
Expected: PASS.

- [ ] **Step 5: Commit** (the controller commits; implementers do not)

`git commit -m "Give the auto engine a source, empty its lock record on drop, and share its quarantine watch"`

---

### Task 3: Engine hooks: `data_version`, `forget_store`, `live_reading`, `take_lease`, `spawner`, daemon paths and record

**Depends on:** 1

The engine changes the daemon schedules through. None adds a decision or a table (D3):
- **`Store::data_version` and `Engine::data_version`** (D3 change 4): SQLite's
  `PRAGMA data_version` on the engine's own connection. `None` while there is no store.
- **`Engine::forget_store`** (Decision 11): drops the cached `Arc<Store>`, so the next use
  opens the file again. **`Engine::store_file`**: `<data>/tagteam.db`, which the store watch
  compares by device and inode (Task 10).
- **`Engine::spawner`**: the process port, for the notifier (D6.2).
- **`LiveReading` and `Engine::live_reading`** (D3 change 5): the managed live account and its
  decision-grade windows relevant under `models`, for the `limit` notice (D6.1).
- **`Store::take_lease`**: §6.1's single lease statement, for the `limit` markers (D6.3).
- **`tagteam_engine::daemon`** (Decision 2): `daemon.lock`'s and the socket's paths, the
  daemon's lock record, the `PATH` lookup, and `MAX_SOCKET_PATH`, the longest socket path
  `sun_path` holds, which plan B's server and `doctor` check.

**Readings this task commits to:**
- **`data_version` is per connection.** It changes when another connection commits, and never
  for this connection's own commits (D4.4). A value read after `forget_store` comes from a new
  connection and cannot be compared with one read before.
- **`forget_store` drops only the engine's handle.** A caller still holding an `Arc<Store>`
  keeps it until it drops it. Nothing is closed under it.
- **`live_reading` finds the live account as a tick does.** It reads the live identity
  (`read_live_identity`), takes its `identity_key`, and looks for the provider's row with that
  key (§11.2 step 2). An unreadable live identity is an error, as it is for a tick. With no
  store, it reads no live identity and returns `None`. Its windows are `decision_windows`
  under `models`, filtered by `is_relevant` (§8.2). A spend window is therefore never there,
  and a scoped window only when `models` names it.
- **`take_lease` is the statement alone.** It is one `INSERT … ON CONFLICT … WHERE`, in
  autocommit, so it is atomic without a transaction. `true` exactly when `changes() == 1`.
  Expiries are epoch ms, as `leases.expires_at` holds them (`store/usage.rs`'s module doc).
- **`find_on_path` already exists** in `tagteam_provider::process` with exactly the skeleton's
  signature (§12.1's lookup). `tagteam_engine::daemon` re-exports it rather than writing a
  second one, so callers may use either path.
- **`MAX_SOCKET_PATH` is a `cfg(target_os)` constant:** 107 bytes on Linux (`sun_path` is 108
  with its NUL) and 103 elsewhere (macOS's is 104). The engine forbids `unsafe`, so it cannot
  size a `sockaddr_un`; its test checks the constant against std's own bound instead
  (`SocketAddr::from_pathname`, which refuses what `bind(2)` would).
- **The daemon record** follows `write_holder` and `read_holder`. It is one JSON line through
  a second descriptor, with the version JSON-escaped. A zero-length file is `Absent`, and any
  other shape, an engine record without a version among them, is `Unreadable`.

**Files:**
- Create: `crates/tagteam-engine/src/daemon.rs`
- Modify: `crates/tagteam-engine/src/lib.rs` (`pub mod daemon;` after `pub mod config;`)
- Modify: `crates/tagteam-engine/src/engine.rs`:
  - `fn store_path` becomes `pub fn store_file`, with its two call sites in `store` and
    `existing_store`
  - add `data_version`, `forget_store` and `spawner`
- Modify: `crates/tagteam-engine/src/store/mod.rs` (`Store::data_version`, after `quick_check`)
- Modify: `crates/tagteam-engine/src/store/usage.rs` (`Store::take_lease`, after
  `usage_lease_expires_at`)
- Modify: `crates/tagteam-engine/src/views.rs` (`LiveReading` after `HistoryView`;
  `Engine::live_reading` after `decision_windows`)
- Test:
  - `crates/tagteam-engine/tests/store.rs`
  - `crates/tagteam-engine/tests/store_usage.rs`
  - `crates/tagteam-engine/tests/engine_basics.rs`
  - `crates/tagteam-engine/tests/views_usage.rs`
  - `crates/tagteam-engine/src/daemon.rs` (`mod tests`)

**Interfaces:**
- Consumes:
  - Task 1's `engine.rs`: this task edits the file after it.
  - Existing: `Engine::existing_store`, `Engine::decision_windows(&self, row: &AccountRow, models: &[String]) -> Result<Option<Vec<Window>>, EngineError>`,
    `Engine::read_live_identity(&self, p: &dyn Provider) -> Result<Option<Identity>, EngineError>` (`switch.rs`, `pub(crate)`),
    `Store::find_by_identity_key`, `tagteam_core::usage::is_relevant`, `TAKE_LEASE_SQL`
    (`store/usage.rs`), `tagteam_provider::process::find_on_path`.
- Produces (as the skeleton fixes them):
  - `impl Engine { pub fn data_version(&self) -> Result<Option<i64>, EngineError>; pub fn forget_store(&self); pub fn store_file(&self) -> PathBuf; pub fn spawner(&self) -> Arc<dyn ProcessSpawner>; }`
  - `tagteam_engine::views::LiveReading { account: AccountId, position: u32, alias: Option<String>, email: Option<String>, label: String, windows: Vec<Window> }` (`Debug, Clone, PartialEq`)
  - `impl Engine { pub fn live_reading(&self, provider: &ProviderId, models: &[String]) -> Result<Option<LiveReading>, EngineError>; }`
  - `impl Store { pub fn data_version(&self) -> Result<i64, StoreError>; pub fn take_lease(&self, name: &str, holder: &str, now_ms: i64, expires_at_ms: i64) -> Result<bool, StoreError>; }`
  - `tagteam_engine::daemon::{daemon_lock_path, daemon_socket_path, MAX_SOCKET_PATH, DaemonRecord, write_daemon_record, read_daemon_record, find_on_path}` with the skeleton's signatures

- [ ] **Step 1: Write the failing tests for the store and the engine hooks**

Append to `crates/tagteam-engine/tests/store.rs`:

```rust
#[test]
fn data_version_moves_for_another_connections_commit_and_never_for_its_own() {
    // D3 change 4, D4.4: the store watch's signal.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    let first = s.data_version().unwrap();
    s.set_unhealthy_ticks(&cc(), 1).unwrap();
    assert_eq!(s.data_version().unwrap(), first, "its own commit");
    let other = Store::open(&path).unwrap();
    other.set_unhealthy_ticks(&cc(), 2).unwrap();
    let second = s.data_version().unwrap();
    assert_ne!(second, first, "another connection committed");
    assert_eq!(s.data_version().unwrap(), second, "and nothing since");
}
```

Append to `crates/tagteam-engine/tests/store_usage.rs`:

```rust
#[test]
fn take_lease_is_the_single_lease_statement() {
    // §6.1: held when the statement changed one row, because the lease was free or its
    // holder's expiry had passed. D6.3's notification markers are such leases.
    let (_d, path, s) = open();
    let name = "notify:limit:a:7d:1790291630";
    assert!(s.take_lease(name, "one", T_MS, T_MS + 60_000).unwrap(), "free");
    assert!(
        !s.take_lease(name, "two", T_MS + 59_999, T_MS + 120_000).unwrap(),
        "held until it expires"
    );
    assert!(
        !s.take_lease(name, "one", T_MS + 1, T_MS + 120_000).unwrap(),
        "by its own holder too"
    );
    assert!(
        s.take_lease(name, "two", T_MS + 60_000, T_MS + 120_000)
            .unwrap(),
        "expired"
    );
    let (holder, expires_at): (String, i64) = raw(&path)
        .query_row(
            "SELECT holder, expires_at FROM leases WHERE name = ?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((holder.as_str(), expires_at), ("two", T_MS + 120_000));
    assert!(
        s.take_lease("another", "one", T_MS, T_MS + 1).unwrap(),
        "each name is its own lease"
    );
}
```

In `crates/tagteam-engine/tests/engine_basics.rs`, replace the imports:

```rust
use common::Fx;
use serde_json::json;
use tagteam_core::{AccountId, OracleVerdict};
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::oracle::verdict;
use tagteam_engine::store::{LoginMeta, NewAccount};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::{Identity, Provider};
```

with

```rust
use std::fs;
use std::sync::Arc;
use std::time::Duration;

use common::Fx;
use serde_json::json;
use tagteam_core::{AccountId, OracleVerdict};
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::oracle::verdict;
use tagteam_engine::store::{LoginMeta, NewAccount, Store};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::process::{Captured, SpawnSpec};
use tagteam_provider::{Cancel, Identity, Provider};
```

and append:

```rust
#[test]
fn data_version_is_none_without_a_store_and_moves_only_for_another_connection() {
    // D3 change 4: the daemon's own writes never trigger the store watch (D4.4).
    let fx = Fx::new();
    assert_eq!(fx.engine.data_version().unwrap(), None);
    assert_eq!(fx.engine.store_file(), fx.env.data_dir().join("tagteam.db"));
    assert!(!fx.engine.store_file().exists(), "never creates the store");
    fx.add("a@x.co", "rt-a");
    let first = fx.engine.data_version().unwrap().unwrap();
    fx.engine
        .store()
        .unwrap()
        .set_unhealthy_ticks(&fx.provider(), 1)
        .unwrap();
    assert_eq!(fx.engine.data_version().unwrap(), Some(first), "its own write");
    Store::open(&fx.engine.store_file())
        .unwrap()
        .set_unhealthy_ticks(&fx.provider(), 2)
        .unwrap();
    let second = fx.engine.data_version().unwrap().unwrap();
    assert_ne!(second, first, "another connection committed");
}

#[test]
fn forget_store_makes_the_next_use_open_the_file_again() {
    // Decision 11: a store deleted and created again under a running engine (its data
    // directory removed, then a first `add`) is read from the new file, never from the
    // unlinked one the cached connection still holds.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let file = fx.engine.store_file();
    let remove_all = || {
        for name in ["tagteam.db", "tagteam.db-wal", "tagteam.db-shm"] {
            let _ = fs::remove_file(fx.env.data_dir().join(name));
        }
    };
    let before = fx.engine.existing_store().unwrap().unwrap();
    remove_all();
    let replacement = Store::open(&file).unwrap();
    common::add(&replacement, &fx.provider(), "other", "z@x.co", 1);
    drop(replacement);
    assert!(
        Arc::ptr_eq(&fx.engine.existing_store().unwrap().unwrap(), &before),
        "still the cached connection"
    );
    drop(before);
    fx.engine.forget_store();
    let ids: Vec<AccountId> = fx
        .engine
        .existing_store()
        .unwrap()
        .unwrap()
        .accounts(&fx.provider())
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, [AccountId::from_string("other")]);
    // A file that is gone is no store, and nothing creates it.
    fx.engine.forget_store();
    remove_all();
    assert!(fx.engine.existing_store().unwrap().is_none());
    assert!(!file.exists());
}

#[test]
fn the_spawner_is_the_port_the_engine_was_built_with() {
    // D6.2: the notifier delivers through the engine's own port, so tests script it.
    let fx = Fx::new();
    fx.spawner.push(Captured::Exited {
        code: Some(0),
        signal: None,
        stdout: Vec::new(),
        stderr: Vec::new(),
    });
    let spec = SpawnSpec {
        program: "/usr/bin/osascript".into(),
        ..SpawnSpec::default()
    };
    let ran = fx
        .engine
        .spawner()
        .run_captured(&spec, Duration::from_secs(5), &Cancel::new());
    assert!(matches!(ran, Captured::Exited { code: Some(0), .. }), "{ran:?}");
    assert_eq!(fx.spawner.specs(), [spec]);
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test store --test store_usage --test engine_basics`
Expected: the build fails: `no method named `data_version` found for struct `Store``,
`no method named `take_lease``, and `no method named `store_file`` / `forget_store` / `spawner`
found for struct `Engine``.

- [ ] **Step 3: Implement the store and engine hooks**

In `crates/tagteam-engine/src/store/mod.rs`, after `pub fn quick_check(&self) …` and before
`fn lock(&self)`:

```rust
    /// SQLite's `PRAGMA data_version` for this connection (D3 change 4): it changes when
    /// another connection commits to the file, and never for this connection's own commits.
    /// A value is this connection's only; another connection's are not comparable with it.
    pub fn data_version(&self) -> Result<i64, StoreError> {
        Ok(self
            .lock()
            .query_row("PRAGMA data_version", [], |r| r.get(0))?)
    }
```

In `crates/tagteam-engine/src/store/usage.rs`, in `impl Store`, after
`pub fn usage_lease_expires_at(…)`:

```rust
    /// §6.1's lease statement, alone: `true` when this call took `name` for `holder` until
    /// `expires_at_ms`, because no lease of that name existed or its expiry was at or before
    /// `now_ms` (`changes() = 1`). Times are epoch ms. One statement in autocommit, so it is
    /// atomic across processes. D6.3's notification markers are such leases.
    pub fn take_lease(
        &self,
        name: &str,
        holder: &str,
        now_ms: i64,
        expires_at_ms: i64,
    ) -> Result<bool, StoreError> {
        let taken = self.lock().execute(
            TAKE_LEASE_SQL,
            params![name, holder, expires_at_ms, now_ms],
        )?;
        Ok(taken == 1)
    }
```

In `crates/tagteam-engine/src/engine.rs`, replace:

```rust
    fn store_path(&self) -> PathBuf {
        self.env.data_dir().join("tagteam.db")
    }
```

with:

```rust
    /// `<data>/tagteam.db` (§5), whether or not it exists. The daemon's store watch compares
    /// its device and inode (Decision 11).
    pub fn store_file(&self) -> PathBuf {
        self.env.data_dir().join("tagteam.db")
    }

    /// D3 change 4: SQLite's `PRAGMA data_version` on this engine's own connection, which
    /// changes when another connection commits and never for this engine's own writes (D4.4).
    /// `None` while there is no store; never creates one (§5). A value read after
    /// `forget_store` comes from a new connection and is not comparable with one read before.
    pub fn data_version(&self) -> Result<Option<i64>, EngineError> {
        match self.existing_store()? {
            Some(store) => Ok(Some(store.data_version()?)),
            None => Ok(None),
        }
    }

    /// Decision 11: drops the engine's cached connection, so the next `store`,
    /// `existing_store` or `data_version` opens `store_file()` again. A store another
    /// process deleted or replaced is then the one read and written, never the unlinked file.
    /// A caller still holding an `Arc<Store>` keeps it until it drops it.
    pub fn forget_store(&self) {
        *self.store.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// The process port this engine spawns through (§4.2): the daemon's notifier delivers
    /// through it (D6.2), so tests script it.
    pub fn spawner(&self) -> Arc<dyn ProcessSpawner> {
        self.spawner.clone()
    }
```

and in `store` and `existing_store`, replace `&self.store_path()` with `&self.store_file()` (two
places: `Store::open(&self.store_path())` and `Store::open_existing(&self.store_path())`).

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p tagteam-engine --test store --test store_usage --test engine_basics`
Expected: PASS.

- [ ] **Step 5: Write the failing tests for the live reading and the daemon module**

In `crates/tagteam-engine/tests/views_usage.rs`, add `LiveReading` to the views import:

```rust
use tagteam_engine::views::{
    HistoryView, HistoryWindow, LiveReading, NO_DATA, StatusView, StatuslineView, UsageStatus,
    UsageView,
};
```

and append:

```rust
#[test]
fn live_reading_is_the_managed_live_accounts_relevant_decision_grade_windows() {
    // D3 change 5, for D6.1's `limit`: the reading a tick's decision uses, from the store
    // alone. A spend window is never relevant, and a scoped one only when the models name it
    // (§8.2).
    let fx = Fx::new();
    assert_eq!(fx.engine.live_reading(&fx.provider(), &[]).unwrap(), None);
    assert!(!fx.env.data_dir().exists(), "never creates the store");
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live, at position 2
    assert_eq!(
        fx.engine.live_reading(&fx.provider(), &[]).unwrap(),
        None,
        "never read"
    );
    let windows = reading(T0, 40.0, 93.0); // 5h, 7d, Fable, spend
    record(&fx, &b, &windows, T0, T0 + 300);
    let store = fx.engine.store().unwrap();
    store.set_alias(&b, Some("spare")).unwrap();
    let label = store.account(&b).unwrap().unwrap().label;
    let expected = |windows: &[Window]| LiveReading {
        account: b.clone(),
        position: 2,
        alias: Some("spare".into()),
        email: Some("b@x.co".into()),
        label: label.clone(),
        windows: windows.to_vec(),
    };
    at(&fx, T0 + 100);
    assert_eq!(
        fx.engine.live_reading(&fx.provider(), &[]).unwrap(),
        Some(expected(&windows[..2]))
    );
    assert_eq!(
        fx.engine
            .live_reading(&fx.provider(), &["fable".to_owned()])
            .unwrap(),
        Some(expected(&windows[..3]))
    );
    assert!(fx.http.requests().is_empty(), "it never fetches");
}

#[test]
fn live_reading_is_none_for_an_unmanaged_login_or_a_reading_past_decision_grade() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a"); // live
    record(&fx, &a, &reading(T0, 40.0, 93.0), T0, T0 + 300);
    at(&fx, T0 + 100);
    assert!(
        fx.engine
            .live_reading(&fx.provider(), &[])
            .unwrap()
            .is_some()
    );
    at(&fx, T0 + 4_000);
    assert_eq!(
        fx.engine.live_reading(&fx.provider(), &[]).unwrap(),
        None,
        "past §8.4's hour, with nothing that extends it"
    );
    at(&fx, T0 + 100);
    fx.login("z@x.co", "rt-z");
    assert_eq!(
        fx.engine.live_reading(&fx.provider(), &[]).unwrap(),
        None,
        "a live login tagteam does not manage"
    );
}
```

Register the new module in `crates/tagteam-engine/src/lib.rs`, after `pub mod config;`:

```rust
pub mod daemon;
```

Then create `crates/tagteam-engine/src/daemon.rs` containing only this test module (Step 7 puts
the implementation above it):

```rust
#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn the_daemon_s_lock_and_socket_live_in_the_data_directory() {
        // D4.1, D5.1: derived from the environment alone.
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        assert_eq!(
            daemon_lock_path(&env),
            env.data_dir().join("locks/daemon.lock")
        );
        assert_eq!(daemon_socket_path(&env), env.data_dir().join("daemon.sock"));
        assert!(!env.data_dir().exists(), "naming them creates nothing");
    }

    #[test]
    fn the_longest_socket_path_is_the_longest_sun_path_holds_with_its_nul() {
        // D5.1: a path of `MAX_SOCKET_PATH` bytes still fits `sockaddr_un`, and one byte more
        // does not. std's `SocketAddr` checks the bound `bind(2)` would, with no socket made.
        use std::os::unix::net::SocketAddr;
        let path = |len: usize| PathBuf::from(format!("/{}", "s".repeat(len - 1)));
        assert!(SocketAddr::from_pathname(path(MAX_SOCKET_PATH)).is_ok());
        assert!(SocketAddr::from_pathname(path(MAX_SOCKET_PATH + 1)).is_err());
    }

    #[test]
    fn the_record_is_one_json_line_that_reads_back() {
        // D4.1: `{"pid":N,"start":S,"version":"…"}`, replacing whatever a dead daemon left.
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let path = daemon_lock_path(&env);
        let lock = FlockGuard::try_lock(&path).unwrap().unwrap();
        fs::write(
            &path,
            "{\"pid\":4294967295,\"start\":18446744073709551615,\"version\":\"a dead daemon's, much longer\"}\n",
        )
        .unwrap();
        write_daemon_record(&lock, "20261014.2045.0").unwrap();
        let me = ProcessStamp::current().unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!(
                "{{\"pid\":{},\"start\":{},\"version\":\"20261014.2045.0\"}}\n",
                me.pid, me.start
            )
        );
        let expected = DaemonRecord {
            pid: me.pid,
            start: me.start,
            version: "20261014.2045.0".into(),
        };
        assert!(matches!(read_daemon_record(&path), Read::Present(r) if r == expected));
        let odd = "1.0 \"dev\" \\ build";
        write_daemon_record(&lock, odd).unwrap();
        assert!(
            matches!(read_daemon_record(&path), Read::Present(r) if r.version == odd),
            "a version is JSON-escaped"
        );
    }

    #[test]
    fn a_missing_or_empty_record_is_absent_and_any_other_shape_unreadable() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("daemon.lock");
        assert!(matches!(read_daemon_record(&path), Read::Absent));
        fs::write(&path, "").unwrap();
        assert!(matches!(read_daemon_record(&path), Read::Absent));
        for text in [
            "garbage",
            "{\"pid\":1,\"start\":2}\n",
            "{\"pid\":4294967296,\"start\":2,\"version\":\"v\"}\n",
            "{\"pid\":1,\"start\":2,\"version\":3}\n",
        ] {
            fs::write(&path, text).unwrap();
            assert!(
                matches!(read_daemon_record(&path), Read::Unreadable(_)),
                "{text}"
            );
        }
    }

    #[test]
    fn find_on_path_finds_the_first_executable_of_a_name() {
        // D6.2's `notify-send`, D9.1's `tagteam`: a file that is not executable is passed over.
        let d = tempfile::tempdir().unwrap();
        let (plain, exec) = (d.path().join("plain"), d.path().join("exec"));
        for dir in [&plain, &exec] {
            fs::create_dir(dir).unwrap();
        }
        fs::write(plain.join("notify-send"), "").unwrap();
        fs::write(exec.join("notify-send"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(exec.join("notify-send"), fs::Permissions::from_mode(0o755))
            .unwrap();
        let path = std::env::join_paths([&plain, &exec]).unwrap();
        assert_eq!(
            find_on_path("notify-send", Some(path.as_os_str())),
            Some(exec.join("notify-send"))
        );
        assert_eq!(find_on_path("notify-send", None), None, "no PATH, nothing found");
    }
}
```

- [ ] **Step 6: Run them and see them fail**

Run: `cargo test -p tagteam-engine --lib daemon::tests`
Expected: the build fails: `cannot find function `daemon_lock_path``, `cannot find value
`MAX_SOCKET_PATH``, `cannot find struct `DaemonRecord``, and so on.
Run: `cargo test -p tagteam-engine --test views_usage live_reading`
Expected: the build fails: `no `LiveReading` in `views``.

- [ ] **Step 7: Implement the live reading and the daemon module**

In `crates/tagteam-engine/src/views.rs`, after `pub struct HistoryView { … }`:

```rust
/// D3 change 5: the provider's managed live account and its reading, as a tick's decision
/// uses it, for D6.1's `limit` notice. Its names are for the notice's text (D6.2): position,
/// alias, email and label.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveReading {
    pub account: AccountId,
    pub position: u32,
    pub alias: Option<String>,
    pub email: Option<String>,
    pub label: String,
    /// The decision-grade (§8.4) windows relevant under `models` (§8.2, `is_relevant`).
    pub windows: Vec<Window>,
}
```

In the `impl Engine` block, after `pub fn decision_windows(…)`:

```rust
    /// D3 change 5: the managed live account, found as a tick finds it (its identity key,
    /// §11.2 step 2), and its decision-grade windows (§8.4) relevant under `models` (§8.2).
    /// `None` with no store, with an absent or unmanaged live login, or without a
    /// decision-grade reading. An unreadable live identity is an error, as for a tick. Reads
    /// the live identity and the store only: it never fetches and never creates the store.
    pub fn live_reading(
        &self,
        provider: &ProviderId,
        models: &[String],
    ) -> Result<Option<LiveReading>, EngineError> {
        let p = self.provider(provider)?;
        let Some(store) = self.existing_store()? else {
            return Ok(None);
        };
        let Some(identity) = self.read_live_identity(p.as_ref())? else {
            return Ok(None);
        };
        let key = p.identity_key(&identity);
        let Some(row) = store.find_by_identity_key(provider, key.as_str())? else {
            return Ok(None);
        };
        let Some(windows) = self.decision_windows(&row, models)? else {
            return Ok(None);
        };
        Ok(Some(LiveReading {
            windows: windows
                .into_iter()
                .filter(|w| is_relevant(w, models))
                .collect(),
            account: row.id,
            position: row.position,
            alias: row.alias,
            email: row.email,
            label: row.label,
        }))
    }
```

Fill `crates/tagteam-engine/src/daemon.rs` above its test module:

```rust
//! D4.1 and D5.1: where the daemon keeps its lock and its socket, and the record its lock
//! holds. They live in the engine (daemon plan A, Decision 2) because `doctor` reads them
//! (D10), and the engine cannot depend on the daemon crate.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use tagteam_provider::{Env, FlockGuard, ProcessStamp, Read, ReadError};

use crate::error::EngineError;

/// The first executable regular file called `name` on a `PATH` value (§12.1's lookup):
/// D6.2's `notify-send` and D9.1's `tagteam`. The provider crate's, re-exported.
pub use tagteam_provider::process::find_on_path;

/// D4.1: `<data>/locks/daemon.lock` (§5 gains it). A try-only `flock`, like the engine locks
/// (§11.1): one daemon per data directory.
pub fn daemon_lock_path(env: &Env) -> PathBuf {
    env.data_dir().join("locks").join("daemon.lock")
}

/// D5.1: `<data>/daemon.sock`, next to the store, derived from the environment alone.
pub fn daemon_socket_path(env: &Env) -> PathBuf {
    env.data_dir().join("daemon.sock")
}

/// D5.1: the longest socket path, in bytes, that `sockaddr_un.sun_path` holds with its NUL:
/// 108 bytes on Linux. Plan B's server refuses a longer path rather than bind a truncated one,
/// and `doctor` reports it (D10). A constant, since the engine forbids the `unsafe` that sizing
/// a `sockaddr_un` would take.
#[cfg(target_os = "linux")]
pub const MAX_SOCKET_PATH: usize = 107;

/// D5.1: the longest socket path, in bytes, that `sockaddr_un.sun_path` holds with its NUL:
/// 104 bytes on macOS and the BSDs.
#[cfg(not(target_os = "linux"))]
pub const MAX_SOCKET_PATH: usize = 103;

/// What a running daemon writes into `daemon.lock` (D4.1): its pid and start time, the start
/// taken as for the `switch_journal` holder (§12.6), and its tagteam version. Never used for
/// exclusion; `doctor` and `daemon status` read it (D8.2, D10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonRecord {
    pub pid: u32,
    pub start: u64,
    pub version: String,
}

/// Replaces the held lock file's whole content with `{"pid":N,"start":S,"version":"…"}\n`,
/// through a second descriptor: the lock stays on the first, as for an engine's record
/// (§11.1).
pub fn write_daemon_record(lock: &FlockGuard, version: &str) -> Result<(), EngineError> {
    let stamp = ProcessStamp::current()?;
    let line = format!(
        "{{\"pid\":{},\"start\":{},\"version\":{}}}\n",
        stamp.pid,
        stamp.start,
        serde_json::Value::from(version)
    );
    let mut file = OpenOptions::new().write(true).open(lock.path())?;
    file.set_len(0)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// The record `write_daemon_record` left at `path`, read without trying the lock. `Absent`:
/// no file, or an empty one. `Unreadable`: anything that is not that shape. Never proof that
/// the daemon runs: judge that by an exact pid and start-time match (§12.6).
pub fn read_daemon_record(path: &Path) -> Read<DaemonRecord> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Read::Absent,
        Err(e) => {
            return Read::Unreadable(ReadError::new(path.display().to_string(), e.to_string()));
        }
    };
    if bytes.is_empty() {
        return Read::Absent;
    }
    let v: Option<serde_json::Value> = serde_json::from_slice(&bytes).ok();
    let pid = v
        .as_ref()
        .and_then(|v| v["pid"].as_u64())
        .and_then(|p| u32::try_from(p).ok());
    let start = v.as_ref().and_then(|v| v["start"].as_u64());
    let version = v.as_ref().and_then(|v| v["version"].as_str());
    match (pid, start, version) {
        (Some(pid), Some(start), Some(version)) => Read::Present(DaemonRecord {
            pid,
            start,
            version: version.to_owned(),
        }),
        _ => Read::Unreadable(ReadError::new(
            path.display().to_string(),
            "it is not a pid, a start time and a version",
        )),
    }
}
```

- [ ] **Step 8: Run the tests and see them pass**

Run: `cargo test -p tagteam-engine --lib daemon::tests`, then
`cargo test -p tagteam-engine --test views_usage --test engine_basics --test store --test store_usage`.
Then `cargo fmt --all && cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`.
Expected: PASS.

- [ ] **Step 9: Commit** (the controller commits; implementers do not)

`git commit -m "Add the engine hooks the daemon schedules through"`

---

### Task 4: Keychain read counts

**Depends on:** none

D3 change 6 and Decision 7: every `Keychain` instance counts its **secret reads** (`find`) by
how they ended. A read is **answered** when it returned a secret, and **locked** when it was
refused because the keychain is locked (rc 36) or it timed out. An absent item (rc 44) counts as
neither, "since a locked keychain can still report absence". Attribute-only probes (`exists`,
`service_has_items`), the lock check, writes and deletes are not counted at all. The counts are
per instance, not process globals, because engine tests run in parallel threads of one process.
`Keychain::read_counts()` exposes them, and defaults to `None`. Only the daemon's keychain
backoff reads them (Task 9), as a `ReadTally` taken before a run and one taken after.

**Readings this task commits to:**
- **`SecurityCli`: answered means the read came back `Present`.** rc 0 with ordinary output is
  answered. rc 0 whose hex-looking output `-g` confirms is answered once. rc 0 whose `-g` call
  fails or disagrees returned no secret and counts as neither. A write's read-back is a `find`,
  so a successful `upsert` counts one answered read. That is how a switch's writes end a backoff
  (D14).
- **`SecurityCli`: locked means rc 36 from the `-w` read, or that read's `TimedOut`.**
  `SpawnFailed` and any other rc count as neither.
- **The fakes count what they model.** `FakeKeychain::set_locked(true)` and `FileKeychain`'s
  `LOCKED` file make every `find` count as locked, absent items included. That is stricter than
  `security`, which still answers rc 44 there, but the fakes refused those reads already. A
  `set_unreadable` item is an injected failure, and an I/O error reading a `FileKeychain` item
  is a failure too; neither is counted.
- **Ordering.** The counts use `SeqCst`, as `FakeKeychain`'s flags do. A collection's threads
  join before the job reads the tally, so any ordering would do.
- **Two small additions, both in the Interfaces:** `ReadTally` implements `std::ops::Add`, so
  a provider job sums the tallies of the keychains it was handed (Decision 7, Task 9).
  `ReadCounts` and `ReadTally` are also re-exported at the crate root beside `Keychain`.

**Files:**
- Modify: `crates/tagteam-provider/src/keychain.rs`:
  - the atomics import
  - `ReadCounts` and `ReadTally`, after `impl std::error::Error for KeychainError {}`
  - `Keychain::read_counts`, and the `Arc<K>` forward
  - the `FakeKeychain` field, `find` and `read_counts`, and its private `blocked`, which is
    removed
  - the `FileKeychain` field, `new`, `find` and `read_counts`
  - tests
- Modify: `crates/tagteam-provider/src/security.rs` (the imports; `SecurityCli`'s field and both
  constructors; `find`; `read_counts`; tests)
- Modify: `crates/tagteam-provider/src/lib.rs` (`pub use keychain::{…}`)
- Test: `crates/tagteam-provider/src/keychain.rs` (`mod tests`), `crates/tagteam-provider/src/security.rs` (`mod tests`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces (`tagteam_provider::keychain`, re-exported at `tagteam_provider::`):
  - `#[derive(Debug, Default)] pub struct ReadCounts` with `pub fn new() -> Arc<Self>`,
    `pub fn record_answered(&self)`, `pub fn record_locked(&self)`, `pub fn tally(&self) -> ReadTally`
  - `#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)] pub struct ReadTally { pub answered: u64, pub locked: u64 }`
    with `pub fn since(self, before: ReadTally) -> ReadTally`, `pub fn looks_locked(self) -> bool`,
    and `impl std::ops::Add for ReadTally`
  - `trait Keychain { fn read_counts(&self) -> Option<Arc<ReadCounts>> { None } }`, forwarded by
    `impl Keychain for Arc<K>`, and returning `Some` for `SecurityCli`, `FakeKeychain` and
    `FileKeychain`

- [ ] **Step 1: Write the failing tests**

Append to `crates/tagteam-provider/src/keychain.rs`'s `mod tests`:

```rust
    #[test]
    fn a_tally_since_an_earlier_one_is_the_reads_between_them() {
        let t = |answered, locked| ReadTally { answered, locked };
        assert_eq!(t(5, 7).since(t(3, 2)), t(2, 5));
        assert_eq!(t(5, 7).since(t(5, 7)), t(0, 0));
        assert_eq!(t(1, 1).since(t(2, 2)), t(0, 0), "never wraps");
        assert_eq!(t(1, 2) + t(3, 4), t(4, 6), "two keychains' reads together");
    }

    #[test]
    fn a_run_looks_locked_when_a_read_ended_locked_and_none_was_answered() {
        // D4.1: a run whose secret reads were all absent, or that made none, changes nothing.
        let t = |answered, locked| ReadTally { answered, locked };
        assert!(t(0, 1).looks_locked());
        assert!(t(0, 3).looks_locked());
        assert!(!t(1, 3).looks_locked(), "one answered read is enough");
        assert!(!t(0, 0).looks_locked());
        assert!(!t(2, 0).looks_locked());
    }

    #[test]
    fn the_fake_counts_answered_and_locked_secret_reads_only() {
        // Decision 7: per instance, read through `read_counts`, through an `Arc` too.
        let k = Arc::new(FakeKeychain::new());
        let counts = Keychain::read_counts(&k).expect("the fake keeps counts");
        assert!(Arc::ptr_eq(&counts, &Keychain::read_counts(&*k).unwrap()));
        k.put("s", "a", b"v");
        let _ = k.find("s", "a");
        let _ = k.find("s", "missing");
        k.set_unreadable("s", "a", true);
        let _ = k.find("s", "a");
        k.set_unreadable("s", "a", false);
        assert_eq!(counts.tally(), ReadTally { answered: 1, locked: 0 });
        k.set_locked(true);
        let _ = k.find("s", "a");
        let _ = k.find("s", "missing");
        let _ = k.exists("s", "a");
        let _ = k.service_has_items("s");
        assert_eq!(counts.tally(), ReadTally { answered: 1, locked: 2 });
        assert_eq!(
            FakeKeychain::new().read_counts().unwrap().tally(),
            ReadTally::default(),
            "each instance counts its own"
        );
    }

    #[cfg(feature = "file-keychain")]
    #[test]
    fn file_keychain_counts_answered_and_locked_secret_reads_only() {
        let d = tempfile::tempdir().unwrap();
        let k = FileKeychain::new(d.path());
        let counts = k.read_counts().unwrap();
        k.upsert("s", "a", b"v").unwrap();
        let _ = k.find("s", "a");
        let _ = k.find("s", "missing");
        let _ = k.exists("s", "a");
        // An item that cannot be read is a failure, not a lock.
        std::fs::create_dir(d.path().join(format!("{}.{}", hex::encode("s"), hex::encode("dir"))))
            .unwrap();
        assert!(matches!(k.find("s", "dir"), Read::Unreadable(_)));
        assert_eq!(counts.tally(), ReadTally { answered: 1, locked: 0 });
        std::fs::write(d.path().join("LOCKED"), "").unwrap();
        let _ = k.find("s", "a");
        let _ = k.exists("s", "a");
        assert_eq!(counts.tally(), ReadTally { answered: 1, locked: 1 });
        assert_eq!(
            FileKeychain::new(d.path()).read_counts().unwrap().tally(),
            ReadTally::default(),
            "per instance, not per directory"
        );
    }
```

In the same module's `a_keychain_that_cannot_enumerate_a_service_refuses_both`, after its last
assertion, add:

```rust
        // Nor does it count its secret reads: only a keychain that keeps counts has any.
        assert!(Bare.read_counts().is_none());
        assert!(Arc::new(Bare).read_counts().is_none());
```

In `crates/tagteam-provider/src/security.rs`'s `mod tests`, add to its imports (after
`use std::sync::{Arc, Mutex};`):

```rust
    use crate::keychain::ReadTally;
```

and append to the module:

```rust
    #[test]
    fn secret_reads_are_counted_by_how_they_ended() {
        // D3 change 6: answered (rc 0, a secret returned) and locked (rc 36, or a timeout).
        // Absent (rc 44), any other rc and a spawn failure count as neither.
        let s = Scripted::default()
            .then(ok(b"{\"a\":1}\n"))
            .then(rc(36))
            .then(RunResult::TimedOut)
            .then(rc(44))
            .then(rc(1))
            .then(RunResult::SpawnFailed("x".into()));
        let k = cli(&s, None);
        let counts = k.read_counts().expect("the security driver keeps counts");
        let mut seen = Vec::new();
        for _ in 0..6 {
            let _ = k.find("svc", "acct");
            seen.push(counts.tally());
        }
        let t = |answered, locked| ReadTally { answered, locked };
        assert_eq!(
            seen,
            [t(1, 0), t(1, 1), t(1, 2), t(1, 2), t(1, 2), t(1, 2)]
        );
    }

    #[test]
    fn an_answer_counts_once_the_read_gives_a_secret() {
        // rc 0 whose hex-looking output `-g` confirms is one answered read; one whose `-g`
        // fails gave no secret, so it counts as neither.
        let data = [0xde, 0xad, 0xbe, 0xef];
        let s = Scripted::default()
            .then(ok(format!("{}\n", hex::encode(data)).as_bytes()))
            .then(og_hex("DEADBEEF"))
            .then(ok(b"cafe\n"))
            .then(rc(1));
        let k = cli(&s, None);
        let counts = k.read_counts().unwrap();
        assert!(k.find("svc", "acct").is_present());
        assert_eq!(counts.tally(), ReadTally { answered: 1, locked: 0 });
        assert!(matches!(k.find("svc", "acct"), Read::Unreadable(_)));
        assert_eq!(counts.tally(), ReadTally { answered: 1, locked: 0 });
    }

    #[test]
    fn a_writes_read_back_is_an_answered_secret_read() {
        // A switch writes the live credential and reads it back: a run that switched read a
        // secret (D4.1, D14).
        let s = Scripted::default().then(ok(b"")).then(ok(b"{}\n"));
        let k = cli(&s, None);
        k.upsert("svc", "acct", b"{}").unwrap();
        assert_eq!(
            k.read_counts().unwrap().tally(),
            ReadTally { answered: 1, locked: 0 }
        );
    }

    #[test]
    fn attribute_reads_the_lock_check_and_deletes_are_never_counted() {
        // Only `find-generic-password -w` reads a secret (D3 change 6).
        let s = Scripted::default()
            .then(ok(b""))
            .then(rc(36))
            .then(rc(36))
            .then(rc(36))
            .then(rc(36));
        let k = cli(&s, None);
        assert!(k.exists("svc", "acct").is_present());
        assert!(matches!(k.exists("svc", "acct"), Read::Unreadable(_)));
        assert!(matches!(k.service_has_items("svc"), Read::Unreadable(_)));
        assert_eq!(k.lock_state(), LockState::Locked);
        assert!(k.delete("svc", "acct").is_err());
        assert_eq!(k.read_counts().unwrap().tally(), ReadTally::default());
    }

    #[test]
    fn a_locked_oauth_read_beside_an_absent_managed_key_still_looks_locked() {
        // D4.1, D14: a locked keychain still reports an absent item (rc 44), which changes
        // nothing; one answered read in the same run means the keychain is not locked.
        let s = Scripted::default()
            .then(rc(36))
            .then(rc(44))
            .then(rc(36))
            .then(ok(b"sk-ant-api03-abc\n"));
        let k = cli(&s, None);
        let counts = k.read_counts().unwrap();
        let before = counts.tally();
        let _ = k.find("Claude Code-credentials", "me");
        let _ = k.find("Claude Code", "me");
        assert!(counts.tally().since(before).looks_locked());
        let before = counts.tally();
        let _ = k.find("Claude Code-credentials", "me");
        let _ = k.find("Claude Code", "me");
        assert!(!counts.tally().since(before).looks_locked());
    }
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-provider --features file-keychain --lib keychain::tests`
Expected: the build fails: `cannot find struct, variant or union type `ReadTally`` and
`no method named `read_counts``.

- [ ] **Step 3: Implement**

In `crates/tagteam-provider/src/keychain.rs`, the import:

```rust
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
```

becomes

```rust
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
```

After `impl std::error::Error for KeychainError {}`:

```rust
/// D3 change 6, Decision 7: how one keychain instance's secret reads (`find`) ended. The
/// daemon's keychain backoff compares a tally taken before a provider's run with one taken
/// after it (D4.1); nothing else reads them.
#[derive(Debug, Default)]
pub struct ReadCounts {
    /// Reads that returned a secret.
    answered: AtomicU64,
    /// Reads refused because the keychain is locked (rc 36), or that timed out.
    locked: AtomicU64,
}

impl ReadCounts {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A secret read that returned a secret (rc 0).
    pub fn record_answered(&self) {
        self.answered.fetch_add(1, Ordering::SeqCst);
    }

    /// A secret read the locked keychain refused (rc 36), or one that timed out.
    pub fn record_locked(&self) {
        self.locked.fetch_add(1, Ordering::SeqCst);
    }

    pub fn tally(&self) -> ReadTally {
        ReadTally {
            answered: self.answered.load(Ordering::SeqCst),
            locked: self.locked.load(Ordering::SeqCst),
        }
    }
}

/// The counts at one moment, or the reads between two moments (`since`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadTally {
    pub answered: u64,
    pub locked: u64,
}

impl ReadTally {
    /// The reads made between `before` and `self`.
    pub fn since(self, before: ReadTally) -> ReadTally {
        ReadTally {
            answered: self.answered.saturating_sub(before.answered),
            locked: self.locked.saturating_sub(before.locked),
        }
    }

    /// D4.1: some secret read ended locked and none was answered. Reads that found an item
    /// absent count as neither, so a run whose reads were all absent, or that made none, does
    /// not look locked.
    pub fn looks_locked(self) -> bool {
        self.locked > 0 && self.answered == 0
    }
}

/// Two instances' reads together: the daemon sums the keychains it was handed (Decision 7).
impl std::ops::Add for ReadTally {
    type Output = ReadTally;

    fn add(self, other: ReadTally) -> ReadTally {
        ReadTally {
            answered: self.answered.saturating_add(other.answered),
            locked: self.locked.saturating_add(other.locked),
        }
    }
}
```

At the end of `pub trait Keychain`, after `service_has_items`'s default:

```rust
    /// The counts of this instance's secret reads (`find`), when it keeps them (D3 change 6,
    /// Decision 7). The default keeps none.
    fn read_counts(&self) -> Option<Arc<ReadCounts>> {
        None
    }
```

At the end of `impl<K: Keychain + ?Sized> Keychain for Arc<K>`:

```rust
    fn read_counts(&self) -> Option<Arc<ReadCounts>> {
        (**self).read_counts()
    }
```

`FakeKeychain`: add the field after `unlock_attempts: AtomicUsize,`:

```rust
    /// Decision 7: its secret reads, by how they ended.
    counts: Arc<ReadCounts>,
```

Remove its private helper, which `find` was its only caller of:

```rust
    fn blocked(&self, s: &str, a: &str) -> bool {
        self.locked.load(Ordering::SeqCst) || self.unreadable.lock().unwrap().contains(&key(s, a))
    }
```

and replace its `find`:

```rust
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        if self.blocked(s, a) {
            return locked_read();
        }
        match self.get(s, a) {
            Some(v) => Read::Present(v),
            None => Read::Absent,
        }
    }
```

with:

```rust
    /// A locked fake refuses every secret read, counted as locked. An item marked unreadable
    /// is an injected failure: refused the same way, and not counted. A secret returned is
    /// counted as answered; an absent item is not counted.
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        if self.locked.load(Ordering::SeqCst) {
            self.counts.record_locked();
            return locked_read();
        }
        if self.unreadable.lock().unwrap().contains(&key(s, a)) {
            return locked_read();
        }
        match self.get(s, a) {
            Some(v) => {
                self.counts.record_answered();
                Read::Present(v)
            }
            None => Read::Absent,
        }
    }
```

and add at the end of `impl Keychain for FakeKeychain`:

```rust
    fn read_counts(&self) -> Option<Arc<ReadCounts>> {
        Some(self.counts.clone())
    }
```

`FakeKeychain` keeps `#[derive(Default)]`: `Arc<ReadCounts>` is `Default`.

`FileKeychain`: the struct and `new` become:

```rust
#[cfg(feature = "file-keychain")]
pub struct FileKeychain {
    dir: std::path::PathBuf,
    /// Decision 7: its secret reads, by how they ended. Per instance, not per directory.
    counts: Arc<ReadCounts>,
}

#[cfg(feature = "file-keychain")]
impl FileKeychain {
    pub fn new(dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            counts: ReadCounts::new(),
        }
    }
```

(the rest of that `impl` is unchanged). Its `find` becomes:

```rust
    /// A `LOCKED` file refuses every secret read, counted as locked; a secret returned is
    /// counted as answered. An absent item, and a file that cannot be read, are not counted.
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        if self.locked() {
            self.counts.record_locked();
            return locked_read();
        }
        match std::fs::read(self.path(s, a)) {
            Ok(v) => {
                self.counts.record_answered();
                Read::Present(v)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Read::Absent,
            Err(e) => Read::Unreadable(ReadError::new("keychain", e.to_string())),
        }
    }
```

and add at the end of `impl Keychain for FileKeychain`:

```rust
    fn read_counts(&self) -> Option<Arc<ReadCounts>> {
        Some(self.counts.clone())
    }
```

In `crates/tagteam-provider/src/security.rs`, the imports:

```rust
use std::sync::mpsc;
```

becomes

```rust
use std::sync::{Arc, mpsc};
```

and

```rust
use crate::keychain::{
    Keychain, KeychainError, LockState, empty_service_err, empty_service_read, locked_err,
};
```

becomes

```rust
use crate::keychain::{
    Keychain, KeychainError, LockState, ReadCounts, empty_service_err, empty_service_read,
    locked_err,
};
```

`SecurityCli` and its constructors become:

```rust
pub struct SecurityCli {
    runner: Box<dyn Runner>,
    keychain_file: Option<PathBuf>,
    /// D3 change 6: how this driver's secret reads ended (Decision 7).
    counts: Arc<ReadCounts>,
}

impl SecurityCli {
    pub fn new() -> Self {
        Self {
            runner: Box::new(ProcessRunner),
            keychain_file: None,
            counts: ReadCounts::new(),
        }
    }

    /// `keychain_file` targets a specific keychain (tests only); production searches the
    /// default list, as CC does.
    pub fn with_runner(runner: Box<dyn Runner>, keychain_file: Option<PathBuf>) -> Self {
        Self {
            runner,
            keychain_file,
            counts: ReadCounts::new(),
        }
    }
```

(the rest of `impl SecurityCli` is unchanged). In `impl Keychain for SecurityCli`, `find`
becomes:

```rust
    /// A secret read (`-w`), counted by how it ended (D3 change 6): answered once it gives a
    /// secret, locked on rc 36 or a timeout. An absent item (rc 44), which a locked keychain
    /// still reports, and every other failure count as neither.
    fn find(&self, service: &str, account: &str) -> Read<Vec<u8>> {
        let read = match self.run(
            s(&["find-generic-password", "-a", account, "-w", "-s", service]),
            None,
        ) {
            RunResult::Exited {
                code: 0, stdout, ..
            } => {
                let raw = decode_output(stdout);
                if looks_like_hex(&raw) {
                    self.disambiguate(service, account, raw)
                } else {
                    Read::Present(raw)
                }
            }
            RunResult::Exited { code: 44, .. } => Read::Absent,
            locked @ (RunResult::Exited { code: 36, .. } | RunResult::TimedOut) => {
                self.counts.record_locked();
                Read::Unreadable(unreadable(locked))
            }
            other => Read::Unreadable(unreadable(other)),
        };
        if read.is_present() {
            self.counts.record_answered();
        }
        read
    }
```

and at the end of that `impl`, after `unlock`:

```rust
    fn read_counts(&self) -> Option<Arc<ReadCounts>> {
        Some(self.counts.clone())
    }
```

In `crates/tagteam-provider/src/lib.rs`:

```rust
pub use keychain::{FakeKeychain, Keychain, KeychainError, LockState};
```

becomes

```rust
pub use keychain::{FakeKeychain, Keychain, KeychainError, LockState, ReadCounts, ReadTally};
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-provider --features file-keychain --lib keychain::tests`, then
`cargo test -p tagteam-provider --lib security::tests`.
Then `cargo fmt --all && cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`.
Expected: PASS. The existing `find_maps_return_codes` and `fake_keychain_models_absent_locked_and_failures`
pass unchanged: no read's result changed, only what is counted.

- [ ] **Step 5: Commit** (the controller commits; implementers do not)

`git commit -m "Count Keychain secret reads by how they ended"`

---

### Task 5: Settings keys `daemon.autoswitch`, `notify.enabled`, `notify.events`

**Depends on:** 1

D7 adds three keys to §6.4's registry "with every rule of §6.4 (forgiving reads, strict `set`,
provider overrides)": `daemon.autoswitch` (bool, default `true`, per provider), `notify.enabled`
(bool, default `true`, global) and `notify.events` (`["switch","quarantine","limit"]`, a list of
those three, duplicates collapse, `''` is the empty list, per provider). The registry already
does everything §6.4 asks of a key once it is declared: reads, `config list|get|set|unset`,
`doctor`'s unknown keys, and completions (`cli.rs` builds them from `KEYS`). So the task is the
declaration, one new kind for `notify.events`, three `Settings` fields, and the tests that pin
the registry's shape.

**Readings of the spec this task commits to:**
- **Where the keys sit.** After `ui.color`, in D7's order. `config list` shows them last. Plan
  B's §6.4 amendment (D16) appends the three rows to the spec's table in the same order, so
  `the_registry_is_section_six_four_s_table_in_order` stays true to its name.
- **`notify.events` is a kind of its own, `KeyKind::NotifyEvents`.** In the file it is one kind
  as a string, or an array of kinds. An item that is not a kind (`"beep"`, `"Switch"`, `3`)
  makes the whole value invalid, as for `autoswitch.models`, never item by item as for
  `run.share_extra`: a typo in a list of three is one warning and the default, which notifies
  more rather than less. A kind repeated collapses into its first, and the order is kept as
  written. `[]` is the empty list. `""` in the file is not a kind, so it is invalid there; `''`
  on the command line is the empty list (§6.4's rule for every list).
- **`notify.enabled` is global.** `per_provider: false`, so `resolve` refuses both
  `provider.<id>.notify.enabled` and `notify.enabled --provider <id>` with `NotPerProvider`
  ("is the same for every provider…"), exactly as for `ui.color`. A
  `[provider.<id>.notify] enabled = …` in the file is never applied and is listed as an unknown
  key. `[provider.<id>.notify]` itself is a known table, because `notify.events` may live there.
- **The phrase** that every warning and refusal uses is
  `must be "switch", "quarantine" or "limit", or a list of them`, built with `one_of` from
  `NotifyEvent::ALL`, so it can never disagree with the parser. The two booleans use the
  existing `must be true, false, 1, 0, yes or no`.
- **`Settings::notify_events` is a `Vec<NotifyEvent>`** in the order written.
  `NotifyEvent::ALL` is both D7's order and the default. `NotifyEvent` derives `Ord`
  (Interfaces) so the notifier may keep kinds in a set.
- **`daemon.autoswitch` affects only the daemon.** `auto_config` does not read it, and
  `tagteam auto` ignores it (D7).

**Files:**
- Modify: `crates/tagteam-engine/src/settings.rs`:
  - `NotifyEvent`, after `impl ColorMode`
  - `KeyKind::NotifyEvents`, after `KeyKind::Provider`
  - three `KEYS` entries, after `ui.color`'s
  - an arm each in `Key::expect`, `Key::parse_item` and `Key::parse_arg`
  - `notify_events_from_item` and `collapse_events`, before `fn is_format`
  - three `Settings` fields, their defaults, and their arms in `Settings::value` and
    `Settings::apply`
- Test: `crates/tagteam-engine/tests/settings.rs` (the registry's shape tests and five new
  tests), `crates/tagteam-engine/tests/config.rs` (`sample`, and two existing tables),
  `crates/tagteam/tests/config_cli.rs` (`config list`'s two snapshots)

**Interfaces:**
- Consumes: `settings::{Key, KeyKind, Value, list_arg, one_of, resolve, inspect}` as they are
  today. Nothing from Task 1: these keys are read with `Settings::load`, as every key is.
- Produces (`tagteam_engine::settings`):
  - `pub enum NotifyEvent { Switch, Quarantine, Limit }`, deriving
    `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord`, with `pub const ALL: [NotifyEvent; 3]`,
    `pub fn as_str(self) -> &'static str` (`"switch" | "quarantine" | "limit"`) and
    `pub fn parse(s: &str) -> Option<Self>`
  - `Settings { …, pub daemon_autoswitch: bool, pub notify_enabled: bool, pub notify_events: Vec<NotifyEvent> }`
  - `KeyKind::NotifyEvents`

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/settings.rs`, make these edits.

The import gains `NotifyEvent`. Replace:

```rust
use tagteam_engine::settings::{
    self, COOLDOWN_SECONDS_RANGE, ColorMode, HYSTERESIS_PCT_RANGE, INTERVAL_SECONDS_RANGE,
    Inspection, KEYS, Key, KeyKind, KeyState, STATUSLINE_PLACEHOLDERS, Settings, SettingsError,
    Source, THRESHOLD_RANGE, UNHEALTHY_TICKS_RANGE, Value, inspect, is_statusline_placeholder,
    parse_bool, resolve,
};
```

with:

```rust
use tagteam_engine::settings::{
    self, COOLDOWN_SECONDS_RANGE, ColorMode, HYSTERESIS_PCT_RANGE, INTERVAL_SECONDS_RANGE,
    Inspection, KEYS, Key, KeyKind, KeyState, NotifyEvent, STATUSLINE_PLACEHOLDERS, Settings,
    SettingsError, Source, THRESHOLD_RANGE, UNHEALTHY_TICKS_RANGE, Value, inspect,
    is_statusline_placeholder, parse_bool, resolve,
};
```

In `the_defaults_are_the_specs_table`, after `assert!(d.share_extra.is_empty());`, add:

```rust
    assert!(d.daemon_autoswitch);
    assert!(d.notify_enabled);
    assert_eq!(d.notify_events, NotifyEvent::ALL);
```

In `every_key_is_read_from_a_full_file`, the file's text ends with the two new tables. Replace:

```rust
[run]
share_extra = ["hook-data"]
"#,
    );
```

with:

```rust
[run]
share_extra = ["hook-data"]

[daemon]
autoswitch = false

[notify]
enabled = false
events = ["limit", "switch"]
"#,
    );
```

and in the same test's expected `Settings`, replace:

```rust
            color: ColorMode::Never,
            share_extra: vec!["hook-data".into()],
        }
```

with:

```rust
            color: ColorMode::Never,
            share_extra: vec!["hook-data".into()],
            daemon_autoswitch: false,
            notify_enabled: false,
            notify_events: vec![NotifyEvent::Limit, NotifyEvent::Switch],
        }
```

In `the_registry_is_section_six_four_s_table_in_order`, after the `ui.color` row (the tuple
ending `KeyKind::Choice(&["auto", "always", "never"]), false ),`), add:

```rust
            ("daemon.autoswitch", KeyKind::Bool, true),
            ("notify.enabled", KeyKind::Bool, false),
            ("notify.events", KeyKind::NotifyEvents, true),
```

In `every_key_reads_back_its_default_from_the_settings`, after
`("ui.color", Value::Str("auto".into())),`, add:

```rust
            ("daemon.autoswitch", Value::Bool(true)),
            ("notify.enabled", Value::Bool(true)),
            ("notify.events", list(&["switch", "quarantine", "limit"])),
```

In `each_key_says_what_a_valid_value_is`, after the `ui.color` assertion (the last one), add:

```rust
    for name in ["daemon.autoswitch", "notify.enabled"] {
        assert_eq!(
            expect(name),
            "must be true, false, 1, 0, yes or no",
            "{name}"
        );
    }
    assert_eq!(
        expect("notify.events"),
        "must be \"switch\", \"quarantine\" or \"limit\", or a list of them"
    );
```

`SAMPLES` gets one sample per new key, which puts the three keys through
`a_value_survives_the_command_line_and_the_file_unchanged` and
`every_key_s_written_value_is_stored_in_the_settings_it_loads_into` (the test that catches a
key without an arm in `Settings::apply`). Replace `const SAMPLES: [(&str, &str); 13] = [` with
`const SAMPLES: [(&str, &str); 16] = [`, and after `("ui.color", "always"),` add:

```rust
    ("daemon.autoswitch", "no"),
    ("notify.enabled", "0"),
    ("notify.events", "limit, switch"),
```

Then append these tests to the end of the file:

```rust
#[test]
fn the_daemon_and_notify_keys_are_read_from_their_tables_the_provider_s_first() {
    // D7: `daemon.autoswitch` and `notify.events` are per provider, `notify.enabled` global.
    let text = "[daemon]\nautoswitch = true\n[notify]\nenabled = \"no\"\nevents = [\"switch\", \"limit\"]\n\n\
                [provider.claude-code.daemon]\nautoswitch = false\n\
                [provider.claude-code.notify]\nevents = []\n";
    let (s, warnings) = load(text);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        (s.daemon_autoswitch, s.notify_enabled, s.notify_events),
        (false, false, Vec::new()),
        "an empty list in the provider's table overrides the global one"
    );
    let (s, warnings) = load_as(text, "fake-agent");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        (s.daemon_autoswitch, s.notify_enabled, s.notify_events),
        (true, false, vec![NotifyEvent::Switch, NotifyEvent::Limit]),
        "another provider's table does not apply"
    );
}

#[test]
fn notify_enabled_is_global_and_no_provider_table_can_hold_it() {
    // D7, like `ui.color`: a provider table's `notify.enabled` is an unknown key, never applied,
    // and neither spelling of it resolves for `config`.
    let i = inspect_as(
        "[notify]\nenabled = true\n\
         [provider.claude-code.notify]\nenabled = false\nevents = \"limit\"\n",
        PROVIDER,
    );
    assert!(i.warnings.is_empty(), "{:?}", i.warnings);
    let enabled = state(&i, "notify.enabled");
    assert_eq!(
        (&enabled.value, enabled.source),
        (&Value::Bool(true), Source::Global)
    );
    let events = state(&i, "notify.events");
    assert_eq!(
        (&events.value, events.source),
        (&list(&["limit"]), Source::Provider)
    );
    assert_eq!(i.unknown, ["provider.claude-code.notify.enabled"]);
    let cc = ProviderId::new(PROVIDER);
    for (name, flag) in [
        ("provider.claude-code.notify.enabled", None),
        ("notify.enabled", Some(&cc)),
    ] {
        assert!(
            matches!(resolve(name, flag), Err(SettingsError::NotPerProvider(k)) if k == "notify.enabled"),
            "{name}"
        );
    }
    for name in [
        "provider.claude-code.daemon.autoswitch",
        "provider.claude-code.notify.events",
    ] {
        assert_eq!(
            resolve(name, None).unwrap().provider,
            Some(cc.clone()),
            "{name}"
        );
    }
}

#[test]
fn notify_events_take_a_kind_or_a_list_and_collapse_repeats() {
    use NotifyEvent::{Limit, Quarantine, Switch};
    for (text, want) in [
        ("\"limit\"", vec![Limit]),
        ("[\"limit\", \"switch\", \"limit\"]", vec![Limit, Switch]),
        ("[]", Vec::new()),
        (
            "[\"quarantine\", \"switch\", \"limit\", \"switch\"]",
            vec![Quarantine, Switch, Limit],
        ),
    ] {
        let (s, warnings) = load(&format!("[notify]\nevents = {text}\n"));
        assert_eq!(s.notify_events, want, "{text}");
        assert!(warnings.is_empty(), "{text}: {warnings:?}");
    }
    assert_eq!(
        NotifyEvent::ALL.map(NotifyEvent::as_str),
        ["switch", "quarantine", "limit"]
    );
    for e in NotifyEvent::ALL {
        assert_eq!(NotifyEvent::parse(e.as_str()), Some(e));
    }
    for name in ["Switch", "quarantined", "", " limit", "all"] {
        assert_eq!(NotifyEvent::parse(name), None, "{name:?}");
    }
}

#[test]
fn an_invalid_daemon_or_notify_value_falls_back_with_one_warning_naming_the_key() {
    // §6.4's forgiving read, and Review Focus 1's premise: a `daemon.autoswitch` that cannot be
    // read is its default, true, and says so.
    const BOOL: &str = "must be true, false, 1, 0, yes or no";
    const EVENTS: &str = "must be \"switch\", \"quarantine\" or \"limit\", or a list of them";
    for (text, key, expect) in [
        (
            "[daemon]\nautoswitch = \"paused\"\n",
            "daemon.autoswitch",
            BOOL,
        ),
        ("[daemon]\nautoswitch = 2\n", "daemon.autoswitch", BOOL),
        ("[notify]\nenabled = \"off\"\n", "notify.enabled", BOOL),
        (
            "[notify]\nevents = [\"switch\", \"beep\"]\n",
            "notify.events",
            EVENTS,
        ),
        (
            "[notify]\nevents = [\"switch\", 3]\n",
            "notify.events",
            EVENTS,
        ),
        (
            "[notify]\nevents = \"switch,limit\"\n",
            "notify.events",
            EVENTS,
        ),
        ("[notify]\nevents = \"\"\n", "notify.events", EVENTS),
        ("[notify]\nevents = true\n", "notify.events", EVENTS),
    ] {
        let (s, warnings) = load(text);
        assert_eq!(
            (s.daemon_autoswitch, s.notify_enabled, s.notify_events),
            (true, true, NotifyEvent::ALL.to_vec()),
            "{text}"
        );
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(
            warnings[0].ends_with(&format!("`{key}` {expect} (ignored)")),
            "{warnings:?}"
        );
    }
    // An invalid provider value warns, naming the provider's key, and the global one applies.
    let (s, warnings) = load(
        "[daemon]\nautoswitch = false\n[notify]\nevents = [\"limit\"]\n\
         [provider.claude-code.daemon]\nautoswitch = \"off\"\n\
         [provider.claude-code.notify]\nevents = [\"Switch\"]\n",
    );
    assert_eq!(
        (s.daemon_autoswitch, s.notify_events),
        (false, vec![NotifyEvent::Limit])
    );
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    for key in ["daemon.autoswitch", "notify.events"] {
        let named = format!("`provider.claude-code.{key}`");
        assert!(warnings.iter().any(|w| w.contains(&named)), "{warnings:?}");
    }
}

#[test]
fn the_daemon_and_notify_keys_on_the_command_line_are_strict() {
    // §6.4: `set` refuses what a read would only warn about.
    for name in ["daemon.autoswitch", "notify.enabled"] {
        let key = reg(name);
        assert_eq!(key.parse_arg("no"), Ok(Value::Bool(false)), "{name}");
        assert_eq!(key.parse_arg("paused"), Err(key.expect()), "{name}");
    }
    let events = reg("notify.events");
    assert_eq!(
        events.parse_arg("limit, switch"),
        Ok(list(&["limit", "switch"]))
    );
    assert_eq!(
        events.parse_arg("switch,switch,quarantine"),
        Ok(list(&["switch", "quarantine"])),
        "a repeat collapses into its first"
    );
    assert_eq!(events.parse_arg(""), Ok(list(&[])), "'' is the empty list");
    for raw in ["beep", "Switch", "switch,all", "limits"] {
        assert_eq!(events.parse_arg(raw), Err(events.expect()), "{raw:?}");
    }
    for raw in ["switch,,limit", " , ", "limit,"] {
        assert!(
            events
                .parse_arg(raw)
                .unwrap_err()
                .starts_with("must be event names separated by commas, with no empty item"),
            "{raw:?}"
        );
    }
}
```

In `crates/tagteam-engine/tests/config.rs`:

`sample` must cover every kind. After `KeyKind::Provider => cc().to_string(),`, add:

```rust
        KeyKind::NotifyEvents => "limit, switch".into(),
```

In `a_key_no_provider_table_may_hold_refuses_under_either_spelling`, the list of global keys
gains `notify.enabled`. After `("default_provider", "claude-code"),`, add:

```rust
        ("notify.enabled", "false"),
```

In `the_spec_s_ranges_and_spellings_hold_at_both_ends_and_nothing_is_clamped`, after the
`("run.share_extra", &["hook-data", ""], &["a/b", ".."]),` case, add:

```rust
        ("daemon.autoswitch", &["no", "true"], &["paused", "2", ""]),
        ("notify.enabled", &["false", "yes"], &["off"]),
        (
            "notify.events",
            &["limit, switch", ""],
            &["beep", "switch,,limit", "Switch"],
        ),
```

and at the end of that test, after `assert_eq!(settings.color, ColorMode::Never);`, add:

```rust
    assert_eq!(
        (
            settings.daemon_autoswitch,
            settings.notify_enabled,
            settings.notify_events
        ),
        (true, true, Vec::new())
    );
```

In `crates/tagteam/tests/config_cli.rs`, `config list` shows the three keys last. In
`config_list_shows_every_key_with_its_value_and_source_then_the_unknown_ones`, after
`"ui.color                             default   auto\n",`, add:

```rust
            "daemon.autoswitch                    default   true\n",
            "notify.enabled                       default   true\n",
            "notify.events                        default   switch,quarantine,limit\n",
```

The key column stays 37 wide: `autoswitch.include_api_key_accounts` is still the longest key.
In `config_list_json_is_the_specs_shape`, after
`let format = json!("{account} · 5h {5h}% · 7d {7d}%{stale}");`, add:

```rust
    let events = json!(["switch", "quarantine", "limit"]);
```

and after `row("ui.color", json!("auto"), json!("auto"), "default"),`, add:

```rust
                row("daemon.autoswitch", json!(true), json!(true), "default"),
                row("notify.enabled", json!(true), json!(true), "default"),
                row("notify.events", events.clone(), events, "default"),
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test settings --test config`
Expected: the build fails: `unresolved import` for `tagteam_engine::settings::NotifyEvent`,
``no variant or associated item named `NotifyEvents` found for enum `KeyKind` ``, and
``struct `Settings` has no field named `daemon_autoswitch` `` (and the other two fields).

Run: `cargo test -p tagteam --features test-support --test config_cli`
Expected: FAIL in `config_list_shows_every_key_with_its_value_and_source_then_the_unknown_ones`
and `config_list_json_is_the_specs_shape`: the output lacks the three rows.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/settings.rs`, after `impl ColorMode { … }` (it ends with
`ColorMode::Never => "never",` and two closing braces), add:

```rust
/// D6.1's notification kinds, as `notify.events` names them (D7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NotifyEvent {
    Switch,
    Quarantine,
    Limit,
}

impl NotifyEvent {
    /// Every kind, in D7's order: `notify.events`' default.
    pub const ALL: [NotifyEvent; 3] = [
        NotifyEvent::Switch,
        NotifyEvent::Quarantine,
        NotifyEvent::Limit,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            NotifyEvent::Switch => "switch",
            NotifyEvent::Quarantine => "quarantine",
            NotifyEvent::Limit => "limit",
        }
    }

    /// The kind `s` names, spelled exactly as `as_str` spells it.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == s)
    }
}
```

In `enum KeyKind`, after the `Provider,` variant and its doc comment, add:

```rust
    /// `notify.events`: D6.1's kinds, as [`NotifyEvent`] spells them. A kind repeated collapses
    /// into its first.
    NotifyEvents,
```

Replace the doc comment of `KEYS`:

```rust
/// Every §6.4 key, in the order of the spec's table. The auto-switch bounds are M3b's
/// `*_RANGE` constants, which `auto` clamps its flags to.
```

with:

```rust
/// Every §6.4 key, in the order of the spec's table, then the daemon's three (D7). The
/// auto-switch bounds are M3b's `*_RANGE` constants, which `auto` clamps its flags to.
```

and after the `ui.color` entry (`kind: KeyKind::Choice(COLORS), per_provider: false, },`), before
the closing `];`, add:

```rust
    Key {
        name: "daemon.autoswitch",
        kind: KeyKind::Bool,
        per_provider: true,
    },
    Key {
        name: "notify.enabled",
        kind: KeyKind::Bool,
        per_provider: false,
    },
    Key {
        name: "notify.events",
        kind: KeyKind::NotifyEvents,
        per_provider: true,
    },
```

In `Key::expect`, after the `KeyKind::Provider => { … }` arm, add:

```rust
            KeyKind::NotifyEvents => format!(
                "must be {}, or a list of them",
                one_of(&NotifyEvent::ALL.map(NotifyEvent::as_str))
            ),
```

In `Key::parse_item`, after the `KeyKind::Provider => { … }` arm, add:

```rust
            KeyKind::NotifyEvents => notify_events_from_item(item).map(Value::List),
```

In `Key::parse_arg`, after `KeyKind::Provider => Err(self.expect()),`, add:

```rust
            KeyKind::NotifyEvents => {
                let names = list_arg(raw, "event names")?;
                collapse_events(names)
                    .map(Value::List)
                    .ok_or_else(|| self.expect())
            }
```

Before `/// A non-empty format whose every `{…}` is one of §13.5's placeholders.` (the doc comment of
`fn is_format`), add:

```rust
/// `notify.events` as the file holds it: a kind or a list of kinds. Any other value, or an item
/// that is not a kind, makes the whole value invalid.
fn notify_events_from_item(item: &Item) -> Option<Vec<String>> {
    let names: Vec<String> = match item.as_str() {
        Some(one) => vec![one.to_owned()],
        None => item
            .as_array()?
            .iter()
            .map(|v| v.as_str().map(str::to_owned))
            .collect::<Option<_>>()?,
    };
    collapse_events(names)
}

/// D7's rule for `notify.events`, from the file or the command line alike: every item is a kind
/// as [`NotifyEvent`] spells it, and a repeated kind collapses into its first.
fn collapse_events(names: Vec<String>) -> Option<Vec<String>> {
    let mut unique: Vec<String> = Vec::with_capacity(names.len());
    for name in names {
        NotifyEvent::parse(&name)?;
        if !unique.contains(&name) {
            unique.push(name);
        }
    }
    Some(unique)
}
```

In `pub struct Settings`, after `pub share_extra: Vec<String>,`, add:

```rust
    /// `daemon.autoswitch` (D7): whether the daemon switches this provider's accounts; false
    /// pauses it (D4.3). `tagteam auto` ignores it. The provider's own table first.
    pub daemon_autoswitch: bool,
    /// `notify.enabled` (D7): every notification on or off. The same for every provider.
    pub notify_enabled: bool,
    /// `notify.events` (D7): the kinds the daemon notifies, in the order written. The provider's
    /// own table first.
    pub notify_events: Vec<NotifyEvent>,
```

In `impl Default for Settings`, after `share_extra: Vec::new(),`, add:

```rust
            daemon_autoswitch: true,
            notify_enabled: true,
            notify_events: NotifyEvent::ALL.to_vec(),
```

In `Settings::value`, after `"ui.color" => Value::Str(self.color.as_str().to_owned()),`, add:

```rust
            "daemon.autoswitch" => Value::Bool(self.daemon_autoswitch),
            "notify.enabled" => Value::Bool(self.notify_enabled),
            "notify.events" => Value::List(
                self.notify_events
                    .iter()
                    .map(|e| e.as_str().to_owned())
                    .collect(),
            ),
```

In `Settings::apply`, after the `("ui.color", Value::Str(s)) => { … }` arm and before the
`(name, value) => unreachable!(…)` arm, add:

```rust
            ("daemon.autoswitch", Value::Bool(b)) => self.daemon_autoswitch = b,
            ("notify.enabled", Value::Bool(b)) => self.notify_enabled = b,
            ("notify.events", Value::List(names)) => {
                self.notify_events = names.iter().filter_map(|n| NotifyEvent::parse(n)).collect()
            }
```

`apply` only ever receives what `parse_item` accepted, so `filter_map` drops nothing; it is
there because `parse` returns an `Option`.

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-engine --test settings --test config`
Expected: PASS. `settings` runs 87 tests (82 before, plus the five new ones).
Run: `cargo test -p tagteam --features test-support --test config_cli --test completions`
Expected: PASS. Completions take their keys from `KEYS`, so they offer the three new keys, and
`provider.claude-code.daemon.autoswitch` and `provider.claude-code.notify.events`, but not
`provider.claude-code.notify.enabled`, with no change to that test.
Then `cargo fmt --all && cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings` and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`.
Expected: clean.

- [ ] **Step 5: Commit** (the controller commits; implementers do not)

```
git add crates/tagteam-engine/src/settings.rs crates/tagteam-engine/tests/settings.rs \
  crates/tagteam-engine/tests/config.rs crates/tagteam/tests/config_cli.rs
git commit -m "Add the daemon.autoswitch, notify.enabled and notify.events settings"
```

---

### Task 6: The `tagteam-daemon` crate, its events and event JSON

**Depends on:** 2

D3 adds a library crate on top of the engine. This task creates it with the two modules every
later task builds on (Decision 1):
- `events.rs`: D5.5's event types, the `DaemonSink` the scheduler's jobs emit into, and three
  adapters. `AutoEvents` lets an engine's tick emit into a daemon sink. `AutoOnly` lets the
  scheduler emit into `tagteam auto`'s own sink (Task 7). `Fanout` gives one emit to several
  sinks (the CLI's, the socket's, the notifier's).
- `protocol.rs`: the JSON. `event_json` moves here from the CLI unchanged (D3: "`auto --json`
  and the socket write the same objects from one function"), and `daemon_event_json` writes
  D5.5's four daemon kinds.

The CLI's `JsonSink` then calls the crate's `event_json`. The crate joins the workspace, the
publish loop and CONTRIBUTING.

**Readings of the spec this task commits to:**
- **The bytes `auto --json` prints do not change.** `event_json` moves verbatim. Its time
  formatting comes from `tagteam_core::time::format_iso8601`, the function
  `tagteam_cc::usage::format_iso8601` re-exports, so the crate needs no `tagteam-cc`. The
  existing test compares `serde_json::Value`s, and `Value` equality ignores key order, so a new
  test pins each kind's exact line. Those lines were printed by today's CLI `event_json`, so
  they are the bytes before the move.
- **The daemon kinds' envelope** is §11.4's: `schemaVersion`, `event`, `ts`, then `provider`
  when the kind concerns one provider, then the kind's fields. D5.5's table gives `changed` no
  fields and `stopping` only `reason`, so neither carries `provider`.
- **Optional fields are left out, never `null`** (`fetchErrors?`, `waitingFor?`, `holderPid?`,
  `backoff?` in D5.5). `usage`'s `updated` is always there, `[]` when nothing was recorded.
  `fetchErrors` appears only when it holds something, as in `poll`. `backoff` is D5.4's shape,
  `{"reason":"keychain","until":"…Z"}`.
- **`daemon_event_json` has its own private `envelope`**, so `event_json` stays byte-for-byte
  the function that moved. The cost is a duplicated envelope of about ten lines.
- **The adapters hand on `tick_done`.** `HumanSink` forgets a tick's poll there (§11.4).
  Without it, a later tick's line could show an earlier tick's account. Task 7 runs
  `tagteam auto` through `AutoEvents` over `AutoOnly`, so both adapters forward it, and a test
  sends a tick's end through the pair.
- **The crate does not `#![forbid(unsafe_code)]`**, unlike `tagteam-core` and
  `tagteam-engine`. Plan B's `server.rs` calls `getpeereid` and `getsockopt(SO_PEERCRED)`
  through `libc`, which needs `unsafe`.
- **Publishing.** The crate goes between `tagteam-engine` and `tagteam` in the publish loop
  (D3). Its first crates.io release must be published by hand with a short-lived token, and
  its trusted publisher registered (CONTRIBUTING, "Releasing"). The controller tells Michael
  before the next release; this task does not publish. `LICENSE` and `NOTICE` are symlinks to
  the root files, as in every other published crate, and `cargo package` follows them. The
  other places that could list crates were checked and need nothing: `dist-workspace.toml` and
  `release.yml` (dist builds only the `tagteam` binary), `release-smoke.yml` and `README.md`,
  which speaks of "the `tagteam-*` crates" without naming them.

**Files:**
- Create: `crates/tagteam-daemon/Cargo.toml`, `crates/tagteam-daemon/src/lib.rs`,
  `crates/tagteam-daemon/src/events.rs`, `crates/tagteam-daemon/src/protocol.rs`
- Create (symlinks): `crates/tagteam-daemon/LICENSE` → `../../LICENSE`,
  `crates/tagteam-daemon/NOTICE` → `../../NOTICE`
- Modify:
  - `Cargo.toml`: `[workspace.dependencies]`, after `tagteam-engine`
  - `crates/tagteam/Cargo.toml`: `[dependencies]`, after `tagteam-engine.workspace = true`
  - `crates/tagteam/src/auto.rs`: `event_json`, `by_position` and the test
    `every_event_is_one_object_in_section_11_4_s_shape` move out, and the imports change
  - `Cargo.lock`
  - `CONTRIBUTING.md`: the crate table, and the version paragraph under "Releasing"
  - `.github/workflows/publish-crates.yml`: the header comment and the crate loop
- Test: unit tests in `events.rs` and `protocol.rs`

**Interfaces:**
- Consumes: `tagteam_engine::auto::{AutoEvent, EventSink}` (with `EventSink::tick_done`),
  `tagteam_core::time::format_iso8601`, `tagteam_core::autoswitch::Trigger`.
- Produces (as the skeleton's Interfaces fix them):
  - `tagteam_daemon::events::{Mode, WaitingFor, Backoff, StopReason, DaemonEvent, DaemonSink, AutoEvents, AutoOnly, Fanout}`.
    `Mode::as_str` (`"auto" | "collect" | "waiting" | "idle"`), `WaitingFor::as_str`
    (`"engine-lock" | "schema"`) and `StopReason::as_str` (`"signal" | "upgrade" | "schema"`).
  - `tagteam_daemon::protocol::{PROTOCOL, event_json, daemon_event_json}`.
  - Workspace dependency `tagteam-daemon`, which the CLI depends on from here on.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-daemon/Cargo.toml`:

```toml
[package]
name = "tagteam-daemon"
description = "Internal crate of tagteam: the daemon's scheduler, socket and service. No stable API."
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
tagteam-core.workspace = true
tagteam-provider.workspace = true
tagteam-engine.workspace = true
fastrand.workspace = true
libc.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
tagteam-cc = { workspace = true, features = ["test-hooks"] }
tagteam-fake.workspace = true
tagteam-provider = { workspace = true, features = ["file-keychain", "mock-server"] }
tempfile.workspace = true

[lints]
workspace = true
```

Link the licence files as the other published crates do:

```sh
ln -s ../../LICENSE crates/tagteam-daemon/LICENSE
ln -s ../../NOTICE crates/tagteam-daemon/NOTICE
```

In the root `Cargo.toml`'s `[workspace.dependencies]`, after the `tagteam-engine` line, add:

```toml
tagteam-daemon = { path = "crates/tagteam-daemon", version = "20261014.2045.0" }
```

Create `crates/tagteam-daemon/src/lib.rs`:

```rust
//! `tagteam daemon` (the daemon spec, D1–D16): the scheduler that the daemon and `tagteam auto`
//! both run on, the daemon's jobs, and the socket protocol (D3). Like the engine, it names no
//! provider's paths, keys or endpoints, and reaches providers through the engine only.

pub mod events;
pub mod protocol;
```

Create `crates/tagteam-daemon/src/events.rs` holding only this test module for now (Step 3 puts
the code above it):

```rust
#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use tagteam_core::CLAUDE_CODE;

    use super::*;

    fn cc() -> ProviderId {
        ProviderId::new(CLAUDE_CODE)
    }

    fn warning() -> AutoEvent {
        AutoEvent::ConfigWarning {
            provider: cc(),
            message: "m".into(),
        }
    }

    /// What a sink was handed: an event, or the end of a tick.
    #[derive(Debug, Clone, PartialEq)]
    enum Call<E> {
        Emit(E),
        Done(ProviderId),
    }

    /// Every call a sink is handed, with the sink's name, in one order shared by several sinks.
    struct Tagged<'a, E> {
        name: &'static str,
        log: &'a RefCell<Vec<(&'static str, Call<E>)>>,
    }

    impl DaemonSink for Tagged<'_, DaemonEvent> {
        fn emit(&self, e: &DaemonEvent) {
            self.log
                .borrow_mut()
                .push((self.name, Call::Emit(e.clone())));
        }

        fn tick_done(&self, provider: &ProviderId) {
            self.log
                .borrow_mut()
                .push((self.name, Call::Done(provider.clone())));
        }
    }

    impl EventSink for Tagged<'_, AutoEvent> {
        fn emit(&self, e: &AutoEvent) {
            self.log
                .borrow_mut()
                .push((self.name, Call::Emit(e.clone())));
        }

        fn tick_done(&self, provider: &ProviderId) {
            self.log
                .borrow_mut()
                .push((self.name, Call::Done(provider.clone())));
        }
    }

    #[test]
    fn modes_waits_and_stop_reasons_are_spelled_as_d5_spells_them() {
        assert_eq!(
            [Mode::Auto, Mode::Collect, Mode::Waiting, Mode::Idle].map(Mode::as_str),
            ["auto", "collect", "waiting", "idle"]
        );
        assert_eq!(
            [WaitingFor::EngineLock, WaitingFor::Schema].map(WaitingFor::as_str),
            ["engine-lock", "schema"]
        );
        assert_eq!(
            [StopReason::Signal, StopReason::Upgrade, StopReason::Schema].map(StopReason::as_str),
            ["signal", "upgrade", "schema"]
        );
    }

    #[test]
    fn auto_events_hands_each_engine_event_on_as_an_auto_kind_with_the_tick_s_end() {
        let log = RefCell::new(Vec::new());
        let daemon = Tagged {
            name: "daemon",
            log: &log,
        };
        let sink = AutoEvents(&daemon);
        sink.emit(&warning());
        sink.tick_done(&cc());
        assert_eq!(
            *log.borrow(),
            [
                ("daemon", Call::Emit(DaemonEvent::Auto(warning()))),
                ("daemon", Call::Done(cc())),
            ]
        );
    }

    #[test]
    fn auto_only_hands_on_the_auto_kinds_and_the_tick_s_end_and_drops_the_daemon_s_own() {
        let log = RefCell::new(Vec::new());
        let auto = Tagged {
            name: "auto",
            log: &log,
        };
        let sink = AutoOnly(&auto);
        for e in [
            DaemonEvent::Usage {
                provider: cc(),
                updated: vec![1],
                fetch_errors: BTreeMap::new(),
            },
            DaemonEvent::Auto(warning()),
            DaemonEvent::Changed,
            DaemonEvent::Mode {
                provider: cc(),
                mode: Mode::Collect,
                waiting_for: None,
                holder_pid: None,
                backoff: None,
            },
            DaemonEvent::Stopping {
                reason: StopReason::Signal,
            },
        ] {
            sink.emit(&e);
        }
        sink.tick_done(&cc());
        assert_eq!(
            *log.borrow(),
            [("auto", Call::Emit(warning())), ("auto", Call::Done(cc()))]
        );
    }

    #[test]
    fn a_tick_through_both_adapters_reaches_the_command_s_sink_unchanged() {
        // `tagteam auto` on the scheduler: the engine emits into `AutoEvents` over `AutoOnly`
        // over the command's sink, and the human sink's end-of-tick bookkeeping still runs.
        let log = RefCell::new(Vec::new());
        let command = Tagged {
            name: "command",
            log: &log,
        };
        let only = AutoOnly(&command);
        let engine_side = AutoEvents(&only);
        engine_side.emit(&warning());
        engine_side.tick_done(&cc());
        assert_eq!(
            *log.borrow(),
            [
                ("command", Call::Emit(warning())),
                ("command", Call::Done(cc()))
            ]
        );
    }

    #[test]
    fn fanout_hands_every_call_to_each_sink_in_order() {
        let log = RefCell::new(Vec::new());
        let (a, b) = (
            Tagged {
                name: "a",
                log: &log,
            },
            Tagged {
                name: "b",
                log: &log,
            },
        );
        let fanout = Fanout(vec![&a, &b]);
        fanout.emit(&DaemonEvent::Changed);
        fanout.tick_done(&cc());
        assert_eq!(
            *log.borrow(),
            [
                ("a", Call::Emit(DaemonEvent::Changed)),
                ("b", Call::Emit(DaemonEvent::Changed)),
                ("a", Call::Done(cc())),
                ("b", Call::Done(cc())),
            ]
        );
    }
}
```

Create `crates/tagteam-daemon/src/protocol.rs` holding only this test module for now. The first
test is `every_event_is_one_object_in_section_11_4_s_shape`, moved unchanged from
`crates/tagteam/src/auto.rs` (Step 3 deletes it there). `auto_lines` holds the lines today's CLI
prints for each kind:

```rust
#[cfg(test)]
mod tests {
    use tagteam_core::CLAUDE_CODE;
    use tagteam_core::autoswitch::Trigger;

    use super::*;
    use crate::events::{Backoff, Mode, StopReason, WaitingFor};

    /// The engine tests' fixture time, epoch seconds: 2026-09-21T14:13:20Z.
    const T0: i64 = 1_790_000_000;

    fn cc() -> ProviderId {
        ProviderId::new(CLAUDE_CODE)
    }

    fn no_switch_event(reason: &str, detail: &str) -> AutoEvent {
        AutoEvent::NoSwitch {
            provider: cc(),
            reason: reason.into(),
            detail: detail.into(),
        }
    }

    #[test]
    fn every_event_is_one_object_in_section_11_4_s_shape() {
        let poll = AutoEvent::Poll {
            provider: cc(),
            active: Some((2, "b@x.co".into())),
            headroom_pct: BTreeMap::from([(1, Some(80.0)), (2, None)]),
            threshold: 90.0,
            fetch_errors: BTreeMap::from([(2, "http-429".into())]),
            windows_pct: BTreeMap::from([(1, BTreeMap::from([("5h".into(), 20.0)]))]),
        };
        let quiet = AutoEvent::Poll {
            provider: cc(),
            active: None,
            headroom_pct: BTreeMap::new(),
            threshold: 90.0,
            fetch_errors: BTreeMap::new(),
            windows_pct: BTreeMap::new(),
        };
        let cases = [
            (
                poll,
                json!({"event": "poll", "active": {"number": 2, "email": "b@x.co"},
                       "headroomPct": {"1": 80.0, "2": null}, "threshold": 90.0,
                       "fetchErrors": {"2": "http-429"}, "windowsPct": {"1": {"5h": 20.0}}}),
            ),
            (
                quiet,
                json!({"event": "poll", "active": null, "headroomPct": {}, "threshold": 90.0}),
            ),
            (
                AutoEvent::Switch {
                    provider: cc(),
                    trigger: tagteam_core::autoswitch::Trigger::Failover,
                    from: 2,
                    to: 1,
                    warnings: vec!["w".into()],
                    dry_run: true,
                },
                json!({"event": "switch", "trigger": "failover", "from": 2, "to": 1,
                       "warnings": ["w"], "dryRun": true}),
            ),
            (
                no_switch_event("cooldown", "3m"),
                json!({"event": "no-switch", "reason": "cooldown", "detail": "3m"}),
            ),
            (
                AutoEvent::AccountQuarantined {
                    provider: cc(),
                    number: 3,
                    email: "w@corp.com".into(),
                    reason: "invalid_grant".into(),
                },
                json!({"event": "account-quarantined", "number": 3, "email": "w@corp.com",
                       "reason": "invalid_grant"}),
            ),
            (
                AutoEvent::AccountUnquarantined {
                    provider: cc(),
                    number: 3,
                    email: "w@corp.com".into(),
                    reason: "account-replaced".into(),
                },
                json!({"event": "account-unquarantined", "number": 3, "email": "w@corp.com",
                       "reason": "account-replaced"}),
            ),
            (
                AutoEvent::AllExhausted {
                    provider: cc(),
                    earliest_reset_at: Some(T0 + 9_630),
                },
                json!({"event": "all-exhausted", "earliestResetAt": "2026-09-21T16:53:50Z"}),
            ),
            (
                AutoEvent::AllExhausted {
                    provider: cc(),
                    earliest_reset_at: None,
                },
                json!({"event": "all-exhausted", "earliestResetAt": null}),
            ),
            (
                AutoEvent::Sleep {
                    provider: cc(),
                    seconds: 600.0,
                    until: T0 + 600,
                },
                json!({"event": "sleep", "seconds": 600.0, "until": "2026-09-21T14:23:20Z"}),
            ),
            (
                AutoEvent::Error {
                    provider: cc(),
                    message: "m".into(),
                    transient: true,
                },
                json!({"event": "error", "message": "m", "transient": true}),
            ),
            (
                AutoEvent::ConfigWarning {
                    provider: cc(),
                    message: "m".into(),
                },
                json!({"event": "config-warning", "message": "m"}),
            ),
        ];
        for (event, fields) in cases {
            let mut want = json!({"schemaVersion": 1, "event": fields["event"],
                                  "ts": "2026-09-21T14:13:20Z", "provider": "claude-code"});
            for (k, v) in fields.as_object().unwrap() {
                want[k] = v.clone();
            }
            assert_eq!(event_json(&event, T0), want);
        }
        let line = event_json(&no_switch_event("cooldown", ""), T0).to_string();
        assert_eq!(
            line,
            r#"{"schemaVersion":1,"event":"no-switch","ts":"2026-09-21T14:13:20Z","provider":"claude-code","reason":"cooldown","detail":""}"#
        );
    }

    /// Each auto kind, and the line `tagteam auto --json` printed for it before the move: the
    /// same bytes, key order included (D3).
    fn auto_lines() -> Vec<(AutoEvent, &'static str)> {
        vec![
            (
                AutoEvent::Poll {
                    provider: cc(),
                    active: Some((2, "b@x.co".into())),
                    headroom_pct: BTreeMap::from([(1, Some(80.0)), (2, None)]),
                    threshold: 90.0,
                    fetch_errors: BTreeMap::from([(2, "http-429".into())]),
                    windows_pct: BTreeMap::from([(1, BTreeMap::from([("5h".into(), 20.0)]))]),
                },
                r#"{"schemaVersion":1,"event":"poll","ts":"2026-09-21T14:13:20Z","provider":"claude-code","active":{"number":2,"email":"b@x.co"},"headroomPct":{"1":80.0,"2":null},"threshold":90.0,"fetchErrors":{"2":"http-429"},"windowsPct":{"1":{"5h":20.0}}}"#,
            ),
            (
                AutoEvent::Poll {
                    provider: cc(),
                    active: None,
                    headroom_pct: BTreeMap::new(),
                    threshold: 90.0,
                    fetch_errors: BTreeMap::new(),
                    windows_pct: BTreeMap::new(),
                },
                r#"{"schemaVersion":1,"event":"poll","ts":"2026-09-21T14:13:20Z","provider":"claude-code","active":null,"headroomPct":{},"threshold":90.0}"#,
            ),
            (
                AutoEvent::Switch {
                    provider: cc(),
                    trigger: Trigger::Failover,
                    from: 2,
                    to: 1,
                    warnings: vec!["w".into()],
                    dry_run: true,
                },
                r#"{"schemaVersion":1,"event":"switch","ts":"2026-09-21T14:13:20Z","provider":"claude-code","trigger":"failover","from":2,"to":1,"warnings":["w"],"dryRun":true}"#,
            ),
            (
                no_switch_event("cooldown", "3m"),
                r#"{"schemaVersion":1,"event":"no-switch","ts":"2026-09-21T14:13:20Z","provider":"claude-code","reason":"cooldown","detail":"3m"}"#,
            ),
            (
                AutoEvent::AccountQuarantined {
                    provider: cc(),
                    number: 3,
                    email: "w@corp.com".into(),
                    reason: "invalid_grant".into(),
                },
                r#"{"schemaVersion":1,"event":"account-quarantined","ts":"2026-09-21T14:13:20Z","provider":"claude-code","number":3,"email":"w@corp.com","reason":"invalid_grant"}"#,
            ),
            (
                AutoEvent::AccountUnquarantined {
                    provider: cc(),
                    number: 3,
                    email: "w@corp.com".into(),
                    reason: "account-replaced".into(),
                },
                r#"{"schemaVersion":1,"event":"account-unquarantined","ts":"2026-09-21T14:13:20Z","provider":"claude-code","number":3,"email":"w@corp.com","reason":"account-replaced"}"#,
            ),
            (
                AutoEvent::AllExhausted {
                    provider: cc(),
                    earliest_reset_at: Some(T0 + 9_630),
                },
                r#"{"schemaVersion":1,"event":"all-exhausted","ts":"2026-09-21T14:13:20Z","provider":"claude-code","earliestResetAt":"2026-09-21T16:53:50Z"}"#,
            ),
            (
                AutoEvent::AllExhausted {
                    provider: cc(),
                    earliest_reset_at: None,
                },
                r#"{"schemaVersion":1,"event":"all-exhausted","ts":"2026-09-21T14:13:20Z","provider":"claude-code","earliestResetAt":null}"#,
            ),
            (
                AutoEvent::Sleep {
                    provider: cc(),
                    seconds: 600.0,
                    until: T0 + 600,
                },
                r#"{"schemaVersion":1,"event":"sleep","ts":"2026-09-21T14:13:20Z","provider":"claude-code","seconds":600.0,"until":"2026-09-21T14:23:20Z"}"#,
            ),
            (
                AutoEvent::Sleep {
                    provider: cc(),
                    seconds: 66.04,
                    until: T0 + 66,
                },
                r#"{"schemaVersion":1,"event":"sleep","ts":"2026-09-21T14:13:20Z","provider":"claude-code","seconds":66.0,"until":"2026-09-21T14:14:26Z"}"#,
            ),
            (
                AutoEvent::Error {
                    provider: cc(),
                    message: "m".into(),
                    transient: true,
                },
                r#"{"schemaVersion":1,"event":"error","ts":"2026-09-21T14:13:20Z","provider":"claude-code","message":"m","transient":true}"#,
            ),
            (
                AutoEvent::ConfigWarning {
                    provider: cc(),
                    message: "m".into(),
                },
                r#"{"schemaVersion":1,"event":"config-warning","ts":"2026-09-21T14:13:20Z","provider":"claude-code","message":"m"}"#,
            ),
        ]
    }

    #[test]
    fn every_auto_kind_is_written_byte_for_byte_as_auto_wrote_it_before() {
        for (event, line) in auto_lines() {
            assert_eq!(event_json(&event, T0).to_string(), line);
        }
    }

    #[test]
    fn an_auto_kind_reaches_a_subscriber_exactly_as_auto_json_prints_it() {
        // D5.5: the socket and `auto --json` write the auto kinds from one function.
        for (event, line) in auto_lines() {
            assert_eq!(
                daemon_event_json(&DaemonEvent::Auto(event), T0).to_string(),
                line
            );
        }
    }

    #[test]
    fn each_daemon_kind_is_one_object_in_the_envelope_and_leaves_out_what_it_lacks() {
        let mode = |mode, waiting_for, holder_pid, backoff| DaemonEvent::Mode {
            provider: cc(),
            mode,
            waiting_for,
            holder_pid,
            backoff,
        };
        let cases = [
            (
                DaemonEvent::Usage {
                    provider: cc(),
                    updated: vec![1, 3],
                    fetch_errors: BTreeMap::new(),
                },
                r#"{"schemaVersion":1,"event":"usage","ts":"2026-09-21T14:13:20Z","provider":"claude-code","updated":[1,3]}"#,
            ),
            (
                DaemonEvent::Usage {
                    provider: cc(),
                    updated: Vec::new(),
                    fetch_errors: BTreeMap::from([(2, "http-429".into())]),
                },
                r#"{"schemaVersion":1,"event":"usage","ts":"2026-09-21T14:13:20Z","provider":"claude-code","updated":[],"fetchErrors":{"2":"http-429"}}"#,
            ),
            (
                DaemonEvent::Changed,
                r#"{"schemaVersion":1,"event":"changed","ts":"2026-09-21T14:13:20Z"}"#,
            ),
            (
                mode(Mode::Auto, None, None, None),
                r#"{"schemaVersion":1,"event":"mode","ts":"2026-09-21T14:13:20Z","provider":"claude-code","mode":"auto"}"#,
            ),
            (
                mode(Mode::Idle, None, None, None),
                r#"{"schemaVersion":1,"event":"mode","ts":"2026-09-21T14:13:20Z","provider":"claude-code","mode":"idle"}"#,
            ),
            (
                mode(
                    Mode::Waiting,
                    Some(WaitingFor::EngineLock),
                    Some(4242),
                    None,
                ),
                r#"{"schemaVersion":1,"event":"mode","ts":"2026-09-21T14:13:20Z","provider":"claude-code","mode":"waiting","waitingFor":"engine-lock","holderPid":4242}"#,
            ),
            (
                mode(Mode::Waiting, Some(WaitingFor::Schema), None, None),
                r#"{"schemaVersion":1,"event":"mode","ts":"2026-09-21T14:13:20Z","provider":"claude-code","mode":"waiting","waitingFor":"schema"}"#,
            ),
            (
                mode(Mode::Collect, None, None, Some(Backoff { until: T0 + 900 })),
                r#"{"schemaVersion":1,"event":"mode","ts":"2026-09-21T14:13:20Z","provider":"claude-code","mode":"collect","backoff":{"reason":"keychain","until":"2026-09-21T14:28:20Z"}}"#,
            ),
            (
                DaemonEvent::Stopping {
                    reason: StopReason::Signal,
                },
                r#"{"schemaVersion":1,"event":"stopping","ts":"2026-09-21T14:13:20Z","reason":"signal"}"#,
            ),
            (
                DaemonEvent::Stopping {
                    reason: StopReason::Upgrade,
                },
                r#"{"schemaVersion":1,"event":"stopping","ts":"2026-09-21T14:13:20Z","reason":"upgrade"}"#,
            ),
            (
                DaemonEvent::Stopping {
                    reason: StopReason::Schema,
                },
                r#"{"schemaVersion":1,"event":"stopping","ts":"2026-09-21T14:13:20Z","reason":"schema"}"#,
            ),
        ];
        for (event, line) in cases {
            assert_eq!(daemon_event_json(&event, T0).to_string(), line, "{event:?}");
        }
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-daemon`
Expected: the build fails. The test modules are the whole files, so every name they take from
`super` or `crate::events` is unresolved: `error[E0432]` for the `crate::events` import, and
`E0412`, `E0405` and `E0425` for `DaemonEvent`, `DaemonSink`, `event_json`, `daemon_event_json`
and the rest. The build also adds `tagteam-daemon` to `Cargo.lock`. The crate needs no new
external crate. If a sandbox refuses cargo's registry, run `cargo fetch --locked` once outside
it.

- [ ] **Step 3: Implement**

In `crates/tagteam-daemon/src/events.rs`, insert above `#[cfg(test)]`:

```rust
//! D5.5's events. A tick's own (§11.4's auto kinds) and the daemon's (`usage`, `changed`, `mode`,
//! `stopping`) reach one `DaemonSink`. The adapters below join it to the engine's `EventSink`
//! both ways, so an engine emits into a daemon sink, and `tagteam auto` runs on the scheduler
//! with its own sink (D2).

use std::collections::BTreeMap;

use tagteam_core::ProviderId;
use tagteam_engine::auto::{AutoEvent, EventSink};

/// D4.3: what a provider job does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Collect,
    Waiting,
    Idle,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Collect => "collect",
            Mode::Waiting => "waiting",
            Mode::Idle => "idle",
        }
    }
}

/// D4.3: what a `waiting` job waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitingFor {
    /// Another live process is the provider's engine (§11.1).
    EngineLock,
    /// The store's schema is newer than this build (D4.1 step 4).
    Schema,
}

impl WaitingFor {
    pub fn as_str(self) -> &'static str {
        match self {
            WaitingFor::EngineLock => "engine-lock",
            WaitingFor::Schema => "schema",
        }
    }
}

/// D4.1: a provider job's keychain backoff, while it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    /// When the job runs next, epoch seconds.
    pub until: i64,
}

/// Why the scheduler stopped: a signal (D4.6), a replaced binary or a newer schema (D4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Signal,
    Upgrade,
    Schema,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::Signal => "signal",
            StopReason::Upgrade => "upgrade",
            StopReason::Schema => "schema",
        }
    }
}

/// One event the daemon emits (D5.5).
#[derive(Debug, Clone, PartialEq)]
pub enum DaemonEvent {
    /// One of §11.4's kinds, from a tick or the auto job around it.
    Auto(AutoEvent),
    /// A `collect` round that recorded or failed something: the positions whose reading was
    /// recorded, and each failed fetch's error kind by position.
    Usage {
        provider: ProviderId,
        updated: Vec<u32>,
        fetch_errors: BTreeMap<u32, String>,
    },
    /// Another process committed to the store (D4.4).
    Changed,
    /// A provider job changed mode, or its keychain backoff started or ended (D4.3).
    Mode {
        provider: ProviderId,
        mode: Mode,
        waiting_for: Option<WaitingFor>,
        /// The other engine's pid, while `waiting_for` is `EngineLock`.
        holder_pid: Option<u32>,
        backoff: Option<Backoff>,
    },
    /// The last event before the daemon closes.
    Stopping { reason: StopReason },
}

/// Where the scheduler's jobs send their events: the CLI's human or JSON output, the socket's
/// subscribers (plan B), the notifier (D6).
pub trait DaemonSink {
    fn emit(&self, e: &DaemonEvent);
    /// The end of a tick of `provider`'s engine, as `EventSink::tick_done` has it.
    fn tick_done(&self, _provider: &ProviderId) {}
}

/// Every `AutoEvent` an engine emits, as `DaemonEvent::Auto`, into a `DaemonSink`; the end of
/// each tick goes with them.
pub struct AutoEvents<'a>(pub &'a dyn DaemonSink);

impl EventSink for AutoEvents<'_> {
    fn emit(&self, e: &AutoEvent) {
        self.0.emit(&DaemonEvent::Auto(e.clone()));
    }

    fn tick_done(&self, provider: &ProviderId) {
        self.0.tick_done(provider);
    }
}

/// A `DaemonSink` over a `tagteam auto` `EventSink`: the auto kinds and the end of each tick are
/// handed on, and the daemon's own kinds dropped, since `auto` prints none (D2).
pub struct AutoOnly<'a>(pub &'a dyn EventSink);

impl DaemonSink for AutoOnly<'_> {
    fn emit(&self, e: &DaemonEvent) {
        if let DaemonEvent::Auto(e) = e {
            self.0.emit(e);
        }
    }

    fn tick_done(&self, provider: &ProviderId) {
        self.0.tick_done(provider);
    }
}

/// Several sinks: each call goes to every one, in order.
pub struct Fanout<'a>(pub Vec<&'a dyn DaemonSink>);

impl DaemonSink for Fanout<'_> {
    fn emit(&self, e: &DaemonEvent) {
        for sink in &self.0 {
            sink.emit(e);
        }
    }

    fn tick_done(&self, provider: &ProviderId) {
        for sink in &self.0 {
            sink.tick_done(provider);
        }
    }
}
```

In `crates/tagteam-daemon/src/protocol.rs`, insert above `#[cfg(test)]`. `event_json` and
`by_position` are the CLI's, unchanged except for where `format_iso8601` is imported from:

```rust
//! D5's wire format. Every event is one JSON object in §11.4's envelope, and `tagteam auto
//! --json`, `tagteam daemon --json` and the socket all write it from here (D3), so they never
//! differ. Plan B adds the requests, the replies and the greeting.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use tagteam_core::ProviderId;
use tagteam_core::time::format_iso8601;
use tagteam_engine::auto::AutoEvent;

use crate::events::DaemonEvent;

/// D5.6: the protocol's version. New ops, kinds and fields are additive and keep it.
pub const PROTOCOL: u32 = 1;

/// One event in §11.4's shape: the envelope `{"schemaVersion":1,"event":<kind>,"ts":"…Z"}`,
/// the additive `provider` (§13.2), then the kind's fields in cswap's spelling. Accounts are
/// keyed by position; times are ISO 8601 UTC. `fetchErrors` and `windowsPct` appear only when
/// they hold something.
pub fn event_json(e: &AutoEvent, now_s: i64) -> Value {
    let (kind, provider, fields) = match e {
        AutoEvent::Poll {
            provider,
            active,
            headroom_pct,
            threshold,
            fetch_errors,
            windows_pct,
        } => {
            let mut f = json!({
                "active": active.as_ref().map(|(number, email)| json!({"number": number, "email": email})),
                "headroomPct": by_position(headroom_pct, |h| json!(h)),
                "threshold": threshold,
            });
            if !fetch_errors.is_empty() {
                f["fetchErrors"] = by_position(fetch_errors, |kind| json!(kind));
            }
            if !windows_pct.is_empty() {
                f["windowsPct"] = by_position(windows_pct, |w| json!(w));
            }
            ("poll", provider, f)
        }
        AutoEvent::Switch {
            provider,
            trigger,
            from,
            to,
            warnings,
            dry_run,
        } => (
            "switch",
            provider,
            json!({"trigger": trigger.as_str(), "from": from, "to": to, "warnings": warnings, "dryRun": dry_run}),
        ),
        AutoEvent::NoSwitch {
            provider,
            reason,
            detail,
        } => (
            "no-switch",
            provider,
            json!({"reason": reason, "detail": detail}),
        ),
        AutoEvent::AccountQuarantined {
            provider,
            number,
            email,
            reason,
        } => (
            "account-quarantined",
            provider,
            json!({"number": number, "email": email, "reason": reason}),
        ),
        AutoEvent::AccountUnquarantined {
            provider,
            number,
            email,
            reason,
        } => (
            "account-unquarantined",
            provider,
            json!({"number": number, "email": email, "reason": reason}),
        ),
        AutoEvent::AllExhausted {
            provider,
            earliest_reset_at,
        } => (
            "all-exhausted",
            provider,
            json!({"earliestResetAt": earliest_reset_at.map(format_iso8601)}),
        ),
        AutoEvent::Sleep {
            provider,
            seconds,
            until,
        } => (
            "sleep",
            provider,
            json!({"seconds": (seconds * 10.0).round() / 10.0, "until": format_iso8601(*until)}),
        ),
        AutoEvent::Error {
            provider,
            message,
            transient,
        } => (
            "error",
            provider,
            json!({"message": message, "transient": transient}),
        ),
        AutoEvent::ConfigWarning { provider, message } => {
            ("config-warning", provider, json!({"message": message}))
        }
    };
    let mut o = json!({
        "schemaVersion": 1,
        "event": kind,
        "ts": format_iso8601(now_s),
        "provider": provider.as_str(),
    });
    if let Value::Object(fields) = fields {
        for (k, v) in fields {
            o[k] = v;
        }
    }
    o
}

/// A map keyed by account position, as §11.4's objects key accounts.
fn by_position<T>(m: &BTreeMap<u32, T>, value: impl Fn(&T) -> Value) -> Value {
    Value::Object(
        m.iter()
            .map(|(position, v)| (position.to_string(), value(v)))
            .collect(),
    )
}

/// One event as `subscribe` delivers it (D5.5): an auto kind exactly as `event_json` writes it,
/// or a daemon kind in the same envelope. A daemon kind's optional field is left out, never
/// null, when it holds nothing. `changed` and `stopping` concern no one provider, so they carry
/// no `provider`.
pub fn daemon_event_json(e: &DaemonEvent, now_s: i64) -> Value {
    let (kind, provider, fields) = match e {
        DaemonEvent::Auto(e) => return event_json(e, now_s),
        DaemonEvent::Usage {
            provider,
            updated,
            fetch_errors,
        } => {
            let mut f = json!({"updated": updated});
            if !fetch_errors.is_empty() {
                f["fetchErrors"] = by_position(fetch_errors, |kind| json!(kind));
            }
            ("usage", Some(provider), f)
        }
        DaemonEvent::Changed => ("changed", None, json!({})),
        DaemonEvent::Mode {
            provider,
            mode,
            waiting_for,
            holder_pid,
            backoff,
        } => {
            let mut f = json!({"mode": mode.as_str()});
            if let Some(waiting_for) = waiting_for {
                f["waitingFor"] = json!(waiting_for.as_str());
            }
            if let Some(pid) = holder_pid {
                f["holderPid"] = json!(pid);
            }
            if let Some(backoff) = backoff {
                f["backoff"] =
                    json!({"reason": "keychain", "until": format_iso8601(backoff.until)});
            }
            ("mode", Some(provider), f)
        }
        DaemonEvent::Stopping { reason } => ("stopping", None, json!({"reason": reason.as_str()})),
    };
    envelope(kind, provider, fields, now_s)
}

/// §11.4's envelope around a daemon kind's `fields`, with `provider` when it concerns one.
fn envelope(kind: &str, provider: Option<&ProviderId>, fields: Value, now_s: i64) -> Value {
    let mut o = json!({
        "schemaVersion": 1,
        "event": kind,
        "ts": format_iso8601(now_s),
    });
    if let Some(provider) = provider {
        o["provider"] = json!(provider.as_str());
    }
    if let Value::Object(fields) = fields {
        for (k, v) in fields {
            o[k] = v;
        }
    }
    o
}
```

Now the CLI uses the crate. In `crates/tagteam/Cargo.toml`'s `[dependencies]`, after
`tagteam-engine.workspace = true`, add:

```toml
tagteam-daemon.workspace = true
```

In `crates/tagteam/src/auto.rs`:
- Delete `pub fn event_json` and `fn by_position`, each with its doc comment: from
  `/// One event in §11.4's shape: the envelope …` down to the closing brace of `by_position`,
  just above `/// §11.4's human output.`.
- In `mod tests`, delete the test `every_event_is_one_object_in_section_11_4_s_shape`, which now
  lives in the crate. Keep the helpers `poll` and `no_switch_event`: the human sink's tests use
  them.
- The imports: replace

  ```rust
  use serde_json::{Value, json};
  use tagteam_cc::usage::format_iso8601;

  use tagteam_core::ProviderId;
  use tagteam_core::autoswitch::{
  ```

  with

  ```rust
  use tagteam_core::ProviderId;
  use tagteam_core::autoswitch::{
  ```

  and after the `use tagteam_core::autoswitch::{ … };` block, add:

  ```rust
  use tagteam_daemon::protocol::event_json;
  ```

`JsonSink::emit` keeps calling `event_json(e, (self.now_ms)().div_euclid(1000))`, which now
resolves to the crate's. The non-test code used `json!` and `Value` only in the two functions
that moved. The test module keeps its own `use serde_json::json;`.

In `CONTRIBUTING.md`, after the crate table's `tagteam-engine` row, add:

```markdown
| `tagteam-daemon` | The daemon: the scheduler that `tagteam auto` also runs on, its jobs and the socket protocol |
```

Under "Releasing", replace:

```markdown
  zeros in `HHMM` (09:05 is `905`). Change `version` in `[workspace.package]` and the four
  internal `version = "…"` entries in `[workspace.dependencies]` (`tagteam-core`,
  `tagteam-provider`, `tagteam-cc` and `tagteam-engine`) together, then refresh `Cargo.lock`.
```

with:

```markdown
  zeros in `HHMM` (09:05 is `905`). Change `version` in `[workspace.package]` and the five
  internal `version = "…"` entries in `[workspace.dependencies]` (`tagteam-core`,
  `tagteam-provider`, `tagteam-cc`, `tagteam-engine` and `tagteam-daemon`) together, then
  refresh `Cargo.lock`.
```

In `.github/workflows/publish-crates.yml`, replace `# the five crates to crates.io in dependency
order.` with `# the six crates to crates.io in dependency order.`, and replace:

```sh
          for crate in tagteam-core tagteam-provider tagteam-cc tagteam-engine tagteam; do
```

with:

```sh
          for crate in tagteam-core tagteam-provider tagteam-cc tagteam-engine tagteam-daemon tagteam; do
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-daemon`
Expected: PASS, 9 tests (5 in `events`, 4 in `protocol`).
Run: `cargo test -p tagteam --features test-support --lib auto::tests` and
`cargo test -p tagteam --features test-support --test auto_cli`
Expected: PASS. `auto --json` prints through the crate's `event_json`.
Run: `cargo package --list -p tagteam-daemon --allow-dirty`
Expected: the list includes `LICENSE`, `NOTICE`, `src/events.rs`, `src/lib.rs` and
`src/protocol.rs`.
Then `cargo fmt --all && cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings` and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`.
Expected: clean. `--locked` passes because Step 2's build already wrote the new crate into
`Cargo.lock`.

- [ ] **Step 5: Commit** (the controller commits; implementers do not)

```
git add crates/tagteam-daemon Cargo.toml Cargo.lock crates/tagteam/Cargo.toml \
  crates/tagteam/src/auto.rs CONTRIBUTING.md .github/workflows/publish-crates.yml
git commit -m "Add the tagteam-daemon crate with the daemon's events and their JSON"
```

---

### Task 7: The scheduler, and `tagteam auto` on it

**Depends on:** 6

D2: "The `auto` loop (wall-clock deadlines, cancellable 1 s slices, settings reload) moves into
the new crate as a job scheduler. `tagteam auto` becomes that scheduler with only auto jobs and
the CLI's sink. Its behaviour, output and exit codes do not change, and its tests move with it."
This task does that in three parts:
- **Part A** writes the generic `Scheduler` (D4.2) with its own toy-job tests.
- **Part B** moves the loop's code out of `crates/tagteam/src/auto.rs` into
  `tagteam_daemon::auto`, rebuilt on the scheduler as one `AutoJob` per provider. The loop's
  tests move with it.
- **Part C** leaves the CLI only its sinks, and points `app.rs` at the crate.

**Readings of the spec this task commits to:**
- **The scheduler is `Driver::forever`, generalised.** Each pass:
  1. checks the cancel token (`StopReason::Signal`)
  2. checks `config.toml`'s mtime
  3. clamps every due time to `now + delay`, so a clock set back cannot stretch a wait
  4. then either runs the earliest due job (the first added on a tie), or sleeps
     `min(left, SLICE_MS)`
  A job's next due time is `now + delay`, with the clock read after the run. A machine that
  wakes past several due times therefore runs each overdue job once, never once per missed
  period (Review Focus 4, which Task 10 tests). `Ran::Stop(reason)` returns at once, before
  any other job runs. A scheduler with no job waits in slices for the token.
- **First due times.** `add(job, due_ms)` takes a wall-clock due time. `run` sets each job's
  delay to `due_ms − now` when it starts. The clamp therefore also holds for a job first due
  later, such as the daemon's upgrade watch at +60 s (Task 10).
- **Decision 9.** The mtime is checked at the top of every pass: in every slice and before
  every job. When it changed, every job's `settings_changed` runs, in the order the jobs were
  added, before the next job.
  - For `tagteam auto` this moves one observable thing. A `config-warning` from a reload now
    comes in the slice after the write, not just before the next tick. The moved test
    `changed_settings_are_read_again_before_the_next_tick_and_flags_still_win` asserted the
    warning at 60 s; it now comes at 1 s, still before the tick at 60 s. That assertion and
    its comment are the only change to the moved tests beyond the feature gate below.
  - Decision 9 records this change. The original expectation of `60` was checked: it fails
    against the scheduler.
- **The mtime baseline.** `run_loop` reads `Settings::mtime` before it reads any provider's
  settings, so a write that lands in between is still seen. The scheduler takes that reading
  through `Scheduler::with_settings_mtime` (Interfaces, Decision 9); `Daemon::start` (Task 12)
  passes the one it reads the same way. Without it, the scheduler's first check records the
  mtime it finds and tells no job. Such a check never repeats the settings warnings the command
  already printed.
- **The scheduler reloads nothing itself.** It calls each job's `settings_changed`. An
  `AutoJob` re-reads its provider's settings there, as `reload_if_changed` did for each slot.
  `Engine::reload_settings` (Decision 3) is not called here, so `tagteam auto` keeps today's
  behaviour. The daemon's engine-level reload belongs to its private settings job (Task 12,
  Decision 3); each provider job re-reads its own provider's settings (Task 8, Decision 8).
- **`run_loop` keeps its signature and behaviour.** It wraps the command's `EventSink` in
  `AutoOnly` for the scheduler's `DaemonSink`. Each `AutoJob` hands its tick
  `AutoEvents(cx.sink)`, so every `AutoEvent` and every `tick_done` still reaches the command's
  sink.
  - An auto job stops the scheduler only for a signal, and `run_loop` returns `Ok(0)` whatever
    the stop reason.
  - `--once` does not use the scheduler; its code moves unchanged.
- **One loop test stays in the CLI.**
  `a_tick_that_ends_in_an_error_leaves_its_poll_to_no_later_line` drives `run_loop` through
  `HumanSink`, which stays in the CLI, so the test stays too. It keeps a fixture trimmed to what
  it uses: Claude Code only, with `add`, `read` and a `Slices` sleeper. The rejected
  alternative was exporting the crate's fixture behind a feature, which is more machinery than
  one test needs.
- **One fixture for the crate's tests.** `Fx` and `Slices` move from the CLI's tests into
  `crates/tagteam-daemon/src/testutil.rs` (`#[cfg(test)]`). The scheduler's tests and the
  auto tests share them there, and Tasks 8 to 13 use and extend the same module (Execution
  notes). `Recorded` is an `EventSink` recorder, so it stays with the auto tests. `the_jitter_draw_stays_within_its_range` moves
  with `uniform_jitter`, to `scheduler.rs`.
- **A feature for the engine's hooks.** The moved `a_signal_met_inside_a_tick_stops_the_loop_with_exit_0`
  uses `Engine::on_point`, which only `tagteam-engine/test-hooks` builds. It was gated on the
  CLI's `test-support`. In the crate it is gated on the crate's new feature `test-hooks`
  (`= ["tagteam-engine/test-hooks"]`), and `tagteam/test-support` turns that on, so CI's
  `--features tagteam/test-support` still runs it. Run alone, the crate's tests take
  `--features test-hooks`.

**Files:**
- Create: `crates/tagteam-daemon/src/scheduler.rs`, `crates/tagteam-daemon/src/auto.rs`,
  `crates/tagteam-daemon/src/testutil.rs`
- Modify:
  - `crates/tagteam-daemon/src/lib.rs`: the module list
  - `crates/tagteam-daemon/Cargo.toml`: a `[features]` table with `test-hooks`
  - `crates/tagteam/Cargo.toml`: `test-support` gains `tagteam-daemon/test-hooks`
  - `crates/tagteam/src/auto.rs`: the loop and its tests move out, and the sinks stay
  - `crates/tagteam/src/app.rs`: its imports, and `parsed_auto_config`
- Test: unit tests in `scheduler.rs` and `auto.rs`. The CLI's remaining `auto::tests`, and the
  binary tests that run `auto` (`auto_cli`, `app`, `run_shell_cli`, `logging`), check that
  nothing changed.

**Interfaces:**
- Consumes:
  - Task 6's `DaemonSink`, `DaemonEvent`, `AutoEvents`, `AutoOnly` and `StopReason`.
  - From the engine:
    - `Engine::auto(&ProviderId, AutoConfig, bool) -> Result<Option<AutoEngine<'_>>, EngineError>`
    - `AutoEngine::{tick, set_config, active_next_poll_at}`
    - `Engine::{now_ms, cancel, env, provider, existing_store}`
    - `Settings::{load, mtime}`
  - From `tagteam_core::autoswitch`: `next_delay`, `announces_sleep`, `once_exit_code` and
    `most_severe`.
- Produces:
  - `tagteam_daemon::scheduler::{SLICE_MS, Sleeper, ThreadSleeper, uniform_jitter, JobCx, Ran, Job, Scheduler}`,
    as the skeleton has them, plus
    `Scheduler::with_settings_mtime(self, mtime: Option<std::time::SystemTime>) -> Self`.
  - `tagteam_daemon::auto::{AutoFlags, auto_config, AutoRun, AutoError, providers, run_loop}`,
    unchanged from the CLI's.
  - Crate-private test fixtures in `crate::testutil`: `T0`, `FAR_MS`, `cc()`, `fake()`, and
    `Fx` with `new`, `add`, `add_fake`, `read` and `settings`, and fields `env`, `kc`,
    `clock`, `http` and `engine`. Also `Slices` with `new` and `none`, and a `taken` field.

#### Part A: The scheduler

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-daemon/src/lib.rs`, replace the module list with:

```rust
pub mod events;
pub mod protocol;
pub mod scheduler;
#[cfg(test)]
mod testutil;
```

Create `crates/tagteam-daemon/src/testutil.rs`. Its `Fx` and `Slices` are the CLI's test
fixtures (`crates/tagteam/src/auto.rs`, `mod tests`), with `pub(crate)` added. `Slices::none`
panics with a message that is no longer about `--once`, since the scheduler's tests use it too:

```rust
//! The fixture the crate's tests share: a home with Claude Code and FakeAgent registered, on a
//! fake wall clock and a scripted HTTP port that answers nothing, and a sleeper that moves that
//! clock (§15.1: no real network, HOME or keychain).

use std::cell::RefCell;
use std::fs;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tagteam_cc::live::Platform;
use tagteam_cc::{ClaudeCode, ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, CLAUDE_CODE, PollBudget, PollPlan, ProviderId, Window, WindowKind};
use tagteam_engine::lifecycle::AddOptions;
use tagteam_engine::oracle::NoOracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::{Eligibility, Reserve};
use tagteam_engine::vault::{KeychainVault, Vault};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_fake::{FAKE_AGENT, FakeAgent};
use tagteam_provider::liveness::SystemProcessProbe;
use tagteam_provider::profile::RunShell;
use tagteam_provider::{Env, FakeClock, FakeKeychain, ScriptedHttp};

use crate::scheduler::Sleeper;

/// The fixture clock's start, epoch seconds.
pub(crate) const T0: i64 = 1_790_000_000;
/// An access-token expiry no test reaches, so nothing is freshened unless a test says so.
pub(crate) const FAR_MS: i64 = 4_102_444_800_000;

pub(crate) fn cc() -> ProviderId {
    ProviderId::new(CLAUDE_CODE)
}

pub(crate) fn fake() -> ProviderId {
    ProviderId::new(FAKE_AGENT)
}

/// A home with Claude Code and FakeAgent registered, on a fake wall clock that starts at
/// `T0` and a scripted HTTP port that answers nothing (every request is `PreSend`).
pub(crate) struct Fx {
    _dir: tempfile::TempDir,
    pub(crate) env: Env,
    pub(crate) kc: Arc<FakeKeychain>,
    pub(crate) clock: Arc<FakeClock>,
    pub(crate) http: Arc<ScriptedHttp>,
    pub(crate) engine: Engine,
}

impl Fx {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        fs::create_dir_all(env.home.join(".claude")).unwrap();
        let kc = Arc::new(FakeKeychain::new());
        let clock = Arc::new(FakeClock::new(T0 * 1000));
        let http = Arc::new(ScriptedHttp::new());
        let engine = Engine::new(EngineConfig {
            env: env.clone(),
            registry: ProviderRegistry::new()
                .with(Arc::new(ClaudeCode::new(kc.clone(), Platform::MacOs)))
                .with(Arc::new(FakeAgent::new())),
            vault: Vault::new(Box::new(KeychainVault::new(kc.clone()))),
            oracle: Arc::new(NoOracle),
            clock: clock.clone(),
            http: http.clone(),
            default_provider: cc(),
            settings: Settings::default(),
            process: Arc::new(SystemProcessProbe),
            // Nothing here validates, and a scripted spawner never starts a process (§15.1).
            spawner: Arc::new(tagteam_provider::process::ScriptedSpawner::new()),
            run_shell: RunShell::Outside,
        });
        Fx {
            _dir: dir,
            env,
            kc,
            clock,
            http,
            engine,
        }
    }

    /// `claude /login` as `email`, then `tagteam add`: the login stays live.
    pub(crate) fn add(&self, email: &str) -> AccountId {
        let account = json!({"emailAddress": email, "organizationUuid": "", "accountUuid": format!("uuid-{email}")});
        fs::write(
            self.env.home.join(".claude.json"),
            json!({"oauthAccount": account}).to_string(),
        )
        .unwrap();
        let credential = json!({"claudeAiOauth": {"accessToken": format!("at-{email}"), "refreshToken": format!("rt-{email}"), "expiresAt": FAR_MS, "refreshTokenExpiresAt": FAR_MS}});
        self.kc.put(
            &keychain_service(&self.env, ItemKind::OAuth),
            &keychain_account(&self.env),
            credential.to_string().as_bytes(),
        );
        self.add_live(cc())
    }

    /// A FakeAgent login as `handle`, then `tagteam add`: the login stays live.
    pub(crate) fn add_fake(&self, handle: &str) -> AccountId {
        let (token, renew) = (format!("tok-{handle}"), format!("renew-{handle}"));
        tagteam_fake::login(&self.env, handle, "ws", &token, &renew);
        self.add_live(fake())
    }

    fn add_live(&self, provider: ProviderId) -> AccountId {
        let opts = AddOptions {
            provider,
            position: None,
            alias: None,
            yes: false,
        };
        self.engine.add_live(opts).unwrap().account.id
    }

    /// `id`'s reading of `short` and `long` percent in its provider's two windows, taken at
    /// `at` with its next poll planned at `next_poll_at`, recorded as a fetch records one.
    pub(crate) fn read(&self, id: &AccountId, short: f64, long: f64, at: i64, next_poll_at: i64) {
        let store = self.engine.store().unwrap();
        let row = store.account(id).unwrap().unwrap();
        let (s, l) = if row.provider == cc() {
            ("5h", "7d")
        } else {
            ("daily", "monthly")
        };
        let window = |key: &str, kind, pct, resets_at, period_s| Window {
            key: key.into(),
            label: key.into(),
            kind,
            pct,
            resets_at: Some(resets_at),
            period_s: Some(period_s),
            detail: None,
        };
        let windows = [
            window(s, WindowKind::Short, short, T0 + 9_630, 18_000),
            window(l, WindowKind::Long, long, T0 + 291_630, 604_800),
        ];
        let budget = &PollBudget::STANDARD;
        let Reserve::Reserved(r) = store
            .reserve_usage(&row, at * 1000, Eligibility::Scheduled, budget)
            .unwrap()
        else {
            panic!("no reservation at {at}");
        };
        let plan = PollPlan {
            interval_s: next_poll_at - at,
            next_poll_at,
        };
        assert!(store.record_usage(&r, &windows, at, &plan, 180).unwrap());
    }

    /// `config.toml` as `text`, with an mtime of its own whatever the file system's
    /// granularity.
    pub(crate) fn settings(&self, text: &str, mtime_s: u64) {
        let path = self.env.config_dir().join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        let at = std::time::UNIX_EPOCH + Duration::from_secs(mtime_s);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }
}

/// The scheduler's sleep on the fixture's wall clock: each slice moves the clock on by its
/// length, then `then` runs with the slice's number (from 0), to jump the clock, rewrite the
/// settings or send a signal.
pub(crate) struct Slices<'a> {
    clock: &'a FakeClock,
    pub(crate) taken: RefCell<Vec<Duration>>,
    then: Box<dyn Fn(usize) + 'a>,
}

impl<'a> Slices<'a> {
    pub(crate) fn new(clock: &'a FakeClock, then: impl Fn(usize) + 'a) -> Self {
        Slices {
            clock,
            taken: RefCell::new(Vec::new()),
            then: Box::new(then),
        }
    }

    /// A sleep that must never be taken.
    pub(crate) fn none(clock: &'a FakeClock) -> Self {
        Self::new(clock, |_| panic!("nothing may sleep here"))
    }
}

impl Sleeper for Slices<'_> {
    fn sleep(&self, d: Duration) {
        let n = self.taken.borrow().len();
        self.taken.borrow_mut().push(d);
        self.clock.advance_ms(d.as_millis() as i64);
        (self.then)(n);
    }
}
```

Create `crates/tagteam-daemon/src/scheduler.rs` holding only this test module for now (Step 3
puts the code above it):

```rust
#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use tagteam_provider::Clock;

    use super::*;
    use crate::events::DaemonEvent;
    use crate::testutil::{Fx, Slices, T0};

    /// Drops every event: the toy jobs emit none.
    struct Quiet;

    impl DaemonSink for Quiet {
        fn emit(&self, _: &DaemonEvent) {}
    }

    /// What the toy jobs did, in order: each run and each settings change, with its time in
    /// milliseconds after `T0`.
    type Log = RefCell<Vec<(i64, String)>>;

    /// A job that writes down its runs and runs again every `every_ms`. With `stop_after`, its
    /// run of that number (from 1) stops the scheduler with `Upgrade` instead.
    struct Toy<'a> {
        name: &'static str,
        every_ms: i64,
        stop_after: Option<usize>,
        runs: usize,
        log: &'a Log,
        /// Called once, as its first run starts: to rewrite the settings, or send a signal.
        before_first: Option<Box<dyn FnOnce() + 'a>>,
    }

    impl<'a> Toy<'a> {
        fn new(name: &'static str, every_ms: i64, log: &'a Log) -> Self {
            Toy {
                name,
                every_ms,
                stop_after: None,
                runs: 0,
                log,
                before_first: None,
            }
        }
    }

    fn at(cx: &JobCx<'_>) -> i64 {
        cx.engine.now_ms() - T0 * 1000
    }

    impl Job for Toy<'_> {
        fn name(&self) -> String {
            self.name.to_owned()
        }

        fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
            if let Some(before) = self.before_first.take() {
                before();
            }
            self.runs += 1;
            self.log.borrow_mut().push((at(cx), self.name.to_owned()));
            if self.stop_after == Some(self.runs) {
                return Ran::Stop(StopReason::Upgrade);
            }
            Ran::After(self.every_ms)
        }

        fn settings_changed(&mut self, cx: &mut JobCx<'_>) {
            let entry = (at(cx), format!("{}: settings", self.name));
            self.log.borrow_mut().push(entry);
        }
    }

    /// Runs `scheduler` over the fixture's engine with no jitter and no events.
    fn run(fx: &Fx, scheduler: &mut Scheduler<'_>, sleeper: &dyn Sleeper) -> StopReason {
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &Quiet,
            jitter: &mut jitter,
        };
        scheduler.run(&mut cx, sleeper)
    }

    fn entries(log: &Log) -> Vec<(i64, String)> {
        log.borrow().clone()
    }

    fn e(at: i64, what: &str) -> (i64, String) {
        (at, what.to_owned())
    }

    #[test]
    fn the_earliest_due_job_runs_first_and_a_tie_goes_to_the_first_added() {
        let fx = Fx::new();
        let log = Log::default();
        let mut scheduler = Scheduler::new();
        scheduler.add(Box::new(Toy::new("late", 10_000, &log)), T0 * 1000 + 2_000);
        scheduler.add(Box::new(Toy::new("tie-1", 4_000, &log)), T0 * 1000);
        scheduler.add(Box::new(Toy::new("tie-2", 4_000, &log)), T0 * 1000);
        let (clock, cancel) = (fx.clock.clone(), fx.engine.cancel().clone());
        let slices = Slices::new(&fx.clock, move |_| {
            if clock.now_ms() >= (T0 + 9) * 1000 {
                cancel.request(libc::SIGTERM);
            }
        });
        assert_eq!(run(&fx, &mut scheduler, &slices), StopReason::Signal);
        assert_eq!(
            entries(&log),
            [
                e(0, "tie-1"),
                e(0, "tie-2"),
                e(2_000, "late"),
                e(4_000, "tie-1"),
                e(4_000, "tie-2"),
                e(8_000, "tie-1"),
                e(8_000, "tie-2"),
            ]
        );
        let taken = slices.taken.borrow();
        assert!(taken.iter().all(|d| *d <= Duration::from_secs(1)));
        assert_eq!(taken.iter().sum::<Duration>(), Duration::from_secs(9));
    }

    #[test]
    fn a_clock_set_back_never_stretches_a_wait_past_its_delay() {
        // The clock is set back an hour during the first slice after a run. The next run still
        // comes the 60 s delay after that, by the clock as it now reads, not an hour later.
        let fx = Fx::new();
        let log = Log::default();
        let mut scheduler = Scheduler::new();
        scheduler.add(Box::new(Toy::new("job", 60_000, &log)), T0 * 1000);
        let (clock, cancel) = (fx.clock.clone(), fx.engine.cancel().clone());
        let slices = Slices::new(&fx.clock, move |n| {
            if n == 0 {
                clock.set(clock.now_ms() - 3_600_000);
            } else if clock.now_ms() > (T0 - 3_539) * 1000 {
                cancel.request(libc::SIGTERM);
            }
        });
        assert_eq!(run(&fx, &mut scheduler, &slices), StopReason::Signal);
        assert_eq!(entries(&log), [e(0, "job"), e(-3_539_000, "job")]);
        let taken = slices.taken.borrow();
        assert_eq!(
            taken[..61].iter().sum::<Duration>(),
            Duration::from_secs(61)
        );
    }

    #[test]
    fn a_job_that_asks_to_stop_ends_the_run_with_its_reason_at_once() {
        let fx = Fx::new();
        let log = Log::default();
        let mut scheduler = Scheduler::new();
        scheduler.add(Box::new(Toy::new("tick", 1_000, &log)), T0 * 1000);
        let mut watch = Toy::new("watch", 60_000, &log);
        watch.stop_after = Some(1);
        scheduler.add(Box::new(watch), T0 * 1000 + 2_500);
        let slices = Slices::new(&fx.clock, |_| {});
        assert_eq!(run(&fx, &mut scheduler, &slices), StopReason::Upgrade);
        assert_eq!(
            entries(&log),
            [
                e(0, "tick"),
                e(1_000, "tick"),
                e(2_000, "tick"),
                e(2_500, "watch")
            ],
            "nothing runs after the stop"
        );
        assert_eq!(slices.taken.borrow().len(), 3, "and nothing sleeps");
    }

    #[test]
    fn a_set_cancel_token_ends_the_run_with_signal_before_the_next_job() {
        // §14.1: before any job, and between two jobs due at once.
        let fx = Fx::new();
        let log = Log::default();
        let mut scheduler = Scheduler::new();
        scheduler.add(Box::new(Toy::new("never", 1_000, &log)), T0 * 1000);
        fx.engine.cancel().request(libc::SIGINT);
        let none = Slices::none(&fx.clock);
        assert_eq!(run(&fx, &mut scheduler, &none), StopReason::Signal);
        assert!(entries(&log).is_empty());

        let fx = Fx::new();
        let mut scheduler = Scheduler::new();
        let mut signaller = Toy::new("first", 1_000, &log);
        let cancel = fx.engine.cancel().clone();
        signaller.before_first = Some(Box::new(move || cancel.request(libc::SIGTERM)));
        scheduler.add(Box::new(signaller), T0 * 1000);
        scheduler.add(Box::new(Toy::new("second", 1_000, &log)), T0 * 1000);
        let none = Slices::none(&fx.clock);
        assert_eq!(run(&fx, &mut scheduler, &none), StopReason::Signal);
        assert_eq!(entries(&log), [e(0, "first")]);
    }

    #[test]
    fn every_job_hears_of_each_settings_change_once_before_the_next_job_runs() {
        // D4.2: the mtime is checked in every slice. Writing the same mtime again is no
        // change; a later one is.
        let fx = Fx::new();
        let log = Log::default();
        let mut scheduler = Scheduler::new().with_settings_mtime(None);
        scheduler.add(Box::new(Toy::new("a", 100_000, &log)), T0 * 1000);
        scheduler.add(Box::new(Toy::new("b", 100_000, &log)), T0 * 1000);
        let cancel = fx.engine.cancel().clone();
        let slices = Slices::new(&fx.clock, |n| match n {
            2 => fx.settings("[autoswitch]\nthreshold = 80\n", 1),
            5 => fx.settings("[autoswitch]\nthreshold = 85\n", 1),
            7 => fx.settings("[autoswitch]\nthreshold = 70\n", 2),
            9 => cancel.request(libc::SIGTERM),
            _ => {}
        });
        assert_eq!(run(&fx, &mut scheduler, &slices), StopReason::Signal);
        assert_eq!(
            entries(&log),
            [
                e(0, "a"),
                e(0, "b"),
                e(3_000, "a: settings"),
                e(3_000, "b: settings"),
                e(8_000, "a: settings"),
                e(8_000, "b: settings"),
            ]
        );
    }

    #[test]
    fn a_change_landing_during_a_job_is_heard_before_the_next_job_runs() {
        // D4.2: a setting takes effect when the running job ends. `writer`'s run rewrites the
        // file, and `reader`, due at the same time, hears of it before it runs.
        let fx = Fx::new();
        let log = Log::default();
        let mut scheduler = Scheduler::new().with_settings_mtime(Settings::mtime(&fx.env));
        let mut writer = Toy::new("writer", 100_000, &log);
        writer.before_first = Some(Box::new(|| {
            fx.settings("[daemon]\nautoswitch = false\n", 1)
        }));
        scheduler.add(Box::new(writer), T0 * 1000);
        scheduler.add(Box::new(Toy::new("reader", 100_000, &log)), T0 * 1000);
        let cancel = fx.engine.cancel().clone();
        let slices = Slices::new(&fx.clock, move |_| cancel.request(libc::SIGTERM));
        assert_eq!(run(&fx, &mut scheduler, &slices), StopReason::Signal);
        assert_eq!(
            entries(&log),
            [
                e(0, "writer"),
                e(0, "writer: settings"),
                e(0, "reader: settings"),
                e(0, "reader"),
            ]
        );
    }

    #[test]
    fn without_a_known_mtime_the_first_check_tells_no_job() {
        // A file that was there before the run, and never changes, is no change.
        let fx = Fx::new();
        fx.settings("[autoswitch]\nthreshold = 80\n", 1);
        let log = Log::default();
        let mut scheduler = Scheduler::new();
        scheduler.add(Box::new(Toy::new("job", 100_000, &log)), T0 * 1000);
        let cancel = fx.engine.cancel().clone();
        let slices = Slices::new(&fx.clock, move |n| {
            if n == 3 {
                cancel.request(libc::SIGTERM);
            }
        });
        assert_eq!(run(&fx, &mut scheduler, &slices), StopReason::Signal);
        assert_eq!(entries(&log), [e(0, "job")]);
    }

    #[test]
    fn the_jitter_draw_stays_within_its_range() {
        for _ in 0..1_000 {
            let j = uniform_jitter();
            assert!((-1.0..1.0).contains(&j), "{j}");
        }
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-daemon scheduler`
Expected: the build fails. `Scheduler`, `Job`, `JobCx`, `Ran`, `Sleeper`, `uniform_jitter`,
`StopReason`, `Settings` and `Duration` are not found, and `testutil.rs`'s
`use crate::scheduler::Sleeper` is unresolved (`error[E0432]`).

- [ ] **Step 3: Implement**

In `crates/tagteam-daemon/src/scheduler.rs`, insert above `#[cfg(test)]`. `Sleeper`,
`ThreadSleeper`, `uniform_jitter` and `SLICE_MS` are the CLI's, with "loop" read as "scheduler"
in their doc comments:

```rust
//! D4.2's scheduler: §11.4's loop, generalised from one engine per provider to any jobs. Each
//! pass either runs the job whose wall-clock due time has passed (the earliest; on a tie, the
//! first added) or sleeps a slice of at most a second toward the earliest. The cancel token is
//! checked before each (§14.1), and the wall clock is read again after each slice, so a machine
//! suspended past a due time runs the job as soon as it wakes. `config.toml`'s mtime is checked
//! in every slice and before every job, and every job hears of a change before the next job
//! runs.

use std::time::{Duration, SystemTime};

use tagteam_engine::Engine;
use tagteam_engine::settings::Settings;

use crate::events::{DaemonSink, StopReason};

/// §11.4: the scheduler checks the cancel token and re-reads the wall clock at least this often.
pub const SLICE_MS: i64 = 1_000;

/// How the scheduler waits between its checks. `ThreadSleeper` in the binary; tests advance a
/// fake wall clock instead, and jump it to model a suspend.
pub trait Sleeper {
    fn sleep(&self, d: Duration);
}

/// The thread's own sleep. Its clock may stop while the machine is suspended, so the scheduler
/// never trusts it for a due time: each slice is at most a second, and the wall clock is read
/// again after it (§11.4).
pub struct ThreadSleeper;

impl Sleeper for ThreadSleeper {
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// The jitter draw: uniform in [-1, 1), which `next_delay` maps to `U(0.9, 1.1)`.
pub fn uniform_jitter() -> f64 {
    fastrand::f64() * 2.0 - 1.0
}

/// What a job runs with.
pub struct JobCx<'a> {
    pub engine: &'a Engine,
    /// Where its events go (D5.5).
    pub sink: &'a dyn DaemonSink,
    /// The jitter draw, in [-1, 1).
    pub jitter: &'a mut dyn FnMut() -> f64,
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ran {
    /// Run again this many milliseconds from now.
    After(i64),
    /// Stop the scheduler, for this reason.
    Stop(StopReason),
}

/// One piece of scheduled work: a provider's engine, a watch.
pub trait Job {
    /// How `status` names it (D5.4): `provider:claude-code`, `store-watch`.
    fn name(&self) -> String;
    /// One run; returns its next delay, or a reason to stop the scheduler. A run may block, on a
    /// tick's requests for one: the scheduler runs one job at a time.
    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran;
    /// `config.toml` changed (D4.2); default: nothing.
    fn settings_changed(&mut self, _cx: &mut JobCx<'_>) {}
}

/// A job and when it runs next.
struct Slot<'j> {
    job: Box<dyn Job + 'j>,
    /// The wall-clock time of its next run, epoch milliseconds.
    due_ms: i64,
    /// The delay that due time was set for, which the wait never exceeds.
    delay_ms: i64,
}

/// The jobs, and when each runs next (D4.2).
#[derive(Default)]
pub struct Scheduler<'j> {
    slots: Vec<Slot<'j>>,
    /// `config.toml`'s mtime as the jobs last read their settings; `None` until it is known.
    settings: Option<Option<SystemTime>>,
}

impl<'j> Scheduler<'j> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The mtime `config.toml` had when the jobs read their settings, taken with
    /// `Settings::mtime` before that read, so a write landing in between counts as a change at
    /// the first check rather than going unseen. Without it, the first check takes the mtime it
    /// finds and tells no job.
    pub fn with_settings_mtime(mut self, mtime: Option<SystemTime>) -> Self {
        self.settings = Some(mtime);
        self
    }

    /// Adds a job, first due at `due_ms`, epoch milliseconds.
    pub fn add(&mut self, job: Box<dyn Job + 'j>, due_ms: i64) {
        self.slots.push(Slot {
            job,
            due_ms,
            delay_ms: 0,
        });
    }

    /// Runs the jobs until one returns `Stop`, or the cancel token is set (`Signal`), which is
    /// checked before every job and in every slice. A signal that lands inside a run is the
    /// job's to end that run at its next cancellation point (§14.1).
    pub fn run(&mut self, cx: &mut JobCx<'_>, sleeper: &dyn Sleeper) -> StopReason {
        let start = cx.engine.now_ms();
        for slot in &mut self.slots {
            slot.delay_ms = (slot.due_ms - start).max(0);
        }
        loop {
            if cx.engine.cancel().requested().is_some() {
                return StopReason::Signal;
            }
            self.check_settings(cx);
            let now_ms = cx.engine.now_ms();
            for slot in &mut self.slots {
                // A wall clock set back would stretch the wait by as much: never wait longer
                // than the delay the due time was set for.
                slot.due_ms = slot.due_ms.min(now_ms + slot.delay_ms);
            }
            let next = (0..self.slots.len()).min_by_key(|&i| self.slots[i].due_ms);
            match next {
                Some(i) if self.slots[i].due_ms <= now_ms => {
                    let slot = &mut self.slots[i];
                    match slot.job.run(cx) {
                        Ran::After(delay_ms) => {
                            slot.delay_ms = delay_ms.max(0);
                            slot.due_ms = cx.engine.now_ms() + slot.delay_ms;
                        }
                        Ran::Stop(reason) => return reason,
                    }
                }
                _ => {
                    let left = next.map_or(SLICE_MS, |i| self.slots[i].due_ms - now_ms);
                    sleeper.sleep(Duration::from_millis(left.min(SLICE_MS) as u64));
                }
            }
        }
    }

    /// D4.2: when `config.toml`'s mtime changed since the jobs last read their settings,
    /// every job hears of it, in the order they were added.
    fn check_settings(&mut self, cx: &mut JobCx<'_>) {
        let mtime = Settings::mtime(cx.engine.env());
        match self.settings {
            Some(seen) if seen == mtime => {}
            None => self.settings = Some(mtime),
            Some(_) => {
                self.settings = Some(mtime);
                for slot in &mut self.slots {
                    slot.job.settings_changed(cx);
                }
            }
        }
    }
}
```

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p tagteam-daemon scheduler`
Expected: PASS, 8 tests. The build warns that `FAR_MS`, `fake`, the fields `kc` and `http`, and
the methods `add`, `add_fake`, `add_live` and `read` are never used. Part B's tests use them all,
and the warnings go with Part B.

#### Part B: `tagteam auto` on the scheduler

- [ ] **Step 5: Move the loop's tests**

In `crates/tagteam-daemon/Cargo.toml`, between the `[package]` table and `[dependencies]`, add:

```toml
[features]
# The engine's crash and cancellation points, for the scheduler's tests. Never enabled in
# release builds; `tagteam/test-support` turns it on.
test-hooks = ["tagteam-engine/test-hooks"]
```

In `crates/tagteam/Cargo.toml`, replace the `test-support` line with:

```toml
test-support = ["tagteam-provider/file-keychain", "tagteam-provider/mock-server", "tagteam-engine/test-hooks", "tagteam-cc/test-hooks", "tagteam-daemon/test-hooks"]
```

In `crates/tagteam-daemon/src/lib.rs`, add `pub mod auto;` above `pub mod events;`.

Create `crates/tagteam-daemon/src/auto.rs` holding only a test module. It starts with this head
(the CLI's `Recorded`, `looping` and `two`, unchanged, over the shared fixture):

```rust
#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::time::Duration;

    use serde_json::json;
    use tagteam_core::AccountId;
    use tagteam_engine::vault::SERVICE;
    use tagteam_provider::{Clock, FakeClock, Method};

    use super::*;
    use crate::testutil::{FAR_MS, Fx, Slices, T0, cc, fake};

    const CC_USAGE: &str = "https://api.anthropic.com/api/oauth/usage";

    /// Every event, with the fixture clock's time it was emitted at, in seconds after `T0`.
    struct Recorded<'a> {
        clock: &'a FakeClock,
        events: RefCell<Vec<(i64, AutoEvent)>>,
    }

    impl<'a> Recorded<'a> {
        fn new(clock: &'a FakeClock) -> Self {
            Recorded {
                clock,
                events: RefCell::new(Vec::new()),
            }
        }

        fn events(&self) -> Vec<(i64, AutoEvent)> {
            self.events.borrow().clone()
        }

        /// When `provider` polled: one per tick that reached §11.2 step 3.
        fn polls(&self, of: &ProviderId) -> Vec<i64> {
            self.events
                .borrow()
                .iter()
                .filter_map(|(at, e)| match e {
                    AutoEvent::Poll { provider, .. } if provider == of => Some(*at),
                    _ => None,
                })
                .collect()
        }

        /// Each `no-switch` as (seconds after `T0`, reason, detail).
        fn no_switches(&self) -> Vec<(i64, String, String)> {
            self.events
                .borrow()
                .iter()
                .filter_map(|(at, e)| match e {
                    AutoEvent::NoSwitch { reason, detail, .. } => {
                        Some((*at, reason.clone(), detail.clone()))
                    }
                    _ => None,
                })
                .collect()
        }
    }

    impl EventSink for Recorded<'_> {
        fn emit(&self, e: &AutoEvent) {
            let at = self.clock.now_ms().div_euclid(1000) - T0;
            self.events.borrow_mut().push((at, e.clone()));
        }
    }

    fn looping() -> AutoRun {
        AutoRun::default()
    }

    /// `b` at position 2 is the live login, `a` at position 1 a candidate.
    fn two(fx: &Fx) -> (AccountId, AccountId) {
        (fx.add("a@x.co"), fx.add("b@x.co"))
    }
```

Below `fn two`, copy the CLI's loop tests (from `crates/tagteam/src/auto.rs`'s `mod tests`)
and close the module with `}`. They form one contiguous block: from the `#[test]` above
`fn flags_override_the_settings_clamped_to_their_ranges` through the closing brace of
`fn consume_first_from_the_settings_or_without_provider_runs_best_with_one_warning`. These are
the 17 tests:
- `flags_override_the_settings_clamped_to_their_ranges`
- `auto_runs_every_provider_with_two_switchable_accounts_or_the_one_named`
- `with_no_provider_to_switch_on_auto_fails_before_any_tick`
- `each_provider_ticks_on_its_own_wall_clock_schedule_in_slices_of_a_second`
- `the_next_tick_comes_no_later_than_the_live_account_s_next_poll`
- `a_long_delay_is_announced_with_a_sleep_event`
- `a_suspend_past_the_deadline_ticks_on_waking_and_never_on_the_old_readings`
- `a_wall_clock_set_back_never_stretches_the_wait_past_the_delay`
- `changed_settings_are_read_again_before_the_next_tick_and_flags_still_win`
- `a_failed_tick_is_reported_and_the_loop_keeps_its_normal_cadence`
- `a_tick_that_fails_with_an_error_is_an_error_event_and_an_interruption_is_not`
- `a_signal_met_inside_a_tick_stops_the_loop_with_exit_0`, with its doc comment
- `a_loop_skips_a_provider_whose_engine_runs_elsewhere_and_fails_when_none_is_left`
- `once_ticks_each_provider_once_and_exits_with_the_most_severe_code`
- `an_interrupted_once_is_the_command_s_interruption`
- `consume_first_named_for_a_provider_without_a_long_window_is_refused_before_any_engine`
- `consume_first_from_the_settings_or_without_provider_runs_best_with_one_warning`

Copy them byte for byte, except for exactly these three edits (Decision 9, and the feature
gate):

In `changed_settings_are_read_again_before_the_next_tick_and_flags_still_win`, replace the
comment's last line

```rust
        // default). The next tick runs with 60 and switches, unless `--threshold 95` holds.
```

with

```rust
        // default). The next tick runs with 60 and switches, unless `--threshold 95` holds. The
        // scheduler reads the file again in the slice after the write (D4.2), so its
        // warning comes at 1 s, still before that tick.
```

and in the same test replace

```rust
            assert!(sink.events().contains(&(
                60,
                AutoEvent::ConfigWarning {
```

with

```rust
            assert!(sink.events().contains(&(
                1,
                AutoEvent::ConfigWarning {
```

On `a_signal_met_inside_a_tick_stops_the_loop_with_exit_0`, replace
`#[cfg(feature = "test-support")]` with `#[cfg(feature = "test-hooks")]`.

The CLI keeps its copies until Step 9.

- [ ] **Step 6: Run them and see them fail**

Run: `cargo test -p tagteam-daemon --features test-hooks auto::`
Expected: the build fails. Nothing the tests take from `super` exists yet: `run_loop`,
`AutoRun`, `AutoFlags`, `AutoError`, `providers`, `auto_config`, `configured` and `settle`, and
the module's imports `AutoEvent`, `EventSink`, `Settings`, `Strategy`, `ProviderId` and
`BTreeMap` (`E0412`, `E0425`, `E0433`).

- [ ] **Step 7: Implement**

In `crates/tagteam-daemon/src/auto.rs`, insert above `#[cfg(test)]`. Every item but the module
doc, the imports, `run_loop`'s body and `AutoJob` is the CLI's, moved unchanged: `ONCE_ERROR`,
`AutoFlags`, `auto_config`, `clamped`, `AutoRun`, `AutoError` and its `From`, `providers`,
`once`, `configured`, `settle`, `no_switch` and `engine_running`. `AutoJob::run` is
`Driver::forever`'s tick together with `Driver::schedule`, and `AutoJob::settings_changed` is
`Driver::reload_if_changed` for one provider:

```rust
//! §11.4: `tagteam auto` on the scheduler (D2). One `AutoJob` per provider wraps
//! `AutoEngine::tick`; the scheduler sleeps toward each one's wall-clock due time in slices of at
//! most a second that check the cancel token, re-reads `config.toml` when its mtime changed
//! (D4.2), and stops with exit 0 on a signal (§11.4, §14.1). `--once` runs one tick per provider
//! instead, without the scheduler.

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use tagteam_core::ProviderId;
use tagteam_core::autoswitch::{
    AutoConfig, Decision, NoSwitchReason, Strategy, announces_sleep, most_severe, next_delay,
    once_exit_code,
};
use tagteam_engine::auto::{AutoEngine, AutoEvent, EventSink, TickOutcome};
use tagteam_engine::settings::{
    COOLDOWN_SECONDS_RANGE, INTERVAL_SECONDS_RANGE, Settings, THRESHOLD_RANGE,
};
use tagteam_engine::{Engine, EngineError};

use crate::events::{AutoEvents, AutoOnly, DaemonEvent, StopReason};
use crate::scheduler::{Job, JobCx, Ran, Scheduler, Sleeper};

/// `--once`'s code for a tick that failed (§11.4).
const ONCE_ERROR: i32 = 1;

/// One invocation's flags (§11.4). Each overrides its setting for every provider, clamped to
/// the setting's range (§6.4), and still wins after a reload (Decision 9).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AutoFlags {
    pub threshold: Option<f64>,
    pub interval_s: Option<i64>,
    pub cooldown_s: Option<i64>,
    pub strategy: Option<Strategy>,
    pub models: Option<Vec<String>>,
    pub include_api_key_accounts: Option<bool>,
}

/// What one provider's engine decides with: its settings, the flags over them, and the
/// provider's long window (§4.5).
pub fn auto_config(s: &Settings, flags: &AutoFlags, long_window: Option<&str>) -> AutoConfig {
    AutoConfig {
        threshold: flags
            .threshold
            .map_or(s.threshold, |v| clamped(v, THRESHOLD_RANGE)),
        hysteresis_pct: s.hysteresis_pct,
        cooldown_s: flags
            .cooldown_s
            .map_or(s.cooldown_seconds, |v| clamped(v, COOLDOWN_SECONDS_RANGE)),
        interval_s: flags
            .interval_s
            .map_or(s.interval_seconds, |v| clamped(v, INTERVAL_SECONDS_RANGE)),
        unhealthy_ticks: s.unhealthy_ticks,
        strategy: flags.strategy.unwrap_or(s.strategy),
        include_api_key_accounts: flags
            .include_api_key_accounts
            .unwrap_or(s.include_api_key_accounts),
        models: flags.models.clone().unwrap_or_else(|| s.models.clone()),
        long_window: long_window.map(str::to_owned),
    }
}

/// `v` clamped into `range`, as §6.4 clamps a flag.
fn clamped<T: PartialOrd + Copy>(v: T, range: RangeInclusive<T>) -> T {
    let (lo, hi) = (*range.start(), *range.end());
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// What one `tagteam auto` invocation asks for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AutoRun {
    /// `--provider`: only this one. Otherwise every provider with two switchable accounts.
    pub provider: Option<ProviderId>,
    pub flags: AutoFlags,
    pub once: bool,
    pub dry_run: bool,
}

/// How `auto` ends as a command error rather than with its own exit code.
#[derive(Debug)]
pub enum AutoError {
    /// Every provider's engine lock is held by another process (§11.1).
    AlreadyRuns(Vec<ProviderId>),
    /// No provider has two switchable accounts to switch between.
    NothingToSwitch,
    /// `--strategy consume-first` with `--provider` naming a provider that has no long window to
    /// rank by (§4.5): a usage error.
    NoLongWindow(ProviderId),
    /// An interrupted `--once` (128 + n, §14.1), or a failure before any tick.
    Engine(EngineError),
}

impl From<EngineError> for AutoError {
    fn from(e: EngineError) -> Self {
        AutoError::Engine(e)
    }
}

/// §11.1: `only`, or every registered provider with at least two switchable accounts, in
/// provider order. Switchable is read from the store, as the tick reads it: enabled, with an
/// identity. A quarantine does not count against an account here: failing over from it, or
/// releasing it, is the tick's work.
pub fn providers(
    engine: &Engine,
    only: Option<&ProviderId>,
) -> Result<Vec<ProviderId>, EngineError> {
    if let Some(p) = only {
        engine.provider(p)?;
        return Ok(vec![p.clone()]);
    }
    let Some(store) = engine.existing_store()? else {
        return Ok(Vec::new());
    };
    let mut switchable: BTreeMap<ProviderId, usize> = BTreeMap::new();
    for row in store.all_accounts()? {
        if !row.disabled && row.identity_json.is_object() && engine.provider(&row.provider).is_ok()
        {
            *switchable.entry(row.provider).or_default() += 1;
        }
    }
    Ok(switchable
        .into_iter()
        .filter(|(_, n)| *n >= 2)
        .map(|(p, _)| p)
        .collect())
}

/// `tagteam auto`: `--once`'s exit code (§11.1), or the loop's 0 once a signal stops it (§11.4).
/// A loop skips a provider whose engine runs elsewhere, with a `no-switch engine-running`
/// event, and fails with `AlreadyRuns` when that leaves it none. Consume-first named for a
/// provider without a long window fails with `NoLongWindow` before any engine starts (§4.5).
/// `jitter` draws in [-1, 1). The loop is the scheduler with one `AutoJob` per provider; `sink`
/// sees only §11.4's kinds, as before (D2).
pub fn run_loop(
    engine: &Engine,
    run: &AutoRun,
    sink: &dyn EventSink,
    sleeper: &dyn Sleeper,
    jitter: &mut dyn FnMut() -> f64,
) -> Result<i32, AutoError> {
    let providers = providers(engine, run.provider.as_ref())?;
    if providers.is_empty() {
        return Err(AutoError::NothingToSwitch);
    }
    // §4.5: from the settings, or with no `--provider`, such a provider's engine runs `best`
    // with a `config-warning` instead.
    if let (Some(p), Some(Strategy::ConsumeFirst)) = (&run.provider, run.flags.strategy) {
        if engine.provider(p)?.primary_long_window().is_none() {
            return Err(AutoError::NoLongWindow(p.clone()));
        }
    }
    // Before the settings are read, so a write landing meanwhile still counts as a change.
    let mtime = Settings::mtime(engine.env());
    if run.once {
        return once(engine, &providers, run, sink);
    }
    let mut scheduler = Scheduler::new().with_settings_mtime(mtime);
    let mut driving = 0;
    let mut held = Vec::new();
    for provider in providers {
        let (cfg, long_window) = configured(engine, &provider, &run.flags)?;
        match engine.auto(&provider, cfg.clone(), run.dry_run)? {
            Some(auto) => {
                let job = AutoJob {
                    provider,
                    long_window,
                    flags: run.flags.clone(),
                    auto,
                    cfg,
                };
                scheduler.add(Box::new(job), engine.now_ms());
                driving += 1;
            }
            None => held.push(provider),
        }
    }
    if driving == 0 {
        return Err(AutoError::AlreadyRuns(held));
    }
    for provider in held {
        sink.emit(&engine_running(provider));
    }
    let only = AutoOnly(sink);
    let mut cx = JobCx {
        engine,
        sink: &only,
        jitter,
    };
    // An auto job stops the scheduler only for a signal, which ends the loop with exit 0.
    scheduler.run(&mut cx, sleeper);
    Ok(0)
}

/// One provider the loop drives: its engine, what it decides with, and the flags that still win
/// after a reload.
struct AutoJob<'e> {
    provider: ProviderId,
    long_window: Option<String>,
    flags: AutoFlags,
    auto: AutoEngine<'e>,
    cfg: AutoConfig,
}

impl Job for AutoJob<'_> {
    fn name(&self) -> String {
        format!("auto:{}", self.provider)
    }

    /// One tick, then its next delay, `next_delay` after now (§11.4), with a `sleep` event when
    /// the delay is long. A failed tick keeps the normal cadence; a signal met inside the tick
    /// stops the scheduler as cleanly as one between ticks.
    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
        let events = AutoEvents(cx.sink);
        let ticked = self.auto.tick(&events);
        let Ok((_, decision)) = settle(&self.provider, ticked, &events) else {
            return Ran::Stop(StopReason::Signal);
        };
        let jitter = (cx.jitter)();
        let now = cx.engine.now_ms().div_euclid(1000);
        let delay = next_delay(
            &decision,
            &self.cfg,
            now,
            self.auto.active_next_poll_at(),
            jitter,
        );
        if announces_sleep(delay, &self.cfg) {
            events.emit(&AutoEvent::Sleep {
                provider: self.provider.clone(),
                seconds: delay as f64,
                until: now + delay,
            });
        }
        Ran::After(delay * 1000)
    }

    /// §6.4, §11.4: the provider's settings read again, with the flags applied over them, since
    /// they still win. Each warning the read gives is a `config-warning`.
    fn settings_changed(&mut self, cx: &mut JobCx<'_>) {
        let (settings, warnings) = Settings::load(cx.engine.env(), &self.provider);
        for message in warnings {
            cx.sink.emit(&DaemonEvent::Auto(AutoEvent::ConfigWarning {
                provider: self.provider.clone(),
                message,
            }));
        }
        self.cfg = auto_config(&settings, &self.flags, self.long_window.as_deref());
        self.auto.set_config(self.cfg.clone());
    }
}

/// `--once` (§11.1): one tick per provider, in order, each holding its engine lock only for its
/// tick. A provider whose engine runs elsewhere reports `engine-running`. The exit code is the
/// most severe. A signal ends it at the next tick's opening cancellation point (§14.1).
fn once(
    engine: &Engine,
    providers: &[ProviderId],
    run: &AutoRun,
    sink: &dyn EventSink,
) -> Result<i32, AutoError> {
    let mut codes = Vec::new();
    for provider in providers {
        let (cfg, _) = configured(engine, provider, &run.flags)?;
        let code = match engine.auto(provider, cfg, run.dry_run)? {
            Some(mut auto) => settle(provider, auto.tick(sink), sink)?.0,
            None => {
                sink.emit(&engine_running(provider.clone()));
                once_exit_code(&no_switch(NoSwitchReason::EngineRunning))
            }
        };
        codes.push(code);
    }
    Ok(most_severe(&codes))
}

/// A provider's settings, flags over them, and its long window. The command printed the
/// settings file's warnings when it started, so they are not repeated here.
fn configured(
    engine: &Engine,
    provider: &ProviderId,
    flags: &AutoFlags,
) -> Result<(AutoConfig, Option<String>), EngineError> {
    let long_window = engine
        .provider(provider)?
        .primary_long_window()
        .map(str::to_owned);
    let (settings, _) = Settings::load(engine.env(), provider);
    Ok((
        auto_config(&settings, flags, long_window.as_deref()),
        long_window,
    ))
}

/// A tick's `--once` code and the decision its delay follows. `Err` only for an interruption.
/// Any other `Err` the tick returns (a store that failed) is reported here as an `error` event,
/// since the engine reports every other failure itself, and the loop keeps its normal cadence
/// (§11.4).
fn settle(
    provider: &ProviderId,
    ticked: Result<(TickOutcome, Decision), EngineError>,
    sink: &dyn EventSink,
) -> Result<(i32, Decision), EngineError> {
    match ticked {
        Ok((TickOutcome::Error, decision)) => Ok((ONCE_ERROR, decision)),
        Ok((_, decision)) => Ok((once_exit_code(&decision), decision)),
        Err(e) if e.signal().is_some() => Err(e),
        Err(e) => {
            sink.emit(&AutoEvent::Error {
                provider: provider.clone(),
                message: e.to_string(),
                transient: false,
            });
            let normal = no_switch(NoSwitchReason::NoActiveAccount);
            Ok((ONCE_ERROR, normal))
        }
    }
}

/// A `no-switch` with no detail, as `decide` would return it.
fn no_switch(reason: NoSwitchReason) -> Decision {
    Decision::NoSwitch {
        reason,
        outcome: reason.outcome(),
        detail: String::new(),
        earliest_reset: None,
    }
}

/// §11.1: the `no-switch` a provider whose engine lock another process holds reports.
fn engine_running(provider: ProviderId) -> AutoEvent {
    AutoEvent::NoSwitch {
        provider,
        reason: NoSwitchReason::EngineRunning.as_str().to_owned(),
        detail: String::new(),
    }
}
```

- [ ] **Step 8: Run them and see them pass**

Run: `cargo test -p tagteam-daemon --features test-hooks`
Expected: PASS, 34 tests: 9 from Task 6, 8 in `scheduler` and 17 in `auto`.
Run: `cargo test -p tagteam-daemon`
Expected: PASS, 33 tests (`a_signal_met_inside_a_tick_stops_the_loop_with_exit_0` needs
`test-hooks`).

#### Part C: The CLI keeps only its sinks

- [ ] **Step 9: Move the CLI onto the crate**

In `crates/tagteam/src/auto.rs`, replace everything from the first line down to the closing
brace of `fn engine_running`. That is the module doc, the imports, `SLICE_MS`, `ONCE_ERROR`,
`Sleeper`, `ThreadSleeper`, `uniform_jitter`, `AutoFlags`, `auto_config`, `clamped`,
`AutoRun`, `AutoError` and its `From`, `providers`, `run_loop`, `Slot`, `Driver`, `once`,
`configured`, `settle`, `no_switch` and `engine_running`. Replace it with:

```rust
//! §11.4's output for `tagteam auto`: human lines or JSONL, from the events the loop in
//! `tagteam_daemon::auto` emits (D2). The loop itself, its flags and its errors live there.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{self, Write};

use tagteam_core::ProviderId;
use tagteam_core::autoswitch::NoSwitchReason;
use tagteam_daemon::protocol::event_json;
use tagteam_engine::auto::{AutoEvent, EventSink};
use tagteam_provider::Cancel;

use crate::render::{MISSING, RESET, duration, severity};
```

What follows stays unchanged: `written`, `JsonSink`, `HumanSink`, `Polled`, `pct`,
`impl EventSink for HumanSink` and `local_time`.

In the same file's tests, replace everything from `#[cfg(test)]` down to the closing brace of
`fn two`. That is the imports, `T0`, `FAR_MS`, `CC_USAGE`, `cc`, `fake`, `Fx`, `Slices`,
`Recorded` and `looping`. Replace it with this head, which keeps the trimmed fixture that the
one loop test still here needs:

```rust
#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::json;
    use tagteam_cc::live::Platform;
    use tagteam_cc::{ClaudeCode, ItemKind, keychain_account, keychain_service};
    use tagteam_core::{AccountId, CLAUDE_CODE, PollBudget, PollPlan, Window, WindowKind};
    use tagteam_daemon::auto::{AutoRun, run_loop};
    use tagteam_daemon::scheduler::Sleeper;
    use tagteam_engine::lifecycle::AddOptions;
    use tagteam_engine::oracle::NoOracle;
    use tagteam_engine::registry::ProviderRegistry;
    use tagteam_engine::settings::Settings;
    use tagteam_engine::store::{Eligibility, Reserve};
    use tagteam_engine::vault::{KeychainVault, SERVICE, Vault};
    use tagteam_engine::{Engine, EngineConfig};
    use tagteam_provider::liveness::SystemProcessProbe;
    use tagteam_provider::profile::RunShell;
    use tagteam_provider::{Clock, Env, FakeClock, FakeKeychain, ScriptedHttp};

    use super::*;

    /// The fixture clock's start, epoch seconds.
    const T0: i64 = 1_790_000_000;
    /// An access-token expiry no test reaches, so nothing is freshened unless a test says so.
    const FAR_MS: i64 = 4_102_444_800_000;

    fn cc() -> ProviderId {
        ProviderId::new(CLAUDE_CODE)
    }

    /// A home with Claude Code registered, on a fake wall clock that starts at `T0` and a
    /// scripted HTTP port that answers nothing: what the one loop test here needs. The loop's
    /// own tests, and their fuller fixture, are in `tagteam_daemon::auto`.
    struct Fx {
        _dir: tempfile::TempDir,
        env: Env,
        kc: Arc<FakeKeychain>,
        clock: Arc<FakeClock>,
        engine: Engine,
    }

    impl Fx {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let env = Env::for_test(dir.path());
            fs::create_dir_all(env.home.join(".claude")).unwrap();
            let kc = Arc::new(FakeKeychain::new());
            let clock = Arc::new(FakeClock::new(T0 * 1000));
            let engine = Engine::new(EngineConfig {
                env: env.clone(),
                registry: ProviderRegistry::new()
                    .with(Arc::new(ClaudeCode::new(kc.clone(), Platform::MacOs))),
                vault: Vault::new(Box::new(KeychainVault::new(kc.clone()))),
                oracle: Arc::new(NoOracle),
                clock: clock.clone(),
                http: Arc::new(ScriptedHttp::new()),
                default_provider: cc(),
                settings: Settings::default(),
                process: Arc::new(SystemProcessProbe),
                // Nothing here validates, and a scripted spawner never starts a process (§15.1).
                spawner: Arc::new(tagteam_provider::process::ScriptedSpawner::new()),
                run_shell: RunShell::Outside,
            });
            Fx {
                _dir: dir,
                env,
                kc,
                clock,
                engine,
            }
        }

        /// `claude /login` as `email`, then `tagteam add`: the login stays live.
        fn add(&self, email: &str) -> AccountId {
            let account = json!({"emailAddress": email, "organizationUuid": "", "accountUuid": format!("uuid-{email}")});
            fs::write(
                self.env.home.join(".claude.json"),
                json!({"oauthAccount": account}).to_string(),
            )
            .unwrap();
            let credential = json!({"claudeAiOauth": {"accessToken": format!("at-{email}"), "refreshToken": format!("rt-{email}"), "expiresAt": FAR_MS, "refreshTokenExpiresAt": FAR_MS}});
            self.kc.put(
                &keychain_service(&self.env, ItemKind::OAuth),
                &keychain_account(&self.env),
                credential.to_string().as_bytes(),
            );
            let opts = AddOptions {
                provider: cc(),
                position: None,
                alias: None,
                yes: false,
            };
            self.engine.add_live(opts).unwrap().account.id
        }

        /// `id`'s reading of `short` and `long` percent in its two windows, taken at `at` with
        /// its next poll planned at `next_poll_at`, recorded as a fetch records one.
        fn read(&self, id: &AccountId, short: f64, long: f64, at: i64, next_poll_at: i64) {
            let store = self.engine.store().unwrap();
            let row = store.account(id).unwrap().unwrap();
            let window = |key: &str, kind, pct, resets_at, period_s| Window {
                key: key.into(),
                label: key.into(),
                kind,
                pct,
                resets_at: Some(resets_at),
                period_s: Some(period_s),
                detail: None,
            };
            let windows = [
                window("5h", WindowKind::Short, short, T0 + 9_630, 18_000),
                window("7d", WindowKind::Long, long, T0 + 291_630, 604_800),
            ];
            let budget = &PollBudget::STANDARD;
            let Reserve::Reserved(r) = store
                .reserve_usage(&row, at * 1000, Eligibility::Scheduled, budget)
                .unwrap()
            else {
                panic!("no reservation at {at}");
            };
            let plan = PollPlan {
                interval_s: next_poll_at - at,
                next_poll_at,
            };
            assert!(store.record_usage(&r, &windows, at, &plan, 180).unwrap());
        }
    }

    /// The loop's sleep on the fixture's wall clock: each slice moves the clock on by its
    /// length, then `then` runs with the slice's number (from 0).
    struct Slices<'a> {
        clock: &'a FakeClock,
        taken: Cell<usize>,
        then: Box<dyn Fn(usize) + 'a>,
    }

    impl<'a> Slices<'a> {
        fn new(clock: &'a FakeClock, then: impl Fn(usize) + 'a) -> Self {
            Slices {
                clock,
                taken: Cell::new(0),
                then: Box::new(then),
            }
        }
    }

    impl Sleeper for Slices<'_> {
        fn sleep(&self, d: Duration) {
            let n = self.taken.replace(self.taken.get() + 1);
            self.clock.advance_ms(d.as_millis() as i64);
            (self.then)(n);
        }
    }

    fn looping() -> AutoRun {
        AutoRun::default()
    }

    /// `b` at position 2 is the live login, `a` at position 1 a candidate.
    fn two(fx: &Fx) -> (AccountId, AccountId) {
        (fx.add("a@x.co"), fx.add("b@x.co"))
    }
```

Then delete the 17 tests that Step 5 copied, from the `#[test]` above
`fn flags_override_the_settings_clamped_to_their_ranges` through the closing brace of
`fn consume_first_from_the_settings_or_without_provider_runs_best_with_one_warning`. Also delete
`the_jitter_draw_stays_within_its_range`, which now lives in `scheduler.rs`. What stays, in
order:
- `poll`, `no_switch_event` and `human`
- `a_tick_is_one_line_and_quarantines_sleeps_errors_and_warnings_are_lines_of_their_own`
- `a_tick_that_ends_in_an_error_leaves_its_poll_to_no_later_line`
- `usage_is_coloured_as_list_colours_it_and_several_providers_are_named`
- `local_time_is_a_time_of_day`
- `Failing`
- `a_broken_pipe_on_either_sink_stops_the_command_as_sigpipe_would`

All of these are unchanged.

In `crates/tagteam/src/app.rs`, after `use tagteam_core::{AccountId, CLAUDE_CODE, Pace, ProviderId, Window};`,
add:

```rust
use tagteam_daemon::auto::{self, AutoError, AutoFlags, AutoRun};
use tagteam_daemon::scheduler::{ThreadSleeper, uniform_jitter};
```

then replace:

```rust
use crate::auto::{
    AutoError, AutoFlags, AutoRun, HumanSink, JsonSink, ThreadSleeper, uniform_jitter,
};
```

with:

```rust
use crate::auto::{HumanSink, JsonSink};
```

and drop `auto, ` from the start of the `use crate::{ … }` list, which now begins
`config_cmd, displaced_cmd,`. The call sites `auto::providers(…)` and `auto::run_loop(…)` in
`App::auto` resolve to the crate's module and stay as they are. In the test helper
`parsed_auto_config`, replace
`let c = crate::auto::auto_config(&Settings::default(), &flags, None);` with
`let c = auto::auto_config(&Settings::default(), &flags, None);`.

- [ ] **Step 10: Run the tests and see them pass**

Run: `cargo test -p tagteam --features test-support --lib auto::tests`
Expected: PASS, 5 tests.
Run: `cargo test -p tagteam --features test-support --lib app::tests` and
`cargo test -p tagteam --features test-support --test auto_cli --test app --test run_shell_cli --test logging`
Expected: PASS. `tagteam auto`'s human output, JSON, exit codes and refusals are unchanged.
Run: `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`
Expected: PASS. The pty tests (`tagteam`'s `prompt::tests`, `tests/signals.rs`) and
`xtask`'s process-group tests need a real terminal. Inside a sandbox that refuses `openpty`
they fail for that reason alone (CONTRIBUTING, "Test quirks"), and they run outside it.
Then `cargo fmt --all && cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings` and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`.
Expected: clean, with no dead-code warning left from Part A.

- [ ] **Step 11: Commit** (the controller commits; implementers do not)

```
git add crates/tagteam-daemon crates/tagteam/Cargo.toml crates/tagteam/src/auto.rs \
  crates/tagteam/src/app.rs
git commit -m "Move the auto loop onto a job scheduler in tagteam-daemon"
```

`Cargo.lock` does not change: a feature table adds no package.

---

### Task 8: Provider jobs, their modes and the status board

**Depends on:** 2, 3, 4, 5, 7

D4.3: "The daemon has one job per registered provider. Its mode is re-evaluated after each of its
runs, every 60 s at most, after any `changed` event (D4.4), and after any settings reload." The
rows of its table are tried in order, the first that applies winning: `waiting` (schema),
`idle`, `waiting` (engine lock), `auto`, `collect`. This task builds `ProviderJob`, which applies
that table through the pure `mode_for`, ticks an `AutoEngine` exactly as `tagteam auto` does while
in `auto`, runs `collect_usage(CollectMode::Scheduled {..})` while in `collect`, and writes its
line of the `StatusBoard` (Decision 12, D5.4) after each run. It also extends Task 7's shared
test fixture, `crate::testutil`, with what this task's tests and the later tasks' need.

**Readings of the spec this task commits to:**
- **Two cadences.** A run first evaluates the mode, then does that mode's work only when the
  work's own wall-clock deadline has come: a tick on §11.4's delay, or a round on D4.3's
  `collect` delay. The job asks to run again at `min(work deadline, 60 s)`, so the mode is
  evaluated at least every 60 s whatever the work's delay (Global Constraints). `idle` and
  `waiting` run every 60 s and do nothing else. A mode change makes the new mode's work due at
  once. The work deadline is clamped as the scheduler clamps a slot's (D4.2): a clock set back
  never stretches it.
- **Entering `auto`** is the job's own step, not a row of the pure table. `mode_for` answers
  with `holds_lock` as given. When the answer is not `auto` but would be with the lock, the job
  tries `engine.auto(..)` once. A refused try is `waiting(engine-lock)`, naming the record's pid
  when that holder is live, and no pid when the record is empty or names a dead process (the
  holder took the lock and has not written its record yet). `ModeInputs` has `refused` for
  this (Interfaces).
- **`collect` never tries the lock** (D4.3). It judges the record with `engine_holder` and
  `ProcessStamp::liveness()`: `Live` or `Unknown` is a live holder. While the job holds the lock
  it reads no record, since the record is its own.
- **Leaving `auto`** drops the `AutoEngine` inside the evaluation, which empties its record and
  frees the lock (Decision 5). `settings_changed` evaluates at once, so pausing drops the lock
  before the scheduler's next job or sleep.
- **A tick ending `unmanaged-active-account` or `no-active-account`** (and not an `Error` tick,
  whose placeholder decision is also `no-active-account`) is followed by `collect`'s round in
  the same run. That round reports no quarantine itself: the next tick's step 1 does (Decision
  6), so nothing is reported twice. The tick's delay is still `next_delay` of its decision.
- **Quarantine reporting in `collect`** uses the job's own `QuarantineWatch`, reset on each entry
  into `collect`. A change another process made between the last tick and the first `collect`
  round is not reported; the tick reported everything before, and `collect` everything after.
- **`usage`** (D5.5) lists the recorded positions sorted, and `fetchErrors` maps `Failed {kind}`
  to its kind and `OverBudget` to `over-budget`, as a tick's `poll` does. It is emitted only when
  something was recorded or failed.
- **The `collect` delay** is `interval × U(0.9, 1.1)`, shortened to the earliest `next_poll_at`
  among the switchable, unquarantined accounts (the ones a round can collect), floored at 60 s.
- **The first evaluation is a `mode` event too**, so a subscriber and the human output see each
  provider's starting mode.
- **`lastOutcome`** of a tick is `switch`, `no-switch` with the decision's reason, or `error` with
  the message of the tick's last `error` event. A `collect` round is `collect`; a round or an
  evaluation that fails is `error` with its message. `lastTickAt` is when the work began, and
  `nextTickAt` its deadline, for ticks and rounds alike, and `None` in `idle` and `waiting`.
- **Notifications** are Task 11's. This task leaves the private hook `after_run_notices`, called
  after each run that did work, with an empty body. `ProviderJobCtx` gets its `notices` field in
  Task 11, since its type, `NoticeQueue`, does not exist yet. Every test builds its context
  through `testutil::job_ctx`, so Task 11 adds the field's `None` in one place.
- **Times on the board are epoch seconds** (Decision 12): `last_tick_at`, `next_tick_at` and
  each outcome's and switch's `at` are the scheduler's milliseconds divided by 1000.
- **The first read's warnings** (Decision 8). `ProviderJob::new` keeps the warnings of its
  `Settings::load`, and the first `run` emits them as `config-warning` events before it
  evaluates the mode. `new` itself emits nothing (it has no sink). A `settings_changed` before
  the first run reports its own read and drops the first read's, which no longer describe the
  file.

**Files:**
- Create: `crates/tagteam-daemon/src/status.rs`
- Create: `crates/tagteam-daemon/src/provider_job.rs`
- Modify: `crates/tagteam-daemon/src/testutil.rs` (Task 7's fixture: `engine_on` with no live-read
  retry delay, `Fx::login`, `CC_USAGE` and `two`; then `Emitted`, `board`, `job_ctx` and
  `status`)
- Modify: `crates/tagteam-daemon/src/lib.rs` (`pub mod provider_job;` and `pub mod status;`)
- Modify: `crates/tagteam-daemon/src/auto.rs` (its `mod tests` takes `CC_USAGE` and `two` from
  `testutil`; `no_switch` becomes `pub(crate)`, so the provider job settles a failed tick with
  the same decision `auto` does)
- Test: `crates/tagteam-daemon/src/status.rs`, `crates/tagteam-daemon/src/provider_job.rs`

**Interfaces:**
- Consumes:
  - Task 2: `AutoEngine::with_source(self, &'static str) -> Self`, `AutoEngine::set_config`,
    `AutoEngine::tick`, `AutoEngine::active_next_poll_at`, `impl Drop for AutoEngine`,
    `QuarantineWatch::{new, report, note_quarantined}`,
    `Engine::engine_holder(&self, &ProviderId) -> Read<ProcessStamp>`.
  - Task 3: nothing directly (`existing_store` is today's).
  - Task 5: `Settings.daemon_autoswitch`, `NotifyEvent`.
  - Task 6: `DaemonEvent`, `DaemonSink`, `AutoEvents`, `Mode`, `WaitingFor`, `Backoff`,
    `StopReason`.
  - Task 7: `Job`, `JobCx`, `Ran`, `Scheduler`, `Sleeper`, `auto_config`, `AutoFlags`; and
    `crate::testutil`'s `T0`, `FAR_MS`, `cc`, `fake`, `Fx` (`new`, `add`, `add_fake`, `read`,
    `settings`) and `Slices`, with the `CC_USAGE` constant and `two` that `auto.rs`'s tests
    hold. The crate's dependencies from Task 6 (`libc`, `serde_json`, `tracing`) and
    dev-dependencies (`tempfile`, `tagteam-cc`, `tagteam-fake`).
  - Task 4: `tagteam_provider::keychain::ReadCounts` (only as a field type here).
- Produces (skeleton names, plus the notes' additions):
  - `status.rs`: `StatusSnapshot`, `DaemonInfo`, `NotifyInfo`, `ProviderStatus`, `OutcomeKind`
    (`as_str`), `LastOutcome`, `LastSwitch`, `JobStatus`, and
    `StatusBoard::{new, snapshot, set_provider, set_job, set_subscribers, set_socket, set_notifications}`
    exactly as the skeleton gives them.
  - `provider_job.rs`: `ProviderJob<'e>`, `ProviderJob::new(engine: &'e Engine, provider:
    ProviderId, ctx: ProviderJobCtx) -> Self`, `impl Job for ProviderJob<'_>` named
    `provider:<id>`, `ProviderJobCtx { board, keychain_counts, schema_newer }` (Task 11 adds
    `notices`), `ModeInputs` (with `refused`), `mode_for(&ModeInputs) -> (Mode,
    Option<WaitingFor>)`, and `pub const JOB_PREFIX: &str = "provider:"`.
  - `testutil.rs` additions (`#[cfg(test)]`, crate-private): `CC_USAGE`, `two`, the private
    `engine_on`, `Fx::login`, `Emitted` (`new`, `events`, `modes`, `polls`), `board`,
    `job_ctx` and `status`.

- [ ] **Step 1: Extend the shared test fixture**

Task 7 created `crates/tagteam-daemon/src/testutil.rs`, the crate's one test fixture. This task's
tests need four more things there, and Tasks 9 to 13 build on them:
- `Fx::login`, a live login with no `add`
- Claude Code's live Keychain reads retried without the 300 ms wait, as the engine's own
  `testutil` builds it, so a locked keychain costs a test no time. The engine is built by a
  private `engine_on`, which Task 10's `Fx::other` reuses.
- `CC_USAGE` and `two`, which `auto.rs`'s tests hold today

No moved test locks the keychain, so none changes.

In `crates/tagteam-daemon/src/testutil.rs`, replace the import

```rust
use tagteam_cc::live::Platform;
```

with

```rust
use tagteam_cc::live::{LiveStore, Platform};
```

and after `use tagteam_provider::liveness::SystemProcessProbe;` add:

```rust
use tagteam_provider::process::ScriptedSpawner;
```

After `pub(crate) const FAR_MS: i64 = 4_102_444_800_000;`, add:

```rust
/// Claude Code's usage endpoint (§8.1), as `ScriptedHttp` scripts it.
pub(crate) const CC_USAGE: &str = "https://api.anthropic.com/api/oauth/usage";
```

Before `/// A home with Claude Code and FakeAgent registered, …` (the doc comment of `struct Fx`),
add:

```rust
/// An engine on `env` with Claude Code and FakeAgent registered. Claude Code's live Keychain
/// reads are retried without a wait (Appendix A.3), so a locked keychain costs a test no time.
fn engine_on(
    env: &Env,
    kc: &Arc<FakeKeychain>,
    clock: &Arc<FakeClock>,
    http: &Arc<ScriptedHttp>,
) -> Engine {
    let claude = ClaudeCode::with_store(
        LiveStore::new(kc.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO),
    );
    Engine::new(EngineConfig {
        env: env.clone(),
        registry: ProviderRegistry::new()
            .with(Arc::new(claude))
            .with(Arc::new(FakeAgent::new())),
        vault: Vault::new(Box::new(KeychainVault::new(kc.clone()))),
        oracle: Arc::new(NoOracle),
        clock: clock.clone(),
        http: http.clone(),
        default_provider: cc(),
        settings: Settings::default(),
        process: Arc::new(SystemProcessProbe),
        // Nothing here validates, and a scripted spawner never starts a process (§15.1).
        spawner: Arc::new(ScriptedSpawner::new()),
        run_shell: RunShell::Outside,
    })
}
```

In `Fx::new`, replace the whole `let engine = Engine::new(EngineConfig { … });` statement with:

```rust
        let engine = engine_on(&env, &kc, &clock, &http);
```

Replace `Fx::add`:

```rust
    /// `claude /login` as `email`, then `tagteam add`: the login stays live.
    pub(crate) fn add(&self, email: &str) -> AccountId {
        let account = json!({"emailAddress": email, "organizationUuid": "", "accountUuid": format!("uuid-{email}")});
        fs::write(
            self.env.home.join(".claude.json"),
            json!({"oauthAccount": account}).to_string(),
        )
        .unwrap();
        let credential = json!({"claudeAiOauth": {"accessToken": format!("at-{email}"), "refreshToken": format!("rt-{email}"), "expiresAt": FAR_MS, "refreshTokenExpiresAt": FAR_MS}});
        self.kc.put(
            &keychain_service(&self.env, ItemKind::OAuth),
            &keychain_account(&self.env),
            credential.to_string().as_bytes(),
        );
        self.add_live(cc())
    }
```

with:

```rust
    /// `claude /login` as `email`: the live login's `oauthAccount` and its Keychain credential.
    pub(crate) fn login(&self, email: &str) {
        let account = json!({"emailAddress": email, "organizationUuid": "", "accountUuid": format!("uuid-{email}")});
        fs::write(
            self.env.home.join(".claude.json"),
            json!({"oauthAccount": account}).to_string(),
        )
        .unwrap();
        let credential = json!({"claudeAiOauth": {"accessToken": format!("at-{email}"), "refreshToken": format!("rt-{email}"), "expiresAt": FAR_MS, "refreshTokenExpiresAt": FAR_MS}});
        self.kc.put(
            &keychain_service(&self.env, ItemKind::OAuth),
            &keychain_account(&self.env),
            credential.to_string().as_bytes(),
        );
    }

    /// `claude /login` as `email`, then `tagteam add`: the login stays live.
    pub(crate) fn add(&self, email: &str) -> AccountId {
        self.login(email);
        self.add_live(cc())
    }
```

After the closing brace of `impl Fx`, add:

```rust
/// `b` at position 2 is the live login, `a` at position 1 a candidate.
pub(crate) fn two(fx: &Fx) -> (AccountId, AccountId) {
    (fx.add("a@x.co"), fx.add("b@x.co"))
}
```

In `crates/tagteam-daemon/src/auto.rs`'s `mod tests`, delete `const CC_USAGE` and `fn two` (with
its doc comment), which `testutil` now holds, and replace

```rust
    use crate::testutil::{FAR_MS, Fx, Slices, T0, cc, fake};
```

with

```rust
    use crate::testutil::{CC_USAGE, FAR_MS, Fx, Slices, T0, cc, fake, two};
```

and delete `use tagteam_core::AccountId;` from the same module's imports: `fn two` was the only
code there that named it, so it would be an unused import at this task's clippy run. If the
compiler then names another import of that `mod tests` unused, delete it too.

Run: `cargo test -p tagteam-daemon --features test-hooks auto::`
Expected: PASS, the 17 moved loop tests unchanged.

- [ ] **Step 2: Write the failing status tests**

Create `crates/tagteam-daemon/src/status.rs` holding only this test module (Step 4 puts the
implementation above it), and in `crates/tagteam-daemon/src/lib.rs` add `pub mod status;` with
the other module lines:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn daemon() -> DaemonInfo {
        DaemonInfo {
            pid: 4242,
            version: "20261014.2045.0".into(),
            started_at: 1_790_000_000,
            socket: None,
            log: PathBuf::from("/home/u/.local/state/tagteam/tagteam.log"),
        }
    }

    fn notify() -> NotifyInfo {
        NotifyInfo {
            enabled: true,
            backend: Some("osascript"),
            events: vec![NotifyEvent::Switch, NotifyEvent::Quarantine, NotifyEvent::Limit],
        }
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

    fn job(name: &str, at: i64) -> JobStatus {
        JobStatus {
            name: name.into(),
            last_run_at: Some(at),
            next_run_at: Some(at + 2),
            last_error: None,
        }
    }

    #[test]
    fn a_new_board_holds_the_daemon_and_its_notifications_and_nothing_else() {
        let board = StatusBoard::new(daemon(), notify());
        assert_eq!(
            board.snapshot(),
            StatusSnapshot {
                daemon: daemon(),
                notifications: notify(),
                providers: Vec::new(),
                jobs: Vec::new(),
                subscribers: 0,
            }
        );
    }

    #[test]
    fn providers_stay_in_provider_order_and_jobs_in_the_order_they_first_reported() {
        // D5.4: `providers` lists the provider jobs, `jobs` every other job. A job's line is
        // replaced in place, whichever order the jobs ran in.
        let board = StatusBoard::new(daemon(), notify());
        board.set_provider(provider("fake-agent", Mode::Collect));
        board.set_provider(provider("claude-code", Mode::Idle));
        board.set_provider(provider("claude-code", Mode::Auto));
        board.set_job(job("upgrade-watch", 10));
        board.set_job(job("store-watch", 10));
        board.set_job(job("upgrade-watch", 70));
        let snap = board.snapshot();
        let providers: Vec<(&str, Mode)> = snap
            .providers
            .iter()
            .map(|p| (p.provider.as_str(), p.mode))
            .collect();
        assert_eq!(
            providers,
            [("claude-code", Mode::Auto), ("fake-agent", Mode::Collect)]
        );
        assert_eq!(snap.jobs, [job("upgrade-watch", 70), job("store-watch", 10)]);
    }

    #[test]
    fn the_server_s_fields_are_kept_beside_the_jobs_lines() {
        // Decision 12: plan B's server sets the subscribers and the socket; the notifier's
        // state may change. None of it touches what the jobs wrote.
        let board = StatusBoard::new(daemon(), notify());
        board.set_provider(provider("claude-code", Mode::Auto));
        let socket = PathBuf::from("/home/u/.local/share/tagteam/daemon.sock");
        board.set_subscribers(3);
        board.set_socket(Some(socket.clone()));
        let off = NotifyInfo {
            enabled: false,
            backend: None,
            events: Vec::new(),
        };
        board.set_notifications(off.clone());
        let snap = board.snapshot();
        assert_eq!(snap.subscribers, 3);
        assert_eq!(snap.daemon.socket, Some(socket));
        assert_eq!(snap.notifications, off);
        assert_eq!(snap.providers, [provider("claude-code", Mode::Auto)]);
    }

    #[test]
    fn outcome_kinds_are_spelled_as_status_spells_them() {
        let kinds = [
            OutcomeKind::Switch,
            OutcomeKind::NoSwitch,
            OutcomeKind::Collect,
            OutcomeKind::Error,
        ]
        .map(OutcomeKind::as_str);
        assert_eq!(kinds, ["switch", "no-switch", "collect", "error"]);
    }
}
```

- [ ] **Step 3: Run them and see them fail**

Run: `cargo test -p tagteam-daemon status::`
Expected: FAIL to compile: `cannot find type StatusBoard in this scope`, and likewise
`DaemonInfo`, `NotifyInfo`, `ProviderStatus`, `JobStatus`, `OutcomeKind`.

- [ ] **Step 4: Implement the status board**

Put this above the test module in `crates/tagteam-daemon/src/status.rs`:

```rust
//! D5.4: the daemon's status, as a snapshot its jobs keep current (Decision 12). The
//! scheduler's jobs write it after each run; plan B's server reads it and fills in
//! `subscribers` and `socket`. Reading it never touches the store and never waits for a job.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tagteam_core::ProviderId;
use tagteam_engine::settings::NotifyEvent;

use crate::events::{Backoff, Mode, WaitingFor};

/// Everything `status` reports (D5.4), as the last job to run left it.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusSnapshot {
    pub daemon: DaemonInfo,
    pub notifications: NotifyInfo,
    /// One entry per provider job, in provider order.
    pub providers: Vec<ProviderStatus>,
    /// Every job other than the provider jobs, in the order each first reported.
    pub jobs: Vec<JobStatus>,
    pub subscribers: usize,
}

/// The daemon itself. Times are epoch seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct DaemonInfo {
    pub pid: u32,
    pub version: String,
    pub started_at: i64,
    /// The socket it serves; `None` before one is bound, or when none could be (D5.1).
    pub socket: Option<PathBuf>,
    pub log: PathBuf,
}

/// D6: whether notifications are on, through which backend, and for which events.
#[derive(Debug, Clone, PartialEq)]
pub struct NotifyInfo {
    pub enabled: bool,
    /// `osascript`, `notify-send`, or `None` without a backend.
    pub backend: Option<&'static str>,
    pub events: Vec<NotifyEvent>,
}

/// One provider job's line (D4.3, D5.4). Times are epoch seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderStatus {
    pub provider: ProviderId,
    pub mode: Mode,
    /// The effective `daemon.autoswitch` (D7).
    pub autoswitch: bool,
    pub waiting_for: Option<WaitingFor>,
    /// The other engine's pid, while waiting for its lock and its record names it.
    pub holder_pid: Option<u32>,
    /// The keychain backoff (D4.1), while it holds.
    pub backoff: Option<Backoff>,
    /// When the last tick or `collect` round began, and when the next is due.
    pub last_tick_at: Option<i64>,
    pub next_tick_at: Option<i64>,
    pub last_outcome: Option<LastOutcome>,
    /// The last switch this daemon made.
    pub last_switch: Option<LastSwitch>,
}

/// What a provider job's last run came to (D5.4 `lastOutcome.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeKind {
    Switch,
    NoSwitch,
    Collect,
    Error,
}

impl OutcomeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OutcomeKind::Switch => "switch",
            OutcomeKind::NoSwitch => "no-switch",
            OutcomeKind::Collect => "collect",
            OutcomeKind::Error => "error",
        }
    }
}

/// A run's outcome. `reason` is the kebab-case reason of a `no-switch`, or the message of an
/// `error`; `None` otherwise.
#[derive(Debug, Clone, PartialEq)]
pub struct LastOutcome {
    pub kind: OutcomeKind,
    pub reason: Option<String>,
    pub at: i64,
}

/// A switch this daemon made: positions, and its trigger's spelling (§11.4).
#[derive(Debug, Clone, PartialEq)]
pub struct LastSwitch {
    pub at: i64,
    pub from: u32,
    pub to: u32,
    pub trigger: String,
}

/// A job other than a provider job (D5.4 `jobs`). Times are epoch seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct JobStatus {
    pub name: String,
    pub last_run_at: Option<i64>,
    pub next_run_at: Option<i64>,
    pub last_error: Option<String>,
}

/// Decision 12: the snapshot, behind a lock no job holds across anything slow.
pub struct StatusBoard {
    snap: Mutex<StatusSnapshot>,
}

impl StatusBoard {
    pub fn new(daemon: DaemonInfo, notifications: NotifyInfo) -> Arc<Self> {
        Arc::new(StatusBoard {
            snap: Mutex::new(StatusSnapshot {
                daemon,
                notifications,
                providers: Vec::new(),
                jobs: Vec::new(),
                subscribers: 0,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, StatusSnapshot> {
        self.snap.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn snapshot(&self) -> StatusSnapshot {
        self.lock().clone()
    }

    /// Replaces the provider's line, or adds it in provider order.
    pub fn set_provider(&self, s: ProviderStatus) {
        let mut snap = self.lock();
        match snap
            .providers
            .binary_search_by(|p| p.provider.cmp(&s.provider))
        {
            Ok(i) => snap.providers[i] = s,
            Err(i) => snap.providers.insert(i, s),
        }
    }

    /// Replaces the job's line by its name, or adds it last.
    pub fn set_job(&self, s: JobStatus) {
        let mut snap = self.lock();
        match snap.jobs.iter_mut().find(|j| j.name == s.name) {
            Some(j) => *j = s,
            None => snap.jobs.push(s),
        }
    }

    pub fn set_subscribers(&self, n: usize) {
        self.lock().subscribers = n;
    }

    pub fn set_socket(&self, path: Option<PathBuf>) {
        self.lock().daemon.socket = path;
    }

    pub fn set_notifications(&self, n: NotifyInfo) {
        self.lock().notifications = n;
    }
}
```

- [ ] **Step 5: Run them and see them pass**

Run: `cargo test -p tagteam-daemon status::`
Expected: PASS (4 tests).

- [ ] **Step 6: Write the failing provider job tests**

Add the daemon helpers to `crates/tagteam-daemon/src/testutil.rs`. Extend its imports:

```rust
use std::path::PathBuf;

use tagteam_engine::settings::NotifyEvent;
use tagteam_provider::Clock;

use crate::events::{DaemonEvent, DaemonSink, Mode, WaitingFor};
use crate::provider_job::ProviderJobCtx;
use crate::status::{DaemonInfo, NotifyInfo, ProviderStatus, StatusBoard};
```

(merge `NotifyEvent` into the existing `tagteam_engine::settings` line and `Clock` into the
existing `tagteam_provider` line), and append:

```rust
/// Every daemon event, with the fixture clock's time it was emitted at, in seconds after `T0`.
pub(crate) struct Emitted<'a> {
    clock: &'a FakeClock,
    events: RefCell<Vec<(i64, DaemonEvent)>>,
}

impl<'a> Emitted<'a> {
    pub(crate) fn new(clock: &'a FakeClock) -> Self {
        Emitted {
            clock,
            events: RefCell::new(Vec::new()),
        }
    }

    pub(crate) fn events(&self) -> Vec<(i64, DaemonEvent)> {
        self.events.borrow().clone()
    }

    /// Each `mode` event of `of`: (seconds after `T0`, mode, waiting for, holder pid).
    pub(crate) fn modes(&self, of: &ProviderId) -> Vec<(i64, Mode, Option<WaitingFor>, Option<u32>)> {
        self.events
            .borrow()
            .iter()
            .filter_map(|(at, e)| match e {
                DaemonEvent::Mode {
                    provider,
                    mode,
                    waiting_for,
                    holder_pid,
                    ..
                } if provider == of => Some((*at, *mode, *waiting_for, *holder_pid)),
                _ => None,
            })
            .collect()
    }

    /// When `of` polled: one per tick that reached §11.2 step 3.
    pub(crate) fn polls(&self, of: &ProviderId) -> Vec<i64> {
        self.events
            .borrow()
            .iter()
            .filter_map(|(at, e)| match e {
                DaemonEvent::Auto(tagteam_engine::auto::AutoEvent::Poll { provider, .. })
                    if provider == of =>
                {
                    Some(*at)
                }
                _ => None,
            })
            .collect()
    }
}

impl DaemonSink for Emitted<'_> {
    fn emit(&self, e: &DaemonEvent) {
        let at = self.clock.now_ms().div_euclid(1000) - T0;
        self.events.borrow_mut().push((at, e.clone()));
    }
}

/// A board for a daemon started at `T0`, with every notification on.
pub(crate) fn board() -> Arc<StatusBoard> {
    StatusBoard::new(
        DaemonInfo {
            pid: 4242,
            version: "20261014.2045.0".into(),
            started_at: T0,
            socket: None,
            log: PathBuf::from("/home/u/.local/state/tagteam/tagteam.log"),
        },
        NotifyInfo {
            enabled: true,
            backend: None,
            events: vec![NotifyEvent::Switch, NotifyEvent::Quarantine, NotifyEvent::Limit],
        },
    )
}

/// A provider job's context as Linux gives it: no Keychain counts, and a store this build
/// reads. Every test builds its context here, so a field added later is added once.
pub(crate) fn job_ctx(board: &Arc<StatusBoard>) -> ProviderJobCtx {
    ProviderJobCtx {
        board: board.clone(),
        keychain_counts: Vec::new(),
        schema_newer: false,
    }
}

/// `of`'s line of the board.
pub(crate) fn status(board: &StatusBoard, of: &ProviderId) -> ProviderStatus {
    board
        .snapshot()
        .providers
        .into_iter()
        .find(|p| &p.provider == of)
        .expect("the provider job has run")
}
```

Create `crates/tagteam-daemon/src/provider_job.rs` holding only this test module (Step 8 puts
the implementation above it), and add `pub mod provider_job;` to
`crates/tagteam-daemon/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use serde_json::{Value, json};
    use tagteam_engine::settings::config_path;
    use tagteam_provider::{FlockGuard, Method};

    use super::*;
    use crate::scheduler::Scheduler;
    use crate::testutil::{
        CC_USAGE, Emitted, Fx, Slices, T0, board, cc, fake, job_ctx, status, two,
    };

    /// Claude Code's configuration under the default settings.
    fn cfg() -> AutoConfig {
        auto_config(&Settings::default(), &AutoFlags::default(), Some("7d"))
    }

    /// A usage reply of 12 % and 30 % in Claude Code's two windows (§8.1).
    fn usage_body() -> Value {
        json!({
            "five_hour": {"utilization": 12.0, "resets_at": "2030-01-04T17:20:00.000000+00:00"},
            "seven_day": {"utilization": 30.0, "resets_at": "2030-01-08T00:00:00.000000+00:00"}
        })
    }

    /// `a` at 10 % and the live `b` at `b_long` % of the long window, both read at `T0` and
    /// next due at `T0 + 300`: no tick or round fetches anything for five minutes.
    fn read_two(fx: &Fx, b_long: f64) -> (AccountId, AccountId) {
        let (a, b) = two(fx);
        fx.read(&a, 10.0, 10.0, T0, T0 + 300);
        fx.read(&b, 20.0, b_long, T0, T0 + 300);
        (a, b)
    }

    fn mode_event(provider: ProviderId, mode: Mode) -> DaemonEvent {
        DaemonEvent::Mode {
            provider,
            mode,
            waiting_for: None,
            holder_pid: None,
            backoff: None,
        }
    }

    #[test]
    fn the_mode_table_s_rows_apply_in_order() {
        // D4.3: the rows are tried in order, and the first that applies wins.
        let full = ModeInputs {
            has_store: true,
            switchable: 2,
            autoswitch: true,
            holds_lock: true,
            ..ModeInputs::default()
        };
        let engine_lock = (Mode::Waiting, Some(WaitingFor::EngineLock));
        // A newer schema, before anything else; the store is not even opened.
        let schema = (Mode::Waiting, Some(WaitingFor::Schema));
        assert_eq!(mode_for(&ModeInputs { schema_newer: true, ..full }), schema);
        assert_eq!(
            mode_for(&ModeInputs {
                schema_newer: true,
                ..ModeInputs::default()
            }),
            schema
        );
        // No store, or no switchable account, even with another engine running.
        assert_eq!(mode_for(&ModeInputs { has_store: false, ..full }), (Mode::Idle, None));
        assert_eq!(
            mode_for(&ModeInputs {
                switchable: 0,
                holder_elsewhere: Some(7),
                holds_lock: false,
                ..full
            }),
            (Mode::Idle, None)
        );
        // Another live engine, paused or not, with one account or two; or a refused try.
        for i in [
            ModeInputs { holder_elsewhere: Some(7), holds_lock: false, ..full },
            ModeInputs { holder_elsewhere: Some(7), holds_lock: false, autoswitch: false, ..full },
            ModeInputs { holder_elsewhere: Some(7), holds_lock: false, switchable: 1, ..full },
            ModeInputs { refused: true, holds_lock: false, ..full },
            ModeInputs { holder_elsewhere: Some(7), ..full },
        ] {
            assert_eq!(mode_for(&i), engine_lock, "{i:?}");
        }
        // Switching on, two switchable accounts, and the lock held.
        assert_eq!(mode_for(&full), (Mode::Auto, None));
        // Otherwise. Entering `auto` is the job's try of the lock, not the table's.
        for i in [
            ModeInputs { autoswitch: false, holds_lock: false, ..full },
            ModeInputs { autoswitch: false, ..full },
            ModeInputs { switchable: 1, holds_lock: false, ..full },
            ModeInputs { holds_lock: false, ..full },
        ] {
            assert_eq!(mode_for(&i), (Mode::Collect, None), "{i:?}");
        }
    }

    #[test]
    fn a_collect_round_waits_its_interval_shortened_to_the_earliest_poll_and_floored() {
        // D4.3: interval × U(0.9, 1.1), shortened to the earliest `next_poll_at`, at least 60 s.
        assert_eq!(collect_delay_s(300, T0, None, 0.0), 300);
        assert_eq!(collect_delay_s(300, T0, None, 1.0), 330);
        assert_eq!(collect_delay_s(300, T0, None, -1.0), 270);
        assert_eq!(collect_delay_s(300, T0, None, f64::NAN), 300);
        assert_eq!(collect_delay_s(300, T0, Some(T0 + 100), 0.0), 100);
        assert_eq!(collect_delay_s(300, T0, Some(T0 + 10), 0.0), 60);
        assert_eq!(collect_delay_s(300, T0, Some(T0 - 500), 0.0), 60);
        assert_eq!(collect_delay_s(15, T0, None, 1.0), 60);
    }

    #[test]
    fn accounts_added_move_the_provider_from_idle_to_collect_to_auto_and_back() {
        // D4.3 as `add`, a second `add` and a disable change the store: no store is `idle`,
        // one switchable account `collect`, two `auto` with the engine lock, and one again
        // `collect`, the lock let go and its record emptied. A mode that works starts with
        // its work at once.
        let fx = Fx::new();
        fx.http.push_json(Method::Get, CC_USAGE, 200, usage_body());
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        assert_eq!(job.run(&mut cx), Ran::After(60_000));
        let a = fx.add("a@x.co");
        job.run(&mut cx);
        fx.add("b@x.co");
        job.run(&mut cx);
        assert!(
            fx.engine.auto(&cc(), cfg(), false).unwrap().is_none(),
            "the job holds the engine lock"
        );
        fx.engine.store().unwrap().set_disabled(&a, true).unwrap();
        job.run(&mut cx);
        assert_eq!(
            events.modes(&cc()),
            [
                (0, Mode::Idle, None, None),
                (0, Mode::Collect, None, None),
                (0, Mode::Auto, None, None),
                (0, Mode::Collect, None, None),
            ]
        );
        assert!(matches!(fx.engine.engine_holder(&cc()), Read::Absent));
        assert!(fx.engine.auto(&cc(), cfg(), false).unwrap().is_some());
        // The first `collect` round recorded a's first reading.
        let first = DaemonEvent::Usage {
            provider: cc(),
            updated: vec![1],
            fetch_errors: BTreeMap::new(),
        };
        assert!(events.events().iter().any(|(_, e)| *e == first));
    }

    #[test]
    fn a_lock_another_engine_holds_is_waited_for_and_taken_once_it_is_dropped() {
        // D4.3: entering `auto` tries the lock once. Another engine holds it, and its record
        // names a live process (this one's, as a `tagteam auto` would name its own), so the
        // job waits and names that pid. It takes the lock at its next look once it is free.
        let fx = Fx::new();
        read_two(&fx, 50.0);
        let held = fx.engine.auto(&cc(), cfg(), false).unwrap().unwrap();
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        assert_eq!(job.run(&mut cx), Ran::After(60_000));
        let pid = Some(std::process::id());
        assert_eq!(
            events.modes(&cc()),
            [(0, Mode::Waiting, Some(WaitingFor::EngineLock), pid)]
        );
        let s = status(&board, &cc());
        assert_eq!(
            (s.mode, s.waiting_for, s.holder_pid, s.next_tick_at),
            (Mode::Waiting, Some(WaitingFor::EngineLock), pid, None)
        );
        drop(held);
        fx.clock.advance_ms(60_000);
        job.run(&mut cx);
        assert_eq!(events.modes(&cc())[1..], [(60, Mode::Auto, None, None)]);
        assert!(fx.engine.auto(&cc(), cfg(), false).unwrap().is_none());
    }

    #[test]
    fn a_lock_held_without_a_record_is_waited_for_with_no_pid() {
        // A process that has taken the lock but not yet written its record (§11.1): the try
        // fails, so the job waits, and has no pid to name.
        let fx = Fx::new();
        read_two(&fx, 50.0);
        let path = fx.env.data_dir().join("locks/autoswitch-claude-code.lock");
        let guard = FlockGuard::try_lock(&path).unwrap().unwrap();
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        assert_eq!(job.run(&mut cx), Ran::After(60_000));
        assert_eq!(
            events.modes(&cc()),
            [(0, Mode::Waiting, Some(WaitingFor::EngineLock), None)]
        );
        drop(guard);
        fx.clock.advance_ms(60_000);
        job.run(&mut cx);
        assert_eq!(events.modes(&cc())[1..], [(60, Mode::Auto, None, None)]);
    }

    #[test]
    fn a_store_schema_newer_than_this_build_keeps_the_provider_waiting() {
        // D4.1 step 4: every provider job waits for the upgrade watch, and tries no lock.
        let fx = Fx::new();
        read_two(&fx, 50.0);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let ctx = ProviderJobCtx {
            schema_newer: true,
            ..job_ctx(&board)
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), ctx);
        assert_eq!(job.run(&mut cx), Ran::After(60_000));
        assert_eq!(
            events.modes(&cc()),
            [(0, Mode::Waiting, Some(WaitingFor::Schema), None)]
        );
        assert!(!fx.env.data_dir().join("locks/autoswitch-claude-code.lock").exists());
    }

    #[test]
    fn pausing_releases_the_engine_lock_within_one_slice_and_empties_its_record() {
        // D4.2, D4.3: `daemon.autoswitch` set false while the scheduler sleeps. The reload in
        // the next slice moves the provider to `collect`, which drops the engine: by the slice
        // after the write, the record is empty and a `tagteam auto` could take the lock.
        let fx = Fx::new();
        read_two(&fx, 50.0);
        let board = board();
        let freed = Cell::new(None);
        let cancel = fx.engine.cancel().clone();
        let slices = Slices::new(&fx.clock, |n| match n {
            2 => fx.settings("[daemon]\nautoswitch = false\n", 2),
            3 => {
                let empty = matches!(fx.engine.engine_holder(&cc()), Read::Absent);
                let free = fx.engine.auto(&cc(), cfg(), false).unwrap().is_some();
                freed.set(Some((empty, free)));
            }
            _ if n >= 4 => cancel.request(libc::SIGTERM),
            _ => {}
        });
        let mut scheduler = Scheduler::new();
        scheduler.add(
            Box::new(ProviderJob::new(&fx.engine, cc(), job_ctx(&board))),
            fx.engine.now_ms(),
        );
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        assert_eq!(scheduler.run(&mut cx, &slices), StopReason::Signal);
        assert_eq!(freed.get(), Some((true, true)));
        assert_eq!(
            events.modes(&cc()),
            [(0, Mode::Auto, None, None), (3, Mode::Collect, None, None)]
        );
    }

    #[test]
    fn a_corrupt_settings_file_resumes_switching_with_a_warning() {
        // Review Focus 1. Paused, the provider collects. A `config.toml` that is no longer
        // TOML reads as the defaults (§6.4), so `daemon.autoswitch` is true again and the job
        // goes back to `auto`. The person is told why, before the mode changes.
        let fx = Fx::new();
        read_two(&fx, 50.0);
        fx.settings("[daemon]\nautoswitch = false\n", 1);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        job.run(&mut cx);
        assert!(!status(&board, &cc()).autoswitch);
        fx.settings("[daemon\nautoswitch = false\n", 2);
        job.settings_changed(&mut cx);
        let warning = format!(
            "{}: the settings file is not valid TOML; using the defaults",
            config_path(&fx.env).display()
        );
        let seen: Vec<DaemonEvent> = events.events().into_iter().map(|(_, e)| e).collect();
        assert_eq!(
            seen,
            [
                mode_event(cc(), Mode::Collect),
                DaemonEvent::Auto(AutoEvent::ConfigWarning {
                    provider: cc(),
                    message: warning,
                }),
                mode_event(cc(), Mode::Auto),
            ]
        );
        assert!(status(&board, &cc()).autoswitch);
        assert!(
            fx.engine.auto(&cc(), cfg(), false).unwrap().is_none(),
            "it holds the engine lock again"
        );
    }

    #[test]
    fn a_settings_file_corrupt_when_the_job_is_built_is_warned_at_its_first_run() {
        // Decision 8. The job's first read comes after the command printed its own, and the
        // file may have been corrupted in between: no mtime change will come to report it. Its
        // warning is the first run's first event, before the mode (`auto`: the defaults switch
        // again), and is never repeated.
        let fx = Fx::new();
        read_two(&fx, 50.0);
        fx.settings("[daemon\nautoswitch = false\n", 1);
        let warning = DaemonEvent::Auto(AutoEvent::ConfigWarning {
            provider: cc(),
            message: format!(
                "{}: the settings file is not valid TOML; using the defaults",
                config_path(&fx.env).display()
            ),
        });
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        assert!(events.events().is_empty(), "building the job emits nothing");
        job.run(&mut cx);
        job.run(&mut cx);
        let seen: Vec<DaemonEvent> = events.events().into_iter().map(|(_, e)| e).collect();
        assert_eq!(seen[..2], [warning.clone(), mode_event(cc(), Mode::Auto)], "{seen:?}");
        assert_eq!(seen.iter().filter(|e| **e == warning).count(), 1, "{seen:?}");

        // A reload before the first run reads the file again and reports what it finds then.
        // The first read's warning no longer describes the file, so it is never emitted.
        let fx = Fx::new();
        read_two(&fx, 50.0);
        fx.settings("[daemon\nautoswitch = false\n", 1);
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        fx.settings("[daemon]\nautoswitch = false\n", 2);
        job.settings_changed(&mut cx);
        job.run(&mut cx);
        assert_eq!(events.modes(&cc()), [(0, Mode::Collect, None, None)]);
        assert!(
            !events
                .events()
                .iter()
                .any(|(_, e)| matches!(e, DaemonEvent::Auto(AutoEvent::ConfigWarning { .. }))),
            "{:?}",
            events.events()
        );
    }

    #[test]
    fn a_provider_table_pauses_only_its_provider() {
        // Review Focus 5: `provider.fake-agent.daemon.autoswitch false` pauses FakeAgent
        // alone. Claude Code keeps its engine lock and ticks on.
        let fx = Fx::new();
        let (a, b) = two(&fx);
        let (alice, bob) = (fx.add_fake("alice"), fx.add_fake("bob"));
        for (id, long) in [(&a, 10.0), (&b, 50.0), (&alice, 10.0), (&bob, 50.0)] {
            fx.read(id, 10.0, long, T0, T0 + 300);
        }
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut claude = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        let mut agent = ProviderJob::new(&fx.engine, fake(), job_ctx(&board));
        claude.run(&mut cx);
        agent.run(&mut cx);
        fx.settings("[provider.fake-agent.daemon]\nautoswitch = false\n", 1);
        claude.settings_changed(&mut cx);
        agent.settings_changed(&mut cx);
        assert_eq!(events.modes(&cc()), [(0, Mode::Auto, None, None)]);
        assert_eq!(
            events.modes(&fake()),
            [(0, Mode::Auto, None, None), (0, Mode::Collect, None, None)]
        );
        let fake_cfg = auto_config(&Settings::default(), &AutoFlags::default(), None);
        assert!(fx.engine.auto(&fake(), fake_cfg, false).unwrap().is_some());
        assert!(fx.engine.auto(&cc(), cfg(), false).unwrap().is_none());
        let shown: Vec<(ProviderId, Mode, bool)> = board
            .snapshot()
            .providers
            .into_iter()
            .map(|p| (p.provider, p.mode, p.autoswitch))
            .collect();
        assert_eq!(
            shown,
            [(cc(), Mode::Auto, true), (fake(), Mode::Collect, false)]
        );
        fx.clock.advance_ms(60_000);
        claude.run(&mut cx);
        agent.run(&mut cx);
        assert_eq!(events.polls(&cc()), [0, 60]);
        assert_eq!(events.polls(&fake()), [0]);
        assert_eq!(
            status(&board, &fake()).last_outcome.map(|o| o.kind),
            Some(OutcomeKind::Collect)
        );
    }

    #[test]
    fn an_unmanaged_live_login_in_auto_still_has_its_candidates_collected() {
        // D4.3, D1.1 criterion 1: the tick stops at §11.2 step 2, since tagteam does not
        // manage the live login, so it collects nothing itself. The job runs `collect`'s
        // round after it: with no managed live account the round escalates (§8.6) and
        // fetches every due candidate.
        let fx = Fx::new();
        two(&fx);
        fx.login("c@x.co");
        fx.http.push_json(Method::Get, CC_USAGE, 200, usage_body());
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        job.run(&mut cx);
        let seen: Vec<DaemonEvent> = events.events().into_iter().map(|(_, e)| e).collect();
        assert_eq!(
            seen,
            [
                mode_event(cc(), Mode::Auto),
                DaemonEvent::Auto(AutoEvent::NoSwitch {
                    provider: cc(),
                    reason: "unmanaged-active-account".into(),
                    detail: String::new(),
                }),
                DaemonEvent::Usage {
                    provider: cc(),
                    updated: vec![1, 2],
                    fetch_errors: BTreeMap::new(),
                },
            ]
        );
        assert_eq!(fx.http.count(Method::Get, CC_USAGE), 2);
        assert_eq!(
            status(&board, &cc()).last_outcome,
            Some(LastOutcome {
                kind: OutcomeKind::NoSwitch,
                reason: Some("unmanaged-active-account".into()),
                at: T0,
            })
        );
    }

    #[test]
    fn collect_reports_a_quarantine_another_process_set_and_never_tries_the_lock() {
        // D4.3: each round compares the provider's accounts with the round before, as a
        // tick's step 1 does, and reports what another process quarantined or released.
        // Paused, the job never tries the engine lock: its file is never even created.
        let fx = Fx::new();
        let (a, _) = read_two(&fx, 50.0);
        fx.settings("[daemon]\nautoswitch = false\n", 1);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        job.run(&mut cx);
        let store = fx.engine.store().unwrap();
        store.set_quarantine(&a, "invalid_grant", "sha256:x", 1).unwrap();
        fx.clock.advance_ms(60_000);
        job.run(&mut cx);
        store
            .clear_quarantine(&a, "credentials-replaced", "cli", (T0 + 60) * 1000)
            .unwrap();
        fx.clock.advance_ms(60_000);
        job.run(&mut cx);
        let reported: Vec<(i64, AutoEvent)> = events
            .events()
            .into_iter()
            .filter_map(|(at, e)| match e {
                DaemonEvent::Auto(e) => Some((at, e)),
                _ => None,
            })
            .collect();
        assert_eq!(
            reported,
            [
                (
                    60,
                    AutoEvent::AccountQuarantined {
                        provider: cc(),
                        number: 1,
                        email: "a@x.co".into(),
                        reason: "invalid_grant".into(),
                    }
                ),
                (
                    120,
                    AutoEvent::AccountUnquarantined {
                        provider: cc(),
                        number: 1,
                        email: "a@x.co".into(),
                        reason: "credentials-replaced".into(),
                    }
                ),
            ]
        );
        assert_eq!(events.modes(&cc()), [(0, Mode::Collect, None, None)]);
        assert!(!fx.env.data_dir().join("locks/autoswitch-claude-code.lock").exists());
    }

    #[test]
    fn the_status_board_shows_a_tick_that_stayed_and_when_the_next_is_due() {
        // D5.4: `no-switch` with its reason; the next tick on §11.4's delay (60 s here).
        let fx = Fx::new();
        read_two(&fx, 50.0);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        assert_eq!(job.run(&mut cx), Ran::After(60_000));
        assert_eq!(
            status(&board, &cc()),
            ProviderStatus {
                provider: cc(),
                mode: Mode::Auto,
                autoswitch: true,
                waiting_for: None,
                holder_pid: None,
                backoff: None,
                last_tick_at: Some(T0),
                next_tick_at: Some(T0 + 60),
                last_outcome: Some(LastOutcome {
                    kind: OutcomeKind::NoSwitch,
                    reason: Some("below-threshold".into()),
                    at: T0,
                }),
                last_switch: None,
            }
        );
    }

    #[test]
    fn the_status_board_shows_a_switch_and_keeps_it() {
        // D5.4: `switch`, and the switch itself: positions and trigger, as its event said.
        let fx = Fx::new();
        read_two(&fx, 95.0);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        job.run(&mut cx);
        let trigger = events
            .events()
            .into_iter()
            .find_map(|(_, e)| match e {
                DaemonEvent::Auto(AutoEvent::Switch {
                    from: 2,
                    to: 1,
                    trigger,
                    dry_run: false,
                    ..
                }) => Some(trigger),
                _ => None,
            })
            .expect("the tick switched from 2 to 1");
        let s = status(&board, &cc());
        assert_eq!(
            s.last_outcome,
            Some(LastOutcome {
                kind: OutcomeKind::Switch,
                reason: None,
                at: T0,
            })
        );
        assert_eq!(
            s.last_switch,
            Some(LastSwitch {
                at: T0,
                from: 2,
                to: 1,
                trigger: trigger.as_str().to_owned(),
            })
        );
    }
}
```

- [ ] **Step 7: Run them and see them fail**

Run: `cargo test -p tagteam-daemon provider_job::`
Expected: FAIL to compile: `cannot find struct ProviderJob`, `cannot find function mode_for`,
`cannot find struct ModeInputs`, `cannot find function collect_delay_s`, and `testutil.rs`'s
`unresolved import crate::provider_job::ProviderJobCtx`.

- [ ] **Step 8: Implement the provider job**

In `crates/tagteam-daemon/src/auto.rs`, make Task 7's `no_switch` crate-visible, so a tick that
failed with a store error keeps its cadence by the same decision in both loops (§11.4). Replace

```rust
fn no_switch(reason: NoSwitchReason) -> Decision {
```

with

```rust
pub(crate) fn no_switch(reason: NoSwitchReason) -> Decision {
```

Put this above the test module in `crates/tagteam-daemon/src/provider_job.rs`:

```rust
//! D4.3: one job per registered provider. Each run evaluates the provider's mode from D4.3's
//! table (`mode_for`), then does that mode's work when its deadline has come: an `auto` tick,
//! exactly as `tagteam auto` ticks, or a `collect` round through the collector (§8.3, §8.6).
//! `idle` and `waiting` do nothing. The mode is evaluated at least every 60 s, after each
//! settings reload (`settings_changed`), and whenever the store watch wakes the job (D4.4). A
//! mode change is a `mode` event and an INFO line. After each run the job writes its line of
//! the status board (Decision 12, D5.4).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use tagteam_core::autoswitch::{
    AutoConfig, Decision, NoSwitchReason, Trigger, announces_sleep, next_delay,
};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::auto::{AutoEngine, AutoEvent, EventSink, QuarantineWatch, TickOutcome};
use tagteam_engine::collect::{CollectMode, Collected};
use tagteam_engine::settings::Settings;
use tagteam_engine::store::AccountRow;
use tagteam_engine::{Engine, EngineError};
use tagteam_provider::Read;
use tagteam_provider::keychain::ReadCounts;
use tagteam_provider::process::Liveness;

use crate::auto::{AutoFlags, auto_config, no_switch};
use crate::events::{AutoEvents, DaemonEvent, DaemonSink, Mode, StopReason, WaitingFor};
use crate::scheduler::{Job, JobCx, Ran};
use crate::status::{LastOutcome, LastSwitch, OutcomeKind, ProviderStatus, StatusBoard};

/// Every provider job's name starts with this; D5.4's `jobs` leaves them out, and the store
/// watch wakes them by it (D4.4).
pub const JOB_PREFIX: &str = "provider:";
/// D4.3: a provider's mode is evaluated again at least this often.
const RECHECK_MS: i64 = 60_000;
/// D4.3: a `collect` round's delay is never shorter.
const COLLECT_FLOOR_S: i64 = 60;
/// §11.4's jitter: a delay of `interval × U(0.9, 1.1)`.
const JITTER_FRAC: f64 = 0.1;

/// What a provider job is given besides its engine and its provider.
pub struct ProviderJobCtx {
    /// Where the job writes its line after each run (Decision 12).
    pub board: Arc<StatusBoard>,
    /// The Keychain read counts the keychain backoff compares around each run (D4.1,
    /// Decision 7); empty where there is no Keychain.
    pub keychain_counts: Vec<Arc<ReadCounts>>,
    /// D4.1 step 4: the store's schema was newer than this build at start-up.
    pub schema_newer: bool,
}

/// What D4.3's table decides from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModeInputs {
    /// The store's schema is newer than this build's (D4.1 step 4).
    pub schema_newer: bool,
    pub has_store: bool,
    /// The provider's switchable accounts: enabled, with an identity, as `auto` counts them
    /// (§11.1). A quarantine does not count against an account here.
    pub switchable: usize,
    /// `daemon.autoswitch` for the provider (D7).
    pub autoswitch: bool,
    /// The pid the engine lock's record names, when that process is live (§13.6).
    pub holder_elsewhere: Option<u32>,
    /// This job tried the engine lock just now, and another process held it.
    pub refused: bool,
    /// This job holds the provider's engine lock.
    pub holds_lock: bool,
}

/// D4.3's table, its rows tried in order and the first that applies winning. `holds_lock` is
/// taken as given: entering `auto` is the job's own try of the lock, which a refusal reports
/// back through `refused`.
pub fn mode_for(i: &ModeInputs) -> (Mode, Option<WaitingFor>) {
    if i.schema_newer {
        return (Mode::Waiting, Some(WaitingFor::Schema));
    }
    if !i.has_store || i.switchable == 0 {
        return (Mode::Idle, None);
    }
    if i.refused || i.holder_elsewhere.is_some() {
        return (Mode::Waiting, Some(WaitingFor::EngineLock));
    }
    if i.autoswitch && i.switchable >= 2 && i.holds_lock {
        return (Mode::Auto, None);
    }
    (Mode::Collect, None)
}

/// D4.3: a `collect` round's delay in seconds: `interval × U(0.9, 1.1)` (`jitter` in
/// [-1, 1)), shortened to the earliest planned poll, and never under 60 s.
fn collect_delay_s(interval_s: i64, now: i64, earliest_next_poll_at: Option<i64>, jitter: f64) -> i64 {
    let j = if jitter.is_finite() {
        jitter.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let mut delay = (interval_s as f64 * (1.0 + j * JITTER_FRAC)).round() as i64;
    if let Some(at) = earliest_next_poll_at {
        delay = delay.min(at.saturating_sub(now));
    }
    delay.max(COLLECT_FLOOR_S)
}

/// What a `mode` event last stated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shown {
    mode: Mode,
    waiting_for: Option<WaitingFor>,
    holder_pid: Option<u32>,
}

/// A run's work, done: its own next delay, and whether it made a switch.
struct Worked {
    delay_ms: i64,
    switched: bool,
}

/// A tick's events, passed on to the daemon's sink as they come (`AutoEvents`). The switch the
/// tick made and the error it reported are kept for the status board.
struct Watching<'a> {
    inner: AutoEvents<'a>,
    switched: RefCell<Option<(u32, u32, Trigger)>>,
    error: RefCell<Option<String>>,
}

impl<'a> Watching<'a> {
    fn new(sink: &'a dyn DaemonSink) -> Self {
        Watching {
            inner: AutoEvents(sink),
            switched: RefCell::new(None),
            error: RefCell::new(None),
        }
    }
}

impl EventSink for Watching<'_> {
    fn emit(&self, e: &AutoEvent) {
        match e {
            AutoEvent::Switch {
                from,
                to,
                trigger,
                dry_run: false,
                ..
            } => *self.switched.borrow_mut() = Some((*from, *to, *trigger)),
            AutoEvent::Error { message, .. } => *self.error.borrow_mut() = Some(message.clone()),
            _ => {}
        }
        self.inner.emit(e);
    }

    fn tick_done(&self, provider: &ProviderId) {
        self.inner.tick_done(provider);
    }
}

/// D4.3: one provider's job.
pub struct ProviderJob<'e> {
    engine: &'e Engine,
    provider: ProviderId,
    ctx: ProviderJobCtx,
    /// The provider's long window (§4.5), which its `AutoConfig` carries.
    long_window: Option<String>,
    /// The provider's settings as last read (Decision 8).
    settings: Settings,
    /// The warnings of the read `new` made, emitted as `config-warning` events at the start of
    /// the first run; emptied then, or by a reload before it, which reports its own read.
    first_warnings: Vec<String>,
    cfg: AutoConfig,
    /// The auto-switch engine while in `auto`. It holds the engine lock (§11.1).
    auto: Option<AutoEngine<'e>>,
    /// `collect`'s quarantine report (Decision 6), started afresh on each entry into
    /// `collect`: a tick reported everything before it.
    quarantines: QuarantineWatch,
    /// `None` before the first evaluation.
    mode: Option<Mode>,
    waiting_for: Option<WaitingFor>,
    holder_pid: Option<u32>,
    shown: Option<Shown>,
    /// The work's wall-clock deadline (the next tick or round), epoch ms, and the delay it
    /// was set for, which the wait never exceeds (D4.2).
    work_due_ms: i64,
    work_delay_ms: i64,
    last_tick_at: Option<i64>,
    last_outcome: Option<LastOutcome>,
    last_switch: Option<LastSwitch>,
}

impl<'e> ProviderJob<'e> {
    /// `provider`'s job, with its settings as `config.toml` holds them now. The read's warnings
    /// are the job's to report, as `config-warning` events at the start of its first run: the
    /// command reported its own read's, but the file may have changed since (during an unlock
    /// offer, say), and no mtime change would then come to report it (Decision 8). It emits
    /// nothing and takes no lock before its first run.
    pub fn new(engine: &'e Engine, provider: ProviderId, ctx: ProviderJobCtx) -> Self {
        let long_window = engine
            .provider(&provider)
            .ok()
            .and_then(|p| p.primary_long_window().map(str::to_owned));
        let (settings, first_warnings) = Settings::load(engine.env(), &provider);
        let cfg = auto_config(&settings, &AutoFlags::default(), long_window.as_deref());
        ProviderJob {
            engine,
            provider,
            ctx,
            long_window,
            settings,
            first_warnings,
            cfg,
            auto: None,
            quarantines: QuarantineWatch::new(),
            mode: None,
            waiting_for: None,
            holder_pid: None,
            shown: None,
            work_due_ms: 0,
            work_delay_ms: 0,
            last_tick_at: None,
            last_outcome: None,
            last_switch: None,
        }
    }

    fn now_s(&self) -> i64 {
        self.engine.now_ms().div_euclid(1000)
    }

    /// The pid the engine lock's record names, when that process is live by an exact pid and
    /// start-time match, judged as `doctor` judges it (§13.6): a start time that cannot be
    /// read counts as live. The lock itself is never tried here, since a starting `auto`
    /// would then find it taken (D4.3).
    fn live_holder(&self) -> Option<u32> {
        match self.engine.engine_holder(&self.provider) {
            Read::Present(stamp) => match stamp.liveness() {
                Liveness::Live | Liveness::Unknown(_) => Some(stamp.pid),
                Liveness::Dead => None,
            },
            Read::Absent | Read::Unreadable(_) => None,
        }
    }

    /// D4.3's inputs as they stand. A store whose schema is newer is never opened.
    fn inputs(&self) -> Result<ModeInputs, EngineError> {
        if self.ctx.schema_newer {
            return Ok(ModeInputs {
                schema_newer: true,
                ..ModeInputs::default()
            });
        }
        let store = self.engine.existing_store()?;
        let switchable = match &store {
            Some(s) => s
                .accounts(&self.provider)?
                .iter()
                .filter(|r| !r.disabled && r.identity_json.is_object())
                .count(),
            None => 0,
        };
        let holds_lock = self.auto.is_some();
        Ok(ModeInputs {
            schema_newer: false,
            has_store: store.is_some(),
            switchable,
            autoswitch: self.settings.daemon_autoswitch,
            // Holding the lock, the record is this job's own.
            holder_elsewhere: if holds_lock { None } else { self.live_holder() },
            refused: false,
            holds_lock,
        })
    }

    /// D4.3's table, applied. Leaving `auto` drops the engine, which empties its lock record
    /// and then frees the lock (D3 change 3). A provider that would be in `auto` but for the
    /// lock tries it once; a refusal is `waiting`, naming the record's live holder if any.
    fn evaluate(&mut self, cx: &JobCx<'_>) -> Result<(), EngineError> {
        let mut inputs = self.inputs()?;
        let (mut mode, mut waiting_for) = mode_for(&inputs);
        if mode != Mode::Auto {
            self.auto = None;
        }
        let with_lock = mode_for(&ModeInputs {
            holds_lock: true,
            ..inputs
        });
        if mode != Mode::Auto && with_lock.0 == Mode::Auto {
            match self.engine.auto(&self.provider, self.cfg.clone(), false)? {
                Some(auto) => {
                    self.auto = Some(auto.with_source("daemon"));
                    inputs.holds_lock = true;
                }
                None => {
                    inputs.refused = true;
                    inputs.holder_elsewhere = self.live_holder();
                }
            }
            (mode, waiting_for) = mode_for(&inputs);
        }
        let holder_pid = match waiting_for {
            Some(WaitingFor::EngineLock) => inputs.holder_elsewhere,
            _ => None,
        };
        self.set_mode(mode, waiting_for, holder_pid, cx);
        Ok(())
    }

    fn set_mode(
        &mut self,
        mode: Mode,
        waiting_for: Option<WaitingFor>,
        holder_pid: Option<u32>,
        cx: &JobCx<'_>,
    ) {
        if self.mode != Some(mode) {
            if mode == Mode::Collect {
                self.quarantines = QuarantineWatch::new();
            }
            // A mode that works starts with its work.
            self.work_due_ms = self.engine.now_ms();
            self.work_delay_ms = 0;
        }
        self.mode = Some(mode);
        self.waiting_for = waiting_for;
        self.holder_pid = holder_pid;
        self.announce(cx);
    }

    /// D4.3: a mode change is a `mode` event (D5.5) and an INFO line (D11).
    fn announce(&mut self, cx: &JobCx<'_>) {
        let Some(mode) = self.mode else {
            return;
        };
        let shown = Shown {
            mode,
            waiting_for: self.waiting_for,
            holder_pid: self.holder_pid,
        };
        if self.shown == Some(shown) {
            return;
        }
        self.shown = Some(shown);
        tracing::info!(
            provider = %self.provider,
            mode = mode.as_str(),
            waiting_for = self.waiting_for.map(|w| w.as_str()),
            holder_pid = self.holder_pid,
            "the provider's daemon mode changed"
        );
        cx.sink.emit(&DaemonEvent::Mode {
            provider: self.provider.clone(),
            mode,
            waiting_for: self.waiting_for,
            holder_pid: self.holder_pid,
            backoff: None,
        });
    }

    /// D4.3: a run that fails is an `error` event, and the job keeps its cadence.
    fn failed(&mut self, cx: &JobCx<'_>, e: &EngineError) {
        let message = e.to_string();
        cx.sink.emit(&DaemonEvent::Auto(AutoEvent::Error {
            provider: self.provider.clone(),
            message: message.clone(),
            transient: e.kind() == "lock-timeout",
        }));
        self.last_outcome = Some(LastOutcome {
            kind: OutcomeKind::Error,
            reason: Some(message),
            at: self.now_s(),
        });
    }

    /// One run: the mode first, then its work when the work's deadline has come.
    fn step(&mut self, cx: &mut JobCx<'_>) -> Ran {
        let now = self.engine.now_ms();
        // D4.2: a wall clock set back never stretches the wait past the delay it was set for.
        self.work_due_ms = self.work_due_ms.min(now.saturating_add(self.work_delay_ms));
        if let Err(e) = self.evaluate(cx) {
            if e.signal().is_some() {
                return Ran::Stop(StopReason::Signal);
            }
            self.failed(cx, &e);
            return Ran::After(RECHECK_MS);
        }
        if !matches!(self.mode, Some(Mode::Auto | Mode::Collect)) {
            return Ran::After(RECHECK_MS);
        }
        let now = self.engine.now_ms();
        if now < self.work_due_ms {
            // D4.3: evaluated again at least every 60 s, while the work waits its deadline.
            return Ran::After((self.work_due_ms - now).clamp(1, RECHECK_MS));
        }
        self.last_tick_at = Some(now.div_euclid(1000));
        let worked = if self.mode == Some(Mode::Auto) {
            self.tick(cx)
        } else {
            self.collect(cx)
        };
        let Some(worked) = worked else {
            return Ran::Stop(StopReason::Signal);
        };
        self.after_run_notices(cx, worked.switched);
        let now = self.engine.now_ms();
        self.work_delay_ms = worked.delay_ms;
        self.work_due_ms = now + worked.delay_ms;
        Ran::After(worked.delay_ms.clamp(1, RECHECK_MS))
    }

    /// D4.3 `auto`: one §11.2 tick, settled as `tagteam auto` settles one (§11.4), its delay
    /// from `next_delay` with a `sleep` event when it is long. A tick that ended before its
    /// collection because the live login is unmanaged or absent is followed, in the same run,
    /// by `collect`'s round. `None`: a signal interrupted it (§14.1).
    fn tick(&mut self, cx: &mut JobCx<'_>) -> Option<Worked> {
        let watching = Watching::new(cx.sink);
        let auto = self.auto.as_mut().expect("a job in auto holds its engine");
        let ticked = auto.tick(&watching);
        let at = self.now_s();
        let (outcome, decision) = match ticked {
            Ok(done) => done,
            Err(e) if e.signal().is_some() => return None,
            Err(e) => {
                // A store that failed: the engine reports every other failure itself (§11.4).
                watching.emit(&AutoEvent::Error {
                    provider: self.provider.clone(),
                    message: e.to_string(),
                    transient: false,
                });
                (TickOutcome::Error, no_switch(NoSwitchReason::NoActiveAccount))
            }
        };
        if let Some((from, to, trigger)) = watching.switched.take() {
            self.last_switch = Some(LastSwitch {
                at,
                from,
                to,
                trigger: trigger.as_str().to_owned(),
            });
        }
        let (kind, reason) = match (outcome, &decision) {
            (TickOutcome::Error, _) => (OutcomeKind::Error, watching.error.take()),
            (_, Decision::NoSwitch { reason, .. }) => {
                (OutcomeKind::NoSwitch, Some(reason.as_str().to_owned()))
            }
            (_, Decision::Switch { .. }) => (OutcomeKind::Switch, None),
        };
        self.last_outcome = Some(LastOutcome { kind, reason, at });
        let unmanaged = outcome != TickOutcome::Error
            && matches!(
                &decision,
                Decision::NoSwitch {
                    reason: NoSwitchReason::UnmanagedActiveAccount | NoSwitchReason::NoActiveAccount,
                    ..
                }
            );
        if unmanaged {
            match self.collect_round(cx, false) {
                Ok(()) => {}
                Err(e) if e.signal().is_some() => return None,
                Err(e) => self.failed(cx, &e),
            }
        }
        let now = self.now_s();
        let next_poll = self.auto.as_ref().and_then(|a| a.active_next_poll_at());
        let delay = next_delay(&decision, &self.cfg, now, next_poll, (cx.jitter)());
        if announces_sleep(delay, &self.cfg) {
            cx.sink.emit(&DaemonEvent::Auto(AutoEvent::Sleep {
                provider: self.provider.clone(),
                seconds: delay as f64,
                until: now + delay,
            }));
        }
        Some(Worked {
            delay_ms: delay * 1000,
            switched: outcome == TickOutcome::Switched,
        })
    }

    /// D4.3 `collect`: one round, then `interval × U(0.9, 1.1)` shortened to the earliest
    /// planned poll and floored at 60 s. `None`: a signal interrupted it (§14.1).
    fn collect(&mut self, cx: &mut JobCx<'_>) -> Option<Worked> {
        match self.collect_round(cx, true) {
            Ok(()) => {
                self.last_outcome = Some(LastOutcome {
                    kind: OutcomeKind::Collect,
                    reason: None,
                    at: self.now_s(),
                });
            }
            Err(e) if e.signal().is_some() => return None,
            Err(e) => self.failed(cx, &e),
        }
        // A store that cannot be read plans nothing, and the floor applies.
        let earliest = self.earliest_next_poll().unwrap_or(None);
        let delay = collect_delay_s(self.cfg.interval_s, self.now_s(), earliest, (cx.jitter)());
        Some(Worked {
            delay_ms: delay * 1000,
            switched: false,
        })
    }

    /// The provider's accounts as the store holds them now; none without a store.
    fn rows(&self) -> Result<Vec<AccountRow>, EngineError> {
        Ok(match self.engine.existing_store()? {
            Some(s) => s.accounts(&self.provider)?,
            None => Vec::new(),
        })
    }

    /// `collect`'s round: `collect_usage` as an auto tick schedules it (§8.6), with the
    /// provider's threshold and models. Its warnings are `error` events that do not fail the
    /// run (§8.3), and a round that recorded or failed something is a `usage` event (D5.5).
    /// `report` is false after an `auto` tick: the next tick's step 1 reports what this round
    /// quarantined, so nothing is reported twice (Decision 6). Releasing is never this
    /// round's (Decision 10).
    fn collect_round(&mut self, cx: &JobCx<'_>, report: bool) -> Result<(), EngineError> {
        let sink = AutoEvents(cx.sink);
        if report {
            let rows = self.rows()?;
            self.quarantines.report(&self.provider, &rows, &[], &sink);
        }
        let collected = self.engine.collect_usage(CollectMode::Scheduled {
            provider: self.provider.clone(),
            threshold: self.cfg.threshold,
            models: self.cfg.models.clone(),
        })?;
        let rows = self.rows()?;
        if report {
            for id in &collected.quarantined {
                if let Some(row) = rows.iter().find(|r| &r.id == id) {
                    self.quarantines.note_quarantined(&self.provider, row, &sink);
                }
            }
        }
        for warning in &collected.warnings {
            sink.emit(&AutoEvent::Error {
                provider: self.provider.clone(),
                message: warning.clone(),
                transient: true,
            });
        }
        let position = |id: &AccountId| rows.iter().find(|r| &r.id == id).map(|r| r.position);
        let mut updated = Vec::new();
        let mut fetch_errors = BTreeMap::new();
        for (id, outcome) in &collected.outcomes {
            let Some(at) = position(id) else {
                continue;
            };
            match outcome {
                Collected::Recorded => updated.push(at),
                Collected::Failed { kind } => {
                    fetch_errors.insert(at, kind.clone());
                }
                Collected::OverBudget { .. } => {
                    fetch_errors.insert(at, "over-budget".to_owned());
                }
                _ => {}
            }
        }
        updated.sort_unstable();
        updated.dedup();
        if !updated.is_empty() || !fetch_errors.is_empty() {
            cx.sink.emit(&DaemonEvent::Usage {
                provider: self.provider.clone(),
                updated,
                fetch_errors,
            });
        }
        Ok(())
    }

    /// The earliest `next_poll_at` among the accounts a round can collect: the switchable
    /// ones that are not quarantined (§8.6).
    fn earliest_next_poll(&self) -> Result<Option<i64>, EngineError> {
        let Some(store) = self.engine.existing_store()? else {
            return Ok(None);
        };
        let mut earliest: Option<i64> = None;
        for row in store.accounts(&self.provider)? {
            if row.disabled || !row.identity_json.is_object() || row.quarantine_reason.is_some() {
                continue;
            }
            if let Some(at) = store.usage_state(&row.id)?.and_then(|s| s.next_poll_at) {
                earliest = Some(earliest.map_or(at, |e| e.min(at)));
            }
        }
        Ok(earliest)
    }

    /// D6.1's `limit` notice is judged here, from the store, after each run that did work: in
    /// `auto` after a tick that did not switch, in `collect` after every round. Notifications
    /// are not wired yet, so it does nothing.
    fn after_run_notices(&mut self, _cx: &mut JobCx<'_>, _switched: bool) {}

    /// Decision 12: this job's line of the status board.
    fn publish(&self) {
        let Some(mode) = self.mode else {
            return;
        };
        let works = matches!(mode, Mode::Auto | Mode::Collect);
        self.ctx.board.set_provider(ProviderStatus {
            provider: self.provider.clone(),
            mode,
            autoswitch: self.settings.daemon_autoswitch,
            waiting_for: self.waiting_for,
            holder_pid: self.holder_pid,
            backoff: None,
            last_tick_at: self.last_tick_at,
            next_tick_at: works.then(|| self.work_due_ms.div_euclid(1000)),
            last_outcome: self.last_outcome.clone(),
            last_switch: self.last_switch.clone(),
        });
    }
}

impl Job for ProviderJob<'_> {
    fn name(&self) -> String {
        format!("{JOB_PREFIX}{}", self.provider)
    }

    /// Decision 8: the first run starts with the warnings of the job's first settings read,
    /// before its mode is evaluated, as a reload's come before its evaluation.
    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
        for message in std::mem::take(&mut self.first_warnings) {
            cx.sink.emit(&DaemonEvent::Auto(AutoEvent::ConfigWarning {
                provider: self.provider.clone(),
                message,
            }));
        }
        let ran = self.step(cx);
        self.publish();
        ran
    }

    /// D4.2, Decision 8: the provider's settings read again, each warning a `config-warning`
    /// event before anything else happens, the engine's configuration replaced, and the mode
    /// evaluated at once. Pausing (`daemon.autoswitch` false) therefore drops the engine lock
    /// here, before the scheduler's next job or sleep. A file that cannot be read gives the
    /// defaults, so switching resumes, and its warning comes first (Review Focus 1). A reload
    /// before the first run supersedes the first read, whose warnings are then never emitted.
    fn settings_changed(&mut self, cx: &mut JobCx<'_>) {
        self.first_warnings.clear();
        let (settings, warnings) = Settings::load(self.engine.env(), &self.provider);
        for message in warnings {
            cx.sink.emit(&DaemonEvent::Auto(AutoEvent::ConfigWarning {
                provider: self.provider.clone(),
                message,
            }));
        }
        self.cfg = auto_config(&settings, &AutoFlags::default(), self.long_window.as_deref());
        self.settings = settings;
        if let Some(auto) = self.auto.as_mut() {
            auto.set_config(self.cfg.clone());
        }
        if let Err(e) = self.evaluate(cx) {
            // An interruption is the scheduler's, at its next cancellation point.
            if e.signal().is_none() {
                self.failed(cx, &e);
            }
        }
        self.publish();
    }
}
```

- [ ] **Step 9: Run the tests and see them pass**

Run: `cargo test -p tagteam-daemon provider_job::`, then
`cargo test -p tagteam-daemon --features test-hooks`; then `cargo fmt --all`,
`cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`
Expected: PASS (14 provider job tests, 4 status tests, and every moved loop test), and no
warning.

- [ ] **Step 10: Commit** (the controller commits; implementers do not)

`git commit -m "Run each provider as a daemon job with its modes and status"`

---

### Task 9: The keychain backoff

**Depends on:** 4, 8

D4.1 step 3: "A locked keychain while running is noticed from the driver's read counters …
compared before and after each provider run: a run in which some secret read ended *locked* and
none was *answered*. … The job then backs off: its next run is 60 s later, doubling with each
such run to at most 15 min. A run with any answered secret read ends the backoff. A run whose
secret reads were all absent, or that made none, changes nothing. Linux has no Keychain, so it
never backs off for one." D4.3 adds that the backoff "is not a mode. It only moves a job's next
run later. The mode is still evaluated as above, before the backoff applies … pausing drops the
`AutoEngine` and its lock at once, and the job's delayed runs are then `collect` rounds. The
backoff carries over a mode change."

**Readings of the spec this task commits to:**
- **"Run" is a run that did work:** a tick (with its follow-up round) or a `collect` round. Task
  8's evaluation-only runs read no secret: they evaluate the mode and may try the engine lock,
  neither of which touches the Keychain. So the backoff moves the work's deadline, and the job
  still evaluates its mode at least every 60 s during a backoff.
- **"Only moves a job's next run later":** the next work runs at `max(mode's delay, backoff's
  delay)`. A mode entered during a backoff starts its work when the backoff ends, not at once.
- **A run with no counted read** keeps the level, and the next work waits that level's delay
  again: a backoff never ends by itself, only through an answered read.
- **`mode` events:** one when a backoff starts and one when it ends, carrying the same mode.
  Doubling sends none; the status board shows each new `until`.
- **The counts** are summed over every `ReadCounts` the job was given (Decision 7), with Task 4's
  `ReadTally: Add`. Empty, as on Linux, they never move.
- **`Backoff.until` is epoch seconds** (Decision 12), rounded up from the backoff's
  milliseconds.

**Files:**
- Create: `crates/tagteam-daemon/src/backoff.rs`
- Modify: `crates/tagteam-daemon/src/lib.rs` (`pub mod backoff;`)
- Modify: `crates/tagteam-daemon/src/provider_job.rs` (the job keeps a `KeychainBackoff`; `step`,
  `set_mode`, `announce` and `publish` use it)
- Test: `crates/tagteam-daemon/src/backoff.rs`, `crates/tagteam-daemon/src/provider_job.rs`
  (a new `mod backoff_tests`)

**Interfaces:**
- Consumes: Task 4's `ReadCounts::tally`, `ReadTally { answered, locked }`, `ReadTally::since`,
  `ReadTally::looks_locked`, `impl Add for ReadTally`, `Keychain::read_counts` (on
  `FakeKeychain`); Task 6's `Backoff`; Task 8's `ProviderJob`, `ProviderJobCtx`, and
  `testutil`'s `Fx`, `Emitted`, `board`, `job_ctx`, `status`, `two`, `T0`.
- Produces: `KeychainBackoff::{new, after_run, current}` as the skeleton gives them, plus
  `KeychainBackoff::until_ms(&self) -> Option<i64>` (crate-private) and `Default`.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-daemon/src/backoff.rs` holding only this test module, and add
`pub mod backoff;` to `crates/tagteam-daemon/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000_000;

    fn locked() -> ReadTally {
        ReadTally {
            answered: 0,
            locked: 2,
        }
    }

    #[test]
    fn a_locked_run_starts_the_backoff_at_60_s_and_each_doubles_it_to_900_s() {
        let mut b = KeychainBackoff::new();
        let mut now = NOW;
        let mut delays = Vec::new();
        for _ in 0..7 {
            let delay = b.after_run(locked(), now).expect("backing off");
            assert_eq!(b.current(), Some(Backoff { until: (now + delay) / 1000 }));
            assert_eq!(b.until_ms(), Some(now + delay));
            delays.push(delay / 1000);
            now += delay;
        }
        assert_eq!(delays, [60, 120, 240, 480, 900, 900, 900]);
    }

    #[test]
    fn an_answered_read_ends_it_and_a_run_without_counted_reads_leaves_it_as_it_was() {
        let mut b = KeychainBackoff::new();
        b.after_run(locked(), NOW);
        assert_eq!(b.after_run(locked(), NOW + 60_000), Some(120_000));
        // Nothing was due, or every item was absent: the level holds, from now.
        assert_eq!(
            b.after_run(ReadTally::default(), NOW + 180_000),
            Some(120_000)
        );
        assert_eq!(b.current(), Some(Backoff { until: (NOW + 300_000) / 1000 }));
        // One answered read ends it, however many others ended locked.
        let mixed = ReadTally {
            answered: 1,
            locked: 3,
        };
        assert_eq!(b.after_run(mixed, NOW + 300_000), None);
        assert_eq!((b.current(), b.until_ms()), (None, None));
        // The next locked run starts again from 60 s.
        assert_eq!(b.after_run(locked(), NOW + 360_000), Some(60_000));
    }

    #[test]
    fn runs_that_were_answered_or_read_nothing_never_start_one() {
        let mut b = KeychainBackoff::new();
        for reads in [
            ReadTally::default(),
            ReadTally {
                answered: 2,
                locked: 0,
            },
            ReadTally {
                answered: 1,
                locked: 1,
            },
        ] {
            assert_eq!(b.after_run(reads, NOW), None, "{reads:?}");
            assert_eq!(b.current(), None);
        }
    }
}
```

At the end of `crates/tagteam-daemon/src/provider_job.rs`, after Task 8's `mod tests`, add:

```rust
#[cfg(test)]
mod backoff_tests {
    use tagteam_engine::vault::SERVICE;
    use tagteam_provider::{Clock, Keychain};

    use super::*;
    use crate::events::Backoff;
    use crate::testutil::{Emitted, Fx, T0, board, cc, job_ctx, status, two};

    /// The job's context with the fixture keychain's read counts, as macOS gives them
    /// (Decision 7: one keychain serves the provider and the vault here).
    fn counted(fx: &Fx, board: &Arc<StatusBoard>) -> ProviderJobCtx {
        ProviderJobCtx {
            keychain_counts: vec![fx.kc.read_counts().expect("the fake keychain counts its reads")],
            ..job_ctx(board)
        }
    }

    /// Each `mode` event of `of`, with its backoff: (seconds after `T0`, mode, backoff).
    fn backoffs(events: &Emitted<'_>, of: &ProviderId) -> Vec<(i64, Mode, Option<Backoff>)> {
        events
            .events()
            .into_iter()
            .filter_map(|(at, e)| match e {
                DaemonEvent::Mode {
                    provider,
                    mode,
                    backoff,
                    ..
                } if provider == *of => Some((at, mode, backoff)),
                _ => None,
            })
            .collect()
    }

    /// The fetch error each `usage` event reported for position `at`.
    fn fetch_errors(events: &Emitted<'_>, at: u32) -> Vec<String> {
        events
            .events()
            .into_iter()
            .filter_map(|(_, e)| match e {
                DaemonEvent::Usage { fetch_errors, .. } => fetch_errors.get(&at).cloned(),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_locked_keychain_backs_the_provider_off_doubling_to_900_s_until_a_read_is_answered() {
        // D4.1: one account, never read, so each round that may fetch reads its live
        // credential. Each round whose reads all end locked doubles the wait, from 60 s to at
        // most 900 s, and the first round with an answered read ends it. The round at 60 s
        // finds the first round's usage lease still held (90 s, §8.3), so it reads nothing,
        // and a run with no read leaves the backoff as it was: 60 s again. The account's own
        // failure backoff (§8.5) always ends before the next round. The backoff's start and
        // its end are `mode` events; doubling is not.
        let fx = Fx::new();
        fx.add("a@x.co");
        fx.kc.set_locked(true);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), counted(&fx, &board));
        let mut gaps = Vec::new();
        for _ in 0..7 {
            job.run(&mut cx);
            let s = status(&board, &cc());
            let now = fx.clock.now_ms().div_euclid(1000);
            let until = s.backoff.expect("backing off").until;
            assert_eq!(s.next_tick_at, Some(until), "the next round waits for it");
            gaps.push(until - now);
            fx.clock.set(until * 1000);
        }
        assert_eq!(gaps, [60, 60, 120, 240, 480, 900, 900]);
        fx.kc.set_locked(false);
        job.run(&mut cx);
        assert_eq!(status(&board, &cc()).backoff, None);
        assert_eq!(
            backoffs(&events, &cc()),
            [
                (0, Mode::Collect, None),
                (0, Mode::Collect, Some(Backoff { until: T0 + 60 })),
                (2_760, Mode::Collect, None),
            ]
        );
    }

    #[test]
    fn an_unreadable_item_beside_answered_reads_starts_no_backoff() {
        // A read that fails for one item (not a locked keychain) is not counted, and the live
        // credential's answered read shows the keychain unlocked.
        let fx = Fx::new();
        let (a, _) = two(&fx);
        fx.settings("[daemon]\nautoswitch = false\n", 1);
        fx.kc.set_unreadable(SERVICE, a.as_str(), true);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), counted(&fx, &board));
        job.run(&mut cx);
        assert_eq!(fetch_errors(&events, 1), ["keychain-unavailable"]);
        assert_eq!(status(&board, &cc()).backoff, None);
        assert_eq!(backoffs(&events, &cc()), [(0, Mode::Collect, None)]);
    }

    #[test]
    fn a_run_that_reads_no_secret_starts_no_backoff() {
        // Locked, but nothing is due: the round reads no secret, so it changes nothing.
        let fx = Fx::new();
        let a = fx.add("a@x.co");
        fx.read(&a, 10.0, 10.0, T0, T0 + 300);
        fx.kc.set_locked(true);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let counts = fx.kc.read_counts().unwrap();
        let before = counts.tally();
        let mut job = ProviderJob::new(&fx.engine, cc(), counted(&fx, &board));
        job.run(&mut cx);
        assert_eq!(counts.tally(), before, "nothing was due, so nothing was read");
        let s = status(&board, &cc());
        assert_eq!((s.backoff, s.next_tick_at), (None, Some(T0 + 60)));
    }

    #[test]
    fn pausing_during_a_backoff_frees_the_lock_at_once_and_the_delayed_run_collects() {
        // D4.3: the mode is evaluated before the backoff applies. Pausing drops the engine
        // and its lock at once; the backoff carries over, and the run it delays is a
        // `collect` round, never a tick.
        let fx = Fx::new();
        two(&fx);
        fx.kc.set_locked(true);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), counted(&fx, &board));
        job.run(&mut cx);
        fx.settings("[daemon]\nautoswitch = false\n", 1);
        job.settings_changed(&mut cx);
        assert!(matches!(fx.engine.engine_holder(&cc()), Read::Absent));
        let free = auto_config(&Settings::default(), &AutoFlags::default(), Some("7d"));
        assert!(fx.engine.auto(&cc(), free, false).unwrap().is_some());
        let backoff = Some(Backoff { until: T0 + 60 });
        assert_eq!(
            backoffs(&events, &cc()),
            [
                (0, Mode::Auto, None),
                (0, Mode::Auto, backoff),
                (0, Mode::Collect, backoff),
            ]
        );
        // Before the backoff ends the job only looks; at its end it runs a `collect` round.
        assert_eq!(job.run(&mut cx), Ran::After(60_000));
        assert_eq!(status(&board, &cc()).last_tick_at, Some(T0));
        fx.clock.advance_ms(60_000);
        job.run(&mut cx);
        let s = status(&board, &cc());
        assert_eq!(
            (s.mode, s.last_tick_at, s.last_outcome.map(|o| o.kind)),
            (Mode::Collect, Some(T0 + 60), Some(OutcomeKind::Collect))
        );
        assert_eq!(events.polls(&cc()), [0], "no tick after the pause");
    }

    #[test]
    fn without_keychain_counts_a_provider_never_backs_off() {
        // Linux: no Keychain, so no counts, whatever the reads came to.
        let fx = Fx::new();
        fx.add("a@x.co");
        fx.kc.set_locked(true);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut job = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        job.run(&mut cx);
        assert_eq!(fetch_errors(&events, 1), ["keychain-unavailable"]);
        let s = status(&board, &cc());
        assert_eq!((s.backoff, s.next_tick_at), (None, Some(T0 + 60)));
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-daemon backoff`
Expected: FAIL to compile: `cannot find type KeychainBackoff in this scope` in `backoff.rs`.
With `backoff.rs`'s tests commented out, `provider_job::backoff_tests` compiles and
`a_locked_keychain_backs_the_provider_off_doubling_to_900_s_until_a_read_is_answered` panics
with `backing off`, and `pausing_during_a_backoff_frees_the_lock_at_once_and_the_delayed_run_collects`
fails its `backoffs` assertion (no backoff field is ever set).

- [ ] **Step 3: Implement**

Put this above the test module in `crates/tagteam-daemon/src/backoff.rs`:

```rust
//! D4.1: the keychain backoff. A provider run whose secret reads ended locked, with none
//! answered, moves the provider's next run later: 60 s, doubling with each such run to at most
//! 15 min. A run with any answered read ends it. A run that made no counted read (nothing was
//! due, or every item was absent) changes nothing. It is not a mode (D4.3), and it carries
//! over a mode change, since the keychain is no less locked.

use tagteam_provider::keychain::ReadTally;

use crate::events::Backoff;

/// The first delay.
const START_MS: i64 = 60_000;
/// The longest.
const CAP_MS: i64 = 900_000;

/// One provider job's keychain backoff.
#[derive(Debug, Default)]
pub struct KeychainBackoff {
    /// Locked runs in a row; 0 while not backing off.
    level: u32,
    /// When the current delay ends, epoch ms.
    until_ms: Option<i64>,
}

impl KeychainBackoff {
    pub fn new() -> Self {
        Self::default()
    }

    /// After a run that made `reads` (D4.1): `Some(delay_ms)` while backing off, `None` when
    /// this run ended the backoff or none was ever started.
    pub fn after_run(&mut self, reads: ReadTally, now_ms: i64) -> Option<i64> {
        if reads.answered > 0 {
            self.level = 0;
            self.until_ms = None;
            return None;
        }
        if reads.looks_locked() {
            self.level = self.level.saturating_add(1);
        }
        if self.level == 0 {
            return None;
        }
        let delay = (START_MS << (self.level - 1).min(5)).min(CAP_MS);
        self.until_ms = Some(now_ms.saturating_add(delay));
        Some(delay)
    }

    /// The backoff as `status` and the `mode` event show it, while it holds.
    pub fn current(&self) -> Option<Backoff> {
        self.until_ms.map(|ms| Backoff {
            until: (ms + 999).div_euclid(1000),
        })
    }

    /// When the current delay ends, epoch ms.
    pub(crate) fn until_ms(&self) -> Option<i64> {
        self.until_ms
    }
}
```

In `crates/tagteam-daemon/src/provider_job.rs`:

1. Imports. Replace `use tagteam_provider::keychain::ReadCounts;` with
   `use tagteam_provider::keychain::{ReadCounts, ReadTally};`, and add
   `use crate::backoff::KeychainBackoff;` beside the other `crate::` imports.

2. `struct Shown` gains whether a backoff holds:

```rust
/// What a `mode` event last stated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shown {
    mode: Mode,
    waiting_for: Option<WaitingFor>,
    holder_pid: Option<u32>,
    backoff: bool,
}
```

3. In `struct ProviderJob`, after the `quarantines` field, add:

```rust
    /// D4.1: the keychain backoff, which carries over a mode change.
    backoff: KeychainBackoff,
```

   and in `ProviderJob::new`'s struct literal, after `quarantines: QuarantineWatch::new(),`,
   add `backoff: KeychainBackoff::new(),`.

4. Add, after `fn now_s`:

```rust
    /// The secret reads made so far, over every Keychain instance the job was given (D4.1).
    fn reads(&self) -> ReadTally {
        self.ctx
            .keychain_counts
            .iter()
            .map(|c| c.tally())
            .fold(ReadTally::default(), |sum, t| sum + t)
    }
```

5. Replace `fn set_mode` with:

```rust
    fn set_mode(
        &mut self,
        mode: Mode,
        waiting_for: Option<WaitingFor>,
        holder_pid: Option<u32>,
        cx: &JobCx<'_>,
    ) {
        if self.mode != Some(mode) {
            if mode == Mode::Collect {
                self.quarantines = QuarantineWatch::new();
            }
            // A mode that works starts with its work, once a keychain backoff lets it (D4.3).
            let now = self.engine.now_ms();
            self.work_due_ms = self.backoff.until_ms().map_or(now, |until| until.max(now));
            self.work_delay_ms = self.work_due_ms - now;
        }
        self.mode = Some(mode);
        self.waiting_for = waiting_for;
        self.holder_pid = holder_pid;
        self.announce(cx);
    }
```

6. Replace `fn announce` with:

```rust
    /// D4.3: a mode change, and a keychain backoff starting or ending, is a `mode` event
    /// (D5.5) and an INFO line (D11).
    fn announce(&mut self, cx: &JobCx<'_>) {
        let Some(mode) = self.mode else {
            return;
        };
        let backoff = self.backoff.current();
        let shown = Shown {
            mode,
            waiting_for: self.waiting_for,
            holder_pid: self.holder_pid,
            backoff: backoff.is_some(),
        };
        if self.shown == Some(shown) {
            return;
        }
        let moved = self.shown.is_none_or(|s| {
            (s.mode, s.waiting_for, s.holder_pid) != (mode, self.waiting_for, self.holder_pid)
        });
        self.shown = Some(shown);
        if moved {
            tracing::info!(
                provider = %self.provider,
                mode = mode.as_str(),
                waiting_for = self.waiting_for.map(|w| w.as_str()),
                holder_pid = self.holder_pid,
                backoff_until = backoff.map(|b| b.until),
                "the provider's daemon mode changed"
            );
        } else if let Some(b) = backoff {
            tracing::info!(
                provider = %self.provider,
                until = b.until,
                "the keychain looks locked, so the provider's runs back off"
            );
        } else {
            tracing::info!(
                provider = %self.provider,
                "the keychain answered, so the provider's backoff ended"
            );
        }
        cx.sink.emit(&DaemonEvent::Mode {
            provider: self.provider.clone(),
            mode,
            waiting_for: self.waiting_for,
            holder_pid: self.holder_pid,
            backoff,
        });
    }
```

7. Replace `fn step` with:

```rust
    /// One run: the mode first, then its work when the work's deadline has come. A keychain
    /// backoff moves that deadline later, never earlier (D4.1, D4.3).
    fn step(&mut self, cx: &mut JobCx<'_>) -> Ran {
        let now = self.engine.now_ms();
        // D4.2: a wall clock set back never stretches the wait past the delay it was set for.
        self.work_due_ms = self.work_due_ms.min(now.saturating_add(self.work_delay_ms));
        if let Err(e) = self.evaluate(cx) {
            if e.signal().is_some() {
                return Ran::Stop(StopReason::Signal);
            }
            self.failed(cx, &e);
            return Ran::After(RECHECK_MS);
        }
        if !matches!(self.mode, Some(Mode::Auto | Mode::Collect)) {
            return Ran::After(RECHECK_MS);
        }
        let now = self.engine.now_ms();
        if now < self.work_due_ms {
            // D4.3: evaluated again at least every 60 s, while the work waits its deadline.
            return Ran::After((self.work_due_ms - now).clamp(1, RECHECK_MS));
        }
        self.last_tick_at = Some(now.div_euclid(1000));
        // D4.1: every secret read the run makes, whether it collects, freshens or switches.
        let before = self.reads();
        let worked = if self.mode == Some(Mode::Auto) {
            self.tick(cx)
        } else {
            self.collect(cx)
        };
        let Some(worked) = worked else {
            return Ran::Stop(StopReason::Signal);
        };
        let reads = self.reads().since(before);
        self.after_run_notices(cx, worked.switched);
        let now = self.engine.now_ms();
        let delay_ms = match self.backoff.after_run(reads, now) {
            Some(backoff_ms) => worked.delay_ms.max(backoff_ms),
            None => worked.delay_ms,
        };
        self.announce(cx);
        self.work_delay_ms = delay_ms;
        self.work_due_ms = now + delay_ms;
        Ran::After(delay_ms.clamp(1, RECHECK_MS))
    }
```

8. In `fn publish`, replace `backoff: None,` with `backoff: self.backoff.current(),`.

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-daemon backoff`, then
`cargo test -p tagteam-daemon --features test-hooks`; then
`cargo fmt --all`, `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`
Expected: PASS (3 backoff unit tests, 5 `backoff_tests`, and Task 8's tests unchanged), and no
warning.

- [ ] **Step 5: Commit** (the controller commits; implementers do not)

`git commit -m "Back a provider job off while the keychain looks locked"`

---

### Task 10: Store and upgrade watches

**Depends on:** 3, 8, 9

D4.4: "Every 2 s, the `store-watch` job reads `Engine::data_version()`. When it changed, it emits
one `changed` event. … Several commits within one watch period give one `changed`." Decision 11
extends it to a store another process replaced: the watch "also compares the store file's
device and inode every 2 s. When they changed, or the file vanished, it calls
`Engine::forget_store()` … and emits `changed`." D4.3: a provider's mode is re-evaluated "after
any `changed` event". D4.5: "Every 60 s, the `upgrade-watch` job checks two things": whether the
file the program path resolves to was replaced (device, inode, size, mtime), and whether "a newer
tagteam migrate[d] the store past this build's schema version … while the daemon ran". Either is
`stopping` and exit 75.

**Readings of the spec this task commits to:**
- **Waking the provider jobs** is a `Wake` handle the `Scheduler` hands out
  (`Scheduler::wake()`). The store watch, given one through `StoreWatch::waking`, wakes every job
  whose name starts with `provider:` after it emits `changed`. Before its next sleep or pick, the
  scheduler makes each named job due now, once; a job then follows its own delays. A provider
  job's run evaluates its mode first (Task 8), so being woken is re-evaluating. A shared flag the
  jobs read was rejected: a job reads a flag only when it runs, and a job 60 s from its next run
  would still need the scheduler to make it due, so the flag would add a second mechanism to the
  one that is needed anyway.
- **The store watch starts from a baseline, and its first run compares** (Decision 11). The
  baseline is the store file's device and inode (or that there is none) and `data_version` on
  the engine's connection to that file. `StoreBaseline::take` establishes it on a fresh
  connection: it looks at the file, forgets the engine's connection, reopens the store, and
  looks again. When the two looks differ, another process replaced the store in between, so
  it tries again, at most three times. After a third try whose looks still differ, it keeps
  the look taken before the last open: a file other than that one at the first run is a move,
  so a race costs at most one `changed` too many, never a connection left on an unlinked file.
  `Daemon::start` (Task 12) takes it once `daemon.lock` is held and before any provider job is
  built, and `StoreWatch::new` takes it as a parameter. A store replaced, or committed to,
  between start and the first run is then a change at that run; a store that is there from the
  start and unchanged is none; one that appears later is a change (Review Focus 3). A store
  this build refuses (`UnsupportedSchema`) has no connection, so its baseline has no version;
  any other failure to open is `take`'s error, which Task 12 makes a start failure.
- **A store replaced or removed:** the watch calls `forget_store()`, emits `changed`, and takes
  the new connection's `data_version` as a new baseline, since `data_version` values compare
  only on one connection (Task 3): it forgets the old value before it reads the new one, and
  never compares the two. The engine's own writes never change its connection's
  `data_version`, so they never give `changed` (D4.4).
- **A watch that cannot read** (metadata other than "not found", or a store this build refuses)
  sets its `JobStatus.last_error`, logs at WARN once per distinct error by its kind (§14.2: an
  error's text may name a stored value), and keeps its cadence.
- **The upgrade watch** compares against the stamp it is given (`ProgramStamp`), taken by its
  caller, never against the file as it is when the watch is built (Decision 17): `tagteam
  daemon` stamps the program first thing, and builds the watch only after the unlock offer and
  the `daemon.lock` wait. The stamp follows links, so a moved Homebrew link is an upgrade
  (D-R4). A path that resolves to nothing now, after resolving to a file at the stamp, is an
  upgrade (D4.5). A path that resolved to nothing at the stamp and still does is not: a restart
  would find nothing either. Metadata that cannot be read when the stamp is taken counts as no
  file. A program file whose metadata cannot be read at a run is an error on the watch's
  status, not an exit.
- **The schema check** (`schema_newer`) reads `PRAGMA user_version` through the engine's
  connection, and counts an open that refuses the store (`UnsupportedSchema`) as newer. It is
  public, so Task 12 can compute `ProviderJobCtx::schema_newer` and the watch's
  `schema_newer_at_start` with it (D4.1 step 4).
- **`program_path`** reads D9.1 as: `argv0` when absolute; else the first `tagteam` in an
  absolute `PATH` entry whose device and inode are the running binary's, passing over one that
  only shadows it; else `current_exe`. A wrapper that shadows the stable link then no longer
  forces the versioned Cellar path, which `brew cleanup` deletes. Because the match is stricter
  than "the first executable on `PATH`", it does not use Task 3's `find_on_path`. Plan B's
  `install` calls `program_path` too, so the unit records the path this watch watches.
- **Status times are epoch seconds** (Decision 12): each watch's `JobStatus` divides the
  scheduler's milliseconds by 1000.

**Files:**
- Create: `crates/tagteam-daemon/src/watch.rs`
- Modify: `crates/tagteam-daemon/src/scheduler.rs` (`Wake`, `Scheduler::wake`, and the pass that
  applies wakes)
- Modify: `crates/tagteam-daemon/src/lib.rs` (`pub mod watch;`)
- Modify: `crates/tagteam-daemon/src/testutil.rs` (`Fx::other`, `Fx::add_on`, `Fx::remove_store`)
- Modify: `crates/tagteam-daemon/Cargo.toml` (`rusqlite` as a dev-dependency)
- Test: `crates/tagteam-daemon/src/watch.rs`, `crates/tagteam-daemon/src/scheduler.rs` (a new
  `mod wake_tests`)

**Interfaces:**
- Consumes: Task 3's `Engine::data_version`, `Engine::forget_store`, `Engine::store_file`,
  `Store::data_version`; the existing `Engine::existing_store`; Task 7's `Scheduler`,
  `#[derive(Default)]`, whose `slots: Vec<Slot>` each hold `job: Box<dyn Job + 'j>`, `due_ms`
  and `delay_ms`, and whose `run` reads the wall clock into `now_ms` at the top of each pass,
  after `check_settings`; `Job`, `JobCx`, `Ran`; Task 6's `DaemonEvent::Changed`,
  `StopReason`; Task 8's `StatusBoard`, `JobStatus`, `ProviderJob`, `JOB_PREFIX`, and
  `testutil`'s `Fx`, `Slices`, `Emitted`, `board`, `job_ctx`, `two`, `T0`, `cc` and the private
  `engine_on`.
- Produces:
  - `scheduler.rs`: `#[derive(Debug, Clone, Default)] pub struct Wake`, `Wake::wake(&self,
    prefix: &str)`, `Scheduler::wake(&self) -> Wake`.
  - `watch.rs`: `StoreBaseline::take(engine) -> Result<StoreBaseline, EngineError>`,
    `StoreWatch::new(board, baseline)`, `StoreWatch::{with_period, waking}` (`impl Job`,
    `"store-watch"`), `ProgramStamp::{take(path), path()}`,
    `UpgradeWatch::new(program: ProgramStamp, schema_newer_at_start, board)`,
    `UpgradeWatch::with_period` (`impl Job`, `"upgrade-watch"`), `program_path`,
    `pub fn schema_newer(engine: &Engine) -> bool`, `pub const STORE_WATCH_MS: i64 = 2_000`,
    `pub const UPGRADE_WATCH_MS: i64 = 60_000`.
  - `testutil.rs`: `Fx::other`, `Fx::add_on`, `Fx::remove_store`.
  - For Task 12: take `StoreBaseline::take(engine)` in `Daemon::start` once `daemon.lock` is
    held, and build the store watch as `StoreWatch::new(board, baseline).waking(scheduler.wake())`;
    build the upgrade watch from the `ProgramStamp` the CLI took first.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-daemon/Cargo.toml`, under `[dev-dependencies]`, add:

```toml
rusqlite.workspace = true
```

Append to `crates/tagteam-daemon/src/testutil.rs` (it already imports `AddOptions`, `Engine`
and `AccountId`):

```rust
impl Fx {
    /// Another process on the same home: an engine of its own, with its own store connection.
    pub(crate) fn other(&self) -> Engine {
        engine_on(&self.env, &self.kc, &self.clock, &self.http)
    }

    /// `claude /login` as `email`, then `tagteam add` through `engine`.
    pub(crate) fn add_on(&self, engine: &Engine, email: &str) -> AccountId {
        self.login(email);
        let opts = AddOptions {
            provider: cc(),
            position: None,
            alias: None,
            yes: false,
        };
        engine.add_live(opts).unwrap().account.id
    }

    /// The store file and its sidecars removed, as a person deleting the store by hand would.
    pub(crate) fn remove_store(&self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut name = self.engine.store_file().into_os_string();
            name.push(suffix);
            match fs::remove_file(&name) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => panic!("{}: {e}", std::path::Path::new(&name).display()),
            }
        }
    }
}
```

At the end of `crates/tagteam-daemon/src/scheduler.rs`, add:

```rust
#[cfg(test)]
mod wake_tests {
    use std::cell::RefCell;

    use tagteam_provider::{Clock, FakeClock};

    use super::*;
    use crate::events::StopReason;
    use crate::testutil::{Emitted, Fx, Slices, T0};

    /// A job that runs every `period_ms`, recording each run, and on its `on`-th run wakes
    /// every job whose name starts with `prefix`.
    struct Every<'a> {
        name: &'static str,
        period_ms: i64,
        clock: &'a FakeClock,
        runs: &'a RefCell<Vec<(&'static str, i64)>>,
        wake: Option<(Wake, usize, &'static str)>,
    }

    impl Job for Every<'_> {
        fn name(&self) -> String {
            self.name.to_owned()
        }

        fn run(&mut self, _cx: &mut JobCx<'_>) -> Ran {
            let mut runs = self.runs.borrow_mut();
            runs.push((self.name, self.clock.now_ms().div_euclid(1000) - T0));
            let mine = runs.iter().filter(|(n, _)| *n == self.name).count();
            if let Some((wake, on, prefix)) = &self.wake {
                if mine == *on {
                    wake.wake(prefix);
                }
            }
            Ran::After(self.period_ms)
        }
    }

    #[test]
    fn a_wake_makes_every_job_it_names_due_before_the_next_sleep() {
        // D4.3: after `changed`, every provider job re-evaluates at once. The jobs a wake
        // names run before the scheduler sleeps again, once, and then keep their own
        // cadence; a job it does not name keeps its deadline.
        let fx = Fx::new();
        let runs = RefCell::new(Vec::new());
        let (clock, cancel) = (fx.clock.clone(), fx.engine.cancel().clone());
        let slices = Slices::new(&fx.clock, move |_| {
            if clock.now_ms() >= (T0 + 10) * 1000 {
                cancel.request(libc::SIGTERM);
            }
        });
        let mut scheduler = Scheduler::new();
        let wake = scheduler.wake();
        let now = fx.engine.now_ms();
        for name in ["provider:a", "provider:b", "upgrade-watch"] {
            let job = Every {
                name,
                period_ms: 60_000,
                clock: &fx.clock,
                runs: &runs,
                wake: None,
            };
            scheduler.add(Box::new(job), now);
        }
        let waker = Every {
            name: "store-watch",
            period_ms: 2_000,
            clock: &fx.clock,
            runs: &runs,
            wake: Some((wake, 3, "provider:")),
        };
        scheduler.add(Box::new(waker), now);
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        assert_eq!(scheduler.run(&mut cx, &slices), StopReason::Signal);
        let of = |name: &str| -> Vec<i64> {
            runs.borrow()
                .iter()
                .filter(|(n, _)| *n == name)
                .map(|(_, at)| *at)
                .collect()
        };
        assert_eq!(of("provider:a"), [0, 4]);
        assert_eq!(of("provider:b"), [0, 4]);
        assert_eq!(of("upgrade-watch"), [0]);
        assert_eq!(of("store-watch"), [0, 2, 4, 6, 8]);
    }
}
```

Create `crates/tagteam-daemon/src/watch.rs` holding only this test module, and add
`pub mod watch;` to `crates/tagteam-daemon/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::{Duration, SystemTime};

    use tagteam_provider::{Clock, FakeClock};

    use super::*;
    use crate::events::Mode;
    use crate::provider_job::ProviderJob;
    use crate::scheduler::Scheduler;
    use crate::testutil::{Emitted, Fx, Slices, T0, board, cc, job_ctx, two};

    /// How many `changed` events `events` holds.
    fn changed(events: &Emitted<'_>) -> usize {
        events
            .events()
            .iter()
            .filter(|(_, e)| *e == DaemonEvent::Changed)
            .count()
    }

    /// The store watch's baseline as `Daemon::start` takes it (Task 12).
    fn baseline(fx: &Fx) -> StoreBaseline {
        StoreBaseline::take(&fx.engine).unwrap()
    }

    /// `PRAGMA user_version` past this build's, as a newer tagteam's migration leaves it.
    fn migrate_past(fx: &Fx) {
        let conn = rusqlite::Connection::open(fx.engine.store_file()).unwrap();
        conn.execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION + 1))
            .unwrap();
    }

    fn set_mtime(p: &Path, secs: u64) {
        fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    /// A program file holding `bytes`, with an mtime `secs` after the epoch.
    fn program(dir: &Path, name: &str, bytes: &[u8], secs: u64) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, bytes).unwrap();
        set_mtime(&p, secs);
        p
    }

    /// An executable file at `rel` under `root`, its directories created.
    fn executable(root: &Path, rel: &str) -> PathBuf {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// The upgrade watch's first run after `change` touched its program file, which starts as
    /// nine bytes with an mtime of 1000 s.
    fn after(fx: &Fx, change: impl FnOnce(&Path)) -> Ran {
        let dir = tempfile::tempdir().unwrap();
        let exe = program(dir.path(), "tagteam", b"binary v1", 1_000);
        let mut watch = UpgradeWatch::new(ProgramStamp::take(exe.clone()), false, board());
        change(&exe);
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        watch.run(&mut cx)
    }

    /// A job that records, by name, when each of its runs began, in seconds after `T0`.
    struct Counted<'a, J> {
        job: J,
        clock: &'a FakeClock,
        runs: &'a RefCell<Vec<(String, i64)>>,
    }

    impl<J: Job> Job for Counted<'_, J> {
        fn name(&self) -> String {
            self.job.name()
        }

        fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
            let at = self.clock.now_ms().div_euclid(1000) - T0;
            self.runs.borrow_mut().push((self.job.name(), at));
            self.job.run(cx)
        }

        fn settings_changed(&mut self, cx: &mut JobCx<'_>) {
            self.job.settings_changed(cx);
        }
    }

    #[test]
    fn another_process_s_commits_are_one_changed_and_the_engine_s_own_writes_none() {
        // D4.4: two commits by another connection within one period give one `changed`;
        // this daemon's own writes give none (they come as their own events).
        let fx = Fx::new();
        fx.add("a@x.co");
        let other = fx.other();
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut watch = StoreWatch::new(board.clone(), baseline(&fx));
        assert_eq!(watch.run(&mut cx), Ran::After(STORE_WATCH_MS));
        assert_eq!(changed(&events), 0, "nothing changed since the baseline");
        let theirs = other.store().unwrap();
        theirs.set_unhealthy_ticks(&cc(), 1).unwrap();
        theirs.set_unhealthy_ticks(&cc(), 2).unwrap();
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1);
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1, "nothing since");
        fx.engine.store().unwrap().set_unhealthy_ticks(&cc(), 3).unwrap();
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1, "the engine's own write");
        assert_eq!(
            board.snapshot().jobs,
            [JobStatus {
                name: "store-watch".into(),
                last_run_at: Some(T0),
                next_run_at: Some(T0 + 2),
                last_error: None,
            }]
        );
    }

    #[test]
    fn a_replaced_store_file_is_reopened_and_reported_as_changed() {
        // Review Focus 2, Decision 11. The store this daemon has open is removed, and another
        // process creates a new one at the same path (the store deleted by hand, then
        // `tagteam add`). The watch reports `changed` once, and the engine reopens the path:
        // it reads the new store, its writes land there, and the watch follows that file's
        // commits. Nothing is written into the unlinked one.
        let fx = Fx::new();
        two(&fx);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut watch = StoreWatch::new(board.clone(), baseline(&fx));
        watch.run(&mut cx);
        fx.remove_store();
        let other = fx.other();
        fx.add_on(&other, "c@x.co");
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1);
        // Decision 11: the new connection's first `data_version` was a baseline. Compared with
        // the unlinked file's, it would read as a commit here.
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1, "nothing committed since the reopen");
        let emails: Vec<Option<String>> = fx
            .engine
            .existing_store()
            .unwrap()
            .unwrap()
            .accounts(&cc())
            .unwrap()
            .into_iter()
            .map(|r| r.email)
            .collect();
        assert_eq!(emails, [Some("c@x.co".to_owned())]);
        other.store().unwrap().set_unhealthy_ticks(&cc(), 1).unwrap();
        watch.run(&mut cx);
        assert_eq!(changed(&events), 2, "another process's commit to the new file");
        fx.engine.store().unwrap().set_unhealthy_ticks(&cc(), 2).unwrap();
        watch.run(&mut cx);
        assert_eq!(changed(&events), 2, "the engine's own write");
        let state = other.store().unwrap().autoswitch_state(&cc()).unwrap();
        assert_eq!(state.unhealthy_ticks, 2, "the engine wrote into the file the path names");
    }

    #[test]
    fn a_store_that_vanished_is_forgotten_and_reported_once() {
        let fx = Fx::new();
        fx.add("a@x.co");
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut watch = StoreWatch::new(board.clone(), baseline(&fx));
        watch.run(&mut cx);
        fx.remove_store();
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1);
        assert!(
            fx.engine.existing_store().unwrap().is_none(),
            "the unlinked store is never used again"
        );
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1);
    }

    #[test]
    fn the_first_run_compares_with_the_baseline() {
        // Decision 11: `Daemon::start` takes the baseline, and serving begins later (plan B
        // binds its socket in between). With nothing changed since, the first run reports
        // nothing. Another process's commit since is reported at the first run, which a first
        // run that only recorded would miss.
        let fx = Fx::new();
        fx.add("a@x.co");
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut quiet = StoreWatch::new(board(), baseline(&fx));
        quiet.run(&mut cx);
        assert_eq!(changed(&events), 0, "nothing changed since the baseline");
        let taken = baseline(&fx);
        fx.other().store().unwrap().set_unhealthy_ticks(&cc(), 1).unwrap();
        let mut watch = StoreWatch::new(board(), taken);
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1, "a commit between the baseline and the first run");
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1, "nothing since");
    }

    #[test]
    fn a_store_replaced_after_the_baseline_is_reopened_at_the_first_run() {
        // Decision 11: the baseline names the file the engine's connection has open. The store
        // is deleted, and another process creates a new one, after the baseline and before the
        // watch first runs (between `Daemon::start` and serving). That first run sees another
        // file: it forgets the engine's connection and reports `changed`, and the engine reads
        // the new store from then on. A first run that only recorded would take the new file
        // for the baseline and leave the engine on the unlinked one.
        let fx = Fx::new();
        two(&fx);
        let taken = baseline(&fx);
        fx.remove_store();
        let other = fx.other();
        fx.add_on(&other, "c@x.co");
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut watch = StoreWatch::new(board(), taken);
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1);
        let emails: Vec<Option<String>> = fx
            .engine
            .existing_store()
            .unwrap()
            .unwrap()
            .accounts(&cc())
            .unwrap()
            .into_iter()
            .map(|r| r.email)
            .collect();
        assert_eq!(
            emails,
            [Some("c@x.co".to_owned())],
            "the engine forgot its connection to the unlinked store"
        );
        watch.run(&mut cx);
        assert_eq!(changed(&events), 1, "nothing committed since the reopen");
    }

    #[test]
    fn a_store_that_appears_later_moves_the_provider_out_of_idle() {
        // Review Focus 3: a fresh machine. The daemon starts before any store exists, so the
        // provider is `idle`. A first `tagteam add` from another process creates the store:
        // the watch's next run reports `changed`, and the provider job, woken by it, moves to
        // `collect` before the scheduler sleeps again.
        let fx = Fx::new();
        assert!(!fx.engine.store_file().exists());
        let other = fx.other();
        let board = board();
        let cancel = fx.engine.cancel().clone();
        let slices = Slices::new(&fx.clock, |n| match n {
            2 => {
                fx.add_on(&other, "a@x.co");
            }
            _ if n >= 4 => cancel.request(libc::SIGTERM),
            _ => {}
        });
        let mut scheduler = Scheduler::new();
        let wake = scheduler.wake();
        let now = fx.engine.now_ms();
        scheduler.add(
            Box::new(ProviderJob::new(&fx.engine, cc(), job_ctx(&board))),
            now,
        );
        let store = StoreWatch::new(board.clone(), baseline(&fx)).waking(wake);
        scheduler.add(Box::new(store), now);
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        assert_eq!(scheduler.run(&mut cx, &slices), StopReason::Signal);
        assert_eq!(
            events.modes(&cc()),
            [(0, Mode::Idle, None, None), (4, Mode::Collect, None, None)]
        );
        let order: Vec<(i64, &str)> = events
            .events()
            .into_iter()
            .filter_map(|(at, e)| match e {
                DaemonEvent::Changed => Some((at, "changed")),
                DaemonEvent::Mode {
                    mode: Mode::Collect,
                    ..
                } => Some((at, "collect")),
                _ => None,
            })
            .collect();
        assert_eq!(order, [(4, "changed"), (4, "collect")]);
    }

    #[test]
    fn a_suspend_runs_each_overdue_job_once_on_waking() {
        // Review Focus 4. The machine sleeps for an hour during the third slice. Every
        // deadline is wall clock, so on waking each job that fell due runs once, at once:
        // never once per period it slept through. The store watch's 2 s cadence resumes from
        // there.
        let fx = Fx::new();
        let dir = tempfile::tempdir().unwrap();
        let exe = program(dir.path(), "tagteam", b"binary v1", 1_000);
        let board = board();
        let runs = RefCell::new(Vec::new());
        let (clock, cancel) = (fx.clock.clone(), fx.engine.cancel().clone());
        let slices = Slices::new(&fx.clock, move |n| match n {
            2 => clock.advance_ms(3_600_000),
            _ if n >= 5 => cancel.request(libc::SIGTERM),
            _ => {}
        });
        let mut scheduler = Scheduler::new();
        let now = fx.engine.now_ms();
        let provider = ProviderJob::new(&fx.engine, cc(), job_ctx(&board));
        scheduler.add(
            Box::new(Counted {
                job: provider,
                clock: &fx.clock,
                runs: &runs,
            }),
            now,
        );
        scheduler.add(
            Box::new(Counted {
                job: StoreWatch::new(board.clone(), baseline(&fx)),
                clock: &fx.clock,
                runs: &runs,
            }),
            now,
        );
        scheduler.add(
            Box::new(Counted {
                job: UpgradeWatch::new(ProgramStamp::take(exe), false, board.clone()),
                clock: &fx.clock,
                runs: &runs,
            }),
            now,
        );
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        assert_eq!(scheduler.run(&mut cx, &slices), StopReason::Signal);
        let of = |name: &str| -> Vec<i64> {
            runs.borrow()
                .iter()
                .filter(|(n, _)| n == name)
                .map(|(_, at)| *at)
                .collect()
        };
        assert_eq!(of("provider:claude-code"), [0, 3_603]);
        assert_eq!(of("store-watch"), [0, 2, 3_603, 3_605]);
        assert_eq!(of("upgrade-watch"), [0, 3_603]);
    }

    #[test]
    fn an_unchanged_program_file_keeps_the_daemon_running() {
        let fx = Fx::new();
        let dir = tempfile::tempdir().unwrap();
        let exe = program(dir.path(), "tagteam", b"binary v1", 1_000);
        let board = board();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let stamp = ProgramStamp::take(exe.clone());
        assert_eq!(stamp.path(), exe.as_path());
        let mut watch = UpgradeWatch::new(stamp.clone(), false, board.clone());
        assert_eq!(watch.run(&mut cx), Ran::After(UPGRADE_WATCH_MS));
        assert_eq!(watch.run(&mut cx), Ran::After(UPGRADE_WATCH_MS));
        assert_eq!(
            board.snapshot().jobs,
            [JobStatus {
                name: "upgrade-watch".into(),
                last_run_at: Some(T0),
                next_run_at: Some(T0 + 60),
                last_error: None,
            }]
        );
        let mut fast = UpgradeWatch::new(stamp, false, board).with_period(500);
        assert_eq!(fast.run(&mut cx), Ran::After(500));
        assert!(events.events().is_empty());
    }

    #[test]
    fn the_watch_compares_with_the_stamp_it_is_given_never_with_the_file_when_built() {
        // D4.5, Decision 17: `tagteam daemon` stamps its program first thing, and builds the
        // watch only after the unlock offer and the `daemon.lock` wait (Task 12). A tagteam
        // installed in between is an upgrade at the watch's first run, never the stamp of a
        // daemon that still runs the old binary.
        let fx = Fx::new();
        let dir = tempfile::tempdir().unwrap();
        let exe = program(dir.path(), "tagteam", b"binary v1", 1_000);
        let stamp = ProgramStamp::take(exe.clone());
        let next = exe.with_file_name("tagteam.new");
        fs::write(&next, b"binary v2").unwrap();
        set_mtime(&next, 1_000);
        fs::rename(&next, &exe).unwrap();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut watch = UpgradeWatch::new(stamp, false, board());
        assert_eq!(watch.run(&mut cx), Ran::Stop(StopReason::Upgrade));
    }

    #[test]
    fn another_mtime_size_or_file_or_none_at_the_path_is_an_upgrade() {
        // D4.5: the file the program path resolves to, by device, inode, size and mtime.
        let fx = Fx::new();
        let upgrade = Ran::Stop(StopReason::Upgrade);
        assert_eq!(after(&fx, |_| {}), Ran::After(UPGRADE_WATCH_MS));
        assert_eq!(after(&fx, |p| set_mtime(p, 2_000)), upgrade, "another mtime");
        let longer = |p: &Path| {
            fs::write(p, b"binary v1.1").unwrap();
            set_mtime(p, 1_000);
        };
        assert_eq!(after(&fx, longer), upgrade, "another size, the same mtime");
        let renamed = |p: &Path| {
            let next = p.with_file_name("tagteam.new");
            fs::write(&next, b"binary v2").unwrap();
            set_mtime(&next, 1_000);
            fs::rename(&next, p).unwrap();
        };
        assert_eq!(after(&fx, renamed), upgrade, "another file of the same size and mtime");
        let removed = |p: &Path| fs::remove_file(p).unwrap();
        assert_eq!(after(&fx, removed), upgrade, "nothing: tagteam was uninstalled");
    }

    #[test]
    fn a_stable_link_moved_to_a_new_binary_is_an_upgrade() {
        // D-R4: Homebrew's `bin/tagteam` links into a versioned directory. The watch follows
        // the link, so moving it to the new version is an upgrade while the old binary stays.
        let fx = Fx::new();
        let dir = tempfile::tempdir().unwrap();
        let old = program(dir.path(), "tagteam-1", b"binary v1", 1_000);
        let new = program(dir.path(), "tagteam-2", b"binary v2", 1_000);
        let link = dir.path().join("tagteam");
        symlink(&old, &link).unwrap();
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut watch = UpgradeWatch::new(ProgramStamp::take(link.clone()), false, board());
        assert_eq!(watch.run(&mut cx), Ran::After(UPGRADE_WATCH_MS));
        fs::remove_file(&link).unwrap();
        symlink(&new, &link).unwrap();
        assert_eq!(watch.run(&mut cx), Ran::Stop(StopReason::Upgrade));
        assert!(old.exists());
    }

    #[test]
    fn a_store_migrated_past_this_build_while_running_stops_the_daemon() {
        // D4.5: a newer tagteam migrated the store while the daemon ran. It shows through the
        // engine's open connection, and, once the engine reopens, as the open's refusal.
        let fx = Fx::new();
        fx.add("a@x.co");
        let dir = tempfile::tempdir().unwrap();
        let exe = program(dir.path(), "tagteam", b"binary v1", 1_000);
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let stamp = ProgramStamp::take(exe);
        let mut watch = UpgradeWatch::new(stamp.clone(), false, board());
        assert_eq!(watch.run(&mut cx), Ran::After(UPGRADE_WATCH_MS));
        migrate_past(&fx);
        assert_eq!(watch.run(&mut cx), Ran::Stop(StopReason::Schema));
        fx.engine.forget_store();
        let mut reopened = UpgradeWatch::new(stamp, false, board());
        assert_eq!(reopened.run(&mut cx), Ran::Stop(StopReason::Schema));
    }

    #[test]
    fn a_store_already_newer_at_start_waits_for_the_binary() {
        // D4.1 step 4: the daemon started on a store this build cannot open. A restart would
        // bring the same binary back to the same store, so only a new binary ends the wait.
        let fx = Fx::new();
        fx.add("a@x.co");
        migrate_past(&fx);
        fx.engine.forget_store();
        assert!(schema_newer(&fx.engine));
        let dir = tempfile::tempdir().unwrap();
        let exe = program(dir.path(), "tagteam", b"binary v1", 1_000);
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        let mut watch = UpgradeWatch::new(ProgramStamp::take(exe.clone()), true, board());
        assert_eq!(watch.run(&mut cx), Ran::After(UPGRADE_WATCH_MS));
        set_mtime(&exe, 2_000);
        assert_eq!(watch.run(&mut cx), Ran::Stop(StopReason::Upgrade));
    }

    #[test]
    fn schema_newer_reads_the_store_s_version_or_its_refusal() {
        let fx = Fx::new();
        assert!(!schema_newer(&fx.engine), "no store");
        fx.add("a@x.co");
        assert!(!schema_newer(&fx.engine), "this build's schema");
        migrate_past(&fx);
        assert!(schema_newer(&fx.engine), "through the open connection");
        fx.engine.forget_store();
        assert!(schema_newer(&fx.engine), "an open that refuses it");
    }

    #[test]
    fn the_program_path_is_argv0_when_absolute_else_the_tagteam_on_path_that_runs() {
        // D4.5, D9.1. A service passes its absolute program path. From a shell argv[0] is a
        // bare name, so the path is the first `tagteam` on PATH that is the running binary (a
        // stable link survives upgrades), passing over one that only shadows it; otherwise
        // the running binary's own path.
        let dir = tempfile::tempdir().unwrap();
        let exe = executable(dir.path(), "Cellar/tagteam/1/bin/tagteam");
        executable(dir.path(), "shadow/tagteam");
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        symlink(&exe, bin.join("tagteam")).unwrap();
        let path = |dirs: &[PathBuf]| std::env::join_paths(dirs).unwrap();
        assert_eq!(
            program_path(OsStr::new("/opt/tagteam/bin/tagteam"), None, &exe),
            PathBuf::from("/opt/tagteam/bin/tagteam"),
            "absolute: as given, even when nothing is there"
        );
        let on_path = path(&[
            dir.path().join("missing"),
            dir.path().join("shadow"),
            bin.clone(),
        ]);
        assert_eq!(
            program_path(OsStr::new("tagteam"), Some(on_path.as_os_str()), &exe),
            bin.join("tagteam")
        );
        let shadowed = path(&[dir.path().join("shadow")]);
        assert_eq!(
            program_path(OsStr::new("tagteam"), Some(shadowed.as_os_str()), &exe),
            exe
        );
        assert_eq!(program_path(OsStr::new("./tagteam"), None, &exe), exe);
        assert_eq!(
            program_path(OsStr::new("tagteam"), Some(OsStr::new("bin")), &exe),
            exe,
            "a relative PATH entry names no stable path"
        );
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-daemon -- watch:: wake_tests`
Expected: FAIL to compile: `cannot find type Wake in this scope`, `no method named wake found
for struct Scheduler`, and `cannot find type StoreWatch` (likewise `StoreBaseline`,
`UpgradeWatch`, `ProgramStamp`, `program_path`, `schema_newer`, `STORE_WATCH_MS`,
`UPGRADE_WATCH_MS`).

- [ ] **Step 3: Implement the wake**

In `crates/tagteam-daemon/src/scheduler.rs`:

1. Task 7's imports have no `std::sync` line. Before `use std::time::{Duration, SystemTime};`,
   add:

```rust
use std::sync::{Arc, Mutex, PoisonError};
```

2. Add, before `pub struct Scheduler`:

```rust
/// D4.3: a handle a job keeps to make other jobs due at once. `wake(prefix)` makes every job
/// whose name starts with `prefix` due before the scheduler next sleeps or picks a job; each
/// then runs once and keeps its own cadence after. The store watch wakes `provider:` after a
/// `changed` event, so every provider job re-evaluates its mode (D4.4).
#[derive(Debug, Clone, Default)]
pub struct Wake(Arc<Mutex<Vec<String>>>);

impl Wake {
    pub fn wake(&self, prefix: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(prefix.to_owned());
    }

    /// The prefixes woken since the last call.
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}
```

3. Add a field to `pub struct Scheduler<'j>`, after `settings`:

```rust
    /// What jobs woke since the last pass; every handle `wake()` gave out feeds it.
    wake: Wake,
```

   `Scheduler` derives `Default` and `new()` is `Self::default()`, and `Wake` derives `Default`
   too, so neither needs another change.

4. Add to `impl<'j> Scheduler<'j>`:

```rust
    /// The handle jobs wake each other through.
    pub fn wake(&self) -> Wake {
        self.wake.clone()
    }

    /// Every job a `Wake` named since the last pass is due now; a deadline already past keeps
    /// its place.
    fn woken(&mut self, now_ms: i64) {
        for prefix in self.wake.take() {
            for slot in self
                .slots
                .iter_mut()
                .filter(|s| s.job.name().starts_with(&prefix))
            {
                slot.due_ms = slot.due_ms.min(now_ms);
            }
        }
    }
```

5. In `Scheduler::run`'s loop, right after `let now_ms = cx.engine.now_ms();` (which follows
   `self.check_settings(cx);`, and comes before the `for slot in &mut self.slots` clamp and the
   pick of the next job), add:

```rust
            self.woken(now_ms);
```

- [ ] **Step 4: Implement the watches**

Put this above the test module in `crates/tagteam-daemon/src/watch.rs`:

```rust
//! D4.4 and D4.5: the jobs that watch the store and the program file. The store watch tells
//! subscribers that another process changed the store, and wakes every provider job to
//! re-evaluate its mode; the upgrade watch stops the daemon to be restarted on a new binary,
//! or past a newer tagteam's store migration (exit 75).

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tagteam_engine::store::{SCHEMA_VERSION, StoreError};
use tagteam_engine::{Engine, EngineError};

use crate::events::{DaemonEvent, StopReason};
use crate::provider_job::JOB_PREFIX;
use crate::scheduler::{Job, JobCx, Ran, Wake};
use crate::status::{JobStatus, StatusBoard};

/// D4.4: the store watch's period.
pub const STORE_WATCH_MS: i64 = 2_000;
/// D4.5: the upgrade watch's period.
pub const UPGRADE_WATCH_MS: i64 = 60_000;

/// A file's device and inode, following links; `None` when nothing is there.
fn file_identity(path: &Path) -> io::Result<Option<(u64, u64)>> {
    match fs::metadata(path) {
        Ok(m) => Ok(Some((m.dev(), m.ino()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// D4.1 step 4 and D4.5: whether the store's schema is newer than this build's (§6.1), read
/// through the engine's connection, or from an open that refuses it. No store is not newer,
/// and neither is a store that cannot be read for another reason.
pub fn schema_newer(engine: &Engine) -> bool {
    match engine.existing_store() {
        Ok(Some(store)) => store.schema_version().is_ok_and(|v| v > SCHEMA_VERSION),
        Ok(None) => false,
        Err(EngineError::Store(StoreError::UnsupportedSchema(_))) => true,
        Err(_) => false,
    }
}

/// D4.5 and D9.1: the program path the upgrade watch watches. `argv0` when it is absolute (a
/// service unit passes it so). Otherwise the first `tagteam` in an absolute `PATH` entry that
/// is the running binary (`current_exe`, by device and inode), so a package manager's stable
/// link survives an upgrade. Otherwise `current_exe` itself.
pub fn program_path(argv0: &OsStr, path_var: Option<&OsStr>, current_exe: &Path) -> PathBuf {
    let argv0 = Path::new(argv0);
    if argv0.is_absolute() {
        return argv0.to_path_buf();
    }
    let running = file_identity(current_exe).ok().flatten();
    if let (Some(running), Some(path_var)) = (running, path_var) {
        for dir in std::env::split_paths(path_var).filter(|d| d.is_absolute()) {
            let candidate = dir.join("tagteam");
            if file_identity(&candidate).ok().flatten() == Some(running) {
                return candidate;
            }
        }
    }
    current_exe.to_path_buf()
}

/// How many times `StoreBaseline::take` opens the store while another process keeps
/// replacing it.
const BASELINE_TRIES: u32 = 3;

/// Decision 11: what the store watch's first run compares with: the store file's device and
/// inode, and `data_version` on the engine's connection to that file. `Daemon::start` takes it
/// once `daemon.lock` is held and before any provider job uses the store, so a store replaced
/// or committed to before the watch first runs is a change at that run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreBaseline {
    /// The store file's device and inode; `None` when there is no store.
    file: Option<(u64, u64)>,
    /// `data_version` on the engine's connection to that file; `None` without a connection.
    version: Option<i64>,
}

impl StoreBaseline {
    /// Forgets the engine's connection and opens the store again, between two looks at the
    /// store file's device and inode, so the identity recorded is the file the new connection
    /// has open. When the looks differ, another process replaced the store meanwhile, and it
    /// tries again, up to three times. A third try whose looks still differ keeps the look
    /// taken before its open: a file other than that one at the watch's first run is a move,
    /// so a race costs at most one `changed` too many, never a connection left on an unlinked
    /// file.
    ///
    /// A store this build refuses (`UnsupportedSchema`, D4.1 step 4) has no connection, so no
    /// version; the watch reports the refusal on its status at each run. Any other failure to
    /// open the store, or to read its metadata, is the error.
    pub fn take(engine: &Engine) -> Result<StoreBaseline, EngineError> {
        let path = engine.store_file();
        let mut tries = 0;
        loop {
            tries += 1;
            let before = file_identity(&path)?;
            engine.forget_store();
            let version = match engine.existing_store() {
                Ok(Some(store)) => Some(store.data_version()?),
                Ok(None) | Err(EngineError::Store(StoreError::UnsupportedSchema(_))) => None,
                Err(e) => return Err(e),
            };
            if file_identity(&path)? == before || tries == BASELINE_TRIES {
                return Ok(StoreBaseline {
                    file: before,
                    version,
                });
            }
        }
    }
}

/// D4.4 and Decision 11: every 2 s, whether another process committed to the store (the
/// engine connection's `data_version`), and whether the store file is still the one the
/// engine opened (its device and inode). Either is one `changed` event, after which every
/// provider job is woken to re-evaluate its mode (D4.3). A store replaced or removed is
/// forgotten, so the engine's next use opens the file the path names now.
pub struct StoreWatch {
    board: Arc<StatusBoard>,
    period_ms: i64,
    wake: Option<Wake>,
    /// The store file's device and inode at the last run, or at the baseline before the
    /// first; `None` when there was none.
    file: Option<(u64, u64)>,
    /// The engine connection's `data_version` at the last run, or at the baseline.
    version: Option<i64>,
    /// The last run's failure, so it is logged once until it changes.
    failing: Option<String>,
}

impl StoreWatch {
    /// A watch whose first run compares with `baseline` (`StoreBaseline::take`), as every
    /// later run compares with the run before.
    pub fn new(board: Arc<StatusBoard>, baseline: StoreBaseline) -> Self {
        StoreWatch {
            board,
            period_ms: STORE_WATCH_MS,
            wake: None,
            file: baseline.file,
            version: baseline.version,
            failing: None,
        }
    }

    pub fn with_period(mut self, ms: i64) -> Self {
        self.period_ms = ms;
        self
    }

    /// After each `changed`, the provider jobs are woken through `wake` (D4.3).
    pub fn waking(mut self, wake: Wake) -> Self {
        self.wake = Some(wake);
        self
    }

    /// One look: whether the store changed since the last look, or since the baseline at the
    /// first, and what could not be read.
    fn check(&mut self, engine: &Engine) -> (bool, Option<EngineError>) {
        let file = match file_identity(&engine.store_file()) {
            Ok(file) => file,
            Err(e) => return (false, Some(e.into())),
        };
        let moved = self.file != file;
        self.file = file;
        if moved {
            // Decision 11: the cached connection may be to an unlinked file. A new connection's
            // `data_version` compares with nothing before it.
            engine.forget_store();
            self.version = None;
        }
        let version = match file {
            Some(_) => match engine.data_version() {
                Ok(v) => v,
                Err(e) => return (moved, Some(e)),
            },
            None => None,
        };
        let before = std::mem::replace(&mut self.version, version);
        let committed = matches!((before, version), (Some(b), Some(v)) if b != v);
        (moved || committed, None)
    }
}

impl Job for StoreWatch {
    fn name(&self) -> String {
        "store-watch".to_owned()
    }

    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
        let now_ms = cx.engine.now_ms();
        let (changed, failure) = self.check(cx.engine);
        if changed {
            tracing::debug!("another process changed the store");
            cx.sink.emit(&DaemonEvent::Changed);
            if let Some(wake) = &self.wake {
                wake.wake(JOB_PREFIX);
            }
        }
        let last_error = failure.map(|e| {
            let message = e.to_string();
            if self.failing.as_ref() != Some(&message) {
                // §14.2: the error's kind, never its text, which may quote a stored value.
                tracing::warn!(kind = e.kind(), "the store watch could not read the store");
            }
            message
        });
        self.failing = last_error.clone();
        self.board.set_job(JobStatus {
            name: self.name(),
            last_run_at: Some(now_ms.div_euclid(1000)),
            next_run_at: Some((now_ms + self.period_ms).div_euclid(1000)),
            last_error,
        });
        Ran::After(self.period_ms)
    }
}

/// The file a program path resolves to: its device, inode, size and mtime (D4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
}

/// The file `path` resolves to, following links; `None` when nothing is there.
fn file_stamp(path: &Path) -> io::Result<Option<FileStamp>> {
    match fs::metadata(path) {
        Ok(m) => Ok(Some(FileStamp {
            dev: m.dev(),
            ino: m.ino(),
            size: m.size(),
            mtime: m.mtime(),
            mtime_nsec: m.mtime_nsec(),
        })),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// D4.5 and Decision 17: a program path, and the file it resolved to when the stamp was taken.
/// `tagteam daemon` takes it first thing, before anything that can wait (an unlock offer, the
/// `daemon.lock` wait), and the upgrade watch compares every run with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramStamp {
    path: PathBuf,
    /// The file `path` resolved to; `None` when it resolved to nothing, or its metadata could
    /// not be read.
    start: Option<FileStamp>,
}

impl ProgramStamp {
    /// Records the file `path` resolves to now, following links.
    pub fn take(path: PathBuf) -> Self {
        let start = file_stamp(&path).ok().flatten();
        ProgramStamp { path, start }
    }

    /// The path the watch follows.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// D4.5: every 60 s, whether the program file was replaced, or a newer tagteam migrated the
/// store past this build's schema while the daemon ran. Either stops the scheduler, so the
/// daemon exits 75 and a service restarts it on the new binary (D9).
pub struct UpgradeWatch {
    /// The program path, and the file it resolved to at the stamp (Decision 17).
    program: ProgramStamp,
    /// D4.1 step 4: a store already newer at start-up waits for the binary instead.
    schema_newer_at_start: bool,
    board: Arc<StatusBoard>,
    period_ms: i64,
}

impl UpgradeWatch {
    /// A watch that compares every run, its first included, with `program`'s stamp. It never
    /// stats the file itself here: a binary replaced between the stamp and now is an upgrade.
    pub fn new(program: ProgramStamp, schema_newer_at_start: bool, board: Arc<StatusBoard>) -> Self {
        UpgradeWatch {
            program,
            schema_newer_at_start,
            board,
            period_ms: UPGRADE_WATCH_MS,
        }
    }

    pub fn with_period(mut self, ms: i64) -> Self {
        self.period_ms = ms;
        self
    }

    /// Why the daemon must restart, if it must. A program file whose metadata cannot be read
    /// is the error, and no reason to restart.
    fn check(&self, engine: &Engine) -> io::Result<Option<StopReason>> {
        if file_stamp(&self.program.path)? != self.program.start {
            return Ok(Some(StopReason::Upgrade));
        }
        if !self.schema_newer_at_start && schema_newer(engine) {
            return Ok(Some(StopReason::Schema));
        }
        Ok(None)
    }
}

impl Job for UpgradeWatch {
    fn name(&self) -> String {
        "upgrade-watch".to_owned()
    }

    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
        let now_ms = cx.engine.now_ms();
        let (stop, last_error) = match self.check(cx.engine) {
            Ok(stop) => (stop, None),
            Err(e) => {
                tracing::warn!(kind = %e.kind(), "the upgrade watch could not read the program file");
                (None, Some(format!("{}: {e}", self.program.path.display())))
            }
        };
        self.board.set_job(JobStatus {
            name: self.name(),
            last_run_at: Some(now_ms.div_euclid(1000)),
            next_run_at: stop
                .is_none()
                .then(|| (now_ms + self.period_ms).div_euclid(1000)),
            last_error,
        });
        match stop {
            Some(reason) => {
                tracing::info!(
                    reason = reason.as_str(),
                    "tagteam was upgraded, so the daemon stops to be restarted"
                );
                Ran::Stop(reason)
            }
            None => Ran::After(self.period_ms),
        }
    }
}
```

- [ ] **Step 5: Run the tests and see them pass**

Run: `cargo test -p tagteam-daemon -- watch:: wake_tests`, then
`cargo test -p tagteam-daemon --features test-hooks`; then `cargo fmt --all`,
`cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`
Expected: PASS (15 watch tests, 1 wake test, and every earlier test of the crate), and no
warning.

- [ ] **Step 6: Commit** (the controller commits; implementers do not)

`git commit -m "Watch the store and the program file from the daemon"`

---

### Task 11: Notifications

**Depends on:** 3, 5, 8, 9, 10

D6: the daemon tells the user about the events that matter. A `NotifySink` turns the daemon's own
`switch` events (never a dry run) and its `account-quarantined` events into notices. The provider
job asks `limit_notice` after a run that did not switch. A `Notifier` delivers each notice on a
thread of its own through the engine's `ProcessSpawner`, so the scheduler never waits on one
(D4). This task also gives `ProviderJobCtx` its `notices` field and fills in Task 8's empty
`ProviderJob::after_run_notices`. The sink's echo to the caller's sink (`NotifySink::echoing`)
is Task 12's, beside `Started::serve`, its one caller: added here, it would be dead code at
this task's clippy run.

**Readings of the spec this task commits to:**
- **The process port already does what D6.2 asks.** `tagteam_provider::process::run_captured`,
  which `SystemSpawner` calls, starts the child in a process group of its own
  (`process_group(0)`). Its stdin is `Stdio::null()`. Its stdout and stderr are pipes, drained on
  threads of their own, so a chatty backend never blocks and nothing reaches the daemon's
  terminal. The notifier throws those bytes away and never logs them, because `notify-send` could
  echo the body. A timeout or a cancel kills the group. Nothing needs adding.
- **Each delivery gets its own `Cancel`,** never the daemon's. A stop therefore never kills a
  notification half-way, and `Notifier::stop` waits for the one in flight (at most 5 s, plus
  `run_captured`'s 2 s drain grace). Notices still queued when the stop flag is seen are dropped
  (D6.2).
- **The notifier thread logs through the dispatcher it was started under**
  (`tracing::dispatcher::get_default`). In production that is the global subscriber, so nothing
  changes. In a test it is the test's capture, so the notifier's lines can be asserted.
- **A failed delivery** is logged at WARN the first time and at DEBUG after that (D6.2). The line
  carries only `backend` and `error` (the exit code, the signal, `timed out` or the spawn error).
  It never carries the text, which names accounts (§14.2). A delivery that succeeds is logged at
  DEBUG (D11).
- **Settings are read for each notifiable event** with `Settings::load(env, provider)`, not
  cached. `switch` and `account-quarantined` are rare, and a setting changed a moment ago then
  always counts, with no cache to refresh. The `limit` notice uses the provider job's own
  settings (Decision 8), which every reload refreshes.
- **`{account}` (§13.5):** `<position> · <alias>`. Without an alias it is the local part of the
  email, or of the label for a provider whose logins have none. A notice never holds a whole
  email, an organization name or a credential (D6.2).
- **The switch notice's reason** is the binding window of the account switched away from
  (`7d at 95%`). It comes from that account's decision-grade reading, through
  `Engine::account_view`. A failover names its trigger instead (`failover`), since the account
  left was not at its limit. So does any switch whose account has no decision-grade reading.
- **The limit notice reads** `2 · a: 7d at 93% (resets in 3d09h)` (Decision 16). Durations use
  `span`, the helper the CLI's `duration` re-exports (`tagteam_core::rank::span`). D6.2's
  example adds "no account to switch to". That is only true for some of the reasons a run does
  not switch, and `limit_notice` is not told the reason, so it is left out.
- **A binding window whose reset has already passed is not notified.** Its pct belongs to a
  window instance that is over. A lease taken for it would expire in the past and be taken
  again on every run.
- **An unreadable live login is no notice.** `Engine::live_reading` returns
  `EngineError::Unreadable` for a live identity it cannot read, as a tick does (Task 3).
  `limit_notice` treats that as nothing to judge: it logs a DEBUG line by the error's kind,
  takes no marker, and returns `Ok(None)`. Any other error is returned, and `after_run` logs it
  at WARN; neither fails the run.
- **`after_run` is the one place limits are judged.** `ProviderJob::after_run_notices` delegates
  to it, so its logic and tests live here.
- **`DaemonSink::notified`** is a new defaulted method (Interfaces). D8.1's human output has a
  line per notification. Limit notices bypass events entirely, so the sink must be told when a
  notice is queued. `Fanout` forwards it; `AutoOnly` and plan B's hub keep the no-op default.
  Neither `--json` nor the socket carries it.
- **The fixture.** The tests run on `testutil`'s `Fx`. This task extends it with what they need
  (Execution notes): the home's `ScriptedSpawner`, a run-shell constructor, a login that names
  its organization (so a test can show a notice never carries it), a reading of any windows,
  and `capture_logs`. `Emitted` also records what is announced to it through `notified`.

**Files:**
- Create: `crates/tagteam-daemon/src/notify.rs`
- Modify: `crates/tagteam-daemon/src/lib.rs` (`pub mod notify;`)
- Modify: `crates/tagteam-daemon/src/testutil.rs` (`Fx` gains `spawner`, `with_run_shell`,
  `login_as`, `add_as` and `read_windows`; `engine_on` takes the spawner and the run shell;
  `SHORT_RESETS`, `LONG_RESETS`, `windows` and `capture_logs`; `Emitted` records `notified`;
  `job_ctx` sets `notices: None`)
- Modify: `crates/tagteam-daemon/src/events.rs` (`DaemonSink` gains `notified`; `Fanout` forwards
  it)
- Modify: `crates/tagteam-daemon/src/provider_job.rs` (`ProviderJobCtx::notices`; the body of
  `after_run_notices`)
- Modify: `crates/tagteam-daemon/Cargo.toml` (`uuid`; dev-dependency `tracing-subscriber`)
- Test: `crates/tagteam-daemon/src/notify.rs` (`mod tests`)

**Interfaces:**
- Consumes:
  - Task 3:
    - `tagteam_engine::daemon::find_on_path(name, path_var) -> Option<PathBuf>`
    - `Engine::live_reading(&self, provider, models) -> Result<Option<LiveReading>, EngineError>`,
      `Err(EngineError::Unreadable)` for an unreadable live identity
    - `LiveReading { account, position, alias, email, label, windows }`
    - `Store::take_lease(&self, name, holder, now_ms, expires_at_ms) -> Result<bool, StoreError>`
  - Task 5: `Settings { notify_enabled, notify_events, .. }`, `NotifyEvent::{Switch, Quarantine,
    Limit}`.
  - Task 6: `DaemonEvent`, `DaemonSink`, `Fanout`.
  - Task 7: `Job`, `JobCx { engine, sink, jitter }`.
  - Task 8:
    - `ProviderJob::new(engine, provider, ProviderJobCtx)`
    - `ProviderJobCtx { board, keychain_counts, schema_newer }`, to which this task adds
      `notices`
    - `ProviderJob`'s fields `provider`, `settings: Settings` and `ctx`, and its private
      `after_run_notices(&mut self, _cx: &mut JobCx<'_>, _switched: bool)`, which Task 8 left
      empty and calls after each run that did work
    - `testutil`'s `Fx`, `Emitted`, `board`, `job_ctx`, `cc`, `T0` and the private `engine_on`
  - Task 10: `testutil`'s `Fx::other`.
  - Existing:
    - `tagteam_provider::process::{ProcessSpawner, SpawnSpec, Captured, ScriptedSpawner}`
    - `tagteam_provider::splice::replace_top_level`
    - `Engine::account_view(&self, row, active) -> AccountView`
    - `tagteam_core::rank::{binding_window, span}`
- Produces (Interfaces, `notify.rs`):
  - `Backend { Osascript, NotifySend(PathBuf) }`, with `detect`, `name` and `spec`
  - `Notice { title, body }`
  - `NoticeQueue::send(&self, n)`, which never blocks, and the `cfg(test)`
    `NoticeQueue::for_test() -> (NoticeQueue, mpsc::Receiver<Notice>)`
  - `Notifier::start(backend, spawner) -> (Notifier, NoticeQueue)` and `Notifier::stop(self)`
  - `NotifySink::new(engine, queue)` and `impl DaemonSink` (Task 12 adds the crate-private
    `NotifySink::echoing(self, to)` beside its one caller, `Started::serve`)
  - `limit_notice(engine, provider, settings, now_ms) -> Result<Option<Notice>, EngineError>`
  - the crate-private `notify::after_run(engine, provider, settings, queue: Option<&NoticeQueue>,
    sink, switched)`
- Produces, elsewhere:
  - `DaemonSink::notified(&self, provider: &ProviderId, notice: &Notice) {}` (defaulted), which
    `Fanout` forwards to each sink
  - `ProviderJobCtx::notices: Option<NoticeQueue>`
  - `testutil`: `SHORT_RESETS`, `LONG_RESETS`, `windows`, `capture_logs`, `Fx::spawner`,
    `Fx::with_run_shell`, `Fx::login_as`, `Fx::add_as`, `Fx::read_windows`,
    `Emitted::announced`

- [ ] **Step 1: Write the failing tests**

First, the dependencies. Task 6 gave the crate `tagteam-cc` and `tempfile` as dev-dependencies
already. In `crates/tagteam-daemon/Cargo.toml`, add under `[dependencies]`:

```toml
uuid.workspace = true
```

for the lease holder (D6.3), and under `[dev-dependencies]`:

```toml
tracing-subscriber.workspace = true
```

Both are workspace dependencies (`uuid` with `v7`), so no new external crate is added.

In `crates/tagteam-daemon/src/lib.rs`, add `pub mod notify;` to the module list, in alphabetical
order with the rest.

Extend `crates/tagteam-daemon/src/testutil.rs`.

Its imports: replace `use std::sync::Arc;` with

```rust
use std::sync::{Arc, Mutex, OnceLock};
```

after `use tagteam_provider::profile::RunShell;` add

```rust
use tagteam_provider::splice::replace_top_level;
```

and add `use crate::notify::Notice;` beside the other `crate::` imports.

After `pub(crate) const CC_USAGE: …;`, add:

```rust
/// When every fixture reading's 5h window resets.
pub(crate) const SHORT_RESETS: i64 = T0 + 9_630;
/// When every fixture reading's 7d window resets: 3d09h after `T0`.
pub(crate) const LONG_RESETS: i64 = T0 + 291_630;
```

After `pub(crate) fn fake() -> ProviderId { … }`, add:

```rust
/// Claude Code's two windows as a fetch reports them: 5h at `short` and 7d at `long` percent,
/// resetting at `SHORT_RESETS` and `LONG_RESETS`.
pub(crate) fn windows(short: f64, long: f64) -> Vec<Window> {
    let window = |key: &str, kind, pct, resets_at, period_s| Window {
        key: key.into(),
        label: key.into(),
        kind,
        pct,
        resets_at: Some(resets_at),
        period_s: Some(period_s),
        detail: None,
    };
    vec![
        window("5h", WindowKind::Short, short, SHORT_RESETS, 18_000),
        window("7d", WindowKind::Long, long, LONG_RESETS, 604_800),
    ]
}
```

`engine_on` takes the home's spawner and the run shell. Replace its doc comment and signature:

```rust
/// An engine on `env` with Claude Code and FakeAgent registered. Claude Code's live Keychain
/// reads are retried without a wait (Appendix A.3), so a locked keychain costs a test no time.
fn engine_on(
    env: &Env,
    kc: &Arc<FakeKeychain>,
    clock: &Arc<FakeClock>,
    http: &Arc<ScriptedHttp>,
) -> Engine {
```

with:

```rust
/// An engine on `env` with Claude Code and FakeAgent registered, spawning through `spawner`
/// (which never starts a process, §15.1), inside or outside a run shell. Claude Code's live
/// Keychain reads are retried without a wait (Appendix A.3), so a locked keychain costs a test
/// no time.
fn engine_on(
    env: &Env,
    kc: &Arc<FakeKeychain>,
    clock: &Arc<FakeClock>,
    http: &Arc<ScriptedHttp>,
    spawner: &Arc<ScriptedSpawner>,
    run_shell: RunShell,
) -> Engine {
```

and in its body replace

```rust
        // Nothing here validates, and a scripted spawner never starts a process (§15.1).
        spawner: Arc::new(ScriptedSpawner::new()),
        run_shell: RunShell::Outside,
```

with

```rust
        spawner: spawner.clone(),
        run_shell,
```

In `pub(crate) struct Fx`, after `pub(crate) http: Arc<ScriptedHttp>,`, add:

```rust
    /// Every engine of this home spawns through it: the notifier's deliveries among them.
    pub(crate) spawner: Arc<ScriptedSpawner>,
```

Replace `Fx::new`:

```rust
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        fs::create_dir_all(env.home.join(".claude")).unwrap();
        let kc = Arc::new(FakeKeychain::new());
        let clock = Arc::new(FakeClock::new(T0 * 1000));
        let http = Arc::new(ScriptedHttp::new());
        let engine = engine_on(&env, &kc, &clock, &http);
        Fx {
            _dir: dir,
            env,
            kc,
            clock,
            http,
            engine,
        }
    }
```

with:

```rust
    pub(crate) fn new() -> Self {
        Self::with_run_shell(RunShell::Outside)
    }

    /// `new`, with the engine inside or outside a run shell (§12.8).
    pub(crate) fn with_run_shell(run_shell: RunShell) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        fs::create_dir_all(env.home.join(".claude")).unwrap();
        let kc = Arc::new(FakeKeychain::new());
        let clock = Arc::new(FakeClock::new(T0 * 1000));
        let http = Arc::new(ScriptedHttp::new());
        let spawner = Arc::new(ScriptedSpawner::new());
        let engine = engine_on(&env, &kc, &clock, &http, &spawner, run_shell);
        Fx {
            _dir: dir,
            env,
            kc,
            clock,
            http,
            spawner,
            engine,
        }
    }
```

Replace Task 10's `Fx::other`:

```rust
    /// Another process on the same home: an engine of its own, with its own store connection.
    pub(crate) fn other(&self) -> Engine {
        engine_on(&self.env, &self.kc, &self.clock, &self.http)
    }
```

with:

```rust
    /// Another process on the same home: an engine of its own, with its own store connection,
    /// over the same Keychain, clock and spawner. It is also what a restarted daemon starts
    /// with: no store open yet.
    pub(crate) fn other(&self) -> Engine {
        engine_on(
            &self.env,
            &self.kc,
            &self.clock,
            &self.http,
            &self.spawner,
            RunShell::Outside,
        )
    }
```

After that `impl Fx` block (Task 10's), add:

```rust
impl Fx {
    /// `claude /login` as `email` in the organization named `org`: `login`, with an
    /// `oauthAccount` that also names the organization, spliced into `~/.claude.json` so the
    /// file's other keys stay. A notice must never carry an organization's name (D6.2), so the
    /// tests that look for one log in this way.
    pub(crate) fn login_as(&self, email: &str, org: &str) {
        let path = self.env.home.join(".claude.json");
        let before = fs::read(&path).unwrap_or_else(|_| b"{}\n".to_vec());
        self.login(email);
        let account = json!({"emailAddress": email, "organizationUuid": format!("org-{org}"),
                             "organizationName": org, "accountUuid": format!("uuid-{email}")});
        fs::write(&path, replace_top_level(&before, "oauthAccount", &account).unwrap()).unwrap();
    }

    /// `login_as`, then `tagteam add` with `alias`: the login stays live.
    pub(crate) fn add_as(&self, email: &str, org: &str, alias: Option<&str>) -> AccountId {
        self.login_as(email, org);
        let opts = AddOptions {
            provider: cc(),
            position: None,
            alias: alias.map(str::to_owned),
            yes: false,
        };
        self.engine.add_live(opts).unwrap().account.id
    }

    /// `windows` as `id`'s reading taken at `at`, its next poll planned at `next_poll_at`,
    /// recorded as a fetch records one (§8.3): reserve, then record.
    pub(crate) fn read_windows(&self, id: &AccountId, windows: &[Window], at: i64, next_poll_at: i64) {
        let store = self.engine.store().unwrap();
        let row = store.account(id).unwrap().unwrap();
        let Reserve::Reserved(r) = store
            .reserve_usage(&row, at * 1000, Eligibility::Scheduled, &PollBudget::STANDARD)
            .unwrap()
        else {
            panic!("no reservation at {at}");
        };
        let plan = PollPlan {
            interval_s: next_poll_at - at,
            next_poll_at,
        };
        assert!(store.record_usage(&r, windows, at, &plan, 180).unwrap());
    }
}
```

`Emitted` also records each notice announced to it. Replace its struct and `new`:

```rust
/// Every daemon event, with the fixture clock's time it was emitted at, in seconds after `T0`.
pub(crate) struct Emitted<'a> {
    clock: &'a FakeClock,
    events: RefCell<Vec<(i64, DaemonEvent)>>,
}

impl<'a> Emitted<'a> {
    pub(crate) fn new(clock: &'a FakeClock) -> Self {
        Emitted {
            clock,
            events: RefCell::new(Vec::new()),
        }
    }
```

with:

```rust
/// Every daemon event, with the fixture clock's time it was emitted at, in seconds after `T0`,
/// and every notice announced to it (`DaemonSink::notified`), in order.
pub(crate) struct Emitted<'a> {
    clock: &'a FakeClock,
    events: RefCell<Vec<(i64, DaemonEvent)>>,
    announced: RefCell<Vec<(ProviderId, Notice)>>,
}

impl<'a> Emitted<'a> {
    pub(crate) fn new(clock: &'a FakeClock) -> Self {
        Emitted {
            clock,
            events: RefCell::new(Vec::new()),
            announced: RefCell::new(Vec::new()),
        }
    }

    /// Each notice announced to this sink, with its provider.
    pub(crate) fn announced(&self) -> Vec<(ProviderId, Notice)> {
        self.announced.borrow().clone()
    }
```

and in `impl DaemonSink for Emitted<'_>`, after `fn emit`, add:

```rust
    fn notified(&self, provider: &ProviderId, notice: &Notice) {
        self.announced
            .borrow_mut()
            .push((provider.clone(), notice.clone()));
    }
```

At the end of the file, add the log capture:

```rust
/// A `tracing` writer that appends to a shared buffer.
#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;

    fn make_writer(&'a self) -> LogBuffer {
        self.clone()
    }
}

/// A dispatcher that wants nothing, kept alive for the whole test process. `tracing` caches
/// each call site's interest from every registered dispatcher. With one alone, a thread with
/// no capture that reaches a call site first would cache it as unwanted for every capture.
/// The engine's test `capture_logs` explains this in full.
static ALWAYS_TWO: OnceLock<tracing::Dispatch> = OnceLock::new();

/// Runs `f` with every `tracing` event this thread emits captured. The notifier's thread is
/// captured too, since it takes the dispatcher it starts under. Returns `f`'s result and the
/// lines: level first, then the message and its fields, with no colour, time or target.
pub(crate) fn capture_logs<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
    ALWAYS_TWO.get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let result = tracing::subscriber::with_default(subscriber, f);
    let text = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    (result, text.lines().map(str::to_owned).collect())
}
```

Create `crates/tagteam-daemon/src/notify.rs` containing only this test module. Step 3 puts the
implementation above it.

```rust
#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    use tagteam_provider::process::ScriptedSpawner;

    use super::*;
    use crate::events::Fanout;
    use crate::provider_job::{ProviderJob, ProviderJobCtx};
    use crate::scheduler::{Job, JobCx};
    use crate::testutil::{
        Emitted, Fx, LONG_RESETS, T0, board, capture_logs, cc, job_ctx, windows,
    };

    /// What no notice of the fixture may carry (D6.2): either email whole, their domain, the
    /// organization's name, and any token (`at-<email>`, `rt-<email>`).
    const NEVER: [&str; 6] = ["w@b.example", "a@b.example", "b.example", "Acme", "at-", "rt-"];

    fn clean(body: &str) {
        for never in NEVER {
            assert!(!body.contains(never), "{never:?} is in {body:?}");
        }
    }

    fn notice(body: &str) -> Notice {
        Notice {
            title: "tagteam".into(),
            body: body.into(),
        }
    }

    fn exited(code: i32) -> Captured {
        Captured::Exited {
            code: Some(code),
            signal: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }

    /// `work` (w@b.example) at position 1 and a@b.example at 2, which is live. Both are in the
    /// organization "Acme". The live account is at 93 % of its 7d window, `work` at 20 %, both
    /// read now.
    fn two_at_the_limit() -> Fx {
        let home = Fx::new();
        let w = home.add_as("w@b.example", "Acme", Some("work"));
        let a = home.add_as("a@b.example", "Acme", None);
        home.read_windows(&w, &windows(10.0, 20.0), T0, T0 + 300);
        home.read_windows(&a, &windows(40.0, 93.0), T0, T0 + 300);
        home
    }

    fn switch(trigger: Trigger, from: u32, to: u32, dry_run: bool) -> AutoEvent {
        AutoEvent::Switch {
            provider: cc(),
            trigger,
            from,
            to,
            warnings: Vec::new(),
            dry_run,
        }
    }

    fn quarantined(number: u32, email: &str) -> AutoEvent {
        AutoEvent::AccountQuarantined {
            provider: cc(),
            number,
            email: email.into(),
            reason: "invalid_grant".into(),
        }
    }

    /// The notices `events` give, through a `NotifySink` over `home`'s engine.
    fn notices_for(home: &Fx, events: &[AutoEvent]) -> Vec<Notice> {
        let (queue, rx) = NoticeQueue::for_test();
        let sink = NotifySink::new(&home.engine, queue);
        for e in events {
            sink.emit(&DaemonEvent::Auto(e.clone()));
        }
        rx.try_iter().collect()
    }

    /// Waits up to 5 s for `spawner` to have recorded `n` specs, and returns them.
    fn delivered(spawner: &ScriptedSpawner, n: usize) -> Vec<SpawnSpec> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let specs = spawner.specs();
            if specs.len() >= n {
                return specs;
            }
            assert!(
                Instant::now() < deadline,
                "{} of {n} notices were delivered",
                specs.len()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn macos_always_has_osascript_and_linux_has_notify_send_only_on_path() {
        // D6.2: macOS's own program, never one on PATH; `notify-send` only where PATH has it.
        assert_eq!(Backend::detect(true, None), Some(Backend::Osascript));
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        assert_eq!(Backend::detect(false, Some(bin.as_os_str())), None);
        assert_eq!(Backend::detect(false, None), None);
        let program = bin.join("notify-send");
        fs::write(&program, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            Backend::detect(false, Some(bin.as_os_str())),
            Some(Backend::NotifySend(program.clone()))
        );
        assert_eq!(
            (Backend::Osascript.name(), Backend::NotifySend(program).name()),
            ("osascript", "notify-send")
        );
    }

    #[test]
    fn each_backend_passes_the_text_as_argv_never_as_script() {
        // D6.2: quotes, a newline and an emoji in an alias reach the program verbatim, as its
        // last argument. Nothing is quoted or escaped, so nothing can be injected.
        let body = "Switched 1 · \"wo'rk\"\nend 🚀 → 2 · spare (5h at 92%)";
        let args = |spec: &SpawnSpec| {
            spec.args
                .iter()
                .map(|a| a.to_str().unwrap().to_owned())
                .collect::<Vec<String>>()
        };
        let mac = Backend::Osascript.spec("tagteam", body);
        assert_eq!(mac.program, PathBuf::from("/usr/bin/osascript"));
        assert_eq!(
            args(&mac),
            [
                "-e",
                "on run argv",
                "-e",
                "display notification (item 2 of argv) with title (item 1 of argv)",
                "-e",
                "end run",
                "--",
                "tagteam",
                body,
            ]
        );
        let linux = Backend::NotifySend(PathBuf::from("/usr/bin/notify-send")).spec("tagteam", body);
        assert_eq!(linux.program, PathBuf::from("/usr/bin/notify-send"));
        assert_eq!(args(&linux), ["--app-name=tagteam", "--", "tagteam", body]);
        for spec in [&mac, &linux] {
            assert!(spec.set.is_empty() && spec.remove.is_empty() && spec.cwd.is_none());
            assert_eq!(spec.args.last().unwrap(), &OsString::from(body));
        }
    }

    #[test]
    fn a_failed_delivery_is_logged_once_at_warn_then_at_debug_and_the_next_still_goes() {
        // D6.2: a delivery that times out never stops the next one. Its failure is a WARN
        // line once and DEBUG after, naming the backend and how it failed, never the text.
        let spawner = Arc::new(ScriptedSpawner::new());
        spawner.push(Captured::TimedOut);
        spawner.push(Captured::Exited {
            code: Some(1),
            signal: None,
            stdout: Vec::new(),
            stderr: b"cannot show 2 \xc2\xb7 bravo".to_vec(),
        });
        spawner.push(exited(0));
        let backend = Backend::NotifySend(PathBuf::from("/usr/bin/notify-send"));
        let bodies = ["1 · alpha", "2 · bravo", "3 · charlie"];
        let ((), lines) = capture_logs(|| {
            let (notifier, queue) = Notifier::start(backend, spawner.clone());
            for body in bodies {
                queue.send(notice(body));
            }
            let specs = delivered(&spawner, 3);
            let last: Vec<&OsString> = specs.iter().map(|s| s.args.last().unwrap()).collect();
            assert_eq!(last, bodies.map(OsString::from).iter().collect::<Vec<_>>());
            notifier.stop();
        });
        let failures: Vec<&str> = lines
            .iter()
            .map(|l| l.trim_start())
            .filter(|l| l.contains("could not be delivered"))
            .collect();
        assert_eq!(failures.len(), 2, "{lines:#?}");
        assert!(
            failures[0].starts_with("WARN")
                && failures[0].contains("backend=\"notify-send\"")
                && failures[0].contains("error=timed out"),
            "{}",
            failures[0]
        );
        assert!(
            failures[1].starts_with("DEBUG") && failures[1].contains("error=exit 1"),
            "{}",
            failures[1]
        );
        let sent: Vec<&String> = lines.iter().filter(|l| l.contains("notification sent")).collect();
        assert_eq!(sent.len(), 1, "{lines:#?}");
        assert!(sent[0].trim_start().starts_with("DEBUG"), "{}", sent[0]);
        for line in &lines {
            for name in ["alpha", "bravo", "charlie"] {
                assert!(!line.contains(name), "a notice's text was logged: {line}");
            }
        }
    }

    #[test]
    fn a_full_queue_drops_a_notice_rather_than_wait() {
        // D4: the scheduler never waits on a notification.
        let (queue, rx) = NoticeQueue::for_test();
        let started = Instant::now();
        for n in 0..40 {
            queue.send(notice(&format!("notice {n}")));
        }
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(rx.try_iter().count(), 32);
    }

    #[test]
    fn a_stopped_notifier_delivers_nothing_more() {
        // D6.2: what is sent once the notifier has stopped is dropped.
        let spawner = Arc::new(ScriptedSpawner::new());
        let (notifier, queue) = Notifier::start(Backend::Osascript, spawner.clone());
        notifier.stop();
        queue.send(notice("too late"));
        thread::sleep(Duration::from_millis(200));
        assert!(spawner.specs().is_empty());
    }

    #[test]
    fn a_switch_and_a_quarantine_are_notified_naming_accounts_by_position_and_alias() {
        // D6.1, D6.2: the daemon's switches (never a dry run) and quarantines. An account is
        // named `<position> · <alias or email's local part>`. A row that is gone falls back to
        // the event's email's local part, or to the position alone.
        let home = two_at_the_limit();
        let got = notices_for(
            &home,
            &[
                switch(Trigger::Proactive, 2, 1, false),
                switch(Trigger::Proactive, 2, 1, true),
                switch(Trigger::Failover, 2, 1, false),
                switch(Trigger::AtLimit, 9, 2, false),
                quarantined(1, "w@b.example"),
                quarantined(9, "gone@b.example"),
                AutoEvent::NoSwitch {
                    provider: cc(),
                    reason: "below-threshold".into(),
                    detail: String::new(),
                },
            ],
        );
        let bodies: Vec<&str> = got.iter().map(|n| n.body.as_str()).collect();
        assert_eq!(
            bodies,
            [
                "Switched 2 · a → 1 · work (7d at 93%)",
                "Switched 2 · a → 1 · work (failover)",
                "Switched 9 → 2 · a (at-limit)",
                "1 · work needs a new login (invalid_grant)",
                "9 · gone needs a new login (invalid_grant)",
            ]
        );
        assert!(got.iter().all(|n| n.title == "tagteam"));
        for body in bodies {
            clean(body);
        }
    }

    #[test]
    fn notify_enabled_and_notify_events_are_read_for_each_event() {
        // D6.4, D7: `notify.enabled` turns every notice off. `notify.events` chooses the kinds,
        // the provider's own table first. A change counts from the next event on.
        let home = two_at_the_limit();
        let events = [switch(Trigger::Proactive, 2, 1, false), quarantined(1, "w@b.example")];
        home.settings("[notify]\nenabled = false\n", 1);
        assert!(notices_for(&home, &events).is_empty());
        home.settings("[provider.claude-code.notify]\nevents = [\"limit\"]\n", 2);
        assert!(notices_for(&home, &events).is_empty());
        home.settings(
            "[notify]\nevents = [\"limit\"]\n\n[provider.claude-code.notify]\nevents = [\"quarantine\"]\n",
            3,
        );
        assert_eq!(
            notices_for(&home, &events),
            [notice("1 · work needs a new login (invalid_grant)")]
        );
    }

    #[test]
    fn fanout_passes_each_announced_notice_to_every_sink() {
        // D8.1: `tagteam daemon` prints a line per notification. It is told through
        // `notified`, which `Fanout` hands to each of its sinks.
        let home = Fx::new();
        let (one, two) = (Emitted::new(&home.clock), Emitted::new(&home.clock));
        let quarantine = notice("1 · work needs a new login (invalid_grant)");
        Fanout(vec![&one, &two]).notified(&cc(), &quarantine);
        assert_eq!(one.announced(), [(cc(), quarantine.clone())]);
        assert_eq!(two.announced(), [(cc(), quarantine)]);
        assert!(one.events().is_empty() && two.events().is_empty());
    }

    #[test]
    fn the_limit_is_notified_once_per_window_instance_across_a_restart() {
        // D6.1, D6.3: a's 7d window binds at 93 % ≥ 90 %. The lease marks this window instance
        // in the store. A second call does not notify it again, and neither does a restarted
        // daemon's engine. The window's next instance is notified.
        let home = two_at_the_limit();
        let s = Settings::default();
        let first = limit_notice(&home.engine, &cc(), &s, T0 * 1000).unwrap();
        assert_eq!(first, Some(notice("2 · a: 7d at 93% (resets in 3d09h)")));
        clean(&first.unwrap().body);
        assert_eq!(limit_notice(&home.engine, &cc(), &s, T0 * 1000).unwrap(), None);
        assert_eq!(
            limit_notice(&home.other(), &cc(), &s, T0 * 1000 + 60_000).unwrap(),
            None
        );
        // The window reset, and its next instance is at 95 % already.
        let later = T0 + 600;
        home.clock.set(later * 1000);
        let a = home
            .engine
            .store()
            .unwrap()
            .find_by_position(&cc(), 2)
            .unwrap()
            .unwrap()
            .id;
        let mut next = windows(40.0, 95.0);
        next[1].resets_at = Some(LONG_RESETS + 604_800);
        home.read_windows(&a, &next, later, later + 300);
        assert_eq!(
            limit_notice(&home.engine, &cc(), &s, later * 1000).unwrap(),
            Some(notice("2 · a: 7d at 95% (resets in 10d08h)"))
        );
    }

    #[test]
    fn below_the_threshold_with_notices_off_or_without_a_managed_live_account_nothing_is_taken() {
        // D6.1, D6.4: nothing is notified, and no marker is taken that would hide a later
        // notice.
        let home = two_at_the_limit();
        let off = Settings {
            notify_enabled: false,
            ..Settings::default()
        };
        let no_limit = Settings {
            notify_events: vec![NotifyEvent::Switch, NotifyEvent::Quarantine],
            ..Settings::default()
        };
        let higher = Settings {
            threshold: 95.0,
            ..Settings::default()
        };
        for s in [&off, &no_limit, &higher] {
            assert_eq!(limit_notice(&home.engine, &cc(), s, T0 * 1000).unwrap(), None);
        }
        assert!(
            limit_notice(&home.engine, &cc(), &Settings::default(), T0 * 1000)
                .unwrap()
                .is_some(),
            "none of the above took the marker"
        );
        // A live login tagteam does not manage.
        let home = two_at_the_limit();
        home.login_as("stranger@b.example", "Acme");
        assert_eq!(
            limit_notice(&home.engine, &cc(), &Settings::default(), T0 * 1000).unwrap(),
            None
        );
    }

    #[test]
    fn a_window_whose_reset_has_passed_is_not_notified() {
        let home = Fx::new();
        let a = home.add_as("a@b.example", "Acme", None);
        let mut stale = windows(40.0, 93.0);
        stale[1].resets_at = Some(T0 - 1);
        home.read_windows(&a, &stale, T0, T0 + 300);
        assert_eq!(
            limit_notice(&home.engine, &cc(), &Settings::default(), T0 * 1000).unwrap(),
            None
        );
    }

    #[test]
    fn an_unreadable_live_login_is_no_notice_and_takes_no_marker() {
        // Task 3's `live_reading` fails for a live identity it cannot read, as a tick does. The
        // limit is then not judged: one DEBUG line, no notice and no marker, and no error.
        let home = two_at_the_limit();
        let config = home.env.home.join(".claude.json");
        let readable = fs::read(&config).unwrap();
        fs::write(&config, "not json").unwrap();
        let (judged, lines) = capture_logs(|| {
            limit_notice(&home.engine, &cc(), &Settings::default(), T0 * 1000)
        });
        assert_eq!(judged.unwrap(), None);
        let said: Vec<&str> = lines
            .iter()
            .map(|l| l.trim_start())
            .filter(|l| l.contains("the live login could not be read"))
            .collect();
        assert_eq!(said.len(), 1, "{lines:#?}");
        assert!(said[0].starts_with("DEBUG"), "{}", said[0]);
        fs::write(&config, readable).unwrap();
        assert_eq!(
            limit_notice(&home.engine, &cc(), &Settings::default(), T0 * 1000).unwrap(),
            Some(notice("2 · a: 7d at 93% (resets in 3d09h)")),
            "no marker was taken"
        );
    }

    #[test]
    fn a_run_that_switched_or_has_no_queue_judges_no_limit() {
        // D6.1: the limit is judged only after a run that did not switch, and only when
        // notices can be sent. Neither takes the marker.
        let home = two_at_the_limit();
        let s = Settings::default();
        let (queue, rx) = NoticeQueue::for_test();
        let echo = Emitted::new(&home.clock);
        after_run(&home.engine, &cc(), &s, Some(&queue), &echo, true);
        after_run(&home.engine, &cc(), &s, None, &echo, false);
        assert_eq!(rx.try_iter().count(), 0);
        assert!(echo.announced().is_empty());
        after_run(&home.engine, &cc(), &s, Some(&queue), &echo, false);
        let got: Vec<Notice> = rx.try_iter().collect();
        assert_eq!(got, [notice("2 · a: 7d at 93% (resets in 3d09h)")]);
        assert_eq!(echo.announced(), [(cc(), got[0].clone())]);
    }

    /// A provider job of `home`'s Claude Code, its notices into `queue`.
    fn job(home: &Fx, queue: NoticeQueue) -> ProviderJob<'_> {
        let ctx = ProviderJobCtx {
            notices: Some(queue),
            ..job_ctx(&board())
        };
        ProviderJob::new(&home.engine, cc(), ctx)
    }

    fn run_once(home: &Fx, job: &mut ProviderJob<'_>) {
        let sink = Emitted::new(&home.clock);
        let mut jitter = || 0.0;
        job.run(&mut JobCx {
            engine: &home.engine,
            sink: &sink,
            jitter: &mut jitter,
        });
    }

    #[test]
    fn the_limit_is_notified_after_a_collect_round_and_after_a_tick_that_did_not_switch() {
        // D6.1 in each mode (D14). One switchable account is `collect` (D4.3). Two are
        // `auto`, where `work` at 97 % has less headroom than a at 93 %, so the tick stays.
        let home = Fx::new();
        let a = home.add_as("a@b.example", "Acme", None);
        home.read_windows(&a, &windows(40.0, 93.0), T0, T0 + 300);
        let (queue, rx) = NoticeQueue::for_test();
        let mut collecting = job(&home, queue);
        run_once(&home, &mut collecting);
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            [notice("1 · a: 7d at 93% (resets in 3d09h)")]
        );
        home.clock.advance_ms(60_000);
        run_once(&home, &mut collecting);
        assert_eq!(rx.try_iter().count(), 0, "once per window instance");

        let home = Fx::new();
        let w = home.add_as("w@b.example", "Acme", Some("work"));
        let a = home.add_as("a@b.example", "Acme", None);
        home.read_windows(&w, &windows(10.0, 97.0), T0, T0 + 300);
        home.read_windows(&a, &windows(40.0, 93.0), T0, T0 + 300);
        let (queue, rx) = NoticeQueue::for_test();
        let mut ticking = job(&home, queue);
        run_once(&home, &mut ticking);
        assert_eq!(
            home.engine.store().unwrap().active(&cc()).unwrap(),
            Some(a),
            "the tick stayed"
        );
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            [notice("2 · a: 7d at 93% (resets in 3d09h)")]
        );
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-daemon notify::`
Expected: FAIL to compile. The tests name `Backend`, `Notice`, `NoticeQueue`, `Notifier`,
`NotifySink`, `limit_notice` and `after_run`, and `testutil.rs` names `crate::notify::Notice`
and implements `DaemonSink::notified`, none of which exists yet.

- [ ] **Step 3: Implement**

In `crates/tagteam-daemon/src/events.rs`, after `use tagteam_engine::auto::{AutoEvent, EventSink};`,
add a blank line and:

```rust
use crate::notify::Notice;
```

In `pub trait DaemonSink`, after its last method,

```rust
    /// The end of a tick of `provider`'s engine, as `EventSink::tick_done` has it.
    fn tick_done(&self, _provider: &ProviderId) {}
```

add:

```rust
    /// A notice was queued for `provider` (D6). `tagteam daemon` prints a line for it (D8.1).
    /// It is not an event, so neither `--json` nor the socket carries it.
    fn notified(&self, _provider: &ProviderId, _notice: &Notice) {}
```

In `impl DaemonSink for Fanout<'_>`, after its `tick_done` method, add:

```rust
    fn notified(&self, provider: &ProviderId, notice: &Notice) {
        for sink in &self.0 {
            sink.notified(provider, notice);
        }
    }
```

`AutoOnly` keeps the no-op default: `tagteam auto` has no notifier.

In `crates/tagteam-daemon/src/provider_job.rs`, add `use crate::notify::NoticeQueue;` beside the
other `crate::` imports. In `pub struct ProviderJobCtx`, after `pub board: Arc<StatusBoard>,`,
add:

```rust
    /// Where the job queues D6.1's `limit` notice (Task 11); `None` when there is no notifier.
    pub notices: Option<NoticeQueue>,
```

and in `crates/tagteam-daemon/src/testutil.rs`'s `job_ctx`, after `board: board.clone(),`, add
`notices: None,`. Every test builds its context there, so no other test changes.

Then replace the empty method Task 8 left, with its doc comment:

```rust
    /// D6.1's `limit` notice is judged here, from the store, after each run that did work: in
    /// `auto` after a tick that did not switch, in `collect` after every round. Notifications
    /// are not wired yet, so it does nothing.
    fn after_run_notices(&mut self, _cx: &mut JobCx<'_>, _switched: bool) {}
```

with:

```rust
    /// D6.1's `limit`, after a run that did work: judged only when the run did not switch
    /// (`switched` is false after an `auto` tick that did not switch, and after every `collect`
    /// round), and only when notices can be sent (`notify::after_run`).
    fn after_run_notices(&mut self, cx: &mut JobCx<'_>, switched: bool) {
        crate::notify::after_run(
            cx.engine,
            &self.provider,
            &self.settings,
            self.ctx.notices.as_ref(),
            cx.sink,
            switched,
        );
    }
```

The body uses Task 8's fields: `provider`, `settings` (the job's own `Settings`, Decision 8) and
`ctx`. Task 9's `step` calls it unchanged.

Put the implementation above the test module in `crates/tagteam-daemon/src/notify.rs`:

```rust
//! D6: notifications. A `NotifySink` turns the daemon's `switch` and `account-quarantined`
//! events into notices. The provider job asks `limit_notice` after a run that did not switch
//! (`after_run`). The `Notifier` delivers each notice on a thread of its own through the
//! engine's `ProcessSpawner`, so the scheduler never waits on one (D4).

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tagteam_core::autoswitch::Trigger;
use tagteam_core::rank::{binding_window, span};
use tagteam_core::{ProviderId, Window};
use tagteam_engine::auto::AutoEvent;
use tagteam_engine::daemon::find_on_path;
use tagteam_engine::settings::{NotifyEvent, Settings};
use tagteam_engine::store::AccountRow;
use tagteam_engine::{Engine, EngineError};
use tagteam_provider::Cancel;
use tagteam_provider::process::{Captured, ProcessSpawner, SpawnSpec};

use crate::events::{DaemonEvent, DaemonSink};

/// D6.2: every notice's title.
const TITLE: &str = "tagteam";
/// D6.2: macOS's own `osascript`, never one found on `PATH`.
const OSASCRIPT: &str = "/usr/bin/osascript";
/// D6.2: the script reads its title and body from argv, after `--`, so no account name can
/// inject AppleScript.
const OSASCRIPT_ARGS: [&str; 7] = [
    "-e",
    "on run argv",
    "-e",
    "display notification (item 2 of argv) with title (item 1 of argv)",
    "-e",
    "end run",
    "--",
];
/// D6.2: `notify-send`'s options; the title and body follow `--`.
const NOTIFY_SEND_ARGS: [&str; 2] = ["--app-name=tagteam", "--"];
/// How many notices wait for the notifier thread. When the queue is full, a new notice is
/// dropped (`NoticeQueue::send`).
const QUEUE: usize = 32;
/// D6.2: one delivery's bound.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(5);
/// How often an idle notifier thread looks for a stop.
const POLL: Duration = Duration::from_millis(100);
/// D6.3: a `limit` marker outlives its window's reset by this much…
const AFTER_RESET_MS: i64 = 60_000;
/// …and lives this long when the reset is unknown.
const UNKNOWN_RESET_MS: i64 = 3_600_000;

/// Where notices are shown (D6.2): macOS's `osascript`, or `notify-send` found on `PATH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    Osascript,
    NotifySend(PathBuf),
}

impl Backend {
    /// macOS always has its own `osascript`. Elsewhere it is `notify-send` on `path_var`, and
    /// without that there is no backend.
    pub fn detect(macos: bool, path_var: Option<&OsStr>) -> Option<Backend> {
        if macos {
            Some(Backend::Osascript)
        } else {
            find_on_path("notify-send", path_var).map(Backend::NotifySend)
        }
    }

    /// As `status` and `doctor` name it (D5.4, D10).
    pub fn name(&self) -> &'static str {
        match self {
            Backend::Osascript => "osascript",
            Backend::NotifySend(_) => "notify-send",
        }
    }

    /// D6.2's command line, with `title` and `body` as the last two arguments, verbatim.
    pub fn spec(&self, title: &str, body: &str) -> SpawnSpec {
        let (program, options): (PathBuf, &[&str]) = match self {
            Backend::Osascript => (PathBuf::from(OSASCRIPT), &OSASCRIPT_ARGS),
            Backend::NotifySend(path) => (path.clone(), &NOTIFY_SEND_ARGS),
        };
        let mut args: Vec<OsString> = options.iter().map(OsString::from).collect();
        args.extend([OsString::from(title), OsString::from(body)]);
        SpawnSpec {
            program,
            args,
            ..SpawnSpec::default()
        }
    }
}

/// One notification: its title and its body, which names accounts only as `{account}` does
/// (§13.5) and never carries a credential, an organization name or a whole email (D6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

/// Where notices wait for the notifier. Every holder sends from the scheduler's thread.
#[derive(Clone)]
pub struct NoticeQueue {
    tx: SyncSender<Notice>,
}

impl NoticeQueue {
    /// Queues `n` and never blocks (D4). A full queue drops it, and so does a notifier that
    /// has stopped.
    pub fn send(&self, n: Notice) {
        match self.tx.try_send(n) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(_)) => {
                tracing::debug!("the notification queue is full; a notification was dropped");
            }
        }
    }

    /// A queue whose notices a test reads from the receiver, with the notifier's capacity.
    #[cfg(test)]
    pub(crate) fn for_test() -> (NoticeQueue, Receiver<Notice>) {
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        (NoticeQueue { tx }, rx)
    }
}

/// The notifier thread (D4, D6.2). Dropping it stops it, as `stop` does.
pub struct Notifier {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Notifier {
    /// Starts the thread that delivers each queued notice through `spawner`. The thread logs
    /// through the dispatcher this call runs under.
    pub fn start(backend: Backend, spawner: Arc<dyn ProcessSpawner>) -> (Notifier, NoticeQueue) {
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let dispatch = tracing::dispatcher::get_default(|d| d.clone());
        let thread = thread::spawn(move || {
            tracing::dispatcher::with_default(&dispatch, || {
                deliver_until_stopped(&backend, spawner.as_ref(), &rx, &stopped)
            })
        });
        let notifier = Notifier {
            stop,
            thread: Some(thread),
        };
        (notifier, NoticeQueue { tx })
    }

    /// Stops the thread once the delivery in flight, if any, has ended. Nothing still queued
    /// is delivered (D6.2).
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for Notifier {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The notifier thread's loop: each notice in turn, until a stop is asked for or every queue
/// holder has gone.
fn deliver_until_stopped(
    backend: &Backend,
    spawner: &dyn ProcessSpawner,
    rx: &Receiver<Notice>,
    stop: &AtomicBool,
) {
    let mut warned = false;
    loop {
        match rx.recv_timeout(POLL) {
            Ok(_) if stop.load(Ordering::SeqCst) => return,
            Ok(notice) => deliver(backend, spawner, &notice, &mut warned),
            Err(RecvTimeoutError::Timeout) if stop.load(Ordering::SeqCst) => return,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// One delivery (D6.2). It has a cancel token of its own, never the daemon's, so a stop never
/// kills it half-way, and a 5 s bound. A failure is logged at WARN the first time and at DEBUG
/// after. The line names the backend and how it failed, never the text, which names accounts
/// (§14.2). The program's output is never read.
fn deliver(backend: &Backend, spawner: &dyn ProcessSpawner, notice: &Notice, warned: &mut bool) {
    let spec = backend.spec(&notice.title, &notice.body);
    let failure = match spawner.run_captured(&spec, DELIVERY_TIMEOUT, &Cancel::new()) {
        Captured::Exited { code: Some(0), .. } => None,
        Captured::Exited {
            code: Some(code), ..
        } => Some(format!("exit {code}")),
        Captured::Exited {
            signal: Some(signal),
            ..
        } => Some(format!("signal {signal}")),
        Captured::Exited { .. } => Some("no exit status".to_owned()),
        Captured::TimedOut => Some("timed out".to_owned()),
        Captured::Interrupted(signal) => Some(format!("interrupted by signal {signal}")),
        Captured::SpawnFailed(why) => Some(why),
    };
    match failure {
        None => tracing::debug!(backend = backend.name(), "notification sent"),
        Some(why) if !*warned => {
            *warned = true;
            tracing::warn!(
                backend = backend.name(),
                error = %why,
                "a notification could not be delivered; later failures log at DEBUG"
            );
        }
        Some(why) => tracing::debug!(
            backend = backend.name(),
            error = %why,
            "a notification could not be delivered"
        ),
    }
}

/// D6.4: `notify.enabled`, then the provider's `notify.events`.
fn wanted(settings: &Settings, kind: NotifyEvent) -> bool {
    settings.notify_enabled && settings.notify_events.contains(&kind)
}

/// The part of an email before `@`; a label without one is itself.
fn local_part(email: &str) -> &str {
    email.split_once('@').map_or(email, |(local, _)| local)
}

/// §13.5's `{account}` after its position: `2 · spare`, from the alias, or else the local part
/// of the email (of the label, for a provider whose logins have none).
fn account(position: u32, alias: Option<&str>, email_or_label: &str) -> String {
    let name = alias.unwrap_or_else(|| local_part(email_or_label));
    format!("{position} · {name}")
}

/// `account` for `row`. Where the row is gone, it uses `fallback`, the event's email, and else
/// the position alone.
fn named(row: Option<&AccountRow>, position: u32, fallback: Option<&str>) -> String {
    match (row, fallback) {
        (Some(r), _) => account(
            r.position,
            r.alias.as_deref(),
            r.email.as_deref().unwrap_or(&r.label),
        ),
        (None, Some(email)) => account(position, None, email),
        (None, None) => position.to_string(),
    }
}

/// A window as a notice names it: `7d at 93%`, rounded as `list` rounds it.
fn window_at(w: &Window) -> String {
    format!("{} at {}%", w.label, w.pct.round() as i64)
}

/// D6.1–D6.3: a notice for each `switch` the daemon made (never a dry run) and each
/// `account-quarantined`, when `notify.enabled` is on and the provider's `notify.events` names
/// the kind. The settings are read for each such event (`Settings::load`): the events are rare,
/// and a setting changed a moment ago then always counts.
pub struct NotifySink<'a> {
    engine: &'a Engine,
    queue: NoticeQueue,
}

impl<'a> NotifySink<'a> {
    pub fn new(engine: &'a Engine, queue: NoticeQueue) -> Self {
        NotifySink { engine, queue }
    }

    /// The notice `e` calls for, with its provider. `None` for any other event, or one the
    /// settings turn off.
    fn notice_for(&self, e: &DaemonEvent) -> Option<(ProviderId, Notice)> {
        let DaemonEvent::Auto(e) = e else {
            return None;
        };
        let (provider, kind) = match e {
            AutoEvent::Switch {
                provider,
                dry_run: false,
                ..
            } => (provider, NotifyEvent::Switch),
            AutoEvent::AccountQuarantined { provider, .. } => (provider, NotifyEvent::Quarantine),
            _ => return None,
        };
        let (settings, _) = Settings::load(self.engine.env(), provider);
        if !wanted(&settings, kind) {
            return None;
        }
        let body = match e {
            AutoEvent::Switch {
                trigger, from, to, ..
            } => {
                let left = self.row(provider, *from);
                let why = self.why_switched(*trigger, left.clone(), &settings.models);
                let to_row = self.row(provider, *to);
                format!(
                    "Switched {} → {} ({why})",
                    named(left.as_ref(), *from, None),
                    named(to_row.as_ref(), *to, None)
                )
            }
            AutoEvent::AccountQuarantined {
                number,
                email,
                reason,
                ..
            } => {
                let row = self.row(provider, *number);
                format!(
                    "{} needs a new login ({reason})",
                    named(row.as_ref(), *number, Some(email))
                )
            }
            _ => return None,
        };
        let notice = Notice {
            title: TITLE.to_owned(),
            body,
        };
        Some((provider.clone(), notice))
    }

    /// The account at `position`, as the store now holds it.
    fn row(&self, provider: &ProviderId, position: u32) -> Option<AccountRow> {
        self.engine
            .existing_store()
            .ok()??
            .find_by_position(provider, position)
            .ok()?
    }

    /// Why a switch was made, as its notice says it. Normally this is the binding window of the
    /// account it left (`5h at 92%`), from that account's decision-grade reading (§8.4). A
    /// failover names its trigger instead: the account it left was not at its limit. So does a
    /// switch whose account has no decision-grade reading.
    fn why_switched(&self, trigger: Trigger, left: Option<AccountRow>, models: &[String]) -> String {
        let binding = left
            .filter(|_| trigger != Trigger::Failover)
            .and_then(|row| {
                let usage = self.engine.account_view(row, false).usage;
                let grade = usage.decision_grade;
                let windows: Vec<Window> = usage
                    .windows
                    .filter(|_| grade)?
                    .into_iter()
                    .map(|(w, _)| w)
                    .collect();
                binding_window(&windows, models).map(window_at)
            });
        binding.unwrap_or_else(|| trigger.as_str().to_owned())
    }
}

impl DaemonSink for NotifySink<'_> {
    fn emit(&self, e: &DaemonEvent) {
        if let Some((_, notice)) = self.notice_for(e) {
            self.queue.send(notice);
        }
    }
}

/// D6.1 `limit`: the provider's managed live account, when its binding relevant window (the
/// one with the highest pct) is at or above `autoswitch.threshold`. `None` covers several
/// cases:
/// - notices are off, or the provider's `notify.events` leaves out `limit` (D6.4)
/// - there is no managed live account, or no decision-grade reading (Task 3's `live_reading`)
/// - the live login cannot be read (`EngineError::Unreadable`), logged at DEBUG: a tick
///   reports that itself, and the run goes on
/// - the window's reset has passed
/// - this window instance was notified before
///
/// D6.3's marker is the lease `notify:limit:<account id>:<window key>:<resets_at>`. It is
/// taken before the notice is returned, and expires 60 s after the reset (an hour from now when
/// the reset is unknown), so it holds across a restart.
pub fn limit_notice(
    engine: &Engine,
    provider: &ProviderId,
    settings: &Settings,
    now_ms: i64,
) -> Result<Option<Notice>, EngineError> {
    if !wanted(settings, NotifyEvent::Limit) {
        return Ok(None);
    }
    let live = match engine.live_reading(provider, &settings.models) {
        Ok(Some(live)) => live,
        Ok(None) => return Ok(None),
        Err(e @ EngineError::Unreadable(_)) => {
            // §14.2: the kind only; the error's text names a path.
            tracing::debug!(
                provider = %provider,
                kind = e.kind(),
                "the live login could not be read, so its limit was not judged"
            );
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    let Some(window) = binding_window(&live.windows, &settings.models) else {
        return Ok(None);
    };
    let now_s = now_ms.div_euclid(1000);
    if window.pct < settings.threshold || window.resets_at.is_some_and(|at| at <= now_s) {
        return Ok(None);
    }
    let Some(store) = engine.existing_store()? else {
        return Ok(None);
    };
    let instance = window
        .resets_at
        .map_or_else(|| "none".to_owned(), |at| at.to_string());
    let lease = format!("notify:limit:{}:{}:{instance}", live.account, window.key);
    let expires_ms = window
        .resets_at
        .map_or(now_ms + UNKNOWN_RESET_MS, |at| at * 1000 + AFTER_RESET_MS);
    let holder = uuid::Uuid::now_v7().to_string();
    if !store.take_lease(&lease, &holder, now_ms, expires_ms)? {
        return Ok(None);
    }
    let name = account(
        live.position,
        live.alias.as_deref(),
        live.email.as_deref().unwrap_or(&live.label),
    );
    let body = match window.resets_at {
        Some(at) => format!("{name}: {} (resets in {})", window_at(window), span(at - now_s)),
        None => format!("{name}: {}", window_at(window)),
    };
    Ok(Some(Notice {
        title: TITLE.to_owned(),
        body,
    }))
}

/// D6.1 `limit`, after a provider job's run (`ProviderJob::after_run_notices`). The limit is
/// judged, queued and announced to `sink`, unless the run switched or notices cannot be sent
/// (`queue` is `None`). A failure to judge is logged and never fails the run.
pub(crate) fn after_run(
    engine: &Engine,
    provider: &ProviderId,
    settings: &Settings,
    queue: Option<&NoticeQueue>,
    sink: &dyn DaemonSink,
    switched: bool,
) {
    let Some(queue) = queue else {
        return;
    };
    if switched {
        return;
    }
    match limit_notice(engine, provider, settings, engine.now_ms()) {
        Ok(Some(notice)) => {
            sink.notified(provider, &notice);
            queue.send(notice);
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(
            provider = %provider,
            kind = e.kind(),
            "could not judge whether the live account is at its limit"
        ),
    }
}
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-daemon notify::`, then
`cargo test -p tagteam-daemon --features test-hooks`; then `cargo fmt --all`,
`cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and
`cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings`.
Expected: PASS. Each `notify::tests` test passes, and every earlier test in the crate still
does: Tasks 8 to 10's provider-job tests run with `notices: None` (`job_ctx`), for which
`after_run` returns at once. No warning: every `testutil` addition is used by this task's tests,
and every crate-private item of `notify.rs` outside `cfg(test)` has a caller outside the tests
(`after_run` in `ProviderJob::after_run_notices`; `NotifySink::echoing` waits for Task 12, its
caller's task).

- [ ] **Step 5: Commit** (the controller commits; implementers do not)

`git commit -m "Send notifications from the daemon"`

---

### Task 12: Start-up, serving and `tagteam daemon`

**Depends on:** 9, 10, 11

D4.1's start-up (refusals, `daemon.lock`, the store probe), D4.6's stop, and the foreground
command D8.1 describes. `Daemon::start` holds everything that can refuse. `Started::serve` runs
the scheduler with one provider job per registered provider, the store and upgrade watches, and
a private settings job that reloads the engine's own settings and keeps the board's
`notifications` current. When the run ends, `serve` emits `stopping` and lets everything go in
D4.6's order (Decision 13). The CLI gains `Command::Daemon`, its human and JSON sinks, and
D8.1's exit codes.

**Readings of the spec this task commits to:**
- **Refusals.** Root is refused by `run_command`, before any engine exists. A readable run
  shell is refused twice: by `App::daemon`, so the CLI's message comes first, and by
  `Daemon::start`, so another caller of the library is refused too. An unreadable marker never
  reaches the daemon (`run_command`).
- **The Keychain check on a terminal** (Decision 14). "On a terminal" means
  `io.prompter.interactive()`: stdin and stderr are both terminals, the same test `auto`'s
  unlock offer depends on. `App::daemon` runs `ensure_unlocked` only then, before
  `Daemon::start`. A declined or failed unlock exits 1 with `keychain-locked`. Without a
  terminal nothing checks. `Command::Daemon` is not among `touches_keychain`, so `run_command`
  runs no check of its own either.
- **The `daemon.lock` wait.** It tries at once, then every 5 s. The first failed try says
  `a daemon already runs (pid N); waiting for it to stop`, once, through `notice`. When the
  holder's record is not there yet (it can be absent for an instant) or cannot be read, it says
  `a daemon already runs in another process; waiting for it to stop`. The CLI prints the
  notice on stderr as `note: …`, under `--json` too, so stdout stays pure JSONL. The wait is a
  cancellation point (§14.1): it looks at the token every 100 ms, and a signal ends `start` with
  `EngineError::Interrupted`. The CLI reports that as any interrupted command (128 + n, the
  `interrupted` error), since the daemon never started (Decision 15). D8.1's exit 0 after a
  signal applies once it serves. The wait is also logged at INFO once, since under a service
  stderr goes nowhere (D11).
- **The record is emptied before `daemon.lock` goes.** This is Decision 5's rule for engine
  locks, applied to the daemon's own lock. A private `DaemonLock` wraps the guard, and its
  `Drop` truncates the file through a second descriptor while the lock is still held. The
  record is therefore empty however a `Started` ends: served, dropped unserved, or failed after
  the lock was taken. Plan B's binary tests assert it after exit 0 and exit 75.
- **The program is stamped first** (Decision 17). The first statement of `App::daemon` is
  `ProgramStamp::take(daemon_cmd::program(PATH))`, before the run-shell refusal, the unlock
  offer and `Daemon::start`'s `daemon.lock` wait. The stamp rides in `DaemonOptions::program`,
  is kept in `Started`, and `serve` builds the upgrade watch from it, so the watch's first run
  compares with the file as it was when the process began. A tagteam installed while this
  daemon waited is an upgrade at that first run (exit 75), never this daemon's own baseline.
- **The settings mtime is read first, and the engine's settings after it.** `start` reads
  `Settings::mtime` before anything else, before the `daemon.lock` wait. The engine read its
  settings earlier, when the command built it, and the unlock offer between the two can wait on
  a person, so `start` then reads them again (`reload_settings`, Decision 3) for the provider
  the command read them for: `opts.provider`, else `engine.default_provider()`, as
  `command_settings` chose it. Every settings read the daemon serves with (the engine's, the
  board's `notifications`, each job's own `Settings::load` in `serve`) therefore follows the
  mtime. `serve` hands the mtime to `Scheduler::with_settings_mtime` (Decision 9), so a write
  that lands meanwhile, during a long wait for another daemon say, is a change at the
  scheduler's first check. The reload's warnings are dropped: each provider job emits its own
  read's as `config-warning` events at the start of its first run, before any mode work
  (Decision 8), and the provider the engine reloads for has a job of its own. A file corrupted
  before the mtime is read is therefore still reported, though no reload will come for it.
- **The store watch's baseline, then the store probe** (Decision 11). Once `daemon.lock` is held
  and the record written, and before any provider job is built, `StoreBaseline::take(engine)`
  forgets the engine's connection and reopens the store between two looks at the file (Task 10).
  A failure to open the store, other than a schema this build refuses, is a start failure,
  exit 1 (D8.1: "a store it cannot open"). The baseline is kept in `Started`, and `serve` builds
  the store watch from it, so its first run compares rather than records. Then
  `existing_store()`, on that fresh connection:
  - `Ok(None)` (no store yet): the provider jobs report `idle` until a store appears.
  - `Err(Store(UnsupportedSchema(v)))`: logged at ERROR once, and `schema_newer` is set
    (D4.1 step 4).
  - Any other error: a start failure, exit 1.
- **The notifier starts whenever `opts.backend` is `Some`,** whatever `notify.enabled` says at
  start. `notify.enabled` is read for each event (Task 11). A user who turns notifications on
  while the daemon runs is then heard within a second, as D4.2 promises for every setting. An
  idle notifier is one parked thread.
- **The private settings job** (`SettingsJob`, named `settings`) has two duties, and is added
  first, so both are done before any provider job hears of a change:
  - **The engine's own settings follow the file** (D3 change 1, Decision 3). On every reload
    (`Job::settings_changed`) it calls `engine.reload_settings` for the provider `start` read
    them for, so the collector and the views read what the file says now. It drops the
    warnings: each provider job already emits its own provider's warnings as `config-warning`
    events (Decision 8), and nothing else emits these.
  - **`status.notifications` follows the settings** (D5.4): at start, and after each reload,
    from `notify.enabled` and that provider's `notify.events` as the engine now holds them.

  It never writes a `JobStatus`, so it never shows in `status.jobs`.
- **The store watch wakes the provider jobs** (D4.3, D4.4): `serve` builds it as
  `StoreWatch::new(board, baseline).waking(scheduler.wake())` (Task 10), so after any `changed`
  every provider job re-evaluates its mode before the scheduler sleeps again, not only within
  60 s.
- **The watches come before the provider jobs** (Decision 11's invariant). `serve` adds the
  settings job, then the store watch, then the upgrade watch, then the provider jobs, all due
  now, and the scheduler runs a tie in the order added. The first pass therefore compares the
  store with start's baseline (forgetting a replaced store's connection), and checks the program
  and the schema, before any provider job ticks or collects: a store replaced between `start`
  and `serve` is never written by a tick or round, and a daemon that must restart (exit 75) does
  no provider work first. Running one watch pass by hand in `serve`, before building the
  scheduler, was rejected: it would duplicate the scheduler's handling of a `Ran::Stop`, for an
  ordering the scheduler's tie rule already gives.
- **The notifier's echo** (`NotifySink::echoing`, D8.1's line per notification) is added in
  this task, in `notify.rs`, beside `serve`, its one caller. Task 11 left it out, since a
  crate-private method with no caller fails that task's clippy run (`dead_code`).
- **The order on stopping** (D4.6): `stopping` reaches every sink first. Then the jobs drop,
  each `AutoEngine` emptying and releasing its engine lock (Decision 5). Then the notifier
  stops, dropping what is queued. Last, `daemon.lock` goes, record first. D4.6 removes the
  socket before the locks go, but plan B's server shuts down after `serve` returns, so a
  successor may already hold `daemon.lock`: plan B's shutdown must never remove a socket it did
  not bind (Decision 13).
- **Exit codes** (D8.1): `DaemonExit::Signal` exits 0 and `DaemonExit::Restart` exits 75. A
  start failure exits 1 through the CLI's usual failure path, and a usage error exits 2 through
  clap. The human sink prints D4.5's line on `stopping`: `tagteam was upgraded; start the
  daemon again`, or `a newer tagteam migrated the store; start the daemon again`.
- **Human lines** (D8.1) go through `auto`'s `HumanSink`. Every line starts with its time,
  and with the provider's name when the daemon runs several providers (§13.1), as `auto`'s
  lines do. That is why the mode line reads `mode collect (paused)`, not
  `claude-code: mode collect (paused)`.
  - Mode changes: `mode auto`, `mode collect`, `mode collect (paused)` when the provider's
    `daemon.autoswitch` is false, `mode idle`, `waiting for the engine lock held by pid N`,
    and `waiting for an upgrade: a newer tagteam migrated the store`.
  - A keychain backoff appends `; keychain locked, retrying at HH:MM:SS`.
  - `collect` rounds: `usage: #1 updated, #3 failed (over-budget)`.
  - Notifications: `notified: <body>`.
  - Start: `daemon started (pid N); Ctrl-C stops it`.
  - Stop: `stopped`, or D4.5's line.
  - `changed` prints nothing: it fires for every CLI command and would only be noise.
- **The program path** (D4.5, Task 10's `program_path`). It is built from `argv[0]`, the
  `PATH` that `Context::from_process` captured into `Env.vars`, and `current_exe()`, falling
  back to `argv[0]` when that fails, and stamped at once (Decision 17). The version is the
  CLI's `CARGO_PKG_VERSION`, which is the workspace version. The upgrade watch's period is
  `watch::UPGRADE_WATCH_MS` (Task 10).
- **Times on the board are epoch seconds** (Decision 12): `started_at` is the engine's clock
  divided by 1000.

**Files:**
- Create: `crates/tagteam-daemon/src/daemon.rs`
- Create: `crates/tagteam/src/daemon_cmd.rs`
- Create: `crates/tagteam/tests/daemon_cli.rs`
- Modify: `crates/tagteam-daemon/src/lib.rs` (`pub mod daemon;`)
- Modify: `crates/tagteam-daemon/src/notify.rs` (`NotifySink`'s `echo` field, `echoing`, and
  the echo in `emit`; a test)
- Modify: `crates/tagteam-engine/src/engine.rs` (`Engine::provider_ids`, after
  `pub fn provider(`)
- Modify: `crates/tagteam/src/cli.rs` (`Command::Daemon {}`, after `Auto { … }`)
- Modify: `crates/tagteam/src/app.rs`:
  - imports and the `App` fields `platform` and `keychain_counts`
  - `run`'s `stops_on_a_signal`, and `run_command`, which builds the new fields
  - `command_name`, `dispatch`, and the new `fn daemon`
  - `impl From<DaemonError> for Failure`
  - the late-notice test
- Modify: `crates/tagteam/src/auto.rs`:
  - `HumanSink` gains `pub(crate)` `line`, `plain`, `now_s` and `time_of`, plus a `cfg(test)`
    `with_clock`
  - `written` becomes `pub(crate)`
- Modify: `crates/tagteam/src/lib.rs` (`mod daemon_cmd;` after `mod config_cmd;`)
- Modify: `crates/tagteam/tests/common/mod.rs` (`Running::stderr`, `Running::wait_for_stderr`)
- Modify: `crates/tagteam/tests/run_shell_cli.rs` (`COMMANDS` gains `&["daemon"]`)
- Test: `crates/tagteam-daemon/src/daemon.rs`, `crates/tagteam-daemon/src/notify.rs`,
  `crates/tagteam-engine/src/engine.rs`, `crates/tagteam/src/daemon_cmd.rs`,
  `crates/tagteam/tests/app.rs`, `crates/tagteam/tests/daemon_cli.rs`

**Interfaces:**
- Consumes:
  - Task 1: `Engine::reload_settings(&self, provider) -> Vec<String>`, `Engine::settings()`;
    the existing `Engine::default_provider()`.
  - Task 2: `Engine::engine_holder(&self, provider) -> Read<ProcessStamp>` (tests).
  - Task 3:
    - `daemon_lock_path(env)`, `write_daemon_record(&FlockGuard, version)`
    - `read_daemon_record(path) -> Read<DaemonRecord>`
    - `Engine::spawner()`
  - Task 4: `tagteam_provider::keychain::ReadCounts`, and
    `Keychain::read_counts(&self) -> Option<Arc<ReadCounts>>`.
  - Task 5: `Settings::{daemon_autoswitch, notify_enabled, notify_events}`, `NotifyEvent`.
  - Task 6: `DaemonEvent`, `DaemonSink`, `Fanout`, `Mode`, `WaitingFor`, `Backoff`,
    `StopReason`, `daemon_event_json`.
  - Task 7:
    - `Scheduler::{new, with_settings_mtime, add, run}`, `Job`, `JobCx`, `Ran`
    - `Sleeper`, `ThreadSleeper`, `uniform_jitter`
    - `Settings::mtime`, as `run_loop` reads it
  - Task 8:
    - `StatusBoard::{new, snapshot, set_notifications}`, `DaemonInfo`, `NotifyInfo`
    - `ProviderJob::new`, `ProviderJobCtx { board, keychain_counts, schema_newer }`
  - Task 10:
    - `Scheduler::wake() -> Wake`
    - `StoreBaseline::take(engine) -> Result<StoreBaseline, EngineError>`,
      `StoreWatch::new(board, baseline).waking(wake)`
    - `ProgramStamp::{take(path), path()}`,
      `UpgradeWatch::new(program: ProgramStamp, schema_newer, board).with_period(ms)`,
      `UPGRADE_WATCH_MS`
    - `program_path(argv0, path_var, current_exe)`
  - Task 11:
    - `Backend`, `Notifier`, `NoticeQueue` (with the `cfg(test)` `for_test`), `NotifySink::new`
      and its private `notice_for`, `Notice`
    - `DaemonSink::notified`, `ProviderJobCtx::notices`
    - `notify::tests`' helpers `two_at_the_limit`, `quarantined` and `notice`
  - `crate::testutil`: `Fx` (with `spawner`, `with_run_shell`, `add_as`, `read_windows`,
    `settings`, `other`, `add_on`, `remove_store`), `Emitted` (with `announced`), `Slices`,
    `T0`, `cc`, `fake`, `windows`, `capture_logs`
- Produces (Interfaces, `daemon.rs`):
  - `DaemonOptions { provider, version, program: ProgramStamp, upgrade_watch_ms,
    keychain_counts, backend }`
  - `DaemonExit { Signal, Restart }`
  - `DaemonError { Engine(EngineError) }`
  - `Daemon::start(engine, opts, notice) -> Result<Started<'e>, DaemonError>`
  - `Started::board(&self) -> Arc<StatusBoard>`
  - `Started::serve(self, sink, sleeper, jitter) -> DaemonExit`
- Produces (Interfaces, `notify.rs`): the crate-private `NotifySink::echoing(self, to)`
- Produces, also in the Interfaces:
  - `Engine::provider_ids(&self) -> Vec<ProviderId>`
  - `tagteam daemon` (`Command::Daemon {}`), `App`'s `platform` and `keychain_counts`, and
    `crate::daemon_cmd`'s `DaemonHuman`, `DaemonJson`, `keychain_counts`, `program` and
    `EXIT_RESTART`
  - `Running::stderr` and `Running::wait_for_stderr`

- [ ] **Step 1: Write the failing tests (the crate)**

`crates/tagteam-daemon/Cargo.toml` needs no change: Task 6 gave the crate `thiserror` (for
`DaemonError`), and Task 10 gave it `rusqlite` as a dev-dependency (to bump a store's schema).

In `crates/tagteam-daemon/src/lib.rs`, add `pub mod daemon;` to the module list.

In `crates/tagteam-engine/src/engine.rs`'s `mod tests`, after `test_engine`:

```rust
    #[test]
    fn provider_ids_are_the_registered_providers_in_order() {
        // D4.3: the daemon runs one job per registered provider.
        let d = tempfile::tempdir().unwrap();
        let engine = Engine::new(EngineConfig {
            registry: ProviderRegistry::new()
                .with(Arc::new(tagteam_cc::ClaudeCode::new(
                    Arc::new(FakeKeychain::new()),
                    tagteam_cc::live::Platform::MacOs,
                )))
                .with(Arc::new(tagteam_fake::FakeAgent::new())),
            ..test_config(Env::for_test(d.path()))
        });
        assert_eq!(
            engine.provider_ids(),
            [
                ProviderId::new(tagteam_core::CLAUDE_CODE),
                ProviderId::new(tagteam_fake::FAKE_AGENT)
            ]
        );
        assert!(test_engine(Env::for_test(d.path())).provider_ids().is_empty());
    }
```

Create `crates/tagteam-daemon/src/daemon.rs` containing only this test module. Step 3 puts the
implementation above it.

```rust
#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use serde_json::json;
    use tagteam_core::AccountId;
    use tagteam_engine::auto::AutoEvent;
    use tagteam_engine::daemon::DaemonRecord;
    use tagteam_engine::settings::{NotifyEvent, config_path};
    use tagteam_provider::process::Captured;
    use tagteam_provider::profile::ProfileMarker;

    // `super::*` already brings `Instant`, `NotifyInfo`, `FlockGuard`, `Read`, `RunShell`,
    // `ProgramStamp` and the lock-record functions.
    use super::*;
    use crate::events::{Mode, WaitingFor};
    use crate::notify::Notice;
    use crate::testutil::{Emitted, Fx, Slices, T0, capture_logs, cc, fake, windows};

    const VERSION: &str = "20261014.2045.0";
    const SIGTERM: i32 = 15;

    /// What `tagteam daemon` passes, with a program file of the fixture's own, outside HOME,
    /// stamped now as `App::daemon` stamps it first (Decision 17). Unless a test replaces it,
    /// it never changes, so the upgrade watch never stops a run (D4.5).
    fn options(home: &Fx) -> DaemonOptions {
        let program = home.env.home.parent().unwrap().join("tagteam-bin");
        if !program.exists() {
            fs::write(&program, "a tagteam binary").unwrap();
        }
        DaemonOptions {
            provider: None,
            version: VERSION.into(),
            program: ProgramStamp::take(program),
            upgrade_watch_ms: 60_000,
            keychain_counts: Vec::new(),
            backend: None,
        }
    }

    /// `daemon.lock`'s content.
    fn record(home: &Fx) -> Vec<u8> {
        fs::read(daemon_lock_path(&home.env)).unwrap()
    }

    /// The scheduler's sleep on the fixture clock (`testutil::Slices`), with SIGTERM in the
    /// cancel token after slice `last`, counted from 1, as a `kill` would leave it.
    fn stop_after(home: &Fx, last: usize) -> Slices<'_> {
        stop_after_with(home, last, |_| {})
    }

    /// `stop_after`, running `on_slice` with each slice's number, from 1, before the signal.
    fn stop_after_with<'a>(
        home: &'a Fx,
        last: usize,
        on_slice: impl Fn(usize) + 'a,
    ) -> Slices<'a> {
        let cancel = home.engine.cancel().clone();
        Slices::new(&home.clock, move |n| {
            on_slice(n + 1);
            if n + 1 == last {
                cancel.request(SIGTERM);
            }
        })
    }

    /// `work` at position 1 at 20 % of its 7d window, and a (alias `alias_a`) at 2, live, at
    /// `a_long` percent.
    fn two_at(a_long: f64, alias_a: Option<&str>) -> Fx {
        let home = Fx::new();
        let w = home.add_as("w@b.example", "Acme", Some("work"));
        let a = home.add_as("a@b.example", "Acme", alias_a);
        home.read_windows(&w, &windows(10.0, 20.0), T0, T0 + 300);
        home.read_windows(&a, &windows(10.0, a_long), T0, T0 + 300);
        home
    }

    #[test]
    fn a_held_daemon_lock_is_announced_once_and_taken_over_when_it_is_released() {
        // D4.1 step 2: the holder is named by its record's pid, once, and the lock is tried
        // again until it is free. The new holder's record replaces the old one.
        let home = Fx::new();
        let path = daemon_lock_path(&home.env);
        let first = FlockGuard::try_lock(&path).unwrap().unwrap();
        write_daemon_record(&first, "first").unwrap();
        let release = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            drop(first);
        });
        let mut notices = Vec::new();
        let started = Daemon::start_with(
            &home.engine,
            options(&home),
            &mut |n| notices.push(n.to_owned()),
            Duration::from_millis(50),
        )
        .unwrap();
        release.join().unwrap();
        assert_eq!(
            notices,
            [format!(
                "a daemon already runs (pid {}); waiting for it to stop",
                std::process::id()
            )]
        );
        match read_daemon_record(&path) {
            Read::Present(DaemonRecord { pid, version, .. }) => {
                assert_eq!((pid, version.as_str()), (std::process::id(), VERSION));
            }
            other => panic!("no record: {other:?}"),
        }
        // A start that never serves still empties its record as it lets the lock go.
        drop(started);
        assert!(record(&home).is_empty());
        assert!(FlockGuard::try_lock(&path).unwrap().is_some());
    }

    #[test]
    fn a_program_replaced_while_waiting_for_daemon_lock_stops_at_the_first_upgrade_watch_run() {
        // D4.5, Decision 17: the stamp is the caller's, taken before the wait. A tagteam
        // installed while this daemon waits for another to stop (the other holder lets go only
        // after the install) is an upgrade at once: serving ends at the upgrade watch's first
        // run, before the scheduler ever sleeps and before any provider job runs (Decision 11's
        // order), and the daemon exits to be restarted (75).
        let home = Fx::new();
        let opts = options(&home);
        let program = opts.program.path().to_path_buf();
        let path = daemon_lock_path(&home.env);
        let first = FlockGuard::try_lock(&path).unwrap().unwrap();
        write_daemon_record(&first, "first").unwrap();
        let install = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            let next = program.with_file_name("tagteam-bin.new");
            fs::write(&next, "a newer tagteam binary").unwrap();
            fs::rename(&next, &program).unwrap();
            drop(first);
        });
        let started =
            Daemon::start_with(&home.engine, opts, &mut |_| {}, Duration::from_millis(50)).unwrap();
        install.join().unwrap();
        let sink = Emitted::new(&home.clock);
        let exit = started.serve(&sink, &Slices::none(&home.clock), &mut || 0.0);
        assert_eq!(exit, DaemonExit::Restart);
        assert_eq!(
            sink.events(),
            [(
                0,
                DaemonEvent::Stopping {
                    reason: StopReason::Upgrade
                }
            )],
            "no provider job ran, so no `mode` event came first"
        );
    }

    #[test]
    fn a_signal_while_waiting_for_daemon_lock_ends_start_interrupted() {
        // §14.1: the wait is a cancellation point. A lock whose record is not written yet is
        // held by "another process".
        let home = Fx::new();
        let _held = FlockGuard::try_lock(&daemon_lock_path(&home.env))
            .unwrap()
            .unwrap();
        let cancel = home.engine.cancel().clone();
        let signal = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            cancel.request(SIGTERM);
        });
        let mut notices = Vec::new();
        let err = Daemon::start_with(
            &home.engine,
            options(&home),
            &mut |n| notices.push(n.to_owned()),
            Duration::from_secs(5),
        )
        .err()
        .unwrap();
        signal.join().unwrap();
        let DaemonError::Engine(e) = err;
        assert_eq!(e.signal(), Some(SIGTERM));
        assert_eq!(
            notices,
            ["a daemon already runs in another process; waiting for it to stop"]
        );
    }

    #[test]
    fn inside_a_run_shell_start_refuses_before_daemon_lock_is_tried() {
        // D4.1 step 1, as `auto` refuses (§12.8).
        let profile = PathBuf::from("/nonexistent/profile");
        let home = Fx::with_run_shell(RunShell::Inside {
            profile: profile.clone(),
            marker: ProfileMarker {
                provider: cc(),
                account_id: AccountId::from_string("a"),
                config_dir: profile.display().to_string(),
                outer: json!({}),
            },
        });
        let err = Daemon::start(&home.engine, options(&home), &mut |n| panic!("{n}"))
            .err()
            .unwrap();
        let DaemonError::Engine(e) = err;
        assert!(matches!(e, EngineError::InsideRunShell), "{e}");
        assert!(!daemon_lock_path(&home.env).exists());
    }

    #[test]
    fn a_store_from_a_newer_tagteam_is_logged_once_and_every_provider_waits_for_the_schema() {
        // D4.1 step 4: not an exit, since a service would restart into the same refusal.
        let home = Fx::new();
        home.add_as("a@b.example", "Acme", None);
        rusqlite::Connection::open(home.env.data_dir().join("tagteam.db"))
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        let engine = home.other();
        let (started, lines) =
            capture_logs(|| Daemon::start(&engine, options(&home), &mut |_| {}).unwrap());
        let errors: Vec<&str> = lines
            .iter()
            .map(|l| l.trim_start())
            .filter(|l| l.starts_with("ERROR"))
            .collect();
        assert_eq!(errors.len(), 1, "{lines:#?}");
        assert!(
            errors[0].contains("newer than this build") && errors[0].contains("found=99"),
            "{}",
            errors[0]
        );
        let board = started.board();
        let exit = started.serve(&Emitted::new(&home.clock), &stop_after(&home, 2), &mut || 0.0);
        assert_eq!(exit, DaemonExit::Signal);
        let provider = board
            .snapshot()
            .providers
            .into_iter()
            .find(|p| p.provider == cc())
            .unwrap();
        assert_eq!(
            (provider.mode, provider.waiting_for),
            (Mode::Waiting, Some(WaitingFor::Schema))
        );
    }

    #[test]
    fn a_signal_ends_serving_with_stopping_last_and_every_lock_record_emptied() {
        // D4.6. Two switchable accounts are `auto`, so the provider job held the engine lock
        // and ticked. The run ends at the slice after SIGTERM, and `stopping` is the last
        // event. Once `serve` returns, both lock files are empty and free.
        let home = two_at(60.0, None);
        let started = Daemon::start(&home.engine, options(&home), &mut |n| panic!("{n}")).unwrap();
        let board = started.board();
        let sink = Emitted::new(&home.clock);
        let exit = started.serve(&sink, &stop_after(&home, 3), &mut || 0.0);
        assert_eq!(exit, DaemonExit::Signal);
        let events: Vec<DaemonEvent> = sink.events().into_iter().map(|(_, e)| e).collect();
        assert_eq!(
            events.last(),
            Some(&DaemonEvent::Stopping {
                reason: StopReason::Signal
            })
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, DaemonEvent::Stopping { .. }))
                .count(),
            1
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, DaemonEvent::Auto(AutoEvent::NoSwitch { .. }))),
            "it ticked: {events:?}"
        );
        assert!(
            !events.contains(&DaemonEvent::Changed),
            "nothing changed since start's baseline, and the tick's own writes are no change"
        );
        let provider = board
            .snapshot()
            .providers
            .into_iter()
            .find(|p| p.provider == cc())
            .unwrap();
        assert_eq!(provider.mode, Mode::Auto, "it held the engine lock");
        assert!(record(&home).is_empty());
        assert!(FlockGuard::try_lock(&daemon_lock_path(&home.env)).unwrap().is_some());
        assert!(matches!(home.engine.engine_holder(&cc()), Read::Absent));
        let engine_lock = home.env.data_dir().join("locks/autoswitch-claude-code.lock");
        assert!(FlockGuard::try_lock(&engine_lock).unwrap().is_some());
    }

    #[test]
    fn a_store_created_while_serving_wakes_the_provider_jobs_at_once() {
        // D4.3, D4.4: `serve` builds its store watch waking the provider jobs (Task 10). A
        // first `tagteam add` from another process creates the store. The watch's next run
        // reports `changed`, and Claude Code's job leaves `idle` before the scheduler sleeps
        // again, not at its next 60 s look.
        let home = Fx::new();
        let other = home.other();
        let started = Daemon::start(&home.engine, options(&home), &mut |n| panic!("{n}")).unwrap();
        let sink = Emitted::new(&home.clock);
        let slices = stop_after_with(&home, 3, |n| {
            if n == 2 {
                home.add_on(&other, "a@x.co");
            }
        });
        assert_eq!(started.serve(&sink, &slices, &mut || 0.0), DaemonExit::Signal);
        assert_eq!(
            sink.modes(&cc()),
            [(0, Mode::Idle, None, None), (2, Mode::Collect, None, None)]
        );
    }

    #[test]
    fn a_store_replaced_between_start_and_serving_is_reopened_before_any_provider_work() {
        // Decision 11: `start` takes the store watch's baseline on a fresh connection, after
        // `daemon.lock`. The store is deleted and another process creates a new one before
        // `serve` (plan B binds its socket in between). The watch's first run sees another
        // file: `changed`, once, at once, and the engine reads the new store from then on.
        // Its invariant: that check precedes all provider work. A provider job's first act in
        // a run is its mode evaluation, which is a `mode` event the first time, and a tick or
        // round only follows it. So `changed` being the run's first event means no provider
        // job evaluated, ticked or collected before the engine's connection to the deleted
        // store was gone: nothing was written into it.
        let home = Fx::new();
        home.add_as("a@b.example", "Acme", None);
        let started = Daemon::start(&home.engine, options(&home), &mut |n| panic!("{n}")).unwrap();
        home.remove_store();
        let other = home.other();
        home.add_on(&other, "c@x.co");
        let sink = Emitted::new(&home.clock);
        assert_eq!(
            started.serve(&sink, &stop_after(&home, 1), &mut || 0.0),
            DaemonExit::Signal
        );
        let seen: Vec<(i64, DaemonEvent)> = sink.events();
        assert_eq!(seen.first(), Some(&(0, DaemonEvent::Changed)), "{seen:?}");
        let changed: Vec<i64> = seen
            .iter()
            .filter(|(_, e)| *e == DaemonEvent::Changed)
            .map(|(at, _)| *at)
            .collect();
        assert_eq!(changed, [0]);
        assert_eq!(
            sink.modes(&cc()),
            [(0, Mode::Collect, None, None)],
            "one evaluation, of the new store's one account"
        );
        let emails: Vec<Option<String>> = home
            .engine
            .existing_store()
            .unwrap()
            .unwrap()
            .accounts(&cc())
            .unwrap()
            .into_iter()
            .map(|r| r.email)
            .collect();
        assert_eq!(emails, [Some("c@x.co".to_owned())]);
    }

    #[test]
    fn settings_written_after_the_engine_was_built_are_the_ones_start_serves_with() {
        // Decision 9, Decision 3: the engine read `config.toml` when the command built it,
        // before an unlock offer that can wait on a person. `start` reads the mtime, then the
        // engine's settings again, for the provider the command acts on, so a write in between
        // is neither lost nor a change the scheduler never sees.
        let home = Fx::new();
        home.settings(
            "[notify]\nenabled = false\n\n[provider.fake-agent.notify]\nevents = [\"limit\"]\n",
            1,
        );
        let started = Daemon::start(&home.engine, options(&home), &mut |n| panic!("{n}")).unwrap();
        let notifications = started.board().snapshot().notifications;
        assert!(!notifications.enabled);
        assert_eq!(notifications.events, NotifyEvent::ALL, "the default provider's");
        assert!(!home.engine.settings().notify_enabled);
        drop(started);
        let opts = DaemonOptions {
            provider: Some(fake()),
            ..options(&home)
        };
        let started = Daemon::start(&home.engine, opts, &mut |n| panic!("{n}")).unwrap();
        assert_eq!(
            started.board().snapshot().notifications.events,
            [NotifyEvent::Limit],
            "--provider's own table first (§6.4)"
        );
    }

    #[test]
    fn a_settings_file_corrupted_before_start_is_warned_before_any_provider_work() {
        // Decision 8. The file pauses switching and is valid when the engine is built, then is
        // corrupted before `Daemon::start` (during an unlock offer, say). Start's mtime is read
        // after the write, so no reload will ever report it. Each provider job's first run
        // emits its first read's warning before its mode is evaluated, and so before any tick
        // or round; the defaults then switch again (`auto`).
        let home = two_at(60.0, None);
        home.settings("[daemon]\nautoswitch = false\n", 1);
        home.settings("[daemon\nautoswitch = false\n", 2);
        let started = Daemon::start(&home.engine, options(&home), &mut |n| panic!("{n}")).unwrap();
        let sink = Emitted::new(&home.clock);
        assert_eq!(
            started.serve(&sink, &stop_after(&home, 1), &mut || 0.0),
            DaemonExit::Signal
        );
        let seen: Vec<DaemonEvent> = sink.events().into_iter().map(|(_, e)| e).collect();
        let first = |want: fn(&DaemonEvent) -> bool| {
            seen.iter()
                .position(want)
                .unwrap_or_else(|| panic!("{seen:?}"))
        };
        let warned = first(|e| {
            matches!(e, DaemonEvent::Auto(AutoEvent::ConfigWarning { provider, .. }) if *provider == cc())
        });
        let evaluated = first(|e| matches!(e, DaemonEvent::Mode { provider, .. } if *provider == cc()));
        let ticked = first(|e| {
            matches!(
                e,
                DaemonEvent::Auto(AutoEvent::Poll { .. } | AutoEvent::NoSwitch { .. })
            )
        });
        assert!(warned < evaluated && evaluated < ticked, "{seen:?}");
        assert_eq!(
            seen[warned],
            DaemonEvent::Auto(AutoEvent::ConfigWarning {
                provider: cc(),
                message: format!(
                    "{}: the settings file is not valid TOML; using the defaults",
                    config_path(&home.env).display()
                ),
            })
        );
        assert_eq!(sink.modes(&cc()), [(0, Mode::Auto, None, None)]);
    }

    #[test]
    fn a_switch_while_serving_is_announced_and_delivered_through_the_notifier() {
        // D6.1, D6.2, D8.1. The daemon's own switch is queued for the notifier, which runs
        // the backend through the engine's spawner. It is also announced to the caller's sink.
        let home = two_at(95.0, Some("spare"));
        home.spawner.push(Captured::Exited {
            code: Some(0),
            signal: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
        });
        let program = PathBuf::from("/usr/bin/notify-send");
        let opts = DaemonOptions {
            backend: Some(Backend::NotifySend(program.clone())),
            ..options(&home)
        };
        let started = Daemon::start(&home.engine, opts, &mut |n| panic!("{n}")).unwrap();
        let sink = Emitted::new(&home.clock);
        let delivered = || {
            home.spawner
                .specs()
                .into_iter()
                .filter(|s| s.program == program)
                .collect::<Vec<_>>()
        };
        let slices = stop_after_with(&home, 2, |n| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while n == 2 && delivered().is_empty() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
        });
        assert_eq!(started.serve(&sink, &slices, &mut || 0.0), DaemonExit::Signal);
        let body = "Switched 2 · spare → 1 · work (7d at 95%)";
        assert_eq!(
            sink.announced(),
            [(
                cc(),
                Notice {
                    title: "tagteam".into(),
                    body: body.into()
                }
            )]
        );
        let specs = delivered();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].args.last().unwrap(), body);
    }

    #[test]
    fn the_board_s_notifications_and_the_engine_s_settings_follow_the_file_while_serving() {
        // D5.4 through D4.2: `notify.enabled` set false while the daemon runs shows on the
        // board within a slice or two. The settings job reloads the engine's own settings too
        // (Decision 3), which the collector and the views read.
        let home = Fx::new();
        let opts = DaemonOptions {
            backend: Some(Backend::Osascript),
            ..options(&home)
        };
        let started = Daemon::start(&home.engine, opts, &mut |n| panic!("{n}")).unwrap();
        let board = started.board();
        assert_eq!(
            board.snapshot().notifications,
            NotifyInfo {
                enabled: true,
                backend: Some("osascript"),
                events: vec![NotifyEvent::Switch, NotifyEvent::Quarantine, NotifyEvent::Limit],
            }
        );
        let slices = stop_after_with(&home, 3, |n| {
            if n == 1 {
                home.settings("[notify]\nenabled = false\n", 1);
            }
        });
        started.serve(&Emitted::new(&home.clock), &slices, &mut || 0.0);
        assert!(!board.snapshot().notifications.enabled);
        assert!(!home.engine.settings().notify_enabled, "the engine's settings were read again");
    }
}
```

In `crates/tagteam-daemon/src/notify.rs`'s `mod tests` (Task 11), after
`fanout_passes_each_announced_notice_to_every_sink`, add the test of the echo `serve` wires up.
It uses that module's `two_at_the_limit`, `quarantined` and `notice`:

```rust
    #[test]
    fn an_echoing_sink_announces_each_notice_it_queues() {
        // D8.1: `tagteam daemon` prints a line per notification, so `serve` builds its
        // `NotifySink` echoing to the caller's sink, inside a `Fanout`. The echo is told of the
        // notice, and is never handed the event by this sink.
        let home = two_at_the_limit();
        let (queue, rx) = NoticeQueue::for_test();
        let echo = Emitted::new(&home.clock);
        let sink = NotifySink::new(&home.engine, queue).echoing(&echo);
        Fanout(vec![&sink]).emit(&DaemonEvent::Auto(quarantined(1, "w@b.example")));
        let queued: Vec<Notice> = rx.try_iter().collect();
        assert_eq!(queued, [notice("1 · work needs a new login (invalid_grant)")]);
        assert_eq!(echo.announced(), [(cc(), queued[0].clone())]);
        assert!(
            echo.events().is_empty(),
            "the echo is told of the notice, never handed the event"
        );
    }
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-daemon -- daemon:: notify::` and
`cargo test -p tagteam-engine --lib provider_ids`
Expected: FAIL to compile. `Daemon`, `DaemonOptions`, `DaemonExit`, `DaemonError`, `Started`,
`NotifySink::echoing` and `Engine::provider_ids` do not exist.

- [ ] **Step 3: Implement (the crate)**

In `crates/tagteam-engine/src/engine.rs`, after `pub fn provider(&self, id: &ProviderId)`:

```rust
    /// Every provider this build registers, in registration order: the daemon runs one job
    /// per provider (D4.3).
    pub fn provider_ids(&self) -> Vec<ProviderId> {
        self.registry.all().iter().map(|p| p.id()).collect()
    }
```

Put the implementation above the test module in `crates/tagteam-daemon/src/daemon.rs`:

```rust
//! D4.1 and D4.6: the daemon's start-up and its run. `Daemon::start` covers everything that
//! can refuse: the run shell, `daemon.lock`, and a store this build cannot open. `Started::serve`
//! runs the scheduler until a signal or a newer tagteam ends it, then lets everything go in
//! D4.6's order (Decision 13). Plan B binds the socket between the two.

use std::fs::OpenOptions;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use tagteam_core::ProviderId;
use tagteam_engine::daemon::{daemon_lock_path, read_daemon_record, write_daemon_record};
use tagteam_engine::settings::Settings;
use tagteam_engine::store::{SCHEMA_VERSION, StoreError};
use tagteam_engine::{Engine, EngineError};
use tagteam_provider::keychain::ReadCounts;
use tagteam_provider::{FlockGuard, Read, RunShell};

use crate::events::{DaemonEvent, DaemonSink, Fanout, StopReason};
use crate::notify::{Backend, NoticeQueue, Notifier, NotifySink};
use crate::provider_job::{ProviderJob, ProviderJobCtx};
use crate::scheduler::{Job, JobCx, Ran, Scheduler, Sleeper};
use crate::status::{DaemonInfo, NotifyInfo, StatusBoard};
use crate::watch::{ProgramStamp, StoreBaseline, StoreWatch, UpgradeWatch};

/// D4.1 step 2: how often a daemon that finds `daemon.lock` held tries it again.
const LOCK_RETRY: Duration = Duration::from_secs(5);
/// How often that wait looks at the cancel token (§14.1).
const WAIT_SLICE: Duration = Duration::from_millis(100);
/// How often the settings job republishes the board's notifications when no reload told it to
/// sooner.
const SETTINGS_JOB_MS: i64 = 3_600_000;

/// What the caller decides, from its command line and environment (D8.1).
pub struct DaemonOptions {
    /// `--provider`: only this provider's job (D8). Otherwise every registered provider.
    pub provider: Option<ProviderId>,
    /// This build's version, recorded in `daemon.lock` (D4.1) and shown by `status` (D5.4).
    pub version: String,
    /// The path the upgrade watch follows, and the file it resolved to when the caller stamped
    /// it, first thing, before anything that can wait (D4.5, Decision 17).
    pub program: ProgramStamp,
    /// The upgrade watch's period: 60 000 ms, and shorter only in plan B's tests.
    pub upgrade_watch_ms: i64,
    /// Decision 7: the read counts the keychain backoff compares (D4.1); empty on Linux.
    pub keychain_counts: Vec<Arc<ReadCounts>>,
    /// Where notices are shown; `None` when there is no backend (D6.2).
    pub backend: Option<Backend>,
}

/// How a run ended (D8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonExit {
    /// A signal stopped it: exit 0 (D4.6).
    Signal,
    /// A newer binary, or a store a newer tagteam migrated, must take over: exit 75 (D4.5).
    Restart,
}

/// Why the daemon could not start (D8.1's exit 1).
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error(transparent)]
    Engine(#[from] EngineError),
}

pub struct Daemon;

impl Daemon {
    /// D4.1 steps 1, 2 and 4:
    /// - read `config.toml`'s mtime, before the settings the daemon serves with (Decision 9)
    /// - refuse inside a run shell
    /// - read the engine's settings again, for the provider the command acts on (Decision 3)
    /// - hold `daemon.lock`, waiting while another daemon holds it, and say so once through
    ///   `notice`
    /// - record this daemon in the lock
    /// - take the store watch's baseline on a fresh connection, and probe the store
    ///   (Decision 11)
    /// - build the status board, and start the notifier when there is a backend
    ///
    /// Root is refused before any engine exists (the CLI's `run_command`). The Keychain check
    /// is the CLI's (Decision 14), and so is the program's stamp in `opts`, taken before
    /// anything that can wait (Decision 17).
    pub fn start<'e>(
        engine: &'e Engine,
        opts: DaemonOptions,
        notice: &mut dyn FnMut(&str),
    ) -> Result<Started<'e>, DaemonError> {
        Self::start_with(engine, opts, notice, LOCK_RETRY)
    }

    /// `start`, with the lock's retry period a parameter so the tests need not wait 5 s.
    fn start_with<'e>(
        engine: &'e Engine,
        opts: DaemonOptions,
        notice: &mut dyn FnMut(&str),
        retry: Duration,
    ) -> Result<Started<'e>, DaemonError> {
        // Before the `daemon.lock` wait and every settings read the daemon serves with, so a
        // write landing meanwhile is a change at the scheduler's first check (D4.2).
        let settings_mtime = Settings::mtime(engine.env());
        if matches!(engine.run_shell(), RunShell::Inside { .. }) {
            return Err(EngineError::InsideRunShell.into());
        }
        // Decision 3 and 9: the engine read its settings when the command built it, before the
        // mtime and before any unlock offer. Read them again, for the provider the command read
        // them for. The warnings are dropped: each provider job emits its own read's at its
        // first run (Decision 8), and that provider's job is among them.
        let settings_provider = opts
            .provider
            .clone()
            .unwrap_or_else(|| engine.default_provider().clone());
        let _warnings = engine.reload_settings(&settings_provider);
        let lock = DaemonLock(wait_for_lock(engine, notice, retry)?);
        write_daemon_record(&lock.0, &opts.version)?;
        // Decision 11: the store watch's baseline, on a fresh connection, before any provider
        // job is built. A failure to open the store, other than a schema this build refuses, is
        // a start failure (D8.1).
        let store = StoreBaseline::take(engine)?;
        let schema_newer = match engine.existing_store() {
            Ok(_) => false,
            Err(EngineError::Store(StoreError::UnsupportedSchema(found))) => {
                tracing::error!(
                    found,
                    supported = SCHEMA_VERSION,
                    "the store's schema is newer than this build: every provider waits until tagteam is upgraded"
                );
                true
            }
            Err(e) => return Err(e.into()),
        };
        let settings = engine.settings();
        let board = StatusBoard::new(
            DaemonInfo {
                pid: std::process::id(),
                version: opts.version.clone(),
                started_at: engine.now_ms().div_euclid(1000),
                socket: None,
                log: engine.env().log_file(),
            },
            NotifyInfo {
                enabled: settings.notify_enabled,
                backend: opts.backend.as_ref().map(Backend::name),
                events: settings.notify_events.clone(),
            },
        );
        let notifier = opts
            .backend
            .clone()
            .map(|backend| Notifier::start(backend, engine.spawner()));
        tracing::info!(
            pid = std::process::id(),
            version = %opts.version,
            "the daemon started"
        );
        Ok(Started {
            engine,
            lock,
            board,
            opts,
            schema_newer,
            settings_mtime,
            settings_provider,
            store,
            notifier,
        })
    }
}

/// D4.1 step 2: `daemon.lock`, tried at once and then every `retry` while another process
/// holds it. The first failed try is said once, naming the holder by its record's pid. The
/// wait is a cancellation point (§14.1).
fn wait_for_lock(
    engine: &Engine,
    notice: &mut dyn FnMut(&str),
    retry: Duration,
) -> Result<FlockGuard, EngineError> {
    let path = daemon_lock_path(engine.env());
    let mut told = false;
    loop {
        if let Some(signal) = engine.cancel().requested() {
            return Err(EngineError::Interrupted(signal));
        }
        if let Some(lock) = FlockGuard::try_lock(&path)? {
            return Ok(lock);
        }
        if !told {
            told = true;
            let holder = match read_daemon_record(&path) {
                Read::Present(r) => Some(r.pid),
                Read::Absent | Read::Unreadable(_) => None,
            };
            tracing::info!(
                holder_pid = holder,
                "another daemon holds daemon.lock; waiting for it to stop"
            );
            notice(&match holder {
                Some(pid) => format!("a daemon already runs (pid {pid}); waiting for it to stop"),
                None => "a daemon already runs in another process; waiting for it to stop".into(),
            });
        }
        let until = Instant::now() + retry;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            if let Some(signal) = engine.cancel().requested() {
                return Err(EngineError::Interrupted(signal));
            }
            thread::sleep(left.min(WAIT_SLICE));
        }
    }
}

/// `daemon.lock`, held for the daemon's life. Its record is emptied before the lock goes, as
/// Decision 5 does for engine locks, so `doctor` never names a daemon that stopped (D10). An
/// I/O error is logged and otherwise ignored.
struct DaemonLock(FlockGuard);

impl Drop for DaemonLock {
    fn drop(&mut self) {
        let emptied = OpenOptions::new()
            .write(true)
            .open(self.0.path())
            .and_then(|file| file.set_len(0));
        if let Err(e) = emptied {
            tracing::warn!(kind = %e.kind(), "could not empty the daemon lock's record");
        }
    }
}

/// A daemon that holds `daemon.lock` and has not run yet.
pub struct Started<'e> {
    engine: &'e Engine,
    lock: DaemonLock,
    board: Arc<StatusBoard>,
    opts: DaemonOptions,
    /// D4.1 step 4: the store's schema was already newer than this build at start.
    schema_newer: bool,
    /// `config.toml`'s mtime, read before the settings the daemon serves with (Decision 9).
    settings_mtime: Option<SystemTime>,
    /// The provider whose tables the engine's own settings are read for (Decision 3).
    settings_provider: ProviderId,
    /// What the store watch's first run compares with (Decision 11).
    store: StoreBaseline,
    notifier: Option<(Notifier, NoticeQueue)>,
}

impl<'e> Started<'e> {
    /// The snapshot plan B's server answers `status` from (Decision 12).
    pub fn board(&self) -> Arc<StatusBoard> {
        Arc::clone(&self.board)
    }

    /// Runs the scheduler (D4.2) with these jobs, added in this order:
    /// - the settings job: the engine's own settings and the board's notifications (Decision 3)
    /// - the store watch, which wakes the provider jobs after `changed` (D4.4), and the upgrade
    ///   watch (D4.5)
    /// - one provider job per provider (D4.3)
    ///
    /// Every job is first due now, and the scheduler runs a tie in the order the jobs were
    /// added (Task 7), so the first pass checks the store and the program before any provider
    /// job ticks or collects: a store replaced since `start` is forgotten, and a newer binary
    /// or schema stops the daemon, before any provider work touches the store.
    ///
    /// Every event goes to `sink`, and to the notifier's `NotifySink`, which announces each
    /// notice back to `sink`. When the run ends, `stopping` is emitted. Then the jobs drop
    /// (their `AutoEngine`s empty and release the engine locks), the notifier stops, and
    /// `daemon.lock` goes, record first (D4.6).
    pub fn serve(
        self,
        sink: &dyn DaemonSink,
        sleeper: &dyn Sleeper,
        jitter: &mut dyn FnMut() -> f64,
    ) -> DaemonExit {
        let Started {
            engine,
            lock,
            board,
            opts,
            schema_newer,
            settings_mtime,
            settings_provider,
            store: baseline,
            notifier,
        } = self;
        let queue = notifier.as_ref().map(|(_, q)| q.clone());
        let providers = match &opts.provider {
            Some(p) => vec![p.clone()],
            None => engine.provider_ids(),
        };
        let now = engine.now_ms();
        let mut scheduler = Scheduler::new().with_settings_mtime(settings_mtime);
        // First, so the engine's own settings are read again before any provider job hears of
        // a change.
        let settings = SettingsJob {
            board: Arc::clone(&board),
            backend: opts.backend.as_ref().map(Backend::name),
            provider: settings_provider,
        };
        scheduler.add(Box::new(settings), now);
        // The watches before every provider job (Decision 11's invariant): a tie runs in the
        // order added, so the first pass's store and program checks precede any provider work.
        // The store watch's first run compares with start's baseline, and after any `changed`
        // every provider job re-evaluates its mode at once (D4.3).
        let store = StoreWatch::new(Arc::clone(&board), baseline).waking(scheduler.wake());
        scheduler.add(Box::new(store), now);
        // Every run compares with the caller's stamp, taken before start (Decision 17).
        let upgrade = UpgradeWatch::new(opts.program.clone(), schema_newer, Arc::clone(&board))
            .with_period(opts.upgrade_watch_ms);
        scheduler.add(Box::new(upgrade), now);
        for provider in providers {
            let ctx = ProviderJobCtx {
                board: Arc::clone(&board),
                notices: queue.clone(),
                keychain_counts: opts.keychain_counts.clone(),
                schema_newer,
            };
            scheduler.add(Box::new(ProviderJob::new(engine, provider, ctx)), now);
        }
        let notify = queue.map(|q| NotifySink::new(engine, q).echoing(sink));
        let mut sinks: Vec<&dyn DaemonSink> = vec![sink];
        if let Some(n) = &notify {
            sinks.push(n);
        }
        let all = Fanout(sinks);
        let reason = scheduler.run(
            &mut JobCx {
                engine,
                sink: &all,
                jitter,
            },
            sleeper,
        );
        all.emit(&DaemonEvent::Stopping { reason });
        tracing::info!(reason = reason.as_str(), "the daemon stopped");
        drop(scheduler);
        drop(all);
        drop(notify);
        if let Some((notifier, queue)) = notifier {
            drop(queue);
            notifier.stop();
        }
        drop(lock);
        match reason {
            StopReason::Signal => DaemonExit::Signal,
            StopReason::Upgrade | StopReason::Schema => DaemonExit::Restart,
        }
    }
}

/// The daemon's settings job, with two duties on every settings reload (D4.2):
/// - the engine's own settings are read again for `provider`, the provider `start` read them
///   for (D3 change 1, Decision 3), so the collector and the views read what the file says now
/// - the board's `notifications` follow `notify.enabled` and that provider's `notify.events`
///   (D5.4); it also sets them at start
///
/// The reload's warnings are dropped: each provider job emits its own provider's as
/// `config-warning` events (Decision 8), so a second emitter would print every warning twice.
/// It writes no `JobStatus`, so it never shows in `status.jobs`.
struct SettingsJob {
    board: Arc<StatusBoard>,
    backend: Option<&'static str>,
    /// `--provider`'s, else the default provider: whose tables the command read (Decision 3).
    provider: ProviderId,
}

impl SettingsJob {
    /// The board's `notifications`, from the engine's settings as they now stand.
    fn publish(&self, engine: &Engine) {
        let settings = engine.settings();
        self.board.set_notifications(NotifyInfo {
            enabled: settings.notify_enabled,
            backend: self.backend,
            events: settings.notify_events.clone(),
        });
    }
}

impl Job for SettingsJob {
    fn name(&self) -> String {
        "settings".into()
    }

    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
        self.publish(cx.engine);
        Ran::After(SETTINGS_JOB_MS)
    }

    fn settings_changed(&mut self, cx: &mut JobCx<'_>) {
        let _warnings = cx.engine.reload_settings(&self.provider);
        self.publish(cx.engine);
    }
}
```

In `crates/tagteam-daemon/src/notify.rs`, give `NotifySink` its echo (Task 11 left it out, since
nothing called it before `serve`). Replace the struct and `new`:

```rust
pub struct NotifySink<'a> {
    engine: &'a Engine,
    queue: NoticeQueue,
}

impl<'a> NotifySink<'a> {
    pub fn new(engine: &'a Engine, queue: NoticeQueue) -> Self {
        NotifySink { engine, queue }
    }
```

with:

```rust
pub struct NotifySink<'a> {
    engine: &'a Engine,
    queue: NoticeQueue,
    /// Where each queued notice is also announced (`DaemonSink::notified`): `tagteam daemon`'s
    /// sink, for its line per notification (D8.1).
    echo: Option<&'a dyn DaemonSink>,
}

impl<'a> NotifySink<'a> {
    pub fn new(engine: &'a Engine, queue: NoticeQueue) -> Self {
        NotifySink {
            engine,
            queue,
            echo: None,
        }
    }

    /// Announces each notice it queues to `to` as well. `Started::serve` echoes to its caller's
    /// sink.
    pub(crate) fn echoing(mut self, to: &'a dyn DaemonSink) -> Self {
        self.echo = Some(to);
        self
    }
```

and replace `impl DaemonSink for NotifySink<'_>`:

```rust
impl DaemonSink for NotifySink<'_> {
    fn emit(&self, e: &DaemonEvent) {
        if let Some((_, notice)) = self.notice_for(e) {
            self.queue.send(notice);
        }
    }
}
```

with:

```rust
impl DaemonSink for NotifySink<'_> {
    fn emit(&self, e: &DaemonEvent) {
        if let Some((provider, notice)) = self.notice_for(e) {
            if let Some(echo) = self.echo {
                echo.notified(&provider, &notice);
            }
            self.queue.send(notice);
        }
    }
}
```

- [ ] **Step 4: Run the tests and see them pass (the crate)**

Run: `cargo test -p tagteam-daemon -- daemon:: notify::`, then
`cargo test -p tagteam-daemon --features test-hooks` and
`cargo test -p tagteam-engine --lib provider_ids`
Expected: PASS, the twelve `daemon::tests`, `notify::tests::an_echoing_sink_announces_each_notice_it_queues`
and `provider_ids_are_the_registered_providers_in_order` among them. Every earlier test still
passes.

- [ ] **Step 5: Write the failing tests (the CLI)**

In `crates/tagteam/tests/common/mod.rs`, inside `impl Running`, after `pub fn stdout`:

```rust
    /// Everything it has printed on stderr so far.
    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.err.lock().unwrap()).into_owned()
    }

    /// `wait_for`, on its stderr.
    pub fn wait_for_stderr(&mut self, within: Duration, what: &str, ready: impl Fn(&str) -> bool) {
        let deadline = Instant::now() + within;
        while !ready(&self.stderr()) {
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!("tagteam exited ({status}) before {what}");
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                panic!("tagteam never got to {what}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
```

In `crates/tagteam/tests/run_shell_cli.rs`, add `&["daemon"],` to `COMMANDS` after
`&["doctor"],`. An unreadable marker refuses it before any lock or wait, like every other
command.

In `crates/tagteam/src/app.rs`'s `the_late_notice_names_each_command_as_it_is_typed`, add
`&["daemon"],` after `&["doctor"],`.

In `crates/tagteam/src/lib.rs`, after `mod config_cmd;`, add `mod daemon_cmd;`. Without it the
module's tests would not be compiled at all, and the next step would pass vacuously.

Create `crates/tagteam/src/daemon_cmd.rs` containing only this test module. Step 7 puts the
implementation above it.

```rust
#[cfg(test)]
mod tests {
    use std::fs;

    use tagteam_core::CLAUDE_CODE;
    use tagteam_daemon::events::Backoff;
    use tagteam_engine::auto::AutoEvent;
    use tagteam_provider::FakeKeychain;

    use super::*;

    const T0: i64 = 1_790_000_000;

    fn cc() -> ProviderId {
        ProviderId::new(CLAUDE_CODE)
    }

    fn mode(
        mode: Mode,
        waiting_for: Option<WaitingFor>,
        holder_pid: Option<u32>,
        backoff: Option<Backoff>,
    ) -> DaemonEvent {
        DaemonEvent::Mode {
            provider: cc(),
            mode,
            waiting_for,
            holder_pid,
            backoff,
        }
    }

    /// What the human sink prints for `events`, on a fixed clock whose times read as seconds
    /// after `T0`, with `config` as `config.toml`.
    fn human(config: &str, f: impl FnOnce(&DaemonHuman<'_>)) -> (String, String) {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        fs::create_dir_all(env.config_dir()).unwrap();
        fs::write(env.config_dir().join("config.toml"), config).unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let now = || T0 * 1000;
        let names = |p: &ProviderId| format!("<{p}>");
        {
            let auto = HumanSink::new(&mut out, &mut err, &now, &names, false, false)
                .with_clock(|s| format!("t+{}", s - T0));
            f(&DaemonHuman::new(auto, &env));
        }
        (String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn the_daemon_s_own_lines_follow_auto_s() {
        // D8.1: a line per start and stop, mode change, `collect` round and notification, each
        // starting as `auto`'s lines do. `changed` prints nothing.
        let (out, err) = human("[daemon]\nautoswitch = false\n", |sink| {
            sink.started(4242);
            sink.emit(&mode(Mode::Collect, None, None, None));
            sink.emit(&mode(Mode::Waiting, Some(WaitingFor::EngineLock), Some(4243), None));
            sink.emit(&mode(Mode::Waiting, Some(WaitingFor::EngineLock), None, None));
            sink.emit(&mode(Mode::Auto, None, None, Some(Backoff { until: T0 + 60 })));
            sink.emit(&mode(Mode::Waiting, Some(WaitingFor::Schema), None, None));
            sink.emit(&mode(Mode::Idle, None, None, None));
            sink.emit(&DaemonEvent::Usage {
                provider: cc(),
                updated: vec![1, 2],
                fetch_errors: BTreeMap::from([(3, "over-budget".to_owned())]),
            });
            sink.emit(&DaemonEvent::Changed);
            sink.notified(
                &cc(),
                &Notice {
                    title: "tagteam".into(),
                    body: "Switched 2 · spare → 1 · work (7d at 95%)".into(),
                },
            );
            for reason in [StopReason::Signal, StopReason::Upgrade, StopReason::Schema] {
                sink.emit(&DaemonEvent::Stopping { reason });
            }
        });
        assert_eq!(
            out,
            "t+0  daemon started (pid 4242); Ctrl-C stops it\n\
             t+0  mode collect (paused)\n\
             t+0  waiting for the engine lock held by pid 4243\n\
             t+0  waiting for the engine lock held by another process\n\
             t+0  mode auto; keychain locked, retrying at t+60\n\
             t+0  waiting for an upgrade: a newer tagteam migrated the store\n\
             t+0  mode idle\n\
             t+0  usage: #1 updated, #2 updated, #3 failed (over-budget)\n\
             t+0  notified: Switched 2 · spare → 1 · work (7d at 95%)\n\
             t+0  stopped\n\
             t+0  tagteam was upgraded; start the daemon again\n\
             t+0  a newer tagteam migrated the store; start the daemon again\n"
        );
        assert_eq!(err, "");
        // With `daemon.autoswitch` true, `collect` is no pause: one switchable account.
        let (out, _) = human("", |sink| sink.emit(&mode(Mode::Collect, None, None, None)));
        assert_eq!(out, "t+0  mode collect\n");
    }

    #[test]
    fn a_tick_s_events_print_as_auto_prints_them() {
        let (out, err) = human("", |sink| {
            sink.emit(&DaemonEvent::Auto(AutoEvent::Poll {
                provider: cc(),
                active: Some((2, "b@x.co".into())),
                headroom_pct: BTreeMap::from([(1, Some(80.0)), (2, Some(40.0))]),
                threshold: 90.0,
                fetch_errors: BTreeMap::new(),
                windows_pct: BTreeMap::new(),
            }));
            sink.emit(&DaemonEvent::Auto(AutoEvent::NoSwitch {
                provider: cc(),
                reason: "below-threshold".into(),
                detail: String::new(),
            }));
            sink.emit(&DaemonEvent::Auto(AutoEvent::ConfigWarning {
                provider: cc(),
                message: "a warning".into(),
            }));
            sink.tick_done(&cc());
        });
        assert_eq!(out, "t+0  #2 b@x.co  60%  no switch: below-threshold\n");
        assert_eq!(err, "t+0  warning: a warning\n");
    }

    #[test]
    fn json_prints_each_event_as_the_socket_delivers_it_and_a_gone_reader_stops_it() {
        // D8.1: `daemon_event_json`, one object per line. A reader that has gone stops the
        // command as SIGPIPE would (§14.1), as `auto --json` does.
        let events = [
            mode(Mode::Auto, None, None, None),
            DaemonEvent::Stopping {
                reason: StopReason::Signal,
            },
        ];
        let mut out = Vec::new();
        let now = || T0 * 1000;
        {
            let sink = DaemonJson::new(&mut out, &now);
            for e in &events {
                sink.emit(e);
            }
        }
        let want: String = events
            .iter()
            .map(|e| format!("{}\n", daemon_event_json(e, T0)))
            .collect();
        assert_eq!(String::from_utf8(out).unwrap(), want);

        struct Gone;
        impl Write for Gone {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let cancel = Cancel::new();
        let mut gone = Gone;
        DaemonJson::new(&mut gone, &now)
            .stopping(&cancel)
            .emit(&events[0]);
        assert_eq!(cancel.requested(), Some(libc::SIGPIPE));
    }

    #[test]
    fn each_keychain_instance_s_counts_are_handed_over_once() {
        // Decision 7: the vault's Keychain is usually the command's own instance.
        let one: Arc<dyn Keychain> = Arc::new(FakeKeychain::new());
        let other: Arc<dyn Keychain> = Arc::new(FakeKeychain::new());
        assert_eq!(keychain_counts([&one, &one]).len(), 1);
        assert_eq!(keychain_counts([&one, &other]).len(), 2);
        let both = keychain_counts([&one, &other]);
        assert!(Arc::ptr_eq(&both[0], &one.read_counts().unwrap()));
    }
}
```

Create `crates/tagteam/tests/daemon_cli.rs`:

```rust
//! `tagteam daemon` through the real binary (D8.1, D4.1, D4.6): its events, its exits, the run
//! shell's refusal, and a second daemon's wait. Every endpoint stays offline (`std_cmd`'s
//! default), and readings are recorded fresh, so a tick sends nothing. Notifications are off,
//! because the fixture says macOS, where they would run the real `/usr/bin/osascript`. Every
//! run is drained while it runs (`Running`). Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Running, cmd, now_epoch_s, record_reading, std_cmd, two_fresh_accounts, usage_window};
use serde_json::{Value, json};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId, WindowKind};
use tagteam_provider::Env;
use tagteam_provider::profile::{ProfileMarker, canonical_profile_path};

const INSIDE: &str = "this command cannot run inside a `tagteam run` session";

/// `a@x.co` at position 1 and `b@x.co` at 2, live. b is at 60 % of its 7d window and a at
/// 20 %, read now, so a tick stays. Notifications are off.
fn staying(root: &Path) -> (String, String) {
    let (a, b) = two_fresh_accounts(root);
    let now = now_epoch_s();
    for (id, seven) in [(&a, 20.0), (&b, 60.0)] {
        let windows = [
            usage_window("5h", "5h", WindowKind::Short, 10.0, Some(now + 9_630), Some(18_000)),
            usage_window("7d", "7d", WindowKind::Long, seven, Some(now + 291_630), Some(604_800)),
        ];
        record_reading(root, id, now, &windows);
    }
    cmd(root)
        .args(["config", "set", "notify.enabled", "false"])
        .assert()
        .success();
    (a, b)
}

fn daemon(root: &Path, args: &[&str]) -> std::process::Command {
    let mut c = std_cmd(root);
    c.arg("daemon").args(args);
    c
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn events(bytes: &[u8]) -> Vec<Value> {
    text(bytes)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Each stdout line after its `HH:MM:SS  ` time.
fn untimed(bytes: &[u8]) -> Vec<String> {
    text(bytes)
        .lines()
        .map(|l| {
            let (time, rest) = l.split_at(10);
            assert!(time.ends_with("  ") && time.as_bytes()[2] == b':', "{l}");
            rest.to_owned()
        })
        .collect()
}

fn lock(root: &Path, name: &str) -> PathBuf {
    Env::for_test(root).data_dir().join("locks").join(name)
}

#[test]
fn json_prints_its_events_and_a_signal_stops_it_with_exit_0_and_both_records_emptied() {
    // D8.1, D4.6: SIGTERM and SIGINT each end it at the next cancellation point. `stopping`
    // is the last event and the exit is 0. The records of `daemon.lock` and of the engine lock
    // it held are emptied before the locks go.
    for signal in [libc::SIGTERM, libc::SIGINT] {
        let d = tempfile::tempdir().unwrap();
        staying(d.path());
        let mut running = Running::spawn(daemon(d.path(), &["--json"]));
        running.wait_for(Duration::from_secs(20), "its first tick", |out| {
            out.contains(r#""event":"no-switch""#)
        });
        let record: Value =
            serde_json::from_slice(&fs::read(lock(d.path(), "daemon.lock")).unwrap()).unwrap();
        assert_eq!(record["pid"], json!(running.child.id()), "{signal}");
        assert_eq!(record["version"], json!(env!("CARGO_PKG_VERSION")));
        running.signal(signal);
        let out = running.finish(Duration::from_secs(10));
        assert_eq!(out.status.code(), Some(0), "{signal}: {}", text(&out.stderr));
        assert_eq!(text(&out.stderr), "", "{signal}");
        let events = events(&out.stdout);
        let modes: Vec<&Value> = events.iter().filter(|e| e["event"] == "mode").collect();
        assert_eq!(
            (modes[0]["provider"].as_str(), modes[0]["mode"].as_str()),
            (Some("claude-code"), Some("auto")),
            "{signal}"
        );
        let last = events.last().unwrap();
        assert_eq!((last["event"].as_str(), last["reason"].as_str()), (Some("stopping"), Some("signal")));
        for name in ["daemon.lock", "autoswitch-claude-code.lock"] {
            assert_eq!(fs::read(lock(d.path(), name)).unwrap(), b"", "{signal}: {name}");
        }
    }
}

#[test]
fn human_output_has_auto_s_tick_lines_and_the_daemon_s_own() {
    // D8.1: `auto`'s line per tick, and the daemon's lines for its start, modes and stop.
    let d = tempfile::tempdir().unwrap();
    staying(d.path());
    let mut running = Running::spawn(daemon(d.path(), &[]));
    running.wait_for(Duration::from_secs(20), "its first tick", |out| {
        out.contains("no switch: below-threshold")
    });
    let pid = running.child.id();
    running.signal(libc::SIGTERM);
    let out = running.finish(Duration::from_secs(10));
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(text(&out.stderr), "");
    let lines = untimed(&out.stdout);
    assert_eq!(lines[0], format!("daemon started (pid {pid}); Ctrl-C stops it"));
    assert!(lines.contains(&"mode auto".to_owned()), "{lines:?}");
    assert!(
        lines.contains(&"#2 b@x.co  60%  no switch: below-threshold".to_owned()),
        "{lines:?}"
    );
    assert_eq!(lines.last().unwrap(), "stopped");
}

#[test]
fn inside_a_run_shell_it_refuses_with_exit_1_and_takes_no_lock() {
    // D4.1 step 1, D8.1.
    let d = tempfile::tempdir().unwrap();
    let (a, _) = staying(d.path());
    let session = Env::for_test(d.path()).data_dir().join("sessions").join(&a);
    fs::create_dir_all(&session).unwrap();
    ProfileMarker {
        provider: ProviderId::new(CLAUDE_CODE),
        account_id: AccountId::from_string(&a),
        config_dir: canonical_profile_path(&session).unwrap().display().to_string(),
        outer: json!({"CLAUDE_CONFIG_DIR": null, "CLAUDE_SECURESTORAGE_CONFIG_DIR": null}),
    }
    .write(&session)
    .unwrap();
    let in_shell = |args: &[&str]| {
        let mut c = daemon(d.path(), args);
        c.env("CLAUDE_CONFIG_DIR", &session);
        Running::spawn(c).finish(Duration::from_secs(20))
    };
    let out = in_shell(&[]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(text(&out.stderr), format!("tagteam: {INSIDE}\n"));
    assert_eq!(out.stdout, b"");
    let out = in_shell(&["--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "inside-run-shell", "message": INSIDE}})
    );
    assert!(!lock(d.path(), "daemon.lock").exists());
}

#[test]
fn a_second_daemon_says_it_waits_and_takes_over_when_the_first_stops() {
    // D4.1 step 2: one daemon per data directory. The second says so once on stderr, runs
    // nothing while it waits, and tries again every 5 s.
    let d = tempfile::tempdir().unwrap();
    staying(d.path());
    let mut first = Running::spawn(daemon(d.path(), &["--json"]));
    first.wait_for(Duration::from_secs(20), "its first tick", |out| {
        out.contains(r#""event":"no-switch""#)
    });
    let mut second = Running::spawn(daemon(d.path(), &["--json"]));
    let notice = format!(
        "note: a daemon already runs (pid {}); waiting for it to stop\n",
        first.child.id()
    );
    second.wait_for_stderr(Duration::from_secs(20), "its notice", |err| err == notice);
    assert_eq!(second.stdout(), "", "nothing ran while it waited");
    first.signal(libc::SIGTERM);
    assert_eq!(first.finish(Duration::from_secs(10)).status.code(), Some(0));
    second.wait_for(Duration::from_secs(20), "its first tick", |out| {
        out.contains(r#""event":"no-switch""#)
    });
    let record: Value =
        serde_json::from_slice(&fs::read(lock(d.path(), "daemon.lock")).unwrap()).unwrap();
    assert_eq!(record["pid"], json!(second.child.id()));
    second.signal(libc::SIGTERM);
    let out = second.finish(Duration::from_secs(10));
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text(&out.stderr), notice, "the notice once, and nothing else");
}

#[test]
fn an_unknown_flag_is_a_usage_error() {
    // D8.1: exit 2.
    let d = tempfile::tempdir().unwrap();
    cmd(d.path()).args(["daemon", "--once"]).assert().code(2);
}
```

Append to `crates/tagteam/tests/app.rs` (it already has `H`, `Scripted`, `UNLOCK` and
`LOCKED`; add `use std::time::Duration;` and `use tagteam_provider::FlockGuard;` to its
imports):

```rust
#[test]
fn daemon_on_a_terminal_offers_to_unlock_first_and_a_decline_exits_1() {
    // D4.1 step 3, Decision 14: as `auto` does, before `daemon.lock` is tried.
    let h = H::new();
    h.kc.set_locked(true);
    let mut no = Scripted::answering(&["n"]);
    let (code, out, err) = h.run(&["daemon"], &mut no);
    assert_eq!(
        (code, out.as_str(), err),
        (1, "", format!("tagteam: {LOCKED}\n"))
    );
    assert_eq!(no.asked, [UNLOCK]);
    h.kc.set_refuse_unlock(true);
    let (code, _, err) = h.run(&["daemon"], &mut Scripted::answering(&["y"]));
    assert_eq!((code, err), (1, format!("tagteam: {LOCKED}\n")));
    assert_eq!(h.kc.unlock_attempts(), 1);
    assert!(!h.env.data_dir().join("locks/daemon.lock").exists());
}

#[test]
fn daemon_without_a_terminal_runs_no_keychain_check_and_a_signal_stops_it_with_exit_0() {
    // D4.1 step 3: a service has nobody to answer, and the check could block on a dialog
    // (Appendix A.3). With no store the provider is idle. The signal ends the run at its next
    // slice, with no "too late" notice.
    let h = H::new();
    h.kc.set_locked(true);
    let cancel = Cancel::new();
    let signal = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(1));
            cancel.request(libc::SIGTERM);
        })
    };
    let (code, out, err) = h.run_with_cancel(&["daemon"], &mut Scripted::none(), &cancel);
    signal.join().unwrap();
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    let lines: Vec<&str> = out.lines().map(|l| &l[10..]).collect();
    let started = format!("daemon started (pid {}); Ctrl-C stops it", std::process::id());
    assert_eq!(lines.first(), Some(&started.as_str()), "{out}");
    assert!(lines.contains(&"mode idle"), "{out}");
    assert_eq!(lines.last(), Some(&"stopped"), "{out}");
    assert_eq!(h.kc.unlock_attempts(), 0);
}

#[test]
fn daemon_with_a_store_it_cannot_open_exits_1_and_lets_daemon_lock_go_empty() {
    // D8.1: a store that cannot be opened is a start failure.
    let h = H::new();
    std::fs::create_dir_all(h.env.data_dir()).unwrap();
    std::fs::write(h.env.data_dir().join("tagteam.db"), "x".repeat(200)).unwrap();
    let (code, out, err) = h.run(&["daemon"], &mut Scripted::none());
    assert_eq!((code, out.as_str()), (1, ""), "{err}");
    assert!(err.starts_with("tagteam: "), "{err}");
    let lock = h.env.data_dir().join("locks/daemon.lock");
    assert_eq!(std::fs::read(&lock).unwrap(), b"");
    assert!(FlockGuard::try_lock(&lock).unwrap().is_some());
}
```

- [ ] **Step 6: Run them and see them fail**

Run: `cargo test -p tagteam --lib daemon_cmd` and
`cargo test -p tagteam --features test-support --test daemon_cli --test app daemon`
Expected:
- The `--lib` run FAILS to compile: `daemon_cmd` holds no `DaemonHuman`, `DaemonJson` or
  `keychain_counts`, and `HumanSink::with_clock` does not exist.
- The binary and in-process runs compile and FAIL. `daemon` is not a subcommand yet: `app.rs`'s
  `Cli::try_parse_from(…).unwrap()` panics on it, and `daemon_cli.rs` sees clap's exit 2.

- [ ] **Step 7: Implement (the CLI)**

In `crates/tagteam/src/cli.rs`, after the `Auto { … }` variant of `Command`:

```rust
    /// Run the daemon in the foreground: collect usage and switch automatically, until stopped
    ///
    /// One job per provider (or only --provider's). While daemon.autoswitch is true it switches
    /// as `tagteam auto` does, and otherwise it only collects usage, so `tagteam config set
    /// daemon.autoswitch false` pauses it. Notifications follow the notify.* settings. --json
    /// prints one event per line: {"schemaVersion":1,"event":<kind>,"ts":"…Z", …}. It exits 0
    /// when a signal stops it, and 75 when a newer tagteam must take over.
    Daemon {},
```

`Command::Daemon` stays out of `touches_keychain` and `touches_keychain_only_with_a_store`.
Its check is its own (Decision 14).

In `crates/tagteam/src/auto.rs`:
- `fn written(` becomes `pub(crate) fn written(`.
- `HumanSink`'s `fn line(&self, now: i64, provider: &ProviderId, text: &str)` becomes
  `pub(crate) fn line(`.
- After `HumanSink::stopping`, add:

```rust
    /// The engine's wall clock, epoch seconds: what `tagteam daemon`'s own lines are timed by.
    pub(crate) fn now_s(&self) -> i64 {
        (self.now_ms)().div_euclid(1000)
    }

    /// Epoch seconds as this sink writes a time of day.
    pub(crate) fn time_of(&self, epoch_s: i64) -> String {
        (self.clock)(epoch_s)
    }

    /// A line about no provider in particular: the time, then `text` (D8.1's start and stop).
    pub(crate) fn plain(&self, now: i64, text: &str) {
        let mut out = self.out.borrow_mut();
        written(
            writeln!(out, "{}  {text}", (self.clock)(now)),
            &self.cancel,
        );
    }

    /// This sink on `clock` instead of the local time of day, for tests outside this module.
    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: fn(i64) -> String) -> Self {
        self.clock = clock;
        self
    }
```

Put the implementation above the test module in `crates/tagteam/src/daemon_cmd.rs`:

```rust
//! `tagteam daemon` (D8.1): the lines a person reads, the JSONL a script reads, and what the
//! command hands the daemon. `tagteam-daemon` schedules and decides; this module only renders.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use tagteam_core::ProviderId;
use tagteam_daemon::events::{DaemonEvent, DaemonSink, Mode, StopReason, WaitingFor};
use tagteam_daemon::notify::Notice;
use tagteam_daemon::protocol::daemon_event_json;
use tagteam_daemon::watch::program_path;
use tagteam_engine::auto::EventSink;
use tagteam_engine::settings::Settings;
use tagteam_provider::keychain::ReadCounts;
use tagteam_provider::{Cancel, Env, Keychain};

use crate::auto::{HumanSink, written};

/// D4.5, D8.1: stopped so that a newer tagteam takes over, either a new binary or the one that
/// migrated the store. A service restarts it.
pub(crate) const EXIT_RESTART: i32 = 75;

/// Decision 7: the read counts of each Keychain instance in `keychains`, an instance named
/// twice counted once. The vault's Keychain is usually the command's own.
pub(crate) fn keychain_counts<'k>(
    keychains: impl IntoIterator<Item = &'k Arc<dyn Keychain>>,
) -> Vec<Arc<ReadCounts>> {
    let mut counts: Vec<Arc<ReadCounts>> = Vec::new();
    for c in keychains.into_iter().filter_map(|k| k.read_counts()) {
        if !counts.iter().any(|seen| Arc::ptr_eq(seen, &c)) {
            counts.push(c);
        }
    }
    counts
}

/// D4.5: the path the upgrade watch follows. It is `argv[0]` when that is absolute, as a
/// service unit passes it, and otherwise what `daemon install` would record (D9.1).
pub(crate) fn program(path_var: Option<&OsStr>) -> PathBuf {
    let argv0 = std::env::args_os().next().unwrap_or_default();
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(&argv0));
    program_path(&argv0, path_var, &exe)
}

/// A mode line's text (D4.3). `paused` means the provider's `daemon.autoswitch` reads false.
fn mode_text(
    mode: Mode,
    waiting_for: Option<WaitingFor>,
    holder_pid: Option<u32>,
    paused: bool,
) -> String {
    match (mode, waiting_for) {
        (Mode::Auto, _) => "mode auto".into(),
        (Mode::Collect, _) if paused => "mode collect (paused)".into(),
        (Mode::Collect, _) => "mode collect".into(),
        (Mode::Idle, _) => "mode idle".into(),
        (Mode::Waiting, Some(WaitingFor::EngineLock)) => match holder_pid {
            Some(pid) => format!("waiting for the engine lock held by pid {pid}"),
            None => "waiting for the engine lock held by another process".into(),
        },
        (Mode::Waiting, Some(WaitingFor::Schema)) => {
            "waiting for an upgrade: a newer tagteam migrated the store".into()
        }
        (Mode::Waiting, None) => "waiting".into(),
    }
}

/// A `collect` round's line (D5.5's `usage`): each recorded position, then each failed one.
fn usage_text(updated: &[u32], fetch_errors: &BTreeMap<u32, String>) -> String {
    let mut parts: Vec<String> = updated.iter().map(|p| format!("#{p} updated")).collect();
    parts.extend(
        fetch_errors
            .iter()
            .map(|(p, kind)| format!("#{p} failed ({kind})")),
    );
    if parts.is_empty() {
        return "usage: nothing collected".into();
    }
    format!("usage: {}", parts.join(", "))
}

/// The last line (D4.5, D4.6).
fn stopping_text(reason: StopReason) -> &'static str {
    match reason {
        StopReason::Signal => "stopped",
        StopReason::Upgrade => "tagteam was upgraded; start the daemon again",
        StopReason::Schema => "a newer tagteam migrated the store; start the daemon again",
    }
}

/// D8.1's human output: `auto`'s lines for the ticks, through its own `HumanSink` (§11.4), and
/// a line of the daemon's own for its start and stop, each mode change, each `collect` round
/// and each notification. Errors and warnings go to stderr, as `auto`'s do.
pub(crate) struct DaemonHuman<'a> {
    auto: HumanSink<'a>,
    /// Where `daemon.autoswitch` is read, to tell a pause from one switchable account.
    env: &'a Env,
}

impl<'a> DaemonHuman<'a> {
    pub(crate) fn new(auto: HumanSink<'a>, env: &'a Env) -> Self {
        DaemonHuman { auto, env }
    }

    /// The first line, once the daemon holds `daemon.lock`.
    pub(crate) fn started(&self, pid: u32) {
        let text = format!("daemon started (pid {pid}); Ctrl-C stops it");
        self.auto.plain(self.auto.now_s(), &text);
    }
}

impl DaemonSink for DaemonHuman<'_> {
    fn emit(&self, e: &DaemonEvent) {
        let now = self.auto.now_s();
        match e {
            DaemonEvent::Auto(e) => self.auto.emit(e),
            DaemonEvent::Mode {
                provider,
                mode,
                waiting_for,
                holder_pid,
                backoff,
            } => {
                let paused = *mode == Mode::Collect
                    && !Settings::load(self.env, provider).0.daemon_autoswitch;
                let text = mode_text(*mode, *waiting_for, *holder_pid, paused);
                let text = match backoff {
                    Some(b) => format!(
                        "{text}; keychain locked, retrying at {}",
                        self.auto.time_of(b.until)
                    ),
                    None => text,
                };
                self.auto.line(now, provider, &text);
            }
            DaemonEvent::Usage {
                provider,
                updated,
                fetch_errors,
            } => self
                .auto
                .line(now, provider, &usage_text(updated, fetch_errors)),
            DaemonEvent::Changed => {}
            DaemonEvent::Stopping { reason } => self.auto.plain(now, stopping_text(*reason)),
        }
    }

    fn tick_done(&self, provider: &ProviderId) {
        self.auto.tick_done(provider);
    }

    fn notified(&self, provider: &ProviderId, notice: &Notice) {
        let text = format!("notified: {}", notice.body);
        self.auto.line(self.auto.now_s(), provider, &text);
    }
}

/// `--json` (D8.1): every event as one object on a line of its own, exactly as the socket's
/// `subscribe` delivers it (D5.5).
pub(crate) struct DaemonJson<'a> {
    out: RefCell<&'a mut dyn Write>,
    /// The engine's wall clock, epoch milliseconds: each event's `ts`.
    now_ms: &'a dyn Fn() -> i64,
    /// What a `BrokenPipe` on `out` stops the command through.
    cancel: Option<Cancel>,
}

impl<'a> DaemonJson<'a> {
    pub(crate) fn new(out: &'a mut dyn Write, now_ms: &'a dyn Fn() -> i64) -> Self {
        DaemonJson {
            out: RefCell::new(out),
            now_ms,
            cancel: None,
        }
    }

    /// A write that fails with `BrokenPipe` (the reader is gone) records SIGPIPE in `cancel`.
    pub(crate) fn stopping(mut self, cancel: &Cancel) -> Self {
        self.cancel = Some(cancel.clone());
        self
    }
}

impl DaemonSink for DaemonJson<'_> {
    fn emit(&self, e: &DaemonEvent) {
        let line = daemon_event_json(e, (self.now_ms)().div_euclid(1000));
        let mut out = self.out.borrow_mut();
        written(writeln!(out, "{line}"), &self.cancel);
    }
}
```

In `crates/tagteam/src/app.rs`:

1. Imports. Add these; Task 7 already imports `ThreadSleeper` and `uniform_jitter` from
   `tagteam_daemon`, and `daemon` uses those imports too:

```rust
use tagteam_daemon::daemon::{Daemon, DaemonError, DaemonExit, DaemonOptions};
use tagteam_daemon::notify::Backend;
use tagteam_daemon::watch::{ProgramStamp, UPGRADE_WATCH_MS};
use tagteam_provider::keychain::ReadCounts;
```

   Also add `daemon_cmd` to the `use crate::{ … }` list.

2. In `struct App`, after the `keychain` field:

```rust
    /// `Context::platform`: the daemon's notification backend depends on it (D6.2).
    platform: Platform,
    /// Decision 7: the read counts of the command's Keychains, which the daemon's keychain
    /// backoff compares (D4.1). Empty on Linux, which has no Keychain to back off for.
    keychain_counts: Vec<Arc<ReadCounts>>,
```

3. In `run`, replace

```rust
    let stops_on_a_signal = matches!(cli.command, Some(Command::Auto { once: false, .. }));
```

   with

```rust
    let stops_on_a_signal = matches!(
        cli.command,
        Some(Command::Auto { once: false, .. } | Command::Daemon { .. })
    );
```

   and extend the comment above it: `§11.4 and D4.6: a signal is how the `auto` loop and the
   daemon end, so it is never too late for them.`

4. In `run_command`, after `let keychain = (ctx.platform == Platform::MacOs).then(|| ctx.keychain.clone());`:

```rust
    let keychain_counts = match ctx.platform {
        Platform::MacOs => daemon_cmd::keychain_counts(
            std::iter::once(&ctx.keychain).chain(ctx.vault_keychain.as_ref()),
        ),
        Platform::Linux => Vec::new(),
    };
    let platform = ctx.platform;
```

   and add `platform,` and `keychain_counts,` to the `App { … }` literal after `keychain,`.

5. In `command_name`, after `Command::Auto { .. } => "auto",`:

```rust
        Command::Daemon { .. } => "daemon",
```

6. In `dispatch`, after the `Command::Auto { … } => { … }` arm:

```rust
            Command::Daemon { .. } => return self.daemon(),
```

7. After `fn auto(&mut self, run: AutoRun)`:

```rust
    /// D8.1: `tagteam daemon` in the foreground. It stamps its program file first (Decision
    /// 17). Like `auto`, it refuses inside a run shell (D4.1 step 1). On a terminal it runs
    /// Appendix A.3's check and offers to unlock, as `auto` does. Without a terminal it runs
    /// none: a service has nobody to answer, and the check can block on a SecurityAgent dialog
    /// (D4.1 step 3, Decision 14). It exits 0 once a signal stops it, and 75 once a newer
    /// tagteam must take over (D4.5).
    fn daemon(&mut self) -> Result<i32, Failure> {
        // D4.5, Decision 17: before anything that can wait (the unlock offer, the
        // `daemon.lock` wait), so a binary replaced meanwhile is an upgrade at the upgrade
        // watch's first run, never this daemon's own baseline.
        let program = ProgramStamp::take(daemon_cmd::program(self.engine.env().var("PATH")));
        if matches!(self.engine.run_shell(), RunShell::Inside { .. }) {
            return Err(EngineError::InsideRunShell.into());
        }
        if self.io.prompter.interactive() {
            self.ensure_unlocked()?;
        }
        let color = self.color();
        let engine = &self.engine;
        let path = engine.env().var("PATH");
        let opts = DaemonOptions {
            provider: self.provider_flag.clone(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            program,
            upgrade_watch_ms: UPGRADE_WATCH_MS,
            keychain_counts: self.keychain_counts.clone(),
            backend: Backend::detect(self.platform == Platform::MacOs, path),
        };
        let several = self.provider_flag.is_none() && engine.provider_ids().len() > 1;
        let now_ms = || engine.now_ms();
        let names = |p: &ProviderId| display_name(engine, p);
        let Io { out, err, .. } = &mut *self.io;
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
        Ok(match exit {
            DaemonExit::Signal => 0,
            DaemonExit::Restart => daemon_cmd::EXIT_RESTART,
        })
    }
```

8. After `impl From<PurgeError> for Failure { … }`:

```rust
impl From<DaemonError> for Failure {
    fn from(e: DaemonError) -> Self {
        match e {
            DaemonError::Engine(e) => Failure::Engine(e),
        }
    }
}
```

   A signal during the `daemon.lock` wait arrives here as `EngineError::Interrupted`.
   `run_command` turns it into the usual interruption (128 + n), since the daemon never served.

- [ ] **Step 8: Run the tests and see them pass**

Run:
```bash
cargo test -p tagteam-daemon --features test-hooks
cargo test -p tagteam --lib
cargo test -p tagteam --features test-support --test daemon_cli --test app --test run_shell_cli
```
then `cargo fmt --all`, `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and again with
`--features tagteam/test-support`.
Expected: PASS. `daemon_cli` takes about 20 s, mostly the second daemon's 5 s retry. The binary
tests never spawn `osascript`, since notifications are off, and use no terminal or pty. They
spawn `tagteam daemon` and signal it: run them sandboxed first, and outside only on sandbox
evidence (Execution notes). `auto_cli` and the other suites are unaffected, because `auto`'s
sinks only gained `pub(crate)` helpers.

- [ ] **Step 9: Commit** (the controller commits; implementers do not)

`git commit -m "Run the daemon in the foreground with tagteam daemon"`

---

### Task 13: Verification

**Depends on:** 12

Plan A is checked as a whole. Two pinned invariants from §15.3 are extended to the daemon:
- the local-state invariant (D12: "The pinned test (§15.3) covers the daemon's runs")
- the log redaction pin (§15.3 "Logs", D11)

Then the full suite, clippy both ways, and the format check run. Nothing else changes; plan B
follows in the same worktree, and the plan's status lines stay as they are.

**Readings of the spec this task commits to:**
- **The local-state pin comes in two halves, because one test cannot hold both.** The pinned
  machinery lives in `tagteam-engine/tests/common` (`Fx::snapshot`,
  `assert_only_surface_changed`, §3's masking and append-only rules), and the engine cannot
  depend on the daemon crate. A dev-dependency would build a second copy of the engine, whose
  `Engine` the daemon's types do not accept.
  - **Engine half,** in `tests/invariant.rs` under the full §3 rules. It runs what the daemon's
    provider job runs (D4.3), with the daemon's source:
    - a tick that switches
    - a `collect` round whose live token is refreshed (§7.5) and fetched
    - a tick over an unmanaged login, followed by its `collect` round

    These are the only paths through which the daemon writes Claude Code's state (D12).
  - **Daemon half,** in `tagteam-daemon`. `Started::serve` runs scripted slices with a switch.
    Every file under HOME except tagteam's data directory must stay byte-identical, and so must
    every Keychain item except the vault's. The surface's JSON file counts with its declared
    keys taken out; its credential files and items are left out. This proves the daemon itself
    (its lock, notifier, watches and board) writes nothing else. The engine half holds what
    changes inside the surface to §3.
- **The redaction pin runs a daemon at TRACE** against the pin's own server and fixture: emails,
  an organization name and tokens that nothing else contains. It runs as a process that ticks
  and is stopped by SIGTERM. Notifications are off because the fixture says macOS, where they
  would run the real `osascript`. Task 11's notifier test pins that the notifier's own lines
  carry no text. The pin asserts that the daemon logged something (`the daemon started`), so it
  cannot pass vacuously.

**Files:**
- Modify: `crates/tagteam-engine/tests/invariant.rs` (the daemon's runs, macOS and Linux)
- Modify: `crates/tagteam-daemon/src/daemon.rs` (`mod tests`: the serve-level pin)
- Modify: `crates/tagteam/tests/logging.rs` (`every_command_at_trace_leaves_no_identity_or_secret_in_the_log`
  gains a daemon run)

**Interfaces:**
- Consumes:
  - `AutoEngine::with_source` (Task 2) and `Engine::collect_usage(CollectMode::Scheduled { .. })`
  - `Daemon::start` and `Started::serve` (Task 12), and Task 12's test helpers `options`,
    `stop_after` and `two_at` in `daemon.rs`'s `mod tests`
  - `crate::testutil`'s `Fx` (with `kc`, `env`, `clock`) and `Emitted`
  - `Provider::identity_surface`
  - `tagteam_provider::splice::{remove_top_level, replace_top_level}`
  - from the engine's `tests/common`: `Fx`, `Recorded`, `record_reading`, `usage_window` and
    `usage_fixture`
  - from the CLI's `tests/common`: `Running`
- Produces: nothing new.

- [ ] **Step 1: The engine half of the local-state pin**

In `crates/tagteam-engine/tests/invariant.rs`, extend the imports:

```rust
use common::{Recorded, record_reading, usage_fixture, usage_window};
use tagteam_core::WindowKind;
use tagteam_core::autoswitch::{AutoConfig, Decision, NoSwitchReason, Strategy};
use tagteam_engine::auto::TickOutcome;
use tagteam_engine::collect::CollectMode;
use tagteam_provider::{Clock, Provider};
```

and add, after `every_m1_command_writes_only_the_identity_surface_through_symlinked_config_on_linux`:

```rust
/// The fixture clock's start (`Fx`), in seconds.
const T0: i64 = 1_790_000_000;

/// D12, §15.3: what the daemon's provider job runs (D4.3), each step under the comparison.
/// `daemon.lock` and the engine lock records live in tagteam's data directory, which the walk
/// leaves out with the rest of tagteam's own data. The steps are:
/// - a tick with the daemon's source that switches
/// - a `collect` round that refreshes the live token (§7.5) and fetches
/// - a tick over a login tagteam does not manage, then its `collect` round
fn run_the_daemon_s_work_on(fx: &Fx) {
    seed_realistic_state(fx);
    login_with_extras(fx, "a@x.co", "rt-a");
    let a = fx.engine.add_live(fx.add_options()).unwrap().account.id;
    login_with_extras(fx, "b@x.co", "rt-b");
    let b = fx.engine.add_live(fx.add_options()).unwrap().account.id;
    let provider = fx.provider();
    let cfg = AutoConfig {
        threshold: 90.0,
        hysteresis_pct: 10.0,
        cooldown_s: 300,
        interval_s: 60,
        unhealthy_ticks: 3,
        strategy: Strategy::Best,
        include_api_key_accounts: false,
        models: Vec::new(),
        long_window: fx.cc.primary_long_window().map(str::to_owned),
    };
    let reading = |long: f64| {
        vec![
            usage_window("5h", WindowKind::Short, 10.0, T0 + 9_630),
            usage_window("7d", WindowKind::Long, long, T0 + 291_630),
        ]
    };
    record_reading(&fx.engine, &a, &reading(20.0), T0, T0 + 300);
    record_reading(&fx.engine, &b, &reading(95.0), T0, T0 + 300);
    let daemon = || {
        fx.engine
            .auto(&provider, cfg.clone(), false)
            .unwrap()
            .unwrap()
            .with_source("daemon")
    };
    check(fx, "daemon tick that switches", || {
        let (outcome, decision) = daemon().tick(&Recorded::default()).unwrap();
        assert_eq!(outcome, TickOutcome::Switched, "{decision:?}");
    });
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_extra_credential_keys_present(fx);

    // `collect`: a, now live, is due and its access token expired, so §7.5 refreshes it before
    // the fetch. The scripted usage reply repeats for the candidate.
    fx.clock.advance_ms(400_000);
    let mut live = fx.live_credential().unwrap();
    live["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms());
    fx.set_live_credential(live.to_string().as_bytes());
    fx.script_refresh(Some("rt-a2"));
    fx.script_usage(200, usage_fixture());
    let collect = || CollectMode::Scheduled {
        provider: provider.clone(),
        threshold: 90.0,
        models: Vec::new(),
    };
    check(fx, "daemon collect round with an active-token refresh", || {
        drop(fx.engine.collect_usage(collect()).unwrap())
    });
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));

    // A login tagteam does not manage. The tick stops at §11.2 step 2, and its `collect` round
    // collects the candidates (D4.3).
    login_with_extras(fx, "stranger@x.co", "rt-stranger");
    fx.clock.advance_ms(400_000);
    check(fx, "daemon tick over an unmanaged login, then its collect round", || {
        let (_, decision) = daemon().tick(&Recorded::default()).unwrap();
        assert!(
            matches!(
                decision,
                Decision::NoSwitch {
                    reason: NoSwitchReason::UnmanagedActiveAccount,
                    ..
                }
            ),
            "{decision:?}"
        );
        drop(fx.engine.collect_usage(collect()).unwrap());
    });
}

#[test]
fn the_daemon_s_runs_write_only_the_identity_surface_on_macos() {
    run_the_daemon_s_work_on(&Fx::with_platform(Platform::MacOs));
}

#[test]
fn the_daemon_s_runs_write_only_the_identity_surface_on_linux() {
    run_the_daemon_s_work_on(&Fx::with_platform(Platform::Linux));
}
```

Run: `cargo test -p tagteam-engine --test invariant the_daemon`
Expected: PASS for both. This pins behaviour Tasks 2 and 8 already built. A failure names the
step and the path or Keychain item that changed outside the surface. That is a real §3
violation: fix it in the code that wrote it, never by loosening this test.

- [ ] **Step 2: The daemon half of the local-state pin**

In `crates/tagteam-daemon/src/daemon.rs`'s `mod tests`, extend the imports:

```rust
    use std::collections::BTreeMap;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use tagteam_cc::ClaudeCode;
    use tagteam_cc::live::Platform;
    use tagteam_engine::vault::SERVICE;
    use tagteam_provider::Provider;
    use tagteam_provider::splice::{remove_top_level, replace_top_level};
```

and add:

```rust
    /// Every file, directory and link under `dir` except under `skip`, by path: its kind, its
    /// mode and what it holds, never following a link.
    fn walk(dir: &Path, skip: &Path, out: &mut BTreeMap<PathBuf, (char, u32, Vec<u8>)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.starts_with(skip) {
                continue;
            }
            let meta = fs::symlink_metadata(&path).unwrap();
            let mode = meta.permissions().mode() & 0o7777;
            let held = if meta.file_type().is_symlink() {
                ('l', fs::read_link(&path).unwrap().as_os_str().as_bytes().to_vec())
            } else if meta.is_dir() {
                walk(&path, skip, out);
                ('d', Vec::new())
            } else {
                ('f', fs::read(&path).unwrap())
            };
            out.insert(path, (held.0, mode, held.1));
        }
    }

    /// What a daemon run must leave byte-identical (D12, §3). That is every entry under HOME
    /// except tagteam's data directory, and every Keychain item except the vault's. Claude
    /// Code's identity surface is taken out: its JSON file without its declared keys, and its
    /// credential files and items left out. The engine's pinned test holds what changes inside
    /// them to §3.
    #[allow(clippy::type_complexity)]
    fn outside_the_surface(
        home: &Fx,
    ) -> (
        BTreeMap<PathBuf, (char, u32, Vec<u8>)>,
        BTreeMap<(String, String), Vec<u8>>,
    ) {
        let surface =
            ClaudeCode::new(home.kc.clone(), Platform::MacOs).identity_surface(&home.env);
        let mut files = BTreeMap::new();
        walk(&home.env.home, &home.env.data_dir(), &mut files);
        for (path, keys) in &surface.json_keys {
            if let Some((_, _, bytes)) = files.get_mut(path) {
                for key in keys {
                    *bytes = remove_top_level(bytes, key).unwrap();
                }
            }
        }
        for path in &surface.credential_files {
            files.remove(path);
        }
        let items = home
            .kc
            .items()
            .into_iter()
            .filter(|(item, _)| {
                item.0 != SERVICE
                    && !surface.credential_items.contains(item)
                    && !surface.owned_items.contains(item)
            })
            .collect();
        (files, items)
    }

    #[test]
    fn a_daemon_run_with_a_switch_writes_nothing_outside_the_identity_surface() {
        // D12, §15.3: a run of the whole daemon (its lock, provider job, watches and
        // notifier) that switches. Claude Code's other state stays byte-identical, as does
        // `~/.claude.json` without its surface keys, among them a key of its own.
        let home = two_at(95.0, Some("spare"));
        let config = home.env.home.join(".claude.json");
        let doc = fs::read(&config).unwrap();
        fs::write(&config, replace_top_level(&doc, "userID", &json!("u")).unwrap()).unwrap();
        let claude = home.env.home.join(".claude");
        fs::create_dir_all(claude.join("projects/-work-app/memory")).unwrap();
        fs::write(claude.join("projects/-work-app/memory/MEMORY.md"), "remember this\n").unwrap();
        fs::write(claude.join("settings.json"), "{\"theme\":\"dark\"}\n").unwrap();
        fs::write(claude.join("CLAUDE.md"), "instructions\n").unwrap();
        fs::set_permissions(claude.join("CLAUDE.md"), fs::Permissions::from_mode(0o640)).unwrap();
        home.kc.put("Other Service", "someone", b"unrelated");
        let before = outside_the_surface(&home);
        let opts = DaemonOptions {
            backend: Some(Backend::Osascript),
            ..options(&home)
        };
        let started = Daemon::start(&home.engine, opts, &mut |n| panic!("{n}")).unwrap();
        let sink = Emitted::new(&home.clock);
        assert_eq!(
            started.serve(&sink, &stop_after(&home, 3), &mut || 0.0),
            DaemonExit::Signal
        );
        assert!(
            sink.events()
                .iter()
                .any(|(_, e)| matches!(e, DaemonEvent::Auto(AutoEvent::Switch { .. }))),
            "the run switched: {:?}",
            sink.events()
        );
        let after = outside_the_surface(&home);
        assert_eq!(before.1, after.1, "a Keychain item outside the surface changed");
        for (path, entry) in &after.0 {
            assert_eq!(before.0.get(path), Some(entry), "{} changed", path.display());
        }
        assert_eq!(before.0.len(), after.0.len(), "an entry outside the surface went away");
    }
```

With `Backend::Osascript` the notifier runs. Its delivery goes to the home's `ScriptedSpawner`
(`Fx::spawner`), which answers `SpawnFailed` because nothing is scripted, so no program ever
starts. The fixture also registers FakeAgent, whose job stays `idle` with no account; the walk
covers whatever it might write.

Run: `cargo test -p tagteam-daemon a_daemon_run_with_a_switch`
Expected: PASS.

- [ ] **Step 3: The log redaction pin runs a daemon**

In `crates/tagteam/tests/logging.rs`:
- add `Running` to the `use common::{ … }` list
- in `every_command_at_trace_leaves_no_identity_or_secret_in_the_log`, insert this block after
  the `for args in [&["auto", "--once", "--dry-run"][..], &["auto", "--once"][..]] { … }` loop
  and before `// M5b's commands.`:

```rust
    // The daemon (D11): one run at TRACE whose provider job ticks against the server, stopped
    // by SIGTERM. Notifications are off, since the fixture says macOS, where they would run the
    // real `osascript`. The notifier's own lines name no account (tagteam-daemon's notify
    // tests).
    run(&["config", "set", "notify.enabled", "false"]);
    let mut daemon = Running::spawn({
        let mut c = std_cmd(root);
        c.env(TAGTEAM_LOG, "trace")
            .env(API_BASE, server.base_url())
            .args(["daemon", "--json"]);
        c
    });
    daemon.wait_for(Duration::from_secs(20), "its first tick", |out| {
        [r#""event":"switch""#, r#""event":"no-switch""#, r#""event":"error""#]
            .iter()
            .any(|kind| out.contains(kind))
    });
    daemon.signal(libc::SIGTERM);
    let out = daemon.finish(Duration::from_secs(10));
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
```

- after the assertion that ends `"the commands logged their state changes:\n{log}"`, add:

```rust
    assert!(
        log.contains(" INFO tagteam_daemon::daemon: "),
        "the daemon logged its start and stop:\n{log}"
    );
```

Run: `cargo test -p tagteam --features test-support --test logging every_command`
Expected: PASS. It spawns `tagteam daemon` and signals it: run it sandboxed first, and outside
only on sandbox evidence (Execution notes). Every line's target still starts with `tagteam` (`tagteam_daemon` does). No
email, organization name or 13-character run of a secret appears.

- [ ] **Step 4: Format, lint and test everything**

Run:
```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings
cargo test --workspace --features tagteam/test-support --no-fail-fast 2>&1 | tee "$TMPDIR/daemon-a-test.log" | grep -E '^test result:|FAILED|panicked'
awk '/^test result:/ {p += $4; f += $6; i += $8} END {printf "%d/%d passed, %d ignored\n", p, p + f, i}' "$TMPDIR/daemon-a-test.log"
cargo test -p tagteam --lib
grep -c '"daemon", "--json"' crates/tagteam/tests/logging.rs
```

Expected:
- `fmt --check` prints nothing, and both clippy runs finish with no warning.
- Every `test result:` line says `ok`, and no `FAILED` or `panicked` line appears.
- `awk` prints `N/N passed, M ignored`, with the two numbers before the slash equal. Report that
  line as the total.
- `cargo test -p tagteam --lib` runs without features, so it exercises the release branch of
  the test-override checks.
- The `grep` prints `1`: the redaction pin runs the daemon.

`--features tagteam/test-support` also turns on `tagteam-daemon/test-hooks`, so the workspace
run includes the daemon crate's hook-gated test. The new tests use no pty and no real Keychain.
Run everything sandboxed first. The pty tests need to run outside it, and the daemon's binary
tests (`daemon_cli`, `logging`) may, as Execution notes list; re-run exactly those outside
only on sandbox evidence, and report the totals of both runs together.

On a Mac, run the real Keychain tests as Task 4's counting touched `SecurityCli`:

```bash
cargo test -p tagteam-provider --features real_keychain --test real_keychain
```

Expected: PASS with no GUI prompt. An agent runs them sandboxed first, and unsandboxed only when
that run fails with sandbox evidence: `Operation not permitted`, or a `securityd` or
`SecKeychain` error.

- [ ] **Step 5: Commit** (the controller commits; implementers do not)

`git commit -m "Pin the daemon's local-state and logging invariants"`
