# The TUI switches accounts while it is open: implementation plan

**Status:** Approved

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** With no daemon, an open TUI runs the daemon's own provider jobs on a lane thread, so it
auto-switches exactly as the daemon would, steps aside for a daemon, pauses with `P`, polls
nothing twice and says truthfully where auto runs.

**Architecture:** The engine lock's holder record gains `"holder": "daemon" | "auto" | "tui"`.
`tagteam-daemon` gains `hosted`: `run_hosted` runs the daemon's settings job, store watch, the
schema half of the upgrade watch, and one provider job per provider behind a *daemon gate*. The
gate reads `daemon.lock` before every provider job's run and every second while the scheduler is
idle, and drops or rebuilds the provider jobs. `tagteam-tui`
runs `run_hosted` on a new `auto` lane thread built from a factory the binary supplies. The lane's
board and events reach the state machine through the protocol's own encoding, as `Msg::Lane`, and
the state machine treats them as it treats a live daemon's `status` and events.

**Tech Stack:** Rust 1.99.1 (MSRV 1.89), ratatui, crossterm `=0.29.0`, rusqlite; daemon tests on
`testutil::Fx` with a fake clock; pty tests through `crates/tagteam/tests/common/pty.rs`.

**Spec:** `docs/superpowers/specs/2026-11-11-tagteam-tui-autoswitch-design.md` (cited "S§n").
The TUI, redesign, daemon and core specs stay the authority for everything S does not change.
Line numbers are as of `a356e75d` and will drift.

## Global Constraints

- The daemon, `tagteam auto`, the decision and every CLI command keep their behaviour; only the
  words that name a lock holder change (S scope). Every daemon test passes unedited, except where
  `Engine::engine_holder`'s new return type needs a mechanical change to compile.
- No new dependency, no new setting. `daemon.autoswitch` keeps its name (S§2 non-goals).
- The lane hosts no notifier (`notices: None`), no token jobs, no program half of the upgrade
  watch, and no socket (S§2, S§3.1).
- The lane's engine is built by the binary's factory, inside the lane thread, with
  `EventSource::Auto`, the lane's own cancel token and the counted Keychain, and holds its locks
  as `HolderKind::Tui` (S§3.1, S§3.6).
- Gate: `daemon.lock`'s record is live when `read_daemon_record` gives `Present` and its stamp is
  not `Liveness::Dead`. It is checked before every provider job's run, and at least every
  `SLICE_MS` (1 000 ms) while the scheduler sleeps. Grace after a record dies:
  `GRACE_MS = 30_000` of forward time on the engine clock (S§3.2).
- Header words are S§3.3's, character for character. Shorten one only if a width check forces it
  (S§6), and record that as a ruling.
- Out of scope: a schema check inside every store write (S§3.7; Michael, 2026-11-11).
- `tagteam-tui` keeps `#![forbid(unsafe_code)]`.
- Commit messages: one plain imperative sentence, no prefix, no attribution.
- Every commit passes `cargo fmt --all --check`, CONTRIBUTING.md's three clippy runs with
  `-D warnings`, and the focused tests of what it changed. pty tests, socket tests and the full
  suite need the sandbox escape. cargo-deny runs unsandboxed from
  `~/.local/share/mise/installs/cargo-deny/0.20.2/cargo-deny-0.20.2-aarch64-apple-darwin/cargo-deny check advisories`.
  Never run `mise trust`.

## Review Focus

1. **The wall clock set back during the 30 s grace** must not stretch the grace past 30 s of
   time that actually passed, as D4.2 rules for every wait. Task 2,
   `a_clock_set_back_during_the_grace_never_stretches_it`.
2. **A second TUI takes over when the first quits**, at its next re-evaluation (60 s or less), as
   a daemon does. Task 2, `two_runs_on_one_home_the_second_waits_and_takes_over`.
3. **Ctrl-Z while a tick is in flight** stops the process once that job returns: the lane's token
   is set before the join, and the UI thread blocks on nothing else. Not cheaply reproducible: the
   reviewer checks `Ui::stop` and `AutoLane::suspend`.
