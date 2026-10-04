# tagteam M4b — `tagteam run` Implementation Plan

**Status:** In progress

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `tagteam run [ACCOUNT]` launches `claude` on that account in its own profile, beside the default login. The profile shares memory, history and settings with the default home, and never shares a rotating token. Every way a session ends leaves the accounts consistent: a clean exit, a killed `tagteam`, a killed `claude`, or a crash. The session's `.claude.json` changes merge back into `~/.claude.json`. `map`, `unmap` and `shell-init` make it automatic per directory.

**Architecture:**
- **Pure policy in `tagteam-core`:** the three-way merge of `projects` and `mcpServers`.
- **Process primitives in `tagteam-provider`:**
  - a captured, cancellable spawn with an environment and a working directory, which validation uses;
  - the attached session spawn that inherits exactly one fd;
  - `exec`, a `PATH` lookup, and `Cancel::take`;
  - the launch reservation file.
- **Claude Code's facts in `tagteam-cc`:** the session environment (set and scrub), the seed and merge-back of the profile's `.claude.json`, the profile credential write, and validation (`claude auth status`). `FakeAgent` implements each with its own shapes.
- **The run protocol in `tagteam-engine`:** the launch decision, bootstrap, the launch under `MutationGuard` and the account lock with §12.5's environment warnings, the per-launch login check, and exit handling. It builds on M4a's provenance, session state, link sync and run-shell detection.
- **The CLI:** `run`'s parse, its `exec` path, its spawn and wait loop with signal forwarding, the exit code, and `map`, `unmap` and `shell-init`.

**Tech Stack:** Rust (edition 2024), libc (`flock`, `fcntl`, `kill`), signal-hook (from M3a), serde_json, clap 4 (`last = true` trailing arguments). Tests use tempfile, assert_cmd, a generated fake `claude` shell script on `PATH`, `FakeKeychain`, `FakeProcessProbe` and `ScriptedHttp`.

**Spec:** `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md` at `87365a1` (on `main` `7ee733b`; the profile-read wording that M4a's Decision 19 and Decision 22 here follow), with M4a's Decision 16 amending §12.6 (signed off 2026-10-01). Read these before starting any task:
- §3, §4.3, §4.5, §5, §6.2;
- §7.2, §7.3;
- §9.1, §9.5, §10.3;
- §12 (all of it), §13.1, §14.1;
- §15.2, §15.3;
- Appendix A.1, A.3, A.6, A.7;
- Appendix B 28–37, 43–46 and 57–63.

**Builds on:**
- **M4a's plan** (`docs/superpowers/plans/2026-10-01-tagteam-m4a-sessions-foundation.md`): its Interface Contract is binding here. That covers profile files, `RunShell`, `detect_run_shell`, `SessionState`, `Engine::session_state`, `apply_provenance`, `sync_profile_links`, the twelve `Provider` session methods, and `FakeProcessProbe`.
- **M3a**, merged into `main` at `7ee733b` (its plan: `docs/superpowers/plans/2026-10-01-tagteam-m3a-signals-strategies.md`): `Cancel`, `signals::install`, `EngineError::Interrupted`, `Ended`, process groups, and the storage-write lock (`acquire_storage_write`, `under_storage_write`).

## Execution notes

- **M4a's execution changed one signature this plan calls:** `Provider::delete_profile_credential(env, dir, spelling)`, where `dir` is the profile's actual directory. When `dir` is a real directory it takes the profile's own credential locks, then CC's storage-write lock around each delete, so it is never called while either is held. Task 9's bootstrap step 5 passes `profile` to both calls; the re-sync checks every other call and the lock nesting. M4a's final review also changed `EngineError::SessionOwned` (it gains `unreadable`) and `EngineError::ProfileSplit` (it gains `cause: SplitCause`), so `launch`'s mapping of `ProfileSplit` follows its cause. `probe_lock` now takes a shared lock, which still sees a launcher's exclusive lock as held. M4a's Execution rulings list the rest.
- **Execution starts after M4a is merged into `main`** (M3a merged at `7ee733b`). The first step rebases this branch and re-syncs this plan with M3a's and M4a's final code: names, line numbers and every interface listed under "Builds on". Record the re-sync under "Execution rulings" before Task 1.
- When execution starts, set this plan's `**Status:**` to `In progress` in one commit.
- Run in the `m4-run-sessions` worktree.
- Feature flags as M4a: `tagteam/test-support` enables them all. Engine hook tests run with `cargo test -p tagteam-engine --features test-hooks`.
- Clippy passes with and without `--features tagteam/test-support`. Every task runs `cargo fmt --all` first.
- Tests never reach the real network, the real HOME, the login keychain, or the real `claude` (§15.1). CLI tests put a generated fake `claude` first on `PATH` (Task 3's `tests/common` helper). Engine tests reach validation through a provider whose spawn is scripted.
- Task 14 is a human step: Michael's live acceptance with the real `claude`, the last step before the merge request.
- **Run as a non-root user.** Tasks 10 and 11's `block_home` relies on a 0500 home being unwritable, and the full suite includes M4a's unlistable `0o000` directories (its Tasks 6 and 9). Root can write and list both, so those tests would fail as root.
- **fish's wrapper text is pinned but was never run.** Task 2's unit test pins it, and `map_cli.rs` skips a shell that is not installed; fish was not installed where this plan was drafted. Task 14's row 7 runs it where fish is installed.
- **The Linux paths are first run by CI's ubuntu job or by the Docker recipe** (Task 14 Step 1): `/dev/fd` as `/proc/self/fd` in Task 3's descriptor test, dash as `/bin/sh` under the fake `claude`, its 50 ms background sleeps and traps, and `ps -o lstart=` in its session records. Nothing in this plan has run on Linux yet.

### Execution rulings

**Re-sync with the merged code (2026-10-02).** The branch is `main` at `f85d2be`, which has M3a, M3b (PR #5), the release pipeline (PR #3) and M4a (PR #6). Every task was compared with that code, and `cargo check --workspace --all-targets` passed. Anchors and line numbers go to each task's implementer. These rulings change what a task does:
- **Tasks 1, 2 and 12, M3b's `auto`.** The late-notice test, `command_name` and `run_shell_cli.rs`'s `COMMANDS` go in as deltas that keep M3b's `auto` entries.
  - Task 2 also adds `shell-init zsh`, and Task 12 adds `run` and `run 1`, to the commands that must refuse under an unreadable marker.
  - `run` keeps M3b's `stops_on_a_signal` and its SIGPIPE exemption, and gains only the `Ended::Child` arm.
  - `run_command` maps `dispatch`'s exit code with `.map(Ended::Code)`.
- **Task 4.** `remove_dead_reservations` treats a profile path that is not a directory as having no reservations, as M4a's `launch_reservations` does.
- **Task 5.** `.tagteam-baseline.json` is read under M4a's own-file rule. A link there that resolves to nothing is unreadable, so the merge-back fails, keeps the baseline, and the launch aborts. CC's own `.claude.json` is still written through its link.
- **Task 8.** `plan_run` refuses an unreadable run shell first.
- **Task 9.**
  - Before the credential write, the bootstrap checks that the profile path is a real directory, and refuses a symlinked one.
  - On an `invalid` login it checks for a profile split before deleting the profile.
  - Its step-4 write releases the profile's locks before step 5's `delete_profile_credential` takes them again.
  - Two `EngineConfig` sites the plan does not list get the new field: `FakeFx::engine_located` and M3b's unit-test engine in `crates/tagteam/src/auto.rs`.
- **Task 10, the launch guard.** The 30 s launch guard is built on M3b's guard functions and keeps their event source and recovery mapping: `guard_recovering` gains a timeout, and `guard_or_refuse_within(provider, timeout)` wraps a private `guard_or_refuse_for`.
- **Task 10, `freshen_for_launch`.** It checks the cancel token before the gate. It judges "due" on the raw vault, because the gate and the launch settle provenance themselves.
- **Task 10, tests.** New tests pin `profile-split`'s two launch causes.
- **Task 11, the provenance body.** Its exit-time copy of the provenance body is a delta onto M4a's merged body. It keeps the final review's rule that an absent identity stops only a capture.
- **Task 11, `finish_locked`.** It runs the reservation's unlink on every path once its locks are held.
- **Task 12.** Inside a run shell, the recorded presence of `CLAUDE_SECURESTORAGE_CONFIG_DIR` follows the outer home, because M4a's outer home rewrites it. At most one corner-case warning is affected.
- **Task 14.** The `--ignored` pass skips PR #3's two network tests by name.

**Rulings during the tasks (2026-10-02 to 2026-10-05):**
- **Test hygiene.**
  - Fake `claude` scripts are installed through a child process (`install_script`). A Linux loop showed 5 failures in 30 runs with ETXTBSY when sibling threads forked while a script was being written.
  - Kill tests null the fake's stdio, so an orphaned `sleep` never holds the harness's pipes.
  - The pipe `drain` is shared by `process.rs` and `security.rs`.
  - One test pause protocol is used, the engine's `pause_at`.
- **Task 5.** Both providers share tagteam's own-file rule: `read_own_bytes`, `has_own_file`, `remove_own_file` and `write_own_json_with`. So a baseline behind a dangling link is unreadable, not absent.
- **Task 6.** The credential write gets the current spelling, while step 2 reads with the recorded one. A symlinked `<profile>/.credentials.json` is refused in each provider's write, through a shared `refuse_linked_credential`, which keeps the engine provider-neutral. The write is never called while the default home's credential locks are held.
- **Task 7.** A `claude auth status` reply with `loggedIn: false` and a method other than `none` or `claude.ai` is `unknown`, not `invalid`. `invalid` deletes the profile, which holds the only copy of its MCP OAuth tokens.
- **Task 8.** A disabled account, named or mapped, plans as a session (§9.3). A mapping re-pointed between the plan and the locks is not re-checked; §12.5 step 1 checks only the live login.
- **Task 9.**
  - `displace_held` aborts on an unreadable identity or seed once the held credential is not the vault's (§4.3, B.5).
  - `mark_profile` refuses a profile path that is not a real directory, so nothing is written through a link.
- **Task 10.**
  - A join of a running session is exempt from the stored-login refusals; it touches no credential.
  - Before the locks, a gate `Dead`, `Unpersisted`, rescued `Transient`, `rescue-unreadable` or `Conflict` is carried across them. A quiescent launch then refuses with the same error before anything is written (§12.3 step 1), and a join goes on.
- **Task 11.** If the login check is interrupted, the CLI takes the signal before exit handling, so exit handling completes. It then exits 128 + n.
- **Task 12.** `abandon` always takes a pending signal before exit handling.
- **Task 13.** The run surfaces are built from scratch: `projects` and `mcpServers`, plus the create-only entries. Negative probes prove a stray identity write fails.

**Rulings from the final whole-branch review (2026-10-05):**
- **Log lines.** Exit handling logs an error's kind, never its message (B.69). A merge-back error after a failed capture is logged, not dropped.
- **Exit notice.** For a `profile-conflict` or an unreadable read, it ends with the error's own remedy.
- **Linked config.** The seed refuses a profile `.claude.json` that is a link (shared `refuse_linked_file`). FakeAgent's `identity.json` is refused the same way.
- **Merge summary.** Bootstrap's merge-back of a waiting baseline puts its §12.4 summary in the launch's warnings. On an aborted launch it stays at WARN only.
- **Interface changes against the contract above:**
  - `bootstrap_profile`, `seed_of` and `prepare_quiescent` return that summary.
  - An orphaned `<pid>.lock` is the new `EngineError::ReservationHeld` (kind `launch-unreachable`).
- **Flaky test.** M4a's flaky log-capture test is fixed: a permanent no-op subscriber stops tracing-core caching an event as never wanted.
- **Parked for M5's logging audit:**
  - log lines older than M4b that interpolate error messages (`engine.rs`, `refresh.rs`, `rescue.rs`, `lifecycle.rs`, `switch.rs`, `views.rs`, and CC's `live.rs` and `provider.rs`);
  - absolute paths in log fields, where §14.2 asks for `~/…`.

## Milestones

| Milestone | Scope |
|---|---|
| M3a, M4a | Prerequisites; merged before this plan executes |
| **M4b (this plan)** | Mappings, `shell-init`, process primitives, launch reservations, the three-way merge, seed and merge-back, the profile credential write, the session environment, validation, the launch decision, bootstrap, launch, the per-launch login check, exit handling, the `run` command and its signals, the run invariants, live acceptance |
| M3b | Auto-switch (`auto`), which skips session-owned candidates through M4a's interfaces |
| M5 | Export/import, `doctor`, `config` (writes), `displaced`, `purge`, `completions`, logging to file, `cargo xtask compat`, release |

**Deliberately absent from M4b:**
- `doctor`'s session checks (§13.6) are M5.
- `purge` deleting profiles is M5. It reuses Task 9's profile removal (M4a).

## Decisions

Each names what it would cost if wrong.

1. **`run`'s wait loop consumes the one-slot cancel token.** It calls a new `Cancel::take()` (an atomic swap to 0) every 50 ms:
   - SIGTERM and SIGHUP are forwarded to `claude` with `kill(pid, sig)`;
   - SIGINT is dropped, because the terminal sends it to `claude`, which is in the same foreground group;
   - SIGQUIT gets a no-op handler, registered before the spawn, so tagteam survives Ctrl-\ while `claude` keeps the default disposition (a handled signal resets on `exec`; an ignored one would not);
   - right before the spawn, the token is checked: a signal ends the launch;
   - the first `take` after `spawn()` returns forwards whatever arrived during the spawn, SIGINT included (§12.5);
   - after `claude` exits, a `take` clears what forwarding left, so the late-signal notice never fires and exit handling sees only new signals.

   Cost if wrong: a SIGTERM and a SIGINT within one 50 ms slice keep only the later one, which is the slot's own limit. A SIGINT landing between `exec` and the first `take` reaches `claude` twice.
2. **Spawning is a new API beside `Runner`.** `tagteam_provider::process` gets `SpawnSpec`, `run_captured` (own process group, timeout, cancel, `killpg` on either, stdout captured), `spawn_session` (attached, foreground group, inherits exactly one fd), `exec_command` and `find_on_path`. `Runner` and `SecurityCli` are untouched. Cost if wrong: two spawn helpers to merge later.
3. **The reservation fd is inherited through `pre_exec`.** It clears `FD_CLOEXEC` on that one fd in the child, so no other child (the `security` calls, the validation spawn) ever holds a reservation. `unsafe` stays in `tagteam-provider`. Cost if wrong: none; this is the standard idiom.
4. **The three-way merge is a pure `tagteam-core` function** at §12.4's granularity: `projects.<path>.<key>` and `mcpServers.<name>`. The default file wins a conflict. The CC provider applies the result with the §9.5 splice of the whole `projects` and `mcpServers` values, which §3 allows ("by three-way merge"). Cost if wrong: re-rendered whitespace inside those two subtrees, which the identity surface already allows.
5. **Seed and merge-back are provider methods.** The engine owns the order and the locks, the provider owns the `.claude.json` shape (§4.5 `seed_profile` / `merge_back`). The baseline is `<profile>/.tagteam-baseline.json`, written by the seed and removed by a successful merge-back. A baseline left behind is "a merge-back that never ran" (§12.4).
6. **The bootstrap credential write is file-only.** A new `LiveStore::write_credential_file` writes `<profile>/.credentials.json` under the profile's own credential locks and storage-write lock (M3a's `under_storage_write`), and never upserts a Keychain item: §12.3 step 4 says tagteam never writes the profile's item. Under the lock it rebases the machine-shared keys from the re-read only when the re-read finds the entry. An entry absent there, and absent at the operation's last read, keeps the composed bytes: Claude Code wrote nothing there (Decision 22). Cost if wrong: a profile whose item tagteam wrote, which step 5 deletes anyway.
7. **Mappings cascade.** `mappings.account_id` cascades on delete, so "a mapping to a removed account" (§12.1) can only happen as a race: the mapping was read, then the account was removed before the launch locked it. The launch then warns and runs plain `claude`, which is §12.1's row. Cost if wrong: none.
8. **`run`'s command line** is `tagteam run [ACCOUNT] [--provider P] [--require-session] [-- <agent args>...]`, with clap's `last = true` for the agent arguments. `--json` changes only pre-launch errors (§12.1). `main_with_args`'s pre-scan for `--json` stops at `--`.
9. **The session environment is provider-owned** (§4.5 `session_env`). It names the variables to set (CC: `CLAUDE_CONFIG_DIR` to the spelling) and to scrub (CC: §12.5's list). The engine applies it identically to the validation spawn and to `claude`. The plain-`claude` paths restore the outer home only (M4a's effective `Env`) and scrub nothing (§12.5).
10. **Validation is a provider method** that spawns through `run_captured`, so it is cancellable (§12.5 "the wait for the login check"). FakeAgent validates without spawning, by reading its profile files. CC's tests script the spawn through an injected `ProcessSpawner` port, so engine tests never run a binary.
11. **One fake `claude`** for CLI tests: Task 3's generated `/bin/sh` script, put first on `PATH`. Its `FAKE_CLAUDE_*` variables, documented once on `fake_claude`, steer it:
    - the exit code, and `auth status`'s reply: verbatim, as a given login, or read from the config home as Claude Code would (an `apiKeyHelper`, the `oauthAccount`);
    - a sleep that a trapped signal ends, or a hold file the session waits for;
    - recording each SIGHUP, SIGINT and SIGTERM, then ending gracefully or running on;
    - writing a session record of a given kind;
    - rotating its profile credential file.

    Every run records its pid, parent, directory, arguments, environment, signals and exit in `FAKE_CLAUDE_OUT` (`fake_claude_calls`). Tasks 12 and 13 run against it and add no variable. No test runs the real `claude`. Cost if wrong: a richer fake, still one.
12. **Exit handling honours the cancel token** (§12.5 "After `claude` exits"), with lock waits as its cancellation points. A cancelled or failed exit handling prints one stderr notice and keeps the child's exit code.
13. **`run` inside a run shell launches from the outer home** (§12.1, M4a's effective `Env`), and its plain-`claude` path restores the outer home's variables. `run` is never refused inside a run shell; only account-changing commands are (B.32).
14. **A target that changed under the launch's locks is planned once more.** `launch` re-derives its decision under `MutationGuard` and the account lock (§12.5 step 1, B.47). When the target was removed (its mapping going with it, Decision 7) or became the live login, it returns the new `EngineError::TargetChanged { why }` (kind `target-changed`). A removal that a read of the account before the locks finds first (the gate refresh's vault read) is `TargetChanged` too, with the same `why` (Task 10). `launch` cannot answer "run plain" itself: it knows neither `--require-session` nor the plain spec. The CLI prints `warning: {why}`, calls `plan_run` once more with the same request, and acts on that plan. A second `TargetChanged` is the command's error, so it never loops. Cost if wrong: a launch that races two account changes in a row refuses, and the user runs it again.
15. **`PATH` is captured at the process boundary.** `PATH` joins the variables `Context::from_process` captures into `Env.vars` (M4a's Decision 5): one line in `app::session_vars` (Task 8). `plan_run` looks the launch command up on `env.var("PATH")`, the `PATH` that `exec` and the spawn inherit. Engine tests set it with `Fx::with_path`, so they never find the machine's `claude` (§15.1). Rejected: a `RunRequest.path` field, which would widen a contract type. Cost if wrong: one more name in M4a's capture list.
16. **FakeAgent seeds and merges back the flat `prefs` key of its private `identity.json`**, not `prefs.json`. M4a's share policy links `prefs.json` into every profile, so a profile's `prefs.json` is the outer file itself: a seed through it would write the outer home, and a merge-back would diff the file against itself. Task 13's run surface declares `(identity.json, ["prefs"])` for FakeAgent. Cost if wrong: one more way FakeAgent's shape differs from Claude Code's, which is its purpose.
17. **Exit handling leaves its own reservation out.** §12.5 captures at exit "if the profile is quiescent apart from this process's own reservation", but M4a's `session_state` and `apply_provenance` count every held reservation. Task 11 adds the crate-private `session_state_apart_from(p, row, own: &Path)` and `apply_provenance_apart_from(p, row, lock, own: &Path)`. M4a's public signatures do not change: each pair shares one private body. `own` is matched by file name inside `.tagteam-launch/`. Rejected: unlinking first, which contradicts "unlink last", and dropping the flock first, which would drop a descendant's hold too. Cost if wrong: two crate-private wrappers to remove.
18. **§12.5's environment warnings are the engine's.** `launch` computes them once into `Launched.warnings` (`launch::environment_warnings`, Task 10): one per scrubbed variable this process has set, and one when the outer home sets the provider's home variable to another directory than the profile. The CLI prints them as `warning: …` and computes none. Which scrubbed variables are set is read from `Env.vars`: `Context::from_process` records each one set at the boundary (`app::scrubbed_vars`, Task 12), by presence only. An engine test therefore sees only what it puts there (§15.1), and no login value is copied into `Env`, which derives `Debug`. Cost if wrong: a scrubbed name that is not UTF-8 is still scrubbed but never warned about.
19. **`shell-init` refuses under an unreadable run-shell marker**, as every command but `statusline` does (§12.8). Its fast path comes after M4a's unreadable-marker check and before the engine is built (Task 2). Cost if wrong: under a corrupt marker the wrapper cannot be printed until the marker is repaired, which the refusal names.
20. **The login check runs the launch command `plan_run` resolved.** `validate_profile` takes `program: &Path` and spawns it, never `claude` by name, so the check and the session run one binary. `RunPlan::Session.launch` therefore travels with the launch: the CLI passes it to `Engine::launch` (whose bootstrap validates, so `bootstrap_profile` takes it too), to `Engine::check_login`, and to `run_session`'s spawn. Cost if wrong: one more parameter on three calls.
21. **Rulings taken inside the tasks**, one line each; the task named records the reasoning:
    - `set_mapping` refuses an account of another provider than the mapping's, as `no-such-account` (Task 1).
    - `nearest_mapping` matches whole path components, and passes over a component that is not UTF-8 (Task 1).
    - `unmap` of a directory that is gone takes the path as given, made absolute (Task 1).
    - zsh and bash get one POSIX wrapper, which `sh` runs too (Task 2).
    - `run_captured` never kills a group after reaping its leader; output held open past the 2 s grace is `TimedOut` (Task 3).
    - `remove_dead_reservations` also deletes the temporary files of a dead create, and does not report them (Task 4).
    - A `projects` or `mcpServers` the profile does not hold as an object counts as unchanged (Task 5).
    - A default value that is not an object takes no change: each change under it is a conflict (Task 5).
    - A `claude.ai` reply with no email is `unknown`, never `invalid` (Task 7).
    - A recorded `""` `CLAUDE_CONFIG_DIR` is removed on the plain path, never exported (Task 8).
    - A stale-marked profile's credential, or another login's, is displaced at bootstrap; an older generation of this account is overwritten without a copy (Task 9).
    - A profile whose canonical path is not UTF-8 is refused at bootstrap (Task 9).
    - The launch waits 30 s for `MutationGuard` (`guard_or_refuse_within`), as §9.1 gives `run`'s bootstrap; the gate refresh before it runs only when the vault's token is due (Task 10).
    - An `invalid` per-launch check marks the seed's `needsBootstrap` without a lock, under its own reservation (Task 11).
    - `run`'s own end, `Ended::Child`, prints no late-signal notice; `run` checks the Keychain lock only on the session path (Task 12).
    - The new `real_keychain` feature in `tagteam-engine` gates Task 13's real-keychain test only (Task 13).
22. **A profile's files are found by its actual directory, its Keychain items by its recorded spelling.** The rule is M4a's (its Decision 19), applied consistently here. M4a's `read_profile_credential(env, dir, spelling)` and `profile_identity(env, dir)` follow it, through its `session::profile_paths(env, dir)` and FakeAgent's `profile_env_in(env, dir)`. The actual directory is where the profile is today (`profile_path`, whose canonical path is the current spelling). The recorded spelling is the marker's `configDir`. The two differ once the data directory has moved, which §12.2 supports: the marker still names the old path, and the profile's hashed items are still named from it, until the next bootstrap deletes the old item and records the new spelling. So:
    - `seed_profile`, `has_baseline` and `merge_back` take the actual directory, `dir: &Path`, never a spelling (Task 5). The old path holds nothing, so a baseline looked for there is never found, and a spelling-change bootstrap would seed over a killed session's unmerged changes.
    - Bootstrap's step 2 reads the credential as the profile holds it, with M4a's `read_profile_credential(env, profile, recorded)` (Task 9): the item named from the recorded spelling first, then `.credentials.json` in the profile's directory. Its identity checks (`bootstrap_trigger`, `displace_held`) read `profile_identity(env, profile)`, and so does exit handling's provenance (Task 11).
    - `write_credential_file` keeps the composed machine-shared keys of an entry absent at its last read and under the lock (Decision 6). On macOS the old spelling's item can be the only copy of the credential and its MCP tokens: Claude Code migrated the file into it. The write under the current spelling finds no item and no file there.

    Cost if wrong (if the two never differed): three run methods that take a directory where a spelling would do.

## Global Constraints

Copied from the spec, M4a's constraints included:
- Platforms are macOS and Linux. `unsafe` code goes only in `tagteam-provider` and `tagteam`; `tagteam-engine` and `tagteam-core` forbid it.
- **Lock order (§4.3).** The order is the mutation guard (30 s for the launch, §9.1), then account locks in ascending ID order, then provider locks.
  - The profile's own credential locks and storage-write lock come after the account lock.
  - The default home's config lock is taken alone for a merge-back (§4.3's standalone exception).
  - No lock is waited on while a later one is held.
  - A reservation is only ever tested non-blocking (B.37).
- **What `run` writes in the default home** is only §3's rows: `projects` and `mcpServers` of `~/.claude.json` by three-way merge on exit, and `projects/` and `history.jsonl` created empty only when absent.
- **What `run` writes in the profile** is only tagteam's `.tagteam-*` files, its links, `.credentials.json` at a bootstrap, and `.claude.json` at a seed.
- **The profile's Keychain item.** It is named from the recorded spelling. It is deleted and verified `Absent` at every bootstrap (§12.3 step 5), and never written.
- **Secret files.** Every file holding a secret is created 0600 at creation and never chmod'ed afterwards (§5, B.33).
- **The exit code** is the child's, `128 + signal` if it was killed by one, whatever exit handling did (§12.5, B.63). A launch refused after its reservation exists exits 1. An interrupted launch exits as §14.1 says.
- **Stable strings.**
  - Validation outcomes: `valid`, `invalid`, `overridden`, `drifted`, `unknown`, `unreachable`.
  - Error kinds (new): `launch-command-missing`, `api-key-account`, `requires-session`, `login-overridden`, `login-invalid`, `login-drifted`, `login-unknown`, `launch-unreachable`, `target-changed`.
  - Files: `.tagteam-launch/<pid>.lock` and `.tagteam-baseline.json`.

## Review Focus

1. **`tagteam` killed with SIGKILL while `claude` runs.** The reservation stays live through `claude`'s inherited fd. Nothing captures or bootstraps under the running session. After `claude` exits, the next launch, switch or gate captures lazily, and the next launch merges back the left-over baseline. (Task 13)
2. **Ctrl-C at the terminal while `claude` runs.** `claude` gets one SIGINT and tagteam ignores it. `kill <tagteam>` (SIGTERM) reaches `claude` once. A Ctrl-\ never kills tagteam. The exit code is `claude`'s. (Task 12)
3. **Two sessions of one account.** The second joins without a seed or a bootstrap. The first to exit leaves capture and merge-back to the last one out. The last one out captures a rotation either session made. (Task 13)
4. **`~/.claude.json` edited by default-home sessions while the profile session runs:** the same project key changed on both sides, a new project, a removed MCP server. The merge-back keeps the default's value where both changed, applies the profile's other changes, warns once in a summary, and leaves every other byte of the file identical. (Tasks 5, 11)
5. **A fresh machine with no `~/.claude/projects` and no `history.jsonl`, and a home whose `settings.json` adds an `apiKeyHelper` after the first run.** The first gets both created empty and shared. The second gets the next launch refused as `overridden`, with the profile kept. (Tasks 9, 11)

---

## File Structure

```
crates/tagteam-core/
  src/merge.rs                   NEW   MergeKey, MergeResult, three_way (Task 5)
  src/lib.rs                     MOD   pub mod merge (Task 5)
crates/tagteam-provider/
  src/cancel.rs                  MOD   Cancel::take (Task 3)
  src/process.rs                 MOD   SpawnSpec, Captured, run_captured, spawn_session, exec_command,
                                       find_on_path, exit_code, ProcessSpawner, SystemSpawner,
                                       ScriptedSpawner (Task 3)
  src/reservation.rs             NEW   LaunchReservation, remove_dead_reservations (Task 4)
  src/provider.rs                MOD   MergeReport, SessionEnv, Validity; the seven Provider run
                                       methods (Tasks 5–7)
  src/lib.rs                     MOD   the re-exports (Tasks 3–7)
crates/tagteam-cc/
  src/session.rs                 MOD   seed, has_baseline, merge_back (Task 5);
                                       CC_SCRUB, session_env, validity (Task 7)
  src/live.rs                    MOD   LiveStore::write_credential_file (Task 6)
  src/provider.rs                MOD   the run methods (Tasks 5–7)
  tests/session.rs               MOD   seed_and_merge_back, profile_credential, validation (Tasks 5–7)
  tests/fixtures/auth-status/claude-ai.json   NEW   Appendix A.7's fields (Task 7)
crates/tagteam-fake/
  src/provider.rs                MOD   the run methods, over identity.json's prefs (Tasks 5–7)
  tests/provider.rs              MOD   seed_and_merge_back, profile_credential, validation (Tasks 5–7)
crates/tagteam-engine/
  Cargo.toml                     MOD   feature real_keychain (Task 13)
  src/store/mod.rs               MOD   Mapping and the mappings API (Task 1)
  src/run.rs                     NEW   RunRequest, RunPlan, Engine::plan_run (Task 8)
  src/bootstrap.rs               NEW   Trigger, Engine::bootstrap_profile (Task 9); its dead_code
                                       allowances deleted (Task 10)
  src/launch.rs                  NEW   Launched, LaunchEnd, Engine::{launch, check_login,
                                       finish_run}, environment_warnings (Tasks 10, 11)
  src/engine.rs                  MOD   EngineConfig.spawner (Task 9); guard_or_refuse_within (Task 10)
  src/error.rs                   MOD   the new kinds (Tasks 8–10)
  src/lifecycle.rs               MOD   remove_profile becomes pub(crate) (Task 9)
  src/switch.rs                  MOD   four items become pub(crate) (Task 10)
  src/session.rs                 MOD   session_state_apart_from (Task 11)
  src/provenance.rs              MOD   apply_provenance_apart_from (Task 11)
  src/lib.rs                     MOD   pub mod run, bootstrap, launch (Tasks 8–10)
  src/testutil.rs                MOD   EngineConfig.spawner (Task 9)
  src/lazy_http.rs               MOD   EngineConfig.spawner (Task 9)
  tests/common/mod.rs            MOD   PATH and work-dir fixtures (Task 8); Fx.spawner and the
                                       login-check fixtures (Task 9); claude_bin (Task 10)
  tests/oracle.rs                MOD   EngineConfig.spawner (Task 9)
  tests/{mappings,run_plan,bootstrap,launch,run_invariants,run_real_keychain}.rs  NEW
crates/tagteam/
  src/cli.rs                     MOD   Map, Unmap (Task 1); ShellInit, ShellArg (Task 2); Run (Task 12)
  src/app.rs                     MOD   map and unmap (Task 1); shell-init's fast path (Task 2);
                                       PATH in session_vars (Task 8); EngineConfig.spawner (Task 9);
                                       run's own path, scrubbed_vars and the boundary's presence
                                       record (Task 12)
  src/render.rs                  MOD   mapping_json, mappings_json, mappings_human (Task 1)
  src/shell_init.rs              NEW   Wrapped, script (Task 2)
  src/run.rs                     NEW   run_session, abandon, exec_failed, forwarded, the pause
                                       points (Tasks 12, 13)
  src/signals.rs                 MOD   survive_quit (Task 12)
  src/statusline.rs              MOD   EngineConfig.spawner (Task 9)
  src/lib.rs                     MOD   mod shell_init (Task 2); mod run, the --json pre-scan (Task 12)
  tests/common/mod.rs            MOD   fake_claude, path_with, FakeCall, fake_claude_calls (Task 3)
  tests/map_cli.rs               NEW   (Tasks 1, 2)
  tests/run_cli.rs               NEW   (Tasks 3, 12, 13)
  tests/run_shell_cli.rs         MOD   COMMANDS gains map and unmap (Task 1)
docs/superpowers/plans/2026-10-01-tagteam-m4b-run.md   MOD   its Status line (Task 14)
```

---

## Interface Contract

Every task implements exactly these names and signatures. M4a's and M3a's contracts are binding where this plan uses their names. M4a's public signatures do not change: where M4b needs more of an M4a function, it adds a crate-private sibling (Task 11).

### `tagteam-core`

**`src/merge.rs`** (Task 5):

```rust
/// One key of §12.4's diff: `projects.<path>.<key>` or `mcpServers.<name>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MergeKey { Project { path: String, key: String }, McpServer { name: String } }

#[derive(Debug, Clone, PartialEq)]
pub struct MergeResult {
    /// The new `projects` and `mcpServers` values for the default file (`None`: leave the
    /// key as it is, because nothing changed under it).
    pub projects: Option<serde_json::Value>,
    pub mcp_servers: Option<serde_json::Value>,
    /// Keys the profile changed (or removed) that were applied.
    pub applied: Vec<MergeKey>,
    /// Keys both sides changed since the baseline: the default's value was kept (§12.4 step 3).
    pub conflicts: Vec<MergeKey>,
}

/// §12.4: the profile's changes since `baseline`, applied over `default`. Each argument is
/// the pair (`projects`, `mcpServers`) as JSON values, `Value::Null` when absent.
pub fn three_way(
    baseline: (&serde_json::Value, &serde_json::Value),
    profile: (&serde_json::Value, &serde_json::Value),
    default: (&serde_json::Value, &serde_json::Value),
) -> MergeResult;
```

### `tagteam-provider`

**`src/cancel.rs`** (Task 3): `impl Cancel { /// Consumes the recorded signal (§12.5's forwarding). pub fn take(&self) -> Option<i32>; }`.

**`src/process.rs`** (Task 3), every name re-exported at the crate root:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpawnSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub set: Vec<(OsString, OsString)>,
    pub remove: Vec<OsString>,
    pub cwd: Option<PathBuf>,
}

/// Non-interactive: its own process group, stdin null, stdout and stderr captured, the group
/// killed (`killpg`) on `timeout` or when `cancel` is set (polled every 10 ms).
pub fn run_captured(spec: &SpawnSpec, timeout: Duration, cancel: &Cancel) -> Captured;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    Exited { code: Option<i32>, signal: Option<i32>, stdout: Vec<u8>, stderr: Vec<u8> },
    TimedOut,
    Interrupted(i32),
    SpawnFailed(String),
}

/// The session: tagteam's foreground process group, stdio inherited, and exactly `inherit`
/// passed without `FD_CLOEXEC` (cleared in `pre_exec`, Decision 3).
pub fn spawn_session(spec: &SpawnSpec, inherit: Option<RawFd>) -> io::Result<std::process::Child>;
/// The plain path: `exec`; returns only the error.
pub fn exec_command(spec: &SpawnSpec) -> io::Error;
/// `PATH` lookup of `name`: the first executable regular file, as a shell finds it.
pub fn find_on_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf>;
/// §12.5: the child's code, `128 + signal` when a signal ended it.
pub fn exit_code(status: std::process::ExitStatus) -> i32;

/// The port engine tests script (Decision 10). `SystemSpawner` calls `run_captured`.
pub trait ProcessSpawner: Send + Sync {
    fn run_captured(&self, spec: &SpawnSpec, timeout: Duration, cancel: &Cancel) -> Captured;
}
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemSpawner;
/// Tests: replies in order, recording each spec.
#[derive(Default)] pub struct ScriptedSpawner { /* Mutex<VecDeque<Captured>>, Mutex<Vec<SpawnSpec>> */ }
impl ScriptedSpawner { pub fn new() -> Self; pub fn push(&self, c: Captured); pub fn specs(&self) -> Vec<SpawnSpec>; }
```

**`src/reservation.rs`** (Task 4), re-exported at the crate root:

```rust
/// §12.5's launch reservation: `<profile>/.tagteam-launch/<pid>.lock`, created under a
/// temporary name, `flock`ed, then renamed into place, so it never appears unlocked.
#[derive(Debug)]
pub struct LaunchReservation { /* File (CLOEXEC in tagteam), PathBuf */ }
impl LaunchReservation {
    /// Holds `MutationGuard` and the account lock: the caller's duty, not checked here. When a
    /// live `<pid>.lock` already holds the name (an orphaned `claude` of an earlier process with
    /// this pid), it fails with `io::ErrorKind::AlreadyExists`, naming the file, and leaves it.
    pub fn create(profile: &Path) -> io::Result<LaunchReservation>;
    pub fn path(&self) -> &Path;
    pub fn fd(&self) -> RawFd;
    /// Unlinks the file (exit handling's last step). The lock goes when the last holder exits.
    /// Dropping a reservation never unlinks it.
    pub fn unlink(self) -> io::Result<()>;
}
/// Removes every reservation in `profile` that `probe_lock` finds `Free` (dead). Returns their
/// paths. Under `MutationGuard` and the account lock (§12.5).
pub fn remove_dead_reservations(profile: &Path) -> io::Result<Vec<PathBuf>>;
```

**`src/provider.rs`** (Tasks 5–7), new types and `Provider` methods:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionEnv { pub set: Vec<(OsString, OsString)>, pub remove: Vec<OsString> }

/// §12.3 step 8's outcomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Validity {
    Valid,
    Invalid(String),
    Overridden { method: String, source: Option<String> },
    Drifted { reported: String },
    Unknown(String),
    Unreachable(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MergeReport { pub applied: usize, pub conflicts: Vec<String> }

pub trait Provider: Send + Sync {
    // …existing and M4a methods…
    /// §12.5 "Environment": what the session's environment sets and scrubs, for `spelling`.
    fn session_env(&self, spelling: &str) -> SessionEnv;
    // Seed and merge-back find the profile's files by `dir`, its actual directory, never by its
    // recorded spelling, which names the old path once the data directory has moved (Decision 22).
    /// §12.4: seeds the profile's config in `dir` from the outer home (`env`), for `identity`;
    /// writes the baseline. Takes the profile's own config lock.
    fn seed_profile(&self, env: &Env, dir: &Path, identity: &Identity) -> Result<(), ProviderError>;
    /// §12.4: whether a baseline is waiting in `dir` (a merge-back that never ran).
    fn has_baseline(&self, dir: &Path) -> bool;
    /// §12.4: merges the changes of the profile in `dir` back into the outer home's config
    /// under its config lock, alone; removes the baseline on success. Fails without touching
    /// the baseline.
    fn merge_back(&self, env: &Env, dir: &Path, cancel: &Cancel) -> Result<MergeReport, ProviderError>;
    /// §12.3 step 4: writes the profile's credential file, composed, under the profile's own
    /// credential locks and storage-write lock (Decision 6). Never writes a Keychain item.
    fn write_profile_credential(&self, env: &Env, spelling: &str, guard: &MutationGuard, bytes: &[u8]) -> Result<(), ProviderError>;
    /// §12.3 step 4's composition: account-scoped keys of `vault`, machine-shared keys of
    /// `profile` (none for a new profile).
    fn compose_profile_credential(&self, vault: &[u8], profile: Option<&[u8]>) -> Result<Vec<u8>, ProviderError>;
    /// §12.3 step 8 / "Every launch is checked", run with `spawner` in the session's exact
    /// environment and `cwd`, 10 s timeout, cancellable. It spawns `program`, the launch command
    /// `plan_run` resolved, never the launch command by name (Decision 20). An interrupted check
    /// is `Unknown("interrupted")`; the caller maps it by reading the token.
    #[allow(clippy::too_many_arguments)]
    fn validate_profile(&self, env: &Env, spelling: &str, cwd: &Path, program: &Path, expect: &Identity, spawner: &dyn ProcessSpawner, cancel: &Cancel) -> Validity;
}
```

### `tagteam-cc`

**`src/live.rs`** (Task 6): `impl LiveStore { pub fn write_credential_file(&self, env: &Env, paths: &CcPaths, bytes: &[u8], fence: Fence<'_>) -> Result<(), ProviderError>; }`: the storage-write lock, a re-read under it (M3a's rule: on macOS it reads the profile's hashed item, as Claude Code reads it first), then `<secure-storage dir>/.credentials.json` at 0600 (atomic). The machine-shared keys written are the re-read's when it finds the entry. An entry absent there and at the operation's last read keeps the composed ones (Decisions 6 and 22). It never writes or deletes a Keychain item.

**`src/session.rs`** (Tasks 5, 7):
- **M4a's two views of a profile** (its Decision 19; Decision 22 here): `profile_paths(env, dir)` names its files by `dir`, its actual directory, and `profile_env(env, spelling)` names its Keychain items by the recorded spelling. Both are crate-private, and M4b adds neither.
- **`CC_SCRUB`:** exactly §12.5's list.
  - Prefixes are expanded at runtime against the process environment: `CLAUDE_CODE_*_FILE_DESCRIPTOR`.
  - Literal names: `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDE_CODE_OAUTH_REFRESH_TOKEN`, `CLAUDE_CODE_OAUTH_SCOPES`, `CLAUDE_CODE_OAUTH_CLIENT_ID`, `CLAUDE_CODE_ACCOUNT_UUID`, `CLAUDE_CODE_USER_EMAIL`, `CLAUDE_CODE_ORGANIZATION_UUID`, `ANTHROPIC_PROFILE`, `ANTHROPIC_CONFIG_DIR`, `ANTHROPIC_FEDERATION_RULE_ID`, `ANTHROPIC_IDENTITY_TOKEN`, `ANTHROPIC_IDENTITY_TOKEN_FILE`, `CLAUDE_CODE_CUSTOM_OAUTH_URL`, `USE_LOCAL_OAUTH`, `USE_STAGING_OAUTH`, `CLAUDE_SECURESTORAGE_CONFIG_DIR`.
- **`BASELINE_FILE`:** `.tagteam-baseline.json`, holding `{"format": "tagteam-baseline", "version": 1, "projects", "mcpServers"}`.
- **The seed (§12.4)** copies `projects` and top-level `mcpServers` from the outer home's global config, sets `oauthAccount` from `identity.raw`, sets `hasCompletedOnboarding: true`, and sets `theme` if absent (from the outer file, else `"dark"`). It starts from the profile's current file, or `{}`, and writes it with the splice, under the profile's config lock. The seed and the merge-back resolve the profile's files with M4a's `profile_paths(env, dir)`.
- **Validation** parses `claude auth status --json` per §12.3's table, with Appendix A.7's fields.

### `tagteam-fake`

**`src/provider.rs`** (Tasks 5–7), FakeAgent's shapes of the same methods:
- **Seed and merge-back** (Decision 16) work on the flat `prefs` key of FakeAgent's private `identity.json`, never on `prefs.json`. The baseline is `.tagteam-baseline.json`, holding `{"format": "tagteam-baseline", "version": 1, "prefs"}`. Conflicts are named `prefs["<name>"]`. Both resolve the profile through M4a's private `profile_env_in(env, dir)` (Decision 22).
- **The session environment** sets `FAKEAGENT_HOME` and scrubs `FAKEAGENT_TOKEN`. **Validation** reads the profile's files and never spawns.

### `tagteam-engine`

**`src/store/mod.rs`** (Task 1):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping { pub path: String, pub provider: ProviderId, pub account_id: AccountId, pub added_at: i64 }
impl Store {
    pub fn set_mapping(&self, path: &str, provider: &ProviderId, account: &AccountId, at: i64) -> Result<(), StoreError>;
    /// Removes `path`'s mapping for `provider`, or for every provider when `None`. Returns the count.
    pub fn remove_mappings(&self, path: &str, provider: Option<&ProviderId>) -> Result<usize, StoreError>;
    pub fn mappings(&self) -> Result<Vec<Mapping>, StoreError>;
    /// §12.7: the mapping of `dir` or its nearest mapped ancestor, for `provider` (`dir` canonical).
    pub fn nearest_mapping(&self, dir: &Path, provider: &ProviderId) -> Result<Option<Mapping>, StoreError>;
}
```

**`src/run.rs`** (Task 8):

```rust
#[derive(Debug, Clone)]
/// `account` is resolved by the CLI (§10.4, with its ambiguity prompt) before planning;
/// `None` means the mapping decides (§12.1). `provider` is the global `--provider`.
pub struct RunRequest { pub account: Option<AccountId>, pub provider: Option<ProviderId>, pub require_session: bool, pub cwd: PathBuf, pub args: Vec<OsString> }
#[derive(Debug, Clone)]
pub enum RunPlan {
    /// §12.1's plain `claude`: exec `spec` (outer home restored, nothing scrubbed); `warning`
    /// for a mapping whose account went away (Decision 7).
    Plain { spec: SpawnSpec, warning: Option<String> },
    /// A session for `account`; `launch` is the launch command found on `PATH`.
    Session { account: AccountRow, provider: ProviderId, launch: PathBuf },
}
impl Engine {
    /// The launch command is looked up on `self.env.var("PATH")` (Decision 15), before
    /// anything else.
    pub fn plan_run(&self, req: &RunRequest) -> Result<RunPlan, EngineError>;
}
```

**`src/bootstrap.rs`** (Task 9): `impl Engine { #[allow(clippy::too_many_arguments)] pub(crate) fn bootstrap_profile(&self, p: &dyn Provider, row: &AccountRow, profile: &Path, cwd: &Path, program: &Path, guard: &MutationGuard, lock: &AccountLock) -> Result<(), EngineError>; }`. This covers §12.3 steps 2–8; step 1's gate refresh runs before the locks, in `launch`. Step 2 reads with M4a's `read_profile_credential(env, profile, recorded)` (Decision 22). It writes the marker's `configDir` after the old item is deleted (M4a). It also seeds `profile`, then validates in `cwd`, spawning `program` (Decision 20); `invalid` deletes the profile.

**`src/launch.rs`** (Tasks 10, 11):

```rust
pub struct Launched {
    pub account: AccountRow,
    pub profile: PathBuf,
    pub spelling: String,
    pub reservation: LaunchReservation,
    pub env: SessionEnv,
    /// This launch bootstrapped, so its login check already ran (§12.3).
    pub bootstrapped: bool,
    /// Every warning to print, §12.5's environment warnings last (Decision 18).
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchEnd {
    /// `claude` ran and exited with this status code.
    Exited(i32),
    /// The launch was refused after its reservation existed (exit 1).
    Refused,
}
impl Engine {
    /// §12.5 steps 1–5 (the gate refresh of step 1 included). `program` is the launch command
    /// `plan_run` resolved, which a bootstrap's validation spawns (Decision 20). Under the
    /// locks, a target that was removed or became the live login is `TargetChanged`
    /// (Decision 14), as is one whose removal a read before the locks finds. A live
    /// `<pid>.lock` already there (`create`'s `AlreadyExists`) refuses as `LaunchUnreachable`,
    /// naming the file.
    pub fn launch(&self, account: &AccountRow, program: &Path, cwd: &Path) -> Result<Launched, EngineError>;
    /// §12.3 "Every launch is checked": after the locks are released, before the spawn,
    /// spawning `program`. A launch that bootstrapped skips it. When the token was set during
    /// the check it returns `Err(EngineError::Interrupted(n))`, read through
    /// `cancel.requested()`, never through the `Validity` text.
    pub fn check_login(&self, launched: &Launched, program: &Path, cwd: &Path) -> Result<(), EngineError>;
    /// §12.5 "When the child exits": capture and merge-back when last out, then the unlink.
    /// Returns the notices to print; never changes the exit code (B.63).
    pub fn finish_run(&self, launched: Launched, end: LaunchEnd) -> Vec<String>;
}

// Crate-private (Task 11, Decision 17), for exit handling: "quiescent apart from this
// process's own reservation". Defined beside M4a's `session_state` (`session.rs`) and
// `apply_provenance` (`provenance.rs`), which keep their signatures and share their bodies.
impl Engine {
    pub(crate) fn session_state_apart_from(&self, p: &dyn Provider, row: &AccountRow, own: &Path) -> Result<SessionState, EngineError>;
    pub(crate) fn apply_provenance_apart_from(&self, p: &dyn Provider, row: &AccountRow, lock: &AccountLock, own: &Path) -> Result<ProfileCheck, EngineError>;
}

// Crate-private (Task 10, Decision 18).
pub(crate) fn environment_warnings(session: &SessionEnv, is_set: &dyn Fn(&OsStr) -> bool, home: Option<(&str, &Path)>) -> Vec<String>;
```

`EngineConfig` gains `spawner: Arc<dyn ProcessSpawner>` in Task 9, its first user, at every `EngineConfig` site M4a's Tasks 8 and 9 already list (the engine test module, `testutil.rs`, `lazy_http.rs`, `tests/common/mod.rs`, `tests/oracle.rs`, `app.rs` and `statusline.rs`): `SystemSpawner` in production and `ScriptedSpawner` in tests.

**`src/error.rs`**, new variants and their kinds:
- `LaunchCommandMissing { command: String }` (`launch-command-missing`)
- `ApiKeyAccount { position: u32 }` (`api-key-account`)
- `RequiresSession { why: String }` (`requires-session`)
- `LoginOverridden { position: u32, method: String, key_source: Option<String> }` (`login-overridden`; thiserror reserves a field named `source`)
- `LoginInvalid { position: u32, detail: String }` (`login-invalid`)
- `LoginDrifted { position: u32, reported: String }` (`login-drifted`)
- `LoginUnknown { position: u32, detail: String }` (`login-unknown`)
- `LaunchUnreachable { detail: String }` (`launch-unreachable`)
- `TargetChanged { why: String }` (`target-changed`, Decision 14): returned by `launch` when its re-check under the locks finds the target removed (its mapping gone with it, Decision 7) or now the live login, and when a read of the account before the locks fails because it was removed. The CLI's answer is a second `plan_run`, never a loop: on a second `TargetChanged` it refuses with that error.

### `tagteam` (CLI)

```rust
// cli.rs
Run {
    account: Option<String>,
    #[arg(long)] require_session: bool,
    /// Arguments for the agent, after `--`.
    #[arg(last = true)] args: Vec<OsString>,
},
/// Map a directory to an account (§12.7); with no arguments, list mappings.
Map { account: Option<String>, path: Option<PathBuf> },
Unmap { path: Option<PathBuf> },
ShellInit { shell: ShellArg },          // zsh | bash | fish
// run.rs: `Ok` is the child's code; `Err` is a launch that never started, whose exit handling
// has already run, so the `--json` envelope reports it (§12.1).
pub(crate) fn run_session(engine: &Engine, launched: Launched, launch: &Path, args: &[OsString], cwd: &Path, cancel: &Cancel, err: &mut dyn Write) -> Result<i32, EngineError>;
// app.rs (Task 12, Decision 18): the names whose presence `Context::from_process` records.
pub(crate) fn scrubbed_vars(registry: &ProviderRegistry) -> Vec<String>;
```

- `Context::from_process` also captures `PATH` into `Env.vars` (`app::session_vars` returns it after M4a's names, Decision 15), and records each `scrubbed_vars` name that is set there as `""`, its presence only (Decision 18).
- `shell-init` answers after M4a's unreadable-marker refusal and before the engine is built (Decision 19).

### Test fixtures

- **`crates/tagteam-engine/tests/common/mod.rs`:** `Fx.spawner: Arc<ScriptedSpawner>` (Task 9); `pub fn claude_bin() -> &'static Path` (Task 10), the launch command engine tests pass to `launch` and `check_login`.
- **`crates/tagteam/tests/common/mod.rs`** (Task 3): `pub fn fake_claude(root: &Path) -> PathBuf` (writes `<root>/bin/claude`, returns `<root>/bin`), `pub fn path_with(bin: &Path) -> OsString`, `pub struct FakeCall { pid, mode, ppid, cwd, args: Vec<OsString>, env, ready, signals, exit }` and `pub fn fake_claude_calls(out: &Path) -> Vec<FakeCall>`. The `FAKE_CLAUDE_*` variables are documented once, on `fake_claude` (Decision 11).

---

## Tasks

| # | Task | Depends on |
|---|---|---|
| 1 | Mappings: store API, `map` and `unmap` | — |
| 2 | `shell-init` | 1 |
| 3 | Process primitives, `Cancel::take`, the fake `claude` | — |
| 4 | Launch reservations | 3 |
| 5 | Three-way merge; seed and merge-back (CC, FakeAgent) | — |
| 6 | The profile credential write and its composition | 5 |
| 7 | Session environment and validation | 3, 5, 6 |
| 8 | The launch decision (`plan_run`) | 1, 3 |
| 9 | Bootstrap and validation (§12.3) | 3, 5, 6, 7, 8 |
| 10 | Launch under the locks (§12.5 steps 1–5) | 3, 4, 5, 7, 8, 9 |
| 11 | The per-launch login check and exit handling | 3, 4, 5, 7, 9, 10 |
| 12 | The `run` command: exec, spawn, signals, exit code | 1, 2, 3, 7, 8, 10, 11 |
| 13 | Run invariants, races and the kill paths | 3, 5, 9, 10, 11, 12 |
| 14 | Final verification and live acceptance | all |

---

### Task 1: Mappings: store API, `map` and `unmap`

§12.7: "`tagteam map [ACCOUNT] [PATH]` maps a directory for the account's provider. `PATH`
defaults to the current directory and is stored as its canonical absolute path. With no
arguments, `map` lists the mappings. A directory can hold one mapping per provider." and
"Subdirectories inherit the nearest mapped ancestor, per provider." The `mappings` table has
existed since schema v1 (`store/schema.sql:104-110`, `PRIMARY KEY (path, provider)`,
`account_id … ON DELETE CASCADE`), but nothing reads or writes it. This task adds the store API
that Task 8's launch decision reads (`nearest_mapping`), and the two commands that write it.

**Readings of the spec this task commits to:**
- **"Canonical absolute path" is `fs::canonicalize`:** `realpath`, so absolute, symlinks
  resolved, no `.` or `..`, no trailing slash. It is not NFC-normalized. NFC is §12.2's rule for
  the spelling exported as `CLAUDE_CONFIG_DIR`, not for mappings, and Task 8 canonicalizes the
  working directory the same way, so the two always meet.
- **`PATH` must be an existing directory.** A missing path or a file is refused as
  `invalid-input`, naming the path. Mappings are stored as text, so a canonical path that is not
  UTF-8 is refused too, with no lossy spelling.
- **Nearest ancestor by components.** `nearest_mapping` walks `dir.ancestors()`, `dir` first,
  and looks each one up exactly. `/a/bc` never inherits `/a/b`, as a string prefix would. `dir`
  is first rebuilt from its components, so a trailing `/` or an inner `.` still matches. A
  component that is not UTF-8 can match no stored path; the walk passes it and goes on to its
  parent (a ruling, Decision 21). A mapping of `/` covers everything.
- **The account's own provider** (§12.7). `--provider`, when given, narrows the account
  reference as for every command (§13.1 rule 1). The store refuses a mapping whose account is
  not of the provider named, as `NoSuchAccount`, so a lookup never returns another provider's
  account. The account is found and the row written in one `INSERT … SELECT`, so a concurrent
  `remove` either cascades the mapping or makes the map fail as `no-such-account`.
- **One mapping per provider per path:** mapping a mapped path again replaces the account and
  `added_at` (an upsert on the primary key).
- **`unmap [PATH]`** removes `PATH`'s mappings, every provider's or only `--provider`'s (the
  global flag; the contract's `Unmap` has no flag of its own). Removing nothing is not an error:
  it says so and exits 0. A directory that no longer exists cannot be canonicalized, so `unmap`
  then takes `PATH` as given, made absolute (`std::path::absolute`): the canonical path `map`
  listed unmaps verbatim.
- **No lock.** Each command is one SQLite statement. Mappings are neither accounts nor the live
  login, so `map` and `unmap` run inside a run shell too (B.32), against the outer home's store
  (M4a's effective `Env`). They write no event: §6.1 names none for mappings, and nothing
  would read one.
- **`map` with no arguments** lists every mapping in path order, `--provider`'s alone when it is
  given. Like `list` on a fresh machine it never creates the store (§5), and neither does
  `unmap`.
- **The JSON shapes** are this task's (§13.2 gives none): `map` prints
  `{schemaVersion, ok, mapping: {path, provider, number, id, email, alias?, addedAt}}`, the list
  `{schemaVersion, mappings: [...]}`, and `unmap` `{schemaVersion, ok, path, removed}`.
  `addedAt` is ISO 8601 UTC, as `history`'s times are. Both are documented in `--help`, as
  `history`'s shape is (§13.2). The text list is `<path>  <position>  <name>`, with the provider
  after the path only when the mappings span more than one provider (§13.1).
- **`remove` already cascades** (§6.1, §10.3: the row goes last and cascades to the mappings).
  Tests pin it at the store, the engine and the CLI.

**Files:**
- Modify: `crates/tagteam-engine/src/store/mod.rs` (`use std::path` line 4; `Mapping`,
  `MAPPING_COLUMNS` and `mapping_from_row` between `DisplacedRow` (171–178) and `Store`
  (180–182); four methods at the end of `impl Store`, after `insert_displaced` (971–977))
- Create: `crates/tagteam-engine/tests/mappings.rs`
- Modify: `crates/tagteam/src/cli.rs` (`use` line 1; `Command::Map` and `Command::Unmap` before
  `Statusline`, line 100)
- Modify: `crates/tagteam/src/app.rs` (imports, lines 1–28; two `dispatch` arms beside
  `Command::Statusline`'s (686); `App::{list_mappings, map, unmap}` after `history`, before
  `live_row` (871); free `canonical_dir`, `unmapped_path`, `stored_path`, `invalid_path` before
  `mod tests` (885); M3a's `command_name` and its test `the_late_notice_names_each_command_as_it_is_typed`)
- Modify: `crates/tagteam/src/render.rs` (`use` line 4; `NO_MAPPINGS` after `NO_ACCOUNTS`,
  line 12; `mapping_json`, `mappings_json`, `mappings_human` after `account_json` (620–627);
  two tests at the end of `mod tests`)
- Modify: `crates/tagteam/tests/run_shell_cli.rs` (M4a Task 8's `COMMANDS`)
- Create: `crates/tagteam/tests/map_cli.rs`

**Interfaces:**
- Consumes:
  - `Store::{lock, exec, delete_account}`, `StoreError::NoSuchAccount`, `rusqlite::{params, OptionalExtension, Row}` (existing, `store/mod.rs`)
  - `Engine::{store, existing_store, now_ms, remove}`; `App::{resolve, print}`; `render::{name, email}`; `tagteam_cc::usage::format_iso8601` (existing)
  - M3a: `app.rs`'s `command_name` (exhaustive over `Command`) and its test
    `the_late_notice_names_each_command_as_it_is_typed`, on `m3-auto-switch`; not on this branch
    yet. The edits below are anchored to M3a's code as its plan gives it.
  - M4a Task 8: `crates/tagteam/tests/run_shell_cli.rs`'s `COMMANDS`, and the refusal of every
    command but `statusline` under an unreadable marker, which now covers `map` and `unmap`.
  - M4a Task 14: `tests/common/mod.rs`'s `cc_profile(root: &Path, id: &str) -> (PathBuf, String)`.
  - The engine fixtures `common::{Fx, add, cc}` (existing).
- Produces (Interface Contract, `src/store/mod.rs`):
  - `Mapping { path: String, provider: ProviderId, account_id: AccountId, added_at: i64 }`
  - `Store::set_mapping(&self, path: &str, provider: &ProviderId, account: &AccountId, at: i64) -> Result<(), StoreError>`
  - `Store::remove_mappings(&self, path: &str, provider: Option<&ProviderId>) -> Result<usize, StoreError>`
  - `Store::mappings(&self) -> Result<Vec<Mapping>, StoreError>`
  - `Store::nearest_mapping(&self, dir: &Path, provider: &ProviderId) -> Result<Option<Mapping>, StoreError>`
  - CLI: `Command::Map { account: Option<String>, path: Option<PathBuf> }`, `Command::Unmap { path: Option<PathBuf> }`
  - Crate-private: `render::{mapping_json, mappings_json, mappings_human}`

**Spec:**
- §12.7: `map [ACCOUNT] [PATH]` maps a directory, stored canonical, for the account's provider;
  no arguments lists; one mapping per provider per path; `unmap [PATH] [--provider P]` removes
  one provider's or all; subdirectories inherit the nearest mapped ancestor, per provider.
- §12.1: with no `ACCOUNT`, `run` uses the nearest mapped ancestor of the canonical working
  directory, for `--provider` or the default (Task 8 reads `nearest_mapping`).
- §6.1: the `mappings` table, its key and its cascade; §10.3: `remove` deletes the row last,
  which cascades to the mappings.
- §13.1: which provider a command acts on; §5: a command that changes nothing creates nothing.
- B.32: only commands that change accounts or the live login refuse in a run shell.

#### Cycle 1: the store API

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-engine/tests/mappings.rs`:

```rust
//! §12.7's mappings in the store: one per provider per path, found by the nearest mapped
//! ancestor, and removed with their account.

mod common;

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use common::{Fx, add, cc};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::store::{Mapping, Store, StoreError};

fn store() -> (tempfile::TempDir, Store) {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    (d, s)
}

fn other() -> ProviderId {
    ProviderId::new("fake-agent")
}

/// The account `nearest_mapping` finds for `dir`, if any.
fn nearest(s: &Store, dir: &str, provider: &ProviderId) -> Option<AccountId> {
    s.nearest_mapping(Path::new(dir), provider)
        .unwrap()
        .map(|m| m.account_id)
}

#[test]
fn a_directory_holds_one_mapping_per_provider_and_a_new_one_replaces_it() {
    let (_d, s) = store();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let f = add(&s, &other(), "f", "a@x.co", 1);
    s.set_mapping("/w/app", &cc(), &a, 10).unwrap();
    s.set_mapping("/w/app", &cc(), &b, 20).unwrap();
    s.set_mapping("/w/app", &other(), &f, 30).unwrap();
    assert_eq!(
        s.mappings().unwrap(),
        vec![
            Mapping {
                path: "/w/app".into(),
                provider: cc(),
                account_id: b,
                added_at: 20,
            },
            Mapping {
                path: "/w/app".into(),
                provider: other(),
                account_id: f,
                added_at: 30,
            },
        ]
    );
}

#[test]
fn the_nearest_mapped_ancestor_is_found_by_whole_path_components() {
    let (_d, s) = store();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    s.set_mapping("/w/a/b", &cc(), &a, 1).unwrap();
    s.set_mapping("/w/a/b/c/d", &cc(), &b, 1).unwrap();
    for (dir, found) in [
        ("/w/a/b", Some(&a)),
        ("/w/a/b/", Some(&a)),
        ("/w/a/b/./c", Some(&a)),
        ("/w/a/b/c", Some(&a)),
        ("/w/a/b/c/d", Some(&b)),
        ("/w/a/b/c/d/e/f", Some(&b)),
        ("/w/a/b/c/dd", Some(&a)),
        // Sibling prefixes: a string prefix would match all of these.
        ("/w/a/bc", None),
        ("/w/a/b c", None),
        ("/w/a/b-x/c", None),
        ("/w/a", None),
        ("/", None),
    ] {
        assert_eq!(nearest(&s, dir, &cc()).as_ref(), found, "{dir}");
    }
}

#[test]
fn a_mapping_of_the_root_covers_everything_below_it() {
    let (_d, s) = store();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    s.set_mapping("/", &cc(), &a, 1).unwrap();
    assert_eq!(nearest(&s, "/x/y/z", &cc()), Some(a.clone()));
    assert_eq!(nearest(&s, "/", &cc()), Some(a));
}

#[test]
fn nearest_mapping_is_per_provider() {
    let (_d, s) = store();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let f = add(&s, &other(), "f", "a@x.co", 1);
    s.set_mapping("/w", &cc(), &a, 1).unwrap();
    s.set_mapping("/w/deep", &other(), &f, 1).unwrap();
    assert_eq!(
        nearest(&s, "/w/deep/x", &cc()),
        Some(a),
        "claude-code's own mapping"
    );
    assert_eq!(nearest(&s, "/w/deep/x", &other()), Some(f));
    assert_eq!(nearest(&s, "/w/x", &other()), None);
    assert_eq!(nearest(&s, "/w/x", &ProviderId::new("none")), None);
}

#[test]
fn a_path_that_is_not_utf8_still_finds_its_mapped_ancestor() {
    let (_d, s) = store();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    s.set_mapping("/w", &cc(), &a, 1).unwrap();
    let dir = Path::new(OsStr::from_bytes(b"/w/caf\xe9/src"));
    assert_eq!(
        s.nearest_mapping(dir, &cc()).unwrap().map(|m| m.path),
        Some("/w".to_owned())
    );
}

#[test]
fn unmapping_removes_one_provider_s_mapping_or_every_one() {
    let (_d, s) = store();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let f = add(&s, &other(), "f", "a@x.co", 1);
    for p in ["/w", "/x"] {
        s.set_mapping(p, &cc(), &a, 1).unwrap();
        s.set_mapping(p, &other(), &f, 1).unwrap();
    }
    assert_eq!(s.remove_mappings("/w", Some(&cc())).unwrap(), 1);
    assert_eq!(nearest(&s, "/w", &cc()), None);
    assert_eq!(
        nearest(&s, "/w", &other()),
        Some(f.clone()),
        "the other provider's stays"
    );
    assert_eq!(s.remove_mappings("/x", None).unwrap(), 2);
    assert_eq!(
        s.remove_mappings("/x", None).unwrap(),
        0,
        "nothing left to remove"
    );
    assert_eq!(
        s.remove_mappings("/w/sub", None).unwrap(),
        0,
        "never an ancestor's"
    );
    let left: Vec<_> = s
        .mappings()
        .unwrap()
        .into_iter()
        .map(|m| (m.path, m.provider))
        .collect();
    assert_eq!(left, vec![("/w".to_owned(), other())]);
}

#[test]
fn a_mapping_names_an_account_of_its_own_provider() {
    let (_d, s) = store();
    let f = add(&s, &other(), "f", "a@x.co", 1);
    assert!(matches!(
        s.set_mapping("/w", &cc(), &AccountId::from_string("nobody"), 1),
        Err(StoreError::NoSuchAccount)
    ));
    assert!(
        matches!(
            s.set_mapping("/w", &cc(), &f, 1),
            Err(StoreError::NoSuchAccount)
        ),
        "fake-agent's account cannot hold claude-code's mapping"
    );
    assert!(s.mappings().unwrap().is_empty());
}

#[test]
fn deleting_an_account_deletes_its_mappings() {
    let (_d, s) = store();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    s.set_mapping("/w", &cc(), &a, 1).unwrap();
    s.set_mapping("/x", &cc(), &a, 1).unwrap();
    s.set_mapping("/y", &cc(), &b, 1).unwrap();
    s.delete_account(&a).unwrap();
    let left: Vec<String> = s.mappings().unwrap().into_iter().map(|m| m.path).collect();
    assert_eq!(left, ["/y"]);
}

#[test]
fn remove_unmaps_the_account_everywhere() {
    // §10.3: `remove` deletes the row last, and the row cascades to its mappings.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let store = fx.engine.store().unwrap();
    store.set_mapping("/w", &fx.provider(), &a, 1).unwrap();
    store.set_mapping("/x", &fx.provider(), &b, 1).unwrap();
    fx.engine.remove(&a).unwrap();
    let left: Vec<AccountId> = store
        .mappings()
        .unwrap()
        .into_iter()
        .map(|m| m.account_id)
        .collect();
    assert_eq!(left, vec![b]);
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test mappings`
Expected: compile errors only: E0432 for `Mapping` (no such item in `tagteam_engine::store`),
and E0599 for `set_mapping`, `remove_mappings`, `mappings` and `nearest_mapping` (no such method
on `Store`).

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/store/mod.rs`, line 4 becomes:

```rust
use std::path::{Path, PathBuf};
```

Between `DisplacedRow` (ends line 178) and `pub struct Store` (line 180), add:

```rust
/// §12.7: a directory mapped to an account, for one provider. `path` is stored as given; the
/// CLI gives the canonical path (§12.7). `added_at` is epoch ms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub path: String,
    pub provider: ProviderId,
    pub account_id: AccountId,
    pub added_at: i64,
}

const MAPPING_COLUMNS: &str = "path, provider, account_id, added_at";

fn mapping_from_row(r: &Row<'_>) -> rusqlite::Result<Mapping> {
    Ok(Mapping {
        path: r.get("path")?,
        provider: ProviderId::new(r.get::<_, String>("provider")?),
        account_id: AccountId::from_string(r.get::<_, String>("account_id")?),
        added_at: r.get("added_at")?,
    })
}
```

At the end of `impl Store`, after `insert_displaced` (971–977), add:

```rust
    /// §12.7: maps `path` to `account` for `provider`, replacing the mapping `path` held for
    /// that provider (one per provider per path). `account` must be an account of `provider`;
    /// otherwise nothing is written and the answer is `NoSuchAccount`.
    pub fn set_mapping(
        &self,
        path: &str,
        provider: &ProviderId,
        account: &AccountId,
        at: i64,
    ) -> Result<(), StoreError> {
        // An INSERT … SELECT finds the account and writes in one statement, so an account a
        // concurrent `remove` deletes is either mapped first, and the mapping cascades with it,
        // or not found. Its WHERE clause also keeps SQLite from reading `ON CONFLICT` as a join.
        let n = self.exec(
            "INSERT INTO mappings (path, provider, account_id, added_at) \
             SELECT ?1, provider, id, ?4 FROM accounts WHERE id = ?3 AND provider = ?2 \
             ON CONFLICT(path, provider) DO UPDATE SET account_id = excluded.account_id, \
             added_at = excluded.added_at",
            &[&path, &provider.as_str(), &account.as_str(), &at],
        )?;
        if n == 0 {
            Err(StoreError::NoSuchAccount)
        } else {
            Ok(())
        }
    }

    /// Removes `path`'s mapping for `provider`, or for every provider when `None`. Returns the
    /// count.
    pub fn remove_mappings(
        &self,
        path: &str,
        provider: Option<&ProviderId>,
    ) -> Result<usize, StoreError> {
        match provider {
            Some(p) => self.exec(
                "DELETE FROM mappings WHERE path = ?1 AND provider = ?2",
                &[&path, &p.as_str()],
            ),
            None => self.exec("DELETE FROM mappings WHERE path = ?1", &[&path]),
        }
    }

    /// Every mapping, by path, then provider.
    pub fn mappings(&self) -> Result<Vec<Mapping>, StoreError> {
        let c = self.lock();
        let mut stmt = c.prepare(&format!(
            "SELECT {MAPPING_COLUMNS} FROM mappings ORDER BY path, provider"
        ))?;
        let rows = stmt
            .query_map([], mapping_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// §12.7: the mapping of `dir` or its nearest mapped ancestor, for `provider` (`dir`
    /// canonical). Ancestors are whole path components, `dir` itself first: `/a/bc` never
    /// inherits `/a/b`'s mapping, as a string prefix would. A component that is not UTF-8 can
    /// match no stored path, so the walk passes it by and goes on to its parent.
    pub fn nearest_mapping(
        &self,
        dir: &Path,
        provider: &ProviderId,
    ) -> Result<Option<Mapping>, StoreError> {
        // Rebuilt from its components: a trailing `/` or a `.` would otherwise spell an
        // ancestor no mapping is stored under.
        let dir: PathBuf = dir.components().collect();
        let c = self.lock();
        let mut stmt = c.prepare(&format!(
            "SELECT {MAPPING_COLUMNS} FROM mappings WHERE path = ?1 AND provider = ?2"
        ))?;
        for ancestor in dir.ancestors() {
            let Some(path) = ancestor.to_str().filter(|p| !p.is_empty()) else {
                continue;
            };
            let found = stmt
                .query_row(params![path, provider.as_str()], mapping_from_row)
                .optional()?;
            if found.is_some() {
                return Ok(found);
            }
        }
        Ok(None)
    }
```

`exec`, `lock`, `params!` and `OptionalExtension` are already in scope in this module. No
schema change: the table is v1's, and M4a's v2 migration leaves it alone.

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-engine --test mappings`
Expected: PASS, 9 tests.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS. Nothing else reads or writes `mappings`.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/store/mod.rs crates/tagteam-engine/tests/mappings.rs
git commit -m "Store directory mappings and find a directory's nearest mapped ancestor"
```

#### Cycle 2: `map` and `unmap`

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam/tests/map_cli.rs`:

```rust
//! §12.7: `map`, `unmap` and `shell-init`, through the real binary. Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use common::{cc_profile, cmd, two_accounts};
use serde_json::{Value, json};
use tagteam_provider::Env;

fn json_of(out: &[u8]) -> Value {
    serde_json::from_slice(out).unwrap()
}

/// `rel` under `root`, created, and its canonical spelling: what `map` stores for it.
fn dir(root: &Path, rel: &str) -> (PathBuf, String) {
    let p = root.join(rel);
    fs::create_dir_all(&p).unwrap();
    let canonical = fs::canonicalize(&p).unwrap();
    (p, canonical.into_os_string().into_string().unwrap())
}

/// `map --json`'s rows.
fn listed(root: &Path) -> Vec<Value> {
    let out = cmd(root).args(["map", "--json"]).assert().success();
    json_of(&out.get_output().stdout)["mappings"]
        .as_array()
        .unwrap()
        .clone()
}

/// `map --json`'s rows without `addedAt`, once each is checked to be ISO 8601 UTC.
fn listed_shapes(root: &Path) -> Vec<Value> {
    listed(root)
        .into_iter()
        .map(|mut row| {
            let at = row.as_object_mut().unwrap().remove("addedAt").unwrap();
            let at = at.as_str().unwrap();
            assert!(at.len() == 20 && at.ends_with('Z'), "{at}");
            row
        })
        .collect()
}

#[test]
fn map_stores_the_canonical_path_even_through_a_symlink() {
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(d.path());
    let (_, real) = dir(d.path(), "work/real/app");
    symlink(d.path().join("work/real"), d.path().join("work/link")).unwrap();
    let out = cmd(d.path())
        .args(["map", "1"])
        .arg(d.path().join("work/link/app"))
        .arg("--json")
        .assert()
        .success();
    let v = json_of(&out.get_output().stdout);
    assert_eq!(v["schemaVersion"], 1);
    assert_eq!(v["ok"], true);
    assert_eq!(v["mapping"]["path"], real.as_str());
    assert_eq!(v["mapping"]["id"], a.as_str());
    assert_eq!(v["mapping"]["number"], 1);
    assert_eq!(listed(d.path()).len(), 1);
}

#[test]
fn map_defaults_to_the_current_directory_and_a_new_mapping_replaces_the_old() {
    // §12.7: one mapping per provider per path.
    let d = tempfile::tempdir().unwrap();
    let (_, b) = two_accounts(d.path());
    let (app, real) = dir(d.path(), "work/app");
    cmd(d.path())
        .current_dir(&app)
        .args(["map", "1"])
        .assert()
        .success()
        .stdout(format!("Mapped {real} to a@x.co (position 1).\n"));
    cmd(d.path())
        .current_dir(&app)
        .args(["map", "b@x.co"])
        .assert()
        .success();
    assert_eq!(
        listed_shapes(d.path()),
        vec![
            json!({"path": real, "provider": "claude-code", "number": 2, "id": b, "email": "b@x.co"})
        ]
    );
}

#[test]
fn map_lists_every_mapping_in_path_order() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = two_accounts(d.path());
    let (x, x_real) = dir(d.path(), "x");
    let (y, y_real) = dir(d.path(), "y");
    cmd(d.path()).args(["map", "2"]).arg(&y).assert().success();
    cmd(d.path()).args(["map", "1"]).arg(&x).assert().success();
    assert_eq!(
        listed_shapes(d.path()),
        vec![
            json!({"path": x_real, "provider": "claude-code", "number": 1, "id": a, "email": "a@x.co"}),
            json!({"path": y_real, "provider": "claude-code", "number": 2, "id": b, "email": "b@x.co"}),
        ]
    );
    cmd(d.path())
        .arg("map")
        .assert()
        .success()
        .stdout(format!("{x_real}  1  a@x.co\n{y_real}  2  b@x.co\n"));
}

#[test]
fn unmap_removes_a_path_s_mappings_for_one_provider_or_all() {
    let d = tempfile::tempdir().unwrap();
    two_accounts(d.path());
    let (x, real) = dir(d.path(), "x");
    cmd(d.path()).args(["map", "1"]).arg(&x).assert().success();
    let out = cmd(d.path())
        .arg("unmap")
        .arg(&x)
        .args(["--provider", "claude-code", "--json"])
        .assert()
        .success();
    assert_eq!(
        json_of(&out.get_output().stdout),
        json!({"schemaVersion": 1, "ok": true, "path": real, "removed": 1})
    );
    assert!(listed(d.path()).is_empty());
    cmd(d.path()).args(["map", "1"]).arg(&x).assert().success();
    cmd(d.path())
        .current_dir(&x)
        .arg("unmap")
        .assert()
        .success()
        .stdout(format!("Unmapped {real}.\n"));
    cmd(d.path())
        .current_dir(&x)
        .arg("unmap")
        .assert()
        .success()
        .stdout(format!("No mapping for {real}.\n"));
}

#[test]
fn unmap_takes_the_listed_path_of_a_directory_that_is_gone() {
    let d = tempfile::tempdir().unwrap();
    two_accounts(d.path());
    let (gone, real) = dir(d.path(), "gone");
    cmd(d.path())
        .args(["map", "1"])
        .arg(&gone)
        .assert()
        .success();
    fs::remove_dir(&gone).unwrap();
    let out = cmd(d.path())
        .args(["unmap", &real, "--json"])
        .assert()
        .success();
    assert_eq!(json_of(&out.get_output().stdout)["removed"], 1);
    assert!(listed(d.path()).is_empty());
}

#[test]
fn map_refuses_a_path_that_is_missing_or_not_a_directory() {
    let d = tempfile::tempdir().unwrap();
    two_accounts(d.path());
    let file = d.path().join("file");
    fs::write(&file, "").unwrap();
    for (path, why) in [
        (d.path().join("missing"), "No such file or directory"),
        (file, "not a directory"),
    ] {
        let out = cmd(d.path())
            .args(["map", "1"])
            .arg(&path)
            .arg("--json")
            .assert()
            .code(1);
        let v = json_of(&out.get_output().stdout);
        assert_eq!(v["error"]["type"], "invalid-input", "{v}");
        let message = v["error"]["message"].as_str().unwrap();
        assert!(
            message.contains(&path.display().to_string()) && message.contains(why),
            "{message}"
        );
    }
    assert!(listed(d.path()).is_empty());
}

#[test]
fn map_needs_an_account_that_exists() {
    let d = tempfile::tempdir().unwrap();
    two_accounts(d.path());
    let (x, _) = dir(d.path(), "x");
    let out = cmd(d.path())
        .args(["map", "9"])
        .arg(&x)
        .arg("--json")
        .assert()
        .code(1);
    assert_eq!(
        json_of(&out.get_output().stdout)["error"]["type"],
        "no-such-account"
    );
}

#[test]
fn removing_an_account_removes_its_mappings() {
    let d = tempfile::tempdir().unwrap();
    two_accounts(d.path());
    let (x, _) = dir(d.path(), "x");
    let (y, y_real) = dir(d.path(), "y");
    cmd(d.path()).args(["map", "1"]).arg(&x).assert().success();
    cmd(d.path()).args(["map", "2"]).arg(&y).assert().success();
    cmd(d.path()).args(["remove", "1"]).assert().success();
    let left: Vec<Value> = listed(d.path())
        .into_iter()
        .map(|r| r["path"].clone())
        .collect();
    assert_eq!(left, [json!(y_real)]);
}

#[test]
fn on_a_fresh_machine_map_and_unmap_create_nothing() {
    // §5: a command that changes nothing creates nothing.
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    let out = cmd(d.path()).args(["map", "--json"]).assert().success();
    assert_eq!(
        json_of(&out.get_output().stdout),
        json!({"schemaVersion": 1, "mappings": []})
    );
    cmd(d.path())
        .arg("map")
        .assert()
        .success()
        .stdout("No mappings yet. Map a directory with `tagteam map ACCOUNT [PATH]`.\n");
    let out = cmd(d.path())
        .args(["unmap", "/nowhere", "--json"])
        .assert()
        .success();
    assert_eq!(
        json_of(&out.get_output().stdout),
        json!({"schemaVersion": 1, "ok": true, "path": "/nowhere", "removed": 0})
    );
    assert!(!Env::for_test(d.path()).data_dir().exists());
}

#[test]
fn map_works_inside_a_run_shell_against_the_outer_home_s_store() {
    // B.32: only commands that change accounts or the live login refuse in a run shell, and
    // a run shell's commands see the outer home (§12.8).
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(d.path());
    let (_, spelling) = cc_profile(d.path(), &a);
    let (x, real) = dir(d.path(), "x");
    cmd(d.path())
        .env("CLAUDE_CONFIG_DIR", &spelling)
        .args(["map", "2"])
        .arg(&x)
        .assert()
        .success();
    let rows = listed(d.path());
    assert_eq!(rows.len(), 1);
    assert_eq!((&rows[0]["path"], &rows[0]["number"]), (&json!(real), &json!(2)));
}
```

In `crates/tagteam/tests/run_shell_cli.rs` (M4a Task 8), replace `COMMANDS` with the same list
plus the two new commands, so `an_unreadable_marker_refuses_every_command_but_statusline`
covers them:

```rust
const COMMANDS: &[&[&str]] = &[
    &["list"],
    &["status"],
    &["switch"],
    &["switch", "1"],
    &["switch", "1", "--force"],
    &["add"],
    &["add-token", "sk-ant-api03-x"],
    &["remove", "1"],
    &["disable", "1"],
    &["enable", "1"],
    &["alias"],
    &["alias", "1", "work"],
    &["move", "1", "2"],
    &["history"],
    &["map"],
    &["map", "1", "/"],
    &["unmap", "/"],
];
```

In `crates/tagteam/src/render.rs`, at the end of `mod tests` (before its closing `}`), add:

```rust
    #[test]
    fn a_mapping_line_names_the_provider_only_when_several_are_mapped() {
        let row = |position, email: &str| {
            view(position, email, OAUTH, unread(UsageStatus::Ok, None, None)).row
        };
        let mapping = |path: &str, account: &AccountRow| Mapping {
            path: path.into(),
            provider: account.provider.clone(),
            account_id: account.id.clone(),
            added_at: NOW * 1000,
        };
        let (a, b) = (row(1, "a@x.co"), row(2, "b@x.co"));
        let one = [
            (mapping("/w", &a), a.clone()),
            (mapping("/x", &b), b.clone()),
        ];
        assert_eq!(mappings_human(&one), "/w  1  a@x.co\n/x  2  b@x.co\n");
        let mut f = row(1, "f@x.co");
        f.provider = ProviderId::new("fake-agent");
        let two = [
            (mapping("/w", &a), a.clone()),
            (mapping("/w", &f), f.clone()),
        ];
        assert_eq!(
            mappings_human(&two),
            "/w  claude-code  1  a@x.co\n/w  fake-agent  1  f@x.co\n"
        );
        assert_eq!(mappings_human(&[]), NO_MAPPINGS);
    }

    #[test]
    fn a_mapping_s_json_names_its_account_and_when_it_was_made() {
        let mut a = view(1, "a@x.co", OAUTH, unread(UsageStatus::Ok, None, None)).row;
        let m = Mapping {
            path: "/w".into(),
            provider: a.provider.clone(),
            account_id: a.id.clone(),
            added_at: NOW * 1000 + 999,
        };
        let expected = json!({"path": "/w", "provider": "claude-code", "number": 1,
                              "id": "id-1", "email": "a@x.co", "addedAt": "2026-09-21T14:13:20Z"});
        assert_eq!(mapping_json(&m, &a), expected);
        a.alias = Some("work".into());
        let mut aliased = expected.clone();
        aliased["alias"] = json!("work");
        assert_eq!(mapping_json(&m, &a), aliased);
        assert_eq!(
            mappings_json(&[(m, a)]),
            json!({"schemaVersion": 1, "mappings": [aliased]})
        );
    }
```

(`NOW` is `testutil`'s 1 790 000 000 s, 2026-09-21T14:13:20Z; `added_at` is in ms, and the
rendered time is whole seconds.)

In `crates/tagteam/src/app.rs`, `mod tests`, replace M3a's
`the_late_notice_names_each_command_as_it_is_typed` with:

```rust
    #[test]
    fn the_late_notice_names_each_command_as_it_is_typed() {
        use clap::Parser;
        let cases: [&[&str]; 16] = [
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
            &["map"],
            &["unmap"],
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

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --lib`
Expected: compile errors in `render::tests`: E0412 for `Mapping` and E0425 for
`mappings_human`, `mapping_json`, `mappings_json` and `NO_MAPPINGS`.

Run: `cargo test -p tagteam --features test-support --test map_cli`
Expected: FAIL, all 10 tests, each at its first `map` or `unmap` call: exit code 2, not 0 or 1,
with clap's `error: unrecognized subcommand 'map'` (or `'unmap'`) on stderr.

Run: `cargo test -p tagteam --features test-support --test run_shell_cli an_unreadable_marker_refuses_every_command_but_statusline`
Expected: FAIL at `["map"]`: exit code 2, not 1.

- [ ] **Step 3: Implement**

In `crates/tagteam/src/cli.rs`, line 1 becomes:

```rust
use std::path::PathBuf;

use clap::{Parser, Subcommand};
```

and before `Statusline` (line 100) add:

```rust
    /// Map PATH (default: here) to ACCOUNT for its provider; with no arguments, list mappings
    ///
    /// `tagteam run` in PATH or below it, and so the `shell-init` wrapper, launches ACCOUNT
    /// there. PATH is stored canonical, and a directory holds one mapping per provider. --json
    /// prints {schemaVersion, ok, mapping: {path, provider, number, id, email, alias?,
    /// addedAt}}, and the list {schemaVersion, mappings: [...]}, with times in ISO 8601 UTC.
    Map {
        account: Option<String>,
        path: Option<PathBuf>,
    },
    /// Remove PATH's mappings (default: here): every provider's, or only --provider's
    ///
    /// --json prints {schemaVersion, ok, path, removed}.
    Unmap { path: Option<PathBuf> },
```

`--provider` is the global flag (`cli.rs:21-23`), so `tagteam unmap PATH --provider P` parses
with no flag of `Unmap`'s own. `touches_keychain` is unchanged: neither command reads a Keychain
item.

In `crates/tagteam/src/render.rs`, line 4 becomes:

```rust
use tagteam_engine::store::{AccountRow, Mapping};
```

after `NO_ACCOUNTS` (line 12) add:

```rust
const NO_MAPPINGS: &str = "No mappings yet. Map a directory with `tagteam map ACCOUNT [PATH]`.\n";
```

and after `account_json` (620–627) add:

```rust
/// One mapping (§12.7) and the account it names, as `map` reports it. `addedAt` is ISO 8601
/// UTC, as `history`'s times are.
pub fn mapping_json(m: &Mapping, account: &AccountRow) -> Value {
    let mut o = json!({
        "path": m.path,
        "provider": m.provider.as_str(),
        "number": account.position,
        "id": account.id.as_str(),
        "email": email(account),
        "addedAt": format_iso8601(m.added_at.div_euclid(1000)),
    });
    if let Some(a) = &account.alias {
        o["alias"] = json!(a);
    }
    o
}

/// `map`'s list.
pub fn mappings_json(rows: &[(Mapping, AccountRow)]) -> Value {
    let mappings: Vec<Value> = rows.iter().map(|(m, a)| mapping_json(m, a)).collect();
    json!({"schemaVersion": 1, "mappings": mappings})
}

/// `map`'s list in text: one line per mapping, in path order. The provider shows only when the
/// mappings span more than one, so a single provider looks as it would without providers
/// (§13.1).
pub fn mappings_human(rows: &[(Mapping, AccountRow)]) -> String {
    let Some((first, _)) = rows.first() else {
        return NO_MAPPINGS.to_owned();
    };
    let several = rows.iter().any(|(m, _)| m.provider != first.provider);
    rows.iter()
        .map(|(m, a)| {
            if several {
                format!("{}  {}  {}  {}\n", m.path, m.provider, a.position, name(a))
            } else {
                format!("{}  {}  {}\n", m.path, a.position, name(a))
            }
        })
        .collect()
}
```

In `crates/tagteam/src/app.rs`:

1. Imports. After `use std::io::{BufRead, IsTerminal, Write};` add
   `use std::path::{Path, PathBuf};`, and `use tagteam_engine::store::AccountRow;` becomes:

   ```rust
   use tagteam_engine::store::{AccountRow, Mapping, StoreError};
   ```

2. `dispatch`: before the `Command::Statusline { .. }` arm (line 686) add:

   ```rust
            Command::Map { account, path } => match account {
                None => self.list_mappings()?,
                Some(account) => self.map(&account, path)?,
            },
            Command::Unmap { path } => self.unmap(path)?,
   ```

3. In `impl App`, after `history` and before `live_row` (line 871), add:

   ```rust
    /// §12.7: `map` with no arguments lists the mappings, only `--provider`'s when it is given.
    /// It never creates the store (§5).
    fn list_mappings(&mut self) -> Result<(), Failure> {
        let mut rows = Vec::new();
        if let Some(store) = self.engine.existing_store()? {
            for m in store.mappings().map_err(EngineError::from)? {
                if self
                    .provider_flag
                    .as_ref()
                    .is_some_and(|p| p != &m.provider)
                {
                    continue;
                }
                // `None` only when a `remove` raced this read: its mappings went with it.
                if let Some(account) = store.account(&m.account_id).map_err(EngineError::from)? {
                    rows.push((m, account));
                }
            }
        }
        self.print(&render::mappings_human(&rows), render::mappings_json(&rows));
        Ok(())
    }

    /// §12.7: maps PATH (default: the current directory), stored canonical, for the account's
    /// own provider, replacing the mapping PATH held for that provider.
    fn map(&mut self, account: &str, path: Option<PathBuf>) -> Result<(), Failure> {
        let row = self.resolve(account)?;
        let mapping = Mapping {
            path: canonical_dir(path)?,
            provider: row.provider.clone(),
            account_id: row.id.clone(),
            added_at: self.engine.now_ms(),
        };
        self.engine
            .store()?
            .set_mapping(
                &mapping.path,
                &mapping.provider,
                &mapping.account_id,
                mapping.added_at,
            )
            .map_err(|e| match e {
                // A `remove` raced this command.
                StoreError::NoSuchAccount => EngineError::NoSuchAccount(account.to_owned()),
                e => e.into(),
            })?;
        let human = format!(
            "Mapped {} to {} (position {}).\n",
            mapping.path,
            render::name(&row),
            row.position
        );
        let mapping = render::mapping_json(&mapping, &row);
        let json = json!({"schemaVersion": 1, "ok": true, "mapping": mapping});
        self.print(&human, json);
        Ok(())
    }

    /// §12.7: removes PATH's mappings (default: the current directory), only `--provider`'s
    /// when it is given. Removing nothing is not an error, and the store is never created.
    fn unmap(&mut self, path: Option<PathBuf>) -> Result<(), Failure> {
        let path = unmapped_path(path)?;
        let removed = match self.engine.existing_store()? {
            Some(store) => store
                .remove_mappings(&path, self.provider_flag.as_ref())
                .map_err(EngineError::from)?,
            None => 0,
        };
        let human = if removed == 0 {
            format!("No mapping for {path}.\n")
        } else {
            format!("Unmapped {path}.\n")
        };
        let json = json!({"schemaVersion": 1, "ok": true, "path": path, "removed": removed});
        self.print(&human, json);
        Ok(())
    }
   ```

4. Before `#[cfg(test)] mod tests` (line 885) add:

   ```rust
/// §12.7: PATH (default: the current directory) as `map` stores it: canonical (symlinks
/// resolved, absolute), and a directory. Mappings are text, so a path that is not UTF-8 is
/// refused.
fn canonical_dir(path: Option<PathBuf>) -> Result<String, Failure> {
    let given = path.unwrap_or_else(|| PathBuf::from("."));
    let canonical =
        std::fs::canonicalize(&given).map_err(|e| invalid_path(&given, &e.to_string()))?;
    if !canonical.is_dir() {
        return Err(invalid_path(&given, "not a directory"));
    }
    stored_path(canonical)
}

/// `unmap`'s PATH: canonical, as `map` stored it, or, for a directory that is gone, absolute as
/// given, so the path `map` listed still unmaps.
fn unmapped_path(path: Option<PathBuf>) -> Result<String, Failure> {
    let given = path.unwrap_or_else(|| PathBuf::from("."));
    let resolved = match std::fs::canonicalize(&given) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::path::absolute(&given).map_err(|e| invalid_path(&given, &e.to_string()))?
        }
        Err(e) => return Err(invalid_path(&given, &e.to_string())),
    };
    stored_path(resolved)
}

/// A path as the store holds it: rebuilt from its components (no trailing `/`), as text.
fn stored_path(path: PathBuf) -> Result<String, Failure> {
    let path: PathBuf = path.components().collect();
    path.into_os_string().into_string().map_err(|raw| {
        invalid_path(
            Path::new(&raw),
            "not valid UTF-8, and mappings are stored as text",
        )
    })
}

fn invalid_path(path: &Path, why: &str) -> Failure {
    Failure::Message(KIND_INVALID_INPUT, format!("{}: {why}", path.display()))
}
   ```

5. M3a's `command_name`: after `Command::History { .. } => "history",` add:

   ```rust
        Command::Map { .. } => "map",
        Command::Unmap { .. } => "unmap",
   ```

`map` and `unmap` call no `refuse_inside_run_shell`: mappings are not accounts (B.32). Under an
unreadable marker they refuse, as every command but `statusline` does, before `dispatch`
(M4a Task 8's `app::run`).

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam --features test-support --test map_cli`
Expected: PASS, 10 tests.

Run: `cargo test -p tagteam --lib`
Expected: PASS, including the two render tests and the late-notice test's 16 cases.

Run: `cargo test -p tagteam --features test-support`
Expected: PASS, `run_shell_cli`'s `an_unreadable_marker_refuses_every_command_but_statusline`
included.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam/src/cli.rs crates/tagteam/src/app.rs crates/tagteam/src/render.rs \
  crates/tagteam/tests/map_cli.rs crates/tagteam/tests/run_shell_cli.rs
git commit -m "Map and unmap directories from the command line"
```

---

### Task 2: `shell-init`

§12.7: "`tagteam shell-init zsh|bash|fish` prints one wrapper function per registered provider
that supports sessions (for now, `claude`). The wrapper always runs
`tagteam run --provider <id> -- "$@"`, which decides from the mappings and `exec`s plain
`claude` where none applies (§12.1), so an unmapped directory costs one `tagteam` start. If
`tagteam` itself is not on `PATH`, the wrapper runs `command <launch_command> "$@"`. It's
opt-in, added by the user to their shell rc."

**Readings of the spec this task commits to:**
- **One function per provider with sessions**, in registration order, named after its launch
  command (M4a's `Provider::launch_command`) and passing its ID to `--provider`. Production
  registers only Claude Code, so the output is one `claude` function.
- **zsh and bash get the same POSIX function** (`name() { … }`), so `sh` runs it too:
  `command -v` is a builtin in each and only tests `PATH`; `command <launch>` skips the function
  itself, so the fallback never recurses; `"$@"` keeps every argument as it was, the empty ones
  included. With POSIX syntax, a user's existing alias of the same name makes the definition fail
  loudly when the rc file runs, rather than shadow the wrapper in silence (`function name {`
  would define it and leave the alias in front of it).
- **fish** gets `function claude … end` over `$argv`, a list that expands one argument per
  element, without splitting; `command -q` tests for an external command without running it.
- **A fast path before the engine**, after the run-shell check: the text needs only the
  registry, so it reads no store and no settings. Under a run-shell marker that cannot be read
  it refuses with `run-shell-unreadable`, as every command but `statusline` does (§12.8,
  Decision 19). M4a's check runs first, then this fast path, then the engine.
- **`--json` is a usage error** (exit 2, the JSON envelope), as for `statusline`: the output is
  shell code. **`--provider`** must name a registered provider, as for every command
  (`unknown-provider`, exit 1); it does not narrow the output, which §12.7 defines as every
  provider with sessions.
- **The exact text is pinned** by unit tests, and its behaviour by running it: through `sh -c`
  and `bash -c` with bash's text, `zsh -f -c` with zsh's, and `fish --no-config -c` with fish's,
  with a recording `tagteam` (and `claude`) as the only programs on `PATH`. A shell that is not
  installed is skipped, and the test prints `skipped: <shell> is not installed`.

**Files:**
- Create: `crates/tagteam/src/shell_init.rs`
- Modify: `crates/tagteam/src/lib.rs` (`mod shell_init;` after `mod root_guard;`, line 12)
- Modify: `crates/tagteam/src/cli.rs` (`ValueEnum` in the `clap` import; `Command::ShellInit`
  after Task 1's `Unmap`; `ShellArg` before `impl Command`, line 107 today)
- Modify: `crates/tagteam/src/app.rs` (imports; `SHELL_INIT_UNDER_JSON` after
  `STATUSLINE_UNDER_JSON`, line 56; the fast path in M3a's `run_command`, after M4a's
  unreadable-marker refusal; `run_shell_init` before `statusline_supported` (417 today); the
  `dispatch` arm; `command_name`; the late-notice test)
- Modify: `crates/tagteam/tests/map_cli.rs` (Task 1's file: imports; tests appended)

**Interfaces:**
- Consumes:
  - Task 1: `Command::{Map, Unmap}` and `map_cli.rs`'s `json_of`
  - M4a Task 7: `Provider::launch_command(&self) -> &'static str`; M4a Task 8:
    `app::build_registry(ctx: &Context) -> ProviderRegistry` (private to `app.rs`), `app::locate`
    and the unreadable-marker refusal in `app::run`, whose `registry` the fast path borrows;
    `tagteam_provider::MARKER_FILE` (test). None is on this branch yet; written against M4a's
    contract and Task 8's code.
  - M3a: `run_command(cli, ctx, io) -> Ended` with its statusline fast path, `Ended::Code`,
    `command_name`; existing: `fail`, `KIND_USAGE`, `EXIT_USAGE`, `EngineError::UnknownProvider`,
    `Capabilities.sessions`, `ProviderRegistry::{all, get}`
- Produces:
  - `cli::ShellArg { Zsh, Bash, Fish }` (`clap::ValueEnum`), `Command::ShellInit { shell: ShellArg }`;
    M5's `completions bash|zsh|fish` (§13.7) can reuse `ShellArg`
  - Crate-private: `shell_init::Wrapped { id: String, launch: &'static str }`,
    `shell_init::script(shell: ShellArg, providers: &[Wrapped]) -> String`,
    `app::run_shell_init(registry: &ProviderRegistry, io: &mut Io<'_>, json: bool, provider: Option<&str>, shell: ShellArg) -> i32`

**Spec:**
- §12.7: `shell-init zsh|bash|fish` prints one wrapper per registered provider with sessions;
  the wrapper runs `tagteam run --provider <id> -- "$@"`, and `command <launch_command> "$@"`
  without `tagteam` on `PATH`; opt-in.
- §12.1: `run` execs plain `claude` where no mapping applies, so the wrapper never decides.
- §13.1: exit codes (2 for a usage error); §13.2: `--json` errors are the envelope.
- §12.8: under a run-shell marker that cannot be read, every command but `statusline` refuses,
  `shell-init` among them (Decision 19).

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam/src/shell_init.rs` with the tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn claude() -> Wrapped {
        Wrapped {
            id: "claude-code".into(),
            launch: "claude",
        }
    }

    const POSIX: &str = "claude() {
  if command -v tagteam >/dev/null 2>&1; then
    tagteam run --provider claude-code -- \"$@\"
  else
    command claude \"$@\"
  fi
}
";

    #[test]
    fn zsh_gets_a_posix_function_that_runs_tagteam_run() {
        assert_eq!(script(ShellArg::Zsh, &[claude()]), POSIX);
    }

    #[test]
    fn bash_gets_the_same_function() {
        assert_eq!(script(ShellArg::Bash, &[claude()]), POSIX);
    }

    #[test]
    fn fish_gets_a_function_over_argv() {
        assert_eq!(
            script(ShellArg::Fish, &[claude()]),
            "function claude --description 'claude through tagteam run'
    if command -q tagteam
        tagteam run --provider claude-code -- $argv
    else
        command claude $argv
    end
end
"
        );
    }

    #[test]
    fn each_provider_with_sessions_gets_its_own_function() {
        let fake = Wrapped {
            id: "fake-agent".into(),
            launch: "fakeagent",
        };
        let text = script(ShellArg::Bash, &[claude(), fake]);
        assert_eq!(
            text,
            format!(
                "{POSIX}\nfakeagent() {{
  if command -v tagteam >/dev/null 2>&1; then
    tagteam run --provider fake-agent -- \"$@\"
  else
    command fakeagent \"$@\"
  fi
}}
"
            )
        );
        assert_eq!(script(ShellArg::Zsh, &[]), "", "no provider, no function");
    }
}
```

and declare it in `crates/tagteam/src/lib.rs`, after `mod root_guard;` (line 12):

```rust
mod shell_init;
```

In `crates/tagteam/tests/map_cli.rs`, change `use std::os::unix::fs::symlink;` to
`use std::os::unix::fs::{PermissionsExt, symlink};` and `use tagteam_provider::Env;` to
`use tagteam_provider::{Env, MARKER_FILE};`, then append:

```rust
/// A stand-in for `name` in `bin`: it writes each argument it gets, one per line, to
/// `<bin>/<name>.args`.
fn recorder(bin: &Path, name: &str) {
    fs::create_dir_all(bin).unwrap();
    let path = bin.join(name);
    let out = bin.join(format!("{name}.args"));
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{}'\n",
            out.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// The arguments the stand-in for `name` was given, if it ran.
fn recorded(bin: &Path, name: &str) -> Option<Vec<String>> {
    let text = fs::read_to_string(bin.join(format!("{name}.args"))).ok()?;
    Some(text.lines().map(str::to_owned).collect())
}

/// The first `name` on this test's `PATH`: the shell, if it is installed.
fn installed(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// What `tagteam shell-init <shell>` prints.
fn wrapper(root: &Path, shell: &str) -> String {
    let out = cmd(root).args(["shell-init", shell]).assert().success();
    String::from_utf8(out.get_output().stdout.clone()).unwrap()
}

/// A call with the arguments a careless wrapper would split, glob, expand or drop.
const CALL: &str = "claude 'a b' '' '*' '$HOME' --json -- \"x\"";
const PASSED: [&str; 7] = ["a b", "", "*", "$HOME", "--json", "--", "x"];

/// Each shell, its flags for running a script without reading any rc file, and the
/// `shell-init` text it runs: POSIX `sh` runs bash's.
const SHELLS: [(&str, &[&str], &str); 4] = [
    ("sh", &["-c"], "bash"),
    ("bash", &["-c"], "bash"),
    ("zsh", &["-f", "-c"], "zsh"),
    ("fish", &["--no-config", "-c"], "fish"),
];

/// Runs each installed shell's wrapper and then `CALL`, with only `bin-<shell>` on `PATH`;
/// a shell that is not installed is skipped, and the test says so.
fn through_each_shell(root: &Path, stand_ins: &[&str]) -> Vec<(&'static str, PathBuf)> {
    let mut ran = Vec::new();
    for (name, flags, init) in SHELLS {
        let Some(shell) = installed(name) else {
            eprintln!("skipped: {name} is not installed");
            continue;
        };
        let bin = root.join(format!("bin-{name}"));
        for stand_in in stand_ins {
            recorder(&bin, stand_in);
        }
        let status = std::process::Command::new(&shell)
            .args(flags)
            .arg(format!("{}\n{CALL}\n", wrapper(root, init)))
            .env_clear()
            .env("PATH", &bin)
            .env("HOME", &bin)
            .status()
            .unwrap();
        assert!(status.success(), "{name}: {status:?}");
        ran.push((name, bin));
    }
    ran
}

#[test]
fn the_wrapper_hands_every_argument_to_tagteam_run_intact() {
    let d = tempfile::tempdir().unwrap();
    let mut expected = vec!["run", "--provider", "claude-code", "--"];
    expected.extend(PASSED);
    for (name, bin) in through_each_shell(d.path(), &["tagteam", "claude"]) {
        assert_eq!(
            recorded(&bin, "tagteam").as_deref(),
            Some(&expected.iter().map(|s| s.to_string()).collect::<Vec<_>>()[..]),
            "{name}"
        );
        assert_eq!(
            recorded(&bin, "claude"),
            None,
            "{name}: tagteam decides, so the wrapper never runs claude itself"
        );
    }
}

#[test]
fn without_tagteam_on_path_the_wrapper_runs_claude_itself() {
    let d = tempfile::tempdir().unwrap();
    for (name, bin) in through_each_shell(d.path(), &["claude"]) {
        assert_eq!(
            recorded(&bin, "claude").as_deref(),
            Some(&PASSED.map(str::to_owned)[..]),
            "{name}"
        );
    }
}

#[test]
fn shell_init_prints_text_never_json_and_knows_its_shells() {
    let d = tempfile::tempdir().unwrap();
    let out = cmd(d.path())
        .args(["shell-init", "zsh", "--json"])
        .assert()
        .code(2);
    assert_eq!(
        json_of(&out.get_output().stdout),
        json!({"schemaVersion": 1, "error": {"type": "usage",
               "message": "shell-init prints shell code; run it without --json"}})
    );
    cmd(d.path())
        .args(["shell-init", "powershell"])
        .assert()
        .code(2);
    cmd(d.path()).arg("shell-init").assert().code(2);
    cmd(d.path())
        .args(["--provider", "nope", "shell-init", "zsh"])
        .assert()
        .code(1)
        .stderr("tagteam: unknown provider \"nope\"\n");
}

#[test]
fn shell_init_needs_no_store_and_creates_nothing() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    let text = wrapper(d.path(), "fish");
    assert!(text.starts_with("function claude "), "{text}");
    assert!(
        fs::read_dir(d.path().join("home"))
            .unwrap()
            .next()
            .is_none(),
        "HOME stays empty"
    );
}

#[test]
fn shell_init_refuses_under_a_marker_that_cannot_be_read() {
    // §12.8, Decision 19: under an unreadable run-shell marker every command but `statusline`
    // refuses, `shell-init` among them, and prints no wrapper.
    let d = tempfile::tempdir().unwrap();
    let profile = Env::for_test(d.path()).data_dir().join("sessions/0192");
    fs::create_dir_all(&profile).unwrap();
    fs::write(profile.join(MARKER_FILE), "{\"format\": \"tagteam-profile\"").unwrap();
    let out = cmd(d.path())
        .env("CLAUDE_CONFIG_DIR", &profile)
        .args(["shell-init", "bash", "--json"])
        .assert()
        .code(1);
    assert_eq!(
        json_of(&out.get_output().stdout)["error"]["type"],
        "run-shell-unreadable",
        "the marker check runs before shell-init's own --json refusal"
    );
    cmd(d.path())
        .env("CLAUDE_CONFIG_DIR", &profile)
        .args(["shell-init", "bash"])
        .assert()
        .code(1)
        .stdout("");
}
```

In `crates/tagteam/src/app.rs`, in Task 1's version of
`the_late_notice_names_each_command_as_it_is_typed`, add `&["shell-init", "zsh"],` after
`&["unmap"],` and make the array `[&[&str]; 17]`.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --lib shell_init::tests`
Expected: compile errors only: E0422 for `Wrapped`, E0425 for `script`, E0433 for `ShellArg`
(no such item in `crate::cli`).

Run: `cargo test -p tagteam --features test-support --test map_cli`
Expected: Task 1's 10 tests pass, and the 5 new ones fail. Four fail at their first
`shell-init` call: exit code 2, with clap's `error: unrecognized subcommand 'shell-init'`, where
they expect success, or 1 for `shell_init_refuses_under_a_marker_that_cannot_be_read`.
`shell_init_prints_text_never_json_and_knows_its_shells` gets its exit 2, but fails on the
message, which is clap's error kind rather than `shell-init prints shell code; …`.

- [ ] **Step 3: Implement**

Put this above the tests in `crates/tagteam/src/shell_init.rs`:

```rust
//! §12.7's opt-in shell wrapper: one function per registered provider that supports sessions,
//! named after its launch command. The function always runs `tagteam run`, which decides from
//! the mappings and `exec`s the plain launch command where none applies (§12.1). Where
//! `tagteam` itself is not on `PATH`, it runs the launch command directly.

use crate::cli::ShellArg;

/// A provider the wrapper covers: its ID, which `--provider` takes, and its launch command,
/// which names the function and runs when `tagteam` cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Wrapped {
    pub(crate) id: String,
    pub(crate) launch: &'static str,
}

/// The text `shell-init` prints: every function in `providers`' order, a blank line between
/// two. zsh and bash get the same POSIX function, which `sh` runs too:
/// - `command -v` is a builtin in each, and tests `PATH` without running anything;
/// - `command <launch>` skips the function itself, so the fallback never recurses;
/// - `"$@"` passes every argument as it was given, the empty ones included.
///
/// A user's alias of the same name makes the definition fail when the rc file runs, rather
/// than shadow the wrapper silently. fish's `$argv` is a list, and expands one argument per
/// element without splitting.
pub(crate) fn script(shell: ShellArg, providers: &[Wrapped]) -> String {
    let functions: Vec<String> = providers.iter().map(|p| function(shell, p)).collect();
    functions.join("\n")
}

fn function(shell: ShellArg, p: &Wrapped) -> String {
    let (id, launch) = (p.id.as_str(), p.launch);
    match shell {
        ShellArg::Zsh | ShellArg::Bash => format!(
            r#"{launch}() {{
  if command -v tagteam >/dev/null 2>&1; then
    tagteam run --provider {id} -- "$@"
  else
    command {launch} "$@"
  fi
}}
"#
        ),
        ShellArg::Fish => format!(
            r#"function {launch} --description '{launch} through tagteam run'
    if command -q tagteam
        tagteam run --provider {id} -- $argv
    else
        command {launch} $argv
    end
end
"#
        ),
    }
}
```

In `crates/tagteam/src/cli.rs`, the `clap` import becomes
`use clap::{Parser, Subcommand, ValueEnum};`. After Task 1's `Unmap` add:

```rust
    /// Print the shell function that runs claude through `tagteam run`
    ///
    /// zsh: add `eval "$(tagteam shell-init zsh)"` to ~/.zshrc. bash: the same line, with
    /// bash, in ~/.bashrc. fish: add `tagteam shell-init fish | source` to
    /// ~/.config/fish/config.fish.
    ShellInit { shell: ShellArg },
```

and before `impl Command` add:

```rust
/// The shells `shell-init` writes for (§12.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ShellArg {
    Zsh,
    Bash,
    Fish,
}
```

In `crates/tagteam/src/app.rs`:

1. Imports: `use crate::cli::{Cli, Command};` becomes `use crate::cli::{Cli, Command, ShellArg};`,
   and `use crate::{history, render, root_guard, statusline};` becomes:

   ```rust
   use crate::shell_init::Wrapped;
   use crate::{history, render, root_guard, shell_init, statusline};
   ```

2. After `STATUSLINE_UNDER_JSON` (line 56) add:

   ```rust
   const SHELL_INIT_UNDER_JSON: &str = "shell-init prints shell code; run it without --json";
   ```

3. In M3a's `run_command`, directly after M4a's unreadable-marker refusal (the
   `if let RunShell::Unreadable { marker, detail } = &run_shell { … }` block) and before
   `let command = cli.command.unwrap_or(Command::List);`, add:

   ```rust
    // §12.7: the wrapper needs only the registered providers, so it reads no store and no
    // settings. It comes after the marker check: under a marker that cannot be read it refuses,
    // as every command but `statusline` does (§12.8, Decision 19).
    if let Some(Command::ShellInit { shell }) = &cli.command {
        return Ended::Code(run_shell_init(
            &registry,
            io,
            json,
            cli.provider.as_deref(),
            *shell,
        ));
    }
   ```

4. Before `statusline_supported` add:

   ```rust
/// §12.7's `shell-init`, answered before the engine is built: one wrapper per registered
/// provider with sessions (`Wrapped`). `--provider` must name a registered provider, as for
/// every command.
fn run_shell_init(
    registry: &ProviderRegistry,
    io: &mut Io<'_>,
    json: bool,
    provider: Option<&str>,
    shell: ShellArg,
) -> i32 {
    if json {
        fail(io, true, KIND_USAGE, SHELL_INIT_UNDER_JSON);
        return EXIT_USAGE;
    }
    if let Some(id) = provider {
        if registry.get(&ProviderId::new(id)).is_none() {
            let e = EngineError::UnknownProvider(id.to_owned());
            return fail(io, false, e.kind(), &e.to_string());
        }
    }
    let wrapped: Vec<Wrapped> = registry
        .all()
        .iter()
        .filter(|p| p.capabilities().sessions)
        .map(|p| Wrapped {
            id: p.id().as_str().to_owned(),
            launch: p.launch_command(),
        })
        .collect();
    let _ = write!(io.out, "{}", shell_init::script(shell, &wrapped));
    0
}
   ```

5. `dispatch`, after the `Command::Statusline { .. }` arm:

   ```rust
            Command::ShellInit { .. } => unreachable!("run answers shell-init before dispatch"),
   ```

6. `command_name`: add `Command::ShellInit { .. } => "shell-init",` after Task 1's
   `Command::Unmap { .. } => "unmap",`.

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam --lib`
Expected: PASS, the 4 `shell_init` tests and the late-notice test's 17 cases included.

Run: `cargo test -p tagteam --features test-support --test map_cli -- --nocapture`
Expected: PASS, 15 tests. Each shell missing on the machine prints
`skipped: <shell> is not installed` (macOS has `sh`, `bash` and `zsh`; fish is usually absent,
and is then covered by its pinned text alone).

Run: `cargo test -p tagteam --features test-support`
Expected: PASS.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam/src/shell_init.rs crates/tagteam/src/lib.rs crates/tagteam/src/cli.rs \
  crates/tagteam/src/app.rs crates/tagteam/tests/map_cli.rs
git commit -m "Print a shell wrapper that sends claude through tagteam run"
```

---

### Task 3: Process primitives, `Cancel::take`, the fake `claude`

Three pieces that every later task spawns through, each its own cycle:
- **`Cancel::take`** (Decision 1): `run`'s wait loop consumes the one-slot token, which M3a
  only ever reads.
- **`tagteam_provider::process`** (Decisions 2, 3, 10):
  - `run_captured` for `claude auth status` (§12.3 step 8): non-interactive, in its own group,
    in a given environment and directory, killed with its group on a timeout or a cancel;
  - `spawn_session` for `claude` itself (§12.5 step 7): attached, in the terminal's foreground
    group, inheriting exactly the reservation fd;
  - `exec_command` for plain `claude` (§12.1); `find_on_path` for the lookup before any lock;
    `exit_code` for §12.5's exit code;
  - the `ProcessSpawner` port that engine tests script.

  `Runner` and `SecurityCli` are untouched (Decision 2).
- **The fake `claude`** (Decision 11) that every CLI test of `run` puts first on `PATH`.

**Readings of the spec this task commits to:**
- **`take` swaps the cell to 0** and returns what was there. Only `run`'s loop calls it; every
  cancellation point keeps reading with `requested`/`check`, and `run_captured` reads, never
  takes, so the engine still sees an interruption after it.
- **`run_captured` and the token.** A token already set spawns nothing and returns
  `Interrupted`. Otherwise the wait polls the child, the deadline and the token every 10 ms, as
  M3a's `run_bounded` polls. On a timeout or a cancel it sends SIGKILL to the whole group
  (`killpg`), while the leader is still unreaped, so the group ID names that group and no
  other, then reaps the leader. `claude auth status` may start children of its own, and §12.5
  says the login check's process "is then killed".
- **Draining, as `run_bounded` does.** Both pipes drain on detached threads. Once the leader
  has exited, the pipes get 2 s (`DRAIN_GRACE`) to close; output held open past that is
  `TimedOut`, never half an output. The token is checked during that wait too. A grandchild
  holding a pipe then is left alone: the leader is reaped, so if its group has emptied the ID
  could already name another one. Killing by a stale ID is the unsafe direction. (Rejected:
  `waitid(WNOWAIT)` to keep the leader unreaped through the drain. It is platform-specific code
  for a case `claude auth status` does not produce.)
- **`SpawnSpec`**: the child starts from tagteam's own environment; `remove` is applied, then
  `set`, so a name in both ends up set. The session and validation specs Task 7 builds never
  overlap the two.
- **`spawn_session`** adds `pre_exec` only when it has a fd to pass, and clears `FD_CLOEXEC` on
  that fd alone, in the child (Decision 3), so no other child ever holds a reservation. It sets
  no process group: the child stays in tagteam's, the terminal's foreground group (§14.1).
  `spawn` returns only after the child has exec'd, so a signal sent to its pid then reaches
  `claude` itself (M4b preflight 1, §6.2 item 2).
- **`find_on_path`** behaves as a shell: a name with a `/` is not searched for; an empty
  `PATH` entry is the current directory; the first regular file (after symlinks) that
  `access(X_OK)` allows wins, so a file this user cannot execute is passed over. The result is
  absolute, since Task 12 may spawn with another working directory. No `PATH` finds nothing.
- **`exit_code`**: the code, else `128 + signal`, else 1 (a stopped status, which `wait` never
  returns here).
- **`ScriptedSpawner`** behaves as the real one where engine tests need it to: a set token
  answers `Interrupted` without using the next reply (so the login check's cancellation point is
  testable), and an empty script is `SpawnFailed`, as a missing launch command (`unreachable`,
  §12.3). Every call's spec is recorded, the cancelled ones included.
- **`SpawnSpec` also derives `PartialEq, Eq`** (`ScriptedSpawner::specs()` is compared in
  tests), and `SystemSpawner` derives `Debug, Clone, Copy, Default`, as the Interface Contract
  records.
- **Every test that forks holds `FORK_GUARD`**, as `process.rs` and `flock.rs` already do: a
  child forked while another test re-locks a flock holds a duplicate of that lock.
- **The fake `claude`** is a `/bin/sh` script written into `<root>/bin`; `FAKE_CLAUDE_*`
  variables steer it, and it records each run as lines `<pid> <key> <value>` in
  `FAKE_CLAUDE_OUT`, so overlapping runs (two sessions of one account, Review Focus 3) never mix.
  It is the only fake: Tasks 12 and 13 run against it, and `fake_claude` documents its one list
  of variables (Decision 11).
  - It records its pid, parent, directory, arguments (as bytes, so one that is not UTF-8
    survives) and environment, then each SIGHUP, SIGINT and SIGTERM it catches, and its exit.
  - It sleeps in short background sleeps that a trapped signal ends, so a signal is recorded at
    once. `FAKE_CLAUDE_HOLD` keeps a session running until a file exists, for the tests that
    must decide when `claude` exits.
  - Like Claude Code it removes its session record on SIGINT, SIGTERM and SIGHUP, and SIGKILL
    leaves it behind (§12.6, Appendix A.7).
  - `auth status` exits non-zero when logged out (A.7). With no reply scripted it answers as
    Claude Code would from the config home: `api_key_helper` when its `settings.json` names an
    `apiKeyHelper`, a `claude.ai` login as its `.claude.json`'s `oauthAccount` when a
    `.credentials.json` is there, and logged out otherwise. CLI tests then need no reply per
    test, and Review Focus 5's helper refuses through the real path.
  - It writes the profile's `.credentials.json` for a rotation: after a bootstrap the
    profile's hashed item is gone (§12.3 step 5), so the file is what Claude Code, and tagteam,
    read.

**Files:**
- Modify: `crates/tagteam-provider/src/cancel.rs` (M3a's file: `take` after `requested`, two
  tests before `an_interruption_names_its_signal`)
- Modify: `crates/tagteam-provider/src/process.rs` (`use std::io;`, line 1, becomes the import
  block below; the new items after the macOS `start_of`, before `#[cfg(test)]` (line 78); the
  test module's imports (line 80) and its new tests)
- Modify: `crates/tagteam-provider/src/lib.rs` (`pub use process::ProcessStamp;`, line 36)
- Modify: `crates/tagteam/tests/common/mod.rs` (four `use` lines; `FAKE_CLAUDE`, `fake_claude`,
  `path_with`, `FakeCall`, `fake_claude_calls` appended)
- Create: `crates/tagteam/tests/run_cli.rs` (Tasks 12 and 13 append to it)

**Interfaces:**
- Consumes:
  - M3a: `tagteam_provider::cancel::{Cancel, Interrupted}` (`new`, `request`, `requested`,
    `check`, `cell`; the private `signal: Arc<AtomicUsize>`), on `m3-auto-switch`, not on this
    branch yet. M3a's `security::ProcessRunner::run_bounded` is the model for draining and
    polling, not a dependency.
  - M4a Task 6 leaves `process.rs`'s `ProcessStamp` and `start_of` (`pub(crate)`) as they are;
    the new items go after them.
  - `crate::FORK_GUARD` (`lib.rs:20-23`); `libc` (`killpg`, `fcntl`, `access`, `getpgid`,
    `getpgrp`, `kill`).
- Produces (Interface Contract, `src/process.rs` and `src/cancel.rs`, all re-exported at the
  crate root):
  - `Cancel::take(&self) -> Option<i32>`
  - `SpawnSpec { program: PathBuf, args: Vec<OsString>, set: Vec<(OsString, OsString)>, remove: Vec<OsString>, cwd: Option<PathBuf> }`
  - `run_captured(spec: &SpawnSpec, timeout: Duration, cancel: &Cancel) -> Captured`
  - `Captured { Exited { code: Option<i32>, signal: Option<i32>, stdout: Vec<u8>, stderr: Vec<u8> }, TimedOut, Interrupted(i32), SpawnFailed(String) }`
  - `spawn_session(spec: &SpawnSpec, inherit: Option<RawFd>) -> io::Result<std::process::Child>`
  - `exec_command(spec: &SpawnSpec) -> io::Error`
  - `find_on_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf>`
  - `exit_code(status: ExitStatus) -> i32`
  - `trait ProcessSpawner: Send + Sync { fn run_captured(&self, spec: &SpawnSpec, timeout: Duration, cancel: &Cancel) -> Captured; }`,
    `SystemSpawner`, `ScriptedSpawner::{new, push, specs}`
  - Test helpers, `crates/tagteam/tests/common/mod.rs`:
    `fake_claude(root: &Path) -> PathBuf` (returns `<root>/bin`), `path_with(bin: &Path) -> OsString`,
    `FakeCall { pid: u32, mode: String, ppid: u32, cwd: PathBuf, args: Vec<OsString>, env: BTreeMap<String, String>, ready: bool, signals: Vec<i32>, exit: Option<i32> }`,
    `fake_claude_calls(out: &Path) -> Vec<FakeCall>`, and the `FAKE_CLAUDE_*` variables
    documented on `fake_claude`
  - `run_cli.rs`'s file-local helpers, which Task 12 calls: `fake`, `wait_for`,
    `send(pid: u32, signal: libc::c_int)` and `mode`

**Spec:**
- §12.1: plain `claude` is `exec`, with the environment it was given; the launch command is
  looked up on `PATH` before any lock is taken.
- §12.3 step 8: validation runs in exactly the session environment and the directory `claude`
  will run in, with a 10 s timeout; `unreachable` when it cannot be spawned.
- §12.5: `claude` is spawned with the reservation fd and stays in the terminal's foreground
  group; the wait for the login check is a cancellation point, whose process is then killed;
  a signal recorded after the last check is sent to `claude` once it exists; the exit code is
  the child's, `128 + signal` when a signal killed it (B.63).
- §14.1: non-interactive children run in their own process group; interactive ones, `claude`
  under `run` among them, stay in the terminal's.
- §12.6, Appendix A.7: what the fake mimics (records, graceful signals, `auth status`).
- Decisions 1, 2, 3, 10 and 11.

#### Cycle 1: `Cancel::take`

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-provider/src/cancel.rs`, `mod tests`, before
`an_interruption_names_its_signal`, add:

```rust
    #[test]
    fn take_returns_the_signal_once_and_clears_it_for_every_clone() {
        let c = Cancel::new();
        let clone = c.clone();
        assert_eq!(c.take(), None, "nothing is recorded yet");
        clone.request(15);
        assert_eq!(c.take(), Some(15));
        assert_eq!(clone.requested(), None);
        assert_eq!(clone.check(), Ok(()));
        assert_eq!(clone.take(), None, "a signal is taken once");
    }

    #[test]
    fn a_signal_after_a_take_is_recorded_again() {
        let c = Cancel::new();
        c.request(2);
        assert_eq!(c.take(), Some(2));
        // What a handler does: it stores into the cell, whatever was taken before.
        c.cell().store(1, Ordering::SeqCst);
        assert_eq!(c.requested(), Some(1));
        assert_eq!(c.take(), Some(1));
    }
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-provider --lib cancel::tests`
Expected: compile error E0599: no method named `take` found for struct `Cancel`.

- [ ] **Step 3: Implement**

In `crates/tagteam-provider/src/cancel.rs`, after `requested`, add:

```rust
    /// Consumes the recorded signal (§12.5's forwarding): returns it and leaves the token unset,
    /// for every clone. Only `run`'s wait loop takes (M4b Decision 1). It forwards what arrived
    /// while `claude` runs, and it clears what forwarding left once `claude` has exited, so the
    /// exit handling's cancellation points see only new signals. Everything else only reads.
    pub fn take(&self) -> Option<i32> {
        match self.signal.swap(0, Ordering::SeqCst) {
            0 => None,
            n => Some(n as i32),
        }
    }
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-provider --lib cancel::tests`
Expected: PASS, 7 tests.

Run: `cargo test -p tagteam-provider`
Expected: PASS.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider/src/cancel.rs
git commit -m "Let the run loop take the signal from the cancel token"
```

#### Cycle 2: spawning, capture, `exec` and the `PATH` lookup

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-provider/src/process.rs`, the first line of `mod tests` (`use super::*;`,
line 80) becomes:

```rust
    use super::*;
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::MutexGuard;
```

and append to `mod tests`, after `an_exited_child_is_not_live`:

```rust
    fn fork_guard() -> MutexGuard<'static, ()> {
        crate::FORK_GUARD
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn spec(program: &str, args: &[&str]) -> SpawnSpec {
        SpawnSpec {
            program: PathBuf::from(program),
            args: args.iter().map(OsString::from).collect(),
            ..SpawnSpec::default()
        }
    }

    /// `/bin/sh -c script`.
    fn sh(script: &str) -> SpawnSpec {
        spec("/bin/sh", &["-c", script])
    }

    /// A shell that leaves a background grandchild writing `marker` a second later, then waits
    /// 30 s: whatever kills only the shell lets the grandchild write.
    fn with_grandchild(marker: &Path) -> SpawnSpec {
        sh(&format!(
            "(sleep 1; echo alive > '{}') & sleep 30",
            marker.display()
        ))
    }

    /// Waits for a shell to write its pid to `file`.
    fn pid_in(file: &Path) -> libc::pid_t {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let written = fs::read_to_string(file).ok();
            if let Some(pid) = written.and_then(|t| t.trim().parse().ok()) {
                return pid;
            }
            assert!(Instant::now() < deadline, "the shell never wrote its pid");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn own_group() -> libc::pid_t {
        // SAFETY: getpgrp(2) takes no arguments and cannot fail.
        unsafe { libc::getpgrp() }
    }

    fn group_of(pid: libc::pid_t) -> libc::pid_t {
        // SAFETY: getpgid(2) only reads the process table; `pid` is a child that is still
        // running and not yet reaped, so it names that child and no other process.
        let group = unsafe { libc::getpgid(pid) };
        assert!(group > 0, "getpgid: {}", io::Error::last_os_error());
        group
    }

    #[test]
    fn a_captured_child_returns_its_code_and_both_outputs() {
        let _fork = fork_guard();
        let got = run_captured(
            &sh("printf out; printf err >&2; exit 3"),
            Duration::from_secs(5),
            &Cancel::new(),
        );
        assert_eq!(
            got,
            Captured::Exited {
                code: Some(3),
                signal: None,
                stdout: b"out".to_vec(),
                stderr: b"err".to_vec(),
            }
        );
    }

    #[test]
    fn a_captured_child_killed_by_a_signal_reports_the_signal() {
        let _fork = fork_guard();
        let got = run_captured(&sh("kill -TERM $$"), Duration::from_secs(5), &Cancel::new());
        assert!(
            matches!(
                got,
                Captured::Exited {
                    code: None,
                    signal: Some(15),
                    ..
                }
            ),
            "{got:?}"
        );
    }

    #[test]
    fn a_captured_child_gets_the_spec_s_environment_and_directory() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let env = SpawnSpec {
            set: vec![("TAGTEAM_PROBE".into(), "on".into())],
            remove: vec!["PATH".into(), "TAGTEAM_PROBE".into()],
            ..spec("/usr/bin/env", &[])
        };
        let Captured::Exited {
            code: Some(0),
            stdout,
            ..
        } = run_captured(&env, Duration::from_secs(5), &Cancel::new())
        else {
            panic!("env did not run");
        };
        let lines: Vec<&str> = std::str::from_utf8(&stdout).unwrap().lines().collect();
        assert!(
            lines.contains(&"TAGTEAM_PROBE=on"),
            "set wins over remove: {lines:?}"
        );
        assert!(!lines.iter().any(|l| l.starts_with("PATH=")), "{lines:?}");
        let pwd = SpawnSpec {
            cwd: Some(d.path().to_path_buf()),
            ..sh("pwd -P")
        };
        let Captured::Exited { stdout, .. } =
            run_captured(&pwd, Duration::from_secs(5), &Cancel::new())
        else {
            panic!("pwd did not run");
        };
        let canonical = fs::canonicalize(d.path()).unwrap();
        assert_eq!(stdout, format!("{}\n", canonical.display()).into_bytes());
    }

    #[test]
    fn a_captured_child_reads_no_terminal() {
        // stdin is /dev/null: a child that reads it sees end of file at once.
        let _fork = fork_guard();
        let got = run_captured(&sh("cat"), Duration::from_secs(5), &Cancel::new());
        assert!(
            matches!(&got, Captured::Exited { code: Some(0), stdout, .. } if stdout.is_empty()),
            "{got:?}"
        );
    }

    #[test]
    fn a_captured_child_leads_a_process_group_of_its_own() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("pid");
        let script = sh(&format!("echo $$ > '{}'; sleep 1", file.display()));
        let run =
            thread::spawn(move || run_captured(&script, Duration::from_secs(5), &Cancel::new()));
        let pid = pid_in(&file);
        let group = group_of(pid);
        assert!(matches!(
            run.join().unwrap(),
            Captured::Exited { code: Some(0), .. }
        ));
        assert_eq!(group, pid, "the child leads its own group");
        assert_ne!(
            group,
            own_group(),
            "never the caller's, the terminal's foreground group"
        );
    }

    #[test]
    fn a_timeout_kills_the_child_s_whole_group() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let marker = d.path().join("alive");
        let started = Instant::now();
        let got = run_captured(
            &with_grandchild(&marker),
            Duration::from_millis(200),
            &Cancel::new(),
        );
        assert_eq!(got, Captured::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "not the shell's 30 s"
        );
        thread::sleep(Duration::from_millis(1500));
        assert!(!marker.exists(), "the grandchild died with its group");
    }

    #[test]
    fn a_cancel_kills_the_child_s_whole_group_and_names_the_signal() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let marker = d.path().join("alive");
        let cancel = Cancel::new();
        let signaller = cancel.clone();
        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            signaller.request(15);
        });
        let started = Instant::now();
        let got = run_captured(&with_grandchild(&marker), Duration::from_secs(30), &cancel);
        sender.join().unwrap();
        assert_eq!(got, Captured::Interrupted(15));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(
            cancel.requested(),
            Some(15),
            "the wait reads the token, never takes it"
        );
        thread::sleep(Duration::from_millis(1500));
        assert!(!marker.exists(), "the grandchild died with its group");
    }

    #[test]
    fn a_token_already_set_spawns_nothing() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let marker = d.path().join("ran");
        let cancel = Cancel::new();
        cancel.request(2);
        let got = run_captured(
            &sh(&format!("echo ran > '{}'", marker.display())),
            Duration::from_secs(5),
            &cancel,
        );
        assert_eq!(got, Captured::Interrupted(2));
        thread::sleep(Duration::from_millis(300));
        assert!(!marker.exists());
    }

    #[test]
    fn a_program_that_cannot_be_spawned_is_a_spawn_failure() {
        let _fork = fork_guard();
        let got = run_captured(
            &spec("/nonexistent/tagteam-probe", &[]),
            Duration::from_secs(5),
            &Cancel::new(),
        );
        assert!(matches!(got, Captured::SpawnFailed(_)), "{got:?}");
    }

    #[test]
    fn output_held_open_past_the_grace_is_never_half_reported() {
        // The shell exits at once, but a grandchild keeps its stdout open for 2 s.
        let _fork = fork_guard();
        let got = capture(
            &sh("sleep 2 & printf partial"),
            Duration::from_secs(5),
            &Cancel::new(),
            Duration::from_millis(200),
        );
        assert_eq!(got, Captured::TimedOut);
    }

    #[test]
    fn a_cancel_while_the_output_drains_is_an_interruption() {
        let _fork = fork_guard();
        let cancel = Cancel::new();
        let signaller = cancel.clone();
        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            signaller.request(1);
        });
        let started = Instant::now();
        let got = capture(
            &sh("sleep 3 & printf partial"),
            Duration::from_secs(5),
            &cancel,
            Duration::from_secs(5),
        );
        sender.join().unwrap();
        assert_eq!(got, Captured::Interrupted(1));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_session_passes_exactly_the_one_descriptor_it_is_given() {
        let _fork = fork_guard();
        let (keep, other) = (
            File::open("/dev/null").unwrap(),
            File::open("/dev/null").unwrap(),
        );
        let (k, o) = (keep.as_raw_fd(), other.as_raw_fd());
        let inherited = sh(&format!("test -e /dev/fd/{k} && ! test -e /dev/fd/{o}"));
        let status = spawn_session(&inherited, Some(k)).unwrap().wait().unwrap();
        assert!(status.success(), "the child sees {k} and only {k}");
        // SAFETY: fcntl(2) with F_GETFD on a descriptor `keep` owns.
        let flags = unsafe { libc::fcntl(k, libc::F_GETFD) };
        assert_ne!(
            flags & libc::FD_CLOEXEC,
            0,
            "only the child's copy lost FD_CLOEXEC"
        );
        let none = sh(&format!("! test -e /dev/fd/{k} && ! test -e /dev/fd/{o}"));
        let status = spawn_session(&none, None).unwrap().wait().unwrap();
        assert!(
            status.success(),
            "without `inherit`, no descriptor reaches the child"
        );
    }

    #[test]
    fn a_session_with_a_closed_descriptor_is_not_spawned() {
        let _fork = fork_guard();
        let err = spawn_session(&sh("exit 0"), Some(10_000)).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EBADF));
    }

    #[test]
    fn a_session_stays_in_the_caller_s_process_group() {
        // §14.1: `claude` stays in the terminal's foreground group, so a Ctrl-C reaches it.
        let _fork = fork_guard();
        let mut child = spawn_session(&spec("/bin/sleep", &["5"]), None).unwrap();
        let group = group_of(child.id() as libc::pid_t);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(group, own_group());
    }

    #[test]
    fn a_session_gets_the_spec_s_environment_and_directory() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let canonical = fs::canonicalize(d.path()).unwrap();
        let session = SpawnSpec {
            set: vec![("TAGTEAM_PROBE".into(), "on".into())],
            cwd: Some(d.path().to_path_buf()),
            ..sh(&format!(
                "test \"$TAGTEAM_PROBE\" = on && test \"$(pwd -P)\" = '{}'",
                canonical.display()
            ))
        };
        let status = spawn_session(&session, None).unwrap().wait().unwrap();
        assert!(status.success());
    }

    /// Set in the copy of this test binary that the exec test starts, naming its scratch dir.
    const EXEC_PROBE: &str = "TAGTEAM_TEST_EXEC_PROBE";

    #[test]
    fn exec_replaces_the_process_and_returns_only_its_error() {
        if let Some(dir) = std::env::var_os(EXEC_PROBE) {
            // The copy. A program that cannot run returns the error, and the process carries
            // on; one that can replaces it, keeping its pid.
            let dir = PathBuf::from(dir);
            let missing = exec_command(&spec("/nonexistent/tagteam-probe", &[]));
            fs::write(dir.join("missing"), format!("{:?}", missing.kind())).unwrap();
            let replaced = SpawnSpec {
                set: vec![("TAGTEAM_PROBE".into(), "on".into())],
                cwd: Some(dir.clone()),
                ..sh("echo \"$$ $TAGTEAM_PROBE $(pwd -P)\" > exec")
            };
            let err = exec_command(&replaced);
            panic!("exec returned: {err}");
        }
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let mut copy = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "process::tests::exec_replaces_the_process_and_returns_only_its_error",
                "--test-threads=1",
            ])
            .env(EXEC_PROBE, d.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = copy.id();
        let status = copy.wait().unwrap();
        assert!(status.success(), "{status:?}");
        assert_eq!(
            fs::read_to_string(d.path().join("missing")).unwrap(),
            "NotFound"
        );
        let canonical = fs::canonicalize(d.path()).unwrap();
        assert_eq!(
            fs::read_to_string(d.path().join("exec")).unwrap(),
            format!("{pid} on {}\n", canonical.display()),
            "the shell ran as the copy itself, with the spec's environment and directory"
        );
    }

    #[test]
    fn the_first_executable_regular_file_on_path_is_found() {
        let d = tempfile::tempdir().unwrap();
        let dir = |name: &str| {
            let p = d.path().join(name);
            fs::create_dir_all(&p).unwrap();
            p
        };
        let file = |path: &Path, mode: u32| {
            fs::write(path, "#!/bin/sh\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        };
        let (plain, holder, real, first, linked) = (
            dir("plain"),
            dir("holder"),
            dir("real"),
            dir("first"),
            dir("linked"),
        );
        file(&plain.join("claude"), 0o644);
        fs::create_dir(holder.join("claude")).unwrap();
        file(&real.join("claude"), 0o755);
        file(&first.join("claude"), 0o755);
        symlink(real.join("claude"), linked.join("claude")).unwrap();
        let path = |dirs: &[&Path]| {
            let joined: Vec<String> = dirs.iter().map(|p| p.display().to_string()).collect();
            OsString::from(joined.join(":"))
        };
        // Neither a file without the execute bit nor a directory is a match; an empty entry
        // (the current directory, which holds no `claude`) is passed over.
        let p = path(&[&plain, &holder, Path::new(""), &real]);
        assert_eq!(find_on_path("claude", Some(&p)), Some(real.join("claude")));
        let p = path(&[&first, &real]);
        assert_eq!(
            find_on_path("claude", Some(&p)),
            Some(first.join("claude")),
            "PATH order"
        );
        let p = path(&[&linked]);
        assert_eq!(
            find_on_path("claude", Some(&p)),
            Some(linked.join("claude")),
            "a link to an executable counts, as itself"
        );
        assert_eq!(find_on_path("nothing-here", Some(&p)), None);
        assert_eq!(find_on_path("claude", None), None, "no PATH, nothing found");
        assert_eq!(find_on_path("", Some(&p)), None);
        assert_eq!(
            find_on_path("/bin/sh", Some(OsStr::new(""))),
            Some(PathBuf::from("/bin/sh")),
            "a name with a slash is taken as it is"
        );
        let unexecutable = plain.join("claude").display().to_string();
        assert_eq!(find_on_path(&unexecutable, Some(&p)), None);
    }

    #[test]
    fn the_exit_code_is_the_child_s_or_128_plus_the_signal() {
        assert_eq!(exit_code(ExitStatus::from_raw(0)), 0);
        assert_eq!(exit_code(ExitStatus::from_raw(3 << 8)), 3);
        assert_eq!(exit_code(ExitStatus::from_raw(libc::SIGTERM)), 143);
        assert_eq!(exit_code(ExitStatus::from_raw(libc::SIGINT)), 130);
        let _fork = fork_guard();
        let killed = Command::new("/bin/sh")
            .args(["-c", "kill -KILL $$"])
            .status()
            .unwrap();
        assert_eq!(exit_code(killed), 137);
    }

    #[test]
    fn the_system_spawner_runs_the_spec() {
        let _fork = fork_guard();
        let got =
            SystemSpawner.run_captured(&sh("printf ok"), Duration::from_secs(5), &Cancel::new());
        assert!(
            matches!(&got, Captured::Exited { code: Some(0), stdout, .. } if stdout == b"ok"),
            "{got:?}"
        );
    }

    #[test]
    fn the_scripted_spawner_replies_in_order_and_records_every_spec() {
        let s = ScriptedSpawner::new();
        let reply = Captured::Exited {
            code: Some(0),
            signal: None,
            stdout: b"{}".to_vec(),
            stderr: vec![],
        };
        s.push(Captured::TimedOut);
        s.push(reply.clone());
        let (one, two) = (
            sh("one"),
            SpawnSpec {
                cwd: Some("/w".into()),
                ..sh("two")
            },
        );
        let (timeout, quiet) = (Duration::from_secs(10), Cancel::new());
        assert_eq!(s.run_captured(&one, timeout, &quiet), Captured::TimedOut);
        assert_eq!(s.run_captured(&two, timeout, &quiet), reply);
        assert!(
            matches!(
                s.run_captured(&one, timeout, &quiet),
                Captured::SpawnFailed(_)
            ),
            "nothing scripted is a spawn failure"
        );
        s.push(Captured::TimedOut);
        let cancelled = Cancel::new();
        cancelled.request(15);
        assert_eq!(
            s.run_captured(&two, timeout, &cancelled),
            Captured::Interrupted(15),
            "a set token wins, as for a real spawn"
        );
        assert_eq!(
            s.run_captured(&one, timeout, &quiet),
            Captured::TimedOut,
            "and leaves the reply queued"
        );
        assert_eq!(
            s.specs(),
            vec![one.clone(), two.clone(), one.clone(), two, one]
        );
    }
```

The exec test runs a copy of this test binary filtered to itself, with `EXEC_PROBE` set: the copy
calls `exec_command`, which on failure may leave the calling process half-changed (std resets
signal dispositions and the working directory before `exec`), so it never runs in the test
process itself.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-provider --lib process::tests`
Expected: compile errors only: E0412/E0422 for `SpawnSpec` and `Captured`, E0425 for
`run_captured`, `capture`, `spawn_session`, `exec_command`, `find_on_path` and `exit_code`,
E0425/E0433 for `SystemSpawner` and `ScriptedSpawner`, and E0425 for the names the test
helpers expect from the module's imports (`OsString`, `PathBuf`, `Path`, `Duration`,
`Instant`, `thread`, `fs`, `Command`, `Stdio`, `ExitStatus`, `PoisonError`, `Cancel`).

- [ ] **Step 3: Implement**

In `crates/tagteam-provider/src/process.rs`, line 1 (`use std::io;`) becomes:

```rust
use std::collections::VecDeque;
use std::ffi::{CString, OsStr, OsString};
use std::fs;
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::cancel::Cancel;
```

`ProcessStamp` and the two `start_of` functions are unchanged; the Linux one keeps its fully
qualified `std::fs::read_to_string`. After the macOS `start_of`, before `#[cfg(test)]`, add:

```rust
/// What to run, and the environment and directory to run it in (§12.3 step 8, §12.5). The
/// child starts from this process's environment: `remove` is applied first, then `set`, so a
/// name in both ends up set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpawnSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub set: Vec<(OsString, OsString)>,
    pub remove: Vec<OsString>,
    pub cwd: Option<PathBuf>,
}

/// How a captured child ended (`run_captured`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    /// It exited, and both its pipes closed. `code` is its exit code, or `signal` the signal
    /// that ended it.
    Exited {
        code: Option<i32>,
        signal: Option<i32>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    /// It ran past the timeout and its group was killed, or it exited but its output stayed
    /// open past the drain grace (a grandchild held a pipe): half an output is never returned.
    TimedOut,
    /// The cancel token was set: its group was killed, or it was never spawned.
    Interrupted(i32),
    /// It could not be spawned, or its wait failed.
    SpawnFailed(String),
}

/// How often a captured child's wait looks at its exit, the deadline and the token.
const POLL: Duration = Duration::from_millis(10);
/// How long `run_captured` waits for the output pipes to close once the child has exited, as
/// `security`'s runner does: a grandchild that inherited a pipe keeps it open after the child.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// The `Command` for `spec`: its program, arguments, environment and directory.
fn command(spec: &SpawnSpec) -> Command {
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args);
    for name in &spec.remove {
        cmd.env_remove(name);
    }
    for (name, value) in &spec.set {
        cmd.env(name, value);
    }
    if let Some(dir) = &spec.cwd {
        cmd.current_dir(dir);
    }
    cmd
}

/// Non-interactive: its own process group, stdin null, stdout and stderr captured, the group
/// killed (`killpg`) on `timeout` or when `cancel` is set (polled every 10 ms). A token already
/// set spawns nothing (§12.5: the wait for the login check is a cancellation point).
pub fn run_captured(spec: &SpawnSpec, timeout: Duration, cancel: &Cancel) -> Captured {
    capture(spec, timeout, cancel, DRAIN_GRACE)
}

/// `run_captured`, with the drain grace a parameter so the tests need not wait 2 s.
fn capture(spec: &SpawnSpec, timeout: Duration, cancel: &Cancel, grace: Duration) -> Captured {
    if let Some(signal) = cancel.requested() {
        return Captured::Interrupted(signal);
    }
    let mut cmd = command(spec);
    // §14.1: a group of its own, so a Ctrl-C at the terminal never reaches it, and a timeout
    // or a cancel kills whatever it started too. Its stdin is null and its output is piped, so
    // the background group never stops it for touching the terminal.
    cmd.process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Captured::SpawnFailed(e.to_string()),
    };
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                kill_group(&mut child);
                return Captured::SpawnFailed(e.to_string());
            }
        }
        if let Some(signal) = cancel.requested() {
            kill_group(&mut child);
            return Captured::Interrupted(signal);
        }
        if Instant::now() >= deadline {
            kill_group(&mut child);
            return Captured::TimedOut;
        }
        thread::sleep(POLL);
    };
    // The child is reaped now, so its group id may name another group once its last member
    // goes: whatever still holds a pipe is left alone rather than killed by a stale id.
    let until = Instant::now() + grace;
    let output =
        collect(&out, until, cancel).and_then(|stdout| Ok((stdout, collect(&err, until, cancel)?)));
    match output {
        Ok((stdout, stderr)) => Captured::Exited {
            code: status.code(),
            signal: status.signal(),
            stdout,
            stderr,
        },
        Err(Unfinished::Late) => Captured::TimedOut,
        Err(Unfinished::Interrupted(signal)) => Captured::Interrupted(signal),
    }
}

/// Kills the child's whole group, then reaps the child. The child leads the group
/// (`process_group(0)`) and is not reaped yet, so its pid still names that group and no other.
/// A kill that fails leaves the child running, and `wait` would block for as long as it does,
/// so then it returns without waiting (as `security`'s runner does).
fn kill_group(child: &mut Child) {
    let group = child.id() as libc::pid_t;
    // SAFETY: killpg(2) takes two integers and touches no memory of ours.
    let sent = unsafe { libc::killpg(group, libc::SIGKILL) } == 0;
    if sent || child.kill().is_ok() {
        let _ = child.wait();
    }
}

/// Reads a pipe to its end on a thread and sends what it read. The thread is detached, so a
/// pipe that never closes costs one parked thread, not a hang.
fn drain<R: io::Read + Send + 'static>(pipe: Option<R>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut buf);
        }
        let _ = tx.send(buf);
    });
    rx
}

/// Why a pipe's bytes are not all there.
enum Unfinished {
    Late,
    Interrupted(i32),
}

/// The drained bytes, once the pipe closes by `until`. The wait is sliced, so the token is
/// checked here as in the child's own wait.
fn collect(
    rx: &mpsc::Receiver<Vec<u8>>,
    until: Instant,
    cancel: &Cancel,
) -> Result<Vec<u8>, Unfinished> {
    loop {
        let left = until.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left.min(POLL)) {
            Ok(bytes) => return Ok(bytes),
            Err(RecvTimeoutError::Disconnected) => return Err(Unfinished::Late),
            Err(RecvTimeoutError::Timeout) => {}
        }
        if let Some(signal) = cancel.requested() {
            return Err(Unfinished::Interrupted(signal));
        }
        if Instant::now() >= until {
            return Err(Unfinished::Late);
        }
    }
}

/// The session: tagteam's foreground process group, stdio inherited, and exactly `inherit`
/// passed without `FD_CLOEXEC` (cleared in `pre_exec`, Decision 3). Every other descriptor
/// tagteam holds is `O_CLOEXEC` (std's default), so no other child ever holds a reservation.
/// `spawn` returns once the child has exec'd, so a signal sent to its pid after that reaches
/// the launch command itself.
pub fn spawn_session(spec: &SpawnSpec, inherit: Option<RawFd>) -> io::Result<Child> {
    let mut cmd = command(spec);
    if let Some(fd) = inherit {
        // SAFETY: the closure runs in the forked child before exec, where only
        // async-signal-safe work is allowed: it makes two fcntl(2) calls on an integer and
        // reads errno, and allocates nothing.
        unsafe {
            cmd.pre_exec(move || keep_across_exec(fd));
        }
    }
    cmd.spawn()
}

/// Clears `FD_CLOEXEC` on `fd`, in the child (`spawn_session`).
fn keep_across_exec(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl(2) with F_GETFD takes and returns integers.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above, with F_SETFD.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The plain path (§12.1): `exec`; returns only the error. On success this process becomes the
/// launch command, with its pid, its terminal and its exit status.
pub fn exec_command(spec: &SpawnSpec) -> io::Error {
    command(spec).exec()
}

/// `PATH` lookup of `name`: the first executable regular file, as a shell finds it (§12.1).
/// A name with a `/` is not searched for, and an empty `PATH` entry is the current directory.
/// The result is absolute unless `name` itself was a relative path.
pub fn find_on_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    if name.contains('/') {
        let p = PathBuf::from(name);
        return executable_file(&p).then_some(p);
    }
    for dir in path?.as_bytes().split(|b| *b == b':') {
        let dir = if dir.is_empty() {
            Path::new(".")
        } else {
            Path::new(OsStr::from_bytes(dir))
        };
        let mut candidate = dir.join(name);
        if candidate.is_relative() {
            match std::env::current_dir() {
                Ok(cwd) => candidate = cwd.join(candidate),
                Err(_) => continue,
            }
        }
        if executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// A regular file (symlinks followed) this user may execute, as `access(X_OK)` decides.
fn executable_file(p: &Path) -> bool {
    if !fs::metadata(p).is_ok_and(|m| m.is_file()) {
        return false;
    }
    let Ok(c) = CString::new(p.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c` is a NUL-terminated path that outlives the call; access(2) only reads it.
    unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
}

/// §12.5: the child's code, `128 + signal` when a signal ended it.
pub fn exit_code(status: ExitStatus) -> i32 {
    match (status.code(), status.signal()) {
        (Some(code), _) => code,
        (None, Some(signal)) => 128 + signal,
        (None, None) => 1,
    }
}

/// The port engine tests script (Decision 10). `SystemSpawner` calls `run_captured`.
pub trait ProcessSpawner: Send + Sync {
    fn run_captured(&self, spec: &SpawnSpec, timeout: Duration, cancel: &Cancel) -> Captured;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemSpawner;

impl ProcessSpawner for SystemSpawner {
    fn run_captured(&self, spec: &SpawnSpec, timeout: Duration, cancel: &Cancel) -> Captured {
        run_captured(spec, timeout, cancel)
    }
}

/// Tests: replies in order, recording each spec. A token already set answers `Interrupted`, as
/// a real spawn does, and leaves the next reply queued; with no reply left the spawn fails, as
/// a missing launch command would.
#[derive(Default)]
pub struct ScriptedSpawner {
    replies: Mutex<VecDeque<Captured>>,
    specs: Mutex<Vec<SpawnSpec>>,
}

impl ScriptedSpawner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, c: Captured) {
        self.replies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(c);
    }

    pub fn specs(&self) -> Vec<SpawnSpec> {
        self.specs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl ProcessSpawner for ScriptedSpawner {
    fn run_captured(&self, spec: &SpawnSpec, _timeout: Duration, cancel: &Cancel) -> Captured {
        self.specs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(spec.clone());
        if let Some(signal) = cancel.requested() {
            return Captured::Interrupted(signal);
        }
        self.replies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| Captured::SpawnFailed("no reply scripted".into()))
    }
}
```

In `SystemSpawner::run_captured`, the unqualified `run_captured` is the module's free function:
a method is never in scope by its bare name.

In `crates/tagteam-provider/src/lib.rs`, `pub use process::ProcessStamp;` (line 36) becomes:

```rust
pub use process::{
    Captured, ProcessSpawner, ProcessStamp, ScriptedSpawner, SpawnSpec, SystemSpawner,
    exec_command, exit_code, find_on_path, run_captured, spawn_session,
};
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-provider --lib process::tests`
Expected: PASS, 22 tests (the 2 existing and 20 new), in about 5 s: the group-kill tests wait
1.5 s each for a grandchild that must not write.

Run: `cargo test -p tagteam-provider`, then
`cargo clippy -p tagteam-provider --all-targets --target x86_64-unknown-linux-gnu -- -D warnings`
Expected: PASS; no warnings. The new code has no platform branch, and every `libc` name it uses
exists on both targets. Its Linux behaviour (`/dev/fd` is `/proc/self/fd`, `/bin/sleep` and
`/usr/bin/env` exist under usrmerge) is first run by CI's ubuntu job, or by the Docker recipe.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider/src/process.rs crates/tagteam-provider/src/lib.rs
git commit -m "Spawn, capture, exec and find the launch command"
```

#### Cycle 3: the fake `claude`

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam/tests/run_cli.rs`:

```rust
//! `tagteam run` through the real binary (Task 12 on), and the fake `claude` those tests put
//! first on `PATH`. Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use common::{FakeCall, fake_claude, fake_claude_calls, path_with};
use serde_json::{Value, json};

/// The fake `claude` run on its own, as tagteam would find it: first on `PATH`, `HOME` under
/// `root`, in `root`, and nothing else in its environment.
fn fake(root: &Path, args: &[&str]) -> Command {
    let bin = fake_claude(root);
    let mut c = Command::new(bin.join("claude"));
    c.args(args)
        .env_clear()
        .env("HOME", root.join("home"))
        .env("PATH", path_with(&bin))
        .current_dir(root);
    c
}

/// Waits up to 10 s for `done` to hold of what the fake recorded in `out`.
fn wait_for(out: &Path, what: &str, done: impl Fn(&[FakeCall]) -> bool) -> Vec<FakeCall> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let calls = fake_claude_calls(out);
        if done(&calls) {
            return calls;
        }
        assert!(Instant::now() < deadline, "no {what}: {calls:?}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// Sends `signal` to `pid` alone, as `kill` does.
fn send(pid: u32, signal: libc::c_int) {
    // SAFETY: kill(2) reads no memory of ours. `pid` is a process this test started and has
    // not reaped, or that process's child, whose pid the fake recorded while it runs.
    let rc = unsafe { libc::kill(pid as libc::pid_t, signal) };
    assert_eq!(rc, 0, "kill: {}", std::io::Error::last_os_error());
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_fake_claude_records_its_arguments_environment_and_pid() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("calls");
    let mut child = fake(d.path(), &["a b", "", "--json"])
        .arg(OsStr::from_bytes(b"caf\xe9"))
        .env("FAKE_CLAUDE_OUT", &out)
        .env("MARK", "on")
        .spawn()
        .unwrap();
    let pid = child.id();
    assert_eq!(child.wait().unwrap().code(), Some(0));
    let calls = fake_claude_calls(&out);
    assert_eq!(calls.len(), 1, "{calls:?}");
    let call = &calls[0];
    assert_eq!(
        (call.pid, call.ppid, call.mode.as_str()),
        (pid, std::process::id(), "session")
    );
    assert_eq!(
        call.args,
        [
            OsString::from("a b"),
            OsString::from(""),
            OsString::from("--json"),
            OsString::from_vec(b"caf\xe9".to_vec()),
        ],
        "every argument as its bytes, one that is not UTF-8 included"
    );
    assert_eq!(call.cwd, fs::canonicalize(d.path()).unwrap());
    assert_eq!(call.env.get("MARK").map(String::as_str), Some("on"));
    assert_eq!(
        call.env.get("HOME").map(PathBuf::from),
        Some(d.path().join("home"))
    );
    assert!(call.ready);
    assert!(call.signals.is_empty());
    assert_eq!(call.exit, Some(0));
}

#[test]
fn the_fake_claude_exits_with_the_code_it_is_given() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("calls");
    let status = fake(d.path(), &[])
        .env("FAKE_CLAUDE_EXIT", "7")
        .env("FAKE_CLAUDE_OUT", &out)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(7));
    assert_eq!(fake_claude_calls(&out)[0].exit, Some(7));
}

#[test]
fn the_fake_claude_auth_status_is_logged_out_unless_a_login_is_given() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("calls");
    let reply = |vars: &[(&str, &str)]| {
        let mut c = fake(d.path(), &["auth", "status", "--json"]);
        c.env("FAKE_CLAUDE_OUT", &out);
        for (k, v) in vars {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        (
            o.status.code(),
            serde_json::from_slice::<Value>(&o.stdout).unwrap(),
        )
    };
    // Claude Code exits non-zero when logged out (Appendix A.7), and still names its config
    // home. Nothing here holds a credential, so no login is read from a profile either.
    let default_home = d.path().join("home/.claude");
    assert_eq!(
        reply(&[]),
        (
            Some(1),
            json!({"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty",
                   "configDirectory": default_home.to_str().unwrap()})
        )
    );
    let profile = d.path().join("profile");
    let profile = profile.to_str().unwrap();
    assert_eq!(
        reply(&[
            ("CLAUDE_CONFIG_DIR", profile),
            ("FAKE_CLAUDE_EMAIL", "a@x.co"),
            ("FAKE_CLAUDE_ORG", "org-1"),
        ]),
        (
            Some(0),
            json!({"loggedIn": true, "authMethod": "claude.ai", "apiProvider": "firstParty",
                   "configDirectory": profile, "email": "a@x.co", "orgId": "org-1",
                   "subscriptionType": "max"})
        )
    );
    let (_, plain) = reply(&[("FAKE_CLAUDE_EMAIL", "a@x.co")]);
    assert_eq!(
        plain["configDirectory"],
        d.path().join("home/.claude").to_str().unwrap(),
        "without CLAUDE_CONFIG_DIR, the default home"
    );
    assert!(plain.get("orgId").is_none(), "{plain}");
    assert_eq!(
        reply(&[
            ("FAKE_CLAUDE_AUTH", "{\"authMethod\": \"api_key\"}"),
            ("FAKE_CLAUDE_AUTH_EXIT", "3"),
        ]),
        (Some(3), json!({"authMethod": "api_key"}))
    );
    let calls = fake_claude_calls(&out);
    assert_eq!(calls.len(), 4);
    assert!(
        calls
            .iter()
            .all(|c| c.mode == "auth" && c.args == ["auth", "status", "--json"] && !c.ready),
        "{calls:?}"
    );
}

#[test]
fn the_fake_claude_auth_status_takes_as_long_as_it_is_told() {
    let d = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let out = fake(d.path(), &["auth", "status"])
        .env("FAKE_CLAUDE_AUTH_SLEEP", "0.3")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(started.elapsed() >= Duration::from_millis(300));
}

#[test]
fn a_graceful_exit_removes_the_fake_s_session_record_and_a_kill_leaves_it() {
    // Claude Code removes its record on SIGINT, SIGTERM and SIGHUP, and SIGKILL leaves it
    // behind (§12.6).
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("calls");
    let profile = d.path().join("profile");
    let start = || {
        fake(d.path(), &[])
            .env("CLAUDE_CONFIG_DIR", &profile)
            .env("FAKE_CLAUDE_OUT", &out)
            .env("FAKE_CLAUDE_RECORD", "interactive")
            .env("FAKE_CLAUDE_SLEEP", "30")
            .spawn()
            .unwrap()
    };
    let mut child = start();
    let pid = child.id();
    wait_for(&out, "running session", |c| {
        c.iter().any(|c| c.pid == pid && c.ready)
    });
    let record = profile.join(format!("sessions/{pid}.json"));
    let v: Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(v["pid"].as_u64(), Some(u64::from(pid)), "{v}");
    assert_eq!(v["kind"], "interactive");
    assert!(
        v["startedAt"]
            .as_i64()
            .is_some_and(|ms| ms > 1_700_000_000_000)
    );
    assert_eq!(mode(&profile.join("sessions")), 0o700);
    send(pid, libc::SIGTERM);
    assert_eq!(child.wait().unwrap().code(), Some(143));
    assert!(!record.exists(), "a graceful exit removes it");
    let call = fake_claude_calls(&out)
        .into_iter()
        .find(|c| c.pid == pid)
        .unwrap();
    assert_eq!((call.signals, call.exit), (vec![15], Some(143)));

    let mut child = start();
    let pid = child.id();
    wait_for(&out, "running session", |c| {
        c.iter().any(|c| c.pid == pid && c.ready)
    });
    send(pid, libc::SIGKILL);
    assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGKILL));
    assert!(
        profile.join(format!("sessions/{pid}.json")).exists(),
        "SIGKILL leaves it behind"
    );
}

#[test]
fn with_continue_the_fake_claude_records_each_signal_and_runs_on() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("calls");
    let mut child = fake(d.path(), &[])
        .env("FAKE_CLAUDE_OUT", &out)
        .env("FAKE_CLAUDE_SLEEP", "30")
        .env("FAKE_CLAUDE_ON_SIGNAL", "continue")
        .spawn()
        .unwrap();
    wait_for(&out, "running session", |c| c.first().is_some_and(|c| c.ready));
    send(child.id(), libc::SIGINT);
    wait_for(&out, "SIGINT", |c| c[0].signals == [2]);
    send(child.id(), libc::SIGTERM);
    wait_for(&out, "SIGTERM", |c| c[0].signals == [2, 15]);
    assert!(child.try_wait().unwrap().is_none(), "still running");
    send(child.id(), libc::SIGKILL);
    child.wait().unwrap();
}

#[test]
fn with_a_hold_file_the_fake_claude_runs_until_it_exists() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("calls");
    let hold = d.path().join("hold");
    let mut child = fake(d.path(), &[])
        .env("FAKE_CLAUDE_OUT", &out)
        .env("FAKE_CLAUDE_HOLD", &hold)
        .spawn()
        .unwrap();
    wait_for(&out, "running session", |c| c.first().is_some_and(|c| c.ready));
    thread::sleep(Duration::from_millis(300));
    assert!(child.try_wait().unwrap().is_none(), "held");
    fs::write(&hold, b"").unwrap();
    let started = Instant::now();
    assert_eq!(child.wait().unwrap().code(), Some(0));
    assert!(started.elapsed() < Duration::from_secs(2), "it let go at once");
    assert_eq!(fake_claude_calls(&out)[0].exit, Some(0));
}

#[test]
fn the_fake_claude_auth_status_follows_its_config_home() {
    // As Claude Code's does (Appendix A.7): a credential and an `oauthAccount` make a claude.ai
    // login, and an `apiKeyHelper` in the settings wins over it.
    let d = tempfile::tempdir().unwrap();
    let profile = d.path().join("profile");
    fs::create_dir_all(&profile).unwrap();
    let spelling = profile.to_str().unwrap();
    let reply = || {
        let o = fake(d.path(), &["auth", "status", "--json"])
            .env("CLAUDE_CONFIG_DIR", &profile)
            .output()
            .unwrap();
        (
            o.status.code(),
            serde_json::from_slice::<Value>(&o.stdout).unwrap(),
        )
    };
    let config = json!({"oauthAccount": {"emailAddress": "a@x.co", "organizationUuid": "org-1"}});
    fs::write(
        profile.join(".claude.json"),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();
    assert_eq!(reply().0, Some(1), "no credential: logged out");
    fs::write(profile.join(".credentials.json"), "{}").unwrap();
    assert_eq!(
        reply(),
        (
            Some(0),
            json!({"loggedIn": true, "authMethod": "claude.ai", "apiProvider": "firstParty",
                   "configDirectory": spelling, "email": "a@x.co", "orgId": "org-1",
                   "subscriptionType": "max"})
        )
    );
    fs::write(profile.join("settings.json"), r#"{"apiKeyHelper": "~/bin/key"}"#).unwrap();
    assert_eq!(
        reply(),
        (
            Some(0),
            json!({"loggedIn": true, "authMethod": "api_key_helper", "apiProvider": "firstParty",
                   "apiKeySource": "apiKeyHelper", "configDirectory": spelling})
        )
    );
}

#[test]
fn the_fake_claude_rotates_its_profile_credential_privately() {
    let d = tempfile::tempdir().unwrap();
    let profile = d.path().join("profile");
    fs::create_dir_all(&profile).unwrap();
    let rotated = json!({"claudeAiOauth": {"refreshToken": "rt-next"}}).to_string();
    let status = fake(d.path(), &[])
        .env("CLAUDE_CONFIG_DIR", &profile)
        .env("FAKE_CLAUDE_ROTATE", &rotated)
        .status()
        .unwrap();
    assert!(status.success());
    let file = profile.join(".credentials.json");
    assert_eq!(fs::read_to_string(&file).unwrap(), rotated);
    assert_eq!(mode(&file), 0o600);
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --features test-support --test run_cli`
Expected: compile error E0432: unresolved imports `common::FakeCall`, `common::fake_claude`,
`common::fake_claude_calls`, `common::path_with`.

- [ ] **Step 3: Implement**

In `crates/tagteam/tests/common/mod.rs`, add to the imports (keeping M4a Task 14's, which
already bring in `PathBuf`):

```rust
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
```

and append:

```rust
/// Decision 11's fake `claude`: a `/bin/sh` script, so no test ever runs the real one. See
/// `fake_claude` for the variables that steer it.
const FAKE_CLAUDE: &str = r##"#!/bin/sh
# tagteam's fake `claude` for the CLI tests (tests/common/mod.rs, `fake_claude`). Every
# FAKE_CLAUDE_* variable is optional; nothing here reaches the network or the real HOME.
me=$$
home=${CLAUDE_CONFIG_DIR:-$HOME/.claude}
if [ -n "$CLAUDE_CONFIG_DIR" ]; then config=$CLAUDE_CONFIG_DIR/.claude.json; else config=$HOME/.claude.json; fi
record=
sleeper=

note() {
  if [ -n "$FAKE_CLAUDE_OUT" ]; then
    printf '%s %s %s\n' "$me" "$1" "$2" >> "$FAKE_CLAUDE_OUT"
  fi
}

finish() {
  if [ -n "$sleeper" ]; then kill "$sleeper" 2>/dev/null; fi
  if [ -n "$record" ]; then rm -f "$record"; fi
  note exit "$1"
  exit "$1"
}

caught() {
  note signal "$1"
  if [ "$FAKE_CLAUDE_ON_SIGNAL" != continue ]; then finish $((128 + $1)); fi
}
trap 'caught 1' HUP
trap 'caught 2' INT
trap 'caught 15' TERM

pause() {
  sleep "$1" &
  sleeper=$!
  while :; do
    wait "$sleeper"
    if [ "$?" -le 128 ] || ! kill -0 "$sleeper" 2>/dev/null; then break; fi
  done
  sleeper=
}

# A string field of the config's `oauthAccount`, as Claude Code pretty-prints it.
field() {
  sed -n "s/.*\"$1\": *\"\([^\"]*\)\".*/\1/p" "$config" 2>/dev/null | head -n 1
}

mode=session
if [ "$1" = auth ] && [ "$2" = status ]; then mode=auth; fi
note call "$mode"
note ppid "$PPID"
note cwd "$(pwd -P)"
for a in "$@"; do note arg "$a"; done
if [ -n "$FAKE_CLAUDE_OUT" ]; then
  /usr/bin/env | while IFS= read -r line; do note env "$line"; done
fi

if [ "$mode" = auth ]; then
  if [ -n "$FAKE_CLAUDE_AUTH_SLEEP" ]; then pause "$FAKE_CLAUDE_AUTH_SLEEP"; fi
  if [ -n "$FAKE_CLAUDE_AUTH" ]; then
    printf '%s\n' "$FAKE_CLAUDE_AUTH"
    finish "${FAKE_CLAUDE_AUTH_EXIT:-0}"
  fi
  if grep -q '"apiKeyHelper"' "$home/settings.json" 2>/dev/null; then
    printf '{"loggedIn":true,"authMethod":"api_key_helper","apiProvider":"firstParty","apiKeySource":"apiKeyHelper","configDirectory":"%s"}\n' "$home"
    finish "${FAKE_CLAUDE_AUTH_EXIT:-0}"
  fi
  email=$FAKE_CLAUDE_EMAIL
  org=$FAKE_CLAUDE_ORG
  if [ -z "$email" ] && [ -f "$home/.credentials.json" ]; then
    email=$(field emailAddress)
    org=$(field organizationUuid)
  fi
  if [ -n "$email" ]; then
    if [ -n "$org" ]; then org=",\"orgId\":\"$org\""; fi
    printf '{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","configDirectory":"%s","email":"%s"%s,"subscriptionType":"max"}\n' \
      "$home" "$email" "$org"
    finish "${FAKE_CLAUDE_AUTH_EXIT:-0}"
  fi
  printf '{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty","configDirectory":"%s"}\n' "$home"
  finish "${FAKE_CLAUDE_AUTH_EXIT:-1}"
fi

if [ -n "$FAKE_CLAUDE_RECORD" ]; then
  mkdir -p "$home/sessions" && chmod 700 "$home/sessions"
  started=$(LC_ALL=C TZ=UTC ps -o lstart= -p "$me" 2>/dev/null | sed 's/^ *//;s/ *$//')
  proc=
  if [ -n "$started" ]; then proc=",\"procStart\":\"$started\""; fi
  record="$home/sessions/$me.json"
  printf '{"pid":%s,"sessionId":"fake-%s","cwd":"%s","startedAt":%s000,"kind":"%s"%s}\n' \
    "$me" "$me" "$(pwd -P)" "$(date +%s)" "$FAKE_CLAUDE_RECORD" "$proc" > "$record"
fi
if [ -n "$FAKE_CLAUDE_ROTATE" ]; then
  (umask 077 && printf '%s' "$FAKE_CLAUDE_ROTATE" > "$home/.credentials.json.fake" \
    && mv -f "$home/.credentials.json.fake" "$home/.credentials.json")
fi
note ready
if [ -n "$FAKE_CLAUDE_HOLD" ]; then
  n=0
  while [ ! -e "$FAKE_CLAUDE_HOLD" ] && [ "$n" -lt 600 ]; do
    pause 0.05
    n=$((n + 1))
  done
elif [ -n "$FAKE_CLAUDE_SLEEP" ]; then
  pause "$FAKE_CLAUDE_SLEEP"
fi
finish "${FAKE_CLAUDE_EXIT:-0}"
"##;

/// Writes Decision 11's fake `claude` to `<root>/bin/claude` (0755) and returns `<root>/bin`,
/// to put first on `PATH` (`path_with`). Its config home is `CLAUDE_CONFIG_DIR`, else
/// `$HOME/.claude`. This is the one list of the variables that steer it, each optional; Tasks
/// 12 and 13 use no others:
/// - `FAKE_CLAUDE_OUT`: a file every run appends its record to (`fake_claude_calls`): its pid,
///   parent, directory, arguments and environment, then `ready` once a session runs, the
///   signals it caught, and its exit.
/// - `FAKE_CLAUDE_EXIT`: the session's exit code (default 0).
/// - `FAKE_CLAUDE_SLEEP`: seconds the session runs before it exits, when `FAKE_CLAUDE_HOLD` is
///   unset.
/// - `FAKE_CLAUDE_HOLD`: a path; the session runs until it exists, for at most 30 s.
/// - `FAKE_CLAUDE_RECORD`: a session-record kind (`interactive`, `bg`, `daemon`). The session
///   writes its record `<home>/sessions/<pid>.json` with it (Appendix A.7: `pid`, `startedAt`,
///   `kind`, and `procStart` when `ps` answers), and a graceful exit removes it. SIGKILL leaves
///   it behind, as for Claude Code.
/// - `FAKE_CLAUDE_ROTATE`: credential JSON the session writes to `<home>/.credentials.json`,
///   0600, by rename, at its start: a rotation of the profile's credential.
/// - `FAKE_CLAUDE_ON_SIGNAL`: `continue` records SIGHUP, SIGINT and SIGTERM and keeps running;
///   otherwise each is recorded and ends the session gracefully with 128 + its number. SIGQUIT
///   keeps its default action.
/// - `FAKE_CLAUDE_AUTH`: `claude auth status`'s reply, verbatim, exiting with
///   `FAKE_CLAUDE_AUTH_EXIT` (default 0).
/// - Without `FAKE_CLAUDE_AUTH`, `auth status` answers from its config home, as Claude Code's
///   does, with `configDirectory` the config home it was given:
///   - `api_key_helper` when `<home>/settings.json` names an `apiKeyHelper`;
///   - else a `claude.ai` login as `FAKE_CLAUDE_EMAIL` (and `FAKE_CLAUDE_ORG`'s `orgId`), or,
///     without them, as the config's `oauthAccount` email and org when `<home>/.credentials.json`
///     exists;
///   - else logged out, exiting 1.
/// - `FAKE_CLAUDE_AUTH_SLEEP`: seconds `auth status` takes before it answers.
///
/// Paths and values go into JSON unescaped, so tests keep them free of quotes and
/// backslashes, and records are lines, so no argument or variable may hold a newline.
pub fn fake_claude(root: &Path) -> PathBuf {
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let path = bin.join("claude");
    fs::write(&path, FAKE_CLAUDE).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// `bin`, then this test process's own `PATH`: what puts the fake `claude` first.
pub fn path_with(bin: &Path) -> OsString {
    let mut path = bin.as_os_str().to_owned();
    if let Some(rest) = std::env::var_os("PATH") {
        path.push(":");
        path.push(rest);
    }
    path
}

/// One run of the fake `claude`, as it recorded itself in `FAKE_CLAUDE_OUT`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FakeCall {
    pub pid: u32,
    /// `auth` for `claude auth status`, else `session`.
    pub mode: String,
    pub ppid: u32,
    /// Its working directory, symlinks resolved.
    pub cwd: PathBuf,
    /// Its arguments, as the bytes it was given.
    pub args: Vec<OsString>,
    pub env: BTreeMap<String, String>,
    /// The session got as far as running: its record and rotation are written.
    pub ready: bool,
    /// The SIGHUP, SIGINT and SIGTERM it caught, by number, in order.
    pub signals: Vec<i32>,
    pub exit: Option<i32>,
}

/// Every run recorded in `out`, in the order they started; a missing file is no runs. Each
/// line is `<pid> <key> <value>`, so runs that overlap never mix. Lines are read as bytes: an
/// argument keeps its own, and every other value is read as UTF-8, lossily.
pub fn fake_claude_calls(out: &Path) -> Vec<FakeCall> {
    let bytes = fs::read(out).unwrap_or_default();
    let mut calls: Vec<FakeCall> = Vec::new();
    for line in bytes.split(|b| *b == b'\n') {
        let mut parts = line.splitn(3, |b| *b == b' ');
        let (Some(pid), Some(key)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(pid) = String::from_utf8_lossy(pid).parse::<u32>() else {
            continue;
        };
        let raw = parts.next().unwrap_or(b"");
        let value = String::from_utf8_lossy(raw);
        let key = String::from_utf8_lossy(key);
        if key == "call" {
            calls.push(FakeCall {
                pid,
                mode: value.into_owned(),
                ..FakeCall::default()
            });
            continue;
        }
        let Some(call) = calls.iter_mut().rev().find(|c| c.pid == pid) else {
            continue;
        };
        match key.as_ref() {
            "ppid" => call.ppid = value.parse().unwrap_or(0),
            "cwd" => call.cwd = PathBuf::from(OsString::from_vec(raw.to_vec())),
            "arg" => call.args.push(OsString::from_vec(raw.to_vec())),
            "env" => {
                if let Some((k, v)) = value.split_once('=') {
                    call.env.insert(k.to_owned(), v.to_owned());
                }
            }
            "ready" => call.ready = true,
            "signal" => call.signals.extend(value.parse::<i32>().ok()),
            "exit" => call.exit = value.parse().ok(),
            _ => {}
        }
    }
    calls
}
```

The script is POSIX: it runs under macOS's `/bin/sh` (bash) and Linux's (dash). Its sleep runs
in the background with `wait`, because a shell runs a trap only once its foreground command
ends: `wait` returns at once on a trapped signal. A background job of a non-interactive shell
ignores SIGINT and SIGQUIT, so a terminal's Ctrl-C (SIGINT to the whole group) reaches the
fake's trap and not its `sleep`, which `finish` then kills, and a Ctrl-\ ends the script alone.
`FAKE_CLAUDE_HOLD` polls in the same 50 ms background sleeps. `ps` failing (as it does under
the agent sandbox) only drops `procStart`, as for Claude Code (A.7), which leaves the record to
M4a's `record_is_live` heuristic: the fake's arguments mention `claude`, so it counts as live.

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam --features test-support --test run_cli`
Expected: PASS, 9 tests.

Run: `cargo test -p tagteam --features test-support`
Expected: PASS. Every other test file compiles the new helpers unused; `common`'s
`#![allow(dead_code)]` covers them.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam/tests/common/mod.rs crates/tagteam/tests/run_cli.rs
git commit -m "Add a fake claude for the run tests"
```

---

### Task 4: Launch reservations

§12.5's launch reservation is what keeps an account session-owned between `claude`'s start and
its session record, and after tagteam itself dies: "The parent creates it under a temporary
name, takes `flock(LOCK_EX)` on it, and renames it into place, so it never appears unlocked. The
locked fd is passed to `claude` without `O_CLOEXEC`, so the kernel holds the lock for as long as
the parent **or** the child lives." M4a reads reservations (`profile::launch_reservations`, each
probed with `flock::probe_lock`); this task creates, unlinks and removes them. Task 10 calls
`remove_dead_reservations` and `create` under `MutationGuard` and the account lock, and Task 12
passes `fd()` to `spawn_session`.

**Readings of the spec this task commits to:**
- **The temporary name** is `.<pid>.lock.tagteam-<pid>-<rand>`, after the atomic writer's
  convention (`.<name>.tagteam-<pid>-<rand>`). It never ends in `.lock`, so
  `launch_reservations`, which counts only `*.lock`, never sees one. It is created 0600 with
  `O_EXCL`, in `<profile>/.tagteam-launch/` (0700, created when missing).
- **Locked before it is visible.** `flock(LOCK_EX | LOCK_NB)` on the new file, then the contents,
  then the rename. A refusal of the lock is an error, never a wait (B.37): nothing else can hold
  a file this process just created.
- **Contents:** `{"pid": <pid>, "startedAt": <ms>}` and a newline, for `doctor` and a person
  reading the directory. Liveness never reads them.
- **The rename never lands on a live reservation.** `<pid>.lock` can already exist only if an
  earlier process had this pid. If that file is dead, the rename replaces it. If it is held, by
  an orphaned `claude` of that earlier process, replacing it would hide a running session, which
  is the unsafe direction. `create` then fails with `AlreadyExists` and leaves it alone, and
  Task 10's `launch` refuses on it as `launch-unreachable`, naming the file (Interface
  Contract); a later `run` gets a new pid. The probe and the rename are race-free because every
  create and every removal holds `MutationGuard` and the account lock, the caller's duty
  (§12.5).
- **Any failure after the temporary file exists removes it.** A create killed in that window
  leaves the temporary file behind; `remove_dead_reservations` removes it once it is free and
  does not report it, since it was never a reservation.
- **`unlink`** removes the path first and then closes this process's fd, so the path disappears
  while still locked. A file already gone is not an error. There is no unlink on `Drop`: a
  dropped reservation stays and goes dead with its last holder, which is §12.5's path for an exit
  handling that fails ("the reservation dies with this process").
- **`remove_dead_reservations`** removes every `*.lock` (the same test as `launch_reservations`)
  that `probe_lock` finds `Free`, and returns those paths in name order. `Held` is kept, `Missing`
  (removed meanwhile) is skipped. A listing or probe error is returned: a reservation that cannot
  be probed may hide a live session (§10.3).
- **Review Focus 1's mechanism:** a reservation whose fd a session child inherited stays `Held`
  after tagteam's own handle is gone, and goes `Free` when the child exits. A child spawned
  without the fd (any other child, Decision 3) never holds it.

**Files:**
- Create: `crates/tagteam-provider/src/reservation.rs`
- Modify: `crates/tagteam-provider/src/lib.rs` (`pub mod reservation;` after `pub mod read;`;
  `pub use reservation::{LaunchReservation, remove_dead_reservations};` after
  `pub use read::{Read, ReadError};`)
- Test: `crates/tagteam-provider/src/reservation.rs` (in-module, as `flock.rs` and `profile.rs`)

**Interfaces:**
- Consumes:
  - M4a Task 6: `flock::{LockProbe, probe_lock}`; M4a Task 7: `profile::LAUNCH_DIR` and, in
    tests, `profile::launch_reservations`. Neither is on this branch yet; written against M4a's
    contract and the code in its plan.
  - Task 3: `process::{SpawnSpec, spawn_session}` (tests)
  - Existing: `atomic::ensure_private_dir`, `flock::FlockGuard::try_lock` (tests),
    `read::Read` (tests), `crate::FORK_GUARD`, `fastrand`, `serde_json`, `libc::flock`
- Produces (Interface Contract, `src/reservation.rs`, re-exported at the crate root):
  - `LaunchReservation` (`Debug`), with `create(profile: &Path) -> io::Result<LaunchReservation>`,
    `path(&self) -> &Path`, `fd(&self) -> RawFd`, `unlink(self) -> io::Result<()>`
  - `remove_dead_reservations(profile: &Path) -> io::Result<Vec<PathBuf>>`

**Spec:**
- §12.5 "Launch reservation": `<profile>/.tagteam-launch/<pid>.lock` holding the parent's pid
  and `startedAt`; created under a temporary name, locked, renamed, so never unlocked; the fd
  passed to `claude` without `O_CLOEXEC`; live while locked, tested non-blocking; unlinked after
  exit handling; created, and a dead one removed, only under `MutationGuard` and the account
  lock.
- §12.6: reservations need no pid; they are live while their file is locked.
- §5: the path; files 0600, directories 0700.
- B.37: a reservation is only ever tested non-blocking. B.46: it stays live while the parent or
  `claude` lives.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-provider/src/reservation.rs` with the tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{MutexGuard, PoisonError};

    use crate::flock::FlockGuard;
    use crate::process::{SpawnSpec, spawn_session};
    use crate::profile::launch_reservations;
    use crate::read::Read;

    fn fork_guard() -> MutexGuard<'static, ()> {
        crate::FORK_GUARD
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn launch_dir(profile: &Path) -> PathBuf {
        profile.join(LAUNCH_DIR)
    }

    /// `sleep 30` as a session child.
    fn sleeper() -> SpawnSpec {
        SpawnSpec {
            program: "/bin/sleep".into(),
            args: vec!["30".into()],
            ..SpawnSpec::default()
        }
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_reservation_is_named_for_this_process_and_records_it() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let r = LaunchReservation::create(d.path()).unwrap();
        let pid = std::process::id();
        assert_eq!(r.path(), launch_dir(d.path()).join(format!("{pid}.lock")));
        let v: serde_json::Value = serde_json::from_slice(&fs::read(r.path()).unwrap()).unwrap();
        assert_eq!(v["pid"], pid);
        assert!(
            v["startedAt"]
                .as_i64()
                .is_some_and(|ms| ms > 1_700_000_000_000),
            "{v}"
        );
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(r.path()), 0o600);
        assert_eq!(mode(&launch_dir(d.path())), 0o700);
        assert_eq!(
            names_in(&launch_dir(d.path())),
            [format!("{pid}.lock")],
            "no temporary file"
        );
    }

    #[test]
    fn a_reservation_is_never_visible_unlocked() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let profile = d.path();
        let looked = Cell::new(false);
        let r = LaunchReservation::create_with(profile, &|temp| {
            // Before the rename: already locked, and no reservation to anyone who lists them.
            assert_eq!(probe_lock(temp).unwrap(), LockProbe::Held);
            assert!(
                temp.extension().is_none_or(|x| x != "lock"),
                "{}",
                temp.display()
            );
            assert!(matches!(launch_reservations(profile), Read::Present(v) if v.is_empty()));
            looked.set(true);
        })
        .unwrap();
        assert!(looked.get());
        let Read::Present(found) = launch_reservations(profile) else {
            panic!("the reservations list");
        };
        assert_eq!(found, vec![(r.path().to_path_buf(), LockProbe::Held)]);
    }

    #[test]
    fn a_reservation_is_live_while_its_holder_lives() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let r = LaunchReservation::create(d.path()).unwrap();
        let path = r.path().to_path_buf();
        assert_eq!(probe_lock(&path).unwrap(), LockProbe::Held);
        assert!(
            remove_dead_reservations(d.path()).unwrap().is_empty(),
            "a held one is kept"
        );
        assert!(path.exists());
        drop(r);
        assert_eq!(
            probe_lock(&path).unwrap(),
            LockProbe::Free,
            "dead once its holder is"
        );
        assert!(path.exists(), "dropping never unlinks: exit handling does");
    }

    #[test]
    fn the_session_s_inherited_descriptor_keeps_it_live_after_tagteam_lets_go() {
        // Review Focus 1's mechanism: a `tagteam` killed while `claude` runs closes its own
        // descriptor, and `claude`'s inherited copy keeps the lock.
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let r = LaunchReservation::create(d.path()).unwrap();
        let path = r.path().to_path_buf();
        let mut child = spawn_session(&sleeper(), Some(r.fd())).unwrap();
        drop(r);
        assert_eq!(probe_lock(&path).unwrap(), LockProbe::Held);
        assert!(remove_dead_reservations(d.path()).unwrap().is_empty());
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(probe_lock(&path).unwrap(), LockProbe::Free);
        assert_eq!(
            remove_dead_reservations(d.path()).unwrap(),
            vec![path.clone()]
        );
        assert!(!path.exists());
    }

    #[test]
    fn a_child_spawned_without_the_descriptor_never_holds_the_reservation() {
        // Decision 3: no other child (a `security` call, the validation spawn) keeps a launch
        // alive after tagteam.
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let r = LaunchReservation::create(d.path()).unwrap();
        let path = r.path().to_path_buf();
        let mut child = spawn_session(&sleeper(), None).unwrap();
        drop(r);
        let probed = probe_lock(&path).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(probed, LockProbe::Free);
    }

    #[test]
    fn dead_reservations_and_leftover_temporary_files_go_and_nothing_else() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let dir = launch_dir(d.path());
        fs::create_dir_all(&dir).unwrap();
        // A launch whose parent and `claude` are both gone.
        fs::write(dir.join("100.lock"), b"{\"pid\":100}\n").unwrap();
        // A create that died before its rename.
        fs::write(dir.join(".7.lock.tagteam-7-0000abcd"), b"").unwrap();
        fs::write(dir.join("notes.txt"), b"").unwrap();
        let held = FlockGuard::try_lock(&dir.join("200.lock"))
            .unwrap()
            .unwrap();
        assert_eq!(
            remove_dead_reservations(d.path()).unwrap(),
            vec![dir.join("100.lock")]
        );
        assert_eq!(names_in(&dir), ["200.lock", "notes.txt"]);
        drop(held);
    }

    #[test]
    fn a_profile_without_reservations_has_none_to_remove_and_gains_no_directory() {
        let d = tempfile::tempdir().unwrap();
        assert!(remove_dead_reservations(d.path()).unwrap().is_empty());
        assert!(!launch_dir(d.path()).exists());
    }

    #[test]
    fn unlink_removes_the_file_and_tolerates_one_already_gone() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let r = LaunchReservation::create(d.path()).unwrap();
        let path = r.path().to_path_buf();
        r.unlink().unwrap();
        assert!(!path.exists());
        assert!(matches!(launch_reservations(d.path()), Read::Present(v) if v.is_empty()));
        let r = LaunchReservation::create(d.path()).unwrap();
        fs::remove_file(r.path()).unwrap();
        r.unlink().unwrap();
    }

    #[test]
    fn a_dead_file_under_this_pid_is_replaced_and_a_live_one_refuses() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let dir = launch_dir(d.path());
        fs::create_dir_all(&dir).unwrap();
        let mine = dir.join(format!("{}.lock", std::process::id()));
        fs::write(&mine, b"stale").unwrap();
        let r = LaunchReservation::create(d.path()).unwrap();
        assert_eq!(probe_lock(&mine).unwrap(), LockProbe::Held);
        assert_ne!(fs::read(&mine).unwrap(), b"stale");
        drop(r);
        // An earlier process with this pid whose orphaned `claude` still holds its reservation.
        let orphan = FlockGuard::try_lock(&mine).unwrap().unwrap();
        let before = fs::read(&mine).unwrap();
        let err = LaunchReservation::create(d.path()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&mine).unwrap(), before, "never replaced");
        assert_eq!(
            names_in(&dir),
            [format!("{}.lock", std::process::id())],
            "no temporary file is left"
        );
        drop(orphan);
    }
}
```

and in `crates/tagteam-provider/src/lib.rs`, after `pub mod read;`:

```rust
pub mod reservation;
```

Every test that creates a reservation holds `FORK_GUARD`: they fork, or drop a lock and probe it
again, and a child forked meanwhile by another test would hold a duplicate of the lock.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-provider --lib reservation::tests`
Expected: compile errors only: E0433/E0412 for `LaunchReservation`, E0425 for
`remove_dead_reservations`, and E0425/E0433 for the names the tests expect from the module's
imports (`probe_lock`, `LockProbe`, `LAUNCH_DIR`, `fs`, `io`, `Path`, `PathBuf`).

- [ ] **Step 3: Implement**

Put this above the tests in `crates/tagteam-provider/src/reservation.rs`:

```rust
//! §12.5's launch reservation: a locked file in the profile saying that a launch, or the
//! `claude` it started, is alive. It is live while its file is locked, and the kernel holds the
//! lock for as long as tagteam or `claude`, which inherits the fd (Decision 3), lives. Others
//! only ever test it, non-blocking (`flock::probe_lock`, B.37); no pid is consulted.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::atomic::ensure_private_dir;
use crate::flock::{LockProbe, probe_lock};
use crate::profile::LAUNCH_DIR;

/// What marks a reservation still being created: its temporary name is
/// `.<pid>.lock.tagteam-<pid>-<rand>`, after the atomic writer's convention. It never ends in
/// `.lock`, so `profile::launch_reservations` never counts one.
const TEMP_MARK: &str = ".lock.tagteam-";

/// §12.5's launch reservation: `<profile>/.tagteam-launch/<pid>.lock`, created under a
/// temporary name, `flock`ed, then renamed into place, so it never appears unlocked.
#[derive(Debug)]
pub struct LaunchReservation {
    /// `O_CLOEXEC` in tagteam (std's default): only the session spawn passes it on.
    file: File,
    path: PathBuf,
}

impl LaunchReservation {
    /// Holds `MutationGuard` and the account lock: the caller's duty, not checked here.
    pub fn create(profile: &Path) -> io::Result<LaunchReservation> {
        Self::create_with(profile, &|_| {})
    }

    /// `create`, calling `before_rename` with the temporary file once it is locked and
    /// written: the moment the tests look at.
    fn create_with(profile: &Path, before_rename: &dyn Fn(&Path)) -> io::Result<LaunchReservation> {
        let dir = profile.join(LAUNCH_DIR);
        ensure_private_dir(&dir)?;
        let pid = std::process::id();
        let name = format!("{pid}.lock");
        let path = dir.join(&name);
        let (temp, file) = create_temp(&dir, &name, pid)?;
        let placed = lock(&file)
            .and_then(|()| (&file).write_all(&contents(pid)))
            .and_then(|()| {
                before_rename(&temp);
                never_over_a_live_one(&path)
            })
            .and_then(|()| fs::rename(&temp, &path));
        if let Err(e) = placed {
            let _ = fs::remove_file(&temp);
            return Err(e);
        }
        Ok(LaunchReservation { file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The locked descriptor, for `process::spawn_session` to pass to `claude`.
    pub fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    /// Unlinks the file (exit handling's last step). The lock goes when the last holder exits:
    /// this process's fd closes here, and a descendant of `claude` still holding the inherited
    /// one no longer matters, since liveness is judged by the path. A file already gone is not
    /// an error.
    pub fn unlink(self) -> io::Result<()> {
        match fs::remove_file(&self.path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// A new temporary file in `dir`, 0600, created with `O_EXCL`, so no file that already exists
/// is ever reused as a reservation.
fn create_temp(dir: &Path, name: &str, pid: u32) -> io::Result<(PathBuf, File)> {
    loop {
        let temp = dir.join(format!(".{name}.tagteam-{pid}-{:08x}", fastrand::u32(..)));
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
        {
            Ok(file) => return Ok((temp, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
}

/// Takes the reservation's lock. The file is new, so nothing else can hold it: a refusal is an
/// error, never a wait (B.37).
fn lock(file: &File) -> io::Result<()> {
    // SAFETY: `file` owns a valid descriptor for the duration of the call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// `{pid, startedAt}`, for `doctor` and for a person reading the directory (§12.5). Liveness
/// never reads it.
fn contents(pid: u32) -> Vec<u8> {
    let started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let mut bytes = json!({"pid": pid, "startedAt": started_at})
        .to_string()
        .into_bytes();
    bytes.push(b'\n');
    bytes
}

/// The rename would replace whatever is at `path`. A dead file there (left by an earlier
/// process that had this pid) may go, but a live one is the reservation of an orphaned `claude`
/// of that process, and replacing it would hide a running session. The caller holds
/// `MutationGuard` and the account lock, the only way a reservation is created or removed, so
/// nothing can appear at `path` between this probe and the rename.
fn never_over_a_live_one(path: &Path) -> io::Result<()> {
    match probe_lock(path)? {
        LockProbe::Missing | LockProbe::Free => Ok(()),
        LockProbe::Held => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "{} is a live launch reservation of an earlier process with this pid",
                path.display()
            ),
        )),
    }
}

/// Removes every reservation in `profile` that `probe_lock` finds `Free` (dead). Returns their
/// paths, in name order. A temporary file left by a create that died before its rename is
/// removed too once it is free, and is not returned, since it was never a reservation. Under
/// `MutationGuard` and the account lock (§12.5).
pub fn remove_dead_reservations(profile: &Path) -> io::Result<Vec<PathBuf>> {
    let dir = profile.join(LAUNCH_DIR);
    let listing = match fs::read_dir(&dir) {
        Ok(l) => l,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut paths = Vec::new();
    for entry in listing {
        paths.push(entry?.path());
    }
    paths.sort();
    let mut removed = Vec::new();
    for path in paths {
        // The same test as `profile::launch_reservations`.
        let reservation = path.extension().is_some_and(|x| x == "lock");
        let temp = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.') && n.contains(TEMP_MARK));
        if !(reservation || temp) || probe_lock(&path)? != LockProbe::Free {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        }
        if reservation {
            removed.push(path);
        }
    }
    Ok(removed)
}
```

In `crates/tagteam-provider/src/lib.rs`, after `pub use read::{Read, ReadError};` add:

```rust
pub use reservation::{LaunchReservation, remove_dead_reservations};
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-provider --lib reservation::tests`
Expected: PASS, 9 tests.

Run: `cargo test -p tagteam-provider`, then
`cargo clippy -p tagteam-provider --all-targets --target x86_64-unknown-linux-gnu -- -D warnings`
Expected: PASS; no warnings. `flock` works on Linux's read-only probe descriptor too (M4a
Task 6).

- [ ] **Step 5: Format and lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider/src/reservation.rs crates/tagteam-provider/src/lib.rs
git commit -m "Reserve a launch with a locked file its claude inherits"
```

---

### Task 5: Three-way merge; seed and merge-back (CC, FakeAgent)

§12.4 gives every quiescent launch a seeded `.claude.json` and every last session out a
merge-back. The seed copies the default home's `projects` and `mcpServers` into the profile and
records what it copied in a baseline. The merge-back diffs the profile's `projects.<path>.<key>`
and `mcpServers.<name>` against that baseline and applies each change to `~/.claude.json`, unless
the default file changed the same key meanwhile, in which case the default wins. Review Focus 4
is this task's main test: default-home sessions and the profile's session edit both files at
once, and the merge-back keeps every byte outside the two subtrees.

Three units, two cycles:
- `tagteam_core::merge::three_way`, the pure merge (Decision 4). It sees values, never bytes.
- The provider methods `seed_profile`, `has_baseline` and `merge_back` (Decision 5). Claude Code
  owns `.claude.json`'s shape and applies the result with the §9.5 splice of the whole `projects`
  and `mcpServers` values. FakeAgent does the same over its own private file. All three take the
  profile's actual directory, never its recorded spelling (Decision 22).
- The engine (Tasks 9–11) owns the order and the outer locks. The summary line and the per-key
  log are Task 11's: it receives `MergeReport`.

**Readings of the spec this task commits to:**
- **Granularity and equality.** A project's keys (`projects.<path>.<key>`) and a user-scope MCP
  server (`mcpServers.<name>`) are the units. Values are compared whole as `serde_json::Value`s:
  key order inside a value does not matter, and number text does (`arbitrary_precision`), so
  `1.0` and `1` differ. That is the faithful direction, because CC rewrites a value it changed.
- **When a change is applied.** A key the profile changed, added or removed since the baseline is
  applied only where the default still holds the baseline's value.
  - The default changed it too, to anything else, including removing it: that is a conflict, and
    the default's value stays (§12.4 step 3).
  - Both made the same change: nothing to write and nothing lost, so the key is neither applied
    nor a conflict.
- **Whole projects.** A project the profile removed loses each of its keys, and goes from the
  default once none of the default's keys remain. A key the default added to it since the seed
  keeps it. A project the profile added is appended after the default's, with its keys in the
  profile's order.
- **A subtree the profile does not hold as an object changed nothing.** Claude Code never drops
  `projects` or `mcpServers` from its config. Their absence means the profile's file was reset,
  and reading that as "every project removed" would wipe the default's trust and MCP approvals.
  Cost if wrong: a profile that dropped `mcpServers` on purpose keeps the default's servers. The
  same holds for one project whose value is not an object.
- **A default value that is there but is not an object** (the subtree, or one project) takes no
  change. Every change under it is a conflict and the default's value is kept, as §9.5 never
  replaces what it cannot splice.
- **The seed copies absence too.** A default without `projects` (or `mcpServers`) removes the
  profile's own, and the baseline records `null`. The baseline is then exactly what the profile
  holds after the seed, so the merge-back sees only this session's changes.
- **The seed writes the config, then the baseline.** A seed that stops between the two leaves no
  baseline, and the next launch seeds again. The other order would leave a baseline over an
  unseeded file, whose merge-back would push the profile's stale subtrees into the default.
- **The seed reads the outer file without its lock.** CC replaces it by rename, so a read sees
  one whole version. Only the write takes a lock (§12.4: "The write takes the profile's own config
  lock"). A profile with no config starts from `{}\n`. An unchanged config is not rewritten; the
  baseline always is.
- **The merge-back's cases.**
  - No baseline: `Ok` with an empty report, and nothing is written.
  - A baseline that cannot be read, or is not a version 1 tagteam baseline: `Err`, and it stays.
  - The profile has no config file at all: there is nothing to merge back. The baseline is
    removed, with a WARN log, so the next launch can seed.
  - A torn profile config: `Err`, with the profile remedy below.
  - Otherwise, under the default home's config lock alone: read `~/.claude.json`, merge, and write
    both spans at once (skipped when nothing was applied). Then remove the baseline.
  - A removal that fails after the write is `Err`. The next launch's merge-back then finds every
    key already applied ("the same change on both sides"), so a retry changes nothing.
- **A profile's own `.claude.json` that cannot be spliced** names `PROFILE_CONFIG_REMEDY`.
  `CONFIG_REMEDY` points at `~/.claude/backups/`, which holds the default home's backups, not
  the profile's.
- **Lock waits.** The seed waits for the profile's config lock under `env.cancel`, since a
  launch's lock wait is a cancellation point (§12.5). The merge-back waits under the `cancel` it
  is given, since exit handling chooses its token (Decision 12).
- **Conflict names.** They are `projects["<path>"].<key>` and `mcpServers["<name>"]`, the path
  JSON-quoted because a path may hold dots. They hold no email (§4.4). `applied` is the count
  of applied keys. Task 11 logs each name and prints one summary line.
- **The seed follows `CcPaths`.** If a profile holds a legacy `.config.json`, Claude Code reads
  that file first, so the seed and the merge-back read and write it instead of `.claude.json`.
- **The profile is named by its directory** (Decision 22). Seed, baseline and merge-back touch
  only files, and a profile's files are where the profile is today, so all three take `dir`.
  The recorded spelling names the old path once the data directory has moved, and a baseline
  looked for there would never be found. Claude Code's paths come from M4a's
  `session::profile_paths(env, dir)`, FakeAgent's from M4a's `profile_env_in(env, dir)`: the
  helpers M4a's own profile reads use (its Decision 19).
- **FakeAgent's shape.** Its private per-home config is `identity.json`, which M4a's share
  policy keeps private. It merges one flat subtree, `prefs.<name>`, through `three_way`'s flat
  (`mcpServers`) slot, and its conflicts read `prefs["<name>"]`.
  - Not `prefs.json`: M4a links `prefs.json` into every profile, so a profile's `prefs.json`
    *is* the outer one (Decision 16).
  - The baseline has the same stable file name, `.tagteam-baseline.json`, in FakeAgent's own
    shape.
  - Its one lock, `.live.lock`, is taken in the profile for the seed and in the outer home
    alone for the merge-back.

**Files:**
- Create: `crates/tagteam-core/src/merge.rs`
- Modify: `crates/tagteam-core/src/lib.rs` (`pub mod merge;` between `pub mod ids;` and
  `pub mod pace;`, today lines 7–8)
- Modify: `crates/tagteam-provider/src/provider.rs`
  - the `Cancel` import, beside today's lines 14–20;
  - `MergeReport` after M4a's `SharePolicy`, before `Capabilities` (today line 136);
  - three methods at the end of `trait Provider`, after M4a's `invoked_by` (today the trait ends
    at line 581).
- Modify: `crates/tagteam-provider/src/lib.rs` (the `provider` re-export, today lines 37–42, as
  M4a left it)
- Modify: `crates/tagteam-cc/src/session.rs` (M4a's file: its imports; the seed, `has_baseline`
  and the merge-back after `profile_paths`)
- Modify: `crates/tagteam-cc/src/provider.rs` (the `tagteam_provider` import, today lines 9–14;
  three methods at the end of `impl Provider for ClaudeCode`, after M4a's `invoked_by`)
- Modify: `crates/tagteam-fake/src/provider.rs`
  - imports, today lines 1–25;
  - constants after `REMEDY` (line 32);
  - helpers before `showable_token` (line 149);
  - three methods at the end of `impl Provider for FakeAgent`, after M4a's `invoked_by`.
- Test:
  - `crates/tagteam-core/src/merge.rs` (in-module);
  - `crates/tagteam-cc/tests/session.rs` (M4a's file: two root helpers and the module
    `seed_and_merge_back`);
  - `crates/tagteam-fake/tests/provider.rs` (the module `seed_and_merge_back`).

**Interfaces:**

M3a's and M4a's code is not on this branch yet. Everything named here from them is written
against their plans' Interface Contracts and task code.

- Consumes:
  - M3a:
    - `tagteam_provider::Cancel` (`new`, `request`, `requested`) and `Env.cancel`.
    - `tagteam_cc::locks::acquire_config(paths: &CcPaths, timeout: Duration, cancel: &Cancel) -> Result<CcConfigSet, LockError>`.
    - `MkdirLockSpec::with_cancel(self, cancel: &Cancel) -> Self`.
    - `LockError::Interrupted { path, signal }`.
  - M4a:
    - `tagteam_cc::session` (M4a's file) and its crate-private
      `profile_paths(env: &Env, dir: &Path) -> CcPaths`; FakeAgent's private
      `profile_env_in(env: &Env, dir: &Path) -> Env` (both M4a's Decision 19).
    - `Provider::profile_spelling` and `Provider::profile_identity(&self, env: &Env, dir: &Path) -> Read<Identity>`.
    - The test fixtures of `crates/tagteam-cc/tests/session.rs`: `Fx { env, kc, cc }`, `fx()`,
      `profile(&Fx, id) -> (PathBuf, String)` and `with_vars`.
    - Those of `crates/tagteam-fake/tests/provider.rs`: `Fx { env, fake }`, `fx()`, `file_json`
      and `with_home`.
  - Existing:
    - `tagteam_provider::splice::{get_top_level, replace_top_level, remove_top_level, SpliceError}`
      and `atomic::{write_atomic_with, write_atomic_private_with, ensure_private_dir}`.
    - `tagteam_cc::config::read_bytes`, `CcPaths::resolve`, `provider::CONFIG_REMEDY` and
      `LiveLockSet::check_owned`.
    - FakeAgent's `read_file`, `present_or_err`, `REMEDY` and `LOCK_STALE`.
- Produces (Interface Contract):
  - `tagteam_core::merge::{MergeKey, MergeResult, three_way}`.
  - `tagteam_provider::MergeReport { applied: usize, conflicts: Vec<String> }`.
  - `Provider::seed_profile(&self, env: &Env, dir: &Path, identity: &Identity) -> Result<(), ProviderError>`.
  - `Provider::has_baseline(&self, dir: &Path) -> bool`.
  - `Provider::merge_back(&self, env: &Env, dir: &Path, cancel: &Cancel) -> Result<MergeReport, ProviderError>`.
  - `tagteam_cc::session::BASELINE_FILE` (crate-private, `.tagteam-baseline.json`) and the
    baseline shape `{"format": "tagteam-baseline", "version": 1, "projects", "mcpServers"}`.
  - For Task 11: `MergeReport.conflicts` names each key as above; `applied` counts applied keys.

**Spec:**
- §12.4 "Seed":
  - start from the profile's file, or `{}`;
  - copy `projects` and top-level `mcpServers` from `~/.claude.json`;
  - set `oauthAccount` from the account, `hasCompletedOnboarding: true`, and `theme` if absent
    (the default file's, else `"dark"`);
  - write `.tagteam-baseline.json`;
  - take the profile's own config lock.
- §12.4 "Merge back":
  - under the default profile's config lock on its own, diff `projects.<path>.<key>` and
    `mcpServers.<name>` against the baseline;
  - apply each changed or removed key with the §9.5 splice of the two subtrees;
  - the default wins a key both sides changed;
  - an unsplicable or unwritable `~/.claude.json` fails it, keeping the profile's changes and the
    baseline.
- §3: `run` writes only the `projects` and `mcpServers` subtrees of `~/.claude.json`, by
  three-way merge; every other byte stays identical.
- §4.3: the standalone config lock for seeding and merge-back takes no credential lock while held.
- §9.5:
  - only the value's span is replaced, rendered as `JSON.stringify(v, null, 2)`;
  - a missing file is created with only the new keys, 0600;
  - torn or not an object aborts without writing.
- Appendix A.6: `projects`, `mcpServers`, `theme` and `hasCompletedOnboarding` are what seeding
  and merge-back read; nothing else is copied.
- Decisions 4 and 5.

#### Cycle A: the pure merge

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-core/src/merge.rs` holding only its test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project(path: &str, key: &str) -> MergeKey {
        MergeKey::Project {
            path: path.into(),
            key: key.into(),
        }
    }

    fn server(name: &str) -> MergeKey {
        MergeKey::McpServer { name: name.into() }
    }

    /// The seeded `projects`: what the baseline holds, and the default too until it moves.
    fn projects() -> Value {
        json!({
            "/work/app": {"allowedTools": [], "hasTrustDialogAccepted": false},
            "/work/lib": {"allowedTools": ["Bash"]}
        })
    }

    fn servers() -> Value {
        json!({"local": {"command": "srv"}, "remote": {"url": "https://mcp.example"}})
    }

    fn untouched() -> MergeResult {
        MergeResult {
            projects: None,
            mcp_servers: None,
            applied: vec![],
            conflicts: vec![],
        }
    }

    /// `projects` alone, with no `mcpServers` on any side.
    fn on_projects(base: &Value, mine: &Value, theirs: &Value) -> MergeResult {
        three_way(
            (base, &Value::Null),
            (mine, &Value::Null),
            (theirs, &Value::Null),
        )
    }

    /// `mcpServers` alone.
    fn on_servers(base: &Value, mine: &Value, theirs: &Value) -> MergeResult {
        three_way(
            (&Value::Null, base),
            (&Value::Null, mine),
            (&Value::Null, theirs),
        )
    }

    fn names(v: &Value) -> Vec<&str> {
        v.as_object().unwrap().keys().map(String::as_str).collect()
    }

    #[test]
    fn a_profile_that_changed_nothing_changes_nothing_whatever_the_default_did() {
        let mut theirs = projects();
        theirs["/work/app"]["allowedTools"] = json!(["Read"]);
        theirs["/work/new"] = json!({"allowedTools": []});
        assert_eq!(
            three_way(
                (&projects(), &servers()),
                (&projects(), &servers()),
                (&theirs, &json!({}))
            ),
            untouched()
        );
    }

    #[test]
    fn a_key_changed_only_in_the_profile_is_applied_in_place() {
        let mut mine = projects();
        mine["/work/lib"]["allowedTools"] = json!(["Bash", "Edit"]);
        let mut theirs = projects();
        theirs["/work/app"]["hasTrustDialogAccepted"] = json!(true);
        let r = on_projects(&projects(), &mine, &theirs);
        let p = r.projects.unwrap();
        assert_eq!(p["/work/lib"]["allowedTools"], json!(["Bash", "Edit"]));
        assert_eq!(
            p["/work/app"]["hasTrustDialogAccepted"],
            json!(true),
            "the default's own change stays"
        );
        assert_eq!(names(&p), ["/work/app", "/work/lib"], "in the default's order");
        assert_eq!(r.applied, [project("/work/lib", "allowedTools")]);
        assert!(r.conflicts.is_empty());
        assert_eq!(r.mcp_servers, None, "nothing changed under mcpServers");
    }

    #[test]
    fn a_key_removed_in_the_profile_goes_and_one_added_is_appended() {
        let mut mine = projects();
        mine["/work/app"]
            .as_object_mut()
            .unwrap()
            .shift_remove("hasTrustDialogAccepted");
        mine["/work/app"]["mcpServers"] = json!({"p": {"command": "x"}});
        let r = on_projects(&projects(), &mine, &projects());
        let p = r.projects.unwrap();
        assert_eq!(names(&p["/work/app"]), ["allowedTools", "mcpServers"]);
        assert_eq!(
            r.applied,
            [
                project("/work/app", "hasTrustDialogAccepted"),
                project("/work/app", "mcpServers")
            ]
        );
    }

    #[test]
    fn a_key_both_sides_changed_keeps_the_default_s_value() {
        let mut mine = projects();
        // Changed on both sides.
        mine["/work/app"]["allowedTools"] = json!(["Edit"]);
        // Changed in the profile, removed in the default.
        mine["/work/app"]["hasTrustDialogAccepted"] = json!(true);
        // Removed in the profile, changed in the default.
        mine["/work/lib"]
            .as_object_mut()
            .unwrap()
            .shift_remove("allowedTools");
        let mut theirs = projects();
        theirs["/work/app"]["allowedTools"] = json!(["Read"]);
        theirs["/work/app"]
            .as_object_mut()
            .unwrap()
            .shift_remove("hasTrustDialogAccepted");
        theirs["/work/lib"]["allowedTools"] = json!(["Bash", "Read"]);
        let r = on_projects(&projects(), &mine, &theirs);
        assert_eq!(r.projects, None, "nothing was applied");
        assert!(r.applied.is_empty());
        assert_eq!(
            r.conflicts,
            [
                project("/work/app", "allowedTools"),
                project("/work/app", "hasTrustDialogAccepted"),
                project("/work/lib", "allowedTools")
            ]
        );
    }

    #[test]
    fn the_same_change_on_both_sides_is_neither_applied_nor_a_conflict() {
        let mut mine = projects();
        mine["/work/app"]["hasTrustDialogAccepted"] = json!(true);
        mine["/work/new"] = json!({"allowedTools": []});
        let theirs = mine.clone();
        assert_eq!(on_projects(&projects(), &mine, &theirs), untouched());
    }

    #[test]
    fn a_whole_project_added_in_the_profile_is_appended_with_its_keys() {
        let mut mine = projects();
        mine["/work/new"] = json!({"allowedTools": ["Bash"], "hasTrustDialogAccepted": true});
        let mut theirs = projects();
        theirs["/work/other"] = json!({"allowedTools": []});
        let r = on_projects(&projects(), &mine, &theirs);
        let p = r.projects.unwrap();
        assert_eq!(
            names(&p),
            ["/work/app", "/work/lib", "/work/other", "/work/new"]
        );
        assert_eq!(p["/work/new"], mine["/work/new"]);
        assert_eq!(names(&p["/work/new"]), ["allowedTools", "hasTrustDialogAccepted"]);
        assert_eq!(
            r.applied,
            [
                project("/work/new", "allowedTools"),
                project("/work/new", "hasTrustDialogAccepted")
            ]
        );
    }

    #[test]
    fn a_whole_project_removed_in_the_profile_goes_unless_the_default_added_to_it() {
        let mut mine = projects();
        mine.as_object_mut().unwrap().shift_remove("/work/app");
        mine.as_object_mut().unwrap().shift_remove("/work/lib");
        let mut theirs = projects();
        theirs["/work/lib"]["hasTrustDialogAccepted"] = json!(true);
        let r = on_projects(&projects(), &mine, &theirs);
        assert_eq!(
            r.projects.unwrap(),
            json!({"/work/lib": {"hasTrustDialogAccepted": true}}),
            "the default's own key keeps its project"
        );
        assert_eq!(
            r.applied,
            [
                project("/work/app", "allowedTools"),
                project("/work/app", "hasTrustDialogAccepted"),
                project("/work/lib", "allowedTools")
            ]
        );
        assert!(r.conflicts.is_empty());
    }

    #[test]
    fn an_mcp_server_added_removed_or_changed_in_the_profile_is_applied() {
        let mine = json!({"local": {"command": "srv2"}, "added": {"command": "new"}});
        let r = on_servers(&servers(), &mine, &servers());
        let s = r.mcp_servers.unwrap();
        assert_eq!(s, mine);
        assert_eq!(names(&s), ["local", "added"]);
        assert_eq!(r.applied, [server("added"), server("local"), server("remote")]);
        assert_eq!(r.projects, None);
    }

    #[test]
    fn an_mcp_server_both_sides_changed_keeps_the_default_s() {
        let mine = json!({"local": {"command": "mine"}, "remote": {"url": "https://mcp.example"}});
        let theirs =
            json!({"local": {"command": "theirs"}, "remote": {"url": "https://mcp.example"}});
        let r = on_servers(&servers(), &mine, &theirs);
        assert_eq!(r.mcp_servers, None);
        assert!(r.applied.is_empty());
        assert_eq!(r.conflicts, [server("local")]);
    }

    #[test]
    fn a_default_without_the_subtrees_gets_the_profile_s_changes_alone() {
        let mine_p = json!({"/work/new": {"allowedTools": []}});
        let mine_s = json!({"added": {"command": "new"}});
        let r = three_way(
            (&json!({}), &json!({})),
            (&mine_p, &mine_s),
            (&Value::Null, &Value::Null),
        );
        assert_eq!(r.projects, Some(mine_p));
        assert_eq!(r.mcp_servers, Some(mine_s));
    }

    #[test]
    fn a_profile_without_the_subtrees_changed_nothing() {
        // A reset profile file has neither: no reason to remove every project and server.
        for (mine_p, mine_s) in [(Value::Null, Value::Null), (json!("reset"), json!(7))] {
            let r = three_way(
                (&projects(), &servers()),
                (&mine_p, &mine_s),
                (&projects(), &servers()),
            );
            assert_eq!(r, untouched(), "{mine_p} {mine_s}");
        }
    }

    #[test]
    fn a_default_value_that_is_not_an_object_takes_no_change() {
        let mut mine = projects();
        mine["/work/lib"]["allowedTools"] = json!(["Edit"]);
        mine["/work/new"] = json!({"allowedTools": []});
        let r = on_projects(&projects(), &mine, &json!(["not", "an", "object"]));
        assert_eq!(r.projects, None);
        assert!(r.applied.is_empty());
        assert_eq!(
            r.conflicts,
            [
                project("/work/lib", "allowedTools"),
                project("/work/new", "allowedTools")
            ]
        );
        let mut theirs = projects();
        theirs["/work/lib"] = json!("garbage");
        let r = on_projects(&projects(), &mine, &theirs);
        assert_eq!(r.conflicts, [project("/work/lib", "allowedTools")]);
        assert_eq!(r.applied, [project("/work/new", "allowedTools")]);
        assert_eq!(
            r.projects.unwrap()["/work/lib"],
            json!("garbage"),
            "the default's value is kept"
        );
    }

    #[test]
    fn an_explicitly_null_default_project_takes_no_change() {
        // A project the default set to null is present, not absent: it is a non-object value
        // like any other, so the profile's changes to it are conflicts and the null is kept.
        // A project the default lacks altogether still starts empty.
        let mut mine = projects();
        mine["/work/lib"]["hasTrustDialogAccepted"] = json!(true);
        let mut theirs = projects();
        theirs["/work/lib"] = Value::Null;
        let r = on_projects(&projects(), &mine, &theirs);
        assert_eq!(r.projects, None);
        assert!(r.applied.is_empty());
        assert_eq!(r.conflicts, [project("/work/lib", "hasTrustDialogAccepted")]);

        let mut theirs = projects();
        theirs.as_object_mut().unwrap().shift_remove("/work/lib");
        let r = on_projects(&projects(), &mine, &theirs);
        assert_eq!(r.applied, [project("/work/lib", "hasTrustDialogAccepted")]);
        assert_eq!(
            r.projects.unwrap()["/work/lib"],
            json!({"hasTrustDialogAccepted": true})
        );
    }
}
```

and declare it in `crates/tagteam-core/src/lib.rs`, between `pub mod ids;` and `pub mod pace;`:

```rust
pub mod merge;
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-core --lib merge`
Expected: compile errors only:
- E0433: failed to resolve `MergeKey`;
- E0412: cannot find types `Value` and `MergeResult`;
- E0425: cannot find function `three_way`.

- [ ] **Step 3: Implement**

In `crates/tagteam-core/src/merge.rs`, above the test module, add:

```rust
//! §12.4's merge-back as a pure function: the profile's changes since the seed's baseline,
//! applied over the default file key by key, at §12.4's granularity (`projects.<path>.<key>`
//! and `mcpServers.<name>`). The default file wins a key both sides changed. A provider
//! applies the result with the §9.5 splice of the whole subtrees (Decision 4), so this module
//! never sees bytes.

use serde_json::{Map, Value};

/// One key of §12.4's diff: `projects.<path>.<key>` or `mcpServers.<name>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MergeKey {
    Project { path: String, key: String },
    McpServer { name: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergeResult {
    /// The new `projects` and `mcpServers` values for the default file (`None`: leave the
    /// key as it is, because nothing changed under it).
    pub projects: Option<serde_json::Value>,
    pub mcp_servers: Option<serde_json::Value>,
    /// Keys the profile changed (or removed) that were applied.
    pub applied: Vec<MergeKey>,
    /// Keys both sides changed since the baseline: the default's value was kept (§12.4 step 3).
    pub conflicts: Vec<MergeKey>,
}

/// §12.4: the profile's changes since `baseline`, applied over `default`. Each argument is
/// the pair (`projects`, `mcpServers`) as JSON values, `Value::Null` when absent.
///
/// - A key the profile changed, added or removed is applied only where the default still holds
///   the baseline's value. Where the default changed it too, that is a conflict and the
///   default's value stays; where both made the same change, there is nothing to do.
/// - A project the profile removed loses each of its keys, and goes once none of the default's
///   remains. A project it added is appended with its keys.
/// - A subtree the profile does not hold as an object changed nothing: Claude Code never drops
///   `projects` or `mcpServers`, so their absence means a reset file, not that all went.
/// - A default value that is there but not an object takes no change: each change under it is
///   a conflict.
///
/// `applied` and `conflicts` are sorted.
pub fn three_way(
    baseline: (&serde_json::Value, &serde_json::Value),
    profile: (&serde_json::Value, &serde_json::Value),
    default: (&serde_json::Value, &serde_json::Value),
) -> MergeResult {
    let mut keys = Keys::default();
    let projects = merge_projects(baseline.0, profile.0, default.0, &mut keys).map(Value::Object);
    let mcp_servers = profile
        .1
        .as_object()
        .and_then(|mine| {
            merge_level(
                baseline.1.as_object(),
                Some(mine),
                // At the top level `null` means the key is absent (the contract).
                (!default.1.is_null()).then_some(default.1),
                &|name: &str| MergeKey::McpServer {
                    name: name.to_owned(),
                },
                &mut keys,
            )
        })
        .map(Value::Object);
    keys.applied.sort();
    keys.conflicts.sort();
    MergeResult {
        projects,
        mcp_servers,
        applied: keys.applied,
        conflicts: keys.conflicts,
    }
}

type Object = Map<String, Value>;

/// What the merge reports, in the order it meets the keys (sorted at the end).
#[derive(Default)]
struct Keys {
    applied: Vec<MergeKey>,
    conflicts: Vec<MergeKey>,
}

fn get<'a>(level: Option<&'a Object>, name: &str) -> Option<&'a Value> {
    level.and_then(|o| o.get(name))
}

/// The names whose value differs between `base` and `mine`: `mine`'s in its order, then those
/// only `base` holds, in its.
fn changed<'a>(base: Option<&'a Object>, mine: Option<&'a Object>) -> Vec<&'a str> {
    let only_base = base
        .into_iter()
        .flat_map(|o| o.keys())
        .filter(move |k| get(mine, k).is_none());
    mine.into_iter()
        .flat_map(|o| o.keys())
        .chain(only_base)
        .map(String::as_str)
        .filter(|k| get(base, k) != get(mine, k))
        .collect()
}

/// One level of the merge: the profile's change from `base` to `mine` (`None`: the profile
/// holds no such object, so each key it had went), applied over `theirs`, the default's value
/// at this level. `None` means absent, so the level starts empty. Any value that is not an
/// object, `null` included, is present and takes no change: its keys are conflicts. Returns
/// the level's new object, or `None` when nothing was applied.
fn merge_level(
    base: Option<&Object>,
    mine: Option<&Object>,
    theirs: Option<&Value>,
    key: &dyn Fn(&str) -> MergeKey,
    keys: &mut Keys,
) -> Option<Object> {
    let changed = changed(base, mine);
    if changed.is_empty() {
        return None;
    }
    let mut out = match theirs {
        None => Object::new(),
        Some(Value::Object(o)) => o.clone(),
        Some(_) => {
            keys.conflicts.extend(changed.into_iter().map(key));
            return None;
        }
    };
    let mut any = false;
    for name in changed {
        let (was, now) = (get(base, name), get(mine, name));
        let held = out.get(name);
        if held == now {
            // Both sides made this change.
            continue;
        }
        if held != was {
            keys.conflicts.push(key(name));
            continue;
        }
        match now {
            Some(v) => {
                out.insert(name.to_owned(), v.clone());
            }
            None => {
                out.shift_remove(name);
            }
        }
        keys.applied.push(key(name));
        any = true;
    }
    any.then_some(out)
}

/// `projects`, two levels deep: each project's keys through `merge_level`, then the project
/// itself set, appended, or removed when the profile removed it and none of the default's keys
/// remain.
fn merge_projects(base: &Value, mine: &Value, theirs: &Value, keys: &mut Keys) -> Option<Object> {
    let mine = mine.as_object()?;
    let base = base.as_object();
    let mut projects = match theirs {
        Value::Null => Some(Object::new()),
        Value::Object(o) => Some(o.clone()),
        _ => None,
    };
    let paths: Vec<&str> = mine
        .keys()
        .chain(
            base.into_iter()
                .flat_map(|o| o.keys())
                .filter(|p| !mine.contains_key(p.as_str())),
        )
        .map(String::as_str)
        .collect();
    let mut any = false;
    for path in paths {
        let was = get(base, path).and_then(Value::as_object);
        let now = match mine.get(path) {
            None => None,
            Some(Value::Object(o)) => Some(o),
            // A project that is not an object has no keys to read: it changed nothing.
            Some(_) => continue,
        };
        let key = |k: &str| MergeKey::Project {
            path: path.to_owned(),
            key: k.to_owned(),
        };
        let Some(map) = projects.as_mut() else {
            // The default's `projects` is not an object: it takes no change.
            keys.conflicts
                .extend(changed(was, now).into_iter().map(key));
            continue;
        };
        let Some(project) = merge_level(was, now, map.get(path), &key, keys) else {
            continue;
        };
        if now.is_none() && project.is_empty() {
            map.shift_remove(path);
        } else {
            map.insert(path.to_owned(), Value::Object(project));
        }
        any = true;
    }
    if any { projects } else { None }
}
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-core --lib merge`
Expected: PASS, 13 tests.

Run: `cargo test -p tagteam-core`
Expected: PASS.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-core/src/merge.rs crates/tagteam-core/src/lib.rs
git commit -m "Merge a profile's projects and MCP servers three ways, the default file winning"
```

#### Cycle B: seed, baseline and merge-back in both providers

The trait's three new methods break both implementations until both have them, so this cycle is
one commit across the three crates.

- [ ] **Step 1: Write the failing tests**

**`crates/tagteam-cc/tests/session.rs`** (M4a Task 7's file). After its last test, add two
helpers at the file's root, which this module and Tasks 6 and 7's use:

```rust
/// `path`'s permission bits.
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Holds a `mkdir` lock at `path` for `for_ms`, as another process would, then lets it go.
fn hold(path: &Path, for_ms: u64) -> std::thread::JoinHandle<()> {
    fs::create_dir(path).unwrap();
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(for_ms));
        fs::remove_dir(&path).unwrap();
    })
}
```

then append:

```rust
mod seed_and_merge_back {
    //! §12.4: the seed of a profile's `.claude.json` from the default file, and the three-way
    //! merge of its `projects` and `mcpServers` back into it (Review Focus 4).

    use super::*;
    use std::thread;
    use std::time::Instant;

    use serde_json::Value;
    use tagteam_provider::splice::{get_top_level, remove_top_level, replace_top_level};
    use tagteam_provider::{Cancel, Identity, LockError, MergeReport};

    /// The default home's `~/.claude.json` as CC writes it: keys before, between and after the
    /// two subtrees a merge-back may touch, and a number serde would re-render if anything ever
    /// re-serialized the file.
    const DEFAULT_JSON: &str = r#"{
  "numStartups": 3,
  "projects": {
    "/work/app": {
      "allowedTools": [],
      "hasTrustDialogAccepted": false
    },
    "/work/lib": {
      "allowedTools": [
        "Bash"
      ]
    }
  },
  "userID": "default-user",
  "mcpServers": {
    "local": {
      "command": "srv"
    },
    "remote": {
      "url": "https://mcp.example"
    }
  },
  "theme": "light",
  "someFutureKey": {
    "n": 1e400
  }
}
"#;

    fn account(f: &Fx) -> Identity {
        f.cc.parse_identity(
            &json!({"emailAddress": "p@x.co", "organizationUuid": "org-1", "accountUuid": "acct-1"}),
        )
        .unwrap()
    }

    fn default_config(f: &Fx) -> PathBuf {
        f.env.home.join(".claude.json")
    }

    fn json_at(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn top(doc: &[u8], key: &str) -> Option<Value> {
        get_top_level(doc, key).unwrap()
    }

    /// `doc` without these top-level keys: the bytes that must not move (§3, §9.5).
    fn strip(doc: &[u8], keys: &[&str]) -> Vec<u8> {
        keys.iter()
            .fold(doc.to_vec(), |d, k| remove_top_level(&d, k).unwrap())
    }

    /// Replaces one top-level value of the file at `path`, as a CC session rewriting it does.
    fn edit(path: &Path, key: &str, value: Value) {
        let doc = fs::read(path).unwrap();
        fs::write(path, replace_top_level(&doc, key, &value).unwrap()).unwrap();
    }

    /// A profile seeded over `DEFAULT_JSON`. Returns its directory.
    fn seeded(f: &Fx) -> PathBuf {
        fs::write(default_config(f), DEFAULT_JSON).unwrap();
        let (dir, _) = profile(f, "0192");
        f.cc.seed_profile(&f.env, &dir, &account(f)).unwrap();
        dir
    }

    #[test]
    fn a_profile_with_no_file_is_seeded_from_nothing() {
        let f = fx();
        fs::write(default_config(&f), DEFAULT_JSON).unwrap();
        let (dir, _) = profile(&f, "0192");
        assert!(!f.cc.has_baseline(&dir));

        f.cc.seed_profile(&f.env, &dir, &account(&f)).unwrap();

        let config = dir.join(".claude.json");
        let v = json_at(&config);
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "projects",
                "mcpServers",
                "oauthAccount",
                "hasCompletedOnboarding",
                "theme"
            ]
        );
        let default = DEFAULT_JSON.as_bytes();
        assert_eq!(Some(v["projects"].clone()), top(default, "projects"));
        assert_eq!(Some(v["mcpServers"].clone()), top(default, "mcpServers"));
        assert_eq!(v["oauthAccount"], account(&f).raw);
        assert_eq!(v["hasCompletedOnboarding"], json!(true));
        assert_eq!(v["theme"], json!("light"), "the default file's theme");
        assert_eq!(mode(&config), 0o600);
        assert!(f.cc.has_baseline(&dir));
        let baseline = dir.join(".tagteam-baseline.json");
        assert_eq!(mode(&baseline), 0o600);
        assert_eq!(
            json_at(&baseline),
            json!({
                "format": "tagteam-baseline", "version": 1,
                "projects": v["projects"], "mcpServers": v["mcpServers"]
            })
        );
        assert_eq!(
            fs::read(default_config(&f)).unwrap(),
            default,
            "a seed never writes the default file"
        );
    }

    #[test]
    fn the_seed_overwrites_the_profile_s_own_subtrees_and_keeps_its_other_keys_and_theme() {
        let f = fx();
        fs::write(default_config(&f), DEFAULT_JSON).unwrap();
        let (dir, _) = profile(&f, "0192");
        let config = dir.join(".claude.json");
        let own = r#"{
  "userID": "profile-user",
  "projects": {
    "/stale": {
      "allowedTools": []
    }
  },
  "theme": "dark-daltonized",
  "mcpServers": {},
  "oauthAccount": {
    "emailAddress": "old@x.co"
  },
  "machineID": "m-1"
}
"#;
        fs::write(&config, own).unwrap();
        fs::set_permissions(&config, std::os::unix::fs::PermissionsExt::from_mode(0o640))
            .unwrap();

        f.cc.seed_profile(&f.env, &dir, &account(&f)).unwrap();

        let after = fs::read(&config).unwrap();
        let default = DEFAULT_JSON.as_bytes();
        assert_eq!(top(&after, "projects"), top(default, "projects"));
        assert_eq!(top(&after, "mcpServers"), top(default, "mcpServers"));
        assert_eq!(top(&after, "oauthAccount"), Some(account(&f).raw));
        assert_eq!(top(&after, "hasCompletedOnboarding"), Some(json!(true)));
        assert_eq!(
            top(&after, "theme"),
            Some(json!("dark-daltonized")),
            "a profile's own theme is kept"
        );
        let spliced = [
            "projects",
            "mcpServers",
            "oauthAccount",
            "hasCompletedOnboarding",
        ];
        assert_eq!(
            strip(&after, &spliced),
            strip(own.as_bytes(), &spliced),
            "every other byte, userID and machineID included"
        );
        assert_eq!(mode(&config), 0o640, "CC's file keeps its mode (§9.5)");
    }

    #[test]
    fn a_seed_with_no_theme_anywhere_sets_dark_and_copies_the_default_s_absence() {
        let f = fx();
        fs::write(default_config(&f), "{\n  \"userID\": \"u\"\n}\n").unwrap();
        let (dir, _) = profile(&f, "0192");
        fs::write(
            dir.join(".claude.json"),
            "{\n  \"projects\": {\n    \"/stale\": {}\n  },\n  \"mcpServers\": {}\n}\n",
        )
        .unwrap();

        f.cc.seed_profile(&f.env, &dir, &account(&f)).unwrap();

        let v = json_at(&dir.join(".claude.json"));
        assert_eq!(v.get("projects"), None, "the default has none, so the profile keeps none");
        assert_eq!(v.get("mcpServers"), None);
        assert_eq!(v["theme"], json!("dark"));
        assert_eq!(
            json_at(&dir.join(".tagteam-baseline.json")),
            json!({"format": "tagteam-baseline", "version": 1, "projects": null, "mcpServers": null})
        );
    }

    #[test]
    fn the_seed_waits_for_the_profile_s_config_lock_and_never_takes_the_default_s() {
        let f = fx();
        fs::write(default_config(&f), DEFAULT_JSON).unwrap();
        let (dir, _) = profile(&f, "0192");
        // The default home's config lock stays held: a seed that took it would time out.
        fs::create_dir(f.env.home.join(".claude.json.lock")).unwrap();
        let cc_writing = hold(&dir.join(".claude.json.lock"), 300);
        let start = Instant::now();

        f.cc.seed_profile(&f.env, &dir, &account(&f)).unwrap();

        assert!(
            start.elapsed() >= Duration::from_millis(300),
            "the seed waited for the profile's own lock"
        );
        cc_writing.join().unwrap();
        assert!(!dir.join(".claude.json.lock").exists(), "and released it");
        assert!(f.env.home.join(".claude.json.lock").is_dir());
    }

    #[test]
    fn a_torn_file_on_either_side_stops_the_seed_and_writes_nothing() {
        let f = fx();
        let (dir, _) = profile(&f, "0192");
        let config = dir.join(".claude.json");
        fs::write(default_config(&f), DEFAULT_JSON).unwrap();
        fs::write(&config, b"{\"userID\": ").unwrap();
        match f.cc.seed_profile(&f.env, &dir, &account(&f)) {
            Err(ProviderError::ConfigUnsplicable { path, remedy }) => {
                assert_eq!(path, config);
                assert!(
                    !remedy.contains("backups"),
                    "a profile's file has no backups: {remedy}"
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(fs::read(&config).unwrap(), b"{\"userID\": ");
        assert!(!f.cc.has_baseline(&dir));

        fs::remove_file(&config).unwrap();
        fs::write(default_config(&f), b"[1]").unwrap();
        assert!(matches!(
            f.cc.seed_profile(&f.env, &dir, &account(&f)),
            Err(ProviderError::ConfigUnsplicable { path, .. }) if path == default_config(&f)
        ));
        assert!(!config.exists());
        assert!(!f.cc.has_baseline(&dir));
    }

    /// Review Focus 4: default-home sessions edit `~/.claude.json` while the profile's session
    /// runs, and the profile's session edits its own file.
    #[test]
    fn a_merge_back_keeps_the_default_where_both_changed_and_applies_the_rest() {
        let f = fx();
        let dir = seeded(&f);
        let profile_config = dir.join(".claude.json");
        // The default home: the same key changed, a project of its own, and a key outside
        // both subtrees.
        edit(
            &default_config(&f),
            "projects",
            json!({
                "/work/app": {"allowedTools": ["Read"], "hasTrustDialogAccepted": true},
                "/work/lib": {"allowedTools": ["Bash"]},
                "/work/default-new": {"allowedTools": []}
            }),
        );
        edit(&default_config(&f), "numStartups", json!(4));
        // The profile: that key changed differently, another key, a new project, and an MCP
        // server removed.
        edit(
            &profile_config,
            "projects",
            json!({
                "/work/app": {"allowedTools": ["Edit"], "hasTrustDialogAccepted": false},
                "/work/lib": {"allowedTools": ["Bash", "Edit"]},
                "/work/profile-new": {"allowedTools": [], "hasTrustDialogAccepted": true}
            }),
        );
        edit(&profile_config, "mcpServers", json!({"local": {"command": "srv"}}));
        let before = fs::read(default_config(&f)).unwrap();
        let profile_before = fs::read(&profile_config).unwrap();

        let report = f.cc.merge_back(&f.env, &dir, &Cancel::new()).unwrap();

        assert_eq!(
            report,
            MergeReport {
                applied: 4,
                conflicts: vec![r#"projects["/work/app"].allowedTools"#.to_owned()],
            }
        );
        let after = fs::read(default_config(&f)).unwrap();
        let projects = top(&after, "projects").unwrap();
        assert_eq!(
            projects,
            json!({
                "/work/app": {"allowedTools": ["Read"], "hasTrustDialogAccepted": true},
                "/work/lib": {"allowedTools": ["Bash", "Edit"]},
                "/work/default-new": {"allowedTools": []},
                "/work/profile-new": {"allowedTools": [], "hasTrustDialogAccepted": true}
            })
        );
        let order: Vec<&str> = projects
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            order,
            [
                "/work/app",
                "/work/lib",
                "/work/default-new",
                "/work/profile-new"
            ]
        );
        assert_eq!(
            top(&after, "mcpServers"),
            Some(json!({"local": {"command": "srv"}}))
        );
        assert_eq!(
            strip(&after, &["projects", "mcpServers"]),
            strip(&before, &["projects", "mcpServers"]),
            "every other byte of ~/.claude.json, 1e400 and the default's numStartups included"
        );
        assert_eq!(
            fs::read(&profile_config).unwrap(),
            profile_before,
            "the profile is left as it is"
        );
        assert!(!f.cc.has_baseline(&dir), "a merge-back that ran leaves no baseline");
    }

    #[test]
    fn a_merge_back_with_nothing_to_merge_writes_nothing() {
        let f = fx();
        let dir = seeded(&f);
        edit(&default_config(&f), "numStartups", json!(9));
        let before = fs::read(default_config(&f)).unwrap();

        let report = f.cc.merge_back(&f.env, &dir, &Cancel::new()).unwrap();

        assert_eq!(report, MergeReport::default());
        assert_eq!(fs::read(default_config(&f)).unwrap(), before, "not one byte");
        assert!(!f.cc.has_baseline(&dir));
        assert_eq!(
            f.cc.merge_back(&f.env, &dir, &Cancel::new()).unwrap(),
            MergeReport::default(),
            "with no baseline left, a second merge-back does nothing"
        );
    }

    #[test]
    fn a_torn_default_file_fails_the_merge_back_and_keeps_the_profile_and_its_baseline() {
        let f = fx();
        let dir = seeded(&f);
        let profile_config = dir.join(".claude.json");
        edit(&profile_config, "mcpServers", json!({}));
        let profile_before = fs::read(&profile_config).unwrap();
        let baseline_before = fs::read(dir.join(".tagteam-baseline.json")).unwrap();
        fs::write(default_config(&f), b"{\"projects\": {").unwrap();

        match f.cc.merge_back(&f.env, &dir, &Cancel::new()) {
            Err(ProviderError::ConfigUnsplicable { path, remedy }) => {
                assert_eq!(path, default_config(&f));
                assert!(remedy.contains("~/.claude/backups/"), "{remedy}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(fs::read(default_config(&f)).unwrap(), b"{\"projects\": {");
        assert_eq!(fs::read(&profile_config).unwrap(), profile_before);
        assert_eq!(
            fs::read(dir.join(".tagteam-baseline.json")).unwrap(),
            baseline_before
        );
        assert!(f.cc.has_baseline(&dir));
        assert!(
            !f.env.home.join(".claude.json.lock").exists(),
            "the default's lock is released"
        );
    }

    #[test]
    fn a_merge_back_waits_for_the_default_s_config_lock_alone() {
        let f = fx();
        let dir = seeded(&f);
        edit(&dir.join(".claude.json"), "mcpServers", json!({}));
        // Locks a merge-back never takes, held throughout: the profile's own config and
        // credential locks, and the default home's credential lock.
        for held in [
            dir.join(".claude.json.lock"),
            dir.join(".oauth_refresh.lock"),
            f.env.home.join(".claude/.oauth_refresh.lock"),
        ] {
            fs::create_dir(held).unwrap();
        }
        let cc_writing = hold(&f.env.home.join(".claude.json.lock"), 300);
        let start = Instant::now();

        let report = f.cc.merge_back(&f.env, &dir, &Cancel::new()).unwrap();

        assert!(start.elapsed() >= Duration::from_millis(300));
        cc_writing.join().unwrap();
        assert_eq!(report.applied, 2, "both servers removed");
        assert_eq!(top(&fs::read(default_config(&f)).unwrap(), "mcpServers"), Some(json!({})));
    }

    #[test]
    fn a_signal_ends_the_merge_back_s_wait_and_keeps_the_baseline() {
        let f = fx();
        let dir = seeded(&f);
        edit(&dir.join(".claude.json"), "mcpServers", json!({}));
        fs::create_dir(f.env.home.join(".claude.json.lock")).unwrap();
        let cancel = Cancel::new();
        let signal = {
            let cancel = cancel.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(200));
                cancel.request(15);
            })
        };

        let result = f.cc.merge_back(&f.env, &dir, &cancel);

        signal.join().unwrap();
        assert!(
            matches!(
                result,
                Err(ProviderError::Lock(LockError::Interrupted { signal: 15, .. }))
            ),
            "{result:?}"
        );
        assert!(f.cc.has_baseline(&dir));
        assert_eq!(fs::read(default_config(&f)).unwrap(), DEFAULT_JSON.as_bytes());
    }

    #[test]
    fn a_default_file_that_is_gone_is_created_with_the_merged_subtree_alone() {
        let f = fx();
        let dir = seeded(&f);
        edit(
            &dir.join(".claude.json"),
            "mcpServers",
            json!({
                "local": {"command": "srv"}, "remote": {"url": "https://mcp.example"},
                "added": {"command": "new"}
            }),
        );
        fs::remove_file(default_config(&f)).unwrap();

        let report = f.cc.merge_back(&f.env, &dir, &Cancel::new()).unwrap();

        assert_eq!(report.applied, 1);
        assert_eq!(
            json_at(&default_config(&f)),
            json!({"mcpServers": {"added": {"command": "new"}}})
        );
        assert_eq!(mode(&default_config(&f)), 0o600);
    }

    #[test]
    fn a_profile_whose_file_is_gone_has_nothing_to_merge_back() {
        let f = fx();
        let dir = seeded(&f);
        fs::remove_file(dir.join(".claude.json")).unwrap();

        assert_eq!(
            f.cc.merge_back(&f.env, &dir, &Cancel::new()).unwrap(),
            MergeReport::default()
        );
        assert_eq!(fs::read(default_config(&f)).unwrap(), DEFAULT_JSON.as_bytes());
        assert!(!f.cc.has_baseline(&dir), "so the next launch can seed");
    }

    #[test]
    fn a_baseline_that_is_not_one_fails_the_merge_back_and_stays() {
        let f = fx();
        let dir = seeded(&f);
        let baseline = dir.join(".tagteam-baseline.json");
        for bytes in [&b"{\"format\": \"tagteam-profile\"}"[..], b"{", b"[]"] {
            fs::write(&baseline, bytes).unwrap();
            assert!(matches!(
                f.cc.merge_back(&f.env, &dir, &Cancel::new()),
                Err(ProviderError::Invalid(_))
            ));
            assert_eq!(fs::read(&baseline).unwrap(), bytes);
            assert!(f.cc.has_baseline(&dir));
        }
        assert_eq!(fs::read(default_config(&f)).unwrap(), DEFAULT_JSON.as_bytes());
    }
}
```

The profiles live under the fixture's `data_dir()/sessions/`. The provider is given the
profile's directory (Decision 22), so a path it reports is under that directory as given, and is
compared with `dir.join(…)`, never with the canonical spelling (`/private/var/…` on macOS).

**`crates/tagteam-fake/tests/provider.rs`**, at the end of the file:

```rust
mod seed_and_merge_back {
    //! §12.4 in FakeAgent's shape: the flat `prefs` of its private `identity.json`.

    use super::*;
    use std::path::{Path, PathBuf};

    use tagteam_provider::splice::{get_top_level, remove_top_level, replace_top_level};
    use tagteam_provider::{Cancel, Identity, MergeReport};

    fn profile(f: &Fx) -> (PathBuf, String) {
        let dir = f.env.data_dir().join("sessions/0193");
        fs::create_dir_all(&dir).unwrap();
        let spelling = dir.to_str().unwrap().to_owned();
        (dir, spelling)
    }

    fn bob(f: &Fx) -> Identity {
        f.fake
            .parse_identity(&identity_json("bob", "ws2", "uid-bob"))
            .unwrap()
    }

    fn set_prefs(path: &Path, prefs: Value) {
        let doc = fs::read(path).unwrap();
        fs::write(path, replace_top_level(&doc, "prefs", &prefs).unwrap()).unwrap();
    }

    #[test]
    fn a_seed_copies_the_outer_prefs_and_sets_the_account_s_identity() {
        let f = fx();
        login(&f.env, "alice", "ws", "tok-a", "renew-a");
        let (dir, _) = profile(&f);
        assert!(!f.fake.has_baseline(&dir));

        f.fake.seed_profile(&f.env, &dir, &bob(&f)).unwrap();

        assert_eq!(
            file_json(&dir.join("identity.json")),
            json!({"prefs": {"theme": "x"}, "identity": identity_json("bob", "ws2", "uid-bob")})
        );
        assert_eq!(
            f.fake
                .profile_identity(&f.env, &dir)
                .present()
                .unwrap()
                .label,
            "bob@ws2"
        );
        assert!(f.fake.has_baseline(&dir));
        let baseline = dir.join(".tagteam-baseline.json");
        assert_eq!(
            file_json(&baseline),
            json!({"format": "tagteam-baseline", "version": 1, "prefs": {"theme": "x"}})
        );
        assert_eq!(
            fs::metadata(&baseline).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_merge_back_applies_the_profile_s_prefs_and_keeps_the_outer_where_both_changed() {
        let f = fx();
        login(&f.env, "alice", "ws", "tok-a", "renew-a");
        let (dir, _) = profile(&f);
        f.fake.seed_profile(&f.env, &dir, &bob(&f)).unwrap();
        let outer = FakePaths::resolve(&f.env).identity;
        set_prefs(&outer, json!({"theme": "y", "font": "mono"}));
        set_prefs(&dir.join("identity.json"), json!({"theme": "z", "lang": "nl"}));
        let before = fs::read(&outer).unwrap();

        let report = f.fake.merge_back(&f.env, &dir, &Cancel::new()).unwrap();

        assert_eq!(
            report,
            MergeReport {
                applied: 1,
                conflicts: vec![r#"prefs["theme"]"#.to_owned()],
            }
        );
        let after = fs::read(&outer).unwrap();
        assert_eq!(
            get_top_level(&after, "prefs").unwrap(),
            Some(json!({"theme": "y", "font": "mono", "lang": "nl"}))
        );
        assert_eq!(
            remove_top_level(&after, "prefs").unwrap(),
            remove_top_level(&before, "prefs").unwrap(),
            "the outer identity stays byte for byte"
        );
        assert!(!f.fake.has_baseline(&dir));
    }

    #[test]
    fn a_torn_outer_file_fails_the_merge_back_and_keeps_the_baseline() {
        let f = fx();
        login(&f.env, "alice", "ws", "tok-a", "renew-a");
        let (dir, _) = profile(&f);
        f.fake.seed_profile(&f.env, &dir, &bob(&f)).unwrap();
        set_prefs(&dir.join("identity.json"), json!({}));
        let outer = FakePaths::resolve(&f.env).identity;
        fs::write(&outer, b"{\"identity\": ").unwrap();

        assert!(matches!(
            f.fake.merge_back(&f.env, &dir, &Cancel::new()),
            Err(ProviderError::ConfigUnsplicable { path, .. }) if path == outer
        ));
        assert_eq!(fs::read(&outer).unwrap(), b"{\"identity\": ");
        assert!(f.fake.has_baseline(&dir));
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-cc --test session seed_and_merge_back`
Expected: compile errors:
- E0432: unresolved import `tagteam_provider::MergeReport`;
- E0599: no method named `seed_profile`, `has_baseline` or `merge_back` found for struct
  `ClaudeCode`.

Run: `cargo test -p tagteam-fake --test provider seed_and_merge_back`
Expected: the same errors, for `FakeAgent`.

- [ ] **Step 3: Implement**

**`crates/tagteam-provider/src/provider.rs`.** Beside the `crate::…` imports (today lines
14–20), add:

```rust
use crate::cancel::Cancel;
```

After M4a's `SharePolicy`, before `Capabilities`, add:

```rust
/// What a merge-back did (§12.4 step 3): how many of the profile's changes it applied, and
/// the keys both sides changed, where the default file's value was kept, named for the log.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MergeReport {
    pub applied: usize,
    pub conflicts: Vec<String>,
}
```

At the end of `trait Provider`, after M4a's `invoked_by`, add:

```rust
    // Seed and merge-back find the profile's files by `dir`, its actual directory, never by its
    // recorded spelling, which names the old path once the data directory has moved (Decision 22).
    /// §12.4: seeds the profile's config in `dir` from the outer home (`env`), for `identity`;
    /// writes the baseline. Takes the profile's own config lock, waiting under `env.cancel`.
    fn seed_profile(&self, env: &Env, dir: &Path, identity: &Identity) -> Result<(), ProviderError>;
    /// §12.4: whether a baseline is waiting in `dir` (a merge-back that never ran). One that
    /// cannot be looked at counts as waiting.
    fn has_baseline(&self, dir: &Path) -> bool;
    /// §12.4: merges the changes of the profile in `dir` back into the outer home's config
    /// under its config lock, alone; removes the baseline on success. Fails without touching
    /// the baseline.
    fn merge_back(&self, env: &Env, dir: &Path, cancel: &Cancel) -> Result<MergeReport, ProviderError>;
```

**`crates/tagteam-provider/src/lib.rs`**: the `provider` re-export (as M4a left it) becomes

```rust
pub use provider::{
    BeforeFallback, Capabilities, CapturedLogin, CredLocks, DeadReason, DoomedEntry, EntryKind,
    Identity, IdentitySurface, KindTraits, LiveAuth, LiveChange, LiveLockSet, LiveLocks,
    MergeReport, MustShare, Provider, ProviderError, RefreshResult, SecretStore, SharePolicy,
    StoredLogin, TransientKind, Undo, UsageResult, Written,
};
```

**`crates/tagteam-cc/src/session.rs`.** Its imports (M4a's) become:

```rust
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_core::merge::{MergeKey, three_way};
use tagteam_provider::atomic::{write_atomic_private_with, write_atomic_with};
use tagteam_provider::splice::{self, SpliceError};
use tagteam_provider::{
    Cancel, EntryKind, Env, Identity, LiveLockSet, MergeReport, ProviderError, Read,
};

use crate::config::read_bytes;
use crate::locks;
use crate::paths::CcPaths;
use crate::provider::CONFIG_REMEDY;
```

After M4a's `profile_paths`, add:

```rust
/// §12.4: the seed's record of the `projects` and `mcpServers` it put in the profile.
pub(crate) const BASELINE_FILE: &str = ".tagteam-baseline.json";
const BASELINE_FORMAT: &str = "tagteam-baseline";

/// What to do about a profile's own `.claude.json` that cannot be spliced. `CONFIG_REMEDY`
/// names `~/.claude/backups/`, which holds the default home's backups, not the profile's.
const PROFILE_CONFIG_REMEDY: &str =
    "repair it, or remove it so the next launch seeds a new one, then retry";

/// The `theme` a seed sets when neither the profile nor the default file has one (§12.4).
const DEFAULT_THEME: &str = "dark";

const PROJECTS: &str = "projects";
const MCP_SERVERS: &str = "mcpServers";

fn unsplicable(path: &Path, remedy: &'static str) -> ProviderError {
    ProviderError::ConfigUnsplicable {
        path: path.to_path_buf(),
        remedy,
    }
}

/// `key` set to `value` in `doc`, or removed when `value` is `None`.
fn put(doc: &[u8], key: &str, value: Option<&Value>) -> Result<Vec<u8>, SpliceError> {
    match value {
        Some(v) => splice::replace_top_level(doc, key, v),
        None => splice::remove_top_level(doc, key),
    }
}

/// One top-level value of `doc`, `Null` when absent (`three_way`'s convention).
fn top(doc: &[u8], key: &str) -> Result<Value, SpliceError> {
    Ok(splice::get_top_level(doc, key)?.unwrap_or(Value::Null))
}

/// §12.4's seed of the profile in `dir`, under its own config lock. From the profile's current
/// file, or `{}`: copy `projects` and top-level `mcpServers` from the outer home's global config
/// (`env`), their absence included; set `oauthAccount` from `identity`, `hasCompletedOnboarding:
/// true`, and `theme` when the profile has none (the default file's, else "dark"). Then write the
/// baseline, exactly the `projects` and `mcpServers` the profile now holds. The config goes
/// first, so a seed that stops between the two leaves no baseline over an unseeded file.
pub(crate) fn seed(
    env: &Env,
    dir: &Path,
    identity: &Identity,
    budget: Duration,
) -> Result<(), ProviderError> {
    // CC replaces the outer file by rename, so this read sees one whole version unlocked.
    let outer = CcPaths::resolve(env).global_config;
    let (projects, servers, theme) = match read_bytes(&outer) {
        Read::Present(b) => {
            let torn = |_: SpliceError| unsplicable(&outer, CONFIG_REMEDY);
            (
                splice::get_top_level(&b, PROJECTS).map_err(torn)?,
                splice::get_top_level(&b, MCP_SERVERS).map_err(torn)?,
                splice::get_top_level(&b, "theme").map_err(torn)?,
            )
        }
        Read::Absent => (None, None, None),
        Read::Unreadable(_) => return Err(unsplicable(&outer, CONFIG_REMEDY)),
    };
    let paths = profile_paths(env, dir);
    let lock = locks::acquire_config(&paths, budget, &env.cancel)?;
    let fence = || lock.check_owned().map_err(ProviderError::from);
    let config = &paths.global_config;
    let torn = |_: SpliceError| unsplicable(config, PROFILE_CONFIG_REMEDY);
    let before = match read_bytes(config) {
        Read::Present(b) => Some(b),
        Read::Absent => None,
        Read::Unreadable(_) => return Err(unsplicable(config, PROFILE_CONFIG_REMEDY)),
    };
    let start = before.clone().unwrap_or_else(|| b"{}\n".to_vec());
    let has_theme = splice::get_top_level(&start, "theme").map_err(torn)?.is_some();
    let mut new = put(&start, PROJECTS, projects.as_ref()).map_err(torn)?;
    new = put(&new, MCP_SERVERS, servers.as_ref()).map_err(torn)?;
    new = splice::replace_top_level(&new, "oauthAccount", &identity.raw).map_err(torn)?;
    new = splice::replace_top_level(&new, "hasCompletedOnboarding", &Value::Bool(true))
        .map_err(torn)?;
    if !has_theme {
        let theme = theme.unwrap_or_else(|| json!(DEFAULT_THEME));
        new = splice::replace_top_level(&new, "theme", &theme).map_err(torn)?;
    }
    if before.as_deref() != Some(new.as_slice()) {
        write_atomic_with(config, &new, 0o600, fence)?;
    }
    let baseline = json!({
        "format": BASELINE_FORMAT,
        "version": 1,
        "projects": projects.unwrap_or(Value::Null),
        "mcpServers": servers.unwrap_or(Value::Null),
    });
    let mut bytes = serde_json::to_vec_pretty(&baseline).expect("a Value always serializes");
    bytes.push(b'\n');
    write_atomic_private_with(&dir.join(BASELINE_FILE), &bytes, 0o600, fence)
}

/// §12.4: a baseline is waiting in `dir`, from a session whose merge-back never ran. One that
/// cannot even be looked at counts as waiting: its merge-back then fails, and nothing seeds
/// over it.
pub(crate) fn has_baseline(dir: &Path) -> bool {
    match fs::symlink_metadata(dir.join(BASELINE_FILE)) {
        Ok(_) => true,
        Err(e) => !matches!(
            e.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
        ),
    }
}

/// The baseline's `projects` and `mcpServers`; `None` when there is none. One that cannot be
/// read, or is not a version 1 tagteam baseline, is an error: the merge-back cannot run without
/// it, and nothing may seed over it (§12.4).
fn read_baseline(path: &Path) -> Result<Option<(Value, Value)>, ProviderError> {
    let bytes = match read_bytes(path) {
        Read::Present(b) => b,
        Read::Absent => return Ok(None),
        Read::Unreadable(e) => return Err(ProviderError::Unreadable(e)),
    };
    let invalid = || {
        ProviderError::Invalid(format!(
            "{} is not a tagteam baseline; remove it to drop the session's unmerged changes",
            path.display()
        ))
    };
    let v: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if v["format"].as_str() != Some(BASELINE_FORMAT) || v["version"].as_i64() != Some(1) {
        return Err(invalid());
    }
    match (v.get(PROJECTS), v.get(MCP_SERVERS)) {
        (Some(p), Some(m)) => Ok(Some((p.clone(), m.clone()))),
        _ => Err(invalid()),
    }
}

fn remove_baseline(path: &Path) -> Result<(), ProviderError> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// A merged key as the summary's log names it (§12.4 step 3): the path JSON-quoted, since a
/// path may hold dots.
fn key_name(k: &MergeKey) -> String {
    let quoted = |s: &str| serde_json::to_string(s).expect("a string always serializes");
    match k {
        MergeKey::Project { path, key } => format!("projects[{}].{key}", quoted(path)),
        MergeKey::McpServer { name } => format!("mcpServers[{}]", quoted(name)),
    }
}

/// §12.4's merge-back of the profile in `dir`. The profile is quiescent (the caller holds
/// `MutationGuard` and the account lock), so its file and baseline are read without a lock. The
/// default home's global config is then read, merged and written under its config lock alone
/// (§4.3), waited for under `cancel`: one write replacing the `projects` and `mcpServers` spans,
/// skipped when nothing was applied. The baseline goes last. Any failure before that leaves it,
/// and the profile, as they were.
pub(crate) fn merge_back(
    env: &Env,
    dir: &Path,
    budget: Duration,
    cancel: &Cancel,
) -> Result<MergeReport, ProviderError> {
    let baseline_path = dir.join(BASELINE_FILE);
    let Some(base) = read_baseline(&baseline_path)? else {
        return Ok(MergeReport::default());
    };
    let profile = profile_paths(env, dir).global_config;
    let mine = match read_bytes(&profile) {
        Read::Present(b) => {
            let torn = |_: SpliceError| unsplicable(&profile, PROFILE_CONFIG_REMEDY);
            (
                top(&b, PROJECTS).map_err(torn)?,
                top(&b, MCP_SERVERS).map_err(torn)?,
            )
        }
        Read::Absent => {
            tracing::warn!(
                "{} is gone, so its session has nothing to merge back",
                profile.display()
            );
            remove_baseline(&baseline_path)?;
            return Ok(MergeReport::default());
        }
        Read::Unreadable(_) => return Err(unsplicable(&profile, PROFILE_CONFIG_REMEDY)),
    };
    let paths = CcPaths::resolve(env);
    let lock = locks::acquire_config(&paths, budget, cancel)?;
    let fence = || lock.check_owned().map_err(ProviderError::from);
    let config = &paths.global_config;
    let torn = |_: SpliceError| unsplicable(config, CONFIG_REMEDY);
    let before = match read_bytes(config) {
        Read::Present(b) => Some(b),
        Read::Absent => None,
        Read::Unreadable(_) => return Err(unsplicable(config, CONFIG_REMEDY)),
    };
    let theirs = match &before {
        Some(b) => (
            top(b, PROJECTS).map_err(torn)?,
            top(b, MCP_SERVERS).map_err(torn)?,
        ),
        None => (Value::Null, Value::Null),
    };
    let merged = three_way(
        (&base.0, &base.1),
        (&mine.0, &mine.1),
        (&theirs.0, &theirs.1),
    );
    if merged.projects.is_some() || merged.mcp_servers.is_some() {
        // §9.5: a missing file is created holding only the merged keys.
        let mut new = before.unwrap_or_else(|| b"{}\n".to_vec());
        if let Some(v) = &merged.projects {
            new = splice::replace_top_level(&new, PROJECTS, v).map_err(torn)?;
        }
        if let Some(v) = &merged.mcp_servers {
            new = splice::replace_top_level(&new, MCP_SERVERS, v).map_err(torn)?;
        }
        write_atomic_with(config, &new, 0o600, fence)?;
    }
    // The config lock covers only the default file.
    drop(lock);
    remove_baseline(&baseline_path)?;
    Ok(MergeReport {
        applied: merged.applied.len(),
        conflicts: merged.conflicts.iter().map(key_name).collect(),
    })
}
```

**`crates/tagteam-cc/src/provider.rs`.** Add `Cancel` and `MergeReport` to the
`use tagteam_provider::{…}` import. With M3a's and M4a's additions, it then reads:

```rust
use tagteam_provider::{
    BeforeFallback, Cancel, Capabilities, CredLocks, Credential, DoomedEntry, Env,
    FreshCredential, Identity, IdentitySurface, Keychain, KindTraits, LiveAuth, LiveChange,
    LiveLockSet, LiveLocks, LockError, MergeReport, MustShare, MutationGuard, Pace, PollBudget,
    Provider, ProviderError, Read, SharePolicy, StoredLogin, Undo, UsageResult, Window, Written,
};
```

At the end of `impl Provider for ClaudeCode`, after M4a's `invoked_by`, add:

```rust
    fn seed_profile(&self, env: &Env, dir: &Path, identity: &Identity) -> Result<(), ProviderError> {
        session::seed(env, dir, identity, self.lock_budget)
    }

    fn has_baseline(&self, dir: &Path) -> bool {
        session::has_baseline(dir)
    }

    fn merge_back(&self, env: &Env, dir: &Path, cancel: &Cancel) -> Result<MergeReport, ProviderError> {
        session::merge_back(env, dir, self.lock_budget, cancel)
    }
```

**`crates/tagteam-fake/src/provider.rs`.** After `use tagteam_core::{Fingerprint, IdentityKey,
ProviderId};` (line 7), add:

```rust
use tagteam_core::merge::{MergeKey, three_way};
```

and add `Cancel` and `MergeReport` to the `use tagteam_provider::{…}` import (as M4a left it):

```rust
use tagteam_provider::{
    BeforeFallback, Cancel, Capabilities, CredLocks, Credential, DoomedEntry, EntryKind, Env,
    FreshCredential, Identity, IdentitySurface, KindTraits, LiveAuth, LiveChange, LiveLockSet,
    LiveLocks, LockError, MergeReport, MkdirLock, MkdirLockSpec, MustShare, MutationGuard, Pace,
    PollBudget, Provider, ProviderError, Read, ReadError, SecretStore, SharePolicy, StoredLogin,
    Undo, UsageResult, Window, Written,
};
```

After `const REMEDY` (line 32), add:

```rust
/// §12.4's baseline: Claude Code's stable file name, in FakeAgent's own shape.
const BASELINE_FILE: &str = ".tagteam-baseline.json";
const BASELINE_FORMAT: &str = "tagteam-baseline";
/// The one subtree of `identity.json` FakeAgent seeds and merges back, `prefs.<name>`: flat,
/// and alone, deliberately unlike Claude Code's two.
const PREFS: &str = "prefs";
```

Before `showable_token` (line 149), after M4a's `profile_env_in`, add:

```rust
fn unsplicable(path: &Path) -> ProviderError {
    ProviderError::ConfigUnsplicable {
        path: path.to_path_buf(),
        remedy: REMEDY,
    }
}

/// `prefs` in an `identity.json`'s bytes, `Null` when absent.
fn prefs_of(path: &Path, doc: &[u8]) -> Result<Value, ProviderError> {
    Ok(splice::get_top_level(doc, PREFS)
        .map_err(|_| unsplicable(path))?
        .unwrap_or(Value::Null))
}

/// FakeAgent's one `.live.lock` at `path`, waited for under `cancel`.
fn live_lock(path: PathBuf, budget: Duration, cancel: &Cancel) -> Result<MkdirLock, ProviderError> {
    Ok(MkdirLock::acquire(
        &MkdirLockSpec::new(path, LOCK_STALE, budget).with_cancel(cancel),
    )?)
}

/// The baseline's `prefs`; `None` when there is none.
fn read_baseline(path: &Path) -> Result<Option<Value>, ProviderError> {
    let bytes = match read_file(path) {
        Read::Present(b) => b,
        Read::Absent => return Ok(None),
        Read::Unreadable(e) => return Err(ProviderError::Unreadable(e)),
    };
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .filter(|v| v["format"] == BASELINE_FORMAT && v["version"] == 1)
        .and_then(|v| v.get(PREFS).cloned())
        .map(Some)
        .ok_or_else(|| {
            ProviderError::Invalid(format!("{} is not a tagteam baseline", path.display()))
        })
}

fn remove_baseline(path: &Path) -> Result<(), ProviderError> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// A merged key as the summary's log names it: `prefs["<name>"]`.
fn pref_name(k: &MergeKey) -> String {
    match k {
        MergeKey::McpServer { name } => format!("prefs[{}]", json!(name)),
        // Never produced: FakeAgent gives `three_way` no two-level subtree.
        MergeKey::Project { path, key } => format!("{path}.{key}"),
    }
}
```

At the end of `impl Provider for FakeAgent`, after M4a's `invoked_by`, add:

```rust
    /// §12.4 in FakeAgent's shape: the `identity.json` of the profile in `dir` (or `{}`) gets
    /// the outer home's `prefs`, absence included, and the account's `identity`; then the
    /// baseline. Under the profile's own `.live.lock`, FakeAgent's only lock.
    fn seed_profile(&self, env: &Env, dir: &Path, identity: &Identity) -> Result<(), ProviderError> {
        let outer = FakePaths::resolve(env).identity;
        let prefs = match read_file(&outer) {
            Read::Present(b) => {
                splice::get_top_level(&b, PREFS).map_err(|_| unsplicable(&outer))?
            }
            Read::Absent => None,
            Read::Unreadable(_) => return Err(unsplicable(&outer)),
        };
        let p = FakePaths::resolve(&profile_env_in(env, dir));
        let lock = live_lock(p.lock.clone(), self.lock_budget, &env.cancel)?;
        let fence = || lock.check_owned().map_err(ProviderError::from);
        let before =
            present_or_err(read_file(&p.identity)).map_err(|_| unsplicable(&p.identity))?;
        let start = before.clone().unwrap_or_else(|| b"{}\n".to_vec());
        let new = match &prefs {
            Some(v) => splice::replace_top_level(&start, PREFS, v),
            None => splice::remove_top_level(&start, PREFS),
        }
        .and_then(|doc| splice::replace_top_level(&doc, "identity", &identity.raw))
        .map_err(|_| unsplicable(&p.identity))?;
        if before.as_deref() != Some(new.as_slice()) {
            ensure_private_dir(&p.dir)?;
            write_atomic_with(&p.identity, &new, 0o600, fence)?;
        }
        let baseline = json!({
            "format": BASELINE_FORMAT,
            "version": 1,
            PREFS: prefs.unwrap_or(Value::Null),
        });
        let mut bytes = serde_json::to_vec_pretty(&baseline).expect("a Value always serializes");
        bytes.push(b'\n');
        write_atomic_private_with(&p.dir.join(BASELINE_FILE), &bytes, 0o600, fence)
    }

    fn has_baseline(&self, dir: &Path) -> bool {
        match fs::symlink_metadata(dir.join(BASELINE_FILE)) {
            Ok(_) => true,
            Err(e) => !matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ),
        }
    }

    /// §12.4 in FakeAgent's shape: the `prefs` of the profile in `dir` merged three ways into
    /// the outer `identity.json`, under the outer `.live.lock` alone; the baseline removed on
    /// success.
    fn merge_back(&self, env: &Env, dir: &Path, cancel: &Cancel) -> Result<MergeReport, ProviderError> {
        let p = FakePaths::resolve(&profile_env_in(env, dir));
        let baseline = p.dir.join(BASELINE_FILE);
        let Some(base) = read_baseline(&baseline)? else {
            return Ok(MergeReport::default());
        };
        let mine = match read_file(&p.identity) {
            Read::Present(b) => prefs_of(&p.identity, &b)?,
            Read::Absent => {
                remove_baseline(&baseline)?;
                return Ok(MergeReport::default());
            }
            Read::Unreadable(_) => return Err(unsplicable(&p.identity)),
        };
        let outer = FakePaths::resolve(env);
        let lock = live_lock(outer.lock.clone(), self.lock_budget, cancel)?;
        let fence = || lock.check_owned().map_err(ProviderError::from);
        let before = present_or_err(read_file(&outer.identity))
            .map_err(|_| unsplicable(&outer.identity))?;
        let theirs = match &before {
            Some(b) => prefs_of(&outer.identity, b)?,
            None => Value::Null,
        };
        let none = Value::Null;
        // FakeAgent's one flat subtree takes `three_way`'s flat slot.
        let merged = three_way((&none, &base), (&none, &mine), (&none, &theirs));
        if let Some(prefs) = &merged.mcp_servers {
            let start = before.unwrap_or_else(|| b"{}\n".to_vec());
            let new = splice::replace_top_level(&start, PREFS, prefs)
                .map_err(|_| unsplicable(&outer.identity))?;
            ensure_private_dir(&outer.dir)?;
            write_atomic_with(&outer.identity, &new, 0o600, fence)?;
        }
        drop(lock);
        remove_baseline(&baseline)?;
        Ok(MergeReport {
            applied: merged.applied.len(),
            conflicts: merged.conflicts.iter().map(pref_name).collect(),
        })
    }
```

`tagteam-fake` stays `#![forbid(unsafe_code)]`, and FakeAgent's `write_identity` keeps its own
local `unsplicable` closure, which shadows the new function inside it.

- [ ] **Step 4: Run them and see them pass**, then the crates' whole suites

Run: `cargo test -p tagteam-cc --test session seed_and_merge_back`
Expected: PASS, 13 tests.

Run: `cargo test -p tagteam-fake --test provider seed_and_merge_back`
Expected: PASS, 3 tests.

Run: `cargo test -p tagteam-core -p tagteam-provider -p tagteam-cc -p tagteam-fake`
Expected: PASS.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. Nothing in the engine or the CLI calls the new methods yet.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider/src/provider.rs crates/tagteam-provider/src/lib.rs \
  crates/tagteam-cc/src/session.rs crates/tagteam-cc/src/provider.rs crates/tagteam-cc/tests/session.rs \
  crates/tagteam-fake/src/provider.rs crates/tagteam-fake/tests/provider.rs
git commit -m "Seed a profile's config from the default home and merge its changes back"
```

---

### Task 6: The profile credential write and its composition

§12.3 step 4 writes a bootstrapped profile's credential:
- composed as an activation composes (§9.4 step 5), with the profile as the live store;
- to `<profile>/.credentials.json` alone;
- under the profile's own credential locks and storage-write lock.

tagteam never writes the profile's hashed Keychain item: CC moves the file into it on its next
credential write (Appendix A.3), and step 5 deletes any item that is left (M4a's
`delete_profile_credential`).

Today no `LiveStore` writer fits. `write_credential_entry` upserts the Keychain item first on
macOS, and `write_credential` also clears the managed-key axis. Decision 6 adds a file-only writer.
It holds M3a's storage-write lock and follows M3a's rule there: the entry is read again under the
lock, and its machine-shared keys are rebased from that read when it finds the entry.

Step 2's read needs nothing new. The engine reads the profile's credential as the profile holds
it with M4a's `read_profile_credential(env, dir, spelling)` (Decision 22): the item named from
the recorded spelling, then the file in the actual directory. Task 9 calls it.

**Readings of the spec this task commits to:**
- **The composition** is `shape::compose` with the vault's bytes as the target and the profile's
  own credential as the live object.
  - Account-scoped keys come from the vault: `claudeAiOauth`, `trustedDeviceToken` and unknown
    siblings such as `designOauth` (Appendix A.4).
  - Machine-shared keys come from the profile, and so does their absence. A new profile
    (`None`) gets none: the vault's copy is from another home, and a rotating MCP token must
    never have two copies.
  - Profile bytes that are not a JSON object, empty ones included, are refused, as the live
    store's are (§9.4 step 3). The engine has already refused an unreadable or degraded read
    (step 2).
- **The write's order** (Decision 6, §9.1):
  1. The profile `Env` (M4a's `profile_env` for the spelling it is given, no
     `CLAUDE_SECURESTORAGE_CONFIG_DIR`). The bootstrap gives the current spelling, the actual
     directory's canonical path, so every path resolves inside the profile as it is today.
  2. The profile's credential locks through `lock_credentials`: `<profile>/.oauth_refresh.lock`,
     then the legacy lock `sessions/<id>.lock`.
  3. A read of the entry under them (`LiveStore::snapshot`). That read is the operation's "last
     read", which the storage-write lock's re-read is held to. M3a records it per operation, and
     an operation that read nothing would have nothing to compare.
  4. `write_credential_file` takes the storage-write lock (`<profile>/.storage-write`), reads
     every place of the entry again, and writes the file.
  5. The credential locks drop on return. M3a's `OperationLocks` then ends the operation, so a
     later default-home operation compares against its own reads.
- **Machine-shared keys under the lock.** They are taken from the re-read under the
  storage-write lock (M3a's rebase) when the re-read finds the entry. That is the profile's own
  credential, as step 4 says, and a CC write since the engine's read (an MCP token refresh) is not
  lost (§9.1). Their absence in a present entry is taken too.
- **An entry that is absent keeps the composed keys** (Decisions 6 and 22). Absent under the lock,
  and absent at every place of the operation's last read, means Claude Code wrote nothing there
  since, so there is nothing to rebase from. M3a's rebase would strip them instead. That matters
  after the data directory moved: on macOS the old spelling's item can be the only copy of the
  profile's credential and MCP tokens, since CC migrated the file into it. The write runs under
  the current spelling, whose item and file are both absent. The account-scoped check is
  unchanged and runs first: an entry absent now whose last read held account-scoped keys is
  still `EntryMoved`. One whose last read held only machine-shared keys (CC deleted it since)
  passes the check and still rebases, to their absence.
- **The Keychain is read, never written or deleted.** On macOS the re-read includes the profile's
  hashed item, because CC reads it first and the rebase must see what CC sees. The Interface
  Contract says so: `write_credential_file` "never writes or deletes a Keychain item", and
  reads the profile's item under the storage-write lock, as M3a's re-read requires.
- **Lock waits honour `env.cancel`.** Before the spawn, every lock wait is a cancellation point
  (§12.5). When a wait ends, nothing has been written, so this is not a critical span. M3a's
  Ruling 2 left the profile bootstrap's token to M4.
- **Never under the default home's credential locks.** M3a's per-operation ledger in
  `LiveStore` is shared by one `ClaudeCode` (M3a Decision 14), so `write_profile_credential`
  must never run while this process holds the default home's credential locks. The bootstrap
  holds only `MutationGuard` and the account lock, so it never does. The profile's legacy
  lock, `sessions/<id>.lock`, is a sibling of the profile: this write creates and removes it,
  and nothing else of the profile lives outside its directory.
- **0600.** `write_atomic_private_with` forces 0600, even over a file whose mode was widened
  (§5, B.33).
- **FakeAgent.**
  - `compose_profile_credential` is its `shape::compose` (the `device` key).
  - `write_profile_credential` writes `<spelling>/credential.json` at 0600 under the profile's own
    `.live.lock`. FakeAgent's spelling is its directory as is (M4a's `profile_spelling`), so it
    resolves through M4a's `profile_env_in`. Its `device` key is taken from that file under the
    lock, since FakeAgent has no storage-write lock.

**Files:**
- Modify: `crates/tagteam-provider/src/provider.rs` (two methods at the end of
  `trait Provider`, after Task 5's `merge_back`)
- Modify: `crates/tagteam-cc/src/live.rs` (`LiveStore::write_credential_file` and the private
  `read_absent`, after M3a's `write_credential_entry` and `write_entry`; today
  `write_credential_entry` is lines 337–376)
- Modify: `crates/tagteam-cc/src/provider.rs` (`entry_object` after `fresh_live_object`, today
  lines 98–112; two methods at the end of `impl Provider for ClaudeCode`, after Task 5's
  `merge_back`)
- Modify: `crates/tagteam-fake/src/provider.rs` (`credential_object` beside Task 5's helpers;
  two methods after Task 5's `merge_back`)
- Test: `crates/tagteam-cc/tests/session.rs` (the module `profile_credential`),
  `crates/tagteam-fake/tests/provider.rs` (the module `profile_credential`)

**Interfaces:**

M3a's and M4a's code is not on this branch yet. Everything named here from them is written
against their plans' Interface Contracts and task code. M3a Task 11 has since landed on
`m3-auto-switch` (`05a1ff1`), and the names below match that code.

- Consumes:
  - M3a Task 11 (`tagteam-cc/src/live.rs`):
    - `LiveStore::under_storage_write(&self, env, paths, kind: ItemKind, fence, write: impl FnOnce(Option<&[u8]>, Fence<'_>) -> Result<T, ProviderError>)`
      (private);
    - the private helpers `rebase(bytes: &[u8], shared: &Map<String, Value>) -> Vec<u8>` and
      `shared_of(bytes: Option<&[u8]>) -> Map<String, Value>`;
    - `write_file`'s `intend`/`settle_file` version;
    - `snapshot`, which records the operation's read;
    - the private ledger `LiveStore.seen: Mutex<Ledger>`, `Ledger::entry(kind)` and
      `Seen.places` (per place, what `snapshot` read, then each value written since);
    - `ProviderError::EntryMoved(String)`;
    - `CcPaths.storage_write_lock`.
  - M3a Tasks 2 and 7: `ClaudeCode::lock_credentials` waits under `env.cancel` and returns
    `OperationLocks`, whose `Drop` calls `LiveStore::end_operation`.
  - M3a: `MutationGuard::acquire(env, timeout)`.
  - M4a:
    - `session::profile_env`;
    - `LiveStore`'s single-item reads (`keychain_service` alone, Task 5);
    - the cc test fixtures `Fx`, `fx`, `fx_on`, `profile` and `hashed`;
    - FakeAgent's private `profile_env_in(env: &Env, dir: &Path) -> Env` (M4a's Decision 19).
  - Task 5: the root test helper `hold(path, for_ms)`.
  - Existing:
    - `shape::compose(target: &[u8], live: Option<&Map<String, Value>>) -> Result<Vec<u8>, ProviderError>`;
    - `live::UNPARSABLE_ENTRY`;
    - `Provider::lock_credentials` and `CredLocks::check_owned`;
    - FakeAgent's `shape::compose`, `read_file`, `present_or_err` and `lock_credentials`.
- Produces (Interface Contract):
  - `LiveStore::write_credential_file(&self, env: &Env, paths: &CcPaths, bytes: &[u8], fence: Fence<'_>) -> Result<(), ProviderError>`,
    which keeps the composed machine-shared keys of an entry absent at its last read and under
    the lock.
  - `Provider::write_profile_credential(&self, env: &Env, spelling: &str, guard: &MutationGuard, bytes: &[u8]) -> Result<(), ProviderError>`.
  - `Provider::compose_profile_credential(&self, vault: &[u8], profile: Option<&[u8]>) -> Result<Vec<u8>, ProviderError>`.
  - For Task 9 (bootstrap step 4): the caller passes the profile credential it read in step 2
    with M4a's `read_profile_credential(env, profile, recorded)`, as `Some(bytes)`, or `None`
    when the read was `Absent`.

**Spec:**
- §12.3 step 4:
  - compose and write `<profile>/.credentials.json` at 0600, under the profile's own credential
    locks and storage-write lock;
  - account-scoped keys come from the vault's current generation, machine-shared keys from the
    profile's own credential, and so does their absence;
  - a new profile has none;
  - tagteam never writes the profile's item.
- §9.1 "The storage-write lock":
  - a leaf lock, taken only under the credential locks (for a profile, its own), around one
    entry's write;
  - under it the entry is read again, and its account-scoped keys must equal what the writer last
    read or wrote, unless CC's dead-token marking is the only difference;
  - the machine-shared keys come from that read.
- §12.3 step 2 and §12.2 "One spelling", which the absent-entry rule serves: the profile's
  credential is read under the recorded spelling, hashed item first, then the file (M4a's read);
  the next bootstrap after a move records the new spelling and deletes the old item.
- Appendix A.4: the machine-shared keys and the account-scoped ones, unknown siblings included.
- Appendix A.3: CC moves `.credentials.json` into its item on its next write.
- §5: a file holding a secret is created 0600.
- Decisions 6 and 22.

- [ ] **Step 1: Write the failing tests**

**`crates/tagteam-cc/tests/session.rs`**, at the end of the file:

```rust
mod profile_credential {
    //! §12.3 step 4: the bootstrap's credential, composed from the vault and the profile's own
    //! credential, written to `<profile>/.credentials.json` alone.

    use super::*;
    use std::sync::Mutex;
    use std::time::Instant;

    use serde_json::Value;
    use tagteam_provider::{KeychainError, LockState, MutationGuard};

    /// The vault's current generation: account-scoped keys, an unknown sibling among them
    /// (Appendix A.4), and machine-shared keys from another home that a profile must never get.
    fn vault() -> Vec<u8> {
        json!({
            "claudeAiOauth": {"accessToken": "at-v", "refreshToken": "rt-v", "expiresAt": 9},
            "trustedDeviceToken": "device-v",
            "designOauth": {"t": "v"},
            "mcpOAuth": {"srv": {"token": "stale-from-another-home"}},
            "pluginSecrets": {"p": "stale"}
        })
        .to_string()
        .into_bytes()
    }

    /// The vault's account-scoped keys alone.
    fn account_keys() -> Value {
        json!({
            "claudeAiOauth": {"accessToken": "at-v", "refreshToken": "rt-v", "expiresAt": 9},
            "trustedDeviceToken": "device-v",
            "designOauth": {"t": "v"}
        })
    }

    /// The profile's own credential: an older generation, and its own MCP token.
    fn profile_cred(mcp: &str) -> Vec<u8> {
        json!({
            "claudeAiOauth": {"accessToken": "at-p", "refreshToken": "rt-p"},
            "trustedDeviceToken": "device-p",
            "mcpOAuth": {"srv": {"token": mcp}}
        })
        .to_string()
        .into_bytes()
    }

    fn parsed(b: &[u8]) -> Value {
        serde_json::from_slice(b).unwrap()
    }

    fn guard(f: &Fx) -> MutationGuard {
        MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap()
    }

    /// Every Keychain call, and whether each watched lock directory was held at that moment.
    struct CallLog {
        inner: Arc<FakeKeychain>,
        watch: Mutex<Vec<PathBuf>>,
        calls: Mutex<Vec<(&'static str, String, Vec<bool>)>>,
    }

    impl CallLog {
        fn record(&self, op: &'static str, svc: &str) {
            let held = self.watch.lock().unwrap().iter().map(|p| p.is_dir()).collect();
            self.calls.lock().unwrap().push((op, svc.to_owned(), held));
        }
    }

    impl Keychain for CallLog {
        fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
            self.record("find", s);
            self.inner.find(s, a)
        }
        fn exists(&self, s: &str, a: &str) -> Read<()> {
            self.record("exists", s);
            self.inner.exists(s, a)
        }
        fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
            self.record("upsert", s);
            self.inner.upsert(s, a, d)
        }
        fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
            self.record("delete", s);
            self.inner.delete(s, a)
        }
        fn lock_state(&self) -> LockState {
            self.inner.lock_state()
        }
        fn unlock(&self) -> bool {
            self.inner.unlock()
        }
    }

    /// A fixture whose Claude Code logs every Keychain call it makes.
    fn logged(platform: Platform) -> (Fx, Arc<CallLog>) {
        let f = fx_on(platform);
        let log = Arc::new(CallLog {
            inner: f.kc.clone(),
            watch: Mutex::new(vec![]),
            calls: Mutex::new(vec![]),
        });
        let cc = ClaudeCode::with_store(
            LiveStore::new(log.clone(), platform).with_retry_delay(Duration::ZERO),
        );
        (Fx { cc, ..f }, log)
    }

    #[test]
    fn a_new_profile_gets_the_vault_s_account_keys_and_no_machine_shared_ones() {
        let f = fx();
        let out = f.cc.compose_profile_credential(&vault(), None).unwrap();
        assert_eq!(parsed(&out), account_keys(), "the vault's stale MCP keys never reach it");
    }

    #[test]
    fn an_existing_profile_keeps_its_own_machine_shared_keys_and_their_absence() {
        let f = fx();
        let own = profile_cred("profile-mcp");
        let out = parsed(&f.cc.compose_profile_credential(&vault(), Some(&own)).unwrap());
        let mut want = account_keys();
        want["mcpOAuth"] = json!({"srv": {"token": "profile-mcp"}});
        assert_eq!(
            out, want,
            "the profile's MCP token, and no pluginSecrets, which the profile does not hold"
        );
    }

    #[test]
    fn a_credential_that_is_not_a_json_object_is_refused() {
        let f = fx();
        let v = vault();
        for (vault, profile) in [
            (&b"not json"[..], None),
            (&v[..], Some(&b"[1]"[..])),
            (&v[..], Some(&b""[..])),
        ] {
            assert!(matches!(
                f.cc.compose_profile_credential(vault, profile),
                Err(ProviderError::Invalid(_))
            ));
        }
    }

    #[test]
    fn the_write_takes_the_profile_s_locks_never_the_default_s_and_only_reads_the_keychain() {
        let (f, log) = logged(Platform::MacOs);
        let (dir, spelling) = profile(&f, "0192");
        let acct = keychain_account(&f.env);
        let item = hashed("Claude Code-credentials", &spelling);
        let managed = hashed("Claude Code", &spelling);
        // The profile's own item holds its credential: CC reads it first (§12.3 step 2).
        f.kc.put(&item, &acct, &profile_cred("item-mcp"));
        let canonical = PathBuf::from(&spelling);
        let mut legacy = canonical.clone().into_os_string();
        legacy.push(".lock");
        let default = CcPaths::resolve(&f.env);
        *log.watch.lock().unwrap() = vec![
            canonical.join(".oauth_refresh.lock"),
            PathBuf::from(legacy),
            canonical.join(".storage-write"),
            default.refresh_lock.clone(),
            default.legacy_lock(),
        ];
        let bytes =
            f.cc.compose_profile_credential(&vault(), Some(&profile_cred("item-mcp")))
                .unwrap();

        f.cc.write_profile_credential(&f.env, &spelling, &guard(&f), &bytes)
            .unwrap();

        let file = dir.join(".credentials.json");
        assert_eq!(parsed(&fs::read(&file).unwrap()), parsed(&bytes));
        assert_eq!(mode(&file), 0o600);
        assert_eq!(
            f.kc.get(&item, &acct).unwrap(),
            profile_cred("item-mcp"),
            "the profile's item is left for step 5 to delete"
        );
        assert_eq!(f.kc.items().len(), 1, "no item was created");
        let calls = log.calls.lock().unwrap().clone();
        let (last, earlier) = calls.split_last().expect("the entry was read");
        for (op, svc, held) in &calls {
            assert_eq!(*op, "find", "the Keychain is only ever read: {calls:?}");
            assert!(svc == &item || svc == &managed, "only the profile's items: {svc}");
            assert_eq!(held[..2], [true, true], "under the profile's credential locks: {svc}");
            assert_eq!(held[3..], [false, false], "never the default home's: {svc}");
        }
        assert_eq!(
            (last.1.as_str(), last.2[2]),
            (item.as_str(), true),
            "the last read is the re-read under the storage-write lock"
        );
        assert!(earlier.iter().all(|c| !c.2[2]), "which is held for the write alone");
        for lock in log.watch.lock().unwrap().iter() {
            assert!(!lock.exists(), "{} is released", lock.display());
        }
    }

    #[test]
    fn on_linux_the_write_is_the_file_alone_at_0600() {
        let (f, log) = logged(Platform::Linux);
        let (dir, spelling) = profile(&f, "0192");
        let file = dir.join(".credentials.json");
        fs::write(&file, profile_cred("old")).unwrap();
        fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        let bytes =
            f.cc.compose_profile_credential(&vault(), Some(&profile_cred("old")))
                .unwrap();

        f.cc.write_profile_credential(&f.env, &spelling, &guard(&f), &bytes)
            .unwrap();

        assert_eq!(parsed(&fs::read(&file).unwrap()), parsed(&bytes));
        assert_eq!(mode(&file), 0o600, "a secret file is 0600, whatever it was");
        assert!(log.calls.lock().unwrap().is_empty(), "no Keychain call at all");
    }

    #[test]
    fn an_mcp_token_cc_wrote_since_the_composition_is_kept() {
        let f = fx_on(Platform::Linux);
        let (dir, spelling) = profile(&f, "0192");
        let file = dir.join(".credentials.json");
        fs::write(&file, profile_cred("mcp-1")).unwrap();
        let bytes =
            f.cc.compose_profile_credential(&vault(), Some(&profile_cred("mcp-1")))
                .unwrap();
        // CC refreshes an MCP token in the profile between the read and the write (§9.1).
        fs::write(&file, profile_cred("mcp-2")).unwrap();

        f.cc.write_profile_credential(&f.env, &spelling, &guard(&f), &bytes)
            .unwrap();

        let out = parsed(&fs::read(&file).unwrap());
        assert_eq!(out["mcpOAuth"], json!({"srv": {"token": "mcp-2"}}));
        assert_eq!(out["claudeAiOauth"]["refreshToken"], json!("rt-v"));
    }

    #[test]
    fn the_write_waits_for_cc_s_storage_write_lock_in_the_profile() {
        let f = fx();
        let (dir, spelling) = profile(&f, "0192");
        let bytes = f.cc.compose_profile_credential(&vault(), None).unwrap();
        let cc_writing = hold(&dir.join(".storage-write"), 300);
        let start = Instant::now();

        f.cc.write_profile_credential(&f.env, &spelling, &guard(&f), &bytes)
            .unwrap();

        assert!(start.elapsed() >= Duration::from_millis(300));
        cc_writing.join().unwrap();
        assert_eq!(parsed(&fs::read(dir.join(".credentials.json")).unwrap()), account_keys());
    }

    #[test]
    fn a_held_default_home_lock_never_delays_the_write() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        let default = CcPaths::resolve(&f.env);
        for lock in [
            default.refresh_lock.clone(),
            default.legacy_lock(),
            default.config_lock.clone(),
            default.storage_write_lock.clone(),
        ] {
            fs::create_dir(lock).unwrap();
        }
        let bytes = f.cc.compose_profile_credential(&vault(), None).unwrap();
        let start = Instant::now();

        f.cc.write_profile_credential(&f.env, &spelling, &guard(&f), &bytes)
            .unwrap();

        assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
    }

    #[test]
    fn an_entry_absent_at_the_read_and_under_the_lock_keeps_the_composed_machine_shared_keys() {
        // Decision 22: Claude Code wrote nothing there, so there is nothing to rebase from. After
        // a move, the keys came from the old spelling's item, the only copy left.
        for platform in [Platform::MacOs, Platform::Linux] {
            let f = fx_on(platform);
            let (dir, spelling) = profile(&f, "0192");
            let bytes =
                f.cc.compose_profile_credential(&vault(), Some(&profile_cred("old-item-mcp")))
                    .unwrap();

            f.cc.write_profile_credential(&f.env, &spelling, &guard(&f), &bytes)
                .unwrap();

            let out = parsed(&fs::read(dir.join(".credentials.json")).unwrap());
            assert_eq!(
                out["mcpOAuth"],
                json!({"srv": {"token": "old-item-mcp"}}),
                "{platform:?}"
            );
            assert_eq!(out["claudeAiOauth"]["refreshToken"], json!("rt-v"), "{platform:?}");
        }
    }

    #[test]
    fn an_entry_present_under_the_lock_still_rebases_its_machine_shared_keys_and_their_absence() {
        // §9.1: the keys the entry holds now win over the composed ones, absence included.
        let f = fx_on(Platform::Linux);
        let (dir, spelling) = profile(&f, "0192");
        let file = dir.join(".credentials.json");
        fs::write(&file, account_keys().to_string()).unwrap();
        let bytes =
            f.cc.compose_profile_credential(&vault(), Some(&profile_cred("composed-mcp")))
                .unwrap();

        f.cc.write_profile_credential(&f.env, &spelling, &guard(&f), &bytes)
            .unwrap();

        let out = parsed(&fs::read(&file).unwrap());
        assert_eq!(out.get("mcpOAuth"), None, "the entry holds none, so none are written");
        assert_eq!(out, account_keys());
    }
}
```

The read of a moved profile (the old spelling's item, then the file where the profile is) is
M4a's, and so are its tests (M4a Task 7, `a_moved_profile_is_read_from_its_old_spelling_s_item_and_from_its_files_where_it_is`).

**`crates/tagteam-fake/tests/provider.rs`**, at the end of the file:

```rust
mod profile_credential {
    //! §12.3 step 4 in FakeAgent's shape: the `device` key is its machine-shared one.

    use super::*;

    fn vault() -> Vec<u8> {
        serde_json::to_vec(
            &json!({"fa": {"token": "tok-v", "renew": "renew-v"}, "device": {"id": "stale-elsewhere"}}),
        )
        .unwrap()
    }

    #[test]
    fn the_composition_takes_the_device_key_from_the_profile_and_its_absence_too() {
        let f = fx();
        let new: Value =
            serde_json::from_slice(&f.fake.compose_profile_credential(&vault(), None).unwrap())
                .unwrap();
        assert_eq!(new, json!({"fa": {"token": "tok-v", "renew": "renew-v"}}));
        let own = serde_json::to_vec(&credential_json("tok-p", Some("renew-p"), None)).unwrap();
        let kept: Value = serde_json::from_slice(
            &f.fake
                .compose_profile_credential(&vault(), Some(&own))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            kept,
            json!({"fa": {"token": "tok-v", "renew": "renew-v"}, "device": {"id": "machine-shared"}})
        );
        assert!(matches!(
            f.fake.compose_profile_credential(&vault(), Some(b"[1]")),
            Err(ProviderError::Invalid(_))
        ));
    }

    #[test]
    fn the_write_is_the_profile_s_file_at_0600_under_its_own_lock() {
        let f = fx();
        let profile = f.env.data_dir().join("sessions/0193");
        fs::create_dir_all(&profile).unwrap();
        let spelling = profile.to_str().unwrap();
        // The outer home's lock stays held: a write that took it would time out.
        fs::create_dir_all(FakePaths::resolve(&f.env).lock).unwrap();
        let file = profile.join("credential.json");
        fs::write(
            &file,
            serde_json::to_vec(&json!({"fa": {"token": "old"}, "device": {"id": "profile-device"}}))
                .unwrap(),
        )
        .unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
        let bytes = f.fake.compose_profile_credential(&vault(), None).unwrap();

        f.fake
            .write_profile_credential(&f.env, spelling, &g, &bytes)
            .unwrap();

        assert_eq!(
            file_json(&file),
            json!({"fa": {"token": "tok-v", "renew": "renew-v"}, "device": {"id": "profile-device"}}),
            "the device key the profile holds now"
        );
        assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(!profile.join(".live.lock").exists(), "its lock is released");
        // Its own lock held: the write waits for it, then gives up.
        fs::create_dir(profile.join(".live.lock")).unwrap();
        assert!(matches!(
            f.fake.write_profile_credential(&f.env, spelling, &g, &bytes),
            Err(ProviderError::Lock(LockError::Timeout(_)))
        ));
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-cc --test session profile_credential::`
Expected: compile errors only: E0599, no method named `compose_profile_credential` or
`write_profile_credential` found for struct `ClaudeCode`.

Run: `cargo test -p tagteam-fake --test provider profile_credential::`
Expected: the same, for `FakeAgent`.

- [ ] **Step 3: Implement**

**`crates/tagteam-provider/src/provider.rs`.** At the end of `trait Provider`, after Task 5's
`merge_back`, add:

```rust
    /// §12.3 step 4: writes the profile's credential file, composed, under the profile's own
    /// credential locks and storage-write lock (Decision 6). Never writes a Keychain item.
    fn write_profile_credential(&self, env: &Env, spelling: &str, guard: &MutationGuard, bytes: &[u8]) -> Result<(), ProviderError>;
    /// §12.3 step 4's composition: account-scoped keys of `vault`, machine-shared keys of
    /// `profile` (none for a new profile).
    fn compose_profile_credential(&self, vault: &[u8], profile: Option<&[u8]>) -> Result<Vec<u8>, ProviderError>;
```

**`crates/tagteam-cc/src/live.rs`.** After M3a's `write_entry` (the private body of
`write_credential_entry`), add:

```rust
    /// §12.3 step 4: `bytes` to `<secure-storage dir>/.credentials.json` alone, at 0600 and
    /// atomically, under CC's storage-write lock (§9.1). Under the lock the entry is read again
    /// as CC reads it. Its account-scoped keys must still be what this operation last read or
    /// wrote (`EntryMoved` otherwise). When the entry is there, the machine-shared keys written
    /// are the ones it holds now, their absence included. When it is absent, and was absent at
    /// the operation's last read, CC wrote nothing there, so the composed keys stand (Decision
    /// 22): after a move they may come from an item an older spelling names, the only copy. It
    /// never writes or deletes a Keychain item: a profile's item is CC's to create, and
    /// tagteam's only to delete (§12.3 step 5). The lock wait honours `env.cancel`.
    pub fn write_credential_file(
        &self,
        env: &Env,
        paths: &CcPaths,
        bytes: &[u8],
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        self.under_storage_write(env, paths, ItemKind::OAuth, fence, |now, fence| {
            let keep = now.is_none() && self.read_absent(ItemKind::OAuth);
            let bytes = if keep {
                bytes.to_vec()
            } else {
                rebase(bytes, &shared_of(now))
            };
            self.write_file(paths, &bytes, fence)
        })
    }

    /// Whether everything this operation read or wrote of `kind`'s entry, at every place, was
    /// absent (§9.1): `snapshot` found nothing, and nothing was written since. `false` when the
    /// operation never read the entry.
    fn read_absent(&self, kind: ItemKind) -> bool {
        let mut seen = self.seen.lock().unwrap();
        seen.entry(kind)
            .places
            .as_ref()
            .is_some_and(|places| places.values().flatten().all(Option::is_none))
    }
```

**`crates/tagteam-cc/src/provider.rs`.** After `fresh_live_object`, add:

```rust
/// A profile's own credential as §12.3 step 4 composes from it: a JSON object, else refused,
/// as a live entry is (§9.4 step 3). Its absence is the caller's `None`.
fn entry_object(bytes: &[u8]) -> Result<Map<String, Value>, ProviderError> {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(o)) => Ok(o),
        _ => Err(ProviderError::Invalid(live::UNPARSABLE_ENTRY.into())),
    }
}
```

At the end of `impl Provider for ClaudeCode`, after Task 5's `merge_back`, add:

```rust
    /// §12.3 step 4 (Decision 6), under the profile's own credential locks:
    /// 1. a read of its entry, the operation's last read (§9.1);
    /// 2. then the file alone, under its storage-write lock.
    ///
    /// Releasing the locks ends the operation.
    fn write_profile_credential(&self, env: &Env, spelling: &str, guard: &MutationGuard, bytes: &[u8]) -> Result<(), ProviderError> {
        let profile = session::profile_env(env, spelling);
        let paths = CcPaths::resolve(&profile);
        let held = self.lock_credentials(&profile, guard, self.lock_budget)?;
        let fence = || held.check_owned().map_err(ProviderError::from);
        // The read the storage-write lock's re-read is held to; nothing else uses it.
        self.live.snapshot(&profile, &paths)?;
        self.live
            .write_credential_file(&profile, &paths, bytes, &fence)
    }

    fn compose_profile_credential(&self, vault: &[u8], profile: Option<&[u8]>) -> Result<Vec<u8>, ProviderError> {
        let live = profile.map(entry_object).transpose()?;
        shape::compose(vault, live.as_ref())
    }
```

**`crates/tagteam-fake/src/provider.rs`.** Beside Task 5's helpers, before `showable_token`,
add:

```rust
/// A FakeAgent credential as a JSON object, else refused, as `write_credential` refuses one.
fn credential_object(bytes: &[u8]) -> Result<serde_json::Map<String, Value>, ProviderError> {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(o)) => Ok(o),
        _ => Err(ProviderError::Invalid(
            "FakeAgent's live credential is not a JSON object".into(),
        )),
    }
}
```

At the end of `impl Provider for FakeAgent`, after Task 5's `merge_back`, add:

```rust
    /// §12.3 step 4 in FakeAgent's shape: `<spelling>/credential.json` at 0600 under the
    /// profile's own `.live.lock`, carrying the `device` key the profile holds now. FakeAgent's
    /// spelling is its directory as is (`profile_spelling`). It has no storage-write lock, so it
    /// reads the file again under that lock.
    fn write_profile_credential(&self, env: &Env, spelling: &str, guard: &MutationGuard, bytes: &[u8]) -> Result<(), ProviderError> {
        let profile = profile_env_in(env, Path::new(spelling));
        let held = self.lock_credentials(&profile, guard, self.lock_budget)?;
        let p = FakePaths::resolve(&profile);
        let now = present_or_err(read_file(&p.credential))?
            .map(|b| credential_object(&b))
            .transpose()?;
        let composed = shape::compose(bytes, now.as_ref())?;
        ensure_private_dir(&p.dir)?;
        write_atomic_private_with(&p.credential, &composed, 0o600, || {
            held.check_owned().map_err(ProviderError::from)
        })
    }

    fn compose_profile_credential(&self, vault: &[u8], profile: Option<&[u8]>) -> Result<Vec<u8>, ProviderError> {
        let live = profile.map(credential_object).transpose()?;
        shape::compose(vault, live.as_ref())
    }
```

- [ ] **Step 4: Run them and see them pass**, then the crates' whole suites

Run: `cargo test -p tagteam-cc --test session profile_credential::`
Expected: PASS, 10 tests. The trailing `::` keeps M4a's root tests whose names hold
`profile_credential` out of the count.

Run: `cargo test -p tagteam-fake --test provider profile_credential::`
Expected: PASS, 2 tests.

Run: `cargo test -p tagteam-provider -p tagteam-cc -p tagteam-fake`
Expected: PASS. The cc `live_store` tests are unaffected, because `write_credential_entry` is
unchanged.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider/src/provider.rs crates/tagteam-cc/src/live.rs \
  crates/tagteam-cc/src/provider.rs crates/tagteam-cc/tests/session.rs \
  crates/tagteam-fake/src/provider.rs crates/tagteam-fake/tests/provider.rs
git commit -m "Write a profile's bootstrap credential to its file alone, composed from the vault"
```

---

### Task 7: Session environment and validation

Two provider facts that the launch (Task 10) and the login check (Task 11) apply identically to
`claude` and to `claude auth status`:
- **The session environment** (Decision 9, §12.5 "Environment"). It sets `CLAUDE_CONFIG_DIR` to
  the recorded spelling. It scrubs every variable that supplies or redirects the login, or renames
  CC's config file or Keychain item.
- **Validation** (Decision 10, §12.3 step 8).
  - It runs `claude auth status --json` through the injected `ProcessSpawner`, in exactly that
    environment and the directory `claude` will run in, with a 10 s timeout. A signal ends the
    wait and kills the process (§12.5).
  - The reply is read into §12.3's six outcomes.
  - Engine tests script the spawn with `ScriptedSpawner`, so no test runs a binary.

**Readings of the spec this task commits to:**
- **The scrub list.**
  - `remove` is §12.5's literal names in the spec's order, then every
    `CLAUDE_CODE_*_FILE_DESCRIPTOR` that the process environment holds, sorted by bytes.
  - The `*` is any run of bytes, possibly empty, compared as bytes, so a name that is not UTF-8
    is still matched.
  - Literal names are removed whether set or not: removing an unset variable changes nothing.
  - The warnings come from the engine (Task 10, Decision 18): one for each removed name that is
    set, and one for a pre-set `CLAUDE_CONFIG_DIR`. The trait signature takes no environment, so the CC
    method reads `std::env::vars_os()`. Tests reach the expansion through the crate-private
    `session::session_env(spelling, names)`, in-module.
- **The spawn.**
  - `program` is the `program` argument: the launch command `plan_run` resolved on `PATH`
    (§12.1), an absolute path. The check therefore runs the binary the session will, and never
    `claude` by name (Decision 20).
  - `args` are `auth status --json`.
  - `set` and `remove` are exactly `session_env(spelling)`. The rest of the process environment
    is inherited, as `claude`'s will be.
  - `cwd` is the session's directory, since a project's own settings there can override the
    login.
  - A token that is already set spawns nothing.
- **The rows, tried in this order:**
  1. `configDirectory` differs from the spelling: `drifted`. The reply is about another config
     dir, so nothing else in it speaks for this profile, and refusing keeps the profile.
  2. `loggedIn` false or `authMethod` `none`: `invalid` ("not logged in").
  3. Any `authMethod` other than `claude.ai`: `overridden`, with `apiKeySource` when present.
     That covers §12.3's four names, and any later method, since it is not the account's own
     login.
  4. A `claude.ai` login whose `email` is another one: `invalid`. One whose `orgId` is another,
     when both name an org (`""` counts as none): `invalid`.
  5. Exit 0: `valid`. A reply that confirms this login but did not exit 0 is `unknown`.
- **Unknown, never invalid, when nothing is confirmed.** Only `invalid` deletes a profile
  (§12.3). So a `claude.ai` reply with no `email` is `unknown`, as are:
  - output that is not one JSON object with `loggedIn` (a bool) and `authMethod` and
    `configDirectory` (strings), named with how the process ended;
  - a timeout.
- **`unreachable`** carries the spawn's failure.
- **Interruption.** `Captured::Interrupted` is `Unknown("interrupted")`, which the caller maps
  by checking the token (§12.5).
- **No email in any detail** (§4.4).
- **FakeAgent.**
  - Its session sets `FAKEAGENT_HOME` and scrubs `FAKEAGENT_TOKEN`: one of each, unlike CC's list.
  - Its check never spawns. It is `valid` when the profile holds a credential with a token and
    the account's identity, and `invalid` without either or with another identity. It is
    `unknown` when a file cannot be read or the token is set.
  - `drifted`, `overridden` and `unreachable` come only from CC with a scripted spawner.

**Files:**
- Create: `crates/tagteam-cc/tests/fixtures/auth-status/claude-ai.json`
- Modify: `crates/tagteam-provider/src/provider.rs`
  - imports: `std::ffi::OsString` beside today's line 1, and `crate::process::ProcessSpawner`
    beside Task 5's `crate::cancel::Cancel`;
  - `SessionEnv` and `Validity` after Task 5's `MergeReport`;
  - two methods at the end of `trait Provider`, after Task 6's `compose_profile_credential`.
- Modify: `crates/tagteam-provider/src/lib.rs` (the `provider` re-export)
- Modify: `crates/tagteam-cc/src/session.rs`
  - imports;
  - `CC_SCRUB`, `session_env`, `AUTH_STATUS_TIMEOUT`, `INTERRUPTED`, `AuthStatus` and `validity`
    after Task 5's `merge_back`;
  - a `#[cfg(test)] mod tests` at the end.
- Modify: `crates/tagteam-cc/src/provider.rs`
  - imports: `std::ffi::OsString`, `tagteam_provider::process::{ProcessSpawner, SpawnSpec}`,
    `SessionEnv`, `Validity`;
  - two methods after Task 6's `compose_profile_credential`.
- Modify: `crates/tagteam-fake/src/provider.rs`
  - imports;
  - `TOKEN_VAR` beside Task 5's constants;
  - two methods after Task 6's `compose_profile_credential`.
- Test:
  - `crates/tagteam-cc/src/session.rs` (in-module);
  - `crates/tagteam-cc/tests/session.rs` (the module `validation`);
  - `crates/tagteam-fake/tests/provider.rs` (the module `validation`).

**Interfaces:**

M3a's and M4a's code is not on this branch yet. Everything named here from them is written
against their plans' Interface Contracts and task code.

- Consumes:
  - Task 3 (`tagteam_provider::process`):
    - `SpawnSpec { program, args, set, remove, cwd }`;
    - `Captured::{Exited { code, signal, stdout, stderr }, TimedOut, Interrupted(i32), SpawnFailed(String)}`;
    - `ProcessSpawner::run_captured(&self, spec: &SpawnSpec, timeout: Duration, cancel: &Cancel) -> Captured`;
    - `ScriptedSpawner::{new, push, specs}`, ungated, as `FakeProcessProbe` is.
  - Tasks 5 and 6: the `Provider` methods, imports and test modules as they left them; this
    task's code goes after Task 6's `compose_profile_credential`.
  - M3a: `Cancel::{new, request, requested}`.
  - M4a:
    - `session::CONFIG_DIR` (`"CLAUDE_CONFIG_DIR"`) and `profile_env`;
    - FakeAgent's `HOME_VAR`, `profile_env_in(env: &Env, dir: &Path) -> Env`, `read_live_auth`
      and `live_identity` over a profile `Env`;
    - the test fixtures named in Task 5, and the fake tests' `with_home`.
  - Existing: `Provider::{parse_identity, identity_key}`, FakeAgent's `shape::token`, and
    `tagteam_fake::login`.
- Produces (Interface Contract):
  - `tagteam_provider::SessionEnv { set: Vec<(OsString, OsString)>, remove: Vec<OsString> }`.
  - `tagteam_provider::Validity::{Valid, Invalid(String), Overridden { method, source }, Drifted { reported }, Unknown(String), Unreachable(String)}`.
  - `Provider::session_env(&self, spelling: &str) -> SessionEnv`.
  - `Provider::validate_profile(&self, env: &Env, spelling: &str, cwd: &Path, program: &Path, expect: &Identity, spawner: &dyn ProcessSpawner, cancel: &Cancel) -> Validity`.
  - `tagteam_cc::session::CC_SCRUB` (crate-private, exactly §12.5's list).
  - For Tasks 9 and 11: `Validity::Unknown("interrupted")` is the one interrupted outcome. The
    caller turns it into `EngineError::Interrupted` by reading `cancel.requested()`, never by
    matching the text.

**Spec:**
- §12.5 "Environment":
  - the scrubbed variables, `CLAUDE_CODE_*_FILE_DESCRIPTOR` among them;
  - a pre-set `CLAUDE_CONFIG_DIR` is overridden;
  - removing `CLAUDE_SECURESTORAGE_CONFIG_DIR` makes the secure-storage dir the profile;
  - nothing is scrubbed on the plain-`claude` paths (Task 8's `RunPlan::Plain`).
- §12.3 step 8: `claude auth status --json` in exactly the session environment, in the directory
  `claude` will run in, with a 10 s timeout; the six-row table.
- §12.3 "Every launch is checked": the same command after the locks are released.
- §12.5 "Signals": the wait for the login check is a cancellation point, and its process is then
  killed.
- Appendix A.7 (`claude auth status`):
  - its fields;
  - `authMethod`'s values;
  - it exits 0 only when logged in;
  - `apiKeySource` when it applies.
- Decisions 9 and 10.

- [ ] **Step 1: Write the failing tests**

**Create `crates/tagteam-cc/tests/fixtures/auth-status/claude-ai.json`**, the field set Appendix
A.7 lists for a `claude.ai` login:

```json
{
  "rc": 0,
  "stdout": {
    "loggedIn": true,
    "authMethod": "claude.ai",
    "apiProvider": "firstParty",
    "analyticsDisabled": false,
    "projectsDirectory": "/CONFIG_DIRECTORY/projects",
    "configDirectory": "/CONFIG_DIRECTORY",
    "email": "probe1@example.com",
    "orgId": "00000000-0000-4000-8000-000000000002",
    "orgName": "Probe Name 3",
    "subscriptionType": "max"
  },
  "synthetic": true,
  "note": "The fields of `claude auth status --json` for a claude.ai login (Appendix A.7, CC 2.1.286); the values are placeholders, and tests point configDirectory and projectsDirectory at the profile's spelling"
}
```

**`crates/tagteam-cc/src/session.rs`**: at the end of the file, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn every_token_file_descriptor_the_process_holds_is_scrubbed_too() {
        let not_utf8 = OsString::from_vec(b"CLAUDE_CODE_\xff_FILE_DESCRIPTOR".to_vec());
        let present = [
            "PATH",
            "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
            "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
            "CLAUDE_CODE__FILE_DESCRIPTOR",
            "CLAUDE_CODE_FILE_DESCRIPTOR",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "XCLAUDE_CODE_A_FILE_DESCRIPTOR",
            "CLAUDE_CODE_A_FILE_DESCRIPTOR_X",
            "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
        ]
        .map(OsString::from)
        .into_iter()
        .chain([not_utf8.clone()]);

        let env = session_env("/p/0192", present);

        assert_eq!(
            env.set,
            [(
                OsString::from("CLAUDE_CONFIG_DIR"),
                OsString::from("/p/0192")
            )]
        );
        let (literal, expanded) = env.remove.split_at(CC_SCRUB.len());
        assert_eq!(
            literal.to_vec(),
            CC_SCRUB.iter().map(OsString::from).collect::<Vec<_>>()
        );
        assert_eq!(
            expanded,
            [
                OsString::from("CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR"),
                OsString::from("CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR"),
                OsString::from("CLAUDE_CODE__FILE_DESCRIPTOR"),
                not_utf8,
            ],
            "sorted, once each; `CLAUDE_CODE_FILE_DESCRIPTOR` is too short to match"
        );
    }
}
```

**`crates/tagteam-cc/tests/session.rs`**, at the end of the file:

```rust
mod validation {
    //! §12.3 step 8 and §12.5 "Environment": the session environment, and `claude auth status`
    //! read into a `Validity`.

    use super::*;
    use std::sync::Mutex;

    use serde_json::Value;
    use tagteam_provider::process::{Captured, ProcessSpawner, ScriptedSpawner, SpawnSpec};
    use tagteam_provider::{Cancel, Identity, SessionEnv, Validity};

    /// §12.5's list, verbatim, so a change to `CC_SCRUB` cannot hide behind itself.
    const SCRUBBED: [&str; 18] = [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
        "CLAUDE_CODE_OAUTH_SCOPES",
        "CLAUDE_CODE_OAUTH_CLIENT_ID",
        "CLAUDE_CODE_ACCOUNT_UUID",
        "CLAUDE_CODE_USER_EMAIL",
        "CLAUDE_CODE_ORGANIZATION_UUID",
        "ANTHROPIC_PROFILE",
        "ANTHROPIC_CONFIG_DIR",
        "ANTHROPIC_FEDERATION_RULE_ID",
        "ANTHROPIC_IDENTITY_TOKEN",
        "ANTHROPIC_IDENTITY_TOKEN_FILE",
        "CLAUDE_CODE_CUSTOM_OAUTH_URL",
        "USE_LOCAL_OAUTH",
        "USE_STAGING_OAUTH",
        "CLAUDE_SECURESTORAGE_CONFIG_DIR",
    ];

    const EMAIL: &str = "probe1@example.com";
    const ORG: &str = "00000000-0000-4000-8000-000000000002";
    /// The launch command `plan_run` resolved (Decision 20); the scripted spawner never runs it.
    const CLAUDE: &str = "/opt/claude/bin/claude";

    fn account(f: &Fx) -> Identity {
        f.cc.parse_identity(&json!({"emailAddress": EMAIL, "organizationUuid": ORG}))
            .unwrap()
    }

    /// The recorded fields of `claude auth status --json` for a claude.ai login (Appendix
    /// A.7), pointed at `spelling`, with the exit code the fixture records.
    fn logged_in(spelling: &str) -> (i32, Value) {
        let fixture: Value =
            serde_json::from_str(include_str!("fixtures/auth-status/claude-ai.json")).unwrap();
        let mut out = fixture["stdout"].clone();
        out["configDirectory"] = json!(spelling);
        out["projectsDirectory"] = json!(format!("{spelling}/projects"));
        (fixture["rc"].as_i64().unwrap() as i32, out)
    }

    /// A reply logged in another way, or not at all, in the same config dir.
    fn reply_by(spelling: &str, logged_in: bool, method: &str) -> Value {
        json!({
            "loggedIn": logged_in, "authMethod": method, "apiProvider": "firstParty",
            "analyticsDisabled": false, "projectsDirectory": format!("{spelling}/projects"),
            "configDirectory": spelling
        })
    }

    fn exited(code: i32, stdout: &Value) -> Captured {
        Captured::Exited {
            code: Some(code),
            signal: None,
            stdout: serde_json::to_vec_pretty(stdout).unwrap(),
            stderr: vec![],
        }
    }

    /// One login check of `spelling` for `account`, with `reply` scripted.
    fn validate(f: &Fx, spelling: &str, reply: Captured) -> Validity {
        let spawner = ScriptedSpawner::new();
        spawner.push(reply);
        f.cc.validate_profile(
            &f.env,
            spelling,
            Path::new("/work/app"),
            Path::new(CLAUDE),
            &account(f),
            &spawner,
            &Cancel::new(),
        )
    }

    #[test]
    fn the_session_environment_sets_the_spelling_and_scrubs_section_12_5_s_list() {
        let f = fx();
        let env = f.cc.session_env("/data/tagteam/sessions/0192");
        assert_eq!(
            env.set,
            [(
                OsString::from("CLAUDE_CONFIG_DIR"),
                OsString::from("/data/tagteam/sessions/0192")
            )]
        );
        assert_eq!(env.remove[..SCRUBBED.len()], SCRUBBED.map(OsString::from));
        for extra in &env.remove[SCRUBBED.len()..] {
            let name = extra.to_string_lossy();
            assert!(
                name.starts_with("CLAUDE_CODE_") && name.ends_with("_FILE_DESCRIPTOR"),
                "{name}"
            );
            assert!(
                std::env::var_os(extra).is_some(),
                "only a descriptor this process holds: {name}"
            );
        }
    }

    #[test]
    fn the_login_check_runs_auth_status_in_the_session_environment_and_directory() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        let spawner = ScriptedSpawner::new();
        let (rc, reply) = logged_in(&spelling);
        spawner.push(exited(rc, &reply));
        let cwd = f.env.home.join("work/app");

        let got = f.cc.validate_profile(
            &f.env,
            &spelling,
            &cwd,
            Path::new(CLAUDE),
            &account(&f),
            &spawner,
            &Cancel::new(),
        );

        assert_eq!(got, Validity::Valid);
        let specs = spawner.specs();
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        assert_eq!(
            spec.program,
            PathBuf::from(CLAUDE),
            "the launch command plan_run resolved, never `claude` by name"
        );
        assert_eq!(spec.args, ["auth", "status", "--json"].map(OsString::from));
        let SessionEnv { set, remove } = f.cc.session_env(&spelling);
        assert_eq!(
            (&spec.set, &spec.remove),
            (&set, &remove),
            "exactly the session's environment"
        );
        assert_eq!(spec.cwd.as_deref(), Some(cwd.as_path()));
    }

    #[test]
    fn the_login_check_is_given_ten_seconds() {
        #[derive(Default)]
        struct Timed(Mutex<Vec<Duration>>);
        impl ProcessSpawner for Timed {
            fn run_captured(&self, _: &SpawnSpec, timeout: Duration, _: &Cancel) -> Captured {
                self.0.lock().unwrap().push(timeout);
                Captured::TimedOut
            }
        }
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        let timed = Timed::default();

        let got = f.cc.validate_profile(
            &f.env,
            &spelling,
            Path::new("/"),
            Path::new(CLAUDE),
            &account(&f),
            &timed,
            &Cancel::new(),
        );

        assert!(
            matches!(&got, Validity::Unknown(why) if why.contains("10 s")),
            "a timeout is unknown, naming it: {got:?}"
        );
        assert_eq!(*timed.0.lock().unwrap(), [Duration::from_secs(10)]);
    }

    #[test]
    fn valid_needs_claude_ai_this_spelling_this_email_and_this_org_when_both_name_one() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        let (rc, reply) = logged_in(&spelling);
        assert_eq!(validate(&f, &spelling, exited(rc, &reply)), Validity::Valid);
        let mut no_org = reply.clone();
        no_org.as_object_mut().unwrap().shift_remove("orgId");
        assert_eq!(
            validate(&f, &spelling, exited(0, &no_org)),
            Validity::Valid,
            "an org is compared only when both name one"
        );
        let personal =
            f.cc.parse_identity(&json!({"emailAddress": EMAIL, "organizationUuid": null}))
                .unwrap();
        let spawner = ScriptedSpawner::new();
        spawner.push(exited(0, &reply));
        assert_eq!(
            f.cc.validate_profile(
                &f.env,
                &spelling,
                Path::new("/"),
                Path::new(CLAUDE),
                &personal,
                &spawner,
                &Cancel::new()
            ),
            Validity::Valid
        );
    }

    #[test]
    fn invalid_is_logged_out_or_another_account_or_organization() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        assert_eq!(
            validate(&f, &spelling, exited(1, &reply_by(&spelling, false, "none"))),
            Validity::Invalid("not logged in".into())
        );
        let (_, mut other) = logged_in(&spelling);
        other["email"] = json!("someone-else@example.com");
        assert_eq!(
            validate(&f, &spelling, exited(0, &other)),
            Validity::Invalid("logged in to claude.ai as another account".into())
        );
        let (_, mut other_org) = logged_in(&spelling);
        other_org["orgId"] = json!("00000000-0000-4000-8000-000000000099");
        assert_eq!(
            validate(&f, &spelling, exited(0, &other_org)),
            Validity::Invalid("logged in to claude.ai in another organization".into())
        );
        for got in [
            validate(&f, &spelling, exited(0, &other)),
            validate(&f, &spelling, exited(0, &other_org)),
        ] {
            assert!(!format!("{got:?}").contains("example.com"), "no email: {got:?}");
        }
    }

    #[test]
    fn overridden_names_the_method_and_its_source() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        for (method, source) in [
            ("api_key_helper", Some("apiKeyHelper")),
            ("api_key", Some("ANTHROPIC_API_KEY")),
            ("oauth_token", None),
            ("third_party", None),
        ] {
            let mut reply = reply_by(&spelling, true, method);
            if let Some(s) = source {
                reply["apiKeySource"] = json!(s);
            }
            assert_eq!(
                validate(&f, &spelling, exited(0, &reply)),
                Validity::Overridden {
                    method: method.into(),
                    source: source.map(str::to_owned)
                },
                "{method}"
            );
        }
    }

    #[test]
    fn drifted_is_another_config_directory_whatever_else_the_reply_says() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        let (_, mut moved) = logged_in(&spelling);
        moved["configDirectory"] = json!(format!("{spelling}/"));
        assert_eq!(
            validate(&f, &spelling, exited(0, &moved)),
            Validity::Drifted {
                reported: format!("{spelling}/")
            }
        );
        let elsewhere = json!({"loggedIn": false, "authMethod": "none", "configDirectory": "/u/.claude"});
        assert_eq!(
            validate(&f, &spelling, exited(1, &elsewhere)),
            Validity::Drifted {
                reported: "/u/.claude".into()
            },
            "never invalid, which would delete the profile"
        );
    }

    #[test]
    fn unknown_is_a_reply_that_does_not_parse_or_cannot_confirm_the_login() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        for stdout in [
            &b""[..],
            b"Logged in as probe1@example.com\n",
            b"[1]",
            br#"{"loggedIn": true}"#,
            br#"{"loggedIn": "yes", "authMethod": "claude.ai", "configDirectory": "/p"}"#,
        ] {
            let got = validate(
                &f,
                &spelling,
                Captured::Exited {
                    code: Some(0),
                    signal: None,
                    stdout: stdout.to_vec(),
                    stderr: vec![],
                },
            );
            assert!(matches!(got, Validity::Unknown(_)), "{got:?}");
        }
        let killed = validate(
            &f,
            &spelling,
            Captured::Exited {
                code: None,
                signal: Some(9),
                stdout: vec![],
                stderr: vec![],
            },
        );
        assert!(
            matches!(&killed, Validity::Unknown(why) if why.contains("signal 9")),
            "{killed:?}"
        );
        let (_, mut no_email) = logged_in(&spelling);
        no_email.as_object_mut().unwrap().shift_remove("email");
        assert!(
            matches!(validate(&f, &spelling, exited(0, &no_email)), Validity::Unknown(_)),
            "an email that is not there confirms nothing, and must not delete the profile"
        );
        let (_, reply) = logged_in(&spelling);
        assert!(
            matches!(validate(&f, &spelling, exited(1, &reply)), Validity::Unknown(_)),
            "a login that did not exit 0"
        );
    }

    #[test]
    fn unreachable_is_a_spawn_failure() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        assert_eq!(
            validate(
                &f,
                &spelling,
                Captured::SpawnFailed("No such file or directory (os error 2)".into())
            ),
            Validity::Unreachable("No such file or directory (os error 2)".into())
        );
    }

    #[test]
    fn an_interrupted_check_is_unknown_interrupted_and_a_set_token_spawns_nothing() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        assert_eq!(
            validate(&f, &spelling, Captured::Interrupted(2)),
            Validity::Unknown("interrupted".into())
        );
        let spawner = ScriptedSpawner::new();
        let cancel = Cancel::new();
        cancel.request(15);
        assert_eq!(
            f.cc.validate_profile(
                &f.env,
                &spelling,
                Path::new("/"),
                Path::new(CLAUDE),
                &account(&f),
                &spawner,
                &cancel
            ),
            Validity::Unknown("interrupted".into())
        );
        assert!(spawner.specs().is_empty(), "nothing was spawned");
    }
}
```

**`crates/tagteam-fake/tests/provider.rs`**, at the end of the file:

```rust
mod validation {
    //! FakeAgent's session environment, and its login check from its profile files.

    use super::*;
    use std::ffi::OsString;
    use std::path::Path;

    use tagteam_provider::process::ScriptedSpawner;
    use tagteam_provider::{Cancel, SessionEnv, Validity};

    #[test]
    fn its_session_sets_its_home_and_scrubs_its_token() {
        let f = fx();
        assert_eq!(
            f.fake.session_env("/p/0193"),
            SessionEnv {
                set: vec![(OsString::from("FAKEAGENT_HOME"), OsString::from("/p/0193"))],
                remove: vec![OsString::from("FAKEAGENT_TOKEN")],
            }
        );
    }

    #[test]
    fn it_validates_from_its_profile_files_without_spawning() {
        let f = fx();
        let profile = f.env.data_dir().join("sessions/0193");
        fs::create_dir_all(&profile).unwrap();
        let spelling = profile.to_str().unwrap();
        let spawner = ScriptedSpawner::new();
        let identity = |handle: &str| {
            f.fake
                .parse_identity(&identity_json(handle, "ws2", &format!("uid-{handle}")))
                .unwrap()
        };
        let check = |who: &str, cancel: &Cancel| {
            f.fake.validate_profile(
                &f.env,
                spelling,
                &profile,
                Path::new("/opt/fakeagent/bin/fakeagent"),
                &identity(who),
                &spawner,
                cancel,
            )
        };

        assert_eq!(
            check("bob", &Cancel::new()),
            Validity::Invalid("not logged in".into())
        );
        login(&with_home(&f, spelling), "bob", "ws2", "tok-p", "renew-p");
        assert_eq!(check("bob", &Cancel::new()), Validity::Valid);
        assert_eq!(
            check("carol", &Cancel::new()),
            Validity::Invalid("logged in as another identity".into())
        );
        let cancel = Cancel::new();
        cancel.request(2);
        assert_eq!(check("bob", &cancel), Validity::Unknown("interrupted".into()));
        fs::write(profile.join("identity.json"), b"{\"identity\": {").unwrap();
        assert!(matches!(check("bob", &Cancel::new()), Validity::Unknown(_)));
        assert!(spawner.specs().is_empty(), "FakeAgent never spawns");
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-cc --lib session`
Expected: compile errors: E0425, cannot find function `session_env` and value `CC_SCRUB` in this
scope.

Run: `cargo test -p tagteam-cc --test session validation`
Expected: compile errors:
- E0432: unresolved imports `tagteam_provider::SessionEnv` and `tagteam_provider::Validity`;
- E0599: no method named `session_env` or `validate_profile` found for struct `ClaudeCode`.

Run: `cargo test -p tagteam-fake --test provider validation`
Expected: the same errors, for `FakeAgent`.

- [ ] **Step 3: Implement**

**`crates/tagteam-provider/src/provider.rs`.** Add `use std::ffi::OsString;` beside
`use std::fmt;` (line 1), and `use crate::process::ProcessSpawner;` beside Task 5's
`use crate::cancel::Cancel;`. After Task 5's `MergeReport`, add:

```rust
/// §12.5 "Environment": what a session's environment sets and scrubs (§4.5 `session_env`).
/// The engine applies it identically to the login check and to the agent (Decision 9).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionEnv {
    pub set: Vec<(OsString, OsString)>,
    pub remove: Vec<OsString>,
}

/// §12.3 step 8's outcomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Validity {
    Valid,
    Invalid(String),
    Overridden { method: String, source: Option<String> },
    Drifted { reported: String },
    Unknown(String),
    Unreachable(String),
}
```

At the end of `trait Provider`, after Task 6's `compose_profile_credential`, add:

```rust
    /// §12.5 "Environment": what the session's environment sets and scrubs, for `spelling`.
    fn session_env(&self, spelling: &str) -> SessionEnv;
    /// §12.3 step 8 / "Every launch is checked", run with `spawner` in the session's exact
    /// environment and `cwd`, 10 s timeout, cancellable. It spawns `program`, the launch
    /// command `plan_run` resolved, never the launch command by name (Decision 20).
    #[allow(clippy::too_many_arguments)]
    fn validate_profile(&self, env: &Env, spelling: &str, cwd: &Path, program: &Path, expect: &Identity, spawner: &dyn ProcessSpawner, cancel: &Cancel) -> Validity;
```

Its eight inputs, `self` included, pass clippy's `too_many_arguments` threshold of seven, so the
declaration carries the `allow`, as `switch.rs`'s `transact` does. The two implementations need
none: clippy never lints a trait implementation's signature.

**`crates/tagteam-provider/src/lib.rs`**: the `provider` re-export becomes

```rust
pub use provider::{
    BeforeFallback, Capabilities, CapturedLogin, CredLocks, DeadReason, DoomedEntry, EntryKind,
    Identity, IdentitySurface, KindTraits, LiveAuth, LiveChange, LiveLockSet, LiveLocks,
    MergeReport, MustShare, Provider, ProviderError, RefreshResult, SecretStore, SessionEnv,
    SharePolicy, StoredLogin, TransientKind, Undo, UsageResult, Validity, Written,
};
```

**`crates/tagteam-cc/src/session.rs`.** The imports (as Task 5 left them) become:

```rust
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_core::merge::{MergeKey, three_way};
use tagteam_provider::atomic::{write_atomic_private_with, write_atomic_with};
use tagteam_provider::process::Captured;
use tagteam_provider::splice::{self, SpliceError};
use tagteam_provider::{
    Cancel, EntryKind, Env, Identity, LiveLockSet, MergeReport, ProviderError, Read, SessionEnv,
    Validity,
};

use crate::config::read_bytes;
use crate::locks;
use crate::paths::CcPaths;
use crate::provider::CONFIG_REMEDY;
```

After Task 5's `merge_back`, before the test module, add:

```rust
/// §12.5 "Environment": scrubbed from every session, because each supplies or redirects the
/// login, or renames CC's config file or Keychain item (Appendix A.1, A.7). Every
/// `CLAUDE_CODE_*_FILE_DESCRIPTOR` set in the process joins them (`session_env`).
pub(crate) const CC_SCRUB: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
    "CLAUDE_CODE_OAUTH_SCOPES",
    "CLAUDE_CODE_OAUTH_CLIENT_ID",
    "CLAUDE_CODE_ACCOUNT_UUID",
    "CLAUDE_CODE_USER_EMAIL",
    "CLAUDE_CODE_ORGANIZATION_UUID",
    "ANTHROPIC_PROFILE",
    "ANTHROPIC_CONFIG_DIR",
    "ANTHROPIC_FEDERATION_RULE_ID",
    "ANTHROPIC_IDENTITY_TOKEN",
    "ANTHROPIC_IDENTITY_TOKEN_FILE",
    "CLAUDE_CODE_CUSTOM_OAUTH_URL",
    "USE_LOCAL_OAUTH",
    "USE_STAGING_OAUTH",
    "CLAUDE_SECURESTORAGE_CONFIG_DIR",
];

const DESCRIPTOR_PREFIX: &[u8] = b"CLAUDE_CODE_";
const DESCRIPTOR_SUFFIX: &[u8] = b"_FILE_DESCRIPTOR";

/// `CLAUDE_CODE_*_FILE_DESCRIPTOR`, the `*` any run of bytes, possibly empty.
fn is_token_descriptor(name: &OsStr) -> bool {
    let b = name.as_bytes();
    b.len() >= DESCRIPTOR_PREFIX.len() + DESCRIPTOR_SUFFIX.len()
        && b.starts_with(DESCRIPTOR_PREFIX)
        && b.ends_with(DESCRIPTOR_SUFFIX)
}

/// §12.5: `CLAUDE_CONFIG_DIR` set to the recorded spelling. `CC_SCRUB` is removed, then every
/// name in `present` (the process environment's names) that is a token file descriptor,
/// sorted, once each.
pub(crate) fn session_env(
    spelling: &str,
    present: impl IntoIterator<Item = OsString>,
) -> SessionEnv {
    let mut descriptors: Vec<OsString> = present
        .into_iter()
        .filter(|n| is_token_descriptor(n))
        .collect();
    descriptors.sort();
    descriptors.dedup();
    SessionEnv {
        set: vec![(OsString::from(CONFIG_DIR), OsString::from(spelling))],
        remove: CC_SCRUB
            .iter()
            .map(OsString::from)
            .chain(descriptors)
            .collect(),
    }
}

/// `claude auth status`'s limit (§12.3 step 8).
pub(crate) const AUTH_STATUS_TIMEOUT: Duration = Duration::from_secs(10);
/// What an interrupted login check reports (§12.5); the caller maps it by reading the token.
pub(crate) const INTERRUPTED: &str = "interrupted";
/// The account's own login (§12.3 step 8): CC's OAuth accounts, and setup-token accounts
/// (*inferred*).
const CLAUDE_AI: &str = "claude.ai";

/// The fields of `claude auth status --json` that §12.3's table reads (Appendix A.7).
struct AuthStatus {
    logged_in: bool,
    auth_method: String,
    config_directory: String,
    email: Option<String>,
    org_id: Option<String>,
    api_key_source: Option<String>,
}

impl AuthStatus {
    /// `None` unless `stdout` is one JSON object holding the three fields CC always prints.
    fn parse(stdout: &[u8]) -> Option<Self> {
        let v: Value = serde_json::from_slice(stdout).ok()?;
        let text = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Some(Self {
            logged_in: v.get("loggedIn")?.as_bool()?,
            auth_method: v.get("authMethod")?.as_str()?.to_owned(),
            config_directory: v.get("configDirectory")?.as_str()?.to_owned(),
            email: text("email"),
            org_id: text("orgId"),
            api_key_source: text("apiKeySource"),
        })
    }
}

/// §12.3 step 8's table, from what `claude auth status --json` did. Rows are tried in order:
/// 1. A reply about another config dir says nothing about this profile (`drifted`).
/// 2. Then not logged in (`invalid`).
/// 3. Then another method (`overridden`).
/// 4. Then another account or org (`invalid`), which needs the email to be there to say so.
/// 5. Then `valid`, which also needs exit 0.
///
/// A login that cannot be confirmed is `unknown`, never `invalid`: only `invalid` deletes a
/// profile. No detail names an email (§4.4).
pub(crate) fn validity(reply: Captured, spelling: &str, expect: &Identity) -> Validity {
    let (code, signal, stdout) = match reply {
        Captured::Exited {
            code,
            signal,
            stdout,
            ..
        } => (code, signal, stdout),
        Captured::TimedOut => {
            return Validity::Unknown(format!(
                "`claude auth status` did not answer within {} s",
                AUTH_STATUS_TIMEOUT.as_secs()
            ));
        }
        Captured::Interrupted(_) => return Validity::Unknown(INTERRUPTED.into()),
        Captured::SpawnFailed(e) => return Validity::Unreachable(e),
    };
    let ended = match (code, signal) {
        (Some(c), _) => format!("exit {c}"),
        (None, Some(s)) => format!("signal {s}"),
        (None, None) => "no exit status".into(),
    };
    let Some(status) = AuthStatus::parse(&stdout) else {
        return Validity::Unknown(format!(
            "`claude auth status` printed no status tagteam can read ({ended})"
        ));
    };
    if status.config_directory != spelling {
        return Validity::Drifted {
            reported: status.config_directory,
        };
    }
    if !status.logged_in || status.auth_method == "none" {
        return Validity::Invalid("not logged in".into());
    }
    if status.auth_method != CLAUDE_AI {
        return Validity::Overridden {
            method: status.auth_method,
            source: status.api_key_source,
        };
    }
    let Some(email) = status.email else {
        return Validity::Unknown(
            "`claude auth status` named no email for the claude.ai login".into(),
        );
    };
    if email != expect.email.as_deref().unwrap_or(&expect.label) {
        return Validity::Invalid("logged in to claude.ai as another account".into());
    }
    if status
        .org_id
        .as_deref()
        .is_some_and(|org| !expect.org_uuid.is_empty() && org != expect.org_uuid)
    {
        return Validity::Invalid("logged in to claude.ai in another organization".into());
    }
    if code != Some(0) {
        return Validity::Unknown(format!(
            "`claude auth status` reported this login but ended with {ended}"
        ));
    }
    Validity::Valid
}
```

**`crates/tagteam-cc/src/provider.rs`.**
- Add `use std::ffi::OsString;` before `use std::path::{Path, PathBuf};`.
- Add `use tagteam_provider::process::{ProcessSpawner, SpawnSpec};` after
  `use tagteam_provider::provider::{DeadReason, RefreshResult};`.
- Add `SessionEnv` and `Validity` to the `use tagteam_provider::{…}` import.

At the end of `impl Provider for ClaudeCode`, after Task 6's `compose_profile_credential`, add:

```rust
    /// §12.5: the process environment's names are read here, so the token file descriptors
    /// actually set are the ones scrubbed.
    fn session_env(&self, spelling: &str) -> SessionEnv {
        session::session_env(spelling, std::env::vars_os().map(|(name, _)| name))
    }

    /// §12.3 step 8: `claude auth status --json` in the session's environment and `cwd`. It
    /// spawns `program`, the launch command `plan_run` resolved on `PATH` (§12.1), so the check
    /// runs the binary the session will (Decision 20). A token already set spawns nothing.
    fn validate_profile(&self, _env: &Env, spelling: &str, cwd: &Path, program: &Path, expect: &Identity, spawner: &dyn ProcessSpawner, cancel: &Cancel) -> Validity {
        if cancel.requested().is_some() {
            return Validity::Unknown(session::INTERRUPTED.into());
        }
        let SessionEnv { set, remove } = self.session_env(spelling);
        let spec = SpawnSpec {
            program: program.to_path_buf(),
            args: Vec::from(["auth", "status", "--json"].map(OsString::from)),
            set,
            remove,
            cwd: Some(cwd.to_path_buf()),
        };
        session::validity(
            spawner.run_captured(&spec, session::AUTH_STATUS_TIMEOUT, cancel),
            spelling,
            expect,
        )
    }
```

**`crates/tagteam-fake/src/provider.rs`.**
- Add `use tagteam_provider::process::ProcessSpawner;` after
  `use tagteam_provider::provider::{DeadReason, RefreshResult, TransientKind};`.
- Add `SessionEnv` and `Validity` to the `use tagteam_provider::{…}` import.
- Beside Task 5's constants, add:

```rust
/// FakeAgent's token variable, the one name its session scrubs.
const TOKEN_VAR: &str = "FAKEAGENT_TOKEN";
```

At the end of `impl Provider for FakeAgent`, after Task 6's `compose_profile_credential`, add:

```rust
    /// FakeAgent's session: its home variable names the profile, and its token variable is
    /// scrubbed. One of each, unlike Claude Code's list.
    fn session_env(&self, spelling: &str) -> SessionEnv {
        SessionEnv {
            set: vec![(HOME_VAR.into(), spelling.into())],
            remove: vec![TOKEN_VAR.into()],
        }
    }

    /// No spawn, from the profile's files, in the directory its spelling names as is
    /// (`profile_spelling`):
    /// - `valid` when it holds a credential with a token and the account's identity;
    /// - `invalid` without either, or with another identity;
    /// - `unknown` when a file cannot be read, or when the token is set.
    fn validate_profile(&self, env: &Env, spelling: &str, _cwd: &Path, _program: &Path, expect: &Identity, _spawner: &dyn ProcessSpawner, cancel: &Cancel) -> Validity {
        if cancel.requested().is_some() {
            return Validity::Unknown("interrupted".into());
        }
        let profile = profile_env_in(env, Path::new(spelling));
        match self.read_live_auth(&profile).credential {
            Read::Present(c) if shape::token(c.bytes()).is_some() => {}
            Read::Present(_) | Read::Absent => return Validity::Invalid("not logged in".into()),
            Read::Unreadable(e) => return Validity::Unknown(e.to_string()),
        }
        match self.live_identity(&profile) {
            Read::Present(id) if self.identity_key(&id) == self.identity_key(expect) => {
                Validity::Valid
            }
            Read::Present(_) => Validity::Invalid("logged in as another identity".into()),
            Read::Absent => Validity::Invalid("not logged in".into()),
            Read::Unreadable(e) => Validity::Unknown(e.to_string()),
        }
    }
```

- [ ] **Step 4: Run them and see them pass**, then the crates' whole suites

Run: `cargo test -p tagteam-cc --lib session`
Expected: PASS, 1 test.

Run: `cargo test -p tagteam-cc --test session validation`
Expected: PASS, 10 tests.

Run: `cargo test -p tagteam-fake --test provider validation`
Expected: PASS, 2 tests.

Run: `cargo test -p tagteam-provider -p tagteam-cc -p tagteam-fake`
Expected: PASS.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. Nothing calls the new methods yet: Tasks 9–11 do.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider/src/provider.rs crates/tagteam-provider/src/lib.rs \
  crates/tagteam-cc/src/session.rs crates/tagteam-cc/src/provider.rs crates/tagteam-cc/tests/session.rs \
  crates/tagteam-cc/tests/fixtures/auth-status/claude-ai.json \
  crates/tagteam-fake/src/provider.rs crates/tagteam-fake/tests/provider.rs
git commit -m "Build a session's environment and check its login with claude auth status"
```

---

### Task 8: The launch decision (`plan_run`)

§12.1 decides, before any lock is taken, whether `tagteam run` starts a session or runs plain
`claude`. It is a pure read: the store (if there is one), the mappings, the live login and
`PATH`. `plan_run` returns either a `Session` for an account, which `launch` (Task 10) decides
again under its locks, or `Plain`, an `exec` of the launch command with the environment `run`
was given. Inside a run shell that environment first gets the outer home back from the marker
(§12.8), so plain `claude` runs in the default home, as it would have outside.

This task is engine-only. The CLI (Task 12) resolves `ACCOUNT` (§10.4) before it calls
`plan_run`, `exec`s a `Plain` plan, and hands a `Session` plan to `launch`.

**Readings of the spec this task commits to:**
- **The provider.** A named account decides its own provider, and a `--provider` that names
  another one is refused (`invalid-input`). With no account, it is `--provider`, else the
  default provider (§12.1). A provider without `sessions` refuses (§4.5, "Without `sessions`:
  `run` refuses for that provider").
- **The launch command is looked up first.** It comes before the mapping, the API-key check,
  the live login and `--require-session`, so a missing command fails the same way everywhere
  and changes nothing (§12.1). The lookup uses the `PATH` the CLI captured into `Env.vars`
  (Decision 15), never the test process's own. This task adds `PATH` to M4a's capture list:
  one line in `app::session_vars`. `plan_run` writes nothing, and with no store it creates
  none (§5).
- **The mapping** is the nearest mapped ancestor of the canonical path of `cwd`, for the
  provider (§12.7, Task 1's `nearest_mapping`). A `cwd` that cannot be resolved, because it
  was deleted for example, is looked up as given.
- **A mapping whose account is gone** can only be Decision 7's race: the mapping was read,
  then the account (and, by the cascade, the mapping) was removed before its row was read.
  That runs plain `claude` with a warning naming the mapped path.
- **The API-key refusal** applies to a named account and to a mapped one alike, and comes before
  the live-login check. An account is an API-key account when its kind lives on the managed-key
  axis (`KindTraits.managed_key_axis`), so the engine never names a provider's kind strings
  (§4.5).
- **The live login** is the default home's live identity, compared by identity key: inside a
  run shell too, since the engine's `Env` is the effective one (§12.8, M4a Decision 6). An
  unreadable live identity refuses (`unreadable`). Running a session there could give one
  rotating token two copies, and running plain could give the wrong account.
- **`--require-session`** refuses exactly on the three plain rows: no mapping, a mapping whose
  account is gone, and a target that is the live login. `why` names which.
- **Plain `claude`'s environment** (§12.1, §12.5 "They are not scrubbed on the plain-`claude`
  paths"):
  - Outside a run shell, nothing is set or removed, and `spec.cwd` is `None`: `exec` keeps the
    directory it was given.
  - Inside a run shell, `set` and `remove` carry exactly the marker's `outer` record. Its keys
    are the marker provider's home variables, as both providers' `outer_home` document (M4a
    Task 7). A string is set, and `null` (undefined) is removed.
  - **M4a's open question on `""`.** A defined-but-empty value of the provider's session
    variable (`CLAUDE_CONFIG_DIR`) is removed, never exported: Appendix A.1 says tagteam treats
    one as unset and never exports one. Every other recorded value is restored as recorded,
    `""` included. A defined-but-empty `CLAUDE_SECURESTORAGE_CONFIG_DIR` means `~/.claude`,
    which is the outer home.
  - Nothing is scrubbed. An unreadable marker refuses with `run-shell-unreadable` (the CLI has
    already refused before building the engine, M4a Task 8).
- **`run` is never refused inside a run shell** (Decision 13), and a session of another account
  can start there.

**Files:**
- Create: `crates/tagteam-engine/src/run.rs`
- Modify: `crates/tagteam-engine/src/lib.rs` (`pub mod run;` after `mod rescue;`, line 19)
- Modify: `crates/tagteam-engine/src/error.rs`:
  - three variants after the last one before `Io` (today line 104; M3a and M4a add variants
    there first);
  - their `kind()` arms before `EngineError::Io(_) => "io"` (today line 142);
  - their cases in `kind_is_pinned_for_every_variant`, before the `Io` case (today line 272).
- Modify: `crates/tagteam-engine/tests/common/mod.rs` (a new `impl Fx` block at the end:
  `fake_bin`, `with_path`, `engine_on_path`, `work_dir`)
- Modify: `crates/tagteam/src/app.rs` (M4a Task 8's `session_vars`: `PATH` joins the capture
  list; its test `the_process_boundary_captures_every_session_variable_and_claudecode`)
- Test: `crates/tagteam-engine/tests/run_plan.rs` (new)

**Interfaces:**
- Consumes:
  - Task 1: `Store::nearest_mapping(&self, dir: &Path, provider: &ProviderId) -> Result<Option<Mapping>, StoreError>`,
    `Mapping { path, provider, account_id, added_at }`, and, in tests,
    `Store::set_mapping(&self, path: &str, provider: &ProviderId, account: &AccountId, at: i64)`.
  - Task 3: `tagteam_provider::process::{SpawnSpec, find_on_path}`.
  - M4a (not on this branch; written against its plan's contract): `Env::var`, `Env.vars`,
    `Engine::run_shell`, `RunShell::{Outside, Inside, Unreadable}`, `ProfileMarker.{provider, outer}`,
    `Provider::{launch_command, session_dir_var}`, `Capabilities.sessions`,
    `EngineError::RunShellUnreadable`, and the fixtures
    `Fx::{make_profile, write_marker, shell_env, engine_located, profile_dir}`. `Fx::engine_with_env`
    is today's, which M4a routes through `engine_in`. M4a Task 8's `app::session_vars` and its
    test `the_process_boundary_captures_every_session_variable_and_claudecode`.
  - M3a: `Env.cancel`, shared by every clone of an `Env`, and `Engine::cancel`.
  - Existing: `Engine::{existing_store, provider, read_live_identity}` (`read_live_identity` is
    `pub(crate)` in `switch.rs`), `Provider::{identity_key, kind_traits, display_name}`,
    `KindTraits.managed_key_axis`, and the fixtures `two_accounts`, `API_KEY`, `Fx::{add, add_api_key}`.
- Produces (Interface Contract):
  - `tagteam_engine::run::{RunRequest, RunPlan}` and `Engine::plan_run(&self, req: &RunRequest) -> Result<RunPlan, EngineError>`.
  - `EngineError::LaunchCommandMissing { command: String }` (`launch-command-missing`),
    `EngineError::ApiKeyAccount { position: u32 }` (`api-key-account`),
    `EngineError::RequiresSession { why: String }` (`requires-session`).
  - Crate-private, for Task 10:
    `Engine::is_live_login(&self, p: &dyn Provider, row: &AccountRow) -> Result<bool, EngineError>`
    and `crate::run::no_sessions(p: &dyn Provider) -> EngineError`.
  - Test fixtures: `Fx::fake_bin(&self, name: &str) -> PathBuf`, `Fx::with_path(env: &Env, dir: &Path) -> Env`,
    `Fx::engine_on_path(&self, dir: &Path) -> Engine`, `Fx::work_dir(&self, rel: &str) -> PathBuf`.
  - `app::session_vars` returns `PATH` after M4a's names, so `Context::from_process` captures
    it into `Env.vars`, where `plan_run` reads it (Decision 15).

**Spec:**
- §12.1: no `ACCOUNT` means the nearest mapping for the provider. A mapping to a removed
  account warns and runs plain; no mapping runs plain with an untouched environment. API-key
  accounts are refused. A target that is the live login runs plain. `--require-session` refuses
  wherever plain would run. Plain is `exec`, and inside a run shell the outer home's variables
  are restored first. The launch command is looked up on `PATH` before any lock; missing, exit 1
  and nothing changed. Inside a run shell, `run` launches as it would from the outer home.
- §12.7: mappings are canonical paths; subdirectories inherit the nearest mapped ancestor, per
  provider.
- §12.8: the live login is the default home's; `run` launches as from the outer home.
- §12.5 "Environment": nothing is scrubbed on the plain paths.
- Appendix A.1: tagteam treats an empty `CLAUDE_CONFIG_DIR` as unset and never exports one.
- §4.5: without `sessions`, `run` refuses for that provider. §5: a read creates nothing.
- Decisions 7, 9, 13 and 15.

- [ ] **Step 1: Write the failing tests**

Append to `crates/tagteam-engine/tests/common/mod.rs`:

```rust
/// `tagteam run` (§12): its launch command on `PATH`, and the directories it starts in.
impl Fx {
    /// A `bin/` directory under the fixture, holding an executable `/bin/sh` stub named `name`
    /// for `PATH` lookups (§12.1). Nothing ever runs it. Returns the directory.
    pub fn fake_bin(&self, name: &str) -> PathBuf {
        let bin = self.dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let path = bin.join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    /// `env` with `PATH` set to `dir` alone, in `Env.vars`, where the CLI captures it (Task 8).
    pub fn with_path(env: &Env, dir: &Path) -> Env {
        let mut env = env.clone();
        env.vars.insert("PATH".into(), dir.as_os_str().to_owned());
        env
    }

    /// An engine for this fixture's own home whose `PATH` is `dir`.
    pub fn engine_on_path(&self, dir: &Path) -> Engine {
        self.engine_with_env(Self::with_path(&self.env, dir))
    }

    /// `~/<rel>` under `~/work`, created: a directory `run` starts in (§12.1).
    pub fn work_dir(&self, rel: &str) -> PathBuf {
        let dir = self.env.home.join("work").join(rel);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
```

Create `crates/tagteam-engine/tests/run_plan.rs`:

```rust
//! §12.1: what `tagteam run` launches, decided before any lock: a session for an account, or
//! plain `claude` with the environment it was given, which inside a run shell is the outer
//! home's (§12.8).

mod common;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use common::{API_KEY, Fx, two_accounts};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::EngineError;
use tagteam_engine::run::{RunPlan, RunRequest};
use tagteam_provider::process::SpawnSpec;

/// `tagteam run [ACCOUNT] -- --resume` from `cwd`, for the default provider.
fn request(account: Option<&AccountId>, cwd: &Path) -> RunRequest {
    RunRequest {
        account: account.cloned(),
        provider: None,
        require_session: false,
        cwd: cwd.to_path_buf(),
        args: vec![OsString::from("--resume")],
    }
}

/// `req` with `--require-session`.
fn requiring(req: RunRequest) -> RunRequest {
    RunRequest {
        require_session: true,
        ..req
    }
}

/// `tagteam map <id> <dir>` (§12.7), stored under the directory's canonical path.
fn map(fx: &Fx, dir: &Path, id: &AccountId) {
    let path = fs::canonicalize(dir).unwrap();
    fx.engine
        .store()
        .unwrap()
        .set_mapping(path.to_str().unwrap(), &fx.provider(), id, 1)
        .unwrap();
}

/// What Decision 7's race leaves: a mapping of `dir` whose account is gone, as `plan_run` reads
/// it between the two reads. The store's cascade never leaves one, so it is written directly.
fn dangling_mapping(fx: &Fx, dir: &Path) {
    let path = fs::canonicalize(dir).unwrap();
    let db = rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db")).unwrap();
    // The bundled SQLite is built with SQLITE_DEFAULT_FOREIGN_KEYS=1, so every new connection
    // enforces `mappings.account_id`'s reference and would refuse the orphan. Off on this raw
    // connection only; the store's own connections keep enforcing it.
    db.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    db.execute(
        "INSERT INTO mappings (path, provider, account_id, added_at) VALUES (?1, ?2, 'gone', 1)",
        [path.to_str().unwrap(), fx.provider().as_str()],
    )
    .unwrap();
}

fn plain(plan: RunPlan) -> (SpawnSpec, Option<String>) {
    match plan {
        RunPlan::Plain { spec, warning } => (spec, warning),
        RunPlan::Session { account, .. } => {
            panic!("a session for position {}, not plain claude", account.position)
        }
    }
}

fn session(plan: RunPlan) -> (AccountId, ProviderId, PathBuf) {
    match plan {
        RunPlan::Session {
            account,
            provider,
            launch,
        } => (account.id, provider, launch),
        RunPlan::Plain { spec, .. } => panic!("plain claude, not a session: {spec:?}"),
    }
}

#[test]
fn with_no_account_and_no_mapping_plain_claude_runs_with_an_untouched_environment() {
    let fx = Fx::new();
    two_accounts(&fx);
    let bin = fx.fake_bin("claude");
    let cwd = fx.work_dir("app");

    let (spec, warning) = plain(fx.engine_on_path(&bin).plan_run(&request(None, &cwd)).unwrap());

    assert_eq!(spec.program, bin.join("claude"));
    assert_eq!(spec.args, [OsString::from("--resume")]);
    assert!(spec.set.is_empty() && spec.remove.is_empty(), "{spec:?}");
    assert_eq!(spec.cwd, None, "exec keeps the directory it was given");
    assert_eq!(warning, None);
}

#[test]
fn with_no_store_plain_claude_runs_and_nothing_is_created() {
    // §5: a decision that only reads creates nothing.
    let fx = Fx::new();
    let bin = fx.fake_bin("claude");
    let cwd = fx.work_dir("app");

    plain(fx.engine_on_path(&bin).plan_run(&request(None, &cwd)).unwrap());

    assert!(!fx.env.data_dir().exists());
}

#[test]
fn the_nearest_mapped_ancestor_decides() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let bin = fx.fake_bin("claude");
    let deeper = fx.work_dir("app/src/lib");
    let sibling = fx.work_dir("other");
    map(&fx, &fx.env.home.join("work"), &b);
    map(&fx, &fx.env.home.join("work/app"), &a);
    let engine = fx.engine_on_path(&bin);

    let (id, provider, launch) = session(engine.plan_run(&request(None, &deeper)).unwrap());
    assert_eq!((id, provider), (a, fx.provider()), "`~/work/app` is nearer than `~/work`");
    assert_eq!(launch, bin.join("claude"));

    // `~/work/other` inherits `~/work`'s mapping: b, the live login, so plain claude.
    let (_, warning) = plain(engine.plan_run(&request(None, &sibling)).unwrap());
    assert_eq!(warning, None);
}

#[test]
fn a_mapping_whose_account_went_away_runs_plain_claude_with_a_warning() {
    // §12.1 and Decision 7.
    let fx = Fx::new();
    two_accounts(&fx);
    let cwd = fx.work_dir("app");
    dangling_mapping(&fx, &cwd);

    let (spec, warning) = plain(
        fx.engine_on_path(&fx.fake_bin("claude"))
            .plan_run(&request(None, &cwd))
            .unwrap(),
    );

    let warning = warning.expect("a warning names the mapping");
    assert!(warning.contains("was removed"), "{warning}");
    assert!(spec.set.is_empty() && spec.remove.is_empty(), "{spec:?}");
}

#[test]
fn require_session_refuses_wherever_plain_claude_would_run() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let engine = fx.engine_on_path(&fx.fake_bin("claude"));
    let unmapped = fx.work_dir("unmapped");
    let orphaned = fx.work_dir("orphaned");
    dangling_mapping(&fx, &orphaned);

    for (req, why) in [
        (request(None, &unmapped), "no mapping applies"),
        (request(None, &orphaned), "was removed"),
        (request(Some(&b), &unmapped), "is the live login"),
    ] {
        let err = engine.plan_run(&requiring(req)).unwrap_err();
        assert_eq!(err.kind(), "requires-session", "{err}");
        assert!(err.to_string().contains(why), "{err}");
    }
    // Where a session would start, the flag changes nothing.
    session(engine.plan_run(&requiring(request(Some(&a), &unmapped))).unwrap());
}

#[test]
fn an_api_key_account_is_refused_named_or_mapped() {
    let fx = Fx::new();
    two_accounts(&fx);
    let k = fx.add_api_key(API_KEY);
    let position = fx.engine.store().unwrap().account(&k).unwrap().unwrap().position;
    let cwd = fx.work_dir("app");
    map(&fx, &cwd, &k);
    let engine = fx.engine_on_path(&fx.fake_bin("claude"));

    for req in [request(Some(&k), &cwd), request(None, &cwd)] {
        let err = engine.plan_run(&req).unwrap_err();
        assert!(
            matches!(err, EngineError::ApiKeyAccount { position: p } if p == position),
            "{err}"
        );
        assert_eq!(err.kind(), "api-key-account");
    }
}

#[test]
fn the_live_login_runs_plain_claude_and_any_other_account_gets_a_session() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let bin = fx.fake_bin("claude");
    let engine = fx.engine_on_path(&bin);
    let cwd = fx.work_dir("app");

    let (_, warning) = plain(engine.plan_run(&request(Some(&b), &cwd)).unwrap());
    assert_eq!(warning, None, "§12.1: never two copies of one rotating token");

    let planned = session(engine.plan_run(&request(Some(&a), &cwd)).unwrap());
    assert_eq!(planned, (a, fx.provider(), bin.join("claude")));
}

#[test]
fn a_missing_launch_command_fails_before_anything_else_and_changes_nothing() {
    let fx = Fx::new();
    let empty = fx.dir.path().join("empty-bin");
    fs::create_dir_all(&empty).unwrap();
    // A `claude` that is not executable is not a launch command either.
    fs::write(empty.join("claude"), "#!/bin/sh\n").unwrap();
    let cwd = fx.work_dir("app");

    let err = fx
        .engine_on_path(&empty)
        .plan_run(&request(None, &cwd))
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::LaunchCommandMissing { command } if command == "claude"),
        "{err}"
    );
    assert_eq!(err.kind(), "launch-command-missing");
    assert!(!fx.env.data_dir().exists(), "nothing was created");

    // It comes before the live-login check and `--require-session`'s refusal.
    let a = fx.add("a@x.co", "rt-a");
    let err = fx
        .engine_on_path(&empty)
        .plan_run(&requiring(request(Some(&a), &cwd)))
        .unwrap_err();
    assert_eq!(err.kind(), "launch-command-missing", "{err}");
}

#[test]
fn an_account_that_does_not_exist_is_refused() {
    let fx = Fx::new();
    two_accounts(&fx);
    let nope = AccountId::from_string("nope");

    let err = fx
        .engine_on_path(&fx.fake_bin("claude"))
        .plan_run(&request(Some(&nope), &fx.work_dir("app")))
        .unwrap_err();

    assert_eq!(err.kind(), "no-such-account", "{err}");
}

#[test]
fn a_named_account_of_another_provider_than_the_one_asked_for_is_refused() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let req = RunRequest {
        provider: Some(ProviderId::new("fake-agent")),
        ..request(Some(&a), &fx.work_dir("app"))
    };

    let err = fx
        .engine_on_path(&fx.fake_bin("claude"))
        .plan_run(&req)
        .unwrap_err();

    assert_eq!(err.kind(), "invalid-input", "{err}");
}

#[test]
fn an_unreadable_live_login_refuses_rather_than_guess() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let engine = fx.engine_on_path(&fx.fake_bin("claude"));
    let cwd = fx.work_dir("app");
    fs::write(fx.paths().global_config, "{ torn").unwrap();

    let err = engine.plan_run(&request(Some(&a), &cwd)).unwrap_err();

    assert_eq!(err.kind(), "unreadable", "{err}");
}

#[test]
fn inside_a_run_shell_plain_claude_gets_the_outer_home_back_and_nothing_is_scrubbed() {
    // §12.1, §12.8: as it would have run outside the session.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let bin = fx.fake_bin("claude");
    let profile = fx.make_profile(&a);
    let process = Fx::with_path(&fx.shell_env(&profile), &bin);
    let engine = fx.engine_located(process.clone());

    let (spec, _) = plain(engine.plan_run(&request(Some(&b), &fx.work_dir("app"))).unwrap());

    assert!(spec.set.is_empty(), "{spec:?}");
    assert_eq!(
        spec.remove,
        [
            OsString::from("CLAUDE_CONFIG_DIR"),
            OsString::from("CLAUDE_SECURESTORAGE_CONFIG_DIR")
        ],
        "both were undefined in the outer home"
    );
    assert!(!spec.remove.contains(&OsString::from("ANTHROPIC_API_KEY")));
    // M4a's `apply_outer_home` clones the `Env` it is given, and `Env::clone` shares the
    // `Cancel` (M3a): the outer `Env` keeps the process's token, which `run` forwards signals
    // through (Decision 1).
    process.cancel.request(libc::SIGTERM);
    assert_eq!(
        engine.cancel().requested(),
        Some(libc::SIGTERM),
        "the outer Env's cancel token is the process's"
    );
}

#[test]
fn a_custom_outer_home_is_restored_as_the_marker_records_it() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let bin = fx.fake_bin("claude");
    let custom = fx.dir.path().join("custom-home");
    let mut outer = fx.env.clone();
    outer.claude_config_dir = Some(custom.clone().into_os_string());
    let profile = fx.profile_dir(&a);
    fx.write_marker(&profile, &a, &outer);
    let engine = fx.engine_located(Fx::with_path(&fx.shell_env(&profile), &bin));

    let (spec, _) = plain(engine.plan_run(&request(None, &fx.work_dir("app"))).unwrap());

    assert_eq!(
        spec.set,
        [(OsString::from("CLAUDE_CONFIG_DIR"), custom.into_os_string())]
    );
    assert_eq!(spec.remove, [OsString::from("CLAUDE_SECURESTORAGE_CONFIG_DIR")]);
}

#[test]
fn an_empty_outer_config_dir_is_unset_never_exported() {
    // Appendix A.1: tagteam treats an empty CLAUDE_CONFIG_DIR as unset and never exports one.
    // A defined-but-empty CLAUDE_SECURESTORAGE_CONFIG_DIR means `~/.claude`, so it stays.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let bin = fx.fake_bin("claude");
    let mut outer = fx.env.clone();
    outer.claude_config_dir = Some(OsString::new());
    outer.claude_securestorage_config_dir = Some(OsString::new());
    let profile = fx.profile_dir(&a);
    fx.write_marker(&profile, &a, &outer);
    let engine = fx.engine_located(Fx::with_path(&fx.shell_env(&profile), &bin));

    let (spec, _) = plain(engine.plan_run(&request(None, &fx.work_dir("app"))).unwrap());

    assert_eq!(
        spec.set,
        [(
            OsString::from("CLAUDE_SECURESTORAGE_CONFIG_DIR"),
            OsString::new()
        )]
    );
    assert_eq!(spec.remove, [OsString::from("CLAUDE_CONFIG_DIR")]);
}

#[test]
fn inside_a_run_shell_another_account_gets_a_session() {
    // §12.1: a session can start a session of another account.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let c = fx.add("c@x.co", "rt-c");
    fx.add("b@x.co", "rt-b");
    let bin = fx.fake_bin("claude");
    let profile = fx.make_profile(&a);
    let engine = fx.engine_located(Fx::with_path(&fx.shell_env(&profile), &bin));

    let (id, _, launch) = session(engine.plan_run(&request(Some(&c), &fx.work_dir("app"))).unwrap());

    assert_eq!((id, launch), (c, bin.join("claude")));
}
```

In `crates/tagteam/src/app.rs`'s `tests` module, M4a Task 8's
`the_process_boundary_captures_every_session_variable_and_claudecode` changes its expected list
(Decision 15):

```rust
        assert_eq!(
            session_vars(&build_registry(&ctx)),
            ["CLAUDE_CONFIG_DIR", "CLAUDECODE", "PATH"]
        );
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test run_plan`
Expected: compile errors: `unresolved import tagteam_engine::run`, and
`no variant named LaunchCommandMissing` / `ApiKeyAccount` in `EngineError`. The fixture helpers
compile, since they use only `Env.vars` (M4a) and existing `Fx` methods.

Run: `cargo test -p tagteam --lib the_process_boundary_captures_every_session_variable_and_claudecode`
Expected: FAIL: the left side is `["CLAUDE_CONFIG_DIR", "CLAUDECODE"]`, without `PATH`.

- [ ] **Step 3: Implement**

**`crates/tagteam/src/app.rs`.** In M4a Task 8's `session_vars`, after
`names.push(CLAUDECODE);`, add the one line of Decision 15, and extend its doc comment's list
with "then `PATH`, which `plan_run` looks the launch command up on (§12.1)":

```rust
    names.push("PATH");
```

**`crates/tagteam-engine/src/error.rs`.** Add after the last variant before `Io` (today
`ForeignLiveCredential`, lines 99–104, with M3a's and M4a's variants after it):

```rust
    /// §12.1: the provider's launch command is not on `PATH`; `run` changed nothing.
    #[error("`{command}` is not on PATH; install it, or add its directory to PATH")]
    LaunchCommandMissing { command: String },
    /// §12.1: an API-key account has no login a session could run in a profile of its own.
    #[error(
        "position {position} is an API-key account, which `tagteam run` cannot start a session for; `tagteam switch {position}` makes it the live login"
    )]
    ApiKeyAccount { position: u32 },
    /// §12.1 `--require-session`: `run` would have run plain `claude` instead of a session.
    #[error("--require-session: {why}, so no session would start")]
    RequiresSession { why: String },
```

In `kind()`, before `EngineError::Io(_) => "io",`:

```rust
            EngineError::LaunchCommandMissing { .. } => "launch-command-missing",
            EngineError::ApiKeyAccount { .. } => "api-key-account",
            EngineError::RequiresSession { .. } => "requires-session",
```

In `kind_is_pinned_for_every_variant`, before `(EngineError::Io(io::Error::other("x")), "io"),`:

```rust
            (
                EngineError::LaunchCommandMissing {
                    command: "claude".into(),
                },
                "launch-command-missing",
            ),
            (EngineError::ApiKeyAccount { position: 1 }, "api-key-account"),
            (
                EngineError::RequiresSession { why: "w".into() },
                "requires-session",
            ),
```

**`crates/tagteam-engine/src/lib.rs`**: add `pub mod run;` after `mod rescue;`.

**Create `crates/tagteam-engine/src/run.rs`:**

```rust
//! §12.1: what `tagteam run` launches, decided before any lock is taken. A session for an
//! account goes on to `launch` (§12.5), which decides again under its locks. Plain `claude` is
//! an `exec` with the environment `run` was given: the outer home's, inside a run shell (§12.8).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::Provider;
use tagteam_provider::process::{SpawnSpec, find_on_path};
use tagteam_provider::profile::RunShell;

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::AccountRow;

#[derive(Debug, Clone)]
/// `account` is resolved by the CLI (§10.4, with its ambiguity prompt) before planning;
/// `None` means the mapping decides (§12.1). `provider` is the global `--provider`.
pub struct RunRequest {
    pub account: Option<AccountId>,
    pub provider: Option<ProviderId>,
    pub require_session: bool,
    pub cwd: PathBuf,
    pub args: Vec<OsString>,
}

#[derive(Debug, Clone)]
pub enum RunPlan {
    /// §12.1's plain `claude`: exec `spec` (outer home restored, nothing scrubbed); `warning`
    /// for a mapping whose account went away (Decision 7).
    Plain {
        spec: SpawnSpec,
        warning: Option<String>,
    },
    /// A session for `account`.
    Session {
        account: AccountRow,
        provider: ProviderId,
        launch: PathBuf,
    },
}

/// What a directory's mapping names (§12.7).
enum Mapped {
    Nothing,
    /// The mapping of `path` names an account that was removed after it was read (Decision 7).
    Gone { path: String },
    Account(AccountRow),
}

/// `run` for a provider without sessions (§4.5 "Capabilities are explicit").
pub(crate) fn no_sessions(p: &dyn Provider) -> EngineError {
    EngineError::InvalidInput(format!(
        "{} has no `tagteam run` sessions",
        p.display_name()
    ))
}

impl Engine {
    /// §12.1. It only reads: no lock, no write, and with no store none is created (§5).
    ///
    /// The launch command is looked up first, on the `PATH` the CLI captured into `Env.vars`,
    /// so a missing one changes nothing. Then the target: the named account, or the nearest
    /// mapping of `cwd`'s canonical path for the provider. An API-key account is refused. Plain
    /// `claude` runs when nothing is mapped, when the mapped account went away (with a
    /// warning), or when the target is the default home's live login; under
    /// `--require-session` each of those refuses instead.
    pub fn plan_run(&self, req: &RunRequest) -> Result<RunPlan, EngineError> {
        let named = match &req.account {
            Some(id) => Some(self.named_account(id)?),
            None => None,
        };
        let provider = match (&named, &req.provider) {
            (Some(row), Some(asked)) if &row.provider != asked => {
                return Err(EngineError::InvalidInput(format!(
                    "position {} is a {} account, not a {asked} one",
                    row.position, row.provider
                )));
            }
            (Some(row), _) => row.provider.clone(),
            (None, Some(asked)) => asked.clone(),
            (None, None) => self.default_provider.clone(),
        };
        let p = self.provider(&provider)?;
        let p = p.as_ref();
        if !p.capabilities().sessions {
            return Err(no_sessions(p));
        }
        let command = p.launch_command();
        let launch = find_on_path(command, self.env.var("PATH")).ok_or_else(|| {
            EngineError::LaunchCommandMissing {
                command: command.to_owned(),
            }
        })?;
        let target = match named {
            Some(row) => row,
            None => match self.mapped_account(&provider, &req.cwd)? {
                Mapped::Account(row) => row,
                Mapped::Nothing => {
                    let why = format!("no mapping applies to {}", req.cwd.display());
                    return self.plain(req, launch, why, None);
                }
                Mapped::Gone { path } => {
                    let why = format!("the account mapped to {path} was removed");
                    let warning = format!("{why}; running plain {command}");
                    return self.plain(req, launch, why, Some(warning));
                }
            },
        };
        if p.kind_traits(&target.kind).managed_key_axis {
            return Err(EngineError::ApiKeyAccount {
                position: target.position,
            });
        }
        if self.is_live_login(p, &target)? {
            let why = format!("position {} is the live login", target.position);
            return self.plain(req, launch, why, None);
        }
        Ok(RunPlan::Session {
            account: target,
            provider,
            launch,
        })
    }

    /// §12.1: whether `row` is the default home's live login, so that a session would give its
    /// rotating token a second copy. The live login is the default home's inside a run shell
    /// too (§12.8). An unreadable live identity is an error, never taken for another login.
    pub(crate) fn is_live_login(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
    ) -> Result<bool, EngineError> {
        Ok(self
            .read_live_identity(p)?
            .is_some_and(|i| p.identity_key(&i).as_str() == row.identity_key))
    }

    /// The account the CLI resolved. With no store there is none, and none is created (§5).
    fn named_account(&self, id: &AccountId) -> Result<AccountRow, EngineError> {
        let row = match self.existing_store()? {
            Some(store) => store.account(id)?,
            None => None,
        };
        row.ok_or_else(|| EngineError::NoSuchAccount(id.to_string()))
    }

    /// §12.7: the mapping of `cwd`'s canonical path, or of its nearest mapped ancestor, for
    /// `provider`. A directory that cannot be resolved is looked up as given.
    fn mapped_account(&self, provider: &ProviderId, cwd: &Path) -> Result<Mapped, EngineError> {
        let Some(store) = self.existing_store()? else {
            return Ok(Mapped::Nothing);
        };
        let dir = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        let Some(mapping) = store.nearest_mapping(&dir, provider)? else {
            return Ok(Mapped::Nothing);
        };
        // Decision 7: a mapping goes with its account (ON DELETE CASCADE), so a row missing
        // now was removed between the two reads.
        Ok(match store.account(&mapping.account_id)? {
            Some(row) => Mapped::Account(row),
            None => Mapped::Gone { path: mapping.path },
        })
    }

    /// §12.1's plain `claude`, or `--require-session`'s refusal of it, naming `why`.
    fn plain(
        &self,
        req: &RunRequest,
        launch: PathBuf,
        why: String,
        warning: Option<String>,
    ) -> Result<RunPlan, EngineError> {
        if req.require_session {
            return Err(EngineError::RequiresSession { why });
        }
        let (set, remove) = self.outer_home_vars()?;
        Ok(RunPlan::Plain {
            spec: SpawnSpec {
                program: launch,
                args: req.args.clone(),
                set,
                remove,
                cwd: None,
            },
            warning,
        })
    }

    /// The variables that give plain `claude` the outer home back inside a run shell (§12.1,
    /// §12.8): the marker's `outer` record, whose keys are the marker provider's home variables
    /// (§4.5 `outer_home`). A string is set, and `null` (undefined) is removed. A
    /// defined-but-empty session variable is removed too, since tagteam treats it as unset and
    /// never exports one (Appendix A.1); every other value is restored as recorded, `""`
    /// included. Nothing is scrubbed (§12.5). Outside a run shell, nothing changes.
    fn outer_home_vars(&self) -> Result<(Vec<(OsString, OsString)>, Vec<OsString>), EngineError> {
        let marker = match self.run_shell() {
            RunShell::Outside => return Ok((Vec::new(), Vec::new())),
            RunShell::Inside { marker, .. } => marker,
            RunShell::Unreadable { marker, detail } => {
                return Err(EngineError::RunShellUnreadable {
                    marker: marker.clone(),
                    detail: detail.clone(),
                });
            }
        };
        let session_var = self.provider(&marker.provider)?.session_dir_var();
        let (mut set, mut remove) = (Vec::new(), Vec::new());
        if let Value::Object(vars) = &marker.outer {
            for (name, value) in vars {
                match value {
                    Value::String(s) if !(s.is_empty() && session_var == Some(name.as_str())) => {
                        set.push((OsString::from(name), OsString::from(s)));
                    }
                    _ => remove.push(OsString::from(name)),
                }
            }
        }
        Ok((set, remove))
    }
}
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-engine --test run_plan`
Expected: PASS, 15 tests.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS, including `kind_is_pinned_for_every_variant` with its three new cases.

Run: `cargo test -p tagteam --lib`
Expected: PASS, `the_process_boundary_captures_every_session_variable_and_claudecode` with
`PATH`.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. Nothing outside these tests calls `plan_run` yet.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/run.rs crates/tagteam-engine/src/lib.rs \
  crates/tagteam-engine/src/error.rs crates/tagteam-engine/tests/common/mod.rs \
  crates/tagteam-engine/tests/run_plan.rs crates/tagteam/src/app.rs
git commit -m "Decide what tagteam run launches before it takes any lock"
```

---

### Task 9: Bootstrap and validation (§12.3)

§12.3 steps 2–8 put the vault's current generation into a quiescent profile, make sure Claude
Code will read exactly that, seed the profile's `.claude.json`, and ask `claude auth status`
whether the session would log in as its account. The launch (Task 10) calls it under
`MutationGuard` and the account lock, which stay held through validation. Step 1's gate refresh
runs before the launch takes them, so it is Task 10's.

Nothing in production calls the bootstrap until Task 10. Its tests drive it through a
`test-hooks` seam, `Engine::bootstrap_quiescent`. The seam runs what a quiescent launch runs
between its locks and its reservation: the marker, the link sync, then a bootstrap for a §12.3
trigger, else the seed. It leaves out provenance, which Task 10 puts first.

**Readings of the spec this task commits to:**
- **The triggers** (§12.3's opening list) are checked in this order, and the first that applies
  is reported:
  1. `Missing`: no seed, so the profile was never bootstrapped.
  2. `Invalid`: the seed's `needsBootstrap`, set by Task 11's per-launch check.
  3. `StaleMarked`: the seed's epoch is not the account's `login_epoch` (§12.5).
  4. `SpellingChanged`: the marker's `configDir` is not the profile's canonical spelling now
     (§12.2).
  5. `OtherCredential`: the profile holds no credential, one whose generation is not the
     vault's, or one whose identity drifted (§12.5 "Identity drift").
- **Pending rescues first** (§6.2): a bootstrap activates the vault's generation, so it settles
  the account's `rescue/` entries under the account lock before it reads the vault.
- **Step 2** reads the credential as the profile holds it (Decision 22), through M4a's
  `read_profile_credential(env, profile, recorded)`: the hashed item named from the *recorded*
  spelling first (§12.2), then `.credentials.json` in the profile's actual directory. After a
  move the recorded spelling names a directory that is gone, so a file looked for there would
  never be found. The identity checks below read `profile_identity(env, profile)`, in the
  profile's directory, for the same reason.
  An unreadable read aborts, and so does a degraded one (the file covered an item that could not
  be read). An empty read aborts too, since a Keychain timeout can produce one (as `switch.rs`'s
  `empty_live_read` rules). A new profile has none.
- **Step 3 displaces**, when the profile's generation is not the vault's:
  - a stale-marked profile's credential (§12.3 step 3);
  - a credential whose identity drifted to another login. This is B.5's rule: bytes that are
    not ours are displaced before being overwritten.

  The reasons are `replaced-profile-login` and `foreign-profile-login`. A failed displacement
  aborts (B.5). A generation older than the vault's, from a profile that is not stale-marked,
  is this account's and possibly consumed, so it is overwritten without a copy.
- **Step 4** composes with the provider's `compose_profile_credential(vault, profile)` and writes
  with `write_profile_credential` (Task 6), under the profile's own credential locks and
  storage-write lock. It writes under the *current* spelling: the canonical path of the actual
  directory, so the file and its locks are the profile's own, and what Claude Code will use.
  After a move, the old spelling's item can be the only copy of the machine-shared keys (CC
  migrated the file into it). Under the current spelling the entry is then absent at the read
  and under the lock, so the write keeps the composed keys rather than rebasing them away
  (Task 6, Decision 22). Step 5 deletes the old item only after that.
- **Step 5** deletes the items for the current spelling, then for the recorded one when it
  differs. M4a's `delete_profile_credential` verifies each `Absent` with the existence probe,
  and its `ShadowingItem` aborts. A hook point, `bootstrap-after-credential-write`, sits between
  steps 4 and 5.
- **Step 6** re-reads as Claude Code would, under the current spelling. It checks the
  account-scoped keys as two fingerprints: the generation (`fingerprint`, the refresh token's
  lineage) and the access token (`access_fingerprint`), both the vault's. It never compares
  bytes, because the storage-write lock's re-read may rebase the machine-shared keys (M3a's
  rule). A mismatch aborts.

  Then it writes the seed (the account's `login_epoch`, the vault's fingerprint,
  `needsBootstrap` false). When the spelling changed, it writes the marker's new `configDir`,
  in that order, as §12.3 step 6 says.
- **Step 8** validates with the engine's `spawner` (the contract's `EngineConfig.spawner`, which
  this task adds as its first user), in `cwd`, spawning `program`. `cwd` is the directory
  `claude` will run in and `program` the launch command `plan_run` resolved, so
  `bootstrap_profile` takes both (the Interface Contract; Decision 20). The `test-hooks` seam
  has no plan, so it passes the provider's launch command by name, which the scripted spawner
  records and never runs.
  - A signal recorded by the end of the check interrupts the launch. The check's wait is a
    cancellation point (§12.5 "Signals").
  - `invalid` deletes the profile through M4a's `remove_profile` and refuses (B.29).
  - Every other non-`valid` outcome refuses and keeps the profile (§12.3's table).
- **Step 7 seeds the profile's actual directory** (`seed_profile(env, profile, …)`, Decision 22),
  whatever the marker records.
- **A non-UTF-8 canonical path** is refused (M4a's note for M4b): its lossy spelling would name
  another directory and another Keychain item.
- **Default-feature lint.** Until Task 10's `launch` calls it, only the `test-hooks` seam reaches
  the bootstrap, so a build without that feature finds it dead, and
  `cargo clippy --workspace --all-targets -- -D warnings` would fail on `dead_code`. The items
  that only the seam reaches carry `#[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]`:
  `current_spelling`, `refusal`, the `impl Engine` block of `bootstrap.rs` that is not
  `test-hooks`-only, and the field `Engine.spawner`. Task 10 deletes each one.
- **The marker at a first launch.** `mark_profile` creates the profile 0700 and its marker, with
  `configDir` set to the current spelling. At a later quiescent launch it updates `outer` only
  (§12.2 "Profile marker"). A marker that names another account or provider refuses: its
  spelling names someone else's Keychain item.

**Files:**
- Create: `crates/tagteam-engine/src/bootstrap.rs`
- Modify: `crates/tagteam-engine/src/lib.rs` (`pub mod bootstrap;` after `pub mod active;`,
  line 4)
- Modify: `crates/tagteam-engine/src/engine.rs`:
  - `EngineConfig.spawner` (struct at lines 16–27);
  - `Engine.spawner` (lines 29–44), with the default-feature `dead_code` allowance until
    Task 10;
  - `Engine::new` (lines 47–63);
  - the test module's `test_config` (lines 293–304).
- Modify: `crates/tagteam-engine/src/error.rs` (five variants before `Io`; their `kind()` arms;
  their pins; a private `from_source` helper)
- Modify: `crates/tagteam-engine/src/lifecycle.rs` (M4a Task 9's `fn remove_profile` becomes
  `pub(crate) fn remove_profile`)
- Modify: `crates/tagteam-engine/src/testutil.rs` (`T::new`'s `EngineConfig`, lines 48–57)
- Modify: `crates/tagteam-engine/src/lazy_http.rs`
  (`an_engine_over_a_lazy_client_does_not_build_it`'s `EngineConfig`, lines 124–138)
- Modify: `crates/tagteam/src/app.rs` (`build_engine`'s `EngineConfig`, lines 167–179)
- Modify: `crates/tagteam/src/statusline.rs` (`engine`'s `EngineConfig`, lines 206–215)
- Modify: `crates/tagteam-engine/tests/common/mod.rs`:
  - `Fx.spawner`;
  - the `spawner` field in `Fx::build` (303–312), `Fx::engine_in` (M4a), `Fx::engine_over_http`
    (1205–1214) and `FakeFx::new` (1028–1039);
  - the validation and profile-credential helpers.
- Modify: `crates/tagteam-engine/tests/oracle.rs`
  (`a_hanging_profile_endpoint_delays_a_switch_by_its_timeout_and_holds_no_lock`'s
  `EngineConfig`, lines 248–257)
- Test: `crates/tagteam-engine/tests/bootstrap.rs` (new)

Line numbers are today's. M3a and M4a edit `engine.rs`, the `EngineConfig` sites and
`tests/common/mod.rs` first; each edit below is anchored on a field or function name that
survives them.

**Interfaces:**
- Consumes:
  - Task 3: `tagteam_provider::process::{ProcessSpawner, SystemSpawner, ScriptedSpawner, Captured}`.
  - Tasks 5–7 (`Provider`): `seed_profile(env, dir, identity)` (Decision 22),
    `compose_profile_credential`, `write_profile_credential`, `validate_profile` (with its
    `program`), and `Validity`.
  - Task 8: the fixture `Fx::work_dir`.
  - M4a (not on this branch; written against its plan's contract and code):
    - `ProfileMarker`, `Seed`, `canonical_profile_path` and `profile_path`;
    - `Provider::launch_command`, the `test-hooks` seam's program;
    - the `Provider` methods `profile_spelling`, `outer_home`,
      `read_profile_credential(&self, env: &Env, dir: &Path, spelling: &str) -> Read<Credential>`,
      `profile_identity(&self, env: &Env, dir: &Path) -> Read<Identity>` (M4a's Decision 19)
      and `delete_profile_credential`;
    - `Engine::sync_profile_links`, `crate::provenance::identity_drifted` and
      `Engine::remove_profile` (made `pub(crate)` here);
    - the fixtures `Fx::{profile_dir, profile_item, item_for_spelling, set_profile_credential, set_profile_identity}`
      and `Fx.process`.
  - M3a (written against its plan's contract): `Engine::cancel`, `Cancel::requested` and
    `EngineError::Interrupted`; `MutationGuard::acquire` waits on `env.cancel`.
  - Existing: `Engine::{settle_rescues, lock_account, store, provider}`, `displace`, `hooks::point`,
    `Provider::{fingerprint, access_fingerprint, parse_identity}`, `Credential::{provenance, is_empty, bytes}`,
    `tagteam_cc::live::Platform`, and the fixtures
    `Fx::{with_platform, put_vault, plant_rescue, displaced, vault_refresh_token}`, `Fx.dir`,
    `credential`, `vault_fp` and `two_accounts`.
- Produces:
  - `EngineConfig.spawner: Arc<dyn ProcessSpawner>` and `Engine.spawner` (crate-private).
    Production builds `SystemSpawner`, and tests build `ScriptedSpawner`.
  - `pub enum tagteam_engine::bootstrap::Trigger { Missing, Invalid, StaleMarked, SpellingChanged, OtherCredential }`
    with `as_str`.
  - Crate-private, for Tasks 10 and 11:
    - `Engine::bootstrap_profile(&self, p: &dyn Provider, row: &AccountRow, profile: &Path, cwd: &Path, program: &Path, guard: &MutationGuard, lock: &AccountLock) -> Result<(), EngineError>`
      (the contract's signature);
    - `Engine::bootstrap_trigger(&self, p, row, profile) -> Result<Option<Trigger>, EngineError>`;
    - `Engine::mark_profile(&self, p, row, profile) -> Result<ProfileMarker, EngineError>`;
    - `Engine::own_marker(&self, row, profile) -> Result<Option<ProfileMarker>, EngineError>`;
    - `Engine::seed_of(&self, p, row, profile) -> Result<(), EngineError>`, which seeds
      `profile`, the actual directory (Decision 22);
    - `Engine::vault_generation(&self, row) -> Result<Vec<u8>, EngineError>`;
    - `bootstrap::refusal(row: &AccountRow, validity: Validity) -> Option<EngineError>`.
  - `test-hooks` only: `Engine::bootstrap_quiescent(&self, id: &AccountId, cwd: &Path) -> Result<Option<Trigger>, EngineError>`.
  - `EngineError::LoginOverridden { position: u32, method: String, key_source: Option<String> }`
    (`login-overridden`; the field is `key_source` because thiserror reserves `source`),
    `LoginInvalid { position, detail }` (`login-invalid`), `LoginDrifted { position, reported }`
    (`login-drifted`), `LoginUnknown { position, detail }` (`login-unknown`), and
    `LaunchUnreachable { detail }` (`launch-unreachable`).
  - The hook point `bootstrap-after-credential-write`.
  - Test fixtures in `crates/tagteam-engine/tests/common/mod.rs`:
    - the field `pub spawner: Arc<ScriptedSpawner>` on `Fx`, threaded through every
      `EngineConfig` site;
    - free functions `auth_status(spelling: &str, email: &str) -> Value`,
      `auth_logged_out(spelling: &str) -> Value`, `auth_helper(spelling: &str) -> Value` and
      `auth_reply(code: i32, stdout: &[u8]) -> Captured`;
    - `impl Fx`: `spelling_for(&self, profile: &Path) -> String`,
      `script_auth(&self, code: i32, body: &Value)`, `script_valid(&self, profile: &Path, email: &str)`,
      `held_credential(&self, profile: &Path) -> Option<Value>` (read with M4a's
      `read_profile_credential(env, profile, recorded)`, as the profile holds it),
      `held_refresh_token(&self, profile: &Path) -> Option<String>`,
      `cc_writes_profile(&self, profile: &Path, edit: impl FnOnce(&mut Value))`,
      `rotate_profile(&self, profile: &Path, new_rt: &str)` and
      `land_replacement(&self, id: &AccountId, rt: &str)`.
    - `tests/bootstrap.rs` keeps its own `row`, `seed`, `marker`, `set_needs_bootstrap`,
      `bootstrap`, `bootstrapped` and `moved_from`.

**Spec:**
- §12.3, opening paragraph: a bootstrap runs only when the profile is quiescent and is missing,
  invalid, stale-marked, holding a credential other than the vault's, or recorded under another
  spelling; both locks are held through validation.
- §12.3 steps 2–8: read (unreadable or degraded aborts); displace when stale-marked; compose
  and write the file; always delete the item and verify it `Absent` (both spellings when it
  changed); verify the effective credential, then write the seed and the new spelling; seed;
  validate with `claude auth status --json` in the session's environment and directory, 10 s,
  and act on its table.
- §6.2 "Pending rescues before activation"; §6.3 `displaced/`; B.5.
- §12.2 "One spelling", "Profile marker", "Must-share entries" (through M4a's sync).
- §9.1: the storage-write lock (inside Task 6's write).
- B.29, B.43, B.46, B.58, B.60, B.62; Decisions 6 and 22; Review Focus 5 (its first half).

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/common/mod.rs`, add to the imports:

```rust
use tagteam_provider::process::{Captured, ScriptedSpawner};
```

Add the field `pub spawner: Arc<ScriptedSpawner>,` to `Fx`, after `pub process: Arc<FakeProcessProbe>,`
(M4a Task 9), with the doc comment
`/// Answers every login check any engine of this fixture runs (§12.3 step 8); it starts no process.`
In `Fx::build`, create it beside `process`:

```rust
        let spawner = Arc::new(ScriptedSpawner::new());
```

Then:
- add `spawner: spawner.clone(),` to `Fx::build`'s `EngineConfig` literal, after
  `process: process.clone(),`, and `spawner,` to the `Fx { .. }` it returns;
- add `spawner: self.spawner.clone(),` after `process: self.process.clone(),` in
  `Fx::engine_in` and in `Fx::engine_over_http`;
- add `spawner: fx.spawner.clone(),` after `process: fx.process.clone(),` in `FakeFx::new`.

In `crates/tagteam-engine/tests/oracle.rs`, add `spawner: fx.spawner.clone(),` after
`process: fx.process.clone(),`.

Append to `crates/tagteam-engine/tests/common/mod.rs`:

```rust
/// `claude auth status --json` (Appendix A.7) for a `claude.ai` login as `email`, whose config
/// dir is `spelling`: §12.3's `valid` for that account, when `spelling` is its profile's.
pub fn auth_status(spelling: &str, email: &str) -> Value {
    json!({
        "loggedIn": true,
        "authMethod": "claude.ai",
        "apiProvider": "firstParty",
        "analyticsDisabled": false,
        "projectsDirectory": format!("{spelling}/projects"),
        "configDirectory": spelling,
        "email": email,
        "orgName": "Personal",
        "subscriptionType": "max"
    })
}

/// `claude auth status --json` logged out (`authMethod` `none`): §12.3's `invalid`.
pub fn auth_logged_out(spelling: &str) -> Value {
    json!({
        "loggedIn": false,
        "authMethod": "none",
        "apiProvider": "firstParty",
        "analyticsDisabled": false,
        "projectsDirectory": format!("{spelling}/projects"),
        "configDirectory": spelling
    })
}

/// `claude auth status --json` logged in through an `apiKeyHelper` in the settings:
/// §12.3's `overridden`.
pub fn auth_helper(spelling: &str) -> Value {
    json!({
        "loggedIn": true,
        "authMethod": "api_key_helper",
        "apiKeySource": "apiKeyHelper",
        "apiProvider": "firstParty",
        "analyticsDisabled": false,
        "projectsDirectory": format!("{spelling}/projects"),
        "configDirectory": spelling
    })
}

/// The login check's process exiting `code`, with `stdout` captured.
pub fn auth_reply(code: i32, stdout: &[u8]) -> Captured {
    Captured::Exited {
        code: Some(code),
        signal: None,
        stdout: stdout.to_vec(),
        stderr: Vec::new(),
    }
}

/// A profile's credential and its login check (§12.3).
impl Fx {
    /// The spelling a bootstrap records for `profile` (§12.2): its canonical path, NFC. It works
    /// before the profile exists, by resolving `sessions/`, which it creates 0700.
    pub fn spelling_for(&self, profile: &Path) -> String {
        let parent = profile.parent().unwrap();
        fs::create_dir_all(parent).unwrap();
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
        let canonical = canonical_profile_path(parent)
            .unwrap()
            .join(profile.file_name().unwrap());
        self.cc.profile_spelling(&canonical)
    }

    /// Queues the next login check's reply: `claude auth status --json` exiting `code`, with
    /// `body` on stdout.
    pub fn script_auth(&self, code: i32, body: &Value) {
        self.spawner
            .push(auth_reply(code, body.to_string().as_bytes()));
    }

    /// Queues a `valid` reply for `email`'s session in `profile`.
    pub fn script_valid(&self, profile: &Path, email: &str) {
        self.script_auth(0, &auth_status(&self.spelling_for(profile), email));
    }

    /// The credential `profile` holds (§12.3 steps 2 and 6, Decision 22): the hashed item for
    /// the spelling its marker records, then `.credentials.json` where the profile is.
    pub fn held_credential(&self, profile: &Path) -> Option<Value> {
        let Read::Present(marker) = ProfileMarker::read(profile) else {
            return None;
        };
        match self
            .cc
            .read_profile_credential(&self.env, profile, &marker.config_dir)
        {
            Read::Present(c) => serde_json::from_slice(c.bytes()).ok(),
            _ => None,
        }
    }

    pub fn held_refresh_token(&self, profile: &Path) -> Option<String> {
        self.held_credential(profile)?["claudeAiOauth"]["refreshToken"]
            .as_str()
            .map(str::to_owned)
    }

    /// Claude Code in a session at `profile` writing its credential as `edit` changes it. On
    /// macOS the write lands in the profile's hashed item and the file it migrated from goes
    /// (Appendix A.3 "Plaintext migration"); on Linux it rewrites the file.
    pub fn cc_writes_profile(&self, profile: &Path, edit: impl FnOnce(&mut Value)) {
        let mut v = self
            .held_credential(profile)
            .expect("the profile holds a credential");
        edit(&mut v);
        let bytes = v.to_string().into_bytes();
        match self.platform {
            Platform::MacOs => {
                let (svc, acct) = self.profile_item(profile);
                self.kc.put(&svc, &acct, &bytes);
                let _ = fs::remove_file(profile.join(".credentials.json"));
            }
            Platform::Linux => self.set_profile_credential(profile, &bytes),
        }
    }

    /// Claude Code in a session at `profile` refreshing its login to `new_rt`.
    pub fn rotate_profile(&self, profile: &Path, new_rt: &str) {
        self.cc_writes_profile(profile, |v| {
            v["claudeAiOauth"]["refreshToken"] = json!(new_rt);
            v["claudeAiOauth"]["accessToken"] = json!(format!("at-{new_rt}"));
        });
    }

    /// An explicit replacement of `id`'s login that landed (§12.5 "Explicit replacements"): the
    /// vault holds `rt`'s generation and the account's `login_epoch` moved on, so every profile
    /// bootstrapped before it is stale-marked.
    pub fn land_replacement(&self, id: &AccountId, rt: &str) {
        let label = self.engine.store().unwrap().account(id).unwrap().unwrap().label;
        self.put_vault(id, &credential(&label, rt));
        rusqlite::Connection::open(self.env.data_dir().join("tagteam.db"))
            .unwrap()
            .execute(
                "UPDATE accounts SET login_epoch = login_epoch + 1 WHERE id = ?1",
                [id.as_str()],
            )
            .unwrap();
    }
}
```

Create `crates/tagteam-engine/tests/bootstrap.rs`:

```rust
//! §12.3 steps 2–8: a quiescent profile's bootstrap from the vault, and its validation. Driven
//! through the `test-hooks` seam `Engine::bootstrap_quiescent`, which runs what a quiescent
//! launch runs between its locks and its reservation, provenance aside (Task 10 puts it
//! first, so a rotation is captured before any bootstrap).
#![cfg(feature = "test-hooks")]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{
    Fx, auth_helper, auth_logged_out, auth_reply, auth_status, credential, two_accounts, vault_fp,
};
use serde_json::{Value, json};
use tagteam_cc::live::Platform;
use tagteam_core::AccountId;
use tagteam_engine::bootstrap::Trigger;
use tagteam_engine::store::AccountRow;
use tagteam_provider::Read;
use tagteam_provider::process::Captured;
use tagteam_provider::profile::{MARKER_FILE, ProfileMarker, Seed};
use tagteam_provider::splice::get_top_level;

fn row(fx: &Fx, id: &AccountId) -> AccountRow {
    fx.engine.store().unwrap().account(id).unwrap().unwrap()
}

fn seed(profile: &Path) -> Seed {
    match Seed::read(profile) {
        Read::Present(s) => s,
        other => panic!("no seed in {}: {other:?}", profile.display()),
    }
}

fn marker(profile: &Path) -> ProfileMarker {
    match ProfileMarker::read(profile) {
        Read::Present(m) => m,
        other => panic!("no marker in {}: {other:?}", profile.display()),
    }
}

/// What Task 11's per-launch check records for an `invalid` login (§12.3).
fn set_needs_bootstrap(profile: &Path) {
    Seed {
        needs_bootstrap: true,
        ..seed(profile)
    }
    .write(profile)
    .unwrap();
}

fn bootstrap(fx: &Fx, id: &AccountId) -> Option<Trigger> {
    fx.engine
        .bootstrap_quiescent(id, &fx.work_dir("app"))
        .unwrap()
}

/// `id`'s profile bootstrapped from the vault and validated, as a first launch leaves it.
fn bootstrapped(fx: &Fx, id: &AccountId, email: &str) -> PathBuf {
    let profile = fx.profile_dir(id);
    fx.script_valid(&profile, email);
    assert_eq!(bootstrap(fx, id), Some(Trigger::Missing));
    profile
}

/// The spelling `id`'s profile had before the data directory moved (§12.2, Decision 22): a
/// path that is gone, which a moved profile's marker still records.
fn moved_from(fx: &Fx, id: &AccountId) -> String {
    let old = fx.dir.path().join("moved-from/sessions").join(id.as_str());
    old.to_str().unwrap().to_owned()
}

#[test]
fn a_missing_profile_is_bootstrapped_from_the_vault_seeded_and_validated() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = fx.profile_dir(&a);
    let cwd = fx.work_dir("app");
    let spelling = fx.spelling_for(&profile);
    fx.script_valid(&profile, "a@x.co");

    let trigger = fx.engine.bootstrap_quiescent(&a, &cwd).unwrap();

    assert_eq!(trigger, Some(Trigger::Missing));
    assert_eq!(marker(&profile).config_dir, spelling, "§12.2: one recorded spelling");
    assert_eq!(
        seed(&profile),
        Seed {
            login_epoch: row(&fx, &a).login_epoch,
            seed_fp: vault_fp(&fx, &a),
            needs_bootstrap: false,
        },
        "step 6"
    );
    let held = fx.held_credential(&profile).unwrap();
    assert_eq!(held["claudeAiOauth"]["refreshToken"], "rt-a");
    assert!(
        held.get("mcpOAuth").is_none(),
        "step 4: a new profile starts with no machine-shared keys"
    );
    let mode = fs::metadata(profile.join(".credentials.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "§5, B.33");
    let (svc, acct) = fx.profile_item(&profile);
    assert_eq!(fx.kc.get(&svc, &acct), None, "step 5: never written, verified gone");
    let config = fs::read(profile.join(".claude.json")).unwrap();
    assert_eq!(
        get_top_level(&config, "hasCompletedOnboarding").unwrap(),
        Some(json!(true)),
        "step 7 (§12.4)"
    );
    let specs = fx.spawner.specs();
    assert_eq!(specs.len(), 1, "step 8: validated once");
    assert_eq!(specs[0].cwd.as_deref(), Some(cwd.as_path()));
    assert!(
        specs[0]
            .set
            .contains(&("CLAUDE_CONFIG_DIR".into(), spelling.clone().into())),
        "in the session's environment: {:?}",
        specs[0]
    );
}

#[test]
fn a_profile_in_step_is_only_seeded() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    let seeded = seed(&profile);

    assert_eq!(bootstrap(&fx, &a), None);

    assert_eq!(seed(&profile), seeded);
    assert_eq!(fx.spawner.specs().len(), 1, "no validation without a bootstrap");
    assert!(profile.join(".tagteam-baseline.json").exists(), "seeded (§12.4)");
}

#[test]
fn a_profile_its_login_check_found_invalid_is_bootstrapped_again() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    set_needs_bootstrap(&profile);
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::Invalid));

    assert!(!seed(&profile).needs_bootstrap);
    assert_eq!(fx.spawner.specs().len(), 2);
}

#[test]
fn a_stale_marked_profile_has_its_credential_displaced_then_is_bootstrapped() {
    // §12.3 step 3: it may hold a live generation of the login the replacement superseded.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    fx.rotate_profile(&profile, "rt-a-session");
    fx.land_replacement(&a, "rt-a-new");
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::StaleMarked));

    let displaced: Vec<Value> = fx
        .displaced()
        .iter()
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect();
    assert_eq!(displaced.len(), 1, "{displaced:?}");
    assert_eq!(displaced[0]["claudeAiOauth"]["refreshToken"], "rt-a-session");
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-new"));
    assert_eq!(seed(&profile).login_epoch, row(&fx, &a).login_epoch);
    let (svc, acct) = fx.profile_item(&profile);
    assert_eq!(fx.kc.get(&svc, &acct), None);
}

#[test]
fn a_profile_the_vault_moved_past_is_bootstrapped_without_a_copy_of_its_old_generation() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    fx.put_vault(&a, &credential("a@x.co", "rt-a-2"));
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::OtherCredential));

    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-2"));
    assert_eq!(seed(&profile).seed_fp, vault_fp(&fx, &a));
    assert!(fx.displaced().is_empty(), "this account's older generation, not displaced");
}

#[test]
fn a_profile_without_a_credential_is_bootstrapped() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    fs::remove_file(profile.join(".credentials.json")).unwrap();
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::OtherCredential));

    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a"));
}

#[test]
fn another_login_in_the_profile_is_displaced_before_the_bootstrap_overwrites_it() {
    // §12.5 "Identity drift", B.5: bytes that are not ours are displaced before being
    // overwritten.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    // Someone ran `/login` as c@x.co inside a session.
    fx.set_profile_identity(&profile, "c@x.co");
    fx.cc_writes_profile(&profile, |v| *v = Fx::credential_json("c@x.co", "rt-c"));
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::OtherCredential));

    let displaced: Vec<Value> = fx
        .displaced()
        .iter()
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect();
    assert_eq!(displaced.len(), 1, "{displaced:?}");
    assert_eq!(displaced[0]["claudeAiOauth"]["refreshToken"], "rt-c");
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a"));
    let config = fs::read(profile.join(".claude.json")).unwrap();
    let identity = get_top_level(&config, "oauthAccount").unwrap().unwrap();
    assert_eq!(identity["emailAddress"], "a@x.co", "re-seeded with the account's login");
}

#[test]
fn a_profile_recorded_under_another_spelling_takes_the_new_one_and_both_items_go() {
    // §12.2 "One spelling", §12.3 steps 5 and 6. A trailing slash changes the hash
    // (Appendix A.2), as a moved data directory changes the whole path.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    let current = fx.spelling_for(&profile);
    let old = format!("{current}/");
    ProfileMarker {
        config_dir: old.clone(),
        ..marker(&profile)
    }
    .write(&profile)
    .unwrap();
    let held = fs::read(profile.join(".credentials.json")).unwrap();
    let (old_item, acct) = fx.item_for_spelling(&old);
    let (new_item, _) = fx.item_for_spelling(&current);
    assert_ne!(old_item, new_item);
    fx.kc.put(&old_item, &acct, &held);
    fx.kc.put(&new_item, &acct, &held);
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::SpellingChanged));

    assert_eq!(fx.kc.get(&old_item, &acct), None, "the old spelling's item, verified gone");
    assert_eq!(fx.kc.get(&new_item, &acct), None, "and the current one's");
    assert_eq!(marker(&profile).config_dir, current, "recorded after the deletion");
    let last = fx.spawner.specs().pop().unwrap();
    assert!(
        last.set
            .contains(&("CLAUDE_CONFIG_DIR".into(), current.clone().into())),
        "{last:?}"
    );
}

#[test]
fn a_moved_profile_whose_old_item_holds_its_only_credential_keeps_its_machine_shared_keys() {
    // Decision 22, §12.3 steps 2, 4 and 5. The data directory moved, so the marker's spelling
    // names a path that is gone. Claude Code had moved the file into that spelling's item
    // (Appendix A.3): the only copy of the credential, and of the MCP token it minted.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    let current = fx.spelling_for(&profile);
    let old = moved_from(&fx, &a);
    let file = profile.join(".credentials.json");
    let mut held: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    held["mcpOAuth"] = json!({"srv": {"token": "minted-in-session"}});
    let (old_item, acct) = fx.item_for_spelling(&old);
    fx.kc.put(&old_item, &acct, held.to_string().as_bytes());
    fs::remove_file(&file).unwrap();
    ProfileMarker {
        config_dir: old.clone(),
        ..marker(&profile)
    }
    .write(&profile)
    .unwrap();
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::SpellingChanged));

    let written: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(
        written["mcpOAuth"]["srv"]["token"], "minted-in-session",
        "step 4 kept the machine-shared keys only the old item held"
    );
    assert_eq!(written["claudeAiOauth"]["refreshToken"], "rt-a");
    assert_eq!(
        fx.kc.get(&old_item, &acct),
        None,
        "step 5: the old spelling's item, deleted once the file holds its keys"
    );
    assert_eq!(marker(&profile).config_dir, current);
}

#[test]
fn a_moved_profile_s_stale_credential_is_read_where_the_profile_is_and_displaced() {
    // Decision 22, §12.3 steps 2 and 3: the file is read in the actual directory, not under the
    // old spelling's path, so a stale-marked credential there is saved before step 4
    // overwrites it.
    let fx = Fx::with_platform(Platform::Linux);
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    fx.rotate_profile(&profile, "rt-a-session");
    fx.land_replacement(&a, "rt-a-new");
    ProfileMarker {
        config_dir: moved_from(&fx, &a),
        ..marker(&profile)
    }
    .write(&profile)
    .unwrap();
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::StaleMarked));

    let displaced: Vec<Value> = fx
        .displaced()
        .iter()
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect();
    assert_eq!(displaced.len(), 1, "{displaced:?}");
    assert_eq!(displaced[0]["claudeAiOauth"]["refreshToken"], "rt-a-session");
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-new"));
    assert_eq!(marker(&profile).config_dir, fx.spelling_for(&profile));
}

#[test]
fn a_bootstrap_stopped_between_the_file_and_the_item_is_redone_with_the_same_machine_shared_keys() {
    // §12.3 step 5: the file is written before the item goes, so the item's machine-shared keys
    // are on disk when it does. Stopped between the two, the old item stays authoritative and
    // the seed unchanged, so the next bootstrap reads the same keys from the same item.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    // Claude Code in a session authenticated an MCP server, which moved the credential into the
    // profile's item.
    fx.cc_writes_profile(&profile, |v| {
        v["mcpOAuth"] = json!({"srv": {"token": "minted-in-session"}})
    });
    fx.put_vault(&a, &credential("a@x.co", "rt-a-2"));
    let (svc, acct) = fx.profile_item(&profile);
    let seeded = seed(&profile);
    fx.engine.fail_at(Some("bootstrap-after-credential-write"));

    let err = fx
        .engine
        .bootstrap_quiescent(&a, &fx.work_dir("app"))
        .unwrap_err();

    fx.engine.fail_at(None);
    assert!(err.to_string().contains("injected failure"), "{err}");
    let item: Value = serde_json::from_slice(&fx.kc.get(&svc, &acct).unwrap()).unwrap();
    assert_eq!(item["claudeAiOauth"]["refreshToken"], "rt-a", "the old item stays");
    let file: Value =
        serde_json::from_slice(&fs::read(profile.join(".credentials.json")).unwrap()).unwrap();
    assert_eq!(file["mcpOAuth"]["srv"]["token"], "minted-in-session", "already on disk");
    assert_eq!(seed(&profile), seeded, "nothing recorded");
    assert_eq!(fx.spawner.specs().len(), 1, "never validated");

    fx.script_valid(&profile, "a@x.co");
    assert_eq!(bootstrap(&fx, &a), Some(Trigger::OtherCredential));

    assert_eq!(fx.kc.get(&svc, &acct), None, "verified gone");
    let held = fx.held_credential(&profile).unwrap();
    assert_eq!(held["claudeAiOauth"]["refreshToken"], "rt-a-2");
    assert_eq!(held["mcpOAuth"]["srv"]["token"], "minted-in-session");
}

#[test]
fn an_item_that_cannot_be_verified_gone_aborts_and_the_next_bootstrap_retries() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    fx.cc_writes_profile(&profile, |_| {});
    fx.put_vault(&a, &credential("a@x.co", "rt-a-2"));
    let (svc, acct) = fx.profile_item(&profile);
    let seeded = seed(&profile);
    fx.kc.set_fail_delete(&svc, true);

    let err = fx
        .engine
        .bootstrap_quiescent(&a, &fx.work_dir("app"))
        .unwrap_err();

    fx.kc.set_fail_delete(&svc, false);
    assert!(err.to_string().contains("could not be verified gone"), "{err}");
    assert!(fx.kc.get(&svc, &acct).is_some());
    assert_eq!(seed(&profile), seeded, "unrecorded, so the next launch bootstraps again");

    fx.script_valid(&profile, "a@x.co");
    assert_eq!(bootstrap(&fx, &a), Some(Trigger::OtherCredential));
    assert_eq!(fx.kc.get(&svc, &acct), None);
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-2"));
}

#[test]
fn an_unreadable_or_degraded_profile_credential_aborts_without_writing() {
    // §12.3 step 2: tagteam never overwrites a credential it could not read.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    set_needs_bootstrap(&profile);
    let (svc, acct) = fx.profile_item(&profile);
    fx.kc.set_unreadable(&svc, &acct, true);
    let file = fs::read(profile.join(".credentials.json")).unwrap();

    // The file covers the unreadable item: degraded.
    let err = fx
        .engine
        .bootstrap_quiescent(&a, &fx.work_dir("app"))
        .unwrap_err();
    assert_eq!(err.kind(), "unreadable", "{err}");
    assert_eq!(fs::read(profile.join(".credentials.json")).unwrap(), file);

    // Nothing covers it: unreadable.
    fs::remove_file(profile.join(".credentials.json")).unwrap();
    let err = fx
        .engine
        .bootstrap_quiescent(&a, &fx.work_dir("app"))
        .unwrap_err();
    assert_eq!(err.kind(), "unreadable", "{err}");
    assert!(!profile.join(".credentials.json").exists());

    assert!(seed(&profile).needs_bootstrap, "still due");
    assert_eq!(fx.spawner.specs().len(), 1, "only the first bootstrap validated");
}

#[test]
fn a_pending_rescue_is_adopted_before_the_profile_is_composed() {
    // §6.2: a rescue consumed the vault's generation; bootstrapping the vault alone would hand
    // the session a spent refresh token.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
    set_needs_bootstrap(&profile);
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::Invalid));

    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-2"));
    assert_eq!(seed(&profile).seed_fp, vault_fp(&fx, &a));
}

#[test]
fn an_invalid_login_at_bootstrap_deletes_the_profile_and_refuses() {
    // §12.3 step 8, B.29: only `invalid` deletes a profile; the next `run` starts afresh.
    let cases: [fn(&str) -> (i32, Value); 2] = [
        |s| (1, auth_logged_out(s)),
        |s| (0, auth_status(s, "c@x.co")),
    ];
    for case in cases {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let profile = fx.profile_dir(&a);
        let (code, body) = case(&fx.spelling_for(&profile));
        fx.script_auth(code, &body);

        let err = fx
            .engine
            .bootstrap_quiescent(&a, &fx.work_dir("app"))
            .unwrap_err();

        assert_eq!(err.kind(), "login-invalid", "{err}");
        assert!(!profile.exists(), "the profile is deleted: {err}");
    }
}

#[test]
fn an_overridden_drifted_unknown_or_unreachable_login_refuses_and_keeps_the_profile() {
    // §12.3 step 8's table, B.62.
    let rows: [(fn(&str) -> Captured, &str); 5] = [
        (
            |s| auth_reply(0, auth_helper(s).to_string().as_bytes()),
            "login-overridden",
        ),
        (
            |_| auth_reply(0, auth_status("/somewhere/else", "a@x.co").to_string().as_bytes()),
            "login-drifted",
        ),
        (|_| Captured::TimedOut, "login-unknown"),
        (|_| auth_reply(0, b"not json"), "login-unknown"),
        (
            |_| Captured::SpawnFailed("No such file or directory".into()),
            "launch-unreachable",
        ),
    ];
    for (reply, kind) in rows {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let profile = fx.profile_dir(&a);
        fx.spawner.push(reply(&fx.spelling_for(&profile)));

        let err = fx
            .engine
            .bootstrap_quiescent(&a, &fx.work_dir("app"))
            .unwrap_err();

        assert_eq!(err.kind(), kind, "{err}");
        assert!(profile.join(MARKER_FILE).exists(), "{kind}: the profile is kept");
        assert_eq!(seed(&profile).seed_fp, vault_fp(&fx, &a), "{kind}");
    }
}

#[test]
fn a_fresh_machine_gets_projects_and_history_created_empty_and_shared() {
    // Review Focus 5, first half: §3's create-only row, §12.2 "Must-share entries", B.43.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let claude = fx.env.home.join(".claude");
    fs::remove_dir_all(claude.join("projects")).unwrap();
    fs::remove_file(claude.join("history.jsonl")).unwrap();

    let profile = bootstrapped(&fx, &a, "a@x.co");

    assert!(
        fs::read_dir(claude.join("projects")).unwrap().next().is_none(),
        "created empty"
    );
    assert_eq!(fs::read(claude.join("history.jsonl")).unwrap(), b"");
    for name in ["projects", "history.jsonl"] {
        assert_eq!(
            fs::read_link(profile.join(name)).unwrap(),
            fs::canonicalize(claude.join(name)).unwrap(),
            "{name} is shared"
        );
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test bootstrap`
Expected: compile errors, from `tests/common/mod.rs` first:
`struct EngineConfig has no field named spawner`. Then from `tests/bootstrap.rs`:
`unresolved import tagteam_engine::bootstrap` and
`no method named bootstrap_quiescent found for struct Engine`.

- [ ] **Step 3: Implement**

**The spawner port.** In `crates/tagteam-engine/src/engine.rs`, add
`use tagteam_provider::process::ProcessSpawner;`. In `EngineConfig`, after `process` (M4a):

```rust
    /// The port the login check (`claude auth status`) spawns through (§12.3 step 8,
    /// Decision 10): `SystemSpawner` in production, `ScriptedSpawner` in tests.
    pub spawner: Arc<dyn ProcessSpawner>,
```

In `Engine`, after `process`:

```rust
    // Read only by the bootstrap's validation, which only the `test-hooks` seam reaches until
    // `launch` (Task 10) calls it; Task 10 removes this attribute.
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub(crate) spawner: Arc<dyn ProcessSpawner>,
```

In `Engine::new`, after `process: cfg.process,`:

```rust
            spawner: cfg.spawner,
```

Add `spawner: Arc::new(tagteam_provider::process::ScriptedSpawner::new()),` after the `process`
field in:
- the test module's `test_config` (`engine.rs`);
- `T::new` (`crates/tagteam-engine/src/testutil.rs`);
- `an_engine_over_a_lazy_client_does_not_build_it` (`crates/tagteam-engine/src/lazy_http.rs`).

In `crates/tagteam/src/app.rs` (`build_engine`) and `crates/tagteam/src/statusline.rs`
(`engine`), add `use tagteam_provider::process::SystemSpawner;` and `spawner: Arc::new(SystemSpawner),`
after the `process` field. The statusline never launches or validates, so its spawner is never
called, like its process probe (M4a Task 9).

**`crates/tagteam-engine/src/lifecycle.rs`:** M4a Task 9's `fn remove_profile(&self, p: &dyn Provider, row: &AccountRow)`
becomes `pub(crate) fn remove_profile`, so that `invalid` at a bootstrap deletes the profile
the way `remove` does (§12.3 step 8). Its body is unchanged.

**`crates/tagteam-engine/src/error.rs`.** After the imports, add:

```rust
/// " (from <source>)" when `claude auth status` named where the overriding key came from.
fn from_source(source: &Option<String>) -> String {
    source
        .as_deref()
        .map_or_else(String::new, |s| format!(" (from {s})"))
}
```

After Task 8's `RequiresSession` variant:

```rust
    /// §12.3: the session would log in by another method than the account's own login.
    #[error(
        "position {position}'s session would log in by {method}{} rather than with its account's login; remove what sets it (an `apiKeyHelper` or `env` entry in the shared settings, the project's own settings, or a workload-identity profile), then run again",
        from_source(key_source)
    )]
    LoginOverridden {
        position: u32,
        method: String,
        key_source: Option<String>,
    },
    /// §12.3: logged out, or logged in as another account.
    #[error(
        "position {position}'s session would not be logged in as its account ({detail}); the next `tagteam run` sets its profile up afresh"
    )]
    LoginInvalid { position: u32, detail: String },
    /// §12.3: Claude Code resolves another config dir than the profile's recorded spelling.
    #[error(
        "position {position}'s session would use {reported} as its config dir, not its profile, so it was not started"
    )]
    LoginDrifted { position: u32, reported: String },
    /// §12.3: the check timed out, or its output did not parse.
    #[error("position {position}'s login could not be confirmed ({detail}), so it was not started")]
    LoginUnknown { position: u32, detail: String },
    /// §12.3: the launch command could not be spawned.
    #[error("the launch command could not be started: {detail}")]
    LaunchUnreachable { detail: String },
```

In `kind()`, after Task 8's three arms:

```rust
            EngineError::LoginOverridden { .. } => "login-overridden",
            EngineError::LoginInvalid { .. } => "login-invalid",
            EngineError::LoginDrifted { .. } => "login-drifted",
            EngineError::LoginUnknown { .. } => "login-unknown",
            EngineError::LaunchUnreachable { .. } => "launch-unreachable",
```

In `kind_is_pinned_for_every_variant`, after Task 8's three cases:

```rust
            (
                EngineError::LoginOverridden {
                    position: 1,
                    method: "api_key_helper".into(),
                    key_source: Some("apiKeyHelper".into()),
                },
                "login-overridden",
            ),
            (
                EngineError::LoginInvalid {
                    position: 1,
                    detail: "d".into(),
                },
                "login-invalid",
            ),
            (
                EngineError::LoginDrifted {
                    position: 1,
                    reported: "/x".into(),
                },
                "login-drifted",
            ),
            (
                EngineError::LoginUnknown {
                    position: 1,
                    detail: "d".into(),
                },
                "login-unknown",
            ),
            (
                EngineError::LaunchUnreachable { detail: "d".into() },
                "launch-unreachable",
            ),
```

**`crates/tagteam-engine/src/lib.rs`**: add `pub mod bootstrap;` after `pub mod active;`.

**Create `crates/tagteam-engine/src/bootstrap.rs`:**

```rust
//! §12.3 steps 2–8: a quiescent profile's bootstrap from the vault, and its validation. The
//! launch (§12.5, `launch.rs`) calls it under `MutationGuard` and the account lock, which stay
//! held through validation; step 1's gate refresh ran before it took them.

use std::path::Path;

#[cfg(feature = "test-hooks")]
use tagteam_core::AccountId;
use tagteam_core::Fingerprint;
use tagteam_provider::atomic::ensure_private_dir;
#[cfg(feature = "test-hooks")]
use tagteam_provider::profile::profile_path;
use tagteam_provider::profile::{ProfileMarker, Seed, canonical_profile_path};
use tagteam_provider::provider::Validity;
use tagteam_provider::{MutationGuard, Provenance, Provider, Read, ReadError};

use crate::account_lock::AccountLock;
use crate::displace::displace;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::provenance::identity_drifted;
use crate::store::AccountRow;

/// Why a quiescent profile needs a bootstrap (§12.3's opening list), in the order
/// `bootstrap_trigger` checks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// No seed: never bootstrapped.
    Missing,
    /// The per-launch check found the login `invalid` (`Seed.needs_bootstrap`).
    Invalid,
    /// The seed's epoch is not the account's `login_epoch` (§12.5).
    StaleMarked,
    /// The canonical path is no longer the spelling the marker records (§12.2).
    SpellingChanged,
    /// No credential, another generation than the vault's, or another login (§12.5).
    OtherCredential,
}

impl Trigger {
    /// For the log.
    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::Missing => "missing",
            Trigger::Invalid => "invalid",
            Trigger::StaleMarked => "stale-marked",
            Trigger::SpellingChanged => "spelling-changed",
            Trigger::OtherCredential => "other-credential",
        }
    }
}

/// §12.2 "One spelling": the profile's canonical path as the provider exports it. A path that
/// is not UTF-8 is refused: its lossy text would name another directory and Keychain item.
// Only the `test-hooks` seam reaches this until `launch` (Task 10) calls the bootstrap; Task 10
// removes this attribute.
#[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
fn current_spelling(p: &dyn Provider, profile: &Path) -> Result<String, EngineError> {
    let canonical = canonical_profile_path(profile)?;
    if canonical.to_str().is_none() {
        return Err(EngineError::InvalidInput(format!(
            "{} is not valid UTF-8, so it cannot be exported as a session's config dir",
            canonical.display()
        )));
    }
    Ok(p.profile_spelling(&canonical))
}

/// §12.3's validation table, but for `invalid`'s deletion: `None` launches.
// Only the `test-hooks` seam reaches this until `launch` (Task 10) calls the bootstrap; Task 10
// removes this attribute.
#[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
pub(crate) fn refusal(row: &AccountRow, validity: Validity) -> Option<EngineError> {
    let position = row.position;
    match validity {
        Validity::Valid => None,
        Validity::Invalid(detail) => Some(EngineError::LoginInvalid { position, detail }),
        Validity::Overridden { method, source } => Some(EngineError::LoginOverridden {
            position,
            method,
            key_source: source,
        }),
        Validity::Drifted { reported } => Some(EngineError::LoginDrifted { position, reported }),
        Validity::Unknown(detail) => Some(EngineError::LoginUnknown { position, detail }),
        Validity::Unreachable(detail) => Some(EngineError::LaunchUnreachable { detail }),
    }
}

// Only the `test-hooks` seam reaches these until `launch` (Task 10) calls the bootstrap; Task 10
// removes this attribute.
#[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
impl Engine {
    /// The profile's marker, when it names `row`. A marker naming another account or provider
    /// refuses: its spelling names someone else's Keychain item, which is never touched here.
    pub(crate) fn own_marker(
        &self,
        row: &AccountRow,
        profile: &Path,
    ) -> Result<Option<ProfileMarker>, EngineError> {
        match ProfileMarker::read(profile) {
            Read::Present(m) if m.account_id == row.id && m.provider == row.provider => Ok(Some(m)),
            Read::Present(_) => Err(EngineError::InvalidInput(format!(
                "the marker in {} names another account; move the profile aside, then run again",
                profile.display()
            ))),
            Read::Absent => Ok(None),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
        }
    }

    /// §12.2 "Profile marker", for a quiescent launch: a first launch creates the profile
    /// (0700) and its marker, whose `configDir` is the current spelling; a later one updates
    /// `outer` to this launch's home, before the links are synced from it. Only a bootstrap
    /// changes `configDir` (§12.3 step 6).
    pub(crate) fn mark_profile(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        profile: &Path,
    ) -> Result<ProfileMarker, EngineError> {
        let outer = p.outer_home(&self.env);
        let marker = match self.own_marker(row, profile)? {
            Some(m) if m.outer == outer => return Ok(m),
            Some(m) => ProfileMarker { outer, ..m },
            None => {
                ensure_private_dir(profile)?;
                ProfileMarker {
                    provider: row.provider.clone(),
                    account_id: row.id.clone(),
                    config_dir: current_spelling(p, profile)?,
                    outer,
                }
            }
        };
        marker.write(profile)?;
        Ok(marker)
    }

    /// The vault's current generation for `row`, which a bootstrap writes into the profile.
    pub(crate) fn vault_generation(&self, row: &AccountRow) -> Result<Vec<u8>, EngineError> {
        match self.vault.read(&row.id) {
            Read::Present(b) if !b.is_empty() => Ok(b),
            Read::Unreadable(source) => Err(EngineError::UnreadableAccount {
                position: row.position,
                label: row.label.clone(),
                source,
            }),
            _ => Err(EngineError::InvalidInput(format!(
                "{} (position {}) has no stored credential; log in and run `tagteam add` again",
                row.label, row.position
            ))),
        }
    }

    /// §12.3 step 2: the credential `profile` holds, read as Claude Code reads it: the item named
    /// from `spelling`, then the file in `profile`, its actual directory (Decision 22); `None`
    /// when it has none. Unreadable, degraded (the file covered an item that could not be read,
    /// so it may be an older generation) and empty (a Keychain timeout can look empty) are
    /// errors: tagteam never overwrites a credential it could not read.
    fn read_held(
        &self,
        p: &dyn Provider,
        profile: &Path,
        spelling: &str,
    ) -> Result<Option<Vec<u8>>, EngineError> {
        let damaged = |detail: &str| {
            EngineError::Unreadable(ReadError::new(
                format!("the credential of {}", profile.display()),
                detail,
            ))
        };
        match p.read_profile_credential(&self.env, profile, spelling) {
            Read::Present(c) if c.provenance() == Provenance::Degraded => Err(damaged(
                "its Keychain item could not be read, and the file that covered it may be out of date",
            )),
            Read::Present(c) if c.is_empty() => Err(damaged(
                "it read back empty, which a Keychain timeout can cause",
            )),
            Read::Present(c) => Ok(Some(c.bytes().to_vec())),
            Read::Absent => Ok(None),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
        }
    }

    /// §12.3: which trigger applies to the quiescent `profile` now, if any, in `Trigger`'s
    /// order. `mark_profile` has run, so the marker exists. Reads only.
    pub(crate) fn bootstrap_trigger(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        profile: &Path,
    ) -> Result<Option<Trigger>, EngineError> {
        let seed = match Seed::read(profile) {
            Read::Present(s) => s,
            Read::Absent => return Ok(Some(Trigger::Missing)),
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        if seed.needs_bootstrap {
            return Ok(Some(Trigger::Invalid));
        }
        if seed.login_epoch != row.login_epoch {
            return Ok(Some(Trigger::StaleMarked));
        }
        let Some(marker) = self.own_marker(row, profile)? else {
            return Ok(Some(Trigger::Missing));
        };
        if marker.config_dir != current_spelling(p, profile)? {
            return Ok(Some(Trigger::SpellingChanged));
        }
        let vault_fp = p.fingerprint(&self.vault_generation(row)?);
        let Some(held) = self.read_held(p, profile, &marker.config_dir)? else {
            return Ok(Some(Trigger::OtherCredential));
        };
        if p.fingerprint(&held) != vault_fp {
            return Ok(Some(Trigger::OtherCredential));
        }
        match p.profile_identity(&self.env, profile) {
            Read::Present(login) if !identity_drifted(&login, row) => Ok(None),
            Read::Present(_) | Read::Absent => Ok(Some(Trigger::OtherCredential)),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
        }
    }

    /// §12.4's seed of `profile`, the profile's actual directory, whatever spelling its marker
    /// records (Decision 22).
    pub(crate) fn seed_of(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        profile: &Path,
    ) -> Result<(), EngineError> {
        p.seed_profile(&self.env, profile, &p.parse_identity(&row.identity_json)?)?;
        Ok(())
    }

    /// §12.3 steps 2–8 for the quiescent `profile`, whose marker exists. The caller holds
    /// `guard` and `lock` (this account's) throughout, validation included. `cwd` is the
    /// directory `claude` will run in, and `program` the launch command `plan_run` resolved,
    /// which the validation spawns (Decision 20).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn bootstrap_profile(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        profile: &Path,
        cwd: &Path,
        program: &Path,
        guard: &MutationGuard,
        lock: &AccountLock,
    ) -> Result<(), EngineError> {
        debug_assert_eq!(lock.id(), &row.id, "the caller holds this account's lock");
        // §6.2 "Pending rescues before activation": a rescue consumed the vault's generation.
        self.settle_rescues(p, row, lock)?;
        let vault = self.vault_generation(row)?;
        let v_fp = p.fingerprint(&vault).ok_or_else(|| {
            EngineError::InvalidInput(format!(
                "position {}'s stored credential has no generation to start a session from",
                row.position
            ))
        })?;
        let marker = self.own_marker(row, profile)?.ok_or_else(|| {
            EngineError::InvalidInput(format!("{} has no profile marker", profile.display()))
        })?;
        let recorded = marker.config_dir.clone();
        let current = current_spelling(p, profile)?;
        // 2. As the profile holds it: the recorded spelling's item, then the file where the
        //    profile is now (Decision 22).
        let held = self.read_held(p, profile, &recorded)?;
        // 3.
        if let Some(bytes) = &held {
            self.displace_held(p, row, profile, bytes, &v_fp)?;
        }
        // 4. Account-scoped keys from the vault, machine-shared ones from the profile's own.
        let composed = p.compose_profile_credential(&vault, held.as_deref())?;
        p.write_profile_credential(&self.env, &current, guard, &composed)?;
        hooks::point(self, "bootstrap-after-credential-write")?;
        // 5. Always, whatever the reason: an item left behind stays authoritative over the file.
        p.delete_profile_credential(&self.env, profile, &current)?;
        if recorded != current {
            p.delete_profile_credential(&self.env, profile, &recorded)?;
        }
        // 6. What Claude Code will read now must be the vault's current generation.
        let effective = self.read_held(p, profile, &current)?;
        let in_step = effective.as_deref().is_some_and(|e| {
            p.fingerprint(e).as_ref() == Some(&v_fp)
                && p.access_fingerprint(e) == p.access_fingerprint(&vault)
        });
        if !in_step {
            return Err(EngineError::InvalidInput(format!(
                "the credential a session of position {} would read is not the vault's current generation, so it was not started",
                row.position
            )));
        }
        Seed {
            login_epoch: row.login_epoch,
            seed_fp: v_fp.as_str().to_owned(),
            needs_bootstrap: false,
        }
        .write(profile)?;
        if recorded != current {
            ProfileMarker {
                config_dir: current.clone(),
                ..marker
            }
            .write(profile)?;
        }
        // 7.
        self.seed_of(p, row, profile)?;
        // 8.
        self.validate_bootstrap(p, row, &current, cwd, program)
    }

    /// §12.3 step 3, and B.5: a stale-marked profile's credential may be a live generation of
    /// the login a replacement superseded, and one whose identity drifted is another login
    /// altogether. Either is saved to `displaced/` before step 4 overwrites it; a failed save
    /// aborts. The vault's own generation, or an older one of this account, is not saved. The
    /// identity is read in `profile`, where the profile is now (Decision 22).
    fn displace_held(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        profile: &Path,
        held: &[u8],
        v_fp: &Fingerprint,
    ) -> Result<(), EngineError> {
        let held_fp = p.fingerprint(held);
        if held_fp.as_ref() == Some(v_fp) {
            return Ok(());
        }
        let stale = matches!(Seed::read(profile), Read::Present(s) if s.login_epoch != row.login_epoch);
        let identity = p.profile_identity(&self.env, profile);
        let foreign = matches!(&identity, Read::Present(i) if identity_drifted(i, row));
        let reason = match (stale, foreign) {
            (_, true) => "foreign-profile-login",
            (true, false) => "replaced-profile-login",
            (false, false) => return Ok(()),
        };
        let raw = identity.present().map(|i| i.raw);
        let id = displace(self, &row.provider, held, held_fp.as_ref(), reason, raw.as_ref())?;
        tracing::warn!(
            position = row.position,
            account = %row.id,
            displaced = %id,
            reason,
            "saved the session profile's credential before bootstrapping over it"
        );
        Ok(())
    }

    /// §12.3 step 8 at a bootstrap, with both locks held. A signal recorded by the end of the
    /// check (whose process the spawner killed) interrupts the launch. `invalid` deletes the
    /// profile (B.29); every other refusal keeps it.
    fn validate_bootstrap(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        spelling: &str,
        cwd: &Path,
        program: &Path,
    ) -> Result<(), EngineError> {
        let identity = p.parse_identity(&row.identity_json)?;
        let validity = p.validate_profile(
            &self.env,
            spelling,
            cwd,
            program,
            &identity,
            self.spawner.as_ref(),
            self.cancel(),
        );
        // §12.5 "Signals": read through the token, never through the `Validity` text.
        if let Some(signal) = self.cancel().requested() {
            return Err(EngineError::Interrupted(signal));
        }
        if matches!(validity, Validity::Invalid(_)) {
            tracing::warn!(
                position = row.position,
                account = %row.id,
                "the session profile's login is invalid; deleting the profile"
            );
            self.remove_profile(p, row)?;
        }
        refusal(row, validity).map_or(Ok(()), Err)
    }
}

#[cfg(feature = "test-hooks")]
impl Engine {
    /// Tests only (Task 9, before `launch` exists): what a quiescent launch runs between its
    /// locks and its reservation, provenance aside. That is the marker, the link sync, then a
    /// bootstrap for a §12.3 trigger, else the seed, under `MutationGuard` (30 s) and the
    /// account lock. Returns the trigger that applied. With no plan to resolve it, the
    /// validation's program is the launch command by name: the scripted spawner records it and
    /// never runs it.
    pub fn bootstrap_quiescent(
        &self,
        id: &AccountId,
        cwd: &Path,
    ) -> Result<Option<Trigger>, EngineError> {
        let guard = MutationGuard::acquire(&self.env, MutationGuard::BOOTSTRAP_TIMEOUT)?;
        let lock = self.lock_account(id)?;
        let row = self
            .store()?
            .account(id)?
            .ok_or_else(|| EngineError::NoSuchAccount(id.to_string()))?;
        let p = self.provider(&row.provider)?;
        let p = p.as_ref();
        let profile = profile_path(&self.env, &row.id);
        self.mark_profile(p, &row, &profile)?;
        self.sync_profile_links(p, &profile, false)?;
        let trigger = self.bootstrap_trigger(p, &row, &profile)?;
        let program = Path::new(p.launch_command());
        match trigger {
            Some(_) => self.bootstrap_profile(p, &row, &profile, cwd, program, &guard, &lock)?,
            None => self.seed_of(p, &row, &profile)?,
        }
        Ok(trigger)
    }
}
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-engine --features test-hooks --test bootstrap`
Expected: PASS, 17 tests.

Run: `cargo test -p tagteam-engine --test bootstrap`
Expected: PASS with 0 tests: the file is `test-hooks` only.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS. `kind_is_pinned_for_every_variant` has its five new cases. Every other test
compiles again with the `spawner` field, and none validates.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS.

Existing tests this changes: none fail. Every `EngineConfig` literal named in Files fails to
compile until it has `spawner`. That covers `engine.rs`'s `test_config`, `testutil.rs`,
`lazy_http.rs`, `tests/common/mod.rs` (four sites), `tests/oracle.rs`, `app.rs` and
`statusline.rs`.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: both clippy runs clean. Without `test-hooks`, nothing calls the bootstrap yet; the
`cfg_attr(not(feature = "test-hooks"), allow(dead_code))` attributes on `current_spelling`,
`refusal`, the first `impl Engine` block of `bootstrap.rs` and `Engine.spawner` keep the second
run from failing on `dead_code`. Task 10 deletes them.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/bootstrap.rs crates/tagteam-engine/src/lib.rs \
  crates/tagteam-engine/src/engine.rs crates/tagteam-engine/src/error.rs \
  crates/tagteam-engine/src/lifecycle.rs crates/tagteam-engine/src/testutil.rs \
  crates/tagteam-engine/src/lazy_http.rs crates/tagteam/src/app.rs \
  crates/tagteam/src/statusline.rs crates/tagteam-engine/tests/common/mod.rs \
  crates/tagteam-engine/tests/oracle.rs crates/tagteam-engine/tests/bootstrap.rs
git commit -m "Bootstrap a session profile from the vault and validate its login"
```

---

### Task 10: Launch under the locks (§12.5 steps 1–5)

`Engine::launch` takes `plan_run`'s `Session` target to a reservation. Before any lock it
settles an interrupted switch (§9.6) and refreshes a vault credential about to expire through
the gate (§12.3 step 1). Under `MutationGuard` (30 s) and the account lock it then:
1. decides again: the target may have been removed, or become the live login;
2. removes dead reservations, and merges back a baseline a killed session left;
3. brings the marker and the links up to date, then prepares a quiescent profile (provenance,
   then a bootstrap or the seed), or joins a running one as it is;
4. creates the reservation;
5. releases both locks.

The login check (step 6) and the exit handling are Task 11's. The spawn (step 7) is Task 12's.

**Readings of the spec this task commits to:**
- **The gate refresh** (§12.3 step 1) runs only when the vault's access token is inside the
  freshen window (§7.2's 10 minutes, `switch.rs`'s `due`), and only for a refreshable kind.
  Refreshing on every launch would rotate a profile that is in step out of step, and every
  launch would then re-bootstrap. Its outcomes map as §12.3 step 1 says, and otherwise as
  §7.2's table does for a direct switch target:
  - `Refreshed`, `AlreadyFresh` and `Busy` go on. The account lock this launch waits for, and
    the rescue settlement under it, pick up another process's refresh.
  - `Transient` with `rescued`, `Transient` `rescue-unreadable`, `Unpersisted` and `Dead`
    refuse: the vault's generation is consumed or dead (`rescue-pending`, `relogin-required`).
  - Any other `Transient`, and `Systemic`, go on with the stored credential and a warning.
  - `Owned(Live)` goes on: the re-check under the locks decides it. `Owned(Session)` goes on
    too, since this launch will join the running session. `Owned(Journal)` goes on with a
    warning, and the mutation lock decides, as for a switch.
  - `Conflict` refuses (`profile-conflict`).
  - A quarantined target is never refreshed: when it is due it is refused as `Dead` is, and
    otherwise it goes on with the warning that it needs a new login (§7.2).
- **The mutation lock** is `guard_or_refuse` with a 30 s wait (§9.1, "30 s for `run`
  bootstrap"). An unresolved interrupted switch for the provider still refuses: it may have
  left the target live.
- **Step 1 decides again** (B.47) on the row read under the account lock:
  - **Removed** (Decision 7) or **now the live login**: `TargetChanged { why }`, a new error
    (Decision 14). The CLI prints `why` and calls `plan_run` once more. The new plan runs plain
    `claude` (a mapping that went away with its account, or a live login), refuses under
    `--require-session`, names a missing account, or launches; a second `TargetChanged`
    refuses.
  - A login replaced by an API key since `plan_run` is refused (`api-key-account`).
- **A removal before the first read of the account.** A `remove` can complete after `plan_run`
  and before `freshen_for_launch`, whose vault read then finds nothing. That is the same race
  as a removal before the locks (Decision 7, B.47), so it must be `TargetChanged` too, not the
  vault read's `invalid-input`. A pre-lock read of the account that fails re-reads the account
  row. If the account is gone, the launch returns `TargetChanged` with `relocked_target`'s own
  wording. Only a present account propagates the read's failure. A re-read that fails itself
  leaves the read's failure as it is. The gate (`refresh_stored`) reads the row under its own
  lock and answers a missing account with `Transient`, so the launch goes on to the re-check
  under the locks. The hook point `launch-before-freshen`, after `settle_or_refuse`, makes the
  race testable.
- **Step 2.** Dead reservations (`Free`) are removed first, so a killed session's leftover
  never makes the profile look owned. A quiescent profile whose baseline is left over gets its
  merge-back now. A failure aborts the launch with the profile and its baseline untouched
  (§12.4, B.44). A conflict summary from it is a launch warning. The baseline is looked for in
  the profile's actual directory (Decision 22). After a move the marker still names the old
  path, and a merge-back that looked there would find no baseline. The spelling-change bootstrap
  would then seed over the killed session's changes.
- **Quiescent** means `session_state` is `NoProfile` or `Quiescent`. `Unreadable` counts as
  owned (§12.6), so the launch joins with a warning. It never seeds or bootstraps over a
  session it could not rule out.
- **Step 3, quiescent.** The marker comes first: created at a first launch, with `outer`
  updated otherwise (§12.2). Then come the sync, the pending rescues (§6.2: a rescue consumed
  the vault's generation, so provenance must compare against its successor), and the
  provenance (M4a's `apply_provenance`):
  - `Conflict` refuses (`profile-conflict`).
  - `Unreadable` aborts (`unreadable`): the profile's credential, seed, marker or identity
    could not be read.
  - `VaultMovedOn` and `ReplacementWins` bootstrap. Task 9's bootstrap displaces a
    stale-marked credential.
  - `InStep`, `Captured` and `NotApplicable` bootstrap only for a remaining §12.3 trigger
    (Task 9's `bootstrap_trigger`). Otherwise the profile is seeded.
- **Step 3, joining.** The sync runs with `joining`. It creates only missing links, and never
  seeds or bootstraps (B.28).
- **Step 4.** The reservation is created under both locks (§12.5 "Launch reservation"), in the
  directory the profile's `session_state` reads, and `Launched.spelling` is the marker's
  recorded spelling (`recorded_spelling`, defined here, its only user). A live `<pid>.lock`
  already there (an orphaned `claude` of an earlier process with this pid, Task 4) refuses the
  launch as `launch-unreachable`, naming the file, and is left alone.
- **Default-feature lint.** `launch` is the bootstrap's production caller, so this task deletes
  Task 9's `#[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]` attributes and their
  comments: on `current_spelling`, `refusal` and the first `impl Engine` block of
  `bootstrap.rs`, and on `Engine.spawner` in `engine.rs`.
- **The environment warnings** (§12.5 "Environment", Decision 18) are computed here, once, into
  `Launched.warnings`, after every other warning: one per variable of `SessionEnv.remove` that
  this process has set, and one when the outer home sets the provider's home variable to
  another directory than the profile. Which variables are set is read from `Env.vars`, where
  the CLI records them (Task 12), so an engine test sees only the ones it puts there. The home
  variable's value is `Provider::session_dir` over the engine's `Env`: the outer home's inside
  a run shell, not the shell's own.
- **The launch command.** `launch` takes the path `plan_run` resolved and hands it to a
  bootstrap's validation (Decision 20).
- **Signals.** Every lock wait is a cancellation point (M3a), so a signal recorded before the
  locks launches nothing (§12.5 "Signals"). Nothing has been created by then.
- **One live reservation per process per profile** (`<pid>.lock`, Task 4). Engine tests
  therefore never keep two `Launched` of one profile alive: they stand in for the other session
  with M4a's `Fx::hold_reservation` (`<pid>-<n>.lock`). They simulate a killed `tagteam` by
  dropping a `Launched` without `finish_run`, which leaves its file dead, since dropping a
  reservation never unlinks it.

**Files:**
- Create: `crates/tagteam-engine/src/launch.rs`
- Modify: `crates/tagteam-engine/src/lib.rs` (`pub mod launch;` after `mod hooks;`, line 9)
- Modify: `crates/tagteam-engine/src/engine.rs`:
  - add `use std::time::Duration;`;
  - `guard_or_refuse` (lines 145–157) and the new `guard_or_refuse_within`;
  - `mutation_guard` (179–181) and `metadata_guard` (186–188);
  - `guard_recovering` (193–227);
  - `Engine.spawner`: Task 9's `dead_code` attribute and its comment deleted.
- Modify: `crates/tagteam-engine/src/bootstrap.rs` (Task 9's file: the `dead_code` attributes and
  their comments on `current_spelling`, `refusal` and the first `impl Engine` block deleted)
- Modify: `crates/tagteam-engine/src/switch.rs` (visibility only):
  - `needs_relogin` (lines 342–347), `cannot_refresh` (349–351) and `works_until_expiry`
    (353–358) become `pub(crate) fn`;
  - `Engine::due` (729–732) becomes `pub(crate) fn due`.
- Modify: `crates/tagteam-engine/src/error.rs` (`TargetChanged`, its `kind()` arm and pin)
- Modify: `crates/tagteam-engine/tests/common/mod.rs` (`claude_bin`, appended)
- Test: `crates/tagteam-engine/tests/launch.rs` (new), and `launch.rs`'s in-module
  `environment_warnings` tests

**Interfaces:**
- Consumes:
  - Task 4: `tagteam_provider::reservation::{LaunchReservation, remove_dead_reservations}`,
    `LaunchReservation::{create, path}`, and `create`'s `AlreadyExists` for a live `<pid>.lock`.
  - Tasks 5 and 7: `Provider::{has_baseline(dir), merge_back(env, dir, cancel), session_env}`
    (Decision 22), `tagteam_provider::provider::{MergeReport, SessionEnv}`.
  - Task 8: `RunPlan::Session`, `Engine::is_live_login`, `run::no_sessions`, and the fixtures
    `Fx::work_dir`.
  - Task 9: `Engine::{mark_profile, own_marker, seed_of, bootstrap_trigger, bootstrap_profile, vault_generation}`,
    `Trigger::as_str`, `EngineError::Login*`, the `dead_code` attributes this task deletes, and
    the fixtures `Fx.spawner`,
    `Fx::{script_valid, spelling_for, held_credential, held_refresh_token, rotate_profile, cc_writes_profile, land_replacement}`.
  - M4a (not on this branch; written against its plan's contract and code):
    - `Engine::{session_state, sync_profile_links, apply_provenance}`, `SessionState`,
      `SyncReport.warnings` and `ProfileCheck`;
    - `EngineError::{ProfileConflict, SessionOwned}`, `profile_path`, `launch_reservations`,
      `LAUNCH_DIR`, `ProfileMarker` and `Seed`;
    - `Provider::{session_dir_var, session_dir}`, `Env::var` and `Env.vars`;
    - `probe_lock` and `LockProbe`;
    - the fixtures `Fx::{profile_dir, profile_item, hold_reservation, engine_with_env}`.
  - M3a (written against its plan's contract): lock waits honour `env.cancel`, `Cancel::request`
    and `EngineError::signal`. The `Cancel::take` used in these tests is Task 3's.
  - Existing: `Engine::{settle_or_refuse, lock_account, refresh_stored, settle_rescues, store, provider}`,
    `GateOutcome`, `OwnedBy`, `hooks::point`, and the fixtures
    `Fx::{expire_access, script_refresh, put_vault, plant_rescue, displaced, vault_refresh_token, switch_request, switch_to}`,
    `two_accounts`, `credential`, `vault_fp`, `token_requests`, `mutation_lock_free` and
    `splice_config_key`; `Fx.dir`.
- Produces (Interface Contract):
  - `tagteam_engine::launch::{Launched, LaunchEnd}`, and
    `Engine::launch(&self, account: &AccountRow, program: &Path, cwd: &Path) -> Result<Launched, EngineError>`.
  - `EngineError::TargetChanged { why: String }`, kind `target-changed` (Decision 14), also for
    a removal that a pre-lock read of the account finds.
  - Crate-private: `Engine::guard_or_refuse_within(&self, provider: &ProviderId, timeout: Duration) -> Result<MutationGuard, EngineError>`,
    `Engine::recorded_spelling(&self, row: &AccountRow, profile: &Path) -> Result<String, EngineError>`,
    `launch::merge_summary(row: &AccountRow, report: &MergeReport) -> Option<String>`
    (Task 11), and
    `launch::environment_warnings(session: &SessionEnv, is_set: &dyn Fn(&OsStr) -> bool, home: Option<(&str, &Path)>) -> Vec<String>`.
    `switch.rs`'s `needs_relogin`, `cannot_refresh`, `works_until_expiry` and `due` become
    `pub(crate)`.
  - The hook points `launch-before-freshen` (after `settle_or_refuse`, before the gate
    refresh's vault read), `launch-before-locks` (after the gate refresh, before
    `MutationGuard`) and `launch-locked` (both locks held, the target re-read).
  - The fixture `pub fn claude_bin() -> &'static Path` (`tests/common/mod.rs`), which Tasks
    11 and 13 pass as the launch command.
  - For Task 12:
    - print `Launched.warnings` as `warning: …`;
    - on `TargetChanged { why }`, print `warning: {why}` and plan once more with `plan_run`;
      a second `TargetChanged` is the command's error (Decision 14).

**Spec:**
- §12.5 "Launch", steps 1–5: re-check the fast path; remove dead reservations; a quiescent
  profile's pending merge-back runs first and its failure aborts; sync links; a quiescent
  profile's provenance (capture; bootstrap when the vault moved on or it is stale-marked; a
  conflict refuses; unreadable or degraded aborts); a bootstrap for any other reason, then the
  seed; a running profile is joined without a seed; create the reservation; release the locks.
- §12.5 "Launch reservation": created, and a dead one removed, only under `MutationGuard` and
  the account lock.
- §12.3 step 1: the gate refresh before the locks; `Transient` with `rescued` and `Unpersisted`
  abort; a plain `Transient` continues. §7.2's table for the rest.
- §12.4: "Seed, on every launch into a quiescent profile"; a pending merge-back first.
- §12.2 "Profile marker" (the first launch creates it; a quiescent launch updates `outer`) and
  "Sync runs on every launch" (a join only creates missing links); a moved data directory is
  re-recorded by the next bootstrap.
- §6.2 "Pending rescues before activation"; §9.1 (30 s); §12.5 "Signals" (before the spawn).
- §12.5 "Environment": a warning naming each scrubbed variable that was set; a pre-set
  `CLAUDE_CONFIG_DIR` is overridden, with a warning.
- B.28, B.37, B.44, B.46, B.47; Decisions 7, 14, 18, 20 and 22.

- [ ] **Step 1: Write the failing tests**

Append to `crates/tagteam-engine/tests/common/mod.rs`:

```rust
/// The launch command a `Session` plan resolved (§12.1), as `launch`, a bootstrap's check and
/// the per-launch check are given it (Decision 20). The scripted spawner records it and never
/// runs it.
pub fn claude_bin() -> &'static Path {
    Path::new("/opt/claude/bin/claude")
}
```

Create `crates/tagteam-engine/tests/launch.rs`:

```rust
//! §12.5 "Launch": a session's start under `MutationGuard` and the account lock, what it
//! decides again under them, and its reservation. Task 11 appends the per-launch login check
//! and the exit handling.

mod common;

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{
    Fx, claude_bin, credential, mutation_lock_free, splice_config_key, token_requests,
    two_accounts, vault_fp,
};
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::launch::Launched;
use tagteam_engine::refresh::{GateOutcome, OwnedBy};
use tagteam_engine::store::AccountRow;
use tagteam_engine::vault::SERVICE;
use tagteam_provider::Read;
use tagteam_provider::flock::{FlockGuard, LockProbe, probe_lock};
use tagteam_provider::http::{HttpError, Method};
use tagteam_provider::profile::{LAUNCH_DIR, ProfileMarker, Seed, launch_reservations};
use tagteam_provider::splice::get_top_level;

/// The row `plan_run` hands `launch` (Task 8), as it stands.
fn row(fx: &Fx, id: &AccountId) -> AccountRow {
    fx.engine.store().unwrap().account(id).unwrap().unwrap()
}

fn refused(result: Result<Launched, EngineError>) -> EngineError {
    match result {
        Ok(launched) => panic!("launched position {}", launched.account.position),
        Err(e) => e,
    }
}

fn seed(profile: &Path) -> Seed {
    match Seed::read(profile) {
        Read::Present(s) => s,
        other => panic!("no seed in {}: {other:?}", profile.display()),
    }
}

/// How many reservations in `profile` are live (§12.5: held).
fn live_reservations(profile: &Path) -> usize {
    match launch_reservations(profile) {
        Read::Present(found) => found
            .iter()
            .filter(|(_, probe)| *probe == LockProbe::Held)
            .count(),
        other => panic!("{other:?}"),
    }
}

/// One top-level key of the JSON file at `path`, `Null` when absent.
fn config_key(path: &Path, key: &str) -> Value {
    get_top_level(&fs::read(path).unwrap(), key)
        .unwrap()
        .unwrap_or(Value::Null)
}

/// A session visiting a new project: one more entry in the profile's `projects` (§12.4).
fn session_adds_project(profile: &Path, project: &str) {
    let config = profile.join(".claude.json");
    let mut projects = config_key(&config, "projects");
    projects[project] = json!({"allowedTools": ["Bash"]});
    splice_config_key(&config, "projects", &projects);
}

/// The fixture's home made unwritable (0500), so nothing can be created directly in it: the
/// merge-back's `~/.claude.json.lock` and its temporary file. Everything below it, tagteam's
/// data dir included, stays writable. Needs a non-root user.
fn block_home(fx: &Fx) {
    fs::set_permissions(&fx.env.home, fs::Permissions::from_mode(0o500)).unwrap();
}

fn unblock_home(fx: &Fx) {
    fs::set_permissions(&fx.env.home, fs::Permissions::from_mode(0o700)).unwrap();
}

/// `id`'s first launch, which bootstraps (with a `valid` check queued) and seeds.
fn first_launch(fx: &Fx, id: &AccountId, email: &str) -> Launched {
    fx.script_valid(&fx.profile_dir(id), email);
    let launched = fx.engine.launch(&row(fx, id), claude_bin(), &fx.work_dir("app")).unwrap();
    assert!(launched.bootstrapped);
    launched
}

/// A session of `id` whose `tagteam` was killed while `claude` ran (Review Focus 1): no exit
/// handling, so its reservation is left dead and its baseline unmerged. Returns the profile.
fn killed_session(fx: &Fx, id: &AccountId, email: &str) -> PathBuf {
    let launched = first_launch(fx, id, email);
    let profile = launched.profile.clone();
    drop(launched);
    profile
}

#[test]
fn a_first_launch_bootstraps_validates_and_reserves_then_releases_its_locks() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = fx.profile_dir(&a);
    let cwd = fx.work_dir("app");
    let spelling = fx.spelling_for(&profile);
    fx.script_valid(&profile, "a@x.co");

    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();

    assert!(launched.bootstrapped);
    assert_eq!(launched.account.id, a);
    assert_eq!(launched.profile, profile);
    assert_eq!(launched.spelling, spelling);
    assert!(
        launched
            .env
            .set
            .contains(&(OsString::from("CLAUDE_CONFIG_DIR"), OsString::from(&spelling))),
        "{:?}",
        launched.env
    );
    assert!(launched.warnings.is_empty(), "{:?}", launched.warnings);
    assert_eq!(
        launched.reservation.path().parent(),
        Some(profile.join(LAUNCH_DIR).as_path())
    );
    assert_eq!(probe_lock(launched.reservation.path()).unwrap(), LockProbe::Held);
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a"));
    assert!(profile.join(".tagteam-baseline.json").exists(), "seeded (§12.4)");
    let specs = fx.spawner.specs();
    assert_eq!(specs.len(), 1, "validated once, under the locks");
    assert_eq!(
        specs[0].program,
        claude_bin(),
        "the launch command plan_run resolved (Decision 20)"
    );
    assert!(mutation_lock_free(&fx.env), "step 5: released");
    assert!(AccountLock::try_acquire(&fx.env, &a).unwrap().is_some());
}

#[test]
fn a_launch_into_a_profile_in_step_only_seeds_it() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");

    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")).unwrap();

    assert!(!launched.bootstrapped, "Task 11's per-launch check runs instead");
    assert_eq!(fx.spawner.specs().len(), 1, "no validation under the locks");
    assert!(profile.join(".tagteam-baseline.json").exists(), "seeded again");
    assert_eq!(live_reservations(&profile), 1);
}

#[test]
fn a_vault_credential_about_to_expire_is_refreshed_first_and_the_profile_starts_from_it() {
    // §12.3 step 1.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a-2"));

    let launched = first_launch(&fx, &a, "a@x.co");

    assert_eq!(token_requests(&fx), 1);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    assert_eq!(fx.held_refresh_token(&launched.profile).as_deref(), Some("rt-a-2"));
}

#[test]
fn a_refresh_whose_successor_reached_only_rescue_aborts_the_launch_before_any_lock() {
    // §12.3 step 1: the vault's generation is consumed.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a-2"));
    fx.kc.set_fail_write(SERVICE, true);

    let err = refused(fx.engine.launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")));

    fx.kc.set_fail_write(SERVICE, false);
    assert_eq!(err.kind(), "rescue-pending", "{err}");
    assert!(!fx.profile_dir(&a).exists(), "nothing was created");
}

#[test]
fn offline_the_launch_goes_on_with_the_stored_credential_and_a_warning() {
    // §12.3 step 1: a plain `Transient` continues with the stored credential.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.http.push(
        Method::Post,
        &Fx::endpoints().token,
        Err(HttpError::PreSend("dns lookup failed".into())),
    );

    let launched = first_launch(&fx, &a, "a@x.co");

    assert_eq!(
        launched.warnings,
        ["could not refresh a@x.co first (pre-send); Claude Code will refresh it when it is online"]
    );
    assert_eq!(fx.held_refresh_token(&launched.profile).as_deref(), Some("rt-a"));
}

#[test]
fn a_rotation_left_by_a_killed_session_is_captured_at_the_next_launch() {
    // Review Focus 1's end (§12.5 "Lazy capture"): quiescent again, so the next launch adopts it.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    fx.rotate_profile(&profile, "rt-a-2");

    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")).unwrap();

    assert!(!launched.bootstrapped, "captured, the profile is the vault's generation");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    assert_eq!(seed(&profile).seed_fp, vault_fp(&fx, &a));
}

#[test]
fn a_vault_that_moved_on_re_bootstraps_the_profile() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    fx.put_vault(&a, &credential("a@x.co", "rt-a-2"));
    fx.script_valid(&profile, "a@x.co");

    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")).unwrap();

    assert!(launched.bootstrapped);
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-2"));
    assert!(fx.displaced().is_empty(), "an older generation of this account is not saved");
}

#[test]
fn a_stale_marked_profile_is_displaced_and_re_bootstrapped() {
    // §12.5's table: the replacement wins.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    fx.rotate_profile(&profile, "rt-a-session");
    fx.land_replacement(&a, "rt-a-new");
    fx.script_valid(&profile, "a@x.co");

    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")).unwrap();

    assert!(launched.bootstrapped);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-new"), "never captured");
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-new"));
    let displaced: Value = serde_json::from_slice(&fx.displaced()[0]).unwrap();
    assert_eq!(displaced["claudeAiOauth"]["refreshToken"], "rt-a-session");
}

#[test]
fn a_conflict_refuses_the_launch_and_overwrites_nothing() {
    // §12.5: both moved in an unknown order.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    fx.rotate_profile(&profile, "rt-a-2");
    fx.put_vault(&a, &credential("a@x.co", "rt-a-3"));

    let err = refused(fx.engine.launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")));

    assert_eq!(err.kind(), "profile-conflict", "{err}");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-3"));
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-2"));
    assert_eq!(live_reservations(&profile), 0);
}

#[test]
fn an_unreadable_profile_credential_aborts_the_launch() {
    // §12.5 step 3: tagteam never overwrites a credential it could not read.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    fx.cc_writes_profile(&profile, |_| {});
    let (svc, acct) = fx.profile_item(&profile);
    fx.kc.set_unreadable(&svc, &acct, true);

    let err = refused(fx.engine.launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")));

    fx.kc.set_unreadable(&svc, &acct, false);
    assert_eq!(err.kind(), "unreadable", "{err}");
    assert_eq!(live_reservations(&profile), 0);
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a"), "untouched");
}

#[test]
fn a_pending_rescue_is_settled_before_the_profile_is_compared() {
    // §6.2: provenance against the consumed generation would call the profile in step.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
    fx.script_valid(&profile, "a@x.co");

    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")).unwrap();

    assert!(launched.bootstrapped, "the vault moved on to the rescued successor");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-2"));
}

#[test]
fn a_left_over_baseline_is_merged_back_before_the_seed() {
    // §12.4: a killed session's changes reach `~/.claude.json` at the next launch.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    session_adds_project(&profile, "/work/new");

    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")).unwrap();

    assert!(!launched.bootstrapped);
    let merged = config_key(&fx.paths().global_config, "projects");
    assert_eq!(merged["/work/new"], json!({"allowedTools": ["Bash"]}));
    assert_eq!(
        config_key(&profile.join(".claude.json"), "projects"),
        merged,
        "then seeded from the merged default"
    );
}

#[test]
fn a_pending_merge_back_that_fails_aborts_with_the_profile_and_its_baseline_untouched() {
    // §12.4, B.44: seeding over them would discard the session's unmerged changes.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let cwd = fx.work_dir("app");
    let profile = killed_session(&fx, &a, "a@x.co");
    session_adds_project(&profile, "/work/new");
    let config = fs::read(profile.join(".claude.json")).unwrap();
    let baseline = fs::read(profile.join(".tagteam-baseline.json")).unwrap();

    block_home(&fx);
    let result = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd);
    unblock_home(&fx);

    let err = refused(result);
    assert_eq!(fs::read(profile.join(".claude.json")).unwrap(), config, "{err}");
    assert_eq!(fs::read(profile.join(".tagteam-baseline.json")).unwrap(), baseline);
    assert_eq!(live_reservations(&profile), 0);
    assert_eq!(
        config_key(&fx.paths().global_config, "projects").get("/work/new"),
        None
    );

    // Once the default home can be written, the next launch merges it first.
    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    assert!(!launched.bootstrapped);
    assert!(
        config_key(&fx.paths().global_config, "projects")
            .get("/work/new")
            .is_some()
    );
}

#[test]
fn a_moved_profile_s_unmerged_changes_are_merged_back_before_its_bootstrap_seeds_it() {
    // Decision 22, §12.4, B.44: the data directory moved, so the marker's spelling names a path
    // that is gone. The killed session's baseline is where the profile is now, and the
    // merge-back must find it there before the spelling-change bootstrap seeds over it.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    session_adds_project(&profile, "/work/new");
    let current = fx.spelling_for(&profile);
    let old = fx.dir.path().join("moved-from/sessions").join(a.as_str());
    let marker = match ProfileMarker::read(&profile) {
        Read::Present(m) => m,
        other => panic!("{other:?}"),
    };
    ProfileMarker {
        config_dir: old.to_str().unwrap().to_owned(),
        ..marker
    }
    .write(&profile)
    .unwrap();
    fx.script_valid(&profile, "a@x.co");

    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")).unwrap();

    assert!(launched.bootstrapped, "§12.3: the spelling changed");
    assert_eq!(launched.spelling, current, "and the new one is recorded");
    let merged = config_key(&fx.paths().global_config, "projects");
    assert_eq!(
        merged["/work/new"],
        json!({"allowedTools": ["Bash"]}),
        "merged back first, from the baseline where the profile is"
    );
    assert_eq!(
        config_key(&profile.join(".claude.json"), "projects"),
        merged,
        "then bootstrapped and seeded from the merged default"
    );
    assert_eq!(
        config_key(&profile.join(".tagteam-baseline.json"), "projects"),
        merged,
        "the seed's new baseline"
    );
}

#[test]
fn a_launch_into_a_running_profile_joins_it_as_it_is() {
    // §12.5 step 3, B.28: no seed and no bootstrap under a running session, even when the vault
    // has moved on.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    let other = fx.hold_reservation(&profile);
    session_adds_project(&profile, "/work/new");
    let config = fs::read(profile.join(".claude.json")).unwrap();
    let seeded = seed(&profile);
    fx.put_vault(&a, &credential("a@x.co", "rt-a-2"));

    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")).unwrap();

    assert!(!launched.bootstrapped);
    assert_eq!(fs::read(profile.join(".claude.json")).unwrap(), config, "not seeded");
    assert_eq!(seed(&profile), seeded);
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a"));
    assert_eq!(fx.spawner.specs().len(), 1);
    assert_eq!(live_reservations(&profile), 2);
    drop(other);
}

#[test]
fn dead_reservations_are_removed_and_the_profile_counts_as_quiescent() {
    // §12.5 step 2.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    let dead = {
        let gone = fx.hold_reservation(&profile);
        gone.path().to_path_buf()
    };
    assert!(dead.exists());

    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")).unwrap();

    assert!(!dead.exists());
    assert_eq!(live_reservations(&profile), 1);
    assert_eq!(launched.reservation.path().parent(), dead.parent());
}

#[test]
fn while_a_launch_holds_its_reservation_the_account_is_session_owned() {
    // §12.5: `remove`, `switch` and the gate all see the reservation once the locks are released.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let launched = first_launch(&fx, &a, "a@x.co");

    assert_eq!(fx.engine.remove(&a).unwrap_err().kind(), "session-owned");
    assert_eq!(
        fx.switch_to(&a, false).err().expect("refused").kind(),
        "session-owned"
    );
    let vault = fx.vault_bytes(&a).unwrap();
    assert!(matches!(
        fx.engine.refresh_stored(fx.cc.as_ref(), &a, &vault).unwrap(),
        GateOutcome::Owned(OwnedBy::Session)
    ));
    drop(launched);
}

#[test]
fn a_signal_before_the_locks_launches_nothing() {
    // §12.5 "Signals": every lock wait before the spawn is a cancellation point.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let cwd = fx.work_dir("app");
    fx.env.cancel.request(libc::SIGINT);

    let err = refused(fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd));

    let _ = fx.env.cancel.take();
    assert_eq!(err.kind(), "interrupted", "{err}");
    assert_eq!(err.signal(), Some(libc::SIGINT));
    assert!(!fx.profile_dir(&a).exists());
}

#[test]
fn a_live_reservation_already_under_this_pid_refuses_the_launch_and_is_left_alone() {
    // Task 4: an orphaned `claude` of an earlier process with this pid still holds
    // `<pid>.lock`. Replacing it would hide a running session.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    let mine = profile
        .join(LAUNCH_DIR)
        .join(format!("{}.lock", std::process::id()));
    let orphan = FlockGuard::try_lock(&mine).unwrap().unwrap();
    let before = fs::read(&mine).unwrap();

    let err = refused(fx.engine.launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")));

    assert_eq!(err.kind(), "launch-unreachable", "{err}");
    assert!(err.to_string().contains(&mine.display().to_string()), "{err}");
    assert_eq!(fs::read(&mine).unwrap(), before, "never replaced");
    assert!(mutation_lock_free(&fx.env));
    drop(orphan);
}

#[test]
fn each_scrubbed_variable_this_process_has_set_is_named_once_in_the_launch_s_warnings() {
    // §12.5 "Environment", Decision 18: the CLI records which are set in `Env.vars`.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let mut env = fx.env.clone();
    for name in ["ANTHROPIC_API_KEY", "USE_STAGING_OAUTH"] {
        env.vars.insert(name.into(), OsString::new());
    }
    fx.script_valid(&fx.profile_dir(&a), "a@x.co");

    let launched = fx
        .engine_with_env(env)
        .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"))
        .unwrap();

    let w = &launched.warnings;
    assert_eq!(w.len(), 2, "{w:?}");
    assert!(w[0].starts_with("ANTHROPIC_API_KEY is set here"), "{w:?}");
    assert!(w[1].starts_with("USE_STAGING_OAUTH is set here"), "{w:?}");
    assert!(
        launched.env.remove.contains(&OsString::from("ANTHROPIC_API_KEY")),
        "and scrubbed: {:?}",
        launched.env
    );
}

#[cfg(feature = "test-hooks")]
mod hooked {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn a_target_removed_before_the_locks_is_not_launched() {
        // Decision 7, B.47: decided again under the locks.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let planned = row(&fx, &a);
        let cwd = fx.work_dir("app");
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let id = a.clone();
        fx.engine.on_point(
            "launch-before-locks",
            Box::new(move || {
                other.remove(&id).unwrap();
            }),
        );

        let err = refused(fx.engine.launch(&planned, claude_bin(), &cwd));

        assert_eq!(err.kind(), "target-changed", "{err}");
        assert!(err.to_string().contains("was removed"), "{err}");
        assert!(!fx.profile_dir(&a).exists(), "nothing was created");
    }

    #[test]
    fn a_target_removed_before_the_launch_reads_its_vault_is_not_launched() {
        // Decision 14, B.47: a `remove` that completes after `plan_run`, before the launch's
        // first read of the account, is the same race the re-check under the locks answers,
        // not a missing credential (`invalid-input`).
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let planned = row(&fx, &a);
        let cwd = fx.work_dir("app");
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let id = a.clone();
        fx.engine.on_point(
            "launch-before-freshen",
            Box::new(move || {
                other.remove(&id).unwrap();
            }),
        );

        let err = refused(fx.engine.launch(&planned, claude_bin(), &cwd));

        assert_eq!(err.kind(), "target-changed", "{err}");
        assert!(err.to_string().contains("was removed"), "{err}");
        assert!(!fx.profile_dir(&a).exists(), "nothing was created");
        assert_eq!(token_requests(&fx), 0, "nothing was refreshed");
    }

    #[test]
    fn a_target_that_became_the_live_login_before_the_locks_is_left_to_plain_claude() {
        // §12.5 step 1: otherwise one token would get a default copy and a profile copy.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let planned = row(&fx, &a);
        let cwd = fx.work_dir("app");
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let req = fx.switch_request(&a, false);
        fx.engine.on_point(
            "launch-before-locks",
            Box::new(move || {
                other.switch(req.clone()).unwrap();
            }),
        );

        let err = refused(fx.engine.launch(&planned, claude_bin(), &cwd));

        assert_eq!(err.kind(), "target-changed", "{err}");
        assert!(err.to_string().contains("became the live login"), "{err}");
        assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
        assert!(!fx.profile_dir(&a).exists());
    }

    #[test]
    fn a_target_whose_login_became_an_api_key_is_refused_under_the_locks() {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let planned = row(&fx, &a);
        let cwd = fx.work_dir("app");
        let (db, id) = (fx.env.data_dir().join("tagteam.db"), a.to_string());
        fx.engine.on_point(
            "launch-before-locks",
            Box::new(move || {
                rusqlite::Connection::open(&db)
                    .unwrap()
                    .execute("UPDATE accounts SET kind = 'api_key' WHERE id = ?1", [&id])
                    .unwrap();
            }),
        );

        let err = refused(fx.engine.launch(&planned, claude_bin(), &cwd));

        assert_eq!(err.kind(), "api-key-account", "{err}");
    }

    #[test]
    fn the_gate_finds_the_account_busy_while_the_launch_holds_its_lock() {
        // §7.3 step 1: a reservation can only appear while the gate cannot hold the lock.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let planned = row(&fx, &a);
        let cwd = fx.work_dir("app");
        fx.script_valid(&fx.profile_dir(&a), "a@x.co");
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let seen = Arc::new(Mutex::new(None));
        let (cc, id, vault, record) = (
            fx.cc.clone(),
            a.clone(),
            fx.vault_bytes(&a).unwrap(),
            seen.clone(),
        );
        fx.engine.on_point(
            "launch-locked",
            Box::new(move || {
                let outcome = other.refresh_stored(cc.as_ref(), &id, &vault);
                *record.lock().unwrap() = Some(matches!(outcome, Ok(GateOutcome::Busy)));
            }),
        );

        let launched = fx.engine.launch(&planned, claude_bin(), &cwd).unwrap();

        assert_eq!(*seen.lock().unwrap(), Some(true));
        drop(launched);
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test launch`
Expected: compile errors: `unresolved import tagteam_engine::launch` and
`no method named launch found for struct Engine`. (`launch.rs`'s two in-module
`environment_warnings` tests arrive with the module in Step 3; the integration test
`each_scrubbed_variable_this_process_has_set_is_named_once_in_the_launch_s_warnings` pins the
same behaviour first.)

- [ ] **Step 3: Implement**

**`crates/tagteam-engine/src/bootstrap.rs` and `engine.rs`.** `launch` below is the bootstrap's
production caller, so delete Task 9's four
`#[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]` attributes, each with the two
comment lines above it: on `current_spelling`, `refusal` and the first `impl Engine` block of
`bootstrap.rs`, and on `Engine.spawner` in `engine.rs`. Nothing else in either changes.

**`crates/tagteam-engine/src/switch.rs`.** Make four items crate-visible, with no other change:
- `fn needs_relogin(target: &AccountRow) -> EngineError` becomes `pub(crate) fn needs_relogin`.
- `fn cannot_refresh(app: &str, label: &str, why: &str) -> String` becomes
  `pub(crate) fn cannot_refresh`.
- `fn works_until_expiry(target: &AccountRow) -> String` becomes
  `pub(crate) fn works_until_expiry`.
- In `impl Engine`, `fn due(&self, p: &dyn Provider, vault: &[u8]) -> bool` becomes
  `pub(crate) fn due`.

**`crates/tagteam-engine/src/engine.rs`.** Add `use std::time::Duration;`. Replace
`guard_or_refuse` (lines 145–157) with:

```rust
    /// The mutation lock for account-changing work, refused while an interrupted switch for
    /// the provider is still unresolved once recovery has run under it (§9.6). The refusal
    /// says why: a recovery that could not take the provider's live locks names the lock and
    /// asks for a retry; only a row recovery could not decide points at `--force`.
    pub(crate) fn guard_or_refuse(
        &self,
        provider: &ProviderId,
    ) -> Result<MutationGuard, EngineError> {
        self.guard_or_refuse_within(provider, MutationGuard::TIMEOUT)
    }

    /// `guard_or_refuse`, waiting up to `timeout` for the lock. `run`'s launch waits 30 s
    /// (§9.1), because another launch may hold it through a bootstrap's validation.
    pub(crate) fn guard_or_refuse_within(
        &self,
        provider: &ProviderId,
        timeout: Duration,
    ) -> Result<MutationGuard, EngineError> {
        let (guard, blocked) = self.guard_recovering(true, timeout)?;
        if self.interrupted(provider)? {
            return Err(blocked
                .into_iter()
                .find_map(|(p, e)| (&p == provider).then_some(e))
                .unwrap_or_else(|| EngineError::InterruptedSwitch(provider.to_string())));
        }
        Ok(guard)
    }
```

`mutation_guard`'s body becomes `Ok(self.guard_recovering(true, MutationGuard::TIMEOUT)?.0)`,
and `metadata_guard`'s becomes `Ok(self.guard_recovering(false, MutationGuard::TIMEOUT)?.0)`.
`guard_recovering` gains the parameter. Today's function becomes the one below. On M3a's
version (Task 2 changes how it reports a recovery's interrupted lock wait), apply only the
same two edits: the `timeout` parameter, and its use in `MutationGuard::acquire`.

```rust
    /// `mutation_guard`, with the refusal for each row whose recovery could not take its
    /// provider's live locks (`RecoveryBlocked`), by provider. With `ask_oracle` false the
    /// rows are recovered from fingerprints alone: no network call (§7.6, §9.6). The lock is
    /// waited for up to `timeout`.
    fn guard_recovering(
        &self,
        ask_oracle: bool,
        timeout: Duration,
    ) -> Result<(MutationGuard, Vec<(ProviderId, EngineError)>), EngineError> {
        let hints: Vec<_> = self
            .dead_journals()?
            .into_iter()
            .map(|row| {
                let hint = if ask_oracle {
                    self.recovery_hints(&row)
                } else {
                    Vec::new()
                };
                (row, hint)
            })
            .collect();
        hooks::point(self, "before-mutation-lock")?;
        let guard = MutationGuard::acquire(&self.env, timeout)?;
        // Enumerated again under the lock: a switch may have died while this command waited,
        // and its row is recovered now too, without a hint.
        let mut blocked = Vec::new();
        for row in self.dead_journals()? {
            let hint = hints
                .iter()
                .find(|(r, _)| *r == row)
                .map_or(&[][..], |(_, h)| h.as_slice());
            if let Err(e) = self.recover_one(&guard, &row, hint) {
                tracing::warn!(provider = %row.provider, "could not recover an interrupted switch: {e}");
                if matches!(e, EngineError::RecoveryBlocked { .. }) {
                    blocked.push((row.provider.clone(), e));
                }
            }
        }
        Ok((guard, blocked))
    }
```

**`crates/tagteam-engine/src/error.rs`.** After Task 9's `LaunchUnreachable`:

```rust
    /// §12.5 launch step 1, B.47: under the launch's locks the target is no longer one a session
    /// may start for. It was removed (Decision 7), or it became the live default login. The
    /// CLI prints `why` and plans again, which runs plain `claude` or refuses under
    /// `--require-session`.
    #[error("{why}")]
    TargetChanged { why: String },
```

Its `kind()` arm, after Task 9's: `EngineError::TargetChanged { .. } => "target-changed",`. Its
pin, after Task 9's:

```rust
            (
                EngineError::TargetChanged { why: "w".into() },
                "target-changed",
            ),
```

**`crates/tagteam-engine/src/lib.rs`**: add `pub mod launch;` after `mod hooks;`.

**Create `crates/tagteam-engine/src/launch.rs`:**

```rust
//! §12.5 "Launch": a session's start under `MutationGuard` and the account lock (Task 10).
//! Task 11 adds the per-launch login check and the exit handling.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use tagteam_provider::profile::profile_path;
use tagteam_provider::provider::{MergeReport, SessionEnv};
use tagteam_provider::reservation::{LaunchReservation, remove_dead_reservations};
use tagteam_provider::{MutationGuard, Provider, ReadError};

use crate::account_lock::AccountLock;
use crate::bootstrap::Trigger;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::provenance::ProfileCheck;
use crate::refresh::{GateOutcome, OwnedBy};
use crate::run::no_sessions;
use crate::session::SessionState;
use crate::store::AccountRow;
use crate::switch::{cannot_refresh, needs_relogin, works_until_expiry};

pub struct Launched {
    pub account: AccountRow,
    pub profile: PathBuf,
    pub spelling: String,
    pub reservation: LaunchReservation,
    pub env: SessionEnv,
    /// This launch bootstrapped, so its login check already ran (§12.3).
    pub bootstrapped: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchEnd {
    /// `claude` ran and exited with this status code.
    Exited(i32),
    /// The launch was refused after its reservation existed (exit 1).
    Refused,
}

/// §12.4 step 3: one summary line when keys changed on both sides since the baseline, the
/// default's values kept. Each key is named in the log only.
pub(crate) fn merge_summary(row: &AccountRow, report: &MergeReport) -> Option<String> {
    for key in &report.conflicts {
        tracing::info!(
            position = row.position,
            account = %row.id,
            key = %key,
            "kept the default home's value of a key the session changed too"
        );
    }
    tracing::debug!(
        position = row.position,
        account = %row.id,
        applied = report.applied,
        "merged the session's config back"
    );
    let n = report.conflicts.len();
    (n > 0).then(|| {
        format!(
            "{n} {} of position {}'s session config changed on both sides while it ran; the default home's values were kept",
            if n == 1 { "key" } else { "keys" },
            row.position
        )
    })
}

impl Engine {
    /// §12.5 steps 1–5 (the gate refresh of step 1 included).
    ///
    /// `account` is `plan_run`'s `Session` target. Before any lock, an interrupted switch is
    /// settled (§9.6), and a vault credential about to expire is refreshed through the gate
    /// (§12.3 step 1); a target that was removed by then is `TargetChanged`. Then, under
    /// `MutationGuard` (30 s) and the account lock:
    /// - the decision is made again (B.47): a target that was removed, or became the live
    ///   login, is `TargetChanged`, for the CLI to plan again;
    /// - dead reservations go, and a quiescent profile's pending merge-back runs, from the
    ///   baseline in its actual directory (Decision 22; a failure aborts);
    /// - the marker and the links are brought up to date;
    /// - a quiescent profile gets its provenance, then a bootstrap or the seed, and a running
    ///   one is joined as it is;
    /// - the reservation is created last; a live one already under this pid refuses
    ///   (`launch-unreachable`).
    ///
    /// The locks are released when this returns. `program` is the launch command `plan_run`
    /// resolved, which a bootstrap's validation spawns (Decision 20). `Launched.warnings` holds
    /// every warning, §12.5's environment warnings last (Decision 18).
    pub fn launch(
        &self,
        account: &AccountRow,
        program: &Path,
        cwd: &Path,
    ) -> Result<Launched, EngineError> {
        let p = self.provider(&account.provider)?;
        let p = p.as_ref();
        if !p.capabilities().sessions {
            return Err(no_sessions(p));
        }
        self.settle_or_refuse(&account.provider)?;
        hooks::point(self, "launch-before-freshen")?;
        let mut warnings = self.freshen_for_launch(p, account)?;
        hooks::point(self, "launch-before-locks")?;
        let guard =
            self.guard_or_refuse_within(&account.provider, MutationGuard::BOOTSTRAP_TIMEOUT)?;
        let lock = self.lock_account(&account.id)?;
        // 1.
        let row = self.relocked_target(p, account)?;
        hooks::point(self, "launch-locked")?;
        let profile = profile_path(&self.env, &row.id);
        // 2.
        if profile.is_dir() {
            for dead in remove_dead_reservations(&profile)? {
                tracing::debug!(
                    position = row.position,
                    account = %row.id,
                    reservation = %dead.display(),
                    "removed a dead launch reservation"
                );
            }
        }
        let state = self.session_state(p, &row)?;
        let joining = state.owned();
        if let SessionState::Unreadable { detail, .. } = &state {
            warnings.push(format!(
                "a session of position {} may be running ({detail}), so this launch joins it as it is",
                row.position
            ));
        }
        if !joining {
            // §12.4: a killed session's baseline is in the profile's actual directory, which
            // the marker's spelling no longer names once the data directory has moved
            // (Decision 22). A marker naming another account refuses first.
            if self.own_marker(&row, &profile)?.is_some() && p.has_baseline(&profile) {
                let report = p.merge_back(&self.env, &profile, self.cancel())?;
                warnings.extend(merge_summary(&row, &report));
            }
            self.mark_profile(p, &row, &profile)?;
        }
        // 3.
        warnings.extend(self.sync_profile_links(p, &profile, joining)?.warnings);
        let bootstrapped = !joining
            && self.prepare_quiescent(p, &row, &profile, cwd, program, &guard, &lock)?;
        // 4.
        let spelling = self.recorded_spelling(&row, &profile)?;
        let reservation = LaunchReservation::create(&profile).map_err(reservation_refused)?;
        // 5.
        drop(lock);
        drop(guard);
        let env = p.session_env(&spelling);
        // §12.5 "Environment", Decision 18: the scrubbed variables this process has set are in
        // `Env.vars`, which the CLI records at its boundary; the home variable is the outer
        // home's, inside a run shell too.
        let home = p.session_dir_var().zip(p.session_dir(&self.env));
        warnings.extend(environment_warnings(
            &env,
            &|name| name.to_str().is_some_and(|n| self.env.var(n).is_some()),
            home.as_ref().map(|(var, dir)| (*var, dir.as_path())),
        ));
        Ok(Launched {
            env,
            account: row,
            profile,
            spelling,
            reservation,
            bootstrapped,
            warnings,
        })
    }

    /// The spelling every operation on the profile's credential uses (§12.2): the marker's.
    pub(crate) fn recorded_spelling(
        &self,
        row: &AccountRow,
        profile: &Path,
    ) -> Result<String, EngineError> {
        self.own_marker(row, profile)?
            .map(|m| m.config_dir)
            .ok_or_else(|| {
                EngineError::InvalidInput(format!("{} has no profile marker", profile.display()))
            })
    }

    /// §12.3 step 1, before any lock. A vault credential about to expire is refreshed through
    /// the gate (§7.3), as a switch freshens its target (§7.2), so a bootstrap starts the
    /// profile from a fresh generation. The outcomes map as §12.3 step 1 says, and otherwise
    /// as §7.2's direct-target column. Returns the warnings to show. A vault read that fails
    /// because the account was removed since `plan_run` is `TargetChanged` (B.47).
    fn freshen_for_launch(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
    ) -> Result<Vec<String>, EngineError> {
        if !p.kind_traits(&row.kind).refreshable {
            return Ok(Vec::new());
        }
        let vault = self
            .vault_generation(row)
            .map_err(|failure| self.removed_or(row, failure))?;
        let due = self.due(p, &vault);
        if row.quarantine_reason.is_some() {
            // §7.4: never refreshed; usable only while its access token lasts.
            return if due {
                Err(needs_relogin(row))
            } else {
                Ok(vec![works_until_expiry(row)])
            };
        }
        if !due {
            return Ok(Vec::new());
        }
        let app = p.display_name();
        let pending = |detail: &str| EngineError::RescuePending {
            position: row.position,
            label: row.label.clone(),
            detail: detail.to_owned(),
        };
        Ok(match self.refresh_stored(p, &row.id, &vault)? {
            // Busy: the account lock this launch waits for, and the rescue settlement under it,
            // pick up the other process's refresh.
            GateOutcome::Refreshed(_) | GateOutcome::AlreadyFresh(_) | GateOutcome::Busy => {
                Vec::new()
            }
            // The live login: the re-check under the locks decides. A session: this launch
            // joins it, and its `claude` refreshes the token.
            GateOutcome::Owned(OwnedBy::Live | OwnedBy::Session) => Vec::new(),
            // The mutation lock decides, as it does for a switch (§7.2).
            GateOutcome::Owned(OwnedBy::Journal) => vec![cannot_refresh(
                app,
                &row.label,
                "an unfinished switch names it",
            )],
            // The vault's generation is dead, or spent with its successor lost (§7.3 step 6).
            GateOutcome::Dead(_) | GateOutcome::Unpersisted => return Err(needs_relogin(row)),
            GateOutcome::Transient { rescued: true, .. } => {
                return Err(pending(
                    "the refresh succeeded, but the vault could not be written; the new token is in rescue/",
                ));
            }
            GateOutcome::Transient { kind, .. } if kind == "rescue-unreadable" => {
                return Err(pending("a pending rescue could not be adopted"));
            }
            // Nothing was spent: the stored credential goes on (§12.3 step 1).
            GateOutcome::Transient { kind, .. } => vec![cannot_refresh(app, &row.label, &kind)],
            GateOutcome::Systemic(detail) => vec![cannot_refresh(app, &row.label, &detail)],
            GateOutcome::Conflict => {
                return Err(EngineError::ProfileConflict {
                    position: row.position,
                    label: row.label.clone(),
                });
            }
        })
    }

    /// `failure`, from a read of `planned` before the locks, unless the account is gone. A
    /// `remove` that completed after `plan_run` is then `TargetChanged`, as the re-check under
    /// the locks would say (Decision 14, B.47). Only a present account propagates `failure`; a
    /// re-read that fails itself leaves `failure` as it is.
    fn removed_or(&self, planned: &AccountRow, failure: EngineError) -> EngineError {
        match self
            .store()
            .and_then(|s| s.account(&planned.id).map_err(EngineError::from))
        {
            Ok(None) => target_removed(planned),
            Ok(Some(_)) | Err(_) => failure,
        }
    }

    /// §12.5 launch step 1 and B.47: the target as it stands under the locks. If it was removed
    /// (Decision 7), or became the live default login, this returns `TargetChanged`, for the
    /// CLI to plan again. A login replaced by an API key since `plan_run` is refused, as
    /// `plan_run` would refuse it.
    fn relocked_target(
        &self,
        p: &dyn Provider,
        planned: &AccountRow,
    ) -> Result<AccountRow, EngineError> {
        let Some(row) = self.store()?.account(&planned.id)? else {
            return Err(target_removed(planned));
        };
        if p.kind_traits(&row.kind).managed_key_axis {
            return Err(EngineError::ApiKeyAccount {
                position: row.position,
            });
        }
        if self.is_live_login(p, &row)? {
            return Err(EngineError::TargetChanged {
                why: format!(
                    "position {} became the live login while `tagteam run` started it",
                    row.position
                ),
            });
        }
        Ok(row)
    }

    /// §12.5 step 3 for a quiescent profile, after its sync. Pending rescues go first (§6.2),
    /// then the profile's provenance (§12.5):
    /// - a rotation is captured;
    /// - a conflict refuses;
    /// - a profile that cannot be read aborts, since tagteam never overwrites what it could not
    ///   read;
    /// - a vault that moved on, or a stale-marked profile, bootstraps.
    ///
    /// Any other §12.3 trigger bootstraps too; otherwise the profile is seeded (§12.4).
    /// Returns whether it bootstrapped. A bootstrap validates in `cwd`, spawning `program`.
    #[allow(clippy::too_many_arguments)]
    fn prepare_quiescent(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        profile: &Path,
        cwd: &Path,
        program: &Path,
        guard: &MutationGuard,
        lock: &AccountLock,
    ) -> Result<bool, EngineError> {
        self.settle_rescues(p, row, lock)?;
        let why = match self.apply_provenance(p, row, lock)? {
            ProfileCheck::Conflict => {
                return Err(EngineError::ProfileConflict {
                    position: row.position,
                    label: row.label.clone(),
                });
            }
            ProfileCheck::Unreadable(detail) => {
                return Err(EngineError::Unreadable(ReadError::new(
                    profile.display().to_string(),
                    detail,
                )));
            }
            ProfileCheck::VaultMovedOn => Some("vault-moved-on"),
            ProfileCheck::ReplacementWins => Some("replacement-wins"),
            ProfileCheck::InStep | ProfileCheck::Captured | ProfileCheck::NotApplicable => self
                .bootstrap_trigger(p, row, profile)?
                .map(Trigger::as_str),
        };
        let Some(why) = why else {
            self.seed_of(p, row, profile)?;
            return Ok(false);
        };
        tracing::info!(
            position = row.position,
            account = %row.id,
            why,
            "bootstrapping the session profile"
        );
        self.bootstrap_profile(p, row, profile, cwd, program, guard, lock)?;
        Ok(true)
    }
}

/// §12.5 "Environment" (Decision 18): one warning per scrubbed variable this process has set,
/// which the session would otherwise inherit, and one for the provider's home variable when the
/// outer home names another directory than the profile. `is_set` answers for this process's
/// environment; `home` is the provider's variable and the directory the outer home gives it.
pub(crate) fn environment_warnings(
    session: &SessionEnv,
    is_set: &dyn Fn(&OsStr) -> bool,
    home: Option<(&str, &Path)>,
) -> Vec<String> {
    let mut warnings: Vec<String> = session
        .remove
        .iter()
        .filter(|name| is_set(name.as_os_str()))
        .map(|name| {
            format!(
                "{} is set here; the session runs without it, so it cannot change which login the session uses",
                name.to_string_lossy()
            )
        })
        .collect();
    if let Some((var, dir)) = home {
        let overridden = session
            .set
            .iter()
            .any(|(name, value)| name.as_os_str() == OsStr::new(var) && Path::new(value) != dir);
        if overridden {
            warnings.push(format!(
                "{var} is set to {}; the session runs in its own profile instead",
                dir.display()
            ));
        }
    }
    warnings
}

/// Decision 14: `planned` was removed while `tagteam run` started it, and its mapping with it
/// (Decision 7). Before the locks and under them alike.
fn target_removed(planned: &AccountRow) -> EngineError {
    EngineError::TargetChanged {
        why: format!(
            "position {} was removed while `tagteam run` started it",
            planned.position
        ),
    }
}

/// Interface Contract: `create` refuses a `<pid>.lock` that is live, the reservation of an
/// orphaned `claude` of an earlier process with this pid (Task 4). Replacing it would hide a
/// running session, so the launch is refused, naming the file; a later `run` gets a new pid.
fn reservation_refused(e: io::Error) -> EngineError {
    if e.kind() == io::ErrorKind::AlreadyExists {
        EngineError::LaunchUnreachable {
            detail: format!("{e}; run it again"),
        }
    } else {
        e.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_env() -> SessionEnv {
        SessionEnv {
            set: vec![("CLAUDE_CONFIG_DIR".into(), "/data/sessions/a".into())],
            remove: vec![
                "ANTHROPIC_API_KEY".into(),
                "CLAUDE_CODE_OAUTH_TOKEN".into(),
                "CLAUDE_SECURESTORAGE_CONFIG_DIR".into(),
            ],
        }
    }

    #[test]
    fn each_scrubbed_variable_that_is_set_is_named_once() {
        let set = |n: &OsStr| {
            n == OsStr::new("ANTHROPIC_API_KEY") || n == OsStr::new("CLAUDE_SECURESTORAGE_CONFIG_DIR")
        };
        let w = environment_warnings(&session_env(), &set, None);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[0].starts_with("ANTHROPIC_API_KEY "), "{w:?}");
        assert!(w[1].starts_with("CLAUDE_SECURESTORAGE_CONFIG_DIR "), "{w:?}");
        assert!(environment_warnings(&session_env(), &|_| false, None).is_empty());
    }

    #[test]
    fn a_home_variable_the_session_overrides_is_named_unless_it_names_the_profile() {
        let none = |_: &OsStr| false;
        let elsewhere = Path::new("/Users/me/.claude-work");
        let w = environment_warnings(&session_env(), &none, Some(("CLAUDE_CONFIG_DIR", elsewhere)));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].starts_with("CLAUDE_CONFIG_DIR ") && w[0].contains("/Users/me/.claude-work"),
            "{w:?}"
        );
        let same = Path::new("/data/sessions/a");
        assert!(environment_warnings(&session_env(), &none, Some(("CLAUDE_CONFIG_DIR", same))).is_empty());
        assert!(
            environment_warnings(&session_env(), &none, Some(("FAKEAGENT_HOME", elsewhere))).is_empty(),
            "a variable the session does not set is not overridden"
        );
    }
}
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-engine --features test-hooks --test launch`
Expected: PASS, 25 tests (5 in `hooked`).

Run: `cargo test -p tagteam-engine --test launch`
Expected: PASS, 20 tests.

Run: `cargo test -p tagteam-engine --lib launch::tests`
Expected: PASS, 2 tests (`environment_warnings`).

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS. `guard_or_refuse`, `mutation_guard` and `metadata_guard` keep their 10 s
wait, so no switch, recovery or lifecycle test moves.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: both clean, without Task 9's `dead_code` attributes: `launch` reaches the bootstrap
in every build.
`grep -n 'cfg_attr(not(feature = "test-hooks")' crates/tagteam-engine/src/bootstrap.rs crates/tagteam-engine/src/engine.rs`
prints nothing.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/launch.rs crates/tagteam-engine/src/lib.rs \
  crates/tagteam-engine/src/engine.rs crates/tagteam-engine/src/bootstrap.rs \
  crates/tagteam-engine/src/switch.rs crates/tagteam-engine/src/error.rs \
  crates/tagteam-engine/tests/common/mod.rs crates/tagteam-engine/tests/launch.rs
git commit -m "Launch a session under the mutation and account locks"
```

---

### Task 11: The per-launch login check and exit handling

A launch that did not bootstrap runs the login check after its locks are released, with its
reservation held, in the directory `claude` will run in (§12.3 "Every launch is checked"). That
is `Engine::check_login`. When `claude` exits, or the launch is refused after its reservation
exists, `Engine::finish_run` does §12.5's exit handling under `MutationGuard` and the account
lock:
- **The last session out** (the profile is quiescent apart from this process's own
  reservation) captures a rotation through the profile's provenance, then merges the profile's
  `.claude.json` back.
- **Otherwise** another session still runs (another `run`, or a `bg` or `daemon` record), and
  only the unlink happens.
- **In both cases** the reservation is unlinked last.

Exit handling never changes the exit code (B.63). It returns the notices the CLI prints.

**Readings of the spec this task commits to:**
- **"Quiescent apart from this process's own reservation"** needs session state and provenance
  that can leave one reservation out. M4a's `session_state` and `apply_provenance` count every
  held reservation. Each therefore gets a crate-private sibling taking `own: &Path`
  (`session_state_apart_from`, `apply_provenance_apart_from`, Decision 17). M4a's two keep
  their signatures; each pair shares one private body that takes `Option<&Path>`. The
  reservation is matched by its file name inside the profile's `.tagteam-launch/`, so a path
  spelled differently still matches. A descendant of `claude` still holding the inherited fd
  is left out with it, as §12.5 says ("no longer matters").
- **The check skips itself** when the launch bootstrapped, so the CLI always calls it. It
  spawns the launch command `plan_run` resolved, which the CLI passes (Decision 20).
  - Its wait is a cancellation point: a signal recorded by its end interrupts the launch
    (§12.5 "Signals"), and the spawner killed the process (Task 3). The check returns
    `Err(EngineError::Interrupted(n))` when the token was set during it, read through
    `cancel.requested()`, never through the `Validity` text.
  - `invalid` keeps the profile and sets the seed's `needsBootstrap`, which Task 9's trigger
    reads at the next launch. The write takes no lock: this launch's reservation is held, so
    every other seed writer (a bootstrap, a capture, a reseed) finds the profile owned and
    stays away. A profile with no seed needs no mark, since the next launch bootstraps it
    anyway. A seed that cannot be written is logged: the next check finds the same `invalid`.
- **Exit handling's lock waits are its cancellation points** (§12.5 "After `claude` exits").
  `MutationGuard` waits 30 s, as the launch does, because another launch may hold it through a
  bootstrap. The capture, the merge-back's splice and the unlink are critical spans. The
  merge-back's own config-lock wait honours the token too.
- **A failure, or a cancellation, gives one notice**, naming the first cause:
  - The steps that can still run, run. A failed capture does not stop the merge-back, and the
    unlink runs whenever the locks were taken.
  - Without the locks, the reservation is not unlinked (it is created and removed only under
    them, §12.5). It dies with this process, and the next launch removes it as dead.
  - A provenance `Conflict` or `Unreadable` at exit counts as a failure: nothing is captured,
    and the next launch refuses or aborts with the reason.
- **The merge-back runs only when a baseline waits** (`has_baseline`). A session that joined
  wrote none, and the last session out may have joined. Both look in `Launched.profile`, the
  profile's actual directory, never at its spelling (Decision 22).
- **`LaunchEnd::Refused`** runs exactly what `Exited` runs (§12.3: "as if `claude` had exited at
  once"). The child's code is logged and changes nothing.

**Files:**
- Modify: `crates/tagteam-engine/src/launch.rs`:
  - the imports;
  - new `check_login`, `mark_needs_bootstrap`, `finish_run` and `finish_locked` in the
    `impl Engine` block.
- Modify: `crates/tagteam-engine/src/session.rs` (M4a Task 9): `Engine::session_state` and the
  new `session_state_apart_from` share its body, now the private `session_state_leaving_out`.
- Modify: `crates/tagteam-engine/src/provenance.rs` (M4a Task 10):
  - `Engine::apply_provenance` and the new `apply_provenance_apart_from` share its body, now
    the private `apply_provenance_leaving_out`;
  - `use std::path::Path;`.
- Test: `crates/tagteam-engine/tests/launch.rs` (its imports; `finished_session`; 13 tests; one
  more in `mod hooked`)

**Interfaces:**
- Consumes:
  - Task 4: `LaunchReservation::{path, unlink}`.
  - Tasks 5 and 7: `Provider::{has_baseline(dir), merge_back(env, dir, cancel), validate_profile}`
    (the merge-back given `Launched.profile`, Decision 22), `Validity`,
    `MergeReport`.
  - Task 9: `bootstrap::refusal`, `EngineError::Login*`, `Engine.spawner`, and the fixtures
    `auth_status`, `auth_logged_out`, `auth_helper`, `auth_reply` and
    `Fx::{script_auth, script_valid, spelling_for, rotate_profile}`.
  - Task 10: `Launched`, `LaunchEnd`, `Engine::launch`, `launch::merge_summary`, the fixture
    `claude_bin`, and the test-file helpers `row`, `refused`, `seed`, `first_launch`,
    `killed_session`, `live_reservations`, `config_key`, `session_adds_project`, `block_home`
    and `unblock_home`.
  - M4a (not on this branch; written against its plan's code):
    - `session.rs`'s `session_state` body and `provenance.rs`'s `apply_provenance` body, which
      reads with `Provider::profile_identity(env, dir)` and
      `Provider::read_profile_credential(env, dir, spelling)` in the profile's directory (M4a's
      Decision 19, Decision 22 here);
    - `Seed`, `ProfileCheck`, `SessionState`;
    - the fixtures `Fx::{hold_reservation, live_record}` and `Fx.process`.
  - M3a: `MutationGuard::acquire` and `Engine::lock_account` wait on `env.cancel`;
    `Engine::cancel`, `Cancel::{request, requested}`, `EngineError::{Interrupted, signal}`.
    Task 3: `Cancel::take`, `Captured::{Interrupted, TimedOut, SpawnFailed}`.
- Produces (Interface Contract):
  - `Engine::check_login(&self, launched: &Launched, program: &Path, cwd: &Path) -> Result<(), EngineError>`.
  - `Engine::finish_run(&self, launched: Launched, end: LaunchEnd) -> Vec<String>`.
  - Crate-private (Decision 17):
    `Engine::session_state_apart_from(&self, p: &dyn Provider, row: &AccountRow, own: &Path) -> Result<SessionState, EngineError>`
    and
    `Engine::apply_provenance_apart_from(&self, p: &dyn Provider, row: &AccountRow, lock: &AccountLock, own: &Path) -> Result<ProfileCheck, EngineError>`.
  - The hook point `exit-before-locks`.
  - For Task 12:
    - call `check_login` before the spawn. On its error, call `finish_run(launched, LaunchEnd::Refused)`,
      print each notice as `note: …`, and fail with the error (exit 1, or §14.1's code for
      `interrupted`).
    - After `claude` exits, call `Cancel::take` (Decision 1), then `finish_run(.., LaunchEnd::Exited(code))`,
      print its notices, and exit with the child's code.

**Spec:**
- §12.3 "Every launch is checked": after the locks, before the spawn, with the reservation
  held; `valid` launches; `overridden`, `drifted`, `unknown` and `unreachable` refuse;
  `invalid` refuses, keeps the profile and records that it needs a bootstrap. A refusal after
  the reservation exists runs the exit handling as if `claude` had exited at once, and exits 1.
- §12.5 step 6 and "Signals": the wait for the login check is a cancellation point; after
  `claude` exits, a new signal cancels the exit handling at its next lock wait; its writes are
  critical spans; a cancelled or failed exit handling prints a notice and loses nothing.
- §12.5 "When the child exits": under `MutationGuard`, then the account lock. Quiescent apart
  from its own reservation, it captures (provenance, same identity, no network) and merges
  back. Otherwise (another `run`, or a `bg` or `daemon` record) it only unlinks. The unlink is
  last.
- §12.5 "Lazy capture": the next launch captures and merges back what was left.
- §12.4 "Merge back": one stderr summary line when both sides changed keys, the default's
  values kept, each key named in the log; a failure keeps the baseline for the next launch.
- §12.6: records of every `kind` count, `bg` and `daemon` included.
- B.62, B.63; Decision 12; Review Focus 3, 4 and 5 (its second half).

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/launch.rs`, replace the `use` block with:

```rust
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{
    Fx, auth_helper, auth_logged_out, auth_reply, auth_status, claude_bin, credential,
    mutation_lock_free, splice_config_key, token_requests, two_accounts, vault_fp,
};
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::launch::{LaunchEnd, Launched};
use tagteam_engine::refresh::{GateOutcome, OwnedBy};
use tagteam_engine::store::AccountRow;
use tagteam_engine::vault::SERVICE;
use tagteam_provider::Read;
use tagteam_provider::flock::{FlockGuard, LockProbe, probe_lock};
use tagteam_provider::http::{HttpError, Method};
use tagteam_provider::process::Captured;
use tagteam_provider::profile::{LAUNCH_DIR, MARKER_FILE, ProfileMarker, Seed, launch_reservations};
use tagteam_provider::splice::{get_top_level, remove_top_level};
```

After `killed_session`, add:

```rust
/// A session of `id` that ran and exited cleanly, its exit handling done. Returns the profile.
fn finished_session(fx: &Fx, id: &AccountId, email: &str) -> PathBuf {
    let launched = first_launch(fx, id, email);
    let profile = launched.profile.clone();
    let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));
    assert!(notices.is_empty(), "{notices:?}");
    profile
}
```

Append these tests before `mod hooked`:

```rust
#[test]
fn the_login_check_is_skipped_by_a_launch_that_bootstrapped() {
    // §12.3: its validation already ran, under the locks.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let launched = first_launch(&fx, &a, "a@x.co");

    fx.engine.check_login(&launched, claude_bin(), &fx.work_dir("app")).unwrap();

    assert_eq!(fx.spawner.specs().len(), 1);
    assert!(fx.engine.finish_run(launched, LaunchEnd::Exited(0)).is_empty());
}

#[test]
fn every_other_launch_is_checked_in_its_own_directory_with_its_reservation_held() {
    // §12.3 "Every launch is checked", §12.5 step 6: after the locks, before the spawn.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = finished_session(&fx, &a, "a@x.co");
    let cwd = fx.work_dir("app/src");
    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    fx.script_valid(&profile, "a@x.co");

    fx.engine.check_login(&launched, claude_bin(), &cwd).unwrap();

    let specs = fx.spawner.specs();
    assert_eq!(specs.len(), 2);
    assert_eq!(
        specs[1].cwd.as_deref(),
        Some(cwd.as_path()),
        "where claude runs, so a project's own settings count"
    );
    assert_eq!(specs[1].program, claude_bin(), "the binary the session will run");
    assert!(mutation_lock_free(&fx.env));
    assert_eq!(live_reservations(&profile), 1);
    assert!(fx.engine.finish_run(launched, LaunchEnd::Exited(0)).is_empty());
}

#[test]
fn every_refusing_outcome_of_the_login_check_keeps_the_profile() {
    // §12.3's table outside a bootstrap, B.62.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = finished_session(&fx, &a, "a@x.co");
    let spelling = fx.spelling_for(&profile);
    let cwd = fx.work_dir("app");
    let rows = [
        (
            auth_reply(0, auth_helper(&spelling).to_string().as_bytes()),
            "login-overridden",
        ),
        (
            auth_reply(0, auth_status("/somewhere/else", "a@x.co").to_string().as_bytes()),
            "login-drifted",
        ),
        (Captured::TimedOut, "login-unknown"),
        (auth_reply(0, b"not json"), "login-unknown"),
        (
            Captured::SpawnFailed("No such file or directory".into()),
            "launch-unreachable",
        ),
    ];
    for (reply, kind) in rows {
        let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
        fx.spawner.push(reply);

        let err = fx.engine.check_login(&launched, claude_bin(), &cwd).unwrap_err();

        assert_eq!(err.kind(), kind, "{err}");
        let notices = fx.engine.finish_run(launched, LaunchEnd::Refused);
        assert!(notices.is_empty(), "{kind}: {notices:?}");
        assert!(profile.join(MARKER_FILE).exists(), "{kind}: kept");
        assert!(!seed(&profile).needs_bootstrap, "{kind}");
    }
}

#[test]
fn an_invalid_login_check_keeps_the_profile_and_the_next_launch_bootstraps_it() {
    // §12.3, B.62: only a bootstrap's validation deletes a profile.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = finished_session(&fx, &a, "a@x.co");
    let cwd = fx.work_dir("app");
    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    fx.script_auth(1, &auth_logged_out(&fx.spelling_for(&profile)));

    let err = fx.engine.check_login(&launched, claude_bin(), &cwd).unwrap_err();

    assert_eq!(err.kind(), "login-invalid", "{err}");
    assert!(seed(&profile).needs_bootstrap, "recorded for the next launch");
    assert!(fx.engine.finish_run(launched, LaunchEnd::Refused).is_empty());
    assert!(profile.join(MARKER_FILE).exists(), "kept");

    fx.script_valid(&profile, "a@x.co");
    let next = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    assert!(next.bootstrapped);
    assert!(!seed(&profile).needs_bootstrap);
    assert!(fx.engine.finish_run(next, LaunchEnd::Exited(0)).is_empty());
}

#[test]
fn a_signal_during_the_login_check_interrupts_the_launch() {
    // §12.5 "Signals": the wait for the check is a cancellation point, and its process killed.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = finished_session(&fx, &a, "a@x.co");
    let cwd = fx.work_dir("app");
    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    fx.env.cancel.request(libc::SIGTERM);
    fx.spawner.push(Captured::Interrupted(libc::SIGTERM));

    let err = fx.engine.check_login(&launched, claude_bin(), &cwd).unwrap_err();

    let _ = fx.env.cancel.take();
    assert_eq!(err.kind(), "interrupted", "{err}");
    assert_eq!(err.signal(), Some(libc::SIGTERM));
    assert!(!seed(&profile).needs_bootstrap);
    assert!(fx.engine.finish_run(launched, LaunchEnd::Refused).is_empty());
}

#[test]
fn a_launch_refused_by_its_login_check_runs_its_exit_handling_at_once() {
    // §12.3, §12.5 step 6: as if `claude` had exited at once.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = finished_session(&fx, &a, "a@x.co");
    let cwd = fx.work_dir("app");
    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    let own = launched.reservation.path().to_path_buf();
    assert!(profile.join(".tagteam-baseline.json").exists(), "seeded");
    fx.script_auth(0, &auth_helper(&fx.spelling_for(&profile)));
    assert_eq!(
        fx.engine.check_login(&launched, claude_bin(), &cwd).unwrap_err().kind(),
        "login-overridden"
    );

    let notices = fx.engine.finish_run(launched, LaunchEnd::Refused);

    assert!(notices.is_empty(), "{notices:?}");
    assert!(!own.exists(), "unlinked");
    assert!(
        !profile.join(".tagteam-baseline.json").exists(),
        "merged back, with nothing to merge"
    );
    assert!(mutation_lock_free(&fx.env));
}

#[test]
fn the_last_session_out_captures_a_rotation_and_merges_its_config_back() {
    // §12.5 "When the child exits".
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let launched = first_launch(&fx, &a, "a@x.co");
    let profile = launched.profile.clone();
    let own = launched.reservation.path().to_path_buf();
    session_adds_project(&profile, "/work/new");
    fx.rotate_profile(&profile, "rt-a-2");

    let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));

    assert!(notices.is_empty(), "{notices:?}");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    assert_eq!(seed(&profile).seed_fp, vault_fp(&fx, &a), "the seed moves with the capture");
    assert_eq!(
        config_key(&fx.paths().global_config, "projects")["/work/new"],
        json!({"allowedTools": ["Bash"]}),
        "merged back (§12.4)"
    );
    assert!(!profile.join(".tagteam-baseline.json").exists());
    assert!(!own.exists(), "unlinked last");
    assert!(mutation_lock_free(&fx.env));
}

#[test]
fn of_two_sessions_the_last_one_out_captures_a_rotation_either_made() {
    // Review Focus 3: the second joins without a seed or a bootstrap; the last one out captures.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = finished_session(&fx, &a, "a@x.co");
    // Another `tagteam run` of the account starts first, in another process.
    let other = fx.hold_reservation(&profile);
    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")).unwrap();
    assert!(!launched.bootstrapped);
    assert!(
        !profile.join(".tagteam-baseline.json").exists(),
        "joined without a seed"
    );
    fx.rotate_profile(&profile, "rt-a-2");
    // The other exits first. Its exit handling finds this session running, so it only unlinks.
    let theirs = other.path().to_path_buf();
    drop(other);
    fs::remove_file(theirs).unwrap();

    let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));

    assert!(notices.is_empty(), "{notices:?}");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    assert_eq!(seed(&profile).seed_fp, vault_fp(&fx, &a));
}

#[test]
fn the_first_session_out_leaves_capture_and_merge_back_to_the_last() {
    // Review Focus 3, the other order.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let cwd = fx.work_dir("app");
    let launched = first_launch(&fx, &a, "a@x.co");
    let profile = launched.profile.clone();
    let own = launched.reservation.path().to_path_buf();
    // A second session joined, in another process.
    let other = fx.hold_reservation(&profile);
    session_adds_project(&profile, "/work/new");
    fx.rotate_profile(&profile, "rt-a-2");

    let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));

    assert!(notices.is_empty(), "{notices:?}");
    assert!(!own.exists(), "only the unlink");
    assert_eq!(
        fx.vault_refresh_token(&a).as_deref(),
        Some("rt-a"),
        "nothing is captured under a running session"
    );
    assert!(profile.join(".tagteam-baseline.json").exists(), "the merge-back waits");
    assert_eq!(
        config_key(&fx.paths().global_config, "projects").get("/work/new"),
        None
    );

    // The other session ends with its `tagteam` killed: the next launch completes both.
    drop(other);
    let next = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    assert!(!next.bootstrapped);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    assert!(
        config_key(&fx.paths().global_config, "projects")
            .get("/work/new")
            .is_some()
    );
    drop(next);
}

#[test]
fn a_background_session_keeps_the_profile_until_it_exits() {
    // §12.5, §12.6: a `daemon` record counts, and capture and merge-back wait for it.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let cwd = fx.work_dir("app");
    let launched = first_launch(&fx, &a, "a@x.co");
    let profile = launched.profile.clone();
    let daemon = fx.live_record(&profile, 4242, "daemon");
    session_adds_project(&profile, "/work/new");
    fx.rotate_profile(&profile, "rt-a-2");

    let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));

    assert!(notices.is_empty(), "{notices:?}");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert!(profile.join(".tagteam-baseline.json").exists());
    assert_eq!(live_reservations(&profile), 0, "its own reservation is gone all the same");

    // The daemon shuts down gracefully and removes its record (Appendix A.7).
    fs::remove_file(daemon).unwrap();
    let next = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    assert!(!next.bootstrapped);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    assert!(
        config_key(&fx.paths().global_config, "projects")
            .get("/work/new")
            .is_some()
    );
    drop(next);
}

#[test]
fn a_failed_merge_back_keeps_the_baseline_and_the_next_launch_merges_it() {
    // §12.4: the profile's changes and the baseline are kept, so it is retried before any re-seed.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let cwd = fx.work_dir("app");
    let launched = first_launch(&fx, &a, "a@x.co");
    let profile = launched.profile.clone();
    let own = launched.reservation.path().to_path_buf();
    session_adds_project(&profile, "/work/new");

    block_home(&fx);
    let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));
    unblock_home(&fx);

    assert_eq!(notices.len(), 1, "one notice: {notices:?}");
    assert!(notices[0].contains("did not finish"), "{}", notices[0]);
    assert!(profile.join(".tagteam-baseline.json").exists());
    assert!(!own.exists(), "the unlink still runs, last");

    let next = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    assert_eq!(
        config_key(&fx.paths().global_config, "projects")["/work/new"],
        json!({"allowedTools": ["Bash"]})
    );
    drop(next);
}

#[test]
fn a_merge_back_keeps_the_default_where_both_changed_and_says_so_once() {
    // Review Focus 4 (§12.4): one key changed on both sides, a new project, a removed MCP
    // server. Every other byte of the default file stays as it was.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let launched = first_launch(&fx, &a, "a@x.co");
    let mine = launched.profile.join(".claude.json");
    let theirs = fx.paths().global_config;
    let mut projects = config_key(&mine, "projects");
    projects["/work/app"]["allowedTools"] = json!(["Bash"]);
    projects["/work/new"] = json!({"allowedTools": []});
    splice_config_key(&mine, "projects", &projects);
    splice_config_key(&mine, "mcpServers", &json!({}));
    let mut defaults = config_key(&theirs, "projects");
    defaults["/work/app"]["allowedTools"] = json!(["Read"]);
    splice_config_key(&theirs, "projects", &defaults);
    let rest = |doc: &[u8]| {
        remove_top_level(&remove_top_level(doc, "projects").unwrap(), "mcpServers").unwrap()
    };
    let before = rest(&fs::read(&theirs).unwrap());

    let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));

    assert_eq!(
        notices,
        ["1 key of position 1's session config changed on both sides while it ran; the default home's values were kept"]
    );
    let merged = config_key(&theirs, "projects");
    assert_eq!(merged["/work/app"]["allowedTools"], json!(["Read"]), "the default wins");
    assert_eq!(merged["/work/app"]["history"], json!(["x"]), "untouched");
    assert_eq!(merged["/work/new"], json!({"allowedTools": []}), "applied");
    assert_eq!(config_key(&theirs, "mcpServers"), json!({}), "the removal applied");
    assert_eq!(rest(&fs::read(&theirs).unwrap()), before, "every other byte as it was");
}

#[test]
fn an_api_key_helper_added_to_the_shared_settings_refuses_the_next_launch_and_keeps_the_profile() {
    // Review Focus 5, second half (§12.3 "Every launch is checked"). The helper is in the
    // shared `settings.json`, which the profile reads through its link; the scripted check
    // answers as Claude Code then does.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = finished_session(&fx, &a, "a@x.co");
    let settings = fx.env.home.join(".claude/settings.json");
    fs::write(&settings, r#"{"theme":"dark","apiKeyHelper":"~/bin/key"}"#).unwrap();
    assert_eq!(
        fs::read_link(profile.join("settings.json")).unwrap(),
        fs::canonicalize(&settings).unwrap()
    );
    let cwd = fx.work_dir("app");
    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    fx.script_auth(0, &auth_helper(&fx.spelling_for(&profile)));

    let err = fx.engine.check_login(&launched, claude_bin(), &cwd).unwrap_err();

    assert!(
        matches!(&err, EngineError::LoginOverridden { method, .. } if method == "api_key_helper"),
        "{err}"
    );
    assert!(!seed(&profile).needs_bootstrap);
    assert!(fx.engine.finish_run(launched, LaunchEnd::Refused).is_empty());
    assert!(profile.join(MARKER_FILE).exists(), "kept");
}
```

Append inside `mod hooked`:

```rust
    #[test]
    fn a_signal_during_exit_handling_defers_it_and_loses_nothing() {
        // §12.5 "After `claude` exits", B.63: its lock wait is a cancellation point; lazy
        // capture and the next launch complete what it left.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let cwd = fx.work_dir("app");
        let launched = first_launch(&fx, &a, "a@x.co");
        let profile = launched.profile.clone();
        let own = launched.reservation.path().to_path_buf();
        session_adds_project(&profile, "/work/new");
        fx.rotate_profile(&profile, "rt-a-2");
        let cancel = fx.env.cancel.clone();
        fx.engine.on_point(
            "exit-before-locks",
            Box::new(move || cancel.request(libc::SIGTERM)),
        );

        let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));

        let _ = fx.env.cancel.take();
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
        assert!(profile.join(".tagteam-baseline.json").exists());
        assert_eq!(
            probe_lock(&own).unwrap(),
            LockProbe::Free,
            "left in place; its lock went with this process's hold"
        );

        let next = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
        assert!(
            config_key(&fx.paths().global_config, "projects")
                .get("/work/new")
                .is_some()
        );
        assert_eq!(live_reservations(&profile), 1);
        drop(next);
    }
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test launch`
Expected: compile errors: `no method named finish_run found for struct Engine` and
`no method named check_login found for struct Engine`.

- [ ] **Step 3: Implement**

**`crates/tagteam-engine/src/session.rs`** (M4a Task 9). Replace `Engine::session_state` with
the three functions below (Decision 17). The body is M4a's, unchanged except for the
reservation filter. On the re-synced code, apply only the filter and the two delegations.

```rust
impl Engine {
    /// §12.5: reservations (any `Held`) and session records (`record_is_live`, any `Unreadable`).
    pub fn session_state(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
    ) -> Result<SessionState, EngineError> {
        self.session_state_leaving_out(p, row, None)
    }

    /// `session_state`, with `own` left out: this process's own launch reservation, as §12.5
    /// "When the child exits" asks whether the profile is quiescent apart from it. It is
    /// matched by file name inside `.tagteam-launch/`.
    pub(crate) fn session_state_apart_from(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        own: &Path,
    ) -> Result<SessionState, EngineError> {
        self.session_state_leaving_out(p, row, Some(own))
    }

    /// The body of both. The profile is `profile_path(env, id)` (§5). A held reservation
    /// other than `own`, then a live record, makes the account `Owned`; failing that, anything
    /// that could not be read makes it `Unreadable`. Every I/O failure is a state, never an
    /// error. A provider without `sessions` has no profiles, and nothing on disk is touched for
    /// it.
    fn session_state_leaving_out(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        own: Option<&Path>,
    ) -> Result<SessionState, EngineError> {
        if !p.capabilities().sessions {
            return Ok(SessionState::NoProfile);
        }
        let profile = profile_path(&self.env, &row.id);
        match fs::symlink_metadata(&profile) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(SessionState::NoProfile),
            Err(e) => {
                let detail = format!("{}: {e}", profile.display());
                return Ok(unreadable_state(&profile, detail));
            }
        }
        let is_own = |path: &Path| own.is_some_and(|own| path.file_name() == own.file_name());
        match launch_reservations(&profile) {
            Read::Present(found) => {
                if found
                    .iter()
                    .any(|(path, probe)| *probe == LockProbe::Held && !is_own(path))
                {
                    return Ok(SessionState::Owned { profile });
                }
            }
            Read::Absent => {}
            Read::Unreadable(e) => return Ok(unreadable_state(&profile, e.to_string())),
        }
        let mut damaged = None;
        match read_session_records(&p.session_records_dir(&profile)) {
            Read::Present(entries) => {
                for entry in entries {
                    match entry {
                        RecordEntry::Record(r) => {
                            if record_is_live(self.process.as_ref(), &r, p.launch_command()) {
                                return Ok(SessionState::Owned { profile });
                            }
                        }
                        RecordEntry::Unreadable { path, detail } => {
                            damaged.get_or_insert_with(|| format!("{}: {detail}", path.display()));
                        }
                    }
                }
            }
            Read::Absent => {}
            Read::Unreadable(e) => return Ok(unreadable_state(&profile, e.to_string())),
        }
        Ok(match damaged {
            Some(detail) => unreadable_state(&profile, detail),
            None => SessionState::Quiescent { profile },
        })
    }
}
```

**`crates/tagteam-engine/src/provenance.rs`** (M4a Task 10). Add `use std::path::Path;` and
replace `Engine::apply_provenance` with the three functions below (Decision 17). The body is
M4a's, unchanged except for its first statement. On the re-synced code, apply only that
statement and the two delegations.

```rust
impl Engine {
    /// §12.5 under `lock` (the account's): reads the seed, the marker's spelling and the
    /// profile credential; applies `tagteam_core::provenance`; captures through
    /// `persist_generation` and moves the seed on `Capture`; reseeds on `InStep { reseed: true }`.
    pub(crate) fn apply_provenance(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
    ) -> Result<ProfileCheck, EngineError> {
        self.apply_provenance_leaving_out(p, row, lock, None)
    }

    /// `apply_provenance` for a profile quiescent apart from `own`, this process's launch
    /// reservation (§12.5 "When the child exits": the last session out captures).
    pub(crate) fn apply_provenance_apart_from(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        own: &Path,
    ) -> Result<ProfileCheck, EngineError> {
        self.apply_provenance_leaving_out(p, row, lock, Some(own))
    }

    /// The body of both.
    ///
    /// A capture needs a quiescent profile with a seed, the same identity, a fresh read of a
    /// credential with a refresh token (§6.2), and the table's `Capture` row, which a stale
    /// mark never reaches. Expiry is never consulted (B.52). A seed that cannot be moved after a
    /// capture or a reseed is an error: going on would make the next comparison a false
    /// `Conflict`. An unreadable vault is `EngineError::Unreadable`, which the caller reports as
    /// it reports its own vault reads. Unreadable also covers a marker that is absent beside a
    /// seed or names another account, and a profile identity that cannot be read.
    fn apply_provenance_leaving_out(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        own: Option<&Path>,
    ) -> Result<ProfileCheck, EngineError> {
        debug_assert_eq!(lock.id(), &row.id, "the caller holds this account's lock");
        let state = match own {
            Some(own) => self.session_state_apart_from(p, row, own)?,
            None => self.session_state(p, row)?,
        };
        let SessionState::Quiescent { profile } = state else {
            return Ok(ProfileCheck::NotApplicable);
        };
        let seed = match Seed::read(&profile) {
            Read::Present(seed) => seed,
            Read::Absent => return Ok(ProfileCheck::NotApplicable),
            Read::Unreadable(e) => return Ok(unreadable(&e)),
        };
        let marker = match ProfileMarker::read(&profile) {
            Read::Present(m) if m.account_id == row.id && m.provider == row.provider => m,
            Read::Present(_) => {
                return Ok(ProfileCheck::Unreadable(format!(
                    "the marker in {} names another account",
                    profile.display()
                )));
            }
            Read::Absent => {
                return Ok(ProfileCheck::Unreadable(format!(
                    "{} has a seed but no marker",
                    profile.display()
                )));
            }
            Read::Unreadable(e) => return Ok(unreadable(&e)),
        };
        // §12.2: the profile's Keychain item is named from the recorded spelling, never one
        // derived again; its files are read in `profile`, where the profile is now.
        let spelling = marker.config_dir.as_str();
        match p.profile_identity(&self.env, &profile) {
            Read::Present(login) if !identity_drifted(&login, row) => {}
            // Another account's login, or none to compare: the profile is ignored (§12.5).
            Read::Present(_) | Read::Absent => return Ok(ProfileCheck::NotApplicable),
            Read::Unreadable(e) => return Ok(unreadable(&e)),
        }
        let held = match p.read_profile_credential(&self.env, &profile, spelling) {
            Read::Present(c) if c.provenance() == Provenance::Degraded => {
                return Ok(ProfileCheck::Unreadable(format!(
                    "the credential of {} could be read only from its file, which may be out of date",
                    profile.display()
                )));
            }
            Read::Present(c) => c.bytes().to_vec(),
            Read::Absent => return Ok(ProfileCheck::NotApplicable),
            Read::Unreadable(e) => return Ok(unreadable(&e)),
        };
        // §6.2: only a credential with a refresh token is a generation the vault may take.
        let Some(p_fp) = p
            .fingerprint(&held)
            .filter(|_| p.has_refresh_token(&held))
        else {
            return Ok(ProfileCheck::NotApplicable);
        };
        let v_fp = match self.vault.read(&row.id) {
            Read::Present(bytes) => match p.fingerprint(&bytes) {
                Some(fp) => fp,
                None => return Ok(ProfileCheck::NotApplicable),
            },
            Read::Absent => return Ok(ProfileCheck::NotApplicable),
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        let stale = seed.login_epoch != row.login_epoch;
        Ok(
            match provenance(p_fp.as_str(), v_fp.as_str(), &seed.seed_fp, stale) {
                ProvenanceVerdict::InStep { reseed } => {
                    if reseed {
                        Seed {
                            seed_fp: v_fp.as_str().to_owned(),
                            ..seed
                        }
                        .write(&profile)?;
                    }
                    ProfileCheck::InStep
                }
                ProvenanceVerdict::Capture => {
                    self.persist_generation(p, row, lock, &held)?;
                    Seed {
                        seed_fp: p_fp.as_str().to_owned(),
                        ..seed
                    }
                    .write(&profile)?;
                    tracing::info!(
                        position = row.position,
                        account = %row.id,
                        "captured the session profile's rotated login into the vault"
                    );
                    ProfileCheck::Captured
                }
                ProvenanceVerdict::VaultMovedOn => ProfileCheck::VaultMovedOn,
                ProvenanceVerdict::ReplacementWins => ProfileCheck::ReplacementWins,
                ProvenanceVerdict::Conflict => {
                    tracing::warn!(
                        position = row.position,
                        account = %row.id,
                        "the session profile and the vault both moved since they last agreed; nothing is captured or refreshed until `tagteam add` replaces the login"
                    );
                    ProfileCheck::Conflict
                }
            },
        )
    }
}
```

**`crates/tagteam-engine/src/launch.rs`.** The imports become:

```rust
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use tagteam_provider::profile::{Seed, profile_path};
use tagteam_provider::provider::{MergeReport, SessionEnv, Validity};
use tagteam_provider::reservation::{LaunchReservation, remove_dead_reservations};
use tagteam_provider::{MutationGuard, Provider, Read, ReadError};

use crate::account_lock::AccountLock;
use crate::bootstrap::{Trigger, refusal};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::provenance::ProfileCheck;
use crate::refresh::{GateOutcome, OwnedBy};
use crate::run::no_sessions;
use crate::session::SessionState;
use crate::store::AccountRow;
use crate::switch::{cannot_refresh, needs_relogin, works_until_expiry};
```

Add these methods to the `impl Engine` block, after `prepare_quiescent`:

```rust
    /// §12.3 "Every launch is checked", run after the launch's locks are released, with its
    /// reservation held, in `cwd`, the directory `claude` will run in, spawning `program`, the
    /// launch command `plan_run` resolved (Decision 20). A launch that bootstrapped was checked
    /// under its locks, and skips it. The check's wait is a cancellation point: a signal
    /// recorded by its end interrupts the launch (§12.5 "Signals"), as
    /// `Err(EngineError::Interrupted(n))`, read through the token and never through the
    /// `Validity` text. `valid` launches; `overridden`, `drifted`, `unknown` and `unreachable`
    /// refuse and keep the profile. `invalid` refuses too, keeps it, and records that the next
    /// launch bootstraps it. After a refusal the caller runs
    /// `finish_run(.., LaunchEnd::Refused)`.
    pub fn check_login(
        &self,
        launched: &Launched,
        program: &Path,
        cwd: &Path,
    ) -> Result<(), EngineError> {
        if launched.bootstrapped {
            return Ok(());
        }
        let row = &launched.account;
        let p = self.provider(&row.provider)?;
        let identity = p.parse_identity(&row.identity_json)?;
        let validity = p.validate_profile(
            &self.env,
            &launched.spelling,
            cwd,
            program,
            &identity,
            self.spawner.as_ref(),
            self.cancel(),
        );
        if let Some(signal) = self.cancel().requested() {
            return Err(EngineError::Interrupted(signal));
        }
        if matches!(validity, Validity::Invalid(_)) {
            self.mark_needs_bootstrap(row, &launched.profile);
        }
        refusal(row, validity).map_or(Ok(()), Err)
    }

    /// §12.3: an `invalid` login outside a bootstrap keeps the profile and marks it for the
    /// next launch to bootstrap. The reservation this launch holds keeps every other seed writer
    /// away (each needs a quiescent profile), so no lock is taken. With no seed, the next launch
    /// bootstraps anyway. A seed that cannot be written is logged: the next check finds the
    /// same `invalid`.
    fn mark_needs_bootstrap(&self, row: &AccountRow, profile: &Path) {
        let result = match Seed::read(profile) {
            Read::Present(seed) => Seed {
                needs_bootstrap: true,
                ..seed
            }
            .write(profile)
            .map_err(EngineError::from),
            Read::Absent => Ok(()),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
        };
        if let Err(e) = result {
            tracing::warn!(
                position = row.position,
                account = %row.id,
                "could not mark the session profile for a bootstrap: {e}"
            );
        }
    }

    /// §12.5 "When the child exits": capture and merge-back when last out, then the unlink.
    /// Returns the notices to print; never changes the exit code (B.63).
    ///
    /// It runs under `MutationGuard` (30 s, as a launch) and the account lock, whose waits are
    /// its cancellation points. `end` changes nothing: a launch refused after its reservation
    /// exists is handled as if `claude` had exited at once (§12.3). A failed or cancelled step
    /// gives one notice. What it left is completed by lazy capture and the next launch, which
    /// loses nothing (§12.5 "Signals").
    pub fn finish_run(&self, launched: Launched, end: LaunchEnd) -> Vec<String> {
        let (position, id) = (launched.account.position, launched.account.id.clone());
        tracing::debug!(position, account = %id, ?end, "the session ended; exit handling starts");
        let mut notices = Vec::new();
        if let Err(e) = self.finish_locked(launched, &mut notices) {
            tracing::warn!(position, account = %id, "exit handling did not finish: {e}");
            notices.push(format!(
                "exit handling for position {position} did not finish ({e}); nothing is lost: the next launch, switch or refresh of the account completes it"
            ));
        }
        notices
    }

    /// `finish_run`'s work. Under the locks, every step runs and the first failure is returned
    /// after the unlink. Without them, the reservation is left as it is: it dies with this
    /// process, and the next launch removes it (§12.5 step 2).
    fn finish_locked(
        &self,
        launched: Launched,
        notices: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        let Launched {
            account,
            profile,
            reservation,
            ..
        } = launched;
        let p = self.provider(&account.provider)?;
        let p = p.as_ref();
        hooks::point(self, "exit-before-locks")?;
        let guard = MutationGuard::acquire(&self.env, MutationGuard::BOOTSTRAP_TIMEOUT)?;
        let lock = self.lock_account(&account.id)?;
        let mut failed: Option<EngineError> = None;
        if let Some(row) = self.store()?.account(&account.id)? {
            let own = reservation.path();
            if let SessionState::Quiescent { .. } = self.session_state_apart_from(p, &row, own)? {
                // 1. Capture: provenance under the account lock, with no network (§6.2).
                match self.apply_provenance_apart_from(p, &row, &lock, own) {
                    Ok(ProfileCheck::Conflict) => {
                        failed = Some(EngineError::ProfileConflict {
                            position: row.position,
                            label: row.label.clone(),
                        });
                    }
                    Ok(ProfileCheck::Unreadable(detail)) => {
                        failed = Some(EngineError::Unreadable(ReadError::new(
                            profile.display().to_string(),
                            detail,
                        )));
                    }
                    Ok(_) => {}
                    Err(e) => failed = Some(e),
                }
                // 2. Merge back, with the default home's config lock taken alone (§4.3), from
                //    the baseline in the profile's actual directory (Decision 22).
                if p.has_baseline(&profile) {
                    match p.merge_back(&self.env, &profile, self.cancel()) {
                        Ok(report) => notices.extend(merge_summary(&row, &report)),
                        Err(e) => {
                            if failed.is_none() {
                                failed = Some(e.into());
                            }
                        }
                    }
                }
            }
        }
        // Last, in either case.
        let unlinked = reservation.unlink();
        drop(lock);
        drop(guard);
        unlinked?;
        failed.map_or(Ok(()), Err)
    }
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-engine --features test-hooks --test launch`
Expected: PASS, 39 tests (6 in `hooked`).

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS. M4a's tests of `session_state` and `apply_provenance` (`tests/session.rs`,
`tests/provenance.rs`, the gate's step 2 and 3) are unchanged: both run their old body, with
no reservation left out.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/launch.rs crates/tagteam-engine/src/session.rs \
  crates/tagteam-engine/src/provenance.rs crates/tagteam-engine/tests/launch.rs
git commit -m "Check every launch's login and finish a session's exit handling"
```

---

### Task 12: The `run` command: exec, spawn, signals, exit code

`run` is the one command whose process outlives its decision. Before the launch it is an
ordinary command: its errors are its own, `--json` gives them the envelope, and a signal at a
cancellation point interrupts it (§14.1). Once `claude` starts, stdout, the terminal and the
exit code belong to `claude`: tagteam forwards the signals a `kill` sent to it alone, drops the
ones the terminal sent to both, runs exit handling when `claude` exits, and exits with
`claude`'s code (§12.5, B.63). Plain `claude` is `exec`, so tagteam is gone before it starts.

This task wires Tasks 3, 8, 10 and 11 into the CLI. It writes nothing in the engine.

**Readings of the spec this task commits to:**
- **`run` has its own end in `app.rs`.** `Ended` gains `Child(i32)`: the child's exit code, after
  which no late-signal notice is printed. Every signal while `claude` ran was either `claude`'s
  (the terminal's) or forwarded to it, and exit handling prints its own notices (Decision 12).
  Errors before the launch still end as `Ended::Code` or `Ended::Interrupted`, as for any
  command, so `--json` and the exit codes 1 and `128 + n` are unchanged.
- **The lock check runs on the session path only.** `run` is not in `touches_keychain`. Plain
  `claude` reads no Keychain item, and the shell wrapper (§12.7) runs `run` in every directory,
  so an unmapped directory must never wait on an unlock prompt. A session reads the vault and
  the profile's item, so `run_agent` calls `ensure_unlocked` once `plan_run` says `Session`.
- **The environment warnings are the engine's** (§12.5 "Environment", Decision 18).
  `Launched.warnings` carries them with the launch's other warnings, and `run` prints every one
  as `warning: …`, computing none itself. Its part is the process boundary:
  `Context::from_process` records in `Env.vars` which variables a session scrubs are set here,
  as their presence only (`app::scrubbed_vars`), and never a value, since a value may be a
  login.
- **A target that changed under the launch's locks is planned once more** (Decision 14).
  `launch`'s `TargetChanged { why }` prints `warning: {why}`, then `plan_run` runs once more
  with the same request, and `run` acts on that plan: plain `claude`, a refusal, or a launch. A
  second `TargetChanged` is the command's error (`target-changed`, exit 1), never a loop. A
  removal that `launch` finds before its locks is the same `TargetChanged` (Task 10), so a
  mapped directory whose account was removed mid-launch runs plain `claude`, with the warning.
- **The launch command is the plan's.** `RunPlan::Session.launch` goes to `launch`, to
  `check_login` and to the spawn, so the login check and the session run one binary
  (Decision 20). After a re-plan, the new plan's path is used.
- **A launch interrupted after its reservation exists spends its signal on that.** Its exit
  handling runs "as for a refused launch" (§12.5), and the token is cleared first, so the exit
  handling's lock waits stop only at a new signal. Without the clear, the very signal that
  interrupted the launch would cancel its exit handling at once, and leave the reservation file
  and the baseline to the next launch.
- **The plain path looks at the token once, before `exec`.** A signal recorded during
  `plan_run` ends the command as interrupted and nothing starts. A signal after that look is
  plain `claude`'s own (§12.1); one landing in the instant between the look and the `exec` is
  lost with tagteam's handlers, as M3a's preflight notes.
- **A spawn that fails after the reservation exists is `launch-unreachable`** (§12.3's
  `unreachable` row: "the launch command could not be spawned"), with exit 1 after its exit
  handling. An `exec` that fails is `launch-command-missing` when the command is gone, and the
  I/O error, naming the command, otherwise.
- **Exit handling's notices are `note:` lines on stderr**, the CLI's existing form for engine
  notices (`App::notices`).
- **The binary tests run Task 3's fake `claude`** (Decision 11), steered by the variables
  `fake_claude` documents, and read what it did from `FAKE_CLAUDE_OUT` (`fake_claude_calls`).

**Files:**
- Create: `crates/tagteam/src/run.rs` (`run_session`, `abandon`, `exec_failed`, `forwarded`,
  and the test-only `before-spawn` pause point)
- Modify: `crates/tagteam/tests/run_cli.rs` (Task 3's file: the `use` block; helpers and tests
  appended)
- Modify: `crates/tagteam/src/cli.rs` (the `Run` variant after `Statusline` (on M3a's branch,
  :100–104); the `touches_keychain` doc comment, :106–124)
- Modify: `crates/tagteam/src/app.rs` (on M3a's branch: `Ended` :300–306, `run` :312–331,
  `run_command` :333–398, `command_name` :400–416, `dispatch`'s `Statusline` arm :773, new
  `App::run_agent` and `App::run_plain`, imports; M4a Task 8's `Context::from_process` and a
  new `scrubbed_vars` beside its `session_vars`; and the `tests` module's
  `the_late_notice_names_each_command_as_it_is_typed` :1045–1078 and three new tests)
- Modify: `crates/tagteam/src/lib.rs` (`mod run;` beside `mod root_guard;` :12; the `--json`
  pre-scan, :53)
- Modify: `crates/tagteam/src/signals.rs` (module doc, `survive_quit`)

**Interfaces:**

Written against M3a's and M4a's plans, whose code is not on this branch yet: `app.rs` is M3a's
`run`/`run_command` split (Task 6 Part B) with M4a Task 8's `build_registry`, `locate` and
run-shell refusal applied, as the execution re-sync leaves it.

- Consumes:
  - Tasks 1 and 2: `Command::{Map, Unmap, ShellInit}` and their `command_name` arms.
  - Task 3: `tagteam_provider::process::{SpawnSpec, spawn_session, exec_command, exit_code}`,
    `Cancel::take(&self) -> Option<i32>`; the fake `claude` in `tests/common/mod.rs`
    (`fake_claude(root: &Path) -> PathBuf`, `path_with(bin: &Path) -> OsString`, `FakeCall`,
    `fake_claude_calls(out: &Path) -> Vec<FakeCall>`, and the `FAKE_CLAUDE_*` variables it
    documents); `run_cli.rs`'s `send(pid: u32, signal: libc::c_int)`.
  - Task 7: `Provider::session_env` and `SessionEnv.remove`, for `scrubbed_vars`.
  - Task 8: `tagteam_engine::run::{RunRequest, RunPlan}`, `Engine::plan_run`,
    `EngineError::{LaunchCommandMissing { command }, RequiresSession { .. }}`.
  - Task 10: `tagteam_engine::launch::Launched` (`account`, `profile`, `spelling`,
    `reservation`, `env`, `bootstrapped`, `warnings`, the environment warnings among them),
    `Engine::launch(&self, account: &AccountRow, program: &Path, cwd: &Path)`,
    `LaunchReservation::fd`, `EngineError::{LaunchUnreachable { detail }, TargetChanged { why }}`,
    and the engine hook points `launch-before-freshen` and `launch-before-locks`.
  - Task 1: `map` (the removal race's mapped directory).
  - Task 11: `Engine::check_login(&self, launched: &Launched, program: &Path, cwd: &Path)` (an
    error whose `signal()` is `Some(n)` when the token was set during the check, §12.5),
    `Engine::finish_run`, `tagteam_engine::launch::LaunchEnd`.
  - M3a: `Cancel`, `Engine::cancel`, `EngineError::{Interrupted, signal}`, `signals::install`
    and its private `ignored`, `Ended`, `fail`, `KIND_INTERRUPTED`, `App::ensure_unlocked`,
    `App::resolve`, and the `TAGTEAM_TEST_PAUSE_AT` / `TAGTEAM_TEST_PAUSE_DIR` protocol
    (`hooks::pause`), which the CLI's own pause point copies.
  - M4a: `app::{build_registry, session_vars, locate, build_engine(ctx, registry, run_shell, env, provider)}`,
    `Context::from_process`, `Env.vars`, `RunShell`, `EngineError::RunShellUnreadable`,
    `ProfileMarker`, `profile_path`, and `tests/common`'s `cc_profile`.
- Produces:
  - `Command::Run { account: Option<String>, require_session: bool, args: Vec<OsString> }`.
  - `pub(crate) fn run_session(engine: &Engine, launched: Launched, launch: &Path, args: &[OsString], cwd: &Path, cancel: &Cancel, err: &mut dyn Write) -> Result<i32, EngineError>`
    (the Interface Contract's signature).
  - `pub(crate) fn abandon(engine: &Engine, launched: Launched, cancel: &Cancel, err: &mut dyn Write, e: EngineError) -> EngineError`.
  - `pub(crate) fn exec_failed(spec: &SpawnSpec, e: io::Error) -> EngineError`.
  - `pub(crate) fn forwarded(signal: i32, first: bool) -> bool`.
  - `pub(crate) fn signals::survive_quit() -> std::io::Result<()>`.
  - `pub(crate) fn app::scrubbed_vars(registry: &ProviderRegistry) -> Vec<String>`, and
    `Context::from_process`'s presence-only record of each one that is set.
  - The test-only pause point `before-spawn` in `run_session` (after the token's last check).
  - `tests/run_cli.rs`'s local helpers, which Task 13 calls: `Home` (`new`, `root`, `out`,
    `tagteam`, `profile`), `spawn`, `send_group`, `wait_while_running`, `finish`, `settle`,
    `session`, `started`, `login_checks`, `caught`, `reservations`, `marker_spelling`, `kind`,
    `stderr`, `release`, `Release`, `start`, and the constants `LONG`, `SETTLE` and `BASELINE`.

**Spec:**
- §12.1: `run [ACCOUNT] [--provider P] [--require-session] [-- <agent args>]`; plain `claude`
  is `exec` with the environment it was given (the outer home's, inside a run shell); `--json`
  covers only errors before the launch.
- §12.3 "Every launch is checked": a launch that did not bootstrap checks after releasing its
  locks; a refusal after the reservation exists runs exit handling and exits 1.
- §12.5 steps 6–7: the check, then the spawn with the reservation fd, in the terminal's
  foreground group.
- §12.5 "Signals": cancellation points before the spawn and one last look at the token; a signal
  recorded after it, SIGINT included, goes to `claude` once spawned; while `claude` runs,
  SIGINT and SIGQUIT are ignored and SIGTERM and SIGHUP forwarded; the exit code is the child's,
  `128 + n` after a signal.
- §12.5 "Environment": the scrub, with a warning per variable that was set; a pre-set home
  variable is overridden with a warning; nothing is scrubbed on the plain path.
- §12.5 "When the child exits": exit handling, its notices on stderr.
- §14.1: interrupted commands exit `128 + n` with the `interrupted` error; `claude` under `run`
  stays in the terminal's foreground group.
- B.36 (`--json` is one object), B.37 (nothing waits on what `run` holds), B.47 (decided again
  under the locks), B.63 (the child's exit status); Decisions 1, 8, 12, 14, 18 and 20.

- [ ] **Step 1: Write the failing tests**

**1a. The fake `claude`** is Task 3's (Decision 11): `common::{fake_claude, path_with,
FakeCall, fake_claude_calls}`, steered by the `FAKE_CLAUDE_*` variables `fake_claude`
documents. This task adds no variable and no second script: a session's facts are read from
the `FakeCall` its run recorded in `FAKE_CLAUDE_OUT`.

**1b. Parsing and the late-notice names.** In `crates/tagteam/src/app.rs`'s `tests` module,
replace `the_late_notice_names_each_command_as_it_is_typed` with the version below. The case
list becomes a slice, so the tasks that add commands no longer resize an array. The `map`,
`unmap` and `shell-init` cases are Tasks 1 and 2's. Then append the two parse tests and the
test of `scrubbed_vars`, whose `Context` is built as M4a Task 8's
`the_process_boundary_captures_every_session_variable_and_claudecode` builds it.

```rust
    #[test]
    fn the_late_notice_names_each_command_as_it_is_typed() {
        use clap::Parser;
        let cases: &[&[&str]] = &[
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
            &["map"],
            &["unmap"],
            &["shell-init", "zsh"],
            &["run"],
            &["run", "2", "--", "--json"],
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

    #[test]
    fn run_hands_everything_after_the_double_dash_to_the_agent() {
        use clap::Parser;
        // Decision 8: tagteam's options come before `--`, and nothing after it is tagteam's.
        let cli = Cli::try_parse_from([
            "tagteam",
            "run",
            "2",
            "--require-session",
            "--",
            "--json",
            "-p",
            "x",
            "--",
        ])
        .unwrap();
        assert!(!cli.json, "an agent's --json is not tagteam's");
        assert_eq!(cli.provider, None, "nor is its -p");
        let Some(Command::Run {
            account,
            require_session,
            args,
        }) = cli.command
        else {
            panic!("not parsed as run");
        };
        assert_eq!(account.as_deref(), Some("2"));
        assert!(require_session);
        assert_eq!(args, ["--json", "-p", "x", "--"].map(OsString::from));
    }

    #[test]
    fn run_s_own_options_stay_its_own_before_the_double_dash() {
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["tagteam", "--json", "run", "-p", "claude-code", "--", "hi"])
                .unwrap();
        assert!(cli.json);
        assert_eq!(cli.provider.as_deref(), Some("claude-code"));
        let Some(Command::Run {
            account,
            require_session,
            args,
        }) = cli.command
        else {
            panic!("not parsed as run");
        };
        assert_eq!((account, require_session), (None, false));
        assert_eq!(args, [OsString::from("hi")]);
        assert!(
            Cli::try_parse_from(["tagteam", "run", "2", "hi"]).is_err(),
            "an agent argument needs the --"
        );
    }

    #[test]
    fn the_boundary_records_the_variables_a_session_scrubs_and_never_its_home() {
        // Decision 18: the names `Context::from_process` records the presence of, for the
        // launch's environment warnings (§12.5).
        let dir = tempfile::tempdir().unwrap();
        let ctx = Context {
            env: Env::for_test(dir.path()),
            keychain: Arc::new(FakeKeychain::new()),
            platform: Platform::MacOs,
            api_base: None,
            stdout_terminal: false,
            no_color_env: false,
            force_color_env: false,
        };
        let names = scrubbed_vars(&build_registry(&ctx));
        for name in [
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "USE_STAGING_OAUTH",
            "CLAUDE_SECURESTORAGE_CONFIG_DIR",
        ] {
            assert!(names.iter().any(|n| n == name), "{name}: {names:?}");
        }
        assert!(
            !names.iter().any(|n| n == "CLAUDE_CONFIG_DIR"),
            "a session sets its home variable, never scrubs it: {names:?}"
        );
        assert!(ctx.env.vars.is_empty(), "only the process boundary records them");
    }
```

**1c. The run module's pure parts.** Create `crates/tagteam/src/run.rs` with only its test
module for now (Step 3 writes the rest above it):

```rust
#[cfg(test)]
mod tests {
    // `io` and `SpawnSpec` come from the module's own imports.
    use super::*;

    #[test]
    fn the_first_look_after_the_spawn_forwards_any_signal_and_later_ones_only_term_and_hup() {
        // §12.5, Decision 1: (signal, first look) → forwarded to claude.
        let cases = [
            (libc::SIGINT, true, true),
            (libc::SIGTERM, true, true),
            (libc::SIGHUP, true, true),
            (libc::SIGINT, false, false),
            (libc::SIGTERM, false, true),
            (libc::SIGHUP, false, true),
            (libc::SIGQUIT, true, false),
            (libc::SIGQUIT, false, false),
        ];
        for (signal, first, want) in cases {
            assert_eq!(forwarded(signal, first), want, "{signal}, first look {first}");
        }
    }

    #[test]
    fn an_exec_that_finds_no_command_is_launch_command_missing_and_any_other_failure_is_io() {
        let spec = SpawnSpec {
            program: "/opt/bin/claude".into(),
            ..SpawnSpec::default()
        };
        let e = exec_failed(&spec, io::Error::from(io::ErrorKind::NotFound));
        assert_eq!(e.kind(), "launch-command-missing");
        let e = exec_failed(&spec, io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(e.kind(), "io");
        assert!(e.to_string().contains("/opt/bin/claude"), "{e}");
    }
}
```

**1d. The binary.** In `crates/tagteam/tests/run_cli.rs` (Task 3's file), replace the `use`
block with:

```rust
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::{
    FakeCall, cc_profile, cmd, fake_claude, fake_claude_calls, path_with, seed_home, std_cmd,
    two_fresh_accounts,
};
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_provider::{Env, MutationGuard, ProfileMarker, Read, profile_path};
```

and append, after Task 3's tests (its `fake`, `wait_for`, `send` and `mode` stay as they are,
and these tests call its `send`):

```rust
// ---- Task 12: `tagteam run` through the binary (§12.1, §12.5, §14.1) ----

/// How long one step of these tests may take before it counts as stuck.
const LONG: Duration = Duration::from_secs(20);
/// Six of the wait loop's 50 ms looks: long enough for anything tagteam would forward to land.
const SETTLE: Duration = Duration::from_millis(300);
/// §12.4's baseline: present while a merge-back is owed.
const BASELINE: &str = ".tagteam-baseline.json";

/// `a@x.co` at position 1 and `b@x.co` at position 2, the live login (`two_fresh_accounts`),
/// with the fake `claude` in `<root>/bin` and a working directory `<root>/work`. `run 1` therefore
/// launches a session; `run 2` would run plain `claude` (§12.1).
struct Home {
    dir: tempfile::TempDir,
    /// Account `a`'s id.
    a: String,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (a, _b) = two_fresh_accounts(dir.path());
        fake_claude(dir.path());
        fs::create_dir_all(dir.path().join("work")).unwrap();
        Home { dir, a }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// A fresh `FAKE_CLAUDE_OUT` file for one step's runs of the fake `claude`.
    fn out(&self, name: &str) -> PathBuf {
        let dir = self.root().join("calls");
        fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// The binary in `<root>/work`, with the fake `claude` first on `PATH`, recording its runs
    /// in `out`.
    fn tagteam(&self, out: &Path) -> Command {
        let mut c = std_cmd(self.root());
        c.env("PATH", path_with(&self.root().join("bin")))
            .env("FAKE_CLAUDE_OUT", out)
            .current_dir(self.root().join("work"));
        c
    }

    /// Account `a`'s profile (§12.2).
    fn profile(&self) -> PathBuf {
        profile_path(
            &Env::for_test(self.root()),
            &AccountId::from_string(&self.a),
        )
    }
}

/// The session's run of the fake `claude` recorded in `out`, if it started.
fn session(out: &Path) -> Option<FakeCall> {
    fake_claude_calls(out)
        .into_iter()
        .find(|c| c.mode == "session")
}

/// The session recorded in `out` is running: its record and rotation are written.
fn started(out: &Path) -> bool {
    session(out).is_some_and(|c| c.ready)
}

/// The login checks (`claude auth status`) recorded in `out`.
fn login_checks(out: &Path) -> Vec<FakeCall> {
    fake_claude_calls(out)
        .into_iter()
        .filter(|c| c.mode == "auth")
        .collect()
}

/// The signals the session recorded in `out` caught, in order.
fn caught(out: &Path) -> Vec<i32> {
    session(out).map(|c| c.signals).unwrap_or_default()
}

/// Starts `c` as a shell starts a foreground job: leading a process group of its own, so a
/// terminal's Ctrl-C or Ctrl-\ can be sent to everything in it (`send_group`).
fn spawn(mut c: Command) -> Child {
    c.process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Sends `signal` to every process in the group `pgid` leads, as a terminal sends Ctrl-C or
/// Ctrl-\ to its foreground group.
fn send_group(pgid: u32, signal: i32) {
    // SAFETY: kill(2) reads no memory of ours. A negative pid names the process group `spawn`
    // made this child lead.
    let rc = unsafe { libc::kill(-(pgid as libc::pid_t), signal) };
    assert_eq!(rc, 0, "kill: {}", std::io::Error::last_os_error());
}

/// Polls `ready` every 10 ms until it holds. Fails if `child` exits first, or after `LONG`.
fn wait_while_running(child: &mut Child, what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + LONG;
    while !ready() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("tagteam exited ({status}) before {what}");
        }
        assert!(Instant::now() < deadline, "tagteam never got to {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

/// The child's output once it has exited. Kills it and fails if it still runs after `within`.
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

fn settle() {
    thread::sleep(SETTLE);
}

/// The reservation files in `profile` (§12.5), whoever holds them.
fn reservations(profile: &Path) -> Vec<PathBuf> {
    fs::read_dir(profile.join(".tagteam-launch"))
        .map(|d| {
            d.map(|e| e.unwrap().path())
                .filter(|p| p.extension().is_some_and(|x| x == "lock"))
                .collect()
        })
        .unwrap_or_default()
}

/// The spelling `profile`'s marker records: its `CLAUDE_CONFIG_DIR` (§12.2).
fn marker_spelling(profile: &Path) -> String {
    let Read::Present(marker) = ProfileMarker::read(profile) else {
        panic!("{} has no readable marker", profile.display());
    };
    marker.config_dir
}

/// The `error.type` of the one JSON object on `stdout`.
fn kind(stdout: &[u8]) -> String {
    let v: Value = serde_json::from_slice(stdout).unwrap();
    v["error"]["type"].as_str().unwrap().to_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn release(hold: &Path) {
    fs::write(hold, b"").unwrap();
}

/// Creates each hold file when dropped, so a failing test never leaves a fake `claude` running
/// out its 30 s.
struct Release(Vec<PathBuf>);

impl Drop for Release {
    fn drop(&mut self) {
        for hold in &self.0 {
            let _ = fs::write(hold, b"");
        }
    }
}

/// `tagteam run 1 -- session`: account `a`'s session, holding until `hold` exists.
fn start(home: &Home, out: &Path, hold: &Path, extra: &[(&str, &str)]) -> Child {
    let mut c = home.tagteam(out);
    c.args(["run", "1", "--", "session"])
        .env("FAKE_CLAUDE_HOLD", hold);
    for (k, v) in extra {
        c.env(k, v);
    }
    spawn(c)
}

// ---- The plain path: `exec` (§12.1) ----

#[test]
fn plain_claude_is_exec_ed_in_place_with_the_environment_and_arguments_it_was_given() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    fs::create_dir_all(root.join("home")).unwrap();
    let bin = fake_claude(root);
    let out = root.join("calls");
    let mut c = std_cmd(root);
    c.env("PATH", path_with(&bin))
        .env("FAKE_CLAUDE_OUT", &out)
        .env("FAKE_CLAUDE_EXIT", "5")
        .env("ANTHROPIC_API_KEY", "sk-ant-plain")
        .args(["run", "--"])
        .arg("--json")
        .arg("two words")
        .arg(OsStr::from_bytes(b"not \xffutf-8"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = c.spawn().unwrap();
    let pid = child.id();
    let output = finish(child, LONG);

    assert_eq!(output.status.code(), Some(5), "{}", stderr(&output));
    let calls = fake_claude_calls(&out);
    assert_eq!(calls.len(), 1, "{calls:?}");
    let run = &calls[0];
    assert_eq!(run.pid, pid, "claude replaced tagteam: one process, one pid");
    assert_eq!(
        run.args,
        [
            OsString::from("--json"),
            OsString::from("two words"),
            OsString::from_vec(b"not \xffutf-8".to_vec()),
        ]
    );
    assert_eq!(
        run.env.get("ANTHROPIC_API_KEY").map(String::as_str),
        Some("sk-ant-plain"),
        "nothing is scrubbed on the plain path (§12.5)"
    );
    assert!(!run.env.contains_key("CLAUDE_CONFIG_DIR"));
    assert!(output.stdout.is_empty(), "the agent's --json is not tagteam's");
    assert!(
        !Env::for_test(root).data_dir().exists(),
        "an unmapped directory costs one start and writes nothing (§12.7)"
    );
}

#[test]
fn plain_claude_inside_a_run_shell_runs_on_the_outer_home() {
    // §12.1, §12.8, Decision 13: from a session's shell, plain `claude` gets the default home
    // back, not the profile the shell names.
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    seed_home(&Env::for_test(root));
    let bin = fake_claude(root);
    let (_profile, spelling) = cc_profile(root, "0192-shell");
    let out = root.join("calls");
    let mut c = std_cmd(root);
    c.env("PATH", path_with(&bin))
        .env("FAKE_CLAUDE_OUT", &out)
        .env("CLAUDE_CONFIG_DIR", &spelling)
        .args(["run", "--", "x"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = c.spawn().unwrap();
    let pid = child.id();
    let output = finish(child, LONG);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let calls = fake_claude_calls(&out);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].pid, pid, "exec'd in place");
    assert!(
        !calls[0].env.contains_key("CLAUDE_CONFIG_DIR"),
        "the outer home defined none, so neither does plain claude"
    );
}

#[test]
fn a_missing_launch_command_fails_before_anything_is_written() {
    // §12.1: looked up on PATH before any lock; missing → exit 1, nothing changed.
    let home = Home::new();
    let output = std_cmd(home.root())
        .env("PATH", "/usr/bin:/bin")
        .current_dir(home.root().join("work"))
        .args(["--json", "run", "1", "--", "x"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(kind(&output.stdout), "launch-command-missing");
    assert!(!home.profile().exists(), "no profile, no reservation");
}

// ---- `--json` and the command line (§12.1, Decision 8, B.36) ----

#[test]
fn errors_before_the_launch_are_one_json_object_and_launch_nothing() {
    let home = Home::new();
    let out = home.out("never");
    let cases: [(&[&str], &str); 3] = [
        (&["--json", "run", "9", "--", "x"], "no-such-account"),
        (
            &["--json", "run", "--require-session", "--", "x"],
            "requires-session",
        ),
        (
            &["--json", "run", "--require-session", "2", "--", "x"],
            "requires-session",
        ),
    ];
    for (args, expected) in cases {
        let output = home.tagteam(&out).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{args:?}: {}", stderr(&output));
        let v: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(v["schemaVersion"], json!(1), "{args:?}");
        assert_eq!(v["error"]["type"], json!(expected), "{args:?}");
    }
    assert!(
        fake_claude_calls(&out).is_empty(),
        "claude never ran, not even its login check"
    );
    assert!(!home.profile().exists());
}

#[test]
fn an_agent_s_json_after_the_double_dash_never_turns_a_usage_error_into_json() {
    // `main_with_args` reads `--json` before clap does, for a usage error; it stops at `--`.
    let d = tempfile::tempdir().unwrap();
    let out = cmd(d.path())
        .args(["run", "--bogus", "--", "--json"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(out.stdout.is_empty(), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(stderr(&out).starts_with("error: "), "{}", stderr(&out));
    // Before the double dash it is tagteam's flag, and the same error is one JSON object.
    let out = cmd(d.path())
        .args(["run", "--bogus", "--json", "--", "x"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert_eq!(kind(&out.stdout), "usage");
}

// ---- The session: environment, arguments, exit code (§12.5) ----

#[test]
fn the_session_gets_its_arguments_verbatim_and_its_own_environment() {
    let home = Home::new();
    let out = home.out("session");
    let output = home
        .tagteam(&out)
        .args(["run", "1", "--", "--json", "-p", "two words", "", "--", "ünï"])
        .env("ANTHROPIC_API_KEY", "sk-ant-x")
        .env("CLAUDE_CODE_OAUTH_TOKEN", "oat")
        .env("CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR", "3")
        .env("CLAUDE_SECURESTORAGE_CONFIG_DIR", "")
        .env("KEEP_ME", "kept")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    let run = session(&out).expect("the session ran");
    assert_eq!(run.args, ["--json", "-p", "two words", "", "--", "ünï"]);
    let spelling = marker_spelling(&home.profile());
    let scrubbed = [
        "ANTHROPIC_API_KEY",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
        "CLAUDE_SECURESTORAGE_CONFIG_DIR",
    ];
    let checks = login_checks(&out);
    assert_eq!(
        checks.len(),
        1,
        "one check, at the bootstrap (§12.3 step 8): {checks:?}"
    );
    let cwd = fs::canonicalize(home.root().join("work")).unwrap();
    for (what, call) in [("the session", &run), ("its login check", &checks[0])] {
        assert_eq!(call.env.get("CLAUDE_CONFIG_DIR"), Some(&spelling), "{what}");
        for gone in scrubbed {
            assert!(!call.env.contains_key(gone), "{what}: {gone} is scrubbed (§12.5)");
        }
        assert_eq!(call.env.get("KEEP_ME").map(String::as_str), Some("kept"), "{what}");
        assert_eq!(call.cwd, cwd, "{what}: where claude runs, as it runs");
    }
    let err = stderr(&output);
    for name in scrubbed {
        assert_eq!(err.matches(name).count(), 1, "one warning names {name}:\n{err}");
    }
}

#[test]
fn run_exits_with_the_session_s_code() {
    let home = Home::new();
    let out = home.out("seven");
    let output = home
        .tagteam(&out)
        .args(["run", "1", "--", "x"])
        .env("FAKE_CLAUDE_EXIT", "7")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7), "{}", stderr(&output));
    assert!(
        reservations(&home.profile()).is_empty(),
        "exit handling ran to its unlink"
    );
}

#[test]
fn a_session_killed_by_a_signal_exits_128_plus_the_signal() {
    let home = Home::new();
    let out = home.out("killed");
    let hold = home.root().join("hold");
    let _release = Release(vec![hold.clone()]);
    let mut child = start(&home, &out, &hold, &[]);
    wait_while_running(&mut child, "the session", || started(&out));
    send(session(&out).unwrap().pid, libc::SIGKILL);
    let output = finish(child, LONG);
    assert_eq!(output.status.code(), Some(128 + libc::SIGKILL), "{}", stderr(&output));
    assert!(reservations(&home.profile()).is_empty());
}

#[test]
fn a_target_that_became_the_live_login_during_the_launch_runs_plain_claude_after_one_more_plan() {
    // Decision 14, B.47: `launch` decides again under its locks; `run` plans once more.
    let home = Home::new();
    let out = home.out("raced");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _release = Release(vec![pause.join("resume")]);
    let mut c = home.tagteam(&out);
    c.args(["run", "1", "--", "x"])
        .env("TAGTEAM_TEST_PAUSE_AT", "launch-before-locks")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause);
    let mut child = spawn(c);
    let pid = child.id();
    wait_while_running(&mut child, "the launch", || pause.join("paused").exists());
    cmd(home.root()).args(["switch", "1"]).assert().success();
    fs::write(pause.join("resume"), b"").unwrap();
    let output = finish(child, LONG);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("became the live login"),
        "{}",
        stderr(&output)
    );
    let calls = fake_claude_calls(&out);
    assert_eq!(calls.len(), 1, "no login check, one plain claude: {calls:?}");
    assert_eq!(calls[0].pid, pid, "exec'd in place");
    assert!(!calls[0].env.contains_key("CLAUDE_CONFIG_DIR"));
    assert!(!home.profile().exists(), "no session was started");
}

#[test]
fn a_mapped_target_removed_before_the_launch_reads_it_runs_plain_claude_after_one_more_plan() {
    // Decision 14, B.47: a `remove` that lands after `plan_run`, before the launch's first read
    // of the account, is `TargetChanged`; the mapping went with the account (Decision 7), so the
    // second plan runs plain `claude`.
    let home = Home::new();
    let out = home.out("removed");
    cmd(home.root())
        .current_dir(home.root().join("work"))
        .args(["map", "1"])
        .assert()
        .success();
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _release = Release(vec![pause.join("resume")]);
    let mut c = home.tagteam(&out);
    c.args(["run", "--", "x"])
        .env("TAGTEAM_TEST_PAUSE_AT", "launch-before-freshen")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause);
    let mut child = spawn(c);
    let pid = child.id();
    wait_while_running(&mut child, "the launch", || pause.join("paused").exists());
    cmd(home.root()).args(["remove", "1"]).assert().success();
    fs::write(pause.join("resume"), b"").unwrap();
    let output = finish(child, LONG);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("was removed"),
        "warned once: {}",
        stderr(&output)
    );
    let calls = fake_claude_calls(&out);
    assert_eq!(calls.len(), 1, "no login check, one plain claude: {calls:?}");
    assert_eq!(calls[0].pid, pid, "exec'd in place");
    assert!(!calls[0].env.contains_key("CLAUDE_CONFIG_DIR"));
    assert!(!home.profile().exists(), "no session was started");
}

// ---- Signals while claude runs (§12.5, Review Focus 2) ----

#[test]
fn sigterm_and_sighup_to_tagteam_reach_the_session_once_and_exit_handling_still_runs() {
    for signal in [libc::SIGTERM, libc::SIGHUP] {
        let home = Home::new();
        let out = home.out("forwarded");
        let hold = home.root().join("hold");
        let _release = Release(vec![hold.clone()]);
        let mut child = start(&home, &out, &hold, &[("FAKE_CLAUDE_ON_SIGNAL", "continue")]);
        wait_while_running(&mut child, "the session", || started(&out));
        settle();

        send(child.id(), signal);
        wait_while_running(&mut child, "the forwarded signal", || caught(&out) == [signal]);
        settle();
        assert_eq!(caught(&out), [signal], "forwarded once, never again");
        assert!(child.try_wait().unwrap().is_none(), "{signal}: tagteam waits on");

        release(&hold);
        let output = finish(child, LONG);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{signal}: the child's code: {}",
            stderr(&output)
        );
        assert!(!stderr(&output).contains("interrupted"), "{signal}: {}", stderr(&output));
        assert!(reservations(&home.profile()).is_empty(), "{signal}: exit handling ran");
        assert!(!home.profile().join(BASELINE).exists(), "{signal}: and merged back");
    }
}

#[test]
fn a_forwarded_sigterm_that_ends_the_session_exits_143() {
    let home = Home::new();
    let out = home.out("term");
    let hold = home.root().join("hold");
    let _release = Release(vec![hold.clone()]);
    let mut child = start(&home, &out, &hold, &[]);
    wait_while_running(&mut child, "the session", || started(&out));
    settle();
    send(child.id(), libc::SIGTERM);
    let output = finish(child, LONG);
    assert_eq!(output.status.code(), Some(128 + libc::SIGTERM), "{}", stderr(&output));
    assert!(reservations(&home.profile()).is_empty());
}

#[test]
fn sigint_to_tagteam_alone_never_reaches_the_session() {
    // Only the terminal sends claude a Ctrl-C (it shares the foreground group); tagteam drops
    // its own copy.
    let home = Home::new();
    let out = home.out("int");
    let hold = home.root().join("hold");
    let _release = Release(vec![hold.clone()]);
    let mut child = start(&home, &out, &hold, &[("FAKE_CLAUDE_ON_SIGNAL", "continue")]);
    wait_while_running(&mut child, "the session", || started(&out));
    settle();
    send(child.id(), libc::SIGINT);
    settle();
    settle();
    assert!(caught(&out).is_empty(), "nothing forwarded");
    assert!(child.try_wait().unwrap().is_none(), "and tagteam ignored it");
    release(&hold);
    let output = finish(child, LONG);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(!stderr(&output).contains("interrupted"), "{}", stderr(&output));
}

#[test]
fn ctrl_c_at_the_terminal_reaches_the_session_once_and_tagteam_ignores_it() {
    let home = Home::new();
    let out = home.out("ctrl-c");
    let hold = home.root().join("hold");
    let _release = Release(vec![hold.clone()]);
    let mut child = start(&home, &out, &hold, &[("FAKE_CLAUDE_ON_SIGNAL", "continue")]);
    wait_while_running(&mut child, "the session", || started(&out));
    settle();

    send_group(child.id(), libc::SIGINT);
    wait_while_running(&mut child, "the Ctrl-C", || caught(&out) == [libc::SIGINT]);
    settle();
    assert_eq!(
        caught(&out),
        [libc::SIGINT],
        "once: the terminal's own, never forwarded on top"
    );
    assert!(child.try_wait().unwrap().is_none());
    release(&hold);
    let output = finish(child, LONG);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(!stderr(&output).contains("interrupted"), "{}", stderr(&output));
}

#[test]
fn sigquit_never_stops_tagteam() {
    let home = Home::new();
    let out = home.out("quit");
    let hold = home.root().join("hold");
    let _release = Release(vec![hold.clone()]);
    let mut child = start(&home, &out, &hold, &[]);
    wait_while_running(&mut child, "the session", || started(&out));
    settle();
    send(child.id(), libc::SIGQUIT);
    settle();
    assert!(
        child.try_wait().unwrap().is_none(),
        "tagteam survives Ctrl-\\ while claude runs (Decision 1)"
    );
    release(&hold);
    let output = finish(child, LONG);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}

#[test]
fn ctrl_backslash_at_the_terminal_ends_the_session_and_tagteam_reports_its_code() {
    // claude keeps Ctrl-\'s default action, which a handler in tagteam does not take from it
    // (an ignored signal would): it dies of it, and tagteam lives to exit 131. The fake leaves
    // SIGQUIT untrapped, as Claude Code does.
    let home = Home::new();
    let out = home.out("ctrl-backslash");
    let hold = home.root().join("hold");
    let _release = Release(vec![hold.clone()]);
    let mut child = start(&home, &out, &hold, &[]);
    wait_while_running(&mut child, "the session", || started(&out));
    settle();
    send_group(child.id(), libc::SIGQUIT);
    let output = finish(child, LONG);
    assert_eq!(output.status.code(), Some(128 + libc::SIGQUIT), "{}", stderr(&output));
    assert!(reservations(&home.profile()).is_empty(), "exit handling ran");
}

// ---- Before the spawn (§12.5 "Signals", §15.2 "Exit paths") ----

#[test]
fn a_signal_while_the_launch_waits_for_its_lock_launches_nothing() {
    let home = Home::new();
    let out = home.out("blocked");
    let guard = MutationGuard::acquire(&Env::for_test(home.root()), Duration::from_secs(5)).unwrap();
    let mut c = home.tagteam(&out);
    c.args(["--json", "run", "1", "--", "x"]);
    let mut child = spawn(c);
    // Into the launch's 30 s wait for the mutation lock (§9.1).
    thread::sleep(Duration::from_millis(500));
    assert!(child.try_wait().unwrap().is_none(), "it waits for the lock");
    send(child.id(), libc::SIGINT);
    let output = finish(child, Duration::from_secs(5));
    drop(guard);

    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "interrupted", "message": "interrupted"}})
    );
    assert!(
        fake_claude_calls(&out).is_empty(),
        "nothing launched, not even a login check"
    );
    assert!(reservations(&home.profile()).is_empty());
}

#[test]
fn a_signal_during_the_login_check_launches_nothing_and_still_runs_exit_handling() {
    let home = Home::new();
    // The first launch bootstraps, and its check runs under the locks. The second checks
    // after its reservation exists (§12.3 "Every launch is checked").
    let first = home.out("first");
    let status = home.tagteam(&first).args(["run", "1", "--", "x"]).status().unwrap();
    assert_eq!(status.code(), Some(0));
    let out = home.out("second");
    let mut c = home.tagteam(&out);
    c.args(["--json", "run", "1", "--", "x"])
        .env("FAKE_CLAUDE_AUTH_SLEEP", "20");
    let mut child = spawn(c);
    wait_while_running(&mut child, "the login check", || !login_checks(&out).is_empty());
    send(child.id(), libc::SIGTERM);
    let output = finish(child, Duration::from_secs(5));

    assert_eq!(output.status.code(), Some(128 + libc::SIGTERM), "{}", stderr(&output));
    assert_eq!(kind(&output.stdout), "interrupted");
    assert!(session(&out).is_none(), "claude never started");
    assert!(
        reservations(&home.profile()).is_empty(),
        "its exit handling ran, as for a refused launch, past the signal that ended it"
    );
}

#[test]
fn a_signal_recorded_after_the_last_check_reaches_the_session_once_it_starts() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let home = Home::new();
        let out = home.out("late");
        let hold = home.root().join("hold");
        let pause = home.root().join("pause");
        fs::create_dir_all(&pause).unwrap();
        let _release = Release(vec![hold.clone(), pause.join("resume")]);
        let mut child = start(
            &home,
            &out,
            &hold,
            &[
                ("FAKE_CLAUDE_ON_SIGNAL", "continue"),
                ("TAGTEAM_TEST_PAUSE_AT", "before-spawn"),
                ("TAGTEAM_TEST_PAUSE_DIR", pause.to_str().unwrap()),
            ],
        );
        wait_while_running(&mut child, "the spawn", || pause.join("paused").exists());
        send(child.id(), signal);
        fs::write(pause.join("resume"), b"").unwrap();

        // The fake records it when its trap was set in time, or dies of it when it was not:
        // either way the signal reached claude, not tagteam's own interruption.
        let deadline = Instant::now() + LONG;
        let recorded = loop {
            if child.try_wait().unwrap().is_some() {
                break false;
            }
            if caught(&out) == [signal] {
                break true;
            }
            assert!(Instant::now() < deadline, "{signal} never reached the session");
            thread::sleep(Duration::from_millis(10));
        };
        if recorded {
            settle();
            assert_eq!(caught(&out), [signal], "{signal}: forwarded once");
        }
        release(&hold);
        let output = finish(child, LONG);
        let expected = if recorded { 0 } else { 128 + signal };
        assert_eq!(output.status.code(), Some(expected), "{signal}: {}", stderr(&output));
        assert!(
            !stderr(&output).contains("interrupted"),
            "past the last check a signal is claude's: {}",
            stderr(&output)
        );
        assert!(reservations(&home.profile()).is_empty(), "{signal}");
    }
}

#[test]
fn a_launch_command_gone_at_the_spawn_refuses_and_runs_exit_handling() {
    let home = Home::new();
    let out = home.out("gone");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _release = Release(vec![pause.join("resume")]);
    let mut c = home.tagteam(&out);
    c.args(["--json", "run", "1", "--", "x"])
        .env("TAGTEAM_TEST_PAUSE_AT", "before-spawn")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause);
    let mut child = spawn(c);
    wait_while_running(&mut child, "the spawn", || pause.join("paused").exists());
    fs::remove_file(home.root().join("bin/claude")).unwrap();
    fs::write(pause.join("resume"), b"").unwrap();
    let output = finish(child, LONG);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(kind(&output.stdout), "launch-unreachable");
    assert!(session(&out).is_none());
    assert!(
        reservations(&home.profile()).is_empty(),
        "a launch refused after its reservation exists runs its exit handling (§12.3)"
    );
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --lib`
Expected: compile errors: `no variant named Run found for enum Command` and
`cannot find function scrubbed_vars` in `app.rs`'s tests, and `cannot find function forwarded`
(and `exec_failed`) in `run.rs`, which is not yet a module of the crate (`run.rs` is ignored
until `lib.rs` names it; add `mod run;` first to see the second set).

Run: `cargo test -p tagteam --features test-support --test run_cli`
Expected: the file compiles (Task 3's fake and helpers exist), Task 3's nine tests pass, and
every new test fails: `run` is not a subcommand, so each `tagteam run` exits 2 with
`error: unrecognized subcommand 'run'`. The two plain-path tests fail on `Some(5)`/`Some(0)`
against `Some(2)`; the session tests and the re-plan test on
`tagteam exited (exit status: 2) before …`.

- [ ] **Step 3: Implement**

**3a. `crates/tagteam/src/cli.rs`.** Add `use std::ffi::OsString;` above `use clap::…`. Add
this variant after `Statusline` (Tasks 1 and 2 add `Map`, `Unmap` and `ShellInit` beside it):

```rust
    /// Run the agent as ACCOUNT, in a session beside the default login
    ///
    /// With no ACCOUNT, the nearest mapped ancestor of this directory decides (`tagteam map`),
    /// and with none the agent runs as it would without tagteam. Everything after `--` goes to
    /// the agent untouched; tagteam's own options, `--json` and `--provider` included, come
    /// before it. `--json` covers only errors before the agent starts: its output is its own.
    Run {
        account: Option<String>,
        /// Refuse wherever the agent would otherwise run without a session
        #[arg(long)]
        require_session: bool,
        /// Arguments for the agent, after `--`.
        #[arg(last = true)]
        args: Vec<OsString>,
    },
```

Replace `touches_keychain`'s doc comment (its body is unchanged) with:

```rust
    /// Whether the command reads or writes a Keychain item on macOS, and so runs the lock check
    /// first (Appendix A.3). `list` and `status` are not among them although they collect usage
    /// and may read Keychain items: by a plan ruling they degrade to `keychain_unavailable`
    /// rows rather than run the lock check, so a script gets rows, not a `keychain-locked`
    /// failure. `history` and `statusline` read only the store and `~/.claude.json`, and the
    /// rest need no Keychain item. A recovery under their mutation lock (Task 21) only reads the
    /// Keychain, tri-state, and leaves what it cannot decide to the next command that checks.
    /// `run` checks for itself, and only once it knows it launches a session: plain `claude`
    /// touches no item, so an unmapped directory never waits on an unlock prompt (§12.7).
```

**3b. `crates/tagteam/src/signals.rs`.** Replace the module doc's last sentence ("SIGKILL and
SIGQUIT keep their default disposition.") with "SIGKILL keeps its default disposition, and so
does SIGQUIT until `run` spawns `claude` (`survive_quit`).", add
`use std::sync::Arc;` and `use std::sync::atomic::AtomicUsize;` to the imports, and append:

```rust
/// Decision 1, §12.5: while `claude` runs, Ctrl-\ is the child's. tagteam survives it through a
/// handler that stores into a cell nothing reads. A handled signal is reset to its default when
/// `claude` is exec'd, so the child keeps Ctrl-\'s default action, which `SIG_IGN` would have
/// taken from it. A SIGQUIT ignored at startup stays ignored, for both (as in `install`).
pub(crate) fn survive_quit() -> std::io::Result<()> {
    if ignored(libc::SIGQUIT) {
        return Ok(());
    }
    signal_hook::flag::register_usize(
        libc::SIGQUIT,
        Arc::new(AtomicUsize::new(0)),
        libc::SIGQUIT as usize,
    )?;
    Ok(())
}
```

**3c. `crates/tagteam/src/run.rs`.** Put this above the test module Step 1 wrote:

```rust
//! §12.5: `tagteam run` once its launch is made. The spawn, the wait, the signals forwarded to
//! `claude`, the exit handling its exit starts, and the exit code (§14.1, B.63).

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::Duration;

use tagteam_engine::launch::{LaunchEnd, Launched};
use tagteam_engine::{Engine, EngineError};
use tagteam_provider::Cancel;
use tagteam_provider::process::{SpawnSpec, exit_code, spawn_session};

use crate::signals;

/// How often the wait loop looks at `claude` and at the cancel token (Decision 1).
const POLL: Duration = Duration::from_millis(50);

/// `run`'s exit code when it lost track of `claude`'s status, which only a failing `waitpid`
/// can cause.
const LOST: i32 = 1;

/// §12.5 steps 6–7 and "When the child exits", after `launch` and the login check. Spawns
/// `launch` with `args` in `cwd` and `launched`'s environment, with the reservation inherited
/// through its fd. Waits for it, forwarding signals (Decision 1), then runs the exit handling
/// and prints its notices on `err`.
///
/// `Ok` is `claude`'s exit code, `128 + n` when signal `n` ended it, whatever exit handling did
/// (B.63). `Err` is a launch that never started: interrupted at the token's last look, or a
/// spawn that failed. Its exit handling has already run (§12.3).
pub(crate) fn run_session(
    engine: &Engine,
    launched: Launched,
    launch: &Path,
    args: &[OsString],
    cwd: &Path,
    cancel: &Cancel,
    err: &mut dyn Write,
) -> Result<i32, EngineError> {
    // Decision 1: Ctrl-\ is the child's. Registered before the spawn, so the exec gives
    // `claude` the default action back.
    if let Err(e) = signals::survive_quit() {
        let _ = writeln!(err, "warning: Ctrl-\\ may stop tagteam before the session ends: {e}");
    }
    // §12.5: the token's last look. A signal recorded after it is `claude`'s, and goes to it as
    // soon as it exists.
    if let Some(signal) = cancel.take() {
        notify(err, &engine.finish_run(launched, LaunchEnd::Refused));
        return Err(EngineError::Interrupted(signal));
    }
    pause_point("before-spawn");
    let spec = SpawnSpec {
        program: launch.to_path_buf(),
        args: args.to_vec(),
        set: launched.env.set.clone(),
        remove: launched.env.remove.clone(),
        cwd: Some(cwd.to_path_buf()),
    };
    let mut child = match spawn_session(&spec, Some(launched.reservation.fd())) {
        Ok(child) => child,
        Err(e) => {
            let e = EngineError::LaunchUnreachable {
                detail: format!("{}: {e}", launch.display()),
            };
            return Err(abandon(engine, launched, cancel, err, e));
        }
    };
    // The first look forwards anything, SIGINT included: what arrived while `claude` was being
    // spawned, the terminal could not deliver to it.
    forward(child.id(), cancel.take(), true);
    let status = wait(&mut child, cancel);
    // What forwarding left is spent, so exit handling meets only a signal sent from now on.
    let _ = cancel.take();
    let code = match status {
        Ok(status) => exit_code(status),
        Err(e) => {
            let _ = writeln!(err, "tagteam: lost track of the session's exit status: {e}");
            LOST
        }
    };
    notify(err, &engine.finish_run(launched, LaunchEnd::Exited(code)));
    Ok(code)
}

/// §12.3, §12.5: a launch refused or interrupted once its reservation exists runs its exit
/// handling as if `claude` had exited at once, then fails with `e`: exit 1, or `128 + n` for an
/// interruption. The signal that interrupted it is spent on that, so the exit handling stops
/// only at a new one (Decision 12).
pub(crate) fn abandon(
    engine: &Engine,
    launched: Launched,
    cancel: &Cancel,
    err: &mut dyn Write,
    e: EngineError,
) -> EngineError {
    if e.signal().is_some() {
        let _ = cancel.take();
    }
    notify(err, &engine.finish_run(launched, LaunchEnd::Refused));
    e
}

/// §12.1: `exec` returns only when it failed. A launch command gone since `plan_run` found it
/// is `launch-command-missing`, as if it had never been there; any other failure is the I/O
/// error, naming the command.
pub(crate) fn exec_failed(spec: &SpawnSpec, e: io::Error) -> EngineError {
    if e.kind() == io::ErrorKind::NotFound {
        let command = spec.program.file_name().map_or_else(
            || spec.program.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        return EngineError::LaunchCommandMissing { command };
    }
    EngineError::Io(io::Error::new(
        e.kind(),
        format!("could not run {}: {e}", spec.program.display()),
    ))
}

/// Decision 1, §12.5: whether a signal the token recorded goes on to `claude`. The first look
/// after the spawn forwards SIGINT, SIGTERM and SIGHUP alike. After it, SIGINT is the
/// terminal's to deliver, since `claude` is in its foreground group, so only SIGTERM and
/// SIGHUP, which a `kill` sends to tagteam alone, are forwarded.
pub(crate) fn forwarded(signal: i32, first: bool) -> bool {
    match signal {
        libc::SIGTERM | libc::SIGHUP => true,
        libc::SIGINT => first,
        _ => false,
    }
}

/// Sends `signal`, when `forwarded` says so, to `claude` (`pid`).
fn forward(pid: u32, signal: Option<i32>, first: bool) {
    let Some(signal) = signal.filter(|&s| forwarded(s, first)) else {
        return;
    };
    // SAFETY: kill(2) reads no memory of ours. `pid` is this process's child, which only
    // `wait` reaps, and nothing is sent once it has: the pid names `claude` or its zombie,
    // never a process that reused the number.
    let rc = unsafe { libc::kill(pid as libc::pid_t, signal) };
    if rc != 0 {
        tracing::warn!(
            signal,
            "could not forward the signal to the session: {}",
            io::Error::last_os_error()
        );
    }
}

/// Waits for `claude`, forwarding what the token records meanwhile. Nothing here waits on a
/// lock (B.37).
fn wait(child: &mut Child, cancel: &Cancel) -> io::Result<ExitStatus> {
    let pid = child.id();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            // No status to poll for: wait it out; it is still the child's exit that ends the run.
            Err(_) => return child.wait(),
        }
        thread::sleep(POLL);
        forward(pid, cancel.take(), false);
    }
}

/// Exit handling's notices (Decision 12), on stderr: stdout is `claude`'s.
fn notify(err: &mut dyn Write, notices: &[String]) {
    for notice in notices {
        let _ = writeln!(err, "note: {notice}");
    }
}

/// A test-only stop at `name`: `before-spawn` is right after the token's last look. It parks
/// as the engine's points do (M3a): with `TAGTEAM_TEST_PAUSE_AT=<name>`, it writes `paused` in
/// `TAGTEAM_TEST_PAUSE_DIR` and waits up to 30 s for `resume` there. It never looks at the
/// token, so a signal sent meanwhile meets the run exactly where it would have.
#[cfg(feature = "test-support")]
fn pause_point(name: &str) {
    if std::env::var("TAGTEAM_TEST_PAUSE_AT").as_deref() != Ok(name) {
        return;
    }
    let Some(dir) = std::env::var_os("TAGTEAM_TEST_PAUSE_DIR").map(std::path::PathBuf::from)
    else {
        return;
    };
    let _ = std::fs::write(dir.join("paused"), b"");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !dir.join("resume").exists() && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(not(feature = "test-support"))]
fn pause_point(_name: &str) {}
```

**3d. `crates/tagteam/src/lib.rs`.** Add `mod run;` after `mod root_guard;`, and in
`main_with_args` replace:

```rust
    let json = args.iter().any(|a| a == "--json");
```

with:

```rust
    // B.36: `--json` promises one JSON object, but only tagteam's own flag does. An argument
    // after `--` belongs to the agent `run` launches (Decision 8), and is never the flag.
    let json = args
        .iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "--json");
```

**3e. `crates/tagteam/src/app.rs`.** Add to the imports:

```rust
use tagteam_engine::run::{RunPlan, RunRequest};
use tagteam_provider::process::{SpawnSpec, exec_command};
```

Replace `Ended`, `run` and `run_command` with the code below. It keeps Task 2's `shell-init`
fast path where Task 2 put it: after the unreadable-marker refusal, before the engine is built.
`dispatch`'s `ShellInit` arm stays `unreachable!`.

```rust
/// How a command ended (§13.1, §14.1).
enum Ended {
    /// It finished with this exit code; its output, and any error, are written.
    Code(i32),
    /// It stopped at a cancellation point after this signal; nothing is reported yet.
    Interrupted(i32),
    /// `run`'s child ran and exited with this code (§12.5, B.63). No late notice follows it:
    /// every signal while the child ran was the child's, and exit handling reports for itself.
    Child(i32),
}

/// Runs one command and returns its exit code (§13.1). A signal the command met at a
/// cancellation point ends it with 128 + the signal and the `interrupted` error (Decision 4);
/// one it never met leaves its output and exit code alone and is reported on stderr as too
/// late (Decision 6). `run` ends with its child's code once the child has run.
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
        Ended::Child(code) => code,
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
    // §12.8: the run shell is found through the registered providers, so the registry comes
    // first. Under a marker that cannot be read the outer home is unknown, and nothing runs.
    let registry = build_registry(&ctx);
    let (run_shell, env) = locate(ctx.env.clone(), &registry);
    if let RunShell::Unreadable { marker, detail } = &run_shell {
        let e = EngineError::RunShellUnreadable {
            marker: marker.clone(),
            detail: detail.clone(),
        };
        return Ended::Code(fail(io, json, e.kind(), &e.to_string()));
    }
    // §12.7: Task 2's fast path, kept. The wrapper needs only the registered providers, so it
    // reads no store and no settings, and it comes after the marker check (Decision 19).
    if let Some(Command::ShellInit { shell }) = &cli.command {
        return Ended::Code(run_shell_init(
            &registry,
            io,
            json,
            cli.provider.as_deref(),
            *shell,
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
    let (engine, warnings) = build_engine(ctx, registry, run_shell, env, &resolved);
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
    let result = match command {
        // §12.5: `run` has an end of its own. It checks the Keychain's lock itself, once it
        // knows it launches a session, and once `claude` has run, its code is the command's.
        Command::Run {
            account,
            require_session,
            args,
        } => app
            .run_agent(account, require_session, args)
            .map(Ended::Child),
        command => {
            // Only a command that touches a Keychain item checks its lock.
            let unlocked = if command.touches_keychain() {
                app.lock_check(&command)
            } else {
                Ok(())
            };
            unlocked
                .and_then(|()| app.dispatch(command))
                .map(|()| Ended::Code(0))
        }
    };
    match result {
        Ok(ended) => ended,
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
```

Replace `command_name` with the version below. The `map`, `unmap` and `shell-init` arms are
Tasks 1 and 2's, kept as they left them:

```rust
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
        Command::Map { .. } => "map",
        Command::Unmap { .. } => "unmap",
        Command::ShellInit { .. } => "shell-init",
        Command::Run { .. } => "run",
    }
}
```

In `App::dispatch`, after the `Command::Statusline { .. } => unreachable!(…)` arm, add:

```rust
            Command::Run { .. } => unreachable!("run_command answers run before dispatch"),
```

Add these two methods to `impl App`, after `switch`:

```rust
    /// §12.1 and §12.5: `run`. Whatever fails before the launch command starts is the
    /// command's own error, as the `--json` envelope too. Once `claude` has run, its exit code
    /// is the command's (B.63).
    fn run_agent(
        &mut self,
        account: Option<String>,
        require_session: bool,
        args: Vec<OsString>,
    ) -> Result<i32, Failure> {
        let account = match &account {
            Some(input) => Some(self.resolve(input)?.id),
            None => None,
        };
        let cwd = std::env::current_dir().map_err(EngineError::Io)?;
        let request = RunRequest {
            account,
            provider: self.provider_flag.clone(),
            require_session,
            cwd: cwd.clone(),
            args: args.clone(),
        };
        let (account, launch) = match self.engine.plan_run(&request)? {
            RunPlan::Plain { spec, warning } => return self.run_plain(&spec, warning),
            RunPlan::Session {
                account, launch, ..
            } => (account, launch),
        };
        // Appendix A.3: a session reads the vault and the profile's Keychain item. Plain
        // `claude` reads neither, so it never waits on this check (§12.7).
        self.ensure_unlocked()?;
        let (launched, launch) = match self.engine.launch(&account, &launch, &cwd) {
            Ok(launched) => (launched, launch),
            // Decision 14: the target changed under the launch's locks. Plan once more and act
            // on that plan; a second change is this command's error, never a loop.
            Err(EngineError::TargetChanged { why }) => {
                let _ = writeln!(self.io.err, "warning: {why}");
                match self.engine.plan_run(&request)? {
                    RunPlan::Plain { spec, warning } => return self.run_plain(&spec, warning),
                    RunPlan::Session {
                        account, launch, ..
                    } => (self.engine.launch(&account, &launch, &cwd)?, launch),
                }
            }
            Err(e) => return Err(e.into()),
        };
        // Decision 18: the environment warnings are among them, computed by the engine.
        for w in &launched.warnings {
            let _ = writeln!(self.io.err, "warning: {w}");
        }
        let cancel = self.engine.cancel();
        // §12.3 "Every launch is checked"; a launch that bootstrapped skips it (Task 11).
        if let Err(e) = self.engine.check_login(&launched, &launch, &cwd) {
            let e = crate::run::abandon(&self.engine, launched, cancel, &mut *self.io.err, e);
            return Err(e.into());
        }
        Ok(crate::run::run_session(
            &self.engine,
            launched,
            &launch,
            &args,
            &cwd,
            cancel,
            &mut *self.io.err,
        )?)
    }

    /// §12.1's plain `claude`: `exec`, after the warning a plan may carry and the token's last
    /// look. Past that look, signals are plain `claude`'s own. It returns only when the `exec`
    /// failed.
    fn run_plain(&mut self, spec: &SpawnSpec, warning: Option<String>) -> Result<i32, Failure> {
        if let Some(w) = warning {
            let _ = writeln!(self.io.err, "warning: {w}");
        }
        if let Some(signal) = self.engine.cancel().requested() {
            return Err(EngineError::Interrupted(signal).into());
        }
        let _ = self.io.out.flush();
        let _ = self.io.err.flush();
        Err(crate::run::exec_failed(spec, exec_command(spec)).into())
    }
```

Beside M4a Task 8's `session_vars`, add:

```rust
/// The variables a session of `registry`'s providers scrubs (§12.5), by name: the names whose
/// presence `Context::from_process` records for the launch's warnings (Decision 18). A name
/// that is not UTF-8 cannot be a key of `Env.vars`, so it is left out: it is still scrubbed,
/// without a warning.
pub(crate) fn scrubbed_vars(registry: &ProviderRegistry) -> Vec<String> {
    registry
        .all()
        .iter()
        .filter(|p| p.capabilities().sessions)
        .flat_map(|p| p.session_env("").remove)
        .filter_map(|name| name.into_string().ok())
        .collect()
}
```

In M4a Task 8's `Context::from_process`, its last three lines

```rust
        let names = session_vars(&build_registry(&ctx));
        ctx.env.capture_vars(&names);
        ctx
```

become:

```rust
        let registry = build_registry(&ctx);
        let names = session_vars(&registry);
        ctx.env.capture_vars(&names);
        // Decision 18: which of the variables a session scrubs are set here, for the launch's
        // warnings (§12.5). Only their presence is kept, never a value: a value may be a login,
        // and `Env` derives `Debug`.
        for name in scrubbed_vars(&registry) {
            if std::env::var_os(&name).is_some() {
                ctx.env.vars.insert(name, OsString::new());
            }
        }
        ctx
```

Callers changed: `app::run` (the new `Ended::Child` arm), `app::run_command` (`run`'s branch),
`App::dispatch` (the unreachable arm), `Context::from_process` (the presence record). Nothing
else calls `run_command` or constructs `Ended`.

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run:
```
cargo test -p tagteam --lib run::tests
cargo test -p tagteam --lib app::tests
cargo test -p tagteam --features test-support --test run_cli
```
Expected: PASS: the two `run::tests`, the four `app::tests` above, and every `run_cli` test:
Task 3's nine and this task's twenty.

Run: `cargo test -p tagteam --features test-support`, then
`cargo test --workspace --features tagteam/test-support`
Expected: PASS. No existing test runs `run`. The `--json` pre-scan only reads further when a `--`
is present, which no existing test passes. M3a's `signals.rs` tests are unaffected: they never
reach `survive_quit`.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`; both clippy runs clean. Without `test-support`,
`pause_point` is the no-op, and `run_cli.rs` compiles to nothing.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam/src/run.rs crates/tagteam/src/cli.rs crates/tagteam/src/app.rs \
  crates/tagteam/src/lib.rs crates/tagteam/src/signals.rs crates/tagteam/tests/run_cli.rs
git commit -m "Launch claude from tagteam run: exec it plainly, or spawn it in its session and forward its signals"
```

---

### Task 13: Run invariants, races and the kill paths

Tasks 5–12 each test their own step. This task pins what only holds across all of them, and
across processes:
- the §15.3 local-state invariant around a whole `run`, for both providers;
- §15.2's fresh home;
- Review Focus 1: `tagteam` killed while `claude` runs;
- Review Focus 3: two sessions of one account;
- §15.2's concurrency bullet: launch and exit racing `remove`, `switch` and the gate;
- a signal that cancels exit handling, which the next launch completes (B.63);
- and the macOS provenance sequence with the real `security` driver.

**Readings of the spec this task commits to:**
- **`run`'s surface is the identity surface plus `projects` and `mcpServers`** of the global
  config (§3's fourth row). The engine test builds it from `identity_surface` (M4a left it with
  the switch's keys and `create_only`), so every other command's §15.3 check stays as strict as
  before. A session that changed nothing is held to the base surface: with nothing to merge,
  `~/.claude.json` is byte-identical, its `projects` and `mcpServers` spans included
  (Decision 4's `None`). That is how this plan reads §15.3's "after `run` with merge-back
  disabled".
- **FakeAgent's run surface is its identity surface plus `(identity.json, ["prefs"])`**
  (Decision 16): the flat `prefs` key of the outer `identity.json`, which its merge-back writes
  (Task 5). Its session changes a pref, so the run merges back, and the test pins that it
  writes nothing outside that surface and its profile, and nothing of Claude Code's.
- **The races are made deterministic at the two windows where ownership is at stake**, through
  the CLI's pause points:
  - `before-spawn` (Task 12): the reservation exists and the locks are released, but `claude`
    has not started.
  - `after-exit` (this task): `claude` has exited, but exit handling has not started.

  Another process's `remove`, `switch` and gate run in each window and must see the account as
  session-owned. Waiting on the launch's or the exit's own locks is ordinary lock order, which
  M2a and M3a already test.
- **"The gate" in a race is observed through `list`**, against a `MockServer` that counts token
  requests: a session-owned account is never refreshed, and no capture happens under a session.

**Files:**
- Create: `crates/tagteam-engine/tests/run_invariants.rs`
- Create: `crates/tagteam-engine/tests/run_real_keychain.rs`
- Modify: `crates/tagteam-engine/Cargo.toml` (`[features]`: `real_keychain`)
- Modify: `crates/tagteam/src/run.rs` (`run_session`: the `after-exit` pause point;
  `pause_point`'s doc comment)
- Modify: `crates/tagteam/tests/run_cli.rs` (Task 3's file, as Task 12 left it: the `use`
  block; helpers and tests appended)

**Interfaces:**

Written against M4a's and M3a's plans, whose code is not on this branch yet.

- Consumes:
  - Task 12: `run_session` and `pause_point`; `run_cli.rs`'s `Home` (`new`, `root`, `out`,
    `tagteam`, `profile`), `start`, `spawn`, `wait_while_running`, `finish`, `settle`,
    `session`, `started`, `reservations`, `kind`, `stderr`, `release`, `Release`, `LONG`,
    `SETTLE` and `BASELINE`.
  - Task 3: `run_cli.rs`'s `send`; `common::{fake_claude, fake_claude_calls, FakeCall}` and the
    `FAKE_CLAUDE_*` variables (`FAKE_CLAUDE_HOLD`, `FAKE_CLAUDE_ROTATE`, `FAKE_CLAUDE_EXIT`);
    `tagteam_provider::process::{Captured, ScriptedSpawner}`.
  - Task 10: `Engine::launch(&self, account: &AccountRow, program: &Path, cwd: &Path)`,
    `Launched::{profile, spelling}`, and the fixture `claude_bin`.
  - Task 9: `EngineConfig.spawner` and the engine fixture's `Fx.spawner: Arc<ScriptedSpawner>`.
  - Task 11: `Engine::finish_run`, `LaunchEnd`.
  - Task 5: FakeAgent's merge-back of the outer `identity.json`'s `prefs`.
  - M4a: `Fx::{snapshot, assert_only_surface_changed, assert_only_surface_changed_for}` with
    `IdentitySurface.create_only` (Task 13); `FakeFx`; `EngineConfig.{process, run_shell}`;
    `FakeProcessProbe`; `RunShell`; `Provider::identity_surface` and
    `Provider::read_profile_credential(env, dir, spelling)` (M4a's Decision 19);
    `tagteam_provider::flock::{LockProbe, probe_lock}`; `inSession` in `list --json`
    (Task 14); `session-owned` refusals in `remove` (Task 9) and `switch` (Task 11); lazy
    capture in the gate and `switch`'s pre-check (Tasks 10, 11).
  - M3a: `MutationGuard::acquire(env, timeout)`, `Engine::refresh_stored`, `GateOutcome`.
- Produces:
  - The `after-exit` pause point in `run_session`.
  - The `tagteam-engine` feature `real_keychain` (gates `run_real_keychain.rs` only).

**Spec:**
- §3: the identity surface's rows, `run`'s two among them.
- §15.3 "Local state": every file outside the surface byte-identical, Keychain included, for
  each provider.
- §15.2 "Fresh home": a `run` against a home without `projects/` or `history.jsonl` leaves both
  shared.
- §15.2 "Concurrency tests": launch and exit racing `remove`, `switch` and the gate; two
  overlapping sessions of one account.
- §15.2 "Exit paths": a signal during exit handling defers it, and the next launch completes the
  capture and merge-back; the exit code is the child's.
- §15.2 "Provenance": on macOS with the real `security` driver, session exit → inactive vault
  refresh → relaunch → capture ends with the vault's generation effective in the profile, never
  the consumed one.
- §12.5 "Launch reservation" (live while the parent or `claude` lives), "When the child exits"
  (the last one out), "Lazy capture".
- §12.4: the merge-back at `projects.<path>.<key>` and `mcpServers.<name>`; account fields are
  never merged back.
- B.28, B.31, B.37, B.44, B.46, B.63.

#### 13a: The local-state invariant and a fresh home

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-engine/tests/run_invariants.rs`:

```rust
//! §15.3's local-state invariant around a whole `tagteam run` (§3, §12.4), for both providers,
//! and §15.2's fresh home. Launch and exit handling write nothing in the default home but the
//! `projects` and `mcpServers` subtrees, by merge, and the must-share entries, created empty.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{FakeFx, Fx, claude_bin};
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::launch::LaunchEnd;
use tagteam_engine::store::AccountRow;
use tagteam_provider::process::Captured;
use tagteam_provider::splice::{get_top_level, replace_top_level};
use tagteam_provider::{IdentitySurface, Provider};

fn row(fx: &Fx, id: &AccountId) -> AccountRow {
    fx.engine.store().unwrap().account(id).unwrap().unwrap()
}

/// The directory `claude` runs in: outside HOME, so the snapshot never sees it.
fn work(dir: &Path) -> PathBuf {
    let work = dir.join("work");
    fs::create_dir_all(&work).unwrap();
    work
}

/// What `claude auth status --json` prints for `email`'s login in `id`'s profile: §12.3's
/// `valid` row. A first launch creates the profile under the canonical data dir, so that is its
/// spelling (§12.2 "One spelling").
fn valid_status(fx: &Fx, id: &AccountId, email: &str) -> Captured {
    let spelling = fs::canonicalize(fx.env.data_dir())
        .unwrap()
        .join("sessions")
        .join(id.as_str());
    Captured::Exited {
        code: Some(0),
        signal: None,
        stdout: json!({
            "loggedIn": true,
            "authMethod": "claude.ai",
            "apiProvider": "firstParty",
            "configDirectory": spelling.to_str().unwrap(),
            "email": email,
        })
        .to_string()
        .into_bytes(),
        stderr: vec![],
    }
}

/// `run`'s surface (§3): the identity surface plus the `projects` and `mcpServers` subtrees of
/// the global config, which a merge-back writes.
fn run_surface(fx: &Fx) -> IdentitySurface {
    let mut surface = fx.cc.identity_surface(&fx.env);
    let config = fx.paths().global_config;
    for (path, keys) in &mut surface.json_keys {
        if *path == config {
            keys.push("projects".into());
            keys.push("mcpServers".into());
        }
    }
    surface
}

/// FakeAgent's `run` surface (Decision 16): its identity surface plus the `prefs` key of the
/// outer `identity.json`, which its merge-back writes (Task 5).
fn fake_run_surface(ffx: &FakeFx) -> IdentitySurface {
    let mut surface = ffx.fake.identity_surface(&ffx.fx.env);
    for (path, keys) in &mut surface.json_keys {
        if path.file_name().is_some_and(|n| n == "identity.json") {
            keys.push("prefs".into());
        }
    }
    surface
}

/// Changes one top-level key of `file`, as the session's agent would.
fn edit_key(file: &Path, key: &str, edit: impl FnOnce(&mut Value)) {
    let doc = fs::read(file).unwrap();
    let mut value = get_top_level(&doc, key).unwrap().unwrap_or(Value::Null);
    edit(&mut value);
    fs::write(file, replace_top_level(&doc, key, &value).unwrap()).unwrap();
}

/// Changes one top-level key of `profile`'s `.claude.json`, as the session's Claude Code would.
fn edit_profile(profile: &Path, key: &str, edit: impl FnOnce(&mut Value)) {
    edit_key(&profile.join(".claude.json"), key, edit);
}

#[test]
fn a_run_merges_back_only_projects_and_mcp_servers_and_leaves_every_other_byte() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.spawner.push(valid_status(&fx, &a, "a@x.co"));
    let config = fx.paths().global_config;
    let before = fx.snapshot();

    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &work(fx.dir.path())).unwrap();
    let profile = launched.profile.clone();
    // The session trusts the project it runs in, opens another, adds a user-scope MCP server
    // and drops `local` (Appendix A.6). It also has a per-home id of its own.
    edit_profile(&profile, "projects", |p| {
        p["/work/app"]["hasTrustDialogAccepted"] = json!(true);
        p["/work/new"] = json!({"allowedTools": [], "hasTrustDialogAccepted": true});
    });
    edit_profile(&profile, "mcpServers", |m| {
        *m = json!({"remote": {"type": "http", "url": "https://mcp.example.com"}});
    });
    edit_profile(&profile, "userID", |u| *u = json!("profile-user"));
    fx.engine.finish_run(launched, LaunchEnd::Exited(0));
    let after = fx.snapshot();

    fx.assert_only_surface_changed_for(&run_surface(&fx), &before, &after, "run with merge-back");
    let doc = fs::read(&config).unwrap();
    assert_eq!(
        get_top_level(&doc, "projects").unwrap().unwrap(),
        json!({
            "/work/app": {"allowedTools": [], "history": ["x"], "hasTrustDialogAccepted": true},
            "/work/new": {"allowedTools": [], "hasTrustDialogAccepted": true}
        })
    );
    assert_eq!(
        get_top_level(&doc, "mcpServers").unwrap().unwrap(),
        json!({"remote": {"type": "http", "url": "https://mcp.example.com"}})
    );
    assert_eq!(
        get_top_level(&doc, "userID").unwrap().unwrap(),
        json!("user-7"),
        "per-home and account fields are never merged back (§12.4)"
    );
    assert!(!profile.join(".tagteam-baseline.json").exists(), "the merge-back ran");
}

#[test]
fn a_run_whose_session_changed_nothing_leaves_the_default_config_byte_identical() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.spawner.push(valid_status(&fx, &a, "a@x.co"));
    let config = fx.paths().global_config;
    let bytes = fs::read(&config).unwrap();
    let before = fx.snapshot();

    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &work(fx.dir.path())).unwrap();
    fx.engine.finish_run(launched, LaunchEnd::Exited(0));
    let after = fx.snapshot();

    fx.assert_only_surface_changed(&before, &after, "run with nothing to merge back");
    assert_eq!(
        fs::read(&config).unwrap(),
        bytes,
        "not even `projects` or `mcpServers` is rewritten"
    );
}

#[test]
fn a_fake_agent_run_merges_back_only_its_prefs_and_writes_nothing_of_claude_code_s() {
    // §15.2 "Provider neutrality": the snapshot walks all of HOME and every Keychain item, so
    // FakeAgent's surface also proves Claude Code's home untouched. FakeAgent validates from
    // its profile files, so the launch spawns nothing (Task 7).
    let ffx = FakeFx::new();
    let h1 = ffx.fake_add("h1", "tok-1", "renew-1");
    ffx.fake_add("h2", "tok-2", "renew-2");
    let row = ffx.engine.store().unwrap().account(&h1).unwrap().unwrap();
    let surface = fake_run_surface(&ffx);
    let outer = surface.json_keys[0].0.clone();
    let before = ffx.fx.snapshot();

    let launched = ffx.engine.launch(&row, claude_bin(), &work(ffx.fx.dir.path())).unwrap();
    // The session sets a pref of its own (Task 5's `prefs.<name>`).
    edit_key(&launched.profile.join("identity.json"), "prefs", |p| {
        p["lang"] = json!("nl");
    });
    ffx.engine.finish_run(launched, LaunchEnd::Exited(0));
    let after = ffx.fx.snapshot();

    ffx.fx
        .assert_only_surface_changed_for(&surface, &before, &after, "a FakeAgent run");
    let prefs = get_top_level(&fs::read(&outer).unwrap(), "prefs").unwrap().unwrap();
    assert_eq!(prefs["lang"], json!("nl"), "merged back into the outer identity.json");
}

#[test]
fn a_fresh_home_gets_its_memory_and_history_created_empty_and_shared() {
    // §15.2 "Fresh home", Review Focus 5's first half, at the level of a whole launch.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let claude = fx.env.home.join(".claude");
    fs::remove_dir_all(claude.join("projects")).unwrap();
    fs::remove_file(claude.join("history.jsonl")).unwrap();
    fx.spawner.push(valid_status(&fx, &a, "a@x.co"));
    let before = fx.snapshot();

    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &work(fx.dir.path())).unwrap();
    let profile = launched.profile.clone();
    fx.engine.finish_run(launched, LaunchEnd::Exited(0));
    let after = fx.snapshot();

    fx.assert_only_surface_changed(&before, &after, "a fresh home's first run");
    assert_eq!(fs::read_dir(claude.join("projects")).unwrap().count(), 0);
    assert_eq!(fs::read(claude.join("history.jsonl")).unwrap(), b"");
    for name in ["projects", "history.jsonl"] {
        let link = profile.join(name);
        assert!(
            fs::symlink_metadata(&link).unwrap().file_type().is_symlink(),
            "{name} is shared, never a private copy (§12.2, B.43)"
        );
        assert_eq!(
            fs::read_link(&link).unwrap(),
            fs::canonicalize(claude.join(name)).unwrap()
        );
    }
}
```

- [ ] **Step 2: Run them and see them pass**

Run: `cargo test -p tagteam-engine --test run_invariants`
Expected: PASS, four tests, at once. They pin invariants that Tasks 5, 9, 10 and 11 already
hold, across a whole launch and exit, and they need no new code. A failure names the step
(`run with merge-back: …/home/.claude.json changed outside [...]`) and is a bug in the task
that owns that write, not in this one.

- [ ] **Step 3: Commit**

```bash
git add crates/tagteam-engine/tests/run_invariants.rs
git commit -m "Pin the local-state invariant around a run for both providers, and a fresh home's shared memory"
```

#### 13b: The kill paths, two sessions, and the races

- [ ] **Step 4: Write the failing tests**

In `crates/tagteam/tests/run_cli.rs`, change the `use` block (Task 12's) to:

```rust
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::{
    FakeCall, cc_profile, cmd, expire_vault, fake_claude, fake_claude_calls, live_email,
    path_with, seed_home, std_cmd, two_fresh_accounts,
};
use serde_json::{Value, json};
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_core::AccountId;
use tagteam_engine::vault::SERVICE;
use tagteam_provider::flock::{LockProbe, probe_lock};
use tagteam_provider::mock_server::MockServer;
use tagteam_provider::splice::{get_top_level, replace_top_level};
use tagteam_provider::{
    Env, FileKeychain, Keychain, MutationGuard, ProfileMarker, Read, profile_path,
};
```

and append:

```rust
// ---- Task 13: the kill paths, two sessions, and the races (§15.2, Review Focus 1 and 3) ----

/// The token endpoint's path under a test base (Appendix A.5).
const TOKEN: &str = "/v1/oauth/token";

impl Home {
    /// The binary for a command that runs no agent.
    fn cmd(&self) -> assert_cmd::Command {
        cmd(self.root())
    }
}

/// Polls `ready` every 10 ms until it holds; fails after `within`. (Task 3's `wait_for` polls
/// one `FAKE_CLAUDE_OUT` file; this one waits on anything.)
fn wait_until(within: Duration, what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + within;
    while !ready() {
        assert!(Instant::now() < deadline, "never got to {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

/// A Claude Code credential of `rt`'s lineage, for `FAKE_CLAUDE_ROTATE`: what the session's
/// own refresh writes to its profile (Appendix A.4's account-scoped keys).
fn rotated(rt: &str) -> String {
    json!({"claudeAiOauth": {
        "accessToken": format!("at-{rt}"),
        "refreshToken": rt,
        "expiresAt": 4_102_444_800_000i64,
        "refreshTokenExpiresAt": 4_102_444_800_000i64,
        "scopes": ["user:inference", "user:profile"]
    }})
    .to_string()
}

/// The refresh token in a credential's bytes.
fn refresh_token(bytes: &[u8]) -> String {
    let v: Value = serde_json::from_slice(bytes).unwrap();
    v["claudeAiOauth"]["refreshToken"].as_str().unwrap().to_owned()
}

/// The refresh token of `id`'s vault generation (§6.2).
fn vault_rt(root: &Path, id: &str) -> String {
    let bytes = FileKeychain::new(root.join("keychain"))
        .find(SERVICE, id)
        .present()
        .expect("a vault generation");
    refresh_token(&bytes)
}

/// The refresh token of the default home's live credential, as Claude Code reads it.
fn live_rt(root: &Path) -> String {
    let env = Env::for_test(root);
    let bytes = FileKeychain::new(root.join("keychain"))
        .find(
            &keychain_service(&env, ItemKind::OAuth),
            &keychain_account(&env),
        )
        .present()
        .expect("a live credential");
    refresh_token(&bytes)
}

/// The refresh token in `profile`'s credential file.
fn profile_rt(profile: &Path) -> String {
    refresh_token(&fs::read(profile.join(".credentials.json")).unwrap())
}

/// What a session does to its own config (§12.4): it trusts the project at `path`.
fn trust(profile: &Path, path: &str) {
    let file = profile.join(".claude.json");
    let doc = fs::read(&file).unwrap();
    let mut projects = get_top_level(&doc, "projects")
        .unwrap()
        .unwrap_or_else(|| json!({}));
    projects[path] = json!({"allowedTools": [], "hasTrustDialogAccepted": true});
    fs::write(&file, replace_top_level(&doc, "projects", &projects).unwrap()).unwrap();
}

/// Whether the config at `file` trusts the project at `path`.
fn trusts(file: &Path, path: &str) -> bool {
    let doc = fs::read(file).unwrap();
    get_top_level(&doc, "projects")
        .unwrap()
        .is_some_and(|p| p[path]["hasTrustDialogAccepted"] == json!(true))
}

/// The default home's `~/.claude.json`.
fn default_config(home: &Home) -> PathBuf {
    home.root().join("home/.claude.json")
}

/// No reservation file in `profile` is held: whatever held one has exited (§12.5).
fn all_free(profile: &Path) -> bool {
    reservations(profile)
        .iter()
        .all(|r| probe_lock(r).unwrap() != LockProbe::Held)
}

/// `list --json`'s row for account `id`.
fn listed(v: &Value, id: &str) -> Value {
    v["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(id))
        .cloned()
        .unwrap()
}

/// `tagteam <args> --json` for a command that refuses: exit 1, and the error's kind.
fn refused(home: &Home, args: &[&str]) -> String {
    let out = home.cmd().args(args).arg("--json").output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{args:?}: {}", stderr(&out));
    kind(&out.stdout)
}

/// Account `a` is in a session, as another process sees it: `list` marks it, sending no token
/// request, and `switch` and `remove` refuse (§10.3, §12.5).
fn assert_session_owned(home: &Home, server: &MockServer) {
    let out = home
        .cmd()
        .env("TAGTEAM_TEST_API_BASE", server.base_url())
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(listed(&v, &home.a)["inSession"], json!(true), "{v}");
    assert_eq!(server.hits("POST", TOKEN), 0, "the gate never refreshes a session's account");
    assert_eq!(refused(home, &["switch", "1"]), "session-owned");
    assert_eq!(live_email(home.root()), "b@x.co");
    assert_eq!(refused(home, &["remove", "1"]), "session-owned");
    assert!(home.profile().exists());
}

/// Review Focus 1's start: account `a`'s session runs, rotates its credential to `rt-a2`, and
/// trusts `/work/killed`; then `tagteam` is killed with SIGKILL under it. Returns the hold file
/// that lets the orphaned session go, and its guard.
fn kill_mid_session(home: &Home) -> (PathBuf, Release) {
    let out = home.out("killed");
    let hold = home.root().join("hold-killed");
    let guard = Release(vec![hold.clone()]);
    let mut c = home.tagteam(&out);
    c.args(["run", "1", "--", "x"])
        .env("FAKE_CLAUDE_HOLD", &hold)
        .env("FAKE_CLAUDE_ROTATE", rotated("rt-a2"))
        .process_group(0)
        .stdin(Stdio::null())
        // The orphaned session keeps tagteam's output: never a pipe this test reads.
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = c.spawn().unwrap();
    wait_while_running(&mut child, "the session", || started(&out));
    trust(&home.profile(), "/work/killed");
    send(child.id(), libc::SIGKILL);
    assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGKILL));
    (hold, guard)
}

/// After `kill_mid_session`: `claude` holds the reservation through its inherited fd, so the
/// account stays session-owned, and nothing is captured under it (§12.5, B.46).
fn assert_still_owned(home: &Home) {
    let held = reservations(&home.profile());
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(probe_lock(&held[0]).unwrap(), LockProbe::Held);
    assert_session_owned(home, &MockServer::start());
    assert_eq!(vault_rt(home.root(), &home.a), "rt-a", "nothing captured under a session");
}

#[test]
fn a_killed_tagteam_leaves_its_reservation_live_and_the_next_launch_captures_and_merges_back() {
    let home = Home::new();
    let profile = home.profile();
    let (hold, _guard) = kill_mid_session(&home);
    assert_still_owned(&home);

    // A launch now joins the orphaned session: no capture, no bootstrap and no seed under it.
    let join = home.out("join");
    let out = home.tagteam(&join).args(["run", "1", "--", "x"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(vault_rt(home.root(), &home.a), "rt-a");
    assert_eq!(profile_rt(&profile), "rt-a2", "never bootstrapped over (B.28)");
    assert!(trusts(&profile.join(".claude.json"), "/work/killed"), "never re-seeded (B.44)");
    assert!(profile.join(BASELINE).exists());
    assert!(!trusts(&default_config(&home), "/work/killed"), "not merged while it runs");

    // The orphan exits, and its lock goes with it.
    release(&hold);
    wait_until(LONG, "the orphaned session's exit", || all_free(&profile));
    let next = home.out("next");
    let out = home.tagteam(&next).args(["run", "1", "--", "x"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(vault_rt(home.root(), &home.a), "rt-a2", "lazily captured at the next launch");
    assert!(
        trusts(&default_config(&home), "/work/killed"),
        "the left-over baseline merged back first (§12.5 step 2)"
    );
    assert!(!profile.join(BASELINE).exists());
    assert!(reservations(&profile).is_empty(), "the dead reservation is gone too");
}

#[test]
fn after_a_killed_tagteam_s_session_ends_a_switch_captures_its_rotation_first() {
    let home = Home::new();
    let profile = home.profile();
    let (hold, _guard) = kill_mid_session(&home);
    assert_eq!(refused(&home, &["switch", "1"]), "session-owned");

    release(&hold);
    wait_until(LONG, "the orphaned session's exit", || all_free(&profile));
    let out = home.cmd().args(["switch", "1", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(vault_rt(home.root(), &home.a), "rt-a2", "§9.2's pre-check captured it");
    assert_eq!(
        live_rt(home.root()),
        "rt-a2",
        "and the switch activated it, never the consumed rt-a"
    );
    assert!(profile.join(BASELINE).exists(), "the merge-back waits for the next launch");
}

#[test]
fn after_a_killed_tagteam_s_session_ends_the_gate_captures_its_rotation() {
    let home = Home::new();
    let profile = home.profile();
    let (hold, _guard) = kill_mid_session(&home);
    release(&hold);
    wait_until(LONG, "the orphaned session's exit", || all_free(&profile));

    // §7.3 step 3: the next refresh `a` needs, which `list`'s collection reaches.
    expire_vault(home.root(), &home.a, 60_000);
    home.cmd().args(["list", "--json"]).assert().success();
    assert_eq!(vault_rt(home.root(), &home.a), "rt-a2");
}

#[test]
fn the_last_of_two_sessions_out_captures_a_rotation_either_made_and_merges_back() {
    // Review Focus 3, in both orders. The session that rotates also exits first, so it is
    // always the other, last one out that captures.
    for first_rotates in [true, false] {
        let home = Home::new();
        let profile = home.profile();
        let (o1, o2) = (home.out("one"), home.out("two"));
        let (h1, h2) = (home.root().join("hold-one"), home.root().join("hold-two"));
        let _guard = Release(vec![h1.clone(), h2.clone()]);
        let rotation = rotated("rt-a2");
        let rotate: &[(&str, &str)] = &[("FAKE_CLAUDE_ROTATE", rotation.as_str())];

        let mut one = start(&home, &o1, &h1, if first_rotates { rotate } else { &[] });
        wait_while_running(&mut one, "the first session", || started(&o1));
        trust(&profile, "/work/shared");
        let baseline = fs::read(profile.join(BASELINE)).unwrap();
        let mut two = start(&home, &o2, &h2, if first_rotates { &[] } else { rotate });
        wait_while_running(&mut two, "the second session", || started(&o2));

        // The second joined: no seed over the first's changes, no bootstrap over its credential.
        assert!(trusts(&profile.join(".claude.json"), "/work/shared"), "{first_rotates}");
        assert_eq!(fs::read(profile.join(BASELINE)).unwrap(), baseline, "{first_rotates}");
        assert_eq!(profile_rt(&profile), "rt-a2", "{first_rotates}");
        assert_eq!(reservations(&profile).len(), 2, "{first_rotates}");

        // The first out leaves capture and merge-back to the last.
        let (first, first_hold, last, last_hold) = if first_rotates {
            (one, h1.clone(), two, h2.clone())
        } else {
            (two, h2.clone(), one, h1.clone())
        };
        release(&first_hold);
        let out = finish(first, LONG);
        assert_eq!(out.status.code(), Some(0), "{first_rotates}: {}", stderr(&out));
        assert_eq!(vault_rt(home.root(), &home.a), "rt-a", "{first_rotates}");
        assert!(!trusts(&default_config(&home), "/work/shared"), "{first_rotates}");
        assert!(profile.join(BASELINE).exists(), "{first_rotates}");
        assert_eq!(reservations(&profile).len(), 1, "{first_rotates}");

        release(&last_hold);
        let out = finish(last, LONG);
        assert_eq!(out.status.code(), Some(0), "{first_rotates}: {}", stderr(&out));
        assert_eq!(vault_rt(home.root(), &home.a), "rt-a2", "{first_rotates}");
        assert!(trusts(&default_config(&home), "/work/shared"), "{first_rotates}");
        assert!(!profile.join(BASELINE).exists(), "{first_rotates}");
        assert!(reservations(&profile).is_empty(), "{first_rotates}");
    }
}

#[test]
fn a_reservation_owns_the_account_before_claude_starts() {
    // §12.5: the reservation covers the gap before claude writes its own session record.
    let home = Home::new();
    let server = MockServer::start();
    // Due, so the gate would refresh `a` if it were not in a session.
    expire_vault(home.root(), &home.a, 60_000);
    let out = home.out("racing");
    let hold = home.root().join("hold");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _guard = Release(vec![hold.clone(), pause.join("resume")]);
    let mut child = start(
        &home,
        &out,
        &hold,
        &[
            ("TAGTEAM_TEST_PAUSE_AT", "before-spawn"),
            ("TAGTEAM_TEST_PAUSE_DIR", pause.to_str().unwrap()),
        ],
    );
    wait_while_running(&mut child, "the spawn", || pause.join("paused").exists());
    assert!(session(&out).is_none(), "claude has not started");
    assert_eq!(reservations(&home.profile()).len(), 1);

    assert_session_owned(&home, &server);

    fs::write(pause.join("resume"), b"").unwrap();
    wait_while_running(&mut child, "the session", || started(&out));
    release(&hold);
    let output = finish(child, LONG);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}

#[test]
fn exit_handling_finishes_before_another_process_can_take_the_account() {
    // §12.5 "When the child exits": between claude's exit and the unlink, the reservation is
    // still tagteam's, so remove, switch and the gate all see a session.
    let home = Home::new();
    let server = MockServer::start();
    expire_vault(home.root(), &home.a, 60_000);
    let profile = home.profile();
    let out = home.out("exiting");
    let hold = home.root().join("hold");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _guard = Release(vec![hold.clone(), pause.join("resume")]);
    let rotation = rotated("rt-a2");
    let mut child = start(
        &home,
        &out,
        &hold,
        &[
            ("FAKE_CLAUDE_ROTATE", rotation.as_str()),
            ("TAGTEAM_TEST_PAUSE_AT", "after-exit"),
            ("TAGTEAM_TEST_PAUSE_DIR", pause.to_str().unwrap()),
        ],
    );
    wait_while_running(&mut child, "the session", || started(&out));
    trust(&profile, "/work/raced");
    release(&hold);
    wait_while_running(&mut child, "the exit handling", || pause.join("paused").exists());
    assert!(
        session(&out).is_some_and(|c| c.exit.is_some()),
        "claude has exited"
    );

    assert_session_owned(&home, &server);
    assert_eq!(vault_rt(home.root(), &home.a), "rt-a", "nothing captured outside exit handling");

    fs::write(pause.join("resume"), b"").unwrap();
    let output = finish(child, LONG);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(vault_rt(home.root(), &home.a), "rt-a2");
    assert!(trusts(&default_config(&home), "/work/raced"));
    assert!(!profile.join(BASELINE).exists());
    assert!(reservations(&profile).is_empty());
    // Quiescent now: remove goes ahead and takes the profile with it.
    home.cmd().args(["remove", "1"]).assert().success();
    assert!(!profile.exists());
}

#[test]
fn a_signal_during_exit_handling_defers_it_to_the_next_launch_and_keeps_the_child_s_code() {
    // §12.5 "After claude exits", B.63, §15.2 "Exit paths".
    let home = Home::new();
    let profile = home.profile();
    let out = home.out("deferred");
    let hold = home.root().join("hold");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _guard = Release(vec![hold.clone(), pause.join("resume")]);
    let rotation = rotated("rt-a2");
    let mut child = start(
        &home,
        &out,
        &hold,
        &[
            ("FAKE_CLAUDE_ROTATE", rotation.as_str()),
            ("FAKE_CLAUDE_EXIT", "3"),
            ("TAGTEAM_TEST_PAUSE_AT", "after-exit"),
            ("TAGTEAM_TEST_PAUSE_DIR", pause.to_str().unwrap()),
        ],
    );
    wait_while_running(&mut child, "the session", || started(&out));
    trust(&profile, "/work/deferred");
    release(&hold);
    wait_while_running(&mut child, "the exit handling", || pause.join("paused").exists());
    let lock = MutationGuard::acquire(&Env::for_test(home.root()), Duration::from_secs(5)).unwrap();
    fs::write(pause.join("resume"), b"").unwrap();
    settle(); // into exit handling's wait for the mutation lock
    send(child.id(), libc::SIGTERM);
    let output = finish(child, LONG);
    drop(lock);

    assert_eq!(output.status.code(), Some(3), "the child's code: {}", stderr(&output));
    let err = stderr(&output);
    assert!(err.contains("note: "), "a notice says what was left undone:\n{err}");
    assert!(!err.contains("too late"), "{err}");
    assert_eq!(vault_rt(home.root(), &home.a), "rt-a");
    assert!(!trusts(&default_config(&home), "/work/deferred"));
    assert!(profile.join(BASELINE).exists());
    assert!(all_free(&profile), "what it left died with it");

    let next = home.out("next");
    let output = home.tagteam(&next).args(["run", "1", "--", "x"]).output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(vault_rt(home.root(), &home.a), "rt-a2");
    assert!(trusts(&default_config(&home), "/work/deferred"));
    assert!(!profile.join(BASELINE).exists());
    assert!(reservations(&profile).is_empty());
}
```

- [ ] **Step 5: Run them and see the right ones fail**

Run: `cargo test -p tagteam --features test-support --test run_cli`
Expected:
- FAIL, two tests, each with `tagteam exited (exit status: N) before the exit handling`: no
  `after-exit` point exists yet, so the run never parks between `claude`'s exit and its exit
  handling. They are `exit_handling_finishes_before_another_process_can_take_the_account`
  (status 0) and
  `a_signal_during_exit_handling_defers_it_to_the_next_launch_and_keeps_the_child_s_code`
  (status 3, the fake's code).
- PASS at once for the other five new tests. They pin what Tasks 4 and 9–12 already hold
  across processes:
  - `a_killed_tagteam_leaves_its_reservation_live_and_the_next_launch_captures_and_merges_back`;
  - `after_a_killed_tagteam_s_session_ends_a_switch_captures_its_rotation_first`;
  - `after_a_killed_tagteam_s_session_ends_the_gate_captures_its_rotation`;
  - `the_last_of_two_sessions_out_captures_a_rotation_either_made_and_merges_back`;
  - `a_reservation_owns_the_account_before_claude_starts`.

- [ ] **Step 6: Implement**

In `crates/tagteam/src/run.rs`, replace `run_session` with the version below. The only change
is the `after-exit` point, after the take that clears what forwarding left:

```rust
/// §12.5 steps 6–7 and "When the child exits", after `launch` and the login check. Spawns
/// `launch` with `args` in `cwd` and `launched`'s environment, with the reservation inherited
/// through its fd. Waits for it, forwarding signals (Decision 1), then runs the exit handling
/// and prints its notices on `err`.
///
/// `Ok` is `claude`'s exit code, `128 + n` when signal `n` ended it, whatever exit handling did
/// (B.63). `Err` is a launch that never started: interrupted at the token's last look, or a
/// spawn that failed. Its exit handling has already run (§12.3).
pub(crate) fn run_session(
    engine: &Engine,
    launched: Launched,
    launch: &Path,
    args: &[OsString],
    cwd: &Path,
    cancel: &Cancel,
    err: &mut dyn Write,
) -> Result<i32, EngineError> {
    // Decision 1: Ctrl-\ is the child's. Registered before the spawn, so the exec gives
    // `claude` the default action back.
    if let Err(e) = signals::survive_quit() {
        let _ = writeln!(err, "warning: Ctrl-\\ may stop tagteam before the session ends: {e}");
    }
    // §12.5: the token's last look. A signal recorded after it is `claude`'s, and goes to it as
    // soon as it exists.
    if let Some(signal) = cancel.take() {
        notify(err, &engine.finish_run(launched, LaunchEnd::Refused));
        return Err(EngineError::Interrupted(signal));
    }
    pause_point("before-spawn");
    let spec = SpawnSpec {
        program: launch.to_path_buf(),
        args: args.to_vec(),
        set: launched.env.set.clone(),
        remove: launched.env.remove.clone(),
        cwd: Some(cwd.to_path_buf()),
    };
    let mut child = match spawn_session(&spec, Some(launched.reservation.fd())) {
        Ok(child) => child,
        Err(e) => {
            let e = EngineError::LaunchUnreachable {
                detail: format!("{}: {e}", launch.display()),
            };
            return Err(abandon(engine, launched, cancel, err, e));
        }
    };
    // The first look forwards anything, SIGINT included: what arrived while `claude` was being
    // spawned, the terminal could not deliver to it.
    forward(child.id(), cancel.take(), true);
    let status = wait(&mut child, cancel);
    // What forwarding left is spent, so exit handling meets only a signal sent from now on.
    let _ = cancel.take();
    pause_point("after-exit");
    let code = match status {
        Ok(status) => exit_code(status),
        Err(e) => {
            let _ = writeln!(err, "tagteam: lost track of the session's exit status: {e}");
            LOST
        }
    };
    notify(err, &engine.finish_run(launched, LaunchEnd::Exited(code)));
    Ok(code)
}
```

and replace `pause_point`'s doc comment's first sentence with "A test-only stop at `name`:
`before-spawn` is right after the token's last look, and `after-exit` is between `claude`'s exit
and its exit handling, while the reservation is still this process's."

- [ ] **Step 7: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam --features test-support --test run_cli`
Expected: PASS, every test: Task 3's nine, Task 12's twenty and these seven.

Run: `cargo test -p tagteam --features test-support`
Expected: PASS.

- [ ] **Step 8: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 9: Commit**

```bash
git add crates/tagteam/src/run.rs crates/tagteam/tests/run_cli.rs
git commit -m "Pin a killed tagteam's live reservation, two sessions of one account, and the launch and exit races"
```

#### 13c: The provenance sequence on a real keychain (macOS)

- [ ] **Step 10: Write the failing test**

Create `crates/tagteam-engine/tests/run_real_keychain.rs`:

```rust
//! §15.2 "Provenance" with the real `security` driver (macOS): a session's exit, an inactive
//! vault refresh, a relaunch, and the capture check at its exit end with the vault's generation
//! effective in the profile, and never capture the consumed one. Every item lives in a
//! throwaway keychain file, never the login keychain.
#![cfg(all(target_os = "macos", feature = "real_keychain"))]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use common::{CLAUDE_JSON, FixedOracle, claude_bin, splice_oauth_account};
use serde_json::{Value, json};
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::{CcPaths, ClaudeCode, ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::launch::LaunchEnd;
use tagteam_engine::lifecycle::AddOptions;
use tagteam_engine::refresh::GateOutcome;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::AccountRow;
use tagteam_engine::vault::{KeychainVault, SERVICE, Vault};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_provider::http::Method;
use tagteam_provider::liveness::FakeProcessProbe;
use tagteam_provider::process::{Captured, ScriptedSpawner};
use tagteam_provider::profile::RunShell;
use tagteam_provider::security::{ProcessRunner, SecurityCli};
use tagteam_provider::{Env, FakeClock, Keychain, Provider, Read, ScriptedHttp};

/// A keychain file made for this test and deleted with it.
struct TempKeychain {
    path: PathBuf,
    _dir: tempfile::TempDir,
}

impl TempKeychain {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.keychain");
        for args in [
            ["create-keychain", "-p", "pw", path.to_str().unwrap()],
            ["unlock-keychain", "-p", "pw", path.to_str().unwrap()],
        ] {
            assert!(Command::new("/usr/bin/security").args(args).status().unwrap().success());
        }
        Self { path, _dir: dir }
    }
}

impl Drop for TempKeychain {
    fn drop(&mut self) {
        let _ = Command::new("/usr/bin/security")
            .args(["delete-keychain", self.path.to_str().unwrap()])
            .status();
    }
}

/// A credential of `rt`'s lineage, with a machine-shared MCP token when `mcp` is given.
fn credential(rt: &str, mcp: Option<&str>) -> Vec<u8> {
    let mut v = json!({
        "claudeAiOauth": {
            "accessToken": format!("at-{rt}"),
            "refreshToken": rt,
            "expiresAt": 1_790_003_600_000i64,
            "refreshTokenExpiresAt": 1_797_000_000_000i64,
            "scopes": ["user:inference", "user:profile"],
            "subscriptionType": "max"
        }
    });
    if let Some(token) = mcp {
        v["mcpOAuth"] = json!({"srv": {"token": token}});
    }
    v.to_string().into_bytes()
}

fn refresh_token(bytes: &[u8]) -> String {
    let v: Value = serde_json::from_slice(bytes).unwrap();
    v["claudeAiOauth"]["refreshToken"].as_str().unwrap().to_owned()
}

/// One engine whose Claude Code provider and vault both drive the real `security` against a
/// throwaway keychain; the network and the `claude` spawn are scripted.
struct Real {
    env: Env,
    kc: Arc<dyn Keychain>,
    cc: Arc<ClaudeCode>,
    http: Arc<ScriptedHttp>,
    spawner: Arc<ScriptedSpawner>,
    engine: Engine,
    home: tempfile::TempDir,
    _keychain: TempKeychain,
}

impl Real {
    fn new() -> Self {
        let keychain = TempKeychain::new();
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path());
        let claude = env.home.join(".claude");
        fs::create_dir_all(claude.join("projects")).unwrap();
        fs::write(claude.join("history.jsonl"), "").unwrap();
        fs::write(CcPaths::resolve(&env).global_config, CLAUDE_JSON).unwrap();
        let kc: Arc<dyn Keychain> = Arc::new(SecurityCli::with_runner(
            Box::new(ProcessRunner),
            Some(keychain.path.clone()),
        ));
        let cc = Arc::new(ClaudeCode::with_store(
            LiveStore::new(kc.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO),
        ));
        let http = Arc::new(ScriptedHttp::new());
        let spawner = Arc::new(ScriptedSpawner::new());
        let engine = Engine::new(EngineConfig {
            env: env.clone(),
            registry: ProviderRegistry::new().with(cc.clone()),
            vault: Vault::new(Box::new(KeychainVault::new(kc.clone()))),
            oracle: Arc::new(FixedOracle::default()),
            clock: Arc::new(FakeClock::new(1_790_000_000_000)),
            http: http.clone(),
            default_provider: ProviderId::new(CLAUDE_CODE),
            settings: Settings::default(),
            process: Arc::new(FakeProcessProbe::new()),
            run_shell: RunShell::Outside,
            spawner: spawner.clone(),
        });
        Real {
            env,
            kc,
            cc,
            http,
            spawner,
            engine,
            home,
            _keychain: keychain,
        }
    }

    /// What `claude /login` as `email` leaves in the default home, then `tagteam add`.
    fn add(&self, email: &str, rt: &str) -> AccountId {
        splice_oauth_account(
            &CcPaths::resolve(&self.env).global_config,
            &json!({"emailAddress": email, "organizationUuid": "", "organizationName": null,
                    "accountUuid": format!("uuid-{email}")}),
        );
        self.kc
            .upsert(
                &keychain_service(&self.env, ItemKind::OAuth),
                &keychain_account(&self.env),
                &credential(rt, Some("mcp-default")),
            )
            .unwrap();
        self.engine
            .add_live(AddOptions {
                provider: ProviderId::new(CLAUDE_CODE),
                position: None,
                alias: None,
                yes: false,
            })
            .unwrap()
            .account
            .id
    }

    fn row(&self, id: &AccountId) -> AccountRow {
        self.engine.store().unwrap().account(id).unwrap().unwrap()
    }

    fn vault_bytes(&self, id: &AccountId) -> Vec<u8> {
        self.kc.find(SERVICE, id.as_str()).present().unwrap()
    }

    /// The hashed item Claude Code names from `spelling` (Appendix A.2).
    fn profile_item(&self, spelling: &str) -> (String, String) {
        let mut env = self.env.clone();
        env.claude_config_dir = Some(spelling.into());
        env.claude_securestorage_config_dir = None;
        (keychain_service(&env, ItemKind::OAuth), keychain_account(&env))
    }

    /// §12.3's `valid` reply for `email` in `spelling`.
    fn valid(&self, spelling: &Path, email: &str) -> Captured {
        Captured::Exited {
            code: Some(0),
            signal: None,
            stdout: json!({"loggedIn": true, "authMethod": "claude.ai",
                           "configDirectory": spelling.to_str().unwrap(), "email": email})
            .to_string()
            .into_bytes(),
            stderr: vec![],
        }
    }

    /// A 200 token reply rotating to `rt`.
    fn script_refresh(&self, rt: &str) {
        self.http.push_json(
            Method::Post,
            &Endpoints::production().token,
            200,
            json!({"token_type": "Bearer", "access_token": format!("at-{rt}"),
                   "refresh_token": rt, "expires_in": 28800,
                   "scope": "user:inference user:profile"}),
        );
    }
}

#[test]
#[ignore = "real Keychain: run with --features real_keychain -- --ignored, outside the sandbox"]
fn a_relaunch_after_an_inactive_refresh_runs_on_the_vault_s_generation_and_never_captures_the_consumed_one()
 {
    let r = Real::new();
    let a = r.add("a@x.co", "rt-a");
    r.add("b@x.co", "rt-b");
    let cwd = r.home.path().join("work");
    fs::create_dir_all(&cwd).unwrap();
    let spelling = fs::canonicalize(r.env.data_dir())
        .unwrap()
        .join("sessions")
        .join(a.as_str());

    // 1. The first session's bootstrap writes the file and leaves no item (§12.3 steps 4–5).
    r.spawner.push(r.valid(&spelling, "a@x.co"));
    let launched = r.engine.launch(&r.row(&a), claude_bin(), &cwd).unwrap();
    let profile = launched.profile.clone();
    let (svc, acct) = r.profile_item(&launched.spelling);
    assert!(matches!(r.kc.exists(&svc, &acct), Read::Absent));
    // 2. Claude Code's first credential write in the session rotates it and moves the file
    //    into its hashed item (Appendix A.3), with an MCP token of the profile's own.
    r.kc.upsert(&svc, &acct, &credential("rt-a1", Some("mcp-profile")))
        .unwrap();
    fs::remove_file(profile.join(".credentials.json")).unwrap();
    // 3. Its exit captures the rotation (§12.5).
    r.engine.finish_run(launched, LaunchEnd::Exited(0));
    assert_eq!(refresh_token(&r.vault_bytes(&a)), "rt-a1");

    // 4. A refresh of the inactive account consumes rt-a1 (§7.3).
    r.script_refresh("rt-a2");
    let snapshot = r.vault_bytes(&a);
    assert!(matches!(
        r.engine.refresh_stored(r.cc.as_ref(), &a, &snapshot).unwrap(),
        GateOutcome::Refreshed(_)
    ));
    assert_eq!(refresh_token(&r.vault_bytes(&a)), "rt-a2");

    // 5. The relaunch finds the vault moved on (§12.5's table: P = S). It bootstraps again and
    //    deletes the item holding the consumed generation, verified gone by the real probe.
    r.spawner.push(r.valid(&spelling, "a@x.co"));
    let launched = r.engine.launch(&r.row(&a), claude_bin(), &cwd).unwrap();
    assert!(matches!(r.kc.exists(&svc, &acct), Read::Absent));
    let effective = r
        .cc
        .read_profile_credential(&r.env, &launched.profile, &launched.spelling)
        .present()
        .expect("the profile's credential");
    let v: Value = serde_json::from_slice(effective.bytes()).unwrap();
    assert_eq!(
        v["claudeAiOauth"]["refreshToken"],
        json!("rt-a2"),
        "the vault's generation is what Claude Code reads"
    );
    assert_eq!(
        v["mcpOAuth"]["srv"]["token"],
        json!("mcp-profile"),
        "with the profile's own MCP token (§12.3 step 4)"
    );

    // 6. Its exit finds nothing to capture: the consumed rt-a1 never comes back.
    r.engine.finish_run(launched, LaunchEnd::Exited(0));
    assert_eq!(refresh_token(&r.vault_bytes(&a)), "rt-a2");
}
```

- [ ] **Step 11: Run it and see it fail**

Run: `cargo test -p tagteam-engine --features real_keychain --test run_real_keychain -- --ignored`
Expected: `error: the package 'tagteam-engine' does not contain this feature: real_keychain`.

- [ ] **Step 12: Implement**

In `crates/tagteam-engine/Cargo.toml`, add to `[features]`, after `test-hooks`:

```toml
# §15.2's provenance sequence against a throwaway real keychain (macOS); gates
# tests/run_real_keychain.rs only.
real_keychain = []
```

- [ ] **Step 13: Run it and see it pass**

Run: `cargo test -p tagteam-engine --features real_keychain --test run_real_keychain`
Expected: `0 passed; 0 failed; 1 ignored`.

Run, on a Mac and outside the agent sandbox (the Security daemons are unreachable from it):
`cargo test -p tagteam-engine --features real_keychain --test run_real_keychain -- --ignored`
Expected: PASS, one test, with no GUI prompt: every item is in the throwaway keychain file,
which `security` reads and writes without asking.

Run: `cargo test -p tagteam-engine` (no features)
Expected: PASS; the file compiles to nothing.

- [ ] **Step 14: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p tagteam-engine --all-targets --features real_keychain -- -D warnings
```

- [ ] **Step 15: Commit**

```bash
git add crates/tagteam-engine/Cargo.toml crates/tagteam-engine/tests/run_real_keychain.rs
git commit -m "Run the session provenance sequence against a real keychain file"
```

---

### Task 14: Final verification and live acceptance

**Human steps (Step 3 prepares; Steps 4–5 are the acceptance and Michael's).** The implementer
does Steps 1–2. The live run uses Michael's real Claude Code (2.1.286 or later), his real
logins and his real login keychain. It launches real sessions, captures real rotations, and
removes one account from tagteam before adding it back. Michael therefore runs it, and the merge
waits for it. An agent never runs Steps 3–5.

The whole-branch review and the pre-merge cross-review run after Step 2. They follow the global
workflow and are not steps of this plan. The live run is the last step before the merge request
(Execution notes).

**Files:**
- Modify: `docs/superpowers/plans/2026-10-01-tagteam-m4b-run.md` (the `**Status:**` line,
  Step 7)

**Interfaces:**
- Consumes: everything Tasks 1–13 produced. In particular:
  - `tagteam run [ACCOUNT] [--require-session] [-- <args>]` and its exit codes: the child's,
    `128 + n` after a signal, 1 for a refusal, `128 + n` with `interrupted` before the launch;
  - the error kinds `launch-command-missing`, `api-key-account`, `requires-session`,
    `login-overridden`, `login-invalid`, `login-drifted`, `login-unknown`,
    `launch-unreachable`, `target-changed`, and M4a's `session-owned`;
  - `map`, `unmap`, `shell-init zsh|bash|fish`;
  - the profile's files: `.tagteam-profile.json`, `.tagteam-seed.json`,
    `.tagteam-baseline.json`, `.tagteam-launch/<pid>.lock`.
- Produces: nothing new.

**Spec:**
- §12 (all of it), checked live against Claude Code 2.1.286+ as §15.4 lists for `xtask compat`,
  by hand until M5 builds that task.
- §16: CI's checks (fmt, clippy, tests, `real_keychain` on macOS).
- The global design-record rules: `In progress` while work or review is active, with the MR
  reference; `Implemented` with the MR in the final pre-merge commit.

- [ ] **Step 1: Format, lint and test everything**

Run:
```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --features tagteam/test-support --no-fail-fast 2>&1 | tee "$TMPDIR/m4b-test.log" | grep -E '^test result:|FAILED|panicked'
awk '/^test result:/ {p += $4; f += $6; i += $8} END {printf "%d/%d passed, %d ignored\n", p, p + f, i}' "$TMPDIR/m4b-test.log"
cargo test --workspace --features tagteam/test-support --no-fail-fast -- --ignored 2>&1 | tee "$TMPDIR/m4b-ignored.log" | grep -E '^test result:|FAILED|panicked'
awk '/^test result:/ {p += $4; f += $6} END {printf "%d/%d passed\n", p, p + f}' "$TMPDIR/m4b-ignored.log"
cargo test -p tagteam --lib
```

Expected:
- `fmt --check` prints nothing, and both clippy runs finish with no warning.
- Every `test result:` line says `ok`, and no `FAILED` or `panicked` line appears. The first
  `awk` prints `N/N passed, M ignored` with the two numbers before the slash equal. Report that
  line as the total.
- The `--ignored` run passes every ignored test: M2a's 9 s CC-lock test, `gate_race`'s 15 s
  stopped-holder test, and M3a's and M4a's ignored tests. The perf tests exist only in release
  builds, and run below. Task 13's real-keychain test needs its own feature, so it is not in
  this run either. Its `awk` prints `K/K passed`.
- `cargo test -p tagteam --lib` runs without features, so it exercises the release branch of the
  test-override checks and `run.rs`'s no-op `pause_point`.

On a Mac, run the real Keychain tests. They need the Security daemons, which the agent sandbox
blocks, so they run outside it (a person runs them in a plain terminal):

```bash
cargo test -p tagteam-provider --features real_keychain --test real_keychain
cargo test -p tagteam-engine --features real_keychain --test run_real_keychain -- --ignored
```

Expected: PASS, three tests and one, with no GUI prompt. The second is Task 13's provenance
sequence: session exit, inactive refresh, relaunch, capture.

Run the timing tests, one at a time, on an otherwise idle machine:

```bash
cargo test --release -p tagteam --features test-support --test perf -- --ignored --nocapture --test-threads=1
```

Expected: PASS, the four tests M4a left: `statusline` p95 ≤ 10 ms on a cache hit, on a cache
miss and in a run shell, and `list` p95 ≤ 50 ms with nothing due. M4b adds `run` to the CLI
but no work to either path. One failure gets one re-run; a second is a real miss of the budget:
stop and report the measured p95.

**The Linux check.** First compile the Linux-only code on the Mac:

Run: `cargo check -p tagteam-core -p tagteam-provider -p tagteam-cc -p tagteam-fake --target x86_64-unknown-linux-gnu`
Expected: `Finished`. This compiles Task 3's `pre_exec` and `killpg` and Task 4's `flock` for
Linux. The engine and the binary are left out, as in M4a: bundled SQLite's build script needs a
Linux C cross-compiler.

Then run the whole suite on Linux in Docker. That is where the fake `claude` runs under
`dash`, and where its 50 ms `sleep` and its traps are first exercised. The run is non-root, as
a user with a passwd entry and a `HOME` outside `/tmp`. `docker` is the first word of its own
Bash call, with no pipe and no redirect; an agent runs it in the background:

```bash
docker run --rm -v /Users/michael/Code/tagteam-m4-run-sessions:/src:ro -v tagteam-target:/target -v tagteam-cargo-registry:/usr/local/cargo/registry -w /src rust:1.88 sh -c 'useradd -m -u 1000 tester && chown -R tester /target /usr/local/cargo && setpriv --reuid=1000 --regid=1000 --clear-groups env HOME=/home/tester USER=tester CARGO_TARGET_DIR=/target cargo test --workspace --features tagteam/test-support --no-fail-fast'
```

Expected: every `test result:` line `ok`. Report its total as passed/total, beside the Mac's.

- [ ] **Step 2: Build the release binary and check it carries no test hooks**

```bash
cargo build --release -p tagteam
grep -a -c TAGTEAM_TEST_ target/release/tagteam ; test $? -eq 1
```

Expected:
- The build succeeds.
- `grep` prints `0` and exits 1, so the final `test` exits 0.
- The keychain-directory, platform and API-base overrides exist only under `test-support`. So
  do the engine's crash and pause points and `run.rs`'s `before-spawn` and `after-exit` pause
  points.

- [ ] **Step 3: Michael — prepare the acceptance run**

The goal of the run:
- `run` gives a second account its own working session beside the default one. Memory, history
  and settings are shared; the token is never shared.
- What the session changes in its config comes back on exit.
- Every way a session ends leaves the accounts consistent: a clean exit, a killed `tagteam`, a
  background session that outlives it, Ctrl-C, Ctrl-\, SIGTERM and SIGHUP.
- `map` and `shell-init` make it automatic, and the refusals say why.

No token is handled by hand, and nothing below prints one: the fingerprints are 12 hex digits
of a SHA-256 of the refresh token.

1. From the worktree root, put the Step 2 binary first on `PATH` for this shell, and keep its
   directory for row 7:
   ```bash
   export TT_BIN="$PWD/target/release"
   export PATH="$TT_BIN:$PATH"
   command -v tagteam
   claude --version
   ```
   Expected: `<worktree>/target/release/tagteam`, and Claude Code 2.1.286 or later. The binary is
   new and ad-hoc signed, so if a request is blocked, suspect Little Snitch and allow
   `api.anthropic.com` and `platform.claude.com` for it.
2. Run `tagteam list`. You need two stored accounts. **A** is live (`*`); **B** is another. Note
   their positions as **PA** and **PB**. The rows write `PA` and `PB` where you type those
   numbers.
3. Make scratch directories: `mkdir -p ~/tmp/tt-b ~/tmp/tt-trust ~/tmp/tt-map/sub`.
4. Define these helpers. They print positions, ids, counts and fingerprints, never an email or
   a token:
   ```bash
   # The id, profile, recorded spelling and hashed Keychain item of the account at position $1.
   tt_id() { tagteam list --json | /usr/bin/python3 -c 'import json,sys; n=int(sys.argv[1]); print(next(a["id"] for a in json.load(sys.stdin)["accounts"] if a["position"] == n))' "$1"; }
   tt_prof() { echo "${XDG_DATA_HOME:-$HOME/.local/share}/tagteam/sessions/$(tt_id "$1")"; }
   tt_spell() { /usr/bin/python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["configDir"])' "$(tt_prof "$1")/.tagteam-profile.json"; }
   tt_svc() { echo "Claude Code-credentials-$(printf %s "$(tt_spell "$1")" | shasum -a 256 | cut -c1-8)"; }
   # 12 hex digits of sha256(refresh token): the vault's generation (tt_fp), and the one the
   # profile's claude reads, its item first (tt_pfp).
   tt_fp_of() { /usr/bin/python3 -c 'import json,sys,hashlib; print(hashlib.sha256(json.load(sys.stdin)["claudeAiOauth"]["refreshToken"].encode()).hexdigest()[:12])'; }
   tt_fp() { security find-generic-password -s tagteam -a "$(tt_id "$1")" -w | tt_fp_of; }
   tt_pfp() { { security find-generic-password -s "$(tt_svc "$1")" -a "$USER" -w 2>/dev/null || cat "$(tt_prof "$1")/.credentials.json"; } | tt_fp_of; }
   # What the profile still holds for later: reservations (§12.5) and a baseline (§12.4).
   tt_left() { local p; p=$(tt_prof "$1"); echo "reservations: $(ls "$p/.tagteam-launch" 2>/dev/null | grep -c '\.lock$') baseline: $([ -e "$p/.tagteam-baseline.json" ] && echo yes || echo no)"; }
   # Who is in a session: (position, inSession, active) per account.
   tt_sessions() { tagteam list --json | /usr/bin/python3 -c 'import json,sys; print(sorted((a["position"], a.get("inSession", False), a["active"]) for a in json.load(sys.stdin)["accounts"]))'; }
   # ~/.claude.json apart from projects and mcpServers, as a digest; and the row 2 keys.
   tt_cfg() { /usr/bin/python3 -c 'import json,os,hashlib; d=json.load(open(os.path.expanduser("~/.claude.json"))); [d.pop(k, None) for k in ("projects", "mcpServers")]; print(hashlib.sha256(json.dumps(d, sort_keys=True).encode()).hexdigest()[:12])'; }
   tt_trusted() { /usr/bin/python3 -c 'import json,os,sys; d=json.load(open(os.path.expanduser("~/.claude.json"))); p=os.path.realpath(os.path.expanduser(sys.argv[1])); print("trusted:", (d.get("projects") or {}).get(p, {}).get("hasTrustDialogAccepted"), " mcp:", "tt-accept" in (d.get("mcpServers") or {}))' "$1"; }
   ```
   Never paste what `security … -w` prints: the helpers pipe it straight into the hash.
5. Run `tt_sessions`. Expected: no `True` in the middle column. Keep the line as **S0**.

- [ ] **Step 4: Michael — run the acceptance**

Run every row in this order. "Terminal 2" and "terminal 3" are two more terminals in the same
directory, each with the Step 3 exports and helpers. Record positions, exit codes and the
helpers' output, never an email.

| # | Run | Do | Pass when |
|---|---|---|---|
| 1 | Terminal 1: `claude`. Terminal 2: `cd ~/tmp/tt-b && tagteam run PB`; in the session `/status`, `! tagteam status`, `! tagteam list`. Terminal 3: `tagteam status`, `tagteam list`, `tt_left PB`. Then `/exit` in the session, `echo "exit $?"`, `tt_left PB`, `tt_sessions` | Send a prompt in terminal 1 while the session runs | The session's `/status` shows B's account, with a config directory under `…/tagteam/sessions/`. Terminal 1 still answers, as A. Inside the session, `tagteam status` names A as `Live:` and adds a `This session:` line for PB, and `tagteam list` marks PB `▶ … this` and A `*`. If `~/.claude/settings.json`'s `statusLine` runs `tagteam statusline`, the session's bar shows B and terminal 1's shows A. In terminal 3, `status` names A with no session line, `list` marks PB `▶` without `this`, and `tt_left PB` prints `reservations: 1 baseline: yes`. After `/exit`: `exit 0`, `tt_left PB` prints `reservations: 0 baseline: no`, and `tt_sessions` equals S0 |
| 2 | Quit terminal 1's `claude`. `tt_cfg` (note **C0**). `cd ~/tmp/tt-trust && tagteam run PB`; accept Claude Code's trust prompt; in the session `! claude mcp add --scope user tt-accept -- /bin/echo tt`, then `/exit`. Then `tt_cfg`, `tt_trusted ~/tmp/tt-trust`, `tt_left PB`. Clean up in the default home: `claude mcp remove --scope user tt-accept` | Nothing else may write `~/.claude.json` during the row | `tt_cfg` still prints C0: nothing outside `projects` and `mcpServers` moved. `tt_trusted` prints `trusted: True  mcp: True`: both came back on exit (§12.4). `tt_left PB` prints `reservations: 0 baseline: no` |
| 3 | `tt_fp PB` (note **F0**) and `tt_pfp PB` (also F0). `cd ~/tmp/tt-b && tagteam run PB`; in the session `/login`, and sign in as B again in the browser; `/exit`. Then `tt_fp PB`, `tt_pfp PB`, `tagteam list` | Sign in as B, never another account | `tt_fp PB` now differs from F0 and equals `tt_pfp PB`: the exit captured the profile's new generation into the vault (§12.5). `list` shows PB with no error |
| 4 | Terminal 2: `cd ~/tmp/tt-b && tagteam run PB -- -p "Write a 1500-word story about a lighthouse." > "$TMPDIR/tt-story.txt"; echo "exit $?"`. Terminal 3, within 5 s: `kill -KILL "$(pgrep -x tagteam)"`; then `tt_sessions`, `tagteam switch PB --json`, `tt_left PB`. Once `pgrep -x claude` prints nothing and the story file has text: `tt_sessions`, `cd ~/tmp/tt-b && tagteam run PB -- --version`, `tt_left PB`, `tt_fp PB`, `tt_pfp PB` | No other `tagteam` may run; quit any other `claude` first | Terminal 2 prints `exit 137` at once, and the story still arrives: `claude` runs on. While it runs, `tt_sessions` shows PB `True`, `switch PB --json` fails with `"type":"session-owned"` and nothing switches, and `tt_left PB` prints `reservations: 1 baseline: yes` (Review Focus 1). Once it has finished, `tt_sessions` equals S0. The next `run` prints the version; `tt_left PB` then prints `reservations: 0 baseline: no` (the dead reservation removed, the left-over baseline merged back), and `tt_fp PB` equals `tt_pfp PB` |
| 5 | `cd ~/tmp/tt-b && tagteam run PB -- --bg "Reply with one word."` (2.1.286's background form; if `claude --help` names another, use that). Then `grep -h '"kind"' "$(tt_prof PB)"/sessions/*.json`, `tt_sessions`, `tt_left PB`. Stop the background session: `kill -TERM <pid>`, with the pid from that record. Then `tt_sessions`, `cd ~/tmp/tt-b && tagteam run PB -- --version`, `tt_left PB` | — | The record's kind is `daemon` (or `bg`), with a live pid. After `tagteam run` returned, `tt_sessions` still shows PB `True`: the background session keeps the account session-owned (§12.6). `tt_left PB` prints `reservations: 0 baseline: yes`: not the last out, so capture and merge-back wait. After the kill, `tt_sessions` equals S0. The next `run` prints the version, and `tt_left PB` prints `reservations: 0 baseline: no` |
| 6 | (a) In the default home: `claude`, Ctrl-C twice, `echo "exit $?"` (note **PC**); `claude`, Ctrl-\, `echo "exit $?"` (note **PQ**; `stty sane` if the terminal needs it). (b) `cd ~/tmp/tt-b && tagteam run PB`; Ctrl-C once, then type a short prompt; Ctrl-C twice; `echo "exit $?"`; `tt_left PB`. (c) `tagteam run PB`; Ctrl-\; `echo "exit $?"`; `tt_left PB`. (d) `tagteam run PB` in terminal 2; terminal 3 `kill -TERM "$(pgrep -x tagteam)"`; terminal 2 `echo "exit $?"`, `tt_left PB`. (e) As (d) with `kill -HUP` | — | (b) One Ctrl-C does in the session exactly what it does in plain `claude`, and the session runs on; the double Ctrl-C ends it with `exit` PC, and tagteam prints no `interrupted` line (Review Focus 2). (c) Ctrl-\ does what it does in plain `claude`, and tagteam is still there to report: `exit` PQ, which is 131 if `claude` died of it. (d) `claude` ends as on its own SIGTERM, once, and `exit` is its code (143 if it died of the signal). (e) The same with SIGHUP (129). After each, `tt_left PB` prints `reservations: 0 baseline: no`, and the terminal is as plain `claude` leaves it |
| 7 | `eval "$(tagteam shell-init zsh)"; type claude`. `tagteam shell-init bash \| bash -n && echo bash-ok`; with fish installed, `tagteam shell-init fish \| fish --no-execute && echo fish-ok`. `tagteam map PB ~/tmp/tt-map`; `tagteam map`. `cd ~/tmp/tt-map/sub && claude` → `/status`, `/exit`. `cd ~ && claude` → `/status`, `/exit`. `tagteam unmap ~/tmp/tt-map`; `cd ~/tmp/tt-map && claude` → `/status`, `/exit`. `( PATH="${PATH#"$TT_BIN:"}"; whence -p tagteam \|\| echo "no tagteam"; claude --version )` | — | `type claude` names a shell function, and `bash-ok` (and `fish-ok`) print. `map` lists `~/tmp/tt-map`'s canonical path for PB. In `sub`, `/status` shows B: a subdirectory inherits the mapping (§12.7). In `~`, it shows A, as plain `claude`. After `unmap`, `~/tmp/tt-map` shows A too. With `tagteam` off `PATH`, `whence` prints `no tagteam` and `claude --version` still prints the version: the wrapper falls back to `command claude` |
| 8 | `cd ~ && tagteam run --require-session -- --version; echo "exit $?"`. `tagteam run --require-session PA -- --version; echo "exit $?"`. `tagteam --json run --require-session -- --version`. `cd ~/tmp/tt-b && tagteam run --require-session PB -- --version; echo "exit $?"`. Then `tagteam run PB`, and in the session `! tagteam run --require-session PA -- --version; echo "exit $?"`, then `/exit` | — | The first two refuse with a `tagteam:` line saying why (no mapping; PA is the live login), `exit 1`, and print no version. The `--json` one prints exactly one object with `"type":"requires-session"`. PB prints the version, `exit 0`. Inside the session, PA refuses the same way: `run` decides from the outer home, where A is live (§12.1, §12.8) |
| 9 | Quit every `claude`. `cp ~/.claude/settings.json "$TMPDIR/tt-settings.json"`, then `/usr/bin/python3 -c 'import json,os; p=os.path.expanduser("~/.claude/settings.json"); d=json.load(open(p)); d["apiKeyHelper"]="/bin/echo not-a-key"; json.dump(d, open(p, "w"), indent=2)'`. `cd ~/tmp/tt-b && tagteam run PB -- --version; echo "exit $?"`; `ls -d "$(tt_prof PB)"`. Restore at once: `cp "$TMPDIR/tt-settings.json" ~/.claude/settings.json`. Then `tagteam run PB -- --version; echo "exit $?"` | Restore `settings.json` before anything else: while the helper is in it, the default home's `claude` would use it too | The run refuses: `exit 1`, a `tagteam:` line naming `api_key_helper` (and `apiKeySource` when Claude Code gives one), and no version (Review Focus 5). `ls` shows the profile is still there: only `invalid` deletes one (B.29). After the restore, the run prints the version, `exit 0` |
| 10 | Terminal 2: `cd ~/tmp/tt-b && tagteam run PB`. Terminal 3: `tagteam remove PB --json; echo "exit $?"`. Quit the session. Then `P=$(tt_prof PB); S=$(tt_svc PB)`; `security find-generic-password -s "$S" -a "$USER" >/dev/null 2>&1; echo "item $?"`; `tagteam remove PB`; `ls -d "$P"`; `security find-generic-password -s "$S" -a "$USER" >/dev/null 2>&1; echo "item $?"`; `tagteam list` | This removes B from tagteam; row 11 adds it back | While the session runs, `remove` fails with `"type":"session-owned"`, `exit 1`, and B stays (B.31). The first `item` check prints `item 0` (Claude Code moved the profile's credential into its item) or `item 44`. After `remove`, `ls` reports no such directory, the second check prints `item 44`, and `list` no longer shows B |
| 11 | In the default home: `claude`, `/login` as B, `/exit`; `tagteam add --position PB`; `tagteam switch PA`; `tt_sessions` | — | `tt_sessions` equals S0: A live at PA, B back at PB, no session |

- [ ] **Step 5: Michael — decide and record**

- **Pass (rows 1–11):** keep the table for the merge request description, with the exit codes,
  `tt_left` lines and positions, plus the Step 1 totals as passed/total (Mac, and Linux in
  Docker). Never record a token or an email; record row 3's fingerprints only as "changed, and
  equal to the profile's". Then continue with Step 6.
- **Fail:** stop. Do not open or merge the merge request. Bring the table to Michael. Where to
  look:
  - Row 1: the launch and links (Tasks 9, 10, 12) and M4a's views (its Tasks 14, 15).
  - Row 2: the seed, the three-way merge and the merge-back (Tasks 5, 11).
  - Row 3: exit handling's capture (Task 11) and M4a's provenance (its Task 10).
  - Row 4: reservations (Task 4), the launch's dead-reservation and baseline steps (Task 10),
    and the kill paths (Task 13).
  - Row 5: M4a's session records and liveness (its Task 6) and exit handling's "last one out"
    (Task 11).
  - Row 6: the signal handling (Task 12).
  - Row 7: mappings and the wrappers (Tasks 1, 2).
  - Row 8: the launch decision (Task 8).
  - Row 9: validation (Tasks 7, 11).
  - Row 10: M4a's destructive guard and profile removal (its Task 9).

- [ ] **Step 6: Open the merge request**

The driver pushes the branch and opens the Draft MR, with the Step 5 table in its description
under the repository's template rules. This is not an agent command of this plan.

- [ ] **Step 7: The plan's status**

Run: `grep -n '^\*\*Status:\*\*' docs/superpowers/plans/2026-10-01-tagteam-m4b-run.md`

Expected: `3:**Status:** In progress`. The Execution notes set it when execution started, and it
stays so while the work and its reviews are active.
- If it still says `Approved`, set it to `**Status:** In progress` and commit that alone:
  `git commit -m "Mark the M4b plan in progress"`.
- Once the Draft MR is open, record it on the same line, `**Status:** In progress — <MR URL>`,
  in one commit: `git commit -m "Record the M4b merge request on the plan"`.
- The final pre-merge commit, made once review has converged and merging is the next action,
  sets `**Status:** Implemented — <MR URL>`:
  ```bash
  git add docs/superpowers/plans/2026-10-01-tagteam-m4b-run.md
  git commit -m "Mark the M4b plan implemented"
  ```

The spec stays `In progress` until M5. The driver pushes each of these commits to the open MR.
