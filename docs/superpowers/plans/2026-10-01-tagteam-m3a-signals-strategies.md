# tagteam M3a — Signals and Usage Strategies Implementation Plan

**Status:** In progress

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Ctrl-C, SIGTERM or SIGHUP never leaves Claude Code's lock directories behind, never
cuts a switch, a token refresh or a write in half, and ends an interrupted command with exit
130 (or 128 + the signal). `tagteam switch --strategy best|next-available [--model M]` picks
the target by usage. The two M1 carry-overs that a long-lived process (M3b's `auto`) needs land
first: the lock heartbeat touches through a held fd, and the Keychain file-mode pin lasts one
operation.

**Architecture:**
- **One cancel token per process, carried by `Env`.** Every lock wait already has an `Env` in
  reach, so `Env.cancel` reaches every cancellation point without new parameters on the
  `Provider` trait. The CLI registers SIGINT, SIGTERM and SIGHUP on it with `signal-hook`;
  nothing else ever stops the process. Lock waits, usage requests and prompts check it;
  critical spans never do (spec §14.1).
- **Usage strategies reuse the rotation's machinery.** Ranking is pure (`tagteam-core`), the
  engine collects on demand, orders candidates, walks vaults lazily, freshens and re-derives
  under the locks exactly as a bare rotation does (§9.3).
- **Non-interactive children leave the terminal's process group**, so a terminal Ctrl-C reaches
  only tagteam.

**Tech Stack:** Rust (edition 2024), `signal-hook` (new, `tagteam` crate only), `libc`
(`poll(2)` and termios for prompts, already a dependency; `rpassword` is removed), rusqlite,
clap 4, thiserror, tracing; tests with tempfile, assert_cmd, `ScriptedHttp`, the engine's
`test-hooks` crash points, and `openpty` pseudo-terminals.

**Spec:** `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`, with the M3
amendments of `a7a32c3` (Task 11: the M4 amendments of `1e79bb9`, the dead-token-marking
amendment of `59d5104` and the rollback amendment of `f686839`). Read §4.2, §4.3, §8.3, §9.1, §9.3, §9.4, §14, §14.1, §15.2,
Appendix A.3 and Appendix B (#23, #47, #53, #54) before starting any task. Section numbers below
refer to that spec.

## Execution notes

- Run this plan in the worktree `~/Code/tagteam-m3-auto-switch` (branch `m3-auto-switch`).
- **Base.** This branch sits on `main` at `3c1f458` (M2b merged), with the spec commits
  `a7a32c3` (M3), `1e79bb9` (M4), `59d5104` (the marking amendment) and `f686839` (the
  rollback amendment) on top. Before Task 1, rebase onto the current `main` if it
  has moved and run the full suite; if anything this plan's Interface Contract consumes has
  changed, stop and report it.
- When execution starts, set this plan's `**Status:**` to `In progress` in one commit. The spec
  stays `In progress` until M5.
- Task 12 is a human step: the live acceptance run is the last step before the merge request.
- **Task 11's rule comes from the M4 spec amendments,** signed off and committed as `1e79bb9`
  (§9.1 "The storage-write lock", §4.3, §7.5 step 5, §9.4 step 7, Appendix A.1/A.3, B #61),
  with Michael's dead-token-marking amendment `59d5104` (§9.1, B #61) and rollback amendment
  `f686839` (§9.1, §9.4 step 10, B #61).
  This branch carries that commit, so the spec also describes M4 behaviour this plan does not
  implement (`login_epoch` columns, profile bootstrap, `run`); M4's plan owns
  those, including Appendix A.2's dropped Keychain read fallbacks in `naming.rs`.
- Feature flags used by tests (unchanged from M2b): `tagteam-provider/file-keychain`,
  `tagteam-provider/mock-server`, `tagteam-engine/test-hooks`, `tagteam-cc/test-hooks`, and
  `tagteam/test-support`, which enables all of them.
- Clippy must pass both with `--features tagteam/test-support` and with no features. Every task
  runs `cargo fmt --all` before `cargo fmt --all --check`.
- **Task 6's pseudo-terminal tests need a real pty.** Claude Code's sandbox blocks
  `/dev/ptmx`, so inside it they fail loudly ("these tests need a pseudo-terminal, and openpty
  failed"); run them outside the sandbox. CI provides a pty. Task 6's first build also
  downloads `signal-hook`, which needs registry access.

## Milestones

| Milestone | Scope |
|---|---|
| M1, M2a, M2b | Implemented (M2b merged into `main` at `3c1f458`) |
| **M3a (this plan)** | Cancel token and signal handling (§14.1), cancellable lock waits, process groups, collector cancellation, prompts, exit codes; `switch --strategy best\|next-available [--model]` (§9.3); carry-overs L356 (heartbeat fd), L396 (file-mode pin), L302 (rotation edge tests); CC's `.storage-write` lock (from the M4 amendments) |
| M3b | Scheduled and re-check collection (§8.6), `primary_long_window` (§4.5), `decide()` and the simulations (§11.1, §11.5), the `auto` engine, tick, loop and CLI (§11) |
| M4 | `tagteam run`; `statusline`'s profile lookup |
| M5 | Export/import, `doctor`, `config` (writes), `displaced`, `purge`, `completions`, logging to file, `cargo xtask compat`, CI and release |

**Deliberately absent from M3a, and why that is safe:**
- **No `auto`.** §14.1's `auto` cancellation points (its sleep and the gap between ticks) land
  with the loop in M3b. Every other cancellation point lands here.
- **No session-owned candidates.** §9.3's "every strategy skips session-owned candidates" has
  nothing to skip until M4 creates sessions; the gate's `session_owned` stays `false`.
- **No temp-file sweep.** A SIGKILL can still leak an atomic writer's temp file (L319); its
  sweep is M5's `doctor`. Caught signals now unwind through the writer's existing `Drop` guard.

## Decisions

Rulings made while planning. Each names what it would cost if wrong.

1. **The token lives in `Env`** (`pub cancel: Cancel`), not in a new trait parameter or a
   process global. Every lock wait is reached through code that holds an `Env`, so
   `Provider::lock_credentials(env, g)` needs no new argument, and tests get an independent
   token per fixture. Cost if wrong: `Env` carries one non-path field.
2. **`signal-hook`'s `flag::register_usize`** stores the signal number into the token's cell.
   It is async-signal-safe and audited, so the workspace gains no hand-written handler. It
   restarts interrupted syscalls (`SA_RESTART`), so no wait relies on `EINTR`: every
   cancellation point polls. Cost: one dependency in the `tagteam` crate.
3. **A lock wait checks the token before every attempt**, the first included. A cancelled
   command therefore never takes a new lock; locks it already holds are released by `Drop`
   as it unwinds. Cost if wrong: none found; every critical span takes its locks before it
   starts (§14.1).
4. **One error kind, `interrupted`, three carriers.** `LockError::Interrupted { path, signal }`
   for lock waits (it names the lock, like every other `LockError`), `EngineError::Interrupted(i32)` for every other cancellation point, and
   `EngineError::signal()` reads the number from any of them (including
   `ProviderError::Lock`). The CLI maps it to exit `128 + signal` and `error.type`
   `interrupted`. Cost: a match arm per carrier, pinned by `kind_is_pinned_for_every_variant`.
5. **Prompts keep their signatures.** `TtyPrompter` holds the token. Every prompt (confirm,
   choose, secret) reads its own freshly opened `/dev/tty` description with `O_NONBLOCK`, so
   no read can block: it waits with `poll(2)` in 100 ms slices, checks the token each slice,
   and treats `WouldBlock` (a line flushed by Ctrl-C between readiness and the read) as "wait
   again". The shell's fd 0 description is never touched. An interrupted prompt answers as a
   decline (`false` / `None`) and discards what was typed; the CLI checks the token after every
   prompt and reports `interrupted`. The secret prompt is the same loop with `ECHO` off and
   `ISIG` on (so Ctrl-C arrives as SIGINT), restoring the terminal in a `Drop` guard; it
   replaces rpassword, whose read could not be cancelled (it restarts under `SA_RESTART`).
   Cost: about 150 lines of terminal code with pseudo-terminal tests, and one dependency fewer.
6. **A late signal** (one that arrived but met no cancellation point because the work finished
   first) leaves the result and exit code alone and adds one stderr line:
   `tagteam: interrupted too late to stop: <command> had already finished`. Under `--json`
   stdout is unchanged (§14.1).
7. **`--strategy` and `ACCOUNT` are mutually exclusive, and `--model` requires `--strategy`**
   (§9.3, cswap's rule). Both are clap usage errors, exit 2.
8. **A usage strategy collects the live account and every rotation candidate** (`is_candidate`:
   enabled, unquarantined, with an identity), on demand (§8.3), before planning. Candidates
   that are not due keep their readings. Cost if wrong: one request per candidate whose reading
   is older than 180 s, within the budget.
9. **Unbound quarantines are released under each account's lock, try-only.** A quarantine is
   released when the vault holds a different generation from `quarantine_fp` and, for the live
   account, the live credential does too (§7.4). A busy account lock leaves the quarantine for
   the next caller. M3b's tick reuses the same function.
10. **`best` orders every known candidate that beats the live headroom** (all known ones when
    the live headroom is unknown or there is no live login), most headroom first, ties to the
    lower position, and walks that order reading vaults lazily, as rotation does.
11. **Under the locks, a usage strategy's pick stands while it is still a candidate and the live
    login names the same account as at planning**; otherwise the strategy plans again from the
    store, with no network (§9.3, §9.4 step 1). M1's double-fire rule (the live login is now
    the pick) applies unchanged.
12. **§7.5's publish is inside its critical span.** The successor is written to the live store
    "in either case, so CC always holds the newest generation" (§7.5 step 5), so the config-lock
    wait `publish` makes after the token request is not a cancellation point: a signal there
    would leave CC on the consumed generation. That one wait uses a token that is never set
    (Task 2). A config-lock *timeout* there keeps today's behaviour (not published; the next pass
    reconciles). The same reasoning puts no cancellation point between a recovery's start and
    its commit; a recovery whose *lock wait* is interrupted reports `interrupted` at once rather
    than `interrupted-switch` (Task 2, `guard_recovering`).
13. **Writes inside a critical span wait for `.storage-write` on a token nothing sets**
    (Task 11). `Engine::critical_env()` returns the engine's `Env` with such a token; the
    switch's step-7 write, recovery's writes and every restore use it, and §7.5's publish after
    its request reuses Task 2's fresh-token `Env`. The self-heal publish, made before any
    request, is the only storage-write wait a signal can end. M3b's auto switch inherits this
    through the same `apply`. Cost if wrong: a stuck `.storage-write` holds the command for its
    9 s budget even after a Ctrl-C.
14. **§9.1's "last read or wrote" lives in the `LiveStore`, beside Task 7's pin**, recorded per
    operation and cleared when the operation's credential locks drop (`OperationLocks`). A
    restore touches only entries its operation wrote, and writes over CC's dead-token marking of
    them (`59d5104`). Cost if wrong: one `ClaudeCode` serves one secure-storage dir per process;
    M4's profile bootstrap must use the profile's own `CcPaths` and locks.
15. **A rollback never merges** (Michael, after plan review round 7). A restore puts a place
    back byte for byte to the state it held just before tagteam's write there (read under
    `.storage-write`), and only while the place still holds exactly what tagteam wrote. A
    place anyone has touched since is left as it is, and the rollback ends `rollback-failed`
    with the journal row kept, so §9.6 recovery decides from the live credential (§9.4
    step 10). Cost: a rare double failure (a step fails after CC wrote the same entry) ends
    with the switch finished forward on the next command instead of rolled back, but no CC
    write is ever at risk.
16. **L303 is already resolved:** `rotation_order` sorts and dedups its input (`rotation.rs`), and
    says so in its doc comment. Nothing to do.

## Global Constraints

Every task's requirements include these. Values are copied from the spec.

- Platforms: macOS and Linux only (§1.2). Rust edition 2024, MSRV ≥ 1.85, toolchain pinned by
  `rust-toolchain.toml` (§16). `#![forbid(unsafe_code)]` stays in `tagteam-core`,
  `tagteam-engine` and `tagteam-fake`; `unsafe` is allowed only in `tagteam`,
  `tagteam-provider` and `tagteam-cc`, with a `// SAFETY:` comment on every block.
- Signals: SIGINT, SIGTERM and SIGHUP are caught; SIGKILL and SIGQUIT keep their default
  disposition (§14.1). Exit `130` after SIGINT, `128 + n` after SIGTERM (143) or SIGHUP (129)
  (§13.1).
- No cancellation point inside a critical span (§14.1): the switch from its journal row to
  commit or rollback (§9.4 steps 6–10); recovery's writes (§9.6); the gate (§7.3) and §7.5 from
  sending the token request to persisting the successor; an explicit replacement's three steps
  (§12.5); any vault, rescue or atomic write.
- Lock order is unchanged (§4.3): mutation lock → account locks (ascending ID) → provider live
  locks (credential locks → config lock). No network while holding a contended lock, except the
  gate's 10 s and §7.5's 6 s requests.
- `--json` stdout is exactly one JSON object; warnings and notices go to stderr (§13.2).
- Secrets never reach `Debug`, logs or error messages. Log lines identify accounts by position
  and ID, never by email (§4.4).
- A usage failure is never a command error (§8.3); an interrupted usage fetch is not a usage
  failure: it records nothing and gives its slot back (§14.1).
- Tests never touch the real HOME, the login keychain, or the network (§15.1).
- Commits: small, imperative mood, no license headers, no agent attribution of any kind.

## Review Focus

Inputs and conditions the spec implies but no feature test would naturally hit. Each has a
pinning test in the task named.

1. **Ctrl-C while `tagteam switch` waits for CC's credential locks** (Claude Code is refreshing
   and holds them). Expected: the command ends within about half a second with exit 130,
   nothing is written, and no lock directory tagteam created is left behind. → Task 2 (engine)
   and Task 6 (a real SIGINT to the binary).
2. **Ctrl-C pressed twice while a switch writes the live credential.** Expected: the switch
   commits and is reported as switched, exit 0, with the late-signal notice; no journal row
   remains. → Task 6.
3. **Ctrl-C at the `Add the current login (<email>) first? [Y/n]` prompt or the secret prompt of
   `add-token`.** Expected: exit 130, nothing added, the terminal's echo restored. → Task 6
   (the exit code, nothing written, the prompt's wait) and Task 12 (echo restored, checked by
   hand: it needs a real terminal).
4. **A double-fired `switch --strategy best`** (a hotkey pressed twice). Expected: one switch;
   the second command reports `already-active` and never moves on to a third account. → Task 9.
5. **A process suspended past a CC lock's staleness while holding it** (laptop lid closed).
   Expected: on resume it never touches or removes the directory that replaced its own, and it
   aborts the protected write. → Task 3.
6. **Claude Code writes its credential entry while a switch writes it** (an MCP OAuth update,
   or its dead-token marking). Expected: neither write is lost. tagteam waits for CC's
   `.storage-write`, keeps CC's machine-shared keys, writes over a marking, and aborts (rolling
   back) on any other change to the account. → Task 11.

---

## File Structure

```
Cargo.toml                                  + signal-hook, − rpassword (workspace dependencies)
crates/tagteam-provider/
  src/cancel.rs                  NEW        Cancel, Interrupted (Task 1)
  src/lib.rs                     MOD        `pub mod cancel`, re-exports (Task 1)
  src/env.rs                     MOD        `Env.cancel` (Task 1)
  src/flock.rs                   MOD        cancellable `FlockGuard::lock`, `MutationGuard` (Task 1)
  src/mkdir_lock.rs              MOD        `LockError::Interrupted`, `MkdirLockSpec.cancel` (Task 1);
                                            heartbeat through a held fd, dev/ino check (Task 3)
  src/security.rs                MOD        children in their own process group (Task 4)
  src/provider.rs                MOD        `SecretStore::Fallback` doc line (Task 7);
                                            `ProviderError::EntryMoved` (Task 11)
crates/tagteam-cc/
  src/locks.rs                   MOD        cancellable credential and config locks (Task 2);
                                            `acquire_storage_write` (Task 11)
  src/paths.rs                   MOD        `CcPaths.storage_write_lock` (Task 11)
  src/provider.rs                MOD        pass `env.cancel` (Task 2); `OperationLocks`, the
                                            file-mode pin's scope (Task 7)
  src/live.rs                    MOD        `unpin_file_mode`; `restore` clears the pin (Task 7);
                                            the lock and re-read around every entry write (Task 11)
  tests/{provider,live_store}.rs MOD        (Tasks 3, 7, 11)
crates/tagteam-fake/
  src/provider.rs                MOD        pass `env.cancel` to its live lock (Task 2)
crates/tagteam-core/
  src/rank.rs                    NEW        best order, next-available filter, binding window,
                                            `span` (Task 8)
  src/lib.rs                     MOD        `pub mod rank` (Task 8)
crates/tagteam-engine/
  src/account_lock.rs            MOD        waits on `env.cancel` (Task 1)
  src/error.rs                   MOD        `Interrupted`, `signal()`, kind `interrupted` (Task 2)
  src/engine.rs                  MOD        `Engine::cancel`; `guard_recovering` returns an
                                            interrupted recovery (Task 2); `critical_env` (Task 11)
  src/active.rs                  MOD        §7.5's publish waits on an unset token (Task 2),
                                            and passes it to the live write (Task 11)
  src/collect.rs                 MOD        cancellation points (Task 5)
  src/hooks.rs                   MOD        test-only pause point for binary tests (Task 6)
  src/quarantine.rs              MOD        `release_unbound_quarantines`, the shared §7.4
                                            predicate (Task 9)
  src/refresh.rs                 MOD        the gate's release uses the shared predicate (Task 9)
  src/views.rs                   MOD        `decision_windows` (Task 9)
  src/switch.rs                  MOD        usage strategies; `switch_planned` (Task 9);
                                            `apply` writes under `critical_env` (Task 11)
  src/recover.rs                 MOD        recovery's writes under `critical_env` (Task 11)
  tests/{cancel,strategy}.rs     NEW
  tests/storage_write.rs         NEW        (Task 11)
  tests/common/mod.rs            MOD        `mutation_lock_free` probes under a fresh token (Task 2)
  tests/{active,collect,rotation,switch,switch_rollback,views_usage}.rs MOD
crates/tagteam/
  Cargo.toml                                + signal-hook, − rpassword (Task 6)
  src/signals.rs                 NEW        handler registration (Task 6)
  src/lib.rs, src/main.rs        MOD        install handlers; exit codes (Task 6)
  src/app.rs                     MOD        interrupted mapping, late notice, prompts (Task 6);
                                            `--strategy`, `--model` (Task 10)
  src/prompt.rs                  MOD        poll-based reads (Task 6)
  src/cli.rs                     MOD        `--strategy`, `--model` (Task 10)
  src/render.rs                  MOD        strategy reasons and warnings; `duration` re-exports
                                            `rank::span` (Task 10)
  tests/{signals,strategy_cli}.rs NEW       (Tasks 6, 10; `signals.rs` gains a test in Task 11)
  tests/app.rs                   MOD        `H::run_with_cancel`, built on M2b's `H::run_in` (Task 6)
```

---

## Interface Contract

Every task implements exactly these names and signatures. A task that finds one unworkable
stops and reports it rather than inventing a variant, because other tasks' code is written
against this list.

### `tagteam-provider`

**`src/cancel.rs`** (Task 1), re-exported at the crate root:

```rust
/// §14.1: the signal the process received, shared by every clone. Production registers the
/// CLI's handlers on its cell; tests call `request` directly. `0` in the cell means none.
#[derive(Debug, Clone, Default)]
pub struct Cancel { /* signal: Arc<AtomicUsize> */ }

impl Cancel {
    pub fn new() -> Self;
    /// Records `signal` (a later one replaces it, as a handler's store does).
    pub fn request(&self, signal: i32);
    pub fn requested(&self) -> Option<i32>;
    /// `Err(Interrupted(n))` once a signal is recorded.
    pub fn check(&self) -> Result<(), Interrupted>;
    /// The cell a signal handler stores the signal number into.
    pub fn cell(&self) -> Arc<AtomicUsize>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("interrupted by signal {0}")]
pub struct Interrupted(pub i32);
```

**`src/env.rs`** (Task 1): `Env` gains `pub cancel: Cancel`. `Env::from_process` and
`Env::for_test` set `Cancel::new()`; clones share it.

**`src/mkdir_lock.rs`** (Task 1, Task 3):

```rust
pub enum LockError {
    Timeout(PathBuf),
    Compromised(PathBuf),
    Io(io::Error),
    /// The cancel token was set while waiting (§14.1).
    #[error("interrupted while waiting for the lock {path}")]   // wording settled by Task 1
    Interrupted { path: PathBuf, signal: i32 },
}
impl LockError { pub fn signal(&self) -> Option<i32>; }   // Some only for Interrupted

pub struct MkdirLockSpec {
    pub path: PathBuf,
    pub stale: Duration,
    pub acquire_timeout: Duration,
    pub touch_every: Duration,
    pub cancel: Cancel,             // Cancel::new() from `new`; set with `with_cancel`
}
impl MkdirLockSpec {
    pub fn new(path: PathBuf, stale: Duration, acquire_timeout: Duration) -> Self;
    pub fn with_cancel(self, cancel: &Cancel) -> Self;
}
// `MkdirLock::acquire(spec)` checks `spec.cancel` before every attempt (Decision 3).
// Task 3: the heartbeat touches through a directory fd held since acquisition, and
// `check_owned` also compares the path's device and inode with that fd's.
```

**`src/flock.rs`** (Task 1):

```rust
impl FlockGuard {
    pub fn try_lock(path: &Path) -> io::Result<Option<Self>>;                       // unchanged
    pub fn lock(path: &Path, timeout: Duration, cancel: &Cancel) -> Result<Self, LockError>;
}
impl MutationGuard {
    pub fn acquire(env: &Env, timeout: Duration) -> Result<Self, LockError>;       // waits on env.cancel
}
```

**`src/security.rs`** (Task 4): `ProcessRunner`'s non-interactive spawn calls
`CommandExt::process_group(0)`; `run_attached` is unchanged (stays in the foreground group).

### `tagteam-cc`

**`src/locks.rs`** (Task 2):

```rust
pub fn acquire_credentials(paths: &CcPaths, timeout: Duration, cancel: &Cancel) -> Result<CcCredSet, LockError>;
pub fn acquire_config(paths: &CcPaths, timeout: Duration, cancel: &Cancel) -> Result<CcConfigSet, LockError>;
```

`ClaudeCode::lock_credentials` / `lock_config` pass `&env.cancel`.

### `tagteam-core`

**`src/rank.rs`** (Task 8):

```rust
/// One candidate as a usage strategy sees it: its position and its decision-grade headroom
/// (`None`: unknown, §8.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate { pub position: u32, pub headroom: Option<f64> }

#[derive(Debug, Clone, PartialEq)]
pub enum BestOrder {
    /// Positions to try in order: known headroom strictly above `live` (every known one when
    /// `live` is None), most headroom first, ties to the lower position.
    Try(Vec<u32>),
    /// The live headroom is known and no known candidate beats it.
    AlreadyBest,
    /// No candidate has a known headroom.
    UsageUnavailable,
}
pub fn best_order(live: Option<f64>, candidates: &[Candidate]) -> BestOrder;

#[derive(Debug, Clone, PartialEq)]
pub enum NextAvailable {
    /// `walk` (rotation order) minus candidates whose known headroom is ≤ 0.
    Try { order: Vec<u32>, skipped: Vec<u32> },
    /// Every candidate in a non-empty walk is known to be exhausted.
    Exhausted,
}
pub fn next_available(walk: &[Candidate]) -> NextAvailable;

/// The relevant window with the highest pct (§8.2): what binds the headroom. Ties go to the
/// earlier window in `windows`.
pub fn binding_window<'w>(windows: &'w [Window], models: &[String]) -> Option<&'w Window>;

/// A duration as the CLI shows it (`2h40m`, `3d09h`, `14m`, `<1m`): the one formatter, which
/// `candidates-exhausted`'s message uses and the CLI's `render::duration` re-exports (Task 10).
pub fn span(secs: i64) -> String;
```

### `tagteam-engine`

**`src/error.rs`** (Task 2):

```rust
pub enum EngineError {
    // …existing variants…
    #[error("interrupted")]
    Interrupted(i32),
}
impl EngineError {
    /// The signal behind an interruption, whichever carrier holds it: `Interrupted`,
    /// `Lock(LockError::Interrupted)`, or `Provider(ProviderError::Lock(LockError::Interrupted))`.
    pub fn signal(&self) -> Option<i32>;
}
// kind() == "interrupted" for all three.
```

`Engine::cancel(&self) -> &Cancel` returns `&self.env.cancel` (Task 2). `AccountLock::acquire`
keeps its signature and waits on `env.cancel` (Task 1, forced by `FlockGuard::lock`'s new
parameter). `guard_recovering` returns a recovery's interrupted lock wait as is, instead of
reporting `interrupted-switch` (Task 2, Decision 12).

**`src/collect.rs`** (Task 5): `collect_usage` returns `Err(EngineError::Interrupted(n))` when
the token was set during the collection, after every thread has joined and given back any slot
it held unsent. An interrupted account records nothing.

**`src/quarantine.rs`** (Task 9):

```rust
impl Engine {
    /// §7.4 / Decision 9: clears every quarantine of `provider` that no longer binds, each under
    /// its account lock (try-only; a busy account is left), recording each release with
    /// `source` ("cli" for a switch, "auto" for M3b's tick). Returns the released accounts.
    pub fn release_unbound_quarantines(&self, provider: &ProviderId, source: &'static str)
        -> Result<Vec<AccountId>, EngineError>;
}
```

The refresh gate's own release (`refresh.rs`, `quarantine_released`) uses the same predicate
(Task 9): for the live account, a quarantine stands while the live credential still carries
`quarantine_fp`, as §7.4 says.

**`src/views.rs`** (Task 9):

```rust
impl Engine {
    /// The account's last good windows if they are decision-grade (§8.4) under `models`'
    /// relevance; `None` otherwise. Reads the store only.
    pub fn decision_windows(&self, row: &AccountRow, models: &[String]) -> Result<Option<Vec<Window>>, EngineError>;
}
```

**`src/switch.rs`** (Task 9):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageStrategy { Best, NextAvailable }
impl UsageStrategy { pub fn as_str(self) -> &'static str; }   // "best" | "next-available"

pub enum SwitchTarget {
    Rotation,
    Account(AccountId),
    /// §9.3: `models` overrides `autoswitch.models` for this switch (`--model`).
    Usage { strategy: UsageStrategy, models: Option<Vec<String>> },
}

pub enum SwitchReason {
    // …existing…
    UsageUnavailable,     // "usage-unavailable"
    AlreadyBest,          // "already-best"
    CandidatesExhausted,  // "candidates-exhausted"
}
// SwitchOutcome.strategy is "best" | "next-available" for a Usage target.
```

### `tagteam` (CLI)

```rust
// src/signals.rs (Task 6)
/// Registers SIGINT, SIGTERM and SIGHUP to store their number in `cancel`'s cell.
pub fn install(cancel: &Cancel) -> std::io::Result<()>;

// src/prompt.rs (Task 6)
impl TtyPrompter { pub fn new(cancel: Cancel) -> Self; }

// src/app.rs (Task 6)
pub(crate) const KIND_INTERRUPTED: &str = "interrupted";

// src/cli.rs (Task 10)
Switch {
    account: Option<String>,
    #[arg(long)] force: bool,
    /// best or next-available
    #[arg(long, value_enum, conflicts_with = "account")] strategy: Option<StrategyArg>,
    /// Model limits that count, comma-separated, or `all`
    #[arg(long, requires = "strategy")] model: Option<String>,
},
#[derive(Clone, Copy, clap::ValueEnum)] pub enum StrategyArg { Best, NextAvailable }
```

### Additions for Task 11 (CC's storage-write lock)

```rust
// tagteam-provider, src/provider.rs
pub enum ProviderError {
    // …existing variants…
    /// Under the storage-write lock, an entry's account-scoped keys were no longer what tagteam
    /// last read or wrote, and not merely by the agent's dead-token marking (§9.1). The name is
    /// a Keychain service or a file path, never bytes. `kind()`: `provider`.
    #[error("{0} was changed by another writer during tagteam's write; it was left as it is")]
    EntryMoved(String),
}

// tagteam-cc
pub struct CcPaths { /* … */ pub storage_write_lock: PathBuf }   // <secure-storage dir>/.storage-write
pub const STORAGE_WRITE_STALE: Duration = Duration::from_secs(15);          // locks.rs
pub fn acquire_storage_write(paths: &CcPaths, timeout: Duration, cancel: &Cancel)
    -> Result<MkdirLock, LockError>;                                         // locks.rs
impl LiveStore {
    pub fn with_storage_write_timeout(self, d: Duration) -> Self;  // default locks::ACQUIRE_TIMEOUT
    pub(crate) fn end_operation(&self);   // clears Task 7's pin and the §9.1 record
}

// tagteam-engine
impl Engine { pub(crate) fn critical_env(&self) -> Env; }   // the Env with a token nothing sets
```

The `Provider` trait and `FakeAgent` do not change.

---

## Tasks

| # | Task | Human? |
|---|---|---|
| 1 | Cancel token and cancellable lock primitives | |
| 2 | Every lock wait honours the token; the `interrupted` error | |
| 3 | The CC lock heartbeat touches through a held fd (L356) | |
| 4 | `security` children run in their own process group | |
| 5 | Collector cancellation points | |
| 6 | Signal handlers, prompts, exit codes and the late-signal notice | |
| 7 | The Keychain file-mode pin lasts one operation (L396) | |
| 8 | Usage ranking in core | |
| 9 | Usage strategies in the engine | |
| 10 | `switch --strategy` and `--model` | |
| 11 | Claude Code's `.storage-write` lock around every credential-entry write | |
| 12 | Final verification and live acceptance | Yes |

### Task 1: Cancel token and cancellable lock primitives

§14.1: "SIGINT, SIGTERM and SIGHUP never stop a command at the instruction they arrive on. The
CLI's handler only records the signal in the engine's cancel token (§4.2), which tests set
directly." Its first cancellation point is "each iteration of a lock wait: the mutation lock,
account locks, and the provider's live locks". This task builds the token and makes the two
wait loops every lock goes through, `MkdirLock::acquire` and `FlockGuard::lock`, honour it.
The tagteam mutation lock and the account locks get it in the same change, because both waits
already have an `Env` in reach. Claude Code's and FakeAgent's waits stay on a token that nothing
sets until Task 2 passes theirs through.

**Readings of the spec this task commits to:**
- **"Each iteration of a lock wait" means before every attempt, the first included**
  (Decision 3). With the token set, a wait makes no attempt at all. It takes no lock, and a
  `flock` wait does not even create the lock file or its directory, because `try_lock` is what
  creates them.
- **A try is not a wait.** `MkdirLock::try_acquire` and `FlockGuard::try_lock` make one attempt
  and never check the token.
- **The interrupted error names the lock it waited for**, like `Timeout`, and carries the
  signal: `LockError::Interrupted { path, signal }`. `LockError::signal()` is `Some` only for
  that variant. Its Display is `interrupted while waiting for the lock <path>`.
- **A set token stays set.** Nothing resets it: once a signal arrives, every later wait in the
  process ends at once. Each `Env::for_test` has its own token, so tests stay independent;
  clones of one `Env` share theirs.
- **The token is checked before each attempt, and the deadline after a failed one.** A token
  set during the final sleep therefore ends the wait as `Interrupted`, not `Timeout`.

**Files:**
- Create: `crates/tagteam-provider/src/cancel.rs`
- Modify: `crates/tagteam-provider/src/lib.rs`
- Modify: `crates/tagteam-provider/src/env.rs` (the field, and its `mod tests`)
- Modify: `crates/tagteam-provider/src/mkdir_lock.rs` (`LockError`, `MkdirLockSpec`,
  `MkdirLock::acquire`, and its `mod tests`)
- Modify: `crates/tagteam-provider/src/flock.rs` (`FlockGuard::lock`, `MutationGuard::acquire`,
  and its `mod tests`)
- Modify: `crates/tagteam-engine/src/account_lock.rs` (the one other `FlockGuard::lock` caller;
  `rg -n 'FlockGuard::lock' crates/` lists `flock.rs` and `account_lock.rs` only)

**Interfaces:**
- Consumes: `Env::for_test`, `MkdirLock`, `MkdirLockSpec::new`, `FlockGuard::try_lock`,
  `MutationGuard::{acquire, TIMEOUT}`, `crate::FORK_GUARD` (the lib tests that re-lock a flock
  take it).
- Produces:
  - `tagteam_provider::cancel::{Cancel, Interrupted}`, re-exported at the crate root:
    `Cancel::new() -> Self`, `Cancel::request(&self, signal: i32)`,
    `Cancel::requested(&self) -> Option<i32>`, `Cancel::check(&self) -> Result<(), Interrupted>`,
    `Cancel::cell(&self) -> Arc<AtomicUsize>`, and `pub struct Interrupted(pub i32)`
  - `Env.cancel: Cancel` (`Env::from_process` and `Env::for_test` set `Cancel::new()`)
  - `LockError::Interrupted { path: PathBuf, signal: i32 }`,
    `LockError::signal(&self) -> Option<i32>`
  - `MkdirLockSpec.cancel: Cancel`, `MkdirLockSpec::with_cancel(self, cancel: &Cancel) -> Self`
  - `FlockGuard::lock(path: &Path, timeout: Duration, cancel: &Cancel) -> Result<Self, LockError>`
  - `MutationGuard::acquire(env: &Env, timeout: Duration)` and
    `AccountLock::acquire(env: &Env, id: &AccountId, wait: Duration)`: signatures unchanged;
    both wait on `env.cancel`
  - private to the crate: `mkdir_lock::check_cancel(cancel: &Cancel, path: &Path) -> Result<(), LockError>`

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-provider/src/cancel.rs` with only its doc line and its tests (Step 3 adds
the code between them):

```rust
//! §14.1's cancel token: what a caught signal leaves for the next cancellation point.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_token_is_unset() {
        let c = Cancel::new();
        assert_eq!(c.requested(), None);
        assert_eq!(c.check(), Ok(()));
    }

    #[test]
    fn every_clone_sees_a_request_and_a_later_one_replaces_it() {
        let c = Cancel::new();
        let clone = c.clone();
        c.request(2);
        assert_eq!(clone.requested(), Some(2));
        assert_eq!(clone.check(), Err(Interrupted(2)));
        clone.request(15);
        assert_eq!(c.requested(), Some(15));
    }

    #[test]
    fn separate_tokens_never_share_a_signal() {
        let (a, b) = (Cancel::new(), Cancel::new());
        a.request(2);
        assert_eq!(b.requested(), None);
    }

    #[test]
    fn a_store_into_the_cell_is_a_request() {
        // What a signal handler does (Task 6): it stores the number and nothing else.
        let c = Cancel::new();
        c.cell().store(1, Ordering::SeqCst);
        assert_eq!(c.requested(), Some(1));
    }

    #[test]
    fn an_interruption_names_its_signal() {
        assert_eq!(Interrupted(15).to_string(), "interrupted by signal 15");
    }
}
```

In `crates/tagteam-provider/src/lib.rs`, replace

```rust
pub mod atomic;
pub mod clock;
```

with

```rust
pub mod atomic;
pub mod cancel;
pub mod clock;
```

In `crates/tagteam-provider/src/env.rs`, inside `mod tests`, add before the `#[test]` line of
`a_run_shell_is_detected_from_claude_config_dir`:

```rust
    #[test]
    fn clones_share_the_cancel_token_and_fixtures_never_do() {
        let env = Env::for_test(Path::new("/tmp/fixture"));
        let clone = env.clone();
        env.cancel.request(15);
        assert_eq!(clone.cancel.requested(), Some(15));
        let other = Env::for_test(Path::new("/tmp/fixture"));
        assert_eq!(other.cancel.requested(), None);
    }

```

In `crates/tagteam-provider/src/mkdir_lock.rs`, inside `mod tests`, replace the `spec` helper:

```rust
    fn spec(dir: &Path, stale_ms: u64, timeout_ms: u64, touch_ms: u64) -> MkdirLockSpec {
        MkdirLockSpec {
            path: dir.join("x.lock"),
            stale: Duration::from_millis(stale_ms),
            acquire_timeout: Duration::from_millis(timeout_ms),
            touch_every: Duration::from_millis(touch_ms),
        }
    }
```

with:

```rust
    fn spec(dir: &Path, stale_ms: u64, timeout_ms: u64, touch_ms: u64) -> MkdirLockSpec {
        MkdirLockSpec {
            path: dir.join("x.lock"),
            stale: Duration::from_millis(stale_ms),
            acquire_timeout: Duration::from_millis(timeout_ms),
            touch_every: Duration::from_millis(touch_ms),
            cancel: Cancel::new(),
        }
    }

    /// Sets `cancel` from another thread 200 ms from now, as a signal handler would, and
    /// returns the instant just before it did.
    fn interrupt_soon(cancel: &Cancel, signal: i32) -> std::thread::JoinHandle<Instant> {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let at = Instant::now();
            cancel.request(signal);
            at
        })
    }

    #[test]
    fn a_set_token_ends_the_wait_before_its_first_attempt() {
        let d = tempfile::tempdir().unwrap();
        let cancel = Cancel::new();
        cancel.request(2);
        let s = spec(d.path(), 60_000, 5_000, 3_000).with_cancel(&cancel);
        let start = Instant::now();
        match MkdirLock::acquire(&s) {
            Err(LockError::Interrupted { path, signal }) => {
                assert_eq!((path, signal), (s.path.clone(), 2))
            }
            Err(e) => panic!("expected an interrupted wait, got {e:?}"),
            Ok(_) => panic!("a set token took the lock"),
        }
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "no poll was waited"
        );
        assert!(
            !s.path.exists(),
            "no attempt was made, so nothing was taken"
        );
    }

    #[test]
    fn an_unset_token_waits_and_acquires_as_before() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 600, 3_000).with_cancel(&Cancel::new());
        let held = MkdirLock::acquire(&s).unwrap();
        assert!(matches!(MkdirLock::acquire(&s), Err(LockError::Timeout(_))));
        drop(held);
        assert!(MkdirLock::acquire(&s).is_ok());
    }

    #[test]
    fn a_token_set_while_another_holder_keeps_the_lock_ends_the_wait_within_one_poll() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 30_000, 3_000);
        let holder = MkdirLock::acquire(&s).unwrap();
        let cancel = Cancel::new();
        let setter = interrupt_soon(&cancel, 15);
        let result = MkdirLock::acquire(&s.clone().with_cancel(&cancel));
        let ended = Instant::now();
        let set_at = setter.join().unwrap();
        assert!(
            matches!(&result, Err(LockError::Interrupted { path, signal: 15 }) if *path == s.path),
            "{:?}",
            result.as_ref().err()
        );
        assert!(ended >= set_at, "the wait ended before the token was set");
        // One jittered poll is at most 500 ms; the timeout is 30 s.
        assert!(
            ended - set_at < Duration::from_secs(1),
            "{:?}",
            ended - set_at
        );
        assert!(
            holder.check_owned().is_ok(),
            "the holder's lock is untouched"
        );
    }

    #[test]
    fn only_an_interrupted_wait_carries_a_signal() {
        let p = PathBuf::from("/x.lock");
        let e = LockError::Interrupted {
            path: p.clone(),
            signal: 1,
        };
        assert_eq!(e.signal(), Some(1));
        assert_eq!(
            e.to_string(),
            "interrupted while waiting for the lock /x.lock"
        );
        assert_eq!(LockError::Timeout(p.clone()).signal(), None);
        assert_eq!(LockError::Compromised(p).signal(), None);
        assert_eq!(LockError::Io(io::Error::other("x")).signal(), None);
    }
```

In `crates/tagteam-provider/src/flock.rs`, inside `mod tests`, in
`a_second_lock_on_the_same_file_is_refused_until_release`, replace

```rust
            FlockGuard::lock(&p, Duration::from_millis(250)),
```

with

```rust
            FlockGuard::lock(&p, Duration::from_millis(250), &Cancel::new()),
```

and add these tests at the end of the module, after `the_mutation_guard_lives_in_the_data_dir`:

```rust
    #[test]
    fn a_set_token_ends_a_flock_wait_before_its_first_attempt() {
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/x.lock");
        let cancel = Cancel::new();
        cancel.request(2);
        match FlockGuard::lock(&p, Duration::from_secs(10), &cancel) {
            Err(LockError::Interrupted { path, signal }) => {
                assert_eq!((path, signal), (p.clone(), 2))
            }
            other => panic!("expected an interrupted wait, got {other:?}"),
        }
        assert!(
            !p.exists(),
            "no attempt was made, so nothing was created or locked"
        );
    }

    #[test]
    fn a_token_set_while_another_holder_keeps_the_flock_ends_the_wait_within_one_poll() {
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.lock");
        let held = FlockGuard::try_lock(&p).unwrap().unwrap();
        let cancel = Cancel::new();
        let setter = {
            let cancel = cancel.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(200));
                let at = Instant::now();
                cancel.request(15);
                at
            })
        };
        let result = FlockGuard::lock(&p, Duration::from_secs(30), &cancel);
        let ended = Instant::now();
        let set_at = setter.join().unwrap();
        assert!(
            matches!(&result, Err(LockError::Interrupted { signal: 15, .. })),
            "{result:?}"
        );
        assert!(ended >= set_at, "the wait ended before the token was set");
        // One poll is 100 ms; the timeout is 30 s.
        assert!(
            ended - set_at < Duration::from_millis(500),
            "{:?}",
            ended - set_at
        );
        assert!(
            FlockGuard::try_lock(&p).unwrap().is_none(),
            "the holder keeps it"
        );
        drop(held);
    }

    #[test]
    fn the_mutation_guard_waits_on_the_envs_token() {
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let held = MutationGuard::acquire(&env, Duration::from_millis(100)).unwrap();
        env.cancel.request(1);
        let start = Instant::now();
        let err = MutationGuard::acquire(&env, MutationGuard::TIMEOUT).unwrap_err();
        assert_eq!(err.signal(), Some(1), "{err}");
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "checked before the first attempt"
        );
        drop(held);
        // The free lock is still refused under the set token, by every clone of the Env; an Env
        // with its own token takes it.
        assert!(MutationGuard::acquire(&env.clone(), Duration::ZERO).is_err());
        assert!(MutationGuard::acquire(&Env::for_test(d.path()), Duration::ZERO).is_ok());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-provider --lib`
Expected: FAIL to compile. Among the errors: `cannot find type Cancel` (in `cancel.rs`'s tests,
`mkdir_lock.rs` and `flock.rs`), `cannot find type Interrupted`, `no field cancel on type Env`
and `&Env`, `no method named with_cancel found for struct MkdirLockSpec`,
`no variant named Interrupted` on `LockError`, `no method named signal`, and `this function
takes 2 arguments but 3 arguments were supplied` for `FlockGuard::lock`. The pre-existing tests
are unchanged in substance and would still pass.

- [ ] **Step 3: Implement the token and check it in both wait loops**

In `crates/tagteam-provider/src/cancel.rs`, insert between the doc line and `#[cfg(test)]`:

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// §14.1: the signal the process received, shared by every clone. Production registers the
/// CLI's handlers on its cell; tests call `request` directly. `0` in the cell means none.
#[derive(Debug, Clone, Default)]
pub struct Cancel {
    signal: Arc<AtomicUsize>,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `signal` (a later one replaces it, as a handler's store does).
    pub fn request(&self, signal: i32) {
        debug_assert!(signal > 0, "signal numbers are positive");
        self.signal.store(signal as usize, Ordering::SeqCst);
    }

    pub fn requested(&self) -> Option<i32> {
        match self.signal.load(Ordering::SeqCst) {
            0 => None,
            n => Some(n as i32),
        }
    }

    /// `Err(Interrupted(n))` once a signal is recorded.
    pub fn check(&self) -> Result<(), Interrupted> {
        match self.requested() {
            Some(n) => Err(Interrupted(n)),
            None => Ok(()),
        }
    }

    /// The cell a signal handler stores the signal number into.
    pub fn cell(&self) -> Arc<AtomicUsize> {
        self.signal.clone()
    }
}

/// A cancellation point found the token set (§14.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("interrupted by signal {0}")]
pub struct Interrupted(pub i32);

```

In `crates/tagteam-provider/src/lib.rs`, replace

```rust
pub use clock::{Clock, FakeClock, SystemClock};
```

with

```rust
pub use cancel::{Cancel, Interrupted};
pub use clock::{Clock, FakeClock, SystemClock};
```

In `crates/tagteam-provider/src/env.rs`, replace

```rust
use std::ffi::OsString;
use std::path::PathBuf;
```

with

```rust
use std::ffi::OsString;
use std::path::PathBuf;

use crate::cancel::Cancel;
```

then replace

```rust
    pub claude_securestorage_config_dir: Option<OsString>,
    forbidden_root: Option<PathBuf>,
}
```

with

```rust
    pub claude_securestorage_config_dir: Option<OsString>,
    /// The process's cancel token (§14.1). Clones share it, so every engine and lock built
    /// from one Env sees the same signal; each `for_test` Env has a token of its own.
    pub cancel: Cancel,
    forbidden_root: Option<PathBuf>,
}
```

In `Env::from_process`, replace

```rust
            claude_securestorage_config_dir: std::env::var_os("CLAUDE_SECURESTORAGE_CONFIG_DIR"),
            forbidden_root: None,
```

with

```rust
            claude_securestorage_config_dir: std::env::var_os("CLAUDE_SECURESTORAGE_CONFIG_DIR"),
            cancel: Cancel::new(),
            forbidden_root: None,
```

and in `Env::for_test`, replace

```rust
            claude_securestorage_config_dir: None,
            forbidden_root: Some(real_home),
```

with

```rust
            claude_securestorage_config_dir: None,
            cancel: Cancel::new(),
            forbidden_root: Some(real_home),
```

In `crates/tagteam-provider/src/mkdir_lock.rs`, replace everything from the line after
`use std::time::{Duration, Instant, SystemTime};` through the closing brace of
`impl MkdirLockSpec` (the current `LockError`, `MkdirLockSpec` and `MkdirLockSpec::new`) with:

```rust

use crate::cancel::Cancel;

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("timed out waiting for the lock {0}")]
    Timeout(PathBuf),
    #[error("the lock {0} was taken over while held")]
    Compromised(PathBuf),
    #[error("lock I/O failed: {0}")]
    Io(#[from] io::Error),
    /// The cancel token was set while waiting (§14.1). The wait took nothing.
    #[error("interrupted while waiting for the lock {path}")]
    Interrupted { path: PathBuf, signal: i32 },
}

impl LockError {
    /// The signal behind an interrupted wait; `None` for every other failure.
    pub fn signal(&self) -> Option<i32> {
        match self {
            LockError::Interrupted { signal, .. } => Some(*signal),
            _ => None,
        }
    }
}

/// Decision 3 (§14.1): every lock wait calls this before each attempt, the first included, so
/// a command whose token is set never takes a new lock.
pub(crate) fn check_cancel(cancel: &Cancel, path: &Path) -> Result<(), LockError> {
    match cancel.requested() {
        Some(signal) => Err(LockError::Interrupted {
            path: path.to_path_buf(),
            signal,
        }),
        None => Ok(()),
    }
}

#[derive(Debug, Clone)]
pub struct MkdirLockSpec {
    pub path: PathBuf,
    pub stale: Duration,
    pub acquire_timeout: Duration,
    pub touch_every: Duration,
    /// Checked before every attempt `MkdirLock::acquire` makes (§14.1). `new` gives a token
    /// nothing sets; `with_cancel` shares the caller's.
    pub cancel: Cancel,
}

impl MkdirLockSpec {
    pub fn new(path: PathBuf, stale: Duration, acquire_timeout: Duration) -> Self {
        Self {
            path,
            stale,
            acquire_timeout,
            touch_every: Duration::from_secs(3),
            cancel: Cancel::new(),
        }
    }

    /// The same lock, waited for under `cancel`.
    pub fn with_cancel(self, cancel: &Cancel) -> Self {
        Self {
            cancel: cancel.clone(),
            ..self
        }
    }
}
```

In the same file, replace the head of `MkdirLock::acquire`:

```rust
    pub fn acquire(spec: &MkdirLockSpec) -> Result<Self, LockError> {
        let deadline = Instant::now() + spec.acquire_timeout;
        loop {
            if let Some(lock) = Self::try_acquire(spec)? {
```

with:

```rust
    /// Waits up to `acquire_timeout`, polling every 250–500 ms. `spec.cancel` is checked before
    /// every attempt (§14.1); `try_acquire`, one attempt and not a wait, never checks it.
    pub fn acquire(spec: &MkdirLockSpec) -> Result<Self, LockError> {
        let deadline = Instant::now() + spec.acquire_timeout;
        loop {
            check_cancel(&spec.cancel, &spec.path)?;
            if let Some(lock) = Self::try_acquire(spec)? {
```

In `crates/tagteam-provider/src/flock.rs`, replace

```rust
use crate::env::Env;
use crate::mkdir_lock::LockError;
```

with

```rust
use crate::cancel::Cancel;
use crate::env::Env;
use crate::mkdir_lock::{LockError, check_cancel};
```

then replace the head of `FlockGuard::lock`:

```rust
    pub fn lock(path: &Path, timeout: Duration) -> Result<Self, LockError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(g) = Self::try_lock(path)? {
```

with:

```rust
    /// Waits up to `timeout`, polling every 100 ms. `cancel` is checked before every attempt,
    /// the first included (§14.1): a set token ends the wait with `LockError::Interrupted` and
    /// takes nothing.
    pub fn lock(path: &Path, timeout: Duration, cancel: &Cancel) -> Result<Self, LockError> {
        let deadline = Instant::now() + timeout;
        loop {
            check_cancel(cancel, path)?;
            if let Some(g) = Self::try_lock(path)? {
```

and replace `MutationGuard::acquire`:

```rust
    pub fn acquire(env: &Env, timeout: Duration) -> Result<Self, LockError> {
        let path = env.data_dir().join(".mutation.lock");
        Ok(Self {
            _lock: FlockGuard::lock(&path, timeout)?,
        })
    }
```

with:

```rust
    /// Waits up to `timeout`, on `env.cancel` (§14.1).
    pub fn acquire(env: &Env, timeout: Duration) -> Result<Self, LockError> {
        let path = env.data_dir().join(".mutation.lock");
        Ok(Self {
            _lock: FlockGuard::lock(&path, timeout, &env.cancel)?,
        })
    }
```

In `crates/tagteam-engine/src/account_lock.rs`, replace

```rust
    pub fn acquire(env: &Env, id: &AccountId, wait: Duration) -> Result<Self, LockError> {
        Ok(Self {
            _guard: FlockGuard::lock(&Self::path(env, id), wait)?,
```

with

```rust
    /// Waits up to `wait`, checking `env.cancel` before every attempt (§14.1).
    pub fn acquire(env: &Env, id: &AccountId, wait: Duration) -> Result<Self, LockError> {
        Ok(Self {
            _guard: FlockGuard::lock(&Self::path(env, id), wait, &env.cancel)?,
```

`MkdirLockSpec::new` callers in `tagteam-cc` and `tagteam-fake` compile unchanged; their waits
stay on a token nothing sets until Task 2.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-provider --lib`
Expected: PASS, including the 13 new tests (5 in `cancel`, 1 in `env`, 4 in `mkdir_lock`,
3 in `flock`) and every pre-existing one. An unset token changes nothing: the pre-existing
timeout and takeover tests (`a_held_lock_times_out`,
`a_second_lock_on_the_same_file_is_refused_until_release`, …) run on `Cancel::new()`.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. `AccountLock::acquire` now waits on `env.cancel`, which no existing test sets.

- [ ] **Step 5: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider/src/cancel.rs crates/tagteam-provider/src/lib.rs \
  crates/tagteam-provider/src/env.rs crates/tagteam-provider/src/mkdir_lock.rs \
  crates/tagteam-provider/src/flock.rs crates/tagteam-engine/src/account_lock.rs
git commit -m "Add the cancel token and check it before every lock attempt"
```

---

### Task 2: Every lock wait honours the token; the `interrupted` error

§14.1: "The work checks the token, and unwinds with an `interrupted` error, only where stopping
loses nothing: each iteration of a lock wait: the mutation lock, account locks, and the
provider's live locks … Unwinding runs every `Drop`. Locks are released, so CC's lock directories
are removed … and a switch that has not journaled has written nothing." It also says "recovery's
writes (§9.6)" are a critical span with no cancellation point inside it. Review Focus 1 is the
case this task pins at the engine level: Ctrl-C while `tagteam switch` waits for CC's credential
locks. Task 6 pins the same case with a real SIGINT to the binary.

Task 1 made the mutation and account locks wait on `env.cancel`. This task passes the token to
the provider live-lock waits (Claude Code's three locks and FakeAgent's one) and gives the error
one kind, `interrupted`, whichever of the three carriers holds it (Decision 4). It also
exposes `Engine::cancel()` for the CLI (Task 6).

**Readings of the spec this task commits to:**
- **CC's credential wait checks the token before every attempt, retries included.** The refresh
  lock's attempts are checked by `MkdirLock::acquire`. When the legacy lock is contended, the
  refresh lock is released and the loop sleeps 250–500 ms. The token is checked right after
  that sleep, so the error names the lock that was being waited for (the legacy lock). The
  first attempt at the legacy lock comes microseconds after the refresh lock's check and is not
  checked again: a token set in that gap is seen by the next wait, the config lock's.
- **Unwinding releases what was taken.** A config wait that is interrupted drops the credential
  locks it was given, as a timed-out one already does, so no CC lock directory of tagteam's
  survives an interruption.
- **A recovery's lock waits are cancellation points; its writes are not.** A recovery interrupted
  at a wait has written nothing. Its row stays for the next command, and the command ends with
  the interruption. It is never reported as `interrupted-switch`, which would point the user at
  `--force`, and never as `RecoveryBlocked`, which is for a timeout. A token set after the
  waits, while recovery writes, lets recovery finish and commit; the command stops at its next
  wait.
- **`kind()` is `interrupted` for all three carriers**: `EngineError::Interrupted(n)`,
  `EngineError::Lock(LockError::Interrupted { .. })` and
  `EngineError::Provider(ProviderError::Lock(LockError::Interrupted { .. }))`. `signal()` reads
  the number from any of them and is `None` for every other error.
  `EngineError::Interrupted(i32)` is defined here; Task 5 is the first to produce it.
- **After a request, §7.5's live write is inside the critical span, its lock wait included**
  (the integrator's ruling, which extends §14.1's "from sending the token request to persisting
  the successor"). The request consumed the generation CC holds, and step 5 writes the
  successor to the live store "in either case, so CC always holds the newest generation". So
  `publish`'s config-lock wait after a request runs under a token nothing sets. A signal that
  arrives during it stays recorded for the next cancellation point. A timeout there still
  leaves the successor unpublished, for step 3's self-heal. The self-heal's own `publish`,
  before any request, stays cancellable: nothing was sent in that pass, so stopping loses
  nothing new.

**Files:**
- Modify: `crates/tagteam-cc/src/locks.rs` (`acquire_credentials`, `acquire_config`, and its `mod tests`)
- Modify: `crates/tagteam-cc/src/provider.rs` (`lock_credentials`, `lock_config`)
- Modify: `crates/tagteam-cc/tests/provider.rs`
- Modify: `crates/tagteam-fake/src/provider.rs` (`lock_credentials`)
- Modify: `crates/tagteam-fake/tests/provider.rs`
- Modify: `crates/tagteam-engine/src/error.rs` (`Interrupted`, `kind`, `signal`, and its `mod tests`)
- Modify: `crates/tagteam-engine/src/engine.rs` (`Engine::cancel`, `guard_recovering`)
- Modify: `crates/tagteam-engine/tests/common/mod.rs` (`mutation_lock_free`)
- Create: `crates/tagteam-engine/tests/cancel.rs`
- Modify: `crates/tagteam-engine/src/active.rs` (`publish` and its two call sites; Steps 12–16)
- Modify: `crates/tagteam-engine/tests/active.rs` (its `mod hooks`; Steps 12–16)

**Interfaces:**
- Consumes:
  - Task 1: `Cancel` (`new`, `request`, `requested`), `Env.cancel`,
    `LockError::Interrupted { path, signal }`, `LockError::signal`,
    `MkdirLockSpec::with_cancel`, and `MutationGuard::acquire` / `AccountLock::acquire`
    waiting on `env.cancel`
  - Existing: `Fx` (`new`, `add`, `rotate_live`, `paths`, `switch_to`, `live_email`, `kc`,
    `engine`, `env`, `provider`), `common::{crashed_switch, write_target_credential, journal,
    mutation_lock_free}`, `Engine::on_point` (`test-hooks`), the `recovery-before-commit`
    hook point, `FakeKeychain::items`, `Store::{active, events}`, `EventRow`
- Produces:
  - `tagteam_cc::locks::acquire_credentials(paths: &CcPaths, timeout: Duration, cancel: &Cancel) -> Result<CcCredSet, LockError>`
  - `tagteam_cc::locks::acquire_config(paths: &CcPaths, timeout: Duration, cancel: &Cancel) -> Result<CcConfigSet, LockError>`
  - `ClaudeCode::lock_credentials` / `lock_config` and `FakeAgent::lock_credentials` wait on
    `&env.cancel` (trait signatures unchanged)
  - `EngineError::Interrupted(i32)` (`#[error("interrupted")]`),
    `EngineError::signal(&self) -> Option<i32>`, and `kind() == "interrupted"` for all three
    carriers
  - `Engine::cancel(&self) -> &Cancel`, which returns `&self.env.cancel`
  - `guard_recovering` (and so `mutation_guard`, `guard_or_refuse` and `settle_or_refuse`)
    returns an interrupted recovery's error unchanged
  - `common::mutation_lock_free(env)` probes under a fresh token
  - private to `active.rs`: `Engine::publish` gains a last parameter `cancel: &Cancel`, the
    token its config-lock wait honours. The self-heal passes `&self.env.cancel`; the call after
    the request passes a fresh `Cancel::new()`.

- [ ] **Step 1: Write the failing provider tests**

In `crates/tagteam-cc/src/locks.rs`, inside `mod tests`, give every existing call a token
nothing sets. Replace each of these lines:

```rust
        let set = acquire_credentials(&p, ACQUIRE_TIMEOUT).unwrap();
```
```rust
        let set = acquire_config(&p, ACQUIRE_TIMEOUT).unwrap();
```
```rust
            acquire_credentials(&p, Duration::from_millis(700)),
```
```rust
        let handle = thread::spawn(move || acquire_credentials(&p2, Duration::from_secs(30)));
```
```rust
            acquire_credentials(&p, Duration::from_millis(500)),
```
```rust
            acquire_config(&p, Duration::from_millis(500)),
```

with, respectively:

```rust
        let set = acquire_credentials(&p, ACQUIRE_TIMEOUT, &Cancel::new()).unwrap();
```
```rust
        let set = acquire_config(&p, ACQUIRE_TIMEOUT, &Cancel::new()).unwrap();
```
```rust
            acquire_credentials(&p, Duration::from_millis(700), &Cancel::new()),
```
```rust
        let handle = thread::spawn(move || {
            acquire_credentials(&p2, Duration::from_secs(30), &Cancel::new())
        });
```
```rust
            acquire_credentials(&p, Duration::from_millis(500), &Cancel::new()),
```
```rust
            acquire_config(&p, Duration::from_millis(500), &Cancel::new()),
```

Then add at the end of the module, after `a_held_config_lock_times_out_without_touching_it`:

```rust
    /// Sets `cancel` to SIGINT from another thread 200 ms from now, as the CLI's handler would
    /// (§14.1), and returns the instant just before it did.
    fn interrupt_soon(cancel: &Cancel) -> thread::JoinHandle<Instant> {
        let cancel = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            let at = Instant::now();
            cancel.request(libc::SIGINT);
            at
        })
    }

    /// Runs `wait` while `interrupt_soon` sets the token; it must still be waiting then, and
    /// must end within one poll (at most 500 ms) of it. Returns the lock it names.
    fn interrupted<T>(wait: impl FnOnce(&Cancel) -> Result<T, LockError>) -> std::path::PathBuf {
        let cancel = Cancel::new();
        let setter = interrupt_soon(&cancel);
        let result = wait(&cancel);
        let ended = Instant::now();
        let set_at = setter.join().unwrap();
        assert!(ended >= set_at, "the wait ended before the token was set");
        assert!(
            ended - set_at < Duration::from_secs(1),
            "{:?}",
            ended - set_at
        );
        match result {
            Err(LockError::Interrupted { path, signal }) => {
                assert_eq!(signal, libc::SIGINT);
                path
            }
            Err(e) => panic!("expected an interrupted wait, got {e:?}"),
            Ok(_) => panic!("expected an interrupted wait, got the locks"),
        }
    }

    #[test]
    fn a_set_token_takes_neither_credential_lock() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        let cancel = Cancel::new();
        cancel.request(libc::SIGTERM);
        match acquire_credentials(&p, ACQUIRE_TIMEOUT, &cancel) {
            Err(LockError::Interrupted { path, signal }) => {
                assert_eq!((path, signal), (p.refresh_lock.clone(), libc::SIGTERM))
            }
            other => panic!("expected an interrupted wait, got {:?}", other.err()),
        }
        assert!(!p.refresh_lock.exists() && !p.legacy_lock().exists());
    }

    #[test]
    fn a_token_set_while_cc_holds_the_refresh_lock_ends_the_wait() {
        // Review Focus 1: CC is mid-refresh.
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(&p.refresh_lock).unwrap();
        let named = interrupted(|c| acquire_credentials(&p, Duration::from_secs(30), c));
        assert_eq!(named, p.refresh_lock);
        assert!(p.refresh_lock.is_dir(), "CC's lock is left alone");
        assert!(!p.legacy_lock().exists());
    }

    #[test]
    fn a_token_set_during_legacy_contention_ends_the_retries_naming_the_legacy_lock() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(p.legacy_lock()).unwrap(); // CC holds it, freshly
        let named = interrupted(|c| acquire_credentials(&p, Duration::from_secs(30), c));
        assert_eq!(named, p.legacy_lock());
        assert!(
            !p.refresh_lock.exists(),
            "the refresh lock each retry took is released"
        );
        assert!(p.legacy_lock().is_dir(), "CC's lock is left alone");
    }

    #[test]
    fn a_token_set_while_the_config_lock_is_held_ends_the_wait() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(&p.config_lock).unwrap(); // something else holds it, freshly
        let named = interrupted(|c| acquire_config(&p, Duration::from_secs(30), c));
        assert_eq!(named, p.config_lock);
        assert!(
            p.config_lock.is_dir(),
            "the other holder's lock is left alone"
        );
    }
```

In `crates/tagteam-cc/tests/provider.rs`, add before the doc comment that begins
`/// §9.1: one budget covers both stages.` (above `lock_live_spends_one_budget_across_both_stages`):

```rust
/// §14.1: a lock wait is a cancellation point, and unwinding releases what is held: the
/// credential locks `lock_config` was given are removed with the interrupted config wait.
#[test]
fn an_interrupted_config_wait_releases_the_credential_locks_it_was_given() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::create_dir(&paths.config_lock).unwrap(); // someone else holds it, freshly
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let cred =
        f.cc.lock_credentials(&f.env, &g, Duration::from_secs(1))
            .unwrap();
    f.env.cancel.request(libc::SIGTERM);
    let start = Instant::now();
    match f.cc.lock_config(&f.env, cred, Duration::from_secs(2)) {
        Err(ProviderError::Lock(LockError::Interrupted { path, signal })) => {
            assert_eq!((path, signal), (paths.config_lock.clone(), libc::SIGTERM))
        }
        other => panic!("expected an interrupted wait, got {:?}", other.err()),
    }
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "checked before the first attempt"
    );
    assert!(!paths.refresh_lock.exists() && !paths.legacy_lock().exists());
    assert!(
        paths.config_lock.is_dir(),
        "the other holder's lock is left alone"
    );
}

```

In `crates/tagteam-fake/tests/provider.rs`, add after
`its_one_live_lock_times_out_within_its_budget_and_is_left_alone`:

```rust
/// Any signal number: the token only carries it.
const SIGTERM: i32 = 15;

#[test]
fn its_live_lock_wait_ends_when_the_token_is_set() {
    // §14.1: FakeAgent's lock wait is a cancellation point like Claude Code's.
    let f = fx();
    login(&f.env, "alice", "ws", "tok-a", "renew-a");
    let p = FakePaths::resolve(&f.env);
    fs::create_dir(&p.lock).unwrap(); // another FakeAgent process holds it, freshly
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    f.env.cancel.request(SIGTERM);
    let start = Instant::now();
    assert!(matches!(
        f.fake.lock_live(&f.env, &g),
        Err(ProviderError::Lock(LockError::Interrupted {
            signal: SIGTERM,
            ..
        }))
    ));
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "not the 1 s budget"
    );
    assert!(p.lock.is_dir(), "the other holder's lock is left alone");
}

```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-cc --lib locks::tests`
Expected: FAIL to compile: `this function takes 2 arguments but 3 arguments were supplied`
(ten times) and `cannot find type Cancel` / `use of undeclared type Cancel`.

Run: `cargo test -p tagteam-cc --test provider an_interrupted_config_wait`
Expected: FAIL after about 2 s: `expected an interrupted wait, got
Some(Lock(Timeout(".../home/.claude.json.lock")))`. CC's config wait ignores the token and runs
out its budget.

Run: `cargo test -p tagteam-fake --test provider its_live_lock_wait_ends`
Expected: FAIL after about 1 s, at the `matches!` assertion. FakeAgent's wait ignores the token
and ends with `Timeout`.

- [ ] **Step 3: Pass the token to the provider live-lock waits**

In `crates/tagteam-cc/src/locks.rs`, replace

```rust
use tagteam_provider::{LiveLockSet, LockError, MkdirLock, MkdirLockSpec};
```

with

```rust
use tagteam_provider::{Cancel, LiveLockSet, LockError, MkdirLock, MkdirLockSpec};
```

and replace `acquire_credentials` and `acquire_config`, from the doc comment
`/// The refresh lock, then the legacy lock.` through the closing brace of `acquire_config`,
with:

```rust
/// The refresh lock, then the legacy lock. If the legacy lock is contended the refresh lock is
/// released and the pair retried, as CC does. tagteam never writes `.oauth_refresh.lock.owner`.
/// `cancel` is checked before every attempt (§14.1): a token set while the legacy lock is
/// contended ends the retries naming that lock, with the refresh lock already released.
pub fn acquire_credentials(
    paths: &CcPaths,
    timeout: Duration,
    cancel: &Cancel,
) -> Result<CcCredSet, LockError> {
    let deadline = Instant::now() + timeout;
    let remaining = || deadline.saturating_duration_since(Instant::now());
    loop {
        let refresh = MkdirLock::acquire(
            &MkdirLockSpec::new(paths.refresh_lock.clone(), CRED_STALE, remaining())
                .with_cancel(cancel),
        )?;
        let legacy_spec = MkdirLockSpec::new(paths.legacy_lock(), CRED_STALE, Duration::ZERO);
        match MkdirLock::try_acquire(&legacy_spec)? {
            Some(legacy) => return Ok(CcCredSet { legacy, refresh }),
            None => {
                drop(refresh);
                if Instant::now() >= deadline {
                    return Err(LockError::Timeout(legacy_spec.path));
                }
                thread::sleep(Duration::from_millis(fastrand::u64(250..=500)).min(remaining()));
                if let Some(signal) = cancel.requested() {
                    return Err(LockError::Interrupted {
                        path: legacy_spec.path,
                        signal,
                    });
                }
            }
        }
    }
}

/// The config lock alone, waited for under `cancel` (§14.1). A caller takes it only while
/// holding the credential locks (`CredLocks::with_config`, §4.3).
pub fn acquire_config(
    paths: &CcPaths,
    timeout: Duration,
    cancel: &Cancel,
) -> Result<CcConfigSet, LockError> {
    Ok(CcConfigSet {
        config: MkdirLock::acquire(
            &MkdirLockSpec::new(paths.config_lock.clone(), CONFIG_STALE, timeout)
                .with_cancel(cancel),
        )?,
    })
}
```

In `crates/tagteam-cc/src/provider.rs`, in `lock_credentials`, replace

```rust
        let set = locks::acquire_credentials(&CcPaths::resolve(env), budget)?;
```

with

```rust
        let set = locks::acquire_credentials(&CcPaths::resolve(env), budget, &env.cancel)?;
```

and in `lock_config`, replace

```rust
        // On a timeout `cred` is dropped as this returns, releasing the credential locks.
        let set = locks::acquire_config(&CcPaths::resolve(env), budget)?;
```

with

```rust
        // On a timeout or an interruption, `cred` is dropped as this returns, releasing the
        // credential locks.
        let set = locks::acquire_config(&CcPaths::resolve(env), budget, &env.cancel)?;
```

In `crates/tagteam-fake/src/provider.rs`, in `lock_credentials`, replace

```rust
        let spec = MkdirLockSpec::new(FakePaths::resolve(env).lock, LOCK_STALE, budget);
```

with

```rust
        let spec = MkdirLockSpec::new(FakePaths::resolve(env).lock, LOCK_STALE, budget)
            .with_cancel(&env.cancel);
```

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p tagteam-cc --lib locks::tests`
Expected: PASS, the four new tests and the six existing ones.
`a_token_set_during_legacy_contention_ends_the_retries_naming_the_legacy_lock` depends on the
explicit check after the sleep. Without that check, the next refresh attempt's own check fires
first and the error names `.oauth_refresh.lock`.

Run: `cargo test -p tagteam-cc --test provider` and `cargo test -p tagteam-fake --test provider`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tagteam-cc/src/locks.rs crates/tagteam-cc/src/provider.rs \
  crates/tagteam-cc/tests/provider.rs crates/tagteam-fake/src/provider.rs \
  crates/tagteam-fake/tests/provider.rs
git commit -m "Wait for Claude Code's and FakeAgent's locks on the Env's cancel token"
```

- [ ] **Step 6: Write the failing engine tests**

In `crates/tagteam-engine/src/error.rs`, inside `mod tests`, replace the end of
`kind_is_pinned_for_every_variant`:

```rust
            (EngineError::Io(io::Error::other("x")), "io"),
        ];
        for (err, want) in cases {
            assert_eq!(err.kind(), want, "{err:?}");
        }
    }
```

with:

```rust
            (EngineError::Io(io::Error::other("x")), "io"),
            (
                EngineError::Provider(ProviderError::Lock(LockError::Interrupted {
                    path: PathBuf::from("x"),
                    signal: 2,
                })),
                "interrupted",
            ),
            (
                EngineError::Lock(LockError::Interrupted {
                    path: PathBuf::from("x"),
                    signal: 15,
                }),
                "interrupted",
            ),
            (EngineError::Interrupted(1), "interrupted"),
        ];
        for (err, want) in cases {
            assert_eq!(err.kind(), want, "{err:?}");
        }
    }

    #[test]
    fn signal_reads_every_carrier_of_an_interruption() {
        let lock = |signal| LockError::Interrupted {
            path: PathBuf::from("x"),
            signal,
        };
        assert_eq!(EngineError::Interrupted(2).signal(), Some(2));
        assert_eq!(EngineError::Lock(lock(15)).signal(), Some(15));
        assert_eq!(
            EngineError::Provider(ProviderError::Lock(lock(1))).signal(),
            Some(1)
        );
        assert_eq!(
            EngineError::Lock(LockError::Timeout(PathBuf::from("x"))).signal(),
            None
        );
        assert_eq!(
            EngineError::Provider(ProviderError::Lock(LockError::Compromised(PathBuf::from(
                "x"
            ))))
            .signal(),
            None
        );
        assert_eq!(EngineError::LiveMoved.signal(), None);
    }
```

In `crates/tagteam-engine/tests/common/mod.rs`, replace

```rust
use tagteam_provider::{
    Clock, Credential, Env, FakeClock, FakeKeychain, Identity, IdentitySurface, MutationGuard,
    ProcessStamp, Provider, Read, ScriptedHttp,
};
```

with

```rust
use tagteam_provider::{
    Cancel, Clock, Credential, Env, FakeClock, FakeKeychain, Identity, IdentitySurface,
    MutationGuard, ProcessStamp, Provider, Read, ScriptedHttp,
};
```

and replace

```rust
/// Whether tagteam's mutation lock is free right now; takes and drops it if so.
pub fn mutation_lock_free(env: &Env) -> bool {
    MutationGuard::acquire(env, Duration::ZERO).is_ok()
}
```

with

```rust
/// Whether tagteam's mutation lock is free right now; takes and drops it if so. It asks under a
/// token of its own: a test that set `env`'s token would otherwise read every lock as taken.
pub fn mutation_lock_free(env: &Env) -> bool {
    let mut probe = env.clone();
    probe.cancel = Cancel::new();
    MutationGuard::acquire(&probe, Duration::ZERO).is_ok()
}
```

Create `crates/tagteam-engine/tests/cancel.rs`:

```rust
//! §14.1: every lock wait is a cancellation point. A token set while a switch waits ends it
//! within one poll, with nothing written and no lock directory of tagteam's left behind
//! (Review Focus 1). Work done under the locks is never cut short: only waits check the token.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use common::{Fx, crashed_switch, journal, mutation_lock_free, write_target_credential};
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::store::EventRow;
use tagteam_provider::{Cancel, MutationGuard};

const SIGINT: i32 = libc::SIGINT;

/// Everything a switch could write: every Keychain item (the live credential, the managed key
/// and the vault), the global config, and the store's journal, active account and events.
#[derive(Debug, PartialEq)]
struct Written {
    items: BTreeMap<(String, String), Vec<u8>>,
    config: Vec<u8>,
    journal: bool,
    active: Option<AccountId>,
    events: Vec<EventRow>,
}

fn written(fx: &Fx) -> Written {
    let store = fx.engine.store().unwrap();
    Written {
        items: fx.kc.items(),
        config: fs::read(fx.paths().global_config).unwrap(),
        journal: journal(fx).is_some(),
        active: store.active(&fx.provider()).unwrap(),
        events: store.events().unwrap(),
    }
}

/// The three CC lock directories (§9.1).
fn cc_locks(fx: &Fx) -> [PathBuf; 3] {
    let p = fx.paths();
    [
        p.refresh_lock.clone(),
        p.legacy_lock(),
        p.config_lock.clone(),
    ]
}

/// Sets `cancel` to SIGINT from another thread once `after` has passed, as the CLI's signal
/// handler would (§14.1), and returns the instant just before it did.
fn interrupt_after(cancel: &Cancel, after: Duration) -> thread::JoinHandle<Instant> {
    let cancel = cancel.clone();
    thread::spawn(move || {
        thread::sleep(after);
        let at = Instant::now();
        cancel.request(SIGINT);
        at
    })
}

/// Runs `work` while a Ctrl-C arrives 200 ms in. `work` must still be waiting then and must
/// end with an error; returns the error and how long `work` ran on after the token was set.
fn interrupted<T: std::fmt::Debug>(
    fx: &Fx,
    work: impl FnOnce() -> Result<T, EngineError>,
) -> (EngineError, Duration) {
    let setter = interrupt_after(fx.engine.cancel(), Duration::from_millis(200));
    let result = work();
    let ended = Instant::now();
    let set_at = setter.join().unwrap();
    assert!(
        ended >= set_at,
        "it ended before the token was set, so it never waited: {result:?}"
    );
    (result.unwrap_err(), ended - set_at)
}

/// The interruption as the CLI reports it (Task 6): the signal and the `interrupted` kind,
/// within one poll of the token.
fn assert_interrupted(err: &EngineError, ran_on: Duration) {
    assert_eq!(err.signal(), Some(SIGINT), "{err}");
    assert_eq!(err.kind(), "interrupted", "{err}");
    // One poll: 100 ms for a flock, at most 500 ms for a CC lock. Every wait here would
    // otherwise run 9 s (CC), 10 s (the mutation lock) or 15 s (an account lock).
    assert!(
        ran_on < Duration::from_secs(1),
        "{ran_on:?} after the token"
    );
}

/// Review Focus 1: Claude Code holds `held` (it is refreshing), and the user presses Ctrl-C
/// while `switch` waits for it. CC's real 9 s budget is in force, so only the token can end the
/// wait this soon.
fn interrupted_while_cc_holds(held: fn(&Fx) -> PathBuf) {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.rotate_live("rt-b2"); // a capture made before the wait would show in the vault
    let held = held(&fx);
    fs::create_dir(&held).unwrap();
    let before = written(&fx);

    let (err, ran_on) = interrupted(&fx, || fx.switch_to(&a, false));

    assert_interrupted(&err, ran_on);
    assert!(
        err.to_string().contains(&held.display().to_string()),
        "names the lock it waited for: {err}"
    );
    assert_eq!(written(&fx), before, "nothing is written");
    for lock in cc_locks(&fx) {
        if lock == held {
            assert!(lock.is_dir(), "CC's lock is left alone");
        } else {
            assert!(!lock.exists(), "{} was left behind", lock.display());
        }
    }
    assert!(mutation_lock_free(&fx.env), "the mutation lock is released");
}

#[test]
fn ctrl_c_while_cc_holds_its_refresh_lock_ends_the_switch() {
    interrupted_while_cc_holds(|fx| fx.paths().refresh_lock);
}

#[test]
fn ctrl_c_while_cc_holds_its_legacy_lock_ends_the_switch() {
    interrupted_while_cc_holds(|fx| fx.paths().legacy_lock());
}

#[test]
fn ctrl_c_while_the_config_lock_is_held_ends_the_switch_and_releases_the_credential_locks() {
    interrupted_while_cc_holds(|fx| fx.paths().config_lock);
}

#[test]
fn ctrl_c_while_another_process_holds_an_account_lock_ends_the_switch() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let before = written(&fx);
    // Another tagteam process's gate or vault write holds a's lock.
    let held = AccountLock::acquire(&fx.env, &a, Duration::from_secs(1)).unwrap();

    let (err, ran_on) = interrupted(&fx, || fx.switch_to(&a, false));
    drop(held);

    assert_interrupted(&err, ran_on);
    assert!(matches!(err, EngineError::Lock(_)), "{err:?}");
    assert_eq!(written(&fx), before, "nothing is written");
    assert!(
        cc_locks(&fx).iter().all(|l| !l.exists()),
        "CC's locks come after the account locks: none was taken"
    );
    assert!(mutation_lock_free(&fx.env), "the mutation lock is released");
}

#[test]
fn ctrl_c_while_another_command_holds_the_mutation_lock_ends_the_switch() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let before = written(&fx);
    let held = MutationGuard::acquire(&fx.env, Duration::ZERO).unwrap(); // another command

    let (err, ran_on) = interrupted(&fx, || fx.switch_to(&a, false));
    drop(held);

    assert_interrupted(&err, ran_on);
    assert!(matches!(err, EngineError::Lock(_)), "{err:?}");
    assert_eq!(written(&fx), before, "nothing is written");
    assert!(cc_locks(&fx).iter().all(|l| !l.exists()));
    assert!(mutation_lock_free(&fx.env), "the mutation lock is released");
}

/// A recovery's lock waits are cancellation points too (§14.1). Interrupted there, it has
/// written nothing, its row stays for the next command, and the command reports the
/// interruption: never `interrupted-switch`, which would send the user to `--force`.
#[test]
fn ctrl_c_while_recovery_waits_for_cc_reports_the_interruption() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a); // died after writing the credential, before the identity
    fs::create_dir(fx.paths().refresh_lock).unwrap(); // CC is refreshing
    let before = written(&fx);

    let (err, ran_on) = interrupted(&fx, || fx.switch_to(&b, false));

    assert_interrupted(&err, ran_on);
    assert!(journal(&fx).is_some(), "the row waits for the next command");
    assert_eq!(written(&fx), before, "recovery wrote nothing");
    let [refresh, legacy, config] = cc_locks(&fx);
    assert!(refresh.is_dir(), "CC's lock is left alone");
    assert!(!legacy.exists() && !config.exists());
    assert!(mutation_lock_free(&fx.env), "the mutation lock is released");
}

/// §14.1: recovery's writes are a critical span. A token set while they run lets them finish
/// and commit; the command stops at its next wait, its own mutation lock, before it changes
/// anything else.
#[cfg(feature = "test-hooks")]
#[test]
fn a_token_set_during_recovery_s_writes_lets_recovery_finish() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    let cancel = fx.engine.cancel().clone();
    fx.engine.on_point(
        "recovery-before-commit",
        Box::new(move || cancel.request(SIGINT)),
    );

    let err = fx.switch_to(&b, false).unwrap_err();

    assert_eq!(err.signal(), Some(SIGINT), "{err}");
    assert!(
        matches!(err, EngineError::Lock(_)),
        "stopped at the switch's own mutation lock: {err:?}"
    );
    assert!(journal(&fx).is_none(), "recovery committed");
    let store = fx.engine.store().unwrap();
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(a));
    assert_eq!(
        store.events().unwrap().last().unwrap().kind,
        "switch-recovered"
    );
    assert_eq!(
        fx.live_email().as_deref(),
        Some("a@x.co"),
        "b was never activated"
    );
    assert!(cc_locks(&fx).iter().all(|l| !l.exists()));
    assert!(mutation_lock_free(&fx.env), "the mutation lock is released");
}
```

How the last two tests reach their point: `switch_to(&b, false)` runs `settle_or_refuse`
first. It finds the dead journal row and recovers it under the mutation lock
(`guard_recovering`) before any planning. In the last test, the switch's own
`guard_or_refuse` is the next wait after recovery commits.

- [ ] **Step 7: Run them to verify they fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test cancel`
Expected: FAIL to compile: `no method named cancel found for struct Engine` and
`no method named signal found for enum EngineError` / `for reference &EngineError`.

Run: `cargo test -p tagteam-engine --lib error::tests`
Expected: FAIL to compile: `no variant or associated item named Interrupted found for enum
EngineError` and `no method named signal`.

- [ ] **Step 8: Add the `interrupted` error and `Engine::cancel`, and stop on an interrupted recovery**

In `crates/tagteam-engine/src/error.rs`, replace the last variant of `EngineError`:

```rust
    #[error(transparent)]
    Io(#[from] io::Error),
}
```

with:

```rust
    #[error(transparent)]
    Io(#[from] io::Error),
    /// §14.1: a cancellation point outside a lock wait found the cancel token set. A lock wait
    /// reports its own `LockError::Interrupted`; `signal()` reads either.
    #[error("interrupted")]
    Interrupted(i32),
}
```

In `kind()`, replace

```rust
            EngineError::Provider(ProviderError::Lock(LockError::Timeout(_))) => "lock-timeout",
```

with

```rust
            EngineError::Provider(ProviderError::Lock(LockError::Timeout(_))) => "lock-timeout",
            EngineError::Provider(ProviderError::Lock(LockError::Interrupted { .. })) => {
                "interrupted"
            }
```

replace

```rust
            EngineError::Lock(LockError::Timeout(_)) => "lock-timeout",
```

with

```rust
            EngineError::Lock(LockError::Timeout(_)) => "lock-timeout",
            EngineError::Lock(LockError::Interrupted { .. }) => "interrupted",
```

and replace the end of `impl EngineError`:

```rust
            EngineError::Io(_) => "io",
        }
    }
}
```

with:

```rust
            EngineError::Io(_) => "io",
            EngineError::Interrupted(_) => "interrupted",
        }
    }

    /// The signal behind an interruption, whichever carrier holds it: `Interrupted`,
    /// `Lock(LockError::Interrupted)`, or `Provider(ProviderError::Lock(LockError::Interrupted))`
    /// (§14.1). `None` for every other error.
    pub fn signal(&self) -> Option<i32> {
        match self {
            EngineError::Interrupted(signal) => Some(*signal),
            EngineError::Lock(e) | EngineError::Provider(ProviderError::Lock(e)) => e.signal(),
            _ => None,
        }
    }
}
```

In `crates/tagteam-engine/src/engine.rs`, replace

```rust
use tagteam_provider::{Clock, Env, Http, MutationGuard, Provider, Read};
```

with

```rust
use tagteam_provider::{Cancel, Clock, Env, Http, MutationGuard, Provider, Read};
```

then add after `pub fn env(&self) -> &Env { … }`:

```rust

    /// The cancel token every cancellation point checks (§4.2, §14.1). It is the Env's, so
    /// every clone of that Env shares it; the CLI registers its signal handlers on it.
    pub fn cancel(&self) -> &Cancel {
        &self.env.cancel
    }
```

replace the doc comment of `guard_recovering`:

```rust
    /// `mutation_guard`, with the refusal for each row whose recovery could not take its
    /// provider's live locks (`RecoveryBlocked`), by provider. With `ask_oracle` false the
    /// rows are recovered from fingerprints alone: no network call (§7.6, §9.6).
```

with

```rust
    /// `mutation_guard`, with the refusal for each row whose recovery could not take its
    /// provider's live locks (`RecoveryBlocked`), by provider. With `ask_oracle` false the
    /// rows are recovered from fingerprints alone: no network call (§7.6, §9.6). A recovery
    /// interrupted at one of its lock waits ends the command with that interruption (§14.1).
```

and, in its body, replace

```rust
            if let Err(e) = self.recover_one(&guard, &row, hint) {
                tracing::warn!(provider = %row.provider, "could not recover an interrupted switch: {e}");
```

with

```rust
            if let Err(e) = self.recover_one(&guard, &row, hint) {
                // Interrupted at a lock wait, the recovery wrote nothing and its row stays for
                // the next command. Reported as itself, never as a switch it could not settle.
                if e.signal().is_some() {
                    return Err(e);
                }
                tracing::warn!(provider = %row.provider, "could not recover an interrupted switch: {e}");
```

`recover_one`'s own mapping (`ProviderError::Lock(LockError::Timeout(lock))` →
`RecoveryBlocked`) stays as it is: an interruption falls to its `e => e.into()` arm and keeps
its signal.

- [ ] **Step 9: Run them to verify they pass**

Run: `cargo test -p tagteam-engine --features test-hooks --test cancel`
Expected: PASS, 7 tests, in well under a second each.

Run: `cargo test -p tagteam-engine --test cancel`
Expected: PASS, 6 tests (the hook test is compiled out).

Run: `cargo test -p tagteam-engine --lib error::tests`
Expected: PASS.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. No existing test sets a token, and `mutation_lock_free` answers as before.

- [ ] **Step 10: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings. Without
the feature, `tests/cancel.rs` has no unused import: every import it has is used by a test that
is not gated on `test-hooks`.

- [ ] **Step 11: Commit**

```bash
git add crates/tagteam-engine/src/error.rs crates/tagteam-engine/src/engine.rs \
  crates/tagteam-engine/tests/common/mod.rs crates/tagteam-engine/tests/cancel.rs
git commit -m "Report an interrupted lock wait as interrupted, recovery included"
```

- [ ] **Step 12: Write the failing test: a signal never stops the live write after a request**

The integrator's ruling. In `Engine::refresh_active` (`crates/tagteam-engine/src/active.rs`),
`request_active` sends the token request and persists the successor, then calls `publish`,
which waits for CC's config lock and writes the live store. Since Step 3, that wait honours
`env.cancel`. A signal there would leave CC holding the generation the request just consumed,
and CC's next refresh would get `invalid_grant`. The wait must run to completion or time out.

In `crates/tagteam-engine/tests/active.rs`, inside `mod hooks` (it already imports `Arc`,
`Mutex`, `Duration` and the outer module's items), add before the doc comment of
`take_over_after_response` (`/// Makes the credential lock look taken over once the response
arrives (§9.1), so the`):

```rust
    /// §14.1, §7.5 step 5: the live write after a request is inside the active refresh's
    /// critical span. The request consumed the generation CC holds, so a Ctrl-C while the
    /// write waits for CC's config lock must not stop it: CC would be left with a spent token.
    #[test]
    fn a_signal_while_the_successor_waits_for_the_config_lock_still_publishes_it() {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        let (config, cancel) = (fx.paths().config_lock, fx.engine.cancel().clone());
        let cc = Arc::new(Mutex::new(None));
        let started = cc.clone();
        // After the request, CC holds its config lock; a Ctrl-C arrives 100 ms into the wait,
        // and CC lets go 200 ms later.
        fx.engine.on_point(
            "active-before-publish",
            Box::new(move || {
                fs::create_dir(&config).unwrap();
                let (config, cancel) = (config.clone(), cancel.clone());
                *started.lock().unwrap() = Some(std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(100));
                    cancel.request(libc::SIGINT);
                    std::thread::sleep(Duration::from_millis(200));
                    fs::remove_dir(&config).unwrap();
                }));
            }),
        );

        let out = active(&fx, ActiveTrigger::Expired);
        cc.lock().unwrap().take().unwrap().join().unwrap();

        assert_eq!(out.unwrap(), ActiveOutcome::Refreshed);
        assert_eq!(
            fx.live_refresh_token().as_deref(),
            Some("rt-a2"),
            "CC holds the successor"
        );
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
        assert_eq!(
            fx.engine.cancel().requested(),
            Some(libc::SIGINT),
            "the signal waits for the next cancellation point"
        );
        assert!(!fx.paths().refresh_lock.exists() && !fx.paths().config_lock.exists());
    }

```

`active-before-publish` runs after the request and the vault write, and just before `publish`,
so the config lock is held only for this wait. The mutation, account and credential locks were
all taken before it, with the token unset.

- [ ] **Step 13: Run it to verify it fails**

Run: `cargo test -p tagteam-engine --features test-hooks --test active a_signal_while`
Expected: FAIL at the first assertion, `left: PersistedNotPublished`, `right: Refreshed`. The
config wait's first attempt meets CC's lock, and its retry (at least 250 ms in) finds the
token already set (100 ms in). So `publish` reports "not published", and the live store
keeps `rt-a`, the generation the request consumed.

- [ ] **Step 14: Wait for the config lock after a request under a token nothing sets**

In `crates/tagteam-engine/src/active.rs`, replace

```rust
use tagteam_provider::{
    CredLocks, Credential, LiveChange, Provenance, Provider, ProviderError, Read, RefreshResult,
    StoredLogin,
};
```

with

```rust
use tagteam_provider::{
    Cancel, CredLocks, Credential, LiveChange, Provenance, Provider, ProviderError, Read,
    RefreshResult, StoredLogin,
};
```

In `refresh_active`, replace the self-heal's call

```rust
                if !self.publish(p, &row, cred, &rec.current, &rec.retire)? {
```

with

```rust
                // Nothing was sent this pass, so a signal may end this wait (§14.1).
                if !self.publish(p, &row, cred, &rec.current, &rec.retire, &self.env.cancel)? {
```

In `request_active`, replace

```rust
                // CC must hold the newest generation whatever became of tagteam's copy.
                let published = if owned {
                    hooks::point(self, "active-before-publish")
                        .and_then(|()| self.publish(p, row, cred, received.bytes(), &rec.retire))
```

with

```rust
                // CC must hold the newest generation whatever became of tagteam's copy. The
                // request consumed the one CC holds, so this wait is inside §7.5's critical
                // span: it waits under a token nothing sets, and a signal waits for the next
                // cancellation point (§14.1). A timeout still leaves it to step 3's self-heal.
                let published = if owned {
                    let uncancelled = Cancel::new();
                    hooks::point(self, "active-before-publish").and_then(|()| {
                        self.publish(p, row, cred, received.bytes(), &rec.retire, &uncancelled)
                    })
```

Replace `publish`'s doc comment and signature head:

```rust
    /// Writes `secret` to the live store under the config lock, taken now with its own budget
    /// (§9.1), then retires `retire` once the write reads back. `false` when the live store
    /// was not written; the caller reports `PersistedNotPublished`, and the next pass
    /// reconciles it (§7.5 step 3). What the write destroys is
    /// saved first unless a vault generation already holds it (§9.4 step 7's rule).
    fn publish(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        cred: CredLocks<'_>,
        secret: &[u8],
        retire: &[PathBuf],
    ) -> Result<bool, EngineError> {
```

with

```rust
    /// Writes `secret` to the live store under the config lock, taken now with its own budget
    /// (§9.1) and waited for under `cancel` (§14.1), then retires `retire` once the write reads
    /// back. `false` when the live store was not written; the caller reports
    /// `PersistedNotPublished`, and the next pass reconciles it (§7.5 step 3). What the write
    /// destroys is saved first unless a vault generation already holds it (§9.4 step 7's rule).
    fn publish(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        cred: CredLocks<'_>,
        secret: &[u8],
        retire: &[PathBuf],
        cancel: &Cancel,
    ) -> Result<bool, EngineError> {
```

and, in its body, replace

```rust
        let locks = match p.lock_config(&self.env, cred, p.live_lock_budget()) {
```

with

```rust
        let mut env = self.env.clone();
        env.cancel = cancel.clone();
        let locks = match p.lock_config(&env, cred, p.live_lock_budget()) {
```

Only the config-lock wait uses `env`; the rest of `publish` keeps `self.env`. `lock_config`
reaches the token through the `Env` it is given (`ClaudeCode::lock_config` passes
`&env.cancel`, Step 3), so an `Env` clone with a fresh token is the smallest change that
reaches it. It adds no parameter to the `Provider` trait.

- [ ] **Step 15: Run it to verify it passes, and the timeout path still does what it did**

Run: `cargo test -p tagteam-engine --features test-hooks --test active`
Expected: PASS, including the new test, which takes about 300–800 ms: the config wait's
retries go on until CC lets go at 300 ms. `a_self_heal_that_cannot_publish_sends_nothing_and_says_so`
(a held config lock with a 300 ms budget) still gets `PersistedNotPublished`: a timeout keeps
today's behaviour.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS.

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings. `publish`
has seven inputs counting `self`, which is within clippy's `too_many_arguments` limit.

- [ ] **Step 16: Commit**

```bash
git add crates/tagteam-engine/src/active.rs crates/tagteam-engine/tests/active.rs
git commit -m "Never let a signal stop publishing a refreshed successor to the live store"
```

---

### Task 3: The CC lock heartbeat touches through a held fd (L356)

The M1 final review's item, quoted because the review file is local and git-ignored:
"**L356** (T10): heartbeat touch-by-path can adopt a replacement lock directory in a stat→open
window after a ≥ stale−3 s stall (hold a dir fd + futimens, compare dev/ino) — same gap as
proper-lockfile" — CAN WAIT "(M3, the long-lived holders)". The heartbeat checks the path's
mtime, then opens the path again to touch it. A holder that stalls between those two steps for
the staleness window lets another process remove the directory and make its own. On resuming,
the holder opens and touches the replacement. The replacement then carries this holder's mtime,
every later check passes, and `Drop` removes another holder's lock. M3b's `auto` is a
long-lived holder that a closed laptop lid suspends.

The same adoption can happen at acquisition, which Codex's plan review found (round 1): a holder
that stalls between its `mkdir` and opening the directory can open, record and touch the
replacement as its own. Holding an fd closes the first window only if the fd really is this
holder's directory, so this task closes both. Round 2 found a third path: `start`'s cleanup on
error removed the path unconditionally. If the open failed in CC's remove/create gap, that
cleanup would remove CC's replacement. This task closes that path too.

§9.1 (amended in `a7a32c3`): "a thread touches the directory's mtime every 3 s, through a
directory fd opened when the lock was acquired (`futimens`), never by path … An ownership check
confirms that the path still names the directory tagteam created (the same device and inode as
the held fd) and that it still carries the mtime tagteam last set … A directory that replaced
tagteam's after a long stall, as a suspended `auto` can meet, is therefore never touched or
removed." Review Focus 5 asks for the pinning test.

**Readings of the spec this task commits to:**
- **"The directory tagteam created" is the one the fd opened right after `mkdir`, and only if
  that open came soon enough.** The wall clock is read just before `mkdir`, and again after the
  fd is open and its device and inode are recorded, before any touch. If that span reaches
  `trusted_span(stale)`, four fifths of the staleness, the directory may have looked stale and
  been replaced before the open. The attempt then touches and removes nothing, drops the fd and
  returns `Ok(None)`. Under that bound the directory's age, measured from its `mkdir`, never
  exceeded the span, so no taker following the protocol could have judged it stale.
- **Why the wall clock.** Staleness is judged on the wall clock: the taker compares its `now`
  with the directory's mtime. macOS's monotonic clock stops during sleep, so `Instant` would miss
  the very suspension this guards against. A clock set back during the stall reads as no time
  spent, and it makes the directory look younger to every taker too.
- **Why a fifth as the margin.** A taker whose filesystem keeps mtimes to the second (HFS+, FAT,
  some network filesystems) can see the directory up to 1 s older than it is. A fifth of the
  staleness is 2 s for CC's 10 s config lock and 12 s for its 60 s credential locks, at least
  twice that. The bound only matters after a stall of 8 s or more between two adjacent syscalls,
  which only a suspension produces, so a generous margin costs nothing. It scales with the
  staleness, so short test locks (200 ms) stay acquirable. A lock with zero staleness would never
  be acquired, and none exists.
- **Why contention, not `Compromised`.** The holder cannot tell whose the directory is, which is
  exactly the state of a contended lock. `Compromised` means a lock that was held was lost, and
  callers abort the protected write on it, but nothing was held yet. So `try_acquire` returns
  `Ok(None)`. `MkdirLock::acquire` retries within its deadline or ends with `Timeout`, and CC's
  legacy-lock try treats it as legacy contention: every caller already handles both. If the
  directory is CC's, it is a held lock. If it is the holder's own orphan, it goes stale like any
  other and is taken over by the existing rule (§9.1), by this retry or by CC.
- **A failed start removes the directory only once ownership is established, and only while it
  still holds.** Before the fd is open, its identity recorded and the span within bound, the
  directory may be another holder's: a failure there (the open, the `fstat`) leaves the path
  untouched, exactly as the tripped bound does. After that, a failure (the first touch, the
  heartbeat's spawn) still removes the fresh directory rather than block CC for the whole
  staleness window, as today. But it goes through `remove_if_ours`, which removes the path only
  while it has the held fd's device and inode and, once touched, the mtime the holder set. The
  fd is still held during that check, so the inode cannot have been reused.
- **Ownership means two things together.** The path's device and inode, read with
  `symlink_metadata` (the path itself must be the directory, never a link to one), must equal the
  held fd's, which are read once at acquisition. And the path's mtime must equal the one last
  set. Holding the fd keeps the inode allocated, so a replacement cannot reuse the number while
  this holder lives. The mtime comparison still catches a takeover in place, such as another
  process re-touching the same directory.
- **Every touch goes through the fd and is read back through the fd** (`futimens`, then
  `fstat`). A touch that lands after a takeover reaches this holder's own unlinked directory,
  which is harmless, and the next check flags the guard compromised. This covers a replacement
  made between a beat's check and its touch, the original L356 window.
- **The residual gap is `Drop`.** The final check and the `rmdir` are two steps, and POSIX has
  no `rmdir` by fd. A takeover between them needs the holder to stall for the whole staleness
  window inside that gap. This is the same residual as `proper-lockfile`'s, and it is recorded
  in a comment.
- **The protected write is aborted, not just the heartbeat.** `check_owned` runs before every
  write the lock protects, through `LiveLocks::check_owned` (CC's `guarded`). The provider-level
  test pins that a write is refused with `LockError::Compromised` and changes nothing.
- **Test seams, not timing.** The windows above are microseconds wide, so the tests reach them
  through a `#[cfg(test)]` seam: hooks keyed by lock path, run at four points. The points are
  after `mkdir`, before the heartbeat starts, at a beat before its check, and at a beat between
  its check and its touch.
  - A hook can fail the step at its point, which models a failed open or spawn; the beat's
    points ignore that.
  - The beat's hooks run with the timestamp mutex held, so a `check_owned` the test makes after
    the hook fires runs after that beat's touch, with no sleep.
  - A stall is a real sleep in the hook, longer than the test lock's 200 ms staleness, so the
    elapsed wall time is guaranteed.
  - Release builds contain none of this.

**Files:**
- Modify: `crates/tagteam-provider/src/mkdir_lock.rs` (`State`, `try_acquire`, `start` and its
  heartbeat, `check_owned`, `check_with`, `Drop`, a new `remove_if_ours`, a new
  `#[cfg(test)] mod seam`, and its `mod tests`; `set_dir_mtime` moves into `mod tests`)
- Modify: `crates/tagteam-cc/tests/provider.rs`

**Interfaces:**
- Consumes: Task 1's `MkdirLockSpec` (with `cancel`) and the `spec` test helper; the existing
  `MkdirLock::{acquire, try_acquire, check_owned, is_compromised}`; and in `tagteam-cc`'s tests,
  `fx()`, `target`, `save_nothing`, `ClaudeCode::{lock_live, write_credential}` and
  `keychain_service` / `keychain_account`.
- Produces: no public signature changes. `MkdirLock::try_acquire` keeps its signature and now
  also returns `Ok(None)` for an attempt that stalled past `trusted_span` between `mkdir` and
  the open.
  - New private items in `mkdir_lock.rs`: `fn touch(dir: &File, t: SystemTime) -> io::Result<SystemTime>`,
    `fn trusted_span(stale: Duration) -> Duration`,
    `fn remove_if_ours(path: &Path, id: DirId, set: Option<SystemTime>)`,
    `struct DirId { dev: u64, ino: u64 }` with `DirId::of(&fs::Metadata)`, and the `State`
    fields `dir: File` and `id: DirId`.
  - `fn start(spec: &MkdirLockSpec, made_at: SystemTime) -> Result<Option<Self>, LockError>`
    (was `-> Result<Self, LockError>`). Its unconditional `remove_dir` cleanup on error is gone.
  - Test-only: `mod seam` with
    `enum Point { AfterMkdir, BeforeHeartbeat, BeatBeforeCheck, BeatBeforeTouch }`,
    `seam::set(path, point, hook: impl Fn() -> io::Result<()> + …)` and
    `seam::run(path, point) -> io::Result<()>`.
  - The crate-private `set_dir_mtime` leaves production code and becomes a test helper.

- [ ] **Step 1: Add the seams and write the failing tests**

In `crates/tagteam-provider/src/mkdir_lock.rs`, add the seam module directly above
`#[cfg(test)] mod tests`:

```rust
/// Test-only seams in the lock's protocol: a test runs code at a named point, for one lock path
/// only, so tests running in parallel never meet each other's hooks.
#[cfg(test)]
mod seam {
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum Point {
        /// `mkdir` made the directory; it has not been opened yet.
        AfterMkdir,
        /// The directory is the holder's and touched; the heartbeat has not been started.
        BeforeHeartbeat,
        /// A heartbeat woke and holds the timestamp mutex; its ownership check has not run.
        BeatBeforeCheck,
        /// A heartbeat's ownership check passed; its touch has not run.
        BeatBeforeTouch,
    }

    type Hook = Arc<dyn Fn() -> io::Result<()> + Send + Sync>;

    static HOOKS: Mutex<Vec<(PathBuf, Point, Hook)>> = Mutex::new(Vec::new());

    /// Runs `hook` each time the lock at `path` passes `point`. A heartbeat hook runs with the
    /// timestamp mutex held, so it must never call `check_owned` on that lock.
    pub(super) fn set(
        path: &Path,
        point: Point,
        hook: impl Fn() -> io::Result<()> + Send + Sync + 'static,
    ) {
        HOOKS
            .lock()
            .unwrap()
            .push((path.to_path_buf(), point, Arc::new(hook)));
    }

    /// Runs the hooks set for `path` at `point`, in the order they were set. The first error
    /// is returned as the failure of the step at that point; the heartbeat's points ignore it.
    pub(super) fn run(path: &Path, point: Point) -> io::Result<()> {
        let hooks: Vec<Hook> = HOOKS
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, at, _)| p == path && *at == point)
            .map(|(_, _, hook)| hook.clone())
            .collect();
        for hook in hooks {
            hook()?;
        }
        Ok(())
    }
}

```

Put the four seams into the current code. In `start`, replace

```rust
        let result = (|| -> Result<Self, LockError> {
            let set = set_dir_mtime(&spec.path, SystemTime::now())?;
```

with

```rust
        let result = (|| -> Result<Self, LockError> {
            #[cfg(test)]
            seam::run(&spec.path, seam::Point::AfterMkdir)?;
            let set = set_dir_mtime(&spec.path, SystemTime::now())?;
```

then replace

```rust
                wake: Condvar::new(),
            });
            let (st, path, every) = (state.clone(), spec.path.clone(), spec.touch_every);
```

with

```rust
                wake: Condvar::new(),
            });
            #[cfg(test)]
            seam::run(&spec.path, seam::Point::BeforeHeartbeat)?;
            let (st, path, every) = (state.clone(), spec.path.clone(), spec.touch_every);
```

and in the heartbeat closure, replace

```rust
                    let mut last = st.last_set.lock().unwrap();
                    if check_with(&path, &st, *last).is_ok() {
                        match set_dir_mtime(&path, SystemTime::now()) {
```

with

```rust
                    let mut last = st.last_set.lock().unwrap();
                    #[cfg(test)]
                    let _ = seam::run(&path, seam::Point::BeatBeforeCheck);
                    if check_with(&path, &st, *last).is_ok() {
                        #[cfg(test)]
                        let _ = seam::run(&path, seam::Point::BeatBeforeTouch);
                        match set_dir_mtime(&path, SystemTime::now()) {
```

Inside `mod tests`, replace the imports at the top of the module:

```rust
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::SystemTime;
```

with:

```rust
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::sync::mpsc;
    use std::time::SystemTime;

    use super::seam::Point;

    /// Sets a directory's mtime by path, as another process would.
    fn set_dir_mtime(path: &Path, t: SystemTime) -> io::Result<SystemTime> {
        File::open(path)?.set_modified(t)?;
        fs::metadata(path)?.modified()
    }

    /// Replaces the directory at `path` with a new one that carries `mtime`: another process's
    /// takeover. Carrying the holder's own mtime, it is what the old touch-by-path left after
    /// adopting a replacement (L356).
    fn replace_with_mtime(path: &Path, mtime: SystemTime) {
        fs::remove_dir(path).unwrap();
        fs::create_dir(path).unwrap();
        assert_eq!(set_dir_mtime(path, mtime).unwrap(), mtime);
    }

    fn mtime(path: &Path) -> SystemTime {
        fs::metadata(path).unwrap().modified().unwrap()
    }

    /// Runs `hook` the first time the lock at `path` passes `point`, then sends what it returns.
    fn once_at<T: Send + 'static>(
        path: &Path,
        point: Point,
        hook: impl Fn() -> T + Send + Sync + 'static,
    ) -> mpsc::Receiver<T> {
        let (tx, rx) = mpsc::channel();
        let fired = AtomicBool::new(false);
        seam::set(path, point, move || {
            if !fired.swap(true, Ordering::SeqCst) {
                let _ = tx.send(hook());
            }
            Ok(())
        });
        rx
    }

    /// Makes the step at `point` fail for the lock at `path`, after the hooks set before this.
    fn fail_at(path: &Path, point: Point) {
        seam::set(path, point, || Err(io::Error::other("injected failure")));
    }
```

(This local `set_dir_mtime` shadows the module's own crate-private one until Step 3 removes it.
The existing tests that call it, `a_stale_lock_is_taken_over`,
`a_non_removable_stale_lock_reports_io_promptly_not_timeout` and
`the_heartbeat_notices_an_external_takeover`, are unchanged.)

Add these eight tests before `a_panic_releases_the_lock`:

```rust
    /// L356, Review Focus 5: a holder suspended past the staleness window resumes to find its
    /// directory replaced, and the replacement carries the very mtime it last set. Only the
    /// device and inode tell them apart. Its check fails, so the write it protects is aborted,
    /// and it never removes the replacement.
    #[test]
    fn a_replacement_carrying_the_holders_mtime_is_still_not_its_own() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 3_600_000); // heartbeat parked: suspended
        let held = MkdirLock::acquire(&s).unwrap();
        let ours = mtime(&s.path);
        replace_with_mtime(&s.path, ours);

        assert!(matches!(held.check_owned(), Err(LockError::Compromised(_))));
        drop(held);
        assert!(
            s.path.is_dir(),
            "the resumed holder removed its replacement"
        );
        assert_eq!(mtime(&s.path), ours);
    }

    /// L356: a beat's own check refuses a directory that replaced the holder's before the beat,
    /// even one carrying the holder's mtime, so the beat never touches it.
    #[test]
    fn the_heartbeat_never_touches_a_directory_that_replaced_its_own() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 20);
        let path = s.path.clone();
        let swapped = once_at(&s.path, Point::BeatBeforeCheck, move || {
            let ours = mtime(&path);
            replace_with_mtime(&path, ours);
            ours
        });
        let held = MkdirLock::acquire(&s).unwrap();

        let ours = swapped.recv_timeout(Duration::from_secs(10)).unwrap();
        // The beat holds the timestamp mutex from its check through its touch, so this check
        // runs after the beat has finished.
        let checked = held.check_owned();
        assert_eq!(mtime(&s.path), ours, "the beat touched the replacement");
        assert!(matches!(checked, Err(LockError::Compromised(_))));
        assert!(held.is_compromised());
        drop(held);
        assert!(
            s.path.is_dir(),
            "a compromised guard leaves the directory alone"
        );
    }

    /// L356 itself: the beat's check passed, and the directory is replaced before its touch.
    /// The touch goes through the fd held since `mkdir`, so it reaches the holder's own
    /// (now unlinked) directory and never the replacement; the next check refuses.
    #[test]
    fn a_directory_replaced_between_the_beats_check_and_its_touch_is_never_touched() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 20);
        let path = s.path.clone();
        let swapped = once_at(&s.path, Point::BeatBeforeTouch, move || {
            let ours = mtime(&path);
            replace_with_mtime(&path, ours);
            ours
        });
        let held = MkdirLock::acquire(&s).unwrap();

        let ours = swapped.recv_timeout(Duration::from_secs(10)).unwrap();
        let checked = held.check_owned(); // after the beat's touch, as above
        assert_eq!(mtime(&s.path), ours, "the beat touched the replacement");
        assert!(matches!(checked, Err(LockError::Compromised(_))));
        drop(held);
        assert!(
            s.path.is_dir(),
            "a compromised guard leaves the directory alone"
        );
    }

    /// A holder suspended between its `mkdir` and opening the directory, long enough for the
    /// directory to look stale, may find another holder's directory there on resume. It never
    /// adopts, touches or removes it: the attempt counts as contention.
    #[test]
    fn a_stall_between_mkdir_and_open_never_adopts_the_directory_found_after_it() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 200, 0, 3_000); // stale after 200 ms; one attempt
        let path = s.path.clone();
        let theirs = once_at(&s.path, Point::AfterMkdir, move || {
            std::thread::sleep(Duration::from_millis(250)); // suspended past the staleness
            fs::remove_dir(&path).unwrap(); // CC took it over
            fs::create_dir(&path).unwrap();
            let m = fs::metadata(&path).unwrap();
            (m.ino(), m.modified().unwrap())
        });

        match MkdirLock::acquire(&s) {
            Err(LockError::Timeout(_)) => {}
            Err(e) => panic!("expected contention, got {e:?}"),
            Ok(_) => panic!("the holder adopted the directory that replaced its own"),
        }
        let (ino, modified) = theirs.try_recv().unwrap();
        let now = fs::metadata(&s.path).unwrap();
        assert_eq!(
            (now.ino(), now.modified().unwrap()),
            (ino, modified),
            "CC's directory is neither touched nor removed"
        );
    }

    /// The same stall with no takeover leaves the holder's own directory as a contended lock.
    /// It goes stale like any other and the retry takes it over (§9.1), within the deadline.
    #[test]
    fn a_stall_with_no_takeover_is_retried_and_the_lock_taken() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 200, 5_000, 3_000);
        let stalled = once_at(&s.path, Point::AfterMkdir, || {
            std::thread::sleep(Duration::from_millis(250));
        });

        let held = MkdirLock::acquire(&s).unwrap();
        stalled.try_recv().unwrap();
        assert!(held.check_owned().is_ok());
        drop(held);
        assert!(!s.path.exists());
    }

    /// A start whose open fails after `mkdir` never removes the path: by then it may name
    /// another holder's directory. Here CC removed the stale directory, the open found nothing,
    /// and CC made its own before the failure unwound.
    #[test]
    fn a_failed_open_after_mkdir_never_removes_the_directory_found_there() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 0, 3_000);
        let path = s.path.clone();
        let theirs = once_at(&s.path, Point::AfterMkdir, move || {
            fs::remove_dir(&path).unwrap();
            fs::create_dir(&path).unwrap();
            let m = fs::metadata(&path).unwrap();
            (m.ino(), m.modified().unwrap())
        });
        fail_at(&s.path, Point::AfterMkdir); // the open fails

        assert!(matches!(MkdirLock::acquire(&s), Err(LockError::Io(_))));
        let (ino, modified) = theirs.try_recv().unwrap();
        assert!(s.path.is_dir(), "the failed start removed CC's directory");
        let now = fs::metadata(&s.path).unwrap();
        assert_eq!(
            (now.ino(), now.modified().unwrap()),
            (ino, modified),
            "CC's directory is neither touched nor removed"
        );
    }

    /// A start that fails once the directory is its own removes it, rather than leave CC
    /// blocked for the whole staleness window.
    #[test]
    fn a_start_that_fails_after_taking_its_directory_removes_it() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 0, 3_000);
        fail_at(&s.path, Point::BeforeHeartbeat);

        assert!(matches!(MkdirLock::acquire(&s), Err(LockError::Io(_))));
        assert!(!s.path.exists());
    }

    /// The same failure after a takeover removes nothing: the cleanup is fenced by the held
    /// fd's device and inode and by the mtime the holder set.
    #[test]
    fn a_start_that_fails_after_a_takeover_leaves_the_replacement() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 0, 3_000);
        let path = s.path.clone();
        let theirs = once_at(&s.path, Point::BeforeHeartbeat, move || {
            let ours = mtime(&path);
            replace_with_mtime(&path, ours);
            fs::metadata(&path).unwrap().ino()
        });
        fail_at(&s.path, Point::BeforeHeartbeat);

        assert!(matches!(MkdirLock::acquire(&s), Err(LockError::Io(_))));
        let ino = theirs.try_recv().unwrap();
        assert!(s.path.is_dir(), "the failed start removed the replacement");
        assert_eq!(fs::metadata(&s.path).unwrap().ino(), ino);
    }

```

In `crates/tagteam-cc/tests/provider.rs`, add before the doc comment that begins
`/// §9.1: one budget covers both stages.`:

```rust
/// L356, Review Focus 5: tagteam was suspended past the staleness window, and Claude Code took
/// the refresh lock over with a directory that carries tagteam's last mtime. The write the lock
/// protects is refused, and releasing tagteam's locks leaves CC's directory where it is.
#[test]
fn a_replaced_lock_directory_aborts_the_write_and_is_left_alone() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    let acct = keychain_account(&f.env);
    let live = br#"{"claudeAiOauth":{"refreshToken":"old"}}"#;
    f.kc.put(&svc, &acct, live);
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let ours = fs::metadata(&paths.refresh_lock)
        .unwrap()
        .modified()
        .unwrap();
    fs::remove_dir(&paths.refresh_lock).unwrap();
    fs::create_dir(&paths.refresh_lock).unwrap(); // CC's
    fs::File::open(&paths.refresh_lock)
        .unwrap()
        .set_modified(ours)
        .unwrap();

    let t = target(&f, "new@b.co", "rt-new");
    assert!(matches!(
        f.cc.write_credential(&f.env, &locks, &t, &mut save_nothing),
        Err(ProviderError::Lock(LockError::Compromised(_)))
    ));
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), live, "nothing was written");
    drop(locks);
    assert!(paths.refresh_lock.is_dir(), "CC's directory is left alone");
    assert!(
        !paths.legacy_lock().exists() && !paths.config_lock.exists(),
        "tagteam's own locks are released"
    );
}

```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-provider --lib mkdir_lock::tests`
Expected: FAIL, six tests. Every other `mkdir_lock` test passes, and so do two of the new ones,
which guard behaviour the fix must keep:
- `a_stall_with_no_takeover_is_retried_and_the_lock_taken`: today's code adopts its own
  directory after the stall, the right outcome there. It guards that the fix keeps such a lock
  acquirable.
- `a_start_that_fails_after_taking_its_directory_removes_it`: today's unconditional cleanup
  removes it. It guards that the fenced cleanup still does.

The six failures:
- `a_replacement_carrying_the_holders_mtime_is_still_not_its_own`: at
  `matches!(held.check_owned(), Err(LockError::Compromised(_)))`. The check compares only the
  mtime, and the replacement carries the holder's.
- `the_heartbeat_never_touches_a_directory_that_replaced_its_own`: `the beat touched the
  replacement`. The beat's mtime-only check passed, and it touched the replacement by path.
- `a_directory_replaced_between_the_beats_check_and_its_touch_is_never_touched`: `the beat
  touched the replacement`. The touch went by path, to the replacement.
- `a_stall_between_mkdir_and_open_never_adopts_the_directory_found_after_it`: `the holder
  adopted the directory that replaced its own`. `start` opened and touched CC's directory.
- `a_failed_open_after_mkdir_never_removes_the_directory_found_there`: `the failed start
  removed CC's directory`. The error cleanup removes the path unconditionally.
- `a_start_that_fails_after_a_takeover_leaves_the_replacement`: `the failed start removed the
  replacement`, for the same reason.

Run: `cargo test -p tagteam-cc --test provider a_replaced_lock_directory`
Expected: FAIL at the `write_credential` `matches!`: the fence's check passes, so the write
goes ahead.

- [ ] **Step 3: Hold the fd from `mkdir`, bound the span before the open, fence the startup cleanup, touch through the fd and check the inode**

In `crates/tagteam-provider/src/mkdir_lock.rs`, replace

```rust
use std::os::unix::fs::DirBuilderExt;
```

with

```rust
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
```

Replace `set_dir_mtime` and the head of `State`:

```rust
pub(crate) fn set_dir_mtime(path: &Path, t: SystemTime) -> io::Result<SystemTime> {
    File::open(path)?.set_modified(t)?;
    fs::metadata(path)?.modified()
}

struct State {
    last_set: Mutex<SystemTime>,
```

with:

```rust
/// Sets the mtime through `dir`, the held directory fd (`futimens`), and reads back what the
/// filesystem stored through the same fd. Never by path (§9.1, L356).
fn touch(dir: &File, t: SystemTime) -> io::Result<SystemTime> {
    dir.set_modified(t)?;
    dir.metadata()?.modified()
}

/// How long `mkdir` may take to become an open fd before the directory opened can no longer be
/// trusted to be the one `mkdir` made: four fifths of the staleness. Within it the directory
/// cannot have looked stale to anyone, so nobody could have taken it over. The fifth held back
/// covers a taker whose filesystem keeps mtimes to the second, which can make the directory
/// look up to 1 s older: 2 s for CC's 10 s config lock, 12 s for its 60 s credential locks.
fn trusted_span(stale: Duration) -> Duration {
    stale - stale / 5
}

/// Removes the lock directory at `path` only while it is still the one this holder made: the
/// device and inode of its held fd, and, once touched, the mtime it set (§9.1). For a start that
/// fails after the directory was established as its own; the caller still holds the fd, so the
/// inode cannot have been reused.
fn remove_if_ours(path: &Path, id: DirId, set: Option<SystemTime>) {
    let ours = fs::symlink_metadata(path)
        .is_ok_and(|m| DirId::of(&m) == id && set.is_none_or(|t| m.modified().ok() == Some(t)));
    if ours {
        let _ = fs::remove_dir(path);
    }
}

/// A directory's identity: the device and inode the lock path must still resolve to (§9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirId {
    dev: u64,
    ino: u64,
}

impl DirId {
    fn of(m: &fs::Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
        }
    }
}

struct State {
    /// The directory this holder created, open since just after `mkdir`. Every touch goes
    /// through it, so a heartbeat never reaches a directory that replaced it; holding it also
    /// keeps its inode allocated, so no replacement can reuse the number.
    dir: File,
    /// `dir`'s device and inode, read once when the lock was taken.
    id: DirId,
    last_set: Mutex<SystemTime>,
```

Replace the head of `try_acquire`:

```rust
    pub fn try_acquire(spec: &MkdirLockSpec) -> Result<Option<Self>, LockError> {
        for _ in 0..2 {
            match fs::create_dir(&spec.path) {
                Ok(()) => return Self::start(spec).map(Some),
```

with:

```rust
    /// One attempt. `None` when the lock is held, or when this attempt stalled so long between
    /// `mkdir` and opening the directory that the directory may no longer be its own (`start`).
    pub fn try_acquire(spec: &MkdirLockSpec) -> Result<Option<Self>, LockError> {
        for _ in 0..2 {
            // Wall-clock time, as staleness is judged: a suspension counts (§9.1).
            let made_at = SystemTime::now();
            match fs::create_dir(&spec.path) {
                Ok(()) => return Self::start(spec, made_at),
```

Replace the whole of `start`, as Step 1 left it:

```rust
    fn start(spec: &MkdirLockSpec) -> Result<Self, LockError> {
        let result = (|| -> Result<Self, LockError> {
            #[cfg(test)]
            seam::run(&spec.path, seam::Point::AfterMkdir)?;
            let set = set_dir_mtime(&spec.path, SystemTime::now())?;
            let state = Arc::new(State {
                last_set: Mutex::new(set),
                compromised: AtomicBool::new(false),
                stop: Mutex::new(false),
                wake: Condvar::new(),
            });
            #[cfg(test)]
            seam::run(&spec.path, seam::Point::BeforeHeartbeat)?;
            let (st, path, every) = (state.clone(), spec.path.clone(), spec.touch_every);
            // `Builder::spawn` (not `thread::spawn`) so a failure to spawn is an error we
            // can clean up after, not a panic.
            let heartbeat = thread::Builder::new().spawn(move || {
                let mut stop = st.stop.lock().unwrap();
                loop {
                    // The standard condvar predicate loop: `wait_timeout_while` checks `*stop`
                    // immediately, under the same lock `Drop` sets it under, before ever
                    // blocking. Without that check-before-wait, a `Drop` that sets `stop` and
                    // notifies before this thread reaches its first wait is lost entirely — the
                    // notify has nothing waiting to wake, and the plain `wait_timeout` used to
                    // block regardless for the full `every`, even though `*stop` was already
                    // true by the time it acquired the lock.
                    let (guard, _) = st.wake.wait_timeout_while(stop, every, |s| !*s).unwrap();
                    stop = guard;
                    if *stop {
                        return;
                    }
                    // Check and touch as one step under the mutex, so a concurrent check
                    // never compares a stale timestamp with the heartbeat's fresh one.
                    let mut last = st.last_set.lock().unwrap();
                    #[cfg(test)]
                    let _ = seam::run(&path, seam::Point::BeatBeforeCheck);
                    if check_with(&path, &st, *last).is_ok() {
                        #[cfg(test)]
                        let _ = seam::run(&path, seam::Point::BeatBeforeTouch);
                        match set_dir_mtime(&path, SystemTime::now()) {
                            Ok(t) => *last = t,
                            Err(_) => st.compromised.store(true, Ordering::SeqCst),
                        }
                    }
                }
            })?;
            Ok(Self {
                path: spec.path.clone(),
                state,
                heartbeat: Some(heartbeat),
            })
        })();
        if result.is_err() {
            // `mkdir` already succeeded but the lock never actually started: remove it
            // now, rather than blocking CC for the whole staleness window.
            let _ = fs::remove_dir(&spec.path);
        }
        result
    }
```

with:

```rust
    /// Takes the directory `mkdir` made at `made_at` as this holder's: opens it, records its
    /// identity, touches it and starts the heartbeat. `None`, touching and removing nothing,
    /// when the directory may have been replaced before it was opened (`trusted_span`): the
    /// attempt then counts as contention, and the staleness rule settles whose it is.
    fn start(spec: &MkdirLockSpec, made_at: SystemTime) -> Result<Option<Self>, LockError> {
        // Until the directory is open and known to be the one `mkdir` made, it may already be
        // another holder's: a failure here leaves the path alone.
        #[cfg(test)]
        seam::run(&spec.path, seam::Point::AfterMkdir)?;
        let dir = File::open(&spec.path)?;
        let id = DirId::of(&dir.metadata()?);
        // A clock set back reads as no time spent; it makes the directory look younger
        // to every taker too.
        let spent = SystemTime::now()
            .duration_since(made_at)
            .unwrap_or_default();
        if spent >= trusted_span(spec.stale) {
            return Ok(None);
        }
        // Ours from here. A failure removes the directory rather than block CC for the whole
        // staleness window, but only while the path still names it (`remove_if_ours`).
        let set =
            touch(&dir, SystemTime::now()).inspect_err(|_| remove_if_ours(&spec.path, id, None))?;
        let state = Arc::new(State {
            dir,
            id,
            last_set: Mutex::new(set),
            compromised: AtomicBool::new(false),
            stop: Mutex::new(false),
            wake: Condvar::new(),
        });
        let abandon = |e: io::Error| {
            remove_if_ours(&spec.path, id, Some(set));
            LockError::from(e)
        };
        #[cfg(test)]
        seam::run(&spec.path, seam::Point::BeforeHeartbeat).map_err(abandon)?;
        let (st, path, every) = (state.clone(), spec.path.clone(), spec.touch_every);
        // `Builder::spawn` (not `thread::spawn`) so a failure to spawn is an error we
        // can clean up after, not a panic.
        let heartbeat = thread::Builder::new()
            .spawn(move || {
                let mut stop = st.stop.lock().unwrap();
                loop {
                    // The standard condvar predicate loop: `wait_timeout_while` checks `*stop`
                    // immediately, under the same lock `Drop` sets it under, before ever
                    // blocking. Without that check-before-wait, a `Drop` that sets `stop` and
                    // notifies before this thread reaches its first wait is lost entirely — the
                    // notify has nothing waiting to wake, and the plain `wait_timeout` used to
                    // block regardless for the full `every`, even though `*stop` was already
                    // true by the time it acquired the lock.
                    let (guard, _) = st.wake.wait_timeout_while(stop, every, |s| !*s).unwrap();
                    stop = guard;
                    if *stop {
                        return;
                    }
                    // Check and touch as one step under the mutex, so a concurrent check
                    // never compares a stale timestamp with the heartbeat's fresh one.
                    let mut last = st.last_set.lock().unwrap();
                    #[cfg(test)]
                    let _ = seam::run(&path, seam::Point::BeatBeforeCheck);
                    if check_with(&path, &st, *last).is_ok() {
                        #[cfg(test)]
                        let _ = seam::run(&path, seam::Point::BeatBeforeTouch);
                        // Through the held fd: a directory that replaced ours between the check
                        // and this touch is never touched (L356).
                        match touch(&st.dir, SystemTime::now()) {
                            Ok(t) => *last = t,
                            Err(_) => st.compromised.store(true, Ordering::SeqCst),
                        }
                    }
                }
            })
            .map_err(abandon)?;
        Ok(Some(Self {
            path: spec.path.clone(),
            state,
            heartbeat: Some(heartbeat),
        }))
    }
```

The old unconditional cleanup (`if result.is_err() { let _ = fs::remove_dir(&spec.path); }`) is
gone. Before ownership is established, a failure leaves the path alone, and so does the
tripped bound, whose `Ok(None)` drops the fd. After that, `remove_if_ours` removes the
directory only while it is still the holder's. `abandon` is a non-`move` closure capturing only
references, so it is `Copy`, and both of its uses take it by value.

Replace the doc comment of `check_owned`:

```rust
    /// Synchronous ownership check (§9.1): the directory must still carry the mtime this
    /// holder last set. A failure marks the guard compromised for good.
```

with

```rust
    /// Synchronous ownership check (§9.1): the path must still name the directory this holder
    /// created (the device and inode of the held fd) and that directory must still carry the
    /// mtime this holder last set. A failure marks the guard compromised for good.
```

In `check_with`, replace

```rust
    match fs::metadata(path).and_then(|m| m.modified()) {
        Ok(m) if m == last => Ok(()),
        _ => {
```

with

```rust
    // `symlink_metadata`: the path itself must be the directory, never a link to one.
    match fs::symlink_metadata(path) {
        Ok(m) if DirId::of(&m) == st.id && m.modified().ok() == Some(last) => Ok(()),
        _ => {
```

In `impl Drop for MkdirLock`, replace

```rust
        if self.check_owned().is_ok() {
            let _ = fs::remove_dir(&self.path);
        }
```

with

```rust
        // The check and the `rmdir` are two steps: a takeover between them needs this holder
        // to stall for the whole staleness window inside that gap, as with `proper-lockfile`.
        if self.check_owned().is_ok() {
            let _ = fs::remove_dir(&self.path);
        }
```

Run: `rg -n 'set_dir_mtime' crates/`
Expected: only the definition and calls inside `mkdir_lock.rs`'s `mod tests`.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p tagteam-provider --lib mkdir_lock::tests`
Expected: PASS, the eight new tests and every existing one. In particular:
- `a_suspended_holder_detects_the_takeover_and_leaves_the_new_lock` passes: a second
  `MkdirLock` takes the stale directory over, and the inode differs.
- `the_heartbeat_notices_an_external_takeover` passes: the mtime is changed in place, so the
  inode is the same and the mtime differs.
- `checks_racing_the_heartbeat_never_see_a_false_takeover` passes: the touch and its read-back
  go through the same inode the check stats.
- `a_stale_lock_is_taken_over` and the parent-directory tests pass: their spans are
  microseconds, far inside `trusted_span`.

Each new test fails against the half-fix it is meant to catch:
- An implementation that adds the inode check but keeps touch-by-path still fails
  `a_directory_replaced_between_the_beats_check_and_its_touch_is_never_touched` with `the beat
  touched the replacement`.
- One that holds the fd but has no `trusted_span` bound still fails
  `a_stall_between_mkdir_and_open_never_adopts_the_directory_found_after_it` with `the holder
  adopted the directory that replaced its own`.
- One that keeps an unconditional `remove_dir` for a failed open fails
  `a_failed_open_after_mkdir_never_removes_the_directory_found_there`.
- One whose post-ownership cleanup is not fenced fails
  `a_start_that_fails_after_a_takeover_leaves_the_replacement`.

Run: `cargo test -p tagteam-provider` and `cargo test -p tagteam-cc`
Expected: PASS, including `a_replaced_lock_directory_aborts_the_write_and_is_left_alone`.
CC's legacy-lock `try_acquire` treats a stalled attempt's `None` as legacy contention, which
`acquire_credentials` already handles.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. Some engine tests move a held CC lock's mtime by path to simulate a takeover,
setting it to `now + 60 s` (`tests/active.rs`, `tests/collect_active.rs`); they still pass,
because the directory and inode are the same and the mtime no longer matches. The kill tests
age leftover lock directories by path for a new process to take over, which is untouched.

- [ ] **Step 5: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings. The
`seam` module and its call sites exist only under `cfg(test)`, and no `set_dir_mtime` is left
outside the tests.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider/src/mkdir_lock.rs crates/tagteam-cc/tests/provider.rs
git commit -m "Hold Claude Code's lock directories by an fd from mkdir and check their inode"
```

---

### Task 4: `security` children run in their own process group

§14.1: "Non-interactive children (`/usr/bin/security`) run in their own process group, so a
Ctrl-C at the terminal reaches tagteam alone and never kills a Keychain write midway.
Interactive children (`security unlock-keychain`, and `claude` under `run`) stay in the
terminal's foreground group." Today every child inherits tagteam's process group. A Ctrl-C
during a switch's Keychain write therefore delivers SIGINT to `security` too, which dies with
the write half-done. That happens even though tagteam itself now only records the signal
(Task 6). §15.2 pins it: "`security` children run outside the terminal's process group."

**Readings of the spec this task commits to:**
- **"Non-interactive" means `Runner::run`**, which is `ProcessRunner::run_bounded`. Every
  Keychain read, probe, write, delete and lock check goes through it. `run_attached`, used only
  by `unlock` (Appendix A.3), is the one interactive child and is not changed.
- **`process_group(0)`** makes the child the leader of a new group. It is std's safe
  `CommandExt::process_group`, so there is no `pre_exec` and no `unsafe`. `setsid` is not used:
  `security` has no controlling terminal to drop, and a new session would also detach it from
  job control for no gain.
- **A background group can never stop the child.** Its stdin is a pipe or `/dev/null` and its
  output is piped, so it never reads the terminal (SIGTTIN) or writes to it (SIGTTOU).
- **The timeout still kills the child alone** (`Child::kill`, by pid). A tagteam killed by
  SIGKILL mid-write leaves `security` to finish on its own. The switch journal (§9.6) covers
  tagteam's side, as it already does.

**Files:**
- Modify: `crates/tagteam-provider/src/security.rs` (`Runner` docs, `ProcessRunner::run_bounded`,
  tests)

**Interfaces:**
- Consumes: nothing from Tasks 1–3.
- Produces: no new names. `ProcessRunner::run_bounded` (and so `Runner::run`) spawns with
  `std::os::unix::process::CommandExt::process_group(0)`. `ProcessRunner::run_attached` is
  unchanged.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-provider/src/security.rs`, inside `mod tests`, add `use std::fs;` between
`use std::collections::VecDeque;` and `use std::io;`. Then append these items at the end of `mod tests`:

```rust
    /// The pid of the shell `run` starts, and that pid's process group as the kernel reports it
    /// from outside while the shell is still running: the shell writes its pid to a file, then
    /// sleeps a second.
    fn pid_and_group(
        run: impl FnOnce(Vec<String>) -> RunResult + Send + 'static,
    ) -> (libc::pid_t, libc::pid_t) {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let script = format!("echo $$ > '{}'; sleep 1", pid_file.display());
        let shell = thread::spawn(move || run(s(&["-c", &script])));
        let deadline = Instant::now() + Duration::from_secs(5);
        let pid: libc::pid_t = loop {
            let written = fs::read_to_string(&pid_file).ok();
            if let Some(pid) = written.and_then(|t| t.trim().parse().ok()) {
                break pid;
            }
            assert!(Instant::now() < deadline, "the shell never wrote its pid");
            thread::sleep(Duration::from_millis(10));
        };
        // SAFETY: getpgid(2) only reads the process table, and `pid` is the shell, which is
        // still sleeping and not yet reaped, so the pid names it and no other process.
        let group = unsafe { libc::getpgid(pid) };
        assert!(group > 0, "getpgid: {}", io::Error::last_os_error());
        let ran = shell.join().unwrap();
        assert!(matches!(ran, RunResult::Exited { code: 0, .. }), "{ran:?}");
        (pid, group)
    }

    /// This test process's group: the one a terminal's Ctrl-C would reach.
    fn own_group() -> libc::pid_t {
        // SAFETY: getpgrp(2) takes no arguments and cannot fail.
        unsafe { libc::getpgrp() }
    }

    #[test]
    fn a_bounded_child_leads_a_process_group_of_its_own() {
        // §14.1: a terminal's Ctrl-C reaches its foreground process group. A `security` write
        // in that group would die midway; in a group of its own, it never sees the signal.
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (pid, group) = pid_and_group(|args| {
            ProcessRunner::run_bounded(
                "/bin/sh",
                &args,
                None,
                Duration::from_secs(5),
                Duration::from_secs(1),
            )
        });
        assert_eq!(group, pid, "the child leads its own group");
        assert_ne!(group, own_group(), "never the caller's group");
    }

    #[test]
    fn an_attached_child_stays_in_the_caller_s_process_group() {
        // §14.1: `security unlock-keychain` reads the password from the terminal, so it stays
        // in the terminal's foreground group, where a Ctrl-C reaches it along with tagteam.
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (pid, group) = pid_and_group(|args| ProcessRunner.run_attached("/bin/sh", &args));
        assert_ne!(group, pid);
        assert_eq!(group, own_group());
    }
```

The tests take `FORK_GUARD` as `process.rs`'s spawning test does. A child forked while a flock
test re-locks would otherwise hold a duplicate of that lock's file description.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-provider --lib security::tests`
Expected: FAIL, one test.
- `a_bounded_child_leads_a_process_group_of_its_own` fails on "the child leads its own group":
  the child inherits the test process's group, so `group` equals `own_group()`, not `pid`.
- `an_attached_child_stays_in_the_caller_s_process_group` passes now and must keep passing. It
  guards the interactive child against the change.

- [ ] **Step 3: Spawn non-interactive children in a group of their own**

In `crates/tagteam-provider/src/security.rs`, replace:

```rust
use std::io::Write as _;
use std::path::PathBuf;
```

with:

```rust
use std::io::Write as _;
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
```

Replace the `Runner` trait:

```rust
pub trait Runner: Send + Sync {
    fn run(
```

with:

```rust
pub trait Runner: Send + Sync {
    /// Runs without the terminal: stdin piped (`stdin` given) or null, stdout and stderr
    /// captured, killed after `timeout`. The child leads a process group of its own (§14.1),
    /// so a Ctrl-C at the terminal never reaches it and never cuts a Keychain write short.
    fn run(
```

and, in the same trait, replace:

```rust
    /// Runs with the terminal attached: stdin and stderr inherited, the child's stdout sent to
    /// stderr (stdout is reserved for command output, §14). No timeout, because a person is
    /// answering. `Exited` carries no output.
    fn run_attached(&self, program: &str, args: &[String]) -> RunResult;
```

with:

```rust
    /// Runs with the terminal attached: stdin and stderr inherited, the child's stdout sent to
    /// stderr (stdout is reserved for command output, §14). No timeout, because a person is
    /// answering. `Exited` carries no output. The child stays in tagteam's process group, the
    /// terminal's foreground one, so a Ctrl-C there reaches it too (§14.1).
    fn run_attached(&self, program: &str, args: &[String]) -> RunResult;
```

In `ProcessRunner::run_bounded`, replace:

```rust
        let mut child = match Command::new(program)
            .args(args)
            .stdin(if stdin.is_some() {
```

with:

```rust
        let mut child = match Command::new(program)
            .args(args)
            // §14.1: a group of its own, so a Ctrl-C at the terminal reaches tagteam alone and
            // never kills a Keychain write midway. Its stdin is a pipe or null and its output is
            // piped, so the background group never stops it for touching the terminal.
            .process_group(0)
            .stdin(if stdin.is_some() {
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-provider --lib security::tests`
Expected: PASS, including both new tests.

Run: `cargo test -p tagteam-provider`
Expected: PASS.

On macOS only, run: `cargo test -p tagteam-provider --features real_keychain --test real_keychain`
Expected: PASS. The real `/usr/bin/security` runs in its own group against a temporary keychain,
and reads, writes and deletes as before.

- [ ] **Step 5: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: `fmt --check` prints nothing, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider/src/security.rs
git commit -m "Run non-interactive security children in their own process group"
```

---

### Task 5: Collector cancellation points

§14.1 lists "before reserving or sending a usage request" among the cancellation points, and
adds: "An interrupted usage fetch is not a usage failure: it records nothing and gives its slot
back (§8.3)." It also names the refresh gate (§7.3) and active-token refresh (§7.5), "from
sending the token request to persisting the successor", as critical spans. The contract
(Task 5): "`collect_usage` returns `Err(EngineError::Interrupted(n))` when the token was set
during the collection, after every thread has joined and given back any slot it held unsent.
An interrupted account records nothing."

The collector does not look at the token yet. Without this task, a `list` that a Ctrl-C hits
can go wrong two ways:
- An account whose lock wait Task 2 interrupted is recorded as `refresh-failed`, with a
  backoff.
- The interruption surfaces as `Lock(Interrupted)` from one thread, while the other threads
  still send.

**Readings of the spec this task commits to:**
- **Cancellation points in the collector:**
  - before reserving (no lease, no slot);
  - at the top of every `send`, so before the store's authorization of the first request and
    of the 401 retry;
  - before starting a gate refresh or a §7.5 refresh for the fetch. A refresh started for a
    usage request is part of sending it, stopping there loses nothing, and a Ctrl-C never
    waits on a token request it did not need.

  §7.5's own lock waits (the mutation lock, Claude Code's credential locks) and the mutation
  lock that `live_bytes` waits for return the interruption through Task 2. `Stop::from` turns
  that into the fetch's interruption, never `refresh-failed` or `keychain-unavailable`.
- **Not cancellation points:**
  - inside the gate, from its try-lock to its successor's persistence;
  - inside §7.5;
  - a request already sent: its result is recorded as usual;
  - phase 3 itself.
- **An interruption records nothing:** no failure count, no backoff, no `last_attempt_at`, no
  plan change. The slot held unsent goes back (`release_slot`, best effort, like `Stop::Error`).
  The lease is left to expire (90 s), as §8.3 leaves it after any record. What a request already
  sent wrote stays: its slot stays counted, and a 401's `rejected_fp` stamp stays.
- **`collect_usage` reports the interruption whenever the token is set once every thread has
  joined**, even if every account finished first. The end of a collection is its caller's next
  cancellation point (`list`, `status`, a usage strategy's switch), and the readings recorded
  stay recorded. `n` is the token's value at the join.

**Files:**
- Modify: `crates/tagteam-engine/src/collect.rs`
- Modify: `crates/tagteam-engine/tests/collect.rs`

**Interfaces:**
- Consumes:
  - Task 1: `tagteam_provider::Cancel::{new, request, requested}`, `Env.cancel` (clones share
    the cell).
  - Task 2: `Engine::cancel(&self) -> &Cancel`, `EngineError::Interrupted(i32)`,
    `EngineError::signal(&self) -> Option<i32>` (`Some` for `Interrupted`,
    `Lock(LockError::Interrupted { .. })` and `Provider(ProviderError::Lock(LockError::Interrupted { .. }))`).
  - Decision 3 via Task 1: `MutationGuard::acquire` checks the token before its first attempt.
  - Existing fixtures: `Fx`, `two_accounts`, `due`, `usage_fixture`, `usage_requests`,
    `usage_bearers`, `methods`, `quarantine_of`, `Fx::{script_usage, script_refresh,
    vault_refresh_token, usage_state, engine_with_env}`, `Engine::on_point`, and the test file's
    own `state` and `collect_on`.
- Produces:
  - `Engine::collect_usage(&self, mode: CollectMode) -> Result<CollectReport, EngineError>`
    keeps its signature and returns `Err(EngineError::Interrupted(n))` as above.
  - Private to `collect.rs`: `Stop::Interrupted(i32)`, `Collection::interruption(&self)`, and
    `Collection::refresh_error(&mut self, e: EngineError) -> Stop`.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/collect.rs`, replace:

```rust
use tagteam_engine::Engine;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::collect::{CollectMode, Collected};
use tagteam_engine::store::{Ineligible, UsageStateRow};
use tagteam_engine::vault::SERVICE;
```

with:

```rust
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::collect::{CollectMode, CollectReport, Collected};
use tagteam_engine::store::{Ineligible, UsageStateRow};
use tagteam_engine::vault::SERVICE;
use tagteam_engine::{Engine, EngineError};
```

and replace:

```rust
use tagteam_provider::{
    Http, HttpError, HttpRequest, HttpResponse, Keychain, Method, Provider, ScriptedHttp,
};
```

with:

```rust
use tagteam_provider::{
    Cancel, Http, HttpError, HttpRequest, HttpResponse, Keychain, Method, Provider, ScriptedHttp,
};
```

Below `fn collect_on`, add:

```rust
/// `id`'s on-demand collection through the fixture's engine, error and all.
fn collect_result(fx: &Fx, id: &AccountId) -> Result<CollectReport, EngineError> {
    fx.engine.collect_usage(CollectMode::OnDemand {
        accounts: vec![id.clone()],
    })
}

/// §14.1: the collection ended as SIGINT's interruption.
fn assert_interrupted(result: Result<CollectReport, EngineError>) {
    match result {
        Err(EngineError::Interrupted(signal)) => assert_eq!(signal, libc::SIGINT),
        other => panic!("expected the collection to be interrupted, got {other:?}"),
    }
}
```

After `an_inactive_account_is_fetched_with_its_stored_token_and_recorded`, add:

```rust
#[test]
fn a_collection_started_after_a_signal_reserves_and_sends_nothing() {
    // §14.1: the first cancellation point comes before reserving.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.script_usage(200, usage_fixture());
    fx.engine.cancel().request(libc::SIGINT);

    assert_interrupted(collect_result(&fx, &a));

    assert!(fx.http.requests().is_empty(), "nothing is sent");
    assert_eq!(fx.usage_state(&a), None, "nothing is recorded");
    assert_eq!(usage_requests(&fx), 0, "no slot is taken");
    // No lease either: a process that got no signal collects the account at once.
    let mut env = fx.env.clone();
    env.cancel = Cancel::new();
    let other = fx.engine_with_env(env);
    assert_eq!(collect_on(&other, &a), [(a.clone(), Collected::Recorded)]);
}
```

Inside `mod hooks` (the `#[cfg(feature = "test-hooks")]` module at the end of the file), add
this helper and these tests:

```rust
    /// The fixture's cancel token, set from inside the named hook as a signal handler would set
    /// it while the work passes that point.
    fn signal_at(fx: &Fx, point: &'static str) {
        let cancel = fx.engine.cancel().clone();
        fx.engine
            .on_point(point, Box::new(move || cancel.request(libc::SIGINT)));
    }

    #[test]
    fn a_signal_after_reserving_gives_the_slot_back_and_records_nothing() {
        // §14.1, §8.3: an interrupted fetch is not a usage failure.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.script_usage(200, usage_fixture());
        fx.collect(&[&a]);
        let before = state(&fx, &a);
        fx.http.clear();
        fx.script_usage(200, usage_fixture());
        // Past the plan (at most 660 s) and the 180 s floor: due again.
        fx.clock.advance_ms(700_000);
        signal_at(&fx, "usage-reserved");

        assert_interrupted(collect_result(&fx, &a));

        assert!(
            fx.http.requests().is_empty(),
            "nothing is sent once the token is set"
        );
        assert_eq!(
            state(&fx, &a),
            before,
            "no failure, no backoff, no attempt: nothing is recorded"
        );
        assert_eq!(
            usage_requests(&fx),
            1,
            "only the first collection's request: the reserved slot went back"
        );
    }

    #[test]
    fn a_gate_refresh_in_flight_when_the_signal_lands_still_persists_its_successor() {
        // §14.1: the gate, from sending the token request to persisting the successor, is a
        // critical span. The usage request after it is a cancellation point.
        let fx = Fx::new();
        let a = due(&fx);
        fx.script_refresh(Some("rt-a2"));
        fx.script_usage(200, usage_fixture());
        signal_at(&fx, "gate-after-response");

        assert_interrupted(collect_result(&fx, &a));

        assert_eq!(
            methods(&fx),
            [Method::Post],
            "the token request only, no usage request"
        );
        assert_eq!(
            fx.vault_refresh_token(&a).as_deref(),
            Some("rt-a2"),
            "the successor is in the vault"
        );
        assert_eq!(quarantine_of(&fx, &a), (None, None));
        assert_eq!(fx.usage_state(&a), None, "nothing is recorded");
        assert_eq!(usage_requests(&fx), 0, "the slot went back");
    }

    #[test]
    fn a_request_already_sent_is_recorded_though_the_collection_reports_the_interruption() {
        // §14.1: a request sent is no cancellation point; its result is recorded as usual.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.script_usage(200, usage_fixture());
        signal_at(&fx, "usage-before-record");

        assert_interrupted(collect_result(&fx, &a));

        let s = state(&fx, &a);
        assert_eq!(
            (s.fetched_at, s.consecutive_failures),
            (Some(NOW_S), 0),
            "the reading is recorded"
        );
        assert_eq!(s.last_good, Some(normalize(&usage_fixture()).unwrap()));
        assert_eq!(usage_requests(&fx), 1, "the request sent keeps its slot");
    }

    #[test]
    fn a_signal_while_the_live_account_waits_for_the_mutation_lock_records_nothing() {
        // §14.1: the mutation lock's wait (Task 2) is a cancellation point. Reading the live
        // token stops there; it is no `keychain-unavailable`, and no `Moved`.
        let fx = Fx::new();
        fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        fx.script_usage(200, usage_fixture());
        signal_at(&fx, "before-mutation-lock");

        assert_interrupted(collect_result(&fx, &b));

        assert!(fx.http.requests().is_empty());
        assert_eq!(fx.usage_state(&b), None, "nothing is recorded");
        assert_eq!(usage_requests(&fx), 0, "the slot went back");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test collect`
Expected: FAIL, five tests, because nothing in the collector looks at the token yet:
- `a_collection_started_after_a_signal_reserves_and_sends_nothing`,
  `a_signal_after_reserving_gives_the_slot_back_and_records_nothing`,
  `a_gate_refresh_in_flight_when_the_signal_lands_still_persists_its_successor` and
  `a_request_already_sent_is_recorded_though_the_collection_reports_the_interruption` panic
  with "expected the collection to be interrupted, got Ok(CollectReport { outcomes: [(…,
  Recorded)], … })". Each collection sent its request and recorded it.
- `a_signal_while_the_live_account_waits_for_the_mutation_lock_records_nothing` panics with
  "… got Ok(CollectReport { outcomes: [(…, Failed { kind: "error" })], warnings: ["usage for
  b@x.co (position 2) was not collected: interrupted while waiting for the lock
  …/.mutation.lock"] })". Task 2's lock wait already stops, but the collector treats that
  account's error like any other (M2b: one account's error becomes its `Failed { kind:
  "error" }` outcome and a warning), not as the collection's interruption.

The other tests in the file pass.

- [ ] **Step 3: Check the token before reserving and after joining**

In `crates/tagteam-engine/src/collect.rs`, replace:

```rust
    /// §8.3 on demand: every listed account on its own thread, and the call waits for them
    /// all. A usage failure is never an error here: it is recorded, and reported in the
    /// report's outcomes and warnings. So is an error that ends one account's collection (the
    /// store failing under it): every thread is joined and kept, and that account's outcome is
    /// `Failed { kind: "error" }` with one warning naming it, so one account never costs the
    /// others' outcomes. `Err` only for an error before any thread starts (opening the store,
    /// reading the listed accounts, reading each provider's recorded active account). IDs
    /// that name no account are skipped. Never creates the store.
```

with:

```rust
    /// §8.3 on demand: every listed account on its own thread, and the call waits for them
    /// all. A usage failure is never an error here: it is recorded, and reported in the
    /// report's outcomes and warnings. So is an error that ends one account's collection (the
    /// store failing under it): every thread is joined and kept, and that account's outcome is
    /// `Failed { kind: "error" }` with one warning naming it, so one account never costs the
    /// others' outcomes. `Err` only for an error before any thread starts (opening the store,
    /// reading the listed accounts, reading each provider's recorded active account), or for
    /// §14.1's cancel token set during the collection: `Interrupted`, once every thread has
    /// joined and given back the slot it held unsent. IDs that name no account are skipped.
    /// Never creates the store.
```

Replace:

```rust
        let mut report = CollectReport::default();
        for (row, result) in rows.iter().zip(results) {
```

with:

```rust
        // §14.1: a collection the token was set during is the command's interruption, whatever
        // each account did. A request already sent was recorded as usual; nothing else was.
        if let Some(signal) = self.cancel().requested() {
            return Err(EngineError::Interrupted(signal));
        }
        let mut report = CollectReport::default();
        for (row, result) in rows.iter().zip(results) {
```

In `collect_one`, replace:

```rust
        let budget = p.poll_budget();
        // Phase 1: eligibility, the lease and the slot, in one transaction.
```

with:

```rust
        let budget = p.poll_budget();
        // §14.1's first cancellation point: once the token is set, nothing is reserved.
        if let Some(signal) = self.cancel().requested() {
            return Err(EngineError::Interrupted(signal));
        }
        // Phase 1: eligibility, the lease and the slot, in one transaction.
```

- [ ] **Step 4: Stop a fetch on the token without recording it**

In `crates/tagteam-engine/src/collect.rs`, replace:

```rust
    /// The store or a test hook failed: returned, never recorded. The unsent slot goes back,
    /// best effort, so a lasting fault does not spend the hourly budget (§8.6); `collect_usage`
    /// turns it into a warning and a `Failed { kind: "error" }` outcome.
    Error(EngineError),
}

impl From<EngineError> for Stop {
    fn from(e: EngineError) -> Self {
        Stop::Error(e)
    }
}
```

with:

```rust
    /// The store or a test hook failed: returned, never recorded. The unsent slot goes back,
    /// best effort, so a lasting fault does not spend the hourly budget (§8.6); `collect_usage`
    /// turns it into a warning and a `Failed { kind: "error" }` outcome.
    Error(EngineError),
    /// §14.1: the cancel token was set at a cancellation point, or a lock wait on the way met
    /// it. Not a usage failure: nothing is recorded (no failure count, no backoff), and the
    /// unsent slot goes back. The lease is left to expire, as after any record. `collect_usage`
    /// reports the whole collection as interrupted, never this account as failed.
    Interrupted(i32),
}

/// An error that carries a signal is the fetch's interruption; any other is an error.
impl From<EngineError> for Stop {
    fn from(e: EngineError) -> Self {
        match e.signal() {
            Some(signal) => Stop::Interrupted(signal),
            None => Stop::Error(e),
        }
    }
}
```

In `impl Collection<'_>`, replace:

```rust
    fn now_s(&self) -> i64 {
        self.now_ms().div_euclid(1000)
    }
```

with:

```rust
    fn now_s(&self) -> i64 {
        self.now_ms().div_euclid(1000)
    }

    /// §14.1's cancellation point before a usage request, or before a refresh started for one.
    fn interruption(&self) -> Result<(), Stop> {
        match self.engine.cancel().requested() {
            Some(signal) => Err(Stop::Interrupted(signal)),
            None => Ok(()),
        }
    }

    /// A refresh's error as the fetch's stop. A lock wait inside the refresh that met the token
    /// is the fetch's interruption; any other error is a failure, with a warning.
    fn refresh_error(&mut self, e: EngineError) -> Stop {
        match e.signal() {
            Some(signal) => Stop::Interrupted(signal),
            None => {
                self.warn_refresh(&e);
                failed("refresh-failed")
            }
        }
    }
```

In `refresh_live`'s doc comment, replace:

```rust
    /// carry-over), except a live credential the oracle gives to another identity, which has a
    /// status of its own. A kind that does not refresh never reaches §7.5, which would refuse
    /// it: its expired token is `token-expired`, and its refused one `http-401`, as on the
    /// inactive path (Decision 11: a refusal is an ordinary failure).
    fn refresh_live(&mut self, trigger: ActiveTrigger) -> Result<Vec<u8>, Stop> {
        if !self.p.kind_traits(&self.row.kind).refreshable {
```

with:

```rust
    /// carry-over), except a live credential the oracle gives to another identity, which has a
    /// status of its own, and §14.1's interruption: no refresh starts once the token is set,
    /// and a lock wait inside §7.5 that meets it stops the fetch without a record. A kind that
    /// does not refresh never reaches §7.5, which would refuse it: its expired token is
    /// `token-expired`, and its refused one `http-401`, as on the inactive path (Decision 11:
    /// a refusal is an ordinary failure).
    fn refresh_live(&mut self, trigger: ActiveTrigger) -> Result<Vec<u8>, Stop> {
        self.interruption()?;
        if !self.p.kind_traits(&self.row.kind).refreshable {
```

and, at the end of the same function, replace:

```rust
            Err(EngineError::ForeignLiveCredential { .. }) => Err(failed("foreign-credential")),
            Err(e) => {
                self.warn_refresh(&e);
                Err(failed("refresh-failed"))
            }
        }
    }
```

with:

```rust
            Err(EngineError::ForeignLiveCredential { .. }) => Err(failed("foreign-credential")),
            Err(e) => Err(self.refresh_error(e)),
        }
    }
```

Replace the head of `gate`:

```rust
    /// §7.3 for an inactive account, `snapshot` being the bytes the collector decided on. A
    /// Dead verdict (the gate has quarantined the account) and every deterministic refusal end
    /// the fetch before anything is sent (§8.1).
    fn gate(&mut self, snapshot: &[u8]) -> Result<Vec<u8>, Stop> {
        hooks::point(self.engine, "usage-before-gate")?;
        match self.engine.refresh_stored(self.p, &self.row.id, snapshot) {
```

with:

```rust
    /// §7.3 for an inactive account, `snapshot` being the bytes the collector decided on. A
    /// Dead verdict (the gate has quarantined the account) and every deterministic refusal end
    /// the fetch before anything is sent (§8.1). No refresh starts once the cancel token is set
    /// (§14.1); one already started runs to its end, its successor persisted.
    fn gate(&mut self, snapshot: &[u8]) -> Result<Vec<u8>, Stop> {
        hooks::point(self.engine, "usage-before-gate")?;
        self.interruption()?;
        match self.engine.refresh_stored(self.p, &self.row.id, snapshot) {
```

and its last arm:

```rust
                | GateOutcome::Transient { .. },
            ) => Err(failed("refresh-failed")),
            Err(e) => {
                self.warn_refresh(&e);
                Err(failed("refresh-failed"))
            }
        }
    }
```

with:

```rust
                | GateOutcome::Transient { .. },
            ) => Err(failed("refresh-failed")),
            Err(e) => Err(self.refresh_error(e)),
        }
    }
```

In `send`, replace:

```rust
    fn send(&mut self, bytes: &[u8]) -> Result<UsageResult, Stop> {
        if !self.usable(bytes) {
```

with:

```rust
    fn send(&mut self, bytes: &[u8]) -> Result<UsageResult, Stop> {
        // §14.1: every request, the 401 retry included, is preceded by a cancellation point.
        self.interruption()?;
        if !self.usable(bytes) {
```

In `record_to_store`, replace its last arm:

```rust
            Err(Stop::Error(e)) => return Err(e),
        })
```

with:

```rust
            // §14.1: no record at all. As after an error, `record` gives the slot of the
            // request that never left back, best effort: a release that fails never masks the
            // interruption.
            Err(Stop::Interrupted(signal)) => return Err(EngineError::Interrupted(signal)),
            Err(Stop::Error(e)) => return Err(e),
        })
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --features test-hooks --test collect`
Expected: PASS, including the five new tests.

Run: `cargo test -p tagteam-engine --test collect`
Expected: PASS. `a_collection_started_after_a_signal_reserves_and_sends_nothing` runs without
the feature, and the `hooks` module is compiled out.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS. No test sets the token outside these five, so every other collector test
(`collect_active.rs`, `views_usage.rs`, the switch tests that collect) is unchanged.

- [ ] **Step 6: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: `fmt --check` prints nothing, and both clippy runs finish with no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/tagteam-engine/src/collect.rs crates/tagteam-engine/tests/collect.rs
git commit -m "Stop usage collection at its cancellation points without recording"
```

---

### Task 6: Signal handlers, prompts, exit codes and the late-signal notice

§14.1: "SIGINT, SIGTERM and SIGHUP never stop a command at the instruction they arrive on. The
CLI's handler only records the signal in the engine's cancel token." The section has three
more rules:
- **Prompts:** "A Ctrl-C at a prompt, including a no-echo secret prompt, restores the terminal
  and counts as an interruption."
- **Exit:** "An interrupted command exits 130 after SIGINT, and 128 + the signal number after
  SIGTERM or SIGHUP. With `--json`, it prints the error envelope with type `interrupted`."
- **A command that reaches no cancellation point** "reports what it did, with its normal output
  and exit code, plus a stderr notice that the signal came too late to stop it. A switch that
  committed is reported as switched, never as interrupted."

Tasks 1–5 put the token everywhere the work can stop. This task adds the handler that sets the
token, the prompts' share of it, and the CLI's reporting. It pins Review Focus 1 and 2 with real
signals sent to the binary, and Focus 3 in-process and on a pseudo-terminal.

**Readings of the spec this task commits to:**
- **The handlers are installed at the process boundary** (`main_with_args`), on the
  `Context`'s `env.cancel`, after parsing and after `statusline`'s stdin drain.
  - In-process tests drive `app::run` with their own tokens. They must never change the test
    runner's signal dispositions.
  - A status bar command stuck on a pipe that never closes still dies on SIGTERM.
  - A failed install is a stderr warning, and the command runs with the default dispositions,
    which is M2's behaviour.
- **One way to report an interruption, whichever carrier holds the signal** (Decision 4):
  - exit `128 + n`;
  - human output `tagteam: interrupted`;
  - `--json` output `{"schemaVersion":1,"error":{"type":"interrupted","message":"interrupted"}}`.

  `n` is the signal the cancellation point read. It is carried by the error, not re-read from
  the token, which a later signal may have overwritten.
- **The late notice** (Decision 6) is `tagteam: interrupted too late to stop: <command> had
  already finished`, on stderr.
  - It is printed after everything else whenever the token is set and the command did not end
    as an interruption. That includes a command that failed for a reason of its own: it keeps
    its error and its exit code.
  - `<command>` is the subcommand in canonical form (`ls` is `list`, a bare `tagteam` is
    `list`).
  - Under `--json`, stdout is unchanged.
- **Every prompt is a cancellation point** (Decision 5):
  - `TtyPrompter` declines without asking when the token is already set.
  - It waits for a line in 100 ms `poll(2)` slices. Interrupted, it discards what was typed,
    ends the prompt's line and declines.
  - **No read ever blocks** (Codex plan review round 2, P2).
    - The problem: `poll` can report a whole line that a Ctrl-C (`ISIG`) then flushes before
      `read` runs. A blocking read would then wait for input that is gone, and `SA_RESTART`
      would restart it after every further signal.
    - Every prompt therefore reads a fresh `/dev/tty` description opened with `O_NONBLOCK`. A
      flushed line makes the read report `WouldBlock`, and the loop goes back to the token.
      Partial input is kept across reads until a newline or the end of input.
    - The description is tagteam's own, never fd 0's (which the shell shares, as a `dup` would
      too), so no flag on anything shared is changed or needs restoring.
    - `interactive()` still gates on stdin and stderr; the answer comes from the controlling
      terminal, as the secret prompt's always did.
  - The CLI checks the token after every prompt, whatever the answer: a yes typed just before
    the signal goes no further.
  - The unlock question has a second check after `security unlock-keychain`. macOS's password
    prompt is a prompt too, and the attached child shares tagteam's group (Task 4), so a Ctrl-C
    there ends both.
- **The secret prompt is tagteam's own, and sliced like the others.** rpassword's read could
  not be cut short: it blocks on `/dev/tty`, and under `SA_RESTART` a SIGTERM, a SIGHUP or a
  SIGINT sent from elsewhere set the token while the read went on, with echo still off, until
  someone typed. `read_secret` replaces it:
  - It opens `/dev/tty` as rpassword did (non-blocking, as above), and writes the same
    question (`Token: `).
  - It turns `ECHO` (and `ECHONL`) off and nothing else. `ICANON` stays on, so the terminal
    edits the line and hands it over whole. `ISIG` stays on, so Ctrl-C arrives as SIGINT,
    which sets the token, and is never a byte of the secret.
  - It reads exactly as the line prompts do (`read_line_between`): the same slices, the same
    non-blocking reads, and the same discard (`tcflush`) when interrupted.
  - A `Drop` guard puts the saved settings back on every path.

  `rpassword` leaves the workspace. A line must fit the terminal's canonical limit (1024 bytes
  on macOS), far above any token; `add-token -` takes a longer one from a pipe.
- **`add-token -` reads a pipe, not a prompt.** It is not a cancellation point; the engine's
  first lock wait is.
- **An interrupted collection interrupts `list` and `status`.** `App::collect` passes that error
  up. Any other collection error stays a stderr warning (§8.3: a usage failure is never a
  command error).

**Files:**
- Modify: `Cargo.toml` (drop `rpassword`, Part A; add `signal-hook`, Part C)
- Modify: `crates/tagteam/Cargo.toml` (the same)
- Modify: `Cargo.lock`
- Create: `crates/tagteam/src/signals.rs`
- Modify: `crates/tagteam/src/lib.rs`
- Modify: `crates/tagteam/src/prompt.rs`
- Modify: `crates/tagteam/src/app.rs`
- Modify: `crates/tagteam-engine/src/hooks.rs` (a test-only pause point; see NOTES 1)
- Modify: `crates/tagteam/tests/app.rs`
- Create: `crates/tagteam/tests/signals.rs`

**Interfaces:**
- Consumes:
  - Task 1: `tagteam_provider::Cancel::{new, request, requested, cell}`, `Env.cancel`.
  - Task 2: `EngineError::Interrupted(i32)`, `EngineError::signal()`, `EngineError::kind()`
    (`"interrupted"` for every carrier), `Engine::cancel()`, and Decision 3's first-attempt
    check in every lock wait.
  - Task 4: `run_attached` stays in the foreground group.
  - Task 5: `collect_usage`'s `Err(EngineError::Interrupted(n))`.
  - Existing fixtures: `tests/common/mod.rs`'s `std_cmd`, `two_fresh_accounts`, `live_email`,
    `OFFLINE_API_BASE`; `tests/app.rs`'s `H` (with `H::run_in`), `Scripted`, `with_one_login`,
    `with_unmanaged_login`, `ADD_STRANGER_FIRST`, `REPLACE_A`, `LIVE_A_OFFLINE`, `UNLOCK`.
- Produces:
  - `crates/tagteam/src/signals.rs`:
    `pub fn install(cancel: &Cancel) -> std::io::Result<()>` (module private to the crate,
    `mod signals`).
  - `crates/tagteam/src/prompt.rs`: `impl TtyPrompter { pub fn new(cancel: Cancel) -> Self }`.
    Private to the module:
    - `fn open_terminal(path: &Path) -> io::Result<File>` (`O_NONBLOCK | O_NOCTTY`);
    - `fn wait_for_input(input: BorrowedFd<'_>, cancel: &Cancel) -> bool`;
    - `fn discard_input(tty: BorrowedFd<'_>)`;
    - `enum LineRead { Line(String), End, Interrupted }`, with a `Debug` that never shows the
      line;
    - `fn read_line(tty: &File, cancel: &Cancel) -> io::Result<LineRead>` and
      `fn read_line_between(tty: &File, cancel: &Cancel, between: impl FnMut()) -> io::Result<LineRead>`;
    - `fn answer_of(read: io::Result<LineRead>) -> Option<String>`;
    - `struct EchoOff<'a>`, which restores the saved `termios` in `Drop`;
    - `fn termios(tty: BorrowedFd<'_>) -> io::Result<libc::termios>`;
    - `fn read_secret(tty: &File, cancel: &Cancel) -> io::Result<LineRead>` and
      `read_secret_between`, with the same seam.

    `read_answer` goes: no prompt reads stdin any more.
  - `crates/tagteam/src/app.rs`: `pub(crate) const KIND_INTERRUPTED: &str = "interrupted";`,
    plus the private `enum Ended { Code(i32), Interrupted(i32) }`,
    `fn run_command(cli: Cli, ctx: Context, io: &mut Io<'_>) -> Ended`,
    `fn command_name(command: &Command) -> &'static str` and
    `App::after_prompt(&self) -> Result<(), Failure>`. `pub fn run` keeps its signature.
  - `crates/tagteam-engine/src/hooks.rs` (feature `test-hooks`): `TAGTEAM_TEST_PAUSE_AT` and
    `TAGTEAM_TEST_PAUSE_DIR`.
  - Test helpers in `crates/tagteam/tests/app.rs`: `H::run_with_cancel(&self, args: &[&str],
    prompter: &mut Scripted, cancel: &Cancel) -> (i32, String, String)` (built on the existing
    `H::run_in(args, prompter, adjust: impl FnOnce(&mut Context))`) and
    `Scripted::interrupted_by(cancel: &Cancel, signal: i32, a: &[&'static str]) -> Scripted`.

#### Part A: prompts, the secret included, that a signal can end

- [ ] **Step 1: Write the failing prompt tests**

In `crates/tagteam/src/prompt.rs`, replace the whole of `mod tests`, from `#[cfg(test)]` to the
end of the file, with the module below.
- The three existing tests keep their cases. They now feed `answer_of` what a read returns
  (`LineRead`), instead of feeding `read_answer` a reader, because every prompt now reads the
  terminal non-blocking, through `read_line`.
- The pseudo-terminal tests open the pty's slave by its path with `open_terminal`, exactly as
  a prompt opens `/dev/tty`: their own file description, non-blocking.
- `input_flushed_between_readiness_and_the_read_never_blocks_either_read` is the deterministic
  test of the window between `poll` and `read`. Through `read_line_between`'s seam it empties
  the terminal's input after the wait has seen a whole line and before the read, as a Ctrl-C
  (`ISIG`) does, and sets the token. A read that blocked would never return; `within_3s` turns
  that into a failure, never a hung test.

```rust
#[cfg(test)]
mod tests {
    use std::ffi::{CStr, OsStr};
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;

    /// A line prompt's answer to `typed`.
    fn line(typed: &str) -> Option<String> {
        answer_of(Ok(LineRead::Line(typed.to_owned())))
    }

    #[test]
    fn ctrl_d_at_a_yes_by_default_prompt_declines() {
        // Ctrl-D on an empty line: end of input, with nothing read.
        assert_eq!(answer_of(Ok(LineRead::End)), None);
        assert!(!confirmed(answer_of(Ok(LineRead::End)), true));
        // Enter is an empty line, and takes the default.
        assert!(confirmed(line(""), true));
        assert!(!confirmed(line(""), false));
        assert!(confirmed(line(" Yes"), false));
        assert!(!confirmed(line("n"), true));
    }

    #[test]
    fn a_read_error_or_an_interruption_declines() {
        let broken = answer_of(Err(io::Error::other("the terminal went away")));
        assert_eq!(broken, None);
        assert!(!confirmed(broken, true));
        assert!(!confirmed(answer_of(Ok(LineRead::Interrupted)), true));
    }

    #[test]
    fn a_choice_is_one_based_and_in_range() {
        assert_eq!(chosen(line("2"), 2), Some(1));
        assert_eq!(chosen(line("3"), 2), None);
        assert_eq!(chosen(line("0"), 2), None);
        assert_eq!(chosen(answer_of(Ok(LineRead::End)), 2), None);
    }

    /// Sets `cancel` to `signal` from another thread after `after`, as the handler would.
    fn set_after(cancel: &Cancel, after: Duration, signal: i32) -> thread::JoinHandle<()> {
        let cancel = cancel.clone();
        thread::spawn(move || {
            thread::sleep(after);
            cancel.request(signal);
        })
    }

    #[test]
    fn a_signal_ends_a_wait_for_input_that_never_comes_within_a_slice() {
        // Decision 5: the handler restarts syscalls, so only the slices end the wait.
        let (quiet, _writer) = UnixStream::pair().unwrap();
        let cancel = Cancel::new();
        let started = Instant::now();
        let setter = set_after(&cancel, Duration::from_millis(150), libc::SIGINT);
        assert!(
            !wait_for_input(quiet.as_fd(), &cancel),
            "interrupted, not ready"
        );
        let waited = started.elapsed();
        setter.join().unwrap();
        assert!(waited >= Duration::from_millis(150), "{waited:?}");
        assert!(
            waited < Duration::from_millis(600),
            "about one slice after the signal: {waited:?}"
        );
    }

    #[test]
    fn a_token_already_set_never_waits() {
        let (quiet, _writer) = UnixStream::pair().unwrap();
        let cancel = Cancel::new();
        cancel.request(libc::SIGTERM);
        let started = Instant::now();
        assert!(!wait_for_input(quiet.as_fd(), &cancel));
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn input_ready_or_closed_ends_the_wait_and_the_read_decides() {
        let (ready, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"y\n").unwrap();
        assert!(wait_for_input(ready.as_fd(), &Cancel::new()));
        let (closed, writer) = UnixStream::pair().unwrap();
        drop(writer);
        assert!(
            wait_for_input(closed.as_fd(), &Cancel::new()),
            "end of input: the read sees it and declines"
        );
    }

    #[test]
    fn every_prompt_declines_without_asking_once_the_token_is_set() {
        let cancel = Cancel::new();
        cancel.request(libc::SIGHUP);
        let mut p = TtyPrompter::new(cancel);
        assert!(
            !p.confirm("Replace it?", true),
            "a decline, never the default yes"
        );
        assert_eq!(p.choose("Which account?", &["a".into(), "b".into()]), None);
        assert_eq!(
            p.secret("Token: "),
            None,
            "the terminal is never even opened"
        );
    }

    /// A pseudo-terminal: `master` is the person's keyboard and screen, `slave` the terminal
    /// itself, and `path` its name, which the readers open afresh as a prompt opens
    /// `/dev/tty`. These tests need a real pty, and fail, never skip, where none can be opened:
    /// some sandboxes block `/dev/ptmx`, and CI runners provide one.
    struct Pty {
        master: File,
        slave: File,
        path: PathBuf,
    }

    impl Pty {
        /// The terminal, opened as `TtyPrompter` opens it: a description of its own,
        /// non-blocking.
        fn open(&self) -> File {
            open_terminal(&self.path).unwrap()
        }
    }

    fn pty() -> Pty {
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: both out-pointers are valid for one write each; a null name, settings and
        // window size ask openpty for its defaults.
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(
            rc,
            0,
            "these tests need a pseudo-terminal, and openpty failed: {}",
            io::Error::last_os_error()
        );
        // SAFETY: openpty returned 0, so both are open descriptors that nothing else owns.
        let (master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        let mut name = [0 as libc::c_char; 256];
        // SAFETY: `name` is writable for the length passed, and `slave` is open for the call.
        let rc = unsafe { libc::ttyname_r(slave.as_raw_fd(), name.as_mut_ptr(), name.len()) };
        assert_eq!(rc, 0, "ttyname_r: {}", io::Error::from_raw_os_error(rc));
        // SAFETY: ttyname_r returned 0, so `name` holds a NUL-terminated path.
        let name = unsafe { CStr::from_ptr(name.as_ptr()) };
        Pty {
            master: File::from(master),
            slave: File::from(slave),
            path: PathBuf::from(OsStr::from_bytes(name.to_bytes())),
        }
    }

    /// The terminal settings these tests compare: the flags and the control characters.
    type Flags = (
        libc::tcflag_t,
        libc::tcflag_t,
        libc::tcflag_t,
        libc::tcflag_t,
        [libc::cc_t; libc::NCCS],
    );

    fn flags(tty: &File) -> Flags {
        let t = termios(tty.as_fd()).unwrap();
        (t.c_iflag, t.c_oflag, t.c_cflag, t.c_lflag, t.c_cc)
    }

    /// Whether `f` has something to read within `ms`.
    fn readable(f: &File, ms: libc::c_int) -> bool {
        let mut fds = libc::pollfd {
            fd: f.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid, writable `pollfd`, and `f` keeps its descriptor open for the call.
        unsafe { libc::poll(&mut fds, 1, ms) > 0 }
    }

    /// What the terminal has shown on its screen since the last look, waiting at most 200 ms for
    /// more each time.
    fn shown(master: &File) -> Vec<u8> {
        let (mut out, mut chunk, mut screen) = (Vec::new(), [0u8; 256], master);
        while readable(master, 200) {
            match screen.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => out.extend_from_slice(&chunk[..n]),
            }
        }
        out
    }

    /// Types `keys` at the terminal, and waits until it has taken them in: its echo shows it.
    fn typed(master: &File, keys: &[u8]) {
        let mut keyboard = master;
        keyboard.write_all(keys).unwrap();
        let echo = String::from_utf8_lossy(keys).trim_end().to_owned();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut screen = Vec::new();
        while !String::from_utf8_lossy(&screen).contains(&echo) {
            assert!(
                Instant::now() < deadline,
                "the terminal never took the input"
            );
            screen.extend(shown(master));
        }
    }

    type Reader = fn(&File, &Cancel) -> io::Result<LineRead>;

    /// A read with its window between readiness and the read (`read_line_between`).
    type WindowReader = fn(&File, &Cancel, &mut dyn FnMut()) -> io::Result<LineRead>;

    /// The line prompts' read and the secret prompt's.
    fn readers() -> [(&'static str, Reader); 2] {
        [("line", read_line), ("secret", read_secret)]
    }

    /// Runs `read` on its own thread and returns what it read. Fails the test, never hangs it,
    /// when the read has not returned within 3 s: one that blocks never does.
    fn within_3s(read: impl FnOnce() -> io::Result<LineRead> + Send + 'static) -> LineRead {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(read());
        });
        rx.recv_timeout(Duration::from_secs(3))
            .expect("the read never returned: it blocked")
            .unwrap()
    }

    #[test]
    fn a_signal_ends_either_read_and_leaves_the_terminal_as_it_was() {
        // §14.1: every prompt, the no-echo one included, is a cancellation point. A Ctrl-C
        // reaches the handler as SIGINT, and a SIGTERM or SIGHUP from elsewhere arrives the
        // same way: each only sets the token. Half an answer is typed, and no newline ever is.
        for (name, read) in readers() {
            for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
                let pty = pty();
                let tty = pty.open();
                let before = flags(&pty.slave);
                assert_ne!(before.3 & libc::ECHO, 0, "a fresh pty echoes");
                (&pty.master).write_all(b"sk-ant-half").unwrap();
                let cancel = Cancel::new();
                let started = Instant::now();
                let setter = set_after(&cancel, Duration::from_millis(150), signal);
                let got = read(&tty, &cancel).unwrap();
                let waited = started.elapsed();
                setter.join().unwrap();
                assert_eq!(got, LineRead::Interrupted, "{name}, signal {signal}");
                assert!(
                    waited >= Duration::from_millis(150) && waited < Duration::from_millis(600),
                    "{name}, signal {signal}: about one slice after it: {waited:?}"
                );
                assert_eq!(
                    flags(&pty.slave),
                    before,
                    "{name}, signal {signal}: the terminal is as it was"
                );
            }
        }
    }

    #[test]
    fn input_flushed_between_readiness_and_the_read_never_blocks_either_read() {
        // The window between `poll` and `read`: a whole line makes the terminal ready, then a
        // Ctrl-C (ISIG) empties its input before the read, and the handler sets the token. A
        // blocking read would wait for input that is gone; this one reports `WouldBlock` and
        // goes back to the token.
        let readers: [(&str, WindowReader); 2] = [
            ("line", |tty, cancel, between| {
                read_line_between(tty, cancel, between)
            }),
            ("secret", |tty, cancel, between| {
                read_secret_between(tty, cancel, between)
            }),
        ];
        for (name, read) in readers {
            let pty = pty();
            let tty = pty.open();
            typed(&pty.master, b"yes\n");
            let cancel = Cancel::new();
            let windows = Arc::new(AtomicUsize::new(0));
            let (handler, seen) = (cancel.clone(), windows.clone());
            let started = Instant::now();
            let got = within_3s(move || {
                let mut between = || {
                    // What the terminal does on a Ctrl-C: it discards its input queue.
                    // SAFETY: tcflush(3) only acts on the terminal behind `tty`, open here.
                    let _ = unsafe { libc::tcflush(tty.as_raw_fd(), libc::TCIFLUSH) };
                    handler.request(libc::SIGINT);
                    seen.fetch_add(1, Ordering::SeqCst);
                };
                read(&tty, &cancel, &mut between)
            });
            assert_eq!(got, LineRead::Interrupted, "{name}");
            assert_eq!(
                windows.load(Ordering::SeqCst),
                1,
                "{name}: the read went through the window once"
            );
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "{name}: {:?}",
                started.elapsed()
            );
        }
    }

    #[test]
    fn an_interrupted_read_discards_what_was_typed() {
        // Half an answer left in the terminal's queue would reach the shell's prompt, and its
        // screen, once tagteam exits.
        for (name, read) in readers() {
            let pty = pty();
            let tty = pty.open();
            typed(&pty.master, b"sk-ant-half");
            let cancel = Cancel::new();
            cancel.request(libc::SIGTERM);
            assert_eq!(
                read(&tty, &cancel).unwrap(),
                LineRead::Interrupted,
                "{name}"
            );
            (&pty.master).write_all(b"\n").unwrap();
            assert_eq!(
                read(&tty, &Cancel::new()).unwrap(),
                LineRead::Line(String::new()),
                "{name}: only the newline typed afterwards is left"
            );
        }
    }

    #[test]
    fn a_line_longer_than_one_read_is_read_whole() {
        let pty = pty();
        let tty = pty.open();
        let long = "a".repeat(300);
        (&pty.master)
            .write_all(format!("{long}\n").as_bytes())
            .unwrap();
        assert_eq!(
            read_line(&tty, &Cancel::new()).unwrap(),
            LineRead::Line(long)
        );
    }

    #[test]
    fn a_secret_is_read_with_echo_off_and_the_terminal_s_signals_on() {
        let pty = pty();
        let tty = pty.open();
        let before = flags(&pty.slave);
        let cancel = Cancel::new();
        let reader = thread::spawn(move || read_secret(&tty, &cancel).unwrap());
        // The barrier: echo going off says the read is under way, before anything is typed.
        let deadline = Instant::now() + Duration::from_secs(5);
        let during = loop {
            let now = flags(&pty.slave);
            if now.3 & libc::ECHO == 0 {
                break now;
            }
            assert!(Instant::now() < deadline, "echo never went off");
            thread::sleep(Duration::from_millis(5));
        };
        assert_ne!(
            during.3 & libc::ISIG,
            0,
            "Ctrl-C still raises SIGINT, so it is never a byte of the secret"
        );
        assert_ne!(
            during.3 & libc::ICANON,
            0,
            "the terminal still edits the line"
        );
        (&pty.master).write_all(b"sk-ant-secret\n").unwrap();
        assert_eq!(
            reader.join().unwrap(),
            LineRead::Line("sk-ant-secret".into())
        );
        assert_eq!(flags(&pty.slave), before, "echo is back on");
        assert!(
            !String::from_utf8_lossy(&shown(&pty.master)).contains("secret"),
            "nothing typed was shown"
        );
    }

    #[test]
    fn a_ctrl_c_typed_at_the_secret_prompt_is_never_part_of_the_secret() {
        // `ISIG` stays on, so the terminal takes ^C itself: it discards the line typed so far
        // and signals its foreground group (nobody here: the pty is no one's controlling
        // terminal; in tagteam, the handler sets the token).
        let pty = pty();
        let tty = pty.open();
        let cancel = Cancel::new();
        let reader = thread::spawn(move || read_secret(&tty, &cancel).unwrap());
        (&pty.master).write_all(b"wrong\x03").unwrap();
        (&pty.master).write_all(b"right\n").unwrap();
        assert_eq!(reader.join().unwrap(), LineRead::Line("right".into()));
    }

    #[test]
    fn end_of_input_before_a_line_reads_as_the_end() {
        for (name, read) in readers() {
            let pty = pty();
            let tty = pty.open();
            (&pty.master).write_all(&[4]).unwrap(); // Ctrl-D on an empty line
            assert_eq!(read(&tty, &Cancel::new()).unwrap(), LineRead::End, "{name}");
        }
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam --lib prompt::tests`
Expected: FAIL to compile. The errors include "cannot find function `wait_for_input` in this
scope", "cannot find function `read_line_between` in this scope", "cannot find type
`LineRead` in this scope", "cannot find function `open_terminal` in this scope", "no function or
associated item named `new` found for struct `TtyPrompter`", and "cannot find type `Cancel` in
this scope".

- [ ] **Step 3: Read every prompt non-blocking from the terminal, the secret with echo off, and drop rpassword**

Why the terminal and not fd 0:
- A read that waits for readiness and then blocks can hang. `poll` reports a whole line, and a
  Ctrl-C (`ISIG`) then flushes it before `read` runs; the handler sets the token, but a
  blocking read waits for input that is gone, and `SA_RESTART` restarts it after every further
  signal. So every read must be non-blocking.
- `O_NONBLOCK` belongs to an open file description, and fd 0's description is shared with the
  shell; a `dup` shares it too.
- So each prompt opens `/dev/tty` afresh (`open_terminal`): a description of tagteam's own,
  with `O_NONBLOCK` (and `O_NOCTTY`), closed with the prompt, with no flag on anything shared
  to put back.
- The line prompts' answer therefore comes from the controlling terminal, as the secret's
  always did (rpassword read `/dev/tty`). `interactive()` still decides whether to prompt at
  all, from stdin and stderr, and for a person at a terminal they are that terminal. The
  in-process tests use their own `Scripted` prompter and are unaffected.
- The questions stay where they were: the line prompts' on stderr, the secret's on the
  terminal.

In `crates/tagteam/src/prompt.rs`, replace everything above `#[cfg(test)]` with:

```rust
use std::fs::{File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use tagteam_provider::Cancel;

pub trait Prompter {
    /// True only when a person can answer: stdin and stderr are both terminals.
    fn interactive(&self) -> bool;
    fn confirm(&mut self, question: &str, default_yes: bool) -> bool;
    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize>;
    fn secret(&mut self, question: &str) -> Option<String>;
}

/// The controlling terminal: every prompt reads its answer there.
const TERMINAL: &str = "/dev/tty";

/// The terminal's prompts. Each waits for its answer in slices the cancel token can end, and
/// never blocks in a read (Decision 5): a prompt a signal cuts short answers as a decline
/// (`false` or `None`), and the CLI, which checks the token after every prompt, reports the
/// interruption.
pub struct TtyPrompter {
    cancel: Cancel,
}

impl TtyPrompter {
    pub fn new(cancel: Cancel) -> Self {
        Self { cancel }
    }

    /// Asks `text` on stderr, then reads one answer from the terminal (`read_line`). `None`
    /// once the token is set: before asking, if it already was, or while waiting; then the
    /// prompt's line is ended, so what follows does not share it with the `^C` the terminal
    /// echoed.
    fn answer(&self, text: &str) -> Option<String> {
        if self.cancel.requested().is_some() {
            return None;
        }
        ask(text);
        let read = open_terminal(Path::new(TERMINAL)).and_then(|tty| read_line(&tty, &self.cancel));
        if matches!(read, Ok(LineRead::Interrupted)) {
            ask("\n");
        }
        answer_of(read)
    }
}

/// The terminal at `path` opened afresh for one prompt: read and write, non-blocking, and never
/// as a controlling terminal. A fresh open is a file description of tagteam's own, so
/// `O_NONBLOCK` never reaches fd 0's, which the shell shares (a `dup` would share it too), and
/// nothing needs putting back: it closes with the prompt. Whether to prompt at all is still
/// `interactive`'s call, from stdin and stderr; a person at a terminal answers on it, and it is
/// the controlling one, which the secret prompt has always read.
fn open_terminal(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
}

/// How long one `poll(2)` waits before the token is looked at again (Decision 5).
const SLICE_MS: libc::c_int = 100;

/// Waits until `input` has something to read, or until `cancel` holds a signal: `true` to go on
/// and read, `false` when interrupted. The handler restarts interrupted syscalls (Decision 2),
/// so a blocked read would never notice the signal; `poll(2)` in 100 ms slices, with the token
/// looked at between them, does. A hang-up or an error counts as ready, so the read that
/// follows reports it. Readiness is only a hint (`read_line_between`).
fn wait_for_input(input: BorrowedFd<'_>, cancel: &Cancel) -> bool {
    loop {
        if cancel.requested().is_some() {
            return false;
        }
        let mut fds = libc::pollfd {
            fd: input.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `fds` is one valid, writable `pollfd` for the duration of the call, and
        // `input` keeps its descriptor open for at least as long.
        let ready = unsafe { libc::poll(&mut fds, 1, SLICE_MS) };
        if ready > 0 {
            return true;
        }
        if ready < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return true;
        }
    }
}

/// Discards what was typed but not yet read, so an interrupted prompt's half answer never
/// reaches the shell once tagteam exits.
fn discard_input(tty: BorrowedFd<'_>) {
    // SAFETY: tcflush(3) only acts on the terminal behind `tty`, which is open for the call.
    let _ = unsafe { libc::tcflush(tty.as_raw_fd(), libc::TCIFLUSH) };
}

/// How a prompt's read ended. No derived `Debug`: the line may be a secret.
#[derive(PartialEq, Eq)]
enum LineRead {
    /// The line typed, without its line ending.
    Line(String),
    /// End of input (Ctrl-D) before anything was typed.
    End,
    /// The cancel token was set before the line was complete; what was typed is discarded.
    Interrupted,
}

impl std::fmt::Debug for LineRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LineRead::Line(s) => write!(f, "Line(<{} bytes>)", s.len()),
            LineRead::End => f.write_str("End"),
            LineRead::Interrupted => f.write_str("Interrupted"),
        }
    }
}

/// Reads one line from `tty`, which `open_terminal` opened non-blocking (§14.1, Decision 5).
fn read_line(tty: &File, cancel: &Cancel) -> io::Result<LineRead> {
    read_line_between(tty, cancel, || {})
}

/// `read_line`, running `between` each time the wait reports input, just before the read: the
/// window in which a Ctrl-C at the terminal can flush the line the wait saw (the tests use it).
/// Readiness is only a hint. The read is non-blocking, so a line flushed in that window makes
/// it report `WouldBlock`, never wait, and the loop goes back to the token. What each read
/// returns is kept until a newline or the end of input. Interrupted, it discards what was
/// typed, so a half answer never reaches the shell once tagteam exits.
fn read_line_between(
    tty: &File,
    cancel: &Cancel,
    mut between: impl FnMut(),
) -> io::Result<LineRead> {
    let (mut line, mut chunk, mut reader) = (Vec::new(), [0u8; 256], tty);
    loop {
        if !wait_for_input(tty.as_fd(), cancel) {
            discard_input(tty.as_fd());
            return Ok(LineRead::Interrupted);
        }
        between();
        match reader.read(&mut chunk) {
            Ok(0) if line.is_empty() => return Ok(LineRead::End),
            Ok(0) => return line_of(line),
            Ok(n) => {
                line.extend_from_slice(&chunk[..n]);
                if line.contains(&b'\n') {
                    return line_of(line);
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
    }
}

/// The line as read, without its line ending. A line that is not UTF-8 is an error that never
/// repeats the bytes.
fn line_of(mut line: Vec<u8>) -> io::Result<LineRead> {
    if let Some(end) = line.iter().position(|&b| b == b'\n') {
        line.truncate(end);
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line)
        .map(LineRead::Line)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the line is not UTF-8"))
}

/// A line prompt's answer, trimmed. `None` at end of input (Ctrl-D), on an interruption or on
/// a read error: every prompt takes that as a decline, never as the default answer an empty
/// line (Enter) gives.
fn answer_of(read: io::Result<LineRead>) -> Option<String> {
    match read {
        Ok(LineRead::Line(s)) => Some(s.trim().to_owned()),
        Ok(LineRead::End | LineRead::Interrupted) | Err(_) => None,
    }
}

/// The terminal's settings.
fn termios(tty: BorrowedFd<'_>) -> io::Result<libc::termios> {
    let mut t = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: `t` is writable for one `termios`, and `tty` is open for the call.
    if unsafe { libc::tcgetattr(tty.as_raw_fd(), t.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: tcgetattr returned 0, so it filled `t` in.
    Ok(unsafe { t.assume_init() })
}

fn set_termios(tty: BorrowedFd<'_>, t: &libc::termios) -> io::Result<()> {
    // SAFETY: `t` is a whole `termios` read from a terminal, and `tty` is open for the call.
    if unsafe { libc::tcsetattr(tty.as_raw_fd(), libc::TCSANOW, t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The terminal with its echo off, put back exactly as it was when this is dropped: on every
/// return, and while a panic unwinds (§14.1: an interrupted prompt restores the terminal).
struct EchoOff<'a> {
    tty: BorrowedFd<'a>,
    saved: libc::termios,
}

impl<'a> EchoOff<'a> {
    /// Turns `ECHO` and `ECHONL` off, and nothing else. `ICANON` stays on, so the terminal
    /// edits the line (backspace, Ctrl-U) and hands it over whole. `ISIG` stays on, so a
    /// Ctrl-C raises SIGINT, which the handler turns into the token, and is never a byte of the
    /// secret.
    fn new(tty: BorrowedFd<'a>) -> io::Result<Self> {
        let saved = termios(tty)?;
        let mut quiet = saved;
        quiet.c_lflag &= !(libc::ECHO | libc::ECHONL);
        set_termios(tty, &quiet)?;
        Ok(Self { tty, saved })
    }
}

impl Drop for EchoOff<'_> {
    fn drop(&mut self) {
        let _ = set_termios(self.tty, &self.saved);
    }
}

/// Reads one line from the terminal `tty` with its echo off (`EchoOff`), as `read_line` does,
/// so a signal ends a secret prompt as promptly as any other (§14.1). The terminal's settings
/// are back as they were whichever way it returns. A line must fit the terminal's line limit
/// (1024 bytes on macOS), far above any token; `add-token -` takes a longer one from a pipe.
fn read_secret(tty: &File, cancel: &Cancel) -> io::Result<LineRead> {
    read_secret_between(tty, cancel, || {})
}

/// `read_secret`, with `read_line_between`'s window.
fn read_secret_between(tty: &File, cancel: &Cancel, between: impl FnMut()) -> io::Result<LineRead> {
    let _echo_off = EchoOff::new(tty.as_fd())?;
    read_line_between(tty, cancel, between)
}

/// Writes a prompt to stderr, so stdout stays the command's output. A failed write is
/// ignored rather than panicking: the answer read next decides either way.
fn ask(text: &str) {
    let mut err = std::io::stderr().lock();
    let _ = write!(err, "{text}");
    let _ = err.flush();
}

fn confirmed(answer: Option<String>, default_yes: bool) -> bool {
    match answer.map(|a| a.to_ascii_lowercase()).as_deref() {
        None => false,
        Some("") => default_yes,
        Some(a) => a == "y" || a == "yes",
    }
}

/// The 0-based index of a 1-based answer within `count` options.
fn chosen(answer: Option<String>, count: usize) -> Option<usize> {
    answer?
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=count).contains(n))
        .map(|n| n - 1)
}

impl Prompter for TtyPrompter {
    fn interactive(&self) -> bool {
        std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
    }

    fn confirm(&mut self, question: &str, default_yes: bool) -> bool {
        let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
        confirmed(self.answer(&format!("{question} {hint} ")), default_yes)
    }

    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize> {
        let mut text: String = options
            .iter()
            .enumerate()
            .map(|(i, o)| format!("  {}) {o}\n", i + 1))
            .collect();
        text.push_str(&format!("{question} [1-{}] ", options.len()));
        chosen(self.answer(&text), options.len())
    }

    /// The no-echo prompt (§10.2), on the terminal itself as before: the question is written
    /// there and the line read from it by `read_secret`, which a signal ends as promptly as any
    /// other prompt and which leaves the terminal as it found it. The Enter typed was not
    /// echoed, so the line is ended here. A terminal that cannot be opened or set up declines.
    fn secret(&mut self, question: &str) -> Option<String> {
        if self.cancel.requested().is_some() {
            return None;
        }
        let tty = open_terminal(Path::new(TERMINAL)).ok()?;
        let _ = (&tty).write_all(question.as_bytes());
        let read = read_secret(&tty, &self.cancel);
        let _ = (&tty).write_all(b"\n");
        match read {
            Ok(LineRead::Line(secret)) => Some(secret),
            Ok(LineRead::End | LineRead::Interrupted) | Err(_) => None,
        }
    }
}
```

In `crates/tagteam/src/lib.rs`, replace:

```rust
    let mut prompter = prompt::TtyPrompter;
    let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr().lock());
    app::run(
        cli,
        app::Context::from_process(),
        &mut app::Io {
```

with:

```rust
    let ctx = app::Context::from_process();
    let mut prompter = prompt::TtyPrompter::new(ctx.env.cancel.clone());
    let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr().lock());
    app::run(
        cli,
        ctx,
        &mut app::Io {
```

`rpassword` now has no user (`rg -n rpassword crates/` finds only the two manifests). In
`crates/tagteam/Cargo.toml`, delete the line:

```toml
rpassword.workspace = true
```

and in the workspace `Cargo.toml`, delete the line:

```toml
rpassword = "7"
```

The next build drops `rpassword` and `rtoolbox` from `Cargo.lock`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam --lib prompt::tests`
Expected: PASS, 14 tests: the three existing ones and eleven new ones. The seven pty tests need
a real pseudo-terminal. Where `/dev/ptmx` is blocked (some sandboxes), they fail with "these
tests need a pseudo-terminal, and openpty failed: Operation not permitted"; they never skip.
Run them where a pty can be opened. CI runners can open one. With `O_NONBLOCK` left out of
`open_terminal`, `input_flushed_between_readiness_and_the_read_never_blocks_either_read` fails
after 3 s with "the read never returned: it blocked".

Run: `cargo test -p tagteam --test app`
Expected: PASS. In-process tests use their own `Scripted` prompter.

- [ ] **Step 5: Run the checks and commit**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: `fmt --check` prints nothing, and both clippy runs finish with no warnings.

```bash
git add Cargo.toml Cargo.lock crates/tagteam/Cargo.toml crates/tagteam/src/prompt.rs \
  crates/tagteam/src/lib.rs
git commit -m "Read every prompt without blocking, in slices a signal can end"
```

#### Part B: exit codes, the interrupted envelope, prompts and the late notice

- [ ] **Step 6: Write the failing in-process tests**

In `crates/tagteam/tests/app.rs`, replace:

```rust
use tagteam_provider::{Env, FakeKeychain};
```

with:

```rust
use tagteam_provider::{Cancel, Env, FakeKeychain};
```

(`tagteam_engine::store::Store`, which the new tests also use, is imported already.)

Replace the `Scripted` struct, its inherent `impl`, and its `Prompter` impl (from
`struct Scripted {` to the closing brace of `impl Prompter for Scripted`) with:

```rust
struct Scripted {
    interactive: bool,
    answers: VecDeque<&'static str>,
    asked: Vec<String>,
    /// The signal "sent" at every prompt, into this token, as a terminal's Ctrl-C would be.
    interrupt: Option<(Cancel, i32)>,
}

impl Scripted {
    fn none() -> Self {
        Self {
            interactive: false,
            answers: VecDeque::new(),
            asked: Vec::new(),
            interrupt: None,
        }
    }
    fn answering(a: &[&'static str]) -> Self {
        Self {
            interactive: true,
            answers: a.iter().copied().collect(),
            asked: Vec::new(),
            interrupt: None,
        }
    }
    /// A person who presses Ctrl-C at every prompt: `signal` lands in `cancel` while the prompt
    /// is up. The scripted answer is still given, where `TtyPrompter` would decline: whatever a
    /// prompt answers, the command must stop on the signal (Decision 5).
    fn interrupted_by(cancel: &Cancel, signal: i32, a: &[&'static str]) -> Self {
        Self {
            interrupt: Some((cancel.clone(), signal)),
            ..Self::answering(a)
        }
    }
    /// Notes the question, and sends the signal if this person interrupts.
    fn record(&mut self, q: &str) {
        self.asked.push(q.to_owned());
        if let Some((cancel, signal)) = &self.interrupt {
            cancel.request(*signal);
        }
    }
}

impl Prompter for Scripted {
    fn interactive(&self) -> bool {
        self.interactive
    }
    fn confirm(&mut self, q: &str, default_yes: bool) -> bool {
        self.record(q);
        match self.answers.pop_front().expect("unexpected prompt") {
            "" => default_yes,
            a => a.starts_with('y'),
        }
    }
    fn choose(&mut self, q: &str, _o: &[String]) -> Option<usize> {
        self.record(q);
        self.answers
            .pop_front()
            .expect("unexpected prompt")
            .parse()
            .ok()
    }
    fn secret(&mut self, q: &str) -> Option<String> {
        self.record(q);
        Some(
            self.answers
                .pop_front()
                .expect("unexpected prompt")
                .to_owned(),
        )
    }
}
```

In `impl H`, add `run_with_cancel` after `run`. `H::run_in(args, prompter, adjust)` already
applies a closure to the `Context` before the run, so the token goes in through it. Replace:

```rust
    fn run(&self, args: &[&str], prompter: &mut Scripted) -> (i32, String, String) {
        self.run_in(args, prompter, |_| {})
    }
```

with:

```rust
    fn run(&self, args: &[&str], prompter: &mut Scripted) -> (i32, String, String) {
        self.run_in(args, prompter, |_| {})
    }

    /// `run` with `cancel` as the process's token, which a signal handler would set (§14.1).
    fn run_with_cancel(
        &self,
        args: &[&str],
        prompter: &mut Scripted,
        cancel: &Cancel,
    ) -> (i32, String, String) {
        self.run_in(args, prompter, |ctx| ctx.env.cancel = cancel.clone())
    }
```

`run` keeps the fixture's own token, which no test sets.

Append to the end of the file:

```rust
/// A token already set when the command starts: the signal came first.
fn signalled(signal: i32) -> Cancel {
    let cancel = Cancel::new();
    cancel.request(signal);
    cancel
}

/// `a@x.co` at position 1, and `b@x.co` at position 2 and live.
fn two_logins() -> H {
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    h.login("b@x.co", "rt-b");
    h.ok(&["add"]);
    h
}

/// §13.2's envelope for §14.1's interruption (Decision 4).
fn interrupted_json() -> Value {
    json!({"schemaVersion": 1, "error": {"type": "interrupted", "message": "interrupted"}})
}

const INTERRUPTED: &str = "tagteam: interrupted\n";

/// Decision 6's notice for `command`.
fn too_late(command: &str) -> String {
    format!("tagteam: interrupted too late to stop: {command} had already finished\n")
}

#[test]
fn an_interrupted_command_exits_128_plus_the_signal() {
    // §13.1, §14.1, Decision 4: the switch stops at its first lock wait, before it writes.
    let h = two_logins();
    let item = (
        keychain_service(&h.env, ItemKind::OAuth),
        keychain_account(&h.env),
    );
    let entry = h.kc.get(&item.0, &item.1);
    for (signal, code) in [
        (libc::SIGINT, 130),
        (libc::SIGTERM, 143),
        (libc::SIGHUP, 129),
    ] {
        let (got, out, err) = h.run_with_cancel(
            &["switch", "1", "--json"],
            &mut Scripted::none(),
            &signalled(signal),
        );
        assert_eq!((got, err.as_str()), (code, ""), "signal {signal}");
        assert_eq!(
            serde_json::from_str::<Value>(&out).unwrap(),
            interrupted_json(),
            "signal {signal}"
        );
    }
    let (code, out, err) = h.run_with_cancel(
        &["switch", "1"],
        &mut Scripted::none(),
        &signalled(libc::SIGINT),
    );
    assert_eq!((code, out.as_str(), err.as_str()), (130, "", INTERRUPTED));
    assert_eq!(common::live_email(h.env.home.parent().unwrap()), "b@x.co");
    assert_eq!(h.kc.get(&item.0, &item.1), entry, "nothing was written");
}

#[test]
fn an_interrupted_usage_collection_interrupts_list_and_status() {
    // §14.1: collecting is a cancellation point, and an interrupted collection records nothing.
    let h = with_one_login();
    for args in [["list", "--json"], ["status", "--json"]] {
        let (code, out, err) =
            h.run_with_cancel(&args, &mut Scripted::none(), &signalled(libc::SIGINT));
        assert_eq!((code, err.as_str()), (130, ""), "{args:?}");
        assert_eq!(
            serde_json::from_str::<Value>(&out).unwrap(),
            interrupted_json(),
            "{args:?}"
        );
    }
    let store = Store::open_existing(&h.env.data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    let a = store.accounts(&ProviderId::new(CLAUDE_CODE)).unwrap()[0]
        .id
        .clone();
    assert_eq!(
        store.usage_state(&a).unwrap(),
        None,
        "nothing was recorded, not even a failure"
    );
}

#[test]
fn a_signal_that_meets_no_cancellation_point_is_too_late_and_changes_nothing() {
    // Decision 6: `list` on a fresh home collects nothing, so nothing could stop it.
    let h = H::new();
    let (code, out, err) = h.run_with_cancel(
        &["list", "--json"],
        &mut Scripted::none(),
        &signalled(libc::SIGINT),
    );
    assert_eq!((code, err), (0, too_late("list")));
    assert_eq!(
        out,
        h.ok(&["list", "--json"]),
        "stdout is one JSON object, as without the signal"
    );
    let (code, out, err) =
        h.run_with_cancel(&["list"], &mut Scripted::none(), &signalled(libc::SIGTERM));
    assert_eq!((code, err), (0, too_late("list")));
    assert_eq!(out, h.ok(&["list"]));
    // A command that failed for a reason of its own keeps its error and its exit code.
    let (code, out, err) = h.run_with_cancel(
        &["switch", "9"],
        &mut Scripted::none(),
        &signalled(libc::SIGINT),
    );
    assert_eq!((code, out.as_str()), (1, ""));
    assert_eq!(
        err,
        format!("tagteam: no account matches \"9\"\n{}", too_late("switch"))
    );
}

#[test]
fn ctrl_c_at_the_offer_to_add_the_live_login_interrupts_and_adds_nothing() {
    // Review Focus 3. The scripted answer is a yes: whatever the prompt answered, the signal
    // stops the command.
    let h = with_unmanaged_login();
    let cancel = Cancel::new();
    let mut ctrl_c = Scripted::interrupted_by(&cancel, libc::SIGINT, &["y"]);
    let (code, out, err) = h.run_with_cancel(&["switch", "1"], &mut ctrl_c, &cancel);
    assert_eq!((code, out.as_str(), err.as_str()), (130, "", INTERRUPTED));
    assert_eq!(ctrl_c.asked, [ADD_STRANGER_FIRST]);
    assert_eq!(
        h.ok(&["status"]),
        "Live: stranger@x.co (not managed by tagteam)\n"
    );
}

#[test]
fn ctrl_c_at_the_secret_prompt_interrupts_and_adds_nothing() {
    // Review Focus 3: `add-token`'s no-echo prompt. The terminal side (echo restored, typed
    // input discarded) is `read_secret`'s, pinned on a pty in `prompt.rs`.
    let h = with_one_login();
    let cancel = Cancel::new();
    let mut ctrl_c = Scripted::interrupted_by(&cancel, libc::SIGINT, &["sk-ant-api03-key"]);
    let (code, out, err) = h.run_with_cancel(&["add-token"], &mut ctrl_c, &cancel);
    assert_eq!((code, out.as_str(), err.as_str()), (130, "", INTERRUPTED));
    assert_eq!(ctrl_c.asked, ["Token: "]);
    assert_eq!(h.ok(&["list"]), LIVE_A_OFFLINE);
}

#[test]
fn ctrl_c_at_the_unlock_question_never_runs_the_unlock() {
    // Appendix A.3's question is a prompt like any other: a yes typed as the signal lands does
    // not go on to `security unlock-keychain`.
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.kc.set_locked(true);
    let cancel = Cancel::new();
    let mut ctrl_c = Scripted::interrupted_by(&cancel, libc::SIGINT, &[""]);
    let (code, out, err) = h.run_with_cancel(&["add"], &mut ctrl_c, &cancel);
    assert_eq!((code, out.as_str(), err.as_str()), (130, "", INTERRUPTED));
    assert_eq!(ctrl_c.asked, [UNLOCK]);
    assert_eq!(h.kc.unlock_attempts(), 0);
    assert!(!h.env.data_dir().exists(), "nothing was added");
}

#[test]
fn ctrl_c_at_a_choice_or_a_replacement_question_interrupts() {
    // §10.4's choice of account, then §10.1's replacement question, under SIGTERM.
    let h = H::new();
    h.login_in("a@x.co", "", "rt-personal");
    h.ok(&["add"]);
    h.login_in("a@x.co", "org-1", "rt-org");
    h.ok(&["add"]);
    let cancel = Cancel::new();
    let mut ctrl_c = Scripted::interrupted_by(&cancel, libc::SIGTERM, &["1"]);
    let (code, out, err) = h.run_with_cancel(&["disable", "a@x.co"], &mut ctrl_c, &cancel);
    assert_eq!((code, out.as_str(), err.as_str()), (143, "", INTERRUPTED));
    assert_eq!(ctrl_c.asked, ["Which account?"]);
    let list = h.json(&["list", "--json"]);
    assert_eq!(
        (
            list["accounts"][0].get("disabled"),
            list["accounts"][1].get("disabled")
        ),
        (None, None),
        "nothing was disabled"
    );

    let h = with_one_login();
    let cancel = Cancel::new();
    let mut ctrl_c = Scripted::interrupted_by(&cancel, libc::SIGTERM, &["y"]);
    let (code, out, err) = h.run_with_cancel(
        &["add-token", "sk-ant-api03-key", "--position", "1"],
        &mut ctrl_c,
        &cancel,
    );
    assert_eq!((code, out.as_str(), err.as_str()), (143, "", INTERRUPTED));
    assert_eq!(ctrl_c.asked, [REPLACE_A]);
    assert_eq!(h.ok(&["list"]), LIVE_A_OFFLINE);
}
```

In `crates/tagteam/src/app.rs`, inside `mod tests`, append:

```rust
    #[test]
    fn the_late_notice_names_each_command_as_it_is_typed() {
        use clap::Parser;
        let cases: [&[&str]; 14] = [
            &["list"],
            &["ls"],
            &["status"],
            &["switch"],
            &["add"],
            &["add-token", "x"],
            &["remove", "1"],
            &["rm", "1"],
            &["disable", "1"],
            &["enable", "1"],
            &["alias"],
            &["move", "1", "2"],
            &["history"],
            &["statusline"],
        ];
        for args in cases {
            let cli = Cli::try_parse_from(std::iter::once("tagteam").chain(args.iter().copied()))
                .unwrap();
            let expected = match args[0] {
                "ls" => "list",
                "rm" => "remove",
                typed => typed,
            };
            assert_eq!(
                command_name(cli.command.as_ref().unwrap()),
                expected,
                "{args:?}"
            );
        }
    }
```

- [ ] **Step 7: Run the tests to verify they fail**

Run: `cargo test -p tagteam --lib app::tests`
Expected: FAIL to compile with "cannot find function `command_name` in this scope".

Run: `cargo test -p tagteam --test app`
Expected: FAIL, the seven new tests. Every existing test passes.
- `an_interrupted_command_exits_128_plus_the_signal`: exit 1, not 130. Task 2 stops the switch
  at its mutation-lock wait, but `run` reports that error like any other.
- `an_interrupted_usage_collection_interrupts_list_and_status`: exit 0. `App::collect` turns the
  interrupted collection into a stderr warning, and `list` goes on.
- `a_signal_that_meets_no_cancellation_point_is_too_late_and_changes_nothing`: stderr is empty,
  not the late notice.
- `ctrl_c_at_the_offer_to_add_the_live_login_interrupts_and_adds_nothing`,
  `ctrl_c_at_the_secret_prompt_interrupts_and_adds_nothing` and
  `ctrl_c_at_a_choice_or_a_replacement_question_interrupts`: exit 1. The answer goes on to the
  engine, whose first lock wait stops it as an ordinary error.
- `ctrl_c_at_the_unlock_question_never_runs_the_unlock`: exit 1 for the same reason (the unlock
  has also run: `unlock_attempts` is 1).

- [ ] **Step 8: Report interruptions, check after prompts, and print the late notice**

In `crates/tagteam/src/app.rs`, replace:

```rust
pub(crate) const EXIT_ERROR: i32 = 1;
pub(crate) const EXIT_USAGE: i32 = 2;
```

with:

```rust
pub(crate) const EXIT_ERROR: i32 = 1;
pub(crate) const EXIT_USAGE: i32 = 2;
/// §13.1: an interrupted command exits 128 + the signal (130 after SIGINT).
const EXIT_SIGNAL_BASE: i32 = 128;
```

Replace:

```rust
/// A provider without the capability a command needs (§4.5).
const KIND_UNSUPPORTED: &str = "unsupported";
```

with:

```rust
/// A provider without the capability a command needs (§4.5).
const KIND_UNSUPPORTED: &str = "unsupported";
/// §14.1, Decision 4: every interruption, whichever carrier holds its signal.
pub(crate) const KIND_INTERRUPTED: &str = "interrupted";
```

Replace:

```rust
const CANCELLED: &str = "cancelled";
```

with:

```rust
const CANCELLED: &str = "cancelled";
const INTERRUPTED: &str = "interrupted";
```

Replace the whole of `pub fn run` (from `pub fn run(cli: Cli, ctx: Context, io: &mut Io<'_>) -> i32 {`
to its closing brace, just above `fn fail`) with:

```rust
/// How a command ended (§13.1, §14.1).
enum Ended {
    /// It finished with this exit code; its output, and any error, are written.
    Code(i32),
    /// It stopped at a cancellation point after this signal; nothing is reported yet.
    Interrupted(i32),
}

/// Runs one command and returns its exit code (§13.1). A signal the command met at a
/// cancellation point ends it with 128 + the signal and the `interrupted` error (Decision 4);
/// one it never met leaves its output and exit code alone and is reported on stderr as too
/// late (Decision 6).
pub fn run(cli: Cli, ctx: Context, io: &mut Io<'_>) -> i32 {
    let json = cli.json;
    let name = cli.command.as_ref().map_or("list", command_name);
    let cancel = ctx.env.cancel.clone();
    match run_command(cli, ctx, io) {
        Ended::Interrupted(signal) => {
            fail(io, json, KIND_INTERRUPTED, INTERRUPTED);
            EXIT_SIGNAL_BASE + signal
        }
        Ended::Code(code) => {
            if cancel.requested().is_some() {
                let _ = writeln!(
                    io.err,
                    "tagteam: interrupted too late to stop: {name} had already finished"
                );
            }
            code
        }
    }
}

fn run_command(cli: Cli, ctx: Context, io: &mut Io<'_>) -> Ended {
    let color = !cli.no_color && !ctx.no_color_env;
    init_logging(cli.debug, color);
    let json = cli.json;
    if let Err(msg) = root_guard::refuse_root() {
        return Ended::Code(fail(io, json, KIND_ROOT, &msg));
    }
    // §13.5: the status bar's fast path, before anything else is built.
    if let Some(Command::Statusline { print_config }) = &cli.command {
        return Ended::Code(run_statusline(
            ctx,
            io,
            json,
            cli.no_color,
            cli.provider,
            *print_config,
        ));
    }
    let command = cli.command.unwrap_or(Command::List);
    let keychain = (ctx.platform == Platform::MacOs).then(|| ctx.keychain.clone());
    let (stdout_terminal, no_color_env, force_color_env) =
        (ctx.stdout_terminal, ctx.no_color_env, ctx.force_color_env);
    let provider_flag = cli.provider.map(ProviderId::new);
    let resolved = provider_flag
        .clone()
        .unwrap_or_else(|| ProviderId::new(CLAUDE_CODE));
    let (engine, warnings) = build_engine(ctx, &resolved);
    for w in &warnings {
        let _ = writeln!(io.err, "warning: {w}");
    }
    let mut app = App {
        engine,
        json,
        stdout_terminal,
        no_color_env,
        force_color_env,
        no_color: cli.no_color,
        provider_flag,
        keychain,
        io,
    };
    if let Some(p) = &app.provider_flag {
        if let Err(e) = app.engine.provider(p) {
            return Ended::Code(fail(app.io, json, e.kind(), &e.to_string()));
        }
    }
    // Only a command that touches a Keychain item checks its lock.
    let unlocked = if command.touches_keychain() {
        app.lock_check(&command)
    } else {
        Ok(())
    };
    let result = unlocked.and_then(|()| app.dispatch(command));
    match result {
        Ok(()) => Ended::Code(0),
        Err(Failure::Engine(e)) => match e.signal() {
            Some(signal) => Ended::Interrupted(signal),
            None => Ended::Code(fail(app.io, json, e.kind(), &e.to_string())),
        },
        Err(Failure::Message(kind, m)) => Ended::Code(fail(app.io, json, kind, &m)),
        Err(Failure::Usage(m)) => {
            fail(app.io, json, KIND_USAGE, &m);
            Ended::Code(EXIT_USAGE)
        }
    }
}

/// The command's name in its canonical spelling, for the late notice (Decision 6).
fn command_name(command: &Command) -> &'static str {
    match command {
        Command::List => "list",
        Command::Status => "status",
        Command::Switch { .. } => "switch",
        Command::Add { .. } => "add",
        Command::AddToken { .. } => "add-token",
        Command::Remove { .. } => "remove",
        Command::Disable { .. } => "disable",
        Command::Enable { .. } => "enable",
        Command::Alias { .. } => "alias",
        Command::Move { .. } => "move",
        Command::History { .. } => "history",
        Command::Statusline { .. } => "statusline",
    }
}
```

In `impl App`, replace:

```rust
    /// A person can answer a prompt: never with `--json`, and only on a terminal.
    fn can_prompt(&self) -> bool {
        !self.json && self.io.prompter.interactive()
    }
```

with:

```rust
    /// A person can answer a prompt: never with `--json`, and only on a terminal.
    fn can_prompt(&self) -> bool {
        !self.json && self.io.prompter.interactive()
    }

    /// §14.1, Decision 5: a prompt is a cancellation point. One a signal cut short has
    /// answered as a decline, but whatever it answered, the command stops here, interrupted.
    fn after_prompt(&self) -> Result<(), Failure> {
        match self.engine.cancel().requested() {
            Some(signal) => Err(EngineError::Interrupted(signal).into()),
            None => Ok(()),
        }
    }
```

Replace `collect`:

```rust
    fn collect(&mut self, accounts: Vec<AccountId>) {
        if accounts.is_empty() {
            return;
        }
        let warnings = match self
            .engine
            .collect_usage(CollectMode::OnDemand { accounts })
        {
            Ok(report) => report.warnings,
            Err(e) => vec![format!("usage was not collected: {e}")],
        };
        for w in warnings {
            let _ = writeln!(self.io.err, "warning: {w}");
        }
    }
```

with:

```rust
    /// An interrupted collection is the command's interruption (§14.1), never a warning.
    fn collect(&mut self, accounts: Vec<AccountId>) -> Result<(), Failure> {
        if accounts.is_empty() {
            return Ok(());
        }
        let warnings = match self
            .engine
            .collect_usage(CollectMode::OnDemand { accounts })
        {
            Ok(report) => report.warnings,
            Err(e) if e.signal().is_some() => return Err(e.into()),
            Err(e) => vec![format!("usage was not collected: {e}")],
        };
        for w in warnings {
            let _ = writeln!(self.io.err, "warning: {w}");
        }
        Ok(())
    }
```

In `dispatch`, replace `self.collect(ids);` with `self.collect(ids)?;`, and replace
`self.collect(vec![account.row.id]);` with `self.collect(vec![account.row.id])?;`.

In `ensure_unlocked`, replace:

```rust
        if self.can_prompt() && self.io.prompter.confirm(UNLOCK_QUESTION, true) && keychain.unlock()
        {
            return Ok(());
        }
        Err(Failure::Message(
```

with:

```rust
        if self.can_prompt() {
            let yes = self.io.prompter.confirm(UNLOCK_QUESTION, true);
            self.after_prompt()?;
            // macOS asks for the password on the terminal: a Ctrl-C there ends `security`,
            // which shares tagteam's process group (§14.1), and the command with it.
            if yes {
                let unlocked = keychain.unlock();
                self.after_prompt()?;
                if unlocked {
                    return Ok(());
                }
            }
        }
        Err(Failure::Message(
```

In `resolve`, replace:

```rust
                self.io
                    .prompter
                    .choose("Which account?", &labels)
                    .and_then(|i| found.get(i).cloned())
                    .ok_or_else(cancelled)
```

with:

```rust
                let choice = self.io.prompter.choose("Which account?", &labels);
                self.after_prompt()?;
                choice
                    .and_then(|i| found.get(i).cloned())
                    .ok_or_else(cancelled)
```

In `token`, replace:

```rust
            None if self.can_prompt() => self.io.prompter.secret("Token: ").ok_or_else(cancelled),
```

with:

```rust
            None if self.can_prompt() => {
                let token = self.io.prompter.secret("Token: ");
                self.after_prompt()?;
                token.ok_or_else(cancelled)
            }
```

In `confirming`, replace:

```rust
                let question = format!("Position {position} holds {occupant}. Replace it?");
                if !self.io.prompter.confirm(&question, false) {
                    return Err(cancelled());
                }
```

with:

```rust
                let question = format!("Position {position} holds {occupant}. Replace it?");
                let yes = self.io.prompter.confirm(&question, false);
                self.after_prompt()?;
                if !yes {
                    return Err(cancelled());
                }
```

In `switch`, replace:

```rust
            if !self
                .io
                .prompter
                .confirm(&format!("Add the current login ({email}) first?"), true)
            {
                return Err(cancelled());
            }
```

with:

```rust
            let add = self
                .io
                .prompter
                .confirm(&format!("Add the current login ({email}) first?"), true);
            self.after_prompt()?;
            if !add {
                return Err(cancelled());
            }
```

- [ ] **Step 9: Run the tests to verify they pass**

Run: `cargo test -p tagteam --lib`
Expected: PASS, including `the_late_notice_names_each_command_as_it_is_typed`.

Run: `cargo test -p tagteam --test app`
Expected: PASS, including the seven new tests.

Run: `cargo test -p tagteam --features test-support`
Expected: PASS. No binary test sends a signal yet, so no command prints the notice, and every
exit code is as before. Part A's seven pty tests run here too, and need a real pseudo-terminal.

- [ ] **Step 10: Run the checks and commit**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: `fmt --check` prints nothing, and both clippy runs finish with no warnings.

```bash
git add crates/tagteam/src/app.rs crates/tagteam/tests/app.rs
git commit -m "Exit 128 plus the signal when a command is interrupted"
```

#### Part C: the handlers, and real signals to the binary

- [ ] **Step 11: Write the failing binary tests**

Create `crates/tagteam/tests/signals.rs`:

```rust
//! §14.1 with real signals sent to the binary: SIGINT while a switch waits for Claude Code's
//! lock (Review Focus 1), and twice while a switch is inside its critical span (Review Focus 2).
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::Path;
use std::process::{Child, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::{live_email, std_cmd, two_fresh_accounts};
use serde_json::{Value, json};
use tagteam_cc::{CcPaths, ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::store::Store;
use tagteam_provider::{Env, FileKeychain, Keychain};

/// Sends `signal` to the child, as a terminal's Ctrl-C (SIGINT) or `kill` would.
fn send(child: &Child, signal: i32) {
    // SAFETY: kill(2) reads no memory of ours. `child` is a process this test spawned and has
    // not reaped, so its pid names it and no other process.
    let rc = unsafe { libc::kill(child.id() as libc::pid_t, signal) };
    assert_eq!(rc, 0, "kill: {}", std::io::Error::last_os_error());
}

/// Polls `ready` every 10 ms until it holds. Fails the test if the child exits first, or if
/// `within` passes.
fn wait_until(child: &mut Child, within: Duration, what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + within;
    while !ready() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("tagteam exited ({status}) before {what}");
        }
        assert!(Instant::now() < deadline, "tagteam never got to {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

/// The child's output once it has exited. Fails the test, and kills the child, if it is still
/// running after `within`.
fn finish(mut child: Child, within: Duration) -> Output {
    let deadline = Instant::now() + within;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("tagteam was still running {within:?} later");
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

fn store(root: &Path) -> Store {
    Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap()
}

/// The live OAuth entry, as Claude Code reads it.
fn live_entry(root: &Path) -> Option<Vec<u8>> {
    let env = Env::for_test(root);
    FileKeychain::new(root.join("keychain"))
        .find(
            &keychain_service(&env, ItemKind::OAuth),
            &keychain_account(&env),
        )
        .present()
}

#[test]
fn ctrl_c_while_a_switch_waits_for_claude_code_s_lock_exits_130_with_nothing_written() {
    // Review Focus 1. Claude Code is refreshing and holds its legacy lock, so the switch takes
    // and releases its own refresh lock over and over while it waits (§9.1).
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (_a, b) = two_fresh_accounts(root);
    let paths = CcPaths::resolve(&Env::for_test(root));
    fs::create_dir(paths.legacy_lock()).unwrap(); // Claude Code's, fresh: never stale here
    let entry = live_entry(root);
    let config_home = paths.refresh_lock.parent().unwrap().to_path_buf();
    let untouched = fs::metadata(&config_home).unwrap().modified().unwrap();

    let mut child = std_cmd(root)
        .args(["switch", "1", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Taking and releasing the refresh lock changes its directory's mtime: by then the switch
    // is in the wait, holding the mutation and account locks.
    wait_until(
        &mut child,
        Duration::from_secs(20),
        "Claude Code's lock",
        || fs::metadata(&config_home).unwrap().modified().unwrap() != untouched,
    );
    let signalled = Instant::now();
    send(&child, libc::SIGINT);
    let out = finish(child, Duration::from_secs(8));

    assert!(
        signalled.elapsed() < Duration::from_secs(2),
        "stopped at the wait's next attempt, not at its 9 s timeout: {:?}",
        signalled.elapsed()
    );
    assert_eq!(
        out.status.code(),
        Some(130),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "interrupted", "message": "interrupted"}})
    );
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert!(
        paths.legacy_lock().is_dir(),
        "Claude Code's lock is left alone"
    );
    assert!(
        !paths.refresh_lock.exists() && !paths.config_lock.exists(),
        "no lock directory of tagteam's is left behind"
    );
    assert_eq!(live_email(root), "b@x.co");
    assert_eq!(live_entry(root), entry, "the live credential is untouched");
    let provider = ProviderId::new(CLAUDE_CODE);
    let s = store(root);
    assert!(
        s.journal(&provider).unwrap().is_none(),
        "the switch never journaled"
    );
    assert_eq!(
        s.active(&provider).unwrap(),
        Some(AccountId::from_string(&b))
    );
}

#[test]
fn ctrl_c_twice_inside_a_switch_s_critical_span_lets_it_commit_and_reports_it_too_late() {
    // Review Focus 2. The switch is parked right after its journal row (§9.4 step 6), before it
    // writes the live credential. Two SIGINTs land there; it then writes and commits.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (a, _b) = two_fresh_accounts(root);
    let pause = root.join("pause");
    fs::create_dir(&pause).unwrap();

    let mut child = std_cmd(root)
        .args(["switch", "1", "--json"])
        .env("TAGTEAM_TEST_PAUSE_AT", "after-journal")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_until(
        &mut child,
        Duration::from_secs(20),
        "its journal row",
        || pause.join("paused").exists(),
    );
    send(&child, libc::SIGINT);
    thread::sleep(Duration::from_millis(100));
    send(&child, libc::SIGINT);
    thread::sleep(Duration::from_millis(100));
    assert!(
        child.try_wait().unwrap().is_none(),
        "neither Ctrl-C stopped the switch midway"
    );
    fs::write(pause.join("resume"), b"").unwrap();
    let out = finish(child, Duration::from_secs(20));

    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (v["switched"].clone(), v["reason"].clone()),
        (json!(true), json!("switched")),
        "{v}"
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "tagteam: interrupted too late to stop: switch had already finished\n"
    );
    assert_eq!(live_email(root), "a@x.co");
    let paths = CcPaths::resolve(&Env::for_test(root));
    assert!(
        !paths.refresh_lock.exists()
            && !paths.legacy_lock().exists()
            && !paths.config_lock.exists()
    );
    let provider = ProviderId::new(CLAUDE_CODE);
    let s = store(root);
    assert!(
        s.journal(&provider).unwrap().is_none(),
        "the switch committed: no journal row remains"
    );
    assert_eq!(
        s.active(&provider).unwrap(),
        Some(AccountId::from_string(&a))
    );
}
```

- [ ] **Step 12: Run the tests to verify they fail**

Run: `cargo test -p tagteam --features test-support --test signals`
Expected: FAIL, both tests.
- `ctrl_c_while_a_switch_waits_for_claude_code_s_lock_exits_130_with_nothing_written`: the
  status is `None`, not `Some(130)`. With no handler installed, SIGINT kills the binary.
- `ctrl_c_twice_inside_a_switch_s_critical_span_lets_it_commit_and_reports_it_too_late`: panics
  with "tagteam exited (exit status: 0) before its journal row". There is no pause point yet, so
  the switch runs straight through.

- [ ] **Step 13: Add `signal-hook`, install the handlers, and add the pause point**

In the workspace `Cargo.toml`, replace:

```toml
sha2 = "0.10"
thiserror = "2"
```

with:

```toml
sha2 = "0.10"
# §14.1, Decision 2: SIGINT, SIGTERM and SIGHUP only set the cancel token; `flag` needs no
# feature.
signal-hook = { version = "0.3", default-features = false }
thiserror = "2"
```

In `crates/tagteam/Cargo.toml`, replace:

```toml
serde_json.workspace = true
tracing.workspace = true
```

with:

```toml
serde_json.workspace = true
signal-hook.workspace = true
tracing.workspace = true
```

Create `crates/tagteam/src/signals.rs`:

```rust
//! §14.1: SIGINT, SIGTERM and SIGHUP never stop a command where they land. Each only records
//! its number in the cancel token, and the work stops at its next cancellation point. SIGKILL
//! and SIGQUIT keep their default disposition.

use tagteam_provider::Cancel;

/// The signals a user sends to stop a command (§14.1).
const CAUGHT: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

/// Registers SIGINT, SIGTERM and SIGHUP to store their number in `cancel`'s cell (Decision 2).
/// `signal-hook`'s handler is async-signal-safe and restarts interrupted syscalls, so every
/// cancellation point polls the token rather than waiting for `EINTR`.
pub fn install(cancel: &Cancel) -> std::io::Result<()> {
    for signal in CAUGHT {
        signal_hook::flag::register_usize(signal, cancel.cell(), signal as usize)?;
    }
    Ok(())
}
```

In `crates/tagteam/src/lib.rs`, replace:

```rust
mod root_guard;
mod statusline;
```

with:

```rust
mod root_guard;
mod signals;
mod statusline;
```

and replace:

```rust
    let ctx = app::Context::from_process();
    let mut prompter = prompt::TtyPrompter::new(ctx.env.cancel.clone());
```

with:

```rust
    let ctx = app::Context::from_process();
    // §14.1, at the process boundary like the drain above: in-process tests drive `run` with
    // tokens of their own, and must never change the test runner's signal dispositions. After
    // the drain, so a status bar command stuck on a pipe that never closes still dies on
    // SIGTERM.
    if let Err(e) = signals::install(&ctx.env.cancel) {
        let _ = writeln!(
            std::io::stderr(),
            "warning: could not catch signals, so one stops tagteam where it lands: {e}"
        );
    }
    let mut prompter = prompt::TtyPrompter::new(ctx.env.cancel.clone());
```

In `crates/tagteam-engine/src/hooks.rs`, replace:

```rust
/// A named point in the switch transaction. With the `test-hooks` feature, the environment
/// variable `TAGTEAM_TEST_CRASH_AT=<name>` ends the process there as a kill would: no
/// destructor runs, so no lock guard and no rollback (the kill tests). `Engine::fail_at`
/// injects an error there (`"<name>"`) or a panic (`"panic:<name>"`) for the rollback tests;
/// `TAGTEAM_TEST_FAIL_AT=<name>` injects the error for a process whose engine a test cannot
/// reach (the real binary).
/// Without the feature, a no-op.
#[cfg(feature = "test-hooks")]
pub(crate) fn point(engine: &Engine, name: &'static str) -> Result<(), EngineError> {
    if std::env::var("TAGTEAM_TEST_CRASH_AT").as_deref() == Ok(name) {
        std::process::exit(137);
    }
```

with:

```rust
/// A named point in the switch transaction. With the `test-hooks` feature, the environment
/// variable `TAGTEAM_TEST_CRASH_AT=<name>` ends the process there as a kill would: no
/// destructor runs, so no lock guard and no rollback (the kill tests).
/// `TAGTEAM_TEST_PAUSE_AT=<name>` parks it there instead until the test lets it go (`pause`),
/// so a binary test can signal it at a known point (§14.1). `Engine::fail_at` injects an
/// error there (`"<name>"`) or a panic (`"panic:<name>"`) for the rollback tests;
/// `TAGTEAM_TEST_FAIL_AT=<name>` injects the error for a process whose engine a test cannot
/// reach (the real binary).
/// Without the feature, a no-op.
#[cfg(feature = "test-hooks")]
pub(crate) fn point(engine: &Engine, name: &'static str) -> Result<(), EngineError> {
    if std::env::var("TAGTEAM_TEST_CRASH_AT").as_deref() == Ok(name) {
        std::process::exit(137);
    }
    if std::env::var("TAGTEAM_TEST_PAUSE_AT").as_deref() == Ok(name) {
        pause();
    }
```

and, directly after that function's closing brace (above the `#[cfg(not(feature = "test-hooks"))]`
no-op), add:

```rust
/// Parks the process at a point. It creates `paused` in `TAGTEAM_TEST_PAUSE_DIR`, then waits
/// for the test to create `resume` there, for at most 30 s so that a broken test cannot leave
/// it running. It never looks at the cancel token, so a signal sent meanwhile meets the work
/// after the point exactly as it would have met it there.
#[cfg(feature = "test-hooks")]
fn pause() {
    let Some(dir) = std::env::var_os("TAGTEAM_TEST_PAUSE_DIR").map(std::path::PathBuf::from) else {
        return;
    };
    let _ = std::fs::write(dir.join("paused"), b"");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !dir.join("resume").exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
```

- [ ] **Step 14: Run the tests to verify they pass**

Run: `cargo test -p tagteam --features test-support --test signals`
Expected: PASS, both tests. The first ends within about half a second of the SIGINT.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS. The pause point acts only with `TAGTEAM_TEST_PAUSE_AT` set.

- [ ] **Step 15: Run the wider checks**

Run:
```
cargo test --workspace --features tagteam/test-support
cargo test --workspace
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected:
- Both test runs PASS. `kill.rs` still sees exit 137 at its crash points, because
  `TAGTEAM_TEST_CRASH_AT` exits before any handler matters. No other binary test sends a signal.
  Part A's seven pty tests need a real pseudo-terminal: where `/dev/ptmx` is blocked they, and
  only they, fail loudly.
- `fmt --check` prints nothing.
- Both clippy runs finish with no warnings.

- [ ] **Step 16: Commit**

```bash
git add Cargo.toml Cargo.lock crates/tagteam/Cargo.toml crates/tagteam/src/signals.rs \
  crates/tagteam/src/lib.rs crates/tagteam-engine/src/hooks.rs crates/tagteam/tests/signals.rs
git commit -m "Catch SIGINT, SIGTERM and SIGHUP into the cancel token"
```

---

### Task 7: The Keychain file-mode pin lasts one operation (L396)

The amended spec, Appendix A.3: "If the item cannot be verified absent, the switch rolls
back. File mode stays pinned until the operation that fell back (one switch or one recovery)
ends, so its later writes go to the file too, and a rollback clears the pin. The next
operation tries the Keychain again, which matters for a long-lived process such as `auto` or
the daemon."

This is M1's carried item L396 ("file mode pinned before commit and kept after rollback",
CAN WAIT until M3).

Today `LiveStore.file_mode_pinned` is an `AtomicBool` on the provider, so it lives as long as
the process. `write_credential_entry` sets it after a verified fallback, before the switch
commits, and nothing ever clears it:
- a rolled-back switch leaves it set;
- every later write of the process goes straight to `.credentials.json`;
- `doomed` treats every fallback-only Keychain item as surely destroyed.

For a one-shot command this is invisible. For M3b's `auto`, one Keychain hiccup would leave
the live credential in the plaintext file for as long as the loop runs.

**Readings of the spec this task commits to:**
- **One operation is one hold of CC's credential locks.** That is the span the engine already
  treats as one unit of live writes:
  - a switch's attempt that reaches the transaction (§9.4). Earlier attempts release their
    locks and plan again before writing anything, so "one switch" is that attempt's hold;
  - one recovery of one journal row (§9.6);
  - one §7.5 pass (`refresh_active` takes the credential locks per pass, and `publish` adds
    the config lock);
  - one `add`.

  The spec names switch and recovery. §7.5's publish is the other path that writes the
  credential entry (`active.rs` `publish`), and it falls under the same rule.
- **The pin is tied to the operation's live locks, from the provider's side.**
  `ClaudeCode::lock_credentials` wraps CC's credential-lock set in `OperationLocks`. It holds
  the set and the `Arc<LiveStore>`, and its `Drop` clears the pin. Every `LiveLocks` CC hands
  out owns one, inside its `CredLocks`. Dropping the operation's locks therefore ends the pin,
  on every path: commit, error, `?` return and unwinding. The engine is unchanged, and the
  provider's write path never needs the locks for this. Alternatives considered:
  - *A flag on `tagteam_provider::LiveLocks`.* Rejected. It puts a CC-only concept (a
    Keychain/file split) into the provider-generic type, and it would thread the locks
    through `LiveStore`'s API and every one of its tests.
  - *An explicit reset the engine calls at the end of every operation.* Rejected. It needs a
    new `Provider` method and a call on every exit path of `switch`, `recover_one`,
    `refresh_active` and `add_live`. A missed path silently brings L396 back.
  - *Clearing the pin when the next operation acquires its locks.* Equivalent for every
    reader, but the pin would then outlive its operation, which contradicts "until the
    operation … ends".
- **"Its later writes go to the file too"** governs:
  - every further `write_credential` under the same `LiveLocks`;
  - `doomed` read under them, which then names every Keychain item as surely destroyed
    (`on_fallback: false`).

  No engine operation writes the credential entry twice today, since a switch writes it once.
  This rule is therefore pinned at the provider level, where one `LiveLocks` is one operation:
  `a_fallback_pins_the_file_for_the_rest_of_its_operation`, and the reworked pinned case of
  `doomed_names_everything_each_change_destroys`.
- **A rollback clears the pin.** `LiveStore::restore` clears it before restoring anything,
  whether or not the restore then succeeds. The items a restore puts back are what CC reads
  first, so a later write of the same operation that stayed on the file would sit behind a
  restored item. Clearing is always safe: a Keychain-first write either lands in the Keychain,
  which is authoritative, or falls back again with the full delete-and-verify. Every rollback
  reaches `restore`:
  - the engine's `Rollback` runs the `SnapshotUndo`;
  - `guarded` runs its own restore when a write fails part-way;
  - `Armed` restores while unwinding.
- **Recovery never sets the pin.** It clears the other axis and splices the identity, and
  neither falls back. It takes CC's credential locks like any operation, so it never inherits
  one either.
- **A pin whose operation commits** lasts until that operation's locks are released. The
  switch's identity splice and commit happen under it. `SecretStore::Fallback`'s doc changes
  from "an earlier one in this process" to "an earlier one of the same operation".
- **"Pinned before commit" (L396's first half) stays, on purpose.** The spec scopes the pin to
  the operation, not to its commit, so the write that falls back still sets it before the
  commit. L396's actual harm is the second half, the pin outliving a rollback and the
  process, and that is what this task removes.

**Files:**
- Modify: `crates/tagteam-cc/src/live.rs`:
  - the `file_mode_pinned` field's doc;
  - new `pub(crate) fn unpin_file_mode`;
  - the `write_credential_entry` doc;
  - `restore`.
- Modify: `crates/tagteam-cc/src/provider.rs` (imports, new private `OperationLocks`,
  `lock_credentials`)
- Modify: `crates/tagteam-provider/src/provider.rs` (the `SecretStore::Fallback` doc line only)
- Modify: `crates/tagteam-cc/tests/provider.rs`
- Modify: `crates/tagteam-engine/tests/switch.rs`
- Modify: `crates/tagteam-engine/tests/switch_rollback.rs`

**Interfaces:**
- Consumes:
  - Task 2: `ClaudeCode::lock_credentials` as Task 2 left it. Its first line calls
    `locks::acquire_credentials(&CcPaths::resolve(env), budget, &env.cancel)`, and this task
    changes only the line after it.
  - M1:
    - `LiveStore`, `LiveStore::{file_mode_pinned, write_credential_entry, restore}`;
    - `locks::CcCredSet`, `CredLocks::new`, `LiveLockSet`, `LockError`, `Undo::undo`;
    - `FakeKeychain::{put, get, set_fail_write}`;
    - `Engine::{fail_at, on_point}` (feature `test-hooks`) and the `after-credential` hook
      point;
    - the engine fixture `Fx` (`add`, `live_item`, `paths`, `live_credential`,
      `live_refresh_token`, `switch_to`, `kc`) and `switch.rs`'s `switch`/`to` helpers.
- Produces:
  - `pub(crate) fn LiveStore::unpin_file_mode(&self)`.
  - A private `struct OperationLocks { set: locks::CcCredSet, live: Arc<LiveStore> }`
    implementing `LiveLockSet`, whose `Drop` calls `unpin_file_mode`.
  - Behaviour: the pin ends when the operation's credential locks are released, and on any
    `restore`. No public signature changes, and `LiveStore::file_mode_pinned()` stays.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-cc/tests/provider.rs`, add `LiveLocks` to the `use tagteam_provider::{…};`
block. As the block stands, it becomes:

```rust
use tagteam_provider::{
    Capabilities, Credential, Env, FakeKeychain, KindTraits, LiveLocks, LockError, MutationGuard,
    Pace, PollBudget, Provider, ProviderError, Read, SecretStore, StoredLogin, UsageResult, Window,
};
```

Rework `doomed_names_everything_each_change_destroys` so its pinned case pins and checks
inside one operation. Today it pins under one set of locks, drops them, and checks under new
ones. Once the pin lasts one operation, that case would silently become the healthy-Keychain
case. Replace:

```rust
    // (change, how the Keychain treats the write: 0 takes it, 1 refuses it, 2 was pinned to
    // the file by an earlier refusal)
```

with:

```rust
    // (change, how the Keychain treats the write: 0 takes it, 1 refuses it, 2 was pinned to
    // the file by an earlier refusal in the same operation)
```

Then replace the loop and the head of `check`, from:

```rust
        let g = MutationGuard::acquire(&env, Duration::from_secs(1)).unwrap();
        let locks = f.cc.lock_live(&env, &g).unwrap();
        let primary = |kind| keychain_service(&env, kind);
        if keychain == 2 {
            f.kc.set_fail_write(&primary(ItemKind::OAuth), true);
            f.cc.write_credential(
                &env,
                &locks,
                &target(&f, "p@x.co", "rt-p"),
                &mut save_nothing,
            )
            .unwrap();
            f.kc.set_fail_write(&primary(ItemKind::OAuth), false);
            drop(locks);
            drop(g);
            plant_every_entry(&f);
            check(&f, &env, change, keychain, api_key);
            continue;
        }
        drop(locks);
        drop(g);
        check(&f, &env, change, keychain, api_key);
    }

    fn check(f: &Fx, env: &Env, change: LiveChange, keychain: u8, api_key: &str) {
        let g = MutationGuard::acquire(env, Duration::from_secs(1)).unwrap();
        let locks = f.cc.lock_live(env, &g).unwrap();
        let before = secrets_by_place(f, env);
        let doomed = f.cc.doomed(env, &locks, change);
```

to:

```rust
        let g = MutationGuard::acquire(&env, Duration::from_secs(1)).unwrap();
        let locks = f.cc.lock_live(&env, &g).unwrap();
        if keychain == 2 {
            // The pin lasts one operation (Appendix A.3), so the write that sets it runs under
            // the same live locks as the change being checked.
            let primary = keychain_service(&env, ItemKind::OAuth);
            f.kc.set_fail_write(&primary, true);
            f.cc.write_credential(
                &env,
                &locks,
                &target(&f, "p@x.co", "rt-p"),
                &mut save_nothing,
            )
            .unwrap();
            f.kc.set_fail_write(&primary, false);
            plant_every_entry(&f);
        }
        check(&f, &env, &locks, change, keychain, api_key);
    }

    fn check(
        f: &Fx,
        env: &Env,
        locks: &LiveLocks<'_>,
        change: LiveChange,
        keychain: u8,
        api_key: &str,
    ) {
        let before = secrets_by_place(f, env);
        let doomed = f.cc.doomed(env, locks, change);
```

In the rest of `check`, replace

```rust
                f.cc.write_credential(env, &locks, &login, &mut record)
                    .unwrap();
```

with

```rust
                f.cc.write_credential(env, locks, &login, &mut record)
                    .unwrap();
```

and

```rust
                f.cc.clear_other_axis(env, &locks, kind).unwrap();
```

with

```rust
                f.cc.clear_other_axis(env, locks, kind).unwrap();
```

Below `check`'s last assertion, which reads:

```rust
        if !refused && !pinned {
            assert!(reported.is_empty(), "{case}: nothing falls back");
        }
```

add the assertion that keeps the pinned case from degrading unnoticed:

```rust
        if pinned {
            assert!(
                !reported.is_empty(),
                "{case}: a pinned write goes to the file and reports the items it deletes"
            );
        }
```

Then append to the end of `crates/tagteam-cc/tests/provider.rs`:

```rust
/// A live OAuth login in the Keychain, as CC leaves it; returns the item's (service, account).
fn keychain_login(f: &Fx) -> (String, String) {
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    let acct = keychain_account(&f.env);
    f.kc.put(
        &svc,
        &acct,
        json!({"claudeAiOauth": {"refreshToken": "rt-live"}})
            .to_string()
            .as_bytes(),
    );
    (svc, acct)
}

#[test]
fn a_fallback_pins_the_file_for_the_rest_of_its_operation() {
    // Appendix A.3: once a write falls back, every later write under the same live locks goes
    // to the file too, even with the Keychain healthy again.
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    let (svc, acct) = keychain_login(&f);
    let fell_back = SecretStore::Fallback(paths.credentials_file.clone());
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    f.kc.set_fail_write(&svc, true);
    let first =
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "a@b.co", "rt-1"),
            &mut save_nothing,
        )
        .unwrap();
    assert_eq!(first.stored_in, fell_back);
    f.kc.set_fail_write(&svc, false);
    let second =
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "a@b.co", "rt-2"),
            &mut save_nothing,
        )
        .unwrap();
    assert_eq!(second.stored_in, fell_back);
    assert_eq!(
        f.kc.get(&svc, &acct),
        None,
        "the Keychain item stays deleted"
    );
    let file: Value = serde_json::from_slice(&fs::read(&paths.credentials_file).unwrap()).unwrap();
    assert_eq!(file["claudeAiOauth"]["refreshToken"], json!("rt-2"));
}

#[test]
fn the_next_operation_tries_the_keychain_again() {
    // Appendix A.3 (L396): releasing the live locks ends the operation that fell back, and
    // with it the pin. A long-lived process (`auto`, the daemon) must not stay on the file.
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    let (svc, acct) = keychain_login(&f);
    {
        let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
        let locks = f.cc.lock_live(&f.env, &g).unwrap();
        f.kc.set_fail_write(&svc, true);
        let written =
            f.cc.write_credential(
                &f.env,
                &locks,
                &target(&f, "a@b.co", "rt-1"),
                &mut save_nothing,
            )
            .unwrap();
        assert_eq!(
            written.stored_in,
            SecretStore::Fallback(paths.credentials_file.clone())
        );
    }
    f.kc.set_fail_write(&svc, false);
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let written =
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "b@b.co", "rt-2"),
            &mut save_nothing,
        )
        .unwrap();
    assert_eq!(written.stored_in, SecretStore::Keychain);
    let item = f.kc.get(&svc, &acct).unwrap();
    assert_eq!(
        oauth_item(&f).unwrap()["claudeAiOauth"]["refreshToken"],
        json!("rt-2")
    );
    assert_eq!(
        fs::read(&paths.credentials_file).unwrap(),
        item,
        "the file the fallback created is rewritten with the item's bytes, for hot reload"
    );
}

#[test]
fn a_rollback_clears_the_pin_within_its_operation() {
    // Appendix A.3: the rollback puts the Keychain item back, so a later write of the same
    // operation must not stay on the file, where the restored item would shadow it.
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    let (svc, acct) = keychain_login(&f);
    let before = f.kc.get(&svc, &acct).unwrap();
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    f.kc.set_fail_write(&svc, true);
    let written =
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "a@b.co", "rt-1"),
            &mut save_nothing,
        )
        .unwrap();
    assert_eq!(
        written.stored_in,
        SecretStore::Fallback(paths.credentials_file.clone())
    );
    f.kc.set_fail_write(&svc, false);
    written.undo.undo(&locks).unwrap();
    assert_eq!(
        f.kc.get(&svc, &acct).unwrap(),
        before,
        "the rollback put the item back"
    );
    assert!(
        !paths.credentials_file.exists(),
        "and removed the fallback's file"
    );
    let again =
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "a@b.co", "rt-2"),
            &mut save_nothing,
        )
        .unwrap();
    assert_eq!(again.stored_in, SecretStore::Keychain);
    assert_eq!(
        oauth_item(&f).unwrap()["claudeAiOauth"]["refreshToken"],
        json!("rt-2")
    );
    assert!(
        !paths.credentials_file.exists(),
        "a Keychain write never creates the file"
    );
}
```

In `crates/tagteam-engine/tests/switch.rs`, the comment on
`a_file_pin_from_an_earlier_switch_never_mislabels_a_later_api_key_switch` describes the
process-wide pin. Replace:

```rust
    // One process: the OAuth fallback pins the credential entry to the file for the rest of
    // it, but an API key the Keychain then takes was stored in the Keychain, and says so.
```

with:

```rust
    // An OAuth fallback in one switch never mislabels a later API-key switch: the key the
    // Keychain takes was stored in the Keychain, and says so.
```

and add this test directly after that test:

```rust
#[test]
fn the_switch_after_a_fallback_tries_the_keychain_again() {
    // Appendix A.3 (L396): file mode lasts only as long as the switch that fell back, so a
    // long-lived process (M3b's `auto`) is not left on the file for good.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: b, in the Keychain
    let (oauth_item, acct) = fx.live_item(ItemKind::OAuth);
    fx.kc.set_fail_write(&oauth_item, true);
    assert_eq!(
        switch(&fx, to(&a), false).unwrap().stored_in,
        Some(SecretStore::Fallback(fx.paths().credentials_file))
    );
    assert_eq!(
        fx.kc.get(&oauth_item, &acct),
        None,
        "the fallback deleted the item"
    );
    fx.kc.set_fail_write(&oauth_item, false);
    assert_eq!(
        switch(&fx, to(&b), false).unwrap().stored_in,
        Some(SecretStore::Keychain)
    );
    let item = fx.kc.get(&oauth_item, &acct).unwrap();
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
    // The file the fallback created now mirrors the item, rewritten for CC's hot reload.
    assert_eq!(fs::read(fx.paths().credentials_file).unwrap(), item);
}
```

In `crates/tagteam-engine/tests/switch_rollback.rs`, replace

```rust
use tagteam_provider::{Credential, Env, Identity, ProcessStamp, Provider};
```

with

```rust
use tagteam_provider::{Credential, Env, Identity, ProcessStamp, Provider, SecretStore};
```

and append to the end of the file:

```rust
#[test]
fn a_rolled_back_fallback_leaves_the_next_switch_on_the_keychain() {
    // Appendix A.3 (L396): a rollback clears the file-mode pin, so the fallback a failed
    // switch took never carries over to the next one.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b, in the Keychain
    let cred_before = fx.live_credential();
    let svc = keychain_service(&fx.env, ItemKind::OAuth);
    fx.kc.set_fail_write(&svc, true);
    // The write falls back; the Keychain takes writes again by the time the switch fails, so
    // the rollback can put b's item back.
    let (kc, healed) = (fx.kc.clone(), svc.clone());
    fx.engine.on_point(
        "after-credential",
        Box::new(move || kc.set_fail_write(&healed, false)),
    );
    fx.engine.fail_at(Some("after-credential"));
    let err = fx.switch_to(&a, false).unwrap_err();
    assert!(matches!(err, EngineError::RolledBack(_)), "{err}");
    assert_eq!(fx.live_credential(), cred_before);
    assert!(
        !fx.paths().credentials_file.exists(),
        "the rollback removed the fallback's file"
    );

    fx.engine.fail_at(None);
    let out = fx.switch_to(&a, false).unwrap();
    assert_eq!(out.stored_in, Some(SecretStore::Keychain));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert!(
        !fx.paths().credentials_file.exists(),
        "a Keychain write never creates the file"
    );
}
```

`hooks::point` runs the `on_point` callback before it checks `fail_at`, so the Keychain is
healed before the injected failure starts the rollback. The callback touches only the fake
keychain, never this engine, so it cannot deadlock on the hook mutex.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-cc --test provider`

Expected: FAIL, two tests, both at their `stored_in` assertion, because today's pin outlives
the locks and the rollback:
- `the_next_operation_tries_the_keychain_again`: `left: Fallback(".../home/.claude/.credentials.json")`,
  `right: Keychain`.
- `a_rollback_clears_the_pin_within_its_operation`: the same. Its earlier assertions pass,
  because the restore already puts the item back and removes the file; only the pin survives
  it.

`a_fallback_pins_the_file_for_the_rest_of_its_operation` and the reworked
`doomed_names_everything_each_change_destroys` already pass, since a process-wide pin also
lasts the operation. Both must keep passing.

Run: `cargo test -p tagteam-engine --test switch the_switch_after_a_fallback_tries_the_keychain_again`

Expected: FAIL at the second switch: `left: Some(Fallback(".../.credentials.json"))`,
`right: Some(Keychain)`.

Run: `cargo test -p tagteam-engine --features test-hooks --test switch_rollback a_rolled_back_fallback_leaves_the_next_switch_on_the_keychain`

Expected: FAIL at the second switch's `stored_in`, with the same left and right. The rollback
assertions before it pass.

- [ ] **Step 3: Scope the pin to the operation**

In `crates/tagteam-cc/src/live.rs`, document the field. Replace:

```rust
pub struct LiveStore {
    keychain: Arc<dyn Keychain>,
    platform: Platform,
    file_mode_pinned: AtomicBool,
    retry_delay: Duration,
}
```

with:

```rust
pub struct LiveStore {
    keychain: Arc<dyn Keychain>,
    platform: Platform,
    /// Appendix A.3: a credential-entry write of the current operation fell back to the file,
    /// so the operation's later writes go there too. Cleared when the operation's credential
    /// locks are released (`ClaudeCode::lock_credentials`) and by `restore`.
    file_mode_pinned: AtomicBool,
    retry_delay: Duration,
}
```

Add the clearing method directly after `file_mode_pinned`. Replace:

```rust
    pub fn file_mode_pinned(&self) -> bool {
        self.file_mode_pinned.load(Ordering::SeqCst)
    }
```

with:

```rust
    pub fn file_mode_pinned(&self) -> bool {
        self.file_mode_pinned.load(Ordering::SeqCst)
    }

    /// Ends the file-mode pin: the next credential-entry write tries the Keychain again
    /// (Appendix A.3).
    pub(crate) fn unpin_file_mode(&self) {
        self.file_mode_pinned.store(false, Ordering::SeqCst);
    }
```

Say what the pin does on `write_credential_entry`. Replace its doc comment:

```rust
    /// Appendix A.3 write, including the verified file fallback, which first reports every
    /// item it will delete to `before_fallback`. Returns where this write put the credential:
    /// a file mirrored for hot reload does not make it a file store.
```

with:

```rust
    /// Appendix A.3 write, including the verified file fallback, which first reports every
    /// item it will delete to `before_fallback`. A fallback pins file mode, so every later
    /// write of the same operation goes straight to the file. Returns where this write put the
    /// credential: a file mirrored for hot reload does not make it a file store.
```

The body of `write_credential_entry` is unchanged. It still pins with
`self.file_mode_pinned.store(true, Ordering::SeqCst)` once `remove_items` has verified every
item gone.

A rollback clears the pin. In `restore`, replace its first three lines:

```rust
        let acct = keychain_account(env);
        let mut failed: Vec<String> = Vec::new();
        let global_config_name = paths.global_config.display().to_string();
```

with:

```rust
        // A rollback clears the pin (Appendix A.3): the items put back below are what CC reads
        // first, so a later write of this operation that stayed on the file would sit behind
        // them. Trying the Keychain first is always safe; a refusal falls back again.
        self.unpin_file_mode();
        let acct = keychain_account(env);
        let mut failed: Vec<String> = Vec::new();
        let global_config_name = paths.global_config.display().to_string();
```

In `crates/tagteam-cc/src/provider.rs`, add `LiveLockSet` and `LockError` to the
`use tagteam_provider::{…};` block. As the block stands, replace:

```rust
use tagteam_provider::{
    BeforeFallback, Capabilities, CredLocks, Credential, DoomedEntry, Env, FreshCredential,
    Identity, IdentitySurface, Keychain, KindTraits, LiveAuth, LiveChange, LiveLocks,
    MutationGuard, Pace, PollBudget, Provider, ProviderError, Read, StoredLogin, Undo, UsageResult,
    Window, Written,
};
```

with:

```rust
use tagteam_provider::{
    BeforeFallback, Capabilities, CredLocks, Credential, DoomedEntry, Env, FreshCredential,
    Identity, IdentitySurface, Keychain, KindTraits, LiveAuth, LiveChange, LiveLockSet, LiveLocks,
    LockError, MutationGuard, Pace, PollBudget, Provider, ProviderError, Read, StoredLogin, Undo,
    UsageResult, Window, Written,
};
```

Insert the operation's lock set directly above `fence_of`, that is, above the line
`/// The ownership fence every protected write and restore checks immediately before mutating`:

```rust
/// CC's credential locks, held for one operation: a switch, a recovery, a §7.5 pass or an
/// `add`. Releasing them ends the operation, and with it the Keychain file-mode pin (Appendix
/// A.3), so the next operation tries the Keychain again.
struct OperationLocks {
    set: locks::CcCredSet,
    live: Arc<LiveStore>,
}

impl LiveLockSet for OperationLocks {
    fn check_owned(&self) -> Result<(), LockError> {
        self.set.check_owned()
    }
}

impl Drop for OperationLocks {
    fn drop(&mut self) {
        // Runs before `set` is dropped, so the pin ends while the locks are still held.
        self.live.unpin_file_mode();
    }
}
```

In `lock_credentials`, keep its first line as Task 2 left it, and replace the line after it:

```rust
        Ok(CredLocks::new(g, Box::new(set)))
```

with:

```rust
        Ok(CredLocks::new(
            g,
            Box::new(OperationLocks {
                set,
                live: self.live.clone(),
            }),
        ))
```

(`lock_config` is unchanged: `cred.with_config(…)` moves the `OperationLocks` into the
`LiveLocks`, and it drops last, after the config lock.)

In `crates/tagteam-provider/src/provider.rs`, replace the doc line of `SecretStore::Fallback`:

```rust
    /// A file, because the keychain refused this write or an earlier one in this process.
```

with:

```rust
    /// A file, because the keychain refused this write or an earlier one of the same operation
    /// (Appendix A.3).
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-cc`

Expected: PASS, including the three new provider tests and the reworked
`doomed_names_everything_each_change_destroys`. The `live_store.rs` pin tests are unchanged
and still pass:
- `file_fallback_requires_the_shadowing_item_to_be_gone`;
- `a_failed_fallback_never_pins_the_file_mode`;
- `a_counting_fence_stops_the_deletes_in_remove_items`.

They drive `LiveStore` directly, under no locks, so one store is one operation.

Run: `cargo test -p tagteam-engine --test switch`

Expected: PASS, including `the_switch_after_a_fallback_tries_the_keychain_again`.

Run: `cargo test -p tagteam-engine --features test-hooks`

Expected: PASS. This includes `a_rolled_back_fallback_leaves_the_next_switch_on_the_keychain`
and the existing rollback, recovery and §15.3 invariant tests: the pin's scope touches no file
outside CC's live entries.

- [ ] **Step 5: Run the wider checks**

Run:
```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --features tagteam/test-support
```

Expected:
- No output from `fmt --check`.
- Both clippy runs finish with no warnings.
- Every test passes, including the CLI's
  `an_oauth_write_the_keychain_refuses_is_reported_where_it_went`, whose third switch already
  expects `credentialStore: "keychain"` once the Keychain takes writes again.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-cc/src/live.rs crates/tagteam-cc/src/provider.rs \
  crates/tagteam-provider/src/provider.rs crates/tagteam-cc/tests/provider.rs \
  crates/tagteam-engine/tests/switch.rs crates/tagteam-engine/tests/switch_rollback.rs
git commit -m "Scope the Keychain file-mode pin to one operation"
```

---

### Task 8: Usage ranking in core

§9.3, the two strategies that rank by usage:

> `switch --strategy next-available`: the rotation walk, skipping candidates whose known headroom
> is ≤ 0 (the message names each one's binding window). Unknown headroom is never skipped
> (§8.2). If the walk skips every candidate: `candidates-exhausted`.
>
> `switch --strategy best`: the candidate with the most known headroom, ties to the lower
> position. Switch only if it has strictly more headroom than the live account; otherwise
> `already-best`. If the live account's headroom is unknown, or there is no live login, switch
> to it with a warning. No candidate with known headroom: `usage-unavailable`.

And §8.2: "The relevant windows for decisions are 5h and 7d, plus any scoped windows whose
names match `autoswitch.models` case-insensitively (`all` matches every scoped window). Spend
never counts. Headroom is `100 − max(relevant pct)`." The binding window is the relevant window
whose pct sets that max.

This task is the pure half (Decision 10). Given each candidate's decision-grade headroom, it
decides the order and the outcome. Task 9 reads the readings, collects, and walks the vaults.

**Readings of the spec this task commits to:**
- **Headroom can be negative.** A pct above 100 is kept (§8.2), so −4 is more headroom than −10,
  and a candidate at −4 beats a live account at −5. Only `next-available` treats ≤ 0 as "at the
  limit". `best` ranks by the number.
- **NaN is unknown.** `usage::headroom` never yields NaN, because §8.2 drops a non-finite pct.
  A NaN that reaches these functions anyway counts as unknown: never ranked, never skipped,
  never compared. Nothing here computes a NaN.
- **Ties are numeric equality**, so `0.0` and `-0.0` tie and go to the lower position. Positions
  are unique within a provider, so the order is total.
- **`best` with no known candidate is `UsageUnavailable` whatever the live headroom**, an empty
  slice included. Task 9 reports `only-one-account` or `no-valid-target` before it ranks, so an
  empty slice never reaches this function from the engine.
- **`next-available`'s `skipped` keeps the walk's order**, so the engine names skipped accounts
  in rotation order. An empty walk is `Try` with nothing in it, never `Exhausted`.
- **The binding window's ties go to the earlier window** (the contract). For Claude Code that is
  5h before 7d before the scoped windows, as normalization emits them.
- **`span`** produces `render::duration`'s exact outputs (CONTRACT ISSUES 1).
- **L302 needs nothing at this level** (NOTES 6).

**Files:**
- Create: `crates/tagteam-core/src/rank.rs`
- Modify: `crates/tagteam-core/src/lib.rs`

**Interfaces:**
- Consumes (M2b, `crates/tagteam-core/src/usage.rs`):
  - `pub struct Window { pub key: String, pub label: String, pub kind: WindowKind, pub pct: f64, pub resets_at: Option<i64>, pub period_s: Option<i64>, pub detail: Option<Value> }`
  - `pub fn is_relevant(w: &Window, models: &[String]) -> bool`
  - `pub fn headroom(windows: &[Window], models: &[String]) -> Option<f64>` (tests only)
- Produces (`tagteam_core::rank`):
  - `#[derive(Debug, Clone, Copy, PartialEq)] pub struct Candidate { pub position: u32, pub headroom: Option<f64> }`
  - `#[derive(Debug, Clone, PartialEq)] pub enum BestOrder { Try(Vec<u32>), AlreadyBest, UsageUnavailable }`
  - `pub fn best_order(live: Option<f64>, candidates: &[Candidate]) -> BestOrder`
  - `#[derive(Debug, Clone, PartialEq)] pub enum NextAvailable { Try { order: Vec<u32>, skipped: Vec<u32> }, Exhausted }`
  - `pub fn next_available(walk: &[Candidate]) -> NextAvailable`
  - `pub fn binding_window<'w>(windows: &'w [Window], models: &[String]) -> Option<&'w Window>`
  - `pub fn span(secs: i64) -> String` (CONTRACT ISSUES 1)

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-core/src/lib.rs`, add `pub mod rank;` between `pub mod poll;` and
`pub mod rotation;`:

```rust
pub mod poll;
pub mod rank;
pub mod rotation;
```

Create `crates/tagteam-core/src/rank.rs` with the module doc and its tests only:

```rust
//! §9.3's usage strategies, decided from each candidate's decision-grade headroom (§8.2,
//! §8.4). Pure: the engine reads the readings and hands in positions and headroom.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{WindowKind, headroom};

    fn c(position: u32, headroom: Option<f64>) -> Candidate {
        Candidate { position, headroom }
    }

    fn win(key: &str, label: &str, kind: WindowKind, pct: f64) -> Window {
        Window {
            key: key.into(),
            label: label.into(),
            kind,
            pct,
            resets_at: None,
            period_s: None,
            detail: None,
        }
    }

    fn models(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn key(w: Option<&Window>) -> Option<&str> {
        w.map(|w| w.key.as_str())
    }

    #[test]
    fn best_orders_the_known_candidates_by_most_headroom() {
        let candidates = [c(1, Some(20.0)), c(2, Some(60.0)), c(3, Some(40.0))];
        assert_eq!(
            best_order(Some(10.0), &candidates),
            BestOrder::Try(vec![2, 3, 1])
        );
    }

    #[test]
    fn best_breaks_ties_to_the_lower_position_whatever_the_input_order() {
        let candidates = [
            c(4, Some(50.0)),
            c(2, Some(50.0)),
            c(3, Some(70.0)),
            c(1, Some(50.0)),
        ];
        assert_eq!(
            best_order(None, &candidates),
            BestOrder::Try(vec![3, 1, 2, 4])
        );
        // Equal as numbers: -0.0 and 0.0 tie.
        assert_eq!(
            best_order(None, &[c(2, Some(0.0)), c(1, Some(-0.0))]),
            BestOrder::Try(vec![1, 2])
        );
    }

    #[test]
    fn best_keeps_only_candidates_strictly_better_than_the_live_account() {
        let candidates = [c(1, Some(30.0)), c(2, Some(30.5)), c(3, Some(29.0))];
        assert_eq!(best_order(Some(30.0), &candidates), BestOrder::Try(vec![2]));
    }

    #[test]
    fn an_unknown_live_headroom_orders_every_known_candidate() {
        let candidates = [c(1, Some(5.0)), c(2, None), c(3, Some(-3.0))];
        assert_eq!(best_order(None, &candidates), BestOrder::Try(vec![1, 3]));
    }

    #[test]
    fn no_known_candidate_is_usage_unavailable_whatever_the_live_headroom() {
        let unknown = [c(1, None), c(2, None)];
        assert_eq!(
            best_order(Some(50.0), &unknown),
            BestOrder::UsageUnavailable
        );
        assert_eq!(best_order(None, &unknown), BestOrder::UsageUnavailable);
        assert_eq!(best_order(Some(50.0), &[]), BestOrder::UsageUnavailable);
    }

    #[test]
    fn a_known_live_headroom_that_no_candidate_beats_is_already_best() {
        let candidates = [c(1, Some(40.0)), c(2, None), c(3, Some(10.0))];
        assert_eq!(
            best_order(Some(40.0), &candidates),
            BestOrder::AlreadyBest,
            "a tie does not beat it"
        );
        assert_eq!(best_order(Some(90.0), &candidates), BestOrder::AlreadyBest);
    }

    #[test]
    fn an_unknown_candidate_is_never_in_the_best_order() {
        let candidates = [c(1, None), c(2, Some(1.0)), c(3, None)];
        for live in [None, Some(0.0)] {
            assert_eq!(
                best_order(live, &candidates),
                BestOrder::Try(vec![2]),
                "{live:?}"
            );
        }
    }

    #[test]
    fn negative_headroom_ranks_below_zero_and_still_beats_a_worse_live_account() {
        let candidates = [c(1, Some(-4.0)), c(2, Some(-10.0)), c(3, Some(0.0))];
        assert_eq!(
            best_order(Some(-5.0), &candidates),
            BestOrder::Try(vec![3, 1])
        );
        assert_eq!(best_order(Some(0.0), &candidates), BestOrder::AlreadyBest);
        assert_eq!(best_order(None, &candidates), BestOrder::Try(vec![3, 1, 2]));
    }

    #[test]
    fn a_nan_headroom_reads_as_unknown_and_never_reaches_an_order() {
        let candidates = [c(1, Some(f64::NAN)), c(2, Some(10.0))];
        assert_eq!(
            best_order(Some(f64::NAN), &candidates),
            BestOrder::Try(vec![2]),
            "a NaN live headroom is unknown"
        );
        assert_eq!(
            best_order(Some(50.0), &[c(1, Some(f64::NAN))]),
            BestOrder::UsageUnavailable
        );
        assert_eq!(
            next_available(&[c(1, Some(f64::NAN))]),
            NextAvailable::Try {
                order: vec![1],
                skipped: vec![]
            },
            "never skipped either"
        );
    }

    #[test]
    fn next_available_skips_only_known_exhausted_candidates_and_keeps_the_walk_order() {
        let walk = [
            c(3, Some(0.0)),
            c(1, None),
            c(2, Some(-4.0)),
            c(5, Some(0.5)),
        ];
        assert_eq!(
            next_available(&walk),
            NextAvailable::Try {
                order: vec![1, 5],
                skipped: vec![3, 2]
            }
        );
    }

    #[test]
    fn next_available_is_exhausted_only_when_every_candidate_is_known_to_be() {
        assert_eq!(
            next_available(&[c(1, Some(0.0)), c(2, Some(-1.0))]),
            NextAvailable::Exhausted
        );
        assert_eq!(
            next_available(&[c(1, Some(0.0)), c(2, None)]),
            NextAvailable::Try {
                order: vec![2],
                skipped: vec![1]
            },
            "an unknown candidate is never skipped"
        );
    }

    #[test]
    fn an_empty_walk_is_an_empty_try_never_exhausted() {
        assert_eq!(
            next_available(&[]),
            NextAvailable::Try {
                order: vec![],
                skipped: vec![]
            }
        );
    }

    #[test]
    fn the_binding_window_is_the_relevant_one_with_the_highest_pct() {
        let windows = vec![
            win("5h", "5h", WindowKind::Short, 40.0),
            win("7d", "7d", WindowKind::Long, 77.0),
            win("spend", "spend", WindowKind::Spend, 99.0),
            win("scoped:Fable", "Fable", WindowKind::Scoped, 95.0),
        ];
        assert_eq!(
            key(binding_window(&windows, &[])),
            Some("7d"),
            "spend and an unnamed model window never bind"
        );
        assert_eq!(
            key(binding_window(&windows, &models(&["fable"]))),
            Some("scoped:Fable")
        );
        assert_eq!(
            key(binding_window(&windows, &models(&["all"]))),
            Some("scoped:Fable")
        );
        assert_eq!(
            key(binding_window(&windows, &models(&["opus"]))),
            Some("7d")
        );
    }

    #[test]
    fn a_tie_binds_the_earlier_window() {
        let windows = vec![
            win("5h", "5h", WindowKind::Short, 100.0),
            win("7d", "7d", WindowKind::Long, 100.0),
        ];
        assert_eq!(key(binding_window(&windows, &[])), Some("5h"));
    }

    #[test]
    fn no_relevant_window_binds_nothing() {
        assert_eq!(binding_window(&[], &[]), None);
        let irrelevant = vec![
            win("spend", "spend", WindowKind::Spend, 100.0),
            win("scoped:Fable", "Fable", WindowKind::Scoped, 100.0),
        ];
        assert_eq!(binding_window(&irrelevant, &[]), None);
    }

    #[test]
    fn the_binding_window_is_what_sets_the_headroom() {
        let windows = vec![
            win("5h", "5h", WindowKind::Short, 104.0),
            win("7d", "7d", WindowKind::Long, 30.0),
            win("scoped:Fable", "Fable", WindowKind::Scoped, 110.0),
        ];
        for m in [models(&[]), models(&["Fable"])] {
            assert_eq!(
                binding_window(&windows, &m).map(|w| 100.0 - w.pct),
                headroom(&windows, &m),
                "{m:?}"
            );
        }
    }

    #[test]
    fn spans_read_as_days_hours_or_minutes() {
        for (secs, text) in [
            (-5, "<1m"),
            (0, "<1m"),
            (59, "<1m"),
            (60, "1m"),
            (3_599, "59m"),
            (3_600, "1h00m"),
            (9_630, "2h40m"),
            (86_399, "23h59m"),
            (86_400, "1d00h"),
            (291_630, "3d09h"),
        ] {
            assert_eq!(span(secs), text, "{secs}");
        }
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-core rank`
Expected: FAIL to compile. `error[E0412]: cannot find type `Candidate` in this scope`, and
`error[E0425]` for `best_order`, `next_available`, `binding_window` and `span`, because only
the tests exist so far.

- [ ] **Step 3: Implement the ranking**

In `crates/tagteam-core/src/rank.rs`, insert between the module doc and `#[cfg(test)]`:

```rust
use std::cmp::Ordering;

use crate::usage::{Window, is_relevant};

/// One candidate as a usage strategy sees it: its position and its decision-grade headroom
/// (`None`: unknown, §8.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub position: u32,
    pub headroom: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BestOrder {
    /// Positions to try in order: known headroom strictly above `live` (every known one when
    /// `live` is None), most headroom first, ties to the lower position.
    Try(Vec<u32>),
    /// The live headroom is known and no known candidate beats it.
    AlreadyBest,
    /// No candidate has a known headroom.
    UsageUnavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NextAvailable {
    /// `walk` (rotation order) minus candidates whose known headroom is ≤ 0.
    Try { order: Vec<u32>, skipped: Vec<u32> },
    /// Every candidate in a non-empty walk is known to be exhausted.
    Exhausted,
}

/// A headroom a strategy may compare. §8.2's `headroom` is always finite; a NaN reaching here
/// anyway is unknown, never a number to rank by.
fn known(headroom: Option<f64>) -> Option<f64> {
    headroom.filter(|h| !h.is_nan())
}

/// §9.3 `best`: the candidates with a known headroom, most first and ties to the lower
/// position, keeping only those strictly above a known `live` headroom. Unknown candidates are
/// never ordered. No known candidate at all is `UsageUnavailable`, whatever `live` is; a known
/// `live` that none beats is `AlreadyBest`.
pub fn best_order(live: Option<f64>, candidates: &[Candidate]) -> BestOrder {
    let mut ranked: Vec<(u32, f64)> = candidates
        .iter()
        .filter_map(|c| known(c.headroom).map(|h| (c.position, h)))
        .collect();
    if ranked.is_empty() {
        return BestOrder::UsageUnavailable;
    }
    if let Some(live) = known(live) {
        ranked.retain(|&(_, h)| h > live);
        if ranked.is_empty() {
            return BestOrder::AlreadyBest;
        }
    }
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    BestOrder::Try(ranked.into_iter().map(|(position, _)| position).collect())
}

/// §9.3 `next-available`: the rotation's walk, in its order, without the candidates known to
/// be at their limit (headroom ≤ 0). Unknown headroom is never skipped (§8.2). `skipped` keeps
/// the walk's order. `Exhausted` only when the walk had candidates and every one was skipped.
pub fn next_available(walk: &[Candidate]) -> NextAvailable {
    let (skipped, order): (Vec<&Candidate>, Vec<&Candidate>) = walk
        .iter()
        .partition(|c| known(c.headroom).is_some_and(|h| h <= 0.0));
    if order.is_empty() && !skipped.is_empty() {
        return NextAvailable::Exhausted;
    }
    NextAvailable::Try {
        order: order.iter().map(|c| c.position).collect(),
        skipped: skipped.iter().map(|c| c.position).collect(),
    }
}

/// The relevant window with the highest pct (§8.2): what binds the headroom. Ties go to the
/// earlier window in `windows`.
pub fn binding_window<'w>(windows: &'w [Window], models: &[String]) -> Option<&'w Window> {
    windows
        .iter()
        .filter(|w| is_relevant(w, models))
        .fold(None::<&Window>, |best, w| match best {
            Some(b) if b.pct >= w.pct => Some(b),
            _ => Some(w),
        })
}

/// A span of time as tagteam states it: `3d09h`, `2h40m`, `45m`, or `<1m`; a negative span is
/// none. `list`'s countdowns and the strategies' messages share it.
pub fn span(secs: i64) -> String {
    let s = secs.max(0);
    let (days, hours, minutes) = (s / 86_400, s % 86_400 / 3_600, s % 3_600 / 60);
    if days > 0 {
        format!("{days}d{hours:02}h")
    } else if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        "<1m".into()
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-core rank`
Expected: PASS, 17 tests.

Run: `cargo test -p tagteam-core`
Expected: PASS. Nothing else in the crate changed.

- [ ] **Step 5: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-core/src/rank.rs crates/tagteam-core/src/lib.rs
git commit -m "Rank accounts by usage headroom for the best and next-available strategies"
```

---

### Task 9: Usage strategies in the engine

§9.3, "Strategies that rank by usage":

> Before planning, they collect on demand (§8.3) for the live account and every switchable
> candidate, then rank decision-grade readings only (§8.4). Relevant windows follow §8.2 and
> `autoswitch.models`, or `--model` for this invocation. `best` never picks a candidate whose
> headroom is unknown; when some were unknown, a warning says how many. Before candidates are
> counted, a quarantine that no longer binds is released (§7.4). Both read vaults lazily in
> their own order, as rotation does. Under the locks, the ranking is not recomputed. The pick
> stands while it is still switchable and the live account is unchanged; otherwise the strategy
> plans again from the store's readings. If the live account under the locks is already the
> pick, the result is a no-op with reason `already-active`. A Dead verdict while freshening the
> pick quarantines it, and the strategy plans again without it (§7.2).

§7.4: "The active account's quarantine holds while *either* the live credential or the vault
matches `quarantine_fp`." Decisions 8–11 settle who is collected, how quarantines are released,
how `best` orders, and what stands under the locks.

The task has four parts. Part B has two commits; the others have one each.
- **A.** `decision_windows` in `views.rs`. The trust computation moves out of `usage_view`, so
  the views and the strategies share it.
- **B.** One §7.4 predicate in `quarantine.rs`. B1 makes the refresh gate hold a live
  account's quarantine while its live credential is bound. B2 adds
  `release_unbound_quarantines`, which records its source, on the same predicate.
- **C.** The strategies in `switch.rs`.
- **D.** M1's L302 rotation edge tests.

**Readings of the spec this task commits to:**
- **The candidates are rotation's candidates** for both strategies: `is_candidate` rows, in
  `candidate_order`'s walk order, which never includes a managed live anchor.
  - With a managed live login and fewer than two candidates, the result is `only-one-account`
    (§9.2).
  - With no managed live login and no candidate, it is `no-valid-target`. Both are checked
    before anything is ranked.
  - `best`'s anchor is "the live account", which is what `candidate_order` already excludes.
- **"Switchable" includes a vault credential (§9.3).** A strategy's lazy walk can end without a
  pick, because every account in its order has an empty vault. Those accounts were never
  switchable, so the outcome is the spec's rule applied to the switchable ones:
  - `best` with a known live headroom: `already-best`.
  - `best` with the live headroom unknown: `usage-unavailable`.
  - `next-available`: `candidates-exhausted` if it skipped anyone, and otherwise rotation's own
    no-op (`only-one-account`, or `no-valid-target` with no managed live login).
- **What the messages say:**
  - A binding window reads `<label> at <pct>%`, pct rounded as `list` rounds it, e.g.
    `5h at 100%`.
  - An account reads by its label, as `already-active`'s message does.
  - `candidates-exhausted` names every candidate with its binding window, then "the earliest
    reset is in <span>". That is the earliest `resets_at` among those binding windows (§11.2
    step 8: "select the binding window first, then its reset"), left out when none has one.
  - A switch that skipped accounts names each in a warning (NOTES 4).
  - `already-best` names the live account's binding window and the best candidate's.
- **Warnings `best` adds:**
  - How many candidates had unknown headroom, on every outcome that ranked (switched,
    `already-best`, an exhausted walk). Not on `usage-unavailable`, whose message already says
    no candidate is known.
  - `switching to the best known candidate; the live account's usage is unknown`, or `…; there
    is no managed live login to compare with`, only on a switch.
  - An API-key candidate has no usage (§13.2 `api_key`), so its headroom is unknown: `best`
    never picks it and counts it, and `next-available` never skips it.
- **`--model` replaces `autoswitch.models` entirely**, an empty list included. It applies to
  relevance (headroom and the binding window) and to trust's post-429 reset (§8.4), which is why
  `decision_windows` takes `models`.
- **"Refuse cases as rotation does" comes first.** A run shell, an unresolved interrupted
  switch, no store, and an unmanaged live login without `--force` are all decided before
  anything is released or collected, so a no-op of that kind uses no network.
- **Collection** (Decision 8) is `CollectMode::OnDemand` for the managed live account and every
  `is_candidate` row. §8.3's on-demand rule is unchanged: older than 180 s, and due or unplanned.
  - The collector's warnings lead the outcome's warnings.
  - A collection error other than an interruption becomes one warning, `usage was not
    collected: <error>`, as `list` words it (`App::collect` in `app.rs`).
  - An interruption propagates (§14.1).
- **Release** (Decision 9) runs before collection and before counting. It takes each
  quarantined account's lock with `try_acquire` only.
  - Under the lock, the row is read again.
  - The vault must be readable and hold another fingerprint than `quarantine_fp`.
  - For the live account, the live credential must also differ from `quarantine_fp`. A live
    identity that cannot be read may be this account's, so the live credential is compared;
    a live credential that cannot be read, or a degraded one, counts as bound.
  - The refresh gate's own release uses the same predicate (`quarantine_released`), so it
    never releases what `release_unbound_quarantines` would hold.
  - Each release is recorded as an `unquarantine` event with the caller's source. A usage
    strategy passes the switch's `req.source`.
  - A pending replacement is not reconciled here. The next `lock_account` does that, as it
    always has.
- **Under the locks** (Decision 11), the pick stands while it is still `is_candidate` and the
  live login is the account the plan ranked against (`Plan.anchor`). Otherwise the attempt loop
  plans again, from the store's readings with no network.
  - A pick that is now the live account is `already-active` (Review Focus 4), as for rotation.
  - A pick quarantined since planning is planned around, as rotation does.

**Files:**
- Modify: `crates/tagteam-engine/src/views.rs` (Part A)
- Modify: `crates/tagteam-engine/tests/views_usage.rs` (Part A)
- Modify: `crates/tagteam-engine/src/quarantine.rs` (Part B), `crates/tagteam-engine/src/refresh.rs`
  (Part B1)
- Modify: `crates/tagteam-engine/tests/gate.rs`, `crates/tagteam-engine/tests/freshen.rs`
  (Part B1)
- Create: `crates/tagteam-engine/tests/strategy.rs` (Part B2, extended in Part C)
- Modify: `crates/tagteam-engine/src/switch.rs` (Part C)
- Modify: `crates/tagteam-engine/tests/rotation.rs` (Part D)

**Interfaces:**
- Consumes:
  - Task 8: `tagteam_core::rank::{Candidate, BestOrder, best_order, NextAvailable,
    next_available, binding_window, span}` with the signatures above.
  - Task 2: `EngineError::signal(&self) -> Option<i32>`.
  - Task 5: `Engine::collect_usage(&self, mode: CollectMode) -> Result<CollectReport,
    EngineError>`, which returns `Err(EngineError::Interrupted(n))` when cancelled.
    `CollectReport.warnings: Vec<String>`.
  - M2b: `tagteam_core::usage::{headroom, earliest_relevant_reset}`,
    `tagteam_core::trust::decision_grade`, `Store::{accounts, account, usage_state,
    usage_lease_live, clear_quarantine}`, `AccountLock::try_acquire(env: &Env, id: &AccountId)
    -> Result<Option<AccountLock>, LockError>`, `Engine::unquarantine(&self, row: &AccountRow)
    -> Result<bool, EngineError>`, `Engine::lock_account`, `Engine::set_disabled`,
    `Engine::move_to`.
  - Fixtures (`tests/common/mod.rs`): `Fx::{new, add, quarantine, put_vault, rotate_live,
    expire_access, script_token_error, script_usage, usage_state, engine_with_env,
    engine_with_settings, engine_with_vault_probe, switch_to, switch_request, live_email,
    live_refresh_token, rotation_request, provider, paths}`, `credential`,
    `quarantine_of`, `token_requests`, `vault_fp`, `usage_bearers`, `usage_fixture`,
    `CLAUDE_JSON`. Also `views_usage.rs`'s own `reading`, `record`, `fail` and `at`, and
    `gate.rs`'s own `gate`.
- Produces:
  - `views.rs`: `pub fn decision_windows(&self, row: &AccountRow, models: &[String]) ->
    Result<Option<Vec<Window>>, EngineError>`, and the private `fn is_decision_grade(&self,
    store: &Store, row: &AccountRow, state: &UsageStateRow, models: &[String], budget:
    &PollBudget) -> Result<bool, EngineError>`, which `usage_view` now calls.
  - `quarantine.rs`:
    - `pub fn release_unbound_quarantines(&self, provider: &ProviderId, source: &'static str)
      -> Result<Vec<AccountId>, EngineError>`.
    - `pub(crate) fn quarantine_released(&self, p: &dyn Provider, row: &AccountRow) -> bool`:
      moved from `refresh.rs`, now §7.4's full rule (the vault half and, for the live account,
      the live half), and shared by the gate and the release.
    - Private: `fn vault_moved_past(&self, p: &dyn Provider, row: &AccountRow) -> bool`, `fn
      live_still_bound(&self, p: &dyn Provider, row: &AccountRow) -> bool`, and `fn
      unquarantine_from(&self, row: &AccountRow, source: &str) -> Result<bool, EngineError>`.
    - `quarantine_event` gains a `source: &str` parameter.
  - `refresh.rs`: `pub(crate) fn fp_str(p: &dyn Provider, bytes: &[u8]) -> String`.
  - `switch.rs`:
    - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum UsageStrategy { Best,
      NextAvailable }` with `pub fn as_str(self) -> &'static str` (`"best"`,
      `"next-available"`).
    - `SwitchTarget::Usage { strategy: UsageStrategy, models: Option<Vec<String>> }`.
    - `SwitchReason::{UsageUnavailable, AlreadyBest, CandidatesExhausted}` (`"usage-unavailable"`,
      `"already-best"`, `"candidates-exhausted"`).
    - `SwitchOutcome.strategy` is `"best"`/`"next-available"` for a usage target.
    - Private: `SwitchTarget::chosen`, `Engine::{prepare_usage, switch_planned, walk,
      usage_pick, best_pick, next_available_pick, exhausted_message}`, `Plan.{anchor, notes}`.

#### Part A: `decision_windows`

- [ ] **Step 1: Write the failing test**

Append to `crates/tagteam-engine/tests/views_usage.rs`:

```rust
#[test]
fn decision_windows_are_the_reading_while_it_is_decision_grade_under_the_models_given() {
    // §8.4 after a 429: trusted until the earliest relevant reset, capped at two hours. Fable
    // resets first here, so it shortens that trust only when it counts (§8.2).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let mut windows = reading(T0, 9.0, 40.0);
    windows[2].resets_at = Some(T0 + 1_000);
    record(&fx, &a, &windows, T0, T0 + 180);
    fail(&fx, &a, "http-429", T0 + 200, T0 + 5_000, Some(T0 + 5_000));
    let store = fx.engine.store().unwrap();
    let a_row = store.account(&a).unwrap().unwrap();
    let b_row = store.account(&b).unwrap().unwrap();
    let fable = ["Fable".to_owned()];

    at(&fx, T0 + 100);
    assert_eq!(
        fx.engine.decision_windows(&a_row, &fable).unwrap(),
        Some(windows.clone()),
        "a young reading counts under any models"
    );
    assert_eq!(
        fx.engine.decision_windows(&b_row, &[]).unwrap(),
        None,
        "never read"
    );

    at(&fx, T0 + 4_000);
    assert_eq!(
        fx.engine.decision_windows(&a_row, &[]).unwrap(),
        Some(windows),
        "trusted until the 5h reset, capped at T0 + 7200"
    );
    assert_eq!(
        fx.engine.decision_windows(&a_row, &fable).unwrap(),
        None,
        "Fable's reset, at T0 + 1000, has passed"
    );
}

#[test]
fn decision_windows_take_a_clock_skewed_plan_for_no_plan() {
    // As the views do (§8.4): past the five-minute rule, a legal plan keeps a reading
    // decision-grade, and a `next_poll_at` a day ahead (clock skew) does not.
    let fx = Fx::new();
    let legal = fx.add("a@x.co", "rt-a");
    let skewed = fx.add("b@x.co", "rt-b");
    record(&fx, &legal, &reading(T0, 9.0, 40.0), T0, T0 + 1_200);
    record(&fx, &skewed, &reading(T0, 9.0, 40.0), T0, T0 + 86_400);
    at(&fx, T0 + 600);
    let store = fx.engine.store().unwrap();
    for (id, grade) in [(&legal, true), (&skewed, false)] {
        let row = store.account(id).unwrap().unwrap();
        assert_eq!(
            fx.engine.decision_windows(&row, &[]).unwrap().is_some(),
            grade,
            "{}",
            row.label
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p tagteam-engine --test views_usage decision_windows`
Expected: FAIL to compile: `error[E0599]: no method named `decision_windows` found for struct
`Engine``.

- [ ] **Step 3: Share the trust computation and add `decision_windows`**

In `crates/tagteam-engine/src/views.rs`, `Engine::usage_view`, replace:

```rust
        let now_ms = self.now_ms();
        let now_s = now_ms.div_euclid(1000);
        let windows = match state.fetched_at {
            Some(at) => {
                let read = state.last_good.as_deref().unwrap_or_default();
                Some(if with_pace {
                    paced(store, &row.id, read, at)?
                } else {
                    read.iter().map(|w| (w.clone(), Pace::default())).collect()
                })
            }
            None => None,
        };
        let reset = state
            .last_good
            .as_deref()
            .and_then(|w| earliest_relevant_reset(w, &self.settings().models));
        // §8.4 extends trust only while failures are being retried, and a quarantined account
        // is never retried: it keeps the five-minute rule alone.
        let retried = row.quarantine_reason.is_none();
        let trusted = windows.is_some()
            && decision_grade(&TrustInputs {
                now_s,
                fetched_at: state.fetched_at,
                consecutive_failures: if retried {
                    state.consecutive_failures
                } else {
                    0
                },
                // A plan further out than any legal one is clock skew (§8.4), not a plan.
                plan_in_force: retried
                    && state
                        .next_poll_at
                        .is_some_and(|at| at > now_s && !plan_is_skewed(at, now_s, budget)),
                live_lease: store.usage_lease_live(&row.id, now_ms)?,
                last_429_at: state.last_429_at,
                earliest_relevant_reset: reset,
            });
```

with:

```rust
        let now_s = self.now_ms().div_euclid(1000);
        let windows = match state.fetched_at {
            Some(at) => {
                let read = state.last_good.as_deref().unwrap_or_default();
                Some(if with_pace {
                    paced(store, &row.id, read, at)?
                } else {
                    read.iter().map(|w| (w.clone(), Pace::default())).collect()
                })
            }
            None => None,
        };
        let trusted = windows.is_some()
            && self.is_decision_grade(store, row, &state, &self.settings().models, budget)?;
```

Then add these two methods to the same `impl Engine` block, directly after `usage_view`.
`is_decision_grade` takes the provider's `PollBudget`, as `usage_view` does, because a planned
poll further out than any legal plan is clock skew, not a plan in force (§8.4).
`decision_windows` keeps the contract's signature and finds the budget from the row's provider,
as `account_view_with` does (`PollBudget::STANDARD` for an unregistered one):

```rust
    /// §8.4 for `row`'s reading in `state`: whether it may drive a decision. Relevance, which
    /// sets the post-429 rule's earliest reset, follows `models` (§8.2), and a planned poll
    /// further out than `budget` allows is clock skew, not a plan in force. Reads the store
    /// only.
    fn is_decision_grade(
        &self,
        store: &Store,
        row: &AccountRow,
        state: &UsageStateRow,
        models: &[String],
        budget: &PollBudget,
    ) -> Result<bool, EngineError> {
        let now_ms = self.now_ms();
        let now_s = now_ms.div_euclid(1000);
        // §8.4 extends trust only while failures are being retried, and a quarantined account
        // is never retried: it keeps the five-minute rule alone.
        let retried = row.quarantine_reason.is_none();
        Ok(decision_grade(&TrustInputs {
            now_s,
            fetched_at: state.fetched_at,
            consecutive_failures: if retried {
                state.consecutive_failures
            } else {
                0
            },
            // A plan further out than any legal one is clock skew (§8.4), not a plan.
            plan_in_force: retried
                && state
                    .next_poll_at
                    .is_some_and(|at| at > now_s && !plan_is_skewed(at, now_s, budget)),
            live_lease: store.usage_lease_live(&row.id, now_ms)?,
            last_429_at: state.last_429_at,
            earliest_relevant_reset: state
                .last_good
                .as_deref()
                .and_then(|w| earliest_relevant_reset(w, models)),
        }))
    }

    /// The account's last good windows if they are decision-grade (§8.4) under `models`'
    /// relevance; `None` otherwise. Reads the store only.
    pub fn decision_windows(
        &self,
        row: &AccountRow,
        models: &[String],
    ) -> Result<Option<Vec<Window>>, EngineError> {
        let Some(store) = self.existing_store()? else {
            return Ok(None);
        };
        let Some(state) = store.usage_state(&row.id)? else {
            return Ok(None);
        };
        let budget = self
            .registry
            .get(&row.provider)
            .map_or(PollBudget::STANDARD, |p| p.poll_budget());
        if state.fetched_at.is_none()
            || !self.is_decision_grade(&store, row, &state, models, &budget)?
        {
            return Ok(None);
        }
        Ok(Some(state.last_good.unwrap_or_default()))
    }
```

A reading with no windows is stored as a null `last_good` with a `fetched_at`. It comes back as
`Some(vec![])`, whose headroom is unknown. That is §8.2's "an empty result normalizes to None".

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --test views_usage`
Expected: PASS, both new tests included. The existing trust tests
(`a_fresh_reading_is_ok_decision_grade_and_carries_pace`,
`trust_extends_while_a_plan_is_in_force_or_a_fetch_is_in_flight`,
`a_clock_skewed_plan_is_not_a_plan_in_force_for_trust`, and the rest) pass unchanged, so
`usage_view` computes trust exactly as before.

- [ ] **Step 5: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/views.rs crates/tagteam-engine/tests/views_usage.rs
git commit -m "Read an account's decision-grade windows under the given models"
```

#### Part B: one §7.4 predicate, and `release_unbound_quarantines`

Part B has two commits. B1 fixes the refresh gate's own release of a quarantine, which today
checks only the vault half of §7.4. B2 adds `release_unbound_quarantines` on top of the same
predicate.

Where B1's gap is reachable: §7.2's quarantined-target rule runs in `freshen` before the gate,
on the row the plan read. So a quarantine that is already on the row when the switch plans is
handled there and never reaches the gate. The gate meets a quarantined live account only when
the strike lands after the caller read the row: another process's gate quarantines the account
between planning and this switch's gate. Today the gate then releases the quarantine on the
vault half alone, returns `Owned(Live)`, and a forced self-switch activates the spent vault
generation with only a warning. B1's freshen test produces that interleaving deterministically
with the vault probe. The other tests pin the predicate at the gate itself.

##### B1: the gate holds a live account's quarantine while its live credential is bound

- [ ] **Step 7: Write the failing tests**

In `crates/tagteam-engine/tests/gate.rs`, change the `common` import to:

```rust
use common::{Fx, crashed_switch, credential, due, quarantine_of, token_requests, vault_fp};
```

and add after `a_quarantine_bound_to_an_older_generation_is_released_and_the_gate_proceeds`:

```rust
#[test]
fn the_live_account_s_quarantine_holds_while_its_live_credential_is_bound() {
    // §7.4: the active account's quarantine holds while either the live credential or the
    // vault matches `quarantine_fp`. The vault has moved on; the live credential has not.
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b");
    let a = fx.add("a@x.co", "rt-a"); // live: rt-a
    let bound = vault_fp(&fx, &a);
    fx.quarantine(&a, "invalid_grant", &bound);
    fx.put_vault(&a, &credential("a@x.co", "rt-a2"));
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Dead(QuarantineReason::InvalidGrant)
    ));
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("invalid_grant".into()), Some(bound))
    );
    // Once Claude Code has rotated the live credential too, neither copy is bound: the gate
    // releases it, and leaves the live token to §7.5.
    fx.rotate_live("rt-a3");
    assert!(matches!(gate(&fx, &a), GateOutcome::Owned(OwnedBy::Live)));
    assert_eq!(quarantine_of(&fx, &a), (None, None));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn with_the_live_identity_unreadable_the_live_credential_still_decides() {
    // An unreadable live identity may be this account's (§4.3), so the live credential is
    // compared as for the live account: still the bound generation, it holds the quarantine;
    // another generation does not.
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b");
    let a = fx.add("a@x.co", "rt-a"); // live: rt-a
    let bound = vault_fp(&fx, &a);
    fx.quarantine(&a, "invalid_grant", &bound);
    fx.put_vault(&a, &credential("a@x.co", "rt-a2"));
    fs::write(fx.paths().global_config, "{\n  \"oauthAccount\": ").unwrap(); // torn
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Dead(QuarantineReason::InvalidGrant)
    ));
    assert!(quarantine_of(&fx, &a).0.is_some());
    fx.rotate_live("rt-a3");
    assert!(matches!(gate(&fx, &a), GateOutcome::Owned(OwnedBy::Live)));
    assert_eq!(quarantine_of(&fx, &a), (None, None));
    assert_eq!(token_requests(&fx), 0);
}
```

In `crates/tagteam-engine/tests/freshen.rs`, add after
`a_quarantined_target_works_until_its_access_token_expires`:

```rust
#[test]
fn a_strike_on_the_live_account_after_planning_holds_while_its_live_credential_is_bound() {
    // Another process's gate quarantines b, the live account, after this forced self-switch
    // has planned on b's unquarantined row and before this switch's own gate runs. b's vault
    // has moved on, but the live credential is still the generation the strike is bound to
    // (§7.4), so the gate must not release it: it reports Dead, and the direct switch refuses
    // (§7.2) instead of activating the vault's generation.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: rt-b
    let bound = vault_fp(&fx, &b);
    fx.put_vault(&b, &credential("b@x.co", "rt-b2"));
    fx.expire_access(&b); // due: the forced self-switch freshens the vault's generation
    let strike = Mutex::new(Some((fx.engine.store().unwrap(), b.clone(), bound.clone())));
    let key = b.to_string();
    // `plan` reads b's vault first (`has_login`): the strike lands there, once.
    let engine = fx.engine_with_vault_probe(move |read| {
        if read != key {
            return;
        }
        if let Some((store, id, fp)) = strike.lock().unwrap().take() {
            store.set_quarantine(&id, "invalid_grant", &fp, 1).unwrap();
        }
    });
    let err = engine.switch(fx.switch_request(&b, true)).unwrap_err();
    assert_eq!(err.kind(), "relogin-required", "{err}");
    assert_eq!(
        quarantine_of(&fx, &b),
        (Some("invalid_grant".into()), Some(bound))
    );
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
    assert_eq!(token_requests(&fx), 0);
}
```

- [ ] **Step 8: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --no-fail-fast --test gate --test freshen`
Expected: FAIL, three tests (`--no-fail-fast`, so the second file runs after the first fails):
- `the_live_account_s_quarantine_holds_while_its_live_credential_is_bound`: the first
  `matches!` assertion fails. The gate releases the quarantine on the vault half alone and
  returns `Owned(Live)`.
- `with_the_live_identity_unreadable_the_live_credential_still_decides`: the first `matches!`
  assertion fails, for the same release and the same `Owned(Live)` (an unreadable live
  identity counts as live in step 2).
- `a_strike_on_the_live_account_after_planning_holds_while_its_live_credential_is_bound`:
  panics with ``called `Result::unwrap_err()` on an `Ok` value``. The released quarantine lets
  the forced self-switch activate b's spent vault generation, with only the "it may be the live
  login" warning.

Every other test in both files passes.

- [ ] **Step 9: Share one §7.4 predicate between the gate and the release**

In `crates/tagteam-engine/src/refresh.rs`, make `fp_str` crate-visible. Replace:

```rust
/// The lineage fingerprint as the store records it; empty for bytes that carry no token.
fn fp_str(p: &dyn Provider, bytes: &[u8]) -> String {
```

with:

```rust
/// The lineage fingerprint as the store records it; empty for bytes that carry no token.
pub(crate) fn fp_str(p: &dyn Provider, bytes: &[u8]) -> String {
```

Then delete this method from the `impl Engine` block that holds `refresh_stored`. It moves to
`quarantine.rs` and gains the live half, and the gate's call `self.quarantine_released(p, &row)`
stays as it is:

```rust
    /// Whether `row`'s quarantine no longer binds: the vault is readable and holds a
    /// generation other than the one the quarantine is bound to. Anything less (unreadable,
    /// absent, empty, or an unbound quarantine) leaves it standing.
    fn quarantine_released(&self, p: &dyn Provider, row: &AccountRow) -> bool {
        match self.vault.read(&row.id) {
            Read::Present(b) if !b.is_empty() => row
                .quarantine_fp
                .as_deref()
                .is_some_and(|bound| bound != fp_str(p, &b)),
            _ => false,
        }
    }
```

In `refresh_stored`, replace the comment above the release:

```rust
        // §7.4: a quarantine holds only while the vault still holds the generation it is bound
        // to. A vault that has moved on releases it (§11.2 step 1), which also heals
        // `persist_generation`'s window between the vault write and the store update.
```

with:

```rust
        // §7.4: a quarantine holds while the vault still holds the generation it is bound to,
        // and, for the live account, while the live credential does (`quarantine_released`,
        // shared with `release_unbound_quarantines`). One that binds neither is released
        // (§11.2 step 1), which also heals `persist_generation`'s window between the vault
        // write and the store update.
```

In `crates/tagteam-engine/src/quarantine.rs`, replace the imports:

```rust
use serde_json::json;
use tagteam_provider::DeadReason;

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::{AccountRow, EventRow};
```

with:

```rust
use serde_json::json;
use tagteam_provider::{DeadReason, Provenance, Provider, Read};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::refresh::fp_str;
use crate::store::{AccountRow, EventRow};
use crate::switch::Axis;
```

and add these methods at the end of the `impl Engine` block, after `unquarantine`:

```rust
    /// §7.4: whether `row`'s quarantine no longer binds. The vault must hold another
    /// generation than the one the quarantine is bound to, and, when `row` is the live
    /// account, so must the live credential: the active account's quarantine holds while
    /// either copy matches `quarantine_fp`. The refresh gate and `release_unbound_quarantines`
    /// share it.
    pub(crate) fn quarantine_released(&self, p: &dyn Provider, row: &AccountRow) -> bool {
        self.vault_moved_past(p, row) && !self.live_still_bound(p, row)
    }

    /// The vault half: the vault is readable and holds a generation other than the one the
    /// quarantine is bound to. Anything less (unreadable, absent, empty, or a quarantine bound
    /// to nothing) leaves it standing.
    fn vault_moved_past(&self, p: &dyn Provider, row: &AccountRow) -> bool {
        match self.vault.read(&row.id) {
            Read::Present(b) if !b.is_empty() => row
                .quarantine_fp
                .as_deref()
                .is_some_and(|bound| bound != fp_str(p, &b)),
            _ => false,
        }
    }

    /// The live half: whether `row` is the live login and its live credential may still be
    /// the generation the quarantine is bound to. A live identity that cannot be read may be
    /// `row`'s, so the live credential is compared; a live credential that cannot be read, or a
    /// degraded one, may be exactly that generation, so it counts as bound.
    fn live_still_bound(&self, p: &dyn Provider, row: &AccountRow) -> bool {
        let is_live = match p.live_identity(&self.env) {
            Read::Present(i) => p.identity_key(&i).as_str() == row.identity_key,
            Read::Absent => false,
            Read::Unreadable(_) => true,
        };
        if !is_live {
            return false;
        }
        let Some(bound) = row.quarantine_fp.as_deref() else {
            return true;
        };
        let auth = p.read_live_auth(&self.env);
        let live = match Axis::of(p, &row.kind) {
            Axis::Entry => match auth.credential {
                Read::Present(c) if c.provenance() == Provenance::Fresh => Some(c.bytes().to_vec()),
                Read::Absent => None,
                _ => return true,
            },
            Axis::ManagedKey => match auth.managed_key {
                Read::Present(k) => Some(k),
                Read::Absent => None,
                Read::Unreadable(_) => return true,
            },
        };
        live.is_some_and(|bytes| fp_str(p, &bytes) == bound)
    }
```

- [ ] **Step 10: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --test gate --test freshen`
Expected: PASS, the three new tests included.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS, with no existing test changed. Every existing test that reaches the gate's
release does so for an account that is not the live login with a readable live identity, and
there the live half is false and the predicate is the old one. Examples:
`a_quarantine_bound_to_an_older_generation_is_released_and_the_gate_proceeds` (a is inactive,
b is live) and `a_quarantined_account_is_never_sent` (still bound by the vault).

- [ ] **Step 11: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 12: Commit**

```bash
git add crates/tagteam-engine/src/quarantine.rs crates/tagteam-engine/src/refresh.rs crates/tagteam-engine/tests/gate.rs crates/tagteam-engine/tests/freshen.rs
git commit -m "Hold a live account's quarantine while its live credential is the bound generation"
```

##### B2: `release_unbound_quarantines`

- [ ] **Step 13: Write the failing tests**

Create `crates/tagteam-engine/tests/strategy.rs`:

```rust
//! §9.3's usage strategies, `best` and `next-available`: the release of quarantines that no
//! longer bind (§7.4, Decision 9), on-demand collection (§8.3, Decision 8), ranking from
//! decision-grade readings (§8.4), lazy vault reads, freshening (§7.2), and what stands under
//! the locks (Decision 11, Review Focus 4). Readings are recorded through the store's own
//! reserve-and-record calls, as the collector records them.
mod common;

use common::{Fx, credential, quarantine_of, vault_fp};
use tagteam_engine::vault::SERVICE;

#[test]
fn nothing_is_released_or_created_without_a_store() {
    let fx = Fx::new();
    assert!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap()
            .is_empty()
    );
    assert!(!fx.env.data_dir().join("tagteam.db").exists());
}

#[test]
fn a_quarantine_the_vault_has_moved_past_is_released_and_recorded_with_its_source() {
    // M3b's tick passes "auto"; the event says who released it.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    assert_eq!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "auto")
            .unwrap(),
        [a.clone()]
    );
    assert_eq!(quarantine_of(&fx, &a), (None, None));
    let events = fx.engine.store().unwrap().events().unwrap();
    let last = events.last().unwrap();
    assert_eq!(
        (
            last.kind.as_str(),
            last.to_id.as_ref(),
            last.source.as_str()
        ),
        ("unquarantine", Some(&a), "auto")
    );
}

#[test]
fn a_quarantine_the_vault_still_holds_stays() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let bound = vault_fp(&fx, &a);
    fx.quarantine(&a, "invalid_grant", &bound);
    assert!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("invalid_grant".into()), Some(bound))
    );
}

#[test]
fn an_unreadable_vault_leaves_the_quarantine() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    fx.kc.set_unreadable(SERVICE, a.as_str(), true);
    assert!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap()
            .is_empty()
    );
    assert!(quarantine_of(&fx, &a).0.is_some());
}

#[test]
fn the_live_account_s_quarantine_holds_while_the_live_credential_is_its_generation() {
    // §7.4: the active account's quarantine holds while either copy matches `quarantine_fp`.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: rt-b
    let bound = vault_fp(&fx, &b);
    fx.quarantine(&b, "invalid_grant", &bound);
    // The vault moves on; the live credential is still the generation the strike is bound to.
    fx.put_vault(&b, &credential("b@x.co", "rt-b2"));
    assert!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap()
            .is_empty()
    );
    assert_eq!(quarantine_of(&fx, &b).1, Some(bound));
    // Claude Code rotates the live credential too: neither copy is bound any more.
    fx.rotate_live("rt-b3");
    assert_eq!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap(),
        [b.clone()]
    );
    assert_eq!(quarantine_of(&fx, &b), (None, None));
}

#[test]
fn a_busy_account_lock_leaves_the_quarantine_for_the_next_caller() {
    // Decision 9: try-only. Whoever holds the lock may be writing this very account.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    let held = fx.engine.lock_account(&a).unwrap();
    assert!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap()
            .is_empty()
    );
    assert!(quarantine_of(&fx, &a).0.is_some());
    drop(held);
    assert_eq!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap(),
        [a]
    );
}
```

- [ ] **Step 14: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --test strategy`
Expected: FAIL to compile: ``error[E0599]: no method named `release_unbound_quarantines`
found for struct `Engine` ``, eight times (six tests; two of them call it twice).

- [ ] **Step 15: Add the release, recording its source**

In `crates/tagteam-engine/src/quarantine.rs`, replace the imports B1 left:

```rust
use serde_json::json;
use tagteam_provider::{DeadReason, Provenance, Provider, Read};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::refresh::fp_str;
use crate::store::{AccountRow, EventRow};
use crate::switch::Axis;
```

with:

```rust
use serde_json::json;
use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::{DeadReason, Provenance, Provider, Read};

use crate::account_lock::AccountLock;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::refresh::fp_str;
use crate::store::{AccountRow, EventRow};
use crate::switch::Axis;
```

Let the events carry their source. Replace:

```rust
    fn quarantine_event(
        &self,
        row: &AccountRow,
        kind: &str,
        reason: Option<&str>,
    ) -> Result<(), EngineError> {
        self.store()?.insert_event(&EventRow {
            at: self.now_ms(),
            provider: row.provider.clone(),
            kind: kind.into(),
            from_id: None,
            to_id: Some(row.id.clone()),
            trigger: None,
            source: "cli".into(),
            detail: reason.map(|r| json!({"reason": r})),
        })?;
        Ok(())
    }
```

with:

```rust
    fn quarantine_event(
        &self,
        row: &AccountRow,
        kind: &str,
        reason: Option<&str>,
        source: &str,
    ) -> Result<(), EngineError> {
        self.store()?.insert_event(&EventRow {
            at: self.now_ms(),
            provider: row.provider.clone(),
            kind: kind.into(),
            from_id: None,
            to_id: Some(row.id.clone()),
            trigger: None,
            source: source.into(),
            detail: reason.map(|r| json!({"reason": r})),
        })?;
        Ok(())
    }
```

In `quarantine`, replace:

```rust
        self.quarantine_event(row, "quarantine", Some(reason.as_str()))?;
```

with:

```rust
        self.quarantine_event(row, "quarantine", Some(reason.as_str()), "cli")?;
```

and replace `unquarantine`:

```rust
    /// Clears the quarantine and records `unquarantine`; `false` when there was none.
    pub(crate) fn unquarantine(&self, row: &AccountRow) -> Result<bool, EngineError> {
        let cleared = self.store()?.clear_quarantine(&row.id)?;
        if cleared {
            self.quarantine_event(row, "unquarantine", None)?;
        }
        Ok(cleared)
    }
```

with:

```rust
    /// Clears the quarantine and records `unquarantine`; `false` when there was none.
    pub(crate) fn unquarantine(&self, row: &AccountRow) -> Result<bool, EngineError> {
        self.unquarantine_from(row, "cli")
    }

    /// `unquarantine`, with the event's `source` (`cli` or `auto`).
    fn unquarantine_from(&self, row: &AccountRow, source: &str) -> Result<bool, EngineError> {
        let cleared = self.store()?.clear_quarantine(&row.id)?;
        if cleared {
            self.quarantine_event(row, "unquarantine", None, source)?;
        }
        Ok(cleared)
    }
```

Then add this method at the end of the `impl Engine` block, after `live_still_bound`:

```rust
    /// §7.4 / Decision 9: clears every quarantine of `provider` that no longer binds
    /// (`quarantine_released`), each under its account lock (try-only; a busy account is
    /// left), and records each release with `source`. Returns the released accounts.
    pub fn release_unbound_quarantines(
        &self,
        provider: &ProviderId,
        source: &'static str,
    ) -> Result<Vec<AccountId>, EngineError> {
        let Some(store) = self.existing_store()? else {
            return Ok(Vec::new());
        };
        let p = self.provider(provider)?;
        let mut released = Vec::new();
        for listed in store.accounts(provider)? {
            if listed.quarantine_reason.is_none() {
                continue;
            }
            // Whoever holds it may be refreshing or writing this very account: it decides, and
            // the next caller looks again.
            let Some(_lock) = AccountLock::try_acquire(&self.env, &listed.id)? else {
                continue;
            };
            // Read again under the lock: the row may have changed since it was listed.
            let Some(row) = store.account(&listed.id)? else {
                continue;
            };
            if row.quarantine_reason.is_none() || !self.quarantine_released(p.as_ref(), &row) {
                continue;
            }
            if self.unquarantine_from(&row, source)? {
                tracing::info!(
                    position = row.position,
                    account = %row.id,
                    "released a quarantine that no longer binds"
                );
                released.push(row.id);
            }
        }
        Ok(released)
    }
```

- [ ] **Step 16: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --test strategy`
Expected: PASS, 6 tests.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS. The gate and `persist_generation` still call `unquarantine`, which records
`source = "cli"` as before (`quarantine_and_unquarantine_record_events` in `quarantine.rs`
pins that), and `quarantine` records `cli` too.

- [ ] **Step 17: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 18: Commit**

```bash
git add crates/tagteam-engine/src/quarantine.rs crates/tagteam-engine/tests/strategy.rs
git commit -m "Release quarantines that no longer bind, each under its account's lock"
```

#### Part C: the strategies

- [ ] **Step 19: Write the failing tests**

In `crates/tagteam-engine/tests/strategy.rs`, replace the two `use` lines after `mod common;`:

```rust
use common::{Fx, credential, quarantine_of, vault_fp};
use tagteam_engine::vault::SERVICE;
```

with:

```rust
use std::fs;

use common::{Fx, credential, quarantine_of, usage_bearers, usage_fixture, vault_fp};
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, PollBudget, PollPlan, Window, WindowKind};
use tagteam_engine::EngineError;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::Reserve;
use tagteam_engine::switch::{
    SwitchOutcome, SwitchReason, SwitchRequest, SwitchTarget, UsageStrategy,
};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::Keychain;
```

and append:

```rust
/// The fixture clock's start (`Fx`), in seconds.
const T0: i64 = 1_790_000_000;

fn window(key: &str, label: &str, kind: WindowKind, pct: f64, resets_at: i64) -> Window {
    Window {
        key: key.into(),
        label: label.into(),
        kind,
        pct,
        resets_at: Some(resets_at),
        period_s: match kind {
            WindowKind::Short => Some(18_000),
            _ => Some(604_800),
        },
        detail: None,
    }
}

/// A Claude Code reading: 5h at `five` (resetting at T0 + 2h40m30s), 7d at `seven` and the
/// Fable model window at `fable` (both resetting at T0 + 3d09h00m30s).
fn reading(five: f64, seven: f64, fable: f64) -> Vec<Window> {
    vec![
        window("5h", "5h", WindowKind::Short, five, T0 + 9_630),
        window("7d", "7d", WindowKind::Long, seven, T0 + 291_630),
        window(
            "scoped:Fable",
            "Fable",
            WindowKind::Scoped,
            fable,
            T0 + 291_630,
        ),
    ]
}

/// Records `windows` as `id`'s reading taken at `at`, its next poll planned at `next_poll_at`,
/// through the collector's own reserve (§8.3 phase 1) and record (phase 3).
fn record_at(fx: &Fx, id: &AccountId, windows: &[Window], at: i64, next_poll_at: i64) {
    let store = fx.engine.store().unwrap();
    let row = store.account(id).unwrap().unwrap();
    let r = match store
        .reserve_usage(&row, at * 1000, false, &PollBudget::STANDARD)
        .unwrap()
    {
        Reserve::Reserved(r) => r,
        other => panic!("not reserved at {at}: {other:?}"),
    };
    let plan = PollPlan {
        interval_s: next_poll_at - at,
        next_poll_at,
    };
    assert!(store.record_usage(&r, windows, at, &plan, 180).unwrap());
}

/// A reading taken now with a plan in force: decision-grade, and not due on demand, so the
/// strategy's collection sends nothing for it.
fn read(fx: &Fx, id: &AccountId, windows: &[Window]) {
    record_at(fx, id, windows, T0, T0 + 300);
}

fn usage(strategy: UsageStrategy, models: Option<Vec<&str>>) -> SwitchTarget {
    SwitchTarget::Usage {
        strategy,
        models: models.map(|m| m.into_iter().map(str::to_owned).collect()),
    }
}

fn request(fx: &Fx, target: SwitchTarget) -> SwitchRequest {
    SwitchRequest {
        provider: fx.provider(),
        target,
        force: false,
        source: "cli",
    }
}

fn best(fx: &Fx) -> Result<SwitchOutcome, EngineError> {
    fx.engine
        .switch(request(fx, usage(UsageStrategy::Best, None)))
}

fn next_available(fx: &Fx) -> Result<SwitchOutcome, EngineError> {
    fx.engine
        .switch(request(fx, usage(UsageStrategy::NextAvailable, None)))
}

/// `a`, `b` and `c` at positions 1, 2 and 3; `c` is live.
fn three(fx: &Fx) -> (AccountId, AccountId, AccountId) {
    (
        fx.add("a@x.co", "rt-a"),
        fx.add("b@x.co", "rt-b"),
        fx.add("c@x.co", "rt-c"),
    )
}

/// `claude /logout`: no `oauthAccount` and no credential. The store is not told.
fn log_out(fx: &Fx) {
    fs::write(fx.paths().global_config, common::CLAUDE_JSON).unwrap();
    fx.kc
        .delete(
            &keychain_service(&fx.env, ItemKind::OAuth),
            &keychain_account(&fx.env),
        )
        .unwrap();
}

#[test]
fn reasons_and_strategies_carry_the_spec_s_tokens() {
    assert_eq!(UsageStrategy::Best.as_str(), "best");
    assert_eq!(UsageStrategy::NextAvailable.as_str(), "next-available");
    assert_eq!(SwitchReason::UsageUnavailable.as_str(), "usage-unavailable");
    assert_eq!(SwitchReason::AlreadyBest.as_str(), "already-best");
    assert_eq!(
        SwitchReason::CandidatesExhausted.as_str(),
        "candidates-exhausted"
    );
}

#[test]
fn best_switches_to_the_candidate_with_the_most_headroom() {
    let fx = Fx::new();
    let (a, b, c) = three(&fx);
    read(&fx, &a, &reading(10.0, 50.0, 0.0));
    read(&fx, &b, &reading(10.0, 20.0, 0.0));
    read(&fx, &c, &reading(10.0, 90.0, 0.0));
    let out = best(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason, out.strategy),
        (true, SwitchReason::Switched, "best")
    );
    assert_eq!(out.to.unwrap().id, b);
    assert_eq!(out.from.unwrap().id, c);
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert!(
        usage_bearers(&fx).is_empty(),
        "every reading was fresh: nothing was fetched"
    );
}

#[test]
fn best_stays_when_no_candidate_beats_the_live_account() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx, &a, &reading(10.0, 60.0, 0.0));
    read(&fx, &b, &reading(10.0, 60.0, 0.0));
    let out = best(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason, out.strategy),
        (false, SwitchReason::AlreadyBest, "best")
    );
    assert_eq!(
        out.message,
        "b@x.co already has the most headroom (7d at 60%); the best candidate is a@x.co (7d at 60%)"
    );
    assert_eq!(out.from.map(|r| r.id), Some(b));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn best_without_a_known_candidate_is_usage_unavailable() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a"); // never read
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx, &b, &reading(10.0, 60.0, 0.0));
    let out = best(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason, out.strategy),
        (false, SwitchReason::UsageUnavailable, "best")
    );
    assert_eq!(
        out.message,
        "no candidate has a usage reading recent enough to rank by"
    );
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    // a was collected first (Decision 8); the scripted port has no reply, so it stays unknown.
    assert_eq!(usage_bearers(&fx), ["at-rt-a"]);
}

#[test]
fn best_counts_the_candidates_it_could_not_rank_in_a_warning() {
    let fx = Fx::new();
    let (a, _b, c) = three(&fx); // b is never read
    read(&fx, &a, &reading(10.0, 20.0, 0.0));
    read(&fx, &c, &reading(10.0, 60.0, 0.0));
    let out = best(&fx).unwrap();
    assert_eq!(out.to.unwrap().id, a);
    assert_eq!(
        out.warnings,
        ["1 candidate has no usage reading recent enough to rank by; it was not considered"]
    );
}

#[test]
fn best_with_the_live_usage_unknown_switches_to_the_best_known_with_a_warning() {
    let fx = Fx::new();
    let (a, b, _c) = three(&fx); // c is live and never read
    read(&fx, &a, &reading(10.0, 50.0, 0.0));
    read(&fx, &b, &reading(10.0, 20.0, 0.0));
    let out = best(&fx).unwrap();
    assert_eq!((out.switched, out.to.unwrap().id), (true, b));
    assert_eq!(
        out.warnings,
        ["switching to the best known candidate; the live account's usage is unknown"]
    );
}

#[test]
fn best_with_no_live_login_switches_to_the_best_known_with_a_warning() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    read(&fx, &a, &reading(10.0, 20.0, 0.0));
    read(&fx, &b, &reading(10.0, 50.0, 0.0));
    log_out(&fx);
    let out = best(&fx).unwrap();
    assert_eq!((out.switched, out.reason), (true, SwitchReason::Switched));
    assert_eq!(out.to.unwrap().id, a);
    assert_eq!(
        out.warnings,
        ["switching to the best known candidate; there is no managed live login to compare with"]
    );
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
}

#[test]
fn a_better_candidate_without_a_stored_credential_leaves_the_live_account_the_best() {
    // §9.3: a switchable account has a vault credential. The walk reads only the candidates
    // that beat the live account, finds none it can activate, and so none beats it.
    let fx = Fx::new();
    let (a, b, c) = three(&fx);
    read(&fx, &a, &reading(10.0, 10.0, 0.0));
    read(&fx, &b, &reading(10.0, 50.0, 0.0));
    read(&fx, &c, &reading(10.0, 30.0, 0.0));
    fx.kc.delete(SERVICE, a.as_str()).unwrap();
    let out = best(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason),
        (false, SwitchReason::AlreadyBest)
    );
    assert_eq!(
        out.message,
        "no candidate with more headroom than c@x.co (7d at 30%) holds a stored credential"
    );
}

#[test]
fn next_available_skips_exhausted_candidates_and_names_their_binding_windows() {
    let fx = Fx::new();
    let (a, b, c) = three(&fx); // c is live: the walk is a, b
    read(&fx, &a, &reading(100.0, 40.0, 0.0));
    read(&fx, &b, &reading(10.0, 95.0, 0.0));
    read(&fx, &c, &reading(10.0, 50.0, 0.0));
    let out = next_available(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason, out.strategy),
        (true, SwitchReason::Switched, "next-available")
    );
    assert_eq!(out.to.unwrap().id, b, "5 points left is not at the limit");
    assert_eq!(
        out.warnings,
        ["skipped a@x.co (position 1): at its limit (5h at 100%)"]
    );
}

#[test]
fn next_available_never_skips_an_unknown_candidate() {
    let fx = Fx::new();
    let (a, b, _c) = three(&fx); // a is never read
    read(&fx, &b, &reading(10.0, 20.0, 0.0));
    let out = next_available(&fx).unwrap();
    assert_eq!(out.to.unwrap().id, a);
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
}

#[test]
fn next_available_with_every_candidate_exhausted_names_each_and_the_earliest_reset() {
    let fx = Fx::new();
    let (a, b, c) = three(&fx);
    read(&fx, &a, &reading(10.0, 100.0, 0.0));
    read(&fx, &b, &reading(104.0, 100.0, 0.0));
    read(&fx, &c, &reading(10.0, 50.0, 0.0));
    let out = next_available(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason, out.strategy),
        (false, SwitchReason::CandidatesExhausted, "next-available")
    );
    assert_eq!(
        out.message,
        "every candidate is at its limit: a@x.co (7d at 100%), b@x.co (5h at 104%); the earliest reset is in 2h40m"
    );
    assert_eq!(fx.live_email().as_deref(), Some("c@x.co"));
}

#[test]
fn fewer_than_two_candidates_is_only_one_account_for_either_strategy() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    read(&fx, &a, &reading(10.0, 50.0, 0.0));
    for strategy in [UsageStrategy::Best, UsageStrategy::NextAvailable] {
        let out = fx
            .engine
            .switch(request(&fx, usage(strategy, None)))
            .unwrap();
        assert_eq!(
            (out.switched, out.reason, out.strategy),
            (false, SwitchReason::OnlyOneAccount, strategy.as_str())
        );
    }
}

#[test]
fn an_unreadable_vault_before_the_pick_names_the_account() {
    let fx = Fx::new();
    let (a, b, c) = three(&fx);
    read(&fx, &a, &reading(10.0, 10.0, 0.0));
    read(&fx, &b, &reading(10.0, 20.0, 0.0));
    read(&fx, &c, &reading(10.0, 90.0, 0.0));
    fx.kc.set_unreadable(SERVICE, a.as_str(), true);
    let err = best(&fx).unwrap_err();
    assert!(
        matches!(&err, EngineError::UnreadableAccount { position: 1, label, .. } if label == "a@x.co"),
        "{err}"
    );
    assert_eq!(fx.live_email().as_deref(), Some("c@x.co"));
}

#[test]
fn a_quarantine_that_no_longer_binds_is_released_before_candidates_are_counted() {
    // With a still quarantined, b would be the only candidate: only-one-account.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx, &a, &reading(10.0, 10.0, 0.0));
    read(&fx, &b, &reading(10.0, 60.0, 0.0));
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    let out = best(&fx).unwrap();
    assert_eq!(out.to.map(|r| r.id), Some(a.clone()), "{}", out.message);
    assert_eq!(quarantine_of(&fx, &a), (None, None));
    // Recorded with the switch's own source.
    let events = fx.engine.store().unwrap().events().unwrap();
    let released = events.iter().find(|e| e.kind == "unquarantine").unwrap();
    assert_eq!(
        (released.to_id.as_ref(), released.source.as_str()),
        (Some(&a), "cli")
    );
}

#[test]
fn a_quarantine_still_bound_keeps_the_account_out() {
    let fx = Fx::new();
    let (a, b, c) = three(&fx);
    read(&fx, &a, &reading(10.0, 10.0, 0.0));
    read(&fx, &b, &reading(10.0, 50.0, 0.0));
    read(&fx, &c, &reading(10.0, 90.0, 0.0));
    let bound = vault_fp(&fx, &a);
    fx.quarantine(&a, "invalid_grant", &bound);
    let out = best(&fx).unwrap();
    assert_eq!(out.to.unwrap().id, b);
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("invalid_grant".into()), Some(bound))
    );
}

#[test]
fn a_dead_pick_is_quarantined_and_the_strategy_plans_again_without_it() {
    let fx = Fx::new();
    let (a, b, c) = three(&fx);
    read(&fx, &a, &reading(10.0, 10.0, 0.0));
    read(&fx, &b, &reading(10.0, 50.0, 0.0));
    read(&fx, &c, &reading(10.0, 90.0, 0.0));
    fx.expire_access(&a); // due for freshening (§7.2)
    fx.script_token_error(400, "invalid_grant");
    let out = best(&fx).unwrap();
    assert_eq!(out.to.map(|r| r.id), Some(b));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(quarantine_of(&fx, &a).0.as_deref(), Some("invalid_grant"));
}

#[test]
fn collection_fetches_only_what_the_on_demand_rule_allows() {
    // §8.3: on demand, a reading must be older than 180 s, and due, to be fetched again.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    record_at(&fx, &a, &reading(10.0, 10.0, 0.0), T0 - 100, T0 - 100);
    record_at(&fx, &b, &reading(10.0, 10.0, 0.0), T0 - 200, T0 - 200);
    fx.script_usage(200, usage_fixture()); // b's fetch: 7d at 77 %
    let out = best(&fx).unwrap();
    assert_eq!(
        usage_bearers(&fx),
        ["at-rt-b"],
        "a's reading is 100 s old: not fetched"
    );
    assert_eq!(fx.usage_state(&a).unwrap().fetched_at, Some(T0 - 100));
    assert_eq!(fx.usage_state(&b).unwrap().fetched_at, Some(T0));
    assert_eq!(
        out.to.map(|r| r.id),
        Some(a),
        "ranked on b's new reading, 23 points left, against a's 90; b's old one tied"
    );
}

#[test]
fn model_names_decide_which_scoped_windows_count() {
    // a: 7d at 20 but Fable at 95; b: 7d at 30, Fable unused; c is live at 7d 90.
    let pick = |settings: Settings, models: Option<Vec<&str>>| {
        let fx = Fx::new();
        let (a, b, c) = three(&fx);
        read(&fx, &a, &reading(10.0, 20.0, 95.0));
        read(&fx, &b, &reading(10.0, 30.0, 0.0));
        read(&fx, &c, &reading(10.0, 90.0, 0.0));
        let engine = fx.engine_with_settings(settings);
        let to = engine
            .switch(request(&fx, usage(UsageStrategy::Best, models)))
            .unwrap()
            .to
            .unwrap()
            .id;
        if to == a {
            "a"
        } else if to == b {
            "b"
        } else {
            "?"
        }
    };
    let fable = Settings {
        models: vec!["Fable".into()],
        ..Settings::default()
    };
    assert_eq!(
        pick(Settings::default(), None),
        "a",
        "no model window counts by default"
    );
    assert_eq!(
        pick(Settings::default(), Some(vec!["fable"])),
        "b",
        "--model names it, in any case"
    );
    assert_eq!(pick(Settings::default(), Some(vec!["all"])), "b");
    assert_eq!(
        pick(Settings::default(), Some(vec!["opus"])),
        "a",
        "a model window counts only when named"
    );
    assert_eq!(
        pick(fable.clone(), None),
        "b",
        "autoswitch.models applies without --model"
    );
    assert_eq!(
        pick(fable, Some(vec![])),
        "a",
        "--model overrides it for this switch"
    );
}

/// Review Focus 4: a hotkey pressed twice. The other process lands c → a while this one waits
/// for the mutation lock; this one finds its pick already live and stops there. Planning again
/// from a would move next-available on to b, a third account.
#[cfg(feature = "test-hooks")]
#[test]
fn a_double_fired_strategy_switches_once() {
    for strategy in [UsageStrategy::Best, UsageStrategy::NextAvailable] {
        let fx = Fx::new();
        let (a, b, c) = three(&fx);
        read(&fx, &a, &reading(10.0, 10.0, 0.0));
        read(&fx, &b, &reading(10.0, 50.0, 0.0));
        read(&fx, &c, &reading(10.0, 90.0, 0.0));
        let other = fx.engine_with_env(fx.env.clone());
        let req = request(&fx, usage(strategy, None));
        let first = req.clone();
        fx.engine.on_point(
            "planned",
            Box::new(move || assert!(other.switch(first.clone()).unwrap().switched)),
        );
        let out = fx.engine.switch(req).unwrap();
        assert_eq!(
            (out.switched, out.reason, out.strategy),
            (false, SwitchReason::AlreadyActive, strategy.as_str())
        );
        assert_eq!(out.from.map(|r| r.id), Some(a.clone()));
        assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
        let switches = fx
            .engine
            .store()
            .unwrap()
            .events()
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "switch")
            .count();
        assert_eq!(switches, 1, "{strategy:?}");
    }
}

/// Decision 11: under the locks the ranking is not recomputed. The pick, a, is disabled by
/// another process while this one waits for the mutation lock; planning again from the
/// store's readings lands on b, and nothing more is fetched.
#[cfg(feature = "test-hooks")]
#[test]
fn a_pick_that_stops_being_a_candidate_while_the_switch_waits_is_replaced_from_the_store() {
    let fx = Fx::new();
    let (a, b, c) = three(&fx);
    read(&fx, &a, &reading(10.0, 10.0, 0.0));
    read(&fx, &b, &reading(10.0, 50.0, 0.0));
    read(&fx, &c, &reading(10.0, 90.0, 0.0));
    let other = fx.engine_with_env(fx.env.clone());
    let id = a.clone();
    fx.engine.on_point(
        "planned",
        Box::new(move || {
            other.set_disabled(&id, true).unwrap();
        }),
    );
    let out = best(&fx).unwrap();
    assert_eq!((out.switched, out.to.map(|r| r.id)), (true, Some(b)));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert!(usage_bearers(&fx).is_empty());
}
```

- [ ] **Step 20: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test strategy`
Expected: FAIL to compile:
- `error[E0432]: unresolved import `tagteam_engine::switch::UsageStrategy``
- `error[E0599]: no variant named `Usage` found for enum `SwitchTarget``
- `no variant or associated item named `UsageUnavailable`` (and `AlreadyBest`,
  `CandidatesExhausted`) `found for enum `SwitchReason``

- [ ] **Step 21: Implement the strategies in `switch.rs`**

All edits are in `crates/tagteam-engine/src/switch.rs`.

(a) Replace the imports:

```rust
use tagteam_core::poll::replan_for_role;
use tagteam_core::{
    AccountId, OracleVerdict, OutgoingAction, OutgoingClass, OutgoingFacts, ProviderId,
    decide_outgoing, rotation_order,
};
```

with:

```rust
use tagteam_core::poll::replan_for_role;
use tagteam_core::rank::{
    BestOrder, Candidate, NextAvailable, best_order, binding_window, next_available, span,
};
use tagteam_core::usage::headroom;
use tagteam_core::{
    AccountId, OracleVerdict, OutgoingAction, OutgoingClass, OutgoingFacts, ProviderId, Window,
    decide_outgoing, rotation_order,
};
```

and `use crate::collect::jitter;` with:

```rust
use crate::collect::{CollectMode, jitter};
```

(b) Replace:

```rust
#[derive(Debug, Clone)]
pub enum SwitchTarget {
    Rotation,
    Account(AccountId),
}
```

with:

```rust
/// §9.3's strategies that rank by usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageStrategy {
    Best,
    NextAvailable,
}

impl UsageStrategy {
    /// The strategy as `switch --json` names it (§13.2).
    pub fn as_str(self) -> &'static str {
        match self {
            UsageStrategy::Best => "best",
            UsageStrategy::NextAvailable => "next-available",
        }
    }
}

#[derive(Debug, Clone)]
pub enum SwitchTarget {
    Rotation,
    Account(AccountId),
    /// §9.3: `models` overrides `autoswitch.models` for this switch (`--model`).
    Usage {
        strategy: UsageStrategy,
        models: Option<Vec<String>>,
    },
}

impl SwitchTarget {
    /// Rotation or a usage strategy: the engine chooses the account, so a pick that turns out
    /// dead or quarantined is replaced by planning again, where a named target is refused or
    /// warned about (§7.2, §9.3, §9.4 step 1).
    fn chosen(&self) -> bool {
        !matches!(self, SwitchTarget::Account(_))
    }
}
```

(c) Replace the `SwitchReason` enum and its `impl`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchReason {
    Switched,
    AlreadyActive,
    Activated,
    UnmanagedAccount,
    OnlyOneAccount,
    NoValidTarget,
}

impl SwitchReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SwitchReason::Switched => "switched",
            SwitchReason::AlreadyActive => "already-active",
            SwitchReason::Activated => "activated",
            SwitchReason::UnmanagedAccount => "unmanaged-account",
            SwitchReason::OnlyOneAccount => "only-one-account",
            SwitchReason::NoValidTarget => "no-valid-target",
        }
    }
}
```

with:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchReason {
    Switched,
    AlreadyActive,
    Activated,
    UnmanagedAccount,
    OnlyOneAccount,
    NoValidTarget,
    UsageUnavailable,
    AlreadyBest,
    CandidatesExhausted,
}

impl SwitchReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SwitchReason::Switched => "switched",
            SwitchReason::AlreadyActive => "already-active",
            SwitchReason::Activated => "activated",
            SwitchReason::UnmanagedAccount => "unmanaged-account",
            SwitchReason::OnlyOneAccount => "only-one-account",
            SwitchReason::NoValidTarget => "no-valid-target",
            SwitchReason::UsageUnavailable => "usage-unavailable",
            SwitchReason::AlreadyBest => "already-best",
            SwitchReason::CandidatesExhausted => "candidates-exhausted",
        }
    }
}
```

(d) Replace `strategy_of`:

```rust
fn strategy_of(target: &SwitchTarget) -> &'static str {
    match target {
        SwitchTarget::Rotation => "rotation",
        SwitchTarget::Account(_) => "direct",
    }
}
```

with:

```rust
fn strategy_of(target: &SwitchTarget) -> &'static str {
    match target {
        SwitchTarget::Rotation => "rotation",
        SwitchTarget::Account(_) => "direct",
        SwitchTarget::Usage { strategy, .. } => strategy.as_str(),
    }
}
```

(e) Directly after `const FRESHEN_WINDOW_MS: i64 = 10 * 60 * 1000;`, add:

```rust
/// §9.2: fewer than two candidates, for a rotation or a usage strategy.
const ONLY_ONE: &str = "there is only one switchable account";
/// §9.3: no live login to anchor on, and nothing in the store to activate.
const NO_VALID: &str = "no account can be activated";
/// §9.3 `best`: no candidate's reading can drive a decision (§8.4).
const USAGE_UNAVAILABLE: &str = "no candidate has a usage reading recent enough to rank by";
const ONE_UNRANKED: &str =
    "1 candidate has no usage reading recent enough to rank by; it was not considered";
const LIVE_UNKNOWN: &str =
    "switching to the best known candidate; the live account's usage is unknown";
const NO_LIVE: &str =
    "switching to the best known candidate; there is no managed live login to compare with";
```

(f) Replace the `Plan` struct:

```rust
struct Plan {
    target: AccountRow,
    strategy: &'static str,
    self_switch: bool,
    hint: Option<OracleHint>,
    /// A rotation's accounts the walk read and passed over before its pick (§9.3). Empty for a
    /// direct target.
    walked: Vec<AccountId>,
    /// What freshening the target decided to tell the user (§7.2), carried into the outcome.
    warnings: Vec<String>,
}
```

with:

```rust
struct Plan {
    target: AccountRow,
    strategy: &'static str,
    self_switch: bool,
    hint: Option<OracleHint>,
    /// The live account the plan was made against: a usage strategy's pick stands under the
    /// locks only while it still is (Decision 11).
    anchor: Option<AccountId>,
    /// A rotation's accounts the walk read and passed over before its pick (§9.3). Empty for a
    /// direct target and a usage strategy.
    walked: Vec<AccountId>,
    /// What a usage strategy's ranking tells the user (§9.3): accounts it skipped, candidates
    /// it could not rank, an unknown live headroom. Made afresh with every plan.
    notes: Vec<String>,
    /// What freshening the target decided to tell the user (§7.2), carried into the outcome.
    warnings: Vec<String>,
}
```

(g) Directly after the `Rotation` enum (which ends `Stay(SwitchReason, &'static str),\n}`), add:

```rust
/// What a usage strategy decided (§9.3).
#[allow(clippy::large_enum_variant)]
enum Ranked {
    /// The pick, and what the ranking tells the user.
    To(AccountRow, Vec<String>),
    /// A no-op: its reason, its message, and what the ranking tells the user.
    Stay(SwitchReason, String, Vec<String>),
}

/// A candidate as a usage strategy ranks it: its row and its decision-grade windows (`None`:
/// its reading cannot drive a decision, §8.4).
struct Rated {
    row: AccountRow,
    windows: Option<Vec<Window>>,
}

impl Rated {
    fn headroom(&self, models: &[String]) -> Option<f64> {
        self.windows.as_deref().and_then(|w| headroom(w, models))
    }

    fn candidate(&self, models: &[String]) -> Candidate {
        Candidate {
            position: self.row.position,
            headroom: self.headroom(models),
        }
    }

    /// Its binding window as a message names it: `7d at 77%`.
    fn binding(&self, models: &[String]) -> String {
        binding_text(self.windows.as_deref().unwrap_or_default(), models)
    }

    /// `a@x.co (7d at 77%)`.
    fn described(&self, models: &[String]) -> String {
        format!("{} ({})", self.row.label, self.binding(models))
    }
}

/// §8.2's binding window, as a strategy's message names it: its label and its pct, rounded as
/// `list` rounds it.
fn binding_text(windows: &[Window], models: &[String]) -> String {
    match binding_window(windows, models) {
        Some(w) => format!("{} at {}%", w.label, w.pct.round() as i64),
        None => "usage unknown".to_owned(),
    }
}

/// A walk that found no account to activate ends as a rotation's does (§9.3).
fn nothing_to_activate(live_row: Option<&AccountRow>) -> Ranked {
    match live_row {
        Some(_) => Ranked::Stay(SwitchReason::OnlyOneAccount, ONLY_ONE.into(), Vec::new()),
        None => Ranked::Stay(SwitchReason::NoValidTarget, NO_VALID.into(), Vec::new()),
    }
}
```

(h) In the `Freshened` enum, replace the `Replan` doc:

```rust
    /// A rotation's pick turned out dead and is quarantined now: plan again; the walk skips
    /// it (§9.3).
    Replan,
```

with:

```rust
    /// A rotation's or a usage strategy's pick turned out dead and is quarantined now: plan
    /// again; the walk skips it (§9.3).
    Replan,
```

(i) Replace the whole `rotation` method:

```rust
    /// §9.3 rotation, reading the vault lazily.
    ///
    /// - The candidates are counted from the store: with a managed live anchor and fewer than
    ///   two of them, it stays put (§9.2).
    /// - Each vault is read only when the walk reaches it, and the walk stops at the first one
    ///   that holds a credential. An unreadable one before that could have been the pick, so
    ///   it fails naming the account; no account after the pick is ever read.
    fn rotation(
        &self,
        store: &Store,
        provider: &ProviderId,
        live_row: Option<&AccountRow>,
    ) -> Result<Rotation, EngineError> {
        const ONLY_ONE: &str = "there is only one switchable account";
        let Some(order) = self.candidate_order(store, provider, live_row)? else {
            return Ok(Rotation::Stay(SwitchReason::OnlyOneAccount, ONLY_ONE));
        };
        let mut walked = Vec::new();
        for row in order {
            if self.vault_holds_login(&row)? {
                return Ok(Rotation::To(row, walked));
            }
            walked.push(row.id);
        }
        Ok(match live_row {
            Some(_) => Rotation::Stay(SwitchReason::OnlyOneAccount, ONLY_ONE),
            None => Rotation::Stay(SwitchReason::NoValidTarget, "no account can be activated"),
        })
    }
```

with:

```rust
    /// §9.3's lazy walk, shared by rotation and the usage strategies: reads each vault in
    /// `order` only until one holds a credential, and returns that pick with the accounts it
    /// passed over. An unreadable vault before the pick could have been the pick, so the walk
    /// fails naming the account; no vault after the pick is read.
    fn walk(
        &self,
        order: impl IntoIterator<Item = AccountRow>,
    ) -> Result<Option<(AccountRow, Vec<AccountId>)>, EngineError> {
        let mut walked = Vec::new();
        for row in order {
            if self.vault_holds_login(&row)? {
                return Ok(Some((row, walked)));
            }
            walked.push(row.id);
        }
        Ok(None)
    }

    /// §9.3 rotation, reading the vault lazily (`walk`). The candidates are counted from the
    /// store: with a managed live anchor and fewer than two of them, it stays put (§9.2).
    fn rotation(
        &self,
        store: &Store,
        provider: &ProviderId,
        live_row: Option<&AccountRow>,
    ) -> Result<Rotation, EngineError> {
        let Some(order) = self.candidate_order(store, provider, live_row)? else {
            return Ok(Rotation::Stay(SwitchReason::OnlyOneAccount, ONLY_ONE));
        };
        if let Some((row, walked)) = self.walk(order)? {
            return Ok(Rotation::To(row, walked));
        }
        Ok(match live_row {
            Some(_) => Rotation::Stay(SwitchReason::OnlyOneAccount, ONLY_ONE),
            None => Rotation::Stay(SwitchReason::NoValidTarget, NO_VALID),
        })
    }
```

(j) Directly after `rotation_pick_stands` (it ends with `.is_some_and(|at| order[..at].iter().all(|r| plan.walked.contains(&r.id))))\n    }`),
add:

```rust
    /// §9.3's usage strategies, planned from the store alone: the candidates are a rotation's
    /// (`candidate_order`), each ranked by its decision-grade headroom under `models` (§8.2,
    /// §8.4), and their vaults are read lazily in the strategy's own order (`walk`).
    fn usage_pick(
        &self,
        store: &Store,
        provider: &ProviderId,
        live_row: Option<&AccountRow>,
        strategy: UsageStrategy,
        models: &[String],
    ) -> Result<Ranked, EngineError> {
        let Some(order) = self.candidate_order(store, provider, live_row)? else {
            return Ok(Ranked::Stay(
                SwitchReason::OnlyOneAccount,
                ONLY_ONE.into(),
                Vec::new(),
            ));
        };
        // Only without a managed live login: nothing in the store can be a candidate.
        if order.is_empty() {
            return Ok(Ranked::Stay(
                SwitchReason::NoValidTarget,
                NO_VALID.into(),
                Vec::new(),
            ));
        }
        let rated = order
            .into_iter()
            .map(|row| {
                Ok(Rated {
                    windows: self.decision_windows(&row, models)?,
                    row,
                })
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        match strategy {
            UsageStrategy::Best => self.best_pick(&rated, live_row, models),
            UsageStrategy::NextAvailable => self.next_available_pick(&rated, live_row, models),
        }
    }

    /// §9.3 `best`: the known candidate with the most headroom, if it beats the live
    /// account's; every known one, with a warning, when the live headroom is unknown or there
    /// is no managed live login. A candidate whose headroom is unknown is never picked, and a
    /// warning counts them. A better candidate whose vault holds nothing is not switchable, so
    /// a walk that finds none to activate is `already-best` (or `usage-unavailable` when the
    /// live headroom is unknown).
    fn best_pick(
        &self,
        rated: &[Rated],
        live_row: Option<&AccountRow>,
        models: &[String],
    ) -> Result<Ranked, EngineError> {
        let candidates: Vec<Candidate> = rated.iter().map(|r| r.candidate(models)).collect();
        let at = |position: &u32| rated.iter().find(|r| r.row.position == *position);
        let live = match live_row {
            Some(row) => Some(Rated {
                windows: self.decision_windows(row, models)?,
                row: row.clone(),
            }),
            None => None,
        };
        let live_headroom = live.as_ref().and_then(|l| l.headroom(models));
        let mut notes = match candidates.iter().filter(|c| c.headroom.is_none()).count() {
            0 => Vec::new(),
            1 => vec![ONE_UNRANKED.to_owned()],
            n => vec![format!(
                "{n} candidates have no usage reading recent enough to rank by; they were not considered"
            )],
        };
        Ok(match best_order(live_headroom, &candidates) {
            BestOrder::UsageUnavailable => Ranked::Stay(
                SwitchReason::UsageUnavailable,
                USAGE_UNAVAILABLE.into(),
                Vec::new(),
            ),
            BestOrder::AlreadyBest => {
                let leader = match best_order(None, &candidates) {
                    BestOrder::Try(order) => order.first().and_then(at),
                    _ => None,
                };
                let message = match (&live, leader) {
                    (Some(live), Some(leader)) => format!(
                        "{} already has the most headroom ({}); the best candidate is {}",
                        live.row.label,
                        live.binding(models),
                        leader.described(models)
                    ),
                    _ => "the live account already has the most headroom".to_owned(),
                };
                Ranked::Stay(SwitchReason::AlreadyBest, message, notes)
            }
            BestOrder::Try(order) => {
                match self.walk(order.iter().filter_map(at).map(|r| r.row.clone()))? {
                    Some((pick, _)) => {
                        if live_headroom.is_none() {
                            let why = if live_row.is_some() {
                                LIVE_UNKNOWN
                            } else {
                                NO_LIVE
                            };
                            notes.push(why.to_owned());
                        }
                        Ranked::To(pick, notes)
                    }
                    None => match &live {
                        Some(live) if live_headroom.is_some() => Ranked::Stay(
                            SwitchReason::AlreadyBest,
                            format!(
                                "no candidate with more headroom than {} holds a stored credential",
                                live.described(models)
                            ),
                            notes,
                        ),
                        _ => Ranked::Stay(
                            SwitchReason::UsageUnavailable,
                            "no candidate with a known usage reading holds a stored credential"
                                .into(),
                            notes,
                        ),
                    },
                }
            }
        })
    }

    /// §9.3 `next-available`: the rotation's walk without the candidates known to be at their
    /// limit (§8.2: unknown headroom is never skipped). Each skipped account is named, with its
    /// binding window, in a warning. If every candidate is skipped, `candidates-exhausted`
    /// names them all and when the first of those windows resets.
    fn next_available_pick(
        &self,
        rated: &[Rated],
        live_row: Option<&AccountRow>,
        models: &[String],
    ) -> Result<Ranked, EngineError> {
        let walk: Vec<Candidate> = rated.iter().map(|r| r.candidate(models)).collect();
        let at = |position: &u32| rated.iter().find(|r| r.row.position == *position);
        let (order, skipped) = match next_available(&walk) {
            NextAvailable::Exhausted => {
                let all: Vec<&Rated> = rated.iter().collect();
                return Ok(Ranked::Stay(
                    SwitchReason::CandidatesExhausted,
                    self.exhausted_message(&all, models),
                    Vec::new(),
                ));
            }
            NextAvailable::Try { order, skipped } => (order, skipped),
        };
        let skipped: Vec<&Rated> = skipped.iter().filter_map(at).collect();
        let notes = skipped
            .iter()
            .map(|r| {
                format!(
                    "skipped {} (position {}): at its limit ({})",
                    r.row.label,
                    r.row.position,
                    r.binding(models)
                )
            })
            .collect();
        Ok(
            match self.walk(order.iter().filter_map(at).map(|r| r.row.clone()))? {
                Some((pick, _)) => Ranked::To(pick, notes),
                None if skipped.is_empty() => nothing_to_activate(live_row),
                // §9.3: an account whose vault holds nothing is not switchable, so every
                // switchable candidate was skipped.
                None => Ranked::Stay(
                    SwitchReason::CandidatesExhausted,
                    self.exhausted_message(&skipped, models),
                    Vec::new(),
                ),
            },
        )
    }

    /// `candidates-exhausted`'s message: each candidate with its binding window, then how long
    /// until the earliest of those windows resets (§9.3; §11.2 step 8: the binding window
    /// first, then its reset). No reset is named when none of them has one.
    fn exhausted_message(&self, rated: &[&Rated], models: &[String]) -> String {
        let named: Vec<String> = rated.iter().map(|r| r.described(models)).collect();
        let reset = rated
            .iter()
            .filter_map(|r| {
                binding_window(r.windows.as_deref().unwrap_or_default(), models)?.resets_at
            })
            .min();
        let now_s = self.now_ms().div_euclid(1000);
        let when = reset.map_or_else(String::new, |at| {
            format!("; the earliest reset is in {}", span(at - now_s))
        });
        format!(
            "every candidate is at its limit: {}{when}",
            named.join(", ")
        )
    }
```

(k) Replace the whole `plan` method, from its doc comment `/// The target and the §9.2 special
cases, decided from the current state.` to its closing brace, with:

```rust
    /// The target and the §9.2 special cases, decided from the current state.
    fn plan(
        &self,
        p: &dyn Provider,
        store: &Store,
        req: &SwitchRequest,
        ask: Ask,
    ) -> Result<Planned, EngineError> {
        let strategy = strategy_of(&req.target);
        let (live, live_row) = self.live_row(p, store, &req.provider)?;
        let unmanaged_email = match (&live, &live_row) {
            (Some(i), None) => Some(login_email(i)),
            _ => None,
        };
        let done = |reason: SwitchReason, message: String, warnings: Vec<String>| {
            let mut outcome = noop(
                strategy,
                reason,
                message,
                live_row.clone(),
                unmanaged_email.clone(),
            );
            outcome.warnings = warnings;
            Planned::Done(outcome)
        };
        if let (Some(email), false) = (&unmanaged_email, req.force) {
            return Ok(done(
                SwitchReason::UnmanagedAccount,
                unmanaged_message(email),
                Vec::new(),
            ));
        }
        let mut walked = Vec::new();
        let mut notes = Vec::new();
        let target = match &req.target {
            // A switch never crosses providers (§9.3).
            SwitchTarget::Account(id) => store
                .account(id)?
                .filter(|a| a.provider == req.provider)
                .ok_or_else(|| EngineError::NoSuchAccount(id.to_string()))?,
            SwitchTarget::Rotation => {
                match self.rotation(store, &req.provider, live_row.as_ref())? {
                    Rotation::To(a, passed) => {
                        walked = passed;
                        a
                    }
                    Rotation::Stay(reason, message) => {
                        return Ok(done(reason, message.into(), Vec::new()));
                    }
                }
            }
            // §9.3: `--model` replaces `autoswitch.models` for this switch, when given.
            SwitchTarget::Usage {
                strategy: by,
                models,
            } => {
                let models = models
                    .clone()
                    .unwrap_or_else(|| self.settings().models.clone());
                match self.usage_pick(store, &req.provider, live_row.as_ref(), *by, &models)? {
                    Ranked::To(a, said) => {
                        notes = said;
                        a
                    }
                    Ranked::Stay(reason, message, said) => {
                        return Ok(done(reason, message, said));
                    }
                }
            }
        };
        if !self.has_login(&target)? {
            return Err(EngineError::InvalidInput(format!(
                "{} cannot be activated: it has no stored credential; log in and run `tagteam add` again",
                target.label
            )));
        }
        // The direct branch displaces whatever is live, so it has nothing to ask about.
        let hint = match ask {
            _ if req.force => None,
            Ask::Oracle => live_row.as_ref().and_then(|out| self.oracle_hint(p, out)),
            Ask::Reuse(hint) => hint,
        };
        let self_switch = live_row.as_ref().is_some_and(|r| r.id == target.id);
        if self_switch && !req.force {
            // A no-op unless the live credential diverged from the vault and the oracle
            // attributed it to this very account; then a full switch reconciles it (§9.2).
            let reconcile = Axis::of(p, &target.kind)
                .live_secret(&p.read_live_auth(&self.env))
                .is_some_and(|live| {
                    !self.matches_vault(p, &target, &live)
                        && verdict(answer_for(hint.as_ref(), &live), &target)
                            == OracleVerdict::ThisAccount
                });
            if !reconcile {
                return Ok(done(
                    SwitchReason::AlreadyActive,
                    already_active(&target),
                    Vec::new(),
                ));
            }
        }
        Ok(Planned::Go(Plan {
            anchor: live_row.as_ref().map(|r| r.id.clone()),
            target,
            strategy,
            self_switch,
            hint,
            walked,
            notes,
            warnings: vec![],
        }))
    }
```

(l) In `freshen`, replace:

```rust
        let rotation = matches!(req.target, SwitchTarget::Rotation);
        if target.quarantine_reason.is_some() {
            // Never refreshed (§7.4): usable only while its current access token lasts.
            return match (due, rotation) {
```

with:

```rust
        let chosen = req.target.chosen();
        if target.quarantine_reason.is_some() {
            // Never refreshed (§7.4): usable only while its current access token lasts.
            return match (due, chosen) {
```

and:

```rust
            GateOutcome::Dead(_) if rotation => Freshened::Replan,
```

with:

```rust
            GateOutcome::Dead(_) if chosen => Freshened::Replan,
```

(m) Directly after `without_store` (it ends `Ok(noop(\n            strategy_of(&req.target),
…\n        ))\n    }`), add:

```rust
    /// §9.3, before a usage strategy plans: quarantines that no longer bind are released
    /// (§7.4, Decision 9), then the managed live account and every candidate are collected on
    /// demand (§8.3, Decision 8). Returns the collector's warnings. Other targets do neither,
    /// and neither does a usage strategy over an unmanaged live login without --force:
    /// planning reports that no-op (§9.2), and the network is not used for it.
    fn prepare_usage(
        &self,
        p: &dyn Provider,
        store: &Store,
        req: &SwitchRequest,
    ) -> Result<Vec<String>, EngineError> {
        if !matches!(req.target, SwitchTarget::Usage { .. }) {
            return Ok(Vec::new());
        }
        let (live, live_row) = self.live_row(p, store, &req.provider)?;
        if live.is_some() && live_row.is_none() && !req.force {
            return Ok(Vec::new());
        }
        self.release_unbound_quarantines(&req.provider, req.source)?;
        let mut accounts: Vec<AccountId> = live_row.into_iter().map(|r| r.id).collect();
        for row in store.accounts(&req.provider)? {
            if is_candidate(&row) && !accounts.contains(&row.id) {
                accounts.push(row.id);
            }
        }
        // §8.3: a usage failure is never a command error. An interruption is not a usage
        // failure, and ends the command (§14.1).
        match self.collect_usage(CollectMode::OnDemand { accounts }) {
            Ok(report) => Ok(report.warnings),
            Err(e) if e.signal().is_some() => Err(e),
            Err(e) => Ok(vec![format!("usage was not collected: {e}")]),
        }
    }
```

(n) Replace the whole `switch` method, from its doc comment `/// §9: plan and ask the oracle
without any lock;` to its closing brace after `Err(EngineError::LiveMoved)`, with:

```rust
    /// §9: plan and ask the oracle without any lock; then take the mutation lock once, and
    /// under it the account locks and the live locks, and re-derive every decision. Anything
    /// that moved while this command waited releases every lock but the mutation lock and
    /// plans again, without the network, for at most `ATTEMPTS` lock acquisitions. A usage
    /// strategy first releases quarantines that no longer bind and collects (§9.3); the
    /// collector's warnings lead the outcome's.
    pub fn switch(&self, req: SwitchRequest) -> Result<SwitchOutcome, EngineError> {
        self.refuse_inside_run_shell()?;
        let provider = self.provider(&req.provider)?;
        let p = provider.as_ref();
        if !req.force {
            self.settle_or_refuse(&req.provider)?;
        }
        let Some(store) = self.existing_store()? else {
            return self.without_store(p, &req);
        };
        let mut warnings = self.prepare_usage(p, &store, &req)?;
        let mut outcome = self.switch_planned(p, &store, &req)?;
        warnings.append(&mut outcome.warnings);
        outcome.warnings = warnings;
        Ok(outcome)
    }

    /// `switch` from its first plan on.
    fn switch_planned(
        &self,
        p: &dyn Provider,
        store: &Store,
        req: &SwitchRequest,
    ) -> Result<SwitchOutcome, EngineError> {
        // §7.2: before the mutation lock, the only place a manual switch may use the network.
        let mut plan = match self.plan(p, store, req, Ask::Oracle)? {
            Planned::Done(outcome) => return Ok(outcome),
            Planned::Go(plan) => match self.freshen_plan(p, store, req, plan)? {
                Planned::Done(outcome) => return Ok(outcome),
                Planned::Go(plan) => plan,
            },
        };
        // Once, before the mutation lock: a test callback here may take that lock itself.
        hooks::point(self, "planned")?;
        let guard = if req.force {
            self.mutation_guard()?
        } else {
            self.guard_or_refuse(&req.provider)?
        };
        for attempt in 1..=ATTEMPTS {
            if attempt > 1 {
                // No freshen here: this runs under the mutation lock, where no network is
                // allowed (§4.3). A pick that changed is activated with the vault's
                // generation, and CC refreshes it.
                let warnings = std::mem::take(&mut plan.warnings);
                plan = match self.plan(p, store, req, Ask::Reuse(plan.hint.take()))? {
                    Planned::Done(mut outcome) => {
                        outcome.warnings.extend(warnings);
                        return Ok(outcome);
                    }
                    Planned::Go(mut next) => {
                        next.warnings = warnings;
                        next
                    }
                };
            }
            let (_, outgoing) = self.live_row(p, store, &req.provider)?;
            let mut ids = vec![&plan.target.id];
            if let Some(o) = &outgoing {
                ids.push(&o.id);
            }
            let accounts = self.lock_accounts(&ids)?;
            let locks = p.lock_live(&self.env, &guard)?;
            match self.rederive(p, store, req, &plan, outgoing.as_ref())? {
                Rederived::Go(locked) => {
                    let outcome = self.transact(p, store, &plan, locked, &accounts, &locks, req)?;
                    // The re-plan needs no lock and never fetches (§8.3).
                    drop(locks);
                    drop(accounts);
                    self.replan_polls(p, store, &outcome);
                    return Ok(outcome);
                }
                Rederived::Done(mut outcome) => {
                    outcome.warnings.extend(plan.warnings.clone());
                    return Ok(outcome);
                }
                Rederived::Replan => {}
            }
            // `locks`, then `accounts`, are released here; the mutation lock is kept.
        }
        Err(EngineError::LiveMoved)
    }
```

(o) In `rederive`, replace:

```rust
        // Review Focus 3: another process landed exactly this rotation's target while this one
        // waited for the mutation lock (a double-fired `switch`). That was this command's work;
        // rotating on from there would switch twice. A direct target needs no such rule: planning
        // again finds the self-switch no-op.
        if matches!(req.target, SwitchTarget::Rotation) && self_switch && !plan.self_switch {
```

with:

```rust
        // M1's Review Focus 3, and M3a's Review Focus 4 for a usage strategy: another process
        // landed exactly this plan's pick while this one waited for the mutation lock (a
        // double-fired `switch`). That was this command's work; moving on from there would
        // switch twice. A direct target needs no such rule: planning again finds the
        // self-switch no-op.
        if req.target.chosen() && self_switch && !plan.self_switch {
```

then:

```rust
        if target.quarantine_reason.is_some() {
            if matches!(req.target, SwitchTarget::Rotation) {
                return Ok(Rederived::Replan);
            }
```

with:

```rust
        if target.quarantine_reason.is_some() {
            if req.target.chosen() {
                return Ok(Rederived::Replan);
            }
```

and:

```rust
        let same_pick = match req.target {
            SwitchTarget::Account(_) => true,
            SwitchTarget::Rotation => self.rotation_pick_stands(store, plan, again.as_ref())?,
        };
```

with:

```rust
        let same_pick = match req.target {
            SwitchTarget::Account(_) => true,
            SwitchTarget::Rotation => self.rotation_pick_stands(store, plan, again.as_ref())?,
            // Decision 11: no network under the locks, so the ranking is not recomputed. The
            // pick stands while it is still a candidate and the live login is the account it
            // was ranked against; otherwise planning again ranks from the store's readings.
            SwitchTarget::Usage { .. } => {
                is_candidate(&target) && again.as_ref().map(|r| &r.id) == plan.anchor.as_ref()
            }
        };
```

(p) In `transact`, replace:

```rust
        let mut warnings = plan.warnings.clone();
        warnings.extend(locked_warnings);
```

with:

```rust
        let mut warnings = plan.notes.clone();
        warnings.extend(plan.warnings.iter().cloned());
        warnings.extend(locked_warnings);
```

- [ ] **Step 22: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --features test-hooks --test strategy`
Expected: PASS, 26 tests: B2's 6 and Part C's 20, the two `test-hooks` tests included.

Run: `cargo test -p tagteam-engine --test strategy`
Expected: PASS, 24 tests. The two hook-driven tests are compiled out.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS. Rotation and direct switches behave exactly as before: `prepare_usage` returns
at once for them, `notes` stays empty, `anchor` is read only for a usage target, and
`walk`/`rotation` keep the old walk. So `rotation.rs`, `switch.rs`, `recover.rs`
(`a_double_fired_rotation_switches_once`), `freshen.rs`, `switch_rollback.rs` and
`invariant.rs` all pass unchanged.

- [ ] **Step 23: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 24: Commit**

```bash
git add crates/tagteam-engine/src/switch.rs crates/tagteam-engine/tests/strategy.rs
git commit -m "Switch by usage with the best and next-available strategies"
```

#### Part D: L302, rotation's edge cases

M1's review deferred L302 ("rotation edge cases untested") to M3. These tests pin the current
behaviour, which §9.3 states: candidates are counted from the store as enabled, unquarantined
rows with an identity, the live account included. With a managed live login the walk starts
after the live account's position, disabled or not, and wraps (NOTES 5 and 6).

- [ ] **Step 25: Write the pinning tests**

Append to `crates/tagteam-engine/tests/rotation.rs`:

```rust
#[test]
fn with_every_other_account_disabled_a_live_anchor_is_only_one_account() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live
    fx.engine.set_disabled(&a, true).unwrap();
    fx.engine.set_disabled(&b, true).unwrap();
    let out = rotate(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason),
        (false, SwitchReason::OnlyOneAccount)
    );
    assert_eq!(fx.live_email().as_deref(), Some("c@x.co"));
}

#[test]
fn a_disabled_live_anchor_does_not_count_toward_two() {
    // §9.3: candidates are counted from the store, and a disabled row is not one, the live
    // account included: one other switchable account is still fewer than two.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    fx.engine.set_disabled(&b, true).unwrap();
    let out = rotate(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason),
        (false, SwitchReason::OnlyOneAccount)
    );
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn a_disabled_live_anchor_still_anchors_the_walk() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c");
    fx.switch_to(&b, false).unwrap(); // b live, at position 2
    fx.engine.set_disabled(&b, true).unwrap();
    assert_eq!(
        rotate(&fx).unwrap().to.unwrap().id,
        c,
        "the walk starts after b, not at the first position"
    );
}

#[test]
fn an_anchor_above_every_other_position_wraps_to_the_first() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c"); // live
    fx.engine.move_to(&c, 9).unwrap(); // positions 1, 2 and 9
    assert_eq!(rotate(&fx).unwrap().to.unwrap().id, a);
}
```

- [ ] **Step 26: Run the tests**

Run: `cargo test -p tagteam-engine --features test-hooks --test rotation`
Expected: PASS, the four new tests included. They pass at once: they pin behaviour that
existed but was untested (L302), and must keep passing.

- [ ] **Step 27: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 28: Commit**

```bash
git add crates/tagteam-engine/tests/rotation.rs
git commit -m "Pin rotation's edge cases: disabled anchors and wrap-around"
```

---

### Task 10: `switch --strategy` and `--model`

§13.1: `switch [ACCOUNT] [--strategy best|next-available [--model M]] [--force]`. §9.3:
"Relevant windows follow §8.2 and `autoswitch.models`, or `--model` (a comma-separated list, or
`all`) for this invocation. `--model` without `--strategy` is a usage error." Decision 7 adds:
"`--strategy` and `ACCOUNT` are mutually exclusive, and `--model` requires `--strategy` (§9.3,
cswap's rule). Both are clap usage errors, exit 2." §13.2's `switch` object already has the
fields a strategy needs: `strategy` is `rotation | best | next-available | direct`, `reason`
takes the three new tokens, and `warnings` is an array.

**Readings of the spec this task commits to:**
- **`--model` parsing:** split on commas, trim each name, drop empty names, and pass `all`
  through as written (§8.2's relevance compares it case-insensitively). An empty result
  (`--model ""`) is still an override: no model window counts for this switch.
- **The values of `--strategy`** are clap's kebab-case names of `StrategyArg`: `best` and
  `next-available`. Any other value is a usage error, exit 2.
- **The rendering needs no new branch.** `render::switch_json` already emits `strategy`,
  `reason.as_str()`, `message` and `warnings` from the outcome. `render::switch_human` prints
  the message for any no-op and the "Switched to … (position N)." line for a switch.
  `App::switch` prints every warning to stderr as `warning: …`. So the new reasons, the skip
  warnings and the unknown-usage warnings reach both outputs. This task pins that with a unit
  test and end-to-end tests.
- **Exit codes:** a strategy's no-op exits 0, like rotation's. Under `--json`, a usage error is
  `{"schemaVersion":1,"error":{"type":"usage","message":<clap's kind text>}}` (`lib.rs`).
- **`touches_keychain`** matches `Command::Switch { .. }`, so it already covers the new fields.
  Nothing changes there.
- **One countdown formatter.** `render::duration` becomes a re-export of Task 8's
  `tagteam_core::rank::span`, so `list`'s countdowns and the strategies' messages cannot drift
  apart (CONTRACT ISSUES 1).

**Files:**
- Modify: `crates/tagteam/src/cli.rs`
- Modify: `crates/tagteam/src/app.rs`
- Modify: `crates/tagteam/src/render.rs`
- Create: `crates/tagteam/tests/strategy_cli.rs`

**Interfaces:**
- Consumes:
  - Task 9: `tagteam_engine::switch::{UsageStrategy, SwitchTarget::Usage { strategy, models },
    SwitchReason::{UsageUnavailable, AlreadyBest, CandidatesExhausted}}`, and the messages and
    warnings Task 9's tests pin.
  - Task 8: `tagteam_core::rank::span(secs: i64) -> String`.
  - M2b test helpers (`crates/tagteam/tests/common/mod.rs`): `cmd`, `login`, `seed_home`,
    `now_epoch_s`, `record_reading(root: &Path, id: &str, at_s: i64, windows: &[Window])`, and
    `usage_window(key, label, kind, pct, resets_at: Option<i64>, period_s: Option<i64>) ->
    Window`.
- Produces:
  - `cli.rs`: `Command::Switch { account: Option<String>, force: bool, strategy:
    Option<StrategyArg>, model: Option<String> }`; `#[derive(Clone, Copy, clap::ValueEnum)]
    pub enum StrategyArg { Best, NextAvailable }`.
  - `app.rs`: `fn usage_strategy(s: StrategyArg) -> UsageStrategy`; `fn model_list(arg: &str)
    -> Vec<String>`; `App::switch(&mut self, account: Option<String>, force: bool, strategy:
    Option<StrategyArg>, model: Option<String>) -> Result<(), Failure>`.
  - `render.rs`: `pub(crate) use tagteam_core::rank::span as duration;` (same calls, same
    outputs).

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam/tests/strategy_cli.rs`:

```rust
//! `switch --strategy` and `--model` through the real binary (§9.3, §13.2). Readings are
//! recorded through the store, fresh and with a plan in force, so a strategy's on-demand
//! collection sends nothing; every endpoint stays offline (`std_cmd`'s default). Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::path::Path;

use common::{cmd, login, now_epoch_s, record_reading, seed_home, usage_window};
use serde_json::{Value, json};
use tagteam_core::{CLAUDE_CODE, ProviderId, Window, WindowKind};
use tagteam_engine::store::Store;
use tagteam_provider::{Env, FileKeychain};

/// Logs each email in and stores it with `add`, which sends no usage request; the last one
/// stays live. Returns the ids in position order, read from the store: `list` would collect.
fn accounts(root: &Path, emails: &[&str]) -> Vec<String> {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    for (i, email) in emails.iter().enumerate() {
        login(&env, &kc, email, "", &format!("rt-{i}"));
        cmd(root).arg("add").assert().success();
    }
    let store = Store::open_existing(&env.data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    store
        .accounts(&ProviderId::new(CLAUDE_CODE))
        .unwrap()
        .into_iter()
        .map(|r| r.id.as_str().to_owned())
        .collect()
}

/// A reading taken at `now`: 5h at `five` and 7d at `seven`, the 7d window resetting in
/// 3d09h00m30s. The 30 s keep the countdown on its minute while the binary runs.
fn reading(now: i64, five: f64, seven: f64) -> Vec<Window> {
    vec![
        usage_window(
            "5h",
            "5h",
            WindowKind::Short,
            five,
            Some(now + 9_630),
            Some(18_000),
        ),
        usage_window(
            "7d",
            "7d",
            WindowKind::Long,
            seven,
            Some(now + 291_630),
            Some(604_800),
        ),
    ]
}

/// `tagteam switch <args> --json`, which must succeed: its one JSON object.
fn switch_json(root: &Path, args: &[&str]) -> Value {
    let out = cmd(root)
        .arg("switch")
        .args(args)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&out).unwrap()
}

#[test]
fn best_switches_to_the_candidate_with_the_most_headroom() {
    let d = tempfile::tempdir().unwrap();
    let ids = accounts(d.path(), &["a@x.co", "b@x.co"]);
    let now = now_epoch_s();
    record_reading(d.path(), &ids[0], now, &reading(now, 10.0, 20.0));
    record_reading(d.path(), &ids[1], now, &reading(now, 10.0, 60.0));
    assert_eq!(
        switch_json(d.path(), &["--strategy", "best"]),
        json!({"schemaVersion": 1, "provider": "claude-code", "switched": true, "from": 2,
               "to": 1, "strategy": "best", "reason": "switched", "message": "Switched to a@x.co",
               "credentialStore": "keychain", "warnings": []})
    );
}

#[test]
fn already_best_is_a_no_op_naming_both_binding_windows() {
    let d = tempfile::tempdir().unwrap();
    let ids = accounts(d.path(), &["a@x.co", "b@x.co"]);
    let now = now_epoch_s();
    record_reading(d.path(), &ids[0], now, &reading(now, 10.0, 70.0));
    record_reading(d.path(), &ids[1], now, &reading(now, 10.0, 40.0));
    assert_eq!(
        switch_json(d.path(), &["--strategy", "best"]),
        json!({"schemaVersion": 1, "provider": "claude-code", "switched": false, "from": 2,
               "to": null, "strategy": "best", "reason": "already-best",
               "message": "b@x.co already has the most headroom (7d at 40%); the best candidate is a@x.co (7d at 70%)",
               "credentialStore": null, "warnings": []})
    );
}

#[test]
fn usage_unavailable_when_no_candidate_has_a_reading() {
    // Neither account was read; the strategy's collection fails offline (`pre-send`), and a
    // usage failure is never a command error (§8.3).
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co", "b@x.co"]);
    assert_eq!(
        switch_json(d.path(), &["--strategy", "best"]),
        json!({"schemaVersion": 1, "provider": "claude-code", "switched": false, "from": 2,
               "to": null, "strategy": "best", "reason": "usage-unavailable",
               "message": "no candidate has a usage reading recent enough to rank by",
               "credentialStore": null, "warnings": []})
    );
}

#[test]
fn candidates_exhausted_names_each_candidate_and_the_earliest_reset() {
    let d = tempfile::tempdir().unwrap();
    let ids = accounts(d.path(), &["a@x.co", "b@x.co"]);
    let now = now_epoch_s();
    record_reading(d.path(), &ids[0], now, &reading(now, 10.0, 100.0));
    record_reading(d.path(), &ids[1], now, &reading(now, 10.0, 40.0));
    assert_eq!(
        switch_json(d.path(), &["--strategy", "next-available"]),
        json!({"schemaVersion": 1, "provider": "claude-code", "switched": false, "from": 2,
               "to": null, "strategy": "next-available", "reason": "candidates-exhausted",
               "message": "every candidate is at its limit: a@x.co (7d at 100%); the earliest reset is in 3d09h",
               "credentialStore": null, "warnings": []})
    );
}

#[test]
fn next_available_names_the_binding_window_of_each_account_it_skips() {
    let d = tempfile::tempdir().unwrap();
    let ids = accounts(d.path(), &["a@x.co", "b@x.co", "c@x.co"]);
    let now = now_epoch_s();
    record_reading(d.path(), &ids[0], now, &reading(now, 10.0, 100.0));
    record_reading(d.path(), &ids[1], now, &reading(now, 10.0, 30.0));
    record_reading(d.path(), &ids[2], now, &reading(now, 10.0, 50.0));
    let out = cmd(d.path())
        .args(["switch", "--strategy", "next-available"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "Switched to b@x.co (position 2).\nClaude Code picks this up within about 30 s; restart it to apply now.\n"
    );
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        "warning: skipped a@x.co (position 1): at its limit (7d at 100%)\n"
    );
}

#[test]
fn the_model_flag_counts_a_named_scoped_window() {
    let d = tempfile::tempdir().unwrap();
    let ids = accounts(d.path(), &["a@x.co", "b@x.co"]);
    let now = now_epoch_s();
    let mut a = reading(now, 10.0, 20.0);
    a.push(usage_window(
        "scoped:Fable",
        "Fable",
        WindowKind::Scoped,
        95.0,
        Some(now + 291_630),
        Some(604_800),
    ));
    record_reading(d.path(), &ids[0], now, &a);
    record_reading(d.path(), &ids[1], now, &reading(now, 10.0, 60.0));
    // Trimmed, with the empty name dropped: `Fable` counts, and a has 5 points left.
    let named = switch_json(d.path(), &["--strategy", "best", "--model", " Fable , "]);
    assert_eq!(
        (&named["reason"], &named["message"]),
        (
            &json!("already-best"),
            &json!(
                "b@x.co already has the most headroom (7d at 60%); the best candidate is a@x.co (Fable at 95%)"
            )
        )
    );
    // Without --model, no model window counts (`autoswitch.models` is empty): a has 80.
    let unnamed = switch_json(d.path(), &["--strategy", "best"]);
    assert_eq!(
        (&unnamed["reason"], &unnamed["to"]),
        (&json!("switched"), &json!(1))
    );
}

#[test]
fn model_needs_a_strategy_and_a_strategy_takes_no_account() {
    // Decision 7: both are clap usage errors, exit 2; under --json, one JSON usage error.
    let d = tempfile::tempdir().unwrap();
    let cases: [(&[&str], &str); 2] = [
        (
            &["switch", "--model", "fable"],
            "one or more required arguments were not provided",
        ),
        (
            &["switch", "1", "--strategy", "best"],
            "an argument cannot be used with one or more of the other specified arguments",
        ),
    ];
    for (args, kind) in cases {
        let err = cmd(d.path())
            .args(args)
            .assert()
            .code(2)
            .get_output()
            .stderr
            .clone();
        let err = String::from_utf8(err).unwrap();
        assert!(
            err.starts_with(&format!("error: {kind}")),
            "{args:?}: {err}"
        );
        let out = cmd(d.path())
            .args(args)
            .arg("--json")
            .assert()
            .code(2)
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            json!({"schemaVersion": 1, "error": {"type": "usage", "message": kind}}),
            "{args:?}"
        );
    }
    cmd(d.path())
        .args(["switch", "--strategy", "fastest"])
        .assert()
        .code(2);
}
```

In `crates/tagteam/src/app.rs`'s `#[cfg(test)] mod tests`, append:

```rust
    #[test]
    fn model_lists_are_trimmed_and_drop_empty_names() {
        assert_eq!(model_list("Fable"), ["Fable"]);
        assert_eq!(
            model_list(" Fable , opus ,, all "),
            ["Fable", "opus", "all"]
        );
        assert!(model_list("").is_empty());
        assert!(model_list(" , ").is_empty());
    }

    #[test]
    fn each_strategy_flag_names_its_engine_strategy() {
        assert_eq!(usage_strategy(StrategyArg::Best), UsageStrategy::Best);
        assert_eq!(
            usage_strategy(StrategyArg::NextAvailable),
            UsageStrategy::NextAvailable
        );
    }
```

In `crates/tagteam/src/render.rs`'s `#[cfg(test)] mod tests`, add after
`only_a_fallback_is_a_notice_and_either_file_is_a_file`:

```rust
    #[test]
    fn a_usage_strategy_outcome_renders_its_strategy_reason_and_warnings() {
        let message =
            "every candidate is at its limit: a@x.co (7d at 100%); the earliest reset is in 3d09h";
        let o = SwitchOutcome {
            switched: false,
            strategy: "next-available",
            reason: SwitchReason::CandidatesExhausted,
            message: message.into(),
            warnings: vec!["usage was not collected: the store is locked".into()],
            ..stored(None)
        };
        assert_eq!(
            switch_json(&o, CLAUDE_CODE),
            json!({"schemaVersion": 1, "provider": CLAUDE_CODE, "switched": false, "from": null,
                   "to": null, "strategy": "next-available", "reason": "candidates-exhausted",
                   "message": message, "credentialStore": null,
                   "warnings": ["usage was not collected: the store is locked"]})
        );
        assert_eq!(switch_human(&o), format!("{message}\n"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam --features test-support --test strategy_cli`
Expected: FAIL, 7 tests:
- The six that run a strategy fail on `.success()`. clap does not know `--strategy` yet, so
  the binary exits 2 with `error: unexpected argument found`.
- `model_needs_a_strategy_and_a_strategy_takes_no_account` fails on its first `starts_with`,
  because the message is `error: unexpected argument found`, not a missing required argument.

Run: `cargo test -p tagteam --lib`
Expected: FAIL to compile: `cannot find function `model_list``, `cannot find function
`usage_strategy``, `failed to resolve: use of undeclared type `StrategyArg`` (and
`UsageStrategy`). The render test
compiles and would pass on its own, since the rendering is already generic. It pins that.

- [ ] **Step 3: Add the flags**

In `crates/tagteam/src/cli.rs`, replace:

```rust
    /// Switch to the next account, or to ACCOUNT
    Switch {
        account: Option<String>,
        /// Activate even over an unmanaged live login, displacing it
        #[arg(long)]
        force: bool,
    },
```

with:

```rust
    /// Switch to the next account, to ACCOUNT, or to the one a usage strategy picks
    Switch {
        account: Option<String>,
        /// Activate even over an unmanaged live login, displacing it
        #[arg(long)]
        force: bool,
        /// best or next-available
        #[arg(long, value_enum, conflicts_with = "account")]
        strategy: Option<StrategyArg>,
        /// Model limits that count, comma-separated, or `all`
        #[arg(long, requires = "strategy")]
        model: Option<String>,
    },
```

and add, directly after the `Command` enum's closing brace and before `impl Command {`:

```rust
/// `switch --strategy` (§9.3): the strategies that rank accounts by usage.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum StrategyArg {
    /// The candidate with the most headroom, if it has more than the live account
    Best,
    /// The next account in rotation that is not at its limit
    NextAvailable,
}
```

- [ ] **Step 4: Build the usage target**

In `crates/tagteam/src/app.rs`, replace:

```rust
use tagteam_engine::switch::{SwitchReason, SwitchRequest, SwitchTarget};
```

with:

```rust
use tagteam_engine::switch::{SwitchReason, SwitchRequest, SwitchTarget, UsageStrategy};
```

and:

```rust
use crate::cli::{Cli, Command};
```

with:

```rust
use crate::cli::{Cli, Command, StrategyArg};
```

Add these two functions directly after `render_usage`:

```rust
/// `--strategy`'s value as the engine names it (§9.3).
fn usage_strategy(s: StrategyArg) -> UsageStrategy {
    match s {
        StrategyArg::Best => UsageStrategy::Best,
        StrategyArg::NextAvailable => UsageStrategy::NextAvailable,
    }
}

/// `--model` (§9.3): a comma-separated list of model names, each trimmed, empty ones dropped.
/// `all` passes as written; §8.2's relevance matches it in any case. An empty list is still an
/// override: no model window counts for this switch.
fn model_list(arg: &str) -> Vec<String> {
    arg.split(',')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_owned)
        .collect()
}
```

In `App::dispatch`, replace:

```rust
            Command::Switch { account, force } => self.switch(account, force)?,
```

with:

```rust
            Command::Switch {
                account,
                force,
                strategy,
                model,
            } => self.switch(account, force, strategy, model)?,
```

In `App::switch`, replace its head:

```rust
    fn switch(&mut self, account: Option<String>, force: bool) -> Result<(), Failure> {
        let (target, provider) = match &account {
            Some(a) => {
                let row = self.resolve(a)?;
                (SwitchTarget::Account(row.id), row.provider)
            }
            None => (SwitchTarget::Rotation, self.provider()),
        };
```

with:

```rust
    /// §9: a direct switch to ACCOUNT, a bare rotation, or a usage strategy (§9.3) with its
    /// `--model` override. clap has already refused `--strategy` with an ACCOUNT and `--model`
    /// without `--strategy` (Decision 7).
    fn switch(
        &mut self,
        account: Option<String>,
        force: bool,
        strategy: Option<StrategyArg>,
        model: Option<String>,
    ) -> Result<(), Failure> {
        let (target, provider) = match (&account, strategy) {
            (Some(a), _) => {
                let row = self.resolve(a)?;
                (SwitchTarget::Account(row.id), row.provider)
            }
            (None, Some(s)) => (
                SwitchTarget::Usage {
                    strategy: usage_strategy(s),
                    models: model.as_deref().map(model_list),
                },
                self.provider(),
            ),
            (None, None) => (SwitchTarget::Rotation, self.provider()),
        };
```

The rest of `App::switch` is unchanged. A strategy's outcome goes through the same unmanaged-
login offer, warnings and `render::switch_human`/`switch_json`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tagteam --features test-support --test strategy_cli`
Expected: PASS, 7 tests.

Run: `cargo test -p tagteam --lib`
Expected: PASS, the three new unit tests included.

Run: `cargo test -p tagteam --features test-support`
Expected: PASS. The existing `switch` tests in `app.rs`, `cli.rs`, `kill.rs` and
`gate_race.rs` run bare or direct switches, which build the same targets as before.

- [ ] **Step 6: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/tagteam/src/cli.rs crates/tagteam/src/app.rs crates/tagteam/src/render.rs crates/tagteam/tests/strategy_cli.rs
git commit -m "Add switch --strategy and --model"
```

- [ ] **Step 8: Share one countdown formatter**

In `crates/tagteam/src/render.rs`, replace:

```rust
/// A span of time as `list` and `status` show it: `3d09h`, `2h40m`, `45m`, or `<1m`.
pub(crate) fn duration(secs: i64) -> String {
    let s = secs.max(0);
    let (days, hours, minutes) = (s / 86_400, s % 86_400 / 3_600, s % 3_600 / 60);
    if days > 0 {
        format!("{days}d{hours:02}h")
    } else if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        "<1m".into()
    }
}
```

with:

```rust
// A span of time as `list` and `status` show it: `3d09h`, `2h40m`, `45m`, or `<1m`. The usage
// strategies' messages state a reset in the same words, so there is one formatter.
pub(crate) use tagteam_core::rank::span as duration;
```

`countdown`, `statusline.rs` (`render::duration(now_s - at)`) and `history.rs`
(`render::duration(…)`) keep their calls unchanged.

- [ ] **Step 9: Run the tests**

Run: `cargo test -p tagteam --lib durations_read_as_days_hours_or_minutes`
Expected: PASS. The same ten cases now exercise `span` through the re-export.

Run: `cargo test -p tagteam --features test-support`
Expected: PASS. `list`, `status`, `history` and `statusline` print exactly what they printed
before (`usage_cli.rs`'s `TABLE`, `statusline.rs`, `history.rs` pin it).

- [ ] **Step 10: Run the checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 11: Commit**

```bash
git add crates/tagteam/src/render.rs
git commit -m "Share one countdown formatter between list and the strategies' messages"
```

---

### Task 11: Claude Code's `.storage-write` lock around every credential-entry write

The rule comes from the M4 spec amendments, committed in `1e79bb9`, with Michael's ruling on
Claude Code's dead-token marking, committed in `59d5104`, and his ruling on a rollback's restore,
committed in `f686839`. §9.1's lock table gains a row:

> | CC storage-write lock | `mkdir` | `<secure-storage dir>/.storage-write`, symlinks not resolved | 15 s | 9 s |

and §9.1 says:

> **The storage-write lock** is CC's serialization of every credential write (CC 2.1.286,
> Appendix A.3). CC takes it for each write to its secure storage, including writes that take no
> refresh lock, such as MCP OAuth updates and its dead-token marking. tagteam takes it for every
> write or delete of a CC credential entry: the OAuth entry (Keychain item or
> `.credentials.json`) and the managed-key item. That covers the switch (§9.4 steps 7 and 10),
> recovery (§9.6), the active-token refresh's live write (§7.5) and profile bootstrap (§12.3).
> - It is a leaf lock. It is taken only while the credential locks are held (for a profile, the
>   profile's own), held around one entry's write, and never across a network call; no other
>   lock is taken while it is held. Its wait is a cancellation point (§14.1).
> - Under it, the entry is read again. Its account-scoped keys must still equal what the writer
>   last read or wrote under the credential locks; otherwise the write aborts, and a switch rolls
>   back. CC changes those keys under the credential locks only by refreshing. Outside them it
>   changes them only by its dead-token marking, which writes both tokens empty and `expiresAt`
>   0 (the `Wiped` shape, §9.4 step 4, Appendix A.3).
>   - **A marking is no conflict.** An entry that differs from what the writer last read or
>     wrote only by such a marking does not abort the write, which goes ahead over it. The
>     marking holds no secret. The writer holds the generation CC marked, or a newer one: §7.5's
>     successor or a switch's target.
>   - The machine-shared keys are taken from this read, so a CC write made since the earlier
>     read is never lost.
>   - **A rollback's restore is stricter** (§9.4 step 10). It puts an entry back byte for byte,
>     and only while nothing has written any of the entry's places since tagteam first changed
>     them. A marking or any other write since leaves the whole entry as it is.

The same rule, from the other sections it touches:
- §4.3: "CC's storage-write lock is a leaf: it is taken only under the credential locks, held
  only around one credential entry's write, and nothing else is taken while it is held (§9.1)."
- §9.4 step 7: "Each entry is written under the storage-write lock (§9.1)." Step 10 (rollback),
  besides §9.1's "the switch (§9.4 steps 7 and 10)", now says (`f686839`):

  > - CC can still write the entry without those locks, under the storage-write lock: its
  >   dead-token marking, an MCP token update (§9.1).
  > - So each credential entry is put back byte for byte, to what its places held just before
  >   tagteam first changed them, and only while nothing has written them since. That is
  >   re-read under the storage-write lock.
  > - Otherwise the entry is left exactly as it is. The rollback never merges, and never moves
  >   keys between places.
  >
  > The operation fails with "rolled back", or with "rollback also failed" listing what could
  > not be restored; the journal row then stays for recovery (§9.6), which decides from the
  > live credential.
- §7.5 step 5: "the config lock is taken only around the live write, and the storage-write lock
  around the credential entry's write (§9.1)."
- §14.1's cancellation points: "each iteration of a lock wait: the mutation lock, account locks,
  and the provider's live locks, CC's storage-write lock included"; and its critical spans, which
  contain no cancellation point: the switch transaction (§9.4 steps 6–10), recovery's writes
  (§9.6), and §7.5 "from sending the token request to persisting the successor".
- Appendix A.1: "`<secure-storage dir>/.storage-write`, `proper-lockfile` with stale 15 s and up
  to 10 retries. CC takes it around every write to its secure storage, re-reading the entry
  strictly under it (§9.1, A.3)." A.3: "**Writes** (*2.1.286*) run under the storage-write lock
  (A.1): CC re-reads strictly, applies its change, and writes. … On `invalid_grant` it writes
  both tokens empty and `expiresAt` 0, the `Wiped` shape (§9.4 step 4)."
- Appendix B #61: "Every write of a CC credential entry holds CC's storage-write lock, re-reads
  the entry under it, and keeps the machine-shared keys CC wrote meanwhile (§9.1). It aborts
  when the account-scoped keys changed since the writer last read or wrote them, except by CC's
  dead-token marking (§9.1), which it writes over. A rollback's restore writes over nothing: it
  puts an entry back only while nothing has written it since tagteam did (§9.4 step 10)."

Profile bootstrap (§12.3) is M4's. Today no tagteam write takes the lock, so CC 2.1.286's writes
that take no credential lock (an MCP token refresh, a dead-token marking) can land between
tagteam's read under the credential locks and its write. A switch then writes the MCP token it
read earlier over CC's newer one.

**Readings of the spec this task commits to:**
- **One entry's write is one write of the OAuth entry or of the managed-key item.** Each
  `LiveStore` write holds the lock across everything it does to that one entry: the primary item
  and its hot-reload rewrite of `.credentials.json`; or, when the Keychain refuses, the fallback's
  file write and its deletion and verification of every item (Appendix A.3); or a clear of every
  place a reader tries. That is CC's own unit: its write, its plaintext migration's file delete
  and its fallback all happen under one hold (A.3). Two entries are never written under one
  hold: a switch's OAuth write and its managed-key clear are two holds, and a restore takes one
  for the managed-key item and one for the OAuth entry. A hold per Keychain call or file write
  was rejected: CC could then write between a fallback's file write and its item deletes, and
  the deletes would take CC's MCP token with them.
- **"The entry is read again" reads every place of it, strictly** (A.3): each Keychain item a
  reader tries, in reader order, then, for the OAuth entry, `.credentials.json` (Linux has only
  the file). An item that exists but cannot be read refuses the write.
  - **Each place is checked on its own.** §9.1 holds at every place a write overwrites or
    deletes, so a second item or the file that another writer changed aborts the write, even
    when the first item is untouched. Every place is checked on every write, not only the
    places this write will touch, because a write that falls back deletes every item.
  - **CC's plaintext migration counts as a change at both places.** It moves the entry from the
    file into a new Keychain item (A.3). A write that meets one aborts (a switch rolls back), and
    the next attempt reads the new layout.
- **Account-scoped keys** are every key but the five machine-shared ones (Appendix A.4). Bytes that
  are not a JSON object, such as an API key, count whole. An absent entry and an entry holding only
  machine-shared keys have none, so CC creating an entry for an MCP token where there was none is
  not a move.
- **A marking, exactly.** A.3: CC "writes both tokens empty and `expiresAt` 0". An entry is a
  marking of one the writer last read or wrote when its account-scoped keys equal that one's,
  except that in `claudeAiOauth`, `accessToken` and `refreshToken` are `""` and `expiresAt` is
  `0`, each replaced or added. Every other key, inside `claudeAiOauth` and beside it, is
  unchanged. Machine-shared keys play no part, as everywhere in this comparison. A marking needs
  a `claudeAiOauth` object to mark, so an absent entry, an API key, or an entry of machine-shared
  keys only is never marked. A write goes ahead over a marking, with the machine-shared keys of
  the read under the lock. Any other difference in the account-scoped keys aborts, including a
  marking combined with another change. A restore never writes over a marking (Ruling 1).
- **"What the writer last read or wrote under the credential locks" is kept for the operation, by
  the `LiveStore`, place by place,** as Task 7 keeps the file-mode pin. For each place (each item
  a reader tries, and the file), it is what the operation's latest `snapshot` read there, plus
  every value it set out to write there since, recorded just before each write. `guarded` snapshots
  under the live locks immediately before every provider write, so that snapshot is the writer's
  last read. A place whose account-scoped keys match one of its values, or CC's marking of one,
  may be written over; anything else was written by another writer since, and aborts. It is a
  list, not one value, because a write that fails part-way leaves a place holding either what was
  read or what was being written. An operation that never read the entry has nothing to compare
  and is not refused; only `LiveStore`'s own tests write that way.
  The record ends with the operation: `OperationLocks`' `Drop` (Task 7) now calls
  `end_operation`, which clears it along with the pin. The rejected alternative, a record passed
  to every `LiveStore` write and kept in each undo, changes every writer's signature and about
  forty test calls for the same behaviour.
- **The machine-shared keys come from the read under the lock.** The bytes a write puts at the
  entry carry exactly the machine-shared keys of the entry as CC reads it now (its first place
  that holds anything), their absence included, as §9.4 step 5 composes, whatever the caller
  composed earlier. Bytes that already carry those keys are written unchanged. The hot-reload
  rewrite mirrors the item's bytes (A.3: "with the same bytes"). A clear keeps each place's own
  machine-shared keys, and a restore follows Ruling 1.
- **Ruling 1 (Michael's decision after Codex's round 7, §9.1 and §9.4 step 10 as amended in
  `f686839`): a restore puts a place back byte for byte, and only while nothing else has written
  to the entry since tagteam did.** It replaces
  rounds 3–6's restore rules (rebased machine-shared keys, the login guard, the merge into the
  place CC reads after the restore). There is no merging, no carrying of keys between places,
  and no rebasing on restore.
  - **What a place gets back:** exactly what it actually held just before the operation's first
    change there. Per place, the operation keeps what the place actually holds, apart from what
    a write sets out to put there:
    - it is read under `.storage-write` at the start of each hold;
    - after a write that succeeded, it is what was written;
    - after a write that failed, it is re-read under the same hold, and an unreadable place is
      unknown, never absent.

    So the state put back is one that was read under the lock, or seen written. A place whose
    state before that first change is unknown is never put back.
  - **When:** only while every place of the entry still holds what the operation last knew
    there; a place it deleted must still be absent. That is checked at the start of every hold,
    the restore's included, so another writer's write between two of the operation's writes is
    caught even after the second write carried it on. A place the operation never changed must
    still hold what it last read there.
  - **Otherwise the whole entry is left exactly as it is.** CC or anyone else wrote to it since:
    a dead-token marking, a machine-shared change, anything. The restore:
    - names each changed place in `ProviderError::Incomplete` as `<the item or file> (changed
      since tagteam wrote it; left as it is)`;
    - still restores every other entry and `~/.claude.json`.

    The engine fails with `rollback-failed` ("… and rolling back also failed: …") and keeps
    the journal row.
    §9.6 recovery then decides from the live credential, which is still the one CC wrote to.
  - **A place the operation never changed is never written,** except by the hot-reload rewrite.
    After a restored Keychain item, `.credentials.json` is rewritten with its own bytes when it
    exists and the restore put it back or the operation never changed it (A.3: "if it *already
    exists*, to bump its mtime. Never create it"). It is never created to be bumped, and a file
    left as another writer wrote it is never touched.
  - **Why (Michael's decision).** A rollback never risks a CC write. What it puts back was read
    under the lock, so it already holds every CC write made before tagteam's. A CC write after
    tagteam's leaves the entry alone. §9.4 step 10's "rollback also failed" path and §9.6
    recovery cover the rest. Round 7 showed what any merging risks: round 6 deleted the Keychain
    item, the only copy of CC's refreshed MCP token, before writing the merged file, so a failed
    file write or a death between the two lost it.
  - **Two readings within the rule, both confirmed and now in the spec (`f686839`):**
    - **The whole entry, not each place on its own.** Putting one place back while leaving
      another can hide CC's write behind a place a reader tries first. After a fallback (file
      written, item deleted), CC writes the file; putting the item back would put the original
      login in front of CC's write. Leaving the whole entry keeps CC reading the entry it wrote
      to, and keeps the live credential the one recovery should decide from. Rejected: per
      place (`a_restore_puts_nothing_back_in_front_of_a_place_cc_changed` and
      `a_restore_leaves_the_whole_entry_once_cc_wrote_to_an_item_tagteam_created` fail with it).
    - **"Since the first change", not only "still holds what tagteam last wrote".** Every write's
      machine-shared keys come from its read under the lock, so a write carries on a CC write
      made since tagteam's previous one, and the place ends holding exactly what tagteam last
      wrote. Putting back the state before the first write would then lose CC's write
      (`a_restore_leaves_a_place_cc_wrote_between_two_of_tagteam_s_writes` fails without the
      check). The engine writes each entry once per operation today, so this guards
      `LiveStore`'s contract rather than a switch path.
  - **What the user sees:**
    - **Nothing wrote since:** `rolled-back`, every place byte for byte, and the row is
      deleted. That covers:
      - a fallback (Task 7's `a_rolled_back_fallback_leaves_the_next_switch_on_the_keychain`);
      - a fallback from a file login, which leaves no Keychain item;
      - a clear that kept each place's own MCP token;
      - a CC write made while the switch waited for the lock, which the state read under the
        lock already holds.
    - **CC wrote to the target after the switch wrote it:** `rollback-failed`, "the switch
      failed (…) and rolling back also failed: `<item>` (changed since tagteam wrote it; left as
      it is)". The row stays.
      - A change that keeps the target's login, such as an MCP refresh: the next command's
        recovery finds the target live and finishes the switch forward (§9.6), keeping CC's
        write.
      - A dead-token marking: the marked login names no account, so recovery cannot decide.
        Account-changing commands refuse with `interrupted-switch` until `tagteam switch
        --force` settles it (§9.6). CC had already found the target's token dead, so a fresh
        login is needed either way.
  - A lock that cannot be taken, or an entry that cannot be read under it, leaves that entry
    unrestored and named with the reason.

  Step 10's "writing the original credential back is safe here" holds in the narrower form
  `f686839` gives it: nothing is written back over anything written since.
- **Ruling 2: the wait honours the token of the `Env` the write is given; a restore's never
  does.** By Decision 3 a set token takes no lock, so a write whose wait sees the token set aborts.
  Every write inside a critical span is therefore given an `Env` whose token nothing sets,
  Decision 12's device:
  - the switch's step 7 write (§9.4 steps 6–10), through the new `Engine::critical_env`;
  - recovery's clear of the other axis (§9.6), through `Engine::critical_env`;
  - §7.5's publish after the request, which already waits for the config lock under a fresh
    token (Task 2) and now passes that same `Env` to the write.

  Every restore (the engine's rollback, `guarded`'s own, `Armed`'s while unwinding) waits under a
  token nothing sets, because a rollback always lies inside a critical span. The self-heal's
  publish before any request keeps the process token for both of its waits, as Task 2 ruled:
  nothing was sent that pass, so a signal there stops nothing new. In M3a that is the only
  storage-write wait a signal can end. Profile bootstrap (M4) rules on its own.
- **Ruling 3: an abort, for a change other than a marking at any place, is
  `ProviderError::EntryMoved(<the first such place>)`.** The write leaves that entry
  untouched. `guarded` then restores what the same call already wrote (a switch to OAuth writes
  the OAuth entry before it clears the managed key), and nothing it did not write.
  - A switch reports `rolled-back` ("the switch failed and was rolled back: Claude
    Code-credentials was changed by another writer during tagteam's write; it was left as it is"),
    deletes its journal row and leaves CC's write in place.
  - Recovery's `clear_other_axis` fails; `guard_recovering` logs it, and the row stays for the
    next command, as for any recovery failure. It is not an interruption, so it never ends the
    command.
  - §7.5's publish counts it as not published: the successor is persisted, the outcome is
    `PersistedNotPublished`, and CC's write stands. A marking of the generation the request
    consumed is written over instead, so CC ends on the successor (§7.5 step 5: "so CC always
    holds the newest generation").
- **A leaf, never across the network.** Only `LiveStore`'s write methods take the lock, inside the
  live locks, and they release it before returning; `LiveStore` has no `Http`. The lock stages
  (`lock_credentials`, `lock_config`) never take it. The only code outside `LiveStore` that runs
  while it is held is a fallback's `before_fallback`, which saves the item about to be deleted to
  tagteam's `displaced/` and store: no §9.1 lock, no request. Each write's fence checks the
  storage-write lock's ownership along with the live locks', immediately before every write it
  protects (§9.1).
- **Its own 9 s, its own staleness.** The wait has its own budget (`locks::ACQUIRE_TIMEOUT`, 9 s),
  not a share of the three-lock budget, and a timeout aborts the write like any write failure.
  The lock goes stale after 15 s (A.1); its path is `<secure-storage dir>/.storage-write` as
  `CcPaths` spells the directory, never canonicalized, unlike the legacy lock.
- **The managed-key item.** Its upsert and, when the Keychain refuses, its fallback
  (`primaryApiKey`, then deleting and verifying every managed-key item) run under one hold, in
  today's order. The `approved` splice before it and the removal of `primaryApiKey` after it are
  config writes under the config lock and stay outside. Linux has no managed-key item, so that
  axis takes no storage-write lock there.
- **Four `live_store.rs` tests stood in for tagteam's own write by editing an entry by hand.** A
  restore now leaves alone what the operation did not write, so such an edit reads as CC's. They
  now make that write through `LiveStore`, and keep what they pin.

**Files:**
- Modify: `crates/tagteam-cc/src/paths.rs` (`CcPaths.storage_write_lock`, its test)
- Modify: `crates/tagteam-cc/src/locks.rs` (`STORAGE_WRITE_STALE`, `acquire_storage_write`, tests)
- Modify: `crates/tagteam-provider/src/provider.rs` (`ProviderError::EntryMoved`, `Incomplete`'s doc)
- Modify: `crates/tagteam-cc/src/live.rs` (the lock around every entry write, the operation's
  record, `snapshot`, `restore`)
- Modify: `crates/tagteam-cc/src/provider.rs` (`OperationLocks` ends the operation)
- Modify: `crates/tagteam-engine/src/engine.rs` (`critical_env`), `switch.rs` (`apply`),
  `recover.rs` (`finish_forward`), `active.rs` (`publish`)
- Modify: `crates/tagteam-cc/tests/live_store.rs`, `crates/tagteam-cc/tests/provider.rs`
- Modify: `crates/tagteam-engine/tests/common/mod.rs`, `crates/tagteam-engine/tests/active.rs`
- Modify: `crates/tagteam/tests/signals.rs` (Task 6's binary tests)
- Create: `crates/tagteam-engine/tests/storage_write.rs`

**Interfaces:**
- Consumes:
  - Task 1: `Cancel`, `Env.cancel`, `MkdirLockSpec::with_cancel`, `LockError::Interrupted`
  - Task 2: `Engine::cancel`; `publish`'s `env` (the `Env` carrying the token its config wait
    honours); `locks.rs`'s test helper `interrupted`
  - Task 3: `MkdirLock` (its directory fd, `check_owned` comparing the inode and the mtime)
  - Task 6: `crates/tagteam/tests/signals.rs` and its helpers `send`, `wait_until`, `finish` and
    `store`; the binary tests' `TAGTEAM_TEST_PAUSE_AT` / `TAGTEAM_TEST_PAUSE_DIR` point;
    `tests/common`'s `two_fresh_accounts`, `std_cmd` and `live_email`
  - Task 7: `OperationLocks` and its `Drop`, `LiveStore::unpin_file_mode`, `restore`'s unpin,
    `a_rolled_back_fallback_leaves_the_next_switch_on_the_keychain`
  - Existing: `LiveStore::{snapshot, restore, write_credential_entry,
    clear_credential_account_keys, write_managed_key, clear_managed_key, report_items,
    remove_items}`, `ClaudeCode::guarded`, `Fence`, `shape::{MACHINE_SHARED_KEYS,
    machine_shared_only}`, `read_services`; `live_store.rs`'s `fx`, `fx_with`, `store`,
    `oauth_svc`, `json_of`, `CountingFence`, `save_nothing`, `open`; `tests/provider.rs`'s `Fx`,
    `fx`, `target`, `save_nothing`; the engine's `Fx` (with `with_fallback_items`,
    `put_fallback_item`, `fallback_item`, `add_api_key`), `API_KEY`, `FALLBACK_ITEM`, `journal`,
    `crashed_switch`, `dead_holder`, `write_target_credential`, `mutation_lock_free`,
    `Engine::{on_point, fail_at, mutation_guard}`, `Store::insert_journal` and the hook
    points `after-journal`, `after-credential`, `active-before-request`,
    `active-before-publish`; `tests/active.rs`'s `expire_live` and `active`
- Produces:
  - `CcPaths.storage_write_lock: PathBuf`
  - `tagteam_cc::locks::STORAGE_WRITE_STALE: Duration` (15 s)
  - `tagteam_cc::locks::acquire_storage_write(paths: &CcPaths, timeout: Duration, cancel: &Cancel) -> Result<MkdirLock, LockError>`
  - `ProviderError::EntryMoved(String)`
  - `LiveStore::with_storage_write_timeout(self, d: Duration) -> Self`; crate-private
    `LiveStore::end_operation(&self)`
  - `pub(crate) fn Engine::critical_env(&self) -> Env`
  - test helpers in `tagteam-engine/tests/common`: `CcWrite`, `cc_holds_storage_write_from`,
    `writer_holds_storage_write_from`, `cc_released`, `cc_marks_dead`, `cc_marks_dead_and_more`
  - Behaviour: as the readings say. No `Provider` trait change; FakeAgent has no storage-write
    lock and is unchanged.

- [ ] **Step 1: Write the failing tests for the lock itself**

In `crates/tagteam-cc/src/locks.rs`, add at the end of `mod tests`, after Task 2's
`a_token_set_while_the_config_lock_is_held_ends_the_wait`:

```rust
    #[test]
    fn the_storage_write_lock_is_taken_alone_and_released_on_drop() {
        // §9.1: a leaf, anchored at the secure-storage dir; taking it takes no other lock.
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        assert_eq!(
            p.storage_write_lock,
            p.secure_storage_dir.join(".storage-write")
        );
        let lock = acquire_storage_write(&p, ACQUIRE_TIMEOUT, &Cancel::new()).unwrap();
        assert!(p.storage_write_lock.is_dir());
        assert!(lock.check_owned().is_ok());
        assert!(!p.refresh_lock.exists() && !p.legacy_lock().exists() && !p.config_lock.exists());
        drop(lock);
        assert!(!p.storage_write_lock.exists(), "Drop removes it");
    }

    #[test]
    fn a_held_storage_write_lock_times_out_without_touching_it() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(&p.storage_write_lock).unwrap(); // CC holds it, freshly
        let start = Instant::now();
        match acquire_storage_write(&p, Duration::from_millis(500), &Cancel::new()) {
            Err(LockError::Timeout(path)) => assert_eq!(path, p.storage_write_lock),
            other => panic!("expected a timeout, got {:?}", other.err()),
        }
        assert!(start.elapsed() >= Duration::from_millis(500));
        assert!(p.storage_write_lock.is_dir(), "CC's lock is left alone");
    }

    #[test]
    fn a_storage_write_lock_is_stale_after_15_s() {
        // Appendix A.1: `proper-lockfile` with stale 15 s, as CC takes it.
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(&p.storage_write_lock).unwrap();
        let aged = |secs| {
            fs::File::open(&p.storage_write_lock)
                .unwrap()
                .set_modified(std::time::SystemTime::now() - Duration::from_secs(secs))
                .unwrap()
        };
        aged(13);
        assert!(matches!(
            acquire_storage_write(&p, Duration::from_millis(300), &Cancel::new()),
            Err(LockError::Timeout(_))
        ));
        aged(16);
        let lock = acquire_storage_write(&p, Duration::from_millis(300), &Cancel::new()).unwrap();
        assert!(lock.check_owned().is_ok(), "the stale lock is taken over");
    }

    #[test]
    fn a_signal_ends_the_storage_write_wait_and_a_set_token_takes_nothing() {
        // §9.1: its wait is a cancellation point (§14.1); a set token makes no attempt
        // (Decision 3).
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        let cancel = Cancel::new();
        cancel.request(libc::SIGTERM);
        match acquire_storage_write(&p, ACQUIRE_TIMEOUT, &cancel) {
            Err(LockError::Interrupted { path, signal }) => {
                assert_eq!(
                    (path, signal),
                    (p.storage_write_lock.clone(), libc::SIGTERM)
                )
            }
            other => panic!("expected an interrupted wait, got {:?}", other.err()),
        }
        assert!(!p.storage_write_lock.exists(), "no attempt was made");
        fs::create_dir(&p.storage_write_lock).unwrap(); // CC holds it, freshly
        let named = interrupted(|c| acquire_storage_write(&p, Duration::from_secs(30), c));
        assert_eq!(named, p.storage_write_lock);
        assert!(p.storage_write_lock.is_dir(), "CC's lock is left alone");
    }
```

They use Task 2's `interrupted` helper, and `libc`, a dependency of this crate.

In `crates/tagteam-cc/src/paths.rs`, add at the end of `mod tests`, after
`the_legacy_lock_resolves_symlinks`:

```rust
    #[test]
    fn the_storage_write_lock_sits_in_the_secure_storage_dir_unresolved() {
        // §9.1: `<secure-storage dir>/.storage-write`, symlinks not resolved, unlike the
        // legacy lock.
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real-claude");
        fs::create_dir_all(&real).unwrap();
        fs::create_dir_all(d.path().join("home")).unwrap();
        std::os::unix::fs::symlink(&real, d.path().join("home/.claude")).unwrap();
        let p = CcPaths::resolve(&env(d.path()));
        assert_eq!(
            p.storage_write_lock,
            d.path().join("home/.claude/.storage-write")
        );
        let mut e = env(d.path());
        e.claude_config_dir = Some("/p".into());
        e.claude_securestorage_config_dir = Some("/s".into());
        assert_eq!(
            CcPaths::resolve(&e).storage_write_lock,
            Path::new("/s/.storage-write")
        );
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-cc --lib`
Expected: FAIL to compile: ``no field `storage_write_lock` on type `paths::CcPaths` `` and
``cannot find function `acquire_storage_write` in this scope``.

- [ ] **Step 3: Add the lock and its path**

In `crates/tagteam-cc/src/paths.rs`, replace

```rust
    pub refresh_lock: PathBuf,
    pub config_lock: PathBuf,
}
```

with:

```rust
    pub refresh_lock: PathBuf,
    /// CC's storage-write lock, `<secure-storage dir>/.storage-write`, symlinks not resolved
    /// (§9.1).
    pub storage_write_lock: PathBuf,
    pub config_lock: PathBuf,
}
```

and in `CcPaths::resolve`, replace

```rust
            refresh_lock: env.guard(secure_storage_dir.join(".oauth_refresh.lock")),
```

with:

```rust
            refresh_lock: env.guard(secure_storage_dir.join(".oauth_refresh.lock")),
            storage_write_lock: env.guard(secure_storage_dir.join(".storage-write")),
```

In `crates/tagteam-cc/src/locks.rs`, replace

```rust
pub const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(9);
```

with:

```rust
pub const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(9);
/// CC's storage-write lock goes stale after 15 s (§9.1, Appendix A.1).
pub const STORAGE_WRITE_STALE: Duration = Duration::from_secs(15);
```

and add directly above `#[cfg(test)]`:

```rust
/// CC's storage-write lock (§9.1), waited for up to `timeout` under `cancel` (§14.1). It is a
/// leaf: a caller takes it only while holding the credential locks, holds it around one
/// credential entry's write, and takes no other lock while it is held. Dropping the guard
/// releases it.
pub fn acquire_storage_write(
    paths: &CcPaths,
    timeout: Duration,
    cancel: &Cancel,
) -> Result<MkdirLock, LockError> {
    MkdirLock::acquire(
        &MkdirLockSpec::new(
            paths.storage_write_lock.clone(),
            STORAGE_WRITE_STALE,
            timeout,
        )
        .with_cancel(cancel),
    )
}
```

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p tagteam-cc --lib`
Expected: PASS, the five new tests and every existing one.
`a_storage_write_lock_is_stale_after_15_s` pins the staleness from both sides: 13 s is contended,
16 s is taken over.

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/tagteam-cc/src/locks.rs crates/tagteam-cc/src/paths.rs
git commit -m "Add Claude Code's storage-write lock and its path"
```

- [ ] **Step 6: Write the failing tests for the writes**

In `crates/tagteam-cc/tests/live_store.rs`, four existing tests changed an entry by hand to stand
in for tagteam's own write before restoring. They now make that write through `LiveStore` (the
last of the readings above). Replace the `std` imports:

```rust
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
```

with:

```rust
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
```

In `restore_continues_past_a_non_lock_failure_and_skips_an_already_matching_entry`, replace

```rust
    // Both OAuth items need restoring (a `Some` snapshot, so `upsert`, not `delete`).
    // One managed-key item already matches its snapshot and must receive no write at
    // all — proving the skip — while the OAuth items prove the loop does not stop at
    // the first failure.
    f.kc.put(&oauth[0], &acct, b"orig-primary");
    f.kc.put(&oauth[1], &acct, b"orig-plain");
    f.kc.put(&managed[0], &acct, b"unchanged-managed");
    let snap = s.snapshot(&f.env, &f.paths).unwrap();

    f.kc.put(&oauth[0], &acct, b"target-primary");
    f.kc.put(&oauth[1], &acct, b"target-plain");
    // `managed[0]` is left untouched, so it still equals its snapshot.
```

with:

```rust
    // Both OAuth items need restoring (a `Some` snapshot, so `upsert`, not `delete`).
    // One managed-key item already matches its snapshot and must receive no write at
    // all — proving the skip — while the OAuth items prove the loop does not stop at
    // the first failure.
    let orig_primary = br#"{"claudeAiOauth":{"refreshToken":"orig-primary"}}"#;
    let orig_plain = br#"{"claudeAiOauth":{"refreshToken":"orig-plain"}}"#;
    f.kc.put(&oauth[0], &acct, orig_primary);
    f.kc.put(&oauth[1], &acct, orig_plain);
    f.kc.put(&managed[0], &acct, b"unchanged-managed");
    let snap = s.snapshot(&f.env, &f.paths).unwrap();

    // This operation's own write changes both OAuth items: a restore puts back only what the
    // operation wrote (§9.1). With nothing machine-shared in them, the clear deletes them.
    s.clear_credential_account_keys(&f.env, &f.paths, &open)
        .unwrap();
    // `managed[0]` is left untouched, so it still equals its snapshot.
```

and, further down in the same test, replace

```rust
    assert_eq!(
        f.kc.get(&oauth[0], &acct).unwrap(),
        b"target-primary",
        "the failed restore must leave the target value in place, not corrupt it"
    );
    assert_eq!(
        f.kc.get(&oauth[1], &acct).unwrap(),
        b"orig-plain",
        "the second item must still be restored after the first one failed"
```

with:

```rust
    assert_eq!(
        f.kc.get(&oauth[0], &acct),
        None,
        "the failed restore must leave the target value in place, not corrupt it"
    );
    assert_eq!(
        f.kc.get(&oauth[1], &acct).unwrap(),
        orig_plain,
        "the second item must still be restored after the first one failed"
```

(The clear deleted the primary item, and its failed restore leaves it deleted.)

In `restore_rewrites_a_matching_credentials_file_when_only_the_item_differed`, replace

```rust
    // Change only the Keychain item, directly: the file stays byte-identical to the
    // snapshot, so a naive "skip when it already matches" would never bump it.
    std::thread::sleep(Duration::from_millis(10));
    f.kc.put(&svc, &acct, b"changed-item");
```

with:

```rust
    // This operation's write changes only the Keychain item: its fence trips before the
    // hot-reload rewrite, so the file stays byte-identical to the snapshot, and a naive "skip
    // when it already matches" would never bump it.
    std::thread::sleep(Duration::from_millis(10));
    let cf = CountingFence::new(1);
    let fence = || cf.check();
    assert!(
        s.write_credential_entry(&f.env, &f.paths, b"changed-item", &fence, &mut save_nothing)
            .is_err()
    );
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), b"changed-item");
```

In `restore_never_creates_a_credentials_file_the_snapshot_says_was_absent`, replace

```rust
    // No credentials file at snapshot time.
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    f.kc.put(&svc, &acct, b"changed-item");
```

with:

```rust
    // No credentials file at snapshot time.
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_credential_entry(&f.env, &f.paths, b"changed-item", &open, &mut save_nothing)
        .unwrap();
```

In `restore_forces_the_credentials_file_to_0600`, replace

```rust
    fs::write(&f.paths.credentials_file, "orig-file").unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    fs::write(&f.paths.credentials_file, "tampered").unwrap();
```

with:

```rust
    fs::write(&f.paths.credentials_file, "orig-file").unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_credential_entry(&f.env, &f.paths, b"tampered", &open, &mut save_nothing)
        .unwrap();
```

Then append to the end of the file:

```rust
// --- CC's storage-write lock (§9.1) ----------------------------------------------

/// Wraps a `FakeKeychain` and records, at every `upsert`/`delete`, the item and whether CC's
/// storage-write lock was held at that moment (§9.1).
struct LockProbeKeychain {
    inner: Arc<FakeKeychain>,
    lock: PathBuf,
    writes: Mutex<Vec<(String, bool)>>,
}

impl LockProbeKeychain {
    fn new(inner: Arc<FakeKeychain>, lock: PathBuf) -> Self {
        Self {
            inner,
            lock,
            writes: Mutex::new(Vec::new()),
        }
    }

    fn writes(&self) -> Vec<(String, bool)> {
        self.writes.lock().unwrap().clone()
    }

    fn record(&self, s: &str) {
        let held = self.lock.is_dir();
        self.writes.lock().unwrap().push((s.to_owned(), held));
    }
}

impl Keychain for LockProbeKeychain {
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        self.inner.find(s, a)
    }
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        self.inner.exists(s, a)
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
        self.record(s);
        self.inner.upsert(s, a, d)
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
        self.record(s);
        self.inner.delete(s, a)
    }
    fn lock_state(&self) -> LockState {
        self.inner.lock_state()
    }
    fn unlock(&self) -> bool {
        self.inner.unlock()
    }
}

/// A live OAuth entry as CC leaves it: `rt`'s login, and an MCP server's token `mcp`, which is
/// machine-shared (Appendix A.4).
fn cc_login(rt: &str, mcp: &str) -> Vec<u8> {
    json!({
        "claudeAiOauth": {"accessToken": format!("at-{rt}"), "refreshToken": rt},
        "mcpOAuth": {"srv": {"token": mcp}}
    })
    .to_string()
    .into_bytes()
}

/// CC's dead-token marking of `cc_login(_, mcp)` (Appendix A.3): both tokens empty and
/// `expiresAt` 0. CC makes it without the credential locks, and it is no conflict (§9.1).
fn cc_wiped(mcp: &str) -> Vec<u8> {
    json!({
        "claudeAiOauth": {"accessToken": "", "refreshToken": "", "expiresAt": 0},
        "mcpOAuth": {"srv": {"token": mcp}}
    })
    .to_string()
    .into_bytes()
}

/// A marking together with another account-scoped change, a new `trustedDeviceToken`: not a
/// marking alone, so a write that finds it aborts (§9.1).
fn cc_wiped_and_more(mcp: &str) -> Vec<u8> {
    json!({
        "claudeAiOauth": {"accessToken": "", "refreshToken": "", "expiresAt": 0},
        "trustedDeviceToken": "cc-device",
        "mcpOAuth": {"srv": {"token": mcp}}
    })
    .to_string()
    .into_bytes()
}

/// Claude Code holding its storage-write lock (§9.1): it takes the lock now, runs `write` 300 ms
/// later, then lets go. The thread returns the instant just before it let go.
fn cc_writes_under_the_lock(
    lock: &Path,
    write: impl FnOnce() + Send + 'static,
) -> thread::JoinHandle<Instant> {
    fs::create_dir(lock).unwrap();
    let lock = lock.to_path_buf();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        write();
        let at = Instant::now();
        fs::remove_dir(&lock).unwrap();
        at
    })
}

/// Runs `write` while CC holds the storage-write lock for good: it must wait at least the
/// store's 300 ms and end with a timeout naming the lock.
fn times_out<T: std::fmt::Debug>(
    lock: &Path,
    what: &str,
    write: impl FnOnce() -> Result<T, ProviderError>,
) {
    let start = Instant::now();
    match write() {
        Err(ProviderError::Lock(LockError::Timeout(path))) => assert_eq!(path, lock, "{what}"),
        other => panic!("{what}: expected a timeout on the storage-write lock, got {other:?}"),
    }
    assert!(
        start.elapsed() >= Duration::from_millis(300),
        "{what} gave up without waiting"
    );
}

#[test]
fn every_credential_entry_write_holds_the_storage_write_lock_and_releases_it() {
    // §9.1, B #61: every write and delete of a CC credential entry holds the lock, a restore's
    // included, and releases it when the write returns.
    let f = fx_with(|e| e.claude_config_dir = Some(e.home.join(".claude").into_os_string()));
    let lock = f.paths.storage_write_lock.clone();
    let probe = Arc::new(LockProbeKeychain::new(f.kc.clone(), lock.clone()));
    let s = LiveStore::new(probe.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO);
    let (svc, acct) = oauth_svc(&f);
    let plain = &read_services(&f.env, ItemKind::OAuth)[1];
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    f.kc.put(plain, &acct, &cc_login("rt-old", "m0"));
    fs::write(&f.paths.credentials_file, cc_login("rt-0", "m0")).unwrap();
    fs::write(&f.paths.global_config, "{}").unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    let released = |what: &str| assert!(!lock.exists(), "{what} left the lock behind");

    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    released("an OAuth write and its hot-reload rewrite");
    f.kc.set_fail_write(&svc, true);
    let mut reported_under_the_lock = Vec::new();
    let mut report = |_: &[u8]| {
        reported_under_the_lock.push(lock.is_dir());
        Ok(())
    };
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-2", "m0"),
        &open,
        &mut report,
    )
    .unwrap();
    f.kc.set_fail_write(&svc, false);
    released("a file fallback");
    s.clear_credential_account_keys(&f.env, &f.paths, &open)
        .unwrap();
    released("an OAuth clear");
    s.write_managed_key(
        &f.env,
        &f.paths,
        b"sk-ant-api03-0123456789abcdefghijKLMNOPQRST",
        &open,
        &mut save_nothing,
    )
    .unwrap();
    released("a managed-key write");
    s.clear_managed_key(&f.env, &f.paths, &open).unwrap();
    released("a managed-key delete");
    s.restore(&f.env, &f.paths, &snap, &open).unwrap();
    released("a restore");

    assert_eq!(
        reported_under_the_lock,
        [true, true],
        "a fallback reports both items it deletes under the lock"
    );
    let writes = probe.writes();
    assert!(writes.len() >= 8, "{writes:?}");
    assert!(
        writes.iter().all(|(_, held)| *held),
        "a Keychain write without the storage-write lock: {writes:?}"
    );
}

#[test]
fn a_held_storage_write_lock_makes_every_entry_write_wait_then_time_out() {
    // §9.1: 9 s in production; this store waits 300 ms. Nothing is written, and CC's lock is
    // left alone.
    let f = fx();
    let s = store(&f, Platform::MacOs).with_storage_write_timeout(Duration::from_millis(300));
    let lock = f.paths.storage_write_lock.clone();
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    fs::write(&f.paths.global_config, "{}").unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    let items = f.kc.items();
    fs::create_dir(&lock).unwrap(); // CC holds it, freshly

    times_out(&lock, "an OAuth write", || {
        s.write_credential_entry(
            &f.env,
            &f.paths,
            &cc_login("rt-2", "m0"),
            &open,
            &mut save_nothing,
        )
    });
    times_out(&lock, "an OAuth clear", || {
        s.clear_credential_account_keys(&f.env, &f.paths, &open)
    });
    times_out(&lock, "a managed-key write", || {
        s.write_managed_key(
            &f.env,
            &f.paths,
            b"sk-ant-api03-0123456789abcdefghijKLMNOPQRST",
            &open,
            &mut save_nothing,
        )
    });
    times_out(&lock, "a managed-key delete", || {
        s.clear_managed_key(&f.env, &f.paths, &open)
    });
    // The managed-key write's approval lands in the config first, as before (§9.4 step 7);
    // a switch's rollback restores it. No credential entry was written.
    assert_eq!(f.kc.items(), items, "no Keychain item was written");
    assert!(!f.paths.credentials_file.exists());

    // A restore waits for each entry this operation wrote, here the OAuth entry only, and
    // names the one it could not restore.
    let start = Instant::now();
    match s.restore(&f.env, &f.paths, &snap, &open) {
        Err(ProviderError::Incomplete { failed }) => {
            assert_eq!(failed.len(), 1, "{failed:?}");
            assert!(
                failed[0].starts_with(&svc) && failed[0].contains(".storage-write"),
                "{failed:?}"
            );
        }
        other => panic!("expected the OAuth entry left unrestored, got {other:?}"),
    }
    assert!(start.elapsed() >= Duration::from_millis(300));
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), cc_login("rt-1", "m0"));
    assert!(lock.is_dir(), "CC's lock is left alone");
}

#[test]
fn a_write_waits_for_cc_and_keeps_the_machine_shared_keys_cc_wrote_meanwhile() {
    // §9.1: the machine-shared keys are taken from the read under the lock, so CC's write made
    // since tagteam's earlier read is never lost (B #61).
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    s.snapshot(&f.env, &f.paths).unwrap(); // tagteam's read under the credential locks
    let (kc, cc_svc, cc_acct) = (f.kc.clone(), svc.clone(), acct.clone());
    let cc = cc_writes_under_the_lock(&f.paths.storage_write_lock, move || {
        kc.put(&cc_svc, &cc_acct, &cc_login("rt-0", "m1")) // CC refreshed its MCP token
    });

    // Composed from the earlier read, so it carries the old MCP token.
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    let ended = Instant::now();
    let released = cc.join().unwrap();

    assert!(ended > released, "the write waited for CC's lock");
    assert_eq!(
        json_of(&f.kc.get(&svc, &acct).unwrap()),
        json_of(&cc_login("rt-1", "m1")),
        "the target's login, with the MCP token CC wrote meanwhile"
    );
    assert!(!f.paths.storage_write_lock.exists());
}

#[test]
fn a_write_after_cc_changed_the_account_scoped_keys_aborts_and_writes_nothing() {
    // §9.1: CC changes the account-scoped keys under the credential locks tagteam holds only by
    // refreshing, and outside them only by its dead-token marking. Any other change found under
    // the storage-write lock aborts the write: here a marking plus a new `trustedDeviceToken`.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    fs::write(&f.paths.credentials_file, cc_login("rt-0", "m0")).unwrap();
    s.snapshot(&f.env, &f.paths).unwrap();
    let (kc, cc_svc, cc_acct) = (f.kc.clone(), svc.clone(), acct.clone());
    let cc = cc_writes_under_the_lock(&f.paths.storage_write_lock, move || {
        kc.put(&cc_svc, &cc_acct, &cc_wiped_and_more("m0"))
    });

    let written = s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    );
    cc.join().unwrap();

    match written {
        Err(ProviderError::EntryMoved(name)) => assert_eq!(name, svc),
        other => panic!("expected the write to abort, got {other:?}"),
    }
    // A clear of the same entry aborts the same way.
    assert!(matches!(
        s.clear_credential_account_keys(&f.env, &f.paths, &open),
        Err(ProviderError::EntryMoved(_))
    ));
    assert_eq!(
        f.kc.get(&svc, &acct).unwrap(),
        cc_wiped_and_more("m0"),
        "CC's write stands"
    );
    assert_eq!(
        fs::read(&f.paths.credentials_file).unwrap(),
        cc_login("rt-0", "m0"),
        "nothing was written, the hot-reload file included"
    );
    assert!(!f.paths.storage_write_lock.exists());
}

#[test]
fn a_write_goes_ahead_over_cc_s_dead_token_marking_and_keeps_cc_s_mcp_token() {
    // §9.1: a marking is no conflict. It holds no secret, and the writer holds the generation
    // CC marked or a newer one. The machine-shared keys still come from the read under the
    // lock.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    s.snapshot(&f.env, &f.paths).unwrap();
    let (kc, cc_svc, cc_acct) = (f.kc.clone(), svc.clone(), acct.clone());
    let cc = cc_writes_under_the_lock(&f.paths.storage_write_lock, move || {
        kc.put(&cc_svc, &cc_acct, &cc_wiped("m1"))
    });

    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    let ended = Instant::now();
    let released = cc.join().unwrap();

    assert!(ended > released, "the write waited for CC's lock");
    assert_eq!(
        json_of(&f.kc.get(&svc, &acct).unwrap()),
        json_of(&cc_login("rt-1", "m1")),
        "the target's login over CC's marking, with the MCP token CC wrote"
    );
}

#[test]
fn a_restore_leaves_an_entry_changed_since_tagteam_wrote_it_and_names_it() {
    // A restore puts a place back only while it holds exactly what tagteam last wrote there.
    // CC's write since stays as CC wrote it, whatever it changed: account-scoped keys, its
    // dead-token marking alone, or only an MCP token. The restore names the place and still
    // restores everything else, here `~/.claude.json`.
    for cc in [
        cc_wiped_and_more("m0"),
        cc_wiped("m1"),
        cc_login("rt-1", "m1"),
    ] {
        let f = fx();
        let s = store(&f, Platform::MacOs);
        let (svc, acct) = oauth_svc(&f);
        f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
        fs::write(&f.paths.global_config, "{\"a\": 1}").unwrap();
        let snap = s.snapshot(&f.env, &f.paths).unwrap();
        s.write_credential_entry(
            &f.env,
            &f.paths,
            &cc_login("rt-1", "m0"),
            &open,
            &mut save_nothing,
        )
        .unwrap();
        config::splice_key(&f.paths.global_config, "b", Some(&json!(2)), &open).unwrap();
        f.kc.put(&svc, &acct, &cc);

        match s.restore(&f.env, &f.paths, &snap, &open) {
            Err(ProviderError::Incomplete { failed }) => assert_eq!(
                failed,
                [format!(
                    "{svc} (changed since tagteam wrote it; left as it is)"
                )]
            ),
            other => panic!("expected the item named as left, got {other:?}"),
        }
        assert_eq!(f.kc.get(&svc, &acct).unwrap(), cc, "CC's write stands");
        assert_eq!(fs::read(&f.paths.global_config).unwrap(), b"{\"a\": 1}");
    }
}

#[test]
fn a_restore_leaves_the_whole_entry_once_cc_wrote_to_an_item_tagteam_created() {
    // Codex rounds 6 and 7: the login is in the file, with no Keychain item. tagteam's write
    // creates the item and mirrors the file; CC then refreshes its MCP token in the item, the
    // only place that holds it. The item stays as CC wrote it, and the file as tagteam wrote
    // it: CC keeps reading the one entry it wrote to.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    fs::write(&f.paths.credentials_file, cc_login("rt-0", "m0")).unwrap(); // no item
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    let mirrored = fs::read(&f.paths.credentials_file).unwrap();
    f.kc.put(&svc, &acct, &cc_login("rt-1", "m1")); // CC refreshed its MCP token

    match s.restore(&f.env, &f.paths, &snap, &open) {
        Err(ProviderError::Incomplete { failed }) => assert_eq!(
            failed,
            [format!(
                "{svc} (changed since tagteam wrote it; left as it is)"
            )]
        ),
        other => panic!("expected the item named as left, got {other:?}"),
    }
    assert_eq!(
        f.kc.get(&svc, &acct).unwrap(),
        cc_login("rt-1", "m1"),
        "CC's write stands"
    );
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), mirrored);
}

#[test]
fn a_restore_puts_nothing_back_in_front_of_a_place_cc_changed() {
    // tagteam's write fell back: it wrote the file and deleted the item. CC then wrote the file,
    // its own Keychain write failing too. Putting the item back would hide CC's write, since a
    // reader tries the item first: the whole entry stays as it is, and the file is named.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    f.kc.set_fail_write(&svc, true);
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    f.kc.set_fail_write(&svc, false);
    fs::write(&f.paths.credentials_file, cc_login("rt-1", "m1")).unwrap(); // CC's write

    match s.restore(&f.env, &f.paths, &snap, &open) {
        Err(ProviderError::Incomplete { failed }) => assert_eq!(
            failed,
            [format!(
                "{} (changed since tagteam wrote it; left as it is)",
                f.paths.credentials_file.display()
            )]
        ),
        other => panic!("expected the file named as left, got {other:?}"),
    }
    assert_eq!(f.kc.get(&svc, &acct), None, "no item hides CC's write");
    assert_eq!(
        fs::read(&f.paths.credentials_file).unwrap(),
        cc_login("rt-1", "m1")
    );
}

#[test]
fn a_restore_leaves_a_place_cc_wrote_between_two_of_tagteam_s_writes() {
    // CC refreshed its MCP token in the item between two of tagteam's writes, and the second
    // carried it on. The item holds exactly what tagteam last wrote, but what it held before
    // tagteam's first write predates CC's token: putting that back would lose the token, so
    // the item is left and named.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    f.kc.put(&svc, &acct, &cc_login("rt-1", "m1")); // CC refreshed its MCP token
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-2", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    let last = f.kc.get(&svc, &acct).unwrap();

    match s.restore(&f.env, &f.paths, &snap, &open) {
        Err(ProviderError::Incomplete { failed }) => assert_eq!(
            failed,
            [format!(
                "{svc} (changed since tagteam wrote it; left as it is)"
            )]
        ),
        other => panic!("expected the item named as left, got {other:?}"),
    }
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), last);
}

#[test]
fn a_restore_leaves_alone_an_entry_this_operation_never_wrote() {
    // A restore undoes this operation's writes only: CC's change to an entry tagteam did not
    // write is neither overwritten nor a failure.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    let managed = keychain_service(&f.env, ItemKind::ManagedKey);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    fs::write(&f.paths.global_config, "{}").unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_managed_key(
        &f.env,
        &f.paths,
        b"sk-ant-api03-0123456789abcdefghijKLMNOPQRST",
        &open,
        &mut save_nothing,
    )
    .unwrap();
    f.kc.put(&svc, &acct, &cc_wiped("m0"));

    s.restore(&f.env, &f.paths, &snap, &open).unwrap();

    assert_eq!(f.kc.get(&managed, &acct), None, "the managed key is undone");
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), cc_wiped("m0"));
}

#[test]
fn a_signal_ends_a_writes_wait_for_the_storage_write_lock_but_never_a_restores() {
    // §9.1, §14.1: the wait is a cancellation point for a write given a token that is set; a
    // restore is a rollback, so it waits under a token nothing sets and runs to completion.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let lock = f.paths.storage_write_lock.clone();
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    fs::create_dir(&lock).unwrap(); // CC holds it
    let cancel = f.env.cancel.clone();
    let ctrl_c = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        cancel.request(libc::SIGINT);
    });

    let start = Instant::now();
    let written = s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-2", "m0"),
        &open,
        &mut save_nothing,
    );
    ctrl_c.join().unwrap();

    match written {
        Err(ProviderError::Lock(LockError::Interrupted { path, signal })) => {
            assert_eq!((path, signal), (lock.clone(), libc::SIGINT))
        }
        other => panic!("expected an interrupted wait, got {other:?}"),
    }
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "not the 9 s budget"
    );
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), cc_login("rt-1", "m0"));
    assert!(lock.is_dir(), "CC's lock is left alone");

    let cc = cc_lets_go_soon(&lock);
    s.restore(&f.env, &f.paths, &snap, &open).unwrap();
    cc.join().unwrap();
    assert_eq!(
        f.kc.get(&svc, &acct).unwrap(),
        cc_login("rt-0", "m0"),
        "the restore waited for CC with the signal set, and ran"
    );
}

/// CC letting go of a storage-write lock it holds, 300 ms from now.
fn cc_lets_go_soon(lock: &Path) -> thread::JoinHandle<()> {
    let lock = lock.to_path_buf();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        fs::remove_dir(&lock).unwrap();
    })
}

/// `fx()` with an explicit `CLAUDE_CONFIG_DIR=~/.claude`: readers also try the unsuffixed item
/// (Appendix A.2), so the OAuth entry has a second item. Returns it with its service.
fn fx_with_fallback_item() -> (Fx, String) {
    let f = fx_with(|e| e.claude_config_dir = Some(e.home.join(".claude").into_os_string()));
    let plain = read_services(&f.env, ItemKind::OAuth)[1].clone();
    (f, plain)
}

#[test]
fn a_clear_aborts_when_another_writer_changed_a_fallback_item() {
    // §9.1 at every place a write overwrites or deletes: the second item a reader tries changed
    // while tagteam waited for the lock, so the clear touches no place.
    let (f, plain) = fx_with_fallback_item();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    f.kc.put(&plain, &acct, &cc_login("rt-0", "m0"));
    s.snapshot(&f.env, &f.paths).unwrap();
    let (kc, other, other_acct) = (f.kc.clone(), plain.clone(), acct.clone());
    let cc = cc_writes_under_the_lock(&f.paths.storage_write_lock, move || {
        kc.put(&other, &other_acct, &cc_login("rt-other", "m0"))
    });

    let cleared = s.clear_credential_account_keys(&f.env, &f.paths, &open);
    cc.join().unwrap();

    match cleared {
        Err(ProviderError::EntryMoved(name)) => assert_eq!(name, plain),
        other => panic!("expected the clear to abort, got {other:?}"),
    }
    assert_eq!(f.kc.get(&plain, &acct).unwrap(), cc_login("rt-other", "m0"));
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), cc_login("rt-0", "m0"));
}

#[test]
fn a_clear_goes_ahead_over_cc_s_marking_of_a_fallback_item() {
    // §9.1: a marking is no conflict at any place, the second item included.
    let (f, plain) = fx_with_fallback_item();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    f.kc.put(&plain, &acct, &cc_login("rt-0", "m0"));
    s.snapshot(&f.env, &f.paths).unwrap();
    let (kc, other, other_acct) = (f.kc.clone(), plain.clone(), acct.clone());
    let cc = cc_writes_under_the_lock(&f.paths.storage_write_lock, move || {
        kc.put(&other, &other_acct, &cc_wiped("m1"))
    });

    s.clear_credential_account_keys(&f.env, &f.paths, &open)
        .unwrap();
    let ended = Instant::now();
    let released = cc.join().unwrap();

    assert!(ended > released, "the clear waited for CC's lock");
    for (place, mcp) in [(&svc, "m0"), (&plain, "m1")] {
        assert_eq!(
            json_of(&f.kc.get(place, &acct).unwrap()),
            json!({"mcpOAuth": {"srv": {"token": mcp}}}),
            "{place} keeps only its own machine-shared keys"
        );
    }
}

#[test]
fn a_write_aborts_when_another_writer_changed_the_credentials_file() {
    // The hot-reload rewrite would overwrite the file, so it is checked like the item.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    fs::write(&f.paths.credentials_file, cc_login("rt-0", "m0")).unwrap();
    s.snapshot(&f.env, &f.paths).unwrap();
    let file = f.paths.credentials_file.clone();
    let cc = cc_writes_under_the_lock(&f.paths.storage_write_lock, move || {
        fs::write(&file, cc_login("rt-other", "m0")).unwrap()
    });

    let written = s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    );
    cc.join().unwrap();

    match written {
        Err(ProviderError::EntryMoved(name)) => {
            assert_eq!(name, f.paths.credentials_file.display().to_string())
        }
        other => panic!("expected the write to abort, got {other:?}"),
    }
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), cc_login("rt-0", "m0"));
    assert_eq!(
        fs::read(&f.paths.credentials_file).unwrap(),
        cc_login("rt-other", "m0")
    );
}

#[test]
fn a_write_goes_ahead_over_a_marking_of_the_credentials_file() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    fs::write(&f.paths.credentials_file, cc_login("rt-0", "m0")).unwrap();
    s.snapshot(&f.env, &f.paths).unwrap();
    let file = f.paths.credentials_file.clone();
    let cc = cc_writes_under_the_lock(&f.paths.storage_write_lock, move || {
        fs::write(&file, cc_wiped("m0")).unwrap()
    });

    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    let ended = Instant::now();
    let released = cc.join().unwrap();

    assert!(ended > released, "the write waited for CC's lock");
    let item = f.kc.get(&svc, &acct).unwrap();
    assert_eq!(json_of(&item), json_of(&cc_login("rt-1", "m0")));
    assert_eq!(
        fs::read(&f.paths.credentials_file).unwrap(),
        item,
        "the file mirrors the item again"
    );
}

#[test]
fn a_restore_puts_each_place_back_byte_for_byte() {
    // Each place goes back to exactly what it held before tagteam changed it, its own
    // machine-shared keys included: the item and the file hold different MCP tokens, and a
    // clear keeps each one's. Nothing else wrote in between.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "item"));
    fs::write(&f.paths.credentials_file, cc_login("rt-0", "file")).unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.clear_credential_account_keys(&f.env, &f.paths, &open)
        .unwrap();

    s.restore(&f.env, &f.paths, &snap, &open).unwrap();

    assert_eq!(f.kc.get(&svc, &acct).unwrap(), cc_login("rt-0", "item"));
    assert_eq!(
        fs::read(&f.paths.credentials_file).unwrap(),
        cc_login("rt-0", "file")
    );
}
```

In `crates/tagteam-cc/tests/provider.rs`, replace the `std` imports

```rust
use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant};
```

with

```rust
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
```

and the `tagteam_provider` imports (as Task 7 left them)

```rust
use tagteam_provider::{
    Capabilities, Credential, Env, FakeKeychain, KindTraits, LiveLocks, LockError, MutationGuard,
    Pace, PollBudget, Provider, ProviderError, Read, SecretStore, StoredLogin, UsageResult, Window,
};
```

with

```rust
use tagteam_provider::{
    Capabilities, Credential, Env, FakeKeychain, Keychain, KeychainError, KindTraits, LiveLocks,
    LockError, LockState, MutationGuard, Pace, PollBudget, Provider, ProviderError, Read,
    SecretStore, StoredLogin, UsageResult, Window,
};
```

Then append to the end of the file:

```rust
/// Wraps a `FakeKeychain` and records, at every `upsert`/`delete`, whether CC's storage-write
/// lock, refresh lock, legacy lock and config lock were each held at that moment (§4.3, §9.1).
struct HeldLocksProbe {
    inner: Arc<FakeKeychain>,
    locks: [PathBuf; 4],
    writes: Mutex<Vec<(String, [bool; 4])>>,
}

impl HeldLocksProbe {
    fn record(&self, s: &str) {
        let held = self.locks.clone().map(|l| l.is_dir());
        self.writes.lock().unwrap().push((s.to_owned(), held));
    }
}

impl Keychain for HeldLocksProbe {
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        self.inner.find(s, a)
    }
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        self.inner.exists(s, a)
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
        self.record(s);
        self.inner.upsert(s, a, d)
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
        self.record(s);
        self.inner.delete(s, a)
    }
    fn lock_state(&self) -> LockState {
        self.inner.lock_state()
    }
    fn unlock(&self) -> bool {
        self.inner.unlock()
    }
}

/// `fx()`, with the provider's Keychain behind a `HeldLocksProbe`.
fn probed_fx() -> (Fx, Arc<HeldLocksProbe>) {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    let paths = CcPaths::resolve(&env);
    let kc = Arc::new(FakeKeychain::new());
    let probe = Arc::new(HeldLocksProbe {
        inner: kc.clone(),
        locks: [
            paths.storage_write_lock.clone(),
            paths.refresh_lock.clone(),
            paths.legacy_lock(),
            paths.config_lock,
        ],
        writes: Mutex::new(Vec::new()),
    });
    let cc = ClaudeCode::with_store(
        LiveStore::new(probe.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO),
    );
    (Fx { _d: d, env, kc, cc }, probe)
}

#[test]
fn the_storage_write_lock_is_a_leaf_taken_only_around_each_entry_write() {
    // §4.3, §9.1: taken only while CC's live locks are held, never by the lock stages
    // themselves, and released after each credential entry's write, a rollback's included.
    let (f, probe) = probed_fx();
    let paths = CcPaths::resolve(&f.env);
    fs::write(&paths.global_config, "{}").unwrap();
    f.kc.put(
        &keychain_service(&f.env, ItemKind::OAuth),
        &keychain_account(&f.env),
        br#"{"claudeAiOauth":{"refreshToken":"old"},"mcpOAuth":{"m":1}}"#,
    );
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let cred =
        f.cc.lock_credentials(&f.env, &g, Duration::from_secs(1))
            .unwrap();
    assert!(
        !paths.storage_write_lock.exists(),
        "the credential locks never take it"
    );
    let locks =
        f.cc.lock_config(&f.env, cred, Duration::from_secs(1))
            .unwrap();
    assert!(
        !paths.storage_write_lock.exists(),
        "nor does the config lock"
    );

    let key = StoredLogin {
        kind: "api_key".into(),
        secret: b"sk-ant-api03-abcdefghijklmnopqrstuvwxyz".to_vec(),
        identity: f.cc.token_identity("api-key-1@token.local"),
    };
    let undo =
        f.cc.write_credential(&f.env, &locks, &key, &mut save_nothing)
            .unwrap()
            .undo;
    assert!(!paths.storage_write_lock.exists());
    undo.undo(&locks).unwrap();
    assert!(!paths.storage_write_lock.exists());
    f.cc.write_credential(
        &f.env,
        &locks,
        &target(&f, "new@b.co", "rt-new"),
        &mut save_nothing,
    )
    .unwrap();
    assert!(!paths.storage_write_lock.exists());
    drop(locks);

    let writes = probe.writes.lock().unwrap().clone();
    assert!(writes.len() >= 4, "{writes:?}");
    assert!(
        writes.iter().all(|(_, held)| *held == [true; 4]),
        "a credential entry written without the storage-write lock or the live locks: {writes:?}"
    );
}

#[test]
fn a_write_that_finds_cc_changed_the_entry_aborts_and_leaves_everything_as_cc_left_it() {
    // §9.1: while tagteam waits for its storage-write lock, CC changes the entry's
    // account-scoped keys by more than a dead-token marking. tagteam's write aborts; having
    // written nothing, it restores nothing either, so the error is the abort itself, not a
    // failed restore.
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::write(&paths.global_config, "{}").unwrap();
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    let acct = keychain_account(&f.env);
    f.kc.put(
        &svc,
        &acct,
        br#"{"claudeAiOauth":{"refreshToken":"old"},"mcpOAuth":{"m":1}}"#,
    );
    let changed: &[u8] = br#"{"claudeAiOauth":{"accessToken":"","refreshToken":"","expiresAt":0},"trustedDeviceToken":"cc-device","mcpOAuth":{"m":1}}"#;
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    fs::create_dir(&paths.storage_write_lock).unwrap(); // CC takes it
    let (kc, cc_svc, cc_acct, lock) = (
        f.kc.clone(),
        svc.clone(),
        acct.clone(),
        paths.storage_write_lock.clone(),
    );
    let cc = thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        kc.put(&cc_svc, &cc_acct, changed);
        fs::remove_dir(&lock).unwrap();
    });

    let err =
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "new@b.co", "rt-new"),
            &mut save_nothing,
        )
        .err()
        .unwrap();
    cc.join().unwrap();

    assert!(
        matches!(&err, ProviderError::EntryMoved(name) if *name == svc),
        "{err}"
    );
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), changed, "CC's write stands");
    assert!(
        f.kc.get(&keychain_service(&f.env, ItemKind::ManagedKey), &acct)
            .is_none()
    );
    assert!(!paths.credentials_file.exists());
    assert_eq!(fs::read_to_string(&paths.global_config).unwrap(), "{}");
    assert!(!paths.storage_write_lock.exists());
}
```

Append to the end of `crates/tagteam-engine/tests/common/mod.rs`:

```rust
/// The thread `cc_holds_storage_write_from` starts; it returns the instant just before CC let go
/// of its storage-write lock.
#[cfg(feature = "test-hooks")]
pub type CcWrite = Arc<Mutex<Option<std::thread::JoinHandle<std::time::Instant>>>>;

/// Claude Code holding its storage-write lock (§9.1) from the engine's hook `point` on: it takes
/// the lock there; then, on another thread, it sets the engine's cancel token to SIGINT after
/// `ctrl_c` when given, applies `cc_write` to the live OAuth item 300 ms in, and lets go.
#[cfg(feature = "test-hooks")]
pub fn cc_holds_storage_write_from(
    fx: &Fx,
    point: &'static str,
    ctrl_c: Option<Duration>,
    cc_write: impl Fn(&mut Value) + Send + Sync + 'static,
) -> CcWrite {
    let (svc, _) = fx.live_item(ItemKind::OAuth);
    writer_holds_storage_write_from(fx, point, ctrl_c, &svc, cc_write)
}

/// `cc_holds_storage_write_from` for the Keychain item `svc`, which need not be the one Claude
/// Code reads first: what another writer holding the storage-write lock does to it.
#[cfg(feature = "test-hooks")]
pub fn writer_holds_storage_write_from(
    fx: &Fx,
    point: &'static str,
    ctrl_c: Option<Duration>,
    svc: &str,
    cc_write: impl Fn(&mut Value) + Send + Sync + 'static,
) -> CcWrite {
    let lock = fx.paths().storage_write_lock;
    let (kc, svc, acct) = (fx.kc.clone(), svc.to_owned(), keychain_account(&fx.env));
    let cancel = fx.engine.cancel().clone();
    let cc_write = Arc::new(cc_write);
    let cc: CcWrite = Arc::new(Mutex::new(None));
    let started = cc.clone();
    fx.engine.on_point(
        point,
        Box::new(move || {
            fs::create_dir(&lock).unwrap();
            let (lock, kc, svc, acct) = (lock.clone(), kc.clone(), svc.clone(), acct.clone());
            let (cancel, cc_write) = (cancel.clone(), cc_write.clone());
            *started.lock().unwrap() = Some(std::thread::spawn(move || {
                let begun = std::time::Instant::now();
                if let Some(after) = ctrl_c {
                    std::thread::sleep(after);
                    cancel.request(libc::SIGINT);
                }
                std::thread::sleep(Duration::from_millis(300).saturating_sub(begun.elapsed()));
                let mut live: Value =
                    serde_json::from_slice(&kc.get(&svc, &acct).unwrap()).unwrap();
                cc_write(&mut live);
                kc.put(&svc, &acct, live.to_string().as_bytes());
                let at = std::time::Instant::now();
                fs::remove_dir(&lock).unwrap();
                at
            }));
        }),
    );
    cc
}

/// The instant CC let go of its storage-write lock (`cc_holds_storage_write_from`).
#[cfg(feature = "test-hooks")]
pub fn cc_released(cc: &CcWrite) -> std::time::Instant {
    cc.lock()
        .unwrap()
        .take()
        .expect("the hook point was reached")
        .join()
        .unwrap()
}

/// CC's dead-token marking (Appendix A.3): both tokens empty and `expiresAt` 0, a write CC
/// makes without its credential locks. It is no conflict (§9.1).
pub fn cc_marks_dead(live: &mut Value) {
    live["claudeAiOauth"]["accessToken"] = json!("");
    live["claudeAiOauth"]["refreshToken"] = json!("");
    live["claudeAiOauth"]["expiresAt"] = json!(0);
}

/// A marking together with another account-scoped change, a new `trustedDeviceToken`: not a
/// marking alone, so a write that finds it aborts (§9.1).
pub fn cc_marks_dead_and_more(live: &mut Value) {
    cc_marks_dead(live);
    live["trustedDeviceToken"] = json!("cc-device");
}
```

`libc` is already a dev-dependency of `tagteam-engine` (Task 2's `tests/cancel.rs` uses it).

Create `crates/tagteam-engine/tests/storage_write.rs`:

```rust
//! Claude Code's storage-write lock (§9.1) as a switch and a recovery meet it. Every write of a
//! CC credential entry waits for CC's own; under it the entry is read again. CC's dead-token
//! marking is written over; any other change to the account-scoped keys aborts the write, and
//! the switch rolls back; the machine-shared keys CC wrote meanwhile are kept. A rollback puts a
//! place back only while it holds exactly what the switch wrote there: once CC wrote to the
//! entry since, the rollback leaves it and keeps the row for recovery (§9.4 step 10, §9.6). The
//! waits inside a critical span are never cut short by a signal (§14.1).
#![cfg(feature = "test-hooks")]

mod common;

use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use common::{
    API_KEY, FALLBACK_ITEM, Fx, cc_holds_storage_write_from, cc_marks_dead, cc_marks_dead_and_more,
    cc_released, crashed_switch, dead_holder, journal, mutation_lock_free, write_target_credential,
    writer_holds_storage_write_from,
};
use serde_json::{Value, json};
use tagteam_cc::ItemKind;
use tagteam_engine::EngineError;
use tagteam_provider::Keychain;

/// The command whose rollback failed exits: its journal row is now a dead holder's, so the next
/// command recovers it first (§9.6, §12.6).
fn the_command_exits(fx: &Fx) {
    let mut row = journal(fx).unwrap();
    row.holder = dead_holder();
    fx.engine.store().unwrap().insert_journal(&row).unwrap();
    fx.engine.fail_at(None);
}

/// Claude Code changing the live credential with `cc_write` once the switch has written its
/// target (the `after-credential` point), before anything fails.
fn cc_writes_after_the_credential(fx: &Fx, cc_write: fn(&mut Value)) {
    let (kc, (svc, acct)) = (fx.kc.clone(), fx.live_item(ItemKind::OAuth));
    fx.engine.on_point(
        "after-credential",
        Box::new(move || {
            let mut live: Value = serde_json::from_slice(&kc.get(&svc, &acct).unwrap()).unwrap();
            cc_write(&mut live);
            kc.put(&svc, &acct, live.to_string().as_bytes());
        }),
    );
}

#[test]
fn a_switch_writes_its_target_over_cc_s_dead_token_marking() {
    // §9.1: a marking is no conflict. Between the journal row and the credential write, CC
    // holds its lock, marks b's token dead and refreshes its MCP token; the switch goes ahead
    // over the marking and keeps CC's MCP token.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b
    let cc = cc_holds_storage_write_from(&fx, "after-journal", None, |live| {
        cc_marks_dead(live);
        live["mcpOAuth"] = json!({"srv": {"token": "refreshed-by-cc"}});
    });

    let out = fx.switch_to(&a, false);
    let ended = Instant::now();
    let released = cc_released(&cc);

    assert!(out.unwrap().switched);
    assert!(ended > released, "the switch waited for CC's lock");
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(
        fx.live_credential().unwrap()["mcpOAuth"],
        json!({"srv": {"token": "refreshed-by-cc"}})
    );
    assert!(journal(&fx).is_none());
    assert!(!fx.paths().storage_write_lock.exists());
}

#[test]
fn a_switch_whose_entry_cc_changes_by_more_than_a_marking_rolls_back() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: b
    let config = fs::read(fx.paths().global_config).unwrap();
    // Between the journal row and the credential write, CC holds its lock and changes b's
    // account-scoped keys by more than a dead-token marking (§9.1).
    let cc = cc_holds_storage_write_from(&fx, "after-journal", None, cc_marks_dead_and_more);

    let out = fx.switch_to(&a, false);
    let ended = Instant::now();
    let released = cc_released(&cc);

    let err = out.unwrap_err();
    assert!(ended > released, "the switch waited for CC's lock");
    assert!(matches!(err, EngineError::RolledBack(_)), "{err}");
    assert!(
        err.to_string().contains("changed by another writer"),
        "{err}"
    );
    let live = fx.live_credential().unwrap();
    assert_eq!(
        live["trustedDeviceToken"],
        json!("cc-device"),
        "CC's write stands"
    );
    assert_eq!(live["claudeAiOauth"]["refreshToken"], json!(""));
    assert_eq!(
        live["mcpOAuth"],
        json!({"srv": {"token": "machine-shared"}})
    );
    assert_eq!(fs::read(fx.paths().global_config).unwrap(), config);
    assert!(
        journal(&fx).is_none(),
        "nothing was written, so nothing is left to recover"
    );
    let store = fx.engine.store().unwrap();
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(b));
    assert!(!fx.paths().storage_write_lock.exists());
    assert!(mutation_lock_free(&fx.env));
}

#[test]
fn an_mcp_token_cc_refreshes_during_the_wait_is_kept_by_the_switch() {
    // B #61: the machine-shared keys are taken from the read under the lock.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let cc = cc_holds_storage_write_from(&fx, "after-journal", None, |live| {
        live["mcpOAuth"] = json!({"srv": {"token": "refreshed-by-cc"}});
    });

    let out = fx.switch_to(&a, false);
    let ended = Instant::now();
    let released = cc_released(&cc);

    assert!(out.unwrap().switched);
    assert!(ended > released, "the switch waited for CC's lock");
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(
        fx.live_credential().unwrap()["mcpOAuth"],
        json!({"srv": {"token": "refreshed-by-cc"}}),
        "CC's MCP write is never lost"
    );
    assert!(!fx.paths().storage_write_lock.exists());
}

#[test]
fn a_rollback_leaves_a_target_cc_marked_since_and_keeps_the_row() {
    // A rollback puts a place back only while it holds exactly what the switch wrote there. CC
    // marked the target dead after the switch wrote it, by itself or with another change: the
    // rollback leaves CC's write, reports it, and the row stays (§9.4 step 10). A marked login
    // names no account, so the next command's recovery cannot decide the row and refuses,
    // pointing at `switch --force` (§9.6). CC's write is still there.
    for cc_write in [cc_marks_dead as fn(&mut Value), cc_marks_dead_and_more] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        cc_writes_after_the_credential(&fx, cc_write);
        fx.engine.fail_at(Some("after-credential"));

        let err = fx.switch_to(&a, false).unwrap_err();

        assert!(matches!(err, EngineError::RollbackFailed { .. }), "{err}");
        assert!(
            err.to_string().contains("changed since tagteam wrote it"),
            "{err}"
        );
        let live = fx.live_credential().unwrap();
        assert_eq!(
            live["claudeAiOauth"]["refreshToken"],
            json!(""),
            "CC's marking stands"
        );
        assert!(journal(&fx).is_some(), "the row stays for recovery");
        assert!(!fx.paths().storage_write_lock.exists());

        the_command_exits(&fx);
        let next = fx.switch_to(&b, false).unwrap_err();
        assert!(matches!(next, EngineError::InterruptedSwitch(_)), "{next}");
        assert!(next.to_string().contains("--force"), "{next}");
        assert_eq!(
            fx.live_credential().unwrap(),
            live,
            "CC's write is still there"
        );
    }
}

#[test]
fn ctrl_c_while_the_switch_write_waits_for_cc_still_commits() {
    // §14.1: steps 7–10 are a critical span, so the storage-write wait there is not a
    // cancellation point. The signal waits for the next one.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let cc = cc_holds_storage_write_from(
        &fx,
        "after-journal",
        Some(Duration::from_millis(100)),
        |_| {},
    );

    let out = fx.switch_to(&a, false);
    let ended = Instant::now();
    let released = cc_released(&cc);

    assert!(out.unwrap().switched, "the switch ran to its commit");
    assert!(ended > released, "the switch waited for CC's lock");
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(fx.engine.cancel().requested(), Some(libc::SIGINT));
    assert!(journal(&fx).is_none());
    assert!(!fx.paths().storage_write_lock.exists());
}

#[test]
fn ctrl_c_while_a_recovery_write_waits_for_cc_lets_the_recovery_finish() {
    // §14.1: recovery's writes are a critical span too. The command stops at its next wait.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a); // died after writing the credential, before the identity
    let lock = fx.paths().storage_write_lock;
    fs::create_dir(&lock).unwrap(); // CC holds it as the command starts
    let cancel = fx.engine.cancel().clone();
    let cc = thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        cancel.request(libc::SIGINT);
        thread::sleep(Duration::from_millis(300));
        let at = Instant::now();
        fs::remove_dir(&lock).unwrap();
        at
    });

    let out = fx.switch_to(&b, false);
    let ended = Instant::now();
    let released = cc.join().unwrap();
    let err = out.unwrap_err();

    assert_eq!(err.signal(), Some(libc::SIGINT), "{err}");
    assert!(
        matches!(err, EngineError::Lock(_)),
        "stopped at the switch's own mutation lock, after recovery: {err:?}"
    );
    assert!(journal(&fx).is_none(), "recovery committed");
    let store = fx.engine.store().unwrap();
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(a));
    assert_eq!(
        store.events().unwrap().last().unwrap().kind,
        "switch-recovered"
    );
    assert!(ended > released, "recovery waited for CC's lock");
    assert!(!fx.paths().storage_write_lock.exists());
}

#[test]
fn a_switch_rolls_back_when_another_writer_changed_a_fallback_item() {
    // §9.1 at every place the write may overwrite or delete: the unsuffixed item a reader also
    // tries (`with_fallback_items`) changes while the switch waits, and the switch rolls back,
    // leaving it as the other writer wrote it.
    let fx = Fx::with_fallback_items();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: b
    fx.put_fallback_item(fx.live_credential().unwrap().to_string().as_bytes());
    let cc = writer_holds_storage_write_from(&fx, "after-journal", None, FALLBACK_ITEM, |item| {
        item["claudeAiOauth"]["refreshToken"] = json!("rt-other");
    });

    let out = fx.switch_to(&a, false);
    let ended = Instant::now();
    let released = cc_released(&cc);

    let err = out.unwrap_err();
    assert!(ended > released, "the switch waited for the lock");
    assert!(matches!(err, EngineError::RolledBack(_)), "{err}");
    assert!(err.to_string().contains(FALLBACK_ITEM), "{err}");
    assert_eq!(
        fx.fallback_item().unwrap()["claudeAiOauth"]["refreshToken"],
        json!("rt-other"),
        "the other writer's item stands"
    );
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
    assert!(journal(&fx).is_none());
    let store = fx.engine.store().unwrap();
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(b));
    assert!(!fx.paths().storage_write_lock.exists());
}

#[test]
fn a_rollback_puts_each_place_back_byte_for_byte() {
    // Nothing wrote between the switch and its rollback, so every place goes back to exactly
    // what it held. The Keychain item and `.credentials.json` hold different MCP tokens; an
    // API-key switch clears the OAuth entry at both, keeping each one's, then fails.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a"); // live: a, in the Keychain
    let key = fx.add_api_key(API_KEY);
    let mut file = fx.live_credential().unwrap();
    file["mcpOAuth"] = json!({"srv": {"token": "the-file's-own"}});
    fs::write(fx.paths().credentials_file, file.to_string()).unwrap();
    let item_before = fx.live_credential();
    let file_before = fs::read(fx.paths().credentials_file).unwrap();
    fx.engine.fail_at(Some("after-credential"));

    let err = fx.switch_to(&key, false).unwrap_err();

    assert!(matches!(err, EngineError::RolledBack(_)), "{err}");
    assert_eq!(fx.live_credential(), item_before);
    assert_eq!(
        fs::read(fx.paths().credentials_file).unwrap(),
        file_before,
        "the file keeps its own MCP token"
    );
    assert!(journal(&fx).is_none());
}

#[test]
fn a_rollback_keeps_the_mcp_token_cc_refreshed_while_the_switch_waited() {
    // §9.1: CC refreshes its MCP token while the switch waits for the storage-write lock; the
    // switch writes its target carrying the new token, then fails. The rollback puts b's login
    // back with the token CC refreshed, never the one the snapshot read before CC's write.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b
    let mut want = fx.live_credential().unwrap();
    want["mcpOAuth"] = json!({"srv": {"token": "refreshed-by-cc"}});
    let cc = cc_holds_storage_write_from(&fx, "after-journal", None, |live| {
        live["mcpOAuth"] = json!({"srv": {"token": "refreshed-by-cc"}});
    });
    fx.engine.fail_at(Some("after-credential"));

    let out = fx.switch_to(&a, false);
    let ended = Instant::now();
    let released = cc_released(&cc);

    let err = out.unwrap_err();
    assert!(ended > released, "the switch waited for CC's lock");
    assert!(matches!(err, EngineError::RolledBack(_)), "{err}");
    assert_eq!(
        fx.live_credential().unwrap(),
        want,
        "b's login is back, with CC's refreshed MCP token"
    );
    assert!(journal(&fx).is_none());
    assert!(!fx.paths().storage_write_lock.exists());
}

#[test]
fn a_rolled_back_fallback_from_a_file_login_creates_no_keychain_item() {
    // Codex round 5: the live login is in `.credentials.json` with no Keychain item. The
    // target's Keychain write fails, so the write falls back to the file; the Keychain
    // recovers and the switch fails. The rollback must put the file login back, MCP keys and
    // all, and leave no Keychain item, which CC would read first.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b, in the Keychain
    let (svc, acct) = fx.live_item(ItemKind::OAuth);
    let login = fx.kc.get(&svc, &acct).unwrap();
    fs::write(fx.paths().credentials_file, &login).unwrap();
    tagteam_provider::Keychain::delete(&*fx.kc, &svc, &acct).unwrap(); // b's login is in the file only
    fx.kc.set_fail_write(&svc, true);
    let (kc, healed) = (fx.kc.clone(), svc.clone());
    fx.engine.on_point(
        "after-credential",
        Box::new(move || kc.set_fail_write(&healed, false)),
    );
    fx.engine.fail_at(Some("after-credential"));

    let err = fx.switch_to(&a, false).unwrap_err();

    assert!(matches!(err, EngineError::RolledBack(_)), "{err}");
    assert_eq!(
        fx.kc.get(&svc, &acct),
        None,
        "no Keychain item hides the file login"
    );
    assert_eq!(fs::read(fx.paths().credentials_file).unwrap(), login);
    assert!(journal(&fx).is_none());
}

#[test]
fn a_rollback_leaves_the_item_cc_refreshed_and_recovery_finishes_forward() {
    // Codex rounds 6 and 7: the live login is in `.credentials.json` with no Keychain item. The
    // switch creates the item and mirrors the file; CC then refreshes its MCP token in the
    // item, the only place that holds it, and the switch fails. The rollback leaves the entry
    // as it is and keeps the row. The next command finds a's login live and finishes the
    // switch forward, CC's token kept (§9.6).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b, in the Keychain
    let (svc, acct) = fx.live_item(ItemKind::OAuth);
    let login = fx.kc.get(&svc, &acct).unwrap();
    fs::write(fx.paths().credentials_file, &login).unwrap();
    fx.kc.delete(&svc, &acct).unwrap(); // b's login is in the file only
    cc_writes_after_the_credential(&fx, |live| {
        live["mcpOAuth"] = json!({"srv": {"token": "refreshed-by-cc"}});
    });
    fx.engine.fail_at(Some("after-credential"));

    let err = fx.switch_to(&a, false).unwrap_err();

    assert!(matches!(err, EngineError::RollbackFailed { .. }), "{err}");
    assert!(
        err.to_string()
            .contains(&format!("{svc} (changed since tagteam wrote it")),
        "{err}"
    );
    let item = fx.kc.get(&svc, &acct).unwrap();
    let cc: Value = serde_json::from_slice(&item).unwrap();
    assert_eq!(cc["claudeAiOauth"]["refreshToken"], json!("rt-a"));
    assert_eq!(
        cc["mcpOAuth"],
        json!({"srv": {"token": "refreshed-by-cc"}}),
        "CC's write stands"
    );
    assert!(journal(&fx).is_some(), "the row stays for recovery");

    the_command_exits(&fx);
    drop(fx.engine.mutation_guard().unwrap());

    assert!(journal(&fx).is_none(), "recovery finished the switch");
    let store = fx.engine.store().unwrap();
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(a));
    assert_eq!(
        fx.kc.get(&svc, &acct).unwrap(),
        item,
        "with CC's token kept"
    );
}
```

Append to the end of `crates/tagteam/tests/signals.rs` (Task 6). It is Review Focus 2 again,
with the two SIGINTs landing while the switch's step 7 write waits for Claude Code's
storage-write lock, and it uses Task 6's helpers in that file (`send`, `wait_until`, `finish`,
`store`) and its `TAGTEAM_TEST_PAUSE_AT` point:

```rust
#[test]
fn ctrl_c_twice_while_the_switch_s_write_waits_for_claude_code_s_storage_write_lock_lets_it_commit()
{
    // Review Focus 2 with Claude Code holding its storage-write lock (§9.1, Task 11): the
    // switch's step 7 write waits for it inside the critical span, and two SIGINTs land during
    // that wait. The switch writes and commits once CC lets go, and says the signal came too
    // late (§14.1).
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (a, _b) = two_fresh_accounts(root);
    let paths = CcPaths::resolve(&Env::for_test(root));
    let pause = root.join("pause");
    fs::create_dir(&pause).unwrap();

    let mut child = std_cmd(root)
        .args(["switch", "1", "--json"])
        .env("TAGTEAM_TEST_PAUSE_AT", "after-journal")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_until(
        &mut child,
        Duration::from_secs(20),
        "its journal row",
        || pause.join("paused").exists(),
    );
    fs::create_dir(&paths.storage_write_lock).unwrap(); // Claude Code takes it
    fs::write(pause.join("resume"), b"").unwrap();
    thread::sleep(Duration::from_millis(300)); // the step 7 write now waits for CC's lock
    send(&child, libc::SIGINT);
    thread::sleep(Duration::from_millis(100));
    send(&child, libc::SIGINT);
    thread::sleep(Duration::from_millis(300));
    assert!(
        child.try_wait().unwrap().is_none(),
        "the switch finished, or a Ctrl-C stopped it, before Claude Code let go of its lock"
    );
    fs::remove_dir(&paths.storage_write_lock).unwrap(); // Claude Code lets go
    let out = finish(child, Duration::from_secs(20));

    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (v["switched"].clone(), v["reason"].clone()),
        (json!(true), json!("switched")),
        "{v}"
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "tagteam: interrupted too late to stop: switch had already finished\n"
    );
    assert_eq!(live_email(root), "a@x.co");
    let provider = ProviderId::new(CLAUDE_CODE);
    let s = store(root);
    assert!(s.journal(&provider).unwrap().is_none());
    assert_eq!(
        s.active(&provider).unwrap(),
        Some(AccountId::from_string(&a))
    );
    assert!(
        !paths.storage_write_lock.exists(),
        "tagteam released the storage-write lock it took"
    );
}
```

In `crates/tagteam-engine/tests/active.rs`, the guard that only the credential locks are held across
§7.5's request now covers the storage-write lock too. In `mod hooks`, replace

```rust
mod hooks {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime};

    use super::*;
```

with

```rust
mod hooks {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant, SystemTime};

    use super::common::{
        cc_holds_storage_write_from, cc_marks_dead, cc_marks_dead_and_more, cc_released,
    };
    use super::*;
```

In `only_the_credential_locks_are_held_across_the_request`, replace

```rust
        let (refresh, config, record) = (
            fx.paths().refresh_lock,
            fx.paths().config_lock,
            seen.clone(),
        );
        fx.engine.on_point(
            "active-before-request",
            Box::new(move || {
                *record.lock().unwrap() = Some((refresh.is_dir(), config.exists()));
            }),
        );
```

with

```rust
        let (refresh, config, storage_write, record) = (
            fx.paths().refresh_lock,
            fx.paths().config_lock,
            fx.paths().storage_write_lock,
            seen.clone(),
        );
        fx.engine.on_point(
            "active-before-request",
            Box::new(move || {
                *record.lock().unwrap() =
                    Some((refresh.is_dir(), config.exists(), storage_write.exists()));
            }),
        );
```

and

```rust
            Some((true, false)),
            "CC's refresh lock is held; its config lock is not (§4.3)"
```

with

```rust
            Some((true, false, false)),
            "CC's refresh lock is held; its config lock and storage-write lock are not (§4.3, \
             §9.1: no network call while holding it)"
```

Then add, after Task 2's `a_signal_while_the_successor_waits_for_the_config_lock_still_publishes_it`
and before the doc comment of `take_over_after_response`:

```rust
    /// §14.1, §7.5 step 5, §9.1: the successor's live write also waits for CC's storage-write
    /// lock inside the critical span, so a Ctrl-C during that wait never leaves CC on the
    /// generation the request consumed.
    #[test]
    fn a_signal_while_the_successor_waits_for_the_storage_write_lock_still_publishes_it() {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        let ctrl_c = Some(Duration::from_millis(100));
        let cc = cc_holds_storage_write_from(&fx, "active-before-publish", ctrl_c, |_| {});

        let out = active(&fx, ActiveTrigger::Expired);
        let ended = Instant::now();
        let released = cc_released(&cc);

        assert_eq!(out.unwrap(), ActiveOutcome::Refreshed);
        assert!(ended > released, "the live write waited for CC's lock");
        assert_eq!(
            fx.live_refresh_token().as_deref(),
            Some("rt-a2"),
            "CC holds the successor"
        );
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
        assert_eq!(fx.engine.cancel().requested(), Some(libc::SIGINT));
        assert!(!fx.paths().storage_write_lock.exists());
    }

    /// §9.1: CC marks the consumed generation dead while the successor waits for the
    /// storage-write lock. A marking is no conflict: the successor is published over it, and CC
    /// ends on the newest generation (§7.5 step 5).
    #[test]
    fn a_live_token_cc_marks_dead_while_the_successor_waits_gets_the_successor() {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        let cc = cc_holds_storage_write_from(&fx, "active-before-publish", None, cc_marks_dead);

        let out = active(&fx, ActiveTrigger::Expired);
        let ended = Instant::now();
        let released = cc_released(&cc);

        assert_eq!(out.unwrap(), ActiveOutcome::Refreshed);
        assert!(ended > released, "the live write waited for CC's lock");
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
        assert!(!fx.paths().storage_write_lock.exists());
    }

    /// §9.1: CC changes the live entry's account-scoped keys by more than a marking while the
    /// successor waits. The live write aborts rather than overwrite it; the successor stays in
    /// the vault, not published (§7.5 step 5).
    #[test]
    fn a_live_entry_cc_changes_while_the_successor_waits_is_left_as_cc_wrote_it() {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        let cc =
            cc_holds_storage_write_from(&fx, "active-before-publish", None, cc_marks_dead_and_more);

        let out = active(&fx, ActiveTrigger::Expired);
        let ended = Instant::now();
        let released = cc_released(&cc);

        assert_eq!(out.unwrap(), ActiveOutcome::PersistedNotPublished);
        assert!(ended > released, "the live write waited for CC's lock");
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
        assert_eq!(
            fx.live_credential().unwrap()["trustedDeviceToken"],
            json!("cc-device"),
            "CC's write stands"
        );
        assert!(!fx.paths().storage_write_lock.exists());
    }
```

- [ ] **Step 7: Run them to verify they fail**

Run: `cargo test -p tagteam-cc`
Expected: FAIL to compile: ``no variant or associated item named `EntryMoved` found for enum
`ProviderError` `` (five times, in `live_store.rs` and `provider.rs`) and ``no method named
`with_storage_write_timeout` found for struct `LiveStore` ``.

Run: `cargo test -p tagteam-engine --features test-hooks --test storage_write`
Expected: it compiles (it needs only Step 3's `storage_write_lock`) and FAILS, nine of its eleven
tests, because no write takes the lock yet:
- `a_switch_writes_its_target_over_cc_s_dead_token_marking`,
  `an_mcp_token_cc_refreshes_during_the_wait_is_kept_by_the_switch`,
  `a_rollback_keeps_the_mcp_token_cc_refreshed_while_the_switch_waited` and
  `ctrl_c_while_the_switch_write_waits_for_cc_still_commits`: `the switch waited for CC's lock`.
- `a_switch_whose_entry_cc_changes_by_more_than_a_marking_rolls_back` and
  `a_switch_rolls_back_when_another_writer_changed_a_fallback_item`: ``called
  `Result::unwrap_err()` on an `Ok` value: SwitchOutcome { switched: true, ...``. The switch wrote
  without waiting or checking.
- `a_rollback_leaves_a_target_cc_marked_since_and_keeps_the_row` and
  `a_rollback_leaves_the_item_cc_refreshed_and_recovery_finishes_forward`: the `matches!` fails
  with `the switch failed and was rolled back: injected failure at after-credential`. The
  rollback wrote b's credential back over CC's write.
- `ctrl_c_while_a_recovery_write_waits_for_cc_lets_the_recovery_finish`: ``called
  `Result::unwrap_err()` on an `Ok` value``. Recovery and then the switch to b ran before the
  signal.

Two pass already, because today's restore writes the snapshot back byte for byte and nothing
else writes in them:
- `a_rollback_puts_each_place_back_byte_for_byte` guards that each place gets back exactly what
  it held, its own MCP token included;
- `a_rolled_back_fallback_from_a_file_login_creates_no_keychain_item` guards that a restore
  never creates a Keychain item in front of a restored file login.

Run: `cargo test -p tagteam --features test-support --test signals`
Expected: FAIL, the new test only:
`ctrl_c_twice_while_the_switch_s_write_waits_for_claude_code_s_storage_write_lock_lets_it_commit`:
`the switch finished, or a Ctrl-C stopped it, before Claude Code let go of its lock`. Nothing
waits for the lock yet, so the switch finished before the signals. Task 6's two tests pass.

Run: `cargo test -p tagteam-engine --features test-hooks --test active hooks::`
Expected: FAIL, three tests:
- `a_signal_while_the_successor_waits_for_the_storage_write_lock_still_publishes_it` and
  `a_live_token_cc_marks_dead_while_the_successor_waits_gets_the_successor`: `the live write
  waited for CC's lock`.
- `a_live_entry_cc_changes_while_the_successor_waits_is_left_as_cc_wrote_it`: `left:
  Refreshed`, `right: PersistedNotPublished`.

`only_the_credential_locks_are_held_across_the_request` passes, before and after: it guards that
nothing takes the storage-write lock with the credential locks, which would hold it across the
request.

- [ ] **Step 8: Add the error and the timeout setting**

In `crates/tagteam-provider/src/provider.rs`, replace

```rust
    /// `restore` could not put back every entry a switch may have touched, having still
    /// attempted every one of them (a fence or `Lock` failure aborts immediately instead,
    /// and is reported as that error, not this one). Each name is a Keychain service or a
    /// file path, never bytes.
    #[error("the previous state could not be restored for: {}", .failed.join(", "))]
    Incomplete { failed: Vec<String> },
```

with:

```rust
    /// `restore` could not put back every entry a switch may have touched, having still
    /// attempted every one of them (a fence or `Lock` failure aborts immediately instead,
    /// and is reported as that error, not this one). Each name is a Keychain service or a
    /// file path, never bytes, followed in parentheses by the reason when the entry was left
    /// alone rather than failing to write.
    #[error("the previous state could not be restored for: {}", .failed.join(", "))]
    Incomplete { failed: Vec<String> },
    /// Under the provider's storage-write lock, a credential entry's account-scoped keys were
    /// no longer what tagteam last read or wrote under the credential locks, and not merely by
    /// the agent's own dead-token marking, which a write goes ahead over (§9.1). The write was
    /// aborted before it touched the entry. The name is a Keychain service or a file path,
    /// never bytes.
    #[error("{0} was changed by another writer during tagteam's write; it was left as it is")]
    EntryMoved(String),
```

`EngineError::kind` maps it to `provider` through its existing `EngineError::Provider(_)` arm; no
other code matches `ProviderError` exhaustively.

In `crates/tagteam-cc/src/live.rs`, replace

```rust
use crate::config::{self, read_bytes};
use crate::naming::{ItemKind, keychain_account, keychain_service, read_services};
```

with

```rust
use crate::config::{self, read_bytes};
use crate::locks;
use crate::naming::{ItemKind, keychain_account, keychain_service, read_services};
```

then, in `struct LiveStore` (as Task 7 left it), replace

```rust
    file_mode_pinned: AtomicBool,
    retry_delay: Duration,
}
```

with

```rust
    file_mode_pinned: AtomicBool,
    /// How long one wait for CC's storage-write lock may take (§9.1: 9 s).
    storage_write_timeout: Duration,
    retry_delay: Duration,
}
```

in `LiveStore::new`, replace

```rust
            file_mode_pinned: AtomicBool::new(false),
            retry_delay: Duration::from_millis(300),
```

with

```rust
            file_mode_pinned: AtomicBool::new(false),
            storage_write_timeout: locks::ACQUIRE_TIMEOUT,
            retry_delay: Duration::from_millis(300),
```

and add after `with_retry_delay`:

```rust
    /// A shorter wait for CC's storage-write lock, so a test of a held lock need not wait 9 s.
    pub fn with_storage_write_timeout(mut self, d: Duration) -> Self {
        self.storage_write_timeout = d;
        self
    }
```

- [ ] **Step 9: Run them to verify they fail for the right reason**

Run: `cargo test -p tagteam-cc --test live_store`
Expected: FAIL, fifteen of the sixteen new tests:
- `every_credential_entry_write_holds_the_storage_write_lock_and_releases_it`: `a fallback
  reports both items it deletes under the lock`, `left: [false, false]`.
- `a_held_storage_write_lock_makes_every_entry_write_wait_then_time_out`: `an OAuth write:
  expected a timeout on the storage-write lock, got Ok(Keychain)`.
- `a_write_waits_for_cc_and_keeps_the_machine_shared_keys_cc_wrote_meanwhile`,
  `a_write_goes_ahead_over_cc_s_dead_token_marking_and_keeps_cc_s_mcp_token` and
  `a_write_goes_ahead_over_a_marking_of_the_credentials_file`: `the write waited for CC's lock`.
- `a_clear_goes_ahead_over_cc_s_marking_of_a_fallback_item`: `the clear waited for CC's lock`.
- `a_write_after_cc_changed_the_account_scoped_keys_aborts_and_writes_nothing` and
  `a_write_aborts_when_another_writer_changed_the_credentials_file`: `expected the write to
  abort, got Ok(Keychain)`.
- `a_clear_aborts_when_another_writer_changed_a_fallback_item`: `expected the clear to abort,
  got Ok(())`.
- `a_restore_leaves_an_entry_changed_since_tagteam_wrote_it_and_names_it`,
  `a_restore_leaves_the_whole_entry_once_cc_wrote_to_an_item_tagteam_created` and
  `a_restore_leaves_a_place_cc_wrote_between_two_of_tagteam_s_writes`: `expected the item named
  as left, got Ok(())`.
- `a_restore_puts_nothing_back_in_front_of_a_place_cc_changed`: `expected the file named as
  left, got Ok(())`.
- `a_restore_leaves_alone_an_entry_this_operation_never_wrote`: the OAuth item holds `rt-0`
  again, not CC's marking.
- `a_signal_ends_a_writes_wait_for_the_storage_write_lock_but_never_a_restores`: `expected an
  interrupted wait, got Ok(Keychain)`.

`a_restore_puts_each_place_back_byte_for_byte` passes already: today's restore writes each place
back byte for byte. It guards that the new restore still does when nothing else wrote.
The four reworked tests pass already: they pin restore behaviour this task keeps.

Run: `cargo test -p tagteam-cc --test provider`
Expected: FAIL, two tests:
- `the_storage_write_lock_is_a_leaf_taken_only_around_each_entry_write`: `a credential entry
  written without the storage-write lock or the live locks: [("Claude Code", [false, true, true,
  true]), ...`.
- `a_write_that_finds_cc_changed_the_entry_aborts_and_leaves_everything_as_cc_left_it`: ``called
  `Option::unwrap()` on a `None` value``: the write succeeded.

- [ ] **Step 10: Take the lock around every entry write**

In `crates/tagteam-cc/src/live.rs`, replace the imports

```rust
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
```

with

```rust
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
```

replace

```rust
use tagteam_provider::{
    BeforeFallback, Credential, DoomedEntry, Env, Keychain, ProviderError, Read, SecretStore,
};
```

with

```rust
use tagteam_provider::{
    BeforeFallback, Cancel, Credential, DoomedEntry, Env, Keychain, MkdirLock, ProviderError, Read,
    SecretStore,
};
```

and replace

```rust
use crate::shape::machine_shared_only;
```

with

```rust
use crate::shape::{MACHINE_SHARED_KEYS, machine_shared_only};
```

Add the account-scoped part and the operation's record after
`type ItemSnapshot = (String, Option<Vec<u8>>);`:

```rust
/// The account-scoped part of a credential entry (§9.1, Appendix A.4): every key but the
/// machine-shared ones. Bytes that are not a JSON object, such as an API key, count whole. An
/// absent entry and one holding only machine-shared keys have none. It holds secrets, so it has
/// no `Debug`.
#[derive(Clone, PartialEq)]
enum AccountPart {
    Keys(Map<String, Value>),
    Raw(Vec<u8>),
}

impl AccountPart {
    fn of(bytes: Option<&[u8]>) -> Self {
        let Some(bytes) = bytes else {
            return AccountPart::Keys(Map::new());
        };
        match object(bytes) {
            Some(mut o) => {
                for k in MACHINE_SHARED_KEYS {
                    o.shift_remove(k);
                }
                AccountPart::Keys(o)
            }
            None => AccountPart::Raw(bytes.to_vec()),
        }
    }

    /// This part as Claude Code's dead-token marking leaves it (§9.1, Appendix A.3): in
    /// `claudeAiOauth`, both tokens empty and `expiresAt` 0, every other key as it was. `None`
    /// when there is no `claudeAiOauth` object to mark.
    fn marked(&self) -> Option<AccountPart> {
        let AccountPart::Keys(keys) = self else {
            return None;
        };
        let mut keys = keys.clone();
        let Some(Value::Object(oauth)) = keys.get_mut("claudeAiOauth") else {
            return None;
        };
        oauth.insert("accessToken".into(), json!(""));
        oauth.insert("refreshToken".into(), json!(""));
        oauth.insert("expiresAt".into(), json!(0));
        Some(AccountPart::Keys(keys))
    }
}

/// What one place of a credential entry held each time the current operation looked: what its
/// latest `snapshot` read there (`None`: absent), then every value written there since.
type Values = Vec<Option<Vec<u8>>>;

/// Every place of an entry, named as `Seen` names it, with what it holds now, in reader order.
type Places = Vec<(String, Option<Vec<u8>>)>;

/// What a place of an entry actually holds, as far as the operation knows: what it read there
/// under the storage-write lock, or what a write it saw succeed put there (`None`: absent).
/// `Unknown` after a write that failed where a read under the same hold could not tell. It holds
/// secrets, so it has no `Debug`.
#[derive(Clone, PartialEq)]
enum Held {
    Known(Option<Vec<u8>>),
    Unknown,
}

impl Held {
    fn of(read: Read<Vec<u8>>) -> Self {
        match read {
            Read::Present(b) => Held::Known(Some(b)),
            Read::Absent => Held::Known(None),
            Read::Unreadable(_) => Held::Unknown,
        }
    }
}

/// What the current operation has seen of one credential entry under the credential locks
/// (§9.1), place by place.
#[derive(Default)]
struct Seen {
    /// By place: a Keychain service, or the credentials file's path. `None` until the
    /// operation reads the entry.
    places: Option<BTreeMap<String, Values>>,
    /// What each place actually holds: read under the storage-write lock at the start of each
    /// hold, then updated after each write. Never an attempted value that did not land.
    held: BTreeMap<String, Held>,
    /// What each place the operation's writes changed actually held just before the first of
    /// those changes: what a restore puts back there.
    first: BTreeMap<String, Held>,
    /// The places a restore must leave as they are: changed by another writer since the
    /// operation first changed the entry, or holding something unknown before that change.
    leave: BTreeSet<String>,
}

impl Seen {
    fn values(&self, place: &str) -> Option<&Values> {
        self.places.as_ref()?.get(place)
    }

    /// §9.1: whether `now`, what `place` holds under the storage-write lock, has account-scoped
    /// keys this operation last read or wrote there, or CC's dead-token marking of them. A place
    /// the operation never read has nothing to compare.
    fn known(&self, place: &str, now: Option<&[u8]>) -> bool {
        let Some(values) = self.values(place) else {
            return true;
        };
        let now = AccountPart::of(now);
        values
            .iter()
            .map(|v| AccountPart::of(v.as_deref()))
            .any(|p| p == now || p.marked().as_ref() == Some(&now))
    }
}

/// `Seen` for both credential entries (§9.1): the OAuth entry and the managed-key item.
#[derive(Default)]
struct Ledger {
    oauth: Seen,
    managed: Seen,
}

impl Ledger {
    fn entry(&mut self, kind: ItemKind) -> &mut Seen {
        match kind {
            ItemKind::OAuth => &mut self.oauth,
            ItemKind::ManagedKey => &mut self.managed,
        }
    }
}

/// Why a restore left a place of an entry as it is: another writer changed the entry since this
/// operation wrote it, so putting back what it held before could lose that write.
const CHANGED_SINCE: &str = "changed since tagteam wrote it; left as it is";
```

In `struct LiveStore`, replace

```rust
    file_mode_pinned: AtomicBool,
    /// How long one wait for CC's storage-write lock may take (§9.1: 9 s).
```

with

```rust
    file_mode_pinned: AtomicBool,
    /// §9.1: what the current operation read and wrote of each credential entry, so a write
    /// under the storage-write lock can tell Claude Code's changes from its own. Cleared, with
    /// the pin, when the operation's credential locks are released.
    seen: Mutex<Ledger>,
    /// How long one wait for CC's storage-write lock may take (§9.1: 9 s).
```

Add the helpers after `remove_if_present`:

```rust
/// The JSON object `bytes` hold, if they hold one.
fn object(bytes: &[u8]) -> Option<Map<String, Value>> {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(o)) => Some(o),
        _ => None,
    }
}

/// The credentials file as a place of the OAuth entry, named as `restore` and `EntryMoved` name
/// it: its path.
fn file_place(paths: &CcPaths) -> String {
    paths.credentials_file.display().to_string()
}

/// The machine-shared keys an entry holds (Appendix A.4): none for an absent entry or bytes
/// that are not a JSON object.
fn shared_of(bytes: Option<&[u8]>) -> Map<String, Value> {
    bytes
        .and_then(object)
        .map(|o| machine_shared_only(&o))
        .unwrap_or_default()
}

/// `bytes` carrying `shared`, the machine-shared keys the entry holds under the storage-write
/// lock, their absence included (§9.1), so a Claude Code write made since `bytes` were composed
/// is kept. Bytes that already carry exactly those keys, or that are not a JSON object, are
/// returned as they are.
fn rebase(bytes: &[u8], shared: &Map<String, Value>) -> Vec<u8> {
    let Some(mut o) = object(bytes) else {
        return bytes.to_vec();
    };
    if machine_shared_only(&o) == *shared {
        return bytes.to_vec();
    }
    for k in MACHINE_SHARED_KEYS {
        o.shift_remove(k);
    }
    o.extend(shared.clone());
    serde_json::to_vec(&Value::Object(o)).expect("a Value always serializes")
}

/// `fence`, then the storage-write lock's own ownership check: the check that runs immediately
/// before every write the lock protects (§9.1).
fn held<'a>(fence: Fence<'a>, lock: &'a MkdirLock) -> impl Fn() -> Result<(), ProviderError> + 'a {
    move || {
        fence()?;
        Ok(lock.check_owned()?)
    }
}
```

In `LiveStore::new`, replace

```rust
            file_mode_pinned: AtomicBool::new(false),
            storage_write_timeout: locks::ACQUIRE_TIMEOUT,
```

with

```rust
            file_mode_pinned: AtomicBool::new(false),
            seen: Mutex::new(Ledger::default()),
            storage_write_timeout: locks::ACQUIRE_TIMEOUT,
```

Add after `unpin_file_mode` (Task 7):

```rust
    /// Ends the operation (Appendix A.3, §9.1): the file-mode pin, and what it read and wrote of
    /// each credential entry.
    pub(crate) fn end_operation(&self) {
        self.unpin_file_mode();
        *self.seen.lock().unwrap() = Ledger::default();
    }

    /// CC's storage-write lock, for one entry's write (§9.1), waited for under `cancel`.
    fn storage_write(&self, paths: &CcPaths, cancel: &Cancel) -> Result<MkdirLock, ProviderError> {
        Ok(locks::acquire_storage_write(
            paths,
            self.storage_write_timeout,
            cancel,
        )?)
    }

    /// Every place of `kind`'s entry, read again under the storage-write lock, strictly (Appendix
    /// A.3): the Keychain items a reader tries, in reader order (macOS), then, for the OAuth
    /// entry, the credentials file. An item that exists but cannot be read refuses.
    fn places_now(
        &self,
        env: &Env,
        paths: &CcPaths,
        kind: ItemKind,
    ) -> Result<Places, ProviderError> {
        let mut out = Vec::new();
        if self.mac() {
            let acct = keychain_account(env);
            for svc in read_services(env, kind) {
                let value = present_or_err(self.retrying(|| self.keychain.find(&svc, &acct)))?;
                out.push((svc, value));
            }
        }
        if kind == ItemKind::OAuth {
            let file = present_or_err(read_bytes(&paths.credentials_file))?;
            out.push((file_place(paths), file));
        }
        Ok(out)
    }

    /// §9.1, at every place of the entry: `EntryMoved`, naming the first place whose
    /// account-scoped keys this operation neither last read nor wrote there; only another writer
    /// can have changed them. CC's dead-token marking of what it read or wrote is no conflict:
    /// the write goes ahead over it.
    fn unmoved(
        &self,
        kind: ItemKind,
        places: &[(String, Option<Vec<u8>>)],
    ) -> Result<(), ProviderError> {
        let mut seen = self.seen.lock().unwrap();
        let seen = seen.entry(kind);
        match places
            .iter()
            .find(|(place, now)| !seen.known(place, now.as_deref()))
        {
            Some((place, _)) => Err(ProviderError::EntryMoved(place.clone())),
            None => Ok(()),
        }
    }

    /// Records, before a write, that this operation writes `bytes` (`None`: deletes) at `place`
    /// of `kind`'s entry (§9.1): a later write or restore of the operation may find it there.
    fn intend(&self, kind: ItemKind, place: &str, bytes: Option<&[u8]>) {
        let mut seen = self.seen.lock().unwrap();
        let entry = seen.entry(kind);
        if let Some(places) = &mut entry.places {
            places
                .entry(place.to_owned())
                .or_default()
                .push(bytes.map(<[u8]>::to_vec));
        }
    }

    /// Records, after a write of `bytes` to the Keychain item `svc`, what the item actually
    /// holds: `bytes` when the write `landed`; otherwise what a read under the same hold finds,
    /// unknown if it cannot be read.
    fn settle_item(
        &self,
        env: &Env,
        kind: ItemKind,
        svc: &str,
        bytes: Option<&[u8]>,
        landed: bool,
    ) {
        let after = if landed {
            Held::Known(bytes.map(<[u8]>::to_vec))
        } else {
            Held::of(self.keychain.find(svc, &keychain_account(env)))
        };
        self.settled(kind, svc, after);
    }

    /// `settle_item` for the credentials file.
    fn settle_file(&self, paths: &CcPaths, bytes: Option<&[u8]>, landed: bool) {
        let after = if landed {
            Held::Known(bytes.map(<[u8]>::to_vec))
        } else {
            Held::of(read_bytes(&paths.credentials_file))
        };
        self.settled(ItemKind::OAuth, &file_place(paths), after);
    }

    /// `place` now holds `after` (§9.1). If that is the operation's first change there, what the
    /// place held just before is kept for a restore; when that is unknown, a restore must leave
    /// the place.
    fn settled(&self, kind: ItemKind, place: &str, after: Held) {
        let mut seen = self.seen.lock().unwrap();
        let entry = seen.entry(kind);
        let before = entry
            .held
            .insert(place.to_owned(), after.clone())
            .unwrap_or(Held::Unknown);
        if before != after && !entry.first.contains_key(place) {
            if before == Held::Unknown {
                entry.leave.insert(place.to_owned());
            }
            entry.first.insert(place.to_owned(), before);
        }
    }

    /// What every place holds at the start of a hold of the storage-write lock (§9.1). Once the
    /// operation has changed the entry, a place holding anything but what the operation last
    /// knew there was changed by another writer since, so a restore must leave the entry; a
    /// write whose outcome was unknown and that never landed left its place as it was. Returns
    /// every place a restore must leave.
    fn observe(&self, kind: ItemKind, places: &[(String, Option<Vec<u8>>)]) -> Vec<String> {
        let mut seen = self.seen.lock().unwrap();
        let seen = seen.entry(kind);
        for (place, value) in places {
            let now = Held::Known(value.clone());
            if !seen.first.is_empty() {
                let was = seen.held.get(place).unwrap_or(&Held::Unknown);
                let unchanged =
                    *was == now || (*was == Held::Unknown && seen.first.get(place) == Some(&now));
                if !unchanged {
                    seen.leave.insert(place.clone());
                }
            }
            seen.held.insert(place.clone(), now);
        }
        seen.leave.iter().cloned().collect()
    }

    /// One write of `kind`'s entry under CC's storage-write lock (§9.1). Waits for the lock
    /// under `env.cancel` (§14.1), reads every place of the entry again and refuses with
    /// `EntryMoved` if the account-scoped keys moved at any of them, then runs `write` with the
    /// entry as Claude Code reads it now (its first place that holds anything) and a fence that
    /// also checks the lock is still this holder's. The lock is released on return, so it is
    /// never held across anything but this one entry's write.
    fn under_storage_write<T>(
        &self,
        env: &Env,
        paths: &CcPaths,
        kind: ItemKind,
        fence: Fence<'_>,
        write: impl FnOnce(Option<&[u8]>, Fence<'_>) -> Result<T, ProviderError>,
    ) -> Result<T, ProviderError> {
        let lock = self.storage_write(paths, &env.cancel)?;
        let places = self.places_now(env, paths, kind)?;
        self.observe(kind, &places);
        self.unmoved(kind, &places)?;
        let now = places.iter().find_map(|(_, value)| value.as_deref());
        let fence = held(fence, &lock);
        write(now, &fence)
    }
```

`under_storage_write` is the one place a write takes the lock. `storage_write`'s token is the
caller's choice: `under_storage_write` passes `env.cancel`, `restore` passes a fresh one (Step 10's
`restore`, below).

Every write records what it sets out to put at each place (`intend`) before it writes there, and
what the place actually holds after (`settle_item`, `settle_file`). In `remove_items`, replace

```rust
        for svc in read_services(env, kind) {
            fence()?;
            let _ = self.keychain.delete(&svc, &acct);
            if !matches!(self.keychain.exists(&svc, &acct), Read::Absent) {
                return Err(ProviderError::ShadowingItem(svc));
            }
        }
```

with

```rust
        for svc in read_services(env, kind) {
            self.intend(kind, &svc, None);
            fence()?;
            let _ = self.keychain.delete(&svc, &acct);
            let gone = matches!(self.keychain.exists(&svc, &acct), Read::Absent);
            self.settle_item(env, kind, &svc, None, gone);
            if !gone {
                return Err(ProviderError::ShadowingItem(svc));
            }
        }
```

and in `write_file`, replace

```rust
        fence()?;
        ensure_private_dir(&paths.secure_storage_dir)?;
        write_atomic_private_with(&paths.credentials_file, bytes, 0o600, fence)
    }
```

with

```rust
        self.intend(ItemKind::OAuth, &file_place(paths), Some(bytes));
        fence()?;
        ensure_private_dir(&paths.secure_storage_dir)?;
        let written = write_atomic_private_with(&paths.credentials_file, bytes, 0o600, fence);
        self.settle_file(paths, Some(bytes), written.is_ok());
        written
    }
```

A write that fails leaves the place re-read under the same hold, so what it records is what the
place actually holds.

Wrap `write_credential_entry`: the public method takes the lock, reads the entry again and
rebases the bytes; the body it had becomes the private `write_entry`. Replace the doc comment and
the head of `write_credential_entry`:

```rust
    /// Appendix A.3 write, including the verified file fallback, which first reports every
    /// item it will delete to `before_fallback`. A fallback pins file mode, so every later
    /// write of the same operation goes straight to the file. Returns where this write put the
    /// credential: a file mirrored for hot reload does not make it a file store.
    pub fn write_credential_entry(
        &self,
        env: &Env,
        paths: &CcPaths,
        bytes: &[u8],
        fence: Fence<'_>,
        before_fallback: BeforeFallback<'_>,
    ) -> Result<SecretStore, ProviderError> {
        if !self.mac() {
```

with:

```rust
    /// Appendix A.3 write, including the verified file fallback, which first reports every
    /// item it will delete to `before_fallback`. A fallback pins file mode, so every later
    /// write of the same operation goes straight to the file. Returns where this write put the
    /// credential: a file mirrored for hot reload does not make it a file store.
    ///
    /// The whole write, its hot-reload rewrite or its fallback included, holds CC's
    /// storage-write lock (§9.1). Under it the entry is read again: its account-scoped keys must
    /// still be what this operation last read or wrote (`ProviderError::EntryMoved` otherwise),
    /// and the machine-shared keys written are the ones it holds now, whatever `bytes` carry.
    pub fn write_credential_entry(
        &self,
        env: &Env,
        paths: &CcPaths,
        bytes: &[u8],
        fence: Fence<'_>,
        before_fallback: BeforeFallback<'_>,
    ) -> Result<SecretStore, ProviderError> {
        self.under_storage_write(env, paths, ItemKind::OAuth, fence, |now, fence| {
            let bytes = rebase(bytes, &shared_of(now));
            self.write_entry(env, paths, &bytes, fence, before_fallback)
        })
    }

    /// `write_credential_entry`'s write, under the storage-write lock.
    fn write_entry(
        &self,
        env: &Env,
        paths: &CcPaths,
        bytes: &[u8],
        fence: Fence<'_>,
        before_fallback: BeforeFallback<'_>,
    ) -> Result<SecretStore, ProviderError> {
        if !self.mac() {
```

In `write_entry`'s Keychain branch, record and settle the item and the hot-reload rewrite. Replace

```rust
        if !self.file_mode_pinned() {
            fence()?;
            match self.keychain.upsert(
                &keychain_service(env, ItemKind::OAuth),
                &keychain_account(env),
                bytes,
            ) {
                Ok(()) => {
                    if paths.credentials_file.try_exists()? {
                        // Bumps the mtime, so CC reloads (hot reload).
                        write_atomic_private_with(&paths.credentials_file, bytes, 0o600, fence)?;
                    }
                    return Ok(SecretStore::Keychain);
                }
```

with

```rust
        if !self.file_mode_pinned() {
            let svc = keychain_service(env, ItemKind::OAuth);
            self.intend(ItemKind::OAuth, &svc, Some(bytes));
            fence()?;
            let upserted = self.keychain.upsert(&svc, &keychain_account(env), bytes);
            self.settle_item(env, ItemKind::OAuth, &svc, Some(bytes), upserted.is_ok());
            match upserted {
                Ok(()) => {
                    if paths.credentials_file.try_exists()? {
                        // Bumps the mtime, so CC reloads (hot reload).
                        self.intend(ItemKind::OAuth, &file_place(paths), Some(bytes));
                        let mirrored =
                            write_atomic_private_with(&paths.credentials_file, bytes, 0o600, fence);
                        self.settle_file(paths, Some(bytes), mirrored.is_ok());
                        mirrored?;
                    }
                    return Ok(SecretStore::Keychain);
                }
```

`write_entry`'s fallback records through `write_file` and `remove_items`; the rest of it is
unchanged.

Wrap `clear_credential_account_keys` the same way. Replace its doc comment and head:

```rust
    /// API-key activation: keep only the machine-shared keys of every credential entry a
    /// reader would try; delete an entry when none remain (§9.4 step 7).
    pub fn clear_credential_account_keys(
        &self,
        env: &Env,
        paths: &CcPaths,
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        if self.mac() {
```

with:

```rust
    /// API-key activation: keep only the machine-shared keys of every credential entry a
    /// reader would try; delete an entry when none remain (§9.4 step 7). It holds CC's
    /// storage-write lock throughout and refuses, writing nothing, if the entry's
    /// account-scoped keys moved (§9.1); what each place keeps is read under the lock.
    pub fn clear_credential_account_keys(
        &self,
        env: &Env,
        paths: &CcPaths,
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        self.under_storage_write(env, paths, ItemKind::OAuth, fence, |_, fence| {
            self.clear_account_keys(env, paths, fence)
        })
    }

    /// `clear_credential_account_keys`' clear, under the storage-write lock.
    fn clear_account_keys(
        &self,
        env: &Env,
        paths: &CcPaths,
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        if self.mac() {
```

In `clear_account_keys`, record and settle each item's clear. Replace

```rust
                    let kept = keep_shared(&b)?;
                    fence()?;
                    match kept {
                        Some(k) => self.keychain.upsert(&svc, &acct, &k)?,
                        None => self.keychain.delete(&svc, &acct)?,
                    }
```

with

```rust
                    let kept = keep_shared(&b)?;
                    self.intend(ItemKind::OAuth, &svc, kept.as_deref());
                    fence()?;
                    let cleared = match &kept {
                        Some(k) => self.keychain.upsert(&svc, &acct, k),
                        None => self.keychain.delete(&svc, &acct),
                    };
                    self.settle_item(env, ItemKind::OAuth, &svc, kept.as_deref(), cleared.is_ok());
                    cleared?;
```

and the file's. Replace

```rust
            let kept = keep_shared(&b)?;
            match kept {
                Some(k) => write_atomic_private_with(&paths.credentials_file, &k, 0o600, fence)?,
                None => {
                    fence()?;
                    remove_if_present(&paths.credentials_file)?
                }
            }
```

with

```rust
            let kept = keep_shared(&b)?;
            self.intend(ItemKind::OAuth, &file_place(paths), kept.as_deref());
            let cleared = match &kept {
                Some(k) => write_atomic_private_with(&paths.credentials_file, k, 0o600, fence),
                None => fence().and_then(|()| remove_if_present(&paths.credentials_file)),
            };
            self.settle_file(paths, kept.as_deref(), cleared.is_ok());
            cleared?;
```

The clear keeps each place's own machine-shared keys, as it did; it reads them under the lock
now.

In `write_managed_key`'s doc comment, replace

```rust
    /// effective key. Returns where this write put the key; the credential entry's file pin
    /// plays no part in it.
```

with

```rust
    /// effective key. Returns where this write put the key; the credential entry's file pin
    /// plays no part in it. The managed-key item's write, its fallback included, holds CC's
    /// storage-write lock and refuses if the item moved (§9.1).
```

and replace the end of its body, from `if self.mac() {` (after the `approved` splice):

```rust
        if self.mac() {
            fence()?;
            match self.keychain.upsert(
                &keychain_service(env, ItemKind::ManagedKey),
                &keychain_account(env),
                key_str.as_bytes(),
            ) {
                Ok(()) => {
                    config::splice_key(&paths.global_config, "primaryApiKey", None, fence)?;
                    return Ok(SecretStore::Keychain);
                }
                Err(e) => {
                    tracing::warn!("keychain write failed, storing primaryApiKey instead: {e}")
                }
            }
            self.report_items(env, ItemKind::ManagedKey, before_fallback)?;
        }
        config::splice_key(
            &paths.global_config,
            "primaryApiKey",
            Some(&Value::String(key_str)),
            fence,
        )?;
        if !self.mac() {
            return Ok(SecretStore::File(paths.global_config.clone()));
        }
        self.remove_items(env, ItemKind::ManagedKey, fence)?;
        Ok(SecretStore::Fallback(paths.global_config.clone()))
    }
```

with:

```rust
        let in_primary = |fence: Fence<'_>| {
            config::splice_key(
                &paths.global_config,
                "primaryApiKey",
                Some(&Value::String(key_str.clone())),
                fence,
            )
        };
        if !self.mac() {
            in_primary(fence)?;
            return Ok(SecretStore::File(paths.global_config.clone()));
        }
        let in_keychain =
            self.under_storage_write(env, paths, ItemKind::ManagedKey, fence, |_, fence| {
                let svc = keychain_service(env, ItemKind::ManagedKey);
                let key = Some(key_str.as_bytes());
                self.intend(ItemKind::ManagedKey, &svc, key);
                fence()?;
                let upserted =
                    self.keychain
                        .upsert(&svc, &keychain_account(env), key_str.as_bytes());
                self.settle_item(env, ItemKind::ManagedKey, &svc, key, upserted.is_ok());
                match upserted {
                    Ok(()) => return Ok(true),
                    Err(e) => {
                        tracing::warn!("keychain write failed, storing primaryApiKey instead: {e}")
                    }
                }
                self.report_items(env, ItemKind::ManagedKey, before_fallback)?;
                in_primary(fence)?;
                self.remove_items(env, ItemKind::ManagedKey, fence)?;
                Ok(false)
            })?;
        if !in_keychain {
            return Ok(SecretStore::Fallback(paths.global_config.clone()));
        }
        config::splice_key(&paths.global_config, "primaryApiKey", None, fence)?;
        Ok(SecretStore::Keychain)
    }
```

The non-macOS path and the order of the macOS fallback are unchanged; only the macOS Keychain
write and its fallback now run under the lock.

Replace `clear_managed_key`'s doc comment and its `if self.mac()` block:

```rust
    /// Writing OAuth clears the managed key: every managed-key item is deleted (verified) and
    /// `primaryApiKey` is dropped. `approved` is kept (B.10).
    pub fn clear_managed_key(
        &self,
        env: &Env,
        paths: &CcPaths,
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        if self.mac() {
            self.remove_items(env, ItemKind::ManagedKey, fence)?;
        }
```

with

```rust
    /// Writing OAuth clears the managed key: every managed-key item is deleted (verified) and
    /// `primaryApiKey` is dropped. `approved` is kept (B.10). The deletes hold CC's
    /// storage-write lock and refuse if the item moved (§9.1).
    pub fn clear_managed_key(
        &self,
        env: &Env,
        paths: &CcPaths,
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        if self.mac() {
            self.under_storage_write(env, paths, ItemKind::ManagedKey, fence, |_, fence| {
                self.remove_items(env, ItemKind::ManagedKey, fence)
            })?;
        }
```

At the end of `snapshot`, replace it and the `item_matches` helper after it, which `restore` no longer uses:

```rust
        Ok(Snapshot {
            oauth_items: items(ItemKind::OAuth)?,
            managed_items: items(ItemKind::ManagedKey)?,
            credentials_file: present_or_err(read_bytes(&paths.credentials_file))?,
            global_config: present_or_err(read_bytes(&paths.global_config))?,
        })
    }

    fn item_matches(&self, svc: &str, acct: &str, expected: &Option<Vec<u8>>) -> bool {
        match self.keychain.find(svc, acct) {
            Read::Present(v) => expected.as_deref() == Some(v.as_slice()),
            Read::Absent => expected.is_none(),
            Read::Unreadable(_) => false,
        }
    }
```

with

```rust
        let snap = Snapshot {
            oauth_items: items(ItemKind::OAuth)?,
            managed_items: items(ItemKind::ManagedKey)?,
            credentials_file: present_or_err(read_bytes(&paths.credentials_file))?,
            global_config: present_or_err(read_bytes(&paths.global_config))?,
        };
        // §9.1: this is the operation's latest read of both entries under the credential locks,
        // place by place.
        let places = |items: &[ItemSnapshot]| -> BTreeMap<String, Values> {
            items
                .iter()
                .map(|(svc, v)| (svc.clone(), vec![v.clone()]))
                .collect()
        };
        let mut oauth = places(&snap.oauth_items);
        oauth.insert(file_place(paths), vec![snap.credentials_file.clone()]);
        let mut seen = self.seen.lock().unwrap();
        seen.oauth.places = Some(oauth);
        seen.managed.places = Some(places(&snap.managed_items));
        drop(seen);
        Ok(snap)
    }
```

Extend `restore`'s doc comment. Replace its last line and the signature's first line:

```rust
    /// every one of them — Keychain services and file paths, never bytes.
    pub fn restore(
```

with

```rust
    /// every one of them — Keychain services and file paths, never bytes.
    ///
    /// Each credential entry, the managed-key item and then the OAuth entry with its file, is
    /// restored under its own hold of CC's storage-write lock (§9.1), and only if this
    /// operation's writes changed it. The lock is waited for under a token nothing sets: a
    /// rollback runs to completion (§14.1). Each place the operation changed is put back, byte
    /// for byte, to what it actually held just before the operation's first change there, read
    /// under the lock; for these entries that, not the snapshot, is what the paragraph above
    /// means. It is put back only while every place of the entry still holds what the operation
    /// last knew there (`observe`). Once another writer changed any place since, by a dead-token
    /// marking or in its machine-shared keys as much as otherwise, or a place's state before the
    /// operation's change is unknown, the whole entry is left as it is and each such place is
    /// named as not restored: putting back any place could overwrite that write, or hide it
    /// behind a place a reader tries first. A place the operation never changed is never
    /// written, but for the credentials file's hot-reload rewrite of its own bytes. A lock that
    /// cannot be taken, or an entry that cannot be read under it, leaves that entry unrestored
    /// too.
    pub fn restore(
```

In `restore`'s body, replace everything from step 2's comment through step 3's closing brace:

```rust
        // 2. The Keychain items: managed-key, then OAuth.
        let mut any_item_restored = false;
        for (i, (svc, value)) in snap
            .managed_items
            .iter()
            .chain(&snap.oauth_items)
            .enumerate()
        {
            if self.item_matches(svc, &acct, value) {
                continue;
            }
            match self.restore_item(svc, &acct, value, fence) {
                Ok(()) => any_item_restored = true,
                Err(e) => {
                    if matches!(e, ProviderError::Lock(_)) {
                        let mut never_attempted = item_names[i..].to_vec();
                        never_attempted.push(credentials_file_name);
                        log_lock_abort(&failed, &never_attempted);
                        return Err(e);
                    }
                    failed.push(svc.clone());
                }
            }
        }

        // 3. The credentials file last, so a Keychain item's hot-reload bump always
        // lands after that item. Never created for a snapshot that had none.
        let mismatched = !file_matches(&paths.credentials_file, &snap.credentials_file);
        let bump_for_reload = any_item_restored
            && snap.credentials_file.is_some()
            && paths.credentials_file.try_exists().unwrap_or(false);
        if mismatched || bump_for_reload {
            if let Err(e) =
                restore_file(&paths.credentials_file, &snap.credentials_file, fence, true)
            {
                if matches!(e, ProviderError::Lock(_)) {
                    log_lock_abort(&failed, std::slice::from_ref(&credentials_file_name));
                    return Err(e);
                }
                failed.push(credentials_file_name);
            }
        }
```

with:

```rust
        // 2. The Keychain items: managed-key, then OAuth; 3. the credentials file last, so a
        // Keychain item's hot-reload bump always lands after that item. Each entry under its
        // own hold of the storage-write lock (§9.1).
        let mut any_item_restored = false;
        let mut start = 0;
        for kind in [ItemKind::ManagedKey, ItemKind::OAuth] {
            let items = match kind {
                ItemKind::ManagedKey => &snap.managed_items,
                ItemKind::OAuth => &snap.oauth_items,
            };
            let at = start;
            start += items.len();
            // What each place this operation changed held just before its first change there.
            let first = self.seen.lock().unwrap().entry(kind).first.clone();
            if first.is_empty() {
                continue;
            }
            // The entry, named by its primary item, else its file (Linux).
            let name = items
                .first()
                .map_or_else(|| credentials_file_name.clone(), |(svc, _)| svc.clone());
            let lock = match self.storage_write(paths, &Cancel::new()) {
                Ok(lock) => lock,
                Err(e) => {
                    failed.push(format!("{name} ({e})"));
                    continue;
                }
            };
            let now = match self.places_now(env, paths, kind) {
                Ok(now) => now,
                Err(e) => {
                    failed.push(format!("{name} ({e})"));
                    continue;
                }
            };
            let changed = self.observe(kind, &now);
            if !changed.is_empty() {
                failed.extend(
                    changed
                        .into_iter()
                        .map(|place| format!("{place} ({CHANGED_SINCE})")),
                );
                continue;
            }
            let fence = held(fence, &lock);
            let mut file_now = None;
            for (i, (place, value)) in now.iter().enumerate() {
                if *place == credentials_file_name {
                    file_now = Some(value.clone());
                    continue;
                }
                let Some(Held::Known(before)) = first.get(place) else {
                    continue;
                };
                if value == before {
                    continue;
                }
                self.intend(kind, place, before.as_deref());
                let restored = self.restore_item(place, &acct, before, &fence);
                self.settle_item(env, kind, place, before.as_deref(), restored.is_ok());
                match restored {
                    Ok(()) => any_item_restored = true,
                    Err(e) => {
                        if matches!(e, ProviderError::Lock(_)) {
                            let mut never_attempted = item_names[at + i..].to_vec();
                            never_attempted.push(credentials_file_name);
                            log_lock_abort(&failed, &never_attempted);
                            return Err(e);
                        }
                        failed.push(place.clone());
                    }
                }
            }
            let Some(file_now) = file_now else {
                continue;
            };
            // The file goes back to what it held before this operation's change. One it never
            // changed is rewritten with its own bytes, if it exists, to bump its mtime after a
            // restored item; it is never created for that.
            let file = match first.get(&credentials_file_name) {
                Some(Held::Known(before)) if *before != file_now => before.clone(),
                _ if any_item_restored && file_now.is_some() => file_now,
                _ => continue,
            };
            self.intend(kind, &credentials_file_name, file.as_deref());
            let restored = restore_file(&paths.credentials_file, &file, &fence, true);
            self.settle_file(paths, file.as_deref(), restored.is_ok());
            if let Err(e) = restored {
                if matches!(e, ProviderError::Lock(_)) {
                    log_lock_abort(&failed, std::slice::from_ref(&credentials_file_name));
                    return Err(e);
                }
                failed.push(credentials_file_name.clone());
            }
        }
```

The config restore before it (step 1) and the final `Incomplete` after it are unchanged. Task 7's
`self.unpin_file_mode()` at the top of `restore` stays.

In `crates/tagteam-cc/src/provider.rs`, extend `OperationLocks`' doc comment. Replace

```rust
/// CC's credential locks, held for one operation: a switch, a recovery, a §7.5 pass or an
/// `add`. Releasing them ends the operation, and with it the Keychain file-mode pin (Appendix
/// A.3), so the next operation tries the Keychain again.
```

with

```rust
/// CC's credential locks, held for one operation: a switch, a recovery, a §7.5 pass or an
/// `add`. Releasing them ends the operation, and with it the Keychain file-mode pin (Appendix
/// A.3), so the next operation tries the Keychain again, and what the operation read and wrote
/// of each credential entry (§9.1), so the next one compares against its own reads.
```

and in its `Drop`, replace

```rust
        // Runs before `set` is dropped, so the pin ends while the locks are still held.
        self.live.unpin_file_mode();
```

with

```rust
        // Runs before `set` is dropped, so the operation ends while the locks are still held.
        self.live.end_operation();
```

In `crates/tagteam-engine/src/engine.rs`, add after `Engine::cancel` (Task 2):

```rust
    /// The Env for a write inside a critical span (§14.1): the same paths, with a cancel token
    /// nothing sets, so a lock wait the write makes (CC's storage-write lock, §9.1) runs to
    /// completion or times out. A signal stays recorded for the next cancellation point.
    pub(crate) fn critical_env(&self) -> Env {
        let mut env = self.env.clone();
        env.cancel = Cancel::new();
        env
    }
```

`Cancel` is already imported there (Task 2).

In `crates/tagteam-engine/src/switch.rs`, in `apply`, replace

```rust
        hooks::point(self, "after-journal")?;
        let stored_in = tx.write(|| {
            p.write_credential(&self.env, locks, target_login, before_fallback)
```

with

```rust
        hooks::point(self, "after-journal")?;
        // Steps 7–10 are a critical span (§14.1): the write's storage-write wait is not a
        // cancellation point, and neither is its rollback's.
        let env = self.critical_env();
        let stored_in = tx.write(|| {
            p.write_credential(&env, locks, target_login, before_fallback)
```

The undo the write returns holds a clone of this `Env`, but `restore` waits under its own fresh
token anyway (Ruling 2).

In `crates/tagteam-engine/src/recover.rs`, in `finish_forward`, replace

```rust
        p.clear_other_axis(&self.env, locks, &to.kind)?;
```

with

```rust
        // Recovery's writes are a critical span (§14.1): their storage-write wait is not a
        // cancellation point.
        p.clear_other_axis(&self.critical_env(), locks, &to.kind)?;
```

In `crates/tagteam-engine/src/active.rs`, in `publish`, replace

```rust
            // The undo is dropped, never run: it would write the consumed generation back.
            p.write_credential(&self.env, &locks, &login, &mut before_fallback)
```

with

```rust
            // The undo is dropped, never run: it would write the consumed generation back. The
            // storage-write wait honours `cancel`, as the config-lock wait did (§9.1, §14.1).
            p.write_credential(&env, &locks, &login, &mut before_fallback)
```

`env` is the clone Task 2 builds in `publish` for the config-lock wait, carrying `cancel`: a fresh
token after the request, the process token for the self-heal.

- [ ] **Step 11: Run them to verify they pass**

Run: `cargo test -p tagteam-cc`
Expected: PASS, including the sixteen new `live_store.rs` tests, the two new `provider.rs` tests
and the four reworked ones. Every pre-existing test passes unchanged, among them:
- `snapshot_and_restore_are_byte_exact`, `restore_bumps_the_credentials_file_after_the_item_it_reflects`,
  `restore_rewrites_a_matching_credentials_file_when_only_the_item_differed` and
  `restore_is_fenced_for_both_files_and_items_and_aborts_immediately_on_fence_failure`. The
  restore keeps its order (config, managed-key items, OAuth items, file last), its byte-exact
  writes, its hot-reload rewrite, and its count of fence checks: the storage-write lock's own
  check runs inside the same fence call;
- the `CountingFence` tests, for the same reason;
- Task 7's pin tests, since a restore still unpins first.

Run: `cargo test -p tagteam-engine --features test-hooks --test storage_write --test active --test switch_rollback`
Expected: PASS, the eleven `storage_write.rs` tests, every `active.rs` test, each new one in
under a second, and Task 7's `switch_rollback.rs` unchanged.

Run: `cargo test -p tagteam --features test-support --test signals`
Expected: PASS, three tests. The new one takes about a second: the switch waits until the test
lets go of the lock.

Run: `cargo test -p tagteam-engine --test active` and `cargo test -p tagteam-engine --test storage_write`
Expected: PASS; without `test-hooks` the hook tests and `storage_write.rs` are compiled out.

- [ ] **Step 12: Run the wider checks**

Run:
```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --features tagteam/test-support
cargo test --workspace
```
Expected:
- No output from `fmt --check`, and both clippy runs finish with no warnings. Without
  `test-hooks`, `tests/common/mod.rs` compiles without the CC helpers that need `on_point`.
- Every test passes, with and without features. The switch, rollback, recovery, §7.5 and §15.3
  invariant suites run unchanged: nothing in them holds `.storage-write`, so every write takes the
  free lock at once.

- [ ] **Step 13: Commit**

```bash
git add crates/tagteam-cc/src/live.rs crates/tagteam-cc/src/provider.rs \
  crates/tagteam-provider/src/provider.rs crates/tagteam-engine/src/engine.rs \
  crates/tagteam-engine/src/switch.rs crates/tagteam-engine/src/recover.rs \
  crates/tagteam-engine/src/active.rs crates/tagteam-cc/tests/live_store.rs \
  crates/tagteam-cc/tests/provider.rs crates/tagteam-engine/tests/common/mod.rs \
  crates/tagteam-engine/tests/active.rs crates/tagteam-engine/tests/storage_write.rs \
  crates/tagteam/tests/signals.rs
git commit -m "Take Claude Code's storage-write lock around every credential-entry write"
```

---

### Task 12: Final verification and live acceptance

**Human steps (Step 3 prepares; Steps 4–5 are the acceptance and Michael's).** Steps 1–2 are
the implementer's. The live run uses Michael's real Claude Code login and his real login
keychain. It sends real usage requests, may refresh stored tokens through the gate, and
switches his live login several times. Michael therefore runs it, and the merge waits for it.
An agent never runs Steps 3–5.

The whole-branch review and the pre-merge cross-review run after Step 2. They follow the
global workflow and are not steps of this plan. The live run is the last step before the
merge request (Execution notes).

**Files:**
- Modify: this plan's own file, `docs/superpowers/plans/2026-10-01-tagteam-m3a-signals-strategies.md`
  (the `**Status:**` line, Step 7)

**Interfaces:**
- Consumes: everything Tasks 1–11 produced. In particular:
  - exit `130`, `128 + n`, and `2` for a usage error;
  - `error.type` `interrupted` (`KIND_INTERRUPTED`);
  - `switch --strategy best|next-available [--model M]`;
  - the reasons `switched | already-active | already-best | usage-unavailable |
    candidates-exhausted`;
  - the §13.2 `switch` keys;
  - CC's storage-write lock, `~/.claude/.storage-write`: a switch's credential write waits up to
    9 s for it, and a timeout rolls the switch back (Task 11).
- Produces: nothing new.

- [ ] **Step 1: Format, lint and test everything**

Run:
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --features tagteam/test-support
cargo test --workspace --features tagteam/test-support -- --ignored
cargo test -p tagteam --lib
cargo test --release -p tagteam --features test-support --test perf -- --ignored
```

Expected:
- No diff, and no warnings with the test features or without them.
- Every test passes. Record the totals as passed/total for the merge request.
- The `--ignored` run includes M2a's 9 s CC-lock test, `gate_race`'s 15 s stopped-holder test,
  and any test M3a marked ignored.
- `cargo test -p tagteam --lib` runs without features, so it exercises the release branch of
  the test-override checks.
- The perf run passes both timing tests: `statusline` p95 ≤ 10 ms and `list` p95 ≤ 50 ms. M3a
  installs signal handlers at startup (Task 6), so this is the check that they cost nothing
  measurable. Run it on an otherwise idle machine. One failure gets one re-run; a second
  failure is a real miss of the budget, so stop and report the measured p95.

On a Mac, also run:
`cargo test -p tagteam-provider --features real_keychain --test real_keychain`

Expected: PASS, with no GUI prompt. It drives the real `/usr/bin/security` through
`ProcessRunner`, which now spawns it in its own process group (Task 4).

Run: `cargo check -p tagteam-core -p tagteam-provider -p tagteam-cc -p tagteam-fake --target x86_64-unknown-linux-gnu`

Expected: `Finished`. Task 4's `process_group` and the cancel token build for Linux too. The
engine and the binary are left out because bundled SQLite's build script needs a Linux C
cross-compiler. If this Mac has one, add `-p tagteam-engine -p tagteam`, which also checks
Task 6's `poll(2)` prompt and `signal-hook` registration.

- [ ] **Step 2: Build the release binary and check it carries no test hooks**

Run:
```bash
cargo build --release -p tagteam
grep -a -c TAGTEAM_TEST_ target/release/tagteam ; test $? -eq 1
```

Expected:
- The build succeeds.
- `grep` prints `0` and exits 1, so the final `test` exits 0.
- The crash points and every test override exist only under `test-support`; `signal-hook` is
  an ordinary dependency.

- [ ] **Step 3: Michael — prepare the acceptance run**

The goal of the run:
- A real Ctrl-C, SIGTERM or SIGHUP stops tagteam cleanly, with the right exit code, and leaves
  no Claude Code lock directory behind.
- The terminal survives a Ctrl-C at a secret prompt.
- A switch's credential write waits for Claude Code's storage-write lock, gives up after 9 s,
  and is not stopped by a signal while it waits.
- The usage strategies pick what the numbers say they should.

No token is handled by hand.

1. From the worktree root, put the Step 2 binary first on `PATH` for this shell:
   ```bash
   export PATH="$PWD/target/release:$PATH"
   command -v tagteam
   ```
   Expected: `<worktree>/target/release/tagteam`. The binary is new and ad-hoc signed, so if a
   request is blocked, suspect Little Snitch and allow `api.anthropic.com` and
   `platform.claude.com` for it.
2. **Quit every Claude Code session** until Step 4 ends. Rows 1, 4 and 5 create CC's refresh
   lock by hand, and rows 6–8 its storage-write lock; a running `claude` would wait on them.
   With CC quit, any lock directory that shows up belongs to tagteam or to you.
3. Run `tagteam list`. You need at least two stored accounts, one of them live. Note the live
   account's position as **P0**, and pick another stored position as **Q**.
4. Define these helpers. They print positions, ids, statuses and numbers, never an email or a
   token:
   ```bash
   # CC's four lock directories (§9.1); the legacy one sits next to ~/.claude's real path.
   tt_locks() {
     local n=$(ls -d ~/.claude/.oauth_refresh.lock "$(realpath ~/.claude).lock" ~/.claude.json.lock ~/.claude/.storage-write 2>/dev/null | wc -l | tr -d ' ')
     echo "locks left: $n"
   }
   # The roster: (position, id, active) per account.
   tt_accounts() {
     tagteam list --json | /usr/bin/python3 -c '
   import json, sys
   print(sorted((a["position"], a["id"], a["active"]) for a in json.load(sys.stdin)["accounts"]))
   '
   }
   # Each account's decision-grade percentages; "*" marks the live one.
   tt_heads() {
     tagteam list --json | /usr/bin/python3 -c '
   import json, sys
   for a in json.load(sys.stdin)["accounts"]:
       u = a.get("usage") or {}
       p = {"5h": (u.get("fiveHour") or {}).get("pct"), "7d": (u.get("sevenDay") or {}).get("pct")}
       for s in u.get("scoped") or []:
           p[s["name"]] = s.get("pct")
       print(a["position"], "*" if a["active"] else " ", a["usageStatus"], p)
   '
   }
   # Holds CC's refresh lock by hand, starts a switch to $2 that waits on it, sends signal $1
   # after 2 s, and reports how long the switch took to stop. Always removes the stand-in lock.
   tt_signal() {
     mkdir ~/.claude/.oauth_refresh.lock || return
     tagteam switch "$2" --json > "$TMPDIR/tt-signal.json" &
     local pid=$!
     sleep 2
     local t0=$(/usr/bin/python3 -c 'import time; print(time.time())')
     kill -"$1" $pid
     wait $pid
     local code=$?
     /usr/bin/python3 -c "import time; print('took', round(time.time() - $t0, 2), 's')"
     echo "exit $code"
     cat "$TMPDIR/tt-signal.json"
     rmdir ~/.claude/.oauth_refresh.lock
   }
   # Holds CC's storage-write lock (§9.1) by hand and starts a switch to $1, which waits on it.
   # With a signal name as $3, sends it 1 s in. Lets go of the lock $2 s in ("never": keeps it
   # until the switch gives up), reports how long the switch took, and always removes the
   # stand-in lock.
   tt_storage_write() {
     local lock=~/.claude/.storage-write
     mkdir "$lock" || return
     local t0=$(/usr/bin/python3 -c 'import time; print(time.time())')
     tagteam switch "$1" &
     local pid=$!
     local rest=$2
     if [ -n "$3" ]; then sleep 1; kill -"$3" $pid; rest=$(( $2 - 1 )); fi
     if [ "$2" != never ]; then sleep "$rest"; rmdir "$lock"; fi
     wait $pid
     local code=$?
     /usr/bin/python3 -c "import time; print('took', round(time.time() - $t0, 2), 's')"
     echo "exit $code"
     if [ -d "$lock" ]; then rmdir "$lock"; fi
   }
   # The keys and fields of one `switch --json` result.
   tt_json() {
     /usr/bin/python3 -c '
   import json, sys
   o = json.load(open(sys.argv[1]))
   print(sorted(o))
   print({k: o[k] for k in ("schemaVersion", "provider", "switched", "from", "to", "strategy", "reason", "credentialStore")})
   print(len(o["warnings"]), "warnings")
   ' "$1"
   }
   ```
   The lock helpers assume Claude Code's secure-storage directory is `~/.claude`. If
   `CLAUDE_SECURESTORAGE_CONFIG_DIR` or `CLAUDE_CONFIG_DIR` is set in this shell, use the
   directory Appendix A.1 names instead.
5. Run `tt_locks` and `tt_accounts`. Expected: `locks left: 0`. Keep the `tt_accounts` line as
   **A0**. If a lock is left, stop: a Claude Code process is still running, or a lock was
   left over, and the rows below could not tell whose it is.

**About the hand-made lock** (rows 1, 4 and 5). `mkdir ~/.claude/.oauth_refresh.lock` stands
in for Claude Code refreshing its token, because a real refresh cannot be timed by hand.
- **Remove it the moment the row ends.** While it exists, any Claude Code started on this
  machine waits on it for its token refresh.
- **Create it immediately before the switch.** A lock directory older than 60 s is stale, and
  tagteam would take it over and actually switch. Row 1 therefore chains the `mkdir` and the
  switch on one line, and `tt_signal` does both within two seconds.
- tagteam gives up on a held lock by itself after 9 s, so a Ctrl-C belongs in the first few
  seconds.

**About the hand-made storage-write lock** (rows 6–8). `tt_storage_write` makes
`~/.claude/.storage-write` to stand in for Claude Code writing its credential, which it does
under this lock for every write, a token refresh or an MCP token update included (§9.1).
- **It must be removed the moment the row ends.** While it exists, every Claude Code session on
  this machine waits on it before any credential write. The helper removes it on every path;
  if a row is cut short, run `rmdir ~/.claude/.storage-write` at once, then `tt_locks`.
- **It must be made immediately before the switch.** A storage-write lock older than 15 s is
  stale, and tagteam (or Claude Code) takes it over and goes ahead. Row 6 would then switch
  instead of timing out. The helper makes it and starts the switch on one line, and tagteam
  gives up after 9 s, well inside the 15.

- [ ] **Step 4: Michael — run the acceptance**

Run every row in this order, in the same shell. After each row, `tt_locks` must print
`locks left: 0`.

| # | Run | Do | Pass when |
|---|---|---|---|
| 1 | `mkdir ~/.claude/.oauth_refresh.lock && tagteam switch Q`, then `echo "exit $?"`, `tagteam status`, `tt_accounts`, `rmdir ~/.claude/.oauth_refresh.lock`, `tt_locks` | Press Ctrl-C 2–5 s after starting the switch, while it waits | The switch returns right after the Ctrl-C, well within a second and long before its own 9 s timeout. Stderr has one `tagteam:` line saying it was interrupted. `exit 130`. `status` still names the account at P0, and `tt_accounts` equals A0. After the `rmdir`, `locks left: 0`: tagteam never created the legacy or config lock |
| 2 | `tagteam add --position Q`, then `echo "exit $?"`, `tt_accounts`, `tt_locks` | At `Position Q holds … Replace it? [y/N]`, press Ctrl-C. **Never answer `y`: that replaces account Q** | `exit 130` and a `tagteam:` interrupted line. `tt_accounts` equals A0, so nothing was added or replaced. `locks left: 0` |
| 3 | `tagteam add-token`, then `echo "exit $?"`, then `stty -a \| tr -s ' \t' '\n' \| grep -x -e isig -e -isig -e echo -e -echo`, `tt_accounts`, `tt_locks` | At `Token: `, type a few characters (not a real token; they are not echoed), then press Ctrl-C | `exit 130` and a `tagteam:` interrupted line. The `stty` command is visible as you type it, and it prints `isig` and `echo`, not `-isig` or `-echo`: the terminal's echo and its Ctrl-C are both restored. `tt_accounts` equals A0. `locks left: 0` |
| 4 | `tt_signal TERM Q`, then `tt_accounts`, `tt_locks` | Nothing: the helper sends SIGTERM after 2 s | `took` is under 1 s. `exit 143`. The file holds exactly `{"schemaVersion":1,"error":{"type":"interrupted",…}}`, which only tagteam's handler prints; a process the signal killed would print nothing. `tt_accounts` equals A0. `locks left: 0` |
| 5 | `tt_signal HUP Q`, then `tt_accounts`, `tt_locks` | Nothing: the helper sends SIGHUP | As row 4, with `exit 129` |
| 6 | `tt_storage_write Q never`, then `tagteam status`, `tt_accounts`, `tt_locks` | Nothing: the helper holds the lock until the switch gives up | `took` is between 9 and 10.5 s: the switch waited the storage-write lock's full 9 s after taking CC's other locks. `exit 1`. Stderr has `tagteam: the switch failed and was rolled back: timed out waiting for the lock …/.claude/.storage-write`. `status` still names the account at P0, and `tt_accounts` equals A0: nothing was written. `locks left: 0` |
| 7 | `tt_storage_write Q 3`, then `tagteam status`, then `tagteam switch P0`, `tt_accounts`, `tt_locks` | Nothing: the helper lets go of the lock 3 s in | `took` is between 3 and 4.5 s: the switch waited for the lock and went ahead as soon as it was gone. `exit 0` and `Switched to … (position Q).`; `status` names Q. After `switch P0`, `tt_accounts` equals A0. `locks left: 0` |
| 8 | `tt_storage_write Q 3 TERM`, then `tagteam status`, then `tagteam switch P0`, `tt_accounts`, `tt_locks` | Nothing: the helper sends SIGTERM 1 s in, while the switch's credential write waits for the lock, and lets go 3 s in | The signal arrived inside the switch's critical span (§14.1), so it did not stop it. `took` is between 3 and 4.5 s. `exit 0` and `Switched to … (position Q).`, with one more stderr line, `tagteam: interrupted too late to stop: … had already finished`. `status` names Q. After `switch P0`, `tt_accounts` equals A0. `locks left: 0` |
| 9 | `tagteam switch Q --strategy best; echo "exit $?"`, then `tagteam switch --model all; echo "exit $?"` | Nothing | Each prints a usage error naming the conflicting option and `exit 2`. Nothing switches: `tt_accounts` equals A0 |
| 10 | `tt_heads`, then `tagteam switch --strategy best; echo "exit $?"`, `tagteam status`, `tt_locks` | From `tt_heads`, work out the expected result **before** running the switch (see below) | `exit 0`, and the result matches the expectation. On a switch: `Switched to … (position N).`, and `status` names N. Otherwise no switch and a message giving the reason. If any candidate showed no usage, a stderr warning says how many. `locks left: 0` |
| 11 | `tt_heads`, then `tagteam switch --strategy next-available; echo "exit $?"`, `tagteam status`, `tt_locks` | Work out the expected result from `tt_heads` first (see below) | `exit 0`, and the result matches: switched to the first position after the live one, wrapping, whose headroom is not known to be ≤ 0. Each skipped account is named with its binding window. If every candidate was skipped, no switch and `candidates-exhausted`. `locks left: 0` |
| 12 | `tt_heads`, then `tagteam switch --strategy best --model all; echo "exit $?"`; then `tt_heads` again, then `tagteam switch --strategy next-available --model all; echo "exit $?"`; `tt_locks` | Recompute each expectation counting **every** scoped column `tt_heads` prints as well as 5h and 7d | Both `exit 0`, and both results match the recomputed expectations. With no scoped column on any account, they match what rows 10 and 11's rule gives for the current live account. `locks left: 0` |
| 13 | `tagteam switch --strategy best --json > "$TMPDIR/tt-best.json"; echo "exit $?"`, then `tt_json "$TMPDIR/tt-best.json"`, `tt_locks` | Nothing | `exit 0`, and `tt_json` parses the file (it fails on anything but one JSON object). The keys are exactly `['credentialStore', 'from', 'message', 'provider', 'reason', 'schemaVersion', 'strategy', 'switched', 'to', 'warnings']`. `schemaVersion` is `1`, `provider` `claude-code`, `strategy` `best`. `reason` is one of `switched`, `already-best`, `usage-unavailable`, `already-active`. `switched` is true exactly when `reason` is `switched`. `from` is the live position before the run. `to` is the pick when switched, else `null`. `credentialStore` is `keychain` when switched, else `null`. `locks left: 0` |
| 14 | `tagteam switch P0; echo "exit $?"`, then `tt_accounts`, `tt_locks` | Nothing: this restores the starting login | `exit 0` (or `already-active` if P0 is live already). `tt_accounts` equals A0. `locks left: 0` |

**Working out the expectation for rows 10–12.** Use only rows whose status is `ok`; any other
status means unknown headroom. Each such account's headroom is `100 −` its highest relevant
percentage (§8.2). The relevant percentages are:
- `5h` and `7d`;
- with `--model all`, every scoped column as well;
- without `--model`, only the scoped columns named in `autoswitch.models`, when
  `~/.config/tagteam/config.toml` sets it. It is empty by default.

The expected result for each strategy:
- **`best`** switches to the candidate with the most headroom, ties to the lower position, but
  only if that headroom is strictly above the live account's. Otherwise the reason is
  `already-best`. If the live account itself is unknown, `best` switches to the best known
  candidate, with a warning. If no candidate is known, the reason is `usage-unavailable`.
- **`next-available`** walks from the live position as a bare `switch` does. It skips a
  candidate only when its headroom is known and ≤ 0; an unknown one is never skipped.

The strategy collects usage just before it plans. `tt_heads` (a `list`) collects too, and a
reading under 180 s old is not fetched again, so both see the same numbers when run
back to back.

Extended, optional (Review Focus 4, the double-fired hotkey): when `tt_heads` shows a
candidate that beats the live account, run
`tagteam switch --strategy best & tagteam switch --strategy best; wait`. Exactly one of the
two prints `Switched to …`. The other reports no switch (`already-active`, or `already-best`
if it planned after the first had committed). `tagteam status` then names the pick, never a
third account. Run row 14 again afterwards.

The other confirmation prompt, `Add the current login (<email>) first? [Y/n]`, is
optional. It appears only when the live login is not stored, so check it only if such a login
is at hand: run `tagteam switch` and press Ctrl-C at the prompt. It goes through the same
prompt code as row 2, and passes on the same terms.

- [ ] **Step 5: Michael — decide and record**

- **Pass (rows 1–14):** keep the table for the merge request description, with exit codes,
  `took` times, reasons and positions, plus the Step 1 totals as passed/total. Never record a
  token or an email. Then continue with Step 6.
- **Fail:** stop. Do not open or merge the merge request. Bring the table to Michael. Where to
  look:
  - Rows 1, 4 and 5: the cancellable lock waits, the handlers and the exit codes (Tasks 1, 2
    and 6).
  - A lock directory left behind after any row: Tasks 1–3.
  - Rows 2 and 3: the prompts (Task 6).
  - Rows 6–8, and a `.storage-write` left behind after any row: the storage-write lock
    (Task 11).
  - Row 9: the clap rules (Task 10).
  - Rows 10–12: ranking, planning or rendering (Tasks 8, 9 and 10).
  - Row 13: the JSON (Task 10).

- [ ] **Step 6: Open the merge request**

The driver pushes the branch and opens the Draft MR, with the Step 5 table in its description
under the repository's template rules. This is not an agent command of this plan.

- [ ] **Step 7: Mark the plan implemented**

This is the final pre-merge commit, made once review has converged and merging is the next
action. Set line 3 of this plan from whatever it says now (`In progress`, with or without the
MR reference) to:

```markdown
**Status:** Implemented — <the Draft MR's URL>
```

This follows the design-record rules: the plan is Implemented and cites its MR. The spec
stays `In progress` until M5.

```bash
git add docs/superpowers/plans/2026-10-01-tagteam-m3a-signals-strategies.md
git commit -m "Mark the M3a plan implemented"
```

The driver pushes this commit to the open MR.