4. **A `daemon.lock` record whose start time cannot be read** counts as live (doctor's rule), so
   the lane stays stopped and the header names the daemon. The reviewer checks that
   `live_daemon_pid` and `ProviderJob::live_holder` judge it alike.
5. **A daemon run with `--provider X`** makes the lane drop every job (S§3.2's table). In live
   mode the other provider's header reads `○ auto off`, which is true. The reviewer confirms that
   no path shows a hosted state for a provider whose job was dropped. Task 3,
   `the_views_are_read_again_when_the_lane_s_jobs_start_or_stop`, covers the stale-line half.

## Rulings made while planning (inside S)

1. **Hosting lives in `tagteam-daemon`** (`hosted.rs`). It reuses the daemon's private
   `SettingsJob` and its test fixture (`Fx`, `Slices`, `Emitted`). The TUI only threads it
   (S§6: "the lane's start and stop mechanics").
2. **The gate is one shared state with two callers.**
   - Every provider job is wrapped (`HostedJob`), and each wrapper looks at the gate before it
     runs. No provider work starts on a stale look, even for a job overdue behind a long run
     (the scheduler picks the earliest due, not the first added).
   - A gate job, `daemon-gate`, looks every `SLICE_MS`, so a daemon that comes up while the
     scheduler sleeps is seen within a second.
   - A look that flips the gate wakes `provider:`, so every wrapper drops or rebuilds its job at
     the next passes.
   - The scheduler lives as long as the lane.
3. **Status and events cross through the protocol's own encoding**: `status_json` →
   `serde_json::from_value::<Status>`, and `daemon_event_json` → `parse_event`. The header
   therefore reads exactly what a socket would give (S§3.3).
4. **Holder field**: `"holder"`, values `daemon`, `auto` and `tui` (`HolderKind`). An engine's
   default follows its `EventSource` (`Daemon` → `daemon`, else `auto`). The lane sets `tui`. An
   unknown value reads as absent.
5. **Writers under §3.7**: every `Action` but `Doctor` and `Autoswitch` (`R` included, since a
   session's launch writes the store). The token index lives in the store, so its refreshes (the
   first, the Tokens timers, `r` on Tokens) stop too ("the TUI as a whole stops writing").
   - The refusal happens twice. The state machine refuses before submitting (Task 4).
   - A shared flag, set by the auto lane before it reports `Upgraded`, also refuses a writer that
     a lane takes off its queue, or that the UI thread is about to suspend for, after detection
     (Task 6).
   - Only work already started when the flag is set finishes (S§3.7).
6. **Header precedence**: live daemon → upgraded → mismatch → stopping → hosted job → daemon
   record without link → lock holder (C-16) → `○ auto off · no daemon`.
7. **A TUI holder is named wherever the TUI shows a waiting-for-engine-lock state**, live or
   hosted, by the snapshot's holder kind for that pid. An unknown kind keeps today's words.
8. **When the lane's jobs start or stop, the views are read again**, so the header's daemon pid
   and holders follow within one read, not the 30 s timer.
9. **The first collection waits for the lane's first report** as it waits for the daemon's first
   `status`, bounded by `SETTLE_MS` (C-R20).
10. **A store that cannot be opened at the lane's start** (other than a newer schema) ends the
    lane as `Off`, logged at WARN. The header stays as today.
11. **`P` before the lane's first report** says `NO_DAEMON_RUNNING`, as today's TUI does before
    the daemon's first `status` (existing test `p_without_a_daemon_says_so_and_changes_nothing`).
12. **`purge`'s refusal is unchanged**: it names a pid only, and S§3.6 lists `auto` and `doctor`.
13. **The lane runs every registered provider** (G1). `--provider` only chooses whose settings
    tables its engine reads (Decision 3), as for the other lanes.
14. **A daemon that has just come up reads as "its socket is down"** until the live thread's next
    try, at most 30 s (TUI §8's backoff). This is accepted, and reported to tagteam-8f with the
    plan.
15. **The lane's engine takes a cancel token of its own**, which the shutdown also sets
    (`Lanes::close`). S§3.1 gives it "the shutdown token", but Ctrl-Z must stop and join the lane
    while every other lane runs on, and setting the shutdown token would stop them too. Every quit
    path still stops the lane: `begin_shutdown` and `finish` both call `Lanes::close`.

## Order

Each task builds on the crates the task before it changed (engine → daemon → tui → binary), so
Tasks 1–7 run one at a time. Task 8 changes documents only and may run beside Task 7. Tier:
sp-standard for every task; escalate to sp-architect after a failed attempt.

---

### Task 1: The engine lock's record names its holder (S§3.6)

**Depends on:** none.

**Files:**
- Modify: `crates/tagteam-engine/src/auto.rs:274-405` (holder record, `Engine::auto`,
  `engine_holder`), `crates/tagteam-engine/src/engine.rs:113-170` (field, `new`),
  `crates/tagteam-engine/src/daemon.rs` (`live_daemon_pid`),
  `crates/tagteam-engine/src/doctor/accounts.rs:30-56,483-495`,
  `crates/tagteam-engine/src/doctor/daemon.rs:170-175`,
  `crates/tagteam/src/app/commands/daemon.rs:65-131,160`
- Adapt only: `crates/tagteam-engine/tests/auto_engine.rs:397-445` and `auto.rs`'s own tests
  (compare `.stamp()`).
- Test: `crates/tagteam-engine/src/auto.rs` tests, `crates/tagteam-engine/tests/auto_engine.rs`,
  `crates/tagteam-engine/src/daemon.rs` tests, doctor's tests, `crates/tagteam/tests/auto_cli.rs`,
  `crates/tagteam/src/app/commands/daemon.rs` tests.

**Interfaces:**
- Produces, in `tagteam_engine::auto`:
  - `HolderKind { Daemon, Auto, Tui }`, declared with the crate's `str_enum!` (`"daemon"`,
    `"auto"`, `"tui"`), as `EventSource` is.
  - `pub struct EngineHolder { pub pid: u32, pub start: u64, pub kind: Option<HolderKind> }` with
    `pub fn stamp(&self) -> ProcessStamp` and `pub fn liveness(&self) -> Liveness`.
  - `Engine::engine_holder(&self, &ProviderId) -> Read<EngineHolder>`.
  - `Engine::with_lock_holder(self, HolderKind) -> Engine`, `Engine::lock_holder(&self) -> HolderKind`.
  - `read_holder(&Path) -> Read<EngineHolder>` (crate-private, as now).
- Produces, in `tagteam_engine::daemon`: `pub fn live_daemon_pid(env: &Env) -> Option<u32>`, the
  pid of `daemon.lock`'s record when it is `Present` and its stamp is not `Liveness::Dead`.
- `ProviderJob::live_holder` and the TUI's `foreign_engine_holders` compile unchanged: they use
  only `pid`, `start` and `liveness()`.

- [ ] **Step 1: Write the failing tests**

  - `auto.rs` tests: `a_record_without_the_holder_field_reads_as_before`: a
    `{"pid":1,"start":2}` record gives `Present(EngineHolder { pid: 1, start: 2, kind: None })`;
    `{"pid":1,"start":2,"holder":"tui"}` gives `kind: Some(HolderKind::Tui)`; `"holder":"robot"`
    gives `kind: None`.
  - `auto_engine.rs`: `each_engine_names_its_holder_in_its_lock_record`: an engine built with
    `EventSource::Daemon` writes `"holder":"daemon"`, one with `EventSource::Auto` writes `"auto"`,
    and `.with_lock_holder(HolderKind::Tui)` writes `"tui"`. Each record is read back through
    `engine_holder` with that kind, and the file is empty again after the `AutoEngine` drops.
  - `daemon.rs` tests: `live_daemon_pid_names_only_a_live_record`: no file gives `None`, an empty
    file gives `None`, this process's stamp gives `Some(own pid)`, and the same pid with
    `start - 1` gives `None`.
  - Doctor: `doctor_names_a_tagteam_window_holding_the_engine_lock`. With this process's stamp
    and `"holder":"tui"`, the `auto.engine` check is info
    `auto-switch runs in a tagteam window (pid N)`. Dead, it is ok
    `auto-switch is not running (its last engine, a tagteam window (pid N), has exited)`. The
    `Unknown` fix says ``check whether pid N is still running a `tagteam tui`, then `tagteam doctor` again``.
    The daemon and other-holder words stay as they are.
  - `commands/daemon.rs` tests, beside the existing `AlreadyRuns` case:
    `auto_s_refusal_names_a_tagteam_window`. `auto_failure` with a holder of
    `(HolderKind::Tui, 4711)` gives `auto-switch already runs for claude-code (a tagteam window, pid 4711; press P there to pause it, or quit it)`.
    The daemon case keeps its words.
  - `auto_cli.rs`: `auto_names_a_tagteam_window_when_a_tui_holds_the_engine_lock`, modelled on
    `auto_names_the_daemon_when_the_daemon_holds_the_engine_lock`. The test holds the engine lock
    with a `"holder":"tui"` record of its own stamp, `tagteam auto --once` exits with
    `engine-running`, and stderr carries the words above.

- [ ] **Step 2: Run them and see them fail**

  Run: `cargo test -p tagteam-engine --locked holder` and
  `cargo test -p tagteam --features tagteam/test-support --test auto_cli tagteam_window`.
  Expected: compile errors (`HolderKind`, `EngineHolder`, `live_daemon_pid` undefined).

- [ ] **Step 3: Implement**

  - `write_holder(path, kind)` appends `"holder":"<kind>"` to today's one JSON line.
  - `read_holder` fills `kind` from `v["holder"]` through `HolderKind::parse`.
  - `Engine` gets a `lock_holder` field, set in `Engine::new` from `cfg.source`.
  - Doctor resolves who holds the lock: `Daemon` when `held_by_daemon` matches (today's rule,
    which now takes `&EngineHolder`), else the record's kind, else as today.
  - In `commands/daemon.rs`, replace `daemon_engine` with
    `engine_lock_holder(engine, provider) -> Option<(HolderKind, u32)>`: `Daemon` by today's
    `daemon.lock` match, `Tui` by the record's kind, otherwise `None` (today's bare name).
  - `auto_failure`'s `daemon` parameter becomes
    `holder: &dyn Fn(&ProviderId) -> Option<(HolderKind, u32)>`.

- [ ] **Step 4: Run the focused tests, then the daemon's and the TUI's suites to prove they compile and pass unchanged**

  Run: `cargo test -p tagteam-engine --locked`, `cargo test -p tagteam-daemon --locked`,
  `cargo test -p tagteam-tui --locked`, `cargo test -p tagteam --features tagteam/test-support --test auto_cli --test doctor_cli`.
  Expected: PASS.

- [ ] **Step 5: Commit**: `Name the engine lock's holder in its record, in doctor and in auto's refusal`

---

### Task 2: Host the daemon's jobs behind a daemon gate (S§3.1, S§3.2)

**Depends on:** Task 1.

**Files:**
- Create: `crates/tagteam-daemon/src/hosted.rs`
- Modify: `crates/tagteam-daemon/src/lib.rs` (`pub mod hosted`),
  `crates/tagteam-daemon/src/daemon.rs:350-397` (`SettingsJob` becomes `pub(crate)` with
  `pub(crate) fn new(board, backend, provider)`),
  `crates/tagteam-daemon/src/watch.rs:271-350` (`UpgradeWatch::schema_only`),
  `crates/tagteam-daemon/src/status.rs:146-170` (`StatusBoard::remove_provider`),
  `crates/tagteam-daemon/src/testutil.rs` (`engine_on` takes an `EventSource`; `Fx::tui_engine`)
- Test: `crates/tagteam-daemon/src/hosted.rs` tests

**Interfaces:**
- Consumes: `live_daemon_pid`, `HolderKind::Tui`, `Engine::with_lock_holder` (Task 1).
- Produces, in `tagteam_daemon::hosted`:
  ```rust
  pub const GRACE_MS: i64 = 30_000;
  pub struct HostedOptions {
      /// Whose tables the settings job reads the engine's settings for (Decision 3).
      pub settings_provider: ProviderId,
      pub keychain_counts: Vec<Arc<ReadCounts>>,
  }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum HostedEnd { Signal, Off, Schema }
  pub fn run_hosted(
      engine: &Engine,
      opts: HostedOptions,
      sink: &dyn DaemonSink,
      on_status: &dyn Fn(&StatusSnapshot),
      sleeper: &dyn Sleeper,
      jitter: &mut dyn FnMut() -> f64,
  ) -> HostedEnd;
  ```
  - `Off`: inside a run shell, or the store could not be opened (Ruling 10). No job ran.
  - `Schema`: the store's schema is newer than this build's, at start or later. Every lock has
    been released.
  - `Signal`: the engine's cancel token ended the scheduler. Every lock has been released.
  - `on_status` is called whenever the board's provider lines differ from the last ones passed:
    after every provider-job run and every gate run. It is called with no lines once no provider
    job runs.
- Produces: `UpgradeWatch::schema_only(board: Arc<StatusBoard>) -> UpgradeWatch` (no program
  check, and newer-at-start is not assumed); `StatusBoard::remove_provider(&self, &ProviderId)`;
  and, in tests, `Fx::tui_engine(&self) -> Engine` (`EventSource::Auto`, holder `Tui`).

- [ ] **Step 1: Write the failing tests** (in `hosted.rs`, on `Fx`, `Slices`, `Emitted`, `read_two`-style readings; `fx.tui_engine()` is the lane's engine)

  - `gate_rule`, a table over the pure gate (`Gate::at_start(daemon_live, now_ms)`,
    `Gate::step(&mut self, daemon_live, now_ms) -> bool` for "changed", `Gate::runs(&self)`):
    - started at once with no daemon;
    - stopped at start with a daemon, and still stopped 29 999 ms after the record died;
    - started at 30 000 ms;
    - stopped again at once when a record comes back during the grace, and the grace restarts;
    - with the record dead 20 000 ms, a step an hour earlier adds nothing. The gate starts once
      10 000 ms more have passed after that step, not 30 000.
  - `with_no_daemon_the_jobs_take_the_lock_and_store_a_switch_as_auto`: with `read_two(95.0)`,
    the first pass holds the lock, its record names `tui`, and the tick switches 2 → 1. The stored
    `switch` event's source is `auto`, and `on_status` got a line in `auto`.
  - `a_live_daemon_record_frees_the_locks_at_the_next_passes`: the run holds the lock; a
    `Slices` hook writes `daemon.lock` with this process's stamp. Within the next slice's passes
    the lock record is `Absent`, the board has no provider line, and `on_status` was last called
    with none. The settings job and schema watch still run: a settings write after that still
    reloads `engine.settings()`.
  - `a_record_that_appears_during_a_held_open_request_frees_the_lock_once_it_returns`: the engine's
    `Http` is a test wrapper whose usage `send` writes the daemon record, asserts the lock record
    is still `Present`, and returns a timeout. Once the job returns, the lock is `Absent` before
    the next tick.
  - `the_jobs_start_again_30_s_after_the_record_dies_and_not_before`: the record is emptied at
    `T0 + 5 s`. The lock is `Absent` at `T0 + 34.9 s`, and `Present` (`tui`) by `T0 + 36 s`.
  - `a_clock_set_back_during_the_grace_never_stretches_it`: the record dies, 20 s of slices pass,
    then the clock jumps back an hour. The lock is `Absent` 9 s of slices after the jump, and
    `Present` by 11 s.
  - `an_overdue_provider_never_starts_work_after_a_daemon_appears`. Claude Code and FakeAgent each
    have two accounts in `auto`. Claude Code's usage request goes through a test `Http`, which
    writes the daemon record and advances the fake clock past FakeAgent's next tick before it
    returns. After that request:
    - no usage request is sent for either provider;
    - both lock records are `Absent` by the end of the next passes.
  - `a_daemon_live_at_start_keeps_every_provider_job_from_running`: no lock is taken and no
    `mode` event is emitted.
  - `a_newer_schema_ends_the_run_with_schema_and_frees_the_lock`: at start (store version set
    above `SCHEMA_VERSION`) it ends with `Schema` before any job runs. Midway, after the watch's
    period, it ends with `Schema` and the lock is `Absent`.
  - `a_settings_change_reaches_the_engine_and_the_jobs`: `fx.settings("[notify]\nenabled = false\n", later_mtime)`
    reaches `engine.settings().notify_enabled` at the next pass. Writing
    `[provider.claude-code.daemon] autoswitch = false` moves the job to `collect`, frees the lock,
    and stores `autoswitch-paused` with source `auto`. Writing it `true` again takes the lock and
    stores `autoswitch-resumed`.
  - `inside_a_run_shell_nothing_runs`: `Fx::with_run_shell(Inside…)` ends `Off` at once; no lock,
    no `on_status` call.
  - `two_runs_on_one_home_the_second_waits_and_takes_over`: the first run's engine holds the lock;
    a second run (another `tui_engine` on the same home) shows `waiting`/`engine-lock` with
    `holder_pid` this process. Once the first run's engine is dropped, the second holds the lock
    within 60 s of slices.
  - `a_signal_ends_the_run_and_frees_every_lock`: `engine.cancel().request(SIGTERM)` in a slice
    hook gives `Signal`, and the lock record is `Absent` when `run_hosted` returns.

- [ ] **Step 2: Run them and see them fail**

  Run: `cargo test -p tagteam-daemon --locked hosted`. Expected: compile errors (`hosted` missing).

- [ ] **Step 3: Implement `run_hosted`**

  Order of work:
  1. Inside a run shell → `Off`.
  2. `watch::schema_newer(engine)` → `Schema`.
  3. Read `Settings::mtime`, then build the board: `pid` this process, `version`
     `env!("CARGO_PKG_VERSION")`, `socket: None`, empty log, notifications off.
  4. `StoreBaseline::take(engine)`; an error → WARN and `Off`.
  5. `Scheduler::new().with_settings_mtime(..)`, all jobs first due now, added in this order:
     the gate, `SettingsJob::new(board, None, settings_provider)`,
     `StoreWatch::new(..).waking(scheduler.wake())`, `UpgradeWatch::schema_only(board)`, then one
     `HostedJob` per `engine.provider_ids()`.
  6. Run. `StopReason::Schema` (or `Upgrade`, never expected) → `Schema`, else `Signal`. Drop the
     scheduler before returning.

  - **`Gate`**, pure. It keeps whether jobs run and the forward time the record has been dead.
    - A step adds `max(0, now_ms - last_ms)` only when this look and the one before both found no
      live record. A clock set back adds nothing (Review Focus 1).
    - A live record stops the jobs and zeroes that time.
  - **One look** (`look(engine, gate, wake) -> bool`, "jobs run"): reads
    `live_daemon_pid(engine.env())`, steps the shared `Rc<RefCell<Gate>>`, and wakes `JOB_PREFIX`
    when the gate flips. The gate job and every `HostedJob` call it.
  - **Gate job** (`name` `daemon-gate`, `Ran::After(SLICE_MS)` always): one look, then reports
    the board through the shared reporter.
  - **`HostedJob`** (`name` `provider:<id>`), holding `Option<ProviderJob<'e>>`. It starts each
    run with one look.
    - While the gate stops jobs: drops its inner job (freeing its lock), calls
      `board.remove_provider`, and returns `Ran::After(60_000)`.
    - While jobs run: builds the inner job if it has none, with
      `ProviderJobCtx { board, notices: None, keychain_counts, schema_newer: false }`, and returns
      the inner job's `Ran`.
    - Either way it then reports the board. `settings_changed` is forwarded to the inner job when
      there is one.
  - **Reporter**: one per run, shared with `Rc`. It calls `on_status` only when
    `snapshot().providers` differs from the last lines it passed.

- [ ] **Step 4: Run the hosted tests, then the whole daemon crate**

  Run: `cargo test -p tagteam-daemon --locked`. Expected: PASS, every pre-existing test unedited.

- [ ] **Step 5: Commit**: `Run the daemon's jobs for a host that steps aside while a daemon runs`

---

### Task 3: The lane's jobs and events in the TUI's state (S§3.3 live events, S§3.5)

**Depends on:** Task 2 (it shares the build; there is no logical dependency).

**Files:**
- Modify: `crates/tagteam-tui/src/msg.rs` (`LaneReport`, `Msg::Lane`),
  `crates/tagteam-tui/src/daemon/mod.rs:65-160` (`DaemonState`), `:288-319` (event body),
  `:531-585` (`on_tick`), `crates/tagteam-tui/src/update.rs:240-245` (the `Msg::Lane` arm)
- Test: `crates/tagteam-tui/src/daemon/mod.rs` tests

**Interfaces:**
- Produces, in `crate::msg`:
  ```rust
  pub(crate) enum LaneReport {
      /// The lane's board as `status` encodes it; no `providers` while it runs no job.
      Status(Status),
      Event(Event),
      /// It runs nothing: inside a run shell, or its engine or store could not be opened.
      Off,
      /// The store's schema is newer than this build's: stopped until the TUI restarts.
      Upgraded,
  }
  ```
  and `Msg::Lane(LaneReport)`.
- Produces, on `DaemonState`:
  - `pub fn job(&self, &ProviderId) -> Option<&StatusProvider>`: the live `status`'s job, else
    the lane's.
  - `pub fn hosted_job(&self, &ProviderId) -> Option<&StatusProvider>`: the lane's only.
  - `pub fn upgraded(&self) -> bool`.
  - `pub(crate) fn await_lane(&mut self, now_ms: i64)`.
  - `settled()`: true once the daemon and, after `await_lane`, the lane are known, each bounded by
    `SETTLE_MS`.
- Produces: `pub(crate) fn on_lane(state: &mut State, report: LaneReport, now_ms: i64) -> Vec<Cmd>`
  in `daemon/mod.rs`.

- [ ] **Step 1: Write the failing tests** (reuse the module's `auto`, `usage`, `mode`, `daemon_status`/`daemon_job` helpers; a lane status is a `Status` with the job lines)

  - `a_lane_status_gives_its_jobs_until_a_live_daemon_s_status_replaces_them`: `job(cc)` is the
    lane's; after `Live::Up` and a `status`, it is the daemon's. `hosted_job` stays the lane's.
  - `the_timer_skips_a_lane_provider_in_auto_and_collects_one_in_idle`: modelled on
    `the_timer_collects_only_for_the_providers_nothing_else_collects_for`, with the lane's status
    in place of the daemon's.
  - `the_first_collection_waits_for_the_lane_s_first_report_or_2_s`: after `await_lane`,
    `drawn` asks for no collection until `Msg::Lane(Off)`, or until `SETTLE_MS` passes on ticks.
    After a lane status with cc in `auto`, the first collection leaves cc out.
  - `a_lane_event_reaches_the_auto_line_and_re_reads_as_a_socket_event_does`. With no link:
    - a `no-switch` is `noted`;
    - `usage` reads the views `REREAD_MS` later, once;
    - a `switch` re-reads History's timeline at once (T4-19);
    - `changed` reads the views;
    - nothing asks for `FetchStatus`.
  - `the_views_are_read_again_when_the_lane_s_jobs_start_or_stop`: a lane status that goes from
    none to some lines, or from some to none, returns `LoadAccounts`. A status that only changes a
    line does not.
  - `off_and_upgraded_end_the_lane_s_jobs_and_settle`: either report clears `hosted_job` and
    settles. `upgraded()` is true only after `Upgraded`.

- [ ] **Step 2: Run them and see them fail**

  Run: `cargo test -p tagteam-tui --locked daemon::tests`. Expected: compile errors.

- [ ] **Step 3: Implement**

  - **The event body.** Move `on_event`'s body below its guard into one function. The socket
    path keeps its link guard; the lane path guards only on `quitting`. A lane event passes
    `status: false` to `reread`, and `mode`, `stopping` and `token-index` from the lane do nothing.
  - **`on_tick`.** The debounced re-reads fire with or without a link. `status` fetches, the 60 s
    poll and C-R19's wait still need one.
  - **`Msg::Lane` arm** in `update` → `daemon::on_lane`.

- [ ] **Step 4: Run the TUI crate's tests**

  Run: `cargo test -p tagteam-tui --locked`. Expected: PASS, live mode's tests unedited.

- [ ] **Step 5: Commit**: `Show the auto lane's jobs and events as the TUI shows a daemon's`

---

### Task 4: A newer store stops every TUI write (S§3.7)

**Depends on:** Task 3.

**Files:**
- Modify: `crates/tagteam-tui/src/host.rs` (`Action::writes_store`),
  `crates/tagteam-tui/src/actions.rs:18-98,145-245` (`UPGRADED`, `start`, the dialog keys, `r`),
  `crates/tagteam-tui/src/daemon/mod.rs:596-610` (`to_collect`),
  `crates/tagteam-tui/src/screens/tokens/index.rs:121-129` (`refresh`)
- Test: `crates/tagteam-tui/src/actions.rs` tests, `host.rs` tests,
  `screens/tokens/index.rs` tests, `update.rs` tests

**Interfaces:**
- Consumes: `DaemonState::upgraded()` (Task 3).
- Produces: `pub const UPGRADED: &str = "tagteam was upgraded; restart the TUI";` in `actions.rs`;
  `Action::writes_store(&self) -> bool`, which is false only for `Doctor` and `Autoswitch`
  (Ruling 5).

- [ ] **Step 1: Write the failing tests** (state after `Msg::Lane(LaneReport::Upgraded)`)

  - `host.rs`: `only_doctor_and_autoswitch_leave_the_store_alone`, over every `Action` variant.
  - `update.rs`: `after_an_upgrade_the_timer_and_the_first_frame_collect_nothing_but_still_read`:
    the 30 s tick returns `LoadAccounts` and no `CollectDue`, and `drawn` asks for no first
    collection.
  - `actions.rs`: `after_an_upgrade_r_and_every_store_writing_action_refuse_before_they_start`:
    - `r` on each screen, and `s`, `b`, `e`, `x`, `a`, `m`, `R`, `A`, each give a `Warn` toast
      of exactly `UPGRADED`;
    - none of them returns a `Cmd`, opens a dialog or sets `running`;
    - `D` still starts `Doctor`.
  - `index.rs`: `after_an_upgrade_the_token_index_is_never_refreshed`: `first_refresh`, the
    Tokens timer at `+70 s` and `+200_250` ms, and a refresh after a report return no
    `RefreshIndex`.

- [ ] **Step 2: Run them and see them fail**

  Run: `cargo test -p tagteam-tui --locked upgrade`. Expected: FAIL (no refusal; collections and
  refreshes still asked for).

- [ ] **Step 3: Implement**

  - One check, `refuses_write(state, now_ms) -> bool`, toasts `UPGRADED` when
    `state.daemon.upgraded()`. It is called by `start` for a writer, by `account_key` before a
    writer's dialog opens, and by `refresh` (`r`).
  - `to_collect` returns nothing once upgraded.
  - The index's `refresh` returns no `Cmd` once upgraded, leaving `started` untouched.

- [ ] **Step 4: Run the TUI crate's tests**

  Run: `cargo test -p tagteam-tui --locked`. Expected: PASS.

- [ ] **Step 5: Commit**: `Stop every TUI write once the store is newer than this build`

---

### Task 5: Header words, `P`, and the lock holders the TUI reads (S§3.3, S§3.4)

**Depends on:** Tasks 1, 3, 4.

**Files:**
- Modify: `crates/tagteam-provider/src/process.rs:28-33` (`ProcessStamp::of`),
  `crates/tagteam-tui/src/source.rs:47-55,123,214-238` (holders, `daemon_pid`),
  `crates/tagteam-tui/src/state.rs:224-232` (`holder_of`, `daemon_pid`),
  `crates/tagteam-tui/src/daemon/words.rs:19-130`,
  `crates/tagteam-tui/src/actions.rs:146-177` (`pause`); fixtures that build `AccountsSnapshot`
  (`runtime.rs:822-832`, `chrome.rs:1029`, `testutil.rs`)
- Test: `daemon/words.rs` tests, `chrome.rs` tests, `actions.rs` tests,
  `crates/tagteam-tui/tests/source.rs:266-310`

**Interfaces:**
- Consumes: `HolderKind`, `EngineHolder::kind`, `live_daemon_pid` (Task 1); `hosted_job`,
  `upgraded` (Task 3).
- Produces:
  - `ProcessStamp::of(pid: u32) -> io::Result<Option<ProcessStamp>>` in `tagteam-provider`.
  - In `source.rs`:
    ```rust
    pub struct AutoHolder { pub provider: ProviderId, pub pid: u32, pub kind: HolderKind }
    // AccountsSnapshot:
    /// Every live engine-lock holder but this process: the daemon's as `HolderKind::Daemon`
    /// (its record matches `daemon.lock`'s), else the record's kind, `Auto` when it has none.
    pub auto_holders: Vec<AutoHolder>,
    /// `live_daemon_pid` at this read.
    pub daemon_pid: Option<u32>,
    ```
  - `State::holder_of(&self, &ProviderId) -> Option<&AutoHolder>` (replaces `foreign_auto_pid`)
    and `State::daemon_pid(&self) -> Option<u32>`.
  - `pub const SOCKET_DOWN: &str = "the daemon's socket is down, so the TUI can't show its state; `tagteam daemon status` says more";`
    in `actions.rs`.

- [ ] **Step 1: Write the failing tests**

  - `words.rs`: `each_state_of_the_autoswitch_header_table_has_its_words_at_both_widths`.
    Daemon pid 4242, other holders 4711. Each row gives `(state, 120 columns, 80 columns)`:

    | State | 120 columns | 80 columns |
    |---|---|---|
    | lane `auto`, best | `● auto on in this window · switches at 90%` | `● auto on · at 90%` |
    | lane `auto`, consume-first | `● auto on in this window · uses the soonest reset first` | `● auto on · soonest reset` |
    | lane `waiting`/`engine-lock`, holder `tui` 4711 | `● auto runs in another tagteam window (pid 4711)` | `● auto in pid 4711` |
    | lane `waiting`/`engine-lock`, holder `auto` 4711 | `● auto runs in another terminal (pid 4711)` | `● auto in pid 4711` |
    | lane `collect`, paused | `‖ auto paused · P resumes` | `‖ paused · P resumes` |
    | lane `idle` | `○ auto off · no usable account` | `○ no usable account` |
    | lane `auto` with a keychain backoff | today's backoff words | today's |
    | live daemon `waiting`/`engine-lock`, holder `tui` 4711 (Ruling 7) | `● auto runs in another tagteam window (pid 4711)` | `● auto in pid 4711` |
    | no lane jobs, holder `tui` 4711 (C-16) | `● auto runs in another tagteam window (pid 4711)` | `● auto in pid 4711` |
    | upgraded | `! auto stopped · tagteam was upgraded; restart the TUI` | `! restart the TUI` |
    | daemon 4242, no link, lock held by the daemon | `● auto runs in the daemon (pid 4242) · its socket is down` | `● auto in daemon` |
    | daemon 4242, no link, lock held by `auto` 4711 | `● auto runs in another terminal (pid 4711)` | `● auto in pid 4711` |
    | daemon 4242, no link, lock held by `tui` 4711 | `● auto runs in another tagteam window (pid 4711)` | `● auto in pid 4711` |
    | daemon 4242, no link, lock not held | `○ the daemon runs (pid 4242) · its socket is down, so its state is unknown` | `○ daemon state unknown` |
    | live daemon `auto` (unchanged) | `● auto on · switches at 90%` | `● auto on · at 90%` |
    | lane `Off`, nothing else | `○ auto off · no daemon` | `○ auto off · no daemon` |

    Today's narrow `○ auto off · no daemon` is kept as it is.
  - `chrome.rs`: every new row fits the header at exactly 120 and 80 columns, using the existing
    fit test's pattern (`chrome.rs:~690`, pid 4711).
  - `actions.rs`:
    - `p_with_the_lane_running_asks_and_writes_the_setting`: lane cc in `auto` gives the
      `Pause auto for Claude Code?` dialog, then `Action::Autoswitch { on: false }`. Lane cc
      paused gives `Resume auto for Claude Code?`.
    - `p_with_a_daemon_without_its_socket_changes_nothing_and_says_so`: with `daemon_pid` set and
      no link, for the lock held by the daemon, held by `auto`, and not held, `P` opens nothing,
      returns no `Cmd`, and toasts `SOCKET_DOWN`.
    - `p_says_no_daemon_runs_only_when_the_lane_cannot_run`: after `Off` (a run shell) and after
      `Upgraded`, `P` toasts `NO_DAEMON_RUNNING`. The existing
      `p_without_a_daemon_says_so_and_changes_nothing` passes as it is.
  - `tests/source.rs`: rewrite `a_live_engine_lock_holder_is_named_and_a_dead_one_is_not` as
    `the_holders_name_the_daemon_a_tagteam_window_and_auto_and_leave_out_this_process`. A child
    `sleep 30` stamped with `ProcessStamp::of` stands in for the other process:
    - a record of the child with `"holder":"tui"` is `AutoHolder { kind: Tui, pid: child }`;
    - without the field it is `Auto`;
    - the child named by `daemon.lock` too is `Daemon`, and `daemon_pid` is the child's;
    - this process's own stamp is never listed;
    - a recycled start time is never listed.

- [ ] **Step 2: Run them and see them fail**

  Run: `cargo test -p tagteam-tui --locked` and `cargo test -p tagteam-provider --locked`.
  Expected: FAIL / compile errors.

- [ ] **Step 3: Implement**

  - `auto_state` follows Ruling 6's precedence.
  - `job_state` gains `here: bool`. When it is true, `auto` reads `auto on in this window` at 120
    columns, and the waiting words look the pid up in `holder_of`.
  - `pause`:
    - live daemon → today;
    - `hosted_job` → today's dialog;
    - mismatch → today;
    - `daemon_pid` without a link → `SOCKET_DOWN`;
    - otherwise → `NO_DAEMON_RUNNING`.
  - In `source.rs`, `engine_holders` replaces `foreign_engine_holders`.

- [ ] **Step 4: Run the focused tests**

  Run: `cargo test -p tagteam-tui --locked`, `cargo test -p tagteam-provider --locked process`.
  Expected: PASS.

- [ ] **Step 5: Commit**: `Say where auto runs in the header and let P pause the TUI's own switching`

---

### Task 6: The auto lane thread, Ctrl-Z, quit, and the binary's factory (S§3.1)

**Depends on:** Tasks 2, 4, 5.

**Files:**
- Create: `crates/tagteam-tui/src/auto_lane.rs`
- Modify: `crates/tagteam-tui/src/lib.rs` (`pub mod auto_lane`),
  `crates/tagteam-tui/src/lanes.rs:226-352` (`Lanes.auto`, `Lanes.upgraded`, the actions and
  index lanes, `spawn`, `close`, `join`),
  `crates/tagteam-tui/src/runtime.rs:43-73,96-124,272-318,578-620,633-662` (`TuiOptions.auto`,
  spawn, `await_lane`, `Ui::suspend`, `Ui::stop`), `crates/tagteam-tui/src/testutil.rs` (a `LaneHome` fixture),
  `crates/tagteam/src/tui_cmd.rs` (`auto_engines`, `TuiOptions.auto`)
- Test: `crates/tagteam-tui/src/auto_lane.rs` tests, `lanes.rs` tests, `runtime.rs` tests,
  `tui_cmd.rs` tests

**Interfaces:**
- Consumes: `run_hosted`, `HostedOptions`, `HostedEnd` (Task 2); `LaneReport`, `await_lane`
  (Task 3); `UPGRADED`, `Action::writes_store` (Task 4).
- Produces, in `tagteam_tui::auto_lane`:
  ```rust
  pub struct LaneEngine { pub engine: Engine, pub keychain_counts: Vec<Arc<ReadCounts>> }
  /// Builds the lane's engine inside its thread, under the token given (S§3.1).
  pub type AutoFactory = Arc<dyn Fn(Cancel) -> Result<LaneEngine, SourceError> + Send + Sync>;
  pub(crate) struct AutoLane { /* factory, provider, tx, the run's token and handle, upgraded flag */ }
  impl AutoLane {
      /// Starts the first run. `upgraded` is `Lanes.upgraded`.
      pub(crate) fn spawn(
          factory: AutoFactory,
          provider: Option<ProviderId>,
          tx: &Sender<Msg>,
          upgraded: Arc<AtomicBool>,
      ) -> AutoLane;
      /// A new run with a fresh token, unless one runs or a run ended `Schema`.
      pub(crate) fn resume(&self);
      /// Sets the run's token and joins its thread (Ctrl-Z).
      pub(crate) fn suspend(&self);
      /// Sets the run's token (the shutdown's step 2).
      pub(crate) fn close(&self);
      /// Joins the run; `Err` when it panicked.
      pub(crate) fn join(self) -> std::thread::Result<()>;
  }
  ```
- Produces:
  - `TuiOptions.auto: Option<AutoFactory>`; the runtime tests' `options()` sets `None`.
  - `Lanes::spawn(.., auto: Option<AutoFactory>)`, `Lanes::suspend_auto(&self)` and
    `Lanes::resume_auto(&self)`, which do nothing without a lane.
  - `Lanes.upgraded: Arc<AtomicBool>` (Ruling 5). The auto lane sets it before it sends
    `LaneReport::Upgraded`. Once it is set, a writer is refused at the moment it would start:
    - the actions lane answers a dequeued `Collect` with
      `Msg::Collected { result: Err(SourceError { kind: "store", message: UPGRADED }) }`;
    - it answers a dequeued `Act` whose `action.writes_store()` with
      `Msg::ActionDone { outcome: ActionOutcome::Done { exit: 1, out: "", err: UPGRADED } }`;
    - the index lane answers a dequeued `Refresh` with `Msg::Refreshed { result: Err(..) }`, the
      same error;
    - `Ui::suspend` delivers that `ActionDone` for a writer, without handing the terminal over.

    None of them calls the source or the host.
  - In the binary, `fn auto_engines(ctx: Context, provider: Option<ProviderId>) -> AutoFactory`.

- [ ] **Step 1: Write the failing tests**

  - `auto_lane.rs`: `the_lane_s_board_and_events_reach_the_ui_as_a_socket_gives_them`. A board
    snapshot with one `auto` line comes out as the `Status` that `status_json` parsed gives. A
    `DaemonEvent::Auto(Switch)` comes out as the `Event` that `parse_event` gives.
  - `runtime.rs`, over `LaneHome` (two FakeAgent accounts, the system clock, an `AutoFactory`
    building `EventSource::Auto` engines held as `Tui`):
    - `quit_joins_the_auto_lane_and_its_lock_is_free_after`: wait until FakeAgent's engine lock
      record is `Present` (`tui`), press `q`; the exit is 0 and the record is `Absent`.
    - `ctrl_z_frees_the_lock_before_the_process_stops_and_resume_takes_it_again`: `FakeTty::stop`
      records the lock record as it is called. It was `Absent` then, and is `Present` again within
      5 s of the resume.
    - `a_daemon_that_comes_up_during_a_hand_over_gets_the_lock_while_the_action_runs`: the host's
      `act_foreground` writes `daemon.lock` with this process's stamp, waits up to 5 s for the
      lock record to be `Absent`, and returns. It found it `Absent`.
    - `an_auto_lane_whose_engine_cannot_be_built_says_off_and_the_tui_runs_on`: the factory
      returns `Err`, the state sees `Off`, `q` exits 0.
    - `an_auto_lane_that_panics_ends_the_tui_with_1_and_says_so`: modelled on
      `a_lane_that_dies_ends_the_tui_with_exit_1_and_says_so`; the report names the `auto` lane.
    - `a_lane_that_ended_for_a_newer_store_is_not_started_again_after_ctrl_z`: the home's store
      schema is set above `SCHEMA_VERSION`, so the state sees `Upgraded`. After Ctrl-Z and the
      resume, the factory has been called once in all.
  - `lanes.rs` tests: `a_store_write_queued_when_the_store_turns_newer_never_starts`. The actions
    lane's source blocks its first `collect_due` on a gate; behind it are queued a second
    `Collect` and an `Act(Switch)`. `upgraded` is set, then the gate opens.
    - The second `Collect` is answered `Err` with `UPGRADED`, and the `Act` with exit 1 and
      `UPGRADED`.
    - `collect_due` ran once, and the host's `act` never ran.
    - An `Act(Doctor)` queued the same way still runs.
    - A queued index `Refresh` is answered `Err`, and `refresh_tokens` never ran.
  - `runtime.rs` tests: `a_writer_handed_the_terminal_after_the_store_turns_newer_never_runs`:
    with `upgraded` set, `Cmd::Suspend(Run(..))` never calls `act_foreground`, never releases the
    terminal, and delivers the exit-1 `UPGRADED` outcome.
  - `tui_cmd.rs`: `the_auto_lane_s_engines_record_auto_and_hold_as_a_tui_under_the_lane_s_token`.
    A `Context` over a temp home goes through `auto_engines(ctx, None)(token)`:
    - `engine.source()` is `Auto`;
    - `lock_holder()` is `Tui`;
    - `token.request(SIGTERM)` shows in `engine.cancel().requested()`.

- [ ] **Step 2: Run them and see them fail**

  Run: `cargo test -p tagteam-tui --locked auto_lane runtime::tests` and
  `cargo test -p tagteam --features tagteam/test-support --lib tui_cmd`. Expected: compile errors.

- [ ] **Step 3: Implement**

  - **The thread** (`tagteam-tui-auto`):
    - It builds the engine through the factory. `Err` → `LaneReport::Off`.
    - It runs `run_hosted` with `ThreadSleeper` and `uniform_jitter`. `settings_provider` is the
      TUI's `--provider`, else `engine.default_provider()` (Ruling 13).
    - Its sink converts each event and `on_status` each snapshot (Ruling 3), and sends each as a
      `Msg::Lane` on `tx`.
    - `Off` → `LaneReport::Off`; `Schema` → set the upgraded flag, then `LaneReport::Upgraded`.
    - A guard sends `Msg::LaneDied { lane: "auto" }` if the thread unwinds.
  - **`Lanes`**: `close` calls `auto.close()`, and `join` pushes `"auto"` when its join fails.
    `upgraded` is created in `Lanes::spawn` and shared with the auto lane, the actions lane and
    the index lane.
  - **The refusal at start** is one function, used by both lanes and `Ui::suspend`. It checks
    `upgraded` (`Ordering::SeqCst`) as each request is taken, before the source or host is
    called.
  - **`run_with`**: spawns the lane when `opts.auto` is set, and `Ui::new` calls
    `state.daemon.await_lane(now)` then.
  - **`Ui::stop`**: after `paused(None)` succeeds, `lanes.suspend_auto()` runs before
    `tty.stop()`. `lanes.resume_auto()` runs only on the `Ok` path, once not shutting down.
  - **The binary**: `auto_engines` clones the context with `env.cancel = token` and builds through
    `app::engine_for(.., EventSource::Auto)`, then `.with_lock_holder(HolderKind::Tui)`. The
    Keychain counts come from `daemon_cmd::keychain_counts`, over the context's Keychain and vault
    Keychain on macOS, and are empty on Linux, as `run_command` gathers them
    (`app/mod.rs:158-163`). `tui_cmd::run` passes `auto: Some(auto_engines(inside.clone(), provider.clone()))`.

- [ ] **Step 4: Run the focused tests, then the TUI and binary unit suites**

  Run: `cargo test -p tagteam-tui --locked` and
  `cargo test -p tagteam --features tagteam/test-support --lib`. Expected: PASS.

- [ ] **Step 5: Commit**: `Run auto-switching on a TUI lane that stops for Ctrl-Z and quit`

---

### Task 7: End to end on a pty, and the pty suites with the lane (S§4 "PTY")

**Depends on:** Task 6.

**Files:**
- Modify: `crates/tagteam/tests/tui_pty.rs`, `crates/tagteam/tests/common/mod.rs` (move
  `two_bearers`, `sent_with` and `usage_reply` from `daemon_procs.rs` into it, and use them there)
- Test: `crates/tagteam/tests/tui_pty.rs`

- [ ] **Step 1: Write the failing test**: `with_no_daemon_the_tui_switches_an_account_at_its_limit`

  Setup, in a `short_tempdir`:
  - `two_bearers`: `a` at 1, `b` at 2 and live;
  - `a`'s reading recorded fresh at 10 % with its next poll an hour away (`record_reading`);
  - notifications off;
  - a `MockServer` whose usage route answers `b`'s fetch at 100 % of the 5h window;
  - `tagteam tui` on a pty with `TAGTEAM_TEST_API_BASE` at the server and the fake `claude`
    first on `PATH`.

  Within `LONG`:
  - `live_email(root) == "a@x.co"`;
  - the screen shows `● auto on in this window`;
  - `locks/autoswitch-claude-code.lock` contains `"holder":"tui"`;
  - `tagteam history --json` lists the switch with source `auto`.

  Then `q`: exit 0, terminal restored, and the lock file is empty.

- [ ] **Step 2: Run it unsandboxed; it fails if anything above is missing, else passes**

  Run: `cargo test -p tagteam --features tagteam/test-support --test tui_pty with_no_daemon`.

- [ ] **Step 3: Run every pty and perf suite unsandboxed, and fix what the lane changed**

  Run: `cargo test -p tagteam --features tagteam/test-support --test tui_pty --test perf --test daemon_procs --test signals`.

  The lane now runs in every TUI test, against the offline API base. If a test's screen or timing
  now differs, change the test's fixture or expectation, never the lane, and say which test and
  why in the commit body. A test that needs no auto-switching gets a home with fewer than two
  accounts or the lane-free path the test is about. The daemon-first tests (`Home::daemon()`
  before `home.tui`) must keep live mode.

- [ ] **Step 4: Commit**: `Show the TUI switching an account at its limit end to end`

---

### Task 8: Records (S§5)

**Depends on:** Task 6 (the words are final).

**Files:**
- Modify: `docs/superpowers/specs/2026-10-14-tagteam-tui-design.md` (§1.1 criteria 4 and 6,
  §1.2's pause non-goal, §4.3, §7.3, §7.4),
  `docs/superpowers/specs/2026-10-20-tagteam-tui-redesign-design.md` (§5.2's header table, C-16,
  `P`'s row near line 835), `docs/superpowers/specs/2026-10-14-tagteam-daemon-design.md` (lines
  32-33, D7), `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md` (§11.1's one-engine
  rule, §6.4's `daemon.autoswitch` note), `README.md` ("Automatic switching", "Daemon"),
  `CHANGELOG.md` (`[Unreleased]`)

- [ ] **Step 1: Amend each passage S§5 names**

  Each passage changes in place, keeping its own voice, and is followed by
  `*(Amended by the TUI autoswitch spec.)*`, linking
  `2026-11-11-tagteam-tui-autoswitch-design.md`. The redesign's header table gains S§3.3's rows.

- [ ] **Step 2: README**

  - The open TUI switches when no daemon runs, and the daemon covers the hours it is closed.
  - `P` and `daemon.autoswitch` pause both.
  - `tagteam auto` is refused while a TUI switches, and the refusal says how to free it.

- [ ] **Step 3: CHANGELOG `[Unreleased]`**, in the file's voice

  - **Added:** the TUI switches while open with no daemon; `P` pauses it.
  - **Changed:**
    - `auto`'s refusal and `doctor` name a tagteam window holding the engine lock;
    - the TUI's header says when the daemon runs without its socket;
    - the TUI writes nothing once a newer tagteam migrated the store.

- [ ] **Step 4: Check**: `rg -n "Amended by the TUI autoswitch spec" docs/superpowers/specs` lists
  every passage of Step 1. Run `cargo test -p tagteam --features tagteam/test-support --test cli`,
  where README examples are checked, if the README's command lines changed.

- [ ] **Step 5: Commit**: `Record the TUI's own auto-switching in the specs, README and changelog`

---

### Task 9: The whole branch

**Depends on:** Tasks 1–8.

- [ ] **Step 1:** `cargo fmt --all --check`, and the three clippy runs in CONTRIBUTING.md with
  `-D warnings`. Expected: clean.
- [ ] **Step 2:** Run
  `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support` unsandboxed.
  Record passed/total. Expected: all pass.
- [ ] **Step 3:** cargo-deny (unsandboxed, the path in Global Constraints). Expected: no
  advisories beyond the ones `deny.toml` ignores.
- [ ] **Step 4:** Check that `crates/tagteam-daemon`'s pre-existing tests are unedited apart from
  Task 1's type adaptation: `git diff main -- crates/tagteam-daemon/src/provider_job.rs crates/tagteam-daemon/tests`
  shows no assertion change.
